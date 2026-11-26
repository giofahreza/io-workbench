impl AgentRuntimeManager {
    async fn start_claude_bridge(
        &self,
        mut context: AgentStartContext,
        client: claude_bridge::Client,
    ) -> Result<()> {
        context.native_rollout_owned_by_provider = true;
        let key = agent_run_key(context.provider, &context.session_id);
        let (abort_tx, mut abort_rx) = oneshot::channel();
        self.register(key.clone(), abort_tx).await?;
        self.publish(
            &context.hub,
            &key,
            WsServerEvent::SessionStatus {
                provider: context.provider,
                session_id: context.session_id.clone(),
                status: iowb_protocol::SessionRuntimeStatus::Starting,
                response_id: Some(context.response_id.clone()),
                sequence: Some(context.next_sequence()),
                latest_user_prompt: Some(context.prompt.clone()),
            },
        )
        .await;
        persist_native_session_id(&context, Some(client.manifest.session_id.clone())).await;
        let manager = self.clone();
        tokio::spawn(async move {
            let run_id = context
                .durable_run_id
                .clone()
                .unwrap_or_else(|| context.response_id.clone());
            let result = async {
                if context.mode.is_some() || context.thinking.is_some() || context.fast.is_some() {
                    return Err(CoreError::Conflict("Keep native Claude session settings while attached. Change permission, thinking, or fast settings in the terminal before sending.".into()));
                }
                if let Some(id) = context.durable_run_id.as_deref() {
                    context.storage.record_native_run_submission(id, "claude_bridge", None)?;
                }
                let submission = claude_bridge::Submission {
                    id: run_id.clone(), prompt: context.prompt.clone(), model: context.model.clone(), effort: context.effort.clone(), ..Default::default()
                };
                let mut receipt = client.submit(&submission).await?;
                let mut output = String::new();
                let mut activities = 0;
                let mut aborted = false;
                let mut associated = None;
                loop {
                    if receipt.turn_id != associated {
                        associated = receipt.turn_id.clone();
                        if let Some(id) = context.durable_run_id.as_deref() {
                            context.storage.record_native_run_submission(id, "claude_bridge", associated.as_deref())?;
                            if let Some(native_id) = receipt.user_message_id.as_deref()
                                && let Some(run) = context.storage.get_durable_chat_run(id)?
                                && let Some(message_id) = run.user_message_id.as_deref()
                            {
                                context.storage.merge_message_metadata(&context.session_id, message_id, serde_json::json!({"nativeUserUuid":native_id}))?;
                            }
                        }
                        if associated.is_some() {
                            manager.publish(&context.hub, &key, WsServerEvent::SessionStatus {
                                provider: context.provider, session_id: context.session_id.clone(),
                                status: iowb_protocol::SessionRuntimeStatus::Running,
                                response_id: Some(context.response_id.clone()), sequence: Some(context.next_sequence()),
                                latest_user_prompt: Some(context.prompt.clone()),
                            }).await;
                        }
                    }
                    if receipt.output.starts_with(&output) {
                        let delta = receipt.output[output.len()..].to_string();
                        if !delta.is_empty() { publish_agent_output(&manager, &context, &key, &mut output, delta).await; }
                    }
                    for activity in receipt.activity.iter().skip(activities) {
                        manager.publish(&context.hub, &key, WsServerEvent::Output {
                            provider: context.provider, session_id: context.session_id.clone(), content: activity.clone(), done: false,
                            response_id: Some(context.response_id.clone()), sequence: Some(context.next_sequence()),
                        }).await;
                    }
                    activities = receipt.activity.len();
                    if receipt.terminal() { break; }
                    tokio::select! {
                        _ = &mut abort_rx, if !aborted => { client.abort(&run_id).await?; aborted = true; }
                        _ = tokio::time::sleep(Duration::from_millis(150)) => {}
                    }
                    receipt = client.receipt(&run_id).await?;
                }
                Ok::<_, CoreError>((receipt, output))
            }.await;
            match result {
                Ok((receipt, output))
                    if receipt.status == "completed" || receipt.status == "aborted" =>
                {
                    let status = if receipt.status == "aborted" {
                        iowb_protocol::SessionRuntimeStatus::Aborted
                    } else {
                        iowb_protocol::SessionRuntimeStatus::Completed
                    };
                    let mut normalizer = ClaudeLiveOutputNormalizer::default();
                    normalizer.normalize_line(&serde_json::json!({"type":"result","result":receipt.answer,"usage":receipt.usage}).to_string());
                    let answer = if receipt.answer.is_empty() {
                        output
                    } else {
                        receipt.answer
                    };
                    manager
                        .finish(
                            &key,
                            &context,
                            status,
                            Some(answer),
                            normalizer.take_final_usage(),
                        )
                        .await;
                }
                Ok((receipt, _)) => {
                    let message = receipt.error.unwrap_or_else(|| format!("Claude bridge ended with {}. Inspect the native conversation before retrying.", receipt.status));
                    if receipt.status == "rejected" {
                        manager
                            .reject_unaccepted_claude_send(&key, &context, &message)
                            .await;
                    } else {
                        manager
                            .publish(
                                &context.hub,
                                &key,
                                WsServerEvent::Error {
                                    history_message: None,
                                    message: message.clone(),
                                    details: None,
                                    session_id: Some(context.session_id.clone()),
                                },
                            )
                            .await;
                    }
                    manager
                        .finish(
                            &key,
                            &context,
                            iowb_protocol::SessionRuntimeStatus::Failed,
                            None,
                            None,
                        )
                        .await;
                }
                Err(error) => {
                    let message = error.to_string();
                    // A transport error after submission is ambiguous. Do not
                    // remove its prompt or launch a fallback print process.
                    manager
                        .publish(
                            &context.hub,
                            &key,
                            WsServerEvent::Error {
                                history_message: None,
                                message: message.clone(),
                                details: None,
                                session_id: Some(context.session_id.clone()),
                            },
                        )
                        .await;
                    manager
                        .finish(
                            &key,
                            &context,
                            iowb_protocol::SessionRuntimeStatus::Failed,
                            None,
                            None,
                        )
                        .await;
                }
            }
        });
        Ok(())
    }

    async fn reject_unaccepted_claude_send(
        &self,
        key: &str,
        context: &AgentStartContext,
        message: &str,
    ) {
        let Some(run_id) = context.durable_run_id.as_deref() else {
            return;
        };
        let Ok(Some(run)) = context.storage.get_durable_chat_run(run_id) else {
            return;
        };
        if !context
            .storage
            .reject_native_run_before_acceptance(run_id, message)
            .unwrap_or(false)
        {
            return;
        }
        let entry = ChatMessage {
            id: format!("chat-error:{run_id}"),
            role: MessageRole::System,
            content: message.into(),
            timestamp: Utc::now(),
            metadata: serde_json::json!({"kind":"chat_error","phase":"rejected","status":"failed","code":"claude_send_rejected","requestId":context.request_id,"action":"retry","durableRunId":run_id}),
        };
        let history_message = context
            .storage
            .get_session_summary(&context.session_id)
            .ok()
            .flatten()
            .and_then(|session| {
                context
                    .storage
                    .persist_chat_error(
                        &session,
                        &entry,
                        run.user_id
                            .as_deref()
                            .map(|user| (user, run.prompt.as_str())),
                    )
                    .ok()
                    .map(|_| entry)
            });
        self.publish(
            &context.hub,
            key,
            WsServerEvent::ChatSetupRequired {
                history_message,
                provider: Provider::Claude,
                session_id: context.session_id.clone(),
                request_id: context.request_id.clone(),
                code: "claude_send_rejected".into(),
                message: message.into(),
                action: "retry".into(),
                docs_url: None,
            },
        )
        .await;
    }
}

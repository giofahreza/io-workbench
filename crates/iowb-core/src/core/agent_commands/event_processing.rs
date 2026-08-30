fn websocket_text_chunks(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut offset = 0;
    while offset < value.len() {
        let remaining = &value[offset..];
        let length = utf8_prefix_boundary(remaining, AGENT_WEBSOCKET_CHUNK_MAX_BYTES);
        if length == 0 {
            break;
        }
        chunks.push(remaining[..length].to_string());
        offset += length;
    }
    chunks
}

async fn process_agent_event(
    manager: &AgentRuntimeManager,
    context: &AgentStartContext,
    key: &str,
    event: AgentProcessEvent,
    codex_normalizer: &mut Option<CodexLiveOutputNormalizer>,
    claude_normalizer: &mut Option<ClaudeLiveOutputNormalizer>,
    gemini_normalizer: &mut Option<GeminiLiveOutputNormalizer>,
    output: &mut String,
) {
    match event {
        AgentProcessEvent::Output { stream, data } => {
            if stream == AgentOutputStream::Stdout
                && let Some(normalizer) = claude_normalizer.as_mut()
            {
                let visible_chunks = normalizer.push_chunks(&data);
                let native_session_id = normalizer.take_session_id();
                persist_native_session_id(context, native_session_id).await;
                for visible in visible_chunks {
                    publish_agent_output(manager, context, key, output, visible).await;
                }
                return;
            }
            let (visible, native_session_id) = if stream == AgentOutputStream::Stdout {
                if let Some(normalizer) = codex_normalizer.as_mut() {
                    let visible = normalizer.push(&data);
                    (visible, normalizer.take_thread_id())
                } else if let Some(normalizer) = gemini_normalizer.as_mut() {
                    let visible = normalizer.push(&data);
                    (visible, normalizer.take_session_id())
                } else {
                    (data, None)
                }
            } else {
                (data, None)
            };
            persist_native_session_id(context, native_session_id).await;
            publish_agent_output(manager, context, key, output, visible).await;
        }
        AgentProcessEvent::Failed(message) => {
            manager
                .publish(
                    &context.hub,
                    key,
                    WsServerEvent::Error {
                        message: "agent output stream failed".to_string(),
                        details: Some(message),
                        session_id: Some(context.session_id.clone()),
                    },
                )
                .await;
        }
    }
}

async fn flush_codex_live_output(
    manager: &AgentRuntimeManager,
    context: &AgentStartContext,
    key: &str,
    codex_normalizer: &mut Option<CodexLiveOutputNormalizer>,
    output: &mut String,
) {
    let Some(normalizer) = codex_normalizer.as_mut() else {
        return;
    };
    let visible = normalizer.finish();
    let native_session_id = normalizer.take_thread_id();
    persist_native_session_id(context, native_session_id).await;
    publish_agent_output(manager, context, key, output, visible).await;
}

async fn flush_claude_live_output(
    manager: &AgentRuntimeManager,
    context: &AgentStartContext,
    key: &str,
    claude_normalizer: &mut Option<ClaudeLiveOutputNormalizer>,
    output: &mut String,
) {
    let Some(normalizer) = claude_normalizer.as_mut() else {
        return;
    };
    let visible = normalizer.finish();
    let native_session_id = normalizer.take_session_id();
    persist_native_session_id(context, native_session_id).await;
    publish_agent_output(manager, context, key, output, visible).await;
}

async fn flush_gemini_live_output(
    manager: &AgentRuntimeManager,
    context: &AgentStartContext,
    key: &str,
    gemini_normalizer: &mut Option<GeminiLiveOutputNormalizer>,
    output: &mut String,
) {
    let Some(normalizer) = gemini_normalizer.as_mut() else {
        return;
    };
    let visible = normalizer.finish();
    let native_session_id = normalizer.take_session_id();
    persist_native_session_id(context, native_session_id).await;
    publish_agent_output(manager, context, key, output, visible).await;
}

async fn process_codex_app_server_live_event(
    manager: &AgentRuntimeManager,
    context: &AgentStartContext,
    key: &str,
    event: CodexAppServerLiveTurnEvent,
    normalizer: &mut CodexAppServerLiveOutputNormalizer,
    output: &mut String,
) {
    match event {
        CodexAppServerLiveTurnEvent::ThreadAssociated { thread_id } => {
            persist_native_session_id(context, Some(thread_id)).await;
        }
        CodexAppServerLiveTurnEvent::TurnAssociated { turn_id } => {
            debug_assert!(!turn_id.trim().is_empty());
        }
        CodexAppServerLiveTurnEvent::Notification { method, params } => {
            let visible = normalizer.push_notification(&method, &params);
            publish_agent_output(manager, context, key, output, visible).await;
        }
    }
}

async fn finish_codex_app_server_outcome(
    manager: &AgentRuntimeManager,
    key: &str,
    context: &AgentStartContext,
    outcome: CodexAppServerLiveTurnOutcome,
    final_assistant: Option<String>,
    output: &str,
    usage: Option<NormalizedRunUsage>,
    error: Option<CodexTurnError>,
) {
    let CodexAppServerLiveTurnOutcome {
        status,
        turn,
        turn_id,
        ..
    } = outcome;
    debug_assert!(turn_id.as_deref().is_none_or(|id| !id.trim().is_empty()));
    match status {
        CodexAppServerTurnTerminalStatus::Completed => {
            if context.context_rollover_id.is_some() {
                let follow_up = manager
                    .finish(
                        key,
                        context,
                        iowb_protocol::SessionRuntimeStatus::Completed,
                        None,
                        usage,
                    )
                    .await;
                if let Some(follow_up) = follow_up {
                    manager
                        .start_context_rollover_follow_up(context, follow_up)
                        .await;
                }
                return;
            }
            match select_completed_agent_output(Provider::Codex, final_assistant, output, true) {
                Ok(persisted_output) => {
                    manager
                        .finish(
                            key,
                            context,
                            iowb_protocol::SessionRuntimeStatus::Completed,
                            Some(persisted_output),
                            usage,
                        )
                        .await;
                }
                Err(error_output) => {
                    manager
                        .publish(
                            &context.hub,
                            key,
                            WsServerEvent::Error {
                                message:
                                    "Codex completed without a final assistant response"
                                        .to_string(),
                                details: Some(
                                    "The Codex app-server turn completed, but its event stream did not contain a final assistant message. The accumulated transcript was not saved as the reply."
                                        .to_string(),
                                ),
                                session_id: Some(context.session_id.clone()),
                            },
                        )
                        .await;
                    manager
                        .finish(
                            key,
                            context,
                            iowb_protocol::SessionRuntimeStatus::Failed,
                            Some(error_output),
                            usage,
                        )
                        .await;
                }
            }
        }
        CodexAppServerTurnTerminalStatus::Interrupted => {
            manager
                .finish(
                    key,
                    context,
                    iowb_protocol::SessionRuntimeStatus::Aborted,
                    final_assistant
                        .or_else(|| (!output.trim().is_empty()).then(|| output.to_string())),
                    usage,
                )
                .await;
        }
        CodexAppServerTurnTerminalStatus::Failed => {
            let turn_error = error.or_else(|| {
                let turn = turn.as_ref()?;
                let message = app_server_turn_failure_message(turn)
                    .unwrap_or_else(|| "Codex app-server turn failed".to_string());
                Some(codex_app_server_turn_error(turn, &message))
            });
            if let Some(error) = turn_error.as_ref() {
                if let Some(run_id) = context.durable_run_id.as_deref() {
                    let _ = context
                        .storage
                        .update_durable_chat_run_error(run_id, &error.message);
                }
                manager
                    .publish_context_recovery_if_needed(key, context, error)
                    .await;
            }
            let persisted_output = final_assistant
                .or_else(|| (!output.trim().is_empty()).then(|| output.to_string()))
                .or_else(|| turn_error.as_ref().map(|error| error.message.clone()));
            manager
                .finish(
                    key,
                    context,
                    iowb_protocol::SessionRuntimeStatus::Failed,
                    persisted_output,
                    usage,
                )
                .await;
        }
    }
}

async fn persist_codex_tool_messages(
    context: &AgentStartContext,
    normalizer: &mut Option<CodexLiveOutputNormalizer>,
) {
    let Some(normalizer) = normalizer.as_mut() else {
        return;
    };
    // Rollover output is provisional until the marker, native mapping,
    // assistant response, and retry run can commit atomically. Tool rows are
    // intentionally kept ephemeral for this one turn; otherwise a failed or
    // aborted rollover would mutate the visible transcript before activation.
    if context.context_rollover_id.is_some() {
        normalizer.take_tool_messages();
        return;
    }
    persist_normalized_tool_messages(context, normalizer.take_tool_messages()).await;
}

async fn persist_normalized_tool_messages(
    context: &AgentStartContext,
    tool_messages: Vec<NormalizedToolMessage>,
) {
    if context.context_rollover_id.is_some() {
        return;
    }
    for tool in tool_messages {
        let metadata = serde_json::json!({
            "kind": "tool_output",
            "toolName": tool.name,
            "provider": context.provider.as_str(),
            "durableRunId": context.durable_run_id,
            "responseId": context.response_id,
        });
        if let Err(error) = context
            .sessions
            .append_message_with_metadata(
                &context.session_id,
                MessageRole::Tool,
                tool.content,
                Some(metadata),
            )
            .await
        {
            warn!(
                error = %error,
                session_id = %context.session_id,
                "failed to persist bounded Codex tool message"
            );
        }
    }
}

async fn persist_native_session_id(context: &AgentStartContext, native_session_id: Option<String>) {
    let Some(native_session_id) = native_session_id else {
        return;
    };
    if let Some(rollover_id) = context.context_rollover_id.as_deref() {
        persist_attempt_native_session_id(context, &native_session_id);
        let Some(run_id) = context.durable_run_id.as_deref() else {
            warn!(
                session_id = %context.session_id,
                native_session_id = %native_session_id,
                rollover_id,
                "ignored clean native context candidate without a durable retry run"
            );
            return;
        };
        match context.storage.set_context_rollover_candidate(
            rollover_id,
            run_id,
            &native_session_id,
        ) {
            Ok(true) => info!(
                session_id = %context.session_id,
                native_session_id = %native_session_id,
                rollover_id,
                "staged clean native context candidate"
            ),
            Ok(false) => warn!(
                session_id = %context.session_id,
                native_session_id = %native_session_id,
                rollover_id,
                "ignored native context candidate for inactive rollover"
            ),
            Err(error) => warn!(
                error = %error,
                session_id = %context.session_id,
                native_session_id = %native_session_id,
                rollover_id,
                "failed to stage clean native context candidate"
            ),
        }
        return;
    }
    // Once a visible chat has rollover history, only its currently active
    // native mapping is valid. Late thread.started events from an archived
    // process must never replace it, including on the durable run row.
    if context.provider == Provider::Codex {
        match context
            .storage
            .context_native_session_ids(&context.session_id)
        {
            Ok(ids) if !ids.is_empty() => {
                match context.storage.get_session_summary(&context.session_id) {
                    Ok(Some(session))
                        if session.native_session_id.as_deref()
                            == Some(native_session_id.as_str()) => {}
                    Ok(_) => {
                        warn!(
                            session_id = %context.session_id,
                            native_session_id = %native_session_id,
                            "ignored non-active native thread id after context rollover"
                        );
                        return;
                    }
                    Err(error) => {
                        warn!(error = %error, session_id = %context.session_id, "failed to validate native thread id after context rollover");
                        return;
                    }
                }
            }
            Ok(_) => {}
            Err(error) => {
                warn!(error = %error, session_id = %context.session_id, "failed to inspect context rollover history");
                return;
            }
        }
    }
    persist_attempt_native_session_id(context, &native_session_id);
    if let Some(run_id) = context.durable_run_id.as_deref()
        && let Err(error) = context
            .storage
            .update_durable_chat_run_native_session_id(run_id, Some(&native_session_id))
    {
        warn!(
            error = %error,
            run_id,
            session_id = %context.session_id,
            native_session_id = %native_session_id,
            "failed to persist native provider thread id on durable run"
        );
    }
    match context
        .sessions
        .set_native_session_id(&context.session_id, native_session_id.clone())
        .await
    {
        Ok(_) => {
            if context.native_rollout_owned_by_provider
                && let Err(error) = context
                    .sessions
                    .set_native_rollout_owned_by_provider(&context.session_id, true)
                    .await
            {
                warn!(
                    error = %error,
                    session_id = %context.session_id,
                    native_session_id = %native_session_id,
                    provider = context.provider.as_str(),
                    "failed to persist provider-owned native rollout marker"
                );
            }
            info!(
                session_id = %context.session_id,
                native_session_id = %native_session_id,
                provider = context.provider.as_str(),
                "associated workbench session with native provider thread"
            );
        }
        Err(error) => warn!(
            error = %error,
            session_id = %context.session_id,
            native_session_id = %native_session_id,
            provider = context.provider.as_str(),
            "failed to persist native provider thread id"
        ),
    }
}

fn persist_attempt_native_session_id(context: &AgentStartContext, native_session_id: &str) {
    if let Some(attempt_id) = context.attempt_id.as_deref()
        && let Err(error) = context
            .storage
            .update_chat_run_attempt_native_session_id(attempt_id, native_session_id)
    {
        warn!(
            error = %error,
            attempt_id,
            session_id = %context.session_id,
            native_session_id = %native_session_id,
            "failed to persist native provider thread id on chat run attempt"
        );
    }
}

impl AppState {
    async fn run_manual_context_compaction(&self, task: ManualContextCompactionTask) {
        let compact_result = {
            let _mutation = self.codex_app_server_mutation.lock().await;
            self.codex_app_server
                .compact_thread_and_wait_with_options(
                    &task.native_session_id,
                    task.app_server_options.as_ref(),
                )
                .await
        };
        if let Err(error) = compact_result {
            let message = error.to_string();
            let _ = self
                .storage
                .fail_context_rollover(&task.rollover_id, &message);
            let _ = self
                .storage
                .mark_durable_chat_run_failed(&task.retry_run_id, &message);
            let _ = self.sessions.set_active(&task.session.id, false).await;
            let _ = self.storage.finish_chat_run_attempt(
                &task.attempt_id,
                runtime_status_label(iowb_protocol::SessionRuntimeStatus::Failed),
                None,
                None,
                Some("codex_app_server"),
                TokenUsageCompleteness::Missing,
            );
            self.ws_hub.publish(WsServerEvent::Error {
                message: "Codex context compaction failed".to_string(),
                details: Some(message),
                session_id: Some(task.session.id.clone()),
            });
            self.ws_hub.publish(WsServerEvent::SessionStatus {
                provider: Provider::Codex,
                session_id: task.session.id.clone(),
                status: iowb_protocol::SessionRuntimeStatus::Failed,
                response_id: Some(task.retry_run_id.clone()),
                sequence: None,
                latest_user_prompt: None,
            });
            self.ws_hub.publish(WsServerEvent::ActiveSessions {
                sessions: self.sessions.list_active().await,
            });
            warn!(
                error = %error,
                session_id = %task.session.id,
                rollover_id = %task.rollover_id,
                "manual Codex context compaction failed"
            );
            return;
        }
        let mut session = task.session.clone();
        session.native_session_id = Some(task.native_session_id.clone());
        session.external = false;
        let context = AgentStartContext {
            provider: Provider::Codex,
            session_id: task.session.id.clone(),
            durable_run_id: Some(task.retry_run_id.clone()),
            attempt_id: Some(task.attempt_id.clone()),
            response_id: task.retry_run_id.clone(),
            sequence: Arc::new(AtomicU64::new(0)),
            project_path: PathBuf::from(&session.project_path),
            prompt: task.handoff.clone(),
            model: task.compact_run.model.clone(),
            runtime: task.runtime,
            effort: task.compact_run.effort.clone(),
            mode: task.compact_run.mode.clone(),
            thinking: task.compact_run.thinking,
            fast: task.compact_run.fast,
            native_resume_session_id: Some(task.native_session_id.clone()),
            native_rollout_owned_by_provider: false,
            context_rollover_id: Some(task.rollover_id.clone()),
            direct_ai_config: None,
            direct_ai_messages: Vec::new(),
            sessions: self.sessions.clone(),
            storage: self.storage.clone(),
            hub: self.ws_hub.clone(),
        };
        if let Err(error) =
            activate_completed_context_rollover(&context, &task.rollover_id, Utc::now()).await
        {
            let message = format!("failed to activate native Codex compaction: {error}");
            let _ = self
                .storage
                .fail_context_rollover(&task.rollover_id, &message);
            let _ = self
                .storage
                .mark_durable_chat_run_failed(&task.retry_run_id, &message);
            let _ = self.sessions.set_active(&task.session.id, false).await;
            let _ = self.storage.finish_chat_run_attempt(
                &task.attempt_id,
                runtime_status_label(iowb_protocol::SessionRuntimeStatus::Failed),
                None,
                None,
                Some("codex_app_server"),
                TokenUsageCompleteness::Missing,
            );
            self.ws_hub.publish(WsServerEvent::Error {
                message: "Codex context compaction could not be activated".to_string(),
                details: Some(message),
                session_id: Some(task.session.id.clone()),
            });
            self.ws_hub.publish(WsServerEvent::SessionStatus {
                provider: Provider::Codex,
                session_id: task.session.id.clone(),
                status: iowb_protocol::SessionRuntimeStatus::Failed,
                response_id: Some(task.retry_run_id.clone()),
                sequence: None,
                latest_user_prompt: None,
            });
            self.ws_hub.publish(WsServerEvent::ActiveSessions {
                sessions: self.sessions.list_active().await,
            });
            warn!(
                error = %error,
                session_id = %task.session.id,
                rollover_id = %task.rollover_id,
                "manual Codex context compaction activation failed"
            );
            return;
        }
        let _ = self.storage.finish_chat_run_attempt(
            &task.attempt_id,
            runtime_status_label(iowb_protocol::SessionRuntimeStatus::Completed),
            None,
            None,
            Some("codex_app_server"),
            TokenUsageCompleteness::Missing,
        );
        self.publish_session_metadata_snapshot(
            Provider::Codex,
            &task.session.id,
            Some(task.retry_run_id.clone()),
        )
        .await;
        self.ws_hub.publish(WsServerEvent::SessionStatus {
            provider: Provider::Codex,
            session_id: task.session.id.clone(),
            status: iowb_protocol::SessionRuntimeStatus::Completed,
            response_id: Some(task.retry_run_id.clone()),
            sequence: None,
            latest_user_prompt: None,
        });
        self.ws_hub.publish(WsServerEvent::ActiveSessions {
            sessions: self.sessions.list_active().await,
        });
    }

    async fn publish_session_metadata_snapshot(
        &self,
        provider: Provider,
        session_id: &str,
        response_id: Option<String>,
    ) {
        let session = match self.storage.get_session(session_id) {
            Ok(Some(session)) => session,
            Ok(None) => return,
            Err(error) => {
                warn!(
                    error = %error,
                    session_id,
                    "failed to load session metadata snapshot"
                );
                return;
            }
        };
        self.ws_hub.publish(WsServerEvent::SessionMetadata {
            provider,
            session_id: session.id,
            model: session.model,
            effort: session.effort,
            mode: session.mode,
            thinking: session.thinking,
            fast: session.fast,
            native_session_id: session.native_session_id,
            received_at: session.received_at.unwrap_or_else(Utc::now),
            last_message_at: session.last_message_at,
            first_user_at: session.first_user_at,
            token_usage: session.token_usage,
            lifetime_token_usage: session.lifetime_token_usage,
            context_token_usage: session.context_token_usage,
            spent_token_usage: session.spent_token_usage,
            response_id,
            sequence: None,
        });
    }

    async fn codex_native_session_id_for_compaction(
        &self,
        session: &SessionSummary,
    ) -> Result<Option<String>> {
        if session.provider != Provider::Codex {
            return Ok(None);
        }
        if let Some(native_session_id) = session
            .native_session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(Some(native_session_id.to_string()));
        }
        if session.external {
            return Ok(Some(session.id.clone()));
        }
        self.sessions
            .infer_native_session_id(&session.id, Provider::Codex, &session.project_path)
            .await
    }

    pub async fn context_recovery(&self, session_id: &str) -> Result<Option<ChatContextRecovery>> {
        if let Some(rollover) = self.storage.latest_context_rollover(session_id)? {
            if rollover.kind != CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN {
                return Ok(None);
            }
            match rollover.state.as_str() {
                "starting" | "failed" => {
                    return Ok(Some(ChatContextRecovery {
                        code: "context_too_large".to_string(),
                        state: rollover.state,
                        message: rollover.error.unwrap_or_else(|| {
                            "This chat needs a clean native context before it can continue."
                                .to_string()
                        }),
                        failed_message_id: rollover.failed_message_id,
                        observed_bytes: rollover.observed_bytes,
                        limit_bytes: rollover.limit_bytes,
                        request_id: Some(rollover.request_id),
                    }));
                }
                // An active rollover is historical. A later turn in this same
                // visible chat may independently hit the gateway limit and
                // must be allowed to surface another recovery operation.
                _ => {}
            }
        }

        let Some(run) = self
            .storage
            .latest_durable_chat_run_for_session(session_id)?
        else {
            return Ok(None);
        };
        if run.status != "failed" || run.provider != Provider::Codex.as_str() {
            return Ok(None);
        }
        let Some(failed_message_id) = run.user_message_id else {
            return Ok(None);
        };
        let observed_bytes = self.sessions.native_rollout_size(session_id).await;
        let error = run.last_error.unwrap_or_default().to_ascii_lowercase();
        if observed_bytes.is_none_or(|bytes| bytes < CODEX_CONTEXT_ROLLOVER_THRESHOLD_BYTES)
            && !error.contains("invalid body")
            && !error.contains("413")
            && !error.contains("too large")
        {
            return Ok(None);
        }
        Ok(Some(ChatContextRecovery {
            code: "context_too_large".to_string(),
            state: "required".to_string(),
            message: "This chat's native context is too large to resume safely. Compact it into a clean context and retry the same message.".to_string(),
            failed_message_id,
            observed_bytes,
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            request_id: None,
        }))
    }

    pub async fn replay_agent_events(&self) -> Vec<WsServerEvent> {
        self.agents.replay_events().await
    }
}

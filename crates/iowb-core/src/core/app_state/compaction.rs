impl AppState {
    async fn delete_compensating_codex_fork(&self, thread_id: Option<&str>) {
        let Some(thread_id) = thread_id else {
            return;
        };
        let result = {
            let _mutation = self.codex_app_server_mutation.lock().await;
            self.codex_app_server.delete_thread(thread_id).await
        };
        if let Err(error) = result {
            warn!(
                error = %error,
                thread_id,
                "failed to delete uncommitted Codex fork"
            );
        }
    }

    /// Continue a durable run that was left active when the Rust server
    /// process stopped. This is an internal startup path: it deliberately does
    /// not append another visible user message or create a second durable row.
    pub async fn recover_agent_run(
        &self,
        run: StoredDurableChatRun,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
    ) -> Result<SessionSummary> {
        let provider = parse_stored_provider(&run.provider)?;
        let stored_session = self
            .storage
            .get_session_summary(&run.session_id)?
            .ok_or_else(|| CoreError::SessionNotFound(run.session_id.clone()))?;
        let runtime = stored_session
            .runtime
            .unwrap_or_else(|| legacy_chat_runtime(run.model.as_deref()));

        if stored_session.provider != provider {
            return Err(CoreError::InvalidInput(format!(
                "durable run {} provider {} does not match session provider {}",
                run.id,
                provider.as_str(),
                stored_session.provider.as_str()
            )));
        }

        // If the server died in the narrow window after persisting the final
        // assistant message but before terminalizing the durable row, do not
        // invoke the provider a second time.
        let already_persisted =
            self.storage
                .list_messages(&run.session_id)?
                .iter()
                .any(|message| {
                    message.role == MessageRole::Assistant
                        && message.metadata.get("durableRunId").and_then(Value::as_str)
                            == Some(run.id.as_str())
                });
        if already_persisted {
            self.storage.mark_durable_chat_run_completed(&run.id)?;
            let session = self.sessions.set_active(&run.session_id, false).await?;
            info!(
                run_id = %run.id,
                session_id = %run.session_id,
                "reconciled durable chat run whose final assistant message was already persisted"
            );
            return Ok(session);
        }

        let resolved_project_path = self
            .path_validator
            .validate_path(PathBuf::from(&run.project_path), false)
            .await?;
        let metadata = tokio::fs::metadata(&resolved_project_path).await?;
        if !metadata.is_dir() {
            return Err(CoreError::InvalidInput(format!(
                "durable run {} project path must be a directory",
                run.id
            )));
        }

        let context_rollover = self.storage.context_rollover_for_retry_run(&run.id)?;
        let mut native_resume_session_id = if let Some(rollover) = context_rollover.as_ref() {
            rollover
                .candidate_native_session_id
                .clone()
                .filter(|candidate| {
                    run.native_session_id
                        .as_deref()
                        .is_none_or(|run_native| run_native == candidate)
                })
        } else {
            run.native_session_id
                .clone()
                .or_else(|| stored_session.native_session_id.clone())
        };
        if context_rollover.is_none() {
            if native_resume_session_id.is_none() && stored_session.external {
                native_resume_session_id = Some(run.session_id.clone());
            }
            if native_resume_session_id.is_none() {
                native_resume_session_id = self
                    .sessions
                    .infer_native_session_id(
                        &run.session_id,
                        provider,
                        &resolved_project_path.display().to_string(),
                    )
                    .await?;
            }
        }
        if let Some(native_session_id) = native_resume_session_id.as_deref() {
            self.storage
                .update_durable_chat_run_native_session_id(&run.id, Some(native_session_id))?;
            if context_rollover.is_none() {
                self.sessions
                    .set_native_session_id(&run.session_id, native_session_id)
                    .await?;
            }
        }

        validate_recovered_agent_runtime_config(provider, runtime, direct_ai_config.as_ref())?;

        let session = self.sessions.set_active(&run.session_id, true).await?;
        let recovery_prompt = durable_chat_recovery_prompt(&run.prompt);
        let direct_ai_messages =
            if should_use_direct_ai_gateway_runtime(provider, run.model.as_deref()) {
                let mut messages =
                    direct_ai_conversation_messages(self.sessions.messages(&run.session_id)?, "");
                append_direct_ai_recovery_prompt(&mut messages, &recovery_prompt);
                messages
            } else {
                Vec::new()
            };

        let response_id = if context_rollover.is_some() {
            run.id.clone()
        } else {
            new_id("response")
        };
        let attempt_id = new_id("attempt");
        self.storage
            .create_chat_run_attempt(&StoredChatRunAttempt::new(
                attempt_id.clone(),
                run.id.clone(),
                run.session_id.clone(),
                run.user_message_id.clone(),
                provider.as_str(),
                runtime_label(runtime),
                run.model.clone(),
                native_resume_session_id.clone(),
            ))?;
        let start_result = self
            .agents
            .start(AgentStartContext {
                provider,
                session_id: run.session_id.clone(),
                durable_run_id: Some(run.id.clone()),
                attempt_id: Some(attempt_id),
                response_id,
                sequence: Arc::new(AtomicU64::new(0)),
                project_path: resolved_project_path,
                prompt: recovery_prompt,
                model: run.model.clone(),
                runtime,
                effort: run.effort.clone(),
                mode: run.mode.clone(),
                thinking: run.thinking,
                fast: run.fast,
                native_resume_session_id,
                native_rollout_owned_by_provider: false,
                context_rollover_id: context_rollover
                    .as_ref()
                    .map(|rollover| rollover.id.clone()),
                direct_ai_config,
                direct_ai_messages,
                sessions: self.sessions.clone(),
                storage: self.storage.clone(),
                hub: self.ws_hub.clone(),
            })
            .await;

        if let Err(error) = start_result {
            let message = error.to_string();
            if let Some(rollover) = context_rollover.as_ref() {
                let _ = self.storage.fail_context_rollover(&rollover.id, &message);
            }
            let _ = self.storage.mark_durable_chat_run_failed(&run.id, &message);
            let _ = self.sessions.set_active(&run.session_id, false).await;
            return Err(error);
        }

        self.ws_hub.publish(WsServerEvent::ActiveSessions {
            sessions: self.sessions.list_active().await,
        });
        info!(
            run_id = %run.id,
            session_id = %run.session_id,
            attempt = run.resume_attempts,
            provider = provider.as_str(),
            "started automatic recovery for interrupted chat run"
        );
        Ok(session)
    }

    pub async fn abort_agent_session(&self, provider: Provider, session_id: &str) -> Result<bool> {
        let aborted = self.agents.abort(provider, session_id).await;
        if !aborted {
            for run in self
                .storage
                .list_active_durable_chat_runs()?
                .into_iter()
                .filter(|run| run.session_id == session_id && run.provider == provider.as_str())
            {
                self.storage.mark_durable_chat_run_terminal(
                    &run.id,
                    "aborted",
                    Some("aborted while no provider process was attached"),
                )?;
            }
            let _ = self.sessions.set_active(session_id, false).await?;
            self.ws_hub.publish(WsServerEvent::SessionStatus {
                provider,
                session_id: session_id.to_string(),
                status: iowb_protocol::SessionRuntimeStatus::Aborted,
                response_id: None,
                sequence: None,
                latest_user_prompt: None,
            });
        }
        self.ws_hub.publish(WsServerEvent::ActiveSessions {
            sessions: self.sessions.list_active().await,
        });
        Ok(aborted)
    }

    pub async fn compact_and_retry_session_context(
        &self,
        user_id: &str,
        session_id: &str,
        failed_message_id: &str,
        request_id: &str,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
    ) -> Result<CompactSessionContextResponse> {
        if let Some(existing) = self
            .storage
            .context_rollover_for_request(user_id, session_id, request_id)?
        {
            if existing.kind != CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN {
                return Err(CoreError::Conflict(
                    "this compaction request id was already used for a different operation"
                        .to_string(),
                ));
            }
            if existing.failed_message_id != failed_message_id {
                return Err(CoreError::Conflict(
                    "this recovery request id was already used for a different failed message"
                        .to_string(),
                ));
            }
            return Ok(CompactSessionContextResponse {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                response_id: existing.retry_run_id,
                state: existing.state,
            });
        }

        let session = self.sessions.get(session_id).await?;
        if session.provider != Provider::Codex {
            return Err(CoreError::InvalidInput(
                "clean context rollover is currently available only for Codex sessions".to_string(),
            ));
        }
        if session.active || self.agents.is_running(Provider::Codex, session_id).await {
            return Err(CoreError::Conflict(
                "stop the active response before compacting this chat".to_string(),
            ));
        }
        let recovery_source =
            resolve_retry_context_rollover_source(&self.storage, session_id, failed_message_id)?;

        let observed_bytes = self
            .sessions
            .native_rollout_size(session_id)
            .await
            .filter(|size| *size > 0);
        let visible_messages = context_materialization_messages(
            session_id,
            self.sessions
                .messages_including_external(session_id)
                .await?,
            &[failed_message_id],
        );
        let handoff = build_context_rollover_handoff(
            visible_messages.clone(),
            &recovery_source.failed_prompt,
        );
        let rollover_id = new_id("rollover");
        let compact_run_id = new_id("run");
        let now = Utc::now();
        let rollover = StoredSessionContextRollover {
            id: rollover_id.clone(),
            user_id: user_id.to_string(),
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            kind: CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN.to_string(),
            failed_message_id: failed_message_id.to_string(),
            trigger_run_id: recovery_source.recovery_run.id.clone(),
            retry_run_id: compact_run_id.clone(),
            from_native_session_id: session.native_session_id.clone(),
            candidate_native_session_id: None,
            state: "starting".to_string(),
            handoff: handoff.clone(),
            observed_bytes,
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut compact_run = StoredDurableChatRun::new(
            compact_run_id.clone(),
            Some(user_id.to_string()),
            session_id.to_string(),
            Provider::Codex.as_str(),
            handoff.clone(),
            session.project_path.clone(),
        );
        compact_run.model = recovery_source
            .recovery_run
            .model
            .clone()
            .or(session.model.clone());
        compact_run.effort = recovery_source
            .recovery_run
            .effort
            .clone()
            .or(session.effort.clone());
        compact_run.mode = recovery_source
            .recovery_run
            .mode
            .clone()
            .or(session.mode.clone());
        compact_run.thinking = recovery_source.recovery_run.thinking.or(session.thinking);
        compact_run.fast = recovery_source.recovery_run.fast.or(session.fast);
        compact_run.native_session_id = None;
        let runtime = session.runtime.unwrap_or(ChatRuntime::NativeCli);
        validate_recovered_agent_runtime_config(
            Provider::Codex,
            runtime,
            direct_ai_config.as_ref(),
        )?;

        self.storage
            .replace_session_messages(session_id, &visible_messages)?;
        if !self
            .storage
            .prepare_context_rollover(&rollover, &compact_run)?
        {
            let existing = self
                .storage
                .context_rollover_for_request(user_id, session_id, request_id)?
                .ok_or_else(|| {
                    CoreError::Conflict("context rollover already exists".to_string())
                })?;
            if existing.kind != CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN {
                return Err(CoreError::Conflict(
                    "this compaction request id was already used for a different operation"
                        .to_string(),
                ));
            }
            if existing.failed_message_id != failed_message_id {
                return Err(CoreError::Conflict(
                    "this recovery request id was already used for a different failed message"
                        .to_string(),
                ));
            }
            return Ok(CompactSessionContextResponse {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                response_id: existing.retry_run_id,
                state: existing.state,
            });
        }
        self.sessions.set_active(session_id, true).await?;
        let attempt_id = new_id("attempt");
        self.storage
            .create_chat_run_attempt(&StoredChatRunAttempt::new(
                attempt_id.clone(),
                compact_run_id.clone(),
                session_id.to_string(),
                compact_run.user_message_id.clone(),
                Provider::Codex.as_str(),
                runtime_label(runtime),
                compact_run.model.clone(),
                None,
            ))?;
        let start_result = self
            .agents
            .start(AgentStartContext {
                provider: Provider::Codex,
                session_id: session_id.to_string(),
                durable_run_id: Some(compact_run_id.clone()),
                attempt_id: Some(attempt_id),
                response_id: compact_run_id.clone(),
                sequence: Arc::new(AtomicU64::new(0)),
                project_path: PathBuf::from(&session.project_path),
                prompt: handoff,
                model: compact_run.model.clone(),
                runtime,
                effort: compact_run.effort.clone(),
                mode: compact_run.mode.clone(),
                thinking: compact_run.thinking,
                fast: compact_run.fast,
                native_resume_session_id: None,
                native_rollout_owned_by_provider: false,
                context_rollover_id: Some(rollover_id.clone()),
                direct_ai_config,
                direct_ai_messages: Vec::new(),
                sessions: self.sessions.clone(),
                storage: self.storage.clone(),
                hub: self.ws_hub.clone(),
            })
            .await;
        if let Err(error) = start_result {
            let message = error.to_string();
            let _ = self.storage.fail_context_rollover(&rollover_id, &message);
            let _ = self
                .storage
                .mark_durable_chat_run_failed(&compact_run_id, &message);
            let _ = self.sessions.set_active(session_id, false).await;
            return Err(error);
        }

        Ok(CompactSessionContextResponse {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            response_id: compact_run_id,
            state: self
                .storage
                .context_rollover_for_retry_run(&rollover.retry_run_id)?
                .map(|stored| stored.state)
                .unwrap_or_else(|| "starting".to_string()),
        })
    }

    pub async fn compact_session_context(
        &self,
        user_id: &str,
        session_id: &str,
        request_id: &str,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
    ) -> Result<CompactSessionContextResponse> {
        if let Some(existing) = self
            .storage
            .context_rollover_for_request(user_id, session_id, request_id)?
        {
            if existing.kind != CONTEXT_ROLLOVER_KIND_MANUAL {
                return Err(CoreError::Conflict(
                    "this compaction request id was already used for a different operation"
                        .to_string(),
                ));
            }
            return Ok(CompactSessionContextResponse {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                response_id: existing.retry_run_id,
                state: existing.state,
            });
        }

        let session = self.sessions.get(session_id).await?;
        if session.provider != Provider::Codex {
            return Err(CoreError::InvalidInput(
                "manual context compaction is currently available only for Codex sessions"
                    .to_string(),
            ));
        }
        if session.active || self.agents.is_running(Provider::Codex, session_id).await {
            return Err(CoreError::Conflict(
                "stop the active response before compacting this chat".to_string(),
            ));
        }
        if self.context_recovery(session_id).await?.is_some() {
            return Err(CoreError::Conflict(
                "use Compact & retry to recover the latest failed turn before manual compaction"
                    .to_string(),
            ));
        }
        let native_session_id = self
            .codex_native_session_id_for_compaction(&session)
            .await?
            .ok_or_else(|| {
                CoreError::Conflict(
                    "codex_native_session_unavailable: this chat is not linked to a Codex thread"
                        .to_string(),
                )
            })?;

        let visible_messages = context_materialization_messages(
            session_id,
            self.sessions
                .messages_including_external(session_id)
                .await?,
            &[],
        );
        if !context_handoff_has_retainable_text(&visible_messages) {
            return Err(CoreError::InvalidInput(
                "there are no text messages to compact in this chat".to_string(),
            ));
        }
        let observed_bytes = self
            .sessions
            .native_rollout_size(session_id)
            .await
            .filter(|size| *size > 0);
        let handoff = "Native Codex context compaction".to_string();
        let rollover_id = new_id("rollover");
        let compact_run_id = new_id("run");
        let attempt_id = new_id("attempt");
        let now = Utc::now();
        let rollover = StoredSessionContextRollover {
            id: rollover_id.clone(),
            user_id: user_id.to_string(),
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            kind: CONTEXT_ROLLOVER_KIND_MANUAL.to_string(),
            failed_message_id: String::new(),
            trigger_run_id: compact_run_id.clone(),
            retry_run_id: compact_run_id.clone(),
            from_native_session_id: Some(native_session_id.clone()),
            candidate_native_session_id: Some(native_session_id.clone()),
            state: "starting".to_string(),
            handoff: handoff.clone(),
            observed_bytes,
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut compact_run = StoredDurableChatRun::new(
            compact_run_id.clone(),
            Some(user_id.to_string()),
            session_id.to_string(),
            Provider::Codex.as_str(),
            handoff.clone(),
            session.project_path.clone(),
        );
        compact_run.model = session.model.clone();
        compact_run.effort = session.effort.clone();
        compact_run.mode = session.mode.clone();
        compact_run.thinking = session.thinking;
        compact_run.fast = session.fast;
        compact_run.native_session_id = Some(native_session_id.clone());
        compact_run.auto_resume = false;
        let runtime = session.runtime.unwrap_or(ChatRuntime::NativeCli);
        let app_server_options =
            codex_app_server_launch_options(runtime, direct_ai_config.as_ref())?;

        self.storage
            .replace_session_messages(session_id, &visible_messages)?;
        if !self
            .storage
            .prepare_manual_context_rollover(&rollover, &compact_run)?
        {
            let existing = self
                .storage
                .context_rollover_for_request(user_id, session_id, request_id)?
                .ok_or_else(|| {
                    CoreError::Conflict("context rollover already exists".to_string())
                })?;
            if existing.kind != CONTEXT_ROLLOVER_KIND_MANUAL {
                return Err(CoreError::Conflict(
                    "this compaction request id was already used for a different operation"
                        .to_string(),
                ));
            }
            return Ok(CompactSessionContextResponse {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                response_id: existing.retry_run_id,
                state: existing.state,
            });
        }
        self.sessions.set_active(session_id, true).await?;
        self.storage
            .create_chat_run_attempt(&StoredChatRunAttempt::new(
                attempt_id.clone(),
                compact_run_id.clone(),
                session_id.to_string(),
                compact_run.user_message_id.clone(),
                Provider::Codex.as_str(),
                "codex_app_server",
                compact_run.model.clone(),
                Some(native_session_id.clone()),
            ))?;
        self.ws_hub.publish(WsServerEvent::SessionStatus {
            provider: Provider::Codex,
            session_id: session_id.to_string(),
            status: iowb_protocol::SessionRuntimeStatus::Starting,
            response_id: Some(compact_run_id.clone()),
            sequence: None,
            latest_user_prompt: None,
        });
        self.ws_hub.publish(WsServerEvent::SessionStatus {
            provider: Provider::Codex,
            session_id: session_id.to_string(),
            status: iowb_protocol::SessionRuntimeStatus::Running,
            response_id: Some(compact_run_id.clone()),
            sequence: None,
            latest_user_prompt: None,
        });
        self.ws_hub.publish(WsServerEvent::ActiveSessions {
            sessions: self.sessions.list_active().await,
        });
        let task = ManualContextCompactionTask {
            session,
            rollover_id: rollover_id.clone(),
            retry_run_id: compact_run_id.clone(),
            attempt_id,
            native_session_id,
            handoff,
            compact_run,
            runtime,
            app_server_options,
        };
        let state = self.clone();
        tokio::spawn(async move {
            state.run_manual_context_compaction(task).await;
        });

        Ok(CompactSessionContextResponse {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            response_id: compact_run_id,
            state: "starting".to_string(),
        })
    }

}

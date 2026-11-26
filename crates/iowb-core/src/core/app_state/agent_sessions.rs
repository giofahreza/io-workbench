impl AppState {
    pub async fn native_chat_configuration(
        &self,
        provider: Provider,
        session_id: Option<&str>,
    ) -> Option<SessionSummary> {
        let session_id = session_id?;
        if !matches!(provider, Provider::Codex | Provider::Claude)
            || self.storage.has_active_context_rollover(session_id).ok()?
        {
            return None;
        }
        if let Some(session) = self
            .sessions
            .get_stored(session_id)
            .filter(|session| !session.external)
        {
            return (session.provider == provider
                && session.runtime == Some(ChatRuntime::NativeCli)
                && session.native_session_id.is_some()
                && session.native_rollout_owned_by_provider)
                .then_some(session);
        }
        let mut record = self
            .sessions
            .external_record(session_id, Some(provider), None)
            .await;
        if record.is_none() && Uuid::parse_str(session_id).is_ok() {
            self.sessions.external_cache.write().await.loaded_at = None;
            record = self
                .sessions
                .external_record(session_id, Some(provider), None)
                .await;
        }
        record.map(|record| record.summary)
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_agent_session_scoped(
        &self,
        provider: Provider,
        project_path: impl Into<String>,
        prompt: impl Into<String>,
        session_id: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<String>,
        thinking: Option<bool>,
        fast: Option<bool>,
        runtime: ChatRuntime,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
        user_id: Option<String>,
        board_scope: Option<(String, Option<String>)>,
        request_id: Option<String>,
    ) -> Result<SessionSummary> {
        let project_path = project_path.into();
        let prompt = prompt.into();
        let _admission = if let Some(id) = session_id.as_deref() {
            Some(self.agents.lock_admission(provider, id).await)
        } else {
            None
        };
        let requested_run_id =
            request_id
                .as_deref()
                .zip(session_id.as_deref())
                .map(|(request, session)| {
                    let scope = serde_json::json!([user_id, provider.as_str(), session, request])
                        .to_string();
                    format!("run_{}", hex::encode(Sha256::digest(scope.as_bytes())))
                });
        if let Some(run_id) = requested_run_id.as_deref()
            && let Some(run) = self.storage.get_durable_chat_run(run_id)?
        {
            let original_prompt = run
                .user_message_id
                .as_deref()
                .and_then(|id| {
                    self.storage
                        .message_by_id(&run.session_id, id)
                        .ok()
                        .flatten()
                })
                .map(|message| message.content)
                .unwrap_or_else(|| run.prompt.clone());
            if original_prompt != prompt || run.project_path != project_path {
                return Err(CoreError::Conflict(
                    "This request id was already used for another message".into(),
                ));
            }
            if run.status == "rejected" {
                return Err(CoreError::Conflict(run.last_error.unwrap_or_else(|| {
                    "This message was not sent; retry with a new request id".into()
                })));
            }
            return self.sessions.get(&run.session_id).await;
        }
        if let Some(id) = session_id.as_deref()
            && self.agents.is_running(provider, id).await
        {
            return Err(CoreError::Conflict("This chat already has a running turn. Wait for it to finish; no additional message was sent.".into()));
        }
        let resolved_project_path = self
            .path_validator
            .validate_path(PathBuf::from(&project_path), false)
            .await?;

        let metadata = tokio::fs::metadata(&resolved_project_path).await?;
        if !metadata.is_dir() {
            return Err(CoreError::InvalidInput(
                "project path must be a directory".to_string(),
            ));
        }

        let command_session_id = if iowb_protocol::leading_chat_command(&prompt).is_some() {
            if let Some(id) = session_id.as_deref() {
                self.sessions.get(id).await.ok().map(|session| session.id)
            } else {
                None
            }
        } else {
            None
        };
        let execution_prompt = self
            .resolve_chat_command_prompt(
                iowb_protocol::ChatCommandContext {
                    provider,
                    project_path: resolved_project_path.display().to_string(),
                    session_id: command_session_id,
                    model: model.clone(),
                    runtime,
                },
                &prompt,
                user_id.as_deref().unwrap_or("local"),
            )
            .await?;

        let enforce_context_limit = provider == Provider::Codex
            && runtime == ChatRuntime::IoGateway
            && self.codex_context_limit_enabled(user_id.as_deref())?;

        // Recovery is a state transition for the current visible chat, not a
        // new turn. Reject ordinary sends before create_or_update can mark the
        // session active or append another user message to a poisoned native
        // thread. The compact-and-retry endpoint bypasses this method.
        if provider == Provider::Codex
            && let Some(existing_session_id) = session_id.as_deref()
            && self.context_recovery(existing_session_id).await?.is_some()
        {
            return Err(CoreError::Conflict(
                "this chat needs clean-context recovery before another message can be sent"
                    .to_string(),
            ));
        }

        let (external, native_resume_session_id) = if let Some(session_id) = session_id.as_deref() {
            let stored_session = self.sessions.get_stored(session_id);
            let has_context_rollover = self.storage.has_active_context_rollover(session_id)?;
            let external = !has_context_rollover
                && (self
                    .sessions
                    .external_record(
                        session_id,
                        Some(provider),
                        Some(&resolved_project_path.display().to_string()),
                    )
                    .await
                    .is_some()
                    || stored_session
                        .as_ref()
                        .is_some_and(|session| session.external));
            let native_resume_session_id = if has_context_rollover {
                stored_session.and_then(|session| session.native_session_id)
            } else if external {
                Some(session_id.to_string())
            } else {
                match stored_session.and_then(|session| session.native_session_id) {
                    Some(native_session_id) => Some(native_session_id),
                    None => {
                        self.sessions
                            .infer_native_session_id(
                                session_id,
                                provider,
                                &resolved_project_path.display().to_string(),
                            )
                            .await?
                    }
                }
            };
            (external, native_resume_session_id)
        } else {
            (false, None)
        };

        if provider == Provider::Claude && runtime == ChatRuntime::NativeCli {
            let bridge = if let Some(id) = native_resume_session_id.as_deref() {
                claude_bridge::Client::discover(id, &resolved_project_path).await?
            } else { None };
            if let Some(client) = bridge {
                if mode.is_some() || thinking.is_some() || fast.is_some() {
                    return Err(CoreError::Conflict("Keep native Claude session settings while attached. Change permission, thinking, or fast settings in the terminal before sending; no message was sent.".into()));
                }
                let status = client.read(None).await?;
                if status["busy"] == true || status["stopped"] == true {
                    return Err(CoreError::Conflict("Claude is busy or stopped in the terminal. Finish the turn or type a message there to continue; no message was sent.".into()));
                }
            } else if claude_bridge::private_owner(&resolved_project_path, native_resume_session_id.as_deref()) {
                return Err(CoreError::Conflict("A separate Claude CLI still owns this project. Exit that chat, or reopen it with `io-workbench claude --resume <native-session-id>` to keep the CLI open; no message was sent.".into()));
            }
        }

        let native_before_turn_id = if provider == Provider::Codex {
            if let Some(native_session_id) = native_resume_session_id.as_deref() {
                match self.codex_app_server.read_thread(native_session_id).await {
                    Ok(snapshot) => snapshot.latest_forkable_turn_id().map(str::to_string),
                    Err(error) => {
                        warn!(
                            error = %error,
                            native_session_id,
                            "failed to capture Codex turn boundary before starting prompt"
                        );
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };
        let native_rollout_bytes = if enforce_context_limit && native_resume_session_id.is_some() {
            if let Some(session_id) = session_id.as_deref() {
                self.sessions.native_rollout_size(session_id).await
            } else {
                None
            }
        } else {
            None
        };

        let mut session = if let Some((board_id, board_task_id)) = board_scope {
            self.sessions
                .create_or_update_board(
                    provider,
                    resolved_project_path.display().to_string(),
                    session_id,
                    external,
                    model.clone(),
                    Some(runtime),
                    effort.clone(),
                    mode.clone(),
                    thinking,
                    fast,
                    board_id,
                    board_task_id,
                )
                .await?
        } else {
            self.sessions
                .create_or_update(
                    provider,
                    resolved_project_path.display().to_string(),
                    session_id,
                    external,
                    model.clone(),
                    Some(runtime),
                    effort.clone(),
                    mode.clone(),
                    thinking,
                    fast,
                )
                .await?
        };

        // The durable row is committed before the provider starts. If the
        // server is killed at any point after this write, startup recovery has
        // enough information to launch a continuation in the same chat.
        for stale_run in self
            .storage
            .list_active_durable_chat_runs()?
            .into_iter()
            .filter(|run| run.session_id == session.id)
        {
            self.storage.mark_durable_chat_run_interrupted(
                &stale_run.id,
                Some("superseded by a newer turn in the same session"),
            )?;
        }
        let durable_run_id = requested_run_id.unwrap_or_else(|| new_id("run"));
        let mut durable_run = StoredDurableChatRun::new(
            durable_run_id.clone(),
            user_id,
            session.id.clone(),
            provider.as_str(),
            execution_prompt.clone(),
            resolved_project_path.display().to_string(),
        );
        durable_run.native_session_id = native_resume_session_id.clone();
        durable_run.model = model.clone();
        durable_run.effort = effort.clone();
        durable_run.mode = mode.clone();
        durable_run.thinking = thinking;
        durable_run.fast = fast;
        durable_run.native_before_turn_id = native_before_turn_id.clone();
        if !prompt.trim().is_empty() {
            let now = Utc::now();
            let user_message_id = new_id("msg");
            durable_run.user_message_id = Some(user_message_id.clone());
            let mut user_metadata = serde_json::json!({
                "cli": provider.as_str(),
                "durableRunId": durable_run_id,
                "model": model.clone().unwrap_or_default(),
                "runtime": runtime,
                "effort": effort.clone().unwrap_or_default(),
                "mode": mode.clone().unwrap_or_default(),
                "thinking": thinking.unwrap_or(false),
                "fast": fast.unwrap_or(false),
                "sentAt": now.to_rfc3339(),
            });
            if matches!(provider, Provider::Codex | Provider::Claude)
                && runtime == ChatRuntime::NativeCli
                && execution_prompt != prompt
            {
                user_metadata["nativeInput"] = Value::String(execution_prompt.clone());
            }
            if let Some(native_before_turn_id) = native_before_turn_id.as_deref()
                && let Some(metadata) = user_metadata.as_object_mut()
            {
                metadata.insert(
                    "nativeBeforeTurnId".to_string(),
                    Value::String(native_before_turn_id.to_string()),
                );
            }
            let message = ChatMessage {
                id: user_message_id,
                role: MessageRole::User,
                content: prompt.clone(),
                timestamp: now,
                metadata: user_metadata,
            };
            match self
                .sessions
                .append_user_message_with_durable_run(&session.id, message, &durable_run)
                .await
            {
                Ok(updated) => session = updated,
                Err(error) => {
                    let _ = self.sessions.set_active(&session.id, false).await;
                    return Err(error);
                }
            }
        } else if let Err(error) = self.storage.create_durable_chat_run(&durable_run) {
            let _ = self.sessions.set_active(&session.id, false).await;
            return Err(error.into());
        }

        if enforce_context_limit
            && native_resume_session_id.is_some()
            && native_rollout_bytes
                .is_some_and(|bytes| bytes >= CODEX_CONTEXT_ROLLOVER_THRESHOLD_BYTES)
        {
            let observed_bytes = native_rollout_bytes;
            let message = format!(
                "native Codex context is {} bytes, above the {} byte safe rollover threshold",
                observed_bytes.unwrap_or_default(),
                CODEX_CONTEXT_ROLLOVER_THRESHOLD_BYTES,
            );
            self.storage
                .mark_durable_chat_run_failed(&durable_run_id, &message)?;
            session = self.sessions.set_active(&session.id, false).await?;
            if let Some(failed_message_id) = durable_run.user_message_id.clone() {
                self.ws_hub.publish(WsServerEvent::ChatRecoveryRequired {
                    provider,
                    session_id: session.id.clone(),
                    response_id: None,
                    recovery: ChatContextRecovery {
                        code: "context_too_large".to_string(),
                        state: "required".to_string(),
                        message: "This chat is close to the gateway request limit. Compact it into a clean context to continue without changing the visible chat.".to_string(),
                        failed_message_id,
                        observed_bytes,
                        limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
                        request_id: None,
                    },
                });
            }
            self.ws_hub.publish(WsServerEvent::ActiveSessions {
                sessions: self.sessions.list_active().await,
            });
            return Ok(session);
        }

        let direct_ai_messages = if should_use_direct_ai_gateway_runtime(provider, model.as_deref())
        {
            direct_ai_conversation_messages(self.sessions.messages(&session.id)?, prompt.as_str())
        } else {
            Vec::new()
        };

        let attempt_id = new_id("attempt");
        self.storage
            .create_chat_run_attempt(&StoredChatRunAttempt::new(
                attempt_id.clone(),
                durable_run_id.clone(),
                session.id.clone(),
                durable_run.user_message_id.clone(),
                provider.as_str(),
                runtime_label(runtime),
                model.clone(),
                native_resume_session_id.clone(),
            ))?;

        let start_result = self
            .agents
            .start(AgentStartContext {
                provider,
                request_id,
                session_id: session.id.clone(),
                durable_run_id: Some(durable_run_id.clone()),
                attempt_id: Some(attempt_id),
                response_id: new_id("response"),
                sequence: Arc::new(AtomicU64::new(0)),
                project_path: resolved_project_path,
                prompt: execution_prompt,
                model,
                runtime,
                effort: effort.clone(),
                mode: mode.clone(),
                thinking,
                fast,
                native_resume_session_id,
                native_rollout_owned_by_provider: false,
                context_rollover_id: None,
                direct_ai_config,
                direct_ai_messages,
                sessions: self.sessions.clone(),
                storage: self.storage.clone(),
                hub: self.ws_hub.clone(),
            })
            .await;
        if let Err(error) = start_result {
            let message = error.to_string();
            let _ = self
                .storage
                .mark_durable_chat_run_failed(&durable_run_id, &message);
            let _ = self.sessions.set_active(&session.id, false).await;
            return Err(error);
        }

        self.ws_hub.publish(WsServerEvent::ActiveSessions {
            sessions: self.sessions.list_active().await,
        });

        Ok(session)
    }

    pub async fn fork_session_before_message(
        &self,
        user_id: &str,
        source_session_id: &str,
        before_message_id: &str,
        request_id: &str,
        replace: bool,
        draft_content: Option<&str>,
    ) -> Result<ForkSessionResponse> {
        if let Some(existing) =
            self.storage
                .get_session_fork(user_id, source_session_id, request_id)?
        {
            return self
                .existing_fork_response(
                    user_id,
                    source_session_id,
                    &existing.before_message_id,
                    &existing.destination_session_id,
                    existing.replaces_source,
                )
                .await;
        }

        let source = self.sessions.get(source_session_id).await?;
        if source.active {
            return Err(CoreError::Conflict(
                "session_active: stop the current response before editing from here".to_string(),
            ));
        }
        let source_is_stored = self
            .storage
            .get_session_summary(source_session_id)?
            .is_some();
        let source_messages = self
            .sessions
            .messages_including_external(source_session_id)
            .await?;
        let target_index = source_messages
            .iter()
            .position(|message| message.id == before_message_id)
            .ok_or_else(|| {
                CoreError::InvalidInput(format!(
                    "message {before_message_id} was not found in session {source_session_id}"
                ))
            })?;
        let target = &source_messages[target_index];
        if target.role != MessageRole::User {
            return Err(CoreError::InvalidInput(
                "Edit from here requires a user prompt".to_string(),
            ));
        }
        let prefix = &source_messages[..target_index];
        let draft_content = draft_content.unwrap_or(&target.content);
        let has_prior_user_turn = prefix
            .iter()
            .any(|message| message.role == MessageRole::User);

        let mut native_forked_thread_id = None;
        if has_prior_user_turn {
            if source.provider == Provider::Codex {
                let native_session_id = if source.external {
                    Some(source.id.clone())
                } else if source.native_session_id.is_some() {
                    source.native_session_id.clone()
                } else {
                    self.sessions
                        .infer_native_session_id(
                            source_session_id,
                            source.provider,
                            &source.project_path,
                        )
                        .await?
                }
                .ok_or_else(|| {
                    CoreError::Conflict(
                        "codex_native_session_unavailable: this chat is not linked to a Codex thread"
                            .to_string(),
                    )
                })?;
                let snapshot = self
                    .codex_app_server
                    .read_thread(&native_session_id)
                    .await?;
                if snapshot.id != native_session_id {
                    warn!(
                        requested_thread_id = %native_session_id,
                        returned_thread_id = %snapshot.id,
                        "Codex thread/read returned a different thread id"
                    );
                }
                let last_turn_id = self.resolve_codex_fork_boundary(
                    source_session_id,
                    target,
                    &source_messages,
                    &snapshot,
                )?;
                native_forked_thread_id = Some({
                    let _mutation = self.codex_app_server_mutation.lock().await;
                    self.codex_app_server
                        .fork_thread(&native_session_id, &last_turn_id)
                        .await?
                });
            } else if !should_use_direct_ai_gateway_runtime(
                source.provider,
                source.model.as_deref(),
            ) {
                return Err(CoreError::InvalidInput(format!(
                    "Edit from here is not yet available for native {} sessions with earlier turns",
                    source.provider.as_str()
                )));
            }
        }

        let now = Utc::now();
        let destination_id = new_id("session");
        let cloned_messages = prefix
            .iter()
            .map(|message| clone_forked_message(source_session_id, message))
            .collect::<Vec<_>>();
        let destination = SessionSummary {
            id: destination_id,
            provider: source.provider,
            external: false,
            board_session: source.is_board_session(),
            board_id: source.board_id.clone(),
            board_task_id: source.board_task_id.clone(),
            native_session_id: native_forked_thread_id.clone(),
            native_rollout_owned_by_provider: source.native_rollout_owned_by_provider,
            title_source: Some(SessionTitleSource::Prompt),
            project_path: source.project_path.clone(),
            title: session_title_from_prompt(draft_content)
                .unwrap_or_else(|| "New Session".to_string()),
            message_count: cloned_messages.len(),
            last_activity: now,
            active: false,
            model: source.model.clone(),
            runtime: source.runtime,
            effort: source.effort.clone(),
            mode: source.mode.clone(),
            thinking: source.thinking,
            fast: source.fast,
            last_message_at: cloned_messages.last().map(|message| message.timestamp),
            first_user_at: cloned_messages
                .iter()
                .find(|message| message.role == MessageRole::User)
                .map(|message| message.timestamp),
            received_at: cloned_messages
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::Assistant)
                .map(|message| message.timestamp),
            token_usage: None,
            lifetime_token_usage: None,
            context_token_usage: None,
            spent_token_usage: None,
        };

        let outcome = self.storage.create_session_fork(
            user_id,
            source_session_id,
            before_message_id,
            request_id,
            &destination,
            &cloned_messages,
            draft_content,
            source_is_stored,
            replace,
        );
        match outcome {
            Ok(CreateSessionForkOutcome::Created) => {
                self.sessions
                    .remember_persisted_session(destination.clone())
                    .await?;
                Ok(ForkSessionResponse {
                    source_session_id: source_session_id.to_string(),
                    before_message_id: before_message_id.to_string(),
                    session: destination.clone(),
                    draft: SessionDraftResponse {
                        session_id: destination.id,
                        content: draft_content.to_string(),
                        updated_at: Some(now),
                    },
                    native_forked: native_forked_thread_id.is_some(),
                    files_unchanged: true,
                    source_hidden: replace,
                })
            }
            Ok(CreateSessionForkOutcome::Existing(existing)) => {
                self.delete_compensating_codex_fork(native_forked_thread_id.as_deref())
                    .await;
                self.existing_fork_response(
                    user_id,
                    source_session_id,
                    &existing.before_message_id,
                    &existing.destination_session_id,
                    existing.replaces_source,
                )
                .await
            }
            Ok(CreateSessionForkOutcome::SourceActive) => {
                self.delete_compensating_codex_fork(native_forked_thread_id.as_deref())
                    .await;
                Err(CoreError::Conflict(
                    "session_active: stop the current response before editing from here"
                        .to_string(),
                ))
            }
            Err(error) => {
                self.delete_compensating_codex_fork(native_forked_thread_id.as_deref())
                    .await;
                Err(error.into())
            }
        }
    }
}

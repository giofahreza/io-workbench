impl AgentRuntimeManager {
    pub fn new(max_runs: usize) -> Self {
        Self {
            runs: Arc::new(RwLock::new(HashMap::new())),
            codex_app_server: default_codex_app_server_client(),
            max_runs,
            max_replay_events: 256,
            max_replay_bytes: AGENT_REPLAY_MAX_BYTES,
            max_output_bytes: AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
        }
    }

    fn start(
        &self,
        context: AgentStartContext,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let key = agent_run_key(context.provider, &context.session_id);
            let runtime_provider = if context.runtime == ChatRuntime::IoGateway {
                context.provider
            } else {
                effective_agent_command_provider(context.provider, context.model.as_deref())
            };
            if should_use_direct_ai_gateway_runtime(context.provider, context.model.as_deref()) {
                return self.start_direct_ai(context).await;
            }
            if codex_app_server_live_enabled(&context) {
                return self.start_codex_app_server_live(context).await;
            }

            let mut command = match resolve_agent_command(
                runtime_provider,
                &context.prompt,
                &context.session_id,
                context.model.as_deref(),
                context.effort.as_deref(),
                context.mode.as_deref(),
                context.thinking,
                context.fast,
                context.native_resume_session_id.as_deref(),
                context.runtime,
            ) {
                Ok(command) => command,
                Err(error) => {
                    let error_message = error.to_string();
                    self.publish(
                        &context.hub,
                        &key,
                        WsServerEvent::Error {
                            message: "failed to prepare agent command".to_string(),
                            details: Some(error_message.clone()),
                            session_id: Some(context.session_id.clone()),
                        },
                    )
                    .await;
                    self.finish(
                        &key,
                        &context,
                        iowb_protocol::SessionRuntimeStatus::Failed,
                        Some(error_message),
                        None,
                    )
                    .await;
                    return Ok(());
                }
            };
            if context.runtime == ChatRuntime::IoGateway && runtime_provider == Provider::Codex {
                let Some(config) = context.direct_ai_config.as_ref() else {
                    return Err(CoreError::InvalidInput(
                        "IO Gateway is not configured for this session".to_string(),
                    ));
                };
                apply_codex_cli_io_gateway_args(&mut command.args, &config.base_url);
            }
            let (abort_tx, abort_rx) = oneshot::channel();

            self.register(key.clone(), abort_tx).await;

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

            let mut child_command = Command::new(&command.command);
            child_command
                .args(&command.args)
                .current_dir(&context.project_path)
                .env("PATH", augmented_user_path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if context.runtime == ChatRuntime::IoGateway && runtime_provider == Provider::Claude {
                let Some(config) = context.direct_ai_config.as_ref() else {
                    let error_message =
                    "IO Gateway is not configured. Configure the IO Gateway URL and API key in Settings."
                        .to_string();
                    self.publish(
                        &context.hub,
                        &key,
                        WsServerEvent::Error {
                            message: "IO Gateway is not configured".to_string(),
                            details: Some(error_message.clone()),
                            session_id: Some(context.session_id.clone()),
                        },
                    )
                    .await;
                    self.finish(
                        &key,
                        &context,
                        iowb_protocol::SessionRuntimeStatus::Failed,
                        Some(error_message),
                        None,
                    )
                    .await;
                    return Ok(());
                };
                apply_claude_cli_io_gateway_env(&mut child_command, config);
            }
            if context.runtime == ChatRuntime::IoGateway && runtime_provider == Provider::Codex {
                let Some(config) = context.direct_ai_config.as_ref() else {
                    unreachable!("gateway configuration was validated above");
                };
                child_command.env(IO_WORKBENCH_GATEWAY_KEY_ENV, &config.api_key);
            }
            if let Some(run_id) = context.durable_run_id.as_deref() {
                // Descendants inherit these markers. The database scope prevents a
                // server opened on a copied database from targeting the original
                // run, while the process identity distinguishes a live owner from
                // a server process that has actually exited.
                child_command.env(DURABLE_AGENT_RUN_ENV, run_id).env(
                    DURABLE_AGENT_SCOPE_ENV,
                    durable_agent_run_scope(context.storage.path()),
                );
                #[cfg(target_os = "linux")]
                if let Some((owner_pid, owner_start)) = current_process_identity() {
                    child_command
                        .env(DURABLE_AGENT_OWNER_PID_ENV, owner_pid.to_string())
                        .env(DURABLE_AGENT_OWNER_START_ENV, owner_start.to_string());
                }
            }
            // Log the exact spawn so misconfigured flag sets are easy to spot.
            let rendered_cmd = std::iter::once(command.command.clone())
                .chain(command.args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ");
            info!(
                provider = context.provider.as_str(),
                session_id = %context.session_id,
                project = %context.project_path.display(),
                cmd = %rendered_cmd,
                "spawning agent command"
            );
            if command.stdin_prompt {
                child_command.stdin(Stdio::piped());
            } else {
                child_command.stdin(Stdio::null());
            }
            isolate_agent_process(&mut child_command);

            let mut child = match child_command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    let error_message = format!(
                        "failed to spawn agent provider: {}: {}",
                        command.command, error
                    );
                    self.publish(
                        &context.hub,
                        &key,
                        WsServerEvent::Error {
                            message: "failed to spawn agent provider".to_string(),
                            details: Some(error_message.clone()),
                            session_id: Some(context.session_id.clone()),
                        },
                    )
                    .await;
                    self.finish(
                        &key,
                        &context,
                        iowb_protocol::SessionRuntimeStatus::Failed,
                        Some(error_message),
                        None,
                    )
                    .await;
                    return Ok(());
                }
            };

            if command.stdin_prompt {
                if let Some(mut stdin) = child.stdin.take() {
                    let prompt = command.prompt.clone();
                    tokio::spawn(async move {
                        let _ = stdin.write_all(prompt.as_bytes()).await;
                        let _ = stdin.write_all(b"\n").await;
                        let _ = stdin.shutdown().await;
                    });
                }
            }

            let (output_tx, mut output_rx) = mpsc::channel::<AgentProcessEvent>(256);
            if let Some(stdout) = child.stdout.take() {
                spawn_agent_output_reader(output_tx.clone(), stdout, AgentOutputStream::Stdout);
            }
            if let Some(stderr) = child.stderr.take() {
                spawn_agent_output_reader(output_tx.clone(), stderr, AgentOutputStream::Stderr);
            }
            drop(output_tx);

            self.publish(
                &context.hub,
                &key,
                WsServerEvent::SessionStatus {
                    provider: context.provider,
                    session_id: context.session_id.clone(),
                    status: iowb_protocol::SessionRuntimeStatus::Running,
                    response_id: Some(context.response_id.clone()),
                    sequence: Some(context.next_sequence()),
                    latest_user_prompt: Some(context.prompt.clone()),
                },
            )
            .await;

            let manager = self.clone();
            tokio::spawn(async move {
                let mut abort_rx = abort_rx;
                let mut output = String::new();
                let mut codex_normalizer =
                    (runtime_provider == Provider::Codex).then(CodexLiveOutputNormalizer::default);
                let mut claude_normalizer = (runtime_provider == Provider::Claude)
                    .then(ClaudeLiveOutputNormalizer::default);
                let mut gemini_normalizer = (runtime_provider == Provider::Gemini)
                    .then(GeminiLiveOutputNormalizer::default);
                loop {
                    tokio::select! {
                        Some(event) = output_rx.recv() => {
                            process_agent_event(
                                &manager,
                                &context,
                                &key,
                                event,
                                &mut codex_normalizer,
                                &mut claude_normalizer,
                                &mut gemini_normalizer,
                                &mut output,
                            ).await;
                        }
                        status = child.wait() => {
                            while let Some(event) = output_rx.recv().await {
                                process_agent_event(
                                    &manager,
                                    &context,
                                    &key,
                                    event,
                                    &mut codex_normalizer,
                                    &mut claude_normalizer,
                                    &mut gemini_normalizer,
                                    &mut output,
                                ).await;
                            }
                            flush_codex_live_output(
                                &manager,
                                &context,
                                &key,
                                &mut codex_normalizer,
                                &mut output,
                            ).await;
                            flush_claude_live_output(
                                &manager,
                                &context,
                                &key,
                                &mut claude_normalizer,
                                &mut output,
                            ).await;
                            flush_gemini_live_output(
                                &manager,
                                &context,
                                &key,
                                &mut gemini_normalizer,
                                &mut output,
                            ).await;
                            let codex_saw_structured_event = codex_normalizer
                                .as_ref()
                                .is_some_and(CodexLiveOutputNormalizer::saw_structured_event);
                            let run_usage = codex_normalizer
                                .as_mut()
                                .and_then(CodexLiveOutputNormalizer::take_final_usage)
                                .or_else(|| {
                                    claude_normalizer
                                        .as_mut()
                                        .and_then(ClaudeLiveOutputNormalizer::take_final_usage)
                                })
                                .or_else(|| {
                                    gemini_normalizer
                                        .as_mut()
                                        .and_then(GeminiLiveOutputNormalizer::take_final_usage)
                                });
                            let codex_final_assistant = codex_normalizer
                                .as_mut()
                                .and_then(CodexLiveOutputNormalizer::take_final_assistant_message);
                            let codex_error = codex_normalizer
                                .as_mut()
                                .and_then(CodexLiveOutputNormalizer::take_error);
                            let claude_final_assistant = claude_normalizer
                                .as_mut()
                                .and_then(ClaudeLiveOutputNormalizer::take_final_assistant_message);
                            persist_codex_tool_messages(&context, &mut codex_normalizer).await;
                            let provider_specific_final = codex_final_assistant
                                .or(claude_final_assistant);
                            match status {
                                Ok(status) if status.success() => {
                                    if context.context_rollover_id.is_some() {
                                        let follow_up = manager.finish(
                                            &key,
                                            &context,
                                            iowb_protocol::SessionRuntimeStatus::Completed,
                                            None,
                                            run_usage.clone(),
                                        ).await;
                                        if let Some(follow_up) = follow_up {
                                            manager
                                                .start_context_rollover_follow_up(&context, follow_up)
                                                .await;
                                        }
                                    } else {
                                        match select_completed_agent_output(
                                            runtime_provider,
                                            provider_specific_final,
                                            &output,
                                            codex_saw_structured_event,
                                        ) {
                                            Ok(persisted_output) => {
                                                manager.finish(
                                                    &key,
                                                    &context,
                                                    iowb_protocol::SessionRuntimeStatus::Completed,
                                                    Some(persisted_output),
                                                    run_usage.clone(),
                                                ).await;
                                            }
                                            Err(error_output) => {
                                                manager.publish(&context.hub, &key, WsServerEvent::Error {
                                                    message: "Codex completed without a final assistant response".to_string(),
                                                    details: Some(
                                                        "The Codex process exited successfully, but its event stream did not contain a final assistant message. The accumulated CLI transcript was not saved as the reply."
                                                            .to_string(),
                                                    ),
                                                    session_id: Some(context.session_id.clone()),
                                                }).await;
                                                manager.finish(
                                                    &key,
                                                    &context,
                                                    iowb_protocol::SessionRuntimeStatus::Failed,
                                                    Some(error_output),
                                                    run_usage.clone(),
                                                ).await;
                                            }
                                        }
                                    }
                                }
                                Ok(status) => {
                                    let mut persisted_output = provider_specific_final
                                        .unwrap_or_else(|| output.clone());
                                    append_bounded(
                                        &mut output,
                                        &format!("\nAgent exited with status {status}"),
                                        manager.max_output_bytes,
                                    );
                                    append_bounded(
                                        &mut persisted_output,
                                        &format!("\nAgent exited with status {status}"),
                                        manager.max_output_bytes,
                                    );
                                    manager.finish(
                                        &key,
                                        &context,
                                        iowb_protocol::SessionRuntimeStatus::Failed,
                                        Some(persisted_output.clone()),
                                        run_usage.clone(),
                                    ).await;
                                    if let Some(error) = codex_error.as_ref() {
                                        if let Some(run_id) = context.durable_run_id.as_deref() {
                                            let _ = context.storage.update_durable_chat_run_error(
                                                run_id,
                                                &error.message,
                                            );
                                        }
                                        manager.publish_context_recovery_if_needed(
                                            &key,
                                            &context,
                                            error,
                                        ).await;
                                    }
                                }
                                Err(error) => {
                                    let persisted_output = provider_specific_final
                                        .unwrap_or_else(|| output.clone());
                                    manager.publish(&context.hub, &key, WsServerEvent::Error {
                                        message: "agent process wait failed".to_string(),
                                        details: Some(error.to_string()),
                                        session_id: Some(context.session_id.clone()),
                                    }).await;
                                    manager.finish(
                                        &key,
                                        &context,
                                        iowb_protocol::SessionRuntimeStatus::Failed,
                                        Some(persisted_output.clone()),
                                        run_usage.clone(),
                                    ).await;
                                }
                            }
                            break;
                        }
                        _ = &mut abort_rx => {
                            terminate_agent_process_tree(&mut child, &context.session_id).await;
                            drain_aborted_agent_output(&mut output_rx).await;
                            let codex_final_assistant = codex_normalizer
                                .as_mut()
                                .and_then(CodexLiveOutputNormalizer::take_final_assistant_message);
                            let claude_final_assistant = claude_normalizer
                                .as_mut()
                                .and_then(ClaudeLiveOutputNormalizer::take_final_assistant_message);
                            persist_codex_tool_messages(&context, &mut codex_normalizer).await;
                            let final_assistant = codex_final_assistant
                                .or(claude_final_assistant)
                                .unwrap_or_else(|| output.clone());
                            manager.finish(
                                &key,
                                &context,
                                iowb_protocol::SessionRuntimeStatus::Aborted,
                                Some(final_assistant),
                                None,
                            ).await;
                            break;
                        }
                        else => break,
                    }
                }
            });

            Ok(())
        })
    }

    async fn start_codex_app_server_live(&self, mut context: AgentStartContext) -> Result<()> {
        context.native_rollout_owned_by_provider = true;
        let key = agent_run_key(context.provider, &context.session_id);
        let (abort_tx, abort_rx) = oneshot::channel();

        self.register(key.clone(), abort_tx).await;

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

        let launch_options = match codex_app_server_launch_options(
            context.runtime,
            context.direct_ai_config.as_ref(),
        ) {
            Ok(options) => options,
            Err(error) => {
                let error_message = error.to_string();
                self.publish(
                    &context.hub,
                    &key,
                    WsServerEvent::Error {
                        message: "failed to prepare Codex app-server".to_string(),
                        details: Some(error_message.clone()),
                        session_id: Some(context.session_id.clone()),
                    },
                )
                .await;
                self.finish(
                    &key,
                    &context,
                    iowb_protocol::SessionRuntimeStatus::Failed,
                    Some(error_message),
                    None,
                )
                .await;
                return Ok(());
            }
        };
        let turn_params = codex_app_server_live_turn_params(&context);
        let client = self.codex_app_server.clone();
        let manager = self.clone();
        let (event_tx, mut event_rx) = mpsc::channel::<CodexAppServerLiveTurnEvent>(256);

        self.publish(
            &context.hub,
            &key,
            WsServerEvent::SessionStatus {
                provider: context.provider,
                session_id: context.session_id.clone(),
                status: iowb_protocol::SessionRuntimeStatus::Running,
                response_id: Some(context.response_id.clone()),
                sequence: Some(context.next_sequence()),
                latest_user_prompt: Some(context.prompt.clone()),
            },
        )
        .await;

        tokio::spawn(async move {
            let mut output = String::new();
            let mut normalizer = CodexAppServerLiveOutputNormalizer::default();
            let mut runner = tokio::spawn(async move {
                client
                    .run_live_turn_with_options(
                        turn_params,
                        launch_options.as_ref(),
                        abort_rx,
                        event_tx,
                    )
                    .await
            });

            let outcome = loop {
                tokio::select! {
                    Some(event) = event_rx.recv() => {
                        process_codex_app_server_live_event(
                            &manager,
                            &context,
                            &key,
                            event,
                            &mut normalizer,
                            &mut output,
                        ).await;
                    }
                    result = &mut runner => {
                        while let Some(event) = event_rx.recv().await {
                            process_codex_app_server_live_event(
                                &manager,
                                &context,
                                &key,
                                event,
                                &mut normalizer,
                                &mut output,
                            ).await;
                        }
                        break result;
                    }
                }
            };

            let visible = normalizer.finish();
            publish_agent_output(&manager, &context, &key, &mut output, visible).await;
            let run_usage = normalizer.take_final_usage();
            let final_assistant = normalizer.take_final_assistant_message();
            let codex_error = normalizer.take_error();
            persist_normalized_tool_messages(&context, normalizer.take_tool_messages()).await;

            match outcome {
                Ok(Ok(outcome)) => {
                    persist_native_session_id(&context, Some(outcome.thread_id.clone())).await;
                    finish_codex_app_server_outcome(
                        &manager,
                        &key,
                        &context,
                        outcome,
                        final_assistant,
                        &output,
                        run_usage,
                        codex_error,
                    )
                    .await;
                }
                Ok(Err(error)) => {
                    let error_message = error.to_string();
                    manager
                        .publish(
                            &context.hub,
                            &key,
                            WsServerEvent::Error {
                                message: "Codex app-server live turn failed".to_string(),
                                details: Some(error_message.clone()),
                                session_id: Some(context.session_id.clone()),
                            },
                        )
                        .await;
                    manager
                        .finish(
                            &key,
                            &context,
                            iowb_protocol::SessionRuntimeStatus::Failed,
                            Some(error_message),
                            run_usage,
                        )
                        .await;
                }
                Err(error) => {
                    let error_message = format!("Codex app-server task failed: {error}");
                    manager
                        .publish(
                            &context.hub,
                            &key,
                            WsServerEvent::Error {
                                message: "Codex app-server task failed".to_string(),
                                details: Some(error_message.clone()),
                                session_id: Some(context.session_id.clone()),
                            },
                        )
                        .await;
                    manager
                        .finish(
                            &key,
                            &context,
                            iowb_protocol::SessionRuntimeStatus::Failed,
                            Some(error_message),
                            run_usage,
                        )
                        .await;
                }
            }
        });

        Ok(())
    }

    async fn start_direct_ai(&self, context: AgentStartContext) -> Result<()> {
        let (abort_tx, abort_rx) = oneshot::channel();
        let key = agent_run_key(context.provider, &context.session_id);

        self.register(key.clone(), abort_tx).await;

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

        let Some(config) = context.direct_ai_config.clone() else {
            let error_message =
                "Direct AI gateway is not configured. Configure IO Gateway in Settings before using gateway models with Claude/Gemini."
                    .to_string();
            self.publish(
                &context.hub,
                &key,
                WsServerEvent::Error {
                    message: "Direct AI gateway is not configured".to_string(),
                    details: Some(error_message.clone()),
                    session_id: Some(context.session_id.clone()),
                },
            )
            .await;
            self.finish(
                &key,
                &context,
                iowb_protocol::SessionRuntimeStatus::Failed,
                Some(error_message),
                None,
            )
            .await;
            return Ok(());
        };

        let Some(model) = context
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        else {
            let error_message = "Direct AI model is missing".to_string();
            self.publish(
                &context.hub,
                &key,
                WsServerEvent::Error {
                    message: error_message.clone(),
                    details: None,
                    session_id: Some(context.session_id.clone()),
                },
            )
            .await;
            self.finish(
                &key,
                &context,
                iowb_protocol::SessionRuntimeStatus::Failed,
                Some(error_message),
                None,
            )
            .await;
            return Ok(());
        };

        info!(
            provider = context.provider.as_str(),
            session_id = %context.session_id,
            project = %context.project_path.display(),
            model = %model,
            "starting Direct AI gateway agent request"
        );

        self.publish(
            &context.hub,
            &key,
            WsServerEvent::SessionStatus {
                provider: context.provider,
                session_id: context.session_id.clone(),
                status: iowb_protocol::SessionRuntimeStatus::Running,
                response_id: Some(context.response_id.clone()),
                sequence: Some(context.next_sequence()),
                latest_user_prompt: Some(context.prompt.clone()),
            },
        )
        .await;

        let manager = self.clone();
        tokio::spawn(async move {
            let mut abort_rx = abort_rx;
            tokio::select! {
                result = stream_direct_ai_model_api(
                    &config,
                    &model,
                    &context.direct_ai_messages,
                    {
                    let hub = context.hub.clone();
                    let key = key.clone();
                    let provider = context.provider;
                    let session_id = context.session_id.clone();
                    let response_id = context.response_id.clone();
                    let sequence = context.sequence.clone();
                    let manager = manager.clone();
                    move |chunk: String| {
                        let hub = hub.clone();
                        let key = key.clone();
                        let session_id = session_id.clone();
                        let response_id = response_id.clone();
                        let sequence = sequence.clone();
                        let manager = manager.clone();
                        async move {
                            manager.publish(&hub, &key, WsServerEvent::Output {
                                provider,
                                session_id,
                                content: chunk,
                                done: false,
                                response_id: Some(response_id),
                                sequence: Some(sequence.fetch_add(1, Ordering::Relaxed) + 1),
                            }).await;
                        }
                    }
                    },
                ) => {
                    match result {
                        Ok(output) => {
                            let mut bounded = String::new();
                            append_bounded(&mut bounded, &output.text, manager.max_output_bytes);
                            if !output.streamed && !bounded.is_empty() {
                                let chunks = direct_ai_display_chunks(&bounded);
                                let chunk_count = chunks.len();
                                for (index, chunk) in chunks.into_iter().enumerate() {
                                    manager.publish(&context.hub, &key, WsServerEvent::Output {
                                        provider: context.provider,
                                        session_id: context.session_id.clone(),
                                        content: chunk,
                                        done: false,
                                        response_id: Some(context.response_id.clone()),
                                        sequence: Some(context.next_sequence()),
                                    }).await;
                                    if index + 1 < chunk_count {
                                        tokio::time::sleep(Duration::from_millis(DIRECT_AI_SYNTHETIC_CHUNK_DELAY_MS)).await;
                                    }
                                }
                            }
                            manager.finish(
                                &key,
                                &context,
                                iowb_protocol::SessionRuntimeStatus::Completed,
                                Some(bounded),
                                output.usage,
                            ).await;
                        }
                        Err(error) => {
                            let error_message = format!("Direct AI gateway request failed\n\n{error}");
                            manager.publish(&context.hub, &key, WsServerEvent::Error {
                                message: "Direct AI gateway request failed".to_string(),
                                details: Some(error_message.clone()),
                                session_id: Some(context.session_id.clone()),
                            }).await;
                            manager.finish(
                                &key,
                                &context,
                                iowb_protocol::SessionRuntimeStatus::Failed,
                                Some(error_message),
                                None,
                            ).await;
                        }
                    }
                }
                _ = &mut abort_rx => {
                    manager.finish(
                        &key,
                        &context,
                        iowb_protocol::SessionRuntimeStatus::Aborted,
                        None,
                        None,
                    ).await;
                }
            }
        });

        Ok(())
    }

    async fn register(&self, key: String, abort_tx: oneshot::Sender<()>) {
        let mut runs = self.runs.write().await;
        runs.insert(
            key,
            AgentRuntimeRecord {
                replay: VecDeque::new(),
                replay_bytes: 0,
                abort_tx: Some(abort_tx),
                last_activity: Utc::now(),
            },
        );
        while runs.len() > self.max_runs {
            if let Some(oldest_key) = runs
                .iter()
                .min_by_key(|(_, record)| record.last_activity)
                .map(|(key, _)| key.clone())
            {
                runs.remove(&oldest_key);
            } else {
                break;
            }
        }
    }

    async fn is_running(&self, provider: Provider, session_id: &str) -> bool {
        let key = agent_run_key(provider, session_id);
        self.runs
            .read()
            .await
            .get(&key)
            .is_some_and(|record| record.abort_tx.is_some())
    }

    async fn publish_context_recovery_if_needed(
        &self,
        key: &str,
        context: &AgentStartContext,
        error: &CodexTurnError,
    ) {
        if context.context_rollover_id.is_some()
            || context.native_resume_session_id.is_none()
            || !is_request_body_too_large_error(error)
        {
            return;
        }
        let Some(run_id) = context.durable_run_id.as_deref() else {
            return;
        };
        let failed_message_id = context
            .storage
            .get_durable_chat_run(run_id)
            .ok()
            .flatten()
            .and_then(|run| run.user_message_id)
            .unwrap_or_default();
        if failed_message_id.is_empty() {
            return;
        }
        self.publish(
            &context.hub,
            key,
            WsServerEvent::ChatRecoveryRequired {
                provider: context.provider,
                session_id: context.session_id.clone(),
                response_id: Some(context.response_id.clone()),
                recovery: ChatContextRecovery {
                    code: "context_too_large".to_string(),
                    state: "required".to_string(),
                    message: "This chat's native context is too large to resume safely. Compact it into a clean context and retry the same message.".to_string(),
                    failed_message_id,
                    observed_bytes: error.observed_bytes,
                    limit_bytes: error.limit_bytes.unwrap_or(CODEX_GATEWAY_BODY_LIMIT_BYTES),
                    request_id: None,
                },
            },
        )
        .await;
    }

    async fn publish(&self, hub: &WsHub, key: &str, event: WsServerEvent) {
        {
            let mut runs = self.runs.write().await;
            if let Some(record) = runs.get_mut(key) {
                record.last_activity = Utc::now();
                let event_bytes = ws_event_estimated_bytes(&event);
                while record.replay.len() >= self.max_replay_events
                    || (!record.replay.is_empty()
                        && record.replay_bytes.saturating_add(event_bytes) > self.max_replay_bytes)
                {
                    if let Some(removed) = record.replay.pop_front() {
                        record.replay_bytes = record
                            .replay_bytes
                            .saturating_sub(ws_event_estimated_bytes(&removed));
                    }
                }
                record.replay_bytes = record.replay_bytes.saturating_add(event_bytes);
                record.replay.push_back(event.clone());
            }
        }
        hub.publish(event);
    }

    async fn finish(
        &self,
        key: &str,
        context: &AgentStartContext,
        status: iowb_protocol::SessionRuntimeStatus,
        assistant_output: Option<String>,
        usage: Option<NormalizedRunUsage>,
    ) -> Option<ContextRolloverFollowUp> {
        let mut status = status;
        let received_at = Utc::now();
        let output = assistant_output
            .map(|output| output.trim().to_string())
            .filter(|output| !output.is_empty())
            .or_else(|| match status {
                iowb_protocol::SessionRuntimeStatus::Failed => Some("Failed".to_string()),
                iowb_protocol::SessionRuntimeStatus::Aborted => Some("Aborted".to_string()),
                _ => None,
            })
            .map(|output| {
                bound_agent_text(
                    &output,
                    AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
                    "assistant response",
                )
            });
        let mut rollover_completed_atomically = false;
        let mut rollover_follow_up = None;
        if let Some(rollover_id) = context.context_rollover_id.as_deref() {
            if matches!(status, iowb_protocol::SessionRuntimeStatus::Completed) {
                match activate_completed_context_rollover(context, rollover_id, received_at).await {
                    Ok(follow_up) => {
                        rollover_completed_atomically = true;
                        rollover_follow_up = follow_up;
                    }
                    Err(error) => {
                        let message = format!("failed to activate clean context: {error}");
                        let _ = context.storage.fail_context_rollover(rollover_id, &message);
                        self.publish(
                            &context.hub,
                            key,
                            WsServerEvent::Error {
                                message: "clean context could not be activated".to_string(),
                                details: Some(message),
                                session_id: Some(context.session_id.clone()),
                            },
                        )
                        .await;
                        status = iowb_protocol::SessionRuntimeStatus::Failed;
                    }
                }
            } else {
                let message = match status {
                    iowb_protocol::SessionRuntimeStatus::Aborted => {
                        "clean context compaction was aborted"
                    }
                    _ => "clean context compaction failed",
                };
                let _ = context.storage.fail_context_rollover(rollover_id, message);
            }
        }
        // A rollover response is committed together with its mapping, marker,
        // and durable-run completion. On any rollover failure, leave the
        // visible transcript untouched so the same recovery can be retried.
        if context.context_rollover_id.is_none()
            && let Some(output) = output
        {
            let persisted_output = output.clone();
            // Persist the assistant message with footer metadata so the
            // bubble at the bottom of the reply stays populated after a
            // refresh or session switch.
            let sent_at = context
                .storage
                .get_session_summary(&context.session_id)
                .ok()
                .flatten()
                .and_then(|s| s.first_user_at);
            let elapsed_ms = sent_at.map(|t| (received_at - t).num_milliseconds().max(0));
            let assistant_meta = serde_json::json!({
                "cli": context.provider.as_str(),
                "durableRunId": context.durable_run_id,
                "model": context.model.clone().unwrap_or_default(),
                "runtime": context.runtime,
                "effort": context.effort.clone().unwrap_or_default(),
                "mode": context.mode.clone().unwrap_or_default(),
                "thinking": context.thinking.unwrap_or(false),
                "fast": context.fast.unwrap_or(false),
                "receivedAt": received_at.to_rfc3339(),
                "sentAt": sent_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                "elapsedMs": elapsed_ms,
                "status": status,
            });
            if let Err(error) = context
                .sessions
                .append_message_with_metadata(
                    &context.session_id,
                    MessageRole::Assistant,
                    output,
                    Some(assistant_meta),
                )
                .await
            {
                warn!(error = %error, session_id = %context.session_id, "failed to persist assistant message");
            } else {
                // Re-stamp with the elapsed value once we know the receiver
                // timestamp is committed (subsquent UI fetches add token
                // usage separately).
                if elapsed_ms.is_some() {
                    let updated = serde_json::json!({
                        "cli": context.provider.as_str(),
                        "durableRunId": context.durable_run_id,
                        "model": context.model.clone().unwrap_or_default(),
                        "runtime": context.runtime,
                        "effort": context.effort.clone().unwrap_or_default(),
                        "mode": context.mode.clone().unwrap_or_default(),
                        "thinking": context.thinking.unwrap_or(false),
                        "fast": context.fast.unwrap_or(false),
                        "receivedAt": received_at.to_rfc3339(),
                        "sentAt": sent_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                        "elapsedMs": elapsed_ms,
                        "status": status,
                    });
                    let _ = context.sessions.stamp_latest_message_metadata(
                        &context.session_id,
                        MessageRole::Assistant,
                        updated,
                    );
                }
                if matches!(status, iowb_protocol::SessionRuntimeStatus::Completed)
                    && context.provider == Provider::Codex
                    && !context.native_rollout_owned_by_provider
                {
                    let native_prompt = resolve_cli_slash_prompt(Provider::Codex, &context.prompt)
                        .unwrap_or_else(|_| context.prompt.clone());
                    if let Err(error) = context
                        .sessions
                        .sync_codex_turn_to_native_rollout(
                            &context.session_id,
                            &native_prompt,
                            &persisted_output,
                        )
                        .await
                    {
                        warn!(
                            error = %error,
                            session_id = %context.session_id,
                            "failed to sync Codex turn into native rollout"
                        );
                    }
                }
            }
        }

        if let Err(error) = context
            .sessions
            .set_active(&context.session_id, false)
            .await
        {
            warn!(error = %error, session_id = %context.session_id, "failed to mark session inactive");
        }

        let lifetime_token_usage = persist_run_attempt_usage(context, status, usage.as_ref()).await;
        let context_token_usage = context
            .storage
            .session_context_token_usage(&context.session_id)
            .map_err(|error| {
                warn!(
                    error = %error,
                    session_id = %context.session_id,
                    "failed to load scoped chat token usage"
                );
                error
            })
            .ok();
        let spent_token_usage = context
            .storage
            .session_spent_token_usage(&context.session_id)
            .map_err(|error| {
                warn!(
                    error = %error,
                    session_id = %context.session_id,
                    "failed to load spent chat token usage"
                );
                error
            })
            .ok();

        if !rollover_completed_atomically && let Some(run_id) = context.durable_run_id.as_deref() {
            let terminal_result = match status {
                iowb_protocol::SessionRuntimeStatus::Completed => {
                    context.storage.mark_durable_chat_run_completed(run_id)
                }
                iowb_protocol::SessionRuntimeStatus::Aborted => context
                    .storage
                    .mark_durable_chat_run_terminal(run_id, "aborted", None),
                iowb_protocol::SessionRuntimeStatus::Failed => context
                    .storage
                    .mark_durable_chat_run_failed(run_id, "provider run failed"),
                _ => context.storage.mark_durable_chat_run_terminal(
                    run_id,
                    "interrupted",
                    Some("provider run ended with a non-terminal runtime status"),
                ),
            };
            if let Err(error) = terminal_result {
                warn!(
                    error = %error,
                    run_id,
                    session_id = %context.session_id,
                    "failed to mark durable chat run terminal"
                );
            }
        }

        // Stamp metadata so the UI can show "received at", normalized usage,
        // and the conversation metadata snapshot without a follow-up rollout
        // scan. Legacy sessions can still use the token-usage endpoint.
        if let Ok(Some(mut session)) = context.storage.get_session_summary(&context.session_id) {
            // Atomic rollover completion owns the marker timestamp. Do not
            // regress its persisted last-message/activity timestamp during
            // the generic footer pass.
            let completed_at = if rollover_completed_atomically {
                session.last_activity
            } else {
                received_at
            };
            session.received_at = Some(received_at);
            session.last_message_at = Some(completed_at);
            session.last_activity = completed_at;
            session.effort = context.effort.clone().or(session.effort);
            session.mode = context.mode.clone().or(session.mode);
            session.thinking = context.thinking.or(session.thinking);
            session.fast = context.fast.or(session.fast);
            session.token_usage = usage
                .as_ref()
                .map(|usage| usage.usage.clone())
                .or(session.token_usage);
            let snapshot = session.clone();
            if let Err(error) = context.storage.upsert_session(&session) {
                warn!(error = %error, session_id = %context.session_id, "failed to persist session metadata");
            }
            // Broadcast the new snapshot so the UI updates the bubble footer.
            self.publish(
                &context.hub,
                key,
                WsServerEvent::SessionMetadata {
                    provider: context.provider,
                    session_id: context.session_id.clone(),
                    model: snapshot.model,
                    effort: snapshot.effort,
                    mode: snapshot.mode,
                    thinking: snapshot.thinking,
                    fast: snapshot.fast,
                    native_session_id: snapshot.native_session_id,
                    received_at,
                    last_message_at: snapshot.last_message_at,
                    first_user_at: snapshot.first_user_at,
                    token_usage: snapshot.token_usage,
                    lifetime_token_usage: lifetime_token_usage
                        .clone()
                        .or(snapshot.lifetime_token_usage),
                    context_token_usage: context_token_usage
                        .clone()
                        .or(snapshot.context_token_usage),
                    spent_token_usage: spent_token_usage.clone().or(snapshot.spent_token_usage),
                    response_id: Some(context.response_id.clone()),
                    sequence: Some(context.next_sequence()),
                },
            )
            .await;
        }

        self.publish(
            &context.hub,
            key,
            WsServerEvent::Output {
                provider: context.provider,
                session_id: context.session_id.clone(),
                content: String::new(),
                done: true,
                response_id: Some(context.response_id.clone()),
                sequence: Some(context.next_sequence()),
            },
        )
        .await;
        self.publish(
            &context.hub,
            key,
            WsServerEvent::SessionStatus {
                provider: context.provider,
                session_id: context.session_id.clone(),
                status,
                response_id: Some(context.response_id.clone()),
                sequence: Some(context.next_sequence()),
                latest_user_prompt: Some(context.prompt.clone()),
            },
        )
        .await;
        context.hub.publish(WsServerEvent::ActiveSessions {
            sessions: context.sessions.list_active().await,
        });

        let mut runs = self.runs.write().await;
        if let Some(record) = runs.get_mut(key) {
            record.abort_tx = None;
            record.last_activity = Utc::now();
        }
        drop(runs);

        rollover_follow_up
    }

    async fn start_context_rollover_follow_up(
        &self,
        context: &AgentStartContext,
        follow_up: ContextRolloverFollowUp,
    ) {
        let run = follow_up.run;
        let run_id = run.id.clone();
        let session_id = run.session_id.clone();
        let key = agent_run_key(context.provider, &session_id);
        let fail_follow_up = |storage: &Storage, sessions: &SessionManager, message: String| {
            let run_id = run_id.clone();
            let session_id = session_id.clone();
            let storage = storage.clone();
            let sessions = sessions.clone();
            async move {
                let _ = storage.mark_durable_chat_run_failed(&run_id, &message);
                let _ = sessions.set_active(&session_id, false).await;
            }
        };

        if let Err(error) = context.sessions.set_active(&session_id, true).await {
            let message =
                format!("failed to activate original prompt after clean context: {error}");
            fail_follow_up(&context.storage, &context.sessions, message.clone()).await;
            self.publish(
                &context.hub,
                &key,
                WsServerEvent::Error {
                    message: "clean-context retry could not start".to_string(),
                    details: Some(message),
                    session_id: Some(session_id),
                },
            )
            .await;
            return;
        }

        let direct_ai_messages =
            if should_use_direct_ai_gateway_runtime(context.provider, run.model.as_deref()) {
                match context.sessions.messages(&session_id) {
                    Ok(messages) => direct_ai_conversation_messages(messages, run.prompt.as_str()),
                    Err(error) => {
                        let message =
                            format!("failed to build retry history after clean context: {error}");
                        fail_follow_up(&context.storage, &context.sessions, message.clone()).await;
                        self.publish(
                            &context.hub,
                            &key,
                            WsServerEvent::Error {
                                message: "clean-context retry could not start".to_string(),
                                details: Some(message),
                                session_id: Some(session_id),
                            },
                        )
                        .await;
                        return;
                    }
                }
            } else {
                Vec::new()
            };

        let attempt_id = new_id("attempt");
        if let Err(error) = context
            .storage
            .create_chat_run_attempt(&StoredChatRunAttempt::new(
                attempt_id.clone(),
                run_id.clone(),
                session_id.clone(),
                run.user_message_id.clone(),
                context.provider.as_str(),
                runtime_label(context.runtime),
                run.model.clone(),
                run.native_session_id.clone(),
            ))
        {
            let message = format!("failed to create retry attempt after clean context: {error}");
            fail_follow_up(&context.storage, &context.sessions, message.clone()).await;
            self.publish(
                &context.hub,
                &key,
                WsServerEvent::Error {
                    message: "clean-context retry could not start".to_string(),
                    details: Some(message),
                    session_id: Some(session_id),
                },
            )
            .await;
            return;
        }

        let follow_up_context = AgentStartContext {
            provider: context.provider,
            session_id: session_id.clone(),
            durable_run_id: Some(run_id.clone()),
            attempt_id: Some(attempt_id),
            response_id: new_id("response"),
            sequence: Arc::new(AtomicU64::new(0)),
            project_path: context.project_path.clone(),
            prompt: run.prompt.clone(),
            model: run.model.clone(),
            runtime: context.runtime,
            effort: run.effort.clone(),
            mode: run.mode.clone(),
            thinking: run.thinking,
            fast: run.fast,
            native_resume_session_id: run.native_session_id.clone(),
            native_rollout_owned_by_provider: false,
            context_rollover_id: None,
            direct_ai_config: context.direct_ai_config.clone(),
            direct_ai_messages,
            sessions: context.sessions.clone(),
            storage: context.storage.clone(),
            hub: context.hub.clone(),
        };
        let start_future: Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> =
            Box::pin(self.start(follow_up_context));
        let start_result = start_future.await;

        if let Err(error) = start_result {
            let message = format!("failed to start original prompt after clean context: {error}");
            fail_follow_up(&context.storage, &context.sessions, message.clone()).await;
            self.publish(
                &context.hub,
                &key,
                WsServerEvent::Error {
                    message: "clean-context retry could not start".to_string(),
                    details: Some(message),
                    session_id: Some(session_id.clone()),
                },
            )
            .await;
        }
        context.hub.publish(WsServerEvent::ActiveSessions {
            sessions: context.sessions.list_active().await,
        });
        info!(
            session_id = %session_id,
            run_id = %run_id,
            "started original prompt after clean context activation"
        );
    }

    pub async fn abort(&self, provider: Provider, session_id: &str) -> bool {
        let key = agent_run_key(provider, session_id);
        let abort_tx = {
            let mut runs = self.runs.write().await;
            runs.get_mut(&key).and_then(|record| record.abort_tx.take())
        };
        if let Some(abort_tx) = abort_tx {
            let _ = abort_tx.send(());
            true
        } else {
            false
        }
    }

    pub async fn replay_events(&self) -> Vec<WsServerEvent> {
        let runs = self.runs.read().await;
        let mut active = runs
            .values()
            .filter(|record| record.abort_tx.is_some())
            .collect::<Vec<_>>();
        active.sort_by_key(|record| record.last_activity);
        let mut replay = VecDeque::new();
        let mut replay_bytes = 0usize;
        for event in active
            .into_iter()
            .flat_map(|record| record.replay.iter().cloned())
        {
            replay_bytes = replay_bytes.saturating_add(ws_event_estimated_bytes(&event));
            replay.push_back(event);
            while replay.len() > AGENT_REPLAY_TOTAL_MAX_EVENTS
                || replay_bytes > AGENT_REPLAY_TOTAL_MAX_BYTES
            {
                let Some(removed) = replay.pop_front() else {
                    break;
                };
                replay_bytes = replay_bytes.saturating_sub(ws_event_estimated_bytes(&removed));
            }
        }
        replay.into()
    }
}

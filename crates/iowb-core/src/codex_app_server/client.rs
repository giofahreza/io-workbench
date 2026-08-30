impl CodexAppServerClient {
    pub(crate) fn new(command: impl Into<OsString>, request_timeout: Duration) -> Self {
        Self {
            command: command.into(),
            request_timeout,
        }
    }

    pub(crate) async fn read_thread(&self, thread_id: &str) -> Result<CodexThreadSnapshot> {
        self.read_thread_with_options(thread_id, None).await
    }

    pub(crate) async fn read_thread_with_options(
        &self,
        thread_id: &str,
        launch_options: Option<&CodexAppServerLaunchOptions>,
    ) -> Result<CodexThreadSnapshot> {
        let result = self
            .request(
                "thread/read",
                json!({
                    "threadId": thread_id,
                    "includeTurns": true,
                }),
                launch_options,
            )
            .await?;
        parse_thread_snapshot(&result)
    }

    pub(crate) async fn fork_thread(&self, thread_id: &str, last_turn_id: &str) -> Result<String> {
        self.fork_thread_with_options(thread_id, last_turn_id, None)
            .await
    }

    pub(crate) async fn fork_thread_with_options(
        &self,
        thread_id: &str,
        last_turn_id: &str,
        launch_options: Option<&CodexAppServerLaunchOptions>,
    ) -> Result<String> {
        let result = self
            .request(
                "thread/fork",
                json!({
                    "threadId": thread_id,
                    "lastTurnId": last_turn_id,
                }),
                launch_options,
            )
            .await?;
        result
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                CoreError::InvalidInput(
                    "Codex app-server returned a fork without a thread id".to_string(),
                )
            })
    }

    pub(crate) async fn delete_thread(&self, thread_id: &str) -> Result<()> {
        self.request("thread/delete", json!({ "threadId": thread_id }), None)
            .await?;
        Ok(())
    }

    pub(crate) async fn compact_thread_and_wait_with_options(
        &self,
        thread_id: &str,
        launch_options: Option<&CodexAppServerLaunchOptions>,
    ) -> Result<()> {
        let thread_id = thread_id.trim().to_string();
        if thread_id.is_empty() {
            return Err(CoreError::InvalidInput(
                "Codex thread id is required for compaction".to_string(),
            ));
        }
        let command = self.command.clone();
        let launch_options = launch_options.cloned().unwrap_or_default();
        let compact_timeout = self.request_timeout.max(Duration::from_secs(120));
        let mut child_command = app_server_command(&command, &launch_options);
        let mut child = child_command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stderr = start_stderr_capture(&mut child, "compaction");
        let mut stdin = child.stdin.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdin was unavailable",
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdout was unavailable",
            ))
        })?;
        let mut lines = BufReader::new(stdout).lines();

        let result = timeout(compact_timeout, async {
            write_json_line(
                &mut stdin,
                &json!({
                    "method": "initialize",
                    "id": 1,
                    "params": {
                        "clientInfo": {
                            "name": "io_workbench",
                            "title": "io-workbench",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "capabilities": {
                            "optOutNotificationMethods": ["thread/started", "item/agentMessage/delta"],
                        },
                    },
                }),
            )
            .await?;
            let _ = read_response(&mut lines, 1).await?;
            write_json_line(&mut stdin, &json!({ "method": "initialized" })).await?;
            write_json_line(
                &mut stdin,
                &json!({
                    "method": "thread/resume",
                    "id": 2,
                    "params": { "threadId": &thread_id },
                }),
            )
            .await?;
            let _ = read_response(&mut lines, 2).await?;
            write_json_line(
                &mut stdin,
                &json!({
                    "method": "thread/compact/start",
                    "id": 3,
                    "params": { "threadId": &thread_id },
                }),
            )
            .await?;
            let _ = read_response(&mut lines, 3).await?;
            let result = wait_for_context_compaction(&mut lines, &thread_id).await;
            result
        })
        .await;
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "Codex app-server compaction timed out after {} ms",
                    compact_timeout.as_millis()
                ),
            ))),
        };
        if result.is_err() {
            log_captured_stderr(&stderr, "compaction").await;
        }
        let _ = stdin.shutdown().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
        result
    }

    pub(crate) async fn run_live_turn_with_options(
        &self,
        params: CodexAppServerLiveTurnParams,
        launch_options: Option<&CodexAppServerLaunchOptions>,
        abort_rx: oneshot::Receiver<()>,
        events: mpsc::Sender<CodexAppServerLiveTurnEvent>,
    ) -> Result<CodexAppServerLiveTurnOutcome> {
        let command = self.command.clone();
        let launch_options = launch_options.cloned().unwrap_or_default();
        let request_timeout = self.request_timeout;
        let mut child_command = app_server_command(&command, &launch_options);
        let mut child = child_command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stderr = start_stderr_capture(&mut child, "live turn");
        let mut stdin = child.stdin.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdin was unavailable",
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdout was unavailable",
            ))
        })?;
        let mut lines = BufReader::new(stdout).lines();

        let result = timeout(request_timeout, async {
            initialize_app_server(&mut stdin, &mut lines, &events).await?;
            let thread_id =
                start_or_resume_thread(&mut stdin, &mut lines, &params, &events).await?;
            let _ = events
                .send(CodexAppServerLiveTurnEvent::ThreadAssociated {
                    thread_id: thread_id.clone(),
                })
                .await;
            let turn_id = start_turn(&mut stdin, &mut lines, &params, &thread_id, &events).await?;
            if let Some(turn_id) = turn_id.as_deref() {
                let _ = events
                    .send(CodexAppServerLiveTurnEvent::TurnAssociated {
                        turn_id: turn_id.to_string(),
                    })
                    .await;
            }
            Ok::<_, CoreError>((thread_id, turn_id))
        })
        .await;

        let (thread_id, mut turn_id) = match result {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                log_captured_stderr(&stderr, "live turn setup").await;
                let _ = stdin.shutdown().await;
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(error);
            }
            Err(_) => {
                let error = CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "Codex app-server live turn setup timed out after {} ms",
                        request_timeout.as_millis()
                    ),
                ));
                log_captured_stderr(&stderr, "live turn setup").await;
                let _ = stdin.shutdown().await;
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(error);
            }
        };

        let mut abort_rx = abort_rx;
        let mut interrupt_request_id = 4_i64;
        let mut abort_deadline: Option<std::pin::Pin<Box<Sleep>>> = None;
        let mut abort_requested = false;
        let mut saw_current_turn_notification = false;
        let outcome = loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Some(line) = line? else {
                        break Err(CoreError::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "Codex app-server closed before the turn completed",
                        )));
                    };
                    if let Some(outcome) = handle_live_jsonrpc_line(
                        &line,
                        &mut stdin,
                        &thread_id,
                        &mut turn_id,
                        &mut saw_current_turn_notification,
                        &events,
                    ).await? {
                        break Ok(outcome);
                    }
                }
                _ = &mut abort_rx, if !abort_requested => {
                    abort_requested = true;
                    if let Some(active_turn_id) = turn_id.as_deref() {
                        write_json_line(
                            &mut stdin,
                            &json!({
                                "method": "turn/interrupt",
                                "id": interrupt_request_id,
                                "params": {
                                    "threadId": &thread_id,
                                    "turnId": active_turn_id,
                                },
                            }),
                        )
                        .await?;
                        interrupt_request_id += 1;
                        abort_deadline = Some(Box::pin(tokio::time::sleep_until(
                            Instant::now() + Duration::from_secs(2),
                        )));
                    } else {
                        break Ok(CodexAppServerLiveTurnOutcome {
                            thread_id: thread_id.clone(),
                            turn_id: None,
                            status: CodexAppServerTurnTerminalStatus::Interrupted,
                            turn: None,
                        });
                    }
                }
                _ = async {
                    if let Some(deadline) = abort_deadline.as_mut() {
                        deadline.as_mut().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if abort_deadline.is_some() => {
                    break Ok(CodexAppServerLiveTurnOutcome {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        status: CodexAppServerTurnTerminalStatus::Interrupted,
                        turn: None,
                    });
                }
            }
        };

        if outcome.is_err() {
            log_captured_stderr(&stderr, "live turn").await;
        }
        let _ = stdin.shutdown().await;
        let _ = child.start_kill();
        let _ = timeout(Duration::from_secs(1), child.wait()).await;
        outcome
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        launch_options: Option<&CodexAppServerLaunchOptions>,
    ) -> Result<Value> {
        let method = method.to_string();
        let command = self.command.clone();
        let launch_options = launch_options.cloned().unwrap_or_default();
        let mut child_command = app_server_command(&command, &launch_options);
        let mut child = child_command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stderr = start_stderr_capture(&mut child, "request");
        let mut stdin = child.stdin.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdin was unavailable",
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdout was unavailable",
            ))
        })?;
        let mut lines = BufReader::new(stdout).lines();

        let result = timeout(self.request_timeout, async {
            write_json_line(
                &mut stdin,
                &json!({
                    "method": "initialize",
                    "id": 1,
                    "params": {
                        "clientInfo": {
                            "name": "io_workbench",
                            "title": "io-workbench",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "capabilities": {
                            "optOutNotificationMethods": ["thread/started"],
                        },
                    },
                }),
            )
            .await?;
            let _ = read_response(&mut lines, 1).await?;
            write_json_line(&mut stdin, &json!({ "method": "initialized" })).await?;
            write_json_line(
                &mut stdin,
                &json!({
                    "method": method,
                    "id": 2,
                    "params": params,
                }),
            )
            .await?;
            let result = read_response(&mut lines, 2).await;
            result
        })
        .await;
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "Codex app-server request timed out after {} ms",
                    self.request_timeout.as_millis()
                ),
            ))),
        };
        if result.is_err() {
            log_captured_stderr(&stderr, "request").await;
        }
        let _ = stdin.shutdown().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
        result
    }
}

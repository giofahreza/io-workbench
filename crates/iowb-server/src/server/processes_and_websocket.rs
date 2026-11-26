include!("terminals.rs");

async fn list_processes(State(state): State<AppState>) -> Json<Vec<iowb_protocol::ProcessInfo>> {
    Json(state.processes.list().await)
}

async fn start_process(
    State(state): State<AppState>,
    Json(request): Json<ProcessStartRequest>,
) -> Result<Json<ProcessStartResponse>> {
    Ok(Json(state.processes.start(request).await?))
}

async fn abort_process(
    State(state): State<AppState>,
    AxumPath(process_id): AxumPath<String>,
) -> Result<Json<PlaceholderResponse>> {
    state.processes.abort(&process_id).await?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "process abort requested".to_string(),
    }))
}

async fn write_process_input(
    State(state): State<AppState>,
    AxumPath(process_id): AxumPath<String>,
    Json(request): Json<ProcessInputRequest>,
) -> Result<Json<PlaceholderResponse>> {
    state
        .processes
        .write_input(&process_id, request.data.into_bytes())
        .await?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "process input written".to_string(),
    }))
}

async fn resize_process(
    State(state): State<AppState>,
    AxumPath(process_id): AxumPath<String>,
    Json(request): Json<ProcessResizeRequest>,
) -> Result<Json<PlaceholderResponse>> {
    state
        .processes
        .resize_terminal(&process_id, request.cols, request.rows)
        .await?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "process resize accepted".to_string(),
    }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let token = request_token(&headers, uri.query());
    let user = match state.auth.require_user(token.as_deref()) {
        Ok(user) => user,
        Err(error) => return ServerError::from(error).into_response(),
    };

    ws.on_upgrade(move |socket| handle_socket(socket, state, user))
        .into_response()
}

async fn handle_socket(socket: WebSocket, state: AppState, user: iowb_protocol::UserProfile) {
    let connection_id = new_id("conn");
    let (mut sender, mut receiver) = socket.split();
    let (command_tx, mut command_rx) =
        mpsc::channel::<WsClientCommand>(WS_COMMAND_CHANNEL_CAPACITY);
    let (direct_tx, mut direct_rx) =
        mpsc::channel::<WsServerEvent>(iowb_protocol::WS_EVENT_CHANNEL_CAPACITY);
    let mut hub_rx = state.ws_hub.subscribe();
    let mut board_session_subscriptions = HashSet::<String>::new();
    let mut chat_session_subscriptions: Option<HashSet<String>> = None;

    let reader_connection_id = connection_id.clone();
    tokio::spawn(async move {
        while let Some(message) = receiver.next().await {
            match message {
                Ok(Message::Text(text)) => match serde_json::from_str::<WsClientCommand>(&text) {
                    Ok(command) => {
                        if command_tx.send(command).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        warn!(
                            connection_id = %reader_connection_id,
                            error = %error,
                            "invalid websocket command"
                        );
                    }
                },
                Ok(Message::Close(_)) => break,
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Binary(_)) => {}
                Err(error) => {
                    warn!(connection_id = %reader_connection_id, error = %error, "websocket read failed");
                    break;
                }
            }
        }
    });

    let _ = direct_tx
        .send(WsServerEvent::Connected {
            connection_id: connection_id.clone(),
            server_time: Utc::now(),
        })
        .await;
    let _ = direct_tx
        .send(WsServerEvent::ActiveSessions {
            sessions: state.sessions.list_active().await,
        })
        .await;

    loop {
        tokio::select! {
            Some(command) = command_rx.recv() => {
                handle_ws_command(
                    &state,
                    &direct_tx,
                    &user,
                    command,
                    &mut board_session_subscriptions,
                    &mut chat_session_subscriptions,
                ).await;
            }
            Some(event) = direct_rx.recv() => {
                let event = state.ws_hub.prepare_event(event);
                // Direct failures acknowledge this connection's command;
                // streaming/replayed board events still require subscriptions.
                if (matches!(&event, WsServerEvent::Error { .. } | WsServerEvent::ChatSetupRequired { .. } | WsServerEvent::ChatRecoveryRequired { .. }) || ws_event_visible_to_connection(
                    &state,
                    &event,
                    &board_session_subscriptions,
                    &chat_session_subscriptions,
                )) && send_ws_event(&mut sender, event).await.is_err() {
                    break;
                }
            }
            event = hub_rx.recv() => {
                match event {
                    Ok(event) => {
                        if ws_event_visible_to_connection(
                            &state,
                            &event,
                            &board_session_subscriptions,
                            &chat_session_subscriptions,
                        ) && send_ws_event(&mut sender, event).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        let warning = WsServerEvent::Error {
                            history_message: None,
                            message: "websocket client fell behind".to_string(),
                            details: Some(format!("skipped {skipped} events")),
                            session_id: None,
                        };
                        if send_ws_event(&mut sender, warning).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            else => break,
        }
    }

    debug!(%connection_id, "websocket disconnected");
}

async fn handle_ws_command(
    state: &AppState,
    direct_tx: &mpsc::Sender<WsServerEvent>,
    user: &iowb_protocol::UserProfile,
    command: WsClientCommand,
    board_session_subscriptions: &mut HashSet<String>,
    chat_session_subscriptions: &mut Option<HashSet<String>>,
) {
    match command {
        WsClientCommand::Ping { nonce } => {
            let _ = direct_tx
                .send(WsServerEvent::Pong {
                    nonce,
                    server_time: Utc::now(),
                })
                .await;
        }
        WsClientCommand::Subscribe {
            session_ids,
            chat_session_ids,
            process_ids,
            ..
        } => {
            *board_session_subscriptions =
                validated_board_session_subscriptions(state, session_ids);
            *chat_session_subscriptions = validated_chat_session_subscriptions(chat_session_ids);
            let _ = direct_tx
                .send(WsServerEvent::ActiveSessions {
                    sessions: state.sessions.list_active().await,
                })
                .await;
            for event in state.replay_agent_events().await {
                if ws_event_visible_to_connection(
                    state,
                    &event,
                    board_session_subscriptions,
                    chat_session_subscriptions,
                ) {
                    // This command is handled by the same task that drains
                    // `direct_tx`. Waiting on a full channel here would
                    // deadlock a reconnect with many active replays.
                    if direct_tx.try_send(event).is_err() {
                        break;
                    }
                }
            }
            // Terminal output is transient, but a mobile Activity may be
            // recreated while its long-lived Termux PTY survives. Replay a
            // small bounded tail only for the explicit process ids the
            // authenticated client says it owns. Live output still comes via
            // the regular hub subscription below.
            let mut replayed_processes = HashSet::new();
            for process_id in process_ids
                .into_iter()
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
                .filter(|id| replayed_processes.insert(id.clone()))
                .take(TERMINAL_REPLAY_MAX_PROCESSES)
            {
                let Ok(chunks) = state.processes.output_snapshot(&process_id).await else {
                    continue;
                };
                for chunk in chunks {
                    if direct_tx
                        .try_send(WsServerEvent::ProcessOutput {
                            process_id: process_id.clone(),
                            stream: chunk.stream,
                            data: chunk.data,
                            sequence: Some(chunk.sequence),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        WsClientCommand::StartSession {
            provider,
            request_id,
            runtime: requested_runtime,
            project_path,
            prompt,
            session_id,
            model,
            effort,
            mode,
            thinking,
            fast,
        } => {
            let session_id = Some(session_id.unwrap_or_else(|| new_id("session")));
            let imported = state.native_chat_configuration(provider, session_id.as_deref()).await;
            let runtime = imported.as_ref().and_then(|session| session.runtime).unwrap_or_else(|| requested_chat_runtime(
                state,
                &user.id,
                provider,
                session_id.as_deref(),
                requested_runtime,
            ));
            // Context recovery takes precedence over unrelated setup checks.
            if let Some(id) = session_id.as_deref()
                && let Ok(Some(recovery)) = state.context_recovery(id).await
            {
                let _ = direct_tx
                    .send(WsServerEvent::ChatRecoveryRequired {
                        provider,
                        session_id: id.to_string(),
                        response_id: None,
                        recovery,
                    })
                    .await;
                return;
            }
            let mut model = model
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty());
            let inherit_native = runtime == ChatRuntime::NativeCli && imported.as_ref().is_some_and(|session| {
                model.as_deref().is_none_or(|model| session.model.as_deref() == Some(model))
            });
            if inherit_native { model = None; }
            let mut problem = native_model_problem(runtime, model.as_deref());
            if let Some(session) = session_id
                .as_deref()
                .and_then(|id| state.storage.get_session_summary(id).ok().flatten())
                && session.provider != provider
            {
                problem = Some(chat_setup_problem(
                    "chat_provider_mismatch",
                    "This chat belongs to a different provider. Start a new chat to change providers.",
                    "choose_route",
                    None,
                ));
            }
            if problem.is_none() {
                let mut catalog = chat_route_catalog(state, &user.id, provider, runtime, true).await;
                apply_imported_chat_catalog(&mut catalog, imported.as_ref());
                problem = catalog.get("problem").filter(|v| !v.is_null()).cloned();
                if problem.is_none() {
                    let stored = session_id
                        .as_deref()
                        .and_then(|id| state.storage.get_session_summary(id).ok().flatten());
                    match checked_route_model(
                        runtime,
                        model.as_deref(),
                        &catalog,
                        stored.as_ref().and_then(|session| session.model.as_deref()),
                    ) {
                        Ok(canonical) => model = canonical,
                        Err(error) => problem = Some(error),
                    }
                }
            }
            if let Some(problem) = problem {
                let history_message = match persist_rejected_chat_send(
                    state, &user.id, provider, runtime, session_id.as_deref().unwrap(),
                    request_id.as_deref(), &project_path, &prompt, &problem,
                ).await {
                    Ok(message) => Some(message),
                    Err(error) => { warn!(error = %error.body.error, "could not persist rejected chat send"); None }
                };
                let event = if request_id.is_none() {
                    WsServerEvent::Error {
                        history_message,
                        message: problem["message"]
                            .as_str()
                            .unwrap_or("Check provider setup.")
                            .to_string(),
                        details: None,
                        session_id: session_id.clone(),
                    }
                } else {
                    WsServerEvent::ChatSetupRequired {
                        history_message,
                        provider,
                        session_id: session_id.clone().unwrap_or_default(),
                        request_id,
                        code: problem["code"]
                            .as_str()
                            .unwrap_or("chat_setup_required")
                            .to_string(),
                        message: problem["message"]
                            .as_str()
                            .unwrap_or("Check provider setup.")
                            .to_string(),
                        action: problem["action"].as_str().unwrap_or("recheck").to_string(),
                        docs_url: problem["docsUrl"].as_str().map(str::to_string),
                    }
                };
                let _ = direct_tx.send(event).await;
                return;
            }
            let (model, effort, mode, thinking) = if runtime == ChatRuntime::NativeCli
                && model.as_deref().is_none_or(|model| model.trim().is_empty())
            {
                (None, None, None, None)
            } else {
                (model, effort, mode, thinking)
            };
            let fast = if inherit_native { None } else { fast };
            let direct_ai_config = (runtime == ChatRuntime::IoGateway)
                .then(|| direct_ai_runtime_config_for_user(state, &user.id, provider))
                .flatten();
            let requested_session_id = session_id.clone();
            if let Err(error) = state
                .start_agent_session_with_request(
                    provider,
                    project_path.clone(),
                    prompt.clone(),
                    session_id,
                    model,
                    effort,
                    mode,
                    thinking,
                    fast,
                    runtime,
                    direct_ai_config,
                    Some(user.id.clone()),
                    request_id.clone(),
                )
                .await
            {
                let mut recovery_sent = false;
                if let Some(session_id) = requested_session_id.as_deref() {
                    match state.context_recovery(session_id).await {
                        Ok(Some(recovery)) => {
                            recovery_sent = direct_tx
                                .send(WsServerEvent::ChatRecoveryRequired {
                                    provider,
                                    session_id: session_id.to_string(),
                                    response_id: None,
                                    recovery,
                                })
                                .await
                                .is_ok();
                        }
                        Ok(None) => {}
                        Err(recovery_error) => {
                            warn!(
                                session_id,
                                error = %recovery_error,
                                "failed to inspect chat context recovery after session start error"
                            );
                        }
                    }
                }
                if !recovery_sent {
                    let explanation = if matches!(error, iowb_core::CoreError::Conflict(_)) { error.to_string() } else { "Failed to start session.".into() };
                    let problem = serde_json::json!({ "code": "chat_start_failed", "message": explanation, "action": "retry" });
                    let history_message = persist_rejected_chat_send(
                        state, &user.id, provider, runtime, requested_session_id.as_deref().unwrap(),
                        request_id.as_deref(), &project_path, &prompt, &problem,
                    ).await.ok();
                    let _ = direct_tx
                        .send(WsServerEvent::Error {
                            history_message,
                            message: "failed to start session".to_string(),
                            details: Some(error.to_string()),
                            session_id: requested_session_id,
                        })
                        .await;
                }
            }
        }
        WsClientCommand::AbortSession {
            provider,
            session_id,
        } => match state.abort_agent_session(provider, &session_id).await {
            Ok(_) => {}
            Err(error) => {
                let _ = direct_tx
                    .send(WsServerEvent::Error {
                        history_message: None,
                        message: "failed to abort session".to_string(),
                        details: Some(error.to_string()),
                        session_id: Some(session_id),
                    })
                    .await;
            }
        },
        WsClientCommand::ProcessInput { process_id, data } => {
            if let Err(error) = state
                .processes
                .write_input(&process_id, data.into_bytes())
                .await
            {
                let _ = direct_tx
                    .send(WsServerEvent::Error {
                        history_message: None,
                        message: "failed to write process input".to_string(),
                        details: Some(error.to_string()),
                        session_id: None,
                    })
                    .await;
            }
        }
        WsClientCommand::ResizeTerminal {
            process_id,
            cols,
            rows,
        } => {
            if let Err(error) = state
                .processes
                .resize_terminal(&process_id, cols, rows)
                .await
            {
                let _ = direct_tx
                    .send(WsServerEvent::Error {
                        history_message: None,
                        message: "failed to resize process".to_string(),
                        details: Some(error.to_string()),
                        session_id: None,
                    })
                    .await;
            }
        }
    }
}

fn validated_board_session_subscriptions(
    state: &AppState,
    session_ids: Vec<String>,
) -> HashSet<String> {
    session_ids
        .into_iter()
        .map(|session_id| session_id.trim().to_string())
        .filter(|session_id| !session_id.is_empty())
        .filter(|session_id| state.sessions.is_board_session_cached(session_id))
        .collect()
}

fn validated_chat_session_subscriptions(
    session_ids: Option<Vec<String>>,
) -> Option<HashSet<String>> {
    session_ids.map(|session_ids| {
        session_ids
            .into_iter()
            .map(|session_id| session_id.trim().to_string())
            .filter(|session_id| !session_id.is_empty())
            .collect()
    })
}

fn ws_event_session_id(event: &WsServerEvent) -> Option<&str> {
    match event {
        WsServerEvent::Error {
            session_id: Some(session_id),
            ..
        }
        | WsServerEvent::ChatRecoveryRequired { session_id, .. }
        | WsServerEvent::ChatSetupRequired { session_id, .. }
        | WsServerEvent::SessionStatus { session_id, .. }
        | WsServerEvent::SessionMetadata { session_id, .. }
        | WsServerEvent::Output { session_id, .. } => Some(session_id),
        _ => None,
    }
}

fn ws_event_visible_to_connection(
    state: &AppState,
    event: &WsServerEvent,
    board_session_subscriptions: &HashSet<String>,
    chat_session_subscriptions: &Option<HashSet<String>>,
) -> bool {
    let Some(session_id) = ws_event_session_id(event) else {
        return true;
    };
    let board_session = state.sessions.is_board_session_cached(session_id);
    if board_session {
        return board_session_subscriptions.contains(session_id);
    }
    match event {
        WsServerEvent::Output { .. }
        | WsServerEvent::ChatRecoveryRequired { .. }
        | WsServerEvent::ChatSetupRequired { .. } => chat_session_subscriptions
            .as_ref()
            .map_or(true, |session_ids| session_ids.contains(session_id)),
        _ => true,
    }
}

fn resolve_session_chat_runtime(
    state: &AppState,
    user_id: &str,
    provider: Provider,
    session_id: Option<&str>,
    _model: Option<&str>,
) -> ChatRuntime {
    if let Some(session) = session_id
        .and_then(|session_id| state.storage.get_session_summary(session_id).ok().flatten())
    {
        return session.runtime.unwrap_or_else(|| {
            if provider == Provider::Gemini {
                ChatRuntime::NativeCli
            } else if session.model.as_deref().is_some_and(is_io_gateway_model) {
                ChatRuntime::IoGateway
            } else {
                // A new default cannot safely identify an old CLI thread's
                // origin. Only a saved gateway model is evidence for gateway
                // routing; otherwise keep legacy chats on Native CLI. The
                // first successful start persists this inferred route.
                ChatRuntime::NativeCli
            }
        });
    }
    if provider == Provider::Gemini {
        ChatRuntime::NativeCli
    } else {
        configured_chat_runtime(state, user_id)
    }
}

fn requested_chat_runtime(
    state: &AppState,
    user_id: &str,
    provider: Provider,
    session_id: Option<&str>,
    requested: Option<ChatRuntime>,
) -> ChatRuntime {
    // A client may choose a route only before a session exists. Legacy runtime
    // inference uses stored metadata, never a newly submitted model.
    let exists = session_id
        .and_then(|id| state.storage.get_session_summary(id).ok().flatten())
        .is_some_and(|session| !rejected_chat_draft(state, &session));
    if exists || requested.is_none() {
        resolve_session_chat_runtime(state, user_id, provider, session_id, None)
    } else {
        requested.unwrap_or(ChatRuntime::NativeCli)
    }
}

async fn send_ws_event(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    event: WsServerEvent,
) -> std::result::Result<(), axum::Error> {
    let payload = serde_json::to_string(&event).unwrap_or_else(|error| {
        serde_json::json!({
            "type": "error",
            "message": "failed to serialize websocket event",
            "details": error.to_string(),
        })
        .to_string()
    });
    sender.send(Message::Text(payload.into())).await
}
const TERMINAL_REPLAY_MAX_PROCESSES: usize = 4;

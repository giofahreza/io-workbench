    use super::*;

    #[test]
    fn parses_file_ranges_for_media_streaming() {
        assert_eq!(
            parse_file_range(Some("bytes=10-19"), 100).unwrap(),
            Some(FileByteRange { start: 10, end: 19 })
        );
        assert_eq!(
            parse_file_range(Some("bytes=90-"), 100).unwrap(),
            Some(FileByteRange { start: 90, end: 99 })
        );
        assert_eq!(
            parse_file_range(Some("bytes=-12"), 100).unwrap(),
            Some(FileByteRange { start: 88, end: 99 })
        );
        assert_eq!(parse_file_range(None, 100).unwrap(), None);
    }

    #[test]
    fn rejects_unsatisfiable_file_ranges() {
        assert!(parse_file_range(Some("bytes=100-120"), 100).is_err());
        assert!(parse_file_range(Some("bytes=20-10"), 100).is_err());
        assert!(parse_file_range(Some("items=0-10"), 100).is_err());
        assert!(parse_file_range(Some("bytes=-0"), 100).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn streams_project_file_with_byte_ranges() {
        let root =
            std::env::temp_dir().join(format!("iowb-server-media-stream-{}", uuid::Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project directory");
        std::fs::write(project.join("clip.mp3"), b"0123456789").expect("media file");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 4,
        })
        .await
        .expect("state initializes");
        state.projects.add_project(&project).expect("add project");

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("listener address");
        let server_state = state.clone();
        let server =
            tokio::spawn(async move { axum::serve(listener, build_router(server_state)).await });
        let response = reqwest::Client::new()
            .get(format!(
                "http://{address}/api/projects/project/files/raw?filePath=clip.mp3"
            ))
            .header(reqwest::header::RANGE, "bytes=2-5")
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .expect("stream response");

        assert_eq!(response.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok()),
            Some("bytes 2-5/10")
        );
        assert_eq!(
            response
                .headers()
                .get(reqwest::header::ACCEPT_RANGES)
                .and_then(|value| value.to_str().ok()),
            Some("bytes")
        );
        assert_eq!(
            response.bytes().await.expect("body bytes").as_ref(),
            b"2345"
        );

        server.abort();
        drop(state);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    async fn board_scope_test_state() -> (PathBuf, AppState) {
        let root = std::env::temp_dir().join(format!(
            "iowb-server-board-ws-scope-{}",
            uuid::Uuid::new_v4()
        ));
        let config_dir = root.join("config");
        std::fs::create_dir_all(&config_dir).expect("config directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");
        (root, state)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn websocket_board_events_require_exact_validated_subscription() {
        let (root, state) = board_scope_test_state().await;
        let now = Utc::now();
        for session in [
            SessionSummary {
                id: "board-chat".to_string(),
                provider: Provider::Codex,
                project_path: "/tmp/project".to_string(),
                title: "Board chat".to_string(),
                last_activity: now,
                board_session: true,
                board_id: Some("board-1".to_string()),
                board_task_id: Some("task-1".to_string()),
                ..Default::default()
            },
            SessionSummary {
                id: "ordinary-chat".to_string(),
                provider: Provider::Codex,
                project_path: "/tmp/project".to_string(),
                title: "Ordinary chat".to_string(),
                last_activity: now,
                ..Default::default()
            },
        ] {
            state
                .storage
                .upsert_session(&session)
                .expect("persist session");
            state
                .sessions
                .remember_persisted_session(session)
                .await
                .expect("cache session");
        }

        let board_output = WsServerEvent::Output {
            provider: Provider::Codex,
            session_id: "board-chat".to_string(),
            response_id: Some("response-board".to_string()),
            sequence: Some(1),
            content: "board output".to_string(),
            done: false,
        };
        let board_error = WsServerEvent::Error {
            message: "board error".to_string(),
            details: None,
            session_id: Some("board-chat".to_string()),
        };
        let ordinary_output = WsServerEvent::Output {
            provider: Provider::Codex,
            session_id: "ordinary-chat".to_string(),
            response_id: Some("response-ordinary".to_string()),
            sequence: Some(1),
            content: "ordinary output".to_string(),
            done: false,
        };
        let ordinary_status = WsServerEvent::SessionStatus {
            provider: Provider::Codex,
            session_id: "ordinary-chat".to_string(),
            status: iowb_protocol::SessionRuntimeStatus::Completed,
            response_id: Some("response-ordinary".to_string()),
            sequence: Some(2),
            latest_user_prompt: None,
        };

        let none = HashSet::new();
        let legacy_chat_subscriptions = None;
        assert!(!ws_event_visible_to_connection(
            &state,
            &board_output,
            &none,
            &legacy_chat_subscriptions,
        ));
        assert!(!ws_event_visible_to_connection(
            &state,
            &board_error,
            &none,
            &legacy_chat_subscriptions,
        ));
        assert!(ws_event_visible_to_connection(
            &state,
            &ordinary_output,
            &none,
            &legacy_chat_subscriptions,
        ));

        let accepted = validated_board_session_subscriptions(
            &state,
            vec![
                " board-chat ".to_string(),
                "ordinary-chat".to_string(),
                "missing-chat".to_string(),
            ],
        );
        assert_eq!(accepted, HashSet::from(["board-chat".to_string()]));
        assert!(ws_event_visible_to_connection(
            &state,
            &board_output,
            &accepted,
            &legacy_chat_subscriptions,
        ));
        assert!(ws_event_visible_to_connection(
            &state,
            &board_error,
            &accepted,
            &legacy_chat_subscriptions,
        ));

        let no_visible_chat = validated_chat_session_subscriptions(Some(Vec::new()));
        assert!(!ws_event_visible_to_connection(
            &state,
            &ordinary_output,
            &none,
            &no_visible_chat,
        ));
        assert!(ws_event_visible_to_connection(
            &state,
            &ordinary_status,
            &none,
            &no_visible_chat,
        ));
        let selected_chat = validated_chat_session_subscriptions(Some(vec![
            " ordinary-chat ".to_string(),
            "ordinary-chat".to_string(),
            "missing-chat".to_string(),
        ]));
        assert_eq!(
            selected_chat,
            Some(HashSet::from([
                "ordinary-chat".to_string(),
                "missing-chat".to_string(),
            ])),
        );
        assert!(ws_event_visible_to_connection(
            &state,
            &ordinary_output,
            &none,
            &selected_chat,
        ));

        drop(state);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocked_websocket_send_returns_context_recovery_descriptor() {
        let root = std::env::temp_dir().join(format!(
            "iowb-server-context-recovery-ws-{}",
            uuid::Uuid::new_v4()
        ));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");

        let now = Utc::now();
        let session_id = "session-needs-recovery";
        let failed_message_id = "message-failed";
        let session = SessionSummary {
            id: session_id.to_string(),
            provider: Provider::Codex,
            project_path: project.display().to_string(),
            title: "Recover this chat".to_string(),
            message_count: 1,
            last_activity: now,
            active: false,
            runtime: Some(ChatRuntime::IoGateway),
            ..Default::default()
        };
        state
            .storage
            .upsert_session(&session)
            .expect("persist session");
        state
            .sessions
            .remember_persisted_session(session)
            .await
            .expect("cache session");
        state
            .storage
            .append_message(
                session_id,
                &ChatMessage {
                    id: failed_message_id.to_string(),
                    role: MessageRole::User,
                    content: "continue".to_string(),
                    timestamp: now,
                    metadata: Value::Null,
                },
            )
            .expect("persist failed prompt");
        let mut failed_run = iowb_storage::StoredDurableChatRun::new(
            "run-failed",
            Some("user-1".to_string()),
            session_id,
            Provider::Codex.as_str(),
            "continue",
            project.display().to_string(),
        );
        failed_run.user_message_id = Some(failed_message_id.to_string());
        state
            .storage
            .create_durable_chat_run(&failed_run)
            .expect("persist failed run");
        state
            .storage
            .mark_durable_chat_run_failed(&failed_run.id, "invalid body")
            .expect("mark run failed");

        let (direct_tx, mut direct_rx) = mpsc::channel(4);
        let mut board_session_subscriptions = HashSet::new();
        let mut chat_session_subscriptions = None;
        handle_ws_command(
            &state,
            &direct_tx,
            &iowb_protocol::UserProfile {
                id: "user-1".to_string(),
                username: "test-user".to_string(),
                email: None,
                created_at: now,
            },
            WsClientCommand::StartSession {
                provider: Provider::Codex,
                project_path: project.display().to_string(),
                prompt: "another message".to_string(),
                session_id: Some(session_id.to_string()),
                model: Some("cod:gpt-5.6-sol".to_string()),
                effort: None,
                mode: None,
                thinking: None,
                fast: None,
            },
            &mut board_session_subscriptions,
            &mut chat_session_subscriptions,
        )
        .await;

        let event = direct_rx.recv().await.expect("recovery event");
        match event {
            WsServerEvent::ChatRecoveryRequired {
                provider,
                session_id: observed_session_id,
                response_id,
                recovery,
            } => {
                assert_eq!(provider, Provider::Codex);
                assert_eq!(observed_session_id, session_id);
                assert_eq!(response_id, None);
                assert_eq!(recovery.state, "required");
                assert_eq!(recovery.failed_message_id, failed_message_id);
            }
            other => panic!("expected chat recovery event, got {other:?}"),
        }
        assert!(
            direct_rx.try_recv().is_err(),
            "generic error must be suppressed"
        );

        drop(state);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn chat_image_upload_writes_project_file_and_returns_path_only() {
        let root = std::env::temp_dir().join(format!(
            "iowb-server-chat-image-upload-{}",
            uuid::Uuid::new_v4()
        ));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");
        state.projects.add_project(&project).expect("add project");
        let project_name = project
            .file_name()
            .and_then(|name| name.to_str())
            .expect("project name")
            .to_string();

        let boundary = "iowb-chat-image-boundary";
        let image_bytes = b"\x89PNG\r\n\x1a\nnot-real-but-path-safe";
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"images\"; filename=\"screen.png\"\r\nContent-Type: image/png\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(image_bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("listener address");
        let server_state = state.clone();
        let server =
            tokio::spawn(async move { axum::serve(listener, build_router(server_state)).await });
        let response = reqwest::Client::new()
            .post(format!(
                "http://{address}/api/projects/{project_name}/upload-images"
            ))
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await
            .expect("upload response");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let response_bytes = response.bytes().await.expect("response bytes");
        let payload: Value = serde_json::from_slice(&response_bytes).expect("response json");
        let images = payload
            .get("images")
            .and_then(Value::as_array)
            .expect("images array");
        assert_eq!(images.len(), 1);
        let image = &images[0];
        let returned_path = image
            .get("path")
            .and_then(Value::as_str)
            .expect("returned image path");
        assert!(returned_path.starts_with(".io-workbench/chat-images/chat-image_"));
        assert!(returned_path.ends_with(".png"));
        assert!(!returned_path.starts_with("data:"));
        assert!(image.get("data").is_none());
        assert!(image.get("base64").is_none());
        assert!(!String::from_utf8_lossy(&response_bytes).contains(";base64,"));
        assert_eq!(
            std::fs::read(project.join(returned_path)).expect("saved image"),
            image_bytes
        );

        server.abort();
        let _ = server.await;
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn sidebar_active_sessions_preserve_order_and_deduplicate_by_session_id() {
        let normalized = normalize_sidebar_active_sessions(serde_json::json!([
            {"sessionId": "session-b", "projectPath": "/work/b", "provider": "codex"},
            {"sessionId": "session-a", "projectPath": "/work/a", "provider": "claude"},
            {"sessionId": "session-b", "projectPath": "/other", "provider": "gemini"},
            {"projectPath": "/missing-id"},
            "session-c"
        ]));

        let sessions = normalized.as_array().expect("normalized pinned sessions");
        assert_eq!(sessions.len(), 3);
        assert_eq!(
            sessions
                .iter()
                .filter_map(sidebar_active_session_key)
                .collect::<Vec<_>>(),
            ["session-b", "session-a", "session-c"]
        );
        assert_eq!(
            sessions[0].get("projectPath").and_then(Value::as_str),
            Some("/work/b")
        );
    }

    #[test]
    fn board_chat_completion_does_not_create_ordinary_push_notification() {
        let ordinary = SessionSummary {
            id: "ordinary-chat".to_string(),
            ..Default::default()
        };
        let board = SessionSummary {
            id: "board-chat".to_string(),
            board_session: true,
            board_id: Some("board-1".to_string()),
            ..Default::default()
        };

        assert!(chat_completion_push_allowed(None));
        assert!(chat_completion_push_allowed(Some(&ordinary)));
        assert!(!chat_completion_push_allowed(Some(&board)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sidebar_active_sessions_treat_user_empty_as_authoritative() {
        let root = std::env::temp_dir().join(format!(
            "iowb-server-sidebar-active-sessions-{}",
            uuid::Uuid::new_v4()
        ));
        let config_dir = root.join("config");
        std::fs::create_dir_all(&config_dir).expect("config directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");
        state
            .storage
            .set_setting(
                SIDEBAR_ACTIVE_SESSIONS_KEY,
                &serde_json::json!([{"sessionId": "legacy-global"}]),
            )
            .expect("global setting");
        state
            .storage
            .set_setting(
                &user_setting_key("mobile-user", SIDEBAR_ACTIVE_SESSIONS_KEY),
                &serde_json::json!([]),
            )
            .expect("user setting");

        let (pinned_sessions, initialized) =
            load_sidebar_active_sessions(&state, "mobile-user").expect("pinned sessions");

        assert!(initialized);
        assert_eq!(pinned_sessions, serde_json::json!([]));

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sidebar_active_sessions_drop_board_chats() {
        let root = std::env::temp_dir().join(format!(
            "iowb-server-sidebar-board-sessions-{}",
            uuid::Uuid::new_v4()
        ));
        let config_dir = root.join("config");
        std::fs::create_dir_all(&config_dir).expect("config directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");
        let session = SessionSummary {
            id: "board-chat".to_string(),
            provider: Provider::Codex,
            project_path: "/tmp/project".to_string(),
            title: "Board chat".to_string(),
            last_activity: Utc::now(),
            board_session: true,
            board_id: Some("board-1".to_string()),
            ..Default::default()
        };
        state
            .storage
            .upsert_session(&session)
            .expect("board session");
        state
            .storage
            .set_setting(
                &user_setting_key("mobile-user", SIDEBAR_ACTIVE_SESSIONS_KEY),
                &serde_json::json!([
                    {"sessionId": "normal-chat"},
                    {"sessionId": "board-chat"}
                ]),
            )
            .expect("sidebar setting");

        let (pinned, initialized) =
            load_sidebar_active_sessions(&state, "mobile-user").expect("pinned sessions");
        assert!(initialized);
        assert_eq!(pinned, serde_json::json!([{"sessionId": "normal-chat"}]));

        let _ = std::fs::remove_dir_all(root);
    }

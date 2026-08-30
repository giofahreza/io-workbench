    use super::*;

    #[test]
    fn parses_thread_turn_ids_and_user_items() {
        let snapshot = parse_thread_snapshot(&json!({
            "thread": {
                "id": "thread-1",
                "turns": [
                    {
                        "id": "turn-1",
                        "status": "completed",
                        "items": [
                            {
                                "type": "userMessage",
                                "id": "item-1",
                                "content": [{"type": "text", "text": "first prompt"}]
                            }
                        ]
                    },
                    {
                        "id": "turn-2",
                        "status": "inProgress",
                        "items": []
                    }
                ]
            }
        }))
        .expect("snapshot");

        assert_eq!(snapshot.id, "thread-1");
        assert_eq!(snapshot.turns[0].user_item_ids, ["item-1"]);
        assert_eq!(snapshot.turns[0].user_text, "first prompt");
        assert_eq!(snapshot.latest_forkable_turn_id(), Some("turn-1"));
    }

    #[test]
    fn latest_forkable_turn_keeps_failed_or_interrupted_boundaries() {
        let snapshot = parse_thread_snapshot(&json!({
            "thread": {
                "id": "thread-1",
                "turns": [
                    {"id": "turn-failed", "status": "failed", "items": []},
                    {"id": "turn-interrupted", "status": "interrupted", "items": []},
                    {"id": "turn-running", "status": "inProgress", "items": []}
                ]
            }
        }))
        .expect("snapshot");

        assert_eq!(snapshot.latest_forkable_turn_id(), Some("turn-interrupted"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn performs_initialize_and_read_handshake() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("fake-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nread first\nprintf '%s\\n' \"$first\" >> '{}'\nprintf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\",\"codexHome\":\"/tmp\",\"platformFamily\":\"unix\",\"platformOs\":\"linux\"}}}}'\nread second\nprintf '%s\\n' \"$second\" >> '{}'\nread third\nprintf '%s\\n' \"$third\" >> '{}'\nprintf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-1\",\"turns\":[]}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let snapshot = client.read_thread("thread-1").await.expect("read thread");
        assert_eq!(snapshot.id, "thread-1");
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"method\":\"initialize\""));
        assert!(requests.contains("\"method\":\"initialized\""));
        assert!(requests.contains("\"method\":\"thread/read\""));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn waits_for_context_compaction_completion_notification() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("compact-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-1\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/started\",\"params\":{{\"threadId\":\"thread-1\",\"item\":{{\"type\":\"contextCompaction\",\"id\":\"item-compact\"}}}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/completed\",\"params\":{{\"threadId\":\"thread-1\",\"item\":{{\"type\":\"contextCompaction\",\"id\":\"item-compact\"}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        client
            .compact_thread_and_wait_with_options("thread-1", None)
            .await
            .expect("compact thread");
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"method\":\"initialize\""));
        assert!(requests.contains("\"method\":\"thread/resume\""));
        assert!(requests.contains("\"method\":\"thread/compact/start\""));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reports_app_server_errors_and_timeouts() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let error_script = root.join("error-codex.sh");
        std::fs::write(
            &error_script,
            "#!/bin/sh\nread first\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nprintf '%s\\n' 'diagnostic secret' >&2\nread second\nread third\nprintf '%s\\n' '{\"id\":2,\"error\":{\"code\":-32600,\"message\":\"bad boundary\"}}'\n",
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&error_script)
            .expect("metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&error_script, permissions).expect("permissions");
        let error_client =
            CodexAppServerClient::new(error_script.as_os_str(), Duration::from_secs(2));
        let error = error_client
            .fork_thread("thread-1", "turn-1")
            .await
            .expect_err("fork should fail");
        assert!(error.to_string().contains("bad boundary"));
        assert!(!error.to_string().contains("diagnostic secret"));

        let timeout_script = root.join("timeout-codex.sh");
        std::fs::write(&timeout_script, "#!/bin/sh\nsleep 2\n").expect("script");
        let mut permissions = std::fs::metadata(&timeout_script)
            .expect("metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&timeout_script, permissions).expect("permissions");
        let timeout_client =
            CodexAppServerClient::new(timeout_script.as_os_str(), Duration::from_millis(30));
        let error = timeout_client
            .read_thread("thread-1")
            .await
            .expect_err("read should time out");
        assert!(error.to_string().contains("timed out"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_live_turn_start_sequence() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("live-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-live\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-live\",\"status\":\"inProgress\",\"items\":[],\"error\":null}}}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/agentMessage/delta\",\"params\":{{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\",\"itemId\":\"msg-1\",\"delta\":\"hello\"}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"turn/completed\",\"params\":{{\"threadId\":\"thread-live\",\"turn\":{{\"id\":\"turn-live\",\"status\":\"completed\",\"items\":[{{\"type\":\"agentMessage\",\"id\":\"msg-1\",\"text\":\"hello\",\"phase\":\"final_answer\"}}],\"error\":null}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let (_abort_tx, abort_rx) = oneshot::channel();
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let outcome = client
            .run_live_turn_with_options(live_turn_params(&root, None), None, abort_rx, event_tx)
            .await
            .expect("live turn");

        assert_eq!(outcome.thread_id, "thread-live");
        assert_eq!(outcome.turn_id.as_deref(), Some("turn-live"));
        assert_eq!(outcome.status, CodexAppServerTurnTerminalStatus::Completed);
        let mut saw_delta = false;
        while let Some(event) = event_rx.recv().await {
            if let CodexAppServerLiveTurnEvent::Notification { method, params } = event
                && method == "item/agentMessage/delta"
                && params.get("delta").and_then(Value::as_str) == Some("hello")
            {
                saw_delta = true;
            }
        }
        assert!(saw_delta);
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"method\":\"initialize\""));
        assert!(requests.contains("\"method\":\"initialized\""));
        assert!(requests.contains("\"method\":\"thread/start\""));
        assert!(requests.contains("\"method\":\"turn/start\""));
        assert!(requests.contains("\"input\":[{\"text\":\"hello\",\"type\":\"text\"}]"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_live_turn_resume_sequence() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("resume-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-existing\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-resume\",\"status\":\"inProgress\",\"items\":[]}}}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"turn/completed\",\"params\":{{\"threadId\":\"thread-existing\",\"turn\":{{\"id\":\"turn-resume\",\"status\":\"completed\",\"items\":[{{\"type\":\"agentMessage\",\"id\":\"msg-1\",\"text\":\"resumed\",\"phase\":\"final_answer\"}}]}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let (_abort_tx, abort_rx) = oneshot::channel();
        let (event_tx, _event_rx) = mpsc::channel(16);
        let outcome = client
            .run_live_turn_with_options(
                live_turn_params(&root, Some("thread-existing")),
                None,
                abort_rx,
                event_tx,
            )
            .await
            .expect("live turn");

        assert_eq!(outcome.thread_id, "thread-existing");
        assert_eq!(outcome.turn_id.as_deref(), Some("turn-resume"));
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"method\":\"thread/resume\""));
        assert!(!requests.contains("\"method\":\"thread/start\""));
        assert!(requests.contains("\"threadId\":\"thread-existing\""));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_turn_filters_notifications_from_other_turns() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("demux-codex.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\n\
             read first\nprintf '%s\\n' '{\"id\":1,\"result\":{\"userAgent\":\"test\"}}'\n\
             read second\n\
             read third\nprintf '%s\\n' '{\"id\":2,\"result\":{\"thread\":{\"id\":\"thread-live\"}}}'\n\
             read fourth\nprintf '%s\\n' '{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn-live\",\"status\":\"inProgress\",\"items\":[]}}}'\n\
             printf '%s\\n' '{\"method\":\"item/agentMessage/delta\",\"params\":{\"threadId\":\"thread-live\",\"turnId\":\"turn-other\",\"itemId\":\"msg-other\",\"delta\":\"wrong\"}}'\n\
             printf '%s\\n' '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thread-live\",\"turn\":{\"id\":\"turn-other\",\"status\":\"completed\",\"items\":[{\"type\":\"agentMessage\",\"id\":\"msg-other\",\"text\":\"wrong\",\"phase\":\"final_answer\"}]}}}'\n\
             printf '%s\\n' '{\"method\":\"turn/started\",\"params\":{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\"}}'\n\
             printf '%s\\n' '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\",\"item\":{\"type\":\"userMessage\",\"id\":\"user-live\",\"text\":\"hello\"}}}'\n\
             printf '%s\\n' '{\"method\":\"thread/tokenUsage/updated\",\"params\":{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\",\"tokenUsage\":{\"outputTokens\":111}}}'\n\
             printf '%s\\n' '{\"method\":\"item/agentMessage/delta\",\"params\":{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\",\"itemId\":\"msg-live\",\"delta\":\"right\"}}'\n\
             printf '%s\\n' '{\"method\":\"thread/tokenUsage/updated\",\"params\":{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\",\"tokenUsage\":{\"outputTokens\":222}}}'\n\
             printf '%s\\n' '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thread-live\",\"turn\":{\"id\":\"turn-live\",\"status\":\"completed\",\"items\":[{\"type\":\"agentMessage\",\"id\":\"msg-live\",\"text\":\"right\",\"phase\":\"final_answer\"}]}}}'\n",
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let (_abort_tx, abort_rx) = oneshot::channel();
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let outcome = client
            .run_live_turn_with_options(live_turn_params(&root, None), None, abort_rx, event_tx)
            .await
            .expect("live turn");

        assert_eq!(outcome.turn_id.as_deref(), Some("turn-live"));
        assert_eq!(outcome.status, CodexAppServerTurnTerminalStatus::Completed);
        let mut deltas = Vec::new();
        let mut output_tokens = Vec::new();
        while let Some(event) = event_rx.recv().await {
            if let CodexAppServerLiveTurnEvent::Notification { method, params } = event {
                if method == "item/agentMessage/delta"
                    && let Some(delta) = params.get("delta").and_then(Value::as_str)
                {
                    deltas.push(delta.to_string());
                }
                if method == "thread/tokenUsage/updated"
                    && let Some(tokens) = params
                        .pointer("/tokenUsage/outputTokens")
                        .and_then(Value::as_i64)
                {
                    output_tokens.push(tokens);
                }
            }
        }
        assert_eq!(deltas, ["right"]);
        assert_eq!(output_tokens, [222]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_turn_declines_server_requests_without_hanging() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("approval-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\n\
                 read second\n\
                 read third\nprintf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-live\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-live\",\"status\":\"inProgress\",\"items\":[]}}}}}}'\n\
                 printf '%s\\n' '{{\"id\":99,\"method\":\"item/commandExecution/requestApproval\",\"params\":{{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\",\"itemId\":\"cmd-1\"}}}}'\n\
                 read approval\nprintf '%s\\n' \"$approval\" >> '{}'\n\
                 printf '%s\\n' '{{\"method\":\"turn/completed\",\"params\":{{\"threadId\":\"thread-live\",\"turn\":{{\"id\":\"turn-live\",\"status\":\"completed\",\"items\":[{{\"type\":\"agentMessage\",\"id\":\"msg-1\",\"text\":\"done\",\"phase\":\"final_answer\"}}]}}}}}}'\n",
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let (_abort_tx, abort_rx) = oneshot::channel();
        let (event_tx, _event_rx) = mpsc::channel(16);
        client
            .run_live_turn_with_options(live_turn_params(&root, None), None, abort_rx, event_tx)
            .await
            .expect("live turn");

        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"id\":99"));
        assert!(requests.contains("\"decision\":\"decline\""));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_turn_rejects_chatgpt_token_refresh_without_hanging() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("auth-refresh-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\n\
                 read second\n\
                 read third\nprintf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-live\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-live\",\"status\":\"inProgress\",\"items\":[]}}}}}}'\n\
                 printf '%s\\n' '{{\"id\":99,\"method\":\"account/chatgptAuthTokens/refresh\",\"params\":{{\"reason\":\"expired\",\"previousAccountId\":\"acct-1\"}}}}'\n\
                 read refresh\nprintf '%s\\n' \"$refresh\" >> '{}'\n\
                 printf '%s\\n' '{{\"method\":\"turn/completed\",\"params\":{{\"threadId\":\"thread-live\",\"turn\":{{\"id\":\"turn-live\",\"status\":\"completed\",\"items\":[{{\"type\":\"agentMessage\",\"id\":\"msg-1\",\"text\":\"done\",\"phase\":\"final_answer\"}}]}}}}}}'\n",
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let (_abort_tx, abort_rx) = oneshot::channel();
        let (event_tx, _event_rx) = mpsc::channel(16);
        client
            .run_live_turn_with_options(live_turn_params(&root, None), None, abort_rx, event_tx)
            .await
            .expect("live turn");

        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"id\":99"));
        assert!(requests.contains("\"code\":-32000"));
        assert!(requests.contains("does not provide external ChatGPT auth tokens"));
        assert!(!requests.contains("\"code\":-32601"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_turn_abort_sends_turn_interrupt() {
        use std::os::unix::fs::PermissionsExt;
        use tokio::time::sleep;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("abort-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\n\
                 read second\n\
                 read third\nprintf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-live\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-live\",\"status\":\"inProgress\",\"items\":[]}}}}}}'\n\
                 read interrupt\nprintf '%s\\n' \"$interrupt\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":4,\"result\":{{}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"turn/completed\",\"params\":{{\"threadId\":\"thread-live\",\"turn\":{{\"id\":\"turn-live\",\"status\":\"interrupted\",\"items\":[]}}}}}}'\n",
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let (abort_tx, abort_rx) = oneshot::channel();
        let (event_tx, _event_rx) = mpsc::channel(16);
        let task_root = root.clone();
        let task = tokio::spawn(async move {
            client
                .run_live_turn_with_options(
                    live_turn_params(&task_root, None),
                    None,
                    abort_rx,
                    event_tx,
                )
                .await
        });
        sleep(Duration::from_millis(50)).await;
        abort_tx.send(()).expect("send abort");
        let outcome = task.await.expect("join").expect("live turn");
        assert_eq!(
            outcome.status,
            CodexAppServerTurnTerminalStatus::Interrupted
        );
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"method\":\"turn/interrupt\""));
        assert!(requests.contains("\"turnId\":\"turn-live\""));
        let _ = std::fs::remove_dir_all(root);
    }

    fn live_turn_params(
        root: &std::path::Path,
        thread_id: Option<&str>,
    ) -> CodexAppServerLiveTurnParams {
        CodexAppServerLiveTurnParams {
            thread_id: thread_id.map(str::to_string),
            cwd: root.to_path_buf(),
            input: vec![json!({ "type": "text", "text": "hello" })],
            client_user_message_id: None,
            model: None,
            effort: None,
            service_tier: None,
            approval_policy: Some(json!("never")),
            sandbox_policy: None,
        }
    }

    #[test]
    fn bounded_stderr_keeps_recent_diagnostics_on_utf8_boundary() {
        let mut output = String::new();
        append_stderr_bounded(&mut output, &"a".repeat(APP_SERVER_STDERR_MAX_BYTES + 10));
        append_stderr_bounded(&mut output, "final");

        assert!(output.len() <= APP_SERVER_STDERR_MAX_BYTES);
        assert!(output.ends_with("final"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn startup_terminalizes_recovery_after_retry_limit() {
        let root =
            std::env::temp_dir().join(format!("iowb-server-recovery-{}", uuid::Uuid::new_v4()));
        let config_dir = root.join("config");
        let project = root.join("project");
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
        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        state
            .sessions
            .append_message(&session.id, MessageRole::User, "keep working")
            .await
            .expect("user message");
        let mut run = iowb_storage::StoredDurableChatRun::new(
            "retry-limit-run",
            Some("retry-user".to_string()),
            session.id.clone(),
            "codex",
            "keep working",
            project.display().to_string(),
        );
        run.resume_attempts = DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS;
        state
            .storage
            .create_durable_chat_run(&run)
            .expect("durable run");

        recover_interrupted_chat_runs(&state)
            .await
            .expect("startup reconciliation");

        let stored_run = state
            .storage
            .get_durable_chat_run(&run.id)
            .expect("read run")
            .expect("run exists");
        assert_eq!(stored_run.status, "interrupted");
        assert_eq!(
            stored_run.last_error.as_deref(),
            Some("automatic recovery attempt limit reached")
        );
        assert!(
            !state
                .storage
                .get_session(&session.id)
                .expect("read session")
                .expect("session exists")
                .active
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn startup_reconciles_stale_manual_context_compaction() {
        let root = std::env::temp_dir().join(format!(
            "iowb-server-manual-compact-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let config_dir = root.join("config");
        let project = root.join("project");
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
        state
            .storage
            .create_user("manual-user", "manual-user", "test-hash")
            .expect("create user");
        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                None,
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        state
            .sessions
            .set_native_session_id(&session.id, "native-manual-stale")
            .await
            .expect("native session");
        state
            .sessions
            .set_active(&session.id, true)
            .await
            .expect("active session");
        let now = Utc::now();
        let rollover = iowb_storage::StoredSessionContextRollover {
            id: "rollover-manual-startup-stale".to_string(),
            user_id: "manual-user".to_string(),
            session_id: session.id.clone(),
            request_id: "request-manual-startup-stale".to_string(),
            kind: "manual".to_string(),
            failed_message_id: String::new(),
            trigger_run_id: "run-manual-startup-stale".to_string(),
            retry_run_id: "run-manual-startup-stale".to_string(),
            from_native_session_id: Some("native-manual-stale".to_string()),
            candidate_native_session_id: Some("native-manual-stale".to_string()),
            state: "starting".to_string(),
            handoff: "Native Codex context compaction".to_string(),
            observed_bytes: Some(18 * 1024 * 1024),
            limit_bytes: 16 * 1024 * 1024,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut compact_run = iowb_storage::StoredDurableChatRun::new(
            "run-manual-startup-stale",
            Some("manual-user".to_string()),
            session.id.clone(),
            "codex",
            rollover.handoff.clone(),
            project.display().to_string(),
        );
        compact_run.native_session_id = Some("native-manual-stale".to_string());
        compact_run.auto_resume = false;
        state
            .storage
            .prepare_manual_context_rollover(&rollover, &compact_run)
            .expect("prepare manual rollover");
        state
            .storage
            .create_chat_run_attempt(&iowb_storage::StoredChatRunAttempt::new(
                "attempt-manual-startup-stale",
                compact_run.id.clone(),
                session.id.clone(),
                None,
                "codex",
                "codex_app_server",
                None,
                Some("native-manual-stale".to_string()),
            ))
            .expect("create compact attempt");
        state
            .storage
            .mark_durable_chat_run_interrupted(
                &compact_run.id,
                Some("automatic recovery is disabled"),
            )
            .expect("terminal compact run");

        recover_interrupted_chat_runs(&state)
            .await
            .expect("startup reconciliation");

        let stored_rollover = state
            .storage
            .context_rollover_for_retry_run(&compact_run.id)
            .expect("rollover lookup")
            .expect("rollover");
        assert_eq!(stored_rollover.state, "failed");
        assert!(
            !state
                .storage
                .get_session(&session.id)
                .expect("read session")
                .expect("session exists")
                .active
        );
        assert!(state.sessions.list_active().await.is_empty());

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn startup_automatically_recovers_legacy_active_chat() {
        let root = std::env::temp_dir().join(format!(
            "iowb-server-legacy-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let config_dir = root.join("config");
        let project = root.join("project");
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
        let session = state
            .sessions
            .create_or_update(
                Provider::Gemini,
                project.display().to_string(),
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        state
            .sessions
            .set_native_session_id(&session.id, "legacy-native-session")
            .await
            .expect("native session");
        state
            .sessions
            .append_message(&session.id, MessageRole::User, "finish legacy work")
            .await
            .expect("user message");
        assert!(
            state
                .storage
                .list_active_durable_chat_runs()
                .expect("list durable runs")
                .is_empty()
        );

        unsafe {
            std::env::set_var("IO_WORKBENCH_GEMINI_COMMAND", "/bin/sh");
            std::env::set_var(
                "IO_WORKBENCH_GEMINI_ARGS_JSON",
                r#"["-c","printf 'startup-resumed:%s\\n' \"$1\"","iowb-recovery","{native_session_id}"]"#,
            );
        }
        recover_interrupted_chat_runs(&state)
            .await
            .expect("startup recovery");
        unsafe {
            std::env::remove_var("IO_WORKBENCH_GEMINI_COMMAND");
            std::env::remove_var("IO_WORKBENCH_GEMINI_ARGS_JSON");
        }

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let runs = state
                    .storage
                    .list_active_durable_chat_runs()
                    .expect("active durable runs");
                if runs.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("automatic recovery completes");

        let messages = state.storage.list_messages(&session.id).expect("messages");
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .count(),
            1
        );
        assert!(messages.iter().any(|message| {
            message.role == MessageRole::Assistant
                && message
                    .content
                    .contains("startup-resumed:legacy-native-session")
        }));
        let durable_runs = state
            .storage
            .list_recoverable_durable_chat_runs(DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS, 10)
            .expect("recoverable runs");
        assert!(durable_runs.is_empty());
        assert!(
            !state
                .storage
                .get_session(&session.id)
                .expect("read session")
                .expect("session exists")
                .active
        );

        let _ = std::fs::remove_dir_all(root);
    }

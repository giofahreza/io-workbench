    #[test]
    fn named_credentials_are_scoped_and_updated_in_place() {
        let (storage, root) = temporary_storage("named-credential");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create first user");
        storage
            .create_user("user-2", "user-2", "test-hash")
            .expect("create second user");

        storage
            .upsert_named_credential(
                "user-1",
                "gateway-key",
                "io_gateway_api_key",
                "first-secret",
                None,
            )
            .expect("create credential");
        storage
            .upsert_named_credential(
                "user-1",
                "gateway-key",
                "io_gateway_api_key",
                "updated-secret",
                None,
            )
            .expect("update credential");

        assert_eq!(
            storage
                .get_active_credential_value_by_name("user-1", "gateway-key", "io_gateway_api_key",)
                .expect("read credential")
                .as_deref(),
            Some("updated-secret")
        );
        assert_eq!(
            storage
                .get_active_credential_value_by_name("user-2", "gateway-key", "io_gateway_api_key",)
                .expect("read other user"),
            None
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn durable_chat_run_round_trips_and_updates_native_session_id() {
        let (storage, root) = temporary_storage("durable-round-trip");
        let mut run = StoredDurableChatRun::new(
            "board-1",
            Some("user-1".to_string()),
            "ui-session-1",
            "codex",
            "finish the interrupted task",
            "/tmp/project",
        );
        run.native_session_id = Some("native-1".to_string());
        run.model = Some("gpt-5.4".to_string());
        run.effort = Some("high".to_string());
        run.mode = Some("agent".to_string());
        run.thinking = Some(true);
        run.fast = Some(true);

        storage
            .create_durable_chat_run(&run)
            .expect("create durable run");
        let restored = storage
            .get_durable_chat_run(&run.id)
            .expect("get durable run")
            .expect("stored durable run");
        assert_eq!(restored, run);

        assert!(
            storage
                .update_durable_chat_run_native_session_id(&run.id, Some("native-2"))
                .expect("update native id")
        );
        assert_eq!(
            storage
                .get_durable_chat_run(&run.id)
                .expect("get updated run")
                .expect("updated run")
                .native_session_id
                .as_deref(),
            Some("native-2")
        );
        assert!(
            storage
                .update_durable_chat_run_native_session_id(&run.id, None)
                .expect("clear native id")
        );
        assert_eq!(
            storage
                .get_durable_chat_run(&run.id)
                .expect("get cleared run")
                .expect("cleared run")
                .native_session_id,
            None
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn durable_chat_run_recovery_respects_status_flags_attempts_and_limit() {
        let (storage, root) = temporary_storage("durable-recovery");
        let base_time = Utc::now();

        let mut eligible = StoredDurableChatRun::new(
            "eligible",
            None,
            "session-1",
            "codex",
            "prompt 1",
            "/tmp/project",
        );
        eligible.created_at = base_time;
        eligible.updated_at = base_time;
        eligible.last_error = Some("server stopped".to_string());
        eligible.fast = Some(true);

        let mut recovering = StoredDurableChatRun::new(
            "recovering",
            None,
            "session-2",
            "claude",
            "prompt 2",
            "/tmp/project",
        );
        recovering.status = "recovering".to_string();
        recovering.resume_attempts = 1;
        recovering.created_at = base_time + chrono::Duration::seconds(1);
        recovering.updated_at = recovering.created_at;

        let mut disabled = StoredDurableChatRun::new(
            "disabled",
            None,
            "session-3",
            "gemini",
            "prompt 3",
            "/tmp/project",
        );
        disabled.auto_resume = false;
        disabled.created_at = base_time + chrono::Duration::seconds(2);
        disabled.updated_at = disabled.created_at;

        let mut exhausted = StoredDurableChatRun::new(
            "exhausted",
            None,
            "session-4",
            "codex",
            "prompt 4",
            "/tmp/project",
        );
        exhausted.resume_attempts = 2;
        exhausted.created_at = base_time + chrono::Duration::seconds(3);
        exhausted.updated_at = exhausted.created_at;

        let mut terminal = StoredDurableChatRun::new(
            "terminal",
            None,
            "session-5",
            "codex",
            "prompt 5",
            "/tmp/project",
        );
        terminal.status = "completed".to_string();
        terminal.created_at = base_time + chrono::Duration::seconds(4);
        terminal.updated_at = terminal.created_at;

        for run in [&eligible, &recovering, &disabled, &exhausted, &terminal] {
            storage
                .create_durable_chat_run(run)
                .expect("create durable run");
        }

        let recoverable = storage
            .list_recoverable_durable_chat_runs(2, 10)
            .expect("list recoverable");
        assert_eq!(
            recoverable
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            vec!["eligible", "recovering"]
        );
        assert_eq!(recoverable[0].fast, Some(true));
        assert_eq!(
            storage
                .list_recoverable_durable_chat_runs(2, 1)
                .expect("limited list")[0]
                .id,
            "eligible"
        );

        let active = storage
            .list_active_durable_chat_runs()
            .expect("list active");
        assert_eq!(
            active.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(),
            vec!["eligible", "recovering", "disabled", "exhausted"]
        );

        let claimed = storage
            .mark_durable_chat_run_recovering("eligible", 2)
            .expect("claim recovery")
            .expect("eligible claim");
        assert_eq!(claimed.status, "recovering");
        assert_eq!(claimed.resume_attempts, 1);
        assert_eq!(claimed.last_error, None);
        assert_eq!(claimed.fast, Some(true));
        assert!(claimed.recovered_at.is_some());

        let claimed_again = storage
            .mark_durable_chat_run_recovering("eligible", 2)
            .expect("claim second recovery")
            .expect("eligible second claim");
        assert_eq!(claimed_again.resume_attempts, 2);
        assert_eq!(claimed_again.fast, Some(true));
        assert!(
            storage
                .mark_durable_chat_run_recovering("eligible", 2)
                .expect("attempt exhausted claim")
                .is_none()
        );
        assert!(
            storage
                .mark_durable_chat_run_recovering("terminal", 2)
                .expect("terminal claim")
                .is_none()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn durable_chat_run_terminal_helpers_persist_outcomes() {
        let (storage, root) = temporary_storage("durable-terminal");
        let failed = StoredDurableChatRun::new(
            "failed",
            None,
            "session-1",
            "codex",
            "prompt",
            "/tmp/project",
        );
        let interrupted = StoredDurableChatRun::new(
            "interrupted",
            None,
            "session-2",
            "codex",
            "prompt",
            "/tmp/project",
        );
        storage
            .create_durable_chat_run(&failed)
            .expect("create failed run");
        storage
            .create_durable_chat_run(&interrupted)
            .expect("create interrupted run");

        assert!(
            storage
                .mark_durable_chat_run_failed("failed", "provider exited")
                .expect("mark failed")
        );
        let failed = storage
            .get_durable_chat_run("failed")
            .expect("get failed")
            .expect("failed run");
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.last_error.as_deref(), Some("provider exited"));
        assert!(failed.completed_at.is_some());

        assert!(
            storage
                .update_durable_chat_run_error("failed", "invalid body")
                .expect("persist structured provider error")
        );
        assert!(
            storage
                .mark_durable_chat_run_failed("failed", "provider run failed")
                .expect("repeat generic failure finalization")
        );
        let failed = storage
            .get_durable_chat_run("failed")
            .expect("get re-finalized failure")
            .expect("re-finalized failed run");
        assert_eq!(
            failed.last_error.as_deref(),
            Some("invalid body"),
            "generic terminalization must not erase the actionable provider error"
        );

        assert!(
            storage
                .mark_durable_chat_run_interrupted("interrupted", Some("retry limit reached"))
                .expect("mark interrupted")
        );
        let interrupted = storage
            .get_durable_chat_run("interrupted")
            .expect("get interrupted")
            .expect("interrupted run");
        assert_eq!(interrupted.status, "interrupted");
        assert_eq!(
            interrupted.last_error.as_deref(),
            Some("retry limit reached")
        );
        assert!(interrupted.completed_at.is_some());

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

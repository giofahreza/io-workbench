    #[test]
    fn context_rollover_completion_is_atomic_scoped_and_idempotent() {
        let (storage, root) = temporary_storage("context-rollover-completion");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let mut session = test_session("session-rollover-completion", false);
        session.title = "Stable visible chat".to_string();
        session.title_source = Some(SessionTitleSource::Manual);
        session.native_session_id = Some("native-poisoned".to_string());
        session.runtime = Some(ChatRuntime::IoGateway);
        session.model = Some("gpt-5.4".to_string());
        session.effort = Some("high".to_string());
        session.mode = Some("default".to_string());
        session.thinking = Some(true);
        session.fast = Some(true);
        session.message_count = 2;
        storage.upsert_session(&session).expect("upsert session");
        let prior_assistant = test_message(
            "message-prior-assistant",
            MessageRole::Assistant,
            "Earlier answer remains visible",
            0,
        );
        let failed_message = test_message(
            "message-failed",
            MessageRole::User,
            "Continue after compacting",
            1,
        );
        for message in [&prior_assistant, &failed_message] {
            storage
                .append_message(&session.id, message)
                .expect("append existing message");
        }
        storage
            .set_session_draft("user-1", &session.id, "draft stays untouched")
            .expect("save draft");

        let mut trigger_run = StoredDurableChatRun::new(
            "run-trigger-completion",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            failed_message.content.clone(),
            session.project_path.clone(),
        );
        trigger_run.user_message_id = Some(failed_message.id.clone());
        trigger_run.native_session_id = Some("native-poisoned".to_string());
        storage
            .create_durable_chat_run(&trigger_run)
            .expect("create trigger run");
        storage
            .mark_durable_chat_run_failed(&trigger_run.id, "invalid body")
            .expect("mark trigger failed");

        let rollover = test_context_rollover(
            "rollover-completion",
            &session.id,
            "request-completion",
            &trigger_run.id,
            "run-retry-completion",
            &failed_message.id,
            Utc::now(),
        );
        let mut retry_run = StoredDurableChatRun::new(
            "run-retry-completion",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            rollover.handoff.clone(),
            session.project_path.clone(),
        );
        retry_run.user_message_id = Some(failed_message.id.clone());
        assert!(
            storage
                .prepare_context_rollover(&rollover, &retry_run)
                .expect("prepare rollover")
        );

        assert!(
            !storage
                .set_context_rollover_candidate(
                    &rollover.id,
                    "run-from-another-response",
                    "native-clean",
                )
                .expect("reject mismatched retry run")
        );
        assert_eq!(
            storage
                .context_rollover_for_retry_run(&retry_run.id)
                .expect("rollover lookup")
                .expect("rollover")
                .candidate_native_session_id,
            None
        );
        assert_eq!(
            storage
                .get_durable_chat_run(&retry_run.id)
                .expect("retry lookup")
                .expect("retry")
                .native_session_id,
            None
        );
        assert!(
            storage
                .set_context_rollover_candidate(&rollover.id, &retry_run.id, "native-clean")
                .expect("stage clean candidate")
        );
        assert!(
            !storage
                .set_context_rollover_candidate(
                    &rollover.id,
                    &retry_run.id,
                    "native-late-conflict",
                )
                .expect("reject conflicting late candidate")
        );

        let activated_at = Utc::now() + chrono::Duration::seconds(2);
        let mut completed_session = session.clone();
        completed_session.native_session_id = Some("native-clean".to_string());
        completed_session.external = false;
        completed_session.active = true;
        completed_session.last_activity = activated_at;
        let marker = ChatMessage {
            id: "message-compaction-marker".to_string(),
            role: MessageRole::System,
            content: "Context compacted here".to_string(),
            timestamp: activated_at,
            metadata: serde_json::json!({
                "kind": "context_compaction",
                "rolloverId": rollover.id,
                "toNativeSessionId": "native-clean",
            }),
        };
        let assistant = test_message(
            "message-clean-assistant",
            MessageRole::Assistant,
            "Completed in the clean context",
            3,
        );

        assert!(
            !storage
                .complete_context_rollover(
                    &rollover.id,
                    "run-from-another-response",
                    "native-clean",
                    &completed_session,
                    &marker,
                    Some(&assistant),
                    None,
                )
                .expect("reject mismatched completion run")
        );
        assert!(
            !storage
                .complete_context_rollover(
                    &rollover.id,
                    &retry_run.id,
                    "native-late-conflict",
                    &completed_session,
                    &marker,
                    Some(&assistant),
                    None,
                )
                .expect("reject mismatched completion candidate")
        );
        let mut invalid_follow_up = StoredDurableChatRun::new(
            "run-follow-up-invalid",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            failed_message.content.clone(),
            session.project_path.clone(),
        );
        invalid_follow_up.user_message_id = Some(failed_message.id.clone());
        invalid_follow_up.native_session_id = Some("native-late-conflict".to_string());
        assert!(
            !storage
                .complete_context_rollover(
                    &rollover.id,
                    &retry_run.id,
                    "native-clean",
                    &completed_session,
                    &marker,
                    None,
                    Some(&invalid_follow_up),
                )
                .expect("reject mismatched follow-up run")
        );
        let duplicate_assistant = ChatMessage {
            id: failed_message.id.clone(),
            role: MessageRole::Assistant,
            content: "must roll back".to_string(),
            timestamp: activated_at + chrono::Duration::seconds(1),
            metadata: Value::Null,
        };
        assert!(
            storage
                .complete_context_rollover(
                    &rollover.id,
                    &retry_run.id,
                    "native-clean",
                    &completed_session,
                    &marker,
                    Some(&duplicate_assistant),
                    None,
                )
                .is_err(),
            "a persistence error must abort the entire activation"
        );

        let before_success = storage
            .get_session(&session.id)
            .expect("session lookup after rollback")
            .expect("session after rollback");
        assert_eq!(
            before_success.native_session_id.as_deref(),
            Some("native-poisoned")
        );
        assert_eq!(before_success.message_count, 2);
        assert!(
            storage
                .message_by_id(&session.id, &marker.id)
                .expect("marker lookup after rollback")
                .is_none()
        );
        assert_eq!(
            storage
                .context_rollover_for_retry_run(&retry_run.id)
                .expect("rollover after rollback")
                .expect("rollover")
                .state,
            "starting"
        );
        assert_eq!(
            storage
                .get_durable_chat_run(&retry_run.id)
                .expect("retry after rollback")
                .expect("retry")
                .status,
            "running"
        );

        let mut follow_up_run = StoredDurableChatRun::new(
            "run-follow-up-completion",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            failed_message.content.clone(),
            session.project_path.clone(),
        );
        follow_up_run.user_message_id = Some(failed_message.id.clone());
        follow_up_run.native_session_id = Some("native-clean".to_string());
        follow_up_run.model = completed_session.model.clone();
        follow_up_run.effort = completed_session.effort.clone();
        follow_up_run.mode = completed_session.mode.clone();
        follow_up_run.thinking = completed_session.thinking;
        follow_up_run.fast = completed_session.fast;
        assert!(
            storage
                .complete_context_rollover(
                    &rollover.id,
                    &retry_run.id,
                    "native-clean",
                    &completed_session,
                    &marker,
                    None,
                    Some(&follow_up_run),
                )
                .expect("complete rollover")
        );
        assert!(
            storage
                .has_active_context_rollover(&session.id)
                .expect("completed rollover is active")
        );
        let completed = storage
            .get_session(&session.id)
            .expect("completed session lookup")
            .expect("completed session");
        assert_eq!(completed.id, session.id);
        assert_eq!(completed.title, "Stable visible chat");
        assert_eq!(completed.title_source, Some(SessionTitleSource::Manual));
        assert_eq!(completed.native_session_id.as_deref(), Some("native-clean"));
        assert_eq!(completed.runtime, Some(ChatRuntime::IoGateway));
        assert!(!completed.active);
        assert_eq!(completed.message_count, 3);
        assert_eq!(
            storage
                .list_messages(&session.id)
                .expect("completed transcript")
                .into_iter()
                .map(|message| message.id)
                .collect::<HashSet<_>>(),
            HashSet::from([
                prior_assistant.id.clone(),
                failed_message.id.clone(),
                marker.id.clone(),
            ])
        );
        assert_eq!(
            storage
                .get_session_draft("user-1", &session.id)
                .expect("draft after activation")
                .content,
            "draft stays untouched"
        );
        let completed_rollover = storage
            .context_rollover_for_retry_run(&retry_run.id)
            .expect("completed rollover lookup")
            .expect("completed rollover");
        assert_eq!(completed_rollover.state, "active");
        assert_eq!(
            completed_rollover.candidate_native_session_id.as_deref(),
            Some("native-clean")
        );
        assert!(completed_rollover.activated_at.is_some());
        let completed_run = storage
            .get_durable_chat_run(&retry_run.id)
            .expect("completed run lookup")
            .expect("completed run");
        assert_eq!(completed_run.status, "completed");
        assert!(!completed_run.auto_resume);
        assert!(completed_run.completed_at.is_some());
        let completed_follow_up = storage
            .get_durable_chat_run(&follow_up_run.id)
            .expect("follow-up run lookup")
            .expect("follow-up run");
        assert_eq!(completed_follow_up.status, "running");
        assert_eq!(
            completed_follow_up.user_message_id.as_deref(),
            Some(failed_message.id.as_str())
        );
        assert_eq!(
            completed_follow_up.native_session_id.as_deref(),
            Some("native-clean")
        );
        assert_eq!(completed_follow_up.prompt, failed_message.content);

        assert!(
            !storage
                .complete_context_rollover(
                    &rollover.id,
                    &retry_run.id,
                    "native-clean",
                    &completed_session,
                    &marker,
                    None,
                    Some(&follow_up_run),
                )
                .expect("repeat completion is a no-op")
        );
        assert!(
            !storage
                .set_context_rollover_candidate(
                    &rollover.id,
                    &retry_run.id,
                    "native-after-completion",
                )
                .expect("late thread event is ignored")
        );
        assert_eq!(
            storage
                .list_messages(&session.id)
                .expect("final transcript")
                .len(),
            3
        );
        assert_eq!(
            storage
                .get_session(&session.id)
                .expect("final session lookup")
                .expect("final session")
                .native_session_id
                .as_deref(),
            Some("native-clean")
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn native_session_id_round_trips_through_session_metadata() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "iowb-storage-native-thread-{}-{unique}",
            std::process::id()
        ));
        let database = root.join("test.db");
        let storage = Storage::open(&database).expect("storage");
        let session = SessionSummary {
            id: "new-session-test".to_string(),
            provider: Provider::Codex,
            project_path: "/tmp/project".to_string(),
            title: "Test".to_string(),
            last_activity: Utc::now(),
            native_session_id: Some("22222222-2222-4222-8222-222222222222".to_string()),
            native_rollout_owned_by_provider: true,
            title_source: Some(SessionTitleSource::Manual),
            runtime: Some(ChatRuntime::IoGateway),
            fast: Some(true),
            token_usage: Some(SessionTokenUsage {
                used: 4_321,
                input: 1_500,
                output: 2_700,
                cache_creation: 0,
                cache_read: 121,
                reasoning: 0,
                cost_usd: 0.0,
            }),
            ..Default::default()
        };

        storage.upsert_session(&session).expect("upsert");
        let restored = storage
            .get_session(&session.id)
            .expect("query")
            .expect("stored session");
        assert_eq!(restored.native_session_id, session.native_session_id);
        assert!(restored.native_rollout_owned_by_provider);
        assert_eq!(restored.title_source, session.title_source);
        assert_eq!(restored.runtime, Some(ChatRuntime::IoGateway));
        assert_eq!(restored.fast, Some(true));
        let usage = restored.token_usage.as_ref().expect("token usage");
        assert_eq!(usage.used, 4_321);
        assert_eq!(usage.input, 1_500);
        assert_eq!(usage.output, 2_700);
        assert_eq!(usage.cache_read, 121);
        let api_value = serde_json::to_value(&restored).expect("serialize session");
        assert_eq!(api_value["token_usage"]["used"], 4_321);
        assert_eq!(api_value["fast"], true);
        assert!(api_value.get("nativeRolloutOwnedByProvider").is_none());

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }
    #[test]
    fn board_scope_round_trips_but_is_hidden_from_discovery_and_search() {
        let (storage, root) = temporary_storage("board-session-scope");
        let mut ordinary = test_session("ordinary-session", true);
        ordinary.title = "ordinary searchable conversation".to_string();
        let mut board = test_session("board-session", true);
        board.title = "board searchable conversation".to_string();
        board.board_session = true;
        board.board_id = Some("board-1".to_string());
        board.board_task_id = Some("task-1".to_string());
        for session in [&ordinary, &board] {
            storage.upsert_session(session).expect("upsert session");
            storage
                .append_message(
                    &session.id,
                    &test_message(
                        &format!("{}-message", session.id),
                        MessageRole::User,
                        &format!("{} searchable", session.id),
                        0,
                    ),
                )
                .expect("append message");
        }

        let restored = storage
            .get_session(&board.id)
            .expect("board query")
            .expect("board session");
        assert!(restored.board_session);
        assert_eq!(restored.board_id.as_deref(), Some("board-1"));
        assert_eq!(restored.board_task_id.as_deref(), Some("task-1"));
        assert_eq!(
            storage
                .list_sessions()
                .expect("visible sessions")
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            vec![ordinary.id.clone()]
        );
        assert!(
            storage
                .list_sessions_including_board()
                .expect("raw sessions")
                .iter()
                .any(|session| session.id == board.id)
        );
        assert_eq!(
            storage
                .list_sessions_for_project("/tmp/project")
                .expect("project sessions")
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            vec![ordinary.id.clone()]
        );
        assert_eq!(
            storage
                .search_messages("searchable", 10)
                .expect("conversation search")
                .into_iter()
                .map(|(session, _)| session.id)
                .collect::<Vec<_>>(),
            vec![ordinary.id]
        );

        // A newer board hit must not consume the result limit and hide an
        // older ordinary conversation from user-facing search.
        let limited = storage
            .search_messages("searchable", 1)
            .expect("limited conversation search");
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].0.id, "ordinary-session");

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn legacy_board_run_scope_is_hidden_from_discovery_and_search() {
        let (storage, root) = temporary_storage("legacy-board-run-scope");
        let mut legacy = test_session("legacy-board-session", true);
        legacy.title = "legacy board searchable conversation".to_string();
        storage.upsert_session(&legacy).expect("upsert session");
        storage
            .append_message(
                &legacy.id,
                &test_message(
                    "legacy-board-message",
                    MessageRole::User,
                    "legacy board searchable",
                    0,
                ),
            )
            .expect("append message");

        storage
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE sessions SET metadata = ?1 WHERE id = ?2",
                    params![r#"{"boardRunId":"board-legacy"}"#, legacy.id],
                )?;
                Ok(())
            })
            .expect("write legacy metadata");

        let restored = storage
            .get_session(&legacy.id)
            .expect("legacy session query")
            .expect("legacy session");
        assert!(restored.is_board_session());
        assert_eq!(restored.board_id.as_deref(), Some("board-legacy"));
        assert!(
            storage
                .list_sessions()
                .expect("visible sessions")
                .is_empty()
        );
        assert!(
            storage
                .list_sessions_for_project("/tmp/project")
                .expect("project sessions")
                .is_empty()
        );
        assert!(
            storage
                .search_messages("searchable", 10)
                .expect("session search")
                .is_empty()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn legacy_prompt_titles_backfill_to_latest_and_preserve_manual_titles() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "iowb-storage-title-backfill-{}-{unique}",
            std::process::id()
        ));
        let database = root.join("test.db");
        let now = Utc::now();

        {
            let storage = Storage::open(&database).expect("storage");
            for (id, title) in [
                ("legacy-auto", "first prompt"),
                ("legacy-manual", "Pinned release investigation"),
            ] {
                storage
                    .upsert_session(&SessionSummary {
                        id: id.to_string(),
                        provider: Provider::Codex,
                        project_path: "/tmp/project".to_string(),
                        title: title.to_string(),
                        last_activity: now,
                        ..Default::default()
                    })
                    .expect("upsert legacy session");
                for (index, content) in ["first prompt", "  latest\n\nprompt  "]
                    .into_iter()
                    .enumerate()
                {
                    storage
                        .append_message(
                            id,
                            &ChatMessage {
                                id: format!("{id}-message-{index}"),
                                role: MessageRole::User,
                                content: content.to_string(),
                                timestamp: now + chrono::Duration::seconds(index as i64),
                                metadata: Value::Null,
                            },
                        )
                        .expect("append legacy prompt");
                }
            }
        }

        let storage = Storage::open(&database).expect("reopen storage");
        let automatic = storage
            .get_session("legacy-auto")
            .expect("automatic query")
            .expect("automatic session");
        assert_eq!(automatic.title, "latest prompt");
        assert_eq!(automatic.title_source, Some(SessionTitleSource::Prompt));

        let manual = storage
            .get_session("legacy-manual")
            .expect("manual query")
            .expect("manual session");
        assert_eq!(manual.title, "Pinned release investigation");
        assert_eq!(manual.title_source, Some(SessionTitleSource::Manual));

        drop(storage);
        let reopened = Storage::open(&database).expect("second reopen");
        assert_eq!(
            reopened
                .get_session("legacy-auto")
                .expect("idempotent query")
                .expect("idempotent session")
                .title,
            "latest prompt"
        );

        drop(reopened);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn lists_only_internal_native_session_ids() {
        let (storage, root) = temporary_storage("internal-native-session-ids");
        for session in [
            SessionSummary {
                id: "internal-session".to_string(),
                provider: Provider::Codex,
                project_path: "/tmp/project".to_string(),
                title: "Internal".to_string(),
                last_activity: Utc::now(),
                native_session_id: Some("native-internal".to_string()),
                ..Default::default()
            },
            SessionSummary {
                id: "external-session".to_string(),
                provider: Provider::Codex,
                external: true,
                project_path: "/tmp/project".to_string(),
                title: "External".to_string(),
                last_activity: Utc::now(),
                native_session_id: Some("native-external".to_string()),
                ..Default::default()
            },
            SessionSummary {
                id: "without-native-session".to_string(),
                provider: Provider::Codex,
                project_path: "/tmp/project".to_string(),
                title: "No native mapping".to_string(),
                last_activity: Utc::now(),
                ..Default::default()
            },
        ] {
            storage.upsert_session(&session).expect("upsert session");
        }

        assert_eq!(
            storage
                .list_internal_native_session_ids()
                .expect("native session ids"),
            vec!["native-internal".to_string()]
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn user_prompt_history_pages_only_user_messages_with_cursor() {
        let (storage, root) = temporary_storage("prompt-history");
        let session = SessionSummary {
            id: "session-prompts".to_string(),
            provider: Provider::Codex,
            project_path: "/tmp/project".to_string(),
            title: "Prompt history".to_string(),
            last_activity: Utc::now(),
            ..Default::default()
        };
        storage.upsert_session(&session).expect("upsert session");
        for (index, role, content) in [
            (0, MessageRole::User, "first"),
            (1, MessageRole::Assistant, "ignored assistant"),
            (2, MessageRole::User, "second"),
            (3, MessageRole::Tool, "ignored tool"),
            (4, MessageRole::User, "second"),
            (5, MessageRole::User, "third"),
        ] {
            storage
                .append_message(
                    &session.id,
                    &ChatMessage {
                        id: format!("m{index}"),
                        role,
                        content: content.to_string(),
                        timestamp: Utc::now() + chrono::Duration::seconds(index),
                        metadata: Value::Null,
                    },
                )
                .expect("append message");
        }

        let (latest, has_more) = storage
            .list_user_prompts_page(&session.id, 2, None)
            .expect("latest prompts");
        assert_eq!(
            latest
                .iter()
                .map(|prompt| prompt.content.as_str())
                .collect::<Vec<_>>(),
            vec!["second", "third"]
        );
        assert!(has_more);

        let cursor = PromptHistoryCursor {
            timestamp: latest.first().expect("oldest latest prompt").timestamp,
            id: latest.first().expect("oldest latest prompt").id.clone(),
        };
        let (older, has_more) = storage
            .list_user_prompts_page(&session.id, 2, Some(&cursor))
            .expect("older prompts");
        assert_eq!(
            older
                .iter()
                .map(|prompt| prompt.content.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert!(!has_more);

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn compacted_codex_context_usage_uses_cumulative_delta_after_latest_compact() {
        let (storage, root) = temporary_storage("codex-context-usage-delta");
        let session = test_session("session-context-usage", false);
        storage.upsert_session(&session).expect("session");
        let compacted_at = Utc::now();
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-before-compact",
            "run-before-compact",
            compacted_at - chrono::Duration::seconds(20),
            SessionTokenUsage {
                used: 10_000,
                input: 9_000,
                output: 1_000,
                cache_creation: 0,
                cache_read: 7_000,
                reasoning: 100,
                cost_usd: 0.20,
            },
            "codex.turn.completed.usage",
        );
        let mut compact_run = StoredDurableChatRun::new(
            "run-compact",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            "compact",
            session.project_path.clone(),
        );
        compact_run.status = "completed".to_string();
        compact_run.completed_at = Some(compacted_at);
        storage
            .create_durable_chat_run(&compact_run)
            .expect("compact run");
        let mut rollover = test_context_rollover(
            "rollover-context-usage",
            &session.id,
            "request-context-usage",
            "run-compact",
            "run-compact",
            "",
            compacted_at,
        );
        rollover.kind = "manual".to_string();
        rollover.state = "active".to_string();
        rollover.candidate_native_session_id = Some("native-1".to_string());
        rollover.activated_at = Some(compacted_at);
        storage
            .with_connection(|conn| {
                conn.execute(
                    r#"
                    INSERT INTO session_context_rollovers (
                        id, user_id, session_id, request_id, kind, failed_message_id,
                        trigger_run_id, retry_run_id, from_native_session_id,
                        candidate_native_session_id, state, handoff, observed_bytes,
                        limit_bytes, error, created_at, updated_at, activated_at
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16, ?17, ?18
                    )
                    "#,
                    params![
                        rollover.id,
                        rollover.user_id,
                        rollover.session_id,
                        rollover.request_id,
                        rollover.kind,
                        rollover.failed_message_id,
                        rollover.trigger_run_id,
                        rollover.retry_run_id,
                        rollover.from_native_session_id,
                        rollover.candidate_native_session_id,
                        rollover.state,
                        rollover.handoff,
                        rollover.observed_bytes.map(|value| value as i64),
                        rollover.limit_bytes as i64,
                        rollover.error,
                        rollover.created_at.to_rfc3339(),
                        rollover.updated_at.to_rfc3339(),
                        rollover.activated_at.map(|time| time.to_rfc3339()),
                    ],
                )?;
                Ok(())
            })
            .expect("rollover");
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-after-compact-1",
            "run-after-compact-1",
            compacted_at + chrono::Duration::seconds(10),
            SessionTokenUsage {
                used: 10_600,
                input: 9_500,
                output: 1_100,
                cache_creation: 0,
                cache_read: 7_200,
                reasoning: 120,
                cost_usd: 0.24,
            },
            "codex.turn.completed.usage",
        );
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-after-compact-2",
            "run-after-compact-2",
            compacted_at + chrono::Duration::seconds(20),
            SessionTokenUsage {
                used: 11_200,
                input: 10_100,
                output: 1_100,
                cache_creation: 0,
                cache_read: 7_800,
                reasoning: 150,
                cost_usd: 0.30,
            },
            "codex.turn.completed.usage",
        );

        let scoped = storage
            .session_context_token_usage(&session.id)
            .expect("context usage");
        assert!(scoped.after_compact);
        assert_eq!(scoped.compacted_at, Some(compacted_at));
        assert_eq!(scoped.total, 1_200);
        assert_eq!(scoped.input, 1_100);
        assert_eq!(scoped.output, 100);
        assert_eq!(scoped.cache_read, 800);
        assert_eq!(scoped.reasoning, 50);
        assert_eq!(scoped.completeness, TokenUsageCompleteness::Complete);
        assert_eq!(scoped.partial_attempts, 0);
        assert_eq!(scoped.missing_attempts, 0);
        let listed = storage
            .list_sessions()
            .expect("session list")
            .into_iter()
            .find(|listed| listed.id == session.id)
            .expect("listed session")
            .context_token_usage
            .expect("listed context usage");
        assert_eq!(listed.total, 1_200);
        assert!(listed.after_compact);

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }
    #[test]
    fn codex_spent_token_usage_uses_cumulative_deltas_for_whole_and_compacted_scope() {
        let (storage, root) = temporary_storage("codex-spent-usage-delta");
        let session = test_session("session-spent-usage", false);
        storage.upsert_session(&session).expect("session");
        let compacted_at = Utc::now();
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-spent-before-1",
            "run-spent-before-1",
            compacted_at - chrono::Duration::seconds(30),
            SessionTokenUsage {
                used: 10_000,
                input: 9_000,
                output: 1_000,
                cache_creation: 0,
                cache_read: 7_000,
                reasoning: 100,
                cost_usd: 0.20,
            },
            "codex.turn.completed.usage",
        );
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-spent-before-2",
            "run-spent-before-2",
            compacted_at - chrono::Duration::seconds(20),
            SessionTokenUsage {
                used: 10_500,
                input: 9_400,
                output: 1_100,
                cache_creation: 0,
                cache_read: 7_200,
                reasoning: 120,
                cost_usd: 0.24,
            },
            "codex.turn.completed.usage",
        );
        let mut compact_run = StoredDurableChatRun::new(
            "run-spent-compact",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            "compact",
            session.project_path.clone(),
        );
        compact_run.status = "completed".to_string();
        compact_run.completed_at = Some(compacted_at);
        storage
            .create_durable_chat_run(&compact_run)
            .expect("compact run");
        let mut rollover = test_context_rollover(
            "rollover-spent-usage",
            &session.id,
            "request-spent-usage",
            "run-spent-compact",
            "run-spent-compact",
            "",
            compacted_at,
        );
        rollover.kind = "manual".to_string();
        rollover.state = "active".to_string();
        rollover.candidate_native_session_id = Some("native-2".to_string());
        rollover.activated_at = Some(compacted_at);
        storage
            .with_connection(|conn| {
                conn.execute(
                    r#"
                    INSERT INTO session_context_rollovers (
                        id, user_id, session_id, request_id, kind, failed_message_id,
                        trigger_run_id, retry_run_id, from_native_session_id,
                        candidate_native_session_id, state, handoff, observed_bytes,
                        limit_bytes, error, created_at, updated_at, activated_at
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16, ?17, ?18
                    )
                    "#,
                    params![
                        rollover.id,
                        rollover.user_id,
                        rollover.session_id,
                        rollover.request_id,
                        rollover.kind,
                        rollover.failed_message_id,
                        rollover.trigger_run_id,
                        rollover.retry_run_id,
                        rollover.from_native_session_id,
                        rollover.candidate_native_session_id,
                        rollover.state,
                        rollover.handoff,
                        rollover.observed_bytes.map(|value| value as i64),
                        rollover.limit_bytes as i64,
                        rollover.error,
                        rollover.created_at.to_rfc3339(),
                        rollover.updated_at.to_rfc3339(),
                        rollover.activated_at.map(|time| time.to_rfc3339()),
                    ],
                )?;
                Ok(())
            })
            .expect("rollover");
        insert_completed_usage_attempt_with_native(
            &storage,
            &session,
            "attempt-spent-after-1",
            "run-spent-after-1",
            compacted_at + chrono::Duration::seconds(10),
            SessionTokenUsage {
                used: 600,
                input: 500,
                output: 100,
                cache_creation: 0,
                cache_read: 200,
                reasoning: 20,
                cost_usd: 0.04,
            },
            "codex.turn.completed.usage",
            Some("native-2"),
        );
        insert_completed_usage_attempt_with_native(
            &storage,
            &session,
            "attempt-spent-after-2",
            "run-spent-after-2",
            compacted_at + chrono::Duration::seconds(20),
            SessionTokenUsage {
                used: 1_200,
                input: 1_000,
                output: 200,
                cache_creation: 0,
                cache_read: 500,
                reasoning: 50,
                cost_usd: 0.10,
            },
            "codex.turn.completed.usage",
            Some("native-2"),
        );

        let spent = storage
            .session_spent_token_usage(&session.id)
            .expect("spent usage");
        assert_eq!(spent.compacted_at, Some(compacted_at));
        assert_eq!(spent.whole_session.total, 11_700);
        assert_eq!(spent.whole_session.input, 10_400);
        assert_eq!(spent.whole_session.output, 1_300);
        let since_compact = spent.since_compact.expect("since compact");
        assert_eq!(since_compact.total, 1_200);
        assert_eq!(since_compact.input, 1_000);
        assert_eq!(since_compact.output, 200);
        assert_eq!(since_compact.cache_read, 500);
        assert_eq!(since_compact.reasoning, 50);
        assert_eq!(since_compact.completeness, TokenUsageCompleteness::Complete);

        let listed = storage
            .list_sessions()
            .expect("session list")
            .into_iter()
            .find(|listed| listed.id == session.id)
            .expect("listed session")
            .spent_token_usage
            .expect("listed spent usage");
        assert_eq!(listed.whole_session.total, 11_700);
        assert_eq!(
            listed.since_compact.expect("listed since compact").total,
            1_200
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }
    #[test]
    fn uncompact_codex_context_usage_uses_latest_cumulative_total() {
        let (storage, root) = temporary_storage("codex-context-usage-whole");
        let session = test_session("session-context-whole", false);
        storage.upsert_session(&session).expect("session");
        let now = Utc::now();
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-cumulative-1",
            "run-cumulative-1",
            now - chrono::Duration::seconds(10),
            SessionTokenUsage {
                used: 10_000,
                input: 9_000,
                output: 1_000,
                cache_creation: 0,
                cache_read: 7_000,
                reasoning: 100,
                cost_usd: 0.20,
            },
            "codex.turn.completed.usage",
        );
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-cumulative-2",
            "run-cumulative-2",
            now,
            SessionTokenUsage {
                used: 11_200,
                input: 10_100,
                output: 1_100,
                cache_creation: 0,
                cache_read: 7_800,
                reasoning: 150,
                cost_usd: 0.30,
            },
            "codex.turn.completed.usage",
        );

        let lifetime = storage
            .session_lifetime_token_usage(&session.id)
            .expect("lifetime");
        assert_eq!(lifetime.total, 21_200);
        let scoped = storage
            .session_context_token_usage(&session.id)
            .expect("context usage");
        assert!(!scoped.after_compact);
        assert_eq!(scoped.total, 11_200);
        assert_eq!(scoped.input, 10_100);
        assert_eq!(scoped.cache_read, 7_800);

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn session_fork_usage_baseline_inherits_only_cloned_prefix() {
        let (storage, root) = temporary_storage("session-fork-usage-baseline");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let source = test_session("session-source-usage", false);
        storage.upsert_session(&source).expect("source session");
        let source_messages = [
            test_message("source-u1", MessageRole::User, "first prompt", 0),
            test_message("source-a1", MessageRole::Assistant, "first answer", 1),
            test_message("source-u2", MessageRole::User, "second prompt", 2),
        ];
        for message in &source_messages {
            storage
                .append_message(&source.id, message)
                .expect("source message");
        }
        for (run_id, message_id, total) in [
            ("run-source-1", "source-u1", 100_u64),
            ("run-source-2", "source-u2", 900_u64),
        ] {
            let mut run = StoredDurableChatRun::new(
                run_id,
                Some("user-1".to_string()),
                source.id.clone(),
                "codex",
                "prompt",
                source.project_path.clone(),
            );
            run.user_message_id = Some(message_id.to_string());
            storage.create_durable_chat_run(&run).expect("run");
            let attempt = StoredChatRunAttempt::new(
                format!("attempt-{run_id}"),
                run.id.clone(),
                source.id.clone(),
                run.user_message_id.clone(),
                "codex",
                "native_cli",
                None,
                None,
            );
            storage.create_chat_run_attempt(&attempt).expect("attempt");
            storage
                .finish_chat_run_attempt(
                    &attempt.id,
                    "completed",
                    Some(&SessionTokenUsage {
                        used: total,
                        input: total - 10,
                        output: 10,
                        cache_creation: 0,
                        cache_read: 0,
                        reasoning: 0,
                        cost_usd: 0.0,
                    }),
                    None,
                    Some("test"),
                    TokenUsageCompleteness::Complete,
                )
                .expect("finish");
        }

        let mut destination = test_session("session-destination-usage", false);
        destination.message_count = 2;
        let cloned = [
            ChatMessage {
                id: "cloned-u1".to_string(),
                metadata: serde_json::json!({
                    "forkedFromSessionId": source.id,
                    "forkedFromMessageId": "source-u1",
                    "usageSourceSessionId": source.id,
                    "usageSourceMessageId": "source-u1",
                }),
                ..source_messages[0].clone()
            },
            ChatMessage {
                id: "cloned-a1".to_string(),
                metadata: serde_json::json!({
                    "forkedFromSessionId": source.id,
                    "forkedFromMessageId": "source-a1",
                }),
                ..source_messages[1].clone()
            },
        ];
        assert_eq!(
            storage
                .create_session_fork(
                    "user-1",
                    &source.id,
                    "source-u2",
                    "request-usage",
                    &destination,
                    &cloned,
                    "second prompt",
                    true,
                    false,
                )
                .expect("fork"),
            CreateSessionForkOutcome::Created
        );
        let restored = storage
            .get_session(&destination.id)
            .expect("session")
            .expect("destination");
        let usage = restored.lifetime_token_usage.expect("usage");
        assert_eq!(usage.total, 100);
        assert_eq!(usage.input, 90);
        assert_eq!(usage.output, 10);

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn session_fork_transaction_preserves_prefix_draft_and_idempotency() {
        let (storage, root) = temporary_storage("session-fork");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let source = test_session("session-source", false);
        storage.upsert_session(&source).expect("source session");
        let source_messages = [
            test_message("source-1", MessageRole::User, "first prompt", 0),
            test_message("source-2", MessageRole::Assistant, "first answer", 1),
            test_message("source-3", MessageRole::User, "second prompt", 2),
            test_message("source-4", MessageRole::Assistant, "second answer", 3),
        ];
        for message in &source_messages {
            storage
                .append_message(&source.id, message)
                .expect("source message");
        }

        let mut destination = test_session("session-destination", false);
        destination.title = "Second prompt".to_string();
        destination.message_count = 2;
        let cloned = [
            ChatMessage {
                id: "cloned-1".to_string(),
                metadata: serde_json::json!({
                    "forkedFromSessionId": source.id,
                    "forkedFromMessageId": source_messages[0].id,
                }),
                ..source_messages[0].clone()
            },
            ChatMessage {
                id: "cloned-2".to_string(),
                metadata: serde_json::json!({
                    "forkedFromSessionId": source.id,
                    "forkedFromMessageId": source_messages[1].id,
                }),
                ..source_messages[1].clone()
            },
        ];
        assert_eq!(
            storage
                .create_session_fork(
                    "user-1",
                    &source.id,
                    "source-3",
                    "request-1",
                    &destination,
                    &cloned,
                    "second prompt",
                    true,
                    true,
                )
                .expect("create fork"),
            CreateSessionForkOutcome::Created
        );

        let restored_source = storage.list_messages(&source.id).expect("source messages");
        assert_eq!(restored_source.len(), source_messages.len());
        assert_eq!(
            restored_source
                .iter()
                .map(|message| (message.id.as_str(), message.role, message.content.as_str()))
                .collect::<Vec<_>>(),
            source_messages
                .iter()
                .map(|message| (message.id.as_str(), message.role, message.content.as_str()))
                .collect::<Vec<_>>()
        );
        let destination_messages = storage
            .list_messages(&destination.id)
            .expect("destination messages");
        assert_eq!(destination_messages.len(), 2);
        assert_eq!(destination_messages[0].id, "cloned-1");
        assert_eq!(
            destination_messages[0].metadata["forkedFromMessageId"],
            "source-1"
        );
        assert_eq!(
            storage
                .get_session_draft("user-1", &destination.id)
                .expect("destination draft")
                .content,
            "second prompt"
        );
        assert_eq!(
            storage
                .get_session_fork("user-1", &source.id, "request-1")
                .expect("fork lookup")
                .expect("stored fork"),
            StoredSessionFork {
                before_message_id: "source-3".to_string(),
                destination_session_id: destination.id.clone(),
                replaces_source: true,
            }
        );
        assert_eq!(
            storage
                .list_sessions()
                .expect("sessions while replacement exists")
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            vec![destination.id.clone()]
        );
        assert_eq!(
            storage
                .list_replaced_source_session_ids()
                .expect("replaced source ids"),
            vec![source.id.clone()]
        );

        let other_destination = test_session("session-other", false);
        assert_eq!(
            storage
                .create_session_fork(
                    "user-1",
                    &source.id,
                    "source-1",
                    "request-1",
                    &other_destination,
                    &[],
                    "different prompt",
                    true,
                    false,
                )
                .expect("idempotent retry"),
            CreateSessionForkOutcome::Existing(StoredSessionFork {
                before_message_id: "source-3".to_string(),
                destination_session_id: destination.id.clone(),
                replaces_source: true,
            })
        );
        assert!(
            storage
                .get_session(&other_destination.id)
                .expect("other destination lookup")
                .is_none()
        );
        assert!(
            storage
                .delete_session(&destination.id)
                .expect("delete replacement")
        );
        assert_eq!(
            storage
                .list_sessions()
                .expect("sessions after deleting replacement")
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            vec![source.id.clone()]
        );
        assert!(
            storage
                .list_replaced_source_session_ids()
                .expect("restored source ids")
                .is_empty()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn session_fork_rejects_active_source_without_partial_writes() {
        let (storage, root) = temporary_storage("session-fork-active");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let source = test_session("session-active", true);
        storage.upsert_session(&source).expect("source session");
        let destination = test_session("session-blocked", false);

        assert_eq!(
            storage
                .create_session_fork(
                    "user-1",
                    &source.id,
                    "source-message",
                    "request-active",
                    &destination,
                    &[],
                    "prompt",
                    true,
                    true,
                )
                .expect("active outcome"),
            CreateSessionForkOutcome::SourceActive
        );
        assert!(
            storage
                .get_session(&destination.id)
                .expect("destination lookup")
                .is_none()
        );
        assert!(
            storage
                .get_session_fork("user-1", &source.id, "request-active")
                .expect("fork lookup")
                .is_none()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn non_replacing_session_fork_keeps_source_visible() {
        let (storage, root) = temporary_storage("session-fork-visible-source");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let source = test_session("session-source", false);
        let destination = test_session("session-destination", false);
        storage.upsert_session(&source).expect("source session");

        assert_eq!(
            storage
                .create_session_fork(
                    "user-1",
                    &source.id,
                    "source-message",
                    "request-visible",
                    &destination,
                    &[],
                    "prompt",
                    true,
                    false,
                )
                .expect("create fork"),
            CreateSessionForkOutcome::Created
        );

        let listed = storage
            .list_sessions()
            .expect("sessions")
            .into_iter()
            .map(|session| session.id)
            .collect::<HashSet<_>>();
        assert_eq!(
            listed,
            HashSet::from([source.id.clone(), destination.id.clone()])
        );
        assert!(
            storage
                .list_replaced_source_session_ids()
                .expect("replaced source ids")
                .is_empty()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn legacy_session_schema_migrates_forks_and_context_rollovers_to_v5() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "iowb-storage-fork-migration-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("storage dir");
        let database = root.join("test.db");
        {
            let connection = Connection::open(&database).expect("legacy database");
            connection
                .execute_batch(
                    r#"
                    CREATE TABLE session_forks (
                        user_id TEXT NOT NULL,
                        source_session_id TEXT NOT NULL,
                        before_message_id TEXT NOT NULL,
                        request_id TEXT NOT NULL,
                        destination_session_id TEXT NOT NULL,
                        created_at TEXT NOT NULL,
                        PRIMARY KEY(user_id, source_session_id, request_id)
                    );
                    "#,
                )
                .expect("legacy schema");
        }

        let storage = Storage::open(&database).expect("migrated storage");
        storage
            .with_connection(|connection| {
                let column_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('session_forks') WHERE name = 'replaces_source'",
                    [],
                    |row| row.get(0),
                )?;
                let index_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_session_forks_replaced_source'",
                    [],
                    |row| row.get(0),
                )?;
                let rollover_table_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'session_context_rollovers'",
                    [],
                    |row| row.get(0),
                )?;
                let rollover_column_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('session_context_rollovers')",
                    [],
                    |row| row.get(0),
                )?;
                let rollover_session_index_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM pragma_index_list('session_context_rollovers') WHERE name = 'idx_session_context_rollovers_session' AND \"unique\" = 0",
                    [],
                    |row| row.get(0),
                )?;
                let rollover_retry_index_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM pragma_index_list('session_context_rollovers') WHERE name = 'idx_session_context_rollovers_retry_run' AND \"unique\" = 1",
                    [],
                    |row| row.get(0),
                )?;
                let rollover_request_unique_count: i64 = connection.query_row(
                    r#"
                    SELECT COUNT(*)
                    FROM pragma_index_list('session_context_rollovers') indexes
                    WHERE indexes."unique" = 1
                      AND (
                          SELECT group_concat(name, ',')
                          FROM (
                              SELECT name
                              FROM pragma_index_info(indexes.name)
                              ORDER BY seqno
                          )
                      ) = 'user_id,session_id,request_id'
                    "#,
                    [],
                    |row| row.get(0),
                )?;
                let schema_version: String = connection.query_row(
                    "SELECT value FROM meta WHERE key = 'schema_version'",
                    [],
                    |row| row.get(0),
                )?;
                let attempts_table_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'chat_run_attempts'",
                    [],
                    |row| row.get(0),
                )?;
                let baselines_table_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'session_usage_baselines'",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(column_count, 1);
                assert_eq!(index_count, 1);
                assert_eq!(rollover_table_count, 1);
                assert_eq!(rollover_column_count, 18);
                assert_eq!(rollover_session_index_count, 1);
                assert_eq!(rollover_retry_index_count, 1);
                assert_eq!(rollover_request_unique_count, 1);
                assert_eq!(attempts_table_count, 1);
                assert_eq!(baselines_table_count, 1);
                assert_eq!(schema_version, "7");
                Ok(())
            })
            .expect("migration checks");

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn context_rollover_prepare_is_idempotent_and_preserves_chat_across_restart() {
        let (storage, root) = temporary_storage("context-rollover-restart");
        let database = root.join("test.db");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let mut session = test_session("session-rollover", false);
        session.title = "Keep this visible chat".to_string();
        session.title_source = Some(SessionTitleSource::Manual);
        session.native_session_id = Some("native-poisoned".to_string());
        session.model = Some("gpt-5.4".to_string());
        session.runtime = Some(ChatRuntime::IoGateway);
        session.effort = Some("high".to_string());
        session.mode = Some("default".to_string());
        session.thinking = Some(true);
        session.fast = Some(true);
        session.message_count = 4;
        storage.upsert_session(&session).expect("upsert session");
        let messages = vec![
            test_message("message-1", MessageRole::User, "Earlier question", 0),
            test_message("message-2", MessageRole::Assistant, "Earlier answer", 1),
            test_message("message-3", MessageRole::Tool, "Large tool result", 2),
            test_message("message-failed", MessageRole::User, "Please continue", 3),
        ];
        for message in &messages {
            storage
                .append_message(&session.id, message)
                .expect("append visible message");
        }
        storage
            .set_session_draft("user-1", &session.id, "unsent follow-up")
            .expect("save draft");

        let mut failed_run = StoredDurableChatRun::new(
            "run-failed",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            "Please continue",
            session.project_path.clone(),
        );
        failed_run.user_message_id = Some("message-failed".to_string());
        failed_run.native_session_id = Some("native-poisoned".to_string());
        storage
            .create_durable_chat_run(&failed_run)
            .expect("create failed run");
        storage
            .mark_durable_chat_run_failed(&failed_run.id, "invalid body")
            .expect("mark original run failed");

        let created_at = Utc::now();
        let rollover = test_context_rollover(
            "rollover-1",
            &session.id,
            "request-1",
            &failed_run.id,
            "run-retry-1",
            "message-failed",
            created_at,
        );
        let mut retry_run = StoredDurableChatRun::new(
            "run-retry-1",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            rollover.handoff.clone(),
            session.project_path.clone(),
        );
        retry_run.user_message_id = Some("message-failed".to_string());
        retry_run.model = session.model.clone();
        retry_run.effort = session.effort.clone();
        retry_run.mode = session.mode.clone();
        retry_run.thinking = session.thinking;
        retry_run.fast = session.fast;

        assert!(
            storage
                .prepare_context_rollover(&rollover, &retry_run)
                .expect("prepare rollover")
        );
        assert!(
            storage
                .has_context_rollover(&session.id)
                .expect("rollover bookkeeping exists")
        );
        assert!(
            !storage
                .has_active_context_rollover(&session.id)
                .expect("prepared rollover is not active")
        );
        assert!(
            !storage
                .prepare_context_rollover(&rollover, &retry_run)
                .expect("repeat identical request")
        );
        assert_eq!(
            storage
                .context_rollover_for_request("user-1", &session.id, "request-1")
                .expect("request lookup")
                .expect("stored rollover")
                .retry_run_id,
            "run-retry-1"
        );
        assert_eq!(
            storage
                .get_durable_chat_run(&failed_run.id)
                .expect("trigger run lookup")
                .expect("trigger run")
                .status,
            "superseded"
        );
        let stored_retry = storage
            .get_durable_chat_run(&retry_run.id)
            .expect("retry run lookup")
            .expect("retry run");
        assert_eq!(stored_retry.native_session_id, None);
        assert_eq!(
            stored_retry.user_message_id.as_deref(),
            Some("message-failed")
        );

        let stored_session = storage
            .get_session(&session.id)
            .expect("session lookup")
            .expect("stored session");
        assert_eq!(stored_session.id, session.id);
        assert_eq!(stored_session.title, "Keep this visible chat");
        assert_eq!(
            stored_session.native_session_id.as_deref(),
            Some("native-poisoned")
        );
        assert_eq!(stored_session.runtime, Some(ChatRuntime::IoGateway));
        assert_eq!(
            storage
                .list_messages(&session.id)
                .expect("messages")
                .into_iter()
                .map(|message| (message.id, message.role, message.content))
                .collect::<Vec<_>>(),
            messages
                .iter()
                .map(|message| { (message.id.clone(), message.role, message.content.clone(),) })
                .collect::<Vec<_>>()
        );
        assert_eq!(
            storage
                .get_session_draft("user-1", &session.id)
                .expect("draft")
                .content,
            "unsent follow-up"
        );

        drop(storage);
        let reopened = Storage::open(&database).expect("reopen storage");
        assert!(
            reopened
                .has_context_rollover(&session.id)
                .expect("rollover presence")
        );
        assert!(
            !reopened
                .has_active_context_rollover(&session.id)
                .expect("prepared rollover is still inactive after restart")
        );
        assert_eq!(
            reopened
                .context_rollover_for_retry_run(&retry_run.id)
                .expect("retry linkage")
                .expect("rollover after restart")
                .id,
            rollover.id
        );
        let recoverable_retry = reopened
            .list_recoverable_durable_chat_runs(3, 10)
            .expect("recoverable retry")
            .into_iter()
            .find(|run| run.id == retry_run.id)
            .expect("retry remains recoverable");
        assert_eq!(recoverable_retry.native_session_id, None);
        assert_eq!(
            reopened
                .list_messages(&session.id)
                .expect("messages")
                .into_iter()
                .map(|message| (message.id, message.role, message.content))
                .collect::<Vec<_>>(),
            messages
                .iter()
                .map(|message| { (message.id.clone(), message.role, message.content.clone(),) })
                .collect::<Vec<_>>()
        );
        assert_eq!(
            reopened
                .get_session_draft("user-1", &session.id)
                .expect("draft")
                .content,
            "unsent follow-up"
        );

        drop(reopened);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn failed_context_rollover_allows_fresh_request_without_duplicate_prompt() {
        let (storage, root) = temporary_storage("context-rollover-failed-retry");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let mut session = test_session("session-rollover-retry", false);
        session.native_session_id = Some("native-poisoned".to_string());
        session.message_count = 1;
        storage.upsert_session(&session).expect("upsert session");
        let failed_message = test_message(
            "message-failed",
            MessageRole::User,
            "Retry this exact prompt",
            0,
        );
        storage
            .append_message(&session.id, &failed_message)
            .expect("append failed prompt");

        let mut trigger_run = StoredDurableChatRun::new(
            "run-trigger",
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

        let first = test_context_rollover(
            "rollover-first",
            &session.id,
            "request-first",
            &trigger_run.id,
            "run-retry-first",
            &failed_message.id,
            Utc::now(),
        );
        let mut first_retry = StoredDurableChatRun::new(
            "run-retry-first",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            first.handoff.clone(),
            session.project_path.clone(),
        );
        first_retry.user_message_id = Some(failed_message.id.clone());
        assert!(
            storage
                .prepare_context_rollover(&first, &first_retry)
                .expect("prepare first rollover")
        );
        assert!(
            storage
                .fail_context_rollover(&first.id, "clean context launch failed")
                .expect("fail first rollover")
        );
        assert!(
            !storage
                .has_active_context_rollover(&session.id)
                .expect("failed rollover is not active")
        );
        assert!(
            storage
                .mark_durable_chat_run_failed(&first_retry.id, "clean context launch failed")
                .expect("fail first retry run")
        );
        assert!(
            !storage
                .prepare_context_rollover(&first, &first_retry)
                .expect("same request remains idempotent")
        );

        let second = test_context_rollover(
            "rollover-second",
            &session.id,
            "request-second",
            &first_retry.id,
            "run-retry-second",
            &failed_message.id,
            Utc::now() + chrono::Duration::milliseconds(1),
        );
        let mut second_retry = StoredDurableChatRun::new(
            "run-retry-second",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            second.handoff.clone(),
            session.project_path.clone(),
        );
        second_retry.user_message_id = Some(failed_message.id.clone());
        assert!(
            storage
                .prepare_context_rollover(&second, &second_retry)
                .expect("prepare fresh request")
        );

        assert_eq!(
            storage
                .context_rollover_for_request("user-1", &session.id, "request-first")
                .expect("first request lookup")
                .expect("first rollover")
                .state,
            "failed"
        );
        assert_eq!(
            storage
                .latest_context_rollover(&session.id)
                .expect("latest rollover lookup")
                .expect("latest rollover")
                .request_id,
            "request-second"
        );
        assert_eq!(
            storage
                .get_durable_chat_run(&first_retry.id)
                .expect("first retry lookup")
                .expect("first retry")
                .status,
            "superseded"
        );
        assert_eq!(
            storage
                .get_session(&session.id)
                .expect("session lookup")
                .expect("session")
                .native_session_id
                .as_deref(),
            Some("native-poisoned")
        );
        let visible_messages = storage
            .list_messages(&session.id)
            .expect("visible messages");
        assert_eq!(visible_messages.len(), 1);
        assert_eq!(visible_messages[0].id, failed_message.id);
        assert_eq!(visible_messages[0].role, failed_message.role);
        assert_eq!(visible_messages[0].content, failed_message.content);

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn stale_manual_context_rollover_reconciliation_clears_processing_state() {
        let (storage, root) = temporary_storage("manual-context-rollover-reconcile");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let mut session = test_session("session-manual-reconcile", true);
        session.native_session_id = Some("native-existing".to_string());
        storage.upsert_session(&session).expect("upsert session");

        let mut rollover = test_context_rollover(
            "rollover-manual-stale",
            &session.id,
            "request-manual-stale",
            "run-manual-stale",
            "run-manual-stale",
            "",
            Utc::now(),
        );
        rollover.kind = "manual".to_string();
        rollover.from_native_session_id = Some("native-existing".to_string());
        rollover.candidate_native_session_id = Some("native-existing".to_string());
        let mut compact_run = StoredDurableChatRun::new(
            "run-manual-stale",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            rollover.handoff.clone(),
            session.project_path.clone(),
        );
        compact_run.native_session_id = Some("native-existing".to_string());
        compact_run.auto_resume = false;
        assert!(
            storage
                .prepare_manual_context_rollover(&rollover, &compact_run)
                .expect("prepare manual rollover")
        );
        let attempt = StoredChatRunAttempt::new(
            "attempt-manual-stale",
            compact_run.id.clone(),
            session.id.clone(),
            None,
            "codex",
            "codex_app_server",
            None,
            Some("native-existing".to_string()),
        );
        assert!(
            storage
                .create_chat_run_attempt(&attempt)
                .expect("create compact attempt")
        );
        assert!(
            storage
                .mark_durable_chat_run_interrupted(
                    &compact_run.id,
                    Some("automatic recovery is disabled"),
                )
                .expect("interrupt compact run")
        );

        let inactive = storage
            .reconcile_stale_manual_context_rollovers()
            .expect("reconcile stale manual rollover");
        assert_eq!(inactive, vec![session.id.clone()]);
        let reconciled_rollover = storage
            .context_rollover_for_retry_run(&compact_run.id)
            .expect("rollover lookup")
            .expect("rollover");
        assert_eq!(reconciled_rollover.state, "failed");
        assert_eq!(
            reconciled_rollover.error.as_deref(),
            Some(
                "manual context compaction ended before activation: automatic recovery is disabled"
            )
        );
        let stored_session = storage
            .get_session(&session.id)
            .expect("session lookup")
            .expect("session");
        assert!(!stored_session.active);
        let (attempt_status, attempt_completed_at): (String, Option<String>) = storage
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT status, completed_at FROM chat_run_attempts WHERE id = ?1",
                    params![attempt.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(StorageError::from)
            })
            .expect("attempt lookup");
        assert_eq!(attempt_status, "failed");
        assert!(attempt_completed_at.is_some());
        assert!(
            storage
                .reconcile_stale_manual_context_rollovers()
                .expect("repeat reconciliation")
                .is_empty()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

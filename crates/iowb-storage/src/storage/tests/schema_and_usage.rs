    #[test]
    fn external_history_index_and_messages_survive_reopen() {
        let (storage, root) = temporary_storage("external-history-index");
        let database = root.join("test.db");
        let mut summary = test_session("external-session", false);
        summary.external = true;
        summary.message_count = 3;
        let source = StoredExternalHistorySource {
            provider: Provider::Codex,
            source_path: "/tmp/state.sqlite".to_string(),
            file_identity: Some("1:2".to_string()),
            file_size: 42,
            modified_nanos: Some(99),
            scan_offset: 42,
            parser_version: 1,
            records: vec![StoredExternalSessionRecord {
                summary: summary.clone(),
                file_path: "/tmp/rollout.jsonl".to_string(),
            }],
        };
        storage
            .upsert_external_history_source(&source)
            .expect("persist source");

        let messages = vec![
            test_message("external-0", MessageRole::User, "question", 0),
            test_message("external-1", MessageRole::Assistant, "answer", 1),
            test_message("external-2", MessageRole::Tool, "tool", 2),
        ];
        let fingerprint = ExternalHistoryFingerprint {
            file_identity: Some("3:4"),
            file_size: 123,
            modified_nanos: Some(456),
            parser_version: 1,
        };
        storage
            .replace_external_messages(
                Provider::Codex,
                &summary.id,
                "/tmp/rollout.jsonl",
                &fingerprint,
                &messages,
            )
            .expect("persist messages");
        drop(storage);

        let reopened = Storage::open(database).expect("reopen storage");
        let restored = reopened
            .external_history_source(Provider::Codex, "/tmp/state.sqlite")
            .expect("load source")
            .expect("source");
        assert_eq!(restored.records.len(), 1);
        assert_eq!(restored.records[0].summary.id, summary.id);
        assert_eq!(restored.records[0].summary.message_count, 3);
        let tail = reopened
            .external_messages_tail_if_current(
                Provider::Codex,
                &summary.id,
                "/tmp/rollout.jsonl",
                &fingerprint,
                2,
            )
            .expect("load tail")
            .expect("current tail");
        assert_eq!(tail.1, 3);
        assert_eq!(
            tail.0
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["external-1", "external-2"]
        );
        let stale_fingerprint = ExternalHistoryFingerprint {
            file_size: 124,
            ..fingerprint
        };
        assert!(
            reopened
                .external_messages_if_current(
                    Provider::Codex,
                    &summary.id,
                    "/tmp/rollout.jsonl",
                    &stale_fingerprint,
                )
                .expect("stale lookup")
                .is_none()
        );

        drop(reopened);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn legacy_durable_chat_runs_schema_migrates_missing_turn_columns() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "iowb-storage-legacy-durable-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let database = root.join("test.db");

        {
            let conn = Connection::open(&database).expect("legacy connection");
            conn.execute_batch(
                r#"
                CREATE TABLE durable_chat_runs (
                    id TEXT PRIMARY KEY,
                    user_id TEXT,
                    session_id TEXT NOT NULL,
                    native_session_id TEXT,
                    provider TEXT NOT NULL,
                    prompt TEXT NOT NULL,
                    project_path TEXT NOT NULL,
                    model TEXT,
                    effort TEXT,
                    mode TEXT,
                    thinking INTEGER,
                    status TEXT NOT NULL,
                    auto_resume INTEGER NOT NULL DEFAULT 1,
                    resume_attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    recovered_at TEXT,
                    completed_at TEXT
                );

                CREATE INDEX idx_durable_chat_runs_recoverable
                    ON durable_chat_runs(status, auto_resume, resume_attempts, updated_at);

                CREATE INDEX idx_durable_chat_runs_session
                    ON durable_chat_runs(session_id, created_at DESC);
                "#,
            )
            .expect("create legacy schema");
        }

        let storage = Storage::open(&database).expect("migrate legacy durable schema");
        storage
            .with_connection(|conn| {
                for column in ["user_message_id", "native_before_turn_id", "fast"] {
                    let present: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('durable_chat_runs') WHERE name = ?1",
                        params![column],
                        |row| row.get(0),
                    )?;
                    assert_eq!(present, 1, "missing migrated column {column}");
                }

                let index_present: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM pragma_index_list('durable_chat_runs') WHERE name = 'idx_durable_chat_runs_user_message'",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(index_present, 1);
                Ok(())
            })
            .expect("inspect migrated schema");

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn durable_chat_turn_is_atomic_and_indexes_native_identity() {
        let (storage, root) = temporary_storage("durable-turn-atomic");
        let mut session = test_session("session-turn", true);
        let message = test_message("message-turn", MessageRole::User, "prompt", 0);
        session.message_count = 1;
        let mut run = StoredDurableChatRun::new(
            "run-turn",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            message.content.clone(),
            session.project_path.clone(),
        );
        run.user_message_id = Some(message.id.clone());
        run.native_before_turn_id = Some("native-turn-before".to_string());

        storage
            .create_durable_chat_turn(&session, &message, &run)
            .expect("create durable turn");
        let stored_messages = storage.list_messages(&session.id).expect("messages");
        assert_eq!(stored_messages.len(), 1);
        assert_eq!(stored_messages[0].id, message.id);
        assert_eq!(stored_messages[0].role, MessageRole::User);
        assert_eq!(stored_messages[0].content, "prompt");
        let restored = storage
            .durable_chat_run_for_user_message(&session.id, "message-turn")
            .expect("durable lookup")
            .expect("durable run");
        assert_eq!(restored.id, "run-turn");
        assert_eq!(
            restored.native_before_turn_id.as_deref(),
            Some("native-turn-before")
        );

        let duplicate = test_message("message-turn", MessageRole::User, "duplicate", 1);
        let mut failed_session = test_session("session-rolled-back", true);
        failed_session.message_count = 1;
        let mut failed_run = StoredDurableChatRun::new(
            "run-rolled-back",
            None,
            failed_session.id.clone(),
            "codex",
            duplicate.content.clone(),
            failed_session.project_path.clone(),
        );
        failed_run.user_message_id = Some(duplicate.id.clone());
        assert!(
            storage
                .create_durable_chat_turn(&failed_session, &duplicate, &failed_run)
                .is_err()
        );
        assert!(
            storage
                .get_session(&failed_session.id)
                .expect("rolled-back session lookup")
                .is_none()
        );
        assert!(
            storage
                .get_durable_chat_run(&failed_run.id)
                .expect("rolled-back run lookup")
                .is_none()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn chat_run_attempt_usage_accumulates_lifetime_total() {
        let (storage, root) = temporary_storage("chat-run-attempt-usage");
        let session = test_session("session-usage", false);
        storage.upsert_session(&session).expect("session");

        let mut run = StoredDurableChatRun::new(
            "run-usage",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            "prompt",
            session.project_path.clone(),
        );
        run.user_message_id = Some("message-usage".to_string());
        storage.create_durable_chat_run(&run).expect("run");

        let attempt = StoredChatRunAttempt::new(
            "attempt-usage",
            run.id.clone(),
            session.id.clone(),
            run.user_message_id.clone(),
            "codex",
            "native_cli",
            Some("gpt-test".to_string()),
            Some("native-1".to_string()),
        );
        assert!(
            storage
                .create_chat_run_attempt(&attempt)
                .expect("insert attempt")
        );
        assert!(
            !storage
                .create_chat_run_attempt(&attempt)
                .expect("idempotent attempt")
        );
        let lifetime = storage
            .finish_chat_run_attempt(
                &attempt.id,
                "completed",
                Some(&SessionTokenUsage {
                    used: 42,
                    input: 30,
                    output: 12,
                    cache_creation: 3,
                    cache_read: 20,
                    reasoning: 5,
                    cost_usd: 0.01,
                }),
                Some(r#"{"total_tokens":42}"#),
                Some("test"),
                TokenUsageCompleteness::Complete,
            )
            .expect("finish attempt")
            .expect("lifetime");
        assert_eq!(lifetime.total, 42);
        assert_eq!(lifetime.input, 30);
        assert_eq!(lifetime.output, 12);
        assert_eq!(lifetime.cache_read, 20);
        assert_eq!(lifetime.reasoning, 5);
        assert_eq!(lifetime.completeness, TokenUsageCompleteness::Complete);
        let latest = storage
            .latest_session_token_usage(&session.id)
            .expect("latest usage query")
            .expect("latest usage");
        assert_eq!(latest.used, 42);
        assert_eq!(latest.input, 30);
        assert_eq!(latest.output, 12);
        assert!(
            storage
                .get_session_summary(&session.id)
                .expect("lightweight session")
                .expect("session")
                .lifetime_token_usage
                .is_none(),
        );
        assert_eq!(
            storage
                .list_sessions()
                .expect("session list")
                .into_iter()
                .find(|listed| listed.id == session.id)
                .and_then(|listed| listed.lifetime_token_usage)
                .map(|usage| usage.total),
            Some(42),
        );
        assert_eq!(
            storage
                .list_sessions()
                .expect("session list")
                .into_iter()
                .find(|listed| listed.id == session.id)
                .and_then(|listed| listed.context_token_usage)
                .map(|usage| (usage.total, usage.after_compact)),
            Some((42, false)),
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

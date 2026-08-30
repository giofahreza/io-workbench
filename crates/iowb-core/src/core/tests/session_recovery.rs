    #[tokio::test(flavor = "current_thread")]
    async fn token_usage_persists_and_clears_when_the_next_turn_starts() {
        let root = std::env::temp_dir().join(format!("iowb-token-usage-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("root");
        let storage = Storage::open(root.join("test.db")).expect("storage");
        let sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        let session = sessions
            .create_or_update(
                Provider::Codex,
                root.display().to_string(),
                None,
                false,
                Some("gpt-test".to_string()),
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("create session");

        sessions
            .set_token_usage(
                &session.id,
                SessionTokenUsage {
                    used: 4_321,
                    input: 1_500,
                    output: 2_700,
                    cache_creation: 0,
                    cache_read: 121,
                    reasoning: 0,
                    cost_usd: 0.0,
                },
            )
            .await
            .expect("store token usage");

        let restored = storage
            .get_session(&session.id)
            .expect("stored session")
            .expect("session exists");
        assert_eq!(
            restored.token_usage.as_ref().map(|usage| usage.used),
            Some(4_321)
        );
        assert_eq!(
            sessions
                .get(&session.id)
                .await
                .expect("cached session")
                .token_usage
                .as_ref()
                .map(|usage| usage.cache_read),
            Some(121),
        );

        let restarted = sessions
            .create_or_update(
                Provider::Codex,
                root.display().to_string(),
                Some(session.id.clone()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("restart session");
        assert!(restarted.token_usage.is_none());
        assert!(
            storage
                .get_session(&session.id)
                .expect("stored session")
                .expect("session exists")
                .token_usage
                .is_none()
        );

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn loading_preserves_active_sessions_until_startup_reconciliation() {
        let root = std::env::temp_dir().join(format!("iowb-stale-session-{}", Uuid::new_v4()));
        let config_dir = root.join("config");
        std::fs::create_dir_all(&config_dir).expect("config dir");
        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        let now = Utc::now();
        let session = SessionSummary {
            id: "stale-session".to_string(),
            provider: Provider::Codex,
            external: false,
            board_session: false,
            board_id: None,
            board_task_id: None,
            project_path: root.display().to_string(),
            title: "Interrupted chat".to_string(),
            message_count: 1,
            last_activity: now,
            active: true,
            model: Some("gpt-test".to_string()),
            runtime: Some(ChatRuntime::NativeCli),
            effort: Some("medium".to_string()),
            mode: Some("default".to_string()),
            thinking: Some(false),
            fast: Some(false),
            last_message_at: Some(now),
            first_user_at: Some(now),
            received_at: None,
            token_usage: None,
            lifetime_token_usage: None,
            context_token_usage: None,
            spent_token_usage: None,
            native_session_id: Some("native-session".to_string()),
            native_rollout_owned_by_provider: false,
            title_source: Some(SessionTitleSource::Manual),
        };
        storage
            .upsert_session(&session)
            .expect("upsert active session");
        storage
            .append_message(
                "stale-session",
                &ChatMessage {
                    id: "msg-user".to_string(),
                    role: MessageRole::User,
                    content: "please continue".to_string(),
                    timestamp: now,
                    metadata: Value::Null,
                },
            )
            .expect("append user message");

        let sessions = SessionManager::load(storage.clone(), 10).expect("sessions");

        assert_eq!(sessions.list_active().await.len(), 1);
        sessions
            .mark_unrecovered_active_sessions_interrupted(&HashSet::new())
            .await
            .expect("reconcile stale session");
        assert!(sessions.list_active().await.is_empty());
        let stored = storage
            .get_session("stale-session")
            .expect("stored session")
            .expect("session exists");
        assert!(!stored.active);
        assert_eq!(stored.message_count, 2);
        let messages = storage
            .list_messages("stale-session")
            .expect("stored messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, MessageRole::System);
        assert!(messages[1].content.contains("Server restarted"));
        assert_eq!(
            messages[1].metadata["reason"].as_str(),
            Some("server_restart")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_codex_thread_mapping_persists_resumes_and_hides_rollout() {
        let root = std::env::temp_dir().join(format!("iowb-native-thread-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        let native_id = "22222222-2222-4222-8222-222222222222";
        let historical_native_id = "33333333-3333-4333-8333-333333333333";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/07/31")
            .join(format!("rollout-2026-07-31T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "first prompt",
                        "kind": "plain"
                    }
                })
            ),
        )
        .expect("rollout");
        let historical_rollout = rollout.parent().expect("rollout parent").join(format!(
            "rollout-2026-07-30T23-59-00-{historical_native_id}.jsonl"
        ));
        std::fs::write(
            historical_rollout,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": now - chrono::Duration::seconds(60),
                    "type": "session_meta",
                    "payload": {"id": historical_native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now - chrono::Duration::seconds(60),
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "older prompt",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now - chrono::Duration::seconds(59),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "older answer"}]
                    }
                })
            ),
        )
        .expect("historical rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        storage
            .upsert_session(&SessionSummary {
                id: historical_native_id.to_string(),
                provider: Provider::Codex,
                external: true,
                project_path: project.display().to_string(),
                title: "Stored historical session".to_string(),
                message_count: 1,
                last_activity: now - chrono::Duration::seconds(60),
                ..Default::default()
            })
            .expect("stored historical external session");
        let mut historical_attempt = StoredChatRunAttempt::new(
            "attempt-historical-usage",
            "run-historical-usage",
            historical_native_id,
            None,
            "codex",
            "legacy_history",
            None,
            Some(historical_native_id.to_string()),
        );
        historical_attempt.status = "completed".to_string();
        historical_attempt.usage = Some(SessionTokenUsage {
            used: 42,
            input: 30,
            output: 12,
            cache_creation: 0,
            cache_read: 0,
            reasoning: 0,
            cost_usd: 0.0,
        });
        historical_attempt.source = Some("test".to_string());
        historical_attempt.completeness = TokenUsageCompleteness::Complete;
        historical_attempt.created_at = now;
        historical_attempt.updated_at = now;
        historical_attempt.completed_at = Some(now);
        storage
            .create_chat_run_attempt(&historical_attempt)
            .expect("historical usage attempt");
        let mut sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        sessions.external_home = Arc::new(root.clone());
        let internal = sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("new-session-test".to_string()),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("internal session");
        sessions
            .append_message(&internal.id, MessageRole::User, "older prompt")
            .await
            .expect("older stored prompt");
        sessions
            .append_message(&internal.id, MessageRole::User, "first prompt")
            .await
            .expect("stored prompt");
        let inferred = sessions
            .infer_native_session_id(
                &internal.id,
                Provider::Codex,
                project.to_str().expect("project path"),
            )
            .await
            .expect("native mapping");
        assert_eq!(inferred.as_deref(), Some(native_id));

        let stored = storage
            .get_session(&internal.id)
            .expect("storage query")
            .expect("stored session");
        assert_eq!(stored.native_session_id.as_deref(), Some(native_id));
        let api_json = serde_json::to_value(&stored).expect("session JSON");
        assert_eq!(api_json["nativeSessionId"], native_id);

        let existing_rollout = std::fs::read_to_string(&rollout).expect("read rollout");
        std::fs::write(
            &rollout,
            format!(
                "{existing_rollout}{}\n{}\n",
                serde_json::json!({
                    "timestamp": now + chrono::Duration::seconds(1),
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "continued outside Workbench",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now + chrono::Duration::seconds(2),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "external answer"}]
                    }
                })
            ),
        )
        .expect("append external continuation");

        let mapped_messages = sessions
            .messages_including_external(&internal.id)
            .await
            .expect("mapped external messages");
        assert_eq!(
            mapped_messages
                .iter()
                .filter(|message| message.content == "first prompt")
                .count(),
            1,
            "mapped history must not duplicate the first Workbench prompt: {mapped_messages:#?}"
        );
        assert_eq!(
            mapped_messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            [
                "first prompt",
                "continued outside Workbench",
                "external answer"
            ]
        );

        let listed = sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("project sessions");
        assert!(
            listed.iter().all(|session| session.id != native_id),
            "mapped native rollout must not appear as an extra chat: {listed:#?}"
        );
        let historical_session = listed
            .iter()
            .find(|session| session.id == historical_native_id);
        assert!(
            historical_session.is_some(),
            "unmapped historical rollout must remain discoverable: {listed:#?}"
        );
        assert_eq!(
            historical_session
                .expect("historical session checked above")
                .message_count,
            2,
            "unmapped external session count must come from loaded rollout messages"
        );
        assert_eq!(
            historical_session
                .expect("historical session checked above")
                .lifetime_token_usage
                .as_ref()
                .map(|usage| usage.total),
            Some(42),
            "external refresh must preserve stored lifetime token usage"
        );
        let internal_session = listed.iter().find(|session| session.id == internal.id);
        assert!(
            internal_session.is_some(),
            "internal chat must remain visible: {listed:#?}"
        );
        assert_eq!(
            internal_session
                .expect("internal session checked above")
                .message_count,
            mapped_messages.len(),
            "mapped native session list count must match loaded message total"
        );
        sessions
            .set_active(&internal.id, true)
            .await
            .expect("mark mapped session active");
        let active_sessions = sessions.list_active().await;
        let active_internal_session = active_sessions
            .iter()
            .find(|session| session.id == internal.id)
            .expect("mapped active session");
        assert_eq!(
            active_internal_session.message_count,
            mapped_messages.len(),
            "active-session events must use the loaded message total"
        );
        sessions
            .set_active(&internal.id, false)
            .await
            .expect("mark mapped session inactive");

        let stale_internal = sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("stale-count-session".to_string()),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("stale-count session");
        sessions
            .set_native_session_id(&stale_internal.id, historical_native_id.to_string())
            .await
            .expect("map stale-count session");
        sessions
            .set_active(&stale_internal.id, false)
            .await
            .expect("mark stale-count session inactive");
        let mut stale_summary = storage
            .get_session(&stale_internal.id)
            .expect("stale-count storage query")
            .expect("stale-count stored session");
        stale_summary.message_count = 99;
        storage
            .upsert_session(&stale_summary)
            .expect("persist stale count");
        let stale_listed = sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("project sessions after stale count");
        let stale_listed_session = stale_listed
            .iter()
            .find(|session| session.id == stale_internal.id)
            .expect("stale-count listed session");
        assert_eq!(
            stale_listed_session.message_count, 2,
            "inactive mapped session counts must come from loaded messages, not stale stored metadata"
        );
        let args = default_agent_args_with_resume(
            Provider::Codex,
            "second prompt",
            None,
            None,
            None,
            None,
            None,
            stored.native_session_id.as_deref(),
            ChatRuntime::NativeCli,
        );
        assert!(
            args.contains(&"--json".to_string()),
            "Codex must emit thread.started JSON: {args:?}"
        );
        assert_eq!(
            &args[args.iter().position(|arg| arg == "resume").unwrap()..],
            ["resume", native_id, "second prompt"]
        );

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn persisted_native_mapping_hides_rollout_after_memory_eviction() {
        let root = std::env::temp_dir().join(format!("iowb-native-eviction-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        let native_id = "44444444-4444-4444-8444-444444444444";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/11")
            .join(format!("rollout-2026-08-11T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project, "thread_source": "user"}
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "mapped prompt",
                        "kind": "plain"
                    }
                })
            ),
        )
        .expect("rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        let mapped_session = SessionSummary {
            id: "mapped-workbench-session".to_string(),
            provider: Provider::Codex,
            project_path: project.display().to_string(),
            title: "Mapped session".to_string(),
            last_activity: now - chrono::Duration::minutes(1),
            native_session_id: Some(native_id.to_string()),
            ..Default::default()
        };
        storage
            .upsert_session(&mapped_session)
            .expect("mapped session");
        storage
            .upsert_session(&SessionSummary {
                id: "newer-session".to_string(),
                provider: Provider::Codex,
                project_path: project.display().to_string(),
                title: "Newer session".to_string(),
                last_activity: now,
                ..Default::default()
            })
            .expect("newer session");

        let mut sessions = SessionManager::load(storage.clone(), 1).expect("sessions");
        assert!(
            sessions
                .sessions
                .read()
                .await
                .get(&mapped_session.id)
                .is_none(),
            "mapped session must be outside the in-memory cache for this regression"
        );
        sessions.external_home = Arc::new(root.clone());

        let listed = sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("project sessions");
        assert!(listed.iter().any(|session| session.id == mapped_session.id));
        assert!(
            listed.iter().all(|session| session.id != native_id),
            "persisted native mapping must hide the external rollout after eviction: {listed:#?}"
        );

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn codex_rollout_sync_appends_missing_workbench_turn_once() {
        let root = std::env::temp_dir().join(format!("iowb-codex-sync-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        let native_id = "44444444-4444-4444-8444-444444444444";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/06")
            .join(format!("rollout-2026-08-06T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "original cli prompt",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "original cli answer"}]
                    }
                })
            ),
        )
        .expect("rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        let mut sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        sessions.external_home = Arc::new(root.clone());
        let internal = sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("workbench-session".to_string()),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("internal session");
        sessions
            .set_native_session_id(&internal.id, native_id)
            .await
            .expect("native id");

        let appended = sessions
            .sync_codex_turn_to_native_rollout(
                &internal.id,
                "continued in Workbench",
                "answer from Workbench",
            )
            .await
            .expect("sync append");
        assert!(appended);

        let messages = sessions
            .messages_including_external(&internal.id)
            .await
            .expect("messages");
        assert_eq!(
            messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            [
                "original cli prompt",
                "original cli answer",
                "continued in Workbench",
                "answer from Workbench"
            ]
        );

        let second = sessions
            .sync_codex_turn_to_native_rollout(
                &internal.id,
                "continued in Workbench",
                "answer from Workbench",
            )
            .await
            .expect("sync duplicate");
        assert!(!second);
        let messages_after_second = sessions
            .messages_including_external(&internal.id)
            .await
            .expect("messages after duplicate sync");
        assert_eq!(messages_after_second.len(), messages.len());

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn codex_rollout_sync_trusts_existing_response_and_rejects_transcript() {
        let root = std::env::temp_dir().join(format!("iowb-codex-sync-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        let native_id = "55555555-5555-4555-8555-555555555555";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/11")
            .join(format!("rollout-2026-08-11T10-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "continued in Workbench",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "phase": "final_answer",
                        "content": [{"type": "output_text", "text": "Native answer with markdown."}]
                    }
                })
            ),
        )
        .expect("rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        let mut sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        sessions.external_home = Arc::new(root.clone());
        let internal = sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("workbench-session-existing-final".to_string()),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("internal session");
        sessions
            .set_native_session_id(&internal.id, native_id)
            .await
            .expect("native id");

        let original_rollout = std::fs::read_to_string(&rollout).expect("original rollout");
        let appended = sessions
            .sync_codex_turn_to_native_rollout(
                &internal.id,
                "continued in Workbench",
                "Native answer with different formatting",
            )
            .await
            .expect("sync existing response");
        assert!(!appended);
        assert_eq!(
            original_rollout,
            std::fs::read_to_string(&rollout).expect("unchanged rollout")
        );

        let transcript = concat!(
            "thinking\nInspecting files\n\n",
            "exec / Parameters\n**Tool:** `command_execution`\n\n",
            "codex\nSynthetic answer\n\n",
            "tokens used\n{\"output_tokens\":8}"
        );
        let transcript_appended = sessions
            .sync_codex_turn_to_native_rollout(&internal.id, "another prompt", transcript)
            .await
            .expect("sync transcript");
        assert!(!transcript_appended);
        assert_eq!(
            original_rollout,
            std::fs::read_to_string(&rollout).expect("rollout after transcript rejection")
        );

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mapped_codex_history_returns_one_main_response_for_legacy_duplicate() {
        let root = std::env::temp_dir().join(format!("iowb-codex-history-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        let native_id = "66666666-6666-4666-8666-666666666666";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/11")
            .join(format!("rollout-2026-08-11T11-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        let transcript = format!(
            "thinking\n{}\n\nexec / Parameters\n**Tool:** `command_execution`\n\ncodex\nNormal main response.\n\ntokens used\n{{\"output_tokens\":8}}",
            "x".repeat(103_000)
        );
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "Why is the response duplicated?",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "id": "msg-native-final",
                        "role": "assistant",
                        "phase": "final_answer",
                        "content": [{"type": "output_text", "text": "Normal main response."}]
                    }
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "id": "msg-workbench-transcript",
                        "role": "assistant",
                        "source": "io-workbench",
                        "content": [{"type": "output_text", "text": transcript}]
                    }
                })
            ),
        )
        .expect("rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        let mut sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        sessions.external_home = Arc::new(root.clone());
        let internal = sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("workbench-session-legacy-duplicate".to_string()),
                false,
                None,
                None,
                Some("ultra".to_string()),
                Some("ultra".to_string()),
                Some(true),
                None,
            )
            .await
            .expect("internal session");
        sessions
            .append_message(
                &internal.id,
                MessageRole::User,
                "Why is the response duplicated?",
            )
            .await
            .expect("stored user message");
        sessions
            .append_message(&internal.id, MessageRole::Assistant, transcript.clone())
            .await
            .expect("stored transcript");
        sessions
            .set_native_session_id(&internal.id, native_id)
            .await
            .expect("native id");

        let messages = sessions
            .messages_including_external(&internal.id)
            .await
            .expect("mapped messages");
        let main_responses = messages
            .iter()
            .filter(|message| {
                message.role == MessageRole::Assistant
                    && !message.content.trim_start().starts_with("thinking\n")
            })
            .collect::<Vec<_>>();
        assert_eq!(1, main_responses.len(), "{messages:#?}");
        assert_eq!("Normal main response.", main_responses[0].content);
        assert_eq!(
            1,
            messages
                .iter()
                .map(|message| message.content.matches("Normal main response.").count())
                .sum::<usize>()
        );
        assert!(messages.iter().all(|message| message.content != transcript));

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn base32_secret_decodes_rfc_vector() {
        let secret =
            decode_base32_secret("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").expect("valid base32");
        assert_eq!(secret, b"12345678901234567890");
    }

    #[test]
    fn hotp_matches_rfc_vector() {
        let secret =
            decode_base32_secret("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").expect("valid base32");
        assert_eq!(hotp(&secret, 1).expect("hotp"), 287082);
    }

    #[test]
    fn base32_secret_rejects_invalid_characters() {
        assert!(decode_base32_secret("iowb-c5e354e6e3a5741e").is_err());
    }

    #[test]
    fn auth_required_env_defaults_secure_but_allows_explicit_opt_out() {
        const KEY: &str = "IOWB_TEST_AUTH_REQUIRED_DEFAULT";
        unsafe {
            std::env::remove_var(KEY);
        }
        assert!(env_bool(KEY, true));

        unsafe {
            std::env::set_var(KEY, "false");
        }
        assert!(!env_bool(KEY, true));

        unsafe {
            std::env::remove_var(KEY);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configured_agent_runtime_persists_assistant_output() {
        let root = std::env::temp_dir().join(format!("iowb-agent-test-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        unsafe {
            std::env::set_var("IO_WORKBENCH_AGENT_COMMAND", "/bin/sh");
            std::env::set_var(
                "IO_WORKBENCH_AGENT_ARGS_JSON",
                r#"["-c","printf 'agent:%s\n' \"$1\"","iowb-agent","{prompt}"]"#,
            );
        }

        let state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
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
            .start_agent_session(
                Provider::Codex,
                project.display().to_string(),
                "hello",
                None,
                None,
                None,
                None,
                None,
                None,
                ChatRuntime::NativeCli,
                None,
                None,
            )
            .await
            .expect("agent starts");

        let mut saw_output = false;
        for _ in 0..20 {
            let messages = state.sessions.messages(&session.id).expect("messages");
            if messages.iter().any(|message| {
                message.role == MessageRole::Assistant && message.content.contains("agent:hello")
            }) {
                saw_output = true;
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }

        unsafe {
            std::env::remove_var("IO_WORKBENCH_AGENT_COMMAND");
            std::env::remove_var("IO_WORKBENCH_AGENT_ARGS_JSON");
        }

        let messages = state.sessions.messages(&session.id).expect("messages");
        let user_message = messages
            .iter()
            .find(|message| message.role == MessageRole::User)
            .expect("persisted user message");
        assert_eq!(user_message.metadata["cli"], "codex");
        assert_eq!(user_message.metadata["model"], "");
        assert!(user_message.metadata["sentAt"].as_str().is_some());

        let assistant_message = messages
            .iter()
            .find(|message| {
                message.role == MessageRole::Assistant && message.content.contains("agent:hello")
            })
            .expect("persisted assistant message");
        assert_eq!(assistant_message.metadata["cli"], "codex");
        assert!(assistant_message.metadata["receivedAt"].as_str().is_some());
        assert!(assistant_message.metadata["sentAt"].as_str().is_some());
        assert!(
            assistant_message.metadata["elapsedMs"].as_i64().is_some(),
            "assistant metadata: {:?}",
            assistant_message.metadata
        );

        // Simulate the server-side token-usage stamp that runs when the UI
        // fetches `/api/.../token-usage`. The per-message metadata should
        // round-trip the nested tokenUsage object so a fresh page load
        // can render the footer without re-hitting the live CLI log.
        let stamp = serde_json::json!({
            "tokenUsage": {
                "used": 4321u64,
                "input": 1500u64,
                "output": 2700u64,
                "cacheCreation": 0u64,
                "cacheRead": 121u64,
            }
        });
        let stamped = state
            .sessions
            .stamp_latest_message_metadata(&session.id, MessageRole::Assistant, stamp.clone())
            .expect("stamp succeeds");
        assert!(stamped, "expected a row to be updated");

        let after = state.sessions.messages(&session.id).expect("messages");
        let assistant_after = after
            .iter()
            .find(|message| {
                message.role == MessageRole::Assistant && message.content.contains("agent:hello")
            })
            .expect("persisted assistant message after stamp");
        assert_eq!(assistant_after.metadata["tokenUsage"]["used"], 4321);
        assert_eq!(assistant_after.metadata["tokenUsage"]["input"], 1500);
        assert_eq!(assistant_after.metadata["tokenUsage"]["output"], 2700);
        assert_eq!(assistant_after.metadata["tokenUsage"]["cacheRead"], 121);
        assert_eq!(assistant_after.metadata["cli"], "codex");
        assert!(assistant_after.metadata["receivedAt"].as_str().is_some());

        let _ = std::fs::remove_dir_all(root);

        assert!(saw_output);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn durable_recovery_resumes_native_session_without_duplicate_user_message() {
        let root = std::env::temp_dir().join(format!("iowb-recovery-test-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");
        let state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
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
            .set_native_session_id(&session.id, "native-recovery-session")
            .await
            .expect("native session id");
        state
            .sessions
            .append_message(
                &session.id,
                MessageRole::User,
                "finish the interrupted implementation",
            )
            .await
            .expect("original user message");

        let mut run = StoredDurableChatRun::new(
            "run-recovery",
            Some("user-recovery".to_string()),
            session.id.clone(),
            "gemini",
            "finish the interrupted implementation",
            project.display().to_string(),
        );
        run.native_session_id = Some("native-recovery-session".to_string());
        state
            .storage
            .create_durable_chat_run(&run)
            .expect("durable run");
        let claimed = state
            .storage
            .mark_durable_chat_run_recovering(&run.id, DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS)
            .expect("claim recovery")
            .expect("recoverable run");

        unsafe {
            std::env::set_var("IO_WORKBENCH_GEMINI_COMMAND", "/bin/sh");
            std::env::set_var(
                "IO_WORKBENCH_GEMINI_ARGS_JSON",
                r#"["-c","printf 'resumed:%s\\n' \"$1\"","iowb-recovery","{native_session_id}"]"#,
            );
        }
        let recovered = state
            .recover_agent_run(claimed, None)
            .await
            .expect("recovery starts");
        unsafe {
            std::env::remove_var("IO_WORKBENCH_GEMINI_COMMAND");
            std::env::remove_var("IO_WORKBENCH_GEMINI_ARGS_JSON");
        }
        assert_eq!(recovered.id, session.id);

        timeout(Duration::from_secs(3), async {
            loop {
                let stored_run = state
                    .storage
                    .get_durable_chat_run(&run.id)
                    .expect("read durable run")
                    .expect("durable run exists");
                if stored_run.status == "completed" {
                    break;
                }
                sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("recovered provider completes");

        let messages = state
            .storage
            .list_messages(&session.id)
            .expect("session messages");
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .count(),
            1,
            "recovery must not append its hidden prompt as another user row"
        );
        let assistant = messages
            .iter()
            .find(|message| message.role == MessageRole::Assistant)
            .expect("recovered assistant message");
        assert!(
            assistant
                .content
                .contains("resumed:native-recovery-session"),
            "{}",
            assistant.content
        );
        assert_eq!(assistant.metadata["durableRunId"], run.id);
        let stored_run = state
            .storage
            .get_durable_chat_run(&run.id)
            .expect("read durable run")
            .expect("durable run exists");
        assert_eq!(stored_run.resume_attempts, 1);
        assert_eq!(
            stored_run.native_session_id.as_deref(),
            Some("native-recovery-session")
        );
        assert!(
            !state
                .storage
                .get_session(&session.id)
                .expect("stored session")
                .expect("session exists")
                .active
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn project_session_list_does_not_cache_external_rollout_messages() {
        let root =
            std::env::temp_dir().join(format!("iowb-list-external-cache-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        let native_id = "44444444-4444-4444-8444-444444444444";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/14")
            .join(format!("rollout-2026-08-14T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now + chrono::Duration::seconds(1),
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "check memory",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now + chrono::Duration::seconds(2),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "done"}]
                    }
                })
            ),
        )
        .expect("rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        let mut sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        sessions.external_home = Arc::new(root.clone());
        let mapped = sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("mapped-list-session".to_string()),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("mapped session");
        sessions
            .set_native_session_id(&mapped.id, native_id.to_string())
            .await
            .expect("native mapping");

        let listed = sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("project sessions");
        assert!(
            listed.iter().any(|session| session.id == mapped.id),
            "mapped Workbench session must remain listed: {listed:#?}"
        );
        assert!(
            listed.iter().all(|session| session.id != native_id),
            "mapped native rollout must not be listed separately: {listed:#?}"
        );
        assert!(
            sessions.external_cache.read().await.messages.is_empty(),
            "project list must not parse and cache full external rollout messages"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rollover_recovery_never_infers_or_resumes_archived_native_context() {
        let root = std::env::temp_dir().join(format!(
            "iowb-rollover-recovery-selection-{}",
            Uuid::new_v4()
        ));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");
        let database = config_dir.join("test.db");
        let initial_state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: database.clone(),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("initial state");
        initial_state
            .storage
            .create_user("user-rollover", "user-rollover", "test-hash")
            .expect("create user");
        let mut session = initial_state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-rollover-recovery".to_string()),
                false,
                None,
                Some(ChatRuntime::IoGateway),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        initial_state
            .sessions
            .set_native_session_id(&session.id, "native-poisoned")
            .await
            .expect("poisoned mapping");
        let failed_message = initial_state
            .sessions
            .append_message(
                &session.id,
                MessageRole::User,
                "finish the image-heavy request",
            )
            .await
            .expect("failed prompt");
        initial_state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("inactive session");
        session = initial_state
            .sessions
            .get(&session.id)
            .await
            .expect("stored session");

        let mut trigger_run = StoredDurableChatRun::new(
            "run-rollover-trigger",
            Some("user-rollover".to_string()),
            session.id.clone(),
            "codex",
            failed_message.content.clone(),
            project.display().to_string(),
        );
        trigger_run.user_message_id = Some(failed_message.id.clone());
        trigger_run.native_session_id = Some("native-poisoned".to_string());
        initial_state
            .storage
            .create_durable_chat_run(&trigger_run)
            .expect("trigger run");
        initial_state
            .storage
            .mark_durable_chat_run_failed(&trigger_run.id, "invalid body")
            .expect("trigger failed");
        let now = Utc::now();
        let rollover = StoredSessionContextRollover {
            id: "rollover-recovery-selection".to_string(),
            user_id: "user-rollover".to_string(),
            session_id: session.id.clone(),
            request_id: "request-rollover-recovery".to_string(),
            kind: CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN.to_string(),
            failed_message_id: failed_message.id.clone(),
            trigger_run_id: trigger_run.id.clone(),
            retry_run_id: "run-rollover-retry".to_string(),
            from_native_session_id: Some("native-poisoned".to_string()),
            candidate_native_session_id: None,
            state: "starting".to_string(),
            handoff: "bounded text-only handoff".to_string(),
            observed_bytes: Some(19_760_000),
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut retry_run = StoredDurableChatRun::new(
            rollover.retry_run_id.clone(),
            Some("user-rollover".to_string()),
            session.id.clone(),
            "codex",
            rollover.handoff.clone(),
            project.display().to_string(),
        );
        retry_run.user_message_id = Some(failed_message.id.clone());
        assert!(
            initial_state
                .storage
                .prepare_context_rollover(&rollover, &retry_run)
                .expect("prepare rollover")
        );
        drop(initial_state);

        // Reopen from disk to exercise the same selection logic used after a
        // forced server restart. This fake external rollout would be a valid
        // inference match for the failed prompt if rollover recovery did not
        // explicitly suppress inference.
        let restarted = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: database.clone(),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("restarted state");
        {
            let mut cache = restarted.sessions.external_cache.write().await;
            cache.loaded_at = Some(Instant::now());
            let record = ExternalSessionRecord {
                summary: SessionSummary {
                    id: "native-inferred-poison".to_string(),
                    provider: Provider::Codex,
                    external: true,
                    project_path: project.display().to_string(),
                    title: failed_message.content.clone(),
                    last_activity: Utc::now(),
                    ..Default::default()
                },
                file_path: root.join("missing-inference-rollout.jsonl"),
            };
            let cache_key = external_session_cache_key(&record);
            let cached_messages = Arc::new(vec![failed_message.clone()]);
            let estimated_bytes = estimate_external_messages_bytes(cached_messages.as_ref());
            cache.records = vec![record];
            cache.message_bytes = estimated_bytes;
            cache.messages.insert(
                cache_key,
                CachedExternalMessages {
                    modified_at: None,
                    estimated_bytes,
                    last_access: Instant::now(),
                    total_count: cached_messages.len(),
                    complete: true,
                    messages: cached_messages,
                },
            );
        }
        let claimed = restarted
            .storage
            .mark_durable_chat_run_recovering(&retry_run.id, DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS)
            .expect("claim rollover recovery")
            .expect("recoverable rollover run");
        let recovery = restarted.recover_agent_run(claimed, None).await;
        assert!(
            matches!(recovery, Err(CoreError::InvalidInput(_))),
            "missing gateway config should stop after native-id selection: {recovery:?}"
        );
        let stored_retry = restarted
            .storage
            .get_durable_chat_run(&retry_run.id)
            .expect("retry lookup")
            .expect("retry run");
        assert_eq!(stored_retry.native_session_id, None);
        assert_ne!(
            stored_retry.native_session_id.as_deref(),
            Some("native-poisoned")
        );
        assert_ne!(
            stored_retry.native_session_id.as_deref(),
            Some("native-inferred-poison")
        );
        assert_eq!(
            restarted
                .storage
                .get_session(&session.id)
                .expect("session lookup")
                .expect("session")
                .native_session_id
                .as_deref(),
            Some("native-poisoned"),
            "failed recovery must not change the visible session mapping"
        );
        assert_eq!(
            restarted
                .storage
                .context_rollover_for_retry_run(&retry_run.id)
                .expect("rollover lookup")
                .expect("rollover")
                .candidate_native_session_id,
            None
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rollover_recovery_resumes_only_the_staged_clean_candidate() {
        let root =
            std::env::temp_dir().join(format!("iowb-rollover-clean-candidate-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");
        let state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
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
        .expect("state");
        state
            .storage
            .create_user("user-rollover", "user-rollover", "test-hash")
            .expect("create user");
        let mut session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-rollover-clean".to_string()),
                false,
                None,
                Some(ChatRuntime::IoGateway),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        state
            .sessions
            .set_native_session_id(&session.id, "native-poisoned")
            .await
            .expect("old mapping");
        let failed_message = state
            .sessions
            .append_message(&session.id, MessageRole::User, "continue cleanly")
            .await
            .expect("failed prompt");
        state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("inactive");
        session = state.sessions.get(&session.id).await.expect("session");
        let mut trigger_run = StoredDurableChatRun::new(
            "run-clean-trigger",
            Some("user-rollover".to_string()),
            session.id.clone(),
            "codex",
            failed_message.content.clone(),
            project.display().to_string(),
        );
        trigger_run.user_message_id = Some(failed_message.id.clone());
        trigger_run.native_session_id = Some("native-poisoned".to_string());
        state
            .storage
            .create_durable_chat_run(&trigger_run)
            .expect("trigger run");
        state
            .storage
            .mark_durable_chat_run_failed(&trigger_run.id, "invalid body")
            .expect("trigger failed");
        let now = Utc::now();
        let rollover = StoredSessionContextRollover {
            id: "rollover-clean-candidate".to_string(),
            user_id: "user-rollover".to_string(),
            session_id: session.id.clone(),
            request_id: "request-clean-candidate".to_string(),
            kind: CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN.to_string(),
            failed_message_id: failed_message.id.clone(),
            trigger_run_id: trigger_run.id.clone(),
            retry_run_id: "run-clean-retry".to_string(),
            from_native_session_id: Some("native-poisoned".to_string()),
            candidate_native_session_id: None,
            state: "starting".to_string(),
            handoff: "bounded clean handoff".to_string(),
            observed_bytes: Some(19_760_000),
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut retry_run = StoredDurableChatRun::new(
            rollover.retry_run_id.clone(),
            Some("user-rollover".to_string()),
            session.id.clone(),
            "codex",
            rollover.handoff.clone(),
            project.display().to_string(),
        );
        retry_run.user_message_id = Some(failed_message.id.clone());
        assert!(
            state
                .storage
                .prepare_context_rollover(&rollover, &retry_run)
                .expect("prepare rollover")
        );
        assert!(
            state
                .storage
                .set_context_rollover_candidate(&rollover.id, &retry_run.id, "native-clean-staged",)
                .expect("stage clean candidate")
        );
        let claimed = state
            .storage
            .mark_durable_chat_run_recovering(&retry_run.id, DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS)
            .expect("claim recovery")
            .expect("recoverable retry");
        let recovery = state.recover_agent_run(claimed, None).await;
        assert!(matches!(recovery, Err(CoreError::InvalidInput(_))));
        assert_eq!(
            state
                .storage
                .get_durable_chat_run(&retry_run.id)
                .expect("retry lookup")
                .expect("retry")
                .native_session_id
                .as_deref(),
            Some("native-clean-staged")
        );
        assert_ne!(
            state
                .storage
                .get_durable_chat_run(&retry_run.id)
                .expect("retry lookup")
                .expect("retry")
                .native_session_id
                .as_deref(),
            Some("native-poisoned")
        );
        assert_eq!(
            state
                .storage
                .get_session(&session.id)
                .expect("session lookup")
                .expect("session")
                .native_session_id
                .as_deref(),
            Some("native-poisoned"),
            "candidate stays staged until successful atomic completion"
        );

        let _ = std::fs::remove_dir_all(root);
    }

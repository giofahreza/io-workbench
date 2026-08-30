    use super::*;
    use iowb_protocol::{ChatRuntime, SessionTokenUsage};
    use std::collections::HashSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_storage(label: &str) -> (Storage, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "iowb-storage-{label}-{}-{unique}",
            std::process::id()
        ));
        let storage = Storage::open(root.join("test.db")).expect("storage");
        (storage, root)
    }

    fn test_session(id: &str, active: bool) -> SessionSummary {
        SessionSummary {
            id: id.to_string(),
            provider: Provider::Codex,
            project_path: "/tmp/project".to_string(),
            title: "Test session".to_string(),
            last_activity: Utc::now(),
            active,
            ..Default::default()
        }
    }

    fn test_message(id: &str, role: MessageRole, content: &str, seconds: i64) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            role,
            content: content.to_string(),
            timestamp: Utc::now() + chrono::Duration::seconds(seconds),
            metadata: Value::Null,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_completed_usage_attempt(
        storage: &Storage,
        session: &SessionSummary,
        attempt_id: &str,
        run_id: &str,
        created_at: DateTime<Utc>,
        usage: SessionTokenUsage,
        source: &str,
    ) {
        insert_completed_usage_attempt_with_native(
            storage,
            session,
            attempt_id,
            run_id,
            created_at,
            usage,
            source,
            Some("native-1"),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_completed_usage_attempt_with_native(
        storage: &Storage,
        session: &SessionSummary,
        attempt_id: &str,
        run_id: &str,
        created_at: DateTime<Utc>,
        usage: SessionTokenUsage,
        source: &str,
        native_session_id: Option<&str>,
    ) {
        let mut run = StoredDurableChatRun::new(
            run_id,
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            "prompt",
            session.project_path.clone(),
        );
        run.user_message_id = Some(format!("message-{attempt_id}"));
        run.completed_at = Some(created_at);
        run.status = "completed".to_string();
        storage.create_durable_chat_run(&run).expect("run");
        let mut attempt = StoredChatRunAttempt::new(
            attempt_id,
            run.id.clone(),
            session.id.clone(),
            run.user_message_id.clone(),
            "codex",
            "native_cli",
            Some("gpt-test".to_string()),
            native_session_id.map(str::to_string),
        );
        attempt.status = "completed".to_string();
        attempt.usage = Some(usage.clone());
        attempt.raw_usage_json = Some(format!(r#"{{"total_tokens":{}}}"#, usage.used));
        attempt.source = Some(source.to_string());
        attempt.completeness = TokenUsageCompleteness::Complete;
        attempt.created_at = created_at;
        attempt.updated_at = created_at;
        attempt.completed_at = Some(created_at);
        storage.create_chat_run_attempt(&attempt).expect("attempt");
    }

    fn test_context_rollover(
        id: &str,
        session_id: &str,
        request_id: &str,
        trigger_run_id: &str,
        retry_run_id: &str,
        failed_message_id: &str,
        created_at: DateTime<Utc>,
    ) -> StoredSessionContextRollover {
        StoredSessionContextRollover {
            id: id.to_string(),
            user_id: "user-1".to_string(),
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            kind: "retry_failed_turn".to_string(),
            failed_message_id: failed_message_id.to_string(),
            trigger_run_id: trigger_run_id.to_string(),
            retry_run_id: retry_run_id.to_string(),
            from_native_session_id: Some("native-poisoned".to_string()),
            candidate_native_session_id: None,
            state: "starting".to_string(),
            handoff: "bounded text-only handoff".to_string(),
            observed_bytes: Some(19_760_000),
            limit_bytes: 16 * 1024 * 1024,
            error: None,
            created_at,
            updated_at: created_at,
            activated_at: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn failed_context_rollover_discards_provisional_codex_tool_rows() {
        let (state, root, project) = temporary_app_state("rollover-tool-rollback").await;
        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-rollover-tool-rollback".to_string()),
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
        let failed_message = state
            .sessions
            .append_message(
                &session.id,
                MessageRole::User,
                "retry without leaking tools",
            )
            .await
            .expect("failed prompt");
        let mut trigger_run = StoredDurableChatRun::new(
            "run-rollover-tool-trigger",
            Some("user-1".to_string()),
            session.id.clone(),
            Provider::Codex.as_str(),
            failed_message.content.clone(),
            project.display().to_string(),
        );
        trigger_run.user_message_id = Some(failed_message.id.clone());
        state
            .storage
            .create_durable_chat_run(&trigger_run)
            .expect("trigger run");
        state
            .storage
            .mark_durable_chat_run_failed(&trigger_run.id, "invalid body")
            .expect("failed trigger");
        let now = Utc::now();
        let rollover = StoredSessionContextRollover {
            id: "rollover-tool-rollback".to_string(),
            user_id: "user-1".to_string(),
            session_id: session.id.clone(),
            request_id: "request-rollover-tool-rollback".to_string(),
            kind: CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN.to_string(),
            failed_message_id: failed_message.id.clone(),
            trigger_run_id: trigger_run.id.clone(),
            retry_run_id: "run-rollover-tool-retry".to_string(),
            from_native_session_id: Some("native-poisoned".to_string()),
            candidate_native_session_id: None,
            state: "starting".to_string(),
            handoff: "bounded handoff".to_string(),
            observed_bytes: Some(CODEX_CONTEXT_ROLLOVER_THRESHOLD_BYTES),
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut retry_run = StoredDurableChatRun::new(
            rollover.retry_run_id.clone(),
            Some("user-1".to_string()),
            session.id.clone(),
            Provider::Codex.as_str(),
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
        let before = state
            .storage
            .list_messages(&session.id)
            .expect("baseline transcript");
        let context = AgentStartContext {
            provider: Provider::Codex,
            session_id: session.id.clone(),
            durable_run_id: Some(retry_run.id.clone()),
            attempt_id: None,
            response_id: retry_run.id.clone(),
            sequence: Arc::new(AtomicU64::new(0)),
            project_path: project.clone(),
            prompt: rollover.handoff.clone(),
            model: None,
            runtime: ChatRuntime::NativeCli,
            effort: None,
            mode: None,
            thinking: None,
            fast: None,
            native_resume_session_id: None,
            native_rollout_owned_by_provider: false,
            context_rollover_id: Some(rollover.id.clone()),
            direct_ai_config: None,
            direct_ai_messages: Vec::new(),
            sessions: state.sessions.clone(),
            storage: state.storage.clone(),
            hub: WsHub::new(),
        };
        let mut normalizer = Some(CodexLiveOutputNormalizer::default());
        normalizer.as_mut().expect("normalizer").push(&format!(
            "{}\n",
            serde_json::json!({
                "type": "item.completed",
                "item": {
                    "type": "custom_tool_call_output",
                    "name": "view_image",
                    "output": "provisional image analysis that must not persist"
                }
            })
        ));
        persist_codex_tool_messages(&context, &mut normalizer).await;
        AgentRuntimeManager::default()
            .finish(
                "codex:session-rollover-tool-rollback",
                &context,
                iowb_protocol::SessionRuntimeStatus::Failed,
                Some("clean retry failed".to_string()),
                None,
            )
            .await;

        let after = state
            .storage
            .list_messages(&session.id)
            .expect("transcript after failed rollover");
        let transcript_identity = |messages: &[ChatMessage]| {
            messages
                .iter()
                .map(|message| (message.id.clone(), message.role, message.content.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            transcript_identity(&after),
            transcript_identity(&before),
            "a failed rollover must not append provisional tool or assistant rows"
        );
        assert_eq!(
            state
                .storage
                .context_rollover_for_retry_run(&retry_run.id)
                .expect("rollover lookup")
                .expect("rollover")
                .state,
            "failed"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retry_context_rollover_activates_without_follow_up_when_failed_message_row_is_missing()
    {
        let (state, root, project) = temporary_app_state("rollover-missing-failed-message").await;
        let mut session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-rollover-missing-failed-message".to_string()),
                false,
                Some("gpt-5.4".to_string()),
                Some(ChatRuntime::IoGateway),
                Some("high".to_string()),
                Some("default".to_string()),
                Some(true),
                Some(false),
            )
            .await
            .expect("session");
        state
            .sessions
            .set_native_session_id(&session.id, "native-poisoned")
            .await
            .expect("old native session");
        let prior = state
            .sessions
            .append_message(
                &session.id,
                MessageRole::Assistant,
                "Visible history survives",
            )
            .await
            .expect("prior message");
        let failed_message = state
            .sessions
            .append_message(
                &session.id,
                MessageRole::User,
                "Retry prompt row disappears before activation",
            )
            .await
            .expect("failed prompt");
        state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("inactive session");
        session = state
            .sessions
            .get(&session.id)
            .await
            .expect("stored session");

        let mut trigger_run = StoredDurableChatRun::new(
            "run-rollover-missing-trigger",
            Some("user-1".to_string()),
            session.id.clone(),
            Provider::Codex.as_str(),
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
            .expect("failed trigger");

        let now = Utc::now();
        let rollover = StoredSessionContextRollover {
            id: "rollover-missing-failed-message".to_string(),
            user_id: "user-1".to_string(),
            session_id: session.id.clone(),
            request_id: "request-rollover-missing-failed-message".to_string(),
            kind: CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN.to_string(),
            failed_message_id: failed_message.id.clone(),
            trigger_run_id: trigger_run.id.clone(),
            retry_run_id: "run-rollover-missing-retry".to_string(),
            from_native_session_id: Some("native-poisoned".to_string()),
            candidate_native_session_id: None,
            state: "starting".to_string(),
            handoff: "bounded clean-context handoff".to_string(),
            observed_bytes: Some(CODEX_CONTEXT_ROLLOVER_THRESHOLD_BYTES),
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut retry_run = StoredDurableChatRun::new(
            rollover.retry_run_id.clone(),
            Some("user-1".to_string()),
            session.id.clone(),
            Provider::Codex.as_str(),
            rollover.handoff.clone(),
            project.display().to_string(),
        );
        retry_run.model = session.model.clone();
        retry_run.effort = session.effort.clone();
        retry_run.mode = session.mode.clone();
        retry_run.thinking = session.thinking;
        retry_run.fast = session.fast;
        assert!(
            state
                .storage
                .prepare_context_rollover(&rollover, &retry_run)
                .expect("prepare rollover")
        );
        assert!(
            state
                .storage
                .set_context_rollover_candidate(&rollover.id, &retry_run.id, "native-clean")
                .expect("stage clean candidate")
        );
        state
            .storage
            .replace_session_messages(&session.id, std::slice::from_ref(&prior))
            .expect("simulate projection without failed row");
        assert!(
            state
                .storage
                .message_by_id(&session.id, &failed_message.id)
                .expect("failed prompt lookup")
                .is_none()
        );

        let context = AgentStartContext {
            provider: Provider::Codex,
            session_id: session.id.clone(),
            durable_run_id: Some(retry_run.id.clone()),
            attempt_id: None,
            response_id: retry_run.id.clone(),
            sequence: Arc::new(AtomicU64::new(0)),
            project_path: project.clone(),
            prompt: rollover.handoff.clone(),
            model: session.model.clone(),
            runtime: ChatRuntime::IoGateway,
            effort: session.effort.clone(),
            mode: session.mode.clone(),
            thinking: session.thinking,
            fast: session.fast,
            native_resume_session_id: Some("native-clean".to_string()),
            native_rollout_owned_by_provider: false,
            context_rollover_id: Some(rollover.id.clone()),
            direct_ai_config: None,
            direct_ai_messages: Vec::new(),
            sessions: state.sessions.clone(),
            storage: state.storage.clone(),
            hub: WsHub::new(),
        };

        let follow_up = activate_completed_context_rollover(
            &context,
            &rollover.id,
            now + chrono::Duration::seconds(1),
        )
        .await
        .expect("activate compact-only rollover");

        assert!(follow_up.is_none());
        assert!(
            state
                .storage
                .has_active_context_rollover(&session.id)
                .expect("active rollover")
        );
        let completed_retry = state
            .storage
            .get_durable_chat_run(&retry_run.id)
            .expect("retry lookup")
            .expect("retry run");
        assert_eq!(completed_retry.status, "completed");
        assert!(
            state
                .storage
                .list_active_durable_chat_runs()
                .expect("active durable runs")
                .is_empty()
        );
        let stored_session = state
            .storage
            .get_session(&session.id)
            .expect("session lookup")
            .expect("session");
        assert_eq!(
            stored_session.native_session_id.as_deref(),
            Some("native-clean")
        );
        let messages = state.storage.list_messages(&session.id).expect("messages");
        assert!(messages.iter().any(|message| message.id == prior.id));
        assert!(messages.iter().any(|message| {
            message.role == MessageRole::System
                && message.content.starts_with("Context compacted here")
                && message.metadata["failedMessageId"] == failed_message.id
        }));
        assert!(
            messages
                .iter()
                .all(|message| message.id != failed_message.id)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retry_compaction_start_allows_missing_failed_message_after_failed_rollover() {
        let (state, root, project) = temporary_app_state("rollover-second-retry-missing-row").await;
        let mut session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-rollover-second-retry-missing-row".to_string()),
                false,
                Some("gpt-5.4".to_string()),
                Some(ChatRuntime::IoGateway),
                Some("high".to_string()),
                Some("default".to_string()),
                Some(true),
                Some(false),
            )
            .await
            .expect("session");
        state
            .sessions
            .set_native_session_id(&session.id, "native-poisoned")
            .await
            .expect("old native session");
        let prior = state
            .sessions
            .append_message(
                &session.id,
                MessageRole::Assistant,
                "Visible history before failed retry",
            )
            .await
            .expect("prior message");
        let failed_message = state
            .sessions
            .append_message(
                &session.id,
                MessageRole::User,
                "Second clean-context retry should compact only",
            )
            .await
            .expect("failed prompt");
        state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("inactive session");
        session = state
            .sessions
            .get(&session.id)
            .await
            .expect("stored session");

        let mut trigger_run = StoredDurableChatRun::new(
            "run-rollover-second-trigger",
            Some("user-1".to_string()),
            session.id.clone(),
            Provider::Codex.as_str(),
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
            .expect("failed trigger");

        let now = Utc::now();
        let previous_rollover = StoredSessionContextRollover {
            id: "rollover-second-previous".to_string(),
            user_id: "user-1".to_string(),
            session_id: session.id.clone(),
            request_id: "request-rollover-second-previous".to_string(),
            kind: CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN.to_string(),
            failed_message_id: failed_message.id.clone(),
            trigger_run_id: trigger_run.id.clone(),
            retry_run_id: "run-rollover-second-previous-retry".to_string(),
            from_native_session_id: Some("native-poisoned".to_string()),
            candidate_native_session_id: None,
            state: "starting".to_string(),
            handoff: "previous bounded handoff".to_string(),
            observed_bytes: Some(CODEX_CONTEXT_ROLLOVER_THRESHOLD_BYTES),
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut previous_retry_run = StoredDurableChatRun::new(
            previous_rollover.retry_run_id.clone(),
            Some("user-1".to_string()),
            session.id.clone(),
            Provider::Codex.as_str(),
            previous_rollover.handoff.clone(),
            project.display().to_string(),
        );
        previous_retry_run.model = session.model.clone();
        previous_retry_run.effort = session.effort.clone();
        previous_retry_run.mode = session.mode.clone();
        previous_retry_run.thinking = session.thinking;
        previous_retry_run.fast = session.fast;
        assert!(
            state
                .storage
                .prepare_context_rollover(&previous_rollover, &previous_retry_run)
                .expect("prepare previous rollover")
        );
        state
            .storage
            .fail_context_rollover(
                &previous_rollover.id,
                "failed to activate clean context: failed user message was not found",
            )
            .expect("fail previous rollover");
        state
            .storage
            .mark_durable_chat_run_failed(&previous_retry_run.id, "provider run failed")
            .expect("fail previous retry run");
        state
            .storage
            .replace_session_messages(&session.id, std::slice::from_ref(&prior))
            .expect("simulate projection without failed row");
        state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("inactive after failed rollover");

        let error = state
            .compact_and_retry_session_context(
                "user-1",
                &session.id,
                &failed_message.id,
                "request-rollover-second-new",
                None,
            )
            .await
            .expect_err("missing IO Gateway config should stop after retry validation");

        assert_eq!(
            error.to_string(),
            "IO Gateway is not configured for this session"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn manual_compaction_uses_native_compact_and_replaces_polluted_local_projection() {
        use std::os::unix::fs::PermissionsExt;

        let (mut state, root, project) = temporary_app_state("native-manual-compact").await;
        let script = root.join("compact-codex.sh");
        let log = root.join("compact-requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"args:$*\" >> '{}'\n\
                 printf '%s\\n' \"gateway:${{IOWB_IO_GATEWAY_API_KEY:-}}\" >> '{}'\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"native-compact\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/completed\",\"params\":{{\"threadId\":\"native-compact\",\"item\":{{\"type\":\"contextCompaction\",\"id\":\"item-compact\"}}}}}}'\n",
                log.display(),
                log.display(),
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
        state.codex_app_server =
            CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));

        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-native-manual-compact".to_string()),
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
            .set_native_session_id(&session.id, "native-compact")
            .await
            .expect("native id");
        state
            .sessions
            .append_message(&session.id, MessageRole::User, "Question before compact")
            .await
            .expect("user message");
        let transcript = concat!(
            "thinking\nInspecting files\n\n",
            "exec / Parameters\n**Tool:** `command_execution`\n\n",
            "codex\nThis whole blob must not replay as the answer.\n\n",
            "tokens used\n{\"output_tokens\":8}"
        );
        state
            .sessions
            .append_message_with_metadata(
                &session.id,
                MessageRole::Assistant,
                transcript,
                Some(serde_json::json!({
                    "cli": "codex",
                    "durableRunId": "run-poisoned",
                })),
            )
            .await
            .expect("polluted assistant row");
        state
            .sessions
            .append_message(&session.id, MessageRole::Assistant, "Actual final answer.")
            .await
            .expect("assistant message");
        state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("idle session");
        let mut metadata_rx = state.ws_hub.subscribe();

        let response = state
            .compact_session_context(
                "user-1",
                &session.id,
                "request-native-compact",
                Some(DirectAiRuntimeConfig {
                    base_url: "https://gateway.example.com/codex/".to_string(),
                    api_key: "test-secret".to_string(),
                    max_tokens: None,
                }),
            )
            .await
            .expect("manual compact");
        assert_eq!(response.state, "starting");
        wait_for_context_rollover_state(&state, &response.response_id, "active").await;
        let metadata_usage = timeout(Duration::from_secs(2), async {
            loop {
                if let WsServerEvent::SessionMetadata {
                    session_id,
                    response_id,
                    context_token_usage,
                    ..
                } = metadata_rx.recv().await.expect("metadata event")
                    && session_id == session.id
                    && response_id.as_deref() == Some(response.response_id.as_str())
                {
                    break context_token_usage.expect("context token usage");
                }
            }
        })
        .await
        .expect("manual compact metadata");
        assert!(metadata_usage.after_compact);
        assert_eq!(metadata_usage.total, 0);
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("args:app-server"));
        assert!(requests.contains("model_provider=iowb_gateway"));
        assert!(
            requests.contains(
                "model_providers.iowb_gateway.base_url=\"https://gateway.example.com/codex\""
            ),
            "{requests}"
        );
        assert!(
            requests.contains("model_providers.iowb_gateway.env_key=\"IOWB_IO_GATEWAY_API_KEY\"")
        );
        assert!(requests.contains("gateway:test-secret"));
        assert!(requests.contains("\"method\":\"thread/resume\""));
        assert!(requests.contains("\"method\":\"thread/compact/start\""));

        let messages = state
            .sessions
            .messages_including_external(&session.id)
            .await
            .expect("messages after compact");
        let contents = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(contents.len(), 3, "{contents:#?}");
        assert!(contents.contains(&"Question before compact"));
        assert!(contents.contains(&"Actual final answer."));
        assert!(
            contents
                .iter()
                .any(|content| content.contains("Context compacted here"))
        );
        assert!(
            contents
                .iter()
                .all(|content| !content.contains("exec / Parameters")),
            "{contents:#?}"
        );

        let stored_run = state
            .storage
            .get_durable_chat_run(&response.response_id)
            .expect("run lookup")
            .expect("run");
        assert_eq!(stored_run.status, "completed");
        assert_eq!(
            stored_run.native_session_id.as_deref(),
            Some("native-compact")
        );
        assert!(!stored_run.auto_resume);

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn manual_compaction_returns_before_slow_app_server_finishes() {
        use std::os::unix::fs::PermissionsExt;

        let (mut state, root, project) = temporary_app_state("native-manual-compact-async").await;
        let script = root.join("compact-codex.sh");
        let log = root.join("compact-requests.log");
        let release = root.join("release-compact");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"native-slow-compact\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 while [ ! -f '{}' ]; do sleep 0.05; done\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/completed\",\"params\":{{\"threadId\":\"native-slow-compact\",\"item\":{{\"type\":\"contextCompaction\",\"id\":\"item-compact\"}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
                log.display(),
                release.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");
        state.codex_app_server =
            CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(4));

        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-native-slow-manual-compact".to_string()),
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
            .set_native_session_id(&session.id, "native-slow-compact")
            .await
            .expect("native id");
        state
            .sessions
            .append_message(
                &session.id,
                MessageRole::User,
                "Question before slow compact",
            )
            .await
            .expect("user message");
        state
            .sessions
            .append_message(
                &session.id,
                MessageRole::Assistant,
                "Answer before slow compact",
            )
            .await
            .expect("assistant message");
        state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("idle session");

        let response = timeout(
            Duration::from_secs(5),
            state.compact_session_context(
                "user-1",
                &session.id,
                "request-native-slow-compact",
                None,
            ),
        )
        .await
        .expect("manual compact should return before app-server compaction completes")
        .expect("manual compact");
        assert_eq!(response.state, "starting");
        assert_eq!(
            state
                .storage
                .get_durable_chat_run(&response.response_id)
                .expect("run lookup")
                .expect("run")
                .status,
            "running"
        );

        std::fs::write(&release, b"release").expect("release fake app-server");
        wait_for_context_rollover_state(&state, &response.response_id, "active").await;
        assert_eq!(
            state
                .storage
                .get_durable_chat_run(&response.response_id)
                .expect("run lookup")
                .expect("run")
                .status,
            "completed"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn manual_compaction_rekeys_external_projection_messages_before_replace() {
        use std::os::unix::fs::PermissionsExt;

        let (mut state, root, project) = temporary_app_state("native-manual-compact-rekey").await;
        state.sessions.external_home = Arc::new(root.clone());
        let native_id = "99999999-9999-4999-8999-999999999999";
        let script = root.join("compact-codex.sh");
        let log = root.join("compact-requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"{native_id}\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/completed\",\"params\":{{\"threadId\":\"{native_id}\",\"item\":{{\"type\":\"contextCompaction\",\"id\":\"item-compact\"}}}}}}'\n",
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
        state.codex_app_server =
            CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));

        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/15")
            .join(format!("rollout-2026-08-15T00-00-00-{native_id}.jsonl"));
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
                    "timestamp": now + chrono::Duration::milliseconds(1),
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "Native prompt to compact",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now + chrono::Duration::milliseconds(2),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "id": "native-answer",
                        "role": "assistant",
                        "phase": "final_answer",
                        "content": [{"type": "output_text", "text": "Native answer to keep."}]
                    }
                })
            ),
        )
        .expect("rollout");

        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-native-manual-compact-rekey".to_string()),
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
            .set_native_session_id(&session.id, native_id)
            .await
            .expect("native id");
        let visible_before = state
            .sessions
            .messages_including_external(&session.id)
            .await
            .expect("visible messages before compact");
        let external_user_id = visible_before
            .iter()
            .find(|message| message.content == "Native prompt to compact")
            .map(|message| message.id.clone())
            .expect("external user message");
        assert!(
            visible_before
                .iter()
                .any(|message| message.id == external_user_id),
            "{visible_before:#?}"
        );
        let other = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-with-existing-external-id".to_string()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("other session");
        state
            .storage
            .append_message(
                &other.id,
                &ChatMessage {
                    id: external_user_id.clone(),
                    role: MessageRole::User,
                    content: "Existing materialized native prompt".to_string(),
                    timestamp: now,
                    metadata: Value::Null,
                },
            )
            .expect("colliding message");
        state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("idle session");

        let response = state
            .compact_session_context("user-1", &session.id, "request-native-rekey-compact", None)
            .await
            .expect("manual compact");
        assert_eq!(response.state, "starting");
        wait_for_context_rollover_state(&state, &response.response_id, "active").await;

        let stored = state
            .storage
            .list_messages(&session.id)
            .expect("stored messages after compact");
        let user = stored
            .iter()
            .find(|message| message.content == "Native prompt to compact")
            .expect("materialized user message");
        assert_ne!(user.id, external_user_id);
        assert!(user.id.starts_with("msg_"));
        assert_eq!(
            user.metadata["contextMaterializedFromMessageId"],
            external_user_id
        );
        assert_eq!(user.metadata["usageSourceMessageId"], external_user_id);
        assert!(
            stored
                .iter()
                .any(|message| message.content == "Native answer to keep.")
        );
        assert!(stored.iter().all(|message| {
            !message
                .id
                .starts_with(&format!("external_codex_{native_id}_"))
        }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn active_compacted_session_reloads_post_compact_native_response() {
        let (mut state, root, project) = temporary_app_state("active-compact-native-merge").await;
        state.sessions.external_home = Arc::new(root.clone());
        let native_id = "77777777-7777-4777-8777-777777777777";
        let before_compact = Utc::now() - chrono::Duration::seconds(30);
        let compacted_at = Utc::now() - chrono::Duration::seconds(20);
        let after_compact = Utc::now() - chrono::Duration::seconds(10);
        let rollout = root
            .join(".codex/sessions/2026/08/15")
            .join(format!("rollout-2026-08-15T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        let legacy_transcript = concat!(
            "thinking\nInspecting files\n\n",
            "exec / Parameters\n**Tool:** `command_execution`\n\n",
            "codex\nPost-compact native answer.\n\n",
            "tokens used\n{\"output_tokens\":8}"
        );
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": before_compact,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": before_compact,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "Pre-compact native prompt that should not be imported",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": after_compact,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "Post-compact prompt",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": after_compact + chrono::Duration::milliseconds(1),
                    "type": "response_item",
                    "payload": {
                        "type": "reasoning",
                        "summary": [{"type": "summary_text", "text": "Checking the active compacted thread"}]
                    }
                }),
                serde_json::json!({
                    "timestamp": after_compact + chrono::Duration::milliseconds(2),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "id": "native-final-after-compact",
                        "role": "assistant",
                        "phase": "final_answer",
                        "content": [{"type": "output_text", "text": "Post-compact native answer."}]
                    }
                }),
                serde_json::json!({
                    "timestamp": after_compact + chrono::Duration::milliseconds(3),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "id": "legacy-workbench-transcript",
                        "role": "assistant",
                        "source": "io-workbench",
                        "content": [{"type": "output_text", "text": legacy_transcript}]
                    }
                })
            ),
        )
        .expect("rollout");

        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("active-compacted-workbench-session".to_string()),
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
            .set_native_session_id(&session.id, native_id)
            .await
            .expect("native id");
        state
            .storage
            .append_message(
                &session.id,
                &ChatMessage {
                    id: new_id("msg"),
                    role: MessageRole::User,
                    content: "Pre-compact Workbench prompt".to_string(),
                    timestamp: before_compact,
                    metadata: Value::Null,
                },
            )
            .expect("pre compact user");
        state
            .storage
            .append_message(
                &session.id,
                &ChatMessage {
                    id: new_id("msg"),
                    role: MessageRole::Assistant,
                    content: "Pre-compact Workbench answer".to_string(),
                    timestamp: before_compact + chrono::Duration::milliseconds(1),
                    metadata: Value::Null,
                },
            )
            .expect("pre compact assistant");
        state
            .storage
            .append_message(
                &session.id,
                &ChatMessage {
                    id: new_id("msg"),
                    role: MessageRole::User,
                    content: "Post-compact prompt".to_string(),
                    timestamp: after_compact,
                    metadata: Value::Null,
                },
            )
            .expect("post compact user");

        let mut run = StoredDurableChatRun::new(
            "run-active-compacted-native-merge",
            Some("user-1".to_string()),
            session.id.clone(),
            Provider::Codex.as_str(),
            "Native Codex context compaction".to_string(),
            project.display().to_string(),
        );
        run.native_session_id = Some(native_id.to_string());
        let rollover = StoredSessionContextRollover {
            id: "rollover-active-compacted-native-merge".to_string(),
            user_id: "user-1".to_string(),
            session_id: session.id.clone(),
            request_id: "request-active-compacted-native-merge".to_string(),
            kind: CONTEXT_ROLLOVER_KIND_MANUAL.to_string(),
            failed_message_id: String::new(),
            trigger_run_id: run.id.clone(),
            retry_run_id: run.id.clone(),
            from_native_session_id: Some(native_id.to_string()),
            candidate_native_session_id: Some(native_id.to_string()),
            state: "starting".to_string(),
            handoff: "Native Codex context compaction".to_string(),
            observed_bytes: None,
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: compacted_at,
            updated_at: compacted_at,
            activated_at: None,
        };
        assert!(
            state
                .storage
                .prepare_manual_context_rollover(&rollover, &run)
                .expect("prepare rollover")
        );
        let marker = ChatMessage {
            id: new_id("msg"),
            role: MessageRole::System,
            content: "Context compacted here. Earlier messages remain visible, while subsequent replies use a clean Codex context.".to_string(),
            timestamp: compacted_at,
            metadata: serde_json::json!({"kind": "context_compaction"}),
        };
        let mut stored_session = state
            .sessions
            .get(&session.id)
            .await
            .expect("stored session");
        stored_session.native_session_id = Some(native_id.to_string());
        stored_session.external = false;
        assert!(
            state
                .storage
                .complete_context_rollover(
                    &rollover.id,
                    &run.id,
                    native_id,
                    &stored_session,
                    &marker,
                    None,
                    None,
                )
                .expect("complete rollover")
        );

        let messages = state
            .sessions
            .messages_including_external(&session.id)
            .await
            .expect("messages");
        let contents = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert!(
            contents.contains(&"Pre-compact Workbench prompt"),
            "{contents:#?}"
        );
        assert!(
            contents.contains(&"Pre-compact Workbench answer"),
            "{contents:#?}"
        );
        assert!(contents.contains(&"Post-compact prompt"), "{contents:#?}");
        assert!(
            contents.contains(&"Post-compact native answer."),
            "{contents:#?}"
        );
        assert!(
            contents
                .iter()
                .any(|content| content.starts_with("thinking\n")),
            "{contents:#?}"
        );
        assert!(
            contents
                .iter()
                .all(|content| !content.contains("exec / Parameters")),
            "{contents:#?}"
        );
        assert!(
            contents
                .iter()
                .all(|content| !content.contains("Pre-compact native prompt")),
            "{contents:#?}"
        );
        assert_eq!(
            1,
            contents
                .iter()
                .filter(|content| **content == "Post-compact prompt")
                .count(),
            "{contents:#?}"
        );

        let (tail, total) = state
            .sessions
            .messages_tail_including_external(&session.id, 3)
            .await
            .expect("tail");
        assert_eq!(total, messages.len());
        assert!(
            tail.iter()
                .any(|message| message.content == "Post-compact native answer."),
            "{tail:#?}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

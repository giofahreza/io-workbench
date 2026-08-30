    use super::*;
    use iowb_protocol::AUTO_SESSION_TITLE_MAX_CHARS;
    use tokio::net::TcpListener;
    use tokio::time::{Duration, sleep, timeout};

    async fn temporary_app_state(label: &str) -> (AppState, PathBuf, PathBuf) {
        let root = env::temp_dir().join(format!("iowb-{label}-{}", Uuid::new_v4()));
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
        state
            .storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("test user");
        (state, root, project)
    }

    async fn wait_for_context_rollover_state(
        state: &AppState,
        retry_run_id: &str,
        expected_state: &str,
    ) -> StoredSessionContextRollover {
        timeout(Duration::from_secs(3), async {
            loop {
                if let Some(rollover) = state
                    .storage
                    .context_rollover_for_retry_run(retry_run_id)
                    .expect("rollover lookup")
                    && rollover.state == expected_state
                {
                    return rollover;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!("context rollover did not reach state {expected_state} for {retry_run_id}")
        })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn edit_from_here_first_prompt_creates_empty_fork_with_unsent_draft() {
        let (state, root, project) = temporary_app_state("fork-first-prompt").await;
        let source = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                None,
                false,
                Some("gpt-5.4".to_string()),
                Some(ChatRuntime::NativeCli),
                Some("high".to_string()),
                Some("default".to_string()),
                Some(true),
                None,
            )
            .await
            .expect("source session");
        let target = state
            .sessions
            .append_message(
                &source.id,
                MessageRole::User,
                "Rewrite the authentication flow",
            )
            .await
            .expect("target prompt");
        state
            .sessions
            .append_message(&source.id, MessageRole::Assistant, "Original answer")
            .await
            .expect("later answer");
        state
            .sessions
            .set_active(&source.id, false)
            .await
            .expect("source inactive");

        let response = state
            .fork_session_before_message(
                "user-1",
                &source.id,
                &target.id,
                "request-first",
                true,
                Some("Rewrite authentication with passkeys"),
            )
            .await
            .expect("fork first prompt");
        assert_eq!(response.source_session_id, source.id);
        assert_eq!(response.before_message_id, target.id);
        assert_eq!(
            response.session.title,
            "Rewrite authentication with passkeys"
        );
        assert_eq!(response.session.message_count, 0);
        assert_eq!(response.session.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(response.session.effort.as_deref(), Some("high"));
        assert_eq!(response.session.thinking, Some(true));
        assert_eq!(
            response.draft.content,
            "Rewrite authentication with passkeys"
        );
        assert!(!response.native_forked);
        assert!(response.files_unchanged);
        assert!(response.source_hidden);
        assert!(
            state
                .sessions
                .messages(&response.session.id)
                .expect("destination messages")
                .is_empty()
        );
        assert_eq!(
            state
                .sessions
                .messages(&source.id)
                .expect("source messages")
                .len(),
            2
        );

        let retry = state
            .fork_session_before_message(
                "user-1",
                &source.id,
                "different-message-id",
                "request-first",
                false,
                Some("This retry must not replace the original draft"),
            )
            .await
            .expect("idempotent retry");
        assert_eq!(retry.session.id, response.session.id);
        assert_eq!(retry.before_message_id, target.id);
        assert_eq!(retry.draft.content, response.draft.content);
        assert!(retry.source_hidden);
        let listed = state
            .sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("replacement list");
        assert!(listed.iter().all(|session| session.id != source.id));
        assert!(
            listed
                .iter()
                .any(|session| session.id == response.session.id)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn board_scope_survives_cache_miss_and_propagates_to_fork() {
        let (state, root, project) = temporary_app_state("board-scope-continuation").await;
        let source = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("board-chat".to_string()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("source session");
        let source = state
            .sessions
            .mark_board_session(&source.id, "board-1", Some("task-1".to_string()))
            .await
            .expect("mark board session");
        let target = state
            .sessions
            .append_message(&source.id, MessageRole::User, "board prompt")
            .await
            .expect("target prompt");
        state
            .sessions
            .set_active(&source.id, false)
            .await
            .expect("source inactive");

        // A fresh manager models restart/eviction. Continuation must seed the
        // cached entry from storage instead of replacing its scope metadata.
        let reloaded = SessionManager::load(state.storage.clone(), 0).expect("reload manager");
        assert!(reloaded.is_board_session_cached(&source.id));
        let continued = reloaded
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some(source.id.clone()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("continue board session");
        assert!(continued.board_session);
        assert_eq!(continued.board_id.as_deref(), Some("board-1"));
        assert_eq!(continued.board_task_id.as_deref(), Some("task-1"));
        assert!(reloaded.list_active().await.is_empty());

        reloaded
            .set_active(&source.id, false)
            .await
            .expect("continued source inactive");
        let fork = state
            .fork_session_before_message(
                "user-1",
                &source.id,
                &target.id,
                "board-fork",
                false,
                None,
            )
            .await
            .expect("fork board session");
        assert!(fork.session.board_session);
        assert_eq!(fork.session.board_id.as_deref(), Some("board-1"));
        assert_eq!(fork.session.board_task_id.as_deref(), Some("task-1"));
        assert!(!fork.source_hidden);
        assert!(
            state
                .sessions
                .list_for_project(project.to_str().expect("project path"))
                .await
                .expect("project sessions")
                .iter()
                .all(|session| session.id != source.id && session.id != fork.session.id)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn edit_from_here_direct_ai_fork_clones_only_prior_messages_with_provenance() {
        let (state, root, project) = temporary_app_state("fork-direct-ai").await;
        let source = state
            .sessions
            .create_or_update(
                Provider::Gemini,
                project.display().to_string(),
                None,
                false,
                Some("gem:gemini-2.5-pro".to_string()),
                Some(ChatRuntime::IoGateway),
                Some("medium".to_string()),
                Some("default".to_string()),
                Some(false),
                None,
            )
            .await
            .expect("source session");
        let first_user = state
            .sessions
            .append_message(&source.id, MessageRole::User, "same prompt")
            .await
            .expect("first prompt");
        let first_assistant = state
            .sessions
            .append_message(&source.id, MessageRole::Assistant, "first answer")
            .await
            .expect("first answer");
        let target = state
            .sessions
            .append_message(&source.id, MessageRole::User, "same prompt")
            .await
            .expect("target prompt");
        state
            .sessions
            .append_message(&source.id, MessageRole::Assistant, "later answer")
            .await
            .expect("later answer");
        state
            .sessions
            .set_active(&source.id, false)
            .await
            .expect("source inactive");

        let response = state
            .fork_session_before_message(
                "user-1",
                &source.id,
                &target.id,
                "request-middle",
                false,
                None,
            )
            .await
            .expect("fork middle prompt");
        let cloned = state
            .sessions
            .messages(&response.session.id)
            .expect("cloned messages");
        assert_eq!(cloned.len(), 2);
        assert_eq!(cloned[0].role, MessageRole::User);
        assert_eq!(cloned[0].content, "same prompt");
        assert_eq!(cloned[1].role, MessageRole::Assistant);
        assert_eq!(cloned[1].content, "first answer");
        assert_ne!(cloned[0].id, first_user.id);
        assert_ne!(cloned[1].id, first_assistant.id);
        assert_eq!(cloned[0].metadata["forkedFromSessionId"], source.id);
        assert_eq!(cloned[0].metadata["forkedFromMessageId"], first_user.id);
        assert_eq!(
            cloned[1].metadata["forkedFromMessageId"],
            first_assistant.id
        );
        assert_eq!(response.session.message_count, 2);
        assert_eq!(response.draft.content, "same prompt");
        assert!(!response.native_forked);
        assert!(!response.source_hidden);
        assert_eq!(
            state
                .sessions
                .messages(&source.id)
                .expect("original messages")
                .len(),
            4
        );
        let listed = state
            .sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("non-replacement list");
        assert!(listed.iter().any(|session| session.id == source.id));
        assert!(
            listed
                .iter()
                .any(|session| session.id == response.session.id)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replacing_external_session_hides_and_delete_restores_source() {
        let (mut state, root, project) = temporary_app_state("fork-external-source").await;
        let native_id = "55555555-5555-4555-8555-555555555555";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/13")
            .join(format!("rollout-2026-08-13T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n",
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
                        "message": "replace external prompt",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now + chrono::Duration::seconds(1),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "external answer"}]
                    }
                })
            ),
        )
        .expect("rollout");
        state.sessions.external_home = Arc::new(root.clone());

        let source = state
            .sessions
            .get(native_id)
            .await
            .expect("external source");
        assert!(source.external);
        let target = state
            .sessions
            .messages_including_external(native_id)
            .await
            .expect("external messages")
            .into_iter()
            .find(|message| message.role == MessageRole::User)
            .expect("external user prompt");

        let response = state
            .fork_session_before_message(
                "user-1",
                native_id,
                &target.id,
                "request-external-replace",
                true,
                Some("edited external prompt"),
            )
            .await
            .expect("replace external source");
        assert!(response.source_hidden);
        let hidden = state
            .sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("hidden source list");
        assert!(hidden.iter().all(|session| session.id != native_id));
        assert!(
            hidden
                .iter()
                .any(|session| session.id == response.session.id)
        );

        state
            .sessions
            .delete(&response.session.id)
            .await
            .expect("delete replacement");
        let restored = state
            .sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("restored source list");
        assert!(restored.iter().any(|session| session.id == native_id));
        assert!(
            restored
                .iter()
                .all(|session| session.id != response.session.id)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn codex_fork_boundary_resolves_duplicate_prompts_in_order() {
        let (state, root, _) = temporary_app_state("fork-boundary").await;
        let now = Utc::now();
        let messages = vec![
            ChatMessage {
                id: "local-user-1".to_string(),
                role: MessageRole::User,
                content: "repeat".to_string(),
                timestamp: now,
                metadata: Value::Null,
            },
            ChatMessage {
                id: "local-assistant-1".to_string(),
                role: MessageRole::Assistant,
                content: "answer".to_string(),
                timestamp: now,
                metadata: Value::Null,
            },
            ChatMessage {
                id: "local-user-2".to_string(),
                role: MessageRole::User,
                content: "repeat".to_string(),
                timestamp: now,
                metadata: Value::Null,
            },
        ];
        let snapshot = CodexThreadSnapshot {
            id: "thread-1".to_string(),
            turns: vec![
                codex_app_server::CodexThreadTurn {
                    id: "turn-1".to_string(),
                    status: "failed".to_string(),
                    user_item_ids: vec!["native-user-1".to_string()],
                    user_text: "repeat".to_string(),
                },
                codex_app_server::CodexThreadTurn {
                    id: "turn-2".to_string(),
                    status: "completed".to_string(),
                    user_item_ids: vec!["native-user-2".to_string()],
                    user_text: "repeat".to_string(),
                },
            ],
        };

        assert_eq!(
            state
                .resolve_codex_fork_boundary(
                    "session-without-durable-runs",
                    &messages[2],
                    &messages,
                    &snapshot,
                )
                .expect("duplicate prompt boundary"),
            "turn-1"
        );

        let target_with_metadata = ChatMessage {
            metadata: serde_json::json!({"nativeBeforeTurnId": "turn-failed"}),
            ..messages[2].clone()
        };
        assert_eq!(
            state
                .resolve_codex_fork_boundary(
                    "session-without-durable-runs",
                    &target_with_metadata,
                    &messages,
                    &snapshot,
                )
                .expect("metadata boundary"),
            "turn-failed"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn abort_terminates_descendants_that_keep_agent_output_open() {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "(trap '' TERM; while :; do sleep 60; done) & echo ready; wait",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        isolate_agent_process(&mut command);
        let mut child = command.spawn().expect("spawn launcher");
        let mut stdout = child.stdout.take().expect("launcher stdout");
        let mut ready = [0_u8; 6];
        timeout(Duration::from_secs(1), stdout.read_exact(&mut ready))
            .await
            .expect("descendant startup timed out")
            .expect("read descendant startup marker");
        assert_eq!(&ready, b"ready\n");

        let output_closed = tokio::spawn(async move {
            let mut remainder = Vec::new();
            stdout.read_to_end(&mut remainder).await
        });
        terminate_agent_process_tree(&mut child, "process-tree-test").await;

        timeout(Duration::from_secs(1), output_closed)
            .await
            .expect("descendant retained the output pipe")
            .expect("output reader task")
            .expect("read output to EOF");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn aborted_output_drain_does_not_wait_for_lingering_sender() {
        let (_sender, mut receiver) = mpsc::channel(1);

        timeout(
            Duration::from_secs(1),
            drain_aborted_agent_output(&mut receiver),
        )
        .await
        .expect("abort drain waited for an open sender");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    async fn startup_cleanup_is_scoped_to_database_and_dead_owner() {
        let run_id = format!("run-orphan-test-{}", Uuid::new_v4());
        let root = env::temp_dir().join(format!("iowb-orphan-test-{}", Uuid::new_v4()));
        let original_database = root.join("original/io-workbench.db");
        let copied_database = root.join("copy/io-workbench.db");
        std::fs::create_dir_all(original_database.parent().expect("original parent"))
            .expect("create original parent");
        std::fs::create_dir_all(copied_database.parent().expect("copy parent"))
            .expect("create copy parent");
        std::fs::write(&original_database, []).expect("create original database");
        std::fs::write(&copied_database, []).expect("create copied database");
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 60 </dev/null >/dev/null 2>&1 & echo $!"])
            .env(DURABLE_AGENT_RUN_ENV, &run_id)
            .env(
                DURABLE_AGENT_SCOPE_ENV,
                durable_agent_run_scope(&original_database),
            )
            .env(DURABLE_AGENT_OWNER_PID_ENV, "2147483647")
            .env(DURABLE_AGENT_OWNER_START_ENV, "1")
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let output = command.output().await.expect("spawn marked orphan");
        assert!(output.status.success());
        let orphan_pid = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<libc::pid_t>()
            .expect("orphan pid");

        assert_eq!(
            terminate_orphaned_agent_run_processes(&run_id, &copied_database),
            OrphanedAgentRunCleanup::default()
        );
        assert!(std::fs::metadata(format!("/proc/{orphan_pid}")).is_ok());
        assert_eq!(
            terminate_orphaned_agent_run_processes(&run_id, &original_database),
            OrphanedAgentRunCleanup {
                terminated_process_groups: 1,
                live_owner: false,
            }
        );
        timeout(Duration::from_secs(1), async {
            loop {
                let state = std::fs::read_to_string(format!("/proc/{orphan_pid}/stat"))
                    .ok()
                    .and_then(|stat| {
                        stat.rsplit_once(')')
                            .and_then(|(_, fields)| fields.split_whitespace().next())
                            .and_then(|state| state.chars().next())
                    });
                if state.is_none_or(|state| state == 'Z') {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("marked orphan was not killed");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    async fn startup_cleanup_preserves_process_with_live_owner() {
        let run_id = format!("run-live-owner-test-{}", Uuid::new_v4());
        let database = env::temp_dir().join(format!("iowb-live-owner-{}.db", Uuid::new_v4()));
        std::fs::write(&database, []).expect("create database");
        let (owner_pid, owner_start) = current_process_identity().expect("test process identity");
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 60"])
            .env(DURABLE_AGENT_RUN_ENV, &run_id)
            .env(DURABLE_AGENT_SCOPE_ENV, durable_agent_run_scope(&database))
            .env(DURABLE_AGENT_OWNER_PID_ENV, owner_pid.to_string())
            .env(DURABLE_AGENT_OWNER_START_ENV, owner_start.to_string())
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().expect("spawn live-owned process");

        sleep(Duration::from_millis(25)).await;
        assert_eq!(
            terminate_orphaned_agent_run_processes(&run_id, &database),
            OrphanedAgentRunCleanup {
                terminated_process_groups: 0,
                live_owner: true,
            }
        );
        assert!(child.try_wait().expect("read child state").is_none());

        terminate_agent_process_tree(&mut child, "live-owner-test").await;
        let _ = std::fs::remove_file(database);
    }

    #[test]
    fn summary_truncates_long_prompts() {
        let prompt = "a".repeat(AUTO_SESSION_TITLE_MAX_CHARS + 30);
        assert_eq!(
            session_title_from_prompt(&prompt),
            Some(format!("{}...", "a".repeat(AUTO_SESSION_TITLE_MAX_CHARS)))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn latest_user_prompt_updates_auto_title_but_manual_title_stays_locked() {
        let root = env::temp_dir().join(format!("iowb-session-title-{}", Uuid::new_v4()));
        let storage = Storage::open(root.join("test.db")).expect("storage");
        let sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        let session = sessions
            .create_or_update(
                Provider::Codex,
                root.display().to_string(),
                Some("session-title-test".to_string()),
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

        sessions
            .append_message(
                &session.id,
                MessageRole::User,
                "  First title\n\nwith spacing  ",
            )
            .await
            .expect("first prompt");
        assert_eq!(
            sessions.get(&session.id).await.expect("first title").title,
            "First title with spacing"
        );

        sessions
            .append_message(&session.id, MessageRole::Assistant, "assistant reply")
            .await
            .expect("assistant reply");
        assert_eq!(
            sessions
                .get(&session.id)
                .await
                .expect("assistant keeps title")
                .title,
            "First title with spacing"
        );

        sessions
            .append_message(&session.id, MessageRole::User, "latest prompt")
            .await
            .expect("latest prompt");
        let automatic = sessions.get(&session.id).await.expect("automatic title");
        assert_eq!(automatic.title, "latest prompt");
        assert_eq!(automatic.title_source, Some(SessionTitleSource::Prompt));

        sessions
            .rename(&session.id, "Manual investigation".to_string())
            .await
            .expect("manual rename");
        sessions
            .append_message(&session.id, MessageRole::User, "do not replace manual")
            .await
            .expect("prompt after manual rename");
        let manual = sessions.get(&session.id).await.expect("manual title");
        assert_eq!(manual.title, "Manual investigation");
        assert_eq!(manual.title_source, Some(SessionTitleSource::Manual));

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn external_refresh_preserves_workbench_prompt_title() {
        let root = env::temp_dir().join(format!("iowb-external-title-{}", Uuid::new_v4()));
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("project");
        let storage = Storage::open(root.join("test.db")).expect("storage");
        storage
            .upsert_session(&SessionSummary {
                id: "external-title-session".to_string(),
                provider: Provider::Codex,
                external: true,
                project_path: project.display().to_string(),
                title: "latest Workbench prompt".to_string(),
                last_activity: Utc::now(),
                title_source: Some(SessionTitleSource::Prompt),
                ..Default::default()
            })
            .expect("stored external session");

        let sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        {
            let mut cache = sessions.external_cache.write().await;
            cache.loaded_at = Some(Instant::now());
            cache.records = vec![ExternalSessionRecord {
                summary: SessionSummary {
                    id: "external-title-session".to_string(),
                    provider: Provider::Codex,
                    external: true,
                    project_path: project.display().to_string(),
                    title: "provider first prompt".to_string(),
                    last_activity: Utc::now(),
                    title_source: Some(SessionTitleSource::External),
                    ..Default::default()
                },
                file_path: root.join("missing-rollout.jsonl"),
            }];
        }

        let listed = sessions
            .list_for_project(&project.display().to_string())
            .await
            .expect("project sessions");
        let session = listed
            .into_iter()
            .find(|session| session.id == "external-title-session")
            .expect("external session");
        assert_eq!(session.title, "latest Workbench prompt");
        assert_eq!(session.title_source, Some(SessionTitleSource::Prompt));

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn direct_ai_history_filters_and_normalizes_stored_messages() {
        let now = Utc::now();
        let message = |role, content: &str| ChatMessage {
            id: new_id("msg"),
            role,
            content: content.to_string(),
            timestamp: now,
            metadata: Value::Null,
        };
        let history = direct_ai_conversation_messages(
            vec![
                message(MessageRole::Assistant, "orphaned assistant"),
                message(MessageRole::System, "internal status"),
                message(MessageRole::User, "first question"),
                message(MessageRole::Tool, "tool output"),
                message(MessageRole::User, "follow-up detail"),
                message(MessageRole::Assistant, "earlier answer"),
                message(MessageRole::User, "current question"),
            ],
            "current question",
        );

        assert_eq!(
            history,
            vec![
                DirectAiConversationMessage {
                    role: "user",
                    content: "first question\n\nfollow-up detail".to_string(),
                },
                DirectAiConversationMessage {
                    role: "assistant",
                    content: "earlier answer".to_string(),
                },
                DirectAiConversationMessage {
                    role: "user",
                    content: "current question".to_string(),
                },
            ]
        );
    }

    #[test]
    fn direct_ai_history_is_bounded_and_keeps_current_prompt() {
        let now = Utc::now();
        let mut messages = (0..80)
            .map(|index| ChatMessage {
                id: format!("msg-{index:03}"),
                role: if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                content: format!("message-{index}"),
                timestamp: now,
                metadata: Value::Null,
            })
            .collect::<Vec<_>>();
        messages.push(ChatMessage {
            id: "msg-current".to_string(),
            role: MessageRole::User,
            content: "current question".to_string(),
            timestamp: now,
            metadata: Value::Null,
        });

        let bounded = direct_ai_conversation_messages(messages, "current question");
        assert!(bounded.len() <= DIRECT_AI_HISTORY_MAX_MESSAGES);
        assert_eq!(bounded.first().map(|message| message.role), Some("user"));
        assert_eq!(
            bounded.last(),
            Some(&DirectAiConversationMessage {
                role: "user",
                content: "current question".to_string(),
            })
        );
        assert!(bounded.iter().all(|message| message.content != "message-0"));

        let oversized_old_message = ChatMessage {
            id: "msg-oversized".to_string(),
            role: MessageRole::User,
            content: "x".repeat(DIRECT_AI_HISTORY_MAX_BYTES),
            timestamp: now,
            metadata: Value::Null,
        };
        let current = ChatMessage {
            id: "msg-latest".to_string(),
            role: MessageRole::User,
            content: "latest prompt".to_string(),
            timestamp: now,
            metadata: Value::Null,
        };
        let bounded_by_bytes =
            direct_ai_conversation_messages(vec![oversized_old_message, current], "latest prompt");
        assert_eq!(
            bounded_by_bytes,
            vec![DirectAiConversationMessage {
                role: "user",
                content: "latest prompt".to_string(),
            }]
        );
    }

    #[test]
    fn normalizes_split_codex_live_tool_and_file_events() {
        let mut normalizer = CodexLiveOutputNormalizer::default();
        let first = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"22222222-2222-4222-8222-222222222222\"}\n",
            "{\"type\":\"turn.started\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"reason-1\",\"type\":\"reasoning\",",
            "\"text\":\"Inspecting files\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"command-1\",\"type\":\"command_execution\",",
            "\"command\":\"pwd\",\"aggregated_output\":\"/tmp/project\\n\",\"exit_code\":0,",
            "\"status\":\"completed\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"change-1\",\"type\":\"file_change\",",
            "\"changes\":[{\"path\":\"created.txt\",\"kind\":\"add\"},",
            "{\"path\":\"updated.txt\",\"kind\":\"update\"},",
            "{\"path\":\"deleted.txt\",\"kind\":\"delete\"},",
            "{\"path\":\"moved.txt\",\"kind\":\"move\"}],\"status\":\"completed\"}}\n"
        );
        let split = first.len() / 2;
        let mut output = normalizer.push(&first[..split]);
        output.push_str(&normalizer.push(&first[split..]));
        output.push_str(&normalizer.finish());

        assert!(output.contains("thinking\nInspecting files"), "{output}");
        assert!(output.contains("exec / Parameters"), "{output}");
        assert!(output.contains("### Command\n```sh\npwd"), "{output}");
        assert!(output.contains("exec / Details"), "{output}");
        assert!(output.contains("create / created.txt"), "{output}");
        assert!(output.contains("edit / updated.txt"), "{output}");
        assert!(output.contains("delete / deleted.txt"), "{output}");
        assert!(output.contains("move / moved.txt"), "{output}");
        assert!(!output.contains("turn.started"), "{output}");
        assert_eq!(
            normalizer.take_thread_id().as_deref(),
            Some("22222222-2222-4222-8222-222222222222")
        );
        assert!(normalizer.take_thread_id().is_none());
    }

    #[test]
    fn codex_app_server_prompt_input_converts_safe_image_markers() {
        let root = env::temp_dir().join(format!("iowb-app-server-input-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("uploads")).expect("temp dir");
        let image = root.join("uploads/screenshot.png");
        std::fs::write(&image, b"png").expect("image");

        let input = codex_app_server_prompt_input(
            "What is this?\nAttached image file: `uploads/screenshot.png` (screenshot.png, image/png)",
            &root,
        );

        assert_eq!(input.len(), 2);
        assert_eq!(
            input[0],
            serde_json::json!({"type": "text", "text": "What is this?"})
        );
        assert_eq!(
            input[1].get("type").and_then(Value::as_str),
            Some("localImage")
        );
        assert_eq!(
            input[1].get("path").and_then(Value::as_str),
            Some(
                std::fs::canonicalize(&image)
                    .expect("canonical image")
                    .to_str()
                    .unwrap()
            )
        );

        let unsafe_input = codex_app_server_prompt_input(
            "Attached image file: `../outside.png` (outside.png, image/png)",
            &root,
        );
        assert_eq!(unsafe_input.len(), 1);
        assert_eq!(
            unsafe_input[0].get("type").and_then(Value::as_str),
            Some("text")
        );
        assert!(
            unsafe_input[0]
                .get("text")
                .and_then(Value::as_str)
                .unwrap()
                .contains("../outside.png")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_app_server_prompt_input_handles_multiple_and_image_only_markers() {
        let root = env::temp_dir().join(format!("iowb-app-server-input-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("uploads")).expect("temp dir");
        let first = root.join("uploads/first.png");
        let second = root.join("uploads/second.jpg");
        std::fs::write(&first, b"png").expect("first image");
        std::fs::write(&second, b"jpg").expect("second image");

        let input = codex_app_server_prompt_input(
            "Compare these\nAttached image file: `uploads/first.png` (first.png, image/png)\nAttached image file: `uploads/second.jpg` (second.jpg, image/jpeg)",
            &root,
        );
        assert_eq!(input.len(), 3);
        assert_eq!(
            input[0],
            serde_json::json!({"type": "text", "text": "Compare these"})
        );
        assert_eq!(
            input[1].get("type").and_then(Value::as_str),
            Some("localImage")
        );
        assert_eq!(
            input[2].get("type").and_then(Value::as_str),
            Some("localImage")
        );
        assert_eq!(
            input[1].get("path").and_then(Value::as_str),
            Some(
                std::fs::canonicalize(&first)
                    .expect("first canonical")
                    .to_str()
                    .unwrap()
            )
        );
        assert_eq!(
            input[2].get("path").and_then(Value::as_str),
            Some(
                std::fs::canonicalize(&second)
                    .expect("second canonical")
                    .to_str()
                    .unwrap()
            )
        );

        let image_only = codex_app_server_prompt_input(
            "Attached image file: `uploads/first.png` (first.png, image/png)",
            &root,
        );
        assert_eq!(image_only.len(), 1);
        assert_eq!(
            image_only[0].get("type").and_then(Value::as_str),
            Some("localImage")
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_app_server_prompt_input_keeps_prompt_only_and_missing_images_as_text() {
        let root = env::temp_dir().join(format!("iowb-app-server-input-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");

        let prompt_only = codex_app_server_prompt_input("Just text", &root);
        assert_eq!(
            prompt_only,
            vec![serde_json::json!({"type": "text", "text": "Just text"})]
        );

        let missing = codex_app_server_prompt_input(
            "Attached image file: `missing.png` (missing.png, image/png)",
            &root,
        );
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].get("type").and_then(Value::as_str), Some("text"));
        assert!(
            missing[0]
                .get("text")
                .and_then(Value::as_str)
                .unwrap()
                .contains("missing.png")
        );

        let _ = std::fs::remove_dir_all(root);
    }

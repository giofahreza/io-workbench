    use super::*;
    use iowb_core::AppConfig;
    use iowb_protocol::ChatMessage;

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());
    static RAG_TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct TestEnvGuard {
        previous: Vec<(&'static str, Option<String>)>,
    }

    impl TestEnvGuard {
        fn set(changes: Vec<(&'static str, Option<String>)>) -> Self {
            let previous = changes
                .iter()
                .map(|(key, _)| (*key, std::env::var(key).ok()))
                .collect::<Vec<_>>();
            unsafe {
                for (key, value) in changes {
                    if let Some(value) = value {
                        std::env::set_var(key, value);
                    } else {
                        std::env::remove_var(key);
                    }
                }
            }
            Self { previous }
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            unsafe {
                for (key, value) in &self.previous {
                    if let Some(value) = value {
                        std::env::set_var(key, value);
                    } else {
                        std::env::remove_var(key);
                    }
                }
            }
        }
    }

    fn board_fixture(value: Value) -> AgenticBoard {
        let request = serde_json::from_value::<CreateBoardRequest>(value).unwrap();
        AgenticBoard::new(None, request).unwrap()
    }

    fn native_rag_plugin_path_for_test() -> Option<PathBuf> {
        if let Ok(path) = std::env::var("IO_WORKBENCH_RAG_PLUGIN")
            && !path.trim().is_empty()
        {
            return Some(PathBuf::from(path));
        }
        if let Ok(path) = std::env::var("IO_WORKBENCH_RAG_PLUGIN_PATH")
            && !path.trim().is_empty()
        {
            return Some(PathBuf::from(path));
        }

        #[cfg(target_os = "windows")]
        const LIB_NAME: &str = "iowb_rag_native.dll";
        #[cfg(target_os = "macos")]
        const LIB_NAME: &str = "libiowb_rag_native.dylib";
        #[cfg(all(unix, not(target_os = "macos")))]
        const LIB_NAME: &str = "libiowb_rag_native.so";

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or(manifest_dir);
        let target_dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root.join("target"));
        let path = target_dir.join("debug").join(LIB_NAME);
        if path.exists() {
            Some(path)
        } else {
            eprintln!(
                "skipping native RAG Kanban test; build the plugin first with `cargo build -p iowb-rag-native` or set IO_WORKBENCH_RAG_PLUGIN"
            );
            None
        }
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires built native RAG plugin and FastEmbed model availability"]
    async fn kanban_board_attaches_context_from_native_rag_plugin() {
        let _env_lock = RAG_TEST_ENV_LOCK.lock().expect("RAG test env lock");
        let Some(plugin_path) = native_rag_plugin_path_for_test() else {
            return;
        };
        let root = std::env::temp_dir().join(format!("iowb-kanban-rag-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(project.join("src")).expect("project source directory");
        fs::write(
            project.join("src/auth.rs"),
            r#"
pub struct SessionTokenStore;

impl SessionTokenStore {
    pub fn rotate_refresh_token(&self, csrf_nonce: &str) -> bool {
        csrf_nonce == "kanban-rag-csrf-nonce"
    }
}

pub const KANBAN_RAG_SENTINEL: &str = "SessionTokenStore validates refresh token rotation with csrf nonce";
"#,
        )
        .expect("write source fixture");

        let _env = TestEnvGuard::set(vec![
            ("IO_WORKBENCH_RAG_MODE", Some("native-plugin".to_string())),
            (
                "IO_WORKBENCH_RAG_PLUGIN",
                Some(plugin_path.display().to_string()),
            ),
            (
                "IOWB_RAG_STORAGE_DIR",
                Some(root.join("rag-store").display().to_string()),
            ),
            (
                "IOWB_RAG_FASTEMBED_CACHE_DIR",
                Some(root.join("fastembed-cache").display().to_string()),
            ),
            ("IOWB_RAG_EMBEDDING_MODEL", Some("bge-small".to_string())),
            ("IOWB_RAG_DENSE_WEIGHT", Some("0.60".to_string())),
            ("IOWB_RAG_BM25_WEIGHT", Some("0.40".to_string())),
        ]);

        let mut run = board_fixture(json!({
            "command": "Implement SessionTokenStore refresh token rotation",
            "projectPath": project.display().to_string(),
            "projectName": format!("kanban-rag-{}", Uuid::new_v4()),
            "provider": "codex",
            "title": "Wire SessionTokenStore token rotation",
            "details": "Use the existing csrf nonce refresh token rotation code."
        }));
        if let Some(task) = run.tasks.get_mut(0) {
            task.acceptance_criteria = vec![
                "Reuse SessionTokenStore for refresh token rotation.".to_string(),
                "Preserve csrf nonce validation behavior.".to_string(),
            ];
        }

        index_project_for_rag(&mut run).await;

        assert!(run.rag_enabled);
        let ingestion = run
            .rag_ingestions
            .iter()
            .find(|value| value.get("kind").and_then(Value::as_str) == Some("project_index"))
            .expect("project index ingestion recorded");
        assert_eq!(
            ingestion.get("ok").and_then(Value::as_bool),
            Some(true),
            "project index failed: {ingestion}"
        );
        assert!(
            ingestion
                .pointer("/response/chunksIndexed")
                .and_then(Value::as_u64)
                .unwrap_or_default()
                > 0,
            "project index did not ingest chunks: {ingestion}"
        );

        attach_rag_context_for_task(&mut run, 0).await;

        let query = run.rag_queries.last().expect("RAG query recorded");
        assert_eq!(
            query.get("ok").and_then(Value::as_bool),
            Some(true),
            "RAG query failed: {query}"
        );
        let task = &run.tasks[0];
        assert!(
            !task.rag_context_refs.is_empty(),
            "RAG query returned no context refs: {query}"
        );
        assert!(
            task.rag_prompt_context.contains("SessionTokenStore")
                || task.rag_prompt_context.contains("KANBAN_RAG_SENTINEL"),
            "RAG prompt context did not include the indexed auth source: {}",
            task.rag_prompt_context
        );
        assert!(
            task.rag_prompt_context.contains("src/auth.rs"),
            "RAG prompt context did not include the source path: {}",
            task.rag_prompt_context
        );

        drop(_env);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn startup_backfill_hides_legacy_board_session_before_discovery() {
        let root =
            std::env::temp_dir().join(format!("iowb-server-board-backfill-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        fs::create_dir_all(&project).expect("project directory");
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

        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": project.display().to_string(),
            "provider": "codex"
        }));
        run.id = "legacy-board".to_string();
        run.user_id = Some("user-1".to_string());
        run.tasks[0].provider_session_id = Some("legacy-board-chat".to_string());
        save_board(&state, &run).expect("persist legacy board");

        let session = SessionSummary {
            id: "legacy-board-chat".to_string(),
            provider: Provider::Codex,
            project_path: project.display().to_string(),
            title: "Legacy board chat".to_string(),
            last_activity: Utc::now(),
            ..Default::default()
        };
        state
            .storage
            .upsert_session(&session)
            .expect("persist session");
        state
            .storage
            .append_message(
                &session.id,
                &ChatMessage {
                    id: "message-1".to_string(),
                    role: MessageRole::User,
                    content: format!("Board id: {}\nTask 1: {}", run.id, run.tasks[0].id),
                    timestamp: Utc::now(),
                    metadata: Value::Null,
                },
            )
            .expect("persist board prompt");

        assert_eq!(
            state
                .storage
                .list_sessions()
                .expect("pre-backfill list")
                .len(),
            1
        );
        backfill_legacy_board_sessions(&state)
            .await
            .expect("backfill succeeds");

        let classified = state
            .storage
            .get_session(&session.id)
            .expect("read session")
            .expect("session exists");
        assert!(classified.board_session);
        assert_eq!(classified.board_id.as_deref(), Some("legacy-board"));
        assert_eq!(
            classified.board_task_id.as_deref(),
            Some(run.tasks[0].id.as_str())
        );
        assert!(
            state
                .storage
                .list_sessions()
                .expect("post-backfill list")
                .is_empty()
        );
        assert!(
            state
                .sessions
                .list_for_project(&session.project_path)
                .await
                .unwrap()
                .is_empty()
        );

        drop(state);
        fs::remove_dir_all(root).expect("cleanup");
    }

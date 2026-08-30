impl AppState {
    pub async fn initialize(config: AppConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.config_dir)?;
        std::fs::create_dir_all(&config.workspace_root)?;
        let storage = Storage::open(&config.database_path)?;
        let files = FileService::new(config.max_scan_depth, config.max_file_read_bytes);
        let path_validator = WorkspacePathValidator::new(config.workspace_root.clone());
        let config = Arc::new(config);

        let sessions = SessionManager::load(storage.clone(), config.max_sessions)?;
        let state = Self {
            config: Arc::clone(&config),
            storage: storage.clone(),
            auth: AuthManager::new(Arc::clone(&config), storage.clone()),
            sessions,
            projects: ProjectIndex::new(storage),
            agents: AgentRuntimeManager::new(config.max_sessions),
            tasks: TaskManager::new(),
            processes: ProcessManager::new(),
            files,
            path_validator,
            watch: WatchManager::new(),
            ws_hub: WsHub::new(),
            codex_app_server: default_codex_app_server_client(),
            codex_app_server_mutation: Arc::new(Mutex::new(())),
        };

        info!(
            product = PRODUCT_NAME,
            config_dir = %state.config.config_dir.display(),
            database = %state.config.database_path.display(),
            "initialized app state"
        );
        Ok(state)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_agent_session(
        &self,
        provider: Provider,
        project_path: impl Into<String>,
        prompt: impl Into<String>,
        session_id: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<String>,
        thinking: Option<bool>,
        fast: Option<bool>,
        runtime: ChatRuntime,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
        user_id: Option<String>,
    ) -> Result<SessionSummary> {
        self.start_agent_session_scoped(
            provider,
            project_path,
            prompt,
            session_id,
            model,
            effort,
            mode,
            thinking,
            fast,
            runtime,
            direct_ai_config,
            user_id,
            None,
        )
        .await
    }

    /// Start a session owned by an agentic board. The scope is persisted
    /// before the provider starts and before any active-session broadcast, so
    /// the board chat can never briefly leak into ordinary chat discovery.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_board_agent_session(
        &self,
        provider: Provider,
        project_path: impl Into<String>,
        prompt: impl Into<String>,
        session_id: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<String>,
        thinking: Option<bool>,
        fast: Option<bool>,
        runtime: ChatRuntime,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
        user_id: Option<String>,
        board_id: impl Into<String>,
        board_task_id: Option<String>,
    ) -> Result<SessionSummary> {
        self.start_agent_session_scoped(
            provider,
            project_path,
            prompt,
            session_id,
            model,
            effort,
            mode,
            thinking,
            fast,
            runtime,
            direct_ai_config,
            user_id,
            Some((board_id.into(), board_task_id)),
        )
        .await
    }

}

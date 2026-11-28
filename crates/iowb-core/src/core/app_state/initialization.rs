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
            projects: ProjectIndex::new(storage.clone()),
            agents: AgentRuntimeManager::new(config.max_sessions),
            tasks: TaskManager::new(),
            processes: ProcessManager::with_working_directory(config.workspace_root.clone()),
            files,
            path_validator,
            watch: WatchManager::new(),
            ws_hub: WsHub::with_storage(storage),
            codex_app_server: default_codex_app_server_client(),
            codex_app_server_mutation: Arc::new(Mutex::new(())),
            chat_command_cache: Arc::new(Mutex::new(HashMap::new())),
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

    #[allow(clippy::too_many_arguments)]
    pub async fn start_agent_session_with_request(
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
        request_id: Option<String>,
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
            request_id,
        )
        .await
    }
}

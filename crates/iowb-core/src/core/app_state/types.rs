#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub storage: Storage,
    pub auth: AuthManager,
    pub sessions: SessionManager,
    pub projects: ProjectIndex,
    pub agents: AgentRuntimeManager,
    pub tasks: TaskManager,
    pub processes: ProcessManager,
    pub files: FileService,
    pub path_validator: WorkspacePathValidator,
    pub watch: WatchManager,
    pub ws_hub: WsHub,
    codex_app_server: CodexAppServerClient,
    codex_app_server_mutation: Arc<Mutex<()>>,
}

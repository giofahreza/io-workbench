const APP_SERVER_STDERR_MAX_BYTES: usize = 32 * 1024;

type AppServerStderr = Arc<Mutex<String>>;

#[derive(Debug, Clone)]
pub(crate) struct CodexThreadSnapshot {
    pub(crate) id: String,
    pub(crate) turns: Vec<CodexThreadTurn>,
}

impl CodexThreadSnapshot {
    pub(crate) fn latest_forkable_turn_id(&self) -> Option<&str> {
        self.turns
            .iter()
            .rev()
            .find(|turn| turn.status != "inProgress")
            .map(|turn| turn.id.as_str())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CodexThreadTurn {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) user_item_ids: Vec<String>,
    pub(crate) user_text: String,
}

#[derive(Clone)]
pub(crate) struct CodexAppServerClient {
    command: OsString,
    request_timeout: Duration,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CodexAppServerLaunchOptions {
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexAppServerLiveTurnParams {
    pub(crate) thread_id: Option<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) input: Vec<Value>,
    pub(crate) client_user_message_id: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) service_tier: Option<String>,
    pub(crate) approval_policy: Option<Value>,
    pub(crate) sandbox_policy: Option<Value>,
}

#[derive(Debug)]
pub(crate) enum CodexAppServerLiveTurnEvent {
    ThreadAssociated { thread_id: String },
    TurnAssociated { turn_id: String },
    Notification { method: String, params: Value },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum CodexAppServerTurnTerminalStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexAppServerLiveTurnOutcome {
    pub(crate) thread_id: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) status: CodexAppServerTurnTerminalStatus,
    pub(crate) turn: Option<Value>,
}

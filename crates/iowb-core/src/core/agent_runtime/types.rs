#[derive(Clone)]
pub struct AgentRuntimeManager {
    runs: Arc<RwLock<HashMap<String, AgentRuntimeRecord>>>,
    codex_app_server: CodexAppServerClient,
    max_runs: usize,
    max_replay_events: usize,
    max_replay_bytes: usize,
    max_output_bytes: usize,
}

struct AgentRuntimeRecord {
    replay: VecDeque<WsServerEvent>,
    replay_bytes: usize,
    abort_tx: Option<oneshot::Sender<()>>,
    last_activity: DateTime<Utc>,
}

#[derive(Clone)]
struct AgentStartContext {
    provider: Provider,
    session_id: String,
    durable_run_id: Option<String>,
    attempt_id: Option<String>,
    response_id: String,
    sequence: Arc<AtomicU64>,
    project_path: PathBuf,
    prompt: String,
    model: Option<String>,
    runtime: ChatRuntime,
    effort: Option<String>,
    mode: Option<String>,
    thinking: Option<bool>,
    fast: Option<bool>,
    native_resume_session_id: Option<String>,
    native_rollout_owned_by_provider: bool,
    context_rollover_id: Option<String>,
    direct_ai_config: Option<DirectAiRuntimeConfig>,
    direct_ai_messages: Vec<DirectAiConversationMessage>,
    sessions: SessionManager,
    storage: iowb_storage::Storage,
    hub: WsHub,
}

#[derive(Clone)]
struct ManualContextCompactionTask {
    session: SessionSummary,
    rollover_id: String,
    retry_run_id: String,
    attempt_id: String,
    native_session_id: String,
    handoff: String,
    compact_run: StoredDurableChatRun,
    runtime: ChatRuntime,
    app_server_options: Option<CodexAppServerLaunchOptions>,
}

#[derive(Clone)]
struct ContextRolloverFollowUp {
    run: StoredDurableChatRun,
}

impl AgentStartContext {
    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }
}

#[derive(Debug, Clone)]
pub struct DirectAiRuntimeConfig {
    pub base_url: String,
    pub api_key: String,
    pub max_tokens: Option<u64>,
}

struct AgentCommandSpec {
    command: String,
    args: Vec<String>,
    stdin_prompt: bool,
    prompt: String,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum AgentOutputStream {
    Stdout,
    Stderr,
}

enum AgentProcessEvent {
    Output {
        stream: AgentOutputStream,
        data: String,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
struct CodexTurnError {
    message: String,
    code: Option<String>,
    limit_bytes: Option<u64>,
    observed_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
struct NormalizedRunUsage {
    usage: SessionTokenUsage,
    raw_usage_json: Option<String>,
    source: &'static str,
    completeness: TokenUsageCompleteness,
}

#[derive(Default)]
struct CodexLiveOutputNormalizer {
    pending_line: String,
    pending_agent_message: Option<String>,
    pending_thread_id: Option<String>,
    final_assistant_message: Option<String>,
    saw_structured_event: bool,
    tool_messages: Vec<NormalizedToolMessage>,
    tool_message_bytes: usize,
    last_error: Option<CodexTurnError>,
    final_usage: Option<NormalizedRunUsage>,
}

#[derive(Default)]
struct CodexAppServerLiveOutputNormalizer {
    pending_agent_message: Option<String>,
    streamed_agent_items: HashSet<String>,
    streamed_agent_text: HashMap<String, String>,
    emitted_agent_stream: bool,
    emitted_reasoning_stream: bool,
    command_output: HashMap<String, String>,
    completed_items: HashSet<String>,
    final_assistant_message: Option<String>,
    tool_messages: Vec<NormalizedToolMessage>,
    tool_message_bytes: usize,
    last_error: Option<CodexTurnError>,
    final_usage: Option<NormalizedRunUsage>,
    emitted_visible_turn_output: bool,
}

#[derive(Debug, Clone)]
struct NormalizedToolMessage {
    name: String,
    content: String,
}

#[derive(Default)]
struct GeminiLiveOutputNormalizer {
    pending_line: String,
    pending_session_id: Option<String>,
    final_usage: Option<NormalizedRunUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsClientCommand {
    Ping {
        #[serde(skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
    },
    Subscribe {
        #[serde(default)]
        topics: Vec<String>,
        /// Board-owned chats are excluded from the ordinary session stream.
        /// A client must name the exact board session it has opened before
        /// live or replayed chat events for that session are delivered.
        #[serde(default, rename = "sessionIds")]
        session_ids: Vec<String>,
        /// Ordinary chat output remains broadcast to legacy clients when this
        /// field is absent. Mobile clients send an explicit list so hidden
        /// chat sessions do not stream into the visible UI.
        #[serde(
            default,
            rename = "chatSessionIds",
            skip_serializing_if = "Option::is_none"
        )]
        chat_session_ids: Option<Vec<String>>,
    },
    StartSession {
        provider: Provider,
        #[serde(rename = "projectPath")]
        project_path: String,
        prompt: String,
        #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fast: Option<bool>,
    },
    AbortSession {
        provider: Provider,
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    ProcessInput {
        #[serde(rename = "processId")]
        process_id: String,
        data: String,
    },
    ResizeTerminal {
        #[serde(rename = "processId")]
        process_id: String,
        cols: u16,
        rows: u16,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsServerEvent {
    Connected {
        connection_id: String,
        server_time: DateTime<Utc>,
    },
    Pong {
        #[serde(skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
        server_time: DateTime<Utc>,
    },
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<String>,
        #[serde(rename = "sessionId", default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    ChatRecoveryRequired {
        provider: Provider,
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "responseId", skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        #[serde(flatten)]
        recovery: ChatContextRecovery,
    },
    LoadingProgress {
        message: String,
        progress: f32,
    },
    ProjectsUpdated {
        projects: Vec<ProjectSummary>,
    },
    ProjectFilesChanged {
        #[serde(rename = "projectPath")]
        project_path: String,
        paths: Vec<String>,
    },
    ActiveSessions {
        sessions: Vec<SessionSummary>,
    },
    SessionStatus {
        provider: Provider,
        #[serde(rename = "sessionId")]
        session_id: String,
        status: SessionRuntimeStatus,
        #[serde(
            rename = "responseId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        response_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
        #[serde(
            rename = "latestUserPrompt",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        latest_user_prompt: Option<String>,
    },
    SessionMetadata {
        provider: Provider,
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fast: Option<bool>,
        #[serde(
            rename = "nativeSessionId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        native_session_id: Option<String>,
        #[serde(rename = "receivedAt")]
        received_at: DateTime<Utc>,
        #[serde(rename = "lastMessageAt", skip_serializing_if = "Option::is_none")]
        last_message_at: Option<DateTime<Utc>>,
        #[serde(rename = "firstUserAt", skip_serializing_if = "Option::is_none")]
        first_user_at: Option<DateTime<Utc>>,
        #[serde(rename = "tokenUsage", skip_serializing_if = "Option::is_none")]
        token_usage: Option<SessionTokenUsage>,
        #[serde(rename = "lifetimeTokenUsage", skip_serializing_if = "Option::is_none")]
        lifetime_token_usage: Option<SessionLifetimeTokenUsage>,
        #[serde(rename = "contextTokenUsage", skip_serializing_if = "Option::is_none")]
        context_token_usage: Option<SessionContextTokenUsage>,
        #[serde(rename = "spentTokenUsage", skip_serializing_if = "Option::is_none")]
        spent_token_usage: Option<SessionSpentTokenUsage>,
        #[serde(
            rename = "responseId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        response_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
    },
    Output {
        provider: Provider,
        #[serde(rename = "sessionId")]
        session_id: String,
        content: String,
        #[serde(default)]
        done: bool,
        #[serde(
            rename = "responseId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        response_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sequence: Option<u64>,
    },
    ProcessOutput {
        #[serde(rename = "processId")]
        process_id: String,
        stream: ProcessStream,
        data: String,
    },
    ProcessExited {
        #[serde(rename = "processId")]
        process_id: String,
        code: Option<i32>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionRuntimeStatus {
    Starting,
    Running,
    WaitingForInput,
    Completed,
    Aborted,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceholderResponse {
    pub implemented: bool,
    pub message: String,
}

impl PlaceholderResponse {
    pub fn not_implemented(feature: impl Into<String>) -> Self {
        Self {
            implemented: false,
            message: format!(
                "{} is not implemented in the Rust rewrite yet",
                feature.into()
            ),
        }
    }
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn default_terminal_cols() -> u16 {
    80
}

fn default_terminal_rows() -> u16 {
    24
}

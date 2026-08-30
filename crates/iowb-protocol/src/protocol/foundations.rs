
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTitleSource {
    Prompt,
    Manual,
    External,
}

pub fn session_title_from_prompt(prompt: &str) -> Option<String> {
    let visible = replace_markdown_images(prompt);
    let visible = omit_inline_base64_data_urls(&visible);
    let normalized = visible.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    if normalized.chars().count() <= AUTO_SESSION_TITLE_MAX_CHARS {
        return Some(normalized);
    }

    Some(format!(
        "{}...",
        normalized
            .chars()
            .take(AUTO_SESSION_TITLE_MAX_CHARS)
            .collect::<String>()
    ))
}

fn replace_markdown_images(input: &str) -> String {
    let mut output = String::with_capacity(input.len().min(4_096));
    let mut cursor = 0;

    while let Some(relative_start) = input[cursor..].find("![") {
        let start = cursor + relative_start;
        output.push_str(&input[cursor..start]);
        let alt_start = start + 2;
        let Some(relative_alt_end) = input[alt_start..].find("](") else {
            output.push_str(&input[start..]);
            return output;
        };
        let alt_end = alt_start + relative_alt_end;
        let target_start = alt_end + 2;
        let Some(relative_target_end) = input[target_start..].find(')') else {
            output.push_str(&input[start..]);
            return output;
        };
        let target_end = target_start + relative_target_end;
        let alt = input[alt_start..alt_end].trim();
        if alt.is_empty() {
            output.push_str("Attached image");
        } else {
            output.push_str("Attached image: ");
            output.push_str(alt);
        }
        cursor = target_end + 1;
    }

    output.push_str(&input[cursor..]);
    output
}

fn omit_inline_base64_data_urls(input: &str) -> String {
    let mut output = String::with_capacity(input.len().min(4_096));
    let mut cursor = 0;

    while let Some(relative_start) = input[cursor..].find("data:") {
        let start = cursor + relative_start;
        let Some(relative_marker) = input[start + 5..].find(";base64,") else {
            output.push_str(&input[cursor..]);
            return output;
        };
        let marker = start + 5 + relative_marker;
        if marker.saturating_sub(start) > 128 {
            output.push_str(&input[cursor..=start + 4]);
            cursor = start + 5;
            continue;
        }
        let payload_start = marker + ";base64,".len();
        let mut payload_end = payload_start;
        while payload_end < input.len() {
            let byte = input.as_bytes()[payload_end];
            if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'-' | b'_') {
                payload_end += 1;
            } else {
                break;
            }
        }
        if payload_end == payload_start {
            output.push_str(&input[cursor..payload_start]);
            cursor = payload_start;
            continue;
        }

        output.push_str(&input[cursor..start]);
        output.push_str("[attachment]");
        cursor = payload_end;
    }

    output.push_str(&input[cursor..]);
    output
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

impl ApiErrorBody {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            details: None,
            code: None,
            category: None,
            retryable: None,
        }
    }

    pub fn with_details(error: impl Into<String>, details: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            details: Some(details.into()),
            code: None,
            category: None,
            retryable: None,
        }
    }

    pub fn database(
        error: impl Into<String>,
        details: Option<String>,
        code: impl Into<String>,
        category: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            error: error.into(),
            details,
            code: Some(code.into()),
            category: Some(category.into()),
            retryable: Some(retryable),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: HealthStatus,
    pub service: String,
    pub version: String,
    pub config_dir: String,
    pub database_path: String,
    pub server_time: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Ok,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Claude,
    Codex,
    Gemini,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProjectRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkspaceRequest {
    #[serde(rename = "workspaceType")]
    pub workspace_type: WorkspaceType,
    pub path: String,
    #[serde(rename = "githubUrl", skip_serializing_if = "Option::is_none")]
    pub github_url: Option<String>,
    #[serde(rename = "githubTokenId", skip_serializing_if = "Option::is_none")]
    pub github_token_id: Option<i64>,
    #[serde(rename = "newGithubToken", skip_serializing_if = "Option::is_none")]
    pub new_github_token: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceType {
    Existing,
    New,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceValidation {
    pub valid: bool,
    #[serde(rename = "resolvedPath", skip_serializing_if = "Option::is_none")]
    pub resolved_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectListResponse {
    pub projects: Vec<ProjectSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionTokenUsage {
    #[serde(rename = "used")]
    pub used: u64,
    #[serde(rename = "input")]
    pub input: u64,
    #[serde(rename = "output")]
    pub output: u64,
    #[serde(rename = "cacheCreation")]
    pub cache_creation: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "reasoning")]
    pub reasoning: u64,
    #[serde(rename = "costUsd")]
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenUsageCompleteness {
    #[default]
    Complete,
    Partial,
    Missing,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionLifetimeTokenUsage {
    pub total: u64,
    #[serde(rename = "input")]
    pub input: u64,
    #[serde(rename = "output")]
    pub output: u64,
    #[serde(rename = "cacheCreation")]
    pub cache_creation: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    pub reasoning: u64,
    #[serde(rename = "costUsd")]
    pub cost_usd: f64,
    pub completeness: TokenUsageCompleteness,
    #[serde(rename = "partialAttempts")]
    pub partial_attempts: u64,
    #[serde(rename = "missingAttempts")]
    pub missing_attempts: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionContextTokenUsage {
    pub total: u64,
    #[serde(rename = "input")]
    pub input: u64,
    #[serde(rename = "output")]
    pub output: u64,
    #[serde(rename = "cacheCreation")]
    pub cache_creation: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    pub reasoning: u64,
    #[serde(rename = "costUsd")]
    pub cost_usd: f64,
    pub completeness: TokenUsageCompleteness,
    #[serde(rename = "partialAttempts")]
    pub partial_attempts: u64,
    #[serde(rename = "missingAttempts")]
    pub missing_attempts: u64,
    #[serde(rename = "afterCompact")]
    pub after_compact: bool,
    #[serde(rename = "compactedAt", skip_serializing_if = "Option::is_none")]
    pub compacted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSpentTokenUsage {
    #[serde(rename = "wholeSession")]
    pub whole_session: SessionLifetimeTokenUsage,
    #[serde(rename = "sinceCompact", skip_serializing_if = "Option::is_none")]
    pub since_compact: Option<SessionLifetimeTokenUsage>,
    #[serde(rename = "compactedAt", skip_serializing_if = "Option::is_none")]
    pub compacted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SessionMode {
    #[default]
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "accept-edits", alias = "acceptEdits")]
    AcceptEdits,
    #[serde(rename = "bypass")]
    Bypass,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "read-only", alias = "readOnly")]
    ReadOnly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRuntime {
    #[default]
    NativeCli,
    IoGateway,
}

impl SessionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionMode::Default => "default",
            SessionMode::AcceptEdits => "accept-edits",
            SessionMode::Bypass => "bypass",
            SessionMode::Plan => "plan",
            SessionMode::ReadOnly => "read-only",
        }
    }
    pub fn parse(value: Option<&str>) -> Self {
        match value.unwrap_or("default").to_ascii_lowercase().as_str() {
            "accept-edits" | "acceptedits" | "accept" => SessionMode::AcceptEdits,
            "bypass" | "bypass-permissions" | "bypasspermissions" | "danger" | "no-approvals"
            | "no_approvals" => SessionMode::Bypass,
            "plan" | "plan-only" => SessionMode::Plan,
            "read-only" | "readonly" | "read" => SessionMode::ReadOnly,
            _ => SessionMode::Default,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionMetadata {
    #[serde(default)]
    pub external: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<SessionMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "lastMessageAt")]
    pub last_message_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "firstUserAt")]
    pub first_user_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "receivedAt")]
    pub received_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<SessionTokenUsage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSummary {
    pub id: String,
    pub provider: Provider,
    #[serde(default)]
    pub external: bool,
    /// Sessions created by the agentic Kanban board are directly addressable
    /// chats, but are kept out of ordinary project/session discovery.
    #[serde(default, rename = "boardSession")]
    pub board_session: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "boardId",
        alias = "boardRunId"
    )]
    pub board_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "boardTaskId"
    )]
    pub board_task_id: Option<String>,
    /// Native CLI thread/session id associated with an internal workbench
    /// session. Clients use this to offer a direct `codex resume` command.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "nativeSessionId"
    )]
    pub native_session_id: Option<String>,
    /// True when the native provider rollout is owned by the provider runtime
    /// and Workbench's stored transcript is the user-visible source of truth.
    #[serde(skip)]
    pub native_rollout_owned_by_provider: bool,
    #[serde(skip)]
    pub title_source: Option<SessionTitleSource>,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    pub title: String,
    #[serde(rename = "messageCount")]
    pub message_count: usize,
    #[serde(rename = "lastActivity")]
    pub last_activity: DateTime<Utc>,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<ChatRuntime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "lastMessageAt"
    )]
    pub last_message_at: Option<DateTime<Utc>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "firstUserAt"
    )]
    pub first_user_at: Option<DateTime<Utc>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "receivedAt"
    )]
    pub received_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<SessionTokenUsage>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "lifetimeTokenUsage"
    )]
    pub lifetime_token_usage: Option<SessionLifetimeTokenUsage>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "contextTokenUsage"
    )]
    pub context_token_usage: Option<SessionContextTokenUsage>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "spentTokenUsage"
    )]
    pub spent_token_usage: Option<SessionSpentTokenUsage>,
}

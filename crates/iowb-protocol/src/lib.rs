use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const PRODUCT_NAME: &str = "io-workbench";
pub const SHORT_ALIAS: &str = "iowb";
pub const CONFIG_DIR_NAME: &str = ".io-workbench";
pub const DATABASE_FILE_NAME: &str = "io-workbench.db";
pub const ENV_PREFIX: &str = "IO_WORKBENCH_";

pub const WS_COMMAND_CHANNEL_CAPACITY: usize = 128;
pub const WS_EVENT_CHANNEL_CAPACITY: usize = 512;
pub const AUTO_SESSION_TITLE_MAX_CHARS: usize = 50;

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
        rename = "boardRunId"
    )]
    pub board_run_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "boardTaskId"
    )]
    pub board_task_id: Option<String>,
    /// Native CLI thread/session id associated with an internal workbench
    /// session. This is persisted by the server but intentionally omitted
    /// from API payloads.
    #[serde(skip)]
    pub native_session_id: Option<String>,
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

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: MessageRole,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagesResponse {
    pub session_id: String,
    pub messages: Vec<ChatMessage>,
    /// `true` when older messages still exist beyond the returned window.
    pub has_more: bool,
    /// Total number of messages stored for the session.
    pub total_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptHistoryEntry {
    pub id: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptHistoryCursor {
    pub timestamp: DateTime<Utc>,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptHistoryResponse {
    pub session_id: String,
    pub prompts: Vec<PromptHistoryEntry>,
    pub has_more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_cursor: Option<PromptHistoryCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshotResponse {
    pub session: SessionSummary,
    pub messages: Vec<ChatMessage>,
    pub has_more: bool,
    pub total_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<ChatContextRecovery>,
}

impl SessionSnapshotResponse {
    pub fn without_recovery(
        session: SessionSummary,
        messages: Vec<ChatMessage>,
        has_more: bool,
        total_count: usize,
    ) -> Self {
        Self {
            session,
            messages,
            has_more,
            total_count,
            recovery: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatContextRecovery {
    pub code: String,
    pub state: String,
    pub message: String,
    #[serde(rename = "failedMessageId")]
    pub failed_message_id: String,
    #[serde(
        rename = "observedBytes",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub observed_bytes: Option<u64>,
    #[serde(rename = "limitBytes")]
    pub limit_bytes: u64,
    #[serde(rename = "requestId", default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactSessionContextRequest {
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "failedMessageId")]
    pub failed_message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualCompactSessionContextRequest {
    #[serde(rename = "requestId")]
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactSessionContextResponse {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "responseId")]
    pub response_id: String,
    pub state: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionDraftResponse {
    pub session_id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateSessionDraftRequest {
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkSessionRequest {
    #[serde(rename = "beforeMessageId")]
    pub before_message_id: String,
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(default)]
    pub replace: bool,
    #[serde(
        rename = "draftContent",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub draft_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkSessionResponse {
    #[serde(rename = "sourceSessionId")]
    pub source_session_id: String,
    #[serde(rename = "beforeMessageId")]
    pub before_message_id: String,
    pub session: SessionSummary,
    pub draft: SessionDraftResponse,
    #[serde(rename = "nativeForked")]
    pub native_forked: bool,
    #[serde(rename = "filesUnchanged")]
    pub files_unchanged: bool,
    #[serde(rename = "sourceHidden")]
    pub source_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterFcmTokenRequest {
    pub token: String,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(rename = "deviceId", default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(rename = "appId", default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFcmTokenRequest {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcmTokenResponse {
    pub success: bool,
    #[serde(rename = "tokenCount", default)]
    pub token_count: usize,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: FileKind,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContentResponse {
    pub path: String,
    pub content: String,
    pub size: u64,
    #[serde(
        rename = "contentEncoding",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub content_encoding: Option<String>,
    #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileRequest {
    #[serde(rename = "filePath")]
    pub file_path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFileRequest {
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub directory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameFileRequest {
    #[serde(rename = "oldPath")]
    pub old_path: String,
    #[serde(rename = "newPath")]
    pub new_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRenameFileRequest {
    pub entries: Vec<RenameFileRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyFileRequest {
    #[serde(rename = "sourcePath")]
    pub source_path: String,
    #[serde(rename = "targetPath")]
    pub target_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCopyFileRequest {
    pub entries: Vec<CopyFileRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFileRequest {
    #[serde(rename = "filePath")]
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchDeleteFileRequest {
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseFilesystemResponse {
    pub path: String,
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatusRequest {
    #[serde(rename = "projectPath")]
    pub project_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatusResponse {
    pub branch: Option<String>,
    #[serde(rename = "hasCommits")]
    pub has_commits: bool,
    #[serde(default)]
    pub modified: Vec<String>,
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub deleted: Vec<String>,
    #[serde(default)]
    pub untracked: Vec<String>,
    #[serde(default)]
    pub conflicted: Vec<String>,
    pub clean: bool,
    pub files: Vec<GitFileStatus>,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileStatus {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConflictsResponse {
    pub files: Vec<GitConflictSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConflictSummary {
    pub path: String,
    pub status: String,
    #[serde(rename = "conflictCount")]
    pub conflict_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConflictFileResponse {
    pub path: String,
    pub status: String,
    pub content: String,
    pub conflicts: Vec<GitConflictRegion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConflictRegion {
    #[serde(rename = "startLine")]
    pub start_line: usize,
    #[serde(rename = "endLine")]
    pub end_line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub ours: String,
    pub theirs: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitOperationResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(rename = "remoteName", skip_serializing_if = "Option::is_none")]
    pub remote_name: Option<String>,
    #[serde(rename = "remoteUrl", skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    #[serde(rename = "remoteBranch", skip_serializing_if = "Option::is_none")]
    pub remote_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

impl GitOperationResponse {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: Some(output.into()),
            message: None,
            error: None,
            details: None,
            remote_name: None,
            remote_url: None,
            remote_branch: None,
            branch: None,
        }
    }

    pub fn message(message: impl Into<String>) -> Self {
        Self {
            success: true,
            output: None,
            message: Some(message.into()),
            error: None,
            details: None,
            remote_name: None,
            remote_url: None,
            remote_branch: None,
            branch: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiffResponse {
    pub diff: String,
    #[serde(rename = "isTruncated", default)]
    pub is_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileWithDiffResponse {
    #[serde(rename = "currentContent")]
    pub current_content: String,
    #[serde(rename = "oldContent")]
    pub old_content: String,
    #[serde(rename = "isDeleted")]
    pub is_deleted: bool,
    #[serde(rename = "isUntracked")]
    pub is_untracked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitBranchesResponse {
    pub branches: Vec<String>,
    #[serde(rename = "localBranches")]
    pub local_branches: Vec<String>,
    #[serde(rename = "remoteBranches")]
    pub remote_branches: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommitSummary {
    pub hash: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub message: String,
    #[serde(default)]
    pub stats: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommitsResponse {
    pub commits: Vec<GitCommitSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStashSummary {
    pub reference: String,
    pub hash: String,
    pub author: String,
    pub date: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStashesResponse {
    pub stashes: Vec<GitStashSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitTagSummary {
    pub name: String,
    pub hash: String,
    #[serde(rename = "objectType")]
    pub object_type: String,
    pub date: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitTagsResponse {
    pub tags: Vec<GitTagSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRemoteStatusResponse {
    #[serde(rename = "hasRemote")]
    pub has_remote: bool,
    #[serde(rename = "hasUpstream")]
    pub has_upstream: bool,
    pub branch: String,
    #[serde(rename = "remoteBranch", skip_serializing_if = "Option::is_none")]
    pub remote_branch: Option<String>,
    #[serde(rename = "remoteName", skip_serializing_if = "Option::is_none")]
    pub remote_name: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    #[serde(rename = "isUpToDate")]
    pub is_up_to_date: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitGenerateMessageResponse {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingEntry {
    pub key: String,
    pub value: Value,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatusResponse {
    pub product: String,
    pub version: String,
    pub config_dir: String,
    pub database_path: String,
    pub workspace_root: String,
    pub auth_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStatusResponse {
    pub enabled: bool,
    pub authenticated: bool,
    #[serde(rename = "needsSetup")]
    pub needs_setup: bool,
    #[serde(rename = "isAuthenticated")]
    pub is_authenticated: bool,
    #[serde(rename = "authMode")]
    pub auth_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<UserProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: String,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokenResponse {
    pub success: bool,
    pub token: String,
    pub user: UserProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessStartRequest {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub pty: bool,
    #[serde(default = "default_terminal_cols")]
    pub cols: u16,
    #[serde(default = "default_terminal_rows")]
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessStartResponse {
    pub id: String,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInputRequest {
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessResizeRequest {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub pty: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SupportedDatabaseType {
    Postgresql,
    Mysql,
    Mariadb,
    Sqlite,
}

impl SupportedDatabaseType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Postgresql => "postgresql",
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
            Self::Sqlite => "sqlite",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseTestStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseObjectType {
    Connection,
    Database,
    Schema,
    Table,
    View,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseQueryStatementType {
    Select,
    Insert,
    Update,
    Delete,
    Ddl,
    Other,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseTransferMode {
    TableCopy,
    SchemaOnly,
    SchemaAndData,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseTransferJobStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConnectionInput {
    pub name: String,
    #[serde(rename = "type")]
    pub db_type: SupportedDatabaseType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "filePath", skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(rename = "showAllDatabases", default)]
    pub show_all_databases: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConnectionProfile {
    pub id: i64,
    pub name: String,
    #[serde(rename = "type")]
    pub db_type: SupportedDatabaseType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "filePath", skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(rename = "showAllDatabases")]
    pub show_all_databases: bool,
    #[serde(rename = "hasPassword")]
    pub has_password: bool,
    #[serde(rename = "lastTestStatus", skip_serializing_if = "Option::is_none")]
    pub last_test_status: Option<DatabaseTestStatus>,
    #[serde(rename = "lastTestMessage", skip_serializing_if = "Option::is_none")]
    pub last_test_message: Option<String>,
    #[serde(rename = "lastTestedAt", skip_serializing_if = "Option::is_none")]
    pub last_tested_at: Option<DateTime<Utc>>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTestConnectionRequest {
    #[serde(
        rename = "existingConnectionId",
        skip_serializing_if = "Option::is_none"
    )]
    pub existing_connection_id: Option<i64>,
    pub connection: DatabaseConnectionInput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTestResult {
    pub status: DatabaseTestStatus,
    pub message: String,
    #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseCapabilities {
    #[serde(rename = "supportsDatabases", default)]
    pub supports_databases: bool,
    #[serde(rename = "supportsSchemas", default)]
    pub supports_schemas: bool,
    #[serde(rename = "supportsViews", default)]
    pub supports_views: bool,
    #[serde(rename = "supportsIndexes", default)]
    pub supports_indexes: bool,
    #[serde(rename = "supportsMultipleDatabases", default)]
    pub supports_multiple_databases: bool,
    #[serde(rename = "supportsForeignKeys", default)]
    pub supports_foreign_keys: bool,
    #[serde(rename = "supportsParameterizedQueries", default)]
    pub supports_parameterized_queries: bool,
    #[serde(rename = "supportsOffset", default)]
    pub supports_offset: bool,
    #[serde(rename = "supportedObjectTypes", default)]
    pub supported_object_types: Vec<DatabaseObjectType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSessionInfo {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "connectionId")]
    pub connection_id: i64,
    #[serde(rename = "type")]
    pub db_type: SupportedDatabaseType,
    pub capabilities: DatabaseCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseExplorerNode {
    pub id: String,
    #[serde(rename = "type")]
    pub object_type: DatabaseObjectType,
    #[serde(rename = "connectionId")]
    pub connection_id: i64,
    pub name: String,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    #[serde(rename = "hasChildren")]
    pub has_children: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseNameSummary {
    pub name: String,
    #[serde(rename = "isDefault", default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseObjectSummary {
    pub name: String,
    #[serde(rename = "type")]
    pub object_type: DatabaseObjectType,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseObjectColumn {
    pub name: String,
    #[serde(rename = "dataType", skip_serializing_if = "Option::is_none")]
    pub data_type: Option<String>,
    #[serde(rename = "nativeType", skip_serializing_if = "Option::is_none")]
    pub native_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,
    #[serde(rename = "defaultValue", skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<String>,
    #[serde(rename = "isPrimaryKey", default)]
    pub is_primary_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseForeignKey {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "columnName")]
    pub column_name: String,
    #[serde(
        rename = "referencedSchemaName",
        skip_serializing_if = "Option::is_none"
    )]
    pub referenced_schema_name: Option<String>,
    #[serde(rename = "referencedTableName")]
    pub referenced_table_name: String,
    #[serde(rename = "referencedColumnName")]
    pub referenced_column_name: String,
    #[serde(rename = "onUpdate", skip_serializing_if = "Option::is_none")]
    pub on_update: Option<String>,
    #[serde(rename = "onDelete", skip_serializing_if = "Option::is_none")]
    pub on_delete: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseRelationalSchemaTable {
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub object_type: DatabaseObjectType,
    pub columns: Vec<DatabaseObjectColumn>,
    #[serde(rename = "isExternal", default)]
    pub is_external: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseRelationalSchemaRelationship {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "sourceDatabaseName", skip_serializing_if = "Option::is_none")]
    pub source_database_name: Option<String>,
    #[serde(rename = "sourceSchemaName", skip_serializing_if = "Option::is_none")]
    pub source_schema_name: Option<String>,
    #[serde(rename = "sourceTableName")]
    pub source_table_name: String,
    #[serde(rename = "sourceColumnName")]
    pub source_column_name: String,
    #[serde(rename = "targetDatabaseName", skip_serializing_if = "Option::is_none")]
    pub target_database_name: Option<String>,
    #[serde(rename = "targetSchemaName", skip_serializing_if = "Option::is_none")]
    pub target_schema_name: Option<String>,
    #[serde(rename = "targetTableName")]
    pub target_table_name: String,
    #[serde(rename = "targetColumnName")]
    pub target_column_name: String,
    #[serde(rename = "onUpdate", skip_serializing_if = "Option::is_none")]
    pub on_update: Option<String>,
    #[serde(rename = "onDelete", skip_serializing_if = "Option::is_none")]
    pub on_delete: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseRelationalSchema {
    #[serde(rename = "scopeType")]
    pub scope_type: DatabaseObjectType,
    #[serde(rename = "scopeName")]
    pub scope_name: String,
    pub tables: Vec<DatabaseRelationalSchemaTable>,
    pub relationships: Vec<DatabaseRelationalSchemaRelationship>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseObjectDetails {
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub object_type: DatabaseObjectType,
    #[serde(default)]
    pub columns: Vec<DatabaseObjectColumn>,
    #[serde(rename = "primaryKey", default)]
    pub primary_key: Vec<String>,
    #[serde(rename = "foreignKeys", default)]
    pub foreign_keys: Vec<DatabaseForeignKey>,
    #[serde(rename = "relationalSchema", skip_serializing_if = "Option::is_none")]
    pub relational_schema: Option<DatabaseRelationalSchema>,
    #[serde(default)]
    pub databases: Vec<DatabaseNameSummary>,
    #[serde(default)]
    pub schemas: Vec<DatabaseNameSummary>,
    #[serde(default)]
    pub objects: Vec<DatabaseObjectSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseQueryRequest {
    pub sql: String,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    #[serde(rename = "maxRows", skip_serializing_if = "Option::is_none")]
    pub max_rows: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseQueryResult {
    pub sql: String,
    #[serde(rename = "statementType")]
    pub statement_type: DatabaseQueryStatementType,
    #[serde(rename = "rowCount")]
    pub row_count: usize,
    #[serde(rename = "returnedRowCount")]
    pub returned_row_count: usize,
    #[serde(rename = "resultTruncated")]
    pub result_truncated: bool,
    #[serde(rename = "maxRows")]
    pub max_rows: usize,
    pub rows: Vec<serde_json::Map<String, Value>>,
    pub columns: Vec<DatabaseObjectColumn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
    #[serde(rename = "durationMs")]
    pub duration_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTableData {
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    #[serde(rename = "tableName")]
    pub table_name: String,
    pub offset: usize,
    pub limit: usize,
    #[serde(rename = "rowCount")]
    pub row_count: usize,
    #[serde(rename = "totalRowCount", skip_serializing_if = "Option::is_none")]
    pub total_row_count: Option<usize>,
    #[serde(rename = "exactTotalRowCount")]
    pub exact_total_row_count: bool,
    #[serde(rename = "hasMore")]
    pub has_more: bool,
    pub columns: Vec<DatabaseObjectColumn>,
    pub rows: Vec<serde_json::Map<String, Value>>,
    #[serde(rename = "durationMs")]
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferEndpoint {
    #[serde(rename = "connectionId")]
    pub connection_id: i64,
    #[serde(rename = "connectionName", skip_serializing_if = "Option::is_none")]
    pub connection_name: Option<String>,
    #[serde(rename = "connectionType", skip_serializing_if = "Option::is_none")]
    pub connection_type: Option<SupportedDatabaseType>,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    #[serde(rename = "tableName")]
    pub table_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferRequest {
    pub mode: DatabaseTransferMode,
    pub source: DatabaseTransferEndpoint,
    pub target: DatabaseTransferEndpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferJobLogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferJobWarning {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferJobError {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferJobResult {
    #[serde(rename = "createdTable")]
    pub created_table: bool,
    #[serde(rename = "copiedRowCount")]
    pub copied_row_count: usize,
    #[serde(rename = "failedRowCount")]
    pub failed_row_count: usize,
    #[serde(rename = "ignoredSourceColumns")]
    pub ignored_source_columns: Vec<String>,
    #[serde(rename = "mappedColumnCount")]
    pub mapped_column_count: usize,
    #[serde(rename = "columnFailures")]
    pub column_failures: Vec<Value>,
    #[serde(rename = "rowFailures")]
    pub row_failures: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferJob {
    pub id: String,
    #[serde(rename = "type")]
    pub job_type: String,
    pub mode: DatabaseTransferMode,
    pub status: DatabaseTransferJobStatus,
    pub source: DatabaseTransferEndpoint,
    pub target: DatabaseTransferEndpoint,
    pub progress: Value,
    pub logs: Vec<DatabaseTransferJobLogEntry>,
    pub warnings: Vec<DatabaseTransferJobWarning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DatabaseTransferJobError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<DatabaseTransferJobResult>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
    #[serde(rename = "startedAt", skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(rename = "finishedAt", skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
}

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

#[cfg(test)]
mod tests {
    use super::{
        ForkSessionRequest, SessionLifetimeTokenUsage, SessionMode, SessionSpentTokenUsage,
        SessionSummary, WsClientCommand, session_title_from_prompt,
    };
    use chrono::{DateTime, Utc};
    use serde_json::json;

    #[test]
    fn start_session_fast_round_trips_and_defaults_to_unspecified() {
        let enabled: WsClientCommand = serde_json::from_value(json!({
            "type": "start_session",
            "provider": "codex",
            "projectPath": "/tmp/project",
            "prompt": "ship it",
            "fast": true
        }))
        .expect("deserialize fast start session");
        match enabled {
            WsClientCommand::StartSession { fast, .. } => assert_eq!(fast, Some(true)),
            _ => panic!("expected start_session"),
        }

        let legacy: WsClientCommand = serde_json::from_value(json!({
            "type": "start_session",
            "provider": "codex",
            "projectPath": "/tmp/project",
            "prompt": "ship it"
        }))
        .expect("deserialize legacy start session");
        match legacy {
            WsClientCommand::StartSession { fast, .. } => assert_eq!(fast, None),
            _ => panic!("expected start_session"),
        }
    }

    #[test]
    fn subscribe_board_session_scope_uses_camel_case_and_defaults_empty() {
        let scoped: WsClientCommand = serde_json::from_value(json!({
            "type": "subscribe",
            "topics": ["sessions"],
            "sessionIds": ["board-session"]
        }))
        .expect("deserialize scoped subscription");
        match scoped {
            WsClientCommand::Subscribe {
                topics,
                session_ids,
            } => {
                assert_eq!(topics, ["sessions"]);
                assert_eq!(session_ids, ["board-session"]);
            }
            _ => panic!("expected subscribe"),
        }

        let ordinary: WsClientCommand = serde_json::from_value(json!({
            "type": "subscribe",
            "topics": ["sessions"]
        }))
        .expect("deserialize ordinary subscription");
        match ordinary {
            WsClientCommand::Subscribe { session_ids, .. } => assert!(session_ids.is_empty()),
            _ => panic!("expected subscribe"),
        }
    }

    #[test]
    fn session_summary_serializes_explicit_fast_state() {
        let enabled = SessionSummary {
            fast: Some(true),
            ..Default::default()
        };
        let disabled = SessionSummary {
            fast: Some(false),
            ..Default::default()
        };
        let unspecified = SessionSummary::default();

        assert_eq!(
            serde_json::to_value(enabled).expect("serialize enabled")["fast"],
            true
        );
        assert_eq!(
            serde_json::to_value(disabled).expect("serialize disabled")["fast"],
            false
        );
        assert!(
            serde_json::to_value(unspecified)
                .expect("serialize unspecified")
                .get("fast")
                .is_none()
        );
    }

    #[test]
    fn board_session_summary_uses_camel_case_scope_fields() {
        let summary = SessionSummary {
            board_session: true,
            board_run_id: Some("run-1".to_string()),
            board_task_id: Some("task-1".to_string()),
            ..Default::default()
        };
        let value = serde_json::to_value(summary).expect("serialize board session");
        assert_eq!(value["boardSession"], true);
        assert_eq!(value["boardRunId"], "run-1");
        assert_eq!(value["boardTaskId"], "task-1");
    }

    #[test]
    fn session_summary_serializes_spent_token_usage_scope_fields() {
        let summary = SessionSummary {
            spent_token_usage: Some(SessionSpentTokenUsage {
                whole_session: SessionLifetimeTokenUsage {
                    total: 1_000,
                    input: 700,
                    output: 300,
                    ..Default::default()
                },
                since_compact: Some(SessionLifetimeTokenUsage {
                    total: 250,
                    input: 175,
                    output: 75,
                    ..Default::default()
                }),
                compacted_at: Some(
                    DateTime::parse_from_rfc3339("2026-08-16T00:00:00Z")
                        .expect("timestamp")
                        .with_timezone(&Utc),
                ),
            }),
            ..Default::default()
        };
        let value = serde_json::to_value(summary).expect("serialize session");
        assert_eq!(value["spentTokenUsage"]["wholeSession"]["total"], 1_000);
        assert_eq!(value["spentTokenUsage"]["sinceCompact"]["total"], 250);
        assert_eq!(
            value["spentTokenUsage"]["compactedAt"],
            "2026-08-16T00:00:00Z"
        );
    }

    #[test]
    fn fork_session_request_defaults_to_non_replacing_and_accepts_edited_draft() {
        let legacy: ForkSessionRequest = serde_json::from_value(json!({
            "beforeMessageId": "message-1",
            "requestId": "request-1"
        }))
        .expect("legacy fork request");
        assert!(!legacy.replace);
        assert_eq!(legacy.draft_content, None);

        let replacement: ForkSessionRequest = serde_json::from_value(json!({
            "beforeMessageId": "message-1",
            "requestId": "request-2",
            "replace": true,
            "draftContent": "edited prompt"
        }))
        .expect("replacement fork request");
        assert!(replacement.replace);
        assert_eq!(replacement.draft_content.as_deref(), Some("edited prompt"));
    }

    #[test]
    fn session_mode_parse_accepts_bypass_permissions_alias() {
        assert_eq!(
            SessionMode::parse(Some("bypass-permissions")),
            SessionMode::Bypass
        );
        assert_eq!(
            SessionMode::parse(Some("bypassPermissions")),
            SessionMode::Bypass
        );
    }

    #[test]
    fn session_title_normalizes_multiline_unicode_prompt() {
        assert_eq!(
            session_title_from_prompt("  Build a café page\n\nwith responsive cards  "),
            Some("Build a café page with responsive cards".to_string())
        );
    }

    #[test]
    fn session_title_truncates_by_unicode_characters() {
        let prompt = "界".repeat(60);
        assert_eq!(
            session_title_from_prompt(&prompt),
            Some(format!("{}...", "界".repeat(50)))
        );
    }

    #[test]
    fn session_title_replaces_inline_image_payload() {
        assert_eq!(
            session_title_from_prompt("![diagram.png](data:image/png;base64,QUJDRA==)"),
            Some("Attached image: diagram.png".to_string())
        );
        assert_eq!(
            session_title_from_prompt("Review this\n\n![screen](data:image/png;base64,QUJDRA==)"),
            Some("Review this Attached image: screen".to_string())
        );
    }
}

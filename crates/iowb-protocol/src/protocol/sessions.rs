impl SessionSummary {
    /// Board ownership was added after some session rows already existed.
    /// Treat either explicit ownership id as authoritative even when the
    /// older `boardSession` boolean was not persisted, or older versions
    /// called the board id `boardRunId`.
    pub fn is_board_session(&self) -> bool {
        self.board_session
            || self
                .board_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || self
                .board_task_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }
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

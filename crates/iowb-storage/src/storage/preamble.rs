use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, NaiveDateTime, Utc};
use iowb_protocol::{
    ChatMessage, DatabaseConnectionInput, DatabaseConnectionProfile, DatabaseTestStatus,
    DatabaseTransferJob, MessageRole, ProjectSummary, PromptHistoryCursor, PromptHistoryEntry,
    Provider, SessionContextTokenUsage, SessionDraftResponse, SessionLifetimeTokenUsage,
    SessionSpentTokenUsage, SessionSummary, SessionTitleSource, SessionTokenUsage, SettingEntry,
    SupportedDatabaseType, TokenUsageCompleteness, session_title_from_prompt,
};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("storage lock poisoned")]
    LockPoisoned,
    #[error("time parse error: {0}")]
    TimeParse(#[from] chrono::ParseError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, StorageError>;

#[derive(Clone)]
pub struct Storage {
    path: PathBuf,
    connection: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone)]
pub struct StoredUser {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiKeyRecord {
    pub id: i64,
    #[serde(rename = "keyName")]
    pub key_name: String,
    #[serde(rename = "api_key")]
    pub masked_key: String,
    #[serde(rename = "keyPrefix")]
    pub key_prefix: String,
    #[serde(rename = "isActive")]
    pub is_active: bool,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CredentialRecord {
    pub id: i64,
    #[serde(rename = "credentialName")]
    pub credential_name: String,
    #[serde(rename = "credentialType")]
    pub credential_type: String,
    pub description: Option<String>,
    #[serde(rename = "isActive")]
    pub is_active: bool,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct StoredDatabaseConnection {
    pub profile: DatabaseConnectionProfile,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredFcmToken {
    pub token: String,
    pub user_id: String,
    pub platform: Option<String>,
    pub device_id: Option<String>,
    pub app_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// A persisted agent invocation that can be continued after the server process
/// exits unexpectedly.
///
/// Status values are intentionally stored as strings so the runtime can add a
/// terminal status without requiring a storage schema migration. Storage uses
/// `running` and `recovering` to identify work eligible for restart recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDurableChatRun {
    pub id: String,
    pub user_id: Option<String>,
    pub session_id: String,
    pub native_session_id: Option<String>,
    pub user_message_id: Option<String>,
    pub native_before_turn_id: Option<String>,
    pub provider: String,
    pub prompt: String,
    pub project_path: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub thinking: Option<bool>,
    pub fast: Option<bool>,
    pub status: String,
    pub auto_resume: bool,
    pub resume_attempts: u32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub recovered_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct StoredChatRunAttempt {
    pub id: String,
    pub durable_run_id: String,
    pub session_id: String,
    pub user_message_id: Option<String>,
    pub provider: String,
    pub runtime: String,
    pub model: Option<String>,
    pub native_session_id: Option<String>,
    pub status: String,
    pub usage: Option<SessionTokenUsage>,
    pub raw_usage_json: Option<String>,
    pub source: Option<String>,
    pub completeness: TokenUsageCompleteness,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// A durable fingerprint and normalized summary set for one provider-owned
/// history source. A source is normally one JSON/JSONL transcript; Codex may
/// use its SQLite thread index as the source for many rollout summaries.
#[derive(Debug, Clone)]
pub struct StoredExternalHistorySource {
    pub provider: Provider,
    pub source_path: String,
    pub file_identity: Option<String>,
    pub file_size: u64,
    pub modified_nanos: Option<i64>,
    pub scan_offset: u64,
    pub parser_version: u32,
    pub records: Vec<StoredExternalSessionRecord>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct StoredExternalSessionRecord {
    pub summary: SessionSummary,
    pub file_path: String,
}

#[derive(Debug, Clone)]
pub struct ExternalHistoryFingerprint<'a> {
    pub file_identity: Option<&'a str>,
    pub file_size: u64,
    pub modified_nanos: Option<i64>,
    pub parser_version: u32,
}

impl StoredChatRunAttempt {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        durable_run_id: impl Into<String>,
        session_id: impl Into<String>,
        user_message_id: Option<String>,
        provider: impl Into<String>,
        runtime: impl Into<String>,
        model: Option<String>,
        native_session_id: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            durable_run_id: durable_run_id.into(),
            session_id: session_id.into(),
            user_message_id,
            provider: provider.into(),
            runtime: runtime.into(),
            model,
            native_session_id,
            status: "running".to_string(),
            usage: None,
            raw_usage_json: None,
            source: None,
            completeness: TokenUsageCompleteness::Missing,
            created_at: now,
            updated_at: now,
            completed_at: None,
        }
    }
}

impl StoredDurableChatRun {
    pub fn new(
        id: impl Into<String>,
        user_id: Option<String>,
        session_id: impl Into<String>,
        provider: impl Into<String>,
        prompt: impl Into<String>,
        project_path: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            user_id,
            session_id: session_id.into(),
            native_session_id: None,
            user_message_id: None,
            native_before_turn_id: None,
            provider: provider.into(),
            prompt: prompt.into(),
            project_path: project_path.into(),
            model: None,
            effort: None,
            mode: None,
            thinking: None,
            fast: None,
            status: "running".to_string(),
            auto_resume: true,
            resume_attempts: 0,
            last_error: None,
            created_at: now,
            updated_at: now,
            recovered_at: None,
            completed_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateSessionForkOutcome {
    Created,
    Existing(StoredSessionFork),
    SourceActive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSessionFork {
    pub before_message_id: String,
    pub destination_session_id: String,
    pub replaces_source: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSessionContextRollover {
    pub id: String,
    pub user_id: String,
    pub session_id: String,
    pub request_id: String,
    pub kind: String,
    pub failed_message_id: String,
    pub trigger_run_id: String,
    pub retry_run_id: String,
    pub from_native_session_id: Option<String>,
    pub candidate_native_session_id: Option<String>,
    pub state: String,
    pub handoff: String,
    pub observed_bytes: Option<u64>,
    pub limit_bytes: u64,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub activated_at: Option<DateTime<Utc>>,
}

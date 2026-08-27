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

impl Storage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let connection = Connection::open(&path)?;
        let storage = Self {
            path,
            connection: Arc::new(Mutex::new(connection)),
        };
        storage.migrate()?;
        storage.backfill_session_title_sources()?;
        Ok(storage)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn with_connection<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self
            .connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        f(&guard)
    }

    fn migrate(&self) -> Result<()> {
        self.with_connection(|conn| {
            conn.execute_batch(
                r#"
                PRAGMA journal_mode = WAL;
                PRAGMA foreign_keys = ON;

                CREATE TABLE IF NOT EXISTS meta (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS users (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    password_hash TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    last_login_at TEXT
                );

                CREATE TABLE IF NOT EXISTS auth_tokens (
                    token_hash TEXT PRIMARY KEY,
                    user_id TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    expires_at TEXT NOT NULL,
                    revoked_at TEXT,
                    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_auth_tokens_user_id
                    ON auth_tokens(user_id);

                CREATE TABLE IF NOT EXISTS projects (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    path TEXT NOT NULL UNIQUE,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS sessions (
                    id TEXT PRIMARY KEY,
                    provider TEXT NOT NULL,
                    project_path TEXT NOT NULL,
                    title TEXT NOT NULL,
                    message_count INTEGER NOT NULL DEFAULT 0,
                    last_activity TEXT NOT NULL,
                    active INTEGER NOT NULL DEFAULT 0,
                    model TEXT
                );

                CREATE TABLE IF NOT EXISTS deleted_sessions (
                    session_id TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    deleted_at TEXT NOT NULL,
                    PRIMARY KEY(session_id, provider)
                );

                CREATE TABLE IF NOT EXISTS messages (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    timestamp TEXT NOT NULL,
                    metadata TEXT NOT NULL DEFAULT 'null',
                    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_messages_session_role_time
                    ON messages(session_id, role, timestamp, id);

                CREATE TABLE IF NOT EXISTS api_keys (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id TEXT NOT NULL,
                    key_name TEXT NOT NULL,
                    key_hash TEXT NOT NULL UNIQUE,
                    key_prefix TEXT NOT NULL,
                    is_active INTEGER NOT NULL DEFAULT 1,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_api_keys_user_id
                    ON api_keys(user_id);

                CREATE TABLE IF NOT EXISTS credentials (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id TEXT NOT NULL,
                    credential_name TEXT NOT NULL,
                    credential_type TEXT NOT NULL,
                    credential_value TEXT NOT NULL,
                    description TEXT,
                    is_active INTEGER NOT NULL DEFAULT 1,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_credentials_user_type
                    ON credentials(user_id, credential_type);

                CREATE TABLE IF NOT EXISTS database_connections (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    db_type TEXT NOT NULL,
                    host TEXT,
                    port INTEGER,
                    username TEXT,
                    password TEXT,
                    database_name TEXT,
                    file_path TEXT,
                    show_all_databases INTEGER NOT NULL DEFAULT 0,
                    last_test_status TEXT,
                    last_test_message TEXT,
                    last_tested_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_database_connections_user_id
                    ON database_connections(user_id);

                CREATE TABLE IF NOT EXISTS database_transfer_jobs (
                    id TEXT PRIMARY KEY,
                    user_id TEXT NOT NULL,
                    job_json TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_database_transfer_jobs_user_id
                    ON database_transfer_jobs(user_id, updated_at DESC);

                CREATE TABLE IF NOT EXISTS durable_chat_runs (
                    id TEXT PRIMARY KEY,
                    user_id TEXT,
                    session_id TEXT NOT NULL,
                    native_session_id TEXT,
                    provider TEXT NOT NULL,
                    prompt TEXT NOT NULL,
                    project_path TEXT NOT NULL,
                    model TEXT,
                    effort TEXT,
                    mode TEXT,
                    thinking INTEGER,
                    fast INTEGER,
                    status TEXT NOT NULL,
                    auto_resume INTEGER NOT NULL DEFAULT 1,
                    resume_attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    recovered_at TEXT,
                    completed_at TEXT,
                    user_message_id TEXT,
                    native_before_turn_id TEXT
                );

                CREATE INDEX IF NOT EXISTS idx_durable_chat_runs_recoverable
                    ON durable_chat_runs(status, auto_resume, resume_attempts, updated_at);

                CREATE INDEX IF NOT EXISTS idx_durable_chat_runs_session
                    ON durable_chat_runs(session_id, created_at DESC);

                CREATE TABLE IF NOT EXISTS session_drafts (
                    user_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    content TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY(user_id, session_id),
                    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
                    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_session_drafts_session
                    ON session_drafts(session_id, updated_at DESC);

                CREATE TABLE IF NOT EXISTS session_forks (
                    user_id TEXT NOT NULL,
                    source_session_id TEXT NOT NULL,
                    before_message_id TEXT NOT NULL,
                    request_id TEXT NOT NULL,
                    destination_session_id TEXT NOT NULL,
                    replaces_source INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY(user_id, source_session_id, request_id),
                    FOREIGN KEY(destination_session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_session_forks_destination
                    ON session_forks(destination_session_id);

                CREATE TABLE IF NOT EXISTS session_context_rollovers (
                    id TEXT PRIMARY KEY,
	                    user_id TEXT NOT NULL,
	                    session_id TEXT NOT NULL,
	                    request_id TEXT NOT NULL,
	                    kind TEXT NOT NULL DEFAULT 'retry_failed_turn',
	                    failed_message_id TEXT NOT NULL,
	                    trigger_run_id TEXT NOT NULL,
	                    retry_run_id TEXT NOT NULL,
                    from_native_session_id TEXT,
                    candidate_native_session_id TEXT,
                    state TEXT NOT NULL,
                    handoff TEXT NOT NULL,
                    observed_bytes INTEGER,
                    limit_bytes INTEGER NOT NULL,
                    error TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    activated_at TEXT,
                    UNIQUE(user_id, session_id, request_id),
                    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_session_context_rollovers_session
                    ON session_context_rollovers(session_id, created_at DESC);

                CREATE UNIQUE INDEX IF NOT EXISTS idx_session_context_rollovers_retry_run
                    ON session_context_rollovers(retry_run_id);

                CREATE TABLE IF NOT EXISTS chat_run_attempts (
                    id TEXT PRIMARY KEY,
                    durable_run_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    user_message_id TEXT,
                    provider TEXT NOT NULL,
                    runtime TEXT NOT NULL,
                    model TEXT,
                    native_session_id TEXT,
                    status TEXT NOT NULL,
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                    total_tokens INTEGER NOT NULL DEFAULT 0,
                    cost_usd REAL NOT NULL DEFAULT 0,
                    raw_usage_json TEXT,
                    source TEXT,
                    completeness TEXT NOT NULL DEFAULT 'missing',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    completed_at TEXT,
                    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_chat_run_attempts_session
                    ON chat_run_attempts(session_id, created_at, id);

                CREATE INDEX IF NOT EXISTS idx_chat_run_attempts_run
                    ON chat_run_attempts(durable_run_id, created_at, id);

                CREATE INDEX IF NOT EXISTS idx_chat_run_attempts_user_message
                    ON chat_run_attempts(session_id, user_message_id);

                CREATE TABLE IF NOT EXISTS session_usage_baselines (
                    session_id TEXT PRIMARY KEY,
                    source_session_id TEXT NOT NULL,
                    before_message_id TEXT NOT NULL,
                    total_tokens INTEGER NOT NULL DEFAULT 0,
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                    cost_usd REAL NOT NULL DEFAULT 0,
                    partial_attempts INTEGER NOT NULL DEFAULT 0,
                    missing_attempts INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS fcm_device_tokens (
                    token TEXT PRIMARY KEY,
                    user_id TEXT NOT NULL,
                    platform TEXT,
                    device_id TEXT,
                    app_id TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    last_seen_at TEXT NOT NULL,
                    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_fcm_device_tokens_user_id
                    ON fcm_device_tokens(user_id, updated_at DESC);

                CREATE INDEX IF NOT EXISTS idx_fcm_device_tokens_device
                    ON fcm_device_tokens(user_id, device_id);
                "#,
            )?;

            // Idempotent column additions for older databases created before
            // the session metadata JSON column existed.
            let has_metadata: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'metadata'",
                [],
                |row| row.get(0),
            )?;
            if has_metadata == 0 {
                conn.execute_batch(
                    "ALTER TABLE sessions ADD COLUMN metadata TEXT NOT NULL DEFAULT '{}';",
                )?;
            }

            for column in ["user_message_id", "native_before_turn_id"] {
                let present: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('durable_chat_runs') WHERE name = ?1",
                    params![column],
                    |row| row.get(0),
                )?;
                if present == 0 {
                    conn.execute_batch(&format!(
                        "ALTER TABLE durable_chat_runs ADD COLUMN {column} TEXT;"
                    ))?;
                }
            }

            let has_fast: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('durable_chat_runs') WHERE name = 'fast'",
                [],
                |row| row.get(0),
            )?;
            if has_fast == 0 {
                conn.execute_batch("ALTER TABLE durable_chat_runs ADD COLUMN fast INTEGER;")?;
            }

            let has_rollover_kind: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('session_context_rollovers') WHERE name = 'kind'",
                [],
                |row| row.get(0),
            )?;
            if has_rollover_kind == 0 {
                conn.execute_batch(
                    "ALTER TABLE session_context_rollovers ADD COLUMN kind TEXT NOT NULL DEFAULT 'retry_failed_turn';",
                )?;
            }

            conn.execute_batch(
                r#"
                CREATE INDEX IF NOT EXISTS idx_durable_chat_runs_user_message
                    ON durable_chat_runs(session_id, user_message_id);

                CREATE TABLE IF NOT EXISTS session_forks (
                    user_id TEXT NOT NULL,
                    source_session_id TEXT NOT NULL,
                    before_message_id TEXT NOT NULL,
                    request_id TEXT NOT NULL,
                    destination_session_id TEXT NOT NULL,
                    replaces_source INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY(user_id, source_session_id, request_id),
                    FOREIGN KEY(destination_session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_session_forks_destination
                    ON session_forks(destination_session_id);

                CREATE TABLE IF NOT EXISTS session_context_rollovers (
                    id TEXT PRIMARY KEY,
	                    user_id TEXT NOT NULL,
	                    session_id TEXT NOT NULL,
	                    request_id TEXT NOT NULL,
	                    kind TEXT NOT NULL DEFAULT 'retry_failed_turn',
	                    failed_message_id TEXT NOT NULL,
	                    trigger_run_id TEXT NOT NULL,
	                    retry_run_id TEXT NOT NULL,
                    from_native_session_id TEXT,
                    candidate_native_session_id TEXT,
                    state TEXT NOT NULL,
                    handoff TEXT NOT NULL,
                    observed_bytes INTEGER,
                    limit_bytes INTEGER NOT NULL,
                    error TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    activated_at TEXT,
                    UNIQUE(user_id, session_id, request_id),
                    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_session_context_rollovers_session
                    ON session_context_rollovers(session_id, created_at DESC);

                CREATE UNIQUE INDEX IF NOT EXISTS idx_session_context_rollovers_retry_run
                    ON session_context_rollovers(retry_run_id);

                CREATE TABLE IF NOT EXISTS chat_run_attempts (
                    id TEXT PRIMARY KEY,
                    durable_run_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    user_message_id TEXT,
                    provider TEXT NOT NULL,
                    runtime TEXT NOT NULL,
                    model TEXT,
                    native_session_id TEXT,
                    status TEXT NOT NULL,
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                    total_tokens INTEGER NOT NULL DEFAULT 0,
                    cost_usd REAL NOT NULL DEFAULT 0,
                    raw_usage_json TEXT,
                    source TEXT,
                    completeness TEXT NOT NULL DEFAULT 'missing',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    completed_at TEXT,
                    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_chat_run_attempts_session
                    ON chat_run_attempts(session_id, created_at, id);

                CREATE INDEX IF NOT EXISTS idx_chat_run_attempts_run
                    ON chat_run_attempts(durable_run_id, created_at, id);

                CREATE INDEX IF NOT EXISTS idx_chat_run_attempts_user_message
                    ON chat_run_attempts(session_id, user_message_id);

                CREATE TABLE IF NOT EXISTS session_usage_baselines (
                    session_id TEXT PRIMARY KEY,
                    source_session_id TEXT NOT NULL,
                    before_message_id TEXT NOT NULL,
                    total_tokens INTEGER NOT NULL DEFAULT 0,
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                    cost_usd REAL NOT NULL DEFAULT 0,
                    partial_attempts INTEGER NOT NULL DEFAULT 0,
                    missing_attempts INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );
                "#,
            )?;

            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS external_history_sources (
                    provider TEXT NOT NULL,
                    source_path TEXT NOT NULL,
                    file_identity TEXT,
                    file_size INTEGER NOT NULL,
                    modified_nanos INTEGER,
                    scan_offset INTEGER NOT NULL DEFAULT 0,
                    parser_version INTEGER NOT NULL,
                    records_json TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY(provider, source_path)
                );

                CREATE INDEX IF NOT EXISTS idx_external_history_sources_provider
                    ON external_history_sources(provider, updated_at DESC);

                CREATE TABLE IF NOT EXISTS external_history_message_state (
                    provider TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    file_path TEXT NOT NULL,
                    file_identity TEXT,
                    file_size INTEGER NOT NULL,
                    modified_nanos INTEGER,
                    parser_version INTEGER NOT NULL,
                    total_count INTEGER NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY(provider, session_id, file_path)
                );

                CREATE TABLE IF NOT EXISTS external_history_messages (
                    provider TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    file_path TEXT NOT NULL,
                    sequence INTEGER NOT NULL,
                    message_json TEXT NOT NULL,
                    PRIMARY KEY(provider, session_id, file_path, sequence)
                );

                CREATE INDEX IF NOT EXISTS idx_external_history_messages_tail
                    ON external_history_messages(
                        provider, session_id, file_path, sequence DESC
                    );
                "#,
            )?;

            let has_replaces_source: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('session_forks') WHERE name = 'replaces_source'",
                [],
                |row| row.get(0),
            )?;
            if has_replaces_source == 0 {
                conn.execute_batch(
                    "ALTER TABLE session_forks ADD COLUMN replaces_source INTEGER NOT NULL DEFAULT 0;",
                )?;
            }
            conn.execute_batch(
                r#"
                CREATE INDEX IF NOT EXISTS idx_session_forks_replaced_source
                    ON session_forks(source_session_id)
                    WHERE replaces_source = 1;
                "#,
            )?;

            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', '7')",
                [],
            )?;

            Ok(())
        })
    }

    fn backfill_session_title_sources(&self) -> Result<()> {
        self.with_connection(|conn| {
            let rows = {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT s.id, s.title, s.metadata,
                           (
                               SELECT m.content
                               FROM messages m
                               WHERE m.session_id = s.id
                                 AND m.role = 'user'
                                 AND TRIM(m.content) <> ''
                               ORDER BY m.timestamp ASC, m.rowid ASC
                               LIMIT 1
                           ) AS first_user_prompt,
                           (
                               SELECT m.content
                               FROM messages m
                               WHERE m.session_id = s.id
                                 AND m.role = 'user'
                                 AND TRIM(m.content) <> ''
                               ORDER BY m.timestamp DESC, m.rowid DESC
                               LIMIT 1
                           ) AS latest_user_prompt
                    FROM sessions s
                    "#,
                )?;
                stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            };

            let transaction = conn.unchecked_transaction()?;
            for (id, current_title, raw_metadata, first_prompt, latest_prompt) in rows {
                let mut metadata = raw_metadata
                    .as_deref()
                    .and_then(deserialize_session_metadata)
                    .and_then(|value| value.as_object().cloned())
                    .unwrap_or_default();
                if metadata.contains_key("titleSource") {
                    continue;
                }

                let external = metadata
                    .get("external")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let legacy_auto_title = first_prompt.as_deref().is_some_and(|prompt| {
                    current_title == legacy_session_title_from_prompt(prompt)
                        || session_title_from_prompt(prompt).as_deref()
                            == Some(current_title.as_str())
                });
                let title_source = if current_title == "New Session" || legacy_auto_title {
                    SessionTitleSource::Prompt
                } else if external && first_prompt.is_none() {
                    SessionTitleSource::External
                } else {
                    SessionTitleSource::Manual
                };
                let next_title = if title_source == SessionTitleSource::Prompt {
                    latest_prompt
                        .as_deref()
                        .and_then(session_title_from_prompt)
                        .unwrap_or(current_title)
                } else {
                    current_title
                };
                metadata.insert(
                    "titleSource".to_string(),
                    serde_json::to_value(title_source)?,
                );
                transaction.execute(
                    "UPDATE sessions SET title = ?2, metadata = ?3 WHERE id = ?1",
                    params![id, next_title, Value::Object(metadata).to_string()],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn has_users(&self) -> Result<bool> {
        self.with_connection(|conn| {
            let count: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?;
            Ok(count > 0)
        })
    }

    pub fn has_non_local_user(&self) -> Result<bool> {
        self.with_connection(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM users WHERE id != 'local'",
                [],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
    }

    pub fn create_user(&self, id: &str, username: &str, password_hash: &str) -> Result<StoredUser> {
        let now = Utc::now();
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO users (id, username, password_hash, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
                params![
                    id,
                    username,
                    password_hash,
                    now.to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )?;

            Ok(StoredUser {
                id: id.to_string(),
                username: username.to_string(),
                password_hash: password_hash.to_string(),
                created_at: now,
                updated_at: now,
                last_login_at: None,
            })
        })
    }

    pub fn get_user_by_username(&self, username: &str) -> Result<Option<StoredUser>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT id, username, password_hash, created_at, updated_at, last_login_at
                FROM users
                WHERE username = ?1
                "#,
                params![username],
                map_user_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn get_user_by_id(&self, user_id: &str) -> Result<Option<StoredUser>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT id, username, password_hash, created_at, updated_at, last_login_at
                FROM users
                WHERE id = ?1
                "#,
                params![user_id],
                map_user_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn get_first_user(&self) -> Result<Option<StoredUser>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT id, username, password_hash, created_at, updated_at, last_login_at
                FROM users
                ORDER BY created_at ASC
                LIMIT 1
                "#,
                [],
                map_user_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn update_last_login(&self, user_id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            conn.execute(
                "UPDATE users SET last_login_at = ?1, updated_at = ?1 WHERE id = ?2",
                params![now, user_id],
            )?;
            Ok(())
        })
    }

    pub fn create_auth_token(
        &self,
        token_hash: &str,
        user_id: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO auth_tokens (token_hash, user_id, created_at, expires_at)
                VALUES (?1, ?2, ?3, ?4)
                "#,
                params![token_hash, user_id, now, expires_at.to_rfc3339()],
            )?;
            Ok(())
        })
    }

    pub fn revoke_auth_token(&self, token_hash: &str) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                "UPDATE auth_tokens SET revoked_at = ?1 WHERE token_hash = ?2 AND revoked_at IS NULL",
                params![now, token_hash],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn find_user_by_token_hash(&self, token_hash: &str) -> Result<Option<StoredUser>> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT u.id, u.username, u.password_hash, u.created_at, u.updated_at, u.last_login_at
                FROM auth_tokens t
                JOIN users u ON u.id = t.user_id
                WHERE t.token_hash = ?1
                  AND t.revoked_at IS NULL
                  AND t.expires_at > ?2
                "#,
                params![token_hash, now],
                map_user_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn upsert_project(&self, project: &ProjectSummary) -> Result<()> {
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO projects (id, name, path, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(path) DO UPDATE SET
                    name = excluded.name,
                    updated_at = excluded.updated_at
                "#,
                params![
                    project.id,
                    project.name,
                    project.path,
                    project.created_at.to_rfc3339(),
                    project.updated_at.to_rfc3339(),
                ],
            )?;
            Ok(())
        })
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectSummary>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, name, path, created_at, updated_at
                FROM projects
                ORDER BY updated_at DESC, name ASC
                "#,
            )?;

            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;

            let mut projects = Vec::new();
            for row in rows {
                let (id, name, path, created_at, updated_at) = row?;
                projects.push(ProjectSummary {
                    id,
                    name,
                    path,
                    repo_name: None,
                    created_at: parse_time(&created_at)?,
                    updated_at: parse_time(&updated_at)?,
                    sessions: Vec::new(),
                });
            }
            Ok(projects)
        })
    }

    pub fn find_project_by_name(&self, name: &str) -> Result<Option<ProjectSummary>> {
        self.with_connection(|conn| {
            let row = conn
                .query_row(
                    r#"
                    SELECT id, name, path, created_at, updated_at
                    FROM projects
                    WHERE name = ?1
                    "#,
                    params![name],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?;

            row.map(|(id, name, path, created_at, updated_at)| {
                Ok(ProjectSummary {
                    id,
                    name,
                    path,
                    repo_name: None,
                    created_at: parse_time(&created_at)?,
                    updated_at: parse_time(&updated_at)?,
                    sessions: Vec::new(),
                })
            })
            .transpose()
        })
    }

    pub fn find_project_by_id(&self, id: &str) -> Result<Option<ProjectSummary>> {
        self.with_connection(|conn| {
            let row = conn
                .query_row(
                    r#"
                    SELECT id, name, path, created_at, updated_at
                    FROM projects
                    WHERE id = ?1
                    "#,
                    params![id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?;

            row.map(|(id, name, path, created_at, updated_at)| {
                Ok(ProjectSummary {
                    id,
                    name,
                    path,
                    repo_name: None,
                    created_at: parse_time(&created_at)?,
                    updated_at: parse_time(&updated_at)?,
                    sessions: Vec::new(),
                })
            })
            .transpose()
        })
    }

    pub fn find_project_by_path(&self, path: &str) -> Result<Option<ProjectSummary>> {
        self.with_connection(|conn| {
            let row = conn
                .query_row(
                    r#"
                    SELECT id, name, path, created_at, updated_at
                    FROM projects
                    WHERE path = ?1
                    "#,
                    params![path],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?;

            row.map(|(id, name, path, created_at, updated_at)| {
                Ok(ProjectSummary {
                    id,
                    name,
                    path,
                    repo_name: None,
                    created_at: parse_time(&created_at)?,
                    updated_at: parse_time(&updated_at)?,
                    sessions: Vec::new(),
                })
            })
            .transpose()
        })
    }

    pub fn delete_project_by_id(&self, id: &str) -> Result<bool> {
        self.with_connection(|conn| {
            let changed = conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
            Ok(changed > 0)
        })
    }

    pub fn external_history_source(
        &self,
        provider: Provider,
        source_path: &str,
    ) -> Result<Option<StoredExternalHistorySource>> {
        self.with_connection(|conn| {
            let row = conn
                .query_row(
                    r#"
                    SELECT file_identity, file_size, modified_nanos, scan_offset,
                           parser_version, records_json
                    FROM external_history_sources
                    WHERE provider = ?1 AND source_path = ?2
                    "#,
                    params![provider.as_str(), source_path],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .optional()?;
            row.map(
                |(
                    file_identity,
                    file_size,
                    modified_nanos,
                    scan_offset,
                    parser_version,
                    records_json,
                )| {
                    Ok(StoredExternalHistorySource {
                        provider,
                        source_path: source_path.to_string(),
                        file_identity,
                        file_size: nonnegative_u64(file_size),
                        modified_nanos,
                        scan_offset: nonnegative_u64(scan_offset),
                        parser_version: nonnegative_u32(parser_version),
                        records: serde_json::from_str(&records_json)?,
                    })
                },
            )
            .transpose()
        })
    }

    pub fn upsert_external_history_source(
        &self,
        source: &StoredExternalHistorySource,
    ) -> Result<()> {
        let records_json = serde_json::to_string(&source.records)?;
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO external_history_sources (
                    provider, source_path, file_identity, file_size,
                    modified_nanos, scan_offset, parser_version, records_json,
                    updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(provider, source_path) DO UPDATE SET
                    file_identity = excluded.file_identity,
                    file_size = excluded.file_size,
                    modified_nanos = excluded.modified_nanos,
                    scan_offset = excluded.scan_offset,
                    parser_version = excluded.parser_version,
                    records_json = excluded.records_json,
                    updated_at = excluded.updated_at
                "#,
                params![
                    source.provider.as_str(),
                    source.source_path,
                    source.file_identity,
                    bounded_i64(source.file_size),
                    source.modified_nanos,
                    bounded_i64(source.scan_offset),
                    i64::from(source.parser_version),
                    records_json,
                    Utc::now().to_rfc3339(),
                ],
            )?;
            Ok(())
        })
    }

    pub fn prune_external_history_sources(
        &self,
        provider: Provider,
        retained_source_paths: &[String],
    ) -> Result<()> {
        let retained = retained_source_paths.iter().collect::<HashSet<_>>();
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT source_path FROM external_history_sources WHERE provider = ?1",
            )?;
            let rows = stmt.query_map(params![provider.as_str()], |row| row.get::<_, String>(0))?;
            let mut stale = Vec::new();
            for row in rows {
                let path = row?;
                if !retained.contains(&path) {
                    stale.push(path);
                }
            }
            drop(stmt);
            let transaction = conn.unchecked_transaction()?;
            for path in stale {
                transaction.execute(
                    "DELETE FROM external_history_sources WHERE provider = ?1 AND source_path = ?2",
                    params![provider.as_str(), path],
                )?;
                transaction.execute(
                    "DELETE FROM external_history_message_state WHERE provider = ?1 AND file_path = ?2",
                    params![provider.as_str(), path],
                )?;
                transaction.execute(
                    "DELETE FROM external_history_messages WHERE provider = ?1 AND file_path = ?2",
                    params![provider.as_str(), path],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn external_messages_if_current(
        &self,
        provider: Provider,
        session_id: &str,
        file_path: &str,
        fingerprint: &ExternalHistoryFingerprint<'_>,
    ) -> Result<Option<Vec<ChatMessage>>> {
        self.with_connection(|conn| {
            if !external_message_state_matches(conn, provider, session_id, file_path, fingerprint)?
            {
                return Ok(None);
            }
            let mut stmt = conn.prepare(
                r#"
                SELECT message_json
                FROM external_history_messages
                WHERE provider = ?1 AND session_id = ?2 AND file_path = ?3
                ORDER BY sequence
                "#,
            )?;
            let rows = stmt
                .query_map(params![provider.as_str(), session_id, file_path], |row| {
                    row.get::<_, String>(0)
                })?;
            let mut messages = Vec::new();
            for row in rows {
                messages.push(serde_json::from_str(&row?)?);
            }
            Ok(Some(messages))
        })
    }

    pub fn external_messages_tail_if_current(
        &self,
        provider: Provider,
        session_id: &str,
        file_path: &str,
        fingerprint: &ExternalHistoryFingerprint<'_>,
        limit: usize,
    ) -> Result<Option<(Vec<ChatMessage>, usize)>> {
        self.with_connection(|conn| {
            let Some(total_count) = external_message_state_total_if_matches(
                conn,
                provider,
                session_id,
                file_path,
                fingerprint,
            )?
            else {
                return Ok(None);
            };
            let mut stmt = conn.prepare(
                r#"
                SELECT message_json
                FROM external_history_messages
                WHERE provider = ?1 AND session_id = ?2 AND file_path = ?3
                ORDER BY sequence DESC
                LIMIT ?4
                "#,
            )?;
            let rows = stmt.query_map(
                params![
                    provider.as_str(),
                    session_id,
                    file_path,
                    bounded_i64(limit as u64),
                ],
                |row| row.get::<_, String>(0),
            )?;
            let mut messages = Vec::new();
            for row in rows {
                messages.push(serde_json::from_str(&row?)?);
            }
            messages.reverse();
            Ok(Some((messages, total_count)))
        })
    }

    pub fn replace_external_messages(
        &self,
        provider: Provider,
        session_id: &str,
        file_path: &str,
        fingerprint: &ExternalHistoryFingerprint<'_>,
        messages: &[ChatMessage],
    ) -> Result<()> {
        let serialized = messages
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            transaction.execute(
                "DELETE FROM external_history_messages WHERE provider = ?1 AND session_id = ?2 AND file_path = ?3",
                params![provider.as_str(), session_id, file_path],
            )?;
            {
                let mut insert = transaction.prepare(
                    r#"
                    INSERT INTO external_history_messages (
                        provider, session_id, file_path, sequence, message_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5)
                    "#,
                )?;
                for (sequence, message_json) in serialized.iter().enumerate() {
                    insert.execute(params![
                        provider.as_str(),
                        session_id,
                        file_path,
                        bounded_i64(sequence as u64),
                        message_json,
                    ])?;
                }
            }
            transaction.execute(
                r#"
                INSERT INTO external_history_message_state (
                    provider, session_id, file_path, file_identity, file_size,
                    modified_nanos, parser_version, total_count, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(provider, session_id, file_path) DO UPDATE SET
                    file_identity = excluded.file_identity,
                    file_size = excluded.file_size,
                    modified_nanos = excluded.modified_nanos,
                    parser_version = excluded.parser_version,
                    total_count = excluded.total_count,
                    updated_at = excluded.updated_at
                "#,
                params![
                    provider.as_str(),
                    session_id,
                    file_path,
                    fingerprint.file_identity,
                    bounded_i64(fingerprint.file_size),
                    fingerprint.modified_nanos,
                    i64::from(fingerprint.parser_version),
                    bounded_i64(messages.len() as u64),
                    Utc::now().to_rfc3339(),
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn upsert_session(&self, session: &SessionSummary) -> Result<()> {
        self.with_connection(|conn| upsert_session_conn(conn, session))
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        Ok(self
            .list_sessions_including_board()?
            .into_iter()
            .filter(|session| !session.is_board_session())
            .collect())
    }

    /// Raw persisted session loading for recovery and the in-memory manager.
    /// Unlike user-facing discovery this deliberately retains board chats.
    pub fn list_sessions_including_board(&self) -> Result<Vec<SessionSummary>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT s.id, s.provider, s.project_path, s.title,
                       COALESCE(m.message_count, s.message_count),
                       s.last_activity, s.active, s.model, s.metadata
                FROM sessions s
                LEFT JOIN (
                    SELECT session_id, COUNT(*) AS message_count
                    FROM messages
                    GROUP BY session_id
                ) m ON m.session_id = s.id
                WHERE NOT EXISTS (
                    SELECT 1 FROM session_forks f
                    WHERE f.source_session_id = s.id AND f.replaces_source = 1
                )
                ORDER BY last_activity DESC, metadata
                "#,
            )?;

            let rows = stmt.query_map([], map_session_row)?;
            let mut sessions = Vec::new();
            for row in rows {
                sessions.push(row?);
            }
            drop(stmt);
            attach_session_usage_conn(conn, &mut sessions)?;
            Ok(sessions)
        })
    }

    pub fn list_internal_native_session_ids(&self) -> Result<Vec<String>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT metadata
                FROM sessions
                WHERE metadata IS NOT NULL AND metadata <> ''
                "#,
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut native_session_ids = Vec::new();
            for row in rows {
                let raw = row?;
                let Some(metadata) = deserialize_session_metadata(&raw) else {
                    continue;
                };
                if metadata
                    .get("external")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    continue;
                }
                if let Some(native_session_id) = metadata
                    .get("nativeSessionId")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    native_session_ids.push(native_session_id.to_string());
                }
            }
            let mut rollover_stmt = conn.prepare(
                r#"
                SELECT from_native_session_id, candidate_native_session_id
                FROM session_context_rollovers
                "#,
            )?;
            let rollover_rows = rollover_stmt.query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            })?;
            for row in rollover_rows {
                let (from_native, candidate_native) = row?;
                native_session_ids.extend(
                    [from_native, candidate_native]
                        .into_iter()
                        .flatten()
                        .filter(|value| !value.trim().is_empty()),
                );
            }
            native_session_ids.sort();
            native_session_ids.dedup();
            Ok(native_session_ids)
        })
    }

    pub fn latest_context_rollover(
        &self,
        session_id: &str,
    ) -> Result<Option<StoredSessionContextRollover>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
	                SELECT id, user_id, session_id, request_id, kind, failed_message_id,
	                       trigger_run_id, retry_run_id, from_native_session_id,
	                       candidate_native_session_id, state, handoff, observed_bytes,
	                       limit_bytes, error, created_at, updated_at, activated_at
                FROM session_context_rollovers
                WHERE session_id = ?1
                ORDER BY created_at DESC, id DESC
                LIMIT 1
                "#,
                params![session_id],
                map_session_context_rollover_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn context_rollover_for_request(
        &self,
        user_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<Option<StoredSessionContextRollover>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
	                SELECT id, user_id, session_id, request_id, kind, failed_message_id,
	                       trigger_run_id, retry_run_id, from_native_session_id,
	                       candidate_native_session_id, state, handoff, observed_bytes,
	                       limit_bytes, error, created_at, updated_at, activated_at
                FROM session_context_rollovers
                WHERE user_id = ?1 AND session_id = ?2 AND request_id = ?3
                "#,
                params![user_id, session_id, request_id],
                map_session_context_rollover_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn context_rollover_for_retry_run(
        &self,
        retry_run_id: &str,
    ) -> Result<Option<StoredSessionContextRollover>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
	                SELECT id, user_id, session_id, request_id, kind, failed_message_id,
	                       trigger_run_id, retry_run_id, from_native_session_id,
	                       candidate_native_session_id, state, handoff, observed_bytes,
	                       limit_bytes, error, created_at, updated_at, activated_at
                FROM session_context_rollovers
                WHERE retry_run_id = ?1
                "#,
                params![retry_run_id],
                map_session_context_rollover_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn has_context_rollover(&self, session_id: &str) -> Result<bool> {
        self.with_connection(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM session_context_rollovers WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
    }

    pub fn has_active_context_rollover(&self, session_id: &str) -> Result<bool> {
        self.with_connection(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM session_context_rollovers WHERE session_id = ?1 AND state = 'active'",
                params![session_id],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
    }

    pub fn context_native_session_ids(&self, session_id: &str) -> Result<Vec<String>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT from_native_session_id, candidate_native_session_id
                FROM session_context_rollovers
                WHERE session_id = ?1
                "#,
            )?;
            let rows = stmt.query_map(params![session_id], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            })?;
            let mut ids = Vec::new();
            for row in rows {
                let (from_native, candidate_native) = row?;
                ids.extend(
                    [from_native, candidate_native]
                        .into_iter()
                        .flatten()
                        .filter(|value| !value.trim().is_empty()),
                );
            }
            ids.sort();
            ids.dedup();
            Ok(ids)
        })
    }

    pub fn list_replaced_source_session_ids(&self) -> Result<Vec<String>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT DISTINCT source_session_id
                FROM session_forks
                WHERE replaces_source = 1
                "#,
            )?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            let mut session_ids = Vec::new();
            for row in rows {
                session_ids.push(row?);
            }
            Ok(session_ids)
        })
    }

    pub fn list_sessions_for_project(&self, project_path: &str) -> Result<Vec<SessionSummary>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT s.id, s.provider, s.project_path, s.title,
                       COALESCE(m.message_count, s.message_count),
                       s.last_activity, s.active, s.model, s.metadata
                FROM sessions s
                LEFT JOIN (
                    SELECT session_id, COUNT(*) AS message_count
                    FROM messages
                    GROUP BY session_id
                ) m ON m.session_id = s.id
                WHERE s.project_path = ?1
                  AND NOT EXISTS (
                      SELECT 1 FROM session_forks f
                      WHERE f.source_session_id = s.id AND f.replaces_source = 1
                  )
                ORDER BY last_activity DESC, metadata
                "#,
            )?;

            let rows = stmt.query_map(params![project_path], map_session_row)?;
            let mut sessions = Vec::new();
            for row in rows {
                let session = row?;
                if !session.is_board_session() {
                    sessions.push(session);
                }
            }
            drop(stmt);
            attach_session_usage_conn(conn, &mut sessions)?;
            Ok(sessions)
        })
    }

    /// Load a persisted session without token aggregates. Internal control
    /// paths generally need identity/runtime metadata only and should not pay
    /// for lifetime aggregation while holding the shared SQLite connection.
    pub fn get_session_summary(&self, session_id: &str) -> Result<Option<SessionSummary>> {
        self.with_connection(|conn| get_session_summary_conn(conn, session_id))
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionSummary>> {
        self.with_connection(|conn| {
            let session = get_session_summary_conn(conn, session_id)?;
            session
                .map(|mut session| {
                    session.lifetime_token_usage =
                        Some(session_lifetime_token_usage_conn(conn, &session.id)?);
                    session.context_token_usage =
                        Some(session_context_token_usage_conn(conn, &session.id)?);
                    session.spent_token_usage =
                        Some(session_spent_token_usage_conn(conn, &session.id)?);
                    Ok(session)
                })
                .transpose()
        })
    }

    pub fn delete_session(&self, session_id: &str) -> Result<bool> {
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            transaction.execute(
                "DELETE FROM session_forks WHERE destination_session_id = ?1",
                params![session_id],
            )?;
            let changed =
                transaction.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
            transaction.commit()?;
            Ok(changed > 0)
        })
    }

    pub fn tombstone_session(&self, session_id: &str, provider: Provider) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO deleted_sessions (session_id, provider, deleted_at)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(session_id, provider) DO UPDATE SET
                    deleted_at = excluded.deleted_at
                "#,
                params![session_id, provider.as_str(), now],
            )?;
            Ok(())
        })
    }

    pub fn list_deleted_sessions(&self) -> Result<Vec<(Provider, String)>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT provider, session_id
                FROM deleted_sessions
                "#,
            )?;
            let rows = stmt.query_map([], |row| {
                let provider_raw: String = row.get(0)?;
                let session_id: String = row.get(1)?;
                Ok((parse_provider(&provider_raw), session_id))
            })?;
            let mut sessions = Vec::new();
            for row in rows {
                sessions.push(row?);
            }
            Ok(sessions)
        })
    }

    /// Persist a run before its agent process is launched. Callers normally
    /// construct the value with [`StoredDurableChatRun::new`], which starts it
    /// in the `running` state with recovery enabled.
    pub fn create_durable_chat_run(&self, run: &StoredDurableChatRun) -> Result<()> {
        self.with_connection(|conn| insert_durable_chat_run_conn(conn, run))
    }

    pub fn create_durable_chat_turn(
        &self,
        session: &SessionSummary,
        message: &ChatMessage,
        run: &StoredDurableChatRun,
    ) -> Result<()> {
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            upsert_session_conn(&transaction, session)?;
            insert_message_conn(&transaction, &session.id, message)?;
            insert_durable_chat_run_conn(&transaction, run)?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn create_chat_run_attempt(&self, attempt: &StoredChatRunAttempt) -> Result<bool> {
        self.with_connection(|conn| insert_chat_run_attempt_conn(conn, attempt))
    }

    pub fn update_chat_run_attempt_native_session_id(
        &self,
        attempt_id: &str,
        native_session_id: &str,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                r#"
                UPDATE chat_run_attempts
                SET native_session_id = ?1, updated_at = ?2
                WHERE id = ?3
                  AND status IN ('starting', 'running', 'recovering', 'waiting_for_input')
                  AND (native_session_id IS NULL OR native_session_id = ?1)
                "#,
                params![native_session_id, now, attempt_id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn chat_run_attempt_native_session_id(&self, attempt_id: &str) -> Result<Option<String>> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT native_session_id FROM chat_run_attempts WHERE id = ?1",
                params![attempt_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(StorageError::from)
        })
    }

    pub fn finish_chat_run_attempt(
        &self,
        attempt_id: &str,
        status: &str,
        usage: Option<&SessionTokenUsage>,
        raw_usage_json: Option<&str>,
        source: Option<&str>,
        completeness: TokenUsageCompleteness,
    ) -> Result<Option<SessionLifetimeTokenUsage>> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let session_id = transaction
                .query_row(
                    "SELECT session_id FROM chat_run_attempts WHERE id = ?1",
                    params![attempt_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let Some(session_id) = session_id else {
                return Ok(None);
            };
            let zero = SessionTokenUsage::default();
            let usage = usage.unwrap_or(&zero);
            transaction.execute(
                r#"
                UPDATE chat_run_attempts
                SET status = ?1,
                    input_tokens = ?2,
                    output_tokens = ?3,
                    cache_creation_tokens = ?4,
                    cache_read_tokens = ?5,
                    reasoning_tokens = ?6,
                    total_tokens = ?7,
                    cost_usd = ?8,
                    raw_usage_json = ?9,
                    source = ?10,
                    completeness = ?11,
                    updated_at = ?12,
                    completed_at = ?12
                WHERE id = ?13
                "#,
                params![
                    status,
                    usage.input as i64,
                    usage.output as i64,
                    usage.cache_creation as i64,
                    usage.cache_read as i64,
                    usage.reasoning as i64,
                    usage.used as i64,
                    usage.cost_usd,
                    raw_usage_json,
                    source,
                    token_usage_completeness_to_str(completeness),
                    now,
                    attempt_id,
                ],
            )?;
            let lifetime = session_lifetime_token_usage_conn(&transaction, &session_id)?;
            transaction.commit()?;
            Ok(Some(lifetime))
        })
    }

    pub fn session_lifetime_token_usage(
        &self,
        session_id: &str,
    ) -> Result<SessionLifetimeTokenUsage> {
        self.with_connection(|conn| session_lifetime_token_usage_conn(conn, session_id))
    }

    pub fn session_context_token_usage(
        &self,
        session_id: &str,
    ) -> Result<SessionContextTokenUsage> {
        self.with_connection(|conn| session_context_token_usage_conn(conn, session_id))
    }

    pub fn session_spent_token_usage(&self, session_id: &str) -> Result<SessionSpentTokenUsage> {
        self.with_connection(|conn| session_spent_token_usage_conn(conn, session_id))
    }

    pub fn latest_session_token_usage(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionTokenUsage>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT total_tokens, input_tokens, output_tokens,
                       cache_creation_tokens, cache_read_tokens,
                       reasoning_tokens, cost_usd
                FROM chat_run_attempts
                WHERE session_id = ?1
                  AND completeness <> 'missing'
                  AND total_tokens > 0
                ORDER BY completed_at DESC, created_at DESC, id DESC
                LIMIT 1
                "#,
                params![session_id],
                |row| {
                    Ok(SessionTokenUsage {
                        used: row_i64_to_u64(row, 0)?,
                        input: row_i64_to_u64(row, 1)?,
                        output: row_i64_to_u64(row, 2)?,
                        cache_creation: row_i64_to_u64(row, 3)?,
                        cache_read: row_i64_to_u64(row, 4)?,
                        reasoning: row_i64_to_u64(row, 5)?,
                        cost_usd: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn set_context_rollover_candidate(
        &self,
        rollover_id: &str,
        retry_run_id: &str,
        native_session_id: &str,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let eligible = transaction
                .query_row(
                    r#"
                    SELECT 1
                    FROM session_context_rollovers r
                    JOIN durable_chat_runs d ON d.id = r.retry_run_id
                    WHERE r.id = ?1
                      AND r.retry_run_id = ?2
                      AND r.state = 'starting'
                      AND (r.candidate_native_session_id IS NULL
                           OR r.candidate_native_session_id = ?3)
                      AND d.session_id = r.session_id
                      AND d.status IN ('running', 'recovering')
                      AND (d.native_session_id IS NULL OR d.native_session_id = ?3)
                    "#,
                    params![rollover_id, retry_run_id, native_session_id],
                    |_| Ok(()),
                )
                .optional()?;
            if eligible.is_none() {
                return Ok(false);
            }
            let run_changed = transaction.execute(
                r#"
                UPDATE durable_chat_runs
                SET native_session_id = ?1, updated_at = ?2
                WHERE id = ?3
                  AND status IN ('running', 'recovering')
                  AND (native_session_id IS NULL OR native_session_id = ?1)
                "#,
                params![native_session_id, now, retry_run_id],
            )?;
            let rollover_changed = transaction.execute(
                r#"
                UPDATE session_context_rollovers
                SET candidate_native_session_id = ?1, updated_at = ?2
                WHERE id = ?3
                  AND retry_run_id = ?4
                  AND state = 'starting'
                  AND (candidate_native_session_id IS NULL
                       OR candidate_native_session_id = ?1)
                "#,
                params![native_session_id, now, rollover_id, retry_run_id],
            )?;
            if run_changed != 1 || rollover_changed != 1 {
                return Ok(false);
            }
            transaction.commit()?;
            Ok(true)
        })
    }

    pub fn prepare_context_rollover(
        &self,
        rollover: &StoredSessionContextRollover,
        retry_run: &StoredDurableChatRun,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let inserted = transaction.execute(
                r#"
	                INSERT OR IGNORE INTO session_context_rollovers (
	                    id, user_id, session_id, request_id, kind, failed_message_id,
	                    trigger_run_id, retry_run_id, from_native_session_id,
	                    candidate_native_session_id, state, handoff, observed_bytes,
	                    limit_bytes, error, created_at, updated_at, activated_at
	                ) VALUES (
	                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
	                    ?13, ?14, ?15, ?16, ?17, ?18
	                )
	                "#,
                params![
                    rollover.id,
                    rollover.user_id,
                    rollover.session_id,
                    rollover.request_id,
                    rollover.kind,
                    rollover.failed_message_id,
                    rollover.trigger_run_id,
                    rollover.retry_run_id,
                    rollover.from_native_session_id,
                    rollover.candidate_native_session_id,
                    rollover.state,
                    rollover.handoff,
                    rollover.observed_bytes.map(|value| value as i64),
                    rollover.limit_bytes as i64,
                    rollover.error,
                    rollover.created_at.to_rfc3339(),
                    rollover.updated_at.to_rfc3339(),
                    rollover.activated_at.map(|time| time.to_rfc3339()),
                ],
            )?;
            if inserted == 0 {
                return Ok(false);
            }
            let superseded = transaction.execute(
                r#"
                UPDATE durable_chat_runs
                SET status = 'superseded', auto_resume = 0,
                    last_error = 'superseded by clean context rollover',
                    updated_at = ?1, completed_at = ?1
                WHERE id = ?2 AND status = 'failed'
                "#,
                params![now, rollover.trigger_run_id],
            )?;
            if superseded != 1 {
                return Ok(false);
            }
            insert_durable_chat_run_conn(&transaction, retry_run)?;
            transaction.commit()?;
            Ok(true)
        })
    }

    pub fn prepare_manual_context_rollover(
        &self,
        rollover: &StoredSessionContextRollover,
        compact_run: &StoredDurableChatRun,
    ) -> Result<bool> {
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let inserted = transaction.execute(
                r#"
                INSERT OR IGNORE INTO session_context_rollovers (
                    id, user_id, session_id, request_id, kind, failed_message_id,
                    trigger_run_id, retry_run_id, from_native_session_id,
                    candidate_native_session_id, state, handoff, observed_bytes,
                    limit_bytes, error, created_at, updated_at, activated_at
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16, ?17, ?18
                )
                "#,
                params![
                    rollover.id,
                    rollover.user_id,
                    rollover.session_id,
                    rollover.request_id,
                    rollover.kind,
                    rollover.failed_message_id,
                    rollover.trigger_run_id,
                    rollover.retry_run_id,
                    rollover.from_native_session_id,
                    rollover.candidate_native_session_id,
                    rollover.state,
                    rollover.handoff,
                    rollover.observed_bytes.map(|value| value as i64),
                    rollover.limit_bytes as i64,
                    rollover.error,
                    rollover.created_at.to_rfc3339(),
                    rollover.updated_at.to_rfc3339(),
                    rollover.activated_at.map(|time| time.to_rfc3339()),
                ],
            )?;
            if inserted == 0 {
                return Ok(false);
            }
            insert_durable_chat_run_conn(&transaction, compact_run)?;
            transaction.commit()?;
            Ok(true)
        })
    }

    /// Atomically switch a visible chat to its staged native thread and
    /// persist the compaction marker, optional completed assistant response,
    /// optional follow-up run, and compact run terminal state. Returning
    /// `false` leaves the transaction untouched.
    pub fn complete_context_rollover(
        &self,
        rollover_id: &str,
        retry_run_id: &str,
        candidate_native_session_id: &str,
        session: &SessionSummary,
        marker: &ChatMessage,
        assistant: Option<&ChatMessage>,
        follow_up_run: Option<&StoredDurableChatRun>,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let eligible = transaction
                .query_row(
                    r#"
                    SELECT 1
                    FROM session_context_rollovers r
                    JOIN durable_chat_runs d ON d.id = r.retry_run_id
                    WHERE r.id = ?1
                      AND r.session_id = ?2
                      AND r.retry_run_id = ?3
                      AND r.candidate_native_session_id = ?4
                      AND r.state = 'starting'
                      AND d.session_id = r.session_id
                      AND d.native_session_id = r.candidate_native_session_id
                      AND d.status IN ('running', 'recovering')
                    "#,
                    params![
                        rollover_id,
                        session.id,
                        retry_run_id,
                        candidate_native_session_id
                    ],
                    |_| Ok(()),
                )
                .optional()?;
            if eligible.is_none()
                || session.native_session_id.as_deref() != Some(candidate_native_session_id)
                || marker.role != MessageRole::System
                || assistant.is_some_and(|message| message.role != MessageRole::Assistant)
                || follow_up_run.is_some_and(|run| {
                    run.session_id != session.id
                        || run.native_session_id.as_deref() != Some(candidate_native_session_id)
                        || run.status != "running"
                })
            {
                return Ok(false);
            }

            upsert_session_conn(&transaction, session)?;
            insert_message_conn(&transaction, &session.id, marker)?;
            if let Some(assistant) = assistant {
                insert_message_conn(&transaction, &session.id, assistant)?;
            }
            if let Some(follow_up_run) = follow_up_run {
                insert_durable_chat_run_conn(&transaction, follow_up_run)?;
            }
            transaction.execute(
                r#"
                UPDATE sessions
                SET message_count = (
                        SELECT COUNT(*) FROM messages WHERE session_id = ?1
                    ),
                    last_activity = ?2,
                    active = 0
                WHERE id = ?1
                "#,
                params![session.id, session.last_activity.to_rfc3339()],
            )?;
            let run_changed = transaction.execute(
                r#"
                UPDATE durable_chat_runs
                SET status = 'completed', auto_resume = 0, last_error = NULL,
                    updated_at = ?1, completed_at = ?1
                WHERE id = ?2
                  AND session_id = ?3
                  AND native_session_id = ?4
                  AND status IN ('running', 'recovering')
                "#,
                params![now, retry_run_id, session.id, candidate_native_session_id],
            )?;
            let rollover_changed = transaction.execute(
                r#"
                UPDATE session_context_rollovers
                SET state = 'active', updated_at = ?1, activated_at = ?1, error = NULL
                WHERE id = ?2
                  AND session_id = ?3
                  AND retry_run_id = ?4
                  AND candidate_native_session_id = ?5
                  AND state = 'starting'
                "#,
                params![
                    now,
                    rollover_id,
                    session.id,
                    retry_run_id,
                    candidate_native_session_id
                ],
            )?;
            if run_changed != 1 || rollover_changed != 1 {
                return Ok(false);
            }
            transaction.commit()?;
            Ok(true)
        })
    }

    pub fn fail_context_rollover(&self, rollover_id: &str, error: &str) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                r#"
                UPDATE session_context_rollovers
                SET state = 'failed', error = ?1, updated_at = ?2
                WHERE id = ?3 AND state = 'starting'
                "#,
                params![error, now, rollover_id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn reconcile_stale_manual_context_rollovers(&self) -> Result<Vec<String>> {
        struct StaleManualRollover {
            id: String,
            retry_run_id: String,
            session_id: String,
            run_status: String,
            run_error: Option<String>,
        }

        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let stale = {
                let mut statement = transaction.prepare(
                    r#"
                    SELECT r.id, r.retry_run_id, r.session_id, d.status, d.last_error
                    FROM session_context_rollovers r
                    JOIN durable_chat_runs d
                      ON d.id = r.retry_run_id
                     AND d.session_id = r.session_id
                    WHERE r.kind = 'manual'
                      AND r.state = 'starting'
                      AND d.status NOT IN ('running', 'recovering')
                    ORDER BY r.created_at ASC, r.id ASC
                    "#,
                )?;
                let rows = statement.query_map([], |row| {
                    Ok(StaleManualRollover {
                        id: row.get(0)?,
                        retry_run_id: row.get(1)?,
                        session_id: row.get(2)?,
                        run_status: row.get(3)?,
                        run_error: row.get(4)?,
                    })
                })?;
                let mut stale = Vec::new();
                for row in rows {
                    stale.push(row?);
                }
                stale
            };

            let mut inactive_session_ids = Vec::new();
            let mut seen_inactive_session_ids = HashSet::new();
            for rollover in stale {
                let detail = rollover
                    .run_error
                    .as_deref()
                    .map(str::trim)
                    .filter(|error| !error.is_empty())
                    .unwrap_or(rollover.run_status.as_str());
                let error = format!("manual context compaction ended before activation: {detail}");
                transaction.execute(
                    r#"
                    UPDATE session_context_rollovers
                    SET state = 'failed', error = ?1, updated_at = ?2
                    WHERE id = ?3 AND state = 'starting'
                    "#,
                    params![error, now, rollover.id],
                )?;
                transaction.execute(
                    r#"
                    UPDATE chat_run_attempts
                    SET status = 'failed',
                        input_tokens = 0,
                        output_tokens = 0,
                        cache_creation_tokens = 0,
                        cache_read_tokens = 0,
                        reasoning_tokens = 0,
                        total_tokens = 0,
                        cost_usd = 0,
                        raw_usage_json = NULL,
                        source = COALESCE(source, 'startup_recovery'),
                        completeness = 'missing',
                        updated_at = ?1,
                        completed_at = ?1
                    WHERE durable_run_id = ?2
                      AND completed_at IS NULL
                      AND status IN ('starting', 'running', 'recovering', 'waiting_for_input')
                    "#,
                    params![now, rollover.retry_run_id],
                )?;
                let active_runs: i64 = transaction.query_row(
                    r#"
                    SELECT COUNT(*)
                    FROM durable_chat_runs
                    WHERE session_id = ?1
                      AND status IN ('running', 'recovering')
                    "#,
                    params![rollover.session_id],
                    |row| row.get(0),
                )?;
                if active_runs == 0 {
                    transaction.execute(
                        r#"
                        UPDATE sessions
                        SET active = 0, last_activity = ?1
                        WHERE id = ?2 AND active = 1
                        "#,
                        params![now, rollover.session_id],
                    )?;
                    if seen_inactive_session_ids.insert(rollover.session_id.clone()) {
                        inactive_session_ids.push(rollover.session_id);
                    }
                }
            }

            transaction.commit()?;
            Ok(inactive_session_ids)
        })
    }

    pub fn get_durable_chat_run(&self, run_id: &str) -> Result<Option<StoredDurableChatRun>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT id, user_id, session_id, native_session_id, provider, prompt,
                       project_path, model, effort, mode, thinking, status, auto_resume,
                       resume_attempts, last_error, created_at, updated_at, recovered_at,
                       completed_at, user_message_id, native_before_turn_id, fast
                FROM durable_chat_runs
                WHERE id = ?1
                "#,
                params![run_id],
                map_durable_chat_run_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn update_durable_chat_run_native_session_id(
        &self,
        run_id: &str,
        native_session_id: Option<&str>,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                r#"
                UPDATE durable_chat_runs
                SET native_session_id = ?1, updated_at = ?2
                WHERE id = ?3 AND status IN ('running', 'recovering')
                "#,
                params![native_session_id, now, run_id],
            )?;
            Ok(changed > 0)
        })
    }

    /// Return resumable runs in oldest-first order. Runs which disabled
    /// automatic recovery, exhausted their retry allowance, or reached a
    /// terminal status are omitted.
    pub fn list_recoverable_durable_chat_runs(
        &self,
        max_resume_attempts: u32,
        limit: usize,
    ) -> Result<Vec<StoredDurableChatRun>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, user_id, session_id, native_session_id, provider, prompt,
                       project_path, model, effort, mode, thinking, status, auto_resume,
                       resume_attempts, last_error, created_at, updated_at, recovered_at,
                       completed_at, user_message_id, native_before_turn_id, fast
                FROM durable_chat_runs
                WHERE status IN ('running', 'recovering')
                  AND auto_resume = 1
                  AND resume_attempts < ?1
                ORDER BY created_at ASC, id ASC
                LIMIT ?2
                "#,
            )?;
            let rows = stmt.query_map(
                params![i64::from(max_resume_attempts), limit],
                map_durable_chat_run_row,
            )?;
            let mut runs = Vec::new();
            for row in rows {
                runs.push(row?);
            }
            Ok(runs)
        })
    }

    /// Return every non-terminal run, including runs which opted out of
    /// recovery or exhausted their retry budget. Startup reconciliation can
    /// use this to explicitly mark those rows interrupted.
    pub fn list_active_durable_chat_runs(&self) -> Result<Vec<StoredDurableChatRun>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, user_id, session_id, native_session_id, provider, prompt,
                       project_path, model, effort, mode, thinking, status, auto_resume,
                       resume_attempts, last_error, created_at, updated_at, recovered_at,
                       completed_at, user_message_id, native_before_turn_id, fast
                FROM durable_chat_runs
                WHERE status IN ('running', 'recovering')
                ORDER BY created_at ASC, id ASC
                "#,
            )?;
            let rows = stmt.query_map([], map_durable_chat_run_row)?;
            let mut runs = Vec::new();
            for row in rows {
                runs.push(row?);
            }
            Ok(runs)
        })
    }

    /// Atomically claim a run for recovery and increment its attempt count.
    /// Returns `None` if another server already made the run terminal, recovery
    /// is disabled, or the configured attempt limit has been reached.
    pub fn mark_durable_chat_run_recovering(
        &self,
        run_id: &str,
        max_resume_attempts: u32,
    ) -> Result<Option<StoredDurableChatRun>> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                UPDATE durable_chat_runs
                SET status = 'recovering',
                    resume_attempts = resume_attempts + 1,
                    last_error = NULL,
                    recovered_at = ?1,
                    updated_at = ?1
                WHERE id = ?2
                  AND status IN ('running', 'recovering')
                  AND auto_resume = 1
                  AND resume_attempts < ?3
                RETURNING id, user_id, session_id, native_session_id, provider, prompt,
                          project_path, model, effort, mode, thinking, status, auto_resume,
                          resume_attempts, last_error, created_at, updated_at, recovered_at,
                          completed_at, user_message_id, native_before_turn_id, fast
                "#,
                params![now, run_id, i64::from(max_resume_attempts)],
                map_durable_chat_run_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    /// Mark any run terminal. `status` is kept open-ended for provider-specific
    /// outcomes such as `completed`, `aborted`, `failed`, or `interrupted`.
    pub fn mark_durable_chat_run_terminal(
        &self,
        run_id: &str,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                r#"
                UPDATE durable_chat_runs
                SET status = ?1,
                    last_error = CASE
                        WHEN ?2 = 'provider run failed'
                             AND last_error IS NOT NULL
                             AND TRIM(last_error) <> ''
                        THEN last_error
                        ELSE ?2
                    END,
                    updated_at = ?3,
                    completed_at = ?3
                WHERE id = ?4
                "#,
                params![status, last_error, now, run_id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn mark_durable_chat_run_completed(&self, run_id: &str) -> Result<bool> {
        self.mark_durable_chat_run_terminal(run_id, "completed", None)
    }

    pub fn mark_durable_chat_run_interrupted(
        &self,
        run_id: &str,
        error: Option<&str>,
    ) -> Result<bool> {
        self.mark_durable_chat_run_terminal(run_id, "interrupted", error)
    }

    pub fn mark_durable_chat_run_failed(&self, run_id: &str, error: &str) -> Result<bool> {
        self.mark_durable_chat_run_terminal(run_id, "failed", Some(error))
    }

    pub fn update_durable_chat_run_error(&self, run_id: &str, error: &str) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                "UPDATE durable_chat_runs SET last_error = ?1, updated_at = ?2 WHERE id = ?3",
                params![error, now, run_id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn append_message(&self, session_id: &str, message: &ChatMessage) -> Result<()> {
        self.with_connection(|conn| insert_message_conn(conn, session_id, message))
    }

    pub fn materialize_session_messages(
        &self,
        session_id: &str,
        messages: &[ChatMessage],
    ) -> Result<usize> {
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let mut inserted = 0usize;
            for message in messages {
                inserted += transaction.execute(
                    r#"
                    INSERT OR IGNORE INTO messages (
                        id, session_id, role, content, timestamp, metadata
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                    "#,
                    params![
                        message.id,
                        session_id,
                        role_to_str(message.role),
                        message.content,
                        message.timestamp.to_rfc3339(),
                        serde_json::to_string(&message.metadata)?,
                    ],
                )?;
            }
            transaction.execute(
                r#"
                UPDATE sessions
                SET message_count = (
                    SELECT COUNT(*) FROM messages WHERE session_id = ?1
                )
                WHERE id = ?1
                "#,
                params![session_id],
            )?;
            transaction.commit()?;
            Ok(inserted)
        })
    }

    pub fn replace_session_messages(
        &self,
        session_id: &str,
        messages: &[ChatMessage],
    ) -> Result<usize> {
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            transaction.execute(
                "DELETE FROM messages WHERE session_id = ?1",
                params![session_id],
            )?;
            let mut inserted = 0usize;
            for message in messages {
                insert_message_conn(&transaction, session_id, message)?;
                inserted += 1;
            }
            transaction.execute(
                r#"
                UPDATE sessions
                SET message_count = (
                    SELECT COUNT(*) FROM messages WHERE session_id = ?1
                )
                WHERE id = ?1
                "#,
                params![session_id],
            )?;
            transaction.commit()?;
            Ok(inserted)
        })
    }

    /// Patch the JSON metadata column for an existing message. Pass `Value::Null`
    /// to clear the metadata back to its default. Used by the chat session
    /// manager to stamp per-turn footer info (cli, model, sentAt, tokenUsage…)
    /// onto the user prompt and assistant reply rows so the UI can re-render
    /// them after a refresh or session switch.
    pub fn update_message_metadata(
        &self,
        session_id: &str,
        message_id: &str,
        metadata: Value,
    ) -> Result<bool> {
        self.with_connection(|conn| {
            let updated = conn.execute(
                r#"
                UPDATE messages
                SET metadata = ?1
                WHERE session_id = ?2 AND id = ?3
                "#,
                params![serde_json::to_string(&metadata)?, session_id, message_id,],
            )?;
            Ok(updated > 0)
        })
    }

    pub fn merge_message_metadata(
        &self,
        session_id: &str,
        message_id: &str,
        metadata: Value,
    ) -> Result<bool> {
        self.with_connection(|conn| {
            let current = conn
                .query_row(
                    r#"
                    SELECT metadata
                    FROM messages
                    WHERE session_id = ?1 AND id = ?2
                    "#,
                    params![session_id, message_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let Some(current) = current else {
                return Ok(false);
            };
            let mut merged = serde_json::from_str::<Value>(&current)
                .unwrap_or_else(|_| Value::Object(Default::default()));
            merge_metadata_patch(&mut merged, metadata);
            let updated = conn.execute(
                r#"
                UPDATE messages
                SET metadata = ?1
                WHERE session_id = ?2 AND id = ?3
                "#,
                params![serde_json::to_string(&merged)?, session_id, message_id],
            )?;
            Ok(updated > 0)
        })
    }

    /// Return the most recent assistant message id for a session so callers can
    /// patch its metadata once the streaming response is finished without
    /// having to look up the row themselves.
    pub fn latest_assistant_message_id(&self, session_id: &str) -> Result<Option<String>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT id FROM messages
                WHERE session_id = ?1 AND role = 'assistant'
                ORDER BY timestamp DESC, id DESC
                LIMIT 1
                "#,
            )?;
            let mut rows = stmt.query(params![session_id])?;
            if let Some(row) = rows.next()? {
                Ok(Some(row.get(0)?))
            } else {
                Ok(None)
            }
        })
    }

    /// Return the most recent user message id for a session. Used to stamp
    /// the "sent at" / cli / model footer onto the freshly-persisted prompt
    /// row after the agent context has been attached.
    pub fn latest_user_message_id(&self, session_id: &str) -> Result<Option<String>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT id FROM messages
                WHERE session_id = ?1 AND role = 'user'
                ORDER BY timestamp DESC, id DESC
                LIMIT 1
                "#,
            )?;
            let mut rows = stmt.query(params![session_id])?;
            if let Some(row) = rows.next()? {
                Ok(Some(row.get(0)?))
            } else {
                Ok(None)
            }
        })
    }

    pub fn latest_user_message_content(&self, session_id: &str) -> Result<Option<String>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT content FROM messages
                WHERE session_id = ?1 AND role = 'user'
                ORDER BY timestamp DESC, id DESC
                LIMIT 1
                "#,
                params![session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn message_by_id(&self, session_id: &str, message_id: &str) -> Result<Option<ChatMessage>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT id, role, content, timestamp, metadata
                FROM messages
                WHERE session_id = ?1 AND id = ?2
                "#,
                params![session_id, message_id],
                |row| {
                    let metadata_raw: String = row.get(4)?;
                    Ok(ChatMessage {
                        id: row.get(0)?,
                        role: parse_role(&row.get::<_, String>(1)?),
                        content: row.get(2)?,
                        timestamp: parse_time_sql(row.get::<_, String>(3)?)?,
                        metadata: serde_json::from_str(&metadata_raw).unwrap_or(Value::Null),
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn get_session_draft(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<SessionDraftResponse> {
        self.with_connection(|conn| {
            let row = conn
                .query_row(
                    r#"
                    SELECT content, updated_at
                    FROM session_drafts
                    WHERE user_id = ?1 AND session_id = ?2
                    "#,
                    params![user_id, session_id],
                    |row| {
                        let updated_at = parse_time_sql(row.get::<_, String>(1)?)?;
                        Ok((row.get::<_, String>(0)?, updated_at))
                    },
                )
                .optional()?;
            Ok(match row {
                Some((content, updated_at)) => SessionDraftResponse {
                    session_id: session_id.to_string(),
                    content,
                    updated_at: Some(updated_at),
                },
                None => SessionDraftResponse {
                    session_id: session_id.to_string(),
                    content: String::new(),
                    updated_at: None,
                },
            })
        })
    }

    pub fn set_session_draft(
        &self,
        user_id: &str,
        session_id: &str,
        content: &str,
    ) -> Result<SessionDraftResponse> {
        let now = Utc::now();
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO session_drafts (user_id, session_id, content, updated_at)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(user_id, session_id) DO UPDATE SET
                    content = excluded.content,
                    updated_at = excluded.updated_at
                "#,
                params![user_id, session_id, content, now.to_rfc3339()],
            )?;
            Ok(SessionDraftResponse {
                session_id: session_id.to_string(),
                content: content.to_string(),
                updated_at: Some(now),
            })
        })
    }

    pub fn delete_session_draft(&self, user_id: &str, session_id: &str) -> Result<()> {
        self.with_connection(|conn| {
            conn.execute(
                "DELETE FROM session_drafts WHERE user_id = ?1 AND session_id = ?2",
                params![user_id, session_id],
            )?;
            Ok(())
        })
    }

    pub fn upsert_fcm_token(
        &self,
        user_id: &str,
        token: &str,
        platform: Option<&str>,
        device_id: Option<&str>,
        app_id: Option<&str>,
    ) -> Result<usize> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO fcm_device_tokens (
                    token, user_id, platform, device_id, app_id,
                    created_at, updated_at, last_seen_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6)
                ON CONFLICT(token) DO UPDATE SET
                    user_id = excluded.user_id,
                    platform = excluded.platform,
                    device_id = excluded.device_id,
                    app_id = excluded.app_id,
                    updated_at = excluded.updated_at,
                    last_seen_at = excluded.last_seen_at
                "#,
                params![token, user_id, platform, device_id, app_id, now],
            )?;
            self.count_fcm_tokens_for_user_conn(conn, user_id)
        })
    }

    pub fn delete_fcm_token(&self, user_id: &str, token: &str) -> Result<usize> {
        self.with_connection(|conn| {
            conn.execute(
                "DELETE FROM fcm_device_tokens WHERE user_id = ?1 AND token = ?2",
                params![user_id, token],
            )?;
            self.count_fcm_tokens_for_user_conn(conn, user_id)
        })
    }

    pub fn list_fcm_tokens_for_user(&self, user_id: &str) -> Result<Vec<StoredFcmToken>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT token, user_id, platform, device_id, app_id,
                       created_at, updated_at, last_seen_at
                FROM fcm_device_tokens
                WHERE user_id = ?1
                ORDER BY updated_at DESC
                "#,
            )?;
            let rows = stmt.query_map(params![user_id], map_fcm_token_row)?;
            let mut tokens = Vec::new();
            for row in rows {
                tokens.push(row?);
            }
            Ok(tokens)
        })
    }

    pub fn list_all_fcm_tokens(&self) -> Result<Vec<StoredFcmToken>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT token, user_id, platform, device_id, app_id,
                       created_at, updated_at, last_seen_at
                FROM fcm_device_tokens
                ORDER BY updated_at DESC
                "#,
            )?;
            let rows = stmt.query_map([], map_fcm_token_row)?;
            let mut tokens = Vec::new();
            for row in rows {
                tokens.push(row?);
            }
            Ok(tokens)
        })
    }

    pub fn latest_durable_chat_run_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<StoredDurableChatRun>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT id, user_id, session_id, native_session_id, provider, prompt,
                       project_path, model, effort, mode, thinking, status, auto_resume,
                       resume_attempts, last_error, created_at, updated_at, recovered_at,
                       completed_at, user_message_id, native_before_turn_id, fast
                FROM durable_chat_runs
                WHERE session_id = ?1
                ORDER BY created_at DESC, id DESC
                LIMIT 1
                "#,
                params![session_id],
                map_durable_chat_run_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn durable_chat_run_for_user_message(
        &self,
        session_id: &str,
        user_message_id: &str,
    ) -> Result<Option<StoredDurableChatRun>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT id, user_id, session_id, native_session_id, provider, prompt,
                       project_path, model, effort, mode, thinking, status, auto_resume,
                       resume_attempts, last_error, created_at, updated_at, recovered_at,
                       completed_at, user_message_id, native_before_turn_id, fast
                FROM durable_chat_runs
                WHERE session_id = ?1 AND user_message_id = ?2
                ORDER BY created_at DESC, id DESC
                LIMIT 1
                "#,
                params![session_id, user_message_id],
                map_durable_chat_run_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn get_session_fork(
        &self,
        user_id: &str,
        source_session_id: &str,
        request_id: &str,
    ) -> Result<Option<StoredSessionFork>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT before_message_id, destination_session_id, replaces_source
                FROM session_forks
                WHERE user_id = ?1 AND source_session_id = ?2 AND request_id = ?3
                "#,
                params![user_id, source_session_id, request_id],
                |row| {
                    Ok(StoredSessionFork {
                        before_message_id: row.get(0)?,
                        destination_session_id: row.get(1)?,
                        replaces_source: row.get::<_, i64>(2)? != 0,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn is_session_fork_destination(&self, session_id: &str) -> Result<bool> {
        self.with_connection(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM session_forks WHERE destination_session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_session_fork(
        &self,
        user_id: &str,
        source_session_id: &str,
        before_message_id: &str,
        request_id: &str,
        destination: &SessionSummary,
        messages: &[ChatMessage],
        draft: &str,
        require_source_inactive: bool,
        replaces_source: bool,
    ) -> Result<CreateSessionForkOutcome> {
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let existing = transaction
                .query_row(
                    r#"
                    SELECT before_message_id, destination_session_id, replaces_source
                    FROM session_forks
                    WHERE user_id = ?1 AND source_session_id = ?2 AND request_id = ?3
                    "#,
                    params![user_id, source_session_id, request_id],
                    |row| {
                        Ok(StoredSessionFork {
                            before_message_id: row.get(0)?,
                            destination_session_id: row.get(1)?,
                            replaces_source: row.get::<_, i64>(2)? != 0,
                        })
                    },
                )
                .optional()?;
            if let Some(existing) = existing {
                return Ok(CreateSessionForkOutcome::Existing(existing));
            }

            if require_source_inactive {
                let source_active = transaction
                    .query_row(
                        "SELECT active FROM sessions WHERE id = ?1",
                        params![source_session_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?;
                if source_active == Some(1) {
                    return Ok(CreateSessionForkOutcome::SourceActive);
                }
            }

            upsert_session_conn(&transaction, destination)?;
            for message in messages {
                insert_message_conn(&transaction, &destination.id, message)?;
            }
            let now = Utc::now().to_rfc3339();
            let usage_baseline = fork_usage_baseline_conn(
                &transaction,
                source_session_id,
                before_message_id,
                destination,
                messages,
            )?;
            transaction.execute(
                r#"
                INSERT INTO session_usage_baselines (
                    session_id, source_session_id, before_message_id,
                    total_tokens, input_tokens, output_tokens,
                    cache_creation_tokens, cache_read_tokens, reasoning_tokens,
                    cost_usd, partial_attempts, missing_attempts, created_at
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13
                )
                "#,
                params![
                    destination.id,
                    source_session_id,
                    before_message_id,
                    usage_baseline.total as i64,
                    usage_baseline.input as i64,
                    usage_baseline.output as i64,
                    usage_baseline.cache_creation as i64,
                    usage_baseline.cache_read as i64,
                    usage_baseline.reasoning as i64,
                    usage_baseline.cost_usd,
                    usage_baseline.partial_attempts as i64,
                    usage_baseline.missing_attempts as i64,
                    now,
                ],
            )?;
            transaction.execute(
                r#"
                INSERT INTO session_drafts (user_id, session_id, content, updated_at)
                VALUES (?1, ?2, ?3, ?4)
                "#,
                params![user_id, destination.id, draft, now],
            )?;
            transaction.execute(
                r#"
                INSERT INTO session_forks (
                    user_id, source_session_id, before_message_id, request_id,
                    destination_session_id, replaces_source, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
                params![
                    user_id,
                    source_session_id,
                    before_message_id,
                    request_id,
                    destination.id,
                    i64::from(replaces_source),
                    now,
                ],
            )?;
            transaction.commit()?;
            Ok(CreateSessionForkOutcome::Created)
        })
    }

    fn count_fcm_tokens_for_user_conn(&self, conn: &Connection, user_id: &str) -> Result<usize> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM fcm_device_tokens WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    pub fn list_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, role, content, timestamp, metadata
                FROM messages
                WHERE session_id = ?1
                ORDER BY timestamp ASC, id ASC
                "#,
            )?;

            let rows = stmt.query_map(params![session_id], |row| {
                let role = parse_role(&row.get::<_, String>(1)?);
                let timestamp = parse_time_sql(row.get::<_, String>(3)?)?;
                let metadata_raw: String = row.get(4)?;
                let metadata = serde_json::from_str::<Value>(&metadata_raw).unwrap_or(Value::Null);
                Ok(ChatMessage {
                    id: row.get(0)?,
                    role,
                    content: row.get(2)?,
                    timestamp,
                    metadata,
                })
            })?;

            let mut messages = Vec::new();
            for row in rows {
                messages.push(row?);
            }
            Ok(messages)
        })
    }

    /// Return messages ordered by oldest-first with a `limit`/`offset` window.
    /// `total_count` reports the full message count so callers can implement
    /// "load older" lazy pagination.
    pub fn list_messages_page(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<ChatMessage>, usize)> {
        let (limit, offset) = (limit as i64, offset as i64);
        self.with_connection(|conn| {
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )?;

            let mut stmt = conn.prepare(
                r#"
                SELECT id, role, content, timestamp, metadata
                FROM messages
                WHERE session_id = ?1
                ORDER BY timestamp ASC, id ASC
                LIMIT ?2 OFFSET ?3
                "#,
            )?;

            let rows = stmt.query_map(params![session_id, limit, offset], |row| {
                let role = parse_role(&row.get::<_, String>(1)?);
                let timestamp = parse_time_sql(row.get::<_, String>(3)?)?;
                let metadata_raw: String = row.get(4)?;
                let metadata = serde_json::from_str::<Value>(&metadata_raw).unwrap_or(Value::Null);
                Ok(ChatMessage {
                    id: row.get(0)?,
                    role,
                    content: row.get(2)?,
                    timestamp,
                    metadata,
                })
            })?;

            let mut messages = Vec::new();
            for row in rows {
                messages.push(row?);
            }
            Ok((messages, total.max(0) as usize))
        })
    }

    pub fn list_user_prompts_page(
        &self,
        session_id: &str,
        limit: usize,
        before: Option<&PromptHistoryCursor>,
    ) -> Result<(Vec<PromptHistoryEntry>, bool)> {
        let limit = limit.max(1) as i64;
        self.with_connection(|conn| {
            let user_role = role_to_str(MessageRole::User);
            let prompts = if let Some(cursor) = before {
                let before_timestamp = cursor.timestamp.to_rfc3339();
                let mut stmt = conn.prepare(
                    r#"
                    SELECT id, content, timestamp
                    FROM (
                        SELECT id, content, timestamp
                        FROM messages
                        WHERE session_id = ?1
                          AND role = ?2
                          AND (timestamp < ?3 OR (timestamp = ?3 AND id < ?4))
                        ORDER BY timestamp DESC, id DESC
                        LIMIT ?5
                    )
                    ORDER BY timestamp ASC, id ASC
                    "#,
                )?;
                let rows = stmt.query_map(
                    params![session_id, user_role, before_timestamp, cursor.id, limit],
                    |row| {
                        Ok(PromptHistoryEntry {
                            id: row.get(0)?,
                            content: row.get(1)?,
                            timestamp: parse_time_sql(row.get::<_, String>(2)?)?,
                        })
                    },
                )?;
                let mut prompts = Vec::new();
                for row in rows {
                    prompts.push(row?);
                }
                prompts
            } else {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT id, content, timestamp
                    FROM (
                        SELECT id, content, timestamp
                        FROM messages
                        WHERE session_id = ?1 AND role = ?2
                        ORDER BY timestamp DESC, id DESC
                        LIMIT ?3
                    )
                    ORDER BY timestamp ASC, id ASC
                    "#,
                )?;
                let rows = stmt.query_map(params![session_id, user_role, limit], |row| {
                    Ok(PromptHistoryEntry {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        timestamp: parse_time_sql(row.get::<_, String>(2)?)?,
                    })
                })?;
                let mut prompts = Vec::new();
                for row in rows {
                    prompts.push(row?);
                }
                prompts
            };

            let has_more = prompts
                .first()
                .map(|oldest| {
                    let oldest_timestamp = oldest.timestamp.to_rfc3339();
                    conn.query_row(
                        r#"
                        SELECT EXISTS(
                            SELECT 1
                            FROM messages
                            WHERE session_id = ?1
                              AND role = ?2
                              AND (timestamp < ?3 OR (timestamp = ?3 AND id < ?4))
                        )
                        "#,
                        params![session_id, user_role, oldest_timestamp, oldest.id],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|value| value != 0)
                })
                .transpose()?
                .unwrap_or(false);

            Ok((prompts, has_more))
        })
    }

    pub fn search_messages(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(SessionSummary, ChatMessage)>> {
        let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT s.id, s.provider, s.project_path, s.title, s.message_count,
                       s.last_activity, s.active, s.model, s.metadata,
                       m.id, m.role, m.content, m.timestamp, m.metadata
                FROM messages m
                JOIN sessions s ON s.id = m.session_id
                WHERE (
                    LOWER(m.content) LIKE LOWER(?1) ESCAPE '\'
                    OR LOWER(s.title) LIKE LOWER(?1) ESCAPE '\'
                    OR LOWER(s.project_path) LIKE LOWER(?1) ESCAPE '\'
                )
                  AND CASE
                      WHEN json_valid(s.metadata)
                      THEN CASE
                          WHEN COALESCE(json_extract(s.metadata, '$.boardSession'), 0) = 1
                               OR json_extract(s.metadata, '$.boardId') IS NOT NULL
                               OR json_extract(s.metadata, '$.boardTaskId') IS NOT NULL
                               OR json_extract(s.metadata, '$.boardRunId') IS NOT NULL
                          THEN 1
                          ELSE 0
                      END
                      ELSE 0
                  END = 0
                ORDER BY m.timestamp DESC
                LIMIT ?2
                "#,
            )?;

            let rows = stmt.query_map(params![pattern, limit as i64], |row| {
                let mut session = SessionSummary {
                    id: row.get(0)?,
                    provider: parse_provider(&row.get::<_, String>(1)?),
                    project_path: row.get(2)?,
                    title: row.get(3)?,
                    message_count: row.get::<_, i64>(4)? as usize,
                    last_activity: parse_time_sql(row.get::<_, String>(5)?)?,
                    active: row.get::<_, i64>(6)? == 1,
                    model: row.get(7)?,
                    ..Default::default()
                };
                if let Ok(raw) = row.get::<_, Option<String>>(8) {
                    if let Some(parsed) = deserialize_session_metadata(&raw.unwrap_or_default()) {
                        merge_metadata_into(&mut session, parsed);
                    }
                }
                let role = parse_role(&row.get::<_, String>(10)?);
                let metadata_raw: String = row.get(13)?;
                let message = ChatMessage {
                    id: row.get(9)?,
                    role,
                    content: row.get(11)?,
                    timestamp: parse_time_sql(row.get::<_, String>(12)?)?,
                    metadata: serde_json::from_str::<Value>(&metadata_raw).unwrap_or(Value::Null),
                };
                Ok((session, message))
            })?;

            let mut results = Vec::new();
            for row in rows {
                let result = row?;
                if !result.0.is_board_session() {
                    results.push(result);
                }
            }
            Ok(results)
        })
    }

    pub fn set_setting(&self, key: &str, value: &Value) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO settings (key, value, updated_at)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(key) DO UPDATE SET
                    value = excluded.value,
                    updated_at = excluded.updated_at
                "#,
                params![key, serde_json::to_string(value)?, now],
            )?;
            Ok(())
        })
    }

    pub fn list_settings(&self) -> Result<Vec<SettingEntry>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT key, value, updated_at
                FROM settings
                ORDER BY key ASC
                "#,
            )?;

            let rows = stmt.query_map([], |row| {
                let value_raw: String = row.get(1)?;
                let value = serde_json::from_str::<Value>(&value_raw).unwrap_or(Value::Null);
                Ok(SettingEntry {
                    key: row.get(0)?,
                    value,
                    updated_at: parse_time_sql(row.get::<_, String>(2)?)?,
                })
            })?;

            let mut settings = Vec::new();
            for row in rows {
                settings.push(row?);
            }
            Ok(settings)
        })
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<Value>> {
        self.with_connection(|conn| {
            let value = conn
                .query_row(
                    "SELECT value FROM settings WHERE key = ?1",
                    params![key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;

            value
                .map(|raw| serde_json::from_str(&raw))
                .transpose()
                .map_err(StorageError::from)
        })
    }

    pub fn list_api_keys(&self, user_id: &str) -> Result<Vec<ApiKeyRecord>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, key_name, key_prefix, is_active, created_at, updated_at
                FROM api_keys
                WHERE user_id = ?1
                ORDER BY created_at DESC
                "#,
            )?;
            let rows = stmt.query_map(params![user_id], map_api_key_row)?;
            let mut keys = Vec::new();
            for row in rows {
                keys.push(row?);
            }
            Ok(keys)
        })
    }

    pub fn create_api_key(
        &self,
        user_id: &str,
        key_name: &str,
        key_hash: &str,
        key_prefix: &str,
    ) -> Result<ApiKeyRecord> {
        let now = Utc::now();
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO api_keys (user_id, key_name, key_hash, key_prefix, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    user_id,
                    key_name,
                    key_hash,
                    key_prefix,
                    now.to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )?;
            Ok(ApiKeyRecord {
                id: conn.last_insert_rowid(),
                key_name: key_name.to_string(),
                masked_key: mask_secret(key_prefix),
                key_prefix: key_prefix.to_string(),
                is_active: true,
                created_at: now,
                updated_at: now,
            })
        })
    }

    pub fn delete_api_key(&self, user_id: &str, key_id: i64) -> Result<bool> {
        self.with_connection(|conn| {
            let changed = conn.execute(
                "DELETE FROM api_keys WHERE user_id = ?1 AND id = ?2",
                params![user_id, key_id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn toggle_api_key(&self, user_id: &str, key_id: i64, is_active: bool) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                r#"
                UPDATE api_keys
                SET is_active = ?1, updated_at = ?2
                WHERE user_id = ?3 AND id = ?4
                "#,
                params![if is_active { 1 } else { 0 }, now, user_id, key_id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn list_credentials(
        &self,
        user_id: &str,
        credential_type: Option<&str>,
    ) -> Result<Vec<CredentialRecord>> {
        self.with_connection(|conn| {
            let mut credentials = Vec::new();
            if let Some(credential_type) = credential_type {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT id, credential_name, credential_type, description, is_active, created_at, updated_at
                    FROM credentials
                    WHERE user_id = ?1 AND credential_type = ?2
                    ORDER BY created_at DESC
                    "#,
                )?;
                let rows = stmt.query_map(params![user_id, credential_type], map_credential_row)?;
                for row in rows {
                    credentials.push(row?);
                }
                return Ok(credentials);
            }

            let mut stmt = conn.prepare(
                r#"
                SELECT id, credential_name, credential_type, description, is_active, created_at, updated_at
                FROM credentials
                WHERE user_id = ?1
                ORDER BY created_at DESC
                "#,
            )?;
            let rows = stmt.query_map(params![user_id], map_credential_row)?;
            for row in rows {
                credentials.push(row?);
            }
            Ok(credentials)
        })
    }

    pub fn get_active_credential_value(
        &self,
        user_id: &str,
        credential_id: i64,
        credential_type: &str,
    ) -> Result<Option<String>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT credential_value
                FROM credentials
                WHERE user_id = ?1
                  AND id = ?2
                  AND credential_type = ?3
                  AND is_active = 1
                "#,
                params![user_id, credential_id, credential_type],
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn get_active_credential_value_by_name(
        &self,
        user_id: &str,
        credential_name: &str,
        credential_type: &str,
    ) -> Result<Option<String>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT credential_value
                FROM credentials
                WHERE user_id = ?1
                  AND credential_name = ?2
                  AND credential_type = ?3
                  AND is_active = 1
                ORDER BY updated_at DESC, id DESC
                LIMIT 1
                "#,
                params![user_id, credential_name, credential_type],
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn upsert_named_credential(
        &self,
        user_id: &str,
        credential_name: &str,
        credential_type: &str,
        credential_value: &str,
        description: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                r#"
                UPDATE credentials
                SET credential_value = ?1, description = ?2, is_active = 1, updated_at = ?3
                WHERE id = (
                    SELECT id FROM credentials
                    WHERE user_id = ?4 AND credential_name = ?5 AND credential_type = ?6
                    ORDER BY updated_at DESC, id DESC
                    LIMIT 1
                )
                "#,
                params![
                    credential_value,
                    description,
                    now,
                    user_id,
                    credential_name,
                    credential_type,
                ],
            )?;
            if changed == 0 {
                conn.execute(
                    r#"
                    INSERT INTO credentials (
                        user_id, credential_name, credential_type, credential_value,
                        description, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                    "#,
                    params![
                        user_id,
                        credential_name,
                        credential_type,
                        credential_value,
                        description,
                        now,
                    ],
                )?;
            }
            Ok(())
        })
    }

    pub fn create_credential(
        &self,
        user_id: &str,
        credential_name: &str,
        credential_type: &str,
        credential_value: &str,
        description: Option<&str>,
    ) -> Result<CredentialRecord> {
        let now = Utc::now();
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO credentials (
                    user_id, credential_name, credential_type, credential_value,
                    description, created_at, updated_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
                params![
                    user_id,
                    credential_name,
                    credential_type,
                    credential_value,
                    description,
                    now.to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )?;
            Ok(CredentialRecord {
                id: conn.last_insert_rowid(),
                credential_name: credential_name.to_string(),
                credential_type: credential_type.to_string(),
                description: description.map(str::to_string),
                is_active: true,
                created_at: now,
                updated_at: now,
            })
        })
    }

    pub fn delete_credential(&self, user_id: &str, credential_id: i64) -> Result<bool> {
        self.with_connection(|conn| {
            let changed = conn.execute(
                "DELETE FROM credentials WHERE user_id = ?1 AND id = ?2",
                params![user_id, credential_id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn toggle_credential(
        &self,
        user_id: &str,
        credential_id: i64,
        is_active: bool,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                r#"
                UPDATE credentials
                SET is_active = ?1, updated_at = ?2
                WHERE user_id = ?3 AND id = ?4
                "#,
                params![if is_active { 1 } else { 0 }, now, user_id, credential_id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn list_database_connections(
        &self,
        user_id: &str,
    ) -> Result<Vec<DatabaseConnectionProfile>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, name, db_type, host, port, username, password, database_name,
                       file_path, show_all_databases, last_test_status, last_test_message,
                       last_tested_at, created_at, updated_at
                FROM database_connections
                WHERE user_id = ?1
                ORDER BY updated_at DESC, name ASC
                "#,
            )?;
            let rows = stmt.query_map(params![user_id], map_database_connection_row)?;
            let mut connections = Vec::new();
            for row in rows {
                connections.push(row?.profile);
            }
            Ok(connections)
        })
    }

    pub fn get_database_connection(
        &self,
        user_id: &str,
        connection_id: i64,
    ) -> Result<Option<StoredDatabaseConnection>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT id, name, db_type, host, port, username, password, database_name,
                       file_path, show_all_databases, last_test_status, last_test_message,
                       last_tested_at, created_at, updated_at
                FROM database_connections
                WHERE user_id = ?1 AND id = ?2
                "#,
                params![user_id, connection_id],
                map_database_connection_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn create_database_connection(
        &self,
        user_id: &str,
        input: &DatabaseConnectionInput,
    ) -> Result<StoredDatabaseConnection> {
        let now = Utc::now();
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO database_connections (
                    user_id, name, db_type, host, port, username, password,
                    database_name, file_path, show_all_databases, created_at, updated_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                "#,
                params![
                    user_id,
                    input.name.as_str(),
                    input.db_type.as_str(),
                    input.host.as_deref(),
                    input.port.map(i64::from),
                    input.username.as_deref(),
                    input.password.as_deref(),
                    input.database_name.as_deref(),
                    input.file_path.as_deref(),
                    if input.show_all_databases { 1 } else { 0 },
                    now.to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )?;

            Ok(StoredDatabaseConnection {
                profile: DatabaseConnectionProfile {
                    id: conn.last_insert_rowid(),
                    name: input.name.clone(),
                    db_type: input.db_type,
                    host: input.host.clone(),
                    port: input.port,
                    username: input.username.clone(),
                    database_name: input.database_name.clone(),
                    file_path: input.file_path.clone(),
                    show_all_databases: input.show_all_databases,
                    has_password: input
                        .password
                        .as_deref()
                        .is_some_and(|value| !value.is_empty()),
                    last_test_status: None,
                    last_test_message: None,
                    last_tested_at: None,
                    created_at: now,
                    updated_at: now,
                },
                password: input.password.clone(),
            })
        })
    }

    pub fn update_database_connection(
        &self,
        user_id: &str,
        connection_id: i64,
        input: &DatabaseConnectionInput,
    ) -> Result<Option<StoredDatabaseConnection>> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                r#"
                UPDATE database_connections
                SET name = ?1,
                    db_type = ?2,
                    host = ?3,
                    port = ?4,
                    username = ?5,
                    password = ?6,
                    database_name = ?7,
                    file_path = ?8,
                    show_all_databases = ?9,
                    updated_at = ?10
                WHERE user_id = ?11 AND id = ?12
                "#,
                params![
                    input.name.as_str(),
                    input.db_type.as_str(),
                    input.host.as_deref(),
                    input.port.map(i64::from),
                    input.username.as_deref(),
                    input.password.as_deref(),
                    input.database_name.as_deref(),
                    input.file_path.as_deref(),
                    if input.show_all_databases { 1 } else { 0 },
                    now,
                    user_id,
                    connection_id,
                ],
            )?;
            if changed == 0 {
                return Ok(None);
            }

            conn.query_row(
                r#"
                SELECT id, name, db_type, host, port, username, password, database_name,
                       file_path, show_all_databases, last_test_status, last_test_message,
                       last_tested_at, created_at, updated_at
                FROM database_connections
                WHERE user_id = ?1 AND id = ?2
                "#,
                params![user_id, connection_id],
                map_database_connection_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn delete_database_connection(&self, user_id: &str, connection_id: i64) -> Result<bool> {
        self.with_connection(|conn| {
            let changed = conn.execute(
                "DELETE FROM database_connections WHERE user_id = ?1 AND id = ?2",
                params![user_id, connection_id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn record_database_connection_test(
        &self,
        user_id: &str,
        connection_id: i64,
        status: DatabaseTestStatus,
        message: &str,
    ) -> Result<Option<DatabaseConnectionProfile>> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                r#"
                UPDATE database_connections
                SET last_test_status = ?1,
                    last_test_message = ?2,
                    last_tested_at = ?3,
                    updated_at = ?3
                WHERE user_id = ?4 AND id = ?5
                "#,
                params![
                    database_test_status_to_str(status),
                    message,
                    now,
                    user_id,
                    connection_id
                ],
            )?;
            if changed == 0 {
                return Ok(None);
            }

            conn.query_row(
                r#"
                SELECT id, name, db_type, host, port, username, password, database_name,
                       file_path, show_all_databases, last_test_status, last_test_message,
                       last_tested_at, created_at, updated_at
                FROM database_connections
                WHERE user_id = ?1 AND id = ?2
                "#,
                params![user_id, connection_id],
                map_database_connection_row,
            )
            .optional()
            .map(|value| value.map(|connection| connection.profile))
            .map_err(StorageError::from)
        })
    }

    pub fn upsert_database_transfer_job(
        &self,
        user_id: &str,
        job: &DatabaseTransferJob,
    ) -> Result<()> {
        let job_json = serde_json::to_string(job)?;
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO database_transfer_jobs (id, user_id, job_json, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(id) DO UPDATE SET
                    job_json = excluded.job_json,
                    updated_at = excluded.updated_at
                "#,
                params![
                    job.id,
                    user_id,
                    job_json,
                    job.created_at.to_rfc3339(),
                    job.updated_at.to_rfc3339(),
                ],
            )?;
            Ok(())
        })
    }

    pub fn list_database_transfer_jobs(&self, user_id: &str) -> Result<Vec<DatabaseTransferJob>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT job_json
                FROM database_transfer_jobs
                WHERE user_id = ?1
                ORDER BY updated_at DESC
                LIMIT 100
                "#,
            )?;
            let rows = stmt.query_map(params![user_id], |row| {
                let raw: String = row.get(0)?;
                serde_json::from_str::<DatabaseTransferJob>(&raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })?;
            let mut jobs = Vec::new();
            for row in rows {
                jobs.push(row?);
            }
            Ok(jobs)
        })
    }

    pub fn get_database_transfer_job(
        &self,
        user_id: &str,
        job_id: &str,
    ) -> Result<Option<DatabaseTransferJob>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT job_json
                FROM database_transfer_jobs
                WHERE user_id = ?1 AND id = ?2
                "#,
                params![user_id, job_id],
                |row| {
                    let raw: String = row.get(0)?;
                    serde_json::from_str::<DatabaseTransferJob>(&raw).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
        })
    }
}

fn parse_time(raw: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(raw)?.with_timezone(&Utc))
}

fn parse_time_sql(raw: String) -> rusqlite::Result<DateTime<Utc>> {
    if let Ok(time) = DateTime::parse_from_rfc3339(&raw) {
        return Ok(time.with_timezone(&Utc));
    }

    NaiveDateTime::parse_from_str(&raw, "%Y-%m-%d %H:%M:%S")
        .map(|time| time.and_utc())
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))
}

fn upsert_session_conn(conn: &Connection, session: &SessionSummary) -> Result<()> {
    let metadata_blob = serialize_session_metadata(session);
    conn.execute(
        r#"
        INSERT INTO sessions (
            id, provider, project_path, title, message_count, last_activity, active, model, metadata
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(id) DO UPDATE SET
            provider = excluded.provider,
            project_path = excluded.project_path,
            title = excluded.title,
            message_count = excluded.message_count,
            last_activity = excluded.last_activity,
            active = excluded.active,
            model = excluded.model,
            metadata = excluded.metadata
        "#,
        params![
            session.id,
            session.provider.as_str(),
            session.project_path,
            session.title,
            session.message_count as i64,
            session.last_activity.to_rfc3339(),
            if session.active { 1 } else { 0 },
            session.model,
            metadata_blob,
        ],
    )?;
    Ok(())
}

fn insert_message_conn(conn: &Connection, session_id: &str, message: &ChatMessage) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO messages (id, session_id, role, content, timestamp, metadata)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        "#,
        params![
            message.id,
            session_id,
            role_to_str(message.role),
            message.content,
            message.timestamp.to_rfc3339(),
            serde_json::to_string(&message.metadata)?,
        ],
    )?;
    Ok(())
}

fn insert_durable_chat_run_conn(conn: &Connection, run: &StoredDurableChatRun) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO durable_chat_runs (
            id, user_id, session_id, native_session_id, provider, prompt,
            project_path, model, effort, mode, thinking, status, auto_resume,
            resume_attempts, last_error, created_at, updated_at, recovered_at,
            completed_at, user_message_id, native_before_turn_id, fast
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22
        )
        "#,
        params![
            run.id,
            run.user_id,
            run.session_id,
            run.native_session_id,
            run.provider,
            run.prompt,
            run.project_path,
            run.model,
            run.effort,
            run.mode,
            run.thinking.map(i64::from),
            run.status,
            i64::from(run.auto_resume),
            i64::from(run.resume_attempts),
            run.last_error,
            run.created_at.to_rfc3339(),
            run.updated_at.to_rfc3339(),
            run.recovered_at.map(|time| time.to_rfc3339()),
            run.completed_at.map(|time| time.to_rfc3339()),
            run.user_message_id,
            run.native_before_turn_id,
            run.fast.map(i64::from),
        ],
    )?;
    Ok(())
}

fn insert_chat_run_attempt_conn(conn: &Connection, attempt: &StoredChatRunAttempt) -> Result<bool> {
    let zero = SessionTokenUsage::default();
    let usage = attempt.usage.as_ref().unwrap_or(&zero);
    let inserted = conn.execute(
        r#"
        INSERT OR IGNORE INTO chat_run_attempts (
            id, durable_run_id, session_id, user_message_id, provider, runtime,
            model, native_session_id, status, input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens, reasoning_tokens,
            total_tokens, cost_usd, raw_usage_json, source, completeness,
            created_at, updated_at, completed_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
            ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22
        )
        "#,
        params![
            attempt.id,
            attempt.durable_run_id,
            attempt.session_id,
            attempt.user_message_id,
            attempt.provider,
            attempt.runtime,
            attempt.model,
            attempt.native_session_id,
            attempt.status,
            usage.input as i64,
            usage.output as i64,
            usage.cache_creation as i64,
            usage.cache_read as i64,
            usage.reasoning as i64,
            usage.used as i64,
            usage.cost_usd,
            attempt.raw_usage_json,
            attempt.source,
            token_usage_completeness_to_str(attempt.completeness),
            attempt.created_at.to_rfc3339(),
            attempt.updated_at.to_rfc3339(),
            attempt.completed_at.map(|time| time.to_rfc3339()),
        ],
    )?;
    Ok(inserted > 0)
}

fn session_lifetime_token_usage_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionLifetimeTokenUsage> {
    let attempts = lifetime_attempt_usage_for_session_conn(conn, session_id)?;
    let baseline = session_usage_baseline_conn(conn, session_id)?;
    Ok(combine_lifetime_usage(baseline, attempts))
}

fn session_spent_token_usage_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionSpentTokenUsage> {
    let provider = session_provider_conn(conn, session_id)?;
    let compacted_at = latest_active_context_compacted_at_conn(conn, session_id)?;
    let whole_session = session_spent_token_usage_scope_conn(conn, session_id, &provider, None)?;
    let since_compact = compacted_at
        .map(|compacted_at| {
            session_spent_token_usage_scope_conn(conn, session_id, &provider, Some(compacted_at))
        })
        .transpose()?;
    Ok(SessionSpentTokenUsage {
        whole_session,
        since_compact,
        compacted_at,
    })
}

fn session_spent_token_usage_scope_conn(
    conn: &Connection,
    session_id: &str,
    provider: &str,
    created_after: Option<DateTime<Utc>>,
) -> Result<SessionLifetimeTokenUsage> {
    let attempts = if provider == Provider::Codex.as_str() {
        codex_spent_attempt_usage_conn(conn, session_id, created_after)?
    } else {
        attempt_spent_usage_for_session_conn(conn, session_id, created_after)?
    };
    if created_after.is_some() {
        Ok(attempts)
    } else {
        Ok(combine_lifetime_usage(
            session_usage_baseline_conn(conn, session_id)?,
            attempts,
        ))
    }
}

fn session_context_token_usage_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionContextTokenUsage> {
    let lifetime = session_lifetime_token_usage_conn(conn, session_id)?;
    let provider = session_provider_conn(conn, session_id)?;
    let Some(compacted_at) = latest_active_context_compacted_at_conn(conn, session_id)? else {
        if provider == Provider::Codex.as_str() {
            let mut usage = codex_latest_cumulative_usage_conn(conn, session_id)?
                .unwrap_or_else(|| lifetime.clone());
            let (partial_attempts, missing_attempts) =
                attempt_completeness_counts_conn(conn, session_id, None)?;
            usage.partial_attempts = partial_attempts;
            usage.missing_attempts = missing_attempts;
            usage.completeness = lifetime_usage_completeness(&usage);
            return Ok(context_usage_from_lifetime(usage, false, None));
        }
        return Ok(context_usage_from_lifetime(lifetime, false, None));
    };
    let scoped = if provider == Provider::Codex.as_str() {
        codex_context_usage_delta_conn(conn, session_id, compacted_at)?
    } else {
        attempt_usage_since_compact_conn(conn, session_id, compacted_at)?
    };
    Ok(context_usage_from_lifetime(
        scoped,
        true,
        Some(compacted_at),
    ))
}

fn session_usage_baseline_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionLifetimeTokenUsage> {
    conn.query_row(
        r#"
        SELECT total_tokens, input_tokens, output_tokens, cache_creation_tokens,
               cache_read_tokens, reasoning_tokens, cost_usd,
               partial_attempts, missing_attempts
        FROM session_usage_baselines
        WHERE session_id = ?1
        "#,
        params![session_id],
        map_lifetime_usage_row,
    )
    .optional()
    .map(|value| value.unwrap_or_default())
    .map_err(StorageError::from)
}

fn session_provider_conn(conn: &Connection, session_id: &str) -> Result<String> {
    conn.query_row(
        "SELECT provider FROM sessions WHERE id = ?1",
        params![session_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map(|value| value.unwrap_or_default())
    .map_err(StorageError::from)
}

fn latest_active_context_compacted_at_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<DateTime<Utc>>> {
    conn.query_row(
        r#"
        SELECT activated_at
        FROM session_context_rollovers
        WHERE session_id = ?1
          AND state = 'active'
          AND activated_at IS NOT NULL
        ORDER BY activated_at DESC, created_at DESC, id DESC
        LIMIT 1
        "#,
        params![session_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|value| parse_time(&value))
    .transpose()
}

fn attempt_usage_since_compact_conn(
    conn: &Connection,
    session_id: &str,
    compacted_at: DateTime<Utc>,
) -> Result<SessionLifetimeTokenUsage> {
    conn.query_row(
        r#"
        SELECT COALESCE(SUM(total_tokens), 0),
               COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(output_tokens), 0),
               COALESCE(SUM(cache_creation_tokens), 0),
               COALESCE(SUM(cache_read_tokens), 0),
               COALESCE(SUM(reasoning_tokens), 0),
               COALESCE(SUM(cost_usd), 0),
               COALESCE(SUM(CASE WHEN completeness = 'partial' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completeness = 'missing' THEN 1 ELSE 0 END), 0)
        FROM chat_run_attempts
        WHERE session_id = ?1
          AND created_at > ?2
          AND COALESCE(source, '') <> 'codex_app_server'
        "#,
        params![session_id, compacted_at.to_rfc3339()],
        map_lifetime_usage_row,
    )
    .map_err(StorageError::from)
}

fn attempt_spent_usage_for_session_conn(
    conn: &Connection,
    session_id: &str,
    created_after: Option<DateTime<Utc>>,
) -> Result<SessionLifetimeTokenUsage> {
    let after_filter = if created_after.is_some() {
        "AND created_at > ?2"
    } else {
        ""
    };
    let sql = format!(
        r#"
        SELECT COALESCE(SUM(total_tokens), 0),
               COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(output_tokens), 0),
               COALESCE(SUM(cache_creation_tokens), 0),
               COALESCE(SUM(cache_read_tokens), 0),
               COALESCE(SUM(reasoning_tokens), 0),
               COALESCE(SUM(cost_usd), 0),
               COALESCE(SUM(CASE WHEN completeness = 'partial' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completeness = 'missing' THEN 1 ELSE 0 END), 0)
        FROM chat_run_attempts
        WHERE session_id = ?1
          {after_filter}
          AND COALESCE(source, '') <> 'codex_app_server'
        "#
    );
    let created_after = created_after.map(|value| value.to_rfc3339());
    let mut usage = match created_after.as_deref() {
        Some(created_after) => conn.query_row(
            &sql,
            params![session_id, created_after],
            map_lifetime_usage_row,
        ),
        None => conn.query_row(&sql, params![session_id], map_lifetime_usage_row),
    }
    .map_err(StorageError::from)?;
    usage.completeness = lifetime_usage_completeness(&usage);
    Ok(usage)
}

#[derive(Debug)]
struct CodexUsageSnapshot {
    native_session_id: Option<String>,
    usage: SessionLifetimeTokenUsage,
}

fn codex_spent_attempt_usage_conn(
    conn: &Connection,
    session_id: &str,
    created_after: Option<DateTime<Utc>>,
) -> Result<SessionLifetimeTokenUsage> {
    let created_after = created_after.map(|value| value.to_rfc3339());
    let mut previous_by_native = created_after
        .as_deref()
        .map(|created_after| codex_usage_baselines_before_conn(conn, session_id, created_after))
        .transpose()?
        .unwrap_or_default();
    let snapshots = codex_usage_snapshots_conn(conn, session_id, created_after.as_deref())?;
    let mut spent = SessionLifetimeTokenUsage::default();
    for snapshot in snapshots {
        let key = snapshot.native_session_id.clone();
        let delta = match previous_by_native.get(&key) {
            Some(previous) if snapshot.usage.total >= previous.total => {
                subtract_lifetime_usage(snapshot.usage.clone(), previous.clone())
            }
            _ => snapshot.usage.clone(),
        };
        spent = combine_lifetime_usage(spent, delta);
        previous_by_native.insert(key, snapshot.usage);
    }
    let (partial_attempts, missing_attempts) =
        attempt_completeness_counts_conn(conn, session_id, created_after.as_deref())?;
    spent.partial_attempts = partial_attempts;
    spent.missing_attempts = missing_attempts;
    spent.completeness = lifetime_usage_completeness(&spent);
    Ok(spent)
}

fn codex_usage_baselines_before_conn(
    conn: &Connection,
    session_id: &str,
    created_before_or_at: &str,
) -> Result<HashMap<Option<String>, SessionLifetimeTokenUsage>> {
    let mut statement = conn.prepare(
        r#"
        SELECT native_session_id, total_tokens, input_tokens, output_tokens,
               cache_creation_tokens, cache_read_tokens, reasoning_tokens, cost_usd, 0, 0
        FROM chat_run_attempts
        WHERE session_id = ?1
          AND provider = 'codex'
          AND completed_at IS NOT NULL
          AND created_at <= ?2
          AND completeness <> 'missing'
          AND total_tokens > 0
          AND COALESCE(source, '') <> 'codex_app_server'
        ORDER BY completed_at ASC, created_at ASC, id ASC
        "#,
    )?;
    let rows = statement.query_map(params![session_id, created_before_or_at], |row| {
        Ok(CodexUsageSnapshot {
            native_session_id: row.get(0)?,
            usage: map_lifetime_usage_row_at(row, 1)?,
        })
    })?;
    let mut baselines = HashMap::new();
    for row in rows {
        let snapshot = row?;
        baselines.insert(snapshot.native_session_id, snapshot.usage);
    }
    Ok(baselines)
}

fn codex_usage_snapshots_conn(
    conn: &Connection,
    session_id: &str,
    created_after: Option<&str>,
) -> Result<Vec<CodexUsageSnapshot>> {
    let after_filter = if created_after.is_some() {
        "AND created_at > ?2"
    } else {
        ""
    };
    let sql = format!(
        r#"
        SELECT native_session_id, total_tokens, input_tokens, output_tokens,
               cache_creation_tokens, cache_read_tokens, reasoning_tokens, cost_usd, 0, 0
        FROM chat_run_attempts
        WHERE session_id = ?1
          AND provider = 'codex'
          AND completed_at IS NOT NULL
          {after_filter}
          AND completeness <> 'missing'
          AND total_tokens > 0
          AND COALESCE(source, '') <> 'codex_app_server'
        ORDER BY completed_at ASC, created_at ASC, id ASC
        "#
    );
    let mut statement = conn.prepare(&sql)?;
    let mapper = |row: &rusqlite::Row<'_>| {
        Ok(CodexUsageSnapshot {
            native_session_id: row.get(0)?,
            usage: map_lifetime_usage_row_at(row, 1)?,
        })
    };
    let rows = match created_after {
        Some(created_after) => statement.query_map(params![session_id, created_after], mapper)?,
        None => statement.query_map(params![session_id], mapper)?,
    };
    let mut snapshots = Vec::new();
    for row in rows {
        snapshots.push(row?);
    }
    Ok(snapshots)
}

fn codex_context_usage_delta_conn(
    conn: &Connection,
    session_id: &str,
    compacted_at: DateTime<Utc>,
) -> Result<SessionLifetimeTokenUsage> {
    let compacted_at = compacted_at.to_rfc3339();
    let latest = cumulative_attempt_usage_conn(
        conn,
        r#"
        SELECT total_tokens, input_tokens, output_tokens, cache_creation_tokens,
               cache_read_tokens, reasoning_tokens, cost_usd, 0, 0
        FROM chat_run_attempts
        WHERE session_id = ?1
          AND provider = 'codex'
          AND completed_at IS NOT NULL
          AND completed_at > ?2
          AND completeness <> 'missing'
          AND total_tokens > 0
          AND COALESCE(source, '') <> 'codex_app_server'
        ORDER BY completed_at DESC, created_at DESC, id DESC
        LIMIT 1
        "#,
        session_id,
        &compacted_at,
    )?;
    let baseline = cumulative_attempt_usage_conn(
        conn,
        r#"
        SELECT total_tokens, input_tokens, output_tokens, cache_creation_tokens,
               cache_read_tokens, reasoning_tokens, cost_usd, 0, 0
        FROM chat_run_attempts
        WHERE session_id = ?1
          AND provider = 'codex'
          AND completed_at IS NOT NULL
          AND completed_at <= ?2
          AND completeness <> 'missing'
          AND total_tokens > 0
          AND COALESCE(source, '') <> 'codex_app_server'
        ORDER BY completed_at DESC, created_at DESC, id DESC
        LIMIT 1
        "#,
        session_id,
        &compacted_at,
    )?;
    let mut usage =
        subtract_lifetime_usage(latest.unwrap_or_default(), baseline.unwrap_or_default());
    let (partial_attempts, missing_attempts) =
        attempt_completeness_counts_conn(conn, session_id, Some(&compacted_at))?;
    usage.partial_attempts = partial_attempts;
    usage.missing_attempts = missing_attempts;
    usage.completeness = lifetime_usage_completeness(&usage);
    Ok(usage)
}

fn codex_latest_cumulative_usage_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionLifetimeTokenUsage>> {
    conn.query_row(
        r#"
        SELECT total_tokens, input_tokens, output_tokens, cache_creation_tokens,
               cache_read_tokens, reasoning_tokens, cost_usd, 0, 0
        FROM chat_run_attempts
        WHERE session_id = ?1
          AND provider = 'codex'
          AND completed_at IS NOT NULL
          AND completeness <> 'missing'
          AND total_tokens > 0
          AND COALESCE(source, '') <> 'codex_app_server'
        ORDER BY completed_at DESC, created_at DESC, id DESC
        LIMIT 1
        "#,
        params![session_id],
        map_lifetime_usage_row,
    )
    .optional()
    .map_err(StorageError::from)
}

fn cumulative_attempt_usage_conn(
    conn: &Connection,
    sql: &str,
    session_id: &str,
    compacted_at: &str,
) -> Result<Option<SessionLifetimeTokenUsage>> {
    conn.query_row(
        sql,
        params![session_id, compacted_at],
        map_lifetime_usage_row,
    )
    .optional()
    .map_err(StorageError::from)
}

fn attempt_completeness_counts_conn(
    conn: &Connection,
    session_id: &str,
    created_after: Option<&str>,
) -> Result<(u64, u64)> {
    let after_filter = if created_after.is_some() {
        "AND created_at > ?2"
    } else {
        ""
    };
    let sql = format!(
        r#"
        SELECT COALESCE(SUM(CASE WHEN completeness = 'partial' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completeness = 'missing' THEN 1 ELSE 0 END), 0)
        FROM chat_run_attempts
        WHERE session_id = ?1
          {after_filter}
          AND COALESCE(source, '') <> 'codex_app_server'
        "#
    );
    let mapper = |row: &rusqlite::Row<'_>| Ok((row_i64_to_u64(row, 0)?, row_i64_to_u64(row, 1)?));
    match created_after {
        Some(created_after) => conn.query_row(&sql, params![session_id, created_after], mapper),
        None => conn.query_row(&sql, params![session_id], mapper),
    }
    .map_err(StorageError::from)
}

fn subtract_lifetime_usage(
    latest: SessionLifetimeTokenUsage,
    baseline: SessionLifetimeTokenUsage,
) -> SessionLifetimeTokenUsage {
    let mut usage = SessionLifetimeTokenUsage {
        total: latest.total.saturating_sub(baseline.total),
        input: latest.input.saturating_sub(baseline.input),
        output: latest.output.saturating_sub(baseline.output),
        cache_creation: latest
            .cache_creation
            .saturating_sub(baseline.cache_creation),
        cache_read: latest.cache_read.saturating_sub(baseline.cache_read),
        reasoning: latest.reasoning.saturating_sub(baseline.reasoning),
        cost_usd: (latest.cost_usd - baseline.cost_usd).max(0.0),
        partial_attempts: 0,
        missing_attempts: 0,
        completeness: TokenUsageCompleteness::Complete,
    };
    usage.completeness = lifetime_usage_completeness(&usage);
    usage
}

fn context_usage_from_lifetime(
    usage: SessionLifetimeTokenUsage,
    after_compact: bool,
    compacted_at: Option<DateTime<Utc>>,
) -> SessionContextTokenUsage {
    SessionContextTokenUsage {
        total: usage.total,
        input: usage.input,
        output: usage.output,
        cache_creation: usage.cache_creation,
        cache_read: usage.cache_read,
        reasoning: usage.reasoning,
        cost_usd: usage.cost_usd,
        completeness: usage.completeness,
        partial_attempts: usage.partial_attempts,
        missing_attempts: usage.missing_attempts,
        after_compact,
        compacted_at,
    }
}

fn attach_session_usage_conn(conn: &Connection, sessions: &mut [SessionSummary]) -> Result<()> {
    if sessions.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", sessions.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        r#"
        SELECT session_id,
               COALESCE(SUM(total_tokens), 0),
               COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(output_tokens), 0),
               COALESCE(SUM(cache_creation_tokens), 0),
               COALESCE(SUM(cache_read_tokens), 0),
               COALESCE(SUM(reasoning_tokens), 0),
               COALESCE(SUM(cost_usd), 0),
               COALESCE(SUM(partial_attempts), 0),
               COALESCE(SUM(missing_attempts), 0)
        FROM (
            SELECT session_id, total_tokens, input_tokens, output_tokens,
                   cache_creation_tokens, cache_read_tokens, reasoning_tokens,
                   cost_usd,
                   CASE WHEN completeness = 'partial' THEN 1 ELSE 0 END AS partial_attempts,
                   CASE WHEN completeness = 'missing' THEN 1 ELSE 0 END AS missing_attempts
            FROM chat_run_attempts

            UNION ALL

            SELECT session_id, total_tokens, input_tokens, output_tokens,
                   cache_creation_tokens, cache_read_tokens, reasoning_tokens,
                   cost_usd, partial_attempts, missing_attempts
            FROM session_usage_baselines
        ) usage
        WHERE session_id IN ({placeholders})
        GROUP BY session_id
        "#,
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        params_from_iter(sessions.iter().map(|session| session.id.as_str())),
        |row| Ok((row.get::<_, String>(0)?, map_lifetime_usage_row_at(row, 1)?)),
    )?;
    let mut usage_by_session = HashMap::new();
    for row in rows {
        let (session_id, usage) = row?;
        usage_by_session.insert(session_id, usage);
    }
    for session in sessions {
        session.lifetime_token_usage =
            Some(usage_by_session.remove(&session.id).unwrap_or_default());
        session.context_token_usage = Some(session_context_token_usage_conn(conn, &session.id)?);
        session.spent_token_usage = Some(session_spent_token_usage_conn(conn, &session.id)?);
    }
    Ok(())
}

fn lifetime_attempt_usage_for_session_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionLifetimeTokenUsage> {
    conn.query_row(
        r#"
        SELECT COALESCE(SUM(total_tokens), 0),
               COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(output_tokens), 0),
               COALESCE(SUM(cache_creation_tokens), 0),
               COALESCE(SUM(cache_read_tokens), 0),
               COALESCE(SUM(reasoning_tokens), 0),
               COALESCE(SUM(cost_usd), 0),
               COALESCE(SUM(CASE WHEN completeness = 'partial' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completeness = 'missing' THEN 1 ELSE 0 END), 0)
        FROM chat_run_attempts
        WHERE session_id = ?1
        "#,
        params![session_id],
        map_lifetime_usage_row,
    )
    .map_err(StorageError::from)
}

fn fork_usage_baseline_conn(
    conn: &Connection,
    source_session_id: &str,
    before_message_id: &str,
    destination: &SessionSummary,
    messages: &[ChatMessage],
) -> Result<SessionLifetimeTokenUsage> {
    let mut usage_sources = messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .map(|message| usage_source_ref(source_session_id, message))
        .collect::<Vec<_>>();
    usage_sources.sort();
    usage_sources.dedup();

    if usage_sources.is_empty() {
        return Ok(SessionLifetimeTokenUsage::default());
    }

    let source_conditions = std::iter::repeat_n(
        "(a.session_id = ? AND a.user_message_id = ?)",
        usage_sources.len(),
    )
    .collect::<Vec<_>>()
    .join(" OR ");
    let baseline_conditions = std::iter::repeat_n(
        "(f.source_session_id = ? AND f.before_message_id = ?)",
        usage_sources.len(),
    )
    .collect::<Vec<_>>()
    .join(" OR ");
    let sql = format!(
        r#"
        SELECT COALESCE(SUM(total_tokens), 0),
               COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(output_tokens), 0),
               COALESCE(SUM(cache_creation_tokens), 0),
               COALESCE(SUM(cache_read_tokens), 0),
               COALESCE(SUM(reasoning_tokens), 0),
               COALESCE(SUM(cost_usd), 0),
               COALESCE(SUM(partial_attempts), 0),
               COALESCE(SUM(missing_attempts), 0)
        FROM (
            SELECT a.total_tokens, a.input_tokens, a.output_tokens,
                   a.cache_creation_tokens, a.cache_read_tokens,
                   a.reasoning_tokens, a.cost_usd,
                   CASE WHEN a.completeness = 'partial' THEN 1 ELSE 0 END AS partial_attempts,
                   CASE WHEN a.completeness = 'missing' THEN 1 ELSE 0 END AS missing_attempts
            FROM chat_run_attempts a
            WHERE {source_conditions}

            UNION ALL

            SELECT b.total_tokens, b.input_tokens, b.output_tokens,
                   b.cache_creation_tokens, b.cache_read_tokens,
                   b.reasoning_tokens, b.cost_usd,
                   b.partial_attempts, b.missing_attempts
            FROM session_usage_baselines b
            JOIN session_forks f ON f.destination_session_id = b.session_id
            WHERE {baseline_conditions}
        )
        "#
    );
    let bind_values = usage_sources
        .iter()
        .flat_map(|(session_id, message_id)| [session_id.clone(), message_id.clone()])
        .chain(
            usage_sources
                .iter()
                .flat_map(|(session_id, message_id)| [session_id.clone(), message_id.clone()]),
        );
    let mut combined =
        conn.query_row(&sql, params_from_iter(bind_values), map_lifetime_usage_row)?;
    combined.completeness = lifetime_usage_completeness(&combined);

    if combined.total == 0
        && combined.partial_attempts == 0
        && combined.missing_attempts == 0
        && destination.lifetime_token_usage.is_some()
    {
        return Ok(destination.lifetime_token_usage.clone().unwrap_or_default());
    }

    let _ = before_message_id;
    Ok(combined)
}

fn usage_source_ref(source_session_id: &str, message: &ChatMessage) -> (String, String) {
    let session_id = message
        .metadata
        .get("usageSourceSessionId")
        .or_else(|| message.metadata.get("forkedFromSessionId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| source_session_id.to_string());
    let message_id = message
        .metadata
        .get("usageSourceMessageId")
        .or_else(|| message.metadata.get("forkedFromMessageId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| message.id.clone());
    (session_id, message_id)
}

fn map_lifetime_usage_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionLifetimeTokenUsage> {
    map_lifetime_usage_row_at(row, 0)
}

fn map_lifetime_usage_row_at(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<SessionLifetimeTokenUsage> {
    let mut usage = SessionLifetimeTokenUsage {
        total: row_i64_to_u64(row, offset)?,
        input: row_i64_to_u64(row, offset + 1)?,
        output: row_i64_to_u64(row, offset + 2)?,
        cache_creation: row_i64_to_u64(row, offset + 3)?,
        cache_read: row_i64_to_u64(row, offset + 4)?,
        reasoning: row_i64_to_u64(row, offset + 5)?,
        cost_usd: row.get(offset + 6)?,
        partial_attempts: row_i64_to_u64(row, offset + 7)?,
        missing_attempts: row_i64_to_u64(row, offset + 8)?,
        completeness: TokenUsageCompleteness::Complete,
    };
    usage.completeness = lifetime_usage_completeness(&usage);
    Ok(usage)
}

fn row_i64_to_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let raw = row.get::<_, i64>(index)?;
    u64::try_from(raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn combine_lifetime_usage(
    mut left: SessionLifetimeTokenUsage,
    right: SessionLifetimeTokenUsage,
) -> SessionLifetimeTokenUsage {
    left.total = left.total.saturating_add(right.total);
    left.input = left.input.saturating_add(right.input);
    left.output = left.output.saturating_add(right.output);
    left.cache_creation = left.cache_creation.saturating_add(right.cache_creation);
    left.cache_read = left.cache_read.saturating_add(right.cache_read);
    left.reasoning = left.reasoning.saturating_add(right.reasoning);
    left.cost_usd += right.cost_usd;
    left.partial_attempts = left.partial_attempts.saturating_add(right.partial_attempts);
    left.missing_attempts = left.missing_attempts.saturating_add(right.missing_attempts);
    left.completeness = lifetime_usage_completeness(&left);
    left
}

fn lifetime_usage_completeness(usage: &SessionLifetimeTokenUsage) -> TokenUsageCompleteness {
    if usage.missing_attempts > 0 && usage.total == 0 {
        TokenUsageCompleteness::Missing
    } else if usage.missing_attempts > 0 || usage.partial_attempts > 0 {
        TokenUsageCompleteness::Partial
    } else {
        TokenUsageCompleteness::Complete
    }
}

fn token_usage_completeness_to_str(value: TokenUsageCompleteness) -> &'static str {
    match value {
        TokenUsageCompleteness::Complete => "complete",
        TokenUsageCompleteness::Partial => "partial",
        TokenUsageCompleteness::Missing => "missing",
    }
}

fn map_durable_chat_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredDurableChatRun> {
    let thinking = row.get::<_, Option<i64>>(10)?.map(|value| value != 0);
    let fast = row.get::<_, Option<i64>>(21)?.map(|value| value != 0);
    let resume_attempts_raw = row.get::<_, i64>(13)?;
    let resume_attempts = u32::try_from(resume_attempts_raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            13,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    let recovered_at = row
        .get::<_, Option<String>>(17)?
        .map(parse_time_sql)
        .transpose()?;
    let completed_at = row
        .get::<_, Option<String>>(18)?
        .map(parse_time_sql)
        .transpose()?;

    Ok(StoredDurableChatRun {
        id: row.get(0)?,
        user_id: row.get(1)?,
        session_id: row.get(2)?,
        native_session_id: row.get(3)?,
        user_message_id: row.get(19)?,
        native_before_turn_id: row.get(20)?,
        provider: row.get(4)?,
        prompt: row.get(5)?,
        project_path: row.get(6)?,
        model: row.get(7)?,
        effort: row.get(8)?,
        mode: row.get(9)?,
        thinking,
        fast,
        status: row.get(11)?,
        auto_resume: row.get::<_, i64>(12)? != 0,
        resume_attempts,
        last_error: row.get(14)?,
        created_at: parse_time_sql(row.get::<_, String>(15)?)?,
        updated_at: parse_time_sql(row.get::<_, String>(16)?)?,
        recovered_at,
        completed_at,
    })
}

fn map_session_context_rollover_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StoredSessionContextRollover> {
    let observed_bytes = row
        .get::<_, Option<i64>>(12)?
        .map(u64::try_from)
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                12,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?;
    let limit_bytes_raw = row.get::<_, i64>(13)?;
    let limit_bytes = u64::try_from(limit_bytes_raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            13,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    Ok(StoredSessionContextRollover {
        id: row.get(0)?,
        user_id: row.get(1)?,
        session_id: row.get(2)?,
        request_id: row.get(3)?,
        kind: row.get(4)?,
        failed_message_id: row.get(5)?,
        trigger_run_id: row.get(6)?,
        retry_run_id: row.get(7)?,
        from_native_session_id: row.get(8)?,
        candidate_native_session_id: row.get(9)?,
        state: row.get(10)?,
        handoff: row.get(11)?,
        observed_bytes,
        limit_bytes,
        error: row.get(14)?,
        created_at: parse_time_sql(row.get::<_, String>(15)?)?,
        updated_at: parse_time_sql(row.get::<_, String>(16)?)?,
        activated_at: row
            .get::<_, Option<String>>(17)?
            .map(parse_time_sql)
            .transpose()?,
    })
}

fn map_fcm_token_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredFcmToken> {
    Ok(StoredFcmToken {
        token: row.get(0)?,
        user_id: row.get(1)?,
        platform: row.get(2)?,
        device_id: row.get(3)?,
        app_id: row.get(4)?,
        created_at: parse_time_sql(row.get::<_, String>(5)?)?,
        updated_at: parse_time_sql(row.get::<_, String>(6)?)?,
        last_seen_at: parse_time_sql(row.get::<_, String>(7)?)?,
    })
}

fn map_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionSummary> {
    let mut session = SessionSummary {
        id: row.get(0)?,
        provider: parse_provider(&row.get::<_, String>(1)?),
        project_path: row.get(2)?,
        title: row.get(3)?,
        message_count: row.get::<_, i64>(4)? as usize,
        last_activity: parse_time_sql(row.get::<_, String>(5)?)?,
        active: row.get::<_, i64>(6)? == 1,
        model: row.get(7)?,
        ..Default::default()
    };
    let metadata_blob: Option<String> = row.get(8).ok();
    if let Some(raw) = metadata_blob {
        if let Some(parsed) = deserialize_session_metadata(&raw) {
            merge_metadata_into(&mut session, parsed);
        }
    }
    Ok(session)
}

fn get_session_summary_conn(conn: &Connection, session_id: &str) -> Result<Option<SessionSummary>> {
    conn.query_row(
        r#"
        SELECT s.id, s.provider, s.project_path, s.title,
               COALESCE(m.message_count, s.message_count),
               s.last_activity, s.active, s.model, s.metadata
        FROM sessions s
        LEFT JOIN (
            SELECT session_id, COUNT(*) AS message_count
            FROM messages
            WHERE session_id = ?1
            GROUP BY session_id
        ) m ON m.session_id = s.id
        WHERE s.id = ?1
        "#,
        params![session_id],
        map_session_row,
    )
    .optional()
    .map_err(StorageError::from)
}

fn serialize_session_metadata(session: &SessionSummary) -> String {
    use serde_json::json;
    let mut value = serde_json::Map::new();
    if session.external {
        value.insert("external".into(), json!(true));
    }
    if session.is_board_session() {
        value.insert("boardSession".into(), json!(true));
    }
    if let Some(board_id) = session.board_id.as_ref() {
        value.insert("boardId".into(), json!(board_id));
    }
    if let Some(board_task_id) = session.board_task_id.as_ref() {
        value.insert("boardTaskId".into(), json!(board_task_id));
    }
    if let Some(native_session_id) = session.native_session_id.as_ref() {
        value.insert("nativeSessionId".into(), json!(native_session_id));
    }
    if session.native_rollout_owned_by_provider {
        value.insert("nativeRolloutOwnedByProvider".into(), json!(true));
    }
    if let Some(title_source) = session.title_source {
        value.insert("titleSource".into(), json!(title_source));
    }
    if let Some(model) = session.model.as_ref() {
        value.insert("model".into(), json!(model));
    }
    if let Some(runtime) = session.runtime {
        value.insert("runtime".into(), json!(runtime));
    }
    if let Some(effort) = session.effort.as_ref() {
        value.insert("effort".into(), json!(effort));
    }
    if let Some(mode) = session.mode.as_ref() {
        value.insert("mode".into(), json!(mode));
    }
    if let Some(thinking) = session.thinking {
        value.insert("thinking".into(), json!(thinking));
    }
    if let Some(fast) = session.fast {
        value.insert("fast".into(), json!(fast));
    }
    if let Some(at) = session.last_message_at {
        value.insert("lastMessageAt".into(), json!(at));
    }
    if let Some(at) = session.first_user_at {
        value.insert("firstUserAt".into(), json!(at));
    }
    if let Some(at) = session.received_at {
        value.insert("receivedAt".into(), json!(at));
    }
    if let Some(usage) = session.token_usage.as_ref() {
        value.insert(
            "tokenUsage".into(),
            serde_json::to_value(usage).unwrap_or(serde_json::Value::Null),
        );
    }
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
}

fn deserialize_session_metadata(raw: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(raw).ok()
}

fn merge_metadata_patch(target: &mut serde_json::Value, patch: serde_json::Value) {
    match (target, patch) {
        (serde_json::Value::Object(target), serde_json::Value::Object(patch)) => {
            target.extend(patch);
        }
        (target, patch) => *target = patch,
    }
}

fn merge_metadata_into(session: &mut SessionSummary, value: serde_json::Value) {
    use serde_json::Value;
    if let Some(v) = value.get("external").and_then(Value::as_bool) {
        session.external = v;
    }
    if let Some(v) = value.get("boardSession").and_then(Value::as_bool) {
        session.board_session = v;
    }
    if let Some(v) = value
        .get("boardId")
        .or_else(|| value.get("boardRunId"))
        .and_then(Value::as_str)
    {
        session.board_id = Some(v.to_string());
    }
    if let Some(v) = value.get("boardTaskId").and_then(Value::as_str) {
        session.board_task_id = Some(v.to_string());
    }
    if session.is_board_session() {
        session.board_session = true;
    }
    if let Some(v) = value.get("nativeSessionId").and_then(Value::as_str) {
        session.native_session_id = Some(v.to_string());
    }
    if let Some(v) = value
        .get("nativeRolloutOwnedByProvider")
        .and_then(Value::as_bool)
    {
        session.native_rollout_owned_by_provider = v;
    }
    if let Some(v) = value.get("titleSource") {
        session.title_source = serde_json::from_value(v.clone()).ok();
    }
    if let Some(v) = value.get("model").and_then(Value::as_str) {
        session.model = Some(v.to_string());
    }
    if let Some(v) = value.get("runtime") {
        session.runtime = serde_json::from_value(v.clone()).ok();
    }
    if let Some(v) = value.get("effort").and_then(Value::as_str) {
        session.effort = Some(v.to_string());
    }
    if let Some(v) = value.get("mode").and_then(Value::as_str) {
        session.mode = Some(v.to_string());
    }
    if let Some(v) = value.get("thinking").and_then(Value::as_bool) {
        session.thinking = Some(v);
    }
    if let Some(v) = value.get("fast").and_then(Value::as_bool) {
        session.fast = Some(v);
    }
    if let Some(v) = value.get("lastMessageAt").and_then(Value::as_str) {
        if let Ok(ts) = parse_time(v) {
            session.last_message_at = Some(ts);
        }
    }
    if let Some(v) = value.get("firstUserAt").and_then(Value::as_str) {
        if let Ok(ts) = parse_time(v) {
            session.first_user_at = Some(ts);
        }
    }
    if let Some(v) = value.get("receivedAt").and_then(Value::as_str) {
        if let Ok(ts) = parse_time(v) {
            session.received_at = Some(ts);
        }
    }
    if let Some(v) = value.get("tokenUsage") {
        if let Ok(usage) = serde_json::from_value::<iowb_protocol::SessionTokenUsage>(v.clone()) {
            session.token_usage = Some(usage);
        }
    }
}

fn legacy_session_title_from_prompt(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.chars().count() <= 50 {
        return trimmed.to_string();
    }

    format!("{}...", trimmed.chars().take(50).collect::<String>())
}

fn map_user_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredUser> {
    let last_login_raw: Option<String> = row.get(5)?;
    Ok(StoredUser {
        id: row.get(0)?,
        username: row.get(1)?,
        password_hash: row.get(2)?,
        created_at: parse_time_sql(row.get::<_, String>(3)?)?,
        updated_at: parse_time_sql(row.get::<_, String>(4)?)?,
        last_login_at: last_login_raw.map(parse_time_sql).transpose()?,
    })
}

fn map_api_key_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApiKeyRecord> {
    let key_prefix: String = row.get(2)?;
    Ok(ApiKeyRecord {
        id: row.get(0)?,
        key_name: row.get(1)?,
        masked_key: mask_secret(&key_prefix),
        key_prefix,
        is_active: row.get::<_, i64>(3)? == 1,
        created_at: parse_time_sql(row.get::<_, String>(4)?)?,
        updated_at: parse_time_sql(row.get::<_, String>(5)?)?,
    })
}

fn map_credential_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CredentialRecord> {
    Ok(CredentialRecord {
        id: row.get(0)?,
        credential_name: row.get(1)?,
        credential_type: row.get(2)?,
        description: row.get(3)?,
        is_active: row.get::<_, i64>(4)? == 1,
        created_at: parse_time_sql(row.get::<_, String>(5)?)?,
        updated_at: parse_time_sql(row.get::<_, String>(6)?)?,
    })
}

fn map_database_connection_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StoredDatabaseConnection> {
    let db_type_raw: String = row.get(2)?;
    let port_raw: Option<i64> = row.get(4)?;
    let password: Option<String> = row.get(6)?;
    let last_test_status_raw: Option<String> = row.get(10)?;
    let last_tested_at_raw: Option<String> = row.get(12)?;

    Ok(StoredDatabaseConnection {
        profile: DatabaseConnectionProfile {
            id: row.get(0)?,
            name: row.get(1)?,
            db_type: parse_database_type(&db_type_raw),
            host: row.get(3)?,
            port: port_raw.and_then(|value| u16::try_from(value).ok()),
            username: row.get(5)?,
            database_name: row.get(7)?,
            file_path: row.get(8)?,
            show_all_databases: row.get::<_, i64>(9)? == 1,
            has_password: password.as_deref().is_some_and(|value| !value.is_empty()),
            last_test_status: last_test_status_raw
                .as_deref()
                .map(parse_database_test_status),
            last_test_message: row.get(11)?,
            last_tested_at: last_tested_at_raw.map(parse_time_sql).transpose()?,
            created_at: parse_time_sql(row.get::<_, String>(13)?)?,
            updated_at: parse_time_sql(row.get::<_, String>(14)?)?,
        },
        password,
    })
}

fn mask_secret(prefix: &str) -> String {
    format!("{prefix}...")
}

fn bounded_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn nonnegative_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn nonnegative_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

fn external_message_state_matches(
    conn: &Connection,
    provider: Provider,
    session_id: &str,
    file_path: &str,
    fingerprint: &ExternalHistoryFingerprint<'_>,
) -> Result<bool> {
    Ok(
        external_message_state_total_if_matches(
            conn,
            provider,
            session_id,
            file_path,
            fingerprint,
        )?
        .is_some(),
    )
}

fn external_message_state_total_if_matches(
    conn: &Connection,
    provider: Provider,
    session_id: &str,
    file_path: &str,
    fingerprint: &ExternalHistoryFingerprint<'_>,
) -> Result<Option<usize>> {
    let state = conn
        .query_row(
            r#"
            SELECT file_identity, file_size, modified_nanos, parser_version,
                   total_count
            FROM external_history_message_state
            WHERE provider = ?1 AND session_id = ?2 AND file_path = ?3
            "#,
            params![provider.as_str(), session_id, file_path],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((identity, size, modified_nanos, parser_version, total_count)) = state else {
        return Ok(None);
    };
    if identity.as_deref() != fingerprint.file_identity
        || nonnegative_u64(size) != fingerprint.file_size
        || modified_nanos != fingerprint.modified_nanos
        || nonnegative_u32(parser_version) != fingerprint.parser_version
    {
        return Ok(None);
    }
    Ok(Some(
        usize::try_from(nonnegative_u64(total_count)).unwrap_or(usize::MAX),
    ))
}

fn parse_provider(raw: &str) -> Provider {
    match raw {
        "codex" => Provider::Codex,
        "gemini" => Provider::Gemini,
        _ => Provider::Claude,
    }
}

fn parse_database_type(raw: &str) -> SupportedDatabaseType {
    match raw {
        "postgresql" => SupportedDatabaseType::Postgresql,
        "mysql" => SupportedDatabaseType::Mysql,
        "mariadb" => SupportedDatabaseType::Mariadb,
        _ => SupportedDatabaseType::Sqlite,
    }
}

fn parse_database_test_status(raw: &str) -> DatabaseTestStatus {
    match raw {
        "success" => DatabaseTestStatus::Success,
        _ => DatabaseTestStatus::Error,
    }
}

fn database_test_status_to_str(status: DatabaseTestStatus) -> &'static str {
    match status {
        DatabaseTestStatus::Success => "success",
        DatabaseTestStatus::Error => "error",
    }
}

fn role_to_str(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn parse_role(raw: &str) -> MessageRole {
    match raw {
        "system" => MessageRole::System,
        "assistant" => MessageRole::Assistant,
        "tool" => MessageRole::Tool,
        _ => MessageRole::User,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iowb_protocol::{ChatRuntime, SessionTokenUsage};
    use std::collections::HashSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_storage(label: &str) -> (Storage, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "iowb-storage-{label}-{}-{unique}",
            std::process::id()
        ));
        let storage = Storage::open(root.join("test.db")).expect("storage");
        (storage, root)
    }

    fn test_session(id: &str, active: bool) -> SessionSummary {
        SessionSummary {
            id: id.to_string(),
            provider: Provider::Codex,
            project_path: "/tmp/project".to_string(),
            title: "Test session".to_string(),
            last_activity: Utc::now(),
            active,
            ..Default::default()
        }
    }

    fn test_message(id: &str, role: MessageRole, content: &str, seconds: i64) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            role,
            content: content.to_string(),
            timestamp: Utc::now() + chrono::Duration::seconds(seconds),
            metadata: Value::Null,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_completed_usage_attempt(
        storage: &Storage,
        session: &SessionSummary,
        attempt_id: &str,
        run_id: &str,
        created_at: DateTime<Utc>,
        usage: SessionTokenUsage,
        source: &str,
    ) {
        insert_completed_usage_attempt_with_native(
            storage,
            session,
            attempt_id,
            run_id,
            created_at,
            usage,
            source,
            Some("native-1"),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_completed_usage_attempt_with_native(
        storage: &Storage,
        session: &SessionSummary,
        attempt_id: &str,
        run_id: &str,
        created_at: DateTime<Utc>,
        usage: SessionTokenUsage,
        source: &str,
        native_session_id: Option<&str>,
    ) {
        let mut run = StoredDurableChatRun::new(
            run_id,
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            "prompt",
            session.project_path.clone(),
        );
        run.user_message_id = Some(format!("message-{attempt_id}"));
        run.completed_at = Some(created_at);
        run.status = "completed".to_string();
        storage.create_durable_chat_run(&run).expect("run");
        let mut attempt = StoredChatRunAttempt::new(
            attempt_id,
            run.id.clone(),
            session.id.clone(),
            run.user_message_id.clone(),
            "codex",
            "native_cli",
            Some("gpt-test".to_string()),
            native_session_id.map(str::to_string),
        );
        attempt.status = "completed".to_string();
        attempt.usage = Some(usage.clone());
        attempt.raw_usage_json = Some(format!(r#"{{"total_tokens":{}}}"#, usage.used));
        attempt.source = Some(source.to_string());
        attempt.completeness = TokenUsageCompleteness::Complete;
        attempt.created_at = created_at;
        attempt.updated_at = created_at;
        attempt.completed_at = Some(created_at);
        storage.create_chat_run_attempt(&attempt).expect("attempt");
    }

    fn test_context_rollover(
        id: &str,
        session_id: &str,
        request_id: &str,
        trigger_run_id: &str,
        retry_run_id: &str,
        failed_message_id: &str,
        created_at: DateTime<Utc>,
    ) -> StoredSessionContextRollover {
        StoredSessionContextRollover {
            id: id.to_string(),
            user_id: "user-1".to_string(),
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            kind: "retry_failed_turn".to_string(),
            failed_message_id: failed_message_id.to_string(),
            trigger_run_id: trigger_run_id.to_string(),
            retry_run_id: retry_run_id.to_string(),
            from_native_session_id: Some("native-poisoned".to_string()),
            candidate_native_session_id: None,
            state: "starting".to_string(),
            handoff: "bounded text-only handoff".to_string(),
            observed_bytes: Some(19_760_000),
            limit_bytes: 16 * 1024 * 1024,
            error: None,
            created_at,
            updated_at: created_at,
            activated_at: None,
        }
    }

    #[test]
    fn external_history_index_and_messages_survive_reopen() {
        let (storage, root) = temporary_storage("external-history-index");
        let database = root.join("test.db");
        let mut summary = test_session("external-session", false);
        summary.external = true;
        summary.message_count = 3;
        let source = StoredExternalHistorySource {
            provider: Provider::Codex,
            source_path: "/tmp/state.sqlite".to_string(),
            file_identity: Some("1:2".to_string()),
            file_size: 42,
            modified_nanos: Some(99),
            scan_offset: 42,
            parser_version: 1,
            records: vec![StoredExternalSessionRecord {
                summary: summary.clone(),
                file_path: "/tmp/rollout.jsonl".to_string(),
            }],
        };
        storage
            .upsert_external_history_source(&source)
            .expect("persist source");

        let messages = vec![
            test_message("external-0", MessageRole::User, "question", 0),
            test_message("external-1", MessageRole::Assistant, "answer", 1),
            test_message("external-2", MessageRole::Tool, "tool", 2),
        ];
        let fingerprint = ExternalHistoryFingerprint {
            file_identity: Some("3:4"),
            file_size: 123,
            modified_nanos: Some(456),
            parser_version: 1,
        };
        storage
            .replace_external_messages(
                Provider::Codex,
                &summary.id,
                "/tmp/rollout.jsonl",
                &fingerprint,
                &messages,
            )
            .expect("persist messages");
        drop(storage);

        let reopened = Storage::open(database).expect("reopen storage");
        let restored = reopened
            .external_history_source(Provider::Codex, "/tmp/state.sqlite")
            .expect("load source")
            .expect("source");
        assert_eq!(restored.records.len(), 1);
        assert_eq!(restored.records[0].summary.id, summary.id);
        assert_eq!(restored.records[0].summary.message_count, 3);
        let tail = reopened
            .external_messages_tail_if_current(
                Provider::Codex,
                &summary.id,
                "/tmp/rollout.jsonl",
                &fingerprint,
                2,
            )
            .expect("load tail")
            .expect("current tail");
        assert_eq!(tail.1, 3);
        assert_eq!(
            tail.0
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["external-1", "external-2"]
        );
        let stale_fingerprint = ExternalHistoryFingerprint {
            file_size: 124,
            ..fingerprint
        };
        assert!(
            reopened
                .external_messages_if_current(
                    Provider::Codex,
                    &summary.id,
                    "/tmp/rollout.jsonl",
                    &stale_fingerprint,
                )
                .expect("stale lookup")
                .is_none()
        );

        drop(reopened);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn legacy_durable_chat_runs_schema_migrates_missing_turn_columns() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "iowb-storage-legacy-durable-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let database = root.join("test.db");

        {
            let conn = Connection::open(&database).expect("legacy connection");
            conn.execute_batch(
                r#"
                CREATE TABLE durable_chat_runs (
                    id TEXT PRIMARY KEY,
                    user_id TEXT,
                    session_id TEXT NOT NULL,
                    native_session_id TEXT,
                    provider TEXT NOT NULL,
                    prompt TEXT NOT NULL,
                    project_path TEXT NOT NULL,
                    model TEXT,
                    effort TEXT,
                    mode TEXT,
                    thinking INTEGER,
                    status TEXT NOT NULL,
                    auto_resume INTEGER NOT NULL DEFAULT 1,
                    resume_attempts INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    recovered_at TEXT,
                    completed_at TEXT
                );

                CREATE INDEX idx_durable_chat_runs_recoverable
                    ON durable_chat_runs(status, auto_resume, resume_attempts, updated_at);

                CREATE INDEX idx_durable_chat_runs_session
                    ON durable_chat_runs(session_id, created_at DESC);
                "#,
            )
            .expect("create legacy schema");
        }

        let storage = Storage::open(&database).expect("migrate legacy durable schema");
        storage
            .with_connection(|conn| {
                for column in ["user_message_id", "native_before_turn_id", "fast"] {
                    let present: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('durable_chat_runs') WHERE name = ?1",
                        params![column],
                        |row| row.get(0),
                    )?;
                    assert_eq!(present, 1, "missing migrated column {column}");
                }

                let index_present: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM pragma_index_list('durable_chat_runs') WHERE name = 'idx_durable_chat_runs_user_message'",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(index_present, 1);
                Ok(())
            })
            .expect("inspect migrated schema");

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn durable_chat_turn_is_atomic_and_indexes_native_identity() {
        let (storage, root) = temporary_storage("durable-turn-atomic");
        let mut session = test_session("session-turn", true);
        let message = test_message("message-turn", MessageRole::User, "prompt", 0);
        session.message_count = 1;
        let mut run = StoredDurableChatRun::new(
            "run-turn",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            message.content.clone(),
            session.project_path.clone(),
        );
        run.user_message_id = Some(message.id.clone());
        run.native_before_turn_id = Some("native-turn-before".to_string());

        storage
            .create_durable_chat_turn(&session, &message, &run)
            .expect("create durable turn");
        let stored_messages = storage.list_messages(&session.id).expect("messages");
        assert_eq!(stored_messages.len(), 1);
        assert_eq!(stored_messages[0].id, message.id);
        assert_eq!(stored_messages[0].role, MessageRole::User);
        assert_eq!(stored_messages[0].content, "prompt");
        let restored = storage
            .durable_chat_run_for_user_message(&session.id, "message-turn")
            .expect("durable lookup")
            .expect("durable run");
        assert_eq!(restored.id, "run-turn");
        assert_eq!(
            restored.native_before_turn_id.as_deref(),
            Some("native-turn-before")
        );

        let duplicate = test_message("message-turn", MessageRole::User, "duplicate", 1);
        let mut failed_session = test_session("session-rolled-back", true);
        failed_session.message_count = 1;
        let mut failed_run = StoredDurableChatRun::new(
            "run-rolled-back",
            None,
            failed_session.id.clone(),
            "codex",
            duplicate.content.clone(),
            failed_session.project_path.clone(),
        );
        failed_run.user_message_id = Some(duplicate.id.clone());
        assert!(
            storage
                .create_durable_chat_turn(&failed_session, &duplicate, &failed_run)
                .is_err()
        );
        assert!(
            storage
                .get_session(&failed_session.id)
                .expect("rolled-back session lookup")
                .is_none()
        );
        assert!(
            storage
                .get_durable_chat_run(&failed_run.id)
                .expect("rolled-back run lookup")
                .is_none()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn chat_run_attempt_usage_accumulates_lifetime_total() {
        let (storage, root) = temporary_storage("chat-run-attempt-usage");
        let session = test_session("session-usage", false);
        storage.upsert_session(&session).expect("session");

        let mut run = StoredDurableChatRun::new(
            "run-usage",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            "prompt",
            session.project_path.clone(),
        );
        run.user_message_id = Some("message-usage".to_string());
        storage.create_durable_chat_run(&run).expect("run");

        let attempt = StoredChatRunAttempt::new(
            "attempt-usage",
            run.id.clone(),
            session.id.clone(),
            run.user_message_id.clone(),
            "codex",
            "native_cli",
            Some("gpt-test".to_string()),
            Some("native-1".to_string()),
        );
        assert!(
            storage
                .create_chat_run_attempt(&attempt)
                .expect("insert attempt")
        );
        assert!(
            !storage
                .create_chat_run_attempt(&attempt)
                .expect("idempotent attempt")
        );
        let lifetime = storage
            .finish_chat_run_attempt(
                &attempt.id,
                "completed",
                Some(&SessionTokenUsage {
                    used: 42,
                    input: 30,
                    output: 12,
                    cache_creation: 3,
                    cache_read: 20,
                    reasoning: 5,
                    cost_usd: 0.01,
                }),
                Some(r#"{"total_tokens":42}"#),
                Some("test"),
                TokenUsageCompleteness::Complete,
            )
            .expect("finish attempt")
            .expect("lifetime");
        assert_eq!(lifetime.total, 42);
        assert_eq!(lifetime.input, 30);
        assert_eq!(lifetime.output, 12);
        assert_eq!(lifetime.cache_read, 20);
        assert_eq!(lifetime.reasoning, 5);
        assert_eq!(lifetime.completeness, TokenUsageCompleteness::Complete);
        let latest = storage
            .latest_session_token_usage(&session.id)
            .expect("latest usage query")
            .expect("latest usage");
        assert_eq!(latest.used, 42);
        assert_eq!(latest.input, 30);
        assert_eq!(latest.output, 12);
        assert!(
            storage
                .get_session_summary(&session.id)
                .expect("lightweight session")
                .expect("session")
                .lifetime_token_usage
                .is_none(),
        );
        assert_eq!(
            storage
                .list_sessions()
                .expect("session list")
                .into_iter()
                .find(|listed| listed.id == session.id)
                .and_then(|listed| listed.lifetime_token_usage)
                .map(|usage| usage.total),
            Some(42),
        );
        assert_eq!(
            storage
                .list_sessions()
                .expect("session list")
                .into_iter()
                .find(|listed| listed.id == session.id)
                .and_then(|listed| listed.context_token_usage)
                .map(|usage| (usage.total, usage.after_compact)),
            Some((42, false)),
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn compacted_codex_context_usage_uses_cumulative_delta_after_latest_compact() {
        let (storage, root) = temporary_storage("codex-context-usage-delta");
        let session = test_session("session-context-usage", false);
        storage.upsert_session(&session).expect("session");
        let compacted_at = Utc::now();
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-before-compact",
            "run-before-compact",
            compacted_at - chrono::Duration::seconds(20),
            SessionTokenUsage {
                used: 10_000,
                input: 9_000,
                output: 1_000,
                cache_creation: 0,
                cache_read: 7_000,
                reasoning: 100,
                cost_usd: 0.20,
            },
            "codex.turn.completed.usage",
        );
        let mut compact_run = StoredDurableChatRun::new(
            "run-compact",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            "compact",
            session.project_path.clone(),
        );
        compact_run.status = "completed".to_string();
        compact_run.completed_at = Some(compacted_at);
        storage
            .create_durable_chat_run(&compact_run)
            .expect("compact run");
        let mut rollover = test_context_rollover(
            "rollover-context-usage",
            &session.id,
            "request-context-usage",
            "run-compact",
            "run-compact",
            "",
            compacted_at,
        );
        rollover.kind = "manual".to_string();
        rollover.state = "active".to_string();
        rollover.candidate_native_session_id = Some("native-1".to_string());
        rollover.activated_at = Some(compacted_at);
        storage
            .with_connection(|conn| {
                conn.execute(
                    r#"
                    INSERT INTO session_context_rollovers (
                        id, user_id, session_id, request_id, kind, failed_message_id,
                        trigger_run_id, retry_run_id, from_native_session_id,
                        candidate_native_session_id, state, handoff, observed_bytes,
                        limit_bytes, error, created_at, updated_at, activated_at
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16, ?17, ?18
                    )
                    "#,
                    params![
                        rollover.id,
                        rollover.user_id,
                        rollover.session_id,
                        rollover.request_id,
                        rollover.kind,
                        rollover.failed_message_id,
                        rollover.trigger_run_id,
                        rollover.retry_run_id,
                        rollover.from_native_session_id,
                        rollover.candidate_native_session_id,
                        rollover.state,
                        rollover.handoff,
                        rollover.observed_bytes.map(|value| value as i64),
                        rollover.limit_bytes as i64,
                        rollover.error,
                        rollover.created_at.to_rfc3339(),
                        rollover.updated_at.to_rfc3339(),
                        rollover.activated_at.map(|time| time.to_rfc3339()),
                    ],
                )?;
                Ok(())
            })
            .expect("rollover");
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-after-compact-1",
            "run-after-compact-1",
            compacted_at + chrono::Duration::seconds(10),
            SessionTokenUsage {
                used: 10_600,
                input: 9_500,
                output: 1_100,
                cache_creation: 0,
                cache_read: 7_200,
                reasoning: 120,
                cost_usd: 0.24,
            },
            "codex.turn.completed.usage",
        );
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-after-compact-2",
            "run-after-compact-2",
            compacted_at + chrono::Duration::seconds(20),
            SessionTokenUsage {
                used: 11_200,
                input: 10_100,
                output: 1_100,
                cache_creation: 0,
                cache_read: 7_800,
                reasoning: 150,
                cost_usd: 0.30,
            },
            "codex.turn.completed.usage",
        );

        let scoped = storage
            .session_context_token_usage(&session.id)
            .expect("context usage");
        assert!(scoped.after_compact);
        assert_eq!(scoped.compacted_at, Some(compacted_at));
        assert_eq!(scoped.total, 1_200);
        assert_eq!(scoped.input, 1_100);
        assert_eq!(scoped.output, 100);
        assert_eq!(scoped.cache_read, 800);
        assert_eq!(scoped.reasoning, 50);
        assert_eq!(scoped.completeness, TokenUsageCompleteness::Complete);
        assert_eq!(scoped.partial_attempts, 0);
        assert_eq!(scoped.missing_attempts, 0);
        let listed = storage
            .list_sessions()
            .expect("session list")
            .into_iter()
            .find(|listed| listed.id == session.id)
            .expect("listed session")
            .context_token_usage
            .expect("listed context usage");
        assert_eq!(listed.total, 1_200);
        assert!(listed.after_compact);

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn codex_spent_token_usage_uses_cumulative_deltas_for_whole_and_compacted_scope() {
        let (storage, root) = temporary_storage("codex-spent-usage-delta");
        let session = test_session("session-spent-usage", false);
        storage.upsert_session(&session).expect("session");
        let compacted_at = Utc::now();
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-spent-before-1",
            "run-spent-before-1",
            compacted_at - chrono::Duration::seconds(30),
            SessionTokenUsage {
                used: 10_000,
                input: 9_000,
                output: 1_000,
                cache_creation: 0,
                cache_read: 7_000,
                reasoning: 100,
                cost_usd: 0.20,
            },
            "codex.turn.completed.usage",
        );
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-spent-before-2",
            "run-spent-before-2",
            compacted_at - chrono::Duration::seconds(20),
            SessionTokenUsage {
                used: 10_500,
                input: 9_400,
                output: 1_100,
                cache_creation: 0,
                cache_read: 7_200,
                reasoning: 120,
                cost_usd: 0.24,
            },
            "codex.turn.completed.usage",
        );
        let mut compact_run = StoredDurableChatRun::new(
            "run-spent-compact",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            "compact",
            session.project_path.clone(),
        );
        compact_run.status = "completed".to_string();
        compact_run.completed_at = Some(compacted_at);
        storage
            .create_durable_chat_run(&compact_run)
            .expect("compact run");
        let mut rollover = test_context_rollover(
            "rollover-spent-usage",
            &session.id,
            "request-spent-usage",
            "run-spent-compact",
            "run-spent-compact",
            "",
            compacted_at,
        );
        rollover.kind = "manual".to_string();
        rollover.state = "active".to_string();
        rollover.candidate_native_session_id = Some("native-2".to_string());
        rollover.activated_at = Some(compacted_at);
        storage
            .with_connection(|conn| {
                conn.execute(
                    r#"
                    INSERT INTO session_context_rollovers (
                        id, user_id, session_id, request_id, kind, failed_message_id,
                        trigger_run_id, retry_run_id, from_native_session_id,
                        candidate_native_session_id, state, handoff, observed_bytes,
                        limit_bytes, error, created_at, updated_at, activated_at
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16, ?17, ?18
                    )
                    "#,
                    params![
                        rollover.id,
                        rollover.user_id,
                        rollover.session_id,
                        rollover.request_id,
                        rollover.kind,
                        rollover.failed_message_id,
                        rollover.trigger_run_id,
                        rollover.retry_run_id,
                        rollover.from_native_session_id,
                        rollover.candidate_native_session_id,
                        rollover.state,
                        rollover.handoff,
                        rollover.observed_bytes.map(|value| value as i64),
                        rollover.limit_bytes as i64,
                        rollover.error,
                        rollover.created_at.to_rfc3339(),
                        rollover.updated_at.to_rfc3339(),
                        rollover.activated_at.map(|time| time.to_rfc3339()),
                    ],
                )?;
                Ok(())
            })
            .expect("rollover");
        insert_completed_usage_attempt_with_native(
            &storage,
            &session,
            "attempt-spent-after-1",
            "run-spent-after-1",
            compacted_at + chrono::Duration::seconds(10),
            SessionTokenUsage {
                used: 600,
                input: 500,
                output: 100,
                cache_creation: 0,
                cache_read: 200,
                reasoning: 20,
                cost_usd: 0.04,
            },
            "codex.turn.completed.usage",
            Some("native-2"),
        );
        insert_completed_usage_attempt_with_native(
            &storage,
            &session,
            "attempt-spent-after-2",
            "run-spent-after-2",
            compacted_at + chrono::Duration::seconds(20),
            SessionTokenUsage {
                used: 1_200,
                input: 1_000,
                output: 200,
                cache_creation: 0,
                cache_read: 500,
                reasoning: 50,
                cost_usd: 0.10,
            },
            "codex.turn.completed.usage",
            Some("native-2"),
        );

        let spent = storage
            .session_spent_token_usage(&session.id)
            .expect("spent usage");
        assert_eq!(spent.compacted_at, Some(compacted_at));
        assert_eq!(spent.whole_session.total, 11_700);
        assert_eq!(spent.whole_session.input, 10_400);
        assert_eq!(spent.whole_session.output, 1_300);
        let since_compact = spent.since_compact.expect("since compact");
        assert_eq!(since_compact.total, 1_200);
        assert_eq!(since_compact.input, 1_000);
        assert_eq!(since_compact.output, 200);
        assert_eq!(since_compact.cache_read, 500);
        assert_eq!(since_compact.reasoning, 50);
        assert_eq!(since_compact.completeness, TokenUsageCompleteness::Complete);

        let listed = storage
            .list_sessions()
            .expect("session list")
            .into_iter()
            .find(|listed| listed.id == session.id)
            .expect("listed session")
            .spent_token_usage
            .expect("listed spent usage");
        assert_eq!(listed.whole_session.total, 11_700);
        assert_eq!(
            listed.since_compact.expect("listed since compact").total,
            1_200
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn uncompact_codex_context_usage_uses_latest_cumulative_total() {
        let (storage, root) = temporary_storage("codex-context-usage-whole");
        let session = test_session("session-context-whole", false);
        storage.upsert_session(&session).expect("session");
        let now = Utc::now();
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-cumulative-1",
            "run-cumulative-1",
            now - chrono::Duration::seconds(10),
            SessionTokenUsage {
                used: 10_000,
                input: 9_000,
                output: 1_000,
                cache_creation: 0,
                cache_read: 7_000,
                reasoning: 100,
                cost_usd: 0.20,
            },
            "codex.turn.completed.usage",
        );
        insert_completed_usage_attempt(
            &storage,
            &session,
            "attempt-cumulative-2",
            "run-cumulative-2",
            now,
            SessionTokenUsage {
                used: 11_200,
                input: 10_100,
                output: 1_100,
                cache_creation: 0,
                cache_read: 7_800,
                reasoning: 150,
                cost_usd: 0.30,
            },
            "codex.turn.completed.usage",
        );

        let lifetime = storage
            .session_lifetime_token_usage(&session.id)
            .expect("lifetime");
        assert_eq!(lifetime.total, 21_200);
        let scoped = storage
            .session_context_token_usage(&session.id)
            .expect("context usage");
        assert!(!scoped.after_compact);
        assert_eq!(scoped.total, 11_200);
        assert_eq!(scoped.input, 10_100);
        assert_eq!(scoped.cache_read, 7_800);

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn session_fork_usage_baseline_inherits_only_cloned_prefix() {
        let (storage, root) = temporary_storage("session-fork-usage-baseline");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let source = test_session("session-source-usage", false);
        storage.upsert_session(&source).expect("source session");
        let source_messages = [
            test_message("source-u1", MessageRole::User, "first prompt", 0),
            test_message("source-a1", MessageRole::Assistant, "first answer", 1),
            test_message("source-u2", MessageRole::User, "second prompt", 2),
        ];
        for message in &source_messages {
            storage
                .append_message(&source.id, message)
                .expect("source message");
        }
        for (run_id, message_id, total) in [
            ("run-source-1", "source-u1", 100_u64),
            ("run-source-2", "source-u2", 900_u64),
        ] {
            let mut run = StoredDurableChatRun::new(
                run_id,
                Some("user-1".to_string()),
                source.id.clone(),
                "codex",
                "prompt",
                source.project_path.clone(),
            );
            run.user_message_id = Some(message_id.to_string());
            storage.create_durable_chat_run(&run).expect("run");
            let attempt = StoredChatRunAttempt::new(
                format!("attempt-{run_id}"),
                run.id.clone(),
                source.id.clone(),
                run.user_message_id.clone(),
                "codex",
                "native_cli",
                None,
                None,
            );
            storage.create_chat_run_attempt(&attempt).expect("attempt");
            storage
                .finish_chat_run_attempt(
                    &attempt.id,
                    "completed",
                    Some(&SessionTokenUsage {
                        used: total,
                        input: total - 10,
                        output: 10,
                        cache_creation: 0,
                        cache_read: 0,
                        reasoning: 0,
                        cost_usd: 0.0,
                    }),
                    None,
                    Some("test"),
                    TokenUsageCompleteness::Complete,
                )
                .expect("finish");
        }

        let mut destination = test_session("session-destination-usage", false);
        destination.message_count = 2;
        let cloned = [
            ChatMessage {
                id: "cloned-u1".to_string(),
                metadata: serde_json::json!({
                    "forkedFromSessionId": source.id,
                    "forkedFromMessageId": "source-u1",
                    "usageSourceSessionId": source.id,
                    "usageSourceMessageId": "source-u1",
                }),
                ..source_messages[0].clone()
            },
            ChatMessage {
                id: "cloned-a1".to_string(),
                metadata: serde_json::json!({
                    "forkedFromSessionId": source.id,
                    "forkedFromMessageId": "source-a1",
                }),
                ..source_messages[1].clone()
            },
        ];
        assert_eq!(
            storage
                .create_session_fork(
                    "user-1",
                    &source.id,
                    "source-u2",
                    "request-usage",
                    &destination,
                    &cloned,
                    "second prompt",
                    true,
                    false,
                )
                .expect("fork"),
            CreateSessionForkOutcome::Created
        );
        let restored = storage
            .get_session(&destination.id)
            .expect("session")
            .expect("destination");
        let usage = restored.lifetime_token_usage.expect("usage");
        assert_eq!(usage.total, 100);
        assert_eq!(usage.input, 90);
        assert_eq!(usage.output, 10);

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn session_fork_transaction_preserves_prefix_draft_and_idempotency() {
        let (storage, root) = temporary_storage("session-fork");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let source = test_session("session-source", false);
        storage.upsert_session(&source).expect("source session");
        let source_messages = [
            test_message("source-1", MessageRole::User, "first prompt", 0),
            test_message("source-2", MessageRole::Assistant, "first answer", 1),
            test_message("source-3", MessageRole::User, "second prompt", 2),
            test_message("source-4", MessageRole::Assistant, "second answer", 3),
        ];
        for message in &source_messages {
            storage
                .append_message(&source.id, message)
                .expect("source message");
        }

        let mut destination = test_session("session-destination", false);
        destination.title = "Second prompt".to_string();
        destination.message_count = 2;
        let cloned = [
            ChatMessage {
                id: "cloned-1".to_string(),
                metadata: serde_json::json!({
                    "forkedFromSessionId": source.id,
                    "forkedFromMessageId": source_messages[0].id,
                }),
                ..source_messages[0].clone()
            },
            ChatMessage {
                id: "cloned-2".to_string(),
                metadata: serde_json::json!({
                    "forkedFromSessionId": source.id,
                    "forkedFromMessageId": source_messages[1].id,
                }),
                ..source_messages[1].clone()
            },
        ];
        assert_eq!(
            storage
                .create_session_fork(
                    "user-1",
                    &source.id,
                    "source-3",
                    "request-1",
                    &destination,
                    &cloned,
                    "second prompt",
                    true,
                    true,
                )
                .expect("create fork"),
            CreateSessionForkOutcome::Created
        );

        let restored_source = storage.list_messages(&source.id).expect("source messages");
        assert_eq!(restored_source.len(), source_messages.len());
        assert_eq!(
            restored_source
                .iter()
                .map(|message| (message.id.as_str(), message.role, message.content.as_str()))
                .collect::<Vec<_>>(),
            source_messages
                .iter()
                .map(|message| (message.id.as_str(), message.role, message.content.as_str()))
                .collect::<Vec<_>>()
        );
        let destination_messages = storage
            .list_messages(&destination.id)
            .expect("destination messages");
        assert_eq!(destination_messages.len(), 2);
        assert_eq!(destination_messages[0].id, "cloned-1");
        assert_eq!(
            destination_messages[0].metadata["forkedFromMessageId"],
            "source-1"
        );
        assert_eq!(
            storage
                .get_session_draft("user-1", &destination.id)
                .expect("destination draft")
                .content,
            "second prompt"
        );
        assert_eq!(
            storage
                .get_session_fork("user-1", &source.id, "request-1")
                .expect("fork lookup")
                .expect("stored fork"),
            StoredSessionFork {
                before_message_id: "source-3".to_string(),
                destination_session_id: destination.id.clone(),
                replaces_source: true,
            }
        );
        assert_eq!(
            storage
                .list_sessions()
                .expect("sessions while replacement exists")
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            vec![destination.id.clone()]
        );
        assert_eq!(
            storage
                .list_replaced_source_session_ids()
                .expect("replaced source ids"),
            vec![source.id.clone()]
        );

        let other_destination = test_session("session-other", false);
        assert_eq!(
            storage
                .create_session_fork(
                    "user-1",
                    &source.id,
                    "source-1",
                    "request-1",
                    &other_destination,
                    &[],
                    "different prompt",
                    true,
                    false,
                )
                .expect("idempotent retry"),
            CreateSessionForkOutcome::Existing(StoredSessionFork {
                before_message_id: "source-3".to_string(),
                destination_session_id: destination.id.clone(),
                replaces_source: true,
            })
        );
        assert!(
            storage
                .get_session(&other_destination.id)
                .expect("other destination lookup")
                .is_none()
        );
        assert!(
            storage
                .delete_session(&destination.id)
                .expect("delete replacement")
        );
        assert_eq!(
            storage
                .list_sessions()
                .expect("sessions after deleting replacement")
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            vec![source.id.clone()]
        );
        assert!(
            storage
                .list_replaced_source_session_ids()
                .expect("restored source ids")
                .is_empty()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn session_fork_rejects_active_source_without_partial_writes() {
        let (storage, root) = temporary_storage("session-fork-active");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let source = test_session("session-active", true);
        storage.upsert_session(&source).expect("source session");
        let destination = test_session("session-blocked", false);

        assert_eq!(
            storage
                .create_session_fork(
                    "user-1",
                    &source.id,
                    "source-message",
                    "request-active",
                    &destination,
                    &[],
                    "prompt",
                    true,
                    true,
                )
                .expect("active outcome"),
            CreateSessionForkOutcome::SourceActive
        );
        assert!(
            storage
                .get_session(&destination.id)
                .expect("destination lookup")
                .is_none()
        );
        assert!(
            storage
                .get_session_fork("user-1", &source.id, "request-active")
                .expect("fork lookup")
                .is_none()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn non_replacing_session_fork_keeps_source_visible() {
        let (storage, root) = temporary_storage("session-fork-visible-source");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let source = test_session("session-source", false);
        let destination = test_session("session-destination", false);
        storage.upsert_session(&source).expect("source session");

        assert_eq!(
            storage
                .create_session_fork(
                    "user-1",
                    &source.id,
                    "source-message",
                    "request-visible",
                    &destination,
                    &[],
                    "prompt",
                    true,
                    false,
                )
                .expect("create fork"),
            CreateSessionForkOutcome::Created
        );

        let listed = storage
            .list_sessions()
            .expect("sessions")
            .into_iter()
            .map(|session| session.id)
            .collect::<HashSet<_>>();
        assert_eq!(
            listed,
            HashSet::from([source.id.clone(), destination.id.clone()])
        );
        assert!(
            storage
                .list_replaced_source_session_ids()
                .expect("replaced source ids")
                .is_empty()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn legacy_session_schema_migrates_forks_and_context_rollovers_to_v5() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "iowb-storage-fork-migration-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("storage dir");
        let database = root.join("test.db");
        {
            let connection = Connection::open(&database).expect("legacy database");
            connection
                .execute_batch(
                    r#"
                    CREATE TABLE session_forks (
                        user_id TEXT NOT NULL,
                        source_session_id TEXT NOT NULL,
                        before_message_id TEXT NOT NULL,
                        request_id TEXT NOT NULL,
                        destination_session_id TEXT NOT NULL,
                        created_at TEXT NOT NULL,
                        PRIMARY KEY(user_id, source_session_id, request_id)
                    );
                    "#,
                )
                .expect("legacy schema");
        }

        let storage = Storage::open(&database).expect("migrated storage");
        storage
            .with_connection(|connection| {
                let column_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('session_forks') WHERE name = 'replaces_source'",
                    [],
                    |row| row.get(0),
                )?;
                let index_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_session_forks_replaced_source'",
                    [],
                    |row| row.get(0),
                )?;
                let rollover_table_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'session_context_rollovers'",
                    [],
                    |row| row.get(0),
                )?;
                let rollover_column_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('session_context_rollovers')",
                    [],
                    |row| row.get(0),
                )?;
                let rollover_session_index_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM pragma_index_list('session_context_rollovers') WHERE name = 'idx_session_context_rollovers_session' AND \"unique\" = 0",
                    [],
                    |row| row.get(0),
                )?;
                let rollover_retry_index_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM pragma_index_list('session_context_rollovers') WHERE name = 'idx_session_context_rollovers_retry_run' AND \"unique\" = 1",
                    [],
                    |row| row.get(0),
                )?;
                let rollover_request_unique_count: i64 = connection.query_row(
                    r#"
                    SELECT COUNT(*)
                    FROM pragma_index_list('session_context_rollovers') indexes
                    WHERE indexes."unique" = 1
                      AND (
                          SELECT group_concat(name, ',')
                          FROM (
                              SELECT name
                              FROM pragma_index_info(indexes.name)
                              ORDER BY seqno
                          )
                      ) = 'user_id,session_id,request_id'
                    "#,
                    [],
                    |row| row.get(0),
                )?;
                let schema_version: String = connection.query_row(
                    "SELECT value FROM meta WHERE key = 'schema_version'",
                    [],
                    |row| row.get(0),
                )?;
                let attempts_table_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'chat_run_attempts'",
                    [],
                    |row| row.get(0),
                )?;
                let baselines_table_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'session_usage_baselines'",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(column_count, 1);
                assert_eq!(index_count, 1);
                assert_eq!(rollover_table_count, 1);
                assert_eq!(rollover_column_count, 18);
                assert_eq!(rollover_session_index_count, 1);
                assert_eq!(rollover_retry_index_count, 1);
                assert_eq!(rollover_request_unique_count, 1);
                assert_eq!(attempts_table_count, 1);
                assert_eq!(baselines_table_count, 1);
                assert_eq!(schema_version, "7");
                Ok(())
            })
            .expect("migration checks");

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn context_rollover_prepare_is_idempotent_and_preserves_chat_across_restart() {
        let (storage, root) = temporary_storage("context-rollover-restart");
        let database = root.join("test.db");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let mut session = test_session("session-rollover", false);
        session.title = "Keep this visible chat".to_string();
        session.title_source = Some(SessionTitleSource::Manual);
        session.native_session_id = Some("native-poisoned".to_string());
        session.model = Some("gpt-5.4".to_string());
        session.runtime = Some(ChatRuntime::IoGateway);
        session.effort = Some("high".to_string());
        session.mode = Some("default".to_string());
        session.thinking = Some(true);
        session.fast = Some(true);
        session.message_count = 4;
        storage.upsert_session(&session).expect("upsert session");
        let messages = vec![
            test_message("message-1", MessageRole::User, "Earlier question", 0),
            test_message("message-2", MessageRole::Assistant, "Earlier answer", 1),
            test_message("message-3", MessageRole::Tool, "Large tool result", 2),
            test_message("message-failed", MessageRole::User, "Please continue", 3),
        ];
        for message in &messages {
            storage
                .append_message(&session.id, message)
                .expect("append visible message");
        }
        storage
            .set_session_draft("user-1", &session.id, "unsent follow-up")
            .expect("save draft");

        let mut failed_run = StoredDurableChatRun::new(
            "run-failed",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            "Please continue",
            session.project_path.clone(),
        );
        failed_run.user_message_id = Some("message-failed".to_string());
        failed_run.native_session_id = Some("native-poisoned".to_string());
        storage
            .create_durable_chat_run(&failed_run)
            .expect("create failed run");
        storage
            .mark_durable_chat_run_failed(&failed_run.id, "invalid body")
            .expect("mark original run failed");

        let created_at = Utc::now();
        let rollover = test_context_rollover(
            "rollover-1",
            &session.id,
            "request-1",
            &failed_run.id,
            "run-retry-1",
            "message-failed",
            created_at,
        );
        let mut retry_run = StoredDurableChatRun::new(
            "run-retry-1",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            rollover.handoff.clone(),
            session.project_path.clone(),
        );
        retry_run.user_message_id = Some("message-failed".to_string());
        retry_run.model = session.model.clone();
        retry_run.effort = session.effort.clone();
        retry_run.mode = session.mode.clone();
        retry_run.thinking = session.thinking;
        retry_run.fast = session.fast;

        assert!(
            storage
                .prepare_context_rollover(&rollover, &retry_run)
                .expect("prepare rollover")
        );
        assert!(
            storage
                .has_context_rollover(&session.id)
                .expect("rollover bookkeeping exists")
        );
        assert!(
            !storage
                .has_active_context_rollover(&session.id)
                .expect("prepared rollover is not active")
        );
        assert!(
            !storage
                .prepare_context_rollover(&rollover, &retry_run)
                .expect("repeat identical request")
        );
        assert_eq!(
            storage
                .context_rollover_for_request("user-1", &session.id, "request-1")
                .expect("request lookup")
                .expect("stored rollover")
                .retry_run_id,
            "run-retry-1"
        );
        assert_eq!(
            storage
                .get_durable_chat_run(&failed_run.id)
                .expect("trigger run lookup")
                .expect("trigger run")
                .status,
            "superseded"
        );
        let stored_retry = storage
            .get_durable_chat_run(&retry_run.id)
            .expect("retry run lookup")
            .expect("retry run");
        assert_eq!(stored_retry.native_session_id, None);
        assert_eq!(
            stored_retry.user_message_id.as_deref(),
            Some("message-failed")
        );

        let stored_session = storage
            .get_session(&session.id)
            .expect("session lookup")
            .expect("stored session");
        assert_eq!(stored_session.id, session.id);
        assert_eq!(stored_session.title, "Keep this visible chat");
        assert_eq!(
            stored_session.native_session_id.as_deref(),
            Some("native-poisoned")
        );
        assert_eq!(stored_session.runtime, Some(ChatRuntime::IoGateway));
        assert_eq!(
            storage
                .list_messages(&session.id)
                .expect("messages")
                .into_iter()
                .map(|message| (message.id, message.role, message.content))
                .collect::<Vec<_>>(),
            messages
                .iter()
                .map(|message| { (message.id.clone(), message.role, message.content.clone(),) })
                .collect::<Vec<_>>()
        );
        assert_eq!(
            storage
                .get_session_draft("user-1", &session.id)
                .expect("draft")
                .content,
            "unsent follow-up"
        );

        drop(storage);
        let reopened = Storage::open(&database).expect("reopen storage");
        assert!(
            reopened
                .has_context_rollover(&session.id)
                .expect("rollover presence")
        );
        assert!(
            !reopened
                .has_active_context_rollover(&session.id)
                .expect("prepared rollover is still inactive after restart")
        );
        assert_eq!(
            reopened
                .context_rollover_for_retry_run(&retry_run.id)
                .expect("retry linkage")
                .expect("rollover after restart")
                .id,
            rollover.id
        );
        let recoverable_retry = reopened
            .list_recoverable_durable_chat_runs(3, 10)
            .expect("recoverable retry")
            .into_iter()
            .find(|run| run.id == retry_run.id)
            .expect("retry remains recoverable");
        assert_eq!(recoverable_retry.native_session_id, None);
        assert_eq!(
            reopened
                .list_messages(&session.id)
                .expect("messages")
                .into_iter()
                .map(|message| (message.id, message.role, message.content))
                .collect::<Vec<_>>(),
            messages
                .iter()
                .map(|message| { (message.id.clone(), message.role, message.content.clone(),) })
                .collect::<Vec<_>>()
        );
        assert_eq!(
            reopened
                .get_session_draft("user-1", &session.id)
                .expect("draft")
                .content,
            "unsent follow-up"
        );

        drop(reopened);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn failed_context_rollover_allows_fresh_request_without_duplicate_prompt() {
        let (storage, root) = temporary_storage("context-rollover-failed-retry");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let mut session = test_session("session-rollover-retry", false);
        session.native_session_id = Some("native-poisoned".to_string());
        session.message_count = 1;
        storage.upsert_session(&session).expect("upsert session");
        let failed_message = test_message(
            "message-failed",
            MessageRole::User,
            "Retry this exact prompt",
            0,
        );
        storage
            .append_message(&session.id, &failed_message)
            .expect("append failed prompt");

        let mut trigger_run = StoredDurableChatRun::new(
            "run-trigger",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            failed_message.content.clone(),
            session.project_path.clone(),
        );
        trigger_run.user_message_id = Some(failed_message.id.clone());
        trigger_run.native_session_id = Some("native-poisoned".to_string());
        storage
            .create_durable_chat_run(&trigger_run)
            .expect("create trigger run");
        storage
            .mark_durable_chat_run_failed(&trigger_run.id, "invalid body")
            .expect("mark trigger failed");

        let first = test_context_rollover(
            "rollover-first",
            &session.id,
            "request-first",
            &trigger_run.id,
            "run-retry-first",
            &failed_message.id,
            Utc::now(),
        );
        let mut first_retry = StoredDurableChatRun::new(
            "run-retry-first",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            first.handoff.clone(),
            session.project_path.clone(),
        );
        first_retry.user_message_id = Some(failed_message.id.clone());
        assert!(
            storage
                .prepare_context_rollover(&first, &first_retry)
                .expect("prepare first rollover")
        );
        assert!(
            storage
                .fail_context_rollover(&first.id, "clean context launch failed")
                .expect("fail first rollover")
        );
        assert!(
            !storage
                .has_active_context_rollover(&session.id)
                .expect("failed rollover is not active")
        );
        assert!(
            storage
                .mark_durable_chat_run_failed(&first_retry.id, "clean context launch failed")
                .expect("fail first retry run")
        );
        assert!(
            !storage
                .prepare_context_rollover(&first, &first_retry)
                .expect("same request remains idempotent")
        );

        let second = test_context_rollover(
            "rollover-second",
            &session.id,
            "request-second",
            &first_retry.id,
            "run-retry-second",
            &failed_message.id,
            Utc::now() + chrono::Duration::milliseconds(1),
        );
        let mut second_retry = StoredDurableChatRun::new(
            "run-retry-second",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            second.handoff.clone(),
            session.project_path.clone(),
        );
        second_retry.user_message_id = Some(failed_message.id.clone());
        assert!(
            storage
                .prepare_context_rollover(&second, &second_retry)
                .expect("prepare fresh request")
        );

        assert_eq!(
            storage
                .context_rollover_for_request("user-1", &session.id, "request-first")
                .expect("first request lookup")
                .expect("first rollover")
                .state,
            "failed"
        );
        assert_eq!(
            storage
                .latest_context_rollover(&session.id)
                .expect("latest rollover lookup")
                .expect("latest rollover")
                .request_id,
            "request-second"
        );
        assert_eq!(
            storage
                .get_durable_chat_run(&first_retry.id)
                .expect("first retry lookup")
                .expect("first retry")
                .status,
            "superseded"
        );
        assert_eq!(
            storage
                .get_session(&session.id)
                .expect("session lookup")
                .expect("session")
                .native_session_id
                .as_deref(),
            Some("native-poisoned")
        );
        let visible_messages = storage
            .list_messages(&session.id)
            .expect("visible messages");
        assert_eq!(visible_messages.len(), 1);
        assert_eq!(visible_messages[0].id, failed_message.id);
        assert_eq!(visible_messages[0].role, failed_message.role);
        assert_eq!(visible_messages[0].content, failed_message.content);

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn stale_manual_context_rollover_reconciliation_clears_processing_state() {
        let (storage, root) = temporary_storage("manual-context-rollover-reconcile");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let mut session = test_session("session-manual-reconcile", true);
        session.native_session_id = Some("native-existing".to_string());
        storage.upsert_session(&session).expect("upsert session");

        let mut rollover = test_context_rollover(
            "rollover-manual-stale",
            &session.id,
            "request-manual-stale",
            "run-manual-stale",
            "run-manual-stale",
            "",
            Utc::now(),
        );
        rollover.kind = "manual".to_string();
        rollover.from_native_session_id = Some("native-existing".to_string());
        rollover.candidate_native_session_id = Some("native-existing".to_string());
        let mut compact_run = StoredDurableChatRun::new(
            "run-manual-stale",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            rollover.handoff.clone(),
            session.project_path.clone(),
        );
        compact_run.native_session_id = Some("native-existing".to_string());
        compact_run.auto_resume = false;
        assert!(
            storage
                .prepare_manual_context_rollover(&rollover, &compact_run)
                .expect("prepare manual rollover")
        );
        let attempt = StoredChatRunAttempt::new(
            "attempt-manual-stale",
            compact_run.id.clone(),
            session.id.clone(),
            None,
            "codex",
            "codex_app_server",
            None,
            Some("native-existing".to_string()),
        );
        assert!(
            storage
                .create_chat_run_attempt(&attempt)
                .expect("create compact attempt")
        );
        assert!(
            storage
                .mark_durable_chat_run_interrupted(
                    &compact_run.id,
                    Some("automatic recovery is disabled"),
                )
                .expect("interrupt compact run")
        );

        let inactive = storage
            .reconcile_stale_manual_context_rollovers()
            .expect("reconcile stale manual rollover");
        assert_eq!(inactive, vec![session.id.clone()]);
        let reconciled_rollover = storage
            .context_rollover_for_retry_run(&compact_run.id)
            .expect("rollover lookup")
            .expect("rollover");
        assert_eq!(reconciled_rollover.state, "failed");
        assert_eq!(
            reconciled_rollover.error.as_deref(),
            Some(
                "manual context compaction ended before activation: automatic recovery is disabled"
            )
        );
        let stored_session = storage
            .get_session(&session.id)
            .expect("session lookup")
            .expect("session");
        assert!(!stored_session.active);
        let (attempt_status, attempt_completed_at): (String, Option<String>) = storage
            .with_connection(|conn| {
                conn.query_row(
                    "SELECT status, completed_at FROM chat_run_attempts WHERE id = ?1",
                    params![attempt.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(StorageError::from)
            })
            .expect("attempt lookup");
        assert_eq!(attempt_status, "failed");
        assert!(attempt_completed_at.is_some());
        assert!(
            storage
                .reconcile_stale_manual_context_rollovers()
                .expect("repeat reconciliation")
                .is_empty()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn context_rollover_completion_is_atomic_scoped_and_idempotent() {
        let (storage, root) = temporary_storage("context-rollover-completion");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create user");
        let mut session = test_session("session-rollover-completion", false);
        session.title = "Stable visible chat".to_string();
        session.title_source = Some(SessionTitleSource::Manual);
        session.native_session_id = Some("native-poisoned".to_string());
        session.runtime = Some(ChatRuntime::IoGateway);
        session.model = Some("gpt-5.4".to_string());
        session.effort = Some("high".to_string());
        session.mode = Some("default".to_string());
        session.thinking = Some(true);
        session.fast = Some(true);
        session.message_count = 2;
        storage.upsert_session(&session).expect("upsert session");
        let prior_assistant = test_message(
            "message-prior-assistant",
            MessageRole::Assistant,
            "Earlier answer remains visible",
            0,
        );
        let failed_message = test_message(
            "message-failed",
            MessageRole::User,
            "Continue after compacting",
            1,
        );
        for message in [&prior_assistant, &failed_message] {
            storage
                .append_message(&session.id, message)
                .expect("append existing message");
        }
        storage
            .set_session_draft("user-1", &session.id, "draft stays untouched")
            .expect("save draft");

        let mut trigger_run = StoredDurableChatRun::new(
            "run-trigger-completion",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            failed_message.content.clone(),
            session.project_path.clone(),
        );
        trigger_run.user_message_id = Some(failed_message.id.clone());
        trigger_run.native_session_id = Some("native-poisoned".to_string());
        storage
            .create_durable_chat_run(&trigger_run)
            .expect("create trigger run");
        storage
            .mark_durable_chat_run_failed(&trigger_run.id, "invalid body")
            .expect("mark trigger failed");

        let rollover = test_context_rollover(
            "rollover-completion",
            &session.id,
            "request-completion",
            &trigger_run.id,
            "run-retry-completion",
            &failed_message.id,
            Utc::now(),
        );
        let mut retry_run = StoredDurableChatRun::new(
            "run-retry-completion",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            rollover.handoff.clone(),
            session.project_path.clone(),
        );
        retry_run.user_message_id = Some(failed_message.id.clone());
        assert!(
            storage
                .prepare_context_rollover(&rollover, &retry_run)
                .expect("prepare rollover")
        );

        assert!(
            !storage
                .set_context_rollover_candidate(
                    &rollover.id,
                    "run-from-another-response",
                    "native-clean",
                )
                .expect("reject mismatched retry run")
        );
        assert_eq!(
            storage
                .context_rollover_for_retry_run(&retry_run.id)
                .expect("rollover lookup")
                .expect("rollover")
                .candidate_native_session_id,
            None
        );
        assert_eq!(
            storage
                .get_durable_chat_run(&retry_run.id)
                .expect("retry lookup")
                .expect("retry")
                .native_session_id,
            None
        );
        assert!(
            storage
                .set_context_rollover_candidate(&rollover.id, &retry_run.id, "native-clean")
                .expect("stage clean candidate")
        );
        assert!(
            !storage
                .set_context_rollover_candidate(
                    &rollover.id,
                    &retry_run.id,
                    "native-late-conflict",
                )
                .expect("reject conflicting late candidate")
        );

        let activated_at = Utc::now() + chrono::Duration::seconds(2);
        let mut completed_session = session.clone();
        completed_session.native_session_id = Some("native-clean".to_string());
        completed_session.external = false;
        completed_session.active = true;
        completed_session.last_activity = activated_at;
        let marker = ChatMessage {
            id: "message-compaction-marker".to_string(),
            role: MessageRole::System,
            content: "Context compacted here".to_string(),
            timestamp: activated_at,
            metadata: serde_json::json!({
                "kind": "context_compaction",
                "rolloverId": rollover.id,
                "toNativeSessionId": "native-clean",
            }),
        };
        let assistant = test_message(
            "message-clean-assistant",
            MessageRole::Assistant,
            "Completed in the clean context",
            3,
        );

        assert!(
            !storage
                .complete_context_rollover(
                    &rollover.id,
                    "run-from-another-response",
                    "native-clean",
                    &completed_session,
                    &marker,
                    Some(&assistant),
                    None,
                )
                .expect("reject mismatched completion run")
        );
        assert!(
            !storage
                .complete_context_rollover(
                    &rollover.id,
                    &retry_run.id,
                    "native-late-conflict",
                    &completed_session,
                    &marker,
                    Some(&assistant),
                    None,
                )
                .expect("reject mismatched completion candidate")
        );
        let mut invalid_follow_up = StoredDurableChatRun::new(
            "run-follow-up-invalid",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            failed_message.content.clone(),
            session.project_path.clone(),
        );
        invalid_follow_up.user_message_id = Some(failed_message.id.clone());
        invalid_follow_up.native_session_id = Some("native-late-conflict".to_string());
        assert!(
            !storage
                .complete_context_rollover(
                    &rollover.id,
                    &retry_run.id,
                    "native-clean",
                    &completed_session,
                    &marker,
                    None,
                    Some(&invalid_follow_up),
                )
                .expect("reject mismatched follow-up run")
        );
        let duplicate_assistant = ChatMessage {
            id: failed_message.id.clone(),
            role: MessageRole::Assistant,
            content: "must roll back".to_string(),
            timestamp: activated_at + chrono::Duration::seconds(1),
            metadata: Value::Null,
        };
        assert!(
            storage
                .complete_context_rollover(
                    &rollover.id,
                    &retry_run.id,
                    "native-clean",
                    &completed_session,
                    &marker,
                    Some(&duplicate_assistant),
                    None,
                )
                .is_err(),
            "a persistence error must abort the entire activation"
        );

        let before_success = storage
            .get_session(&session.id)
            .expect("session lookup after rollback")
            .expect("session after rollback");
        assert_eq!(
            before_success.native_session_id.as_deref(),
            Some("native-poisoned")
        );
        assert_eq!(before_success.message_count, 2);
        assert!(
            storage
                .message_by_id(&session.id, &marker.id)
                .expect("marker lookup after rollback")
                .is_none()
        );
        assert_eq!(
            storage
                .context_rollover_for_retry_run(&retry_run.id)
                .expect("rollover after rollback")
                .expect("rollover")
                .state,
            "starting"
        );
        assert_eq!(
            storage
                .get_durable_chat_run(&retry_run.id)
                .expect("retry after rollback")
                .expect("retry")
                .status,
            "running"
        );

        let mut follow_up_run = StoredDurableChatRun::new(
            "run-follow-up-completion",
            Some("user-1".to_string()),
            session.id.clone(),
            "codex",
            failed_message.content.clone(),
            session.project_path.clone(),
        );
        follow_up_run.user_message_id = Some(failed_message.id.clone());
        follow_up_run.native_session_id = Some("native-clean".to_string());
        follow_up_run.model = completed_session.model.clone();
        follow_up_run.effort = completed_session.effort.clone();
        follow_up_run.mode = completed_session.mode.clone();
        follow_up_run.thinking = completed_session.thinking;
        follow_up_run.fast = completed_session.fast;
        assert!(
            storage
                .complete_context_rollover(
                    &rollover.id,
                    &retry_run.id,
                    "native-clean",
                    &completed_session,
                    &marker,
                    None,
                    Some(&follow_up_run),
                )
                .expect("complete rollover")
        );
        assert!(
            storage
                .has_active_context_rollover(&session.id)
                .expect("completed rollover is active")
        );
        let completed = storage
            .get_session(&session.id)
            .expect("completed session lookup")
            .expect("completed session");
        assert_eq!(completed.id, session.id);
        assert_eq!(completed.title, "Stable visible chat");
        assert_eq!(completed.title_source, Some(SessionTitleSource::Manual));
        assert_eq!(completed.native_session_id.as_deref(), Some("native-clean"));
        assert_eq!(completed.runtime, Some(ChatRuntime::IoGateway));
        assert!(!completed.active);
        assert_eq!(completed.message_count, 3);
        assert_eq!(
            storage
                .list_messages(&session.id)
                .expect("completed transcript")
                .into_iter()
                .map(|message| message.id)
                .collect::<HashSet<_>>(),
            HashSet::from([
                prior_assistant.id.clone(),
                failed_message.id.clone(),
                marker.id.clone(),
            ])
        );
        assert_eq!(
            storage
                .get_session_draft("user-1", &session.id)
                .expect("draft after activation")
                .content,
            "draft stays untouched"
        );
        let completed_rollover = storage
            .context_rollover_for_retry_run(&retry_run.id)
            .expect("completed rollover lookup")
            .expect("completed rollover");
        assert_eq!(completed_rollover.state, "active");
        assert_eq!(
            completed_rollover.candidate_native_session_id.as_deref(),
            Some("native-clean")
        );
        assert!(completed_rollover.activated_at.is_some());
        let completed_run = storage
            .get_durable_chat_run(&retry_run.id)
            .expect("completed run lookup")
            .expect("completed run");
        assert_eq!(completed_run.status, "completed");
        assert!(!completed_run.auto_resume);
        assert!(completed_run.completed_at.is_some());
        let completed_follow_up = storage
            .get_durable_chat_run(&follow_up_run.id)
            .expect("follow-up run lookup")
            .expect("follow-up run");
        assert_eq!(completed_follow_up.status, "running");
        assert_eq!(
            completed_follow_up.user_message_id.as_deref(),
            Some(failed_message.id.as_str())
        );
        assert_eq!(
            completed_follow_up.native_session_id.as_deref(),
            Some("native-clean")
        );
        assert_eq!(completed_follow_up.prompt, failed_message.content);

        assert!(
            !storage
                .complete_context_rollover(
                    &rollover.id,
                    &retry_run.id,
                    "native-clean",
                    &completed_session,
                    &marker,
                    None,
                    Some(&follow_up_run),
                )
                .expect("repeat completion is a no-op")
        );
        assert!(
            !storage
                .set_context_rollover_candidate(
                    &rollover.id,
                    &retry_run.id,
                    "native-after-completion",
                )
                .expect("late thread event is ignored")
        );
        assert_eq!(
            storage
                .list_messages(&session.id)
                .expect("final transcript")
                .len(),
            3
        );
        assert_eq!(
            storage
                .get_session(&session.id)
                .expect("final session lookup")
                .expect("final session")
                .native_session_id
                .as_deref(),
            Some("native-clean")
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn native_session_id_round_trips_through_session_metadata() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "iowb-storage-native-thread-{}-{unique}",
            std::process::id()
        ));
        let database = root.join("test.db");
        let storage = Storage::open(&database).expect("storage");
        let session = SessionSummary {
            id: "new-session-test".to_string(),
            provider: Provider::Codex,
            project_path: "/tmp/project".to_string(),
            title: "Test".to_string(),
            last_activity: Utc::now(),
            native_session_id: Some("22222222-2222-4222-8222-222222222222".to_string()),
            native_rollout_owned_by_provider: true,
            title_source: Some(SessionTitleSource::Manual),
            runtime: Some(ChatRuntime::IoGateway),
            fast: Some(true),
            token_usage: Some(SessionTokenUsage {
                used: 4_321,
                input: 1_500,
                output: 2_700,
                cache_creation: 0,
                cache_read: 121,
                reasoning: 0,
                cost_usd: 0.0,
            }),
            ..Default::default()
        };

        storage.upsert_session(&session).expect("upsert");
        let restored = storage
            .get_session(&session.id)
            .expect("query")
            .expect("stored session");
        assert_eq!(restored.native_session_id, session.native_session_id);
        assert!(restored.native_rollout_owned_by_provider);
        assert_eq!(restored.title_source, session.title_source);
        assert_eq!(restored.runtime, Some(ChatRuntime::IoGateway));
        assert_eq!(restored.fast, Some(true));
        let usage = restored.token_usage.as_ref().expect("token usage");
        assert_eq!(usage.used, 4_321);
        assert_eq!(usage.input, 1_500);
        assert_eq!(usage.output, 2_700);
        assert_eq!(usage.cache_read, 121);
        let api_value = serde_json::to_value(&restored).expect("serialize session");
        assert_eq!(api_value["token_usage"]["used"], 4_321);
        assert_eq!(api_value["fast"], true);
        assert!(api_value.get("nativeRolloutOwnedByProvider").is_none());

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn board_scope_round_trips_but_is_hidden_from_discovery_and_search() {
        let (storage, root) = temporary_storage("board-session-scope");
        let mut ordinary = test_session("ordinary-session", true);
        ordinary.title = "ordinary searchable conversation".to_string();
        let mut board = test_session("board-session", true);
        board.title = "board searchable conversation".to_string();
        board.board_session = true;
        board.board_id = Some("board-1".to_string());
        board.board_task_id = Some("task-1".to_string());
        for session in [&ordinary, &board] {
            storage.upsert_session(session).expect("upsert session");
            storage
                .append_message(
                    &session.id,
                    &test_message(
                        &format!("{}-message", session.id),
                        MessageRole::User,
                        &format!("{} searchable", session.id),
                        0,
                    ),
                )
                .expect("append message");
        }

        let restored = storage
            .get_session(&board.id)
            .expect("board query")
            .expect("board session");
        assert!(restored.board_session);
        assert_eq!(restored.board_id.as_deref(), Some("board-1"));
        assert_eq!(restored.board_task_id.as_deref(), Some("task-1"));
        assert_eq!(
            storage
                .list_sessions()
                .expect("visible sessions")
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            vec![ordinary.id.clone()]
        );
        assert!(
            storage
                .list_sessions_including_board()
                .expect("raw sessions")
                .iter()
                .any(|session| session.id == board.id)
        );
        assert_eq!(
            storage
                .list_sessions_for_project("/tmp/project")
                .expect("project sessions")
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            vec![ordinary.id.clone()]
        );
        assert_eq!(
            storage
                .search_messages("searchable", 10)
                .expect("conversation search")
                .into_iter()
                .map(|(session, _)| session.id)
                .collect::<Vec<_>>(),
            vec![ordinary.id]
        );

        // A newer board hit must not consume the result limit and hide an
        // older ordinary conversation from user-facing search.
        let limited = storage
            .search_messages("searchable", 1)
            .expect("limited conversation search");
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].0.id, "ordinary-session");

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn legacy_board_run_scope_is_hidden_from_discovery_and_search() {
        let (storage, root) = temporary_storage("legacy-board-run-scope");
        let mut legacy = test_session("legacy-board-session", true);
        legacy.title = "legacy board searchable conversation".to_string();
        storage.upsert_session(&legacy).expect("upsert session");
        storage
            .append_message(
                &legacy.id,
                &test_message(
                    "legacy-board-message",
                    MessageRole::User,
                    "legacy board searchable",
                    0,
                ),
            )
            .expect("append message");

        storage
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE sessions SET metadata = ?1 WHERE id = ?2",
                    params![r#"{"boardRunId":"board-legacy"}"#, legacy.id],
                )?;
                Ok(())
            })
            .expect("write legacy metadata");

        let restored = storage
            .get_session(&legacy.id)
            .expect("legacy session query")
            .expect("legacy session");
        assert!(restored.is_board_session());
        assert_eq!(restored.board_id.as_deref(), Some("board-legacy"));
        assert!(
            storage
                .list_sessions()
                .expect("visible sessions")
                .is_empty()
        );
        assert!(
            storage
                .list_sessions_for_project("/tmp/project")
                .expect("project sessions")
                .is_empty()
        );
        assert!(
            storage
                .search_messages("searchable", 10)
                .expect("session search")
                .is_empty()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn legacy_prompt_titles_backfill_to_latest_and_preserve_manual_titles() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "iowb-storage-title-backfill-{}-{unique}",
            std::process::id()
        ));
        let database = root.join("test.db");
        let now = Utc::now();

        {
            let storage = Storage::open(&database).expect("storage");
            for (id, title) in [
                ("legacy-auto", "first prompt"),
                ("legacy-manual", "Pinned release investigation"),
            ] {
                storage
                    .upsert_session(&SessionSummary {
                        id: id.to_string(),
                        provider: Provider::Codex,
                        project_path: "/tmp/project".to_string(),
                        title: title.to_string(),
                        last_activity: now,
                        ..Default::default()
                    })
                    .expect("upsert legacy session");
                for (index, content) in ["first prompt", "  latest\n\nprompt  "]
                    .into_iter()
                    .enumerate()
                {
                    storage
                        .append_message(
                            id,
                            &ChatMessage {
                                id: format!("{id}-message-{index}"),
                                role: MessageRole::User,
                                content: content.to_string(),
                                timestamp: now + chrono::Duration::seconds(index as i64),
                                metadata: Value::Null,
                            },
                        )
                        .expect("append legacy prompt");
                }
            }
        }

        let storage = Storage::open(&database).expect("reopen storage");
        let automatic = storage
            .get_session("legacy-auto")
            .expect("automatic query")
            .expect("automatic session");
        assert_eq!(automatic.title, "latest prompt");
        assert_eq!(automatic.title_source, Some(SessionTitleSource::Prompt));

        let manual = storage
            .get_session("legacy-manual")
            .expect("manual query")
            .expect("manual session");
        assert_eq!(manual.title, "Pinned release investigation");
        assert_eq!(manual.title_source, Some(SessionTitleSource::Manual));

        drop(storage);
        let reopened = Storage::open(&database).expect("second reopen");
        assert_eq!(
            reopened
                .get_session("legacy-auto")
                .expect("idempotent query")
                .expect("idempotent session")
                .title,
            "latest prompt"
        );

        drop(reopened);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn lists_only_internal_native_session_ids() {
        let (storage, root) = temporary_storage("internal-native-session-ids");
        for session in [
            SessionSummary {
                id: "internal-session".to_string(),
                provider: Provider::Codex,
                project_path: "/tmp/project".to_string(),
                title: "Internal".to_string(),
                last_activity: Utc::now(),
                native_session_id: Some("native-internal".to_string()),
                ..Default::default()
            },
            SessionSummary {
                id: "external-session".to_string(),
                provider: Provider::Codex,
                external: true,
                project_path: "/tmp/project".to_string(),
                title: "External".to_string(),
                last_activity: Utc::now(),
                native_session_id: Some("native-external".to_string()),
                ..Default::default()
            },
            SessionSummary {
                id: "without-native-session".to_string(),
                provider: Provider::Codex,
                project_path: "/tmp/project".to_string(),
                title: "No native mapping".to_string(),
                last_activity: Utc::now(),
                ..Default::default()
            },
        ] {
            storage.upsert_session(&session).expect("upsert session");
        }

        assert_eq!(
            storage
                .list_internal_native_session_ids()
                .expect("native session ids"),
            vec!["native-internal".to_string()]
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn user_prompt_history_pages_only_user_messages_with_cursor() {
        let (storage, root) = temporary_storage("prompt-history");
        let session = SessionSummary {
            id: "session-prompts".to_string(),
            provider: Provider::Codex,
            project_path: "/tmp/project".to_string(),
            title: "Prompt history".to_string(),
            last_activity: Utc::now(),
            ..Default::default()
        };
        storage.upsert_session(&session).expect("upsert session");
        for (index, role, content) in [
            (0, MessageRole::User, "first"),
            (1, MessageRole::Assistant, "ignored assistant"),
            (2, MessageRole::User, "second"),
            (3, MessageRole::Tool, "ignored tool"),
            (4, MessageRole::User, "second"),
            (5, MessageRole::User, "third"),
        ] {
            storage
                .append_message(
                    &session.id,
                    &ChatMessage {
                        id: format!("m{index}"),
                        role,
                        content: content.to_string(),
                        timestamp: Utc::now() + chrono::Duration::seconds(index),
                        metadata: Value::Null,
                    },
                )
                .expect("append message");
        }

        let (latest, has_more) = storage
            .list_user_prompts_page(&session.id, 2, None)
            .expect("latest prompts");
        assert_eq!(
            latest
                .iter()
                .map(|prompt| prompt.content.as_str())
                .collect::<Vec<_>>(),
            vec!["second", "third"]
        );
        assert!(has_more);

        let cursor = PromptHistoryCursor {
            timestamp: latest.first().expect("oldest latest prompt").timestamp,
            id: latest.first().expect("oldest latest prompt").id.clone(),
        };
        let (older, has_more) = storage
            .list_user_prompts_page(&session.id, 2, Some(&cursor))
            .expect("older prompts");
        assert_eq!(
            older
                .iter()
                .map(|prompt| prompt.content.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert!(!has_more);

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn named_credentials_are_scoped_and_updated_in_place() {
        let (storage, root) = temporary_storage("named-credential");
        storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("create first user");
        storage
            .create_user("user-2", "user-2", "test-hash")
            .expect("create second user");

        storage
            .upsert_named_credential(
                "user-1",
                "gateway-key",
                "io_gateway_api_key",
                "first-secret",
                None,
            )
            .expect("create credential");
        storage
            .upsert_named_credential(
                "user-1",
                "gateway-key",
                "io_gateway_api_key",
                "updated-secret",
                None,
            )
            .expect("update credential");

        assert_eq!(
            storage
                .get_active_credential_value_by_name("user-1", "gateway-key", "io_gateway_api_key",)
                .expect("read credential")
                .as_deref(),
            Some("updated-secret")
        );
        assert_eq!(
            storage
                .get_active_credential_value_by_name("user-2", "gateway-key", "io_gateway_api_key",)
                .expect("read other user"),
            None
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn durable_chat_run_round_trips_and_updates_native_session_id() {
        let (storage, root) = temporary_storage("durable-round-trip");
        let mut run = StoredDurableChatRun::new(
            "board-1",
            Some("user-1".to_string()),
            "ui-session-1",
            "codex",
            "finish the interrupted task",
            "/tmp/project",
        );
        run.native_session_id = Some("native-1".to_string());
        run.model = Some("gpt-5.4".to_string());
        run.effort = Some("high".to_string());
        run.mode = Some("agent".to_string());
        run.thinking = Some(true);
        run.fast = Some(true);

        storage
            .create_durable_chat_run(&run)
            .expect("create durable run");
        let restored = storage
            .get_durable_chat_run(&run.id)
            .expect("get durable run")
            .expect("stored durable run");
        assert_eq!(restored, run);

        assert!(
            storage
                .update_durable_chat_run_native_session_id(&run.id, Some("native-2"))
                .expect("update native id")
        );
        assert_eq!(
            storage
                .get_durable_chat_run(&run.id)
                .expect("get updated run")
                .expect("updated run")
                .native_session_id
                .as_deref(),
            Some("native-2")
        );
        assert!(
            storage
                .update_durable_chat_run_native_session_id(&run.id, None)
                .expect("clear native id")
        );
        assert_eq!(
            storage
                .get_durable_chat_run(&run.id)
                .expect("get cleared run")
                .expect("cleared run")
                .native_session_id,
            None
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn durable_chat_run_recovery_respects_status_flags_attempts_and_limit() {
        let (storage, root) = temporary_storage("durable-recovery");
        let base_time = Utc::now();

        let mut eligible = StoredDurableChatRun::new(
            "eligible",
            None,
            "session-1",
            "codex",
            "prompt 1",
            "/tmp/project",
        );
        eligible.created_at = base_time;
        eligible.updated_at = base_time;
        eligible.last_error = Some("server stopped".to_string());
        eligible.fast = Some(true);

        let mut recovering = StoredDurableChatRun::new(
            "recovering",
            None,
            "session-2",
            "claude",
            "prompt 2",
            "/tmp/project",
        );
        recovering.status = "recovering".to_string();
        recovering.resume_attempts = 1;
        recovering.created_at = base_time + chrono::Duration::seconds(1);
        recovering.updated_at = recovering.created_at;

        let mut disabled = StoredDurableChatRun::new(
            "disabled",
            None,
            "session-3",
            "gemini",
            "prompt 3",
            "/tmp/project",
        );
        disabled.auto_resume = false;
        disabled.created_at = base_time + chrono::Duration::seconds(2);
        disabled.updated_at = disabled.created_at;

        let mut exhausted = StoredDurableChatRun::new(
            "exhausted",
            None,
            "session-4",
            "codex",
            "prompt 4",
            "/tmp/project",
        );
        exhausted.resume_attempts = 2;
        exhausted.created_at = base_time + chrono::Duration::seconds(3);
        exhausted.updated_at = exhausted.created_at;

        let mut terminal = StoredDurableChatRun::new(
            "terminal",
            None,
            "session-5",
            "codex",
            "prompt 5",
            "/tmp/project",
        );
        terminal.status = "completed".to_string();
        terminal.created_at = base_time + chrono::Duration::seconds(4);
        terminal.updated_at = terminal.created_at;

        for run in [&eligible, &recovering, &disabled, &exhausted, &terminal] {
            storage
                .create_durable_chat_run(run)
                .expect("create durable run");
        }

        let recoverable = storage
            .list_recoverable_durable_chat_runs(2, 10)
            .expect("list recoverable");
        assert_eq!(
            recoverable
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>(),
            vec!["eligible", "recovering"]
        );
        assert_eq!(recoverable[0].fast, Some(true));
        assert_eq!(
            storage
                .list_recoverable_durable_chat_runs(2, 1)
                .expect("limited list")[0]
                .id,
            "eligible"
        );

        let active = storage
            .list_active_durable_chat_runs()
            .expect("list active");
        assert_eq!(
            active.iter().map(|run| run.id.as_str()).collect::<Vec<_>>(),
            vec!["eligible", "recovering", "disabled", "exhausted"]
        );

        let claimed = storage
            .mark_durable_chat_run_recovering("eligible", 2)
            .expect("claim recovery")
            .expect("eligible claim");
        assert_eq!(claimed.status, "recovering");
        assert_eq!(claimed.resume_attempts, 1);
        assert_eq!(claimed.last_error, None);
        assert_eq!(claimed.fast, Some(true));
        assert!(claimed.recovered_at.is_some());

        let claimed_again = storage
            .mark_durable_chat_run_recovering("eligible", 2)
            .expect("claim second recovery")
            .expect("eligible second claim");
        assert_eq!(claimed_again.resume_attempts, 2);
        assert_eq!(claimed_again.fast, Some(true));
        assert!(
            storage
                .mark_durable_chat_run_recovering("eligible", 2)
                .expect("attempt exhausted claim")
                .is_none()
        );
        assert!(
            storage
                .mark_durable_chat_run_recovering("terminal", 2)
                .expect("terminal claim")
                .is_none()
        );

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn durable_chat_run_terminal_helpers_persist_outcomes() {
        let (storage, root) = temporary_storage("durable-terminal");
        let failed = StoredDurableChatRun::new(
            "failed",
            None,
            "session-1",
            "codex",
            "prompt",
            "/tmp/project",
        );
        let interrupted = StoredDurableChatRun::new(
            "interrupted",
            None,
            "session-2",
            "codex",
            "prompt",
            "/tmp/project",
        );
        storage
            .create_durable_chat_run(&failed)
            .expect("create failed run");
        storage
            .create_durable_chat_run(&interrupted)
            .expect("create interrupted run");

        assert!(
            storage
                .mark_durable_chat_run_failed("failed", "provider exited")
                .expect("mark failed")
        );
        let failed = storage
            .get_durable_chat_run("failed")
            .expect("get failed")
            .expect("failed run");
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.last_error.as_deref(), Some("provider exited"));
        assert!(failed.completed_at.is_some());

        assert!(
            storage
                .update_durable_chat_run_error("failed", "invalid body")
                .expect("persist structured provider error")
        );
        assert!(
            storage
                .mark_durable_chat_run_failed("failed", "provider run failed")
                .expect("repeat generic failure finalization")
        );
        let failed = storage
            .get_durable_chat_run("failed")
            .expect("get re-finalized failure")
            .expect("re-finalized failed run");
        assert_eq!(
            failed.last_error.as_deref(),
            Some("invalid body"),
            "generic terminalization must not erase the actionable provider error"
        );

        assert!(
            storage
                .mark_durable_chat_run_interrupted("interrupted", Some("retry limit reached"))
                .expect("mark interrupted")
        );
        let interrupted = storage
            .get_durable_chat_run("interrupted")
            .expect("get interrupted")
            .expect("interrupted run");
        assert_eq!(interrupted.status, "interrupted");
        assert_eq!(
            interrupted.last_error.as_deref(),
            Some("retry limit reached")
        );
        assert!(interrupted.completed_at.is_some());

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, NaiveDateTime, Utc};
use iowb_protocol::{
    ChatMessage, DatabaseConnectionInput, DatabaseConnectionProfile, DatabaseTestStatus,
    DatabaseTransferJob, MessageRole, ProjectSummary, PromptHistoryCursor, PromptHistoryEntry,
    Provider, SessionDraftResponse, SessionSummary, SessionTitleSource, SettingEntry,
    SupportedDatabaseType, session_title_from_prompt,
};
use rusqlite::{Connection, OptionalExtension, params};
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
                    created_at TEXT NOT NULL,
                    PRIMARY KEY(user_id, source_session_id, request_id),
                    FOREIGN KEY(destination_session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_session_forks_destination
                    ON session_forks(destination_session_id);

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
                    created_at TEXT NOT NULL,
                    PRIMARY KEY(user_id, source_session_id, request_id),
                    FOREIGN KEY(destination_session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_session_forks_destination
                    ON session_forks(destination_session_id);
                "#,
            )?;

            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', '3')",
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

    pub fn delete_project_by_name(&self, name: &str) -> Result<bool> {
        self.with_connection(|conn| {
            let changed = conn.execute("DELETE FROM projects WHERE name = ?1", params![name])?;
            Ok(changed > 0)
        })
    }

    pub fn upsert_session(&self, session: &SessionSummary) -> Result<()> {
        self.with_connection(|conn| upsert_session_conn(conn, session))
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT s.id, s.provider, s.project_path, s.title,
                       CASE
                           WHEN EXISTS (SELECT 1 FROM messages m WHERE m.session_id = s.id)
                           THEN (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id)
                           ELSE s.message_count
                       END,
                       s.last_activity, s.active, s.model, s.metadata
                FROM sessions s
                ORDER BY last_activity DESC, metadata
                "#,
            )?;

            let rows = stmt.query_map([], map_session_row)?;
            let mut sessions = Vec::new();
            for row in rows {
                sessions.push(row?);
            }
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
            Ok(native_session_ids)
        })
    }

    pub fn list_sessions_for_project(&self, project_path: &str) -> Result<Vec<SessionSummary>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT s.id, s.provider, s.project_path, s.title,
                       CASE
                           WHEN EXISTS (SELECT 1 FROM messages m WHERE m.session_id = s.id)
                           THEN (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id)
                           ELSE s.message_count
                       END,
                       s.last_activity, s.active, s.model, s.metadata
                FROM sessions s
                WHERE s.project_path = ?1
                ORDER BY last_activity DESC, metadata
                "#,
            )?;

            let rows = stmt.query_map(params![project_path], map_session_row)?;
            let mut sessions = Vec::new();
            for row in rows {
                sessions.push(row?);
            }
            Ok(sessions)
        })
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionSummary>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT s.id, s.provider, s.project_path, s.title,
                       CASE
                           WHEN EXISTS (SELECT 1 FROM messages m WHERE m.session_id = s.id)
                           THEN (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id)
                           ELSE s.message_count
                       END,
                       s.last_activity, s.active, s.model, s.metadata
                FROM sessions s
                WHERE s.id = ?1
                "#,
                params![session_id],
                map_session_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn delete_session(&self, session_id: &str) -> Result<bool> {
        self.with_connection(|conn| {
            let changed =
                conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
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
                WHERE id = ?3
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
                    last_error = ?2,
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

    pub fn append_message(&self, session_id: &str, message: &ChatMessage) -> Result<()> {
        self.with_connection(|conn| insert_message_conn(conn, session_id, message))
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
                SELECT before_message_id, destination_session_id
                FROM session_forks
                WHERE user_id = ?1 AND source_session_id = ?2 AND request_id = ?3
                "#,
                params![user_id, source_session_id, request_id],
                |row| {
                    Ok(StoredSessionFork {
                        before_message_id: row.get(0)?,
                        destination_session_id: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
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
    ) -> Result<CreateSessionForkOutcome> {
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let existing = transaction
                .query_row(
                    r#"
                    SELECT before_message_id, destination_session_id
                    FROM session_forks
                    WHERE user_id = ?1 AND source_session_id = ?2 AND request_id = ?3
                    "#,
                    params![user_id, source_session_id, request_id],
                    |row| {
                        Ok(StoredSessionFork {
                            before_message_id: row.get(0)?,
                            destination_session_id: row.get(1)?,
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
                    destination_session_id, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    user_id,
                    source_session_id,
                    before_message_id,
                    request_id,
                    destination.id,
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
                WHERE LOWER(m.content) LIKE LOWER(?1) ESCAPE '\'
                   OR LOWER(s.title) LIKE LOWER(?1) ESCAPE '\'
                   OR LOWER(s.project_path) LIKE LOWER(?1) ESCAPE '\'
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
                let role = parse_role(&row.get::<_, String>(11)?);
                let metadata_raw: String = row.get(14)?;
                let message = ChatMessage {
                    id: row.get(9)?,
                    role,
                    content: row.get(12)?,
                    timestamp: parse_time_sql(row.get::<_, String>(13)?)?,
                    metadata: serde_json::from_str::<Value>(&metadata_raw).unwrap_or(Value::Null),
                };
                Ok((session, message))
            })?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
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

fn serialize_session_metadata(session: &SessionSummary) -> String {
    use serde_json::json;
    let mut value = serde_json::Map::new();
    if session.external {
        value.insert("external".into(), json!(true));
    }
    if let Some(native_session_id) = session.native_session_id.as_ref() {
        value.insert("nativeSessionId".into(), json!(native_session_id));
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
    if let Some(v) = value.get("nativeSessionId").and_then(Value::as_str) {
        session.native_session_id = Some(v.to_string());
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
            }
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
                )
                .expect("idempotent retry"),
            CreateSessionForkOutcome::Existing(StoredSessionFork {
                before_message_id: "source-3".to_string(),
                destination_session_id: destination.id.clone(),
            })
        );
        assert!(
            storage
                .get_session(&other_destination.id)
                .expect("other destination lookup")
                .is_none()
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
            title_source: Some(SessionTitleSource::Manual),
            runtime: Some(ChatRuntime::IoGateway),
            fast: Some(true),
            token_usage: Some(SessionTokenUsage {
                used: 4_321,
                input: 1_500,
                output: 2_700,
                cache_creation: 0,
                cache_read: 121,
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
            "run-1",
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

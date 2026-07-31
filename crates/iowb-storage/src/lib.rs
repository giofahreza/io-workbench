use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use iowb_protocol::{
    ChatMessage, DatabaseConnectionInput, DatabaseConnectionProfile, DatabaseTestStatus,
    DatabaseTransferJob, MessageRole, ProjectSummary, Provider, SessionSummary, SettingEntry,
    SupportedDatabaseType,
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

                CREATE TABLE IF NOT EXISTS messages (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    timestamp TEXT NOT NULL,
                    metadata TEXT NOT NULL DEFAULT 'null',
                    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

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

            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', '1')",
                [],
            )?;

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
        self.with_connection(|conn| {
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
        })
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

    pub fn append_message(&self, session_id: &str, message: &ChatMessage) -> Result<()> {
        self.with_connection(|conn| {
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
    DateTime::parse_from_rfc3339(&raw)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))
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
    if let Some(model) = session.model.as_ref() {
        value.insert("model".into(), json!(model));
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

fn merge_metadata_into(session: &mut SessionSummary, value: serde_json::Value) {
    use serde_json::Value;
    if let Some(v) = value.get("external").and_then(Value::as_bool) {
        session.external = v;
    }
    if let Some(v) = value.get("nativeSessionId").and_then(Value::as_str) {
        session.native_session_id = Some(v.to_string());
    }
    if let Some(v) = value.get("model").and_then(Value::as_str) {
        session.model = Some(v.to_string());
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
    use std::time::{SystemTime, UNIX_EPOCH};

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
            ..Default::default()
        };

        storage.upsert_session(&session).expect("upsert");
        let restored = storage
            .get_session(&session.id)
            .expect("query")
            .expect("stored session");
        assert_eq!(restored.native_session_id, session.native_session_id);

        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}

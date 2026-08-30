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

                CREATE TABLE IF NOT EXISTS user_password_hashes (
                    user_id TEXT NOT NULL,
                    password_hash TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY(user_id, password_hash),
                    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_user_password_hashes_user_id
                    ON user_password_hashes(user_id);

                INSERT OR IGNORE INTO user_password_hashes (user_id, password_hash, created_at)
                SELECT id, password_hash, created_at
                FROM users;

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
            conn.execute(
                r#"
                INSERT OR IGNORE INTO user_password_hashes (user_id, password_hash, created_at)
                VALUES (?1, ?2, ?3)
                "#,
                params![id, password_hash, now.to_rfc3339()],
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

    pub fn get_user_password_hashes(&self, user_id: &str) -> Result<Vec<String>> {
        self.with_connection(|conn| {
            let mut statement = conn.prepare(
                r#"
                SELECT password_hash
                FROM user_password_hashes
                WHERE user_id = ?1
                ORDER BY created_at ASC, password_hash ASC
                "#,
            )?;
            let hashes = statement
                .query_map(params![user_id], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(hashes)
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

}

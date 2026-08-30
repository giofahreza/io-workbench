impl Storage {
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

}

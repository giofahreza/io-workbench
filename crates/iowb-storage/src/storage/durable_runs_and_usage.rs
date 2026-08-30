impl Storage {
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

}

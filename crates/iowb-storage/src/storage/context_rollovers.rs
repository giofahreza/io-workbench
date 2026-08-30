impl Storage {
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

}

impl Storage {
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

}

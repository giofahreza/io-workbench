impl Storage {
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

}

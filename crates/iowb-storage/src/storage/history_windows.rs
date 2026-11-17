#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageHistoryVersion {
    pub revision: i64,
    pub rewrite_revision: i64,
    pub last_timestamp: String,
    pub last_id: String,
}

impl Storage {
    fn migrate_message_history_versions(&self) -> Result<()> {
        self.with_connection(|conn| {
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS message_history_versions (
                    session_id TEXT PRIMARY KEY,
                    revision INTEGER NOT NULL,
                    rewrite_revision INTEGER NOT NULL,
                    last_timestamp TEXT NOT NULL,
                    last_id TEXT NOT NULL
                );
                CREATE TRIGGER IF NOT EXISTS message_history_insert AFTER INSERT ON messages
                BEGIN
                    INSERT INTO message_history_versions
                    SELECT NEW.session_id, 1, 1, timestamp, id FROM messages
                    WHERE session_id = NEW.session_id ORDER BY timestamp DESC, id DESC LIMIT 1
                    ON CONFLICT(session_id) DO UPDATE SET
                        revision = revision + 1,
                        rewrite_revision = rewrite_revision + CASE
                            WHEN (NEW.timestamp, NEW.id) <= (last_timestamp, last_id) THEN 1 ELSE 0 END,
                        last_timestamp = excluded.last_timestamp,
                        last_id = excluded.last_id;
                END;
                CREATE TRIGGER IF NOT EXISTS message_history_delete AFTER DELETE ON messages
                BEGIN
                    INSERT INTO message_history_versions VALUES (
                        OLD.session_id, 1, 1,
                        COALESCE((SELECT timestamp FROM messages WHERE session_id = OLD.session_id
                            ORDER BY timestamp DESC, id DESC LIMIT 1), ''),
                        COALESCE((SELECT id FROM messages WHERE session_id = OLD.session_id
                            ORDER BY timestamp DESC, id DESC LIMIT 1), '')
                    ) ON CONFLICT(session_id) DO UPDATE SET
                        revision = revision + 1, rewrite_revision = rewrite_revision + 1,
                        last_timestamp = excluded.last_timestamp, last_id = excluded.last_id;
                END;
                CREATE TRIGGER IF NOT EXISTS message_history_update AFTER UPDATE ON messages
                BEGIN
                    INSERT INTO message_history_versions VALUES (
                        OLD.session_id, 1, 1,
                        COALESCE((SELECT timestamp FROM messages WHERE session_id = OLD.session_id
                            ORDER BY timestamp DESC, id DESC LIMIT 1), ''),
                        COALESCE((SELECT id FROM messages WHERE session_id = OLD.session_id
                            ORDER BY timestamp DESC, id DESC LIMIT 1), '')
                    ) ON CONFLICT(session_id) DO UPDATE SET
                        revision = revision + 1, rewrite_revision = rewrite_revision + 1,
                        last_timestamp = excluded.last_timestamp, last_id = excluded.last_id;
                    INSERT INTO message_history_versions
                    SELECT NEW.session_id, 1, 1, timestamp, id FROM messages
                    WHERE session_id = NEW.session_id AND NEW.session_id != OLD.session_id
                    ORDER BY timestamp DESC, id DESC LIMIT 1
                    ON CONFLICT(session_id) DO UPDATE SET
                        revision = revision + 1, rewrite_revision = rewrite_revision + 1,
                        last_timestamp = excluded.last_timestamp, last_id = excluded.last_id;
                END;
                "#,
            )?;
            Ok(())
        })
    }

    pub fn message_history_version(&self, session_id: &str) -> Result<MessageHistoryVersion> {
        self.with_connection(|conn| {
            let version = conn
                .query_row(
                    "SELECT revision, rewrite_revision, last_timestamp, last_id
                 FROM message_history_versions WHERE session_id = ?1",
                    params![session_id],
                    |row| {
                        Ok(MessageHistoryVersion {
                            revision: row.get(0)?,
                            rewrite_revision: row.get(1)?,
                            last_timestamp: row.get(2)?,
                            last_id: row.get(3)?,
                        })
                    },
                )
                .optional()?;
            if let Some(version) = version {
                return Ok(version);
            }
            let last = conn
                .query_row(
                    "SELECT timestamp, id FROM messages WHERE session_id = ?1
                 ORDER BY timestamp DESC, id DESC LIMIT 1",
                    params![session_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
                .unwrap_or_default();
            Ok(MessageHistoryVersion {
                revision: 0,
                rewrite_revision: 0,
                last_timestamp: last.0,
                last_id: last.1,
            })
        })
    }

    pub fn messages_after_history_version(
        &self,
        session_id: &str,
        version: &MessageHistoryVersion,
    ) -> Result<Vec<ChatMessage>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, role, content, timestamp, metadata FROM messages
                 WHERE session_id = ?1 AND (timestamp, id) > (?2, ?3)
                 ORDER BY timestamp, id",
            )?;
            let rows = stmt.query_map(
                params![session_id, version.last_timestamp, version.last_id],
                |row| {
                    Ok(ChatMessage {
                        id: row.get(0)?,
                        role: parse_role(&row.get::<_, String>(1)?),
                        content: row.get(2)?,
                        timestamp: parse_time_sql(row.get::<_, String>(3)?)?,
                        metadata: serde_json::from_str(&row.get::<_, String>(4)?)
                            .unwrap_or(Value::Null),
                    })
                },
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(StorageError::from)
        })
    }

    pub fn external_messages_page_if_current(
        &self,
        provider: Provider,
        session_id: &str,
        file_path: &str,
        fingerprint: &ExternalHistoryFingerprint<'_>,
        limit: usize,
        offset: usize,
    ) -> Result<Option<(Vec<ChatMessage>, usize)>> {
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let Some(total) = external_message_state_total_if_matches(
                &transaction,
                provider,
                session_id,
                file_path,
                fingerprint,
            )?
            else {
                return Ok(None);
            };
            let mut stmt = transaction.prepare(
                "SELECT message_json FROM external_history_messages
                 WHERE provider = ?1 AND session_id = ?2 AND file_path = ?3 AND sequence >= ?4
                 ORDER BY sequence LIMIT ?5",
            )?;
            let mut rows = stmt.query(params![
                provider.as_str(),
                session_id,
                file_path,
                bounded_i64(offset as u64),
                bounded_i64(limit as u64),
            ])?;
            let mut messages = Vec::new();
            while let Some(row) = rows.next()? {
                messages.push(serde_json::from_str(&row.get::<_, String>(0)?)?);
            }
            Ok(Some((messages, total)))
        })
    }
}

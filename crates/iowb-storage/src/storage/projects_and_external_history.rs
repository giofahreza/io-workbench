impl Storage {
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

}

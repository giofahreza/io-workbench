impl Storage {
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

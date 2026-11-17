struct TransferSourceSnapshot {
    columns: Vec<DatabaseObjectColumn>,
    rows: Vec<Map<String, Value>>,
}

const PORTABLE_SCHEMA_LIMITATIONS: &str = "Portable schema copies preserve basic column types, nullability, and primary keys only. Defaults, generated/identity behavior, secondary indexes, foreign keys, triggers, and comments are not preserved.";

// A transfer must not page a changing source between target writes. Spooling a
// single SELECT keeps the database's statement snapshot without retaining the
// whole table in memory or holding its transaction open during a slow import.
struct TransferSourceSpool {
    file: Option<std::io::BufReader<std::fs::File>>,
    path: PathBuf,
    row_count: usize,
}

impl Drop for TransferSourceSpool {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

impl TransferSourceSpool {
    fn read_batch(&mut self) -> Result<Vec<Map<String, Value>>> {
        use std::io::BufRead;
        let mut rows = Vec::with_capacity(TRANSFER_ROW_BATCH_SIZE);
        for _ in 0..TRANSFER_ROW_BATCH_SIZE {
            let mut line = String::new();
            if self.file.as_mut().expect("open transfer snapshot").read_line(&mut line).map_err(io_server_error)? == 0 {
                break;
            }
            rows.push(serde_json::from_str(&line).map_err(|error| {
                ServerError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("Cannot read transfer snapshot: {error}"))
            })?);
        }
        Ok(rows)
    }
}

async fn spool_transfer_source(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
) -> Result<TransferSourceSpool> {
    use std::io::{Seek, Write};
    let path = std::env::temp_dir().join(format!("{}.jsonl", new_id("iowb-transfer")));
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(io_server_error)?;
    let mut spool = TransferSourceSpool { file: Some(std::io::BufReader::new(file)), path, row_count: 0 };
    // Keep only an open descriptor on Unix, including if the process crashes.
    #[cfg(unix)]
    std::fs::remove_file(&spool.path).map_err(io_server_error)?;
    {
        let mut writer = std::io::BufWriter::new(spool.file.as_mut().expect("open transfer snapshot").get_mut());
        let mut write_row = |row: Map<String, Value>| -> Result<()> {
            serde_json::to_writer(&mut writer, &row).map_err(|error| {
                ServerError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("Cannot stage transfer row: {error}"))
            })?;
            writer.write_all(b"\n").map_err(io_server_error)?;
            spool.row_count += 1;
            Ok(())
        };
        if connection.profile.db_type == SupportedDatabaseType::Sqlite {
            let conn = sqlite_connection(connection)?;
            let mut stmt = conn.prepare(&format!("SELECT * FROM {}", quote_identifier(&endpoint.table_name))).map_err(sqlite_server_error)?;
            let columns = stmt.column_names().into_iter().map(str::to_string).collect::<Vec<_>>();
            let mut rows = stmt.query([]).map_err(sqlite_server_error)?;
            while let Some(row) = rows.next().map_err(sqlite_server_error)? {
                write_row(row_to_json_map(row, &columns)?)?;
            }
        } else {
            let sql = format!("SELECT * FROM {}", transfer_table_ref(connection.profile.db_type, endpoint));
            match typed_live_pool_for_database(connection, endpoint.database_name.as_deref()).await? {
                TypedLivePool::Postgresql(pool) => {
                    let mut rows = sqlx::raw_sql(&sql).fetch(&pool);
                    while let Some(row) = rows.next().await {
                        write_row(postgres_row_to_json_map(&row.map_err(sqlx_server_error)?))?;
                    }
                }
                TypedLivePool::Mysql(pool) => {
                    let mut rows = sqlx::raw_sql(&sql).fetch(&pool);
                    while let Some(row) = rows.next().await {
                        write_row(mysql_row_to_json_map(&row.map_err(sqlx_server_error)?))?;
                    }
                }
            }
        }
        writer.flush().map_err(io_server_error)?;
    }
    spool.file.as_mut().expect("open transfer snapshot").rewind().map_err(io_server_error)?;
    Ok(spool)
}

fn append_transfer_job_log<F>(
    job: &mut DatabaseTransferJob,
    level: &str,
    message: impl Into<String>,
    details: Option<String>,
    persist_job: &mut F,
) where
    F: FnMut(&DatabaseTransferJob),
{
    let timestamp = Utc::now();
    job.logs.push(DatabaseTransferJobLogEntry {
        timestamp,
        level: level.to_string(),
        message: message.into(),
        details,
    });
    job.updated_at = timestamp;
    persist_job(job);
}

fn set_transfer_job_phase<F>(
    job: &mut DatabaseTransferJob,
    message: impl AsRef<str>,
    persist_job: &mut F,
) where
    F: FnMut(&DatabaseTransferJob),
{
    // Setup steps have no meaningful row denominator. Do not mix their step
    // count with the later row percentage (for example, 75% -> 200/401=50%).
    job.progress = serde_json::json!({"message": message.as_ref()});
    job.updated_at = Utc::now();
    persist_job(job);
}

fn transfer_copy_progress(current: usize, total: usize, copied: usize, failed: usize) -> Value {
    let percentage = if total == 0 {
        0
    } else {
        let rounded = ((current as f64 / total as f64) * 100.0).round() as usize;
        // Rounding 400/401 to 100 must not imply that the last row is done.
        rounded.min(if current < total { 99 } else { 100 })
    };
    serde_json::json!({
        "current": current,
        "total": total,
        "percentage": percentage,
        "message": format!("Copying row data: {current} of {total} row(s) processed; {copied} copied, {failed} failed"),
    })
}

fn transfer_copy_checkpoint_due(
    processed: usize,
    batch_finished: bool,
    elapsed: std::time::Duration,
) -> bool {
    processed == 1 || batch_finished || elapsed >= std::time::Duration::from_millis(500)
}

fn transfer_value_for_target_column(column: &DatabaseObjectColumn, value: Value) -> Value {
    let is_boolean = column.native_type.as_deref().or(column.data_type.as_deref())
        .is_some_and(|kind| matches!(kind.to_ascii_lowercase().as_str(), "bool" | "boolean"));
    if is_boolean {
        // SQLite and MySQL expose Boolean storage as numeric0/1. PostgreSQL
        // accepts TRUE/FALSE, not an integer assignment to BOOLEAN. Normalize
        // only these exact values; 2/-1/fractions must still fail validation,
        // and SQL NULL, strings, arrays and unrelated columns retain their type.
        match value.as_i64() {
            Some(0) => return Value::Bool(false),
            Some(1) => return Value::Bool(true),
            _ => {}
        }
    }
    value
}

fn transfer_value_for_column_mapping(
    source: Option<&DatabaseObjectColumn>,
    target: &DatabaseObjectColumn,
    value: Value,
) -> Value {
    let source_is_json = source.and_then(|column| column.native_type.as_deref().or(column.data_type.as_deref()))
        .is_some_and(|kind| matches!(kind.to_ascii_lowercase().as_str(), "json" | "jsonb"));
    let target_is_text = target.native_type.as_deref().or(target.data_type.as_deref())
        .is_some_and(|kind| {
            let kind = kind.to_ascii_lowercase();
            matches!(kind.as_str(), "text" | "tinytext" | "mediumtext" | "longtext" | "char" | "varchar" | "character" | "character varying")
                || kind.starts_with("varchar(") || kind.starts_with("char(")
                || kind.starts_with("character varying(") || kind.starts_with("character(")
        });
    if target_is_text && source.is_some_and(|column| transfer_unsigned_integer_width(column) == Some(64)) {
        // SQLite parses unquoted integers above i64::MAX through REAL before
        // applying TEXT affinity, losing digits. Quote the exact decimal token.
        // This also preserves u64 values copied into existing text columns.
        if let Value::Number(number) = &value {
            return Value::String(number.to_string());
        }
    }
    if source_is_json && target_is_text {
        // JSON true inserted as SQL TRUE into SQLite TEXT becomes "1", which
        // is a JSON number, not a Boolean. Preserve the source JSON document.
        // Strings from the database decoder are already encoded JSON text
        // (including JSON null and string scalars); never double-encode them.
        return match value {
            Value::Null | Value::String(_) => value,
            _ => Value::String(value.to_string()),
        };
    }
    transfer_value_for_target_column(target, value)
}

fn add_transfer_job_warning<F>(
    job: &mut DatabaseTransferJob,
    message: impl Into<String>,
    scope: Option<String>,
    code: Option<String>,
    persist_job: &mut F,
) where
    F: FnMut(&DatabaseTransferJob),
{
    let message = message.into();
    job.warnings.push(DatabaseTransferJobWarning {
        message: message.clone(),
        scope: scope.clone(),
        code,
    });
    append_transfer_job_log(job, "warning", message, scope, persist_job);
}

async fn run_database_transfer<F>(
    source_connection: &StoredDatabaseConnection,
    target_connection: &StoredDatabaseConnection,
    job: &mut DatabaseTransferJob,
    persist_job: &mut F,
) -> Result<DatabaseTransferJobResult>
where
    F: FnMut(&DatabaseTransferJob) + Send,
{
    let source = job.source.clone();
    let target = job.target.clone();
    let mode = job.mode;
    set_transfer_job_phase(job, "Opening connections", persist_job);

    if mode != DatabaseTransferMode::TableCopy {
        add_transfer_job_warning(job, PORTABLE_SCHEMA_LIMITATIONS, None,
            Some("SCHEMA_PORTABILITY_LIMITATIONS".to_string()), persist_job);
    }

    append_transfer_job_log(
        job,
        "info",
        "Opening source and target connections",
        None,
        persist_job,
    );
    append_transfer_job_log(
        job,
        "info",
        "Connections opened",
        Some(format!(
            "{} -> {}",
            source_connection.profile.db_type.as_str(),
            target_connection.profile.db_type.as_str()
        )),
        persist_job,
    );
    set_transfer_job_phase(job, "Inspecting source table", persist_job);

    let source_metadata = database_table_metadata(
        source_connection,
        source.database_name.as_deref(),
        source.schema_name.as_deref(),
        &source.table_name,
    )
    .await?;
    if source_metadata.columns.is_empty() {
        return Err(ServerError::database(
            StatusCode::BAD_REQUEST,
            "Source table has no columns",
            None,
            "NO_COMPATIBLE_COLUMNS",
            "validation",
            false,
        ));
    }
    append_transfer_job_log(
        job,
        "info",
        "Loaded source table model",
        Some(format!(
            "{} ({} column(s))",
            source.table_name,
            source_metadata.columns.len()
        )),
        persist_job,
    );

    // Row-only copies retain the existing target schema and defaults. Only
    // portable schema creation can omit a source default definition.
    if mode != DatabaseTransferMode::TableCopy {
        for column in &source_metadata.columns {
            if column.default_value.is_some() {
                add_transfer_job_warning(
                    job,
                    format!("Skipped default value mapping for column {}", column.name),
                    Some(column.name.clone()),
                    Some("DEFAULT_SKIPPED".to_string()),
                    persist_job,
                );
            }
        }
    }

    // Capture one read snapshot before any target writes. OFFSET reads taken
    // between inserts can consume their own output through a connection alias,
    // or skip/duplicate rows when another client changes the source.
    let source_snapshot = if mode != DatabaseTransferMode::SchemaOnly {
        set_transfer_job_phase(job, "Reading source snapshot", persist_job);
        Some(spool_transfer_source(source_connection, &source).await?)
    } else {
        None
    };

    set_transfer_job_phase(job, "Checking target table", persist_job);
    let target_exists = transfer_target_exists(target_connection, &target).await?;
    let mut created_table = false;
    match mode {
        DatabaseTransferMode::TableCopy if !target_exists => {
            return Err(ServerError::database(
                StatusCode::NOT_FOUND,
                format!("Table not found: {}", target.table_name),
                None,
                "TABLE_NOT_FOUND",
                "metadata",
                false,
            ));
        }
        DatabaseTransferMode::SchemaOnly | DatabaseTransferMode::SchemaAndData if target_exists => {
            return Err(ServerError::database(
                StatusCode::BAD_REQUEST,
                format!("Target table already exists: {}", target.table_name),
                None,
                "TARGET_TABLE_EXISTS",
                "validation",
                false,
            ));
        }
        DatabaseTransferMode::SchemaOnly | DatabaseTransferMode::SchemaAndData => {
            set_transfer_job_phase(job, "Preparing target table", persist_job);
            create_transfer_target_table(target_connection, &target, &source_metadata.columns)
                .await?;
            created_table = true;
            append_transfer_job_log(
                job,
                "info",
                "Created target table",
                Some(target.table_name.clone()),
                persist_job,
            );
        }
        DatabaseTransferMode::TableCopy => {}
    }

    // Persist the surviving table even if later mapping or insertion fails.
    job.result = Some(DatabaseTransferJobResult {
        created_table, copied_row_count: 0, failed_row_count: 0,
        ignored_source_columns: Vec::new(), mapped_column_count: 0,
        column_failures: Vec::new(), row_failures: Vec::new(),
    });
    persist_job(job);

    if mode == DatabaseTransferMode::SchemaOnly {
        return Ok(DatabaseTransferJobResult {
            created_table,
            copied_row_count: 0,
            failed_row_count: 0,
            ignored_source_columns: Vec::new(),
            mapped_column_count: source_metadata.columns.len(),
            column_failures: Vec::new(),
            row_failures: Vec::new(),
        });
    }

    set_transfer_job_phase(job, "Loading target table metadata", persist_job);
    let target_metadata = database_table_metadata(
        target_connection,
        target.database_name.as_deref(),
        target.schema_name.as_deref(),
        &target.table_name,
    )
    .await?;
    let source_column_names = source_metadata
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    let mappings = build_column_mappings(&source_column_names, &target_metadata.columns)?;
    let used_source_columns = mappings
        .iter()
        .map(|(_, source_name)| source_name.clone())
        .collect::<Vec<_>>();
    let ignored_source_columns = source_column_names
        .iter()
        .filter(|source_name| !used_source_columns.iter().any(|used| used == *source_name))
        .cloned()
        .collect::<Vec<_>>();
    let column_failures = target_metadata
        .columns
        .iter()
        .filter(|column| {
            column.nullable == Some(false)
                && column.default_value.is_none()
                && !column.is_primary_key
                && !database_column_is_server_managed(column)
                && !mappings
                    .iter()
                    .any(|(target_name, _)| target_name == &column.name)
        })
        .map(|column| {
            serde_json::json!({
                "columnName": column.name,
                "message": "Required target column has no matching source column",
            })
        })
        .collect::<Vec<_>>();
    let mapping_details = serde_json::json!({
        "mappings": mappings.iter().map(|(target_name, source_name)| serde_json::json!({
            "targetColumnName": target_name,
            "sourceColumnName": source_name,
        })).collect::<Vec<_>>(),
        "ignoredSourceColumns": ignored_source_columns,
        "columnFailures": column_failures,
    });
    if mappings.is_empty() {
        return Err(ServerError::database(
            StatusCode::BAD_REQUEST,
            "No compatible columns were found between the source and target tables",
            Some(mapping_details.to_string()),
            "NO_COMPATIBLE_COLUMNS",
            "validation",
            false,
        ));
    }
    if !column_failures.is_empty() {
        return Err(ServerError::database(
            StatusCode::BAD_REQUEST,
            "Target table is missing required column mappings",
            Some(mapping_details.to_string()),
            "INCOMPATIBLE_TARGET_TABLE",
            "validation",
            false,
        ));
    }
    if !ignored_source_columns.is_empty() {
        add_transfer_job_warning(
            job,
            format!(
                "Ignored {} unmapped source column(s)",
                ignored_source_columns.len()
            ),
            Some(ignored_source_columns.join(", ")),
            Some("IGNORED_SOURCE_COLUMNS".to_string()),
            persist_job,
        );
    }

    let mapped_columns = mappings
        .iter()
        .filter_map(|(target_name, _)| {
            target_metadata
                .columns
                .iter()
                .find(|column| column.name == *target_name)
                .cloned()
        })
        .collect::<Vec<_>>();
    let mapped_source_columns = mappings.iter().map(|(_, name)| {
        source_metadata.columns.iter().find(|column| column.name == *name)
    }).collect::<Vec<_>>();
    let mut copied_row_count = 0usize;
    let mut failed_row_count = 0usize;
    let mut row_failures = Vec::new();
    let mut processed_row_count = 0usize;
    let mut source_rows = source_snapshot.expect("data transfer snapshot");
    let total_row_count = source_rows.row_count;
    let mut offset = 0usize;
    job.result = Some(DatabaseTransferJobResult {
        created_table, copied_row_count, failed_row_count,
        ignored_source_columns: ignored_source_columns.clone(), mapped_column_count: mappings.len(),
        column_failures: Vec::new(), row_failures: Vec::new(),
    });
    job.progress = transfer_copy_progress(0, total_row_count, 0, 0);
    job.updated_at = Utc::now();
    persist_job(job);
    let mut last_checkpoint = std::time::Instant::now();

    loop {
        let page = source_rows.read_batch()?;
        if page.is_empty() { break; }
        let page_row_count = page.len();
        for (page_row_index, source_row) in page.iter().enumerate() {
            let mapped_row = mapped_columns.iter().zip(&mappings).zip(&mapped_source_columns)
                .map(|((column, (target_name, source_name)), source_column)| {
                    (
                        target_name.clone(),
                        transfer_value_for_column_mapping(*source_column, column,
                            source_row.get(source_name).cloned().unwrap_or(Value::Null)),
                    )
                })
                .collect::<Map<_, _>>();
            match insert_transfer_rows(target_connection, &target, &mapped_columns, &[mapped_row])
                .await
            {
                Ok(0) => {
                    failed_row_count += 1;
                    if row_failures.len() < MAX_ROW_FAILURE_DETAILS {
                        row_failures.push(serde_json::json!({
                            "rowIndex": offset + page_row_index,
                            "message": database_error_message(&database_insert_suppressed()),
                        }));
                    }
                }
                Ok(count) => copied_row_count += count,
                Err(error) => {
                    failed_row_count += 1;
                    if row_failures.len() < MAX_ROW_FAILURE_DETAILS {
                        row_failures.push(serde_json::json!({
                            "rowIndex": offset + page_row_index,
                            "message": database_error_message(&error),
                            "code": error.body.code,
                        }));
                    }
                }
            }
            // Advance only after an acknowledged outcome. A slow first batch
            // must not leave the job claiming it is still loading metadata.
            // Persist result + progress together, bounded by time/batch size;
            // crash recovery still warns about writes after the last checkpoint.
            processed_row_count += 1;
            if transfer_copy_checkpoint_due(
                processed_row_count,
                page_row_index + 1 == page_row_count,
                last_checkpoint.elapsed(),
            ) {
                job.result = Some(DatabaseTransferJobResult {
                    created_table, copied_row_count, failed_row_count,
                    ignored_source_columns: ignored_source_columns.clone(), mapped_column_count: mappings.len(),
                    column_failures: Vec::new(), row_failures: row_failures.clone(),
                });
                job.progress = transfer_copy_progress(
                    processed_row_count, total_row_count, copied_row_count, failed_row_count,
                );
                job.updated_at = Utc::now();
                persist_job(job);
                last_checkpoint = std::time::Instant::now();
            }
        }

        offset = offset.checked_add(page_row_count).ok_or_else(|| {
            ServerError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "table row offset overflow",
            )
        })?;
    }

    if failed_row_count > 0 {
        add_transfer_job_warning(
            job,
            format!("{failed_row_count} row(s) failed to copy"),
            Some(target.table_name.clone()),
            Some("ROW_COPY_FAILURES".to_string()),
            persist_job,
        );
    }

    Ok(DatabaseTransferJobResult {
        created_table,
        copied_row_count,
        failed_row_count,
        ignored_source_columns,
        mapped_column_count: mappings.len(),
        column_failures: Vec::new(),
        row_failures,
    })
}

async fn read_transfer_source_page(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
    limit: usize,
    offset: usize,
    include_total_count: bool,
) -> Result<DatabaseTableData> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        return read_sqlite_table_data(
            connection,
            &endpoint.table_name,
            limit,
            offset,
            include_total_count,
        );
    }

    read_live_table_data(
        connection,
        endpoint.database_name.as_deref(),
        endpoint.schema_name.as_deref(),
        &endpoint.table_name,
        limit,
        offset,
        include_total_count,
    )
    .await
}

async fn read_transfer_source(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
) -> Result<TransferSourceSnapshot> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_connection(connection)?;
        let table =
            describe_sqlite_table_inner(&conn, &endpoint.table_name, DatabaseObjectType::Table)?;
        let sql = format!("SELECT * FROM {}", quote_identifier(&endpoint.table_name));
        let mut stmt = conn.prepare(&sql).map_err(sqlite_server_error)?;
        let column_names = stmt
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut rows = stmt.query([]).map_err(sqlite_server_error)?;
        let mut output = Vec::new();
        while let Some(row) = rows.next().map_err(sqlite_server_error)? {
            output.push(row_to_json_map(row, &column_names)?);
        }
        return Ok(TransferSourceSnapshot {
            columns: table.columns,
            rows: output,
        });
    }

    let table = describe_live_table(
        connection,
        endpoint.database_name.as_deref(),
        endpoint.schema_name.as_deref(),
        &endpoint.table_name,
        DatabaseObjectType::Table,
    )
    .await?;
    let rows = fetch_live_row_maps(
        connection,
        endpoint.database_name.as_deref(),
        &format!("SELECT * FROM {}", transfer_table_ref(connection.profile.db_type, endpoint)),
    ).await?;
    Ok(TransferSourceSnapshot {
        columns: table.columns,
        rows,
    })
}

async fn transfer_target_exists(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
) -> Result<bool> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_transfer_target_connection(connection)?;
        return sqlite_table_exists(&conn, &endpoint.table_name);
    }

    let pool = live_pool_for_database(connection, endpoint.database_name.as_deref()).await?;
    let (_database, schema) = live_scope(
        connection,
        endpoint.database_name.as_deref(),
        endpoint.schema_name.as_deref(),
    );
    let exists_sql = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
            "SELECT 1 FROM information_schema.tables WHERE table_schema = $1 AND table_name = $2"
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            "SELECT 1 FROM information_schema.tables WHERE table_schema = ? AND table_name = ?"
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let row = sqlx::query(exists_sql)
        .bind(&schema)
        .bind(&endpoint.table_name)
        .fetch_optional(&pool)
        .await
        .map_err(sqlx_server_error)?;
    Ok(row.is_some())
}

async fn ensure_transfer_target_schema(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
) -> Result<()> {
    if connection.profile.db_type != SupportedDatabaseType::Postgresql {
        return Ok(());
    }
    let Some(schema_name) = endpoint
        .schema_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let pool = live_pool_for_database(connection, endpoint.database_name.as_deref()).await?;
    let sql = format!(
        "CREATE SCHEMA IF NOT EXISTS {}",
        quote_identifier(schema_name)
    );
    validate_database_mutation_text(connection.profile.db_type, &sql)?;
    pool.execute(sql.as_str())
        .await
        .map_err(sqlx_server_error)?;
    Ok(())
}

async fn create_transfer_target_table(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
    columns: &[DatabaseObjectColumn],
) -> Result<()> {
    let sql = build_transfer_create_table_sql(connection.profile.db_type, endpoint, columns);
    validate_database_mutation_text(connection.profile.db_type, &sql)?;
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_transfer_target_connection(connection)?;
        conn.execute_batch(&sql).map_err(sqlite_server_error)?;
        return Ok(());
    }

    ensure_transfer_target_schema(connection, endpoint).await?;
    let pool = live_pool_for_database(connection, endpoint.database_name.as_deref()).await?;
    pool.execute(sql.as_str())
        .await
        .map_err(sqlx_server_error)?;
    Ok(())
}

async fn insert_transfer_rows(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
    columns: &[DatabaseObjectColumn],
    rows: &[Map<String, Value>],
) -> Result<usize> {
    if rows.is_empty() {
        return Ok(0);
    }

    for row in rows {
        for column in columns {
            if let Some(value) = row.get(&column.name) {
                validate_database_binary_value(column, value)?;
            }
        }
    }

    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let mut conn = sqlite_transfer_target_connection(connection)?;
        let tx = conn.transaction().map_err(sqlite_server_error)?;
        let mut inserted = 0usize;
        for row in rows {
            let sql = build_transfer_insert_sql(connection.profile.db_type, endpoint, columns, row);
            inserted += tx.execute(&sql, []).map_err(sqlite_server_error)?;
        }
        tx.commit().map_err(sqlite_server_error)?;
        return Ok(inserted);
    }

    let pool = live_pool_for_database(connection, endpoint.database_name.as_deref()).await?;
    let mut inserted = 0usize;
    for row in rows {
        let sql = build_transfer_insert_sql(connection.profile.db_type, endpoint, columns, row);
        validate_database_mutation_text(connection.profile.db_type, &sql)?;
        inserted += pool.execute(sql.as_str())
            .await
            .map_err(sqlx_server_error)?.rows_affected() as usize;
    }
    Ok(inserted)
}

fn build_transfer_create_table_sql(
    db_type: SupportedDatabaseType,
    endpoint: &DatabaseTransferEndpoint,
    columns: &[DatabaseObjectColumn],
) -> String {
    let primary_key_columns = columns
        .iter()
        .filter(|column| column.is_primary_key)
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    let single_primary_key = primary_key_columns.len() == 1;
    let mut definitions = Vec::new();
    for column in columns {
        let mut definition = format!(
            "{} {}",
            quote_transfer_identifier(db_type, &column.name),
            transfer_column_type(db_type, column)
        );
        if single_primary_key && column.is_primary_key {
            definition.push_str(" PRIMARY KEY");
        }
        // A SQLite text PRIMARY KEY does not imply NOT NULL. Preserve the
        // source constraint explicitly, including on single-column keys.
        if column.nullable == Some(false) {
            definition.push_str(" NOT NULL");
        }
        definitions.push(definition);
    }
    if primary_key_columns.len() > 1 {
        definitions.push(format!(
            "PRIMARY KEY ({})",
            primary_key_columns
                .iter()
                .map(|column| quote_transfer_identifier(db_type, column))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    format!(
        "CREATE TABLE IF NOT EXISTS {} ({})",
        transfer_table_ref(db_type, endpoint),
        definitions.join(", ")
    )
}

fn build_transfer_insert_sql(
    db_type: SupportedDatabaseType,
    endpoint: &DatabaseTransferEndpoint,
    columns: &[DatabaseObjectColumn],
    row: &Map<String, Value>,
) -> String {
    let metadata = DatabaseTableMetadata {
        columns: columns.to_vec(),
        primary_key: Vec::new(),
    };
    let column_names = columns
        .iter()
        .map(|column| quote_transfer_identifier(db_type, &column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let values = columns
        .iter()
        .map(|column| {
            row.get(&column.name)
                .map(|value| database_row_value_literal(db_type, &metadata, &column.name, value))
                .unwrap_or_else(|| "NULL".to_string())
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {} ({column_names}) VALUES ({values})",
        transfer_table_ref(db_type, endpoint)
    )
}

fn transfer_table_ref(
    db_type: SupportedDatabaseType,
    endpoint: &DatabaseTransferEndpoint,
) -> String {
    match db_type {
        SupportedDatabaseType::Postgresql => format!(
            "{}.{}",
            quote_identifier(endpoint.schema_name.as_deref().unwrap_or("public")),
            quote_identifier(&endpoint.table_name)
        ),
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            if let Some(database_name) = endpoint.database_name.as_deref() {
                format!(
                    "{}.{}",
                    quote_mysql_identifier(database_name),
                    quote_mysql_identifier(&endpoint.table_name)
                )
            } else {
                quote_mysql_identifier(&endpoint.table_name)
            }
        }
        SupportedDatabaseType::Sqlite => quote_identifier(&endpoint.table_name),
    }
}

fn quote_transfer_identifier(db_type: SupportedDatabaseType, value: &str) -> String {
    match db_type {
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            quote_mysql_identifier(value)
        }
        SupportedDatabaseType::Postgresql | SupportedDatabaseType::Sqlite => {
            quote_identifier(value)
        }
    }
}

fn transfer_column_type(
    db_type: SupportedDatabaseType,
    column: &DatabaseObjectColumn,
) -> &'static str {
    if let Some(width) = transfer_unsigned_integer_width(column) {
        return match db_type {
            SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => match width {
                8 => "TINYINT UNSIGNED",
                16 => "SMALLINT UNSIGNED",
                24 => "MEDIUMINT UNSIGNED",
                32 => "INT UNSIGNED",
                _ => "BIGINT UNSIGNED",
            },
            SupportedDatabaseType::Postgresql => match width {
                64 => "NUMERIC(20, 0)",
                32 => "BIGINT",
                _ => "INTEGER",
            },
            SupportedDatabaseType::Sqlite => if width == 64 { "TEXT" } else { "INTEGER" },
        };
    }
    let native = column
        .native_type
        .as_deref()
        .or(column.data_type.as_deref())
        .unwrap_or("")
        .to_ascii_lowercase();
    match db_type {
        SupportedDatabaseType::Sqlite => {
            if native.contains("bool") || native.contains("int") {
                "INTEGER"
            } else if native.contains("decimal") || native.contains("numeric") {
                "NUMERIC"
            } else if native.contains("real")
                || native.contains("float")
                || native.contains("double")
            {
                "REAL"
            } else if native.contains("blob") || native.contains("binary") {
                "BLOB"
            } else {
                "TEXT"
            }
        }
        SupportedDatabaseType::Postgresql => {
            if native.contains("bool") {
                "BOOLEAN"
            } else if native.contains("bigint") {
                "BIGINT"
            } else if native.contains("int") {
                "INTEGER"
            } else if native.contains("decimal") || native.contains("numeric") {
                "NUMERIC"
            } else if native.contains("real")
                || native.contains("float")
                || native.contains("double")
            {
                "DOUBLE PRECISION"
            } else if native.contains("json") {
                "JSONB"
            } else if native.contains("uuid") {
                "UUID"
            } else if native.contains("blob")
                || native.contains("binary")
                || native.contains("bytea")
            {
                "BYTEA"
            } else if native.contains("timestamp") || native.contains("datetime") {
                "TIMESTAMP"
            } else if native == "date" {
                "DATE"
            } else if native == "time" {
                "TIME"
            } else {
                "TEXT"
            }
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            if native.contains("bool") {
                "BOOLEAN"
            } else if native.contains("bigint") {
                "BIGINT"
            } else if native.contains("int") {
                "INT"
            } else if native.contains("decimal") || native.contains("numeric") {
                "DECIMAL(38, 10)"
            } else if native.contains("real")
                || native.contains("float")
                || native.contains("double")
            {
                "DOUBLE"
            } else if native.contains("json") {
                if db_type == SupportedDatabaseType::Mysql {
                    "JSON"
                } else {
                    "LONGTEXT"
                }
            } else if native.contains("uuid") {
                "CHAR(36)"
            } else if native.contains("blob")
                || native.contains("binary")
                || native.contains("bytea")
            {
                "LONGBLOB"
            } else if native.contains("timestamp") || native.contains("datetime") {
                "DATETIME"
            } else if native == "date" {
                "DATE"
            } else if native == "time" {
                "TIME"
            } else {
                "TEXT"
            }
        }
    }
}

fn transfer_unsigned_integer_width(column: &DatabaseObjectColumn) -> Option<u8> {
    let native = column.native_type.as_deref().or(column.data_type.as_deref())?.to_ascii_lowercase();
    let tokens = native.split(|character: char| !character.is_ascii_alphanumeric()).collect::<Vec<_>>();
    if !tokens.contains(&"unsigned") && !tokens.contains(&"zerofill") {
        return None;
    }
    match tokens.first().copied() {
        Some("tinyint") => Some(8),
        Some("smallint") => Some(16),
        Some("mediumint") => Some(24),
        Some("int" | "integer") => Some(32),
        Some("bigint") => Some(64),
        _ => None,
    }
}

fn transfer_value_literal(db_type: SupportedDatabaseType, value: &Value) -> String {
    if let Some(encoded) = database_buffer_base64(value) {
        return match db_type {
            SupportedDatabaseType::Postgresql => {
                format!("decode({}, 'base64')", database_text_literal(db_type, encoded))
            }
            SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
                format!("FROM_BASE64({})", database_text_literal(db_type, encoded))
            }
            SupportedDatabaseType::Sqlite => BASE64_STANDARD
                .decode(encoded)
                .map(|bytes| {
                    let hex = bytes
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>();
                    format!("X'{hex}'")
                })
                .unwrap_or_else(|_| "NULL".to_string()),
        };
    }
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(value) => {
            if *value {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        }
        Value::Number(value) => value.to_string(),
        Value::String(value) => database_text_literal(db_type, value),
        Value::Array(_) | Value::Object(_) => {
            database_text_literal(db_type, &serde_json::to_string(value).unwrap_or_default())
        }
    }
}

fn database_text_literal(db_type: SupportedDatabaseType, value: &str) -> String {
    match db_type {
        SupportedDatabaseType::Postgresql => {
            let mut tag = "iowb".to_string();
            while value.contains(&format!("${tag}$")) {
                tag.push('_');
            }
            format!("${tag}${value}${tag}$")
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            let hex = value
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            format!("CONVERT(X'{hex}' USING utf8mb4)")
        }
        SupportedDatabaseType::Sqlite => {
            let hex = value
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            format!("CAST(X'{hex}' AS TEXT)")
        }
    }
}

fn database_buffer_base64(value: &Value) -> Option<&str> {
    let object = value.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("buffer")
        || object.get("encoding").and_then(Value::as_str) != Some("base64")
    {
        return None;
    }
    object.get("value").and_then(Value::as_str)
}

fn sqlite_table_exists(conn: &Connection, table_name: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE name = ?1 AND type = 'table'",
            params![table_name],
            |_| Ok(()),
        )
        .optional()
        .map_err(sqlite_server_error)?
        .is_some())
}

fn row_to_json_map(row: &rusqlite::Row<'_>, column_names: &[String]) -> Result<Map<String, Value>> {
    let mut item = Map::new();
    for (index, name) in column_names.iter().enumerate() {
        item.insert(
            name.clone(),
            sqlite_value_to_json(row.get_ref(index).map_err(sqlite_server_error)?),
        );
    }
    Ok(item)
}

fn sqlite_value_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::Number(value.into()),
        ValueRef::Real(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        ValueRef::Text(value) => Value::String(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(value) => database_buffer_value(value),
    }
}

fn database_buffer_value(value: &[u8]) -> Value {
    serde_json::json!({
        "type": "buffer",
        "encoding": "base64",
        "value": BASE64_STANDARD.encode(value),
    })
}

fn classify_statement(sql: &str) -> DatabaseQueryStatementType {
    let keyword = sql
        .trim_start()
        .trim_start_matches('(')
        .split(|ch: char| ch.is_whitespace() || ch == ';')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match keyword.as_str() {
        "select" | "show" | "describe" | "pragma" | "with" | "explain" => {
            DatabaseQueryStatementType::Select
        }
        "insert" => DatabaseQueryStatementType::Insert,
        "update" => DatabaseQueryStatementType::Update,
        "delete" => DatabaseQueryStatementType::Delete,
        "create" | "alter" | "drop" | "truncate" => DatabaseQueryStatementType::Ddl,
        _ => DatabaseQueryStatementType::Other,
    }
}

fn parse_object_type(raw: &str) -> Result<DatabaseObjectType> {
    match raw {
        "database" => Ok(DatabaseObjectType::Database),
        "schema" => Ok(DatabaseObjectType::Schema),
        "table" => Ok(DatabaseObjectType::Table),
        "view" => Ok(DatabaseObjectType::View),
        "materialized-view" | "materializedview" => Ok(DatabaseObjectType::MaterializedView),
        "index" => Ok(DatabaseObjectType::Index),
        "trigger" => Ok(DatabaseObjectType::Trigger),
        "routine" | "function" | "procedure" => Ok(DatabaseObjectType::Routine),
        "sequence" => Ok(DatabaseObjectType::Sequence),
        "event" => Ok(DatabaseObjectType::Event),
        "constraint" => Ok(DatabaseObjectType::Constraint),
        "policy" => Ok(DatabaseObjectType::Policy),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Unsupported database object type",
        )),
    }
}

fn database_object_type_as_str(object_type: DatabaseObjectType) -> &'static str {
    match object_type {
        DatabaseObjectType::Connection => "connection",
        DatabaseObjectType::Database => "database",
        DatabaseObjectType::Schema => "schema",
        DatabaseObjectType::Table => "table",
        DatabaseObjectType::View => "view",
        DatabaseObjectType::MaterializedView => "materialized-view",
        DatabaseObjectType::Index => "index",
        DatabaseObjectType::Trigger => "trigger",
        DatabaseObjectType::Routine => "routine",
        DatabaseObjectType::Sequence => "sequence",
        DatabaseObjectType::Event => "event",
        DatabaseObjectType::Constraint => "constraint",
        DatabaseObjectType::Policy => "policy",
    }
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn enrich_transfer_endpoint(
    mut endpoint: DatabaseTransferEndpoint,
    profile: &DatabaseConnectionProfile,
) -> DatabaseTransferEndpoint {
    endpoint.connection_name = Some(profile.name.clone());
    endpoint.connection_type = Some(profile.db_type);
    endpoint.database_name = endpoint.database_name.or_else(|| {
        profile.database_name.clone().or_else(|| {
            Some(
                match profile.db_type {
                    SupportedDatabaseType::Postgresql => "postgres",
                    SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => "mysql",
                    SupportedDatabaseType::Sqlite => "main",
                }
                .to_string(),
            )
        })
    });
    endpoint
}

fn progress(current: usize, total: usize, message: &str) -> Value {
    let safe_total = total.max(1);
    let safe_current = current.min(safe_total);
    let percentage = ((safe_current as f64 / safe_total as f64) * 100.0).round() as usize;
    serde_json::json!({
        "current": safe_current,
        "total": safe_total,
        "percentage": percentage,
        "message": message,
    })
}

fn database_error_message(error: &ServerError) -> String {
    error
        .body
        .details
        .as_deref()
        .filter(|details| !details.trim().is_empty())
        .unwrap_or(&error.body.error)
        .to_string()
}

fn database_error_response(
    error: ServerError,
    fallback_message: &str,
    context: Option<Value>,
) -> Response {
    let message = database_error_message(&error);
    let code = error.body.code.unwrap_or_else(|| {
        if error.status == StatusCode::NOT_FOUND {
            "NOT_FOUND".to_string()
        } else {
            "DATABASE_ERROR".to_string()
        }
    });
    let category = error.body.category.unwrap_or_else(|| {
        if error.status == StatusCode::BAD_REQUEST {
            "validation".to_string()
        } else {
            "unknown".to_string()
        }
    });
    let retryable = error.body.retryable.unwrap_or(false);
    let status = if error.status == StatusCode::NOT_FOUND
        || code == "TABLE_NOT_FOUND"
        || code == "SESSION_NOT_FOUND"
    {
        StatusCode::NOT_FOUND
    } else if matches!(
        category.as_str(),
        "connection" | "authentication" | "validation" | "metadata"
    ) {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    let mut payload = serde_json::json!({
        "error": if message.trim().is_empty() { fallback_message } else { &message },
        "code": code,
        "category": category,
        "retryable": retryable,
    });
    if let Some(context) = context {
        payload
            .as_object_mut()
            .expect("database error payload must be an object")
            .insert("context".to_string(), context);
    }
    (status, Json(payload)).into_response()
}

fn sqlite_server_error(error: rusqlite::Error) -> ServerError {
    let message = error.to_string();
    let raw_code = error
        .sqlite_error_code()
        .map(|code| format!("{code:?}"))
        .unwrap_or_default();
    let code = match raw_code.as_str() {
        "DatabaseBusy" => "SQLITE_BUSY",
        "DatabaseLocked" => "SQLITE_LOCKED",
        "ReadOnly" => "SQLITE_READONLY",
        "CannotOpen" => "SQLITE_CANTOPEN",
        "ConstraintViolation" => "SQLITE_CONSTRAINT",
        _ => "SQLITE_ERROR",
    };
    let retryable = matches!(code, "SQLITE_BUSY" | "SQLITE_LOCKED");
    let category = if matches!(
        code,
        "SQLITE_BUSY" | "SQLITE_LOCKED" | "SQLITE_READONLY" | "SQLITE_CANTOPEN"
    ) {
        "connection"
    } else {
        "execution"
    };
    ServerError::database(
        if category == "execution" {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::BAD_REQUEST
        },
        message,
        None,
        code,
        category,
        retryable,
    )
}

fn sqlx_server_error(error: sqlx::Error) -> ServerError {
    let vendor_code = match &error {
        sqlx::Error::Database(database_error) => database_error
            .try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
            .map(|error| normalize_sqlx_vendor_code(&error.number().to_string()).to_string())
            .or_else(|| database_error.code().map(|code| normalize_sqlx_vendor_code(code.as_ref()).to_string())),
        _ => None,
    };
    let mut message = error.to_string();
    if vendor_code.as_deref() == Some("ER_DUP_ENTRY") {
        message = redact_duplicate_entry_value(&message);
    }
    let normalized = message.to_ascii_lowercase();
    let code = vendor_code.unwrap_or_else(|| {
        if normalized.contains("connection refused") {
            "ECONNREFUSED".to_string()
        } else if normalized.contains("timed out") || normalized.contains("timeout") {
            "ETIMEDOUT".to_string()
        } else if normalized.contains("dns") || normalized.contains("name or service not known") {
            "ENOTFOUND".to_string()
        } else {
            "DATABASE_ERROR".to_string()
        }
    });
    let category = if code == "28P01"
        || code == "ER_ACCESS_DENIED_ERROR"
        || normalized.contains("access denied")
        || normalized.contains("authentication")
        || normalized.contains("password")
    {
        "authentication"
    } else if code == "3D000"
        || code == "ER_BAD_DB_ERROR"
        || normalized.contains("unknown database")
        || normalized.contains("does not exist")
        || normalized.contains("not found")
    {
        "metadata"
    } else if matches!(code.as_str(), "ECONNREFUSED" | "ETIMEDOUT" | "ENOTFOUND")
        || normalized.contains("connection")
    {
        "connection"
    } else {
        "execution"
    };
    let retryable = matches!(code.as_str(), "ECONNREFUSED" | "ETIMEDOUT" | "ENOTFOUND");
    ServerError::database(
        if category == "execution" {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::BAD_REQUEST
        },
        message,
        None,
        code,
        category,
        retryable,
    )
}

fn redact_duplicate_entry_value(message: &str) -> String {
    let constraint = message.rsplit_once(" for key ").map(|(_, key)| key);
    match constraint {
        Some(key) => format!("Duplicate key violates unique constraint {key}"),
        None => "Duplicate key violates a unique constraint".to_string(),
    }
}

fn normalize_sqlx_vendor_code(code: &str) -> &str {
    match code {
        "1045" => "ER_ACCESS_DENIED_ERROR",
        "1048" => "ER_BAD_NULL_ERROR",
        "1049" => "ER_BAD_DB_ERROR",
        "1054" => "ER_BAD_FIELD_ERROR",
        "1062" => "ER_DUP_ENTRY",
        "1064" => "ER_PARSE_ERROR",
        "1146" => "ER_NO_SUCH_TABLE",
        "1205" => "ER_LOCK_WAIT_TIMEOUT",
        "1213" => "ER_LOCK_DEADLOCK",
        "1366" => "ER_TRUNCATED_WRONG_VALUE_FOR_FIELD",
        "1406" => "ER_DATA_TOO_LONG",
        "1451" => "ER_ROW_IS_REFERENCED_2",
        "1452" => "ER_NO_REFERENCED_ROW_2",
        _ => code,
    }
}

fn io_server_error(error: std::io::Error) -> ServerError {
    ServerError::database(
        StatusCode::BAD_REQUEST,
        error.to_string(),
        None,
        "DATABASE_FILESYSTEM_ERROR",
        "validation",
        false,
    )
}

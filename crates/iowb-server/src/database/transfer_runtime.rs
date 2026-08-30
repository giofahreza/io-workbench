struct TransferSourceSnapshot {
    columns: Vec<DatabaseObjectColumn>,
    rows: Vec<Map<String, Value>>,
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

fn set_transfer_job_progress<F>(
    job: &mut DatabaseTransferJob,
    current: usize,
    total: usize,
    message: impl AsRef<str>,
    persist_job: &mut F,
) where
    F: FnMut(&DatabaseTransferJob),
{
    job.progress = progress(current, total, message.as_ref());
    job.updated_at = Utc::now();
    persist_job(job);
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

    append_transfer_job_log(
        job,
        "info",
        "Opening source and target connections",
        None,
        persist_job,
    );
    set_transfer_job_progress(job, 0, 4, "Opening connections", persist_job);
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
    set_transfer_job_progress(job, 1, 4, "Inspecting source table", persist_job);

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
            set_transfer_job_progress(job, 2, 4, "Preparing target table", persist_job);
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

    set_transfer_job_progress(job, 3, 4, "Loading target table metadata", persist_job);
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
    let mappings = build_column_mappings(&source_column_names, &target_metadata.columns);
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
    let mut copied_row_count = 0usize;
    let mut failed_row_count = 0usize;
    let mut row_failures = Vec::new();
    let mut processed_row_count = 0usize;
    let mut total_row_count = 0usize;
    let mut offset = 0usize;

    loop {
        let page = read_transfer_source_page(
            source_connection,
            &source,
            TRANSFER_ROW_BATCH_SIZE,
            offset,
            offset == 0,
        )
        .await?;
        if let Some(total) = page.total_row_count {
            total_row_count = total;
        }
        let page_row_count = page.rows.len();
        if page_row_count == 0 {
            break;
        }

        for (page_row_index, source_row) in page.rows.iter().enumerate() {
            let mapped_row = mappings
                .iter()
                .map(|(target_name, source_name)| {
                    (
                        target_name.clone(),
                        source_row.get(source_name).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect::<Map<_, _>>();
            match insert_transfer_rows(target_connection, &target, &mapped_columns, &[mapped_row])
                .await
            {
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
        }

        processed_row_count = processed_row_count.saturating_add(page_row_count);
        offset = offset.checked_add(page_row_count).ok_or_else(|| {
            ServerError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "table row offset overflow",
            )
        })?;
        let progress_total = total_row_count.max(processed_row_count).max(1);
        set_transfer_job_progress(
            job,
            processed_row_count,
            progress_total,
            format!(
                "Copied {} of {} row(s)",
                processed_row_count, progress_total
            ),
            persist_job,
        );

        if !page.has_more {
            break;
        }
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
    let mut rows = Vec::new();
    let mut offset = 0usize;
    loop {
        let page = read_live_table_data(
            connection,
            endpoint.database_name.as_deref(),
            endpoint.schema_name.as_deref(),
            &endpoint.table_name,
            TRANSFER_ROW_BATCH_SIZE,
            offset,
            false,
        )
        .await?;
        let page_row_count = page.rows.len();
        rows.extend(page.rows);
        if !page.has_more || page_row_count == 0 {
            break;
        }
        offset = offset.checked_add(page_row_count).ok_or_else(|| {
            ServerError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "table row offset overflow",
            )
        })?;
    }
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

    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let mut conn = sqlite_transfer_target_connection(connection)?;
        let tx = conn.transaction().map_err(sqlite_server_error)?;
        for row in rows {
            let sql = build_transfer_insert_sql(connection.profile.db_type, endpoint, columns, row);
            tx.execute(&sql, []).map_err(sqlite_server_error)?;
        }
        tx.commit().map_err(sqlite_server_error)?;
        return Ok(rows.len());
    }

    let pool = live_pool_for_database(connection, endpoint.database_name.as_deref()).await?;
    for row in rows {
        let sql = build_transfer_insert_sql(connection.profile.db_type, endpoint, columns, row);
        pool.execute(sql.as_str())
            .await
            .map_err(sqlx_server_error)?;
    }
    Ok(rows.len())
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
        if column.nullable == Some(false) && !(single_primary_key && column.is_primary_key) {
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
    let column_names = columns
        .iter()
        .map(|column| quote_transfer_identifier(db_type, &column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let values = columns
        .iter()
        .map(|column| {
            row.get(&column.name)
                .map(|value| transfer_value_literal(db_type, value))
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

fn transfer_value_literal(db_type: SupportedDatabaseType, value: &Value) -> String {
    if let Some(encoded) = database_buffer_base64(value) {
        return match db_type {
            SupportedDatabaseType::Postgresql => {
                format!("decode('{encoded}', 'base64')")
            }
            SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
                format!("FROM_BASE64('{encoded}')")
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
            .code()
            .map(|code| normalize_sqlx_vendor_code(code.as_ref()).to_string()),
        _ => None,
    };
    let message = error.to_string();
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

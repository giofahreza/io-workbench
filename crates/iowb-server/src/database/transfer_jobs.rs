async fn create_transfer(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<DatabaseTransferRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    if request.source.table_name.trim().is_empty() || request.target.table_name.trim().is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "source and target table names are required",
        ));
    }
    if request.source.connection_id == request.target.connection_id
        && request.source.database_name == request.target.database_name
        && request.source.schema_name == request.target.schema_name
        && request.source.table_name == request.target.table_name
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Source and target tables must be different",
        ));
    }

    let source_connection = load_connection(&state, &user.0.id, request.source.connection_id)?;
    let target_connection = load_connection(&state, &user.0.id, request.target.connection_id)?;

    let now = Utc::now();
    let job = DatabaseTransferJob {
        id: new_id("dbjob"),
        job_type: "table-transfer".to_string(),
        mode: request.mode,
        status: DatabaseTransferJobStatus::Running,
        source: enrich_transfer_endpoint(request.source, &source_connection.profile),
        target: enrich_transfer_endpoint(request.target, &target_connection.profile),
        progress: progress(0, 1, "Queued"),
        logs: vec![DatabaseTransferJobLogEntry {
            timestamp: now,
            level: "info".to_string(),
            message: "Transfer queued".to_string(),
            details: None,
        }],
        warnings: Vec::new(),
        error: None,
        result: None,
        created_at: now,
        updated_at: now,
        started_at: None,
        finished_at: None,
    };

    state
        .storage
        .upsert_database_transfer_job(&user.0.id, &job)?;

    let response_job = job.clone();
    let task_state = state.clone();
    let user_id = user.0.id.clone();
    tokio::spawn(async move {
        let mut job = job;
        let started_at = Utc::now();
        job.started_at = Some(started_at);
        job.updated_at = started_at;
        let storage = task_state.storage.clone();
        let persist_user_id = user_id.clone();
        let transfer_result = {
            let mut persist_job = move |updated_job: &DatabaseTransferJob| {
                let _ = storage.upsert_database_transfer_job(&persist_user_id, updated_job);
            };
            run_database_transfer(
                &source_connection,
                &target_connection,
                &mut job,
                &mut persist_job,
            )
            .await
        };

        match transfer_result {
            Ok(result) => {
                let finished_at = Utc::now();
                let completion_message = match job.mode {
                    DatabaseTransferMode::TableCopy => "Table copy completed",
                    DatabaseTransferMode::SchemaOnly => "Schema copy completed",
                    DatabaseTransferMode::SchemaAndData => "Schema and data copy completed",
                };
                job.status = DatabaseTransferJobStatus::Succeeded;
                job.progress = progress(1, 1, completion_message);
                job.logs.push(DatabaseTransferJobLogEntry {
                    timestamp: finished_at,
                    level: "info".to_string(),
                    message: completion_message.to_string(),
                    details: None,
                });
                job.updated_at = finished_at;
                job.finished_at = Some(finished_at);
                job.result = Some(result);
            }
            Err(error) => fail_database_job(&mut job, error),
        }
        let _ = task_state
            .storage
            .upsert_database_transfer_job(&user_id, &job);
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "success": true,
            "job": response_job,
        })),
    ))
}

async fn list_jobs(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "success": true,
        "jobs": state.storage.list_database_transfer_jobs(&user.0.id)?,
    })))
}

async fn get_job(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(job_id): AxumPath<String>,
) -> Result<Json<Value>> {
    let job = state
        .storage
        .get_database_transfer_job(&user.0.id, &job_id)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Transfer job not found"))?;
    Ok(Json(serde_json::json!({
        "success": true,
        "job": job,
    })))
}

async fn export_table(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<DatabaseFileJobRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    if request.table_name.trim().is_empty() || request.file_path.trim().is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "tableName and filePath are required",
        ));
    }
    let connection = load_connection(&state, &user.0.id, request.connection_id)?;
    let output_path = state
        .path_validator
        .validate_path(PathBuf::from(&request.file_path), true)
        .await?;
    let endpoint = DatabaseTransferEndpoint {
        connection_id: request.connection_id,
        connection_name: Some(connection.profile.name.clone()),
        connection_type: Some(connection.profile.db_type),
        database_name: request
            .database_name
            .or_else(|| Some(default_database_name(&connection))),
        schema_name: request.schema_name,
        table_name: request.table_name,
    };
    let now = Utc::now();
    let mut job = database_file_job("data-export", &endpoint, &endpoint, now);

    match read_transfer_source(&connection, &endpoint).await {
        Ok(snapshot) => {
            let finished_at = Utc::now();
            let export = serde_json::json!({
                "format": "io-workbench.table-export.v1",
                "connection": connection.profile,
                "table": endpoint,
                "columns": snapshot.columns,
                "rows": snapshot.rows,
                "truncated": false,
                "exportedAt": finished_at,
            });
            if let Some(parent) = output_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(io_server_error)?;
            }
            let content = serde_json::to_vec_pretty(&export).map_err(|error| {
                ServerError::with_details(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to encode export",
                    error.to_string(),
                )
            })?;
            tokio::fs::write(&output_path, content)
                .await
                .map_err(io_server_error)?;
            job.status = DatabaseTransferJobStatus::Succeeded;
            job.progress = progress(1, 1, "Export completed");
            job.updated_at = finished_at;
            job.finished_at = Some(finished_at);
            job.result = Some(DatabaseTransferJobResult {
                created_table: false,
                copied_row_count: snapshot.rows.len(),
                failed_row_count: 0,
                ignored_source_columns: Vec::new(),
                mapped_column_count: snapshot.columns.len(),
                column_failures: Vec::new(),
                row_failures: Vec::new(),
            });
            job.logs.push(DatabaseTransferJobLogEntry {
                timestamp: finished_at,
                level: "info".to_string(),
                message: format!("Exported table to {}", output_path.display()),
                details: None,
            });
        }
        Err(error) => fail_database_job(&mut job, error),
    }

    state
        .storage
        .upsert_database_transfer_job(&user.0.id, &job)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "success": job.status == DatabaseTransferJobStatus::Succeeded,
            "job": job,
        })),
    ))
}

async fn import_table(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<DatabaseFileJobRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    if request.table_name.trim().is_empty() || request.file_path.trim().is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "tableName and filePath are required",
        ));
    }
    let connection = load_connection(&state, &user.0.id, request.connection_id)?;
    let input_path = state
        .path_validator
        .validate_path(PathBuf::from(&request.file_path), false)
        .await?;
    let endpoint = DatabaseTransferEndpoint {
        connection_id: request.connection_id,
        connection_name: Some(connection.profile.name.clone()),
        connection_type: Some(connection.profile.db_type),
        database_name: request
            .database_name
            .or_else(|| Some(default_database_name(&connection))),
        schema_name: request.schema_name,
        table_name: request.table_name,
    };
    let now = Utc::now();
    let mut job = database_file_job("data-import", &endpoint, &endpoint, now);

    match read_import_rows(&input_path).await {
        Ok(rows) => {
            let columns = infer_import_columns(&rows);
            let result = async {
                let target_exists = transfer_target_exists(&connection, &endpoint).await?;
                if !target_exists {
                    create_transfer_target_table(&connection, &endpoint, &columns).await?;
                }
                let copied = insert_transfer_rows(&connection, &endpoint, &columns, &rows).await?;
                Ok::<_, ServerError>((target_exists, copied))
            }
            .await;
            match result {
                Ok((target_exists, copied)) => {
                    let finished_at = Utc::now();
                    job.status = DatabaseTransferJobStatus::Succeeded;
                    job.progress = progress(1, 1, "Import completed");
                    job.updated_at = finished_at;
                    job.finished_at = Some(finished_at);
                    job.result = Some(DatabaseTransferJobResult {
                        created_table: !target_exists,
                        copied_row_count: copied,
                        failed_row_count: 0,
                        ignored_source_columns: Vec::new(),
                        mapped_column_count: columns.len(),
                        column_failures: Vec::new(),
                        row_failures: Vec::new(),
                    });
                    job.logs.push(DatabaseTransferJobLogEntry {
                        timestamp: finished_at,
                        level: "info".to_string(),
                        message: format!("Imported rows from {}", input_path.display()),
                        details: None,
                    });
                }
                Err(error) => fail_database_job(&mut job, error),
            }
        }
        Err(error) => fail_database_job(&mut job, error),
    }

    state
        .storage
        .upsert_database_transfer_job(&user.0.id, &job)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "success": job.status == DatabaseTransferJobStatus::Succeeded,
            "job": job,
        })),
    ))
}

fn database_file_job(
    job_type: &str,
    source: &DatabaseTransferEndpoint,
    target: &DatabaseTransferEndpoint,
    now: chrono::DateTime<Utc>,
) -> DatabaseTransferJob {
    DatabaseTransferJob {
        id: new_id("dbjob"),
        job_type: job_type.to_string(),
        mode: DatabaseTransferMode::SchemaAndData,
        status: DatabaseTransferJobStatus::Running,
        source: source.clone(),
        target: target.clone(),
        progress: progress(0, 1, "Job started"),
        logs: vec![DatabaseTransferJobLogEntry {
            timestamp: now,
            level: "info".to_string(),
            message: format!("{job_type} job created"),
            details: None,
        }],
        warnings: Vec::new(),
        error: None,
        result: None,
        created_at: now,
        updated_at: now,
        started_at: Some(now),
        finished_at: None,
    }
}

fn fail_database_job(job: &mut DatabaseTransferJob, error: ServerError) {
    let finished_at = Utc::now();
    let body = error.body;
    let details = body.details.map(|details| {
        serde_json::from_str::<Value>(&details).unwrap_or_else(|_| Value::String(details))
    });
    let message = body.error;
    let code = body.code;
    job.status = DatabaseTransferJobStatus::Failed;
    job.logs.push(DatabaseTransferJobLogEntry {
        timestamp: finished_at,
        level: "error".to_string(),
        message: message.clone(),
        details: code.clone(),
    });
    job.error = Some(DatabaseTransferJobError {
        message,
        code,
        category: body.category,
        retryable: body.retryable.unwrap_or(false),
        details,
    });
    job.updated_at = finished_at;
    job.finished_at = Some(finished_at);
}

async fn read_import_rows(path: &PathBuf) -> Result<Vec<Map<String, Value>>> {
    let content = tokio::fs::read(path).await.map_err(io_server_error)?;
    let value = serde_json::from_slice::<Value>(&content).map_err(|error| {
        ServerError::with_details(
            StatusCode::BAD_REQUEST,
            "import file must be valid JSON",
            error.to_string(),
        )
    })?;
    let rows_value = value.get("rows").cloned().unwrap_or(value);
    let rows = rows_value
        .as_array()
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "import JSON must be an array or an object with a rows array",
            )
        })?
        .iter()
        .map(|row| {
            row.as_object().cloned().ok_or_else(|| {
                ServerError::new(StatusCode::BAD_REQUEST, "import rows must be JSON objects")
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "import file has no rows",
        ));
    }
    Ok(rows)
}

fn infer_import_columns(rows: &[Map<String, Value>]) -> Vec<DatabaseObjectColumn> {
    let mut names = Vec::new();
    for row in rows {
        for key in row.keys() {
            if !names.contains(key) {
                names.push(key.clone());
            }
        }
    }
    names
        .into_iter()
        .map(|name| DatabaseObjectColumn {
            name,
            data_type: Some("text".to_string()),
            native_type: Some("TEXT".to_string()),
            nullable: Some(true),
            default_value: None,
            extra: None,
            is_primary_key: false,
        })
        .collect()
}

#[derive(Clone)]
struct DatabaseTableMetadata {
    columns: Vec<DatabaseObjectColumn>,
    primary_key: Vec<String>,
}

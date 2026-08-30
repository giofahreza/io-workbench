async fn table_data(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Query(query): Query<TableDataQuery>,
) -> Result<Json<Value>> {
    if query.table_name.trim().is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "tableName is required",
        ));
    }

    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let limit = query
        .limit
        .filter(|value| *value > 0)
        .map(|value| value.min(MAX_TABLE_PAGE_SIZE))
        .unwrap_or(DEFAULT_TABLE_PAGE_SIZE);
    let offset = query.offset.unwrap_or(0);
    let mut data = if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        read_sqlite_table_data(
            &connection,
            &query.table_name,
            limit,
            offset,
            query.include_total_count.unwrap_or(false),
        )?
    } else {
        read_live_table_data(
            &connection,
            query.database_name.as_deref(),
            query.schema_name.as_deref(),
            &query.table_name,
            limit,
            offset,
            query.include_total_count.unwrap_or(false),
        )
        .await?
    };
    data.database_name = query
        .database_name
        .or_else(|| Some(default_database_name(&connection)));
    data.schema_name = query.schema_name;

    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "data": data,
    })))
}

async fn paste_table_rows(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabasePasteRequest>,
) -> Result<Response> {
    validate_table_scope(&request.scope)?;
    let (source_column_names, source_rows) = if let Some(clipboard) = request.clipboard {
        if clipboard.clipboard_type != "rows"
            || clipboard.columns.is_empty()
            || clipboard.rows.is_empty()
        {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "A structured row clipboard payload is required",
                    "code": "INVALID_CLIPBOARD",
                    "category": "validation",
                    "retryable": false,
                })),
            )
                .into_response());
        }
        (
            clipboard
                .columns
                .into_iter()
                .map(|column| column.name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>(),
            clipboard.rows,
        )
    } else {
        if request.source_rows.is_empty() {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "A structured row clipboard payload is required",
                    "code": "INVALID_CLIPBOARD",
                    "category": "validation",
                    "retryable": false,
                })),
            )
                .into_response());
        }
        let names = if request.source_columns.is_empty() {
            request
                .source_rows
                .iter()
                .flat_map(|row| row.keys().cloned())
                .fold(Vec::<String>::new(), |mut names, name| {
                    if !names.iter().any(|candidate| candidate == &name) {
                        names.push(name);
                    }
                    names
                })
        } else {
            request
                .source_columns
                .into_iter()
                .map(|column| column.name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect()
        };
        (names, request.source_rows)
    };

    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let metadata = database_table_metadata(
        &connection,
        request.scope.database_name.as_deref(),
        request.scope.schema_name.as_deref(),
        &request.scope.table_name,
    )
    .await?;
    let mappings = build_column_mappings(&source_column_names, &metadata.columns);
    let used_source_columns = mappings
        .iter()
        .map(|(_, source)| source.clone())
        .collect::<Vec<_>>();
    let ignored_source_columns = source_column_names
        .iter()
        .filter(|name| !used_source_columns.iter().any(|used| used == *name))
        .cloned()
        .collect::<Vec<_>>();
    let mut column_failures = metadata
        .columns
        .iter()
        .filter(|column| {
            column.nullable == Some(false)
                && column.default_value.is_none()
                && !column.is_primary_key
                && !mappings.iter().any(|(target, _)| target == &column.name)
        })
        .map(|column| {
            serde_json::json!({
                "columnName": column.name,
                "message": "Required target column has no matching source column",
            })
        })
        .collect::<Vec<_>>();

    if mappings.is_empty() {
        column_failures.push(serde_json::json!({
            "message": "No compatible columns were found between the clipboard payload and the target table",
        }));
    }
    if mappings.is_empty() || !column_failures.is_empty() {
        let error = if mappings.is_empty() {
            "No compatible columns available for paste"
        } else {
            "Target table is missing required column mappings"
        };
        let result = serde_json::json!({
            "databaseName": request.scope.database_name,
            "schemaName": request.scope.schema_name,
            "tableName": request.scope.table_name,
            "attemptedRowCount": source_rows.len(),
            "insertedRowCount": 0,
            "failedRowCount": 0,
            "mappings": mappings.iter().map(|(target, source)| serde_json::json!({
                "targetColumnName": target,
                "sourceColumnName": source,
            })).collect::<Vec<_>>(),
            "ignoredSourceColumns": ignored_source_columns,
            "columnFailures": column_failures,
            "rowFailures": [],
        });
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": error, "result": result })),
        )
            .into_response());
    }

    let mut inserted_row_count = 0usize;
    let mut row_failures = Vec::new();
    for (row_index, source_row) in source_rows.iter().enumerate() {
        let values = mappings
            .iter()
            .map(|(target, source)| {
                (
                    target.clone(),
                    source_row.get(source).cloned().unwrap_or(Value::Null),
                )
            })
            .collect::<Map<_, _>>();
        match insert_database_row(
            &connection,
            request.scope.database_name.as_deref(),
            request.scope.schema_name.as_deref(),
            &request.scope.table_name,
            &metadata,
            &values,
        )
        .await
        {
            Ok(_) => inserted_row_count += 1,
            Err(error) => row_failures.push(serde_json::json!({
                "rowIndex": row_index,
                "message": database_error_message(&error),
                "code": error.body.code,
            })),
        }
    }

    let failed_row_count = row_failures.len();
    let result = serde_json::json!({
        "databaseName": request.scope.database_name,
        "schemaName": request.scope.schema_name,
        "tableName": request.scope.table_name,
        "attemptedRowCount": source_rows.len(),
        "insertedRowCount": inserted_row_count,
        "failedRowCount": failed_row_count,
        "mappings": mappings.iter().map(|(target, source)| serde_json::json!({
            "targetColumnName": target,
            "sourceColumnName": source,
        })).collect::<Vec<_>>(),
        "ignoredSourceColumns": ignored_source_columns,
        "columnFailures": column_failures,
        "rowFailures": row_failures,
    });
    Ok(Json(serde_json::json!({
        "success": failed_row_count == 0,
        "session": database_session(&connection.profile),
        "result": result,
    }))
    .into_response())
}

async fn edit_table_row(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabaseRowEditRequest>,
) -> Result<Json<Value>> {
    validate_table_scope(&request.scope)?;
    if request.primary_key_values.is_empty() || request.column_values.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "primaryKeyValues and columnValues must be non-empty objects",
        ));
    }
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let row = update_database_row(
        &connection,
        &request.scope,
        &request.primary_key_values,
        &request.column_values,
        &request.original_values,
    )
    .await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "row": row,
    })))
}

async fn add_table_row(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabaseRowAddRequest>,
) -> Result<Json<Value>> {
    validate_table_scope(&request.scope)?;
    if request.column_values.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "columnValues must be a non-empty object",
        ));
    }
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let metadata = database_table_metadata(
        &connection,
        request.scope.database_name.as_deref(),
        request.scope.schema_name.as_deref(),
        &request.scope.table_name,
    )
    .await?;
    let row = insert_database_row(
        &connection,
        request.scope.database_name.as_deref(),
        request.scope.schema_name.as_deref(),
        &request.scope.table_name,
        &metadata,
        &request.column_values,
    )
    .await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "row": row,
    })))
}

async fn delete_table_row(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabaseRowDeleteRequest>,
) -> Result<Json<Value>> {
    validate_table_scope(&request.scope)?;
    if request.primary_key_values.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "primaryKeyValues must be a non-empty object",
        ));
    }
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    delete_database_row(
        &connection,
        &request.scope,
        &request.primary_key_values,
        &request.original_values,
    )
    .await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "deletedRowCount": 1,
    })))
}

async fn delete_table_rows(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabaseRowsDeleteRequest>,
) -> Result<Json<Value>> {
    validate_table_scope(&request.scope)?;
    if request.primary_key_rows.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "primaryKeyRows must be a non-empty array",
        ));
    }
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let mut deleted_row_count = 0usize;
    let mut row_failures = Vec::new();
    for (row_index, primary_key_values) in request.primary_key_rows.iter().enumerate() {
        match delete_database_row(&connection, &request.scope, primary_key_values, &Map::new())
            .await
        {
            Ok(_) => deleted_row_count += 1,
            Err(error) => row_failures.push(serde_json::json!({
                "rowIndex": row_index,
                "message": error.body.details.clone().unwrap_or(error.body.error),
            })),
        }
    }
    Ok(Json(serde_json::json!({
        "success": row_failures.is_empty(),
        "session": database_session(&connection.profile),
        "deletedRowCount": deleted_row_count,
        "failedRowCount": row_failures.len(),
        "rowFailures": row_failures,
    })))
}

async fn export_portable_table(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabasePortableExportRequest>,
) -> Result<Json<Value>> {
    validate_table_scope(&request.scope)?;
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let metadata = database_table_metadata(
        &connection,
        request.scope.database_name.as_deref(),
        request.scope.schema_name.as_deref(),
        &request.scope.table_name,
    )
    .await?;
    let endpoint = DatabaseTransferEndpoint {
        connection_id,
        connection_name: Some(connection.profile.name.clone()),
        connection_type: Some(connection.profile.db_type),
        database_name: request
            .scope
            .database_name
            .clone()
            .or_else(|| Some(default_database_name(&connection))),
        schema_name: request.scope.schema_name.clone(),
        table_name: request.scope.table_name.clone(),
    };
    let snapshot = if request.include_data {
        Some(read_transfer_source(&connection, &endpoint).await?)
    } else {
        None
    };
    let mut payload = serde_json::json!({
        "format": "web-ai-cli/database-portable-v1",
        "type": if request.include_data { "table-schema-and-data" } else { "table-schema" },
        "exportedAt": Utc::now(),
        "source": {
            "connectionType": connection.profile.db_type,
            "databaseName": endpoint.database_name,
            "schemaName": endpoint.schema_name,
            "tableName": endpoint.table_name,
            "objectType": "table",
        },
        "table": {
            "name": request.scope.table_name,
            "type": "table",
            "columns": metadata.columns.iter().map(canonical_portable_column).collect::<Vec<_>>(),
            "primaryKey": metadata.primary_key,
        },
    });
    if let Some(snapshot) = snapshot {
        payload
            .as_object_mut()
            .expect("portable payload must be an object")
            .insert(
                "rows".to_string(),
                Value::Array(snapshot.rows.into_iter().map(Value::Object).collect()),
            );
    }
    Ok(Json(
        serde_json::json!({ "success": true, "payload": payload }),
    ))
}

async fn import_portable_table(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabasePortableImportRequest>,
) -> Result<Json<Value>> {
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let portable = parse_portable_table_payload(&request.payload)?;
    let table_name = request
        .table_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&portable.table_name)
        .to_string();
    if table_name.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "A target table name is required for portable imports",
        ));
    }
    let endpoint = DatabaseTransferEndpoint {
        connection_id,
        connection_name: Some(connection.profile.name.clone()),
        connection_type: Some(connection.profile.db_type),
        database_name: request
            .database_name
            .clone()
            .or_else(|| Some(default_database_name(&connection))),
        schema_name: request.schema_name.clone(),
        table_name: table_name.clone(),
    };
    if transfer_target_exists(&connection, &endpoint).await? {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!("Target table already exists: {table_name}"),
        ));
    }
    ensure_transfer_target_schema(&connection, &endpoint).await?;
    create_transfer_target_table(&connection, &endpoint, &portable.columns).await?;
    let mut imported_row_count = 0usize;
    let mut failed_row_count = 0usize;
    let mut row_failures = Vec::new();
    if portable.include_data {
        for (row_index, row) in portable.rows.iter().enumerate() {
            match insert_transfer_rows(
                &connection,
                &endpoint,
                &portable.columns,
                std::slice::from_ref(row),
            )
            .await
            {
                Ok(count) => imported_row_count += count,
                Err(error) => {
                    failed_row_count += 1;
                    if row_failures.len() < MAX_ROW_FAILURE_DETAILS {
                        row_failures.push(serde_json::json!({
                            "rowIndex": row_index,
                            "message": database_error_message(&error),
                            "code": error.body.code,
                        }));
                    }
                }
            }
        }
    }
    let result = serde_json::json!({
        "databaseName": endpoint.database_name,
        "schemaName": endpoint.schema_name,
        "tableName": endpoint.table_name,
        "createdTable": true,
        "importedRowCount": imported_row_count,
        "failedRowCount": failed_row_count,
        "ignoredSourceColumns": [],
        "mappedColumnCount": portable.columns.len(),
        "columnFailures": [],
        "rowFailures": row_failures,
    });
    Ok(Json(serde_json::json!({
        "success": result["failedRowCount"].as_u64() == Some(0),
        "session": database_session(&connection.profile),
        "result": result,
    })))
}

async fn database_status() -> Json<Value> {
    Json(serde_json::json!({
        "success": true,
        "adapters": [
            { "type": "sqlite", "implemented": true },
            { "type": "postgresql", "implemented": true },
            { "type": "mysql", "implemented": true },
            { "type": "mariadb", "implemented": true }
        ]
    }))
}

async fn list_connections(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "success": true,
        "connections": state.storage.list_database_connections(&user.0.id)?,
    })))
}

async fn create_connection(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(input): Json<DatabaseConnectionInput>,
) -> Result<Json<Value>> {
    let input = normalize_connection_input(&state, input, None).await?;
    let connection = state
        .storage
        .create_database_connection(&user.0.id, &input)?
        .profile;
    Ok(Json(serde_json::json!({
        "success": true,
        "connection": connection,
    })))
}

async fn test_unsaved_connection(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<DatabaseTestConnectionRequest>,
) -> Result<Response> {
    let existing_password = if let Some(connection_id) = request.existing_connection_id {
        state
            .storage
            .get_database_connection(&user.0.id, connection_id)?
            .and_then(|connection| connection.password)
    } else {
        None
    };
    let input = normalize_connection_input(&state, request.connection, existing_password).await?;
    match test_connection_input(&input).await {
        Ok(result) => Ok(Json(serde_json::json!({
            "success": true,
            "result": result,
        }))
        .into_response()),
        Err(error) => Ok(database_error_response(
            error,
            "Failed to test database connection",
            None,
        )),
    }
}

async fn update_connection(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(input): Json<DatabaseConnectionInput>,
) -> Result<Json<Value>> {
    let existing = state
        .storage
        .get_database_connection(&user.0.id, connection_id)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Connection not found"))?;
    let input = normalize_connection_input(&state, input, existing.password).await?;
    let connection = state
        .storage
        .update_database_connection(&user.0.id, connection_id, &input)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Connection not found"))?
        .profile;
    evict_live_pools(connection_id).await;
    Ok(Json(serde_json::json!({
        "success": true,
        "connection": connection,
    })))
}

async fn delete_connection(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
) -> Result<Json<Value>> {
    if !state
        .storage
        .delete_database_connection(&user.0.id, connection_id)?
    {
        return Err(ServerError::new(
            StatusCode::NOT_FOUND,
            "Connection not found",
        ));
    }
    evict_live_pools(connection_id).await;
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn test_saved_connection(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
) -> Result<Response> {
    let connection = state
        .storage
        .get_database_connection(&user.0.id, connection_id)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Connection not found"))?;

    match test_connection_record(&connection).await {
        Ok(result) => {
            let updated = state
                .storage
                .record_database_connection_test(
                    &user.0.id,
                    connection_id,
                    DatabaseTestStatus::Success,
                    &result.message,
                )?
                .unwrap_or(connection.profile);
            Ok(Json(serde_json::json!({
                "success": true,
                "connection": updated,
                "result": result,
            }))
            .into_response())
        }
        Err(error) => {
            let message = database_error_message(&error);
            let updated = state
                .storage
                .record_database_connection_test(
                    &user.0.id,
                    connection_id,
                    DatabaseTestStatus::Error,
                    &message,
                )?
                .unwrap_or(connection.profile);
            Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": message,
                    "connection": updated,
                    "result": {
                        "status": "error",
                        "message": message,
                    }
                })),
            )
                .into_response())
        }
    }
}

#[derive(Debug, Deserialize)]
struct ExplorerQuery {
    #[serde(rename = "nodeType")]
    node_type: Option<String>,
    #[serde(rename = "databaseName")]
    database_name: Option<String>,
    #[serde(rename = "schemaName")]
    schema_name: Option<String>,
}

async fn explorer(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Query(query): Query<ExplorerQuery>,
) -> Result<Json<Value>> {
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    if connection.profile.db_type != SupportedDatabaseType::Sqlite {
        return explorer_live(connection, query).await;
    }

    let node_type = query.node_type.as_deref().unwrap_or("connection");
    let session = database_session(&connection.profile);
    let capabilities = sqlite_capabilities();
    let nodes = match node_type {
        "connection" => vec![DatabaseExplorerNode {
            id: format!("database:{connection_id}:main::main"),
            object_type: DatabaseObjectType::Database,
            connection_id,
            name: "main".to_string(),
            database_name: Some("main".to_string()),
            schema_name: None,
            has_children: true,
            description: Some("default".to_string()),
        }],
        "database" | "schema" => sqlite_objects(&connection)?
            .into_iter()
            .map(|object| DatabaseExplorerNode {
                id: format!(
                    "{}:{connection_id}:{}::{}",
                    database_object_type_as_str(object.object_type),
                    object.database_name.as_deref().unwrap_or("main"),
                    object.name
                ),
                object_type: object.object_type,
                connection_id,
                name: object.name,
                database_name: query
                    .database_name
                    .clone()
                    .or_else(|| Some("main".to_string())),
                schema_name: query.schema_name.clone(),
                has_children: false,
                description: Some(database_object_type_as_str(object.object_type).to_string()),
            })
            .collect(),
        _ => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Unsupported explorer node type",
            ));
        }
    };

    Ok(Json(serde_json::json!({
        "success": true,
        "session": session,
        "nodes": nodes,
        "capabilities": capabilities,
    })))
}

#[derive(Debug, Deserialize)]
struct ObjectDetailsQuery {
    #[serde(rename = "objectType")]
    object_type: String,
    #[serde(rename = "databaseName")]
    database_name: Option<String>,
    #[serde(rename = "schemaName")]
    schema_name: Option<String>,
    name: Option<String>,
    #[serde(rename = "includeRelational")]
    include_relational: Option<bool>,
}

async fn object_details(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Query(query): Query<ObjectDetailsQuery>,
) -> Result<Json<Value>> {
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    if connection.profile.db_type != SupportedDatabaseType::Sqlite {
        return object_details_live(connection, query).await;
    }

    let object_type = parse_object_type(&query.object_type)?;
    let name = query.name.clone().unwrap_or_else(|| "main".to_string());
    let details = match object_type {
        DatabaseObjectType::Database => {
            let objects = sqlite_objects(&connection)?;
            let relational_schema = if query.include_relational.unwrap_or(false) {
                Some(sqlite_relational_schema(&connection, &objects)?)
            } else {
                None
            };
            DatabaseObjectDetails {
                database_name: Some(name.clone()),
                schema_name: query.schema_name.clone(),
                name,
                object_type,
                columns: Vec::new(),
                primary_key: Vec::new(),
                foreign_keys: Vec::new(),
                relational_schema,
                databases: vec![DatabaseNameSummary {
                    name: "main".to_string(),
                    is_default: true,
                }],
                schemas: Vec::new(),
                objects,
            }
        }
        DatabaseObjectType::Table | DatabaseObjectType::View => {
            let table_name = query
                .name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| {
                    ServerError::new(StatusCode::BAD_REQUEST, "Object name is required")
                })?;
            let table = describe_sqlite_table(&connection, table_name, object_type)?;
            DatabaseObjectDetails {
                database_name: query.database_name.or_else(|| Some("main".to_string())),
                schema_name: query.schema_name,
                name: table_name.to_string(),
                object_type,
                primary_key: table
                    .columns
                    .iter()
                    .filter(|column| column.is_primary_key)
                    .map(|column| column.name.clone())
                    .collect(),
                foreign_keys: table.foreign_keys,
                columns: table.columns,
                relational_schema: None,
                databases: Vec::new(),
                schemas: Vec::new(),
                objects: Vec::new(),
            }
        }
        _ => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Unsupported database object type",
            ));
        }
    };

    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "details": details,
    })))
}

async fn execute_query(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabaseQueryRequest>,
) -> Result<Response> {
    let sql = request.sql.trim();
    if sql.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "SQL is required",
                "code": "INVALID_SQL",
                "category": "validation",
                "retryable": false,
                "context": {
                    "databaseName": request.database_name,
                    "schemaName": request.schema_name,
                },
            })),
        )
            .into_response());
    }

    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let max_rows = request
        .max_rows
        .filter(|value| *value > 0)
        .map(|value| value.min(MAX_QUERY_MAX_ROWS))
        .unwrap_or(DEFAULT_QUERY_MAX_ROWS);
    let database_name = match connection.profile.db_type {
        SupportedDatabaseType::Sqlite => None,
        SupportedDatabaseType::Postgresql
        | SupportedDatabaseType::Mysql
        | SupportedDatabaseType::Mariadb => request
            .database_name
            .or_else(|| connection.profile.database_name.clone()),
    };
    let schema_name = if connection.profile.db_type == SupportedDatabaseType::Postgresql {
        request.schema_name
    } else {
        None
    };
    let execution = if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        execute_sqlite_query(&connection, sql, max_rows)
    } else {
        execute_live_query(
            &connection,
            sql,
            max_rows,
            database_name.as_deref(),
            schema_name.as_deref(),
        )
        .await
    };
    let mut result = match execution {
        Ok(result) => result,
        Err(error) => {
            return Ok(database_error_response(
                error,
                "Failed to execute database query",
                Some(serde_json::json!({
                    "databaseName": database_name,
                    "schemaName": schema_name,
                })),
            ));
        }
    };
    result.database_name = database_name;
    result.schema_name = schema_name;

    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "context": {
            "databaseName": result.database_name,
            "schemaName": result.schema_name,
        },
        "result": result,
    }))
    .into_response())
}

#[derive(Debug, Deserialize)]
struct TableDataQuery {
    #[serde(rename = "databaseName")]
    database_name: Option<String>,
    #[serde(rename = "schemaName")]
    schema_name: Option<String>,
    #[serde(rename = "tableName")]
    table_name: String,
    limit: Option<usize>,
    offset: Option<usize>,
    #[serde(rename = "includeTotalCount")]
    include_total_count: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct DatabaseFileJobRequest {
    #[serde(rename = "connectionId")]
    connection_id: i64,
    #[serde(rename = "databaseName")]
    database_name: Option<String>,
    #[serde(rename = "schemaName")]
    schema_name: Option<String>,
    #[serde(rename = "tableName")]
    table_name: String,
    #[serde(rename = "filePath")]
    file_path: String,
}

#[derive(Debug, Deserialize)]
struct DatabaseTableScopeRequest {
    #[serde(rename = "databaseName")]
    database_name: Option<String>,
    #[serde(rename = "schemaName")]
    schema_name: Option<String>,
    #[serde(rename = "tableName")]
    table_name: String,
}

#[derive(Debug, Deserialize)]
struct DatabaseRowEditRequest {
    #[serde(flatten)]
    scope: DatabaseTableScopeRequest,
    #[serde(rename = "primaryKeyValues")]
    primary_key_values: Map<String, Value>,
    #[serde(rename = "columnValues")]
    column_values: Map<String, Value>,
    #[serde(rename = "originalValues", default)]
    original_values: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct DatabaseRowAddRequest {
    #[serde(flatten)]
    scope: DatabaseTableScopeRequest,
    #[serde(rename = "columnValues")]
    column_values: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct DatabaseRowDeleteRequest {
    #[serde(flatten)]
    scope: DatabaseTableScopeRequest,
    #[serde(rename = "primaryKeyValues")]
    primary_key_values: Map<String, Value>,
    #[serde(rename = "originalValues", default)]
    original_values: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct DatabaseRowsDeleteRequest {
    #[serde(flatten)]
    scope: DatabaseTableScopeRequest,
    #[serde(rename = "primaryKeyRows", default)]
    primary_key_rows: Vec<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
struct DatabasePasteColumn {
    name: String,
}

#[derive(Debug, Deserialize)]
struct DatabaseClipboardPayload {
    #[serde(rename = "type")]
    clipboard_type: String,
    #[serde(rename = "createdAt")]
    _created_at: Option<String>,
    #[serde(rename = "sourceConnectionId")]
    _source_connection_id: Option<i64>,
    #[serde(rename = "sourceDatabaseName")]
    _source_database_name: Option<String>,
    #[serde(rename = "sourceSchemaName")]
    _source_schema_name: Option<String>,
    #[serde(rename = "sourceTableName")]
    _source_table_name: Option<String>,
    #[serde(default)]
    columns: Vec<DatabaseObjectColumn>,
    #[serde(default)]
    rows: Vec<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
struct DatabasePasteRequest {
    #[serde(flatten)]
    scope: DatabaseTableScopeRequest,
    #[serde(default)]
    clipboard: Option<DatabaseClipboardPayload>,
    #[serde(rename = "sourceColumns", default)]
    source_columns: Vec<DatabasePasteColumn>,
    #[serde(rename = "sourceRows", default)]
    source_rows: Vec<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
struct DatabasePortableExportRequest {
    #[serde(flatten)]
    scope: DatabaseTableScopeRequest,
    #[serde(rename = "includeData", default)]
    include_data: bool,
}

#[derive(Debug, Deserialize)]
struct DatabasePortableImportRequest {
    #[serde(rename = "databaseName")]
    database_name: Option<String>,
    #[serde(rename = "schemaName")]
    schema_name: Option<String>,
    #[serde(rename = "tableName")]
    table_name: Option<String>,
    payload: Value,
}

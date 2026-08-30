fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn load_connection(
    state: &AppState,
    user_id: &str,
    connection_id: i64,
) -> Result<StoredDatabaseConnection> {
    state
        .storage
        .get_database_connection(user_id, connection_id)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Connection not found"))
}

fn ensure_sqlite_connection(connection: &StoredDatabaseConnection) -> Result<PathBuf> {
    if connection.profile.db_type != SupportedDatabaseType::Sqlite {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "{} connections are supported for live browsing and queries; table transfer currently requires SQLite connections",
                connection.profile.db_type.as_str()
            ),
        ));
    }

    connection
        .profile
        .file_path
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "SQLite file path is required"))
}

async fn test_connection_input(input: &DatabaseConnectionInput) -> Result<DatabaseTestResult> {
    if input.db_type == SupportedDatabaseType::Sqlite {
        let path = input.file_path.as_ref().map(PathBuf::from).ok_or_else(|| {
            ServerError::new(StatusCode::BAD_REQUEST, "SQLite file path is required")
        })?;
        return test_sqlite_path(&path);
    }

    let profile = DatabaseConnectionProfile {
        id: 0,
        name: input.name.clone(),
        db_type: input.db_type,
        host: input.host.clone(),
        port: input.port,
        username: input.username.clone(),
        database_name: input.database_name.clone(),
        file_path: None,
        show_all_databases: input.show_all_databases,
        has_password: input
            .password
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        last_test_status: None,
        last_test_message: None,
        last_tested_at: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let connection = StoredDatabaseConnection {
        profile,
        password: input.password.clone(),
    };
    test_live_connection(&connection).await
}

async fn test_connection_record(
    connection: &StoredDatabaseConnection,
) -> Result<DatabaseTestResult> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let path = ensure_sqlite_connection(connection)?;
        return test_sqlite_path(&path);
    }
    test_live_connection(connection).await
}

fn test_sqlite_path(path: &PathBuf) -> Result<DatabaseTestResult> {
    let started_at = Instant::now();
    if !path.exists() {
        return Err(ServerError::database(
            StatusCode::BAD_REQUEST,
            format!("SQLite database file does not exist: {}", path.display()),
            None,
            "SQLITE_CANTOPEN",
            "connection",
            false,
        ));
    }
    if !path.is_file() {
        return Err(ServerError::database(
            StatusCode::BAD_REQUEST,
            format!("SQLite path is not a file: {}", path.display()),
            None,
            "SQLITE_CANTOPEN",
            "connection",
            false,
        ));
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(sqlite_server_error)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(sqlite_server_error)?;
    let version: String = conn
        .query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .map_err(sqlite_server_error)?;
    Ok(DatabaseTestResult {
        status: DatabaseTestStatus::Success,
        message: format!("SQLite connection successful ({version})"),
        duration_ms: Some(started_at.elapsed().as_millis()),
    })
}

fn database_session(profile: &DatabaseConnectionProfile) -> DatabaseSessionInfo {
    DatabaseSessionInfo {
        session_id: format!("{}:{}", profile.db_type.as_str(), profile.id),
        connection_id: profile.id,
        db_type: profile.db_type,
        capabilities: database_capabilities(profile.db_type),
    }
}

fn database_capabilities(db_type: SupportedDatabaseType) -> DatabaseCapabilities {
    match db_type {
        SupportedDatabaseType::Sqlite => sqlite_capabilities(),
        SupportedDatabaseType::Postgresql => DatabaseCapabilities {
            supports_databases: true,
            supports_schemas: true,
            supports_views: true,
            supports_indexes: true,
            supports_multiple_databases: true,
            supports_foreign_keys: true,
            supports_parameterized_queries: true,
            supports_offset: true,
            supported_object_types: vec![DatabaseObjectType::Table, DatabaseObjectType::View],
        },
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => DatabaseCapabilities {
            supports_databases: true,
            supports_schemas: false,
            supports_views: true,
            supports_indexes: true,
            supports_multiple_databases: true,
            supports_foreign_keys: true,
            supports_parameterized_queries: true,
            supports_offset: true,
            supported_object_types: vec![DatabaseObjectType::Table, DatabaseObjectType::View],
        },
    }
}

fn sqlite_capabilities() -> DatabaseCapabilities {
    DatabaseCapabilities {
        supports_databases: false,
        supports_schemas: false,
        supports_views: true,
        supports_indexes: true,
        supports_multiple_databases: false,
        supports_foreign_keys: true,
        supports_parameterized_queries: true,
        supports_offset: true,
        supported_object_types: vec![DatabaseObjectType::Table, DatabaseObjectType::View],
    }
}

async fn test_live_connection(connection: &StoredDatabaseConnection) -> Result<DatabaseTestResult> {
    let started_at = Instant::now();
    let pool = live_pool(connection).await?;
    let version_sql = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => "SELECT version() AS version",
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            "SELECT VERSION() AS version"
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let row = sqlx::query(version_sql)
        .fetch_one(&pool)
        .await
        .map_err(sqlx_server_error)?;
    let version = any_string(&row, 0).unwrap_or_else(|| "connected".to_string());
    Ok(DatabaseTestResult {
        status: DatabaseTestStatus::Success,
        message: format!(
            "{} connection successful ({version})",
            connection.profile.db_type.as_str()
        ),
        duration_ms: Some(started_at.elapsed().as_millis()),
    })
}

async fn explorer_live(
    connection: StoredDatabaseConnection,
    query: ExplorerQuery,
) -> Result<Json<Value>> {
    let node_type = query.node_type.as_deref().unwrap_or("connection");
    let session = database_session(&connection.profile);
    let capabilities = database_capabilities(connection.profile.db_type);
    let nodes = match node_type {
        "connection" => live_database_nodes(&connection).await?,
        "database" if capabilities.supports_schemas => {
            live_schema_nodes(&connection, query.database_name.as_deref()).await?
        }
        "database" | "schema" => {
            live_object_nodes(
                &connection,
                query.database_name.as_deref(),
                query.schema_name.as_deref(),
            )
            .await?
        }
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

async fn object_details_live(
    connection: StoredDatabaseConnection,
    query: ObjectDetailsQuery,
) -> Result<Json<Value>> {
    let object_type = parse_object_type(&query.object_type)?;
    let resolved_database_name = if object_type == DatabaseObjectType::Database {
        query.name.clone().or_else(|| query.database_name.clone())
    } else {
        query.database_name.clone()
    };
    let resolved_schema_name = if object_type == DatabaseObjectType::Schema {
        query.name.clone().or_else(|| query.schema_name.clone())
    } else {
        query.schema_name.clone()
    };
    let name = query
        .name
        .clone()
        .or_else(|| resolved_database_name.clone())
        .unwrap_or_else(|| default_database_name(&connection));
    let details = match object_type {
        DatabaseObjectType::Database | DatabaseObjectType::Schema => {
            let objects = live_objects(
                &connection,
                resolved_database_name.as_deref(),
                resolved_schema_name.as_deref(),
            )
            .await?;
            let relational_schema = if query.include_relational.unwrap_or(false) {
                Some(
                    live_relational_schema(
                        &connection,
                        object_type,
                        &name,
                        resolved_schema_name.as_deref(),
                        &objects,
                    )
                    .await?,
                )
            } else {
                None
            };
            DatabaseObjectDetails {
                database_name: resolved_database_name
                    .clone()
                    .or_else(|| Some(default_database_name(&connection))),
                schema_name: resolved_schema_name.clone(),
                name,
                object_type,
                columns: Vec::new(),
                primary_key: Vec::new(),
                foreign_keys: Vec::new(),
                relational_schema,
                databases: live_databases(&connection).await?,
                schemas: live_schemas(&connection, resolved_database_name.as_deref()).await?,
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
            let table = describe_live_table(
                &connection,
                resolved_database_name.as_deref(),
                resolved_schema_name.as_deref(),
                table_name,
                object_type,
            )
            .await?;
            DatabaseObjectDetails {
                database_name: resolved_database_name
                    .or_else(|| Some(default_database_name(&connection))),
                schema_name: resolved_schema_name,
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

struct LiveTableDescription {
    columns: Vec<DatabaseObjectColumn>,
    foreign_keys: Vec<DatabaseForeignKey>,
}

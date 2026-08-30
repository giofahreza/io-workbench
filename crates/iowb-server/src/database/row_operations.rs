struct PortableTablePayload {
    table_name: String,
    columns: Vec<DatabaseObjectColumn>,
    rows: Vec<Map<String, Value>>,
    include_data: bool,
}

fn validate_table_scope(scope: &DatabaseTableScopeRequest) -> Result<()> {
    if scope.table_name.trim().is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "tableName is required",
        ));
    }
    Ok(())
}

async fn database_table_metadata(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    table_name: &str,
) -> Result<DatabaseTableMetadata> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let description = describe_sqlite_table(connection, table_name, DatabaseObjectType::Table)?;
        let primary_key = description
            .columns
            .iter()
            .filter(|column| column.is_primary_key)
            .map(|column| column.name.clone())
            .collect();
        return Ok(DatabaseTableMetadata {
            columns: description.columns,
            primary_key,
        });
    }

    let description = describe_live_table(
        connection,
        database_name,
        schema_name,
        table_name,
        DatabaseObjectType::Table,
    )
    .await?;
    let primary_key = description
        .columns
        .iter()
        .filter(|column| column.is_primary_key)
        .map(|column| column.name.clone())
        .collect();
    Ok(DatabaseTableMetadata {
        columns: description.columns,
        primary_key,
    })
}

fn build_column_mappings(
    source_columns: &[String],
    target_columns: &[DatabaseObjectColumn],
) -> Vec<(String, String)> {
    target_columns
        .iter()
        .filter_map(|target| {
            source_columns
                .iter()
                .find(|source| *source == &target.name)
                .or_else(|| {
                    let target_key = normalize_database_column_key(&target.name);
                    source_columns
                        .iter()
                        .find(|source| normalize_database_column_key(source) == target_key)
                })
                .map(|source| (target.name.clone(), source.clone()))
        })
        .collect()
}

fn normalize_database_column_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn canonical_portable_column(column: &DatabaseObjectColumn) -> Value {
    serde_json::json!({
        "name": column.name,
        "typeFamily": portable_type_family(column),
        "dataType": column.data_type,
        "nativeType": column.native_type,
        "nullable": column.nullable.unwrap_or(true),
        "defaultValue": column.default_value,
        "isPrimaryKey": column.is_primary_key,
    })
}

fn portable_type_family(column: &DatabaseObjectColumn) -> &'static str {
    let data_type = column
        .native_type
        .as_deref()
        .or(column.data_type.as_deref())
        .unwrap_or("")
        .to_ascii_lowercase();
    if data_type.contains("bool") {
        "boolean"
    } else if data_type.contains("bigint") {
        "bigint"
    } else if data_type.contains("int") {
        "integer"
    } else if data_type.contains("decimal") || data_type.contains("numeric") {
        "numeric"
    } else if data_type.contains("real")
        || data_type.contains("float")
        || data_type.contains("double")
    {
        "float"
    } else if data_type.contains("json") {
        "json"
    } else if data_type.contains("uuid") {
        "uuid"
    } else if data_type.contains("blob")
        || data_type.contains("binary")
        || data_type.contains("bytea")
    {
        "binary"
    } else if data_type.contains("timestamp") || data_type.contains("datetime") {
        "datetime"
    } else if data_type == "date" {
        "date"
    } else if data_type == "time" {
        "time"
    } else {
        "text"
    }
}

fn parse_portable_table_payload(payload: &Value) -> Result<PortableTablePayload> {
    let object = payload.as_object().ok_or_else(|| {
        ServerError::new(StatusCode::BAD_REQUEST, "A portable payload is required")
    })?;
    if object.get("format").and_then(Value::as_str) != Some("web-ai-cli/database-portable-v1") {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Unsupported portable table payload",
        ));
    }
    let payload_type = object.get("type").and_then(Value::as_str).unwrap_or("");
    let include_data = match payload_type {
        "table-schema" => false,
        "table-schema-and-data" => true,
        _ => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Unsupported portable table payload type",
            ));
        }
    };
    let table = object
        .get("table")
        .and_then(Value::as_object)
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Portable table is missing"))?;
    let table_name = table
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let columns = table
        .get("columns")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "Portable table columns are missing",
            )
        })?
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let name = entry.get("name")?.as_str()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let type_family = entry
                .get("typeFamily")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let data_type = entry
                .get("dataType")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    type_family
                        .map(portable_type_family_name)
                        .map(str::to_string)
                });
            Some(DatabaseObjectColumn {
                name,
                data_type,
                native_type: entry
                    .get("nativeType")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                nullable: entry.get("nullable").and_then(Value::as_bool),
                default_value: entry
                    .get("defaultValue")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                extra: None,
                is_primary_key: entry
                    .get("isPrimaryKey")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect::<Vec<_>>();
    if columns.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Portable table has no columns",
        ));
    }
    let rows = if include_data {
        object
            .get("rows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| row.as_object().cloned())
            .collect()
    } else {
        Vec::new()
    };
    Ok(PortableTablePayload {
        table_name,
        columns,
        rows,
        include_data,
    })
}

fn portable_type_family_name(type_family: &str) -> &'static str {
    match type_family {
        "boolean" => "boolean",
        "bigint" => "bigint",
        "integer" => "integer",
        "numeric" => "numeric",
        "float" => "double",
        "json" => "json",
        "uuid" => "uuid",
        "binary" => "binary",
        "datetime" => "datetime",
        "date" => "date",
        "time" => "time",
        _ => "text",
    }
}

async fn update_database_row(
    connection: &StoredDatabaseConnection,
    scope: &DatabaseTableScopeRequest,
    primary_key_values: &Map<String, Value>,
    column_values: &Map<String, Value>,
    original_values: &Map<String, Value>,
) -> Result<Option<Map<String, Value>>> {
    let metadata = database_table_metadata(
        connection,
        scope.database_name.as_deref(),
        scope.schema_name.as_deref(),
        &scope.table_name,
    )
    .await?;
    if metadata.primary_key.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Table has no primary key. Row editing is not supported.",
        ));
    }
    let primary_keys = canonical_primary_key_values(&metadata, primary_key_values)?;
    let current = select_database_row(connection, scope, &primary_keys)
        .await?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Row not found"))?;
    ensure_row_is_current(&current, original_values)?;
    let updates = canonical_column_values(&metadata, column_values, false)?;
    if updates.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No non-primary-key columns provided for update",
        ));
    }
    let endpoint = database_scope_endpoint(connection, scope);
    let set_sql = updates
        .iter()
        .map(|(column, value)| {
            format!(
                "{} = {}",
                quote_transfer_identifier(connection.profile.db_type, column),
                transfer_value_literal(connection.profile.db_type, value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let where_sql = build_database_where_clause(connection.profile.db_type, &primary_keys);
    let sql = format!(
        "UPDATE {} SET {set_sql} WHERE {where_sql}",
        transfer_table_ref(connection.profile.db_type, &endpoint)
    );
    execute_database_mutation(connection, scope.database_name.as_deref(), &sql).await?;
    select_database_row(connection, scope, &primary_keys).await
}

async fn insert_database_row(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    table_name: &str,
    metadata: &DatabaseTableMetadata,
    column_values: &Map<String, Value>,
) -> Result<Option<Map<String, Value>>> {
    let mut values = canonical_column_values(metadata, column_values, true)?;
    values.retain(|(name, value)| {
        let column = metadata.columns.iter().find(|column| column.name == *name);
        !(value.is_null()
            && column.is_some_and(|column| {
                column.default_value.is_some()
                    || column
                        .extra
                        .as_deref()
                        .is_some_and(|extra| extra.to_ascii_lowercase().contains("auto_increment"))
            }))
    });
    if values.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No valid column values provided for insert",
        ));
    }
    let scope = DatabaseTableScopeRequest {
        database_name: database_name.map(str::to_string),
        schema_name: schema_name.map(str::to_string),
        table_name: table_name.to_string(),
    };
    let endpoint = database_scope_endpoint(connection, &scope);
    let columns = values
        .iter()
        .map(|(name, _)| quote_transfer_identifier(connection.profile.db_type, name))
        .collect::<Vec<_>>()
        .join(", ");
    let sql_values = values
        .iter()
        .map(|(_, value)| transfer_value_literal(connection.profile.db_type, value))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO {} ({columns}) VALUES ({sql_values})",
        transfer_table_ref(connection.profile.db_type, &endpoint)
    );
    execute_database_mutation(connection, database_name, &sql).await?;

    let provided = values.into_iter().collect::<Map<_, _>>();
    let primary_keys = metadata
        .primary_key
        .iter()
        .filter_map(|column| {
            find_json_map_value(&provided, column).map(|value| (column.clone(), value.clone()))
        })
        .collect::<Map<_, _>>();
    if !metadata.primary_key.is_empty() && primary_keys.len() == metadata.primary_key.len() {
        return select_database_row(connection, &scope, &primary_keys).await;
    }
    Ok(None)
}

async fn delete_database_row(
    connection: &StoredDatabaseConnection,
    scope: &DatabaseTableScopeRequest,
    primary_key_values: &Map<String, Value>,
    original_values: &Map<String, Value>,
) -> Result<()> {
    let metadata = database_table_metadata(
        connection,
        scope.database_name.as_deref(),
        scope.schema_name.as_deref(),
        &scope.table_name,
    )
    .await?;
    if metadata.primary_key.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Table has no primary key. Row deletion is not supported.",
        ));
    }
    let primary_keys = canonical_primary_key_values(&metadata, primary_key_values)?;
    let current = select_database_row(connection, scope, &primary_keys)
        .await?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Row not found"))?;
    ensure_row_is_current(&current, original_values)?;
    let endpoint = database_scope_endpoint(connection, scope);
    let sql = format!(
        "DELETE FROM {} WHERE {}",
        transfer_table_ref(connection.profile.db_type, &endpoint),
        build_database_where_clause(connection.profile.db_type, &primary_keys),
    );
    execute_database_mutation(connection, scope.database_name.as_deref(), &sql).await?;
    Ok(())
}

fn canonical_primary_key_values(
    metadata: &DatabaseTableMetadata,
    values: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    metadata
        .primary_key
        .iter()
        .map(|column| {
            find_json_map_value(values, column)
                .cloned()
                .map(|value| (column.clone(), value))
                .ok_or_else(|| {
                    ServerError::new(
                        StatusCode::BAD_REQUEST,
                        format!("Primary key value for \"{column}\" is missing"),
                    )
                })
        })
        .collect()
}

fn canonical_column_values(
    metadata: &DatabaseTableMetadata,
    values: &Map<String, Value>,
    include_primary_keys: bool,
) -> Result<Vec<(String, Value)>> {
    let mut output: Vec<(String, Value)> = Vec::new();
    for (requested_name, value) in values {
        let Some(column) = metadata
            .columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(requested_name))
        else {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                format!("Unknown table column: {requested_name}"),
            ));
        };
        if !include_primary_keys && column.is_primary_key {
            continue;
        }
        if output
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(&column.name))
        {
            continue;
        }
        output.push((column.name.clone(), value.clone()));
    }
    Ok(output)
}

fn find_json_map_value<'a>(values: &'a Map<String, Value>, column: &str) -> Option<&'a Value> {
    values.get(column).or_else(|| {
        values
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(column))
            .map(|(_, value)| value)
    })
}

fn ensure_row_is_current(
    current: &Map<String, Value>,
    original_values: &Map<String, Value>,
) -> Result<()> {
    if original_values.is_empty() {
        return Ok(());
    }
    let changed = original_values.iter().any(|(column, original)| {
        find_json_map_value(current, column).is_none_or(|value| value != original)
    });
    if changed {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The row changed after it was loaded. Refresh before saving or deleting it.",
        ));
    }
    Ok(())
}

fn database_scope_endpoint(
    connection: &StoredDatabaseConnection,
    scope: &DatabaseTableScopeRequest,
) -> DatabaseTransferEndpoint {
    DatabaseTransferEndpoint {
        connection_id: connection.profile.id,
        connection_name: Some(connection.profile.name.clone()),
        connection_type: Some(connection.profile.db_type),
        database_name: scope
            .database_name
            .clone()
            .or_else(|| Some(default_database_name(connection))),
        schema_name: scope.schema_name.clone(),
        table_name: scope.table_name.clone(),
    }
}

fn build_database_where_clause(
    db_type: SupportedDatabaseType,
    values: &Map<String, Value>,
) -> String {
    values
        .iter()
        .map(|(column, value)| {
            let column = quote_transfer_identifier(db_type, column);
            if value.is_null() {
                format!("{column} IS NULL")
            } else {
                format!("{column} = {}", transfer_value_literal(db_type, value))
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

async fn select_database_row(
    connection: &StoredDatabaseConnection,
    scope: &DatabaseTableScopeRequest,
    primary_keys: &Map<String, Value>,
) -> Result<Option<Map<String, Value>>> {
    let endpoint = database_scope_endpoint(connection, scope);
    let sql = format!(
        "SELECT * FROM {} WHERE {} LIMIT 1",
        transfer_table_ref(connection.profile.db_type, &endpoint),
        build_database_where_clause(connection.profile.db_type, primary_keys),
    );
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_connection(connection)?;
        let mut statement = conn.prepare(&sql).map_err(sqlite_server_error)?;
        let column_names = statement
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut rows = statement.query([]).map_err(sqlite_server_error)?;
        return rows
            .next()
            .map_err(sqlite_server_error)?
            .map(|row| row_to_json_map(row, &column_names))
            .transpose();
    }
    Ok(
        fetch_live_row_maps(connection, scope.database_name.as_deref(), &sql)
            .await?
            .into_iter()
            .next(),
    )
}

async fn execute_database_mutation(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    sql: &str,
) -> Result<u64> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_connection(connection)?;
        let affected = conn.execute(sql, []).map_err(sqlite_server_error)?;
        return Ok(affected as u64);
    }
    let pool = live_pool_for_database(connection, database_name).await?;
    let result = pool.execute(sql).await.map_err(sqlx_server_error)?;
    Ok(result.rows_affected())
}

async fn normalize_connection_input(
    state: &AppState,
    mut input: DatabaseConnectionInput,
    existing_password: Option<String>,
) -> Result<DatabaseConnectionInput> {
    input.name = input.name.trim().to_string();
    if input.name.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Connection name is required",
        ));
    }

    if input.password.as_deref().is_none_or(str::is_empty) {
        input.password = existing_password;
    }

    match input.db_type {
        SupportedDatabaseType::Sqlite => {
            let file_path = input
                .file_path
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ServerError::new(StatusCode::BAD_REQUEST, "SQLite file path is required")
                })?;
            let resolved = state
                .path_validator
                .validate_path(PathBuf::from(file_path), false)
                .await?;
            input.host = None;
            input.port = None;
            input.username = None;
            input.password = None;
            input.database_name = None;
            input.file_path = Some(resolved.display().to_string());
            input.show_all_databases = false;
        }
        SupportedDatabaseType::Postgresql
        | SupportedDatabaseType::Mysql
        | SupportedDatabaseType::Mariadb => {
            input.host = trim_optional(input.host);
            input.username = trim_optional(input.username);
            input.database_name = trim_optional(input.database_name);
            if input.host.is_none() {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Host is required",
                ));
            }
            if input.username.is_none() {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Username is required",
                ));
            }
            if input.port.is_none() {
                input.port = Some(if input.db_type == SupportedDatabaseType::Postgresql {
                    5432
                } else {
                    3306
                });
            }
            input.file_path = None;
            input.show_all_databases =
                input.db_type == SupportedDatabaseType::Postgresql && input.show_all_databases;
        }
    }

    Ok(input)
}

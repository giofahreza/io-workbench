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

async fn ensure_postgres_mutation_privilege_when_primary_key_hidden(
    connection: &StoredDatabaseConnection,
    scope: &DatabaseTableScopeRequest,
    update: bool,
) -> Result<()> {
    if connection.profile.db_type != SupportedDatabaseType::Postgresql {
        return Ok(());
    }
    // PostgreSQL's information_schema constraint views can hide the real PK
    // after UPDATE/DELETE grants are revoked. Keep normal metadata/read-only UI
    // semantics, but do not misdiagnose denied writes as a table without a PK.
    let (database, schema) = live_scope(
        connection,
        scope.database_name.as_deref(),
        scope.schema_name.as_deref(),
    );
    let TypedLivePool::Postgresql(pool) =
        typed_live_pool_for_database(connection, Some(&database)).await?
    else {
        unreachable!("PostgreSQL connection requires PostgreSQL pool")
    };
    let permitted: Option<bool> = sqlx::query_scalar(
        "SELECT CASE WHEN $3 THEN \
           has_table_privilege(relation.oid, 'UPDATE') OR \
           has_any_column_privilege(relation.oid, 'UPDATE') \
         ELSE has_table_privilege(relation.oid, 'DELETE') END \
         FROM pg_catalog.pg_class relation \
         JOIN pg_catalog.pg_namespace namespace ON namespace.oid=relation.relnamespace \
         WHERE namespace.nspname=$1 AND relation.relname=$2",
    )
    .bind(schema)
    .bind(&scope.table_name)
    .bind(update)
    .fetch_optional(&pool)
    .await
    .map_err(sqlx_server_error)?;
    let permitted = permitted
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Table not found"))?;
    if !permitted {
        return Err(ServerError::database(
            StatusCode::FORBIDDEN,
            if update {
                "Permission denied: UPDATE privileges are required to edit rows."
            } else {
                "Permission denied: DELETE privileges are required to delete rows."
            },
            None,
            "42501",
            "permission",
            false,
        ));
    }
    Ok(())
}

fn build_column_mappings(
    source_columns: &[String],
    target_columns: &[DatabaseObjectColumn],
) -> Result<Vec<(String, String)>> {
    let mut mappings = Vec::new();
    for target in target_columns
        .iter()
        // Generated, identity, and auto-increment columns are selected by the
        // database.  Never map imported values into them: the row operation
        // path uses the same metadata guard, and this keeps CSV/transfer
        // imports from failing just because their source includes a derived
        // value.
        .filter(|target| !database_column_is_server_managed(target))
    {
        let exact = source_columns.iter().find(|source| *source == &target.name);
        let key = normalize_database_column_key(&target.name);
        let candidates = source_columns.iter().filter(|source| {
            !key.is_empty() && normalize_database_column_key(source) == key
                // An exact source/target pair owns that source regardless of
                // target order; do not reuse it for a case/normalized alias.
                && !target_columns.iter().any(|other| other.name == **source)
        }).collect::<Vec<_>>();
        let source = if let Some(exact) = exact {
            Some(exact)
        } else if candidates.len() > 1 {
            return Err(ServerError::new(StatusCode::BAD_REQUEST,
                format!("Ambiguous source columns match target column {}", target.name)));
        } else {
            candidates.first().copied()
        };
        if let Some(source) = source {
            if mappings.iter().any(|(_, used)| used == source) {
                return Err(ServerError::new(StatusCode::BAD_REQUEST,
                    format!("Source column {source} matches multiple target columns")));
            }
            mappings.push((target.name.clone(), source.clone()));
        }
    }
    Ok(mappings)
}

fn database_column_is_generated(column: &DatabaseObjectColumn) -> bool {
    column
        .extra
        .as_deref()
        .is_some_and(|extra| extra.to_ascii_lowercase().contains("generated"))
}

fn database_column_is_server_managed(column: &DatabaseObjectColumn) -> bool {
    column.extra.as_deref().is_some_and(|extra| {
        let extra = extra.to_ascii_lowercase();
        extra.contains("generated")
            || extra.contains("identity")
            || extra.contains("auto_increment")
            || extra.contains("auto-increment")
            || extra.contains("auto increment")
    })
}

fn normalize_database_column_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
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
    if table_name.is_empty() {
        return Err(ServerError::new(StatusCode::BAD_REQUEST, "Portable table name is missing"));
    }
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
        .map(|entry| {
            let malformed = || ServerError::new(StatusCode::BAD_REQUEST, "Malformed portable column entry");
            let entry = entry.as_object().ok_or_else(malformed)?;
            let name = entry.get("name").and_then(Value::as_str).ok_or_else(malformed)?.trim().to_string();
            if name.is_empty() {
                return Err(malformed());
            }
            for key in ["nullable", "isPrimaryKey"] {
                if entry.get(key).is_some_and(|value| !value.is_boolean()) {
                    return Err(malformed());
                }
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
            Ok(DatabaseObjectColumn {
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
        .collect::<Result<Vec<_>>>()?;
    if columns.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Portable table has no columns",
        ));
    }
    for (index, column) in columns.iter().enumerate() {
        if columns[..index].iter().any(|other| other.name == column.name) {
            return Err(ServerError::new(StatusCode::BAD_REQUEST, "Duplicate portable column name"));
        }
    }
    if let Some(primary_key) = table.get("primaryKey") {
        let names = primary_key.as_array().ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Malformed portable primary key"))?;
        let declared = names.iter().map(|name| name.as_str()).collect::<Option<Vec<_>>>()
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Malformed portable primary key"))?;
        let actual = columns.iter().filter(|column| column.is_primary_key).map(|column| column.name.as_str()).collect::<Vec<_>>();
        if declared.len() != actual.len()
            || declared.iter().any(|name| !actual.contains(name))
            || declared.iter().enumerate().any(|(index, name)| declared[..index].contains(name))
        {
            return Err(ServerError::new(StatusCode::BAD_REQUEST, "Conflicting portable primary-key metadata"));
        }
    }
    let rows = if include_data {
        object.get("rows").and_then(Value::as_array)
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Portable data rows are missing"))?
            .iter().map(|row| row.as_object().cloned().ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Malformed portable row entry")))
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    for row in &rows {
        for column in &columns {
            if let Some(value) = row.get(&column.name) {
                validate_database_binary_value(column, value)?;
            }
        }
    }
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
        ensure_postgres_mutation_privilege_when_primary_key_hidden(connection, scope, true).await?;
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Table has no primary key. Row editing is not supported.",
        ));
    }
    let primary_keys = canonical_primary_key_values(&metadata, primary_key_values)?;
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
                database_row_value_literal(connection.profile.db_type, &metadata, column, value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let where_sql = build_database_where_clause(connection.profile.db_type, &primary_keys);
    let sql = format!(
        "UPDATE {} SET {set_sql} WHERE {where_sql}",
        transfer_table_ref(connection.profile.db_type, &endpoint)
    );
    mutate_database_row_atomically(connection, scope, &primary_keys, original_values, &sql, true).await
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
            && column.is_some_and(database_column_is_server_managed))
    });
    let scope = DatabaseTableScopeRequest {
        database_name: database_name.map(str::to_string),
        schema_name: schema_name.map(str::to_string),
        table_name: table_name.to_string(),
    };
    let endpoint = database_scope_endpoint(connection, &scope);
    let table_ref = transfer_table_ref(connection.profile.db_type, &endpoint);
    let sql = if values.is_empty() {
        default_row_insert_sql(connection.profile.db_type, &table_ref)
    } else {
        let columns = values
            .iter()
            .map(|(name, _)| quote_transfer_identifier(connection.profile.db_type, name))
            .collect::<Vec<_>>()
            .join(", ");
        let sql_values = values
            .iter()
            .map(|(column, value)| {
                database_row_value_literal(connection.profile.db_type, metadata, column, value)
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("INSERT INTO {table_ref} ({columns}) VALUES ({sql_values})")
    };
    let affected = execute_database_mutation(connection, database_name, &sql).await?;
    if affected == 0 {
        return Err(database_insert_suppressed());
    }

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

fn database_insert_suppressed() -> ServerError {
    ServerError::new(
        StatusCode::CONFLICT,
        "The database did not insert this row (a trigger or rule may have suppressed it)",
    )
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
        ensure_postgres_mutation_privilege_when_primary_key_hidden(connection, scope, false).await?;
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Table has no primary key. Row deletion is not supported.",
        ));
    }
    let primary_keys = canonical_primary_key_values(&metadata, primary_key_values)?;
    let endpoint = database_scope_endpoint(connection, scope);
    let sql = format!(
        "DELETE FROM {} WHERE {}",
        transfer_table_ref(connection.profile.db_type, &endpoint),
        build_database_where_clause(connection.profile.db_type, &primary_keys),
    );
    mutate_database_row_atomically(connection, scope, &primary_keys, original_values, &sql, false).await?;
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

fn database_column_is_bytea_array(column: &DatabaseObjectColumn) -> bool {
    let kind = column.native_type.as_deref().or(column.data_type.as_deref())
        .unwrap_or("").trim().to_ascii_lowercase();
    kind == "_bytea" || (kind.ends_with("[]") && kind.trim_end_matches("[]") == "bytea")
}

fn validate_database_binary_value(column: &DatabaseObjectColumn, value: &Value) -> Result<()> {
    if database_column_is_bytea_array(column) {
        if let Value::Array(values) = value {
            for element in values {
                validate_database_binary_value(column, element)?;
            }
            return Ok(());
        }
        if !value.is_null() && !value.is_string() && !value.is_object() {
            return Err(ServerError::new(StatusCode::BAD_REQUEST,
                format!("Column \"{}\" has an invalid binary array element", column.name)));
        }
    }
    let family = portable_type_family(column);
    if (family == "binary" && value.is_object())
        || (family != "json" && database_buffer_base64(value).is_some())
    {
        let encoded = database_buffer_base64(value).ok_or_else(|| ServerError::new(
            StatusCode::BAD_REQUEST, format!("Column \"{}\" has an invalid binary buffer marker", column.name),
        ))?;
        BASE64_STANDARD.decode(encoded).map_err(|_| ServerError::new(
            StatusCode::BAD_REQUEST, format!("Column \"{}\" has invalid base64 binary data", column.name),
        ))?;
    }
    Ok(())
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
            .find(|column| column.name == *requested_name)
            .or_else(|| metadata.columns.iter().find(|column| column.name.eq_ignore_ascii_case(requested_name)))
        else {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                format!("Unknown table column: {requested_name}"),
            ));
        };
        validate_database_binary_value(column, value)?;
        if include_primary_keys && database_column_is_generated(column) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                format!("Column \"{}\" is generated by the database", column.name),
            ));
        }
        if !include_primary_keys && database_column_is_server_managed(column) {
            continue;
        }
        if !include_primary_keys && column.is_primary_key {
            continue;
        }
        if output
            .iter()
            .any(|(name, _)| name == &column.name)
        {
            continue;
        }
        output.push((column.name.clone(), value.clone()));
    }
    Ok(output)
}

// JSON arrays returned by PostgreSQL must be encoded using PostgreSQL's array
// input grammar, not JSON syntax. Keep JSON/JSONB columns on their usual path.
fn database_column_is_json(metadata: &DatabaseTableMetadata, column_name: &str) -> bool {
    metadata.columns.iter().any(|column| {
        column.name == column_name
            && column.native_type.as_deref().or(column.data_type.as_deref())
                .is_some_and(|data_type| matches!(data_type.to_ascii_lowercase().as_str(), "json" | "jsonb"))
    })
}

fn database_paste_value(
    metadata: &DatabaseTableMetadata,
    column_name: &str,
    value: Value,
    format: &DatabaseRowValueFormat,
) -> Value {
    if *format == DatabaseRowValueFormat::NativeJson
        && !value.is_null()
        && database_column_is_json(metadata, column_name)
    {
        Value::String(value.to_string())
    } else {
        value
    }
}

fn database_row_value_literal(
    db_type: SupportedDatabaseType,
    metadata: &DatabaseTableMetadata,
    column_name: &str,
    value: &Value,
) -> String {
    if database_column_is_json(metadata, column_name) && !value.is_null() {
        // Editors/CSV may supply already-encoded JSON text, whereas row
        // snapshots supply native JSON scalars. Serialize non-string scalars
        // and plain strings, while retaining the existing encoded-text path.
        let encoded = match value {
            Value::String(text) if serde_json::from_str::<Value>(text).is_ok() => text.clone(),
            _ => value.to_string(),
        };
        return database_text_literal(db_type, &encoded);
    }
    if db_type == SupportedDatabaseType::Postgresql
        && metadata.columns.iter().any(|column| {
            column.name == column_name
                && column.native_type.as_deref().or(column.data_type.as_deref())
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("point"))
        })
    {
        if let Some(point) = value.as_object() {
            if let (Some(Value::Number(x)), Some(Value::Number(y))) = (point.get("x"), point.get("y")) {
                return database_text_literal(db_type, &format!("({x},{y})"));
            }
        }
    }
    let is_postgres_array = db_type == SupportedDatabaseType::Postgresql
        && metadata.columns.iter().any(|column| {
            column.name == column_name
                && column.data_type.as_deref().is_some_and(|data_type| {
                    data_type.eq_ignore_ascii_case("array") || data_type.ends_with("[]")
                })
        });
    if is_postgres_array && value.is_array() {
        if metadata.columns.iter().any(|column| column.name == column_name && database_column_is_bytea_array(column)) {
            postgres_binary_array_literal(value)
        } else {
            database_text_literal(db_type, &postgres_array_input(value))
        }
    } else {
        transfer_value_literal(db_type, value)
    }
}

fn postgres_binary_array_literal(value: &Value) -> String {
    match value {
        Value::Array(values) => format!(
            "ARRAY[{}]::bytea[]",
            values.iter().map(postgres_binary_array_literal).collect::<Vec<_>>().join(", "),
        ),
        // Buffers have already been validated recursively. Reuse the scalar
        // byte decoder rather than storing each marker's JSON as bytea text.
        _ => format!("{}::bytea", transfer_value_literal(SupportedDatabaseType::Postgresql, value)),
    }
}

fn postgres_array_input(value: &Value) -> String {
    match value {
        Value::Array(values) => format!(
            "{{{}}}",
            values.iter().map(postgres_array_input).collect::<Vec<_>>().join(",")
        ),
        Value::Null => "NULL".to_string(),
        _ => {
            let text = match value {
                Value::String(text) => text.clone(),
                _ => value.to_string(),
            };
            format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
        }
    }
}

fn default_row_insert_sql(db_type: SupportedDatabaseType, table_ref: &str) -> String {
    match db_type {
        SupportedDatabaseType::Postgresql | SupportedDatabaseType::Sqlite => {
            format!("INSERT INTO {table_ref} DEFAULT VALUES")
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            format!("INSERT INTO {table_ref} () VALUES ()")
        }
    }
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
    validate_database_mutation_text(connection.profile.db_type, sql)?;
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_connection(connection)?;
        let affected = conn.execute(sql, []).map_err(sqlite_server_error)?;
        return Ok(affected as u64);
    }
    let pool = live_pool_for_database(connection, database_name).await?;
    let result = pool.execute(sql).await.map_err(sqlx_server_error)?;
    Ok(result.rows_affected())
}

fn validate_database_mutation_text(db_type: SupportedDatabaseType, sql: &str) -> Result<()> {
    // PostgreSQL's text protocol cannot carry an embedded zero byte. Reject it
    // before sending the statement, rather than exposing an opaque protocol
    // error. Binary values are encoded literals and do not contain raw NULs.
    if db_type == SupportedDatabaseType::Postgresql && sql.contains('\0') {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "PostgreSQL text values cannot contain U+0000 (NUL). Use a binary column to store zero bytes.",
        ));
    }
    Ok(())
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

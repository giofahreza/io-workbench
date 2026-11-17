async fn execute_live_query(
    connection: &StoredDatabaseConnection,
    sql: &str,
    max_rows: usize,
    database_name: Option<&str>,
    schema_name: Option<&str>,
) -> Result<DatabaseQueryResult> {
    match typed_live_pool_for_database(connection, database_name).await? {
        TypedLivePool::Postgresql(pool) => {
            execute_postgres_query(&pool, sql, max_rows, schema_name).await
        }
        TypedLivePool::Mysql(pool) => execute_mysql_query(&pool, sql, max_rows).await,
    }
}

async fn execute_postgres_query(
    pool: &PgPool,
    sql: &str,
    max_rows: usize,
    schema_name: Option<&str>,
) -> Result<DatabaseQueryResult> {
    let start = Instant::now();
    let mut database = pool.acquire().await.map_err(sqlx_server_error)?;
    if let Some(schema_name) = schema_name.map(str::trim).filter(|value| !value.is_empty()) {
        let scope_sql = format!(
            "SET search_path TO {}, public",
            quote_identifier(schema_name)
        );
        sqlx::query(&scope_sql)
            .execute(&mut *database)
            .await
            .map_err(sqlx_server_error)?;
    }

    let statement_type = classify_statement(sql);
    let described_columns = (&mut *database)
        .describe(sql)
        .await
        .map(|description| description.columns().to_vec())
        .unwrap_or_default();
    let mut rows = Vec::new();
    let mut observed_rows = 0usize;
    let mut result_truncated = false;
    let mut affected_rows = 0u64;
    let mut stream = sqlx::raw_sql(sql).fetch_many(&mut *database);
    while let Some(step) = stream.next().await {
        match step.map_err(sqlx_server_error)? {
            Either::Left(result) => {
                affected_rows = affected_rows.saturating_add(result.rows_affected());
            }
            Either::Right(row) => {
                observed_rows = observed_rows.saturating_add(1);
                if rows.len() < max_rows {
                    rows.push(row);
                } else {
                    result_truncated = true;
                    break;
                }
            }
        }
    }
    drop(stream);

    let columns = rows
        .first()
        .map(|row| {
            // Raw/simple-query rows may retain only a custom type's OID. Use
            // described names only when ordinal, name and OID identify the
            // same result column; never borrow metadata from a different set.
            let resolved = row.columns().iter().enumerate().map(|(index, column)| {
                described_columns.get(index).filter(|known| {
                    known.name() == column.name()
                        && column.type_info().oid().is_some()
                        && known.type_info().oid() == column.type_info().oid()
                }).unwrap_or(column).clone()
            }).collect::<Vec<_>>();
            postgres_columns(&resolved)
        })
        .filter(|columns| !columns.is_empty())
        .unwrap_or_else(|| postgres_columns(&described_columns));
    let row_count = if columns.is_empty() {
        usize::try_from(affected_rows).unwrap_or(usize::MAX)
    } else {
        observed_rows
    };
    let output = rows
        .into_iter()
        .map(|row| postgres_row_to_json_map(&row))
        .collect::<Vec<_>>();
    let returned_row_count = output.len();
    Ok(DatabaseQueryResult {
        sql: sql.to_string(),
        statement_type,
        row_count,
        returned_row_count,
        result_truncated,
        max_rows,
        rows: output,
        columns,
        notices: Vec::new(),
        duration_ms: start.elapsed().as_millis(),
        meta: Some(query_result_meta(
            returned_row_count,
            result_truncated,
            max_rows,
            !result_truncated,
            None,
        )),
        database_name: None,
        schema_name: None,
    })
}

async fn execute_mysql_query(
    pool: &MySqlPool,
    sql: &str,
    max_rows: usize,
) -> Result<DatabaseQueryResult> {
    let start = Instant::now();
    let mut database = pool.acquire().await.map_err(sqlx_server_error)?;
    let statement_type = classify_statement(sql);
    let described_columns = (&mut *database)
        .describe(sql)
        .await
        .map(|description| mysql_columns(description.columns()))
        .unwrap_or_default();
    let mut rows = Vec::new();
    let mut observed_rows = 0usize;
    let mut result_truncated = false;
    let mut affected_rows = 0u64;
    let mut last_insert_id = 0u64;
    let mut stream = sqlx::raw_sql(sql).fetch_many(&mut *database);
    while let Some(step) = stream.next().await {
        match step.map_err(sqlx_server_error)? {
            Either::Left(result) => {
                affected_rows = affected_rows.saturating_add(result.rows_affected());
                last_insert_id = result.last_insert_id();
            }
            Either::Right(row) => {
                observed_rows = observed_rows.saturating_add(1);
                if rows.len() < max_rows {
                    rows.push(row);
                } else {
                    result_truncated = true;
                    break;
                }
            }
        }
    }
    drop(stream);

    let columns = rows
        .first()
        .map(|row| mysql_columns(row.columns()))
        .filter(|columns| !columns.is_empty())
        .unwrap_or(described_columns);
    let has_row_set = !columns.is_empty();
    let row_count = if has_row_set {
        observed_rows
    } else {
        usize::try_from(affected_rows).unwrap_or(usize::MAX)
    };
    let output = rows
        .into_iter()
        .map(|row| mysql_row_to_json_map(&row))
        .collect::<Vec<_>>();
    let returned_row_count = output.len();
    let warning_status = if has_row_set {
        None
    } else {
        let mut status = None;
        let mut warning_stream = sqlx::raw_sql("SHOW COUNT(*) WARNINGS").fetch_many(&mut *database);
        while let Some(step) = warning_stream.next().await {
            match step {
                Ok(Either::Left(_)) => {}
                Ok(Either::Right(row)) => {
                    let value = mysql_value_to_json(&row, 0);
                    status = value
                        .as_u64()
                        .or_else(|| value.as_str()?.parse::<u64>().ok());
                    break;
                }
                Err(_) => break,
            }
        }
        drop(warning_stream);
        status
    };
    let extra_meta = (!has_row_set).then(|| {
        serde_json::json!({
            "affectedRows": affected_rows,
            "insertId": last_insert_id,
            "warningStatus": warning_status,
        })
    });
    Ok(DatabaseQueryResult {
        sql: sql.to_string(),
        statement_type,
        row_count,
        returned_row_count,
        result_truncated,
        max_rows,
        rows: output,
        columns,
        notices: Vec::new(),
        duration_ms: start.elapsed().as_millis(),
        meta: Some(query_result_meta(
            returned_row_count,
            result_truncated,
            max_rows,
            !result_truncated,
            extra_meta,
        )),
        database_name: None,
        schema_name: None,
    })
}

async fn read_live_table_data(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    table_name: &str,
    limit: usize,
    offset: usize,
    include_total_count: bool,
) -> Result<DatabaseTableData> {
    read_live_table_data_with_options(
        connection,
        database_name,
        schema_name,
        table_name,
        limit,
        offset,
        include_total_count,
        &DatabaseTableQueryOptions::default(),
    )
    .await
}

async fn read_live_table_data_with_options(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    table_name: &str,
    limit: usize,
    offset: usize,
    include_total_count: bool,
    options: &DatabaseTableQueryOptions,
) -> Result<DatabaseTableData> {
    let start = Instant::now();
    // The data grid is read-only for views, but it must be able to inspect
    // every relation that the explorer exposes. PostgreSQL materialized views
    // are not listed by information_schema.tables, so try them explicitly.
    let table = match describe_live_table(
        connection,
        database_name,
        schema_name,
        table_name,
        DatabaseObjectType::Table,
    )
    .await
    {
        Ok(table) => table,
        Err(_) => match describe_live_table(
            connection,
            database_name,
            schema_name,
            table_name,
            DatabaseObjectType::View,
        )
        .await
        {
            Ok(view) => view,
            Err(_) => {
                describe_live_table(
                    connection,
                    database_name,
                    schema_name,
                    table_name,
                    DatabaseObjectType::MaterializedView,
                )
                .await?
            }
        },
    };
    let (_database, schema) = live_scope(connection, database_name, schema_name);
    let table_ref = live_table_ref(connection.profile.db_type, &schema, table_name);
    let fetch_limit = limit.saturating_add(1);
    let (where_clause, order_clause) =
        database_table_query_clauses(connection.profile.db_type, &table.columns, options)?;
    let rows = fetch_live_row_maps(
        connection,
        database_name,
        &format!("SELECT * FROM {table_ref}{where_clause}{order_clause} LIMIT {fetch_limit} OFFSET {offset}"),
    )
    .await?;
    let total_row_count = if include_total_count {
        fetch_live_count(
            connection,
            database_name,
            &format!("SELECT COUNT(*) AS __iowb_count FROM {table_ref}{where_clause}"),
        )
        .await?
    } else {
        None
    };
    let mut output = rows;
    let has_more = output.len() > limit;
    if has_more {
        output.truncate(limit);
    }
    Ok(DatabaseTableData {
        database_name: database_name
            .map(str::to_string)
            .or_else(|| Some(default_database_name(connection))),
        schema_name: schema_name.map(str::to_string),
        table_name: table_name.to_string(),
        offset,
        limit,
        row_count: output.len(),
        total_row_count,
        exact_total_row_count: include_total_count,
        has_more,
        columns: table.columns,
        rows: output,
        duration_ms: start.elapsed().as_millis(),
    })
}

fn database_table_query_clauses(
    db_type: SupportedDatabaseType,
    columns: &[DatabaseObjectColumn],
    options: &DatabaseTableQueryOptions,
) -> Result<(String, String)> {
    let quote_column = |name: &str| match db_type {
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            quote_mysql_identifier(name)
        }
        _ => quote_identifier(name),
    };
    let find_column = |name: &str| {
        columns
            .iter()
            .find(|column| column.name == name)
            .ok_or_else(|| {
                ServerError::new(
                    StatusCode::BAD_REQUEST,
                    format!("Unknown table column: {name}"),
                )
            })
    };
    let mut predicates = Vec::new();
    for filter in &options.filters {
        let column = find_column(&filter.column)?;
        let identifier = quote_column(&column.name);
        let predicate = match filter.operator.as_str() {
            "isNull" => format!("{identifier} IS NULL"),
            "isNotNull" => format!("{identifier} IS NOT NULL"),
            "eq" | "ne" if filter.value.is_null() => {
                format!(
                    "{identifier} IS {}NULL",
                    if filter.operator == "ne" { "NOT " } else { "" }
                )
            }
            "eq" | "ne" => {
                let operator = if filter.operator == "eq" { "=" } else { "<>" };
                let is_json = column
                    .data_type
                    .as_deref()
                    .into_iter()
                    .chain(column.native_type.as_deref())
                    .any(|kind| matches!(kind.to_ascii_lowercase().as_str(), "json" | "jsonb"));
                if is_json && db_type == SupportedDatabaseType::Postgresql {
                    let literal = database_text_literal(db_type, &filter.value.to_string());
                    format!("{identifier}::jsonb {operator} {literal}::jsonb")
                } else {
                    let literal = if is_json {
                        database_text_literal(db_type, &filter.value.to_string())
                    } else {
                        match &filter.value {
                            Value::String(value) => database_text_literal(db_type, value),
                            Value::Number(value) => value.to_string(),
                            Value::Bool(value) => if *value { "TRUE" } else { "FALSE" }.to_string(),
                            _ => {
                                return Err(ServerError::new(
                                    StatusCode::BAD_REQUEST,
                                    "Objects and arrays may only filter JSON columns",
                                ));
                            }
                        }
                    };
                    format!("{identifier} {operator} {literal}")
                }
            }
            _ => {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Supported filter operators: eq, ne, isNull, isNotNull",
                ));
            }
        };
        predicates.push(predicate);
    }
    let mut ordered_columns = Vec::new();
    let mut ordering = Vec::new();
    for sort in &options.sort {
        let column = find_column(&sort.column)?;
        let direction = match sort.direction.as_str() {
            "asc" => "ASC",
            "desc" => "DESC",
            _ => {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Sort direction must be asc or desc",
                ));
            }
        };
        if !ordered_columns.contains(&column.name.as_str()) {
            ordering.push(format!("{} {direction}", quote_column(&column.name)));
            ordered_columns.push(column.name.as_str());
        }
    }
    for column in columns.iter().filter(|column| column.is_primary_key) {
        if !ordered_columns.contains(&column.name.as_str()) {
            ordering.push(quote_column(&column.name));
        }
    }
    let where_clause = if predicates.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", predicates.join(" AND "))
    };
    let order_clause = if ordering.is_empty() {
        String::new()
    } else {
        format!(" ORDER BY {}", ordering.join(", "))
    };
    Ok((where_clause, order_clause))
}

#[cfg(test)]
fn live_table_page_sql(
    db_type: SupportedDatabaseType,
    table_ref: &str,
    primary_key: &[String],
    limit: usize,
    offset: usize,
) -> String {
    let order_by = if primary_key.is_empty() {
        String::new()
    } else {
        let columns = primary_key
            .iter()
            .map(|column| match db_type {
                SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
                    quote_mysql_identifier(column)
                }
                _ => quote_identifier(column),
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(" ORDER BY {columns}")
    };
    format!("SELECT * FROM {table_ref}{order_by} LIMIT {limit} OFFSET {offset}")
}

fn live_scope(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
) -> (String, String) {
    let database = database_name
        .or(connection.profile.database_name.as_deref())
        .map(str::to_string)
        .unwrap_or_else(|| default_database_name(connection));
    let schema = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => schema_name.unwrap_or("public").to_string(),
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => database.clone(),
        SupportedDatabaseType::Sqlite => "main".to_string(),
    };
    (database, schema)
}

fn live_table_ref(db_type: SupportedDatabaseType, schema: &str, table_name: &str) -> String {
    match db_type {
        SupportedDatabaseType::Postgresql => {
            format!(
                "{}.{}",
                quote_identifier(schema),
                quote_identifier(table_name)
            )
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            format!(
                "{}.{}",
                quote_mysql_identifier(schema),
                quote_mysql_identifier(table_name)
            )
        }
        SupportedDatabaseType::Sqlite => quote_identifier(table_name),
    }
}

fn quote_mysql_identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

async fn fetch_live_row_maps(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    sql: &str,
) -> Result<Vec<Map<String, Value>>> {
    match typed_live_pool_for_database(connection, database_name).await? {
        TypedLivePool::Postgresql(pool) => sqlx::raw_sql(sql)
            .fetch_all(&pool)
            .await
            .map(|rows| {
                rows.into_iter()
                    .map(|row| postgres_row_to_json_map(&row))
                    .collect()
            })
            .map_err(sqlx_server_error),
        TypedLivePool::Mysql(pool) => sqlx::raw_sql(sql)
            .fetch_all(&pool)
            .await
            .map(|rows| {
                rows.into_iter()
                    .map(|row| mysql_row_to_json_map(&row))
                    .collect()
            })
            .map_err(sqlx_server_error),
    }
}

async fn fetch_live_count(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    sql: &str,
) -> Result<Option<usize>> {
    let value = fetch_live_row_maps(connection, database_name, sql)
        .await?
        .into_iter()
        .next()
        .and_then(|row| row.get("__iowb_count").cloned());
    Ok(value.as_ref().and_then(json_value_to_usize))
}

fn json_value_to_usize(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .or_else(|| {
            value
                .as_i64()
                .filter(|value| *value >= 0)
                .and_then(|value| usize::try_from(value).ok())
        })
        .or_else(|| value.as_str()?.parse::<usize>().ok())
}

fn postgres_columns(columns: &[sqlx::postgres::PgColumn]) -> Vec<DatabaseObjectColumn> {
    columns
        .iter()
        .map(|column| DatabaseObjectColumn {
            name: column.name().to_string(),
            data_type: None,
            native_type: Some(column.type_info().name().to_string()),
            nullable: None,
            default_value: None,
            extra: None,
            is_primary_key: false,
        })
        .collect()
}

fn mysql_columns(columns: &[sqlx::mysql::MySqlColumn]) -> Vec<DatabaseObjectColumn> {
    columns
        .iter()
        .map(|column| DatabaseObjectColumn {
            name: column.name().to_string(),
            data_type: None,
            native_type: Some(column.type_info().name().to_string()),
            nullable: None,
            default_value: None,
            extra: None,
            is_primary_key: false,
        })
        .collect()
}

fn postgres_row_to_json_map(row: &PgRow) -> Map<String, Value> {
    let mut item = Map::new();
    for (index, column) in row.columns().iter().enumerate() {
        item.insert(
            column.name().to_string(),
            postgres_value_to_json(row, index),
        );
    }
    item
}

fn postgres_value_to_json(row: &PgRow, index: usize) -> Value {
    let type_info = row.columns()[index].type_info();
    // Simple-query/raw_sql rows can expose custom enum/domain OIDs without
    // resolving their catalog metadata. SQLx names those declarations "?";
    // calling kind() on one panics, including during post-insert readback.
    // The simple-query wire value is text and remains safe to preserve.
    if type_info.name() == "?" {
        return postgres_string_fallback(row, index);
    }
    if let PgTypeKind::Array(element_type) = type_info.kind() {
        return postgres_raw_string(row, index)
            .map(|value| {
                if postgres_array_element_is_normalized(element_type.name()) {
                    parse_postgres_array(&value, element_type.name())
                } else {
                    Value::String(value)
                }
            })
            .unwrap_or(Value::Null);
    }

    let type_name = match type_info.kind() {
        PgTypeKind::Domain(base_type) => base_type.name(),
        _ => type_info.name(),
    }
    .to_ascii_uppercase();

    match type_name.as_str() {
        "BOOL" => row
            .try_get_unchecked::<Option<bool>, _>(index)
            .ok()
            .flatten()
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        "INT2" | "INT4" | "OID" | "XID" | "CID" => row
            .try_get_unchecked::<Option<i64>, _>(index)
            .ok()
            .flatten()
            .map(|value| Value::Number(value.into()))
            .unwrap_or_else(|| postgres_string_fallback(row, index)),
        "INT8" | "NUMERIC" | "DECIMAL" | "MONEY" => postgres_string_fallback(row, index),
        "FLOAT4" | "FLOAT8" => row
            .try_get_unchecked::<Option<f64>, _>(index)
            .ok()
            .flatten()
            .map(json_float_value)
            .unwrap_or(Value::Null),
        "BYTEA" => row
            .try_get_unchecked::<Option<Vec<u8>>, _>(index)
            .ok()
            .flatten()
            .map(|value| database_buffer_value(&value))
            .unwrap_or(Value::Null),
        "JSON" | "JSONB" => row
            .try_get_unchecked::<Option<Value>, _>(index)
            .ok()
            .flatten()
            .map(database_json_wire_value)
            .unwrap_or(Value::Null),
        "DATE" => row
            .try_get_unchecked::<Option<NaiveDate>, _>(index)
            .ok()
            .flatten()
            .map(|value| value.to_string())
            .map(Value::String)
            .unwrap_or(Value::Null),
        "TIMESTAMP" => row
            .try_get_unchecked::<Option<NaiveDateTime>, _>(index)
            .ok()
            .flatten()
            .map(|value| value.format("%Y-%m-%dT%H:%M:%S%.f").to_string())
            .map(Value::String)
            .unwrap_or(Value::Null),
        "TIMESTAMPTZ" => row
            .try_get_unchecked::<Option<DateTime<Utc>>, _>(index)
            .ok()
            .flatten()
            .map(utc_datetime_to_iso)
            .map(Value::String)
            .unwrap_or(Value::Null),
        "POINT" => postgres_raw_string(row, index)
            .and_then(|value| parse_postgres_point(&value))
            .unwrap_or(Value::Null),
        "CIRCLE" => postgres_raw_string(row, index)
            .and_then(|value| parse_postgres_circle(&value))
            .unwrap_or(Value::Null),
        "INTERVAL" => postgres_raw_string(row, index)
            .map(|value| parse_postgres_interval(&value))
            .unwrap_or(Value::Null),
        _ => postgres_string_fallback(row, index),
    }
}

fn postgres_raw_string(row: &PgRow, index: usize) -> Option<String> {
    row.try_get_unchecked::<Option<String>, _>(index)
        .ok()
        .flatten()
}

fn postgres_string_fallback(row: &PgRow, index: usize) -> Value {
    postgres_raw_string(row, index)
        .map(Value::String)
        .or_else(|| {
            row.try_get_unchecked::<Option<Vec<u8>>, _>(index)
                .ok()
                .flatten()
                .map(|value| database_buffer_value(&value))
        })
        .unwrap_or(Value::Null)
}

fn postgres_array_element_is_normalized(type_name: &str) -> bool {
    matches!(
        type_name.to_ascii_uppercase().as_str(),
        "BOOL"
            | "BYTEA"
            | "INT2"
            | "INT4"
            | "OID"
            | "INT8"
            | "POINT"
            | "FLOAT4"
            | "FLOAT8"
            | "CHAR"
            | "VARCHAR"
            | "TEXT"
            | "BPCHAR"
            | "NAME"
            | "CIDR"
            | "MACADDR"
            | "INET"
            | "TIMESTAMP"
            | "DATE"
            | "TIMESTAMPTZ"
            | "INTERVAL"
            | "NUMERIC"
            | "JSON"
            | "JSONB"
            | "UUID"
            | "MONEY"
            | "TIME"
            | "TIMETZ"
    )
}

fn mysql_row_to_json_map(row: &MySqlRow) -> Map<String, Value> {
    let mut item = Map::new();
    for (index, column) in row.columns().iter().enumerate() {
        item.insert(column.name().to_string(), mysql_value_to_json(row, index));
    }
    item
}

fn mysql_value_to_json(row: &MySqlRow, index: usize) -> Value {
    let type_name = row.columns()[index].type_info().name().to_ascii_uppercase();
    match type_name.as_str() {
        "BOOLEAN" | "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "YEAR" => {
            mysql_integer_value(row, index, type_name.contains("UNSIGNED"))
        }
        "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "MEDIUMINT UNSIGNED" | "INT UNSIGNED"
        | "BIGINT UNSIGNED" => mysql_integer_value(row, index, true),
        "BIGINT" => mysql_integer_value(row, index, false),
        "FLOAT" | "DOUBLE" => row
            .try_get_unchecked::<Option<f64>, _>(index)
            .ok()
            .flatten()
            .map(json_float_value)
            .unwrap_or(Value::Null),
        "DECIMAL" => mysql_string_fallback(row, index),
        "JSON" => row
            .try_get_unchecked::<Option<Value>, _>(index)
            .ok()
            .flatten()
            .map(database_json_wire_value)
            .unwrap_or(Value::Null),
        "DATE" => row
            .try_get_unchecked::<Option<NaiveDate>, _>(index)
            .ok()
            .flatten()
            .map(|value| value.to_string())
            .map(Value::String)
            .unwrap_or(Value::Null),
        "DATETIME" | "TIMESTAMP" => row
            .try_get_unchecked::<Option<NaiveDateTime>, _>(index)
            .ok()
            .flatten()
            .map(|value| value.format("%Y-%m-%dT%H:%M:%S%.f").to_string())
            .map(Value::String)
            .unwrap_or(Value::Null),
        "GEOMETRY" => row
            .try_get_unchecked::<Option<Vec<u8>>, _>(index)
            .ok()
            .flatten()
            .and_then(|value| parse_mysql_geometry(&value))
            .unwrap_or(Value::Null),
        "VECTOR" => row
            .try_get_unchecked::<Option<Vec<u8>>, _>(index)
            .ok()
            .flatten()
            .map(|value| parse_mysql_vector(&value))
            .unwrap_or(Value::Null),
        // MariaDB's JSON alias is textual and takes the string fallback,
        // preserving encoded JSON (including null/string scalar provenance).
        // Native MySQL JSON keeps its dedicated decoder. Actual binary data
        // must remain bytes even when its UTF-8 happens to be valid JSON.
        "BIT" | "BINARY" | "VARBINARY" | "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" => row
            .try_get_unchecked::<Option<Vec<u8>>, _>(index)
            .ok()
            .flatten()
            .map(|value| database_buffer_value(&value))
            .unwrap_or(Value::Null),
        _ => mysql_string_fallback(row, index),
    }
}

fn mysql_integer_value(row: &MySqlRow, index: usize, unsigned: bool) -> Value {
    if unsigned {
        return row
            .try_get_unchecked::<Option<u64>, _>(index)
            .ok()
            .flatten()
            .map(|value| Value::Number(value.into()))
            .unwrap_or_else(|| mysql_string_fallback(row, index));
    }
    row.try_get_unchecked::<Option<i64>, _>(index)
        .ok()
        .flatten()
        .map(|value| Value::Number(value.into()))
        .unwrap_or_else(|| mysql_string_fallback(row, index))
}

fn mysql_string_fallback(row: &MySqlRow, index: usize) -> Value {
    row.try_get_unchecked::<Option<String>, _>(index)
        .ok()
        .flatten()
        .map(Value::String)
        .or_else(|| {
            row.try_get_unchecked::<Option<Vec<u8>>, _>(index)
                .ok()
                .flatten()
                .map(|value| database_buffer_value(&value))
        })
        .unwrap_or(Value::Null)
}

fn json_float_value(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or_else(|| Value::String(if value.is_nan() {
            "NaN".to_string()
        } else if value.is_sign_negative() {
            "-Infinity".to_string()
        } else {
            "Infinity".to_string()
        }))
}

// Preserve the difference between SQL NULL and JSON null, and between JSON
// strings such as "true" and their boolean/number lookalikes. Editors already
// submit encoded JSON text; objects and arrays remain structured for viewers.
fn database_json_wire_value(value: Value) -> Value {
    match value {
        Value::Null | Value::String(_) => Value::String(value.to_string()),
        _ => value,
    }
}

fn local_naive_to_iso(value: NaiveDateTime) -> String {
    Local
        .from_local_datetime(&value)
        .earliest()
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|| value.and_utc())
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn utc_datetime_to_iso(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

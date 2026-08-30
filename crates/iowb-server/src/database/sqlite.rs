fn sqlite_transfer_target_connection(connection: &StoredDatabaseConnection) -> Result<Connection> {
    let path = ensure_sqlite_connection(connection)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_server_error)?;
    }
    let conn = Connection::open(path).map_err(sqlite_server_error)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(sqlite_server_error)?;
    Ok(conn)
}

fn sqlite_objects(connection: &StoredDatabaseConnection) -> Result<Vec<DatabaseObjectSummary>> {
    let conn = sqlite_connection(connection)?;
    list_sqlite_objects(&conn)
}

fn list_sqlite_objects(conn: &Connection) -> Result<Vec<DatabaseObjectSummary>> {
    let mut stmt = conn
        .prepare(
            r#"
            SELECT name, type
            FROM sqlite_master
            WHERE type IN ('table', 'view')
              AND name NOT LIKE 'sqlite_%'
            ORDER BY type ASC, name ASC
            "#,
        )
        .map_err(sqlite_server_error)?;
    let rows = stmt
        .query_map([], |row| {
            let object_type = match row.get::<_, String>(1)?.as_str() {
                "view" => DatabaseObjectType::View,
                _ => DatabaseObjectType::Table,
            };
            Ok(DatabaseObjectSummary {
                name: row.get(0)?,
                object_type,
                database_name: Some("main".to_string()),
                schema_name: None,
            })
        })
        .map_err(sqlite_server_error)?;

    let mut objects = Vec::new();
    for row in rows {
        objects.push(row.map_err(sqlite_server_error)?);
    }
    Ok(objects)
}

struct SqliteTableDescription {
    columns: Vec<DatabaseObjectColumn>,
    foreign_keys: Vec<DatabaseForeignKey>,
}

fn describe_sqlite_table(
    connection: &StoredDatabaseConnection,
    table_name: &str,
    object_type: DatabaseObjectType,
) -> Result<SqliteTableDescription> {
    let conn = sqlite_connection(connection)?;
    describe_sqlite_table_inner(&conn, table_name, object_type)
}

fn describe_sqlite_table_inner(
    conn: &Connection,
    table_name: &str,
    object_type: DatabaseObjectType,
) -> Result<SqliteTableDescription> {
    let expected_sql_type = match object_type {
        DatabaseObjectType::View => "view",
        _ => "table",
    };
    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE name = ?1 AND type = ?2",
            params![table_name, expected_sql_type],
            |_| Ok(()),
        )
        .optional()
        .map_err(sqlite_server_error)?
        .is_some();
    if !exists {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "Table not found"));
    }

    let mut columns_stmt = conn
        .prepare(&format!(
            "PRAGMA table_info({})",
            quote_identifier(table_name)
        ))
        .map_err(sqlite_server_error)?;
    let column_rows = columns_stmt
        .query_map([], |row| {
            let native_type: String = row.get(2)?;
            let not_null = row.get::<_, i64>(3)? == 1;
            let default_value: Option<String> = row.get(4)?;
            let primary_key_position = row.get::<_, i64>(5)?;
            Ok(DatabaseObjectColumn {
                name: row.get(1)?,
                data_type: Some(native_type.to_lowercase()),
                native_type: Some(native_type),
                nullable: Some(!not_null),
                default_value,
                extra: None,
                is_primary_key: primary_key_position > 0,
            })
        })
        .map_err(sqlite_server_error)?;

    let mut columns = Vec::new();
    for row in column_rows {
        columns.push(row.map_err(sqlite_server_error)?);
    }

    let mut fk_stmt = conn
        .prepare(&format!(
            "PRAGMA foreign_key_list({})",
            quote_identifier(table_name)
        ))
        .map_err(sqlite_server_error)?;
    let fk_rows = fk_stmt
        .query_map([], |row| {
            Ok(DatabaseForeignKey {
                name: None,
                column_name: row.get(3)?,
                referenced_schema_name: None,
                referenced_table_name: row.get(2)?,
                referenced_column_name: row.get(4)?,
                on_update: row.get(5)?,
                on_delete: row.get(6)?,
            })
        })
        .map_err(sqlite_server_error)?;

    let mut foreign_keys = Vec::new();
    for row in fk_rows {
        foreign_keys.push(row.map_err(sqlite_server_error)?);
    }

    Ok(SqliteTableDescription {
        columns,
        foreign_keys,
    })
}

fn sqlite_relational_schema(
    connection: &StoredDatabaseConnection,
    objects: &[DatabaseObjectSummary],
) -> Result<DatabaseRelationalSchema> {
    let conn = sqlite_connection(connection)?;
    let mut tables = Vec::new();
    let mut relationships = Vec::new();

    for object in objects.iter().filter(|object| {
        matches!(
            object.object_type,
            DatabaseObjectType::Table | DatabaseObjectType::View
        )
    }) {
        let table = describe_sqlite_table_inner(&conn, &object.name, object.object_type)?;
        for foreign_key in &table.foreign_keys {
            relationships.push(DatabaseRelationalSchemaRelationship {
                id: format!(
                    "{}:{}:{}",
                    object.name, foreign_key.column_name, foreign_key.referenced_table_name
                ),
                name: foreign_key.name.clone(),
                source_database_name: Some("main".to_string()),
                source_schema_name: None,
                source_table_name: object.name.clone(),
                source_column_name: foreign_key.column_name.clone(),
                target_database_name: Some("main".to_string()),
                target_schema_name: None,
                target_table_name: foreign_key.referenced_table_name.clone(),
                target_column_name: foreign_key.referenced_column_name.clone(),
                on_update: foreign_key.on_update.clone(),
                on_delete: foreign_key.on_delete.clone(),
            });
        }
        tables.push(DatabaseRelationalSchemaTable {
            database_name: Some("main".to_string()),
            schema_name: None,
            name: object.name.clone(),
            object_type: object.object_type,
            columns: table.columns,
            is_external: false,
        });
    }

    Ok(DatabaseRelationalSchema {
        scope_type: DatabaseObjectType::Database,
        scope_name: "main".to_string(),
        tables,
        relationships,
    })
}

fn execute_sqlite_query(
    connection: &StoredDatabaseConnection,
    sql: &str,
    max_rows: usize,
) -> Result<DatabaseQueryResult> {
    let start = Instant::now();
    let conn = sqlite_connection(connection)?;
    let statement_type = classify_statement(sql);
    let mut stmt = conn.prepare(sql).map_err(sqlite_server_error)?;
    let column_count = stmt.column_count();

    if column_count == 0 {
        let changed = stmt.execute([]).map_err(sqlite_server_error)?;
        let last_insert_row_id = conn.last_insert_rowid().to_string();
        return Ok(DatabaseQueryResult {
            sql: sql.to_string(),
            statement_type,
            row_count: changed,
            returned_row_count: 0,
            result_truncated: false,
            max_rows,
            rows: Vec::new(),
            columns: Vec::new(),
            notices: Vec::new(),
            duration_ms: start.elapsed().as_millis(),
            meta: Some(query_result_meta(
                0,
                false,
                max_rows,
                true,
                Some(serde_json::json!({
                    "changes": changed,
                    "lastInsertRowid": last_insert_row_id,
                })),
            )),
            database_name: None,
            schema_name: None,
        });
    }

    let column_names = stmt
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let columns = column_names
        .iter()
        .map(|name| DatabaseObjectColumn {
            name: name.clone(),
            data_type: None,
            native_type: None,
            nullable: None,
            default_value: None,
            extra: None,
            is_primary_key: false,
        })
        .collect::<Vec<_>>();
    let mut rows = stmt.query([]).map_err(sqlite_server_error)?;
    let mut output = Vec::new();
    let mut row_count = 0usize;
    let mut result_truncated = false;
    while let Some(row) = rows.next().map_err(sqlite_server_error)? {
        row_count += 1;
        if output.len() < max_rows {
            output.push(row_to_json_map(row, &column_names)?);
        } else {
            result_truncated = true;
            break;
        }
    }
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

fn query_result_meta(
    returned_row_count: usize,
    result_truncated: bool,
    max_rows: usize,
    row_count_exact: bool,
    extra: Option<Value>,
) -> Value {
    let mut meta = match extra {
        Some(Value::Object(meta)) => meta,
        _ => Map::new(),
    };
    meta.insert(
        "returnedRowCount".to_string(),
        Value::from(returned_row_count),
    );
    meta.insert("resultTruncated".to_string(), Value::Bool(result_truncated));
    meta.insert("maxRows".to_string(), Value::from(max_rows));
    meta.insert("rowCountExact".to_string(), Value::Bool(row_count_exact));
    Value::Object(meta)
}

fn read_sqlite_table_data(
    connection: &StoredDatabaseConnection,
    table_name: &str,
    limit: usize,
    offset: usize,
    include_total_count: bool,
) -> Result<DatabaseTableData> {
    let start = Instant::now();
    let conn = sqlite_connection(connection)?;
    let table = describe_sqlite_table_inner(&conn, table_name, DatabaseObjectType::Table)
        .or_else(|_| describe_sqlite_table_inner(&conn, table_name, DatabaseObjectType::View))?;
    let total_row_count = if include_total_count {
        Some(count_sqlite_rows(&conn, table_name)?)
    } else {
        None
    };
    let sql = format!(
        "SELECT * FROM {} LIMIT ?1 OFFSET ?2",
        quote_identifier(table_name)
    );
    let mut stmt = conn.prepare(&sql).map_err(sqlite_server_error)?;
    let column_names = stmt
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut rows = stmt
        .query(params![limit as i64 + 1, offset as i64])
        .map_err(sqlite_server_error)?;
    let mut output = Vec::new();
    while let Some(row) = rows.next().map_err(sqlite_server_error)? {
        output.push(row_to_json_map(row, &column_names)?);
        if output.len() > limit {
            break;
        }
    }
    let has_more = output.len() > limit;
    if has_more {
        output.truncate(limit);
    }

    Ok(DatabaseTableData {
        database_name: None,
        schema_name: None,
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

fn count_sqlite_rows(conn: &Connection, table_name: &str) -> Result<usize> {
    let sql = format!("SELECT COUNT(*) FROM {}", quote_identifier(table_name));
    let count: i64 = conn
        .query_row(&sql, [], |row| row.get(0))
        .map_err(sqlite_server_error)?;
    Ok(count.max(0) as usize)
}

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
            SELECT name, type, tbl_name
            FROM sqlite_master
            WHERE type IN ('table', 'view', 'index', 'trigger')
              AND name NOT LIKE 'sqlite_%'
            ORDER BY
              CASE type
                WHEN 'table' THEN 0
                WHEN 'view' THEN 1
                WHEN 'index' THEN 2
                WHEN 'trigger' THEN 3
                ELSE 4
              END,
              name ASC
            "#,
        )
        .map_err(sqlite_server_error)?;
    let rows = stmt
        .query_map([], |row| {
            let raw_type = row.get::<_, String>(1)?;
            let object_type = match raw_type.as_str() {
                "view" => DatabaseObjectType::View,
                "index" => DatabaseObjectType::Index,
                "trigger" => DatabaseObjectType::Trigger,
                _ => DatabaseObjectType::Table,
            };
            let name = row.get::<_, String>(0)?;
            Ok(DatabaseObjectSummary {
                name: name.clone(),
                object_type,
                database_name: Some("main".to_string()),
                schema_name: None,
                parent_name: matches!(object_type, DatabaseObjectType::Index | DatabaseObjectType::Trigger)
                    .then(|| row.get::<_, String>(2))
                    .transpose()?,
                identity: Some(name),
            })
        })
        .map_err(sqlite_server_error)?;

    let mut objects = Vec::new();
    for row in rows {
        objects.push(row.map_err(sqlite_server_error)?);
    }

    let table_names = objects
        .iter()
        .filter(|object| object.object_type == DatabaseObjectType::Table)
        .map(|object| object.name.clone())
        .collect::<Vec<_>>();
    for table_name in table_names {
        objects.extend(sqlite_constraint_summaries(conn, &table_name)?);
    }
    objects.sort_by(|left, right| {
        database_object_type_as_str(left.object_type)
            .cmp(database_object_type_as_str(right.object_type))
            .then_with(|| left.parent_name.cmp(&right.parent_name))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.identity.cmp(&right.identity))
    });
    Ok(objects)
}

fn sqlite_constraint_summaries(
    conn: &Connection,
    table_name: &str,
) -> Result<Vec<DatabaseObjectSummary>> {
    let mut summaries = Vec::new();
    let primary_key_columns = sqlite_primary_key_columns(conn, table_name)?;
    if !primary_key_columns.is_empty() {
        summaries.push(DatabaseObjectSummary {
            name: "PRIMARY KEY".to_string(),
            object_type: DatabaseObjectType::Constraint,
            database_name: Some("main".to_string()),
            schema_name: None,
            parent_name: Some(table_name.to_string()),
            identity: Some(format!("{table_name}:primary-key")),
        });
    }

    let mut foreign_key_ids = sqlite_foreign_key_rows(conn, table_name)?
        .into_iter()
        .map(|foreign_key| foreign_key.id)
        .collect::<Vec<_>>();
    foreign_key_ids.sort_unstable();
    foreign_key_ids.dedup();
    for (position, id) in foreign_key_ids.into_iter().enumerate() {
        summaries.push(DatabaseObjectSummary {
            name: format!("FOREIGN KEY #{}", position + 1),
            object_type: DatabaseObjectType::Constraint,
            database_name: Some("main".to_string()),
            schema_name: None,
            parent_name: Some(table_name.to_string()),
            identity: Some(format!("{table_name}:foreign-key:{id}")),
        });
    }

    for index in sqlite_unique_indexes(conn, table_name)? {
        if index.unique && index.origin == "u" {
            summaries.push(DatabaseObjectSummary {
                name: format!("UNIQUE ({})", index.columns.join(", ")),
                object_type: DatabaseObjectType::Constraint,
                database_name: Some("main".to_string()),
                schema_name: None,
                parent_name: Some(table_name.to_string()),
                identity: Some(format!("{table_name}:unique:{}", index.name)),
            });
        }
    }

    for (position, check) in sqlite_check_constraints(conn, table_name)?.into_iter().enumerate() {
        summaries.push(DatabaseObjectSummary {
            name: check.name,
            object_type: DatabaseObjectType::Constraint,
            database_name: Some("main".to_string()),
            schema_name: None,
            parent_name: Some(table_name.to_string()),
            identity: Some(format!("{table_name}:check:{}", position + 1)),
        });
    }

    Ok(summaries)
}

#[derive(Debug, Clone)]
struct SqliteForeignKeyRow {
    id: i64,
    sequence: i64,
    key: DatabaseForeignKey,
}

#[derive(Debug, Clone)]
struct SqliteUniqueIndex {
    name: String,
    unique: bool,
    origin: String,
    partial: bool,
    columns: Vec<String>,
}

#[derive(Debug, Clone)]
struct SqliteCheckConstraint {
    name: String,
    definition: String,
    expression: String,
}

fn sqlite_primary_key_columns(conn: &Connection, table_name: &str) -> Result<Vec<String>> {
    let table_info_sql = format!("PRAGMA table_xinfo({})", quote_identifier(table_name));
    let mut table_info = conn
        .prepare(&table_info_sql)
        .map_err(sqlite_server_error)?;
    let rows = table_info
        .query_map([], |row| Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?)))
        .map_err(sqlite_server_error)?;
    let mut columns = Vec::new();
    for row in rows {
        let (name, position) = row.map_err(sqlite_server_error)?;
        if position > 0 {
            columns.push((position, name));
        }
    }
    columns.sort_by_key(|(position, _)| *position);
    Ok(columns.into_iter().map(|(_, name)| name).collect())
}

fn sqlite_foreign_key_rows(
    conn: &Connection,
    table_name: &str,
) -> Result<Vec<SqliteForeignKeyRow>> {
    let foreign_key_sql = format!("PRAGMA foreign_key_list({})", quote_identifier(table_name));
    let mut foreign_keys = conn
        .prepare(&foreign_key_sql)
        .map_err(sqlite_server_error)?;
    let rows = foreign_keys
        .query_map([], |row| {
            Ok(SqliteForeignKeyRow {
                id: row.get(0)?,
                sequence: row.get(1)?,
                key: DatabaseForeignKey {
                    name: None,
                    column_name: row.get(3)?,
                    referenced_database_name: None,
                    referenced_schema_name: None,
                    referenced_table_name: row.get(2)?,
                    referenced_column_name: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    on_update: row.get(5)?,
                    on_delete: row.get(6)?,
                },
            })
        })
        .map_err(sqlite_server_error)?;
    let mut output = Vec::new();
    for row in rows {
        output.push(row.map_err(sqlite_server_error)?);
    }
    Ok(output)
}

fn sqlite_unique_indexes(conn: &Connection, table_name: &str) -> Result<Vec<SqliteUniqueIndex>> {
    let index_sql = format!("PRAGMA index_list({})", quote_identifier(table_name));
    let mut indexes = conn.prepare(&index_sql).map_err(sqlite_server_error)?;
    let index_rows = indexes
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? > 0,
                row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                row.get::<_, i64>(4).unwrap_or(0) > 0,
            ))
        })
        .map_err(sqlite_server_error)?;
    let mut output = Vec::new();
    for row in index_rows {
        let (name, unique, origin, partial) = row.map_err(sqlite_server_error)?;
        let columns = sqlite_index_columns(conn, &name)?;
        output.push(SqliteUniqueIndex {
            name,
            unique,
            origin,
            partial,
            columns,
        });
    }
    Ok(output)
}

fn sqlite_index_columns(conn: &Connection, index_name: &str) -> Result<Vec<String>> {
    let index_columns_sql = format!("PRAGMA index_info({})", quote_identifier(index_name));
    let mut index_columns = conn
        .prepare(&index_columns_sql)
        .map_err(sqlite_server_error)?;
    let rows = index_columns
        .query_map([], |row| row.get::<_, Option<String>>(2))
        .map_err(sqlite_server_error)?;
    let mut columns = Vec::new();
    for row in rows {
        if let Some(column) = row.map_err(sqlite_server_error)? {
            columns.push(column);
        }
    }
    Ok(columns)
}

/// SQLite does not expose CHECK constraints through a pragma. The source DDL
/// stored in `sqlite_master` is therefore the authoritative catalog. Keep the
/// parser intentionally small and scoped to the `CREATE TABLE (...)` body:
/// strings, quoted identifiers, and comments are masked before looking for
/// structural punctuation so an expression such as `CHECK(note = 'a,b')` is
/// not accidentally split in two.
fn sqlite_check_constraints(
    conn: &Connection,
    table_name: &str,
) -> Result<Vec<SqliteCheckConstraint>> {
    let create_sql = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table_name],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(sqlite_server_error)?
        .flatten();
    Ok(create_sql
        .as_deref()
        .map(sqlite_check_constraints_from_create_sql)
        .unwrap_or_default())
}

fn sqlite_check_constraints_from_create_sql(create_sql: &str) -> Vec<SqliteCheckConstraint> {
    let mask = sqlite_sql_code_mask(create_sql);
    let Some(body_open) = mask.iter().position(|byte| *byte == b'(') else {
        return Vec::new();
    };
    let Some(body_close) = sqlite_matching_parenthesis(&mask, body_open) else {
        return Vec::new();
    };

    let mut checks = Vec::new();
    for (definition_start, definition_end) in
        sqlite_top_level_definitions(create_sql, &mask, body_open + 1, body_close)
    {
        for check_start in
            sqlite_top_level_keyword_positions(&mask, definition_start, definition_end, b"CHECK")
        {
            let check_open = sqlite_skip_code_whitespace(&mask, check_start + b"CHECK".len());
            if mask.get(check_open) != Some(&b'(') {
                continue;
            }
            let Some(check_close) = sqlite_matching_parenthesis(&mask, check_open) else {
                continue;
            };
            if check_close >= definition_end {
                continue;
            }
            let named_constraint = sqlite_constraint_name_before_check(
                create_sql,
                &mask,
                definition_start,
                check_start,
            );
            let clause_start = named_constraint
                .as_ref()
                .map(|(_, position)| *position)
                .unwrap_or(check_start);
            let display_name = named_constraint
                .map(|(name, _)| format!("CONSTRAINT {name}"))
                .unwrap_or_else(|| format!("CHECK #{}", checks.len() + 1));
            checks.push(SqliteCheckConstraint {
                name: display_name,
                definition: create_sql[clause_start..=check_close].trim().to_string(),
                expression: create_sql[check_open + 1..check_close].trim().to_string(),
            });
        }
    }
    checks
}

/// Return a copy where every byte inside a quoted string/identifier or SQL
/// comment is a space. Structural ASCII outside those regions remains intact.
fn sqlite_sql_code_mask(sql: &str) -> Vec<u8> {
    let source = sql.as_bytes();
    let mut mask = source.to_vec();
    let mut index = 0;
    while index < source.len() {
        match source[index] {
            b'\'' => {
                mask[index] = b' ';
                index += 1;
                while index < source.len() {
                    mask[index] = b' ';
                    if source[index] == b'\'' {
                        if source.get(index + 1) == Some(&b'\'') {
                            mask[index + 1] = b' ';
                            index += 2;
                            continue;
                        }
                        index += 1;
                        break;
                    }
                    index += 1;
                }
            }
            b'"' => {
                mask[index] = b' ';
                index += 1;
                while index < source.len() {
                    mask[index] = b' ';
                    if source[index] == b'"' {
                        if source.get(index + 1) == Some(&b'"') {
                            mask[index + 1] = b' ';
                            index += 2;
                            continue;
                        }
                        index += 1;
                        break;
                    }
                    index += 1;
                }
            }
            b'`' => {
                mask[index] = b' ';
                index += 1;
                while index < source.len() {
                    mask[index] = b' ';
                    if source[index] == b'`' {
                        if source.get(index + 1) == Some(&b'`') {
                            mask[index + 1] = b' ';
                            index += 2;
                            continue;
                        }
                        index += 1;
                        break;
                    }
                    index += 1;
                }
            }
            b'[' => {
                mask[index] = b' ';
                index += 1;
                while index < source.len() {
                    mask[index] = b' ';
                    if source[index] == b']' {
                        index += 1;
                        break;
                    }
                    index += 1;
                }
            }
            b'-' if source.get(index + 1) == Some(&b'-') => {
                mask[index] = b' ';
                mask[index + 1] = b' ';
                index += 2;
                while index < source.len() && source[index] != b'\n' {
                    mask[index] = b' ';
                    index += 1;
                }
            }
            b'/' if source.get(index + 1) == Some(&b'*') => {
                mask[index] = b' ';
                mask[index + 1] = b' ';
                index += 2;
                while index < source.len() {
                    mask[index] = b' ';
                    if source[index] == b'*' && source.get(index + 1) == Some(&b'/') {
                        mask[index + 1] = b' ';
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            _ => index += 1,
        }
    }
    mask
}

fn sqlite_matching_parenthesis(mask: &[u8], open: usize) -> Option<usize> {
    if mask.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    for (index, byte) in mask.iter().copied().enumerate().skip(open) {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn sqlite_top_level_definitions(
    sql: &str,
    mask: &[u8],
    body_start: usize,
    body_end: usize,
) -> Vec<(usize, usize)> {
    let mut definitions = Vec::new();
    let mut definition_start = body_start;
    let mut depth = 0usize;
    for index in body_start..body_end {
        match mask[index] {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                if let Some(bounds) = sqlite_trim_sql_bounds(sql, definition_start, index) {
                    definitions.push(bounds);
                }
                definition_start = index + 1;
            }
            _ => {}
        }
    }
    if let Some(bounds) = sqlite_trim_sql_bounds(sql, definition_start, body_end) {
        definitions.push(bounds);
    }
    definitions
}

fn sqlite_trim_sql_bounds(sql: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let value = sql.get(start..end)?;
    let trimmed_start = value.len() - value.trim_start().len();
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some((start + trimmed_start, start + trimmed_start + trimmed.len()))
}

fn sqlite_top_level_keyword_positions(
    mask: &[u8],
    start: usize,
    end: usize,
    keyword: &[u8],
) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut index = start;
    let mut depth = 0usize;
    while index + keyword.len() <= end {
        match mask[index] {
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            _ if depth == 0
                && mask[index..index + keyword.len()].eq_ignore_ascii_case(keyword)
                && !sqlite_identifier_byte(mask.get(index.wrapping_sub(1)).copied())
                && !sqlite_identifier_byte(mask.get(index + keyword.len()).copied()) =>
            {
                positions.push(index);
                index += keyword.len();
            }
            _ => index += 1,
        }
    }
    positions
}

fn sqlite_identifier_byte(byte: Option<u8>) -> bool {
    byte.is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
}

fn sqlite_skip_code_whitespace(mask: &[u8], mut index: usize) -> usize {
    while mask.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

fn sqlite_constraint_name_before_check(
    sql: &str,
    mask: &[u8],
    definition_start: usize,
    check_start: usize,
) -> Option<(String, usize)> {
    let constraint_start = sqlite_top_level_keyword_positions(
        mask,
        definition_start,
        check_start,
        b"CONSTRAINT",
    )
    .pop()?;
    let name_start = sqlite_skip_sql_space_and_comments(
        sql,
        constraint_start + b"CONSTRAINT".len(),
        check_start,
    );
    let (name, _) = sqlite_read_identifier(sql, name_start, check_start)?;
    Some((name, constraint_start))
}

fn sqlite_skip_sql_space_and_comments(sql: &str, mut index: usize, end: usize) -> usize {
    let bytes = sql.as_bytes();
    while index < end {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if bytes[index] == b'-' && bytes.get(index + 1) == Some(&b'-') {
            index += 2;
            while index < end && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index < end {
                if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        break;
    }
    index
}

fn sqlite_read_identifier(sql: &str, start: usize, end: usize) -> Option<(String, usize)> {
    let bytes = sql.as_bytes();
    let first = *bytes.get(start)?;
    let close = match first {
        b'"' => b'"',
        b'`' => b'`',
        b'[' => b']',
        _ => {
            let mut index = start;
            while index < end
                && bytes
                    .get(index)
                    .is_some_and(|byte| sqlite_identifier_byte(Some(*byte)))
            {
                index += 1;
            }
            return (index > start).then(|| (sql[start..index].to_string(), index));
        }
    };
    let mut index = start + 1;
    while index < end {
        if bytes[index] == close {
            if close != b']' && bytes.get(index + 1) == Some(&close) {
                index += 2;
                continue;
            }
            return Some((sql[start..=index].to_string(), index + 1));
        }
        index += 1;
    }
    None
}

struct SqliteTableDescription {
    columns: Vec<DatabaseObjectColumn>,
    foreign_keys: Vec<DatabaseForeignKey>,
    metadata: Option<Value>,
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

    // SQLite treats a sole `INTEGER PRIMARY KEY` on a rowid table as an
    // automatically assigned rowid, even without the AUTOINCREMENT keyword.
    // Surface that fact like the live-engine auto-increment markers so every
    // mobile client can leave it out of an insert form.
    let primary_key_columns = sqlite_primary_key_columns(conn, table_name)?;
    let master_definition = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = ?1 AND type = ?2",
            params![table_name, expected_sql_type],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(sqlite_server_error)?
        .flatten();
    let normalized_definition = master_definition
        .as_deref()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let is_rowid_table = if object_type == DatabaseObjectType::Table {
        !normalized_definition.contains("WITHOUT ROWID")
    } else {
        false
    };
    let has_generated_rowid_primary_key = is_rowid_table && primary_key_columns.len() == 1;

    let mut columns_stmt = conn
        .prepare(&format!(
            "PRAGMA table_xinfo({})",
            quote_identifier(table_name)
        ))
        .map_err(sqlite_server_error)?;
    let column_rows = columns_stmt
        .query_map([], |row| {
            let native_type: String = row.get(2)?;
            let not_null = row.get::<_, i64>(3)? == 1;
            let default_value: Option<String> = row.get(4)?;
            let primary_key_position = row.get::<_, i64>(5)?;
            let hidden = row.get::<_, i64>(6)?;
            // table_xinfo includes generated columns (2 = virtual, 3 =
            // stored) while hiding internal virtual-table implementation
            // columns (1). SELECT * exposes generated values, so returning
            // them here keeps the row data and its metadata in sync.
            if hidden == 1 {
                return Ok(None);
            }
            let extra = match hidden {
                2 => Some("generated virtual".to_string()),
                3 => Some("generated stored".to_string()),
                _ if has_generated_rowid_primary_key
                    && primary_key_position == 1
                    && native_type.trim().eq_ignore_ascii_case("integer") =>
                {
                    Some("auto_increment".to_string())
                }
                _ => None,
            };
            Ok(Some(DatabaseObjectColumn {
                name: row.get(1)?,
                data_type: Some(native_type.to_lowercase()),
                native_type: Some(native_type),
                nullable: Some(!not_null),
                default_value,
                extra,
                is_primary_key: primary_key_position > 0,
            }))
        })
        .map_err(sqlite_server_error)?;

    let mut columns = Vec::new();
    for row in column_rows {
        if let Some(column) = row.map_err(sqlite_server_error)? {
            columns.push(column);
        }
    }

    let mut fk_rows = sqlite_foreign_key_rows(conn, table_name)?;
    fk_rows.sort_by_key(|row| (row.id, row.sequence));
    let foreign_keys = fk_rows
        .into_iter()
        .map(|mut row| {
            // PRAGMA omits constraint names but identifies each relationship
            // by id. Keep composite parts together and separate distinct
            // relationships to the same table in the shared client protocol.
            row.key.name = Some(format!("sqlite_fk_{}", row.id));
            row.key
        })
        .collect();

    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "objectType".to_string(),
        Value::String(expected_sql_type.to_string()),
    );
    if object_type == DatabaseObjectType::Table {
        let normalized_without_semicolon = normalized_definition
            .trim_end_matches(';')
            .trim_end();
        metadata.insert(
            "withoutRowid".to_string(),
            Value::Bool(normalized_definition.contains("WITHOUT ROWID")),
        );
        metadata.insert(
            "strict".to_string(),
            Value::Bool(normalized_without_semicolon.ends_with("STRICT")),
        );
        metadata.insert(
            "virtual".to_string(),
            Value::Bool(normalized_definition.starts_with("CREATE VIRTUAL TABLE")),
        );
        metadata.insert(
            "rowidPrimaryKey".to_string(),
            Value::Bool(has_generated_rowid_primary_key),
        );
    }
    let metadata = Some(Value::Object(metadata));

    Ok(SqliteTableDescription {
        columns,
        foreign_keys,
        metadata,
    })
}

struct SqliteObjectDescription {
    parent_name: Option<String>,
    identity: Option<String>,
    columns: Vec<DatabaseObjectColumn>,
    primary_key: Vec<String>,
    foreign_keys: Vec<DatabaseForeignKey>,
    definition: Option<String>,
    metadata: Option<Value>,
}

fn describe_sqlite_object(
    connection: &StoredDatabaseConnection,
    object_type: DatabaseObjectType,
    name: &str,
    parent_name: Option<&str>,
    identity: Option<&str>,
) -> Result<SqliteObjectDescription> {
    let conn = sqlite_connection(connection)?;
    describe_sqlite_object_inner(&conn, object_type, name, parent_name, identity)
}

fn describe_sqlite_object_inner(
    conn: &Connection,
    object_type: DatabaseObjectType,
    name: &str,
    parent_name: Option<&str>,
    identity: Option<&str>,
) -> Result<SqliteObjectDescription> {
    if matches!(object_type, DatabaseObjectType::Table | DatabaseObjectType::View) {
        let expected_type = if object_type == DatabaseObjectType::View {
            "view"
        } else {
            "table"
        };
        let definition = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = ?1 AND type = ?2",
                params![name, expected_type],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(sqlite_server_error)?
            .flatten()
            .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Database object not found"))?;
        let table = describe_sqlite_table_inner(conn, name, object_type)?;
        let primary_key = sqlite_primary_key_columns(conn, name)?;
        return Ok(SqliteObjectDescription {
            parent_name: None,
            identity: Some(identity.unwrap_or(name).to_string()),
            columns: table.columns,
            primary_key,
            foreign_keys: table.foreign_keys,
            definition: Some(definition),
            metadata: table.metadata,
        });
    }

    if object_type == DatabaseObjectType::Constraint {
        return describe_sqlite_constraint_inner(conn, name, parent_name, identity);
    }

    let (master_type, master_parent, definition) = conn
        .query_row(
            "SELECT type, tbl_name, sql FROM sqlite_master WHERE name = ?1",
            params![name],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_server_error)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Database object not found"))?;
    let expected_type = match object_type {
        DatabaseObjectType::Index => "index",
        DatabaseObjectType::Trigger => "trigger",
        _ => "",
    };
    if master_type != expected_type {
        return Err(ServerError::new(
            StatusCode::NOT_FOUND,
            "Database object type does not match",
        ));
    }
    let resolved_parent = parent_name
        .filter(|value| !value.trim().is_empty())
        .or_else(|| Some(master_parent.as_str()))
        .map(str::to_string);
    let resolved_identity = identity
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(name)
        .to_string();

    match object_type {
        DatabaseObjectType::Index => {
            let table_name = resolved_parent.as_deref().unwrap_or("");
            let index_list_sql = format!("PRAGMA index_list({})", quote_identifier(table_name));
            let mut index_list = conn
                .prepare(&index_list_sql)
                .map_err(sqlite_server_error)?;
            let index_info = index_list
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? > 0,
                        row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                        row.get::<_, i64>(4).unwrap_or(0) > 0,
                    ))
                })
                .map_err(sqlite_server_error)?
                .filter_map(|row| row.ok().filter(|row| row.0 == name))
                .next();
            let (_, unique, origin, partial) =
                index_info.unwrap_or((name.to_string(), false, "c".to_string(), false));
            let index_columns_sql = format!("PRAGMA index_info({})", quote_identifier(name));
            let mut index_columns_stmt = conn
                .prepare(&index_columns_sql)
                .map_err(sqlite_server_error)?;
            let index_columns = index_columns_stmt
                .query_map([], |row| row.get::<_, Option<String>>(2))
                .map_err(sqlite_server_error)?
                .filter_map(|row| row.ok().flatten())
                .collect::<Vec<_>>();
            Ok(SqliteObjectDescription {
                parent_name: resolved_parent,
                identity: Some(resolved_identity),
                columns: Vec::new(),
                primary_key: Vec::new(),
                foreign_keys: Vec::new(),
                definition,
                metadata: Some(serde_json::json!({
                    "unique": unique,
                    "origin": origin,
                    "partial": partial,
                    "columns": index_columns,
                })),
            })
        }
        DatabaseObjectType::Trigger => Ok(SqliteObjectDescription {
            parent_name: resolved_parent,
            identity: Some(resolved_identity),
            columns: Vec::new(),
            primary_key: Vec::new(),
            foreign_keys: Vec::new(),
            definition,
            metadata: Some(serde_json::json!({
                "tableName": master_parent,
            })),
        }),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Unsupported SQLite object type",
        )),
    }
}

fn describe_sqlite_constraint_inner(
    conn: &Connection,
    name: &str,
    parent_name: Option<&str>,
    identity: Option<&str>,
) -> Result<SqliteObjectDescription> {
    let table_name = parent_name
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Constraint table is required"))?;
    let table = describe_sqlite_table_inner(conn, table_name, DatabaseObjectType::Table)?;
    let resolved_identity = identity
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(name);
    let prefix = format!("{table_name}:");
    let kind = resolved_identity.strip_prefix(&prefix).ok_or_else(|| {
        ServerError::new(
            StatusCode::NOT_FOUND,
            "Constraint identity does not belong to the selected table",
        )
    })?;

    let object = |kind: &str,
                  columns: Vec<DatabaseObjectColumn>,
                  foreign_keys: Vec<DatabaseForeignKey>,
                  definition: String,
                  extra_metadata: Value| {
        let mut metadata = serde_json::json!({
            "tableName": table_name,
            "displayName": name,
            "kind": kind,
        });
        if let (Some(metadata), Value::Object(extra)) = (metadata.as_object_mut(), extra_metadata) {
            metadata.extend(extra);
        }
        SqliteObjectDescription {
            parent_name: Some(table_name.to_string()),
            identity: Some(resolved_identity.to_string()),
            columns,
            primary_key: Vec::new(),
            foreign_keys,
            definition: Some(definition),
            metadata: Some(metadata),
        }
    };

    if kind == "primary-key" {
        let primary_key = sqlite_primary_key_columns(conn, table_name)?;
        if primary_key.is_empty() {
            return Err(ServerError::new(
                StatusCode::NOT_FOUND,
                "Primary-key constraint no longer exists",
            ));
        }
        let columns = sqlite_table_columns_for_names(&table.columns, &primary_key);
        return Ok(object(
            "primary-key",
            columns,
            Vec::new(),
            format!(
                "PRIMARY KEY ({})",
                primary_key
                    .iter()
                    .map(|column| quote_identifier(column))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            serde_json::json!({ "columns": primary_key }),
        ));
    }

    if let Some(id) = kind.strip_prefix("foreign-key:") {
        let id = id.parse::<i64>().map_err(|_| {
            ServerError::new(StatusCode::NOT_FOUND, "Foreign-key constraint is no longer valid")
        })?;
        let mut foreign_keys = sqlite_foreign_key_rows(conn, table_name)?
            .into_iter()
            .filter(|foreign_key| foreign_key.id == id)
            .collect::<Vec<_>>();
        foreign_keys.sort_by_key(|foreign_key| foreign_key.sequence);
        if foreign_keys.is_empty() {
            return Err(ServerError::new(
                StatusCode::NOT_FOUND,
                "Foreign-key constraint no longer exists",
            ));
        }
        let local_columns = foreign_keys
            .iter()
            .map(|foreign_key| foreign_key.key.column_name.clone())
            .collect::<Vec<_>>();
        let referenced_table = foreign_keys[0].key.referenced_table_name.clone();
        let referenced_columns = foreign_keys
            .iter()
            .map(|foreign_key| foreign_key.key.referenced_column_name.clone())
            .filter(|column| !column.is_empty())
            .collect::<Vec<_>>();
        let on_update = foreign_keys[0].key.on_update.clone().unwrap_or_default();
        let on_delete = foreign_keys[0].key.on_delete.clone().unwrap_or_default();
        let mut definition = format!(
            "FOREIGN KEY ({}) REFERENCES {}",
            local_columns
                .iter()
                .map(|column| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", "),
            quote_identifier(&referenced_table),
        );
        if referenced_columns.len() == local_columns.len() {
            definition.push_str(&format!(
                " ({})",
                referenced_columns
                    .iter()
                    .map(|column| quote_identifier(column))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !on_update.trim().is_empty() {
            definition.push_str(&format!(" ON UPDATE {on_update}"));
        }
        if !on_delete.trim().is_empty() {
            definition.push_str(&format!(" ON DELETE {on_delete}"));
        }
        let keys = foreign_keys
            .into_iter()
            .map(|foreign_key| foreign_key.key)
            .collect::<Vec<_>>();
        return Ok(object(
            "foreign-key",
            sqlite_table_columns_for_names(&table.columns, &local_columns),
            keys,
            definition,
            serde_json::json!({
                "constraintId": id,
                "columns": local_columns,
                "referencedTable": referenced_table,
                "referencedColumns": referenced_columns,
                "onUpdate": on_update,
                "onDelete": on_delete,
            }),
        ));
    }

    if let Some(index_name) = kind.strip_prefix("unique:") {
        let index = sqlite_unique_indexes(conn, table_name)?
            .into_iter()
            .find(|index| index.name == index_name && index.unique && index.origin == "u")
            .ok_or_else(|| {
                ServerError::new(StatusCode::NOT_FOUND, "Unique constraint no longer exists")
            })?;
        let columns = index.columns.clone();
        return Ok(object(
            "unique",
            sqlite_table_columns_for_names(&table.columns, &columns),
            Vec::new(),
            format!(
                "UNIQUE ({})",
                columns
                    .iter()
                    .map(|column| quote_identifier(column))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            serde_json::json!({
                "indexName": index.name,
                "columns": columns,
                "partial": index.partial,
            }),
        ));
    }

    if let Some(position) = kind.strip_prefix("check:") {
        let position = position.parse::<usize>().map_err(|_| {
            ServerError::new(StatusCode::NOT_FOUND, "Check constraint is no longer valid")
        })?;
        let checks = sqlite_check_constraints(conn, table_name)?;
        let check = position
            .checked_sub(1)
            .and_then(|position| checks.get(position).cloned())
            .ok_or_else(|| {
                ServerError::new(StatusCode::NOT_FOUND, "Check constraint no longer exists")
            })?;
        return Ok(object(
            "check",
            Vec::new(),
            Vec::new(),
            check.definition,
            serde_json::json!({
                "expression": check.expression,
                "ordinal": position,
            }),
        ));
    }

    Err(ServerError::new(
        StatusCode::NOT_FOUND,
        "Unsupported or stale SQLite constraint identity",
    ))
}

fn sqlite_table_columns_for_names(
    table_columns: &[DatabaseObjectColumn],
    names: &[String],
) -> Vec<DatabaseObjectColumn> {
    names
        .iter()
        .filter_map(|name| {
            table_columns
                .iter()
                .find(|column| column.name == *name)
                .cloned()
        })
        .collect()
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

const SQLITE_SINGLE_STATEMENT_ERROR: &str = "SQLite Run Query executes one SQL statement at a time. Split a script into separate Run Query actions.";

/// `rusqlite::Connection::prepare` deliberately prepares only the first SQL
/// statement in its input. Without this guard a request such as `CREATE ...;
/// INSERT ...;` reports success after applying only `CREATE`, which is both
/// surprising and unsafe for the database workspace.
///
/// Ask SQLite where a statement is complete instead of trying to split on
/// semicolons ourselves: quoted strings, comments, and trigger bodies may
/// contain semicolons that are not statement boundaries.
fn ensure_sqlite_single_statement(sql: &str) -> Result<()> {
    for (index, byte) in sql.bytes().enumerate() {
        if byte != b';' {
            continue;
        }
        let prefix = std::ffi::CString::new(&sql[..=index]).map_err(|_| {
            ServerError::new(StatusCode::BAD_REQUEST, "SQL must not contain a NUL byte")
        })?;
        // SAFETY: `CString` gives SQLite a valid, NUL-terminated SQL buffer
        // for this read-only completeness check.
        let complete = unsafe { rusqlite::ffi::sqlite3_complete(prefix.as_ptr()) != 0 };
        if !complete {
            continue;
        }
        if sqlite_tail_has_statement(&sql[index + 1..]) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                SQLITE_SINGLE_STATEMENT_ERROR,
            ));
        }
        return Ok(());
    }
    Ok(())
}

/// A completed statement may be followed by whitespace, empty semicolons, or
/// SQL comments. Any other token starts a second statement and must be
/// rejected before the first statement can mutate the database.
fn sqlite_tail_has_statement(tail: &str) -> bool {
    let bytes = tail.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            byte if byte.is_ascii_whitespace() || byte == b';' => index += 1,
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                if index + 1 >= bytes.len() {
                    // Preserve SQLite's own syntax error for an unterminated
                    // trailing comment instead of accepting a partial script.
                    return true;
                }
                index += 2;
            }
            _ => return true,
        }
    }
    false
}

fn execute_sqlite_query(
    connection: &StoredDatabaseConnection,
    sql: &str,
    max_rows: usize,
) -> Result<DatabaseQueryResult> {
    ensure_sqlite_single_statement(sql)?;
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
    read_sqlite_table_data_with_options(
        connection,
        table_name,
        limit,
        offset,
        include_total_count,
        &DatabaseTableQueryOptions::default(),
    )
}

fn read_sqlite_table_data_with_options(
    connection: &StoredDatabaseConnection,
    table_name: &str,
    limit: usize,
    offset: usize,
    include_total_count: bool,
    options: &DatabaseTableQueryOptions,
) -> Result<DatabaseTableData> {
    let start = Instant::now();
    let conn = sqlite_connection(connection)?;
    let table = describe_sqlite_table_inner(&conn, table_name, DatabaseObjectType::Table)
        .or_else(|_| describe_sqlite_table_inner(&conn, table_name, DatabaseObjectType::View))?;
    let (where_clause, order_clause) =
        database_table_query_clauses(SupportedDatabaseType::Sqlite, &table.columns, options)?;
    let table_ref = quote_identifier(table_name);
    let total_row_count = if include_total_count {
        let count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table_ref}{where_clause}"),
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_server_error)?;
        Some(count.max(0) as usize)
    } else {
        None
    };
    let sql = format!("SELECT * FROM {table_ref}{where_clause}{order_clause} LIMIT ?1 OFFSET ?2");
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

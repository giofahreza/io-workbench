async fn mysql_integrity_fixture(pool: &MySqlPool, table: &str, temporary: bool) {
    let table_ref = quote_mysql_identifier(table);
    let temporary = if temporary { "TEMPORARY " } else { "" };
    sqlx::raw_sql(&format!(
        "CREATE {temporary}TABLE {table_ref} (
         row_key VARCHAR(30) PRIMARY KEY, text_value TEXT, bytes_value BLOB,
         variable_bytes VARBINARY(50), fixed_bytes BINARY(4), doc JSON)
         CHARACTER SET utf8mb4 COLLATE utf8mb4_bin"
    ))
    .execute(pool)
    .await
    .expect("create new MySQL integrity fixture");
    for (key, text, bytes, document) in [
        (
            "scalar",
            Some("true"),
            Some(b"true".as_slice()),
            Some("null"),
        ),
        (
            "object",
            Some("{ \"b\": 2, \"a\": 1 }"),
            Some(b"{ \"b\": 2, \"a\": 1 }".as_slice()),
            Some("{\"ok\":true,\"items\":[1,\"1\",null]}"),
        ),
        ("empty", Some(""), Some(b"".as_slice()), Some("\"\"")),
        ("null", None, None, None),
    ] {
        sqlx::query(&format!("INSERT INTO {table_ref} VALUES(?, ?, ?, ?, ?, ?)"))
            .bind(key)
            .bind(text)
            .bind(bytes)
            .bind(bytes)
            .bind(Some([0_u8, 255, 0, 1].as_slice()))
            .bind(document)
            .execute(pool)
            .await
            .expect("seed exact native values");
    }
}

#[tokio::test]
async fn mysql_binary_collation_text_and_json_looking_blobs_keep_native_kinds() {
    let Some(connection) = mysql_test_connection() else {
        return;
    };
    let url = connection_url_for_database(&connection, connection.profile.database_name.as_deref())
        .unwrap();
    let pool = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let name = new_id("iowb_mysql_value_kind").replace('-', "_");
    mysql_integrity_fixture(&pool, &name, true).await;
    let rows = sqlx::raw_sql(&format!(
        "SELECT * FROM {} ORDER BY row_key",
        quote_mysql_identifier(&name)
    ))
    .fetch_all(&pool)
    .await
    .unwrap();
    let wire_json = rows[0]
        .columns()
        .iter()
        .find(|column| column.name() == "doc")
        .unwrap()
        .type_info()
        .name()
        == "JSON";
    let columns = mysql_columns(rows[0].columns());
    for (name, expected) in [
        ("row_key", "VARCHAR"),
        ("text_value", "TEXT"),
        ("bytes_value", "BLOB"),
        ("variable_bytes", "VARBINARY"),
        ("fixed_bytes", "BINARY"),
    ] {
        assert_eq!(
            columns
                .iter()
                .find(|column| column.name == name)
                .unwrap()
                .native_type
                .as_deref(),
            Some(expected)
        );
    }
    for (row, (key, text, bytes, document)) in rows.iter().zip([
        ("empty", Some(""), Some(b"".as_slice()), Some("\"\"")),
        ("null", None, None, None),
        (
            "object",
            Some("{ \"b\": 2, \"a\": 1 }"),
            Some(b"{ \"b\": 2, \"a\": 1 }".as_slice()),
            Some("{\"ok\":true,\"items\":[1,\"1\",null]}"),
        ),
        (
            "scalar",
            Some("true"),
            Some(b"true".as_slice()),
            Some("null"),
        ),
    ]) {
        let decoded = mysql_row_to_json_map(row);
        assert_eq!(
            decoded["row_key"],
            Value::String(key.into()),
            "binary collation is still text"
        );
        assert_eq!(
            decoded["text_value"],
            text.map(|text| Value::String(text.into()))
                .unwrap_or(Value::Null),
            "never guess JSON from ordinary TEXT content"
        );
        let expected_bytes = bytes.map(database_buffer_value).unwrap_or(Value::Null);
        assert_eq!(
            decoded["bytes_value"], expected_bytes,
            "a BLOB containing JSON keeps its exact bytes"
        );
        assert_eq!(decoded["variable_bytes"], expected_bytes);
        assert_eq!(
            decoded["fixed_bytes"],
            database_buffer_value(&[0, 255, 0, 1])
        );
        let expected_json = document
            .map(|text| {
                if wire_json {
                    database_json_wire_value(serde_json::from_str(text).unwrap())
                } else {
                    Value::String(text.into())
                }
            })
            .unwrap_or(Value::Null);
        assert_eq!(
            decoded["doc"], expected_json,
            "native JSON and MariaDB's textual alias retain their distinct wire provenance"
        );
    }
    pool.close().await; // The temporary fixture is scoped to this connection.
}

#[tokio::test]
async fn mysql_table_transfer_preserves_binary_collation_text_and_json_looking_bytes() {
    let Some(connection) = mysql_test_connection() else {
        return;
    };
    let database_name = connection.profile.database_name.clone().unwrap();
    let url = connection_url_for_database(&connection, Some(&database_name)).unwrap();
    let pool = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let source_name = new_id("iowb_typed_src").replace('-', "_");
    let target_name = new_id("iowb_typed_dst").replace('-', "_");
    mysql_integrity_fixture(&pool, &source_name, false).await;
    sqlx::raw_sql(&format!(
        "CREATE TABLE {} LIKE {}",
        quote_mysql_identifier(&target_name),
        quote_mysql_identifier(&source_name)
    ))
    .execute(&pool)
    .await
    .unwrap();
    let endpoint = |table: &str| DatabaseTransferEndpoint {
        connection_id: connection.profile.id,
        connection_name: None,
        connection_type: Some(connection.profile.db_type),
        database_name: Some(database_name.clone()),
        schema_name: None,
        table_name: table.into(),
    };
    let mut job = test_transfer_job(
        endpoint(&source_name),
        endpoint(&target_name),
        DatabaseTransferMode::TableCopy,
    );
    let result = run_database_transfer(&connection, &connection, &mut job, &mut |_| {}).await;
    let native_sql = |table: &str| {
        format!(
            "SELECT row_key,text_value,HEX(bytes_value),HEX(variable_bytes),HEX(fixed_bytes),CAST(doc AS CHAR) FROM {} ORDER BY row_key",
            quote_mysql_identifier(table),
        )
    };
    type NativeRow = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let source = sqlx::query_as::<_, NativeRow>(&native_sql(&source_name))
        .fetch_all(&pool)
        .await;
    let target = sqlx::query_as::<_, NativeRow>(&native_sql(&target_name))
        .fetch_all(&pool)
        .await;
    sqlx::raw_sql(&format!(
        "DROP TABLE {}; DROP TABLE {}",
        quote_mysql_identifier(&target_name),
        quote_mysql_identifier(&source_name)
    ))
    .execute(&pool)
    .await
    .unwrap();
    let result = result.expect("the real transfer succeeds");
    assert_eq!(result.copied_row_count, 4);
    assert_eq!(
        result.failed_row_count, 0,
        "valid JSON-looking bytes cannot become invalid buffer markers"
    );
    assert_eq!(result.mapped_column_count, 6);
    assert!(result.row_failures.is_empty());
    assert_eq!(
        source.unwrap(),
        target.unwrap(),
        "independent native bytes, text, JSON kinds and NULLs match exactly"
    );
}

#[test]
fn transfer_boolean_conversion_only_accepts_exact_zero_and_one_for_scalar_boolean() {
    let boolean: DatabaseObjectColumn = serde_json::from_value(serde_json::json!({
        "name":"value", "dataType":"boolean", "nativeType":"bool"
    }))
    .unwrap();
    assert_eq!(
        transfer_value_for_target_column(&boolean, Value::from(0)),
        Value::Bool(false)
    );
    assert_eq!(
        transfer_value_for_target_column(&boolean, Value::from(1)),
        Value::Bool(true)
    );
    for unchanged in [
        Value::Null,
        Value::Bool(false),
        Value::Bool(true),
        Value::from(2),
        Value::from(-1),
        serde_json::json!(0.5),
        serde_json::json!([0, 1]),
        Value::String("1".into()),
    ] {
        assert_eq!(
            transfer_value_for_target_column(&boolean, unchanged.clone()),
            unchanged
        );
    }
    for kind in [
        "integer",
        "text",
        "tinyint(1)",
        "_bool",
        "boolean[]",
        "jsonb",
    ] {
        let column: DatabaseObjectColumn =
            serde_json::from_value(serde_json::json!({"name":"value", "nativeType":kind})).unwrap();
        assert_eq!(
            transfer_value_for_target_column(&column, Value::from(1)),
            Value::from(1)
        );
    }
}

#[test]
fn transfer_json_to_text_preserves_document_kinds_without_guessing_or_double_encoding() {
    let column = |kind: &str| {
        serde_json::from_value::<DatabaseObjectColumn>(
            serde_json::json!({"name":"value","nativeType":kind}),
        )
        .unwrap()
    };
    let json = column("json");
    let text = column("text");
    for value in [
        Value::Bool(true),
        Value::Bool(false),
        Value::from(3),
        serde_json::json!([true, "1", null]),
        serde_json::json!({"n":9007199254740993_u64}),
    ] {
        assert_eq!(
            transfer_value_for_column_mapping(Some(&json), &text, value.clone()),
            Value::String(value.to_string())
        );
    }
    for value in [
        Value::Null,
        Value::String("null".into()),
        Value::String("\"true\"".into()),
        Value::String("{ \"n\": 1 }".into()),
    ] {
        assert_eq!(
            transfer_value_for_column_mapping(Some(&json), &text, value.clone()),
            value
        );
    }
    // An ordinary Boolean copied into text retains the engine's established
    // conversion. JSON metadata, not lookalike content or target alone, gates it.
    assert_eq!(
        transfer_value_for_column_mapping(Some(&column("boolean")), &text, Value::Bool(true)),
        Value::Bool(true)
    );
    assert_eq!(
        transfer_value_for_column_mapping(None, &text, Value::Bool(true)),
        Value::Bool(true)
    );
    assert_eq!(
        transfer_value_for_column_mapping(Some(&json), &json, Value::Bool(true)),
        Value::Bool(true)
    );
}

#[tokio::test]
async fn mysql_json_table_transfer_to_sqlite_text_preserves_json_kinds_and_sql_null() {
    let Some(source_connection) = mysql_test_connection() else {
        return;
    };
    let database_name = source_connection.profile.database_name.clone().unwrap();
    let url = connection_url_for_database(&source_connection, Some(&database_name)).unwrap();
    let pool = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let source_name = new_id("iowb_json_to_text").replace('-', "_");
    let source_ref = quote_mysql_identifier(&source_name);
    sqlx::raw_sql(&format!("CREATE TABLE {source_ref}(id VARCHAR(30) PRIMARY KEY,payload JSON) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin"))
        .execute(&pool).await.unwrap();
    let documents = [
        Some("true"),
        Some("false"),
        Some("null"),
        Some("\"true\""),
        Some("1.25"),
        Some("[true,\"1\",null]"),
        Some("{\"n\":9007199254740993}"),
        None,
    ];
    for (index, document) in documents.iter().enumerate() {
        sqlx::query(&format!("INSERT INTO {source_ref} VALUES(?,?)"))
            .bind(index.to_string())
            .bind(*document)
            .execute(&pool)
            .await
            .unwrap();
    }
    let path = env::temp_dir().join(format!("{}.sqlite", new_id("iowb_json_text_target")));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE records(id TEXT PRIMARY KEY,payload TEXT)")
        .unwrap();
    let target_connection = sqlite_test_connection(&path);
    let source = DatabaseTransferEndpoint {
        connection_id: source_connection.profile.id,
        connection_name: None,
        connection_type: Some(source_connection.profile.db_type),
        database_name: Some(database_name),
        schema_name: None,
        table_name: source_name.clone(),
    };
    let target = DatabaseTransferEndpoint {
        connection_id: target_connection.profile.id,
        connection_name: None,
        connection_type: Some(SupportedDatabaseType::Sqlite),
        database_name: Some("main".into()),
        schema_name: None,
        table_name: "records".into(),
    };
    let mut job = test_transfer_job(source, target, DatabaseTransferMode::TableCopy);
    let result = run_database_transfer(
        &source_connection,
        &target_connection,
        &mut job,
        &mut |_| {},
    )
    .await;
    let native_rows = {
        let native = Connection::open(&path).unwrap();
        let mut query = native
            .prepare("SELECT id,payload,typeof(id),typeof(payload) FROM records ORDER BY id")
            .unwrap();
        query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    };
    sqlx::raw_sql(&format!("DROP TABLE {source_ref}"))
        .execute(&pool)
        .await
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    let result = result.unwrap();
    assert_eq!(result.copied_row_count, 8);
    assert_eq!(result.failed_row_count, 0);
    assert_eq!(native_rows.len(), 8);
    for ((id, actual, key_kind, payload_kind), (index, expected)) in native_rows
        .into_iter()
        .zip(documents.into_iter().enumerate())
    {
        assert_eq!(id, index.to_string());
        assert_eq!(key_kind, "text");
        assert_eq!(
            payload_kind,
            if expected.is_some() { "text" } else { "null" }
        );
        assert_eq!(
            actual.map(|text| serde_json::from_str::<Value>(&text).unwrap()),
            expected.map(|text| serde_json::from_str::<Value>(text).unwrap())
        );
    }
}

#[tokio::test]
async fn postgres_transfer_from_sqlite_integer_booleans_preserves_zero_one_null_and_rejects_two() {
    let Some(target_connection) = postgres_test_connection() else {
        return;
    };
    let database_name = target_connection.profile.database_name.clone().unwrap();
    let url = connection_url_for_database(&target_connection, Some(&database_name)).unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let path = env::temp_dir().join(format!("{}.sqlite", new_id("iowb_boolean_source")));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    {
        let sqlite = Connection::open(&path).unwrap();
        sqlite
            .execute_batch(
                "CREATE TABLE records (row_key TEXT PRIMARY KEY, value INTEGER);\
            INSERT INTO records VALUES ('zero',0),('one',1),('null',NULL),('invalid',2)",
            )
            .unwrap();
    }
    let source_connection = sqlite_test_connection(&path);
    let target_name = new_id("iowb_boolean_target").replace('-', "_");
    sqlx::raw_sql(&format!(
        "CREATE TABLE {} (row_key TEXT PRIMARY KEY, value BOOLEAN)",
        quote_identifier(&target_name)
    ))
    .execute(&pool)
    .await
    .unwrap();
    let source = DatabaseTransferEndpoint {
        connection_id: source_connection.profile.id,
        connection_name: None,
        connection_type: Some(SupportedDatabaseType::Sqlite),
        database_name: Some("main".into()),
        schema_name: None,
        table_name: "records".into(),
    };
    let target = DatabaseTransferEndpoint {
        connection_id: target_connection.profile.id,
        connection_name: None,
        connection_type: Some(SupportedDatabaseType::Postgresql),
        database_name: Some(database_name),
        schema_name: Some("public".into()),
        table_name: target_name.clone(),
    };
    let mut job = test_transfer_job(source, target, DatabaseTransferMode::TableCopy);
    let result = run_database_transfer(
        &source_connection,
        &target_connection,
        &mut job,
        &mut |_| {},
    )
    .await;
    let rows = sqlx::query_as::<_, (String, Option<bool>)>(&format!(
        "SELECT row_key,value FROM {} ORDER BY row_key",
        quote_identifier(&target_name)
    ))
    .fetch_all(&pool)
    .await;
    sqlx::raw_sql(&format!("DROP TABLE {}", quote_identifier(&target_name)))
        .execute(&pool)
        .await
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    let result = result.unwrap();
    assert_eq!(result.copied_row_count, 3);
    assert_eq!(
        result.failed_row_count, 1,
        "numeric2 cannot be silently converted to true"
    );
    assert_eq!(result.row_failures[0]["rowIndex"], Value::from(3));
    assert_eq!(
        rows.unwrap(),
        vec![
            ("null".into(), None),
            ("one".into(), Some(true)),
            ("zero".into(), Some(false))
        ]
    );
}

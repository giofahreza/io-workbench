#[tokio::test]
async fn sqlite_suppressed_inserts_are_not_reported_as_copied_rows() {
    let path = env::temp_dir().join(format!("{}.sqlite", new_id("suppressed-insert-test")));
    let sqlite = Connection::open(&path).unwrap();
    sqlite.execute_batch("CREATE TABLE records (id INTEGER PRIMARY KEY, title TEXT); \
        CREATE TRIGGER suppress_record BEFORE INSERT ON records WHEN NEW.id = 2 \
        BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    let connection = sqlite_test_connection(&path);
    let scope = DatabaseTableScopeRequest {
        database_name: None, schema_name: None, table_name: "records".into(),
    };
    let metadata = database_table_metadata(&connection, None, None, "records").await.unwrap();
    let suppressed = serde_json::json!({"id":2,"title":"suppressed"}).as_object().unwrap().clone();
    let error = insert_database_row(&connection, None, None, "records", &metadata, &suppressed)
        .await.expect_err("direct paste must not count a suppressed insert as success");
    assert!(database_error_message(&error).contains("did not insert"));
    let endpoint = database_scope_endpoint(&connection, &scope);
    assert_eq!(insert_transfer_rows(&connection, &endpoint, &metadata.columns, &[]).await.unwrap(), 0);
    assert_eq!(insert_transfer_rows(&connection, &endpoint, &metadata.columns, &[suppressed.clone(), suppressed.clone()]).await.unwrap(), 0);
    assert_eq!(sqlite.query_row("SELECT count(*) FROM records", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    let rows = [
        serde_json::json!({"id":1,"title":"one"}).as_object().unwrap().clone(),
        suppressed,
        serde_json::json!({"id":3,"title":"three"}).as_object().unwrap().clone(),
    ];
    let copied = insert_transfer_rows(&connection, &database_scope_endpoint(&connection, &scope), &metadata.columns, &rows).await.unwrap();
    assert_eq!(copied, 2);
    assert_eq!(sqlite.query_row("SELECT count(*) FROM records", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
    drop(sqlite);
    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn postgres_suppressed_inserts_are_not_reported_as_copied_rows() {
    let Some(connection) = postgres_test_connection() else { return; };
    let url = connection_url_for_database(&connection, connection.profile.database_name.as_deref()).unwrap();
    let pool = PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
    let table = new_id("suppressed_insert").replace('-', "_");
    let function = format!("{table}_trigger");
    let table_ref = quote_identifier(&table);
    let function_ref = quote_identifier(&function);
    sqlx::raw_sql(&format!("CREATE TABLE {table_ref} (id integer PRIMARY KEY, title text); \
        CREATE FUNCTION {function_ref}() RETURNS trigger LANGUAGE plpgsql AS $$ \
        BEGIN IF NEW.id = 2 THEN RETURN NULL; END IF; RETURN NEW; END $$; \
        CREATE TRIGGER suppress BEFORE INSERT ON {table_ref} FOR EACH ROW EXECUTE FUNCTION {function_ref}();"))
        .execute(&pool).await.unwrap();
    let scope = DatabaseTableScopeRequest {
        database_name: connection.profile.database_name.clone(), schema_name: Some("public".into()), table_name: table,
    };
    let metadata = database_table_metadata(&connection, scope.database_name.as_deref(), Some("public"), &scope.table_name).await.unwrap();
    let suppressed = serde_json::json!({"id":2,"title":"suppressed"}).as_object().unwrap().clone();
    let direct = insert_database_row(&connection, scope.database_name.as_deref(), Some("public"), &scope.table_name, &metadata, &suppressed).await;
    let endpoint = database_scope_endpoint(&connection, &scope);
    let empty = insert_transfer_rows(&connection, &endpoint, &metadata.columns, &[]).await;
    let all_suppressed = insert_transfer_rows(&connection, &endpoint, &metadata.columns, &[suppressed.clone(), suppressed.clone()]).await;
    let rows = [
        serde_json::json!({"id":1,"title":"one"}).as_object().unwrap().clone(),
        suppressed,
        serde_json::json!({"id":3,"title":"three"}).as_object().unwrap().clone(),
    ];
    let copied = insert_transfer_rows(&connection, &database_scope_endpoint(&connection, &scope), &metadata.columns, &rows).await;
    let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table_ref}")).fetch_one(&pool).await.unwrap();
    sqlx::raw_sql(&format!("DROP TABLE {table_ref}; DROP FUNCTION {function_ref}();")).execute(&pool).await.unwrap();
    assert!(database_error_message(&direct.expect_err("suppressed insert must fail")).contains("did not insert"));
    assert_eq!(empty.unwrap(), 0);
    assert_eq!(all_suppressed.unwrap(), 0);
    assert_eq!(copied.unwrap(), 2);
    assert_eq!(count, 2);
}

#[test]
fn database_mutation_nul_guard_preserves_other_engines_and_encoded_bytes() {
    let private_value = "private-prefix\0private-suffix";
    let postgres_literal = database_text_literal(SupportedDatabaseType::Postgresql, private_value);
    let error = validate_database_mutation_text(SupportedDatabaseType::Postgresql, &postgres_literal)
        .expect_err("PostgreSQL text must reject raw NUL before sending SQL");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert!(error.body.error.contains("U+0000 (NUL)"));
    assert!(!error.body.error.contains("private"));
    for db_type in [SupportedDatabaseType::Sqlite, SupportedDatabaseType::Mysql, SupportedDatabaseType::Mariadb] {
        assert!(validate_database_mutation_text(db_type, private_value).is_ok());
        let literal = database_text_literal(db_type, private_value);
        assert!(!literal.contains('\0'));
        assert!(validate_database_mutation_text(db_type, &literal).is_ok());
    }
    let binary = transfer_value_literal(SupportedDatabaseType::Postgresql, &database_buffer_value(&[0, 1, 0, 255]));
    assert!(validate_database_mutation_text(SupportedDatabaseType::Postgresql, &binary).is_ok());
    let escaped_json = database_text_literal(SupportedDatabaseType::Postgresql, r#"{"text":"\u0000"}"#);
    assert!(validate_database_mutation_text(SupportedDatabaseType::Postgresql, &escaped_json).is_ok());
    let sqlite = Connection::open_in_memory().unwrap();
    let sql = format!("SELECT {}", database_text_literal(SupportedDatabaseType::Sqlite, private_value));
    let actual: String = sqlite.query_row(&sql, [], |row| row.get(0)).unwrap();
    assert_eq!(actual, private_value);
}

#[tokio::test]
async fn postgres_query_metadata_names_types_for_populated_and_empty_results() {
    let Some(connection) = postgres_test_connection() else { return; };
    let url = connection_url_for_database(&connection, connection.profile.database_name.as_deref()).unwrap();
    let pool = PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
    let enum_name = new_id("iowb_query_enum").replace('-', "_");
    let enum_ref = quote_identifier(&enum_name);
    sqlx::raw_sql(&format!("CREATE TYPE {enum_ref} AS ENUM ('ready')")).execute(&pool).await.unwrap();
    let select = format!("SELECT decode('00ff','hex') AS bytes, ARRAY[1::bigint] AS numbers, \
        point(1.5,-2.5) AS location, '{{\"type\":\"buffer\"}}'::jsonb AS document, 'ready'::{enum_ref} AS state");
    let populated = execute_postgres_query(&pool, &select, 10, Some("public")).await;
    let empty = execute_postgres_query(&pool, &format!("{select} WHERE FALSE"), 10, Some("public")).await;
    sqlx::raw_sql(&format!("DROP TYPE {enum_ref}")).execute(&pool).await.unwrap();
    for result in [populated.as_ref().unwrap(), empty.as_ref().unwrap()] {
        for (name, native_type) in [("bytes", "BYTEA"), ("numbers", "INT8[]"), ("location", "POINT"), ("document", "JSONB"), ("state", enum_name.as_str())] {
            let column = result.columns.iter().find(|column| column.name == name).unwrap();
            assert_eq!(column.native_type.as_deref(), Some(native_type), "{name}");
        }
    }
    let populated = populated.unwrap();
    assert_eq!(populated.rows.len(), 1);
    assert_eq!(populated.rows[0]["bytes"], database_buffer_value(&[0, 255]));
    assert_eq!(populated.rows[0]["state"], "ready");
    assert!(empty.unwrap().rows.is_empty());
}

#[tokio::test]
async fn postgres_mutation_nul_rejection_preserves_row_and_allows_binary_zero_bytes() {
    let Some(connection) = postgres_test_connection() else { return; };
    let database_name = connection.profile.database_name.clone().unwrap_or_else(|| "postgres".into());
    let url = connection_url_for_database(&connection, Some(&database_name)).unwrap();
    let pool = PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
    let table_name = new_id("iowb_nul_guard").replace('-', "_");
    let table_ref = quote_identifier(&table_name);
    sqlx::raw_sql(&format!(
        "CREATE TABLE {table_ref} (id integer PRIMARY KEY, title text, labels text[], bytes bytea); \
         INSERT INTO {table_ref} VALUES (1, 'unchanged', ARRAY['original'], decode('01', 'hex'))"
    )).execute(&pool).await.unwrap();
    let scope = DatabaseTableScopeRequest {
        database_name: Some(database_name), schema_name: Some("public".into()), table_name,
    };
    let keys = serde_json::json!({"id": 1}).as_object().unwrap().clone();
    let original = select_database_row(&connection, &scope, &keys).await.unwrap().unwrap();
    let mut rejections = Vec::new();
    for updates in [serde_json::json!({"title":"private\u{0000}value"}), serde_json::json!({"labels":["safe","bad\u{0000}value"]})] {
        rejections.push(update_database_row(&connection, &scope, &keys, updates.as_object().unwrap(), &original).await);
    }
    let unchanged = select_database_row(&connection, &scope, &keys).await.unwrap().unwrap();
    let bytes = database_buffer_value(&[0, 1, 0, 255]);
    let updates = Map::from_iter([("bytes".to_string(), bytes.clone())]);
    let binary_result = update_database_row(&connection, &scope, &keys, &updates, &original).await;
    let metadata = database_table_metadata(&connection, scope.database_name.as_deref(), Some("public"), &scope.table_name).await.unwrap();
    let transfer_row = serde_json::json!({"id":2,"title":"bad\u{0000}value"}).as_object().unwrap().clone();
    let transfer_result = insert_transfer_rows(&connection, &database_scope_endpoint(&connection, &scope), &metadata.columns, &[transfer_row]).await;
    let row_count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table_ref}")).fetch_one(&pool).await.unwrap();
    sqlx::raw_sql(&format!("DROP TABLE {table_ref}")).execute(&pool).await.unwrap();
    for result in rejections {
        let error = result.expect_err("raw NUL must be rejected");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.body.error.contains("U+0000 (NUL)"));
    }
    assert_eq!(unchanged, original);
    let binary_row = binary_result.expect("encoded binary zero bytes remain supported").unwrap();
    assert_eq!(binary_row["bytes"], bytes);
    assert_eq!(binary_row["title"], "unchanged");
    assert_eq!(transfer_result.expect_err("transfer must reject raw NUL too").status, StatusCode::BAD_REQUEST);
    assert_eq!(row_count, 1);
}

#[test]
fn database_row_binary_markers_reject_invalid_encoding_before_mutation() {
    let metadata = DatabaseTableMetadata {
        columns: serde_json::from_value(serde_json::json!([
            {"name":"v", "dataType":"blob"},
            {"name":"text", "dataType":"text"},
            {"name":"j", "dataType":"jsonb"}
        ])).unwrap(),
        primary_key: Vec::new(),
    };
    for value in [
        serde_json::json!({"type":"buffer","encoding":"base64","value":"%%%"}),
        serde_json::json!({"type":"buffer","encoding":"hex","value":"00ff"}),
        serde_json::json!({"type":"buffer","encoding":"base64","value":"'); SELECT 1; --"}),
    ] {
        let values = Map::from_iter([("v".to_string(), value)]);
        assert!(canonical_column_values(&metadata, &values, true).is_err());
        assert!(canonical_column_values(&metadata, &values, false).is_err());
    }
    for value in [Value::Null, database_buffer_value(&[]), database_buffer_value(&[0,1,255])] {
        assert!(canonical_column_values(&metadata, &Map::from_iter([("v".to_string(), value)]), true).is_ok());
    }
    let ordinary_json = serde_json::json!({"type":"buffer","data":"ordinary domain data"});
    assert!(canonical_column_values(&metadata, &Map::from_iter([("j".to_string(),ordinary_json)]), true).is_ok());
    let invalid_marker_in_text = serde_json::json!({"type":"buffer","encoding":"base64","value":"%%%"});
    assert!(canonical_column_values(&metadata, &Map::from_iter([("text".to_string(),invalid_marker_in_text)]), true).is_err());
}

#[test]
fn database_row_json_scalar_literals_are_json_text() {
    let metadata = DatabaseTableMetadata {
        columns: serde_json::from_value(serde_json::json!([
            {"name":"v", "dataType":"jsonb"}
        ])).unwrap(),
        primary_key: Vec::new(),
    };
    for db_type in [SupportedDatabaseType::Postgresql, SupportedDatabaseType::Mysql, SupportedDatabaseType::Mariadb, SupportedDatabaseType::Sqlite] {
        for (value, expected) in [
            (serde_json::json!(true), "true"),
            (serde_json::json!(42), "42"),
            (serde_json::json!("hello"), "\"hello\""),
            (serde_json::json!("\"hello\""), "\"hello\""),
            (serde_json::json!("null"), "null"),
        ] {
            assert_eq!(database_row_value_literal(db_type, &metadata, "v", &value), database_text_literal(db_type, expected));
        }
        assert_eq!(database_row_value_literal(db_type, &metadata, "v", &Value::Null), "NULL");
    }
}

#[test]
fn database_native_json_paste_preserves_scalar_strings_and_internal_nulls() {
    let metadata = DatabaseTableMetadata {
        columns: serde_json::from_value(serde_json::json!([
            {"name":"v", "dataType":"jsonb"}
        ])).unwrap(),
        primary_key: Vec::new(),
    };
    for source in ["true", "42", "null", "hello", "\"already quoted\""] {
        let value = serde_json::json!(source);
        let prepared = database_paste_value(&metadata, "v", value.clone(), &DatabaseRowValueFormat::NativeJson);
        assert_eq!(database_row_value_literal(SupportedDatabaseType::Postgresql, &metadata, "v", &prepared), database_text_literal(SupportedDatabaseType::Postgresql, &value.to_string()));
        assert_eq!(database_json_wire_value(value.clone()), Value::String(value.to_string()));
    }
    assert_eq!(database_json_wire_value(Value::Null), serde_json::json!("null"));
    assert_eq!(database_paste_value(&metadata, "v", Value::Null, &DatabaseRowValueFormat::NativeJson), Value::Null);
    for format in [Value::Null, serde_json::json!("native-json"), serde_json::json!("encoded-text")] {
        assert!(serde_json::from_value::<DatabasePasteRequest>(serde_json::json!({"tableName":"x","valueFormat":format})).is_ok());
    }
    assert!(serde_json::from_value::<DatabasePasteRequest>(serde_json::json!({"tableName":"x","valueFormat":"ambiguous"})).is_err());
}
#[test]
fn database_native_scalar_roundtrip_values_preserve_precision_and_null_distinctions() {
    let decimal = "12345678901234567.12345678901234567890";
    assert_eq!(postgres_array_scalar_to_json(decimal, "NUMERIC"), serde_json::json!(decimal));
    assert_eq!(postgres_array_scalar_to_json("2024-02-29", "DATE"), serde_json::json!("2024-02-29"));
    assert_eq!(postgres_array_scalar_to_json("2024-02-29 00:00:00.123456", "TIMESTAMP"), serde_json::json!("2024-02-29T00:00:00.123456"));
    assert_eq!(json_float_value(f64::NAN), serde_json::json!("NaN"));
    assert_eq!(json_float_value(f64::INFINITY), serde_json::json!("Infinity"));
    assert_eq!(json_float_value(f64::NEG_INFINITY), serde_json::json!("-Infinity"));
    assert_eq!(json_float_value(1.25), serde_json::json!(1.25));
}
#[test]
fn database_mariadb_json_alias_check_matches_only_the_exact_column() {
    assert!(mariadb_check_is_json_column("json_valid(`payload`)", "payload"));
    assert!(mariadb_check_is_json_column("JSON_VALID(`has `` tick`)", "has ` tick"));
    assert!(!mariadb_check_is_json_column("json_valid(`other`)", "payload"));
    assert!(!mariadb_check_is_json_column("json_valid(`payload`) OR 1=1", "payload"));
    assert!(!mariadb_check_is_json_column("char_length(`payload`) < 10", "payload"));
}
#[test]
fn database_postgres_point_snapshot_uses_native_input_literal() {
    let metadata = DatabaseTableMetadata {
        columns: serde_json::from_value(serde_json::json!([{"name":"v","dataType":"point"}])).unwrap(),
        primary_key: Vec::new(),
    };
    assert_eq!(database_row_value_literal(SupportedDatabaseType::Postgresql, &metadata, "v", &serde_json::json!({"x":1.25,"y":-2.5})), database_text_literal(SupportedDatabaseType::Postgresql,"(1.25,-2.5)"));
}
#[test]
fn database_paste_raw_json_decimal_number_retains_all_digits() {
    let raw = r#"{"tableName":"numbers","sourceRows":[{"v":12345678901234567.12345678901234567890}]}"#;
    let request: DatabasePasteRequest = serde_json::from_str(raw).unwrap();
    let value = &request.source_rows[0]["v"];
    assert_eq!(value.to_string(), "12345678901234567.12345678901234567890");
    assert_eq!(transfer_value_literal(SupportedDatabaseType::Postgresql, value), "12345678901234567.12345678901234567890");
    let serialized = serde_json::to_string(value).unwrap();
    assert_eq!(serialized, "12345678901234567.12345678901234567890");
    for token in ["9223372036854775807", "18446744073709551615", "0", "-1"] {
        let number: Value = serde_json::from_str(token).unwrap();
        assert!(number.is_number());
        assert_eq!(number.to_string(), token);
    }
}

#[test]
fn database_postgres_json_arrays_keep_encoded_elements_and_sql_nulls() {
    let elements = serde_json::json!([
        "\"true\"", "\"123\"", "\"null\"", "\"\"", "null", null,
        "true", "12345678901234567890.12345678901234567890",
        "{\"quoted\":\"a,b\\\"c\\\\d\"}", "[1,null,\"two\"]"
    ]);
    for element_type in ["JSON", "JSONB"] {
        let native = postgres_array_input(&elements);
        assert_eq!(parse_postgres_array(&native, element_type), elements);
        let multidimensional = serde_json::json!([elements.clone(), elements.clone()]);
        assert_eq!(
            parse_postgres_array(&postgres_array_input(&multidimensional), element_type),
            multidimensional,
        );
        assert_eq!(parse_postgres_array("{}", element_type), serde_json::json!([]));
        assert_eq!(parse_postgres_array("{NULL}", element_type), serde_json::json!([null]));
    }
}

#[test]
fn database_postgres_json_array_editor_and_paste_literals_preserve_wire_values() {
    let wire = serde_json::json!(["\"true\"", "null", null, "[1,2]", "{\"ok\":true}"]);
    for native_type in ["_json", "_jsonb"] {
        let metadata = DatabaseTableMetadata {
            columns: serde_json::from_value(serde_json::json!([
                {"name":"v", "dataType":"ARRAY", "nativeType":native_type}
            ])).unwrap(),
            primary_key: Vec::new(),
        };
        let prepared = database_paste_value(
            &metadata, "v", wire.clone(), &DatabaseRowValueFormat::EncodedText,
        );
        assert_eq!(prepared, wire);
        assert_eq!(
            database_row_value_literal(SupportedDatabaseType::Postgresql, &metadata, "v", &prepared),
            database_text_literal(SupportedDatabaseType::Postgresql, &postgres_array_input(&wire)),
        );
    }
}

#[test]
fn database_postgres_binary_arrays_use_decoders_and_validate_every_element() {
    let metadata = DatabaseTableMetadata {
        columns: serde_json::from_value(serde_json::json!([
            {"name":"v", "dataType":"ARRAY", "nativeType":"_bytea"}
        ])).unwrap(),
        primary_key: Vec::new(),
    };
    let buffers = serde_json::json!([
        [database_buffer_value(&[0, 1, 2, 255]), Value::Null],
        [database_buffer_value(&[]), "\\x7b7d"]
    ]);
    assert!(canonical_column_values(&metadata, &Map::from_iter([("v".to_string(), buffers.clone())]), false).is_ok());
    let literal = database_row_value_literal(SupportedDatabaseType::Postgresql, &metadata, "v", &buffers);
    assert!(literal.starts_with("ARRAY[ARRAY[decode("));
    assert!(literal.contains("AAEC/w=="));
    assert!(literal.contains("NULL::bytea"));
    assert!(literal.ends_with("]::bytea[]]::bytea[]"));
    assert!(!literal.contains("\"encoding\""));
    assert_eq!(postgres_binary_array_literal(&serde_json::json!([])), "ARRAY[]::bytea[]");
    for invalid in [
        serde_json::json!({"type":"buffer","encoding":"base64","value":"%%%"}),
        serde_json::json!({"type":"buffer","encoding":"hex","value":"ff"}),
        serde_json::json!({"type":"buffer","encoding":"base64"}),
        serde_json::json!({"ordinary":"object"}),
        serde_json::json!(true),
    ] {
        for values in [serde_json::json!([invalid.clone()]), serde_json::json!([[database_buffer_value(&[]), invalid]])] {
            assert!(canonical_column_values(&metadata, &Map::from_iter([("v".to_string(), values)]), false).is_err());
        }
    }
}

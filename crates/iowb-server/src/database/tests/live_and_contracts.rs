    #[tokio::test]
    async fn postgres_live_queries_preserve_returning_rows_and_empty_columns() {
        let Some(connection) = postgres_test_connection() else {
            return;
        };
        let url =
            connection_url_for_database(&connection, connection.profile.database_name.as_deref())
                .expect("build live PostgreSQL URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live PostgreSQL");
        let table_name = new_id("iowb_database_parity").replace('-', "_");
        let table_ref = quote_identifier(&table_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE TEMP TABLE {table_ref} (
                id BIGSERIAL PRIMARY KEY,
                amount NUMERIC NOT NULL,
                payload JSONB NOT NULL,
                uid UUID NOT NULL,
                bytes BYTEA NOT NULL,
                numbers BIGINT[] NOT NULL,
                elapsed INTERVAL NOT NULL,
                location POINT NOT NULL,
                boundary CIRCLE NOT NULL
            )
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create PostgreSQL parity table");

        let insert = execute_postgres_query(
            &pool,
            &format!(
                r#"
                INSERT INTO {table_ref} (
                    amount, payload, uid, bytes, numbers, elapsed, location, boundary
                ) VALUES (
                    1234567890.123456789,
                    '{{"ok":true,"nested":[1,2]}}'::jsonb,
                    '123e4567-e89b-12d3-a456-426614174000'::uuid,
                    decode('AAEC/w==', 'base64'),
                    ARRAY[1::bigint, 9007199254740993::bigint],
                    '1 year 2 mons 3 days 04:05:06.789'::interval,
                    point(1.5, -2.25),
                    circle(point(1, 2), 3.5)
                )
                RETURNING id, amount, payload, uid, bytes, numbers, elapsed, location, boundary
                "#,
            ),
            1000,
            Some("public"),
        )
        .await
        .expect("execute INSERT RETURNING");
        assert_eq!(insert.statement_type, DatabaseQueryStatementType::Insert);
        assert_eq!(insert.row_count, 1);
        assert_eq!(insert.returned_row_count, 1);
        assert_eq!(insert.columns.len(), 9);
        let row = insert.rows.first().expect("returned PostgreSQL row");
        assert!(row.get("id").is_some_and(Value::is_string));
        assert_eq!(
            row.get("amount"),
            Some(&Value::String("1234567890.123456789".to_string())),
        );
        assert_eq!(
            row.get("payload"),
            Some(&serde_json::json!({"ok": true, "nested": [1, 2]}))
        );
        assert_eq!(
            row.get("uid"),
            Some(&Value::String(
                "123e4567-e89b-12d3-a456-426614174000".to_string(),
            )),
        );
        assert_eq!(
            row.get("bytes"),
            Some(&serde_json::json!({
                "type": "buffer",
                "encoding": "base64",
                "value": "AAEC/w==",
            })),
        );
        assert_eq!(
            row.get("numbers"),
            Some(&serde_json::json!(["1", "9007199254740993"]))
        );
        assert_eq!(
            row.get("elapsed"),
            Some(&serde_json::json!({
                "years": 1,
                "months": 2,
                "days": 3,
                "hours": 4,
                "minutes": 5,
                "seconds": 6,
                "milliseconds": 789,
            })),
        );
        assert_eq!(
            row.get("location"),
            Some(&serde_json::json!({"x": 1.5, "y": -2.25}))
        );
        assert_eq!(
            row.get("boundary"),
            Some(&serde_json::json!({"x": 1.0, "y": 2.0, "radius": 3.5})),
        );

        let empty = execute_postgres_query(
            &pool,
            &format!("SELECT amount, payload FROM {table_ref} WHERE FALSE"),
            1000,
            Some("public"),
        )
        .await
        .expect("execute empty PostgreSQL select");
        assert_eq!(empty.statement_type, DatabaseQueryStatementType::Select);
        assert_eq!(empty.row_count, 0);
        assert!(empty.rows.is_empty());
        assert_eq!(
            empty
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec!["amount", "payload"],
        );

        sqlx::raw_sql(&format!("DROP TABLE {table_ref}"))
            .execute(&pool)
            .await
            .expect("drop PostgreSQL parity table");
    }

    #[tokio::test]
    async fn mysql_live_queries_preserve_complex_values_empty_columns_and_meta() {
        let Some(connection) = mysql_test_connection() else {
            return;
        };
        let url =
            connection_url_for_database(&connection, connection.profile.database_name.as_deref())
                .expect("build live MySQL URL");
        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live MySQL");
        let table_name = new_id("iowb_database_parity").replace('-', "_");
        let table_ref = format!("`{}`", table_name.replace('`', "``"));
        sqlx::raw_sql(&format!(
            r#"
            CREATE TEMPORARY TABLE {table_ref} (
                id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
                amount DECIMAL(30, 9) NOT NULL,
                payload JSON NOT NULL,
                bytes BLOB NOT NULL,
                location POINT NOT NULL
            )
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create MySQL parity table");

        let insert = execute_mysql_query(
            &pool,
            &format!(
                r#"
                INSERT INTO {table_ref} (amount, payload, bytes, location)
                VALUES (
                    123456789012345678901.123456789,
                    JSON_OBJECT('ok', TRUE, 'nested', JSON_ARRAY(1, 2)),
                    FROM_BASE64('AAEC/w=='),
                    ST_GeomFromText('POINT(1.5 -2.25)')
                )
                "#,
            ),
            1000,
        )
        .await
        .expect("execute MySQL insert");
        assert_eq!(insert.statement_type, DatabaseQueryStatementType::Insert);
        assert_eq!(insert.row_count, 1);
        assert!(insert.rows.is_empty());
        let insert_meta = insert
            .meta
            .as_ref()
            .and_then(Value::as_object)
            .expect("MySQL insert metadata");
        assert_eq!(
            insert_meta.get("affectedRows").and_then(Value::as_u64),
            Some(1)
        );
        assert!(
            insert_meta
                .get("insertId")
                .and_then(Value::as_u64)
                .is_some_and(|value| value > 0)
        );
        assert_eq!(
            insert_meta.get("warningStatus").and_then(Value::as_u64),
            Some(0)
        );

        let select = execute_mysql_query(
            &pool,
            &format!("SELECT id, amount, payload, bytes, location FROM {table_ref}"),
            1000,
        )
        .await
        .expect("execute MySQL select");
        assert_eq!(select.statement_type, DatabaseQueryStatementType::Select);
        assert_eq!(select.row_count, 1);
        assert_eq!(select.returned_row_count, 1);
        let row = select.rows.first().expect("returned MySQL row");
        assert_eq!(row.get("id").and_then(Value::as_u64), Some(1));
        assert_eq!(
            row.get("amount"),
            Some(&Value::String(
                "123456789012345678901.123456789".to_string(),
            )),
        );
        assert_eq!(
            row.get("payload"),
            Some(&serde_json::json!({"ok": true, "nested": [1, 2]}))
        );
        assert_eq!(
            row.get("bytes"),
            Some(&serde_json::json!({
                "type": "buffer",
                "encoding": "base64",
                "value": "AAEC/w==",
            })),
        );
        assert_eq!(
            row.get("location"),
            Some(&serde_json::json!({"x": 1.5, "y": -2.25}))
        );

        let empty = execute_mysql_query(
            &pool,
            &format!("SELECT amount, payload FROM {table_ref} WHERE FALSE"),
            1000,
        )
        .await
        .expect("execute empty MySQL select");
        assert_eq!(empty.row_count, 0);
        assert!(empty.rows.is_empty());
        assert_eq!(
            empty
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec!["amount", "payload"],
        );

        sqlx::raw_sql(&format!("DROP TEMPORARY TABLE {table_ref}"))
            .execute(&pool)
            .await
            .expect("drop MySQL parity table");
    }

    #[tokio::test]
    async fn sqlite_row_crud_enforces_optimistic_concurrency() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("database-test")));
        let sqlite = Connection::open(&path).expect("open test database");
        sqlite
            .execute_batch(
                r#"
                CREATE TABLE items (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    note TEXT
                );
                "#,
            )
            .expect("create test table");
        drop(sqlite);

        let connection = sqlite_test_connection(&path);
        let metadata = database_table_metadata(&connection, None, None, "items")
            .await
            .expect("load metadata");
        let inserted = insert_database_row(
            &connection,
            None,
            None,
            "items",
            &metadata,
            &serde_json::json!({"id": 1, "name": "Alpha", "note": null})
                .as_object()
                .expect("insert row object")
                .clone(),
        )
        .await
        .expect("insert row")
        .expect("inserted row result");
        assert_eq!(
            inserted.get("name"),
            Some(&Value::String("Alpha".to_string()))
        );

        let scope = DatabaseTableScopeRequest {
            database_name: None,
            schema_name: None,
            table_name: "items".to_string(),
        };
        let primary_key = serde_json::json!({"id": 1})
            .as_object()
            .expect("primary key object")
            .clone();
        let updated = update_database_row(
            &connection,
            &scope,
            &primary_key,
            serde_json::json!({"name": "Beta"})
                .as_object()
                .expect("update object"),
            &inserted,
        )
        .await
        .expect("update row")
        .expect("updated row result");
        assert_eq!(
            updated.get("name"),
            Some(&Value::String("Beta".to_string()))
        );

        let stale_error = update_database_row(
            &connection,
            &scope,
            &primary_key,
            serde_json::json!({"name": "Gamma"})
                .as_object()
                .expect("stale update object"),
            &inserted,
        )
        .await
        .expect_err("stale update must fail");
        assert_eq!(stale_error.status, StatusCode::CONFLICT);

        delete_database_row(&connection, &scope, &primary_key, &updated)
            .await
            .expect("delete row");
        let page =
            read_sqlite_table_data(&connection, "items", 50, 0, true).expect("read empty table");
        assert_eq!(page.row_count, 0);
        assert_eq!(page.total_row_count, Some(0));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn portable_payload_and_structured_column_mapping_match_web_ai_cli() {
        let payload = serde_json::json!({
            "format": "web-ai-cli/database-portable-v1",
            "type": "table-schema-and-data",
            "table": {
                "name": "people",
                "columns": [
                    {
                        "name": "user_id",
                        "dataType": "integer",
                        "nativeType": "INTEGER",
                        "nullable": false,
                        "isPrimaryKey": true
                    },
                    {
                        "name": "display_name",
                        "dataType": "text",
                        "nativeType": "TEXT",
                        "nullable": true
                    }
                ]
            },
            "rows": [{"user_id": 1, "display_name": "Ada"}]
        });

        let portable = parse_portable_table_payload(&payload).expect("parse portable payload");
        assert_eq!(portable.table_name, "people");
        assert!(portable.include_data);
        assert_eq!(portable.columns.len(), 2);
        assert_eq!(portable.rows.len(), 1);

        let mappings = build_column_mappings(
            &["User ID".to_string(), "DISPLAY-NAME".to_string()],
            &portable.columns,
        );
        assert_eq!(
            mappings,
            vec![
                ("user_id".to_string(), "User ID".to_string()),
                ("display_name".to_string(), "DISPLAY-NAME".to_string()),
            ],
        );
    }

    #[test]
    fn complex_value_normalizers_match_web_driver_shapes() {
        assert_eq!(
            parse_postgres_array("{1,2,NULL}", "INT8"),
            serde_json::json!(["1", "2", null]),
        );
        assert_eq!(
            parse_postgres_array("{{1.25,2.5},{3.75,4}}", "NUMERIC"),
            serde_json::json!([[1.25, 2.5], [3.75, 4.0]]),
        );
        assert_eq!(
            parse_postgres_array("{\"{\\\"ok\\\":true}\",NULL}", "JSONB"),
            serde_json::json!([{"ok": true}, null]),
        );
        assert_eq!(
            parse_postgres_point("(1.5,-2.25)"),
            Some(serde_json::json!({"x": 1.5, "y": -2.25})),
        );
        assert_eq!(
            parse_postgres_circle("<(1,2),3.5>"),
            Some(serde_json::json!({"x": 1.0, "y": 2.0, "radius": 3.5})),
        );
        assert_eq!(
            parse_postgres_interval("1 year 2 mons 3 days 04:05:06.789"),
            serde_json::json!({
                "years": 1,
                "months": 2,
                "days": 3,
                "hours": 4,
                "minutes": 5,
                "seconds": 6,
                "milliseconds": 789,
            }),
        );

        let mut geometry = vec![0, 0, 0, 0, 1];
        geometry.extend_from_slice(&1u32.to_le_bytes());
        geometry.extend_from_slice(&1.5f64.to_le_bytes());
        geometry.extend_from_slice(&(-2.25f64).to_le_bytes());
        assert_eq!(
            parse_mysql_geometry(&geometry),
            Some(serde_json::json!({"x": 1.5, "y": -2.25})),
        );
    }

    #[test]
    fn database_text_literals_preserve_quotes_backslashes_and_unicode() {
        let value = "quote ' slash \\ newline\nemoji 🧪";
        let postgres = database_text_literal(SupportedDatabaseType::Postgresql, value);
        assert!(postgres.starts_with("$iowb$"));
        assert!(postgres.ends_with("$iowb$"));
        assert!(postgres.contains(value));
        assert_eq!(
            database_text_literal(SupportedDatabaseType::Mysql, value),
            "CONVERT(X'71756f7465202720736c617368205c206e65776c696e650a656d6f6a6920f09fa7aa' USING utf8mb4)",
        );
        assert_eq!(
            database_text_literal(SupportedDatabaseType::Sqlite, value),
            "CAST(X'71756f7465202720736c617368205c206e65776c696e650a656d6f6a6920f09fa7aa' AS TEXT)",
        );
    }

    #[test]
    fn capabilities_match_web_adapter_contract() {
        let postgres =
            serde_json::to_value(database_capabilities(SupportedDatabaseType::Postgresql))
                .expect("serialize postgres capabilities");
        assert_eq!(postgres["supportsDatabases"], Value::Bool(true));
        assert_eq!(postgres["supportsSchemas"], Value::Bool(true));
        assert_eq!(postgres["supportsViews"], Value::Bool(true));
        assert_eq!(postgres["supportsIndexes"], Value::Bool(true));
        assert_eq!(postgres["supportsForeignKeys"], Value::Bool(true));
        assert_eq!(postgres["supportsParameterizedQueries"], Value::Bool(true));
        assert_eq!(postgres["supportsOffset"], Value::Bool(true));
        assert_eq!(
            postgres["supportedObjectTypes"],
            serde_json::json!(["table", "view"]),
        );

        let sqlite = database_capabilities(SupportedDatabaseType::Sqlite);
        assert!(!sqlite.supports_databases);
        assert!(!sqlite.supports_schemas);
        assert!(sqlite.supports_views);
        assert!(sqlite.supports_indexes);
    }

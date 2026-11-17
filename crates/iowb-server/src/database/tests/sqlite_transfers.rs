    #[test]
    fn query_and_binary_contracts_match_web_ai_cli() {
        assert!(matches!(
            classify_statement("SHOW TABLES"),
            DatabaseQueryStatementType::Select
        ));
        assert!(matches!(
            classify_statement("DESCRIBE users"),
            DatabaseQueryStatementType::Select
        ));
        assert!(matches!(
            classify_statement("EXPLAIN SELECT * FROM users"),
            DatabaseQueryStatementType::Select
        ));

        let binary = database_buffer_value(&[0, 1, 2, 255]);
        assert_eq!(binary["type"], Value::String("buffer".to_string()));
        assert_eq!(binary["encoding"], Value::String("base64".to_string()));
        assert_eq!(binary["value"], Value::String("AAEC/w==".to_string()));
        assert_eq!(
            transfer_value_literal(SupportedDatabaseType::Postgresql, &binary),
            "decode($iowb$AAEC/w==$iowb$, 'base64')"
        );
        assert_eq!(
            transfer_value_literal(SupportedDatabaseType::Mysql, &binary),
            "FROM_BASE64(CONVERT(X'414145432f773d3d' USING utf8mb4))"
        );
        assert_eq!(
            transfer_value_literal(SupportedDatabaseType::Sqlite, &binary),
            "X'000102ff'"
        );

        let meta = query_result_meta(
            2,
            true,
            2,
            false,
            Some(serde_json::json!({ "driver": "sqlite" })),
        );
        assert_eq!(meta["returnedRowCount"], Value::from(2));
        assert_eq!(meta["resultTruncated"], Value::Bool(true));
        assert_eq!(meta["maxRows"], Value::from(2));
        assert_eq!(meta["rowCountExact"], Value::Bool(false));
        assert_eq!(meta["driver"], Value::String("sqlite".to_string()));
    }

    #[test]
    fn sqlite_queries_do_not_create_missing_files_but_transfer_targets_can() {
        let directory = env::temp_dir().join(new_id("database-missing-file-test"));
        let path = directory.join("new.sqlite");
        let connection = sqlite_test_connection(&path);

        let error =
            sqlite_connection(&connection).expect_err("normal browsing must not create a file");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(!path.exists());

        drop(
            sqlite_transfer_target_connection(&connection)
                .expect("an explicit transfer target may create its SQLite file"),
        );
        assert!(path.exists());

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(directory);
    }

    #[test]
    fn sqlite_query_stops_after_requested_rows_plus_one() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("database-query-test")));
        let sqlite = Connection::open(&path).expect("open query test database");
        sqlite
            .execute_batch(
                "CREATE TABLE records (id INTEGER PRIMARY KEY); INSERT INTO records VALUES (1), (2), (3), (4), (5);",
            )
            .expect("seed query test database");
        drop(sqlite);

        let result = execute_sqlite_query(
            &sqlite_test_connection(&path),
            "SELECT id FROM records ORDER BY id",
            2,
        )
        .expect("execute query");
        assert_eq!(result.row_count, 3);
        assert_eq!(result.returned_row_count, 2);
        assert!(result.result_truncated);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(
            result.meta.expect("query metadata")["rowCountExact"],
            Value::Bool(false)
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn sqlite_query_rejects_multi_statement_scripts_before_mutating() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("database-script-test")));
        drop(Connection::open(&path).expect("create test database"));

        let error = execute_sqlite_query(
            &sqlite_test_connection(&path),
            "CREATE TABLE audit_items (id INTEGER PRIMARY KEY); INSERT INTO audit_items VALUES (1);",
            100,
        )
        .expect_err("multi-statement scripts must not silently run only their prefix");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.body.error.contains("one SQL statement at a time"));

        let connection = Connection::open(&path).expect("reopen database after rejected script");
        let table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'audit_items'",
                [],
                |row| row.get(0),
            )
            .expect("inspect rejected script side effects");
        assert_eq!(table_count, 0, "the first statement must not be applied");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn sqlite_single_statement_guard_allows_literals_comments_and_triggers() {
        assert!(ensure_sqlite_single_statement("SELECT ';' AS punctuation;").is_ok());
        assert!(ensure_sqlite_single_statement("SELECT 1; -- trailing comment\n").is_ok());
        assert!(ensure_sqlite_single_statement(
            "CREATE TRIGGER audit_trigger AFTER INSERT ON records BEGIN UPDATE records SET id = id; END;"
        )
        .is_ok());
    }

    #[tokio::test]
    async fn sqlite_transfer_source_reads_beyond_former_row_limit() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("database-transfer-test")));
        let mut sqlite = Connection::open(&path).expect("open transfer test database");
        sqlite
            .execute_batch("CREATE TABLE records (id INTEGER PRIMARY KEY, value TEXT NOT NULL);")
            .expect("create transfer test table");
        let transaction = sqlite.transaction().expect("begin insert transaction");
        {
            let mut statement = transaction
                .prepare("INSERT INTO records (id, value) VALUES (?1, ?2)")
                .expect("prepare insert");
            for id in 0..10_250i64 {
                statement
                    .execute(params![id, format!("row-{id}")])
                    .expect("insert transfer row");
            }
        }
        transaction.commit().expect("commit transfer rows");
        drop(sqlite);

        let connection = sqlite_test_connection(&path);
        let endpoint = DatabaseTransferEndpoint {
            connection_id: connection.profile.id,
            connection_name: Some(connection.profile.name.clone()),
            connection_type: Some(connection.profile.db_type),
            database_name: None,
            schema_name: None,
            table_name: "records".to_string(),
        };
        let snapshot = read_transfer_source(&connection, &endpoint)
            .await
            .expect("read complete transfer source");

        assert_eq!(snapshot.rows.len(), 10_250);
        assert_eq!(
            snapshot.rows.first().and_then(|row| row.get("id")),
            Some(&Value::from(0))
        );
        assert_eq!(
            snapshot.rows.last().and_then(|row| row.get("id")),
            Some(&Value::from(10_249)),
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn sqlite_transfer_copies_multiple_two_hundred_row_batches() {
        let source_path = env::temp_dir().join(format!("{}.sqlite", new_id("transfer-source")));
        let target_path = env::temp_dir().join(format!("{}.sqlite", new_id("transfer-target")));
        let mut source_sqlite = Connection::open(&source_path).expect("open transfer source");
        source_sqlite
            .execute_batch("CREATE TABLE records (id INTEGER PRIMARY KEY, value TEXT NOT NULL);")
            .expect("create transfer source table");
        let transaction = source_sqlite.transaction().expect("begin source seed");
        {
            let mut statement = transaction
                .prepare("INSERT INTO records (id, value) VALUES (?1, ?2)")
                .expect("prepare source seed");
            for id in 0..450i64 {
                statement
                    .execute(params![id, format!("row-{id}")])
                    .expect("insert source row");
            }
        }
        transaction.commit().expect("commit source seed");
        drop(source_sqlite);

        let source_connection = sqlite_test_connection(&source_path);
        let mut target_connection = sqlite_test_connection(&target_path);
        target_connection.profile.id = 2;
        target_connection.profile.name = "test-target-sqlite".to_string();
        let source_endpoint = DatabaseTransferEndpoint {
            connection_id: source_connection.profile.id,
            connection_name: Some(source_connection.profile.name.clone()),
            connection_type: Some(source_connection.profile.db_type),
            database_name: None,
            schema_name: None,
            table_name: "records".to_string(),
        };
        let target_endpoint = DatabaseTransferEndpoint {
            connection_id: target_connection.profile.id,
            connection_name: Some(target_connection.profile.name.clone()),
            connection_type: Some(target_connection.profile.db_type),
            database_name: None,
            schema_name: None,
            table_name: "records_copy".to_string(),
        };
        let mut job = test_transfer_job(
            source_endpoint,
            target_endpoint,
            DatabaseTransferMode::SchemaAndData,
        );
        let mut progress_updates = Vec::new();
        let result = {
            let mut persist_job = |updated_job: &DatabaseTransferJob| {
                progress_updates.push(updated_job.progress.clone());
            };
            run_database_transfer(
                &source_connection,
                &target_connection,
                &mut job,
                &mut persist_job,
            )
            .await
            .expect("copy multiple transfer batches")
        };

        assert!(result.created_table);
        assert_eq!(result.copied_row_count, 450);
        assert_eq!(result.failed_row_count, 0);
        let percentages = progress_updates.iter()
            .filter_map(|update| update["percentage"].as_u64())
            .collect::<Vec<_>>();
        assert!(percentages.windows(2).all(|pair| pair[0] <= pair[1]),
            "transfer percentage regressed: {percentages:?}");
        assert!(progress_updates.iter().any(|update| {
            update["current"] == 0 && update["total"] == 450
        }), "copy phase must start before the first insert");
        assert!(progress_updates.iter().any(|update| {
            update["current"] == 1 && update["total"] == 450
        }), "the first committed row must not wait for the 200-row batch");
        assert!(progress_updates.iter().any(|update| {
            update["current"] == Value::from(200) && update["total"] == Value::from(450)
        }));
        assert!(progress_updates.iter().any(|update| {
            update["current"] == Value::from(400) && update["total"] == Value::from(450)
        }));
        assert!(progress_updates.iter().any(|update| {
            update["current"] == Value::from(450) && update["total"] == Value::from(450)
        }));

        let target_sqlite = Connection::open(&target_path).expect("open transfer target");
        let copied_count: i64 = target_sqlite
            .query_row("SELECT COUNT(*) FROM records_copy", [], |row| row.get(0))
            .expect("count copied rows");
        assert_eq!(copied_count, 450);
        drop(target_sqlite);

        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(target_path);
    }

    #[tokio::test]
    async fn sqlite_transfer_caps_failure_details_but_preserves_failed_count() {
        let source_path = env::temp_dir().join(format!("{}.sqlite", new_id("failure-source")));
        let target_path = env::temp_dir().join(format!("{}.sqlite", new_id("failure-target")));
        let mut source_sqlite = Connection::open(&source_path).expect("open failure source");
        source_sqlite
            .execute_batch("CREATE TABLE records (id INTEGER PRIMARY KEY, value TEXT NOT NULL);")
            .expect("create failure source table");
        let transaction = source_sqlite
            .transaction()
            .expect("begin failure source seed");
        {
            let mut statement = transaction
                .prepare("INSERT INTO records (id, value) VALUES (?1, 'duplicate')")
                .expect("prepare failure source seed");
            for id in 0..75i64 {
                statement.execute(params![id]).expect("insert failure row");
            }
        }
        transaction.commit().expect("commit failure source seed");
        drop(source_sqlite);

        let target_sqlite = Connection::open(&target_path).expect("open failure target");
        target_sqlite
            .execute_batch(
                "CREATE TABLE records_copy (id INTEGER PRIMARY KEY, value TEXT NOT NULL UNIQUE);\
                 INSERT INTO records_copy (id, value) VALUES (999, 'duplicate');",
            )
            .expect("seed conflicting target");
        drop(target_sqlite);

        let source_connection = sqlite_test_connection(&source_path);
        let mut target_connection = sqlite_test_connection(&target_path);
        target_connection.profile.id = 2;
        target_connection.profile.name = "test-target-sqlite".to_string();
        let source_endpoint = DatabaseTransferEndpoint {
            connection_id: source_connection.profile.id,
            connection_name: Some(source_connection.profile.name.clone()),
            connection_type: Some(source_connection.profile.db_type),
            database_name: None,
            schema_name: None,
            table_name: "records".to_string(),
        };
        let target_endpoint = DatabaseTransferEndpoint {
            connection_id: target_connection.profile.id,
            connection_name: Some(target_connection.profile.name.clone()),
            connection_type: Some(target_connection.profile.db_type),
            database_name: None,
            schema_name: None,
            table_name: "records_copy".to_string(),
        };
        let mut job = test_transfer_job(
            source_endpoint,
            target_endpoint,
            DatabaseTransferMode::TableCopy,
        );
        let result = run_database_transfer(
            &source_connection,
            &target_connection,
            &mut job,
            &mut |_| {},
        )
        .await
        .expect("complete transfer with row failures");

        assert_eq!(result.copied_row_count, 0);
        assert_eq!(result.failed_row_count, 75);
        assert_eq!(result.row_failures.len(), MAX_ROW_FAILURE_DETAILS);
        assert!(
            result
                .row_failures
                .iter()
                .all(|failure| failure["code"] == Value::String("SQLITE_CONSTRAINT".to_string()))
        );
        assert!(job.warnings.iter().any(|warning| {
            warning.code.as_deref() == Some("ROW_COPY_FAILURES")
                && warning.message == "75 row(s) failed to copy"
        }));
        assert!(job.logs.iter().any(|entry| {
            entry.level == "warning"
                && entry.message == "75 row(s) failed to copy"
                && entry.details.as_deref() == Some("records_copy")
        }));

        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(target_path);
    }

    struct TransferDefaultWarningFixture {
        source_path: std::path::PathBuf,
        target_path: std::path::PathBuf,
    }

    impl Drop for TransferDefaultWarningFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.source_path);
            let _ = std::fs::remove_file(&self.target_path);
        }
    }

    async fn assert_transfer_default_warning_scope(mode: DatabaseTransferMode) {
        let fixture = TransferDefaultWarningFixture {
            source_path: env::temp_dir().join(format!("{}.sqlite", new_id("default-warning-source"))),
            target_path: env::temp_dir().join(format!("{}.sqlite", new_id("default-warning-target"))),
        };
        for path in [&fixture.source_path, &fixture.target_path] {
            std::fs::OpenOptions::new().write(true).create_new(true).open(path)
                .expect("reserve private default-warning fixture");
        }
        let source_native = Connection::open(&fixture.source_path).unwrap();
        source_native.execute_batch(
            "CREATE TABLE records (id TEXT PRIMARY KEY, value TEXT DEFAULT 'source-default'); \
             INSERT INTO records (id) VALUES ('copied');"
        ).unwrap();
        let target_native = Connection::open(&fixture.target_path).unwrap();
        let existing_target = mode == DatabaseTransferMode::TableCopy;
        let original_target_ddl: Option<String> = if existing_target {
            target_native.execute_batch(
                "CREATE TABLE records_copy (id TEXT PRIMARY KEY, value TEXT DEFAULT 'target-default', \
                 target_only TEXT DEFAULT 'kept-default'); \
                 INSERT INTO records_copy (id, value, target_only) VALUES ('existing', 'untouched', 'original');"
            ).unwrap();
            Some(target_native.query_row(
                "SELECT sql FROM sqlite_master WHERE name='records_copy'", [], |row| row.get(0),
            ).unwrap())
        } else {
            None
        };
        let source = sqlite_test_connection(&fixture.source_path);
        let mut target = sqlite_test_connection(&fixture.target_path);
        target.profile.id = 2;
        let endpoint = |connection_id, table_name: &str| DatabaseTransferEndpoint {
            connection_id,
            connection_name: None,
            connection_type: Some(SupportedDatabaseType::Sqlite),
            database_name: None,
            schema_name: None,
            table_name: table_name.to_string(),
        };
        let mut job = test_transfer_job(endpoint(1, "records"), endpoint(2, "records_copy"), mode);
        let result = run_database_transfer(&source, &target, &mut job, &mut |_| {}).await.unwrap();
        assert_eq!(result.created_table, !existing_target);
        assert_eq!(result.failed_row_count, 0);
        assert_eq!(result.copied_row_count, usize::from(mode != DatabaseTransferMode::SchemaOnly));
        let source_row: (String, String) = source_native.query_row(
            "SELECT id, value FROM records", [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(source_row, ("copied".into(), "source-default".into()));
        let source_default: Option<String> = source_native.query_row(
            "SELECT dflt_value FROM pragma_table_info('records') WHERE name='value'", [], |row| row.get(0),
        ).unwrap();
        assert_eq!(source_default.as_deref(), Some("'source-default'"));

        if existing_target {
            let after_ddl: String = target_native.query_row(
                "SELECT sql FROM sqlite_master WHERE name='records_copy'", [], |row| row.get(0),
            ).unwrap();
            assert_eq!(Some(after_ddl), original_target_ddl, "row copying must not rewrite target defaults/schema");
            let rows = target_native.prepare("SELECT id, value, target_only FROM records_copy ORDER BY id")
                .unwrap().query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))
                .unwrap().collect::<std::result::Result<Vec<_>, _>>().unwrap();
            assert_eq!(rows, vec![
                ("copied".into(), "source-default".into(), "kept-default".into()),
                ("existing".into(), "untouched".into(), "original".into()),
            ]);
            assert!(job.warnings.is_empty(), "existing-table copy has no schema-loss warning: {:?}", job.warnings);
            assert!(!job.logs.iter().any(|entry| entry.level == "warning"));
        } else {
            let default: Option<String> = target_native.query_row(
                "SELECT dflt_value FROM pragma_table_info('records_copy') WHERE name='value'", [], |row| row.get(0),
            ).unwrap();
            assert_eq!(default, None, "portable schema really omits the source default");
            let count: usize = target_native.query_row("SELECT COUNT(*) FROM records_copy", [], |row| row.get(0)).unwrap();
            assert_eq!(count, result.copied_row_count);
            if mode == DatabaseTransferMode::SchemaAndData {
                let copied: (String, String) = target_native.query_row(
                    "SELECT id, value FROM records_copy", [], |row| Ok((row.get(0)?, row.get(1)?)),
                ).unwrap();
                assert_eq!(copied, source_row);
            }
            assert_eq!(job.warnings.len(), 2);
            assert!(job.warnings.iter().any(|warning| warning.code.as_deref() == Some("SCHEMA_PORTABILITY_LIMITATIONS")));
            assert!(job.warnings.iter().any(|warning| {
                warning.code.as_deref() == Some("DEFAULT_SKIPPED")
                    && warning.scope.as_deref() == Some("value")
                    && warning.message == "Skipped default value mapping for column value"
            }));
        }
    }

    #[tokio::test]
    async fn transfer_default_warning_existing_table_preserves_target_defaults_without_schema_loss_warning() {
        assert_transfer_default_warning_scope(DatabaseTransferMode::TableCopy).await;
    }

    #[tokio::test]
    async fn transfer_default_warning_schema_only_reports_actual_default_loss() {
        assert_transfer_default_warning_scope(DatabaseTransferMode::SchemaOnly).await;
    }

    #[tokio::test]
    async fn transfer_default_warning_schema_and_data_reports_actual_default_loss() {
        assert_transfer_default_warning_scope(DatabaseTransferMode::SchemaAndData).await;
    }

    async fn assert_transfer_explicit_not_null_text_primary_key(mode: DatabaseTransferMode) {
        let fixture = TransferDefaultWarningFixture {
            source_path: env::temp_dir().join(format!("{}.sqlite", new_id("not-null-source"))),
            target_path: env::temp_dir().join(format!("{}.sqlite", new_id("not-null-target"))),
        };
        for path in [&fixture.source_path, &fixture.target_path] {
            std::fs::OpenOptions::new().write(true).create_new(true).open(path)
                .expect("reserve private nullability fixture");
        }
        let source_native = Connection::open(&fixture.source_path).unwrap();
        source_native.execute_batch(
            "CREATE TABLE records (row_key TEXT NOT NULL PRIMARY KEY, title TEXT NOT NULL DEFAULT 'fixture-default'); \
             INSERT INTO records (row_key) VALUES ('source-row');"
        ).unwrap();
        let read_flags = |native: &Connection, table: &str| {
            native.prepare(&format!("SELECT name, \"notnull\", pk FROM pragma_table_info('{table}') ORDER BY cid"))
                .unwrap().query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?, row.get::<_, i64>(2)?)))
                .unwrap().collect::<std::result::Result<Vec<_>, _>>().unwrap()
        };
        let expected_flags = vec![("row_key".to_string(), true, 1), ("title".to_string(), true, 0)];
        assert_eq!(read_flags(&source_native, "records"), expected_flags);
        assert!(source_native.execute("INSERT INTO records VALUES (NULL, 'probe')", []).is_err());
        assert!(source_native.execute("INSERT INTO records VALUES ('null-title', NULL)", []).is_err());

        let source = sqlite_test_connection(&fixture.source_path);
        let mut target = sqlite_test_connection(&fixture.target_path);
        target.profile.id = 2;
        let metadata = database_table_metadata(&source, None, None, "records").await.unwrap();
        assert!(metadata.columns.iter().all(|column| column.nullable == Some(false)));
        let endpoint = |connection_id, table_name: &str| DatabaseTransferEndpoint {
            connection_id,
            connection_name: None,
            connection_type: Some(SupportedDatabaseType::Sqlite),
            database_name: None,
            schema_name: None,
            table_name: table_name.to_string(),
        };
        let mut job = test_transfer_job(endpoint(1, "records"), endpoint(2, "records_copy"), mode);
        let result = run_database_transfer(&source, &target, &mut job, &mut |_| {}).await.unwrap();
        assert!(result.created_table);
        assert_eq!(result.failed_row_count, 0);
        assert_eq!(result.copied_row_count, usize::from(mode == DatabaseTransferMode::SchemaAndData));

        let target_native = Connection::open(&fixture.target_path).unwrap();
        let target_flags = read_flags(&target_native, "records_copy");
        let target_ddl: String = target_native.query_row(
            "SELECT sql FROM sqlite_master WHERE name='records_copy'", [], |row| row.get(0),
        ).unwrap();
        // Exercise actual SQLite constraints, not just the generated SQL text.
        // Both probes are rolled back even if a broken schema accepts one.
        target_native.execute_batch("SAVEPOINT nullability_probe").unwrap();
        let null_key_rejected = target_native.execute(
            "INSERT INTO records_copy VALUES (NULL, 'null-key-probe')", [],
        ).is_err();
        let null_title_rejected = target_native.execute(
            "INSERT INTO records_copy VALUES ('null-title-probe', NULL)", [],
        ).is_err();
        target_native.execute_batch("ROLLBACK TO nullability_probe; RELEASE nullability_probe").unwrap();
        let copied_rows = target_native.prepare("SELECT row_key, title FROM records_copy ORDER BY row_key")
            .unwrap().query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .unwrap().collect::<std::result::Result<Vec<_>, _>>().unwrap();
        let expected_rows = if mode == DatabaseTransferMode::SchemaAndData {
            vec![("source-row".to_string(), "fixture-default".to_string())]
        } else {
            Vec::new()
        };
        assert_eq!(copied_rows, expected_rows);
        assert_eq!(source_native.query_row("SELECT COUNT(*) FROM records", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
        assert_eq!(
            (target_flags, null_key_rejected, null_title_rejected),
            (expected_flags, true, true),
            "explicit source NOT NULL must remain enforced on the text PK and ordinary column; target DDL: {target_ddl}",
        );
    }

    #[tokio::test]
    async fn transfer_not_null_schema_only_preserves_explicit_text_primary_key_constraint() {
        assert_transfer_explicit_not_null_text_primary_key(DatabaseTransferMode::SchemaOnly).await;
    }

    #[tokio::test]
    async fn transfer_not_null_schema_and_data_preserves_explicit_text_primary_key_constraint() {
        assert_transfer_explicit_not_null_text_primary_key(DatabaseTransferMode::SchemaAndData).await;
    }

    #[test]
    fn transfer_not_null_portable_schema_builder_preserves_only_explicit_constraints() {
        let payload = serde_json::json!({
            "format": "web-ai-cli/database-portable-v1",
            "type": "table-schema",
            "table": {"name": "portable_records", "columns": [
                {"name": "row_key", "dataType": "text", "nativeType": "TEXT", "nullable": false, "isPrimaryKey": true},
                {"name": "title", "dataType": "text", "nativeType": "TEXT", "nullable": false}
            ]}
        });
        let portable = parse_portable_table_payload(&payload).unwrap();
        let endpoint = DatabaseTransferEndpoint {
            connection_id: 1,
            connection_name: None,
            connection_type: None,
            database_name: None,
            schema_name: None,
            table_name: portable.table_name,
        };
        for nullable in [Some(false), Some(true), None] {
            let mut columns = portable.columns.clone();
            columns[0].nullable = nullable;
            for db_type in [SupportedDatabaseType::Sqlite, SupportedDatabaseType::Postgresql,
                SupportedDatabaseType::Mysql, SupportedDatabaseType::Mariadb] {
                let sql = build_transfer_create_table_sql(db_type, &endpoint, &columns);
                assert_eq!(sql.matches("NOT NULL").count(), 1 + usize::from(nullable == Some(false)),
                    "shared portable-import builder must preserve explicit constraints: {sql}");
            }
            let native = Connection::open_in_memory().unwrap();
            native.execute_batch(&build_transfer_create_table_sql(SupportedDatabaseType::Sqlite, &endpoint, &columns)).unwrap();
            let null_pk = native.execute("INSERT INTO portable_records VALUES (NULL, 'provided')", []);
            assert_eq!(null_pk.is_err(), nullable == Some(false),
                "do not remove explicit NOT NULL or invent it for nullable/unknown text primary keys");
            assert!(native.execute("INSERT INTO portable_records VALUES ('null-title', NULL)", []).is_err());
            assert_eq!(native.execute("INSERT INTO portable_records VALUES ('valid', 'retained')", []).unwrap(), 1);
        }
    }

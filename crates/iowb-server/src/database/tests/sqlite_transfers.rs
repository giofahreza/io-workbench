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
            "decode('AAEC/w==', 'base64')"
        );
        assert_eq!(
            transfer_value_literal(SupportedDatabaseType::Mysql, &binary),
            "FROM_BASE64('AAEC/w==')"
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

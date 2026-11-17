    use super::*;
    use std::env;

    #[test]
    fn database_cell_query_post_body_preserves_large_json_and_validates_limits() {
        let value = serde_json::json!({"payload": "large workflow value ".repeat(15_000)});
        let request = serde_json::json!({
            "tableName": "items", "limit": 1, "includeTotalCount": true,
            "filters": [{"columnName": "payload", "operator": "eq", "value": value}],
            "sort": [{"columnName": "id", "direction": "desc"}]
        });
        let body: TableDataQueryBody = serde_json::from_value(request).expect("large POST body");
        let options = DatabaseTableQueryOptions::new(body.filters, body.sort).expect("validated options");
        assert_eq!(options.filters[0].value, value);
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("database-cell-query-body")));
        let sqlite = Connection::open(&path).expect("fixture");
        sqlite.execute_batch("CREATE TABLE items (id INTEGER PRIMARY KEY, payload JSON)").unwrap();
        sqlite.execute("INSERT INTO items VALUES (1, ?1), (2, ?1)", params![value.to_string()]).unwrap();
        drop(sqlite);
        let connection = sqlite_test_connection(&path);
        let page = read_sqlite_table_data_with_options(&connection, "items", 1, 0, true, &options).unwrap();
        assert_eq!(page.total_row_count, Some(2));
        assert_eq!(page.rows[0]["id"], 2);
        assert!(page.has_more);
        std::fs::remove_file(path).unwrap();

        let too_many = (0..33).map(|_| DatabaseTableFilter::default()).collect();
        assert!(DatabaseTableQueryOptions::new(too_many, Vec::new()).is_err());
        assert!(serde_json::from_value::<TableDataQueryBody>(serde_json::json!({
            "tableName": "items", "filters": [{"columnName":"id","operator":"eq","sql":"1=1"}]
        })).is_err());
    }

    #[test]
    fn database_cell_query_filters_sort_and_count_across_pages() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("database-cell-query")));
        let sqlite = Connection::open(&path).expect("open cell query fixture");
        sqlite.execute_batch(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, score INTEGER); \
             INSERT INTO items VALUES (9, 'match', 4), (2, 'match', 4), (3, 'match', 7), \
             (4, 'other', 8), (5, NULL, 9);"
        ).expect("seed fixture");
        drop(sqlite);
        let connection = sqlite_test_connection(&path);
        let options = DatabaseTableQueryOptions::parse(
            Some(r#"[{"column":"name","operator":"eq","value":"match"}]"#),
            Some(r#"[{"column":"score","direction":"desc"}]"#),
        ).expect("parse query");
        let first = read_sqlite_table_data_with_options(&connection, "items", 1, 0, true, &options)
            .expect("read first page");
        assert_eq!(first.rows[0]["id"], 3);
        assert_eq!(first.total_row_count, Some(3));
        assert!(first.has_more);
        let second = read_sqlite_table_data_with_options(&connection, "items", 1, 1, true, &options)
            .expect("read stable tie page");
        assert_eq!(second.rows[0]["id"], 2);
        assert_eq!(second.total_row_count, Some(3));
        let nulls = DatabaseTableQueryOptions::parse(
            Some(r#"[{"column":"name","operator":"isNull"}]"#), None,
        ).expect("parse null query");
        let null_page = read_sqlite_table_data_with_options(&connection, "items", 50, 0, true, &nulls)
            .expect("read null page");
        assert_eq!(null_page.total_row_count, Some(1));
        assert_eq!(null_page.rows[0]["id"], 5);
        let injection = DatabaseTableQueryOptions::parse(
            Some(r#"[{"column":"name","operator":"eq","value":"' OR 1=1 --"}]"#), None,
        ).expect("parse literal query");
        let empty = read_sqlite_table_data_with_options(&connection, "items", 50, 0, true, &injection)
            .expect("literal must be data, not SQL");
        assert_eq!(empty.total_row_count, Some(0));
        let combined = DatabaseTableQueryOptions::parse(
            Some(r#"[{"column":"name","operator":"isNotNull"},{"column":"score","operator":"ne","value":4}]"#), None,
        ).expect("parse combined query");
        let combined_page = read_sqlite_table_data_with_options(&connection, "items", 50, 0, true, &combined)
            .expect("read AND filter");
        assert_eq!(combined_page.total_row_count, Some(2));
        std::fs::remove_file(path).expect("remove disposable fixture");
    }

    #[test]
    fn database_cell_query_validates_columns_operators_and_directions() {
        let columns: Vec<DatabaseObjectColumn> = serde_json::from_value(serde_json::json!([
            {"name":"id", "dataType":"integer", "isPrimaryKey":true},
            {"name":"payload", "dataType":"jsonb"}
        ])).expect("columns");
        for filters in [
            r#"[{"column":"unknown","operator":"eq","value":1}]"#,
            r#"[{"column":"id","operator":"= 1 OR TRUE --","value":1}]"#,
            r#"[{"column":"id","operator":"eq","value":{"type":"Buffer","data":"'); DROP TABLE items;--"}}]"#,
        ] {
            let options = DatabaseTableQueryOptions::parse(Some(filters), None).expect("parse filter");
            assert!(database_table_query_clauses(SupportedDatabaseType::Postgresql, &columns, &options).is_err());
        }
        let options = DatabaseTableQueryOptions::parse(None,
            Some(r#"[{"column":"id","direction":"ASC; DELETE FROM items"}]"#)
        ).expect("parse sort");
        assert!(database_table_query_clauses(SupportedDatabaseType::Postgresql, &columns, &options).is_err());
        assert!(DatabaseTableQueryOptions::parse(Some("invalid"), None).is_err());
        assert!(DatabaseTableQueryOptions::parse(Some(r#"[{"column":"id","operator":"eq","sql":"1=1"}]"#), None).is_err());
    }

    #[test]
    fn database_cell_query_quotes_json_and_appends_primary_key_sort() {
        let columns: Vec<DatabaseObjectColumn> = serde_json::from_value(serde_json::json!([
            {"name":"record\"id", "dataType":"integer", "isPrimaryKey":true},
            {"name":"payload", "dataType":"jsonb"}
        ])).expect("columns");
        let options = DatabaseTableQueryOptions::parse(
            Some(r#"[{"column":"payload","operator":"eq","value":{"name":"O'Reilly"}}]"#),
            Some(r#"[{"column":"payload","direction":"desc"}]"#),
        ).expect("parse JSON filter");
        let (filter, order) = database_table_query_clauses(SupportedDatabaseType::Postgresql, &columns, &options)
            .expect("safe clauses");
        assert_eq!(filter, " WHERE \"payload\"::jsonb = $iowb${\"name\":\"O'Reilly\"}$iowb$::jsonb");
        assert_eq!(order, " ORDER BY \"payload\" DESC, \"record\"\"id\"");
    }

    #[test]
    fn live_table_pages_order_by_quoted_primary_keys() {
        assert_eq!(
            live_table_page_sql(
                SupportedDatabaseType::Postgresql,
                "\"public\".\"workflow_definition\"",
                &["tenant_id".to_string(), "record\"id".to_string()],
                51,
                50,
            ),
            "SELECT * FROM \"public\".\"workflow_definition\" ORDER BY \"tenant_id\", \"record\"\"id\" LIMIT 51 OFFSET 50"
        );
        for db_type in [SupportedDatabaseType::Mysql, SupportedDatabaseType::Mariadb] {
            assert_eq!(
                live_table_page_sql(db_type, "`records`", &["record`id".to_string()], 26, 25),
                "SELECT * FROM `records` ORDER BY `record``id` LIMIT 26 OFFSET 25"
            );
        }
    }

    #[test]
    fn live_table_pages_without_primary_keys_do_not_invent_ordering_columns() {
        assert_eq!(
            live_table_page_sql(SupportedDatabaseType::Postgresql, "\"view\"", &[], 51, 0),
            "SELECT * FROM \"view\" LIMIT 51 OFFSET 0"
        );
    }

    fn sqlite_test_connection(path: &std::path::Path) -> StoredDatabaseConnection {
        let now = Utc::now();
        StoredDatabaseConnection {
            profile: DatabaseConnectionProfile {
                id: 1,
                name: "test-sqlite".to_string(),
                db_type: SupportedDatabaseType::Sqlite,
                host: None,
                port: None,
                username: None,
                database_name: None,
                file_path: Some(path.to_string_lossy().into_owned()),
                show_all_databases: false,
                has_password: false,
                last_test_status: None,
                last_test_message: None,
                last_tested_at: None,
                created_at: now,
                updated_at: now,
            },
            password: None,
        }
    }

    fn test_transfer_job(
        source: DatabaseTransferEndpoint,
        target: DatabaseTransferEndpoint,
        mode: DatabaseTransferMode,
    ) -> DatabaseTransferJob {
        let now = Utc::now();
        DatabaseTransferJob {
            id: new_id("database-transfer-job-test"),
            job_type: "table-transfer".to_string(),
            mode,
            status: DatabaseTransferJobStatus::Running,
            source,
            target,
            progress: progress(0, 1, "Queued"),
            logs: Vec::new(),
            warnings: Vec::new(),
            error: None,
            result: None,
            created_at: now,
            updated_at: now,
            started_at: Some(now),
            finished_at: None,
        }
    }

    fn postgres_test_connection() -> Option<StoredDatabaseConnection> {
        if env::var("IOWB_RUN_LIVE_POSTGRES_TESTS").ok().as_deref() != Some("1") {
            return None;
        }
        let now = Utc::now();
        Some(StoredDatabaseConnection {
            profile: DatabaseConnectionProfile {
                id: -1,
                name: "test-postgresql".to_string(),
                db_type: SupportedDatabaseType::Postgresql,
                host: Some(
                    env::var("IOWB_TEST_POSTGRES_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
                ),
                port: Some(
                    env::var("IOWB_TEST_POSTGRES_PORT")
                        .ok()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(5432),
                ),
                username: Some(
                    env::var("IOWB_TEST_POSTGRES_USER")
                        .or_else(|_| env::var("USER"))
                        .unwrap_or_else(|_| "postgres".to_string()),
                ),
                database_name: Some(
                    env::var("IOWB_TEST_POSTGRES_DATABASE")
                        .unwrap_or_else(|_| "postgres".to_string()),
                ),
                file_path: None,
                show_all_databases: false,
                has_password: env::var_os("IOWB_TEST_POSTGRES_PASSWORD").is_some(),
                last_test_status: None,
                last_test_message: None,
                last_tested_at: None,
                created_at: now,
                updated_at: now,
            },
            password: env::var("IOWB_TEST_POSTGRES_PASSWORD").ok(),
        })
    }

    fn mysql_test_connection() -> Option<StoredDatabaseConnection> {
        if env::var("IOWB_RUN_LIVE_MYSQL_TESTS").ok().as_deref() != Some("1") {
            return None;
        }
        let now = Utc::now();
        Some(StoredDatabaseConnection {
            profile: DatabaseConnectionProfile {
                id: -2,
                name: "test-mysql".to_string(),
                db_type: SupportedDatabaseType::Mysql,
                host: Some(
                    env::var("IOWB_TEST_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
                ),
                port: Some(
                    env::var("IOWB_TEST_MYSQL_PORT")
                        .ok()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(3306),
                ),
                username: Some(
                    env::var("IOWB_TEST_MYSQL_USER").unwrap_or_else(|_| "root".to_string()),
                ),
                database_name: Some(
                    env::var("IOWB_TEST_MYSQL_DATABASE").unwrap_or_else(|_| "iowb".to_string()),
                ),
                file_path: None,
                show_all_databases: false,
                has_password: env::var_os("IOWB_TEST_MYSQL_PASSWORD").is_some(),
                last_test_status: None,
                last_test_message: None,
                last_tested_at: None,
                created_at: now,
                updated_at: now,
            },
            password: env::var("IOWB_TEST_MYSQL_PASSWORD").ok(),
        })
    }

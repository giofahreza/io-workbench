    #[tokio::test]
    async fn paste_preserves_exact_whitespace_column_identifiers_in_all_payload_formats() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("paste-column-identity")));
        let sqlite = Connection::open(&path).unwrap();
        let names = ["marker", "\u{a0}marker", "marker\u{a0}", " marker", "marker ", " ", "\u{feff}marker", "\u{200b}marker"];
        let definitions = names.iter().map(|name| format!("{} TEXT", quote_identifier(name)))
            .collect::<Vec<_>>().join(", ");
        sqlite.execute_batch(&format!("CREATE TABLE records ({definitions})")).unwrap();
        let connection = sqlite_test_connection(&path);
        let metadata = database_table_metadata(&connection, None, None, "records").await.unwrap();
        let columns = names.iter().map(|name| serde_json::json!({"name": name})).collect::<Vec<_>>();
        let row = names.iter().enumerate().map(|(index, name)|
            (name.to_string(), serde_json::json!(format!("sentinel-{index}"))))
            .collect::<Map<_, _>>();
        let requests = [
            serde_json::json!({"tableName":"records", "clipboard":{"type":"rows", "columns":columns, "rows":[row]}}),
            serde_json::json!({"tableName":"records", "sourceColumns":columns, "sourceRows":[row]}),
            serde_json::json!({"tableName":"records", "sourceRows":[row]}),
        ];
        for (format_index, payload) in requests.into_iter().enumerate() {
            let request: DatabasePasteRequest = serde_json::from_value(payload).unwrap();
            let source_names = database_paste_source_column_names(&request);
            assert_eq!(source_names.len(), names.len(), "format {format_index}");
            let mappings = build_column_mappings(&source_names, &metadata.columns).unwrap();
            assert_eq!(mappings.len(), names.len(), "format {format_index}");
            let source_row = request.clipboard.as_ref().map(|clipboard| &clipboard.rows[0])
                .unwrap_or_else(|| &request.source_rows[0]);
            let values = mappings.iter().map(|(target, source)| {
                assert_eq!(target, source, "exact identifiers must own their mapping");
                (target.clone(), source_row.get(source).cloned().unwrap_or(Value::Null))
            }).collect::<Map<_, _>>();
            assert_eq!(values, row, "format {format_index} must not lose any source value");
            insert_database_row(&connection, None, None, "records", &metadata, &values).await.unwrap();
            let stored = sqlite.query_row("SELECT * FROM records WHERE rowid = ?1", [format_index + 1], |stored| {
                (0..names.len()).map(|index| stored.get::<_, String>(index)).collect::<rusqlite::Result<Vec<_>>>()
            }).unwrap();
            assert_eq!(stored, (0..names.len()).map(|index| format!("sentinel-{index}")).collect::<Vec<_>>());
        }
        drop(sqlite);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mapping_rejects_ambiguous_names_and_unrelated_unicode() {
        let payload = serde_json::json!({"format":"web-ai-cli/database-portable-v1","type":"table-schema","table":{"name":"t","columns":[{"name":"user_id"}]}});
        let columns = parse_portable_table_payload(&payload).unwrap().columns;
        assert!(build_column_mappings(&["User ID".into(),"USER-ID".into()], &columns).is_err());
        assert_eq!(build_column_mappings(&["User ID".into(),"user_id".into()], &columns).unwrap(), vec![("user_id".into(),"user_id".into())]);
        let mut unicode = columns.clone(); unicode[0].name = "城市".into();
        assert!(build_column_mappings(&["名前".into()], &unicode).unwrap().is_empty());
        let mut targets = columns.clone(); targets[0].name = "User ID".into();
        let mut second = targets[0].clone(); second.name = "USER-ID".into(); targets.push(second);
        assert!(build_column_mappings(&["user_id".into()], &targets).is_err());
        let mut case_columns = columns.clone(); case_columns[0].name = "name".into();
        let mut case_alias = case_columns[0].clone(); case_alias.name = "Name".into(); case_columns.push(case_alias);
        assert_eq!(build_column_mappings(&["name".into()], &case_columns).unwrap(), vec![("name".into(),"name".into())]);
        let metadata = DatabaseTableMetadata { columns: case_columns, primary_key: Vec::new() };
        let values = serde_json::json!({"name":"lower","Name":"upper"}).as_object().unwrap().clone();
        let canonical = canonical_column_values(&metadata, &values, true).unwrap();
        assert!(canonical.contains(&("name".into(), serde_json::json!("lower"))));
        assert!(canonical.contains(&("Name".into(), serde_json::json!("upper"))));
    }

    #[test]
    fn duplicate_constraint_error_does_not_echo_sensitive_values() {
        let message = redact_duplicate_entry_value("error returned from database: Duplicate entry 'private-token-value' for key 'users.secret'");
        assert!(!message.contains("private-token-value"));
        assert!(message.contains("users.secret"));
    }

    #[test]
    fn portable_json_domain_buffer_object_is_not_reinterpreted_as_bytes() {
        let payload = serde_json::json!({"format":"web-ai-cli/database-portable-v1","type":"table-schema-and-data","table":{"name":"json_values","columns":[{"name":"value","dataType":"json"}]},"rows":[{"value":{"type":"buffer","encoding":"base64","value":"AA=="}}]});
        let portable = parse_portable_table_payload(&payload).unwrap();
        let endpoint = DatabaseTransferEndpoint { connection_id: 1, connection_name: None, connection_type: None, database_name: None, schema_name: None, table_name: "json_values".into() };
        let sql = build_transfer_insert_sql(SupportedDatabaseType::Sqlite, &endpoint, &portable.columns, &portable.rows[0]);
        assert!(sql.contains("CAST(X'"));
        assert!(!sql.contains("VALUES (X'00')"));
    }

    #[test]
    fn portable_payload_rejects_silently_discarded_or_conflicting_content() {
        let good = serde_json::json!({"format":"web-ai-cli/database-portable-v1","type":"table-schema-and-data","table":{"name":"t","columns":[{"name":"id","isPrimaryKey":true}],"primaryKey":["id"]},"rows":[{"id":"one"}]});
        assert!(parse_portable_table_payload(&good).is_ok());
        let mut bad = good.clone(); bad["rows"].as_array_mut().unwrap().push(Value::Null); assert!(parse_portable_table_payload(&bad).is_err());
        let mut bad = good.clone(); bad["table"]["columns"].as_array_mut().unwrap().push(Value::Null); assert!(parse_portable_table_payload(&bad).is_err());
        let mut bad = good.clone(); bad["table"]["primaryKey"] = serde_json::json!(["other"]); assert!(parse_portable_table_payload(&bad).is_err());
        let mut bad = good.clone(); bad["table"]["columns"].as_array_mut().unwrap().push(serde_json::json!({"name":"other","isPrimaryKey":true})); bad["table"]["primaryKey"] = serde_json::json!(["id","id"]); assert!(parse_portable_table_payload(&bad).is_err());
        let mut bad = good.clone(); bad.as_object_mut().unwrap().remove("rows"); assert!(parse_portable_table_payload(&bad).is_err());
        let bad = serde_json::json!({"format":"web-ai-cli/database-portable-v1","type":"table-schema-and-data","table":{"name":"bytes","columns":[{"name":"value","dataType":"blob"}]},"rows":[{"value":{"type":"buffer","encoding":"base64","value":"%%%invalid%%%"}}]});
        assert!(parse_portable_table_payload(&bad).is_err());
    }

    #[tokio::test]
    async fn transfer_snapshot_prevents_alias_growth_and_source_mutation() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("snapshot-safety")));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE source (id TEXT PRIMARY KEY, value TEXT);WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<401) INSERT INTO source SELECT printf('%03d',x),'before' FROM n;").unwrap();
        let connection = sqlite_test_connection(&path);
        let endpoint = DatabaseTransferEndpoint { connection_id: 1, connection_name: None, connection_type: None, database_name: None, schema_name: None, table_name: "source".into() };
        let mut spool = spool_transfer_source(&connection, &endpoint).await.unwrap();
        conn.execute_batch("DELETE FROM source WHERE id<='100';UPDATE source SET value='after' WHERE id='301';INSERT INTO source VALUES('999','new');").unwrap();
        let mut rows = Vec::new(); loop { let page = spool.read_batch().unwrap(); if page.is_empty() {break;} rows.extend(page); }
        assert_eq!(rows.len(),401);
        assert!(rows.iter().any(|row| row["id"] == "301" && row["value"] == "before"));
        assert!(!rows.iter().any(|row| row["id"] == "999"));
        drop(spool); drop(conn); let _ = std::fs::remove_file(path);
    }

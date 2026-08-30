    #[test]
    fn failed_database_job_preserves_structured_error_metadata() {
        let endpoint = DatabaseTransferEndpoint {
            connection_id: 1,
            connection_name: Some("test".to_string()),
            connection_type: Some(SupportedDatabaseType::Sqlite),
            database_name: None,
            schema_name: None,
            table_name: "records".to_string(),
        };
        let mut job =
            test_transfer_job(endpoint.clone(), endpoint, DatabaseTransferMode::TableCopy);
        job.progress = progress(3, 4, "Loading target table metadata");
        fail_database_job(
            &mut job,
            ServerError::database(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to insert row",
                Some(serde_json::json!({ "operation": "transfer", "rowIndex": 7 }).to_string()),
                "SQLITE_CONSTRAINT",
                "execution",
                true,
            ),
        );

        let error = job.error.expect("structured job error");
        assert_eq!(error.message, "Failed to insert row");
        assert_eq!(error.code.as_deref(), Some("SQLITE_CONSTRAINT"));
        assert_eq!(error.category.as_deref(), Some("execution"));
        assert!(error.retryable);
        assert_eq!(
            error.details,
            Some(serde_json::json!({ "operation": "transfer", "rowIndex": 7 }))
        );
        assert_eq!(job.progress["current"], Value::from(3));
        assert_eq!(
            job.logs.last().and_then(|entry| entry.details.as_deref()),
            Some("SQLITE_CONSTRAINT")
        );
    }

    #[test]
    fn transfer_progress_matches_web_rounding_and_clamping() {
        assert_eq!(progress(2, 3, "Copying")["percentage"], Value::from(67));
        assert_eq!(progress(8, 4, "Copying")["current"], Value::from(4));
        assert_eq!(progress(0, 0, "Queued")["total"], Value::from(1));
    }

    #[test]
    fn mysql_vendor_codes_match_mysql2_symbolic_codes() {
        assert_eq!(normalize_sqlx_vendor_code("1045"), "ER_ACCESS_DENIED_ERROR");
        assert_eq!(normalize_sqlx_vendor_code("1049"), "ER_BAD_DB_ERROR");
        assert_eq!(normalize_sqlx_vendor_code("1062"), "ER_DUP_ENTRY");
        assert_eq!(normalize_sqlx_vendor_code("23505"), "23505");
    }

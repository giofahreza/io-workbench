    use super::*;
    use std::env;

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

// These tests deliberately hold a native peer UPDATE uncommitted while the
// Workbench mutation begins. An update-before-Save test cannot expose the gap
// between Workbench's optimistic SELECT and its later mutation.
async fn postgres_peer_transaction_mutation_case(delete: bool) {
    let Some(connection) = postgres_test_connection() else {
        return;
    };
    let database = connection
        .profile
        .database_name
        .clone()
        .unwrap_or_else(|| "postgres".into());
    let url = connection_url_for_database(&connection, Some(&database)).unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    let table_name = new_id("iowb_peer_race").replace('-', "_");
    let table_ref = quote_identifier(&table_name);
    sqlx::raw_sql(&format!(
        "CREATE TABLE {table_ref} (id integer PRIMARY KEY, title text NOT NULL); \
         INSERT INTO {table_ref} VALUES (1, 'before')"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let scope = DatabaseTableScopeRequest {
        database_name: Some(database),
        schema_name: Some("public".into()),
        table_name: table_name.clone(),
    };
    let keys = serde_json::json!({"id": 1}).as_object().unwrap().clone();
    let original = select_database_row(&connection, &scope, &keys)
        .await
        .unwrap()
        .unwrap();
    let updates = serde_json::json!({"title": "app-must-not-win"})
        .as_object()
        .unwrap()
        .clone();
    let mut peer = pool.begin().await.unwrap();
    sqlx::query(&format!(
        "UPDATE {table_ref} SET title = 'peer-committed' WHERE id = 1"
    ))
    .execute(&mut *peer)
    .await
    .unwrap();

    let mutation = async {
        tokio::time::timeout(Duration::from_secs(35), async {
            if delete {
                delete_database_row(&connection, &scope, &keys, &original).await
            } else {
                update_database_row(&connection, &scope, &keys, &updates, &original)
                    .await
                    .map(|_| ())
            }
        })
        .await
    };
    let release_peer = async {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut observed_blocked = false;
        while Instant::now() < deadline {
            let blocked: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE pid <> pg_backend_pid() \
                 AND state = 'active' AND wait_event_type = 'Lock' AND query LIKE $1",
            )
            .bind(format!("%{table_name}%"))
            .fetch_one(&pool)
            .await
            .unwrap();
            if blocked > 0 {
                observed_blocked = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        // Always release the owned transaction, including harness failure.
        peer.commit().await.unwrap();
        observed_blocked
    };
    let (outcome, observed_blocked) = tokio::join!(mutation, release_peer);
    let actual: Option<String> =
        sqlx::query_scalar(&format!("SELECT title FROM {table_ref} WHERE id = 1"))
            .fetch_optional(&pool)
            .await
            .unwrap();
    sqlx::query(&format!("DROP TABLE {table_ref}"))
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        observed_blocked,
        "harness must observe actual Workbench lock wait before peer COMMIT"
    );
    let result = outcome.expect("mutation completed after peer COMMIT");
    let error = result.expect_err("concurrent peer mutation must reject the stale snapshot");
    assert_eq!(
        error.status,
        StatusCode::CONFLICT,
        "unrelated database failure: {error:?}"
    );
    assert!(error.body.error.contains("row changed after it was loaded"));
    assert_eq!(
        actual.as_deref(),
        Some("peer-committed"),
        "concurrent peer value/row must survive"
    );
}

#[tokio::test]
async fn postgres_cell_save_during_peer_transaction_does_not_lose_peer_update() {
    postgres_peer_transaction_mutation_case(false).await;
}

#[tokio::test]
async fn postgres_row_delete_during_peer_transaction_does_not_delete_changed_row() {
    postgres_peer_transaction_mutation_case(true).await;
}

#[tokio::test]
async fn postgres_delete_only_role_keeps_atomic_snapshot_protection_without_update_privilege() {
    let Some(connection) = postgres_test_connection() else {
        return;
    };
    let database = connection.profile.database_name.clone().unwrap();
    let url = connection_url_for_database(&connection, Some(&database)).unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    let role_name = new_id("iowb_delete_role").replace('-', "_");
    let table_name = new_id("iowb_delete_only").replace('-', "_");
    let role_ref = quote_identifier(&role_name);
    let table_ref = format!("public.{}", quote_identifier(&table_name));
    let sequence_ref = format!("public.{}", quote_identifier(&format!("{table_name}_seq")));
    let trigger_ref = format!("public.{}", quote_identifier(&format!("{table_name}_fn")));
    let password = new_id("iowb_role_password");
    sqlx::raw_sql(&format!(
        "CREATE ROLE {role_ref} LOGIN PASSWORD '{}'; \
         CREATE TABLE {table_ref}(id integer PRIMARY KEY,title text); \
         INSERT INTO {table_ref} VALUES(1,'before'),(2,'peer-committed'); \
         CREATE SEQUENCE {sequence_ref}; \
         CREATE FUNCTION {trigger_ref}() RETURNS trigger LANGUAGE plpgsql AS $$ \
           BEGIN PERFORM nextval('{sequence_ref}'); RETURN OLD; END $$; \
         CREATE TRIGGER delete_probe BEFORE DELETE ON {table_ref} \
           FOR EACH ROW EXECUTE FUNCTION {trigger_ref}(); \
         GRANT USAGE ON SCHEMA public TO {role_ref}; \
         GRANT USAGE ON SEQUENCE {sequence_ref} TO {role_ref}; \
         GRANT SELECT,DELETE ON {table_ref} TO {role_ref}",
        password,
    ))
    .execute(&pool)
    .await
    .unwrap();
    let mut limited = connection.clone();
    limited.profile.username = Some(role_name.clone());
    limited.password = Some(password);
    limited.profile.has_password = true;
    let scope = DatabaseTableScopeRequest {
        database_name: Some(database),
        schema_name: Some("public".into()),
        table_name,
    };
    let keys = serde_json::json!({"id":1}).as_object().unwrap().clone();
    let original = serde_json::json!({"id":1,"title":"before"})
        .as_object()
        .unwrap()
        .clone();
    let valid_delete = delete_database_row(&limited, &scope, &keys, &original).await;
    let stale_keys = serde_json::json!({"id":2}).as_object().unwrap().clone();
    let stale_original = serde_json::json!({"id":2,"title":"peer-committed"})
        .as_object()
        .unwrap()
        .clone();
    let mut peer = pool.begin().await.unwrap();
    sqlx::query(&format!(
        "UPDATE {table_ref} SET title='peer-late' WHERE id=2"
    ))
    .execute(&mut *peer)
    .await
    .unwrap();
    let mutation = tokio::time::timeout(
        Duration::from_secs(35),
        delete_database_row(&limited, &scope, &stale_keys, &stale_original),
    );
    let release_peer = async {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut observed_blocked = false;
        while Instant::now() < deadline {
            let blocked: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE pid <> pg_backend_pid() \
                 AND state='active' AND wait_event_type='Lock' AND query LIKE $1",
            )
            .bind(format!("%{}%", scope.table_name))
            .fetch_one(&pool)
            .await
            .unwrap();
            if blocked > 0 {
                observed_blocked = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        peer.commit().await.unwrap();
        observed_blocked
    };
    let (stale_delete, observed_blocked) = tokio::join!(mutation, release_peer);
    let remaining: Vec<(i32, String)> =
        sqlx::query_as(&format!("SELECT id,title FROM {table_ref} ORDER BY id"))
            .fetch_all(&pool)
            .await
            .unwrap();
    let trigger_count: (i64, bool) =
        sqlx::query_as(&format!("SELECT last_value,is_called FROM {sequence_ref}"))
            .fetch_one(&pool)
            .await
            .unwrap();
    // Cleanup before assertions, including the first expected-red execution.
    sqlx::raw_sql(&format!(
        "DROP TABLE {table_ref}; DROP FUNCTION {trigger_ref}(); DROP SEQUENCE {sequence_ref}; \
         DROP OWNED BY {role_ref}; DROP ROLE {role_ref}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    valid_delete.expect("SELECT+DELETE must not require an unrelated UPDATE grant");
    assert!(
        observed_blocked,
        "DELETE-only fallback must wait on the actual peer transaction"
    );
    let stale_error = stale_delete
        .expect("mutation completed after peer COMMIT")
        .expect_err("stale snapshot must still reject deletion");
    assert_eq!(stale_error.status, StatusCode::CONFLICT);
    assert!(stale_error
        .body
        .error
        .contains("row changed after it was loaded"));
    assert_eq!(remaining, vec![(2, "peer-late".into())]);
    assert_eq!(trigger_count, (1, true),
        "only the valid DELETE may fire its trigger: a stale DELETE must reject before even nontransactional sequence effects");
}

async fn mysql_peer_transaction_mutation_case(delete: bool) {
    let Some(connection) = mysql_test_connection() else {
        return;
    };
    let database = connection.profile.database_name.clone().unwrap();
    let url = connection_url_for_database(&connection, Some(&database)).unwrap();
    let pool = MySqlPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    let version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&pool)
        .await
        .unwrap();
    let table_name = new_id("iowb_peer_race").replace('-', "_");
    let table_ref = quote_mysql_identifier(&table_name);
    sqlx::query(&format!("CREATE TABLE {table_ref} (id integer PRIMARY KEY, title varchar(64) NOT NULL) ENGINE=InnoDB"))
        .execute(&pool).await.unwrap();
    sqlx::query(&format!("INSERT INTO {table_ref} VALUES (1, 'before')"))
        .execute(&pool)
        .await
        .unwrap();
    let scope = DatabaseTableScopeRequest {
        database_name: Some(database.clone()),
        schema_name: None,
        table_name: table_name.clone(),
    };
    let keys = serde_json::json!({"id": 1}).as_object().unwrap().clone();
    let original = select_database_row(&connection, &scope, &keys)
        .await
        .unwrap()
        .unwrap();
    let updates = serde_json::json!({"title": "app-must-not-win"})
        .as_object()
        .unwrap()
        .clone();
    let mut peer = pool.begin().await.unwrap();
    let peer_id: u64 = sqlx::query_scalar("SELECT CONNECTION_ID()")
        .fetch_one(&mut *peer)
        .await
        .unwrap();
    sqlx::query(&format!(
        "UPDATE {table_ref} SET title = 'peer-committed' WHERE id = 1"
    ))
    .execute(&mut *peer)
    .await
    .unwrap();
    let mutation = async {
        tokio::time::timeout(Duration::from_secs(35), async {
            if delete {
                delete_database_row(&connection, &scope, &keys, &original).await
            } else {
                update_database_row(&connection, &scope, &keys, &updates, &original)
                    .await
                    .map(|_| ())
            }
        })
        .await
    };
    let release_peer = async {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut observed_blocked = false;
        while Instant::now() < deadline {
            // Real engine lock-wait views, not timing or merely an active query.
            let blocked: i64 = if version.to_lowercase().contains("mariadb") {
                sqlx::query_scalar(
                    "SELECT count(*) FROM information_schema.INNODB_LOCK_WAITS w \
                     JOIN information_schema.INNODB_TRX t ON t.trx_id=w.blocking_trx_id \
                     WHERE t.trx_mysql_thread_id=?",
                )
                .bind(peer_id)
                .fetch_one(&pool)
                .await
                .unwrap()
            } else {
                sqlx::query_scalar(
                    "SELECT count(*) FROM performance_schema.data_lock_waits w \
                     JOIN performance_schema.threads t ON t.THREAD_ID=w.BLOCKING_THREAD_ID \
                     WHERE t.PROCESSLIST_ID=?",
                )
                .bind(peer_id)
                .fetch_one(&pool)
                .await
                .unwrap()
            };
            if blocked > 0 {
                observed_blocked = true;
                break;
            }
            // MariaDB trx0i_s.cc refreshes the shared INNODB_* cache only
            // after 100 ms without a read. Faster polling perpetually serves
            // the initial empty cache, even while the real row lock is held.
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        peer.commit().await.unwrap();
        observed_blocked
    };
    let (outcome, observed_blocked) = tokio::join!(mutation, release_peer);
    let actual: Option<String> =
        sqlx::query_scalar(&format!("SELECT title FROM {table_ref} WHERE id=1"))
            .fetch_optional(&pool)
            .await
            .unwrap();
    sqlx::query(&format!("DROP TABLE {table_ref}"))
        .execute(&pool)
        .await
        .unwrap();
    let result = outcome.expect("mutation completed after peer COMMIT");
    assert!(
        observed_blocked,
        "harness must observe engine lock wait before peer COMMIT; outcome={result:?}, actual={actual:?}"
    );
    let error = result.expect_err("concurrent peer mutation must reject the stale snapshot");
    assert_eq!(
        error.status,
        StatusCode::CONFLICT,
        "unrelated database failure: {error:?}"
    );
    assert!(error.body.error.contains("row changed after it was loaded"));
    assert_eq!(actual.as_deref(), Some("peer-committed"));
}

#[tokio::test]
async fn mysql_cell_save_during_peer_transaction_does_not_lose_peer_update() {
    mysql_peer_transaction_mutation_case(false).await;
}

#[tokio::test]
async fn mysql_row_delete_during_peer_transaction_does_not_delete_changed_row() {
    mysql_peer_transaction_mutation_case(true).await;
}

#[tokio::test]
async fn mysql_nontransactional_row_mutations_are_rejected_without_writes() {
    let Some(connection) = mysql_test_connection() else {
        return;
    };
    let database = connection.profile.database_name.clone().unwrap();
    let url = connection_url_for_database(&connection, Some(&database)).unwrap();
    let pool = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let table_name = new_id("iowb_unsafe_engine").replace('-', "_");
    let table_ref = quote_mysql_identifier(&table_name);
    sqlx::query(&format!(
        "CREATE TABLE {table_ref}(id integer PRIMARY KEY,title varchar(64)) ENGINE=MyISAM"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(&format!("INSERT INTO {table_ref} VALUES(1,'unchanged')"))
        .execute(&pool)
        .await
        .unwrap();
    let scope = DatabaseTableScopeRequest {
        database_name: Some(database),
        schema_name: None,
        table_name,
    };
    let keys = serde_json::json!({"id":1}).as_object().unwrap().clone();
    let original = select_database_row(&connection, &scope, &keys)
        .await
        .unwrap()
        .unwrap();
    let updates = serde_json::json!({"title":"must-not-write"})
        .as_object()
        .unwrap()
        .clone();
    let update = update_database_row(&connection, &scope, &keys, &updates, &original).await;
    let delete = delete_database_row(&connection, &scope, &keys, &original).await;
    let actual: Option<String> =
        sqlx::query_scalar(&format!("SELECT title FROM {table_ref} WHERE id=1"))
            .fetch_optional(&pool)
            .await
            .unwrap();
    sqlx::query(&format!("DROP TABLE {table_ref}"))
        .execute(&pool)
        .await
        .unwrap();
    assert!(update.is_err(), "nontransactional UPDATE must be rejected");
    assert!(delete.is_err(), "nontransactional DELETE must be rejected");
    assert_eq!(actual.as_deref(), Some("unchanged"));
}

#[tokio::test]
async fn sqlite_atomic_row_mutation_unchanged_and_stale_controls() {
    let path = env::temp_dir().join(format!("{}.sqlite", new_id("iowb_atomic_rows")));
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE records(id INTEGER PRIMARY KEY,title TEXT,payload TEXT); \
                      INSERT INTO records VALUES(1,'before','preserved');",
    )
    .unwrap();
    let connection = sqlite_test_connection(&path);
    let scope = DatabaseTableScopeRequest {
        database_name: Some("main".into()),
        schema_name: None,
        table_name: "records".into(),
    };
    let keys = serde_json::json!({"id":1}).as_object().unwrap().clone();
    let original = select_database_row(&connection, &scope, &keys)
        .await
        .unwrap()
        .unwrap();
    let updates = serde_json::json!({"id":999,"title":"updated"})
        .as_object()
        .unwrap()
        .clone();
    let first = update_database_row(&connection, &scope, &keys, &updates, &original).await;
    let stale_save = update_database_row(&connection, &scope, &keys, &updates, &original).await;
    let stale_delete = delete_database_row(&connection, &scope, &keys, &original).await;
    let current = select_database_row(&connection, &scope, &keys)
        .await
        .unwrap()
        .unwrap();
    let unchanged_delete = delete_database_row(&connection, &scope, &keys, &current).await;
    let absent = select_database_row(&connection, &scope, &keys)
        .await
        .unwrap();
    let missing_update = update_database_row(&connection, &scope, &keys, &updates, &current).await;
    let missing_delete = delete_database_row(&connection, &scope, &keys, &current).await;
    drop(db);
    std::fs::remove_file(path).unwrap();
    let first_row = first.unwrap().unwrap();
    assert_eq!(first_row["title"], "updated");
    assert_eq!(
        first_row["id"], 1,
        "the existing primary-key editing guard must remain in force"
    );
    assert_eq!(
        first_row["payload"], "preserved",
        "readback must retain untouched values"
    );
    assert!(stale_save.is_err() && stale_delete.is_err());
    assert_eq!(current["payload"], "preserved");
    unchanged_delete.unwrap();
    assert!(absent.is_none());
    assert_eq!(missing_update.unwrap_err().status, StatusCode::NOT_FOUND);
    assert_eq!(missing_delete.unwrap_err().status, StatusCode::NOT_FOUND);
}

async fn live_missing_row_mutations_case(connection: StoredDatabaseConnection) {
    let database = connection.profile.database_name.clone().unwrap();
    let table_name = new_id("iowb_missing_atomic").replace('-', "_");
    let scope = DatabaseTableScopeRequest {
        database_name: Some(database.clone()),
        schema_name: (connection.profile.db_type == SupportedDatabaseType::Postgresql)
            .then(|| "public".into()),
        table_name,
    };
    let endpoint = database_scope_endpoint(&connection, &scope);
    let table_ref = transfer_table_ref(connection.profile.db_type, &endpoint);
    let engine = if connection.profile.db_type == SupportedDatabaseType::Postgresql {
        ""
    } else {
        " ENGINE=InnoDB"
    };
    execute_database_mutation(
        &connection,
        Some(&database),
        &format!("CREATE TABLE {table_ref}(id integer PRIMARY KEY,title varchar(64)){engine}"),
    )
    .await
    .unwrap();
    let keys = serde_json::json!({"id":1}).as_object().unwrap().clone();
    let values = serde_json::json!({"title":"absent"})
        .as_object()
        .unwrap()
        .clone();
    let edit = update_database_row(&connection, &scope, &keys, &values, &values).await;
    let delete = delete_database_row(&connection, &scope, &keys, &values).await;
    execute_database_mutation(
        &connection,
        Some(&database),
        &format!("DROP TABLE {table_ref}"),
    )
    .await
    .unwrap();
    for error in [edit.unwrap_err(), delete.unwrap_err()] {
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.body.error, "Row not found");
    }
}

#[tokio::test]
async fn postgres_atomic_missing_row_edit_and_delete_return_not_found() {
    if let Some(connection) = postgres_test_connection() {
        live_missing_row_mutations_case(connection).await;
    }
}

#[tokio::test]
async fn mysql_atomic_missing_row_edit_and_delete_return_not_found() {
    if let Some(connection) = mysql_test_connection() {
        live_missing_row_mutations_case(connection).await;
    }
}

#[tokio::test]
async fn sqlite_bulk_delete_snapshots_keep_input_indices_and_partial_results() {
    let path = env::temp_dir().join(format!("{}.sqlite", new_id("iowb_bulk_snapshots")));
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE records(id INTEGER PRIMARY KEY,title TEXT); \
        INSERT INTO records VALUES(1,'peer-new'),(2,'two'),(3,'three');",
    )
    .unwrap();
    let connection = sqlite_test_connection(&path);
    let request: DatabaseRowsDeleteRequest = serde_json::from_value(serde_json::json!({
        "tableName":"records", "primaryKeyRows":[{"id":3},{"id":1},{"id":2}],
        "originalRows":[{"id":3,"title":"three"},{"id":1,"title":"old"},{"id":2,"title":"two"}]
    }))
    .unwrap();
    let result = delete_database_rows_with_snapshots(&connection, &request).await;
    let remaining: Vec<(i64, String)> = db
        .prepare("SELECT id,title FROM records ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    drop(db);
    std::fs::remove_file(path).unwrap();
    let (deleted, failures) = result.unwrap();
    assert_eq!(deleted, 2);
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0]["rowIndex"], 1,
        "failure index belongs to input position, not primary key"
    );
    assert!(failures[0]["message"]
        .as_str()
        .unwrap()
        .contains("row changed after it was loaded"));
    assert_eq!(remaining, vec![(1, "peer-new".into())]);
}

#[tokio::test]
async fn sqlite_bulk_delete_rejects_misaligned_snapshots_before_any_write_and_keeps_legacy_route() {
    let path = env::temp_dir().join(format!("{}.sqlite", new_id("iowb_bulk_validation")));
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE records(id INTEGER PRIMARY KEY,title TEXT); INSERT INTO records VALUES(1,'one'),(2,'two');").unwrap();
    let connection = sqlite_test_connection(&path);
    for snapshots in [
        serde_json::json!([]),
        serde_json::json!([{"id":1}]),
        serde_json::json!([{"id":1},{}]),
    ] {
        let request: DatabaseRowsDeleteRequest = serde_json::from_value(serde_json::json!({
            "tableName":"records", "primaryKeyRows":[{"id":1},{"id":2}], "originalRows":snapshots
        }))
        .unwrap();
        let error = delete_database_rows_with_snapshots(&connection, &request)
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.body.error.contains("originalRows"));
        assert_eq!(
            db.query_row("SELECT count(*) FROM records", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
    let legacy: DatabaseRowsDeleteRequest = serde_json::from_value(serde_json::json!({
        "tableName":"records", "primaryKeyRows":[{"id":2}]
    }))
    .unwrap();
    assert!(legacy.original_rows.is_none());
    let result = delete_database_rows_with_snapshots(&connection, &legacy)
        .await
        .unwrap();
    assert_eq!(
        result,
        (1, vec![]),
        "legacy omission remains an explicitly blind delete"
    );
    assert_eq!(
        db.query_row("SELECT id FROM records", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(db);
    std::fs::remove_file(path).unwrap();
}

fn unsigned_transfer_column(kind: &str) -> DatabaseObjectColumn {
    serde_json::from_value(serde_json::json!({
        "name": "amount", "nativeType": kind, "nullable": false
    }))
    .unwrap()
}

#[test]
fn unsigned_schema_transfer_preserves_full_integer_ranges() {
    for (native, mysql, postgres, sqlite) in [
        ("tinyint unsigned", "TINYINT UNSIGNED", "INTEGER", "INTEGER"),
        (
            "smallint(5) unsigned",
            "SMALLINT UNSIGNED",
            "INTEGER",
            "INTEGER",
        ),
        (
            "mediumint unsigned",
            "MEDIUMINT UNSIGNED",
            "INTEGER",
            "INTEGER",
        ),
        ("int(10) unsigned", "INT UNSIGNED", "BIGINT", "INTEGER"),
        ("integer unsigned", "INT UNSIGNED", "BIGINT", "INTEGER"),
        (
            "BIGINT(20) UNSIGNED",
            "BIGINT UNSIGNED",
            "NUMERIC(20, 0)",
            "TEXT",
        ),
    ] {
        let column = unsigned_transfer_column(native);
        for engine in [SupportedDatabaseType::Mysql, SupportedDatabaseType::Mariadb] {
            assert_eq!(transfer_column_type(engine, &column), mysql, "{native}");
        }
        assert_eq!(
            transfer_column_type(SupportedDatabaseType::Postgresql, &column),
            postgres,
            "{native}"
        );
        assert_eq!(
            transfer_column_type(SupportedDatabaseType::Sqlite, &column),
            sqlite,
            "{native}"
        );
    }
    for engine in [
        SupportedDatabaseType::Mysql,
        SupportedDatabaseType::Mariadb,
        SupportedDatabaseType::Postgresql,
    ] {
        assert_eq!(
            transfer_column_type(engine, &unsigned_transfer_column("bigint")),
            "BIGINT"
        );
    }
    assert_eq!(
        transfer_column_type(
            SupportedDatabaseType::Sqlite,
            &unsigned_transfer_column("bigint")
        ),
        "INTEGER"
    );
}

#[test]
fn unsigned_schema_transfer_sqlite_stores_exact_decimal_text_not_real() {
    let source = unsigned_transfer_column("bigint unsigned");
    let target = unsigned_transfer_column("TEXT");
    let endpoint: DatabaseTransferEndpoint = serde_json::from_value(serde_json::json!({
        "connectionId": 2, "tableName": "unsigned_values"
    }))
    .unwrap();
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(&build_transfer_create_table_sql(
        SupportedDatabaseType::Sqlite,
        &endpoint,
        &[source.clone()],
    ))
    .unwrap();
    for value in [0_u64, i64::MAX as u64, 9223372036854775808, u64::MAX] {
        for input in [Value::from(value), Value::String(value.to_string())] {
            let mapped = transfer_value_for_column_mapping(Some(&source), &target, input);
            assert_eq!(mapped, Value::String(value.to_string()));
            let row = Map::from_iter([("amount".to_string(), mapped)]);
            db.execute(
                &build_transfer_insert_sql(
                    SupportedDatabaseType::Sqlite,
                    &endpoint,
                    &[target.clone()],
                    &row,
                ),
                [],
            )
            .unwrap();
            let actual: (String, String) = db
                .query_row(
                    "SELECT amount,typeof(amount) FROM unsigned_values ORDER BY rowid DESC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(actual, (value.to_string(), "text".to_string()));
        }
    }
    assert_eq!(
        transfer_value_for_column_mapping(Some(&source), &target, Value::Null),
        Value::Null
    );
    let signed = unsigned_transfer_column("bigint");
    assert_eq!(
        transfer_value_for_column_mapping(Some(&signed), &target, Value::from(7)),
        Value::from(7)
    );
}

#[tokio::test]
async fn unsigned_schema_transfer_mysql_accepts_exact_u64_boundaries() {
    let Some(connection) = mysql_test_connection() else {
        return;
    };
    let pool = MySqlPoolOptions::new()
        .max_connections(1)
        .connect(
            &connection_url_for_database(&connection, connection.profile.database_name.as_deref())
                .unwrap(),
        )
        .await
        .unwrap();
    let endpoint: DatabaseTransferEndpoint = serde_json::from_value(serde_json::json!({
        "connectionId": -2, "tableName": new_id("iowb_unsigned").replace('-', "_")
    }))
    .unwrap();
    let column = unsigned_transfer_column("bigint unsigned");
    let ddl =
        build_transfer_create_table_sql(SupportedDatabaseType::Mysql, &endpoint, &[column.clone()])
            .replacen("CREATE TABLE", "CREATE TEMPORARY TABLE", 1);
    pool.execute(ddl.as_str()).await.unwrap();
    for value in [9223372036854775808_u64, u64::MAX] {
        let row = Map::from_iter([("amount".to_string(), Value::from(value))]);
        pool.execute(
            build_transfer_insert_sql(
                SupportedDatabaseType::Mysql,
                &endpoint,
                &[column.clone()],
                &row,
            )
            .as_str(),
        )
        .await
        .unwrap();
    }
    let actual: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT CAST(amount AS CHAR) FROM {} ORDER BY amount",
        transfer_table_ref(SupportedDatabaseType::Mysql, &endpoint)
    ))
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(actual, ["9223372036854775808", "18446744073709551615"]);
    pool.close().await; // temporary fixture is scoped to this one connection
}

#[tokio::test]
async fn unsigned_schema_transfer_postgres_accepts_exact_u64_boundaries() {
    let Some(connection) = postgres_test_connection() else {
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(
            &connection_url_for_database(&connection, connection.profile.database_name.as_deref())
                .unwrap(),
        )
        .await
        .unwrap();
    let endpoint: DatabaseTransferEndpoint = serde_json::from_value(serde_json::json!({
        "connectionId": -1, "schemaName": "pg_temp", "tableName": new_id("iowb_unsigned").replace('-', "_")
    })).unwrap();
    let column = unsigned_transfer_column("bigint unsigned");
    let ddl = build_transfer_create_table_sql(
        SupportedDatabaseType::Postgresql,
        &endpoint,
        &[column.clone()],
    )
    .replacen("CREATE TABLE", "CREATE TEMPORARY TABLE", 1);
    pool.execute(ddl.as_str()).await.unwrap();
    for value in [9223372036854775808_u64, u64::MAX] {
        let row = Map::from_iter([("amount".to_string(), Value::from(value))]);
        pool.execute(
            build_transfer_insert_sql(
                SupportedDatabaseType::Postgresql,
                &endpoint,
                &[column.clone()],
                &row,
            )
            .as_str(),
        )
        .await
        .unwrap();
    }
    let actual: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT amount::text FROM {} ORDER BY amount::numeric",
        transfer_table_ref(SupportedDatabaseType::Postgresql, &endpoint)
    ))
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(actual, ["9223372036854775808", "18446744073709551615"]);
    pool.close().await;
}

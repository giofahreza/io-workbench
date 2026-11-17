struct PostgresMutationPermissionFixture {
    connection: StoredDatabaseConnection,
    pool: PgPool,
    table: String,
    role: String,
    scope: DatabaseTableScopeRequest,
}

impl PostgresMutationPermissionFixture {
    async fn new(primary_key: bool) -> Option<Self> {
        let admin = postgres_test_connection()?;
        let database = admin.profile.database_name.clone().unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&connection_url_for_database(&admin, Some(&database)).unwrap())
            .await
            .unwrap();
        let table = new_id("iowb_privilege").replace('-', "_");
        let role = format!("{table}_r");
        let password = new_id("test_password");
        let key = if primary_key { " PRIMARY KEY" } else { "" };
        sqlx::raw_sql(&format!(
            "CREATE TABLE public.\"{table}\"(id integer{key},title text NOT NULL); \
             INSERT INTO public.\"{table}\" VALUES(1,'before'); \
             CREATE ROLE \"{role}\" LOGIN PASSWORD '{password}'; \
             GRANT USAGE ON SCHEMA public TO \"{role}\"; \
             GRANT SELECT,UPDATE,DELETE ON public.\"{table}\" TO \"{role}\""
        ))
        .execute(&pool)
        .await
        .unwrap();
        let mut connection = admin;
        connection.profile.username = Some(role.clone());
        connection.profile.has_password = true;
        connection.password = Some(password);
        Some(Self {
            connection,
            pool,
            table: table.clone(),
            role,
            scope: DatabaseTableScopeRequest {
                database_name: Some(database),
                schema_name: Some("public".into()),
                table_name: table,
            },
        })
    }

    async fn execute(&self, sql: &str) {
        sqlx::raw_sql(sql).execute(&self.pool).await.unwrap();
    }

    async fn title(&self) -> String {
        sqlx::query_scalar(&format!(
            "SELECT title FROM public.\"{}\" WHERE id=1",
            self.table
        ))
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn cleanup(self) {
        self.execute(&format!(
            "DROP TABLE public.\"{}\"; REVOKE USAGE ON SCHEMA public FROM \"{}\"; DROP ROLE \"{}\"",
            self.table, self.role, self.role,
        ))
        .await;
        self.pool.close().await;
    }
}

#[tokio::test]
async fn postgres_revoked_mutations_report_permissions_not_missing_primary_key() {
    let Some(fixture) = PostgresMutationPermissionFixture::new(true).await else {
        return;
    };
    let keys = serde_json::json!({"id":1}).as_object().unwrap().clone();
    let original = select_database_row(&fixture.connection, &fixture.scope, &keys)
        .await
        .unwrap()
        .unwrap();
    let updates = serde_json::json!({"title":"must-not-save"})
        .as_object()
        .unwrap()
        .clone();
    fixture
        .execute(&format!(
            "REVOKE UPDATE,DELETE ON public.\"{}\" FROM \"{}\"",
            fixture.table, fixture.role
        ))
        .await;
    let metadata = database_table_metadata(
        &fixture.connection,
        fixture.scope.database_name.as_deref(),
        Some("public"),
        &fixture.table,
    )
    .await
    .unwrap();
    assert!(
        metadata.primary_key.is_empty(),
        "SELECT-only metadata/UI guard is retained"
    );
    let update_error = update_database_row(
        &fixture.connection,
        &fixture.scope,
        &keys,
        &updates,
        &original,
    )
    .await
    .unwrap_err();
    let delete_error = delete_database_row(&fixture.connection, &fixture.scope, &keys, &original)
        .await
        .unwrap_err();
    let title = fixture.title().await;
    fixture.cleanup().await;
    for (error, operation) in [(update_error, "UPDATE"), (delete_error, "DELETE")] {
        assert_eq!(error.status, StatusCode::FORBIDDEN, "{error:?}");
        assert!(error.body.error.contains("Permission denied"));
        assert!(error.body.error.contains(operation));
        assert!(!error.body.error.contains("no primary key"));
    }
    assert_eq!(title, "before");
}

#[tokio::test]
async fn postgres_column_update_grant_is_not_reported_as_denied() {
    let Some(fixture) = PostgresMutationPermissionFixture::new(true).await else {
        return;
    };
    fixture.execute(&format!("REVOKE UPDATE,DELETE ON public.\"{}\" FROM \"{}\"; GRANT UPDATE(title) ON public.\"{}\" TO \"{}\"", fixture.table, fixture.role, fixture.table, fixture.role)).await;
    let allowed = ensure_postgres_mutation_privilege_when_primary_key_hidden(
        &fixture.connection,
        &fixture.scope,
        true,
    )
    .await;
    let keys = serde_json::json!({"id":1}).as_object().unwrap().clone();
    let original = select_database_row(&fixture.connection, &fixture.scope, &keys)
        .await
        .unwrap()
        .unwrap();
    let updates = serde_json::json!({"title":"column-granted"})
        .as_object()
        .unwrap()
        .clone();
    let result = update_database_row(
        &fixture.connection,
        &fixture.scope,
        &keys,
        &updates,
        &original,
    )
    .await;
    let title = fixture.title().await;
    fixture.cleanup().await;
    assert!(allowed.is_ok(), "{allowed:?}");
    // Column-only UPDATE is sufficient for this real primary-key table.
    // Reject misleading no-PK errors as well as permission-denied errors.
    let updated = result.expect("Column-granted UPDATE must succeed");
    assert_eq!(
        updated,
        Some(
            serde_json::json!({"id": 1, "title": "column-granted"})
                .as_object()
                .unwrap()
                .clone()
        )
    );
    assert_eq!(title, "column-granted");
}

#[tokio::test]
async fn postgres_genuine_missing_primary_key_keeps_schema_diagnostic() {
    let Some(fixture) = PostgresMutationPermissionFixture::new(false).await else {
        return;
    };
    let keys = serde_json::json!({"id":1}).as_object().unwrap().clone();
    let updates = serde_json::json!({"title":"must-not-save"})
        .as_object()
        .unwrap()
        .clone();
    let original = serde_json::json!({"id":1,"title":"before"})
        .as_object()
        .unwrap()
        .clone();
    let update_error = update_database_row(
        &fixture.connection,
        &fixture.scope,
        &keys,
        &updates,
        &original,
    )
    .await
    .unwrap_err();
    let delete_error = delete_database_row(&fixture.connection, &fixture.scope, &keys, &original)
        .await
        .unwrap_err();
    let title = fixture.title().await;
    fixture.cleanup().await;
    for error in [update_error, delete_error] {
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.body.error.contains("no primary key"));
    }
    assert_eq!(title, "before");
}

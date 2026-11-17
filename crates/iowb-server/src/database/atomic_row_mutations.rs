/// Lock and verify the loaded snapshot on the same connection that writes it.
/// A separate SELECT/check followed by an autocommit UPDATE/DELETE can lose a
/// peer's change while the mutation waits for that peer's uncommitted row lock.
async fn mutate_database_row_atomically(
    connection: &StoredDatabaseConnection,
    scope: &DatabaseTableScopeRequest,
    primary_keys: &Map<String, Value>,
    original_values: &Map<String, Value>,
    mutation_sql: &str,
    return_row: bool,
) -> Result<Option<Map<String, Value>>> {
    validate_database_mutation_text(connection.profile.db_type, mutation_sql)?;
    let endpoint = database_scope_endpoint(connection, scope);
    let select_sql = format!(
        "SELECT * FROM {} WHERE {} LIMIT 1",
        transfer_table_ref(connection.profile.db_type, &endpoint),
        build_database_where_clause(connection.profile.db_type, primary_keys),
    );
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let mut database = sqlite_connection(connection)?;
        // Acquire the write reservation before reading the snapshot. A deferred
        // read transaction cannot safely upgrade after another writer commits.
        let transaction = database
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(sqlite_server_error)?;
        let current = select_sqlite_atomic_row(&transaction, &select_sql)?
            .ok_or_else(database_atomic_row_missing)?;
        ensure_row_is_current(&current, original_values)?;
        let affected = transaction
            .execute(mutation_sql, [])
            .map_err(sqlite_server_error)?;
        ensure_atomic_row_count(affected as u64, return_row, false)?;
        let result = if return_row {
            select_sqlite_atomic_row(&transaction, &select_sql)?
        } else {
            None
        };
        transaction.commit().map_err(sqlite_server_error)?;
        return Ok(result);
    }

    let locked_select_sql = format!("{select_sql} FOR UPDATE");
    match typed_live_pool_for_database(connection, scope.database_name.as_deref()).await? {
        TypedLivePool::Postgresql(pool) => {
            let mut transaction = pool.begin().await.map_err(sqlx_server_error)?;
            // PostgreSQL requires an UPDATE grant for SELECT FOR UPDATE even
            // when the requested operation is DELETE. A SELECT+DELETE role
            // can instead lock the relation before reading. This broader lock
            // is only the fallback for that role: ordinary writers retain
            // row-level concurrency, and no DELETE/trigger runs before the
            // snapshot has been checked against committed peer changes.
            let table_ref = transfer_table_ref(connection.profile.db_type, &endpoint);
            let needs_delete_table_lock = !return_row
                && !sqlx::query_scalar::<_, bool>(
                    "SELECT has_table_privilege($1::regclass, 'UPDATE') OR \
                     has_any_column_privilege($1::regclass, 'UPDATE')",
                )
                .bind(&table_ref)
                .fetch_one(&mut *transaction)
                .await
                .map_err(sqlx_server_error)?;
            let postgres_select_sql = if needs_delete_table_lock {
                (&mut *transaction)
                    .execute(sqlx::raw_sql(&format!(
                        "LOCK TABLE {table_ref} IN EXCLUSIVE MODE"
                    )))
                    .await
                    .map_err(sqlx_server_error)?;
                &select_sql
            } else {
                &locked_select_sql
            };
            let current = (&mut *transaction)
                .fetch_all(sqlx::raw_sql(postgres_select_sql))
                .await
                .map_err(sqlx_server_error)?
                .into_iter()
                .next()
                .map(|row| postgres_row_to_json_map(&row))
                .ok_or_else(database_atomic_row_missing)?;
            // Keep the existing typed raw-query decoder, including exact
            // decimals, arrays, JSON and encoded binary snapshot values.
            ensure_row_is_current(&current, original_values)?;
            let affected = (&mut *transaction)
                .execute(sqlx::raw_sql(mutation_sql))
                .await
                .map_err(sqlx_server_error)?
                .rows_affected();
            ensure_atomic_row_count(affected, return_row, false)?;
            let result = if return_row {
                (&mut *transaction)
                    .fetch_all(sqlx::raw_sql(&select_sql))
                    .await
                    .map_err(sqlx_server_error)?
                    .into_iter()
                    .next()
                    .map(|row| postgres_row_to_json_map(&row))
            } else {
                None
            };
            transaction.commit().await.map_err(sqlx_server_error)?;
            Ok(result)
        }
        TypedLivePool::Mysql(pool) => {
            let mut transaction = pool.begin().await.map_err(sqlx_server_error)?;
            let current = (&mut *transaction)
                .fetch_all(sqlx::raw_sql(&locked_select_sql))
                .await
                .map_err(sqlx_server_error)?
                .into_iter()
                .next()
                .map(|row| mysql_row_to_json_map(&row));
            // The SELECT holds a metadata lock for this transaction, preventing
            // ALTER TABLE from swapping engines between this check and write.
            // Do not promise rollback/row-lock safety for MyISAM or MEMORY.
            let engine: Option<String> = sqlx::query_scalar::<_, Option<String>>(
                "SELECT ENGINE FROM information_schema.TABLES WHERE BINARY TABLE_SCHEMA = BINARY ? AND BINARY TABLE_NAME = BINARY ?",
            )
            .bind(endpoint.database_name.as_deref().unwrap_or_default())
            .bind(&scope.table_name)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(sqlx_server_error)?
            .flatten();
            if !engine
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("InnoDB"))
            {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Safe row editing and deletion require an InnoDB table. This table's engine cannot guarantee an atomic snapshot check; use an explicit SQL operation instead.",
                ));
            }
            let current = current.ok_or_else(database_atomic_row_missing)?;
            ensure_row_is_current(&current, original_values)?;
            let affected = (&mut *transaction)
                .execute(sqlx::raw_sql(mutation_sql))
                .await
                .map_err(sqlx_server_error)?
                .rows_affected();
            // Some MySQL clients report changed rather than matched rows for
            // a no-op UPDATE. The matching row was already locked and checked.
            ensure_atomic_row_count(affected, return_row, true)?;
            let result = if return_row {
                (&mut *transaction)
                    .fetch_all(sqlx::raw_sql(&select_sql))
                    .await
                    .map_err(sqlx_server_error)?
                    .into_iter()
                    .next()
                    .map(|row| mysql_row_to_json_map(&row))
            } else {
                None
            };
            transaction.commit().await.map_err(sqlx_server_error)?;
            Ok(result)
        }
    }
}

fn database_atomic_row_missing() -> ServerError {
    ServerError::new(StatusCode::NOT_FOUND, "Row not found")
}

fn ensure_atomic_row_count(affected: u64, update: bool, allow_noop_update: bool) -> Result<()> {
    if affected == 1 || (affected == 0 && update && allow_noop_update) {
        return Ok(());
    }
    // This also rolls back a possible multi-row match if primary-key metadata
    // was changed before the transaction obtained its relation/metadata lock.
    Err(ServerError::new(
        StatusCode::CONFLICT,
        "The row could not be changed as one record. Refresh its data and table metadata before trying again.",
    ))
}

fn select_sqlite_atomic_row(
    database: &Connection,
    sql: &str,
) -> Result<Option<Map<String, Value>>> {
    let mut statement = database.prepare(sql).map_err(sqlite_server_error)?;
    let column_names = statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut rows = statement.query([]).map_err(sqlite_server_error)?;
    rows.next()
        .map_err(sqlite_server_error)?
        .map(|row| row_to_json_map(row, &column_names))
        .transpose()
}

// Compile-time regression for SQLx 0.8 raw-query lifetime/Send constraints.
#[cfg(test)]
#[allow(dead_code)]
fn assert_atomic_mutation_future_is_send(
    connection: &StoredDatabaseConnection,
    scope: &DatabaseTableScopeRequest,
    values: &Map<String, Value>,
) {
    fn check<T: Send>(_: T) {}
    check(mutate_database_row_atomically(
        connection, scope, values, values, "", true,
    ));
}

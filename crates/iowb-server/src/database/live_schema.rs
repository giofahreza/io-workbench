async fn live_pool(connection: &StoredDatabaseConnection) -> Result<AnyPool> {
    live_pool_for_database(connection, None).await
}

async fn live_pool_for_database(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
) -> Result<AnyPool> {
    SQLX_ANY_DRIVERS.call_once(sqlx::any::install_default_drivers);
    let url = connection_url_for_database(connection, database_name)?;
    if connection.profile.id <= 0 {
        return connect_live_pool(&url).await;
    }

    let cache_key = format!("{}:{url}", connection.profile.id);
    let now = Instant::now();
    {
        let mut cache = live_pool_cache().lock().await;
        cache.retain(|_, entry| {
            !entry.pool.is_closed()
                && now.saturating_duration_since(entry.last_used) <= LIVE_POOL_CACHE_TTL
        });
        if let Some(entry) = cache.get_mut(&cache_key) {
            entry.last_used = now;
            return Ok(entry.pool.clone());
        }
    }

    let pool = connect_live_pool(&url).await?;
    let mut cache = live_pool_cache().lock().await;
    if let Some(entry) = cache.get_mut(&cache_key) {
        entry.last_used = now;
        return Ok(entry.pool.clone());
    }
    if cache.len() >= LIVE_POOL_CACHE_MAX_ENTRIES {
        if let Some(oldest_key) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest_key);
        }
    }
    cache.insert(
        cache_key,
        CachedLivePool {
            connection_id: connection.profile.id,
            pool: pool.clone(),
            last_used: now,
        },
    );
    Ok(pool)
}

async fn connect_live_pool(url: &str) -> Result<AnyPool> {
    AnyPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Some(LIVE_POOL_CACHE_TTL))
        .connect(url)
        .await
        .map_err(sqlx_server_error)
}

fn live_pool_cache() -> &'static Mutex<HashMap<String, CachedLivePool>> {
    LIVE_POOL_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn typed_live_pool_for_database(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
) -> Result<TypedLivePool> {
    let url = connection_url_for_database(connection, database_name)?;
    if connection.profile.id <= 0 {
        return connect_typed_live_pool(connection.profile.db_type, &url).await;
    }

    let cache_key = format!("{}:{url}", connection.profile.id);
    let now = Instant::now();
    {
        let mut cache = typed_live_pool_cache().lock().await;
        cache.retain(|_, entry| {
            !entry.pool.is_closed()
                && now.saturating_duration_since(entry.last_used) <= LIVE_POOL_CACHE_TTL
        });
        if let Some(entry) = cache.get_mut(&cache_key) {
            entry.last_used = now;
            return Ok(entry.pool.clone());
        }
    }

    let pool = connect_typed_live_pool(connection.profile.db_type, &url).await?;
    let mut cache = typed_live_pool_cache().lock().await;
    if let Some(entry) = cache.get_mut(&cache_key) {
        entry.last_used = now;
        return Ok(entry.pool.clone());
    }
    if cache.len() >= LIVE_POOL_CACHE_MAX_ENTRIES {
        if let Some(oldest_key) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest_key);
        }
    }
    cache.insert(
        cache_key,
        CachedTypedLivePool {
            connection_id: connection.profile.id,
            pool: pool.clone(),
            last_used: now,
        },
    );
    Ok(pool)
}

async fn connect_typed_live_pool(
    db_type: SupportedDatabaseType,
    url: &str,
) -> Result<TypedLivePool> {
    match db_type {
        SupportedDatabaseType::Postgresql => PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Some(LIVE_POOL_CACHE_TTL))
            .connect(url)
            .await
            .map(TypedLivePool::Postgresql)
            .map_err(sqlx_server_error),
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => MySqlPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Some(LIVE_POOL_CACHE_TTL))
            .connect(url)
            .await
            .map(TypedLivePool::Mysql)
            .map_err(sqlx_server_error),
        SupportedDatabaseType::Sqlite => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "SQLite does not use a live SQLx pool",
        )),
    }
}

fn typed_live_pool_cache() -> &'static Mutex<HashMap<String, CachedTypedLivePool>> {
    TYPED_LIVE_POOL_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn evict_live_pools(connection_id: i64) {
    if let Some(cache) = LIVE_POOL_CACHE.get() {
        cache
            .lock()
            .await
            .retain(|_, entry| entry.connection_id != connection_id);
    }
    if let Some(cache) = TYPED_LIVE_POOL_CACHE.get() {
        cache
            .lock()
            .await
            .retain(|_, entry| entry.connection_id != connection_id);
    }
}

fn connection_url_for_database(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
) -> Result<String> {
    let host = connection
        .profile
        .host
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Host is required"))?;
    let username = connection
        .profile
        .username
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Username is required"))?;
    let password = connection.password.as_deref().unwrap_or("");
    let port = connection
        .profile
        .port
        .unwrap_or(match connection.profile.db_type {
            SupportedDatabaseType::Postgresql => 5432,
            SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => 3306,
            SupportedDatabaseType::Sqlite => unreachable!(),
        });
    let database = database_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(connection.profile.database_name.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(match connection.profile.db_type {
            SupportedDatabaseType::Postgresql => "postgres",
            SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => "mysql",
            SupportedDatabaseType::Sqlite => unreachable!(),
        });
    let scheme = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => "postgres",
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => "mysql",
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    Ok(format!(
        "{scheme}://{}:{}@{}:{}/{}",
        url_encode(username),
        url_encode(password),
        host,
        port,
        url_encode(database)
    ))
}

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect::<Vec<_>>(),
        })
        .collect()
}

fn default_database_name(connection: &StoredDatabaseConnection) -> String {
    connection
        .profile
        .database_name
        .clone()
        .unwrap_or_else(|| match connection.profile.db_type {
            SupportedDatabaseType::Postgresql => "postgres".to_string(),
            SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => "mysql".to_string(),
            SupportedDatabaseType::Sqlite => "main".to_string(),
        })
}

async fn live_database_nodes(
    connection: &StoredDatabaseConnection,
) -> Result<Vec<DatabaseExplorerNode>> {
    Ok(live_databases(connection)
        .await?
        .into_iter()
        .map(|database| DatabaseExplorerNode {
            id: format!("database:{}:{}:", connection.profile.id, database.name),
            object_type: DatabaseObjectType::Database,
            connection_id: connection.profile.id,
            name: database.name.clone(),
            database_name: Some(database.name),
            schema_name: None,
            has_children: true,
            description: database.is_default.then_some("default".to_string()),
        })
        .collect())
}

async fn live_schema_nodes(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
) -> Result<Vec<DatabaseExplorerNode>> {
    Ok(live_schemas(connection, database_name)
        .await?
        .into_iter()
        .map(|schema| DatabaseExplorerNode {
            id: format!(
                "schema:{}:{}:{}",
                connection.profile.id,
                database_name.unwrap_or_else(|| {
                    connection
                        .profile
                        .database_name
                        .as_deref()
                        .unwrap_or("postgres")
                }),
                schema.name
            ),
            object_type: DatabaseObjectType::Schema,
            connection_id: connection.profile.id,
            name: schema.name.clone(),
            database_name: Some(
                database_name
                    .map(str::to_string)
                    .unwrap_or_else(|| default_database_name(connection)),
            ),
            schema_name: Some(schema.name),
            has_children: true,
            description: schema.is_default.then_some("default".to_string()),
        })
        .collect())
}

async fn live_object_nodes(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
) -> Result<Vec<DatabaseExplorerNode>> {
    Ok(live_objects(connection, database_name, schema_name)
        .await?
        .into_iter()
        .map(|object| DatabaseExplorerNode {
            id: format!(
                "{}:{}:{}:{}:{}",
                database_object_type_as_str(object.object_type),
                connection.profile.id,
                object.database_name.as_deref().unwrap_or(""),
                object.schema_name.as_deref().unwrap_or(""),
                object.name
            ),
            object_type: object.object_type,
            connection_id: connection.profile.id,
            name: object.name,
            database_name: object.database_name,
            schema_name: object.schema_name,
            has_children: false,
            description: Some(database_object_type_as_str(object.object_type).to_string()),
        })
        .collect())
}

async fn live_databases(connection: &StoredDatabaseConnection) -> Result<Vec<DatabaseNameSummary>> {
    let current = default_database_name(connection);
    if connection.profile.db_type == SupportedDatabaseType::Postgresql
        && !connection.profile.show_all_databases
    {
        return Ok(vec![DatabaseNameSummary {
            name: current,
            is_default: true,
        }]);
    }
    let pool = live_pool(connection).await?;
    let sql = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
            "SELECT datname::text AS name FROM pg_database WHERE datistemplate = false ORDER BY datname"
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            "SELECT schema_name AS name FROM information_schema.schemata ORDER BY schema_name"
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let rows = sqlx::query(sql)
        .fetch_all(&pool)
        .await
        .map_err(sqlx_server_error)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| any_string(&row, 0))
        .map(|name| DatabaseNameSummary {
            is_default: name == current,
            name,
        })
        .collect())
}

async fn live_schemas(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
) -> Result<Vec<DatabaseNameSummary>> {
    if connection.profile.db_type != SupportedDatabaseType::Postgresql {
        return Ok(Vec::new());
    }
    let pool = live_pool_for_database(connection, database_name).await?;
    let rows = sqlx::query(
        r#"
        SELECT schema_name::text AS schema_name
        FROM information_schema.schemata
        WHERE schema_name NOT IN ('pg_catalog', 'information_schema')
          AND schema_name NOT LIKE 'pg_toast%'
        ORDER BY schema_name
        "#,
    )
    .fetch_all(&pool)
    .await
    .map_err(sqlx_server_error)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| any_string(&row, 0))
        .map(|name| DatabaseNameSummary {
            is_default: name == "public",
            name,
        })
        .collect())
}

async fn live_objects(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
) -> Result<Vec<DatabaseObjectSummary>> {
    let pool = live_pool_for_database(connection, database_name).await?;
    match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
            let database = database_name
                .or(connection.profile.database_name.as_deref())
                .map(str::to_string)
                .unwrap_or_else(|| default_database_name(connection));
            let schema = schema_name.unwrap_or("public");
            let rows = sqlx::query(
                r#"
                SELECT table_name::text, table_type::text
                FROM information_schema.tables
                WHERE table_schema = $1
                  AND table_type IN ('BASE TABLE', 'VIEW')
                ORDER BY table_type, table_name
                "#,
            )
            .bind(schema)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            Ok(rows
                .into_iter()
                .filter_map(|row| {
                    let name = any_string(&row, 0)?;
                    let object_type = if any_string(&row, 1).as_deref() == Some("VIEW") {
                        DatabaseObjectType::View
                    } else {
                        DatabaseObjectType::Table
                    };
                    Some(DatabaseObjectSummary {
                        name,
                        object_type,
                        database_name: Some(database.clone()),
                        schema_name: Some(schema.to_string()),
                    })
                })
                .collect())
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            let database = database_name
                .or(connection.profile.database_name.as_deref())
                .map(str::to_string)
                .unwrap_or_else(|| default_database_name(connection));
            let rows = sqlx::query(
                r#"
                SELECT table_name, table_type
                FROM information_schema.tables
                WHERE table_schema = ?
                  AND table_type IN ('BASE TABLE', 'VIEW')
                ORDER BY table_type, table_name
                "#,
            )
            .bind(&database)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            Ok(rows
                .into_iter()
                .filter_map(|row| {
                    let name = any_string(&row, 0)?;
                    let object_type = if any_string(&row, 1).as_deref() == Some("VIEW") {
                        DatabaseObjectType::View
                    } else {
                        DatabaseObjectType::Table
                    };
                    Some(DatabaseObjectSummary {
                        name,
                        object_type,
                        database_name: Some(database.clone()),
                        schema_name: None,
                    })
                })
                .collect())
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    }
}

async fn describe_live_table(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    table_name: &str,
    object_type: DatabaseObjectType,
) -> Result<LiveTableDescription> {
    let pool = live_pool_for_database(connection, database_name).await?;
    let expected_table_types: &[&str] = match object_type {
        DatabaseObjectType::View => &["VIEW"],
        _ => &["BASE TABLE"],
    };

    let (database, schema) = live_scope(connection, database_name, schema_name);
    let columns_sql = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
            r#"
            SELECT
              column_name::text,
              data_type::text,
              udt_name::text,
              is_nullable::text,
              column_default::text
            FROM information_schema.columns
            WHERE table_schema = $1 AND table_name = $2
            ORDER BY ordinal_position
            "#
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            r#"
            SELECT column_name, data_type, column_type, is_nullable, column_default
            FROM information_schema.columns
            WHERE table_schema = ? AND table_name = ?
            ORDER BY ordinal_position
            "#
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let columns_rows = sqlx::query(columns_sql)
        .bind(&schema)
        .bind(table_name)
        .fetch_all(&pool)
        .await
        .map_err(sqlx_server_error)?;
    if columns_rows.is_empty() {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "Table not found"));
    }

    let primary_keys =
        live_primary_keys(&pool, connection.profile.db_type, &schema, table_name).await?;
    let foreign_keys =
        live_foreign_keys(&pool, connection.profile.db_type, &schema, table_name).await?;
    let columns = columns_rows
        .into_iter()
        .filter_map(|row| {
            let name = any_string(&row, 0)?;
            let data_type = any_string(&row, 1);
            let native_type = any_string(&row, 2).or_else(|| data_type.clone());
            let nullable = any_string(&row, 3).map(|value| value.eq_ignore_ascii_case("yes"));
            Some(DatabaseObjectColumn {
                is_primary_key: primary_keys.contains(&name),
                name,
                data_type,
                native_type,
                nullable,
                default_value: any_string(&row, 4),
                extra: None,
            })
        })
        .collect::<Vec<_>>();

    let table_type =
        live_table_type(&pool, connection.profile.db_type, &schema, table_name).await?;
    if !expected_table_types.contains(&table_type.as_str()) {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "Table not found"));
    }

    let _ = database;
    Ok(LiveTableDescription {
        columns,
        foreign_keys,
    })
}

async fn live_table_type(
    pool: &AnyPool,
    db_type: SupportedDatabaseType,
    schema: &str,
    table_name: &str,
) -> Result<String> {
    let sql = match db_type {
        SupportedDatabaseType::Postgresql => {
            r#"
            SELECT table_type::text
            FROM information_schema.tables
            WHERE table_schema = $1 AND table_name = $2
            "#
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            r#"
            SELECT table_type
            FROM information_schema.tables
            WHERE table_schema = ? AND table_name = ?
            "#
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let row = sqlx::query(sql)
        .bind(schema)
        .bind(table_name)
        .fetch_optional(pool)
        .await
        .map_err(sqlx_server_error)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Table not found"))?;
    Ok(any_string(&row, 0).unwrap_or_else(|| "BASE TABLE".to_string()))
}

async fn live_primary_keys(
    pool: &AnyPool,
    db_type: SupportedDatabaseType,
    schema: &str,
    table_name: &str,
) -> Result<Vec<String>> {
    let sql = match db_type {
        SupportedDatabaseType::Postgresql => {
            r#"
            SELECT kcu.column_name::text
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
              ON tc.constraint_name = kcu.constraint_name
             AND tc.table_schema = kcu.table_schema
            WHERE tc.constraint_type = 'PRIMARY KEY'
              AND tc.table_schema = $1
              AND tc.table_name = $2
            ORDER BY kcu.ordinal_position
            "#
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            r#"
            SELECT kcu.column_name
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
              ON tc.constraint_name = kcu.constraint_name
             AND tc.table_schema = kcu.table_schema
             AND tc.table_name = kcu.table_name
            WHERE tc.constraint_type = 'PRIMARY KEY'
              AND tc.table_schema = ?
              AND tc.table_name = ?
            ORDER BY kcu.ordinal_position
            "#
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let rows = sqlx::query(sql)
        .bind(schema)
        .bind(table_name)
        .fetch_all(pool)
        .await
        .map_err(sqlx_server_error)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| any_string(&row, 0))
        .collect())
}

async fn live_foreign_keys(
    pool: &AnyPool,
    db_type: SupportedDatabaseType,
    schema: &str,
    table_name: &str,
) -> Result<Vec<DatabaseForeignKey>> {
    let sql = match db_type {
        SupportedDatabaseType::Postgresql => {
            r#"
            SELECT
              tc.constraint_name::text,
              kcu.column_name::text,
              ccu.table_schema::text AS referenced_table_schema,
              ccu.table_name::text AS referenced_table_name,
              ccu.column_name::text AS referenced_column_name
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
              ON tc.constraint_name = kcu.constraint_name
             AND tc.table_schema = kcu.table_schema
            JOIN information_schema.constraint_column_usage ccu
              ON ccu.constraint_name = tc.constraint_name
             AND ccu.table_schema = tc.table_schema
            WHERE tc.constraint_type = 'FOREIGN KEY'
              AND tc.table_schema = $1
              AND tc.table_name = $2
            ORDER BY tc.constraint_name, kcu.ordinal_position
            "#
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            r#"
            SELECT
              kcu.constraint_name,
              kcu.column_name,
              kcu.referenced_table_schema,
              kcu.referenced_table_name,
              kcu.referenced_column_name
            FROM information_schema.key_column_usage kcu
            WHERE kcu.table_schema = ?
              AND kcu.table_name = ?
              AND kcu.referenced_table_name IS NOT NULL
            ORDER BY kcu.constraint_name, kcu.ordinal_position
            "#
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let rows = sqlx::query(sql)
        .bind(schema)
        .bind(table_name)
        .fetch_all(pool)
        .await
        .map_err(sqlx_server_error)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            Some(DatabaseForeignKey {
                name: any_string(&row, 0),
                column_name: any_string(&row, 1)?,
                referenced_schema_name: any_string(&row, 2),
                referenced_table_name: any_string(&row, 3)?,
                referenced_column_name: any_string(&row, 4)?,
                on_update: None,
                on_delete: None,
            })
        })
        .collect())
}

async fn live_relational_schema(
    connection: &StoredDatabaseConnection,
    scope_type: DatabaseObjectType,
    scope_name: &str,
    schema_name: Option<&str>,
    objects: &[DatabaseObjectSummary],
) -> Result<DatabaseRelationalSchema> {
    let mut tables = Vec::new();
    let mut relationships = Vec::new();
    for object in objects.iter().filter(|object| {
        matches!(
            object.object_type,
            DatabaseObjectType::Table | DatabaseObjectType::View
        )
    }) {
        let table = describe_live_table(
            connection,
            object.database_name.as_deref(),
            object.schema_name.as_deref().or(schema_name),
            &object.name,
            object.object_type,
        )
        .await?;
        for foreign_key in &table.foreign_keys {
            relationships.push(DatabaseRelationalSchemaRelationship {
                id: format!(
                    "{}:{}:{}",
                    object.name, foreign_key.column_name, foreign_key.referenced_table_name
                ),
                name: foreign_key.name.clone(),
                source_database_name: object.database_name.clone(),
                source_schema_name: object.schema_name.clone(),
                source_table_name: object.name.clone(),
                source_column_name: foreign_key.column_name.clone(),
                target_database_name: object.database_name.clone(),
                target_schema_name: foreign_key.referenced_schema_name.clone(),
                target_table_name: foreign_key.referenced_table_name.clone(),
                target_column_name: foreign_key.referenced_column_name.clone(),
                on_update: foreign_key.on_update.clone(),
                on_delete: foreign_key.on_delete.clone(),
            });
        }
        tables.push(DatabaseRelationalSchemaTable {
            database_name: object.database_name.clone(),
            schema_name: object.schema_name.clone(),
            name: object.name.clone(),
            object_type: object.object_type,
            columns: table.columns,
            is_external: false,
        });
    }

    Ok(DatabaseRelationalSchema {
        scope_type,
        scope_name: scope_name.to_string(),
        tables,
        relationships,
    })
}

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
            parent_name: None,
            identity: None,
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
            parent_name: None,
            identity: None,
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
                "{}:{}:{}:{}:{}:{}:{}",
                database_object_type_as_str(object.object_type),
                connection.profile.id,
                object.database_name.as_deref().unwrap_or(""),
                object.schema_name.as_deref().unwrap_or(""),
                object.parent_name.as_deref().unwrap_or(""),
                object.name,
                object.identity.as_deref().unwrap_or("")
            ),
            object_type: object.object_type,
            connection_id: connection.profile.id,
            name: object.name,
            database_name: object.database_name,
            schema_name: object.schema_name,
            parent_name: object.parent_name,
            identity: object.identity,
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
    let database = database_name
        .or(connection.profile.database_name.as_deref())
        .map(str::to_string)
        .unwrap_or_else(|| default_database_name(connection));
    let schema = schema_name.unwrap_or("public");
    let mut objects = Vec::new();
    match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
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
            objects.extend(rows.into_iter().filter_map(|row| {
                let name = any_string(&row, 0)?;
                let object_type = if any_string(&row, 1).as_deref() == Some("VIEW") {
                    DatabaseObjectType::View
                } else {
                    DatabaseObjectType::Table
                };
                Some(DatabaseObjectSummary {
                    name: name.clone(),
                    object_type,
                    database_name: Some(database.clone()),
                    schema_name: Some(schema.to_string()),
                    parent_name: None,
                    identity: Some(name),
                })
            }));

            let rows = sqlx::query(
                r#"
                SELECT matviewname::text
                FROM pg_matviews
                WHERE schemaname = $1
                ORDER BY matviewname
                "#,
            )
            .bind(schema)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            objects.extend(rows.into_iter().filter_map(|row| {
                let name = any_string(&row, 0)?;
                Some(DatabaseObjectSummary {
                    name: name.clone(),
                    object_type: DatabaseObjectType::MaterializedView,
                    database_name: Some(database.clone()),
                    schema_name: Some(schema.to_string()),
                    parent_name: None,
                    identity: Some(name),
                })
            }));

            let rows = sqlx::query(
                r#"
                SELECT tablename::text, indexname::text
                FROM pg_indexes
                WHERE schemaname = $1
                ORDER BY tablename, indexname
                "#,
            )
            .bind(schema)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            objects.extend(rows.into_iter().filter_map(|row| {
                let parent_name = any_string(&row, 0)?;
                let name = any_string(&row, 1)?;
                Some(DatabaseObjectSummary {
                    name: name.clone(),
                    object_type: DatabaseObjectType::Index,
                    database_name: Some(database.clone()),
                    schema_name: Some(schema.to_string()),
                    parent_name: Some(parent_name),
                    identity: Some(name),
                })
            }));

            // `information_schema.triggers` has one row for every event a
            // trigger listens to.  A single `INSERT OR UPDATE` trigger would
            // therefore become two explorer nodes with the same name and
            // identity.  `pg_trigger` is one row per actual trigger and its
            // OID gives the client a stable discriminator for details.
            let rows = sqlx::query(
                r#"
                SELECT trigger.oid::text, trigger.tgname::text, relation.relname::text
                FROM pg_catalog.pg_trigger AS trigger
                INNER JOIN pg_catalog.pg_class AS relation
                  ON relation.oid = trigger.tgrelid
                INNER JOIN pg_catalog.pg_namespace AS namespace
                  ON namespace.oid = relation.relnamespace
                WHERE namespace.nspname = $1
                  AND NOT trigger.tgisinternal
                ORDER BY relation.relname, trigger.tgname, trigger.oid
                "#,
            )
            .bind(schema)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            objects.extend(rows.into_iter().filter_map(|row| {
                let identity = any_string(&row, 0)?;
                let name = any_string(&row, 1)?;
                let parent_name = any_string(&row, 2);
                Some(DatabaseObjectSummary {
                    name: name.clone(),
                    object_type: DatabaseObjectType::Trigger,
                    database_name: Some(database.clone()),
                    schema_name: Some(schema.to_string()),
                    parent_name,
                    identity: Some(identity),
                })
            }));

            let rows = sqlx::query(
                r#"
                SELECT routine_name::text, specific_name::text, routine_type::text
                FROM information_schema.routines
                WHERE routine_schema = $1
                ORDER BY routine_name, specific_name
                "#,
            )
            .bind(schema)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            objects.extend(rows.into_iter().filter_map(|row| {
                let name = any_string(&row, 0)?;
                let identity = any_string(&row, 1).or_else(|| Some(name.clone()));
                Some(DatabaseObjectSummary {
                    name,
                    object_type: DatabaseObjectType::Routine,
                    database_name: Some(database.clone()),
                    schema_name: Some(schema.to_string()),
                    parent_name: None,
                    identity,
                })
            }));

            let rows = sqlx::query(
                r#"
                -- information_schema.sequences deliberately excludes identity
                -- backing sequences. pg_sequences is the PostgreSQL catalog
                -- view that includes both explicit and identity-backed
                -- sequences, matching what the explorer promises to expose.
                SELECT sequencename::text
                FROM pg_sequences
                WHERE schemaname = $1
                ORDER BY sequencename
                "#,
            )
            .bind(schema)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            objects.extend(rows.into_iter().filter_map(|row| {
                let name = any_string(&row, 0)?;
                Some(DatabaseObjectSummary {
                    name: name.clone(),
                    object_type: DatabaseObjectType::Sequence,
                    database_name: Some(database.clone()),
                    schema_name: Some(schema.to_string()),
                    parent_name: None,
                    identity: Some(name),
                })
            }));

            // information_schema.table_constraints only exposes the SQL
            // standard constraint families. PostgreSQL exclusion constraints
            // live in pg_constraint with contype = 'x', so use the catalog
            // directly for the explorer's complete supported set.
            let rows = sqlx::query(
                r#"
                SELECT
                  catalog_constraint.conname::text,
                  relation.relname::text,
                  CASE catalog_constraint.contype
                    WHEN 'p' THEN 'PRIMARY KEY'
                    WHEN 'u' THEN 'UNIQUE'
                    WHEN 'f' THEN 'FOREIGN KEY'
                    WHEN 'c' THEN 'CHECK'
                    WHEN 'x' THEN 'EXCLUSION'
                  END::text AS constraint_type
                FROM pg_catalog.pg_constraint AS catalog_constraint
                INNER JOIN pg_catalog.pg_class AS relation
                  ON relation.oid = catalog_constraint.conrelid
                INNER JOIN pg_catalog.pg_namespace AS namespace
                  ON namespace.oid = relation.relnamespace
                WHERE namespace.nspname = $1
                  AND catalog_constraint.contype IN ('p', 'u', 'f', 'c', 'x')
                ORDER BY relation.relname, catalog_constraint.conname
                "#,
            )
            .bind(schema)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            objects.extend(rows.into_iter().filter_map(|row| {
                let name = any_string(&row, 0)?;
                let parent_name = any_string(&row, 1);
                Some(DatabaseObjectSummary {
                    name: name.clone(),
                    object_type: DatabaseObjectType::Constraint,
                    database_name: Some(database.clone()),
                    schema_name: Some(schema.to_string()),
                    parent_name,
                    identity: Some(name),
                })
            }));

            // Policy names are unique only within a table. Keep the owning
            // relation as the parent and the catalog OID as a stable identity
            // so equally named policies on different tables remain distinct.
            let rows = sqlx::query(
                r#"
                SELECT
                  policy.oid::text,
                  policy.polname::text,
                  relation.relname::text
                FROM pg_catalog.pg_policy AS policy
                INNER JOIN pg_catalog.pg_class AS relation
                  ON relation.oid = policy.polrelid
                INNER JOIN pg_catalog.pg_namespace AS namespace
                  ON namespace.oid = relation.relnamespace
                WHERE namespace.nspname = $1
                ORDER BY relation.relname, policy.polname, policy.oid
                "#,
            )
            .bind(schema)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            objects.extend(rows.into_iter().filter_map(|row| {
                let identity = any_string(&row, 0)?;
                let name = any_string(&row, 1)?;
                let parent_name = any_string(&row, 2)?;
                Some(DatabaseObjectSummary {
                    name,
                    object_type: DatabaseObjectType::Policy,
                    database_name: Some(database.clone()),
                    schema_name: Some(schema.to_string()),
                    parent_name: Some(parent_name),
                    identity: Some(identity),
                })
            }));
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
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
            objects.extend(rows.into_iter().filter_map(|row| {
                let name = any_string(&row, 0)?;
                let object_type = if any_string(&row, 1).as_deref() == Some("VIEW") {
                    DatabaseObjectType::View
                } else {
                    DatabaseObjectType::Table
                };
                Some(DatabaseObjectSummary {
                    name: name.clone(),
                    object_type,
                    database_name: Some(database.clone()),
                    schema_name: None,
                    parent_name: None,
                    identity: Some(name),
                })
            }));

            let rows = sqlx::query(
                r#"
                SELECT DISTINCT table_name, index_name
                FROM information_schema.statistics
                WHERE table_schema = ?
                ORDER BY table_name, index_name
                "#,
            )
            .bind(&database)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            objects.extend(rows.into_iter().filter_map(|row| {
                let parent_name = any_string(&row, 0)?;
                let name = any_string(&row, 1)?;
                Some(DatabaseObjectSummary {
                    name: name.clone(),
                    object_type: DatabaseObjectType::Index,
                    database_name: Some(database.clone()),
                    schema_name: None,
                    parent_name: Some(parent_name),
                    identity: Some(name),
                })
            }));

            let rows = sqlx::query(
                r#"
                SELECT trigger_name, event_object_table
                FROM information_schema.triggers
                WHERE trigger_schema = ?
                ORDER BY event_object_table, trigger_name
                "#,
            )
            .bind(&database)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            objects.extend(rows.into_iter().filter_map(|row| {
                let name = any_string(&row, 0)?;
                Some(DatabaseObjectSummary {
                    name: name.clone(),
                    object_type: DatabaseObjectType::Trigger,
                    database_name: Some(database.clone()),
                    schema_name: None,
                    parent_name: any_string(&row, 1),
                    identity: Some(name),
                })
            }));

            let rows = sqlx::query(
                r#"
                SELECT routine_name, specific_name, routine_type
                FROM information_schema.routines
                WHERE routine_schema = ?
                ORDER BY routine_name, specific_name
                "#,
            )
            .bind(&database)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            objects.extend(rows.into_iter().filter_map(|row| {
                let name = any_string(&row, 0)?;
                Some(DatabaseObjectSummary {
                    name,
                    object_type: DatabaseObjectType::Routine,
                    database_name: Some(database.clone()),
                    schema_name: None,
                    parent_name: None,
                    identity: any_string(&row, 1),
                })
            }));

            let rows = sqlx::query(
                r#"
                SELECT event_name
                FROM information_schema.events
                WHERE event_schema = ?
                ORDER BY event_name
                "#,
            )
            .bind(&database)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            objects.extend(rows.into_iter().filter_map(|row| {
                let name = any_string(&row, 0)?;
                Some(DatabaseObjectSummary {
                    name: name.clone(),
                    object_type: DatabaseObjectType::Event,
                    database_name: Some(database.clone()),
                    schema_name: None,
                    parent_name: None,
                    identity: Some(name),
                })
            }));

            let rows = sqlx::query(
                r#"
                SELECT constraint_name, table_name, constraint_type
                FROM information_schema.table_constraints
                WHERE constraint_schema = ?
                ORDER BY table_name, constraint_name
                "#,
            )
            .bind(&database)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            objects.extend(rows.into_iter().filter_map(|row| {
                let name = any_string(&row, 0)?;
                Some(DatabaseObjectSummary {
                    name: name.clone(),
                    object_type: DatabaseObjectType::Constraint,
                    database_name: Some(database.clone()),
                    schema_name: None,
                    parent_name: any_string(&row, 1),
                    identity: Some(name),
                })
            }));
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    }
    Ok(objects)
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
        DatabaseObjectType::MaterializedView => &[],
        _ => &["BASE TABLE"],
    };

    let (database, schema) = live_scope(connection, database_name, schema_name);
    let columns_sql = match (connection.profile.db_type, object_type) {
        // PostgreSQL materialized views are relations but deliberately do not
        // appear in information_schema.columns. Read their attributes from
        // pg_catalog so the data-grid path has the same column metadata as
        // normal tables and views.
        (SupportedDatabaseType::Postgresql, DatabaseObjectType::MaterializedView) => {
            r#"
            SELECT
              attribute.attname::text,
              pg_catalog.format_type(attribute.atttypid, attribute.atttypmod)::text,
              type.typname::text,
              CASE WHEN attribute.attnotnull THEN 'NO' ELSE 'YES' END,
              pg_get_expr(default_value.adbin, default_value.adrelid)::text,
              NULL::text,
              NULL::text,
              NULL::text,
              NULL::text
            FROM pg_catalog.pg_attribute AS attribute
            INNER JOIN pg_catalog.pg_class AS relation
              ON relation.oid = attribute.attrelid
            INNER JOIN pg_catalog.pg_namespace AS namespace
              ON namespace.oid = relation.relnamespace
            INNER JOIN pg_catalog.pg_type AS type
              ON type.oid = attribute.atttypid
            LEFT JOIN pg_catalog.pg_attrdef AS default_value
              ON default_value.adrelid = relation.oid
             AND default_value.adnum = attribute.attnum
            WHERE namespace.nspname = $1
              AND relation.relname = $2
              AND relation.relkind = 'm'
              AND attribute.attnum > 0
              AND NOT attribute.attisdropped
            ORDER BY attribute.attnum
            "#
            .to_string()
        }
        (SupportedDatabaseType::Postgresql, _) => {
            // PostgreSQL added identity fields in v10 and generated-column
            // fields in v12. Keep opening ordinary tables compatible with
            // older supported servers by projecting typed NULLs when a
            // catalog field does not exist yet.
            let server_version_num = postgresql_server_version_num(&pool).await?;
            let identity_sql = if server_version_num >= 100_000 {
                "is_identity::text, identity_generation::text"
            } else {
                "NULL::text AS is_identity, NULL::text AS identity_generation"
            };
            let generated_sql = if server_version_num >= 120_000 {
                "is_generated::text, generation_expression::text"
            } else {
                "NULL::text AS is_generated, NULL::text AS generation_expression"
            };
            format!(
                r#"
            SELECT
              column_name::text,
              data_type::text,
              udt_name::text,
              is_nullable::text,
              column_default::text,
              {identity_sql},
              {generated_sql}
            FROM information_schema.columns
            WHERE table_schema = $1 AND table_name = $2
            ORDER BY ordinal_position
            "#,
            )
        }
        (SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb, _) => {
            r#"
            SELECT column_name, data_type, column_type, is_nullable, column_default, extra
            FROM information_schema.columns
            WHERE table_schema = ? AND table_name = ?
            ORDER BY ordinal_position
            "#
            .to_string()
        }
        (SupportedDatabaseType::Sqlite, _) => unreachable!(),
    };
    let columns_rows = sqlx::query(&columns_sql)
        .bind(&schema)
        .bind(table_name)
        .fetch_all(&pool)
        .await
        .map_err(sqlx_server_error)?;
    if columns_rows.is_empty() {
        return Err(ServerError::new(
            StatusCode::NOT_FOUND,
            if object_type == DatabaseObjectType::MaterializedView {
                "Materialized view not found"
            } else {
                "Table not found"
            },
        ));
    }

    let primary_keys =
        live_primary_keys(&pool, connection.profile.db_type, &schema, table_name).await?;
    let foreign_keys =
        live_foreign_keys(&pool, connection.profile.db_type, &schema, table_name).await?;
    // MariaDB's JSON alias is stored as LONGTEXT plus an exact JSON_VALID
    // check. Retain its semantic type for JSON editors and paste conversion;
    // ordinary LONGTEXT and unrelated/compound checks are not JSON aliases.
    let maria_json_checks = if connection.profile.db_type == SupportedDatabaseType::Mariadb {
        // MySQL-compatible profiles may point to either backend. MariaDB
        // adds TABLE_NAME to this catalog; MySQL exposes only the standard
        // constraint identity. Keep MariaDB's exact table qualification
        // (including same-named checks on different tables) when available.
        let has_table_name = sqlx::query(
            "SELECT column_name FROM information_schema.columns WHERE table_schema = 'information_schema' AND table_name = 'CHECK_CONSTRAINTS' AND column_name = 'TABLE_NAME'",
        )
        .fetch_optional(&pool)
        .await
        .map_err(sqlx_server_error)?
        .is_some();
        let checks_sql = if has_table_name {
            "SELECT check_clause FROM information_schema.check_constraints WHERE constraint_schema = ? AND table_name = ?"
        } else {
            "SELECT cc.check_clause FROM information_schema.check_constraints AS cc \
             JOIN information_schema.table_constraints AS tc \
               ON tc.constraint_catalog = cc.constraint_catalog \
              AND tc.constraint_schema = cc.constraint_schema \
              AND tc.constraint_name = cc.constraint_name \
             WHERE tc.table_schema = ? AND tc.table_name = ? \
               AND tc.constraint_type = 'CHECK'"
        };
        sqlx::query(checks_sql)
        .bind(&schema)
        .bind(table_name)
        .fetch_all(&pool)
        .await
        .map_err(sqlx_server_error)?
        .into_iter()
        .filter_map(|row| any_string(&row, 0))
        .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let columns = columns_rows
        .into_iter()
        .filter_map(|row| {
            let name = any_string(&row, 0)?;
            let mut data_type = any_string(&row, 1);
            let mut native_type = any_string(&row, 2).or_else(|| data_type.clone());
            if data_type.as_deref().is_some_and(|kind| kind.eq_ignore_ascii_case("longtext"))
                && maria_json_checks.iter().any(|check| mariadb_check_is_json_column(check, &name))
            {
                data_type = Some("json".to_string());
                native_type = Some("json".to_string());
            }
            let nullable = any_string(&row, 3).map(|value| value.eq_ignore_ascii_case("yes"));
            let default_value = any_string(&row, 4);
            let extra = match connection.profile.db_type {
                SupportedDatabaseType::Postgresql => postgresql_column_extra(
                    default_value.as_deref(),
                    any_string(&row, 5).as_deref(),
                    any_string(&row, 6).as_deref(),
                    any_string(&row, 7).as_deref(),
                ),
                SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => any_string(&row, 5)
                    .filter(|extra| !extra.trim().is_empty()),
                SupportedDatabaseType::Sqlite => None,
            };
            Some(DatabaseObjectColumn {
                is_primary_key: primary_keys.contains(&name),
                name,
                data_type,
                native_type,
                nullable,
                default_value,
                extra,
            })
        })
        .collect::<Vec<_>>();

    if object_type == DatabaseObjectType::MaterializedView {
        let exists = sqlx::query(
            "SELECT 1 FROM pg_matviews WHERE schemaname = $1 AND matviewname = $2",
        )
        .bind(&schema)
        .bind(table_name)
        .fetch_optional(&pool)
        .await
        .map_err(sqlx_server_error)?
        .is_some();
        if !exists {
            return Err(ServerError::new(StatusCode::NOT_FOUND, "Materialized view not found"));
        }
    } else {
        let table_type =
            live_table_type(&pool, connection.profile.db_type, &schema, table_name).await?;
        if !expected_table_types.contains(&table_type.as_str()) {
            return Err(ServerError::new(StatusCode::NOT_FOUND, "Table not found"));
        }
    }

    let _ = database;
    Ok(LiveTableDescription {
        columns,
        foreign_keys,
    })
}

fn mariadb_check_is_json_column(check: &str, column_name: &str) -> bool {
    let quoted = format!("`{}`", column_name.replace('`', "``"));
    check.trim().eq_ignore_ascii_case(&format!("json_valid({quoted})"))
}

fn postgresql_column_extra(
    default_value: Option<&str>,
    is_identity: Option<&str>,
    identity_generation: Option<&str>,
    is_generated: Option<&str>,
) -> Option<String> {
    if is_identity.is_some_and(|value| value.eq_ignore_ascii_case("YES")) {
        let generation = identity_generation
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase)
            .unwrap_or_else(|| "by default".to_string());
        return Some(format!("identity {generation}"));
    }
    if is_generated.is_some_and(|value| !value.eq_ignore_ascii_case("NEVER")) {
        return Some("generated stored".to_string());
    }
    default_value
        .filter(|value| value.to_ascii_lowercase().contains("nextval("))
        .map(|_| "auto_increment".to_string())
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
             AND tc.table_name = kcu.table_name
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
              catalog_constraint.conname::text,
              source_attribute.attname::text,
              NULL::text AS referenced_database_name,
              target_namespace.nspname::text AS referenced_table_schema,
              target_relation.relname::text AS referenced_table_name,
              target_attribute.attname::text AS referenced_column_name,
              CASE catalog_constraint.confupdtype
                WHEN 'a' THEN 'NO ACTION'
                WHEN 'r' THEN 'RESTRICT'
                WHEN 'c' THEN 'CASCADE'
                WHEN 'n' THEN 'SET NULL'
                WHEN 'd' THEN 'SET DEFAULT'
              END::text AS on_update,
              CASE catalog_constraint.confdeltype
                WHEN 'a' THEN 'NO ACTION'
                WHEN 'r' THEN 'RESTRICT'
                WHEN 'c' THEN 'CASCADE'
                WHEN 'n' THEN 'SET NULL'
                WHEN 'd' THEN 'SET DEFAULT'
              END::text AS on_delete
            FROM pg_catalog.pg_constraint AS catalog_constraint
            INNER JOIN pg_catalog.pg_class AS source_relation
              ON source_relation.oid = catalog_constraint.conrelid
            INNER JOIN pg_catalog.pg_namespace AS source_namespace
              ON source_namespace.oid = source_relation.relnamespace
            INNER JOIN pg_catalog.pg_class AS target_relation
              ON target_relation.oid = catalog_constraint.confrelid
            INNER JOIN pg_catalog.pg_namespace AS target_namespace
              ON target_namespace.oid = target_relation.relnamespace
            INNER JOIN LATERAL unnest(catalog_constraint.conkey, catalog_constraint.confkey)
              WITH ORDINALITY AS key_pair(source_attnum, target_attnum, ordinal_position)
              ON TRUE
            INNER JOIN pg_catalog.pg_attribute AS source_attribute
              ON source_attribute.attrelid = source_relation.oid
             AND source_attribute.attnum = key_pair.source_attnum
            INNER JOIN pg_catalog.pg_attribute AS target_attribute
              ON target_attribute.attrelid = target_relation.oid
             AND target_attribute.attnum = key_pair.target_attnum
            WHERE catalog_constraint.contype = 'f'
              AND source_namespace.nspname = $1
              AND source_relation.relname = $2
            ORDER BY catalog_constraint.conname, key_pair.ordinal_position
            "#
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            r#"
            SELECT
              kcu.constraint_name,
              kcu.column_name,
              kcu.referenced_table_schema AS referenced_database_name,
              NULL AS referenced_schema_name,
              kcu.referenced_table_name,
              kcu.referenced_column_name,
              referential.update_rule,
              referential.delete_rule
            FROM information_schema.key_column_usage kcu
            LEFT JOIN information_schema.referential_constraints referential
              ON referential.constraint_schema = kcu.constraint_schema
             AND referential.constraint_name = kcu.constraint_name
             AND referential.table_name = kcu.table_name
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
                referenced_database_name: any_string(&row, 2),
                referenced_schema_name: any_string(&row, 3),
                referenced_table_name: any_string(&row, 4)?,
                referenced_column_name: any_string(&row, 5)?,
                on_update: any_string(&row, 6),
                on_delete: any_string(&row, 7),
            })
        })
        .collect())
}

struct LiveObjectDescription {
    parent_name: Option<String>,
    identity: Option<String>,
    columns: Vec<DatabaseObjectColumn>,
    primary_key: Vec<String>,
    foreign_keys: Vec<DatabaseForeignKey>,
    definition: Option<String>,
    metadata: Option<Value>,
}

async fn describe_live_object(
    connection: &StoredDatabaseConnection,
    object_type: DatabaseObjectType,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    name: &str,
    parent_name: Option<&str>,
    identity: Option<&str>,
) -> Result<LiveObjectDescription> {
    if matches!(
        object_type,
        DatabaseObjectType::Table
            | DatabaseObjectType::View
            | DatabaseObjectType::MaterializedView
    ) {
        let table = describe_live_table(
            connection,
            database_name,
            schema_name,
            name,
            object_type,
        )
        .await?;
        let primary_key = table
            .columns
            .iter()
            .filter(|column| column.is_primary_key)
            .map(|column| column.name.clone())
            .collect();
        let definition = live_relation_definition(
            connection,
            database_name,
            schema_name,
            name,
            object_type,
        )
        .await?;
        let metadata = live_relation_metadata(
            connection,
            database_name,
            schema_name,
            name,
            object_type,
        )
        .await?;
        return Ok(LiveObjectDescription {
            parent_name: None,
            identity: Some(identity.unwrap_or(name).to_string()),
            columns: table.columns,
            primary_key,
            foreign_keys: table.foreign_keys,
            definition,
            metadata,
        });
    }

    let pool = live_pool_for_database(connection, database_name).await?;
    let (database, schema) = live_scope(connection, database_name, schema_name);
    let resolved_parent = parent_name
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let resolved_identity = identity
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(name)
        .to_string();

    match object_type {
        DatabaseObjectType::Index => {
            let (definition, metadata) = match connection.profile.db_type {
                SupportedDatabaseType::Postgresql => {
                    let row = sqlx::query(
                        r#"
                        SELECT tablename::text, indexdef::text
                        FROM pg_indexes
                        WHERE schemaname = $1 AND indexname = $2
                        "#,
                    )
                    .bind(&schema)
                    .bind(name)
                    .fetch_optional(&pool)
                    .await
                    .map_err(sqlx_server_error)?
                    .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Index not found"))?;
                    let parent = any_string(&row, 0);
                    let definition = any_string(&row, 1);
                    let metadata = serde_json::json!({
                        "tableName": parent,
                        "definition": definition,
                    });
                    (definition, Some(metadata))
                }
                SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
                    let parent_name = resolved_parent.as_deref().unwrap_or("");
                    let rows = sqlx::query(
                        r#"
                        SELECT table_name, non_unique, index_type, column_name, seq_in_index
                        FROM information_schema.statistics
                        WHERE table_schema = ?
                          AND index_name = ?
                          AND (? = '' OR table_name = ?)
                        ORDER BY table_name, seq_in_index
                        "#,
                    )
                    .bind(&database)
                    .bind(name)
                    .bind(parent_name)
                    .bind(parent_name)
                    .fetch_all(&pool)
                    .await
                    .map_err(sqlx_server_error)?;
                    if rows.is_empty() {
                        return Err(ServerError::new(StatusCode::NOT_FOUND, "Index not found"));
                    }
                    let table_name = any_string(&rows[0], 0);
                    let unique = any_string(&rows[0], 1).as_deref() == Some("0");
                    let index_type = any_string(&rows[0], 2);
                    let columns = rows
                        .iter()
                        .filter_map(|row| any_string(row, 3))
                        .collect::<Vec<_>>();
                    let definition = table_name.as_deref().map(|table| {
                        format!(
                            "CREATE {}INDEX {} ON {} ({})",
                            if unique { "UNIQUE " } else { "" },
                            quote_mysql_identifier(name),
                            quote_mysql_identifier(table),
                            columns
                                .iter()
                                .map(|column| quote_mysql_identifier(column))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    });
                    let metadata = serde_json::json!({
                        "tableName": table_name,
                        "unique": unique,
                        "indexType": index_type,
                        "columns": columns,
                    });
                    (definition, Some(metadata))
                }
                SupportedDatabaseType::Sqlite => unreachable!(),
            };
            Ok(LiveObjectDescription {
                parent_name: resolved_parent,
                identity: Some(resolved_identity),
                columns: Vec::new(),
                primary_key: Vec::new(),
                foreign_keys: Vec::new(),
                definition,
                metadata,
            })
        }
        DatabaseObjectType::Trigger => match connection.profile.db_type {
            SupportedDatabaseType::Postgresql => {
                // Older persisted mobile trees used the trigger name as
                // `identity`; only use an identity filter when it is the new
                // pg_trigger OID form so those trees remain openable.
                let trigger_identity = resolved_identity
                    .parse::<u64>()
                    .ok()
                    .map(|_| resolved_identity.as_str())
                    .unwrap_or("");
                let row = sqlx::query(
                    r#"
                    SELECT
                      trigger.oid::text,
                      relation.relname::text,
                      pg_catalog.pg_get_triggerdef(trigger.oid, true)::text,
                      CASE
                        WHEN (trigger.tgtype::integer & 64) <> 0 THEN 'INSTEAD OF'
                        WHEN (trigger.tgtype::integer & 2) <> 0 THEN 'BEFORE'
                        ELSE 'AFTER'
                      END::text AS timing,
                      CASE
                        WHEN (trigger.tgtype::integer & 1) <> 0 THEN 'ROW'
                        ELSE 'STATEMENT'
                      END::text AS level,
                      concat_ws(', ',
                        CASE WHEN (trigger.tgtype::integer & 4) <> 0 THEN 'INSERT' END,
                        CASE WHEN (trigger.tgtype::integer & 8) <> 0 THEN 'DELETE' END,
                        CASE WHEN (trigger.tgtype::integer & 16) <> 0 THEN 'UPDATE' END,
                        CASE WHEN (trigger.tgtype::integer & 32) <> 0 THEN 'TRUNCATE' END
                      )::text AS events,
                      trigger.tgenabled::text
                    FROM pg_catalog.pg_trigger AS trigger
                    INNER JOIN pg_catalog.pg_class AS relation
                      ON relation.oid = trigger.tgrelid
                    INNER JOIN pg_catalog.pg_namespace AS namespace
                      ON namespace.oid = relation.relnamespace
                    WHERE namespace.nspname = $1
                      AND trigger.tgname = $2
                      AND NOT trigger.tgisinternal
                      AND ($3 = '' OR relation.relname = $3)
                      AND ($4 = '' OR trigger.oid::text = $4)
                    ORDER BY relation.relname, trigger.oid
                    LIMIT 1
                    "#,
                )
                .bind(&schema)
                .bind(name)
                .bind(resolved_parent.as_deref().unwrap_or(""))
                .bind(trigger_identity)
                .fetch_optional(&pool)
                .await
                .map_err(sqlx_server_error)?
                .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Trigger not found"))?;
                let identity = any_string(&row, 0).unwrap_or(resolved_identity);
                let table_name = any_string(&row, 1);
                let definition = any_string(&row, 2);
                let timing = any_string(&row, 3);
                let level = any_string(&row, 4);
                let events = any_string(&row, 5);
                let enabled = any_string(&row, 6);
                Ok(LiveObjectDescription {
                    parent_name: resolved_parent.or(table_name.clone()),
                    identity: Some(identity),
                    columns: Vec::new(),
                    primary_key: Vec::new(),
                    foreign_keys: Vec::new(),
                    definition: definition.clone(),
                    metadata: Some(serde_json::json!({
                        "tableName": table_name,
                        "timing": timing,
                        "level": level,
                        "events": events,
                        "enabled": enabled,
                        "definition": definition,
                    })),
                })
            }
            SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
                let row = sqlx::query(
                    r#"
                    SELECT event_object_table, action_timing, event_manipulation, action_statement
                    FROM information_schema.triggers
                    WHERE trigger_schema = ?
                      AND trigger_name = ?
                      AND (? = '' OR event_object_table = ?)
                    ORDER BY event_object_table
                    LIMIT 1
                    "#,
                )
                .bind(&database)
                .bind(name)
                .bind(resolved_parent.as_deref().unwrap_or(""))
                .bind(resolved_parent.as_deref().unwrap_or(""))
                .fetch_optional(&pool)
                .await
                .map_err(sqlx_server_error)?
                .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Trigger not found"))?;
                let table_name = any_string(&row, 0);
                let timing = any_string(&row, 1);
                let event = any_string(&row, 2);
                let action = any_string(&row, 3);
                // information_schema.triggers exposes only ACTION_STATEMENT,
                // which is the trigger body rather than a statement that can
                // be recreated from the mobile SQL editor. SHOW CREATE
                // TRIGGER is the engine's complete declaration on both
                // MySQL and MariaDB. Keep the information_schema body as a
                // graceful fallback for servers that return no CREATE row.
                // SHOW CREATE TRIGGER also returns a Created timestamp.
                // AnyPool cannot decode that MySQL/MariaDB temporal column,
                // even though only the declaration itself is needed here.
                let TypedLivePool::Mysql(trigger_pool) =
                    typed_live_pool_for_database(connection, Some(&database)).await?
                else {
                    unreachable!("MySQL trigger requires the typed MySQL pool");
                };
                let definition = mysql_trigger_definition(&trigger_pool, &database, name)
                    .await?
                    .or(action.clone())
                    .or_else(|| {
                        Some(
                            [timing.clone(), event.clone(), table_name.clone()]
                                .into_iter()
                                .flatten()
                                .collect::<Vec<_>>()
                                .join(" "),
                        )
                    });
                let metadata = serde_json::json!({
                    "tableName": table_name.clone(),
                    "timing": timing,
                    "event": event,
                    "action": action,
                    "definition": definition.clone(),
                });
                Ok(LiveObjectDescription {
                    parent_name: resolved_parent.or(table_name.clone()),
                    identity: Some(resolved_identity),
                    columns: Vec::new(),
                    primary_key: Vec::new(),
                    foreign_keys: Vec::new(),
                    definition,
                    metadata: Some(metadata),
                })
            }
            SupportedDatabaseType::Sqlite => unreachable!(),
        },
        DatabaseObjectType::Routine => {
            let row = match connection.profile.db_type {
                SupportedDatabaseType::Postgresql => sqlx::query(
                    r#"
                    SELECT routine_definition::text, routine_type::text,
                           data_type::text, specific_name::text
                    FROM information_schema.routines
                    WHERE routine_schema = $1
                      AND (specific_name = $2 OR routine_name = $2)
                    ORDER BY specific_name
                    LIMIT 1
                    "#,
                )
                .bind(&schema)
                .bind(&resolved_identity)
                .fetch_optional(&pool)
                .await
                .map_err(sqlx_server_error)?,
                SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => sqlx::query(
                    r#"
                    SELECT routine_definition, routine_type, data_type, specific_name
                    FROM information_schema.routines
                    WHERE routine_schema = ?
                      AND (specific_name = ? OR routine_name = ?)
                    ORDER BY specific_name
                    LIMIT 1
                    "#,
                )
                .bind(&database)
                .bind(&resolved_identity)
                .bind(name)
                .fetch_optional(&pool)
                .await
                .map_err(sqlx_server_error)?,
                SupportedDatabaseType::Sqlite => unreachable!(),
            }
            .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Routine not found"))?;
            let definition = any_string(&row, 0);
            Ok(LiveObjectDescription {
                parent_name: None,
                identity: Some(any_string(&row, 3).unwrap_or(resolved_identity)),
                columns: Vec::new(),
                primary_key: Vec::new(),
                foreign_keys: Vec::new(),
                definition,
                metadata: Some(serde_json::json!({
                    "routineType": any_string(&row, 1),
                    "returnType": any_string(&row, 2),
                })),
            })
        }
        DatabaseObjectType::Sequence => {
            if connection.profile.db_type != SupportedDatabaseType::Postgresql {
                return Err(ServerError::new(
                    StatusCode::NOT_FOUND,
                    "Sequences are not supported by this database",
                ));
            }
            let row = sqlx::query(
                r#"
                SELECT data_type::text, start_value::text, min_value::text,
                       max_value::text, increment_by::text, cycle::text, last_value::text
                FROM pg_sequences
                WHERE schemaname = $1 AND sequencename = $2
                "#,
            )
            .bind(&schema)
            .bind(name)
            .fetch_optional(&pool)
            .await
            .map_err(sqlx_server_error)?
            .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Sequence not found"))?;
            Ok(LiveObjectDescription {
                parent_name: None,
                identity: Some(resolved_identity),
                columns: Vec::new(),
                primary_key: Vec::new(),
                foreign_keys: Vec::new(),
                definition: Some(format!("{} sequence", any_string(&row, 0).unwrap_or_default())),
                metadata: Some(serde_json::json!({
                    "dataType": any_string(&row, 0),
                    "startValue": any_string(&row, 1),
                    "minimumValue": any_string(&row, 2),
                    "maximumValue": any_string(&row, 3),
                    "increment": any_string(&row, 4),
                    "cycle": any_string(&row, 5),
                    "lastValue": any_string(&row, 6),
                })),
            })
        }
        DatabaseObjectType::Event => {
            if !matches!(
                connection.profile.db_type,
                SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb
            ) {
                return Err(ServerError::new(
                    StatusCode::NOT_FOUND,
                    "Events are not supported by this database",
                ));
            }
            let row = sqlx::query(
                r#"
                SELECT event_definition, event_type, execute_at, interval_value,
                       interval_field, status, starts, ends
                FROM information_schema.events
                WHERE event_schema = ? AND event_name = ?
                "#,
            )
            .bind(&database)
            .bind(name)
            .fetch_optional(&pool)
            .await
            .map_err(sqlx_server_error)?
            .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Event not found"))?;
            Ok(LiveObjectDescription {
                parent_name: None,
                identity: Some(resolved_identity),
                columns: Vec::new(),
                primary_key: Vec::new(),
                foreign_keys: Vec::new(),
                definition: any_string(&row, 0),
                metadata: Some(serde_json::json!({
                    "eventType": any_string(&row, 1),
                    "executeAt": any_string(&row, 2),
                    "intervalValue": any_string(&row, 3),
                    "intervalField": any_string(&row, 4),
                    "status": any_string(&row, 5),
                    "starts": any_string(&row, 6),
                    "ends": any_string(&row, 7),
                })),
            })
        }
        DatabaseObjectType::Constraint => match connection.profile.db_type {
            SupportedDatabaseType::Postgresql => {
                let row = sqlx::query(
                    r#"
                    SELECT
                      CASE catalog_constraint.contype
                        WHEN 'p' THEN 'PRIMARY KEY'
                        WHEN 'u' THEN 'UNIQUE'
                        WHEN 'f' THEN 'FOREIGN KEY'
                        WHEN 'c' THEN 'CHECK'
                        WHEN 'x' THEN 'EXCLUSION'
                      END::text AS constraint_type,
                      pg_catalog.pg_get_constraintdef(catalog_constraint.oid)::text
                    FROM pg_catalog.pg_constraint AS catalog_constraint
                    INNER JOIN pg_catalog.pg_class AS relation
                      ON relation.oid = catalog_constraint.conrelid
                    INNER JOIN pg_catalog.pg_namespace AS namespace
                      ON namespace.oid = relation.relnamespace
                    WHERE namespace.nspname = $1
                      AND relation.relname = $2
                      AND catalog_constraint.conname = $3
                      AND catalog_constraint.contype IN ('p', 'u', 'f', 'c', 'x')
                    LIMIT 1
                    "#,
                )
                .bind(&schema)
                .bind(resolved_parent.as_deref().unwrap_or(""))
                .bind(name)
                .fetch_optional(&pool)
                .await
                .map_err(sqlx_server_error)?
                .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Constraint not found"))?;
                Ok(LiveObjectDescription {
                    parent_name: resolved_parent,
                    identity: Some(resolved_identity),
                    columns: Vec::new(),
                    primary_key: Vec::new(),
                    foreign_keys: Vec::new(),
                    definition: any_string(&row, 1),
                    metadata: Some(serde_json::json!({
                        "constraintType": any_string(&row, 0),
                    })),
                })
            }
            SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
                let parent_table = resolved_parent.as_deref().unwrap_or("");
                let row = sqlx::query(
                    r#"
                    SELECT tc.constraint_type
                    FROM information_schema.table_constraints tc
                    WHERE tc.constraint_schema = ?
                      AND tc.table_name = ?
                      AND tc.constraint_name = ?
                    LIMIT 1
                    "#,
                )
                .bind(&database)
                .bind(parent_table)
                .bind(name)
                .fetch_optional(&pool)
                .await
                .map_err(sqlx_server_error)?
                .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Constraint not found"))?;
                let constraint_type = any_string(&row, 0).unwrap_or_default();
                let constraint_type_key = constraint_type.to_ascii_uppercase();
                let check_clause = if constraint_type_key == "CHECK" {
                    sqlx::query(
                        r#"
                        SELECT check_clause
                        FROM information_schema.check_constraints
                        WHERE constraint_schema = ? AND constraint_name = ?
                        LIMIT 1
                        "#,
                    )
                    .bind(&database)
                    .bind(name)
                    .fetch_optional(&pool)
                    .await
                    .map_err(sqlx_server_error)?
                    .and_then(|check_row| any_string(&check_row, 0))
                } else {
                    None
                };
                let key_rows = sqlx::query(
                    r#"
                    SELECT
                      key_usage.column_name,
                      key_usage.referenced_table_schema AS referenced_database_name,
                      key_usage.referenced_table_name,
                      key_usage.referenced_column_name,
                      referential.update_rule,
                      referential.delete_rule
                    FROM information_schema.key_column_usage AS key_usage
                    LEFT JOIN information_schema.referential_constraints AS referential
                      ON referential.constraint_schema = key_usage.constraint_schema
                     AND referential.constraint_name = key_usage.constraint_name
                     AND referential.table_name = key_usage.table_name
                    WHERE key_usage.constraint_schema = ?
                      AND key_usage.table_name = ?
                      AND key_usage.constraint_name = ?
                    ORDER BY key_usage.ordinal_position
                    "#,
                )
                .bind(&database)
                .bind(parent_table)
                .bind(name)
                .fetch_all(&pool)
                .await
                .map_err(sqlx_server_error)?;
                let column_names = key_rows
                    .iter()
                    .filter_map(|key_row| any_string(key_row, 0))
                    .collect::<Vec<_>>();
                let table_columns = if column_names.is_empty() {
                    Vec::new()
                } else {
                    describe_live_table(
                        connection,
                        database_name,
                        schema_name,
                        parent_table,
                        DatabaseObjectType::Table,
                    )
                    .await?
                    .columns
                };
                let columns = column_names
                    .iter()
                    .filter_map(|column_name| {
                        table_columns
                            .iter()
                            .find(|column| column.name == *column_name)
                            .cloned()
                    })
                    .collect::<Vec<_>>();
                let foreign_keys = if constraint_type_key == "FOREIGN KEY" {
                    key_rows
                        .iter()
                        .filter_map(|key_row| {
                            Some(DatabaseForeignKey {
                                name: Some(name.to_string()),
                                column_name: any_string(key_row, 0)?,
                                referenced_database_name: any_string(key_row, 1),
                                referenced_schema_name: None,
                                referenced_table_name: any_string(key_row, 2)?,
                                referenced_column_name: any_string(key_row, 3)?,
                                on_update: any_string(key_row, 4),
                                on_delete: any_string(key_row, 5),
                            })
                        })
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                let referenced_columns = foreign_keys
                    .iter()
                    .map(|foreign_key| foreign_key.referenced_column_name.clone())
                    .collect::<Vec<_>>();
                let quoted_columns = column_names
                    .iter()
                    .map(|column_name| quote_mysql_identifier(column_name))
                    .collect::<Vec<_>>()
                    .join(", ");
                let table_ref = quote_mysql_identifier(parent_table);
                let definition = match constraint_type_key.as_str() {
                    "PRIMARY KEY" if !quoted_columns.is_empty() => {
                        Some(format!("ALTER TABLE {table_ref} ADD PRIMARY KEY ({quoted_columns})"))
                    }
                    "UNIQUE" if !quoted_columns.is_empty() => Some(format!(
                        "ALTER TABLE {table_ref} ADD CONSTRAINT {} UNIQUE ({quoted_columns})",
                        quote_mysql_identifier(name),
                    )),
                    "FOREIGN KEY"
                        if !quoted_columns.is_empty()
                            && foreign_keys.len() == column_names.len() =>
                    {
                        let first_foreign_key = foreign_keys
                            .first()
                            .expect("a non-empty foreign-key list has a first entry");
                        let referenced_table = match first_foreign_key
                            .referenced_database_name
                            .as_deref()
                            .filter(|database_name| !database_name.trim().is_empty())
                        {
                            Some(database_name) => format!(
                                "{}.{}",
                                quote_mysql_identifier(database_name),
                                quote_mysql_identifier(&first_foreign_key.referenced_table_name),
                            ),
                            None => quote_mysql_identifier(&first_foreign_key.referenced_table_name),
                        };
                        let quoted_referenced_columns = referenced_columns
                            .iter()
                            .map(|column_name| quote_mysql_identifier(column_name))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let mut definition = format!(
                            "ALTER TABLE {table_ref} ADD CONSTRAINT {} FOREIGN KEY ({quoted_columns}) REFERENCES {referenced_table} ({quoted_referenced_columns})",
                            quote_mysql_identifier(name),
                        );
                        if let Some(on_update) = first_foreign_key
                            .on_update
                            .as_deref()
                            .filter(|rule| !rule.trim().is_empty())
                        {
                            definition.push_str(&format!(" ON UPDATE {on_update}"));
                        }
                        if let Some(on_delete) = first_foreign_key
                            .on_delete
                            .as_deref()
                            .filter(|rule| !rule.trim().is_empty())
                        {
                            definition.push_str(&format!(" ON DELETE {on_delete}"));
                        }
                        Some(definition)
                    }
                    "CHECK" => check_clause.as_deref().map(|clause| {
                        let clause = clause.trim();
                        let check_clause = if clause.to_ascii_uppercase().starts_with("CHECK") {
                            clause.to_string()
                        } else {
                            format!("CHECK {clause}")
                        };
                        format!(
                            "ALTER TABLE {table_ref} ADD CONSTRAINT {} {check_clause}",
                            quote_mysql_identifier(name),
                        )
                    }),
                    _ => check_clause.clone(),
                };
                let mut metadata = Map::new();
                metadata.insert(
                    "constraintType".to_string(),
                    Value::String(constraint_type),
                );
                metadata.insert(
                    "tableName".to_string(),
                    Value::String(parent_table.to_string()),
                );
                if !column_names.is_empty() {
                    metadata.insert(
                        "columns".to_string(),
                        Value::Array(
                            column_names
                                .iter()
                                .cloned()
                                .map(Value::String)
                                .collect(),
                        ),
                    );
                }
                if let Some(foreign_key) = foreign_keys.first() {
                    if let Some(database_name) = foreign_key.referenced_database_name.as_ref() {
                        metadata.insert(
                            "referencedDatabaseName".to_string(),
                            Value::String(database_name.clone()),
                        );
                    }
                    metadata.insert(
                        "referencedTableName".to_string(),
                        Value::String(foreign_key.referenced_table_name.clone()),
                    );
                    metadata.insert(
                        "referencedColumns".to_string(),
                        Value::Array(
                            referenced_columns
                                .iter()
                                .cloned()
                                .map(Value::String)
                                .collect(),
                        ),
                    );
                    if let Some(rule) = foreign_key.on_update.as_ref() {
                        metadata.insert("onUpdate".to_string(), Value::String(rule.clone()));
                    }
                    if let Some(rule) = foreign_key.on_delete.as_ref() {
                        metadata.insert("onDelete".to_string(), Value::String(rule.clone()));
                    }
                }
                if let Some(clause) = check_clause.as_ref() {
                    metadata.insert("checkClause".to_string(), Value::String(clause.clone()));
                }
                Ok(LiveObjectDescription {
                    parent_name: resolved_parent,
                    identity: Some(resolved_identity),
                    columns,
                    primary_key: Vec::new(),
                    foreign_keys,
                    definition,
                    metadata: Some(Value::Object(metadata)),
                })
            }
            SupportedDatabaseType::Sqlite => unreachable!(),
        },
        DatabaseObjectType::Policy => {
            if connection.profile.db_type != SupportedDatabaseType::Postgresql {
                return Err(ServerError::new(
                    StatusCode::NOT_FOUND,
                    "Policies are not supported by this database",
                ));
            }
            let parent_table = resolved_parent.as_deref().unwrap_or("");
            let row = sqlx::query(
                r#"
                SELECT
                  policy.oid::text,
                  relation.relname::text,
                  policy.polpermissive::text,
                  CASE policy.polcmd
                    WHEN '*' THEN 'ALL'
                    WHEN 'r' THEN 'SELECT'
                    WHEN 'a' THEN 'INSERT'
                    WHEN 'w' THEN 'UPDATE'
                    WHEN 'd' THEN 'DELETE'
                  END::text AS command,
                  COALESCE(
                    (
                      SELECT string_agg(
                        CASE
                          WHEN policy_role.role_id = 0 THEN 'PUBLIC'
                          ELSE pg_catalog.quote_ident(role.rolname)
                        END,
                        ', '
                        ORDER BY policy_role.role_id
                      )
                      FROM unnest(policy.polroles) AS policy_role(role_id)
                      LEFT JOIN pg_catalog.pg_roles AS role
                        ON role.oid = policy_role.role_id
                    ),
                    'PUBLIC'
                  )::text AS roles,
                  pg_catalog.pg_get_expr(policy.polqual, policy.polrelid)::text AS using_expression,
                  pg_catalog.pg_get_expr(policy.polwithcheck, policy.polrelid)::text AS check_expression
                FROM pg_catalog.pg_policy AS policy
                INNER JOIN pg_catalog.pg_class AS relation
                  ON relation.oid = policy.polrelid
                INNER JOIN pg_catalog.pg_namespace AS namespace
                  ON namespace.oid = relation.relnamespace
                WHERE namespace.nspname = $1
                  AND relation.relname = $2
                  AND policy.polname = $3
                  AND ($4 = '' OR $4 = $3 OR policy.oid::text = $4)
                LIMIT 1
                "#,
            )
            .bind(&schema)
            .bind(parent_table)
            .bind(name)
            // Older clients do not send the optional catalog identity. In
            // that case the schema/table/name tuple is already unambiguous.
            .bind(identity.filter(|value| !value.trim().is_empty()).unwrap_or(""))
            .fetch_optional(&pool)
            .await
            .map_err(sqlx_server_error)?
            .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Policy not found"))?;
            let identity = any_string(&row, 0).unwrap_or(resolved_identity);
            let table_name = any_string(&row, 1).unwrap_or_else(|| parent_table.to_string());
            let permissive = any_string(&row, 2)
                .is_none_or(|value| value.eq_ignore_ascii_case("true"));
            let command = any_string(&row, 3).unwrap_or_else(|| "ALL".to_string());
            let roles = any_string(&row, 4).unwrap_or_else(|| "PUBLIC".to_string());
            let using_expression = any_string(&row, 5);
            let check_expression = any_string(&row, 6);

            let mut definition = format!(
                "CREATE POLICY {} ON {}.{}",
                quote_identifier(name),
                quote_identifier(&schema),
                quote_identifier(&table_name),
            );
            if !permissive {
                definition.push_str(" AS RESTRICTIVE");
            }
            definition.push_str(&format!(" FOR {command} TO {roles}"));
            if let Some(expression) = using_expression.as_deref() {
                definition.push_str(" USING ");
                definition.push_str(&postgresql_policy_expression(expression));
            }
            if let Some(expression) = check_expression.as_deref() {
                definition.push_str(" WITH CHECK ");
                definition.push_str(&postgresql_policy_expression(expression));
            }
            definition.push(';');

            Ok(LiveObjectDescription {
                parent_name: Some(table_name.clone()),
                identity: Some(identity),
                columns: Vec::new(),
                primary_key: Vec::new(),
                foreign_keys: Vec::new(),
                definition: Some(definition),
                metadata: Some(serde_json::json!({
                    "tableName": table_name,
                    "permissive": permissive,
                    "command": command,
                    "roles": roles,
                    "usingExpression": using_expression,
                    "checkExpression": check_expression,
                })),
            })
        }
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Unsupported live database object type",
        )),
    }
}

fn postgresql_policy_expression(expression: &str) -> String {
    let expression = expression.trim();
    if expression.starts_with('(') && expression.ends_with(')') {
        expression.to_string()
    } else {
        format!("({expression})")
    }
}

/// Return catalog metadata for relation-like objects.  The explorer already
/// exposes columns and definitions, but those alone hide important state such
/// as ownership, comments, storage, partitioning, and row security.  Keep the
/// payload a flexible JSON object so the shared mobile clients can render
/// engine-specific fields without growing a second protocol model for every
/// catalog release.
async fn live_relation_metadata(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    name: &str,
    object_type: DatabaseObjectType,
) -> Result<Option<Value>> {
    let pool = live_pool_for_database(connection, database_name).await?;
    let (database, schema) = live_scope(connection, database_name, schema_name);
    match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
            let row = sqlx::query(
                r#"
                SELECT
                  relation.relkind::text,
                  pg_get_userbyid(relation.relowner)::text,
                  obj_description(relation.oid, 'pg_class')::text,
                  pg_total_relation_size(relation.oid)::text,
                  relation.reltuples::text,
                  relation.relpages::text,
                  relation.relispartition::text,
                  relation.relrowsecurity::text,
                  relation.relforcerowsecurity::text,
                  parent_namespace.nspname::text,
                  parent_relation.relname::text,
                  pg_get_partkeydef(relation.oid)::text
                FROM pg_catalog.pg_class AS relation
                INNER JOIN pg_catalog.pg_namespace AS namespace
                  ON namespace.oid = relation.relnamespace
                LEFT JOIN pg_catalog.pg_inherits AS inheritance
                  ON inheritance.inhrelid = relation.oid
                LEFT JOIN pg_catalog.pg_class AS parent_relation
                  ON parent_relation.oid = inheritance.inhparent
                LEFT JOIN pg_catalog.pg_namespace AS parent_namespace
                  ON parent_namespace.oid = parent_relation.relnamespace
                WHERE namespace.nspname = $1
                  AND relation.relname = $2
                  AND relation.relkind = $3
                LIMIT 1
                "#,
            )
            .bind(&schema)
            .bind(name)
            .bind(match object_type {
                DatabaseObjectType::View => "v",
                DatabaseObjectType::MaterializedView => "m",
                _ => "r",
            })
            .fetch_optional(&pool)
            .await
            .map_err(sqlx_server_error)?;
            let Some(row) = row else {
                return Ok(None);
            };
            let mut metadata = Map::new();
            let insert = |metadata: &mut Map<String, Value>, key: &str, value: Option<String>| {
                if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
                    metadata.insert(key.to_string(), Value::String(value));
                }
            };
            insert(&mut metadata, "relationKind", any_string(&row, 0));
            insert(&mut metadata, "owner", any_string(&row, 1));
            insert(&mut metadata, "comment", any_string(&row, 2));
            insert(&mut metadata, "totalBytes", any_string(&row, 3));
            insert(&mut metadata, "estimatedRows", any_string(&row, 4));
            insert(&mut metadata, "pages", any_string(&row, 5));
            insert(&mut metadata, "isPartition", any_string(&row, 6));
            insert(&mut metadata, "rowSecurity", any_string(&row, 7));
            insert(&mut metadata, "forceRowSecurity", any_string(&row, 8));
            let parent_schema = any_string(&row, 9);
            let parent_name = any_string(&row, 10);
            if let Some(parent_name) = parent_name {
                let parent = parent_schema
                    .map(|schema| format!("{schema}.{parent_name}"))
                    .unwrap_or(parent_name);
                metadata.insert("partitionParent".to_string(), Value::String(parent));
            }
            insert(&mut metadata, "partitionKey", any_string(&row, 11));
            return Ok((!metadata.is_empty()).then_some(Value::Object(metadata)));
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            let row = sqlx::query(
                r#"
                SELECT table_type, engine, table_collation, table_comment,
                       create_options, table_rows, data_length, index_length
                FROM information_schema.tables
                WHERE table_schema = ? AND table_name = ?
                LIMIT 1
                "#,
            )
            .bind(&database)
            .bind(name)
            .fetch_optional(&pool)
            .await
            .map_err(sqlx_server_error)?;
            let Some(row) = row else {
                return Ok(None);
            };
            let mut metadata = Map::new();
            let insert = |metadata: &mut Map<String, Value>, key: &str, value: Option<String>| {
                if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
                    metadata.insert(key.to_string(), Value::String(value));
                }
            };
            insert(&mut metadata, "tableType", any_string(&row, 0));
            insert(&mut metadata, "engine", any_string(&row, 1));
            insert(&mut metadata, "collation", any_string(&row, 2));
            insert(&mut metadata, "comment", any_string(&row, 3));
            insert(&mut metadata, "createOptions", any_string(&row, 4));
            insert(&mut metadata, "estimatedRows", any_string(&row, 5));
            insert(&mut metadata, "dataBytes", any_string(&row, 6));
            insert(&mut metadata, "indexBytes", any_string(&row, 7));
            return Ok((!metadata.is_empty()).then_some(Value::Object(metadata)));
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    }
}

async fn live_relation_definition(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    name: &str,
    object_type: DatabaseObjectType,
) -> Result<Option<String>> {
    let pool = live_pool_for_database(connection, database_name).await?;
    let (_, schema) = live_scope(connection, database_name, schema_name);
    let row = match (connection.profile.db_type, object_type) {
        (SupportedDatabaseType::Postgresql, DatabaseObjectType::Table) => {
            return postgresql_table_definition(&pool, &schema, name).await;
        }
        (SupportedDatabaseType::Postgresql, DatabaseObjectType::View) => sqlx::query(
            "SELECT definition::text FROM pg_views WHERE schemaname = $1 AND viewname = $2",
        )
        .bind(&schema)
        .bind(name)
        .fetch_optional(&pool)
        .await
        .map_err(sqlx_server_error)?,
        (SupportedDatabaseType::Postgresql, DatabaseObjectType::MaterializedView) => {
            sqlx::query(
                "SELECT definition::text FROM pg_matviews WHERE schemaname = $1 AND matviewname = $2",
            )
            .bind(&schema)
            .bind(name)
            .fetch_optional(&pool)
            .await
            .map_err(sqlx_server_error)?
        }
        (SupportedDatabaseType::Mysql, DatabaseObjectType::Table)
        | (SupportedDatabaseType::Mariadb, DatabaseObjectType::Table) => {
            return mysql_table_definition(&pool, &schema, name).await;
        }
        (SupportedDatabaseType::Mysql, DatabaseObjectType::View)
        | (SupportedDatabaseType::Mariadb, DatabaseObjectType::View) => sqlx::query(
            "SELECT view_definition FROM information_schema.views WHERE table_schema = ? AND table_name = ?",
        )
        .bind(
            database_name
                .or(connection.profile.database_name.as_deref())
                .unwrap_or("mysql"),
        )
        .bind(name)
        .fetch_optional(&pool)
        .await
        .map_err(sqlx_server_error)?,
        _ => None,
    };
    Ok(row.and_then(|row| any_string(&row, 0)))
}

/// PostgreSQL intentionally does not expose a `SHOW CREATE TABLE` command.
/// Build a table statement from the same catalog deparsers used by pg_dump:
/// `format_type`, `pg_get_expr`, and `pg_get_constraintdef`. We only return a
/// statement when every table feature in scope can be recreated by it. A
/// source fragment is worse than no handoff target here: it looks runnable in
/// the mobile SQL editor while silently losing table state.
async fn postgresql_table_definition(
    pool: &AnyPool,
    schema: &str,
    table_name: &str,
) -> Result<Option<String>> {
    let relation = sqlx::query(
        r#"
        SELECT
          relation.relkind::text,
          relation.relpersistence::text,
          relation.relispartition::text,
          EXISTS (
            SELECT 1
            FROM pg_catalog.pg_inherits AS inheritance
            WHERE inheritance.inhrelid = relation.oid
              AND NOT relation.relispartition
          )::text AS has_regular_inheritance,
          (relation.reloftype <> 0)::text AS is_typed_table,
          relation.relrowsecurity::text,
          relation.relforcerowsecurity::text,
          relation.relhasrules::text,
          access_method.amname::text,
          array_to_string(relation.reloptions, ', ')::text,
          tablespace.spcname::text,
          pg_catalog.pg_get_partkeydef(relation.oid)::text
        FROM pg_catalog.pg_class AS relation
        INNER JOIN pg_catalog.pg_namespace AS namespace
          ON namespace.oid = relation.relnamespace
        LEFT JOIN pg_catalog.pg_am AS access_method
          ON access_method.oid = relation.relam
        LEFT JOIN pg_catalog.pg_tablespace AS tablespace
          ON tablespace.oid = relation.reltablespace
        WHERE namespace.nspname = $1
          AND relation.relname = $2
          AND relation.relkind IN ('r', 'p')
        "#,
    )
    .bind(schema)
    .bind(table_name)
    .fetch_optional(pool)
    .await
    .map_err(sqlx_server_error)?;
    let Some(relation) = relation else {
        return Ok(None);
    };

    let relation_kind = any_string(&relation, 0).unwrap_or_default();
    let persistence = any_string(&relation, 1).unwrap_or_default();
    let is_partition = postgres_catalog_true(&relation, 2);
    let has_regular_inheritance = postgres_catalog_true(&relation, 3);
    let is_typed_table = postgres_catalog_true(&relation, 4);
    let has_row_security = postgres_catalog_true(&relation, 5)
        || postgres_catalog_true(&relation, 6);
    let has_rules = postgres_catalog_true(&relation, 7);
    let access_method = any_string(&relation, 8);
    let reloptions = any_string(&relation, 9);
    let tablespace = any_string(&relation, 10);
    let partition_key = any_string(&relation, 11);

    // A partition child, inherited table, typed table, temporary table, RLS
    // policy, or rewrite rule needs extra DDL outside CREATE TABLE. Preserve
    // source-only semantics rather than falsely marking a partial statement
    // executable. Partitioned *parents* are safe because their children are
    // independent explorer objects.
    if !matches!(relation_kind.as_str(), "r" | "p")
        || is_partition
        || has_regular_inheritance
        || is_typed_table
        || has_row_security
        || has_rules
        || persistence == "t"
        || (relation_kind == "p" && partition_key.is_none())
    {
        return Ok(None);
    }

    // `attgenerated` and `attcompression` arrived after the oldest PostgreSQL
    // version supported by the live adapter. Select them only where available,
    // so opening a normal table never fails on an older server.
    let server_version_num = postgresql_server_version_num(pool).await?;
    let generated_mode_sql = if server_version_num >= 120_000 {
        r#"
          CASE attribute.attgenerated
            WHEN 's' THEN 'STORED'
            ELSE NULL
          END::text AS generated_mode
        "#
    } else {
        "NULL::text AS generated_mode"
    };
    let compression_method_sql = if server_version_num >= 140_000 {
        r#"
          CASE attribute.attcompression
            WHEN 'p' THEN 'pglz'
            WHEN 'l' THEN 'lz4'
            WHEN '' THEN NULL
            ELSE 'unsupported'
          END::text AS compression_method
        "#
    } else {
        "NULL::text AS compression_method"
    };
    let column_sql = format!(
        r#"
        SELECT
          attribute.attname::text,
          pg_catalog.format_type(attribute.atttypid, attribute.atttypmod)::text,
          attribute.attnotnull::text,
          pg_catalog.pg_get_expr(default_value.adbin, default_value.adrelid)::text,
          CASE attribute.attidentity
            WHEN 'a' THEN 'ALWAYS'
            WHEN 'd' THEN 'BY DEFAULT'
            ELSE NULL
          END::text AS identity_mode,
          {generated_mode_sql},
          CASE
            WHEN attribute.attcollation <> type_info.typcollation
             AND attribute.attcollation <> 0
            THEN pg_catalog.quote_ident(collation_namespace.nspname)
              || '.' || pg_catalog.quote_ident(catalog_collation.collname)
            ELSE NULL
          END::text AS collation_name,
          {compression_method_sql},
          (attribute.attstorage <> type_info.typstorage)::text AS has_custom_storage,
          CASE sequence_dependency.deptype
            WHEN 'a' THEN 'serial'
            WHEN 'i' THEN 'identity'
            ELSE NULL
          END::text AS sequence_dependency_kind,
          sequence_namespace.nspname::text,
          sequence_relation.relname::text,
          pg_catalog.format_type(sequence_config.seqtypid, NULL)::text,
          sequence_config.seqstart::text,
          sequence_config.seqmin::text,
          sequence_config.seqmax::text,
          sequence_config.seqincrement::text,
          sequence_config.seqcache::text,
          sequence_config.seqcycle::text
        FROM pg_catalog.pg_attribute AS attribute
        INNER JOIN pg_catalog.pg_class AS relation
          ON relation.oid = attribute.attrelid
        INNER JOIN pg_catalog.pg_namespace AS namespace
          ON namespace.oid = relation.relnamespace
        INNER JOIN pg_catalog.pg_type AS type_info
          ON type_info.oid = attribute.atttypid
        LEFT JOIN pg_catalog.pg_attrdef AS default_value
          ON default_value.adrelid = relation.oid
         AND default_value.adnum = attribute.attnum
        LEFT JOIN pg_catalog.pg_collation AS catalog_collation
          ON catalog_collation.oid = attribute.attcollation
        LEFT JOIN pg_catalog.pg_namespace AS collation_namespace
          ON collation_namespace.oid = catalog_collation.collnamespace
        LEFT JOIN pg_catalog.pg_depend AS sequence_dependency
          ON sequence_dependency.refobjid = relation.oid
         AND sequence_dependency.refobjsubid = attribute.attnum
         AND sequence_dependency.classid = 'pg_catalog.pg_class'::regclass
         AND sequence_dependency.refclassid = 'pg_catalog.pg_class'::regclass
         AND sequence_dependency.deptype IN ('a', 'i')
        LEFT JOIN pg_catalog.pg_class AS sequence_relation
          ON sequence_relation.oid = sequence_dependency.objid
        LEFT JOIN pg_catalog.pg_namespace AS sequence_namespace
          ON sequence_namespace.oid = sequence_relation.relnamespace
        LEFT JOIN pg_catalog.pg_sequence AS sequence_config
          ON sequence_config.seqrelid = sequence_relation.oid
        WHERE namespace.nspname = $1
          AND relation.relname = $2
          AND relation.relkind IN ('r', 'p')
          AND attribute.attnum > 0
          AND NOT attribute.attisdropped
        ORDER BY attribute.attnum
        "#,
    );
    let column_rows = sqlx::query(&column_sql)
        .bind(schema)
        .bind(table_name)
        .fetch_all(pool)
        .await
        .map_err(sqlx_server_error)?;
    if column_rows.is_empty() {
        return Ok(None);
    }

    let constraint_rows = sqlx::query(
        r#"
        SELECT
          catalog_constraint.conname::text,
          pg_catalog.pg_get_constraintdef(catalog_constraint.oid, true)::text,
          catalog_constraint.convalidated::text
        FROM pg_catalog.pg_constraint AS catalog_constraint
        INNER JOIN pg_catalog.pg_class AS relation
          ON relation.oid = catalog_constraint.conrelid
        INNER JOIN pg_catalog.pg_namespace AS namespace
          ON namespace.oid = relation.relnamespace
        WHERE namespace.nspname = $1
          AND relation.relname = $2
          AND catalog_constraint.contype IN ('p', 'u', 'f', 'c', 'x')
        ORDER BY catalog_constraint.conname
        "#,
    )
    .bind(schema)
    .bind(table_name)
    .fetch_all(pool)
    .await
    .map_err(sqlx_server_error)?;

    let mut column_definitions = Vec::with_capacity(column_rows.len() + constraint_rows.len());
    let mut prelude = Vec::new();
    let mut postlude = Vec::new();
    for row in &column_rows {
        let Some(column_definition) = postgres_table_column_definition(
            row,
            schema,
            table_name,
            &mut prelude,
            &mut postlude,
        ) else {
            return Ok(None);
        };
        column_definitions.push(column_definition);
    }
    for constraint in constraint_rows {
        // NOT VALID constraints can only be introduced through ALTER TABLE;
        // emitting them in CREATE TABLE would not recreate their validation
        // state. Do not advertise a partial source as safe executable DDL.
        if !postgres_catalog_true(&constraint, 2) {
            return Ok(None);
        }
        let (Some(name), Some(definition)) =
            (any_string(&constraint, 0), any_string(&constraint, 1))
        else {
            return Ok(None);
        };
        column_definitions.push(format!("CONSTRAINT {} {definition}", quote_identifier(&name)));
    }

    let mut create = String::from("CREATE ");
    if persistence == "u" {
        create.push_str("UNLOGGED ");
    }
    create.push_str("TABLE ");
    create.push_str(&format!(
        "{}.{} (\n  {}\n)",
        quote_identifier(schema),
        quote_identifier(table_name),
        column_definitions.join(",\n  ")
    ));
    if relation_kind == "p" {
        create.push_str(" PARTITION BY ");
        create.push_str(partition_key.as_deref().expect("checked partition key"));
    } else if server_version_num >= 120_000 {
        if let Some(access_method) = access_method.filter(|value| !value.is_empty()) {
            create.push_str(" USING ");
            create.push_str(&quote_identifier(&access_method));
        }
    }
    if let Some(reloptions) = reloptions.filter(|value| !value.is_empty()) {
        create.push_str(" WITH (");
        create.push_str(&reloptions);
        create.push(')');
    }
    if let Some(tablespace) = tablespace.filter(|value| !value.is_empty()) {
        create.push_str(" TABLESPACE ");
        create.push_str(&quote_identifier(&tablespace));
    }
    create.push(';');

    prelude.push(create);
    prelude.extend(postlude);
    Ok(Some(prelude.join("\n")))
}

async fn postgresql_server_version_num(pool: &AnyPool) -> Result<u32> {
    let row = sqlx::query("SHOW server_version_num")
        .fetch_one(pool)
        .await
        .map_err(sqlx_server_error)?;
    Ok(any_string(&row, 0)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_default())
}

fn postgres_catalog_true(row: &AnyRow, index: usize) -> bool {
    any_string(row, index).is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

/// Return a complete column clause, or `None` if the catalog reports a
/// feature that CREATE TABLE alone cannot faithfully represent.
fn postgres_table_column_definition(
    row: &AnyRow,
    table_schema: &str,
    table_name: &str,
    prelude: &mut Vec<String>,
    postlude: &mut Vec<String>,
) -> Option<String> {
    let name = any_string(row, 0)?;
    let data_type = any_string(row, 1)?;
    let is_not_null = postgres_catalog_true(row, 2);
    let mut default_value = any_string(row, 3);
    let identity_mode = any_string(row, 4);
    let generated_mode = any_string(row, 5);
    let collation = any_string(row, 6);
    let compression = any_string(row, 7);
    let has_custom_storage = postgres_catalog_true(row, 8);
    let sequence_dependency_kind = any_string(row, 9);

    // CREATE TABLE has no syntax for a non-default TOAST storage strategy.
    // Keeping the definition source-only is safer than silently discarding it.
    if has_custom_storage
        || compression.as_deref() == Some("unsupported")
        || (identity_mode.is_some() && generated_mode.is_some())
    {
        return None;
    }

    let mut definition = format!("{} {}", quote_identifier(&name), data_type);
    if let Some(compression) = compression {
        definition.push_str(" COMPRESSION ");
        definition.push_str(&quote_identifier(&compression));
    }
    if let Some(collation) = collation {
        definition.push_str(" COLLATE ");
        definition.push_str(&collation);
    }

    match (identity_mode, generated_mode, sequence_dependency_kind.as_deref()) {
        (Some(identity_mode), None, Some("identity")) => {
            let sequence_options = postgres_sequence_options(row)?;
            definition.push_str(" GENERATED ");
            definition.push_str(&identity_mode);
            definition.push_str(" AS IDENTITY (");
            definition.push_str(&sequence_options);
            definition.push(')');
        }
        (None, Some(generated_mode), None) if generated_mode == "STORED" => {
            let expression = default_value?;
            definition.push_str(" GENERATED ALWAYS AS (");
            definition.push_str(&expression);
            definition.push_str(") STORED");
        }
        (None, None, Some("serial")) => {
            let sequence_options = postgres_sequence_options(row)?;
            let sequence_schema = any_string(row, 10)?;
            let sequence_name = any_string(row, 11)?;
            let sequence_data_type = any_string(row, 12)?;
            // Serial columns are backed by an independently-created sequence.
            // Recreate that sequence first, then make the default and ownership
            // explicit so the table DDL remains executable after a drop.
            let expression = default_value.as_deref()?.trim_start();
            if !expression.to_ascii_lowercase().starts_with("nextval(") {
                return None;
            }
            let sequence_ref = postgres_qualified_identifier(&sequence_schema, &sequence_name);
            let table_ref = postgres_qualified_identifier(table_schema, table_name);
            prelude.push(format!(
                "CREATE SEQUENCE {sequence_ref} AS {sequence_data_type} {sequence_options};"
            ));
            postlude.push(format!(
                "ALTER SEQUENCE {sequence_ref} OWNED BY {table_ref}.{};",
                quote_identifier(&name)
            ));
            default_value = Some(format!(
                "nextval({}::regclass)",
                postgres_string_literal(&sequence_ref)
            ));
            definition.push_str(" DEFAULT ");
            definition.push_str(default_value.as_deref().expect("serial default set"));
        }
        (None, None, None) => {
            if let Some(default_value) = default_value {
                definition.push_str(" DEFAULT ");
                definition.push_str(&default_value);
            }
        }
        // An identity/generated marker without the required backing catalog
        // data is not a complete column definition. The same is true for a
        // serial dependency combined with an unexpected generated/default form.
        _ => return None,
    }

    if is_not_null {
        definition.push_str(" NOT NULL");
    }
    Some(definition)
}

fn postgres_sequence_options(row: &AnyRow) -> Option<String> {
    let start = any_string(row, 13)?;
    let minimum = any_string(row, 14)?;
    let maximum = any_string(row, 15)?;
    let increment = any_string(row, 16)?;
    let cache = any_string(row, 17)?;
    let cycle = match any_string(row, 18)?.as_str() {
        "true" => "CYCLE",
        "false" => "NO CYCLE",
        _ => return None,
    };
    Some(format!(
        "INCREMENT BY {increment} MINVALUE {minimum} MAXVALUE {maximum} START WITH {start} CACHE {cache} {cycle}"
    ))
}

fn postgres_qualified_identifier(schema: &str, name: &str) -> String {
    format!("{}.{}", quote_identifier(schema), quote_identifier(name))
}

fn postgres_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// MySQL and MariaDB provide the engine's exact table declaration through
/// SHOW CREATE TABLE. Keep it unchanged instead of attempting to rebuild
/// engine, charset, generated-column, partition, and table-option syntax from
/// information_schema fragments.
async fn mysql_table_definition(
    pool: &AnyPool,
    database: &str,
    table_name: &str,
) -> Result<Option<String>> {
    let statement = format!(
        "SHOW CREATE TABLE {}.{}",
        quote_mysql_identifier(database),
        quote_mysql_identifier(table_name)
    );
    let row = sqlx::query(&statement)
        .fetch_optional(pool)
        .await
        .map_err(sqlx_server_error)?;
    // SHOW CREATE TABLE returns the table name in column 0 and the DDL in
    // column 1. The positional lookup is stable across MySQL and MariaDB,
    // whereas the textual result-column name is not.
    Ok(row.and_then(|row| any_string(&row, 1)))
}

/// MySQL and MariaDB expose the full trigger declaration through SHOW CREATE
/// TRIGGER. `information_schema.triggers.action_statement` contains only the
/// body, so it must not be used as the primary SQL-editor handoff.
async fn mysql_trigger_definition(
    pool: &MySqlPool,
    database: &str,
    trigger_name: &str,
) -> Result<Option<String>> {
    let statement = format!(
        "SHOW CREATE TRIGGER {}.{}",
        quote_mysql_identifier(database),
        quote_mysql_identifier(trigger_name),
    );
    let row = sqlx::query(&statement)
        .fetch_optional(pool)
        .await
        .map_err(sqlx_server_error)?;
    // The SQL Original Statement is column 2 for both MySQL and MariaDB:
    // trigger name, SQL mode, then the complete CREATE TRIGGER declaration.
    row.map(|row| row.try_get::<Option<String>, _>(2))
        .transpose()
        .map(Option::flatten)
        .map_err(sqlx_server_error)
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
            DatabaseObjectType::Table
                | DatabaseObjectType::View
                | DatabaseObjectType::MaterializedView
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
            let target_database_name = foreign_key
                .referenced_database_name
                .clone()
                .or_else(|| object.database_name.clone());
            let target_schema_name = foreign_key.referenced_schema_name.clone();
            relationships.push(DatabaseRelationalSchemaRelationship {
                id: relational_relationship_id(
                    object.database_name.as_deref(),
                    object.schema_name.as_deref(),
                    &object.name,
                    &foreign_key.column_name,
                    target_database_name.as_deref(),
                    target_schema_name.as_deref(),
                    &foreign_key.referenced_table_name,
                    &foreign_key.referenced_column_name,
                    foreign_key.name.as_deref(),
                ),
                name: foreign_key.name.clone(),
                source_database_name: object.database_name.clone(),
                source_schema_name: object.schema_name.clone(),
                source_table_name: object.name.clone(),
                source_column_name: foreign_key.column_name.clone(),
                target_database_name,
                target_schema_name,
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

    let mut external_tables = Vec::new();
    for relationship in &relationships {
        let target_is_in_scope = tables.iter().any(|table| {
            relational_table_matches(
                table,
                relationship.target_database_name.as_deref(),
                relationship.target_schema_name.as_deref(),
                &relationship.target_table_name,
            )
        });
        if target_is_in_scope {
            continue;
        }

        let external_table = external_tables.iter_mut().find(|table| {
            relational_table_matches(
                table,
                relationship.target_database_name.as_deref(),
                relationship.target_schema_name.as_deref(),
                &relationship.target_table_name,
            )
        });
        let table = match external_table {
            Some(table) => table,
            None => {
                external_tables.push(DatabaseRelationalSchemaTable {
                    database_name: relationship.target_database_name.clone(),
                    schema_name: relationship.target_schema_name.clone(),
                    name: relationship.target_table_name.clone(),
                    object_type: DatabaseObjectType::Table,
                    columns: Vec::new(),
                    is_external: true,
                });
                external_tables
                    .last_mut()
                    .expect("an inserted external relationship target exists")
            }
        };
        if !relationship.target_column_name.trim().is_empty()
            && !table.columns.iter().any(|column| {
                column
                    .name
                    .eq_ignore_ascii_case(&relationship.target_column_name)
            })
        {
            table.columns.push(DatabaseObjectColumn {
                name: relationship.target_column_name.clone(),
                data_type: None,
                native_type: None,
                nullable: None,
                default_value: None,
                extra: None,
                is_primary_key: false,
            });
        }
    }
    tables.extend(external_tables);

    Ok(DatabaseRelationalSchema {
        scope_type,
        scope_name: scope_name.to_string(),
        tables,
        relationships,
    })
}

fn relational_table_matches(
    table: &DatabaseRelationalSchemaTable,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    table_name: &str,
) -> bool {
    relational_scope_name_matches(table.database_name.as_deref(), database_name)
        && relational_scope_name_matches(table.schema_name.as_deref(), schema_name)
        && table.name.eq_ignore_ascii_case(table_name)
}

fn relational_scope_name_matches(left: Option<&str>, right: Option<&str>) -> bool {
    match (
        left.map(str::trim).filter(|value| !value.is_empty()),
        right.map(str::trim).filter(|value| !value.is_empty()),
    ) {
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        (None, None) => true,
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn relational_relationship_id(
    source_database_name: Option<&str>,
    source_schema_name: Option<&str>,
    source_table_name: &str,
    source_column_name: &str,
    target_database_name: Option<&str>,
    target_schema_name: Option<&str>,
    target_table_name: &str,
    target_column_name: &str,
    constraint_name: Option<&str>,
) -> String {
    [
        source_database_name.unwrap_or(""),
        source_schema_name.unwrap_or(""),
        source_table_name,
        source_column_name,
        target_database_name.unwrap_or(""),
        target_schema_name.unwrap_or(""),
        target_table_name,
        target_column_name,
        constraint_name.unwrap_or(""),
    ]
    .join(":")
}

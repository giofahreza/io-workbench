    #[tokio::test]
    async fn postgres_live_queries_preserve_returning_rows_and_empty_columns() {
        let Some(connection) = postgres_test_connection() else {
            return;
        };
        let url =
            connection_url_for_database(&connection, connection.profile.database_name.as_deref())
                .expect("build live PostgreSQL URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live PostgreSQL");
        let table_name = new_id("iowb_database_parity").replace('-', "_");
        let table_ref = quote_identifier(&table_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE TEMP TABLE {table_ref} (
                id BIGSERIAL PRIMARY KEY,
                amount NUMERIC NOT NULL,
                payload JSONB NOT NULL,
                uid UUID NOT NULL,
                bytes BYTEA NOT NULL,
                numbers BIGINT[] NOT NULL,
                elapsed INTERVAL NOT NULL,
                location POINT NOT NULL,
                boundary CIRCLE NOT NULL
            )
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create PostgreSQL parity table");

        let insert = execute_postgres_query(
            &pool,
            &format!(
                r#"
                INSERT INTO {table_ref} (
                    amount, payload, uid, bytes, numbers, elapsed, location, boundary
                ) VALUES (
                    1234567890.123456789,
                    '{{"ok":true,"nested":[1,2]}}'::jsonb,
                    '123e4567-e89b-12d3-a456-426614174000'::uuid,
                    decode('AAEC/w==', 'base64'),
                    ARRAY[1::bigint, 9007199254740993::bigint],
                    '1 year 2 mons 3 days 04:05:06.789'::interval,
                    point(1.5, -2.25),
                    circle(point(1, 2), 3.5)
                )
                RETURNING id, amount, payload, uid, bytes, numbers, elapsed, location, boundary
                "#,
            ),
            1000,
            Some("public"),
        )
        .await
        .expect("execute INSERT RETURNING");
        assert_eq!(insert.statement_type, DatabaseQueryStatementType::Insert);
        assert_eq!(insert.row_count, 1);
        assert_eq!(insert.returned_row_count, 1);
        assert_eq!(insert.columns.len(), 9);
        let row = insert.rows.first().expect("returned PostgreSQL row");
        assert!(row.get("id").is_some_and(Value::is_string));
        assert_eq!(
            row.get("amount"),
            Some(&Value::String("1234567890.123456789".to_string())),
        );
        assert_eq!(
            row.get("payload"),
            Some(&serde_json::json!({"ok": true, "nested": [1, 2]}))
        );
        assert_eq!(
            row.get("uid"),
            Some(&Value::String(
                "123e4567-e89b-12d3-a456-426614174000".to_string(),
            )),
        );
        assert_eq!(
            row.get("bytes"),
            Some(&serde_json::json!({
                "type": "buffer",
                "encoding": "base64",
                "value": "AAEC/w==",
            })),
        );
        assert_eq!(
            row.get("numbers"),
            Some(&serde_json::json!(["1", "9007199254740993"]))
        );
        assert_eq!(
            row.get("elapsed"),
            Some(&serde_json::json!({
                "years": 1,
                "months": 2,
                "days": 3,
                "hours": 4,
                "minutes": 5,
                "seconds": 6,
                "milliseconds": 789,
            })),
        );
        assert_eq!(
            row.get("location"),
            Some(&serde_json::json!({"x": 1.5, "y": -2.25}))
        );
        assert_eq!(
            row.get("boundary"),
            Some(&serde_json::json!({"x": 1.0, "y": 2.0, "radius": 3.5})),
        );

        let empty = execute_postgres_query(
            &pool,
            &format!("SELECT amount, payload FROM {table_ref} WHERE FALSE"),
            1000,
            Some("public"),
        )
        .await
        .expect("execute empty PostgreSQL select");
        assert_eq!(empty.statement_type, DatabaseQueryStatementType::Select);
        assert_eq!(empty.row_count, 0);
        assert!(empty.rows.is_empty());
        assert_eq!(
            empty
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec!["amount", "payload"],
        );

        sqlx::raw_sql(&format!("DROP TABLE {table_ref}"))
            .execute(&pool)
            .await
            .expect("drop PostgreSQL parity table");
    }

    #[tokio::test]
    async fn postgres_materialized_views_are_explorable_and_readable() {
        let Some(connection) = postgres_test_connection() else {
            return;
        };
        let database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "postgres".to_string());
        let url = connection_url_for_database(&connection, Some(&database_name))
            .expect("build materialized-view PostgreSQL URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live PostgreSQL");
        let view_name = new_id("iowb_materialized_view_test").replace('-', "_");
        let view_ref = quote_identifier(&view_name);
        sqlx::raw_sql(&format!(
            "CREATE MATERIALIZED VIEW {view_ref} AS SELECT 7::integer AS id, 'ready'::text AS status",
        ))
        .execute(&pool)
        .await
        .expect("create PostgreSQL materialized view");

        let verification = async {
            let objects = live_objects(&connection, Some(&database_name), Some("public")).await?;
            assert!(objects.iter().any(|object| {
                object.name == view_name
                    && object.object_type == DatabaseObjectType::MaterializedView
            }));

            let page = read_live_table_data(
                &connection,
                Some(&database_name),
                Some("public"),
                &view_name,
                50,
                0,
                true,
            )
            .await?;
            assert_eq!(page.row_count, 1);
            assert_eq!(page.total_row_count, Some(1));
            assert_eq!(
                page.columns.iter().map(|column| column.name.as_str()).collect::<Vec<_>>(),
                vec!["id", "status"],
            );
            assert_eq!(
                page.rows.first().and_then(|row| row.get("status")),
                Some(&Value::String("ready".to_string())),
            );
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!("DROP MATERIALIZED VIEW IF EXISTS {view_ref}"))
            .execute(&pool)
            .await
            .expect("drop PostgreSQL materialized view");
        verification.expect("materialized view should be discoverable and readable");
    }

    #[tokio::test]
    async fn postgres_row_security_policies_are_explorable_with_complete_details() {
        let Some(connection) = postgres_test_connection() else {
            return;
        };
        let database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "postgres".to_string());
        let url = connection_url_for_database(&connection, Some(&database_name))
            .expect("build row-security-policy PostgreSQL URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live PostgreSQL");
        let table_name = new_id("iowb_policy_table").replace('-', "_");
        let policy_name = new_id("tenant_read").replace('-', "_");
        let table_ref = quote_identifier(&table_name);
        let policy_ref = quote_identifier(&policy_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE TABLE {table_ref} (id integer PRIMARY KEY, tenant_id integer NOT NULL);
            ALTER TABLE {table_ref} ENABLE ROW LEVEL SECURITY;
            CREATE POLICY {policy_ref} ON {table_ref}
              FOR SELECT TO PUBLIC USING (tenant_id > 0);
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create PostgreSQL row-security policy");

        let verification = async {
            let objects = live_objects(&connection, Some(&database_name), Some("public")).await?;
            let policy = objects
                .iter()
                .find(|object| {
                    object.name == policy_name
                        && object.object_type == DatabaseObjectType::Policy
                        && object.parent_name.as_deref() == Some(table_name.as_str())
                })
                .expect("discovered row-security policy");
            assert!(policy
                .identity
                .as_deref()
                .is_some_and(|identity| identity != policy_name));

            let details = describe_live_object(
                &connection,
                DatabaseObjectType::Policy,
                Some(&database_name),
                Some("public"),
                &policy_name,
                Some(&table_name),
                policy.identity.as_deref(),
            )
            .await?;
            assert_eq!(details.parent_name.as_deref(), Some(table_name.as_str()));
            assert_eq!(details.identity, policy.identity);
            assert_eq!(
                details
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("command"))
                    .and_then(Value::as_str),
                Some("SELECT"),
            );
            assert_eq!(
                details
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("roles"))
                    .and_then(Value::as_str),
                Some("PUBLIC"),
            );
            let definition = details.definition.as_deref().expect("policy definition");
            assert!(definition.starts_with("CREATE POLICY "));
            assert!(definition.contains(" FOR SELECT TO PUBLIC"));
            assert!(definition.contains(" USING (tenant_id > 0)"));

            // Identity is optional in the API for compatibility with older
            // clients; the qualified policy name must still resolve.
            let legacy_details = describe_live_object(
                &connection,
                DatabaseObjectType::Policy,
                Some(&database_name),
                Some("public"),
                &policy_name,
                Some(&table_name),
                None,
            )
            .await?;
            assert_eq!(legacy_details.identity, policy.identity);
            let name_identity_details = describe_live_object(
                &connection,
                DatabaseObjectType::Policy,
                Some(&database_name),
                Some("public"),
                &policy_name,
                Some(&table_name),
                Some(&policy_name),
            )
            .await?;
            assert_eq!(name_identity_details.identity, policy.identity);
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!("DROP TABLE IF EXISTS {table_ref} CASCADE"))
            .execute(&pool)
            .await
            .expect("drop PostgreSQL row-security fixture");
        verification.expect("row-security policy should be discoverable and inspectable");
    }

    #[tokio::test]
    async fn postgres_identity_sequences_are_explorable() {
        let Some(connection) = postgres_test_connection() else {
            return;
        };
        let database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "postgres".to_string());
        let url = connection_url_for_database(&connection, Some(&database_name))
            .expect("build identity-sequence PostgreSQL URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live PostgreSQL");
        // Keep the generated identity sequence name below PostgreSQL's
        // 63-character identifier limit so the assertion targets discovery,
        // not identifier truncation.
        let table_name = new_id("iowb_ident_seq").replace('-', "_");
        let sequence_name = format!("{table_name}_id_seq");
        let table_ref = quote_identifier(&table_name);
        sqlx::raw_sql(&format!(
            "CREATE TABLE {table_ref} (id integer GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY)",
        ))
        .execute(&pool)
        .await
        .expect("create PostgreSQL identity table");

        let verification = async {
            let objects = live_objects(&connection, Some(&database_name), Some("public")).await?;
            assert!(objects.iter().any(|object| {
                object.name == sequence_name && object.object_type == DatabaseObjectType::Sequence
            }));
            let sequence = describe_live_object(
                &connection,
                DatabaseObjectType::Sequence,
                Some(&database_name),
                Some("public"),
                &sequence_name,
                None,
                Some(&sequence_name),
            )
            .await?;
            assert_eq!(sequence.identity.as_deref(), Some(sequence_name.as_str()));
            assert_eq!(
                sequence
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("increment"))
                    .and_then(Value::as_str),
                Some("1"),
            );
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!("DROP TABLE IF EXISTS {table_ref}"))
            .execute(&pool)
            .await
            .expect("drop PostgreSQL identity table");
        verification.expect("identity-backed sequence should be discoverable");
    }

    #[tokio::test]
    async fn postgres_table_definition_is_complete_and_recreatable() {
        let Some(connection) = postgres_test_connection() else {
            return;
        };
        let database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "postgres".to_string());
        let url = connection_url_for_database(&connection, Some(&database_name))
            .expect("build PostgreSQL URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live PostgreSQL");
        let schema_name = new_id("iowb_table_ddl_schema").replace('-', "_");
        let parent_table_name = new_id("iowb_table_ddl_parent").replace('-', "_");
        let child_table_name = new_id("iowb_table_ddl_child").replace('-', "_");
        let schema_ref = quote_identifier(&schema_name);
        let parent_ref = quote_identifier(&parent_table_name);
        let child_ref = quote_identifier(&child_table_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE SCHEMA {schema_ref};
            CREATE TABLE {schema_ref}.{parent_ref} (id integer PRIMARY KEY);
            CREATE TABLE {schema_ref}.{child_ref} (
                id bigint GENERATED BY DEFAULT AS IDENTITY
                    (START WITH 10 INCREMENT BY 5 MINVALUE 5 CACHE 3) NOT NULL,
                legacy_id serial NOT NULL,
                parent_id integer NOT NULL,
                label text NOT NULL DEFAULT 'new',
                compressed_payload text COMPRESSION pglz,
                doubled_id bigint GENERATED ALWAYS AS (id * 2) STORED,
                CONSTRAINT {child_table_name}_pk PRIMARY KEY (id),
                CONSTRAINT {child_table_name}_legacy_unique UNIQUE (legacy_id),
                CONSTRAINT {child_table_name}_parent_fk FOREIGN KEY (parent_id)
                    REFERENCES {schema_ref}.{parent_ref}(id) ON DELETE RESTRICT,
                CONSTRAINT {child_table_name}_label_check CHECK (char_length(label) > 0)
            ) WITH (fillfactor = 80);
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create PostgreSQL table-DDL fixture");

        let verification = async {
            let details = describe_live_object(
                &connection,
                DatabaseObjectType::Table,
                Some(&database_name),
                Some(&schema_name),
                &child_table_name,
                None,
                Some(&child_table_name),
            )
            .await?;
            let definition = details
                .definition
                .clone()
                .expect("complete PostgreSQL table definition");
            assert!(database_definition_is_executable(
                SupportedDatabaseType::Postgresql,
                DatabaseObjectType::Table,
                Some(&definition),
            ));
            let normalized = definition.to_ascii_uppercase();
            assert!(normalized.contains("CREATE SEQUENCE"));
            assert!(normalized.contains("CREATE TABLE"));
            assert!(normalized.contains("GENERATED BY DEFAULT AS IDENTITY"));
            assert!(normalized.contains("GENERATED ALWAYS AS"));
            assert!(normalized.contains("COMPRESSION"));
            assert!(normalized.contains("FOREIGN KEY"));
            assert!(normalized.contains("CHECK"));
            assert!(normalized.contains("WITH (FILLFACTOR=80)"));
            assert!(normalized.contains("ALTER SEQUENCE"));

            // The statement must be directly reusable, not merely look like
            // DDL. Dropping the serial-backed table also drops its sequence,
            // so this specifically proves that the generated sequence prelude
            // is sufficient for recreation.
            sqlx::raw_sql(&format!("DROP TABLE {schema_ref}.{child_ref}"))
                .execute(&pool)
                .await
                .map_err(sqlx_server_error)?;
            sqlx::raw_sql(&definition)
                .execute(&pool)
                .await
                .map_err(sqlx_server_error)?;
            let recreated = sqlx::query(
                r#"
                SELECT 1
                FROM information_schema.tables
                WHERE table_schema = $1 AND table_name = $2
                "#,
            )
            .bind(&schema_name)
            .bind(&child_table_name)
            .fetch_optional(&pool)
            .await
            .map_err(sqlx_server_error)?;
            assert!(recreated.is_some());
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!("DROP SCHEMA IF EXISTS {schema_ref} CASCADE"))
            .execute(&pool)
            .await
            .expect("drop PostgreSQL table-DDL fixture");
        verification.expect("PostgreSQL table DDL should recreate the full base table");
    }

    #[tokio::test]
    async fn postgres_trigger_details_are_scoped_to_the_parent_table() {
        let Some(connection) = postgres_test_connection() else {
            return;
        };
        let database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "postgres".to_string());
        let url = connection_url_for_database(&connection, Some(&database_name))
            .expect("build trigger PostgreSQL URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live PostgreSQL");
        let first_table_name = new_id("iowb_trigger_first").replace('-', "_");
        let second_table_name = new_id("iowb_trigger_second").replace('-', "_");
        let function_name = new_id("iowb_trigger_function").replace('-', "_");
        let trigger_name = "iowb_shared_trigger";
        let first_table_ref = quote_identifier(&first_table_name);
        let second_table_ref = quote_identifier(&second_table_name);
        let function_ref = quote_identifier(&function_name);
        let trigger_ref = quote_identifier(trigger_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE TABLE {first_table_ref} (id integer);
            CREATE TABLE {second_table_ref} (id integer);
            CREATE FUNCTION {function_ref}() RETURNS trigger
            LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END; $$;
            CREATE TRIGGER {trigger_ref}
            BEFORE INSERT ON {first_table_ref}
            FOR EACH ROW EXECUTE FUNCTION {function_ref}();
            CREATE TRIGGER {trigger_ref}
            BEFORE INSERT ON {second_table_ref}
            FOR EACH ROW EXECUTE FUNCTION {function_ref}();
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create PostgreSQL trigger fixtures");

        let verification = async {
            let objects = live_objects(&connection, Some(&database_name), Some("public")).await?;
            let second_trigger = objects
                .iter()
                .find(|object| {
                    object.object_type == DatabaseObjectType::Trigger
                        && object.name == trigger_name
                        && object.parent_name.as_deref() == Some(second_table_name.as_str())
                })
                .expect("second trigger summary");
            let details = describe_live_object(
                &connection,
                DatabaseObjectType::Trigger,
                Some(&database_name),
                Some("public"),
                trigger_name,
                Some(&second_table_name),
                second_trigger.identity.as_deref(),
            )
            .await?;
            assert_eq!(details.parent_name.as_deref(), Some(second_table_name.as_str()));
            assert_eq!(
                details
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("tableName"))
                    .and_then(Value::as_str),
                Some(second_table_name.as_str()),
            );
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!(
            "DROP TABLE IF EXISTS {first_table_ref}; \
             DROP TABLE IF EXISTS {second_table_ref}; \
             DROP FUNCTION IF EXISTS {function_ref}()",
        ))
        .execute(&pool)
        .await
        .expect("drop PostgreSQL trigger fixtures");
        verification.expect("trigger details should use the selected parent table");
    }

    #[tokio::test]
    async fn postgres_composite_keys_and_multi_event_triggers_are_exact() {
        let Some(connection) = postgres_test_connection() else {
            return;
        };
        let database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "postgres".to_string());
        let url = connection_url_for_database(&connection, Some(&database_name))
            .expect("build PostgreSQL URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live PostgreSQL");
        let parent_table_name = new_id("iowb_composite_parent").replace('-', "_");
        let child_table_name = new_id("iowb_composite_child").replace('-', "_");
        let function_name = new_id("iowb_multi_event_function").replace('-', "_");
        let trigger_name = new_id("iowb_multi_event_trigger").replace('-', "_");
        let parent_table_ref = quote_identifier(&parent_table_name);
        let child_table_ref = quote_identifier(&child_table_name);
        let function_ref = quote_identifier(&function_name);
        let trigger_ref = quote_identifier(&trigger_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE TABLE {parent_table_ref} (
                first_key integer NOT NULL,
                second_key integer NOT NULL,
                PRIMARY KEY (first_key, second_key)
            );
            CREATE TABLE {child_table_ref} (
                first_key integer,
                second_key integer,
                CONSTRAINT child_parent_key FOREIGN KEY (first_key, second_key)
                    REFERENCES {parent_table_ref} (first_key, second_key)
                    ON UPDATE CASCADE
                    ON DELETE SET NULL
            );
            CREATE FUNCTION {function_ref}() RETURNS trigger
            LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END; $$;
            CREATE TRIGGER {trigger_ref}
            BEFORE INSERT OR UPDATE ON {child_table_ref}
            FOR EACH ROW EXECUTE FUNCTION {function_ref}();
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create composite-key PostgreSQL fixture");

        let verification = async {
            let live_pool = live_pool_for_database(&connection, Some(&database_name)).await?;
            let primary_key = live_primary_keys(
                &live_pool,
                SupportedDatabaseType::Postgresql,
                "public",
                &parent_table_name,
            )
            .await?;
            assert_eq!(primary_key, vec!["first_key", "second_key"]);

            let foreign_keys = live_foreign_keys(
                &live_pool,
                SupportedDatabaseType::Postgresql,
                "public",
                &child_table_name,
            )
            .await?;
            assert_eq!(foreign_keys.len(), 2);
            assert_eq!(foreign_keys[0].column_name, "first_key");
            assert_eq!(foreign_keys[0].referenced_column_name, "first_key");
            assert_eq!(foreign_keys[1].column_name, "second_key");
            assert_eq!(foreign_keys[1].referenced_column_name, "second_key");
            assert!(foreign_keys
                .iter()
                .all(|key| key.on_update.as_deref() == Some("CASCADE")));
            assert!(foreign_keys
                .iter()
                .all(|key| key.on_delete.as_deref() == Some("SET NULL")));

            let matching_triggers = live_objects(&connection, Some(&database_name), Some("public"))
                .await?
                .into_iter()
                .filter(|object| {
                    object.object_type == DatabaseObjectType::Trigger
                        && object.name == trigger_name
                        && object.parent_name.as_deref() == Some(child_table_name.as_str())
                })
                .collect::<Vec<_>>();
            assert_eq!(matching_triggers.len(), 1);
            let trigger = matching_triggers.first().expect("trigger summary");
            assert!(trigger.identity.as_deref().is_some_and(|identity| {
                !identity.is_empty() && identity != trigger_name && identity.parse::<u64>().is_ok()
            }));
            let details = describe_live_object(
                &connection,
                DatabaseObjectType::Trigger,
                Some(&database_name),
                Some("public"),
                &trigger_name,
                Some(&child_table_name),
                trigger.identity.as_deref(),
            )
            .await?;
            let definition = details.definition.unwrap_or_default().to_ascii_uppercase();
            assert!(definition.contains("CREATE TRIGGER"));
            assert!(definition.contains("INSERT"));
            assert!(definition.contains("UPDATE"));
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!(
            "DROP TABLE IF EXISTS {child_table_ref}; \
             DROP TABLE IF EXISTS {parent_table_ref}; \
             DROP FUNCTION IF EXISTS {function_ref}()",
        ))
        .execute(&pool)
        .await
        .expect("drop composite-key PostgreSQL fixture");
        verification.expect("PostgreSQL composite-key and trigger metadata should be exact");
    }

    #[tokio::test]
    async fn postgres_exclusion_constraints_are_explorable_and_described() {
        let Some(connection) = postgres_test_connection() else {
            return;
        };
        let database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "postgres".to_string());
        let url = connection_url_for_database(&connection, Some(&database_name))
            .expect("build PostgreSQL URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live PostgreSQL");
        let table_name = new_id("iowb_exclusion").replace('-', "_");
        let constraint_name = new_id("iowb_exclusion_constraint").replace('-', "_");
        let table_ref = quote_identifier(&table_name);
        let constraint_ref = quote_identifier(&constraint_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE TABLE {table_ref} (
                period int4range NOT NULL,
                CONSTRAINT {constraint_ref} EXCLUDE USING gist (period WITH &&)
            )
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create PostgreSQL exclusion-constraint fixture");

        let verification = async {
            let summary = live_objects(&connection, Some(&database_name), Some("public"))
                .await?
                .into_iter()
                .find(|object| {
                    object.object_type == DatabaseObjectType::Constraint
                        && object.name == constraint_name
                        && object.parent_name.as_deref() == Some(table_name.as_str())
                })
                .expect("exclusion constraint summary");
            let details = describe_live_object(
                &connection,
                DatabaseObjectType::Constraint,
                Some(&database_name),
                Some("public"),
                &constraint_name,
                Some(&table_name),
                summary.identity.as_deref(),
            )
            .await?;
            assert_eq!(details.parent_name.as_deref(), Some(table_name.as_str()));
            assert_eq!(
                details
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("constraintType"))
                    .and_then(Value::as_str),
                Some("EXCLUSION"),
            );
            assert!(details
                .definition
                .as_deref()
                .is_some_and(|definition| definition.to_ascii_uppercase().contains("EXCLUDE")));
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!("DROP TABLE IF EXISTS {table_ref}"))
            .execute(&pool)
            .await
            .expect("drop PostgreSQL exclusion-constraint fixture");
        verification.expect("PostgreSQL exclusion constraints should be explorable");
    }

    #[tokio::test]
    async fn postgres_relational_schema_uses_external_stub_for_cross_schema_foreign_key() {
        let Some(connection) = postgres_test_connection() else {
            return;
        };
        let database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "postgres".to_string());
        let url = connection_url_for_database(&connection, Some(&database_name))
            .expect("build PostgreSQL URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live PostgreSQL");
        let source_schema_name = new_id("iowb_fk_source_schema").replace('-', "_");
        let target_schema_name = new_id("iowb_fk_target_schema").replace('-', "_");
        let shared_table_name = new_id("iowb_fk_same_table").replace('-', "_");
        let child_table_name = new_id("iowb_fk_child").replace('-', "_");
        let source_schema_ref = quote_identifier(&source_schema_name);
        let target_schema_ref = quote_identifier(&target_schema_name);
        let shared_table_ref = quote_identifier(&shared_table_name);
        let child_table_ref = quote_identifier(&child_table_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE SCHEMA {source_schema_ref};
            CREATE SCHEMA {target_schema_ref};
            CREATE TABLE {target_schema_ref}.{shared_table_ref} (id integer PRIMARY KEY);
            CREATE TABLE {source_schema_ref}.{shared_table_ref} (id integer PRIMARY KEY);
            CREATE TABLE {source_schema_ref}.{child_table_ref} (
                parent_id integer REFERENCES {target_schema_ref}.{shared_table_ref}(id)
            );
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create cross-schema PostgreSQL fixtures");

        let verification = async {
            let objects = live_objects(
                &connection,
                Some(&database_name),
                Some(&source_schema_name),
            )
            .await?;
            let graph = live_relational_schema(
                &connection,
                DatabaseObjectType::Schema,
                &source_schema_name,
                Some(&source_schema_name),
                &objects,
            )
            .await?;
            let relationship = graph
                .relationships
                .iter()
                .find(|relationship| relationship.source_table_name == child_table_name)
                .expect("cross-schema relationship");
            assert_eq!(
                relationship.source_database_name.as_deref(),
                Some(database_name.as_str()),
            );
            assert_eq!(
                relationship.source_schema_name.as_deref(),
                Some(source_schema_name.as_str()),
            );
            assert_eq!(
                relationship.target_database_name.as_deref(),
                Some(database_name.as_str()),
            );
            assert_eq!(
                relationship.target_schema_name.as_deref(),
                Some(target_schema_name.as_str()),
            );
            assert_eq!(relationship.target_table_name, shared_table_name);
            let tables_with_shared_name = graph
                .tables
                .iter()
                .filter(|table| table.name == shared_table_name)
                .collect::<Vec<_>>();
            assert_eq!(tables_with_shared_name.len(), 2);
            assert!(tables_with_shared_name.iter().any(|table| {
                !table.is_external
                    && table.schema_name.as_deref() == Some(source_schema_name.as_str())
            }));
            let external_target = tables_with_shared_name
                .iter()
                .find(|table| {
                    table.is_external
                        && table.database_name.as_deref() == Some(database_name.as_str())
                        && table.schema_name.as_deref() == Some(target_schema_name.as_str())
                })
                .expect("fully-qualified external target stub");
            assert_eq!(
                external_target
                    .columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>(),
                vec!["id"],
            );
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!(
            "DROP SCHEMA IF EXISTS {source_schema_ref} CASCADE; \
             DROP SCHEMA IF EXISTS {target_schema_ref} CASCADE",
        ))
        .execute(&pool)
        .await
        .expect("drop cross-schema PostgreSQL fixtures");
        verification.expect("PostgreSQL graph should retain cross-schema target identity");
    }

    #[tokio::test]
    async fn mysql_live_queries_preserve_complex_values_empty_columns_and_meta() {
        let Some(connection) = mysql_test_connection() else {
            return;
        };
        let url =
            connection_url_for_database(&connection, connection.profile.database_name.as_deref())
                .expect("build live MySQL URL");
        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live MySQL");
        let table_name = new_id("iowb_database_parity").replace('-', "_");
        let table_ref = format!("`{}`", table_name.replace('`', "``"));
        sqlx::raw_sql(&format!(
            r#"
            CREATE TEMPORARY TABLE {table_ref} (
                id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
                amount DECIMAL(30, 9) NOT NULL,
                payload JSON NOT NULL,
                bytes BLOB NOT NULL,
                location POINT NOT NULL
            )
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create MySQL parity table");

        let insert = execute_mysql_query(
            &pool,
            &format!(
                r#"
                INSERT INTO {table_ref} (amount, payload, bytes, location)
                VALUES (
                    123456789012345678901.123456789,
                    JSON_OBJECT('ok', TRUE, 'nested', JSON_ARRAY(1, 2)),
                    FROM_BASE64('AAEC/w=='),
                    ST_GeomFromText('POINT(1.5 -2.25)')
                )
                "#,
            ),
            1000,
        )
        .await
        .expect("execute MySQL insert");
        assert_eq!(insert.statement_type, DatabaseQueryStatementType::Insert);
        assert_eq!(insert.row_count, 1);
        assert!(insert.rows.is_empty());
        let insert_meta = insert
            .meta
            .as_ref()
            .and_then(Value::as_object)
            .expect("MySQL insert metadata");
        assert_eq!(
            insert_meta.get("affectedRows").and_then(Value::as_u64),
            Some(1)
        );
        assert!(
            insert_meta
                .get("insertId")
                .and_then(Value::as_u64)
                .is_some_and(|value| value > 0)
        );
        assert_eq!(
            insert_meta.get("warningStatus").and_then(Value::as_u64),
            Some(0)
        );

        let select = execute_mysql_query(
            &pool,
            &format!("SELECT id, amount, payload, bytes, location FROM {table_ref}"),
            1000,
        )
        .await
        .expect("execute MySQL select");
        assert_eq!(select.statement_type, DatabaseQueryStatementType::Select);
        assert_eq!(select.row_count, 1);
        assert_eq!(select.returned_row_count, 1);
        let row = select.rows.first().expect("returned MySQL row");
        assert_eq!(row.get("id").and_then(Value::as_u64), Some(1));
        assert_eq!(
            row.get("amount"),
            Some(&Value::String(
                "123456789012345678901.123456789".to_string(),
            )),
        );
        assert_eq!(
            row.get("payload"),
            Some(&serde_json::json!({"ok": true, "nested": [1, 2]}))
        );
        assert_eq!(
            row.get("bytes"),
            Some(&serde_json::json!({
                "type": "buffer",
                "encoding": "base64",
                "value": "AAEC/w==",
            })),
        );
        assert_eq!(
            row.get("location"),
            Some(&serde_json::json!({"x": 1.5, "y": -2.25}))
        );

        let empty = execute_mysql_query(
            &pool,
            &format!("SELECT amount, payload FROM {table_ref} WHERE FALSE"),
            1000,
        )
        .await
        .expect("execute empty MySQL select");
        assert_eq!(empty.row_count, 0);
        assert!(empty.rows.is_empty());
        assert_eq!(
            empty
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec!["amount", "payload"],
        );

        sqlx::raw_sql(&format!("DROP TEMPORARY TABLE {table_ref}"))
            .execute(&pool)
            .await
            .expect("drop MySQL parity table");
    }

    #[tokio::test]
    async fn mysql_and_mariadb_table_definition_uses_show_create_and_recreates() {
        let Some(connection) = mysql_test_connection() else {
            return;
        };
        let database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "iowb".to_string());
        let url = connection_url_for_database(&connection, Some(&database_name))
            .expect("build MySQL URL");
        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live MySQL");
        let parent_table_name = new_id("iowb_ddl_parent").replace('-', "_");
        let child_table_name = new_id("iowb_ddl_child").replace('-', "_");
        let unique_name = new_id("iowb_ddl_unique").replace('-', "_");
        let foreign_key_name = new_id("iowb_ddl_fk").replace('-', "_");
        let parent_ref = quote_mysql_identifier(&parent_table_name);
        let child_ref = quote_mysql_identifier(&child_table_name);
        let unique_ref = quote_mysql_identifier(&unique_name);
        let foreign_key_ref = quote_mysql_identifier(&foreign_key_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE TABLE {parent_ref} (
                id integer NOT NULL,
                PRIMARY KEY (id)
            ) ENGINE=InnoDB;
            CREATE TABLE {child_ref} (
                id bigint NOT NULL AUTO_INCREMENT,
                parent_id integer NOT NULL,
                label varchar(64) NOT NULL DEFAULT 'new',
                PRIMARY KEY (id),
                UNIQUE KEY {unique_ref} (label),
                CONSTRAINT {foreign_key_ref} FOREIGN KEY (parent_id)
                    REFERENCES {parent_ref} (id) ON DELETE RESTRICT
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create MySQL table-DDL fixture");

        let verification = async {
            let mut definition = None;
            for db_type in [SupportedDatabaseType::Mysql, SupportedDatabaseType::Mariadb] {
                let mut scoped_connection = connection.clone();
                scoped_connection.profile.db_type = db_type;
                let details = describe_live_object(
                    &scoped_connection,
                    DatabaseObjectType::Table,
                    Some(&database_name),
                    None,
                    &child_table_name,
                    None,
                    Some(&child_table_name),
                )
                .await?;
                let current_definition = details
                    .definition
                    .expect("engine-provided SHOW CREATE TABLE definition");
                assert!(database_definition_is_executable(
                    db_type,
                    DatabaseObjectType::Table,
                    Some(&current_definition),
                ));
                let normalized = current_definition.to_ascii_uppercase();
                assert!(normalized.starts_with("CREATE TABLE"));
                assert!(normalized.contains("AUTO_INCREMENT"));
                assert!(normalized.contains("FOREIGN KEY"));
                assert!(normalized.contains("ENGINE=INNODB"));
                definition = Some(current_definition);
            }

            // SHOW CREATE TABLE is the exact engine declaration. Execute it
            // unchanged after a drop to prove it is safe for SQL handoff.
            sqlx::raw_sql(&format!("DROP TABLE {child_ref}"))
                .execute(&pool)
                .await
                .map_err(sqlx_server_error)?;
            sqlx::raw_sql(definition.as_deref().expect("table definition"))
                .execute(&pool)
                .await
                .map_err(sqlx_server_error)?;
            let recreated = sqlx::query(
                r#"
                SELECT 1
                FROM information_schema.tables
                WHERE table_schema = ? AND table_name = ?
                "#,
            )
            .bind(&database_name)
            .bind(&child_table_name)
            .fetch_optional(&pool)
            .await
            .map_err(sqlx_server_error)?;
            assert!(recreated.is_some());
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!(
            "DROP TABLE IF EXISTS {child_ref}; DROP TABLE IF EXISTS {parent_ref}",
        ))
        .execute(&pool)
        .await
        .expect("drop MySQL table-DDL fixture");
        verification.expect("SHOW CREATE TABLE output should recreate the base table");
    }

    #[tokio::test]
    async fn mysql_and_mariadb_trigger_details_use_show_create_and_recreate() {
        let Some(connection) = mysql_test_connection() else {
            return;
        };
        let database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "iowb".to_string());
        let url = connection_url_for_database(&connection, Some(&database_name))
            .expect("build MySQL URL");
        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live MySQL");
        let table_name = new_id("iowb_trigger_ddl_table").replace('-', "_");
        let trigger_name = new_id("iowb_trigger_ddl").replace('-', "_");
        let table_ref = quote_mysql_identifier(&table_name);
        let trigger_ref = quote_mysql_identifier(&trigger_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE TABLE {table_ref} (
                id integer NOT NULL PRIMARY KEY,
                audit_count integer NOT NULL DEFAULT 0
            );
            CREATE TRIGGER {trigger_ref}
            BEFORE INSERT ON {table_ref}
            FOR EACH ROW SET NEW.audit_count = NEW.audit_count + 1
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create MySQL trigger-DDL fixture");

        let verification = async {
            let mut definition = None;
            for db_type in [SupportedDatabaseType::Mysql, SupportedDatabaseType::Mariadb] {
                let mut scoped_connection = connection.clone();
                scoped_connection.profile.db_type = db_type;
                let details = describe_live_object(
                    &scoped_connection,
                    DatabaseObjectType::Trigger,
                    Some(&database_name),
                    None,
                    &trigger_name,
                    Some(&table_name),
                    Some(&trigger_name),
                )
                .await?;
                assert_eq!(details.parent_name.as_deref(), Some(table_name.as_str()));
                let current_definition = details
                    .definition
                    .expect("engine-provided SHOW CREATE TRIGGER definition");
                let normalized = current_definition.to_ascii_uppercase();
                assert!(normalized.starts_with("CREATE "));
                assert!(normalized.contains("TRIGGER"));
                assert!(normalized.contains("BEFORE INSERT"));
                assert!(database_definition_is_executable(
                    db_type,
                    DatabaseObjectType::Trigger,
                    Some(&current_definition),
                ));
                assert!(details
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("action"))
                    .and_then(Value::as_str)
                    .is_some_and(|action| action.to_ascii_uppercase().contains("NEW.AUDIT_COUNT")));
                definition = Some(current_definition);
            }

            // SHOW CREATE TRIGGER returns a complete statement rather than
            // the action body. Recreate it unchanged to validate SQL-editor
            // handoff on both engines' shared adapter path.
            sqlx::raw_sql(&format!("DROP TRIGGER {trigger_ref}"))
                .execute(&pool)
                .await
                .map_err(sqlx_server_error)?;
            sqlx::raw_sql(definition.as_deref().expect("trigger definition"))
                .execute(&pool)
                .await
                .map_err(sqlx_server_error)?;
            let recreated = sqlx::query(
                r#"
                SELECT 1
                FROM information_schema.triggers
                WHERE trigger_schema = ? AND trigger_name = ?
                "#,
            )
            .bind(&database_name)
            .bind(&trigger_name)
            .fetch_optional(&pool)
            .await
            .map_err(sqlx_server_error)?;
            assert!(recreated.is_some());
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!(
            "DROP TRIGGER IF EXISTS {trigger_ref}; DROP TABLE IF EXISTS {table_ref}",
        ))
        .execute(&pool)
        .await
        .expect("drop MySQL trigger-DDL fixture");
        verification.expect("SHOW CREATE TRIGGER output should recreate the trigger");
    }

    #[tokio::test]
    async fn mysql_index_details_are_scoped_to_the_selected_parent_table() {
        let Some(connection) = mysql_test_connection() else {
            return;
        };
        let database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "iowb".to_string());
        let url = connection_url_for_database(&connection, Some(&database_name))
            .expect("build MySQL URL");
        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live MySQL");
        let first_table_name = new_id("iowb_index_first").replace('-', "_");
        let second_table_name = new_id("iowb_index_second").replace('-', "_");
        let index_name = new_id("iowb_shared_index").replace('-', "_");
        let first_table_ref = quote_mysql_identifier(&first_table_name);
        let second_table_ref = quote_mysql_identifier(&second_table_name);
        let index_ref = quote_mysql_identifier(&index_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE TABLE {first_table_ref} (
                id integer NOT NULL,
                first_value integer,
                KEY {index_ref} (first_value)
            );
            CREATE TABLE {second_table_ref} (
                id integer NOT NULL,
                second_value integer,
                KEY {index_ref} (second_value)
            );
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create MySQL index fixture");

        let verification = async {
            let index = live_objects(&connection, Some(&database_name), None)
                .await?
                .into_iter()
                .find(|object| {
                    object.object_type == DatabaseObjectType::Index
                        && object.name == index_name
                        && object.parent_name.as_deref() == Some(second_table_name.as_str())
                })
                .expect("second table index summary");
            let details = describe_live_object(
                &connection,
                DatabaseObjectType::Index,
                Some(&database_name),
                None,
                &index_name,
                Some(&second_table_name),
                index.identity.as_deref(),
            )
            .await?;
            assert_eq!(details.parent_name.as_deref(), Some(second_table_name.as_str()));
            assert_eq!(
                details
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("tableName"))
                    .and_then(Value::as_str),
                Some(second_table_name.as_str()),
            );
            assert_eq!(
                details
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("columns"))
                    .and_then(Value::as_array)
                    .and_then(|columns| columns.first())
                    .and_then(Value::as_str),
                Some("second_value"),
            );
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!(
            "DROP TABLE IF EXISTS {first_table_ref}; DROP TABLE IF EXISTS {second_table_ref}",
        ))
        .execute(&pool)
        .await
        .expect("drop MySQL index fixture");
        verification.expect("MySQL index details should use the selected parent table");
    }

    #[tokio::test]
    async fn mysql_constraint_details_include_columns_references_and_definition() {
        let Some(connection) = mysql_test_connection() else {
            return;
        };
        let database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "iowb".to_string());
        let url = connection_url_for_database(&connection, Some(&database_name))
            .expect("build MySQL URL");
        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live MySQL");
        let parent_table_name = new_id("iowb_constraint_parent").replace('-', "_");
        let child_table_name = new_id("iowb_constraint_child").replace('-', "_");
        let unique_name = new_id("iowb_constraint_unique").replace('-', "_");
        let foreign_key_name = new_id("iowb_constraint_fk").replace('-', "_");
        let parent_table_ref = quote_mysql_identifier(&parent_table_name);
        let child_table_ref = quote_mysql_identifier(&child_table_name);
        let unique_ref = quote_mysql_identifier(&unique_name);
        let foreign_key_ref = quote_mysql_identifier(&foreign_key_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE TABLE {parent_table_ref} (
                id integer NOT NULL,
                PRIMARY KEY (id)
            ) ENGINE=InnoDB;
            CREATE TABLE {child_table_ref} (
                id integer NOT NULL,
                parent_id integer NOT NULL,
                code varchar(64) NOT NULL,
                PRIMARY KEY (id),
                CONSTRAINT {unique_ref} UNIQUE (code),
                CONSTRAINT {foreign_key_ref} FOREIGN KEY (parent_id)
                    REFERENCES {parent_table_ref} (id)
                    ON UPDATE CASCADE
                    ON DELETE RESTRICT
            ) ENGINE=InnoDB;
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create MySQL constraint fixtures");

        let verification = async {
            let objects = live_objects(&connection, Some(&database_name), None).await?;
            let find_constraint = |name: &str| {
                objects
                    .iter()
                    .find(|object| {
                        object.object_type == DatabaseObjectType::Constraint
                            && object.name == name
                            && object.parent_name.as_deref() == Some(child_table_name.as_str())
                    })
                    .expect("constraint summary")
            };
            let primary_key = find_constraint("PRIMARY");
            let unique = find_constraint(&unique_name);
            let foreign_key = find_constraint(&foreign_key_name);

            let primary_key_details = describe_live_object(
                &connection,
                DatabaseObjectType::Constraint,
                Some(&database_name),
                None,
                "PRIMARY",
                Some(&child_table_name),
                primary_key.identity.as_deref(),
            )
            .await?;
            assert_eq!(
                primary_key_details
                    .columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>(),
                vec!["id"],
            );
            assert_eq!(
                primary_key_details
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("constraintType"))
                    .and_then(Value::as_str),
                Some("PRIMARY KEY"),
            );
            assert!(primary_key_details
                .definition
                .as_deref()
                .is_some_and(|definition| {
                    definition
                        .to_ascii_uppercase()
                        .contains("ALTER TABLE")
                        && definition.to_ascii_uppercase().contains("ADD PRIMARY KEY")
                }));

            let unique_details = describe_live_object(
                &connection,
                DatabaseObjectType::Constraint,
                Some(&database_name),
                None,
                &unique_name,
                Some(&child_table_name),
                unique.identity.as_deref(),
            )
            .await?;
            assert_eq!(
                unique_details
                    .columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>(),
                vec!["code"],
            );
            assert!(unique_details
                .definition
                .as_deref()
                .is_some_and(|definition| {
                    definition.to_ascii_uppercase().contains("ALTER TABLE")
                        && definition.to_ascii_uppercase().contains("UNIQUE")
                }));

            let foreign_key_details = describe_live_object(
                &connection,
                DatabaseObjectType::Constraint,
                Some(&database_name),
                None,
                &foreign_key_name,
                Some(&child_table_name),
                foreign_key.identity.as_deref(),
            )
            .await?;
            assert_eq!(
                foreign_key_details
                    .columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>(),
                vec!["parent_id"],
            );
            let foreign_keys = &foreign_key_details.foreign_keys;
            assert_eq!(foreign_keys.len(), 1);
            let foreign_key = foreign_keys.first().expect("foreign-key detail");
            assert_eq!(
                foreign_key.referenced_database_name.as_deref(),
                Some(database_name.as_str()),
            );
            assert_eq!(foreign_key.referenced_schema_name, None);
            assert_eq!(foreign_key.referenced_table_name, parent_table_name);
            assert_eq!(foreign_key.referenced_column_name, "id");
            assert_eq!(foreign_key.on_update.as_deref(), Some("CASCADE"));
            assert_eq!(foreign_key.on_delete.as_deref(), Some("RESTRICT"));
            let metadata = foreign_key_details.metadata.as_ref().expect("foreign-key metadata");
            assert_eq!(
                metadata
                    .get("referencedDatabaseName")
                    .and_then(Value::as_str),
                Some(database_name.as_str()),
            );
            assert_eq!(
                metadata.get("referencedSchemaName").and_then(Value::as_str),
                None,
            );
            assert_eq!(
                metadata.get("referencedTableName").and_then(Value::as_str),
                Some(parent_table_name.as_str()),
            );
            assert_eq!(metadata.get("onUpdate").and_then(Value::as_str), Some("CASCADE"));
            assert_eq!(metadata.get("onDelete").and_then(Value::as_str), Some("RESTRICT"));
            assert!(foreign_key_details
                .definition
                .as_deref()
                .is_some_and(|definition| {
                    let definition = definition.to_ascii_uppercase();
                    definition.contains("ALTER TABLE")
                        && definition.contains("FOREIGN KEY")
                        && definition.contains("ON UPDATE CASCADE")
                        && definition.contains("ON DELETE RESTRICT")
                }));
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!(
            "DROP TABLE IF EXISTS {child_table_ref}; DROP TABLE IF EXISTS {parent_table_ref}",
        ))
        .execute(&pool)
        .await
        .expect("drop MySQL constraint fixtures");
        verification.expect("MySQL constraint details should include usable metadata and DDL");
    }

    #[tokio::test]
    async fn mysql_relational_schema_uses_external_stub_for_cross_database_foreign_key() {
        let Some(mut connection) = mysql_test_connection() else {
            return;
        };
        let admin_database_name = connection
            .profile
            .database_name
            .clone()
            .unwrap_or_else(|| "iowb".to_string());
        let url = connection_url_for_database(&connection, Some(&admin_database_name))
            .expect("build MySQL URL");
        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live MySQL");
        let database_name = new_id("iowb_fk_source_database").replace('-', "_");
        let target_database_name = new_id("iowb_fk_target_database").replace('-', "_");
        let shared_table_name = new_id("iowb_fk_same_table").replace('-', "_");
        let child_table_name = new_id("iowb_fk_child").replace('-', "_");
        let foreign_key_name = new_id("iowb_fk_constraint").replace('-', "_");
        let database_ref = quote_mysql_identifier(&database_name);
        let target_database_ref = quote_mysql_identifier(&target_database_name);
        let shared_table_ref = quote_mysql_identifier(&shared_table_name);
        let child_table_ref = quote_mysql_identifier(&child_table_name);
        let foreign_key_ref = quote_mysql_identifier(&foreign_key_name);
        connection.profile.database_name = Some(database_name.clone());
        sqlx::raw_sql(&format!(
            r#"
            CREATE DATABASE {database_ref};
            CREATE DATABASE {target_database_ref};
            CREATE TABLE {target_database_ref}.{shared_table_ref} (
                id integer NOT NULL,
                PRIMARY KEY (id)
            ) ENGINE=InnoDB;
            CREATE TABLE {database_ref}.{shared_table_ref} (
                id integer NOT NULL,
                PRIMARY KEY (id)
            ) ENGINE=InnoDB;
            CREATE TABLE {database_ref}.{child_table_ref} (
                parent_id integer NOT NULL,
                CONSTRAINT {foreign_key_ref} FOREIGN KEY (parent_id)
                    REFERENCES {target_database_ref}.{shared_table_ref} (id)
            ) ENGINE=InnoDB;
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create cross-database MySQL fixtures");

        let verification = async {
            let live_pool = live_pool_for_database(&connection, Some(&database_name)).await?;
            let foreign_keys = live_foreign_keys(
                &live_pool,
                SupportedDatabaseType::Mysql,
                &database_name,
                &child_table_name,
            )
            .await?;
            let foreign_key = foreign_keys.first().expect("cross-database foreign key");
            assert_eq!(
                foreign_key.referenced_database_name.as_deref(),
                Some(target_database_name.as_str()),
            );
            assert_eq!(foreign_key.referenced_schema_name, None);
            assert_eq!(foreign_key.referenced_table_name, shared_table_name);

            let objects = live_objects(&connection, Some(&database_name), None).await?;
            let graph = live_relational_schema(
                &connection,
                DatabaseObjectType::Database,
                &database_name,
                None,
                &objects,
            )
            .await?;
            let relationship = graph
                .relationships
                .iter()
                .find(|relationship| relationship.source_table_name == child_table_name)
                .expect("cross-database relationship");
            assert_eq!(
                relationship.target_database_name.as_deref(),
                Some(target_database_name.as_str()),
            );
            assert_eq!(relationship.target_schema_name, None);
            assert_eq!(relationship.target_table_name, shared_table_name);
            let tables_with_shared_name = graph
                .tables
                .iter()
                .filter(|table| table.name == shared_table_name)
                .collect::<Vec<_>>();
            assert_eq!(tables_with_shared_name.len(), 2);
            assert!(tables_with_shared_name.iter().any(|table| {
                !table.is_external && table.database_name.as_deref() == Some(database_name.as_str())
            }));
            let external_target = tables_with_shared_name
                .iter()
                .find(|table| {
                    table.is_external
                        && table.database_name.as_deref() == Some(target_database_name.as_str())
                        && table.schema_name.is_none()
                })
                .expect("fully-qualified external target stub");
            assert_eq!(
                external_target
                    .columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>(),
                vec!["id"],
            );
            Ok::<(), ServerError>(())
        }
        .await;

        sqlx::raw_sql(&format!(
            "DROP DATABASE IF EXISTS {database_ref}; \
             DROP DATABASE IF EXISTS {target_database_ref}",
        ))
        .execute(&pool)
        .await
        .expect("drop cross-database MySQL fixtures");
        verification.expect("MySQL graph should retain cross-database target identity");
    }

    #[tokio::test]
    async fn sqlite_row_crud_enforces_optimistic_concurrency() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("database-test")));
        let sqlite = Connection::open(&path).expect("open test database");
        sqlite
            .execute_batch(
                r#"
                CREATE TABLE items (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    note TEXT
                );
                "#,
            )
            .expect("create test table");
        drop(sqlite);

        let connection = sqlite_test_connection(&path);
        let metadata = database_table_metadata(&connection, None, None, "items")
            .await
            .expect("load metadata");
        let inserted = insert_database_row(
            &connection,
            None,
            None,
            "items",
            &metadata,
            &serde_json::json!({"id": 1, "name": "Alpha", "note": null})
                .as_object()
                .expect("insert row object")
                .clone(),
        )
        .await
        .expect("insert row")
        .expect("inserted row result");
        assert_eq!(
            inserted.get("name"),
            Some(&Value::String("Alpha".to_string()))
        );

        let scope = DatabaseTableScopeRequest {
            database_name: None,
            schema_name: None,
            table_name: "items".to_string(),
        };
        let primary_key = serde_json::json!({"id": 1})
            .as_object()
            .expect("primary key object")
            .clone();
        let updated = update_database_row(
            &connection,
            &scope,
            &primary_key,
            serde_json::json!({"name": "Beta"})
                .as_object()
                .expect("update object"),
            &inserted,
        )
        .await
        .expect("update row")
        .expect("updated row result");
        assert_eq!(
            updated.get("name"),
            Some(&Value::String("Beta".to_string()))
        );

        let stale_error = update_database_row(
            &connection,
            &scope,
            &primary_key,
            serde_json::json!({"name": "Gamma"})
                .as_object()
                .expect("stale update object"),
            &inserted,
        )
        .await
        .expect_err("stale update must fail");
        assert_eq!(stale_error.status, StatusCode::CONFLICT);

        delete_database_row(&connection, &scope, &primary_key, &updated)
            .await
            .expect("delete row");
        let page =
            read_sqlite_table_data(&connection, "items", 50, 0, true).expect("read empty table");
        assert_eq!(page.row_count, 0);
        assert_eq!(page.total_row_count, Some(0));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn sqlite_generated_columns_are_visible_and_default_only_inserts_work() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("generated-columns")));
        let sqlite = Connection::open(&path).expect("open generated-column database");
        sqlite
            .execute_batch(
                r#"
                CREATE TABLE generated_items (
                    id INTEGER PRIMARY KEY,
                    value INTEGER NOT NULL,
                    doubled INTEGER GENERATED ALWAYS AS (value * 2) STORED,
                    nullable_default TEXT DEFAULT 'server default'
                );
                CREATE TABLE only_id (id INTEGER PRIMARY KEY);
                "#,
            )
            .expect("create generated-column fixture");
        drop(sqlite);

        let connection = sqlite_test_connection(&path);
        let generated_metadata = database_table_metadata(&connection, None, None, "generated_items")
            .await
            .expect("load generated table metadata");
        let id = generated_metadata
            .columns
            .iter()
            .find(|column| column.name == "id")
            .expect("rowid primary key");
        assert_eq!(id.extra.as_deref(), Some("auto_increment"));
        let doubled = generated_metadata
            .columns
            .iter()
            .find(|column| column.name == "doubled")
            .expect("generated column");
        assert_eq!(doubled.extra.as_deref(), Some("generated stored"));

        let generated_insert_error = insert_database_row(
            &connection,
            None,
            None,
            "generated_items",
            &generated_metadata,
            &serde_json::json!({"value": 4, "doubled": 99})
                .as_object()
                .expect("generated insert values")
                .clone(),
        )
        .await
        .expect_err("generated columns must not accept explicit values");
        assert_eq!(generated_insert_error.status, StatusCode::BAD_REQUEST);

        insert_database_row(
            &connection,
            None,
            None,
            "generated_items",
            &generated_metadata,
            &serde_json::json!({"value": 4})
                .as_object()
                .expect("insert values")
                .clone(),
        )
        .await
        .expect("insert generated row");

        insert_database_row(
            &connection,
            None,
            None,
            "generated_items",
            &generated_metadata,
            &serde_json::json!({"value": 5, "nullable_default": null})
                .as_object()
                .expect("explicit null values")
                .clone(),
        )
        .await
        .expect("insert explicit null over default");

        let generated_data = read_sqlite_table_data(&connection, "generated_items", 50, 0, true)
            .expect("read generated table data");
        assert_eq!(
            generated_data
                .columns
                .iter()
                .find(|column| column.name == "doubled")
                .and_then(|column| column.extra.as_deref()),
            Some("generated stored"),
        );
        assert_eq!(generated_data.rows.len(), 2);
        let inserted = generated_data
            .rows
            .iter()
            .find(|row| row.get("value") == Some(&serde_json::json!(4)))
            .expect("generated row");
        assert_eq!(inserted.get("doubled"), Some(&serde_json::json!(8)));
        let explicit_null = generated_data
            .rows
            .iter()
            .find(|row| row.get("value") == Some(&serde_json::json!(5)))
            .expect("explicit-null row");
        assert_eq!(explicit_null.get("nullable_default"), Some(&Value::Null));

        let generated_update_error = update_database_row(
            &connection,
            &DatabaseTableScopeRequest {
                database_name: None,
                schema_name: None,
                table_name: "generated_items".to_string(),
            },
            &serde_json::json!({"id": 1})
                .as_object()
                .expect("primary key")
                .clone(),
            &serde_json::json!({"doubled": 99})
                .as_object()
                .expect("generated update values")
                .clone(),
            inserted,
        )
        .await
        .expect_err("generated columns must not be writable");
        assert_eq!(generated_update_error.status, StatusCode::BAD_REQUEST);

        let only_id_metadata = database_table_metadata(&connection, None, None, "only_id")
            .await
            .expect("load default-only table metadata");
        let default_insert = insert_database_row(
            &connection,
            None,
            None,
            "only_id",
            &only_id_metadata,
            &Map::new(),
        )
        .await
        .expect("default-only insert");
        assert!(default_insert.is_none());
        let only_id_data = read_sqlite_table_data(&connection, "only_id", 50, 0, true)
            .expect("read default-only table data");
        assert_eq!(only_id_data.row_count, 1);
        assert_eq!(only_id_data.rows.len(), 1);
        assert_eq!(only_id_data.rows[0].get("id"), Some(&serde_json::json!(1)));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn default_row_inserts_use_each_engine_syntax() {
        assert_eq!(
            default_row_insert_sql(SupportedDatabaseType::Postgresql, "\"public\".\"only_id\""),
            "INSERT INTO \"public\".\"only_id\" DEFAULT VALUES",
        );
        assert_eq!(
            default_row_insert_sql(SupportedDatabaseType::Sqlite, "\"only_id\""),
            "INSERT INTO \"only_id\" DEFAULT VALUES",
        );
        assert_eq!(
            default_row_insert_sql(SupportedDatabaseType::Mysql, "`app`.`only_id`"),
            "INSERT INTO `app`.`only_id` () VALUES ()",
        );
        assert_eq!(
            default_row_insert_sql(SupportedDatabaseType::Mariadb, "`app`.`only_id`"),
            "INSERT INTO `app`.`only_id` () VALUES ()",
        );
    }

    #[test]
    fn portable_payload_and_structured_column_mapping_match_web_ai_cli() {
        let payload = serde_json::json!({
            "format": "web-ai-cli/database-portable-v1",
            "type": "table-schema-and-data",
            "table": {
                "name": "people",
                "columns": [
                    {
                        "name": "user_id",
                        "dataType": "integer",
                        "nativeType": "INTEGER",
                        "nullable": false,
                        "isPrimaryKey": true
                    },
                    {
                        "name": "display_name",
                        "dataType": "text",
                        "nativeType": "TEXT",
                        "nullable": true
                    }
                ]
            },
            "rows": [{"user_id": 1, "display_name": "Ada"}]
        });

        let portable = parse_portable_table_payload(&payload).expect("parse portable payload");
        assert_eq!(portable.table_name, "people");
        assert!(portable.include_data);
        assert_eq!(portable.columns.len(), 2);
        assert_eq!(portable.rows.len(), 1);

        let mappings = build_column_mappings(
            &["User ID".to_string(), "DISPLAY-NAME".to_string()],
            &portable.columns,
        );
        assert_eq!(
            mappings,
            vec![
                ("user_id".to_string(), "User ID".to_string()),
                ("display_name".to_string(), "DISPLAY-NAME".to_string()),
            ],
        );
    }

    #[test]
    fn complex_value_normalizers_match_web_driver_shapes() {
        assert_eq!(
            parse_postgres_array("{1,2,NULL}", "INT8"),
            serde_json::json!(["1", "2", null]),
        );
        assert_eq!(
            parse_postgres_array("{{1.25,2.5},{3.75,4}}", "NUMERIC"),
            serde_json::json!([[1.25, 2.5], [3.75, 4.0]]),
        );
        assert_eq!(
            parse_postgres_array("{\"{\\\"ok\\\":true}\",NULL}", "JSONB"),
            serde_json::json!([{"ok": true}, null]),
        );
        assert_eq!(
            parse_postgres_point("(1.5,-2.25)"),
            Some(serde_json::json!({"x": 1.5, "y": -2.25})),
        );
        assert_eq!(
            parse_postgres_circle("<(1,2),3.5>"),
            Some(serde_json::json!({"x": 1.0, "y": 2.0, "radius": 3.5})),
        );
        assert_eq!(
            parse_postgres_interval("1 year 2 mons 3 days 04:05:06.789"),
            serde_json::json!({
                "years": 1,
                "months": 2,
                "days": 3,
                "hours": 4,
                "minutes": 5,
                "seconds": 6,
                "milliseconds": 789,
            }),
        );

        let mut geometry = vec![0, 0, 0, 0, 1];
        geometry.extend_from_slice(&1u32.to_le_bytes());
        geometry.extend_from_slice(&1.5f64.to_le_bytes());
        geometry.extend_from_slice(&(-2.25f64).to_le_bytes());
        assert_eq!(
            parse_mysql_geometry(&geometry),
            Some(serde_json::json!({"x": 1.5, "y": -2.25})),
        );
    }

    #[test]
    fn database_text_literals_preserve_quotes_backslashes_and_unicode() {
        let value = "quote ' slash \\ newline\nemoji 🧪";
        let postgres = database_text_literal(SupportedDatabaseType::Postgresql, value);
        assert!(postgres.starts_with("$iowb$"));
        assert!(postgres.ends_with("$iowb$"));
        assert!(postgres.contains(value));
        assert_eq!(
            database_text_literal(SupportedDatabaseType::Mysql, value),
            "CONVERT(X'71756f7465202720736c617368205c206e65776c696e650a656d6f6a6920f09fa7aa' USING utf8mb4)",
        );
        assert_eq!(
            database_text_literal(SupportedDatabaseType::Sqlite, value),
            "CAST(X'71756f7465202720736c617368205c206e65776c696e650a656d6f6a6920f09fa7aa' AS TEXT)",
        );
    }

    #[test]
    fn capabilities_match_web_adapter_contract() {
        let postgres =
            serde_json::to_value(database_capabilities(SupportedDatabaseType::Postgresql))
                .expect("serialize postgres capabilities");
        assert_eq!(postgres["supportsDatabases"], Value::Bool(true));
        assert_eq!(postgres["supportsSchemas"], Value::Bool(true));
        assert_eq!(postgres["supportsViews"], Value::Bool(true));
        assert_eq!(postgres["supportsIndexes"], Value::Bool(true));
        assert_eq!(postgres["supportsForeignKeys"], Value::Bool(true));
        assert_eq!(postgres["supportsParameterizedQueries"], Value::Bool(false));
        assert_eq!(postgres["supportsOffset"], Value::Bool(true));
        assert_eq!(
            postgres["supportedObjectTypes"],
            serde_json::json!([
                "table",
                "view",
                "materialized-view",
                "index",
                "trigger",
                "routine",
                "sequence",
                "constraint",
                "policy"
            ]),
        );

        let sqlite = database_capabilities(SupportedDatabaseType::Sqlite);
        assert!(!sqlite.supports_databases);
        assert!(!sqlite.supports_schemas);
        assert!(sqlite.supports_views);
        assert!(sqlite.supports_indexes);
        assert!(sqlite.supports_triggers);
        assert!(sqlite.supports_constraints);
        assert!(!sqlite.supports_parameterized_queries);
    }

    #[test]
    fn executable_definition_flag_rejects_catalog_source_fragments() {
        assert!(database_definition_is_executable(
            SupportedDatabaseType::Sqlite,
            DatabaseObjectType::Table,
            Some("CREATE TABLE items (id INTEGER PRIMARY KEY)"),
        ));
        assert!(database_definition_is_executable(
            SupportedDatabaseType::Postgresql,
            DatabaseObjectType::Table,
            Some("CREATE TABLE public.items (id integer PRIMARY KEY)"),
        ));
        assert!(database_definition_is_executable(
            SupportedDatabaseType::Mysql,
            DatabaseObjectType::Table,
            Some("CREATE TABLE `items` (`id` integer PRIMARY KEY)"),
        ));
        assert!(database_definition_is_executable(
            SupportedDatabaseType::Mariadb,
            DatabaseObjectType::Table,
            Some("CREATE TABLE `items` (`id` integer PRIMARY KEY)"),
        ));
        assert!(database_definition_is_executable(
            SupportedDatabaseType::Postgresql,
            DatabaseObjectType::Index,
            Some("CREATE INDEX items_name_idx ON public.items (name)"),
        ));
        assert!(database_definition_is_executable(
            SupportedDatabaseType::Postgresql,
            DatabaseObjectType::Trigger,
            Some("CREATE TRIGGER audit BEFORE INSERT ON items EXECUTE FUNCTION audit()"),
        ));
        assert!(database_definition_is_executable(
            SupportedDatabaseType::Postgresql,
            DatabaseObjectType::Policy,
            Some("CREATE POLICY tenant_read ON public.items FOR SELECT USING (tenant_id > 0)"),
        ));
        assert!(database_definition_is_executable(
            SupportedDatabaseType::Mysql,
            DatabaseObjectType::Trigger,
            Some("CREATE DEFINER=`app`@`%` TRIGGER `audit` BEFORE INSERT ON `items` FOR EACH ROW SET NEW.created_at = NOW()"),
        ));
        assert!(database_definition_is_executable(
            SupportedDatabaseType::Mariadb,
            DatabaseObjectType::Trigger,
            Some("CREATE DEFINER=`app`@`%` TRIGGER `audit` BEFORE INSERT ON `items` FOR EACH ROW SET NEW.created_at = NOW()"),
        ));

        // These are catalog source snippets, not statements that can be
        // recreated by pasting them into the query editor.
        assert!(!database_definition_is_executable(
            SupportedDatabaseType::Postgresql,
            DatabaseObjectType::View,
            Some("SELECT id, name FROM items"),
        ));
        assert!(!database_definition_is_executable(
            SupportedDatabaseType::Postgresql,
            DatabaseObjectType::Constraint,
            Some("FOREIGN KEY (owner_id) REFERENCES owners(id)"),
        ));
        assert!(!database_definition_is_executable(
            SupportedDatabaseType::Postgresql,
            DatabaseObjectType::Sequence,
            Some("bigint sequence"),
        ));
        assert!(!database_definition_is_executable(
            SupportedDatabaseType::Mariadb,
            DatabaseObjectType::Routine,
            Some("BEGIN SELECT 1; END"),
        ));
        assert!(!database_definition_is_executable(
            SupportedDatabaseType::Mysql,
            DatabaseObjectType::Trigger,
            Some("SET NEW.created_at = NOW()"),
        ));
        assert!(!database_definition_is_executable(
            SupportedDatabaseType::Mariadb,
            DatabaseObjectType::Trigger,
            Some("CREATE TEMPORARY TABLE trigger_audit (id integer)"),
        ));
        assert!(!database_definition_is_executable(
            SupportedDatabaseType::Mysql,
            DatabaseObjectType::Event,
            Some("INSERT INTO events_log VALUES (NOW())"),
        ));
    }

    #[test]
    fn sqlite_explorer_discovers_objects_and_returns_definitions() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("database-explorer")));
        let sqlite = Connection::open(&path).expect("open explorer database");
        sqlite
            .execute_batch(
                r#"
                PRAGMA foreign_keys = ON;
                CREATE TABLE parents (id INTEGER PRIMARY KEY);
                CREATE TABLE items (
                    id INTEGER PRIMARY KEY,
                    parent_id INTEGER,
                    code TEXT UNIQUE,
                    FOREIGN KEY (parent_id) REFERENCES parents(id)
                );
                CREATE INDEX idx_items_parent ON items(parent_id);
                CREATE VIEW item_view AS SELECT id, code FROM items;
                CREATE TRIGGER items_after_insert
                AFTER INSERT ON items
                BEGIN
                    UPDATE parents SET id = id WHERE id = NEW.parent_id;
                END;
                "#,
            )
            .expect("create explorer objects");

        let objects = list_sqlite_objects(&sqlite).expect("list sqlite objects");
        assert!(objects.iter().any(|object| {
            object.name == "items" && object.object_type == DatabaseObjectType::Table
        }));
        assert!(objects.iter().any(|object| {
            object.name == "item_view" && object.object_type == DatabaseObjectType::View
        }));
        assert!(objects.iter().any(|object| {
            object.name == "idx_items_parent" && object.object_type == DatabaseObjectType::Index
        }));
        assert!(objects.iter().any(|object| {
            object.name == "items_after_insert"
                && object.object_type == DatabaseObjectType::Trigger
                && object.parent_name.as_deref() == Some("items")
        }));
        assert!(objects.iter().any(|object| {
            object.object_type == DatabaseObjectType::Constraint
                && object.parent_name.as_deref() == Some("items")
        }));

        let connection = sqlite_test_connection(&path);
        let table_details = describe_sqlite_object(
            &connection,
            DatabaseObjectType::Table,
            "items",
            None,
            Some("items"),
        )
        .expect("table details");
        let table_metadata = table_details.metadata.as_ref().expect("table metadata");
        assert_eq!(
            table_metadata.get("objectType").and_then(Value::as_str),
            Some("table")
        );
        assert_eq!(
            table_metadata.get("withoutRowid").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            table_metadata.get("rowidPrimaryKey").and_then(Value::as_bool),
            Some(true)
        );
        let index = objects
            .iter()
            .find(|object| {
                object.name == "idx_items_parent" && object.object_type == DatabaseObjectType::Index
            })
            .expect("index summary");
        let index_details = describe_sqlite_object(
            &connection,
            index.object_type,
            &index.name,
            index.parent_name.as_deref(),
            index.identity.as_deref(),
        )
        .expect("index details");
        assert!(index_details
            .definition
            .as_deref()
            .is_some_and(|definition| definition.contains("CREATE INDEX")));
        assert_eq!(
            index_details
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("columns"))
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1),
        );

        let trigger = objects
            .iter()
            .find(|object| object.object_type == DatabaseObjectType::Trigger)
            .expect("trigger summary");
        let trigger_details = describe_sqlite_object(
            &connection,
            trigger.object_type,
            &trigger.name,
            trigger.parent_name.as_deref(),
            trigger.identity.as_deref(),
        )
        .expect("trigger details");
        assert!(trigger_details.definition.is_some());
        assert_eq!(trigger_details.parent_name.as_deref(), Some("items"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn sqlite_table_description_accepts_implicit_foreign_key_target_column() {
        let sqlite = Connection::open_in_memory().expect("open SQLite fixture");
        sqlite
            .execute_batch(
                r#"
                PRAGMA foreign_keys = ON;
                CREATE TABLE parents (id INTEGER PRIMARY KEY);
                CREATE TABLE children (
                    id INTEGER PRIMARY KEY,
                    parent_id INTEGER REFERENCES parents
                );
                "#,
            )
            .expect("create implicit foreign-key fixture");

        let table = describe_sqlite_table_inner(&sqlite, "children", DatabaseObjectType::Table)
            .expect("describe table with implicit foreign key");
        assert_eq!(table.foreign_keys.len(), 1);
        let foreign_key = table.foreign_keys.first().expect("foreign key");
        assert_eq!(foreign_key.column_name, "parent_id");
        assert_eq!(foreign_key.referenced_table_name, "parents");
        assert_eq!(foreign_key.referenced_column_name, "");
    }

    #[test]
    fn sqlite_constraint_details_are_exact_for_each_catalog_entry() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("sqlite-constraints")));
        let sqlite = Connection::open(&path).expect("open constraint database");
        sqlite
            .execute_batch(
                r#"
                PRAGMA foreign_keys = ON;
                CREATE TABLE parents (
                    id INTEGER PRIMARY KEY,
                    alt_id INTEGER UNIQUE
                );
                CREATE TABLE items (
                    id INTEGER PRIMARY KEY,
                    parent_id INTEGER,
                    alternate_parent_id INTEGER,
                    code TEXT,
                    amount INTEGER,
                    UNIQUE(code, amount),
                    CHECK(amount > 0),
                    FOREIGN KEY(parent_id)
                        REFERENCES parents(id)
                        ON UPDATE CASCADE
                        ON DELETE SET NULL,
                    FOREIGN KEY(alternate_parent_id)
                        REFERENCES parents(alt_id)
                        ON UPDATE RESTRICT
                        ON DELETE CASCADE
                );
                "#,
            )
            .expect("create constraint fixture");

        let objects = list_sqlite_objects(&sqlite).expect("list SQLite objects");
        let constraints = objects
            .iter()
            .filter(|object| {
                object.object_type == DatabaseObjectType::Constraint
                    && object.parent_name.as_deref() == Some("items")
            })
            .collect::<Vec<_>>();
        let foreign_keys = constraints
            .iter()
            .filter(|object| {
                object
                    .identity
                    .as_deref()
                    .is_some_and(|identity| identity.contains(":foreign-key:"))
            })
            .collect::<Vec<_>>();
        assert_eq!(foreign_keys.len(), 2);
        assert_ne!(foreign_keys[0].identity, foreign_keys[1].identity);
        let unique = constraints
            .iter()
            .find(|object| {
                object
                    .identity
                    .as_deref()
                    .is_some_and(|identity| identity.contains(":unique:"))
                    && object.name == "UNIQUE (code, amount)"
            })
            .expect("composite unique constraint");
        let check = constraints
            .iter()
            .find(|object| {
                object
                    .identity
                    .as_deref()
                    .is_some_and(|identity| identity.contains(":check:"))
            })
            .expect("check constraint");

        let connection = sqlite_test_connection(&path);
        for foreign_key in foreign_keys {
            let details = describe_sqlite_object(
                &connection,
                DatabaseObjectType::Constraint,
                &foreign_key.name,
                foreign_key.parent_name.as_deref(),
                foreign_key.identity.as_deref(),
            )
            .expect("foreign-key details");
            assert_eq!(details.foreign_keys.len(), 1);
            let key = details.foreign_keys.first().expect("foreign key");
            assert_eq!(
                details
                    .columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>(),
                vec![key.column_name.as_str()],
            );
            let definition = details.definition.as_deref().expect("foreign-key DDL");
            match key.column_name.as_str() {
                "parent_id" => {
                    assert_eq!(key.referenced_column_name, "id");
                    assert_eq!(key.on_update.as_deref(), Some("CASCADE"));
                    assert_eq!(key.on_delete.as_deref(), Some("SET NULL"));
                    assert!(definition.contains("ON UPDATE CASCADE"));
                    assert!(definition.contains("ON DELETE SET NULL"));
                }
                "alternate_parent_id" => {
                    assert_eq!(key.referenced_column_name, "alt_id");
                    assert_eq!(key.on_update.as_deref(), Some("RESTRICT"));
                    assert_eq!(key.on_delete.as_deref(), Some("CASCADE"));
                    assert!(definition.contains("ON UPDATE RESTRICT"));
                    assert!(definition.contains("ON DELETE CASCADE"));
                }
                other => panic!("unexpected foreign-key column {other}"),
            }
        }

        let unique_details = describe_sqlite_object(
            &connection,
            DatabaseObjectType::Constraint,
            &unique.name,
            unique.parent_name.as_deref(),
            unique.identity.as_deref(),
        )
        .expect("unique constraint details");
        assert_eq!(
            unique_details
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec!["code", "amount"],
        );
        assert!(unique_details
            .definition
            .as_deref()
            .is_some_and(|definition| definition.contains("UNIQUE")));

        let check_details = describe_sqlite_object(
            &connection,
            DatabaseObjectType::Constraint,
            &check.name,
            check.parent_name.as_deref(),
            check.identity.as_deref(),
        )
        .expect("check constraint details");
        assert!(check_details
            .definition
            .as_deref()
            .is_some_and(|definition| definition.contains("CHECK") && definition.contains("amount > 0")));
        assert_eq!(
            check_details
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("expression"))
                .and_then(Value::as_str),
            Some("amount > 0"),
        );

        let stale = describe_sqlite_object(
            &connection,
            DatabaseObjectType::Constraint,
            "FOREIGN KEY #9",
            Some("items"),
            Some("items:foreign-key:999"),
        );
        assert!(stale.is_err());

        drop(sqlite);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn sqlite_check_constraint_parser_ignores_sql_literals_and_comments() {
        let checks = sqlite_check_constraints_from_create_sql(
            r#"
            CREATE TABLE "checks (archive)" (
                payload TEXT DEFAULT 'CHECK(fake, value)',
                amount INTEGER CONSTRAINT positive_amount CHECK (
                    amount > 0 AND payload <> 'CHECK(ignored, value)'
                ),
                -- CHECK(commented_out)
                CONSTRAINT [payload_check] CHECK (payload <> 'one,two')
                /* CHECK(block_comment) */
            )
            "#,
        );

        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].name, "CONSTRAINT positive_amount");
        assert!(checks[0].definition.contains("CHECK"));
        assert!(checks[0].expression.contains("amount > 0"));
        assert_eq!(checks[1].name, "CONSTRAINT [payload_check]");
        assert_eq!(checks[1].expression, "payload <> 'one,two'");
    }

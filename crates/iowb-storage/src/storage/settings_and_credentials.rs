impl Storage {
    pub fn set_setting(&self, key: &str, value: &Value) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO settings (key, value, updated_at)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(key) DO UPDATE SET
                    value = excluded.value,
                    updated_at = excluded.updated_at
                "#,
                params![key, serde_json::to_string(value)?, now],
            )?;
            Ok(())
        })
    }

    pub fn list_settings(&self) -> Result<Vec<SettingEntry>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT key, value, updated_at
                FROM settings
                ORDER BY key ASC
                "#,
            )?;

            let rows = stmt.query_map([], |row| {
                let value_raw: String = row.get(1)?;
                let value = serde_json::from_str::<Value>(&value_raw).unwrap_or(Value::Null);
                Ok(SettingEntry {
                    key: row.get(0)?,
                    value,
                    updated_at: parse_time_sql(row.get::<_, String>(2)?)?,
                })
            })?;

            let mut settings = Vec::new();
            for row in rows {
                settings.push(row?);
            }
            Ok(settings)
        })
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<Value>> {
        self.with_connection(|conn| {
            let value = conn
                .query_row(
                    "SELECT value FROM settings WHERE key = ?1",
                    params![key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;

            value
                .map(|raw| serde_json::from_str(&raw))
                .transpose()
                .map_err(StorageError::from)
        })
    }

    pub fn list_api_keys(&self, user_id: &str) -> Result<Vec<ApiKeyRecord>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT id, key_name, key_prefix, is_active, created_at, updated_at
                FROM api_keys
                WHERE user_id = ?1
                ORDER BY created_at DESC
                "#,
            )?;
            let rows = stmt.query_map(params![user_id], map_api_key_row)?;
            let mut keys = Vec::new();
            for row in rows {
                keys.push(row?);
            }
            Ok(keys)
        })
    }

    pub fn create_api_key(
        &self,
        user_id: &str,
        key_name: &str,
        key_hash: &str,
        key_prefix: &str,
    ) -> Result<ApiKeyRecord> {
        let now = Utc::now();
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO api_keys (user_id, key_name, key_hash, key_prefix, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    user_id,
                    key_name,
                    key_hash,
                    key_prefix,
                    now.to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )?;
            Ok(ApiKeyRecord {
                id: conn.last_insert_rowid(),
                key_name: key_name.to_string(),
                masked_key: mask_secret(key_prefix),
                key_prefix: key_prefix.to_string(),
                is_active: true,
                created_at: now,
                updated_at: now,
            })
        })
    }

    pub fn delete_api_key(&self, user_id: &str, key_id: i64) -> Result<bool> {
        self.with_connection(|conn| {
            let changed = conn.execute(
                "DELETE FROM api_keys WHERE user_id = ?1 AND id = ?2",
                params![user_id, key_id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn toggle_api_key(&self, user_id: &str, key_id: i64, is_active: bool) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                r#"
                UPDATE api_keys
                SET is_active = ?1, updated_at = ?2
                WHERE user_id = ?3 AND id = ?4
                "#,
                params![if is_active { 1 } else { 0 }, now, user_id, key_id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn list_credentials(
        &self,
        user_id: &str,
        credential_type: Option<&str>,
    ) -> Result<Vec<CredentialRecord>> {
        self.with_connection(|conn| {
            let mut credentials = Vec::new();
            if let Some(credential_type) = credential_type {
                let mut stmt = conn.prepare(
                    r#"
                    SELECT id, credential_name, credential_type, description, is_active, created_at, updated_at
                    FROM credentials
                    WHERE user_id = ?1 AND credential_type = ?2
                    ORDER BY created_at DESC
                    "#,
                )?;
                let rows = stmt.query_map(params![user_id, credential_type], map_credential_row)?;
                for row in rows {
                    credentials.push(row?);
                }
                return Ok(credentials);
            }

            let mut stmt = conn.prepare(
                r#"
                SELECT id, credential_name, credential_type, description, is_active, created_at, updated_at
                FROM credentials
                WHERE user_id = ?1
                ORDER BY created_at DESC
                "#,
            )?;
            let rows = stmt.query_map(params![user_id], map_credential_row)?;
            for row in rows {
                credentials.push(row?);
            }
            Ok(credentials)
        })
    }

    pub fn get_active_credential_value(
        &self,
        user_id: &str,
        credential_id: i64,
        credential_type: &str,
    ) -> Result<Option<String>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT credential_value
                FROM credentials
                WHERE user_id = ?1
                  AND id = ?2
                  AND credential_type = ?3
                  AND is_active = 1
                "#,
                params![user_id, credential_id, credential_type],
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn get_active_credential_value_by_name(
        &self,
        user_id: &str,
        credential_name: &str,
        credential_type: &str,
    ) -> Result<Option<String>> {
        self.with_connection(|conn| {
            conn.query_row(
                r#"
                SELECT credential_value
                FROM credentials
                WHERE user_id = ?1
                  AND credential_name = ?2
                  AND credential_type = ?3
                  AND is_active = 1
                ORDER BY updated_at DESC, id DESC
                LIMIT 1
                "#,
                params![user_id, credential_name, credential_type],
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn upsert_named_credential(
        &self,
        user_id: &str,
        credential_name: &str,
        credential_type: &str,
        credential_value: &str,
        description: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                r#"
                UPDATE credentials
                SET credential_value = ?1, description = ?2, is_active = 1, updated_at = ?3
                WHERE id = (
                    SELECT id FROM credentials
                    WHERE user_id = ?4 AND credential_name = ?5 AND credential_type = ?6
                    ORDER BY updated_at DESC, id DESC
                    LIMIT 1
                )
                "#,
                params![
                    credential_value,
                    description,
                    now,
                    user_id,
                    credential_name,
                    credential_type,
                ],
            )?;
            if changed == 0 {
                conn.execute(
                    r#"
                    INSERT INTO credentials (
                        user_id, credential_name, credential_type, credential_value,
                        description, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                    "#,
                    params![
                        user_id,
                        credential_name,
                        credential_type,
                        credential_value,
                        description,
                        now,
                    ],
                )?;
            }
            Ok(())
        })
    }

    pub fn create_credential(
        &self,
        user_id: &str,
        credential_name: &str,
        credential_type: &str,
        credential_value: &str,
        description: Option<&str>,
    ) -> Result<CredentialRecord> {
        let now = Utc::now();
        self.with_connection(|conn| {
            conn.execute(
                r#"
                INSERT INTO credentials (
                    user_id, credential_name, credential_type, credential_value,
                    description, created_at, updated_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
                params![
                    user_id,
                    credential_name,
                    credential_type,
                    credential_value,
                    description,
                    now.to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )?;
            Ok(CredentialRecord {
                id: conn.last_insert_rowid(),
                credential_name: credential_name.to_string(),
                credential_type: credential_type.to_string(),
                description: description.map(str::to_string),
                is_active: true,
                created_at: now,
                updated_at: now,
            })
        })
    }

    pub fn delete_credential(&self, user_id: &str, credential_id: i64) -> Result<bool> {
        self.with_connection(|conn| {
            let changed = conn.execute(
                "DELETE FROM credentials WHERE user_id = ?1 AND id = ?2",
                params![user_id, credential_id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn toggle_credential(
        &self,
        user_id: &str,
        credential_id: i64,
        is_active: bool,
    ) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.with_connection(|conn| {
            let changed = conn.execute(
                r#"
                UPDATE credentials
                SET is_active = ?1, updated_at = ?2
                WHERE user_id = ?3 AND id = ?4
                "#,
                params![if is_active { 1 } else { 0 }, now, user_id, credential_id],
            )?;
            Ok(changed > 0)
        })
    }

}

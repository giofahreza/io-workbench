fn serialize_session_metadata(session: &SessionSummary) -> String {
    use serde_json::json;
    let mut value = serde_json::Map::new();
    if session.external {
        value.insert("external".into(), json!(true));
    }
    if session.is_board_session() {
        value.insert("boardSession".into(), json!(true));
    }
    if let Some(board_id) = session.board_id.as_ref() {
        value.insert("boardId".into(), json!(board_id));
    }
    if let Some(board_task_id) = session.board_task_id.as_ref() {
        value.insert("boardTaskId".into(), json!(board_task_id));
    }
    if let Some(native_session_id) = session.native_session_id.as_ref() {
        value.insert("nativeSessionId".into(), json!(native_session_id));
    }
    if session.native_rollout_owned_by_provider {
        value.insert("nativeRolloutOwnedByProvider".into(), json!(true));
    }
    if let Some(title_source) = session.title_source {
        value.insert("titleSource".into(), json!(title_source));
    }
    if let Some(model) = session.model.as_ref() {
        value.insert("model".into(), json!(model));
    }
    if let Some(runtime) = session.runtime {
        value.insert("runtime".into(), json!(runtime));
    }
    if let Some(effort) = session.effort.as_ref() {
        value.insert("effort".into(), json!(effort));
    }
    if let Some(mode) = session.mode.as_ref() {
        value.insert("mode".into(), json!(mode));
    }
    if let Some(thinking) = session.thinking {
        value.insert("thinking".into(), json!(thinking));
    }
    if let Some(fast) = session.fast {
        value.insert("fast".into(), json!(fast));
    }
    if let Some(at) = session.last_message_at {
        value.insert("lastMessageAt".into(), json!(at));
    }
    if let Some(at) = session.first_user_at {
        value.insert("firstUserAt".into(), json!(at));
    }
    if let Some(at) = session.received_at {
        value.insert("receivedAt".into(), json!(at));
    }
    if let Some(usage) = session.token_usage.as_ref() {
        value.insert(
            "tokenUsage".into(),
            serde_json::to_value(usage).unwrap_or(serde_json::Value::Null),
        );
    }
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
}

fn deserialize_session_metadata(raw: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(raw).ok()
}

fn merge_metadata_patch(target: &mut serde_json::Value, patch: serde_json::Value) {
    match (target, patch) {
        (serde_json::Value::Object(target), serde_json::Value::Object(patch)) => {
            target.extend(patch);
        }
        (target, patch) => *target = patch,
    }
}

fn merge_metadata_into(session: &mut SessionSummary, value: serde_json::Value) {
    use serde_json::Value;
    if let Some(v) = value.get("external").and_then(Value::as_bool) {
        session.external = v;
    }
    if let Some(v) = value.get("boardSession").and_then(Value::as_bool) {
        session.board_session = v;
    }
    if let Some(v) = value
        .get("boardId")
        .or_else(|| value.get("boardRunId"))
        .and_then(Value::as_str)
    {
        session.board_id = Some(v.to_string());
    }
    if let Some(v) = value.get("boardTaskId").and_then(Value::as_str) {
        session.board_task_id = Some(v.to_string());
    }
    if session.is_board_session() {
        session.board_session = true;
    }
    if let Some(v) = value.get("nativeSessionId").and_then(Value::as_str) {
        session.native_session_id = Some(v.to_string());
    }
    if let Some(v) = value
        .get("nativeRolloutOwnedByProvider")
        .and_then(Value::as_bool)
    {
        session.native_rollout_owned_by_provider = v;
    }
    if let Some(v) = value.get("titleSource") {
        session.title_source = serde_json::from_value(v.clone()).ok();
    }
    if let Some(v) = value.get("model").and_then(Value::as_str) {
        session.model = Some(v.to_string());
    }
    if let Some(v) = value.get("runtime") {
        session.runtime = serde_json::from_value(v.clone()).ok();
    }
    if let Some(v) = value.get("effort").and_then(Value::as_str) {
        session.effort = Some(v.to_string());
    }
    if let Some(v) = value.get("mode").and_then(Value::as_str) {
        session.mode = Some(v.to_string());
    }
    if let Some(v) = value.get("thinking").and_then(Value::as_bool) {
        session.thinking = Some(v);
    }
    if let Some(v) = value.get("fast").and_then(Value::as_bool) {
        session.fast = Some(v);
    }
    if let Some(v) = value.get("lastMessageAt").and_then(Value::as_str) {
        if let Ok(ts) = parse_time(v) {
            session.last_message_at = Some(ts);
        }
    }
    if let Some(v) = value.get("firstUserAt").and_then(Value::as_str) {
        if let Ok(ts) = parse_time(v) {
            session.first_user_at = Some(ts);
        }
    }
    if let Some(v) = value.get("receivedAt").and_then(Value::as_str) {
        if let Ok(ts) = parse_time(v) {
            session.received_at = Some(ts);
        }
    }
    if let Some(v) = value.get("tokenUsage") {
        if let Ok(usage) = serde_json::from_value::<iowb_protocol::SessionTokenUsage>(v.clone()) {
            session.token_usage = Some(usage);
        }
    }
}

fn legacy_session_title_from_prompt(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.chars().count() <= 50 {
        return trimmed.to_string();
    }

    format!("{}...", trimmed.chars().take(50).collect::<String>())
}

fn map_user_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredUser> {
    let last_login_raw: Option<String> = row.get(5)?;
    Ok(StoredUser {
        id: row.get(0)?,
        username: row.get(1)?,
        password_hash: row.get(2)?,
        created_at: parse_time_sql(row.get::<_, String>(3)?)?,
        updated_at: parse_time_sql(row.get::<_, String>(4)?)?,
        last_login_at: last_login_raw.map(parse_time_sql).transpose()?,
    })
}

fn map_api_key_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApiKeyRecord> {
    let key_prefix: String = row.get(2)?;
    Ok(ApiKeyRecord {
        id: row.get(0)?,
        key_name: row.get(1)?,
        masked_key: mask_secret(&key_prefix),
        key_prefix,
        is_active: row.get::<_, i64>(3)? == 1,
        created_at: parse_time_sql(row.get::<_, String>(4)?)?,
        updated_at: parse_time_sql(row.get::<_, String>(5)?)?,
    })
}

fn map_credential_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CredentialRecord> {
    Ok(CredentialRecord {
        id: row.get(0)?,
        credential_name: row.get(1)?,
        credential_type: row.get(2)?,
        description: row.get(3)?,
        is_active: row.get::<_, i64>(4)? == 1,
        created_at: parse_time_sql(row.get::<_, String>(5)?)?,
        updated_at: parse_time_sql(row.get::<_, String>(6)?)?,
    })
}

fn map_database_connection_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StoredDatabaseConnection> {
    let db_type_raw: String = row.get(2)?;
    let port_raw: Option<i64> = row.get(4)?;
    let password: Option<String> = row.get(6)?;
    let last_test_status_raw: Option<String> = row.get(10)?;
    let last_tested_at_raw: Option<String> = row.get(12)?;

    Ok(StoredDatabaseConnection {
        profile: DatabaseConnectionProfile {
            id: row.get(0)?,
            name: row.get(1)?,
            db_type: parse_database_type(&db_type_raw),
            host: row.get(3)?,
            port: port_raw.and_then(|value| u16::try_from(value).ok()),
            username: row.get(5)?,
            database_name: row.get(7)?,
            file_path: row.get(8)?,
            show_all_databases: row.get::<_, i64>(9)? == 1,
            has_password: password.as_deref().is_some_and(|value| !value.is_empty()),
            last_test_status: last_test_status_raw
                .as_deref()
                .map(parse_database_test_status),
            last_test_message: row.get(11)?,
            last_tested_at: last_tested_at_raw.map(parse_time_sql).transpose()?,
            created_at: parse_time_sql(row.get::<_, String>(13)?)?,
            updated_at: parse_time_sql(row.get::<_, String>(14)?)?,
        },
        password,
    })
}

fn mask_secret(prefix: &str) -> String {
    format!("{prefix}...")
}

fn bounded_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn nonnegative_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn nonnegative_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

fn external_message_state_matches(
    conn: &Connection,
    provider: Provider,
    session_id: &str,
    file_path: &str,
    fingerprint: &ExternalHistoryFingerprint<'_>,
) -> Result<bool> {
    Ok(
        external_message_state_total_if_matches(
            conn,
            provider,
            session_id,
            file_path,
            fingerprint,
        )?
        .is_some(),
    )
}

fn external_message_state_total_if_matches(
    conn: &Connection,
    provider: Provider,
    session_id: &str,
    file_path: &str,
    fingerprint: &ExternalHistoryFingerprint<'_>,
) -> Result<Option<usize>> {
    let state = conn
        .query_row(
            r#"
            SELECT file_identity, file_size, modified_nanos, parser_version,
                   total_count
            FROM external_history_message_state
            WHERE provider = ?1 AND session_id = ?2 AND file_path = ?3
            "#,
            params![provider.as_str(), session_id, file_path],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((identity, size, modified_nanos, parser_version, total_count)) = state else {
        return Ok(None);
    };
    if identity.as_deref() != fingerprint.file_identity
        || nonnegative_u64(size) != fingerprint.file_size
        || modified_nanos != fingerprint.modified_nanos
        || nonnegative_u32(parser_version) != fingerprint.parser_version
    {
        return Ok(None);
    }
    Ok(Some(
        usize::try_from(nonnegative_u64(total_count)).unwrap_or(usize::MAX),
    ))
}

fn parse_provider(raw: &str) -> Provider {
    match raw {
        "codex" => Provider::Codex,
        "gemini" => Provider::Gemini,
        _ => Provider::Claude,
    }
}

fn parse_database_type(raw: &str) -> SupportedDatabaseType {
    match raw {
        "postgresql" => SupportedDatabaseType::Postgresql,
        "mysql" => SupportedDatabaseType::Mysql,
        "mariadb" => SupportedDatabaseType::Mariadb,
        _ => SupportedDatabaseType::Sqlite,
    }
}

fn parse_database_test_status(raw: &str) -> DatabaseTestStatus {
    match raw {
        "success" => DatabaseTestStatus::Success,
        _ => DatabaseTestStatus::Error,
    }
}

fn database_test_status_to_str(status: DatabaseTestStatus) -> &'static str {
    match status {
        DatabaseTestStatus::Success => "success",
        DatabaseTestStatus::Error => "error",
    }
}

fn role_to_str(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn parse_role(raw: &str) -> MessageRole {
    match raw {
        "system" => MessageRole::System,
        "assistant" => MessageRole::Assistant,
        "tool" => MessageRole::Tool,
        _ => MessageRole::User,
    }
}

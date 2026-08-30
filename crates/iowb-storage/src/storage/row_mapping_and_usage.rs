fn parse_time(raw: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(raw)?.with_timezone(&Utc))
}

fn parse_time_sql(raw: String) -> rusqlite::Result<DateTime<Utc>> {
    if let Ok(time) = DateTime::parse_from_rfc3339(&raw) {
        return Ok(time.with_timezone(&Utc));
    }

    NaiveDateTime::parse_from_str(&raw, "%Y-%m-%d %H:%M:%S")
        .map(|time| time.and_utc())
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))
}

fn upsert_session_conn(conn: &Connection, session: &SessionSummary) -> Result<()> {
    let metadata_blob = serialize_session_metadata(session);
    conn.execute(
        r#"
        INSERT INTO sessions (
            id, provider, project_path, title, message_count, last_activity, active, model, metadata
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(id) DO UPDATE SET
            provider = excluded.provider,
            project_path = excluded.project_path,
            title = excluded.title,
            message_count = excluded.message_count,
            last_activity = excluded.last_activity,
            active = excluded.active,
            model = excluded.model,
            metadata = excluded.metadata
        "#,
        params![
            session.id,
            session.provider.as_str(),
            session.project_path,
            session.title,
            session.message_count as i64,
            session.last_activity.to_rfc3339(),
            if session.active { 1 } else { 0 },
            session.model,
            metadata_blob,
        ],
    )?;
    Ok(())
}

fn insert_message_conn(conn: &Connection, session_id: &str, message: &ChatMessage) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO messages (id, session_id, role, content, timestamp, metadata)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        "#,
        params![
            message.id,
            session_id,
            role_to_str(message.role),
            message.content,
            message.timestamp.to_rfc3339(),
            serde_json::to_string(&message.metadata)?,
        ],
    )?;
    Ok(())
}

fn insert_durable_chat_run_conn(conn: &Connection, run: &StoredDurableChatRun) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO durable_chat_runs (
            id, user_id, session_id, native_session_id, provider, prompt,
            project_path, model, effort, mode, thinking, status, auto_resume,
            resume_attempts, last_error, created_at, updated_at, recovered_at,
            completed_at, user_message_id, native_before_turn_id, fast
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22
        )
        "#,
        params![
            run.id,
            run.user_id,
            run.session_id,
            run.native_session_id,
            run.provider,
            run.prompt,
            run.project_path,
            run.model,
            run.effort,
            run.mode,
            run.thinking.map(i64::from),
            run.status,
            i64::from(run.auto_resume),
            i64::from(run.resume_attempts),
            run.last_error,
            run.created_at.to_rfc3339(),
            run.updated_at.to_rfc3339(),
            run.recovered_at.map(|time| time.to_rfc3339()),
            run.completed_at.map(|time| time.to_rfc3339()),
            run.user_message_id,
            run.native_before_turn_id,
            run.fast.map(i64::from),
        ],
    )?;
    Ok(())
}

fn insert_chat_run_attempt_conn(conn: &Connection, attempt: &StoredChatRunAttempt) -> Result<bool> {
    let zero = SessionTokenUsage::default();
    let usage = attempt.usage.as_ref().unwrap_or(&zero);
    let inserted = conn.execute(
        r#"
        INSERT OR IGNORE INTO chat_run_attempts (
            id, durable_run_id, session_id, user_message_id, provider, runtime,
            model, native_session_id, status, input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens, reasoning_tokens,
            total_tokens, cost_usd, raw_usage_json, source, completeness,
            created_at, updated_at, completed_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
            ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22
        )
        "#,
        params![
            attempt.id,
            attempt.durable_run_id,
            attempt.session_id,
            attempt.user_message_id,
            attempt.provider,
            attempt.runtime,
            attempt.model,
            attempt.native_session_id,
            attempt.status,
            usage.input as i64,
            usage.output as i64,
            usage.cache_creation as i64,
            usage.cache_read as i64,
            usage.reasoning as i64,
            usage.used as i64,
            usage.cost_usd,
            attempt.raw_usage_json,
            attempt.source,
            token_usage_completeness_to_str(attempt.completeness),
            attempt.created_at.to_rfc3339(),
            attempt.updated_at.to_rfc3339(),
            attempt.completed_at.map(|time| time.to_rfc3339()),
        ],
    )?;
    Ok(inserted > 0)
}

fn session_lifetime_token_usage_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionLifetimeTokenUsage> {
    let attempts = lifetime_attempt_usage_for_session_conn(conn, session_id)?;
    let baseline = session_usage_baseline_conn(conn, session_id)?;
    Ok(combine_lifetime_usage(baseline, attempts))
}

fn session_spent_token_usage_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionSpentTokenUsage> {
    let provider = session_provider_conn(conn, session_id)?;
    let compacted_at = latest_active_context_compacted_at_conn(conn, session_id)?;
    let whole_session = session_spent_token_usage_scope_conn(conn, session_id, &provider, None)?;
    let since_compact = compacted_at
        .map(|compacted_at| {
            session_spent_token_usage_scope_conn(conn, session_id, &provider, Some(compacted_at))
        })
        .transpose()?;
    Ok(SessionSpentTokenUsage {
        whole_session,
        since_compact,
        compacted_at,
    })
}

fn session_spent_token_usage_scope_conn(
    conn: &Connection,
    session_id: &str,
    provider: &str,
    created_after: Option<DateTime<Utc>>,
) -> Result<SessionLifetimeTokenUsage> {
    let attempts = if provider == Provider::Codex.as_str() {
        codex_spent_attempt_usage_conn(conn, session_id, created_after)?
    } else {
        attempt_spent_usage_for_session_conn(conn, session_id, created_after)?
    };
    if created_after.is_some() {
        Ok(attempts)
    } else {
        Ok(combine_lifetime_usage(
            session_usage_baseline_conn(conn, session_id)?,
            attempts,
        ))
    }
}

fn session_context_token_usage_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionContextTokenUsage> {
    let lifetime = session_lifetime_token_usage_conn(conn, session_id)?;
    let provider = session_provider_conn(conn, session_id)?;
    let Some(compacted_at) = latest_active_context_compacted_at_conn(conn, session_id)? else {
        if provider == Provider::Codex.as_str() {
            let mut usage = codex_latest_cumulative_usage_conn(conn, session_id)?
                .unwrap_or_else(|| lifetime.clone());
            let (partial_attempts, missing_attempts) =
                attempt_completeness_counts_conn(conn, session_id, None)?;
            usage.partial_attempts = partial_attempts;
            usage.missing_attempts = missing_attempts;
            usage.completeness = lifetime_usage_completeness(&usage);
            return Ok(context_usage_from_lifetime(usage, false, None));
        }
        return Ok(context_usage_from_lifetime(lifetime, false, None));
    };
    let scoped = if provider == Provider::Codex.as_str() {
        codex_context_usage_delta_conn(conn, session_id, compacted_at)?
    } else {
        attempt_usage_since_compact_conn(conn, session_id, compacted_at)?
    };
    Ok(context_usage_from_lifetime(
        scoped,
        true,
        Some(compacted_at),
    ))
}

fn session_usage_baseline_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionLifetimeTokenUsage> {
    conn.query_row(
        r#"
        SELECT total_tokens, input_tokens, output_tokens, cache_creation_tokens,
               cache_read_tokens, reasoning_tokens, cost_usd,
               partial_attempts, missing_attempts
        FROM session_usage_baselines
        WHERE session_id = ?1
        "#,
        params![session_id],
        map_lifetime_usage_row,
    )
    .optional()
    .map(|value| value.unwrap_or_default())
    .map_err(StorageError::from)
}

fn session_provider_conn(conn: &Connection, session_id: &str) -> Result<String> {
    conn.query_row(
        "SELECT provider FROM sessions WHERE id = ?1",
        params![session_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map(|value| value.unwrap_or_default())
    .map_err(StorageError::from)
}

fn latest_active_context_compacted_at_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<DateTime<Utc>>> {
    conn.query_row(
        r#"
        SELECT activated_at
        FROM session_context_rollovers
        WHERE session_id = ?1
          AND state = 'active'
          AND activated_at IS NOT NULL
        ORDER BY activated_at DESC, created_at DESC, id DESC
        LIMIT 1
        "#,
        params![session_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|value| parse_time(&value))
    .transpose()
}

fn attempt_usage_since_compact_conn(
    conn: &Connection,
    session_id: &str,
    compacted_at: DateTime<Utc>,
) -> Result<SessionLifetimeTokenUsage> {
    conn.query_row(
        r#"
        SELECT COALESCE(SUM(total_tokens), 0),
               COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(output_tokens), 0),
               COALESCE(SUM(cache_creation_tokens), 0),
               COALESCE(SUM(cache_read_tokens), 0),
               COALESCE(SUM(reasoning_tokens), 0),
               COALESCE(SUM(cost_usd), 0),
               COALESCE(SUM(CASE WHEN completeness = 'partial' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completeness = 'missing' THEN 1 ELSE 0 END), 0)
        FROM chat_run_attempts
        WHERE session_id = ?1
          AND created_at > ?2
          AND COALESCE(source, '') <> 'codex_app_server'
        "#,
        params![session_id, compacted_at.to_rfc3339()],
        map_lifetime_usage_row,
    )
    .map_err(StorageError::from)
}

fn attempt_spent_usage_for_session_conn(
    conn: &Connection,
    session_id: &str,
    created_after: Option<DateTime<Utc>>,
) -> Result<SessionLifetimeTokenUsage> {
    let after_filter = if created_after.is_some() {
        "AND created_at > ?2"
    } else {
        ""
    };
    let sql = format!(
        r#"
        SELECT COALESCE(SUM(total_tokens), 0),
               COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(output_tokens), 0),
               COALESCE(SUM(cache_creation_tokens), 0),
               COALESCE(SUM(cache_read_tokens), 0),
               COALESCE(SUM(reasoning_tokens), 0),
               COALESCE(SUM(cost_usd), 0),
               COALESCE(SUM(CASE WHEN completeness = 'partial' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completeness = 'missing' THEN 1 ELSE 0 END), 0)
        FROM chat_run_attempts
        WHERE session_id = ?1
          {after_filter}
          AND COALESCE(source, '') <> 'codex_app_server'
        "#
    );
    let created_after = created_after.map(|value| value.to_rfc3339());
    let mut usage = match created_after.as_deref() {
        Some(created_after) => conn.query_row(
            &sql,
            params![session_id, created_after],
            map_lifetime_usage_row,
        ),
        None => conn.query_row(&sql, params![session_id], map_lifetime_usage_row),
    }
    .map_err(StorageError::from)?;
    usage.completeness = lifetime_usage_completeness(&usage);
    Ok(usage)
}

#[derive(Debug)]
struct CodexUsageSnapshot {
    native_session_id: Option<String>,
    usage: SessionLifetimeTokenUsage,
}

fn codex_spent_attempt_usage_conn(
    conn: &Connection,
    session_id: &str,
    created_after: Option<DateTime<Utc>>,
) -> Result<SessionLifetimeTokenUsage> {
    let created_after = created_after.map(|value| value.to_rfc3339());
    let mut previous_by_native = created_after
        .as_deref()
        .map(|created_after| codex_usage_baselines_before_conn(conn, session_id, created_after))
        .transpose()?
        .unwrap_or_default();
    let snapshots = codex_usage_snapshots_conn(conn, session_id, created_after.as_deref())?;
    let mut spent = SessionLifetimeTokenUsage::default();
    for snapshot in snapshots {
        let key = snapshot.native_session_id.clone();
        let delta = match previous_by_native.get(&key) {
            Some(previous) if snapshot.usage.total >= previous.total => {
                subtract_lifetime_usage(snapshot.usage.clone(), previous.clone())
            }
            _ => snapshot.usage.clone(),
        };
        spent = combine_lifetime_usage(spent, delta);
        previous_by_native.insert(key, snapshot.usage);
    }
    let (partial_attempts, missing_attempts) =
        attempt_completeness_counts_conn(conn, session_id, created_after.as_deref())?;
    spent.partial_attempts = partial_attempts;
    spent.missing_attempts = missing_attempts;
    spent.completeness = lifetime_usage_completeness(&spent);
    Ok(spent)
}

fn codex_usage_baselines_before_conn(
    conn: &Connection,
    session_id: &str,
    created_before_or_at: &str,
) -> Result<HashMap<Option<String>, SessionLifetimeTokenUsage>> {
    let mut statement = conn.prepare(
        r#"
        SELECT native_session_id, total_tokens, input_tokens, output_tokens,
               cache_creation_tokens, cache_read_tokens, reasoning_tokens, cost_usd, 0, 0
        FROM chat_run_attempts
        WHERE session_id = ?1
          AND provider = 'codex'
          AND completed_at IS NOT NULL
          AND created_at <= ?2
          AND completeness <> 'missing'
          AND total_tokens > 0
          AND COALESCE(source, '') <> 'codex_app_server'
        ORDER BY completed_at ASC, created_at ASC, id ASC
        "#,
    )?;
    let rows = statement.query_map(params![session_id, created_before_or_at], |row| {
        Ok(CodexUsageSnapshot {
            native_session_id: row.get(0)?,
            usage: map_lifetime_usage_row_at(row, 1)?,
        })
    })?;
    let mut baselines = HashMap::new();
    for row in rows {
        let snapshot = row?;
        baselines.insert(snapshot.native_session_id, snapshot.usage);
    }
    Ok(baselines)
}

fn codex_usage_snapshots_conn(
    conn: &Connection,
    session_id: &str,
    created_after: Option<&str>,
) -> Result<Vec<CodexUsageSnapshot>> {
    let after_filter = if created_after.is_some() {
        "AND created_at > ?2"
    } else {
        ""
    };
    let sql = format!(
        r#"
        SELECT native_session_id, total_tokens, input_tokens, output_tokens,
               cache_creation_tokens, cache_read_tokens, reasoning_tokens, cost_usd, 0, 0
        FROM chat_run_attempts
        WHERE session_id = ?1
          AND provider = 'codex'
          AND completed_at IS NOT NULL
          {after_filter}
          AND completeness <> 'missing'
          AND total_tokens > 0
          AND COALESCE(source, '') <> 'codex_app_server'
        ORDER BY completed_at ASC, created_at ASC, id ASC
        "#
    );
    let mut statement = conn.prepare(&sql)?;
    let mapper = |row: &rusqlite::Row<'_>| {
        Ok(CodexUsageSnapshot {
            native_session_id: row.get(0)?,
            usage: map_lifetime_usage_row_at(row, 1)?,
        })
    };
    let rows = match created_after {
        Some(created_after) => statement.query_map(params![session_id, created_after], mapper)?,
        None => statement.query_map(params![session_id], mapper)?,
    };
    let mut snapshots = Vec::new();
    for row in rows {
        snapshots.push(row?);
    }
    Ok(snapshots)
}

fn codex_context_usage_delta_conn(
    conn: &Connection,
    session_id: &str,
    compacted_at: DateTime<Utc>,
) -> Result<SessionLifetimeTokenUsage> {
    let compacted_at = compacted_at.to_rfc3339();
    let latest = cumulative_attempt_usage_conn(
        conn,
        r#"
        SELECT total_tokens, input_tokens, output_tokens, cache_creation_tokens,
               cache_read_tokens, reasoning_tokens, cost_usd, 0, 0
        FROM chat_run_attempts
        WHERE session_id = ?1
          AND provider = 'codex'
          AND completed_at IS NOT NULL
          AND completed_at > ?2
          AND completeness <> 'missing'
          AND total_tokens > 0
          AND COALESCE(source, '') <> 'codex_app_server'
        ORDER BY completed_at DESC, created_at DESC, id DESC
        LIMIT 1
        "#,
        session_id,
        &compacted_at,
    )?;
    let baseline = cumulative_attempt_usage_conn(
        conn,
        r#"
        SELECT total_tokens, input_tokens, output_tokens, cache_creation_tokens,
               cache_read_tokens, reasoning_tokens, cost_usd, 0, 0
        FROM chat_run_attempts
        WHERE session_id = ?1
          AND provider = 'codex'
          AND completed_at IS NOT NULL
          AND completed_at <= ?2
          AND completeness <> 'missing'
          AND total_tokens > 0
          AND COALESCE(source, '') <> 'codex_app_server'
        ORDER BY completed_at DESC, created_at DESC, id DESC
        LIMIT 1
        "#,
        session_id,
        &compacted_at,
    )?;
    let mut usage =
        subtract_lifetime_usage(latest.unwrap_or_default(), baseline.unwrap_or_default());
    let (partial_attempts, missing_attempts) =
        attempt_completeness_counts_conn(conn, session_id, Some(&compacted_at))?;
    usage.partial_attempts = partial_attempts;
    usage.missing_attempts = missing_attempts;
    usage.completeness = lifetime_usage_completeness(&usage);
    Ok(usage)
}

fn codex_latest_cumulative_usage_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionLifetimeTokenUsage>> {
    conn.query_row(
        r#"
        SELECT total_tokens, input_tokens, output_tokens, cache_creation_tokens,
               cache_read_tokens, reasoning_tokens, cost_usd, 0, 0
        FROM chat_run_attempts
        WHERE session_id = ?1
          AND provider = 'codex'
          AND completed_at IS NOT NULL
          AND completeness <> 'missing'
          AND total_tokens > 0
          AND COALESCE(source, '') <> 'codex_app_server'
        ORDER BY completed_at DESC, created_at DESC, id DESC
        LIMIT 1
        "#,
        params![session_id],
        map_lifetime_usage_row,
    )
    .optional()
    .map_err(StorageError::from)
}

fn cumulative_attempt_usage_conn(
    conn: &Connection,
    sql: &str,
    session_id: &str,
    compacted_at: &str,
) -> Result<Option<SessionLifetimeTokenUsage>> {
    conn.query_row(
        sql,
        params![session_id, compacted_at],
        map_lifetime_usage_row,
    )
    .optional()
    .map_err(StorageError::from)
}

fn attempt_completeness_counts_conn(
    conn: &Connection,
    session_id: &str,
    created_after: Option<&str>,
) -> Result<(u64, u64)> {
    let after_filter = if created_after.is_some() {
        "AND created_at > ?2"
    } else {
        ""
    };
    let sql = format!(
        r#"
        SELECT COALESCE(SUM(CASE WHEN completeness = 'partial' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completeness = 'missing' THEN 1 ELSE 0 END), 0)
        FROM chat_run_attempts
        WHERE session_id = ?1
          {after_filter}
          AND COALESCE(source, '') <> 'codex_app_server'
        "#
    );
    let mapper = |row: &rusqlite::Row<'_>| Ok((row_i64_to_u64(row, 0)?, row_i64_to_u64(row, 1)?));
    match created_after {
        Some(created_after) => conn.query_row(&sql, params![session_id, created_after], mapper),
        None => conn.query_row(&sql, params![session_id], mapper),
    }
    .map_err(StorageError::from)
}

fn subtract_lifetime_usage(
    latest: SessionLifetimeTokenUsage,
    baseline: SessionLifetimeTokenUsage,
) -> SessionLifetimeTokenUsage {
    let mut usage = SessionLifetimeTokenUsage {
        total: latest.total.saturating_sub(baseline.total),
        input: latest.input.saturating_sub(baseline.input),
        output: latest.output.saturating_sub(baseline.output),
        cache_creation: latest
            .cache_creation
            .saturating_sub(baseline.cache_creation),
        cache_read: latest.cache_read.saturating_sub(baseline.cache_read),
        reasoning: latest.reasoning.saturating_sub(baseline.reasoning),
        cost_usd: (latest.cost_usd - baseline.cost_usd).max(0.0),
        partial_attempts: 0,
        missing_attempts: 0,
        completeness: TokenUsageCompleteness::Complete,
    };
    usage.completeness = lifetime_usage_completeness(&usage);
    usage
}

fn context_usage_from_lifetime(
    usage: SessionLifetimeTokenUsage,
    after_compact: bool,
    compacted_at: Option<DateTime<Utc>>,
) -> SessionContextTokenUsage {
    SessionContextTokenUsage {
        total: usage.total,
        input: usage.input,
        output: usage.output,
        cache_creation: usage.cache_creation,
        cache_read: usage.cache_read,
        reasoning: usage.reasoning,
        cost_usd: usage.cost_usd,
        completeness: usage.completeness,
        partial_attempts: usage.partial_attempts,
        missing_attempts: usage.missing_attempts,
        after_compact,
        compacted_at,
    }
}

fn attach_session_usage_conn(conn: &Connection, sessions: &mut [SessionSummary]) -> Result<()> {
    if sessions.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", sessions.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        r#"
        SELECT session_id,
               COALESCE(SUM(total_tokens), 0),
               COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(output_tokens), 0),
               COALESCE(SUM(cache_creation_tokens), 0),
               COALESCE(SUM(cache_read_tokens), 0),
               COALESCE(SUM(reasoning_tokens), 0),
               COALESCE(SUM(cost_usd), 0),
               COALESCE(SUM(partial_attempts), 0),
               COALESCE(SUM(missing_attempts), 0)
        FROM (
            SELECT session_id, total_tokens, input_tokens, output_tokens,
                   cache_creation_tokens, cache_read_tokens, reasoning_tokens,
                   cost_usd,
                   CASE WHEN completeness = 'partial' THEN 1 ELSE 0 END AS partial_attempts,
                   CASE WHEN completeness = 'missing' THEN 1 ELSE 0 END AS missing_attempts
            FROM chat_run_attempts

            UNION ALL

            SELECT session_id, total_tokens, input_tokens, output_tokens,
                   cache_creation_tokens, cache_read_tokens, reasoning_tokens,
                   cost_usd, partial_attempts, missing_attempts
            FROM session_usage_baselines
        ) usage
        WHERE session_id IN ({placeholders})
        GROUP BY session_id
        "#,
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        params_from_iter(sessions.iter().map(|session| session.id.as_str())),
        |row| Ok((row.get::<_, String>(0)?, map_lifetime_usage_row_at(row, 1)?)),
    )?;
    let mut usage_by_session = HashMap::new();
    for row in rows {
        let (session_id, usage) = row?;
        usage_by_session.insert(session_id, usage);
    }
    for session in sessions {
        session.lifetime_token_usage =
            Some(usage_by_session.remove(&session.id).unwrap_or_default());
        session.context_token_usage = Some(session_context_token_usage_conn(conn, &session.id)?);
        session.spent_token_usage = Some(session_spent_token_usage_conn(conn, &session.id)?);
    }
    Ok(())
}

fn lifetime_attempt_usage_for_session_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionLifetimeTokenUsage> {
    conn.query_row(
        r#"
        SELECT COALESCE(SUM(total_tokens), 0),
               COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(output_tokens), 0),
               COALESCE(SUM(cache_creation_tokens), 0),
               COALESCE(SUM(cache_read_tokens), 0),
               COALESCE(SUM(reasoning_tokens), 0),
               COALESCE(SUM(cost_usd), 0),
               COALESCE(SUM(CASE WHEN completeness = 'partial' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completeness = 'missing' THEN 1 ELSE 0 END), 0)
        FROM chat_run_attempts
        WHERE session_id = ?1
        "#,
        params![session_id],
        map_lifetime_usage_row,
    )
    .map_err(StorageError::from)
}

fn fork_usage_baseline_conn(
    conn: &Connection,
    source_session_id: &str,
    before_message_id: &str,
    destination: &SessionSummary,
    messages: &[ChatMessage],
) -> Result<SessionLifetimeTokenUsage> {
    let mut usage_sources = messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .map(|message| usage_source_ref(source_session_id, message))
        .collect::<Vec<_>>();
    usage_sources.sort();
    usage_sources.dedup();

    if usage_sources.is_empty() {
        return Ok(SessionLifetimeTokenUsage::default());
    }

    let source_conditions = std::iter::repeat_n(
        "(a.session_id = ? AND a.user_message_id = ?)",
        usage_sources.len(),
    )
    .collect::<Vec<_>>()
    .join(" OR ");
    let baseline_conditions = std::iter::repeat_n(
        "(f.source_session_id = ? AND f.before_message_id = ?)",
        usage_sources.len(),
    )
    .collect::<Vec<_>>()
    .join(" OR ");
    let sql = format!(
        r#"
        SELECT COALESCE(SUM(total_tokens), 0),
               COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(output_tokens), 0),
               COALESCE(SUM(cache_creation_tokens), 0),
               COALESCE(SUM(cache_read_tokens), 0),
               COALESCE(SUM(reasoning_tokens), 0),
               COALESCE(SUM(cost_usd), 0),
               COALESCE(SUM(partial_attempts), 0),
               COALESCE(SUM(missing_attempts), 0)
        FROM (
            SELECT a.total_tokens, a.input_tokens, a.output_tokens,
                   a.cache_creation_tokens, a.cache_read_tokens,
                   a.reasoning_tokens, a.cost_usd,
                   CASE WHEN a.completeness = 'partial' THEN 1 ELSE 0 END AS partial_attempts,
                   CASE WHEN a.completeness = 'missing' THEN 1 ELSE 0 END AS missing_attempts
            FROM chat_run_attempts a
            WHERE {source_conditions}

            UNION ALL

            SELECT b.total_tokens, b.input_tokens, b.output_tokens,
                   b.cache_creation_tokens, b.cache_read_tokens,
                   b.reasoning_tokens, b.cost_usd,
                   b.partial_attempts, b.missing_attempts
            FROM session_usage_baselines b
            JOIN session_forks f ON f.destination_session_id = b.session_id
            WHERE {baseline_conditions}
        )
        "#
    );
    let bind_values = usage_sources
        .iter()
        .flat_map(|(session_id, message_id)| [session_id.clone(), message_id.clone()])
        .chain(
            usage_sources
                .iter()
                .flat_map(|(session_id, message_id)| [session_id.clone(), message_id.clone()]),
        );
    let mut combined =
        conn.query_row(&sql, params_from_iter(bind_values), map_lifetime_usage_row)?;
    combined.completeness = lifetime_usage_completeness(&combined);

    if combined.total == 0
        && combined.partial_attempts == 0
        && combined.missing_attempts == 0
        && destination.lifetime_token_usage.is_some()
    {
        return Ok(destination.lifetime_token_usage.clone().unwrap_or_default());
    }

    let _ = before_message_id;
    Ok(combined)
}

fn usage_source_ref(source_session_id: &str, message: &ChatMessage) -> (String, String) {
    let session_id = message
        .metadata
        .get("usageSourceSessionId")
        .or_else(|| message.metadata.get("forkedFromSessionId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| source_session_id.to_string());
    let message_id = message
        .metadata
        .get("usageSourceMessageId")
        .or_else(|| message.metadata.get("forkedFromMessageId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| message.id.clone());
    (session_id, message_id)
}

fn map_lifetime_usage_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionLifetimeTokenUsage> {
    map_lifetime_usage_row_at(row, 0)
}

fn map_lifetime_usage_row_at(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<SessionLifetimeTokenUsage> {
    let mut usage = SessionLifetimeTokenUsage {
        total: row_i64_to_u64(row, offset)?,
        input: row_i64_to_u64(row, offset + 1)?,
        output: row_i64_to_u64(row, offset + 2)?,
        cache_creation: row_i64_to_u64(row, offset + 3)?,
        cache_read: row_i64_to_u64(row, offset + 4)?,
        reasoning: row_i64_to_u64(row, offset + 5)?,
        cost_usd: row.get(offset + 6)?,
        partial_attempts: row_i64_to_u64(row, offset + 7)?,
        missing_attempts: row_i64_to_u64(row, offset + 8)?,
        completeness: TokenUsageCompleteness::Complete,
    };
    usage.completeness = lifetime_usage_completeness(&usage);
    Ok(usage)
}

fn row_i64_to_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let raw = row.get::<_, i64>(index)?;
    u64::try_from(raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn combine_lifetime_usage(
    mut left: SessionLifetimeTokenUsage,
    right: SessionLifetimeTokenUsage,
) -> SessionLifetimeTokenUsage {
    left.total = left.total.saturating_add(right.total);
    left.input = left.input.saturating_add(right.input);
    left.output = left.output.saturating_add(right.output);
    left.cache_creation = left.cache_creation.saturating_add(right.cache_creation);
    left.cache_read = left.cache_read.saturating_add(right.cache_read);
    left.reasoning = left.reasoning.saturating_add(right.reasoning);
    left.cost_usd += right.cost_usd;
    left.partial_attempts = left.partial_attempts.saturating_add(right.partial_attempts);
    left.missing_attempts = left.missing_attempts.saturating_add(right.missing_attempts);
    left.completeness = lifetime_usage_completeness(&left);
    left
}

fn lifetime_usage_completeness(usage: &SessionLifetimeTokenUsage) -> TokenUsageCompleteness {
    if usage.missing_attempts > 0 && usage.total == 0 {
        TokenUsageCompleteness::Missing
    } else if usage.missing_attempts > 0 || usage.partial_attempts > 0 {
        TokenUsageCompleteness::Partial
    } else {
        TokenUsageCompleteness::Complete
    }
}

fn token_usage_completeness_to_str(value: TokenUsageCompleteness) -> &'static str {
    match value {
        TokenUsageCompleteness::Complete => "complete",
        TokenUsageCompleteness::Partial => "partial",
        TokenUsageCompleteness::Missing => "missing",
    }
}

fn map_durable_chat_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredDurableChatRun> {
    let thinking = row.get::<_, Option<i64>>(10)?.map(|value| value != 0);
    let fast = row.get::<_, Option<i64>>(21)?.map(|value| value != 0);
    let resume_attempts_raw = row.get::<_, i64>(13)?;
    let resume_attempts = u32::try_from(resume_attempts_raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            13,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    let recovered_at = row
        .get::<_, Option<String>>(17)?
        .map(parse_time_sql)
        .transpose()?;
    let completed_at = row
        .get::<_, Option<String>>(18)?
        .map(parse_time_sql)
        .transpose()?;

    Ok(StoredDurableChatRun {
        id: row.get(0)?,
        user_id: row.get(1)?,
        session_id: row.get(2)?,
        native_session_id: row.get(3)?,
        user_message_id: row.get(19)?,
        native_before_turn_id: row.get(20)?,
        provider: row.get(4)?,
        prompt: row.get(5)?,
        project_path: row.get(6)?,
        model: row.get(7)?,
        effort: row.get(8)?,
        mode: row.get(9)?,
        thinking,
        fast,
        status: row.get(11)?,
        auto_resume: row.get::<_, i64>(12)? != 0,
        resume_attempts,
        last_error: row.get(14)?,
        created_at: parse_time_sql(row.get::<_, String>(15)?)?,
        updated_at: parse_time_sql(row.get::<_, String>(16)?)?,
        recovered_at,
        completed_at,
    })
}

fn map_session_context_rollover_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StoredSessionContextRollover> {
    let observed_bytes = row
        .get::<_, Option<i64>>(12)?
        .map(u64::try_from)
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                12,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?;
    let limit_bytes_raw = row.get::<_, i64>(13)?;
    let limit_bytes = u64::try_from(limit_bytes_raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            13,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    Ok(StoredSessionContextRollover {
        id: row.get(0)?,
        user_id: row.get(1)?,
        session_id: row.get(2)?,
        request_id: row.get(3)?,
        kind: row.get(4)?,
        failed_message_id: row.get(5)?,
        trigger_run_id: row.get(6)?,
        retry_run_id: row.get(7)?,
        from_native_session_id: row.get(8)?,
        candidate_native_session_id: row.get(9)?,
        state: row.get(10)?,
        handoff: row.get(11)?,
        observed_bytes,
        limit_bytes,
        error: row.get(14)?,
        created_at: parse_time_sql(row.get::<_, String>(15)?)?,
        updated_at: parse_time_sql(row.get::<_, String>(16)?)?,
        activated_at: row
            .get::<_, Option<String>>(17)?
            .map(parse_time_sql)
            .transpose()?,
    })
}

fn map_fcm_token_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredFcmToken> {
    Ok(StoredFcmToken {
        token: row.get(0)?,
        user_id: row.get(1)?,
        platform: row.get(2)?,
        device_id: row.get(3)?,
        app_id: row.get(4)?,
        created_at: parse_time_sql(row.get::<_, String>(5)?)?,
        updated_at: parse_time_sql(row.get::<_, String>(6)?)?,
        last_seen_at: parse_time_sql(row.get::<_, String>(7)?)?,
    })
}

fn map_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionSummary> {
    let mut session = SessionSummary {
        id: row.get(0)?,
        provider: parse_provider(&row.get::<_, String>(1)?),
        project_path: row.get(2)?,
        title: row.get(3)?,
        message_count: row.get::<_, i64>(4)? as usize,
        last_activity: parse_time_sql(row.get::<_, String>(5)?)?,
        active: row.get::<_, i64>(6)? == 1,
        model: row.get(7)?,
        ..Default::default()
    };
    let metadata_blob: Option<String> = row.get(8).ok();
    if let Some(raw) = metadata_blob {
        if let Some(parsed) = deserialize_session_metadata(&raw) {
            merge_metadata_into(&mut session, parsed);
        }
    }
    Ok(session)
}

fn get_session_summary_conn(conn: &Connection, session_id: &str) -> Result<Option<SessionSummary>> {
    conn.query_row(
        r#"
        SELECT s.id, s.provider, s.project_path, s.title,
               COALESCE(m.message_count, s.message_count),
               s.last_activity, s.active, s.model, s.metadata
        FROM sessions s
        LEFT JOIN (
            SELECT session_id, COUNT(*) AS message_count
            FROM messages
            WHERE session_id = ?1
            GROUP BY session_id
        ) m ON m.session_id = s.id
        WHERE s.id = ?1
        "#,
        params![session_id],
        map_session_row,
    )
    .optional()
    .map_err(StorageError::from)
}

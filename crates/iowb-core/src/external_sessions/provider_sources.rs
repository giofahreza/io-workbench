#[cfg(test)]
fn discover_claude(home: &Path, records: &mut Vec<ExternalSessionRecord>) {
    let root = home.join(".claude/projects");
    for path in files_below(&root, 2, "jsonl") {
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("agent-"))
        {
            continue;
        }

        records.extend(discover_claude_file(&path));
    }
}

#[cfg(test)]
fn discover_claude_file(path: &Path) -> Vec<ExternalSessionRecord> {
    let fallback_time = modified_time(path);
    let mut sessions = HashMap::<String, SessionBuilder>::new();
    for_each_json_line(path, |entry| update_claude_builder(&mut sessions, entry));
    finish_claude_builders(sessions, path, fallback_time)
}

fn update_claude_builder(sessions: &mut HashMap<String, SessionBuilder>, entry: &Value) {
    if entry
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return;
    }
    let Some(session_id) = entry
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let builder = sessions
        .entry(session_id.to_string())
        .or_insert_with(|| SessionBuilder {
            id: session_id.to_string(),
            ..Default::default()
        });
    if builder.project_path.is_empty() {
        builder.project_path = entry
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    }
    if entry.get("type").and_then(Value::as_str) == Some("summary")
        && let Some(summary) = entry.get("summary").and_then(Value::as_str)
    {
        builder.title = Some(summary.to_string());
    }
    builder.last_activity = latest(
        builder.last_activity,
        value_timestamp(entry.get("timestamp")),
    );

    let Some(role) = entry
        .get("message")
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
    else {
        return;
    };
    let content = extract_text(
        entry
            .get("message")
            .and_then(|message| message.get("content")),
    );
    if content.is_empty() || (role == "user" && !is_visible_user_text(&content)) {
        return;
    }
    if matches!(role, "user" | "assistant") {
        builder.message_count += 1;
    }
    if role == "user" && builder.first_user.is_none() {
        builder.first_user = Some(content);
    }
    if role == "assistant" && builder.model.is_none() {
        builder.model = entry
            .get("message")
            .and_then(|message| message.get("model"))
            .and_then(Value::as_str)
            .map(str::to_string);
    }
}

fn finish_claude_builders(
    sessions: HashMap<String, SessionBuilder>,
    path: &Path,
    fallback_time: Option<DateTime<Utc>>,
) -> Vec<ExternalSessionRecord> {
    sessions
        .into_values()
        .filter_map(|builder| {
            finish_builder(builder, Provider::Claude, path.to_path_buf(), fallback_time)
        })
        .collect()
}

fn discover_codex(home: &Path, records: &mut Vec<ExternalSessionRecord>) {
    let root = home.join(".codex/sessions");
    if discover_codex_index(home, records) {
        return;
    }
    let mut candidates = Vec::new();
    for path in files_below(&root, 8, "jsonl") {
        let fallback_time = modified_time(&path);
        let mut builder = SessionBuilder::default();
        let mut last_visible: Option<(MessageRole, String)> = None;
        let mut subagent = false;
        let mut forked_from_id = None;
        let mut found_session_meta = false;
        for_each_json_line(&path, |entry| {
            let timestamp = value_timestamp(entry.get("timestamp"));
            builder.last_activity = latest(builder.last_activity, timestamp);
            match entry.get("type").and_then(Value::as_str) {
                Some("session_meta") => {
                    let payload = entry.get("payload").unwrap_or(&Value::Null);
                    if !found_session_meta && let Some(meta) = codex_session_meta(payload) {
                        builder.id = meta.id;
                        builder.project_path = meta.project_path;
                        builder.model = meta.model;
                        subagent = meta.subagent;
                        forked_from_id = meta.forked_from_id;
                        found_session_meta = true;
                    }
                }
                Some("event_msg") => {
                    let payload = entry.get("payload").unwrap_or(&Value::Null);
                    if payload.get("type").and_then(Value::as_str) != Some("user_message")
                        || payload
                            .get("kind")
                            .and_then(Value::as_str)
                            .is_some_and(|kind| kind != "plain")
                    {
                        return;
                    }
                    if let Some(content) = payload
                        .get("message")
                        .and_then(Value::as_str)
                        .map(visible_user_text)
                        .filter(|content| !content.is_empty())
                    {
                        record_visible_message(
                            &mut builder,
                            &mut last_visible,
                            MessageRole::User,
                            content,
                        );
                    }
                }
                Some("response_item") => {
                    let payload = entry.get("payload").unwrap_or(&Value::Null);
                    if payload.get("type").and_then(Value::as_str) != Some("message") {
                        return;
                    }
                    let role = match payload.get("role").and_then(Value::as_str) {
                        Some("user") => MessageRole::User,
                        Some("assistant") => MessageRole::Assistant,
                        _ => return,
                    };
                    let mut content = extract_text(payload.get("content"));
                    if role == MessageRole::User {
                        content = visible_user_text(&content);
                    }
                    if !content.is_empty() {
                        record_visible_message(&mut builder, &mut last_visible, role, content);
                    }
                }
                _ => {}
            }
        });
        if subagent {
            continue;
        }
        if builder.id.is_empty() {
            builder.id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(extract_uuid)
                .unwrap_or_default()
                .to_string();
        }
        if let Some(mut record) = finish_builder(builder, Provider::Codex, path, fallback_time) {
            record.summary.message_count = count_external_messages(&record);
            candidates.push((record, forked_from_id));
        }
    }
    records.extend(remove_resumed_codex_ancestors(candidates));
}

fn discover_codex_index(home: &Path, records: &mut Vec<ExternalSessionRecord>) -> bool {
    let codex_dir = home.join(".codex");
    let Some(database_path) = newest_matching_file(&codex_dir, "state_", "sqlite") else {
        return false;
    };
    let Ok(connection) = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return false;
    };
    let Ok(mut statement) = connection.prepare(
        r#"
        SELECT t.id, t.rollout_path, t.cwd, t.title, t.first_user_message,
               t.updated_at_ms, t.updated_at, t.model, t.thread_source,
               COUNT(*) OVER (
                   PARTITION BY t.cwd, t.first_user_message
               ) AS possible_resume_count
        FROM threads AS t
        WHERE t.archived = 0
          AND t.first_user_message <> ''
          AND LOWER(COALESCE(t.thread_source, '')) <> 'subagent'
          AND INSTR(LOWER(COALESCE(t.source, '')), '"subagent"') = 0
          AND NOT EXISTS (
              SELECT 1
              FROM thread_spawn_edges AS edge
              WHERE edge.child_thread_id = t.id
          )
        ORDER BY t.updated_at_ms DESC
        "#,
    ) else {
        return false;
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, i64>(9)?,
        ))
    }) else {
        return false;
    };

    let mut candidates = Vec::new();
    for row in rows.flatten() {
        let (
            id,
            rollout_path,
            project_path,
            title,
            first_user,
            updated_ms,
            updated,
            model,
            thread_source,
            possible_resume_count,
        ) = row;
        if is_codex_subagent_thread_source(thread_source.as_deref()) {
            continue;
        }
        if id.is_empty() || project_path.is_empty() || rollout_path.is_empty() {
            continue;
        }
        let file_path = PathBuf::from(rollout_path);
        if !file_path.is_file() {
            continue;
        }
        let forked_from_id = if possible_resume_count > 1 {
            first_codex_session_meta(&file_path)
                .filter(|meta| meta.id == id)
                .and_then(|meta| meta.forked_from_id)
        } else {
            None
        };
        let last_activity = updated_ms
            .and_then(DateTime::from_timestamp_millis)
            .or_else(|| DateTime::from_timestamp(updated, 0))
            .or_else(|| modified_time(&file_path))
            .unwrap_or_else(|| DateTime::from_timestamp(0, 0).expect("Unix epoch is valid"));
        let visible_title = [title, first_user]
            .into_iter()
            .find(|value| is_visible_user_text(value))
            .map(|value| summarize(&value))
            .unwrap_or_else(|| "Codex session".to_string());
        candidates.push((
            ExternalSessionRecord {
                summary: SessionSummary {
                    id,
                    provider: Provider::Codex,
                    external: true,
                    project_path,
                    title: visible_title,
                    // The Codex index does not store a message count. Keep discovery
                    // metadata-only; the messages endpoint loads the selected rollout
                    // lazily and returns its authoritative total.
                    message_count: 1,
                    last_activity,
                    active: false,
                    model,
                    last_message_at: Some(last_activity),
                    title_source: Some(SessionTitleSource::External),
                    ..Default::default()
                },
                file_path,
            },
            forked_from_id,
        ));
    }
    let discovered = remove_resumed_codex_ancestors(candidates);
    let found = !discovered.is_empty();
    records.extend(discovered);
    found
}

fn codex_session_meta(payload: &Value) -> Option<CodexSessionMeta> {
    let id = payload
        .get("id")
        .or_else(|| payload.get("session_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?
        .to_string();
    let forked_from_id = payload
        .get("forked_from_id")
        .or_else(|| payload.get("forkedFromId"))
        .and_then(Value::as_str)
        .filter(|parent_id| !parent_id.is_empty() && *parent_id != id.as_str())
        .map(str::to_string);
    Some(CodexSessionMeta {
        id,
        forked_from_id,
        project_path: payload
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        model: payload
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        subagent: is_codex_subagent_session_meta(payload),
    })
}

fn first_codex_session_meta(path: &Path) -> Option<CodexSessionMeta> {
    let file = File::open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok).take(16) {
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        if let Some(meta) = codex_session_meta(entry.get("payload").unwrap_or(&Value::Null)) {
            return Some(meta);
        }
    }
    None
}

fn remove_resumed_codex_ancestors(
    candidates: Vec<(ExternalSessionRecord, Option<String>)>,
) -> Vec<ExternalSessionRecord> {
    let candidate_ids = candidates
        .iter()
        .map(|(record, _)| record.summary.id.clone())
        .collect::<HashSet<_>>();
    let resumed_ancestor_ids = candidates
        .iter()
        .filter_map(|(_, parent_id)| parent_id.as_deref())
        .filter(|parent_id| candidate_ids.contains(*parent_id))
        .map(str::to_string)
        .collect::<HashSet<_>>();
    candidates
        .into_iter()
        .filter(|(record, _)| !resumed_ancestor_ids.contains(&record.summary.id))
        .map(|(record, _)| record)
        .collect()
}

fn is_codex_subagent_thread_source(thread_source: Option<&str>) -> bool {
    thread_source.is_some_and(|value| value.eq_ignore_ascii_case("subagent"))
}

fn is_codex_subagent_session_meta(payload: &Value) -> bool {
    is_codex_subagent_thread_source(payload.get("thread_source").and_then(Value::as_str))
        || payload
            .get("source")
            .and_then(|source| source.get("subagent"))
            .is_some()
}

#[cfg(test)]
fn discover_gemini(home: &Path, records: &mut Vec<ExternalSessionRecord>) {
    let root = home.join(".gemini/tmp");
    let Ok(project_dirs) = fs::read_dir(root) else {
        return;
    };
    for project_dir in project_dirs.flatten().filter(|entry| entry.path().is_dir()) {
        let project_path = fs::read_to_string(project_dir.path().join(".project_root"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if project_path.is_empty() {
            continue;
        }
        let chats_dir = project_dir.path().join("chats");
        let Ok(chat_files) = fs::read_dir(chats_dir) else {
            continue;
        };
        for chat_file in chat_files.flatten() {
            let path = chat_file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Some(record) = discover_gemini_file(&path, &project_path) {
                records.push(record);
            }
        }
    }
}

fn discover_gemini_file(path: &Path, project_path: &str) -> Option<ExternalSessionRecord> {
    let raw = fs::read_to_string(path).ok()?;
    let session = serde_json::from_str::<Value>(&raw).ok()?;
    let messages = session
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut builder = SessionBuilder {
        id: session
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_default(),
        project_path: project_path.to_string(),
        last_activity: value_timestamp(
            session
                .get("lastUpdated")
                .or_else(|| session.get("startTime")),
        ),
        model: session
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        ..Default::default()
    };
    for message in messages {
        let role = message
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let content = extract_text(message.get("content"));
        if content.is_empty() || (role == "user" && !is_visible_user_text(&content)) {
            continue;
        }
        if matches!(role, "user" | "gemini" | "assistant") {
            builder.message_count += 1;
        }
        if role == "user" && builder.first_user.is_none() {
            builder.first_user = Some(content);
        }
        builder.last_activity = latest(
            builder.last_activity,
            value_timestamp(message.get("timestamp")),
        );
    }
    finish_builder(
        builder,
        Provider::Gemini,
        path.to_path_buf(),
        modified_time(path),
    )
}

fn sync_gemini_sources(
    home: &Path,
    storage: &Storage,
    records: &mut Vec<ExternalSessionRecord>,
) -> iowb_storage::Result<()> {
    let root = home.join(".gemini/tmp");
    let mut retained = Vec::new();
    let Ok(project_dirs) = fs::read_dir(root) else {
        return Ok(());
    };
    for project_dir in project_dirs.flatten().filter(|entry| entry.path().is_dir()) {
        let project_path = fs::read_to_string(project_dir.path().join(".project_root"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if project_path.is_empty() {
            continue;
        }
        let Ok(chat_files) = fs::read_dir(project_dir.path().join("chats")) else {
            continue;
        };
        for path in chat_files.flatten().map(|entry| entry.path()) {
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(fingerprint) = external_file_fingerprint(&path) else {
                continue;
            };
            let source_path = path.display().to_string();
            retained.push(source_path.clone());
            if let Some(stored) = storage
                .external_history_source(Provider::Gemini, &source_path)?
                .filter(|source| source_matches(source, &fingerprint))
            {
                records.extend(restored_records(&stored));
                continue;
            }
            let discovered = discover_gemini_file(&path, &project_path)
                .into_iter()
                .collect::<Vec<_>>();
            persist_source(
                storage,
                Provider::Gemini,
                &path,
                &fingerprint,
                fingerprint.file_size,
                &discovered,
            )?;
            records.extend(discovered);
        }
    }
    storage.prune_external_history_sources(Provider::Gemini, &retained)
}

fn finish_builder(
    builder: SessionBuilder,
    provider: Provider,
    file_path: PathBuf,
    fallback_time: Option<DateTime<Utc>>,
) -> Option<ExternalSessionRecord> {
    if builder.id.is_empty() || builder.project_path.is_empty() || builder.message_count == 0 {
        return None;
    }
    let title = builder
        .title
        .or_else(|| builder.first_user.map(|message| summarize(&message)))
        .unwrap_or_else(|| format!("{} session", provider.as_str()));
    let last_activity = builder
        .last_activity
        .or(fallback_time)
        .unwrap_or_else(|| DateTime::from_timestamp(0, 0).expect("Unix epoch is valid"));
    Some(ExternalSessionRecord {
        summary: SessionSummary {
            id: builder.id,
            provider,
            external: true,
            project_path: builder.project_path,
            title,
            message_count: builder.message_count,
            last_activity,
            active: false,
            model: builder.model,
            last_message_at: Some(last_activity),
            title_source: Some(SessionTitleSource::External),
            ..Default::default()
        },
        file_path,
    })
}

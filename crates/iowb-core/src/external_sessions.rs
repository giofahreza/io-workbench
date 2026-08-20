use std::{
    collections::hash_map::DefaultHasher,
    collections::{HashMap, HashSet},
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use iowb_protocol::{ChatMessage, MessageRole, Provider, SessionSummary, SessionTitleSource};
use iowb_storage::{Storage, StoredExternalHistorySource, StoredExternalSessionRecord};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const MAX_EXTERNAL_TOOL_CONTENT_BYTES: usize = 128 * 1024;
const EXTERNAL_TOOL_CONTENT_TAIL_BYTES: usize = 32 * 1024;
pub(crate) const EXTERNAL_HISTORY_PARSER_VERSION: u32 = 1;
pub(crate) const EXTERNAL_MESSAGE_PARSER_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub(crate) struct ExternalSessionRecord {
    pub summary: SessionSummary,
    pub file_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalFileFingerprint {
    pub file_identity: Option<String>,
    pub file_size: u64,
    pub modified_nanos: Option<i64>,
}

pub(crate) fn external_file_fingerprint(path: &Path) -> Option<ExternalFileFingerprint> {
    let metadata = fs::metadata(path).ok()?;
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_nanos()).ok());
    #[cfg(unix)]
    let file_identity = Some(format!("{}:{}", metadata.dev(), metadata.ino()));
    #[cfg(not(unix))]
    let file_identity = None;
    Some(ExternalFileFingerprint {
        file_identity,
        file_size: metadata.len(),
        modified_nanos,
    })
}

#[derive(Default)]
struct SessionBuilder {
    id: String,
    project_path: String,
    title: Option<String>,
    first_user: Option<String>,
    message_count: usize,
    last_activity: Option<DateTime<Utc>>,
    model: Option<String>,
}

struct CodexSessionMeta {
    id: String,
    forked_from_id: Option<String>,
    project_path: String,
    model: Option<String>,
    subagent: bool,
}

struct CountedCodexMessage {
    role: MessageRole,
    fingerprint: u64,
    trimmed_len: usize,
    native_final: bool,
    io_workbench_live_transcript: bool,
}

#[cfg(test)]
pub(crate) fn discover_external_sessions(home: &Path) -> Vec<ExternalSessionRecord> {
    let mut records = Vec::new();
    discover_claude(home, &mut records);
    discover_codex(home, &mut records);
    discover_gemini(home, &mut records);

    let mut unique = HashMap::<(Provider, String), ExternalSessionRecord>::new();
    for record in records {
        let key = (record.summary.provider, record.summary.id.clone());
        match unique.get(&key) {
            Some(existing) if existing.summary.last_activity >= record.summary.last_activity => {}
            _ => {
                unique.insert(key, record);
            }
        }
    }
    let mut records = unique.into_values().collect::<Vec<_>>();
    records.sort_by_key(|record| std::cmp::Reverse(record.summary.last_activity));
    records
}

/// Discover provider histories through a durable metadata index. Unchanged
/// sources are served directly from SQLite; append-only Claude JSONL files are
/// resumed from their last complete byte offset.
pub(crate) fn sync_external_sessions(
    home: &Path,
    storage: &Storage,
) -> iowb_storage::Result<Vec<ExternalSessionRecord>> {
    let mut records = Vec::new();
    sync_claude_sources(home, storage, &mut records)?;
    sync_codex_sources(home, storage, &mut records)?;
    sync_gemini_sources(home, storage, &mut records)?;
    Ok(unique_external_records(records))
}

fn unique_external_records(records: Vec<ExternalSessionRecord>) -> Vec<ExternalSessionRecord> {
    let mut unique = HashMap::<(Provider, String), ExternalSessionRecord>::new();
    for record in records {
        let key = (record.summary.provider, record.summary.id.clone());
        match unique.get(&key) {
            Some(existing) if existing.summary.last_activity >= record.summary.last_activity => {}
            _ => {
                unique.insert(key, record);
            }
        }
    }
    let mut records = unique.into_values().collect::<Vec<_>>();
    records.sort_by_key(|record| std::cmp::Reverse(record.summary.last_activity));
    records
}

fn stored_records(records: &[ExternalSessionRecord]) -> Vec<StoredExternalSessionRecord> {
    records
        .iter()
        .map(|record| StoredExternalSessionRecord {
            summary: record.summary.clone(),
            file_path: record.file_path.display().to_string(),
        })
        .collect()
}

fn restored_records(source: &StoredExternalHistorySource) -> Vec<ExternalSessionRecord> {
    source
        .records
        .iter()
        .map(|record| ExternalSessionRecord {
            summary: record.summary.clone(),
            file_path: PathBuf::from(&record.file_path),
        })
        .collect()
}

fn source_matches(
    source: &StoredExternalHistorySource,
    fingerprint: &ExternalFileFingerprint,
) -> bool {
    source.parser_version == EXTERNAL_HISTORY_PARSER_VERSION
        && source.file_identity == fingerprint.file_identity
        && source.file_size == fingerprint.file_size
        && source.modified_nanos == fingerprint.modified_nanos
}

fn persist_source(
    storage: &Storage,
    provider: Provider,
    source_path: &Path,
    fingerprint: &ExternalFileFingerprint,
    scan_offset: u64,
    records: &[ExternalSessionRecord],
) -> iowb_storage::Result<()> {
    storage.upsert_external_history_source(&StoredExternalHistorySource {
        provider,
        source_path: source_path.display().to_string(),
        file_identity: fingerprint.file_identity.clone(),
        file_size: fingerprint.file_size,
        modified_nanos: fingerprint.modified_nanos,
        scan_offset,
        parser_version: EXTERNAL_HISTORY_PARSER_VERSION,
        records: stored_records(records),
    })
}

fn sync_claude_sources(
    home: &Path,
    storage: &Storage,
    records: &mut Vec<ExternalSessionRecord>,
) -> iowb_storage::Result<()> {
    let root = home.join(".claude/projects");
    if !root.is_dir() {
        return Ok(());
    }
    let paths = files_below(&root, 2, "jsonl")
        .into_iter()
        .filter(|path| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("agent-"))
        })
        .collect::<Vec<_>>();
    let retained = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    for path in paths {
        let Some(fingerprint) = external_file_fingerprint(&path) else {
            continue;
        };
        let source_path = path.display().to_string();
        let stored = storage.external_history_source(Provider::Claude, &source_path)?;
        if let Some(stored) = stored
            .as_ref()
            .filter(|source| source_matches(source, &fingerprint))
        {
            records.extend(restored_records(stored));
            continue;
        }

        let (discovered, scan_offset) = if let Some(stored) = stored.filter(|source| {
            source.parser_version == EXTERNAL_HISTORY_PARSER_VERSION
                && source.file_identity == fingerprint.file_identity
                && source.file_size <= fingerprint.file_size
                && source.scan_offset <= source.file_size
                && !source.records.is_empty()
        }) {
            let mut builders = stored
                .records
                .iter()
                .map(|record| {
                    let summary = &record.summary;
                    (
                        summary.id.clone(),
                        SessionBuilder {
                            id: summary.id.clone(),
                            project_path: summary.project_path.clone(),
                            title: Some(summary.title.clone()),
                            first_user: None,
                            message_count: summary.message_count,
                            last_activity: Some(summary.last_activity),
                            model: summary.model.clone(),
                        },
                    )
                })
                .collect::<HashMap<_, _>>();
            let offset = for_each_json_line_from(&path, stored.scan_offset, |entry| {
                update_claude_builder(&mut builders, entry)
            });
            (
                finish_claude_builders(builders, &path, modified_time(&path)),
                offset,
            )
        } else {
            let mut builders = HashMap::new();
            let offset = for_each_json_line_from(&path, 0, |entry| {
                update_claude_builder(&mut builders, entry)
            });
            (
                finish_claude_builders(builders, &path, modified_time(&path)),
                offset,
            )
        };
        persist_source(
            storage,
            Provider::Claude,
            &path,
            &fingerprint,
            scan_offset,
            &discovered,
        )?;
        records.extend(discovered);
    }
    storage.prune_external_history_sources(Provider::Claude, &retained)
}

fn sync_codex_sources(
    home: &Path,
    storage: &Storage,
    records: &mut Vec<ExternalSessionRecord>,
) -> iowb_storage::Result<()> {
    let codex_dir = home.join(".codex");
    let (source_path, fingerprint) =
        if let Some(index) = newest_matching_file(&codex_dir, "state_", "sqlite") {
            let fingerprint = codex_sqlite_fingerprint(&index);
            (index, fingerprint)
        } else {
            let root = codex_dir.join("sessions");
            (root.clone(), external_tree_fingerprint(&root, 8, "jsonl"))
        };
    let retained = vec![source_path.display().to_string()];
    if let Some(stored) = storage
        .external_history_source(Provider::Codex, &retained[0])?
        .filter(|source| source_matches(source, &fingerprint))
    {
        records.extend(restored_records(&stored));
        return storage.prune_external_history_sources(Provider::Codex, &retained);
    }

    let mut discovered = Vec::new();
    discover_codex(home, &mut discovered);
    persist_source(
        storage,
        Provider::Codex,
        &source_path,
        &fingerprint,
        fingerprint.file_size,
        &discovered,
    )?;
    records.extend(discovered);
    storage.prune_external_history_sources(Provider::Codex, &retained)
}

fn codex_sqlite_fingerprint(database_path: &Path) -> ExternalFileFingerprint {
    // SQLite may keep fresh thread metadata exclusively in WAL for a long
    // time, so the durable cursor must include both files.
    let wal_path = PathBuf::from(format!("{}-wal", database_path.display()));
    let mut hasher = DefaultHasher::new();
    let mut total_size = 0u64;
    let mut modified_nanos = None;
    for path in [database_path, wal_path.as_path()] {
        let Some(fingerprint) = external_file_fingerprint(path) else {
            continue;
        };
        path.hash(&mut hasher);
        fingerprint.file_identity.hash(&mut hasher);
        fingerprint.file_size.hash(&mut hasher);
        fingerprint.modified_nanos.hash(&mut hasher);
        total_size = total_size.saturating_add(fingerprint.file_size);
        modified_nanos = modified_nanos.max(fingerprint.modified_nanos);
    }
    ExternalFileFingerprint {
        file_identity: Some(format!("sqlite:{:016x}", hasher.finish())),
        file_size: total_size,
        modified_nanos,
    }
}

fn external_tree_fingerprint(
    root: &Path,
    max_depth: usize,
    extension: &str,
) -> ExternalFileFingerprint {
    let mut hasher = DefaultHasher::new();
    let mut total_size = 0u64;
    let mut modified_nanos = None;
    for path in files_below(root, max_depth, extension) {
        let Some(fingerprint) = external_file_fingerprint(&path) else {
            continue;
        };
        path.hash(&mut hasher);
        fingerprint.file_identity.hash(&mut hasher);
        fingerprint.file_size.hash(&mut hasher);
        fingerprint.modified_nanos.hash(&mut hasher);
        total_size = total_size.saturating_add(fingerprint.file_size);
        modified_nanos = modified_nanos.max(fingerprint.modified_nanos);
    }
    ExternalFileFingerprint {
        file_identity: Some(format!("tree:{:016x}", hasher.finish())),
        file_size: total_size,
        modified_nanos,
    }
}

pub(crate) fn load_external_messages(record: &ExternalSessionRecord) -> Vec<ChatMessage> {
    match record.summary.provider {
        Provider::Claude => load_claude_messages(record),
        Provider::Codex => load_codex_messages(record),
        Provider::Gemini => load_gemini_messages(record),
    }
}

pub(crate) fn count_external_messages(record: &ExternalSessionRecord) -> usize {
    match record.summary.provider {
        Provider::Codex => count_codex_messages(record),
        Provider::Claude | Provider::Gemini => record.summary.message_count,
    }
}

pub(crate) fn same_project_path(left: &str, right: &str) -> bool {
    let left_path = Path::new(left);
    let right_path = Path::new(right);
    match (left_path.canonicalize(), right_path.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => normalize_path(left) == normalize_path(right),
    }
}

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

fn load_claude_messages(record: &ExternalSessionRecord) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    for_each_json_line(&record.file_path, |entry| {
        if entry.get("sessionId").and_then(Value::as_str) != Some(record.summary.id.as_str())
            || entry
                .get("isSidechain")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            return;
        }
        let Some(role) = entry
            .get("message")
            .and_then(|message| message.get("role"))
            .and_then(Value::as_str)
            .and_then(parse_role)
        else {
            return;
        };
        let mut content = extract_text(
            entry
                .get("message")
                .and_then(|message| message.get("content")),
        );
        if role == MessageRole::User {
            content = visible_user_text(&content);
        }
        push_message(
            &mut messages,
            record,
            role,
            content,
            value_timestamp(entry.get("timestamp")),
        );
    });
    messages
}

fn load_codex_messages(record: &ExternalSessionRecord) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    let mut tool_names = HashMap::<String, String>::new();
    for_each_json_line(&record.file_path, |entry| {
        let timestamp = value_timestamp(entry.get("timestamp"));
        match entry.get("type").and_then(Value::as_str) {
            Some("event_msg") => {
                let payload = entry.get("payload").unwrap_or(&Value::Null);
                match payload.get("type").and_then(Value::as_str) {
                    Some("user_message") => {
                        if payload
                            .get("kind")
                            .and_then(Value::as_str)
                            .is_some_and(|kind| kind != "plain")
                        {
                            return;
                        }
                        let content = payload
                            .get("message")
                            .and_then(Value::as_str)
                            .map(visible_user_text)
                            .unwrap_or_default();
                        push_message(&mut messages, record, MessageRole::User, content, timestamp);
                    }
                    Some("task_complete") => {
                        let Some(error) = payload.get("error").filter(|error| !error.is_null())
                        else {
                            return;
                        };
                        let Some(detail) = codex_task_error_detail(error) else {
                            return;
                        };
                        push_codex_task_failure(&mut messages, record, detail, error, timestamp);
                    }
                    _ => {}
                }
            }
            Some("response_item") => {
                let payload = entry.get("payload").unwrap_or(&Value::Null);
                match payload.get("type").and_then(Value::as_str) {
                    Some("message") => {
                        let Some(role) = payload
                            .get("role")
                            .and_then(Value::as_str)
                            .and_then(parse_role)
                        else {
                            return;
                        };
                        let mut content = extract_text(payload.get("content"));
                        if role == MessageRole::User {
                            content = visible_user_text(&content);
                        }
                        push_message_with_metadata(
                            &mut messages,
                            record,
                            role,
                            content,
                            timestamp,
                            codex_response_message_metadata(payload),
                        );
                    }
                    Some("reasoning") => {
                        let content = extract_text(payload.get("summary"));
                        if !content.is_empty() {
                            push_message_with_metadata(
                                &mut messages,
                                record,
                                MessageRole::Assistant,
                                format!("thinking\n{content}"),
                                timestamp,
                                json!({
                                    "kind": "thinking",
                                    "thinkingSource": "summary",
                                }),
                            );
                        }
                    }
                    Some("function_call") => {
                        let name = payload
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool");
                        let call_id = payload
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if !call_id.is_empty() {
                            tool_names.insert(call_id.to_string(), name.to_string());
                        }
                        let arguments = payload
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        push_message_with_metadata(
                            &mut messages,
                            record,
                            MessageRole::Tool,
                            format_function_call(name, arguments),
                            timestamp,
                            tool_metadata("tool_use", name, call_id),
                        );
                    }
                    Some("function_call_output") => {
                        let call_id = payload
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let name = tool_names
                            .get(call_id)
                            .map(String::as_str)
                            .unwrap_or("tool");
                        let output = display_json_value(payload.get("output"));
                        push_message_with_metadata(
                            &mut messages,
                            record,
                            MessageRole::Tool,
                            format!("tool / Details\n**Tool:** `{name}`\n\n{output}"),
                            timestamp,
                            tool_metadata("tool_result", name, call_id),
                        );
                    }
                    Some("custom_tool_call") => {
                        let name = payload
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("custom_tool");
                        let call_id = payload
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if !call_id.is_empty() {
                            tool_names.insert(call_id.to_string(), name.to_string());
                        }
                        let input = payload
                            .get("input")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let (content, operations) = if name == "apply_patch" {
                            format_patch_tool(input)
                        } else {
                            (
                                format!(
                                    "tool / Parameters\n**Tool:** `{name}`\n\n{}",
                                    fenced_text(input)
                                ),
                                Vec::new(),
                            )
                        };
                        let mut metadata = tool_metadata("tool_use", name, call_id);
                        if !operations.is_empty() {
                            metadata["fileOperations"] = Value::Array(operations);
                        }
                        push_message_with_metadata(
                            &mut messages,
                            record,
                            MessageRole::Tool,
                            content,
                            timestamp,
                            metadata,
                        );
                    }
                    Some("custom_tool_call_output") => {
                        let call_id = payload
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let name = tool_names
                            .get(call_id)
                            .map(String::as_str)
                            .unwrap_or("custom_tool");
                        let output = display_json_value(payload.get("output"));
                        push_message_with_metadata(
                            &mut messages,
                            record,
                            MessageRole::Tool,
                            format!("tool / Details\n**Tool:** `{name}`\n\n{output}"),
                            timestamp,
                            tool_metadata("tool_result", name, call_id),
                        );
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    });
    deduplicate_adjacent(filter_legacy_codex_transcript_messages(messages))
}

fn count_codex_messages(record: &ExternalSessionRecord) -> usize {
    let mut messages = Vec::<CountedCodexMessage>::new();
    let mut tool_names = HashMap::<String, String>::new();
    for_each_json_line(&record.file_path, |entry| {
        match entry.get("type").and_then(Value::as_str) {
            Some("event_msg") => {
                let payload = entry.get("payload").unwrap_or(&Value::Null);
                match payload.get("type").and_then(Value::as_str) {
                    Some("user_message") => {
                        if payload
                            .get("kind")
                            .and_then(Value::as_str)
                            .is_some_and(|kind| kind != "plain")
                        {
                            return;
                        }
                        let content = payload
                            .get("message")
                            .and_then(Value::as_str)
                            .map(visible_user_text)
                            .unwrap_or_default();
                        push_counted_codex_message(
                            &mut messages,
                            MessageRole::User,
                            &content,
                            false,
                            false,
                        );
                    }
                    Some("task_complete") => {
                        let Some(error) = payload.get("error").filter(|error| !error.is_null())
                        else {
                            return;
                        };
                        if let Some(detail) = codex_task_error_detail(error) {
                            push_counted_codex_message(
                                &mut messages,
                                MessageRole::Assistant,
                                &format!("ERROR: {detail}"),
                                false,
                                false,
                            );
                        }
                    }
                    _ => {}
                }
            }
            Some("response_item") => {
                let payload = entry.get("payload").unwrap_or(&Value::Null);
                match payload.get("type").and_then(Value::as_str) {
                    Some("message") => {
                        let Some(role) = payload
                            .get("role")
                            .and_then(Value::as_str)
                            .and_then(parse_role)
                        else {
                            return;
                        };
                        let mut content = extract_text(payload.get("content"));
                        if role == MessageRole::User {
                            content = visible_user_text(&content);
                        }
                        push_counted_codex_message(
                            &mut messages,
                            role,
                            &content,
                            counted_native_codex_final_message(role, payload, &content),
                            counted_io_workbench_live_transcript(role, payload, &content),
                        );
                    }
                    Some("reasoning") => {
                        let content = extract_text(payload.get("summary"));
                        if !content.is_empty() {
                            push_counted_codex_message(
                                &mut messages,
                                MessageRole::Assistant,
                                &format!("thinking\n{content}"),
                                false,
                                false,
                            );
                        }
                    }
                    Some("function_call") => {
                        let name = payload
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool");
                        let call_id = payload
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if !call_id.is_empty() {
                            tool_names.insert(call_id.to_string(), name.to_string());
                        }
                        let arguments = payload
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        push_counted_tool_message(
                            &mut messages,
                            "function_call",
                            name,
                            call_id,
                            arguments,
                        );
                    }
                    Some("function_call_output") => {
                        let call_id = payload
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let name = tool_names
                            .get(call_id)
                            .map(String::as_str)
                            .unwrap_or("tool");
                        push_counted_tool_message(
                            &mut messages,
                            "function_call_output",
                            name,
                            call_id,
                            payload
                                .get("output")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        );
                    }
                    Some("custom_tool_call") => {
                        let name = payload
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("custom_tool");
                        let call_id = payload
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if !call_id.is_empty() {
                            tool_names.insert(call_id.to_string(), name.to_string());
                        }
                        let input = payload
                            .get("input")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        push_counted_tool_message(
                            &mut messages,
                            "custom_tool_call",
                            name,
                            call_id,
                            input,
                        );
                    }
                    Some("custom_tool_call_output") => {
                        let call_id = payload
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let name = tool_names
                            .get(call_id)
                            .map(String::as_str)
                            .unwrap_or("custom_tool");
                        push_counted_tool_message(
                            &mut messages,
                            "custom_tool_call_output",
                            name,
                            call_id,
                            payload
                                .get("output")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        );
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    });

    count_visible_codex_messages(messages)
}

fn push_counted_codex_message(
    messages: &mut Vec<CountedCodexMessage>,
    role: MessageRole,
    content: &str,
    native_final: bool,
    io_workbench_live_transcript: bool,
) {
    let content = content.trim();
    if content.is_empty() {
        return;
    }
    messages.push(CountedCodexMessage {
        role,
        fingerprint: text_fingerprint(content),
        trimmed_len: content.len(),
        native_final,
        io_workbench_live_transcript,
    });
}

fn push_counted_tool_message(
    messages: &mut Vec<CountedCodexMessage>,
    kind: &str,
    name: &str,
    call_id: &str,
    content_hint: &str,
) {
    let content_hint = content_hint.trim();
    let mut hasher = DefaultHasher::new();
    kind.hash(&mut hasher);
    name.hash(&mut hasher);
    call_id.hash(&mut hasher);
    content_hint.hash(&mut hasher);
    messages.push(CountedCodexMessage {
        role: MessageRole::Tool,
        fingerprint: hasher.finish(),
        trimmed_len: kind.len() + name.len() + call_id.len() + content_hint.len(),
        native_final: false,
        io_workbench_live_transcript: false,
    });
}

fn count_visible_codex_messages(messages: Vec<CountedCodexMessage>) -> usize {
    let mut turn = 0_usize;
    let mut message_turns = Vec::with_capacity(messages.len());
    let mut native_final_turns = HashSet::new();

    for message in &messages {
        if message.role == MessageRole::User {
            turn += 1;
        }
        message_turns.push(turn);
        if message.native_final {
            native_final_turns.insert(turn);
        }
    }

    let mut count = 0_usize;
    let mut previous: Option<(MessageRole, u64, usize)> = None;
    for (message, turn) in messages.into_iter().zip(message_turns) {
        if native_final_turns.contains(&turn) && message.io_workbench_live_transcript {
            continue;
        }
        let current = (message.role, message.fingerprint, message.trimmed_len);
        if previous.is_some_and(|previous| previous == current) {
            continue;
        }
        previous = Some(current);
        count += 1;
    }
    count
}

fn counted_native_codex_final_message(role: MessageRole, payload: &Value, content: &str) -> bool {
    if role != MessageRole::Assistant || counted_io_workbench_source(payload) {
        return false;
    }
    if payload.get("kind").and_then(Value::as_str) == Some("thinking")
        || payload.get("kind").and_then(Value::as_str) == Some("terminal_status")
        || payload.get("phase").and_then(Value::as_str) == Some("commentary")
    {
        return false;
    }
    let content = content.trim_start();
    !content.starts_with("thinking\n") && !content.starts_with("ERROR:")
}

fn counted_io_workbench_live_transcript(role: MessageRole, payload: &Value, content: &str) -> bool {
    role == MessageRole::Assistant
        && counted_io_workbench_source(payload)
        && looks_like_codex_live_transcript(content)
}

fn counted_io_workbench_source(payload: &Value) -> bool {
    payload
        .get("source")
        .and_then(Value::as_str)
        .is_some_and(|source| source.eq_ignore_ascii_case("io-workbench"))
}

fn text_fingerprint(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn codex_response_message_metadata(payload: &Value) -> Value {
    let mut metadata = serde_json::Map::new();
    for (source, target) in [
        ("id", "nativeMessageId"),
        ("phase", "phase"),
        ("source", "source"),
    ] {
        if let Some(value) = payload.get(source).filter(|value| !value.is_null()) {
            metadata.insert(target.to_string(), value.clone());
        }
    }
    Value::Object(metadata)
}

fn filter_legacy_codex_transcript_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut turn = 0_usize;
    let mut message_turns = Vec::with_capacity(messages.len());
    let mut native_final_turns = HashSet::new();

    for message in &messages {
        if message.role == MessageRole::User {
            turn += 1;
        }
        message_turns.push(turn);
        if is_native_codex_final_message(message) {
            native_final_turns.insert(turn);
        }
    }

    messages
        .into_iter()
        .zip(message_turns)
        .filter(|(message, turn)| {
            !(native_final_turns.contains(turn)
                && is_io_workbench_codex_message(message)
                && looks_like_codex_live_transcript(&message.content))
        })
        .map(|(message, _)| message)
        .collect()
}

fn is_native_codex_final_message(message: &ChatMessage) -> bool {
    if message.role != MessageRole::Assistant || is_io_workbench_codex_message(message) {
        return false;
    }
    if message.metadata.get("kind").and_then(Value::as_str) == Some("thinking")
        || message.metadata.get("kind").and_then(Value::as_str) == Some("terminal_status")
        || message.metadata.get("phase").and_then(Value::as_str) == Some("commentary")
    {
        return false;
    }
    let content = message.content.trim_start();
    !content.starts_with("thinking\n") && !content.starts_with("ERROR:")
}

fn is_io_workbench_codex_message(message: &ChatMessage) -> bool {
    message
        .metadata
        .get("source")
        .and_then(Value::as_str)
        .is_some_and(|source| source.eq_ignore_ascii_case("io-workbench"))
}

pub(crate) fn looks_like_codex_live_transcript(content: &str) -> bool {
    let mut has_thinking = false;
    let mut has_tool = false;
    let mut has_codex = false;
    let mut has_token_usage = false;

    for line in content.lines().map(str::trim) {
        match line {
            "thinking" => has_thinking = true,
            "codex" => has_codex = true,
            "tokens used" => has_token_usage = true,
            _ if line.ends_with(" / Parameters") || line.ends_with(" / Details") => {
                has_tool = true;
            }
            _ => {}
        }
    }

    (has_token_usage && (has_thinking || has_tool || has_codex))
        || (has_codex && (has_thinking || has_tool))
        || (content.len() >= 16 * 1024 && has_thinking && has_tool)
}

fn codex_task_error_detail(error: &Value) -> Option<String> {
    json_error_detail(error)
}

fn json_error_detail(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|parsed| json_error_detail(&parsed))
                .or_else(|| Some(text.to_string()))
        }
        Value::Object(values) => [
            "errorDetail",
            "error_detail",
            "detail",
            "message",
            "error",
            "reason",
        ]
        .into_iter()
        .find_map(|key| values.get(key).and_then(json_error_detail))
        .or_else(|| values.values().find_map(json_error_detail)),
        Value::Array(values) => values.iter().find_map(json_error_detail),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null => None,
    }
}

fn load_gemini_messages(record: &ExternalSessionRecord) -> Vec<ChatMessage> {
    let Ok(raw) = fs::read_to_string(&record.file_path) else {
        return Vec::new();
    };
    let Ok(session) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let mut messages = Vec::new();
    for message in session
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let role = match message.get("type").and_then(Value::as_str) {
            Some("user") => MessageRole::User,
            Some("gemini" | "assistant") => MessageRole::Assistant,
            _ => continue,
        };
        let mut content = extract_text(message.get("content"));
        if role == MessageRole::User {
            content = visible_user_text(&content);
        }
        push_message(
            &mut messages,
            record,
            role,
            content,
            value_timestamp(message.get("timestamp")),
        );
    }
    messages
}

fn push_message(
    messages: &mut Vec<ChatMessage>,
    record: &ExternalSessionRecord,
    role: MessageRole,
    content: String,
    timestamp: Option<DateTime<Utc>>,
) {
    push_message_with_metadata(messages, record, role, content, timestamp, Value::Null);
}

fn push_message_with_metadata(
    messages: &mut Vec<ChatMessage>,
    record: &ExternalSessionRecord,
    role: MessageRole,
    content: String,
    timestamp: Option<DateTime<Utc>>,
    extra_metadata: Value,
) {
    if content.trim().is_empty() {
        return;
    }
    let mut metadata = json!({
        "external": true,
        "cli": record.summary.provider.as_str(),
        "model": record.summary.model,
    });
    if let (Some(base), Some(extra)) = (metadata.as_object_mut(), extra_metadata.as_object()) {
        base.extend(extra.clone());
    }
    messages.push(ChatMessage {
        id: format!(
            "external_{}_{}_{}",
            record.summary.provider.as_str(),
            record.summary.id,
            messages.len()
        ),
        role,
        content,
        timestamp: timestamp.unwrap_or(record.summary.last_activity),
        metadata,
    });
}

fn push_codex_task_failure(
    messages: &mut Vec<ChatMessage>,
    record: &ExternalSessionRecord,
    detail: String,
    error: &Value,
    timestamp: Option<DateTime<Utc>>,
) {
    messages.push(ChatMessage {
        id: format!(
            "external_{}_{}_{}",
            record.summary.provider.as_str(),
            record.summary.id,
            messages.len()
        ),
        role: MessageRole::Assistant,
        content: format!("ERROR: {detail}"),
        timestamp: timestamp.unwrap_or(record.summary.last_activity),
        metadata: json!({
            "external": true,
            "cli": record.summary.provider.as_str(),
            "model": record.summary.model,
            "kind": "terminal_status",
            "status": "failed",
            "errorDetail": detail,
            "error": error,
        }),
    });
}

fn format_function_call(name: &str, arguments: &str) -> String {
    let parsed = serde_json::from_str::<Value>(arguments).ok();
    if matches!(name, "exec_command" | "shell_command") {
        let command = parsed
            .as_ref()
            .and_then(|value| value.get("cmd").or_else(|| value.get("command")))
            .and_then(Value::as_str)
            .unwrap_or(arguments);
        return bounded_tool_text(&format!(
            "tool / Parameters\n**Tool:** `{name}`\n\n### Command\n```sh\n{command}\n```"
        ));
    }
    let display = parsed
        .as_ref()
        .and_then(|value| serde_json::to_string_pretty(value).ok())
        .unwrap_or_else(|| arguments.to_string());
    bounded_tool_text(&format!(
        "tool / Parameters\n**Tool:** `{name}`\n\n{}",
        fenced_text(&display)
    ))
}

fn format_patch_tool(input: &str) -> (String, Vec<Value>) {
    let mut operations = Vec::new();
    for line in input.lines() {
        let trimmed = line.trim();
        let operation = [
            ("*** Add File: ", "create"),
            ("*** Update File: ", "update"),
            ("*** Delete File: ", "delete"),
            ("*** Move to: ", "move"),
        ]
        .into_iter()
        .find_map(|(prefix, kind)| trimmed.strip_prefix(prefix).map(|path| (kind, path.trim())));
        if let Some((kind, path)) = operation {
            operations.push(json!({"operation": kind, "path": path}));
        }
    }
    let summary = operations
        .iter()
        .filter_map(|operation| {
            Some(format!(
                "- **{}:** `{}`",
                operation
                    .get("operation")?
                    .as_str()?
                    .replace("create", "created")
                    .replace("update", "updated")
                    .replace("delete", "deleted")
                    .replace("move", "moved"),
                operation.get("path")?.as_str()?,
            ))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let content = bounded_tool_text(&format!(
        "apply_patch\n{}\n\n```diff\n{}\n```",
        summary,
        input.trim()
    ));
    (content, operations)
}

fn tool_metadata(kind: &str, name: &str, call_id: &str) -> Value {
    json!({
        "kind": kind,
        "toolName": name,
        "toolCallId": call_id,
    })
}

fn display_json_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => bounded_tool_text(text),
        Some(value) => {
            let sanitized = sanitize_inline_data_value(value);
            bounded_tool_text(
                &serde_json::to_string_pretty(&sanitized).unwrap_or_else(|_| sanitized.to_string()),
            )
        }
        None => String::new(),
    }
}

fn fenced_text(value: &str) -> String {
    bounded_tool_text(&format!("```json\n{}\n```", value.trim()))
}

fn sanitize_inline_data_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => {
            Value::Array(values.iter().map(sanitize_inline_data_value).collect())
        }
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), sanitize_inline_data_value(value)))
                .collect(),
        ),
        Value::String(value) => Value::String(omit_inline_data_urls(value)),
        value => value.clone(),
    }
}

fn bounded_tool_text(value: &str) -> String {
    let sanitized = omit_inline_data_urls(value);
    if sanitized.len() <= MAX_EXTERNAL_TOOL_CONTENT_BYTES {
        return sanitized;
    }

    let tail_start = floor_char_boundary(
        &sanitized,
        sanitized
            .len()
            .saturating_sub(EXTERNAL_TOOL_CONTENT_TAIL_BYTES),
    );
    let marker = format!(
        "\n\n[tool output truncated: {} bytes omitted]\n\n",
        tail_start
            .saturating_sub(MAX_EXTERNAL_TOOL_CONTENT_BYTES - EXTERNAL_TOOL_CONTENT_TAIL_BYTES,)
    );
    let head_budget = MAX_EXTERNAL_TOOL_CONTENT_BYTES
        .saturating_sub(EXTERNAL_TOOL_CONTENT_TAIL_BYTES)
        .saturating_sub(marker.len());
    let head_end = floor_char_boundary(&sanitized, head_budget);
    format!(
        "{}{}{}",
        &sanitized[..head_end],
        marker,
        &sanitized[tail_start..]
    )
}

fn omit_inline_data_urls(value: &str) -> String {
    let mut cursor = 0;
    let mut output: Option<String> = None;
    while let Some(relative_start) = value[cursor..].find("data:") {
        let start = cursor + relative_start;
        let header_end_limit = (start + 160).min(value.len());
        let header = &value[start..header_end_limit];
        let Some(marker_offset) = header.find(";base64,") else {
            cursor = start + "data:".len();
            continue;
        };
        let payload_start = start + marker_offset + ";base64,".len();
        let mut payload_end = payload_start;
        for byte in value.as_bytes()[payload_start..].iter().copied() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'\r' | b'\n') {
                payload_end += 1;
            } else {
                break;
            }
        }
        if payload_end == payload_start {
            cursor = payload_start;
            continue;
        }

        let output = output.get_or_insert_with(|| String::with_capacity(value.len().min(4096)));
        output.push_str(&value[cursor..start]);
        let mime = value[start + "data:".len()..start + marker_offset].trim();
        output.push_str(&format!(
            "[inline {} omitted: {} encoded bytes]",
            if mime.is_empty() { "data" } else { mime },
            payload_end - payload_start,
        ));
        cursor = payload_end;
    }

    match output {
        Some(mut output) => {
            output.push_str(&value[cursor..]);
            output
        }
        None => value.to_string(),
    }
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn record_visible_message(
    builder: &mut SessionBuilder,
    last_visible: &mut Option<(MessageRole, String)>,
    role: MessageRole,
    content: String,
) {
    if last_visible
        .as_ref()
        .is_some_and(|(last_role, last_content)| *last_role == role && *last_content == content)
    {
        return;
    }
    if role == MessageRole::User && builder.first_user.is_none() {
        builder.first_user = Some(content.clone());
    }
    builder.message_count += 1;
    *last_visible = Some((role, content));
}

fn deduplicate_adjacent(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut deduplicated = Vec::<ChatMessage>::new();
    for mut message in messages {
        if deduplicated.last().is_some_and(|last| {
            last.role == message.role && last.content.trim() == message.content.trim()
        }) {
            continue;
        }
        message.id = format!("{}_{}", message.id, deduplicated.len());
        deduplicated.push(message);
    }
    deduplicated
}

fn extract_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.as_str()
                    .map(str::to_string)
                    .or_else(|| part.get("text").and_then(Value::as_str).map(str::to_string))
            })
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

pub(crate) fn visible_user_text(text: &str) -> String {
    let mut candidate = text.trim();
    while let Some(tag) = [
        "system-reminder",
        "recommended_plugins",
        "environment_context",
    ]
    .into_iter()
    .find(|tag| candidate.starts_with(&format!("<{tag}>")))
    {
        let close = format!("</{tag}>");
        let Some(index) = candidate.find(&close) else {
            return String::new();
        };
        candidate = candidate[index + close.len()..].trim();
    }
    if is_visible_user_text(candidate) {
        candidate.to_string()
    } else {
        String::new()
    }
}

fn is_visible_user_text(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && ![
            "<command-name>",
            "<command-message>",
            "<command-args>",
            "<local-command-caveat>",
            "<local-command-stdout>",
            "<system-reminder>",
            "<recommended_plugins>",
            "<environment_context>",
            "# AGENTS.md instructions",
            "Caveat:",
            "This session is being continued from a previous",
            "[Request interrupted",
            "<turn_aborted>",
        ]
        .into_iter()
        .any(|prefix| text.starts_with(prefix))
}

fn parse_role(role: &str) -> Option<MessageRole> {
    match role {
        "user" => Some(MessageRole::User),
        "assistant" | "gemini" => Some(MessageRole::Assistant),
        _ => None,
    }
}

fn summarize(text: &str) -> String {
    const MAX_CHARS: usize = 72;
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= MAX_CHARS {
        normalized
    } else {
        format!(
            "{}...",
            normalized.chars().take(MAX_CHARS).collect::<String>()
        )
    }
}

fn value_timestamp(value: Option<&Value>) -> Option<DateTime<Utc>> {
    match value {
        Some(Value::String(raw)) => DateTime::parse_from_rfc3339(raw)
            .map(|timestamp| timestamp.with_timezone(&Utc))
            .ok(),
        Some(Value::Number(raw)) => raw.as_i64().and_then(|timestamp| {
            if timestamp > 10_000_000_000 {
                DateTime::from_timestamp_millis(timestamp)
            } else {
                DateTime::from_timestamp(timestamp, 0)
            }
        }),
        _ => None,
    }
}

fn latest(
    current: Option<DateTime<Utc>>,
    candidate: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match (current, candidate) {
        (Some(current), Some(candidate)) => Some(current.max(candidate)),
        (current, candidate) => current.or(candidate),
    }
}

fn modified_time(path: &Path) -> Option<DateTime<Utc>> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(DateTime::<Utc>::from)
}

fn for_each_json_line(path: &Path, mut visit: impl FnMut(&Value)) {
    let Ok(file) = File::open(path) else {
        return;
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            visit(&value);
        }
    }
}

/// Parse complete JSONL records beginning at a previously persisted boundary.
/// The returned offset always points immediately after a newline, so a partial
/// final write is retried safely on the next synchronization.
fn for_each_json_line_from(path: &Path, offset: u64, mut visit: impl FnMut(&Value)) -> u64 {
    let Ok(mut file) = File::open(path) else {
        return offset;
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return offset;
    }
    let mut reader = BufReader::new(file);
    let mut committed_offset = offset;
    loop {
        let mut bytes = Vec::new();
        let Ok(read) = reader.read_until(b'\n', &mut bytes) else {
            break;
        };
        if read == 0 {
            break;
        }
        if !bytes.ends_with(b"\n") {
            break;
        }
        committed_offset = committed_offset.saturating_add(read as u64);
        bytes.pop();
        if bytes.ends_with(b"\r") {
            bytes.pop();
        }
        if let Ok(line) = std::str::from_utf8(&bytes)
            && let Ok(value) = serde_json::from_str::<Value>(line)
        {
            visit(&value);
        }
    }
    committed_offset
}

fn files_below(root: &Path, max_depth: usize, extension: &str) -> Vec<PathBuf> {
    fn visit(dir: &Path, depth: usize, max_depth: usize, extension: &str, out: &mut Vec<PathBuf>) {
        if depth > max_depth {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, depth + 1, max_depth, extension, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some(extension) {
                out.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, 0, max_depth, extension, &mut files);
    files
}

fn newest_matching_file(root: &Path, prefix: &str, extension: &str) -> Option<PathBuf> {
    fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix))
                && path.extension().and_then(|ext| ext.to_str()) == Some(extension)
        })
        .max_by_key(|path| {
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        })
}

fn normalize_path(path: &str) -> String {
    let normalized = path.trim().trim_end_matches(['/', '\\']).replace('\\', "/");
    if cfg!(windows) {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}

fn extract_uuid(value: &str) -> Option<&str> {
    value
        .split('-')
        .collect::<Vec<_>>()
        .windows(5)
        .find_map(|parts| {
            let candidate = parts.join("-");
            let offset = value.find(&candidate)?;
            Uuid::parse_str(&candidate)
                .ok()
                .map(|_| &value[offset..offset + candidate.len()])
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use uuid::Uuid;

    #[test]
    fn visible_user_text_strips_codex_context_wrappers() {
        let hidden = concat!(
            "<recommended_plugins>\nplugins\n</recommended_plugins>\n",
            "<environment_context>\ncontext\n</environment_context>\n"
        );
        assert_eq!(visible_user_text(hidden), "");
        assert_eq!(
            visible_user_text(&format!("{hidden}\nActual prompt")),
            "Actual prompt"
        );
    }

    #[test]
    fn discovers_and_loads_all_supported_cli_histories() {
        let root = std::env::temp_dir().join(format!("iowb-external-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();

        let claude_id = "11111111-1111-4111-8111-111111111111";
        let claude_file = root
            .join(".claude/projects/test")
            .join(format!("{claude_id}.jsonl"));
        write_jsonl(
            &claude_file,
            &[
                json!({"type":"user","sessionId":claude_id,"cwd":project,"timestamp":"2026-07-29T10:00:00Z","message":{"role":"user","content":"Claude question"}}),
                json!({"type":"assistant","sessionId":claude_id,"cwd":project,"timestamp":"2026-07-29T10:00:01Z","message":{"role":"assistant","model":"claude-test","content":[{"type":"text","text":"Claude answer"}]}}),
            ],
        );

        let codex_id = "22222222-2222-4222-8222-222222222222";
        let codex_file = root
            .join(".codex/sessions/2026/07/29")
            .join(format!("rollout-2026-07-29T10-00-00-{codex_id}.jsonl"));
        write_jsonl(
            &codex_file,
            &[
                json!({"timestamp":"2026-07-29T10:01:00Z","type":"session_meta","payload":{"id":codex_id,"cwd":project}}),
                json!({"timestamp":"2026-07-29T10:01:01Z","type":"event_msg","payload":{"type":"user_message","message":"Codex question","kind":"plain"}}),
                json!({"timestamp":"2026-07-29T10:01:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Codex answer"}]}}),
            ],
        );

        let gemini_id = "33333333-3333-4333-8333-333333333333";
        let gemini_root = root.join(".gemini/tmp/project-hash");
        fs::create_dir_all(gemini_root.join("chats")).unwrap();
        fs::write(
            gemini_root.join(".project_root"),
            project.to_string_lossy().as_bytes(),
        )
        .unwrap();
        fs::write(
            gemini_root.join("chats").join(format!("{gemini_id}.json")),
            serde_json::to_vec(&json!({
                "sessionId": gemini_id,
                "lastUpdated": "2026-07-29T10:02:02Z",
                "messages": [
                    {"type":"user","timestamp":"2026-07-29T10:02:01Z","content":"Gemini question"},
                    {"type":"gemini","timestamp":"2026-07-29T10:02:02Z","content":[{"text":"Gemini answer"}]}
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let records = discover_external_sessions(&root);
        assert_eq!(records.len(), 3, "{records:#?}");
        for (provider, session_id, expected_question, expected_answer) in [
            (
                Provider::Claude,
                claude_id,
                "Claude question",
                "Claude answer",
            ),
            (Provider::Codex, codex_id, "Codex question", "Codex answer"),
            (
                Provider::Gemini,
                gemini_id,
                "Gemini question",
                "Gemini answer",
            ),
        ] {
            let record = records
                .iter()
                .find(|record| {
                    record.summary.provider == provider && record.summary.id == session_id
                })
                .unwrap();
            assert!(record.summary.external);
            assert!(same_project_path(
                &record.summary.project_path,
                project.to_str().unwrap()
            ));
            let messages = load_external_messages(record);
            assert_eq!(messages.len(), 2, "{messages:#?}");
            assert_eq!(
                record.summary.message_count,
                messages.len(),
                "summary count must match visible messages for {provider:?}",
            );
            assert_eq!(messages[0].content, expected_question);
            assert_eq!(messages[1].content, expected_answer);
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_claude_index_resumes_from_last_complete_jsonl_offset() {
        let root =
            std::env::temp_dir().join(format!("iowb-external-incremental-{}", Uuid::new_v4()));
        let project = root.join("project");
        let session_id = "44444444-4444-4444-8444-444444444444";
        let history = root
            .join(".claude/projects/test")
            .join(format!("{session_id}.jsonl"));
        fs::create_dir_all(history.parent().expect("history parent")).expect("history directory");
        fs::create_dir_all(&project).expect("project directory");
        write_jsonl(
            &history,
            &[
                json!({"type":"user","sessionId":session_id,"cwd":project,"timestamp":"2026-08-14T01:00:00Z","message":{"role":"user","content":"first"}}),
                json!({"type":"assistant","sessionId":session_id,"cwd":project,"timestamp":"2026-08-14T01:00:01Z","message":{"role":"assistant","content":"second"}}),
            ],
        );
        let database = root.join("index.db");
        let storage = Storage::open(&database).expect("storage");
        let initial = sync_external_sessions(&root, &storage).expect("initial sync");
        let initial = initial
            .iter()
            .find(|record| record.summary.id == session_id)
            .expect("initial record");
        assert_eq!(initial.summary.message_count, 2);
        drop(storage);

        let partial = format!(
            "{{\"type\":\"assistant\",\"sessionId\":\"{session_id}\",\"cwd\":\"{}\",",
            project.display()
        );
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&history)
            .expect("append partial");
        file.write_all(partial.as_bytes()).expect("partial line");
        drop(file);
        let storage = Storage::open(&database).expect("reopened storage");
        let partial_sync = sync_external_sessions(&root, &storage).expect("partial sync");
        assert_eq!(
            partial_sync
                .iter()
                .find(|record| record.summary.id == session_id)
                .expect("partial record")
                .summary
                .message_count,
            2,
        );

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&history)
            .expect("complete partial");
        file.write_all(
            b"\"timestamp\":\"2026-08-14T01:00:02Z\",\"message\":{\"role\":\"assistant\",\"content\":\"third\"}}\n",
        )
        .expect("complete line");
        drop(file);
        let completed = sync_external_sessions(&root, &storage).expect("completed sync");
        assert_eq!(
            completed
                .iter()
                .find(|record| record.summary.id == session_id)
                .expect("completed record")
                .summary
                .message_count,
            3,
        );

        drop(storage);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn ignores_internal_and_malformed_history_rows() {
        let root = std::env::temp_dir().join(format!("iowb-external-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let session_id = "44444444-4444-4444-8444-444444444444";
        let file = root
            .join(".codex/sessions")
            .join(format!("rollout-{session_id}.jsonl"));
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(
            &file,
            format!(
                "not-json\n{}\n{}\n",
                json!({"timestamp":"2026-07-29T10:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-07-29T10:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"<environment_context>internal</environment_context>","kind":"plain"}}),
            ),
        )
        .unwrap();

        assert!(discover_external_sessions(&root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_codex_reasoning_tools_and_patch_file_operations() {
        let root = std::env::temp_dir().join(format!("iowb-external-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let session_id = "44444444-4444-4444-8444-444444444444";
        let file = root
            .join(".codex/sessions/2026/07/30")
            .join(format!("rollout-2026-07-30T00-00-00-{session_id}.jsonl"));
        write_jsonl(
            &file,
            &[
                json!({"timestamp":"2026-07-30T00:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-07-30T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Change files","kind":"plain"}}),
                json!({"timestamp":"2026-07-30T00:00:02Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"Inspecting the project"}]}}),
                json!({"timestamp":"2026-07-30T00:00:03Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"call-exec","arguments":"{\"cmd\":\"pwd\"}"}}),
                json!({"timestamp":"2026-07-30T00:00:04Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call-exec","output":"Chunk ID: one\nProcess exited with code 0"}}),
                json!({"timestamp":"2026-07-30T00:00:05Z","type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","call_id":"call-patch","input":"*** Begin Patch\n*** Add File: created.txt\n+created\n*** Update File: updated.txt\n-old\n+new\n*** Delete File: deleted.txt\n*** Move to: moved.txt\n*** End Patch"}}),
                json!({"timestamp":"2026-07-30T00:00:06Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call-patch","output":"Success"}}),
                json!({"timestamp":"2026-07-30T00:00:07Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Finished"}]}}),
            ],
        );

        let record = discover_external_sessions(&root)
            .into_iter()
            .find(|record| record.summary.id == session_id)
            .unwrap();
        let messages = load_external_messages(&record);

        assert_eq!(7, messages.len(), "{messages:#?}");
        assert_eq!(7, record.summary.message_count);
        assert_eq!(MessageRole::Assistant, messages[1].role);
        assert!(messages[1].content.starts_with("thinking\n"));
        assert_eq!(MessageRole::Tool, messages[2].role);
        assert!(messages[2].content.contains("### Command"));
        assert_eq!(messages[2].metadata["toolName"], "exec_command");
        assert_eq!(MessageRole::Tool, messages[4].role);
        assert!(messages[4].content.contains("apply_patch"));
        assert!(messages[4].content.contains("created.txt"));
        assert!(messages[4].content.contains("updated.txt"));
        assert!(messages[4].content.contains("deleted.txt"));
        assert!(messages[4].content.contains("moved.txt"));
        assert_eq!(
            messages[4].metadata["fileOperations"]
                .as_array()
                .map(Vec::len),
            Some(4),
        );
        assert_eq!("Finished", messages[6].content);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_codex_task_failure_after_reasoning_as_terminal_assistant_message() {
        let root = std::env::temp_dir().join(format!("iowb-external-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let session_id = "88888888-8888-4888-8888-888888888888";
        let file = root
            .join(".codex/sessions/2026/08/11")
            .join(format!("rollout-2026-08-11T08-00-00-{session_id}.jsonl"));
        write_jsonl(
            &file,
            &[
                json!({"timestamp":"2026-08-11T08:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-08-11T08:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Run the full audit","kind":"plain"}}),
                json!({"timestamp":"2026-08-11T08:00:02Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"Inspecting tenant isolation"}]}}),
                json!({"timestamp":"2026-08-11T08:00:03Z","type":"event_msg","payload":{"type":"task_complete","last_agent_message":null,"error":{"message":"{\"detail\":\"The 'gpt-5.6-sol' model is not supported when using Codex with a ChatGPT account.\"}","codex_error_info":"other"}}}),
            ],
        );

        let record = discover_external_sessions(&root)
            .into_iter()
            .find(|record| record.summary.id == session_id)
            .unwrap();
        let messages = load_external_messages(&record);
        let terminal = messages.last().expect("terminal failure message");

        assert_eq!(3, messages.len(), "{messages:#?}");
        assert_eq!(MessageRole::Assistant, terminal.role);
        assert_eq!(terminal.metadata["kind"], "terminal_status");
        assert_eq!(terminal.metadata["status"], "failed");
        assert_eq!(
            terminal.metadata["errorDetail"],
            "The 'gpt-5.6-sol' model is not supported when using Codex with a ChatGPT account.",
        );
        assert!(terminal.content.starts_with("ERROR: The 'gpt-5.6-sol'"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hides_legacy_workbench_transcript_when_native_final_exists() {
        let root = std::env::temp_dir().join(format!("iowb-external-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let session_id = "99999999-9999-4999-8999-999999999999";
        let file = root
            .join(".codex/sessions/2026/08/11")
            .join(format!("rollout-2026-08-11T09-00-00-{session_id}.jsonl"));
        let transcript = format!(
            "thinking\n{}\n\nexec / Parameters\n**Tool:** `command_execution`\n\ncodex\nOnly the native final should remain.\n\ntokens used\n{{\"output_tokens\":12}}",
            "x".repeat(103_000)
        );
        write_jsonl(
            &file,
            &[
                json!({"timestamp":"2026-08-11T09:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-08-11T09:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Explain the fix","kind":"plain"}}),
                json!({"timestamp":"2026-08-11T09:00:02Z","type":"response_item","payload":{"type":"message","id":"msg-native","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"Only the native final should remain."}]}}),
                json!({"timestamp":"2026-08-11T09:00:03Z","type":"response_item","payload":{"type":"message","id":"msg-workbench","role":"assistant","source":"io-workbench","content":[{"type":"output_text","text":transcript}]}}),
            ],
        );

        let record = discover_external_sessions(&root)
            .into_iter()
            .find(|record| record.summary.id == session_id)
            .unwrap();
        let messages = load_external_messages(&record);

        assert_eq!(2, messages.len(), "{messages:#?}");
        assert_eq!(MessageRole::Assistant, messages[1].role);
        assert_eq!("Only the native final should remain.", messages[1].content);
        assert_eq!(Some("final_answer"), messages[1].metadata["phase"].as_str());
        assert_eq!(
            Some("msg-native"),
            messages[1].metadata["nativeMessageId"].as_str()
        );
        assert!(
            messages
                .iter()
                .all(|message| message.metadata["source"].as_str() != Some("io-workbench"))
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_structured_and_plain_codex_task_errors() {
        assert_eq!(
            codex_task_error_detail(&json!({
                "message": "{\"detail\":\"model unsupported\"}",
                "codex_error_info": "other",
            }))
            .as_deref(),
            Some("model unsupported"),
        );
        assert_eq!(
            codex_task_error_detail(&json!({
                "message": "This content was flagged for possible cybersecurity risk.",
                "codex_error_info": "cyber_policy",
            }))
            .as_deref(),
            Some("This content was flagged for possible cybersecurity risk."),
        );
    }

    #[test]
    fn omits_inline_tool_data_and_bounds_external_tool_output() {
        let root = std::env::temp_dir().join(format!("iowb-external-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let session_id = "77777777-7777-4777-8777-777777777777";
        let file = root
            .join(".codex/sessions/2026/08/01")
            .join(format!("rollout-2026-08-01T00-00-00-{session_id}.jsonl"));
        let image = format!("data:image/png;base64,{}", "A".repeat(300_000));
        let long_text = format!("{}TAIL", "B".repeat(180_000));
        write_jsonl(
            &file,
            &[
                json!({"timestamp":"2026-08-01T00:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-08-01T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Inspect images","kind":"plain"}}),
                json!({"timestamp":"2026-08-01T00:00:02Z","type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"call-image","input":"view image"}}),
                json!({"timestamp":"2026-08-01T00:00:03Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call-image","output":[{"type":"input_image","image_url":image},{"type":"input_text","text":long_text}]}}),
            ],
        );

        let record = discover_external_sessions(&root)
            .into_iter()
            .find(|record| record.summary.id == session_id)
            .unwrap();
        let messages = load_external_messages(&record);
        let tool_use = &messages[1];
        let tool_output = &messages[2];

        assert!(tool_use.metadata.get("payload").is_none());
        assert!(tool_output.metadata.get("payload").is_none());
        assert!(!tool_output.content.contains("data:image/png;base64"));
        assert!(tool_output.content.contains("inline image/png omitted"));
        assert!(tool_output.content.contains("tool output truncated"));
        assert!(tool_output.content.contains("TAIL"));
        assert!(tool_output.content.len() <= MAX_EXTERNAL_TOOL_CONTENT_BYTES + 128);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignores_codex_subagent_rollouts_in_json_fallback() {
        let root = std::env::temp_dir().join(format!("iowb-codex-subagent-{}", Uuid::new_v4()));
        let project = root.join("project");
        let sessions_dir = root.join(".codex/sessions/2026/08/11");
        let parent_id = "11111111-1111-4111-8111-111111111111";
        let subagent_id = "22222222-2222-4222-8222-222222222222";
        fs::create_dir_all(&project).unwrap();
        write_jsonl(
            &sessions_dir.join(format!("rollout-parent-{parent_id}.jsonl")),
            &[
                json!({"timestamp":"2026-08-11T00:00:00Z","type":"session_meta","payload":{"id":parent_id,"cwd":project,"thread_source":"user"}}),
                json!({"timestamp":"2026-08-11T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Visible parent","kind":"plain"}}),
            ],
        );
        write_jsonl(
            &sessions_dir.join(format!("rollout-subagent-{subagent_id}.jsonl")),
            &[
                json!({"timestamp":"2026-08-11T00:00:02Z","type":"session_meta","payload":{"id":subagent_id,"cwd":project,"thread_source":"subagent","source":{"subagent":{"thread_spawn":{"parent_thread_id":parent_id}}}}}),
                json!({"timestamp":"2026-08-11T00:00:03Z","type":"event_msg","payload":{"type":"user_message","message":"Hidden child","kind":"plain"}}),
            ],
        );

        let records = discover_external_sessions(&root);
        assert!(records.iter().any(|record| record.summary.id == parent_id));
        assert!(
            records
                .iter()
                .all(|record| record.summary.id != subagent_id)
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hides_resumed_codex_ancestors_in_json_fallback() {
        let root = std::env::temp_dir().join(format!("iowb-codex-resume-{}", Uuid::new_v4()));
        let project = root.join("project");
        let sessions_dir = root.join(".codex/sessions/2026/08/13");
        let parent_id = "11111111-1111-4111-8111-111111111111";
        let resumed_id = "22222222-2222-4222-8222-222222222222";
        let sibling_id = "33333333-3333-4333-8333-333333333333";
        fs::create_dir_all(&project).unwrap();
        write_jsonl(
            &sessions_dir.join(format!("rollout-parent-{parent_id}.jsonl")),
            &[
                json!({"timestamp":"2026-08-13T00:00:00Z","type":"session_meta","payload":{"id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Original question","kind":"plain"}}),
            ],
        );
        write_jsonl(
            &sessions_dir.join(format!("rollout-resumed-{resumed_id}.jsonl")),
            &[
                json!({"timestamp":"2026-08-13T00:01:00Z","type":"session_meta","payload":{"id":resumed_id,"forked_from_id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:01:00Z","type":"session_meta","payload":{"id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:01:01Z","type":"event_msg","payload":{"type":"user_message","message":"Original question","kind":"plain"}}),
                json!({"timestamp":"2026-08-13T00:01:02Z","type":"event_msg","payload":{"type":"user_message","message":"Continue the original chat","kind":"plain"}}),
            ],
        );
        write_jsonl(
            &sessions_dir.join(format!("rollout-sibling-{sibling_id}.jsonl")),
            &[
                json!({"timestamp":"2026-08-13T00:02:00Z","type":"session_meta","payload":{"id":sibling_id,"forked_from_id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:02:01Z","type":"event_msg","payload":{"type":"user_message","message":"Explore another branch","kind":"plain"}}),
            ],
        );

        let ids = discover_external_sessions(&root)
            .into_iter()
            .map(|record| record.summary.id)
            .collect::<HashSet<_>>();
        assert_eq!(
            HashSet::from([resumed_id.to_string(), sibling_id.to_string()]),
            ids,
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_index_discovery_defers_rollout_message_loading() {
        let root = std::env::temp_dir().join(format!("iowb-codex-index-{}", Uuid::new_v4()));
        let project = root.join("project");
        let codex_dir = root.join(".codex");
        let session_id = "55555555-5555-4555-8555-555555555555";
        let subagent_id = "66666666-6666-4666-8666-666666666666";
        let rollout = codex_dir
            .join("sessions/2026/07/31")
            .join(format!("rollout-{session_id}.jsonl"));
        let subagent_rollout = codex_dir
            .join("sessions/2026/07/31")
            .join(format!("rollout-{subagent_id}.jsonl"));
        fs::create_dir_all(&project).unwrap();
        write_jsonl(
            &rollout,
            &[
                json!({"timestamp":"2026-07-31T00:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-07-31T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Indexed question","kind":"plain"}}),
                json!({"timestamp":"2026-07-31T00:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Indexed answer"}]}}),
            ],
        );
        write_jsonl(
            &subagent_rollout,
            &[
                json!({"timestamp":"2026-07-31T00:00:03Z","type":"session_meta","payload":{"id":subagent_id,"cwd":project,"thread_source":"subagent","source":{"subagent":{"thread_spawn":{"parent_thread_id":session_id}}}}}),
                json!({"timestamp":"2026-07-31T00:00:04Z","type":"event_msg","payload":{"type":"user_message","message":"Indexed child","kind":"plain"}}),
            ],
        );

        let connection = Connection::open(codex_dir.join("state_5.sqlite")).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT NOT NULL,
                    cwd TEXT NOT NULL,
                    title TEXT NOT NULL,
                    first_user_message TEXT NOT NULL,
                    updated_at_ms INTEGER,
                    updated_at INTEGER NOT NULL,
                    model TEXT,
                    source TEXT NOT NULL DEFAULT 'exec',
                    thread_source TEXT,
                    archived INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE thread_spawn_edges (
                    parent_thread_id TEXT NOT NULL,
                    child_thread_id TEXT NOT NULL PRIMARY KEY,
                    status TEXT NOT NULL
                );
                "#,
            )
            .unwrap();
        connection
            .execute(
                r#"
                INSERT INTO threads (
                    id, rollout_path, cwd, title, first_user_message,
                    updated_at_ms, updated_at, model, thread_source, archived
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)
                "#,
                rusqlite::params![
                    session_id,
                    rollout.display().to_string(),
                    project.display().to_string(),
                    "Indexed session",
                    "Indexed question",
                    1_785_459_602_000_i64,
                    1_785_459_602_i64,
                    "gpt-test",
                    "user",
                ],
            )
            .unwrap();
        connection
            .execute(
                r#"
                INSERT INTO threads (
                    id, rollout_path, cwd, title, first_user_message,
                    updated_at_ms, updated_at, model, thread_source, archived
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)
                "#,
                rusqlite::params![
                    subagent_id,
                    subagent_rollout.display().to_string(),
                    project.display().to_string(),
                    "Indexed subagent",
                    "Indexed child",
                    1_785_459_603_000_i64,
                    1_785_459_603_i64,
                    "gpt-test",
                    Option::<String>::None,
                ],
            )
            .unwrap();
        connection
            .execute(
                r#"
                INSERT INTO thread_spawn_edges (
                    parent_thread_id, child_thread_id, status
                ) VALUES (?1, ?2, 'running')
                "#,
                rusqlite::params![session_id, subagent_id],
            )
            .unwrap();
        drop(connection);

        let records = discover_external_sessions(&root);
        let record = records
            .iter()
            .find(|record| record.summary.id == session_id)
            .unwrap();
        assert_eq!(1, record.summary.message_count);
        assert_eq!(2, load_external_messages(record).len());
        assert!(
            records
                .iter()
                .all(|record| record.summary.id != subagent_id)
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_index_hides_resumed_ancestors() {
        let root = std::env::temp_dir().join(format!("iowb-codex-index-resume-{}", Uuid::new_v4()));
        let project = root.join("project");
        let codex_dir = root.join(".codex");
        let parent_id = "11111111-1111-4111-8111-111111111111";
        let resumed_id = "22222222-2222-4222-8222-222222222222";
        let parent_rollout = codex_dir
            .join("sessions/2026/08/13")
            .join(format!("rollout-{parent_id}.jsonl"));
        let resumed_rollout = codex_dir
            .join("sessions/2026/08/13")
            .join(format!("rollout-{resumed_id}.jsonl"));
        fs::create_dir_all(&project).unwrap();
        write_jsonl(
            &parent_rollout,
            &[
                json!({"timestamp":"2026-08-13T00:00:00Z","type":"session_meta","payload":{"id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Original question","kind":"plain"}}),
            ],
        );
        write_jsonl(
            &resumed_rollout,
            &[
                json!({"timestamp":"2026-08-13T00:01:00Z","type":"session_meta","payload":{"id":resumed_id,"forked_from_id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:01:00Z","type":"session_meta","payload":{"id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:01:01Z","type":"event_msg","payload":{"type":"user_message","message":"Continue the original chat","kind":"plain"}}),
            ],
        );

        let connection = Connection::open(codex_dir.join("state_5.sqlite")).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT NOT NULL,
                    cwd TEXT NOT NULL,
                    title TEXT NOT NULL,
                    first_user_message TEXT NOT NULL,
                    updated_at_ms INTEGER,
                    updated_at INTEGER NOT NULL,
                    model TEXT,
                    source TEXT NOT NULL DEFAULT 'exec',
                    thread_source TEXT,
                    archived INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE thread_spawn_edges (
                    parent_thread_id TEXT NOT NULL,
                    child_thread_id TEXT NOT NULL PRIMARY KEY,
                    status TEXT NOT NULL
                );
                "#,
            )
            .unwrap();
        for (id, rollout, updated) in [
            (parent_id, &parent_rollout, 1_786_579_200_i64),
            (resumed_id, &resumed_rollout, 1_786_579_260_i64),
        ] {
            connection
                .execute(
                    r#"
                    INSERT INTO threads (
                        id, rollout_path, cwd, title, first_user_message,
                        updated_at_ms, updated_at, model, thread_source, archived
                    ) VALUES (?1, ?2, ?3, 'Same title', 'Original question', ?4, ?5, 'gpt-test', 'user', 0)
                    "#,
                    rusqlite::params![
                        id,
                        rollout.display().to_string(),
                        project.display().to_string(),
                        updated * 1_000,
                        updated,
                    ],
                )
                .unwrap();
        }
        drop(connection);

        let records = discover_external_sessions(&root);
        assert_eq!(1, records.len(), "{records:#?}");
        assert_eq!(resumed_id, records[0].summary.id);

        fs::remove_dir_all(root).unwrap();
    }

    fn write_jsonl(path: &Path, entries: &[Value]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let content = entries
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{content}\n")).unwrap();
    }
}

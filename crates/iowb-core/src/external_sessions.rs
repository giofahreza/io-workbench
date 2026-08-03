use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use iowb_protocol::{ChatMessage, MessageRole, Provider, SessionSummary};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use uuid::Uuid;

const MAX_EXTERNAL_TOOL_CONTENT_BYTES: usize = 128 * 1024;
const EXTERNAL_TOOL_CONTENT_TAIL_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct ExternalSessionRecord {
    pub summary: SessionSummary,
    pub file_path: PathBuf,
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

pub(crate) fn load_external_messages(record: &ExternalSessionRecord) -> Vec<ChatMessage> {
    match record.summary.provider {
        Provider::Claude => load_claude_messages(record),
        Provider::Codex => load_codex_messages(record),
        Provider::Gemini => load_gemini_messages(record),
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

        let fallback_time = modified_time(&path);
        let mut sessions = HashMap::<String, SessionBuilder>::new();
        for_each_json_line(&path, |entry| {
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
            let builder =
                sessions
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
            if entry.get("type").and_then(Value::as_str) == Some("summary") {
                if let Some(summary) = entry.get("summary").and_then(Value::as_str) {
                    builder.title = Some(summary.to_string());
                }
            }
            let timestamp = value_timestamp(entry.get("timestamp"));
            builder.last_activity = latest(builder.last_activity, timestamp);

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
        });

        for builder in sessions.into_values() {
            if let Some(record) =
                finish_builder(builder, Provider::Claude, path.clone(), fallback_time)
            {
                records.push(record);
            }
        }
    }
}

fn discover_codex(home: &Path, records: &mut Vec<ExternalSessionRecord>) {
    let root = home.join(".codex/sessions");
    if discover_codex_index(home, records) {
        return;
    }
    for path in files_below(&root, 8, "jsonl") {
        let fallback_time = modified_time(&path);
        let mut builder = SessionBuilder::default();
        let mut last_visible: Option<(MessageRole, String)> = None;
        for_each_json_line(&path, |entry| {
            let timestamp = value_timestamp(entry.get("timestamp"));
            builder.last_activity = latest(builder.last_activity, timestamp);
            match entry.get("type").and_then(Value::as_str) {
                Some("session_meta") => {
                    let payload = entry.get("payload").unwrap_or(&Value::Null);
                    builder.id = payload
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    builder.project_path = payload
                        .get("cwd")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    builder.model = payload
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_string);
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
        if builder.id.is_empty() {
            builder.id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(extract_uuid)
                .unwrap_or_default()
                .to_string();
        }
        if let Some(mut record) = finish_builder(builder, Provider::Codex, path, fallback_time) {
            record.summary.message_count = load_codex_messages(&record).len();
            records.push(record);
        }
    }
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
        SELECT id, rollout_path, cwd, title, first_user_message,
               updated_at_ms, updated_at, model
        FROM threads
        WHERE archived = 0 AND first_user_message <> ''
        ORDER BY updated_at_ms DESC
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
        ))
    }) else {
        return false;
    };

    let mut found = false;
    for row in rows.flatten() {
        let (id, rollout_path, project_path, title, first_user, updated_ms, updated, model) = row;
        if id.is_empty() || project_path.is_empty() || rollout_path.is_empty() {
            continue;
        }
        let file_path = PathBuf::from(rollout_path);
        if !file_path.is_file() {
            continue;
        }
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
        records.push(ExternalSessionRecord {
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
                ..Default::default()
            },
            file_path,
        });
        found = true;
    }
    found
}

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
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(session) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
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
                project_path: project_path.clone(),
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
            if let Some(record) = finish_builder(
                builder,
                Provider::Gemini,
                path.clone(),
                modified_time(&path),
            ) {
                records.push(record);
            }
        }
    }
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
                if payload.get("type").and_then(Value::as_str) != Some("user_message")
                    || payload
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
                        push_message(&mut messages, record, role, content, timestamp);
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
    deduplicate_adjacent(messages)
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

fn visible_user_text(text: &str) -> String {
    let mut candidate = text.trim();
    loop {
        let Some(tag) = ["system-reminder", "environment_context"]
            .into_iter()
            .find(|tag| candidate.starts_with(&format!("<{tag}>")))
        else {
            break;
        };
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
    use uuid::Uuid;

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
    fn codex_index_discovery_defers_rollout_message_loading() {
        let root = std::env::temp_dir().join(format!("iowb-codex-index-{}", Uuid::new_v4()));
        let project = root.join("project");
        let codex_dir = root.join(".codex");
        let session_id = "55555555-5555-4555-8555-555555555555";
        let rollout = codex_dir
            .join("sessions/2026/07/31")
            .join(format!("rollout-{session_id}.jsonl"));
        fs::create_dir_all(&project).unwrap();
        write_jsonl(
            &rollout,
            &[
                json!({"timestamp":"2026-07-31T00:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-07-31T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Indexed question","kind":"plain"}}),
                json!({"timestamp":"2026-07-31T00:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Indexed answer"}]}}),
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
                    archived INTEGER NOT NULL DEFAULT 0
                );
                "#,
            )
            .unwrap();
        connection
            .execute(
                r#"
                INSERT INTO threads (
                    id, rollout_path, cwd, title, first_user_message,
                    updated_at_ms, updated_at, model, archived
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0)
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
                ],
            )
            .unwrap();
        drop(connection);

        let record = discover_external_sessions(&root)
            .into_iter()
            .find(|record| record.summary.id == session_id)
            .unwrap();
        assert_eq!(1, record.summary.message_count);
        assert_eq!(2, load_external_messages(&record).len());

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

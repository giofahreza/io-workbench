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

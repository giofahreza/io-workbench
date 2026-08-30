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

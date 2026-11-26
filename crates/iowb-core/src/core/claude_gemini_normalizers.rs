#[derive(Default)]
struct ClaudeLiveOutputNormalizer {
    pending_line: String,
    pending_session_id: Option<String>,
    pending_init: Option<Value>,
    observed_session_id: Option<String>,
    streamed_text: String,
    streamed_thinking: bool,
    streamed_text_started: bool,
    saw_stream_event: bool,
    emitted_content: bool,
    active_tool: Option<ClaudeStreamingTool>,
    tool_names: HashMap<String, String>,
    emitted_tool_results: HashSet<String>,
    final_assistant_message: Option<String>,
    final_usage: Option<NormalizedRunUsage>,
    history_messages: Vec<ClaudeHistoryMessage>,
    history_bytes: usize,
    recorded_activity: HashSet<String>,
    pending_public_text: String,
    streamed_message_text: bool,
}

#[derive(Default, Clone)]
struct ClaudeStreamingTool {
    id: Option<String>,
    name: String,
    input_json: String,
}

struct ClaudeHistoryMessage {
    role: MessageRole,
    content: String,
    kind: &'static str,
    tool_name: String,
    call_id: Option<String>,
    source_hash: [u8; 32],
}

impl ClaudeLiveOutputNormalizer {
    fn push_chunks(&mut self, chunk: &str) -> Vec<String> {
        self.pending_line.push_str(chunk);
        let mut output = Vec::new();
        while let Some(newline) = self.pending_line.find('\n') {
            let line = self.pending_line[..newline]
                .trim_end_matches('\r')
                .to_string();
            self.pending_line.drain(..=newline);
            let chunk = self.normalize_line(&line);
            if !chunk.is_empty() {
                output.push(chunk);
            }
        }
        output
    }

    fn finish(&mut self) -> String {
        let mut output = String::new();
        if !self.pending_line.is_empty() {
            let line = std::mem::take(&mut self.pending_line);
            let chunk = self.normalize_line(line.trim_end_matches('\r'));
            if !chunk.is_empty() {
                output.push_str(&chunk);
            }
        }
        output
    }

    fn normalize_line(&mut self, line: &str) -> String {
        if line.trim().is_empty() {
            return String::new();
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return line.to_string();
        };
        if let Some(session_id) = event
            .get("session_id")
            .or_else(|| event.get("sessionId"))
            .and_then(Value::as_str)
            .filter(|session_id| !session_id.trim().is_empty())
            .filter(|session_id| self.observed_session_id.as_deref() != Some(*session_id))
        {
            self.observed_session_id = Some(session_id.to_string());
            self.pending_session_id = Some(session_id.to_string());
        }
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return String::new();
        };
        if event_type == "system" && event.get("subtype").and_then(Value::as_str) == Some("init") {
            let strings = |key: &str| {
                event.get(key).and_then(Value::as_array).into_iter().flatten()
                    .filter_map(Value::as_str).take(512).collect::<Vec<_>>()
            };
            let servers = event.get("mcp_servers").and_then(Value::as_array).into_iter().flatten()
                .take(64).map(|server| serde_json::json!({"name": server.get("name"), "status": server.get("status")}))
                .collect::<Vec<_>>();
            self.pending_init = event.get("slash_commands").and_then(Value::as_array).map(|_| {
                serde_json::json!({"names": strings("slash_commands"), "terminal": strings("terminal_slash_commands"), "mcpServers": servers})
            });
        }
        if event_type == "stream_event" {
            self.saw_stream_event = true;
            return event
                .get("event")
                .map(|stream_event| {
                    self.observe_history_event(stream_event);
                    self.normalize_event(stream_event)
                })
                .unwrap_or_default();
        }
        self.observe_history_event(&event);
        self.normalize_event(&event)
    }

    fn observe_history_event(&mut self, event: &Value) {
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "message_start" => {
                self.flush_public_text();
                self.streamed_message_text = false;
            }
            "content_block_delta" => {
                if let Some(text) = event
                    .get("delta")
                    .filter(|delta| delta.get("type").and_then(Value::as_str) == Some("text_delta"))
                    .and_then(|delta| delta.get("text"))
                    .and_then(Value::as_str)
                {
                    self.streamed_message_text = true;
                    self.pending_public_text.push_str(text);
                    self.pending_public_text = bound_agent_final_text(&self.pending_public_text);
                }
            }
            "content_block_start" => {
                if let Some(block) = event.get("content_block") {
                    match block.get("type").and_then(Value::as_str) {
                        Some("tool_use") => self.flush_public_text(),
                        Some("text") => {
                            if !self.pending_public_text.is_empty() {
                                self.pending_public_text.push('\n');
                            }
                            if let Some(text) = block
                                .get("text")
                                .and_then(Value::as_str)
                                .filter(|text| !text.is_empty())
                            {
                                self.streamed_message_text = true;
                                self.pending_public_text.push_str(text);
                                self.pending_public_text =
                                    bound_agent_final_text(&self.pending_public_text);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "content_block_stop" => {
                if let Some(tool) = self.active_tool.clone() {
                    let content = format_claude_streaming_tool(tool.clone());
                    self.record_history(
                        MessageRole::Tool,
                        "tool_call",
                        &tool.name,
                        tool.id,
                        content,
                    );
                }
            }
            "message_stop" | "result" => self.flush_public_text(),
            "assistant" | "user" => {
                let message = event.get("message").unwrap_or(event);
                let assistant = event.get("type").and_then(Value::as_str) == Some("assistant");
                if assistant && (!self.saw_stream_event || !self.streamed_message_text) {
                    if let Some(text) = extract_claude_assistant_text(message) {
                        self.record_history(MessageRole::Assistant, "progress", "", None, text);
                    }
                }
                if let Some(blocks) = message.get("content").and_then(Value::as_array) {
                    for block in blocks {
                        match block.get("type").and_then(Value::as_str) {
                            Some("tool_use") if assistant => self.record_history_tool(block, false),
                            Some("tool_result") => self.record_history_tool(block, true),
                            _ => {}
                        }
                    }
                }
            }
            "tool_use" => self.record_history_tool(event, false),
            "system" if event.get("subtype").and_then(Value::as_str) == Some("compact_boundary") => {
                self.record_history(MessageRole::System, "context_compaction", "", None,
                    "Claude compacted its native context. Earlier messages remain visible in Workbench.".into());
            }
            "tool_result" | "tool_use_result" => self.record_history_tool(event, true),
            _ => {}
        }
    }

    fn record_history_tool(&mut self, event: &Value, result: bool) {
        self.flush_public_text();
        let call_id = claude_tool_result_key(event);
        let name = if result {
            call_id
                .as_ref()
                .and_then(|id| self.tool_names.get(id))
                .cloned()
                .unwrap_or_else(|| claude_tool_name(event))
        } else {
            claude_tool_name(event)
        };
        let content = if result {
            format_claude_tool_result(event, &self.tool_names)
        } else {
            format_claude_tool_use(event, &mut self.tool_names)
        };
        self.record_history(
            MessageRole::Tool,
            if result { "tool_result" } else { "tool_call" },
            &name,
            call_id,
            content,
        );
    }

    fn flush_public_text(&mut self) {
        let text = std::mem::take(&mut self.pending_public_text);
        self.record_history(MessageRole::Assistant, "progress", "", None, text);
    }

    fn record_history(
        &mut self,
        role: MessageRole,
        kind: &'static str,
        name: &str,
        call_id: Option<String>,
        content: String,
    ) {
        if content.trim().is_empty()
            || self.history_messages.len() >= AGENT_TOOL_MESSAGES_MAX_COUNT
            || self.history_bytes >= AGENT_TOOL_MESSAGES_MAX_TOTAL_BYTES
        {
            return;
        }
        let source_hash: [u8; 32] = Sha256::digest(content.trim().as_bytes()).into();
        let content = bound_agent_text(
            &content,
            AGENT_TOOL_MESSAGE_MAX_BYTES
                .min(AGENT_TOOL_MESSAGES_MAX_TOTAL_BYTES - self.history_bytes),
            "activity",
        );
        let key = format!(
            "{kind}:{}",
            call_id.clone().unwrap_or_else(|| hex::encode(source_hash))
        );
        if !self.recorded_activity.insert(key) {
            return;
        }
        self.history_bytes += content.len();
        self.history_messages.push(ClaudeHistoryMessage {
            role,
            content,
            kind,
            tool_name: name.to_string(),
            call_id,
            source_hash,
        });
    }

    fn take_history_messages(
        &mut self,
        final_assistant: Option<&str>,
    ) -> Vec<ClaudeHistoryMessage> {
        self.flush_public_text();
        let mut messages = std::mem::take(&mut self.history_messages);
        if let Some(index) = messages
            .iter()
            .rposition(|message| message.role == MessageRole::Assistant)
            && final_assistant.is_some_and(|text| {
                <[u8; 32]>::from(Sha256::digest(text.trim().as_bytes()))
                    == messages[index].source_hash
            })
        {
            messages.remove(index);
        }
        messages
    }

    fn normalize_event(&mut self, event: &Value) -> String {
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return String::new();
        };
        match event_type {
            // The actual streamed text chunks.
            "content_block_delta" => {
                let delta_type = event
                    .get("delta")
                    .and_then(|delta| delta.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match delta_type {
                    "thinking_delta" => {
                        let thinking = event
                            .get("delta")
                            .and_then(|delta| delta.get("thinking"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if thinking.is_empty() {
                            return String::new();
                        }
                        let prefix = if self.streamed_thinking {
                            ""
                        } else {
                            self.streamed_thinking = true;
                            self.emitted_content = true;
                            "thinking\n"
                        };
                        format!("{prefix}{thinking}")
                    }
                    "text_delta" => {
                        let text = event
                            .get("delta")
                            .and_then(|delta| delta.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if text.is_empty() {
                            return String::new();
                        }
                        self.streamed_text.push_str(&text);
                        let prefix = if !self.streamed_text_started {
                            if self.streamed_thinking {
                                "\n\nclaude\n"
                            } else {
                                "claude\n"
                            }
                        } else {
                            ""
                        };
                        self.streamed_text_started = true;
                        self.emitted_content = true;
                        self.final_assistant_message =
                            Some(bound_agent_final_text(&self.streamed_text));
                        format!("{prefix}{text}")
                    }
                    "input_json_delta" => {
                        let partial_json = event
                            .get("delta")
                            .and_then(|delta| delta.get("partial_json"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if partial_json.is_empty() {
                            return String::new();
                        }
                        if let Some(tool) = self.active_tool.as_mut() {
                            tool.input_json.push_str(partial_json);
                        }
                        String::new()
                    }
                    _ => String::new(),
                }
            }
            "content_block_start" => event
                .get("content_block")
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
                .map(|block| {
                    let id = block.get("id").and_then(Value::as_str).map(str::to_string);
                    let name = claude_tool_name(block);
                    if let Some(id) = id.as_deref() {
                        self.tool_names.insert(id.to_string(), name.clone());
                    }
                    self.active_tool = Some(ClaudeStreamingTool {
                        id,
                        name,
                        input_json: block
                            .get("input")
                            .filter(|input| !input.is_null() && !is_empty_json_container(input))
                            .map(display_codex_live_value)
                            .unwrap_or_default(),
                    });
                    String::new()
                })
                .unwrap_or_default(),
            "content_block_stop" => self
                .active_tool
                .take()
                .filter(|tool| !tool.name.trim().is_empty())
                .map(format_claude_streaming_tool)
                .map(|section| self.format_activity_section(section))
                .unwrap_or_default(),
            "assistant" if !self.saw_stream_event => {
                let message = event.get("message").unwrap_or(event);
                if let Some(text) = extract_claude_assistant_text(message) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        self.final_assistant_message = Some(bound_agent_final_text(&text));
                    }
                }
                let section = format_claude_message_content(message, false, &mut self.tool_names);
                self.format_activity_section(section)
            }
            "user" if self.saw_stream_event => {
                let section = format_claude_message_tool_results(
                    event.get("message").unwrap_or(event),
                    &mut self.tool_names,
                    &mut self.emitted_tool_results,
                );
                self.format_activity_section(section)
            }
            "user" => {
                let section = format_claude_message_content(
                    event.get("message").unwrap_or(event),
                    true,
                    &mut self.tool_names,
                );
                self.format_activity_section(section)
            }
            "tool_use" => {
                let section = format_claude_tool_use(event, &mut self.tool_names);
                self.format_activity_section(section)
            }
            "tool_result" | "tool_use_result" => self.format_tool_result_once(event),
            // Lifecycle events we currently ignore but want to swallow
            // silently when the user has not asked for verbose noise.
            "message_start" | "message_delta" | "message_stop" | "ping" => String::new(),
            // Final result event with optional usage info.
            "result" => {
                let mut parts = Vec::new();
                if let Some(text) = event
                    .get("result")
                    .or_else(|| event.get("message"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    self.final_assistant_message = Some(bound_agent_final_text(text));
                    let remaining = text.strip_prefix(&self.streamed_text).unwrap_or(text);
                    let trimmed = remaining.trim();
                    if !trimmed.is_empty()
                        && (self.streamed_text.is_empty() || !remaining.is_empty())
                    {
                        parts.push(format!("claude\n{trimmed}"));
                    }
                }
                if event
                    .get("modelUsage")
                    .filter(|value| !value.is_null())
                    .is_some()
                    || event
                        .get("model_usage")
                        .filter(|value| !value.is_null())
                        .is_some()
                    || event
                        .get("usage")
                        .filter(|value| !value.is_null())
                        .is_some()
                {
                    self.final_usage = Some(normalize_claude_run_usage(event));
                }
                if let Some(usage) = event.get("usage").filter(|value| !value.is_null()) {
                    parts.push(self.format_activity_section(format!(
                        "tokens used\n{}",
                        serde_json::to_string_pretty(usage).unwrap_or_else(|_| usage.to_string())
                    )));
                }
                parts.join("\n\n")
            }
            // Tool use / progress noise: do not surface to the chat stream.
            "stream_request_start"
            | "stream_request_end"
            | "tool_use_request_start"
            | "tool_use_request_end"
            | "progress"
            | "error" => String::new(),
            _ => String::new(),
        }
    }

    fn take_session_id(&mut self) -> Option<String> {
        self.pending_session_id.take()
    }

    fn take_final_assistant_message(&mut self) -> Option<String> {
        self.final_assistant_message.take()
    }

    fn take_final_usage(&mut self) -> Option<NormalizedRunUsage> {
        self.final_usage.take()
    }

    fn format_activity_section(&mut self, section: String) -> String {
        let section = section.trim();
        if section.is_empty() {
            return String::new();
        }
        let prefix = if self.emitted_content { "\n\n" } else { "" };
        self.emitted_content = true;
        format!("{prefix}{section}")
    }

    fn format_tool_result_once(&mut self, event: &Value) -> String {
        if let Some(key) = claude_tool_result_key(event)
            && !self.emitted_tool_results.insert(key)
        {
            return String::new();
        }
        self.format_activity_section(format_claude_tool_result(event, &self.tool_names))
    }
}

fn is_empty_json_container(value: &Value) -> bool {
    value.as_object().is_some_and(|object| object.is_empty())
        || value.as_array().is_some_and(|array| array.is_empty())
}

fn claude_tool_name(event: &Value) -> String {
    event
        .get("name")
        .or_else(|| event.get("tool_name"))
        .or_else(|| event.get("toolName"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("tool")
        .to_string()
}

fn format_claude_streaming_tool(tool: ClaudeStreamingTool) -> String {
    let input = if tool.input_json.trim().is_empty() {
        "{}".to_string()
    } else {
        serde_json::from_str::<Value>(&tool.input_json)
            .map(|value| display_codex_live_value(&value))
            .unwrap_or(tool.input_json)
    };
    format_claude_tool_sections(&tool.name, tool.id.as_deref(), Some(&input), None)
}

pub(crate) fn format_claude_tool_use(
    event: &Value,
    tool_names: &mut HashMap<String, String>,
) -> String {
    let name = claude_tool_name(event);
    let id = event
        .get("id")
        .or_else(|| event.get("tool_use_id"))
        .or_else(|| event.get("toolUseId"))
        .and_then(Value::as_str);
    if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
        tool_names.insert(id.to_string(), name.clone());
    }
    let input = event
        .get("input")
        .or_else(|| event.get("arguments"))
        .or_else(|| event.get("args"))
        .filter(|value| !value.is_null())
        .map(display_codex_live_value);
    format_claude_tool_sections(&name, id, input.as_deref(), None)
}

pub(crate) fn format_claude_tool_result(
    event: &Value,
    tool_names: &HashMap<String, String>,
) -> String {
    let id = event
        .get("tool_use_id")
        .or_else(|| event.get("toolUseId"))
        .or_else(|| event.get("id"))
        .and_then(Value::as_str);
    let name = id
        .and_then(|id| tool_names.get(id))
        .cloned()
        .unwrap_or_else(|| claude_tool_name(event));
    let result = event
        .get("content")
        .or_else(|| event.get("result"))
        .or_else(|| event.get("output"))
        .or_else(|| event.get("error"))
        .map(display_codex_live_value);
    format_claude_tool_sections(&name, id, None, result.as_deref())
}

fn claude_tool_result_key(event: &Value) -> Option<String> {
    event
        .get("tool_use_id")
        .or_else(|| event.get("toolUseId"))
        .or_else(|| event.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(|id| id.trim().to_string())
}

fn format_claude_message_content(
    message: &Value,
    user_message: bool,
    tool_names: &mut HashMap<String, String>,
) -> String {
    let Some(content) = message.get("content") else {
        return String::new();
    };
    let mut output = String::new();
    let blocks: Vec<&Value> = content
        .as_array()
        .map(|items| items.iter().collect())
        .unwrap_or_else(|| vec![content]);
    for block in blocks {
        match block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "text" if !user_message => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        append_live_section(&mut output, &format!("claude\n{trimmed}"));
                    }
                }
            }
            "thinking" | "thinking_delta" if !user_message => {
                let thinking = block
                    .get("thinking")
                    .or_else(|| block.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !thinking.trim().is_empty() {
                    append_live_section(&mut output, &format!("thinking\n{thinking}"));
                }
            }
            "tool_use" if !user_message => {
                append_live_section(&mut output, &format_claude_tool_use(block, tool_names));
            }
            "tool_result" => {
                append_live_section(&mut output, &format_claude_tool_result(block, tool_names));
            }
            _ => {}
        }
    }
    output.trim().to_string()
}

fn extract_claude_assistant_text(message: &Value) -> Option<String> {
    let content = message.get("content")?;
    let blocks: Vec<&Value> = content
        .as_array()
        .map(|items| items.iter().collect())
        .unwrap_or_else(|| vec![content]);
    let mut combined = String::new();
    for block in blocks {
        let block_type = block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if block_type == "text" {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(text);
            }
        }
    }
    if combined.trim().is_empty() {
        None
    } else {
        Some(combined)
    }
}

fn format_claude_message_tool_results(
    message: &Value,
    tool_names: &mut HashMap<String, String>,
    emitted_tool_results: &mut HashSet<String>,
) -> String {
    let Some(content) = message.get("content") else {
        return String::new();
    };
    let mut output = String::new();
    let blocks: Vec<&Value> = content
        .as_array()
        .map(|items| items.iter().collect())
        .unwrap_or_else(|| vec![content]);
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        if let Some(key) = claude_tool_result_key(block)
            && !emitted_tool_results.insert(key)
        {
            continue;
        }
        append_live_section(&mut output, &format_claude_tool_result(block, tool_names));
    }
    output.trim().to_string()
}

fn format_claude_tool_sections(
    name: &str,
    id: Option<&str>,
    input: Option<&str>,
    result: Option<&str>,
) -> String {
    if is_claude_command_tool(name) {
        return format_claude_command_sections(name, id, input, result);
    }
    let mut content = String::new();
    if input.is_some() || result.is_none() {
        content.push_str("tool / Parameters\n");
        content.push_str(&format!("**Tool:** `{}`", name.trim()));
        if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
            content.push_str(&format!("\n- **ID:** `{}`", id.trim()));
        }
        if let Some(input) = input.filter(|input| !input.trim().is_empty()) {
            content.push_str("\n\n### Input\n```json\n");
            content.push_str(input.trim());
            content.push_str("\n```");
        }
    }
    if let Some(result) = result.filter(|result| !result.trim().is_empty()) {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str("tool / Details\n");
        content.push_str(&format!("**Tool:** `{}`", name.trim()));
        if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
            content.push_str(&format!("\n- **ID:** `{}`", id.trim()));
        }
        content.push_str("\n\n```text\n");
        content.push_str(result.trim());
        content.push_str("\n```");
    }
    bound_agent_text(&content, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output")
}

fn is_claude_command_tool(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "bash" | "shell" | "sh" | "exec" | "command" | "shell_command" | "exec_command"
    )
}

fn format_claude_command_sections(
    name: &str,
    id: Option<&str>,
    input: Option<&str>,
    result: Option<&str>,
) -> String {
    let mut content = String::new();
    if input.is_some() || result.is_none() {
        content.push_str("exec / Parameters\n");
        content.push_str(&format!("**Tool:** `{}`", name.trim()));
        if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
            content.push_str(&format!("\n- **ID:** `{}`", id.trim()));
        }
        if let Some(command) = input.and_then(claude_command_input_shell) {
            content.push_str("\n\n### Command\n```sh\n");
            content.push_str(command.trim());
            content.push_str("\n```");
        } else if let Some(input) = input.filter(|input| !input.trim().is_empty()) {
            content.push_str("\n\n```json\n");
            content.push_str(input.trim());
            content.push_str("\n```");
        }
    }
    if let Some(result) = result.filter(|result| !result.trim().is_empty()) {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str("exec / Details\n");
        content.push_str(&format!("**Tool:** `{}`", name.trim()));
        if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
            content.push_str(&format!("\n- **ID:** `{}`", id.trim()));
        }
        content.push_str("\n\n```text\n");
        content.push_str(result.trim());
        content.push_str("\n```");
    }
    bound_agent_text(&content, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output")
}

fn claude_command_input_shell(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(trimmed)
        .ok()
        .and_then(|value| {
            value
                .get("command")
                .or_else(|| value.get("cmd"))
                .or_else(|| value.get("script"))
                .and_then(Value::as_str)
                .filter(|command| !command.trim().is_empty())
                .map(str::to_string)
        })
}

impl GeminiLiveOutputNormalizer {
    fn push(&mut self, chunk: &str) -> String {
        self.pending_line.push_str(chunk);
        let mut output = String::new();
        while let Some(newline) = self.pending_line.find('\n') {
            let line = self.pending_line[..newline]
                .trim_end_matches('\r')
                .to_string();
            self.pending_line.drain(..=newline);
            output.push_str(&self.normalize_line(&line));
        }
        output
    }

    fn finish(&mut self) -> String {
        if self.pending_line.is_empty() {
            return String::new();
        }
        let line = std::mem::take(&mut self.pending_line);
        self.normalize_line(line.trim_end_matches('\r'))
    }

    fn normalize_line(&mut self, line: &str) -> String {
        if line.trim().is_empty() {
            return String::new();
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return line.to_string();
        };
        if let Some(session_id) = event
            .get("session_id")
            .or_else(|| event.get("sessionId"))
            .or_else(|| event.pointer("/session/id"))
            .and_then(Value::as_str)
            .filter(|session_id| !session_id.trim().is_empty())
        {
            self.pending_session_id = Some(session_id.to_string());
        }

        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "result" => {
                if let Some(usage) = normalize_gemini_run_usage(&event) {
                    self.final_usage = Some(usage);
                }
                String::new()
            }
            "message" | "content" => {
                let role = event
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("assistant");
                if !matches!(role, "assistant" | "model" | "gemini") {
                    return String::new();
                }
                event
                    .get("content")
                    .and_then(|content| {
                        content
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| collect_direct_ai_text(Some(content)))
                    })
                    .or_else(|| {
                        event
                            .get("text")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default()
            }
            "error" => event
                .get("message")
                .or_else(|| event.get("error"))
                .map(display_codex_live_value)
                .filter(|message| !message.is_empty())
                .map(|message| format!("ERROR: {message}\n"))
                .unwrap_or_default(),
            // init/result/tool lifecycle records carry metadata but no new
            // assistant delta that belongs in the visible chat bubble.
            _ => String::new(),
        }
    }

    fn take_session_id(&mut self) -> Option<String> {
        self.pending_session_id.take()
    }

    fn take_final_usage(&mut self) -> Option<NormalizedRunUsage> {
        self.final_usage.take()
    }
}

fn append_live_section(output: &mut String, section: &str) {
    let section = section.trim();
    if section.is_empty() {
        return;
    }
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str(section);
    output.push('\n');
}

fn format_codex_live_command(item: &Value) -> String {
    let command = item
        .get("command")
        .map(display_codex_live_value)
        .unwrap_or_default();
    let result = item
        .get("aggregated_output")
        .or_else(|| item.get("output"))
        .map(display_codex_live_value)
        .unwrap_or_default();
    let exit_code = item.get("exit_code").and_then(Value::as_i64);
    let mut content = format!(
        "exec / Parameters\n**Tool:** `command_execution`\n\n### Command\n```sh\n{}\n```",
        command.trim()
    );
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    append_live_section(
        &mut content,
        &format!(
            "exec / Details\n- **Status:** `{status}`\n- **Exit code:** `{}`\n\n```text\n{}\n```",
            exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "-".to_string()),
            result.trim()
        ),
    );
    bound_agent_text(&content, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output")
}

fn format_codex_live_file_change(item: &Value) -> String {
    let changes = item
        .get("changes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut content = String::new();
    for change in &changes {
        let path = change
            .get("path")
            .or_else(|| change.get("file_path"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let kind = change
            .get("kind")
            .or_else(|| change.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("update");
        let operation = match kind.to_ascii_lowercase().as_str() {
            "add" | "create" | "created" => "create",
            "delete" | "deleted" | "remove" => "delete",
            "move" | "moved" | "rename" | "renamed" => "move",
            _ => "edit",
        };
        append_live_section(&mut content, &format!("{operation} / {path}"));
    }
    if content.is_empty() {
        content.push_str("file_change / Details\n");
    }
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    append_live_section(&mut content, &format!("- **Status:** `{status}`"));
    bound_agent_text(&content, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output")
}

fn format_codex_live_function_call(item: &Value) -> String {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("function_call");
    if matches!(name, "exec_command" | "shell_command") {
        let arguments = item
            .get("arguments")
            .map(display_codex_live_value)
            .unwrap_or_default();
        let parsed = serde_json::from_str::<Value>(&arguments).ok();
        let command = parsed
            .as_ref()
            .and_then(|value| value.get("cmd").or_else(|| value.get("command")))
            .and_then(Value::as_str)
            .unwrap_or(&arguments);
        return bound_agent_text(
            &format!("exec / Parameters\n**Tool:** `{name}`\n\n### Command\n```sh\n{command}\n```"),
            AGENT_TOOL_MESSAGE_MAX_BYTES,
            "tool output",
        );
    }
    format_codex_live_named_tool(name, item.get("arguments"), None)
}

fn format_codex_live_custom_tool(item: &Value) -> String {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("custom_tool_call");
    let input = item
        .get("input")
        .map(display_codex_live_value)
        .unwrap_or_default();
    if name != "apply_patch" {
        return format_codex_live_named_tool(name, Some(&Value::String(input)), None);
    }

    let mut content = String::from("apply_patch");
    for line in input.lines() {
        let trimmed = line.trim();
        let operation = [
            ("*** Add File: ", "create"),
            ("*** Update File: ", "edit"),
            ("*** Delete File: ", "delete"),
            ("*** Move to: ", "move"),
        ]
        .into_iter()
        .find_map(|(prefix, kind)| trimmed.strip_prefix(prefix).map(|path| (kind, path)));
        if let Some((kind, path)) = operation {
            append_live_section(&mut content, &format!("{kind} / {}", path.trim()));
        }
    }
    append_live_section(&mut content, &format!("```diff\n{}\n```", input.trim()));
    bound_agent_text(&content, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output")
}

fn format_codex_live_tool_result(item: &Value, fallback_name: &str) -> String {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(fallback_name);
    format_codex_live_named_tool(
        name,
        None,
        item.get("output").or_else(|| item.get("result")),
    )
}

fn format_codex_live_named_tool(
    name: &str,
    input: Option<&Value>,
    result: Option<&Value>,
) -> String {
    let mut content = format!("tool / Parameters\n**Tool:** `{name}`");
    if let Some(input) = input {
        append_live_section(
            &mut content,
            &format!("```json\n{}\n```", display_codex_live_value(input).trim()),
        );
    }
    if let Some(result) = result {
        append_live_section(
            &mut content,
            &format!(
                "tool / Details\n```text\n{}\n```",
                display_codex_live_value(result).trim()
            ),
        );
    }
    bound_agent_text(&content, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output")
}

fn display_codex_live_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        value => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
    }
}

fn spawn_agent_output_reader<R>(
    tx: mpsc::Sender<AgentProcessEvent>,
    reader: R,
    stream: AgentOutputStream,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut reader = reader;
        let mut buffer = vec![0_u8; 8192];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    if tx
                        .send(AgentProcessEvent::Output {
                            stream,
                            data: String::from_utf8_lossy(&buffer[..read]).into_owned(),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => {
                    let _ = tx.send(AgentProcessEvent::Failed(error.to_string())).await;
                    break;
                }
            }
        }
    });
}

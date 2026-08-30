impl CodexAppServerLiveOutputNormalizer {
    fn push_notification(&mut self, method: &str, params: &Value) -> String {
        if method == "thread/tokenUsage/updated" {
            return self.normalize_token_usage(params);
        }
        let visible = match method {
            "item/agentMessage/delta" => self.push_agent_message_delta(params),
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                self.push_reasoning_delta(params)
            }
            "item/commandExecution/outputDelta" => self.push_command_output_delta(params),
            "item/completed" => {
                let item = params.get("item").unwrap_or(params);
                self.normalize_completed_item(item)
            }
            "turn/completed" => self.normalize_turn_completed(params),
            "turn/plan/updated" => self.normalize_plan(params),
            "turn/diff/updated" => self.normalize_diff(params),
            "error" => self.normalize_error(params),
            "warning" | "configWarning" => self.normalize_warning(params),
            "item/started"
            | "turn/started"
            | "thread/status/changed"
            | "serverRequest/resolved"
            | "thread/settings/updated" => String::new(),
            _ => String::new(),
        };
        if !visible.trim().is_empty() {
            self.emitted_visible_turn_output = true;
        }
        visible
    }

    fn finish(&mut self) -> String {
        self.take_pending_agent_message(false)
    }

    fn push_agent_message_delta(&mut self, params: &Value) -> String {
        let delta = params
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if delta.is_empty() {
            return String::new();
        }
        let item_id = app_server_item_id(params).unwrap_or_else(|| "agent".to_string());
        self.streamed_agent_items.insert(item_id.clone());
        let entry = self.streamed_agent_text.entry(item_id).or_default();
        entry.push_str(delta);
        self.final_assistant_message = Some(bound_agent_text(
            entry.trim(),
            AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
            "assistant response",
        ));
        let prefix = if self.emitted_agent_stream {
            ""
        } else if self.emitted_reasoning_stream {
            "\n\ncodex\n"
        } else {
            "codex\n"
        };
        self.emitted_agent_stream = true;
        format!("{prefix}{delta}")
    }

    fn push_reasoning_delta(&mut self, params: &Value) -> String {
        let delta = params
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if delta.is_empty() {
            return String::new();
        }
        let prefix = if self.emitted_reasoning_stream {
            ""
        } else {
            self.emitted_reasoning_stream = true;
            "thinking\n"
        };
        format!("{prefix}{delta}")
    }

    fn push_command_output_delta(&mut self, params: &Value) -> String {
        let delta = params
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if delta.trim().is_empty() {
            return String::new();
        }
        if let Some(item_id) = app_server_item_id(params) {
            self.command_output
                .entry(item_id)
                .or_default()
                .push_str(delta);
        }
        let mut output = String::new();
        append_live_section(
            &mut output,
            &format!("exec / Details\n```text\n{}\n```", delta.trim_end()),
        );
        output
    }

    fn normalize_turn_completed(&mut self, params: &Value) -> String {
        let turn = params.get("turn").unwrap_or(params);
        let mut output = String::new();
        for item in turn
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            append_live_section(&mut output, &self.normalize_completed_item(item));
        }
        match turn
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("completed")
        {
            "completed" => {
                append_live_section(&mut output, &self.take_pending_agent_message(false));
            }
            "failed" => {
                append_live_section(&mut output, &self.take_pending_agent_message(true));
                let message = app_server_turn_failure_message(turn)
                    .unwrap_or_else(|| "Codex app-server turn failed".to_string());
                self.last_error = Some(codex_app_server_turn_error(turn, &message));
                append_live_section(&mut output, &format!("ERROR: {message}"));
            }
            "interrupted" => {
                append_live_section(&mut output, &self.take_pending_agent_message(true));
            }
            _ => {}
        }
        output
    }

    fn normalize_completed_item(&mut self, item: &Value) -> String {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        if !item_id.is_empty() && !self.completed_items.insert(item_id.to_string()) {
            if item_type == "agentMessage" {
                self.capture_agent_message_final(item);
            }
            return String::new();
        }

        match item_type {
            "agentMessage" => self.normalize_agent_message_item(item),
            "reasoning" => {
                let text = app_server_reasoning_text(item);
                if text.trim().is_empty() {
                    String::new()
                } else {
                    let mut output = String::new();
                    append_live_section(&mut output, &format!("thinking\n{}", text.trim()));
                    output
                }
            }
            "plan" => item
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(|text| {
                    let mut output = String::new();
                    append_live_section(&mut output, &format!("thinking\n{}", text.trim()));
                    output
                })
                .unwrap_or_default(),
            "commandExecution" => {
                let formatted = self.format_command_execution(item);
                self.record_tool_message("command_execution", &formatted);
                formatted
            }
            "fileChange" => {
                let formatted = format_codex_live_file_change(item);
                self.record_tool_message("file_change", &formatted);
                formatted
            }
            "mcpToolCall" => {
                let name = item
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("mcp_tool_call");
                let formatted = format_codex_live_named_tool(
                    name,
                    item.get("arguments"),
                    item.get("result").or_else(|| item.get("error")),
                );
                self.record_tool_message(name, &formatted);
                formatted
            }
            "dynamicToolCall" => {
                let name = item
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("dynamic_tool_call");
                let formatted = format_codex_live_named_tool(
                    name,
                    item.get("arguments"),
                    item.get("contentItems").or_else(|| item.get("error")),
                );
                self.record_tool_message(name, &formatted);
                formatted
            }
            "webSearch" => {
                let formatted = format_codex_live_named_tool(
                    "web_search",
                    item.get("query"),
                    item.get("results"),
                );
                self.record_tool_message("web_search", &formatted);
                formatted
            }
            "imageView" => {
                let formatted = format_codex_live_named_tool("image_view", item.get("path"), None);
                self.record_tool_message("image_view", &formatted);
                formatted
            }
            "exitedReviewMode" => item
                .get("review")
                .and_then(Value::as_str)
                .filter(|review| !review.trim().is_empty())
                .map(|review| {
                    self.final_assistant_message = Some(bound_agent_text(
                        review.trim(),
                        AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
                        "assistant response",
                    ));
                    let mut output = String::new();
                    append_live_section(&mut output, &format!("codex\n{}", review.trim()));
                    output
                })
                .unwrap_or_default(),
            "contextCompaction" | "userMessage" | "enteredReviewMode" | "" => String::new(),
            _ => {
                let formatted = format_codex_live_named_tool(item_type, Some(item), None);
                if is_codex_app_server_tool_item_type(item_type) {
                    self.record_tool_message(item_type, &formatted);
                }
                formatted
            }
        }
    }

    fn normalize_agent_message_item(&mut self, item: &Value) -> String {
        let content = item
            .get("text")
            .map(display_codex_live_value)
            .unwrap_or_default();
        if content.trim().is_empty() {
            return String::new();
        }
        self.capture_agent_message_final(item);
        let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        if !item_id.is_empty() && self.streamed_agent_items.contains(item_id) {
            return String::new();
        }
        match item.get("phase").and_then(Value::as_str) {
            Some("commentary") => {
                let mut output = self.take_pending_agent_message(true);
                append_live_section(&mut output, &format!("thinking\n{}", content.trim()));
                output
            }
            Some("final_answer") => {
                let mut output = self.take_pending_agent_message(true);
                append_live_section(&mut output, &format!("codex\n{}", content.trim()));
                output
            }
            _ => {
                let previous = self.take_pending_agent_message(true);
                self.pending_agent_message = Some(content.trim().to_string());
                previous
            }
        }
    }

    fn capture_agent_message_final(&mut self, item: &Value) {
        let Some(text) = item
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            return;
        };
        if item.get("phase").and_then(Value::as_str) == Some("commentary") {
            return;
        }
        self.final_assistant_message = Some(bound_agent_text(
            text,
            AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
            "assistant response",
        ));
    }

    fn format_command_execution(&mut self, item: &Value) -> String {
        let mut normalized = item.clone();
        if let Some(object) = normalized.as_object_mut() {
            if !object.contains_key("aggregated_output")
                && let Some(value) = object.get("aggregatedOutput").cloned()
            {
                object.insert("aggregated_output".to_string(), value);
            }
            if !object.contains_key("exit_code")
                && let Some(value) = object.get("exitCode").cloned()
            {
                object.insert("exit_code".to_string(), value);
            }
            if !object.contains_key("aggregated_output")
                && let Some(item_id) = object.get("id").and_then(Value::as_str)
                && let Some(output) = self.command_output.get(item_id)
            {
                object.insert(
                    "aggregated_output".to_string(),
                    Value::String(output.clone()),
                );
            }
        }
        format_codex_live_command(&normalized)
    }

    fn normalize_token_usage(&mut self, params: &Value) -> String {
        let Some(token_usage) = params
            .get("tokenUsage")
            .or_else(|| params.get("token_usage"))
        else {
            return String::new();
        };
        self.final_usage = Some(normalize_codex_app_server_token_usage(token_usage));
        if !self.emitted_visible_turn_output {
            return String::new();
        }
        let mut output = String::new();
        append_live_section(
            &mut output,
            &format!(
                "tokens used\n{}",
                serde_json::to_string_pretty(token_usage)
                    .unwrap_or_else(|_| token_usage.to_string())
            ),
        );
        output
    }

    fn normalize_plan(&mut self, params: &Value) -> String {
        let mut plan_text = String::new();
        if let Some(explanation) = params
            .get("explanation")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            plan_text.push_str(explanation.trim());
        }
        for item in params
            .get("plan")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let step = item
                .get("step")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if step.is_empty() {
                continue;
            }
            if !plan_text.is_empty() {
                plan_text.push('\n');
            }
            let status = item
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            plan_text.push_str(&format!("- [{status}] {step}"));
        }
        if plan_text.trim().is_empty() {
            return String::new();
        }
        let mut output = String::new();
        append_live_section(&mut output, &format!("thinking\n{}", plan_text.trim()));
        output
    }

    fn normalize_diff(&mut self, params: &Value) -> String {
        let Some(diff) = params
            .get("diff")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        else {
            return String::new();
        };
        let formatted = bound_agent_text(
            &format!("diff / Details\n```diff\n{}\n```", diff.trim()),
            AGENT_TOOL_MESSAGE_MAX_BYTES,
            "tool output",
        );
        self.record_tool_message("diff", &formatted);
        let mut output = String::new();
        append_live_section(&mut output, &formatted);
        output
    }

    fn normalize_error(&mut self, params: &Value) -> String {
        let message = app_server_error_message(params)
            .unwrap_or_else(|| "Codex app-server reported an error".to_string());
        self.last_error = Some(codex_app_server_turn_error(params, &message));
        let mut output = String::new();
        append_live_section(&mut output, &format!("ERROR: {message}"));
        output
    }

    fn normalize_warning(&mut self, params: &Value) -> String {
        let message = params
            .get("message")
            .or_else(|| params.get("summary"))
            .map(display_codex_live_value)
            .unwrap_or_default();
        if message.trim().is_empty() {
            return String::new();
        }
        let mut output = String::new();
        append_live_section(&mut output, &format!("WARNING: {}", message.trim()));
        output
    }

    fn take_pending_agent_message(&mut self, thinking: bool) -> String {
        self.pending_agent_message
            .take()
            .map(|content| {
                if thinking {
                    format!("thinking\n{content}")
                } else {
                    self.final_assistant_message = Some(bound_agent_text(
                        &content,
                        AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
                        "assistant response",
                    ));
                    format!("codex\n{content}")
                }
            })
            .unwrap_or_default()
    }

    fn record_tool_message(&mut self, name: &str, content: &str) {
        if self.tool_messages.len() >= AGENT_TOOL_MESSAGES_MAX_COUNT
            || self.tool_message_bytes >= AGENT_TOOL_MESSAGES_MAX_TOTAL_BYTES
        {
            return;
        }
        let remaining = AGENT_TOOL_MESSAGES_MAX_TOTAL_BYTES - self.tool_message_bytes;
        let max_bytes = AGENT_TOOL_MESSAGE_MAX_BYTES.min(remaining);
        let content = bound_agent_text(content, max_bytes, "tool output");
        if content.is_empty() {
            return;
        }
        self.tool_message_bytes += content.len();
        self.tool_messages.push(NormalizedToolMessage {
            name: name.to_string(),
            content,
        });
    }

    fn take_tool_messages(&mut self) -> Vec<NormalizedToolMessage> {
        self.tool_message_bytes = 0;
        std::mem::take(&mut self.tool_messages)
    }

    fn take_final_assistant_message(&mut self) -> Option<String> {
        self.final_assistant_message.take()
    }

    fn take_final_usage(&mut self) -> Option<NormalizedRunUsage> {
        self.final_usage.take()
    }

    fn take_error(&mut self) -> Option<CodexTurnError> {
        self.last_error.take()
    }
}

fn app_server_item_id(value: &Value) -> Option<String> {
    value
        .get("itemId")
        .or_else(|| value.get("item_id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn app_server_reasoning_text(item: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(text) = item.get("text").and_then(Value::as_str) {
        parts.push(text.to_string());
    }
    for key in ["summary", "content"] {
        match item.get(key) {
            Some(Value::Array(items)) => {
                parts.extend(items.iter().filter_map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .or_else(|| item.get("text").and_then(Value::as_str).map(str::to_string))
                }));
            }
            Some(Value::String(text)) => parts.push(text.clone()),
            _ => {}
        }
    }
    parts
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_codex_app_server_tool_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "commandExecution"
            | "fileChange"
            | "mcpToolCall"
            | "dynamicToolCall"
            | "webSearch"
            | "imageView"
            | "collabAgentToolCall"
            | "subAgentActivity"
            | "imageGeneration"
    )
}

fn app_server_turn_failure_message(turn: &Value) -> Option<String> {
    turn.pointer("/error/message")
        .or_else(|| turn.get("error"))
        .map(display_codex_live_value)
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
}

fn app_server_error_message(value: &Value) -> Option<String> {
    value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .or_else(|| value.get("error"))
        .map(display_codex_live_value)
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
}

fn codex_app_server_turn_error(event: &Value, message: &str) -> CodexTurnError {
    CodexTurnError {
        message: message.to_string(),
        code: app_server_error_code(event),
        limit_bytes: app_server_error_u64(event, "limit_bytes")
            .or_else(|| app_server_error_u64(event, "limitBytes")),
        observed_bytes: app_server_error_u64(event, "content_length_bytes")
            .or_else(|| app_server_error_u64(event, "contentLengthBytes")),
    }
}

fn app_server_error_code(value: &Value) -> Option<String> {
    value
        .pointer("/error/code")
        .or_else(|| value.pointer("/error/codexErrorInfo"))
        .or_else(|| value.pointer("/error/codex_error_info"))
        .or_else(|| value.get("code"))
        .or_else(|| value.get("codexErrorInfo"))
        .and_then(|code| match code {
            Value::String(code) => Some(code.clone()),
            Value::Object(object) => object.keys().next().cloned(),
            _ => None,
        })
}

fn app_server_error_u64(value: &Value, key: &str) -> Option<u64> {
    value
        .pointer(&format!("/error/details/{key}"))
        .or_else(|| value.pointer(&format!("/error/additionalDetails/{key}")))
        .or_else(|| value.pointer(&format!("/details/{key}")))
        .and_then(Value::as_u64)
}

impl CodexLiveOutputNormalizer {
    fn push(&mut self, chunk: &str) -> String {
        self.pending_line.push_str(chunk);
        let mut output = String::new();
        while let Some(newline) = self.pending_line.find('\n') {
            let line = self.pending_line[..newline]
                .trim_end_matches('\r')
                .to_string();
            self.pending_line.drain(..=newline);
            append_live_section(&mut output, &self.normalize_line(&line));
        }
        output
    }

    fn finish(&mut self) -> String {
        let mut output = String::new();
        if !self.pending_line.is_empty() {
            let line = std::mem::take(&mut self.pending_line);
            append_live_section(
                &mut output,
                &self.normalize_line(line.trim_end_matches('\r')),
            );
        }
        append_live_section(&mut output, &self.take_pending_agent_message(false));
        output
    }

    fn normalize_line(&mut self, line: &str) -> String {
        if line.trim().is_empty() {
            return String::new();
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return line.to_string();
        };
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return line.to_string();
        };
        self.saw_structured_event = true;
        match event_type {
            "thread.started" => {
                if let Some(thread_id) = event
                    .get("thread_id")
                    .or_else(|| event.get("threadId"))
                    .and_then(Value::as_str)
                    .filter(|thread_id| !thread_id.trim().is_empty())
                {
                    self.pending_thread_id = Some(thread_id.to_string());
                }
                String::new()
            }
            "item.started" | "item.updated" | "turn.started" => String::new(),
            "item.completed" => self.normalize_completed_item(&event),
            "turn.completed" => {
                let mut output = self.take_pending_agent_message(false);
                if let Some(usage) = event.get("usage").filter(|value| !value.is_null()) {
                    self.final_usage = Some(normalize_codex_run_usage(usage));
                    append_live_section(
                        &mut output,
                        &format!(
                            "tokens used\n{}",
                            serde_json::to_string_pretty(usage)
                                .unwrap_or_else(|_| usage.to_string())
                        ),
                    );
                }
                output
            }
            "turn.failed" => {
                let mut output = self.take_pending_agent_message(true);
                let message = event
                    .pointer("/error/message")
                    .or_else(|| event.get("error"))
                    .map(display_codex_live_value)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "Codex turn failed".to_string());
                self.last_error = Some(codex_turn_error(&event, &message));
                append_live_section(&mut output, &format!("ERROR: {message}"));
                output
            }
            "error" => {
                let mut output = self.take_pending_agent_message(true);
                let message = event
                    .get("message")
                    .or_else(|| event.get("error"))
                    .map(display_codex_live_value)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "Codex reported an error".to_string());
                self.last_error = Some(codex_turn_error(&event, &message));
                append_live_section(&mut output, &format!("ERROR: {message}"));
                output
            }
            _ => line.to_string(),
        }
    }

    fn normalize_completed_item(&mut self, event: &Value) -> String {
        let item = event.get("item").unwrap_or(&Value::Null);
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if item_type == "agent_message" {
            let content = item
                .get("text")
                .or_else(|| item.pointer("/message/content"))
                .map(display_codex_live_value)
                .unwrap_or_default();
            if content.trim().is_empty() {
                return String::new();
            }
            return match item.get("phase").and_then(Value::as_str) {
                Some("commentary") => {
                    let mut output = self.take_pending_agent_message(true);
                    append_live_section(&mut output, &format!("thinking\n{}", content.trim()));
                    output
                }
                Some("final_answer") => {
                    self.final_assistant_message = Some(bound_agent_text(
                        content.trim(),
                        AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
                        "assistant response",
                    ));
                    let mut output = self.take_pending_agent_message(true);
                    append_live_section(&mut output, &format!("codex\n{}", content.trim()));
                    output
                }
                _ => {
                    let previous = self.take_pending_agent_message(true);
                    self.pending_agent_message = Some(content.trim().to_string());
                    previous
                }
            };
        }

        let mut output = String::new();
        let item_output = match item_type {
            "reasoning" => item
                .get("text")
                .map(display_codex_live_value)
                .filter(|value| !value.is_empty())
                .map(|text| format!("thinking\n{text}"))
                .unwrap_or_default(),
            "command_execution" => format_codex_live_command(item),
            "file_change" => format_codex_live_file_change(item),
            "function_call" => format_codex_live_function_call(item),
            "function_call_output" => format_codex_live_tool_result(item, "function_call"),
            "custom_tool_call" => format_codex_live_custom_tool(item),
            "custom_tool_call_output" => format_codex_live_tool_result(item, "custom_tool_call"),
            "mcp_tool_call" => format_codex_live_named_tool(
                item.get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("mcp_tool_call"),
                item.get("arguments"),
                item.get("result").or_else(|| item.get("error")),
            ),
            "web_search" => {
                format_codex_live_named_tool("web_search", item.get("query"), item.get("result"))
            }
            "todo_list" => format_codex_live_named_tool("todo_list", item.get("items"), None),
            "error" => format!(
                "ERROR: {}",
                item.get("message")
                    .map(display_codex_live_value)
                    .unwrap_or_else(|| "Codex item failed".to_string())
            ),
            "" => return output,
            _ => format_codex_live_named_tool(item_type, Some(item), None),
        };
        if is_codex_tool_item_type(item_type) && !item_output.trim().is_empty() {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(item_type);
            self.record_tool_message(name, &item_output);
        }
        append_live_section(&mut output, &item_output);
        output
    }

    fn take_pending_agent_message(&mut self, thinking: bool) -> String {
        self.pending_agent_message
            .take()
            .map(|content| {
                if thinking {
                    format!("thinking\n{content}")
                } else {
                    self.final_assistant_message = Some(bound_agent_text(
                        &content,
                        AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
                        "assistant response",
                    ));
                    format!("codex\n{content}")
                }
            })
            .unwrap_or_default()
    }

    fn record_tool_message(&mut self, name: &str, content: &str) {
        if self.tool_messages.len() >= AGENT_TOOL_MESSAGES_MAX_COUNT
            || self.tool_message_bytes >= AGENT_TOOL_MESSAGES_MAX_TOTAL_BYTES
        {
            return;
        }
        let remaining = AGENT_TOOL_MESSAGES_MAX_TOTAL_BYTES - self.tool_message_bytes;
        let max_bytes = AGENT_TOOL_MESSAGE_MAX_BYTES.min(remaining);
        let content = bound_agent_text(content, max_bytes, "tool output");
        if content.is_empty() {
            return;
        }
        self.tool_message_bytes += content.len();
        self.tool_messages.push(NormalizedToolMessage {
            name: name.to_string(),
            content,
        });
    }

    fn take_final_assistant_message(&mut self) -> Option<String> {
        self.final_assistant_message.take()
    }

    fn take_final_usage(&mut self) -> Option<NormalizedRunUsage> {
        self.final_usage.take()
    }

    fn saw_structured_event(&self) -> bool {
        self.saw_structured_event
    }

    fn take_tool_messages(&mut self) -> Vec<NormalizedToolMessage> {
        self.tool_message_bytes = 0;
        std::mem::take(&mut self.tool_messages)
    }

    fn take_thread_id(&mut self) -> Option<String> {
        self.pending_thread_id.take()
    }

    fn take_error(&mut self) -> Option<CodexTurnError> {
        self.last_error.take()
    }
}

fn codex_turn_error(event: &Value, message: &str) -> CodexTurnError {
    let code = event
        .pointer("/error/code")
        .or_else(|| event.get("code"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let limit_bytes = event
        .pointer("/error/details/limit_bytes")
        .or_else(|| event.pointer("/details/limit_bytes"))
        .and_then(Value::as_u64);
    let observed_bytes = event
        .pointer("/error/details/content_length_bytes")
        .or_else(|| event.pointer("/details/content_length_bytes"))
        .and_then(Value::as_u64);
    CodexTurnError {
        message: message.to_string(),
        code,
        limit_bytes,
        observed_bytes,
    }
}

fn is_request_body_too_large_error(error: &CodexTurnError) -> bool {
    error.code.as_deref() == Some("request_body_too_large")
        || error.code.as_deref() == Some("contextWindowExceeded")
        || error.message.to_ascii_lowercase().contains("http 413")
        || error.message.to_ascii_lowercase().contains("payload too large")
        // Compatibility with gateways deployed before the structured 413 fix.
        || error.message.trim().eq_ignore_ascii_case("invalid body")
}

fn is_codex_tool_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "command_execution"
            | "file_change"
            | "function_call"
            | "function_call_output"
            | "custom_tool_call"
            | "custom_tool_call_output"
            | "mcp_tool_call"
            | "web_search"
            | "todo_list"
    )
}

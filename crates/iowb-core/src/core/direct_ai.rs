#[derive(Debug, Clone)]
struct DirectAiStreamOutput {
    text: String,
    streamed: bool,
    usage: Option<NormalizedRunUsage>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct DirectAiConversationMessage {
    role: &'static str,
    content: String,
}

fn direct_ai_conversation_messages(
    messages: Vec<ChatMessage>,
    fallback_prompt: &str,
) -> Vec<DirectAiConversationMessage> {
    let mut selected = Vec::new();
    let mut selected_bytes = 0usize;

    for message in messages.into_iter().rev() {
        let role = match message.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System | MessageRole::Tool => continue,
        };
        let content = message.content.trim();
        if content.is_empty() {
            continue;
        }
        if selected.len() >= DIRECT_AI_HISTORY_MAX_MESSAGES {
            break;
        }
        let next_bytes = selected_bytes.saturating_add(content.len());
        if !selected.is_empty() && next_bytes > DIRECT_AI_HISTORY_MAX_BYTES {
            break;
        }
        selected_bytes = next_bytes;
        selected.push(DirectAiConversationMessage {
            role,
            content: content.to_string(),
        });
    }
    selected.reverse();

    while selected
        .first()
        .is_some_and(|message| message.role == "assistant")
    {
        selected.remove(0);
    }

    let mut normalized: Vec<DirectAiConversationMessage> = Vec::new();
    for message in selected {
        if let Some(previous) = normalized.last_mut()
            && previous.role == message.role
        {
            previous.content.push_str("\n\n");
            previous.content.push_str(&message.content);
        } else {
            normalized.push(message);
        }
    }

    if normalized.is_empty() {
        let fallback_prompt = fallback_prompt.trim();
        if !fallback_prompt.is_empty() {
            normalized.push(DirectAiConversationMessage {
                role: "user",
                content: fallback_prompt.to_string(),
            });
        }
    }

    normalized
}

fn append_direct_ai_recovery_prompt(
    messages: &mut Vec<DirectAiConversationMessage>,
    recovery_prompt: &str,
) {
    if let Some(last) = messages.last_mut()
        && last.role == "user"
    {
        last.content.push_str("\n\n");
        last.content.push_str(recovery_prompt);
    } else {
        messages.push(DirectAiConversationMessage {
            role: "user",
            content: recovery_prompt.to_string(),
        });
    }
}

async fn stream_direct_ai_model_api<F, Fut>(
    config: &DirectAiRuntimeConfig,
    model: &str,
    messages: &[DirectAiConversationMessage],
    mut on_chunk: F,
) -> std::result::Result<DirectAiStreamOutput, String>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ()>,
{
    let api_key = config.api_key.trim();
    if api_key.is_empty() {
        return Err("Direct AI API key is empty".to_string());
    }
    let base_url = config.base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return Err("Direct AI base URL is empty".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| format!("failed to create Direct AI client: {error}"))?;

    let max_tokens = config.max_tokens.unwrap_or(4096);
    let messages = messages
        .iter()
        .map(|message| {
            serde_json::json!({
                "role": message.role,
                "content": message.content,
            })
        })
        .collect::<Vec<_>>();
    if messages.is_empty() {
        return Err("Direct AI conversation is empty".to_string());
    }
    let messages_body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": messages,
    });
    let chat_body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "stream_options": {
            "include_usage": true,
        },
        "messages": messages,
    });

    let mut best_error: Option<String> = None;
    for candidate in direct_ai_request_candidates(base_url, model) {
        let body = match candidate.kind {
            DirectAiRequestKind::Messages => &messages_body,
            DirectAiRequestKind::ChatCompletions => &chat_body,
        };
        let response = post_direct_ai_json(&client, &candidate.url, api_key, body).await?;
        if response.status().is_success() {
            let output = read_direct_ai_response(response, &mut on_chunk).await?;
            if output.text.trim().is_empty() {
                return Err("Direct AI returned an empty response".to_string());
            }
            return Ok(output);
        }

        let status = response.status();
        let error = direct_ai_http_error(response).await;
        if best_error
            .as_deref()
            .map(is_low_value_direct_ai_route_error)
            .unwrap_or(true)
            || !is_low_value_direct_ai_route_error(&error)
        {
            best_error = Some(error);
        }
        if !matches!(status.as_u16(), 400 | 404 | 405) {
            break;
        }
    }

    Err(best_error.unwrap_or_else(|| "Direct AI gateway request failed".to_string()))
}

async fn read_direct_ai_response<F, Fut>(
    mut response: reqwest::Response,
    on_chunk: &mut F,
) -> std::result::Result<DirectAiStreamOutput, String>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut raw = Vec::new();
    let mut line_buffer = String::new();
    let mut text = String::new();
    let mut streamed = false;
    let mut usage = None;

    while let Some(bytes) = response
        .chunk()
        .await
        .map_err(|error| format!("Direct AI response stream failed: {error}"))?
    {
        raw.extend_from_slice(&bytes);
        line_buffer.push_str(&String::from_utf8_lossy(&bytes));
        drain_direct_ai_sse_lines(
            &mut line_buffer,
            &mut text,
            &mut streamed,
            &mut usage,
            on_chunk,
        )
        .await;
    }
    if !line_buffer.trim().is_empty() {
        process_direct_ai_sse_line(
            line_buffer.trim(),
            &mut text,
            &mut streamed,
            &mut usage,
            on_chunk,
        )
        .await;
    }

    if streamed {
        return Ok(DirectAiStreamOutput {
            text,
            streamed,
            usage,
        });
    }

    let value = serde_json::from_slice::<Value>(&raw)
        .map_err(|error| format!("Direct AI returned invalid JSON: {error}"))?;
    Ok(DirectAiStreamOutput {
        text: extract_direct_ai_response_text(&value),
        streamed: false,
        usage: normalize_direct_ai_run_usage(&value),
    })
}

async fn drain_direct_ai_sse_lines<F, Fut>(
    buffer: &mut String,
    text: &mut String,
    streamed: &mut bool,
    usage: &mut Option<NormalizedRunUsage>,
    on_chunk: &mut F,
) where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ()>,
{
    while let Some(index) = buffer.find('\n') {
        let line = buffer[..index].trim_end_matches('\r').to_string();
        buffer.drain(..index + 1);
        process_direct_ai_sse_line(&line, text, streamed, usage, on_chunk).await;
    }
}

async fn process_direct_ai_sse_line<F, Fut>(
    line: &str,
    text: &mut String,
    streamed: &mut bool,
    usage: &mut Option<NormalizedRunUsage>,
    on_chunk: &mut F,
) where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ()>,
{
    let Some(data) = line.trim().strip_prefix("data:") else {
        return;
    };
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return;
    };
    if let Some(parsed) = normalize_direct_ai_run_usage(&value) {
        *usage = Some(parsed);
    }
    let chunk = extract_direct_ai_stream_delta(&value);
    if chunk.is_empty() {
        return;
    }
    *streamed = true;
    text.push_str(&chunk);
    let chunks = direct_ai_display_chunks(&chunk);
    let chunk_count = chunks.len();
    for (index, chunk) in chunks.into_iter().enumerate() {
        on_chunk(chunk).await;
        if chunk_count > 1 && index + 1 < chunk_count {
            tokio::time::sleep(Duration::from_millis(DIRECT_AI_SYNTHETIC_CHUNK_DELAY_MS)).await;
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum DirectAiRequestKind {
    Messages,
    ChatCompletions,
}

#[derive(Debug, Clone)]
struct DirectAiRequestCandidate {
    url: String,
    kind: DirectAiRequestKind,
}

fn direct_ai_request_candidates(base_url: &str, model: &str) -> Vec<DirectAiRequestCandidate> {
    let root = url_origin(base_url).unwrap_or_else(|| base_url.to_string());
    let claude_base = if base_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .is_some_and(|segment| segment.eq_ignore_ascii_case("claude"))
    {
        base_url.trim_end_matches('/').to_string()
    } else {
        format!("{}/claude", root.trim_end_matches('/'))
    };
    let root = root.trim_end_matches('/').to_string();
    let base = base_url.trim_end_matches('/').to_string();
    let prefix = gateway_model_prefix(model).unwrap_or_default();

    let mut candidates = Vec::new();
    match prefix.as_str() {
        "cld" => {
            push_direct_ai_candidate(
                &mut candidates,
                format!("{claude_base}/v1/messages"),
                DirectAiRequestKind::Messages,
            );
            push_direct_ai_candidate(
                &mut candidates,
                format!("{root}/v1/chat/completions"),
                DirectAiRequestKind::ChatCompletions,
            );
        }
        "agw" | "gem" | "cop" | "ctm" | "dsk" | "glm" | "grk" | "min" => {
            push_direct_ai_candidate(
                &mut candidates,
                format!("{root}/v1/chat/completions"),
                DirectAiRequestKind::ChatCompletions,
            );
            push_direct_ai_candidate(
                &mut candidates,
                format!("{claude_base}/v1/messages"),
                DirectAiRequestKind::Messages,
            );
        }
        _ => {
            push_direct_ai_candidate(
                &mut candidates,
                format!("{base}/v1/messages"),
                DirectAiRequestKind::Messages,
            );
            push_direct_ai_candidate(
                &mut candidates,
                format!("{base}/v1/chat/completions"),
                DirectAiRequestKind::ChatCompletions,
            );
            push_direct_ai_candidate(
                &mut candidates,
                format!("{root}/v1/chat/completions"),
                DirectAiRequestKind::ChatCompletions,
            );
            push_direct_ai_candidate(
                &mut candidates,
                format!("{claude_base}/v1/messages"),
                DirectAiRequestKind::Messages,
            );
        }
    }
    candidates
}

fn push_direct_ai_candidate(
    candidates: &mut Vec<DirectAiRequestCandidate>,
    url: String,
    kind: DirectAiRequestKind,
) {
    if !candidates.iter().any(|candidate| candidate.url == url) {
        candidates.push(DirectAiRequestCandidate { url, kind });
    }
}

fn is_low_value_direct_ai_route_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("endpoint not found") || lower.contains("\"message\":\"not found\"")
}

fn url_origin(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let scheme_end = trimmed.find("://")?;
    let after_scheme = &trimmed[scheme_end + 3..];
    let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());
    let origin = &trimmed[..scheme_end + 3 + path_start];
    if origin.is_empty() {
        None
    } else {
        Some(origin.trim_end_matches('/').to_string())
    }
}

async fn post_direct_ai_json(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &Value,
) -> std::result::Result<reqwest::Response, String> {
    client
        .post(url)
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .bearer_auth(api_key)
        .header("x-api-key", api_key)
        .json(body)
        .send()
        .await
        .map_err(|error| format!("Direct AI request failed: {error}"))
}

async fn direct_ai_http_error(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    format!(
        "Direct AI HTTP {status}: {}",
        body.chars().take(300).collect::<String>()
    )
}

fn extract_direct_ai_response_text(value: &Value) -> String {
    collect_direct_ai_text(value.get("content"))
        .or_else(|| {
            value
                .get("choices")
                .and_then(Value::as_array)
                .map(|choices| {
                    choices
                        .iter()
                        .filter_map(|choice| {
                            collect_direct_ai_text(
                                choice
                                    .get("message")
                                    .and_then(|message| message.get("content")),
                            )
                            .or_else(|| {
                                collect_direct_ai_text(
                                    choice.get("delta").and_then(|delta| delta.get("content")),
                                )
                            })
                            .or_else(|| {
                                choice
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                        })
                        .collect::<Vec<_>>()
                        .join("")
                })
        })
        .or_else(|| {
            value.get("output").and_then(Value::as_array).map(|output| {
                output
                    .iter()
                    .filter_map(|item| {
                        collect_direct_ai_text(item.get("content")).or_else(|| {
                            item.get("text").and_then(Value::as_str).map(str::to_string)
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("")
            })
        })
        .or_else(|| {
            value
                .get("output_text")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn extract_direct_ai_stream_delta(value: &Value) -> String {
    value
        .get("choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .filter_map(|choice| {
                    choice
                        .get("delta")
                        .and_then(|delta| {
                            collect_direct_ai_text(delta.get("content")).or_else(|| {
                                delta
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                        })
                        .or_else(|| {
                            choice
                                .get("text")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .filter(|text| !text.is_empty())
        .or_else(|| {
            value.get("delta").and_then(|delta| {
                delta.as_str().map(str::to_string).or_else(|| {
                    delta
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| collect_direct_ai_text(delta.get("content")))
                })
            })
        })
        .or_else(|| {
            value
                .get("content_block")
                .and_then(|block| block.get("text"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("type")
                .and_then(Value::as_str)
                .filter(|event_type| {
                    event_type.ends_with(".delta") || event_type.contains("_delta")
                })
                .and_then(|_| collect_direct_ai_text(value.get("content")))
        })
        .unwrap_or_default()
}

fn direct_ai_display_chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for segment in text.split_inclusive('\n') {
        if current.len() + segment.len() > DIRECT_AI_DISPLAY_CHUNK_CHARS && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        if segment.len() > DIRECT_AI_DISPLAY_CHUNK_CHARS * 2 {
            for piece in split_on_char_boundaries(segment, DIRECT_AI_DISPLAY_CHUNK_CHARS) {
                if !current.is_empty() {
                    chunks.push(std::mem::take(&mut current));
                }
                chunks.push(piece);
            }
        } else {
            current.push_str(segment);
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn split_on_char_boundaries(text: &str, target_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if current.chars().count() >= target_chars {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn collect_direct_ai_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .or_else(|| item.get("text").and_then(Value::as_str).map(str::to_string))
                        .or_else(|| collect_direct_ai_text(item.get("content")))
                        .or_else(|| {
                            item.get("output_text")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                })
                .collect::<Vec<_>>()
                .join("");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn normalize_codex_run_usage(value: &Value) -> NormalizedRunUsage {
    let usage = usage_container(value);
    let input = usage_u64(usage, &["input_tokens", "inputTokens", "input"]);
    let output = usage_u64(usage, &["output_tokens", "outputTokens", "output"]);
    let cache_creation = usage_u64(
        usage,
        &[
            "cache_write_input_tokens",
            "cacheWriteInputTokens",
            "cache_creation_input_tokens",
            "cacheCreationInputTokens",
            "cache_creation",
            "cacheCreation",
        ],
    );
    let cache_read = usage_u64(
        usage,
        &[
            "cached_input_tokens",
            "cachedInputTokens",
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cache_read",
            "cacheRead",
        ],
    );
    let reasoning = usage_u64(
        usage,
        &[
            "reasoning_output_tokens",
            "reasoningOutputTokens",
            "reasoning_tokens",
            "reasoningTokens",
            "reasoning",
        ],
    );
    normalized_run_usage(
        usage,
        "codex.turn.completed.usage",
        input,
        output,
        cache_creation,
        cache_read,
        reasoning,
        usage_f64(usage, &["cost_usd", "costUsd"]),
    )
}

fn normalize_codex_app_server_token_usage(value: &Value) -> NormalizedRunUsage {
    let usage = value.get("last").unwrap_or(value);
    let input = usage_u64(usage, &["input_tokens", "inputTokens", "input"]);
    let output = usage_u64(usage, &["output_tokens", "outputTokens", "output"]);
    let cache_creation = usage_u64(
        usage,
        &[
            "cache_write_input_tokens",
            "cacheWriteInputTokens",
            "cache_creation_input_tokens",
            "cacheCreationInputTokens",
            "cache_creation",
            "cacheCreation",
        ],
    );
    let cache_read = usage_u64(
        usage,
        &[
            "cached_input_tokens",
            "cachedInputTokens",
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cache_read",
            "cacheRead",
        ],
    );
    let reasoning = usage_u64(
        usage,
        &[
            "reasoning_output_tokens",
            "reasoningOutputTokens",
            "reasoning_tokens",
            "reasoningTokens",
            "reasoning",
        ],
    );
    normalized_run_usage(
        usage,
        "codex.app_server.turn.usage",
        input,
        output,
        cache_creation,
        cache_read,
        reasoning,
        None,
    )
}

fn normalize_claude_run_usage(event: &Value) -> NormalizedRunUsage {
    let usage = event
        .get("modelUsage")
        .or_else(|| event.get("model_usage"))
        .or_else(|| event.get("usage"))
        .unwrap_or(event);
    let totals = aggregate_usage_like_values(usage);
    let mut result = normalized_run_usage(
        usage,
        if event.get("modelUsage").is_some() || event.get("model_usage").is_some() {
            "claude.result.modelUsage"
        } else {
            "claude.result.usage"
        },
        totals.input,
        totals.output,
        totals.cache_creation,
        totals.cache_read,
        totals.reasoning,
        usage_f64(
            event,
            &["total_cost_usd", "totalCostUsd", "cost_usd", "costUsd"],
        )
        .or_else(|| {
            usage_f64(
                usage,
                &["total_cost_usd", "totalCostUsd", "cost_usd", "costUsd"],
            )
        }),
    );
    if result.usage.used == 0 {
        result.completeness = TokenUsageCompleteness::Missing;
    }
    result
}

fn normalize_gemini_run_usage(event: &Value) -> Option<NormalizedRunUsage> {
    let usage = event
        .get("stats")
        .or_else(|| event.pointer("/result/stats"))
        .or_else(|| event.get("usage"))
        .or_else(|| event.get("usageMetadata"))
        .or_else(|| event.get("usage_metadata"))?;
    let input = usage_u64(
        usage,
        &[
            "input_tokens",
            "inputTokens",
            "prompt_token_count",
            "promptTokenCount",
        ],
    );
    let output = usage_u64(
        usage,
        &[
            "output_tokens",
            "outputTokens",
            "candidates_token_count",
            "candidatesTokenCount",
        ],
    );
    let cache_read = usage_u64(
        usage,
        &[
            "cached_content_token_count",
            "cachedContentTokenCount",
            "cache_read_input_tokens",
            "cacheReadInputTokens",
        ],
    );
    let mut result = normalized_run_usage(
        usage,
        "gemini.result.stats",
        input,
        output,
        0,
        cache_read,
        usage_u64(
            usage,
            &["thoughts_token_count", "thoughtsTokenCount", "reasoning"],
        ),
        usage_f64(usage, &["cost_usd", "costUsd"]),
    );
    if result.usage.used == 0 {
        result.completeness = TokenUsageCompleteness::Missing;
    }
    Some(result)
}

fn normalize_direct_ai_run_usage(value: &Value) -> Option<NormalizedRunUsage> {
    let usage = value
        .get("usage")
        .or_else(|| value.get("usageMetadata"))
        .or_else(|| value.get("usage_metadata"))
        .or_else(|| value.pointer("/response/usage"))?;
    let mut result = if value.get("usageMetadata").is_some()
        || value.get("usage_metadata").is_some()
    {
        normalize_gemini_run_usage(value)
            .unwrap_or_else(|| normalized_run_usage(usage, "direct_ai.usage", 0, 0, 0, 0, 0, None))
    } else {
        normalized_direct_ai_usage_from_container(usage)
    };
    result.source = "direct_ai.usage";
    Some(result)
}

fn normalized_direct_ai_usage_from_container(usage: &Value) -> NormalizedRunUsage {
    let input = usage_u64(
        usage,
        &[
            "input_tokens",
            "inputTokens",
            "prompt_tokens",
            "promptTokens",
            "input",
        ],
    );
    let output = usage_u64(
        usage,
        &[
            "output_tokens",
            "outputTokens",
            "completion_tokens",
            "completionTokens",
            "output",
        ],
    );
    let cache_creation = usage_u64(
        usage,
        &[
            "cache_creation_input_tokens",
            "cacheCreationInputTokens",
            "cache_write_input_tokens",
            "cacheWriteInputTokens",
        ],
    );
    let cache_read = usage_u64(
        usage,
        &[
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cached_input_tokens",
            "cachedInputTokens",
        ],
    );
    let reasoning = usage_u64(
        usage,
        &[
            "reasoning_tokens",
            "reasoningTokens",
            "reasoning_output_tokens",
            "reasoningOutputTokens",
        ],
    );
    normalized_run_usage(
        usage,
        "direct_ai.usage",
        input,
        output,
        cache_creation,
        cache_read,
        reasoning,
        usage_f64(
            usage,
            &["cost_usd", "costUsd", "total_cost_usd", "totalCostUsd"],
        ),
    )
}

#[derive(Default)]
struct UsageFields {
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
    reasoning: u64,
}

fn aggregate_usage_like_values(value: &Value) -> UsageFields {
    if let Some(object) = value.as_object() {
        let directly_has_usage = object.keys().any(|key| {
            matches!(
                key.as_str(),
                "input_tokens"
                    | "inputTokens"
                    | "output_tokens"
                    | "outputTokens"
                    | "cache_creation_input_tokens"
                    | "cacheCreationInputTokens"
                    | "cache_read_input_tokens"
                    | "cacheReadInputTokens"
            )
        });
        if directly_has_usage {
            return UsageFields {
                input: usage_u64(value, &["input_tokens", "inputTokens", "input"]),
                output: usage_u64(value, &["output_tokens", "outputTokens", "output"]),
                cache_creation: usage_u64(
                    value,
                    &[
                        "cache_creation_input_tokens",
                        "cacheCreationInputTokens",
                        "cache_creation",
                        "cacheCreation",
                    ],
                ),
                cache_read: usage_u64(
                    value,
                    &[
                        "cache_read_input_tokens",
                        "cacheReadInputTokens",
                        "cache_read",
                        "cacheRead",
                    ],
                ),
                reasoning: usage_u64(value, &["reasoning_tokens", "reasoningTokens", "reasoning"]),
            };
        }
        return object.values().map(aggregate_usage_like_values).fold(
            UsageFields::default(),
            |mut total, usage| {
                total.input = total.input.saturating_add(usage.input);
                total.output = total.output.saturating_add(usage.output);
                total.cache_creation = total.cache_creation.saturating_add(usage.cache_creation);
                total.cache_read = total.cache_read.saturating_add(usage.cache_read);
                total.reasoning = total.reasoning.saturating_add(usage.reasoning);
                total
            },
        );
    }
    if let Some(array) = value.as_array() {
        return array.iter().map(aggregate_usage_like_values).fold(
            UsageFields::default(),
            |mut total, usage| {
                total.input = total.input.saturating_add(usage.input);
                total.output = total.output.saturating_add(usage.output);
                total.cache_creation = total.cache_creation.saturating_add(usage.cache_creation);
                total.cache_read = total.cache_read.saturating_add(usage.cache_read);
                total.reasoning = total.reasoning.saturating_add(usage.reasoning);
                total
            },
        );
    }
    UsageFields::default()
}

fn normalized_run_usage(
    usage: &Value,
    source: &'static str,
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
    reasoning: u64,
    cost_usd: Option<f64>,
) -> NormalizedRunUsage {
    let total = usage_u64(
        usage,
        &[
            "total_tokens",
            "totalTokens",
            "total_token_count",
            "totalTokenCount",
            "total",
        ],
    )
    .max(input.saturating_add(output));
    let usage_value = SessionTokenUsage {
        used: total,
        input,
        output,
        cache_creation,
        cache_read,
        reasoning,
        cost_usd: cost_usd.unwrap_or(0.0),
    };
    let completeness = if total > 0 {
        TokenUsageCompleteness::Complete
    } else {
        TokenUsageCompleteness::Missing
    };
    NormalizedRunUsage {
        usage: usage_value,
        raw_usage_json: serde_json::to_string(usage_container(usage)).ok(),
        source,
        completeness,
    }
}

fn usage_container(value: &Value) -> &Value {
    value
        .get("total_token_usage")
        .or_else(|| value.get("totalTokenUsage"))
        .unwrap_or(value)
}

fn usage_u64(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| {
            value.get(*key).and_then(|raw| {
                raw.as_u64().or_else(|| {
                    raw.as_i64()
                        .and_then(|value| u64::try_from(value).ok())
                        .or_else(|| raw.as_str().and_then(|value| value.parse::<u64>().ok()))
                })
            })
        })
        .unwrap_or(0)
}

fn usage_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|raw| {
            raw.as_f64()
                .or_else(|| raw.as_str().and_then(|value| value.parse::<f64>().ok()))
        })
    })
}

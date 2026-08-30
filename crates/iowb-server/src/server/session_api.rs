#[derive(Debug, Default, serde::Deserialize)]
struct SessionMessagesQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    tail: bool,
}

#[derive(Debug, Default, serde::Deserialize)]
struct SessionPromptsQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default, alias = "beforeTimestamp")]
    before_timestamp: Option<String>,
    #[serde(default, alias = "beforeId")]
    before_id: Option<String>,
}

fn sanitize_session_response_text(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len().min(SESSION_RESPONSE_MAX_CONTENT_BYTES));
    let mut line_chars = 0;
    for character in value.chars() {
        if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
            continue;
        }
        if character == '\n' || character == '\r' {
            line_chars = 0;
        } else {
            line_chars += 1;
            if line_chars > SESSION_RESPONSE_MAX_LINE_CHARS {
                sanitized.push_str("\n[long line wrapped for display]\n");
                line_chars = 1;
            }
        }
        sanitized.push(character);
    }
    sanitized
}

fn response_utf8_prefix_boundary(value: &str, max_bytes: usize) -> usize {
    if value.len() <= max_bytes {
        return value.len();
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn response_utf8_suffix_boundary(value: &str, max_bytes: usize) -> usize {
    if value.len() <= max_bytes {
        return 0;
    }
    let mut boundary = value.len().saturating_sub(max_bytes);
    while boundary < value.len() && !value.is_char_boundary(boundary) {
        boundary += 1;
    }
    boundary
}

fn bound_session_response_text(value: &str, max_bytes: usize, label: &str) -> String {
    let sanitized = sanitize_session_response_text(value);
    if sanitized.len() <= max_bytes {
        return sanitized;
    }
    if max_bytes == 0 {
        return String::new();
    }
    let marker = format!(
        "\n\n[truncated {label}: original {} bytes; showing beginning and end]\n\n",
        sanitized.len()
    );
    if marker.len() >= max_bytes {
        let end = response_utf8_prefix_boundary(&marker, max_bytes);
        return marker[..end].to_string();
    }
    let available = max_bytes - marker.len();
    let head_budget = available.saturating_mul(3) / 4;
    let tail_budget = available - head_budget;
    let head_end = response_utf8_prefix_boundary(&sanitized, head_budget);
    let tail_start = response_utf8_suffix_boundary(&sanitized, tail_budget);
    format!(
        "{}{}{}",
        &sanitized[..head_end],
        marker,
        &sanitized[tail_start..]
    )
}

fn bounded_session_response_metadata(metadata: &Value) -> Value {
    if serde_json::to_vec(metadata)
        .is_ok_and(|encoded| encoded.len() <= SESSION_RESPONSE_METADATA_MAX_BYTES)
    {
        return metadata.clone();
    }
    let mut bounded = serde_json::Map::new();
    if let Some(source) = metadata.as_object() {
        for key in [
            "kind",
            "type",
            "toolName",
            "toolCallId",
            "provider",
            "model",
            "mode",
            "effort",
            "thinking",
            "status",
            "exitCode",
            "responseId",
            "sequence",
            "receivedAt",
            "sentAt",
            "elapsedMs",
            "tokenUsage",
        ] {
            let Some(value) = source.get(key) else {
                continue;
            };
            let value = match value {
                Value::String(text) => Value::String(bound_session_response_text(
                    text,
                    4 * 1024,
                    "metadata value",
                )),
                value
                    if serde_json::to_vec(value).is_ok_and(|encoded| encoded.len() <= 4 * 1024) =>
                {
                    value.clone()
                }
                _ => continue,
            };
            bounded.insert(key.to_string(), value);
        }
    }
    bounded.insert("metadataTruncated".to_string(), Value::Bool(true));
    Value::Object(bounded)
}

fn bound_session_messages_for_response(mut messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut remaining = SESSION_RESPONSE_MAX_CONTENT_BYTES;
    for message in messages.iter_mut().rev() {
        let original_bytes = message.content.len();
        let per_message_limit = match message.role {
            MessageRole::Assistant => SESSION_RESPONSE_ASSISTANT_MAX_BYTES,
            MessageRole::Tool => SESSION_RESPONSE_TOOL_MAX_BYTES,
            MessageRole::User => SESSION_RESPONSE_USER_MAX_BYTES,
            MessageRole::System => SESSION_RESPONSE_SYSTEM_MAX_BYTES,
        };
        let allowed = per_message_limit.min(remaining);
        message.content = bound_session_response_text(
            &message.content,
            allowed,
            match message.role {
                MessageRole::Tool => "tool output",
                _ => "chat message",
            },
        );
        remaining = remaining.saturating_sub(message.content.len());
        message.metadata = bounded_session_response_metadata(&message.metadata);
        if message.content.len() < original_bytes {
            if !message.metadata.is_object() {
                message.metadata = Value::Object(serde_json::Map::new());
            }
            let metadata = message.metadata.as_object_mut().expect("metadata object");
            metadata.insert("contentTruncated".to_string(), Value::Bool(true));
            metadata.insert(
                "originalContentBytes".to_string(),
                Value::from(original_bytes as u64),
            );
        }
    }
    messages
}

async fn session_messages(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Query(query): Query<SessionMessagesQuery>,
) -> Result<Json<MessagesResponse>> {
    let offset = query.offset.unwrap_or(0);
    let limit = query
        .limit
        .unwrap_or(SESSION_HISTORY_DEFAULT_MESSAGES)
        .clamp(1, SESSION_HISTORY_MAX_MESSAGES);
    let use_tail = query.tail || (query.limit.is_none() && query.offset.is_none());
    if use_tail {
        let (messages, total_count) = state
            .sessions
            .messages_tail_including_external(&session_id, limit)
            .await?;
        let has_more = messages.len() < total_count;
        return Ok(Json(MessagesResponse {
            session_id,
            messages: bound_session_messages_for_response(messages),
            has_more,
            total_count,
        }));
    }
    let (messages, total_count) = state
        .sessions
        .messages_page_including_external(&session_id, limit, offset)
        .await?;
    let has_more = offset + messages.len() < total_count;
    Ok(Json(MessagesResponse {
        session_id,
        messages: bound_session_messages_for_response(messages),
        has_more,
        total_count,
    }))
}

async fn session_prompts(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Query(query): Query<SessionPromptsQuery>,
) -> Result<Json<PromptHistoryResponse>> {
    let limit = query
        .limit
        .unwrap_or(SESSION_PROMPT_HISTORY_DEFAULT)
        .clamp(1, SESSION_PROMPT_HISTORY_MAX);
    let before = match (
        query.before_timestamp.as_deref(),
        query.before_id.as_deref(),
    ) {
        (Some(timestamp), Some(id)) if !id.trim().is_empty() => Some(PromptHistoryCursor {
            timestamp: DateTime::parse_from_rfc3339(timestamp)
                .map_err(|_| ServerError::new(StatusCode::BAD_REQUEST, "Invalid prompt cursor."))?
                .with_timezone(&Utc),
            id: id.trim().to_string(),
        }),
        (None, None) => None,
        _ => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Prompt cursor requires before_timestamp and before_id.",
            ));
        }
    };
    let (prompts, has_more) = state
        .sessions
        .user_prompts_page_including_external(&session_id, limit, before)
        .await?;
    let oldest_cursor = prompts.first().map(|prompt| PromptHistoryCursor {
        timestamp: prompt.timestamp,
        id: prompt.id.clone(),
    });
    Ok(Json(PromptHistoryResponse {
        session_id,
        prompts: prompts
            .into_iter()
            .map(|mut prompt| {
                prompt.content = bound_session_response_text(
                    &prompt.content,
                    SESSION_RESPONSE_USER_MAX_BYTES,
                    "chat prompt",
                );
                prompt
            })
            .collect(),
        has_more,
        oldest_cursor,
    }))
}

async fn session_snapshot(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Query(query): Query<SessionMessagesQuery>,
) -> Result<Json<SessionSnapshotResponse>> {
    let limit = query
        .limit
        .unwrap_or(SESSION_HISTORY_DEFAULT_MESSAGES)
        .clamp(1, SESSION_HISTORY_MAX_MESSAGES);
    let session_before = state.sessions.get(&session_id).await?;
    let (mut messages, mut total_count) = state
        .sessions
        .messages_tail_including_external(&session_id, limit)
        .await?;
    let mut session = state.sessions.get(&session_id).await?;
    // Finishing a run persists the assistant reply before marking the
    // session inactive. If that transition happened between the two reads,
    // fetch the messages once more so an inactive snapshot can never omit
    // the final reply.
    if session_before.active && !session.active {
        let refreshed = state
            .sessions
            .messages_tail_including_external(&session_id, limit)
            .await?;
        messages = refreshed.0;
        total_count = refreshed.1;
    }
    session.message_count = total_count;
    Ok(Json(SessionSnapshotResponse {
        session,
        has_more: messages.len() < total_count,
        messages: bound_session_messages_for_response(messages),
        total_count,
        recovery: state.context_recovery(&session_id).await?,
    }))
}

fn validate_context_compaction_request_id(value: &str) -> Result<&str> {
    let request_id = value.trim();
    if request_id.is_empty()
        || request_id.len() > 200
        || !request_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "requestId must be a non-empty stable identifier",
        ));
    }
    Ok(request_id)
}

async fn compact_session_context(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<ManualCompactSessionContextRequest>,
) -> Result<(StatusCode, Json<CompactSessionContextResponse>)> {
    validate_session_id(&session_id)?;
    let request_id = validate_context_compaction_request_id(&request.request_id)?;
    let session = state.sessions.get(&session_id).await?;
    let runtime = session
        .runtime
        .unwrap_or_else(|| configured_chat_runtime(&state, &user.0.id));
    let direct_ai_config = (runtime == ChatRuntime::IoGateway)
        .then(|| direct_ai_runtime_config_for_user(&state, &user.0.id, Provider::Codex))
        .flatten();
    let response = state
        .compact_session_context(&user.0.id, &session_id, request_id, direct_ai_config)
        .await?;
    publish_projects(&state).await;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn compact_and_retry_session_context(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<CompactSessionContextRequest>,
) -> Result<(StatusCode, Json<CompactSessionContextResponse>)> {
    validate_session_id(&session_id)?;
    let request_id = validate_context_compaction_request_id(&request.request_id)?;
    let failed_message_id = request.failed_message_id.trim();
    if failed_message_id.is_empty() || failed_message_id.len() > 1_000 {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "failedMessageId must be a non-empty message id",
        ));
    }
    let session = state.sessions.get(&session_id).await?;
    let runtime = session
        .runtime
        .unwrap_or_else(|| configured_chat_runtime(&state, &user.0.id));
    let direct_ai_config = (runtime == ChatRuntime::IoGateway)
        .then(|| direct_ai_runtime_config_for_user(&state, &user.0.id, Provider::Codex))
        .flatten();
    let response = state
        .compact_and_retry_session_context(
            &user.0.id,
            &session_id,
            failed_message_id,
            request_id,
            direct_ai_config,
        )
        .await?;
    publish_projects(&state).await;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn fork_session(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<ForkSessionRequest>,
) -> Result<Json<ForkSessionResponse>> {
    validate_session_id(&session_id)?;
    let before_message_id = request.before_message_id.trim();
    if before_message_id.is_empty() || before_message_id.len() > 1_000 {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "beforeMessageId must be a non-empty message id",
        ));
    }
    let request_id = request.request_id.trim();
    if request_id.is_empty()
        || request_id.len() > 200
        || !request_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "requestId must be a non-empty stable identifier",
        ));
    }
    if request
        .draft_content
        .as_ref()
        .is_some_and(|content| content.len() > SESSION_DRAFT_MAX_BYTES)
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!("draftContent exceeds {SESSION_DRAFT_MAX_BYTES} bytes"),
        ));
    }
    let response = state
        .fork_session_before_message(
            &user.0.id,
            &session_id,
            before_message_id,
            request_id,
            request.replace,
            request.draft_content.as_deref(),
        )
        .await?;
    publish_projects(&state).await;
    Ok(Json(response))
}

async fn get_session_draft(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(session_id): AxumPath<String>,
) -> Result<Json<SessionDraftResponse>> {
    validate_session_id(&session_id)?;
    let _ = state.sessions.get(&session_id).await?;
    Ok(Json(
        state.storage.get_session_draft(&user.0.id, &session_id)?,
    ))
}

async fn update_session_draft(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<UpdateSessionDraftRequest>,
) -> Result<Json<SessionDraftResponse>> {
    validate_session_id(&session_id)?;
    let _ = state.sessions.get(&session_id).await?;
    if request.content.len() > SESSION_DRAFT_MAX_BYTES {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!("session draft exceeds {} bytes", SESSION_DRAFT_MAX_BYTES),
        ));
    }
    Ok(Json(state.storage.set_session_draft(
        &user.0.id,
        &session_id,
        &request.content,
    )?))
}

async fn delete_session_draft(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(session_id): AxumPath<String>,
) -> Result<Json<PlaceholderResponse>> {
    validate_session_id(&session_id)?;
    state
        .storage
        .delete_session_draft(&user.0.id, &session_id)?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "session draft cleared".to_string(),
    }))
}

async fn session_model(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Query(query): Query<SessionProviderQuery>,
) -> Result<Json<Value>> {
    validate_session_id(&session_id)?;
    if let Some(provider) = query.provider.as_deref() {
        validate_provider_name(provider)?;
    }
    let session = state.sessions.get(&session_id).await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "model": session.model,
    })))
}

async fn update_session_model(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<UpdateSessionModelRequest>,
) -> Result<Json<Value>> {
    validate_session_id(&session_id)?;
    if let Some(provider) = request.provider.as_deref() {
        validate_provider_name(provider)?;
    }
    let model = request.model.trim();
    if model.is_empty() || model.chars().count() > MAX_SESSION_MODEL_LENGTH {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!("Model must be a non-empty string up to {MAX_SESSION_MODEL_LENGTH} characters"),
        ));
    }
    let session = state
        .sessions
        .update_model(&session_id, Some(model.to_string()))
        .await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "model": session.model,
    })))
}

async fn rename_session(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<RenameSessionRequest>,
) -> Result<Json<Value>> {
    validate_session_id(&session_id)?;
    validate_provider_name(&request.provider)?;
    let title = request.summary.trim();
    if title.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Summary is required",
        ));
    }
    if title.chars().count() > MAX_SESSION_TITLE_LENGTH {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!("Summary must not exceed {MAX_SESSION_TITLE_LENGTH} characters"),
        ));
    }
    let session = state
        .sessions
        .rename(&session_id, title.to_string())
        .await?;
    state.ws_hub.publish(WsServerEvent::ActiveSessions {
        sessions: state.sessions.list_active().await,
    });
    publish_projects(&state).await;
    Ok(Json(serde_json::json!({
        "success": true,
        "session": session,
    })))
}

async fn delete_session(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> Result<Json<Value>> {
    validate_session_id(&session_id)?;
    let session = state.sessions.get(&session_id).await?;
    if session.active {
        let _ = state
            .abort_agent_session(session.provider, &session_id)
            .await;
    }
    let deleted = state.sessions.delete(&session_id).await?;
    state.ws_hub.publish(WsServerEvent::ActiveSessions {
        sessions: state.sessions.list_active().await,
    });
    publish_projects(&state).await;
    Ok(Json(serde_json::json!({
        "success": true,
        "session": deleted,
    })))
}

async fn search_conversations(
    State(state): State<AppState>,
    Query(query): Query<SearchConversationsQuery>,
) -> Result<Json<Value>> {
    let q = query.q.trim();
    if q.chars().count() < 2 {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Query must be at least 2 characters",
        ));
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let results = state
        .storage
        .search_messages(q, limit)?
        .into_iter()
        .map(|(session, message)| {
            serde_json::json!({
                "sessionId": session.id,
                "sessionTitle": session.title,
                "provider": session.provider,
                "projectPath": session.project_path,
                "messageId": message.id,
                "role": message.role,
                "content": message.content,
                "timestamp": message.timestamp,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!({
        "success": true,
        "query": q,
        "totalMatches": results.len(),
        "results": results,
    })))
}

#[derive(Debug, Deserialize)]
struct SessionProviderQuery {
    provider: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateSessionModelRequest {
    provider: Option<String>,
    model: String,
}

#[derive(Debug, Deserialize)]
struct RenameSessionRequest {
    provider: String,
    summary: String,
}

#[derive(Debug, Deserialize)]
struct SearchConversationsQuery {
    q: String,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TokenUsageQuery {
    provider: Option<String>,
}

#[derive(Debug, Clone)]
struct TokenUsageSnapshot {
    usage: SessionTokenUsage,
    total: u64,
}

async fn session_token_usage(
    State(state): State<AppState>,
    AxumPath((_project_name, session_id)): AxumPath<(String, String)>,
    Query(query): Query<TokenUsageQuery>,
) -> Result<Json<Value>> {
    validate_session_id(&session_id)?;
    let session = state.sessions.get(&session_id).await?;
    let provider = query
        .provider
        .as_deref()
        .map(parse_provider_param)
        .transpose()?
        .unwrap_or(session.provider);

    if provider == Provider::Gemini {
        return Ok(Json(serde_json::json!({
            "used": 0,
            "total": 0,
            "breakdown": { "input": 0, "output": 0, "cacheCreation": 0, "cacheRead": 0 },
            "unsupported": true,
            "message": "Token usage tracking not available for Gemini sessions",
        })));
    }

    let persisted_usage = state
        .storage
        .latest_session_token_usage(&session.id)?
        .or_else(|| session.token_usage.clone());
    let snapshot = if let Some(usage) = persisted_usage {
        TokenUsageSnapshot {
            usage,
            total: match provider {
                Provider::Claude => env::var("CONTEXT_WINDOW")
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(160_000),
                Provider::Codex => 200_000,
                Provider::Gemini => 0,
            },
        }
    } else {
        match provider {
            Provider::Codex => {
                let session_file = state
                    .sessions
                    .external_session_file(&session.id)
                    .await
                    .ok_or_else(|| {
                        ServerError::new(StatusCode::NOT_FOUND, "Codex session file not found")
                    })?;
                codex_token_usage(&session_file).await?
            }
            Provider::Claude => {
                let native_session_id = session
                    .native_session_id
                    .as_deref()
                    .unwrap_or(session.id.as_str());
                claude_token_usage(&session.project_path, native_session_id).await?
            }
            Provider::Gemini => unreachable!(),
        }
    };

    state
        .sessions
        .set_token_usage(&session_id, snapshot.usage.clone())
        .await?;
    let stamp = serde_json::json!({ "tokenUsage": &snapshot.usage });
    if let Err(error) =
        state
            .sessions
            .stamp_latest_message_metadata(&session_id, MessageRole::Assistant, stamp)
    {
        warn!(error = %error, session_id = %session_id, "failed to stamp token usage on assistant message");
    }

    Ok(Json(token_usage_response(&snapshot)))
}

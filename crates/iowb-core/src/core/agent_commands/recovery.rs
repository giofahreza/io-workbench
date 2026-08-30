fn expand_agent_template(
    value: String,
    prompt: &str,
    session_id: &str,
    model: Option<&str>,
    native_resume_session_id: Option<&str>,
) -> String {
    value
        .replace("{prompt}", prompt)
        .replace("{session_id}", session_id)
        .replace("{model}", model.unwrap_or(""))
        .replace(
            "{native_session_id}",
            native_resume_session_id.unwrap_or(""),
        )
        .replace(
            "{resume_session_id}",
            native_resume_session_id.unwrap_or(""),
        )
}

fn agent_run_key(provider: Provider, session_id: &str) -> String {
    format!("{}:{session_id}", provider.as_str())
}

fn parse_stored_provider(provider: &str) -> Result<Provider> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "claude" => Ok(Provider::Claude),
        "codex" => Ok(Provider::Codex),
        "gemini" => Ok(Provider::Gemini),
        _ => Err(CoreError::InvalidInput(format!(
            "unsupported durable chat provider: {provider}"
        ))),
    }
}

fn durable_chat_recovery_prompt(original_prompt: &str) -> String {
    let mut clipped = original_prompt
        .chars()
        .take(DURABLE_CHAT_RUN_RECOVERY_PROMPT_LIMIT)
        .collect::<String>();
    if original_prompt.chars().count() > DURABLE_CHAT_RUN_RECOVERY_PROMPT_LIMIT {
        clipped.push_str("\n[original request truncated]");
    }
    // Keep the internal instruction as one complete reminder block so native
    // session readers can filter it from the visible user transcript.
    clipped = clipped.replace("</system-reminder>", "&lt;/system-reminder&gt;");
    format!(
        "<system-reminder>\nThe io-workbench Rust server was forced to stop while the previous turn was still running. Continue the interrupted task now in the current repository and conversation. Inspect the current files and state before acting, avoid repeating work that is already complete, and finish the original request. Do not ask the user to resend it.\n\nOriginal user request:\n{clipped}\n</system-reminder>"
    )
}

fn codex_rollout_user_message(timestamp: DateTime<Utc>, prompt: &str) -> Value {
    serde_json::json!({
        "timestamp": timestamp.to_rfc3339(),
        "type": "event_msg",
        "payload": {
            "type": "user_message",
            "message": prompt,
            "kind": "plain",
            "source": "io-workbench"
        }
    })
}

fn codex_rollout_assistant_message(timestamp: DateTime<Utc>, assistant_output: &str) -> Value {
    serde_json::json!({
        "timestamp": timestamp.to_rfc3339(),
        "type": "response_item",
        "payload": {
            "type": "message",
            "id": format!("msg_{}", Uuid::new_v4().simple()),
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": assistant_output
            }],
            "source": "io-workbench"
        }
    })
}

fn append_codex_rollout_entries(path: &Path, entries: &[Value]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut file = OpenOptions::new().append(true).open(path)?;
    for entry in entries {
        writeln!(file, "{}", serde_json::to_string(entry)?)?;
    }
    file.flush()?;
    Ok(())
}

fn is_codex_assistant_response(message: &ChatMessage) -> bool {
    if message.role != MessageRole::Assistant {
        return false;
    }
    if message.metadata.get("kind").and_then(Value::as_str) == Some("thinking")
        || message.metadata.get("phase").and_then(Value::as_str) == Some("commentary")
    {
        return false;
    }
    !message.content.trim_start().starts_with("thinking\n")
}

fn select_completed_agent_output(
    runtime_provider: Provider,
    provider_specific_final: Option<String>,
    accumulated_output: &str,
    codex_saw_structured_event: bool,
) -> std::result::Result<String, String> {
    if runtime_provider == Provider::Codex {
        if let Some(final_output) = provider_specific_final {
            return Ok(final_output);
        }
        if codex_saw_structured_event || looks_like_codex_live_transcript(accumulated_output) {
            return Err(CODEX_MISSING_FINAL_RESPONSE.to_string());
        }
    }
    Ok(provider_specific_final.unwrap_or_else(|| accumulated_output.to_string()))
}

fn append_bounded(output: &mut String, chunk: &str, max_bytes: usize) {
    output.push_str(chunk);
    if output.len() > max_bytes {
        let overflow = output.len() - max_bytes;
        let trim_at = output
            .char_indices()
            .map(|(index, _)| index)
            .find(|index| *index >= overflow)
            .unwrap_or(overflow);
        output.drain(..trim_at);
    }
}

fn sanitize_agent_text(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len().min(AGENT_LIVE_EVENT_MAX_BYTES));
    let mut line_chars = 0;
    for character in value.chars() {
        if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
            continue;
        }
        if character == '\n' || character == '\r' {
            line_chars = 0;
        } else {
            line_chars += 1;
            if line_chars > AGENT_DISPLAY_MAX_LINE_CHARS {
                sanitized.push_str("\n[long line wrapped for display]\n");
                line_chars = 1;
            }
        }
        sanitized.push(character);
    }
    sanitized
}

async fn activate_completed_context_rollover(
    context: &AgentStartContext,
    rollover_id: &str,
    activated_at: DateTime<Utc>,
) -> Result<Option<ContextRolloverFollowUp>> {
    let retry_run_id = context
        .durable_run_id
        .as_deref()
        .ok_or_else(|| CoreError::InvalidInput("rollover run id is missing".to_string()))?;
    let rollover = context
        .storage
        .context_rollover_for_retry_run(retry_run_id)?
        .filter(|rollover| rollover.id == rollover_id)
        .ok_or_else(|| CoreError::InvalidInput("context rollover was not found".to_string()))?;
    let candidate = rollover
        .candidate_native_session_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CoreError::InvalidInput("Codex did not report a clean thread id".to_string())
        })?;
    let mut session = context
        .storage
        .get_session_summary(&context.session_id)?
        .ok_or_else(|| CoreError::SessionNotFound(context.session_id.clone()))?;
    session.native_session_id = Some(candidate.clone());
    session.external = false;
    session.active = false;
    session.last_activity = activated_at;
    session.last_message_at = Some(activated_at);
    session.received_at = Some(activated_at);
    session.effort = context.effort.clone().or(session.effort);
    session.mode = context.mode.clone().or(session.mode);
    session.thinking = context.thinking.or(session.thinking);
    session.fast = context.fast.or(session.fast);
    let observed = rollover
        .observed_bytes
        .map(|bytes| {
            format!(
                " · {} of image-heavy context archived",
                human_byte_size(bytes)
            )
        })
        .unwrap_or_default();
    let marker = ChatMessage {
        id: new_id("msg"),
        role: MessageRole::System,
        content: format!(
            "Context compacted here{observed}. Earlier messages remain visible, while subsequent replies use a clean Codex context."
        ),
        timestamp: activated_at,
        metadata: serde_json::json!({
            "kind": "context_compaction",
            "rolloverKind": rollover.kind.clone(),
            "rolloverId": rollover.id,
            "requestId": rollover.request_id,
            "failedMessageId": rollover.failed_message_id,
            "fromNativeSessionId": rollover.from_native_session_id,
            "toNativeSessionId": candidate.clone(),
            "observedBytes": rollover.observed_bytes,
            "limitBytes": rollover.limit_bytes,
        }),
    };
    if rollover.kind == CONTEXT_ROLLOVER_KIND_MANUAL {
        if !context.storage.complete_context_rollover(
            rollover_id,
            retry_run_id,
            &candidate,
            &session,
            &marker,
            None,
            None,
        )? {
            return Err(CoreError::Conflict(
                "manual context compaction is no longer pending".to_string(),
            ));
        }
        let persisted_session = context
            .storage
            .get_session_summary(&context.session_id)?
            .ok_or_else(|| CoreError::SessionNotFound(context.session_id.clone()))?;
        context
            .sessions
            .remember_persisted_session(persisted_session)
            .await?;
        info!(
            session_id = %context.session_id,
            rollover_id,
            native_session_id = %candidate,
            "activated manual clean native context"
        );
        return Ok(None);
    }
    if rollover.kind != CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN {
        return Err(CoreError::InvalidInput(
            "unknown context rollover kind".to_string(),
        ));
    }
    let follow_up_run = match context
        .storage
        .message_by_id(&context.session_id, &rollover.failed_message_id)?
    {
        Some(message) if message.role == MessageRole::User => {
            let mut run = StoredDurableChatRun::new(
                new_id("run"),
                Some(rollover.user_id.clone()),
                session.id.clone(),
                context.provider.as_str(),
                message.content.clone(),
                session.project_path.clone(),
            );
            run.user_message_id = Some(message.id.clone());
            run.native_session_id = Some(candidate.clone());
            run.model = context.model.clone();
            run.effort = context.effort.clone();
            run.mode = context.mode.clone();
            run.thinking = context.thinking;
            run.fast = context.fast;
            Some(run)
        }
        Some(_) => {
            return Err(CoreError::InvalidInput(
                "failed message was not a user message".to_string(),
            ));
        }
        None => None,
    };
    if !context.storage.complete_context_rollover(
        rollover_id,
        retry_run_id,
        &candidate,
        &session,
        &marker,
        None,
        follow_up_run.as_ref(),
    )? {
        return Err(CoreError::Conflict(
            "clean context rollover is no longer pending".to_string(),
        ));
    }
    let persisted_session = context
        .storage
        .get_session_summary(&context.session_id)?
        .ok_or_else(|| CoreError::SessionNotFound(context.session_id.clone()))?;
    context
        .sessions
        .remember_persisted_session(persisted_session)
        .await?;
    info!(
        session_id = %context.session_id,
        rollover_id,
        native_session_id = %candidate,
        retry_staged = follow_up_run.is_some(),
        "activated clean native context"
    );
    if follow_up_run.is_none() {
        warn!(
            session_id = %context.session_id,
            rollover_id,
            failed_message_id = %rollover.failed_message_id,
            "clean context activated without retrying missing failed prompt"
        );
    }
    Ok(follow_up_run.map(|run| ContextRolloverFollowUp { run }))
}

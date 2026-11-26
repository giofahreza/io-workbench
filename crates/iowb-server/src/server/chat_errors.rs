/// A first-send rejection is a recoverable draft, not an executed native turn.
fn rejected_chat_draft(state: &AppState, session: &SessionSummary) -> bool {
    session.native_session_id.is_none()
        && session.first_user_at.is_none()
        && session.message_count > 0
        && state
            .storage
            .latest_user_message_id(&session.id)
            .ok()
            .flatten()
            .is_none()
}

#[allow(clippy::too_many_arguments)]
async fn persist_rejected_chat_send(
    state: &AppState,
    user_id: &str,
    provider: Provider,
    runtime: ChatRuntime,
    session_id: &str,
    request_id: Option<&str>,
    project_path: &str,
    prompt: &str,
    problem: &Value,
) -> Result<ChatMessage> {
    let path = state
        .path_validator
        .validate_path(PathBuf::from(project_path), false)
        .await?;
    if !path.is_dir() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Project path must be a directory.",
        ));
    }
    let existing = match state.storage.get_session_summary(session_id)? {
        Some(session) => Some(session),
        None => state.sessions.get(session_id).await.ok(),
    };
    let session = existing.unwrap_or_else(|| SessionSummary {
        id: session_id.to_string(),
        provider,
        project_path: path.display().to_string(),
        title: iowb_protocol::session_title_from_prompt(prompt)
            .unwrap_or_else(|| "Unsent chat".into()),
        title_source: Some(iowb_protocol::SessionTitleSource::Prompt),
        runtime: Some(runtime),
        last_activity: Utc::now(),
        active: false,
        ..Default::default()
    });
    if Path::new(&session.project_path) != path {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Chat belongs to another project.",
        ));
    }
    let id = request_id
        .filter(|id| id.len() <= 128)
        .map(|id| format!("chat-error:{session_id}:{id}"))
        .unwrap_or_else(|| new_id("chat-error"));
    if let Some(existing) = state.storage.message_by_id(session_id, &id)? {
        return Ok(existing);
    }
    let message = ChatMessage {
        id,
        role: MessageRole::System,
        content: problem["message"]
            .as_str()
            .unwrap_or("Chat could not be started.")
            .chars()
            .take(4096)
            .collect(),
        timestamp: Utc::now(),
        metadata: serde_json::json!({
            "kind": "chat_error", "phase": "rejected", "status": "failed",
            "code": problem["code"], "action": problem["action"], "docsUrl": problem["docsUrl"],
            "requestId": request_id, "provider": provider, "runtime": runtime,
        }),
    };
    state
        .storage
        .persist_chat_error(&session, &message, Some((user_id, prompt)))?;
    if let Some(saved) = state.storage.get_session_summary(session_id)? {
        state.sessions.remember_persisted_session(saved).await?;
    }
    Ok(message)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatCommandsQuery {
    #[serde(flatten)]
    context: iowb_protocol::ChatCommandContext,
    #[serde(default)]
    refresh: bool,
}

async fn resolve_command_context(
    state: &AppState,
    user_id: &str,
    context: &mut iowb_protocol::ChatCommandContext,
) -> Result<()> {
    if let Some(id) = context.session_id.as_deref() {
        validate_session_id(id)?;
    }
    context.runtime = state.native_chat_configuration(context.provider, context.session_id.as_deref()).await
        .and_then(|session| session.runtime).unwrap_or_else(|| resolve_session_chat_runtime(
        state,
        user_id,
        context.provider,
        context.session_id.as_deref(),
        context.model.as_deref(),
    ));
    Ok(())
}

async fn chat_commands_catalog(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(mut query): Query<ChatCommandsQuery>,
) -> Result<Json<iowb_protocol::ChatCommandCatalog>> {
    resolve_command_context(&state, &user.0.id, &mut query.context).await?;
    Ok(Json(
        state
            .chat_command_catalog(&query.context, &user.0.id, query.refresh)
            .await?,
    ))
}

async fn prepare_chat_command(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(mut request): Json<iowb_protocol::PrepareChatCommandRequest>,
) -> Result<Json<iowb_protocol::PreparedChatCommand>> {
    if request.prompt.len() > 256 * 1024 {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "command input is too large",
        ));
    }
    resolve_command_context(&state, &user.0.id, &mut request.context).await?;
    state
        .chat_command_catalog(&request.context, &user.0.id, true)
        .await?;
    Ok(Json(
        state.prepare_chat_command(&request, &user.0.id).await?,
    ))
}

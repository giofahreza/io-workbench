async fn recover_interrupted_chat_runs(state: &AppState) -> anyhow::Result<()> {
    synthesize_legacy_durable_runs(state).await?;
    let reconciled_manual_sessions = reconcile_stale_manual_context_rollovers(state).await?;
    let mut active_runs = state.storage.list_active_durable_chat_runs()?;
    if active_runs.is_empty() {
        state
            .sessions
            .mark_unrecovered_active_sessions_interrupted(&reconciled_manual_sessions)
            .await?;
        return Ok(());
    }

    // A session can only have one provider invocation attached. Older active
    // rows can exist if a process was killed between superseding one turn and
    // committing the next; keep the newest row and terminalize the rest.
    let mut newest_run_by_session = HashMap::<String, String>::new();
    for run in &active_runs {
        let replace = newest_run_by_session
            .get(&run.session_id)
            .and_then(|id| active_runs.iter().find(|candidate| &candidate.id == id))
            .is_none_or(|current| {
                (run.created_at, run.id.as_str()) > (current.created_at, current.id.as_str())
            });
        if replace {
            newest_run_by_session.insert(run.session_id.clone(), run.id.clone());
        }
    }
    active_runs.sort_by_key(|run| (run.created_at, run.id.clone()));

    let mut recovered_session_ids = HashSet::new();
    for run in active_runs {
        if newest_run_by_session.get(&run.session_id) != Some(&run.id) {
            state.storage.mark_durable_chat_run_interrupted(
                &run.id,
                Some("superseded by a newer interrupted turn in the same session"),
            )?;
            continue;
        }
        if !run.auto_resume || run.resume_attempts >= DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS {
            state.storage.mark_durable_chat_run_interrupted(
                &run.id,
                Some(if run.auto_resume {
                    "automatic recovery attempt limit reached"
                } else {
                    "automatic recovery is disabled"
                }),
            )?;
            continue;
        }

        let cleanup = terminate_orphaned_agent_run_processes(&run.id, state.storage.path());
        if cleanup.live_owner {
            recovered_session_ids.insert(run.session_id.clone());
            info!(
                run_id = %run.id,
                session_id = %run.session_id,
                "left durable chat run attached to its live server owner"
            );
            continue;
        }

        let Some(claimed) = state
            .storage
            .mark_durable_chat_run_recovering(&run.id, DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS)?
        else {
            continue;
        };
        let direct_ai_config = parse_provider_param(&claimed.provider)
            .ok()
            .and_then(|provider| {
                claimed
                    .user_id
                    .as_deref()
                    .and_then(|user_id| direct_ai_runtime_config_for_user(state, user_id, provider))
            });
        match state
            .recover_agent_run(claimed.clone(), direct_ai_config)
            .await
        {
            Ok(session) => {
                if session.active {
                    recovered_session_ids.insert(session.id);
                }
            }
            Err(error) => {
                let message = error.to_string();
                state
                    .storage
                    .mark_durable_chat_run_failed(&claimed.id, &message)?;
                let _ = state.sessions.set_active(&claimed.session_id, false).await;
                warn!(
                    error = %error,
                    run_id = %claimed.id,
                    session_id = %claimed.session_id,
                    "failed to recover interrupted chat run"
                );
            }
        }
    }

    recovered_session_ids.extend(reconciled_manual_sessions);
    recovered_session_ids.extend(reconcile_stale_manual_context_rollovers(state).await?);
    let interrupted = state
        .sessions
        .mark_unrecovered_active_sessions_interrupted(&recovered_session_ids)
        .await?;
    info!(
        recovered = recovered_session_ids.len(),
        interrupted = interrupted.len(),
        "reconciled chat runs after server restart"
    );
    Ok(())
}

async fn reconcile_stale_manual_context_rollovers(
    state: &AppState,
) -> anyhow::Result<HashSet<String>> {
    let session_ids = state.storage.reconcile_stale_manual_context_rollovers()?;
    for session_id in &session_ids {
        state.sessions.set_active(session_id, false).await?;
    }
    if !session_ids.is_empty() {
        info!(
            sessions = session_ids.len(),
            "reconciled stale manual context compactions after server restart"
        );
    }
    Ok(session_ids.into_iter().collect())
}

async fn synthesize_legacy_durable_runs(state: &AppState) -> anyhow::Result<()> {
    let durable_session_ids = state
        .storage
        .list_active_durable_chat_runs()?
        .into_iter()
        .map(|run| run.session_id)
        .collect::<HashSet<_>>();
    let fallback_user_id = state
        .storage
        .get_first_user()?
        .map(|user| user.id)
        .or_else(|| Some("local".to_string()));

    for session in state
        .storage
        .list_sessions_including_board()?
        .into_iter()
        .filter(|session| session.active && !durable_session_ids.contains(&session.id))
    {
        // Workbench-local rows are authoritative here. Native CLI history can
        // already contain partial assistant output from the interrupted turn,
        // which must not be mistaken for a completed response.
        let messages = state.storage.list_messages(&session.id)?;
        let last_conversation_message = messages.iter().rev().find(|message| {
            matches!(message.role, MessageRole::User | MessageRole::Assistant)
                && !message.content.trim().is_empty()
        });
        let Some(last_user_prompt) = last_conversation_message
            .filter(|message| message.role == MessageRole::User)
            .map(|message| message.content.clone())
        else {
            // A final assistant row means the old process most likely died
            // after persistence but before clearing the active bit. There is
            // nothing left for the provider to continue.
            if last_conversation_message
                .is_some_and(|message| message.role == MessageRole::Assistant)
            {
                let _ = state.sessions.set_active(&session.id, false).await;
            }
            continue;
        };

        let mut run = iowb_storage::StoredDurableChatRun::new(
            new_id("run"),
            fallback_user_id.clone(),
            session.id.clone(),
            session.provider.as_str(),
            last_user_prompt,
            session.project_path.clone(),
        );
        run.native_session_id = session
            .native_session_id
            .clone()
            .or_else(|| session.external.then(|| session.id.clone()));
        run.model = session.model.clone();
        run.effort = session.effort.clone();
        run.mode = session.mode.clone();
        run.thinking = session.thinking;
        state.storage.create_durable_chat_run(&run)?;
        info!(
            run_id = %run.id,
            session_id = %session.id,
            provider = session.provider.as_str(),
            "created durable recovery record for legacy active chat session"
        );
    }
    Ok(())
}

pub fn build_router(state: AppState) -> Router {
    agentic_board::recover_active_boards(&state);

    let protected_routes = Router::new()
        .route("/api/auth/logout", post(auth_logout))
        .route("/api/auth/user", get(auth_user))
        .route("/api/projects", get(list_projects))
        .route("/api/projects/create", post(create_project))
        .route("/api/projects/create-workspace", post(create_workspace))
        .route("/api/projects/clone-progress", get(clone_progress))
        .route(
            "/api/projects/{project_name}/sessions",
            get(project_sessions),
        )
        .route(
            "/api/projects/{project_name}",
            patch(rename_project).delete(delete_project),
        )
        .route("/api/projects/{project_name}/file", put(write_project_file))
        .route(
            "/api/projects/{project_name}/files",
            get(list_project_files).delete(delete_project_file),
        )
        .route(
            "/api/projects/{project_name}/files/content",
            get(read_project_file),
        )
        .route(
            "/api/projects/{project_name}/files/raw",
            get(stream_project_file),
        )
        .route(
            "/api/projects/{project_name}/files/create",
            post(create_project_file),
        )
        .route(
            "/api/projects/{project_name}/files/rename",
            put(rename_project_file),
        )
        .route(
            "/api/projects/{project_name}/files/rename-batch",
            put(rename_project_files_batch),
        )
        .route(
            "/api/projects/{project_name}/files/copy",
            post(copy_project_file),
        )
        .route(
            "/api/projects/{project_name}/files/copy-batch",
            post(copy_project_files_batch),
        )
        .route(
            "/api/projects/{project_name}/files/delete-batch",
            post(delete_project_files_batch),
        )
        .route(
            "/api/projects/{project_name}/files/upload",
            post(files_upload),
        )
        .route(
            "/api/projects/{project_name}/upload-images",
            post(upload_images),
        )
        .route(
            "/api/projects/{project_name}/sessions/{session_id}/token-usage",
            get(session_token_usage),
        )
        .route("/api/sessions/{session_id}", delete(delete_session))
        .route("/api/sessions/{session_id}/messages", get(session_messages))
        .route("/api/sessions/{session_id}/prompts", get(session_prompts))
        .route("/api/sessions/{session_id}/snapshot", get(session_snapshot))
        .route("/api/sessions/{session_id}/fork", post(fork_session))
        .route(
            "/api/sessions/{session_id}/compact",
            post(compact_session_context),
        )
        .route(
            "/api/sessions/{session_id}/compact-and-retry",
            post(compact_and_retry_session_context),
        )
        .route(
            "/api/sessions/{session_id}/draft",
            get(get_session_draft)
                .put(update_session_draft)
                .delete(delete_session_draft),
        )
        .route("/api/sessions/{session_id}/model", get(session_model))
        .route(
            "/api/sessions/{session_id}/model",
            put(update_session_model),
        )
        .route("/api/sessions/{session_id}/rename", put(rename_session))
        .route("/api/browse-filesystem", get(browse_filesystem))
        .route("/api/create-folder", post(create_folder))
        .route("/api/search/conversations", get(search_conversations))
        .route("/api/audio/transcribe", post(audio_transcribe))
        .merge(git::router())
        .route("/api/settings/server-status", get(server_status))
        .route("/api/metrics/runtime", get(runtime_metrics))
        .route(
            "/api/settings/mobile-overview",
            get(mobile_settings_overview),
        )
        .route("/api/settings", get(list_settings))
        .route(
            "/api/settings/value/{key}",
            get(get_setting).put(set_setting),
        )
        .route(
            "/api/settings/notification-preferences",
            get(get_notification_preferences).put(set_notification_preferences),
        )
        .route("/api/settings/agent/{provider}", put(set_agent_preferences))
        .route(
            "/api/settings/sidebar-active-sessions",
            get(get_sidebar_active_sessions).put(set_sidebar_active_sessions),
        )
        .route(
            "/api/settings/direct-ai",
            get(get_direct_ai).put(set_direct_ai),
        )
        .route("/api/settings/direct-ai/models", get(direct_ai_models))
        .route("/api/chat/models", get(chat_provider_models))
        .route(
            "/api/settings/api-keys",
            get(list_api_keys).post(create_api_key),
        )
        .route("/api/settings/api-keys/{key_id}", delete(delete_api_key))
        .route(
            "/api/settings/api-keys/{key_id}/toggle",
            patch(toggle_api_key),
        )
        .route(
            "/api/settings/credentials",
            get(list_credentials).post(create_credential),
        )
        .route(
            "/api/settings/credentials/{credential_id}",
            delete(delete_credential),
        )
        .route(
            "/api/settings/credentials/{credential_id}/toggle",
            patch(toggle_credential),
        )
        .route("/api/settings/{*path}", any(settings_compat))
        .route("/api/process", get(list_processes).post(start_process))
        .route("/api/process/{process_id}", delete(abort_process))
        .route("/api/process/{process_id}/input", post(write_process_input))
        .route("/api/process/{process_id}/resize", post(resize_process))
        .merge(database::router())
        .route(
            "/api/mcp/servers",
            get(list_mcp_servers).post(start_mcp_server),
        )
        .route("/api/mcp/servers/{server_id}", delete(stop_mcp_server))
        .route("/api/mcp/tools/call", post(call_mcp_tool))
        .route("/api/mcp-utils/run", post(run_mcp_utils))
        .route("/api/commands/run", post(run_slash_command))
        .route("/api/commands/taskmaster/run", post(run_taskmaster))
        .route("/api/taskmaster/run", post(run_taskmaster))
        .route("/api/plugins/install", post(install_plugin))
        .route("/api/plugins/remove", post(remove_plugin))
        .route("/api/plugins/run", post(run_plugin_command))
        .merge(agentic_board::router())
        .route(
            "/api/devices/fcm-token",
            post(register_fcm_token).delete(delete_fcm_token),
        )
        .route("/api/notifications/push", post(send_push_notification))
        .route("/api/notifications/test", post(test_push_notification))
        .route("/api/tool-runs/{namespace}", get(list_tool_runs))
        .route("/api/agent", any(agent_compat))
        .route("/api/agent/{*path}", any(agent_compat))
        .route("/api/mcp", any(mcp_compat))
        .route("/api/mcp/{*path}", any(mcp_compat))
        .route("/api/mcp-utils", any(mcp_utils_compat))
        .route("/api/mcp-utils/{*path}", any(mcp_utils_compat))
        .route("/api/commands", any(commands_compat))
        .route("/api/commands/{*path}", any(commands_compat))
        .route("/api/cli/{provider}/status", get(cli_provider_status))
        .route("/api/cli", get(cli_overview))
        .route(
            "/api/user/git-config",
            get(get_git_config).post(set_git_config),
        )
        .route("/api/user/onboarding-status", get(onboarding_status))
        .route("/api/user/complete-onboarding", post(complete_onboarding))
        .route("/api/user", get(user_settings_overview))
        .route("/api/codex", any(provider_compat))
        .route("/api/codex/{*path}", any(provider_compat))
        .route("/api/gemini", any(provider_compat))
        .route("/api/gemini/{*path}", any(provider_compat))
        .route("/api/plugins", any(plugins_compat))
        .route("/api/plugins/{*path}", any(plugins_compat))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .route("/health", get(health))
        .route("/ws", get(ws_handler))
        .route("/api/auth/status", get(auth_status))
        .route("/api/auth/register", post(auth_register))
        .route("/api/auth/login", post(auth_login))
        .merge(protected_routes)
        .fallback(static_asset)
        .layer(CompressionLayer::new())
        .layer(DefaultBodyLimit::max(
            MAX_UPLOAD_FILE_BYTES * MAX_UPLOAD_FILES,
        ))
        .layer(middleware::from_fn(no_store_api_cache_headers))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn no_store_api_cache_headers(request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    if is_api {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

#[derive(Debug)]
pub struct ServerError {
    pub(crate) status: StatusCode,
    pub(crate) body: ApiErrorBody,
}

type Result<T> = std::result::Result<T, ServerError>;

impl ServerError {
    pub(crate) fn new(status: StatusCode, error: impl Into<String>) -> Self {
        Self {
            status,
            body: ApiErrorBody::new(error),
        }
    }

    pub(crate) fn with_details(
        status: StatusCode,
        error: impl Into<String>,
        details: impl Into<String>,
    ) -> Self {
        Self {
            status,
            body: ApiErrorBody::with_details(error, details),
        }
    }

    pub(crate) fn database(
        status: StatusCode,
        error: impl Into<String>,
        details: Option<String>,
        code: impl Into<String>,
        category: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            status,
            body: ApiErrorBody::database(error, details, code, category, retryable),
        }
    }
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<CoreError> for ServerError {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::ProjectNotFound(_) | CoreError::SessionNotFound(_) => {
                Self::new(StatusCode::NOT_FOUND, error.to_string())
            }
            CoreError::AuthenticationFailed => {
                Self::new(StatusCode::UNAUTHORIZED, error.to_string())
            }
            CoreError::Forbidden(_) => Self::new(StatusCode::FORBIDDEN, error.to_string()),
            CoreError::Conflict(_) => Self::new(StatusCode::CONFLICT, error.to_string()),
            CoreError::InvalidInput(_) | CoreError::Fs(FsError::InvalidPath(_)) => {
                Self::new(StatusCode::BAD_REQUEST, error.to_string())
            }
            CoreError::Fs(FsError::OutsideRoot) => {
                Self::new(StatusCode::FORBIDDEN, error.to_string())
            }
            _ => Self::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error",
                error.to_string(),
            ),
        }
    }
}

impl From<iowb_storage::StorageError> for ServerError {
    fn from(error: iowb_storage::StorageError) -> Self {
        Self::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage error",
            error.to_string(),
        )
    }
}

impl From<FsError> for ServerError {
    fn from(error: FsError) -> Self {
        match error {
            FsError::InvalidPath(_) | FsError::BinaryFile => {
                Self::new(StatusCode::BAD_REQUEST, error.to_string())
            }
            FsError::OutsideRoot => Self::new(StatusCode::FORBIDDEN, error.to_string()),
            FsError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Self::new(StatusCode::NOT_FOUND, "path not found")
            }
            FsError::Io(error) => Self::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "filesystem error",
                error.to_string(),
            ),
        }
    }
}

impl From<ProcessError> for ServerError {
    fn from(error: ProcessError) -> Self {
        match error {
            ProcessError::NotFound => Self::new(StatusCode::NOT_FOUND, error.to_string()),
            ProcessError::EmptyCommand => Self::new(StatusCode::BAD_REQUEST, error.to_string()),
            _ => Self::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "process error",
                error.to_string(),
            ),
        }
    }
}

fn multipart_server_error(error: axum::extract::multipart::MultipartError) -> ServerError {
    ServerError::with_details(
        StatusCode::BAD_REQUEST,
        "multipart upload error",
        error.to_string(),
    )
}

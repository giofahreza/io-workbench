async fn static_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let asset_path = if path.is_empty() { "index.html" } else { path };
    if let Some(asset) = iowb_ui::get_asset(asset_path) {
        return (
            [
                (header::CONTENT_TYPE, asset.content_type),
                (header::CACHE_CONTROL, STATIC_CACHE_CONTROL),
            ],
            Bytes::from_static(asset.bytes),
        )
            .into_response();
    }

    if path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiErrorBody::new("route not found")),
        )
            .into_response();
    }

    if let Some(asset) = iowb_ui::get_asset("index.html") {
        return (
            [
                (header::CONTENT_TYPE, asset.content_type),
                (header::CACHE_CONTROL, STATIC_CACHE_CONTROL),
            ],
            Bytes::from_static(asset.bytes),
        )
            .into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

#[derive(Debug, Deserialize)]
struct ExternalToolRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(default)]
    payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExternalToolRun {
    id: String,
    namespace: String,
    action: String,
    command: String,
    args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    success: bool,
    status: Option<i32>,
    stdout: String,
    stderr: String,
    #[serde(rename = "durationMs")]
    duration_ms: u128,
    #[serde(rename = "createdAt")]
    created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct McpServerStartRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpServerRecord {
    id: String,
    name: String,
    command: String,
    args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(rename = "processId")]
    process_id: String,
    status: String,
    #[serde(rename = "startedAt")]
    started_at: chrono::DateTime<Utc>,
    #[serde(rename = "stoppedAt", skip_serializing_if = "Option::is_none")]
    stopped_at: Option<chrono::DateTime<Utc>>,
}

async fn list_mcp_servers(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "success": true,
        "servers": load_mcp_servers(&state, &user.0.id)?,
    })))
}

async fn start_mcp_server(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<McpServerStartRequest>,
) -> Result<Json<Value>> {
    let command = request
        .command
        .or_else(|| env::var("IO_WORKBENCH_MCP_SERVER_COMMAND").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "MCP server command is required; pass command or set IO_WORKBENCH_MCP_SERVER_COMMAND",
            )
        })?;
    let args = if request.args.is_empty() {
        env_json_vec("IO_WORKBENCH_MCP_SERVER_ARGS_JSON")?.unwrap_or_default()
    } else {
        request.args
    };
    let cwd = validate_optional_cwd(&state, request.cwd).await?;
    let started = state
        .processes
        .start(ProcessStartRequest {
            command: command.clone(),
            args: args.clone(),
            cwd: cwd.clone(),
            pty: false,
            cols: 80,
            rows: 24,
        })
        .await?;
    let record = McpServerRecord {
        id: request.id.unwrap_or_else(|| new_id("mcp")),
        name: if request.name.trim().is_empty() {
            command.clone()
        } else {
            request.name.trim().to_string()
        },
        command,
        args,
        cwd,
        process_id: started.id,
        status: "running".to_string(),
        started_at: started.started_at,
        stopped_at: None,
    };
    let mut servers = load_mcp_servers(&state, &user.0.id)?;
    servers.retain(|server| server.id != record.id);
    servers.insert(0, record.clone());
    save_mcp_servers(&state, &user.0.id, &servers)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "server": record,
    })))
}

async fn stop_mcp_server(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(server_id): AxumPath<String>,
) -> Result<Json<Value>> {
    let mut servers = load_mcp_servers(&state, &user.0.id)?;
    let mut stopped = None;
    for server in &mut servers {
        if server.id == server_id {
            let _ = state.processes.abort(&server.process_id).await;
            server.status = "stopped".to_string();
            server.stopped_at = Some(Utc::now());
            stopped = Some(server.clone());
            break;
        }
    }
    let server =
        stopped.ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "MCP server not found"))?;
    save_mcp_servers(&state, &user.0.id, &servers)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "server": server,
    })))
}

async fn call_mcp_tool(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<Value>,
) -> Result<Json<Value>> {
    run_external_tool_json(
        &state,
        &user.0.id,
        "mcp",
        "IO_WORKBENCH_MCP_COMMAND",
        "IO_WORKBENCH_MCP_ARGS_JSON",
        "call",
        request,
    )
    .await
}

async fn run_mcp_utils(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<Value>,
) -> Result<Json<Value>> {
    run_external_tool_json(
        &state,
        &user.0.id,
        "mcp-utils",
        "IO_WORKBENCH_MCP_UTILS_COMMAND",
        "IO_WORKBENCH_MCP_UTILS_ARGS_JSON",
        "run",
        request,
    )
    .await
}

async fn run_slash_command(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<Value>,
) -> Result<Json<Value>> {
    run_external_tool_json(
        &state,
        &user.0.id,
        "commands",
        "IO_WORKBENCH_COMMANDS_COMMAND",
        "IO_WORKBENCH_COMMANDS_ARGS_JSON",
        "run",
        request,
    )
    .await
}

async fn install_plugin(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<Value>,
) -> Result<Json<Value>> {
    run_external_tool_json(
        &state,
        &user.0.id,
        "plugins",
        "IO_WORKBENCH_PLUGIN_COMMAND",
        "IO_WORKBENCH_PLUGIN_ARGS_JSON",
        "install",
        with_default_action(request, "install"),
    )
    .await
}

async fn remove_plugin(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<Value>,
) -> Result<Json<Value>> {
    run_external_tool_json(
        &state,
        &user.0.id,
        "plugins",
        "IO_WORKBENCH_PLUGIN_COMMAND",
        "IO_WORKBENCH_PLUGIN_ARGS_JSON",
        "remove",
        with_default_action(request, "remove"),
    )
    .await
}

async fn run_plugin_command(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<Value>,
) -> Result<Json<Value>> {
    run_external_tool_json(
        &state,
        &user.0.id,
        "plugins",
        "IO_WORKBENCH_PLUGIN_COMMAND",
        "IO_WORKBENCH_PLUGIN_ARGS_JSON",
        "run",
        request,
    )
    .await
}

async fn run_taskmaster(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<Value>,
) -> Result<Json<Value>> {
    run_external_tool_json(
        &state,
        &user.0.id,
        "taskmaster",
        "IO_WORKBENCH_TASKMASTER_COMMAND",
        "IO_WORKBENCH_TASKMASTER_ARGS_JSON",
        "run",
        request,
    )
    .await
}

async fn register_fcm_token(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<RegisterFcmTokenRequest>,
) -> Result<Json<FcmTokenResponse>> {
    let token = request.token.trim();
    if token.is_empty() {
        return Err(CoreError::InvalidInput("FCM token is required".to_string()).into());
    }
    if token.len() > 8192 {
        return Err(CoreError::InvalidInput("FCM token is too large".to_string()).into());
    }
    let token_count = state.storage.upsert_fcm_token(
        &user.0.id,
        token,
        request
            .platform
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty()),
        request
            .device_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty()),
        request
            .app_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty()),
    )?;
    Ok(Json(FcmTokenResponse {
        success: true,
        token_count,
    }))
}

async fn delete_fcm_token(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<DeleteFcmTokenRequest>,
) -> Result<Json<FcmTokenResponse>> {
    let token = request.token.trim();
    if token.is_empty() {
        return Ok(Json(FcmTokenResponse {
            success: true,
            token_count: state.storage.list_fcm_tokens_for_user(&user.0.id)?.len(),
        }));
    }
    let token_count = state.storage.delete_fcm_token(&user.0.id, token)?;
    Ok(Json(FcmTokenResponse {
        success: true,
        token_count,
    }))
}

async fn send_push_notification(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<Value>,
) -> Result<Json<Value>> {
    run_external_tool_json(
        &state,
        &user.0.id,
        "notifications",
        "IO_WORKBENCH_PUSH_COMMAND",
        "IO_WORKBENCH_PUSH_ARGS_JSON",
        "send",
        with_default_action(request, "send"),
    )
    .await
}

async fn test_push_notification(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<Value>,
) -> Result<Json<Value>> {
    let payload = if request.is_null() {
        serde_json::json!({
            "title": "io-workbench",
            "body": "Test notification"
        })
    } else {
        request
    };
    run_external_tool_json(
        &state,
        &user.0.id,
        "notifications",
        "IO_WORKBENCH_PUSH_COMMAND",
        "IO_WORKBENCH_PUSH_ARGS_JSON",
        "test",
        with_default_action(payload, "test"),
    )
    .await
}

async fn list_tool_runs(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(namespace): AxumPath<String>,
) -> Result<Json<Value>> {
    let namespace = sanitize_tool_namespace(&namespace)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "namespace": namespace,
        "runs": load_tool_runs(&state, &user.0.id, &namespace)?,
    })))
}

async fn run_external_tool_json(
    state: &AppState,
    user_id: &str,
    namespace: &'static str,
    command_env: &'static str,
    args_env: &'static str,
    default_action: &'static str,
    request: Value,
) -> Result<Json<Value>> {
    let mut request: ExternalToolRequest =
        serde_json::from_value(request.clone()).unwrap_or(ExternalToolRequest {
            action: None,
            command: None,
            args: Vec::new(),
            cwd: None,
            payload: request,
        });
    if request.payload.is_null() {
        request.payload = serde_json::json!({});
    }
    let action = request
        .action
        .clone()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_action.to_string());
    let command = request
        .command
        .clone()
        .or_else(|| env::var(command_env).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                format!("{namespace} command is not configured; pass command or set {command_env}"),
            )
        })?;
    let args = if request.args.is_empty() {
        env_json_vec(args_env)?
            .unwrap_or_else(|| vec!["{action}".to_string(), "{payload_path}".to_string()])
    } else {
        request.args.clone()
    };
    let cwd = validate_optional_cwd(state, request.cwd.clone()).await?;
    let run = run_external_command(
        state,
        namespace,
        &action,
        &command,
        args,
        cwd,
        request.payload,
    )
    .await?;
    append_tool_run(state, user_id, namespace, &run)?;
    Ok(Json(serde_json::json!({
        "success": run.success,
        "run": run,
    })))
}

async fn run_external_command(
    state: &AppState,
    namespace: &str,
    action: &str,
    command: &str,
    args: Vec<String>,
    cwd: Option<String>,
    payload: Value,
) -> Result<ExternalToolRun> {
    let run_id = new_id("run");
    let payload_path = state
        .config
        .config_dir
        .join("tmp")
        .join(format!("{run_id}.json"));
    if let Some(parent) = payload_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(FsError::Io)?;
    }
    let payload_json = serde_json::to_string(&payload).map_err(|error| {
        ServerError::with_details(
            StatusCode::BAD_REQUEST,
            "failed to serialize tool payload",
            error.to_string(),
        )
    })?;
    tokio::fs::write(&payload_path, &payload_json)
        .await
        .map_err(FsError::Io)?;

    let rendered_args = args
        .into_iter()
        .map(|arg| {
            arg.replace("{action}", action)
                .replace("{namespace}", namespace)
                .replace("{payload_path}", &payload_path.display().to_string())
                .replace("{payload_json}", &payload_json)
        })
        .collect::<Vec<_>>();

    let started = std::time::Instant::now();
    let created_at = Utc::now();
    let mut child = Command::new(command);
    child.args(&rendered_args);
    if let Some(cwd) = &cwd {
        child.current_dir(cwd);
    }
    let output = timeout(tool_timeout(), child.output()).await;
    let _ = tokio::fs::remove_file(&payload_path).await;
    let output = match output {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return Err(ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to run {namespace} command"),
                error.to_string(),
            ));
        }
        Err(_) => {
            return Err(ServerError::new(
                StatusCode::GATEWAY_TIMEOUT,
                format!("{namespace} command timed out"),
            ));
        }
    };
    Ok(ExternalToolRun {
        id: run_id,
        namespace: namespace.to_string(),
        action: action.to_string(),
        command: command.to_string(),
        args: rendered_args,
        cwd,
        success: output.status.success(),
        status: output.status.code(),
        stdout: bounded_output(&output.stdout),
        stderr: bounded_output(&output.stderr),
        duration_ms: started.elapsed().as_millis(),
        created_at,
    })
}

fn bounded_output(bytes: &[u8]) -> String {
    let truncated = bytes.len() > MAX_TOOL_OUTPUT_BYTES;
    let start = bytes.len().saturating_sub(MAX_TOOL_OUTPUT_BYTES);
    let mut output = String::from_utf8_lossy(&bytes[start..]).to_string();
    if truncated {
        output.insert_str(0, "[output truncated]\n");
    }
    output
}

fn tool_timeout() -> Duration {
    Duration::from_secs(
        env::var("IO_WORKBENCH_TOOL_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(120)
            .clamp(1, 3600),
    )
}

async fn validate_optional_cwd(state: &AppState, cwd: Option<String>) -> Result<Option<String>> {
    let Some(cwd) = cwd else {
        return Ok(None);
    };
    let resolved = state
        .path_validator
        .validate_path(PathBuf::from(cwd), false)
        .await?;
    Ok(Some(resolved.display().to_string()))
}

fn env_json_vec(name: &str) -> Result<Option<Vec<String>>> {
    let Some(raw) = env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    serde_json::from_str::<Vec<String>>(&raw)
        .map(Some)
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::BAD_REQUEST,
                format!("{name} must be a JSON string array"),
                error.to_string(),
            )
        })
}

fn with_default_action(mut value: Value, action: &str) -> Value {
    if let Some(object) = value.as_object_mut() {
        object
            .entry("action")
            .or_insert_with(|| Value::String(action.to_string()));
    }
    value
}

fn load_mcp_servers(state: &AppState, user_id: &str) -> Result<Vec<McpServerRecord>> {
    let key = user_setting_key(user_id, "mcp:servers");
    let value = state
        .storage
        .get_setting(&key)?
        .unwrap_or_else(|| serde_json::json!([]));
    Ok(serde_json::from_value(value).unwrap_or_default())
}

fn save_mcp_servers(state: &AppState, user_id: &str, servers: &[McpServerRecord]) -> Result<()> {
    let key = user_setting_key(user_id, "mcp:servers");
    state
        .storage
        .set_setting(&key, &serde_json::json!(servers))?;
    Ok(())
}

fn load_tool_runs(
    state: &AppState,
    user_id: &str,
    namespace: &str,
) -> Result<Vec<ExternalToolRun>> {
    let key = user_setting_key(user_id, &format!("tool-runs:{namespace}"));
    let value = state
        .storage
        .get_setting(&key)?
        .unwrap_or_else(|| serde_json::json!([]));
    Ok(serde_json::from_value(value).unwrap_or_default())
}

fn append_tool_run(
    state: &AppState,
    user_id: &str,
    namespace: &str,
    run: &ExternalToolRun,
) -> Result<()> {
    let mut runs = load_tool_runs(state, user_id, namespace)?;
    runs.insert(0, run.clone());
    runs.truncate(MAX_TOOL_HISTORY);
    let key = user_setting_key(user_id, &format!("tool-runs:{namespace}"));
    state.storage.set_setting(&key, &serde_json::json!(runs))?;
    Ok(())
}

fn sanitize_tool_namespace(namespace: &str) -> Result<String> {
    let namespace = namespace.trim();
    if namespace.is_empty()
        || namespace
            .chars()
            .any(|ch| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_')
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "invalid tool namespace",
        ));
    }
    Ok(namespace.to_string())
}

async fn settings_compat(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<Json<Value>> {
    persisted_compat_endpoint(state, user, request, "settings", serde_json::json!({})).await
}

async fn agent_compat(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<Json<Value>> {
    let default = serde_json::json!({
        "providers": ["claude", "codex", "gemini"],
        "transport": "websocket",
        "websocketCommand": "start_session"
    });
    persisted_compat_endpoint(state, user, request, "agent", default).await
}

async fn mcp_compat(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<Json<Value>> {
    persisted_compat_endpoint(
        state,
        user,
        request,
        "mcp",
        serde_json::json!({ "servers": [] }),
    )
    .await
}

async fn mcp_utils_compat(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<Json<Value>> {
    persisted_compat_endpoint(
        state,
        user,
        request,
        "mcp-utils",
        serde_json::json!({ "tools": [] }),
    )
    .await
}

async fn commands_compat(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<Json<Value>> {
    persisted_compat_endpoint(
        state,
        user,
        request,
        "commands",
        serde_json::json!({ "commands": [] }),
    )
    .await
}

async fn provider_compat(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<Json<Value>> {
    persisted_compat_endpoint(state, user, request, "provider", serde_json::json!({})).await
}

async fn plugins_compat(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<Json<Value>> {
    persisted_compat_endpoint(
        state,
        user,
        request,
        "plugins",
        serde_json::json!({ "plugins": [] }),
    )
    .await
}

async fn persisted_compat_endpoint(
    state: AppState,
    user: AuthenticatedUser,
    request: Request,
    namespace: &'static str,
    default_value: Value,
) -> Result<Json<Value>> {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let key = user_setting_key(
        &user.0.id,
        &format!("compat:{namespace}:{}", compat_path_key(&path)),
    );

    match method {
        Method::GET => {
            let value = state.storage.get_setting(&key)?.unwrap_or(default_value);
            Ok(Json(serde_json::json!({
                "success": true,
                "namespace": namespace,
                "path": path,
                "value": value,
            })))
        }
        Method::POST | Method::PUT | Method::PATCH => {
            let body = parse_request_json(request).await?;
            let value = body.get("value").cloned().unwrap_or(body);
            state.storage.set_setting(&key, &value)?;
            Ok(Json(serde_json::json!({
                "success": true,
                "namespace": namespace,
                "path": path,
                "value": value,
            })))
        }
        Method::DELETE => {
            state.storage.set_setting(&key, &Value::Null)?;
            Ok(Json(serde_json::json!({
                "success": true,
                "namespace": namespace,
                "path": path,
                "deleted": true,
            })))
        }
        _ => Err(ServerError::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "method not allowed",
        )),
    }
}

async fn parse_request_json(request: Request) -> Result<Value> {
    let bytes = to_bytes(request.into_body(), 1024 * 1024)
        .await
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::BAD_REQUEST,
                "failed to read request body",
                error.to_string(),
            )
        })?;
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        ServerError::with_details(
            StatusCode::BAD_REQUEST,
            "invalid JSON body",
            error.to_string(),
        )
    })
}

fn compat_path_key(path: &str) -> String {
    path.trim_start_matches("/api/").replace(
        |ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_',
        ":",
    )
}

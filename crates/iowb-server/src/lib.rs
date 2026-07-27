mod database;
mod git;

use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    env,
    path::{Path, PathBuf},
    time::Duration,
};

use axum::{
    Extension, Json, Router,
    body::{Bytes, to_bytes},
    extract::{
        DefaultBodyLimit, Multipart, Path as AxumPath, Query, Request, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, Method, StatusCode, Uri, header},
    middleware,
    middleware::Next,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{any, delete, get, patch, post, put},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use iowb_core::{AppConfig, AppState, CoreError, generate_secret_token, hash_secret_token};
use iowb_fs::FsError;
use iowb_process::{ProcessError, ProcessEvent};
use iowb_protocol::{
    ApiErrorBody, AuthStatusResponse, BrowseFilesystemResponse, CreateFileRequest,
    CreateProjectRequest, CreateWorkspaceRequest, DeleteFileRequest, FileContentResponse,
    FileEntry, HealthResponse, HealthStatus, LoginRequest, MessagesResponse, PRODUCT_NAME,
    PlaceholderResponse, ProcessInputRequest, ProcessResizeRequest, ProcessStartRequest,
    ProcessStartResponse, ProjectListResponse, ProjectSummary, Provider, RenameFileRequest,
    ServerStatusResponse, SessionRuntimeStatus, SessionSummary, WS_COMMAND_CHANNEL_CAPACITY,
    WorkspaceType, WsClientCommand, WsServerEvent, new_id,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    time::timeout,
};
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};
use tracing::{debug, info, warn};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_SESSION_MODEL_LENGTH: usize = 200;
const MAX_SESSION_TITLE_LENGTH: usize = 500;
const MAX_UPLOAD_FILES: usize = 20;
const MAX_UPLOAD_FILE_BYTES: usize = 50 * 1024 * 1024;
const MAX_UPLOAD_IMAGES: usize = 5;
const MAX_UPLOAD_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_TOOL_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_TOOL_HISTORY: usize = 50;
const STATIC_CACHE_CONTROL: &str = "no-cache";

pub async fn serve(config: AppConfig) -> anyhow::Result<()> {
    let addr = config.socket_addr();
    let state = AppState::initialize(config).await?;
    spawn_process_event_bridge(state.clone());
    spawn_project_watch_bridge(state.clone());

    let listener = TcpListener::bind(addr).await?;
    info!(%addr, "starting io-workbench server");

    axum::serve(listener, build_router(state)).await?;
    Ok(())
}

pub fn build_router(state: AppState) -> Router {
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
        .route("/api/projects/{project_name}", delete(delete_project))
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
            "/api/projects/{project_name}/files/create",
            post(create_project_file),
        )
        .route(
            "/api/projects/{project_name}/files/rename",
            put(rename_project_file),
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
        .route("/api/sessions/{session_id}/messages", get(session_messages))
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
        .route("/api/settings", get(list_settings))
        .route(
            "/api/settings/value/{key}",
            get(get_setting).put(set_setting),
        )
        .route(
            "/api/settings/notification-preferences",
            get(get_notification_preferences).put(set_notification_preferences),
        )
        .route(
            "/api/settings/sidebar-active-sessions",
            get(get_sidebar_active_sessions).put(set_sidebar_active_sessions),
        )
        .route(
            "/api/settings/direct-ai",
            get(get_direct_ai).put(set_direct_ai),
        )
        .route("/api/settings/direct-ai/models", get(direct_ai_models))
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
        .route("/api/danger/run", post(run_danger))
        .route("/api/danger/runs", get(list_danger_runs))
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
        .route("/api/cursor", any(provider_compat))
        .route("/api/cursor/{*path}", any(provider_compat))
        .route("/api/gemini", any(provider_compat))
        .route("/api/gemini/{*path}", any(provider_compat))
        .route("/api/plugins", any(plugins_compat))
        .route("/api/plugins/{*path}", any(plugins_compat))
        .route("/api/danger", any(danger_compat))
        .route("/api/danger/{*path}", any(danger_compat))
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
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
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

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: HealthStatus::Ok,
        service: PRODUCT_NAME.to_string(),
        version: VERSION.to_string(),
        config_dir: state.config.config_dir.display().to_string(),
        database_path: state.config.database_path.display().to_string(),
        server_time: Utc::now(),
    })
}

async fn server_status(State(state): State<AppState>) -> Json<ServerStatusResponse> {
    Json(state.config.server_status(VERSION))
}

async fn runtime_metrics(State(state): State<AppState>) -> Result<Json<Value>> {
    let projects = state.projects.list(&state.sessions).await?;
    let active_sessions = state.sessions.list_active().await;
    let processes = state.processes.list().await;
    Ok(Json(serde_json::json!({
        "success": true,
        "metrics": {
            "timestamp": Utc::now(),
            "memory": process_memory_metrics().await,
            "projects": {
                "count": projects.len()
            },
            "sessions": {
                "active": active_sessions.len()
            },
            "processes": {
                "active": processes.len()
            },
            "limits": {
                "maxSessions": state.config.max_sessions,
                "maxScanDepth": state.config.max_scan_depth,
                "maxFileReadBytes": state.config.max_file_read_bytes,
                "maxUploadFileBytes": MAX_UPLOAD_FILE_BYTES,
                "maxUploadFiles": MAX_UPLOAD_FILES
            }
        }
    })))
}

async fn process_memory_metrics() -> Value {
    let Ok(status) = tokio::fs::read_to_string("/proc/self/status").await else {
        return serde_json::json!({
            "available": false
        });
    };
    let mut vm_rss_kb = None;
    let mut vm_size_kb = None;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            vm_rss_kb = parse_proc_status_kb(value);
        }
        if let Some(value) = line.strip_prefix("VmSize:") {
            vm_size_kb = parse_proc_status_kb(value);
        }
    }
    serde_json::json!({
        "available": true,
        "rssKb": vm_rss_kb,
        "virtualKb": vm_size_kb
    })
}

fn parse_proc_status_kb(value: &str) -> Option<u64> {
    value
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u64>().ok())
}

#[derive(Clone)]
pub(crate) struct AuthenticatedUser(pub iowb_protocol::UserProfile);

async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response> {
    let token = request_token(request.headers(), request.uri().query());
    let user = state.auth.require_user(token.as_deref())?;
    request.extensions_mut().insert(AuthenticatedUser(user));
    Ok(next.run(request).await)
}

async fn auth_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AuthStatusResponse>> {
    Ok(Json(state.auth.status(bearer_token(&headers).as_deref())?))
}

async fn auth_register(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<iowb_protocol::AuthTokenResponse>> {
    Ok(Json(
        state.auth.register(&request.username, &request.password)?,
    ))
}

async fn auth_login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<iowb_protocol::AuthTokenResponse>> {
    Ok(Json(
        state.auth.login(&request.username, &request.password)?,
    ))
}

async fn auth_logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<PlaceholderResponse>> {
    state.auth.logout(bearer_token(&headers).as_deref())?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "logged out successfully".to_string(),
    }))
}

async fn auth_user(Extension(user): Extension<AuthenticatedUser>) -> Result<Json<UserEnvelope>> {
    Ok(Json(UserEnvelope { user: user.0 }))
}

#[derive(serde::Serialize)]
struct UserEnvelope {
    user: iowb_protocol::UserProfile,
}

async fn list_projects(State(state): State<AppState>) -> Result<Json<ProjectListResponse>> {
    let projects = state.projects.list(&state.sessions).await?;
    Ok(Json(ProjectListResponse { projects }))
}

async fn create_project(
    State(state): State<AppState>,
    Json(request): Json<CreateProjectRequest>,
) -> Result<Json<ProjectSummary>> {
    let path = state
        .path_validator
        .validate_path(PathBuf::from(request.path), false)
        .await?;
    let metadata = tokio::fs::metadata(&path).await.map_err(FsError::Io)?;
    if !metadata.is_dir() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "project path must be a directory",
        ));
    }

    let project = state.projects.add_project(&path)?;
    publish_projects(&state).await;
    Ok(Json(project))
}

async fn create_workspace(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<CreateWorkspaceRequest>,
) -> Result<Json<Value>> {
    let path = state
        .path_validator
        .validate_path(PathBuf::from(&request.path), false)
        .await?;

    match request.workspace_type {
        WorkspaceType::Existing => {
            let metadata = tokio::fs::metadata(&path).await.map_err(FsError::Io)?;
            if !metadata.is_dir() {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "existing workspace path must be a directory",
                ));
            }
        }
        WorkspaceType::New => {
            tokio::fs::create_dir_all(&path)
                .await
                .map_err(FsError::Io)?;
        }
    }

    let project_path = if let Some(github_url) = request
        .github_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        if request.workspace_type != WorkspaceType::New {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Git clone is only supported for new workspaces",
            ));
        }
        let github_token = resolve_github_token(
            &state,
            &user.0.id,
            request.github_token_id,
            request.new_github_token.as_deref(),
        )?;
        clone_repository(github_url, &path, github_token.as_deref()).await?
    } else {
        path
    };

    let project = state.projects.add_project(&project_path)?;
    publish_projects(&state).await;
    Ok(Json(serde_json::json!({
        "success": true,
        "project": project,
        "message": if request.github_url.is_some() {
            "New workspace created and repository cloned successfully"
        } else if request.workspace_type == WorkspaceType::New {
            "New workspace created successfully"
        } else {
            "Existing workspace added successfully"
        },
    })))
}

async fn clone_progress(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<CloneProgressQuery>,
) -> Response {
    let (tx, rx) = mpsc::channel::<Event>(64);

    tokio::spawn(async move {
        if let Err(error) = run_clone_progress(state, user, query, tx.clone()).await {
            send_sse_json(
                &tx,
                serde_json::json!({
                    "type": "error",
                    "message": error.body.error,
                    "details": error.body.details,
                }),
            )
            .await;
        }
    });

    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|event| (Ok::<_, Infallible>(event), rx))
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[derive(Debug, Deserialize)]
struct CloneProgressQuery {
    path: Option<String>,
    #[serde(rename = "workspacePath")]
    workspace_path: Option<String>,
    #[serde(rename = "githubUrl")]
    github_url: Option<String>,
    #[serde(rename = "githubTokenId")]
    github_token_id: Option<i64>,
    #[serde(rename = "newGithubToken")]
    new_github_token: Option<String>,
}

impl CloneProgressQuery {
    fn workspace_path(&self) -> Option<&str> {
        self.workspace_path
            .as_deref()
            .or(self.path.as_deref())
            .map(str::trim)
            .filter(|path| !path.is_empty())
    }
}

async fn run_clone_progress(
    state: AppState,
    user: AuthenticatedUser,
    query: CloneProgressQuery,
    tx: mpsc::Sender<Event>,
) -> Result<()> {
    let workspace_path = query
        .workspace_path()
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "workspacePath is required"))?;
    let github_url = query
        .github_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "githubUrl is required"))?;

    let path = state
        .path_validator
        .validate_path(PathBuf::from(workspace_path), false)
        .await?;
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(FsError::Io)?;

    let github_token = resolve_github_token(
        &state,
        &user.0.id,
        query.github_token_id,
        query.new_github_token.as_deref(),
    )?;
    clone_repository_with_progress(github_url, &path, github_token.as_deref(), &tx, &state).await
}

async fn send_sse_json(tx: &mpsc::Sender<Event>, value: Value) {
    let _ = tx.send(Event::default().data(value.to_string())).await;
}

fn resolve_github_token(
    state: &AppState,
    user_id: &str,
    credential_id: Option<i64>,
    one_time_token: Option<&str>,
) -> Result<Option<String>> {
    if let Some(credential_id) = credential_id {
        return state
            .storage
            .get_active_credential_value(user_id, credential_id, "github_token")?
            .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "GitHub token not found"))
            .map(Some);
    }

    Ok(one_time_token
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string))
}

async fn clone_repository(
    github_url: &str,
    workspace_path: &Path,
    github_token: Option<&str>,
) -> Result<PathBuf> {
    let repo_name = repository_name(github_url);
    let clone_path = workspace_path.join(&repo_name);
    ensure_clone_destination_available(&clone_path).await?;

    let clone_url = clone_url_with_token(github_url, github_token);
    let output = Command::new("git")
        .args([
            "clone",
            "--progress",
            &clone_url,
            &clone_path.display().to_string(),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Git is not installed or not in PATH",
                )
            } else {
                ServerError::with_details(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to start git clone",
                    error.to_string(),
                )
            }
        })?;

    if output.status.success() {
        return Ok(clone_path);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let message = clone_error_message(&format!("{stderr}{stdout}"), github_token);
    let _ = tokio::fs::remove_dir_all(&clone_path).await;
    Err(ServerError::new(StatusCode::BAD_REQUEST, message))
}

async fn clone_repository_with_progress(
    github_url: &str,
    workspace_path: &Path,
    github_token: Option<&str>,
    tx: &mpsc::Sender<Event>,
    state: &AppState,
) -> Result<()> {
    let repo_name = repository_name(github_url);
    let clone_path = workspace_path.join(&repo_name);
    ensure_clone_destination_available(&clone_path).await?;

    let clone_url = clone_url_with_token(github_url, github_token);
    send_sse_json(
        tx,
        serde_json::json!({
            "type": "progress",
            "message": format!("Cloning into '{repo_name}'...")
        }),
    )
    .await;

    let mut child = Command::new("git")
        .args([
            "clone",
            "--progress",
            &clone_url,
            &clone_path.display().to_string(),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Git is not installed or not in PATH",
                )
            } else {
                ServerError::with_details(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to start git clone",
                    error.to_string(),
                )
            }
        })?;

    let mut last_output = String::new();
    let (line_tx, mut line_rx) = mpsc::channel::<String>(64);
    if let Some(stdout) = child.stdout.take() {
        spawn_clone_line_reader(stdout, line_tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_clone_line_reader(stderr, line_tx.clone());
    }
    drop(line_tx);

    loop {
        tokio::select! {
            line = line_rx.recv() => {
                if let Some(line) = line {
                    last_output = sanitize_secret(&line, github_token);
                    send_sse_json(tx, serde_json::json!({
                        "type": "progress",
                        "message": last_output,
                    })).await;
                }
            }
            status = child.wait() => {
                match status {
                    Ok(status) if status.success() => {
                        let project = state.projects.add_project(&clone_path)?;
                        publish_projects(state).await;
                        send_sse_json(tx, serde_json::json!({
                            "type": "complete",
                            "project": project,
                            "message": "Repository cloned successfully",
                        })).await;
                        return Ok(());
                    }
                    Ok(_) => {
                        let message = clone_error_message(&last_output, github_token);
                        let _ = tokio::fs::remove_dir_all(&clone_path).await;
                        return Err(ServerError::new(StatusCode::BAD_REQUEST, message));
                    }
                    Err(error) => {
                        let _ = tokio::fs::remove_dir_all(&clone_path).await;
                        return Err(ServerError::with_details(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "git clone failed",
                            error.to_string(),
                        ));
                    }
                }
            }
        }
    }
}

fn spawn_clone_line_reader<R>(reader: R, tx: mpsc::Sender<String>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();
            if !line.is_empty() && tx.send(line).await.is_err() {
                break;
            }
        }
    });
}

async fn ensure_clone_destination_available(path: &Path) -> Result<()> {
    match tokio::fs::try_exists(path).await {
        Ok(false) => Ok(()),
        Ok(true) => Err(ServerError::new(
            StatusCode::CONFLICT,
            format!("Directory already exists: {}", path.display()),
        )),
        Err(error) => Err(ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to check clone destination",
            error.to_string(),
        )),
    }
}

fn repository_name(github_url: &str) -> String {
    let normalized = github_url.trim().trim_end_matches('/');
    normalized
        .strip_suffix(".git")
        .unwrap_or(normalized)
        .rsplit(['/', ':'])
        .next()
        .map(sanitize_repo_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "repository".to_string())
}

fn sanitize_repo_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .collect()
}

fn clone_url_with_token(github_url: &str, github_token: Option<&str>) -> String {
    let Some(token) = github_token.filter(|token| !token.is_empty()) else {
        return github_url.to_string();
    };
    let Some(rest) = github_url.strip_prefix("https://") else {
        return github_url.to_string();
    };
    format!("https://{}@{}", token, rest)
}

fn clone_error_message(raw: &str, github_token: Option<&str>) -> String {
    let sanitized = sanitize_secret(raw, github_token);
    if sanitized.contains("Authentication failed") || sanitized.contains("could not read Username")
    {
        "Authentication failed. Please check your credentials.".to_string()
    } else if sanitized.contains("Repository not found") {
        "Repository not found. Please check the URL and ensure you have access.".to_string()
    } else if sanitized.contains("already exists") {
        "Directory already exists".to_string()
    } else if sanitized.trim().is_empty() {
        "Git clone failed".to_string()
    } else {
        sanitized
    }
}

fn sanitize_secret(message: &str, secret: Option<&str>) -> String {
    let mut sanitized = message.trim().to_string();
    if let Some(secret) = secret.filter(|secret| !secret.is_empty()) {
        sanitized = sanitized.replace(secret, "***");
    }
    sanitized
}

async fn project_sessions(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
) -> Result<Json<Vec<SessionSummary>>> {
    let project = state.projects.find_by_name(&project_name)?;
    Ok(Json(state.sessions.list_for_project(&project.path).await?))
}

async fn delete_project(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
) -> Result<Json<PlaceholderResponse>> {
    let deleted = state.projects.delete_by_name(&project_name)?;
    if !deleted {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "project not found"));
    }
    publish_projects(&state).await;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "project removed from io-workbench index".to_string(),
    }))
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    path: Option<String>,
    #[serde(rename = "filePath")]
    file_path: Option<String>,
}

impl FileQuery {
    fn requested_path(&self) -> &str {
        self.file_path
            .as_deref()
            .or(self.path.as_deref())
            .unwrap_or("")
    }
}

async fn list_project_files(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<Vec<FileEntry>>> {
    let project = state.projects.find_by_name(&project_name)?;
    Ok(Json(
        state
            .files
            .list_tree(project.path, query.requested_path())
            .await?,
    ))
}

async fn read_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<FileContentResponse>> {
    let project = state.projects.find_by_name(&project_name)?;
    Ok(Json(
        state
            .files
            .read_file(project.path, query.requested_path())
            .await?,
    ))
}

async fn write_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<WriteFileRequestCompat>,
) -> Result<Json<FileContentResponse>> {
    let project = state.projects.find_by_name(&project_name)?;
    Ok(Json(
        state
            .files
            .write_file(project.path, request.file_path, &request.content)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct WriteFileRequestCompat {
    #[serde(alias = "path", rename = "filePath")]
    file_path: String,
    content: String,
}

async fn create_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<CreateFileRequest>,
) -> Result<Json<FileEntry>> {
    let project = state.projects.find_by_name(&project_name)?;
    Ok(Json(
        state
            .files
            .create_path(
                project.path,
                request.file_path,
                &request.content,
                request.directory,
            )
            .await?,
    ))
}

async fn rename_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<RenameFileRequest>,
) -> Result<Json<FileEntry>> {
    let project = state.projects.find_by_name(&project_name)?;
    Ok(Json(
        state
            .files
            .rename_path(project.path, request.old_path, request.new_path)
            .await?,
    ))
}

async fn delete_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<DeleteFileRequest>,
) -> Result<Json<PlaceholderResponse>> {
    let project = state.projects.find_by_name(&project_name)?;
    state
        .files
        .delete_path(project.path, request.file_path)
        .await?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "file deleted".to_string(),
    }))
}

async fn files_upload(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    multipart: Multipart,
) -> Result<Json<Value>> {
    let project = state.projects.find_by_name(&project_name)?;
    let (fields, files) =
        collect_multipart_files(multipart, MAX_UPLOAD_FILES, MAX_UPLOAD_FILE_BYTES).await?;
    if files.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No files provided",
        ));
    }

    let target_path = fields
        .get("targetPath")
        .map(String::as_str)
        .unwrap_or("")
        .trim();
    let relative_paths = fields
        .get("relativePaths")
        .and_then(|value| serde_json::from_str::<Vec<String>>(value).ok())
        .unwrap_or_default();

    let mut uploaded = Vec::new();
    for (index, file) in files.into_iter().enumerate() {
        let file_name = relative_paths
            .get(index)
            .map(String::as_str)
            .unwrap_or(file.file_name.as_str())
            .trim();
        if file_name.is_empty() {
            continue;
        }

        let destination = if target_path.is_empty() || matches!(target_path, "." | "./") {
            PathBuf::from(file_name)
        } else {
            PathBuf::from(target_path).join(file_name)
        };
        let size = file.bytes.len();
        let entry = state
            .files
            .write_bytes(&project.path, &destination, &file.bytes)
            .await?;
        uploaded.push(serde_json::json!({
            "name": file_name,
            "path": entry.path,
            "size": size,
            "mimeType": file.content_type,
        }));
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "files": uploaded,
        "targetPath": target_path,
        "message": format!("Uploaded {} file(s) successfully", uploaded.len()),
    })))
}

async fn upload_images(
    AxumPath(_project_name): AxumPath<String>,
    multipart: Multipart,
) -> Result<Json<Value>> {
    let (_fields, files) =
        collect_multipart_files(multipart, MAX_UPLOAD_IMAGES, MAX_UPLOAD_IMAGE_BYTES).await?;
    if files.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No image files provided",
        ));
    }

    let mut images = Vec::new();
    for file in files {
        let mime_type = file
            .content_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        if !is_allowed_image_mime(&mime_type) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Invalid file type. Only JPEG, PNG, GIF, WebP, and SVG are allowed.",
            ));
        }
        let data = BASE64_STANDARD.encode(&file.bytes);
        images.push(serde_json::json!({
            "name": file.file_name,
            "data": format!("data:{mime_type};base64,{data}"),
            "size": file.bytes.len(),
            "mimeType": mime_type,
        }));
    }

    Ok(Json(serde_json::json!({ "images": images })))
}

async fn audio_transcribe(multipart: Multipart) -> Result<Json<Value>> {
    let command = env::var("IO_WORKBENCH_TRANSCRIBE_COMMAND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "audio transcription is not configured; set IO_WORKBENCH_TRANSCRIBE_COMMAND",
            )
        })?;
    let (_fields, files) = collect_multipart_files(multipart, 1, MAX_UPLOAD_FILE_BYTES).await?;
    let file = files
        .into_iter()
        .next()
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "audio file is required"))?;
    let mime_type = file
        .content_type
        .clone()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let temp_path = env::temp_dir().join(format!("{}.audio", new_id("iowb")));
    tokio::fs::write(&temp_path, &file.bytes)
        .await
        .map_err(FsError::Io)?;

    let args = transcribe_args(&temp_path, &file.file_name, &mime_type)?;
    let output = Command::new(&command)
        .args(args)
        .output()
        .await
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to run transcription command",
                error.to_string(),
            )
        });
    let _ = tokio::fs::remove_file(&temp_path).await;
    let output = output?;
    if !output.status.success() {
        return Err(ServerError::with_details(
            StatusCode::BAD_GATEWAY,
            "transcription command failed",
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "text": String::from_utf8_lossy(&output.stdout).trim(),
    })))
}

fn transcribe_args(path: &Path, filename: &str, mime_type: &str) -> Result<Vec<String>> {
    let template = env::var("IO_WORKBENCH_TRANSCRIBE_ARGS_JSON")
        .ok()
        .unwrap_or_else(|| serde_json::json!(["{audio_path}"]).to_string());
    let args = serde_json::from_str::<Vec<String>>(&template).map_err(|error| {
        ServerError::with_details(
            StatusCode::BAD_REQUEST,
            "invalid IO_WORKBENCH_TRANSCRIBE_ARGS_JSON",
            error.to_string(),
        )
    })?;
    Ok(args
        .into_iter()
        .map(|arg| {
            arg.replace("{audio_path}", &path.display().to_string())
                .replace("{filename}", filename)
                .replace("{mime_type}", mime_type)
        })
        .collect())
}

struct UploadedPart {
    file_name: String,
    content_type: Option<String>,
    bytes: Bytes,
}

async fn collect_multipart_files(
    mut multipart: Multipart,
    max_files: usize,
    max_file_bytes: usize,
) -> Result<(HashMap<String, String>, Vec<UploadedPart>)> {
    let mut fields = HashMap::new();
    let mut files = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(multipart_server_error)?
    {
        let name = field.name().unwrap_or("").to_string();
        let file_name = field.file_name().map(str::to_string);
        let content_type = field.content_type().map(str::to_string);

        if let Some(file_name) = file_name {
            if files.len() >= max_files {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    format!("Too many files. Maximum is {max_files} files."),
                ));
            }
            let bytes = field.bytes().await.map_err(multipart_server_error)?;
            if bytes.len() > max_file_bytes {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    format!("File too large. Maximum size is {max_file_bytes} bytes."),
                ));
            }
            files.push(UploadedPart {
                file_name,
                content_type,
                bytes,
            });
        } else {
            let value = field.text().await.map_err(multipart_server_error)?;
            fields.insert(name, value);
        }
    }

    Ok((fields, files))
}

fn is_allowed_image_mime(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp" | "image/svg+xml"
    )
}

async fn browse_filesystem(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
) -> Result<Json<BrowseFilesystemResponse>> {
    let path = query
        .path
        .as_deref()
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| state.config.workspace_root.to_str().unwrap_or("~"));
    let entries = state.files.browse_directories(path).await?;
    Ok(Json(BrowseFilesystemResponse {
        path: path.to_string(),
        entries,
    }))
}

async fn create_folder(
    State(state): State<AppState>,
    Json(request): Json<CreateFolderRequest>,
) -> Result<Json<PlaceholderResponse>> {
    let path = state
        .path_validator
        .validate_path(PathBuf::from(request.path), false)
        .await?;
    tokio::fs::create_dir_all(path).await.map_err(FsError::Io)?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "folder created".to_string(),
    }))
}

#[derive(Debug, Deserialize)]
struct CreateFolderRequest {
    path: String,
}

async fn list_settings(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "success": true,
        "settings": state.storage.list_settings()?,
    })))
}

async fn get_setting(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<String>,
) -> Result<Json<Value>> {
    let value = state
        .storage
        .get_setting(&key)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "setting not found"))?;
    Ok(Json(serde_json::json!({
        "success": true,
        "key": key,
        "value": value,
    })))
}

async fn set_setting(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let value = body.get("value").cloned().unwrap_or(body);
    state.storage.set_setting(&key, &value)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "key": key,
        "value": value,
    })))
}

async fn get_notification_preferences(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "notification-preferences");
    let preferences = state
        .storage
        .get_setting(&key)?
        .unwrap_or_else(default_notification_preferences);
    Ok(Json(serde_json::json!({
        "success": true,
        "preferences": preferences,
    })))
}

async fn set_notification_preferences(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(preferences): Json<Value>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "notification-preferences");
    state.storage.set_setting(&key, &preferences)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "preferences": preferences,
    })))
}

async fn get_sidebar_active_sessions(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "sidebar-active-sessions");
    let pinned_sessions = state
        .storage
        .get_setting(&key)?
        .unwrap_or_else(|| serde_json::json!([]));
    Ok(Json(serde_json::json!({
        "success": true,
        "pinnedSessions": pinned_sessions,
    })))
}

async fn set_sidebar_active_sessions(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let pinned_sessions = body
        .get("pinnedSessions")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let key = user_setting_key(&user.0.id, "sidebar-active-sessions");
    state.storage.set_setting(&key, &pinned_sessions)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "pinnedSessions": pinned_sessions,
    })))
}

async fn get_direct_ai(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "direct-ai");
    let config = state
        .storage
        .get_setting(&key)?
        .unwrap_or_else(default_direct_ai_config);
    Ok(Json(serde_json::json!({
        "success": true,
        "config": config,
    })))
}

async fn set_direct_ai(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(config): Json<Value>,
) -> Result<Json<Value>> {
    validate_direct_ai_config(&config)?;
    let key = user_setting_key(&user.0.id, "direct-ai");
    state.storage.set_setting(&key, &config)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "config": config,
    })))
}

async fn direct_ai_models(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Json<Value> {
    let key = user_setting_key(&user.0.id, "direct-ai");
    let config = state
        .storage
        .get_setting(&key)
        .ok()
        .flatten()
        .unwrap_or_else(default_direct_ai_config);
    let models = fetch_direct_ai_models(&config).await.unwrap_or_default();
    Json(serde_json::json!({
        "success": true,
        "models": models,
    }))
}

async fn get_git_config(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "git-config");
    let stored = state.storage.get_setting(&key)?;
    let git_name = stored
        .as_ref()
        .and_then(|value| value.get("gitName"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| stored_git_alias(&stored, "git_name"))
        .or_else(|| blocking_git_config("user.name"));
    let git_email = stored
        .as_ref()
        .and_then(|value| value.get("gitEmail"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| stored_git_alias(&stored, "git_email"))
        .or_else(|| blocking_git_config("user.email"));

    if stored.is_none() && (git_name.is_some() || git_email.is_some()) {
        state.storage.set_setting(
            &key,
            &serde_json::json!({
                "gitName": git_name,
                "gitEmail": git_email,
            }),
        )?;
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "gitName": git_name,
        "gitEmail": git_email,
    })))
}

async fn set_git_config(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<GitConfigRequest>,
) -> Result<Json<Value>> {
    let git_name = request.git_name.trim();
    let git_email = request.git_email.trim();
    if git_name.is_empty() || git_email.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Git name and email are required",
        ));
    }
    if !looks_like_email(git_email) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid email format",
        ));
    }

    let key = user_setting_key(&user.0.id, "git-config");
    state.storage.set_setting(
        &key,
        &serde_json::json!({
            "gitName": git_name,
            "gitEmail": git_email,
        }),
    )?;

    let name_result = set_git_global_config("user.name", git_name).await;
    let email_result = set_git_global_config("user.email", git_email).await;
    let git_applied = name_result.is_ok() && email_result.is_ok();

    Ok(Json(serde_json::json!({
        "success": true,
        "gitName": git_name,
        "gitEmail": git_email,
        "gitApplied": git_applied,
        "gitError": name_result
            .err()
            .or_else(|| email_result.err())
            .map(|error| error.to_string()),
    })))
}

async fn onboarding_status(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "onboarding-complete");
    let has_completed = state
        .storage
        .get_setting(&key)?
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    Ok(Json(serde_json::json!({
        "success": true,
        "hasCompletedOnboarding": has_completed,
    })))
}

async fn complete_onboarding(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "onboarding-complete");
    state.storage.set_setting(&key, &Value::Bool(true))?;
    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Onboarding completed successfully",
    })))
}

async fn user_settings_overview(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "success": true,
        "user": user.0,
        "gitConfig": state.storage.get_setting(&user_setting_key(&user.0.id, "git-config"))?,
        "hasCompletedOnboarding": state
            .storage
            .get_setting(&user_setting_key(&user.0.id, "onboarding-complete"))?
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
    })))
}

async fn cli_provider_status(AxumPath(provider): AxumPath<String>) -> Result<Json<Value>> {
    let provider = parse_provider_param(&provider)?;
    Ok(Json(provider_cli_status(provider).await))
}

async fn cli_overview() -> Json<Value> {
    let mut providers = serde_json::Map::new();
    for provider in [
        Provider::Claude,
        Provider::Cursor,
        Provider::Codex,
        Provider::Gemini,
    ] {
        providers.insert(
            provider.as_str().to_string(),
            provider_cli_status(provider).await,
        );
    }
    Json(serde_json::json!({
        "success": true,
        "providers": providers,
    }))
}

async fn list_api_keys(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "apiKeys": state.storage.list_api_keys(&user.0.id)?,
    })))
}

async fn create_api_key(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<CreateApiKeyRequest>,
) -> Result<Json<Value>> {
    let key_name = request.key_name.trim();
    if key_name.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "keyName is required",
        ));
    }

    let api_key = generate_secret_token("iowb_key");
    let key_prefix = api_key.chars().take(18).collect::<String>();
    let record = state.storage.create_api_key(
        &user.0.id,
        key_name,
        &hash_secret_token(&api_key),
        &key_prefix,
    )?;
    let mut api_key_value = serde_json::to_value(record).unwrap_or_else(|_| serde_json::json!({}));
    if let Value::Object(map) = &mut api_key_value {
        map.insert("api_key".to_string(), Value::String(api_key));
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "apiKey": api_key_value,
    })))
}

async fn delete_api_key(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(key_id): AxumPath<i64>,
) -> Result<Json<Value>> {
    if !state.storage.delete_api_key(&user.0.id, key_id)? {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "API key not found"));
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn toggle_api_key(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(key_id): AxumPath<i64>,
    Json(request): Json<ToggleActiveRequest>,
) -> Result<Json<Value>> {
    if !state
        .storage
        .toggle_api_key(&user.0.id, key_id, request.is_active)?
    {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "API key not found"));
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn list_credentials(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<CredentialsQuery>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "credentials": state
            .storage
            .list_credentials(&user.0.id, query.credential_type.as_deref())?,
    })))
}

async fn create_credential(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<CreateCredentialRequest>,
) -> Result<Json<Value>> {
    let credential_name = request.credential_name.trim();
    let credential_type = request.credential_type.trim();
    let credential_value = request.credential_value.trim();
    if credential_name.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "credentialName is required",
        ));
    }
    if credential_type.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "credentialType is required",
        ));
    }
    if credential_value.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "credentialValue is required",
        ));
    }

    let credential = state.storage.create_credential(
        &user.0.id,
        credential_name,
        credential_type,
        credential_value,
        request.description.as_deref().map(str::trim),
    )?;

    Ok(Json(serde_json::json!({
        "success": true,
        "credential": credential,
    })))
}

async fn delete_credential(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(credential_id): AxumPath<i64>,
) -> Result<Json<Value>> {
    if !state.storage.delete_credential(&user.0.id, credential_id)? {
        return Err(ServerError::new(
            StatusCode::NOT_FOUND,
            "credential not found",
        ));
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn toggle_credential(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(credential_id): AxumPath<i64>,
    Json(request): Json<ToggleActiveRequest>,
) -> Result<Json<Value>> {
    if !state
        .storage
        .toggle_credential(&user.0.id, credential_id, request.is_active)?
    {
        return Err(ServerError::new(
            StatusCode::NOT_FOUND,
            "credential not found",
        ));
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

#[derive(Debug, Deserialize)]
struct CreateApiKeyRequest {
    #[serde(alias = "key_name", rename = "keyName")]
    key_name: String,
}

#[derive(Debug, Deserialize)]
struct GitConfigRequest {
    #[serde(alias = "git_name", rename = "gitName")]
    git_name: String,
    #[serde(alias = "git_email", rename = "gitEmail")]
    git_email: String,
}

#[derive(Debug, Deserialize)]
struct ToggleActiveRequest {
    #[serde(alias = "is_active", rename = "isActive")]
    is_active: bool,
}

#[derive(Debug, Deserialize)]
struct CredentialsQuery {
    #[serde(rename = "type")]
    credential_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateCredentialRequest {
    #[serde(alias = "credential_name", rename = "credentialName")]
    credential_name: String,
    #[serde(alias = "credential_type", rename = "credentialType")]
    credential_type: String,
    #[serde(alias = "credential_value", rename = "credentialValue")]
    credential_value: String,
    description: Option<String>,
}

async fn session_messages(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> Result<Json<MessagesResponse>> {
    let messages = state.sessions.messages(&session_id)?;
    Ok(Json(MessagesResponse {
        session_id,
        messages,
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

async fn session_token_usage(
    State(state): State<AppState>,
    AxumPath((project_name, session_id)): AxumPath<(String, String)>,
    Query(query): Query<TokenUsageQuery>,
) -> Result<Json<Value>> {
    validate_session_id(&session_id)?;
    let provider = query
        .provider
        .as_deref()
        .map(parse_provider_param)
        .transpose()?
        .unwrap_or(Provider::Claude);

    let usage = match provider {
        Provider::Cursor => serde_json::json!({
            "used": 0,
            "total": 0,
            "breakdown": { "input": 0, "cacheCreation": 0, "cacheRead": 0 },
            "unsupported": true,
            "message": "Token usage tracking not available for Cursor sessions",
        }),
        Provider::Gemini => serde_json::json!({
            "used": 0,
            "total": 0,
            "breakdown": { "input": 0, "cacheCreation": 0, "cacheRead": 0 },
            "unsupported": true,
            "message": "Token usage tracking not available for Gemini sessions",
        }),
        Provider::Codex => codex_token_usage(&session_id).await?,
        Provider::Claude => {
            let project = state.projects.find_by_name(&project_name)?;
            claude_token_usage(&project.path, &session_id).await?
        }
    };

    Ok(Json(usage))
}

async fn list_processes(State(state): State<AppState>) -> Json<Vec<iowb_protocol::ProcessInfo>> {
    Json(state.processes.list().await)
}

async fn start_process(
    State(state): State<AppState>,
    Json(request): Json<ProcessStartRequest>,
) -> Result<Json<ProcessStartResponse>> {
    Ok(Json(state.processes.start(request).await?))
}

async fn abort_process(
    State(state): State<AppState>,
    AxumPath(process_id): AxumPath<String>,
) -> Result<Json<PlaceholderResponse>> {
    state.processes.abort(&process_id).await?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "process abort requested".to_string(),
    }))
}

async fn write_process_input(
    State(state): State<AppState>,
    AxumPath(process_id): AxumPath<String>,
    Json(request): Json<ProcessInputRequest>,
) -> Result<Json<PlaceholderResponse>> {
    state
        .processes
        .write_input(&process_id, request.data.into_bytes())
        .await?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "process input written".to_string(),
    }))
}

async fn resize_process(
    State(state): State<AppState>,
    AxumPath(process_id): AxumPath<String>,
    Json(request): Json<ProcessResizeRequest>,
) -> Result<Json<PlaceholderResponse>> {
    state
        .processes
        .resize_terminal(&process_id, request.cols, request.rows)
        .await?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "process resize accepted".to_string(),
    }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let token = request_token(&headers, uri.query());
    if let Err(error) = state.auth.require_user(token.as_deref()) {
        return ServerError::from(error).into_response();
    }

    ws.on_upgrade(move |socket| handle_socket(socket, state))
        .into_response()
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let connection_id = new_id("conn");
    let (mut sender, mut receiver) = socket.split();
    let (command_tx, mut command_rx) =
        mpsc::channel::<WsClientCommand>(WS_COMMAND_CHANNEL_CAPACITY);
    let (direct_tx, mut direct_rx) =
        mpsc::channel::<WsServerEvent>(iowb_protocol::WS_EVENT_CHANNEL_CAPACITY);
    let mut hub_rx = state.ws_hub.subscribe();

    let reader_connection_id = connection_id.clone();
    tokio::spawn(async move {
        while let Some(message) = receiver.next().await {
            match message {
                Ok(Message::Text(text)) => match serde_json::from_str::<WsClientCommand>(&text) {
                    Ok(command) => {
                        if command_tx.send(command).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        warn!(
                            connection_id = %reader_connection_id,
                            error = %error,
                            "invalid websocket command"
                        );
                    }
                },
                Ok(Message::Close(_)) => break,
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Binary(_)) => {}
                Err(error) => {
                    warn!(connection_id = %reader_connection_id, error = %error, "websocket read failed");
                    break;
                }
            }
        }
    });

    let _ = direct_tx
        .send(WsServerEvent::Connected {
            connection_id: connection_id.clone(),
            server_time: Utc::now(),
        })
        .await;

    loop {
        tokio::select! {
            Some(command) = command_rx.recv() => {
                handle_ws_command(&state, &direct_tx, command).await;
            }
            Some(event) = direct_rx.recv() => {
                if send_ws_event(&mut sender, event).await.is_err() {
                    break;
                }
            }
            event = hub_rx.recv() => {
                match event {
                    Ok(event) => {
                        if send_ws_event(&mut sender, event).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        let warning = WsServerEvent::Error {
                            message: "websocket client fell behind".to_string(),
                            details: Some(format!("skipped {skipped} events")),
                        };
                        if send_ws_event(&mut sender, warning).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            else => break,
        }
    }

    debug!(%connection_id, "websocket disconnected");
}

async fn handle_ws_command(
    state: &AppState,
    direct_tx: &mpsc::Sender<WsServerEvent>,
    command: WsClientCommand,
) {
    match command {
        WsClientCommand::Ping { nonce } => {
            let _ = direct_tx
                .send(WsServerEvent::Pong {
                    nonce,
                    server_time: Utc::now(),
                })
                .await;
        }
        WsClientCommand::Subscribe { .. } => {
            let _ = direct_tx
                .send(WsServerEvent::ActiveSessions {
                    sessions: state.sessions.list_active().await,
                })
                .await;
            for event in state.replay_agent_events().await {
                let _ = direct_tx.send(event).await;
            }
        }
        WsClientCommand::StartSession {
            provider,
            project_path,
            prompt,
            session_id,
            model,
        } => match state
            .start_agent_session(provider, project_path, prompt, session_id, model)
            .await
        {
            Ok(session) => {
                let _ = direct_tx
                    .send(WsServerEvent::SessionStatus {
                        provider,
                        session_id: session.id,
                        status: SessionRuntimeStatus::Starting,
                    })
                    .await;
            }
            Err(error) => {
                let _ = direct_tx
                    .send(WsServerEvent::Error {
                        message: "failed to start session".to_string(),
                        details: Some(error.to_string()),
                    })
                    .await;
            }
        },
        WsClientCommand::AbortSession {
            provider,
            session_id,
        } => match state.abort_agent_session(provider, &session_id).await {
            Ok(_) => {}
            Err(error) => {
                let _ = direct_tx
                    .send(WsServerEvent::Error {
                        message: "failed to abort session".to_string(),
                        details: Some(error.to_string()),
                    })
                    .await;
            }
        },
        WsClientCommand::ProcessInput { process_id, data } => {
            if let Err(error) = state
                .processes
                .write_input(&process_id, data.into_bytes())
                .await
            {
                let _ = direct_tx
                    .send(WsServerEvent::Error {
                        message: "failed to write process input".to_string(),
                        details: Some(error.to_string()),
                    })
                    .await;
            }
        }
        WsClientCommand::ResizeTerminal {
            process_id,
            cols,
            rows,
        } => {
            if let Err(error) = state
                .processes
                .resize_terminal(&process_id, cols, rows)
                .await
            {
                let _ = direct_tx
                    .send(WsServerEvent::Error {
                        message: "failed to resize process".to_string(),
                        details: Some(error.to_string()),
                    })
                    .await;
            }
        }
    }
}

async fn send_ws_event(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    event: WsServerEvent,
) -> std::result::Result<(), axum::Error> {
    let payload = serde_json::to_string(&event).unwrap_or_else(|error| {
        serde_json::json!({
            "type": "error",
            "message": "failed to serialize websocket event",
            "details": error.to_string(),
        })
        .to_string()
    });
    sender.send(Message::Text(payload.into())).await
}

fn spawn_process_event_bridge(state: AppState) {
    let mut rx = state.processes.subscribe();
    let hub = state.ws_hub.clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ProcessEvent::Output {
                    process_id,
                    stream,
                    data,
                }) => hub.publish(WsServerEvent::ProcessOutput {
                    process_id,
                    stream,
                    data,
                }),
                Ok(ProcessEvent::Exited { process_id, code }) => {
                    hub.publish(WsServerEvent::ProcessExited { process_id, code });
                }
                Ok(ProcessEvent::Failed {
                    process_id,
                    message,
                }) => hub.publish(WsServerEvent::Error {
                    message: format!("process {process_id} failed"),
                    details: Some(message),
                }),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "process event bridge lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn spawn_project_watch_bridge(state: AppState) {
    let (tx, mut rx) = mpsc::channel::<notify::Result<notify::Event>>(256);
    let debounce = Duration::from_millis(state.watch.debounce_ms());

    tokio::spawn(async move {
        let mut watchers: HashMap<String, RecommendedWatcher> = HashMap::new();
        let mut discovery = tokio::time::interval(Duration::from_secs(5));
        discovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut pending_update = false;

        loop {
            tokio::select! {
                _ = discovery.tick() => {
                    refresh_project_watchers(&state, &tx, &mut watchers).await;
                }
                Some(event) = rx.recv() => {
                    match event {
                        Ok(event) => {
                            if is_interesting_watch_event(&event) {
                                pending_update = true;
                            }
                        }
                        Err(error) => warn!(error = %error, "project watcher error"),
                    }
                }
                _ = tokio::time::sleep(debounce), if pending_update => {
                    pending_update = false;
                    publish_projects(&state).await;
                }
                else => break,
            }
        }
    });
}

async fn refresh_project_watchers(
    state: &AppState,
    tx: &mpsc::Sender<notify::Result<notify::Event>>,
    watchers: &mut HashMap<String, RecommendedWatcher>,
) {
    let projects = match state.storage.list_projects() {
        Ok(projects) => projects,
        Err(error) => {
            warn!(error = %error, "failed to load projects for watcher refresh");
            return;
        }
    };
    let desired = projects
        .iter()
        .map(|project| project.path.clone())
        .collect::<HashSet<_>>();

    watchers.retain(|path, _| desired.contains(path));

    for project in projects {
        if watchers.contains_key(&project.path) {
            continue;
        }
        let project_path = PathBuf::from(&project.path);
        if !project_path.is_dir() {
            continue;
        }

        let event_tx = tx.clone();
        match notify::recommended_watcher(move |event| {
            let _ = event_tx.blocking_send(event);
        }) {
            Ok(mut watcher) => {
                if let Err(error) = watcher.watch(&project_path, RecursiveMode::Recursive) {
                    warn!(
                        project_path = %project_path.display(),
                        error = %error,
                        "failed to watch project"
                    );
                    continue;
                }
                watchers.insert(project.path, watcher);
            }
            Err(error) => warn!(
                project_path = %project_path.display(),
                error = %error,
                "failed to create project watcher"
            ),
        }
    }
}

fn is_interesting_watch_event(event: &notify::Event) -> bool {
    use notify::EventKind;
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

async fn publish_projects(state: &AppState) {
    match state.projects.list(&state.sessions).await {
        Ok(projects) => state
            .ws_hub
            .publish(WsServerEvent::ProjectsUpdated { projects }),
        Err(error) => warn!(error = %error, "failed to publish project list"),
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_string)
}

fn request_token(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    bearer_token(headers).or_else(|| {
        query.and_then(|query| {
            query.split('&').find_map(|part| {
                let (key, value) = part.split_once('=')?;
                (key == "token" && !value.is_empty()).then(|| value.to_string())
            })
        })
    })
}

fn user_setting_key(user_id: &str, key: &str) -> String {
    format!("user:{user_id}:{key}")
}

fn default_notification_preferences() -> Value {
    serde_json::json!({
        "channels": {
            "browser": true,
            "webPush": false
        },
        "events": {
            "sessionComplete": true,
            "permissionRequired": true,
            "processFailed": true
        }
    })
}

fn default_direct_ai_config() -> Value {
    serde_json::json!({
        "mode": "off",
        "baseUrl": null,
        "apiKeyEnv": null,
        "model": null
    })
}

async fn fetch_direct_ai_models(config: &Value) -> std::result::Result<Vec<Value>, ServerError> {
    let Some((base_url, api_key)) = direct_ai_endpoint_config(config) else {
        return Ok(Vec::new());
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create Direct AI client",
                error.to_string(),
            )
        })?;
    let response = client
        .get(format!("{base_url}/v1/models"))
        .bearer_auth(&api_key)
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::BAD_GATEWAY,
                "Direct AI model request failed",
                error.to_string(),
            )
        })?;
    if !response.status().is_success() {
        return Ok(Vec::new());
    }
    let body = response.json::<Value>().await.map_err(|error| {
        ServerError::with_details(
            StatusCode::BAD_GATEWAY,
            "Direct AI model response was invalid",
            error.to_string(),
        )
    })?;

    let raw_models = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(raw_models
        .into_iter()
        .filter_map(|model| {
            let value = model
                .as_str()
                .map(str::to_string)
                .or_else(|| model.get("id").and_then(Value::as_str).map(str::to_string))
                .or_else(|| {
                    model
                        .get("value")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .or_else(|| {
                    model
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })?;
            Some(serde_json::json!({
                "value": value,
                "label": value,
            }))
        })
        .collect())
}

fn direct_ai_endpoint_config(config: &Value) -> Option<(String, String)> {
    let mode = config.get("mode").and_then(Value::as_str).unwrap_or("off");
    if mode == "off" || mode.is_empty() {
        return None;
    }

    let base_url = config
        .get("baseUrl")
        .or_else(|| config.get("base_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .or_else(|| match mode {
            "direct" | "anthropic" => Some("https://api.anthropic.com".to_string()),
            "minimax" => Some("https://api.minimax.io/anthropic".to_string()),
            _ => None,
        })?;

    let env_key = config
        .get("apiKeyEnv")
        .or_else(|| config.get("api_key_env"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let api_key = env_key
        .and_then(|key| env::var(key).ok())
        .or_else(|| match mode {
            "direct" | "anthropic" => env::var("ANTHROPIC_API_KEY")
                .or_else(|_| env::var("ANTHROPIC_AUTH_TOKEN"))
                .ok(),
            "minimax" => env::var("MINIMAX_API_KEY")
                .or_else(|_| env::var("ANTHROPIC_API_KEY"))
                .ok(),
            _ => env::var("CODEX_GATEWAY_KEY").ok(),
        })
        .filter(|value| !value.trim().is_empty())?;

    Some((base_url, api_key))
}

fn validate_direct_ai_config(config: &Value) -> Result<()> {
    let allowed_modes = ["off", "proxy", "direct", "anthropic", "minimax", "aiproxy"];
    if let Some(mode) = config.get("mode").and_then(Value::as_str) {
        if !allowed_modes.contains(&mode) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "invalid direct AI mode",
            ));
        }
    }
    Ok(())
}

fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid sessionId",
        ));
    }
    Ok(())
}

fn validate_provider_name(provider: &str) -> Result<()> {
    parse_provider_param(provider).map(|_| ())
}

fn parse_provider_param(provider: &str) -> Result<Provider> {
    match provider {
        "claude" => Ok(Provider::Claude),
        "codex" => Ok(Provider::Codex),
        "cursor" => Ok(Provider::Cursor),
        "gemini" => Ok(Provider::Gemini),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Provider must be one of: claude, codex, cursor, gemini",
        )),
    }
}

fn stored_git_alias(stored: &Option<Value>, key: &str) -> Option<String> {
    stored
        .as_ref()
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn blocking_git_config(key: &str) -> Option<String> {
    std::process::Command::new("git")
        .args(["config", "--global", "--get", key])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn set_git_global_config(key: &str, value: &str) -> std::io::Result<()> {
    let status = Command::new("git")
        .args(["config", "--global", key, value])
        .status()
        .await?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "git config exited with status {status}"
        )))
    }
}

fn looks_like_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

async fn provider_cli_status(provider: Provider) -> Value {
    let command = provider_command(provider);
    let installed = command_available(command).await;
    let auth = provider_auth_hint(provider);
    let authenticated = auth.is_some();
    let error = if installed {
        (!authenticated).then_some("Not authenticated")
    } else {
        Some("CLI not installed or not in PATH")
    };

    serde_json::json!({
        "authenticated": authenticated,
        "email": auth.as_deref(),
        "method": auth_method(provider, auth.as_deref()),
        "error": error,
        "installed": installed,
        "command": command,
    })
}

fn provider_command(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::Cursor => "cursor-agent",
        Provider::Gemini => "gemini",
    }
}

async fn command_available(command: &str) -> bool {
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        Command::new(command).arg("--version").status(),
    )
    .await;
    matches!(result, Ok(Ok(_)))
}

fn provider_auth_hint(provider: Provider) -> Option<String> {
    let env_key = match provider {
        Provider::Claude => ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"]
            .into_iter()
            .find(|key| env_has_value(key)),
        Provider::Codex => ["OPENAI_API_KEY"]
            .into_iter()
            .find(|key| env_has_value(key)),
        Provider::Cursor => ["CURSOR_API_KEY"]
            .into_iter()
            .find(|key| env_has_value(key)),
        Provider::Gemini => ["GEMINI_API_KEY", "GOOGLE_API_KEY"]
            .into_iter()
            .find(|key| env_has_value(key)),
    };
    if let Some(env_key) = env_key {
        return Some(format!("API Key Auth ({env_key})"));
    }

    let home = home_dir()?;
    let configured = match provider {
        Provider::Claude => home.join(".claude").join(".credentials.json").is_file(),
        Provider::Codex => home.join(".codex").join("auth.json").is_file(),
        Provider::Cursor => home.join(".cursor").exists(),
        Provider::Gemini => home.join(".gemini").join("oauth_creds.json").is_file(),
    };
    configured.then(|| "Configured on disk".to_string())
}

fn auth_method(provider: Provider, auth: Option<&str>) -> Option<&'static str> {
    auth.map(|auth| {
        if auth.starts_with("API Key Auth") {
            "api_key"
        } else {
            match provider {
                Provider::Claude | Provider::Codex | Provider::Gemini => "credentials_file",
                Provider::Cursor => "cli",
            }
        }
    })
}

fn env_has_value(key: &str) -> bool {
    env::var(key)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

async fn claude_token_usage(project_path: &str, session_id: &str) -> Result<Value> {
    let home = home_dir()
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "home directory not found"))?;
    let encoded_project = encode_claude_project_path(project_path);
    let session_file = home
        .join(".claude")
        .join("projects")
        .join(encoded_project)
        .join(format!("{session_id}.jsonl"));

    let content = tokio::fs::read_to_string(&session_file)
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ServerError::with_details(
                    StatusCode::NOT_FOUND,
                    "Session file not found",
                    session_file.display().to_string(),
                )
            } else {
                ServerError::with_details(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to read session token usage",
                    error.to_string(),
                )
            }
        })?;

    let (input, cache_creation, cache_read) = parse_claude_usage(&content);
    let used = input + cache_creation + cache_read;
    let total = env::var("CONTEXT_WINDOW")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(160_000);

    Ok(serde_json::json!({
        "used": used,
        "total": total,
        "breakdown": {
            "input": input,
            "cacheCreation": cache_creation,
            "cacheRead": cache_read,
        },
    }))
}

async fn codex_token_usage(session_id: &str) -> Result<Value> {
    let home = home_dir()
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "home directory not found"))?;
    let sessions_dir = home.join(".codex").join("sessions");
    let session_id = session_id.to_string();
    let session_file = tokio::task::spawn_blocking(move || {
        walkdir::WalkDir::new(sessions_dir)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .find(|entry| {
                entry.file_type().is_file()
                    && entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.contains(&session_id) && name.ends_with(".jsonl"))
            })
            .map(|entry| entry.into_path())
    })
    .await
    .map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to search Codex sessions",
            error.to_string(),
        )
    })?
    .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Codex session file not found"))?;

    let content = tokio::fs::read_to_string(&session_file)
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ServerError::with_details(
                    StatusCode::NOT_FOUND,
                    "Session file not found",
                    session_file.display().to_string(),
                )
            } else {
                ServerError::with_details(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to read session token usage",
                    error.to_string(),
                )
            }
        })?;

    let (used, total) = parse_codex_usage(&content);
    Ok(serde_json::json!({
        "used": used,
        "total": total,
    }))
}

fn encode_claude_project_path(project_path: &str) -> String {
    project_path
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn parse_claude_usage(content: &str) -> (u64, u64, u64) {
    for line in content.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = value
            .get("message")
            .and_then(|message| message.get("usage"))
        else {
            continue;
        };
        return (
            usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            usage
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
    }
    (0, 0, 0)
}

fn parse_codex_usage(content: &str) -> (u64, u64) {
    for line in content.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(info) = value
            .get("payload")
            .filter(|_| value.get("type").and_then(Value::as_str) == Some("event_msg"))
            .and_then(|payload| {
                (payload.get("type").and_then(Value::as_str) == Some("token_count"))
                    .then_some(payload)
            })
            .and_then(|payload| payload.get("info"))
        else {
            continue;
        };

        let used = info
            .get("total_token_usage")
            .and_then(|usage| usage.get("total_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let total = info
            .get("model_context_window")
            .and_then(Value::as_u64)
            .unwrap_or(200_000);
        return (used, total);
    }
    (0, 200_000)
}

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

async fn run_danger(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<Value>,
) -> Result<Json<Value>> {
    run_external_tool_json(
        &state,
        &user.0.id,
        "danger",
        "IO_WORKBENCH_DANGER_COMMAND",
        "IO_WORKBENCH_DANGER_ARGS_JSON",
        "run",
        request,
    )
    .await
}

async fn list_danger_runs(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "success": true,
        "runs": load_tool_runs(&state, &user.0.id, "danger")?,
    })))
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
        "providers": ["claude", "codex", "cursor", "gemini"],
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

async fn danger_compat(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<Json<Value>> {
    persisted_compat_endpoint(
        state,
        user,
        request,
        "danger",
        serde_json::json!({ "runs": [] }),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_latest_claude_usage() {
        let content = r#"
{"type":"assistant","message":{"usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3}}}
{"type":"assistant","message":{"usage":{"input_tokens":10,"cache_creation_input_tokens":20,"cache_read_input_tokens":30}}}
"#;

        assert_eq!(parse_claude_usage(content), (10, 20, 30));
    }

    #[test]
    fn parses_latest_codex_usage() {
        let content = r#"
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":12},"model_context_window":1000}}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":42},"model_context_window":2000}}}
"#;

        assert_eq!(parse_codex_usage(content), (42, 2000));
    }

    #[test]
    fn sanitizes_repository_names() {
        assert_eq!(
            repository_name("https://github.com/example/io-workbench.git"),
            "io-workbench"
        );
        assert_eq!(repository_name("git@github.com:example/repo.git"), "repo");
        assert_eq!(
            repository_name("https://example.com/a bad repo.git"),
            "abadrepo"
        );
    }
}

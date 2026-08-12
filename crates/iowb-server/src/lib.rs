#![recursion_limit = "256"]

mod agentic_board;
mod database;
mod git;
mod rag_client;

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    convert::Infallible,
    env,
    path::{Path, PathBuf},
    time::{Duration, Instant},
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
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use iowb_core::{
    AppConfig, AppState, CoreError, DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS, DirectAiRuntimeConfig,
    augmented_user_path, generate_secret_token, hash_secret_token,
    terminate_orphaned_agent_run_processes,
};
use iowb_fs::FsError;
use iowb_process::{ProcessError, ProcessEvent};
use iowb_protocol::{
    ApiErrorBody, AuthStatusResponse, BatchCopyFileRequest, BatchDeleteFileRequest,
    BatchRenameFileRequest, BrowseFilesystemResponse, ChatMessage, ChatRuntime, CopyFileRequest,
    CreateFileRequest, CreateProjectRequest, CreateWorkspaceRequest, DeleteFcmTokenRequest,
    DeleteFileRequest, FcmTokenResponse, FileContentResponse, FileEntry, ForkSessionRequest,
    ForkSessionResponse, HealthResponse, HealthStatus, LoginRequest, MessageRole, MessagesResponse,
    PRODUCT_NAME, PlaceholderResponse, ProcessInputRequest, ProcessResizeRequest,
    ProcessStartRequest, ProcessStartResponse, ProjectListResponse, ProjectSummary,
    PromptHistoryCursor, PromptHistoryResponse, Provider, RegisterFcmTokenRequest,
    RenameFileRequest, ServerStatusResponse, SessionDraftResponse, SessionSnapshotResponse,
    SessionSummary, SessionTokenUsage, UpdateSessionDraftRequest, WS_COMMAND_CHANNEL_CAPACITY,
    WorkspaceType, WsClientCommand, WsServerEvent, new_id,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
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
const SESSION_HISTORY_DEFAULT_MESSAGES: usize = 30;
const SESSION_HISTORY_MAX_MESSAGES: usize = 100;
const SESSION_PROMPT_HISTORY_DEFAULT: usize = 10;
const SESSION_PROMPT_HISTORY_MAX: usize = 100;
const SESSION_RESPONSE_MAX_CONTENT_BYTES: usize = 512 * 1024;
const SESSION_RESPONSE_ASSISTANT_MAX_BYTES: usize = 256 * 1024;
const SESSION_RESPONSE_TOOL_MAX_BYTES: usize = 64 * 1024;
const SESSION_RESPONSE_USER_MAX_BYTES: usize = 128 * 1024;
const SESSION_RESPONSE_SYSTEM_MAX_BYTES: usize = 32 * 1024;
const SESSION_RESPONSE_METADATA_MAX_BYTES: usize = 16 * 1024;
const SESSION_RESPONSE_MAX_LINE_CHARS: usize = 8 * 1024;
const SESSION_DRAFT_MAX_BYTES: usize = 256 * 1024;
const STATIC_CACHE_CONTROL: &str = "no-cache";
const RESOURCE_SAMPLE_INTERVAL_MS: u64 = 160;
const SYS_THERMAL_PATH: &str = "/sys/class/thermal";
const SYS_HWMON_PATH: &str = "/sys/class/hwmon";
const SYS_POWER_SUPPLY_PATH: &str = "/sys/class/power_supply";
const DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL: &str = "http://141.144.197.96:8319/claude";
const IO_GATEWAY_API_KEY_CREDENTIAL: &str = "io-workbench-io-gateway-api-key";
const IO_GATEWAY_API_KEY_CREDENTIAL_TYPE: &str = "io_gateway_api_key";
const IO_GATEWAY_OTP_CREDENTIAL: &str = "io-workbench-io-gateway-otp";
const IO_GATEWAY_OTP_CREDENTIAL_TYPE: &str = "io_gateway_otp";

pub async fn serve(config: AppConfig) -> anyhow::Result<()> {
    let addr = config.socket_addr();
    let state = AppState::initialize(config).await?;
    recover_interrupted_chat_runs(&state).await?;
    spawn_process_event_bridge(state.clone());
    spawn_project_watch_bridge(state.clone());
    spawn_fcm_notification_bridge(state.clone());

    let listener = TcpListener::bind(addr).await?;
    info!(%addr, "starting io-workbench server");

    axum::serve(listener, build_router(state)).await?;
    Ok(())
}

async fn recover_interrupted_chat_runs(state: &AppState) -> anyhow::Result<()> {
    synthesize_legacy_durable_runs(state).await?;
    let mut active_runs = state.storage.list_active_durable_chat_runs()?;
    if active_runs.is_empty() {
        state
            .sessions
            .mark_unrecovered_active_sessions_interrupted(&HashSet::new())
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
        .list_sessions()?
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
    agentic_board::recover_active_runs(&state);

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
        .route("/api/cursor", any(cursor_compat))
        .route("/api/cursor/{*path}", any(cursor_compat))
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
    Ok(Json(serde_json::json!({
        "success": true,
        "metrics": runtime_metrics_payload(&state).await?,
    })))
}

async fn runtime_metrics_payload(state: &AppState) -> Result<Value> {
    let projects = state.projects.list(&state.sessions).await?;
    let active_sessions = state.sessions.list_active().await;
    let processes = state.processes.list().await;
    let resources = system_resource_metrics(&state.config.workspace_root).await;
    let process_uptime_seconds = resources
        .get("processUptimeSeconds")
        .cloned()
        .unwrap_or(Value::Null);
    Ok(serde_json::json!({
        "timestamp": Utc::now(),
        "memory": process_memory_metrics().await,
        "resources": resources,
        "server": {
            "status": "ok",
            "appRoot": state.config.workspace_root.display().to_string(),
            "installMode": "rust",
            "packageName": PRODUCT_NAME,
            "version": VERSION,
            "uptimeSeconds": process_uptime_seconds,
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "pid": std::process::id(),
            "port": state.config.port.to_string(),
            "environment": env::var("IO_WORKBENCH_ENV").unwrap_or_else(|_| "local".to_string()),
        },
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
    }))
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
        "virtualKb": vm_size_kb,
        "rssBytes": vm_rss_kb.map(|value| value * 1024),
        "virtualBytes": vm_size_kb.map(|value| value * 1024)
    })
}

fn parse_proc_status_kb(value: &str) -> Option<u64> {
    value
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u64>().ok())
}

#[derive(Debug, Clone, Copy)]
struct CpuTimes {
    idle: u64,
    total: u64,
}

#[derive(Debug, Clone)]
struct CpuSnapshot {
    aggregate: CpuTimes,
    cores: Vec<CpuTimes>,
}

#[derive(Debug, Clone)]
struct NetworkInterfaceSample {
    name: String,
    rx_bytes: u64,
    rx_packets: u64,
    rx_errors: u64,
    tx_bytes: u64,
    tx_packets: u64,
    tx_errors: u64,
}

async fn system_resource_metrics(workspace_root: &Path) -> Value {
    let previous_cpu = read_cpu_snapshot().await;
    let previous_network = read_network_snapshot().await;
    tokio::time::sleep(Duration::from_millis(RESOURCE_SAMPLE_INTERVAL_MS)).await;

    let current_cpu = read_cpu_snapshot().await;
    let current_network = read_network_snapshot().await;
    let memory = system_memory_metrics().await;
    let hardware = read_hardware_stats().await;
    let disk = disk_metrics(workspace_root).await;
    let load_average = read_load_average().await;
    let cpu_model = read_cpu_model().await;
    let system_uptime_seconds = read_system_uptime_seconds().await;
    let process_uptime_seconds = read_process_uptime_seconds(system_uptime_seconds).await;

    serde_json::json!({
        "cpu": cpu_metrics(previous_cpu, current_cpu, load_average, cpu_model),
        "memory": memory,
        "disk": disk,
        "network": network_metrics(previous_network, current_network),
        "hardware": hardware,
        "systemUptimeSeconds": system_uptime_seconds,
        "processUptimeSeconds": process_uptime_seconds,
    })
}

async fn read_text_path(path: impl AsRef<Path>) -> Option<String> {
    tokio::fs::read_to_string(path).await.ok()
}

async fn read_trimmed_path(path: impl AsRef<Path>) -> Option<String> {
    read_text_path(path)
        .await
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn read_sysfs_number(path: impl AsRef<Path>) -> Option<f64> {
    read_trimmed_path(path)
        .await
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

async fn read_directory_paths(directory: &str) -> Vec<PathBuf> {
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let is_directory_like = entry
            .file_type()
            .await
            .ok()
            .is_some_and(|file_type| file_type.is_dir() || file_type.is_symlink());
        if is_directory_like {
            paths.push(entry.path());
        }
    }
    paths
}

async fn read_directory_file_names(directory: &Path) -> Vec<String> {
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return Vec::new();
    };
    let mut names = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names
}

fn path_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string()
}

fn json_f64(value: Option<f64>) -> Value {
    value
        .filter(|value| value.is_finite())
        .map(Value::from)
        .unwrap_or(Value::Null)
}

fn json_u64(value: Option<u64>) -> Value {
    value.map(Value::from).unwrap_or(Value::Null)
}

fn parse_cpu_line(line: &str) -> Option<CpuTimes> {
    let mut parts = line.split_whitespace();
    let _name = parts.next()?;
    let values: Vec<u64> = parts
        .filter_map(|value| value.parse::<u64>().ok())
        .collect();
    if values.len() < 4 {
        return None;
    }
    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    let total = values.iter().copied().sum();
    Some(CpuTimes { idle, total })
}

async fn read_cpu_snapshot() -> Option<CpuSnapshot> {
    let content = read_text_path("/proc/stat").await?;
    let aggregate = content
        .lines()
        .find(|line| line.starts_with("cpu "))
        .and_then(parse_cpu_line)?;
    let cores = content
        .lines()
        .filter(|line| {
            line.strip_prefix("cpu")
                .and_then(|rest| rest.chars().next())
                .is_some_and(|ch| ch.is_ascii_digit())
        })
        .filter_map(parse_cpu_line)
        .collect();
    Some(CpuSnapshot { aggregate, cores })
}

fn calculate_cpu_percent(previous: CpuTimes, current: CpuTimes) -> Option<f64> {
    let total_delta = current.total.checked_sub(previous.total)?;
    let idle_delta = current.idle.checked_sub(previous.idle)?;
    if total_delta == 0 {
        return None;
    }
    Some(((total_delta.saturating_sub(idle_delta)) as f64 / total_delta as f64) * 100.0)
}

fn cpu_metrics(
    previous: Option<CpuSnapshot>,
    current: Option<CpuSnapshot>,
    load_average: Vec<f64>,
    model: String,
) -> Value {
    let usage_percent = previous
        .as_ref()
        .zip(current.as_ref())
        .and_then(|(previous, current)| {
            calculate_cpu_percent(previous.aggregate, current.aggregate)
        });
    let per_core = current
        .as_ref()
        .map(|current| {
            current
                .cores
                .iter()
                .enumerate()
                .map(|(index, current_core)| {
                    let usage = previous
                        .as_ref()
                        .and_then(|previous| previous.cores.get(index).copied())
                        .and_then(|previous_core| {
                            calculate_cpu_percent(previous_core, *current_core)
                        });
                    serde_json::json!({
                        "index": index,
                        "usagePercent": json_f64(usage),
                        "temperatureCelsius": Value::Null,
                        "temperatureLabel": Value::Null,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let cores = current
        .as_ref()
        .map(|snapshot| snapshot.cores.len())
        .unwrap_or(0);

    serde_json::json!({
        "usagePercent": json_f64(usage_percent),
        "processUsagePercent": Value::Null,
        "loadAverage": load_average,
        "cores": cores,
        "model": model,
        "perCore": per_core,
    })
}

async fn read_cpu_model() -> String {
    let Some(content) = read_text_path("/proc/cpuinfo").await else {
        return "Unknown CPU".to_string();
    };
    for key in ["model name", "Hardware", "Processor"] {
        if let Some(value) = content.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name.trim() == key).then(|| value.trim().to_string())
        }) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    "Unknown CPU".to_string()
}

async fn read_load_average() -> Vec<f64> {
    read_trimmed_path("/proc/loadavg")
        .await
        .map(|content| {
            content
                .split_whitespace()
                .take(3)
                .filter_map(|value| value.parse::<f64>().ok())
                .collect()
        })
        .unwrap_or_default()
}

async fn system_memory_metrics() -> Value {
    let Some(content) = read_text_path("/proc/meminfo").await else {
        return serde_json::json!({
            "total": 0,
            "used": 0,
            "free": 0,
            "available": 0,
            "usedPercent": Value::Null,
            "cached": 0,
            "buffers": 0,
            "swap": {
                "total": 0,
                "used": 0,
                "free": 0,
                "usedPercent": 0.0,
            }
        });
    };
    let mut values = HashMap::<String, u64>::new();
    for line in content.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if let Some(kb) = value
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u64>().ok())
        {
            values.insert(key.to_string(), kb * 1024);
        }
    }

    let total = values.get("MemTotal").copied().unwrap_or(0);
    let available = values
        .get("MemAvailable")
        .copied()
        .unwrap_or_else(|| values.get("MemFree").copied().unwrap_or(0));
    let free = values.get("MemFree").copied().unwrap_or(available);
    let cached = values.get("Cached").copied().unwrap_or(0)
        + values.get("SReclaimable").copied().unwrap_or(0);
    let buffers = values.get("Buffers").copied().unwrap_or(0);
    let swap_total = values.get("SwapTotal").copied().unwrap_or(0);
    let swap_free = values.get("SwapFree").copied().unwrap_or(0);
    let used = total.saturating_sub(available);
    let swap_used = swap_total.saturating_sub(swap_free);
    let used_percent = (total > 0).then(|| used as f64 / total as f64 * 100.0);
    let swap_percent = (swap_total > 0)
        .then(|| swap_used as f64 / swap_total as f64 * 100.0)
        .unwrap_or(0.0);

    serde_json::json!({
        "total": total,
        "used": used,
        "free": free,
        "available": available,
        "usedPercent": json_f64(used_percent),
        "cached": cached,
        "buffers": buffers,
        "swap": {
            "total": swap_total,
            "used": swap_used,
            "free": swap_free,
            "usedPercent": swap_percent,
        }
    })
}

async fn disk_metrics(path: &Path) -> Value {
    let output = timeout(
        Duration::from_secs(2),
        Command::new("df").arg("-PB1").arg(path).output(),
    )
    .await;
    let Ok(Ok(output)) = output else {
        return Value::Null;
    };
    if !output.status.success() {
        return Value::Null;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(line) = stdout.lines().nth(1) else {
        return Value::Null;
    };
    let columns: Vec<&str> = line.split_whitespace().collect();
    if columns.len() < 6 {
        return Value::Null;
    }
    let total = columns.get(1).and_then(|value| value.parse::<u64>().ok());
    let used = columns.get(2).and_then(|value| value.parse::<u64>().ok());
    let available = columns.get(3).and_then(|value| value.parse::<u64>().ok());
    let used_percent = total
        .zip(used)
        .and_then(|(total, used)| (total > 0).then(|| used as f64 / total as f64 * 100.0));

    serde_json::json!({
        "filesystem": columns[0],
        "mount": columns[5],
        "total": json_u64(total),
        "used": json_u64(used),
        "available": json_u64(available),
        "free": json_u64(available),
        "usedPercent": json_f64(used_percent),
    })
}

async fn read_network_snapshot() -> Option<Vec<NetworkInterfaceSample>> {
    let content = read_text_path("/proc/net/dev").await?;
    let mut interfaces = Vec::new();
    for line in content.lines().skip(2) {
        let Some((name, values)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || name == "lo" {
            continue;
        }
        let numbers: Vec<u64> = values
            .split_whitespace()
            .filter_map(|value| value.parse::<u64>().ok())
            .collect();
        if numbers.len() < 16 {
            continue;
        }
        interfaces.push(NetworkInterfaceSample {
            name: name.to_string(),
            rx_bytes: numbers[0],
            rx_packets: numbers[1],
            rx_errors: numbers[2],
            tx_bytes: numbers[8],
            tx_packets: numbers[9],
            tx_errors: numbers[10],
        });
    }
    Some(interfaces)
}

fn network_metrics(
    previous: Option<Vec<NetworkInterfaceSample>>,
    current: Option<Vec<NetworkInterfaceSample>>,
) -> Value {
    let elapsed_seconds = RESOURCE_SAMPLE_INTERVAL_MS as f64 / 1000.0;
    let current = current.unwrap_or_default();
    let previous_by_name: HashMap<String, NetworkInterfaceSample> = previous
        .unwrap_or_default()
        .into_iter()
        .map(|sample| (sample.name.clone(), sample))
        .collect();
    let mut rx_bytes = 0_u64;
    let mut tx_bytes = 0_u64;
    let mut rx_rate = 0.0;
    let mut tx_rate = 0.0;
    let interfaces = current
        .into_iter()
        .map(|sample| {
            let previous = previous_by_name.get(&sample.name);
            let sample_rx_rate = previous
                .map(|previous| {
                    sample.rx_bytes.saturating_sub(previous.rx_bytes) as f64 / elapsed_seconds
                })
                .unwrap_or(0.0);
            let sample_tx_rate = previous
                .map(|previous| {
                    sample.tx_bytes.saturating_sub(previous.tx_bytes) as f64 / elapsed_seconds
                })
                .unwrap_or(0.0);
            rx_bytes = rx_bytes.saturating_add(sample.rx_bytes);
            tx_bytes = tx_bytes.saturating_add(sample.tx_bytes);
            rx_rate += sample_rx_rate;
            tx_rate += sample_tx_rate;
            serde_json::json!({
                "name": sample.name,
                "rxBytes": sample.rx_bytes,
                "txBytes": sample.tx_bytes,
                "rxRateBytesPerSecond": sample_rx_rate,
                "txRateBytesPerSecond": sample_tx_rate,
                "rxPackets": sample.rx_packets,
                "txPackets": sample.tx_packets,
                "rxErrors": sample.rx_errors,
                "txErrors": sample.tx_errors,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "rxBytes": rx_bytes,
        "txBytes": tx_bytes,
        "rxRateBytesPerSecond": rx_rate,
        "txRateBytesPerSecond": tx_rate,
        "interfaces": interfaces,
    })
}

async fn read_system_uptime_seconds() -> Option<u64> {
    read_trimmed_path("/proc/uptime")
        .await
        .and_then(|content| content.split_whitespace().next()?.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| value.floor() as u64)
}

async fn read_process_uptime_seconds(system_uptime_seconds: Option<u64>) -> Option<u64> {
    let system_uptime_seconds = system_uptime_seconds?;
    let stat = read_trimmed_path("/proc/self/stat").await?;
    let after_command = stat.rsplit_once(')')?.1.trim();
    let fields: Vec<&str> = after_command.split_whitespace().collect();
    let start_ticks = fields.get(19)?.parse::<u64>().ok()?;
    let start_seconds = start_ticks / clock_ticks_per_second().max(1);
    Some(system_uptime_seconds.saturating_sub(start_seconds))
}

fn clock_ticks_per_second() -> u64 {
    env::var("IO_WORKBENCH_CLK_TCK")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(100)
}

fn normalize_temperature_celsius(raw_value: Option<f64>) -> Option<f64> {
    let raw_value = raw_value?;
    let celsius = if raw_value.abs() > 1000.0 {
        raw_value / 1000.0
    } else {
        raw_value
    };
    (-100.0..=250.0).contains(&celsius).then_some(celsius)
}

async fn read_thermal_zone_temperatures() -> Vec<Value> {
    let mut sensors = Vec::new();
    for zone_path in read_directory_paths(SYS_THERMAL_PATH).await {
        let name = path_file_name(&zone_path);
        if !name.starts_with("thermal_zone") {
            continue;
        }
        let raw_temp = read_sysfs_number(zone_path.join("temp")).await;
        let Some(celsius) = normalize_temperature_celsius(raw_temp) else {
            continue;
        };
        let label = read_trimmed_path(zone_path.join("type"))
            .await
            .unwrap_or_else(|| name.clone());
        sensors.push(serde_json::json!({
            "id": name,
            "label": label,
            "celsius": celsius,
            "source": "thermal",
            "path": zone_path.display().to_string(),
        }));
    }
    sensors
}

async fn read_hwmon_stats() -> (Vec<Value>, Vec<Value>) {
    let mut temperature_sensors = Vec::new();
    let mut fans = Vec::new();
    for hwmon_path in read_directory_paths(SYS_HWMON_PATH).await {
        let name = path_file_name(&hwmon_path);
        if !name.starts_with("hwmon") {
            continue;
        }
        let hwmon_name = read_trimmed_path(hwmon_path.join("name")).await;
        let files = read_directory_file_names(&hwmon_path).await;
        for file_name in files {
            if let Some(index) = file_name
                .strip_prefix("temp")
                .and_then(|value| value.strip_suffix("_input"))
            {
                let raw_temp = read_sysfs_number(hwmon_path.join(&file_name)).await;
                let Some(celsius) = normalize_temperature_celsius(raw_temp) else {
                    continue;
                };
                let label = read_trimmed_path(hwmon_path.join(format!("temp{index}_label")))
                    .await
                    .or_else(|| hwmon_name.clone())
                    .unwrap_or_else(|| format!("Temperature {index}"));
                temperature_sensors.push(serde_json::json!({
                    "id": format!("{name}:temp{index}"),
                    "label": label,
                    "celsius": celsius,
                    "source": "hwmon",
                    "path": hwmon_path.display().to_string(),
                }));
                continue;
            }

            if let Some(index) = file_name
                .strip_prefix("fan")
                .and_then(|value| value.strip_suffix("_input"))
            {
                let Some(rpm) = read_sysfs_number(hwmon_path.join(&file_name)).await else {
                    continue;
                };
                let label = read_trimmed_path(hwmon_path.join(format!("fan{index}_label")))
                    .await
                    .or_else(|| hwmon_name.clone())
                    .unwrap_or_else(|| format!("Fan {index}"));
                let fault = read_sysfs_number(hwmon_path.join(format!("fan{index}_fault")))
                    .await
                    .unwrap_or(0.0);
                let alarm = read_sysfs_number(hwmon_path.join(format!("fan{index}_alarm")))
                    .await
                    .unwrap_or(0.0);
                let status = if fault > 0.0 || alarm > 0.0 {
                    "fault"
                } else if rpm > 0.0 {
                    "ok"
                } else {
                    "stopped"
                };
                fans.push(serde_json::json!({
                    "id": format!("{name}:fan{index}"),
                    "label": label,
                    "rpm": rpm.max(0.0),
                    "status": status,
                    "source": "hwmon",
                    "path": hwmon_path.display().to_string(),
                }));
            }
        }
    }
    (temperature_sensors, fans)
}

fn temperature_value(sensor: &Value) -> f64 {
    sensor.get("celsius").and_then(Value::as_f64).unwrap_or(0.0)
}

fn temperature_sensor_score(sensor: &Value) -> i32 {
    let label = format!(
        "{} {}",
        sensor
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        sensor.get("id").and_then(Value::as_str).unwrap_or_default()
    )
    .to_ascii_lowercase();
    let mut score = 0;
    for token in ["cpu", "processor", "coretemp", "k10temp", "zenpower"] {
        if label.contains(token) {
            score += 5;
        }
    }
    for token in ["package", "x86_pkg", "tctl", "tdie"] {
        if label.contains(token) {
            score += 4;
        }
    }
    if label.contains("core") {
        score += 2;
    }
    for token in ["nvme", "gpu", "wifi", "pch"] {
        if label.contains(token) {
            score -= 4;
        }
    }
    score
}

fn select_processor_temperature(sensors: &[Value]) -> Value {
    sensors
        .iter()
        .filter_map(|sensor| {
            let score = temperature_sensor_score(sensor);
            (score > 0).then_some((score, sensor))
        })
        .max_by(|(left_score, left), (right_score, right)| {
            left_score.cmp(right_score).then_with(|| {
                temperature_value(left)
                    .partial_cmp(&temperature_value(right))
                    .unwrap_or(Ordering::Equal)
            })
        })
        .map(|(_, sensor)| sensor.clone())
        .unwrap_or(Value::Null)
}

async fn read_battery_stats() -> Vec<Value> {
    let mut batteries = Vec::new();
    for battery_path in read_directory_paths(SYS_POWER_SUPPLY_PATH).await {
        let type_value = read_trimmed_path(battery_path.join("type")).await;
        if !type_value
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("battery"))
        {
            continue;
        }
        let raw_capacity = read_sysfs_number(battery_path.join("capacity")).await;
        let energy_now = read_sysfs_number(battery_path.join("energy_now")).await;
        let energy_full = read_sysfs_number(battery_path.join("energy_full")).await;
        let charge_now = read_sysfs_number(battery_path.join("charge_now")).await;
        let charge_full = read_sysfs_number(battery_path.join("charge_full")).await;
        let energy_percent = energy_now
            .zip(energy_full)
            .and_then(|(now, full)| (full > 0.0).then(|| now / full * 100.0));
        let charge_percent = charge_now
            .zip(charge_full)
            .and_then(|(now, full)| (full > 0.0).then(|| now / full * 100.0));
        let level_percent = raw_capacity
            .or(energy_percent)
            .or(charge_percent)
            .map(|value| value.clamp(0.0, 100.0));
        batteries.push(serde_json::json!({
            "name": path_file_name(&battery_path),
            "levelPercent": json_f64(level_percent),
            "status": read_trimmed_path(battery_path.join("status")).await,
            "manufacturer": read_trimmed_path(battery_path.join("manufacturer")).await,
            "model": read_trimmed_path(battery_path.join("model_name")).await,
            "technology": read_trimmed_path(battery_path.join("technology")).await,
            "path": battery_path.display().to_string(),
        }));
    }
    batteries
}

async fn read_hardware_stats() -> Value {
    let (mut hwmon_temperatures, fans) = read_hwmon_stats().await;
    hwmon_temperatures.extend(read_thermal_zone_temperatures().await);
    hwmon_temperatures.sort_by(|left, right| {
        temperature_value(right)
            .partial_cmp(&temperature_value(left))
            .unwrap_or(Ordering::Equal)
    });
    let processor_temperature = select_processor_temperature(&hwmon_temperatures);
    let temperature_sensors = hwmon_temperatures.into_iter().take(16).collect::<Vec<_>>();
    serde_json::json!({
        "processorTemperature": processor_temperature,
        "temperatureSensors": temperature_sensors,
        "fans": fans,
        "batteries": read_battery_stats().await,
    })
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListProjectsQuery {
    include_sessions: Option<bool>,
}

async fn list_projects(
    State(state): State<AppState>,
    Query(query): Query<ListProjectsQuery>,
) -> Result<Json<ProjectListResponse>> {
    let mut projects = if query.include_sessions.unwrap_or(true) {
        state.projects.list(&state.sessions).await?
    } else {
        state.storage.list_projects()?
    };
    populate_repository_names(&mut projects).await;
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

    let mut project = state.projects.add_project(&path)?;
    project.repo_name = project_repository_name(&project.path).await;
    publish_projects(&state).await;
    Ok(Json(project))
}

async fn populate_repository_names(projects: &mut [ProjectSummary]) {
    for project in projects {
        project.repo_name = project_repository_name(&project.path).await;
    }
}

async fn project_repository_name(project_path: &str) -> Option<String> {
    let config = tokio::fs::read_to_string(Path::new(project_path).join(".git/config"))
        .await
        .ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let value = line.trim();
        if value.starts_with('[') {
            in_origin = value == r#"[remote "origin"]"#;
            continue;
        }
        if !in_origin {
            continue;
        }
        let Some((key, remote)) = value.split_once('=') else {
            continue;
        };
        if key.trim() != "url" {
            continue;
        }
        return repository_name_from_remote(remote.trim());
    }
    None
}

fn repository_name_from_remote(remote: &str) -> Option<String> {
    remote
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .map(|name| name.trim_end_matches(".git").trim())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
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
struct RenameProjectRequest {
    name: String,
}

async fn rename_project(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<RenameProjectRequest>,
) -> Result<Json<ProjectSummary>> {
    let mut project = state.projects.find_by_name(&project_name)?;
    let name = request.name.trim();
    if name.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "project name is required",
        ));
    }
    if name.len() > 200 || name.chars().any(char::is_control) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "project name must be 200 printable characters or fewer",
        ));
    }
    if let Some(existing) = state.storage.find_project_by_name(name)?
        && existing.id != project.id
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "another project already uses that name",
        ));
    }

    project.name = name.to_string();
    project.updated_at = Utc::now();
    state.storage.upsert_project(&project)?;
    project.sessions = state.sessions.list_for_project(&project.path).await?;
    publish_projects(&state).await;
    Ok(Json(project))
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    path: Option<String>,
    #[serde(rename = "filePath")]
    file_path: Option<String>,
    #[serde(rename = "dirPath")]
    dir_path: Option<String>,
    #[serde(rename = "maxDepth")]
    max_depth: Option<usize>,
}

impl FileQuery {
    fn requested_path(&self) -> &str {
        self.dir_path
            .as_deref()
            .or(self.file_path.as_deref())
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
            .list_tree_with_depth(
                project.path,
                query.requested_path(),
                query.max_depth.unwrap_or(state.config.max_scan_depth),
            )
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

async fn rename_project_files_batch(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<BatchRenameFileRequest>,
) -> Result<Json<Vec<FileEntry>>> {
    let project = state.projects.find_by_name(&project_name)?;
    let mut renamed = Vec::with_capacity(request.entries.len());
    for entry in request.entries {
        renamed.push(
            state
                .files
                .rename_path(project.path.clone(), entry.old_path, entry.new_path)
                .await?,
        );
    }
    Ok(Json(renamed))
}

async fn copy_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<CopyFileRequest>,
) -> Result<Json<FileEntry>> {
    let project = state.projects.find_by_name(&project_name)?;
    Ok(Json(
        state
            .files
            .copy_path(project.path, request.source_path, request.target_path)
            .await?,
    ))
}

async fn copy_project_files_batch(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<BatchCopyFileRequest>,
) -> Result<Json<Vec<FileEntry>>> {
    let project = state.projects.find_by_name(&project_name)?;
    let mut copied = Vec::with_capacity(request.entries.len());
    for entry in request.entries {
        copied.push(
            state
                .files
                .copy_path(project.path.clone(), entry.source_path, entry.target_path)
                .await?,
        );
    }
    Ok(Json(copied))
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

async fn delete_project_files_batch(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<BatchDeleteFileRequest>,
) -> Result<Json<PlaceholderResponse>> {
    let project = state.projects.find_by_name(&project_name)?;
    let count = request.paths.len();
    for path in request.paths {
        state.files.delete_path(project.path.clone(), path).await?;
    }
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: format!("deleted: {count} item(s)"),
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
        "settings": public_settings(state.storage.list_settings()?),
    })))
}

#[derive(Debug, Default, Deserialize)]
struct MobileSettingsOverviewQuery {
    section: Option<String>,
}

async fn mobile_settings_overview(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<MobileSettingsOverviewQuery>,
) -> Result<Json<Value>> {
    let user_id = &user.0.id;
    match query.section.as_deref() {
        Some("agents") => {
            let (
                claude_status,
                codex_status,
                gemini_status,
                cursor_status,
                claude_mcp_config,
                cursor_mcp_config,
                codex_mcp_config,
            ) = tokio::join!(
                provider_cli_status(Provider::Claude),
                provider_cli_status(Provider::Codex),
                provider_cli_status(Provider::Gemini),
                cursor_cli_status(),
                claude_mcp_config_overview(&state.config.workspace_root),
                cursor_mcp_config_overview(),
                codex_mcp_config_overview(),
            );
            let providers = serde_json::json!({
                "claude": claude_status,
                "codex": codex_status,
                "gemini": gemini_status,
                "cursor": cursor_status,
            });
            return Ok(Json(serde_json::json!({
                "success": true,
                "agents": {
                    "providers": providers,
                    "permissions": {
                        "claude": state.storage
                            .get_setting(&user_setting_key(user_id, "claude-settings"))?
                            .unwrap_or_else(default_claude_agent_settings),
                        "cursor": state.storage
                            .get_setting(&user_setting_key(user_id, "cursor-tools-settings"))?
                            .unwrap_or_else(default_cursor_agent_settings),
                        "codex": state.storage
                            .get_setting(&user_setting_key(user_id, "codex-settings"))?
                            .unwrap_or_else(default_codex_agent_settings),
                        "gemini": state.storage
                            .get_setting(&user_setting_key(user_id, "gemini-settings"))?
                            .unwrap_or_else(default_gemini_agent_settings),
                    },
                    "mcp": {
                        "servers": load_mcp_servers(&state, user_id)?,
                        "claude": { "config": claude_mcp_config },
                        "cursor": { "config": cursor_mcp_config },
                        "codex": { "config": codex_mcp_config },
                    },
                    "models": {
                        "claude": fallback_models(Provider::Claude),
                        "cursor": ["gpt-5", "gpt-4.1", "claude-sonnet-4-5"],
                        "codex": fallback_models(Provider::Codex),
                        "gemini": fallback_models(Provider::Gemini),
                    }
                }
            })));
        }
        Some("appearance") => {
            return Ok(Json(serde_json::json!({
                "success": true,
                "appearance": state.storage
                    .get_setting(&user_setting_key(user_id, "appearance-settings"))?
                    .unwrap_or_else(default_appearance_settings),
            })));
        }
        Some("tasks") => {
            return Ok(Json(serde_json::json!({
                "success": true,
                "tasks": state.storage
                    .get_setting(&user_setting_key(user_id, "tasks-settings"))?
                    .unwrap_or_else(default_tasks_settings),
            })));
        }
        Some("plugins") => {
            return Ok(Json(serde_json::json!({
                "success": true,
                "plugins": compat_value(
                    &state,
                    user_id,
                    "plugins",
                    "/api/plugins",
                    serde_json::json!({
                        "plugins": [],
                        "namespace": "plugins",
                        "path": "/api/plugins"
                    }),
                )?,
            })));
        }
        _ => {}
    }

    let direct_ai_status = io_gateway_settings_status(&state, user_id)?;
    let direct_ai_effective = direct_ai_status
        .get("effective")
        .cloned()
        .unwrap_or_else(default_direct_ai_config);
    let (runtime, claude_status, codex_status, gemini_status, cursor_status, direct_ai_models) = tokio::join!(
        runtime_metrics_payload(&state),
        provider_cli_status(Provider::Claude),
        provider_cli_status(Provider::Codex),
        provider_cli_status(Provider::Gemini),
        cursor_cli_status(),
        timeout(
            Duration::from_secs(2),
            fetch_direct_ai_models(&direct_ai_effective),
        ),
    );
    let runtime = runtime?;
    let mut providers = serde_json::Map::new();
    providers.insert(Provider::Claude.as_str().to_string(), claude_status);
    providers.insert(Provider::Codex.as_str().to_string(), codex_status);
    providers.insert(Provider::Gemini.as_str().to_string(), gemini_status);
    providers.insert("cursor".to_string(), cursor_status);
    let direct_ai_models = match direct_ai_models {
        Ok(Ok(models)) => models,
        _ => Vec::new(),
    };
    let git = current_git_config_overview(&state, user_id)?;
    let api_keys = state.storage.list_api_keys(user_id)?;
    let credentials = state.storage.list_credentials(user_id, None)?;
    let github_credentials = state
        .storage
        .list_credentials(user_id, Some("github_token"))?;
    let notification_preferences = state
        .storage
        .get_setting(&user_setting_key(user_id, "notification-preferences"))?
        .unwrap_or_else(default_notification_preferences);
    let plugins = compat_value(
        &state,
        user_id,
        "plugins",
        "/api/plugins",
        serde_json::json!({
            "plugins": [],
            "namespace": "plugins",
            "path": "/api/plugins"
        }),
    )?;
    let claude_mcp_config = claude_mcp_config_overview(&state.config.workspace_root).await;
    let cursor_config = cursor_config_overview().await;
    let cursor_mcp_config = cursor_mcp_config_overview().await;
    let codex_config = codex_config_overview().await;
    let codex_mcp_config = codex_mcp_config_overview().await;
    let claude_mcp_cli = compat_value(
        &state,
        user_id,
        "mcp",
        "/api/mcp/cli/list",
        serde_json::json!({ "success": true, "servers": [] }),
    )?;
    let codex_mcp_cli = compat_value(
        &state,
        user_id,
        "provider",
        "/api/codex/mcp/cli/list",
        serde_json::json!({ "success": true, "servers": [] }),
    )?;
    let mcp_utils_all = compat_value(
        &state,
        user_id,
        "mcp-utils",
        "/api/mcp-utils/all-servers",
        serde_json::json!({ "success": true, "servers": {} }),
    )?;
    let runtime_resources = runtime.get("resources").cloned().unwrap_or(Value::Null);
    let process_uptime_seconds = runtime_resources
        .get("processUptimeSeconds")
        .cloned()
        .unwrap_or(Value::Null);

    Ok(Json(serde_json::json!({
        "success": true,
        "server": {
            "status": state.config.server_status(VERSION),
            "runtime": runtime,
            "webStatus": {
                "success": true,
                "server": {
                    "status": "ok",
                    "timestamp": Utc::now(),
                    "appRoot": state.config.workspace_root.display().to_string(),
                    "installMode": "rust",
                    "packageName": PRODUCT_NAME,
                    "version": VERSION,
                    "uptimeSeconds": process_uptime_seconds,
                    "platform": std::env::consts::OS,
                    "arch": std::env::consts::ARCH,
                    "pid": std::process::id(),
                    "port": state.config.port.to_string(),
                    "environment": env::var("IO_WORKBENCH_ENV").unwrap_or_else(|_| "local".to_string()),
                    "resources": runtime_resources,
                }
            },
        },
        "agents": {
            "providers": providers,
            "permissions": {
                "claude": state.storage
                    .get_setting(&user_setting_key(user_id, "claude-settings"))?
                    .unwrap_or_else(default_claude_agent_settings),
                "cursor": state.storage
                    .get_setting(&user_setting_key(user_id, "cursor-tools-settings"))?
                    .unwrap_or_else(default_cursor_agent_settings),
                "codex": state.storage
                    .get_setting(&user_setting_key(user_id, "codex-settings"))?
                    .unwrap_or_else(default_codex_agent_settings),
                "gemini": state.storage
                    .get_setting(&user_setting_key(user_id, "gemini-settings"))?
                    .unwrap_or_else(default_gemini_agent_settings),
            },
            "mcp": {
                "servers": load_mcp_servers(&state, user_id)?,
                "claude": {
                    "config": claude_mcp_config,
                    "cli": claude_mcp_cli,
                },
                "cursor": {
                    "config": cursor_mcp_config,
                },
                "codex": {
                    "config": codex_mcp_config,
                    "cli": codex_mcp_cli,
                },
                "utils": {
                    "allServers": mcp_utils_all,
                },
            },
            "models": {
                "claude": fallback_models(Provider::Claude),
                "cursor": ["gpt-5", "gpt-4.1", "claude-sonnet-4-5"],
                "codex": fallback_models(Provider::Codex),
                "gemini": fallback_models(Provider::Gemini),
            }
        },
        "cursor": {
            "config": cursor_config,
            "mcp": cursor_mcp_config,
        },
        "codex": {
            "config": codex_config,
            "mcp": codex_mcp_config,
        },
        "appearance": state.storage
            .get_setting(&user_setting_key(user_id, "appearance-settings"))?
            .unwrap_or_else(default_appearance_settings),
        "git": git,
        "api": {
            "apiKeys": api_keys,
            "credentials": credentials,
            "githubCredentials": github_credentials,
        },
        "credentials": {
            "all": credentials,
            "github": github_credentials,
        },
        "tasks": state.storage
            .get_setting(&user_setting_key(user_id, "tasks-settings"))?
            .unwrap_or_else(default_tasks_settings),
        "notifications": {
            "preferences": notification_preferences,
            "fcm": {
                "enabled": env_bool_local("IO_WORKBENCH_FCM_ENABLED", false),
                "configured": fcm_config_from_env().is_some()
            },
        },
        "plugins": plugins,
        "directAi": {
            "config": direct_ai_status.get("config").cloned().unwrap_or(Value::Null),
            "effective": direct_ai_status.get("effective").cloned().unwrap_or(Value::Null),
            "runtimeReady": direct_ai_status.get("runtimeReady").cloned().unwrap_or(Value::Bool(false)),
            "apiKeyConfigured": direct_ai_status.get("apiKeyConfigured").cloned().unwrap_or(Value::Bool(false)),
            "auth": direct_ai_status.get("auth").cloned().unwrap_or(Value::Null),
            "models": direct_ai_models,
            "modelsEndpoint": "/api/settings/direct-ai/models",
        },
        "settings": {
            "all": public_settings(state.storage.list_settings()?),
        },
        "about": {
            "product": PRODUCT_NAME,
            "version": VERSION,
        }
    })))
}

async fn get_setting(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<String>,
) -> Result<Json<Value>> {
    let mut value = state
        .storage
        .get_setting(&key)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "setting not found"))?;
    if is_io_gateway_setting_key(&key) {
        value = public_direct_ai_config(&value);
    }
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

async fn set_agent_preferences(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(provider): AxumPath<String>,
    Json(settings): Json<Value>,
) -> Result<Json<Value>> {
    if !settings.is_object() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "agent settings must be a JSON object",
        ));
    }
    let setting_name = match provider.as_str() {
        "claude" => "claude-settings",
        "cursor" => "cursor-tools-settings",
        "codex" => "codex-settings",
        "gemini" => "gemini-settings",
        _ => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "provider must be claude, cursor, codex, or gemini",
            ));
        }
    };
    let key = user_setting_key(&user.0.id, setting_name);
    state.storage.set_setting(&key, &settings)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "provider": provider,
        "settings": settings,
    })))
}

async fn get_sidebar_active_sessions(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    let pinned_sessions = load_sidebar_active_sessions(&state, &user.0.id)?;
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
    state
        .storage
        .set_setting(SIDEBAR_ACTIVE_SESSIONS_KEY, &pinned_sessions)?;
    state.storage.set_setting(
        &user_setting_key(&user.0.id, SIDEBAR_ACTIVE_SESSIONS_KEY),
        &pinned_sessions,
    )?;
    Ok(Json(serde_json::json!({
        "success": true,
        "pinnedSessions": pinned_sessions,
    })))
}

const SIDEBAR_ACTIVE_SESSIONS_KEY: &str = "sidebar-active-sessions";

fn load_sidebar_active_sessions(state: &AppState, user_id: &str) -> Result<Value> {
    let user_key = user_setting_key(user_id, SIDEBAR_ACTIVE_SESSIONS_KEY);
    let user_key_suffix = format!(":{SIDEBAR_ACTIVE_SESSIONS_KEY}");

    let mut candidates: Vec<_> = state
        .storage
        .list_settings()?
        .into_iter()
        .filter(|setting| {
            setting.key == SIDEBAR_ACTIVE_SESSIONS_KEY
                || setting.key == user_key
                || (setting.key.starts_with("user:") && setting.key.ends_with(&user_key_suffix))
        })
        .filter(|setting| !sidebar_active_sessions_empty(&setting.value))
        .collect();
    candidates.sort_by(|a, b| {
        (b.key == user_key)
            .cmp(&(a.key == user_key))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });

    let pinned_sessions =
        merge_sidebar_active_sessions(candidates.into_iter().map(|setting| setting.value));
    if !sidebar_active_sessions_empty(&pinned_sessions) {
        state
            .storage
            .set_setting(SIDEBAR_ACTIVE_SESSIONS_KEY, &pinned_sessions)?;
        return Ok(pinned_sessions);
    }

    Ok(serde_json::json!([]))
}

fn merge_sidebar_active_sessions(sources: impl IntoIterator<Item = Value>) -> Value {
    let mut merged = Vec::new();
    let mut seen = HashSet::new();
    let mut fallback = None;

    for source in sources {
        match source {
            Value::Array(items) => {
                for item in items {
                    if seen.insert(sidebar_active_session_key(&item)) {
                        merged.push(item);
                    }
                }
            }
            value if fallback.is_none() => fallback = Some(value),
            _ => {}
        }
    }

    if merged.is_empty() {
        fallback.unwrap_or_else(|| serde_json::json!([]))
    } else {
        Value::Array(merged)
    }
}

fn sidebar_active_session_key(value: &Value) -> String {
    let provider = value
        .get("provider")
        .or_else(|| value.get("cli"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let project_path = value
        .get("projectPath")
        .or_else(|| value.get("project_path"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let session_id = value
        .get("sessionId")
        .or_else(|| value.get("session_id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("");

    if !session_id.is_empty() {
        format!("{provider}\u{1f}{project_path}\u{1f}{session_id}")
    } else {
        value.to_string()
    }
}

fn sidebar_active_sessions_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.is_empty(),
        _ => false,
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct DirectAiSettingsQuery {
    #[serde(default, rename = "revealSecrets")]
    reveal_secrets: bool,
}

async fn get_direct_ai(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<DirectAiSettingsQuery>,
) -> Result<impl IntoResponse> {
    let mut status = io_gateway_settings_status(&state, &user.0.id)?;
    if query.reveal_secrets {
        let resolved = resolved_direct_ai_config_for_user(&state, &user.0.id);
        add_direct_ai_secrets(&mut status, &resolved);
    }
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(status)))
}

fn add_direct_ai_secrets(status: &mut Value, resolved: &Value) {
    if let Some(obj) = status.as_object_mut() {
        obj.insert(
            "secrets".to_string(),
            serde_json::json!({
                "gatewayApiKey": resolved
                    .get("gatewayApiKey")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                "gatewayOtpSecret": resolved
                    .get("gatewayOtpSecret")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            }),
        );
    }
}

async fn set_direct_ai(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(mut config): Json<Value>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "direct-ai");
    if let Some(stored) = state.storage.get_setting(&key)? {
        preserve_direct_ai_secrets(&stored, &mut config);
    }
    validate_direct_ai_config(&config)?;
    persist_io_gateway_secrets(&state, &user.0.id, &mut config)?;
    state.storage.set_setting(&key, &config)?;
    let status = io_gateway_settings_status(&state, &user.0.id)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "config": status.get("config").cloned().unwrap_or(Value::Null),
    })))
}

#[derive(Debug, Default, serde::Deserialize)]
struct ChatModelsQuery {
    #[serde(default)]
    provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GatewayChatModel {
    value: String,
    label: String,
}

/// Per-provider model catalog used by the chat UI's model picker. Native CLI
/// mode reads local/fallback models, while IO Gateway mode reads the
/// configured gateway catalog without failing the picker when unavailable.
async fn chat_provider_models(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<ChatModelsQuery>,
) -> Result<Json<Value>> {
    let provider = match query.provider.as_deref() {
        Some(name) => parse_provider_param(name)?,
        None => Provider::Codex,
    };
    let runtime = configured_chat_runtime(&state, &user.0.id);
    let mut models: Vec<String> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut gateway_labels = std::collections::BTreeMap::new();
    let mut gateway_available = true;

    if runtime == ChatRuntime::IoGateway && matches!(provider, Provider::Codex | Provider::Claude) {
        if let Some(gateway_models) = direct_ai_models_for_user(&state, &user.0.id, provider).await
        {
            for model in gateway_models {
                gateway_labels
                    .entry(model.value.trim().to_ascii_lowercase())
                    .or_insert(model.label);
                push_chat_model(&mut models, &mut seen, model.value);
            }
        } else {
            gateway_available = false;
        }
    } else if matches!(provider, Provider::Codex) {
        let mut base_models = Vec::new();
        if let Some(model) = configured_codex_model()
            .await
            .filter(|model| is_local_codex_cli_model(model))
        {
            base_models.push(model);
        }
        base_models.extend(
            cached_codex_models()
                .await
                .into_iter()
                .filter(|model| is_local_codex_cli_model(model)),
        );
        push_codex_chat_models(&mut models, &mut seen, base_models);
    } else {
        for model in fallback_models(provider) {
            push_chat_model(&mut models, &mut seen, model);
        }
    }

    let mut model_values: Vec<Value> = Vec::new();
    if runtime == ChatRuntime::NativeCli && matches!(provider, Provider::Codex | Provider::Claude) {
        model_values.push(serde_json::json!({
            "value": "",
            "label": "CLI default",
        }));
    }
    model_values.extend(models.into_iter().map(|value| {
        let label = gateway_labels
            .get(&value.to_ascii_lowercase())
            .cloned()
            .unwrap_or_else(|| value.clone());
        serde_json::json!({
            "value": value,
            "label": label,
        })
    }));

    Ok(Json(serde_json::json!({
        "success": true,
        "provider": provider.as_str(),
        "runtime": runtime,
        "gatewayAvailable": gateway_available,
        "models": model_values,
    })))
}

fn push_chat_model(
    models: &mut Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
    model: impl AsRef<str>,
) {
    let value = model.as_ref().trim();
    if value.is_empty() {
        return;
    }
    let key = value.to_ascii_lowercase();
    if seen.insert(key) {
        models.push(value.to_string());
    }
}

fn push_codex_chat_models(
    models: &mut Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
    base_models: Vec<String>,
) {
    for model in base_models {
        if is_local_codex_cli_model(&model) {
            push_chat_model(models, seen, model);
        }
    }
}

fn is_local_codex_cli_model(model: &str) -> bool {
    let trimmed = model.trim();
    looks_like_model_id(trimmed)
        && !trimmed.contains(':')
        && !trimmed.eq_ignore_ascii_case("gpt-5-codex")
}

fn is_io_gateway_model(model: &str) -> bool {
    let trimmed = model.trim();
    let Some((prefix, rest)) = trimmed.split_once(':') else {
        return false;
    };
    !rest.trim().is_empty()
        && looks_like_model_id(trimmed)
        && matches!(
            prefix.to_ascii_lowercase().as_str(),
            "agw"
                | "cod"
                | "proxy"
                | "gateway"
                | "aiproxy"
                | "cld"
                | "gem"
                | "cop"
                | "ctm"
                | "dsk"
                | "glm"
                | "grk"
                | "min"
        )
}

async fn cached_codex_models() -> Vec<String> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let path = home.join(".codex").join("models_cache.json");
    let Some(content) = read_text_path(&path).await else {
        return Vec::new();
    };
    let Ok(root) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    root.get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|model| model.get("visibility").and_then(Value::as_str) != Some("hide"))
        .filter_map(|model| model.get("slug").and_then(Value::as_str))
        .filter(|model| looks_like_model_id(model))
        .map(str::to_string)
        .collect()
}

async fn configured_codex_model() -> Option<String> {
    let path = home_dir()?.join(".codex").join("config.toml");
    let content = read_text_path(&path).await?;
    parse_top_level_toml_values(&content)
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| looks_like_model_id(model))
        .map(str::to_string)
}

/// Fetch the configured gateway's model list for a chat provider.
async fn direct_ai_models_for_user(
    state: &AppState,
    user_id: &str,
    provider: Provider,
) -> Option<Vec<GatewayChatModel>> {
    let config = chat_ai_config_for_user(state, user_id, provider);
    let raw = fetch_direct_ai_models(&config).await.unwrap_or_default();
    if raw.is_empty() {
        return None;
    }
    let ids: Vec<GatewayChatModel> = raw
        .into_iter()
        .filter_map(|model| {
            let value = model
                .get("value")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    model
                        .get("label")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })?;
            let label = model
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| value.clone());
            Some(GatewayChatModel { value, label })
        })
        .collect();
    if ids.is_empty() { None } else { Some(ids) }
}

fn direct_ai_runtime_config_for_user(
    state: &AppState,
    user_id: &str,
    provider: Provider,
) -> Option<DirectAiRuntimeConfig> {
    let config = chat_ai_config_for_user(state, user_id, provider);
    let (base_url, api_key) = direct_ai_endpoint_config(&config)?;
    let max_tokens = config
        .get("maxTokens")
        .or_else(|| config.get("max_tokens"))
        .and_then(Value::as_u64);
    Some(DirectAiRuntimeConfig {
        base_url,
        api_key,
        max_tokens,
    })
}

/// Resolve the gateway credential/config used for model discovery and gateway
/// CLI turns.
fn chat_ai_config_for_user(state: &AppState, user_id: &str, provider: Provider) -> Value {
    let mut config = resolved_direct_ai_config_for_user(state, user_id);
    if matches!(provider, Provider::Codex | Provider::Claude) {
        apply_io_gateway_config(&mut config, provider);
    }
    config
}

fn configured_chat_runtime(state: &AppState, user_id: &str) -> ChatRuntime {
    let key = user_setting_key(user_id, "direct-ai");
    let config = state.storage.get_setting(&key).ok().flatten();
    config
        .as_ref()
        .and_then(|config| {
            config
                .get("chatRuntime")
                .or_else(|| config.get("chat_runtime"))
        })
        .and_then(Value::as_str)
        .and_then(parse_chat_runtime)
        .unwrap_or_else(|| {
            let has_legacy_key = config
                .as_ref()
                .is_some_and(|config| direct_ai_secret_configured(config, "gatewayApiKey"))
                || state
                    .storage
                    .get_active_credential_value_by_name(
                        user_id,
                        IO_GATEWAY_API_KEY_CREDENTIAL,
                        IO_GATEWAY_API_KEY_CREDENTIAL_TYPE,
                    )
                    .ok()
                    .flatten()
                    .is_some();
            if has_legacy_key {
                ChatRuntime::IoGateway
            } else {
                ChatRuntime::NativeCli
            }
        })
}

fn parse_chat_runtime(value: &str) -> Option<ChatRuntime> {
    match value.trim().to_ascii_lowercase().as_str() {
        "native_cli" | "native" | "cli" | "default" => Some(ChatRuntime::NativeCli),
        "io_gateway" | "gateway" | "custom_api" | "aiproxy" => Some(ChatRuntime::IoGateway),
        _ => None,
    }
}

fn io_gateway_settings_status(state: &AppState, user_id: &str) -> Result<Value> {
    let key = user_setting_key(user_id, "direct-ai");
    let config = state
        .storage
        .get_setting(&key)?
        .unwrap_or_else(default_direct_ai_config);
    let mut effective = resolved_direct_ai_config_for_user(state, user_id);
    apply_io_gateway_config(&mut effective, Provider::Claude);

    let api_key_configured = direct_ai_secret_configured(&effective, "gatewayApiKey");
    let otp_configured = direct_ai_secret_configured(&effective, "gatewayOtpSecret");
    let auth = state.auth.status(None)?;

    let mut public_config = public_direct_ai_config(&config);
    if let Some(obj) = public_config.as_object_mut() {
        obj.insert(
            "gatewayApiKeyConfigured".to_string(),
            Value::Bool(api_key_configured),
        );
        obj.insert(
            "gatewayOtpConfigured".to_string(),
            Value::Bool(otp_configured),
        );
    }

    Ok(serde_json::json!({
        "success": true,
        "config": public_config,
        "effective": {
            "chatRuntime": configured_chat_runtime(state, user_id),
            "mode": effective.get("mode").cloned().unwrap_or(Value::Null),
            "baseUrl": effective.get("baseUrl").cloned().unwrap_or(Value::Null),
            "apiKeyEnv": effective.get("apiKeyEnv").cloned().unwrap_or(Value::Null),
            "model": effective.get("model").cloned().unwrap_or(Value::Null),
        },
        "runtimeReady": direct_ai_endpoint_config(&effective).is_some(),
        "apiKeyConfigured": api_key_configured,
        "gatewayOtpConfigured": otp_configured,
        "auth": {
            "mode": auth.auth_mode,
            "otpConfigured": state.config.otp_secret.is_some(),
            "tokenConfigured": state.config.local_token.is_some(),
        },
    }))
}

fn apply_io_gateway_config(config: &mut Value, provider: Provider) {
    if !config.is_object() {
        *config = default_direct_ai_config();
    }

    let Some(obj) = config.as_object_mut() else {
        return;
    };
    obj.insert("mode".to_string(), Value::String("aiproxy".to_string()));
    obj.remove("base_url");
    let endpoint_key = if provider == Provider::Codex {
        "codexEndpoint"
    } else {
        "claudeEndpoint"
    };
    let default_endpoint = if provider == Provider::Codex {
        "codex"
    } else {
        "claude"
    };
    let endpoint = obj
        .get(endpoint_key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_endpoint);
    let gateway_root = obj
        .get("gatewayUrl")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .or_else(|| {
            obj.get("baseUrl")
                .and_then(Value::as_str)
                .and_then(|value| {
                    let value = value.trim().trim_end_matches('/');
                    if io_gateway_url_has_endpoint_path(value, endpoint) {
                        Some(value.to_string())
                    } else {
                        url_origin(value)
                    }
                })
        })
        .unwrap_or_else(|| {
            url_origin(DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL)
                .unwrap_or_else(|| DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL.to_string())
        });
    let base_url = join_io_gateway_endpoint_url(&gateway_root, endpoint);
    obj.insert("baseUrl".to_string(), Value::String(base_url));
    obj.remove("api_key_env");
    obj.remove("apiKeyEnv");
}

pub(crate) fn join_io_gateway_endpoint_url(gateway_root: &str, endpoint: &str) -> String {
    let gateway_root = gateway_root.trim().trim_end_matches('/');
    let endpoint = endpoint.trim();
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return endpoint.trim_end_matches('/').to_string();
    }
    let endpoint = endpoint.trim_matches('/');
    if endpoint.is_empty() || io_gateway_url_has_endpoint_path(gateway_root, endpoint) {
        gateway_root.to_string()
    } else {
        format!("{gateway_root}/{endpoint}")
    }
}

fn io_gateway_url_has_endpoint_path(url: &str, endpoint: &str) -> bool {
    let without_fragment = url.split_once('#').map_or(url, |(value, _)| value);
    let without_suffix = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(value, _)| value);
    let path = without_suffix
        .split_once("://")
        .map_or(without_suffix, |(_, remainder)| {
            remainder
                .find('/')
                .map_or("", |path_start| &remainder[path_start + 1..])
        });
    let path_segments: Vec<_> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let endpoint_segments: Vec<_> = endpoint
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    !endpoint_segments.is_empty() && path_segments.ends_with(&endpoint_segments)
}

fn direct_ai_secret_configured(config: &Value, key: &str) -> bool {
    config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

fn preserve_direct_ai_secrets(stored: &Value, config: &mut Value) {
    let (Some(stored), Some(config)) = (stored.as_object(), config.as_object_mut()) else {
        return;
    };
    for key in ["gatewayApiKey", "gatewayOtpSecret"] {
        if !config.contains_key(key) {
            if let Some(value) = stored.get(key) {
                config.insert(key.to_string(), value.clone());
            }
        }
    }
}

fn persist_io_gateway_secrets(state: &AppState, user_id: &str, config: &mut Value) -> Result<()> {
    let Some(obj) = config.as_object_mut() else {
        return Ok(());
    };
    if let Some(secret) = obj
        .remove("gatewayApiKey")
        .and_then(|value| value.as_str().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty())
    {
        state.storage.upsert_named_credential(
            user_id,
            IO_GATEWAY_API_KEY_CREDENTIAL,
            IO_GATEWAY_API_KEY_CREDENTIAL_TYPE,
            &secret,
            Some("IO Gateway API key"),
        )?;
    }
    if let Some(secret) = obj
        .remove("gatewayOtpSecret")
        .and_then(|value| value.as_str().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty())
    {
        state.storage.upsert_named_credential(
            user_id,
            IO_GATEWAY_OTP_CREDENTIAL,
            IO_GATEWAY_OTP_CREDENTIAL_TYPE,
            &secret,
            Some("IO Gateway TOTP secret"),
        )?;
    }
    Ok(())
}

fn public_direct_ai_config(config: &Value) -> Value {
    let mut public = config.clone();
    let api_key_configured = direct_ai_secret_configured(config, "gatewayApiKey");
    let otp_configured = direct_ai_secret_configured(config, "gatewayOtpSecret");
    if let Some(obj) = public.as_object_mut() {
        obj.remove("gatewayApiKey");
        obj.remove("gatewayOtpSecret");
        obj.insert(
            "gatewayApiKeyConfigured".to_string(),
            Value::Bool(api_key_configured),
        );
        obj.insert(
            "gatewayOtpConfigured".to_string(),
            Value::Bool(otp_configured),
        );
    }
    public
}

fn resolved_direct_ai_config_for_user(state: &AppState, user_id: &str) -> Value {
    let key = user_setting_key(user_id, "direct-ai");
    let mut config = state
        .storage
        .get_setting(&key)
        .ok()
        .flatten()
        .unwrap_or_else(default_direct_ai_config);
    if direct_ai_secret_configured(&config, "gatewayApiKey")
        || direct_ai_secret_configured(&config, "gatewayOtpSecret")
    {
        let mut sanitized = config.clone();
        if sanitized
            .get("chatRuntime")
            .or_else(|| sanitized.get("chat_runtime"))
            .is_none()
            && direct_ai_secret_configured(&sanitized, "gatewayApiKey")
            && let Some(obj) = sanitized.as_object_mut()
        {
            obj.insert(
                "chatRuntime".to_string(),
                Value::String("io_gateway".to_string()),
            );
        }
        if persist_io_gateway_secrets(state, user_id, &mut sanitized).is_ok() {
            let _ = state.storage.set_setting(&key, &sanitized);
            config = sanitized;
        }
    }
    if let Some(obj) = config.as_object_mut() {
        if let Ok(Some(secret)) = state.storage.get_active_credential_value_by_name(
            user_id,
            IO_GATEWAY_API_KEY_CREDENTIAL,
            IO_GATEWAY_API_KEY_CREDENTIAL_TYPE,
        ) {
            obj.insert("gatewayApiKey".to_string(), Value::String(secret));
        }
        if let Ok(Some(secret)) = state.storage.get_active_credential_value_by_name(
            user_id,
            IO_GATEWAY_OTP_CREDENTIAL,
            IO_GATEWAY_OTP_CREDENTIAL_TYPE,
        ) {
            obj.insert("gatewayOtpSecret".to_string(), Value::String(secret));
        }
    }
    config
}

/// Heuristic check that a token is a plausible model identifier. Model ids
/// from the supported providers are short slugs (e.g. `gpt-5-codex`,
/// `claude-opus-4-1`, `gemini-2.5-pro`) that contain letters or digits plus
/// `-`, `.`, `_`, `:`, or `/`. Real identifiers are at least 3 characters
/// and always contain either a digit or one of the separator characters
/// (otherwise prose tokens like `A` or `If` from interactive prompts would
/// be misclassified as model ids).
fn looks_like_model_id(token: &str) -> bool {
    if token.len() < 3 || token.len() > 80 {
        return false;
    }
    if token.chars().any(char::is_whitespace) {
        return false;
    }
    let mut has_alnum = false;
    let mut has_separator = false;
    for ch in token.chars() {
        if ch.is_alphanumeric() {
            has_alnum = true;
            continue;
        }
        if matches!(ch, '-' | '.' | '_' | ':' | '/') {
            has_separator = true;
            continue;
        }
        return false;
    }
    has_alnum && (has_separator || token.chars().any(|c| c.is_ascii_digit()))
}

fn fallback_models(provider: Provider) -> Vec<String> {
    match provider {
        Provider::Codex => vec![
            "gpt-5".to_string(),
            "gpt-5-codex".to_string(),
            "gpt-5-mini".to_string(),
            "gpt-5-nano".to_string(),
            "gpt-4.1".to_string(),
            "gpt-4.1-mini".to_string(),
            "gpt-4o".to_string(),
            "o4-mini".to_string(),
        ],
        Provider::Claude => vec![
            "sonnet".to_string(),
            "opus".to_string(),
            "haiku".to_string(),
            "fable".to_string(),
            "claude-sonnet-4-5".to_string(),
            "claude-sonnet-4".to_string(),
            "claude-opus-4".to_string(),
            "claude-3-7-sonnet-latest".to_string(),
            "claude-3-5-sonnet-latest".to_string(),
            "claude-3-5-haiku-latest".to_string(),
        ],
        Provider::Gemini => vec![
            "gemini-2.5-pro".to_string(),
            "gemini-2.5-flash".to_string(),
            "gemini-2.0-flash".to_string(),
            "gemini-1.5-pro-latest".to_string(),
            "gemini-1.5-flash-latest".to_string(),
        ],
    }
}

async fn direct_ai_models(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Json<Value> {
    let mut config = resolved_direct_ai_config_for_user(&state, &user.0.id);
    apply_io_gateway_config(&mut config, Provider::Claude);
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
    if provider == "cursor" {
        return Ok(Json(cursor_cli_status().await));
    }
    let provider = parse_provider_param(&provider)?;
    Ok(Json(provider_cli_status(provider).await))
}

async fn cli_overview() -> Json<Value> {
    let mut providers = serde_json::Map::new();
    for provider in [Provider::Claude, Provider::Codex, Provider::Gemini] {
        providers.insert(
            provider.as_str().to_string(),
            provider_cli_status(provider).await,
        );
    }
    providers.insert("cursor".to_string(), cursor_cli_status().await);
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
        .max(1)
        .min(SESSION_HISTORY_MAX_MESSAGES);
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
        .max(1)
        .min(SESSION_PROMPT_HISTORY_MAX);
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
        timestamp: prompt.timestamp.clone(),
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
        .max(1)
        .min(SESSION_HISTORY_MAX_MESSAGES);
    let session_before = state.sessions.get(&session_id).await?;
    let (mut messages, mut total_count) = state
        .sessions
        .messages_tail_including_external(&session_id, limit)
        .await?;
    let session = state.sessions.get(&session_id).await?;
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
    Ok(Json(SessionSnapshotResponse {
        session,
        has_more: messages.len() < total_count,
        messages: bound_session_messages_for_response(messages),
        total_count,
    }))
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
    let response = state
        .fork_session_before_message(&user.0.id, &session_id, before_message_id, request_id)
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

    let native_session_id = session
        .native_session_id
        .as_deref()
        .unwrap_or(session.id.as_str());
    let snapshot = match provider {
        Provider::Codex => codex_token_usage(native_session_id).await?,
        Provider::Claude => claude_token_usage(&session.project_path, native_session_id).await?,
        Provider::Gemini => unreachable!(),
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
    let user = match state.auth.require_user(token.as_deref()) {
        Ok(user) => user,
        Err(error) => return ServerError::from(error).into_response(),
    };

    ws.on_upgrade(move |socket| handle_socket(socket, state, user))
        .into_response()
}

async fn handle_socket(socket: WebSocket, state: AppState, user: iowb_protocol::UserProfile) {
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
    let _ = direct_tx
        .send(WsServerEvent::ActiveSessions {
            sessions: state.sessions.list_active().await,
        })
        .await;

    loop {
        tokio::select! {
            Some(command) = command_rx.recv() => {
                handle_ws_command(&state, &direct_tx, &user, command).await;
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
    user: &iowb_protocol::UserProfile,
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
            effort,
            mode,
            thinking,
            fast,
        } => {
            let runtime = resolve_session_chat_runtime(
                state,
                &user.id,
                provider,
                session_id.as_deref(),
                model.as_deref(),
            );
            let (model, effort, mode, thinking) = if runtime == ChatRuntime::NativeCli
                && model.as_deref().is_none_or(|model| model.trim().is_empty())
            {
                (None, None, None, None)
            } else {
                (model, effort, mode, thinking)
            };
            let direct_ai_config = (runtime == ChatRuntime::IoGateway)
                .then(|| direct_ai_runtime_config_for_user(state, &user.id, provider))
                .flatten();
            if let Err(error) = state
                .start_agent_session(
                    provider,
                    project_path,
                    prompt,
                    session_id,
                    model,
                    effort,
                    mode,
                    thinking,
                    fast,
                    runtime,
                    direct_ai_config,
                    Some(user.id.clone()),
                )
                .await
            {
                let _ = direct_tx
                    .send(WsServerEvent::Error {
                        message: "failed to start session".to_string(),
                        details: Some(error.to_string()),
                    })
                    .await;
            }
        }
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

fn resolve_session_chat_runtime(
    state: &AppState,
    user_id: &str,
    provider: Provider,
    session_id: Option<&str>,
    model: Option<&str>,
) -> ChatRuntime {
    if provider == Provider::Gemini {
        return ChatRuntime::NativeCli;
    }
    if let Some(session) =
        session_id.and_then(|session_id| state.storage.get_session(session_id).ok().flatten())
    {
        return session.runtime.unwrap_or_else(|| {
            if model
                .or(session.model.as_deref())
                .is_some_and(is_io_gateway_model)
            {
                ChatRuntime::IoGateway
            } else {
                ChatRuntime::NativeCli
            }
        });
    }
    configured_chat_runtime(state, user_id)
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

#[derive(Debug, Clone)]
struct FcmConfig {
    project_id: String,
    service_account_json: String,
}

#[derive(Debug, Deserialize)]
struct FirebaseServiceAccount {
    #[serde(default)]
    project_id: String,
    client_email: String,
    private_key: String,
    #[serde(default)]
    token_uri: Option<String>,
}

#[derive(Debug, Serialize)]
struct FcmJwtClaims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    iat: i64,
    exp: i64,
}

#[derive(Debug, Deserialize)]
struct FcmAccessTokenResponse {
    access_token: String,
}

fn fcm_config_from_env() -> Option<FcmConfig> {
    if !env_bool_local("IO_WORKBENCH_FCM_ENABLED", false) {
        return None;
    }
    let service_account_json = env::var("IO_WORKBENCH_FCM_SERVICE_ACCOUNT_JSON")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("IO_WORKBENCH_FCM_SERVICE_ACCOUNT_PATH")
                .ok()
                .or_else(|| env::var("GOOGLE_APPLICATION_CREDENTIALS").ok())
                .and_then(|path| std::fs::read_to_string(path).ok())
        })?;
    let service_account =
        serde_json::from_str::<FirebaseServiceAccount>(&service_account_json).ok()?;
    let project_id = env::var("IO_WORKBENCH_FCM_PROJECT_ID")
        .ok()
        .or_else(|| env::var("FIREBASE_PROJECT_ID").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(service_account.project_id)
        .trim()
        .to_string();
    if project_id.is_empty() {
        return None;
    }
    Some(FcmConfig {
        project_id,
        service_account_json,
    })
}

fn env_bool_local(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

async fn fcm_access_token(
    client: &reqwest::Client,
    service_account: &FirebaseServiceAccount,
) -> anyhow::Result<String> {
    let token_uri = service_account
        .token_uri
        .as_deref()
        .unwrap_or("https://oauth2.googleapis.com/token");
    let now = Utc::now().timestamp();
    let claims = FcmJwtClaims {
        iss: &service_account.client_email,
        scope: "https://www.googleapis.com/auth/firebase.messaging",
        aud: token_uri,
        iat: now,
        exp: now + 3600,
    };
    let assertion = jsonwebtoken::encode(
        &Header::new(Algorithm::RS256),
        &claims,
        &EncodingKey::from_rsa_pem(service_account.private_key.as_bytes())?,
    )?;
    let response = client
        .post(token_uri)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion.as_str()),
        ])
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("FCM OAuth token request failed: {status} {text}");
    }
    let token = serde_json::from_str::<FcmAccessTokenResponse>(&text)?;
    Ok(token.access_token)
}

async fn send_fcm_notification_to_token(
    client: &reqwest::Client,
    config: &FcmConfig,
    access_token: &str,
    token: &str,
    title: &str,
    body: &str,
    data: Value,
) -> anyhow::Result<()> {
    let url = format!(
        "https://fcm.googleapis.com/v1/projects/{}/messages:send",
        config.project_id
    );
    let response = client
        .post(url)
        .bearer_auth(access_token)
        .json(&serde_json::json!({
            "message": {
                "token": token,
                "notification": {
                    "title": title,
                    "body": body,
                },
                "data": data,
                "android": {
                    "priority": "HIGH",
                    "notification": {
                        "channel_id": "io_workbench_activity",
                        "default_sound": true,
                        "default_vibrate_timings": true,
                    }
                }
            }
        }))
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("FCM send failed: {status} {text}");
    }
    Ok(())
}

fn spawn_fcm_notification_bridge(state: AppState) {
    let Some(config) = fcm_config_from_env() else {
        info!("FCM push notifications are disabled");
        return;
    };
    let mut hub_rx = state.ws_hub.subscribe();
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                warn!(error = %error, "failed to create FCM HTTP client");
                return;
            }
        };
        loop {
            match hub_rx.recv().await {
                Ok(WsServerEvent::SessionStatus {
                    provider,
                    session_id,
                    status: iowb_protocol::SessionRuntimeStatus::Completed,
                    latest_user_prompt,
                    ..
                }) => {
                    if let Err(error) = send_fcm_chat_completed(
                        &state,
                        &client,
                        &config,
                        provider,
                        &session_id,
                        latest_user_prompt.as_deref(),
                    )
                    .await
                    {
                        warn!(error = %error, session_id = %session_id, "failed to send FCM chat completion notification");
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "FCM notification bridge lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

async fn send_fcm_chat_completed(
    state: &AppState,
    client: &reqwest::Client,
    config: &FcmConfig,
    provider: Provider,
    session_id: &str,
    latest_user_prompt: Option<&str>,
) -> anyhow::Result<()> {
    let session = state.sessions.get(session_id).await.ok();
    let run = state
        .storage
        .latest_durable_chat_run_for_session(session_id)?;
    let user_id = run.as_ref().and_then(|run| run.user_id.as_deref());
    let tokens = if let Some(user_id) = user_id {
        state.storage.list_fcm_tokens_for_user(user_id)?
    } else {
        state.storage.list_all_fcm_tokens()?
    };
    if tokens.is_empty() {
        return Ok(());
    }

    let service_account =
        serde_json::from_str::<FirebaseServiceAccount>(&config.service_account_json)?;
    let access_token = fcm_access_token(client, &service_account).await?;
    let project_folder = session
        .as_ref()
        .and_then(|session| Path::new(&session.project_path).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("io-workbench");
    let prompt = latest_user_prompt
        .filter(|prompt| !prompt.trim().is_empty())
        .map(str::trim)
        .or_else(|| {
            run.as_ref()
                .map(|run| run.prompt.trim())
                .filter(|prompt| !prompt.is_empty())
        })
        .unwrap_or("latest prompt");
    let title = format!("{project_folder} | {}", provider.as_str());
    let body = format!("finished: {}", truncate_notification_text(prompt, 180));
    let data = serde_json::json!({
        "event": "chat_completed",
        "sessionId": session_id,
        "provider": provider.as_str(),
    });
    for stored in tokens {
        if let Err(error) = send_fcm_notification_to_token(
            client,
            config,
            &access_token,
            &stored.token,
            &title,
            &body,
            data.clone(),
        )
        .await
        {
            warn!(
                error = %error,
                user_id = %stored.user_id,
                platform = ?stored.platform,
                "failed to send FCM notification to token"
            );
        }
    }
    Ok(())
}

fn truncate_notification_text(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= max_chars {
            output.push('…');
            return output;
        }
        output.push(ch);
    }
    output
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
    let (tx, mut rx) = mpsc::channel::<(String, notify::Result<notify::Event>)>(256);
    let debounce = Duration::from_millis(state.watch.debounce_ms());

    tokio::spawn(async move {
        let mut watchers: HashMap<String, RecommendedWatcher> = HashMap::new();
        let mut failed_watchers = HashMap::<String, Instant>::new();
        let mut discovery = tokio::time::interval(Duration::from_secs(5));
        discovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut pending_updates = HashMap::<String, HashSet<String>>::new();

        loop {
            tokio::select! {
                _ = discovery.tick() => {
                    refresh_project_watchers(
                        &state,
                        &tx,
                        &mut watchers,
                        &mut failed_watchers,
                    ).await;
                }
                Some((project_path, event)) = rx.recv() => {
                    match event {
                        Ok(event) => {
                            let paths = interesting_project_watch_paths(&project_path, &event);
                            if !paths.is_empty() {
                                pending_updates
                                    .entry(project_path)
                                    .or_default()
                                    .extend(paths);
                            }
                        }
                        Err(error) => warn!(error = %error, "project watcher error"),
                    }
                }
                _ = tokio::time::sleep(debounce), if !pending_updates.is_empty() => {
                    for (project_path, changed_paths) in std::mem::take(&mut pending_updates) {
                        let mut paths = changed_paths.into_iter().collect::<Vec<_>>();
                        paths.sort();
                        state.ws_hub.publish(WsServerEvent::ProjectFilesChanged {
                            project_path,
                            paths,
                        });
                    }
                }
                else => break,
            }
        }
    });
}

async fn refresh_project_watchers(
    state: &AppState,
    tx: &mpsc::Sender<(String, notify::Result<notify::Event>)>,
    watchers: &mut HashMap<String, RecommendedWatcher>,
    failed_watchers: &mut HashMap<String, Instant>,
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
    failed_watchers.retain(|path, _| desired.contains(path));

    for project in projects {
        if watchers.contains_key(&project.path) {
            continue;
        }
        if failed_watchers
            .get(&project.path)
            .is_some_and(|retry_at| *retry_at > Instant::now())
        {
            continue;
        }
        let project_path = PathBuf::from(&project.path);
        if !project_path.is_dir() {
            continue;
        }
        let broad_ancestor = state.config.workspace_root.starts_with(&project_path)
            && state.config.workspace_root != project_path;
        let requested_mode = if broad_ancestor {
            RecursiveMode::NonRecursive
        } else {
            RecursiveMode::Recursive
        };

        let event_tx = tx.clone();
        let event_project_path = project.path.clone();
        match notify::recommended_watcher(move |event| {
            // Watch registration can synchronously emit hundreds of events.
            // Never block the notify thread waiting for this task to finish
            // registering the watcher, otherwise a broad project can deadlock
            // the entire server runtime.
            let _ = event_tx.try_send((event_project_path.clone(), event));
        }) {
            Ok(mut watcher) => {
                if broad_ancestor {
                    warn!(
                        project_path = %project_path.display(),
                        workspace_root = %state.config.workspace_root.display(),
                        "project is an ancestor of the workspace root; using root-only watch"
                    );
                }
                if let Err(recursive_error) = watcher.watch(&project_path, requested_mode) {
                    match watcher.watch(&project_path, RecursiveMode::NonRecursive) {
                        Ok(()) => {
                            warn!(
                                project_path = %project_path.display(),
                                error = %recursive_error,
                                "recursive project watch unavailable; using root-only watch"
                            );
                        }
                        Err(fallback_error) => {
                            warn!(
                                project_path = %project_path.display(),
                                recursive_error = %recursive_error,
                                fallback_error = %fallback_error,
                                "failed to watch project"
                            );
                            failed_watchers
                                .insert(project.path, Instant::now() + Duration::from_secs(5 * 60));
                            continue;
                        }
                    }
                }
                failed_watchers.remove(&project.path);
                watchers.insert(project.path, watcher);
            }
            Err(error) => {
                warn!(
                    project_path = %project_path.display(),
                    error = %error,
                    "failed to create project watcher"
                );
                failed_watchers.insert(project.path, Instant::now() + Duration::from_secs(5 * 60));
            }
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

fn interesting_project_watch_paths(project_path: &str, event: &notify::Event) -> Vec<String> {
    if !is_interesting_watch_event(event) {
        return Vec::new();
    }
    event
        .paths
        .iter()
        .filter_map(|path| project_relative_watch_path(project_path, path))
        .collect()
}

fn project_relative_watch_path(project_path: &str, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(project_path).ok()?;
    if relative
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == ".git")
    {
        return None;
    }
    let normalized = relative.to_string_lossy().replace('\\', "/");
    Some(if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized
    })
}

async fn publish_projects(state: &AppState) {
    match state.projects.list(&state.sessions).await {
        Ok(mut projects) => {
            populate_repository_names(&mut projects).await;
            state
                .ws_hub
                .publish(WsServerEvent::ProjectsUpdated { projects });
        }
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

fn is_io_gateway_setting_key(key: &str) -> bool {
    key == "direct-ai" || key.ends_with(":direct-ai")
}

fn public_settings(settings: Vec<iowb_protocol::SettingEntry>) -> Vec<iowb_protocol::SettingEntry> {
    settings
        .into_iter()
        .map(|mut setting| {
            if is_io_gateway_setting_key(&setting.key) {
                setting.value = public_direct_ai_config(&setting.value);
            }
            setting
        })
        .collect()
}

fn current_git_config_overview(state: &AppState, user_id: &str) -> Result<Value> {
    let stored = state
        .storage
        .get_setting(&user_setting_key(user_id, "git-config"))?;
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
    let source = if stored.is_some() {
        "server-setting"
    } else if git_name.is_some() || git_email.is_some() {
        "git-global"
    } else {
        "unset"
    };

    Ok(serde_json::json!({
        "gitName": git_name,
        "gitEmail": git_email,
        "source": source,
    }))
}

fn default_claude_agent_settings() -> Value {
    serde_json::json!({
        "allowedTools": [],
        "disallowedTools": [],
        "skipPermissions": false,
        "providerMode": "anthropic",
        "aiProxyBaseUrl": "",
        "aiProxyApiKeyEnv": "",
        "minimaxBaseUrl": "https://api.minimax.io/anthropic",
        "minimaxApiKeyEnv": "MINIMAX_API_KEY",
        "minimaxModel": "MiniMax-M3"
    })
}

fn default_cursor_agent_settings() -> Value {
    serde_json::json!({
        "allowedCommands": [],
        "disallowedCommands": [],
        "skipPermissions": false
    })
}

fn default_codex_agent_settings() -> Value {
    serde_json::json!({
        "permissionMode": "default"
    })
}

fn default_gemini_agent_settings() -> Value {
    serde_json::json!({
        "permissionMode": "default"
    })
}

fn default_appearance_settings() -> Value {
    serde_json::json!({
        "projectSortOrder": "name",
        "codeEditor": {
            "theme": "dark",
            "wordWrap": false,
            "showMinimap": true,
            "lineNumbers": true,
            "fontSize": "14"
        }
    })
}

fn default_tasks_settings() -> Value {
    serde_json::json!({
        "enabled": true,
        "runEndpoint": "/api/taskmaster/run",
        "commandsEndpoint": "/api/commands/run"
    })
}

fn default_notification_preferences() -> Value {
    serde_json::json!({
        "channels": {
            "inApp": true,
            "fcm": true,
            "webPush": false,
            "telegram": false,
            "googleChat": false
        },
        "telegram": {
            "botToken": "",
            "chatId": ""
        },
        "googleChat": {
            "webhookUrl": ""
        },
        "events": {
            "actionRequired": true,
            "stop": true,
            "error": true,
            "agenticRunStarted": true,
            "agenticTaskUpdated": false,
            "agenticRunCompleted": true,
            "agenticRunNeedsAttention": true
        }
    })
}

fn default_direct_ai_config() -> Value {
    serde_json::json!({
        "mode": "off",
        "chatRuntime": "native_cli",
        "baseUrl": null,
        "apiKeyEnv": null,
        "model": null
    })
}

fn default_cursor_compat() -> Value {
    serde_json::json!({
        "success": true,
        "servers": [],
        "isDefault": true
    })
}

async fn read_json_file(path: &Path) -> Option<Value> {
    let content = read_text_path(path).await?;
    serde_json::from_str::<Value>(&content).ok()
}

async fn cursor_config_overview() -> Value {
    let Some(home) = home_dir() else {
        return serde_json::json!({
            "success": false,
            "error": "home directory not found",
            "config": default_cursor_config(),
            "isDefault": true
        });
    };
    let config_path = home.join(".cursor").join("cli-config.json");
    if let Some(config) = read_json_file(&config_path).await {
        return serde_json::json!({
            "success": true,
            "config": config,
            "path": config_path.display().to_string(),
            "isDefault": false
        });
    }
    serde_json::json!({
        "success": true,
        "config": default_cursor_config(),
        "path": config_path.display().to_string(),
        "isDefault": true
    })
}

fn default_cursor_config() -> Value {
    serde_json::json!({
        "version": 1,
        "model": {
            "modelId": "gpt-5",
            "displayName": "GPT-5"
        },
        "permissions": {
            "allow": [],
            "deny": []
        }
    })
}

async fn cursor_mcp_config_overview() -> Value {
    let Some(home) = home_dir() else {
        return serde_json::json!({
            "success": false,
            "error": "home directory not found",
            "servers": []
        });
    };
    let mcp_path = home.join(".cursor").join("mcp.json");
    let Some(config) = read_json_file(&mcp_path).await else {
        return serde_json::json!({
            "success": true,
            "servers": [],
            "path": mcp_path.display().to_string(),
            "isDefault": true
        });
    };
    let servers = config
        .get("mcpServers")
        .and_then(Value::as_object)
        .map(|servers| mcp_servers_from_object(servers, "cursor", None))
        .unwrap_or_default();
    serde_json::json!({
        "success": true,
        "servers": servers,
        "path": mcp_path.display().to_string(),
        "raw": config,
        "isDefault": false
    })
}

async fn claude_mcp_config_overview(workspace_root: &Path) -> Value {
    let Some(home) = home_dir() else {
        return serde_json::json!({
            "success": false,
            "message": "home directory not found",
            "servers": []
        });
    };
    let config_paths = [
        home.join(".claude.json"),
        home.join(".claude").join("settings.json"),
    ];
    let mut config_data = None;
    let mut config_path = None;
    for path in config_paths {
        if let Some(value) = read_json_file(&path).await {
            config_data = Some(value);
            config_path = Some(path);
            break;
        }
    }
    let Some(config) = config_data else {
        return serde_json::json!({
            "success": false,
            "message": "No Claude configuration file found",
            "servers": []
        });
    };

    let mut servers = Vec::new();
    if let Some(root_servers) = config.get("mcpServers").and_then(Value::as_object) {
        servers.extend(mcp_servers_from_object(root_servers, "user", None));
    }
    let workspace_key = workspace_root.display().to_string();
    if let Some(project_servers) = config
        .get("projects")
        .and_then(Value::as_object)
        .and_then(|projects| projects.get(&workspace_key))
        .and_then(|project| project.get("mcpServers"))
        .and_then(Value::as_object)
    {
        servers.extend(mcp_servers_from_object(
            project_servers,
            "local",
            Some(workspace_key),
        ));
    }

    serde_json::json!({
        "success": true,
        "configPath": config_path.map(|path| path.display().to_string()),
        "servers": servers
    })
}

fn mcp_servers_from_object(
    servers: &serde_json::Map<String, Value>,
    scope: &str,
    project_path: Option<String>,
) -> Vec<Value> {
    servers
        .iter()
        .map(|(name, config)| mcp_server_record(name, config, scope, project_path.clone()))
        .collect()
}

fn mcp_server_record(
    name: &str,
    config: &Value,
    scope: &str,
    project_path: Option<String>,
) -> Value {
    let server_type = if config.get("command").is_some() {
        "stdio".to_string()
    } else {
        config
            .get("transport")
            .and_then(Value::as_str)
            .unwrap_or("http")
            .to_string()
    };
    let config_details = if server_type == "stdio" {
        serde_json::json!({
            "command": config.get("command").and_then(Value::as_str).unwrap_or_default(),
            "args": config.get("args").cloned().unwrap_or_else(|| serde_json::json!([])),
            "env": config.get("env").cloned().unwrap_or_else(|| serde_json::json!({})),
        })
    } else {
        serde_json::json!({
            "url": config.get("url").and_then(Value::as_str).unwrap_or_default(),
            "headers": config.get("headers").cloned().unwrap_or_else(|| serde_json::json!({})),
        })
    };
    serde_json::json!({
        "id": if scope == "local" { format!("local:{name}") } else { name.to_string() },
        "name": name,
        "type": server_type,
        "scope": scope,
        "projectPath": project_path,
        "config": config_details,
        "raw": config,
    })
}

async fn codex_config_overview() -> Value {
    let Some(home) = home_dir() else {
        return default_codex_config_overview(Value::Null);
    };
    let config_path = home.join(".codex").join("config.toml");
    let Some(content) = read_text_path(&config_path).await else {
        return default_codex_config_overview(Value::String(config_path.display().to_string()));
    };
    let top_values = parse_top_level_toml_values(&content);
    let model = top_values.get("model").cloned().unwrap_or(Value::Null);
    let reasoning_effort = top_values
        .get("model_reasoning_effort")
        .cloned()
        .unwrap_or(Value::Null);
    let approval_mode = top_values
        .get("approval_mode")
        .cloned()
        .unwrap_or_else(|| Value::String("suggest".to_string()));
    let profile_name = env::var("CODEX_PROFILE").unwrap_or_else(|_| "default".to_string());

    serde_json::json!({
        "success": true,
        "configPath": config_path.display().to_string(),
        "config": {
            "model": model,
            "profileModel": Value::Null,
            "resolvedModel": top_values.get("model").cloned().unwrap_or(Value::Null),
            "activeProfile": profile_name,
            "profiles": parse_toml_section_names(&content, "profiles"),
            "mcpServers": codex_mcp_servers_map(&content),
            "approvalMode": approval_mode,
            "modelReasoningEffort": reasoning_effort,
        }
    })
}

fn default_codex_config_overview(config_path: Value) -> Value {
    serde_json::json!({
        "success": true,
        "configPath": config_path,
        "config": {
            "model": Value::Null,
            "profileModel": Value::Null,
            "resolvedModel": Value::Null,
            "activeProfile": "default",
            "profiles": [],
            "mcpServers": {},
            "approvalMode": "suggest"
        }
    })
}

async fn codex_mcp_config_overview() -> Value {
    let Some(home) = home_dir() else {
        return serde_json::json!({
            "success": false,
            "error": "home directory not found",
            "servers": []
        });
    };
    let config_path = home.join(".codex").join("config.toml");
    let Some(content) = read_text_path(&config_path).await else {
        return serde_json::json!({
            "success": true,
            "configPath": config_path.display().to_string(),
            "servers": []
        });
    };
    serde_json::json!({
        "success": true,
        "configPath": config_path.display().to_string(),
        "servers": parse_codex_mcp_servers(&content)
    })
}

fn parse_top_level_toml_values(content: &str) -> serde_json::Map<String, Value> {
    let mut values = serde_json::Map::new();
    for line in content.lines().map(str::trim) {
        if line.starts_with('[') {
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if let Some(parsed) = parse_simple_toml_value(value.trim()) {
            values.insert(key.trim().to_string(), parsed);
        }
    }
    values
}

fn parse_toml_section_names(content: &str, prefix: &str) -> Vec<Value> {
    let section_prefix = format!("{prefix}.");
    content
        .lines()
        .filter_map(|line| {
            let section = line.trim().strip_prefix('[')?.strip_suffix(']')?;
            let name = section.strip_prefix(&section_prefix)?;
            (!name.contains('.')).then(|| {
                serde_json::json!({
                    "name": unquote_toml_key(name),
                    "model": Value::Null,
                    "modelProvider": Value::Null,
                })
            })
        })
        .collect()
}

fn codex_mcp_servers_map(content: &str) -> Value {
    let mut map = serde_json::Map::new();
    for server in parse_codex_mcp_servers(content) {
        if let Some(name) = server.get("name").and_then(Value::as_str) {
            if let Some(raw) = server.get("raw") {
                map.insert(name.to_string(), raw.clone());
            }
        }
    }
    Value::Object(map)
}

fn parse_codex_mcp_servers(content: &str) -> Vec<Value> {
    let mut servers = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_config = serde_json::Map::new();
    let mut current_env = false;

    for line in content.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(section) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            if let Some(rest) = section.strip_prefix("mcp_servers.") {
                if let Some(name) = rest.strip_suffix(".env") {
                    let name = unquote_toml_key(name);
                    if current_name.as_deref() != Some(name.as_str()) {
                        flush_codex_mcp_server(
                            &mut servers,
                            &mut current_name,
                            &mut current_config,
                        );
                        current_name = Some(name);
                    }
                    current_env = true;
                } else {
                    flush_codex_mcp_server(&mut servers, &mut current_name, &mut current_config);
                    current_name = Some(unquote_toml_key(rest));
                    current_env = false;
                }
            } else {
                flush_codex_mcp_server(&mut servers, &mut current_name, &mut current_config);
                current_env = false;
            }
            continue;
        }

        if current_name.is_none() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        let Some(parsed) = parse_simple_toml_value(value.trim()) else {
            continue;
        };
        if current_env {
            let env = current_config
                .entry("env".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if let Value::Object(env) = env {
                env.insert(key, parsed);
            }
        } else {
            current_config.insert(key, parsed);
        }
    }
    flush_codex_mcp_server(&mut servers, &mut current_name, &mut current_config);
    servers
}

fn flush_codex_mcp_server(
    servers: &mut Vec<Value>,
    current_name: &mut Option<String>,
    current_config: &mut serde_json::Map<String, Value>,
) {
    let Some(name) = current_name.take() else {
        return;
    };
    let raw = Value::Object(std::mem::take(current_config));
    servers.push(mcp_server_record(&name, &raw, "user", None));
}

fn parse_simple_toml_value(value: &str) -> Option<Value> {
    let value = value.split('#').next().unwrap_or(value).trim();
    if let Some(string) = parse_toml_string(value) {
        return Some(Value::String(string));
    }
    if value.starts_with('[') && value.ends_with(']') {
        let inner = &value[1..value.len().saturating_sub(1)];
        let values = inner
            .split(',')
            .filter_map(|item| parse_toml_string(item.trim()).map(Value::String))
            .collect::<Vec<_>>();
        return Some(Value::Array(values));
    }
    match value {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        _ => value.parse::<i64>().ok().map(Value::from),
    }
}

fn parse_toml_string(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        Some(value[1..value.len() - 1].replace("\\\"", "\""))
    } else {
        None
    }
}

fn unquote_toml_key(value: &str) -> String {
    parse_toml_string(value).unwrap_or_else(|| value.trim().to_string())
}

fn compat_value(
    state: &AppState,
    user_id: &str,
    namespace: &str,
    path: &str,
    default_value: Value,
) -> Result<Value> {
    let key = user_setting_key(
        user_id,
        &format!("compat:{namespace}:{}", compat_path_key(path)),
    );
    Ok(state.storage.get_setting(&key)?.unwrap_or(default_value))
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
                "failed to create IO Gateway client",
                error.to_string(),
            )
        })?;

    // Try multiple catalog URLs (path-aware, then origin-based) so we can
    // match the URL shapes served by both OpenAI-compatible and Claude
    // /v1/models gateways. Mirrors web-ai-cli/server/utils/codex-models.js
    // `buildModelCatalogUrls`. The first URL that returns a non-empty model
    // list wins.
    let mut urls: Vec<String> = Vec::new();
    urls.push(format!("{base_url}/models"));
    urls.push(format!("{base_url}/v1/models"));
    if let Some(origin) = url_origin(&base_url) {
        urls.push(format!("{origin}/models"));
        urls.push(format!("{origin}/v1/models"));
    }

    for url in urls {
        let response = match client
            .get(&url)
            .bearer_auth(&api_key)
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => continue,
        };
        if !response.status().is_success() {
            continue;
        }
        let body = match response.json::<Value>().await {
            Ok(body) => body,
            Err(_) => continue,
        };
        let raw_models = body
            .get("data")
            .or_else(|| body.get("models"))
            .and_then(Value::as_array)
            .or_else(|| body.as_array())
            .cloned()
            .unwrap_or_default();
        if raw_models.is_empty() {
            continue;
        }
        let mapped: Vec<Value> = raw_models
            .into_iter()
            .filter_map(|model| {
                if model
                    .get("visibility")
                    .and_then(Value::as_str)
                    .is_some_and(|visibility| visibility.eq_ignore_ascii_case("hide"))
                {
                    return None;
                }
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
                            .get("slug")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .or_else(|| {
                        model
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })?;
                let label = model
                    .get("display_name")
                    .and_then(Value::as_str)
                    .or_else(|| model.get("label").and_then(Value::as_str))
                    .or_else(|| model.get("name").and_then(Value::as_str))
                    .unwrap_or(&value)
                    .to_string();
                Some(serde_json::json!({
                    "value": value,
                    "label": label,
                }))
            })
            .collect();
        if !mapped.is_empty() {
            return Ok(mapped);
        }
    }

    Ok(Vec::new())
}

/// Extract the origin (scheme + host + port) from a URL string. Returns
/// None for malformed URLs. Used to build origin-based fallbacks when
/// the configured base URL has a path prefix the gateway does not echo.
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

#[cfg(test)]
mod url_origin_tests {
    use super::url_origin;

    #[test]
    fn strips_trailing_path() {
        assert_eq!(
            url_origin("http://141.144.197.96:8319/claude"),
            Some("http://141.144.197.96:8319".to_string())
        );
    }

    #[test]
    fn leaves_origin_only_intact() {
        assert_eq!(
            url_origin("https://api.anthropic.com/"),
            Some("https://api.anthropic.com".to_string())
        );
    }

    #[test]
    fn returns_none_for_garbage() {
        assert_eq!(url_origin("not a url"), None);
    }
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
            "proxy" | "aiproxy" => Some(DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL.to_string()),
            _ => None,
        })?;

    let env_key = config
        .get("apiKeyEnv")
        .or_else(|| config.get("api_key_env"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let stored_gateway_key = config
        .get("gatewayApiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let api_key = if matches!(mode, "proxy" | "aiproxy") {
        stored_gateway_key
    } else {
        stored_gateway_key.or_else(|| {
            env_key
                .and_then(|key| env::var(key).ok())
                .or_else(|| match mode {
                    "direct" | "anthropic" => env::var("ANTHROPIC_API_KEY")
                        .or_else(|_| env::var("ANTHROPIC_AUTH_TOKEN"))
                        .ok(),
                    "minimax" => env::var("MINIMAX_API_KEY")
                        .or_else(|_| env::var("ANTHROPIC_API_KEY"))
                        .ok(),
                    _ => None,
                })
        })
    }
    .filter(|value| !value.trim().is_empty())?;

    Some((base_url, api_key))
}

fn validate_direct_ai_config(config: &Value) -> Result<()> {
    let allowed_modes = ["off", "proxy", "direct", "anthropic", "minimax", "aiproxy"];
    if let Some(mode) = config.get("mode").and_then(Value::as_str) {
        if !allowed_modes.contains(&mode) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "invalid IO Gateway mode",
            ));
        }
    }
    if let Some(runtime) = config
        .get("chatRuntime")
        .or_else(|| config.get("chat_runtime"))
        .and_then(Value::as_str)
        && parse_chat_runtime(runtime).is_none()
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "invalid chat runtime",
        ));
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
        "gemini" => Ok(Provider::Gemini),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Provider must be one of: claude, codex, gemini",
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
    let version = command_version(&command).await;
    let installed = version.is_some();
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
        "version": version,
    })
}

async fn cursor_cli_status() -> Value {
    let command = "cursor-agent";
    let installed = command_available(command).await;
    if !installed {
        return serde_json::json!({
            "authenticated": false,
            "email": Value::Null,
            "method": Value::Null,
            "error": "Cursor CLI is not installed",
            "installed": false,
            "command": command,
        });
    }

    let status = timeout(
        Duration::from_secs(5),
        Command::new(command)
            .arg("status")
            .env("PATH", augmented_user_path())
            .output(),
    )
    .await;
    let Ok(Ok(output)) = status else {
        return serde_json::json!({
            "authenticated": false,
            "email": Value::Null,
            "method": Value::Null,
            "error": "Command timeout",
            "installed": true,
            "command": command,
        });
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let authenticated = output.status.success() && stdout.contains("Logged in");
    let email =
        authenticated.then(|| extract_email(&stdout).unwrap_or_else(|| "Logged in".to_string()));
    let error = if authenticated {
        None
    } else {
        let error = stderr.trim();
        Some(if error.is_empty() {
            "Not logged in".to_string()
        } else {
            error.to_string()
        })
    };

    serde_json::json!({
        "authenticated": authenticated,
        "email": email,
        "method": authenticated.then_some("cli_login"),
        "error": error,
        "installed": true,
        "command": command,
    })
}

fn extract_email(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|token| {
        let candidate = token.trim_matches(|ch: char| {
            ch.is_ascii_punctuation()
                && ch != '@'
                && ch != '.'
                && ch != '_'
                && ch != '-'
                && ch != '+'
        });
        looks_like_email(candidate).then(|| candidate.to_string())
    })
}

fn provider_command(provider: Provider) -> String {
    let command = match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::Gemini => "gemini",
    };
    preferred_user_command(command).unwrap_or_else(|| command.to_string())
}

fn preferred_user_command(command: &str) -> Option<String> {
    let home = home_dir()?;
    let candidate = home.join(".local").join("bin").join(command);
    if candidate.is_file() {
        Some(candidate.display().to_string())
    } else {
        None
    }
}

async fn command_available(command: &str) -> bool {
    command_version(command).await.is_some()
}

async fn command_version(command: &str) -> Option<String> {
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        Command::new(command)
            .arg("--version")
            .env("PATH", augmented_user_path())
            .output(),
    )
    .await;
    let output = match result {
        Ok(Ok(output)) if output.status.success() => output,
        _ => return None,
    };
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    [stdout, stderr].into_iter().find(|value| !value.is_empty())
}

fn provider_auth_hint(provider: Provider) -> Option<String> {
    let env_key = match provider {
        Provider::Claude => ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"]
            .into_iter()
            .find(|key| env_has_value(key)),
        Provider::Codex => ["OPENAI_API_KEY"]
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

async fn claude_token_usage(project_path: &str, session_id: &str) -> Result<TokenUsageSnapshot> {
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

    let total = env::var("CONTEXT_WINDOW")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(160_000);

    Ok(TokenUsageSnapshot {
        usage: parse_claude_usage(&content),
        total,
    })
}

async fn codex_token_usage(session_id: &str) -> Result<TokenUsageSnapshot> {
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

    Ok(parse_codex_usage(&content))
}

fn token_usage_response(snapshot: &TokenUsageSnapshot) -> Value {
    serde_json::json!({
        "used": snapshot.usage.used,
        "total": snapshot.total,
        "breakdown": {
            "input": snapshot.usage.input,
            "output": snapshot.usage.output,
            "cacheCreation": snapshot.usage.cache_creation,
            "cacheRead": snapshot.usage.cache_read,
        },
        "costUsd": snapshot.usage.cost_usd,
    })
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

fn parse_claude_usage(content: &str) -> SessionTokenUsage {
    let mut usage_by_message = HashMap::new();
    for (line_index, line) in content.lines().enumerate() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let input = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_creation = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_read = usage
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let key = message
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("line-{line_index}"));
        usage_by_message.insert(
            key,
            SessionTokenUsage {
                used: input
                    .saturating_add(output)
                    .saturating_add(cache_creation)
                    .saturating_add(cache_read),
                input,
                output,
                cache_creation,
                cache_read,
                cost_usd: usage
                    .get("cost_usd")
                    .or_else(|| usage.get("costUsd"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
            },
        );
    }

    usage_by_message
        .into_values()
        .fold(SessionTokenUsage::default(), |mut total, usage| {
            total.used = total.used.saturating_add(usage.used);
            total.input = total.input.saturating_add(usage.input);
            total.output = total.output.saturating_add(usage.output);
            total.cache_creation = total.cache_creation.saturating_add(usage.cache_creation);
            total.cache_read = total.cache_read.saturating_add(usage.cache_read);
            total.cost_usd += usage.cost_usd;
            total
        })
}

fn parse_codex_usage(content: &str) -> TokenUsageSnapshot {
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

        let usage = info.get("total_token_usage").unwrap_or(info);
        let input = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let total = info
            .get("model_context_window")
            .and_then(Value::as_u64)
            .unwrap_or(200_000);
        return TokenUsageSnapshot {
            usage: SessionTokenUsage {
                used: usage
                    .get("total_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_else(|| input.saturating_add(output)),
                input,
                output,
                cache_creation: usage
                    .get("cache_write_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cache_read: usage
                    .get("cached_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cost_usd: usage
                    .get("cost_usd")
                    .or_else(|| usage.get("costUsd"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
            },
            total,
        };
    }
    TokenUsageSnapshot {
        usage: SessionTokenUsage::default(),
        total: 200_000,
    }
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

async fn cursor_compat(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<Json<Value>> {
    persisted_compat_endpoint(state, user, request, "cursor", default_cursor_compat()).await
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
    fn bounds_pathological_history_for_clients_and_prioritizes_newest_messages() {
        let now = Utc::now();
        let messages = vec![
            ChatMessage {
                id: "older-tool".to_string(),
                role: MessageRole::Tool,
                content: format!("<style>bad</style>\0{}", "a".repeat(900_000)),
                timestamp: now,
                metadata: serde_json::json!({"huge": "m".repeat(100_000)}),
            },
            ChatMessage {
                id: "latest-assistant".to_string(),
                role: MessageRole::Assistant,
                content: format!("{}FINAL-TAIL", "b".repeat(600_000)),
                timestamp: now,
                metadata: Value::Null,
            },
        ];

        let bounded = bound_session_messages_for_response(messages);
        assert_eq!(bounded.len(), 2);
        assert!(
            bounded
                .iter()
                .map(|message| message.content.len())
                .sum::<usize>()
                <= SESSION_RESPONSE_MAX_CONTENT_BYTES
        );
        assert!(bounded[0].content.len() <= SESSION_RESPONSE_TOOL_MAX_BYTES);
        assert!(bounded[1].content.len() <= SESSION_RESPONSE_ASSISTANT_MAX_BYTES);
        assert!(bounded[1].content.contains("FINAL-TAIL"));
        assert!(!bounded[0].content.contains('\0'));
        assert!(bounded.iter().all(|message| {
            message
                .content
                .lines()
                .all(|line| line.chars().count() <= SESSION_RESPONSE_MAX_LINE_CHARS + 80)
        }));
        assert_eq!(
            bounded[0]
                .metadata
                .get("contentTruncated")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            bounded[0]
                .metadata
                .get("metadataTruncated")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn io_gateway_config_uses_provider_specific_endpoint_without_env_key() {
        let mut config = serde_json::json!({
            "mode": "anthropic",
            "gatewayUrl": "https://gateway.example.com",
            "apiKeyEnv": "ANTHROPIC_API_KEY",
            "maxTokens": 1234,
        });

        apply_io_gateway_config(&mut config, Provider::Claude);

        assert_eq!(config.get("mode").and_then(Value::as_str), Some("aiproxy"));
        assert_eq!(
            config.get("baseUrl").and_then(Value::as_str),
            Some("https://gateway.example.com/claude")
        );
        assert!(config.get("apiKeyEnv").is_none());
        assert_eq!(config.get("maxTokens").and_then(Value::as_u64), Some(1234));
        apply_io_gateway_config(&mut config, Provider::Codex);
        assert_eq!(
            config.get("baseUrl").and_then(Value::as_str),
            Some("https://gateway.example.com/codex")
        );
    }

    #[test]
    fn io_gateway_config_does_not_duplicate_endpoint_inclusive_urls() {
        let mut codex = serde_json::json!({
            "gatewayUrl": "https://ai.qif.us/codex/",
        });
        apply_io_gateway_config(&mut codex, Provider::Codex);
        assert_eq!(
            codex.get("baseUrl").and_then(Value::as_str),
            Some("https://ai.qif.us/codex")
        );

        let mut claude = serde_json::json!({
            "gatewayUrl": "https://ai.qif.us/claude",
        });
        apply_io_gateway_config(&mut claude, Provider::Claude);
        assert_eq!(
            claude.get("baseUrl").and_then(Value::as_str),
            Some("https://ai.qif.us/claude")
        );

        let mut legacy_base = serde_json::json!({
            "baseUrl": "https://gateway.example.com/api/codex/",
        });
        apply_io_gateway_config(&mut legacy_base, Provider::Codex);
        assert_eq!(
            legacy_base.get("baseUrl").and_then(Value::as_str),
            Some("https://gateway.example.com/api/codex")
        );
    }

    #[test]
    fn io_gateway_config_preserves_custom_and_absolute_endpoint_overrides() {
        let mut relative = serde_json::json!({
            "gatewayUrl": "https://gateway.example.com/root",
            "codexEndpoint": "api/codex",
        });
        apply_io_gateway_config(&mut relative, Provider::Codex);
        assert_eq!(
            relative.get("baseUrl").and_then(Value::as_str),
            Some("https://gateway.example.com/root/api/codex")
        );

        let mut absolute = serde_json::json!({
            "gatewayUrl": "https://gateway.example.com/root",
            "codexEndpoint": "https://codex.example.com/custom/",
        });
        apply_io_gateway_config(&mut absolute, Provider::Codex);
        assert_eq!(
            absolute.get("baseUrl").and_then(Value::as_str),
            Some("https://codex.example.com/custom")
        );

        assert_eq!(
            join_io_gateway_endpoint_url("https://gateway.example.com/mycodex", "codex"),
            "https://gateway.example.com/mycodex/codex"
        );
    }

    #[test]
    fn io_gateway_runtime_does_not_fall_back_to_environment_credentials() {
        let config = serde_json::json!({
            "mode": "aiproxy",
            "baseUrl": "https://gateway.example.com/claude",
            "apiKeyEnv": "PATH",
        });

        assert_eq!(direct_ai_endpoint_config(&config), None);
    }

    #[test]
    fn parses_supported_chat_runtime_values() {
        assert_eq!(
            parse_chat_runtime("native_cli"),
            Some(ChatRuntime::NativeCli)
        );
        assert_eq!(
            parse_chat_runtime("io_gateway"),
            Some(ChatRuntime::IoGateway)
        );
        assert_eq!(parse_chat_runtime("invalid"), None);
    }

    #[test]
    fn claude_fallback_models_include_local_cli_aliases() {
        let models = fallback_models(Provider::Claude);

        for alias in ["sonnet", "opus", "haiku", "fable"] {
            assert!(
                models.iter().any(|model| model == alias),
                "models: {models:?}"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn io_gateway_chat_models_returns_empty_success_when_catalog_unavailable() {
        let root =
            std::env::temp_dir().join(format!("iowb-server-chat-models-{}", uuid::Uuid::new_v4()));
        let config_dir = root.join("config");
        std::fs::create_dir_all(&config_dir).expect("config directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");
        let user_id = "model-test-user";
        state
            .storage
            .set_setting(
                &user_setting_key(user_id, "direct-ai"),
                &serde_json::json!({
                    "chatRuntime": "io_gateway",
                    "mode": "aiproxy",
                    "gatewayUrl": "http://127.0.0.1:1",
                    "gatewayApiKey": "test-key",
                }),
            )
            .expect("setting");

        let Json(body) = chat_provider_models(
            State(state),
            Extension(AuthenticatedUser(iowb_protocol::UserProfile {
                id: user_id.to_string(),
                username: "model-test".to_string(),
                email: None,
                created_at: chrono::Utc::now(),
            })),
            Query(ChatModelsQuery {
                provider: Some("codex".to_string()),
            }),
        )
        .await
        .expect("models response");

        assert_eq!(body.get("success").and_then(Value::as_bool), Some(true));
        assert_eq!(
            body.get("gatewayAvailable").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            body.get("models").and_then(Value::as_array).map(Vec::len),
            Some(0)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn io_gateway_codex_chat_models_preserve_slug_ids_and_prefixed_aliases() {
        async fn models(headers: HeaderMap) -> impl IntoResponse {
            let bearer = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok());
            let api_key = headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok());
            if bearer != Some("Bearer stored-key") || api_key != Some("stored-key") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            Json(serde_json::json!({
                "models": [
                    {"slug": "gpt-5.6-sol", "display_name": "GPT-5.6-Sol"},
                    {"slug": "cod:gpt-5.6-sol", "display_name": "GPT-5.6-Sol (alias)"},
                    {
                        "slug": "gpt-5.6-sol-wm",
                        "display_name": "GPT-5.6-Sol-WM",
                        "visibility": "hide"
                    }
                ]
            }))
            .into_response()
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/codex/models", get(models))).await
        });

        let root = std::env::temp_dir().join(format!(
            "iowb-server-codex-chat-models-{}",
            uuid::Uuid::new_v4()
        ));
        let config_dir = root.join("config");
        std::fs::create_dir_all(&config_dir).expect("config directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");
        let user_id = "codex-model-test-user";
        state
            .storage
            .set_setting(
                &user_setting_key(user_id, "direct-ai"),
                &serde_json::json!({
                    "chatRuntime": "io_gateway",
                    "mode": "aiproxy",
                    "gatewayUrl": format!("http://{address}"),
                    "gatewayApiKey": "stored-key",
                }),
            )
            .expect("setting");

        let Json(body) = chat_provider_models(
            State(state),
            Extension(AuthenticatedUser(iowb_protocol::UserProfile {
                id: user_id.to_string(),
                username: "codex-model-test".to_string(),
                email: None,
                created_at: chrono::Utc::now(),
            })),
            Query(ChatModelsQuery {
                provider: Some("codex".to_string()),
            }),
        )
        .await
        .expect("models response");

        let model_values: Vec<_> = body
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| {
                Some((model.get("value")?.as_str()?, model.get("label")?.as_str()?))
            })
            .collect();
        assert_eq!(
            model_values,
            [
                ("gpt-5.6-sol", "GPT-5.6-Sol"),
                ("cod:gpt-5.6-sol", "GPT-5.6-Sol (alias)"),
            ],
            "gateway model ids must remain byte-for-byte selectable"
        );
        assert_eq!(
            body.get("gatewayAvailable").and_then(Value::as_bool),
            Some(true)
        );

        server.abort();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_chat_models_merge_local_and_gateway_models() {
        let mut models = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let base_models = vec![
            "gpt-5".to_string(),
            "gpt-5.4".to_string(),
            "cod:gpt-5.5".to_string(),
            "min:MiniMax-M3".to_string(),
            "gpt-5-codex".to_string(),
        ];

        push_codex_chat_models(&mut models, &mut seen, base_models);
        for model in ["cod:gpt-5.5", "min:MiniMax-M3", "unknown:model"] {
            if is_io_gateway_model(model) {
                push_chat_model(&mut models, &mut seen, model);
            }
        }

        assert_eq!(
            models,
            vec![
                "gpt-5".to_string(),
                "gpt-5.4".to_string(),
                "cod:gpt-5.5".to_string(),
                "min:MiniMax-M3".to_string(),
            ]
        );
    }

    #[test]
    fn io_gateway_config_replaces_legacy_env_reference() {
        let mut config = serde_json::json!({
            "mode": "aiproxy",
            "baseUrl": "http://127.0.0.1:8319/claude",
            "apiKeyEnv": "WRONG_KEY",
        });

        apply_io_gateway_config(&mut config, Provider::Claude);

        assert_eq!(
            config.get("baseUrl").and_then(Value::as_str),
            Some("http://127.0.0.1:8319/claude")
        );
        assert!(config.get("apiKeyEnv").is_none());
    }

    #[test]
    fn forced_io_gateway_config_uses_stored_gateway_url() {
        let mut config = serde_json::json!({
            "mode": "aiproxy",
            "gatewayUrl": "https://gateway.example.com/root/",
            "gatewayApiKey": "stored-key",
        });

        apply_io_gateway_config(&mut config, Provider::Claude);

        assert_eq!(
            config.get("baseUrl").and_then(Value::as_str),
            Some("https://gateway.example.com/root/claude")
        );
        assert_eq!(
            direct_ai_endpoint_config(&config),
            Some((
                "https://gateway.example.com/root/claude".to_string(),
                "stored-key".to_string(),
            ))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn io_gateway_model_catalog_uses_stored_key_and_keeps_full_list() {
        async fn models(headers: HeaderMap) -> impl IntoResponse {
            let bearer = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok());
            let api_key = headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok());
            if bearer != Some("Bearer stored-key") || api_key != Some("stored-key") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            Json(serde_json::json!({
                "data": [
                    {"id": "gpt-5.4"},
                    {"id": "claude-sonnet-4-5"},
                    {"id": "minimax-m3"}
                ]
            }))
            .into_response()
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/claude/models", get(models))).await
        });
        let config = serde_json::json!({
            "mode": "aiproxy",
            "baseUrl": format!("http://{address}/claude"),
            "gatewayApiKey": "stored-key",
        });

        let models = fetch_direct_ai_models(&config).await.expect("models");
        let values: Vec<_> = models
            .iter()
            .filter_map(|model| model.get("value").and_then(Value::as_str))
            .collect();
        assert_eq!(values, ["gpt-5.4", "claude-sonnet-4-5", "minimax-m3"]);

        server.abort();
    }

    #[test]
    fn public_direct_ai_config_redacts_stored_secrets() {
        let config = serde_json::json!({
            "mode": "aiproxy",
            "gatewayUrl": "https://gateway.example.com",
            "gatewayApiKey": "private-key",
            "gatewayOtpSecret": "PRIVATEOTP",
        });

        let public = public_direct_ai_config(&config);

        assert!(public.get("gatewayApiKey").is_none());
        assert!(public.get("gatewayOtpSecret").is_none());
        assert_eq!(
            public
                .get("gatewayApiKeyConfigured")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            public.get("gatewayOtpConfigured").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn direct_ai_secret_reveal_is_explicit_and_scoped() {
        let resolved = serde_json::json!({
            "gatewayApiKey": "private-key",
            "gatewayOtpSecret": "PRIVATEOTP",
        });
        let mut status = serde_json::json!({"success": true});

        assert!(status.get("secrets").is_none());
        add_direct_ai_secrets(&mut status, &resolved);

        assert_eq!(
            status
                .get("secrets")
                .and_then(|secrets| secrets.get("gatewayApiKey"))
                .and_then(Value::as_str),
            Some("private-key")
        );
        assert_eq!(
            status
                .get("secrets")
                .and_then(|secrets| secrets.get("gatewayOtpSecret"))
                .and_then(Value::as_str),
            Some("PRIVATEOTP")
        );
    }

    #[test]
    fn direct_ai_updates_preserve_omitted_stored_secrets() {
        let stored = serde_json::json!({
            "gatewayApiKey": "private-key",
            "gatewayOtpSecret": "PRIVATEOTP",
        });
        let mut update = serde_json::json!({
            "mode": "aiproxy",
            "gatewayUrl": "https://new-gateway.example.com",
        });

        preserve_direct_ai_secrets(&stored, &mut update);

        assert_eq!(
            update.get("gatewayApiKey").and_then(Value::as_str),
            Some("private-key")
        );
        assert_eq!(
            update.get("gatewayOtpSecret").and_then(Value::as_str),
            Some("PRIVATEOTP")
        );
    }

    #[test]
    fn parses_total_claude_usage_without_double_counting_stream_updates() {
        let content = r#"
{"type":"assistant","message":{"id":"msg-1","usage":{"input_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0}}}
{"type":"assistant","message":{"id":"msg-1","usage":{"input_tokens":10,"cache_creation_input_tokens":20,"cache_read_input_tokens":30,"output_tokens":40}}}
{"type":"assistant","message":{"id":"msg-2","usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4}}}
"#;

        let usage = parse_claude_usage(content);
        assert_eq!(usage.used, 110);
        assert_eq!(usage.input, 11);
        assert_eq!(usage.output, 44);
        assert_eq!(usage.cache_creation, 22);
        assert_eq!(usage.cache_read, 33);
    }

    #[test]
    fn parses_latest_codex_usage() {
        let content = r#"
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":4,"cache_write_input_tokens":2,"output_tokens":2,"total_tokens":12},"model_context_window":1000}}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":30,"cached_input_tokens":12,"cache_write_input_tokens":3,"output_tokens":12,"total_tokens":42},"model_context_window":2000}}}
"#;

        let snapshot = parse_codex_usage(content);
        assert_eq!(snapshot.usage.used, 42);
        assert_eq!(snapshot.usage.input, 30);
        assert_eq!(snapshot.usage.output, 12);
        assert_eq!(snapshot.usage.cache_creation, 3);
        assert_eq!(snapshot.usage.cache_read, 12);
        assert_eq!(snapshot.total, 2000);
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
        assert_eq!(
            repository_name_from_remote("git@github.com:example/mobile-app.git").as_deref(),
            Some("mobile-app")
        );
    }

    #[test]
    fn normalizes_project_watch_paths_and_ignores_git_metadata() {
        let root = "/tmp/project";
        assert_eq!(
            project_relative_watch_path(root, Path::new("/tmp/project/src/main.rs")).as_deref(),
            Some("src/main.rs"),
        );
        assert_eq!(
            project_relative_watch_path(root, Path::new("/tmp/project")).as_deref(),
            Some("."),
        );
        assert_eq!(
            project_relative_watch_path(root, Path::new("/tmp/project/.git/index")),
            None,
        );
        assert_eq!(
            project_relative_watch_path(root, Path::new("/tmp/another/file.txt")),
            None,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn startup_terminalizes_recovery_after_retry_limit() {
        let root =
            std::env::temp_dir().join(format!("iowb-server-recovery-{}", uuid::Uuid::new_v4()));
        let config_dir = root.join("config");
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("project directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");
        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        state
            .sessions
            .append_message(&session.id, MessageRole::User, "keep working")
            .await
            .expect("user message");
        let mut run = iowb_storage::StoredDurableChatRun::new(
            "retry-limit-run",
            Some("retry-user".to_string()),
            session.id.clone(),
            "codex",
            "keep working",
            project.display().to_string(),
        );
        run.resume_attempts = DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS;
        state
            .storage
            .create_durable_chat_run(&run)
            .expect("durable run");

        recover_interrupted_chat_runs(&state)
            .await
            .expect("startup reconciliation");

        let stored_run = state
            .storage
            .get_durable_chat_run(&run.id)
            .expect("read run")
            .expect("run exists");
        assert_eq!(stored_run.status, "interrupted");
        assert_eq!(
            stored_run.last_error.as_deref(),
            Some("automatic recovery attempt limit reached")
        );
        assert!(
            !state
                .storage
                .get_session(&session.id)
                .expect("read session")
                .expect("session exists")
                .active
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn startup_automatically_recovers_legacy_active_chat() {
        let root = std::env::temp_dir().join(format!(
            "iowb-server-legacy-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let config_dir = root.join("config");
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("project directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");
        let session = state
            .sessions
            .create_or_update(
                Provider::Gemini,
                project.display().to_string(),
                None,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        state
            .sessions
            .set_native_session_id(&session.id, "legacy-native-session")
            .await
            .expect("native session");
        state
            .sessions
            .append_message(&session.id, MessageRole::User, "finish legacy work")
            .await
            .expect("user message");
        assert!(
            state
                .storage
                .list_active_durable_chat_runs()
                .expect("list durable runs")
                .is_empty()
        );

        unsafe {
            std::env::set_var("IO_WORKBENCH_GEMINI_COMMAND", "/bin/sh");
            std::env::set_var(
                "IO_WORKBENCH_GEMINI_ARGS_JSON",
                r#"["-c","printf 'startup-resumed:%s\\n' \"$1\"","iowb-recovery","{native_session_id}"]"#,
            );
        }
        recover_interrupted_chat_runs(&state)
            .await
            .expect("startup recovery");
        unsafe {
            std::env::remove_var("IO_WORKBENCH_GEMINI_COMMAND");
            std::env::remove_var("IO_WORKBENCH_GEMINI_ARGS_JSON");
        }

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let runs = state
                    .storage
                    .list_active_durable_chat_runs()
                    .expect("active durable runs");
                if runs.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("automatic recovery completes");

        let messages = state.storage.list_messages(&session.id).expect("messages");
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .count(),
            1
        );
        assert!(messages.iter().any(|message| {
            message.role == MessageRole::Assistant
                && message
                    .content
                    .contains("startup-resumed:legacy-native-session")
        }));
        let durable_runs = state
            .storage
            .list_recoverable_durable_chat_runs(DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS, 10)
            .expect("recoverable runs");
        assert!(durable_runs.is_empty());
        assert!(
            !state
                .storage
                .get_session(&session.id)
                .expect("read session")
                .expect("session exists")
                .active
        );

        let _ = std::fs::remove_dir_all(root);
    }
}

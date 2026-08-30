use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    convert::Infallible,
    env,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use axum::{
    Extension, Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{
        DefaultBodyLimit, Multipart, Path as AxumPath, Query, Request, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    middleware,
    middleware::Next,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{any, delete, get, patch, post, put},
};
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
    BatchRenameFileRequest, BrowseFilesystemResponse, ChatMessage, ChatRuntime,
    CompactSessionContextRequest, CompactSessionContextResponse, CopyFileRequest,
    CreateFileRequest, CreateProjectRequest, CreateWorkspaceRequest, DeleteFcmTokenRequest,
    DeleteFileRequest, FcmTokenResponse, FileContentResponse, FileEntry, ForkSessionRequest,
    ForkSessionResponse, HealthResponse, HealthStatus, LoginRequest,
    ManualCompactSessionContextRequest, MessageRole, MessagesResponse, PRODUCT_NAME,
    PlaceholderResponse, ProcessInputRequest, ProcessResizeRequest, ProcessStartRequest,
    ProcessStartResponse, ProjectListResponse, ProjectSummary, PromptHistoryCursor,
    PromptHistoryResponse, Provider, RegisterFcmTokenRequest, RenameFileRequest,
    ServerStatusResponse, SessionDraftResponse, SessionSnapshotResponse, SessionSummary,
    SessionTokenUsage, UpdateSessionDraftRequest, WS_COMMAND_CHANNEL_CAPACITY, WorkspaceType,
    WsClientCommand, WsServerEvent, new_id,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    time::timeout,
};
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};
use tracing::{debug, info, warn};
use walkdir::WalkDir;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_SESSION_MODEL_LENGTH: usize = 200;
const MAX_SESSION_TITLE_LENGTH: usize = 500;
const MAX_UPLOAD_FILES: usize = 20;
const MAX_UPLOAD_FILE_BYTES: usize = 50 * 1024 * 1024;
const MAX_UPLOAD_IMAGES: usize = 5;
const PROJECT_WATCH_MAX_DIRECTORIES_PER_PROJECT: usize = 4_096;
const PROJECT_WATCH_MAX_DIRECTORIES_TOTAL: usize = 12_000;
const PROJECT_WATCH_EXCLUDED_DIRECTORIES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".gradle",
    ".idea",
    ".venv",
    ".cache",
    ".next",
    ".io-workbench",
    "node_modules",
    "target",
    "build",
    "dist",
    "venv",
    "Pods",
    "DerivedData",
];
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
const CODEX_TOKEN_USAGE_TAIL_MAX_BYTES: u64 = 8 * 1024 * 1024;
const CODEX_TOKEN_USAGE_CACHE_MAX_ENTRIES: usize = 64;

#[derive(Clone)]
struct CachedCodexTokenUsage {
    file_len: u64,
    modified_at: Option<SystemTime>,
    last_access: Instant,
    snapshot: TokenUsageSnapshot,
}

#[derive(Default)]
struct CodexTokenUsageCache {
    entries: HashMap<PathBuf, CachedCodexTokenUsage>,
    file_locks: HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>,
}

static CODEX_TOKEN_USAGE_CACHE: OnceLock<tokio::sync::Mutex<CodexTokenUsageCache>> =
    OnceLock::new();

pub async fn serve(config: AppConfig) -> anyhow::Result<()> {
    let addr = config.socket_addr();
    let state = AppState::initialize(config).await?;
    // This upgrade migration must finish before recovery can publish events
    // and before the listener can serve ordinary project/session discovery.
    agentic_board::backfill_legacy_board_sessions(&state)
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                error
                    .body
                    .details
                    .as_deref()
                    .filter(|details| !details.is_empty())
                    .map(|details| format!("{}: {details}", error.body.error))
                    .unwrap_or(error.body.error)
            )
        })?;
    recover_interrupted_chat_runs(&state).await?;
    spawn_process_event_bridge(state.clone());
    spawn_project_watch_bridge(state.clone());
    spawn_fcm_notification_bridge(state.clone());

    let listener = TcpListener::bind(addr).await?;
    info!(%addr, "starting io-workbench server");

    axum::serve(listener, build_router(state)).await?;
    Ok(())
}

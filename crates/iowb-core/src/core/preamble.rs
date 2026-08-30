use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    ffi::OsString,
    fs::OpenOptions,
    future::Future,
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::{
        Arc, RwLock as StdRwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use bcrypt::{DEFAULT_COST, hash, verify};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use iowb_fs::{FileService, WorkspacePathValidator};
use iowb_process::ProcessManager;
use iowb_protocol::{
    AuthStatusResponse, AuthTokenResponse, CONFIG_DIR_NAME, ChatContextRecovery, ChatMessage,
    ChatRuntime, CompactSessionContextResponse, DATABASE_FILE_NAME, ForkSessionResponse,
    MessageRole, PRODUCT_NAME, ProjectSummary, PromptHistoryCursor, PromptHistoryEntry, Provider,
    ServerStatusResponse, SessionDraftResponse, SessionLifetimeTokenUsage, SessionSummary,
    SessionTitleSource, SessionTokenUsage, TokenUsageCompleteness, UserProfile, WsServerEvent,
    new_id, session_title_from_prompt,
};
use iowb_storage::{
    CreateSessionForkOutcome, ExternalHistoryFingerprint, Storage, StoredChatRunAttempt,
    StoredDurableChatRun, StoredSessionContextRollover,
};
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{Mutex, RwLock, broadcast, mpsc, oneshot},
};
use tracing::{info, warn};
use uuid::Uuid;

use codex_app_server::{
    CodexAppServerClient, CodexAppServerLaunchOptions, CodexAppServerLiveTurnEvent,
    CodexAppServerLiveTurnOutcome, CodexAppServerLiveTurnParams, CodexAppServerTurnTerminalStatus,
    CodexThreadSnapshot,
};
use external_sessions::{
    EXTERNAL_MESSAGE_PARSER_VERSION, ExternalSessionRecord, external_file_fingerprint,
    load_external_messages, looks_like_codex_live_transcript, same_project_path,
    sync_external_sessions, visible_user_text,
};

type HmacSha1 = Hmac<Sha1>;
const DIRECT_AI_DISPLAY_CHUNK_CHARS: usize = 36;
const DIRECT_AI_SYNTHETIC_CHUNK_DELAY_MS: u64 = 45;
const DIRECT_AI_HISTORY_MAX_MESSAGES: usize = 48;
const DIRECT_AI_HISTORY_MAX_BYTES: usize = 96 * 1024;
const AGENT_LIVE_EVENT_MAX_BYTES: usize = 256 * 1024;
const AGENT_WEBSOCKET_CHUNK_MAX_BYTES: usize = 32 * 1024;
const AGENT_REPLAY_MAX_BYTES: usize = 1024 * 1024;
const AGENT_REPLAY_TOTAL_MAX_BYTES: usize = 4 * 1024 * 1024;
const AGENT_REPLAY_TOTAL_MAX_EVENTS: usize = 384;
const AGENT_TOOL_MESSAGE_MAX_BYTES: usize = 64 * 1024;
const AGENT_TOOL_MESSAGES_MAX_COUNT: usize = 96;
const AGENT_TOOL_MESSAGES_MAX_TOTAL_BYTES: usize = 512 * 1024;
const AGENT_ASSISTANT_MESSAGE_MAX_BYTES: usize = 256 * 1024;
const AGENT_DISPLAY_MAX_LINE_CHARS: usize = 8 * 1024;
const CODEX_MISSING_FINAL_RESPONSE: &str =
    "ERROR: Codex completed without a final assistant response.";
const AGENT_ABORT_TERM_GRACE: Duration = Duration::from_millis(250);
const AGENT_ABORT_REAP_TIMEOUT: Duration = Duration::from_secs(1);
const AGENT_ABORT_OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);
const DURABLE_AGENT_RUN_ENV: &str = "IO_WORKBENCH_DURABLE_RUN_ID";
const DURABLE_AGENT_SCOPE_ENV: &str = "IO_WORKBENCH_DURABLE_RUN_SCOPE";
const DURABLE_AGENT_OWNER_PID_ENV: &str = "IO_WORKBENCH_DURABLE_OWNER_PID";
const DURABLE_AGENT_OWNER_START_ENV: &str = "IO_WORKBENCH_DURABLE_OWNER_START";
const IO_WORKBENCH_GATEWAY_KEY_ENV: &str = "IOWB_IO_GATEWAY_API_KEY";
const CODEX_APP_SERVER_LIVE_ENV: &str = "IO_WORKBENCH_CODEX_APP_SERVER_LIVE";
const CODEX_APP_SERVER_LIVE_IO_GATEWAY_ENV: &str = "IO_WORKBENCH_CODEX_APP_SERVER_LIVE_IO_GATEWAY";
pub const DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS: u32 = 3;
const DURABLE_CHAT_RUN_RECOVERY_PROMPT_LIMIT: usize = 6_000;
const CODEX_GATEWAY_BODY_LIMIT_BYTES: u64 = 16 * 1024 * 1024;
const CODEX_CONTEXT_ROLLOVER_THRESHOLD_BYTES: u64 = 12 * 1024 * 1024;
const CONTEXT_ROLLOVER_HANDOFF_MAX_BYTES: usize = 48 * 1024;
const CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN: &str = "retry_failed_turn";
const CONTEXT_ROLLOVER_KIND_MANUAL: &str = "manual";
const EXTERNAL_MESSAGE_CACHE_MAX_ENTRIES: usize = 8;
const EXTERNAL_MESSAGE_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
const EXTERNAL_MESSAGE_TAIL_CACHE_MAX_MESSAGES: usize = 500;
const ORDERED_TEXT_MATCH_MATRIX_MAX_CELLS: usize = 1_000_000;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("storage error: {0}")]
    Storage(#[from] iowb_storage::StorageError),
    #[error("filesystem error: {0}")]
    Fs(#[from] iowb_fs::FsError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("project not found: {0}")]
    ProjectNotFound(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("authentication failed")]
    AuthenticationFailed,
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("password hashing failed: {0}")]
    PasswordHash(String),
    #[error("{0}")]
    InvalidInput(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;

struct RetryContextRolloverSource {
    recovery_run: StoredDurableChatRun,
    failed_prompt: String,
}

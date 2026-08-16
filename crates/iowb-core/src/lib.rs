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

mod codex_app_server;
mod external_sessions;

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

use codex_app_server::{CodexAppServerClient, CodexAppServerLaunchOptions, CodexThreadSnapshot};
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

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub host: IpAddr,
    pub port: u16,
    pub config_dir: PathBuf,
    pub database_path: PathBuf,
    pub workspace_root: PathBuf,
    pub auth_required: bool,
    pub local_token: Option<String>,
    pub otp_secret: Option<String>,
    pub max_sessions: usize,
    pub max_scan_depth: usize,
    pub max_file_read_bytes: u64,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let home = dirs::home_dir()
            .ok_or_else(|| CoreError::InvalidInput("could not resolve home directory".into()))?;
        let config_dir =
            env_path("IO_WORKBENCH_CONFIG_DIR").unwrap_or_else(|| home.join(CONFIG_DIR_NAME));
        let database_path = env_path("IO_WORKBENCH_DATABASE_PATH")
            .unwrap_or_else(|| config_dir.join(DATABASE_FILE_NAME));
        let workspace_root = env_path("IO_WORKBENCH_WORKSPACE_ROOT").unwrap_or(home);
        let host = env::var("IO_WORKBENCH_HOST")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let port = env::var("IO_WORKBENCH_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(8787);
        let auth_required = env_bool("IO_WORKBENCH_AUTH_REQUIRED", true);
        let local_token = env::var("IO_WORKBENCH_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty());
        let otp_secret = env::var("IO_WORKBENCH_OTP_SECRET")
            .ok()
            .map(|secret| secret.trim().to_string())
            .filter(|secret| !secret.is_empty());
        if let Some(secret) = otp_secret.as_deref() {
            decode_base32_secret(secret)?;
        }

        Ok(Self {
            host,
            port,
            config_dir,
            database_path,
            workspace_root,
            auth_required,
            local_token,
            otp_secret,
            max_sessions: env_usize("IO_WORKBENCH_MAX_SESSIONS", 100),
            max_scan_depth: env_usize("IO_WORKBENCH_MAX_SCAN_DEPTH", 6),
            max_file_read_bytes: env_u64("IO_WORKBENCH_MAX_FILE_READ_BYTES", 2 * 1024 * 1024),
        })
    }

    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }

    pub fn server_status(&self, version: &str) -> ServerStatusResponse {
        ServerStatusResponse {
            product: PRODUCT_NAME.to_string(),
            version: version.to_string(),
            config_dir: self.config_dir.display().to_string(),
            database_path: self.database_path.display().to_string(),
            workspace_root: self.workspace_root.display().to_string(),
            auth_required: self.auth_required
                || self.local_token.is_some()
                || self.otp_secret.is_some(),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub storage: Storage,
    pub auth: AuthManager,
    pub sessions: SessionManager,
    pub projects: ProjectIndex,
    pub agents: AgentRuntimeManager,
    pub tasks: TaskManager,
    pub processes: ProcessManager,
    pub files: FileService,
    pub path_validator: WorkspacePathValidator,
    pub watch: WatchManager,
    pub ws_hub: WsHub,
    codex_app_server: CodexAppServerClient,
    codex_app_server_mutation: Arc<Mutex<()>>,
}

impl AppState {
    pub async fn initialize(config: AppConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.config_dir)?;
        std::fs::create_dir_all(&config.workspace_root)?;
        let storage = Storage::open(&config.database_path)?;
        let files = FileService::new(config.max_scan_depth, config.max_file_read_bytes);
        let path_validator = WorkspacePathValidator::new(config.workspace_root.clone());
        let config = Arc::new(config);

        let sessions = SessionManager::load(storage.clone(), config.max_sessions)?;
        let state = Self {
            config: Arc::clone(&config),
            storage: storage.clone(),
            auth: AuthManager::new(Arc::clone(&config), storage.clone()),
            sessions,
            projects: ProjectIndex::new(storage),
            agents: AgentRuntimeManager::new(config.max_sessions),
            tasks: TaskManager::new(),
            processes: ProcessManager::new(),
            files,
            path_validator,
            watch: WatchManager::new(),
            ws_hub: WsHub::new(),
            codex_app_server: CodexAppServerClient::new(
                configured_codex_command(),
                Duration::from_secs(
                    env::var("IO_WORKBENCH_CODEX_APP_SERVER_TIMEOUT_SECS")
                        .ok()
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(15)
                        .clamp(1, 120),
                ),
            ),
            codex_app_server_mutation: Arc::new(Mutex::new(())),
        };

        info!(
            product = PRODUCT_NAME,
            config_dir = %state.config.config_dir.display(),
            database = %state.config.database_path.display(),
            "initialized app state"
        );
        Ok(state)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_agent_session(
        &self,
        provider: Provider,
        project_path: impl Into<String>,
        prompt: impl Into<String>,
        session_id: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<String>,
        thinking: Option<bool>,
        fast: Option<bool>,
        runtime: ChatRuntime,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
        user_id: Option<String>,
    ) -> Result<SessionSummary> {
        self.start_agent_session_scoped(
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
            user_id,
            None,
        )
        .await
    }

    /// Start a session owned by an agentic-board run. The scope is persisted
    /// before the provider starts and before any active-session broadcast, so
    /// the board chat can never briefly leak into ordinary chat discovery.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_board_agent_session(
        &self,
        provider: Provider,
        project_path: impl Into<String>,
        prompt: impl Into<String>,
        session_id: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<String>,
        thinking: Option<bool>,
        fast: Option<bool>,
        runtime: ChatRuntime,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
        user_id: Option<String>,
        board_run_id: impl Into<String>,
        board_task_id: Option<String>,
    ) -> Result<SessionSummary> {
        self.start_agent_session_scoped(
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
            user_id,
            Some((board_run_id.into(), board_task_id)),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_agent_session_scoped(
        &self,
        provider: Provider,
        project_path: impl Into<String>,
        prompt: impl Into<String>,
        session_id: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        mode: Option<String>,
        thinking: Option<bool>,
        fast: Option<bool>,
        runtime: ChatRuntime,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
        user_id: Option<String>,
        board_scope: Option<(String, Option<String>)>,
    ) -> Result<SessionSummary> {
        let project_path = project_path.into();
        let prompt = prompt.into();
        let resolved_project_path = self
            .path_validator
            .validate_path(PathBuf::from(&project_path), false)
            .await?;

        let metadata = tokio::fs::metadata(&resolved_project_path).await?;
        if !metadata.is_dir() {
            return Err(CoreError::InvalidInput(
                "project path must be a directory".to_string(),
            ));
        }

        // Recovery is a state transition for the current visible chat, not a
        // new turn. Reject ordinary sends before create_or_update can mark the
        // session active or append another user message to a poisoned native
        // thread. The compact-and-retry endpoint bypasses this method.
        if provider == Provider::Codex
            && let Some(existing_session_id) = session_id.as_deref()
            && self.context_recovery(existing_session_id).await?.is_some()
        {
            return Err(CoreError::Conflict(
                "this chat needs clean-context recovery before another message can be sent"
                    .to_string(),
            ));
        }

        let (external, native_resume_session_id) = if let Some(session_id) = session_id.as_deref() {
            let stored_session = self.sessions.get_stored(session_id);
            let has_context_rollover = self.storage.has_active_context_rollover(session_id)?;
            let external = !has_context_rollover
                && (self
                    .sessions
                    .external_record(
                        session_id,
                        Some(provider),
                        Some(&resolved_project_path.display().to_string()),
                    )
                    .await
                    .is_some()
                    || stored_session
                        .as_ref()
                        .is_some_and(|session| session.external));
            let native_resume_session_id = if has_context_rollover {
                stored_session.and_then(|session| session.native_session_id)
            } else if external {
                Some(session_id.to_string())
            } else {
                match stored_session.and_then(|session| session.native_session_id) {
                    Some(native_session_id) => Some(native_session_id),
                    None => {
                        self.sessions
                            .infer_native_session_id(
                                session_id,
                                provider,
                                &resolved_project_path.display().to_string(),
                            )
                            .await?
                    }
                }
            };
            (external, native_resume_session_id)
        } else {
            (false, None)
        };

        let native_before_turn_id = if provider == Provider::Codex {
            if let Some(native_session_id) = native_resume_session_id.as_deref() {
                match self.codex_app_server.read_thread(native_session_id).await {
                    Ok(snapshot) => snapshot.latest_forkable_turn_id().map(str::to_string),
                    Err(error) => {
                        warn!(
                            error = %error,
                            native_session_id,
                            "failed to capture Codex turn boundary before starting prompt"
                        );
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };
        let native_rollout_bytes =
            if provider == Provider::Codex && native_resume_session_id.is_some() {
                if let Some(session_id) = session_id.as_deref() {
                    self.sessions.native_rollout_size(session_id).await
                } else {
                    None
                }
            } else {
                None
            };

        let mut session = if let Some((board_run_id, board_task_id)) = board_scope {
            self.sessions
                .create_or_update_board(
                    provider,
                    resolved_project_path.display().to_string(),
                    session_id,
                    external,
                    model.clone(),
                    Some(runtime),
                    effort.clone(),
                    mode.clone(),
                    thinking,
                    fast,
                    board_run_id,
                    board_task_id,
                )
                .await?
        } else {
            self.sessions
                .create_or_update(
                    provider,
                    resolved_project_path.display().to_string(),
                    session_id,
                    external,
                    model.clone(),
                    Some(runtime),
                    effort.clone(),
                    mode.clone(),
                    thinking,
                    fast,
                )
                .await?
        };

        // The durable row is committed before the provider starts. If the
        // server is killed at any point after this write, startup recovery has
        // enough information to launch a continuation in the same chat.
        for stale_run in self
            .storage
            .list_active_durable_chat_runs()?
            .into_iter()
            .filter(|run| run.session_id == session.id)
        {
            self.storage.mark_durable_chat_run_interrupted(
                &stale_run.id,
                Some("superseded by a newer turn in the same session"),
            )?;
        }
        let durable_run_id = new_id("run");
        let mut durable_run = StoredDurableChatRun::new(
            durable_run_id.clone(),
            user_id,
            session.id.clone(),
            provider.as_str(),
            prompt.clone(),
            resolved_project_path.display().to_string(),
        );
        durable_run.native_session_id = native_resume_session_id.clone();
        durable_run.model = model.clone();
        durable_run.effort = effort.clone();
        durable_run.mode = mode.clone();
        durable_run.thinking = thinking;
        durable_run.fast = fast;
        durable_run.native_before_turn_id = native_before_turn_id.clone();
        if !prompt.trim().is_empty() {
            let now = Utc::now();
            let user_message_id = new_id("msg");
            durable_run.user_message_id = Some(user_message_id.clone());
            let mut user_metadata = serde_json::json!({
                "cli": provider.as_str(),
                "durableRunId": durable_run_id,
                "model": model.clone().unwrap_or_default(),
                "runtime": runtime,
                "effort": effort.clone().unwrap_or_default(),
                "mode": mode.clone().unwrap_or_default(),
                "thinking": thinking.unwrap_or(false),
                "fast": fast.unwrap_or(false),
                "sentAt": now.to_rfc3339(),
            });
            if let Some(native_before_turn_id) = native_before_turn_id.as_deref()
                && let Some(metadata) = user_metadata.as_object_mut()
            {
                metadata.insert(
                    "nativeBeforeTurnId".to_string(),
                    Value::String(native_before_turn_id.to_string()),
                );
            }
            let message = ChatMessage {
                id: user_message_id,
                role: MessageRole::User,
                content: prompt.clone(),
                timestamp: now,
                metadata: user_metadata,
            };
            match self
                .sessions
                .append_user_message_with_durable_run(&session.id, message, &durable_run)
                .await
            {
                Ok(updated) => session = updated,
                Err(error) => {
                    let _ = self.sessions.set_active(&session.id, false).await;
                    return Err(error);
                }
            }
        } else if let Err(error) = self.storage.create_durable_chat_run(&durable_run) {
            let _ = self.sessions.set_active(&session.id, false).await;
            return Err(error.into());
        }

        if provider == Provider::Codex
            && runtime == ChatRuntime::IoGateway
            && native_resume_session_id.is_some()
            && native_rollout_bytes
                .is_some_and(|bytes| bytes >= CODEX_CONTEXT_ROLLOVER_THRESHOLD_BYTES)
        {
            let observed_bytes = native_rollout_bytes;
            let message = format!(
                "native Codex context is {} bytes, above the {} byte safe rollover threshold",
                observed_bytes.unwrap_or_default(),
                CODEX_CONTEXT_ROLLOVER_THRESHOLD_BYTES,
            );
            self.storage
                .mark_durable_chat_run_failed(&durable_run_id, &message)?;
            session = self.sessions.set_active(&session.id, false).await?;
            if let Some(failed_message_id) = durable_run.user_message_id.clone() {
                self.ws_hub.publish(WsServerEvent::ChatRecoveryRequired {
                    provider,
                    session_id: session.id.clone(),
                    response_id: None,
                    recovery: ChatContextRecovery {
                        code: "context_too_large".to_string(),
                        state: "required".to_string(),
                        message: "This chat is close to the gateway request limit. Compact it into a clean context to continue without changing the visible chat.".to_string(),
                        failed_message_id,
                        observed_bytes,
                        limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
                        request_id: None,
                    },
                });
            }
            self.ws_hub.publish(WsServerEvent::ActiveSessions {
                sessions: self.sessions.list_active().await,
            });
            return Ok(session);
        }

        let direct_ai_messages = if should_use_direct_ai_gateway_runtime(provider, model.as_deref())
        {
            direct_ai_conversation_messages(self.sessions.messages(&session.id)?, prompt.as_str())
        } else {
            Vec::new()
        };

        let attempt_id = new_id("attempt");
        self.storage
            .create_chat_run_attempt(&StoredChatRunAttempt::new(
                attempt_id.clone(),
                durable_run_id.clone(),
                session.id.clone(),
                durable_run.user_message_id.clone(),
                provider.as_str(),
                runtime_label(runtime),
                model.clone(),
                native_resume_session_id.clone(),
            ))?;

        let start_result = self
            .agents
            .start(AgentStartContext {
                provider,
                session_id: session.id.clone(),
                durable_run_id: Some(durable_run_id.clone()),
                attempt_id: Some(attempt_id),
                response_id: new_id("response"),
                sequence: Arc::new(AtomicU64::new(0)),
                project_path: resolved_project_path,
                prompt,
                model,
                runtime,
                effort: effort.clone(),
                mode: mode.clone(),
                thinking,
                fast,
                native_resume_session_id,
                context_rollover_id: None,
                direct_ai_config,
                direct_ai_messages,
                sessions: self.sessions.clone(),
                storage: self.storage.clone(),
                hub: self.ws_hub.clone(),
            })
            .await;
        if let Err(error) = start_result {
            let message = error.to_string();
            let _ = self
                .storage
                .mark_durable_chat_run_failed(&durable_run_id, &message);
            let _ = self.sessions.set_active(&session.id, false).await;
            return Err(error);
        }

        self.ws_hub.publish(WsServerEvent::ActiveSessions {
            sessions: self.sessions.list_active().await,
        });

        Ok(session)
    }

    pub async fn fork_session_before_message(
        &self,
        user_id: &str,
        source_session_id: &str,
        before_message_id: &str,
        request_id: &str,
        replace: bool,
        draft_content: Option<&str>,
    ) -> Result<ForkSessionResponse> {
        if let Some(existing) =
            self.storage
                .get_session_fork(user_id, source_session_id, request_id)?
        {
            return self
                .existing_fork_response(
                    user_id,
                    source_session_id,
                    &existing.before_message_id,
                    &existing.destination_session_id,
                    existing.replaces_source,
                )
                .await;
        }

        let source = self.sessions.get(source_session_id).await?;
        if source.active {
            return Err(CoreError::Conflict(
                "session_active: stop the current response before editing from here".to_string(),
            ));
        }
        let source_is_stored = self
            .storage
            .get_session_summary(source_session_id)?
            .is_some();
        let source_messages = self
            .sessions
            .messages_including_external(source_session_id)
            .await?;
        let target_index = source_messages
            .iter()
            .position(|message| message.id == before_message_id)
            .ok_or_else(|| {
                CoreError::InvalidInput(format!(
                    "message {before_message_id} was not found in session {source_session_id}"
                ))
            })?;
        let target = &source_messages[target_index];
        if target.role != MessageRole::User {
            return Err(CoreError::InvalidInput(
                "Edit from here requires a user prompt".to_string(),
            ));
        }
        let prefix = &source_messages[..target_index];
        let draft_content = draft_content.unwrap_or(&target.content);
        let has_prior_user_turn = prefix
            .iter()
            .any(|message| message.role == MessageRole::User);

        let mut native_forked_thread_id = None;
        if has_prior_user_turn {
            if source.provider == Provider::Codex {
                let native_session_id = if source.external {
                    Some(source.id.clone())
                } else if source.native_session_id.is_some() {
                    source.native_session_id.clone()
                } else {
                    self.sessions
                        .infer_native_session_id(
                            source_session_id,
                            source.provider,
                            &source.project_path,
                        )
                        .await?
                }
                .ok_or_else(|| {
                    CoreError::Conflict(
                        "codex_native_session_unavailable: this chat is not linked to a Codex thread"
                            .to_string(),
                    )
                })?;
                let snapshot = self
                    .codex_app_server
                    .read_thread(&native_session_id)
                    .await?;
                if snapshot.id != native_session_id {
                    warn!(
                        requested_thread_id = %native_session_id,
                        returned_thread_id = %snapshot.id,
                        "Codex thread/read returned a different thread id"
                    );
                }
                let last_turn_id = self.resolve_codex_fork_boundary(
                    source_session_id,
                    target,
                    &source_messages,
                    &snapshot,
                )?;
                native_forked_thread_id = Some({
                    let _mutation = self.codex_app_server_mutation.lock().await;
                    self.codex_app_server
                        .fork_thread(&native_session_id, &last_turn_id)
                        .await?
                });
            } else if !should_use_direct_ai_gateway_runtime(
                source.provider,
                source.model.as_deref(),
            ) {
                return Err(CoreError::InvalidInput(format!(
                    "Edit from here is not yet available for native {} sessions with earlier turns",
                    source.provider.as_str()
                )));
            }
        }

        let now = Utc::now();
        let destination_id = new_id("session");
        let cloned_messages = prefix
            .iter()
            .map(|message| clone_forked_message(source_session_id, message))
            .collect::<Vec<_>>();
        let destination = SessionSummary {
            id: destination_id,
            provider: source.provider,
            external: false,
            board_session: source.board_session,
            board_run_id: source.board_run_id.clone(),
            board_task_id: source.board_task_id.clone(),
            native_session_id: native_forked_thread_id.clone(),
            title_source: Some(SessionTitleSource::Prompt),
            project_path: source.project_path.clone(),
            title: session_title_from_prompt(draft_content)
                .unwrap_or_else(|| "New Session".to_string()),
            message_count: cloned_messages.len(),
            last_activity: now,
            active: false,
            model: source.model.clone(),
            runtime: source.runtime,
            effort: source.effort.clone(),
            mode: source.mode.clone(),
            thinking: source.thinking,
            fast: source.fast,
            last_message_at: cloned_messages.last().map(|message| message.timestamp),
            first_user_at: cloned_messages
                .iter()
                .find(|message| message.role == MessageRole::User)
                .map(|message| message.timestamp),
            received_at: cloned_messages
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::Assistant)
                .map(|message| message.timestamp),
            token_usage: None,
            lifetime_token_usage: None,
        };

        let outcome = self.storage.create_session_fork(
            user_id,
            source_session_id,
            before_message_id,
            request_id,
            &destination,
            &cloned_messages,
            draft_content,
            source_is_stored,
            replace,
        );
        match outcome {
            Ok(CreateSessionForkOutcome::Created) => {
                self.sessions
                    .remember_persisted_session(destination.clone())
                    .await?;
                Ok(ForkSessionResponse {
                    source_session_id: source_session_id.to_string(),
                    before_message_id: before_message_id.to_string(),
                    session: destination.clone(),
                    draft: SessionDraftResponse {
                        session_id: destination.id,
                        content: draft_content.to_string(),
                        updated_at: Some(now),
                    },
                    native_forked: native_forked_thread_id.is_some(),
                    files_unchanged: true,
                    source_hidden: replace,
                })
            }
            Ok(CreateSessionForkOutcome::Existing(existing)) => {
                self.delete_compensating_codex_fork(native_forked_thread_id.as_deref())
                    .await;
                self.existing_fork_response(
                    user_id,
                    source_session_id,
                    &existing.before_message_id,
                    &existing.destination_session_id,
                    existing.replaces_source,
                )
                .await
            }
            Ok(CreateSessionForkOutcome::SourceActive) => {
                self.delete_compensating_codex_fork(native_forked_thread_id.as_deref())
                    .await;
                Err(CoreError::Conflict(
                    "session_active: stop the current response before editing from here"
                        .to_string(),
                ))
            }
            Err(error) => {
                self.delete_compensating_codex_fork(native_forked_thread_id.as_deref())
                    .await;
                Err(error.into())
            }
        }
    }

    async fn existing_fork_response(
        &self,
        user_id: &str,
        source_session_id: &str,
        before_message_id: &str,
        destination_session_id: &str,
        source_hidden: bool,
    ) -> Result<ForkSessionResponse> {
        let session = self.sessions.get(destination_session_id).await?;
        let draft = self
            .storage
            .get_session_draft(user_id, destination_session_id)?;
        Ok(ForkSessionResponse {
            source_session_id: source_session_id.to_string(),
            before_message_id: before_message_id.to_string(),
            native_forked: session.native_session_id.is_some(),
            files_unchanged: true,
            source_hidden,
            session,
            draft,
        })
    }

    fn resolve_codex_fork_boundary(
        &self,
        source_session_id: &str,
        target: &ChatMessage,
        messages: &[ChatMessage],
        snapshot: &CodexThreadSnapshot,
    ) -> Result<String> {
        if let Some(boundary) = target
            .metadata
            .get("nativeBeforeTurnId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return Ok(boundary.to_string());
        }
        if let Some(boundary) = self
            .storage
            .durable_chat_run_for_user_message(source_session_id, &target.id)?
            .and_then(|run| run.native_before_turn_id)
            .filter(|value| !value.trim().is_empty())
        {
            return Ok(boundary);
        }
        if let Some(native_message_id) = target
            .metadata
            .get("nativeMessageId")
            .and_then(Value::as_str)
            && let Some(turn_index) = snapshot.turns.iter().position(|turn| {
                turn.user_item_ids
                    .iter()
                    .any(|item_id| item_id == native_message_id)
            })
            && let Some(previous) = turn_index
                .checked_sub(1)
                .and_then(|index| snapshot.turns.get(index))
        {
            return Ok(previous.id.clone());
        }

        let local_users = messages
            .iter()
            .filter(|message| message.role == MessageRole::User)
            .collect::<Vec<_>>();
        let target_user_index = local_users
            .iter()
            .position(|message| message.id == target.id)
            .ok_or_else(|| {
                CoreError::InvalidInput(
                    "selected prompt was not present in user history".to_string(),
                )
            })?;
        let native_user_turns = snapshot
            .turns
            .iter()
            .enumerate()
            .filter(|(_, turn)| !turn.user_text.trim().is_empty())
            .collect::<Vec<_>>();
        let local_text = local_users
            .iter()
            .map(|message| normalized_fork_prompt(&message.content))
            .collect::<Vec<_>>();
        let native_text = native_user_turns
            .iter()
            .map(|(_, turn)| normalized_fork_prompt(&turn.user_text))
            .collect::<Vec<_>>();
        let matches = ordered_text_matches(&local_text, &native_text);
        let native_user_index = matches
            .iter()
            .find_map(|(local_index, native_index)| {
                (*local_index == target_user_index).then_some(*native_index)
            })
            .ok_or_else(|| {
                CoreError::Conflict(
                    "codex_turn_boundary_unresolved: refresh the session and try again".to_string(),
                )
            })?;
        let native_turn_index = native_user_turns[native_user_index].0;
        snapshot
            .turns
            .get(native_turn_index.saturating_sub(1))
            .filter(|_| native_turn_index > 0)
            .map(|turn| turn.id.clone())
            .ok_or_else(|| {
                CoreError::Conflict(
                    "codex_turn_boundary_unresolved: selected prompt has no prior native turn"
                        .to_string(),
                )
            })
    }

    async fn delete_compensating_codex_fork(&self, thread_id: Option<&str>) {
        let Some(thread_id) = thread_id else {
            return;
        };
        let result = {
            let _mutation = self.codex_app_server_mutation.lock().await;
            self.codex_app_server.delete_thread(thread_id).await
        };
        if let Err(error) = result {
            warn!(
                error = %error,
                thread_id,
                "failed to delete uncommitted Codex fork"
            );
        }
    }

    /// Continue a durable run that was left active when the Rust server
    /// process stopped. This is an internal startup path: it deliberately does
    /// not append another visible user message or create a second durable row.
    pub async fn recover_agent_run(
        &self,
        run: StoredDurableChatRun,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
    ) -> Result<SessionSummary> {
        let provider = parse_stored_provider(&run.provider)?;
        let stored_session = self
            .storage
            .get_session_summary(&run.session_id)?
            .ok_or_else(|| CoreError::SessionNotFound(run.session_id.clone()))?;
        let runtime = stored_session
            .runtime
            .unwrap_or_else(|| legacy_chat_runtime(run.model.as_deref()));

        if stored_session.provider != provider {
            return Err(CoreError::InvalidInput(format!(
                "durable run {} provider {} does not match session provider {}",
                run.id,
                provider.as_str(),
                stored_session.provider.as_str()
            )));
        }

        // If the server died in the narrow window after persisting the final
        // assistant message but before terminalizing the durable row, do not
        // invoke the provider a second time.
        let already_persisted =
            self.storage
                .list_messages(&run.session_id)?
                .iter()
                .any(|message| {
                    message.role == MessageRole::Assistant
                        && message.metadata.get("durableRunId").and_then(Value::as_str)
                            == Some(run.id.as_str())
                });
        if already_persisted {
            self.storage.mark_durable_chat_run_completed(&run.id)?;
            let session = self.sessions.set_active(&run.session_id, false).await?;
            info!(
                run_id = %run.id,
                session_id = %run.session_id,
                "reconciled durable chat run whose final assistant message was already persisted"
            );
            return Ok(session);
        }

        let resolved_project_path = self
            .path_validator
            .validate_path(PathBuf::from(&run.project_path), false)
            .await?;
        let metadata = tokio::fs::metadata(&resolved_project_path).await?;
        if !metadata.is_dir() {
            return Err(CoreError::InvalidInput(format!(
                "durable run {} project path must be a directory",
                run.id
            )));
        }

        let context_rollover = self.storage.context_rollover_for_retry_run(&run.id)?;
        let mut native_resume_session_id = if let Some(rollover) = context_rollover.as_ref() {
            rollover
                .candidate_native_session_id
                .clone()
                .filter(|candidate| {
                    run.native_session_id
                        .as_deref()
                        .is_none_or(|run_native| run_native == candidate)
                })
        } else {
            run.native_session_id
                .clone()
                .or_else(|| stored_session.native_session_id.clone())
        };
        if context_rollover.is_none() {
            if native_resume_session_id.is_none() && stored_session.external {
                native_resume_session_id = Some(run.session_id.clone());
            }
            if native_resume_session_id.is_none() {
                native_resume_session_id = self
                    .sessions
                    .infer_native_session_id(
                        &run.session_id,
                        provider,
                        &resolved_project_path.display().to_string(),
                    )
                    .await?;
            }
        }
        if let Some(native_session_id) = native_resume_session_id.as_deref() {
            self.storage
                .update_durable_chat_run_native_session_id(&run.id, Some(native_session_id))?;
            if context_rollover.is_none() {
                self.sessions
                    .set_native_session_id(&run.session_id, native_session_id)
                    .await?;
            }
        }

        let session = self.sessions.set_active(&run.session_id, true).await?;
        let recovery_prompt = durable_chat_recovery_prompt(&run.prompt);
        let direct_ai_messages =
            if should_use_direct_ai_gateway_runtime(provider, run.model.as_deref()) {
                let mut messages =
                    direct_ai_conversation_messages(self.sessions.messages(&run.session_id)?, "");
                append_direct_ai_recovery_prompt(&mut messages, &recovery_prompt);
                messages
            } else {
                Vec::new()
            };

        let response_id = if context_rollover.is_some() {
            run.id.clone()
        } else {
            new_id("response")
        };
        let attempt_id = new_id("attempt");
        self.storage
            .create_chat_run_attempt(&StoredChatRunAttempt::new(
                attempt_id.clone(),
                run.id.clone(),
                run.session_id.clone(),
                run.user_message_id.clone(),
                provider.as_str(),
                runtime_label(runtime),
                run.model.clone(),
                native_resume_session_id.clone(),
            ))?;
        let start_result = self
            .agents
            .start(AgentStartContext {
                provider,
                session_id: run.session_id.clone(),
                durable_run_id: Some(run.id.clone()),
                attempt_id: Some(attempt_id),
                response_id,
                sequence: Arc::new(AtomicU64::new(0)),
                project_path: resolved_project_path,
                prompt: recovery_prompt,
                model: run.model.clone(),
                runtime,
                effort: run.effort.clone(),
                mode: run.mode.clone(),
                thinking: run.thinking,
                fast: run.fast,
                native_resume_session_id,
                context_rollover_id: context_rollover
                    .as_ref()
                    .map(|rollover| rollover.id.clone()),
                direct_ai_config,
                direct_ai_messages,
                sessions: self.sessions.clone(),
                storage: self.storage.clone(),
                hub: self.ws_hub.clone(),
            })
            .await;

        if let Err(error) = start_result {
            let message = error.to_string();
            if let Some(rollover) = context_rollover.as_ref() {
                let _ = self.storage.fail_context_rollover(&rollover.id, &message);
            }
            let _ = self.storage.mark_durable_chat_run_failed(&run.id, &message);
            let _ = self.sessions.set_active(&run.session_id, false).await;
            return Err(error);
        }

        self.ws_hub.publish(WsServerEvent::ActiveSessions {
            sessions: self.sessions.list_active().await,
        });
        info!(
            run_id = %run.id,
            session_id = %run.session_id,
            attempt = run.resume_attempts,
            provider = provider.as_str(),
            "started automatic recovery for interrupted chat run"
        );
        Ok(session)
    }

    pub async fn abort_agent_session(&self, provider: Provider, session_id: &str) -> Result<bool> {
        let aborted = self.agents.abort(provider, session_id).await;
        if !aborted {
            for run in self
                .storage
                .list_active_durable_chat_runs()?
                .into_iter()
                .filter(|run| run.session_id == session_id && run.provider == provider.as_str())
            {
                self.storage.mark_durable_chat_run_terminal(
                    &run.id,
                    "aborted",
                    Some("aborted while no provider process was attached"),
                )?;
            }
            let _ = self.sessions.set_active(session_id, false).await?;
            self.ws_hub.publish(WsServerEvent::SessionStatus {
                provider,
                session_id: session_id.to_string(),
                status: iowb_protocol::SessionRuntimeStatus::Aborted,
                response_id: None,
                sequence: None,
                latest_user_prompt: None,
            });
        }
        self.ws_hub.publish(WsServerEvent::ActiveSessions {
            sessions: self.sessions.list_active().await,
        });
        Ok(aborted)
    }

    pub async fn compact_and_retry_session_context(
        &self,
        user_id: &str,
        session_id: &str,
        failed_message_id: &str,
        request_id: &str,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
    ) -> Result<CompactSessionContextResponse> {
        if let Some(existing) = self
            .storage
            .context_rollover_for_request(user_id, session_id, request_id)?
        {
            if existing.kind != CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN {
                return Err(CoreError::Conflict(
                    "this compaction request id was already used for a different operation"
                        .to_string(),
                ));
            }
            if existing.failed_message_id != failed_message_id {
                return Err(CoreError::Conflict(
                    "this recovery request id was already used for a different failed message"
                        .to_string(),
                ));
            }
            return Ok(CompactSessionContextResponse {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                response_id: existing.retry_run_id,
                state: existing.state,
            });
        }

        let session = self.sessions.get(session_id).await?;
        if session.provider != Provider::Codex {
            return Err(CoreError::InvalidInput(
                "clean context rollover is currently available only for Codex sessions".to_string(),
            ));
        }
        if session.active || self.agents.is_running(Provider::Codex, session_id).await {
            return Err(CoreError::Conflict(
                "stop the active response before compacting this chat".to_string(),
            ));
        }
        let failed_message = self
            .storage
            .message_by_id(session_id, failed_message_id)?
            .filter(|message| message.role == MessageRole::User)
            .ok_or_else(|| {
                CoreError::InvalidInput("failed user message was not found".to_string())
            })?;
        let failed_run = self
            .storage
            .durable_chat_run_for_user_message(session_id, failed_message_id)?
            .filter(|run| run.status == "failed")
            .ok_or_else(|| {
                CoreError::Conflict(
                    "the selected user message does not belong to a failed turn".to_string(),
                )
            })?;
        let latest_run = self
            .storage
            .latest_durable_chat_run_for_session(session_id)?;
        if latest_run.as_ref().map(|run| run.id.as_str()) != Some(failed_run.id.as_str()) {
            return Err(CoreError::Conflict(
                "only the latest failed turn can be retried with a clean context".to_string(),
            ));
        }

        let observed_bytes = self
            .sessions
            .native_rollout_size(session_id)
            .await
            .filter(|size| *size > 0);
        let visible_messages = context_materialization_messages(
            session_id,
            self.sessions
                .messages_including_external(session_id)
                .await?,
            &[failed_message_id],
        );
        let handoff =
            build_context_rollover_handoff(visible_messages.clone(), &failed_message.content);
        let rollover_id = new_id("rollover");
        let compact_run_id = new_id("run");
        let now = Utc::now();
        let rollover = StoredSessionContextRollover {
            id: rollover_id.clone(),
            user_id: user_id.to_string(),
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            kind: CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN.to_string(),
            failed_message_id: failed_message_id.to_string(),
            trigger_run_id: failed_run.id.clone(),
            retry_run_id: compact_run_id.clone(),
            from_native_session_id: session.native_session_id.clone(),
            candidate_native_session_id: None,
            state: "starting".to_string(),
            handoff: handoff.clone(),
            observed_bytes,
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut compact_run = StoredDurableChatRun::new(
            compact_run_id.clone(),
            Some(user_id.to_string()),
            session_id.to_string(),
            Provider::Codex.as_str(),
            handoff.clone(),
            session.project_path.clone(),
        );
        compact_run.model = failed_run.model.clone().or(session.model.clone());
        compact_run.effort = failed_run.effort.clone().or(session.effort.clone());
        compact_run.mode = failed_run.mode.clone().or(session.mode.clone());
        compact_run.thinking = failed_run.thinking.or(session.thinking);
        compact_run.fast = failed_run.fast.or(session.fast);
        compact_run.native_session_id = None;

        self.storage
            .replace_session_messages(session_id, &visible_messages)?;
        if !self
            .storage
            .prepare_context_rollover(&rollover, &compact_run)?
        {
            let existing = self
                .storage
                .context_rollover_for_request(user_id, session_id, request_id)?
                .ok_or_else(|| {
                    CoreError::Conflict("context rollover already exists".to_string())
                })?;
            if existing.kind != CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN {
                return Err(CoreError::Conflict(
                    "this compaction request id was already used for a different operation"
                        .to_string(),
                ));
            }
            if existing.failed_message_id != failed_message_id {
                return Err(CoreError::Conflict(
                    "this recovery request id was already used for a different failed message"
                        .to_string(),
                ));
            }
            return Ok(CompactSessionContextResponse {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                response_id: existing.retry_run_id,
                state: existing.state,
            });
        }
        self.sessions.set_active(session_id, true).await?;
        let runtime = session.runtime.unwrap_or(ChatRuntime::NativeCli);
        let attempt_id = new_id("attempt");
        self.storage
            .create_chat_run_attempt(&StoredChatRunAttempt::new(
                attempt_id.clone(),
                compact_run_id.clone(),
                session_id.to_string(),
                compact_run.user_message_id.clone(),
                Provider::Codex.as_str(),
                runtime_label(runtime),
                compact_run.model.clone(),
                None,
            ))?;
        let start_result = self
            .agents
            .start(AgentStartContext {
                provider: Provider::Codex,
                session_id: session_id.to_string(),
                durable_run_id: Some(compact_run_id.clone()),
                attempt_id: Some(attempt_id),
                response_id: compact_run_id.clone(),
                sequence: Arc::new(AtomicU64::new(0)),
                project_path: PathBuf::from(&session.project_path),
                prompt: handoff,
                model: compact_run.model.clone(),
                runtime,
                effort: compact_run.effort.clone(),
                mode: compact_run.mode.clone(),
                thinking: compact_run.thinking,
                fast: compact_run.fast,
                native_resume_session_id: None,
                context_rollover_id: Some(rollover_id.clone()),
                direct_ai_config,
                direct_ai_messages: Vec::new(),
                sessions: self.sessions.clone(),
                storage: self.storage.clone(),
                hub: self.ws_hub.clone(),
            })
            .await;
        if let Err(error) = start_result {
            let message = error.to_string();
            let _ = self.storage.fail_context_rollover(&rollover_id, &message);
            let _ = self
                .storage
                .mark_durable_chat_run_failed(&compact_run_id, &message);
            let _ = self.sessions.set_active(session_id, false).await;
            return Err(error);
        }

        Ok(CompactSessionContextResponse {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            response_id: compact_run_id,
            state: self
                .storage
                .context_rollover_for_retry_run(&rollover.retry_run_id)?
                .map(|stored| stored.state)
                .unwrap_or_else(|| "starting".to_string()),
        })
    }

    pub async fn compact_session_context(
        &self,
        user_id: &str,
        session_id: &str,
        request_id: &str,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
    ) -> Result<CompactSessionContextResponse> {
        if let Some(existing) = self
            .storage
            .context_rollover_for_request(user_id, session_id, request_id)?
        {
            if existing.kind != CONTEXT_ROLLOVER_KIND_MANUAL {
                return Err(CoreError::Conflict(
                    "this compaction request id was already used for a different operation"
                        .to_string(),
                ));
            }
            return Ok(CompactSessionContextResponse {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                response_id: existing.retry_run_id,
                state: existing.state,
            });
        }

        let session = self.sessions.get(session_id).await?;
        if session.provider != Provider::Codex {
            return Err(CoreError::InvalidInput(
                "manual context compaction is currently available only for Codex sessions"
                    .to_string(),
            ));
        }
        if session.active || self.agents.is_running(Provider::Codex, session_id).await {
            return Err(CoreError::Conflict(
                "stop the active response before compacting this chat".to_string(),
            ));
        }
        if self.context_recovery(session_id).await?.is_some() {
            return Err(CoreError::Conflict(
                "use Compact & retry to recover the latest failed turn before manual compaction"
                    .to_string(),
            ));
        }
        let native_session_id = self
            .codex_native_session_id_for_compaction(&session)
            .await?
            .ok_or_else(|| {
                CoreError::Conflict(
                    "codex_native_session_unavailable: this chat is not linked to a Codex thread"
                        .to_string(),
                )
            })?;

        let visible_messages = context_materialization_messages(
            session_id,
            self.sessions
                .messages_including_external(session_id)
                .await?,
            &[],
        );
        if !context_handoff_has_retainable_text(&visible_messages) {
            return Err(CoreError::InvalidInput(
                "there are no text messages to compact in this chat".to_string(),
            ));
        }
        let observed_bytes = self
            .sessions
            .native_rollout_size(session_id)
            .await
            .filter(|size| *size > 0);
        let handoff = "Native Codex context compaction".to_string();
        let rollover_id = new_id("rollover");
        let compact_run_id = new_id("run");
        let attempt_id = new_id("attempt");
        let now = Utc::now();
        let rollover = StoredSessionContextRollover {
            id: rollover_id.clone(),
            user_id: user_id.to_string(),
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            kind: CONTEXT_ROLLOVER_KIND_MANUAL.to_string(),
            failed_message_id: String::new(),
            trigger_run_id: compact_run_id.clone(),
            retry_run_id: compact_run_id.clone(),
            from_native_session_id: Some(native_session_id.clone()),
            candidate_native_session_id: Some(native_session_id.clone()),
            state: "starting".to_string(),
            handoff: handoff.clone(),
            observed_bytes,
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut compact_run = StoredDurableChatRun::new(
            compact_run_id.clone(),
            Some(user_id.to_string()),
            session_id.to_string(),
            Provider::Codex.as_str(),
            handoff.clone(),
            session.project_path.clone(),
        );
        compact_run.model = session.model.clone();
        compact_run.effort = session.effort.clone();
        compact_run.mode = session.mode.clone();
        compact_run.thinking = session.thinking;
        compact_run.fast = session.fast;
        compact_run.native_session_id = Some(native_session_id.clone());
        compact_run.auto_resume = false;
        let runtime = session.runtime.unwrap_or(ChatRuntime::NativeCli);
        let app_server_options =
            codex_app_server_launch_options(runtime, direct_ai_config.as_ref())?;

        self.storage
            .replace_session_messages(session_id, &visible_messages)?;
        if !self
            .storage
            .prepare_manual_context_rollover(&rollover, &compact_run)?
        {
            let existing = self
                .storage
                .context_rollover_for_request(user_id, session_id, request_id)?
                .ok_or_else(|| {
                    CoreError::Conflict("context rollover already exists".to_string())
                })?;
            if existing.kind != CONTEXT_ROLLOVER_KIND_MANUAL {
                return Err(CoreError::Conflict(
                    "this compaction request id was already used for a different operation"
                        .to_string(),
                ));
            }
            return Ok(CompactSessionContextResponse {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                response_id: existing.retry_run_id,
                state: existing.state,
            });
        }
        self.sessions.set_active(session_id, true).await?;
        self.storage
            .create_chat_run_attempt(&StoredChatRunAttempt::new(
                attempt_id.clone(),
                compact_run_id.clone(),
                session_id.to_string(),
                compact_run.user_message_id.clone(),
                Provider::Codex.as_str(),
                "codex_app_server",
                compact_run.model.clone(),
                Some(native_session_id.clone()),
            ))?;
        self.ws_hub.publish(WsServerEvent::SessionStatus {
            provider: Provider::Codex,
            session_id: session_id.to_string(),
            status: iowb_protocol::SessionRuntimeStatus::Starting,
            response_id: Some(compact_run_id.clone()),
            sequence: None,
            latest_user_prompt: None,
        });
        self.ws_hub.publish(WsServerEvent::SessionStatus {
            provider: Provider::Codex,
            session_id: session_id.to_string(),
            status: iowb_protocol::SessionRuntimeStatus::Running,
            response_id: Some(compact_run_id.clone()),
            sequence: None,
            latest_user_prompt: None,
        });
        self.ws_hub.publish(WsServerEvent::ActiveSessions {
            sessions: self.sessions.list_active().await,
        });
        let task = ManualContextCompactionTask {
            session,
            rollover_id: rollover_id.clone(),
            retry_run_id: compact_run_id.clone(),
            attempt_id,
            native_session_id,
            handoff,
            compact_run,
            runtime,
            app_server_options,
        };
        let state = self.clone();
        tokio::spawn(async move {
            state.run_manual_context_compaction(task).await;
        });

        Ok(CompactSessionContextResponse {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            response_id: compact_run_id,
            state: "starting".to_string(),
        })
    }

    async fn run_manual_context_compaction(&self, task: ManualContextCompactionTask) {
        let compact_result = {
            let _mutation = self.codex_app_server_mutation.lock().await;
            self.codex_app_server
                .compact_thread_and_wait_with_options(
                    &task.native_session_id,
                    task.app_server_options.as_ref(),
                )
                .await
        };
        if let Err(error) = compact_result {
            let message = error.to_string();
            let _ = self
                .storage
                .fail_context_rollover(&task.rollover_id, &message);
            let _ = self
                .storage
                .mark_durable_chat_run_failed(&task.retry_run_id, &message);
            let _ = self.sessions.set_active(&task.session.id, false).await;
            let _ = self.storage.finish_chat_run_attempt(
                &task.attempt_id,
                runtime_status_label(iowb_protocol::SessionRuntimeStatus::Failed),
                None,
                None,
                Some("codex_app_server"),
                TokenUsageCompleteness::Missing,
            );
            self.ws_hub.publish(WsServerEvent::Error {
                message: "Codex context compaction failed".to_string(),
                details: Some(message),
                session_id: Some(task.session.id.clone()),
            });
            self.ws_hub.publish(WsServerEvent::SessionStatus {
                provider: Provider::Codex,
                session_id: task.session.id.clone(),
                status: iowb_protocol::SessionRuntimeStatus::Failed,
                response_id: Some(task.retry_run_id.clone()),
                sequence: None,
                latest_user_prompt: None,
            });
            self.ws_hub.publish(WsServerEvent::ActiveSessions {
                sessions: self.sessions.list_active().await,
            });
            warn!(
                error = %error,
                session_id = %task.session.id,
                rollover_id = %task.rollover_id,
                "manual Codex context compaction failed"
            );
            return;
        }
        let mut session = task.session.clone();
        session.native_session_id = Some(task.native_session_id.clone());
        session.external = false;
        let context = AgentStartContext {
            provider: Provider::Codex,
            session_id: task.session.id.clone(),
            durable_run_id: Some(task.retry_run_id.clone()),
            attempt_id: Some(task.attempt_id.clone()),
            response_id: task.retry_run_id.clone(),
            sequence: Arc::new(AtomicU64::new(0)),
            project_path: PathBuf::from(&session.project_path),
            prompt: task.handoff.clone(),
            model: task.compact_run.model.clone(),
            runtime: task.runtime,
            effort: task.compact_run.effort.clone(),
            mode: task.compact_run.mode.clone(),
            thinking: task.compact_run.thinking,
            fast: task.compact_run.fast,
            native_resume_session_id: Some(task.native_session_id.clone()),
            context_rollover_id: Some(task.rollover_id.clone()),
            direct_ai_config: None,
            direct_ai_messages: Vec::new(),
            sessions: self.sessions.clone(),
            storage: self.storage.clone(),
            hub: self.ws_hub.clone(),
        };
        if let Err(error) =
            activate_completed_context_rollover(&context, &task.rollover_id, Utc::now()).await
        {
            let message = format!("failed to activate native Codex compaction: {error}");
            let _ = self
                .storage
                .fail_context_rollover(&task.rollover_id, &message);
            let _ = self
                .storage
                .mark_durable_chat_run_failed(&task.retry_run_id, &message);
            let _ = self.sessions.set_active(&task.session.id, false).await;
            let _ = self.storage.finish_chat_run_attempt(
                &task.attempt_id,
                runtime_status_label(iowb_protocol::SessionRuntimeStatus::Failed),
                None,
                None,
                Some("codex_app_server"),
                TokenUsageCompleteness::Missing,
            );
            self.ws_hub.publish(WsServerEvent::Error {
                message: "Codex context compaction could not be activated".to_string(),
                details: Some(message),
                session_id: Some(task.session.id.clone()),
            });
            self.ws_hub.publish(WsServerEvent::SessionStatus {
                provider: Provider::Codex,
                session_id: task.session.id.clone(),
                status: iowb_protocol::SessionRuntimeStatus::Failed,
                response_id: Some(task.retry_run_id.clone()),
                sequence: None,
                latest_user_prompt: None,
            });
            self.ws_hub.publish(WsServerEvent::ActiveSessions {
                sessions: self.sessions.list_active().await,
            });
            warn!(
                error = %error,
                session_id = %task.session.id,
                rollover_id = %task.rollover_id,
                "manual Codex context compaction activation failed"
            );
            return;
        }
        let _ = self.storage.finish_chat_run_attempt(
            &task.attempt_id,
            runtime_status_label(iowb_protocol::SessionRuntimeStatus::Completed),
            None,
            None,
            Some("codex_app_server"),
            TokenUsageCompleteness::Missing,
        );
        self.ws_hub.publish(WsServerEvent::SessionStatus {
            provider: Provider::Codex,
            session_id: task.session.id.clone(),
            status: iowb_protocol::SessionRuntimeStatus::Completed,
            response_id: Some(task.retry_run_id.clone()),
            sequence: None,
            latest_user_prompt: None,
        });
        self.ws_hub.publish(WsServerEvent::ActiveSessions {
            sessions: self.sessions.list_active().await,
        });
    }

    async fn codex_native_session_id_for_compaction(
        &self,
        session: &SessionSummary,
    ) -> Result<Option<String>> {
        if session.provider != Provider::Codex {
            return Ok(None);
        }
        if let Some(native_session_id) = session
            .native_session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(Some(native_session_id.to_string()));
        }
        if session.external {
            return Ok(Some(session.id.clone()));
        }
        self.sessions
            .infer_native_session_id(&session.id, Provider::Codex, &session.project_path)
            .await
    }

    pub async fn context_recovery(&self, session_id: &str) -> Result<Option<ChatContextRecovery>> {
        if let Some(rollover) = self.storage.latest_context_rollover(session_id)? {
            if rollover.kind != CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN {
                return Ok(None);
            }
            match rollover.state.as_str() {
                "starting" | "failed" => {
                    return Ok(Some(ChatContextRecovery {
                        code: "context_too_large".to_string(),
                        state: rollover.state,
                        message: rollover.error.unwrap_or_else(|| {
                            "This chat needs a clean native context before it can continue."
                                .to_string()
                        }),
                        failed_message_id: rollover.failed_message_id,
                        observed_bytes: rollover.observed_bytes,
                        limit_bytes: rollover.limit_bytes,
                        request_id: Some(rollover.request_id),
                    }));
                }
                // An active rollover is historical. A later turn in this same
                // visible chat may independently hit the gateway limit and
                // must be allowed to surface another recovery operation.
                _ => {}
            }
        }

        let Some(run) = self
            .storage
            .latest_durable_chat_run_for_session(session_id)?
        else {
            return Ok(None);
        };
        if run.status != "failed" || run.provider != Provider::Codex.as_str() {
            return Ok(None);
        }
        let Some(failed_message_id) = run.user_message_id else {
            return Ok(None);
        };
        let observed_bytes = self.sessions.native_rollout_size(session_id).await;
        let error = run.last_error.unwrap_or_default().to_ascii_lowercase();
        if observed_bytes.is_none_or(|bytes| bytes < CODEX_CONTEXT_ROLLOVER_THRESHOLD_BYTES)
            && !error.contains("invalid body")
            && !error.contains("413")
            && !error.contains("too large")
        {
            return Ok(None);
        }
        Ok(Some(ChatContextRecovery {
            code: "context_too_large".to_string(),
            state: "required".to_string(),
            message: "This chat's native context is too large to resume safely. Compact it into a clean context and retry the same message.".to_string(),
            failed_message_id,
            observed_bytes,
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            request_id: None,
        }))
    }

    pub async fn replay_agent_events(&self) -> Vec<WsServerEvent> {
        self.agents.replay_events().await
    }
}

#[derive(Clone)]
pub struct AuthManager {
    config: Arc<AppConfig>,
    storage: Storage,
}

impl AuthManager {
    pub fn new(config: Arc<AppConfig>, storage: Storage) -> Self {
        Self { config, storage }
    }

    pub fn status(&self, token: Option<&str>) -> Result<AuthStatusResponse> {
        let has_users = self.has_configured_user()?;
        let auth_enabled = self.should_enforce_auth()?;
        let auth_mode = self.auth_mode(has_users);
        let user = match token {
            Some(token) if !token.trim().is_empty() => self.authenticate_token(Some(token))?,
            _ if !auth_enabled => Some(self.local_user()?),
            _ => None,
        };
        let authenticated = user.is_some();
        Ok(AuthStatusResponse {
            enabled: auth_enabled,
            authenticated,
            needs_setup: auth_mode == "setup",
            is_authenticated: authenticated,
            auth_mode: auth_mode.to_string(),
            user,
        })
    }

    pub fn register(&self, username: &str, password: &str) -> Result<AuthTokenResponse> {
        if self.config.otp_secret.is_some() || self.config.local_token.is_some() {
            return Err(CoreError::Forbidden(
                "setup is disabled while token or OTP auth is configured".to_string(),
            ));
        }
        validate_credentials(username, password)?;
        if self.has_configured_user()? {
            return Err(CoreError::Forbidden(
                "user already exists; io-workbench is currently single-user".to_string(),
            ));
        }

        let password_hash = hash(password, DEFAULT_COST)
            .map_err(|error| CoreError::PasswordHash(error.to_string()))?;
        let user = self.storage.create_user(
            &format!("user_{}", Uuid::new_v4().simple()),
            username.trim(),
            &password_hash,
        )?;
        self.issue_token(&user)
    }

    pub fn login(&self, username: &str, password: &str) -> Result<AuthTokenResponse> {
        if let Some(secret) = self.config.otp_secret.as_deref() {
            if !verify_totp(secret, password.trim())? {
                return Err(CoreError::AuthenticationFailed);
            }
            let user = self.ensure_local_user()?;
            self.storage.update_last_login(&user.id)?;
            return self.issue_token(&user);
        }

        if !self.has_configured_user()? && self.config.local_token.is_some() {
            let expected = self
                .config
                .local_token
                .as_deref()
                .ok_or(CoreError::AuthenticationFailed)?;
            if password != expected {
                return Err(CoreError::AuthenticationFailed);
            }
            return Ok(AuthTokenResponse {
                success: true,
                token: expected.to_string(),
                user: self.local_user()?,
            });
        }

        let user = self
            .storage
            .get_user_by_username(username.trim())?
            .ok_or(CoreError::AuthenticationFailed)?;

        let password_ok = verify(password, &user.password_hash)
            .map_err(|error| CoreError::PasswordHash(error.to_string()))?;
        if !password_ok {
            return Err(CoreError::AuthenticationFailed);
        }

        self.storage.update_last_login(&user.id)?;
        self.issue_token(&user)
    }

    pub fn logout(&self, token: Option<&str>) -> Result<bool> {
        let Some(token) = token else {
            return Ok(false);
        };
        if self.config.local_token.as_deref() == Some(token) {
            return Ok(true);
        }
        self.storage
            .revoke_auth_token(&hash_secret_token(token))
            .map_err(CoreError::from)
    }

    pub fn authenticate_token(&self, token: Option<&str>) -> Result<Option<UserProfile>> {
        if !self.should_enforce_auth()? && token.is_none() {
            return Ok(Some(self.local_user()?));
        }

        let Some(token) = token.filter(|token| !token.trim().is_empty()) else {
            return Ok(None);
        };

        if self.config.local_token.as_deref() == Some(token) {
            return Ok(Some(
                self.storage
                    .get_first_user()?
                    .map(|user| user_to_profile(&user))
                    .unwrap_or(self.local_user()?),
            ));
        }

        Ok(self
            .storage
            .find_user_by_token_hash(&hash_secret_token(token))?
            .map(|user| user_to_profile(&user)))
    }

    pub fn should_enforce_auth(&self) -> Result<bool> {
        Ok(self.config.auth_required
            || self.config.local_token.is_some()
            || self.config.otp_secret.is_some())
    }

    pub fn require_user(&self, token: Option<&str>) -> Result<UserProfile> {
        self.authenticate_token(token)?
            .ok_or(CoreError::AuthenticationFailed)
    }

    fn issue_token(&self, user: &iowb_storage::StoredUser) -> Result<AuthTokenResponse> {
        let token = generate_secret_token("iowb");
        let expires_at = Utc::now() + chrono::Duration::days(7);
        self.storage
            .create_auth_token(&hash_secret_token(&token), &user.id, expires_at)?;

        Ok(AuthTokenResponse {
            success: true,
            token,
            user: user_to_profile(user),
        })
    }

    fn local_user(&self) -> Result<UserProfile> {
        Ok(user_to_profile(&self.ensure_local_user()?))
    }

    fn ensure_local_user(&self) -> Result<iowb_storage::StoredUser> {
        if let Some(user) = self.storage.get_user_by_id("local")? {
            return Ok(user);
        }

        let password_hash = hash(generate_secret_token("local").as_str(), DEFAULT_COST)
            .map_err(|error| CoreError::PasswordHash(error.to_string()))?;
        match self.storage.create_user("local", "local", &password_hash) {
            Ok(user) => Ok(user),
            Err(_) => self
                .storage
                .get_user_by_id("local")?
                .ok_or_else(|| CoreError::InvalidInput("failed to create local user".to_string())),
        }
    }

    fn has_configured_user(&self) -> Result<bool> {
        Ok(self.storage.has_non_local_user()?)
    }

    fn auth_mode(&self, has_users: bool) -> &'static str {
        if self.config.otp_secret.is_some() {
            "otp"
        } else if self.config.local_token.is_some() {
            "token"
        } else if self.config.auth_required && !has_users {
            "setup"
        } else if self.config.auth_required {
            "password"
        } else {
            "open"
        }
    }
}

#[derive(Clone)]
pub struct SessionManager {
    storage: Storage,
    sessions: Arc<RwLock<HashMap<String, SessionSummary>>>,
    board_session_ids: Arc<StdRwLock<HashSet<String>>>,
    max_sessions: usize,
    external_home: Arc<PathBuf>,
    external_cache: Arc<RwLock<ExternalSessionCache>>,
    external_sync: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Default)]
struct ExternalSessionCache {
    loaded_at: Option<Instant>,
    records: Vec<ExternalSessionRecord>,
    message_bytes: usize,
    messages: HashMap<String, CachedExternalMessages>,
}

#[derive(Clone)]
struct CachedExternalMessages {
    modified_at: Option<SystemTime>,
    estimated_bytes: usize,
    last_access: Instant,
    total_count: usize,
    complete: bool,
    messages: Arc<Vec<ChatMessage>>,
}

fn external_session_cache_key(record: &ExternalSessionRecord) -> String {
    format!(
        "{}:{}:{}",
        record.summary.provider.as_str(),
        record.summary.id.as_str(),
        record.file_path.display()
    )
}

impl SessionManager {
    pub fn load(storage: Storage, max_sessions: usize) -> Result<Self> {
        // Active sessions are reconciled against durable chat runs by the
        // server after AppState is fully initialized. Clearing them here would
        // destroy the only durable signal that a forced-stop recovery is due.
        let persisted_sessions = storage.list_sessions_including_board()?;
        let board_session_ids = persisted_sessions
            .iter()
            .filter(|session| session.board_session)
            .map(|session| session.id.clone())
            .collect();
        let sessions = persisted_sessions
            .into_iter()
            .take(max_sessions)
            .map(|session| (session.id.clone(), session))
            .collect();

        Ok(Self {
            storage,
            sessions: Arc::new(RwLock::new(sessions)),
            board_session_ids: Arc::new(StdRwLock::new(board_session_ids)),
            max_sessions,
            external_home: Arc::new(
                env_path("IO_WORKBENCH_CLI_HOME")
                    .or_else(dirs::home_dir)
                    .unwrap_or_default(),
            ),
            external_cache: Arc::new(RwLock::new(ExternalSessionCache::default())),
            external_sync: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    pub async fn mark_unrecovered_active_sessions_interrupted(
        &self,
        recovered_session_ids: &HashSet<String>,
    ) -> Result<Vec<SessionSummary>> {
        let now = Utc::now();
        let mut interrupted = Vec::new();
        let mut sessions = self.sessions.write().await;
        for session in sessions
            .values_mut()
            .filter(|session| session.active && !recovered_session_ids.contains(&session.id))
        {
            session.active = false;
            session.last_activity = now;
            session.last_message_at = Some(now);
            session.received_at = Some(now);
            session.message_count = session.message_count.saturating_add(1);
            self.storage.upsert_session(session)?;
            self.storage.append_message(
                &session.id,
                &ChatMessage {
                    id: new_id("msg"),
                    role: MessageRole::System,
                    content: "Server restarted before this response completed. The previous turn was marked interrupted; send another prompt to continue this chat."
                        .to_string(),
                    timestamp: now,
                    metadata: serde_json::json!({
                        "status": "interrupted",
                        "reason": "server_restart",
                        "receivedAt": now.to_rfc3339(),
                        "internalLogs": [
                            format!("{} WARN stale active session marked interrupted after server restart", now.to_rfc3339())
                        ],
                    }),
                },
            )?;
            warn!(
                session_id = %session.id,
                provider = session.provider.as_str(),
                "marked unrecovered active session interrupted after server restart"
            );
            interrupted.push(session.clone());
        }
        Ok(interrupted)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_or_update(
        &self,
        provider: Provider,
        project_path: impl Into<String>,
        session_id: Option<String>,
        external: bool,
        model: Option<String>,
        runtime: Option<ChatRuntime>,
        effort: Option<String>,
        mode: Option<String>,
        thinking: Option<bool>,
        fast: Option<bool>,
    ) -> Result<SessionSummary> {
        self.create_or_update_scoped(
            provider,
            project_path,
            session_id,
            external,
            model,
            runtime,
            effort,
            mode,
            thinking,
            fast,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_or_update_board(
        &self,
        provider: Provider,
        project_path: impl Into<String>,
        session_id: Option<String>,
        external: bool,
        model: Option<String>,
        runtime: Option<ChatRuntime>,
        effort: Option<String>,
        mode: Option<String>,
        thinking: Option<bool>,
        fast: Option<bool>,
        board_run_id: String,
        board_task_id: Option<String>,
    ) -> Result<SessionSummary> {
        self.create_or_update_scoped(
            provider,
            project_path,
            session_id,
            external,
            model,
            runtime,
            effort,
            mode,
            thinking,
            fast,
            Some((board_run_id, board_task_id)),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_or_update_scoped(
        &self,
        provider: Provider,
        project_path: impl Into<String>,
        session_id: Option<String>,
        external: bool,
        model: Option<String>,
        runtime: Option<ChatRuntime>,
        effort: Option<String>,
        mode: Option<String>,
        thinking: Option<bool>,
        fast: Option<bool>,
        board_scope: Option<(String, Option<String>)>,
    ) -> Result<SessionSummary> {
        let id = session_id.unwrap_or_else(|| new_id("session"));
        let now = Utc::now();
        let mut sessions = self.sessions.write().await;
        // A persisted row may have been evicted from the bounded in-memory
        // cache. Seed from storage so continuation never drops classification
        // metadata such as board ownership.
        if !sessions.contains_key(&id)
            && let Some(stored) = self.storage.get_session_summary(&id)?
        {
            sessions.insert(id.clone(), stored);
        }
        let session = sessions
            .entry(id.clone())
            .or_insert_with(|| SessionSummary {
                id: id.clone(),
                provider,
                external,
                board_session: false,
                board_run_id: None,
                board_task_id: None,
                project_path: project_path.into(),
                title: "New Session".to_string(),
                message_count: 0,
                last_activity: now,
                active: true,
                model: model.clone(),
                runtime,
                effort: effort.clone(),
                mode: mode.clone(),
                thinking,
                fast,
                last_message_at: None,
                first_user_at: None,
                received_at: None,
                token_usage: None,
                lifetime_token_usage: None,
                native_session_id: None,
                title_source: Some(SessionTitleSource::Prompt),
            });

        session.provider = provider;
        session.external = external;
        if let Some(model) = model {
            session.model = Some(model);
        }
        if let Some(runtime) = runtime {
            session.runtime = Some(runtime);
        }
        if let Some(effort) = effort {
            session.effort = Some(effort);
        }
        if let Some(mode) = mode {
            session.mode = Some(mode);
        }
        if let Some(thinking) = thinking {
            session.thinking = Some(thinking);
        }
        if let Some(fast) = fast {
            session.fast = Some(fast);
        }
        session.last_activity = now;
        session.active = true;
        session.token_usage = None;
        if let Some((board_run_id, board_task_id)) = board_scope {
            if board_run_id.trim().is_empty() {
                return Err(CoreError::InvalidInput(
                    "board run id must not be empty".to_string(),
                ));
            }
            session.board_session = true;
            session.board_run_id = Some(board_run_id);
            session.board_task_id = board_task_id.filter(|value| !value.trim().is_empty());
        }

        self.storage.upsert_session(session)?;
        if session.board_session {
            self.board_session_ids
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(id.clone());
        }
        let updated = session.clone();
        self.evict_if_needed(&mut sessions)?;
        Ok(updated)
    }

    pub async fn mark_board_session(
        &self,
        session_id: &str,
        board_run_id: impl Into<String>,
        board_task_id: Option<String>,
    ) -> Result<SessionSummary> {
        let board_run_id = board_run_id.into();
        if board_run_id.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "board run id must not be empty".to_string(),
            ));
        }
        let mut sessions = self.sessions.write().await;
        if !sessions.contains_key(session_id)
            && let Some(stored) = self.storage.get_session_summary(session_id)?
        {
            sessions.insert(session_id.to_string(), stored);
        }
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;
        session.board_session = true;
        session.board_run_id = Some(board_run_id);
        session.board_task_id = board_task_id.filter(|value| !value.trim().is_empty());
        self.storage.upsert_session(session)?;
        self.board_session_ids
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(session_id.to_string());
        Ok(session.clone())
    }

    pub async fn append_message(
        &self,
        session_id: &str,
        role: MessageRole,
        content: impl Into<String>,
    ) -> Result<ChatMessage> {
        self.append_message_with_metadata(session_id, role, content, None)
            .await
    }

    pub async fn append_message_with_metadata(
        &self,
        session_id: &str,
        role: MessageRole,
        content: impl Into<String>,
        metadata: Option<Value>,
    ) -> Result<ChatMessage> {
        let content = content.into();
        let message = ChatMessage {
            id: new_id("msg"),
            role,
            content,
            timestamp: Utc::now(),
            metadata: metadata.unwrap_or(Value::Null),
        };

        {
            let mut sessions = self.sessions.write().await;
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;
            session.message_count += 1;
            session.last_activity = message.timestamp;
            if role == MessageRole::User
                && session.title_source != Some(SessionTitleSource::Manual)
                && let Some(title) = session_title_from_prompt(&message.content)
            {
                session.title = title;
                session.title_source = Some(SessionTitleSource::Prompt);
            }
            self.storage.upsert_session(session)?;
        }

        self.storage.append_message(session_id, &message)?;
        Ok(message)
    }

    pub async fn append_user_message_with_durable_run(
        &self,
        session_id: &str,
        message: ChatMessage,
        run: &StoredDurableChatRun,
    ) -> Result<SessionSummary> {
        if message.role != MessageRole::User {
            return Err(CoreError::InvalidInput(
                "durable chat turn message must have the user role".to_string(),
            ));
        }
        if run.session_id != session_id || run.user_message_id.as_deref() != Some(&message.id) {
            return Err(CoreError::InvalidInput(
                "durable chat turn identity does not match its user message".to_string(),
            ));
        }

        let mut sessions = self.sessions.write().await;
        if !sessions.contains_key(session_id)
            && let Some(stored) = self.storage.get_session_summary(session_id)?
        {
            sessions.insert(session_id.to_string(), stored);
        }
        let current = sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;
        let mut updated = current;
        updated.message_count = updated.message_count.saturating_add(1);
        updated.last_activity = message.timestamp;
        updated.first_user_at.get_or_insert(message.timestamp);
        if updated.title_source != Some(SessionTitleSource::Manual)
            && let Some(title) = session_title_from_prompt(&message.content)
        {
            updated.title = title;
            updated.title_source = Some(SessionTitleSource::Prompt);
        }

        self.storage
            .create_durable_chat_turn(&updated, &message, run)?;
        sessions.insert(session_id.to_string(), updated.clone());
        self.evict_if_needed(&mut sessions)?;
        Ok(updated)
    }

    /// Patch the stored metadata for an existing message id. Returns
    /// `true` when the row was updated.
    pub fn update_message_metadata(
        &self,
        session_id: &str,
        message_id: &str,
        metadata: Value,
    ) -> Result<bool> {
        Ok(self
            .storage
            .update_message_metadata(session_id, message_id, metadata)?)
    }

    /// Stamp metadata onto the most recent message of a given role. Used by
    /// the chat flow to attach footer info (cli / model / sent / received /
    /// token usage / elapsed) onto the user prompt and assistant reply rows
    /// after the full chat-context is known.
    pub fn stamp_latest_message_metadata(
        &self,
        session_id: &str,
        role: MessageRole,
        metadata: Value,
    ) -> Result<bool> {
        let id = match role {
            MessageRole::User => self.storage.latest_user_message_id(session_id)?,
            MessageRole::Assistant => self.storage.latest_assistant_message_id(session_id)?,
            MessageRole::System | MessageRole::Tool => None,
        };
        let Some(id) = id else { return Ok(false) };
        Ok(self
            .storage
            .merge_message_metadata(session_id, &id, metadata)?)
    }

    pub async fn set_token_usage(
        &self,
        session_id: &str,
        token_usage: SessionTokenUsage,
    ) -> Result<SessionSummary> {
        let fallback = self.get(session_id).await?;
        let mut sessions = self.sessions.write().await;
        let session = sessions.entry(session_id.to_string()).or_insert(fallback);
        session.token_usage = Some(token_usage);
        self.storage.upsert_session(session)?;
        Ok(session.clone())
    }

    pub async fn set_active(&self, session_id: &str, active: bool) -> Result<SessionSummary> {
        let mut sessions = self.sessions.write().await;
        if !sessions.contains_key(session_id)
            && let Some(stored) = self.storage.get_session_summary(session_id)?
        {
            sessions.insert(session_id.to_string(), stored);
        }
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;
        session.active = active;
        session.last_activity = Utc::now();
        self.storage.upsert_session(session)?;
        Ok(session.clone())
    }

    pub async fn set_native_session_id(
        &self,
        session_id: &str,
        native_session_id: impl Into<String>,
    ) -> Result<SessionSummary> {
        let native_session_id = native_session_id.into();
        if native_session_id.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "native session id must not be empty".to_string(),
            ));
        }

        let mut sessions = self.sessions.write().await;
        if !sessions.contains_key(session_id)
            && let Some(stored) = self.storage.get_session_summary(session_id)?
        {
            sessions.insert(session_id.to_string(), stored);
        }
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;
        if session.native_session_id.as_deref() != Some(native_session_id.as_str()) {
            session.native_session_id = Some(native_session_id);
            self.storage.upsert_session(session)?;
        }
        Ok(session.clone())
    }

    async fn infer_native_session_id(
        &self,
        session_id: &str,
        provider: Provider,
        project_path: &str,
    ) -> Result<Option<String>> {
        let Some(last_user_prompt) = self
            .storage
            .list_messages(session_id)?
            .into_iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .map(|message| message.content)
            .filter(|content| !content.trim().is_empty())
        else {
            return Ok(None);
        };

        let records = self
            .external_records()
            .await
            .into_iter()
            .filter(|record| {
                record.summary.provider == provider
                    && same_project_path(&record.summary.project_path, project_path)
            })
            .collect::<Vec<_>>();
        let mut candidate: Option<ExternalSessionRecord> = None;
        for record in records {
            let messages = self.external_messages(&record).await;
            if messages.iter().any(|message| {
                message.role == MessageRole::User && message.content == last_user_prompt
            }) && candidate.as_ref().is_none_or(|existing| {
                existing.summary.last_activity < record.summary.last_activity
            }) {
                candidate = Some(record);
            }
        }

        let Some(candidate) = candidate else {
            return Ok(None);
        };
        let native_session_id = candidate.summary.id;
        self.set_native_session_id(session_id, native_session_id.clone())
            .await?;
        info!(
            session_id,
            native_session_id = %native_session_id,
            provider = provider.as_str(),
            "reconciled existing workbench session with native provider thread"
        );
        Ok(Some(native_session_id))
    }

    pub async fn list_active(&self) -> Vec<SessionSummary> {
        let mut sessions: Vec<_> = {
            self.sessions
                .read()
                .await
                .values()
                .filter(|session| session.active && !session.board_session)
                .cloned()
                .collect()
        };
        for session in &mut sessions {
            if let Err(error) = self.refresh_summary_message_count(session).await {
                warn!(
                    error = %error,
                    session_id = %session.id,
                    "failed to refresh active session message count"
                );
            }
        }
        sessions
    }

    pub async fn list_for_project(&self, project_path: &str) -> Result<Vec<SessionSummary>> {
        let mut sessions = self.storage.list_sessions_for_project(project_path)?;
        let active = self.sessions.read().await;
        for session in &mut sessions {
            session.active = active
                .get(&session.id)
                .map(|entry| entry.active)
                .unwrap_or(session.active);
        }
        drop(active);

        for record in self.external_records().await {
            if !same_project_path(&record.summary.project_path, project_path) {
                continue;
            }
            let mut external_summary = record.summary.clone();
            external_summary.message_count = self.external_record_message_count(&record).await;
            if let Some(existing) = sessions.iter_mut().find(|session| {
                session.id == external_summary.id && session.provider == external_summary.provider
            }) {
                if existing.external {
                    let active = existing.active;
                    let title = existing.title.clone();
                    let title_source = existing.title_source;
                    let preserve_local_title = matches!(
                        title_source,
                        Some(SessionTitleSource::Prompt | SessionTitleSource::Manual)
                    ) || (title_source.is_none()
                        && title != "New Session");
                    let model = existing.model.clone().or(record.summary.model.clone());
                    let effort = existing.effort.clone();
                    let mode = existing.mode.clone();
                    let thinking = existing.thinking;
                    let token_usage = existing.token_usage.clone();
                    let lifetime_token_usage = existing.lifetime_token_usage.clone();
                    *existing = external_summary;
                    existing.active = active;
                    if preserve_local_title {
                        existing.title = title;
                        existing.title_source = title_source;
                    }
                    existing.model = model;
                    existing.effort = effort;
                    existing.mode = mode;
                    existing.thinking = thinking;
                    existing.token_usage = token_usage;
                    existing.lifetime_token_usage = lifetime_token_usage;
                }
            } else {
                sessions.push(external_summary);
            }
        }
        for session in &mut sessions {
            self.refresh_summary_message_count(session).await?;
        }
        sessions.sort_by_key(|session| std::cmp::Reverse(session.last_activity));
        Ok(sessions)
    }

    async fn refresh_summary_message_count(&self, session: &mut SessionSummary) -> Result<()> {
        let stored_summary_count = session.message_count;
        let loaded_count = if session.external {
            if let Some(record) = self
                .external_record(
                    &session.id,
                    Some(session.provider),
                    Some(&session.project_path),
                )
                .await
            {
                Some(self.external_record_message_count(&record).await)
            } else {
                None
            }
        } else if session.provider == Provider::Codex
            && session.native_session_id.is_some()
            && !self.storage.has_active_context_rollover(&session.id)?
        {
            if let Some(record) = self.external_record_for_messages(&session.id).await {
                self.cached_external_record_message_count(&record)
                    .await
                    .or_else(|| {
                        (record.summary.message_count > 1).then_some(record.summary.message_count)
                    })
            } else {
                None
            }
        } else {
            None
        };
        if let Some(loaded_count) = loaded_count {
            session.message_count = if session.active {
                stored_summary_count.max(loaded_count)
            } else {
                loaded_count
            };
        }
        Ok(())
    }

    async fn external_record_message_count(&self, record: &ExternalSessionRecord) -> usize {
        self.cached_external_record_message_count(record)
            .await
            .unwrap_or(record.summary.message_count)
    }

    async fn cached_external_record_message_count(
        &self,
        record: &ExternalSessionRecord,
    ) -> Option<usize> {
        let key = external_session_cache_key(record);
        let modified_at = std::fs::metadata(&record.file_path)
            .and_then(|metadata| metadata.modified())
            .ok();
        self.external_cache
            .read()
            .await
            .messages
            .get(&key)
            .filter(|cached| cached.modified_at == modified_at)
            .map(|cached| cached.total_count)
    }

    pub fn messages(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        Ok(self.storage.list_messages(session_id)?)
    }

    pub async fn messages_including_external(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        if self.storage.has_active_context_rollover(session_id)? {
            return self
                .active_context_messages_including_external(session_id)
                .await;
        }
        if let Some(messages) = self.external_messages_for_session(session_id).await? {
            return Ok(messages);
        }
        self.messages(session_id)
    }

    async fn sync_codex_turn_to_native_rollout(
        &self,
        session_id: &str,
        prompt: &str,
        assistant_output: &str,
    ) -> Result<bool> {
        let prompt = prompt.trim();
        let assistant_output = assistant_output.trim();
        if prompt.is_empty() || assistant_output.is_empty() {
            return Ok(false);
        }
        if looks_like_codex_live_transcript(assistant_output) {
            warn!(
                session_id,
                assistant_bytes = assistant_output.len(),
                "refused to append a Codex live transcript to the native rollout"
            );
            return Ok(false);
        }

        let Some(record) = self.external_record_for_messages(session_id).await else {
            return Ok(false);
        };
        if record.summary.provider != Provider::Codex {
            return Ok(false);
        }

        let messages = load_external_messages(&record);
        let matching_prompt_index = messages.iter().rposition(|message| {
            message.role == MessageRole::User && message.content.trim() == prompt
        });
        let has_prompt = matching_prompt_index.is_some();
        let has_assistant_after_prompt = matching_prompt_index.is_some_and(|prompt_index| {
            messages[prompt_index + 1..]
                .iter()
                .take_while(|message| message.role != MessageRole::User)
                .any(is_codex_assistant_response)
        });
        if has_assistant_after_prompt {
            return Ok(false);
        }

        let now = Utc::now();
        let mut entries = Vec::new();
        if !has_prompt {
            entries.push(codex_rollout_user_message(now, prompt));
        }
        if !has_assistant_after_prompt {
            entries.push(codex_rollout_assistant_message(
                now + chrono::Duration::milliseconds(entries.len() as i64),
                assistant_output,
            ));
        }
        append_codex_rollout_entries(&record.file_path, &entries)?;

        {
            let mut cache = self.external_cache.write().await;
            cache.loaded_at = None;
            if let Some(stale) = cache.messages.remove(&external_session_cache_key(&record)) {
                cache.message_bytes = cache.message_bytes.saturating_sub(stale.estimated_bytes);
            }
        }

        info!(
            session_id,
            native_session_id = %record.summary.id,
            path = %record.file_path.display(),
            appended = entries.len(),
            "synced Workbench Codex turn into native rollout"
        );
        Ok(true)
    }

    /// Return a window of the oldest messages for `session_id`. Use
    /// `(limit, offset)` for "load older" lazy loading.
    pub fn messages_page(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<ChatMessage>, usize)> {
        Ok(self
            .storage
            .list_messages_page(session_id, limit.clamp(1, 500), offset)?)
    }

    pub async fn messages_page_including_external(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<ChatMessage>, usize)> {
        if self.storage.has_active_context_rollover(session_id)? {
            let messages = self
                .active_context_messages_including_external(session_id)
                .await?;
            let total = messages.len();
            let start = offset.min(total);
            let end = start.saturating_add(limit.clamp(1, 500)).min(total);
            return Ok((messages[start..end].to_vec(), total));
        }
        if let Some(messages) = self.external_messages_for_session(session_id).await? {
            let total = messages.len();
            let start = offset.min(total);
            let end = start.saturating_add(limit.clamp(1, 500)).min(total);
            return Ok((messages[start..end].to_vec(), total));
        }
        self.messages_page(session_id, limit, offset)
    }

    pub async fn messages_tail_including_external(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<(Vec<ChatMessage>, usize)> {
        let limit = limit.clamp(1, 500);
        if self.storage.has_active_context_rollover(session_id)? {
            let messages = self
                .active_context_messages_including_external(session_id)
                .await?;
            let total = messages.len();
            let start = total.saturating_sub(limit);
            return Ok((messages[start..].to_vec(), total));
        }
        if let Some((messages, total)) = self
            .external_messages_tail_for_session(session_id, limit)
            .await?
        {
            return Ok((messages, total));
        }
        let (_, total) = self.messages_page(session_id, 1, 0)?;
        let start = total.saturating_sub(limit);
        self.messages_page(session_id, limit, start)
    }

    pub async fn user_prompts_page_including_external(
        &self,
        session_id: &str,
        limit: usize,
        before: Option<PromptHistoryCursor>,
    ) -> Result<(Vec<PromptHistoryEntry>, bool)> {
        let limit = limit.clamp(1, 500);
        if self.storage.has_active_context_rollover(session_id)? {
            return Ok(self
                .storage
                .list_user_prompts_page(session_id, limit, before.as_ref())?);
        }
        if let Some(messages) = self.external_messages_for_session(session_id).await? {
            let mut prompts = messages
                .into_iter()
                .filter(|message| message.role == MessageRole::User)
                .map(|message| PromptHistoryEntry {
                    id: message.id,
                    content: message.content,
                    timestamp: message.timestamp,
                })
                .collect::<Vec<_>>();
            if let Some(cursor) = before {
                prompts.retain(|prompt| {
                    prompt.timestamp < cursor.timestamp
                        || (prompt.timestamp == cursor.timestamp && prompt.id < cursor.id)
                });
            }
            let start = prompts.len().saturating_sub(limit);
            let has_more = start > 0;
            return Ok((prompts[start..].to_vec(), has_more));
        }
        Ok(self
            .storage
            .list_user_prompts_page(session_id, limit, before.as_ref())?)
    }

    pub async fn get(&self, session_id: &str) -> Result<SessionSummary> {
        if let Some(mut session) = self.sessions.read().await.get(session_id).cloned() {
            self.refresh_summary_message_count(&mut session).await?;
            return Ok(session);
        }

        if let Some(mut session) = self.storage.get_session(session_id)? {
            self.refresh_summary_message_count(&mut session).await?;
            return Ok(session);
        }
        if let Some(record) = self.external_record(session_id, None, None).await {
            let mut session = record.summary.clone();
            session.message_count = self.external_record_message_count(&record).await;
            return Ok(session);
        }
        Err(CoreError::SessionNotFound(session_id.to_string()))
    }

    pub async fn remember_persisted_session(&self, session: SessionSummary) -> Result<()> {
        if session.board_session {
            self.board_session_ids
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(session.id.clone());
        }
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.id.clone(), session);
        self.evict_if_needed(&mut sessions)
    }

    /// Board visibility is checked for every streamed WebSocket event. Keep
    /// this lookup entirely in memory so output chunks never contend on the
    /// single SQLite connection.
    pub fn is_board_session_cached(&self, session_id: &str) -> bool {
        self.board_session_ids
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(session_id)
    }

    fn get_stored(&self, session_id: &str) -> Option<SessionSummary> {
        self.storage.get_session_summary(session_id).ok().flatten()
    }

    async fn external_records(&self) -> Vec<ExternalSessionRecord> {
        const CACHE_TTL: Duration = Duration::from_secs(30);
        let cached = {
            let cache = self.external_cache.read().await;
            cache
                .loaded_at
                .is_some_and(|loaded_at| loaded_at.elapsed() < CACHE_TTL)
                .then(|| cache.records.clone())
        };
        let mut records = if let Some(records) = cached {
            records
        } else {
            // Only one request may refresh provider indexes at a time. Recheck
            // after acquiring the guard so concurrent mobile/web connections
            // reuse the first completed synchronization.
            let _sync_guard = self.external_sync.lock().await;
            let refreshed = {
                let cache = self.external_cache.read().await;
                cache
                    .loaded_at
                    .is_some_and(|loaded_at| loaded_at.elapsed() < CACHE_TTL)
                    .then(|| cache.records.clone())
            };
            if let Some(records) = refreshed {
                records
            } else {
                let stale_records = self.external_cache.read().await.records.clone();
                let external_home = self.external_home.clone();
                let storage = self.storage.clone();
                let records = match tokio::task::spawn_blocking(move || {
                    sync_external_sessions(&external_home, &storage)
                })
                .await
                {
                    Ok(Ok(records)) => records,
                    Ok(Err(error)) => {
                        warn!(%error, "external session synchronization failed");
                        stale_records
                    }
                    Err(error) => {
                        warn!(%error, "external session discovery worker failed");
                        stale_records
                    }
                };
                let mut cache = self.external_cache.write().await;
                cache.loaded_at = Some(Instant::now());
                cache.records = records.clone();
                records
            }
        };

        {
            let cache = self.external_cache.read().await;
            for record in &mut records {
                let key = external_session_cache_key(record);
                let modified_at = std::fs::metadata(&record.file_path)
                    .and_then(|metadata| metadata.modified())
                    .ok();
                if let Some(cached) = cache
                    .messages
                    .get(&key)
                    .filter(|cached| cached.modified_at == modified_at)
                {
                    record.summary.message_count = cached.total_count;
                }
            }
        }

        let mut mapped_native_ids = match self.storage.list_internal_native_session_ids() {
            Ok(session_ids) => session_ids.into_iter().collect::<HashSet<_>>(),
            Err(error) => {
                warn!(%error, "failed to load persisted native session mappings");
                HashSet::new()
            }
        };
        mapped_native_ids.extend(
            self.sessions
                .read()
                .await
                .values()
                .filter(|session| !session.external)
                .filter_map(|session| session.native_session_id.clone()),
        );
        let deleted_sessions = match self.storage.list_deleted_sessions() {
            Ok(sessions) => sessions.into_iter().collect::<HashSet<_>>(),
            Err(error) => {
                warn!(%error, "failed to load deleted external session tombstones");
                HashSet::new()
            }
        };
        let replaced_source_ids = match self.storage.list_replaced_source_session_ids() {
            Ok(session_ids) => session_ids.into_iter().collect::<HashSet<_>>(),
            Err(error) => {
                warn!(%error, "failed to load replaced source session ids");
                HashSet::new()
            }
        };
        records
            .into_iter()
            .filter(|record| !mapped_native_ids.contains(&record.summary.id))
            .filter(|record| !replaced_source_ids.contains(&record.summary.id))
            .filter(|record| {
                !deleted_sessions.contains(&(record.summary.provider, record.summary.id.clone()))
            })
            .collect()
    }

    async fn external_record(
        &self,
        session_id: &str,
        provider: Option<Provider>,
        project_path: Option<&str>,
    ) -> Option<ExternalSessionRecord> {
        self.external_records().await.into_iter().find(|record| {
            record.summary.id == session_id
                && provider.is_none_or(|provider| record.summary.provider == provider)
                && project_path.is_none_or(|project_path| {
                    same_project_path(&record.summary.project_path, project_path)
                })
        })
    }

    async fn external_record_for_messages(
        &self,
        session_id: &str,
    ) -> Option<ExternalSessionRecord> {
        if let Some(record) = self.external_record(session_id, None, None).await {
            return Some(record);
        }

        let session = self
            .sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .or_else(|| self.storage.get_session_summary(session_id).ok().flatten())?;
        if session.provider != Provider::Codex {
            return None;
        }
        let native_session_id = session.native_session_id.as_deref()?;

        // Populate the discovery cache even though normal listing hides native
        // rollouts already mapped to a Workbench session.
        let _ = self.external_records().await;
        let find_record = || async {
            self.external_cache
                .read()
                .await
                .records
                .iter()
                .find(|record| {
                    record.summary.id == native_session_id
                        && record.summary.provider == session.provider
                        && same_project_path(&record.summary.project_path, &session.project_path)
                })
                .cloned()
        };
        if let Some(record) = find_record().await {
            return Some(record);
        }

        // The native rollout may have been created after the last discovery.
        // Force one refresh so a just-finished external continuation appears
        // immediately instead of waiting for the normal cache TTL.
        self.external_cache.write().await.loaded_at = None;
        let _ = self.external_records().await;
        find_record().await
    }

    async fn native_rollout_size(&self, session_id: &str) -> Option<u64> {
        let record = self.external_record_for_messages(session_id).await?;
        std::fs::metadata(record.file_path)
            .ok()
            .map(|metadata| metadata.len())
    }

    /// Resolve the provider-owned transcript through the existing discovery
    /// index/cache. Callers should use this instead of recursively walking the
    /// entire CLI history tree for a known session id.
    pub async fn external_session_file(&self, session_id: &str) -> Option<PathBuf> {
        self.external_record_for_messages(session_id)
            .await
            .map(|record| record.file_path)
    }

    async fn external_messages_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<Vec<ChatMessage>>> {
        let Some(record) = self.external_record_for_messages(session_id).await else {
            return Ok(None);
        };
        let external = self.external_messages(&record).await;
        if external.is_empty() {
            return Ok(None);
        }
        if record.summary.id == session_id {
            return Ok(Some(external.as_ref().clone()));
        }

        let stored = self.messages(session_id)?;
        Ok(Some(merge_mapped_external_messages(
            stored,
            external.as_ref().clone(),
        )))
    }

    async fn active_context_messages_including_external(
        &self,
        session_id: &str,
    ) -> Result<Vec<ChatMessage>> {
        let stored = sanitize_context_materialization_messages(self.messages(session_id)?);
        let compacted_at = match latest_context_compaction_marker_timestamp(&stored) {
            Some(compacted_at) => Some(compacted_at),
            None => self
                .storage
                .latest_context_rollover(session_id)?
                .filter(|rollover| rollover.state == "active")
                .and_then(|rollover| rollover.activated_at),
        };
        let Some(record) = self.external_record_for_messages(session_id).await else {
            return Ok(stored);
        };
        let external = self.external_messages(&record).await;
        if external.is_empty() {
            return Ok(stored);
        }
        Ok(merge_active_context_external_messages(
            stored,
            external.as_ref().clone(),
            compacted_at,
        ))
    }

    async fn external_messages_tail_for_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Option<(Vec<ChatMessage>, usize)>> {
        let Some(record) = self.external_record_for_messages(session_id).await else {
            return Ok(None);
        };
        let (external, external_total) = self.external_messages_tail(&record, limit).await;
        if external.is_empty() {
            return Ok(None);
        }
        if record.summary.id == session_id {
            return Ok(Some((external, external_total)));
        }

        let stored = self.messages(session_id)?;
        let stored_system_count = stored
            .iter()
            .filter(|message| message.role == MessageRole::System)
            .count();
        let merged = merge_mapped_external_messages(stored, external);
        let start = merged.len().saturating_sub(limit);
        Ok(Some((
            merged[start..].to_vec(),
            external_total.saturating_add(stored_system_count),
        )))
    }

    async fn external_messages_tail(
        &self,
        record: &ExternalSessionRecord,
        limit: usize,
    ) -> (Vec<ChatMessage>, usize) {
        let key = external_session_cache_key(record);
        let modified_at = std::fs::metadata(&record.file_path)
            .and_then(|metadata| metadata.modified())
            .ok();
        {
            let mut cache = self.external_cache.write().await;
            if let Some(cached) = cache.messages.get_mut(&key) {
                if cached.modified_at == modified_at {
                    cached.last_access = Instant::now();
                    let start = cached.messages.len().saturating_sub(limit);
                    return (cached.messages[start..].to_vec(), cached.total_count);
                }
                if let Some(stale) = cache.messages.remove(&key) {
                    cache.message_bytes = cache.message_bytes.saturating_sub(stale.estimated_bytes);
                }
            }
        }

        if let Some(fingerprint) = external_file_fingerprint(&record.file_path) {
            let file_path = record.file_path.display().to_string();
            let storage_fingerprint = ExternalHistoryFingerprint {
                file_identity: fingerprint.file_identity.as_deref(),
                file_size: fingerprint.file_size,
                modified_nanos: fingerprint.modified_nanos,
                parser_version: EXTERNAL_MESSAGE_PARSER_VERSION,
            };
            match self.storage.external_messages_tail_if_current(
                record.summary.provider,
                &record.summary.id,
                &file_path,
                &storage_fingerprint,
                limit,
            ) {
                Ok(Some(messages)) => return messages,
                Ok(None) => {}
                Err(error) => warn!(
                    %error,
                    session_id = %record.summary.id,
                    "failed to read persisted external message tail"
                ),
            }
        }

        let messages = self.external_messages(record).await;
        let total = messages.len();
        let start = total.saturating_sub(limit);
        (messages[start..].to_vec(), total)
    }

    async fn external_messages(&self, record: &ExternalSessionRecord) -> Arc<Vec<ChatMessage>> {
        let key = external_session_cache_key(record);
        let modified_at = std::fs::metadata(&record.file_path)
            .and_then(|metadata| metadata.modified())
            .ok();
        {
            let mut cache = self.external_cache.write().await;
            if let Some(cached) = cache.messages.get_mut(&key) {
                if cached.modified_at == modified_at && cached.complete {
                    cached.last_access = Instant::now();
                    return cached.messages.clone();
                } else if let Some(stale) = cache.messages.remove(&key) {
                    cache.message_bytes = cache.message_bytes.saturating_sub(stale.estimated_bytes);
                }
            }
        }

        let cache_warning_session_id = record.summary.id.clone();
        let cache_warning_provider = record.summary.provider;
        let fingerprint_before = external_file_fingerprint(&record.file_path);
        let file_path = record.file_path.display().to_string();
        let persisted = fingerprint_before.as_ref().and_then(|fingerprint| {
            let storage_fingerprint = ExternalHistoryFingerprint {
                file_identity: fingerprint.file_identity.as_deref(),
                file_size: fingerprint.file_size,
                modified_nanos: fingerprint.modified_nanos,
                parser_version: EXTERNAL_MESSAGE_PARSER_VERSION,
            };
            match self.storage.external_messages_if_current(
                record.summary.provider,
                &record.summary.id,
                &file_path,
                &storage_fingerprint,
            ) {
                Ok(messages) => messages,
                Err(error) => {
                    warn!(
                        %error,
                        session_id = %record.summary.id,
                        "failed to read persisted external messages"
                    );
                    None
                }
            }
        });
        let messages = if let Some(messages) = persisted {
            Arc::new(messages)
        } else {
            let parse_record = record.clone();
            let messages =
                match tokio::task::spawn_blocking(move || load_external_messages(&parse_record))
                    .await
                {
                    Ok(messages) => Arc::new(messages),
                    Err(error) => {
                        warn!(%error, "external session parser worker failed");
                        return Arc::new(Vec::new());
                    }
                };
            let fingerprint_after = external_file_fingerprint(&record.file_path);
            if fingerprint_before == fingerprint_after
                && let Some(fingerprint) = fingerprint_after.as_ref()
            {
                let storage_fingerprint = ExternalHistoryFingerprint {
                    file_identity: fingerprint.file_identity.as_deref(),
                    file_size: fingerprint.file_size,
                    modified_nanos: fingerprint.modified_nanos,
                    parser_version: EXTERNAL_MESSAGE_PARSER_VERSION,
                };
                if let Err(error) = self.storage.replace_external_messages(
                    record.summary.provider,
                    &record.summary.id,
                    &file_path,
                    &storage_fingerprint,
                    messages.as_ref(),
                ) {
                    warn!(
                        %error,
                        session_id = %record.summary.id,
                        "failed to persist external messages"
                    );
                }
            }
            messages
        };
        let estimated_bytes = estimate_external_messages_bytes(&messages);
        let total_count = messages.len();
        let mut cache = self.external_cache.write().await;
        if let Some(stale) = cache.messages.remove(&key) {
            cache.message_bytes = cache.message_bytes.saturating_sub(stale.estimated_bytes);
        }
        let (cached_messages, cached_bytes, complete) =
            if estimated_bytes <= EXTERNAL_MESSAGE_CACHE_MAX_BYTES {
                (messages.clone(), estimated_bytes, true)
            } else {
                let tail = bounded_external_message_tail(
                    &messages,
                    EXTERNAL_MESSAGE_TAIL_CACHE_MAX_MESSAGES,
                    EXTERNAL_MESSAGE_CACHE_MAX_BYTES,
                );
                let cached_bytes = estimate_external_messages_bytes(&tail);
                (Arc::new(tail), cached_bytes, false)
            };
        if cached_bytes <= EXTERNAL_MESSAGE_CACHE_MAX_BYTES {
            cache.messages.insert(
                key,
                CachedExternalMessages {
                    modified_at,
                    estimated_bytes: cached_bytes,
                    last_access: Instant::now(),
                    total_count,
                    complete,
                    messages: cached_messages,
                },
            );
            cache.message_bytes = cache.message_bytes.saturating_add(cached_bytes);
            evict_external_message_cache(&mut cache);
        }
        if !complete {
            warn!(
                session_id = %cache_warning_session_id,
                provider = cache_warning_provider.as_str(),
                estimated_bytes,
                max_bytes = EXTERNAL_MESSAGE_CACHE_MAX_BYTES,
                cached_tail_messages = total_count.min(EXTERNAL_MESSAGE_TAIL_CACHE_MAX_MESSAGES),
                "external session messages exceed full-cache budget; retained a bounded tail"
            );
        }
        messages
    }

    pub async fn update_model(
        &self,
        session_id: &str,
        model: Option<String>,
    ) -> Result<SessionSummary> {
        let mut sessions = self.sessions.write().await;
        let mut session = sessions
            .get(session_id)
            .cloned()
            .or_else(|| self.storage.get_session_summary(session_id).ok().flatten())
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        session.model = model;
        session.last_activity = Utc::now();
        self.storage.upsert_session(&session)?;
        sessions.insert(session.id.clone(), session.clone());
        self.evict_if_needed(&mut sessions)?;
        Ok(session)
    }

    pub async fn rename(&self, session_id: &str, title: String) -> Result<SessionSummary> {
        let mut sessions = self.sessions.write().await;
        let mut session = sessions
            .get(session_id)
            .cloned()
            .or_else(|| self.storage.get_session_summary(session_id).ok().flatten())
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        session.title = title;
        session.title_source = Some(SessionTitleSource::Manual);
        session.last_activity = Utc::now();
        self.storage.upsert_session(&session)?;
        sessions.insert(session.id.clone(), session.clone());
        self.evict_if_needed(&mut sessions)?;
        Ok(session)
    }

    pub async fn delete(&self, session_id: &str) -> Result<SessionSummary> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        }
        .or_else(|| self.storage.get_session_summary(session_id).ok().flatten());
        let session = match session {
            Some(session) => session,
            None => self
                .external_record(session_id, None, None)
                .await
                .map(|record| record.summary)
                .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?,
        };
        for native_session_id in self.storage.context_native_session_ids(session_id)? {
            self.storage
                .tombstone_session(&native_session_id, session.provider)?;
        }
        if session.external {
            self.storage
                .tombstone_session(session_id, session.provider)?;
        } else if self.storage.is_session_fork_destination(session_id)? {
            if let Some(native_session_id) = session.native_session_id.as_deref() {
                self.storage
                    .tombstone_session(native_session_id, session.provider)?;
            }
        }
        if !self.storage.delete_session(session_id)? {
            if !session.external {
                return Err(CoreError::SessionNotFound(session_id.to_string()));
            }
        }
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        self.board_session_ids
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session_id);
        Ok(session)
    }

    fn evict_if_needed(&self, sessions: &mut HashMap<String, SessionSummary>) -> Result<()> {
        while sessions.len() > self.max_sessions {
            if let Some(oldest_id) = sessions
                .values()
                .min_by_key(|session| session.last_activity)
                .map(|session| session.id.clone())
            {
                sessions.remove(&oldest_id);
            } else {
                break;
            }
        }
        Ok(())
    }
}

fn estimate_external_messages_bytes(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .map(|message| {
            std::mem::size_of::<ChatMessage>()
                .saturating_add(message.id.len())
                .saturating_add(message.content.len())
                .saturating_add(estimate_json_value_bytes(&message.metadata))
        })
        .sum()
}

fn bounded_external_message_tail(
    messages: &[ChatMessage],
    max_messages: usize,
    max_bytes: usize,
) -> Vec<ChatMessage> {
    let mut tail = Vec::new();
    let mut estimated_bytes = 0usize;
    for message in messages.iter().rev().take(max_messages) {
        let message_bytes = estimate_external_messages_bytes(std::slice::from_ref(message));
        if !tail.is_empty() && estimated_bytes.saturating_add(message_bytes) > max_bytes {
            break;
        }
        estimated_bytes = estimated_bytes.saturating_add(message_bytes);
        tail.push(message.clone());
    }
    tail.reverse();
    tail
}

fn estimate_json_value_bytes(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => std::mem::size_of_val(value),
        Value::String(value) => value.len(),
        Value::Array(values) => values.iter().map(estimate_json_value_bytes).sum(),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| key.len().saturating_add(estimate_json_value_bytes(value)))
            .sum(),
    }
}

fn evict_external_message_cache(cache: &mut ExternalSessionCache) {
    while cache.messages.len() > EXTERNAL_MESSAGE_CACHE_MAX_ENTRIES
        || cache.message_bytes > EXTERNAL_MESSAGE_CACHE_MAX_BYTES
    {
        let Some(key) = cache
            .messages
            .iter()
            .min_by_key(|(_, cached)| cached.last_access)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        if let Some(removed) = cache.messages.remove(&key) {
            cache.message_bytes = cache.message_bytes.saturating_sub(removed.estimated_bytes);
        }
    }
}

fn merge_mapped_external_messages(
    stored: Vec<ChatMessage>,
    mut external: Vec<ChatMessage>,
) -> Vec<ChatMessage> {
    let mut matched_stored = vec![false; stored.len()];
    let stored_keys = stored.iter().map(message_match_key).collect::<Vec<_>>();
    let external_keys = external.iter().map(message_match_key).collect::<Vec<_>>();
    for (stored_index, external_index) in ordered_text_matches(&stored_keys, &external_keys) {
        let stored_message = &stored[stored_index];
        let external_message = &mut external[external_index];
        matched_stored[stored_index] = true;
        external_message.id = stored_message.id.clone();
        if let (Some(external_metadata), Some(stored_metadata)) = (
            external_message.metadata.as_object_mut(),
            stored_message.metadata.as_object(),
        ) {
            external_metadata.extend(stored_metadata.clone());
            external_metadata.insert("external".to_string(), Value::Bool(true));
        }
    }

    external.extend(
        stored
            .into_iter()
            .enumerate()
            .filter(|(index, message)| {
                !matched_stored[*index] && message.role == MessageRole::System
            })
            .map(|(_, message)| message),
    );
    external.sort_by(|left, right| left.timestamp.cmp(&right.timestamp));
    external
}

fn merge_active_context_external_messages(
    stored: Vec<ChatMessage>,
    external: Vec<ChatMessage>,
    compacted_at: Option<DateTime<Utc>>,
) -> Vec<ChatMessage> {
    let Some(compacted_at) = compacted_at else {
        return stored;
    };
    let mut matched_external = vec![false; external.len()];
    let stored_keys = stored.iter().map(message_match_key).collect::<Vec<_>>();
    let external_keys = external.iter().map(message_match_key).collect::<Vec<_>>();
    for (_, external_index) in ordered_text_matches(&stored_keys, &external_keys) {
        matched_external[external_index] = true;
    }

    let mut merged = stored;
    merged.extend(
        external
            .into_iter()
            .enumerate()
            .filter(|(index, message)| {
                !matched_external[*index]
                    && should_import_active_context_external_message(message, compacted_at)
            })
            .map(|(_, message)| message),
    );
    merged.sort_by(|left, right| left.timestamp.cmp(&right.timestamp));
    merged
}

fn latest_context_compaction_marker_timestamp(messages: &[ChatMessage]) -> Option<DateTime<Utc>> {
    messages
        .iter()
        .filter(|message| {
            message.metadata.get("kind").and_then(Value::as_str) == Some("context_compaction")
                || message.content.starts_with("Context compacted here")
        })
        .map(|message| message.timestamp)
        .max()
}

fn should_import_active_context_external_message(
    message: &ChatMessage,
    compacted_at: DateTime<Utc>,
) -> bool {
    if message.timestamp <= compacted_at || is_context_rollover_setup_message(message) {
        return false;
    }
    true
}

fn is_context_rollover_setup_message(message: &ChatMessage) -> bool {
    let content = message.content.trim();
    if content.eq_ignore_ascii_case("Context ready.")
        || content.eq_ignore_ascii_case("Context ready")
    {
        return true;
    }
    content.contains("visible io-workbench chat is being moved into a clean native Codex context")
        || content.contains("Recent text-only handoff:")
}

fn message_match_key(message: &ChatMessage) -> String {
    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    format!("{role}\0{}", message.content.trim())
}

fn clone_forked_message(source_session_id: &str, source: &ChatMessage) -> ChatMessage {
    let mut metadata = source.metadata.as_object().cloned().unwrap_or_default();
    let usage_source_session_id = metadata
        .get("usageSourceSessionId")
        .and_then(Value::as_str)
        .unwrap_or(source_session_id)
        .to_string();
    let usage_source_message_id = metadata
        .get("usageSourceMessageId")
        .and_then(Value::as_str)
        .unwrap_or(&source.id)
        .to_string();
    metadata.insert(
        "forkedFromSessionId".to_string(),
        Value::String(source_session_id.to_string()),
    );
    metadata.insert(
        "forkedFromMessageId".to_string(),
        Value::String(source.id.clone()),
    );
    metadata.insert(
        "usageSourceSessionId".to_string(),
        Value::String(usage_source_session_id),
    );
    metadata.insert(
        "usageSourceMessageId".to_string(),
        Value::String(usage_source_message_id),
    );
    ChatMessage {
        id: new_id("msg"),
        role: source.role,
        content: source.content.clone(),
        timestamp: source.timestamp,
        metadata: Value::Object(metadata),
    }
}

fn normalized_fork_prompt(value: &str) -> String {
    let visible = visible_user_text(value);
    let selected = if visible.trim().is_empty() {
        value.trim()
    } else {
        visible.trim()
    };
    selected.replace("\r\n", "\n")
}

fn build_context_rollover_handoff(messages: Vec<ChatMessage>, failed_prompt: &str) -> String {
    let history = build_context_handoff_history(messages, Some(failed_prompt));
    format!(
        "<system-reminder>\nThe visible io-workbench chat is being moved into a clean native Codex context because its previous history exceeded the gateway body-size limit. The same Workbench chat and full visible transcript remain available to the user. Use the bounded text handoff below only to re-establish context. Do not answer the failed user request yet; a subsequent message will contain that request after this clean context is activated. Do not claim that old tool outputs or inline image bytes are present. Reopen only specific local image paths when genuinely needed, one at a time. Inspect current files before changing them and preserve existing unrelated work. Reply exactly: Context ready.\n\nRecent text-only handoff:\n{history}\n</system-reminder>"
    )
}

fn sanitize_context_materialization_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    messages
        .into_iter()
        .filter(|message| !is_persisted_codex_live_transcript_message(message))
        .collect()
}

fn context_materialization_messages(
    session_id: &str,
    messages: Vec<ChatMessage>,
    preserved_message_ids: &[&str],
) -> Vec<ChatMessage> {
    let preserved_message_ids = preserved_message_ids
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut used_message_ids = HashSet::new();
    sanitize_context_materialization_messages(messages)
        .into_iter()
        .map(|message| {
            if preserved_message_ids.contains(message.id.as_str())
                && used_message_ids.insert(message.id.clone())
            {
                return message;
            }
            clone_context_materialized_message(session_id, &message, &mut used_message_ids)
        })
        .collect()
}

fn clone_context_materialized_message(
    session_id: &str,
    source: &ChatMessage,
    used_message_ids: &mut HashSet<String>,
) -> ChatMessage {
    let source_message_id = source.id.clone();
    let mut metadata = source.metadata.as_object().cloned().unwrap_or_default();
    let usage_source_session_id = metadata
        .get("usageSourceSessionId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(session_id)
        .to_string();
    let usage_source_message_id = metadata
        .get("usageSourceMessageId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&source_message_id)
        .to_string();
    metadata.insert(
        "contextMaterializedFromSessionId".to_string(),
        Value::String(session_id.to_string()),
    );
    metadata.insert(
        "contextMaterializedFromMessageId".to_string(),
        Value::String(source_message_id),
    );
    metadata.insert(
        "usageSourceSessionId".to_string(),
        Value::String(usage_source_session_id),
    );
    metadata.insert(
        "usageSourceMessageId".to_string(),
        Value::String(usage_source_message_id),
    );

    let mut id = new_id("msg");
    while !used_message_ids.insert(id.clone()) {
        id = new_id("msg");
    }
    ChatMessage {
        id,
        role: source.role,
        content: source.content.clone(),
        timestamp: source.timestamp,
        metadata: Value::Object(metadata),
    }
}

fn is_persisted_codex_live_transcript_message(message: &ChatMessage) -> bool {
    if message.role != MessageRole::Assistant || !looks_like_codex_live_transcript(&message.content)
    {
        return false;
    }
    let providerish = message
        .metadata
        .get("provider")
        .or_else(|| message.metadata.get("cli"))
        .and_then(Value::as_str)
        .is_some_and(|provider| provider.eq_ignore_ascii_case("codex"));
    providerish
        || message.metadata.get("durableRunId").is_some()
        || message
            .metadata
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|source| source.eq_ignore_ascii_case("io-workbench"))
        || message.metadata.is_null()
}

fn build_context_handoff_history(
    messages: Vec<ChatMessage>,
    excluded_user_prompt: Option<&str>,
) -> String {
    let excluded_user_prompt = excluded_user_prompt.map(sanitize_context_handoff_text);
    let mut selected = Vec::<String>::new();
    let mut remaining = CONTEXT_ROLLOVER_HANDOFF_MAX_BYTES;
    for message in messages.into_iter().rev() {
        if !matches!(message.role, MessageRole::User | MessageRole::Assistant) {
            continue;
        }
        if message.role == MessageRole::Assistant
            && (message.metadata.get("kind").and_then(Value::as_str) == Some("thinking")
                || message.metadata.get("phase").and_then(Value::as_str) == Some("commentary"))
        {
            continue;
        }
        if message.role == MessageRole::User && message.id.is_empty() {
            continue;
        }
        let content = sanitize_context_handoff_text(&message.content);
        if content.is_empty()
            || (message.role == MessageRole::User
                && excluded_user_prompt.as_deref() == Some(content.as_str()))
        {
            continue;
        }
        let role = if message.role == MessageRole::User {
            "User"
        } else {
            "Assistant"
        };
        let entry = format!("{role}: {content}");
        if entry.len() + 2 > remaining {
            continue;
        }
        remaining -= entry.len() + 2;
        selected.push(entry);
        if selected.len() >= 24 {
            break;
        }
    }
    selected.reverse();
    if selected.is_empty() {
        "No earlier text messages were retained.".to_string()
    } else {
        selected.join("\n\n")
    }
}

fn context_handoff_has_retainable_text(messages: &[ChatMessage]) -> bool {
    messages.iter().any(|message| {
        if !matches!(message.role, MessageRole::User | MessageRole::Assistant) {
            return false;
        }
        if message.role == MessageRole::Assistant
            && (message.metadata.get("kind").and_then(Value::as_str) == Some("thinking")
                || message.metadata.get("phase").and_then(Value::as_str) == Some("commentary"))
        {
            return false;
        }
        !sanitize_context_handoff_text(&message.content).is_empty()
    })
}

fn sanitize_context_handoff_text(value: &str) -> String {
    let visible = normalized_fork_prompt(value);
    let mut output = String::with_capacity(visible.len().min(8 * 1024));
    let mut cursor = 0;
    while let Some(relative_start) = visible[cursor..].find("data:") {
        let start = cursor + relative_start;
        output.push_str(&visible[cursor..start]);
        let tail = &visible[start..];
        let Some(marker) = tail.find(";base64,") else {
            output.push_str("data:");
            cursor = start + "data:".len();
            continue;
        };
        let payload_start = start + marker + ";base64,".len();
        let payload_len = visible.as_bytes()[payload_start..]
            .iter()
            .take_while(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'+' | b'/' | b'=' | b'-' | b'_' | b'\r' | b'\n')
            })
            .count();
        if payload_len == 0 {
            output.push_str("[inline attachment omitted]");
            cursor = payload_start;
        } else {
            output.push_str("[inline image omitted; use its local file path if available]");
            cursor = payload_start + payload_len;
        }
    }
    output.push_str(&visible[cursor..]);
    bound_agent_text(&output, 8 * 1024, "handoff message")
        .trim()
        .to_string()
}

fn ordered_text_matches(left: &[String], right: &[String]) -> Vec<(usize, usize)> {
    if left.len().saturating_mul(right.len()) > ORDERED_TEXT_MATCH_MATRIX_MAX_CELLS {
        return ordered_text_matches_greedy(left, right);
    }
    let mut lengths = vec![vec![0usize; right.len() + 1]; left.len() + 1];
    for left_index in (0..left.len()).rev() {
        for right_index in (0..right.len()).rev() {
            lengths[left_index][right_index] = if left[left_index] == right[right_index] {
                lengths[left_index + 1][right_index + 1] + 1
            } else {
                lengths[left_index + 1][right_index].max(lengths[left_index][right_index + 1])
            };
        }
    }

    let mut matches = Vec::new();
    let (mut left_index, mut right_index) = (0usize, 0usize);
    while left_index < left.len() && right_index < right.len() {
        if left[left_index] == right[right_index] {
            matches.push((left_index, right_index));
            left_index += 1;
            right_index += 1;
        } else if lengths[left_index + 1][right_index] >= lengths[left_index][right_index + 1] {
            left_index += 1;
        } else {
            right_index += 1;
        }
    }
    matches
}

fn ordered_text_matches_greedy(left: &[String], right: &[String]) -> Vec<(usize, usize)> {
    let mut positions = HashMap::<&str, VecDeque<usize>>::new();
    for (index, value) in right.iter().enumerate() {
        positions
            .entry(value.as_str())
            .or_default()
            .push_back(index);
    }
    let mut next_right_index = 0usize;
    let mut matches = Vec::new();
    for (left_index, value) in left.iter().enumerate() {
        let Some(indices) = positions.get_mut(value.as_str()) else {
            continue;
        };
        while indices
            .front()
            .is_some_and(|index| *index < next_right_index)
        {
            indices.pop_front();
        }
        if let Some(right_index) = indices.pop_front() {
            matches.push((left_index, right_index));
            next_right_index = right_index.saturating_add(1);
        }
    }
    matches
}

fn ws_event_estimated_bytes(event: &WsServerEvent) -> usize {
    const ENVELOPE_BYTES: usize = 256;
    match event {
        WsServerEvent::Output { content, .. } => ENVELOPE_BYTES.saturating_add(content.len()),
        WsServerEvent::Error {
            message, details, ..
        } => ENVELOPE_BYTES
            .saturating_add(message.len())
            .saturating_add(details.as_deref().map_or(0, str::len)),
        WsServerEvent::SessionStatus {
            latest_user_prompt, ..
        } => ENVELOPE_BYTES.saturating_add(latest_user_prompt.as_deref().map_or(0, str::len)),
        WsServerEvent::ProjectFilesChanged { paths, .. } => {
            ENVELOPE_BYTES.saturating_add(paths.iter().map(String::len).sum::<usize>())
        }
        _ => 1024,
    }
}

#[derive(Clone)]
pub struct AgentRuntimeManager {
    runs: Arc<RwLock<HashMap<String, AgentRuntimeRecord>>>,
    max_runs: usize,
    max_replay_events: usize,
    max_replay_bytes: usize,
    max_output_bytes: usize,
}

struct AgentRuntimeRecord {
    replay: VecDeque<WsServerEvent>,
    replay_bytes: usize,
    abort_tx: Option<oneshot::Sender<()>>,
    last_activity: DateTime<Utc>,
}

#[derive(Clone)]
struct AgentStartContext {
    provider: Provider,
    session_id: String,
    durable_run_id: Option<String>,
    attempt_id: Option<String>,
    response_id: String,
    sequence: Arc<AtomicU64>,
    project_path: PathBuf,
    prompt: String,
    model: Option<String>,
    runtime: ChatRuntime,
    effort: Option<String>,
    mode: Option<String>,
    thinking: Option<bool>,
    fast: Option<bool>,
    native_resume_session_id: Option<String>,
    context_rollover_id: Option<String>,
    direct_ai_config: Option<DirectAiRuntimeConfig>,
    direct_ai_messages: Vec<DirectAiConversationMessage>,
    sessions: SessionManager,
    storage: iowb_storage::Storage,
    hub: WsHub,
}

#[derive(Clone)]
struct ManualContextCompactionTask {
    session: SessionSummary,
    rollover_id: String,
    retry_run_id: String,
    attempt_id: String,
    native_session_id: String,
    handoff: String,
    compact_run: StoredDurableChatRun,
    runtime: ChatRuntime,
    app_server_options: Option<CodexAppServerLaunchOptions>,
}

#[derive(Clone)]
struct ContextRolloverFollowUp {
    run: StoredDurableChatRun,
}

impl AgentStartContext {
    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }
}

#[derive(Debug, Clone)]
pub struct DirectAiRuntimeConfig {
    pub base_url: String,
    pub api_key: String,
    pub max_tokens: Option<u64>,
}

struct AgentCommandSpec {
    command: String,
    args: Vec<String>,
    stdin_prompt: bool,
    prompt: String,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum AgentOutputStream {
    Stdout,
    Stderr,
}

enum AgentProcessEvent {
    Output {
        stream: AgentOutputStream,
        data: String,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
struct CodexTurnError {
    message: String,
    code: Option<String>,
    limit_bytes: Option<u64>,
    observed_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
struct NormalizedRunUsage {
    usage: SessionTokenUsage,
    raw_usage_json: Option<String>,
    source: &'static str,
    completeness: TokenUsageCompleteness,
}

#[derive(Default)]
struct CodexLiveOutputNormalizer {
    pending_line: String,
    pending_agent_message: Option<String>,
    pending_thread_id: Option<String>,
    final_assistant_message: Option<String>,
    saw_structured_event: bool,
    tool_messages: Vec<NormalizedToolMessage>,
    tool_message_bytes: usize,
    last_error: Option<CodexTurnError>,
    final_usage: Option<NormalizedRunUsage>,
}

#[derive(Debug, Clone)]
struct NormalizedToolMessage {
    name: String,
    content: String,
}

#[derive(Default)]
struct GeminiLiveOutputNormalizer {
    pending_line: String,
    pending_session_id: Option<String>,
    final_usage: Option<NormalizedRunUsage>,
}

impl AgentRuntimeManager {
    pub fn new(max_runs: usize) -> Self {
        Self {
            runs: Arc::new(RwLock::new(HashMap::new())),
            max_runs,
            max_replay_events: 256,
            max_replay_bytes: AGENT_REPLAY_MAX_BYTES,
            max_output_bytes: AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
        }
    }

    fn start(
        &self,
        context: AgentStartContext,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let key = agent_run_key(context.provider, &context.session_id);
            let runtime_provider = if context.runtime == ChatRuntime::IoGateway {
                context.provider
            } else {
                effective_agent_command_provider(context.provider, context.model.as_deref())
            };
            if should_use_direct_ai_gateway_runtime(context.provider, context.model.as_deref()) {
                return self.start_direct_ai(context).await;
            }

            let mut command = match resolve_agent_command(
                runtime_provider,
                &context.prompt,
                &context.session_id,
                context.model.as_deref(),
                context.effort.as_deref(),
                context.mode.as_deref(),
                context.thinking,
                context.fast,
                context.native_resume_session_id.as_deref(),
                context.runtime,
            ) {
                Ok(command) => command,
                Err(error) => {
                    let error_message = error.to_string();
                    self.publish(
                        &context.hub,
                        &key,
                        WsServerEvent::Error {
                            message: "failed to prepare agent command".to_string(),
                            details: Some(error_message.clone()),
                            session_id: Some(context.session_id.clone()),
                        },
                    )
                    .await;
                    self.finish(
                        &key,
                        &context,
                        iowb_protocol::SessionRuntimeStatus::Failed,
                        Some(error_message),
                        None,
                    )
                    .await;
                    return Ok(());
                }
            };
            if context.runtime == ChatRuntime::IoGateway && runtime_provider == Provider::Codex {
                let Some(config) = context.direct_ai_config.as_ref() else {
                    return Err(CoreError::InvalidInput(
                        "IO Gateway is not configured for this session".to_string(),
                    ));
                };
                apply_codex_cli_io_gateway_args(&mut command.args, &config.base_url);
            }
            let (abort_tx, abort_rx) = oneshot::channel();

            self.register(key.clone(), abort_tx).await;

            self.publish(
                &context.hub,
                &key,
                WsServerEvent::SessionStatus {
                    provider: context.provider,
                    session_id: context.session_id.clone(),
                    status: iowb_protocol::SessionRuntimeStatus::Starting,
                    response_id: Some(context.response_id.clone()),
                    sequence: Some(context.next_sequence()),
                    latest_user_prompt: Some(context.prompt.clone()),
                },
            )
            .await;

            let mut child_command = Command::new(&command.command);
            child_command
                .args(&command.args)
                .current_dir(&context.project_path)
                .env("PATH", augmented_user_path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if context.runtime == ChatRuntime::IoGateway && runtime_provider == Provider::Claude {
                let Some(config) = context.direct_ai_config.as_ref() else {
                    let error_message =
                    "IO Gateway is not configured. Configure the IO Gateway URL and API key in Settings."
                        .to_string();
                    self.publish(
                        &context.hub,
                        &key,
                        WsServerEvent::Error {
                            message: "IO Gateway is not configured".to_string(),
                            details: Some(error_message.clone()),
                            session_id: Some(context.session_id.clone()),
                        },
                    )
                    .await;
                    self.finish(
                        &key,
                        &context,
                        iowb_protocol::SessionRuntimeStatus::Failed,
                        Some(error_message),
                        None,
                    )
                    .await;
                    return Ok(());
                };
                apply_claude_cli_io_gateway_env(&mut child_command, config);
            }
            if context.runtime == ChatRuntime::IoGateway && runtime_provider == Provider::Codex {
                let Some(config) = context.direct_ai_config.as_ref() else {
                    unreachable!("gateway configuration was validated above");
                };
                child_command.env(IO_WORKBENCH_GATEWAY_KEY_ENV, &config.api_key);
            }
            if let Some(run_id) = context.durable_run_id.as_deref() {
                // Descendants inherit these markers. The database scope prevents a
                // server opened on a copied database from targeting the original
                // run, while the process identity distinguishes a live owner from
                // a server process that has actually exited.
                child_command.env(DURABLE_AGENT_RUN_ENV, run_id).env(
                    DURABLE_AGENT_SCOPE_ENV,
                    durable_agent_run_scope(context.storage.path()),
                );
                #[cfg(target_os = "linux")]
                if let Some((owner_pid, owner_start)) = current_process_identity() {
                    child_command
                        .env(DURABLE_AGENT_OWNER_PID_ENV, owner_pid.to_string())
                        .env(DURABLE_AGENT_OWNER_START_ENV, owner_start.to_string());
                }
            }
            // Log the exact spawn so misconfigured flag sets are easy to spot.
            let rendered_cmd = std::iter::once(command.command.clone())
                .chain(command.args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ");
            info!(
                provider = context.provider.as_str(),
                session_id = %context.session_id,
                project = %context.project_path.display(),
                cmd = %rendered_cmd,
                "spawning agent command"
            );
            if command.stdin_prompt {
                child_command.stdin(Stdio::piped());
            } else {
                child_command.stdin(Stdio::null());
            }
            isolate_agent_process(&mut child_command);

            let mut child = match child_command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    let error_message = format!(
                        "failed to spawn agent provider: {}: {}",
                        command.command, error
                    );
                    self.publish(
                        &context.hub,
                        &key,
                        WsServerEvent::Error {
                            message: "failed to spawn agent provider".to_string(),
                            details: Some(error_message.clone()),
                            session_id: Some(context.session_id.clone()),
                        },
                    )
                    .await;
                    self.finish(
                        &key,
                        &context,
                        iowb_protocol::SessionRuntimeStatus::Failed,
                        Some(error_message),
                        None,
                    )
                    .await;
                    return Ok(());
                }
            };

            if command.stdin_prompt {
                if let Some(mut stdin) = child.stdin.take() {
                    let prompt = command.prompt.clone();
                    tokio::spawn(async move {
                        let _ = stdin.write_all(prompt.as_bytes()).await;
                        let _ = stdin.write_all(b"\n").await;
                        let _ = stdin.shutdown().await;
                    });
                }
            }

            let (output_tx, mut output_rx) = mpsc::channel::<AgentProcessEvent>(256);
            if let Some(stdout) = child.stdout.take() {
                spawn_agent_output_reader(output_tx.clone(), stdout, AgentOutputStream::Stdout);
            }
            if let Some(stderr) = child.stderr.take() {
                spawn_agent_output_reader(output_tx.clone(), stderr, AgentOutputStream::Stderr);
            }
            drop(output_tx);

            self.publish(
                &context.hub,
                &key,
                WsServerEvent::SessionStatus {
                    provider: context.provider,
                    session_id: context.session_id.clone(),
                    status: iowb_protocol::SessionRuntimeStatus::Running,
                    response_id: Some(context.response_id.clone()),
                    sequence: Some(context.next_sequence()),
                    latest_user_prompt: Some(context.prompt.clone()),
                },
            )
            .await;

            let manager = self.clone();
            tokio::spawn(async move {
                let mut abort_rx = abort_rx;
                let mut output = String::new();
                let mut codex_normalizer =
                    (runtime_provider == Provider::Codex).then(CodexLiveOutputNormalizer::default);
                let mut claude_normalizer = (runtime_provider == Provider::Claude)
                    .then(ClaudeLiveOutputNormalizer::default);
                let mut gemini_normalizer = (runtime_provider == Provider::Gemini)
                    .then(GeminiLiveOutputNormalizer::default);
                loop {
                    tokio::select! {
                        Some(event) = output_rx.recv() => {
                            process_agent_event(
                                &manager,
                                &context,
                                &key,
                                event,
                                &mut codex_normalizer,
                                &mut claude_normalizer,
                                &mut gemini_normalizer,
                                &mut output,
                            ).await;
                        }
                        status = child.wait() => {
                            while let Some(event) = output_rx.recv().await {
                                process_agent_event(
                                    &manager,
                                    &context,
                                    &key,
                                    event,
                                    &mut codex_normalizer,
                                    &mut claude_normalizer,
                                    &mut gemini_normalizer,
                                    &mut output,
                                ).await;
                            }
                            flush_codex_live_output(
                                &manager,
                                &context,
                                &key,
                                &mut codex_normalizer,
                                &mut output,
                            ).await;
                            flush_claude_live_output(
                                &manager,
                                &context,
                                &key,
                                &mut claude_normalizer,
                                &mut output,
                            ).await;
                            flush_gemini_live_output(
                                &manager,
                                &context,
                                &key,
                                &mut gemini_normalizer,
                                &mut output,
                            ).await;
                            let codex_saw_structured_event = codex_normalizer
                                .as_ref()
                                .is_some_and(CodexLiveOutputNormalizer::saw_structured_event);
                            let run_usage = codex_normalizer
                                .as_mut()
                                .and_then(CodexLiveOutputNormalizer::take_final_usage)
                                .or_else(|| {
                                    claude_normalizer
                                        .as_mut()
                                        .and_then(ClaudeLiveOutputNormalizer::take_final_usage)
                                })
                                .or_else(|| {
                                    gemini_normalizer
                                        .as_mut()
                                        .and_then(GeminiLiveOutputNormalizer::take_final_usage)
                                });
                            let codex_final_assistant = codex_normalizer
                                .as_mut()
                                .and_then(CodexLiveOutputNormalizer::take_final_assistant_message);
                            let codex_error = codex_normalizer
                                .as_mut()
                                .and_then(CodexLiveOutputNormalizer::take_error);
                            let claude_final_assistant = claude_normalizer
                                .as_mut()
                                .and_then(ClaudeLiveOutputNormalizer::take_final_assistant_message);
                            persist_codex_tool_messages(&context, &mut codex_normalizer).await;
                            let provider_specific_final = codex_final_assistant
                                .or(claude_final_assistant);
                            match status {
                                Ok(status) if status.success() => {
                                    if context.context_rollover_id.is_some() {
                                        let follow_up = manager.finish(
                                            &key,
                                            &context,
                                            iowb_protocol::SessionRuntimeStatus::Completed,
                                            None,
                                            run_usage.clone(),
                                        ).await;
                                        if let Some(follow_up) = follow_up {
                                            manager
                                                .start_context_rollover_follow_up(&context, follow_up)
                                                .await;
                                        }
                                    } else {
                                        match select_completed_agent_output(
                                            runtime_provider,
                                            provider_specific_final,
                                            &output,
                                            codex_saw_structured_event,
                                        ) {
                                            Ok(persisted_output) => {
                                                manager.finish(
                                                    &key,
                                                    &context,
                                                    iowb_protocol::SessionRuntimeStatus::Completed,
                                                    Some(persisted_output),
                                                    run_usage.clone(),
                                                ).await;
                                            }
                                            Err(error_output) => {
                                                manager.publish(&context.hub, &key, WsServerEvent::Error {
                                                    message: "Codex completed without a final assistant response".to_string(),
                                                    details: Some(
                                                        "The Codex process exited successfully, but its event stream did not contain a final assistant message. The accumulated CLI transcript was not saved as the reply."
                                                            .to_string(),
                                                    ),
                                                    session_id: Some(context.session_id.clone()),
                                                }).await;
                                                manager.finish(
                                                    &key,
                                                    &context,
                                                    iowb_protocol::SessionRuntimeStatus::Failed,
                                                    Some(error_output),
                                                    run_usage.clone(),
                                                ).await;
                                            }
                                        }
                                    }
                                }
                                Ok(status) => {
                                    let mut persisted_output = provider_specific_final
                                        .unwrap_or_else(|| output.clone());
                                    append_bounded(
                                        &mut output,
                                        &format!("\nAgent exited with status {status}"),
                                        manager.max_output_bytes,
                                    );
                                    append_bounded(
                                        &mut persisted_output,
                                        &format!("\nAgent exited with status {status}"),
                                        manager.max_output_bytes,
                                    );
                                    manager.finish(
                                        &key,
                                        &context,
                                        iowb_protocol::SessionRuntimeStatus::Failed,
                                        Some(persisted_output.clone()),
                                        run_usage.clone(),
                                    ).await;
                                    if let Some(error) = codex_error.as_ref() {
                                        if let Some(run_id) = context.durable_run_id.as_deref() {
                                            let _ = context.storage.update_durable_chat_run_error(
                                                run_id,
                                                &error.message,
                                            );
                                        }
                                        manager.publish_context_recovery_if_needed(
                                            &key,
                                            &context,
                                            error,
                                        ).await;
                                    }
                                }
                                Err(error) => {
                                    let persisted_output = provider_specific_final
                                        .unwrap_or_else(|| output.clone());
                                    manager.publish(&context.hub, &key, WsServerEvent::Error {
                                        message: "agent process wait failed".to_string(),
                                        details: Some(error.to_string()),
                                        session_id: Some(context.session_id.clone()),
                                    }).await;
                                    manager.finish(
                                        &key,
                                        &context,
                                        iowb_protocol::SessionRuntimeStatus::Failed,
                                        Some(persisted_output.clone()),
                                        run_usage.clone(),
                                    ).await;
                                }
                            }
                            break;
                        }
                        _ = &mut abort_rx => {
                            terminate_agent_process_tree(&mut child, &context.session_id).await;
                            drain_aborted_agent_output(&mut output_rx).await;
                            let codex_final_assistant = codex_normalizer
                                .as_mut()
                                .and_then(CodexLiveOutputNormalizer::take_final_assistant_message);
                            let claude_final_assistant = claude_normalizer
                                .as_mut()
                                .and_then(ClaudeLiveOutputNormalizer::take_final_assistant_message);
                            persist_codex_tool_messages(&context, &mut codex_normalizer).await;
                            let final_assistant = codex_final_assistant
                                .or(claude_final_assistant)
                                .unwrap_or_else(|| output.clone());
                            manager.finish(
                                &key,
                                &context,
                                iowb_protocol::SessionRuntimeStatus::Aborted,
                                Some(final_assistant),
                                None,
                            ).await;
                            break;
                        }
                        else => break,
                    }
                }
            });

            Ok(())
        })
    }

    async fn start_direct_ai(&self, context: AgentStartContext) -> Result<()> {
        let (abort_tx, abort_rx) = oneshot::channel();
        let key = agent_run_key(context.provider, &context.session_id);

        self.register(key.clone(), abort_tx).await;

        self.publish(
            &context.hub,
            &key,
            WsServerEvent::SessionStatus {
                provider: context.provider,
                session_id: context.session_id.clone(),
                status: iowb_protocol::SessionRuntimeStatus::Starting,
                response_id: Some(context.response_id.clone()),
                sequence: Some(context.next_sequence()),
                latest_user_prompt: Some(context.prompt.clone()),
            },
        )
        .await;

        let Some(config) = context.direct_ai_config.clone() else {
            let error_message =
                "Direct AI gateway is not configured. Configure IO Gateway in Settings before using gateway models with Claude/Gemini."
                    .to_string();
            self.publish(
                &context.hub,
                &key,
                WsServerEvent::Error {
                    message: "Direct AI gateway is not configured".to_string(),
                    details: Some(error_message.clone()),
                    session_id: Some(context.session_id.clone()),
                },
            )
            .await;
            self.finish(
                &key,
                &context,
                iowb_protocol::SessionRuntimeStatus::Failed,
                Some(error_message),
                None,
            )
            .await;
            return Ok(());
        };

        let Some(model) = context
            .model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        else {
            let error_message = "Direct AI model is missing".to_string();
            self.publish(
                &context.hub,
                &key,
                WsServerEvent::Error {
                    message: error_message.clone(),
                    details: None,
                    session_id: Some(context.session_id.clone()),
                },
            )
            .await;
            self.finish(
                &key,
                &context,
                iowb_protocol::SessionRuntimeStatus::Failed,
                Some(error_message),
                None,
            )
            .await;
            return Ok(());
        };

        info!(
            provider = context.provider.as_str(),
            session_id = %context.session_id,
            project = %context.project_path.display(),
            model = %model,
            "starting Direct AI gateway agent request"
        );

        self.publish(
            &context.hub,
            &key,
            WsServerEvent::SessionStatus {
                provider: context.provider,
                session_id: context.session_id.clone(),
                status: iowb_protocol::SessionRuntimeStatus::Running,
                response_id: Some(context.response_id.clone()),
                sequence: Some(context.next_sequence()),
                latest_user_prompt: Some(context.prompt.clone()),
            },
        )
        .await;

        let manager = self.clone();
        tokio::spawn(async move {
            let mut abort_rx = abort_rx;
            tokio::select! {
                result = stream_direct_ai_model_api(
                    &config,
                    &model,
                    &context.direct_ai_messages,
                    {
                    let hub = context.hub.clone();
                    let key = key.clone();
                    let provider = context.provider;
                    let session_id = context.session_id.clone();
                    let response_id = context.response_id.clone();
                    let sequence = context.sequence.clone();
                    let manager = manager.clone();
                    move |chunk: String| {
                        let hub = hub.clone();
                        let key = key.clone();
                        let session_id = session_id.clone();
                        let response_id = response_id.clone();
                        let sequence = sequence.clone();
                        let manager = manager.clone();
                        async move {
                            manager.publish(&hub, &key, WsServerEvent::Output {
                                provider,
                                session_id,
                                content: chunk,
                                done: false,
                                response_id: Some(response_id),
                                sequence: Some(sequence.fetch_add(1, Ordering::Relaxed) + 1),
                            }).await;
                        }
                    }
                    },
                ) => {
                    match result {
                        Ok(output) => {
                            let mut bounded = String::new();
                            append_bounded(&mut bounded, &output.text, manager.max_output_bytes);
                            if !output.streamed && !bounded.is_empty() {
                                let chunks = direct_ai_display_chunks(&bounded);
                                let chunk_count = chunks.len();
                                for (index, chunk) in chunks.into_iter().enumerate() {
                                    manager.publish(&context.hub, &key, WsServerEvent::Output {
                                        provider: context.provider,
                                        session_id: context.session_id.clone(),
                                        content: chunk,
                                        done: false,
                                        response_id: Some(context.response_id.clone()),
                                        sequence: Some(context.next_sequence()),
                                    }).await;
                                    if index + 1 < chunk_count {
                                        tokio::time::sleep(Duration::from_millis(DIRECT_AI_SYNTHETIC_CHUNK_DELAY_MS)).await;
                                    }
                                }
                            }
                            manager.finish(
                                &key,
                                &context,
                                iowb_protocol::SessionRuntimeStatus::Completed,
                                Some(bounded),
                                output.usage,
                            ).await;
                        }
                        Err(error) => {
                            let error_message = format!("Direct AI gateway request failed\n\n{error}");
                            manager.publish(&context.hub, &key, WsServerEvent::Error {
                                message: "Direct AI gateway request failed".to_string(),
                                details: Some(error_message.clone()),
                                session_id: Some(context.session_id.clone()),
                            }).await;
                            manager.finish(
                                &key,
                                &context,
                                iowb_protocol::SessionRuntimeStatus::Failed,
                                Some(error_message),
                                None,
                            ).await;
                        }
                    }
                }
                _ = &mut abort_rx => {
                    manager.finish(
                        &key,
                        &context,
                        iowb_protocol::SessionRuntimeStatus::Aborted,
                        None,
                        None,
                    ).await;
                }
            }
        });

        Ok(())
    }

    async fn register(&self, key: String, abort_tx: oneshot::Sender<()>) {
        let mut runs = self.runs.write().await;
        runs.insert(
            key,
            AgentRuntimeRecord {
                replay: VecDeque::new(),
                replay_bytes: 0,
                abort_tx: Some(abort_tx),
                last_activity: Utc::now(),
            },
        );
        while runs.len() > self.max_runs {
            if let Some(oldest_key) = runs
                .iter()
                .min_by_key(|(_, record)| record.last_activity)
                .map(|(key, _)| key.clone())
            {
                runs.remove(&oldest_key);
            } else {
                break;
            }
        }
    }

    async fn is_running(&self, provider: Provider, session_id: &str) -> bool {
        let key = agent_run_key(provider, session_id);
        self.runs
            .read()
            .await
            .get(&key)
            .is_some_and(|record| record.abort_tx.is_some())
    }

    async fn publish_context_recovery_if_needed(
        &self,
        key: &str,
        context: &AgentStartContext,
        error: &CodexTurnError,
    ) {
        if context.context_rollover_id.is_some()
            || context.native_resume_session_id.is_none()
            || !is_request_body_too_large_error(error)
        {
            return;
        }
        let Some(run_id) = context.durable_run_id.as_deref() else {
            return;
        };
        let failed_message_id = context
            .storage
            .get_durable_chat_run(run_id)
            .ok()
            .flatten()
            .and_then(|run| run.user_message_id)
            .unwrap_or_default();
        if failed_message_id.is_empty() {
            return;
        }
        self.publish(
            &context.hub,
            key,
            WsServerEvent::ChatRecoveryRequired {
                provider: context.provider,
                session_id: context.session_id.clone(),
                response_id: Some(context.response_id.clone()),
                recovery: ChatContextRecovery {
                    code: "context_too_large".to_string(),
                    state: "required".to_string(),
                    message: "This chat's native context is too large to resume safely. Compact it into a clean context and retry the same message.".to_string(),
                    failed_message_id,
                    observed_bytes: error.observed_bytes,
                    limit_bytes: error.limit_bytes.unwrap_or(CODEX_GATEWAY_BODY_LIMIT_BYTES),
                    request_id: None,
                },
            },
        )
        .await;
    }

    async fn publish(&self, hub: &WsHub, key: &str, event: WsServerEvent) {
        {
            let mut runs = self.runs.write().await;
            if let Some(record) = runs.get_mut(key) {
                record.last_activity = Utc::now();
                let event_bytes = ws_event_estimated_bytes(&event);
                while record.replay.len() >= self.max_replay_events
                    || (!record.replay.is_empty()
                        && record.replay_bytes.saturating_add(event_bytes) > self.max_replay_bytes)
                {
                    if let Some(removed) = record.replay.pop_front() {
                        record.replay_bytes = record
                            .replay_bytes
                            .saturating_sub(ws_event_estimated_bytes(&removed));
                    }
                }
                record.replay_bytes = record.replay_bytes.saturating_add(event_bytes);
                record.replay.push_back(event.clone());
            }
        }
        hub.publish(event);
    }

    async fn finish(
        &self,
        key: &str,
        context: &AgentStartContext,
        status: iowb_protocol::SessionRuntimeStatus,
        assistant_output: Option<String>,
        usage: Option<NormalizedRunUsage>,
    ) -> Option<ContextRolloverFollowUp> {
        let mut status = status;
        let received_at = Utc::now();
        let output = assistant_output
            .map(|output| output.trim().to_string())
            .filter(|output| !output.is_empty())
            .or_else(|| match status {
                iowb_protocol::SessionRuntimeStatus::Failed => Some("Failed".to_string()),
                iowb_protocol::SessionRuntimeStatus::Aborted => Some("Aborted".to_string()),
                _ => None,
            })
            .map(|output| {
                bound_agent_text(
                    &output,
                    AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
                    "assistant response",
                )
            });
        let mut rollover_completed_atomically = false;
        let mut rollover_follow_up = None;
        if let Some(rollover_id) = context.context_rollover_id.as_deref() {
            if matches!(status, iowb_protocol::SessionRuntimeStatus::Completed) {
                match activate_completed_context_rollover(context, rollover_id, received_at).await {
                    Ok(follow_up) => {
                        rollover_completed_atomically = true;
                        rollover_follow_up = follow_up;
                    }
                    Err(error) => {
                        let message = format!("failed to activate clean context: {error}");
                        let _ = context.storage.fail_context_rollover(rollover_id, &message);
                        self.publish(
                            &context.hub,
                            key,
                            WsServerEvent::Error {
                                message: "clean context could not be activated".to_string(),
                                details: Some(message),
                                session_id: Some(context.session_id.clone()),
                            },
                        )
                        .await;
                        status = iowb_protocol::SessionRuntimeStatus::Failed;
                    }
                }
            } else {
                let message = match status {
                    iowb_protocol::SessionRuntimeStatus::Aborted => {
                        "clean context compaction was aborted"
                    }
                    _ => "clean context compaction failed",
                };
                let _ = context.storage.fail_context_rollover(rollover_id, message);
            }
        }
        // A rollover response is committed together with its mapping, marker,
        // and durable-run completion. On any rollover failure, leave the
        // visible transcript untouched so the same recovery can be retried.
        if context.context_rollover_id.is_none()
            && let Some(output) = output
        {
            let persisted_output = output.clone();
            // Persist the assistant message with footer metadata so the
            // bubble at the bottom of the reply stays populated after a
            // refresh or session switch.
            let sent_at = context
                .storage
                .get_session_summary(&context.session_id)
                .ok()
                .flatten()
                .and_then(|s| s.first_user_at);
            let elapsed_ms = sent_at.map(|t| (received_at - t).num_milliseconds().max(0));
            let assistant_meta = serde_json::json!({
                "cli": context.provider.as_str(),
                "durableRunId": context.durable_run_id,
                "model": context.model.clone().unwrap_or_default(),
                "runtime": context.runtime,
                "effort": context.effort.clone().unwrap_or_default(),
                "mode": context.mode.clone().unwrap_or_default(),
                "thinking": context.thinking.unwrap_or(false),
                "fast": context.fast.unwrap_or(false),
                "receivedAt": received_at.to_rfc3339(),
                "sentAt": sent_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                "elapsedMs": elapsed_ms,
                "status": status,
            });
            if let Err(error) = context
                .sessions
                .append_message_with_metadata(
                    &context.session_id,
                    MessageRole::Assistant,
                    output,
                    Some(assistant_meta),
                )
                .await
            {
                warn!(error = %error, session_id = %context.session_id, "failed to persist assistant message");
            } else {
                // Re-stamp with the elapsed value once we know the receiver
                // timestamp is committed (subsquent UI fetches add token
                // usage separately).
                if elapsed_ms.is_some() {
                    let updated = serde_json::json!({
                        "cli": context.provider.as_str(),
                        "durableRunId": context.durable_run_id,
                        "model": context.model.clone().unwrap_or_default(),
                        "runtime": context.runtime,
                        "effort": context.effort.clone().unwrap_or_default(),
                        "mode": context.mode.clone().unwrap_or_default(),
                        "thinking": context.thinking.unwrap_or(false),
                        "fast": context.fast.unwrap_or(false),
                        "receivedAt": received_at.to_rfc3339(),
                        "sentAt": sent_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                        "elapsedMs": elapsed_ms,
                        "status": status,
                    });
                    let _ = context.sessions.stamp_latest_message_metadata(
                        &context.session_id,
                        MessageRole::Assistant,
                        updated,
                    );
                }
                if matches!(status, iowb_protocol::SessionRuntimeStatus::Completed)
                    && context.provider == Provider::Codex
                {
                    let native_prompt = resolve_cli_slash_prompt(Provider::Codex, &context.prompt)
                        .unwrap_or_else(|_| context.prompt.clone());
                    if let Err(error) = context
                        .sessions
                        .sync_codex_turn_to_native_rollout(
                            &context.session_id,
                            &native_prompt,
                            &persisted_output,
                        )
                        .await
                    {
                        warn!(
                            error = %error,
                            session_id = %context.session_id,
                            "failed to sync Codex turn into native rollout"
                        );
                    }
                }
            }
        }

        if let Err(error) = context
            .sessions
            .set_active(&context.session_id, false)
            .await
        {
            warn!(error = %error, session_id = %context.session_id, "failed to mark session inactive");
        }

        let lifetime_token_usage = persist_run_attempt_usage(context, status, usage.as_ref()).await;

        if !rollover_completed_atomically && let Some(run_id) = context.durable_run_id.as_deref() {
            let terminal_result = match status {
                iowb_protocol::SessionRuntimeStatus::Completed => {
                    context.storage.mark_durable_chat_run_completed(run_id)
                }
                iowb_protocol::SessionRuntimeStatus::Aborted => context
                    .storage
                    .mark_durable_chat_run_terminal(run_id, "aborted", None),
                iowb_protocol::SessionRuntimeStatus::Failed => context
                    .storage
                    .mark_durable_chat_run_failed(run_id, "provider run failed"),
                _ => context.storage.mark_durable_chat_run_terminal(
                    run_id,
                    "interrupted",
                    Some("provider run ended with a non-terminal runtime status"),
                ),
            };
            if let Err(error) = terminal_result {
                warn!(
                    error = %error,
                    run_id,
                    session_id = %context.session_id,
                    "failed to mark durable chat run terminal"
                );
            }
        }

        // Stamp metadata so the UI can show "received at", normalized usage,
        // and the conversation metadata snapshot without a follow-up rollout
        // scan. Legacy sessions can still use the token-usage endpoint.
        if let Ok(Some(mut session)) = context.storage.get_session_summary(&context.session_id) {
            // Atomic rollover completion owns the marker timestamp. Do not
            // regress its persisted last-message/activity timestamp during
            // the generic footer pass.
            let completed_at = if rollover_completed_atomically {
                session.last_activity
            } else {
                received_at
            };
            session.received_at = Some(received_at);
            session.last_message_at = Some(completed_at);
            session.last_activity = completed_at;
            session.effort = context.effort.clone().or(session.effort);
            session.mode = context.mode.clone().or(session.mode);
            session.thinking = context.thinking.or(session.thinking);
            session.fast = context.fast.or(session.fast);
            session.token_usage = usage
                .as_ref()
                .map(|usage| usage.usage.clone())
                .or(session.token_usage);
            let snapshot = session.clone();
            if let Err(error) = context.storage.upsert_session(&session) {
                warn!(error = %error, session_id = %context.session_id, "failed to persist session metadata");
            }
            // Broadcast the new snapshot so the UI updates the bubble footer.
            self.publish(
                &context.hub,
                key,
                WsServerEvent::SessionMetadata {
                    provider: context.provider,
                    session_id: context.session_id.clone(),
                    model: snapshot.model,
                    effort: snapshot.effort,
                    mode: snapshot.mode,
                    thinking: snapshot.thinking,
                    fast: snapshot.fast,
                    received_at,
                    last_message_at: snapshot.last_message_at,
                    first_user_at: snapshot.first_user_at,
                    token_usage: snapshot.token_usage,
                    lifetime_token_usage: lifetime_token_usage
                        .clone()
                        .or(snapshot.lifetime_token_usage),
                    response_id: Some(context.response_id.clone()),
                    sequence: Some(context.next_sequence()),
                },
            )
            .await;
        }

        self.publish(
            &context.hub,
            key,
            WsServerEvent::Output {
                provider: context.provider,
                session_id: context.session_id.clone(),
                content: String::new(),
                done: true,
                response_id: Some(context.response_id.clone()),
                sequence: Some(context.next_sequence()),
            },
        )
        .await;
        self.publish(
            &context.hub,
            key,
            WsServerEvent::SessionStatus {
                provider: context.provider,
                session_id: context.session_id.clone(),
                status,
                response_id: Some(context.response_id.clone()),
                sequence: Some(context.next_sequence()),
                latest_user_prompt: Some(context.prompt.clone()),
            },
        )
        .await;
        context.hub.publish(WsServerEvent::ActiveSessions {
            sessions: context.sessions.list_active().await,
        });

        let mut runs = self.runs.write().await;
        if let Some(record) = runs.get_mut(key) {
            record.abort_tx = None;
            record.last_activity = Utc::now();
        }
        drop(runs);

        rollover_follow_up
    }

    async fn start_context_rollover_follow_up(
        &self,
        context: &AgentStartContext,
        follow_up: ContextRolloverFollowUp,
    ) {
        let run = follow_up.run;
        let run_id = run.id.clone();
        let session_id = run.session_id.clone();
        let key = agent_run_key(context.provider, &session_id);
        let fail_follow_up = |storage: &Storage, sessions: &SessionManager, message: String| {
            let run_id = run_id.clone();
            let session_id = session_id.clone();
            let storage = storage.clone();
            let sessions = sessions.clone();
            async move {
                let _ = storage.mark_durable_chat_run_failed(&run_id, &message);
                let _ = sessions.set_active(&session_id, false).await;
            }
        };

        if let Err(error) = context.sessions.set_active(&session_id, true).await {
            let message =
                format!("failed to activate original prompt after clean context: {error}");
            fail_follow_up(&context.storage, &context.sessions, message.clone()).await;
            self.publish(
                &context.hub,
                &key,
                WsServerEvent::Error {
                    message: "clean-context retry could not start".to_string(),
                    details: Some(message),
                    session_id: Some(session_id),
                },
            )
            .await;
            return;
        }

        let direct_ai_messages =
            if should_use_direct_ai_gateway_runtime(context.provider, run.model.as_deref()) {
                match context.sessions.messages(&session_id) {
                    Ok(messages) => direct_ai_conversation_messages(messages, run.prompt.as_str()),
                    Err(error) => {
                        let message =
                            format!("failed to build retry history after clean context: {error}");
                        fail_follow_up(&context.storage, &context.sessions, message.clone()).await;
                        self.publish(
                            &context.hub,
                            &key,
                            WsServerEvent::Error {
                                message: "clean-context retry could not start".to_string(),
                                details: Some(message),
                                session_id: Some(session_id),
                            },
                        )
                        .await;
                        return;
                    }
                }
            } else {
                Vec::new()
            };

        let attempt_id = new_id("attempt");
        if let Err(error) = context
            .storage
            .create_chat_run_attempt(&StoredChatRunAttempt::new(
                attempt_id.clone(),
                run_id.clone(),
                session_id.clone(),
                run.user_message_id.clone(),
                context.provider.as_str(),
                runtime_label(context.runtime),
                run.model.clone(),
                run.native_session_id.clone(),
            ))
        {
            let message = format!("failed to create retry attempt after clean context: {error}");
            fail_follow_up(&context.storage, &context.sessions, message.clone()).await;
            self.publish(
                &context.hub,
                &key,
                WsServerEvent::Error {
                    message: "clean-context retry could not start".to_string(),
                    details: Some(message),
                    session_id: Some(session_id),
                },
            )
            .await;
            return;
        }

        let follow_up_context = AgentStartContext {
            provider: context.provider,
            session_id: session_id.clone(),
            durable_run_id: Some(run_id.clone()),
            attempt_id: Some(attempt_id),
            response_id: new_id("response"),
            sequence: Arc::new(AtomicU64::new(0)),
            project_path: context.project_path.clone(),
            prompt: run.prompt.clone(),
            model: run.model.clone(),
            runtime: context.runtime,
            effort: run.effort.clone(),
            mode: run.mode.clone(),
            thinking: run.thinking,
            fast: run.fast,
            native_resume_session_id: run.native_session_id.clone(),
            context_rollover_id: None,
            direct_ai_config: context.direct_ai_config.clone(),
            direct_ai_messages,
            sessions: context.sessions.clone(),
            storage: context.storage.clone(),
            hub: context.hub.clone(),
        };
        let start_future: Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> =
            Box::pin(self.start(follow_up_context));
        let start_result = start_future.await;

        if let Err(error) = start_result {
            let message = format!("failed to start original prompt after clean context: {error}");
            fail_follow_up(&context.storage, &context.sessions, message.clone()).await;
            self.publish(
                &context.hub,
                &key,
                WsServerEvent::Error {
                    message: "clean-context retry could not start".to_string(),
                    details: Some(message),
                    session_id: Some(session_id.clone()),
                },
            )
            .await;
        }
        context.hub.publish(WsServerEvent::ActiveSessions {
            sessions: context.sessions.list_active().await,
        });
        info!(
            session_id = %session_id,
            run_id = %run_id,
            "started original prompt after clean context activation"
        );
    }

    pub async fn abort(&self, provider: Provider, session_id: &str) -> bool {
        let key = agent_run_key(provider, session_id);
        let abort_tx = {
            let mut runs = self.runs.write().await;
            runs.get_mut(&key).and_then(|record| record.abort_tx.take())
        };
        if let Some(abort_tx) = abort_tx {
            let _ = abort_tx.send(());
            true
        } else {
            false
        }
    }

    pub async fn replay_events(&self) -> Vec<WsServerEvent> {
        let runs = self.runs.read().await;
        let mut active = runs
            .values()
            .filter(|record| record.abort_tx.is_some())
            .collect::<Vec<_>>();
        active.sort_by_key(|record| record.last_activity);
        let mut replay = VecDeque::new();
        let mut replay_bytes = 0usize;
        for event in active
            .into_iter()
            .flat_map(|record| record.replay.iter().cloned())
        {
            replay_bytes = replay_bytes.saturating_add(ws_event_estimated_bytes(&event));
            replay.push_back(event);
            while replay.len() > AGENT_REPLAY_TOTAL_MAX_EVENTS
                || replay_bytes > AGENT_REPLAY_TOTAL_MAX_BYTES
            {
                let Some(removed) = replay.pop_front() else {
                    break;
                };
                replay_bytes = replay_bytes.saturating_sub(ws_event_estimated_bytes(&removed));
            }
        }
        replay.into()
    }
}

async fn persist_run_attempt_usage(
    context: &AgentStartContext,
    status: iowb_protocol::SessionRuntimeStatus,
    usage: Option<&NormalizedRunUsage>,
) -> Option<SessionLifetimeTokenUsage> {
    let Some(attempt_id) = context.attempt_id.as_deref() else {
        return None;
    };
    let status = runtime_status_label(status);
    let (usage_value, raw, source, completeness) = if let Some(usage) = usage {
        (
            Some(&usage.usage),
            usage.raw_usage_json.as_deref(),
            Some(usage.source),
            usage.completeness,
        )
    } else {
        (
            None,
            None,
            Some("provider"),
            TokenUsageCompleteness::Missing,
        )
    };
    match context.storage.finish_chat_run_attempt(
        attempt_id,
        status,
        usage_value,
        raw,
        source,
        completeness,
    ) {
        Ok(lifetime) => lifetime,
        Err(error) => {
            warn!(
                error = %error,
                attempt_id,
                session_id = %context.session_id,
                "failed to persist chat run token usage"
            );
            None
        }
    }
}

fn isolate_agent_process(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);

    // A forced SIGKILL gives Rust no opportunity to run cleanup code. On
    // Linux, ask the kernel to kill the provider CLI when its server parent
    // disappears so startup recovery never overlaps an orphaned old turn.
    #[cfg(target_os = "linux")]
    unsafe {
        command.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn durable_agent_run_scope(database_path: &Path) -> String {
    let canonical_path = std::fs::canonicalize(database_path).unwrap_or_else(|_| {
        if database_path.is_absolute() {
            database_path.to_path_buf()
        } else {
            env::current_dir()
                .map(|current_dir| current_dir.join(database_path))
                .unwrap_or_else(|_| database_path.to_path_buf())
        }
    });
    hex::encode(Sha256::digest(
        canonical_path.as_os_str().as_encoded_bytes(),
    ))
}

#[cfg(target_os = "linux")]
fn process_start_time(process_id: libc::pid_t) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{process_id}/stat")).ok()?;
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "linux")]
fn current_process_identity() -> Option<(libc::pid_t, u64)> {
    let process_id = std::process::id() as libc::pid_t;
    process_start_time(process_id).map(|start_time| (process_id, start_time))
}

#[cfg(target_os = "linux")]
fn process_environment_value<'a>(environment: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let key = key.as_bytes();
    environment
        .split(|byte| *byte == 0)
        .find_map(|entry| entry.strip_prefix(key)?.strip_prefix(b"="))
}

#[cfg(target_os = "linux")]
fn marked_process_owner_is_alive(environment: &[u8]) -> bool {
    let owner_pid = process_environment_value(environment, DURABLE_AGENT_OWNER_PID_ENV)
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.parse::<libc::pid_t>().ok());
    let owner_start = process_environment_value(environment, DURABLE_AGENT_OWNER_START_ENV)
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.parse::<u64>().ok());

    match (owner_pid, owner_start) {
        (Some(owner_pid), Some(owner_start)) => process_start_time(owner_pid) == Some(owner_start),
        // New scoped processes always carry a complete owner identity. Treat
        // incomplete markers as live so cleanup fails closed rather than
        // risking termination of a process owned by another server.
        _ => true,
    }
}

/// Kill provider descendants left behind by a stopped server before a durable
/// continuation is launched. A process must match both the run and canonical
/// database path, and its recorded server owner must no longer be alive.
/// Linux `/proc` exposes these inherited markers even when the original
/// process-group leader has already exited.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OrphanedAgentRunCleanup {
    pub terminated_process_groups: usize,
    pub live_owner: bool,
}

#[cfg(target_os = "linux")]
pub fn terminate_orphaned_agent_run_processes(
    run_id: &str,
    database_path: impl AsRef<Path>,
) -> OrphanedAgentRunCleanup {
    let run_id = run_id.trim();
    if run_id.is_empty() {
        return OrphanedAgentRunCleanup::default();
    }
    let expected_scope = durable_agent_run_scope(database_path.as_ref());
    let current_process_group = unsafe { libc::getpgrp() };
    let mut process_groups = HashSet::<libc::pid_t>::new();
    let mut live_owner = false;
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return OrphanedAgentRunCleanup::default();
    };

    for entry in entries.flatten() {
        let Some(process_id) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        let Ok(environment) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        if process_environment_value(&environment, DURABLE_AGENT_RUN_ENV) != Some(run_id.as_bytes())
            || process_environment_value(&environment, DURABLE_AGENT_SCOPE_ENV)
                != Some(expected_scope.as_bytes())
        {
            continue;
        }
        if marked_process_owner_is_alive(&environment) {
            live_owner = true;
            continue;
        }
        let process_group = unsafe { libc::getpgid(process_id) };
        if process_group > 0 && process_group != current_process_group {
            process_groups.insert(process_group);
        }
    }

    if live_owner {
        return OrphanedAgentRunCleanup {
            terminated_process_groups: 0,
            live_owner: true,
        };
    }

    let mut terminated = 0;
    for process_group in process_groups {
        let result = unsafe { libc::kill(-process_group, libc::SIGKILL) };
        if result == 0 {
            terminated += 1;
            info!(
                run_id,
                process_group, "terminated orphaned durable agent process group"
            );
        } else {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                warn!(
                    error = %error,
                    run_id,
                    process_group,
                    "failed to terminate orphaned durable agent process group"
                );
            }
        }
    }
    OrphanedAgentRunCleanup {
        terminated_process_groups: terminated,
        live_owner,
    }
}

#[cfg(not(target_os = "linux"))]
pub fn terminate_orphaned_agent_run_processes(
    _run_id: &str,
    _database_path: impl AsRef<Path>,
) -> OrphanedAgentRunCleanup {
    OrphanedAgentRunCleanup::default()
}

async fn terminate_agent_process_tree(child: &mut tokio::process::Child, session_id: &str) {
    let process_id = child.id();
    let mut tree_signal_sent = false;

    #[cfg(unix)]
    if let Some(process_id) = process_id {
        match signal_agent_process_group(process_id, libc::SIGTERM) {
            Ok(()) => tree_signal_sent = true,
            Err(error) => {
                warn!(error = %error, session_id, process_id, "failed to terminate agent process group");
            }
        }
        tokio::time::sleep(AGENT_ABORT_TERM_GRACE).await;
        match signal_agent_process_group(process_id, libc::SIGKILL) {
            Ok(()) => tree_signal_sent = true,
            Err(error) => {
                warn!(error = %error, session_id, process_id, "failed to kill agent process group");
            }
        }
    }

    #[cfg(windows)]
    if let Some(process_id) = process_id {
        tree_signal_sent = Command::new("taskkill")
            .args(["/PID", &process_id.to_string(), "/T", "/F"])
            .status()
            .await
            .is_ok_and(|status| status.success());
    }

    if !tree_signal_sent {
        let _ = child.start_kill();
    }

    match tokio::time::timeout(AGENT_ABORT_REAP_TIMEOUT, child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            warn!(error = %error, session_id, "failed to reap aborted agent process");
        }
        Err(_) => {
            let _ = child.start_kill();
            warn!(session_id, "timed out reaping aborted agent process");
        }
    }
}

#[cfg(unix)]
fn signal_agent_process_group(process_id: u32, signal: i32) -> std::io::Result<()> {
    let process_group = i32::try_from(process_id)
        .map_err(|_| std::io::Error::other("agent process id exceeds i32"))?;
    // The child is spawned as its own process-group leader, so a negative PID
    // reaches launchers and every descendant that inherits the group.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

async fn drain_aborted_agent_output(output_rx: &mut mpsc::Receiver<AgentProcessEvent>) {
    output_rx.close();
    let started = Instant::now();
    while let Some(remaining) = AGENT_ABORT_OUTPUT_DRAIN_TIMEOUT.checked_sub(started.elapsed()) {
        match tokio::time::timeout(remaining, output_rx.recv()).await {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
}

impl Default for AgentRuntimeManager {
    fn default() -> Self {
        Self::new(100)
    }
}

#[derive(Clone)]
pub struct ProjectIndex {
    storage: Storage,
}

impl ProjectIndex {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }

    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    pub fn add_project(&self, path: impl AsRef<Path>) -> Result<ProjectSummary> {
        let path = path.as_ref();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "workspace".to_string());
        let now = Utc::now();
        let project = ProjectSummary {
            id: new_id("project"),
            name,
            path: path.display().to_string(),
            repo_name: None,
            created_at: now,
            updated_at: now,
            sessions: Vec::new(),
        };
        self.storage.upsert_project(&project)?;
        Ok(project)
    }

    pub async fn list(&self, sessions: &SessionManager) -> Result<Vec<ProjectSummary>> {
        let mut projects = self.storage.list_projects()?;
        for project in &mut projects {
            project.sessions = sessions.list_for_project(&project.path).await?;
        }
        Ok(projects)
    }

    pub fn find_by_name(&self, project_name: &str) -> Result<ProjectSummary> {
        self.storage
            .find_project_by_name(project_name)?
            .ok_or_else(|| CoreError::ProjectNotFound(project_name.to_string()))
    }

    pub fn delete_by_name(&self, project_name: &str) -> Result<bool> {
        Ok(self.storage.delete_project_by_name(project_name)?)
    }
}

#[derive(Debug, Clone)]
pub struct TaskManager {
    started_at: DateTime<Utc>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            started_at: Utc::now(),
        }
    }

    pub fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct WatchManager {
    debounce_ms: u64,
}

impl WatchManager {
    pub fn new() -> Self {
        Self { debounce_ms: 300 }
    }

    pub fn debounce_ms(&self) -> u64 {
        self.debounce_ms
    }
}

impl Default for WatchManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct WsHub {
    tx: broadcast::Sender<WsServerEvent>,
}

impl WsHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(iowb_protocol::WS_EVENT_CHANNEL_CAPACITY);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WsServerEvent> {
        self.tx.subscribe()
    }

    pub fn publish(&self, event: WsServerEvent) {
        let _ = self.tx.send(event);
    }
}

impl Default for WsHub {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
struct DirectAiStreamOutput {
    text: String,
    streamed: bool,
    usage: Option<NormalizedRunUsage>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct DirectAiConversationMessage {
    role: &'static str,
    content: String,
}

fn direct_ai_conversation_messages(
    messages: Vec<ChatMessage>,
    fallback_prompt: &str,
) -> Vec<DirectAiConversationMessage> {
    let mut selected = Vec::new();
    let mut selected_bytes = 0usize;

    for message in messages.into_iter().rev() {
        let role = match message.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System | MessageRole::Tool => continue,
        };
        let content = message.content.trim();
        if content.is_empty() {
            continue;
        }
        if selected.len() >= DIRECT_AI_HISTORY_MAX_MESSAGES {
            break;
        }
        let next_bytes = selected_bytes.saturating_add(content.len());
        if !selected.is_empty() && next_bytes > DIRECT_AI_HISTORY_MAX_BYTES {
            break;
        }
        selected_bytes = next_bytes;
        selected.push(DirectAiConversationMessage {
            role,
            content: content.to_string(),
        });
    }
    selected.reverse();

    while selected
        .first()
        .is_some_and(|message| message.role == "assistant")
    {
        selected.remove(0);
    }

    let mut normalized: Vec<DirectAiConversationMessage> = Vec::new();
    for message in selected {
        if let Some(previous) = normalized.last_mut()
            && previous.role == message.role
        {
            previous.content.push_str("\n\n");
            previous.content.push_str(&message.content);
        } else {
            normalized.push(message);
        }
    }

    if normalized.is_empty() {
        let fallback_prompt = fallback_prompt.trim();
        if !fallback_prompt.is_empty() {
            normalized.push(DirectAiConversationMessage {
                role: "user",
                content: fallback_prompt.to_string(),
            });
        }
    }

    normalized
}

fn append_direct_ai_recovery_prompt(
    messages: &mut Vec<DirectAiConversationMessage>,
    recovery_prompt: &str,
) {
    if let Some(last) = messages.last_mut()
        && last.role == "user"
    {
        last.content.push_str("\n\n");
        last.content.push_str(recovery_prompt);
    } else {
        messages.push(DirectAiConversationMessage {
            role: "user",
            content: recovery_prompt.to_string(),
        });
    }
}

async fn stream_direct_ai_model_api<F, Fut>(
    config: &DirectAiRuntimeConfig,
    model: &str,
    messages: &[DirectAiConversationMessage],
    mut on_chunk: F,
) -> std::result::Result<DirectAiStreamOutput, String>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ()>,
{
    let api_key = config.api_key.trim();
    if api_key.is_empty() {
        return Err("Direct AI API key is empty".to_string());
    }
    let base_url = config.base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return Err("Direct AI base URL is empty".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|error| format!("failed to create Direct AI client: {error}"))?;

    let max_tokens = config.max_tokens.unwrap_or(4096);
    let messages = messages
        .iter()
        .map(|message| {
            serde_json::json!({
                "role": message.role,
                "content": message.content,
            })
        })
        .collect::<Vec<_>>();
    if messages.is_empty() {
        return Err("Direct AI conversation is empty".to_string());
    }
    let messages_body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": messages,
    });
    let chat_body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "stream_options": {
            "include_usage": true,
        },
        "messages": messages,
    });

    let mut best_error: Option<String> = None;
    for candidate in direct_ai_request_candidates(base_url, model) {
        let body = match candidate.kind {
            DirectAiRequestKind::Messages => &messages_body,
            DirectAiRequestKind::ChatCompletions => &chat_body,
        };
        let response = post_direct_ai_json(&client, &candidate.url, api_key, body).await?;
        if response.status().is_success() {
            let output = read_direct_ai_response(response, &mut on_chunk).await?;
            if output.text.trim().is_empty() {
                return Err("Direct AI returned an empty response".to_string());
            }
            return Ok(output);
        }

        let status = response.status();
        let error = direct_ai_http_error(response).await;
        if best_error
            .as_deref()
            .map(is_low_value_direct_ai_route_error)
            .unwrap_or(true)
            || !is_low_value_direct_ai_route_error(&error)
        {
            best_error = Some(error);
        }
        if !matches!(status.as_u16(), 400 | 404 | 405) {
            break;
        }
    }

    Err(best_error.unwrap_or_else(|| "Direct AI gateway request failed".to_string()))
}

async fn read_direct_ai_response<F, Fut>(
    mut response: reqwest::Response,
    on_chunk: &mut F,
) -> std::result::Result<DirectAiStreamOutput, String>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut raw = Vec::new();
    let mut line_buffer = String::new();
    let mut text = String::new();
    let mut streamed = false;
    let mut usage = None;

    while let Some(bytes) = response
        .chunk()
        .await
        .map_err(|error| format!("Direct AI response stream failed: {error}"))?
    {
        raw.extend_from_slice(&bytes);
        line_buffer.push_str(&String::from_utf8_lossy(&bytes));
        drain_direct_ai_sse_lines(
            &mut line_buffer,
            &mut text,
            &mut streamed,
            &mut usage,
            on_chunk,
        )
        .await;
    }
    if !line_buffer.trim().is_empty() {
        process_direct_ai_sse_line(
            line_buffer.trim(),
            &mut text,
            &mut streamed,
            &mut usage,
            on_chunk,
        )
        .await;
    }

    if streamed {
        return Ok(DirectAiStreamOutput {
            text,
            streamed,
            usage,
        });
    }

    let value = serde_json::from_slice::<Value>(&raw)
        .map_err(|error| format!("Direct AI returned invalid JSON: {error}"))?;
    Ok(DirectAiStreamOutput {
        text: extract_direct_ai_response_text(&value),
        streamed: false,
        usage: normalize_direct_ai_run_usage(&value),
    })
}

async fn drain_direct_ai_sse_lines<F, Fut>(
    buffer: &mut String,
    text: &mut String,
    streamed: &mut bool,
    usage: &mut Option<NormalizedRunUsage>,
    on_chunk: &mut F,
) where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ()>,
{
    while let Some(index) = buffer.find('\n') {
        let line = buffer[..index].trim_end_matches('\r').to_string();
        buffer.drain(..index + 1);
        process_direct_ai_sse_line(&line, text, streamed, usage, on_chunk).await;
    }
}

async fn process_direct_ai_sse_line<F, Fut>(
    line: &str,
    text: &mut String,
    streamed: &mut bool,
    usage: &mut Option<NormalizedRunUsage>,
    on_chunk: &mut F,
) where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ()>,
{
    let Some(data) = line.trim().strip_prefix("data:") else {
        return;
    };
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return;
    };
    if let Some(parsed) = normalize_direct_ai_run_usage(&value) {
        *usage = Some(parsed);
    }
    let chunk = extract_direct_ai_stream_delta(&value);
    if chunk.is_empty() {
        return;
    }
    *streamed = true;
    text.push_str(&chunk);
    let chunks = direct_ai_display_chunks(&chunk);
    let chunk_count = chunks.len();
    for (index, chunk) in chunks.into_iter().enumerate() {
        on_chunk(chunk).await;
        if chunk_count > 1 && index + 1 < chunk_count {
            tokio::time::sleep(Duration::from_millis(DIRECT_AI_SYNTHETIC_CHUNK_DELAY_MS)).await;
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum DirectAiRequestKind {
    Messages,
    ChatCompletions,
}

#[derive(Debug, Clone)]
struct DirectAiRequestCandidate {
    url: String,
    kind: DirectAiRequestKind,
}

fn direct_ai_request_candidates(base_url: &str, model: &str) -> Vec<DirectAiRequestCandidate> {
    let root = url_origin(base_url).unwrap_or_else(|| base_url.to_string());
    let claude_base = if base_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .is_some_and(|segment| segment.eq_ignore_ascii_case("claude"))
    {
        base_url.trim_end_matches('/').to_string()
    } else {
        format!("{}/claude", root.trim_end_matches('/'))
    };
    let root = root.trim_end_matches('/').to_string();
    let base = base_url.trim_end_matches('/').to_string();
    let prefix = gateway_model_prefix(model).unwrap_or_default();

    let mut candidates = Vec::new();
    match prefix.as_str() {
        "cld" => {
            push_direct_ai_candidate(
                &mut candidates,
                format!("{claude_base}/v1/messages"),
                DirectAiRequestKind::Messages,
            );
            push_direct_ai_candidate(
                &mut candidates,
                format!("{root}/v1/chat/completions"),
                DirectAiRequestKind::ChatCompletions,
            );
        }
        "agw" | "gem" | "cop" | "ctm" | "dsk" | "glm" | "grk" | "min" => {
            push_direct_ai_candidate(
                &mut candidates,
                format!("{root}/v1/chat/completions"),
                DirectAiRequestKind::ChatCompletions,
            );
            push_direct_ai_candidate(
                &mut candidates,
                format!("{claude_base}/v1/messages"),
                DirectAiRequestKind::Messages,
            );
        }
        _ => {
            push_direct_ai_candidate(
                &mut candidates,
                format!("{base}/v1/messages"),
                DirectAiRequestKind::Messages,
            );
            push_direct_ai_candidate(
                &mut candidates,
                format!("{base}/v1/chat/completions"),
                DirectAiRequestKind::ChatCompletions,
            );
            push_direct_ai_candidate(
                &mut candidates,
                format!("{root}/v1/chat/completions"),
                DirectAiRequestKind::ChatCompletions,
            );
            push_direct_ai_candidate(
                &mut candidates,
                format!("{claude_base}/v1/messages"),
                DirectAiRequestKind::Messages,
            );
        }
    }
    candidates
}

fn push_direct_ai_candidate(
    candidates: &mut Vec<DirectAiRequestCandidate>,
    url: String,
    kind: DirectAiRequestKind,
) {
    if !candidates.iter().any(|candidate| candidate.url == url) {
        candidates.push(DirectAiRequestCandidate { url, kind });
    }
}

fn is_low_value_direct_ai_route_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("endpoint not found") || lower.contains("\"message\":\"not found\"")
}

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

async fn post_direct_ai_json(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &Value,
) -> std::result::Result<reqwest::Response, String> {
    client
        .post(url)
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .bearer_auth(api_key)
        .header("x-api-key", api_key)
        .json(body)
        .send()
        .await
        .map_err(|error| format!("Direct AI request failed: {error}"))
}

async fn direct_ai_http_error(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    format!(
        "Direct AI HTTP {status}: {}",
        body.chars().take(300).collect::<String>()
    )
}

fn extract_direct_ai_response_text(value: &Value) -> String {
    collect_direct_ai_text(value.get("content"))
        .or_else(|| {
            value
                .get("choices")
                .and_then(Value::as_array)
                .map(|choices| {
                    choices
                        .iter()
                        .filter_map(|choice| {
                            collect_direct_ai_text(
                                choice
                                    .get("message")
                                    .and_then(|message| message.get("content")),
                            )
                            .or_else(|| {
                                collect_direct_ai_text(
                                    choice.get("delta").and_then(|delta| delta.get("content")),
                                )
                            })
                            .or_else(|| {
                                choice
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                        })
                        .collect::<Vec<_>>()
                        .join("")
                })
        })
        .or_else(|| {
            value.get("output").and_then(Value::as_array).map(|output| {
                output
                    .iter()
                    .filter_map(|item| {
                        collect_direct_ai_text(item.get("content")).or_else(|| {
                            item.get("text").and_then(Value::as_str).map(str::to_string)
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("")
            })
        })
        .or_else(|| {
            value
                .get("output_text")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn extract_direct_ai_stream_delta(value: &Value) -> String {
    value
        .get("choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .filter_map(|choice| {
                    choice
                        .get("delta")
                        .and_then(|delta| {
                            collect_direct_ai_text(delta.get("content")).or_else(|| {
                                delta
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                        })
                        .or_else(|| {
                            choice
                                .get("text")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .filter(|text| !text.is_empty())
        .or_else(|| {
            value.get("delta").and_then(|delta| {
                delta.as_str().map(str::to_string).or_else(|| {
                    delta
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| collect_direct_ai_text(delta.get("content")))
                })
            })
        })
        .or_else(|| {
            value
                .get("content_block")
                .and_then(|block| block.get("text"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("type")
                .and_then(Value::as_str)
                .filter(|event_type| {
                    event_type.ends_with(".delta") || event_type.contains("_delta")
                })
                .and_then(|_| collect_direct_ai_text(value.get("content")))
        })
        .unwrap_or_default()
}

fn direct_ai_display_chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for segment in text.split_inclusive('\n') {
        if current.len() + segment.len() > DIRECT_AI_DISPLAY_CHUNK_CHARS && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        if segment.len() > DIRECT_AI_DISPLAY_CHUNK_CHARS * 2 {
            for piece in split_on_char_boundaries(segment, DIRECT_AI_DISPLAY_CHUNK_CHARS) {
                if !current.is_empty() {
                    chunks.push(std::mem::take(&mut current));
                }
                chunks.push(piece);
            }
        } else {
            current.push_str(segment);
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn split_on_char_boundaries(text: &str, target_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if current.chars().count() >= target_chars {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn collect_direct_ai_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .or_else(|| item.get("text").and_then(Value::as_str).map(str::to_string))
                        .or_else(|| collect_direct_ai_text(item.get("content")))
                        .or_else(|| {
                            item.get("output_text")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                })
                .collect::<Vec<_>>()
                .join("");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn normalize_codex_run_usage(value: &Value) -> NormalizedRunUsage {
    let usage = usage_container(value);
    let input = usage_u64(usage, &["input_tokens", "inputTokens", "input"]);
    let output = usage_u64(usage, &["output_tokens", "outputTokens", "output"]);
    let cache_creation = usage_u64(
        usage,
        &[
            "cache_write_input_tokens",
            "cacheWriteInputTokens",
            "cache_creation_input_tokens",
            "cacheCreationInputTokens",
            "cache_creation",
            "cacheCreation",
        ],
    );
    let cache_read = usage_u64(
        usage,
        &[
            "cached_input_tokens",
            "cachedInputTokens",
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cache_read",
            "cacheRead",
        ],
    );
    let reasoning = usage_u64(
        usage,
        &[
            "reasoning_output_tokens",
            "reasoningOutputTokens",
            "reasoning_tokens",
            "reasoningTokens",
            "reasoning",
        ],
    );
    normalized_run_usage(
        usage,
        "codex.turn.completed.usage",
        input,
        output,
        cache_creation,
        cache_read,
        reasoning,
        usage_f64(usage, &["cost_usd", "costUsd"]),
    )
}

fn normalize_claude_run_usage(event: &Value) -> NormalizedRunUsage {
    let usage = event
        .get("modelUsage")
        .or_else(|| event.get("model_usage"))
        .or_else(|| event.get("usage"))
        .unwrap_or(event);
    let totals = aggregate_usage_like_values(usage);
    let mut result = normalized_run_usage(
        usage,
        if event.get("modelUsage").is_some() || event.get("model_usage").is_some() {
            "claude.result.modelUsage"
        } else {
            "claude.result.usage"
        },
        totals.input,
        totals.output,
        totals.cache_creation,
        totals.cache_read,
        totals.reasoning,
        usage_f64(
            event,
            &["total_cost_usd", "totalCostUsd", "cost_usd", "costUsd"],
        )
        .or_else(|| {
            usage_f64(
                usage,
                &["total_cost_usd", "totalCostUsd", "cost_usd", "costUsd"],
            )
        }),
    );
    if result.usage.used == 0 {
        result.completeness = TokenUsageCompleteness::Missing;
    }
    result
}

fn normalize_gemini_run_usage(event: &Value) -> Option<NormalizedRunUsage> {
    let usage = event
        .get("stats")
        .or_else(|| event.pointer("/result/stats"))
        .or_else(|| event.get("usage"))
        .or_else(|| event.get("usageMetadata"))
        .or_else(|| event.get("usage_metadata"))?;
    let input = usage_u64(
        usage,
        &[
            "input_tokens",
            "inputTokens",
            "prompt_token_count",
            "promptTokenCount",
        ],
    );
    let output = usage_u64(
        usage,
        &[
            "output_tokens",
            "outputTokens",
            "candidates_token_count",
            "candidatesTokenCount",
        ],
    );
    let cache_read = usage_u64(
        usage,
        &[
            "cached_content_token_count",
            "cachedContentTokenCount",
            "cache_read_input_tokens",
            "cacheReadInputTokens",
        ],
    );
    let mut result = normalized_run_usage(
        usage,
        "gemini.result.stats",
        input,
        output,
        0,
        cache_read,
        usage_u64(
            usage,
            &["thoughts_token_count", "thoughtsTokenCount", "reasoning"],
        ),
        usage_f64(usage, &["cost_usd", "costUsd"]),
    );
    if result.usage.used == 0 {
        result.completeness = TokenUsageCompleteness::Missing;
    }
    Some(result)
}

fn normalize_direct_ai_run_usage(value: &Value) -> Option<NormalizedRunUsage> {
    let usage = value
        .get("usage")
        .or_else(|| value.get("usageMetadata"))
        .or_else(|| value.get("usage_metadata"))
        .or_else(|| value.pointer("/response/usage"))?;
    let mut result = if value.get("usageMetadata").is_some()
        || value.get("usage_metadata").is_some()
    {
        normalize_gemini_run_usage(value)
            .unwrap_or_else(|| normalized_run_usage(usage, "direct_ai.usage", 0, 0, 0, 0, 0, None))
    } else {
        normalized_direct_ai_usage_from_container(usage)
    };
    result.source = "direct_ai.usage";
    Some(result)
}

fn normalized_direct_ai_usage_from_container(usage: &Value) -> NormalizedRunUsage {
    let input = usage_u64(
        usage,
        &[
            "input_tokens",
            "inputTokens",
            "prompt_tokens",
            "promptTokens",
            "input",
        ],
    );
    let output = usage_u64(
        usage,
        &[
            "output_tokens",
            "outputTokens",
            "completion_tokens",
            "completionTokens",
            "output",
        ],
    );
    let cache_creation = usage_u64(
        usage,
        &[
            "cache_creation_input_tokens",
            "cacheCreationInputTokens",
            "cache_write_input_tokens",
            "cacheWriteInputTokens",
        ],
    );
    let cache_read = usage_u64(
        usage,
        &[
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cached_input_tokens",
            "cachedInputTokens",
        ],
    );
    let reasoning = usage_u64(
        usage,
        &[
            "reasoning_tokens",
            "reasoningTokens",
            "reasoning_output_tokens",
            "reasoningOutputTokens",
        ],
    );
    normalized_run_usage(
        usage,
        "direct_ai.usage",
        input,
        output,
        cache_creation,
        cache_read,
        reasoning,
        usage_f64(
            usage,
            &["cost_usd", "costUsd", "total_cost_usd", "totalCostUsd"],
        ),
    )
}

#[derive(Default)]
struct UsageFields {
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
    reasoning: u64,
}

fn aggregate_usage_like_values(value: &Value) -> UsageFields {
    if let Some(object) = value.as_object() {
        let directly_has_usage = object.keys().any(|key| {
            matches!(
                key.as_str(),
                "input_tokens"
                    | "inputTokens"
                    | "output_tokens"
                    | "outputTokens"
                    | "cache_creation_input_tokens"
                    | "cacheCreationInputTokens"
                    | "cache_read_input_tokens"
                    | "cacheReadInputTokens"
            )
        });
        if directly_has_usage {
            return UsageFields {
                input: usage_u64(value, &["input_tokens", "inputTokens", "input"]),
                output: usage_u64(value, &["output_tokens", "outputTokens", "output"]),
                cache_creation: usage_u64(
                    value,
                    &[
                        "cache_creation_input_tokens",
                        "cacheCreationInputTokens",
                        "cache_creation",
                        "cacheCreation",
                    ],
                ),
                cache_read: usage_u64(
                    value,
                    &[
                        "cache_read_input_tokens",
                        "cacheReadInputTokens",
                        "cache_read",
                        "cacheRead",
                    ],
                ),
                reasoning: usage_u64(value, &["reasoning_tokens", "reasoningTokens", "reasoning"]),
            };
        }
        return object.values().map(aggregate_usage_like_values).fold(
            UsageFields::default(),
            |mut total, usage| {
                total.input = total.input.saturating_add(usage.input);
                total.output = total.output.saturating_add(usage.output);
                total.cache_creation = total.cache_creation.saturating_add(usage.cache_creation);
                total.cache_read = total.cache_read.saturating_add(usage.cache_read);
                total.reasoning = total.reasoning.saturating_add(usage.reasoning);
                total
            },
        );
    }
    if let Some(array) = value.as_array() {
        return array.iter().map(aggregate_usage_like_values).fold(
            UsageFields::default(),
            |mut total, usage| {
                total.input = total.input.saturating_add(usage.input);
                total.output = total.output.saturating_add(usage.output);
                total.cache_creation = total.cache_creation.saturating_add(usage.cache_creation);
                total.cache_read = total.cache_read.saturating_add(usage.cache_read);
                total.reasoning = total.reasoning.saturating_add(usage.reasoning);
                total
            },
        );
    }
    UsageFields::default()
}

fn normalized_run_usage(
    usage: &Value,
    source: &'static str,
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
    reasoning: u64,
    cost_usd: Option<f64>,
) -> NormalizedRunUsage {
    let total = usage_u64(
        usage,
        &[
            "total_tokens",
            "totalTokens",
            "total_token_count",
            "totalTokenCount",
            "total",
        ],
    )
    .max(input.saturating_add(output));
    let usage_value = SessionTokenUsage {
        used: total,
        input,
        output,
        cache_creation,
        cache_read,
        reasoning,
        cost_usd: cost_usd.unwrap_or(0.0),
    };
    let completeness = if total > 0 {
        TokenUsageCompleteness::Complete
    } else {
        TokenUsageCompleteness::Missing
    };
    NormalizedRunUsage {
        usage: usage_value,
        raw_usage_json: serde_json::to_string(usage_container(usage)).ok(),
        source,
        completeness,
    }
}

fn usage_container(value: &Value) -> &Value {
    value
        .get("total_token_usage")
        .or_else(|| value.get("totalTokenUsage"))
        .unwrap_or(value)
}

fn usage_u64(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| {
            value.get(*key).and_then(|raw| {
                raw.as_u64().or_else(|| {
                    raw.as_i64()
                        .and_then(|value| u64::try_from(value).ok())
                        .or_else(|| raw.as_str().and_then(|value| value.parse::<u64>().ok()))
                })
            })
        })
        .unwrap_or(0)
}

fn usage_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|raw| {
            raw.as_f64()
                .or_else(|| raw.as_str().and_then(|value| value.parse::<f64>().ok()))
        })
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_agent_command(
    provider: Provider,
    prompt: &str,
    session_id: &str,
    model: Option<&str>,
    effort: Option<&str>,
    mode: Option<&str>,
    thinking: Option<bool>,
    fast: Option<bool>,
    native_resume_session_id: Option<&str>,
    runtime: ChatRuntime,
) -> Result<AgentCommandSpec> {
    let command_provider = if runtime == ChatRuntime::IoGateway {
        provider
    } else {
        effective_agent_command_provider(provider, model)
    };
    let provider_prefix = format!(
        "IO_WORKBENCH_{}_",
        command_provider.as_str().to_ascii_uppercase()
    );
    let command = env::var(format!("{provider_prefix}COMMAND"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("IO_WORKBENCH_AGENT_COMMAND")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| default_agent_command(command_provider));

    let args_template = env::var(format!("{provider_prefix}ARGS_JSON"))
        .ok()
        .or_else(|| env::var("IO_WORKBENCH_AGENT_ARGS_JSON").ok());
    let cli_prompt = resolve_cli_slash_prompt(command_provider, prompt)?;
    let args = if let Some(args_template) = args_template {
        let raw_args: Vec<String> = serde_json::from_str(&args_template).map_err(|error| {
            CoreError::InvalidInput(format!("invalid agent args JSON: {error}"))
        })?;
        raw_args
            .into_iter()
            .map(|arg| {
                expand_agent_template(
                    arg,
                    &cli_prompt,
                    session_id,
                    model,
                    native_resume_session_id,
                )
            })
            .collect()
    } else {
        default_agent_args_with_resume(
            command_provider,
            &cli_prompt,
            mode,
            effort,
            thinking,
            fast,
            model,
            native_resume_session_id,
            runtime,
        )
    };

    let stdin_prompt = env_bool(&format!("{provider_prefix}STDIN"), false)
        || env_bool("IO_WORKBENCH_AGENT_STDIN", false);

    Ok(AgentCommandSpec {
        command,
        args,
        stdin_prompt,
        prompt: cli_prompt,
    })
}

fn resolve_cli_slash_prompt(provider: Provider, prompt: &str) -> Result<String> {
    if provider != Provider::Codex || !prompt.trim_start().starts_with('/') {
        return Ok(prompt.to_string());
    }
    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
    resolve_codex_slash_prompt(prompt, codex_home.as_deref())
}

fn resolve_codex_slash_prompt(prompt: &str, codex_home: Option<&Path>) -> Result<String> {
    let trimmed = prompt.trim();
    let Some(command_token) = trimmed.split_whitespace().next() else {
        return Ok(prompt.to_string());
    };
    let arguments = trimmed
        .strip_prefix(command_token)
        .unwrap_or_default()
        .trim();

    if let Some(name) = command_token.strip_prefix("/prompts:") {
        let Some(codex_home) = codex_home else {
            return Err(CoreError::InvalidInput(
                "CODEX_HOME is unavailable for custom slash commands".to_string(),
            ));
        };
        return expand_codex_custom_prompt(codex_home, name, arguments);
    }

    let name = command_token.trim_start_matches('/');
    if valid_codex_extension_name(name)
        && codex_home.is_some_and(|home| codex_skill_exists(home, name))
    {
        return Ok(if arguments.is_empty() {
            format!("${name}")
        } else {
            format!("${name} {arguments}")
        });
    }

    Ok(prompt.to_string())
}

fn codex_skill_exists(codex_home: &Path, name: &str) -> bool {
    [
        codex_home.join("skills").join(name).join("SKILL.md"),
        codex_home
            .join("skills")
            .join(".system")
            .join(name)
            .join("SKILL.md"),
        codex_home
            .parent()
            .map(|home| {
                home.join(".agents")
                    .join("skills")
                    .join(name)
                    .join("SKILL.md")
            })
            .unwrap_or_default(),
    ]
    .iter()
    .any(|path| path.is_file())
}

fn expand_codex_custom_prompt(codex_home: &Path, name: &str, arguments: &str) -> Result<String> {
    if !valid_codex_extension_name(name) {
        return Err(CoreError::InvalidInput(format!(
            "invalid Codex slash command name: {name}"
        )));
    }
    let path = codex_home.join("prompts").join(format!("{name}.md"));
    let content = std::fs::read_to_string(&path).map_err(|error| {
        CoreError::InvalidInput(format!(
            "Codex custom slash command /prompts:{name} is unavailable: {error}"
        ))
    })?;
    let template = strip_markdown_frontmatter(&content);
    let values = parse_slash_arguments(arguments)?;
    let positional = values
        .iter()
        .filter(|value| !value.contains('='))
        .cloned()
        .collect::<Vec<_>>();
    let named = values
        .iter()
        .filter_map(|value| value.split_once('='))
        .filter(|(key, _)| {
            !key.is_empty()
                && key
                    .chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
        })
        .collect::<HashMap<_, _>>();

    let dollar_placeholder = "\u{0}IOWB_DOLLAR\u{0}";
    let mut expanded = template.replace("$$", dollar_placeholder);
    expanded = expanded.replace("$ARGUMENTS", arguments);
    for index in 1..=9 {
        expanded = expanded.replace(
            &format!("${index}"),
            positional.get(index - 1).map(String::as_str).unwrap_or(""),
        );
    }
    for (key, value) in named {
        expanded = expanded.replace(&format!("${key}"), value);
    }
    Ok(expanded.replace(dollar_placeholder, "$"))
}

fn valid_codex_extension_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

fn strip_markdown_frontmatter(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("---\n") else {
        return content;
    };
    rest.split_once("\n---\n")
        .map(|(_, body)| body)
        .unwrap_or(content)
}

fn parse_slash_arguments(arguments: &str) -> Result<Vec<String>> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in arguments.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                values.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if quote.is_some() {
        return Err(CoreError::InvalidInput(
            "slash command contains an unterminated quote".to_string(),
        ));
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        values.push(current);
    }
    Ok(values)
}

fn effective_agent_command_provider(provider: Provider, model: Option<&str>) -> Provider {
    if model.is_some_and(uses_codex_aiproxy_cli_runtime) {
        Provider::Codex
    } else {
        provider
    }
}

fn should_use_direct_ai_gateway_runtime(provider: Provider, model: Option<&str>) -> bool {
    match provider {
        // Claude provider runs gateway-prefixed models through Claude Code CLI
        // so the session keeps workspace tools. Use unprefixed aliases such as
        // `sonnet`, `opus`, `haiku`, or `fable` for the local Claude config.
        Provider::Claude => false,
        Provider::Gemini => model.is_some_and(|model| {
            looks_like_proxy_model(model) && !uses_codex_aiproxy_cli_runtime(model)
        }),
        Provider::Codex => false,
    }
}

fn runtime_label(runtime: ChatRuntime) -> &'static str {
    match runtime {
        ChatRuntime::NativeCli => "native_cli",
        ChatRuntime::IoGateway => "io_gateway",
    }
}

fn runtime_status_label(status: iowb_protocol::SessionRuntimeStatus) -> &'static str {
    match status {
        iowb_protocol::SessionRuntimeStatus::Starting => "starting",
        iowb_protocol::SessionRuntimeStatus::Running => "running",
        iowb_protocol::SessionRuntimeStatus::WaitingForInput => "waiting_for_input",
        iowb_protocol::SessionRuntimeStatus::Completed => "completed",
        iowb_protocol::SessionRuntimeStatus::Aborted => "aborted",
        iowb_protocol::SessionRuntimeStatus::Failed => "failed",
    }
}

fn uses_codex_aiproxy_cli_runtime(model: &str) -> bool {
    gateway_model_prefix(model).is_some_and(|prefix| prefix == "cod")
}

#[cfg(test)]
fn uses_claude_aiproxy_cli_runtime(model: &str) -> bool {
    let Some(prefix) = gateway_model_prefix(model) else {
        return false;
    };
    prefix != "cod"
}

#[cfg(test)]
fn should_force_claude_cli_io_gateway(provider: Provider, model: Option<&str>) -> bool {
    provider == Provider::Claude && model.is_some_and(uses_claude_aiproxy_cli_runtime)
}

#[cfg(test)]
fn should_force_codex_cli_io_gateway(model: Option<&str>) -> bool {
    model.is_some_and(looks_like_proxy_model)
}

fn legacy_chat_runtime(model: Option<&str>) -> ChatRuntime {
    if model.is_some_and(looks_like_proxy_model) {
        ChatRuntime::IoGateway
    } else {
        ChatRuntime::NativeCli
    }
}

fn default_agent_command(provider: Provider) -> String {
    let command = match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::Gemini => "gemini",
    };
    preferred_user_command(command).unwrap_or_else(|| command.to_string())
}

fn configured_codex_command() -> String {
    env::var("IO_WORKBENCH_CODEX_COMMAND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_agent_command(Provider::Codex))
}

fn preferred_user_command(command: &str) -> Option<String> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    let candidate = home.join(".local").join("bin").join(command);
    if candidate.is_file() {
        Some(candidate.display().to_string())
    } else {
        None
    }
}

pub fn augmented_user_path() -> OsString {
    let original = env::var_os("PATH").unwrap_or_default();
    let mut paths = Vec::<PathBuf>::new();
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        for candidate in [
            home.join(".local/bin"),
            home.join(".volta/bin"),
            home.join(".fnm/current/bin"),
            home.join(".asdf/shims"),
            home.join(".local/share/mise/shims"),
            home.join(".bun/bin"),
        ] {
            push_unique_directory(&mut paths, candidate);
        }

        let mut nvm_node_bins = std::fs::read_dir(home.join(".nvm/versions/node"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path().join("bin"))
            .filter(|path| path.join("node").is_file())
            .collect::<Vec<_>>();
        nvm_node_bins.sort_by(|left, right| {
            node_version_components(right).cmp(&node_version_components(left))
        });
        for candidate in nvm_node_bins {
            push_unique_directory(&mut paths, candidate);
        }
    }
    for path in env::split_paths(&original) {
        push_unique_directory(&mut paths, path);
    }
    env::join_paths(paths).unwrap_or(original)
}

fn push_unique_directory(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.is_dir() && !paths.contains(&candidate) {
        paths.push(candidate);
    }
}

fn node_version_components(path: &Path) -> Vec<u64> {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse().unwrap_or_default())
        .collect()
}

#[cfg(test)]
fn default_agent_args_with(
    provider: Provider,
    prompt: &str,
    mode: Option<&str>,
    effort: Option<&str>,
    thinking: Option<bool>,
    model: Option<&str>,
) -> Vec<String> {
    let runtime = if model.is_some_and(looks_like_proxy_model) {
        ChatRuntime::IoGateway
    } else {
        ChatRuntime::NativeCli
    };
    default_agent_args_with_resume(
        provider, prompt, mode, effort, thinking, None, model, None, runtime,
    )
}

#[allow(clippy::too_many_arguments)]
fn default_agent_args_with_resume(
    provider: Provider,
    prompt: &str,
    mode: Option<&str>,
    effort: Option<&str>,
    thinking: Option<bool>,
    fast: Option<bool>,
    model: Option<&str>,
    resume_session_id: Option<&str>,
    runtime: ChatRuntime,
) -> Vec<String> {
    let mut args: Vec<String> = match provider {
        Provider::Claude => {
            // Use NDJSON streaming so the chat UI receives partial output the
            // way it does for Codex. Claude requires `--print` for
            // `--output-format stream-json`, and partial assistant deltas only
            // show up when `--include-partial-messages` is enabled.
            let mut args = vec![
                "--print".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--verbose".to_string(),
                "--include-partial-messages".to_string(),
            ];
            if runtime == ChatRuntime::IoGateway {
                args.push("--setting-sources".to_string());
                args.push("project,local".to_string());
            }
            args
        }
        // Codex exec options must precede the `resume` subcommand. The prompt
        // and resume arguments are appended after all shared options below.
        Provider::Codex => vec!["exec".to_string(), "--json".to_string()],
        Provider::Gemini => {
            let mut args = vec![
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--prompt".to_string(),
                prompt.to_string(),
            ];
            if let Some(session_id) = resume_session_id {
                args.extend(["--resume".to_string(), session_id.to_string()]);
            }
            args
        }
    };
    // Always relax Codex's directory / git repo enforcement so it can run in
    // arbitrary project directories, which is the common case for an embedded
    // workspace UI. Native runs inherit the CLI provider configuration; IO
    // Gateway runs receive an isolated provider definition below.
    if matches!(provider, Provider::Codex) {
        args.push("--skip-git-repo-check".to_string());
        if runtime == ChatRuntime::IoGateway {
            push_codex_provider_override(&mut args, "iowb_gateway");
        }
    }
    // Per-mode flags (Claude supports --permission-mode; Codex exec uses
    // --sandbox / --dangerously-bypass-approvals-and-sandbox). Provider-specific.
    if let Some(mode) = mode {
        match normalize_agent_mode(mode).as_deref() {
            Some("bypass") => match provider {
                Provider::Claude => {
                    args.push("--permission-mode".to_string());
                    args.push("bypassPermissions".to_string());
                    args.push("--dangerously-skip-permissions".to_string());
                    args.push("--tools".to_string());
                    args.push("default".to_string());
                }
                Provider::Codex => {
                    args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
                }
                Provider::Gemini => {
                    args.push("--yolo".to_string());
                }
            },
            Some("accept-edits") => match provider {
                Provider::Claude => {
                    args.push("--permission-mode".to_string());
                    args.push("acceptEdits".to_string());
                }
                Provider::Codex => {
                    args.push("--sandbox".to_string());
                    args.push("workspace-write".to_string());
                }
                Provider::Gemini => {}
            },
            Some("plan") => match provider {
                Provider::Claude => {
                    args.push("--permission-mode".to_string());
                    args.push("plan".to_string());
                }
                Provider::Codex => {
                    args.push("--sandbox".to_string());
                    args.push("read-only".to_string());
                }
                Provider::Gemini => {}
            },
            Some("read-only") => match provider {
                Provider::Claude => {
                    args.push("--permission-mode".to_string());
                    args.push("readonly".to_string());
                }
                Provider::Codex => {
                    args.push("--sandbox".to_string());
                    args.push("read-only".to_string());
                }
                Provider::Gemini => {}
            },
            _ => {}
        }
    }
    // The user-selected model is independent from reasoning effort so the
    // dropdown value the user actually picked is what gets sent. Native Codex
    // can omit stale legacy model ids and inherit the model from its own config.
    let selected_model = model.map(str::trim).filter(|value| !value.is_empty());
    if let Some(trimmed) = selected_model
        && !args.iter().any(|a| a == "--model")
        && let Some(cli_model) = agent_cli_model_arg(provider, trimmed)
    {
        args.push("--model".to_string());
        args.push(cli_model);
    }
    if matches!(provider, Provider::Codex) {
        if let Some(reasoning_effort) =
            effective_codex_reasoning_effort(effort, thinking.unwrap_or(false))
        {
            push_codex_config_override(
                &mut args,
                "model_reasoning_effort",
                &format!("\"{reasoning_effort}\""),
            );
        }
        if let Some(fast) = fast {
            push_codex_config_override(&mut args, "features.fast_mode", "true");
            push_codex_config_override(
                &mut args,
                "service_tier",
                if fast { "\"fast\"" } else { "\"default\"" },
            );
        }
    } else if let Some(effort) = effort
        && matches!(provider, Provider::Claude)
    {
        args.push("--effort".to_string());
        args.push(effort.to_string());
    }
    if thinking.unwrap_or(false) && matches!(provider, Provider::Gemini) {
        args.push("--thinking".to_string());
    }
    if matches!(provider, Provider::Codex) {
        if let Some(session_id) = resume_session_id {
            args.extend(["resume".to_string(), session_id.to_string()]);
        }
        args.push(prompt.to_string());
    } else if matches!(provider, Provider::Claude) {
        if let Some(session_id) = resume_session_id {
            args.extend(["--resume".to_string(), session_id.to_string()]);
        }
        args.push(prompt.to_string());
    }
    args
}

fn effective_codex_reasoning_effort(effort: Option<&str>, thinking: bool) -> Option<&str> {
    let selected = effort.and_then(|value| match value {
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra" => Some(value),
        _ => None,
    });
    if thinking && !matches!(selected, Some("xhigh" | "max" | "ultra")) {
        Some("xhigh")
    } else {
        selected
    }
}

/// Returns true when a model id belongs to the IO gateway namespace.
fn looks_like_proxy_model(model: &str) -> bool {
    gateway_model_prefix(model).is_some()
}

fn gateway_model_prefix(model: &str) -> Option<String> {
    let trimmed = model.trim();
    let (prefix, rest) = trimmed.split_once(':')?;
    let normalized = prefix.to_ascii_lowercase();
    if prefix.len() < 2
        || prefix.len() > 12
        || rest.trim().is_empty()
        || !prefix
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        return None;
    }
    Some(match normalized.as_str() {
        "agw" | "cod" | "proxy" | "gateway" | "aiproxy" | "cld" | "gem" | "cop" | "ctm" | "dsk"
        | "glm" | "grk" | "min" => normalized,
        _ => return None,
    })
}

fn agent_cli_model_arg(provider: Provider, model: &str) -> Option<String> {
    if provider == Provider::Codex {
        normalize_codex_cli_model(model).map(str::to_string)
    } else {
        Some(model.to_string())
    }
}

fn normalize_codex_cli_model(model: &str) -> Option<&str> {
    let trimmed = model.trim();
    if is_local_codex_cli_model(trimmed) || looks_like_proxy_model(trimmed) {
        Some(trimmed)
    } else {
        None
    }
}

fn is_local_codex_cli_model(model: &str) -> bool {
    let trimmed = model.trim();
    !trimmed.is_empty()
        && !trimmed.contains(':')
        && gateway_model_prefix(trimmed).is_none()
        && !trimmed.eq_ignore_ascii_case("gpt-5-codex")
}

fn normalize_agent_mode(mode: &str) -> Option<&'static str> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "bypass" | "bypass-permissions" | "bypasspermissions" | "danger" | "no-approvals"
        | "no_approvals" => Some("bypass"),
        "accept-edits" | "acceptedits" | "accept" => Some("accept-edits"),
        "plan" | "plan-only" => Some("plan"),
        "read-only" | "readonly" | "read" => Some("read-only"),
        "default" | "" => Some("default"),
        _ => None,
    }
}

fn apply_claude_cli_io_gateway_env(command: &mut Command, config: &DirectAiRuntimeConfig) {
    command
        .env("ANTHROPIC_BASE_URL", config.base_url.trim_end_matches('/'))
        .env("ANTHROPIC_API_KEY", &config.api_key)
        .env("ANTHROPIC_AUTH_TOKEN", &config.api_key)
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN");
}

fn apply_codex_cli_io_gateway_args(args: &mut Vec<String>, base_url: &str) {
    push_codex_config_override(args, "model_provider", "iowb_gateway");
    push_codex_config_override(args, "model_providers.iowb_gateway.name", "\"IO Gateway\"");
    push_codex_config_override(
        args,
        "model_providers.iowb_gateway.base_url",
        &format!("{:?}", base_url.trim_end_matches('/')),
    );
    push_codex_config_override(
        args,
        "model_providers.iowb_gateway.env_key",
        &format!("\"{IO_WORKBENCH_GATEWAY_KEY_ENV}\""),
    );
    push_codex_config_override(
        args,
        "model_providers.iowb_gateway.wire_api",
        "\"responses\"",
    );
}

fn codex_app_server_launch_options(
    runtime: ChatRuntime,
    config: Option<&DirectAiRuntimeConfig>,
) -> Result<Option<CodexAppServerLaunchOptions>> {
    if runtime != ChatRuntime::IoGateway {
        return Ok(None);
    }
    let config = config.ok_or_else(|| {
        CoreError::InvalidInput(
            "IO Gateway is not configured for Codex context compaction".to_string(),
        )
    })?;
    let mut args = vec!["app-server".to_string()];
    apply_codex_cli_io_gateway_args(&mut args, &config.base_url);
    args.push("--stdio".to_string());
    Ok(Some(CodexAppServerLaunchOptions {
        args,
        env: vec![(
            IO_WORKBENCH_GATEWAY_KEY_ENV.to_string(),
            config.api_key.clone(),
        )],
    }))
}

/// Keep `-c key=value` overrides before the `resume` positional so Codex
/// applies the ephemeral provider configuration to resumed turns.
fn push_codex_provider_override(args: &mut Vec<String>, provider: &str) {
    push_codex_config_override(args, "model_provider", provider);
}

fn push_codex_config_override(args: &mut Vec<String>, key: &str, value: &str) {
    let needle = format!("{key}=");
    if let Some(index) = args
        .windows(2)
        .position(|pair| pair[0] == "-c" && pair[1].starts_with(&needle))
    {
        args[index + 1] = format!("{key}={value}");
    } else {
        let insert_at = args
            .iter()
            .position(|arg| arg == "resume")
            .unwrap_or(args.len());
        args.splice(
            insert_at..insert_at,
            ["-c".to_string(), format!("{key}={value}")],
        );
    }
}

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
    let failed_message = context
        .storage
        .message_by_id(&context.session_id, &rollover.failed_message_id)?
        .filter(|message| message.role == MessageRole::User)
        .ok_or_else(|| CoreError::InvalidInput("failed user message was not found".to_string()))?;
    let mut follow_up_run = StoredDurableChatRun::new(
        new_id("run"),
        Some(rollover.user_id.clone()),
        session.id.clone(),
        context.provider.as_str(),
        failed_message.content.clone(),
        session.project_path.clone(),
    );
    follow_up_run.user_message_id = Some(failed_message.id.clone());
    follow_up_run.native_session_id = Some(candidate.clone());
    follow_up_run.model = context.model.clone();
    follow_up_run.effort = context.effort.clone();
    follow_up_run.mode = context.mode.clone();
    follow_up_run.thinking = context.thinking;
    follow_up_run.fast = context.fast;
    if !context.storage.complete_context_rollover(
        rollover_id,
        retry_run_id,
        &candidate,
        &session,
        &marker,
        None,
        Some(&follow_up_run),
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
        "activated clean native context and staged original prompt"
    );
    Ok(Some(ContextRolloverFollowUp { run: follow_up_run }))
}

fn human_byte_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn utf8_prefix_boundary(value: &str, max_bytes: usize) -> usize {
    if value.len() <= max_bytes {
        return value.len();
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn utf8_suffix_boundary(value: &str, max_bytes: usize) -> usize {
    if value.len() <= max_bytes {
        return 0;
    }
    let mut boundary = value.len().saturating_sub(max_bytes);
    while boundary < value.len() && !value.is_char_boundary(boundary) {
        boundary += 1;
    }
    boundary
}

fn bound_agent_text(value: &str, max_bytes: usize, label: &str) -> String {
    let sanitized = sanitize_agent_text(value);
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
        let end = utf8_prefix_boundary(&marker, max_bytes);
        return marker[..end].to_string();
    }
    let available = max_bytes - marker.len();
    let head_budget = available.saturating_mul(3) / 4;
    let tail_budget = available - head_budget;
    let head_end = utf8_prefix_boundary(&sanitized, head_budget);
    let tail_start = utf8_suffix_boundary(&sanitized, tail_budget);
    format!(
        "{}{}{}",
        &sanitized[..head_end],
        marker,
        &sanitized[tail_start..]
    )
}

fn websocket_text_chunks(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut offset = 0;
    while offset < value.len() {
        let remaining = &value[offset..];
        let length = utf8_prefix_boundary(remaining, AGENT_WEBSOCKET_CHUNK_MAX_BYTES);
        if length == 0 {
            break;
        }
        chunks.push(remaining[..length].to_string());
        offset += length;
    }
    chunks
}

async fn process_agent_event(
    manager: &AgentRuntimeManager,
    context: &AgentStartContext,
    key: &str,
    event: AgentProcessEvent,
    codex_normalizer: &mut Option<CodexLiveOutputNormalizer>,
    claude_normalizer: &mut Option<ClaudeLiveOutputNormalizer>,
    gemini_normalizer: &mut Option<GeminiLiveOutputNormalizer>,
    output: &mut String,
) {
    match event {
        AgentProcessEvent::Output { stream, data } => {
            if stream == AgentOutputStream::Stdout
                && let Some(normalizer) = claude_normalizer.as_mut()
            {
                let visible_chunks = normalizer.push_chunks(&data);
                let native_session_id = normalizer.take_session_id();
                persist_native_session_id(context, native_session_id).await;
                for visible in visible_chunks {
                    publish_agent_output(manager, context, key, output, visible).await;
                }
                return;
            }
            let (visible, native_session_id) = if stream == AgentOutputStream::Stdout {
                if let Some(normalizer) = codex_normalizer.as_mut() {
                    let visible = normalizer.push(&data);
                    (visible, normalizer.take_thread_id())
                } else if let Some(normalizer) = gemini_normalizer.as_mut() {
                    let visible = normalizer.push(&data);
                    (visible, normalizer.take_session_id())
                } else {
                    (data, None)
                }
            } else {
                (data, None)
            };
            persist_native_session_id(context, native_session_id).await;
            publish_agent_output(manager, context, key, output, visible).await;
        }
        AgentProcessEvent::Failed(message) => {
            manager
                .publish(
                    &context.hub,
                    key,
                    WsServerEvent::Error {
                        message: "agent output stream failed".to_string(),
                        details: Some(message),
                        session_id: Some(context.session_id.clone()),
                    },
                )
                .await;
        }
    }
}

async fn flush_codex_live_output(
    manager: &AgentRuntimeManager,
    context: &AgentStartContext,
    key: &str,
    codex_normalizer: &mut Option<CodexLiveOutputNormalizer>,
    output: &mut String,
) {
    let Some(normalizer) = codex_normalizer.as_mut() else {
        return;
    };
    let visible = normalizer.finish();
    let native_session_id = normalizer.take_thread_id();
    persist_native_session_id(context, native_session_id).await;
    publish_agent_output(manager, context, key, output, visible).await;
}

async fn flush_claude_live_output(
    manager: &AgentRuntimeManager,
    context: &AgentStartContext,
    key: &str,
    claude_normalizer: &mut Option<ClaudeLiveOutputNormalizer>,
    output: &mut String,
) {
    let Some(normalizer) = claude_normalizer.as_mut() else {
        return;
    };
    let visible = normalizer.finish();
    let native_session_id = normalizer.take_session_id();
    persist_native_session_id(context, native_session_id).await;
    publish_agent_output(manager, context, key, output, visible).await;
}

async fn flush_gemini_live_output(
    manager: &AgentRuntimeManager,
    context: &AgentStartContext,
    key: &str,
    gemini_normalizer: &mut Option<GeminiLiveOutputNormalizer>,
    output: &mut String,
) {
    let Some(normalizer) = gemini_normalizer.as_mut() else {
        return;
    };
    let visible = normalizer.finish();
    let native_session_id = normalizer.take_session_id();
    persist_native_session_id(context, native_session_id).await;
    publish_agent_output(manager, context, key, output, visible).await;
}

async fn persist_codex_tool_messages(
    context: &AgentStartContext,
    normalizer: &mut Option<CodexLiveOutputNormalizer>,
) {
    let Some(normalizer) = normalizer.as_mut() else {
        return;
    };
    // Rollover output is provisional until the marker, native mapping,
    // assistant response, and retry run can commit atomically. Tool rows are
    // intentionally kept ephemeral for this one turn; otherwise a failed or
    // aborted rollover would mutate the visible transcript before activation.
    if context.context_rollover_id.is_some() {
        normalizer.take_tool_messages();
        return;
    }
    for tool in normalizer.take_tool_messages() {
        let metadata = serde_json::json!({
            "kind": "tool_output",
            "toolName": tool.name,
            "provider": context.provider.as_str(),
            "durableRunId": context.durable_run_id,
            "responseId": context.response_id,
        });
        if let Err(error) = context
            .sessions
            .append_message_with_metadata(
                &context.session_id,
                MessageRole::Tool,
                tool.content,
                Some(metadata),
            )
            .await
        {
            warn!(
                error = %error,
                session_id = %context.session_id,
                "failed to persist bounded Codex tool message"
            );
        }
    }
}

async fn persist_native_session_id(context: &AgentStartContext, native_session_id: Option<String>) {
    let Some(native_session_id) = native_session_id else {
        return;
    };
    if let Some(rollover_id) = context.context_rollover_id.as_deref() {
        let Some(run_id) = context.durable_run_id.as_deref() else {
            warn!(
                session_id = %context.session_id,
                native_session_id = %native_session_id,
                rollover_id,
                "ignored clean native context candidate without a durable retry run"
            );
            return;
        };
        match context.storage.set_context_rollover_candidate(
            rollover_id,
            run_id,
            &native_session_id,
        ) {
            Ok(true) => info!(
                session_id = %context.session_id,
                native_session_id = %native_session_id,
                rollover_id,
                "staged clean native context candidate"
            ),
            Ok(false) => warn!(
                session_id = %context.session_id,
                native_session_id = %native_session_id,
                rollover_id,
                "ignored native context candidate for inactive rollover"
            ),
            Err(error) => warn!(
                error = %error,
                session_id = %context.session_id,
                native_session_id = %native_session_id,
                rollover_id,
                "failed to stage clean native context candidate"
            ),
        }
        return;
    }
    // Once a visible chat has rollover history, only its currently active
    // native mapping is valid. Late thread.started events from an archived
    // process must never replace it, including on the durable run row.
    if context.provider == Provider::Codex {
        match context
            .storage
            .context_native_session_ids(&context.session_id)
        {
            Ok(ids) if !ids.is_empty() => {
                match context.storage.get_session_summary(&context.session_id) {
                    Ok(Some(session))
                        if session.native_session_id.as_deref()
                            == Some(native_session_id.as_str()) => {}
                    Ok(_) => {
                        warn!(
                            session_id = %context.session_id,
                            native_session_id = %native_session_id,
                            "ignored non-active native thread id after context rollover"
                        );
                        return;
                    }
                    Err(error) => {
                        warn!(error = %error, session_id = %context.session_id, "failed to validate native thread id after context rollover");
                        return;
                    }
                }
            }
            Ok(_) => {}
            Err(error) => {
                warn!(error = %error, session_id = %context.session_id, "failed to inspect context rollover history");
                return;
            }
        }
    }
    if let Some(run_id) = context.durable_run_id.as_deref()
        && let Err(error) = context
            .storage
            .update_durable_chat_run_native_session_id(run_id, Some(&native_session_id))
    {
        warn!(
            error = %error,
            run_id,
            session_id = %context.session_id,
            native_session_id = %native_session_id,
            "failed to persist native provider thread id on durable run"
        );
    }
    match context
        .sessions
        .set_native_session_id(&context.session_id, native_session_id.clone())
        .await
    {
        Ok(_) => info!(
            session_id = %context.session_id,
            native_session_id = %native_session_id,
            provider = context.provider.as_str(),
            "associated workbench session with native provider thread"
        ),
        Err(error) => warn!(
            error = %error,
            session_id = %context.session_id,
            native_session_id = %native_session_id,
            provider = context.provider.as_str(),
            "failed to persist native provider thread id"
        ),
    }
}

async fn publish_agent_output(
    manager: &AgentRuntimeManager,
    context: &AgentStartContext,
    key: &str,
    output: &mut String,
    content: String,
) {
    if content.is_empty() {
        return;
    }
    let original_bytes = content.len();
    let content = bound_agent_text(&content, AGENT_LIVE_EVENT_MAX_BYTES, "agent event");
    if content.len() < original_bytes {
        warn!(
            provider = context.provider.as_str(),
            session_id = %context.session_id,
            original_bytes,
            published_bytes = content.len(),
            "truncated oversized agent output event"
        );
    }
    append_bounded(output, &content, manager.max_output_bytes);
    for chunk in websocket_text_chunks(&content) {
        manager
            .publish(
                &context.hub,
                key,
                WsServerEvent::Output {
                    provider: context.provider,
                    session_id: context.session_id.clone(),
                    content: chunk,
                    done: false,
                    response_id: Some(context.response_id.clone()),
                    sequence: Some(context.next_sequence()),
                },
            )
            .await;
    }
}

impl CodexLiveOutputNormalizer {
    fn push(&mut self, chunk: &str) -> String {
        self.pending_line.push_str(chunk);
        let mut output = String::new();
        while let Some(newline) = self.pending_line.find('\n') {
            let line = self.pending_line[..newline]
                .trim_end_matches('\r')
                .to_string();
            self.pending_line.drain(..=newline);
            append_live_section(&mut output, &self.normalize_line(&line));
        }
        output
    }

    fn finish(&mut self) -> String {
        let mut output = String::new();
        if !self.pending_line.is_empty() {
            let line = std::mem::take(&mut self.pending_line);
            append_live_section(
                &mut output,
                &self.normalize_line(line.trim_end_matches('\r')),
            );
        }
        append_live_section(&mut output, &self.take_pending_agent_message(false));
        output
    }

    fn normalize_line(&mut self, line: &str) -> String {
        if line.trim().is_empty() {
            return String::new();
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return line.to_string();
        };
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return line.to_string();
        };
        self.saw_structured_event = true;
        match event_type {
            "thread.started" => {
                if let Some(thread_id) = event
                    .get("thread_id")
                    .or_else(|| event.get("threadId"))
                    .and_then(Value::as_str)
                    .filter(|thread_id| !thread_id.trim().is_empty())
                {
                    self.pending_thread_id = Some(thread_id.to_string());
                }
                String::new()
            }
            "item.started" | "item.updated" | "turn.started" => String::new(),
            "item.completed" => self.normalize_completed_item(&event),
            "turn.completed" => {
                let mut output = self.take_pending_agent_message(false);
                if let Some(usage) = event.get("usage").filter(|value| !value.is_null()) {
                    self.final_usage = Some(normalize_codex_run_usage(usage));
                    append_live_section(
                        &mut output,
                        &format!(
                            "tokens used\n{}",
                            serde_json::to_string_pretty(usage)
                                .unwrap_or_else(|_| usage.to_string())
                        ),
                    );
                }
                output
            }
            "turn.failed" => {
                let mut output = self.take_pending_agent_message(true);
                let message = event
                    .pointer("/error/message")
                    .or_else(|| event.get("error"))
                    .map(display_codex_live_value)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "Codex turn failed".to_string());
                self.last_error = Some(codex_turn_error(&event, &message));
                append_live_section(&mut output, &format!("ERROR: {message}"));
                output
            }
            "error" => {
                let mut output = self.take_pending_agent_message(true);
                let message = event
                    .get("message")
                    .or_else(|| event.get("error"))
                    .map(display_codex_live_value)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "Codex reported an error".to_string());
                self.last_error = Some(codex_turn_error(&event, &message));
                append_live_section(&mut output, &format!("ERROR: {message}"));
                output
            }
            _ => line.to_string(),
        }
    }

    fn normalize_completed_item(&mut self, event: &Value) -> String {
        let item = event.get("item").unwrap_or(&Value::Null);
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if item_type == "agent_message" {
            let content = item
                .get("text")
                .or_else(|| item.pointer("/message/content"))
                .map(display_codex_live_value)
                .unwrap_or_default();
            if content.trim().is_empty() {
                return String::new();
            }
            return match item.get("phase").and_then(Value::as_str) {
                Some("commentary") => {
                    let mut output = self.take_pending_agent_message(true);
                    append_live_section(&mut output, &format!("thinking\n{}", content.trim()));
                    output
                }
                Some("final_answer") => {
                    self.final_assistant_message = Some(bound_agent_text(
                        content.trim(),
                        AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
                        "assistant response",
                    ));
                    let mut output = self.take_pending_agent_message(true);
                    append_live_section(&mut output, &format!("codex\n{}", content.trim()));
                    output
                }
                _ => {
                    let previous = self.take_pending_agent_message(true);
                    self.pending_agent_message = Some(content.trim().to_string());
                    previous
                }
            };
        }

        let mut output = String::new();
        let item_output = match item_type {
            "reasoning" => item
                .get("text")
                .map(display_codex_live_value)
                .filter(|value| !value.is_empty())
                .map(|text| format!("thinking\n{text}"))
                .unwrap_or_default(),
            "command_execution" => format_codex_live_command(item),
            "file_change" => format_codex_live_file_change(item),
            "function_call" => format_codex_live_function_call(item),
            "function_call_output" => format_codex_live_tool_result(item, "function_call"),
            "custom_tool_call" => format_codex_live_custom_tool(item),
            "custom_tool_call_output" => format_codex_live_tool_result(item, "custom_tool_call"),
            "mcp_tool_call" => format_codex_live_named_tool(
                item.get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("mcp_tool_call"),
                item.get("arguments"),
                item.get("result").or_else(|| item.get("error")),
            ),
            "web_search" => {
                format_codex_live_named_tool("web_search", item.get("query"), item.get("result"))
            }
            "todo_list" => format_codex_live_named_tool("todo_list", item.get("items"), None),
            "error" => format!(
                "ERROR: {}",
                item.get("message")
                    .map(display_codex_live_value)
                    .unwrap_or_else(|| "Codex item failed".to_string())
            ),
            "" => return output,
            _ => format_codex_live_named_tool(item_type, Some(item), None),
        };
        if is_codex_tool_item_type(item_type) && !item_output.trim().is_empty() {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(item_type);
            self.record_tool_message(name, &item_output);
        }
        append_live_section(&mut output, &item_output);
        output
    }

    fn take_pending_agent_message(&mut self, thinking: bool) -> String {
        self.pending_agent_message
            .take()
            .map(|content| {
                if thinking {
                    format!("thinking\n{content}")
                } else {
                    self.final_assistant_message = Some(bound_agent_text(
                        &content,
                        AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
                        "assistant response",
                    ));
                    format!("codex\n{content}")
                }
            })
            .unwrap_or_default()
    }

    fn record_tool_message(&mut self, name: &str, content: &str) {
        if self.tool_messages.len() >= AGENT_TOOL_MESSAGES_MAX_COUNT
            || self.tool_message_bytes >= AGENT_TOOL_MESSAGES_MAX_TOTAL_BYTES
        {
            return;
        }
        let remaining = AGENT_TOOL_MESSAGES_MAX_TOTAL_BYTES - self.tool_message_bytes;
        let max_bytes = AGENT_TOOL_MESSAGE_MAX_BYTES.min(remaining);
        let content = bound_agent_text(content, max_bytes, "tool output");
        if content.is_empty() {
            return;
        }
        self.tool_message_bytes += content.len();
        self.tool_messages.push(NormalizedToolMessage {
            name: name.to_string(),
            content,
        });
    }

    fn take_final_assistant_message(&mut self) -> Option<String> {
        self.final_assistant_message.take()
    }

    fn take_final_usage(&mut self) -> Option<NormalizedRunUsage> {
        self.final_usage.take()
    }

    fn saw_structured_event(&self) -> bool {
        self.saw_structured_event
    }

    fn take_tool_messages(&mut self) -> Vec<NormalizedToolMessage> {
        self.tool_message_bytes = 0;
        std::mem::take(&mut self.tool_messages)
    }

    fn take_thread_id(&mut self) -> Option<String> {
        self.pending_thread_id.take()
    }

    fn take_error(&mut self) -> Option<CodexTurnError> {
        self.last_error.take()
    }
}

fn codex_turn_error(event: &Value, message: &str) -> CodexTurnError {
    let code = event
        .pointer("/error/code")
        .or_else(|| event.get("code"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let limit_bytes = event
        .pointer("/error/details/limit_bytes")
        .or_else(|| event.pointer("/details/limit_bytes"))
        .and_then(Value::as_u64);
    let observed_bytes = event
        .pointer("/error/details/content_length_bytes")
        .or_else(|| event.pointer("/details/content_length_bytes"))
        .and_then(Value::as_u64);
    CodexTurnError {
        message: message.to_string(),
        code,
        limit_bytes,
        observed_bytes,
    }
}

fn is_request_body_too_large_error(error: &CodexTurnError) -> bool {
    error.code.as_deref() == Some("request_body_too_large")
        || error.message.to_ascii_lowercase().contains("http 413")
        || error.message.to_ascii_lowercase().contains("payload too large")
        // Compatibility with gateways deployed before the structured 413 fix.
        || error.message.trim().eq_ignore_ascii_case("invalid body")
}

fn is_codex_tool_item_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "command_execution"
            | "file_change"
            | "function_call"
            | "function_call_output"
            | "custom_tool_call"
            | "custom_tool_call_output"
            | "mcp_tool_call"
            | "web_search"
            | "todo_list"
    )
}

#[derive(Default)]
struct ClaudeLiveOutputNormalizer {
    pending_line: String,
    pending_session_id: Option<String>,
    observed_session_id: Option<String>,
    streamed_text: String,
    streamed_thinking: bool,
    streamed_text_started: bool,
    saw_stream_event: bool,
    emitted_content: bool,
    active_tool: Option<ClaudeStreamingTool>,
    tool_names: HashMap<String, String>,
    emitted_tool_results: HashSet<String>,
    final_assistant_message: Option<String>,
    final_usage: Option<NormalizedRunUsage>,
}

#[derive(Default)]
struct ClaudeStreamingTool {
    id: Option<String>,
    name: String,
    input_json: String,
}

impl ClaudeLiveOutputNormalizer {
    fn push_chunks(&mut self, chunk: &str) -> Vec<String> {
        self.pending_line.push_str(chunk);
        let mut output = Vec::new();
        while let Some(newline) = self.pending_line.find('\n') {
            let line = self.pending_line[..newline]
                .trim_end_matches('\r')
                .to_string();
            self.pending_line.drain(..=newline);
            let chunk = self.normalize_line(&line);
            if !chunk.is_empty() {
                output.push(chunk);
            }
        }
        output
    }

    fn finish(&mut self) -> String {
        let mut output = String::new();
        if !self.pending_line.is_empty() {
            let line = std::mem::take(&mut self.pending_line);
            let chunk = self.normalize_line(line.trim_end_matches('\r'));
            if !chunk.is_empty() {
                output.push_str(&chunk);
            }
        }
        output
    }

    fn normalize_line(&mut self, line: &str) -> String {
        if line.trim().is_empty() {
            return String::new();
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return line.to_string();
        };
        if let Some(session_id) = event
            .get("session_id")
            .or_else(|| event.get("sessionId"))
            .and_then(Value::as_str)
            .filter(|session_id| !session_id.trim().is_empty())
            .filter(|session_id| self.observed_session_id.as_deref() != Some(*session_id))
        {
            self.observed_session_id = Some(session_id.to_string());
            self.pending_session_id = Some(session_id.to_string());
        }
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return String::new();
        };
        if event_type == "stream_event" {
            self.saw_stream_event = true;
            return event
                .get("event")
                .map(|stream_event| self.normalize_event(stream_event))
                .unwrap_or_default();
        }
        self.normalize_event(&event)
    }

    fn normalize_event(&mut self, event: &Value) -> String {
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return String::new();
        };
        match event_type {
            // The actual streamed text chunks.
            "content_block_delta" => {
                let delta_type = event
                    .get("delta")
                    .and_then(|delta| delta.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match delta_type {
                    "thinking_delta" => {
                        let thinking = event
                            .get("delta")
                            .and_then(|delta| delta.get("thinking"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if thinking.is_empty() {
                            return String::new();
                        }
                        let prefix = if self.streamed_thinking {
                            ""
                        } else {
                            self.streamed_thinking = true;
                            self.emitted_content = true;
                            "thinking\n"
                        };
                        format!("{prefix}{thinking}")
                    }
                    "text_delta" => {
                        let text = event
                            .get("delta")
                            .and_then(|delta| delta.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if text.is_empty() {
                            return String::new();
                        }
                        self.streamed_text.push_str(&text);
                        let prefix = if !self.streamed_text_started {
                            if self.streamed_thinking {
                                "\n\nclaude\n"
                            } else {
                                "claude\n"
                            }
                        } else {
                            ""
                        };
                        self.streamed_text_started = true;
                        self.emitted_content = true;
                        if self.final_assistant_message.is_none() {
                            self.final_assistant_message = Some(bound_agent_text(
                                self.streamed_text.trim(),
                                AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
                                "assistant response",
                            ));
                        }
                        format!("{prefix}{text}")
                    }
                    "input_json_delta" => {
                        let partial_json = event
                            .get("delta")
                            .and_then(|delta| delta.get("partial_json"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if partial_json.is_empty() {
                            return String::new();
                        }
                        if let Some(tool) = self.active_tool.as_mut() {
                            tool.input_json.push_str(partial_json);
                        }
                        String::new()
                    }
                    _ => String::new(),
                }
            }
            "content_block_start" => event
                .get("content_block")
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
                .map(|block| {
                    let id = block.get("id").and_then(Value::as_str).map(str::to_string);
                    let name = claude_tool_name(block);
                    if let Some(id) = id.as_deref() {
                        self.tool_names.insert(id.to_string(), name.clone());
                    }
                    self.active_tool = Some(ClaudeStreamingTool {
                        id,
                        name,
                        input_json: block
                            .get("input")
                            .filter(|input| !input.is_null() && !is_empty_json_container(input))
                            .map(display_codex_live_value)
                            .unwrap_or_default(),
                    });
                    String::new()
                })
                .unwrap_or_default(),
            "content_block_stop" => self
                .active_tool
                .take()
                .filter(|tool| !tool.name.trim().is_empty())
                .map(format_claude_streaming_tool)
                .map(|section| self.format_activity_section(section))
                .unwrap_or_default(),
            "assistant" if !self.saw_stream_event => {
                let message = event.get("message").unwrap_or(event);
                if self.final_assistant_message.is_none() {
                    if let Some(text) = extract_claude_assistant_text(message) {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            self.final_assistant_message = Some(bound_agent_text(
                                trimmed,
                                AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
                                "assistant response",
                            ));
                        }
                    }
                }
                let section = format_claude_message_content(message, false, &mut self.tool_names);
                self.format_activity_section(section)
            }
            "user" if self.saw_stream_event => {
                let section = format_claude_message_tool_results(
                    event.get("message").unwrap_or(event),
                    &mut self.tool_names,
                    &mut self.emitted_tool_results,
                );
                self.format_activity_section(section)
            }
            "user" => {
                let section = format_claude_message_content(
                    event.get("message").unwrap_or(event),
                    true,
                    &mut self.tool_names,
                );
                self.format_activity_section(section)
            }
            "tool_use" => {
                let section = format_claude_tool_use(event, &mut self.tool_names);
                self.format_activity_section(section)
            }
            "tool_result" | "tool_use_result" => self.format_tool_result_once(event),
            // Lifecycle events we currently ignore but want to swallow
            // silently when the user has not asked for verbose noise.
            "message_start" | "message_delta" | "message_stop" | "ping" => String::new(),
            // Final result event with optional usage info.
            "result" => {
                let mut parts = Vec::new();
                if let Some(text) = event
                    .get("result")
                    .or_else(|| event.get("message"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    let remaining = text.strip_prefix(&self.streamed_text).unwrap_or(text);
                    let trimmed = remaining.trim();
                    if !trimmed.is_empty()
                        && (self.streamed_text.is_empty() || !remaining.is_empty())
                    {
                        if self.final_assistant_message.is_none() {
                            self.final_assistant_message = Some(bound_agent_text(
                                trimmed,
                                AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
                                "assistant response",
                            ));
                        }
                        parts.push(format!("claude\n{trimmed}"));
                    }
                }
                if event
                    .get("modelUsage")
                    .filter(|value| !value.is_null())
                    .is_some()
                    || event
                        .get("model_usage")
                        .filter(|value| !value.is_null())
                        .is_some()
                    || event
                        .get("usage")
                        .filter(|value| !value.is_null())
                        .is_some()
                {
                    self.final_usage = Some(normalize_claude_run_usage(event));
                }
                if let Some(usage) = event.get("usage").filter(|value| !value.is_null()) {
                    parts.push(self.format_activity_section(format!(
                        "tokens used\n{}",
                        serde_json::to_string_pretty(usage).unwrap_or_else(|_| usage.to_string())
                    )));
                }
                parts.join("\n\n")
            }
            // Tool use / progress noise: do not surface to the chat stream.
            "stream_request_start"
            | "stream_request_end"
            | "tool_use_request_start"
            | "tool_use_request_end"
            | "progress"
            | "error" => String::new(),
            _ => String::new(),
        }
    }

    fn take_session_id(&mut self) -> Option<String> {
        self.pending_session_id.take()
    }

    fn take_final_assistant_message(&mut self) -> Option<String> {
        self.final_assistant_message.take()
    }

    fn take_final_usage(&mut self) -> Option<NormalizedRunUsage> {
        self.final_usage.take()
    }

    fn format_activity_section(&mut self, section: String) -> String {
        let section = section.trim();
        if section.is_empty() {
            return String::new();
        }
        let prefix = if self.emitted_content { "\n\n" } else { "" };
        self.emitted_content = true;
        format!("{prefix}{section}")
    }

    fn format_tool_result_once(&mut self, event: &Value) -> String {
        if let Some(key) = claude_tool_result_key(event)
            && !self.emitted_tool_results.insert(key)
        {
            return String::new();
        }
        self.format_activity_section(format_claude_tool_result(event, &self.tool_names))
    }
}

fn is_empty_json_container(value: &Value) -> bool {
    value.as_object().is_some_and(|object| object.is_empty())
        || value.as_array().is_some_and(|array| array.is_empty())
}

fn claude_tool_name(event: &Value) -> String {
    event
        .get("name")
        .or_else(|| event.get("tool_name"))
        .or_else(|| event.get("toolName"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("tool")
        .to_string()
}

fn format_claude_streaming_tool(tool: ClaudeStreamingTool) -> String {
    let input = if tool.input_json.trim().is_empty() {
        "{}".to_string()
    } else {
        serde_json::from_str::<Value>(&tool.input_json)
            .map(|value| display_codex_live_value(&value))
            .unwrap_or(tool.input_json)
    };
    format_claude_tool_sections(&tool.name, tool.id.as_deref(), Some(&input), None)
}

fn format_claude_tool_use(event: &Value, tool_names: &mut HashMap<String, String>) -> String {
    let name = claude_tool_name(event);
    let id = event
        .get("id")
        .or_else(|| event.get("tool_use_id"))
        .or_else(|| event.get("toolUseId"))
        .and_then(Value::as_str);
    if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
        tool_names.insert(id.to_string(), name.clone());
    }
    let input = event
        .get("input")
        .or_else(|| event.get("arguments"))
        .or_else(|| event.get("args"))
        .filter(|value| !value.is_null())
        .map(display_codex_live_value);
    format_claude_tool_sections(&name, id, input.as_deref(), None)
}

fn format_claude_tool_result(event: &Value, tool_names: &HashMap<String, String>) -> String {
    let id = event
        .get("tool_use_id")
        .or_else(|| event.get("toolUseId"))
        .or_else(|| event.get("id"))
        .and_then(Value::as_str);
    let name = id
        .and_then(|id| tool_names.get(id))
        .cloned()
        .unwrap_or_else(|| claude_tool_name(event));
    let result = event
        .get("content")
        .or_else(|| event.get("result"))
        .or_else(|| event.get("output"))
        .or_else(|| event.get("error"))
        .map(display_codex_live_value);
    format_claude_tool_sections(&name, id, None, result.as_deref())
}

fn claude_tool_result_key(event: &Value) -> Option<String> {
    event
        .get("tool_use_id")
        .or_else(|| event.get("toolUseId"))
        .or_else(|| event.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(|id| id.trim().to_string())
}

fn format_claude_message_content(
    message: &Value,
    user_message: bool,
    tool_names: &mut HashMap<String, String>,
) -> String {
    let Some(content) = message.get("content") else {
        return String::new();
    };
    let mut output = String::new();
    let blocks: Vec<&Value> = content
        .as_array()
        .map(|items| items.iter().collect())
        .unwrap_or_else(|| vec![content]);
    for block in blocks {
        match block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "text" if !user_message => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        append_live_section(&mut output, &format!("claude\n{trimmed}"));
                    }
                }
            }
            "thinking" | "thinking_delta" if !user_message => {
                let thinking = block
                    .get("thinking")
                    .or_else(|| block.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !thinking.trim().is_empty() {
                    append_live_section(&mut output, &format!("thinking\n{thinking}"));
                }
            }
            "tool_use" if !user_message => {
                append_live_section(&mut output, &format_claude_tool_use(block, tool_names));
            }
            "tool_result" => {
                append_live_section(&mut output, &format_claude_tool_result(block, tool_names));
            }
            _ => {}
        }
    }
    output.trim().to_string()
}

fn extract_claude_assistant_text(message: &Value) -> Option<String> {
    let content = message.get("content")?;
    let blocks: Vec<&Value> = content
        .as_array()
        .map(|items| items.iter().collect())
        .unwrap_or_else(|| vec![content]);
    let mut combined = String::new();
    for block in blocks {
        let block_type = block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if block_type == "text" {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(text);
            }
        }
    }
    if combined.trim().is_empty() {
        None
    } else {
        Some(combined)
    }
}

fn format_claude_message_tool_results(
    message: &Value,
    tool_names: &mut HashMap<String, String>,
    emitted_tool_results: &mut HashSet<String>,
) -> String {
    let Some(content) = message.get("content") else {
        return String::new();
    };
    let mut output = String::new();
    let blocks: Vec<&Value> = content
        .as_array()
        .map(|items| items.iter().collect())
        .unwrap_or_else(|| vec![content]);
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        if let Some(key) = claude_tool_result_key(block)
            && !emitted_tool_results.insert(key)
        {
            continue;
        }
        append_live_section(&mut output, &format_claude_tool_result(block, tool_names));
    }
    output.trim().to_string()
}

fn format_claude_tool_sections(
    name: &str,
    id: Option<&str>,
    input: Option<&str>,
    result: Option<&str>,
) -> String {
    if is_claude_command_tool(name) {
        return format_claude_command_sections(name, id, input, result);
    }
    let mut content = String::new();
    if input.is_some() || result.is_none() {
        content.push_str("tool / Parameters\n");
        content.push_str(&format!("**Tool:** `{}`", name.trim()));
        if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
            content.push_str(&format!("\n- **ID:** `{}`", id.trim()));
        }
        if let Some(input) = input.filter(|input| !input.trim().is_empty()) {
            content.push_str("\n\n### Input\n```json\n");
            content.push_str(input.trim());
            content.push_str("\n```");
        }
    }
    if let Some(result) = result.filter(|result| !result.trim().is_empty()) {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str("tool / Details\n");
        content.push_str(&format!("**Tool:** `{}`", name.trim()));
        if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
            content.push_str(&format!("\n- **ID:** `{}`", id.trim()));
        }
        content.push_str("\n\n```text\n");
        content.push_str(result.trim());
        content.push_str("\n```");
    }
    bound_agent_text(&content, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output")
}

fn is_claude_command_tool(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "bash" | "shell" | "sh" | "exec" | "command" | "shell_command" | "exec_command"
    )
}

fn format_claude_command_sections(
    name: &str,
    id: Option<&str>,
    input: Option<&str>,
    result: Option<&str>,
) -> String {
    let mut content = String::new();
    if input.is_some() || result.is_none() {
        content.push_str("exec / Parameters\n");
        content.push_str(&format!("**Tool:** `{}`", name.trim()));
        if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
            content.push_str(&format!("\n- **ID:** `{}`", id.trim()));
        }
        if let Some(command) = input.and_then(claude_command_input_shell) {
            content.push_str("\n\n### Command\n```sh\n");
            content.push_str(command.trim());
            content.push_str("\n```");
        } else if let Some(input) = input.filter(|input| !input.trim().is_empty()) {
            content.push_str("\n\n```json\n");
            content.push_str(input.trim());
            content.push_str("\n```");
        }
    }
    if let Some(result) = result.filter(|result| !result.trim().is_empty()) {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str("exec / Details\n");
        content.push_str(&format!("**Tool:** `{}`", name.trim()));
        if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
            content.push_str(&format!("\n- **ID:** `{}`", id.trim()));
        }
        content.push_str("\n\n```text\n");
        content.push_str(result.trim());
        content.push_str("\n```");
    }
    bound_agent_text(&content, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output")
}

fn claude_command_input_shell(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(trimmed)
        .ok()
        .and_then(|value| {
            value
                .get("command")
                .or_else(|| value.get("cmd"))
                .or_else(|| value.get("script"))
                .and_then(Value::as_str)
                .filter(|command| !command.trim().is_empty())
                .map(str::to_string)
        })
}

impl GeminiLiveOutputNormalizer {
    fn push(&mut self, chunk: &str) -> String {
        self.pending_line.push_str(chunk);
        let mut output = String::new();
        while let Some(newline) = self.pending_line.find('\n') {
            let line = self.pending_line[..newline]
                .trim_end_matches('\r')
                .to_string();
            self.pending_line.drain(..=newline);
            output.push_str(&self.normalize_line(&line));
        }
        output
    }

    fn finish(&mut self) -> String {
        if self.pending_line.is_empty() {
            return String::new();
        }
        let line = std::mem::take(&mut self.pending_line);
        self.normalize_line(line.trim_end_matches('\r'))
    }

    fn normalize_line(&mut self, line: &str) -> String {
        if line.trim().is_empty() {
            return String::new();
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return line.to_string();
        };
        if let Some(session_id) = event
            .get("session_id")
            .or_else(|| event.get("sessionId"))
            .or_else(|| event.pointer("/session/id"))
            .and_then(Value::as_str)
            .filter(|session_id| !session_id.trim().is_empty())
        {
            self.pending_session_id = Some(session_id.to_string());
        }

        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "result" => {
                if let Some(usage) = normalize_gemini_run_usage(&event) {
                    self.final_usage = Some(usage);
                }
                String::new()
            }
            "message" | "content" => {
                let role = event
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("assistant");
                if !matches!(role, "assistant" | "model" | "gemini") {
                    return String::new();
                }
                event
                    .get("content")
                    .and_then(|content| {
                        content
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| collect_direct_ai_text(Some(content)))
                    })
                    .or_else(|| {
                        event
                            .get("text")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default()
            }
            "error" => event
                .get("message")
                .or_else(|| event.get("error"))
                .map(display_codex_live_value)
                .filter(|message| !message.is_empty())
                .map(|message| format!("ERROR: {message}\n"))
                .unwrap_or_default(),
            // init/result/tool lifecycle records carry metadata but no new
            // assistant delta that belongs in the visible chat bubble.
            _ => String::new(),
        }
    }

    fn take_session_id(&mut self) -> Option<String> {
        self.pending_session_id.take()
    }

    fn take_final_usage(&mut self) -> Option<NormalizedRunUsage> {
        self.final_usage.take()
    }
}

fn append_live_section(output: &mut String, section: &str) {
    let section = section.trim();
    if section.is_empty() {
        return;
    }
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str(section);
    output.push('\n');
}

fn format_codex_live_command(item: &Value) -> String {
    let command = item
        .get("command")
        .map(display_codex_live_value)
        .unwrap_or_default();
    let result = item
        .get("aggregated_output")
        .or_else(|| item.get("output"))
        .map(display_codex_live_value)
        .unwrap_or_default();
    let exit_code = item.get("exit_code").and_then(Value::as_i64);
    let mut content = format!(
        "exec / Parameters\n**Tool:** `command_execution`\n\n### Command\n```sh\n{}\n```",
        command.trim()
    );
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    append_live_section(
        &mut content,
        &format!(
            "exec / Details\n- **Status:** `{status}`\n- **Exit code:** `{}`\n\n```text\n{}\n```",
            exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "-".to_string()),
            result.trim()
        ),
    );
    bound_agent_text(&content, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output")
}

fn format_codex_live_file_change(item: &Value) -> String {
    let changes = item
        .get("changes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut content = String::new();
    for change in &changes {
        let path = change
            .get("path")
            .or_else(|| change.get("file_path"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let kind = change
            .get("kind")
            .or_else(|| change.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("update");
        let operation = match kind.to_ascii_lowercase().as_str() {
            "add" | "create" | "created" => "create",
            "delete" | "deleted" | "remove" => "delete",
            "move" | "moved" | "rename" | "renamed" => "move",
            _ => "edit",
        };
        append_live_section(&mut content, &format!("{operation} / {path}"));
    }
    if content.is_empty() {
        content.push_str("file_change / Details\n");
    }
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    append_live_section(&mut content, &format!("- **Status:** `{status}`"));
    bound_agent_text(&content, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output")
}

fn format_codex_live_function_call(item: &Value) -> String {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("function_call");
    if matches!(name, "exec_command" | "shell_command") {
        let arguments = item
            .get("arguments")
            .map(display_codex_live_value)
            .unwrap_or_default();
        let parsed = serde_json::from_str::<Value>(&arguments).ok();
        let command = parsed
            .as_ref()
            .and_then(|value| value.get("cmd").or_else(|| value.get("command")))
            .and_then(Value::as_str)
            .unwrap_or(&arguments);
        return bound_agent_text(
            &format!("exec / Parameters\n**Tool:** `{name}`\n\n### Command\n```sh\n{command}\n```"),
            AGENT_TOOL_MESSAGE_MAX_BYTES,
            "tool output",
        );
    }
    format_codex_live_named_tool(name, item.get("arguments"), None)
}

fn format_codex_live_custom_tool(item: &Value) -> String {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("custom_tool_call");
    let input = item
        .get("input")
        .map(display_codex_live_value)
        .unwrap_or_default();
    if name != "apply_patch" {
        return format_codex_live_named_tool(name, Some(&Value::String(input)), None);
    }

    let mut content = String::from("apply_patch");
    for line in input.lines() {
        let trimmed = line.trim();
        let operation = [
            ("*** Add File: ", "create"),
            ("*** Update File: ", "edit"),
            ("*** Delete File: ", "delete"),
            ("*** Move to: ", "move"),
        ]
        .into_iter()
        .find_map(|(prefix, kind)| trimmed.strip_prefix(prefix).map(|path| (kind, path)));
        if let Some((kind, path)) = operation {
            append_live_section(&mut content, &format!("{kind} / {}", path.trim()));
        }
    }
    append_live_section(&mut content, &format!("```diff\n{}\n```", input.trim()));
    bound_agent_text(&content, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output")
}

fn format_codex_live_tool_result(item: &Value, fallback_name: &str) -> String {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(fallback_name);
    format_codex_live_named_tool(
        name,
        None,
        item.get("output").or_else(|| item.get("result")),
    )
}

fn format_codex_live_named_tool(
    name: &str,
    input: Option<&Value>,
    result: Option<&Value>,
) -> String {
    let mut content = format!("tool / Parameters\n**Tool:** `{name}`");
    if let Some(input) = input {
        append_live_section(
            &mut content,
            &format!("```json\n{}\n```", display_codex_live_value(input).trim()),
        );
    }
    if let Some(result) = result {
        append_live_section(
            &mut content,
            &format!(
                "tool / Details\n```text\n{}\n```",
                display_codex_live_value(result).trim()
            ),
        );
    }
    bound_agent_text(&content, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output")
}

fn display_codex_live_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        value => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
    }
}

fn spawn_agent_output_reader<R>(
    tx: mpsc::Sender<AgentProcessEvent>,
    reader: R,
    stream: AgentOutputStream,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut reader = reader;
        let mut buffer = vec![0_u8; 8192];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    if tx
                        .send(AgentProcessEvent::Output {
                            stream,
                            data: String::from_utf8_lossy(&buffer[..read]).into_owned(),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => {
                    let _ = tx.send(AgentProcessEvent::Failed(error.to_string())).await;
                    break;
                }
            }
        }
    });
}

fn env_path(key: &str) -> Option<PathBuf> {
    env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
}

fn env_bool(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .and_then(|value| match value.to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn validate_credentials(username: &str, password: &str) -> Result<()> {
    let username = username.trim();
    if username.is_empty() || password.is_empty() {
        return Err(CoreError::InvalidInput(
            "username and password are required".to_string(),
        ));
    }
    if username.chars().count() < 3 {
        return Err(CoreError::InvalidInput(
            "username must be at least 3 characters".to_string(),
        ));
    }
    if password.chars().count() < 6 {
        return Err(CoreError::InvalidInput(
            "password must be at least 6 characters".to_string(),
        ));
    }
    Ok(())
}

fn verify_totp(secret: &str, code: &str) -> Result<bool> {
    let code = code.trim();
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(false);
    }
    let expected = code
        .parse::<u32>()
        .map_err(|_| CoreError::AuthenticationFailed)?;
    let secret = decode_base32_secret(secret)?;
    let now = Utc::now().timestamp().max(0) as u64 / 30;
    for offset in [-1_i64, 0, 1] {
        let counter = if offset < 0 {
            now.saturating_sub(offset.unsigned_abs())
        } else {
            now.saturating_add(offset as u64)
        };
        if hotp(&secret, counter)? == expected {
            return Ok(true);
        }
    }
    Ok(false)
}

fn hotp(secret: &[u8], counter: u64) -> Result<u32> {
    let mut mac = HmacSha1::new_from_slice(secret)
        .map_err(|_| CoreError::InvalidInput("invalid OTP secret".to_string()))?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let binary = (((digest[offset] & 0x7f) as u32) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | (digest[offset + 3] as u32);
    Ok(binary % 1_000_000)
}

fn decode_base32_secret(secret: &str) -> Result<Vec<u8>> {
    let normalized: String = secret
        .trim()
        .trim_start_matches("otpauth://totp/")
        .chars()
        .filter(|char| !char.is_whitespace() && *char != '-' && *char != '=')
        .map(|char| char.to_ascii_uppercase())
        .collect();
    if normalized.is_empty() {
        return Err(CoreError::InvalidInput(
            "IO_WORKBENCH_OTP_SECRET must not be empty".to_string(),
        ));
    }

    let mut bits: u32 = 0;
    let mut bit_count: u8 = 0;
    let mut output = Vec::with_capacity(normalized.len() * 5 / 8);
    for byte in normalized.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'2'..=b'7' => byte - b'2' + 26,
            _ => {
                return Err(CoreError::InvalidInput(
                    "IO_WORKBENCH_OTP_SECRET must be a valid Base32 secret".to_string(),
                ));
            }
        };
        bits = (bits << 5) | value as u32;
        bit_count += 5;
        while bit_count >= 8 {
            bit_count -= 8;
            output.push(((bits >> bit_count) & 0xff) as u8);
        }
    }
    if output.len() < 10 {
        return Err(CoreError::InvalidInput(
            "IO_WORKBENCH_OTP_SECRET must decode to at least 10 bytes".to_string(),
        ));
    }
    Ok(output)
}

pub fn generate_secret_token(prefix: &str) -> String {
    format!(
        "{prefix}_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

pub fn hash_secret_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(digest)
}

fn user_to_profile(user: &iowb_storage::StoredUser) -> UserProfile {
    UserProfile {
        id: user.id.clone(),
        username: user.username.clone(),
        email: None,
        created_at: user.created_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio::time::{Duration, sleep, timeout};

    async fn temporary_app_state(label: &str) -> (AppState, PathBuf, PathBuf) {
        let root = env::temp_dir().join(format!("iowb-{label}-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");
        let state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
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
        state
            .storage
            .create_user("user-1", "user-1", "test-hash")
            .expect("test user");
        (state, root, project)
    }

    async fn wait_for_context_rollover_state(
        state: &AppState,
        retry_run_id: &str,
        expected_state: &str,
    ) -> StoredSessionContextRollover {
        timeout(Duration::from_secs(3), async {
            loop {
                if let Some(rollover) = state
                    .storage
                    .context_rollover_for_retry_run(retry_run_id)
                    .expect("rollover lookup")
                    && rollover.state == expected_state
                {
                    return rollover;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!("context rollover did not reach state {expected_state} for {retry_run_id}")
        })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn edit_from_here_first_prompt_creates_empty_fork_with_unsent_draft() {
        let (state, root, project) = temporary_app_state("fork-first-prompt").await;
        let source = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                None,
                false,
                Some("gpt-5.4".to_string()),
                Some(ChatRuntime::NativeCli),
                Some("high".to_string()),
                Some("default".to_string()),
                Some(true),
                None,
            )
            .await
            .expect("source session");
        let target = state
            .sessions
            .append_message(
                &source.id,
                MessageRole::User,
                "Rewrite the authentication flow",
            )
            .await
            .expect("target prompt");
        state
            .sessions
            .append_message(&source.id, MessageRole::Assistant, "Original answer")
            .await
            .expect("later answer");
        state
            .sessions
            .set_active(&source.id, false)
            .await
            .expect("source inactive");

        let response = state
            .fork_session_before_message(
                "user-1",
                &source.id,
                &target.id,
                "request-first",
                true,
                Some("Rewrite authentication with passkeys"),
            )
            .await
            .expect("fork first prompt");
        assert_eq!(response.source_session_id, source.id);
        assert_eq!(response.before_message_id, target.id);
        assert_eq!(
            response.session.title,
            "Rewrite authentication with passkeys"
        );
        assert_eq!(response.session.message_count, 0);
        assert_eq!(response.session.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(response.session.effort.as_deref(), Some("high"));
        assert_eq!(response.session.thinking, Some(true));
        assert_eq!(
            response.draft.content,
            "Rewrite authentication with passkeys"
        );
        assert!(!response.native_forked);
        assert!(response.files_unchanged);
        assert!(response.source_hidden);
        assert!(
            state
                .sessions
                .messages(&response.session.id)
                .expect("destination messages")
                .is_empty()
        );
        assert_eq!(
            state
                .sessions
                .messages(&source.id)
                .expect("source messages")
                .len(),
            2
        );

        let retry = state
            .fork_session_before_message(
                "user-1",
                &source.id,
                "different-message-id",
                "request-first",
                false,
                Some("This retry must not replace the original draft"),
            )
            .await
            .expect("idempotent retry");
        assert_eq!(retry.session.id, response.session.id);
        assert_eq!(retry.before_message_id, target.id);
        assert_eq!(retry.draft.content, response.draft.content);
        assert!(retry.source_hidden);
        let listed = state
            .sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("replacement list");
        assert!(listed.iter().all(|session| session.id != source.id));
        assert!(
            listed
                .iter()
                .any(|session| session.id == response.session.id)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn board_scope_survives_cache_miss_and_propagates_to_fork() {
        let (state, root, project) = temporary_app_state("board-scope-continuation").await;
        let source = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("board-chat".to_string()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("source session");
        let source = state
            .sessions
            .mark_board_session(&source.id, "run-1", Some("task-1".to_string()))
            .await
            .expect("mark board session");
        let target = state
            .sessions
            .append_message(&source.id, MessageRole::User, "board prompt")
            .await
            .expect("target prompt");
        state
            .sessions
            .set_active(&source.id, false)
            .await
            .expect("source inactive");

        // A fresh manager models restart/eviction. Continuation must seed the
        // cached entry from storage instead of replacing its scope metadata.
        let reloaded = SessionManager::load(state.storage.clone(), 0).expect("reload manager");
        assert!(reloaded.is_board_session_cached(&source.id));
        let continued = reloaded
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some(source.id.clone()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("continue board session");
        assert!(continued.board_session);
        assert_eq!(continued.board_run_id.as_deref(), Some("run-1"));
        assert_eq!(continued.board_task_id.as_deref(), Some("task-1"));
        assert!(reloaded.list_active().await.is_empty());

        reloaded
            .set_active(&source.id, false)
            .await
            .expect("continued source inactive");
        let fork = state
            .fork_session_before_message(
                "user-1",
                &source.id,
                &target.id,
                "board-fork",
                false,
                None,
            )
            .await
            .expect("fork board session");
        assert!(fork.session.board_session);
        assert_eq!(fork.session.board_run_id.as_deref(), Some("run-1"));
        assert_eq!(fork.session.board_task_id.as_deref(), Some("task-1"));
        assert!(!fork.source_hidden);
        assert!(
            state
                .sessions
                .list_for_project(project.to_str().expect("project path"))
                .await
                .expect("project sessions")
                .iter()
                .all(|session| session.id != source.id && session.id != fork.session.id)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn edit_from_here_direct_ai_fork_clones_only_prior_messages_with_provenance() {
        let (state, root, project) = temporary_app_state("fork-direct-ai").await;
        let source = state
            .sessions
            .create_or_update(
                Provider::Gemini,
                project.display().to_string(),
                None,
                false,
                Some("gem:gemini-2.5-pro".to_string()),
                Some(ChatRuntime::IoGateway),
                Some("medium".to_string()),
                Some("default".to_string()),
                Some(false),
                None,
            )
            .await
            .expect("source session");
        let first_user = state
            .sessions
            .append_message(&source.id, MessageRole::User, "same prompt")
            .await
            .expect("first prompt");
        let first_assistant = state
            .sessions
            .append_message(&source.id, MessageRole::Assistant, "first answer")
            .await
            .expect("first answer");
        let target = state
            .sessions
            .append_message(&source.id, MessageRole::User, "same prompt")
            .await
            .expect("target prompt");
        state
            .sessions
            .append_message(&source.id, MessageRole::Assistant, "later answer")
            .await
            .expect("later answer");
        state
            .sessions
            .set_active(&source.id, false)
            .await
            .expect("source inactive");

        let response = state
            .fork_session_before_message(
                "user-1",
                &source.id,
                &target.id,
                "request-middle",
                false,
                None,
            )
            .await
            .expect("fork middle prompt");
        let cloned = state
            .sessions
            .messages(&response.session.id)
            .expect("cloned messages");
        assert_eq!(cloned.len(), 2);
        assert_eq!(cloned[0].role, MessageRole::User);
        assert_eq!(cloned[0].content, "same prompt");
        assert_eq!(cloned[1].role, MessageRole::Assistant);
        assert_eq!(cloned[1].content, "first answer");
        assert_ne!(cloned[0].id, first_user.id);
        assert_ne!(cloned[1].id, first_assistant.id);
        assert_eq!(cloned[0].metadata["forkedFromSessionId"], source.id);
        assert_eq!(cloned[0].metadata["forkedFromMessageId"], first_user.id);
        assert_eq!(
            cloned[1].metadata["forkedFromMessageId"],
            first_assistant.id
        );
        assert_eq!(response.session.message_count, 2);
        assert_eq!(response.draft.content, "same prompt");
        assert!(!response.native_forked);
        assert!(!response.source_hidden);
        assert_eq!(
            state
                .sessions
                .messages(&source.id)
                .expect("original messages")
                .len(),
            4
        );
        let listed = state
            .sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("non-replacement list");
        assert!(listed.iter().any(|session| session.id == source.id));
        assert!(
            listed
                .iter()
                .any(|session| session.id == response.session.id)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replacing_external_session_hides_and_delete_restores_source() {
        let (mut state, root, project) = temporary_app_state("fork-external-source").await;
        let native_id = "55555555-5555-4555-8555-555555555555";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/13")
            .join(format!("rollout-2026-08-13T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project, "thread_source": "user"}
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "replace external prompt",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now + chrono::Duration::seconds(1),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "external answer"}]
                    }
                })
            ),
        )
        .expect("rollout");
        state.sessions.external_home = Arc::new(root.clone());

        let source = state
            .sessions
            .get(native_id)
            .await
            .expect("external source");
        assert!(source.external);
        let target = state
            .sessions
            .messages_including_external(native_id)
            .await
            .expect("external messages")
            .into_iter()
            .find(|message| message.role == MessageRole::User)
            .expect("external user prompt");

        let response = state
            .fork_session_before_message(
                "user-1",
                native_id,
                &target.id,
                "request-external-replace",
                true,
                Some("edited external prompt"),
            )
            .await
            .expect("replace external source");
        assert!(response.source_hidden);
        let hidden = state
            .sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("hidden source list");
        assert!(hidden.iter().all(|session| session.id != native_id));
        assert!(
            hidden
                .iter()
                .any(|session| session.id == response.session.id)
        );

        state
            .sessions
            .delete(&response.session.id)
            .await
            .expect("delete replacement");
        let restored = state
            .sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("restored source list");
        assert!(restored.iter().any(|session| session.id == native_id));
        assert!(
            restored
                .iter()
                .all(|session| session.id != response.session.id)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn codex_fork_boundary_resolves_duplicate_prompts_in_order() {
        let (state, root, _) = temporary_app_state("fork-boundary").await;
        let now = Utc::now();
        let messages = vec![
            ChatMessage {
                id: "local-user-1".to_string(),
                role: MessageRole::User,
                content: "repeat".to_string(),
                timestamp: now,
                metadata: Value::Null,
            },
            ChatMessage {
                id: "local-assistant-1".to_string(),
                role: MessageRole::Assistant,
                content: "answer".to_string(),
                timestamp: now,
                metadata: Value::Null,
            },
            ChatMessage {
                id: "local-user-2".to_string(),
                role: MessageRole::User,
                content: "repeat".to_string(),
                timestamp: now,
                metadata: Value::Null,
            },
        ];
        let snapshot = CodexThreadSnapshot {
            id: "thread-1".to_string(),
            turns: vec![
                codex_app_server::CodexThreadTurn {
                    id: "turn-1".to_string(),
                    status: "failed".to_string(),
                    user_item_ids: vec!["native-user-1".to_string()],
                    user_text: "repeat".to_string(),
                },
                codex_app_server::CodexThreadTurn {
                    id: "turn-2".to_string(),
                    status: "completed".to_string(),
                    user_item_ids: vec!["native-user-2".to_string()],
                    user_text: "repeat".to_string(),
                },
            ],
        };

        assert_eq!(
            state
                .resolve_codex_fork_boundary(
                    "session-without-durable-runs",
                    &messages[2],
                    &messages,
                    &snapshot,
                )
                .expect("duplicate prompt boundary"),
            "turn-1"
        );

        let target_with_metadata = ChatMessage {
            metadata: serde_json::json!({"nativeBeforeTurnId": "turn-failed"}),
            ..messages[2].clone()
        };
        assert_eq!(
            state
                .resolve_codex_fork_boundary(
                    "session-without-durable-runs",
                    &target_with_metadata,
                    &messages,
                    &snapshot,
                )
                .expect("metadata boundary"),
            "turn-failed"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn abort_terminates_descendants_that_keep_agent_output_open() {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "(trap '' TERM; while :; do sleep 60; done) & echo ready; wait",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        isolate_agent_process(&mut command);
        let mut child = command.spawn().expect("spawn launcher");
        let mut stdout = child.stdout.take().expect("launcher stdout");
        let mut ready = [0_u8; 6];
        timeout(Duration::from_secs(1), stdout.read_exact(&mut ready))
            .await
            .expect("descendant startup timed out")
            .expect("read descendant startup marker");
        assert_eq!(&ready, b"ready\n");

        let output_closed = tokio::spawn(async move {
            let mut remainder = Vec::new();
            stdout.read_to_end(&mut remainder).await
        });
        terminate_agent_process_tree(&mut child, "process-tree-test").await;

        timeout(Duration::from_secs(1), output_closed)
            .await
            .expect("descendant retained the output pipe")
            .expect("output reader task")
            .expect("read output to EOF");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn aborted_output_drain_does_not_wait_for_lingering_sender() {
        let (_sender, mut receiver) = mpsc::channel(1);

        timeout(
            Duration::from_secs(1),
            drain_aborted_agent_output(&mut receiver),
        )
        .await
        .expect("abort drain waited for an open sender");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    async fn startup_cleanup_is_scoped_to_database_and_dead_owner() {
        let run_id = format!("run-orphan-test-{}", Uuid::new_v4());
        let root = env::temp_dir().join(format!("iowb-orphan-test-{}", Uuid::new_v4()));
        let original_database = root.join("original/io-workbench.db");
        let copied_database = root.join("copy/io-workbench.db");
        std::fs::create_dir_all(original_database.parent().expect("original parent"))
            .expect("create original parent");
        std::fs::create_dir_all(copied_database.parent().expect("copy parent"))
            .expect("create copy parent");
        std::fs::write(&original_database, []).expect("create original database");
        std::fs::write(&copied_database, []).expect("create copied database");
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 60 </dev/null >/dev/null 2>&1 & echo $!"])
            .env(DURABLE_AGENT_RUN_ENV, &run_id)
            .env(
                DURABLE_AGENT_SCOPE_ENV,
                durable_agent_run_scope(&original_database),
            )
            .env(DURABLE_AGENT_OWNER_PID_ENV, "2147483647")
            .env(DURABLE_AGENT_OWNER_START_ENV, "1")
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let output = command.output().await.expect("spawn marked orphan");
        assert!(output.status.success());
        let orphan_pid = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<libc::pid_t>()
            .expect("orphan pid");

        assert_eq!(
            terminate_orphaned_agent_run_processes(&run_id, &copied_database),
            OrphanedAgentRunCleanup::default()
        );
        assert!(std::fs::metadata(format!("/proc/{orphan_pid}")).is_ok());
        assert_eq!(
            terminate_orphaned_agent_run_processes(&run_id, &original_database),
            OrphanedAgentRunCleanup {
                terminated_process_groups: 1,
                live_owner: false,
            }
        );
        timeout(Duration::from_secs(1), async {
            loop {
                let state = std::fs::read_to_string(format!("/proc/{orphan_pid}/stat"))
                    .ok()
                    .and_then(|stat| {
                        stat.rsplit_once(')')
                            .and_then(|(_, fields)| fields.split_whitespace().next())
                            .and_then(|state| state.chars().next())
                    });
                if state.is_none_or(|state| state == 'Z') {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("marked orphan was not killed");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    async fn startup_cleanup_preserves_process_with_live_owner() {
        let run_id = format!("run-live-owner-test-{}", Uuid::new_v4());
        let database = env::temp_dir().join(format!("iowb-live-owner-{}.db", Uuid::new_v4()));
        std::fs::write(&database, []).expect("create database");
        let (owner_pid, owner_start) = current_process_identity().expect("test process identity");
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 60"])
            .env(DURABLE_AGENT_RUN_ENV, &run_id)
            .env(DURABLE_AGENT_SCOPE_ENV, durable_agent_run_scope(&database))
            .env(DURABLE_AGENT_OWNER_PID_ENV, owner_pid.to_string())
            .env(DURABLE_AGENT_OWNER_START_ENV, owner_start.to_string())
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().expect("spawn live-owned process");

        sleep(Duration::from_millis(25)).await;
        assert_eq!(
            terminate_orphaned_agent_run_processes(&run_id, &database),
            OrphanedAgentRunCleanup {
                terminated_process_groups: 0,
                live_owner: true,
            }
        );
        assert!(child.try_wait().expect("read child state").is_none());

        terminate_agent_process_tree(&mut child, "live-owner-test").await;
        let _ = std::fs::remove_file(database);
    }

    #[test]
    fn summary_truncates_long_prompts() {
        let prompt = "a".repeat(80);
        assert_eq!(
            session_title_from_prompt(&prompt),
            Some(format!("{}...", "a".repeat(50)))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn latest_user_prompt_updates_auto_title_but_manual_title_stays_locked() {
        let root = env::temp_dir().join(format!("iowb-session-title-{}", Uuid::new_v4()));
        let storage = Storage::open(root.join("test.db")).expect("storage");
        let sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        let session = sessions
            .create_or_update(
                Provider::Codex,
                root.display().to_string(),
                Some("session-title-test".to_string()),
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

        sessions
            .append_message(
                &session.id,
                MessageRole::User,
                "  First title\n\nwith spacing  ",
            )
            .await
            .expect("first prompt");
        assert_eq!(
            sessions.get(&session.id).await.expect("first title").title,
            "First title with spacing"
        );

        sessions
            .append_message(&session.id, MessageRole::Assistant, "assistant reply")
            .await
            .expect("assistant reply");
        assert_eq!(
            sessions
                .get(&session.id)
                .await
                .expect("assistant keeps title")
                .title,
            "First title with spacing"
        );

        sessions
            .append_message(&session.id, MessageRole::User, "latest prompt")
            .await
            .expect("latest prompt");
        let automatic = sessions.get(&session.id).await.expect("automatic title");
        assert_eq!(automatic.title, "latest prompt");
        assert_eq!(automatic.title_source, Some(SessionTitleSource::Prompt));

        sessions
            .rename(&session.id, "Manual investigation".to_string())
            .await
            .expect("manual rename");
        sessions
            .append_message(&session.id, MessageRole::User, "do not replace manual")
            .await
            .expect("prompt after manual rename");
        let manual = sessions.get(&session.id).await.expect("manual title");
        assert_eq!(manual.title, "Manual investigation");
        assert_eq!(manual.title_source, Some(SessionTitleSource::Manual));

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn external_refresh_preserves_workbench_prompt_title() {
        let root = env::temp_dir().join(format!("iowb-external-title-{}", Uuid::new_v4()));
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("project");
        let storage = Storage::open(root.join("test.db")).expect("storage");
        storage
            .upsert_session(&SessionSummary {
                id: "external-title-session".to_string(),
                provider: Provider::Codex,
                external: true,
                project_path: project.display().to_string(),
                title: "latest Workbench prompt".to_string(),
                last_activity: Utc::now(),
                title_source: Some(SessionTitleSource::Prompt),
                ..Default::default()
            })
            .expect("stored external session");

        let sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        {
            let mut cache = sessions.external_cache.write().await;
            cache.loaded_at = Some(Instant::now());
            cache.records = vec![ExternalSessionRecord {
                summary: SessionSummary {
                    id: "external-title-session".to_string(),
                    provider: Provider::Codex,
                    external: true,
                    project_path: project.display().to_string(),
                    title: "provider first prompt".to_string(),
                    last_activity: Utc::now(),
                    title_source: Some(SessionTitleSource::External),
                    ..Default::default()
                },
                file_path: root.join("missing-rollout.jsonl"),
            }];
        }

        let listed = sessions
            .list_for_project(&project.display().to_string())
            .await
            .expect("project sessions");
        let session = listed
            .into_iter()
            .find(|session| session.id == "external-title-session")
            .expect("external session");
        assert_eq!(session.title, "latest Workbench prompt");
        assert_eq!(session.title_source, Some(SessionTitleSource::Prompt));

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn direct_ai_history_filters_and_normalizes_stored_messages() {
        let now = Utc::now();
        let message = |role, content: &str| ChatMessage {
            id: new_id("msg"),
            role,
            content: content.to_string(),
            timestamp: now,
            metadata: Value::Null,
        };
        let history = direct_ai_conversation_messages(
            vec![
                message(MessageRole::Assistant, "orphaned assistant"),
                message(MessageRole::System, "internal status"),
                message(MessageRole::User, "first question"),
                message(MessageRole::Tool, "tool output"),
                message(MessageRole::User, "follow-up detail"),
                message(MessageRole::Assistant, "earlier answer"),
                message(MessageRole::User, "current question"),
            ],
            "current question",
        );

        assert_eq!(
            history,
            vec![
                DirectAiConversationMessage {
                    role: "user",
                    content: "first question\n\nfollow-up detail".to_string(),
                },
                DirectAiConversationMessage {
                    role: "assistant",
                    content: "earlier answer".to_string(),
                },
                DirectAiConversationMessage {
                    role: "user",
                    content: "current question".to_string(),
                },
            ]
        );
    }

    #[test]
    fn direct_ai_history_is_bounded_and_keeps_current_prompt() {
        let now = Utc::now();
        let mut messages = (0..80)
            .map(|index| ChatMessage {
                id: format!("msg-{index:03}"),
                role: if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                content: format!("message-{index}"),
                timestamp: now,
                metadata: Value::Null,
            })
            .collect::<Vec<_>>();
        messages.push(ChatMessage {
            id: "msg-current".to_string(),
            role: MessageRole::User,
            content: "current question".to_string(),
            timestamp: now,
            metadata: Value::Null,
        });

        let bounded = direct_ai_conversation_messages(messages, "current question");
        assert!(bounded.len() <= DIRECT_AI_HISTORY_MAX_MESSAGES);
        assert_eq!(bounded.first().map(|message| message.role), Some("user"));
        assert_eq!(
            bounded.last(),
            Some(&DirectAiConversationMessage {
                role: "user",
                content: "current question".to_string(),
            })
        );
        assert!(bounded.iter().all(|message| message.content != "message-0"));

        let oversized_old_message = ChatMessage {
            id: "msg-oversized".to_string(),
            role: MessageRole::User,
            content: "x".repeat(DIRECT_AI_HISTORY_MAX_BYTES),
            timestamp: now,
            metadata: Value::Null,
        };
        let current = ChatMessage {
            id: "msg-latest".to_string(),
            role: MessageRole::User,
            content: "latest prompt".to_string(),
            timestamp: now,
            metadata: Value::Null,
        };
        let bounded_by_bytes =
            direct_ai_conversation_messages(vec![oversized_old_message, current], "latest prompt");
        assert_eq!(
            bounded_by_bytes,
            vec![DirectAiConversationMessage {
                role: "user",
                content: "latest prompt".to_string(),
            }]
        );
    }

    #[test]
    fn normalizes_split_codex_live_tool_and_file_events() {
        let mut normalizer = CodexLiveOutputNormalizer::default();
        let first = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"22222222-2222-4222-8222-222222222222\"}\n",
            "{\"type\":\"turn.started\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"reason-1\",\"type\":\"reasoning\",",
            "\"text\":\"Inspecting files\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"command-1\",\"type\":\"command_execution\",",
            "\"command\":\"pwd\",\"aggregated_output\":\"/tmp/project\\n\",\"exit_code\":0,",
            "\"status\":\"completed\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"change-1\",\"type\":\"file_change\",",
            "\"changes\":[{\"path\":\"created.txt\",\"kind\":\"add\"},",
            "{\"path\":\"updated.txt\",\"kind\":\"update\"},",
            "{\"path\":\"deleted.txt\",\"kind\":\"delete\"},",
            "{\"path\":\"moved.txt\",\"kind\":\"move\"}],\"status\":\"completed\"}}\n"
        );
        let split = first.len() / 2;
        let mut output = normalizer.push(&first[..split]);
        output.push_str(&normalizer.push(&first[split..]));
        output.push_str(&normalizer.finish());

        assert!(output.contains("thinking\nInspecting files"), "{output}");
        assert!(output.contains("exec / Parameters"), "{output}");
        assert!(output.contains("### Command\n```sh\npwd"), "{output}");
        assert!(output.contains("exec / Details"), "{output}");
        assert!(output.contains("create / created.txt"), "{output}");
        assert!(output.contains("edit / updated.txt"), "{output}");
        assert!(output.contains("delete / deleted.txt"), "{output}");
        assert!(output.contains("move / moved.txt"), "{output}");
        assert!(!output.contains("turn.started"), "{output}");
        assert_eq!(
            normalizer.take_thread_id().as_deref(),
            Some("22222222-2222-4222-8222-222222222222")
        );
        assert!(normalizer.take_thread_id().is_none());
    }

    #[test]
    fn normalizes_codex_agent_messages_and_apply_patch_without_duplicates() {
        let mut normalizer = CodexLiveOutputNormalizer::default();
        let output = normalizer.push(concat!(
            "{\"type\":\"item.started\",\"item\":{\"id\":\"patch-1\",\"type\":\"custom_tool_call\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"message-1\",\"type\":\"agent_message\",",
            "\"text\":\"I will update the file.\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"patch-1\",\"type\":\"custom_tool_call\",",
            "\"name\":\"apply_patch\",\"input\":\"*** Begin Patch\\n*** Add File: new.txt\\n+new\\n",
            "*** Update File: old.txt\\n-old\\n+updated\\n*** End Patch\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"message-2\",\"type\":\"agent_message\",",
            "\"text\":\"Both files are ready.\"}}\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":12,\"output_tokens\":8}}\n"
        ));

        assert!(
            output.contains("thinking\nI will update the file."),
            "{output}"
        );
        assert!(output.contains("apply_patch"), "{output}");
        assert!(output.contains("create / new.txt"), "{output}");
        assert!(output.contains("edit / old.txt"), "{output}");
        assert!(output.contains("```diff"), "{output}");
        assert!(output.contains("codex\nBoth files are ready."), "{output}");
        assert!(output.contains("tokens used"), "{output}");
        assert_eq!(output.matches("apply_patch").count(), 1, "{output}");
        assert_eq!(
            normalizer.take_final_assistant_message().as_deref(),
            Some("Both files are ready.")
        );
        let tools = normalizer.take_tool_messages();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "apply_patch");
    }

    #[test]
    fn codex_unphased_final_survives_trailing_todo_before_completion() {
        let mut normalizer = CodexLiveOutputNormalizer::default();
        let mut output = normalizer.push(concat!(
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"message-final\",\"type\":\"agent_message\",",
            "\"text\":\"Only one clean final response.\"}}\n"
        ));
        assert!(output.is_empty(), "{output}");

        output.push_str(&normalizer.push(concat!(
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"todo-final\",\"type\":\"todo_list\",",
            "\"items\":[{\"text\":\"Done\",\"completed\":true}]}}\n"
        )));
        assert!(
            !output.contains("Only one clean final response."),
            "{output}"
        );

        output.push_str(&normalizer.push(
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":6}}\n",
        ));

        assert!(
            output.contains("codex\nOnly one clean final response."),
            "{output}"
        );
        assert!(
            !output.contains("thinking\nOnly one clean final response."),
            "{output}"
        );
        assert_eq!(output.matches("Only one clean final response.").count(), 1);
        assert_eq!(
            normalizer.take_final_assistant_message().as_deref(),
            Some("Only one clean final response.")
        );
    }

    #[test]
    fn codex_explicit_final_remains_canonical_across_trailing_tools() {
        let mut normalizer = CodexLiveOutputNormalizer::default();
        let output = normalizer.push(concat!(
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"commentary\",\"type\":\"agent_message\",",
            "\"phase\":\"commentary\",\"text\":\"Checking the result.\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"command\",\"type\":\"command_execution\",",
            "\"command\":\"true\",\"aggregated_output\":\"\",\"exit_code\":0,\"status\":\"completed\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"final\",\"type\":\"agent_message\",",
            "\"phase\":\"final_answer\",\"text\":\"The final answer is stable.\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"todo\",\"type\":\"todo_list\",\"items\":[]}}\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":12,\"output_tokens\":8}}\n"
        ));

        assert!(
            output.contains("thinking\nChecking the result."),
            "{output}"
        );
        assert!(
            output.contains("codex\nThe final answer is stable."),
            "{output}"
        );
        assert_eq!(output.matches("The final answer is stable.").count(), 1);
        assert_eq!(
            normalizer.take_final_assistant_message().as_deref(),
            Some("The final answer is stable.")
        );
    }

    #[test]
    fn successful_codex_output_never_falls_back_to_live_transcript() {
        let transcript = concat!(
            "thinking\nInspecting files\n\n",
            "exec / Parameters\n**Tool:** `command_execution`\n\n",
            "codex\nThe actual final.\n\n",
            "tokens used\n{\"output_tokens\":8}"
        );

        assert_eq!(
            select_completed_agent_output(Provider::Codex, None, transcript, true),
            Err(CODEX_MISSING_FINAL_RESPONSE.to_string())
        );
        assert_eq!(
            select_completed_agent_output(
                Provider::Codex,
                Some("The actual final.".to_string()),
                transcript,
                true,
            ),
            Ok("The actual final.".to_string())
        );
        assert_eq!(
            select_completed_agent_output(Provider::Claude, None, transcript, false),
            Ok(transcript.to_string())
        );
        assert_eq!(
            select_completed_agent_output(Provider::Codex, None, "plain custom output", false),
            Ok("plain custom output".to_string())
        );
    }

    #[test]
    fn bounds_pathological_codex_tool_output_and_websocket_chunks() {
        let pathological = format!(
            "<style>body{{display:none}}</style><script>bad()</script>\0{}TAIL",
            "x".repeat(1_246_298)
        );
        let item = serde_json::json!({
            "type": "custom_tool_call_output",
            "name": "browser_output",
            "output": pathological,
        });
        let formatted = format_codex_live_tool_result(&item, "custom_tool_call");
        assert!(formatted.len() <= AGENT_TOOL_MESSAGE_MAX_BYTES);
        assert!(formatted.contains("truncated tool output"), "{formatted}");
        assert!(formatted.contains("TAIL"), "{formatted}");
        assert!(!formatted.contains('\0'));
        assert!(
            formatted
                .lines()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0)
                <= AGENT_DISPLAY_MAX_LINE_CHARS + 80
        );

        let chunks = websocket_text_chunks(&formatted);
        assert!(chunks.len() >= 2);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.len() <= AGENT_WEBSOCKET_CHUNK_MAX_BYTES)
        );
        assert_eq!(chunks.concat(), formatted);
    }

    #[test]
    fn codex_normalizer_separates_bounded_tool_rows_from_final_answer() {
        let event = serde_json::json!({
            "type": "item.completed",
            "item": {
                "type": "custom_tool_call_output",
                "name": "large_tool",
                "output": "z".repeat(200_000),
            }
        });
        let final_event = serde_json::json!({
            "type": "item.completed",
            "item": {
                "type": "agent_message",
                "phase": "final_answer",
                "text": "Only this is the final answer.",
            }
        });
        let mut normalizer = CodexLiveOutputNormalizer::default();
        let visible = normalizer.push(&format!("{event}\n{final_event}\n"));
        assert!(visible.contains("large_tool"));
        assert!(visible.contains("Only this is the final answer."));
        let tools = normalizer.take_tool_messages();
        assert_eq!(tools.len(), 1);
        assert!(tools[0].content.len() <= AGENT_TOOL_MESSAGE_MAX_BYTES);
        assert_eq!(
            normalizer.take_final_assistant_message().as_deref(),
            Some("Only this is the final answer.")
        );
    }

    #[test]
    fn codex_live_normalizer_preserves_plain_output_and_partial_last_line() {
        let mut normalizer = CodexLiveOutputNormalizer::default();
        assert_eq!(normalizer.push("plain out"), "");
        assert_eq!(normalizer.push("put\n"), "plain output\n");
        assert_eq!(normalizer.push("last line"), "");
        assert_eq!(normalizer.finish(), "last line\n");
    }

    #[test]
    fn run_usage_normalizers_keep_total_and_subset_fields_separate() {
        let codex = normalize_codex_run_usage(&serde_json::json!({
            "input_tokens": 30,
            "cached_input_tokens": 12,
            "cache_write_input_tokens": 3,
            "output_tokens": 12,
            "reasoning_output_tokens": 5,
            "total_tokens": 42
        }));
        assert_eq!(codex.usage.used, 42);
        assert_eq!(codex.usage.input, 30);
        assert_eq!(codex.usage.output, 12);
        assert_eq!(codex.usage.cache_read, 12);
        assert_eq!(codex.usage.cache_creation, 3);
        assert_eq!(codex.usage.reasoning, 5);

        let claude = normalize_claude_run_usage(&serde_json::json!({
            "type": "result",
            "modelUsage": {
                "sonnet": {
                    "input_tokens": 10,
                    "cache_creation_input_tokens": 20,
                    "cache_read_input_tokens": 30,
                    "output_tokens": 40
                },
                "haiku": {
                    "input_tokens": 1,
                    "output_tokens": 4
                }
            },
            "total_cost_usd": 0.02
        }));
        assert_eq!(claude.usage.used, 55);
        assert_eq!(claude.usage.input, 11);
        assert_eq!(claude.usage.output, 44);
        assert_eq!(claude.usage.cache_creation, 20);
        assert_eq!(claude.usage.cache_read, 30);
        assert_eq!(claude.usage.cost_usd, 0.02);

        let gemini = normalize_gemini_run_usage(&serde_json::json!({
            "type": "result",
            "stats": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 25,
                "cachedContentTokenCount": 80,
                "thoughtsTokenCount": 7,
                "totalTokenCount": 125
            }
        }))
        .expect("gemini usage");
        assert_eq!(gemini.usage.used, 125);
        assert_eq!(gemini.usage.input, 100);
        assert_eq!(gemini.usage.output, 25);
        assert_eq!(gemini.usage.cache_read, 80);
        assert_eq!(gemini.usage.reasoning, 7);
    }

    #[test]
    fn claude_and_gemini_normalizers_capture_native_session_ids() {
        let mut claude = ClaudeLiveOutputNormalizer::default();
        let claude_output = claude.push_chunks(concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"claude-native\"}\n",
            "{\"type\":\"stream_event\",\"session_id\":\"claude-native\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"continued\"}}}\n"
        ));
        assert_eq!(claude_output, ["claude\ncontinued"]);
        assert_eq!(claude.take_session_id().as_deref(), Some("claude-native"));
        assert!(
            claude
                .push_chunks(
                    "{\"type\":\"stream_event\",\"session_id\":\"claude-native\",\"event\":{\"type\":\"ping\"}}\n"
                )
                .is_empty()
        );
        assert_eq!(claude.take_session_id(), None);

        let mut gemini = GeminiLiveOutputNormalizer::default();
        let gemini_output = gemini.push(concat!(
            "{\"type\":\"init\",\"session_id\":\"gemini-native\"}\n",
            "{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"continued\",\"delta\":true}\n"
        ));
        assert_eq!(gemini_output, "continued");
        assert_eq!(gemini.take_session_id().as_deref(), Some("gemini-native"));
    }

    #[test]
    fn claude_normalizer_streams_wrapped_deltas_without_repeating_final_result() {
        let mut claude = ClaudeLiveOutputNormalizer::default();

        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello \"}}}\n"
            ),
            ["claude\nHello "]
        );
        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"mobile\"}}}\n"
            ),
            ["mobile"]
        );
        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"Hello mobile\"}\n"
            ),
            Vec::<String>::new()
        );
        assert_eq!(claude.finish(), "");
    }

    #[test]
    fn claude_normalizer_streams_thinking_before_final_text() {
        let mut claude = ClaudeLiveOutputNormalizer::default();

        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Inspecting files\"}}}\n"
            ),
            ["thinking\nInspecting files"]
        );
        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\" now\"}}}\n"
            ),
            [" now"]
        );
        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Finished.\"}}}\n"
            ),
            ["\n\nclaude\nFinished."]
        );
        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"Finished.\"}\n"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn claude_normalizer_formats_tool_use_sections() {
        let mut claude = ClaudeLiveOutputNormalizer::default();

        let output = claude.push_chunks(concat!(
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"Bash\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"pwd\\\"}\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_stop\"}}\n",
            "{\"type\":\"tool_result\",\"tool_use_id\":\"toolu_1\",\"name\":\"Bash\",\"content\":\"/tmp/project\\n\"}\n"
        ));

        assert_eq!(output.len(), 2);
        assert!(output[0].contains("exec / Parameters"), "{output:?}");
        assert!(output[0].contains("**Tool:** `Bash`"), "{output:?}");
        assert!(output[0].contains("### Command"), "{output:?}");
        assert!(output[0].contains("pwd"), "{output:?}");
        assert!(output[1].contains("exec / Details"), "{output:?}");
        assert!(output[1].contains("/tmp/project"), "{output:?}");
    }

    #[test]
    fn claude_normalizer_formats_message_enveloped_thinking_and_tools() {
        let mut claude = ClaudeLiveOutputNormalizer::default();

        let output = claude.push_chunks(concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":[",
            "{\"type\":\"thinking\",\"thinking\":\"Checking files\"},",
            "{\"type\":\"tool_use\",\"id\":\"toolu_2\",\"name\":\"Read\",\"input\":{\"file_path\":\"Cargo.toml\"}},",
            "{\"type\":\"text\",\"text\":\"Done.\"}",
            "]}}\n"
        ));

        assert_eq!(output.len(), 1);
        assert!(output[0].contains("thinking\nChecking files"), "{output:?}");
        assert!(output[0].contains("tool / Parameters"), "{output:?}");
        assert!(output[0].contains("**Tool:** `Read`"), "{output:?}");
        assert!(
            output[0].contains("\"file_path\": \"Cargo.toml\""),
            "{output:?}"
        );
        assert!(output[0].contains("Done."), "{output:?}");
    }

    #[test]
    fn claude_normalizer_prefers_stream_events_over_duplicate_message_envelopes() {
        let mut claude = ClaudeLiveOutputNormalizer::default();

        let output = claude.push_chunks(concat!(
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Checking Cargo.toml presence\"}}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[",
            "{\"type\":\"thinking\",\"thinking\":\"Checking Cargo.toml presence\"},",
            "{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"Bash\",\"input\":{\"command\":\"pwd && ls Cargo.toml\"}},",
            "{\"type\":\"text\",\"text\":\"Cargo.toml exists.\"}",
            "]}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"Bash\",\"input\":{}}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"pwd && ls Cargo.toml\\\"}\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_stop\"}}\n",
            "{\"type\":\"tool_result\",\"tool_use_id\":\"call_1\",\"content\":\"/tmp/project\\nCargo.toml\\n\"}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Cargo.toml exists.\"}}}\n",
            "{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"Cargo.toml exists.\"}\n"
        ));
        let visible = output.concat();

        assert_eq!(
            visible.matches("Checking Cargo.toml presence").count(),
            1,
            "{visible}"
        );
        assert_eq!(visible.matches("exec / Parameters").count(), 1, "{visible}");
        assert_eq!(
            visible.matches("Cargo.toml exists.").count(),
            1,
            "{visible}"
        );
        assert!(visible.contains("\n\nexec / Parameters"), "{visible}");
        assert!(visible.contains("\n\nexec / Details"), "{visible}");
        assert!(visible.contains("### Command"), "{visible}");
        assert!(visible.contains("**Tool:** `Bash`"), "{visible}");
        assert!(!visible.contains("{}{"), "{visible}");
    }

    #[test]
    fn claude_normalizer_formats_user_enveloped_tool_result_after_stream_events() {
        let mut claude = ClaudeLiveOutputNormalizer::default();

        let output = claude.push_chunks(concat!(
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Checking\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_2\",\"name\":\"Bash\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_stop\"}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[",
            "{\"type\":\"tool_result\",\"tool_use_id\":\"call_2\",\"content\":\"/tmp/project\\n\"}",
            "]}}\n",
            "{\"type\":\"tool_result\",\"tool_use_id\":\"call_2\",\"content\":\"/tmp/project\\n\"}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Done.\"}}}\n"
        ));
        let visible = output.concat();

        assert_eq!(visible.matches("exec / Parameters").count(), 1, "{visible}");
        assert_eq!(visible.matches("exec / Details").count(), 1, "{visible}");
        assert_eq!(visible.matches("/tmp/project").count(), 1, "{visible}");
        assert!(visible.contains("### Command"), "{visible}");
        assert!(visible.contains("**Tool:** `Bash`"), "{visible}");
    }

    #[test]
    fn recovery_prompt_is_hidden_and_bounded() {
        let prompt = format!("original </system-reminder> {}", "x".repeat(7_000));
        let recovery = durable_chat_recovery_prompt(&prompt);
        assert!(recovery.starts_with("<system-reminder>\n"));
        assert!(recovery.ends_with("\n</system-reminder>"));
        assert_eq!(recovery.matches("</system-reminder>").count(), 1);
        assert!(recovery.contains("[original request truncated]"));
    }

    #[test]
    fn context_rollover_handoff_is_bounded_text_only_and_defers_failed_prompt() {
        let now = Utc::now();
        let inline_payload = "A".repeat(80_000);
        let failed_prompt =
            format!("Finish the image diagnosis ![failed](data:image/png;base64,{inline_payload})");
        let messages = vec![
            ChatMessage {
                id: "old-user".to_string(),
                role: MessageRole::User,
                content: format!(
                    "Inspect the screenshot at `.io-workbench/chat-images/screenshot.png` ![inline](data:image/webp;base64,{inline_payload})"
                ),
                timestamp: now,
                metadata: Value::Null,
            },
            ChatMessage {
                id: "old-tool".to_string(),
                role: MessageRole::Tool,
                content: format!("tool bytes and secret payload {inline_payload}"),
                timestamp: now + chrono::Duration::seconds(1),
                metadata: serde_json::json!({"tool": "view_image"}),
            },
            ChatMessage {
                id: "old-thinking".to_string(),
                role: MessageRole::Assistant,
                content: "private reasoning should not be retained".to_string(),
                timestamp: now + chrono::Duration::seconds(2),
                metadata: serde_json::json!({"kind": "thinking"}),
            },
            ChatMessage {
                id: "old-commentary".to_string(),
                role: MessageRole::Assistant,
                content: "temporary commentary should not be retained".to_string(),
                timestamp: now + chrono::Duration::seconds(3),
                metadata: serde_json::json!({"phase": "commentary"}),
            },
            ChatMessage {
                id: "old-assistant".to_string(),
                role: MessageRole::Assistant,
                content:
                    "The request failed because the native context exceeded the gateway limit."
                        .to_string(),
                timestamp: now + chrono::Duration::seconds(4),
                metadata: Value::Null,
            },
            ChatMessage {
                id: "failed-user".to_string(),
                role: MessageRole::User,
                content: failed_prompt.clone(),
                timestamp: now + chrono::Duration::seconds(5),
                metadata: Value::Null,
            },
        ];

        let handoff = build_context_rollover_handoff(messages, &failed_prompt);

        assert!(handoff.starts_with("<system-reminder>\n"));
        assert!(handoff.contains("Recent text-only handoff:"));
        assert!(handoff.contains(".io-workbench/chat-images/screenshot.png"));
        assert!(handoff.contains("native context exceeded the gateway limit"));
        assert!(handoff.contains("[inline image omitted; use its local file path if available]"));
        assert!(!handoff.contains(";base64,"));
        assert!(!handoff.contains(&inline_payload));
        assert!(!handoff.contains("tool bytes and secret payload"));
        assert!(!handoff.contains("private reasoning should not be retained"));
        assert!(!handoff.contains("temporary commentary should not be retained"));
        assert_eq!(handoff.matches("Finish the image diagnosis").count(), 0);
        assert!(
            handoff.len() <= CONTEXT_ROLLOVER_HANDOFF_MAX_BYTES + 2 * 1024,
            "handoff unexpectedly large: {} bytes",
            handoff.len()
        );
    }

    #[test]
    fn context_rollover_handoff_keeps_newest_text_that_fits() {
        let now = Utc::now();
        let mut messages = (0..40)
            .map(|index| ChatMessage {
                id: format!("message-{index:02}"),
                role: if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                content: format!("history-{index:02} {}", "x".repeat(3_000)),
                timestamp: now + chrono::Duration::seconds(index),
                metadata: Value::Null,
            })
            .collect::<Vec<_>>();
        messages.push(ChatMessage {
            id: "failed-user".to_string(),
            role: MessageRole::User,
            content: "retry newest request".to_string(),
            timestamp: now + chrono::Duration::seconds(41),
            metadata: Value::Null,
        });

        let handoff = build_context_rollover_handoff(messages, "retry newest request");

        assert!(handoff.contains("history-39"));
        assert!(!handoff.contains("history-00"));
        assert_eq!(handoff.matches("retry newest request").count(), 0);
        assert!(
            handoff.len() <= CONTEXT_ROLLOVER_HANDOFF_MAX_BYTES + 2 * 1024,
            "handoff unexpectedly large: {} bytes",
            handoff.len()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn failed_context_rollover_discards_provisional_codex_tool_rows() {
        let (state, root, project) = temporary_app_state("rollover-tool-rollback").await;
        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-rollover-tool-rollback".to_string()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        let failed_message = state
            .sessions
            .append_message(
                &session.id,
                MessageRole::User,
                "retry without leaking tools",
            )
            .await
            .expect("failed prompt");
        let mut trigger_run = StoredDurableChatRun::new(
            "run-rollover-tool-trigger",
            Some("user-1".to_string()),
            session.id.clone(),
            Provider::Codex.as_str(),
            failed_message.content.clone(),
            project.display().to_string(),
        );
        trigger_run.user_message_id = Some(failed_message.id.clone());
        state
            .storage
            .create_durable_chat_run(&trigger_run)
            .expect("trigger run");
        state
            .storage
            .mark_durable_chat_run_failed(&trigger_run.id, "invalid body")
            .expect("failed trigger");
        let now = Utc::now();
        let rollover = StoredSessionContextRollover {
            id: "rollover-tool-rollback".to_string(),
            user_id: "user-1".to_string(),
            session_id: session.id.clone(),
            request_id: "request-rollover-tool-rollback".to_string(),
            kind: CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN.to_string(),
            failed_message_id: failed_message.id.clone(),
            trigger_run_id: trigger_run.id.clone(),
            retry_run_id: "run-rollover-tool-retry".to_string(),
            from_native_session_id: Some("native-poisoned".to_string()),
            candidate_native_session_id: None,
            state: "starting".to_string(),
            handoff: "bounded handoff".to_string(),
            observed_bytes: Some(CODEX_CONTEXT_ROLLOVER_THRESHOLD_BYTES),
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut retry_run = StoredDurableChatRun::new(
            rollover.retry_run_id.clone(),
            Some("user-1".to_string()),
            session.id.clone(),
            Provider::Codex.as_str(),
            rollover.handoff.clone(),
            project.display().to_string(),
        );
        retry_run.user_message_id = Some(failed_message.id.clone());
        assert!(
            state
                .storage
                .prepare_context_rollover(&rollover, &retry_run)
                .expect("prepare rollover")
        );
        let before = state
            .storage
            .list_messages(&session.id)
            .expect("baseline transcript");
        let context = AgentStartContext {
            provider: Provider::Codex,
            session_id: session.id.clone(),
            durable_run_id: Some(retry_run.id.clone()),
            attempt_id: None,
            response_id: retry_run.id.clone(),
            sequence: Arc::new(AtomicU64::new(0)),
            project_path: project.clone(),
            prompt: rollover.handoff.clone(),
            model: None,
            runtime: ChatRuntime::NativeCli,
            effort: None,
            mode: None,
            thinking: None,
            fast: None,
            native_resume_session_id: None,
            context_rollover_id: Some(rollover.id.clone()),
            direct_ai_config: None,
            direct_ai_messages: Vec::new(),
            sessions: state.sessions.clone(),
            storage: state.storage.clone(),
            hub: WsHub::new(),
        };
        let mut normalizer = Some(CodexLiveOutputNormalizer::default());
        normalizer.as_mut().expect("normalizer").push(&format!(
            "{}\n",
            serde_json::json!({
                "type": "item.completed",
                "item": {
                    "type": "custom_tool_call_output",
                    "name": "view_image",
                    "output": "provisional image analysis that must not persist"
                }
            })
        ));
        persist_codex_tool_messages(&context, &mut normalizer).await;
        AgentRuntimeManager::default()
            .finish(
                "codex:session-rollover-tool-rollback",
                &context,
                iowb_protocol::SessionRuntimeStatus::Failed,
                Some("clean retry failed".to_string()),
                None,
            )
            .await;

        let after = state
            .storage
            .list_messages(&session.id)
            .expect("transcript after failed rollover");
        let transcript_identity = |messages: &[ChatMessage]| {
            messages
                .iter()
                .map(|message| (message.id.clone(), message.role, message.content.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            transcript_identity(&after),
            transcript_identity(&before),
            "a failed rollover must not append provisional tool or assistant rows"
        );
        assert_eq!(
            state
                .storage
                .context_rollover_for_retry_run(&retry_run.id)
                .expect("rollover lookup")
                .expect("rollover")
                .state,
            "failed"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn manual_compaction_uses_native_compact_and_replaces_polluted_local_projection() {
        use std::os::unix::fs::PermissionsExt;

        let (mut state, root, project) = temporary_app_state("native-manual-compact").await;
        let script = root.join("compact-codex.sh");
        let log = root.join("compact-requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"args:$*\" >> '{}'\n\
                 printf '%s\\n' \"gateway:${{IOWB_IO_GATEWAY_API_KEY:-}}\" >> '{}'\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"native-compact\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/completed\",\"params\":{{\"threadId\":\"native-compact\",\"item\":{{\"type\":\"contextCompaction\",\"id\":\"item-compact\"}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
                log.display(),
                log.display(),
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");
        state.codex_app_server =
            CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));

        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-native-manual-compact".to_string()),
                false,
                None,
                Some(ChatRuntime::IoGateway),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        state
            .sessions
            .set_native_session_id(&session.id, "native-compact")
            .await
            .expect("native id");
        state
            .sessions
            .append_message(&session.id, MessageRole::User, "Question before compact")
            .await
            .expect("user message");
        let transcript = concat!(
            "thinking\nInspecting files\n\n",
            "exec / Parameters\n**Tool:** `command_execution`\n\n",
            "codex\nThis whole blob must not replay as the answer.\n\n",
            "tokens used\n{\"output_tokens\":8}"
        );
        state
            .sessions
            .append_message_with_metadata(
                &session.id,
                MessageRole::Assistant,
                transcript,
                Some(serde_json::json!({
                    "cli": "codex",
                    "durableRunId": "run-poisoned",
                })),
            )
            .await
            .expect("polluted assistant row");
        state
            .sessions
            .append_message(&session.id, MessageRole::Assistant, "Actual final answer.")
            .await
            .expect("assistant message");
        state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("idle session");

        let response = state
            .compact_session_context(
                "user-1",
                &session.id,
                "request-native-compact",
                Some(DirectAiRuntimeConfig {
                    base_url: "https://gateway.example.com/codex/".to_string(),
                    api_key: "test-secret".to_string(),
                    max_tokens: None,
                }),
            )
            .await
            .expect("manual compact");
        assert_eq!(response.state, "starting");
        wait_for_context_rollover_state(&state, &response.response_id, "active").await;
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("args:app-server"));
        assert!(requests.contains("model_provider=iowb_gateway"));
        assert!(
            requests.contains(
                "model_providers.iowb_gateway.base_url=\"https://gateway.example.com/codex\""
            ),
            "{requests}"
        );
        assert!(
            requests.contains("model_providers.iowb_gateway.env_key=\"IOWB_IO_GATEWAY_API_KEY\"")
        );
        assert!(requests.contains("gateway:test-secret"));
        assert!(requests.contains("\"method\":\"thread/resume\""));
        assert!(requests.contains("\"method\":\"thread/compact/start\""));

        let messages = state
            .sessions
            .messages_including_external(&session.id)
            .await
            .expect("messages after compact");
        let contents = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(contents.len(), 3, "{contents:#?}");
        assert!(contents.contains(&"Question before compact"));
        assert!(contents.contains(&"Actual final answer."));
        assert!(
            contents
                .iter()
                .any(|content| content.contains("Context compacted here"))
        );
        assert!(
            contents
                .iter()
                .all(|content| !content.contains("exec / Parameters")),
            "{contents:#?}"
        );

        let stored_run = state
            .storage
            .get_durable_chat_run(&response.response_id)
            .expect("run lookup")
            .expect("run");
        assert_eq!(stored_run.status, "completed");
        assert_eq!(
            stored_run.native_session_id.as_deref(),
            Some("native-compact")
        );
        assert!(!stored_run.auto_resume);

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn manual_compaction_returns_before_slow_app_server_finishes() {
        use std::os::unix::fs::PermissionsExt;

        let (mut state, root, project) = temporary_app_state("native-manual-compact-async").await;
        let script = root.join("compact-codex.sh");
        let log = root.join("compact-requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"native-slow-compact\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 sleep 2\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/completed\",\"params\":{{\"threadId\":\"native-slow-compact\",\"item\":{{\"type\":\"contextCompaction\",\"id\":\"item-compact\"}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");
        state.codex_app_server =
            CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(4));

        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-native-slow-manual-compact".to_string()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        state
            .sessions
            .set_native_session_id(&session.id, "native-slow-compact")
            .await
            .expect("native id");
        state
            .sessions
            .append_message(
                &session.id,
                MessageRole::User,
                "Question before slow compact",
            )
            .await
            .expect("user message");
        state
            .sessions
            .append_message(
                &session.id,
                MessageRole::Assistant,
                "Answer before slow compact",
            )
            .await
            .expect("assistant message");
        state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("idle session");

        let response = timeout(
            Duration::from_secs(1),
            state.compact_session_context(
                "user-1",
                &session.id,
                "request-native-slow-compact",
                None,
            ),
        )
        .await
        .expect("manual compact should return before app-server compaction completes")
        .expect("manual compact");
        assert_eq!(response.state, "starting");
        assert_eq!(
            state
                .storage
                .get_durable_chat_run(&response.response_id)
                .expect("run lookup")
                .expect("run")
                .status,
            "running"
        );

        wait_for_context_rollover_state(&state, &response.response_id, "active").await;
        assert_eq!(
            state
                .storage
                .get_durable_chat_run(&response.response_id)
                .expect("run lookup")
                .expect("run")
                .status,
            "completed"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn manual_compaction_rekeys_external_projection_messages_before_replace() {
        use std::os::unix::fs::PermissionsExt;

        let (mut state, root, project) = temporary_app_state("native-manual-compact-rekey").await;
        state.sessions.external_home = Arc::new(root.clone());
        let native_id = "99999999-9999-4999-8999-999999999999";
        let script = root.join("compact-codex.sh");
        let log = root.join("compact-requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"{native_id}\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/completed\",\"params\":{{\"threadId\":\"{native_id}\",\"item\":{{\"type\":\"contextCompaction\",\"id\":\"item-compact\"}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");
        state.codex_app_server =
            CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));

        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/15")
            .join(format!("rollout-2026-08-15T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now + chrono::Duration::milliseconds(1),
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "Native prompt to compact",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now + chrono::Duration::milliseconds(2),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "id": "native-answer",
                        "role": "assistant",
                        "phase": "final_answer",
                        "content": [{"type": "output_text", "text": "Native answer to keep."}]
                    }
                })
            ),
        )
        .expect("rollout");

        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-native-manual-compact-rekey".to_string()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        state
            .sessions
            .set_native_session_id(&session.id, native_id)
            .await
            .expect("native id");
        let visible_before = state
            .sessions
            .messages_including_external(&session.id)
            .await
            .expect("visible messages before compact");
        let external_user_id = visible_before
            .iter()
            .find(|message| message.content == "Native prompt to compact")
            .map(|message| message.id.clone())
            .expect("external user message");
        assert!(
            visible_before
                .iter()
                .any(|message| message.id == external_user_id),
            "{visible_before:#?}"
        );
        let other = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-with-existing-external-id".to_string()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("other session");
        state
            .storage
            .append_message(
                &other.id,
                &ChatMessage {
                    id: external_user_id.clone(),
                    role: MessageRole::User,
                    content: "Existing materialized native prompt".to_string(),
                    timestamp: now,
                    metadata: Value::Null,
                },
            )
            .expect("colliding message");
        state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("idle session");

        let response = state
            .compact_session_context("user-1", &session.id, "request-native-rekey-compact", None)
            .await
            .expect("manual compact");
        assert_eq!(response.state, "starting");
        wait_for_context_rollover_state(&state, &response.response_id, "active").await;

        let stored = state
            .storage
            .list_messages(&session.id)
            .expect("stored messages after compact");
        let user = stored
            .iter()
            .find(|message| message.content == "Native prompt to compact")
            .expect("materialized user message");
        assert_ne!(user.id, external_user_id);
        assert!(user.id.starts_with("msg_"));
        assert_eq!(
            user.metadata["contextMaterializedFromMessageId"],
            external_user_id
        );
        assert_eq!(user.metadata["usageSourceMessageId"], external_user_id);
        assert!(
            stored
                .iter()
                .any(|message| message.content == "Native answer to keep.")
        );
        assert!(stored.iter().all(|message| {
            !message
                .id
                .starts_with(&format!("external_codex_{native_id}_"))
        }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn active_compacted_session_reloads_post_compact_native_response() {
        let (mut state, root, project) = temporary_app_state("active-compact-native-merge").await;
        state.sessions.external_home = Arc::new(root.clone());
        let native_id = "77777777-7777-4777-8777-777777777777";
        let before_compact = Utc::now() - chrono::Duration::seconds(30);
        let compacted_at = Utc::now() - chrono::Duration::seconds(20);
        let after_compact = Utc::now() - chrono::Duration::seconds(10);
        let rollout = root
            .join(".codex/sessions/2026/08/15")
            .join(format!("rollout-2026-08-15T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        let legacy_transcript = concat!(
            "thinking\nInspecting files\n\n",
            "exec / Parameters\n**Tool:** `command_execution`\n\n",
            "codex\nPost-compact native answer.\n\n",
            "tokens used\n{\"output_tokens\":8}"
        );
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": before_compact,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": before_compact,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "Pre-compact native prompt that should not be imported",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": after_compact,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "Post-compact prompt",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": after_compact + chrono::Duration::milliseconds(1),
                    "type": "response_item",
                    "payload": {
                        "type": "reasoning",
                        "summary": [{"type": "summary_text", "text": "Checking the active compacted thread"}]
                    }
                }),
                serde_json::json!({
                    "timestamp": after_compact + chrono::Duration::milliseconds(2),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "id": "native-final-after-compact",
                        "role": "assistant",
                        "phase": "final_answer",
                        "content": [{"type": "output_text", "text": "Post-compact native answer."}]
                    }
                }),
                serde_json::json!({
                    "timestamp": after_compact + chrono::Duration::milliseconds(3),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "id": "legacy-workbench-transcript",
                        "role": "assistant",
                        "source": "io-workbench",
                        "content": [{"type": "output_text", "text": legacy_transcript}]
                    }
                })
            ),
        )
        .expect("rollout");

        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("active-compacted-workbench-session".to_string()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        state
            .sessions
            .set_native_session_id(&session.id, native_id)
            .await
            .expect("native id");
        state
            .storage
            .append_message(
                &session.id,
                &ChatMessage {
                    id: new_id("msg"),
                    role: MessageRole::User,
                    content: "Pre-compact Workbench prompt".to_string(),
                    timestamp: before_compact,
                    metadata: Value::Null,
                },
            )
            .expect("pre compact user");
        state
            .storage
            .append_message(
                &session.id,
                &ChatMessage {
                    id: new_id("msg"),
                    role: MessageRole::Assistant,
                    content: "Pre-compact Workbench answer".to_string(),
                    timestamp: before_compact + chrono::Duration::milliseconds(1),
                    metadata: Value::Null,
                },
            )
            .expect("pre compact assistant");
        state
            .storage
            .append_message(
                &session.id,
                &ChatMessage {
                    id: new_id("msg"),
                    role: MessageRole::User,
                    content: "Post-compact prompt".to_string(),
                    timestamp: after_compact,
                    metadata: Value::Null,
                },
            )
            .expect("post compact user");

        let mut run = StoredDurableChatRun::new(
            "run-active-compacted-native-merge",
            Some("user-1".to_string()),
            session.id.clone(),
            Provider::Codex.as_str(),
            "Native Codex context compaction".to_string(),
            project.display().to_string(),
        );
        run.native_session_id = Some(native_id.to_string());
        let rollover = StoredSessionContextRollover {
            id: "rollover-active-compacted-native-merge".to_string(),
            user_id: "user-1".to_string(),
            session_id: session.id.clone(),
            request_id: "request-active-compacted-native-merge".to_string(),
            kind: CONTEXT_ROLLOVER_KIND_MANUAL.to_string(),
            failed_message_id: String::new(),
            trigger_run_id: run.id.clone(),
            retry_run_id: run.id.clone(),
            from_native_session_id: Some(native_id.to_string()),
            candidate_native_session_id: Some(native_id.to_string()),
            state: "starting".to_string(),
            handoff: "Native Codex context compaction".to_string(),
            observed_bytes: None,
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: compacted_at,
            updated_at: compacted_at,
            activated_at: None,
        };
        assert!(
            state
                .storage
                .prepare_manual_context_rollover(&rollover, &run)
                .expect("prepare rollover")
        );
        let marker = ChatMessage {
            id: new_id("msg"),
            role: MessageRole::System,
            content: "Context compacted here. Earlier messages remain visible, while subsequent replies use a clean Codex context.".to_string(),
            timestamp: compacted_at,
            metadata: serde_json::json!({"kind": "context_compaction"}),
        };
        let mut stored_session = state
            .sessions
            .get(&session.id)
            .await
            .expect("stored session");
        stored_session.native_session_id = Some(native_id.to_string());
        stored_session.external = false;
        assert!(
            state
                .storage
                .complete_context_rollover(
                    &rollover.id,
                    &run.id,
                    native_id,
                    &stored_session,
                    &marker,
                    None,
                    None,
                )
                .expect("complete rollover")
        );

        let messages = state
            .sessions
            .messages_including_external(&session.id)
            .await
            .expect("messages");
        let contents = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert!(
            contents.contains(&"Pre-compact Workbench prompt"),
            "{contents:#?}"
        );
        assert!(
            contents.contains(&"Pre-compact Workbench answer"),
            "{contents:#?}"
        );
        assert!(contents.contains(&"Post-compact prompt"), "{contents:#?}");
        assert!(
            contents.contains(&"Post-compact native answer."),
            "{contents:#?}"
        );
        assert!(
            contents
                .iter()
                .any(|content| content.starts_with("thinking\n")),
            "{contents:#?}"
        );
        assert!(
            contents
                .iter()
                .all(|content| !content.contains("exec / Parameters")),
            "{contents:#?}"
        );
        assert!(
            contents
                .iter()
                .all(|content| !content.contains("Pre-compact native prompt")),
            "{contents:#?}"
        );
        assert_eq!(
            1,
            contents
                .iter()
                .filter(|content| **content == "Post-compact prompt")
                .count(),
            "{contents:#?}"
        );

        let (tail, total) = state
            .sessions
            .messages_tail_including_external(&session.id, 3)
            .await
            .expect("tail");
        assert_eq!(total, messages.len());
        assert!(
            tail.iter()
                .any(|message| message.content == "Post-compact native answer."),
            "{tail:#?}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn token_usage_persists_and_clears_when_the_next_turn_starts() {
        let root = std::env::temp_dir().join(format!("iowb-token-usage-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("root");
        let storage = Storage::open(root.join("test.db")).expect("storage");
        let sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        let session = sessions
            .create_or_update(
                Provider::Codex,
                root.display().to_string(),
                None,
                false,
                Some("gpt-test".to_string()),
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("create session");

        sessions
            .set_token_usage(
                &session.id,
                SessionTokenUsage {
                    used: 4_321,
                    input: 1_500,
                    output: 2_700,
                    cache_creation: 0,
                    cache_read: 121,
                    reasoning: 0,
                    cost_usd: 0.0,
                },
            )
            .await
            .expect("store token usage");

        let restored = storage
            .get_session(&session.id)
            .expect("stored session")
            .expect("session exists");
        assert_eq!(
            restored.token_usage.as_ref().map(|usage| usage.used),
            Some(4_321)
        );
        assert_eq!(
            sessions
                .get(&session.id)
                .await
                .expect("cached session")
                .token_usage
                .as_ref()
                .map(|usage| usage.cache_read),
            Some(121),
        );

        let restarted = sessions
            .create_or_update(
                Provider::Codex,
                root.display().to_string(),
                Some(session.id.clone()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("restart session");
        assert!(restarted.token_usage.is_none());
        assert!(
            storage
                .get_session(&session.id)
                .expect("stored session")
                .expect("session exists")
                .token_usage
                .is_none()
        );

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn loading_preserves_active_sessions_until_startup_reconciliation() {
        let root = std::env::temp_dir().join(format!("iowb-stale-session-{}", Uuid::new_v4()));
        let config_dir = root.join("config");
        std::fs::create_dir_all(&config_dir).expect("config dir");
        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        let now = Utc::now();
        let session = SessionSummary {
            id: "stale-session".to_string(),
            provider: Provider::Codex,
            external: false,
            board_session: false,
            board_run_id: None,
            board_task_id: None,
            project_path: root.display().to_string(),
            title: "Interrupted chat".to_string(),
            message_count: 1,
            last_activity: now,
            active: true,
            model: Some("gpt-test".to_string()),
            runtime: Some(ChatRuntime::NativeCli),
            effort: Some("medium".to_string()),
            mode: Some("default".to_string()),
            thinking: Some(false),
            fast: Some(false),
            last_message_at: Some(now),
            first_user_at: Some(now),
            received_at: None,
            token_usage: None,
            lifetime_token_usage: None,
            native_session_id: Some("native-session".to_string()),
            title_source: Some(SessionTitleSource::Manual),
        };
        storage
            .upsert_session(&session)
            .expect("upsert active session");
        storage
            .append_message(
                "stale-session",
                &ChatMessage {
                    id: "msg-user".to_string(),
                    role: MessageRole::User,
                    content: "please continue".to_string(),
                    timestamp: now,
                    metadata: Value::Null,
                },
            )
            .expect("append user message");

        let sessions = SessionManager::load(storage.clone(), 10).expect("sessions");

        assert_eq!(sessions.list_active().await.len(), 1);
        sessions
            .mark_unrecovered_active_sessions_interrupted(&HashSet::new())
            .await
            .expect("reconcile stale session");
        assert!(sessions.list_active().await.is_empty());
        let stored = storage
            .get_session("stale-session")
            .expect("stored session")
            .expect("session exists");
        assert!(!stored.active);
        assert_eq!(stored.message_count, 2);
        let messages = storage
            .list_messages("stale-session")
            .expect("stored messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, MessageRole::System);
        assert!(messages[1].content.contains("Server restarted"));
        assert_eq!(
            messages[1].metadata["reason"].as_str(),
            Some("server_restart")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_codex_thread_mapping_persists_resumes_and_hides_rollout() {
        let root = std::env::temp_dir().join(format!("iowb-native-thread-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        let native_id = "22222222-2222-4222-8222-222222222222";
        let historical_native_id = "33333333-3333-4333-8333-333333333333";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/07/31")
            .join(format!("rollout-2026-07-31T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "first prompt",
                        "kind": "plain"
                    }
                })
            ),
        )
        .expect("rollout");
        let historical_rollout = rollout.parent().expect("rollout parent").join(format!(
            "rollout-2026-07-30T23-59-00-{historical_native_id}.jsonl"
        ));
        std::fs::write(
            historical_rollout,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": now - chrono::Duration::seconds(60),
                    "type": "session_meta",
                    "payload": {"id": historical_native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now - chrono::Duration::seconds(60),
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "older prompt",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now - chrono::Duration::seconds(59),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "older answer"}]
                    }
                })
            ),
        )
        .expect("historical rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        storage
            .upsert_session(&SessionSummary {
                id: historical_native_id.to_string(),
                provider: Provider::Codex,
                external: true,
                project_path: project.display().to_string(),
                title: "Stored historical session".to_string(),
                message_count: 1,
                last_activity: now - chrono::Duration::seconds(60),
                ..Default::default()
            })
            .expect("stored historical external session");
        let mut historical_attempt = StoredChatRunAttempt::new(
            "attempt-historical-usage",
            "run-historical-usage",
            historical_native_id,
            None,
            "codex",
            "legacy_history",
            None,
            Some(historical_native_id.to_string()),
        );
        historical_attempt.status = "completed".to_string();
        historical_attempt.usage = Some(SessionTokenUsage {
            used: 42,
            input: 30,
            output: 12,
            cache_creation: 0,
            cache_read: 0,
            reasoning: 0,
            cost_usd: 0.0,
        });
        historical_attempt.source = Some("test".to_string());
        historical_attempt.completeness = TokenUsageCompleteness::Complete;
        historical_attempt.created_at = now;
        historical_attempt.updated_at = now;
        historical_attempt.completed_at = Some(now);
        storage
            .create_chat_run_attempt(&historical_attempt)
            .expect("historical usage attempt");
        let mut sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        sessions.external_home = Arc::new(root.clone());
        let internal = sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("new-session-test".to_string()),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("internal session");
        sessions
            .append_message(&internal.id, MessageRole::User, "older prompt")
            .await
            .expect("older stored prompt");
        sessions
            .append_message(&internal.id, MessageRole::User, "first prompt")
            .await
            .expect("stored prompt");
        let inferred = sessions
            .infer_native_session_id(
                &internal.id,
                Provider::Codex,
                project.to_str().expect("project path"),
            )
            .await
            .expect("native mapping");
        assert_eq!(inferred.as_deref(), Some(native_id));

        let stored = storage
            .get_session(&internal.id)
            .expect("storage query")
            .expect("stored session");
        assert_eq!(stored.native_session_id.as_deref(), Some(native_id));
        let api_json = serde_json::to_value(&stored).expect("session JSON");
        assert!(api_json.get("nativeSessionId").is_none());

        let existing_rollout = std::fs::read_to_string(&rollout).expect("read rollout");
        std::fs::write(
            &rollout,
            format!(
                "{existing_rollout}{}\n{}\n",
                serde_json::json!({
                    "timestamp": now + chrono::Duration::seconds(1),
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "continued outside Workbench",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now + chrono::Duration::seconds(2),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "external answer"}]
                    }
                })
            ),
        )
        .expect("append external continuation");

        let mapped_messages = sessions
            .messages_including_external(&internal.id)
            .await
            .expect("mapped external messages");
        assert_eq!(
            mapped_messages
                .iter()
                .filter(|message| message.content == "first prompt")
                .count(),
            1,
            "mapped history must not duplicate the first Workbench prompt: {mapped_messages:#?}"
        );
        assert_eq!(
            mapped_messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            [
                "first prompt",
                "continued outside Workbench",
                "external answer"
            ]
        );

        let listed = sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("project sessions");
        assert!(
            listed.iter().all(|session| session.id != native_id),
            "mapped native rollout must not appear as an extra chat: {listed:#?}"
        );
        let historical_session = listed
            .iter()
            .find(|session| session.id == historical_native_id);
        assert!(
            historical_session.is_some(),
            "unmapped historical rollout must remain discoverable: {listed:#?}"
        );
        assert_eq!(
            historical_session
                .expect("historical session checked above")
                .message_count,
            2,
            "unmapped external session count must come from loaded rollout messages"
        );
        assert_eq!(
            historical_session
                .expect("historical session checked above")
                .lifetime_token_usage
                .as_ref()
                .map(|usage| usage.total),
            Some(42),
            "external refresh must preserve stored lifetime token usage"
        );
        let internal_session = listed.iter().find(|session| session.id == internal.id);
        assert!(
            internal_session.is_some(),
            "internal chat must remain visible: {listed:#?}"
        );
        assert_eq!(
            internal_session
                .expect("internal session checked above")
                .message_count,
            mapped_messages.len(),
            "mapped native session list count must match loaded message total"
        );
        sessions
            .set_active(&internal.id, true)
            .await
            .expect("mark mapped session active");
        let active_sessions = sessions.list_active().await;
        let active_internal_session = active_sessions
            .iter()
            .find(|session| session.id == internal.id)
            .expect("mapped active session");
        assert_eq!(
            active_internal_session.message_count,
            mapped_messages.len(),
            "active-session events must use the loaded message total"
        );
        sessions
            .set_active(&internal.id, false)
            .await
            .expect("mark mapped session inactive");

        let stale_internal = sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("stale-count-session".to_string()),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("stale-count session");
        sessions
            .set_native_session_id(&stale_internal.id, historical_native_id.to_string())
            .await
            .expect("map stale-count session");
        sessions
            .set_active(&stale_internal.id, false)
            .await
            .expect("mark stale-count session inactive");
        let mut stale_summary = storage
            .get_session(&stale_internal.id)
            .expect("stale-count storage query")
            .expect("stale-count stored session");
        stale_summary.message_count = 99;
        storage
            .upsert_session(&stale_summary)
            .expect("persist stale count");
        let stale_listed = sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("project sessions after stale count");
        let stale_listed_session = stale_listed
            .iter()
            .find(|session| session.id == stale_internal.id)
            .expect("stale-count listed session");
        assert_eq!(
            stale_listed_session.message_count, 2,
            "inactive mapped session counts must come from loaded messages, not stale stored metadata"
        );
        let args = default_agent_args_with_resume(
            Provider::Codex,
            "second prompt",
            None,
            None,
            None,
            None,
            None,
            stored.native_session_id.as_deref(),
            ChatRuntime::NativeCli,
        );
        assert!(
            args.contains(&"--json".to_string()),
            "Codex must emit thread.started JSON: {args:?}"
        );
        assert_eq!(
            &args[args.iter().position(|arg| arg == "resume").unwrap()..],
            ["resume", native_id, "second prompt"]
        );

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn persisted_native_mapping_hides_rollout_after_memory_eviction() {
        let root = std::env::temp_dir().join(format!("iowb-native-eviction-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        let native_id = "44444444-4444-4444-8444-444444444444";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/11")
            .join(format!("rollout-2026-08-11T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project, "thread_source": "user"}
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "mapped prompt",
                        "kind": "plain"
                    }
                })
            ),
        )
        .expect("rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        let mapped_session = SessionSummary {
            id: "mapped-workbench-session".to_string(),
            provider: Provider::Codex,
            project_path: project.display().to_string(),
            title: "Mapped session".to_string(),
            last_activity: now - chrono::Duration::minutes(1),
            native_session_id: Some(native_id.to_string()),
            ..Default::default()
        };
        storage
            .upsert_session(&mapped_session)
            .expect("mapped session");
        storage
            .upsert_session(&SessionSummary {
                id: "newer-session".to_string(),
                provider: Provider::Codex,
                project_path: project.display().to_string(),
                title: "Newer session".to_string(),
                last_activity: now,
                ..Default::default()
            })
            .expect("newer session");

        let mut sessions = SessionManager::load(storage.clone(), 1).expect("sessions");
        assert!(
            sessions
                .sessions
                .read()
                .await
                .get(&mapped_session.id)
                .is_none(),
            "mapped session must be outside the in-memory cache for this regression"
        );
        sessions.external_home = Arc::new(root.clone());

        let listed = sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("project sessions");
        assert!(listed.iter().any(|session| session.id == mapped_session.id));
        assert!(
            listed.iter().all(|session| session.id != native_id),
            "persisted native mapping must hide the external rollout after eviction: {listed:#?}"
        );

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn codex_rollout_sync_appends_missing_workbench_turn_once() {
        let root = std::env::temp_dir().join(format!("iowb-codex-sync-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        let native_id = "44444444-4444-4444-8444-444444444444";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/06")
            .join(format!("rollout-2026-08-06T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "original cli prompt",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "original cli answer"}]
                    }
                })
            ),
        )
        .expect("rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        let mut sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        sessions.external_home = Arc::new(root.clone());
        let internal = sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("workbench-session".to_string()),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("internal session");
        sessions
            .set_native_session_id(&internal.id, native_id)
            .await
            .expect("native id");

        let appended = sessions
            .sync_codex_turn_to_native_rollout(
                &internal.id,
                "continued in Workbench",
                "answer from Workbench",
            )
            .await
            .expect("sync append");
        assert!(appended);

        let messages = sessions
            .messages_including_external(&internal.id)
            .await
            .expect("messages");
        assert_eq!(
            messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            [
                "original cli prompt",
                "original cli answer",
                "continued in Workbench",
                "answer from Workbench"
            ]
        );

        let second = sessions
            .sync_codex_turn_to_native_rollout(
                &internal.id,
                "continued in Workbench",
                "answer from Workbench",
            )
            .await
            .expect("sync duplicate");
        assert!(!second);
        let messages_after_second = sessions
            .messages_including_external(&internal.id)
            .await
            .expect("messages after duplicate sync");
        assert_eq!(messages_after_second.len(), messages.len());

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn codex_rollout_sync_trusts_existing_response_and_rejects_transcript() {
        let root = std::env::temp_dir().join(format!("iowb-codex-sync-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        let native_id = "55555555-5555-4555-8555-555555555555";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/11")
            .join(format!("rollout-2026-08-11T10-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "continued in Workbench",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "phase": "final_answer",
                        "content": [{"type": "output_text", "text": "Native answer with markdown."}]
                    }
                })
            ),
        )
        .expect("rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        let mut sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        sessions.external_home = Arc::new(root.clone());
        let internal = sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("workbench-session-existing-final".to_string()),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("internal session");
        sessions
            .set_native_session_id(&internal.id, native_id)
            .await
            .expect("native id");

        let original_rollout = std::fs::read_to_string(&rollout).expect("original rollout");
        let appended = sessions
            .sync_codex_turn_to_native_rollout(
                &internal.id,
                "continued in Workbench",
                "Native answer with different formatting",
            )
            .await
            .expect("sync existing response");
        assert!(!appended);
        assert_eq!(
            original_rollout,
            std::fs::read_to_string(&rollout).expect("unchanged rollout")
        );

        let transcript = concat!(
            "thinking\nInspecting files\n\n",
            "exec / Parameters\n**Tool:** `command_execution`\n\n",
            "codex\nSynthetic answer\n\n",
            "tokens used\n{\"output_tokens\":8}"
        );
        let transcript_appended = sessions
            .sync_codex_turn_to_native_rollout(&internal.id, "another prompt", transcript)
            .await
            .expect("sync transcript");
        assert!(!transcript_appended);
        assert_eq!(
            original_rollout,
            std::fs::read_to_string(&rollout).expect("rollout after transcript rejection")
        );

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mapped_codex_history_returns_one_main_response_for_legacy_duplicate() {
        let root = std::env::temp_dir().join(format!("iowb-codex-history-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        let native_id = "66666666-6666-4666-8666-666666666666";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/11")
            .join(format!("rollout-2026-08-11T11-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        let transcript = format!(
            "thinking\n{}\n\nexec / Parameters\n**Tool:** `command_execution`\n\ncodex\nNormal main response.\n\ntokens used\n{{\"output_tokens\":8}}",
            "x".repeat(103_000)
        );
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "Why is the response duplicated?",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "id": "msg-native-final",
                        "role": "assistant",
                        "phase": "final_answer",
                        "content": [{"type": "output_text", "text": "Normal main response."}]
                    }
                }),
                serde_json::json!({
                    "timestamp": now,
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "id": "msg-workbench-transcript",
                        "role": "assistant",
                        "source": "io-workbench",
                        "content": [{"type": "output_text", "text": transcript}]
                    }
                })
            ),
        )
        .expect("rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        let mut sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        sessions.external_home = Arc::new(root.clone());
        let internal = sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("workbench-session-legacy-duplicate".to_string()),
                false,
                None,
                None,
                Some("ultra".to_string()),
                Some("ultra".to_string()),
                Some(true),
                None,
            )
            .await
            .expect("internal session");
        sessions
            .append_message(
                &internal.id,
                MessageRole::User,
                "Why is the response duplicated?",
            )
            .await
            .expect("stored user message");
        sessions
            .append_message(&internal.id, MessageRole::Assistant, transcript.clone())
            .await
            .expect("stored transcript");
        sessions
            .set_native_session_id(&internal.id, native_id)
            .await
            .expect("native id");

        let messages = sessions
            .messages_including_external(&internal.id)
            .await
            .expect("mapped messages");
        let main_responses = messages
            .iter()
            .filter(|message| {
                message.role == MessageRole::Assistant
                    && !message.content.trim_start().starts_with("thinking\n")
            })
            .collect::<Vec<_>>();
        assert_eq!(1, main_responses.len(), "{messages:#?}");
        assert_eq!("Normal main response.", main_responses[0].content);
        assert_eq!(
            1,
            messages
                .iter()
                .map(|message| message.content.matches("Normal main response.").count())
                .sum::<usize>()
        );
        assert!(messages.iter().all(|message| message.content != transcript));

        drop(sessions);
        drop(storage);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn base32_secret_decodes_rfc_vector() {
        let secret =
            decode_base32_secret("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").expect("valid base32");
        assert_eq!(secret, b"12345678901234567890");
    }

    #[test]
    fn hotp_matches_rfc_vector() {
        let secret =
            decode_base32_secret("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").expect("valid base32");
        assert_eq!(hotp(&secret, 1).expect("hotp"), 287082);
    }

    #[test]
    fn base32_secret_rejects_invalid_characters() {
        assert!(decode_base32_secret("iowb-c5e354e6e3a5741e").is_err());
    }

    #[test]
    fn auth_required_env_defaults_secure_but_allows_explicit_opt_out() {
        const KEY: &str = "IOWB_TEST_AUTH_REQUIRED_DEFAULT";
        unsafe {
            std::env::remove_var(KEY);
        }
        assert!(env_bool(KEY, true));

        unsafe {
            std::env::set_var(KEY, "false");
        }
        assert!(!env_bool(KEY, true));

        unsafe {
            std::env::remove_var(KEY);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configured_agent_runtime_persists_assistant_output() {
        let root = std::env::temp_dir().join(format!("iowb-agent-test-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        unsafe {
            std::env::set_var("IO_WORKBENCH_AGENT_COMMAND", "/bin/sh");
            std::env::set_var(
                "IO_WORKBENCH_AGENT_ARGS_JSON",
                r#"["-c","printf 'agent:%s\n' \"$1\"","iowb-agent","{prompt}"]"#,
            );
        }

        let state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
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
            .start_agent_session(
                Provider::Codex,
                project.display().to_string(),
                "hello",
                None,
                None,
                None,
                None,
                None,
                None,
                ChatRuntime::NativeCli,
                None,
                None,
            )
            .await
            .expect("agent starts");

        let mut saw_output = false;
        for _ in 0..20 {
            let messages = state.sessions.messages(&session.id).expect("messages");
            if messages.iter().any(|message| {
                message.role == MessageRole::Assistant && message.content.contains("agent:hello")
            }) {
                saw_output = true;
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }

        unsafe {
            std::env::remove_var("IO_WORKBENCH_AGENT_COMMAND");
            std::env::remove_var("IO_WORKBENCH_AGENT_ARGS_JSON");
        }

        let messages = state.sessions.messages(&session.id).expect("messages");
        let user_message = messages
            .iter()
            .find(|message| message.role == MessageRole::User)
            .expect("persisted user message");
        assert_eq!(user_message.metadata["cli"], "codex");
        assert_eq!(user_message.metadata["model"], "");
        assert!(user_message.metadata["sentAt"].as_str().is_some());

        let assistant_message = messages
            .iter()
            .find(|message| {
                message.role == MessageRole::Assistant && message.content.contains("agent:hello")
            })
            .expect("persisted assistant message");
        assert_eq!(assistant_message.metadata["cli"], "codex");
        assert!(assistant_message.metadata["receivedAt"].as_str().is_some());
        assert!(assistant_message.metadata["sentAt"].as_str().is_some());
        assert!(
            assistant_message.metadata["elapsedMs"].as_i64().is_some(),
            "assistant metadata: {:?}",
            assistant_message.metadata
        );

        // Simulate the server-side token-usage stamp that runs when the UI
        // fetches `/api/.../token-usage`. The per-message metadata should
        // round-trip the nested tokenUsage object so a fresh page load
        // can render the footer without re-hitting the live CLI log.
        let stamp = serde_json::json!({
            "tokenUsage": {
                "used": 4321u64,
                "input": 1500u64,
                "output": 2700u64,
                "cacheCreation": 0u64,
                "cacheRead": 121u64,
            }
        });
        let stamped = state
            .sessions
            .stamp_latest_message_metadata(&session.id, MessageRole::Assistant, stamp.clone())
            .expect("stamp succeeds");
        assert!(stamped, "expected a row to be updated");

        let after = state.sessions.messages(&session.id).expect("messages");
        let assistant_after = after
            .iter()
            .find(|message| {
                message.role == MessageRole::Assistant && message.content.contains("agent:hello")
            })
            .expect("persisted assistant message after stamp");
        assert_eq!(assistant_after.metadata["tokenUsage"]["used"], 4321);
        assert_eq!(assistant_after.metadata["tokenUsage"]["input"], 1500);
        assert_eq!(assistant_after.metadata["tokenUsage"]["output"], 2700);
        assert_eq!(assistant_after.metadata["tokenUsage"]["cacheRead"], 121);
        assert_eq!(assistant_after.metadata["cli"], "codex");
        assert!(assistant_after.metadata["receivedAt"].as_str().is_some());

        let _ = std::fs::remove_dir_all(root);

        assert!(saw_output);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn durable_recovery_resumes_native_session_without_duplicate_user_message() {
        let root = std::env::temp_dir().join(format!("iowb-recovery-test-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");
        let state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
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
            .set_native_session_id(&session.id, "native-recovery-session")
            .await
            .expect("native session id");
        state
            .sessions
            .append_message(
                &session.id,
                MessageRole::User,
                "finish the interrupted implementation",
            )
            .await
            .expect("original user message");

        let mut run = StoredDurableChatRun::new(
            "run-recovery",
            Some("user-recovery".to_string()),
            session.id.clone(),
            "gemini",
            "finish the interrupted implementation",
            project.display().to_string(),
        );
        run.native_session_id = Some("native-recovery-session".to_string());
        state
            .storage
            .create_durable_chat_run(&run)
            .expect("durable run");
        let claimed = state
            .storage
            .mark_durable_chat_run_recovering(&run.id, DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS)
            .expect("claim recovery")
            .expect("recoverable run");

        unsafe {
            std::env::set_var("IO_WORKBENCH_GEMINI_COMMAND", "/bin/sh");
            std::env::set_var(
                "IO_WORKBENCH_GEMINI_ARGS_JSON",
                r#"["-c","printf 'resumed:%s\\n' \"$1\"","iowb-recovery","{native_session_id}"]"#,
            );
        }
        let recovered = state
            .recover_agent_run(claimed, None)
            .await
            .expect("recovery starts");
        unsafe {
            std::env::remove_var("IO_WORKBENCH_GEMINI_COMMAND");
            std::env::remove_var("IO_WORKBENCH_GEMINI_ARGS_JSON");
        }
        assert_eq!(recovered.id, session.id);

        timeout(Duration::from_secs(3), async {
            loop {
                let stored_run = state
                    .storage
                    .get_durable_chat_run(&run.id)
                    .expect("read durable run")
                    .expect("durable run exists");
                if stored_run.status == "completed" {
                    break;
                }
                sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("recovered provider completes");

        let messages = state
            .storage
            .list_messages(&session.id)
            .expect("session messages");
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .count(),
            1,
            "recovery must not append its hidden prompt as another user row"
        );
        let assistant = messages
            .iter()
            .find(|message| message.role == MessageRole::Assistant)
            .expect("recovered assistant message");
        assert!(
            assistant
                .content
                .contains("resumed:native-recovery-session"),
            "{}",
            assistant.content
        );
        assert_eq!(assistant.metadata["durableRunId"], run.id);
        let stored_run = state
            .storage
            .get_durable_chat_run(&run.id)
            .expect("read durable run")
            .expect("durable run exists");
        assert_eq!(stored_run.resume_attempts, 1);
        assert_eq!(
            stored_run.native_session_id.as_deref(),
            Some("native-recovery-session")
        );
        assert!(
            !state
                .storage
                .get_session(&session.id)
                .expect("stored session")
                .expect("session exists")
                .active
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn project_session_list_does_not_cache_external_rollout_messages() {
        let root =
            std::env::temp_dir().join(format!("iowb-list-external-cache-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");

        let native_id = "44444444-4444-4444-8444-444444444444";
        let now = Utc::now();
        let rollout = root
            .join(".codex/sessions/2026/08/14")
            .join(format!("rollout-2026-08-14T00-00-00-{native_id}.jsonl"));
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": now,
                    "type": "session_meta",
                    "payload": {"id": native_id, "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": now + chrono::Duration::seconds(1),
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "check memory",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": now + chrono::Duration::seconds(2),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "done"}]
                    }
                })
            ),
        )
        .expect("rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
        let mut sessions = SessionManager::load(storage.clone(), 10).expect("sessions");
        sessions.external_home = Arc::new(root.clone());
        let mapped = sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("mapped-list-session".to_string()),
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("mapped session");
        sessions
            .set_native_session_id(&mapped.id, native_id.to_string())
            .await
            .expect("native mapping");

        let listed = sessions
            .list_for_project(project.to_str().expect("project path"))
            .await
            .expect("project sessions");
        assert!(
            listed.iter().any(|session| session.id == mapped.id),
            "mapped Workbench session must remain listed: {listed:#?}"
        );
        assert!(
            listed.iter().all(|session| session.id != native_id),
            "mapped native rollout must not be listed separately: {listed:#?}"
        );
        assert!(
            sessions.external_cache.read().await.messages.is_empty(),
            "project list must not parse and cache full external rollout messages"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rollover_recovery_never_infers_or_resumes_archived_native_context() {
        let root = std::env::temp_dir().join(format!(
            "iowb-rollover-recovery-selection-{}",
            Uuid::new_v4()
        ));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");
        let database = config_dir.join("test.db");
        let initial_state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: database.clone(),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("initial state");
        initial_state
            .storage
            .create_user("user-rollover", "user-rollover", "test-hash")
            .expect("create user");
        let mut session = initial_state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-rollover-recovery".to_string()),
                false,
                None,
                Some(ChatRuntime::IoGateway),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        initial_state
            .sessions
            .set_native_session_id(&session.id, "native-poisoned")
            .await
            .expect("poisoned mapping");
        let failed_message = initial_state
            .sessions
            .append_message(
                &session.id,
                MessageRole::User,
                "finish the image-heavy request",
            )
            .await
            .expect("failed prompt");
        initial_state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("inactive session");
        session = initial_state
            .sessions
            .get(&session.id)
            .await
            .expect("stored session");

        let mut trigger_run = StoredDurableChatRun::new(
            "run-rollover-trigger",
            Some("user-rollover".to_string()),
            session.id.clone(),
            "codex",
            failed_message.content.clone(),
            project.display().to_string(),
        );
        trigger_run.user_message_id = Some(failed_message.id.clone());
        trigger_run.native_session_id = Some("native-poisoned".to_string());
        initial_state
            .storage
            .create_durable_chat_run(&trigger_run)
            .expect("trigger run");
        initial_state
            .storage
            .mark_durable_chat_run_failed(&trigger_run.id, "invalid body")
            .expect("trigger failed");
        let now = Utc::now();
        let rollover = StoredSessionContextRollover {
            id: "rollover-recovery-selection".to_string(),
            user_id: "user-rollover".to_string(),
            session_id: session.id.clone(),
            request_id: "request-rollover-recovery".to_string(),
            kind: CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN.to_string(),
            failed_message_id: failed_message.id.clone(),
            trigger_run_id: trigger_run.id.clone(),
            retry_run_id: "run-rollover-retry".to_string(),
            from_native_session_id: Some("native-poisoned".to_string()),
            candidate_native_session_id: None,
            state: "starting".to_string(),
            handoff: "bounded text-only handoff".to_string(),
            observed_bytes: Some(19_760_000),
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut retry_run = StoredDurableChatRun::new(
            rollover.retry_run_id.clone(),
            Some("user-rollover".to_string()),
            session.id.clone(),
            "codex",
            rollover.handoff.clone(),
            project.display().to_string(),
        );
        retry_run.user_message_id = Some(failed_message.id.clone());
        assert!(
            initial_state
                .storage
                .prepare_context_rollover(&rollover, &retry_run)
                .expect("prepare rollover")
        );
        drop(initial_state);

        // Reopen from disk to exercise the same selection logic used after a
        // forced server restart. This fake external rollout would be a valid
        // inference match for the failed prompt if rollover recovery did not
        // explicitly suppress inference.
        let restarted = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: database.clone(),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("restarted state");
        {
            let mut cache = restarted.sessions.external_cache.write().await;
            cache.loaded_at = Some(Instant::now());
            let record = ExternalSessionRecord {
                summary: SessionSummary {
                    id: "native-inferred-poison".to_string(),
                    provider: Provider::Codex,
                    external: true,
                    project_path: project.display().to_string(),
                    title: failed_message.content.clone(),
                    last_activity: Utc::now(),
                    ..Default::default()
                },
                file_path: root.join("missing-inference-rollout.jsonl"),
            };
            let cache_key = external_session_cache_key(&record);
            let cached_messages = Arc::new(vec![failed_message.clone()]);
            let estimated_bytes = estimate_external_messages_bytes(cached_messages.as_ref());
            cache.records = vec![record];
            cache.message_bytes = estimated_bytes;
            cache.messages.insert(
                cache_key,
                CachedExternalMessages {
                    modified_at: None,
                    estimated_bytes,
                    last_access: Instant::now(),
                    total_count: cached_messages.len(),
                    complete: true,
                    messages: cached_messages,
                },
            );
        }
        let claimed = restarted
            .storage
            .mark_durable_chat_run_recovering(&retry_run.id, DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS)
            .expect("claim rollover recovery")
            .expect("recoverable rollover run");
        let recovery = restarted.recover_agent_run(claimed, None).await;
        assert!(
            matches!(recovery, Err(CoreError::InvalidInput(_))),
            "missing gateway config should stop after native-id selection: {recovery:?}"
        );
        let stored_retry = restarted
            .storage
            .get_durable_chat_run(&retry_run.id)
            .expect("retry lookup")
            .expect("retry run");
        assert_eq!(stored_retry.native_session_id, None);
        assert_ne!(
            stored_retry.native_session_id.as_deref(),
            Some("native-poisoned")
        );
        assert_ne!(
            stored_retry.native_session_id.as_deref(),
            Some("native-inferred-poison")
        );
        assert_eq!(
            restarted
                .storage
                .get_session(&session.id)
                .expect("session lookup")
                .expect("session")
                .native_session_id
                .as_deref(),
            Some("native-poisoned"),
            "failed recovery must not change the visible session mapping"
        );
        assert_eq!(
            restarted
                .storage
                .context_rollover_for_retry_run(&retry_run.id)
                .expect("rollover lookup")
                .expect("rollover")
                .candidate_native_session_id,
            None
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rollover_recovery_resumes_only_the_staged_clean_candidate() {
        let root =
            std::env::temp_dir().join(format!("iowb-rollover-clean-candidate-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");
        let state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
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
        .expect("state");
        state
            .storage
            .create_user("user-rollover", "user-rollover", "test-hash")
            .expect("create user");
        let mut session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-rollover-clean".to_string()),
                false,
                None,
                Some(ChatRuntime::IoGateway),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        state
            .sessions
            .set_native_session_id(&session.id, "native-poisoned")
            .await
            .expect("old mapping");
        let failed_message = state
            .sessions
            .append_message(&session.id, MessageRole::User, "continue cleanly")
            .await
            .expect("failed prompt");
        state
            .sessions
            .set_active(&session.id, false)
            .await
            .expect("inactive");
        session = state.sessions.get(&session.id).await.expect("session");
        let mut trigger_run = StoredDurableChatRun::new(
            "run-clean-trigger",
            Some("user-rollover".to_string()),
            session.id.clone(),
            "codex",
            failed_message.content.clone(),
            project.display().to_string(),
        );
        trigger_run.user_message_id = Some(failed_message.id.clone());
        trigger_run.native_session_id = Some("native-poisoned".to_string());
        state
            .storage
            .create_durable_chat_run(&trigger_run)
            .expect("trigger run");
        state
            .storage
            .mark_durable_chat_run_failed(&trigger_run.id, "invalid body")
            .expect("trigger failed");
        let now = Utc::now();
        let rollover = StoredSessionContextRollover {
            id: "rollover-clean-candidate".to_string(),
            user_id: "user-rollover".to_string(),
            session_id: session.id.clone(),
            request_id: "request-clean-candidate".to_string(),
            kind: CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN.to_string(),
            failed_message_id: failed_message.id.clone(),
            trigger_run_id: trigger_run.id.clone(),
            retry_run_id: "run-clean-retry".to_string(),
            from_native_session_id: Some("native-poisoned".to_string()),
            candidate_native_session_id: None,
            state: "starting".to_string(),
            handoff: "bounded clean handoff".to_string(),
            observed_bytes: Some(19_760_000),
            limit_bytes: CODEX_GATEWAY_BODY_LIMIT_BYTES,
            error: None,
            created_at: now,
            updated_at: now,
            activated_at: None,
        };
        let mut retry_run = StoredDurableChatRun::new(
            rollover.retry_run_id.clone(),
            Some("user-rollover".to_string()),
            session.id.clone(),
            "codex",
            rollover.handoff.clone(),
            project.display().to_string(),
        );
        retry_run.user_message_id = Some(failed_message.id.clone());
        assert!(
            state
                .storage
                .prepare_context_rollover(&rollover, &retry_run)
                .expect("prepare rollover")
        );
        assert!(
            state
                .storage
                .set_context_rollover_candidate(&rollover.id, &retry_run.id, "native-clean-staged",)
                .expect("stage clean candidate")
        );
        let claimed = state
            .storage
            .mark_durable_chat_run_recovering(&retry_run.id, DURABLE_CHAT_RUN_MAX_RECOVERY_ATTEMPTS)
            .expect("claim recovery")
            .expect("recoverable retry");
        let recovery = state.recover_agent_run(claimed, None).await;
        assert!(matches!(recovery, Err(CoreError::InvalidInput(_))));
        assert_eq!(
            state
                .storage
                .get_durable_chat_run(&retry_run.id)
                .expect("retry lookup")
                .expect("retry")
                .native_session_id
                .as_deref(),
            Some("native-clean-staged")
        );
        assert_ne!(
            state
                .storage
                .get_durable_chat_run(&retry_run.id)
                .expect("retry lookup")
                .expect("retry")
                .native_session_id
                .as_deref(),
            Some("native-poisoned")
        );
        assert_eq!(
            state
                .storage
                .get_session(&session.id)
                .expect("session lookup")
                .expect("session")
                .native_session_id
                .as_deref(),
            Some("native-poisoned"),
            "candidate stays staged until successful atomic completion"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn claude_prefixed_minimax_model_uses_cli_runtime_with_gateway_env() {
        assert_eq!(
            effective_agent_command_provider(Provider::Claude, Some("min:MiniMax-M3")),
            Provider::Claude
        );
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("min:MiniMax-M3")
        ));
        assert!(should_force_claude_cli_io_gateway(
            Provider::Claude,
            Some("min:MiniMax-M3")
        ));

        let args = default_agent_args_with(
            Provider::Claude,
            "pwd",
            Some("bypass"),
            None,
            None,
            Some("min:MiniMax-M3"),
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", "min:MiniMax-M3"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--setting-sources", "project,local"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--permission-mode", "bypassPermissions"]),
            "args: {args:?}"
        );
    }

    #[test]
    fn claude_unprefixed_model_uses_local_cli_runtime() {
        assert_eq!(
            effective_agent_command_provider(Provider::Claude, Some("claude-sonnet-4-5")),
            Provider::Claude
        );
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("claude-sonnet-4-5")
        ));

        let args = default_agent_args_with(
            Provider::Claude,
            "inspect the repo",
            Some("accept-edits"),
            Some("high"),
            None,
            Some("claude-sonnet-4-5"),
        );
        assert!(args.contains(&"--print".to_string()), "args: {args:?}");
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", "claude-sonnet-4-5"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--permission-mode", "acceptEdits"]),
            "args: {args:?}"
        );
        assert_eq!(args.last().map(String::as_str), Some("inspect the repo"));
    }

    #[test]
    fn claude_prefixed_model_uses_cli_runtime_with_prefixed_gateway_model_arg() {
        assert_eq!(
            effective_agent_command_provider(Provider::Claude, Some("cld:claude-sonnet-5")),
            Provider::Claude
        );
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("cld:claude-sonnet-5")
        ));
        assert!(should_force_claude_cli_io_gateway(
            Provider::Claude,
            Some("cld:claude-sonnet-5")
        ));

        let args = default_agent_args_with(
            Provider::Claude,
            "pwd",
            Some("bypass"),
            None,
            None,
            Some("cld:claude-sonnet-5"),
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", "cld:claude-sonnet-5"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--setting-sources", "project,local"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--permission-mode", "bypassPermissions"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2).any(|pair| pair == ["--tools", "default"]),
            "args: {args:?}"
        );
        assert!(
            args.iter().any(|arg| arg == "cld:claude-sonnet-5"),
            "args: {args:?}"
        );
    }

    #[test]
    fn claude_bypass_permissions_alias_enables_bypass_mode() {
        let args = default_agent_args_with(
            Provider::Claude,
            "pwd",
            Some("bypass-permissions"),
            None,
            None,
            Some("cld:claude-sonnet-5"),
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--permission-mode", "bypassPermissions"]),
            "args: {args:?}"
        );
        assert!(
            args.contains(&"--dangerously-skip-permissions".to_string()),
            "args: {args:?}"
        );
        assert!(
            args.windows(2).any(|pair| pair == ["--tools", "default"]),
            "args: {args:?}"
        );
    }

    #[test]
    fn claude_unprefixed_alias_uses_local_cli_runtime() {
        let args = default_agent_args_with(
            Provider::Claude,
            "pwd",
            Some("bypass"),
            None,
            None,
            Some("sonnet"),
        );
        assert!(
            args.windows(2).any(|pair| pair == ["--model", "sonnet"]),
            "args: {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg == "--setting-sources"),
            "args: {args:?}"
        );
        assert!(
            !should_use_direct_ai_gateway_runtime(Provider::Claude, Some("sonnet")),
            "args: {args:?}"
        );
        assert_eq!(args.last().map(String::as_str), Some("pwd"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn gemini_gateway_model_calls_direct_ai_api() {
        assert_gateway_model_calls_direct_ai_api(
            Provider::Gemini,
            "agw:gemini-3.6-flash-medium",
            "/v1/chat/completions",
        )
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn direct_ai_failure_persists_assistant_error_message() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake gateway");
        let gateway_addr = listener.local_addr().expect("gateway address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept gateway request");
            let mut buffer = [0u8; 1024];
            let _ = stream
                .read(&mut buffer)
                .await
                .expect("read gateway request");
            let body = r#"{"error":"upstream unavailable"}"#;
            let response = format!(
                "HTTP/1.1 502 Bad Gateway\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write gateway error response");
        });

        let root = std::env::temp_dir().join(format!("iowb-direct-ai-fail-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");
        let state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
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
            .start_agent_session(
                Provider::Gemini,
                project.display().to_string(),
                "trigger failure",
                None,
                Some("agw:gemini-3.6-flash-medium".to_string()),
                None,
                None,
                None,
                None,
                ChatRuntime::NativeCli,
                Some(DirectAiRuntimeConfig {
                    base_url: format!("http://{gateway_addr}"),
                    api_key: "test-key".to_string(),
                    max_tokens: Some(32),
                }),
                None,
            )
            .await
            .expect("direct gateway session starts");

        let mut persisted_error = None;
        for _ in 0..20 {
            let messages = state.sessions.messages(&session.id).expect("messages");
            persisted_error = messages.into_iter().find(|message| {
                message.role == MessageRole::Assistant
                    && message.content.contains("Direct AI gateway request failed")
            });
            if persisted_error.is_some() {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }

        let assistant = persisted_error.expect("persisted assistant error");
        assert!(assistant.content.contains("502 Bad Gateway"));
        assert_eq!(assistant.metadata["status"], "failed");
        assert_eq!(assistant.metadata["cli"], "gemini");

        let _ = std::fs::remove_dir_all(root);
    }

    async fn assert_gateway_model_calls_direct_ai_api(
        provider: Provider,
        model: &str,
        expected_path: &str,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake gateway");
        let gateway_addr = listener.local_addr().expect("gateway address");
        let (request_tx, request_rx) = oneshot::channel::<String>();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept gateway request");
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 1024];
            let mut header_end = None;
            let mut content_length = 0usize;
            loop {
                let read = stream.read(&mut chunk).await.expect("read gateway request");
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if header_end.is_none() {
                    if let Some(index) = buffer.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        header_end = Some(index + 4);
                        let headers = String::from_utf8_lossy(&buffer[..index]);
                        content_length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                    }
                }
                if let Some(end) = header_end {
                    if buffer.len() >= end + content_length {
                        break;
                    }
                }
            }

            let request = String::from_utf8_lossy(&buffer).to_string();
            let _ = request_tx.send(request);
            let body = r#"{"content":[{"type":"text","text":"direct:ok"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write gateway response");
        });

        let root = std::env::temp_dir().join(format!("iowb-direct-ai-test-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");
        let state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
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

        let existing_session = state
            .sessions
            .create_or_update(
                provider,
                project.display().to_string(),
                None,
                false,
                Some(model.to_string()),
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("existing session");
        state
            .sessions
            .append_message(&existing_session.id, MessageRole::User, "earlier question")
            .await
            .expect("earlier user message");
        state
            .sessions
            .append_message(
                &existing_session.id,
                MessageRole::Assistant,
                "earlier answer",
            )
            .await
            .expect("earlier assistant message");

        let session = state
            .start_agent_session(
                provider,
                project.display().to_string(),
                "reply ok",
                Some(existing_session.id.clone()),
                Some(model.to_string()),
                None,
                None,
                None,
                None,
                ChatRuntime::NativeCli,
                Some(DirectAiRuntimeConfig {
                    base_url: format!("http://{gateway_addr}"),
                    api_key: "test-key".to_string(),
                    max_tokens: Some(32),
                }),
                None,
            )
            .await
            .expect("direct gateway session starts");

        let request = request_rx.await.expect("captured gateway request");
        assert!(
            request.starts_with(&format!("POST {expected_path} ")),
            "{request}"
        );
        assert!(
            request.contains("authorization: Bearer test-key"),
            "{request}"
        );
        assert!(
            request.contains(&format!(r#""model":"{model}""#)),
            "{request}"
        );
        assert!(request.contains(r#""max_tokens":32"#), "{request}");
        let request_body = request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .expect("request body");
        let request_body: Value = serde_json::from_str(request_body).expect("gateway request JSON");
        assert_eq!(
            request_body["messages"],
            serde_json::json!([
                {"role": "user", "content": "earlier question"},
                {"role": "assistant", "content": "earlier answer"},
                {"role": "user", "content": "reply ok"},
            ])
        );

        let mut saw_output = false;
        for _ in 0..20 {
            let messages = state.sessions.messages(&session.id).expect("messages");
            if messages.iter().any(|message| {
                message.role == MessageRole::Assistant && message.content.contains("direct:ok")
            }) {
                saw_output = true;
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }

        let messages = state.sessions.messages(&session.id).expect("messages");
        let assistant = messages
            .iter()
            .find(|message| {
                message.role == MessageRole::Assistant && message.content.contains("direct:ok")
            })
            .expect("assistant message");
        assert_eq!(assistant.metadata["cli"], provider.as_str());
        assert_eq!(assistant.metadata["model"], model);

        let _ = std::fs::remove_dir_all(root);
        assert!(saw_output);
    }

    #[test]
    fn codex_accept_edits_uses_sandbox_flag() {
        let args = default_agent_args_with(
            Provider::Codex,
            "hi",
            Some("accept-edits"),
            None,
            None,
            None,
        );
        eprintln!("codex accept-edits args: {:?}", args);
        assert!(
            !args.iter().any(|a| a == "--approval-mode"),
            "must not pass --approval-mode: {:?}",
            args
        );
        assert!(args.contains(&"--sandbox".to_string()), "args: {:?}", args);
        assert!(
            args.contains(&"workspace-write".to_string()),
            "args: {:?}",
            args
        );
    }

    #[test]
    fn external_provider_sessions_use_native_resume_arguments() {
        let session_id = "11111111-1111-4111-8111-111111111111";

        let claude = default_agent_args_with_resume(
            Provider::Claude,
            "continue",
            Some("plan"),
            None,
            None,
            None,
            None,
            Some(session_id),
            ChatRuntime::NativeCli,
        );
        assert!(
            claude
                .windows(2)
                .any(|args| args == ["--resume", session_id])
        );
        assert_eq!(
            &claude[..5],
            [
                "--print",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
            ]
        );
        let prompt_index = claude.iter().position(|arg| arg == "continue").unwrap();
        let resume_index = claude
            .windows(2)
            .position(|args| args == ["--resume", session_id])
            .unwrap();
        assert_eq!(
            &claude[claude.len() - 3..],
            ["--resume", session_id, "continue"],
            "claude args: {claude:?}"
        );
        let permission_index = claude
            .windows(2)
            .position(|args| args == ["--permission-mode", "plan"])
            .unwrap();
        assert!(
            permission_index < resume_index && resume_index + 2 == prompt_index,
            "claude args: {claude:?}"
        );

        let codex = default_agent_args_with_resume(
            Provider::Codex,
            "continue",
            Some("plan"),
            None,
            None,
            None,
            None,
            Some(session_id),
            ChatRuntime::NativeCli,
        );
        let resume_index = codex.iter().position(|arg| arg == "resume").unwrap();
        let sandbox_index = codex.iter().position(|arg| arg == "--sandbox").unwrap();
        assert!(sandbox_index < resume_index, "codex args: {codex:?}");
        assert_eq!(&codex[resume_index..], ["resume", session_id, "continue"]);

        let gemini = default_agent_args_with_resume(
            Provider::Gemini,
            "continue",
            None,
            None,
            None,
            None,
            None,
            Some(session_id),
            ChatRuntime::NativeCli,
        );
        assert!(
            gemini
                .windows(2)
                .any(|args| args == ["--resume", session_id])
        );
        assert!(
            gemini
                .windows(2)
                .any(|args| args == ["--prompt", "continue"])
        );
    }

    #[test]
    fn claude_and_gemini_keep_native_slash_commands_unchanged() {
        for provider in [Provider::Claude, Provider::Gemini] {
            assert_eq!(
                resolve_cli_slash_prompt(provider, "/compact preserve decisions").unwrap(),
                "/compact preserve decisions"
            );
        }
    }

    #[test]
    fn codex_expands_custom_slash_prompts_for_headless_exec() {
        let root =
            std::env::temp_dir().join(format!("iowb-codex-slash-prompt-{}", uuid::Uuid::new_v4()));
        let prompts = root.join("prompts");
        std::fs::create_dir_all(&prompts).expect("prompt directory");
        std::fs::write(
            prompts.join("draftpr.md"),
            "---\ndescription: Draft a PR\n---\nReview $1 for $FOCUS. Args: $ARGUMENTS. Cost: $$5.",
        )
        .expect("custom prompt");

        let expanded = resolve_codex_slash_prompt(
            "/prompts:draftpr src/lib.rs FOCUS=\"error handling\"",
            Some(&root),
        )
        .expect("slash prompt expands");

        assert_eq!(
            expanded,
            "Review src/lib.rs for error handling. Args: src/lib.rs FOCUS=\"error handling\". Cost: $5."
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_slash_skill_uses_headless_skill_mention() {
        let root =
            std::env::temp_dir().join(format!("iowb-codex-slash-skill-{}", uuid::Uuid::new_v4()));
        let skill = root.join("skills").join("security-review");
        std::fs::create_dir_all(&skill).expect("skill directory");
        std::fs::write(skill.join("SKILL.md"), "# Security review").expect("skill");

        assert_eq!(
            resolve_codex_slash_prompt("/security-review staged changes", Some(&root)).unwrap(),
            "$security-review staged changes"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_bypass_uses_bypass_flag() {
        let args = default_agent_args_with(Provider::Codex, "hi", Some("bypass"), None, None, None);
        eprintln!("codex bypass args: {:?}", args);
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    }

    #[test]
    fn codex_proxy_model_uses_isolated_gateway_cli_provider() {
        let args = default_agent_args_with(
            Provider::Codex,
            "hi",
            None,
            Some("medium"),
            None,
            Some("agw:claude-opus-4-6-thinking"),
        );
        eprintln!("codex proxy model args: {:?}", args);
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"agw:claude-opus-4-6-thinking".to_string()));
        assert!(args.iter().any(|a| a == "model_provider=iowb_gateway"));
        assert!(!args.iter().any(|a| a == "model_provider=openai"));
    }

    #[test]
    fn codex_minimax_alias_uses_isolated_gateway_cli_provider() {
        let args = default_agent_args_with(
            Provider::Codex,
            "hi",
            None,
            Some("medium"),
            None,
            Some("min:MiniMax-M3"),
        );
        assert!(args.iter().any(|arg| arg == "--model"));
        assert!(args.iter().any(|arg| arg == "model_provider=iowb_gateway"));
        assert!(!args.iter().any(|arg| arg == "model_provider=openai"));
        assert!(args.iter().any(|arg| arg == "min:MiniMax-M3"));
        assert!(!args.iter().any(|arg| arg == "model_provider=minimax"));
    }

    #[test]
    fn codex_effort_uses_reasoning_config_without_forcing_an_old_model() {
        let args = default_agent_args_with(Provider::Codex, "hi", None, Some("medium"), None, None);
        assert!(!args.iter().any(|arg| arg == "model_provider=aiproxy"));
        assert!(!args.iter().any(|arg| arg.starts_with("model_provider=")));
        assert!(!args.iter().any(|arg| arg == "--model"));
        assert!(
            args.windows(2)
                .any(|pair| { pair == ["-c", "model_reasoning_effort=\"medium\""] })
        );

        let thinking_args = default_agent_args_with(
            Provider::Codex,
            "hi",
            None,
            Some("medium"),
            Some(true),
            None,
        );
        assert!(
            thinking_args
                .windows(2)
                .any(|pair| { pair == ["-c", "model_reasoning_effort=\"xhigh\""] })
        );
        assert!(!thinking_args.iter().any(|arg| arg == "--reasoning-effort"));
    }

    #[test]
    fn codex_extended_efforts_are_forwarded_without_downgrade() {
        for effort in ["xhigh", "max", "ultra"] {
            let args = default_agent_args_with(
                Provider::Codex,
                "hi",
                None,
                Some(effort),
                Some(true),
                Some("gpt-5.6++"),
            );
            let expected = format!("model_reasoning_effort=\"{effort}\"");
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["-c", expected.as_str()])
            );
            assert_eq!(
                args.iter()
                    .filter(|arg| arg.starts_with("model_reasoning_effort="))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn codex_unprefixed_model_uses_local_cli_provider() {
        let args =
            default_agent_args_with(Provider::Codex, "hi", None, None, None, Some("gpt-5.4"));
        eprintln!("codex real model args: {:?}", args);
        assert!(args.contains(&"gpt-5.4".to_string()));
        assert!(!args.iter().any(|a| a == "model_provider=aiproxy"));
        assert!(!args.iter().any(|arg| arg.starts_with("model_provider=")));
        assert!(args.contains(&"--skip-git-repo-check".to_string()));
    }

    #[test]
    fn codex_legacy_model_is_ignored_for_local_cli() {
        let args =
            default_agent_args_with(Provider::Codex, "hi", None, None, None, Some("gpt-5-codex"));

        assert!(!args.iter().any(|arg| arg == "--model"));
        assert!(!args.iter().any(|arg| arg == "gpt-5-codex"));
        assert!(!args.iter().any(|arg| arg == "model_provider=aiproxy"));
        assert!(!args.iter().any(|arg| arg.starts_with("model_provider=")));
    }

    #[test]
    fn gateway_model_routes_by_model_family_for_claude_and_gemini_selection() {
        assert_eq!(
            effective_agent_command_provider(Provider::Claude, Some("cod:gpt-5.5")),
            Provider::Codex
        );
        assert_eq!(
            effective_agent_command_provider(Provider::Gemini, Some("agw:gemini-3.6-flash-medium")),
            Provider::Gemini
        );
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("cod:gpt-5.5")
        ));
        assert!(should_use_direct_ai_gateway_runtime(
            Provider::Gemini,
            Some("agw:gemini-3.6-flash-medium")
        ));
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("cld:claude-haiku-4-5-20251001")
        ));
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("min:MiniMax-M3")
        ));
        assert!(should_force_claude_cli_io_gateway(
            Provider::Claude,
            Some("gateway:claude-haiku-4-5-20251001")
        ));
        assert!(should_force_claude_cli_io_gateway(
            Provider::Claude,
            Some("min:MiniMax-M3")
        ));
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("claude-sonnet-4-5")
        ));
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            None
        ));
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Codex,
            Some("cod:gpt-5.5")
        ));
        assert!(should_force_codex_cli_io_gateway(Some("cod:gpt-5.5")));
        let args = default_agent_args_with(
            effective_agent_command_provider(Provider::Codex, Some("cod:gpt-5.5")),
            "hi",
            None,
            None,
            None,
            Some("cod:gpt-5.5"),
        );
        assert!(args.contains(&"exec".to_string()));
        assert!(args.contains(&"cod:gpt-5.5".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.iter().any(|a| a == "model_provider=iowb_gateway"));
        assert!(!args.iter().any(|a| a == "model_provider=openai"));
    }

    #[test]
    fn codex_gateway_runtime_builds_complete_ephemeral_provider_config() {
        let mut args = default_agent_args_with_resume(
            Provider::Codex,
            "hi",
            None,
            None,
            None,
            None,
            Some("gpt-custom"),
            None,
            ChatRuntime::IoGateway,
        );
        apply_codex_cli_io_gateway_args(&mut args, "https://gateway.example.com/codex/");

        for expected in [
            "model_provider=iowb_gateway",
            "model_providers.iowb_gateway.name=\"IO Gateway\"",
            "model_providers.iowb_gateway.base_url=\"https://gateway.example.com/codex\"",
            "model_providers.iowb_gateway.env_key=\"IOWB_IO_GATEWAY_API_KEY\"",
            "model_providers.iowb_gateway.wire_api=\"responses\"",
        ] {
            assert!(args.iter().any(|arg| arg == expected), "args: {args:?}");
        }
    }

    #[test]
    fn codex_gateway_unprefixed_sol_keeps_gateway_provider_and_fast_tier() {
        let mut args = default_agent_args_with_resume(
            Provider::Codex,
            "hi",
            None,
            Some("medium"),
            None,
            Some(true),
            Some("gpt-5.6-sol"),
            None,
            ChatRuntime::IoGateway,
        );
        apply_codex_cli_io_gateway_args(&mut args, "https://ai.qif.us/codex");

        let model_index = args
            .iter()
            .position(|arg| arg == "--model")
            .expect("model flag");
        assert_eq!(
            args.get(model_index + 1).map(String::as_str),
            Some("gpt-5.6-sol")
        );
        for expected in [
            "model_provider=iowb_gateway",
            "model_providers.iowb_gateway.base_url=\"https://ai.qif.us/codex\"",
            "features.fast_mode=true",
            "service_tier=\"fast\"",
        ] {
            assert!(args.iter().any(|arg| arg == expected), "args: {args:?}");
        }
    }

    #[test]
    fn codex_gateway_provider_config_precedes_resume_positional() {
        let mut args = default_agent_args_with_resume(
            Provider::Codex,
            "continue",
            Some("bypass"),
            Some("medium"),
            None,
            None,
            Some("min:MiniMax-M3"),
            Some("native-session-id"),
            ChatRuntime::IoGateway,
        );
        apply_codex_cli_io_gateway_args(&mut args, "https://gateway.example.com/codex");

        let resume_index = args
            .iter()
            .position(|arg| arg == "resume")
            .expect("resume positional");
        for key in [
            "model_provider=",
            "model_providers.iowb_gateway.name=",
            "model_providers.iowb_gateway.base_url=",
            "model_providers.iowb_gateway.env_key=",
            "model_providers.iowb_gateway.wire_api=",
        ] {
            let config_index = args
                .iter()
                .position(|arg| arg.starts_with(key))
                .unwrap_or_else(|| panic!("missing {key} in {args:?}"));
            assert!(config_index < resume_index, "args: {args:?}");
        }
        assert_eq!(
            &args[resume_index..resume_index + 3],
            ["resume", "native-session-id", "continue"]
        );
    }

    #[test]
    fn codex_fast_setting_selects_fast_or_standard_before_resume() {
        for (fast, expected_tier) in [(true, "\"fast\""), (false, "\"default\"")] {
            let args = default_agent_args_with_resume(
                Provider::Codex,
                "continue",
                None,
                Some("medium"),
                None,
                Some(fast),
                Some("cod:gpt-5.6-sol"),
                Some("native-session-id"),
                ChatRuntime::IoGateway,
            );

            let resume_index = args
                .iter()
                .position(|arg| arg == "resume")
                .expect("resume positional");
            let fast_feature_index = args
                .iter()
                .position(|arg| arg == "features.fast_mode=true")
                .unwrap_or_else(|| panic!("missing Fast feature override in {args:?}"));
            let tier = format!("service_tier={expected_tier}");
            let tier_index = args
                .iter()
                .position(|arg| arg == &tier)
                .unwrap_or_else(|| panic!("missing {tier} in {args:?}"));

            assert!(fast_feature_index < resume_index, "args: {args:?}");
            assert!(tier_index < resume_index, "args: {args:?}");
            assert_eq!(
                &args[resume_index..resume_index + 3],
                ["resume", "native-session-id", "continue"]
            );
        }
    }

    #[test]
    fn codex_unspecified_fast_setting_inherits_cli_configuration() {
        let args = default_agent_args_with_resume(
            Provider::Codex,
            "hi",
            None,
            None,
            None,
            None,
            Some("gpt-5.6-sol"),
            None,
            ChatRuntime::NativeCli,
        );

        assert!(!args.iter().any(|arg| arg.starts_with("service_tier=")));
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("features.fast_mode="))
        );
    }

    #[test]
    fn native_runtime_does_not_override_codex_provider() {
        let args = default_agent_args_with_resume(
            Provider::Codex,
            "hi",
            None,
            None,
            None,
            None,
            None,
            None,
            ChatRuntime::NativeCli,
        );

        assert!(!args.iter().any(|arg| arg.starts_with("model_provider=")));
        assert!(!args.iter().any(|arg| arg == "--model"));
    }

    #[test]
    fn extracts_direct_ai_text_from_common_response_shapes() {
        let anthropic = serde_json::json!({
            "content": [{ "type": "text", "text": "hello" }]
        });
        assert_eq!(extract_direct_ai_response_text(&anthropic), "hello");

        let chat = serde_json::json!({
            "choices": [{ "message": { "content": "world" } }]
        });
        assert_eq!(extract_direct_ai_response_text(&chat), "world");

        let responses = serde_json::json!({
            "output": [{ "content": [{ "text": "done" }] }]
        });
        assert_eq!(extract_direct_ai_response_text(&responses), "done");
    }

    #[test]
    fn extracts_direct_ai_stream_deltas_from_common_sse_shapes() {
        let chat = serde_json::json!({
            "choices": [{ "delta": { "content": "hel" } }]
        });
        assert_eq!(extract_direct_ai_stream_delta(&chat), "hel");

        let anthropic = serde_json::json!({
            "type": "content_block_delta",
            "delta": { "type": "text_delta", "text": "lo" }
        });
        assert_eq!(extract_direct_ai_stream_delta(&anthropic), "lo");

        let responses = serde_json::json!({
            "type": "response.output_text.delta",
            "delta": "!"
        });
        assert_eq!(extract_direct_ai_stream_delta(&responses), "!");
    }

    #[test]
    fn direct_ai_display_chunks_round_trip_text() {
        let text =
            "alpha beta gamma\nSTREAM_MOBILE_LINE_1\nSTREAM_MOBILE_LINE_2\nunicode: cepat bisa";
        let chunks = direct_ai_display_chunks(text);
        assert!(chunks.len() > 1);
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn native_models_keep_selected_provider_runtime() {
        assert_eq!(
            effective_agent_command_provider(Provider::Claude, Some("claude-sonnet-4-5")),
            Provider::Claude
        );
        assert_eq!(
            effective_agent_command_provider(Provider::Gemini, Some("gemini-2.5-pro")),
            Provider::Gemini
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replay_history_is_bounded_by_bytes() {
        let mut manager = AgentRuntimeManager::new(1);
        manager.max_replay_bytes = 900;
        let (abort_tx, _abort_rx) = oneshot::channel();
        let key = "codex:replay-test".to_string();
        manager.register(key.clone(), abort_tx).await;
        let hub = WsHub::new();
        for sequence in 1..=4 {
            manager
                .publish(
                    &hub,
                    &key,
                    WsServerEvent::Output {
                        provider: Provider::Codex,
                        session_id: "replay-test".to_string(),
                        response_id: Some("response-1".to_string()),
                        sequence: Some(sequence),
                        content: "x".repeat(400),
                        done: false,
                    },
                )
                .await;
        }

        let replay = manager.replay_events().await;
        assert_eq!(replay.len(), 1);
        assert!(
            replay.iter().map(ws_event_estimated_bytes).sum::<usize>() <= manager.max_replay_bytes
        );
        assert!(matches!(
            replay.last(),
            Some(WsServerEvent::Output {
                sequence: Some(4),
                ..
            })
        ));
    }

    #[test]
    fn looks_like_proxy_model_recognizes_known_prefixes() {
        assert!(looks_like_proxy_model("agw:claude-opus-4-6-thinking"));
        assert!(looks_like_proxy_model("cod:gpt-5.4-mini"));
        assert!(looks_like_proxy_model("AGW:foo"));
        assert!(looks_like_proxy_model("cld:claude-haiku-4-5-20251001"));
        assert!(looks_like_proxy_model("gem:gemini-2.5-pro"));
        assert!(looks_like_proxy_model("cop:gpt-4o"));
        assert!(looks_like_proxy_model("proxy:bar"));
        assert!(!looks_like_proxy_model("gpt-5-codex"));
        assert!(!looks_like_proxy_model("claude-sonnet-4-5"));
        assert!(!looks_like_proxy_model("o4-mini"));
        assert!(!looks_like_proxy_model("unknown:model"));
    }
}

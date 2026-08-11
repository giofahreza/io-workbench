use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    ffi::OsString,
    fs::OpenOptions,
    future::Future,
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

mod external_sessions;

use bcrypt::{DEFAULT_COST, hash, verify};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use iowb_fs::{FileService, WorkspacePathValidator};
use iowb_process::ProcessManager;
use iowb_protocol::{
    AuthStatusResponse, AuthTokenResponse, CONFIG_DIR_NAME, ChatMessage, ChatRuntime,
    DATABASE_FILE_NAME, MessageRole, PRODUCT_NAME, ProjectSummary, PromptHistoryCursor,
    PromptHistoryEntry, Provider, ServerStatusResponse, SessionSummary, SessionTitleSource,
    UserProfile, WsServerEvent, new_id, session_title_from_prompt,
};
use iowb_storage::{Storage, StoredDurableChatRun};
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{RwLock, broadcast, mpsc, oneshot},
};
use tracing::{info, warn};
use uuid::Uuid;

use external_sessions::{
    ExternalSessionRecord, discover_external_sessions, load_external_messages,
    looks_like_codex_live_transcript, same_project_path,
};

type HmacSha1 = Hmac<Sha1>;
const DIRECT_AI_DISPLAY_CHUNK_CHARS: usize = 36;
const DIRECT_AI_SYNTHETIC_CHUNK_DELAY_MS: u64 = 45;
const DIRECT_AI_HISTORY_MAX_MESSAGES: usize = 48;
const DIRECT_AI_HISTORY_MAX_BYTES: usize = 96 * 1024;
const AGENT_LIVE_EVENT_MAX_BYTES: usize = 256 * 1024;
const AGENT_WEBSOCKET_CHUNK_MAX_BYTES: usize = 32 * 1024;
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
        runtime: ChatRuntime,
        direct_ai_config: Option<DirectAiRuntimeConfig>,
        user_id: Option<String>,
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

        let (external, native_resume_session_id) = if let Some(session_id) = session_id.as_deref() {
            let stored_session = self.sessions.get_stored(session_id);
            let external = self
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
                    .is_some_and(|session| session.external);
            let native_resume_session_id = if external {
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

        let mut session = self
            .sessions
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
            )
            .await?;
        if !prompt.trim().is_empty() {
            // Stamp the user-prompt row with the override footer used for this
            // turn so the chat-line footer survives a refresh / session
            // switch. Earlier turns are unaffected because each row carries
            // its own metadata.
            let now = Utc::now();
            let user_metadata = serde_json::json!({
                "cli": provider.as_str(),
                "model": model.clone().unwrap_or_default(),
                "runtime": runtime,
                "effort": effort.clone().unwrap_or_default(),
                "mode": mode.clone().unwrap_or_default(),
                "thinking": thinking.unwrap_or(false),
                "sentAt": now.to_rfc3339(),
            });
            self.sessions
                .append_message_with_metadata(
                    &session.id,
                    MessageRole::User,
                    prompt.clone(),
                    Some(user_metadata),
                )
                .await?;
            // Stamp first-user-at timestamp so the UI can show "sent at".
            // The session map entry is updated in place so subsequent turns
            // keep the original "first sent" timestamp instead of clobbering
            // it with the new prompt's now.
            {
                let mut sessions = self.sessions.sessions.write().await;
                if let Some(existing) = sessions.get_mut(&session.id) {
                    existing.first_user_at.get_or_insert(now);
                    if let Err(error) = self.storage.upsert_session(existing) {
                        warn!(error = %error, session_id = %session.id, "failed to persist first_user_at");
                    }
                } else {
                    let mut cloned = session.clone();
                    cloned.first_user_at.get_or_insert(now);
                    self.storage.upsert_session(&cloned)?;
                }
            }
            session = self.sessions.get(&session.id).await?;
        }

        let direct_ai_messages = if should_use_direct_ai_gateway_runtime(provider, model.as_deref())
        {
            direct_ai_conversation_messages(self.sessions.messages(&session.id)?, prompt.as_str())
        } else {
            Vec::new()
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
        if let Err(error) = self.storage.create_durable_chat_run(&durable_run) {
            let _ = self.sessions.set_active(&session.id, false).await;
            return Err(error.into());
        }

        let start_result = self
            .agents
            .start(AgentStartContext {
                provider,
                session_id: session.id.clone(),
                durable_run_id: Some(durable_run_id.clone()),
                response_id: new_id("response"),
                sequence: Arc::new(AtomicU64::new(0)),
                project_path: resolved_project_path,
                prompt,
                model,
                runtime,
                effort: effort.clone(),
                mode: mode.clone(),
                thinking,
                native_resume_session_id,
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
            .get_session(&run.session_id)?
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

        let mut native_resume_session_id = run
            .native_session_id
            .clone()
            .or_else(|| stored_session.native_session_id.clone());
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
        if let Some(native_session_id) = native_resume_session_id.as_deref() {
            self.storage
                .update_durable_chat_run_native_session_id(&run.id, Some(native_session_id))?;
            self.sessions
                .set_native_session_id(&run.session_id, native_session_id)
                .await?;
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

        let start_result = self
            .agents
            .start(AgentStartContext {
                provider,
                session_id: run.session_id.clone(),
                durable_run_id: Some(run.id.clone()),
                response_id: new_id("response"),
                sequence: Arc::new(AtomicU64::new(0)),
                project_path: resolved_project_path,
                prompt: recovery_prompt,
                model: run.model.clone(),
                runtime,
                effort: run.effort.clone(),
                mode: run.mode.clone(),
                thinking: run.thinking,
                native_resume_session_id,
                direct_ai_config,
                direct_ai_messages,
                sessions: self.sessions.clone(),
                storage: self.storage.clone(),
                hub: self.ws_hub.clone(),
            })
            .await;

        if let Err(error) = start_result {
            let message = error.to_string();
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
    max_sessions: usize,
    external_home: Arc<PathBuf>,
    external_cache: Arc<RwLock<ExternalSessionCache>>,
}

#[derive(Default)]
struct ExternalSessionCache {
    loaded_at: Option<Instant>,
    records: Vec<ExternalSessionRecord>,
    messages: HashMap<String, CachedExternalMessages>,
}

#[derive(Clone)]
struct CachedExternalMessages {
    modified_at: Option<SystemTime>,
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
        let sessions = storage
            .list_sessions()?
            .into_iter()
            .take(max_sessions)
            .map(|session| (session.id.clone(), session))
            .collect();

        Ok(Self {
            storage,
            sessions: Arc::new(RwLock::new(sessions)),
            max_sessions,
            external_home: Arc::new(
                env_path("IO_WORKBENCH_CLI_HOME")
                    .or_else(dirs::home_dir)
                    .unwrap_or_default(),
            ),
            external_cache: Arc::new(RwLock::new(ExternalSessionCache::default())),
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
    ) -> Result<SessionSummary> {
        let id = session_id.unwrap_or_else(|| new_id("session"));
        let now = Utc::now();
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .entry(id.clone())
            .or_insert_with(|| SessionSummary {
                id: id.clone(),
                provider,
                external,
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
                last_message_at: None,
                first_user_at: None,
                received_at: None,
                token_usage: None,
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
        session.last_activity = now;
        session.active = true;

        self.storage.upsert_session(session)?;
        let updated = session.clone();
        self.evict_if_needed(&mut sessions)?;
        Ok(updated)
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
            .update_message_metadata(session_id, &id, metadata)?)
    }

    pub async fn set_active(&self, session_id: &str, active: bool) -> Result<SessionSummary> {
        let mut sessions = self.sessions.write().await;
        if !sessions.contains_key(session_id)
            && let Some(stored) = self.storage.get_session(session_id)?
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
            && let Some(stored) = self.storage.get_session(session_id)?
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
        self.sessions
            .read()
            .await
            .values()
            .filter(|session| session.active)
            .cloned()
            .collect()
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
            if let Some(existing) = sessions.iter_mut().find(|session| {
                session.id == record.summary.id && session.provider == record.summary.provider
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
                    *existing = record.summary;
                    existing.active = active;
                    if preserve_local_title {
                        existing.title = title;
                        existing.title_source = title_source;
                    }
                    existing.model = model;
                    existing.effort = effort;
                    existing.mode = mode;
                    existing.thinking = thinking;
                }
            } else {
                sessions.push(record.summary);
            }
        }
        sessions.sort_by_key(|session| std::cmp::Reverse(session.last_activity));
        Ok(sessions)
    }

    pub fn messages(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        Ok(self.storage.list_messages(session_id)?)
    }

    pub async fn messages_including_external(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
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
            cache.messages.remove(&external_session_cache_key(&record));
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
            .list_messages_page(session_id, limit.max(1).min(500), offset)?)
    }

    pub async fn messages_page_including_external(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<ChatMessage>, usize)> {
        if let Some(messages) = self.external_messages_for_session(session_id).await? {
            let total = messages.len();
            let start = offset.min(total);
            let end = start.saturating_add(limit.max(1).min(500)).min(total);
            return Ok((messages[start..end].to_vec(), total));
        }
        self.messages_page(session_id, limit, offset)
    }

    pub async fn messages_tail_including_external(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<(Vec<ChatMessage>, usize)> {
        let limit = limit.max(1).min(500);
        if let Some(messages) = self.external_messages_for_session(session_id).await? {
            let total = messages.len();
            let start = total.saturating_sub(limit);
            return Ok((messages[start..].to_vec(), total));
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
        let limit = limit.max(1).min(500);
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
        if let Some(session) = self.sessions.read().await.get(session_id).cloned() {
            return Ok(session);
        }

        if let Some(session) = self.storage.get_session(session_id)? {
            return Ok(session);
        }
        self.external_record(session_id, None, None)
            .await
            .map(|record| record.summary)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))
    }

    fn get_stored(&self, session_id: &str) -> Option<SessionSummary> {
        self.storage.get_session(session_id).ok().flatten()
    }

    async fn external_records(&self) -> Vec<ExternalSessionRecord> {
        const CACHE_TTL: Duration = Duration::from_secs(30);
        let mut records = {
            let cache = self.external_cache.read().await;
            if cache
                .loaded_at
                .is_some_and(|loaded_at| loaded_at.elapsed() < CACHE_TTL)
            {
                cache.records.clone()
            } else {
                let stale_records = cache.records.clone();
                drop(cache);
                let external_home = self.external_home.clone();
                let records = match tokio::task::spawn_blocking(move || {
                    discover_external_sessions(&external_home)
                })
                .await
                {
                    Ok(records) => records,
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
                    record.summary.message_count = cached.messages.len();
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
        records
            .into_iter()
            .filter(|record| !mapped_native_ids.contains(&record.summary.id))
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
            .or_else(|| self.storage.get_session(session_id).ok().flatten())?;
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

    async fn external_messages(&self, record: &ExternalSessionRecord) -> Arc<Vec<ChatMessage>> {
        let key = external_session_cache_key(record);
        let modified_at = std::fs::metadata(&record.file_path)
            .and_then(|metadata| metadata.modified())
            .ok();
        {
            let cache = self.external_cache.read().await;
            if let Some(cached) = cache.messages.get(&key) {
                if cached.modified_at == modified_at {
                    return cached.messages.clone();
                }
            }
        }

        let record = record.clone();
        let messages =
            match tokio::task::spawn_blocking(move || load_external_messages(&record)).await {
                Ok(messages) => Arc::new(messages),
                Err(error) => {
                    warn!(%error, "external session parser worker failed");
                    return Arc::new(Vec::new());
                }
            };
        let mut cache = self.external_cache.write().await;
        cache.messages.insert(
            key,
            CachedExternalMessages {
                modified_at,
                messages: messages.clone(),
            },
        );
        while cache.messages.len() > 64 {
            let Some(key) = cache.messages.keys().next().cloned() else {
                break;
            };
            cache.messages.remove(&key);
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
            .or_else(|| self.storage.get_session(session_id).ok().flatten())
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
            .or_else(|| self.storage.get_session(session_id).ok().flatten())
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
        .or_else(|| self.storage.get_session(session_id).ok().flatten());
        let session = match session {
            Some(session) => session,
            None => self
                .external_record(session_id, None, None)
                .await
                .map(|record| record.summary)
                .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?,
        };
        if session.external {
            self.storage
                .tombstone_session(session_id, session.provider)?;
        }
        if !self.storage.delete_session(session_id)? {
            if !session.external {
                return Err(CoreError::SessionNotFound(session_id.to_string()));
            }
        }
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
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

fn merge_mapped_external_messages(
    stored: Vec<ChatMessage>,
    mut external: Vec<ChatMessage>,
) -> Vec<ChatMessage> {
    let mut matched_stored = vec![false; stored.len()];
    for external_message in &mut external {
        let Some((index, stored_message)) = stored.iter().enumerate().find(|(index, message)| {
            !matched_stored[*index]
                && message.role == external_message.role
                && message.content.trim() == external_message.content.trim()
        }) else {
            continue;
        };
        matched_stored[index] = true;
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

#[derive(Clone)]
pub struct AgentRuntimeManager {
    runs: Arc<RwLock<HashMap<String, AgentRuntimeRecord>>>,
    max_runs: usize,
    max_replay_events: usize,
    max_output_bytes: usize,
}

struct AgentRuntimeRecord {
    replay: VecDeque<WsServerEvent>,
    abort_tx: Option<oneshot::Sender<()>>,
    last_activity: DateTime<Utc>,
}

#[derive(Clone)]
struct AgentStartContext {
    provider: Provider,
    session_id: String,
    durable_run_id: Option<String>,
    response_id: String,
    sequence: Arc<AtomicU64>,
    project_path: PathBuf,
    prompt: String,
    model: Option<String>,
    runtime: ChatRuntime,
    effort: Option<String>,
    mode: Option<String>,
    thinking: Option<bool>,
    native_resume_session_id: Option<String>,
    direct_ai_config: Option<DirectAiRuntimeConfig>,
    direct_ai_messages: Vec<DirectAiConversationMessage>,
    sessions: SessionManager,
    storage: iowb_storage::Storage,
    hub: WsHub,
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

#[derive(Default)]
struct CodexLiveOutputNormalizer {
    pending_line: String,
    pending_agent_message: Option<String>,
    pending_thread_id: Option<String>,
    final_assistant_message: Option<String>,
    saw_structured_event: bool,
    tool_messages: Vec<NormalizedToolMessage>,
    tool_message_bytes: usize,
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
}

impl AgentRuntimeManager {
    pub fn new(max_runs: usize) -> Self {
        Self {
            runs: Arc::new(RwLock::new(HashMap::new())),
            max_runs,
            max_replay_events: 256,
            max_output_bytes: AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
        }
    }

    async fn start(&self, context: AgentStartContext) -> Result<()> {
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
                    },
                )
                .await;
                self.finish(
                    &key,
                    &context,
                    iowb_protocol::SessionRuntimeStatus::Failed,
                    Some(error_message),
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
                    },
                )
                .await;
                self.finish(
                    &key,
                    &context,
                    iowb_protocol::SessionRuntimeStatus::Failed,
                    Some(error_message),
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
                    },
                )
                .await;
                self.finish(
                    &key,
                    &context,
                    iowb_protocol::SessionRuntimeStatus::Failed,
                    Some(error_message),
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
            let mut claude_normalizer =
                (runtime_provider == Provider::Claude).then(ClaudeLiveOutputNormalizer::default);
            let mut gemini_normalizer =
                (runtime_provider == Provider::Gemini).then(GeminiLiveOutputNormalizer::default);
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
                        let codex_final_assistant = codex_normalizer
                            .as_mut()
                            .and_then(CodexLiveOutputNormalizer::take_final_assistant_message);
                        let claude_final_assistant = claude_normalizer
                            .as_mut()
                            .and_then(ClaudeLiveOutputNormalizer::take_final_assistant_message);
                        persist_codex_tool_messages(&context, &mut codex_normalizer).await;
                        let provider_specific_final = codex_final_assistant
                            .or(claude_final_assistant);
                        match status {
                            Ok(status) if status.success() => {
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
                                        ).await;
                                    }
                                    Err(error_output) => {
                                        manager.publish(&context.hub, &key, WsServerEvent::Error {
                                            message: "Codex completed without a final assistant response".to_string(),
                                            details: Some(
                                                "The Codex process exited successfully, but its event stream did not contain a final assistant message. The accumulated CLI transcript was not saved as the reply."
                                                    .to_string(),
                                            ),
                                        }).await;
                                        manager.finish(
                                            &key,
                                            &context,
                                            iowb_protocol::SessionRuntimeStatus::Failed,
                                            Some(error_output),
                                        ).await;
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
                                ).await;
                            }
                            Err(error) => {
                                let persisted_output = provider_specific_final
                                    .unwrap_or_else(|| output.clone());
                                manager.publish(&context.hub, &key, WsServerEvent::Error {
                                    message: "agent process wait failed".to_string(),
                                    details: Some(error.to_string()),
                                }).await;
                                manager.finish(
                                    &key,
                                    &context,
                                    iowb_protocol::SessionRuntimeStatus::Failed,
                                    Some(persisted_output.clone()),
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
                        ).await;
                        break;
                    }
                    else => break,
                }
            }
        });

        Ok(())
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
                },
            )
            .await;
            self.finish(
                &key,
                &context,
                iowb_protocol::SessionRuntimeStatus::Failed,
                Some(error_message),
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
                },
            )
            .await;
            self.finish(
                &key,
                &context,
                iowb_protocol::SessionRuntimeStatus::Failed,
                Some(error_message),
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
                            ).await;
                        }
                        Err(error) => {
                            let error_message = format!("Direct AI gateway request failed\n\n{error}");
                            manager.publish(&context.hub, &key, WsServerEvent::Error {
                                message: "Direct AI gateway request failed".to_string(),
                                details: Some(error_message.clone()),
                            }).await;
                            manager.finish(
                                &key,
                                &context,
                                iowb_protocol::SessionRuntimeStatus::Failed,
                                Some(error_message),
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

    async fn publish(&self, hub: &WsHub, key: &str, event: WsServerEvent) {
        {
            let mut runs = self.runs.write().await;
            if let Some(record) = runs.get_mut(key) {
                record.last_activity = Utc::now();
                if record.replay.len() >= self.max_replay_events {
                    record.replay.pop_front();
                }
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
    ) {
        let received_at = Utc::now();
        let output = assistant_output
            .map(|output| output.trim().to_string())
            .filter(|output| !output.is_empty())
            .or_else(|| match status {
                iowb_protocol::SessionRuntimeStatus::Failed => Some("Failed".to_string()),
                iowb_protocol::SessionRuntimeStatus::Aborted => Some("Aborted".to_string()),
                _ => None,
            });
        if let Some(output) = output {
            let output = bound_agent_text(
                &output,
                AGENT_ASSISTANT_MESSAGE_MAX_BYTES,
                "assistant response",
            );
            let persisted_output = output.clone();
            // Persist the assistant message with footer metadata so the
            // bubble at the bottom of the reply stays populated after a
            // refresh or session switch.
            let sent_at = context
                .storage
                .get_session(&context.session_id)
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

        if let Some(run_id) = context.durable_run_id.as_deref() {
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

        // Stamp metadata so the UI can show "received at" and the conversation
        // metadata snapshot. Token usage is fetched separately by the UI via
        // /api/projects/{name}/sessions/{id}/token-usage.
        if let Ok(Some(mut session)) = context.storage.get_session(&context.session_id) {
            session.received_at = Some(received_at);
            session.last_message_at = Some(received_at);
            session.last_activity = received_at;
            session.effort = context.effort.clone().or(session.effort);
            session.mode = context.mode.clone().or(session.mode);
            session.thinking = context.thinking.or(session.thinking);
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
                    received_at,
                    last_message_at: snapshot.last_message_at,
                    first_user_at: snapshot.first_user_at,
                    token_usage: snapshot.token_usage,
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
        runs.values()
            .filter(|record| record.abort_tx.is_some())
            .flat_map(|record| record.replay.iter().cloned())
            .collect()
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

    while let Some(bytes) = response
        .chunk()
        .await
        .map_err(|error| format!("Direct AI response stream failed: {error}"))?
    {
        raw.extend_from_slice(&bytes);
        line_buffer.push_str(&String::from_utf8_lossy(&bytes));
        drain_direct_ai_sse_lines(&mut line_buffer, &mut text, &mut streamed, on_chunk).await;
    }
    if !line_buffer.trim().is_empty() {
        process_direct_ai_sse_line(line_buffer.trim(), &mut text, &mut streamed, on_chunk).await;
    }

    if streamed {
        return Ok(DirectAiStreamOutput { text, streamed });
    }

    let value = serde_json::from_slice::<Value>(&raw)
        .map_err(|error| format!("Direct AI returned invalid JSON: {error}"))?;
    Ok(DirectAiStreamOutput {
        text: extract_direct_ai_response_text(&value),
        streamed: false,
    })
}

async fn drain_direct_ai_sse_lines<F, Fut>(
    buffer: &mut String,
    text: &mut String,
    streamed: &mut bool,
    on_chunk: &mut F,
) where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = ()>,
{
    while let Some(index) = buffer.find('\n') {
        let line = buffer[..index].trim_end_matches('\r').to_string();
        buffer.drain(..index + 1);
        process_direct_ai_sse_line(&line, text, streamed, on_chunk).await;
    }
}

async fn process_direct_ai_sse_line<F, Fut>(
    line: &str,
    text: &mut String,
    streamed: &mut bool,
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

#[allow(clippy::too_many_arguments)]
fn resolve_agent_command(
    provider: Provider,
    prompt: &str,
    session_id: &str,
    model: Option<&str>,
    effort: Option<&str>,
    mode: Option<&str>,
    thinking: Option<bool>,
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
        provider, prompt, mode, effort, thinking, model, None, runtime,
    )
}

#[allow(clippy::too_many_arguments)]
fn default_agent_args_with_resume(
    provider: Provider,
    prompt: &str,
    mode: Option<&str>,
    effort: Option<&str>,
    thinking: Option<bool>,
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
            last_message_at: Some(now),
            first_user_at: Some(now),
            received_at: None,
            token_usage: None,
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
                "{}\n{}\n",
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
                })
            ),
        )
        .expect("historical rollout");

        let storage = Storage::open(config_dir.join("test.db")).expect("storage");
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
        assert!(
            listed
                .iter()
                .any(|session| session.id == historical_native_id),
            "unmapped historical rollout must remain discoverable: {listed:#?}"
        );
        assert!(
            listed.iter().any(|session| session.id == internal.id),
            "internal chat must remain visible: {listed:#?}"
        );

        let args = default_agent_args_with_resume(
            Provider::Codex,
            "second prompt",
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
    fn codex_gateway_provider_config_precedes_resume_positional() {
        let mut args = default_agent_args_with_resume(
            Provider::Codex,
            "continue",
            Some("bypass"),
            Some("medium"),
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
    fn native_runtime_does_not_override_codex_provider() {
        let args = default_agent_args_with_resume(
            Provider::Codex,
            "hi",
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

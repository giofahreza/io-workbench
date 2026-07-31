use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    ffi::OsString,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

mod external_sessions;

use bcrypt::{DEFAULT_COST, hash, verify};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use iowb_fs::{FileService, WorkspacePathValidator};
use iowb_process::ProcessManager;
use iowb_protocol::{
    AuthStatusResponse, AuthTokenResponse, CONFIG_DIR_NAME, ChatMessage, DATABASE_FILE_NAME,
    MessageRole, PRODUCT_NAME, ProjectSummary, Provider, ServerStatusResponse, SessionSummary,
    UserProfile, WsServerEvent, new_id,
};
use iowb_storage::Storage;
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
    ExternalSessionRecord, discover_external_sessions, load_external_messages, same_project_path,
};

type HmacSha1 = Hmac<Sha1>;
const DIRECT_AI_DISPLAY_CHUNK_CHARS: usize = 36;
const DIRECT_AI_SYNTHETIC_CHUNK_DELAY_MS: u64 = 45;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("storage error: {0}")]
    Storage(#[from] iowb_storage::StorageError),
    #[error("filesystem error: {0}")]
    Fs(#[from] iowb_fs::FsError),
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
        direct_ai_config: Option<DirectAiRuntimeConfig>,
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

        let session = self
            .sessions
            .create_or_update(
                provider,
                resolved_project_path.display().to_string(),
                session_id,
                external,
                model.clone(),
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
        }

        self.agents
            .start(AgentStartContext {
                provider,
                session_id: session.id.clone(),
                project_path: resolved_project_path,
                prompt,
                model,
                effort: effort.clone(),
                mode: mode.clone(),
                thinking,
                native_resume_session_id,
                direct_ai_config,
                sessions: self.sessions.clone(),
                storage: self.storage.clone(),
                hub: self.ws_hub.clone(),
            })
            .await?;

        self.ws_hub.publish(WsServerEvent::ActiveSessions {
            sessions: self.sessions.list_active().await,
        });

        Ok(session)
    }

    pub async fn abort_agent_session(&self, provider: Provider, session_id: &str) -> Result<bool> {
        let aborted = self.agents.abort(provider, session_id).await;
        if !aborted {
            let _ = self.sessions.set_active(session_id, false).await?;
            self.ws_hub.publish(WsServerEvent::SessionStatus {
                provider,
                session_id: session_id.to_string(),
                status: iowb_protocol::SessionRuntimeStatus::Aborted,
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
}

impl SessionManager {
    pub fn load(storage: Storage, max_sessions: usize) -> Result<Self> {
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

    #[allow(clippy::too_many_arguments)]
    pub async fn create_or_update(
        &self,
        provider: Provider,
        project_path: impl Into<String>,
        session_id: Option<String>,
        external: bool,
        model: Option<String>,
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
                effort: effort.clone(),
                mode: mode.clone(),
                thinking,
                last_message_at: None,
                first_user_at: None,
                received_at: None,
                token_usage: None,
                native_session_id: None,
            });

        session.provider = provider;
        session.external = external;
        if let Some(model) = model {
            session.model = Some(model);
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
            if role == MessageRole::User && session.title == "New Session" {
                session.title = summarize(&message.content);
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
        if provider != Provider::Codex {
            return Ok(None);
        }
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

        let candidate = self
            .external_records()
            .await
            .into_iter()
            .filter(|record| {
                record.summary.provider == provider
                    && same_project_path(&record.summary.project_path, project_path)
            })
            .filter(|record| {
                load_external_messages(record).into_iter().any(|message| {
                    message.role == MessageRole::User && message.content == last_user_prompt
                })
            })
            .max_by_key(|record| record.summary.last_activity);

        let Some(candidate) = candidate else {
            return Ok(None);
        };
        let native_session_id = candidate.summary.id;
        self.set_native_session_id(session_id, native_session_id.clone())
            .await?;
        info!(
            session_id,
            native_session_id = %native_session_id,
            "reconciled existing workbench session with native Codex thread"
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
                    let model = existing.model.clone().or(record.summary.model.clone());
                    let effort = existing.effort.clone();
                    let mode = existing.mode.clone();
                    let thinking = existing.thinking;
                    *existing = record.summary;
                    existing.active = active;
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
        if let Some(record) = self.external_record(session_id, None, None).await {
            let messages = load_external_messages(&record);
            if !messages.is_empty() {
                return Ok(messages);
            }
        }
        self.messages(session_id)
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
        if let Some(record) = self.external_record(session_id, None, None).await {
            let messages = load_external_messages(&record);
            if !messages.is_empty() {
                let total = messages.len();
                let start = offset.min(total);
                let end = start.saturating_add(limit.max(1).min(500)).min(total);
                return Ok((messages[start..end].to_vec(), total));
            }
        }
        self.messages_page(session_id, limit, offset)
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
        let records = {
            let cache = self.external_cache.read().await;
            if cache
                .loaded_at
                .is_some_and(|loaded_at| loaded_at.elapsed() < CACHE_TTL)
            {
                cache.records.clone()
            } else {
                drop(cache);
                let records = discover_external_sessions(&self.external_home);
                let mut cache = self.external_cache.write().await;
                cache.loaded_at = Some(Instant::now());
                cache.records = records.clone();
                records
            }
        };

        let mapped_native_ids = self
            .sessions
            .read()
            .await
            .values()
            .filter(|session| !session.external)
            .filter_map(|session| session.native_session_id.clone())
            .collect::<HashSet<_>>();
        records
            .into_iter()
            .filter(|record| !mapped_native_ids.contains(&record.summary.id))
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
        session.last_activity = Utc::now();
        self.storage.upsert_session(&session)?;
        sessions.insert(session.id.clone(), session.clone());
        self.evict_if_needed(&mut sessions)?;
        Ok(session)
    }

    pub async fn delete(&self, session_id: &str) -> Result<SessionSummary> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get(session_id)
            .cloned()
            .or_else(|| self.storage.get_session(session_id).ok().flatten())
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;
        if !self.storage.delete_session(session_id)? {
            return Err(CoreError::SessionNotFound(session_id.to_string()));
        }
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
    project_path: PathBuf,
    prompt: String,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    thinking: Option<bool>,
    native_resume_session_id: Option<String>,
    direct_ai_config: Option<DirectAiRuntimeConfig>,
    sessions: SessionManager,
    storage: iowb_storage::Storage,
    hub: WsHub,
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
}

impl AgentRuntimeManager {
    pub fn new(max_runs: usize) -> Self {
        Self {
            runs: Arc::new(RwLock::new(HashMap::new())),
            max_runs,
            max_replay_events: 256,
            max_output_bytes: 1024 * 1024,
        }
    }

    async fn start(&self, context: AgentStartContext) -> Result<()> {
        if context.native_resume_session_id.is_none()
            && should_use_direct_ai_gateway_runtime(context.provider, context.model.as_deref())
        {
            return self.start_direct_ai(context).await;
        }

        let command = resolve_agent_command(
            context.provider,
            &context.prompt,
            &context.session_id,
            context.model.as_deref(),
            context.effort.as_deref(),
            context.mode.as_deref(),
            context.thinking,
            context.native_resume_session_id.as_deref(),
        )?;
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

        let mut child = match child_command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.publish(
                    &context.hub,
                    &key,
                    WsServerEvent::Error {
                        message: "failed to spawn agent provider".to_string(),
                        details: Some(format!("{}: {}", command.command, error)),
                    },
                )
                .await;
                self.finish(
                    &key,
                    &context,
                    iowb_protocol::SessionRuntimeStatus::Failed,
                    None,
                )
                .await;
                return Ok(());
            }
        };

        if command.stdin_prompt {
            if let Some(mut stdin) = child.stdin.take() {
                let prompt = context.prompt.clone();
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
            },
        )
        .await;

        let manager = self.clone();
        tokio::spawn(async move {
            let mut abort_rx = abort_rx;
            let mut output = String::new();
            let mut codex_normalizer =
                (context.provider == Provider::Codex).then(CodexLiveOutputNormalizer::default);
            loop {
                tokio::select! {
                    Some(event) = output_rx.recv() => {
                        process_agent_event(
                            &manager,
                            &context,
                            &key,
                            event,
                            &mut codex_normalizer,
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
                        match status {
                            Ok(status) if status.success() => {
                                manager.finish(
                                    &key,
                                    &context,
                                    iowb_protocol::SessionRuntimeStatus::Completed,
                                    Some(output.clone()),
                                ).await;
                            }
                            Ok(status) => {
                                append_bounded(
                                    &mut output,
                                    &format!("\nAgent exited with status {status}"),
                                    manager.max_output_bytes,
                                );
                                manager.finish(
                                    &key,
                                    &context,
                                    iowb_protocol::SessionRuntimeStatus::Failed,
                                    Some(output.clone()),
                                ).await;
                            }
                            Err(error) => {
                                manager.publish(&context.hub, &key, WsServerEvent::Error {
                                    message: "agent process wait failed".to_string(),
                                    details: Some(error.to_string()),
                                }).await;
                                manager.finish(
                                    &key,
                                    &context,
                                    iowb_protocol::SessionRuntimeStatus::Failed,
                                    Some(output.clone()),
                                ).await;
                            }
                        }
                        break;
                    }
                    _ = &mut abort_rx => {
                        let _ = child.kill().await;
                        while let Some(event) = output_rx.recv().await {
                            process_agent_event(
                                &manager,
                                &context,
                                &key,
                                event,
                                &mut codex_normalizer,
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
                        manager.finish(
                            &key,
                            &context,
                            iowb_protocol::SessionRuntimeStatus::Aborted,
                            Some(output.clone()),
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
            },
        )
        .await;

        let Some(config) = context.direct_ai_config.clone() else {
            self.publish(
                &context.hub,
                &key,
                WsServerEvent::Error {
                    message: "Direct AI gateway is not configured".to_string(),
                    details: Some(
                        "Set Direct AI settings or CODEX_GATEWAY_KEY before using gateway models with Claude/Gemini."
                            .to_string(),
                    ),
                },
            )
            .await;
            self.finish(
                &key,
                &context,
                iowb_protocol::SessionRuntimeStatus::Failed,
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
            self.publish(
                &context.hub,
                &key,
                WsServerEvent::Error {
                    message: "Direct AI model is missing".to_string(),
                    details: None,
                },
            )
            .await;
            self.finish(
                &key,
                &context,
                iowb_protocol::SessionRuntimeStatus::Failed,
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
            },
        )
        .await;

        let manager = self.clone();
        tokio::spawn(async move {
            let mut abort_rx = abort_rx;
            tokio::select! {
                result = stream_direct_ai_model_api(&config, &model, &context.prompt, {
                    let hub = context.hub.clone();
                    let key = key.clone();
                    let provider = context.provider;
                    let session_id = context.session_id.clone();
                    let manager = manager.clone();
                    move |chunk: String| {
                        let hub = hub.clone();
                        let key = key.clone();
                        let session_id = session_id.clone();
                        let manager = manager.clone();
                        async move {
                            manager.publish(&hub, &key, WsServerEvent::Output {
                                provider,
                                session_id,
                                content: chunk,
                                done: false,
                            }).await;
                        }
                    }
                }) => {
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
                            manager.publish(&context.hub, &key, WsServerEvent::Error {
                                message: "Direct AI gateway request failed".to_string(),
                                details: Some(error),
                            }).await;
                            manager.finish(
                                &key,
                                &context,
                                iowb_protocol::SessionRuntimeStatus::Failed,
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
        if let Some(output) = assistant_output
            .map(|output| output.trim().to_string())
            .filter(|output| !output.is_empty())
        {
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
                "model": context.model.clone().unwrap_or_default(),
                "effort": context.effort.clone().unwrap_or_default(),
                "mode": context.mode.clone().unwrap_or_default(),
                "thinking": context.thinking.unwrap_or(false),
                "receivedAt": received_at.to_rfc3339(),
                "sentAt": sent_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                "elapsedMs": elapsed_ms,
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
                        "model": context.model.clone().unwrap_or_default(),
                        "effort": context.effort.clone().unwrap_or_default(),
                        "mode": context.mode.clone().unwrap_or_default(),
                        "thinking": context.thinking.unwrap_or(false),
                        "receivedAt": received_at.to_rfc3339(),
                        "sentAt": sent_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                        "elapsedMs": elapsed_ms,
                    });
                    let _ = context.sessions.stamp_latest_message_metadata(
                        &context.session_id,
                        MessageRole::Assistant,
                        updated,
                    );
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
            context.hub.publish(WsServerEvent::SessionMetadata {
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
            });
        }

        self.publish(
            &context.hub,
            key,
            WsServerEvent::Output {
                provider: context.provider,
                session_id: context.session_id.clone(),
                content: String::new(),
                done: true,
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

async fn stream_direct_ai_model_api<F, Fut>(
    config: &DirectAiRuntimeConfig,
    model: &str,
    prompt: &str,
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
    let messages_body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let chat_body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": [{ "role": "user", "content": prompt }],
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
) -> Result<AgentCommandSpec> {
    let command_provider = effective_agent_command_provider(provider, model);
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
    let args = if let Some(args_template) = args_template {
        let raw_args: Vec<String> = serde_json::from_str(&args_template).map_err(|error| {
            CoreError::InvalidInput(format!("invalid agent args JSON: {error}"))
        })?;
        raw_args
            .into_iter()
            .map(|arg| expand_agent_template(arg, prompt, session_id, model))
            .collect()
    } else {
        default_agent_args_with_resume(
            command_provider,
            prompt,
            mode,
            effort,
            thinking,
            model,
            native_resume_session_id,
        )
    };

    let stdin_prompt = env_bool(&format!("{provider_prefix}STDIN"), false)
        || env_bool("IO_WORKBENCH_AGENT_STDIN", false);

    Ok(AgentCommandSpec {
        command,
        args,
        stdin_prompt,
    })
}

fn effective_agent_command_provider(provider: Provider, model: Option<&str>) -> Provider {
    if model.is_some_and(uses_codex_aiproxy_cli_runtime) {
        Provider::Codex
    } else {
        provider
    }
}

fn should_use_direct_ai_gateway_runtime(provider: Provider, model: Option<&str>) -> bool {
    matches!(provider, Provider::Claude | Provider::Gemini)
        && model.is_some_and(|model| {
            looks_like_proxy_model(model) && !uses_codex_aiproxy_cli_runtime(model)
        })
}

fn uses_codex_aiproxy_cli_runtime(model: &str) -> bool {
    gateway_model_prefix(model).is_some_and(|prefix| prefix == "cod")
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
    default_agent_args_with_resume(provider, prompt, mode, effort, thinking, model, None)
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
) -> Vec<String> {
    let mut args: Vec<String> = match provider {
        Provider::Claude => {
            let mut args = vec!["--print".to_string()];
            if let Some(session_id) = resume_session_id {
                args.extend(["--resume".to_string(), session_id.to_string()]);
            }
            args.push(prompt.to_string());
            args
        }
        // Codex exec options must precede the `resume` subcommand. The prompt
        // and resume arguments are appended after all shared options below.
        Provider::Codex => vec!["exec".to_string(), "--json".to_string()],
        Provider::Gemini => {
            let mut args = vec!["--prompt".to_string(), prompt.to_string()];
            if let Some(session_id) = resume_session_id {
                args.extend(["--resume".to_string(), session_id.to_string()]);
            }
            args
        }
    };
    // Always relax Codex's directory / git repo enforcement so it can run in
    // arbitrary project directories, which is the common case for an embedded
    // workspace UI.
    if matches!(provider, Provider::Codex) {
        args.push("--skip-git-repo-check".to_string());
    }
    // Per-mode flags (Claude supports --permission-mode; Codex exec uses
    // --sandbox / --dangerously-bypass-approvals-and-sandbox). Provider-specific.
    if let Some(mode) = mode {
        match mode {
            "bypass" => match provider {
                Provider::Claude => {
                    args.push("--dangerously-skip-permissions".to_string());
                }
                Provider::Codex => {
                    args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
                }
                Provider::Gemini => {
                    args.push("--yolo".to_string());
                }
            },
            "accept-edits" => match provider {
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
            "plan" => match provider {
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
            "read-only" => match provider {
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
    // dropdown value the user actually picked is what gets sent.
    if let Some(model) = model {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            // Gateway catalog prefixes select the Codex model provider. MiniMax
            // is configured as its own provider and expects the bare model id;
            // the remaining routed aliases are resolved by aiproxy as-is.
            if matches!(provider, Provider::Codex) {
                if let Some(provider) = codex_model_provider_override(trimmed) {
                    push_codex_provider_override(&mut args, provider);
                }
            }
            if !args.iter().any(|a| a == "--model") {
                args.push("--model".to_string());
                args.push(codex_launch_model(trimmed).to_string());
            }
        }
    }
    if let Some(effort) = effort {
        match provider {
            Provider::Claude => {
                args.push("--effort".to_string());
                args.push(effort.to_string());
            }
            Provider::Codex => {
                let reasoning_effort = match effort {
                    "minimal" | "low" | "medium" | "high" | "xhigh" => effort,
                    "max" => "xhigh",
                    _ => "",
                };
                if !reasoning_effort.is_empty() {
                    push_codex_config_override(
                        &mut args,
                        "model_reasoning_effort",
                        &format!("\"{reasoning_effort}\""),
                    );
                }
            }
            Provider::Gemini => {}
        }
    }
    if thinking.unwrap_or(false) {
        if matches!(provider, Provider::Codex) {
            push_codex_config_override(&mut args, "model_reasoning_effort", "\"xhigh\"");
        } else if matches!(provider, Provider::Gemini) {
            args.push("--thinking".to_string());
        }
    }
    if matches!(provider, Provider::Codex) {
        if let Some(session_id) = resume_session_id {
            args.extend(["resume".to_string(), session_id.to_string()]);
        }
        args.push(prompt.to_string());
    }
    args
}

/// Returns true when a model id is served by a codex-side model_provider
/// rather than the default openai gateway. The antigravity/CLIProxyAPI
/// gateway exposes its models with namespace prefixes (`agw:` for routed
/// upstreams, `cod:` for codex-flavoured aliases) that the default
/// openai-backed codex profile cannot resolve. Callers should reroute
/// codex to the `aiproxy` provider when they see one of these prefixes.
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

fn codex_model_provider_override(model: &str) -> Option<&'static str> {
    gateway_model_prefix(model).map(|prefix| {
        if prefix == "min" {
            "minimax"
        } else {
            "aiproxy"
        }
    })
}

fn codex_launch_model(model: &str) -> &str {
    if gateway_model_prefix(model).as_deref() == Some("min") {
        model
            .split_once(':')
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
            .unwrap_or(model)
    } else {
        model
    }
}

/// Insert the `-c key=value` override before the `exec` positional so
/// codex applies the override to its subcommand invocation. Existing
/// overrides for the same key are skipped so we don't double up.
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
        args.push("-c".to_string());
        args.push(format!("{key}={value}"));
    }
}

fn expand_agent_template(
    value: String,
    prompt: &str,
    session_id: &str,
    model: Option<&str>,
) -> String {
    value
        .replace("{prompt}", prompt)
        .replace("{session_id}", session_id)
        .replace("{model}", model.unwrap_or(""))
}

fn agent_run_key(provider: Provider, session_id: &str) -> String {
    format!("{}:{session_id}", provider.as_str())
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

async fn process_agent_event(
    manager: &AgentRuntimeManager,
    context: &AgentStartContext,
    key: &str,
    event: AgentProcessEvent,
    codex_normalizer: &mut Option<CodexLiveOutputNormalizer>,
    output: &mut String,
) {
    match event {
        AgentProcessEvent::Output { stream, data } => {
            let (visible, native_session_id) = if stream == AgentOutputStream::Stdout {
                if let Some(normalizer) = codex_normalizer.as_mut() {
                    let visible = normalizer.push(&data);
                    (visible, normalizer.take_thread_id())
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

async fn persist_native_session_id(context: &AgentStartContext, native_session_id: Option<String>) {
    let Some(native_session_id) = native_session_id else {
        return;
    };
    match context
        .sessions
        .set_native_session_id(&context.session_id, native_session_id.clone())
        .await
    {
        Ok(_) => info!(
            session_id = %context.session_id,
            native_session_id = %native_session_id,
            "associated workbench session with native Codex thread"
        ),
        Err(error) => warn!(
            error = %error,
            session_id = %context.session_id,
            native_session_id = %native_session_id,
            "failed to persist native Codex thread id"
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
    append_bounded(output, &content, manager.max_output_bytes);
    manager
        .publish(
            &context.hub,
            key,
            WsServerEvent::Output {
                provider: context.provider,
                session_id: context.session_id.clone(),
                content,
                done: false,
            },
        )
        .await;
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
                Some("commentary") => format!("thinking\n{}", content.trim()),
                Some("final_answer") => {
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

        let mut output = self.take_pending_agent_message(true);
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
                    format!("codex\n{content}")
                }
            })
            .unwrap_or_default()
    }

    fn take_thread_id(&mut self) -> Option<String> {
        self.pending_thread_id.take()
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
    content
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
    content
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
        return format!(
            "exec / Parameters\n**Tool:** `{name}`\n\n### Command\n```sh\n{command}\n```"
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
    content
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
    content
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

fn summarize(content: &str) -> String {
    let trimmed = content.trim();
    if trimmed.chars().count() <= 50 {
        return trimmed.to_string();
    }

    let mut summary = trimmed.chars().take(50).collect::<String>();
    summary.push_str("...");
    summary
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
    use tokio::time::{Duration, sleep};

    #[test]
    fn summary_truncates_long_prompts() {
        let prompt = "a".repeat(80);
        assert_eq!(summarize(&prompt), format!("{}...", "a".repeat(50)));
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
    }

    #[test]
    fn codex_live_normalizer_preserves_plain_output_and_partial_last_line() {
        let mut normalizer = CodexLiveOutputNormalizer::default();
        assert_eq!(normalizer.push("plain out"), "");
        assert_eq!(normalizer.push("put\n"), "plain output\n");
        assert_eq!(normalizer.push("last line"), "");
        assert_eq!(normalizer.finish(), "last line\n");
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

    #[tokio::test(flavor = "current_thread")]
    async fn claude_gateway_model_calls_direct_ai_api() {
        assert_gateway_model_calls_direct_ai_api(
            Provider::Claude,
            "cld:claude-haiku-4-5-20251001",
            "/claude/v1/messages",
        )
        .await;
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

        let session = state
            .start_agent_session(
                provider,
                project.display().to_string(),
                "reply ok",
                None,
                Some(model.to_string()),
                None,
                None,
                None,
                Some(DirectAiRuntimeConfig {
                    base_url: format!("http://{gateway_addr}"),
                    api_key: "test-key".to_string(),
                    max_tokens: Some(32),
                }),
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
            .find(|message| message.role == MessageRole::Assistant)
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
        );
        assert!(
            claude
                .windows(2)
                .any(|args| args == ["--resume", session_id])
        );
        assert_eq!(claude.first().map(String::as_str), Some("--print"));

        let codex = default_agent_args_with_resume(
            Provider::Codex,
            "continue",
            Some("plan"),
            None,
            None,
            None,
            Some(session_id),
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
    fn codex_bypass_uses_bypass_flag() {
        let args = default_agent_args_with(Provider::Codex, "hi", Some("bypass"), None, None, None);
        eprintln!("codex bypass args: {:?}", args);
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    }

    #[test]
    fn codex_user_model_overrides_effort_default() {
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
        // Proxy models also reroute codex to the user's `aiproxy` provider.
        assert!(args.iter().any(|a| a == "-c"));
        assert!(args.iter().any(|a| a == "model_provider=aiproxy"));
        // Reasoning effort must not silently replace the selected model.
        assert!(!args.contains(&"gpt-5".to_string()));
    }

    #[test]
    fn codex_minimax_model_uses_minimax_provider_and_bare_model_id() {
        let args = default_agent_args_with(
            Provider::Codex,
            "hi",
            None,
            Some("medium"),
            None,
            Some("min:MiniMax-M3"),
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-c", "model_provider=minimax"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", "MiniMax-M3"]),
            "args: {args:?}"
        );
        assert!(!args.iter().any(|arg| arg == "model_provider=aiproxy"));
        assert!(!args.iter().any(|arg| arg == "min:MiniMax-M3"));
    }

    #[test]
    fn codex_effort_uses_reasoning_config_without_forcing_an_old_model() {
        let args = default_agent_args_with(Provider::Codex, "hi", None, Some("medium"), None, None);
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
    fn codex_real_model_does_not_add_provider_override() {
        // Real codex/openai model ids do not need the aiproxy provider override.
        let args =
            default_agent_args_with(Provider::Codex, "hi", None, None, None, Some("gpt-5-codex"));
        eprintln!("codex real model args: {:?}", args);
        assert!(args.contains(&"gpt-5-codex".to_string()));
        assert!(!args.iter().any(|a| a == "model_provider=aiproxy"));
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
        assert!(should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("cld:claude-haiku-4-5-20251001")
        ));
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Codex,
            Some("cod:gpt-5.5")
        ));

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
        assert!(args.iter().any(|a| a == "model_provider=aiproxy"));
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

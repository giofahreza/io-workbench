use std::{
    collections::{HashMap, VecDeque},
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

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

type HmacSha1 = Hmac<Sha1>;

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
        let auth_required = env_bool("IO_WORKBENCH_AUTH_REQUIRED", false);
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

    pub async fn start_agent_session(
        &self,
        provider: Provider,
        project_path: impl Into<String>,
        prompt: impl Into<String>,
        session_id: Option<String>,
        model: Option<String>,
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

        let session = self
            .sessions
            .create_or_update(
                provider,
                resolved_project_path.display().to_string(),
                session_id,
                model.clone(),
            )
            .await?;
        if !prompt.trim().is_empty() {
            self.sessions
                .append_message(&session.id, MessageRole::User, prompt.clone())
                .await?;
        }

        self.agents
            .start(AgentStartContext {
                provider,
                session_id: session.id.clone(),
                project_path: resolved_project_path,
                prompt,
                model,
                sessions: self.sessions.clone(),
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
        })
    }

    pub async fn create_or_update(
        &self,
        provider: Provider,
        project_path: impl Into<String>,
        session_id: Option<String>,
        model: Option<String>,
    ) -> Result<SessionSummary> {
        let id = session_id.unwrap_or_else(|| new_id("session"));
        let now = Utc::now();
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .entry(id.clone())
            .or_insert_with(|| SessionSummary {
                id: id.clone(),
                provider,
                project_path: project_path.into(),
                title: "New Session".to_string(),
                message_count: 0,
                last_activity: now,
                active: true,
                model: model.clone(),
            });

        session.provider = provider;
        if let Some(model) = model {
            session.model = Some(model);
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
        let content = content.into();
        let message = ChatMessage {
            id: new_id("msg"),
            role,
            content,
            timestamp: Utc::now(),
            metadata: Value::Null,
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
        Ok(sessions)
    }

    pub fn messages(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        Ok(self.storage.list_messages(session_id)?)
    }

    pub async fn get(&self, session_id: &str) -> Result<SessionSummary> {
        if let Some(session) = self.sessions.read().await.get(session_id).cloned() {
            return Ok(session);
        }

        self.storage
            .get_session(session_id)?
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))
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
    sessions: SessionManager,
    hub: WsHub,
}

struct AgentCommandSpec {
    command: String,
    args: Vec<String>,
    stdin_prompt: bool,
}

enum AgentProcessEvent {
    Output(String),
    Failed(String),
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
        let command = resolve_agent_command(
            context.provider,
            &context.prompt,
            &context.session_id,
            context.model.as_deref(),
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
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
            spawn_agent_output_reader(output_tx.clone(), stdout);
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_agent_output_reader(output_tx.clone(), stderr);
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
            loop {
                tokio::select! {
                    Some(event) = output_rx.recv() => {
                        match event {
                            AgentProcessEvent::Output(data) => {
                                append_bounded(&mut output, &data, manager.max_output_bytes);
                                manager.publish(&context.hub, &key, WsServerEvent::Output {
                                    provider: context.provider,
                                    session_id: context.session_id.clone(),
                                    content: data,
                                    done: false,
                                }).await;
                            }
                            AgentProcessEvent::Failed(message) => {
                                manager.publish(&context.hub, &key, WsServerEvent::Error {
                                    message: "agent output stream failed".to_string(),
                                    details: Some(message),
                                }).await;
                            }
                        }
                    }
                    status = child.wait() => {
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
        if let Some(output) = assistant_output
            .map(|output| output.trim().to_string())
            .filter(|output| !output.is_empty())
        {
            if let Err(error) = context
                .sessions
                .append_message(&context.session_id, MessageRole::Assistant, output)
                .await
            {
                warn!(error = %error, session_id = %context.session_id, "failed to persist assistant message");
            }
        }

        if let Err(error) = context
            .sessions
            .set_active(&context.session_id, false)
            .await
        {
            warn!(error = %error, session_id = %context.session_id, "failed to mark session inactive");
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

fn resolve_agent_command(
    provider: Provider,
    prompt: &str,
    session_id: &str,
    model: Option<&str>,
) -> Result<AgentCommandSpec> {
    let provider_prefix = format!("IO_WORKBENCH_{}_", provider.as_str().to_ascii_uppercase());
    let command = env::var(format!("{provider_prefix}COMMAND"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("IO_WORKBENCH_AGENT_COMMAND")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| default_agent_command(provider).to_string());

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
        default_agent_args(provider, prompt)
    };

    let stdin_prompt = env_bool(&format!("{provider_prefix}STDIN"), false)
        || env_bool("IO_WORKBENCH_AGENT_STDIN", false);

    Ok(AgentCommandSpec {
        command,
        args,
        stdin_prompt,
    })
}

fn default_agent_command(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::Cursor => "cursor-agent",
        Provider::Gemini => "gemini",
    }
}

fn default_agent_args(provider: Provider, prompt: &str) -> Vec<String> {
    match provider {
        Provider::Claude => vec!["--print".to_string(), prompt.to_string()],
        Provider::Codex => vec!["exec".to_string(), prompt.to_string()],
        Provider::Cursor => vec!["-p".to_string(), prompt.to_string()],
        Provider::Gemini => vec!["--prompt".to_string(), prompt.to_string()],
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

fn spawn_agent_output_reader<R>(tx: mpsc::Sender<AgentProcessEvent>, reader: R)
where
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
                        .send(AgentProcessEvent::Output(
                            String::from_utf8_lossy(&buffer[..read]).into_owned(),
                        ))
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
    use tokio::time::{Duration, sleep};

    #[test]
    fn summary_truncates_long_prompts() {
        let prompt = "a".repeat(80);
        assert_eq!(summarize(&prompt), format!("{}...", "a".repeat(50)));
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
        let _ = std::fs::remove_dir_all(root);

        assert!(saw_output);
    }
}

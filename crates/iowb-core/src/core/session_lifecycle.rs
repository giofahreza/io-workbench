fn resolve_retry_context_rollover_source(
    storage: &Storage,
    session_id: &str,
    failed_message_id: &str,
) -> Result<RetryContextRolloverSource> {
    let failed_message = match storage.message_by_id(session_id, failed_message_id)? {
        Some(message) if message.role == MessageRole::User => Some(message),
        Some(_) => {
            return Err(CoreError::InvalidInput(
                "failed message was not a user message".to_string(),
            ));
        }
        None => None,
    };
    let original_run = storage.durable_chat_run_for_user_message(session_id, failed_message_id)?;
    let recovery_run = if let Some(run) = original_run
        .as_ref()
        .filter(|run| run.status == "failed")
        .cloned()
    {
        run
    } else {
        let latest_rollover = storage
            .latest_context_rollover(session_id)?
            .filter(|rollover| {
                rollover.kind == CONTEXT_ROLLOVER_KIND_RETRY_FAILED_TURN
                    && rollover.state == "failed"
                    && rollover.failed_message_id == failed_message_id
            })
            .ok_or_else(|| {
                CoreError::Conflict(
                    "the selected user message does not belong to a failed turn".to_string(),
                )
            })?;
        storage
            .get_durable_chat_run(&latest_rollover.retry_run_id)?
            .filter(|run| run.status == "failed")
            .ok_or_else(|| {
                CoreError::Conflict(
                    "the selected clean-context retry is no longer failed".to_string(),
                )
            })?
    };
    let latest_run = storage.latest_durable_chat_run_for_session(session_id)?;
    if latest_run.as_ref().map(|run| run.id.as_str()) != Some(recovery_run.id.as_str()) {
        return Err(CoreError::Conflict(
            "only the latest failed turn can be retried with a clean context".to_string(),
        ));
    }
    let failed_prompt = failed_message
        .as_ref()
        .map(|message| message.content.clone())
        .or_else(|| original_run.as_ref().map(|run| run.prompt.clone()))
        .unwrap_or_else(|| recovery_run.prompt.clone());
    Ok(RetryContextRolloverSource {
        recovery_run,
        failed_prompt,
    })
}

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

    pub fn server_id(&self) -> String {
        if let Some(configured) = env::var("IO_WORKBENCH_SERVER_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return configured;
        }

        self.persisted_server_id()
            .unwrap_or_else(|| self.fallback_server_id())
    }

    fn persisted_server_id(&self) -> Option<String> {
        let path = self.config_dir.join("server-id");
        if let Some(existing) = read_server_id(&path) {
            return Some(existing);
        }
        std::fs::create_dir_all(&self.config_dir).ok()?;

        let generated = format!("iowb_{}", Uuid::new_v4().simple());
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return read_server_id(&path);
            }
            Err(_) => return None,
        };
        if writeln!(file, "{generated}").is_err() || file.sync_all().is_err() {
            drop(file);
            let _ = std::fs::remove_file(path);
            return None;
        }
        Some(generated)
    }

    fn fallback_server_id(&self) -> String {
        // Read-only configurations cannot persist an instance ID. Keep a
        // stable opaque per-machine fallback so notification routing remains
        // usable without making default paths collide across machines.
        let machine_id = std::fs::read_to_string("/etc/machine-id").unwrap_or_default();
        let seed = format!(
            "{}\n{}\n{}",
            self.config_dir.display(),
            self.database_path.display(),
            machine_id.trim(),
        );
        let digest = Sha256::digest(seed.as_bytes());
        let suffix = digest[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!("iowb_{suffix}")
    }

    pub fn server_status(&self, version: &str) -> ServerStatusResponse {
        ServerStatusResponse {
            product: PRODUCT_NAME.to_string(),
            version: version.to_string(),
            server_id: self.server_id(),
            config_dir: self.config_dir.display().to_string(),
            database_path: self.database_path.display().to_string(),
            workspace_root: self.workspace_root.display().to_string(),
            auth_required: self.auth_required
                || self.local_token.is_some()
                || self.otp_secret.is_some(),
        }
    }
}

fn read_server_id(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

use super::*;
use std::time::Duration;

#[derive(Clone)]
pub struct Client {
    pub manifest: Manifest,
    http: reqwest::Client,
}

impl Client {
    pub async fn discover(session_id: &str, cwd: &Path) -> Result<Option<Self>> {
        validate_id(session_id)?;
        let path = directory().join(session_id).join("connection.json");
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.len() > 8192 {
            return Err(CoreError::Conflict(
                "Invalid Claude bridge registry entry".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
                return Err(CoreError::Conflict(
                    "Claude bridge credentials must belong to this OS user and be private".into(),
                ));
            }
        }
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(path)?)?;
        let url = reqwest::Url::parse(&manifest.url)
            .map_err(|_| CoreError::Conflict("Invalid Claude bridge address".into()))?;
        if manifest.protocol != 1
            || manifest.session_id != session_id
            || std::fs::canonicalize(cwd)? != manifest.cwd
            || url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || url.port().is_none()
            || url.path() != "/"
            || url.query().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(CoreError::Conflict(
                "Claude bridge session, project or protocol does not match".into(),
            ));
        }
        let client = Self {
            manifest,
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(3))
                .build()
                .map_err(|e| CoreError::InvalidInput(e.to_string()))?,
        };
        match client.read(None).await {
            Ok(value) if value["connected"] == true => Ok(Some(client)),
            _ if !process_alive(client.manifest.pid) => Ok(None),
            _ => Err(CoreError::Conflict("The Claude bridge is not responding. Inspect the terminal; no second Claude process was started.".into())),
        }
    }

    pub async fn call(&self, route: &str, body: &impl Serialize) -> Result<Value> {
        let response = self
            .http
            .post(format!("{}{route}", self.manifest.url))
            .bearer_auth(&self.manifest.token)
            .json(body)
            .send()
            .await
            .map_err(|e| {
                CoreError::Conflict(format!(
                    "Claude bridge delivery could not be confirmed: {e}"
                ))
            })?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| CoreError::Conflict(e.to_string()))?;
        if !status.is_success() {
            return Err(CoreError::Conflict(text));
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn read(&self, id: Option<&str>) -> Result<Value> {
        self.call("/read", &serde_json::json!({"sessionId":self.manifest.session_id,"instance":self.manifest.instance,"id":id})).await
    }

    pub async fn submit(&self, request: &Submission) -> Result<Submission> {
        let response = self
            .http
            .post(format!("{}/submit", self.manifest.url))
            .bearer_auth(&self.manifest.token)
            .json(request)
            .send()
            .await
            .map_err(|e| {
                CoreError::Conflict(format!(
                    "Claude bridge delivery could not be confirmed: {e}"
                ))
            })?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| CoreError::Conflict(e.to_string()))?;
        if status == reqwest::StatusCode::CONFLICT {
            return Ok(Submission {
                id: request.id.clone(),
                prompt: request.prompt.clone(),
                status: "rejected".into(),
                error: Some(text),
                ..Default::default()
            });
        }
        if !status.is_success() {
            return Err(CoreError::Conflict(text));
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn receipt(&self, id: &str) -> Result<Submission> {
        Ok(serde_json::from_value(self.read(Some(id)).await?)?)
    }
    pub async fn abort(&self, id: &str) -> Result<()> {
        self.call("/abort", &serde_json::json!({"id":id})).await?;
        Ok(())
    }
}

fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Plain Claude terminals have no shared-runtime lock. Conservatively refuse
/// a separate resume while an unbridged Claude process owns this project.
pub fn private_owner(cwd: &Path, session_id: Option<&str>) -> bool {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return true;
        };
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if pid == std::process::id() {
                continue;
            }
            let root = entry.path();
            let Ok(bytes) = std::fs::read(root.join("cmdline")) else {
                continue;
            };
            let args = bytes
                .split(|b| *b == 0)
                .filter_map(|p| std::str::from_utf8(p).ok())
                .collect::<Vec<_>>();
            let Some(command) = args.first() else {
                continue;
            };
            let is_claude = Path::new(command)
                .file_name()
                .is_some_and(|name| name == "claude")
                || command.contains("/claude/versions/")
                || args
                    .iter()
                    .take(3)
                    .any(|arg| arg.ends_with("/claude-code/cli.js"));
            if !is_claude || args.iter().any(|a| matches!(*a, "--version" | "--help")) {
                continue;
            }
            let explicit = args
                .windows(2)
                .find(|pair| matches!(pair[0], "--resume" | "-r" | "--session-id"))
                .map(|p| p[1])
                .or_else(|| {
                    args.iter().find_map(|arg| {
                        arg.strip_prefix("--resume=")
                            .or_else(|| arg.strip_prefix("--session-id="))
                    })
                });
            if session_id.is_some() && explicit == session_id {
                return true;
            }
            if std::fs::read_link(root.join("cwd")).ok().as_deref()
                != std::fs::canonicalize(cwd).ok().as_deref()
            {
                continue;
            }
            if session_id.is_none() || explicit.is_none_or(|id| uuid::Uuid::parse_str(id).is_err())
            {
                return true;
            }
        }
        false
    }
    // Without reliable process/cwd inspection, an imported transcript cannot
    // safely be resumed in a second runtime. A live bridge still works.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = cwd;
        session_id.is_some()
    }
}

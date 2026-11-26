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

    pub fn find_by_ref(&self, project_ref: &str) -> Result<ProjectSummary> {
        if let Some(project) = self.storage.find_project_by_id(project_ref)? {
            return Ok(project);
        }
        if let Some(project) = self.storage.find_project_by_path(project_ref)? {
            return Ok(project);
        }
        self.find_by_name(project_ref)
    }

    pub fn delete_by_ref(&self, project_ref: &str) -> Result<bool> {
        let project = self.find_by_ref(project_ref)?;
        Ok(self.storage.delete_project_by_id(&project.id)?)
    }
}

#[derive(Clone)]
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
    storage: Option<Storage>,
}

impl WsHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(iowb_protocol::WS_EVENT_CHANNEL_CAPACITY);
        Self { tx, storage: None }
    }

    pub fn with_storage(storage: Storage) -> Self {
        Self {
            storage: Some(storage),
            ..Self::new()
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WsServerEvent> {
        self.tx.subscribe()
    }

    /// Persist session errors before delivery, including when no client is
    /// connected. Connection-only warnings have no session and stay transient.
    pub fn prepare_event(&self, mut event: WsServerEvent) -> WsServerEvent {
        let (message, session_id, history_message, metadata) = match &mut event {
            WsServerEvent::Error {
                message,
                session_id: Some(session_id),
                history_message,
                ..
            } => (
                message,
                session_id,
                history_message,
                serde_json::json!({
                    "kind": "chat_error", "code": "chat_runtime_error", "phase": "runtime", "status": "failed"
                }),
            ),
            WsServerEvent::ChatSetupRequired {
                message,
                session_id,
                history_message,
                code,
                action,
                docs_url,
                request_id,
                ..
            } => (
                message,
                session_id,
                history_message,
                serde_json::json!({
                    "kind": "chat_error", "code": code, "phase": "runtime", "status": "failed",
                    "action": action, "docsUrl": docs_url, "requestId": request_id,
                }),
            ),
            _ => return event,
        };
        if history_message.is_none()
            && let Some(storage) = &self.storage
        {
            match storage.get_session_summary(session_id) {
                Ok(Some(session)) => {
                    let entry = ChatMessage {
                        id: new_id("chat-error"),
                        role: MessageRole::System,
                        content: message.chars().take(4096).collect(),
                        timestamp: Utc::now(),
                        metadata,
                    };
                    match storage.persist_chat_error(&session, &entry, None) {
                        Ok(()) => *history_message = Some(entry),
                        Err(error) => warn!(%session_id, %error, "could not persist chat error"),
                    }
                }
                Ok(None) => {}
                Err(error) => warn!(%session_id, %error, "could not load failed chat"),
            }
        }
        event
    }

    pub fn publish(&self, event: WsServerEvent) {
        let event = self.prepare_event(event);
        let _ = self.tx.send(event);
    }
}

impl Default for WsHub {
    fn default() -> Self {
        Self::new()
    }
}

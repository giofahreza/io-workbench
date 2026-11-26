use std::cell::Cell;

use chrono::Utc;
use iowb_protocol::{
    ChatMessage, ForkSessionResponse, MessageRole, MessagesResponse, ProjectSessionsPage,
    ProjectSummary, SessionRuntimeStatus, SessionSnapshotResponse, SessionSummary, WsClientCommand,
    WsServerEvent,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    api::{
        Api, ApiError, DEFAULT_SERVER_URL, MESSAGES_PAGE_SIZE, PinnedSession, normalize_base_url,
    },
    config::{self, Config},
    headers::{self, CLOUDFLARE_ACCESS, Header},
    input::Input,
    switcher::{Load, ProjectNode, Row, Switcher},
    ws,
};

const NOTICE_TICKS: u32 = 20;
/// Lines the chat moves for one notch of the mouse wheel.
const WHEEL_LINES: usize = 3;

/// Everything that can change the screen: terminal input, socket events and the
/// results of requests that were started in the background.
// Each message is moved through a channel once, so boxing the large variants would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Msg {
    Key(KeyEvent),
    Paste(String),
    /// One notch of the mouse wheel.
    Wheel {
        up: bool,
    },
    Tick,
    Ws(WsServerEvent),
    WsConnection {
        connected: bool,
        detail: String,
    },
    Resumed(Result<bool, ApiError>),
    SignedIn(Result<Option<String>, ApiError>),
    Pinned(Result<Vec<PinnedSession>, ApiError>),
    Projects(Result<Vec<ProjectSummary>, ApiError>),
    ProjectSessions {
        project_id: String,
        offset: usize,
        result: Result<ProjectSessionsPage, ApiError>,
    },
    Snapshot {
        session_id: String,
        reset: bool,
        result: Result<SessionSnapshotResponse, ApiError>,
    },
    Older {
        session_id: String,
        result: Result<MessagesResponse, ApiError>,
    },
    Forked(Result<ForkSessionResponse, ApiError>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Login,
    Chat,
}

pub struct LoginForm {
    pub server: String,
    pub username: String,
    pub password: String,
    pub focus: usize,
    /// The checked custom headers that are sent; the editor works on a copy.
    pub headers: Vec<Header>,
    pub editor: Option<HeadersEditor>,
    pub busy: bool,
    pub status: Option<String>,
    pub error: Option<String>,
}

impl LoginForm {
    pub const PASSWORD: usize = 2;
    /// A row that opens the headers editor rather than a text field.
    pub const HEADERS: usize = 3;
    pub const FIELDS: usize = 4;

    /// The text field under the focus, if it is one.
    fn field(&mut self) -> Option<&mut String> {
        match self.focus {
            0 => Some(&mut self.server),
            1 => Some(&mut self.username),
            Self::PASSWORD => Some(&mut self.password),
            _ => None,
        }
    }
}

/// The custom headers being edited: one name and value per row, one cell focused.
pub struct HeadersEditor {
    pub rows: Vec<Header>,
    pub row: usize,
    pub on_value: bool,
    pub error: Option<String>,
}

impl HeadersEditor {
    fn new(headers: &[Header]) -> Self {
        let mut rows = headers.to_vec();
        if rows.is_empty() {
            rows.push(Header::blank());
        }
        Self {
            rows,
            row: 0,
            on_value: false,
            error: None,
        }
    }

    fn cell(&mut self) -> &mut String {
        let row = &mut self.rows[self.row];
        if self.on_value {
            &mut row.value
        } else {
            &mut row.name
        }
    }

    fn step_row(&mut self, delta: isize) {
        let last = self.rows.len() as isize - 1;
        self.row = (self.row as isize + delta).clamp(0, last) as usize;
    }

    fn next_cell(&mut self) {
        if self.on_value {
            self.row = (self.row + 1) % self.rows.len();
        }
        self.on_value = !self.on_value;
    }

    fn previous_cell(&mut self) {
        if !self.on_value {
            self.row = (self.row + self.rows.len() - 1) % self.rows.len();
        }
        self.on_value = !self.on_value;
    }

    fn add_row(&mut self) {
        self.rows.push(Header::blank());
        self.row = self.rows.len() - 1;
        self.on_value = false;
    }

    fn remove_row(&mut self) {
        self.rows.remove(self.row);
        if self.rows.is_empty() {
            self.rows.push(Header::blank());
        }
        self.row = self.row.min(self.rows.len() - 1);
    }

    /// Adds the Cloudflare Access service-token rows that are missing, as the mobile
    /// app's preset does, and moves to the first value still to be filled in.
    fn add_cloudflare_access(&mut self) {
        if self
            .rows
            .iter()
            .all(|row| row.name.trim().is_empty() && row.value.trim().is_empty())
        {
            self.rows.clear();
        }
        let mut positions = Vec::new();
        for name in CLOUDFLARE_ACCESS {
            let existing = self
                .rows
                .iter()
                .position(|row| row.name.trim().eq_ignore_ascii_case(name));
            positions.push(existing.unwrap_or_else(|| {
                self.rows.push(Header {
                    name: name.to_string(),
                    value: String::new(),
                });
                self.rows.len() - 1
            }));
        }
        self.row = positions
            .iter()
            .copied()
            .find(|&position| self.rows[position].value.trim().is_empty())
            .unwrap_or(positions[0]);
        self.on_value = true;
    }
}

#[derive(Default)]
pub struct Chat {
    pub current_id: Option<String>,
    pub session: Option<SessionSummary>,
    pub messages: Vec<ChatMessage>,
    pub total_count: usize,
    pub loading: bool,
    /// Assistant text that is streaming in and not stored yet.
    pub live: String,
    pub running: bool,
    pub request_id: Option<String>,
    pub pending_prompt: Option<String>,
    /// The server has said the run began; until then the prompt is still being sent.
    pub started: bool,
    /// An abort was asked for and the run has not ended yet.
    pub stopping: bool,
    /// Lines scrolled up from the newest output; 0 follows the latest.
    pub scroll: usize,
    /// Filled in while drawing so key handling can clamp scrolling.
    pub max_scroll: Cell<usize>,
    pub view_height: Cell<usize>,
    /// Width of the prompt box, for moving the cursor between wrapped rows.
    pub input_width: Cell<usize>,
    pub input: Input,
    /// Tool calls and their output in full instead of a few lines each.
    pub show_tools: bool,
}

impl Chat {
    fn opening(session_id: String) -> Self {
        Self {
            current_id: Some(session_id),
            loading: true,
            ..Default::default()
        }
    }

    pub fn older_count(&self) -> usize {
        self.total_count.saturating_sub(self.messages.len())
    }

    /// What a reply that is still going is doing, in the words the web and mobile
    /// apps use for it.
    pub fn phase(&self) -> Option<&'static str> {
        if !self.running {
            return None;
        }
        Some(if self.stopping {
            "Stopping"
        } else if !self.started {
            "Sending"
        } else {
            "Processing"
        })
    }
}

/// A prompt that can be rewound to.
pub struct RewindPrompt {
    pub message_id: String,
    pub text: String,
}

/// The list of earlier prompts shown by a second Esc.
pub struct Rewind {
    pub prompts: Vec<RewindPrompt>,
    pub cursor: usize,
    /// The new chat is being made.
    pub busy: bool,
}

/// What a second Esc will do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscIntent {
    Clear,
    Rewind,
}

/// A second Esc has to come within this many ticks of the first.
const ESC_TICKS: usize = 8;

pub struct App {
    pub screen: Screen,
    pub login: LoginForm,
    pub chat: Chat,
    pub switcher: Option<Switcher>,
    pub rewind: Option<Rewind>,
    pub notice: Option<(String, u32)>,
    pub ws_connected: bool,
    pub ws_detail: String,
    pub tick: usize,
    pub quit: bool,
    api: Api,
    config: Config,
    tx: UnboundedSender<Msg>,
    pub(crate) ws: Option<UnboundedSender<WsClientCommand>>,
    ws_dropped: bool,
    pending_server: Option<String>,
    esc_armed: Option<(EscIntent, usize)>,
    save_config: bool,
}

/// Edits `buffer` for printable keys, Backspace, Ctrl+U (clear line) and Ctrl+W
/// (delete word). Returns false for keys it does not handle.
fn edit(buffer: &mut String, key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char('u') if ctrl => buffer.clear(),
        KeyCode::Char('w') if ctrl => {
            while buffer.ends_with(char::is_whitespace) {
                buffer.pop();
            }
            while buffer.chars().last().is_some_and(|ch| !ch.is_whitespace()) {
                buffer.pop();
            }
        }
        KeyCode::Char(ch) if !ctrl && !alt => buffer.push(ch),
        KeyCode::Backspace => {
            buffer.pop();
        }
        _ => return false,
    }
    true
}

impl App {
    pub fn new(
        tx: UnboundedSender<Msg>,
        config: Config,
        server_override: Option<String>,
        save_config: bool,
    ) -> Self {
        let server = server_override
            .as_deref()
            .map(normalize_base_url)
            .or_else(|| config.server_url.clone())
            .unwrap_or_else(|| DEFAULT_SERVER_URL.to_string());
        // A saved token and its headers belong to the server they were set up for;
        // sending them to another address would hand that server the secrets.
        let saved_here = config.server_url.as_deref() == Some(server.as_str());
        let token = config.token.clone().filter(|_| saved_here);
        let headers = if saved_here {
            config.headers.clone()
        } else {
            Vec::new()
        };
        Self {
            screen: Screen::Login,
            login: LoginForm {
                server: server.clone(),
                username: String::new(),
                password: String::new(),
                focus: 1,
                headers: headers.clone(),
                editor: None,
                busy: false,
                status: None,
                error: None,
            },
            chat: Chat::default(),
            switcher: None,
            rewind: None,
            notice: None,
            ws_connected: false,
            ws_detail: String::new(),
            tick: 0,
            quit: false,
            api: Api::new(&server, token, headers),
            config,
            tx,
            ws: None,
            ws_dropped: false,
            pending_server: None,
            esc_armed: None,
            save_config,
        }
    }

    /// Checks a saved sign-in; without one the login form is simply shown.
    pub fn start(&mut self) {
        if self.api.token().is_none() {
            return;
        }
        self.login.busy = true;
        self.login.status = Some("Checking your saved sign-in…".into());
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let result = api
                .auth_status()
                .await
                .map(|status| status.is_authenticated || !status.enabled);
            let _ = tx.send(Msg::Resumed(result));
        });
    }

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Key(key) => self.handle_key(key),
            Msg::Paste(text) => self.handle_paste(&text),
            Msg::Wheel { up } => self.handle_wheel(up),
            Msg::Tick => {
                self.tick = self.tick.wrapping_add(1);
                if let Some((_, ticks)) = &mut self.notice {
                    *ticks = ticks.saturating_sub(1);
                    if *ticks == 0 {
                        self.notice = None;
                    }
                }
            }
            Msg::Ws(event) => self.on_ws_event(event),
            Msg::WsConnection { connected, detail } => self.on_ws_connection(connected, detail),
            Msg::Resumed(result) => {
                self.login.busy = false;
                self.login.status = None;
                match result {
                    Ok(true) => self.enter_chat(),
                    Ok(false) => {
                        self.login.error = Some("Your saved sign-in expired. Sign in again.".into())
                    }
                    Err(error) => self.login.error = Some(error.message),
                }
            }
            Msg::SignedIn(result) => self.on_signed_in(result),
            Msg::Pinned(result) => {
                if let Some(switcher) = &mut self.switcher {
                    switcher.pinned = match result {
                        Ok(pinned) => Load::Ready(pinned),
                        Err(error) if error.unauthorized() => return self.session_expired(),
                        Err(error) => Load::Failed(error.message),
                    };
                    switcher.clamp();
                }
            }
            Msg::Projects(result) => self.on_projects(result),
            Msg::ProjectSessions {
                project_id,
                offset,
                result,
            } => {
                if let Err(error) = &result
                    && error.unauthorized()
                {
                    return self.session_expired();
                }
                if let Some(node) = self
                    .switcher
                    .as_mut()
                    .and_then(Switcher::nodes_mut)
                    .and_then(|nodes| nodes.iter_mut().find(|node| node.project.id == project_id))
                {
                    match result {
                        Ok(page) => node.absorb(offset, page),
                        Err(error) => {
                            node.loading = false;
                            node.error = Some(error.message);
                        }
                    }
                }
                if let Some(switcher) = &mut self.switcher {
                    switcher.clamp();
                }
            }
            Msg::Snapshot {
                session_id,
                reset,
                result,
            } => self.on_snapshot(session_id, reset, result),
            Msg::Older { session_id, result } => self.on_older(session_id, result),
            Msg::Forked(result) => self.on_forked(result),
        }
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), NOTICE_TICKS));
    }

    fn persist(&mut self) {
        if self.save_config
            && let Err(error) = config::save(&self.config)
        {
            self.notify(format!("could not save settings: {error:#}"));
        }
    }

    fn fail(&mut self, error: ApiError) {
        if error.unauthorized() {
            self.session_expired();
        } else {
            self.notify(error.message);
        }
    }

    fn session_expired(&mut self) {
        self.sign_out("Your session expired. Sign in again.", true);
    }

    fn sign_out(&mut self, message: &str, is_error: bool) {
        self.config.token = None;
        self.config.last_session_id = None;
        self.persist();
        self.api = Api::new(self.api.base(), None, self.api.headers().to_vec());
        self.ws = None;
        self.ws_connected = false;
        self.ws_dropped = false;
        self.chat = Chat::default();
        self.switcher = None;
        self.rewind = None;
        self.login.server = self.api.base().to_string();
        self.login.password.clear();
        self.login.busy = false;
        self.login.focus = 1;
        let message = Some(message.to_string());
        (self.login.status, self.login.error) = if is_error {
            (None, message)
        } else {
            (message, None)
        };
        self.screen = Screen::Login;
    }

    // ---- sign in -------------------------------------------------------------

    fn submit_login(&mut self) {
        if self.login.busy {
            return;
        }
        let server = normalize_base_url(&self.login.server);
        let username = self.login.username.trim().to_string();
        let password = self.login.password.clone();
        if username.is_empty() || password.is_empty() {
            self.login.error = Some("Enter a username and a password.".into());
            return;
        }
        self.login.busy = true;
        self.login.error = None;
        self.login.status = Some("Signing in…".into());
        self.pending_server = Some(server.clone());
        let (tx, headers) = (self.tx.clone(), self.login.headers.clone());
        tokio::spawn(async move {
            let api = Api::new(&server, None, headers);
            let result = async {
                let status = api.auth_status().await?;
                if !status.enabled {
                    return Ok(None);
                }
                if status.needs_setup {
                    return Err(ApiError::local(
                        "This server has no account yet. Create the first account in the web app, then sign in here.",
                    ));
                }
                Ok(Some(api.login(&username, &password).await?.token))
            }
            .await;
            let _ = tx.send(Msg::SignedIn(result));
        });
    }

    fn on_signed_in(&mut self, result: Result<Option<String>, ApiError>) {
        self.login.busy = false;
        self.login.status = None;
        match result {
            Ok(token) => {
                let server = self
                    .pending_server
                    .take()
                    .unwrap_or_else(|| self.api.base().to_string());
                let headers = self.login.headers.clone();
                self.api = Api::new(&server, token.clone(), headers.clone());
                self.config.server_url = Some(server);
                self.config.headers = headers;
                self.config.token = token;
                self.persist();
                self.login.password.clear();
                self.enter_chat();
            }
            Err(error) => self.login.error = Some(error.message),
        }
    }

    fn enter_chat(&mut self) {
        self.screen = Screen::Chat;
        self.ws = Some(ws::spawn(
            self.api.base(),
            self.api.token().map(str::to_string),
            self.api.headers().to_vec(),
            self.tx.clone(),
        ));
        match self.config.last_session_id.clone() {
            Some(id) => self.open_session(id),
            None => self.open_switcher(),
        }
    }

    // ---- sockets -------------------------------------------------------------

    fn send_ws(&self, command: WsClientCommand) -> bool {
        self.ws.as_ref().is_some_and(|ws| ws.send(command).is_ok())
    }

    fn on_ws_connection(&mut self, connected: bool, detail: String) {
        self.ws_connected = connected;
        self.ws_detail = detail;
        if connected {
            if self.ws_dropped
                && let Some(id) = self.chat.current_id.clone()
            {
                // Catch up on anything that streamed while the socket was down.
                self.fetch_snapshot(id, false);
            }
            self.ws_dropped = false;
        } else {
            self.ws_dropped = true;
        }
    }

    fn on_ws_event(&mut self, event: WsServerEvent) {
        let current = self.chat.current_id.clone();
        match event {
            WsServerEvent::Output {
                session_id,
                content,
                done,
                ..
            } if current.as_deref() == Some(session_id.as_str()) => {
                self.chat.live.push_str(&content);
                self.chat.started = true;
                self.chat.running = !done;
                if done {
                    self.chat.stopping = false;
                    self.fetch_snapshot(session_id, false);
                }
            }
            WsServerEvent::SessionStatus {
                session_id, status, ..
            } if current.as_deref() == Some(session_id.as_str()) => {
                let active = matches!(
                    status,
                    SessionRuntimeStatus::Starting
                        | SessionRuntimeStatus::Running
                        | SessionRuntimeStatus::WaitingForInput
                );
                let was_running = std::mem::replace(&mut self.chat.running, active);
                if matches!(
                    status,
                    SessionRuntimeStatus::Running | SessionRuntimeStatus::WaitingForInput
                ) {
                    self.chat.started = true;
                }
                if !active {
                    self.chat.started = false;
                    self.chat.stopping = false;
                }
                if was_running && !active {
                    self.fetch_snapshot(session_id, false);
                }
            }
            WsServerEvent::Error {
                message,
                details,
                session_id,
                ..
            } => {
                let ours = session_id.is_none() || session_id == current;
                if ours {
                    self.chat.running = false;
                    self.chat.stopping = false;
                    self.notify(match details {
                        // Servers often repeat the message at the start of the details.
                        Some(details) if details.starts_with(&message) => details,
                        Some(details) if !details.is_empty() => format!("{message}: {details}"),
                        _ => message,
                    });
                }
            }
            WsServerEvent::ChatSetupRequired {
                session_id,
                request_id,
                message,
                docs_url,
                ..
            } if current.as_deref() == Some(session_id.as_str())
                && request_id.is_some()
                && request_id == self.chat.request_id =>
            {
                self.chat.running = false;
                self.chat.stopping = false;
                self.chat.request_id = None;
                if self.chat.input.text().is_empty()
                    && let Some(prompt) = self.chat.pending_prompt.take()
                {
                    self.chat.input.set(prompt);
                }
                self.notify(format!("{message} {}", docs_url.unwrap_or_default()));
            }
            WsServerEvent::ChatRecoveryRequired { session_id, .. }
                if current.as_deref() == Some(session_id.as_str()) =>
            {
                self.notify("This chat needs its context recovered; use the web or mobile app.");
            }
            _ => {}
        }
    }

    // ---- background requests ---------------------------------------------------

    fn fetch_snapshot(&self, session_id: String, reset: bool) {
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let result = api.snapshot(&session_id).await;
            let _ = tx.send(Msg::Snapshot {
                session_id,
                reset,
                result,
            });
        });
    }

    fn fetch_sessions(&self, project_id: String, offset: usize) {
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let result = api.project_sessions(&project_id, offset).await;
            let _ = tx.send(Msg::ProjectSessions {
                project_id,
                offset,
                result,
            });
        });
    }

    fn on_snapshot(
        &mut self,
        session_id: String,
        reset: bool,
        result: Result<SessionSnapshotResponse, ApiError>,
    ) {
        if self.chat.current_id.as_deref() != Some(session_id.as_str()) {
            return;
        }
        self.chat.loading = false;
        match result {
            Ok(snapshot) => {
                self.chat.running = snapshot.session.active;
                // A run that was already going when the chat was opened is past sending.
                self.chat.started = snapshot.session.active;
                if !snapshot.session.active {
                    self.chat.live.clear();
                    self.chat.stopping = false;
                }
                self.chat.total_count = snapshot.total_count;
                self.chat.messages = snapshot.messages;
                self.chat.session = Some(snapshot.session);
                if reset {
                    self.chat.scroll = 0;
                }
            }
            Err(error) => {
                let unopened = self.chat.session.is_none();
                if unopened {
                    // A remembered chat that no longer opens must not strand the user.
                    self.chat = Chat::default();
                    self.config.last_session_id = None;
                    self.persist();
                }
                let unauthorized = error.unauthorized();
                self.fail(error);
                if unopened && !unauthorized {
                    self.open_switcher();
                }
            }
        }
    }

    fn load_older(&mut self) {
        let Some(id) = self.chat.current_id.clone() else {
            return;
        };
        let older = self.chat.older_count();
        if older == 0 || self.chat.loading {
            self.notify("No older messages.");
            return;
        }
        self.chat.loading = true;
        let limit = older.min(MESSAGES_PAGE_SIZE);
        let start = older - limit;
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let result = api.messages(&id, start, limit).await;
            let _ = tx.send(Msg::Older {
                session_id: id,
                result,
            });
        });
    }

    fn on_older(&mut self, session_id: String, result: Result<MessagesResponse, ApiError>) {
        if self.chat.current_id.as_deref() != Some(session_id.as_str()) {
            return;
        }
        self.chat.loading = false;
        match result {
            Ok(page) => {
                let mut merged: Vec<ChatMessage> = page
                    .messages
                    .into_iter()
                    .filter(|message| {
                        self.chat
                            .messages
                            .iter()
                            .all(|known| known.id != message.id)
                    })
                    .collect();
                merged.append(&mut self.chat.messages);
                self.chat.messages = merged;
                self.chat.total_count = page.total_count;
            }
            Err(error) => self.fail(error),
        }
    }

    fn on_projects(&mut self, result: Result<Vec<ProjectSummary>, ApiError>) {
        let Some(switcher) = &mut self.switcher else {
            return;
        };
        let projects = match result {
            Ok(projects) => projects,
            Err(error) if error.unauthorized() => return self.session_expired(),
            Err(error) => {
                switcher.projects = Load::Failed(error.message);
                switcher.clamp();
                return;
            }
        };
        let mut nodes: Vec<ProjectNode> = projects.into_iter().map(ProjectNode::new).collect();
        // Start with the project of the open chat expanded: switching within it is the common case.
        let mut to_load = None;
        if let Some(path) = self
            .chat
            .session
            .as_ref()
            .map(|session| session.project_path.as_str())
            && let Some(node) = nodes.iter_mut().find(|node| node.project.path == path)
        {
            node.expanded = true;
            node.loading = true;
            to_load = Some(node.project.id.clone());
        }
        switcher.projects = Load::Ready(nodes);
        switcher.clamp();
        if let Some(project_id) = to_load {
            self.fetch_sessions(project_id, 0);
        }
    }

    // ---- opening and sending -----------------------------------------------------

    fn open_switcher(&mut self) {
        self.switcher = Some(Switcher::new());
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn({
            let (api, tx) = (api.clone(), tx.clone());
            async move {
                let _ = tx.send(Msg::Pinned(api.pinned_sessions().await));
            }
        });
        tokio::spawn(async move {
            let _ = tx.send(Msg::Projects(api.projects().await));
        });
    }

    fn open_session(&mut self, session_id: String) {
        self.switcher = None;
        let show_tools = self.chat.show_tools;
        self.chat = Chat::opening(session_id.clone());
        self.chat.show_tools = show_tools;
        self.config.last_session_id = Some(session_id.clone());
        self.persist();
        self.send_ws(WsClientCommand::Subscribe {
            topics: vec!["sessions".into()],
            session_ids: Vec::new(),
            chat_session_ids: Some(vec![session_id.clone()]),
            process_ids: Vec::new(),
        });
        self.fetch_snapshot(session_id, true);
    }

    fn send_prompt(&mut self) {
        let prompt = self.chat.input.text().trim().to_string();
        if prompt.is_empty() {
            return;
        }
        let Some(session) = self.chat.session.clone() else {
            self.notify("Open a chat first (Ctrl+K).");
            return;
        };
        if self.chat.running {
            self.notify("A reply is still running. Esc stops it.");
            return;
        }
        let request_id = format!("tui-{}", Utc::now().timestamp_micros());
        let command = WsClientCommand::StartSession {
            provider: session.provider,
            request_id: Some(request_id.clone()),
            runtime: session.runtime,
            project_path: session.project_path.clone(),
            prompt: prompt.clone(),
            session_id: Some(session.id.clone()),
            model: session.model.clone(),
            effort: session.effort.clone(),
            mode: session.mode.clone(),
            thinking: session.thinking,
            fast: session.fast,
        };
        if !self.send_ws(command) {
            self.notify("Not connected to the server yet.");
            return;
        }
        self.chat.request_id = Some(request_id);
        self.chat.pending_prompt = Some(prompt.clone());
        self.chat.messages.push(ChatMessage {
            id: format!("local-{}", self.chat.total_count),
            role: MessageRole::User,
            content: prompt,
            timestamp: Utc::now(),
            metadata: serde_json::Value::Null,
        });
        self.chat.total_count += 1;
        self.chat.input.clear();
        self.chat.live.clear();
        self.chat.running = true;
        self.chat.started = false;
        self.chat.stopping = false;
        self.chat.scroll = 0;
    }

    fn abort(&mut self) {
        let Some(session) = self.chat.session.clone().filter(|_| self.chat.running) else {
            self.notify("Nothing is running.");
            return;
        };
        if self.chat.stopping {
            self.notify("Already stopping…");
            return;
        }
        self.send_ws(WsClientCommand::AbortSession {
            provider: session.provider,
            session_id: session.id,
        });
        self.chat.stopping = true;
    }

    // ---- keys ------------------------------------------------------------------------

    fn handle_paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        match self.screen {
            Screen::Login if !self.login.busy => {
                let line = text.lines().next().unwrap_or("");
                match &mut self.login.editor {
                    Some(editor) => {
                        editor.cell().push_str(line);
                        editor.error = None;
                    }
                    None => {
                        if let Some(field) = self.login.field() {
                            field.push_str(line);
                        }
                    }
                }
            }
            Screen::Chat => match &mut self.switcher {
                Some(switcher) => {
                    switcher.filter.push_str(text.lines().next().unwrap_or(""));
                    switcher.clamp();
                }
                None if self.rewind.is_some() => {}
                None => self.chat.input.insert(&text),
            },
            Screen::Login => {}
        }
    }

    /// The wheel scrolls the chat when nothing is drawn over it.
    fn handle_wheel(&mut self, up: bool) {
        if self.screen != Screen::Chat || self.rewind.is_some() || self.switcher.is_some() {
            return;
        }
        if up {
            self.page_up(WHEEL_LINES);
        } else {
            self.scroll_down(WHEEL_LINES);
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('q') {
            self.quit = true;
            return;
        }
        match self.screen {
            Screen::Login => self.login_key(key),
            Screen::Chat if self.rewind.is_some() => self.rewind_key(key),
            Screen::Chat if self.switcher.is_some() => self.switcher_key(key),
            Screen::Chat => self.chat_key(key),
        }
    }

    fn login_key(&mut self, key: KeyEvent) {
        if self.login.editor.is_some() {
            return self.headers_key(key);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc || (ctrl && key.code == KeyCode::Char('c')) {
            self.quit = true;
            return;
        }
        if self.login.busy {
            return;
        }
        let fields = LoginForm::FIELDS;
        match key.code {
            KeyCode::Tab | KeyCode::Down => self.login.focus = (self.login.focus + 1) % fields,
            KeyCode::BackTab | KeyCode::Up => {
                self.login.focus = (self.login.focus + fields - 1) % fields
            }
            KeyCode::Enter if self.login.focus == LoginForm::HEADERS => {
                self.login.editor = Some(HeadersEditor::new(&self.login.headers));
            }
            KeyCode::Enter if self.login.focus < LoginForm::PASSWORD => self.login.focus += 1,
            KeyCode::Enter => self.submit_login(),
            _ => {
                if let Some(field) = self.login.field()
                    && edit(field, &key)
                {
                    self.login.error = None;
                }
            }
        }
    }

    /// Esc drops the changes; Enter keeps them once every row checks out.
    fn headers_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(editor) = self.login.editor.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.login.editor = None,
            KeyCode::Enter => match headers::validate(&editor.rows) {
                Ok(headers) => {
                    self.login.headers = headers;
                    self.login.editor = None;
                    self.login.error = None;
                }
                Err(error) => editor.error = Some(error),
            },
            KeyCode::Tab => editor.next_cell(),
            KeyCode::BackTab => editor.previous_cell(),
            KeyCode::Down => editor.step_row(1),
            KeyCode::Up => editor.step_row(-1),
            KeyCode::Char('n') if ctrl => editor.add_row(),
            KeyCode::Char('d') if ctrl => editor.remove_row(),
            KeyCode::Char('f') if ctrl => editor.add_cloudflare_access(),
            _ => {
                if edit(editor.cell(), &key) {
                    editor.error = None;
                }
            }
        }
    }

    /// The prompt box is always live: typing goes into it, and the keys that would
    /// get in the way of typing are Esc, Ctrl combinations, the paging keys and
    /// Up/Down once the cursor has no row left to move to.
    fn chat_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code != KeyCode::Esc {
            self.esc_armed = None;
        }
        let page = self.chat.view_height.get().saturating_sub(2).max(1);
        let half = (self.chat.view_height.get() / 2).max(1);
        match key.code {
            KeyCode::Esc => self.escape(),
            KeyCode::Char('u') if ctrl => self.page_up(half),
            KeyCode::Char('d') if ctrl => self.scroll_down(half),
            KeyCode::Char('k') if ctrl => self.open_switcher(),
            KeyCode::Char('l') if ctrl => self.sign_out("Signed out.", false),
            KeyCode::Char('c') if ctrl => self.interrupt(),
            KeyCode::Char('o') if ctrl => self.toggle_tools(),
            KeyCode::PageUp => self.page_up(page),
            KeyCode::PageDown => self.scroll_down(page),
            KeyCode::Home if ctrl => self.scroll_up(usize::MAX / 2),
            KeyCode::End if ctrl => self.chat.scroll = 0,
            KeyCode::Up if ctrl => self.scroll_up(1),
            KeyCode::Down if ctrl => self.scroll_down(1),
            KeyCode::Enter
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.send_prompt()
            }
            _ => self.edit_input(key),
        }
    }

    fn input_width(&self) -> usize {
        match self.chat.input_width.get() {
            0 => 80,
            width => width,
        }
    }

    /// Editing the prompt: arrows, Home/End, Ctrl or Alt with the arrows to go by
    /// word, Backspace and Delete (also by word), and the usual Ctrl+A/E/W.
    fn edit_input(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let by_word = ctrl || alt;
        let width = self.input_width();
        let input = &mut self.chat.input;
        match key.code {
            KeyCode::Enter => input.insert("\n"),
            KeyCode::Char('j') if ctrl => input.insert("\n"),
            KeyCode::Char('a') if ctrl => input.line_start(),
            KeyCode::Char('e') if ctrl => input.line_end(),
            // Many terminals send Ctrl+H for Ctrl+Backspace.
            KeyCode::Char('w' | 'h') if ctrl => input.delete_word_left(),
            KeyCode::Char('b') if alt => input.word_left(),
            KeyCode::Char('f') if alt => input.word_right(),
            KeyCode::Char('d') if alt => input.delete_word_right(),
            KeyCode::Char(ch) if !ctrl && !alt => input.insert(ch.encode_utf8(&mut [0; 4])),
            KeyCode::Left if by_word => input.word_left(),
            KeyCode::Left => input.left(),
            KeyCode::Right if by_word => input.word_right(),
            KeyCode::Right => input.right(),
            KeyCode::Home => input.line_start(),
            KeyCode::End => input.line_end(),
            KeyCode::Backspace if by_word => input.delete_word_left(),
            KeyCode::Backspace => input.backspace(),
            KeyCode::Delete if by_word => input.delete_word_right(),
            KeyCode::Delete => input.delete(),
            // With no row left to move to, the arrows scroll the chat, which is also
            // what a mouse wheel turns into in most terminals.
            KeyCode::Up if !input.up(width) => self.scroll_up(3),
            KeyCode::Down if !input.down(width) => self.scroll_down(3),
            KeyCode::Up | KeyCode::Down => {}
            _ => {}
        }
    }

    /// Esc stops a running reply. Otherwise it needs a second press: with text in
    /// the prompt it clears it, and with an empty prompt it opens the rewind list.
    fn escape(&mut self) {
        if self.chat.running {
            self.esc_armed = None;
            return self.abort();
        }
        let intent = if self.chat.input.is_empty() {
            EscIntent::Rewind
        } else {
            EscIntent::Clear
        };
        if self
            .esc_armed
            .is_some_and(|(armed, until)| armed == intent && self.tick <= until)
        {
            self.esc_armed = None;
            match intent {
                EscIntent::Clear => self.chat.input.clear(),
                EscIntent::Rewind => self.open_rewind(),
            }
            return;
        }
        self.esc_armed = Some((intent, self.tick + ESC_TICKS));
        self.notify(match intent {
            EscIntent::Clear => "Press Esc again to clear the prompt.",
            EscIntent::Rewind => "Press Esc again to rewind to an earlier prompt.",
        });
    }

    /// Ctrl+C stops a running reply, otherwise clears the prompt.
    fn interrupt(&mut self) {
        if self.chat.running {
            return self.abort();
        }
        if self.chat.input.is_empty() {
            self.notify("Press Ctrl+Q to quit.");
        } else {
            self.chat.input.clear();
        }
    }

    fn toggle_tools(&mut self) {
        self.chat.show_tools = !self.chat.show_tools;
        self.notify(if self.chat.show_tools {
            "Showing tool output in full."
        } else {
            "Tool output collapsed."
        });
    }

    fn scroll_up(&mut self, lines: usize) {
        self.chat.scroll = (self.chat.scroll + lines).min(self.chat.max_scroll.get());
    }

    fn scroll_down(&mut self, lines: usize) {
        self.chat.scroll = self.chat.scroll.saturating_sub(lines);
    }

    /// Page Up, which at the very top goes on to fetch older messages.
    fn page_up(&mut self, page: usize) {
        let at_top = self.chat.scroll >= self.chat.max_scroll.get();
        if at_top && self.chat.older_count() > 0 && !self.chat.loading {
            return self.load_older();
        }
        self.scroll_up(page);
    }

    // ---- rewind ------------------------------------------------------------------

    fn open_rewind(&mut self) {
        let prompts: Vec<RewindPrompt> = self
            .chat
            .messages
            .iter()
            .rev()
            .filter(|message| {
                message.role == MessageRole::User && !message.id.starts_with("local-")
            })
            .map(|message| RewindPrompt {
                message_id: message.id.clone(),
                text: message.content.clone(),
            })
            .collect();
        if self.chat.session.is_none() || prompts.is_empty() {
            self.notify("There is no earlier prompt to rewind to.");
            return;
        }
        self.notice = None;
        self.rewind = Some(Rewind {
            prompts,
            cursor: 0,
            busy: false,
        });
    }

    fn rewind_key(&mut self, key: KeyEvent) {
        let Some(rewind) = self.rewind.as_mut() else {
            return;
        };
        if rewind.busy {
            return;
        }
        let last = rewind.prompts.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => self.rewind = None,
            KeyCode::Up => rewind.cursor = rewind.cursor.saturating_sub(1),
            KeyCode::Down => rewind.cursor = (rewind.cursor + 1).min(last),
            KeyCode::PageUp => rewind.cursor = rewind.cursor.saturating_sub(10),
            KeyCode::PageDown => rewind.cursor = (rewind.cursor + 10).min(last),
            KeyCode::Home => rewind.cursor = 0,
            KeyCode::End => rewind.cursor = last,
            KeyCode::Enter => self.fork_selected(),
            _ => {}
        }
    }

    /// Makes a new chat from everything before the chosen prompt. The server leaves
    /// this chat as it is and puts the prompt back as a draft to edit.
    fn fork_selected(&mut self) {
        let (Some(session_id), Some(rewind)) = (self.chat.current_id.clone(), self.rewind.as_mut())
        else {
            return;
        };
        let Some(prompt) = rewind.prompts.get(rewind.cursor) else {
            return;
        };
        rewind.busy = true;
        let message_id = prompt.message_id.clone();
        let request_id = format!(
            "tui-{}-{}",
            std::process::id(),
            Utc::now().timestamp_millis()
        );
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let result = api
                .fork_session(&session_id, &message_id, &request_id)
                .await;
            let _ = tx.send(Msg::Forked(result));
        });
    }

    fn on_forked(&mut self, result: Result<ForkSessionResponse, ApiError>) {
        self.rewind = None;
        match result {
            Ok(fork) => {
                let draft = fork.draft.content;
                self.open_session(fork.session.id);
                self.chat.input.set(draft);
                self.notify(
                    "Rewound. Edit the prompt and press Enter; the original chat is unchanged.",
                );
            }
            Err(error) => self.fail(error),
        }
    }

    // ---- chat switcher ---------------------------------------------------------

    /// Typing filters the list; the arrows move through it and open or close projects.
    fn switcher_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('k') {
            self.switcher = None;
            return;
        }
        if ctrl && key.code == KeyCode::Char('r') {
            return self.open_switcher();
        }
        let Some(switcher) = self.switcher.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc if switcher.filter.is_empty() => self.switcher = None,
            KeyCode::Esc => {
                switcher.filter.clear();
                switcher.clamp();
            }
            KeyCode::Up => switcher.step(-1),
            KeyCode::Down => switcher.step(1),
            KeyCode::PageUp => (0..10).for_each(|_| switcher.step(-1)),
            KeyCode::PageDown => (0..10).for_each(|_| switcher.step(1)),
            KeyCode::Home => switcher.jump(false),
            KeyCode::End => switcher.jump(true),
            KeyCode::Right => self.expand_selected(),
            KeyCode::Left => self.collapse_selected(),
            KeyCode::Enter => self.activate_selected(),
            _ => {
                if edit(&mut switcher.filter, &key) {
                    switcher.clamp();
                }
            }
        }
    }

    fn selected_row(&self) -> Option<Row> {
        self.switcher.as_ref().and_then(Switcher::selected)
    }

    fn expand_project(&mut self, index: usize) {
        let Some(node) = self
            .switcher
            .as_mut()
            .and_then(Switcher::nodes_mut)
            .and_then(|nodes| nodes.get_mut(index))
        else {
            return;
        };
        node.expanded = true;
        if node.loaded || node.loading {
            return;
        }
        node.loading = true;
        node.error = None;
        let project_id = node.project.id.clone();
        self.fetch_sessions(project_id, 0);
    }

    fn load_more(&mut self, index: usize) {
        let Some(node) = self
            .switcher
            .as_mut()
            .and_then(Switcher::nodes_mut)
            .and_then(|nodes| nodes.get_mut(index))
        else {
            return;
        };
        if !node.has_more || node.loading {
            return;
        }
        node.loading = true;
        let (project_id, offset) = (node.project.id.clone(), node.next_offset);
        self.fetch_sessions(project_id, offset);
    }

    fn expand_selected(&mut self) {
        if let Some(Row::Project(index)) = self.selected_row() {
            self.expand_project(index);
        }
    }

    fn collapse_selected(&mut self) {
        let index = match self.selected_row() {
            Some(Row::Project(index) | Row::Session(index, _) | Row::More(index)) => index,
            _ => return,
        };
        if let Some(switcher) = &mut self.switcher {
            if let Some(node) = switcher.nodes_mut().and_then(|nodes| nodes.get_mut(index)) {
                node.expanded = false;
            }
            switcher.focus_project(index);
        }
    }

    fn activate_selected(&mut self) {
        let row = self.selected_row();
        let open = match (&row, self.switcher.as_ref()) {
            (Some(Row::Pinned(index)), Some(switcher)) => match &switcher.pinned {
                Load::Ready(pinned) => pinned.get(*index).map(|entry| entry.session_id.clone()),
                _ => None,
            },
            (Some(Row::Session(project, session)), Some(switcher)) => switcher
                .nodes()
                .and_then(|nodes| nodes.get(*project))
                .and_then(|node| node.sessions.get(*session))
                .map(|session| session.id.clone()),
            _ => None,
        };
        if let Some(session_id) = open {
            return self.open_session(session_id);
        }
        match row {
            Some(Row::Project(index)) => {
                let expanded = self
                    .switcher
                    .as_ref()
                    .and_then(Switcher::nodes)
                    .and_then(|nodes| nodes.get(index))
                    .is_some_and(|node| node.expanded);
                if expanded {
                    self.collapse_selected();
                } else {
                    self.expand_project(index);
                }
            }
            Some(Row::More(index)) => self.load_more(index),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use iowb_protocol::Provider;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use super::*;

    fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    fn key_with(code: KeyCode, modifiers: KeyModifiers) -> Msg {
        Msg::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    fn ctrl(ch: char) -> Msg {
        Msg::Key(KeyEvent {
            code: KeyCode::Char(ch),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    fn typed(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.update(key(KeyCode::Char(ch)));
        }
    }

    /// An app pointed at a closed port so background requests fail fast and offline.
    fn app() -> (App, UnboundedReceiver<Msg>) {
        let (tx, rx) = unbounded_channel();
        let config = Config {
            server_url: Some("http://127.0.0.1:1".into()),
            ..Default::default()
        };
        (App::new(tx, config, None, false), rx)
    }

    fn session(id: &str) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            title: "Fix login".into(),
            provider: Provider::Codex,
            project_path: "/work/web".into(),
            model: Some("gpt-test".into()),
            effort: Some("high".into()),
            ..Default::default()
        }
    }

    fn message(id: &str, role: MessageRole, content: &str) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            role,
            content: content.into(),
            timestamp: Utc::now(),
            metadata: serde_json::Value::Null,
        }
    }

    /// A chat that is open on `s1` with a connected socket whose commands we can read.
    fn open_chat() -> (
        App,
        UnboundedReceiver<Msg>,
        UnboundedReceiver<WsClientCommand>,
    ) {
        let (mut app, rx) = app();
        let (ws_tx, ws_rx) = unbounded_channel();
        app.screen = Screen::Chat;
        app.ws = Some(ws_tx);
        app.ws_connected = true;
        app.chat.current_id = Some("s1".into());
        app.chat.session = Some(session("s1"));
        (app, rx, ws_rx)
    }

    #[test]
    fn login_form_moves_between_fields_and_hides_nothing_until_submitted() {
        let (mut app, _rx) = app();
        assert_eq!((app.screen, app.login.focus), (Screen::Login, 1));
        typed(&mut app, "ana");
        app.update(key(KeyCode::Tab));
        typed(&mut app, "pw");
        app.update(key(KeyCode::BackTab));
        app.update(key(KeyCode::Backspace));
        assert_eq!(
            (app.login.username.as_str(), app.login.password.as_str()),
            ("an", "pw")
        );
        app.update(ctrl('u'));
        assert!(app.login.username.is_empty());
    }

    fn header(name: &str, value: &str) -> Header {
        Header {
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn the_headers_row_opens_an_editor_and_enter_keeps_valid_rows() {
        let (mut app, _rx) = app();
        app.update(key(KeyCode::Tab));
        app.update(key(KeyCode::Tab));
        assert_eq!(app.login.focus, LoginForm::HEADERS);
        app.update(key(KeyCode::Enter));
        assert!(app.login.editor.is_some());

        app.update(ctrl('f'));
        typed(&mut app, "the-id");
        app.update(key(KeyCode::Down));
        typed(&mut app, "the-secret");
        app.update(key(KeyCode::Enter));

        assert!(app.login.editor.is_none());
        assert_eq!(
            app.login.headers,
            [
                header("CF-Access-Client-Id", "the-id"),
                header("CF-Access-Client-Secret", "the-secret"),
            ]
        );
    }

    #[test]
    fn a_bad_row_keeps_the_editor_open_and_escape_drops_the_changes() {
        let (mut app, _rx) = app();
        app.login.headers = vec![header("X-Team", "blue")];
        app.login.focus = LoginForm::HEADERS;
        app.update(key(KeyCode::Enter));
        app.update(ctrl('n'));
        typed(&mut app, "X-Other");
        app.update(key(KeyCode::Enter));
        assert_eq!(
            app.login.editor.as_ref().and_then(|e| e.error.as_deref()),
            Some("Header 2 needs a value.")
        );

        app.update(key(KeyCode::Tab));
        typed(&mut app, "1");
        assert!(app.login.editor.as_ref().unwrap().error.is_none());
        app.update(key(KeyCode::Esc));
        assert!(app.login.editor.is_none());
        assert_eq!(app.login.headers, [header("X-Team", "blue")]);

        // Removing the only row leaves one blank row to type into, and blank rows save as nothing.
        app.update(key(KeyCode::Enter));
        app.update(ctrl('d'));
        app.update(key(KeyCode::Enter));
        assert!(app.login.headers.is_empty());
    }

    #[test]
    fn the_cloudflare_preset_adds_only_the_missing_rows() {
        let mut editor = HeadersEditor::new(&[header("cf-access-client-id", "id")]);
        editor.add_cloudflare_access();
        assert_eq!(
            editor.rows,
            [
                header("cf-access-client-id", "id"),
                header("CF-Access-Client-Secret", ""),
            ]
        );
        assert_eq!((editor.row, editor.on_value), (1, true));
    }

    #[test]
    fn pasting_while_the_editor_is_open_fills_its_cell() {
        let (mut app, _rx) = app();
        app.login.focus = LoginForm::HEADERS;
        app.update(key(KeyCode::Enter));
        app.update(Msg::Paste("X-Pasted\nsecond line".into()));
        assert_eq!(app.login.editor.as_ref().unwrap().rows[0].name, "X-Pasted");
        assert!(
            app.login.server.starts_with("http"),
            "the form is untouched"
        );
    }

    #[test]
    fn saved_headers_are_only_used_for_the_server_they_were_saved_for() {
        let config = Config {
            server_url: Some("http://127.0.0.1:1".into()),
            headers: vec![header("CF-Access-Client-Secret", "s3cret")],
            token: Some("tok".into()),
            ..Default::default()
        };
        let (tx, _rx) = unbounded_channel();
        let same = App::new(tx.clone(), config.clone(), None, false);
        assert_eq!(same.api.headers(), config.headers);
        assert_eq!(same.login.headers, config.headers);

        let other = App::new(tx, config, Some("http://127.0.0.1:2".into()), false);
        assert!(other.api.headers().is_empty() && other.login.headers.is_empty());
        assert_eq!(other.api.token(), None);
    }

    #[tokio::test]
    async fn signing_in_saves_the_headers_and_signing_out_keeps_them() {
        let (mut app, _rx) = app();
        app.login.headers = vec![header("CF-Access-Client-Id", "the-id")];
        app.login.busy = true;
        app.update(Msg::SignedIn(Ok(Some("tok".into()))));
        assert_eq!(app.config.headers, app.login.headers);
        assert_eq!(app.api.headers(), app.login.headers);

        // A first sign-in opens the chooser; sign out from the chat page behind it.
        app.update(ctrl('k'));
        app.update(ctrl('l'));
        assert_eq!(app.screen, Screen::Login);
        assert_eq!(app.config.token, None);
        assert_eq!(
            app.config.headers,
            [header("CF-Access-Client-Id", "the-id")]
        );
        assert_eq!(app.api.headers(), app.config.headers);
        assert_eq!(app.login.headers, app.config.headers);
    }

    #[tokio::test]
    async fn submitting_without_credentials_explains_what_is_missing() {
        let (mut app, _rx) = app();
        app.update(key(KeyCode::Tab));
        app.update(key(KeyCode::Enter));
        assert_eq!(
            app.login.error.as_deref(),
            Some("Enter a username and a password.")
        );
        assert!(!app.login.busy);
    }

    #[tokio::test]
    async fn a_failed_sign_in_shows_the_error_and_a_good_one_opens_the_chooser() {
        let (mut app, _rx) = app();
        app.login.busy = true;
        app.update(Msg::SignedIn(Err(ApiError::local("Invalid credentials"))));
        assert_eq!(app.login.error.as_deref(), Some("Invalid credentials"));
        assert!(!app.login.busy);

        app.update(Msg::SignedIn(Ok(Some("tok".into()))));
        assert_eq!(app.screen, Screen::Chat);
        assert!(
            app.switcher.is_some(),
            "a first sign-in lands in the chat chooser"
        );
        assert_eq!(app.api.token(), Some("tok"));
    }

    #[tokio::test]
    async fn an_expired_token_returns_to_the_login_form() {
        let (mut app, _rx, _ws) = open_chat();
        app.api = Api::new("http://127.0.0.1:1", Some("old".into()), Vec::new());
        app.update(Msg::Snapshot {
            session_id: "s1".into(),
            reset: false,
            result: Err(ApiError {
                status: Some(401),
                message: "unauthorized".into(),
            }),
        });
        assert_eq!(app.screen, Screen::Login);
        assert_eq!(app.api.token(), None);
        assert_eq!(
            app.login.error.as_deref(),
            Some("Your session expired. Sign in again.")
        );
    }

    #[test]
    fn ctrl_o_expands_and_collapses_tool_output_and_t_is_just_a_letter() {
        let (mut app, _rx, _ws) = open_chat();
        assert!(!app.chat.show_tools);
        app.update(ctrl('o'));
        assert!(app.chat.show_tools);
        assert_eq!(
            app.notice.as_ref().map(|(text, _)| text.as_str()),
            Some("Showing tool output in full.")
        );
        app.update(ctrl('o'));
        assert!(!app.chat.show_tools);

        app.update(key(KeyCode::Char('t')));
        assert!(!app.chat.show_tools);
        assert_eq!(app.chat.input.text(), "t");
    }

    #[test]
    fn streamed_output_accumulates_and_marks_the_reply_as_running() {
        let (mut app, _rx, _ws) = open_chat();
        let output = |session: &str, content: &str, done: bool| WsServerEvent::Output {
            provider: Provider::Codex,
            session_id: session.into(),
            content: content.into(),
            done,
            response_id: None,
            sequence: None,
        };
        app.update(Msg::Ws(output("s1", "Hel", false)));
        app.update(Msg::Ws(output("s1", "lo", false)));
        assert_eq!((app.chat.live.as_str(), app.chat.running), ("Hello", true));

        // Output for a chat that is not open must not leak into this one.
        app.update(Msg::Ws(output("other", "x", false)));
        assert_eq!(app.chat.live, "Hello");
    }

    #[tokio::test]
    async fn finishing_a_reply_refetches_and_the_stored_history_replaces_the_live_text() {
        let (mut app, mut rx, _ws) = open_chat();
        app.chat.live = "Hello".into();
        app.chat.running = true;
        app.update(Msg::Ws(WsServerEvent::Output {
            provider: Provider::Codex,
            session_id: "s1".into(),
            content: String::new(),
            done: true,
            response_id: None,
            sequence: None,
        }));
        assert!(!app.chat.running);
        assert!(
            matches!(rx.recv().await, Some(Msg::Snapshot { session_id, .. }) if session_id == "s1"),
            "a snapshot request is started when the reply completes"
        );

        let mut stored = session("s1");
        stored.active = false;
        app.update(Msg::Snapshot {
            session_id: "s1".into(),
            reset: false,
            result: Ok(SessionSnapshotResponse::without_recovery(
                stored,
                vec![message("m1", MessageRole::Assistant, "Hello")],
                false,
                1,
            )),
        });
        assert!(app.chat.live.is_empty());
        assert_eq!(app.chat.messages.len(), 1);
    }

    #[tokio::test]
    async fn a_snapshot_for_a_chat_that_is_no_longer_open_is_ignored() {
        let (mut app, _rx, _ws) = open_chat();
        app.update(Msg::Snapshot {
            session_id: "stale".into(),
            reset: true,
            result: Ok(SessionSnapshotResponse::without_recovery(
                session("stale"),
                vec![message("m", MessageRole::User, "x")],
                false,
                1,
            )),
        });
        assert!(app.chat.messages.is_empty());
        assert_eq!(app.chat.session.as_ref().unwrap().id, "s1");
    }

    #[test]
    fn enter_sends_the_prompt_with_the_sessions_own_settings() {
        let (mut app, _rx, mut ws) = open_chat();
        typed(&mut app, "fix it");
        app.update(key(KeyCode::Enter));

        match ws.try_recv().unwrap() {
            WsClientCommand::StartSession {
                provider,
                project_path,
                prompt,
                session_id,
                model,
                effort,
                ..
            } => {
                assert_eq!(provider, Provider::Codex);
                assert_eq!(project_path, "/work/web");
                assert_eq!(prompt, "fix it");
                assert_eq!(session_id.as_deref(), Some("s1"));
                assert_eq!(model.as_deref(), Some("gpt-test"));
                assert_eq!(effort.as_deref(), Some("high"));
            }
            other => panic!("unexpected command {other:?}"),
        }
        assert!(app.chat.running && app.chat.input.is_empty());
        assert_eq!(app.chat.messages.last().unwrap().content, "fix it");
        assert_eq!(app.chat.phase(), Some("Sending"));

        // While the reply runs a second prompt is refused rather than queued.
        typed(&mut app, "again");
        app.update(key(KeyCode::Enter));
        assert!(ws.try_recv().is_err());
        assert!(app.notice.is_some());
        assert_eq!(app.chat.input.text(), "again", "the draft is kept");
    }

    #[test]
    fn a_server_error_is_shown_once_even_when_its_details_repeat_it() {
        let (mut app, _rx, _ws) = open_chat();
        app.chat.running = true;
        app.update(Msg::Ws(WsServerEvent::Error {
            history_message: None,
            message: "failed to spawn agent provider".into(),
            details: Some("failed to spawn agent provider: claude: No such file".into()),
            session_id: Some("s1".into()),
        }));
        assert!(!app.chat.running);
        assert_eq!(
            app.notice.as_ref().map(|(text, _)| text.as_str()),
            Some("failed to spawn agent provider: claude: No such file")
        );

        app.update(Msg::Ws(WsServerEvent::Error {
            history_message: None,
            message: "bad request".into(),
            details: Some("missing prompt".into()),
            session_id: None,
        }));
        assert_eq!(
            app.notice.as_ref().map(|(text, _)| text.as_str()),
            Some("bad request: missing prompt")
        );
    }

    #[test]
    fn alt_enter_and_ctrl_j_add_a_line_without_sending() {
        let (mut app, _rx, mut ws) = open_chat();
        typed(&mut app, "a");
        app.update(ctrl('j'));
        typed(&mut app, "b");
        app.update(key_with(KeyCode::Enter, KeyModifiers::ALT));
        typed(&mut app, "c");
        assert_eq!(app.chat.input.text(), "a\nb\nc");
        assert!(ws.try_recv().is_err());
    }

    #[test]
    fn ctrl_c_stops_a_running_reply_and_otherwise_clears_the_prompt() {
        let (mut app, _rx, mut ws) = open_chat();
        typed(&mut app, "draft");
        app.update(ctrl('c'));
        assert!(app.chat.input.is_empty(), "idle: it clears the prompt");
        assert!(ws.try_recv().is_err(), "nothing to abort yet");
        app.update(ctrl('c'));
        assert_eq!(
            app.notice.as_ref().map(|(text, _)| text.as_str()),
            Some("Press Ctrl+Q to quit.")
        );

        app.chat.running = true;
        app.update(ctrl('c'));
        assert!(matches!(
            ws.try_recv().unwrap(),
            WsClientCommand::AbortSession { session_id, .. } if session_id == "s1"
        ));
    }

    #[test]
    fn the_prompt_has_a_cursor_that_arrows_home_end_and_word_keys_move() {
        let (mut app, _rx, _ws) = open_chat();
        typed(&mut app, "one two three");
        let text = |app: &App| app.chat.input.text().to_string();

        app.update(key_with(KeyCode::Left, KeyModifiers::CONTROL));
        typed(&mut app, "|");
        assert_eq!(text(&app), "one two |three");
        app.update(key_with(KeyCode::Left, KeyModifiers::ALT));
        typed(&mut app, "<");
        assert_eq!(text(&app), "one <two |three");
        app.update(key_with(KeyCode::Right, KeyModifiers::CONTROL));
        typed(&mut app, ">");
        assert_eq!(text(&app), "one <two> |three");

        app.update(key(KeyCode::End));
        typed(&mut app, ".");
        app.update(key(KeyCode::Home));
        typed(&mut app, "^");
        assert_eq!(text(&app), "^one <two> |three.");

        app.update(key(KeyCode::Delete));
        assert_eq!(text(&app), "^ne <two> |three.");
        app.update(key(KeyCode::Backspace));
        assert_eq!(text(&app), "ne <two> |three.");

        app.update(key(KeyCode::Right));
        app.update(key(KeyCode::Left));
        app.update(key(KeyCode::End));
        app.update(key_with(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(text(&app), "ne <two> |");
        app.update(key_with(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(text(&app), "ne <");
    }

    #[test]
    fn up_and_down_move_through_the_prompt_and_scroll_the_chat_at_its_edges() {
        let (mut app, _rx, _ws) = open_chat();
        app.chat.input_width.set(80);
        app.chat.max_scroll.set(50);
        typed(&mut app, "first");
        app.update(ctrl('j'));
        typed(&mut app, "second");

        app.update(key(KeyCode::Up));
        typed(&mut app, "!");
        assert_eq!(app.chat.input.text(), "first!\nsecond");
        assert_eq!(app.chat.scroll, 0, "the cursor moved, the chat did not");

        app.update(key(KeyCode::Up));
        assert_eq!(
            app.chat.scroll, 3,
            "already on the first row: scroll the chat"
        );
        app.update(key(KeyCode::Down));
        app.update(key(KeyCode::Down));
        assert_eq!(
            app.chat.scroll, 0,
            "already on the last row: scroll back down"
        );
    }

    #[test]
    fn page_keys_scroll_the_chat_and_ctrl_home_and_end_jump() {
        let (mut app, _rx, _ws) = open_chat();
        app.chat.view_height.set(12);
        app.chat.max_scroll.set(100);
        app.update(key(KeyCode::PageUp));
        assert_eq!(app.chat.scroll, 10);
        app.update(key(KeyCode::PageDown));
        app.update(key(KeyCode::PageDown));
        assert_eq!(app.chat.scroll, 0);
        app.update(key_with(KeyCode::Home, KeyModifiers::CONTROL));
        assert_eq!(app.chat.scroll, 100);
        app.update(key_with(KeyCode::End, KeyModifiers::CONTROL));
        assert_eq!(app.chat.scroll, 0);
        app.update(key_with(KeyCode::Up, KeyModifiers::CONTROL));
        assert_eq!(app.chat.scroll, 1);
        // Home and End alone belong to the prompt.
        typed(&mut app, "abc");
        app.update(key(KeyCode::Home));
        typed(&mut app, "x");
        assert_eq!(app.chat.input.text(), "xabc");
        assert_eq!(app.chat.scroll, 1);
    }

    #[test]
    fn ctrl_u_and_d_scroll_half_a_page_and_leave_the_prompt_alone() {
        let (mut app, _rx, _ws) = open_chat();
        app.chat.view_height.set(12);
        app.chat.max_scroll.set(100);
        typed(&mut app, "abc");
        app.update(key_with(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(app.chat.scroll, 6);
        app.update(key_with(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(app.chat.scroll, 12);
        app.update(key_with(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert_eq!(app.chat.scroll, 6);
        assert_eq!(app.chat.input.text(), "abc");
    }

    #[test]
    fn the_mouse_wheel_scrolls_the_chat_three_lines_a_notch() {
        let (mut app, _rx, _ws) = open_chat();
        app.chat.max_scroll.set(100);
        app.update(Msg::Wheel { up: true });
        app.update(Msg::Wheel { up: true });
        assert_eq!(app.chat.scroll, 6);
        app.update(Msg::Wheel { up: false });
        assert_eq!(app.chat.scroll, 3);
        app.screen = Screen::Login;
        app.update(Msg::Wheel { up: true });
        assert_eq!(app.chat.scroll, 3, "only the chat scrolls");
    }

    #[tokio::test]
    async fn page_up_at_the_very_top_loads_older_messages() {
        let (mut app, _rx, _ws) = open_chat();
        app.chat.view_height.set(12);
        app.chat.max_scroll.set(40);
        app.chat.total_count = 5;
        app.chat.messages = vec![message("m4", MessageRole::User, "4")];
        app.chat.scroll = 39;
        app.update(key(KeyCode::PageUp));
        assert!(
            !app.chat.loading,
            "one page short of the top: it only scrolls"
        );
        assert_eq!(app.chat.scroll, 40);
        app.update(key(KeyCode::PageUp));
        assert!(
            app.chat.loading,
            "at the top: it fetches the older messages"
        );
    }

    #[test]
    fn esc_stops_a_running_reply_once() {
        let (mut app, _rx, mut ws) = open_chat();
        app.chat.running = true;
        app.chat.started = true;
        assert_eq!(app.chat.phase(), Some("Processing"));
        app.update(key(KeyCode::Esc));
        assert!(matches!(
            ws.try_recv().unwrap(),
            WsClientCommand::AbortSession { session_id, .. } if session_id == "s1"
        ));
        assert_eq!(app.chat.phase(), Some("Stopping"));
        app.update(key(KeyCode::Esc));
        assert!(ws.try_recv().is_err(), "no second abort while stopping");
    }

    #[test]
    fn a_second_esc_clears_the_prompt_and_any_other_key_cancels_the_first() {
        let (mut app, _rx, _ws) = open_chat();
        typed(&mut app, "draft");
        app.update(key(KeyCode::Esc));
        assert_eq!(app.chat.input.text(), "draft");
        assert_eq!(
            app.notice.as_ref().map(|(text, _)| text.as_str()),
            Some("Press Esc again to clear the prompt.")
        );
        app.update(key(KeyCode::Esc));
        assert!(app.chat.input.is_empty());

        typed(&mut app, "x");
        app.update(key(KeyCode::Esc));
        typed(&mut app, "y");
        app.update(key(KeyCode::Esc));
        assert_eq!(
            app.chat.input.text(),
            "xy",
            "typing in between cancelled it"
        );

        for _ in 0..(ESC_TICKS + 2) {
            app.update(Msg::Tick);
        }
        app.update(key(KeyCode::Esc));
        assert_eq!(app.chat.input.text(), "xy", "too late: it only armed again");
        app.update(key(KeyCode::Esc));
        assert!(app.chat.input.is_empty());
    }

    #[test]
    fn a_second_esc_on_an_empty_prompt_lists_earlier_prompts_newest_first() {
        let (mut app, _rx, _ws) = open_chat();
        app.update(key(KeyCode::Esc));
        app.update(key(KeyCode::Esc));
        assert!(app.rewind.is_none());
        assert_eq!(
            app.notice.as_ref().map(|(text, _)| text.as_str()),
            Some("There is no earlier prompt to rewind to."),
            "nothing loaded yet"
        );

        app.chat.messages = vec![
            message("m1", MessageRole::User, "first"),
            message("m2", MessageRole::Assistant, "a reply"),
            message("m3", MessageRole::User, "second"),
            message("local-3", MessageRole::User, "not saved yet"),
        ];
        app.update(key(KeyCode::Esc));
        assert!(
            app.rewind.is_none(),
            "the first Esc only says what comes next"
        );
        assert_eq!(
            app.notice.as_ref().map(|(text, _)| text.as_str()),
            Some("Press Esc again to rewind to an earlier prompt.")
        );
        app.update(key(KeyCode::Esc));
        let rewind = app.rewind.as_ref().expect("the list is open");
        let texts: Vec<&str> = rewind.prompts.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(
            texts,
            ["second", "first"],
            "unsaved prompts cannot be rewound to"
        );

        app.update(key(KeyCode::Down));
        assert_eq!(app.rewind.as_ref().unwrap().cursor, 1);
        app.update(key(KeyCode::Down));
        assert_eq!(app.rewind.as_ref().unwrap().cursor, 1, "stops at the last");
        app.update(key(KeyCode::Home));
        assert_eq!(app.rewind.as_ref().unwrap().cursor, 0);
        app.update(key(KeyCode::Esc));
        assert!(app.rewind.is_none());
    }

    #[tokio::test]
    async fn choosing_a_prompt_makes_a_new_chat_and_puts_the_prompt_back_to_edit() {
        let (mut app, _rx, mut ws) = open_chat();
        app.chat.messages = vec![
            message("m1", MessageRole::User, "first"),
            message("m3", MessageRole::User, "second"),
        ];
        app.update(key(KeyCode::Esc));
        app.update(key(KeyCode::Esc));
        app.update(key(KeyCode::Enter));
        assert!(app.rewind.as_ref().unwrap().busy);
        app.update(key(KeyCode::Down));
        assert_eq!(
            app.rewind.as_ref().unwrap().cursor,
            0,
            "keys wait while it works"
        );

        let fork = ForkSessionResponse {
            source_session_id: "s1".into(),
            before_message_id: "m3".into(),
            session: session("s9"),
            draft: iowb_protocol::SessionDraftResponse {
                session_id: "s9".into(),
                content: "second".into(),
                updated_at: None,
            },
            native_forked: false,
            files_unchanged: true,
            source_hidden: false,
        };
        app.update(Msg::Forked(Ok(fork)));
        assert!(app.rewind.is_none());
        assert_eq!(app.chat.current_id.as_deref(), Some("s9"));
        assert_eq!(app.chat.input.text(), "second");
        assert!(matches!(
            ws.try_recv().unwrap(),
            WsClientCommand::Subscribe { chat_session_ids: Some(ids), .. } if ids == ["s9"]
        ));
    }

    #[tokio::test]
    async fn a_refused_rewind_keeps_the_chat_and_says_why() {
        let (mut app, _rx, _ws) = open_chat();
        app.chat.messages = vec![message("m1", MessageRole::User, "first")];
        app.update(key(KeyCode::Esc));
        app.update(key(KeyCode::Esc));
        app.update(key(KeyCode::Enter));
        app.update(Msg::Forked(Err(ApiError::local(
            "session_active: stop the current response before editing from here",
        ))));
        assert!(app.rewind.is_none());
        assert_eq!(app.chat.current_id.as_deref(), Some("s1"));
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|(text, _)| text.starts_with("session_active"))
        );
    }

    #[tokio::test]
    async fn a_reply_goes_from_sending_to_processing_to_stopping_like_web_and_mobile() {
        let (mut app, _rx, _ws) = open_chat();
        let status = |status| {
            Msg::Ws(WsServerEvent::SessionStatus {
                provider: Provider::Codex,
                session_id: "s1".into(),
                status,
                response_id: None,
                sequence: None,
                latest_user_prompt: None,
            })
        };
        assert_eq!(app.chat.phase(), None);
        typed(&mut app, "go");
        app.update(key(KeyCode::Enter));
        assert_eq!(app.chat.phase(), Some("Sending"));
        app.update(status(SessionRuntimeStatus::Starting));
        assert_eq!(
            app.chat.phase(),
            Some("Sending"),
            "starting is still sending"
        );
        app.update(status(SessionRuntimeStatus::Running));
        assert_eq!(app.chat.phase(), Some("Processing"));
        app.update(key(KeyCode::Esc));
        assert_eq!(app.chat.phase(), Some("Stopping"));
        app.update(status(SessionRuntimeStatus::Aborted));
        assert_eq!(app.chat.phase(), None);
        assert!(!app.chat.stopping && !app.chat.started);
    }

    #[tokio::test]
    async fn older_pages_are_prepended_without_duplicates() {
        let (mut app, _rx, _ws) = open_chat();
        app.chat.messages = vec![
            message("m3", MessageRole::User, "3"),
            message("m4", MessageRole::Assistant, "4"),
        ];
        app.chat.total_count = 4;
        assert_eq!(app.chat.older_count(), 2);
        app.update(Msg::Older {
            session_id: "s1".into(),
            result: Ok(MessagesResponse {
                session_id: "s1".into(),
                messages: vec![
                    message("m1", MessageRole::User, "1"),
                    message("m2", MessageRole::Assistant, "2"),
                    message("m3", MessageRole::User, "3"),
                ],
                has_more: false,
                total_count: 4,
            }),
        });
        let ids: Vec<_> = app.chat.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["m1", "m2", "m3", "m4"]);
        assert_eq!(app.chat.older_count(), 0);
    }

    #[tokio::test]
    async fn the_switcher_opens_with_ctrl_k_expands_projects_and_opens_a_session() {
        let (mut app, _rx, mut ws) = open_chat();
        app.update(ctrl('k'));
        assert!(app.switcher.is_some());

        let now = Utc::now();
        let project = ProjectSummary {
            id: "p-web".into(),
            name: "web".into(),
            path: "/work/web".into(),
            repo_name: None,
            created_at: now,
            updated_at: now,
            sessions: Vec::new(),
        };
        app.update(Msg::Pinned(Ok(Vec::new())));
        app.update(Msg::Projects(Ok(vec![project])));
        // The open chat's project starts expanded and loading.
        let node = &app.switcher.as_ref().unwrap().nodes().unwrap()[0];
        assert!(node.expanded && node.loading);

        app.update(Msg::ProjectSessions {
            project_id: "p-web".into(),
            offset: 0,
            result: Ok(ProjectSessionsPage {
                sessions: vec![session("s2")],
                has_more: false,
                total_count: 1,
            }),
        });
        let switcher = app.switcher.as_mut().unwrap();
        switcher.cursor = switcher
            .rows()
            .iter()
            .position(|row| *row == Row::Session(0, 0))
            .unwrap();
        app.update(key(KeyCode::Enter));

        assert!(app.switcher.is_none());
        assert_eq!(app.chat.current_id.as_deref(), Some("s2"));
        assert!(app.chat.loading);
        match ws.try_recv().unwrap() {
            WsClientCommand::Subscribe {
                chat_session_ids, ..
            } => {
                assert_eq!(chat_session_ids, Some(vec!["s2".to_string()]));
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn pasted_text_goes_in_at_the_cursor_without_sending() {
        let (mut app, _rx, mut ws) = open_chat();
        typed(&mut app, "ab");
        app.update(key(KeyCode::Left));
        app.update(Msg::Paste("line one\r\nline two".into()));
        assert_eq!(app.chat.input.text(), "aline one\nline twob");
        assert!(
            ws.try_recv().is_err(),
            "a pasted newline must not send the message"
        );
    }

    #[tokio::test]
    async fn typing_in_the_switcher_filters_it_and_escape_clears_before_it_closes() {
        let (mut app, _rx, _ws) = open_chat();
        app.update(ctrl('k'));
        typed(&mut app, "jk fix");
        assert_eq!(
            app.switcher.as_ref().unwrap().filter,
            "jk fix",
            "letters are text here too"
        );
        app.update(key(KeyCode::Backspace));
        assert_eq!(app.switcher.as_ref().unwrap().filter, "jk fi");
        app.update(key(KeyCode::Esc));
        assert_eq!(app.switcher.as_ref().unwrap().filter, "");
        assert!(app.switcher.is_some());
        app.update(key(KeyCode::Esc));
        assert!(app.switcher.is_none());
        assert_eq!(app.chat.input.text(), "", "nothing leaked into the prompt");
    }
}

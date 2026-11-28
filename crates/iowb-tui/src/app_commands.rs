#[derive(Default)]
pub struct CommandUi {
    pub catalog: Option<iowb_protocol::ChatCommandCatalog>,
    pub selected: usize,
    pub models: Option<Vec<String>>,
    scope: String,
    loading: bool,
    fetched_at: usize,
    hidden_for: Option<String>,
    preparing: bool,
    compact_request: Option<String>,
}

impl App {
    fn command_context(&self) -> Option<iowb_protocol::ChatCommandContext> {
        let session = self.chat.session.as_ref()?;
        Some(iowb_protocol::ChatCommandContext {
            provider: session.provider,
            project_path: session.project_path.clone(),
            session_id: (!(session.id.starts_with("draft-")
                && self.chat.total_count == 0
                && !self.chat.running))
                .then(|| session.id.clone()),
            model: session.model.clone(),
            runtime: session.runtime.unwrap_or_default(),
        })
    }

    fn command_scope(&self) -> String {
        format!(
            "{}:{}",
            self.api.base(),
            serde_json::to_string(&self.command_context()).unwrap_or_default()
        )
    }

    fn sync_command_scope(&mut self) -> String {
        let scope = self.command_scope();
        if self.command_ui.scope != scope {
            self.command_ui = CommandUi {
                scope: scope.clone(),
                ..Default::default()
            };
        }
        scope
    }

    pub fn command_matches(&self) -> Vec<&iowb_protocol::ChatCommandDescriptor> {
        let prompt = self.chat.input.text();
        let Some(prefix) = prompt.strip_prefix('/') else {
            return Vec::new();
        };
        if self.command_ui.hidden_for.as_deref() == Some(prompt)
            || prefix.chars().any(char::is_whitespace)
            || prefix.contains('/')
        {
            return Vec::new();
        }
        self.command_ui
            .catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .commands
                    .iter()
                    .filter(|command| command.name.starts_with(prefix))
                    .take(40)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn maybe_load_commands(&mut self) {
        if self.screen != Screen::Chat || !self.chat.input.text().starts_with('/') {
            return;
        }
        let scope = self.sync_command_scope();
        if self.command_ui.loading
            || ((self.command_ui.catalog.is_some() || self.command_ui.fetched_at > 0)
                && self.tick.saturating_sub(self.command_ui.fetched_at) < 150)
        {
            return;
        }
        let Some(context) = self.command_context() else {
            return;
        };
        self.command_ui.loading = true;
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let _ = tx.send(Msg::Commands {
                scope,
                result: api.chat_commands(&context).await,
            });
        });
    }

    fn on_commands(
        &mut self,
        scope: String,
        result: Result<iowb_protocol::ChatCommandCatalog, ApiError>,
    ) {
        if self.command_scope() != scope {
            return;
        }
        self.command_ui.loading = false;
        self.command_ui.fetched_at = self.tick;
        match result {
            Ok(catalog) => self.command_ui.catalog = Some(catalog),
            Err(error) => {
                self.command_ui.hidden_for = Some(self.chat.input.text().to_string());
                self.notify(error.message);
            }
        }
    }

    fn prepare_command(&mut self, prompt: String) {
        let scope = self.sync_command_scope();
        if self.command_ui.preparing {
            return;
        }
        let Some(context) = self.command_context() else {
            return;
        };
        self.command_ui.preparing = true;
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let result = api.prepare_chat_command(context, prompt.clone()).await;
            let _ = tx.send(Msg::PreparedCommand {
                scope,
                prompt,
                result,
            });
        });
    }

    fn on_prepared_command(
        &mut self,
        scope: String,
        prompt: String,
        result: Result<iowb_protocol::PreparedChatCommand, ApiError>,
    ) {
        if self.command_scope() != scope {
            return;
        }
        self.command_ui.preparing = false;
        if self.chat.input.text().trim() != prompt {
            return;
        }
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                self.notify(error.message);
                return;
            }
        };
        match prepared.action {
            ChatCommandAction::Prompt => self.send_prompt_checked(true),
            ChatCommandAction::Mcp => {
                self.chat.input.clear();
                self.notify(prepared.message.unwrap_or_default().replace('\n', " · "));
            }
            ChatCommandAction::Help => {
                self.chat.input.set("/");
                self.command_ui.hidden_for = None;
                self.maybe_load_commands();
            }
            ChatCommandAction::Status => {
                self.chat.input.clear();
                if let Some(session) = &self.chat.session {
                    self.notify(format!(
                        "{} · {} · {}",
                        session.provider.as_str(),
                        session.model.as_deref().unwrap_or("CLI default"),
                        if self.chat.running { "running" } else { "idle" }
                    ));
                }
            }
            ChatCommandAction::New => {
                if let Some(mut session) = self.chat.session.clone() {
                    session.id = format!("draft-{}", iowb_protocol::new_id("chat"));
                    session.native_session_id = None;
                    session.external = false;
                    session.active = false;
                    session.message_count = 0;
                    session.title = "New chat".to_string();
                    self.chat = Chat {
                        current_id: Some(session.id.clone()),
                        session: Some(session.clone()),
                        ..Default::default()
                    };
                    self.command_ui = CommandUi::default();
                    self.send_ws(WsClientCommand::Subscribe {
                        topics: vec!["sessions".into()],
                        chat_session_ids: Some(vec![session.id]),
                        process_ids: Vec::new(),
                    });
                    self.notify("Fresh chat. Previous history is unchanged.");
                }
            }
            ChatCommandAction::Model => {
                let Some(session) = &self.chat.session else {
                    return;
                };
                let provider = session.provider;
                self.chat.input.clear();
                self.command_ui.selected = 0;
                self.command_ui.models = Some(Vec::new());
                let (api, tx) = (self.api.clone(), self.tx.clone());
                tokio::spawn(async move {
                    let _ = tx.send(Msg::CommandModels {
                        scope,
                        result: api.chat_models(provider).await,
                    });
                });
            }
            ChatCommandAction::Compact => {
                let Some(session_id) = self.chat.current_id.clone() else {
                    return;
                };
                let request_id = self
                    .command_ui
                    .compact_request
                    .get_or_insert_with(|| iowb_protocol::new_id("compact"))
                    .clone();
                self.command_ui.preparing = true;
                let (api, tx) = (self.api.clone(), self.tx.clone());
                tokio::spawn(async move {
                    let _ = tx.send(Msg::CommandDone {
                        scope,
                        result: api.compact_chat(&session_id, &request_id).await,
                    });
                });
            }
        }
    }

    fn command_key(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT)
        {
            return false;
        }
        let model_count = self.command_ui.models.as_ref().map(Vec::len);
        let count = model_count.unwrap_or_else(|| self.command_matches().len());
        if model_count.is_none() && count == 0 {
            return false;
        }
        match key.code {
            KeyCode::Esc => {
                self.command_ui.models = None;
                self.command_ui.hidden_for = Some(self.chat.input.text().to_string());
            }
            KeyCode::Up | KeyCode::Down => {
                if count > 0 {
                    self.command_ui.selected = (self.command_ui.selected
                        + if key.code == KeyCode::Up {
                            count - 1
                        } else {
                            1
                        })
                        % count;
                }
            }
            KeyCode::Tab | KeyCode::Enter => {
                let selected = self.command_ui.selected.min(count.saturating_sub(1));
                if let Some(models) = &self.command_ui.models {
                    if let Some(model) = models.get(selected).cloned() {
                        if let Some(session) = &mut self.chat.session {
                            session.model = (!model.is_empty()).then_some(model);
                        }
                        self.command_ui.models = None;
                        self.command_ui.catalog = None;
                        self.notify("Model selected for the next message.");
                    }
                } else if let Some(command) = self.command_matches().get(selected).copied() {
                    if command.available {
                        self.chat.input.set(format!("/{} ", command.name));
                    } else {
                        self.notify(command.reason.clone().unwrap_or_default());
                    }
                }
            }
            _ => return model_count.is_some(),
        }
        true
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use iowb_protocol::Provider;

    fn app() -> App {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            tx,
            Config::default(),
            Some("http://127.0.0.1:1".into()),
            false,
        );
        app.screen = Screen::Chat;
        app.chat.current_id = Some("session-fixture".into());
        app.chat.session = Some(SessionSummary {
            id: "session-fixture".into(),
            provider: Provider::Claude,
            project_path: "/project".into(),
            message_count: 2,
            ..Default::default()
        });
        app
    }

    fn catalog() -> iowb_protocol::ChatCommandCatalog {
        iowb_protocol::ChatCommandCatalog {
            commands: vec![iowb_protocol::ChatCommandDescriptor {
                id: "workbench:help".into(),
                name: "help".into(),
                description: "Help".into(),
                argument_hint: String::new(),
                kind: iowb_protocol::ChatCommandKind::AppAction,
                source: "workbench".into(),
                action: ChatCommandAction::Help,
                available: true,
                reason: None,
                verified: true,
            }],
            revision: "fixture".into(),
            warnings: Vec::new(),
            authoritative: false,
        }
    }

    fn prepared(
        action: ChatCommandAction,
        prompt: &str,
    ) -> Result<iowb_protocol::PreparedChatCommand, ApiError> {
        Ok(iowb_protocol::PreparedChatCommand {
            action,
            prompt: prompt.into(),
            command: None,
            message: None,
        })
    }

    #[test]
    fn chat_commands_menu_selects_without_sending_and_escape_hides_it() {
        let mut app = app();
        app.command_ui.catalog = Some(catalog());
        app.chat.input.set("/he");
        assert_eq!(app.command_matches().len(), 1);
        assert!(app.command_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        assert_eq!(app.chat.input.text(), "/help ");
        assert!(!app.chat.running);
        app.chat.input.set("/");
        assert!(app.command_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(app.command_matches().is_empty());
        assert_eq!(app.chat.input.text(), "/");
    }

    #[test]
    fn chat_commands_stale_preparation_does_not_change_a_new_draft_or_context() {
        let mut app = app();
        let scope = app.command_scope();
        app.chat.input.set("New draft");
        app.on_prepared_command(scope.clone(), "/clear".into(), prepared(ChatCommandAction::New, "/clear"));
        assert_eq!(app.chat.input.text(), "New draft");
        app.chat.input.set("/clear");
        app.chat.session.as_mut().unwrap().project_path = "/other".into();
        app.on_prepared_command(scope, "/clear".into(), prepared(ChatCommandAction::New, "/clear"));
        assert_eq!(app.chat.current_id.as_deref(), Some("session-fixture"));
        assert_eq!(app.chat.input.text(), "/clear");
    }

    #[test]
    fn chat_commands_new_creates_only_local_draft_and_does_not_delete_original() {
        let mut app = app();
        app.chat.session.as_mut().unwrap().native_session_id = Some("native-fixture".into());
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        app.ws = Some(sender);
        app.chat.input.set("/new");
        app.on_prepared_command(app.command_scope(), "/new".into(), prepared(ChatCommandAction::New, "/new"));
        let draft = app.chat.session.as_ref().unwrap();
        assert!(draft.id.starts_with("draft-"));
        assert!(draft.native_session_id.is_none());
        assert_eq!(draft.message_count, 0);
        assert!(app.chat.messages.is_empty());
        assert!(matches!(
            receiver.try_recv().unwrap(),
            WsClientCommand::Subscribe { .. }
        ));
        assert!(receiver.try_recv().is_err());
        assert!(app.command_context().unwrap().session_id.is_none());
        app.chat.total_count = 2;
        assert_eq!(
            app.command_context().unwrap().session_id.as_deref(),
            app.chat.current_id.as_deref()
        );
    }

    #[tokio::test]
    async fn chat_commands_preparation_is_not_reset_by_the_first_catalog_tick() {
        let mut app = app();
        app.chat.input.set("/status");
        app.prepare_command("/status".into());
        assert!(app.command_ui.preparing);
        app.maybe_load_commands();
        assert!(app.command_ui.preparing);
        app.on_prepared_command(
            app.command_scope(),
            "/status".into(),
            prepared(ChatCommandAction::Status, "/status"),
        );
        assert!(!app.command_ui.preparing);
        assert!(app.chat.input.text().is_empty());
    }

    #[test]
    fn chat_commands_mcp_shows_the_server_message_and_clears_the_draft() {
        let mut app = app();
        app.chat.input.set("/mcp");
        let mut prepared = prepared(ChatCommandAction::Mcp, "/mcp").unwrap();
        prepared.message = Some("github — connected\nidle — configured".into());
        app.on_prepared_command(app.command_scope(), "/mcp".into(), Ok(prepared));
        assert!(app.chat.input.text().is_empty());
        assert_eq!(
            app.notice.as_ref().map(|(text, _)| text.as_str()),
            Some("github — connected · idle — configured")
        );
    }
}

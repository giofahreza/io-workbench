impl SessionManager {
    pub fn load(storage: Storage, max_sessions: usize) -> Result<Self> {
        // Active sessions are reconciled against durable chat runs by the
        // server after AppState is fully initialized. Clearing them here would
        // destroy the only durable signal that a forced-stop recovery is due.
        let persisted_sessions = storage.list_sessions_including_board()?;
        let board_session_ids = persisted_sessions
            .iter()
            .filter(|session| session.is_board_session())
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
        board_id: String,
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
            Some((board_id, board_task_id)),
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
                board_id: None,
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
                context_token_usage: None,
                spent_token_usage: None,
                native_session_id: None,
                native_rollout_owned_by_provider: false,
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
        if let Some((board_id, board_task_id)) = board_scope {
            if board_id.trim().is_empty() {
                return Err(CoreError::InvalidInput(
                    "board id must not be empty".to_string(),
                ));
            }
            session.board_session = true;
            session.board_id = Some(board_id);
            session.board_task_id = board_task_id.filter(|value| !value.trim().is_empty());
        }

        self.storage.upsert_session(session)?;
        if session.is_board_session() {
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
        board_id: impl Into<String>,
        board_task_id: Option<String>,
    ) -> Result<SessionSummary> {
        let board_id = board_id.into();
        if board_id.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "board id must not be empty".to_string(),
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
        session.board_id = Some(board_id);
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

    pub async fn set_native_rollout_owned_by_provider(
        &self,
        session_id: &str,
        owned: bool,
    ) -> Result<SessionSummary> {
        let mut sessions = self.sessions.write().await;
        if !sessions.contains_key(session_id)
            && let Some(stored) = self.storage.get_session_summary(session_id)?
        {
            sessions.insert(session_id.to_string(), stored);
        }
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;
        if session.native_rollout_owned_by_provider != owned {
            session.native_rollout_owned_by_provider = owned;
            self.storage.upsert_session(session)?;
        }
        Ok(session.clone())
    }

    async fn has_provider_owned_native_rollout(&self, session_id: &str) -> Result<bool> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        }
        .or_else(|| self.storage.get_session_summary(session_id).ok().flatten());
        Ok(session.is_some_and(|session| {
            !session.external
                && session.provider == Provider::Codex
                && session.native_session_id.is_some()
                && session.native_rollout_owned_by_provider
        }))
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
                .filter(|session| session.active && !session.is_board_session())
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

}

impl SessionManager {
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
            if record.summary.is_board_session() {
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
                    let spent_token_usage = existing.spent_token_usage.clone();
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
                    existing.spent_token_usage = spent_token_usage;
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
            && !session.native_rollout_owned_by_provider
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

}

impl SessionManager {
    pub async fn get(&self, session_id: &str) -> Result<SessionSummary> {
        if let Some(mut session) = self.sessions.read().await.get(session_id).cloned() {
            self.refresh_summary_message_count(&mut session).await?;
            return Ok(session);
        }

        if let Some(mut session) = self.storage.get_session(session_id)? {
            self.refresh_summary_message_count(&mut session).await?;
            return Ok(session);
        }
        if let Some(record) = self.external_record(session_id, None, None).await {
            let mut session = record.summary.clone();
            session.message_count = self.external_record_message_count(&record).await;
            return Ok(session);
        }
        Err(CoreError::SessionNotFound(session_id.to_string()))
    }

    pub async fn remember_persisted_session(&self, session: SessionSummary) -> Result<()> {
        if session.is_board_session() {
            self.board_session_ids
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(session.id.clone());
        }
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.id.clone(), session);
        self.evict_if_needed(&mut sessions)
    }

    /// Board visibility is checked for every streamed WebSocket event. Keep
    /// this lookup entirely in memory so output chunks never contend on the
    /// single SQLite connection.
    pub fn is_board_session_cached(&self, session_id: &str) -> bool {
        self.board_session_ids
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(session_id)
    }

    fn get_stored(&self, session_id: &str) -> Option<SessionSummary> {
        self.storage.get_session_summary(session_id).ok().flatten()
    }

    async fn external_records(&self) -> Vec<ExternalSessionRecord> {
        const CACHE_TTL: Duration = Duration::from_secs(30);
        let cached = {
            let cache = self.external_cache.read().await;
            cache
                .loaded_at
                .is_some_and(|loaded_at| loaded_at.elapsed() < CACHE_TTL)
                .then(|| cache.records.clone())
        };
        let mut records = if let Some(records) = cached {
            records
        } else {
            // Only one request may refresh provider indexes at a time. Recheck
            // after acquiring the guard so concurrent mobile/web connections
            // reuse the first completed synchronization.
            let _sync_guard = self.external_sync.lock().await;
            let refreshed = {
                let cache = self.external_cache.read().await;
                cache
                    .loaded_at
                    .is_some_and(|loaded_at| loaded_at.elapsed() < CACHE_TTL)
                    .then(|| cache.records.clone())
            };
            if let Some(records) = refreshed {
                records
            } else {
                let stale_records = self.external_cache.read().await.records.clone();
                let external_home = self.external_home.clone();
                let storage = self.storage.clone();
                let records = match tokio::task::spawn_blocking(move || {
                    sync_external_sessions(&external_home, &storage)
                })
                .await
                {
                    Ok(Ok(records)) => records,
                    Ok(Err(error)) => {
                        warn!(%error, "external session synchronization failed");
                        stale_records
                    }
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
                    record.summary.message_count = cached.total_count;
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
        let replaced_source_ids = match self.storage.list_replaced_source_session_ids() {
            Ok(session_ids) => session_ids.into_iter().collect::<HashSet<_>>(),
            Err(error) => {
                warn!(%error, "failed to load replaced source session ids");
                HashSet::new()
            }
        };
        records
            .into_iter()
            .filter(|record| !mapped_native_ids.contains(&record.summary.id))
            .filter(|record| !replaced_source_ids.contains(&record.summary.id))
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
            .or_else(|| self.storage.get_session_summary(session_id).ok().flatten())?;
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

    async fn native_rollout_size(&self, session_id: &str) -> Option<u64> {
        let record = self.external_record_for_messages(session_id).await?;
        std::fs::metadata(record.file_path)
            .ok()
            .map(|metadata| metadata.len())
    }

    /// Resolve the provider-owned transcript through the existing discovery
    /// index/cache. Callers should use this instead of recursively walking the
    /// entire CLI history tree for a known session id.
    pub async fn external_session_file(&self, session_id: &str) -> Option<PathBuf> {
        self.external_record_for_messages(session_id)
            .await
            .map(|record| record.file_path)
    }

    async fn external_messages_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<Vec<ChatMessage>>> {
        if self.has_provider_owned_native_rollout(session_id).await? {
            return Ok(None);
        }
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

    async fn active_context_messages_including_external(
        &self,
        session_id: &str,
    ) -> Result<Vec<ChatMessage>> {
        let stored = sanitize_context_materialization_messages(self.messages(session_id)?);
        let compacted_at = match latest_context_compaction_marker_timestamp(&stored) {
            Some(compacted_at) => Some(compacted_at),
            None => self
                .storage
                .latest_context_rollover(session_id)?
                .filter(|rollover| rollover.state == "active")
                .and_then(|rollover| rollover.activated_at),
        };
        let Some(record) = self.external_record_for_messages(session_id).await else {
            return Ok(stored);
        };
        let external = self.external_messages(&record).await;
        if external.is_empty() {
            return Ok(stored);
        }
        Ok(merge_active_context_external_messages(
            stored,
            external.as_ref().clone(),
            compacted_at,
        ))
    }

    async fn external_messages_tail_for_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Option<(Vec<ChatMessage>, usize)>> {
        if self.has_provider_owned_native_rollout(session_id).await? {
            return Ok(None);
        }
        let Some(record) = self.external_record_for_messages(session_id).await else {
            return Ok(None);
        };
        let (external, external_total) = self.external_messages_tail(&record, limit).await;
        if external.is_empty() {
            return Ok(None);
        }
        if record.summary.id == session_id {
            return Ok(Some((external, external_total)));
        }

        let stored = self.messages(session_id)?;
        let stored_system_count = stored
            .iter()
            .filter(|message| message.role == MessageRole::System)
            .count();
        let merged = merge_mapped_external_messages(stored, external);
        let start = merged.len().saturating_sub(limit);
        Ok(Some((
            merged[start..].to_vec(),
            external_total.saturating_add(stored_system_count),
        )))
    }

    async fn external_messages_tail(
        &self,
        record: &ExternalSessionRecord,
        limit: usize,
    ) -> (Vec<ChatMessage>, usize) {
        let key = external_session_cache_key(record);
        let modified_at = std::fs::metadata(&record.file_path)
            .and_then(|metadata| metadata.modified())
            .ok();
        {
            let mut cache = self.external_cache.write().await;
            if let Some(cached) = cache.messages.get_mut(&key) {
                if cached.modified_at == modified_at {
                    cached.last_access = Instant::now();
                    let start = cached.messages.len().saturating_sub(limit);
                    return (cached.messages[start..].to_vec(), cached.total_count);
                }
                if let Some(stale) = cache.messages.remove(&key) {
                    cache.message_bytes = cache.message_bytes.saturating_sub(stale.estimated_bytes);
                }
            }
        }

        if let Some(fingerprint) = external_file_fingerprint(&record.file_path) {
            let file_path = record.file_path.display().to_string();
            let storage_fingerprint = ExternalHistoryFingerprint {
                file_identity: fingerprint.file_identity.as_deref(),
                file_size: fingerprint.file_size,
                modified_nanos: fingerprint.modified_nanos,
                parser_version: EXTERNAL_MESSAGE_PARSER_VERSION,
            };
            match self.storage.external_messages_tail_if_current(
                record.summary.provider,
                &record.summary.id,
                &file_path,
                &storage_fingerprint,
                limit,
            ) {
                Ok(Some(messages)) => return messages,
                Ok(None) => {}
                Err(error) => warn!(
                    %error,
                    session_id = %record.summary.id,
                    "failed to read persisted external message tail"
                ),
            }
        }

        let messages = self.external_messages(record).await;
        let total = messages.len();
        let start = total.saturating_sub(limit);
        (messages[start..].to_vec(), total)
    }

    async fn external_messages(&self, record: &ExternalSessionRecord) -> Arc<Vec<ChatMessage>> {
        let key = external_session_cache_key(record);
        let modified_at = std::fs::metadata(&record.file_path)
            .and_then(|metadata| metadata.modified())
            .ok();
        {
            let mut cache = self.external_cache.write().await;
            if let Some(cached) = cache.messages.get_mut(&key) {
                if cached.modified_at == modified_at && cached.complete {
                    cached.last_access = Instant::now();
                    return cached.messages.clone();
                } else if let Some(stale) = cache.messages.remove(&key) {
                    cache.message_bytes = cache.message_bytes.saturating_sub(stale.estimated_bytes);
                }
            }
        }

        let cache_warning_session_id = record.summary.id.clone();
        let cache_warning_provider = record.summary.provider;
        let fingerprint_before = external_file_fingerprint(&record.file_path);
        let file_path = record.file_path.display().to_string();
        let persisted = fingerprint_before.as_ref().and_then(|fingerprint| {
            let storage_fingerprint = ExternalHistoryFingerprint {
                file_identity: fingerprint.file_identity.as_deref(),
                file_size: fingerprint.file_size,
                modified_nanos: fingerprint.modified_nanos,
                parser_version: EXTERNAL_MESSAGE_PARSER_VERSION,
            };
            match self.storage.external_messages_if_current(
                record.summary.provider,
                &record.summary.id,
                &file_path,
                &storage_fingerprint,
            ) {
                Ok(messages) => messages,
                Err(error) => {
                    warn!(
                        %error,
                        session_id = %record.summary.id,
                        "failed to read persisted external messages"
                    );
                    None
                }
            }
        });
        let messages = if let Some(messages) = persisted {
            Arc::new(messages)
        } else {
            let parse_record = record.clone();
            let messages =
                match tokio::task::spawn_blocking(move || load_external_messages(&parse_record))
                    .await
                {
                    Ok(messages) => Arc::new(messages),
                    Err(error) => {
                        warn!(%error, "external session parser worker failed");
                        return Arc::new(Vec::new());
                    }
                };
            let fingerprint_after = external_file_fingerprint(&record.file_path);
            if fingerprint_before == fingerprint_after
                && let Some(fingerprint) = fingerprint_after.as_ref()
            {
                let storage_fingerprint = ExternalHistoryFingerprint {
                    file_identity: fingerprint.file_identity.as_deref(),
                    file_size: fingerprint.file_size,
                    modified_nanos: fingerprint.modified_nanos,
                    parser_version: EXTERNAL_MESSAGE_PARSER_VERSION,
                };
                if let Err(error) = self.storage.replace_external_messages(
                    record.summary.provider,
                    &record.summary.id,
                    &file_path,
                    &storage_fingerprint,
                    messages.as_ref(),
                ) {
                    warn!(
                        %error,
                        session_id = %record.summary.id,
                        "failed to persist external messages"
                    );
                }
            }
            messages
        };
        let estimated_bytes = estimate_external_messages_bytes(&messages);
        let total_count = messages.len();
        let mut cache = self.external_cache.write().await;
        if let Some(stale) = cache.messages.remove(&key) {
            cache.message_bytes = cache.message_bytes.saturating_sub(stale.estimated_bytes);
        }
        let (cached_messages, cached_bytes, complete) =
            if estimated_bytes <= EXTERNAL_MESSAGE_CACHE_MAX_BYTES {
                (messages.clone(), estimated_bytes, true)
            } else {
                let tail = bounded_external_message_tail(
                    &messages,
                    EXTERNAL_MESSAGE_TAIL_CACHE_MAX_MESSAGES,
                    EXTERNAL_MESSAGE_CACHE_MAX_BYTES,
                );
                let cached_bytes = estimate_external_messages_bytes(&tail);
                (Arc::new(tail), cached_bytes, false)
            };
        if cached_bytes <= EXTERNAL_MESSAGE_CACHE_MAX_BYTES {
            cache.messages.insert(
                key,
                CachedExternalMessages {
                    modified_at,
                    estimated_bytes: cached_bytes,
                    last_access: Instant::now(),
                    total_count,
                    complete,
                    messages: cached_messages,
                },
            );
            cache.message_bytes = cache.message_bytes.saturating_add(cached_bytes);
            evict_external_message_cache(&mut cache);
        }
        if !complete {
            warn!(
                session_id = %cache_warning_session_id,
                provider = cache_warning_provider.as_str(),
                estimated_bytes,
                max_bytes = EXTERNAL_MESSAGE_CACHE_MAX_BYTES,
                cached_tail_messages = total_count.min(EXTERNAL_MESSAGE_TAIL_CACHE_MAX_MESSAGES),
                "external session messages exceed full-cache budget; retained a bounded tail"
            );
        }
        messages
    }

}

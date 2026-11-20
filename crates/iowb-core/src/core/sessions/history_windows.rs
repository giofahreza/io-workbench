impl SessionManager {
    async fn cached_history_window(&self, key: &str) -> Option<CachedHistoryWindow> {
        let mut cache = self.external_cache.write().await;
        let entry = cache.history_windows.get_mut(key)?;
        entry.last_access = Instant::now();
        Some(entry.clone())
    }

    async fn remember_history_window(
        &self,
        key: String,
        version: Option<MessageHistoryVersion>,
        fingerprint: Option<ExternalFileFingerprint>,
        value: HistoryWindowValue,
    ) {
        let estimated_bytes = match &value {
            HistoryWindowValue::Messages(messages) => estimate_external_messages_bytes(messages),
            HistoryWindowValue::Descriptors(descriptors) => descriptors
                .iter()
                .map(|descriptor| {
                    std::mem::size_of::<ActiveContextMessageDescriptor>()
                        + match &descriptor.source {
                            ActiveContextMessageSource::Stored(id) => id.len(),
                            ActiveContextMessageSource::External(_) => 0,
                        }
                })
                .sum(),
        };
        if estimated_bytes > EXTERNAL_MESSAGE_CACHE_MAX_BYTES {
            return;
        }
        let mut cache = self.external_cache.write().await;
        cache.history_windows.insert(
            key,
            CachedHistoryWindow {
                version,
                fingerprint,
                last_access: Instant::now(),
                estimated_bytes,
                value,
            },
        );
        evict_external_message_cache(&mut cache);
    }

    async fn external_messages_window_for_session(
        &self,
        session_id: &str,
        limit: usize,
        offset: Option<usize>,
    ) -> Result<Option<(Vec<ChatMessage>, usize)>> {
        if self.has_provider_owned_native_rollout(session_id).await? {
            return Ok(None);
        }
        let Some(record) = self.external_record_for_messages(session_id).await else {
            return Ok(None);
        };
        if record.summary.id == session_id {
            if offset.is_none() {
                let page = self.external_messages_tail(&record, limit).await;
                return Ok((!page.0.is_empty()).then_some(page));
            }
            if let Some(fingerprint) = external_file_fingerprint(&record.file_path) {
                let storage_fingerprint = ExternalHistoryFingerprint {
                    file_identity: fingerprint.file_identity.as_deref(),
                    file_size: fingerprint.file_size,
                    modified_nanos: fingerprint.modified_nanos,
                    parser_version: EXTERNAL_MESSAGE_PARSER_VERSION,
                };
                if let Some(page) = self.storage.external_messages_page_if_current(
                    record.summary.provider,
                    &record.summary.id,
                    &record.file_path.display().to_string(),
                    &storage_fingerprint,
                    limit,
                    offset.unwrap_or(0),
                )? {
                    return Ok(Some(page));
                }
            }
            let messages = self.external_messages(&record).await;
            if messages.is_empty() {
                return Ok(None);
            }
            return Ok(Some(history_message_window(&messages, limit, offset)));
        }

        let key = format!(
            "mapped:{session_id}:{}",
            external_session_cache_key(&record)
        );
        for _ in 0..2 {
            let version = self.storage.message_history_version(session_id)?;
            let fingerprint = external_file_fingerprint(&record.file_path);
            if fingerprint.is_some()
                && let Some(cached) = self.cached_history_window(&key).await
                && cached.version.as_ref() == Some(&version)
                && cached.fingerprint == fingerprint
                && let HistoryWindowValue::Messages(messages) = cached.value
            {
                return Ok(Some(history_message_window(&messages, limit, offset)));
            }
            let external = self.external_messages(&record).await;
            if external.is_empty() {
                return Ok(None);
            }
            let merge = if record.summary.provider == Provider::Claude {
                merge_mapped_claude_messages
            } else {
                merge_mapped_external_messages
            };
            let messages = Arc::new(merge(self.messages(session_id)?, external.as_ref().clone()));
            if self.storage.message_history_version(session_id)? != version
                || external_file_fingerprint(&record.file_path) != fingerprint
            {
                continue;
            }
            let page = history_message_window(&messages, limit, offset);
            if fingerprint.is_some() {
                self.remember_history_window(
                    key.clone(),
                    Some(version),
                    fingerprint,
                    HistoryWindowValue::Messages(messages),
                )
                .await;
            }
            return Ok(Some(page));
        }
        let Some(messages) = self.external_messages_for_session(session_id).await? else {
            return Ok(None);
        };
        Ok(Some(history_message_window(&messages, limit, offset)))
    }

    async fn stored_context_history(
        &self,
        session_id: &str,
        compacted_at: Option<DateTime<Utc>>,
        version: &MessageHistoryVersion,
    ) -> Result<Option<Arc<Vec<ActiveContextMessageDescriptor>>>> {
        let key = format!("stored:{session_id}:{compacted_at:?}");
        let cached = self.cached_history_window(&key).await;
        let descriptors = if let Some(cached) = cached
            && let Some(previous) = cached.version
            && previous.rewrite_revision == version.rewrite_revision
            && previous.revision <= version.revision
            && let HistoryWindowValue::Descriptors(mut descriptors) = cached.value
        {
            if previous == *version {
                return Ok(Some(descriptors));
            }
            let appended = self
                .storage
                .messages_after_history_version(session_id, &previous)?;
            for message in appended {
                if !is_persisted_codex_live_transcript_message(&message) {
                    Arc::make_mut(&mut descriptors).push(
                        ActiveContextMessageDescriptor::from_message(
                            ActiveContextMessageSource::Stored(message.id.clone()),
                            &message,
                        ),
                    );
                }
            }
            descriptors
        } else {
            let mut hidden = HashSet::new();
            self.storage
                .visit_assistant_messages_ordered(session_id, |message| {
                    if is_persisted_codex_live_transcript_message(&message) {
                        hidden.insert(message.id);
                    }
                })?;
            let mut descriptors = Vec::new();
            self.storage.visit_message_references_ordered(
                session_id,
                compacted_at,
                |reference| {
                    if !hidden.contains(&reference.id) {
                        descriptors.push(ActiveContextMessageDescriptor::from_stored_reference(
                            reference,
                        ));
                    }
                },
            )?;
            if let Some(compacted_at) = compacted_at {
                self.storage
                    .visit_messages_after(session_id, compacted_at, |message| {
                        if !hidden.contains(&message.id) {
                            descriptors.push(ActiveContextMessageDescriptor::from_message(
                                ActiveContextMessageSource::Stored(message.id.clone()),
                                &message,
                            ));
                        }
                    })?;
            }
            Arc::new(descriptors)
        };
        if self.storage.message_history_version(session_id)? != *version {
            return Ok(None);
        }
        self.remember_history_window(
            key,
            Some(version.clone()),
            None,
            HistoryWindowValue::Descriptors(descriptors.clone()),
        )
        .await;
        Ok(Some(descriptors))
    }
}

fn history_message_window(
    messages: &[ChatMessage],
    limit: usize,
    offset: Option<usize>,
) -> (Vec<ChatMessage>, usize) {
    let total = messages.len();
    let start = offset
        .unwrap_or_else(|| total.saturating_sub(limit))
        .min(total);
    let end = start.saturating_add(limit).min(total);
    (messages[start..end].to_vec(), total)
}

impl SessionManager {
    pub async fn update_model(
        &self,
        session_id: &str,
        model: Option<String>,
    ) -> Result<SessionSummary> {
        let mut sessions = self.sessions.write().await;
        let mut session = sessions
            .get(session_id)
            .cloned()
            .or_else(|| self.storage.get_session_summary(session_id).ok().flatten())
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
            .or_else(|| self.storage.get_session_summary(session_id).ok().flatten())
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?;

        session.title = title;
        session.title_source = Some(SessionTitleSource::Manual);
        session.last_activity = Utc::now();
        self.storage.upsert_session(&session)?;
        sessions.insert(session.id.clone(), session.clone());
        self.evict_if_needed(&mut sessions)?;
        Ok(session)
    }

    pub async fn delete(&self, session_id: &str) -> Result<SessionSummary> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        }
        .or_else(|| self.storage.get_session_summary(session_id).ok().flatten());
        let session = match session {
            Some(session) => session,
            None => self
                .external_record(session_id, None, None)
                .await
                .map(|record| record.summary)
                .ok_or_else(|| CoreError::SessionNotFound(session_id.to_string()))?,
        };
        for native_session_id in self.storage.context_native_session_ids(session_id)? {
            self.storage
                .tombstone_session(&native_session_id, session.provider)?;
        }
        if session.external {
            self.storage
                .tombstone_session(session_id, session.provider)?;
        } else if self.storage.is_session_fork_destination(session_id)? {
            if let Some(native_session_id) = session.native_session_id.as_deref() {
                self.storage
                    .tombstone_session(native_session_id, session.provider)?;
            }
        }
        if !self.storage.delete_session(session_id)? {
            if !session.external {
                return Err(CoreError::SessionNotFound(session_id.to_string()));
            }
        }
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        self.board_session_ids
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session_id);
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

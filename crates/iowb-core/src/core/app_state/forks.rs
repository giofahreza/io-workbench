impl AppState {
    async fn existing_fork_response(
        &self,
        user_id: &str,
        source_session_id: &str,
        before_message_id: &str,
        destination_session_id: &str,
        source_hidden: bool,
    ) -> Result<ForkSessionResponse> {
        let session = self.sessions.get(destination_session_id).await?;
        let draft = self
            .storage
            .get_session_draft(user_id, destination_session_id)?;
        Ok(ForkSessionResponse {
            source_session_id: source_session_id.to_string(),
            before_message_id: before_message_id.to_string(),
            native_forked: session.native_session_id.is_some(),
            files_unchanged: true,
            source_hidden,
            session,
            draft,
        })
    }

    fn resolve_codex_fork_boundary(
        &self,
        source_session_id: &str,
        target: &ChatMessage,
        messages: &[ChatMessage],
        snapshot: &CodexThreadSnapshot,
    ) -> Result<String> {
        if let Some(boundary) = target
            .metadata
            .get("nativeBeforeTurnId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return Ok(boundary.to_string());
        }
        if let Some(boundary) = self
            .storage
            .durable_chat_run_for_user_message(source_session_id, &target.id)?
            .and_then(|run| run.native_before_turn_id)
            .filter(|value| !value.trim().is_empty())
        {
            return Ok(boundary);
        }
        if let Some(native_message_id) = target
            .metadata
            .get("nativeMessageId")
            .and_then(Value::as_str)
            && let Some(turn_index) = snapshot.turns.iter().position(|turn| {
                turn.user_item_ids
                    .iter()
                    .any(|item_id| item_id == native_message_id)
            })
            && let Some(previous) = turn_index
                .checked_sub(1)
                .and_then(|index| snapshot.turns.get(index))
        {
            return Ok(previous.id.clone());
        }

        let local_users = messages
            .iter()
            .filter(|message| message.role == MessageRole::User)
            .collect::<Vec<_>>();
        let target_user_index = local_users
            .iter()
            .position(|message| message.id == target.id)
            .ok_or_else(|| {
                CoreError::InvalidInput(
                    "selected prompt was not present in user history".to_string(),
                )
            })?;
        let native_user_turns = snapshot
            .turns
            .iter()
            .enumerate()
            .filter(|(_, turn)| !turn.user_text.trim().is_empty())
            .collect::<Vec<_>>();
        let local_text = local_users
            .iter()
            .map(|message| normalized_fork_prompt(&message.content))
            .collect::<Vec<_>>();
        let native_text = native_user_turns
            .iter()
            .map(|(_, turn)| normalized_fork_prompt(&turn.user_text))
            .collect::<Vec<_>>();
        let matches = ordered_text_matches(&local_text, &native_text);
        let native_user_index = matches
            .iter()
            .find_map(|(local_index, native_index)| {
                (*local_index == target_user_index).then_some(*native_index)
            })
            .ok_or_else(|| {
                CoreError::Conflict(
                    "codex_turn_boundary_unresolved: refresh the session and try again".to_string(),
                )
            })?;
        let native_turn_index = native_user_turns[native_user_index].0;
        snapshot
            .turns
            .get(native_turn_index.saturating_sub(1))
            .filter(|_| native_turn_index > 0)
            .map(|turn| turn.id.clone())
            .ok_or_else(|| {
                CoreError::Conflict(
                    "codex_turn_boundary_unresolved: selected prompt has no prior native turn"
                        .to_string(),
                )
            })
    }

}

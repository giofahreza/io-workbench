fn estimate_external_messages_bytes(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .map(|message| {
            std::mem::size_of::<ChatMessage>()
                .saturating_add(message.id.len())
                .saturating_add(message.content.len())
                .saturating_add(estimate_json_value_bytes(&message.metadata))
        })
        .sum()
}

fn bounded_external_message_tail(
    messages: &[ChatMessage],
    max_messages: usize,
    max_bytes: usize,
) -> Vec<ChatMessage> {
    let mut tail = Vec::new();
    let mut estimated_bytes = 0usize;
    for message in messages.iter().rev().take(max_messages) {
        let message_bytes = estimate_external_messages_bytes(std::slice::from_ref(message));
        if !tail.is_empty() && estimated_bytes.saturating_add(message_bytes) > max_bytes {
            break;
        }
        estimated_bytes = estimated_bytes.saturating_add(message_bytes);
        tail.push(message.clone());
    }
    tail.reverse();
    tail
}

fn estimate_json_value_bytes(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => std::mem::size_of_val(value),
        Value::String(value) => value.len(),
        Value::Array(values) => values.iter().map(estimate_json_value_bytes).sum(),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| key.len().saturating_add(estimate_json_value_bytes(value)))
            .sum(),
    }
}

fn evict_external_message_cache(cache: &mut ExternalSessionCache) {
    while cache.messages.len() > EXTERNAL_MESSAGE_CACHE_MAX_ENTRIES
        || cache.message_bytes > EXTERNAL_MESSAGE_CACHE_MAX_BYTES
    {
        let Some(key) = cache
            .messages
            .iter()
            .min_by_key(|(_, cached)| cached.last_access)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        if let Some(removed) = cache.messages.remove(&key) {
            cache.message_bytes = cache.message_bytes.saturating_sub(removed.estimated_bytes);
        }
    }
}

fn merge_mapped_external_messages(
    stored: Vec<ChatMessage>,
    mut external: Vec<ChatMessage>,
) -> Vec<ChatMessage> {
    let mut matched_stored = vec![false; stored.len()];
    let stored_keys = stored.iter().map(message_match_key).collect::<Vec<_>>();
    let external_keys = external.iter().map(message_match_key).collect::<Vec<_>>();
    for (stored_index, external_index) in ordered_text_matches(&stored_keys, &external_keys) {
        let stored_message = &stored[stored_index];
        let external_message = &mut external[external_index];
        matched_stored[stored_index] = true;
        external_message.id = stored_message.id.clone();
        if let (Some(external_metadata), Some(stored_metadata)) = (
            external_message.metadata.as_object_mut(),
            stored_message.metadata.as_object(),
        ) {
            external_metadata.extend(stored_metadata.clone());
            external_metadata.insert("external".to_string(), Value::Bool(true));
        }
    }

    external.extend(
        stored
            .into_iter()
            .enumerate()
            .filter(|(index, message)| {
                !matched_stored[*index] && message.role == MessageRole::System
            })
            .map(|(_, message)| message),
    );
    external.sort_by(|left, right| left.timestamp.cmp(&right.timestamp));
    external
}

fn merge_active_context_external_messages(
    stored: Vec<ChatMessage>,
    external: Vec<ChatMessage>,
    compacted_at: Option<DateTime<Utc>>,
) -> Vec<ChatMessage> {
    let Some(compacted_at) = compacted_at else {
        return stored;
    };
    let mut matched_external = vec![false; external.len()];
    let stored_keys = stored.iter().map(message_match_key).collect::<Vec<_>>();
    let external_keys = external.iter().map(message_match_key).collect::<Vec<_>>();
    for (_, external_index) in ordered_text_matches(&stored_keys, &external_keys) {
        matched_external[external_index] = true;
    }

    let mut merged = stored;
    merged.extend(
        external
            .into_iter()
            .enumerate()
            .filter(|(index, message)| {
                !matched_external[*index]
                    && should_import_active_context_external_message(message, compacted_at)
            })
            .map(|(_, message)| message),
    );
    merged.sort_by(|left, right| left.timestamp.cmp(&right.timestamp));
    merged
}

fn latest_context_compaction_marker_timestamp(messages: &[ChatMessage]) -> Option<DateTime<Utc>> {
    messages
        .iter()
        .filter(|message| {
            message.metadata.get("kind").and_then(Value::as_str) == Some("context_compaction")
                || message.content.starts_with("Context compacted here")
        })
        .map(|message| message.timestamp)
        .max()
}

fn should_import_active_context_external_message(
    message: &ChatMessage,
    compacted_at: DateTime<Utc>,
) -> bool {
    if message.timestamp <= compacted_at || is_context_rollover_setup_message(message) {
        return false;
    }
    true
}

fn is_context_rollover_setup_message(message: &ChatMessage) -> bool {
    let content = message.content.trim();
    if content.eq_ignore_ascii_case("Context ready.")
        || content.eq_ignore_ascii_case("Context ready")
    {
        return true;
    }
    content.contains("visible io-workbench chat is being moved into a clean native Codex context")
        || content.contains("Recent text-only handoff:")
}

fn message_match_key(message: &ChatMessage) -> String {
    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    format!("{role}\0{}", message.content.trim())
}

fn clone_forked_message(source_session_id: &str, source: &ChatMessage) -> ChatMessage {
    let mut metadata = source.metadata.as_object().cloned().unwrap_or_default();
    let usage_source_session_id = metadata
        .get("usageSourceSessionId")
        .and_then(Value::as_str)
        .unwrap_or(source_session_id)
        .to_string();
    let usage_source_message_id = metadata
        .get("usageSourceMessageId")
        .and_then(Value::as_str)
        .unwrap_or(&source.id)
        .to_string();
    metadata.insert(
        "forkedFromSessionId".to_string(),
        Value::String(source_session_id.to_string()),
    );
    metadata.insert(
        "forkedFromMessageId".to_string(),
        Value::String(source.id.clone()),
    );
    metadata.insert(
        "usageSourceSessionId".to_string(),
        Value::String(usage_source_session_id),
    );
    metadata.insert(
        "usageSourceMessageId".to_string(),
        Value::String(usage_source_message_id),
    );
    ChatMessage {
        id: new_id("msg"),
        role: source.role,
        content: source.content.clone(),
        timestamp: source.timestamp,
        metadata: Value::Object(metadata),
    }
}

fn normalized_fork_prompt(value: &str) -> String {
    let visible = visible_user_text(value);
    let selected = if visible.trim().is_empty() {
        value.trim()
    } else {
        visible.trim()
    };
    selected.replace("\r\n", "\n")
}

fn build_context_rollover_handoff(messages: Vec<ChatMessage>, failed_prompt: &str) -> String {
    let history = build_context_handoff_history(messages, Some(failed_prompt));
    format!(
        "<system-reminder>\nThe visible io-workbench chat is being moved into a clean native Codex context because its previous history exceeded the gateway body-size limit. The same Workbench chat and full visible transcript remain available to the user. Use the bounded text handoff below only to re-establish context. Do not answer the failed user request yet; a subsequent message will contain that request after this clean context is activated. Do not claim that old tool outputs or inline image bytes are present. Reopen only specific local image paths when genuinely needed, one at a time. Inspect current files before changing them and preserve existing unrelated work. Reply exactly: Context ready.\n\nRecent text-only handoff:\n{history}\n</system-reminder>"
    )
}

fn sanitize_context_materialization_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    messages
        .into_iter()
        .filter(|message| !is_persisted_codex_live_transcript_message(message))
        .collect()
}

fn context_materialization_messages(
    session_id: &str,
    messages: Vec<ChatMessage>,
    preserved_message_ids: &[&str],
) -> Vec<ChatMessage> {
    let preserved_message_ids = preserved_message_ids
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let mut used_message_ids = HashSet::new();
    sanitize_context_materialization_messages(messages)
        .into_iter()
        .map(|message| {
            if preserved_message_ids.contains(message.id.as_str())
                && used_message_ids.insert(message.id.clone())
            {
                return message;
            }
            clone_context_materialized_message(session_id, &message, &mut used_message_ids)
        })
        .collect()
}

fn clone_context_materialized_message(
    session_id: &str,
    source: &ChatMessage,
    used_message_ids: &mut HashSet<String>,
) -> ChatMessage {
    let source_message_id = source.id.clone();
    let mut metadata = source.metadata.as_object().cloned().unwrap_or_default();
    let usage_source_session_id = metadata
        .get("usageSourceSessionId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(session_id)
        .to_string();
    let usage_source_message_id = metadata
        .get("usageSourceMessageId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&source_message_id)
        .to_string();
    metadata.insert(
        "contextMaterializedFromSessionId".to_string(),
        Value::String(session_id.to_string()),
    );
    metadata.insert(
        "contextMaterializedFromMessageId".to_string(),
        Value::String(source_message_id),
    );
    metadata.insert(
        "usageSourceSessionId".to_string(),
        Value::String(usage_source_session_id),
    );
    metadata.insert(
        "usageSourceMessageId".to_string(),
        Value::String(usage_source_message_id),
    );

    let mut id = new_id("msg");
    while !used_message_ids.insert(id.clone()) {
        id = new_id("msg");
    }
    ChatMessage {
        id,
        role: source.role,
        content: source.content.clone(),
        timestamp: source.timestamp,
        metadata: Value::Object(metadata),
    }
}

fn is_persisted_codex_live_transcript_message(message: &ChatMessage) -> bool {
    if message.role != MessageRole::Assistant || !looks_like_codex_live_transcript(&message.content)
    {
        return false;
    }
    let providerish = message
        .metadata
        .get("provider")
        .or_else(|| message.metadata.get("cli"))
        .and_then(Value::as_str)
        .is_some_and(|provider| provider.eq_ignore_ascii_case("codex"));
    providerish
        || message.metadata.get("durableRunId").is_some()
        || message
            .metadata
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|source| source.eq_ignore_ascii_case("io-workbench"))
        || message.metadata.is_null()
}

fn build_context_handoff_history(
    messages: Vec<ChatMessage>,
    excluded_user_prompt: Option<&str>,
) -> String {
    let excluded_user_prompt = excluded_user_prompt.map(sanitize_context_handoff_text);
    let mut selected = Vec::<String>::new();
    let mut remaining = CONTEXT_ROLLOVER_HANDOFF_MAX_BYTES;
    for message in messages.into_iter().rev() {
        if !matches!(message.role, MessageRole::User | MessageRole::Assistant) {
            continue;
        }
        if message.role == MessageRole::Assistant
            && (message.metadata.get("kind").and_then(Value::as_str) == Some("thinking")
                || message.metadata.get("phase").and_then(Value::as_str) == Some("commentary"))
        {
            continue;
        }
        if message.role == MessageRole::User && message.id.is_empty() {
            continue;
        }
        let content = sanitize_context_handoff_text(&message.content);
        if content.is_empty()
            || (message.role == MessageRole::User
                && excluded_user_prompt.as_deref() == Some(content.as_str()))
        {
            continue;
        }
        let role = if message.role == MessageRole::User {
            "User"
        } else {
            "Assistant"
        };
        let entry = format!("{role}: {content}");
        if entry.len() + 2 > remaining {
            continue;
        }
        remaining -= entry.len() + 2;
        selected.push(entry);
        if selected.len() >= 24 {
            break;
        }
    }
    selected.reverse();
    if selected.is_empty() {
        "No earlier text messages were retained.".to_string()
    } else {
        selected.join("\n\n")
    }
}

fn context_handoff_has_retainable_text(messages: &[ChatMessage]) -> bool {
    messages.iter().any(|message| {
        if !matches!(message.role, MessageRole::User | MessageRole::Assistant) {
            return false;
        }
        if message.role == MessageRole::Assistant
            && (message.metadata.get("kind").and_then(Value::as_str) == Some("thinking")
                || message.metadata.get("phase").and_then(Value::as_str) == Some("commentary"))
        {
            return false;
        }
        !sanitize_context_handoff_text(&message.content).is_empty()
    })
}

fn sanitize_context_handoff_text(value: &str) -> String {
    let visible = normalized_fork_prompt(value);
    let mut output = String::with_capacity(visible.len().min(8 * 1024));
    let mut cursor = 0;
    while let Some(relative_start) = visible[cursor..].find("data:") {
        let start = cursor + relative_start;
        output.push_str(&visible[cursor..start]);
        let tail = &visible[start..];
        let Some(marker) = tail.find(";base64,") else {
            output.push_str("data:");
            cursor = start + "data:".len();
            continue;
        };
        let payload_start = start + marker + ";base64,".len();
        let payload_len = visible.as_bytes()[payload_start..]
            .iter()
            .take_while(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'+' | b'/' | b'=' | b'-' | b'_' | b'\r' | b'\n')
            })
            .count();
        if payload_len == 0 {
            output.push_str("[inline attachment omitted]");
            cursor = payload_start;
        } else {
            output.push_str("[inline image omitted; use its local file path if available]");
            cursor = payload_start + payload_len;
        }
    }
    output.push_str(&visible[cursor..]);
    bound_agent_text(&output, 8 * 1024, "handoff message")
        .trim()
        .to_string()
}

fn ordered_text_matches(left: &[String], right: &[String]) -> Vec<(usize, usize)> {
    if left.len().saturating_mul(right.len()) > ORDERED_TEXT_MATCH_MATRIX_MAX_CELLS {
        return ordered_text_matches_greedy(left, right);
    }
    let mut lengths = vec![vec![0usize; right.len() + 1]; left.len() + 1];
    for left_index in (0..left.len()).rev() {
        for right_index in (0..right.len()).rev() {
            lengths[left_index][right_index] = if left[left_index] == right[right_index] {
                lengths[left_index + 1][right_index + 1] + 1
            } else {
                lengths[left_index + 1][right_index].max(lengths[left_index][right_index + 1])
            };
        }
    }

    let mut matches = Vec::new();
    let (mut left_index, mut right_index) = (0usize, 0usize);
    while left_index < left.len() && right_index < right.len() {
        if left[left_index] == right[right_index] {
            matches.push((left_index, right_index));
            left_index += 1;
            right_index += 1;
        } else if lengths[left_index + 1][right_index] >= lengths[left_index][right_index + 1] {
            left_index += 1;
        } else {
            right_index += 1;
        }
    }
    matches
}

fn ordered_text_matches_greedy(left: &[String], right: &[String]) -> Vec<(usize, usize)> {
    let mut positions = HashMap::<&str, VecDeque<usize>>::new();
    for (index, value) in right.iter().enumerate() {
        positions
            .entry(value.as_str())
            .or_default()
            .push_back(index);
    }
    let mut next_right_index = 0usize;
    let mut matches = Vec::new();
    for (left_index, value) in left.iter().enumerate() {
        let Some(indices) = positions.get_mut(value.as_str()) else {
            continue;
        };
        while indices
            .front()
            .is_some_and(|index| *index < next_right_index)
        {
            indices.pop_front();
        }
        if let Some(right_index) = indices.pop_front() {
            matches.push((left_index, right_index));
            next_right_index = right_index.saturating_add(1);
        }
    }
    matches
}

fn ws_event_estimated_bytes(event: &WsServerEvent) -> usize {
    const ENVELOPE_BYTES: usize = 256;
    match event {
        WsServerEvent::Output { content, .. } => ENVELOPE_BYTES.saturating_add(content.len()),
        WsServerEvent::Error {
            message, details, ..
        } => ENVELOPE_BYTES
            .saturating_add(message.len())
            .saturating_add(details.as_deref().map_or(0, str::len)),
        WsServerEvent::SessionStatus {
            latest_user_prompt, ..
        } => ENVELOPE_BYTES.saturating_add(latest_user_prompt.as_deref().map_or(0, str::len)),
        WsServerEvent::ProjectFilesChanged { paths, .. } => {
            ENVELOPE_BYTES.saturating_add(paths.iter().map(String::len).sum::<usize>())
        }
        _ => 1024,
    }
}

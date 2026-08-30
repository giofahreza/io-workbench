fn parse_thread_snapshot(result: &Value) -> Result<CodexThreadSnapshot> {
    let thread = result.get("thread").ok_or_else(|| {
        CoreError::InvalidInput("Codex thread/read response omitted thread".to_string())
    })?;
    let id = thread
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            CoreError::InvalidInput("Codex thread/read response omitted thread id".to_string())
        })?;
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_thread_turn)
        .collect();
    Ok(CodexThreadSnapshot { id, turns })
}

fn parse_thread_turn(value: &Value) -> Option<CodexThreadTurn> {
    let id = value.get("id")?.as_str()?.trim();
    if id.is_empty() {
        return None;
    }
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed")
        .to_string();
    let mut user_item_ids = Vec::new();
    let mut user_texts = Vec::new();
    for item in value
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("userMessage"))
    {
        if let Some(item_id) = item.get("id").and_then(Value::as_str) {
            user_item_ids.push(item_id.to_string());
        }
        let text = item
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|content| content.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|content| content.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            user_texts.push(text);
        }
    }
    Some(CodexThreadTurn {
        id: id.to_string(),
        status,
        user_item_ids,
        user_text: user_texts.join("\n"),
    })
}

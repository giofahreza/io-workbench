fn long_single_line_agent_result() -> String {
    serde_json::json!({
        "summary": "Generated task plan",
        "tasks": [{
            "title": "Build a small web app",
            "description": "x".repeat(9_300),
            "acceptanceCriteria": ["Render titles as text", "Preserve café and 日本語"]
        }]
    })
    .to_string()
}

#[test]
fn canonical_final_output_preserves_json_and_exact_text_below_retention_cap() {
    let json = long_single_line_agent_result();
    assert!(json.len() > AGENT_DISPLAY_MAX_LINE_CHARS);
    assert!(json.len() < AGENT_ASSISTANT_MESSAGE_MAX_BYTES);
    assert_eq!(bound_agent_final_text(&json), json);
    assert!(serde_json::from_str::<Value>(&bound_agent_final_text(&json)).is_ok());

    let exact = format!(" \r\n{json}\n\t\0\u{001b}");
    assert_eq!(bound_agent_final_text(&exact), exact);

    // This is deliberately display-only: injecting the line-wrap marker
    // into a JSON string makes the displayed copy unsuitable for parsing.
    let displayed = bound_agent_text(&json, AGENT_LIVE_EVENT_MAX_BYTES, "agent event");
    assert!(displayed.contains("[long line wrapped for display]"));
    assert!(serde_json::from_str::<Value>(&displayed).is_err());
}

#[test]
fn canonical_final_output_keeps_existing_utf8_safe_retention_cap() {
    let exact_limit = "x".repeat(AGENT_ASSISTANT_MESSAGE_MAX_BYTES);
    assert_eq!(bound_agent_final_text(&exact_limit), exact_limit);
    let oversized = format!("HEAD{}TAIL", "界".repeat(AGENT_ASSISTANT_MESSAGE_MAX_BYTES));
    let bounded = bound_agent_final_text(&oversized);
    assert!(bounded.len() <= AGENT_ASSISTANT_MESSAGE_MAX_BYTES);
    assert!(bounded.starts_with("HEAD"));
    assert!(bounded.ends_with("TAIL"));
    assert!(bounded.contains("[truncated assistant response:"));
    assert!(!bounded.contains("[long line wrapped for display]"));
    assert_eq!(bound_agent_bytes("abc", 0, "test"), "");
    assert!(bound_agent_bytes(&oversized, 12, "test").len() <= 12);

    let display = bound_agent_text(&oversized, AGENT_TOOL_MESSAGE_MAX_BYTES, "tool output");
    assert!(display.len() <= AGENT_TOOL_MESSAGE_MAX_BYTES);
    assert!(display.contains("[truncated tool output:"));
    assert!(
        display
            .lines()
            .all(|line| line.chars().count() <= AGENT_DISPLAY_MAX_LINE_CHARS + 80)
    );
}

#[test]
fn canonical_final_output_codex_exec_preserves_phased_and_unphased_json() {
    let raw = format!(" \r\n{}\n\t", long_single_line_agent_result());
    for phase in [Some("final_answer"), None] {
        let mut normalizer = CodexLiveOutputNormalizer::default();
        let event = serde_json::json!({
            "type": "item.completed",
            "item": {"type": "agent_message", "phase": phase, "text": raw}
        });
        normalizer.push(&format!("{event}\n"));
        normalizer.push("{\"type\":\"turn.completed\"}\n");
        let final_text = normalizer.take_final_assistant_message().expect("final");
        assert_eq!(final_text, raw, "phase {phase:?}");
        assert!(serde_json::from_str::<Value>(&final_text).is_ok());
    }
}

#[test]
fn canonical_final_output_codex_app_server_preserves_completed_and_review_json() {
    let raw = format!(" \r\n{}\n\t", long_single_line_agent_result());
    for item in [
        serde_json::json!({"type": "agentMessage", "phase": "final_answer", "text": raw}),
        serde_json::json!({"type": "agentMessage", "text": raw}),
        serde_json::json!({"type": "exitedReviewMode", "review": raw}),
    ] {
        let mut normalizer = CodexAppServerLiveOutputNormalizer::default();
        normalizer.push_notification("item/completed", &serde_json::json!({"item": item}));
        normalizer.finish();
        let final_text = normalizer.take_final_assistant_message().expect("final");
        assert_eq!(final_text, raw);
        assert!(serde_json::from_str::<Value>(&final_text).is_ok());
    }
}

#[test]
fn canonical_final_output_codex_app_server_preserves_streamed_json() {
    let raw = format!(" \r\n{}\n\t", long_single_line_agent_result());
    let mut normalizer = CodexAppServerLiveOutputNormalizer::default();
    for chunk in [&raw[..4_000], &raw[4_000..]] {
        normalizer.push_notification(
            "item/agentMessage/delta",
            &serde_json::json!({"itemId": "final-json", "delta": chunk}),
        );
    }
    assert_eq!(
        normalizer.take_final_assistant_message().as_deref(),
        Some(raw.as_str())
    );
    normalizer.push_notification(
        "item/completed",
        &serde_json::json!({"item": {
            "id": "final-json", "type": "agentMessage", "phase": "final_answer", "text": raw
        }}),
    );
    assert_eq!(
        normalizer.take_final_assistant_message().as_deref(),
        Some(raw.as_str())
    );
}

#[test]
fn canonical_final_output_claude_preserves_result_stream_and_assistant_json() {
    let raw = format!(" \r\n{}\n\t", long_single_line_agent_result());
    for event in [
        serde_json::json!({"type": "result", "subtype": "success", "result": raw}),
        serde_json::json!({"type": "assistant", "message": {
            "content": [{"type": "text", "text": raw}]
        }}),
        serde_json::json!({"type": "stream_event", "event": {
            "type": "content_block_delta", "delta": {"type": "text_delta", "text": raw}
        }}),
    ] {
        let mut normalizer = ClaudeLiveOutputNormalizer::default();
        normalizer.push_chunks(&format!("{event}\n"));
        let final_text = normalizer.take_final_assistant_message().expect("final");
        assert_eq!(final_text, raw);
        assert!(serde_json::from_str::<Value>(&final_text).is_ok());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn canonical_final_output_persists_json_while_live_display_remains_sanitized() {
    let (mut state, root, project) = temporary_app_state("canonical-final-json").await;
    state.sessions.external_home = Arc::new(root.clone());
    let session = state
        .sessions
        .create_or_update(
            Provider::Codex,
            project.display().to_string(),
            None,
            false,
            None,
            Some(ChatRuntime::NativeCli),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("session");
    let raw = format!(" \r\n{}\n\t", long_single_line_agent_result());
    let mut normalizer = CodexLiveOutputNormalizer::default();
    let visible = normalizer.push(&format!(
        "{}\n",
        serde_json::json!({
            "type": "item.completed", "item": {
                "type": "agent_message", "phase": "final_answer", "text": raw
            }
        })
    ));
    let canonical = normalizer
        .take_final_assistant_message()
        .expect("canonical final");
    let context = AgentStartContext {
        request_id: None,
        provider: Provider::Codex,
        session_id: session.id.clone(),
        durable_run_id: None,
        attempt_id: None,
        response_id: "canonical-final-json".to_string(),
        sequence: Arc::new(AtomicU64::new(0)),
        project_path: project,
        prompt: "Generate a task plan".to_string(),
        model: None,
        runtime: ChatRuntime::NativeCli,
        effort: None,
        mode: None,
        thinking: None,
        fast: None,
        native_resume_session_id: None,
        native_rollout_owned_by_provider: true,
        context_rollover_id: None,
        direct_ai_config: None,
        direct_ai_messages: Vec::new(),
        sessions: state.sessions.clone(),
        storage: state.storage.clone(),
        hub: WsHub::new(),
    };
    let manager = AgentRuntimeManager::default();
    let mut live_events = context.hub.subscribe();
    let mut displayed = String::new();
    publish_agent_output(
        &manager,
        &context,
        "canonical-final-json",
        &mut displayed,
        visible,
    )
    .await;
    assert!(displayed.contains("[long line wrapped for display]"));
    assert!(displayed.len() <= AGENT_LIVE_EVENT_MAX_BYTES);
    assert!(matches!(
        live_events.try_recv(),
        Ok(WsServerEvent::Output { .. })
    ));
    manager
        .finish(
            "canonical-final-json",
            &context,
            iowb_protocol::SessionRuntimeStatus::Completed,
            Some(canonical),
            None,
        )
        .await;
    let messages = state
        .storage
        .list_messages(&session.id)
        .expect("persisted messages");
    let final_message = messages
        .iter()
        .find(|message| message.role == MessageRole::Assistant)
        .expect("persisted final assistant message");
    assert_eq!(final_message.content, raw);
    let parsed: Value = serde_json::from_str(&final_message.content).expect("valid persisted JSON");
    assert_eq!(
        parsed["tasks"][0]["description"].as_str().unwrap().len(),
        9_300
    );
    std::fs::remove_dir_all(root).expect("temporary fixture cleanup");
}

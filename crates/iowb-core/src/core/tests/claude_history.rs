fn claude_history_events(streamed: bool, final_text: &str) -> String {
    let progress = serde_json::json!({"type": "assistant", "message": {
        "content": [{"type": "text", "text": "Checking the fixture."}]
    }});
    let tool = serde_json::json!({"type": "tool_use", "id": "history-call", "name": "Bash", "input": {"command": "cat fixture.txt"}});
    let result = serde_json::json!({"type": "tool_result", "tool_use_id": "history-call", "content": "HISTORY_FIXTURE"});
    let mut events = Vec::new();
    if streamed {
        for text in ["Checking the ", "fixture."] {
            events.push(serde_json::json!({"type": "stream_event", "event": {
                "type": "content_block_delta", "delta": {"type": "text_delta", "text": text}
            }}));
        }
        events.push(
            serde_json::json!({"type": "stream_event", "event": {"type": "content_block_stop"}}),
        );
        events.push(progress);
        events.push(serde_json::json!({"type": "stream_event", "event": {"type": "content_block_start", "content_block": tool}}));
        events.push(
            serde_json::json!({"type": "stream_event", "event": {"type": "content_block_stop"}}),
        );
    } else {
        events.push(progress.clone());
        events.push(progress);
    }
    events.push(serde_json::json!({"type": "assistant", "message": {"content": [tool]}}));
    events.push(serde_json::json!({"type": "user", "message": {"content": [result.clone()]}}));
    events.push(result.clone());
    events.push(serde_json::json!({"type": "user", "message": {"content": [result]}}));
    if streamed {
        events
            .push(serde_json::json!({"type": "stream_event", "event": {"type": "message_start"}}));
        events.push(serde_json::json!({"type": "stream_event", "event": {
            "type": "content_block_delta", "delta": {"type": "text_delta", "text": final_text}
        }}));
        events.push(
            serde_json::json!({"type": "stream_event", "event": {"type": "content_block_stop"}}),
        );
    }
    events.push(
        serde_json::json!({"type": "assistant", "message": {"content": [
            {"type": "thinking", "thinking": "PRIVATE_THINKING_NOT_HISTORY"},
            {"type": "text", "text": final_text}
        ]}}),
    );
    events.push(serde_json::json!({"type": "result", "result": final_text}));
    events
        .into_iter()
        .map(|event| format!("{event}\n"))
        .collect()
}

#[test]
fn claude_history_deduplicates_streamed_and_full_events_without_polluting_final_json() {
    let final_text = format!(" \n{}\n\t", long_single_line_agent_result());
    for streamed in [false, true] {
        let mut normalizer = ClaudeLiveOutputNormalizer::default();
        let events = claude_history_events(streamed, &final_text);
        let mut remaining = events.as_str();
        while !remaining.is_empty() {
            let boundary = utf8_prefix_boundary(remaining, 127);
            normalizer.push_chunks(&remaining[..boundary]);
            remaining = &remaining[boundary..];
        }
        normalizer.finish();
        let canonical = normalizer.take_final_assistant_message().expect("final");
        assert_eq!(canonical, final_text);
        let messages = normalizer.take_history_messages(Some(&canonical));
        assert_eq!(messages.len(), 3, "streamed={streamed}");
        assert_eq!(messages[0].content, "Checking the fixture.");
        assert_eq!(messages[0].role, MessageRole::Assistant);
        assert_eq!(messages[1].kind, "tool_call");
        assert!(messages[1].content.contains("cat fixture.txt"));
        assert_eq!(messages[2].kind, "tool_result");
        assert!(messages[2].content.contains("HISTORY_FIXTURE"));
        assert!(
            messages
                .iter()
                .all(|message| !message.content.contains("PRIVATE_THINKING"))
        );
        assert!(
            normalizer
                .take_history_messages(Some(&canonical))
                .is_empty()
        );
    }
}

#[test]
fn claude_history_keeps_multiblock_final_text_out_of_progress() {
    let mut normalizer = ClaudeLiveOutputNormalizer::default();
    for text in ["First final paragraph", "Second final paragraph"] {
        normalizer.normalize_line(
            &serde_json::json!({"type": "stream_event", "event": {
                "type": "content_block_start", "content_block": {"type": "text", "text": ""}
            }})
            .to_string(),
        );
        normalizer.normalize_line(
            &serde_json::json!({"type": "stream_event", "event": {
                "type": "content_block_delta", "delta": {"type": "text_delta", "text": text}
            }})
            .to_string(),
        );
        normalizer.normalize_line(
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_stop\"}}",
        );
    }
    let canonical = "First final paragraph\nSecond final paragraph";
    normalizer
        .normalize_line(&serde_json::json!({"type": "result", "result": canonical}).to_string());
    assert!(normalizer.take_history_messages(Some(canonical)).is_empty());
    assert_eq!(
        normalizer.take_final_assistant_message().as_deref(),
        Some(canonical)
    );
}

#[test]
fn claude_history_limits_count_and_bytes_and_does_not_duplicate_large_final_answers() {
    let final_text = "final".repeat(40_000);
    let mut normalizer = ClaudeLiveOutputNormalizer::default();
    normalizer.push_chunks(&claude_history_events(true, &final_text));
    assert_eq!(normalizer.take_history_messages(Some(&final_text)).len(), 3);
    for index in 0..200 {
        normalizer.normalize_line(&serde_json::json!({"type": "tool_result", "tool_use_id": format!("call-{index}"), "content": "界".repeat(50_000)}).to_string());
    }
    let history = normalizer.take_history_messages(None);
    assert!(history.len() <= AGENT_TOOL_MESSAGES_MAX_COUNT);
    assert!(
        history
            .iter()
            .all(|message| message.content.len() <= AGENT_TOOL_MESSAGE_MAX_BYTES)
    );
    assert!(
        history
            .iter()
            .map(|message| message.content.len())
            .sum::<usize>()
            <= AGENT_TOOL_MESSAGES_MAX_TOTAL_BYTES
    );
    assert!(normalizer.recorded_activity.len() <= AGENT_TOOL_MESSAGES_MAX_COUNT);
}

#[tokio::test(flavor = "current_thread")]
async fn claude_history_is_persisted_before_terminal_status_and_survives_restart() {
    for status in [
        iowb_protocol::SessionRuntimeStatus::Completed,
        iowb_protocol::SessionRuntimeStatus::Failed,
        iowb_protocol::SessionRuntimeStatus::Aborted,
    ] {
        let (mut state, root, project) = temporary_app_state("claude-history-runtime").await;
        state.sessions.external_home = Arc::new(root.clone());
        let session = state
            .sessions
            .create_or_update(
                Provider::Claude,
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
        state
            .sessions
            .append_message(&session.id, MessageRole::User, "Inspect the fixture")
            .await
            .expect("prompt");
        let mut context = AgentStartContext {
            provider: Provider::Claude,
            session_id: session.id.clone(),
            durable_run_id: None,
            attempt_id: None,
            response_id: "claude-history-response".to_string(),
            sequence: Arc::new(AtomicU64::new(0)),
            project_path: project,
            prompt: "Inspect the fixture".to_string(),
            model: None,
            runtime: ChatRuntime::NativeCli,
            effort: None,
            mode: None,
            thinking: None,
            fast: None,
            native_resume_session_id: None,
            native_rollout_owned_by_provider: false,
            context_rollover_id: None,
            direct_ai_config: None,
            direct_ai_messages: Vec::new(),
            sessions: state.sessions.clone(),
            storage: state.storage.clone(),
            hub: WsHub::new(),
        };
        let mut normalizer = Some(ClaudeLiveOutputNormalizer::default());
        normalizer
            .as_mut()
            .unwrap()
            .push_chunks(&claude_history_events(true, "Finished."));
        let canonical = normalizer
            .as_mut()
            .unwrap()
            .take_final_assistant_message()
            .unwrap();
        context.context_rollover_id = Some("provisional".to_string());
        persist_claude_history_messages(&context, &mut normalizer, Some(&canonical)).await;
        assert_eq!(state.sessions.messages(&session.id).unwrap().len(), 1);
        context.context_rollover_id = None;
        normalizer = Some(ClaudeLiveOutputNormalizer::default());
        normalizer
            .as_mut()
            .unwrap()
            .push_chunks(&claude_history_events(true, "Finished."));
        persist_claude_history_messages(&context, &mut normalizer, Some(&canonical)).await;
        assert_eq!(state.sessions.messages(&session.id).unwrap().len(), 4);
        let mut events = context.hub.subscribe();
        AgentRuntimeManager::default()
            .finish("claude-history", &context, status, Some(canonical), None)
            .await;
        let messages = state.sessions.messages(&session.id).unwrap();
        assert_eq!(messages.len(), 5);
        assert_eq!(messages.last().unwrap().content, "Finished.");
        assert!(events.try_recv().is_ok());
        let mut restored = SessionManager::load(state.storage.clone(), 10).unwrap();
        restored.external_home = Arc::new(root.clone());
        let (history, total) = restored
            .messages_tail_including_external(&session.id, 30)
            .await
            .unwrap();
        assert_eq!(total, 5);
        assert_eq!(
            history
                .iter()
                .map(|message| &message.id)
                .collect::<Vec<_>>(),
            messages
                .iter()
                .map(|message| &message.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            history
                .iter()
                .filter(|message| message.role == MessageRole::Tool)
                .count(),
            2
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn claude_history_backfills_mapped_native_tools_without_dropping_stored_history() {
    let (mut state, root, project) = temporary_app_state("claude-history-backfill").await;
    state.sessions.external_home = Arc::new(root.clone());
    let session = state
        .sessions
        .create_or_update(
            Provider::Claude,
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
        .unwrap();
    let native_id = "11111111-1111-4111-8111-111111111111";
    let native_file = root
        .join(".claude/projects/fixture")
        .join(format!("{native_id}.jsonl"));
    std::fs::create_dir_all(native_file.parent().unwrap()).unwrap();
    let now = Utc::now();
    let entries = [
        serde_json::json!({"role": "user", "content": "Inspect the fixture"}),
        serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "Checking the fixture."}]}),
        serde_json::json!({"role": "assistant", "content": [{"type": "tool_use", "id": "history-call", "name": "Bash", "input": {"command": "cat fixture.txt"}}]}),
        serde_json::json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "history-call", "content": "HISTORY_FIXTURE"}]}),
        serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "Finished."}]}),
    ];
    let lines = entries.iter().enumerate().map(|(index, message)| format!("{}\n", serde_json::json!({"type": message["role"], "sessionId": native_id, "cwd": project, "uuid": format!("entry-{index}"), "timestamp": now + chrono::Duration::seconds(index as i64), "message": message}))).collect::<String>();
    let api_blob = serde_json::json!({"type": "api-request-blob", "sessionId": native_id,
        "message": {"role": "user", "content": [{"type": "text", "text": "Inspect the fixture"}]}});
    std::fs::write(&native_file, format!("{lines}{lines}{api_blob}\n")).unwrap();
    for (role, content) in [
        (MessageRole::User, "Earlier stored prompt"),
        (MessageRole::Assistant, "Earlier stored answer"),
        (MessageRole::User, "Inspect the fixture"),
        (MessageRole::Assistant, " \nFinished.\n"),
    ] {
        state
            .sessions
            .append_message(&session.id, role, content)
            .await
            .unwrap();
    }
    state
        .sessions
        .set_native_session_id(&session.id, native_id.to_string())
        .await
        .unwrap();
    state.sessions.set_active(&session.id, false).await.unwrap();
    let listed = state
        .sessions
        .list_for_project(project.to_str().unwrap())
        .await
        .unwrap();
    assert!(listed.iter().any(|item| item.id == session.id));
    assert!(
        state
            .sessions
            .external_cache
            .read()
            .await
            .messages
            .is_empty()
    );
    let (first, total) = state
        .sessions
        .messages_tail_including_external(&session.id, 30)
        .await
        .unwrap();
    assert_eq!(total, 7);
    assert_eq!(first[0].content, "Earlier stored prompt");
    assert_eq!(first[1].content, "Earlier stored answer");
    assert_eq!(first.last().unwrap().content, " \nFinished.\n");
    assert_eq!(
        state.sessions.get(&session.id).await.unwrap().message_count,
        7
    );
    assert_eq!(
        first
            .iter()
            .filter(|message| message.role == MessageRole::Tool)
            .count(),
        2
    );
    let mut restored = SessionManager::load(state.storage.clone(), 10).unwrap();
    restored.external_home = Arc::new(root.clone());
    let (tail, count) = restored
        .messages_tail_including_external(&session.id, 3)
        .await
        .unwrap();
    assert_eq!(count, 7);
    assert_eq!(
        tail.iter().map(|message| &message.id).collect::<Vec<_>>(),
        first[4..]
            .iter()
            .map(|message| &message.id)
            .collect::<Vec<_>>()
    );
    let (page, count) = restored
        .messages_page_including_external(&session.id, 4, 0)
        .await
        .unwrap();
    assert_eq!(count, 7);
    assert_eq!(
        page.iter().map(|message| &message.id).collect::<Vec<_>>(),
        first[..4]
            .iter()
            .map(|message| &message.id)
            .collect::<Vec<_>>()
    );
    std::fs::remove_dir_all(root).unwrap();
}

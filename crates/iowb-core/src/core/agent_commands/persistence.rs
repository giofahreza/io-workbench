async fn publish_agent_output(
    manager: &AgentRuntimeManager,
    context: &AgentStartContext,
    key: &str,
    output: &mut String,
    content: String,
) {
    if content.is_empty() {
        return;
    }
    let original_bytes = content.len();
    let content = bound_agent_text(&content, AGENT_LIVE_EVENT_MAX_BYTES, "agent event");
    if content.len() < original_bytes {
        warn!(
            provider = context.provider.as_str(),
            session_id = %context.session_id,
            original_bytes,
            published_bytes = content.len(),
            "truncated oversized agent output event"
        );
    }
    append_bounded(output, &content, manager.max_output_bytes);
    for chunk in websocket_text_chunks(&content) {
        manager
            .publish(
                &context.hub,
                key,
                WsServerEvent::Output {
                    provider: context.provider,
                    session_id: context.session_id.clone(),
                    content: chunk,
                    done: false,
                    response_id: Some(context.response_id.clone()),
                    sequence: Some(context.next_sequence()),
                },
            )
            .await;
    }
}

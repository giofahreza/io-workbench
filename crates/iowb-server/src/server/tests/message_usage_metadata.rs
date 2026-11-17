#[tokio::test(flavor = "current_thread")]
async fn token_usage_snapshot_preserves_durable_reply_usage() {
    let (root, state) = board_scope_test_state().await;
    let session = SessionSummary {
        id: "usage-snapshot-session".to_string(),
        provider: Provider::Codex,
        project_path: root.display().to_string(),
        token_usage: Some(SessionTokenUsage {
            used: 184791,
            ..Default::default()
        }),
        ..Default::default()
    };
    state.storage.upsert_session(&session).unwrap();
    for (id, metadata, expected) in [
        ("legacy", serde_json::json!({}), Some(184791)),
        (
            "failed",
            serde_json::json!({"durableRunId": "run-failed", "status": "failed", "tokenUsage": null}),
            None,
        ),
        (
            "completed",
            serde_json::json!({"durableRunId": "run-completed", "status": "completed", "tokenUsage": {"used": 123}}),
            Some(123),
        ),
    ] {
        state
            .storage
            .append_message(
                &session.id,
                &ChatMessage {
                    id: id.to_string(),
                    role: MessageRole::Assistant,
                    content: id.to_string(),
                    timestamp: Utc::now(),
                    metadata,
                },
            )
            .unwrap();
        let snapshot = session_token_usage(
            State(state.clone()),
            AxumPath(("test".to_string(), session.id.clone())),
            Query(TokenUsageQuery { provider: None }),
        )
        .await
        .unwrap();
        assert_eq!(snapshot.0["used"], 184791);
        let message = state
            .storage
            .message_by_id(&session.id, id)
            .unwrap()
            .unwrap();
        assert_eq!(
            message.metadata["tokenUsage"]["used"].as_u64(),
            expected,
            "{id}"
        );
    }
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

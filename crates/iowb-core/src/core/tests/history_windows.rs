#[tokio::test(flavor = "current_thread")]
async fn history_descriptors_reuse_warm_cache_and_invalidate_rewrites() {
    let (state, root, project) = temporary_app_state("history-descriptor-cache").await;
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
    let timestamp = Utc::now();
    let first = ChatMessage {
        id: "history-first".to_string(),
        role: MessageRole::User,
        content: "first prompt".to_string(),
        timestamp,
        metadata: Value::Null,
    };
    state
        .storage
        .append_message(&session.id, &first)
        .expect("first");
    let version = state.storage.message_history_version(&session.id).unwrap();
    let initial = state
        .sessions
        .stored_context_history(&session.id, None, &version)
        .await
        .unwrap()
        .unwrap();
    let warm = state
        .sessions
        .stored_context_history(&session.id, None, &version)
        .await
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&initial, &warm));
    assert_eq!(initial.len(), 1);

    let second = ChatMessage {
        id: "history-second".to_string(),
        timestamp: timestamp + chrono::Duration::seconds(1),
        content: "second prompt".to_string(),
        ..first.clone()
    };
    state.storage.append_message(&session.id, &second).unwrap();
    let appended_version = state.storage.message_history_version(&session.id).unwrap();
    assert_eq!(version.rewrite_revision, appended_version.rewrite_revision);
    let appended = state
        .sessions
        .stored_context_history(&session.id, None, &appended_version)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(appended.len(), 2);
    assert_eq!(initial.len(), 1);

    let backfill = ChatMessage {
        id: "history-backfill".to_string(),
        timestamp: timestamp - chrono::Duration::seconds(1),
        ..first.clone()
    };
    state
        .storage
        .append_message(&session.id, &backfill)
        .unwrap();
    let backfill_version = state.storage.message_history_version(&session.id).unwrap();
    let rebuilt = state
        .sessions
        .stored_context_history(&session.id, None, &backfill_version)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rebuilt.len(), 3);
    assert!(
        matches!(&rebuilt[0].source, ActiveContextMessageSource::Stored(id) if id == &backfill.id)
    );

    state
        .storage
        .replace_session_messages(&session.id, &[second])
        .unwrap();
    let replaced_version = state.storage.message_history_version(&session.id).unwrap();
    let replaced = state
        .sessions
        .stored_context_history(&session.id, None, &replaced_version)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replaced.len(), 1);
    assert!(
        matches!(&replaced[0].source, ActiveContextMessageSource::Stored(id) if id == "history-second")
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

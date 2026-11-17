#[test]
fn history_versions_distinguish_appends_from_rewrites_across_connections() {
    let (storage, root) = temporary_storage("history-versions");
    storage
        .upsert_session(&test_session("history", false))
        .expect("session");
    let other = Storage::open(storage.path()).expect("other connection");
    let write = |id: &str, timestamp: &str| {
        other
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO messages(id, session_id, role, content, timestamp, metadata)
                 VALUES (?1, 'history', 'user', 'prompt', ?2, '{}')",
                    params![id, timestamp],
                )?;
                Ok(())
            })
            .expect("insert");
    };
    write("first", "2026-09-01T00:00:00+00:00");
    let first = storage.message_history_version("history").expect("version");
    write("second", "2026-09-01T00:00:01+00:00");
    let second = storage.message_history_version("history").expect("version");
    assert!(second.revision > first.revision);
    assert_eq!(second.rewrite_revision, first.rewrite_revision);
    assert_eq!(
        storage
            .messages_after_history_version("history", &first)
            .expect("append")
            .len(),
        1
    );
    write("backfill", "2026-08-31T00:00:00+00:00");
    let backfill = storage.message_history_version("history").expect("version");
    assert!(backfill.rewrite_revision > second.rewrite_revision);
    other
        .with_connection(|conn| {
            conn.execute(
                "UPDATE messages SET content = 'edited' WHERE id = 'first'",
                [],
            )?;
            Ok(())
        })
        .expect("edit");
    let edited = storage.message_history_version("history").expect("version");
    assert!(edited.rewrite_revision > backfill.rewrite_revision);
    other
        .with_connection(|conn| {
            conn.execute("DELETE FROM messages WHERE session_id = 'history'", [])?;
            Ok(())
        })
        .expect("delete");
    let deleted = storage.message_history_version("history").expect("version");
    assert!(deleted.rewrite_revision > edited.rewrite_revision);
    assert!(deleted.last_id.is_empty());
    drop(other);
    drop(storage);
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn external_history_pages_seek_by_sequence_and_reject_stale_sources() {
    let (storage, root) = temporary_storage("external-pages");
    let fingerprint = ExternalHistoryFingerprint {
        file_identity: None,
        file_size: 100,
        modified_nanos: Some(1),
        parser_version: 1,
    };
    let messages = (0..65)
        .map(|index| ChatMessage {
            id: format!("external-{index}"),
            role: MessageRole::User,
            content: format!("prompt {index}"),
            timestamp: Utc::now(),
            metadata: Value::Null,
        })
        .collect::<Vec<_>>();
    storage
        .replace_external_messages(
            Provider::Codex,
            "session",
            "rollout",
            &fingerprint,
            &messages,
        )
        .expect("persist");
    for offset in [0, 1, 29, 30, 60, 65, 99] {
        let (page, total) = storage
            .external_messages_page_if_current(
                Provider::Codex,
                "session",
                "rollout",
                &fingerprint,
                30,
                offset,
            )
            .expect("page")
            .expect("current");
        assert_eq!(total, messages.len());
        assert_eq!(
            page.iter().map(|message| &message.id).collect::<Vec<_>>(),
            messages
                .iter()
                .skip(offset)
                .take(30)
                .map(|message| &message.id)
                .collect::<Vec<_>>()
        );
    }
    let stale = ExternalHistoryFingerprint {
        file_size: 101,
        ..fingerprint
    };
    assert!(
        storage
            .external_messages_page_if_current(Provider::Codex, "session", "rollout", &stale, 30, 0)
            .expect("stale")
            .is_none()
    );
    drop(storage);
    std::fs::remove_dir_all(root).expect("cleanup");
}

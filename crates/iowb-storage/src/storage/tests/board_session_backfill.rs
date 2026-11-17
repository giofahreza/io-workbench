#[test]
fn board_backfill_project_summaries_preserve_discovery_identity_and_time_filtering() {
    let (storage, root) = temporary_storage("backfill-project");
    for id in [
        "ordinary",
        "board",
        "other",
        "replaced",
        "retained",
        "destination",
    ] {
        let mut session = test_session(id, false);
        session.project_path = if id == "other" {
            "/other"
        } else {
            "/tmp/project"
        }
        .into();
        session.board_session = id == "board";
        session.board_id = (id == "board").then(|| "board-1".into());
        storage.upsert_session(&session).unwrap();
    }
    storage.with_connection(|conn| {
        for (id, replaces) in [("replaced", 1), ("retained", 0)] {
            conn.execute("INSERT INTO session_forks (user_id,source_session_id,before_message_id,request_id,destination_session_id,created_at,replaces_source) VALUES ('u',?1,'m',?1,'destination','2026-10-05T00:00:00Z',?2)", params![id,replaces])?;
        }
        for (id,time) in [("ordinary","2026-10-05T07:00:00+07:00"),("board","2026-10-05T00:04:00Z"),("retained","2026-10-05T00:04:00.000001Z"),("destination","2026-10-05 00:02:00")] {
            conn.execute("UPDATE sessions SET last_activity=?2 WHERE id=?1",params![id,time])?;
        }
        Ok(())
    }).unwrap();
    let old = storage
        .list_sessions_including_board()
        .unwrap()
        .into_iter()
        .filter(|s| s.project_path == "/tmp/project")
        .collect::<Vec<_>>();
    let new = storage
        .list_session_summaries_for_project("/tmp/project")
        .unwrap();
    let identity = |s: &SessionSummary| {
        (
            s.id.clone(),
            s.project_path.clone(),
            s.board_session,
            s.board_id.clone(),
            s.board_task_id.clone(),
            s.last_activity,
        )
    };
    assert_eq!(
        old.iter().map(identity).collect::<Vec<_>>(),
        new.iter().map(identity).collect::<Vec<_>>()
    );
    assert_eq!(new.len(), 4);
    let start = DateTime::parse_from_rfc3339("2026-10-05T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let end = start + chrono::Duration::minutes(4);
    let mut eligible = new
        .iter()
        .filter(|s| s.last_activity >= start && s.last_activity <= end)
        .map(|s| s.id.as_str())
        .collect::<Vec<_>>();
    eligible.sort();
    assert_eq!(eligible, vec!["board", "destination", "ordinary"]);
    assert!(
        storage
            .list_session_summaries_for_project("/missing")
            .unwrap()
            .is_empty()
    );
    drop(storage);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_backfill_first_user_preserves_order_empty_content_and_legacy_roles() {
    let (storage, root) = temporary_storage("backfill-first");
    storage
        .upsert_session(&test_session("session", false))
        .unwrap();
    let timestamp = Utc::now();
    for (id, role, content) in [
        ("00", MessageRole::System, "system"),
        ("01", MessageRole::Assistant, "assistant"),
        ("03", MessageRole::User, "Board id: later"),
        ("02", MessageRole::User, ""),
    ] {
        let mut message = test_message(id, role, content, 0);
        message.timestamp = timestamp;
        storage.append_message("session", &message).unwrap();
    }
    let check = || {
        let old = storage
            .list_messages("session")
            .unwrap()
            .into_iter()
            .find(|m| m.role == MessageRole::User)
            .unwrap();
        let new = storage.first_user_message("session").unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(old).unwrap(),
            serde_json::to_value(new).unwrap()
        );
    };
    check();
    assert_eq!(
        storage
            .first_user_message("session")
            .unwrap()
            .unwrap()
            .content,
        ""
    );
    storage
        .with_connection(|conn| {
            conn.execute(
                "UPDATE messages SET role='legacy-human', metadata='invalid json' WHERE id='01'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    check();
    assert_eq!(
        storage.first_user_message("session").unwrap().unwrap().id,
        "01"
    );
    storage
        .with_connection(|conn| {
            conn.execute("UPDATE messages SET role='tool'", [])?;
            Ok(())
        })
        .unwrap();
    assert!(storage.first_user_message("session").unwrap().is_none());
    assert!(storage.first_user_message("missing").unwrap().is_none());
    drop(storage);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn session_backfill_scoped_reads_preserve_legacy_classification() {
    let fixture = BoardReadAccessFixture::new().await;
    let mut run = board_fixture(
        json!({"command":"Backfill parity","projectPath":fixture.project.display().to_string(),"provider":"codex"}),
    );
    run.id = "backfill-parity".into();
    run.tasks[0].status = TASK_STATUS_DONE.into();
    run.tasks[0].provider_session_id = None;
    let timestamp = run.created_at;
    run.updated_at = timestamp;
    let prompt = format!("Board id: {}\nTask 1: {}", run.id, run.tasks[0].id);
    for id in [
        "explicit",
        "other-project",
        "outside-window",
        "empty-first",
        "other-board",
        "signature-only",
    ] {
        let session = SessionSummary {
            id: id.into(),
            provider: Provider::Codex,
            project_path: if id == "other-project" {
                fixture.root.join("other").display().to_string()
            } else {
                run.project_path.clone()
            },
            last_activity: timestamp,
            board_session: id == "other-board",
            board_id: (id == "other-board").then(|| "another-board".into()),
            ..Default::default()
        };
        fixture.state.storage.upsert_session(&session).unwrap();
        let content = match id {
            "empty-first" => "",
            "signature-only" => "autonomous Kanban agent",
            _ => prompt.as_str(),
        };
        fixture
            .state
            .storage
            .append_message(
                id,
                &ChatMessage {
                    id: format!("{id}-first"),
                    role: MessageRole::User,
                    content: content.into(),
                    timestamp,
                    metadata: Value::Null,
                },
            )
            .unwrap();
        if id == "empty-first" {
            fixture
                .state
                .storage
                .append_message(
                    id,
                    &ChatMessage {
                        id: format!("{id}-later"),
                        role: MessageRole::User,
                        content: prompt.clone(),
                        timestamp: timestamp + chrono::Duration::seconds(1),
                        metadata: Value::Null,
                    },
                )
                .unwrap();
        }
        // append_message updates activity; set the controlled boundary afterwards.
        if id == "outside-window" {
            let mut outside = fixture.state.storage.get_session(id).unwrap().unwrap();
            outside.last_activity = timestamp + chrono::Duration::minutes(3);
            fixture.state.storage.upsert_session(&outside).unwrap();
        }
    }
    backfill_board_session_links(&fixture.state, &mut run)
        .await
        .unwrap();
    assert_eq!(
        run.tasks[0].provider_session_id.as_deref(),
        Some("explicit")
    );
    assert_eq!(run.updated_at, timestamp);
    for id in [
        "other-project",
        "outside-window",
        "empty-first",
        "signature-only",
    ] {
        assert!(
            !fixture
                .state
                .storage
                .get_session(id)
                .unwrap()
                .unwrap()
                .is_board_session(),
            "unexpected classification: {id}"
        );
    }
    assert_eq!(
        fixture
            .state
            .storage
            .get_session("other-board")
            .unwrap()
            .unwrap()
            .board_id
            .as_deref(),
        Some("another-board")
    );
    let before = serde_json::to_value(&run).unwrap();
    backfill_board_session_links(&fixture.state, &mut run)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&run).unwrap(), before);
    run.prompt_telemetry
        .push(json!({"startedAt":timestamp.to_rfc3339()}));
    backfill_board_session_links(&fixture.state, &mut run)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .state
            .storage
            .get_session("signature-only")
            .unwrap()
            .unwrap()
            .board_id
            .as_deref(),
        Some(run.id.as_str())
    );
}

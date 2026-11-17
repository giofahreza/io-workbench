async fn discussion_lifecycle_fixture() -> (PathBuf, AppState, AgenticBoard) {
    let root = std::env::temp_dir().join(format!("iowb-discussion-lifecycle-{}", Uuid::new_v4()));
    let project = root.join("project");
    let config_dir = root.join("config");
    fs::create_dir_all(&project).unwrap();
    let state = AppState::initialize(AppConfig {
        host: "127.0.0.1".parse().unwrap(),
        port: 0,
        config_dir: config_dir.clone(),
        database_path: config_dir.join("test.db"),
        workspace_root: root.clone(),
        auth_required: false,
        local_token: None,
        otp_secret: None,
        max_sessions: 10,
        max_scan_depth: 2,
        max_file_read_bytes: 1024 * 1024,
    })
    .await
    .unwrap();
    let mut run = board_fixture(
        json!({"command":"Configure test runner", "projectPath":project.display().to_string(), "provider":"codex"}),
    );
    run.user_id = Some("discussion-user".into());
    save_board(&state, &run).unwrap();
    (root, state, run)
}

fn discussion_lifecycle_request() -> DiscussionRequest {
    DiscussionRequest {
        message: Some("Use stable TAP output".into()),
        action: Some("edit".into()),
        payload: Some(json!({"details":"Use stable TAP output"})),
    }
}

fn discussion_lifecycle_provider_output() -> String {
    json!({"action":"edit", "summary":"Use stable TAP output", "payload":{"details":"Use stable TAP output"}, "warnings":[]}).to_string()
}

#[tokio::test(flavor = "current_thread")]
async fn discussion_lifecycle_persists_pending_proposal_after_http_waiter_disconnects() {
    let (root, state, original) = discussion_lifecycle_fixture().await;
    let board_id = original.id.clone();
    let operation = claim_discussion_operation("discussion-user", &board_id).unwrap();
    let worker_state = state.clone();
    let worker_board = board_id.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let http_waiter = tokio::spawn(async move {
        await_discussion_completion(operation, async move {
            let result = run_discussion_proposal_with_provider(
                worker_state,
                "discussion-user".into(),
                worker_board,
                "task-1".into(),
                discussion_lifecycle_request(),
                move |_, _, _, _, _| async move {
                    started_tx.send(()).unwrap();
                    release_rx.await.unwrap();
                    Ok(discussion_lifecycle_provider_output())
                },
            )
            .await;
            finished_tx.send(()).unwrap();
            result
        })
        .await
    });
    timeout(Duration::from_secs(2), started_rx)
        .await
        .unwrap()
        .unwrap();
    http_waiter.abort();
    assert!(http_waiter.await.unwrap_err().is_cancelled());
    assert_eq!(
        claim_discussion_operation("discussion-user", &board_id)
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        load_user_board(&state, "discussion-user", &board_id)
            .unwrap()
            .board
            .discussion_proposals[0]["status"],
        "running"
    );
    release_tx.send(()).unwrap();
    timeout(Duration::from_secs(2), finished_rx)
        .await
        .unwrap()
        .unwrap();
    let stored = load_user_board(&state, "discussion-user", &board_id)
        .unwrap()
        .board;
    assert_eq!(stored.discussion_proposals.len(), 1);
    assert_eq!(stored.discussion_proposals[0]["status"], "pending");
    assert_eq!(stored.tasks[0].hierarchy.discussion[0]["status"], "pending");
    assert_eq!(stored.tasks[0].description, original.tasks[0].description);
    assert_eq!(
        stored.tasks[0].hierarchy.scope_version,
        original.tasks[0].hierarchy.scope_version
    );
    assert!(!stored.active);
    assert!(claim_discussion_operation("discussion-user", &board_id).is_ok());
    drop(state);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test(flavor = "current_thread")]
async fn discussion_lifecycle_persists_provider_and_contract_failures_without_applying() {
    for malformed in [false, true] {
        let (root, state, original) = discussion_lifecycle_fixture().await;
        let board_id = original.id.clone();
        let operation = claim_discussion_operation("discussion-user", &board_id).unwrap();
        let response = await_discussion_completion(
            operation,
            run_discussion_proposal_with_provider(
                state.clone(),
                "discussion-user".into(),
                board_id.clone(),
                "task-1".into(),
                discussion_lifecycle_request(),
                move |_, _, _, _, _| async move {
                    if malformed {
                        Ok("not valid JSON".into())
                    } else {
                        Err(bad_request("provider unavailable"))
                    }
                },
            ),
        )
        .await
        .unwrap();
        assert_eq!(response.0["success"], false);
        let stored = load_user_board(&state, "discussion-user", &board_id)
            .unwrap()
            .board;
        assert_eq!(stored.discussion_proposals[0]["status"], "failed");
        assert!(
            !stored.discussion_proposals[0]["error"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        assert_eq!(stored.tasks[0].description, original.tasks[0].description);
        assert!(!stored.active);
        assert!(claim_discussion_operation("discussion-user", &board_id).is_ok());
        drop(state);
        let _ = fs::remove_dir_all(root);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn discussion_lifecycle_retry_recovers_orphaned_running_history_not_pending_review() {
    let (root, state, mut original) = discussion_lifecycle_fixture().await;
    let mut orphan = discussion_running_entry(
        "orphan",
        "task-1",
        "Old edit",
        "edit",
        &Value::Null,
        "codex",
        "min:MiniMax-M3",
        Utc::now(),
    );
    append_discussion_proposal(&mut original, "task-1", orphan.clone()).unwrap();
    orphan["id"] = json!("pending-review");
    orphan["status"] = json!("pending");
    append_discussion_proposal(&mut original, "task-1", orphan).unwrap();
    save_board(&state, &original).unwrap();
    let board_id = original.id.clone();
    let operation = claim_discussion_operation("discussion-user", &board_id).unwrap();
    let _ = await_discussion_completion(
        operation,
        run_discussion_proposal_with_provider(
            state.clone(),
            "discussion-user".into(),
            board_id.clone(),
            "task-1".into(),
            discussion_lifecycle_request(),
            |_, _, _, _, _| async { Ok(discussion_lifecycle_provider_output()) },
        ),
    )
    .await
    .unwrap();
    let stored = load_user_board(&state, "discussion-user", &board_id)
        .unwrap()
        .board;
    assert_eq!(stored.discussion_proposals.len(), 3);
    assert_eq!(stored.discussion_proposals[0]["id"], "orphan");
    assert_eq!(stored.discussion_proposals[0]["status"], "failed");
    assert!(
        stored.discussion_proposals[0]["error"]
            .as_str()
            .unwrap()
            .contains("nothing was applied")
    );
    assert_eq!(stored.discussion_proposals[1]["status"], "pending");
    assert_eq!(stored.discussion_proposals[2]["status"], "pending");
    assert_eq!(stored.tasks[0].description, original.tasks[0].description);
    drop(state);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test(flavor = "current_thread")]
async fn discussion_lifecycle_scope_change_during_generation_persists_failure_not_rebased_proposal()
{
    let (root, state, original) = discussion_lifecycle_fixture().await;
    let board_id = original.id.clone();
    let operation = claim_discussion_operation("discussion-user", &board_id).unwrap();
    let _ = await_discussion_completion(
        operation,
        run_discussion_proposal_with_provider(
            state.clone(),
            "discussion-user".into(),
            board_id.clone(),
            "task-1".into(),
            discussion_lifecycle_request(),
            |state, user_id, board_id, _, _| async move {
                mutate_stored_board(&state, &user_id, &board_id, |run| {
                    run.tasks[0].hierarchy.scope_version += 1;
                    run.tasks[0].description = "New user scope".into();
                    Ok(())
                })?;
                Ok(discussion_lifecycle_provider_output())
            },
        ),
    )
    .await
    .unwrap();
    let stored = load_user_board(&state, "discussion-user", &board_id)
        .unwrap()
        .board;
    assert_eq!(stored.discussion_proposals[0]["status"], "failed");
    assert!(
        stored.discussion_proposals[0]["error"]
            .as_str()
            .unwrap()
            .contains("scope changed")
    );
    assert_eq!(stored.tasks[0].description, "New user scope");
    drop(state);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn discussion_lifecycle_pending_apply_rejects_changed_source_scope() {
    let mut run =
        board_fixture(json!({"command":"Configure runner", "projectPath":"/tmp/project"}));
    let version = run.tasks[0].hierarchy.scope_version;
    let proposal = json!({"status":"pending", "sourceScopeVersion":version,
        "sourceScopeIdentity":discussion_source_identity(&discussion_scope_snapshot(&run,"task-1")).unwrap()});
    assert!(
        validate_discussion_source_scope(&run, "task-1", &json!({"status":"pending"})).is_err()
    );
    assert!(validate_discussion_source_scope(&run, "task-1", &proposal).is_ok());
    run.tasks[0].hierarchy.scope_version += 1;
    assert_eq!(
        validate_discussion_source_scope(&run, "task-1", &proposal)
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
    let legacy = json!({"before":{"task":{"scopeVersion":version}}});
    assert!(validate_discussion_source_scope(&run, "task-1", &legacy).is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn discussion_lifecycle_descendant_added_during_generation_rejects_stale_proposal() {
    let (root, state, original) = discussion_lifecycle_fixture().await;
    let board_id = original.id.clone();
    let operation = claim_discussion_operation("discussion-user", &board_id).unwrap();
    let response = await_discussion_completion(
        operation,
        run_discussion_proposal_with_provider(
            state.clone(),
            "discussion-user".into(),
            board_id.clone(),
            "task-1".into(),
            discussion_lifecycle_request(),
            |state, user_id, board_id, _, _| async move {
                mutate_stored_board(&state, &user_id, &board_id, |run| {
                    let mut child = BoardTask::draft(run, "New child".into(), "New scope".into());
                    child.hierarchy.parent_id = Some("task-1".into());
                    child.hierarchy.level = TASK_LEVEL_TASK.into();
                    run.tasks.push(child);
                    Ok(())
                })?;
                Ok(discussion_lifecycle_provider_output())
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(response.0["success"], false);
    let stored = load_user_board(&state, "discussion-user", &board_id)
        .unwrap()
        .board;
    assert_eq!(stored.discussion_proposals[0]["status"], "failed");
    assert!(
        stored.discussion_proposals[0]["error"]
            .as_str()
            .unwrap()
            .contains("descendant scope changed")
    );
    assert_eq!(stored.tasks.len(), 2);
    assert_eq!(
        stored.tasks[0].hierarchy.scope_version,
        original.tasks[0].hierarchy.scope_version
    );
    assert_eq!(stored.tasks[1].description, "New scope");
    assert!(!stored.active);
    drop(state);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn discussion_lifecycle_apply_requires_the_reviewed_descendant_identity_not_status() {
    let mut run =
        board_fixture(json!({"command":"Configure runner", "projectPath":"/tmp/project"}));
    let before = discussion_scope_snapshot(&run, "task-1");
    let proposal = json!({"status":"pending", "before": before});
    run.tasks[0].status = TASK_STATUS_TODO.into();
    assert!(validate_discussion_source_scope(&run, "task-1", &proposal).is_ok());
    let mut child = BoardTask::draft(&mut run, "New child".into(), "New user scope".into());
    child.hierarchy.parent_id = Some("task-1".into());
    child.hierarchy.level = TASK_LEVEL_TASK.into();
    run.tasks.push(child);
    assert!(validate_discussion_source_scope(&run, "task-1", &proposal).is_err());
    let before = discussion_scope_snapshot(&run, "task-1");
    let reviewed = json!({"status":"pending", "before": before});
    assert!(validate_discussion_source_scope(&run, "task-1", &reviewed).is_ok());
    run.tasks[1].hierarchy.scope_version += 1;
    assert!(validate_discussion_source_scope(&run, "task-1", &reviewed).is_err());
}

fn interrupted_board_fixture() -> AgenticBoard {
    let mut run = retry_hierarchy_fixture();
    run.id = format!("restart-recovery-{}", Uuid::new_v4());
    run.tasks[2].status = TASK_STATUS_IN_PROGRESS.to_string();
    let scope_version = run.tasks[2].hierarchy.scope_version;
    run.tasks[2].hierarchy.attempts.push(json!({
        "attemptId": "interrupted-attempt", "attemptNumber": 2,
        "scopeVersion": scope_version,
        "startedAt": "2026-09-28T16:27:15Z", "status": "running"
    }));
    run.tasks[2].result = Some(json!({"status":"failed", "summary":"old assertion failure"}));
    run.tasks[2].provider_session_id = Some("previous-provider-session".to_string());
    run.tasks[2].changed_files = vec!["old.js".to_string()];
    run.current_provider_session_id = run.tasks[2].provider_session_id.clone();
    run.provider_call_started_at = Some(Utc::now());
    refresh_hierarchy_rollups(&mut run);
    run
}

#[test]
fn restart_recovery_finds_orphan_leaf_on_already_blocked_board_not_active_parents() {
    let run = interrupted_board_fixture();
    assert_eq!(run.status, "blocked");
    assert!(!run.active);
    assert_eq!(
        interrupted_execution_task_ids(&run),
        vec![run.tasks[2].id.clone()]
    );
}

#[test]
fn restart_recovery_closes_attempt_without_trusting_old_result_or_attribution() {
    let mut run = interrupted_board_fixture();
    let previous_attempt = run.tasks[2].hierarchy.attempts[0].clone();
    let task_id = run.tasks[2].id.clone();
    let scope = run.tasks[2].hierarchy.scope_version;
    assert_eq!(
        settle_interrupted_board_tasks(&mut run, &[task_id.clone()]),
        1
    );
    let task = &run.tasks[2];
    assert_eq!(task.status, TASK_STATUS_BLOCKED);
    assert!(task_failure_is_transient(task));
    assert!(task.error.as_deref().unwrap().contains("server restart"));
    assert_eq!(task.hierarchy.scope_version, scope);
    assert_eq!(task.hierarchy.attempts[0], previous_attempt);
    let attempt = task.hierarchy.attempts.last().unwrap();
    assert_eq!(attempt["status"], "blocked");
    assert_eq!(attempt["interrupted"], true);
    assert!(attempt["finishedAt"].is_string());
    assert_eq!(attempt["workspaceAttributionUnavailable"], true);
    assert!(attempt.get("filesChanged").is_none());
    assert_eq!(task.changed_files, vec!["old.js"]);
    assert_eq!(
        task.provider_session_id.as_deref(),
        Some("previous-provider-session")
    );
    assert!(run.current_provider_session_id.is_none());
    assert!(run.provider_call_started_at.is_none());
    assert!(!run.active && !run.loop_started);
    assert!(restart_recovery_requires_explicit_retry(&run));
    assert_eq!(
        retry_attention_tasks_in_board(
            &mut run,
            &[task_id],
            RETRY_MODE_TRANSIENT,
            None,
            "retry interrupted provider"
        )
        .unwrap(),
        1
    );
    assert_eq!(run.tasks[2].status, TASK_STATUS_TODO);
    assert!(!restart_recovery_requires_explicit_retry(&run));
}

#[test]
fn restart_recovery_waits_when_either_session_or_durable_worker_is_active() {
    for status in [None, Some("completed"), Some("failed"), Some("interrupted")] {
        assert!(recovery_provider_is_active(true, status));
        assert!(!recovery_provider_is_active(false, status));
    }
    assert!(recovery_provider_is_active(false, Some("running")));
    assert!(recovery_provider_is_active(false, Some("recovering")));
}

#[test]
fn restart_recovery_does_not_replace_explicit_abort_with_failure() {
    let mut run = interrupted_board_fixture();
    let task_id = run.tasks[2].id.clone();
    reset_in_flight_board_tasks(
        &mut run,
        "Task returned to Todo because the board was aborted",
    );
    run.status = "cancelled".to_string();
    assert_eq!(
        run.tasks[2].hierarchy.attempts.last().unwrap()["status"],
        "cancelled"
    );
    assert_eq!(settle_interrupted_board_tasks(&mut run, &[task_id]), 0);
    assert_eq!(run.status, "cancelled");
    assert_eq!(run.tasks[2].status, TASK_STATUS_TODO);
    assert!(run.current_provider_session_id.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn restart_recovery_waits_for_live_provider_and_blocks_duplicate_start() {
    let root = std::env::temp_dir().join(format!("iowb-board-restart-{}", Uuid::new_v4()));
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
    let session = state
        .sessions
        .create_or_update(
            Provider::Codex,
            project.display().to_string(),
            None,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    state.sessions.set_active(&session.id, true).await.unwrap();
    let mut run = interrupted_board_fixture();
    run.project_path = project.display().to_string();
    run.user_id = Some("restart-user".to_string());
    run.tasks[2].provider_session_id = Some(session.id.clone());
    run.current_provider_session_id = Some(session.id.clone());
    save_board(&state, &run).unwrap();
    assert!(claim_project_execution(&run.project_path, &run.id).unwrap());
    let worker_state = state.clone();
    let worker_run = run.clone();
    let task_ids = interrupted_execution_task_ids(&run);
    let mut worker = tokio::spawn(async move {
        recover_interrupted_board_execution(
            &worker_state,
            "restart-user",
            &worker_run.id,
            &worker_run.project_path,
            task_ids,
        )
        .await
    });
    assert!(
        timeout(Duration::from_millis(100), &mut worker)
            .await
            .is_err()
    );
    let waiting = load_user_board(&state, "restart-user", &run.id)
        .unwrap()
        .board;
    assert!(waiting.active);
    assert_eq!(waiting.tasks[2].status, TASK_STATUS_IN_PROGRESS);
    assert_eq!(
        waiting.current_phase.as_deref(),
        Some("recovering_interrupted_task")
    );
    assert!(start_board_execution(&state, "restart-user", &run.id).is_err());
    assert!(!claim_project_execution(&run.project_path, &run.id).unwrap());
    state.sessions.set_active(&session.id, false).await.unwrap();
    timeout(Duration::from_secs(3), &mut worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let recovered = load_user_board(&state, "restart-user", &run.id)
        .unwrap()
        .board;
    assert_eq!(recovered.tasks[2].status, TASK_STATUS_BLOCKED);
    assert_eq!(recovered.tasks[2].attempt_count, run.tasks[2].attempt_count);
    assert!(ensure_restart_recovery_finished(&run.id).is_ok());
    assert!(claim_project_execution(&run.project_path, &run.id).unwrap());
    release_project_execution(&run.project_path, &run.id);
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

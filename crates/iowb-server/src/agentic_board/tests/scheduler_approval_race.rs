async fn scheduler_approval_fixture() -> (PathBuf, AppState, AgenticBoard, String, String, String) {
    let (root, state, mut run) = discussion_lifecycle_fixture().await;
    run.status = "running".into();
    run.active = true;
    run.loop_started = true;
    run.bootstrap_complete = true;
    run.auto_retry["enabled"] = json!(false);
    let mut story = BoardTask::draft(&mut run, "Approved shell wrapper".into(), "Create the requested shell".into());
    story.status = TASK_STATUS_TODO.into();
    story.hierarchy.level = TASK_LEVEL_STORY.into();
    story.hierarchy.executable = false;
    let story_id = story.id.clone();
    run.tasks.push(story);
    let mut parent = BoardTask::draft(&mut run, "Planned shell task".into(), "Create index.html".into());
    parent.status = TASK_STATUS_BACKLOG.into();
    parent.hierarchy.level = TASK_LEVEL_TASK.into();
    parent.hierarchy.parent_id = Some(story_id.clone());
    parent.hierarchy.executable = false;
    let parent_id = parent.id.clone();
    run.tasks.push(parent);
    let mut leaf = BoardTask::draft(&mut run, "Create index.html".into(), "Create the approved static HTML shell".into());
    leaf.status = TASK_STATUS_BACKLOG.into();
    leaf.hierarchy.level = TASK_LEVEL_SUBTASK.into();
    leaf.hierarchy.parent_id = Some(parent_id.clone());
    leaf.hierarchy.executable = true;
    let leaf_id = leaf.id.clone();
    run.tasks.push(leaf);
    refresh_hierarchy_rollups(&mut run);
    save_board(&state, &run).unwrap();
    (root, state, run, story_id, parent_id, leaf_id)
}

#[tokio::test(flavor = "current_thread")]
async fn scheduler_approval_race_preserves_acknowledged_parent_promotion_before_child() {
    use std::sync::mpsc;
    let (root, state, run, story_id, parent_id, leaf_id) = scheduler_approval_fixture().await;
    let (loaded_tx, loaded_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let worker_state = state.clone();
    let worker_board = run.id.clone();
    let worker = std::thread::spawn(move || {
        let decision = prepare_board_execution_step_observed(&worker_state, "discussion-user", &worker_board, || {
            loaded_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }).unwrap();
        assert!(matches!(decision, BoardExecutionPreparation::Stop));
    });
    loaded_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (trying_tx, trying_rx) = mpsc::channel();
    let (promoted_tx, promoted_rx) = mpsc::channel();
    let mutation_state = state.clone();
    let mutation_board = run.id.clone();
    let mutation_parent = parent_id.clone();
    let mutation = std::thread::spawn(move || {
        trying_tx.send(()).unwrap();
        let result = update_task_status(&mutation_state, "discussion-user", &mutation_board, &[mutation_parent], TASK_STATUS_TODO);
        promoted_tx.send(result.is_ok()).unwrap();
        assert!(result.is_ok());
    });
    trying_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // Before the fix the acknowledged promotion commits while the worker
    // holds an older snapshot, which then overwrites it. With the transaction
    // guard it waits until the worker has finished its synchronous decision.
    let acknowledged_before_release = promoted_rx.recv_timeout(Duration::from_millis(250)).ok();
    release_tx.send(()).unwrap();
    worker.join().unwrap();
    mutation.join().unwrap();
    if acknowledged_before_release.is_none() {
        assert!(promoted_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    }
    let current = load_user_board(&state, "discussion-user", &run.id).unwrap().board;
    let parent = current.tasks.iter().find(|task| task.id == parent_id).unwrap();
    assert_eq!(parent.status, TASK_STATUS_TODO, "a successful parent promotion must survive the worker save");
    assert!(task_scope_is_approved(parent));
    assert!(task_scope_is_approved(current.tasks.iter().find(|task| task.id == story_id).unwrap()));
    assert_eq!(current.tasks.iter().find(|task| task.id == leaf_id).unwrap().status, TASK_STATUS_BACKLOG);
    assert_eq!(current.tasks[0].status, TASK_STATUS_BACKLOG);
    assert!(current.tasks.iter().all(|task| task.attempt_count == 0));
    assert!(update_task_status(&state, "discussion-user", &run.id, &[leaf_id], TASK_STATUS_TODO).is_ok(), "the next parent-first native promotion must not receive a false ancestor-approval conflict");
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn scheduler_approval_preparation_claims_only_the_approved_executable_leaf() {
    let (root, state, mut run, story_id, parent_id, leaf_id) = scheduler_approval_fixture().await;
    for task in &mut run.tasks {
        if task.id == parent_id || task.id == leaf_id { task.status = TASK_STATUS_TODO.into(); }
    }
    refresh_hierarchy_rollups(&mut run);
    save_board(&state, &run).unwrap();
    let decision = prepare_board_execution_step(&state, "discussion-user", &run.id).unwrap();
    let BoardExecutionPreparation::Execute { stored, task_index, task_id, .. } = decision else { panic!("approved executable leaf should be claimed"); };
    assert_eq!(task_id, leaf_id);
    assert_eq!(stored.board.tasks[task_index].status, TASK_STATUS_IN_PROGRESS);
    assert_eq!(stored.board.tasks[task_index].attempt_count, 1);
    assert_eq!(stored.board.tasks[0].status, TASK_STATUS_BACKLOG);
    for id in [story_id, parent_id] {
        assert_eq!(stored.board.tasks.iter().find(|task| task.id == id).unwrap().attempt_count, 0);
    }
    let current = load_user_board(&state, "discussion-user", &run.id).unwrap().board;
    assert_eq!(current.current_task_id.as_deref(), Some(leaf_id.as_str()));
    assert_eq!(current.tasks.iter().filter(|task| task.attempt_count > 0).count(), 1);
    assert!(current.provider_call_started_at.is_none(), "preparation does not invoke a provider");
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn scheduler_approval_preparation_preserves_terminal_bootstrap_and_breakdown_decisions() {
    let (root, state, original, story_id, _, _) = scheduler_approval_fixture().await;
    for status in ["paused", "cancelled", "blocked", "failed", "completed"] {
        let mut run = original.clone();
        run.status = status.into();
        run.bootstrap_complete = false;
        save_board(&state, &run).unwrap();
        assert!(matches!(prepare_board_execution_step(&state, "discussion-user", &run.id).unwrap(), BoardExecutionPreparation::Stop));
        let current = load_user_board(&state, "discussion-user", &run.id).unwrap().board;
        assert_eq!(current.status, status);
        assert!(!current.active);
        assert!(current.tasks.iter().all(|task| task.attempt_count == 0));
    }
    let mut run = original.clone();
    run.bootstrap_complete = false;
    save_board(&state, &run).unwrap();
    assert!(matches!(prepare_board_execution_step(&state, "discussion-user", &run.id).unwrap(), BoardExecutionPreparation::Bootstrap));
    run.bootstrap_complete = true;
    run.tasks.retain(|task| task.hierarchy.parent_id.is_none());
    refresh_hierarchy_rollups(&mut run);
    save_board(&state, &run).unwrap();
    let decision = prepare_board_execution_step(&state, "discussion-user", &run.id).unwrap();
    assert!(matches!(decision, BoardExecutionPreparation::Breakdown(id) if id == story_id));
    let current = load_user_board(&state, "discussion-user", &run.id).unwrap().board;
    assert_eq!(current.current_phase.as_deref(), Some("hierarchy_breakdown"));
    assert!(current.provider_call_started_at.is_none());
    assert!(current.tasks.iter().all(|task| task.attempt_count == 0));
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

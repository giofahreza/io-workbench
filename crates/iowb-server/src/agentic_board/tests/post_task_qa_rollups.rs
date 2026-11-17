fn post_task_qa_rollup_fixture() -> (AgenticBoard, BoardTask) {
    let mut run = board_fixture(json!({
        "command": "Build Pocket Shopping",
        "projectPath": "/tmp/project"
    }));
    run.tasks[0].status = TASK_STATUS_IN_PROGRESS.to_string();
    let mut parent = run.tasks[0].clone();
    parent.id = "test-parent".to_string();
    parent.hierarchy.level = TASK_LEVEL_TASK.to_string();
    parent.hierarchy.parent_id = Some(run.tasks[0].id.clone());
    parent.hierarchy.executable = false;
    parent.task_type = TASK_KIND_TEST_IMPLEMENTATION.to_string();
    let mut source = parent.clone();
    source.id = "test-leaf".to_string();
    source.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
    source.hierarchy.parent_id = Some(parent.id.clone());
    source.hierarchy.executable = true;
    source.status = TASK_STATUS_DONE.to_string();
    source.attempt_count = 1;
    source.completed_at = Some(Utc::now());
    source.result = Some(json!({"status": "done", "summary": "Verified red baseline"}));
    source.hierarchy.attempts = vec![json!({"attemptId": "verified", "status": "done"})];
    run.tasks.extend([parent, source]);
    reconcile_scope_approval_records(&mut run);
    let source = run.tasks[2].clone();
    (run, source)
}

#[test]
fn post_task_qa_rollup_waits_for_required_qa_before_completing_parents() {
    let (mut run, source) = post_task_qa_rollup_fixture();
    let completed_leaf = serde_json::to_value(&source).unwrap();

    // The completion path must append its required QA before refreshing
    // hierarchy status; completing the source alone is not parent completion.
    assert!(append_task_qa_task(
        &mut run,
        &source,
        "Review the red baseline"
    ));
    refresh_hierarchy_rollups(&mut run);

    assert_eq!(run.tasks[0].status, TASK_STATUS_IN_PROGRESS);
    assert_eq!(run.tasks[1].status, TASK_STATUS_IN_PROGRESS);
    let qa_index = run
        .tasks
        .iter()
        .position(|task| task.task_level_qa)
        .unwrap();
    assert!(task_ancestors_are_approved(&run, &run.tasks[qa_index]));
    assert_eq!(pick_next_task_index(&run), Some(qa_index));
    assert_eq!(serde_json::to_value(&run.tasks[2]).unwrap(), completed_leaf);

    run.tasks[qa_index].status = TASK_STATUS_DONE.to_string();
    refresh_hierarchy_rollups(&mut run);
    assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
    assert_eq!(run.tasks[1].status, TASK_STATUS_DONE);
}

#[test]
fn post_task_qa_rollup_recovers_persisted_done_ancestors_in_one_refresh() {
    for (qa_status, expected_parent_status, expected_story_status) in [
        (
            TASK_STATUS_TODO,
            TASK_STATUS_IN_PROGRESS,
            TASK_STATUS_IN_PROGRESS,
        ),
        (
            TASK_STATUS_IN_PROGRESS,
            TASK_STATUS_IN_PROGRESS,
            TASK_STATUS_IN_PROGRESS,
        ),
        (
            TASK_STATUS_BACKLOG,
            TASK_STATUS_TODO,
            TASK_STATUS_IN_PROGRESS,
        ),
        (
            TASK_STATUS_BLOCKED,
            TASK_STATUS_BLOCKED,
            TASK_STATUS_BLOCKED,
        ),
    ] {
        let (mut run, source) = post_task_qa_rollup_fixture();
        refresh_hierarchy_rollups(&mut run);
        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
        assert_eq!(run.tasks[1].status, TASK_STATUS_DONE);
        let approvals = run.tasks[..3]
            .iter()
            .map(|task| serde_json::to_value(&task.hierarchy.scope_approval).unwrap())
            .collect::<Vec<_>>();
        let completed_leaf = serde_json::to_value(&run.tasks[2]).unwrap();
        assert!(append_task_qa_task(
            &mut run,
            &source,
            "Previously inserted too late"
        ));
        let qa_index = run
            .tasks
            .iter()
            .position(|task| task.task_level_qa)
            .unwrap();
        run.tasks[qa_index].status = qa_status.to_string();
        let mut run: AgenticBoard =
            serde_json::from_value(serde_json::to_value(run).unwrap()).unwrap();

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, expected_story_status, "{qa_status}");
        assert_eq!(run.tasks[1].status, expected_parent_status, "{qa_status}");
        assert_eq!(serde_json::to_value(&run.tasks[2]).unwrap(), completed_leaf);
        assert_eq!(run.tasks[qa_index].attempt_count, 0);
        for (task, approval) in run.tasks[..3].iter().zip(&approvals) {
            assert_eq!(
                &serde_json::to_value(&task.hierarchy.scope_approval).unwrap(),
                approval
            );
        }
        if qa_status == TASK_STATUS_TODO {
            assert!(task_ancestors_are_approved(&run, &run.tasks[qa_index]));
            assert_eq!(pick_next_task_index(&run), Some(qa_index));
        } else if qa_status == TASK_STATUS_BACKLOG {
            assert!(!task_scope_is_approved(&run.tasks[qa_index]));
            assert_eq!(pick_next_task_index(&run), None);
        }
        let once = serde_json::to_value(&run).unwrap();
        refresh_hierarchy_rollups(&mut run);
        assert_eq!(
            serde_json::to_value(&run).unwrap(),
            once,
            "idempotent {qa_status}"
        );
    }
}

#[test]
fn post_task_qa_rollup_optional_work_does_not_reopen_done_parents() {
    let (mut run, source) = post_task_qa_rollup_fixture();
    refresh_hierarchy_rollups(&mut run);
    assert!(append_task_qa_task(&mut run, &source, "Optional review"));
    let qa_index = run
        .tasks
        .iter()
        .position(|task| task.task_level_qa)
        .unwrap();
    run.tasks[qa_index].hierarchy.required = false;

    refresh_hierarchy_rollups(&mut run);

    assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
    assert_eq!(run.tasks[1].status, TASK_STATUS_DONE);
    assert_eq!(run.tasks[2].status, TASK_STATUS_DONE);
}

#[test]
fn post_task_qa_rollup_does_not_reopen_superseded_parents() {
    let (mut run, source) = post_task_qa_rollup_fixture();
    refresh_hierarchy_rollups(&mut run);
    run.tasks[1].superseded_by = Some("replacement-parent".to_string());
    assert!(append_task_qa_task(&mut run, &source, "Stale review"));

    refresh_hierarchy_rollups(&mut run);

    assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
    assert_eq!(run.tasks[1].status, TASK_STATUS_DONE);
    assert_eq!(pick_next_task_index(&run), None);
}

#[test]
fn post_task_qa_rollup_recovery_requires_proven_automatic_qa_lineage() {
    for case in [
        "ordinary-child",
        "missing-source",
        "unfinished-source",
        "unexecuted-source",
        "superseded-source",
        "other-source-parent",
        "stale-qa-scope",
        "missing-dependency",
        "another-unfinished-child",
        "revoked-parent",
        "stale-parent-scope",
        "parent-error",
    ] {
        let (mut run, source) = post_task_qa_rollup_fixture();
        refresh_hierarchy_rollups(&mut run);
        append_task_qa_task(&mut run, &source, "Previously inserted too late");
        match case {
            "ordinary-child" => run.tasks[3].task_origin = "manual".to_string(),
            "missing-source" => run.tasks[3].source_task_id = Some("missing".to_string()),
            "unfinished-source" => run.tasks[2].status = TASK_STATUS_BLOCKED.to_string(),
            "unexecuted-source" => run.tasks[2].attempt_count = 0,
            "superseded-source" => run.tasks[2].superseded_by = Some("replacement".to_string()),
            "other-source-parent" => run.tasks[2].hierarchy.parent_id = Some("other".to_string()),
            "stale-qa-scope" => run.tasks[3].hierarchy.scope_version += 1,
            "missing-dependency" => {
                run.tasks[3].depends_on.clear();
                run.tasks[3].hierarchy.blocked_by.clear();
            }
            "another-unfinished-child" => {
                let mut child = source.clone();
                child.id = "ordinary-incomplete-work".to_string();
                child.status = TASK_STATUS_BLOCKED.to_string();
                run.tasks.push(child);
            }
            "revoked-parent" => {
                run.tasks[1]
                    .hierarchy
                    .scope_approval
                    .as_mut()
                    .unwrap()
                    .approved = false
            }
            "stale-parent-scope" => run.tasks[1].hierarchy.scope_version += 1,
            "parent-error" => {
                run.tasks[1].error = Some("Planning error: independent blocker".to_string())
            }
            _ => unreachable!(),
        }

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE, "{case}");
        assert_eq!(run.tasks[1].status, TASK_STATUS_DONE, "{case}");
        assert!(!task_ancestors_are_approved(&run, &run.tasks[3]), "{case}");
        assert_eq!(pick_next_task_index(&run), None, "{case}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn post_task_qa_rollup_start_then_explicit_todo_recovers_persisted_parent_error() {
    let root = std::env::temp_dir().join(format!("iowb-post-task-qa-{}", Uuid::new_v4()));
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
    let (mut run, source) = post_task_qa_rollup_fixture();
    run.project_path = project.display().to_string();
    run.user_id = Some("qa-recovery-user".to_string());
    run.bootstrap_complete = true;
    run.auto_retry = json!({"enabled": false});
    refresh_hierarchy_rollups(&mut run);
    append_task_qa_task(&mut run, &source, "Previously inserted too late");
    let qa_id = run.tasks[3].id.clone();
    run.status = TASK_STATUS_BLOCKED.to_string();
    run.active = false;
    save_board(&state, &run).unwrap();
    let loaded = load_user_board(&state, "qa-recovery-user", &run.id)
        .unwrap()
        .board;
    assert_eq!(loaded.tasks[3].status, TASK_STATUS_BLOCKED);
    assert!(
        loaded.tasks[3]
            .error
            .as_deref()
            .unwrap()
            .contains("not approved (done)")
    );
    assert_eq!(loaded.tasks[3].attempt_count, 0);
    let completed_leaf = serde_json::to_value(&loaded.tasks[2]).unwrap();

    // The normal Start action refreshes derived parents without executing or
    // silently retrying the previously blocked QA. The user can then move
    // that never-started QA to Todo through the ordinary status endpoint.
    start_board_execution(&state, "qa-recovery-user", &run.id).unwrap();
    timeout(Duration::from_secs(5), async {
        loop {
            let current = load_user_board(&state, "qa-recovery-user", &run.id).unwrap();
            if !current.board.active {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Start should settle without a provider call");
    let settled = load_user_board(&state, "qa-recovery-user", &run.id)
        .unwrap()
        .board;
    assert_eq!(settled.tasks[1].status, TASK_STATUS_BLOCKED);
    assert!(task_ancestors_are_approved(&settled, &settled.tasks[3]));
    assert_eq!(settled.tasks[3].attempt_count, 0);
    let _ = update_task_status(
        &state,
        "qa-recovery-user",
        &run.id,
        &[qa_id],
        TASK_STATUS_TODO,
    )
    .expect("normal explicit Move to Todo is now allowed");
    let updated = load_user_board(&state, "qa-recovery-user", &run.id)
        .unwrap()
        .board;
    assert_eq!(updated.tasks[3].status, TASK_STATUS_TODO);
    assert!(updated.tasks[3].error.is_none());
    assert_eq!(updated.tasks[3].attempt_count, 0);
    assert_eq!(pick_next_task_index(&updated), Some(3));
    assert_eq!(
        serde_json::to_value(&updated.tasks[2]).unwrap(),
        completed_leaf
    );
    let _ = fs::remove_dir_all(root);
}

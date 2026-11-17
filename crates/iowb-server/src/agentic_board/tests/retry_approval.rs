fn retry_hierarchy_fixture() -> AgenticBoard {
    let mut run = board_fixture(json!({
        "command": "Build Pocket Tasks",
        "projectPath": "/tmp/pocket-tasks"
    }));
    run.status = "blocked".to_string();
    run.active = false;
    run.tasks[0].status = TASK_STATUS_TODO.to_string();
    let story_id = run.tasks[0].id.clone();
    let mut parent = BoardTask::draft(
        &mut run,
        "App shell".to_string(),
        "Build the shell".to_string(),
    );
    parent.status = TASK_STATUS_TODO.to_string();
    parent.hierarchy.level = TASK_LEVEL_TASK.to_string();
    parent.hierarchy.parent_id = Some(story_id);
    parent.hierarchy.executable = false;
    let parent_id = parent.id.clone();
    run.tasks.push(parent);
    for title in ["Create shell", "Style shell"] {
        let mut child = BoardTask::draft(
            &mut run,
            title.to_string(),
            "Implement the shell".to_string(),
        );
        child.status = TASK_STATUS_BLOCKED.to_string();
        child.error = Some("provider run failed".to_string());
        child.summary =
            "Provider completed without returning the required task result JSON.".to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id.clone());
        child.hierarchy.executable = true;
        child.attempt_count = 1;
        child.hierarchy.attempts.push(json!({
            "attemptNumber": 1,
            "startedAt": "2026-09-28T08:37:31Z",
            "status": "blocked",
            "error": "provider run failed"
        }));
        run.tasks.push(child);
    }
    refresh_hierarchy_rollups(&mut run);
    assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
    assert_eq!(run.tasks[1].status, TASK_STATUS_BLOCKED);
    run
}

#[test]
fn transient_retry_preserves_approved_blocked_task_and_story_scopes() {
    let mut run = retry_hierarchy_fixture();
    let child_id = run.tasks[2].id.clone();
    assert_eq!(
        retry_attention_tasks_in_board(
            &mut run,
            &[child_id],
            RETRY_MODE_TRANSIENT,
            None,
            "retry provider"
        )
        .unwrap(),
        1
    );
    normalize_board_hierarchy(&mut run);
    refresh_hierarchy_rollups(&mut run);
    assert_eq!(run.tasks[0].status, TASK_STATUS_IN_PROGRESS);
    assert_eq!(run.tasks[1].status, TASK_STATUS_IN_PROGRESS);
    assert_eq!(run.tasks[2].status, TASK_STATUS_TODO);
    assert!(task_is_runnable_in_board(&run, &run.tasks[2]));
    assert_eq!(run.tasks[3].status, TASK_STATUS_BLOCKED);
    assert_eq!(run.tasks[2].attempt_count, 1);
    assert_eq!(
        run.tasks[2].hierarchy.attempts.last().unwrap()["kind"],
        "retry_request"
    );
}

#[test]
fn board_wide_retry_recovers_rollups_without_approving_backlog_scope() {
    let mut run = retry_hierarchy_fixture();
    let backlog = BoardTask::draft(
        &mut run,
        "Unapproved new feature".to_string(),
        "Needs approval".to_string(),
    );
    run.tasks.push(backlog);
    assert_eq!(
        retry_attention_tasks_in_board(&mut run, &[], RETRY_MODE_TRANSIENT, None, "retry provider")
            .unwrap(),
        2
    );
    normalize_board_hierarchy(&mut run);
    assert!(task_is_runnable_in_board(&run, &run.tasks[2]));
    assert!(task_is_runnable_in_board(&run, &run.tasks[3]));
    assert_eq!(run.tasks[4].status, TASK_STATUS_BACKLOG);
    assert!(
        !run.tasks[4]
            .hierarchy
            .scope_approval
            .as_ref()
            .unwrap()
            .approved
    );
}

#[test]
fn legacy_blocked_scope_migration_uses_real_execution_evidence() {
    let mut run = retry_hierarchy_fixture();
    for task in &mut run.tasks {
        task.hierarchy.scope_approval = None;
    }
    // Reproduce the old broken retry: it replaced the leaf's error but kept
    // the actual attempt and result summary, and did not call a provider.
    run.tasks[2].error = Some("Parent is not approved (blocked)".to_string());
    run.tasks[2]
        .hierarchy
        .attempts
        .push(json!({"kind": "retry_request", "retryNumber": 1}));
    let mut run: AgenticBoard = serde_json::from_value(serde_json::to_value(run).unwrap()).unwrap();
    normalize_board_hierarchy(&mut run);
    assert!(
        run.tasks[0]
            .hierarchy
            .scope_approval
            .as_ref()
            .unwrap()
            .approved
    );
    assert!(
        run.tasks[1]
            .hierarchy
            .scope_approval
            .as_ref()
            .unwrap()
            .approved
    );
    let child_id = run.tasks[2].id.clone();
    retry_attention_tasks_in_board(
        &mut run,
        &[child_id],
        RETRY_MODE_TRANSIENT,
        None,
        "retry provider",
    )
    .unwrap();
    normalize_board_hierarchy(&mut run);
    assert!(task_is_runnable_in_board(&run, &run.tasks[2]));
}

#[test]
fn retry_requests_alone_do_not_migrate_unapproved_legacy_parents() {
    let mut run = retry_hierarchy_fixture();
    for task in &mut run.tasks {
        task.hierarchy.scope_approval = None;
        task.attempt_count = 0;
        task.hierarchy.attempts = vec![json!({"kind": "retry_request", "retryNumber": 1})];
    }
    let child_id = run.tasks[2].id.clone();
    let error = retry_attention_tasks_in_board(
        &mut run,
        &[child_id],
        RETRY_MODE_TRANSIENT,
        None,
        "retry provider",
    )
    .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert!(error.body.error.contains("parent"));
    assert_eq!(run.tasks[2].status, TASK_STATUS_BLOCKED);
}

#[test]
fn retry_does_not_bypass_real_parent_approval_boundaries() {
    for boundary in [
        "backlog",
        "done",
        "planning",
        "side_effect",
        "revoked",
        "new_version",
    ] {
        let mut run = retry_hierarchy_fixture();
        match boundary {
            "backlog" => run.tasks[0].status = TASK_STATUS_BACKLOG.to_string(),
            "done" => run.tasks[0].status = TASK_STATUS_DONE.to_string(),
            "planning" => {
                run.tasks[0].error =
                    Some("Planning error: acceptance criteria conflict".to_string())
            }
            "side_effect" => {
                run.tasks[0].error =
                    Some("External side-effect approval required: deployment".to_string());
                run.tasks[0].hierarchy.side_effects = vec!["deployment".to_string()];
            }
            "revoked" => {
                run.tasks[0]
                    .hierarchy
                    .scope_approval
                    .as_mut()
                    .unwrap()
                    .approved = false
            }
            "new_version" => run.tasks[0].hierarchy.scope_version += 1,
            _ => unreachable!(),
        }
        let child_id = run.tasks[2].id.clone();
        let error = retry_attention_tasks_in_board(
            &mut run,
            &[child_id],
            RETRY_MODE_TRANSIENT,
            None,
            "retry provider",
        )
        .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT, "{boundary}");
        assert_eq!(run.tasks[2].status, TASK_STATUS_BLOCKED, "{boundary}");
        assert_eq!(run.tasks[2].hierarchy.attempts.len(), 1, "{boundary}");
        assert_eq!(reset_attention_tasks_for_retry(&mut run), 0, "{boundary}");
        refresh_hierarchy_rollups(&mut run);
        assert!(
            !task_ancestors_are_approved(&run, &run.tasks[2]),
            "{boundary}"
        );
    }
}

#[test]
fn retry_rejects_subtask_side_effect_and_planning_blockers() {
    for blocker in ["side_effect", "planning"] {
        let mut run = retry_hierarchy_fixture();
        if blocker == "side_effect" {
            run.tasks[2].hierarchy.side_effects = vec!["database schema".to_string()];
        } else {
            run.tasks[2].error = Some("Planning error: provider scope invalid".to_string());
        }
        let child_id = run.tasks[2].id.clone();
        assert!(
            retry_attention_tasks_in_board(
                &mut run,
                &[child_id],
                RETRY_MODE_TRANSIENT,
                None,
                "retry provider"
            )
            .is_err(),
            "{blocker}"
        );
        assert_eq!(run.tasks[2].status, TASK_STATUS_BLOCKED, "{blocker}");
    }
}

#[test]
fn automatic_retry_recovers_previously_approved_rollup_parents() {
    let mut run = retry_hierarchy_fixture();
    assert!(run_has_transient_retryable_attention(&run));
    assert_eq!(reset_attention_tasks_for_retry(&mut run), 2);
    normalize_board_hierarchy(&mut run);
    assert_eq!(run.tasks[0].status, TASK_STATUS_IN_PROGRESS);
    assert!(task_is_runnable_in_board(&run, &run.tasks[2]));
    assert!(task_is_runnable_in_board(&run, &run.tasks[3]));
}

#[test]
fn approved_rollup_can_recover_after_dependency_is_resolved_without_retry() {
    let mut run = retry_hierarchy_fixture();
    run.tasks[2].status = TASK_STATUS_TODO.to_string();
    run.tasks[2].error = None;
    run.tasks[2].hierarchy.blocked_by = vec![run.tasks[3].id.clone()];
    run.tasks[3].status = TASK_STATUS_TODO.to_string();
    run.tasks[3].error = None;
    normalize_board_hierarchy(&mut run);
    refresh_hierarchy_rollups(&mut run);
    assert_eq!(run.tasks[2].status, TASK_STATUS_TODO);
    assert!(task_is_runnable_in_board(&run, &run.tasks[3]));
    run.tasks[3].status = TASK_STATUS_DONE.to_string();
    refresh_hierarchy_rollups(&mut run);
    assert!(task_rollup_child_is_eligible(&run, &run.tasks[2]));
}

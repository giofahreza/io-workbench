fn dependency_message_fixture() -> (AgenticBoard, String, String, String) {
    let mut run = board_fixture(json!({
        "command": "Verify the static shell after its test baseline and HTML are complete",
        "projectPath": "/tmp/dependency-message-fixture"
    }));
    run.tasks[0].status = TASK_STATUS_TODO.into();
    let story_id = run.tasks[0].id.clone();
    let mut parent_ids = Vec::new();
    for title in ["Capture baseline", "Create HTML shell", "Verify completed shell"] {
        let mut parent = BoardTask::draft(&mut run, title.into(), title.into());
        parent.status = TASK_STATUS_TODO.into();
        parent.hierarchy.level = TASK_LEVEL_TASK.into();
        parent.hierarchy.parent_id = Some(story_id.clone());
        parent.hierarchy.executable = false;
        parent_ids.push(parent.id.clone());
        run.tasks.push(parent);
    }
    let mut leaf = BoardTask::draft(&mut run, "Verify static shell".into(), "Check the completed shell without editing source".into());
    leaf.status = TASK_STATUS_TODO.into();
    leaf.hierarchy.level = TASK_LEVEL_SUBTASK.into();
    leaf.hierarchy.parent_id = Some(parent_ids[2].clone());
    leaf.hierarchy.executable = true;
    leaf.depends_on = parent_ids[..2].to_vec();
    leaf.hierarchy.blocked_by = leaf.depends_on.clone();
    let leaf_id = leaf.id.clone();
    run.tasks.push(leaf);
    reconcile_scope_approval_records(&mut run);
    mark_dependency_blockers(&mut run);
    (run, parent_ids[0].clone(), parent_ids[1].clone(), leaf_id)
}

#[test]
fn dependency_message_partial_completion_refreshes_only_the_remaining_error() {
    let (mut run, baseline, html, leaf_id) = dependency_message_fixture();
    assert_eq!(run.tasks.last().unwrap().error.as_deref(), Some(format!("Waiting on dependency: {baseline}, {html}").as_str()));
    run.tasks.iter_mut().find(|task| task.id == baseline).unwrap().status = TASK_STATUS_DONE.into();
    let mut expected = run.tasks.iter().find(|task| task.id == leaf_id).unwrap().clone();
    expected.error = Some(format!("Waiting on dependency: {html}"));
    let logs_before = serde_json::to_value(&run.logs).unwrap();

    reconcile_dependency_statuses(&mut run);

    assert_eq!(serde_json::to_value(run.tasks.iter().find(|task| task.id == leaf_id).unwrap()).unwrap(), serde_json::to_value(expected).unwrap(), "partial completion updates the reason only, not status, approval, attempts, scope, or evidence");
    assert_eq!(serde_json::to_value(&run.logs).unwrap(), logs_before, "a still-blocked task is not announced as unblocked");
    let after = serde_json::to_value(&run).unwrap();
    reconcile_dependency_statuses(&mut run);
    assert_eq!(serde_json::to_value(&run).unwrap(), after, "unchanged blocker reconciliation is idempotent");
}

#[test]
fn dependency_message_rechecks_missing_and_superseded_remaining_dependencies() {
    let (mut run, baseline, html, leaf_id) = dependency_message_fixture();
    run.tasks.iter_mut().find(|task| task.id == baseline).unwrap().status = TASK_STATUS_DONE.into();
    let html_task = run.tasks.iter().find(|task| task.id == html).unwrap().clone();
    run.tasks.retain(|task| task.id != html);
    reconcile_dependency_statuses(&mut run);
    assert_eq!(run.tasks.iter().find(|task| task.id == leaf_id).unwrap().error.as_deref(), Some(format!("Missing dependency: {html}").as_str()));

    run.tasks.push(html_task);
    reconcile_dependency_statuses(&mut run);
    assert_eq!(run.tasks.iter().find(|task| task.id == leaf_id).unwrap().error.as_deref(), Some(format!("Waiting on dependency: {html}").as_str()));
    let html_task = run.tasks.iter_mut().find(|task| task.id == html).unwrap();
    html_task.status = TASK_STATUS_DONE.into();
    html_task.superseded_by = Some("replacement-html".into());
    reconcile_dependency_statuses(&mut run);
    let dependent = run.tasks.iter().find(|task| task.id == leaf_id).unwrap();
    assert_eq!(dependent.status, TASK_STATUS_BLOCKED);
    assert_eq!(dependent.error.as_deref(), Some(format!("Superseded dependency: {html} (replacement replacement-html). Choose the replacement dependency or remove the blocker.").as_str()));
    assert_eq!(dependent.depends_on, vec![baseline, html], "reason refresh cannot silently relink dependencies");
}

#[test]
fn dependency_message_keeps_retry_fix_dependency_until_it_really_finishes() {
    let (mut run, baseline, html, leaf_id) = dependency_message_fixture();
    for task in &mut run.tasks {
        if task.id == baseline || task.id == html { task.status = TASK_STATUS_DONE.into(); }
    }
    let mut fix = BoardTask::draft(&mut run, "Resolve the failed check".into(), "Perform approved repair".into());
    fix.status = TASK_STATUS_TODO.into();
    let fix_id = fix.id.clone();
    run.tasks.push(fix);
    run.tasks.iter_mut().find(|task| task.id == leaf_id).unwrap().hierarchy.attempts.push(json!({
        "kind": "retry_request", "mode": RETRY_MODE_FIX, "fixTaskId": fix_id
    }));
    reconcile_dependency_statuses(&mut run);
    let dependent = run.tasks.iter().find(|task| task.id == leaf_id).unwrap();
    assert_eq!(dependent.status, TASK_STATUS_BLOCKED);
    assert_eq!(dependent.error.as_deref(), Some(format!("Waiting on dependency: {fix_id}").as_str()));
    assert_eq!(dependent.attempt_count, 0);
    run.tasks.iter_mut().find(|task| task.id == fix_id).unwrap().status = TASK_STATUS_DONE.into();
    reconcile_dependency_statuses(&mut run);
    let dependent = run.tasks.iter().find(|task| task.id == leaf_id).unwrap();
    assert_eq!(dependent.status, TASK_STATUS_TODO);
    assert_eq!(dependent.error, None);
    assert_eq!(dependent.attempt_count, 0);
}

#[test]
fn dependency_message_preserves_unrelated_errors_and_nonblocked_or_parent_records() {
    for mode in ["provider_error", "planning_error", "done", "backlog", "parent"] {
        let (mut run, baseline, html, leaf_id) = dependency_message_fixture();
        for task in &mut run.tasks {
            if task.id == baseline || task.id == html { task.status = TASK_STATUS_DONE.into(); }
        }
        let dependent = run.tasks.iter_mut().find(|task| task.id == leaf_id).unwrap();
        match mode {
            "provider_error" => dependent.error = Some("Provider run failed".into()),
            "planning_error" => dependent.error = Some("Planning error: unresolved scope".into()),
            "done" => dependent.status = TASK_STATUS_DONE.into(),
            "backlog" => dependent.status = TASK_STATUS_BACKLOG.into(),
            "parent" => { dependent.hierarchy.executable = false; dependent.hierarchy.level = TASK_LEVEL_TASK.into(); }
            _ => unreachable!(),
        }
        let before = serde_json::to_value(&run).unwrap();
        reconcile_dependency_statuses(&mut run);
        assert_eq!(serde_json::to_value(&run).unwrap(), before, "{mode}");
    }
}

#[test]
fn dependency_message_all_completed_keeps_the_existing_unblock_transition() {
    let (mut run, baseline, html, leaf_id) = dependency_message_fixture();
    for task in &mut run.tasks {
        if task.id == baseline || task.id == html { task.status = TASK_STATUS_DONE.into(); }
    }
    let mut expected = run.tasks.iter().find(|task| task.id == leaf_id).unwrap().clone();
    expected.status = TASK_STATUS_TODO.into();
    expected.error = None;
    expected.completed_at = None;
    reconcile_dependency_statuses(&mut run);
    assert_eq!(serde_json::to_value(run.tasks.iter().find(|task| task.id == leaf_id).unwrap()).unwrap(), serde_json::to_value(expected).unwrap());
}

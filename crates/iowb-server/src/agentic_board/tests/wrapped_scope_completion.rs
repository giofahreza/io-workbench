fn approve_wrapped_completion_task(task: &mut BoardTask) {
    task.hierarchy.scope_approval = Some(BoardTaskScopeApproval {
        scope_version: task.hierarchy.scope_version.max(1), approved: true,
    });
}

fn finish_wrapped_completion_leaf(task: &mut BoardTask) {
    task.status = TASK_STATUS_DONE.into();
    task.attempt_count = 1;
    task.completed_at = Some(Utc::now());
    task.result = Some(json!({"status":"done","evidenceGate":{"passed":true}}));
    task.hierarchy.attempts = vec![json!({"status":"done","scopeVersion":task.hierarchy.scope_version,"finishedAt":Utc::now()})];
    approve_wrapped_completion_task(task);
}

// The observed G shape has two approval wrappers: a Story's HTML Task is
// wrapped once, then that Task's executable HTML child is wrapped again.
// Only the second wrapper owns the completed HTML leaf and its required QA.
fn wrapped_completion_fixture() -> (AgenticBoard, String, String, String, String, String) {
    let mut run = board_fixture(json!({"command":"Unapproved full application", "projectPath":"/tmp/wrapped-completion"}));
    let mut story = BoardTask::draft(&mut run, "Approved static shell".into(), "Create index.html without packages or interactions.".into());
    story.status = TASK_STATUS_TODO.into();
    story.hierarchy.level = TASK_LEVEL_STORY.into();
    story.hierarchy.executable = false;
    approve_wrapped_completion_task(&mut story);
    run.tasks.push(story.clone());
    let mut planned = BoardTask::draft(&mut run, "Create static index.html".into(), "Create the requested shell".into());
    planned.hierarchy.required = true;
    planned.task_type = TASK_KIND_IMPLEMENTATION.into();
    let first = wrap_generated_child_in_backlog(&mut run, &story, planned, TASK_LEVEL_TASK, "create static index.html");
    let source_id = first.last().unwrap().id.clone();
    run.tasks.extend(first);
    let source = run.tasks.iter().find(|task| task.id == source_id).unwrap().clone();
    let mut leaf = BoardTask::draft(&mut run, "Write static HTML".into(), "Create only index.html".into());
    leaf.hierarchy.required = true;
    leaf.task_type = TASK_KIND_IMPLEMENTATION.into();
    let second = wrap_generated_child_in_backlog(&mut run, &source, leaf, TASK_LEVEL_SUBTASK, "write static html");
    let wrapper_id = second[0].id.clone();
    let leaf_id = second.last().unwrap().id.clone();
    let leaf_parent_id = second.last().unwrap().hierarchy.parent_id.clone();
    run.tasks.extend(second);
    for task in run.tasks.iter_mut().skip(1) {
        task.status = TASK_STATUS_TODO.into();
        approve_wrapped_completion_task(task);
        // Exercise the deployed historical shape, which predates explicit
        // sourceScopeVersion metadata and has never been scope-edited.
        for entry in &mut task.hierarchy.discussion {
            entry.as_object_mut().unwrap().remove("sourceScopeVersion");
        }
    }
    finish_wrapped_completion_leaf(run.tasks.iter_mut().find(|task| task.id == leaf_id).unwrap());
    let mut qa = BoardTask::draft(&mut run, "QA validate static HTML".into(), "Verify actual HTML and tests".into());
    qa.task_type = TASK_KIND_QA.into();
    qa.task_origin = "system_qa".into();
    qa.task_level_qa = true;
    qa.source_task_id = Some(leaf_id.clone());
    qa.hierarchy.level = TASK_LEVEL_SUBTASK.into();
    qa.hierarchy.parent_id = leaf_parent_id;
    qa.hierarchy.executable = true;
    qa.hierarchy.scope_version = source.hierarchy.scope_version + 1;
    qa.depends_on = vec![leaf_id.clone()];
    qa.hierarchy.blocked_by = qa.depends_on.clone();
    finish_wrapped_completion_leaf(&mut qa);
    let qa_id = qa.id.clone();
    run.tasks.push(qa);
    let mut qa_parent = BoardTask::draft(&mut run, "Original downstream QA".into(), "Run the requested verification".into());
    qa_parent.status = TASK_STATUS_TODO.into();
    qa_parent.hierarchy.level = TASK_LEVEL_TASK.into();
    qa_parent.hierarchy.parent_id = Some(story.id);
    qa_parent.hierarchy.executable = false;
    approve_wrapped_completion_task(&mut qa_parent);
    let qa_parent_id = qa_parent.id.clone();
    run.tasks.push(qa_parent);
    let mut waiting = BoardTask::draft(&mut run, "Capture original green QA".into(), "Verify without changing implementation".into());
    waiting.task_type = TASK_KIND_QA.into();
    waiting.status = TASK_STATUS_BLOCKED.into();
    waiting.hierarchy.level = TASK_LEVEL_SUBTASK.into();
    waiting.hierarchy.parent_id = Some(qa_parent_id);
    waiting.hierarchy.executable = true;
    waiting.depends_on = vec![source_id.clone()];
    waiting.hierarchy.blocked_by = waiting.depends_on.clone();
    waiting.error = Some(format!("Waiting on dependency: {source_id}"));
    approve_wrapped_completion_task(&mut waiting);
    let waiting_id = waiting.id.clone();
    run.tasks.push(waiting);
    (run, source_id, wrapper_id, leaf_id, qa_id, waiting_id)
}

#[test]
fn wrapped_completion_actual_nested_g_shape_releases_original_dependency_without_rerun() {
    let (mut run, source, wrapper, leaf, qa, waiting) = wrapped_completion_fixture();
    let completed_before = [leaf.as_str(), qa.as_str()].map(|id| serde_json::to_value(run.tasks.iter().find(|task| task.id == id).unwrap()).unwrap());
    refresh_hierarchy_rollups(&mut run);
    assert_eq!(run.tasks.iter().find(|task| task.id == wrapper).unwrap().status, TASK_STATUS_DONE);
    assert_eq!(run.tasks.iter().find(|task| task.id == source).unwrap().status, TASK_STATUS_DONE, "the already-completed wrapped work must satisfy the source planning item");
    assert!(unmet_task_dependencies(&run, run.tasks.iter().find(|task| task.id == waiting).unwrap()).is_empty());
    reconcile_dependency_statuses(&mut run);
    assert_eq!(run.tasks.iter().find(|task| task.id == waiting).unwrap().status, TASK_STATUS_TODO);
    assert_eq!(run.tasks[0].status, TASK_STATUS_BACKLOG);
    assert!(!run.tasks[0].hierarchy.scope_approval.as_ref().unwrap().approved);
    assert_eq!([leaf.as_str(), qa.as_str()].map(|id| serde_json::to_value(run.tasks.iter().find(|task| task.id == id).unwrap()).unwrap()), completed_before);
    assert_eq!(run.tasks.iter().find(|task| task.id == source).unwrap().attempt_count, 0);
    // Dependency reconciliation above legitimately changes downstream parent
    // progress on the next pass. Settle that before checking idempotence.
    refresh_hierarchy_rollups(&mut run);
    let settled = serde_json::to_value(&run).unwrap();
    for _ in 0..3 { refresh_hierarchy_rollups(&mut run); }
    assert_eq!(serde_json::to_value(&run).unwrap(), settled, "verified completion is idempotent, not a new execution or approval");
}

#[test]
fn wrapped_completion_requires_current_required_approved_matching_scope() {
    for mode in ["optional", "unapproved", "unrelated", "source_changed", "wrapper_version_changed", "wrapper_edited", "source_unapproved", "source_error"] {
        let (mut run, source, wrapper, _, _, _) = wrapped_completion_fixture();
        if mode == "source_changed" { run.tasks.iter_mut().find(|task| task.id == source).unwrap().hierarchy.scope_version += 1; }
        else if mode == "source_unapproved" { run.tasks.iter_mut().find(|task| task.id == source).unwrap().status = TASK_STATUS_BACKLOG.into(); }
        else if mode == "source_error" { let task = run.tasks.iter_mut().find(|task| task.id == source).unwrap(); task.status = TASK_STATUS_BLOCKED.into(); task.error = Some("Planning error: unresolved scope".into()); }
        else {
            let task = run.tasks.iter_mut().find(|task| task.id == wrapper).unwrap();
            match mode {
                "optional" => task.hierarchy.required = false,
                "unapproved" => { task.status = TASK_STATUS_BACKLOG.into(); task.hierarchy.scope_approval.as_mut().unwrap().approved = false; }
                "unrelated" => task.hierarchy.discussion[0]["sourceParentId"] = json!("unrelated"),
                "wrapper_edited" => task.hierarchy.discussion.push(json!({"kind":"scope_edit"})),
                "wrapper_version_changed" => task.hierarchy.scope_version += 1,
                _ => unreachable!(),
            }
        }
        for _ in 0..3 {
            refresh_hierarchy_rollups(&mut run);
            assert_ne!(run.tasks.iter().find(|task| task.id == source).unwrap().status, TASK_STATUS_DONE, "{mode}");
        }
    }
}

#[test]
fn wrapped_completion_requires_completed_required_qa_and_real_current_leaf_evidence() {
    for mode in ["pending_qa", "failed_qa", "missing_attempt", "wrong_attempt_scope", "failed_evidence", "missing_evidence", "superseded_leaf", "unapproved_done_wrapper"] {
        let (mut run, source, wrapper, leaf, qa, _) = wrapped_completion_fixture();
        let id = if mode.ends_with("qa") { &qa } else if mode == "unapproved_done_wrapper" { &wrapper } else { &leaf };
        let task = run.tasks.iter_mut().find(|task| &task.id == id).unwrap();
        match mode {
            "pending_qa" => task.status = TASK_STATUS_TODO.into(),
            "failed_qa" => { task.status = TASK_STATUS_FAILED.into(); task.error = Some("QA assertion failed".into()); }
            "missing_attempt" => task.attempt_count = 0,
            "wrong_attempt_scope" => task.hierarchy.attempts[0]["scopeVersion"] = json!(999),
            "failed_evidence" => task.result.as_mut().unwrap()["evidenceGate"]["passed"] = json!(false),
            "missing_evidence" => task.result = None,
            "superseded_leaf" => task.superseded_by = Some("replacement".into()),
            "unapproved_done_wrapper" => { task.status = TASK_STATUS_DONE.into(); task.hierarchy.scope_approval.as_mut().unwrap().approved = false; }
            _ => unreachable!(),
        }
        refresh_hierarchy_rollups(&mut run);
        assert_ne!(run.tasks.iter().find(|task| task.id == source).unwrap().status, TASK_STATUS_DONE, "{mode}");
    }
}

#[test]
fn wrapped_completion_does_not_replace_real_unfinished_direct_children() {
    let (mut run, source, _, _, _, _) = wrapped_completion_fixture();
    let mut direct = BoardTask::draft(&mut run, "Additional required direct work".into(), "Preserve the real unfinished work".into());
    direct.status = TASK_STATUS_BLOCKED.into();
    direct.hierarchy.parent_id = Some(source.clone());
    direct.hierarchy.level = TASK_LEVEL_SUBTASK.into();
    direct.hierarchy.executable = true;
    direct.error = Some("Not completed".into());
    approve_wrapped_completion_task(&mut direct);
    run.tasks.push(direct);
    refresh_hierarchy_rollups(&mut run);
    assert_ne!(run.tasks.iter().find(|task| task.id == source).unwrap().status, TASK_STATUS_DONE);
}

#[test]
fn wrapped_completion_waits_for_every_required_wrapper_and_source_ancestor_approval() {
    for mode in ["second_required", "source_ancestor_unapproved", "wrong_explicit_version", "wrong_wrapped_level"] {
        let (mut run, source, wrapper, _, _, _) = wrapped_completion_fixture();
        match mode {
            "second_required" => {
                let mut additional = run.tasks.iter().find(|task| task.id == wrapper).unwrap().clone();
                additional.id = "another-required-wrapper".into();
                additional.status = TASK_STATUS_BACKLOG.into();
                additional.hierarchy.scope_approval.as_mut().unwrap().approved = false;
                additional.hierarchy.discussion[0]["sourceTitleKey"] = json!("another required slice");
                run.tasks.push(additional);
            }
            "source_ancestor_unapproved" => {
                let parent_id = run.tasks.iter().find(|task| task.id == source).unwrap().hierarchy.parent_id.clone().unwrap();
                let parent = run.tasks.iter_mut().find(|task| task.id == parent_id).unwrap();
                parent.status = TASK_STATUS_BACKLOG.into();
                parent.hierarchy.scope_approval.as_mut().unwrap().approved = false;
            }
            "wrong_explicit_version" => run.tasks.iter_mut().find(|task| task.id == wrapper).unwrap().hierarchy.discussion[0]["sourceScopeVersion"] = json!(999),
            "wrong_wrapped_level" => run.tasks.iter_mut().find(|task| task.id == wrapper).unwrap().hierarchy.discussion[0]["wrappedLevel"] = json!("story"),
            _ => unreachable!(),
        }
        refresh_hierarchy_rollups(&mut run);
        assert_ne!(run.tasks.iter().find(|task| task.id == source).unwrap().status, TASK_STATUS_DONE, "{mode}");
    }
}

#[test]
fn wrapped_completion_new_provenance_records_the_source_scope_version() {
    let mut run = board_fixture(json!({"command":"Approved static shell","projectPath":"/tmp/wrapper-metadata"}));
    let source = run.tasks[0].clone();
    let mut wrapper = BoardTask::draft(&mut run, "Scope proposal".into(), "Requested work".into());
    mark_generated_scope_wrapper(&mut wrapper, &source, "requested work", TASK_LEVEL_TASK);
    assert_eq!(wrapper.hierarchy.discussion[0]["sourceScopeVersion"], json!(source.hierarchy.scope_version.max(1)));
}

#[test]
fn wrapped_completion_ambiguous_ancestor_approval_defense_is_per_pass_not_a_new_contract() {
    for mode in ["false", "stale", "missing"] {
        let (mut run, source, _, _, _, _) = wrapped_completion_fixture();
        let parent_id = run.tasks.iter().find(|task| task.id == source).unwrap().hierarchy.parent_id.clone().unwrap();
        let ancestor = run.tasks.iter_mut().find(|task| task.id == parent_id).unwrap();
        assert_eq!(ancestor.status, TASK_STATUS_TODO);
        match mode {
            "false" => ancestor.hierarchy.scope_approval.as_mut().unwrap().approved = false,
            "stale" => ancestor.hierarchy.scope_approval.as_mut().unwrap().scope_version = 999,
            "missing" => ancestor.hierarchy.scope_approval = None,
            _ => unreachable!(),
        }
        refresh_hierarchy_rollups(&mut run);
        assert_ne!(run.tasks.iter().find(|task| task.id == source).unwrap().status, TASK_STATUS_DONE, "{mode}");
        // Todo is the existing approval boundary. Normalization repairs this
        // contradictory fixture; the local guard must not be advertised as
        // persistent protection against that established status contract.
        assert!(run.tasks.iter().find(|task| task.id == parent_id).unwrap().hierarchy.scope_approval.as_ref().unwrap().approved);
        refresh_hierarchy_rollups(&mut run);
        assert_eq!(run.tasks.iter().find(|task| task.id == source).unwrap().status, TASK_STATUS_DONE, "canonical Todo approval after normalization: {mode}");
    }
}

#[test]
fn wrapped_completion_requires_the_original_wrapped_child_scope_not_unrelated_done_work() {
    for mode in ["edited", "unrelated", "missing_provenance"] {
        let (mut run, source, _, leaf, _, _) = wrapped_completion_fixture();
        let task = run.tasks.iter_mut().find(|task| task.id == leaf).unwrap();
        match mode {
            "edited" => {
                task.hierarchy.scope_version += 1;
                task.hierarchy.discussion.push(json!({"kind":"scope_edit"}));
                finish_wrapped_completion_leaf(task);
            }
            "unrelated" => task.hierarchy.discussion[0]["sourceParentId"] = json!("other-source"),
            "missing_provenance" => task.hierarchy.discussion.clear(),
            _ => unreachable!(),
        }
        refresh_hierarchy_rollups(&mut run);
        assert_ne!(run.tasks.iter().find(|task| task.id == source).unwrap().status, TASK_STATUS_DONE, "{mode}");
    }
}

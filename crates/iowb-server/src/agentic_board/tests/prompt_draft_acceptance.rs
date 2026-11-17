fn preview_acceptance_drafts(items: Value) -> Vec<Value> {
    sanitize_prompt_task_drafts(&json!({"tasks":items}), "Add a keyboard hint")
}

#[test]
fn preview_acceptance_preserves_existing_board_dependency_and_generated_origin() {
    let mut run = board_fixture(json!({"command":"Existing keyboard controls","projectPath":"/tmp/project"}));
    let drafts = preview_acceptance_drafts(json!([{"planKey":"hint","title":"Show keyboard shortcut hint","kind":"implementation","dependsOn":["board:task-1"]}]));
    assert_eq!(drafts[0]["blockedBy"], json!([]), "This is the actual preview wire contract, not raw provider JSON");
    assert_eq!(drafts[0]["planDependencies"], json!(["board:task-1"]));
    assert_eq!(accept_previewed_task_drafts(&mut run, drafts, "Add a keyboard hint").unwrap(), 1);
    let added = run.tasks.last().unwrap();
    assert_eq!(task_blockers(added), vec!["task-1".to_string()]);
    assert_eq!(added.task_origin, "user_prompt_generated");
    assert!(added.prompt_task && !added.manual_task);
    assert_eq!(added.status, TASK_STATUS_BACKLOG);
    assert_eq!(added.attempt_count, 0);
    assert!(!task_scope_is_approved(added));
}

#[test]
fn preview_acceptance_resolves_forward_plan_dependencies_in_one_batch() {
    let mut run = board_fixture(json!({"command":"Existing app","projectPath":"/tmp/project"}));
    let drafts = preview_acceptance_drafts(json!([
        {"planKey":"verify","title":"Check hint","dependsOn":["plan:hint"]},
        {"planKey":"hint","title":"Show hint","dependsOn":["board:task-1"]}
    ]));
    accept_previewed_task_drafts(&mut run, drafts, "Add hint").unwrap();
    assert_eq!(task_blockers(&run.tasks[1]), vec![run.tasks[2].id.clone()]);
    assert_eq!(task_blockers(&run.tasks[2]), vec!["task-1".to_string()]);
}

#[test]
fn preview_acceptance_wraps_executable_draft_without_approving_it() {
    let mut run = board_fixture(json!({"command":"Existing app","projectPath":"/tmp/project"}));
    let drafts = preview_acceptance_drafts(json!([{"planKey":"hint","level":"subtask","kind":"implementation","title":"Render hint","dependsOn":["board:task-1"]}]));
    assert_eq!(accept_previewed_task_drafts(&mut run, drafts, "Add hint").unwrap(), 3);
    let added = &run.tasks[1..];
    assert_eq!(added.iter().map(task_level).collect::<Vec<_>>(), vec![TASK_LEVEL_STORY,TASK_LEVEL_TASK,TASK_LEVEL_SUBTASK]);
    assert_eq!(added[1].hierarchy.parent_id.as_deref(), Some(added[0].id.as_str()));
    assert_eq!(added[2].hierarchy.parent_id.as_deref(), Some(added[1].id.as_str()));
    assert!(added.iter().all(|task| task.status == TASK_STATUS_BACKLOG && task.attempt_count == 0 && task.task_origin == "user_prompt_generated"));
    assert!(added.iter().all(|task| !task_scope_is_approved(task)));
    assert!(task_is_executable(&added[2]));
}

#[test]
fn preview_acceptance_rejects_invalid_references_atomically() {
    for reference in ["board:missing", "plan:missing", "task-1", "board:", "plan:hint"] {
        let mut run = board_fixture(json!({"command":"Existing app","projectPath":"/tmp/project"}));
        let before = serde_json::to_value(&run).unwrap();
        let drafts = preview_acceptance_drafts(json!([
            {"planKey":"first","title":"Valid item"},
            {"planKey":"hint","title":"Hint","dependsOn":[reference]}
        ]));
        assert!(accept_previewed_task_drafts(&mut run, drafts, "Add hint").is_err(), "{reference}");
        assert_eq!(serde_json::to_value(&run).unwrap(), before, "{reference}");
    }
}

#[test]
fn preview_acceptance_rejects_missing_duplicate_plan_ids_and_cycles_atomically() {
    for drafts in [
        vec![json!({"title":"Missing key","planDependencies":[]})],
        vec![json!({"planKey":"same","title":"First","planDependencies":[]}),json!({"planKey":"same","title":"Second","planDependencies":[]})],
        preview_acceptance_drafts(json!([
            {"planKey":"first","title":"First","dependsOn":["plan:second"]},
            {"planKey":"second","title":"Second","dependsOn":["plan:first"]}
        ])),
    ] {
        let mut run = board_fixture(json!({"command":"Existing app","projectPath":"/tmp/project"}));
        let before = serde_json::to_value(&run).unwrap();
        assert!(accept_previewed_task_drafts(&mut run, drafts, "Add hint").is_err());
        assert_eq!(serde_json::to_value(&run).unwrap(), before);
    }
}

#[test]
fn preview_acceptance_preserves_execution_controls_and_existing_history() {
    let mut run = board_fixture(json!({"command":"Existing app","projectPath":"/tmp/project"}));
    // The HTTP handler receives a board normalized by load_user_board. Capture
    // that real ingress state, rather than an incomplete fixture approval shape.
    normalize_board_hierarchy(&mut run);
    run.active = true;
    run.status = "running".into();
    run.current_task_id = Some("task-1".into());
    run.control_revision = 41;
    let existing = serde_json::to_value(&run.tasks[0]).unwrap();
    let drafts = preview_acceptance_drafts(json!([{"planKey":"hint","title":"Show hint"}]));
    accept_previewed_task_drafts(&mut run, drafts, "Add hint").unwrap();
    assert!(run.active);
    assert_eq!(run.status, "running");
    assert_eq!(run.current_task_id.as_deref(), Some("task-1"));
    assert_eq!(run.control_revision, 41);
    assert_eq!(serde_json::to_value(&run.tasks[0]).unwrap(), existing);
    assert_eq!(run.tasks[1].attempt_count, 0);
    assert!(!task_scope_is_approved(&run.tasks[1]));
}

#[test]
fn preview_acceptance_rejects_invalid_shape_empty_batches_and_malformed_refs() {
    for drafts in [
        vec![], vec![Value::Null], vec![json!({"planKey":"a","title":"   ","planDependencies":[]})],
        vec![json!({"planKey":"a","title":"Hint"})],
        vec![json!({"planKey":"a","title":"Hint","planDependencies":[17]})],
        vec![json!({"planKey":"a","title":"Hint","planDependencies":[""]})],
        vec![json!({"planKey":"a","title":"Hint","planDependencies":"board:task-1"})],
        vec![json!({"planKey":"a","title":"Hint","planDependencies":[]});101],
    ] {
        let mut run = board_fixture(json!({"command":"Existing app","projectPath":"/tmp/project"}));
        let before = serde_json::to_value(&run).unwrap();
        assert!(accept_previewed_task_drafts(&mut run, drafts, "Add hint").is_err());
        assert_eq!(serde_json::to_value(&run).unwrap(), before);
    }
}

#[test]
fn preview_acceptance_retains_strict_manual_contracts_atomically() {
    let mut run = board_fixture(json!({"command":"Existing app","projectPath":"/tmp/project"}));
    let before = serde_json::to_value(&run).unwrap();
    let invalid = preview_acceptance_drafts(json!([
        {"planKey":"first","title":"Valid shell"},
        {"planKey":"manual","title":"Verify phone","kind":"manual_test","manualVerification":{"version":1,"mode":"ui","viewports":["phone"]}}
    ]));
    assert!(accept_previewed_task_drafts(&mut run, invalid, "Verify phone").is_err());
    assert_eq!(serde_json::to_value(&run).unwrap(), before);
    let valid = preview_acceptance_drafts(json!([{"planKey":"manual","title":"Verify phone","kind":"manual_test","level":"subtask","manualVerification":{"version":1,"mode":"ui","viewports":["360x800","414x896"]}}]));
    accept_previewed_task_drafts(&mut run, valid, "Verify phone").unwrap();
    let leaf = run.tasks.last().unwrap();
    assert_eq!(leaf.task_type, TASK_KIND_MANUAL_TEST);
    assert_eq!(leaf.hierarchy.manual_verification.as_ref().unwrap()["viewports"], json!([{"width":360,"height":800},{"width":414,"height":896}]));
    assert!(!task_scope_is_approved(leaf));
}

#[test]
fn preview_acceptance_does_not_change_ordinary_manual_add_semantics() {
    let mut run = board_fixture(json!({"command":"Existing app","projectPath":"/tmp/project"}));
    let manual = BoardTask::manual(&mut run, serde_json::from_value(json!({
        "title":"A manually authored story","blockedBy":["task-1"],"status":"backlog"
    })).unwrap()).unwrap();
    assert_eq!(manual.task_origin, "user_manual");
    assert_eq!(task_blockers(&manual), vec!["task-1".to_string()]);
    assert!(!task_scope_is_approved(&manual));
}

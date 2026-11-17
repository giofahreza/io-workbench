fn hierarchy_planning_contract_fixture() -> AgenticBoard {
    let mut run = board_fixture(json!({
        "command": "Build Pocket Shopping",
        "projectPath": "/tmp/project",
        "tddEnabled": true
    }));
    let story_id = run.tasks[0].id.clone();
    for (id, kind, description, dependencies) in [
        (
            "tests",
            TASK_KIND_TEST_IMPLEMENTATION,
            "Write executable tests and record the failing baseline.",
            vec![],
        ),
        (
            "implementation",
            TASK_KIND_IMPLEMENTATION,
            "Deliver exactly one implementation subtask for the whole app. Reuse the existing tests and baseline. QA belongs to qa.",
            vec!["tests"],
        ),
        (
            "qa",
            TASK_KIND_QA,
            "Run the full suite and check all app behavior and phone layout.",
            vec!["implementation"],
        ),
    ] {
        let mut task = BoardTask::draft(&mut run, id.to_string(), description.to_string());
        task.id = id.to_string();
        task.description = description.to_string();
        task.task_type = kind.to_string();
        task.hierarchy.level = TASK_LEVEL_TASK.to_string();
        task.hierarchy.parent_id = Some(story_id.clone());
        task.hierarchy.blocked_by = dependencies.into_iter().map(str::to_string).collect();
        task.depends_on = task.hierarchy.blocked_by.clone();
        task.acceptance_criteria = vec![format!("{id} acceptance must be preserved")];
        run.tasks.push(task);
    }
    run
}

#[test]
fn hierarchy_planning_contract_reuses_existing_tdd_stages_without_waiving_gates() {
    let run = hierarchy_planning_contract_fixture();
    let parent = run
        .tasks
        .iter()
        .find(|task| task.id == "implementation")
        .unwrap();
    let prompt = build_hierarchy_breakdown_prompt(&run, parent, TASK_LEVEL_SUBTASK);
    assert!(prompt.contains("Existing prerequisite: tests [test_implementation]"));
    assert!(prompt.contains("Existing downstream verification: qa [qa]"));
    assert!(prompt.contains("tests acceptance must be preserved"));
    assert!(prompt.contains("Do not duplicate an existing test/baseline prerequisite"));
    assert!(prompt.contains("Preserve an explicit one-implementation-subtask scope"));
    assert!(prompt.contains("Planned prerequisites are not evidence of a completed baseline"));
    assert!(prompt.contains("all applicable completion and green-test gates still apply"));
    assert!(!prompt.contains("order at this hierarchy level"));
    assert!(!prompt.contains("Carry this requirement into lower-level breakdowns"));
}

#[test]
fn hierarchy_planning_contract_context_follows_dependencies_not_unrelated_task_names() {
    let mut run = hierarchy_planning_contract_fixture();
    let implementation = run
        .tasks
        .iter_mut()
        .find(|task| task.id == "implementation")
        .unwrap();
    implementation.hierarchy.blocked_by.clear();
    implementation.depends_on.clear();
    let parent = implementation.clone();
    let prompt = build_hierarchy_breakdown_prompt(&run, &parent, TASK_LEVEL_SUBTASK);
    assert!(!prompt.contains("Existing prerequisite: tests [test_implementation]"));
    assert!(prompt.contains("If a required stage is missing, identify the missing prerequisite"));

    let mut intermediary = run
        .tasks
        .iter()
        .find(|task| task.id == "tests")
        .unwrap()
        .clone();
    intermediary.id = "test-evidence".to_string();
    intermediary.task_type = TASK_KIND_REVIEW.to_string();
    intermediary.hierarchy.blocked_by = vec!["tests".to_string()];
    intermediary.depends_on = intermediary.hierarchy.blocked_by.clone();
    run.tasks.push(intermediary);
    let implementation = run
        .tasks
        .iter_mut()
        .find(|task| task.id == "implementation")
        .unwrap();
    implementation.hierarchy.blocked_by = vec!["test-evidence".to_string()];
    implementation.depends_on = implementation.hierarchy.blocked_by.clone();
    let parent = implementation.clone();
    let prompt = build_hierarchy_breakdown_prompt(&run, &parent, TASK_LEVEL_SUBTASK);
    assert!(prompt.contains("Existing prerequisite: tests [test_implementation]"));
    assert!(prompt.contains("Existing prerequisite: test-evidence [review]"));
}

#[test]
fn hierarchy_planning_contract_does_not_expand_test_or_qa_parents_into_new_lifecycles() {
    let run = hierarchy_planning_contract_fixture();
    let tests = run.tasks.iter().find(|task| task.id == "tests").unwrap();
    let prompt = build_hierarchy_breakdown_prompt(&run, tests, TASK_LEVEL_SUBTASK);
    assert!(prompt.contains("Refine only this test-writing/baseline stage"));
    assert!(
        prompt.contains("Do not add production implementation or post-implementation QA children")
    );
    let qa = run.tasks.iter().find(|task| task.id == "qa").unwrap();
    let prompt = build_hierarchy_breakdown_prompt(&run, qa, TASK_LEVEL_SUBTASK);
    assert!(prompt.contains("Refine only this verification stage"));
    assert!(
        prompt.contains("Do not create a fresh pre-development baseline or implementation stage")
    );
}

fn hierarchy_planning_contract_scope_task(description: &str) -> BoardTask {
    let mut run = hierarchy_planning_contract_fixture();
    let mut task = BoardTask::draft(
        &mut run,
        "Re-run existing Node test suite".to_string(),
        description.to_string(),
    );
    task.description = description.to_string();
    task.task_type = TASK_KIND_QA.to_string();
    task.hierarchy.required = true;
    task.hierarchy.side_effects.clear();
    task
}

#[test]
fn hierarchy_planning_contract_does_not_wrap_explicit_elsewhere_scope_boundary() {
    let task = hierarchy_planning_contract_scope_task(
        "Re-run the existing Node test suite and confirm all tests pass. Broader QA (manual viewport checks across breakpoints and end-to-end acceptance walkthrough) remains the responsibility of task-1-task-1.3 and is out of scope here.",
    );
    assert!(!generated_child_requires_backlog_approval(&task));
}

#[test]
fn hierarchy_planning_contract_elsewhere_boundary_does_not_hide_other_unapproved_scope() {
    let boundary =
        "Broader QA remains the responsibility of task-1-task-1.3 and is out of scope here.";
    for extra in [
        "Add out of scope analytics.",
        "Add an out-of-scope theme switcher.",
        "This is optional work.",
        "Add a nice-to-have export button.",
    ] {
        let task = hierarchy_planning_contract_scope_task(&format!("{boundary}\n{extra}"));
        assert!(generated_child_requires_backlog_approval(&task), "{extra}");
    }
    let mut task = hierarchy_planning_contract_scope_task(boundary);
    task.title = "Implement out of scope analytics".to_string();
    assert!(generated_child_requires_backlog_approval(&task));
}

#[test]
fn hierarchy_planning_contract_retains_structured_and_external_approval_guards() {
    let mut task = hierarchy_planning_contract_scope_task(
        "Broader QA remains the responsibility of task-1-task-1.3 and is out of scope here.",
    );
    task.hierarchy.required = false;
    assert!(generated_child_requires_backlog_approval(&task));
    task.hierarchy.required = true;
    task.hierarchy.side_effects = vec!["Update shared deployment".to_string()];
    assert!(generated_child_requires_backlog_approval(&task));
    task.hierarchy.side_effects.clear();
    task.description.push_str(" Change production config.");
    assert!(generated_child_requires_backlog_approval(&task));
    task.description.clear();
    task.task_type = TASK_KIND_MIGRATION.to_string();
    assert!(generated_child_requires_backlog_approval(&task));
}

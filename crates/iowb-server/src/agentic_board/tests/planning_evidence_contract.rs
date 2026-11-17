fn planning_evidence_contract_prompts(run: &AgenticBoard, request: &str) -> [String; 2] {
    [
        build_hierarchy_breakdown_prompt(run, &run.tasks[0], TASK_LEVEL_TASK),
        build_prompt_task_draft_prompt(run, request, "complete_app"),
    ]
}

#[test]
fn planning_evidence_contract_reaches_hierarchy_and_prompt_drafts() {
    let run = board_fixture(json!({
        "command": "Build a small grocery list",
        "projectPath": "/tmp/project",
        "tddEnabled": true
    }));
    for prompt in planning_evidence_contract_prompts(&run, "Build a small grocery list") {
        assert!(prompt.contains("Deterministic TDD evidence contract"));
        assert!(prompt.contains("Node built-in test runner's complete TAP/spec report"));
        assert!(prompt.contains("Playwright's JSON reporter"));
        assert!(prompt.contains("test:ui or test:e2e"));
        assert!(prompt.contains("real executed behavior assertions"));
        assert!(
            prompt.contains("missing imports, syntax errors, missing runners, skips or timeouts")
        );
        assert!(prompt.contains("Do not fabricate or translate reports"));
        assert!(prompt.contains("final implementation and QA still require green checks"));
    }
}

#[test]
fn planning_evidence_contract_preserves_explicit_existing_harness_constraints() {
    let request = "Use the existing Vitest suite; do not migrate or replace it.";
    let mut run = board_fixture(json!({
        "command": request,
        "projectPath": "/tmp/project",
        "tddEnabled": true
    }));
    run.tasks[0].description = request.to_string();
    for prompt in planning_evidence_contract_prompts(&run, request) {
        assert!(prompt.contains(request));
        assert!(prompt.contains("Preserve explicit user and existing-harness constraints"));
        assert!(prompt.contains("do not silently migrate, replace or weaken existing tests"));
        assert!(prompt.contains("report the compatibility blocker in the plan"));
        assert!(prompt.contains("separately approved verification scope"));
    }
}

#[test]
fn planning_evidence_contract_discloses_narrow_new_harness_file_authority() {
    let run = board_fixture(json!({
        "command": "Build a small grocery list",
        "projectPath": "/tmp/project",
        "tddEnabled": true
    }));
    for prompt in planning_evidence_contract_prompts(&run, "Build a small grocery list") {
        assert!(prompt.contains("For a new JavaScript harness in a non-Git workspace"));
        assert!(prompt.contains("plannedFiles"));
        assert!(prompt.contains("package.json"));
        assert!(prompt.contains("package-lock.json"));
        assert!(prompt.contains("versioned jsdom or Playwright devDependencies"));
        assert!(prompt.contains("not production/runtime dependencies or lifecycle scripts"));
        assert!(prompt.contains("Reuse compatible existing tests and prerequisite stages"));
        assert!(prompt.contains("do not add another TDD lifecycle inside each task"));
    }
}

#[test]
fn planning_evidence_contract_does_not_enable_a_disabled_red_baseline_policy() {
    for (enabled, require_red, version) in [(false, true, 3), (true, false, 3), (true, true, 2)] {
        let mut run = board_fixture(json!({
            "command": "Build a small grocery list",
            "projectPath": "/tmp/project",
            "tddEnabled": enabled,
            "tddPolicy": { "requireFailingTestBeforeDev": require_red }
        }));
        run.orchestration_version = version;
        for prompt in planning_evidence_contract_prompts(&run, "Build a small grocery list") {
            assert!(!prompt.contains("Deterministic TDD evidence contract"));
        }
    }
}

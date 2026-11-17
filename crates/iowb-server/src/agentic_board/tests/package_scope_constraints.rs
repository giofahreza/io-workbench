const PACKAGE_SCOPE_MANUAL_REQUEST: &str = "Create index.html with a heading Checklist, a labeled item input, an Add button, an empty list, and total and remaining counters at zero. Use plain HTML and CSS, responsive on phones. No backend, external services, packages, deployment, or accounts. Do not implement persistence or task interactions in this task.";

fn package_scope_fixture() -> AgenticBoard {
    let mut run = board_fixture(json!({
        "command": "Build a checklist application",
        "projectPath": "/tmp/package-scope-fixture",
        "tddEnabled": true
    }));
    let story = &mut run.tasks[0];
    story.title = "Create a local checklist page shell".into();
    story.description = PACKAGE_SCOPE_MANUAL_REQUEST.into();
    story.details = story.description.clone();
    story.prompt = story.description.clone();
    story.task_origin = "user_manual".into();
    story.status = TASK_STATUS_TODO.into();
    let mut parent = story.clone();
    parent.id = "generated-tests".into();
    parent.title = "Add the static shell tests".into();
    parent.description = "Write automated structure assertions before implementation.".into();
    parent.details = parent.description.clone();
    parent.prompt.clear();
    parent.task_origin = "hierarchy_breakdown".into();
    parent.task_type = TASK_KIND_TEST_IMPLEMENTATION.into();
    parent.hierarchy.level = TASK_LEVEL_TASK.into();
    parent.hierarchy.parent_id = Some(story.id.clone());
    let mut leaf = parent.clone();
    leaf.id = "generated-harness".into();
    leaf.title = "Add Playwright test harness".into();
    leaf.description =
        "Create package.json with @playwright/test as a devDependency. Run npm install.".into();
    leaf.details = leaf.description.clone();
    leaf.hierarchy.level = TASK_LEVEL_SUBTASK.into();
    leaf.hierarchy.parent_id = Some(parent.id.clone());
    leaf.hierarchy.executable = true;
    leaf.hierarchy.planned_files = vec!["package.json".into(), "playwright.config.js".into()];
    run.tasks.extend([parent, leaf]);
    run
}

#[test]
fn package_scope_recognizes_unqualified_bans_including_actual_oxford_list() {
    for text in [
        PACKAGE_SCOPE_MANUAL_REQUEST,
        "No new packages.",
        "Do not install any dependencies.",
        "Use built-in tooling without third-party packages.",
        "No backend or packages; keep it local.",
    ] {
        assert!(scope_disallows_new_packages(text), "{text}");
    }
    for text in [
        "No runtime dependencies. Test packages are allowed.",
        "No production packages; Playwright devDependencies are permitted.",
        "No external services. Test packages are allowed.",
        "Do not add package.json. Run existing tests.",
    ] {
        assert!(!scope_disallows_new_packages(text), "{text}");
    }
}

#[test]
fn package_scope_must_not_is_a_boundary_and_a_negated_operation() {
    assert!(scope_disallows_new_packages(
        "You must not install new packages."
    ));
    let mut run = package_scope_fixture();
    run.tasks[2].title = "Verify with existing tools".into();
    run.tasks[2].description = "You must not run npm install; inspect existing test output.".into();
    run.tasks[2].details = run.tasks[2].description.clone();
    assert!(task_package_scope_conflict(&run, &run.tasks[2]).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn package_scope_execution_entry_rejects_conflict_before_provider_dispatch() {
    let fixture = BoardReadAccessFixture::new().await;
    let mut run = package_scope_fixture();
    // An intentionally invalid provider makes accidental dispatch fail locally,
    // never contact an upstream or launch a provider process in this regression.
    run.provider = "invalid-provider-package-scope-test".into();
    let result = execute_provider_task_with_retry_instruction(&fixture.state, &run, 2, None).await;
    let Err(error) = result else {
        panic!("package conflict must stop execution")
    };
    assert!(
        error.body.error.starts_with("Package scope conflict:"),
        "{}",
        error.body.error
    );
}

#[test]
fn package_scope_blocks_generated_grandchild_before_provider_or_plan_persistence() {
    let run = package_scope_fixture();
    let issue =
        task_package_scope_conflict(&run, &run.tasks[2]).expect("ancestor ban must block install");
    assert!(issue.contains("task-1"));
    assert!(issue.contains("package"));
    assert!(hierarchy_package_scope_conflict(&run, &run.tasks[1], &run.tasks[2]).is_some());
}

#[test]
fn package_scope_candidate_local_id_cannot_hide_the_real_parent_boundary() {
    let run = package_scope_fixture();
    let mut child = run.tasks[2].clone();
    child.id = run.tasks[0].id.clone();
    child.hierarchy.parent_id = None;
    assert!(hierarchy_package_scope_conflict(&run, &run.tasks[0], &child).is_some());
}

#[test]
fn package_scope_blocks_command_only_install_regardless_of_kind_or_planned_files() {
    let mut run = package_scope_fixture();
    for command in [
        "npm install --no-save @playwright/test",
        "npm exec playwright test",
        "npx playwright test",
        "pnpm add -D jsdom",
    ] {
        let leaf = &mut run.tasks[2];
        leaf.title = "Prepare verification".into();
        leaf.description = command.into();
        leaf.details = command.into();
        leaf.hierarchy.planned_files.clear();
        leaf.task_type = TASK_KIND_IMPLEMENTATION.into();
        assert!(
            task_package_scope_conflict(&run, &run.tasks[2]).is_some(),
            "{command}"
        );
    }
}

#[test]
fn package_scope_negation_and_no_download_flags_are_local_to_the_operation() {
    let mut run = package_scope_fixture();
    for text in [
        "Do not modify production code and run npm install --no-save @playwright/test",
        "Do not change the UI, run npm install",
        "Run npx playwright test and then npm exec --offline other",
        "Run npx playwright test -- --offline",
    ] {
        let leaf = &mut run.tasks[2];
        leaf.title = "Prepare verification".into();
        leaf.description = text.into();
        leaf.details = text.into();
        leaf.hierarchy.planned_files.clear();
        assert!(
            task_package_scope_conflict(&run, &run.tasks[2]).is_some(),
            "{text}"
        );
    }
    for text in [
        "Do not run npm install or npx playwright test",
        "Run npm exec --offline -- playwright test",
        "Do not change the UI and run npx --no-install playwright test",
    ] {
        run.tasks[2].description = text.into();
        run.tasks[2].details = text.into();
        assert!(
            task_package_scope_conflict(&run, &run.tasks[2]).is_none(),
            "{text}"
        );
    }
}

#[test]
fn package_scope_allows_generated_prohibition_lists_without_masking_new_instructions() {
    let mut run = package_scope_fixture();
    for text in [
        "Do not introduce package.json, dependencies, devDependencies, npm install, or any third-party package; the user boundary forbids packages, so use the no-install built-in runner only.",
        "Do not run npm install and npx playwright test.",
        "No npm install, npx playwright test.",
    ] {
        run.tasks[2].title = "Use built-in Node tests".into();
        run.tasks[2].description = text.into();
        run.tasks[2].details = text.into();
        assert!(
            task_package_scope_conflict(&run, &run.tasks[2]).is_none(),
            "{text}"
        );
    }
    for text in [
        "Do not modify production code and run npm install --no-save @playwright/test",
        "Do not change the UI, run npm install",
        "Do not introduce packages, but run npm install",
        "Do not introduce packages; run npm install",
        "Do not introduce packages, then run npm install",
        "Do not introduce packages && npm install",
        "Do not introduce package.json, dependencies, devDependencies. Run npm install",
        "Do not introduce package.json, dependencies, run npm install",
        "Do not modify package.json, npm install",
    ] {
        run.tasks[2].description = text.into();
        run.tasks[2].details = text.into();
        assert!(
            task_package_scope_conflict(&run, &run.tasks[2]).is_some(),
            "{text}"
        );
    }
}

#[test]
fn package_scope_manifest_dependency_prohibition_preserves_actual_generated_negative_list() {
    let mut run = package_scope_fixture();
    let leaf = &mut run.tasks[2];
    leaf.title = "Write structural assertions with Node built-ins".into();
    leaf.description = "Use only built-in node modules (node:test, node:assert, node:fs); do not introduce package.json dependencies, npm install, devDependencies, jsdom, or Playwright.".into();
    leaf.details = leaf.description.clone();
    leaf.acceptance_criteria = vec!["No npm install, no node_modules, no package.json dependencies, no jsdom, no Playwright, and no other external package is added by this ticket".into()];
    leaf.hierarchy.planned_files = vec!["tests/checklist.test.js".into()];
    assert!(!task_requests_new_packages(&run.tasks[2]));
    assert!(task_package_scope_conflict(&run, &run.tasks[2]).is_none());
    assert!(hierarchy_package_scope_conflict(&run, &run.tasks[1], &run.tasks[2]).is_none());

    for text in [
        "Do not introduce package.json dependencies, npm install.",
        "No package.json dependencies, npm install.",
        "Never add package.json dependencies, npm install, or npx playwright test.",
    ] {
        run.tasks[2].description = text.into();
        run.tasks[2].details = text.into();
        assert!(!task_requests_new_packages(&run.tasks[2]), "{text}");
    }
}

#[test]
fn package_scope_manifest_dependency_prohibition_does_not_hide_positive_install() {
    let mut run = package_scope_fixture();
    run.tasks[2].title = "Verify the shell".into();
    for text in [
        "Do not introduce package.json dependencies, run npm install.",
        "Do not introduce package.json dependencies and run npm install.",
        "Do not introduce package.json dependencies, but run npm install.",
        "Do not introduce package.json dependencies; npm install.",
        "Do not introduce package.json dependencies. Run npm install.",
        "Do not introduce package.json dependencies then npm install.",
        "Do not introduce package.json dependencies && npm install.",
        "Do not modify package.json dependencies, npm install.",
        "Do not introduce package.json dependencies documentation, npm install.",
        "Add package.json dependencies and run npm install.",
    ] {
        run.tasks[2].description = text.into();
        run.tasks[2].details = text.into();
        assert!(task_requests_new_packages(&run.tasks[2]), "{text}");
        assert!(task_package_scope_conflict(&run, &run.tasks[2]).is_some(), "{text}");
    }
}

#[test]
fn package_scope_does_not_leak_sibling_constraints_or_override_runtime_only_bans() {
    let mut run = package_scope_fixture();
    run.tasks[1].hierarchy.parent_id = None;
    assert!(task_package_scope_conflict(&run, &run.tasks[2]).is_none());
    run.tasks[1].hierarchy.parent_id = Some("task-1".into());
    for text in [
        "No runtime dependencies. Test packages are allowed.",
        "No external services. Test packages are allowed.",
    ] {
        run.tasks[0].description = text.into();
        run.tasks[0].details = text.into();
        run.tasks[0].prompt = text.into();
        assert!(
            task_package_scope_conflict(&run, &run.tasks[2]).is_none(),
            "{text}"
        );
    }
}

#[test]
fn package_scope_allows_builtin_or_existing_tooling_and_negated_install_instructions() {
    let mut run = package_scope_fixture();
    for text in [
        "Write tests with node --test using built-in modules only.",
        "Run existing Playwright with ./node_modules/.bin/playwright test; do not install packages.",
        "Run npx --no-install playwright test against the existing suite.",
        "Do not add package.json or install packages; inspect the existing test output.",
    ] {
        let leaf = &mut run.tasks[2];
        leaf.title = "Verify the shell".into();
        leaf.description = text.into();
        leaf.details = text.into();
        leaf.hierarchy.planned_files.clear();
        assert!(
            task_package_scope_conflict(&run, &run.tasks[2]).is_none(),
            "{text}"
        );
    }
}

#[test]
fn package_scope_inherits_boundaries_in_planning_and_execution_without_waiving_tdd() {
    let run = package_scope_fixture();
    for prompt in [
        build_hierarchy_breakdown_prompt(&run, &run.tasks[1], TASK_LEVEL_SUBTASK),
        build_task_execution_prompt(&run, &run.tasks[2], 2),
    ] {
        assert!(prompt.contains("Inherited scope boundaries"));
        assert!(
            prompt.contains("No backend, external services, packages, deployment, or accounts")
        );
        assert!(prompt.contains("Do not implement persistence or task interactions in this task"));
        assert!(prompt.contains("do not override these boundaries"));
        assert!(prompt.contains("blocked"));
    }
    assert!(
        build_task_execution_prompt(&run, &run.tasks[2], 2)
            .contains("actual failing command output")
    );
}

#[test]
fn package_scope_test_tooling_allowlist_cannot_accept_forbidden_dependencies() {
    let root = filesystem_evidence_fixture();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    fs::write(
        root.join("package.json"),
        playwright_test_package().to_string(),
    )
    .unwrap();
    let mut run = package_scope_fixture();
    run.project_path = root.to_str().unwrap().into();
    let delta = record_task_workspace_changes(&mut run, "generated-harness", before);
    assert_eq!(delta["testTooling"]["manifestAllowed"], true);
    let result = apply_completion_evidence_gate(
        &run,
        "generated-harness",
        json!({
            "status": "done", "qaResult": "pass", "commandsRun": ["npm test"]
        }),
        &delta,
    );
    assert_eq!(result["filePolicy"]["passed"], false);
    assert_eq!(result["evidenceGate"]["passed"], false);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_scope_builtin_manifest_stays_allowed_without_package_changes() {
    let root = filesystem_evidence_fixture();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    fs::write(
        root.join("package.json"),
        minimal_test_package().to_string(),
    )
    .unwrap();
    let mut run = package_scope_fixture();
    run.project_path = root.to_str().unwrap().into();
    run.tasks[2].title = "Configure built-in Node test runner".into();
    run.tasks[2].description =
        "Add only a local node --test script; do not install packages.".into();
    run.tasks[2].details = run.tasks[2].description.clone();
    let delta = record_task_workspace_changes(&mut run, "generated-harness", before);
    let result = apply_completion_evidence_gate(
        &run,
        "generated-harness",
        json!({
            "status": "done", "qaResult": "pass", "commandsRun": ["node --test"]
        }),
        &delta,
    );
    assert_eq!(result["filePolicy"]["passed"], true);
    assert_eq!(result["evidenceGate"]["passed"], true);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_scope_reused_tooling_evidence_cannot_override_a_new_ancestor_ban() {
    let run = package_scope_fixture();
    for policy in [
        json!({"manifestAllowed": true, "dependencyAdditionsAbsent": false}),
        json!({"manifestAllowed": true}), // Legacy evidence cannot prove no additions.
    ] {
        let summary = json!({"reusedEvidence": [{
            "path": "package.json", "classification": "reused_same_task",
            "afterHash": "retained-evidence-hash", "testToolingPolicy": policy
        }]});
        assert_eq!(
            task_package_file_policy_violations(&run, &run.tasks[2], &summary),
            vec!["package.json"]
        );
    }
    let safe = json!({"reusedEvidence": [{
        "path": "package.json", "classification": "reused_same_task",
        "afterHash": "retained-evidence-hash",
        "testToolingPolicy": {"manifestAllowed": true, "dependencyAdditionsAbsent": true}
    }]});
    assert!(task_package_file_policy_violations(&run, &run.tasks[2], &safe).is_empty());
}

#[test]
fn package_scope_dependency_fingerprints_distinguish_reuse_removal_and_addition() {
    let root = filesystem_evidence_fixture();
    let original = jsdom_test_package();
    fs::write(root.join("package.json"), original.to_string()).unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    for (manifest, additions_absent) in [
        (original.clone(), true),
        (minimal_test_package(), true),
        (playwright_test_package(), false),
        (
            {
                let mut value = original;
                value["dependencies"] = json!({"runtime-package": "1.0.0"});
                value
            },
            false,
        ),
    ] {
        fs::write(root.join("package.json"), manifest.to_string()).unwrap();
        let after = capture_workspace_snapshot(root.to_str().unwrap());
        assert_eq!(
            test_tooling_delta(&before, &after)["dependencyAdditionsAbsent"],
            additions_absent
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_scope_lockfile_only_package_changes_cannot_bypass_manifest_fingerprints() {
    let root = filesystem_evidence_fixture();
    let manifest = playwright_test_package();
    let original = playwright_test_lock(&manifest);
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    fs::write(root.join("package-lock.json"), original.to_string()).unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    let mut run = package_scope_fixture();
    run.project_path = root.to_str().unwrap().into();
    for add_transitive in [false, true] {
        let mut lock = original.clone();
        if add_transitive {
            lock["packages"]["node_modules/playwright-core"]["dependencies"] =
                json!({"new-test-package": "1.0.0"});
            lock["packages"]["node_modules/new-test-package"] =
                json!({"version": "1.0.0", "dev": true});
        } else {
            lock["packages"]["node_modules/playwright-core"]["resolved"] =
                json!("https://registry.example.test/changed.tgz");
        }
        fs::write(root.join("package-lock.json"), lock.to_string()).unwrap();
        let after = capture_workspace_snapshot(root.to_str().unwrap());
        let delta = summarize_workspace_delta("generated-harness", &before, &after);
        assert_eq!(delta["testTooling"]["lockAllowed"], true);
        assert_eq!(
            task_package_file_policy_violations(&run, &run.tasks[2], &delta),
            vec!["package-lock.json"]
        );
    }
    fs::write(root.join("package-lock.json"), original.to_string()).unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let policy = test_tooling_delta(&before, &after);
    assert_eq!(policy["lockDependencyAdditionsAbsent"], true);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_scope_lock_package_removal_or_reuse_still_has_no_additions() {
    let root = filesystem_evidence_fixture();
    let manifest = playwright_test_package();
    let mut lock = playwright_test_lock(&manifest);
    lock["packages"]["node_modules/playwright-core"]["optionalDependencies"] =
        json!({"optional-helper": "1.0.0"});
    lock["packages"]["node_modules/optional-helper"] = json!({"version": "1.0.0", "dev": true});
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    fs::write(root.join("package-lock.json"), lock.to_string()).unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    assert_eq!(
        test_tooling_delta(&before, &before)["lockDependencyAdditionsAbsent"],
        true
    );
    lock["packages"]
        .as_object_mut()
        .unwrap()
        .remove("node_modules/optional-helper");
    fs::write(root.join("package-lock.json"), lock.to_string()).unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    assert_eq!(
        test_tooling_delta(&before, &after)["lockDependencyAdditionsAbsent"],
        true
    );
    fs::remove_dir_all(root).unwrap();
}

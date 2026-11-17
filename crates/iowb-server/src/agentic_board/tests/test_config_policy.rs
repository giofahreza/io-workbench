fn test_config_policy_board(root: &Path) -> AgenticBoard {
    let mut run = board_fixture(json!({
        "command": "Configure the local test runner",
        "projectPath": root.to_str().unwrap()
    }));
    let task = &mut run.tasks[0];
    task.task_type = TASK_KIND_TEST_IMPLEMENTATION.to_string();
    task.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
    task.hierarchy.executable = true;
    task.hierarchy.planned_files = vec!["package.json".to_string()];
    run
}

fn minimal_test_package() -> Value {
    json!({
        "name": "pocket-tasks",
        "version": "1.0.0",
        "private": true,
        "type": "module",
        "scripts": {"test": "node --test"}
    })
}

#[test]
fn test_config_policy_accepts_planned_new_minimal_runner_from_actual_snapshot() {
    let root = filesystem_evidence_fixture();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    fs::write(
        root.join("package.json"),
        minimal_test_package().to_string(),
    )
    .unwrap();
    let mut run = test_config_policy_board(&root);
    let delta = record_task_workspace_changes(&mut run, "task-1", before);
    assert_eq!(
        delta["changedBySubtask"][0]["testRunnerConfig"],
        "minimal-node-test-v1"
    );
    assert!(task_file_edit_policy_violations(&run.tasks[0], &delta).is_empty());
    let result = apply_completion_evidence_gate(
        &run,
        "task-1",
        json!({"status": "done", "qaResult": "pass", "commandsRun": ["npm test"]}),
        &delta,
    );
    assert_eq!(result["filePolicy"]["passed"], true);
    assert_eq!(result["evidenceGate"]["passed"], true);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_rejects_unplanned_configs_and_non_test_source_edits() {
    let root = filesystem_evidence_fixture();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    fs::write(
        root.join("package.json"),
        minimal_test_package().to_string(),
    )
    .unwrap();
    fs::write(root.join("app.js"), "export const production = true;").unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &before, &after);
    let mut run = test_config_policy_board(&root);
    assert_eq!(
        task_file_edit_policy_violations(&run.tasks[0], &delta),
        vec!["app.js"]
    );
    run.tasks[0].hierarchy.planned_files.clear();
    assert_eq!(
        task_file_edit_policy_violations(&run.tasks[0], &delta),
        vec!["app.js", "package.json"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_allows_test_script_change_but_not_production_mutations_or_deletions() {
    let root = filesystem_evidence_fixture();
    let mut manifest = minimal_test_package();
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    manifest["scripts"]["test"] = json!("node --test tests/*.test.js");
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &before, &after);
    let run = test_config_policy_board(&root);
    assert!(task_file_edit_policy_violations(&run.tasks[0], &delta).is_empty());
    manifest["main"] = json!("app.js");
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    let production_changed = capture_workspace_snapshot(root.to_str().unwrap());
    let production_delta = summarize_workspace_delta("task-1", &after, &production_changed);
    assert_eq!(
        task_file_edit_policy_violations(&run.tasks[0], &production_delta),
        vec!["package.json"]
    );
    fs::remove_file(root.join("package.json")).unwrap();
    let deleted = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &after, &deleted);
    assert_eq!(
        task_file_edit_policy_violations(&run.tasks[0], &delta),
        vec!["package.json"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_keeps_qa_and_manual_review_tasks_read_only() {
    let root = filesystem_evidence_fixture();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    fs::write(
        root.join("package.json"),
        minimal_test_package().to_string(),
    )
    .unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &before, &after);
    let mut run = test_config_policy_board(&root);
    for kind in [
        TASK_KIND_QA,
        TASK_KIND_MANUAL_TEST,
        TASK_KIND_REVIEW,
        TASK_KIND_RESEARCH,
        TASK_KIND_DESIGN,
    ] {
        run.tasks[0].task_type = kind.to_string();
        assert_eq!(
            task_file_edit_policy_violations(&run.tasks[0], &delta),
            vec!["package.json"],
            "{kind}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_does_not_trust_provider_claims_or_incomplete_snapshots() {
    let root = filesystem_evidence_fixture();
    let before = capture_filesystem_workspace_snapshot(root.to_str().unwrap(), "not git", 10);
    fs::write(
        root.join("package.json"),
        minimal_test_package().to_string(),
    )
    .unwrap();
    let after = capture_filesystem_workspace_snapshot(root.to_str().unwrap(), "not git", 10);
    let run = test_config_policy_board(&root);
    for unavailable_before in [
        json!({"filesByPath": {}, "error": "no baseline"}),
        json!({"provider": "filesystem sha256", "complete": false, "filesByPath": {}}),
    ] {
        let delta = summarize_workspace_delta("task-1", &unavailable_before, &after);
        assert_eq!(
            task_file_edit_policy_violations(&run.tasks[0], &delta),
            vec!["package.json"]
        );
    }
    let mut delta = summarize_workspace_delta("task-1", &before, &after);
    delta["changedBySubtask"][0]
        .as_object_mut()
        .unwrap()
        .remove("testRunnerConfig");
    delta["testTooling"] = Value::Null;
    let result = apply_completion_evidence_gate(
        &run,
        "task-1",
        json!({
            "status": "done", "qaResult": "pass", "changedFiles": ["package.json"],
            "testRunnerConfig": "minimal-node-test-v1", "evidence": ["Test-only package.json"]
        }),
        &delta,
    );
    assert_eq!(result["filePolicy"]["passed"], false);
    assert_eq!(result["status"], "needs_followup");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_rejects_production_metadata_dependencies_and_lifecycle_scripts() {
    for (key, value) in [
        ("dependencies", json!({"left-pad": "1"})),
        ("devDependencies", json!({"test-runner": "1"})),
        ("main", json!("app.js")),
        ("exports", json!("./app.js")),
        ("workspaces", json!(["packages/*"])),
        ("config", json!({"anything": true})),
    ] {
        let mut manifest = minimal_test_package();
        manifest[key] = value;
        assert!(!is_minimal_node_test_manifest(&manifest), "{key}");
    }
    for script in ["pretest", "posttest", "install", "prepare", "start"] {
        let mut manifest = minimal_test_package();
        manifest["scripts"][script] = json!("node app.js");
        assert!(!is_minimal_node_test_manifest(&manifest), "{script}");
    }
    assert!(!is_minimal_node_test_manifest(
        &json!({"name": "no-runner"})
    ));
}

#[test]
fn test_config_policy_accepts_only_local_node_test_script_arguments() {
    for script in [
        "node --test",
        "node --test tests/*.test.js",
        "node --test --test-reporter=spec tests",
        "node --test --test-reporter=tap test/",
        "node --test --test-reporter=tap test/*.test.js",
        "node --test ./tests/example.test.mjs",
    ] {
        assert!(is_local_node_test_command(script), "{script}");
    }
    for script in [
        "npm install && node --test",
        "node --test; node app.js",
        "node --test\nnode app.js",
        "node --test $(echo tests)",
        "node --test `echo tests`",
        "node --test > output.txt",
        "node --test --require=app.js",
        "node --test ../tests/foo.js",
        "node --test /tmp/tests/a.js",
        "node --test app.js",
        "node app.js",
        "npx test-runner",
        "node --test tests || true",
    ] {
        assert!(!is_local_node_test_command(script), "{script}");
    }
}

#[test]
fn test_config_policy_captured_manifest_must_match_hash_and_be_regular_bounded_json() {
    let root = filesystem_evidence_fixture();
    let path = root.join("package.json");
    let bytes = minimal_test_package().to_string();
    fs::write(&path, &bytes).unwrap();
    assert!(captured_minimal_node_test_manifest(
        &path,
        &sha256_hex(bytes.as_bytes())
    ));
    assert!(!captured_minimal_node_test_manifest(&path, "wrong-hash"));
    fs::write(&path, "not json").unwrap();
    assert!(!captured_minimal_node_test_manifest(
        &path,
        &sha256_hex(b"not json")
    ));
    fs::write(&path, " ".repeat(64 * 1024 + 1)).unwrap();
    assert!(!captured_minimal_node_test_manifest(&path, "wrong-hash"));
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn test_config_policy_rejects_symlink_manifest_evidence() {
    let root = filesystem_evidence_fixture();
    let target = root.join("manifest.json");
    let bytes = minimal_test_package().to_string();
    fs::write(&target, &bytes).unwrap();
    std::os::unix::fs::symlink(&target, root.join("package.json")).unwrap();
    assert!(!captured_minimal_node_test_manifest(
        &root.join("package.json"),
        &sha256_hex(bytes.as_bytes())
    ));
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    assert_eq!(before["testRunnerManifestAbsent"], false);
    fs::remove_file(root.join("package.json")).unwrap();
    fs::write(root.join("package.json"), &bytes).unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &before, &after);
    let run = test_config_policy_board(&root);
    assert_eq!(
        task_file_edit_policy_violations(&run.tasks[0], &delta),
        vec!["package.json"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_prompt_describes_the_narrow_exception() {
    let root = filesystem_evidence_fixture();
    let run = test_config_policy_board(&root);
    let prompt = build_test_implementation_prompt(&run, &run.tasks[0], 0);
    assert!(prompt.contains("explicitly planned root package.json"));
    assert!(prompt.contains("lifecycle scripts must remain unchanged"));
    assert!(prompt.contains("do not add arbitrary config files"));
    assert!(prompt.contains("versioned Playwright or jsdom devDependencies only"));
    assert!(prompt.contains("never as a new runtime dependency"));
    fs::remove_dir_all(root).unwrap();
}

fn playwright_test_package() -> Value {
    let mut manifest = minimal_test_package();
    manifest["devDependencies"] = json!({"@playwright/test": "^1.50.0"});
    manifest["scripts"]["test:ui"] = json!("playwright test");
    manifest
}

fn jsdom_test_package() -> Value {
    let mut manifest = minimal_test_package();
    manifest["scripts"]["test"] = json!("node --test --test-reporter=tap test/*.test.js");
    manifest["devDependencies"] = json!({"jsdom": "^26.0.0"});
    manifest
}

fn jsdom_test_lock(manifest: &Value) -> Value {
    json!({
        "name": "pocket-tasks", "version": "1.0.0", "lockfileVersion": 3, "requires": true,
        "packages": {
            "": lock_root_projection(manifest),
            "node_modules/jsdom": {"version": "26.1.0", "dev": true, "dependencies": {"parse5": "7.2.1"}},
            "node_modules/parse5": {"version": "7.2.1", "dev": true}
        }
    })
}

#[test]
fn test_config_policy_accepts_planned_new_node_jsdom_harness_and_dev_lock() {
    let root = filesystem_evidence_fixture();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    let manifest = jsdom_test_package();
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    fs::write(
        root.join("package-lock.json"),
        jsdom_test_lock(&manifest).to_string(),
    )
    .unwrap();
    fs::create_dir_all(root.join("test/helpers")).unwrap();
    fs::write(root.join("test/helpers/dom.js"), "import { JSDOM } from 'jsdom';").unwrap();
    let mut run = test_config_policy_board(&root);
    run.tasks[0].hierarchy.planned_files.push("test/helpers/dom.js".to_string());
    let delta = record_task_workspace_changes(&mut run, "task-1", before);
    assert_eq!(delta["testTooling"]["manifestAllowed"], true);
    assert_eq!(delta["testTooling"]["lockAllowed"], true);
    assert_eq!(delta["testTooling"]["playwrightConfigured"], false);
    assert!(task_file_edit_policy_violations(&run.tasks[0], &delta).is_empty());
    let result = apply_completion_evidence_gate(
        &run,
        "task-1",
        json!({"status": "done", "qaResult": "pass", "commandsRun": ["npm test"]}),
        &delta,
    );
    assert_eq!(result["filePolicy"]["passed"], true);
    assert_eq!(result["evidenceGate"]["passed"], true);
    run.tasks[0].hierarchy.planned_files.clear();
    assert_eq!(
        task_file_edit_policy_violations(&run.tasks[0], &delta),
        vec!["package-lock.json", "package.json"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_accepts_node_jsdom_addition_preserving_existing_metadata() {
    let root = filesystem_evidence_fixture();
    let mut manifest = minimal_test_package();
    manifest["dependencies"] = json!({"production-lib": "1.0.0"});
    manifest["devDependencies"] = json!({"typescript": "5.0.0"});
    manifest["scripts"]["postinstall"] = json!("node setup.js");
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    manifest["devDependencies"]["jsdom"] = json!("~26.0.0");
    manifest["scripts"]["test"] = json!("node --test --test-reporter=tap test/*.test.js");
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &before, &after);
    let run = test_config_policy_board(&root);
    assert_eq!(delta["testTooling"]["manifestAllowed"], true);
    assert!(task_file_edit_policy_violations(&run.tasks[0], &delta).is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_node_jsdom_rejects_runtime_metadata_lifecycle_and_unversioned_deps() {
    let root = filesystem_evidence_fixture();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    let run = test_config_policy_board(&root);
    for (field, value) in [
        ("dependencies", json!({"jsdom": "^26.0.0"})),
        ("main", json!("app.js")),
        ("exports", json!("./app.js")),
        ("workspaces", json!(["packages/*"])),
        ("devDependencies", json!({"jsdom": "^26.0.0", "unrelated": "1.0.0"})),
        ("devDependencies", json!({"jsdom": "latest"})),
        ("devDependencies", json!({"jsdom": "file:../jsdom"})),
        ("devDependencies", json!({"jsdom": "https://example.invalid/jsdom.tgz"})),
        ("devDependencies", json!({"jsdom": "npm:other-package@1.0.0"})),
        ("scripts", json!({"test": "node --test", "postinstall": "node setup.js"})),
        ("scripts", json!({"test": "node --test", "pretest": "node setup.js"})),
        ("scripts", json!({"test": "node --test", "start": "node app.js"})),
        ("scripts", json!({"test": "node --test; node app.js"})),
    ] {
        let mut manifest = jsdom_test_package();
        manifest[field] = value;
        fs::write(root.join("package.json"), manifest.to_string()).unwrap();
        let after = capture_workspace_snapshot(root.to_str().unwrap());
        let delta = summarize_workspace_delta("task-1", &before, &after);
        assert_eq!(
            task_file_edit_policy_violations(&run.tasks[0], &delta),
            vec!["package.json"],
            "{manifest}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_node_jsdom_does_not_grant_playwright_privileges() {
    let root = filesystem_evidence_fixture();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    let mut manifest = jsdom_test_package();
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    fs::write(root.join("playwright.config.js"), "export default {};").unwrap();
    let mut run = test_config_policy_board(&root);
    run.tasks[0].hierarchy.planned_files.push("playwright.config.js".to_string());
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &before, &after);
    assert_eq!(delta["testTooling"]["manifestAllowed"], true);
    assert_eq!(delta["testTooling"]["playwrightConfigured"], false);
    assert_eq!(
        task_file_edit_policy_violations(&run.tasks[0], &delta),
        vec!["playwright.config.js"]
    );
    manifest["scripts"]["test:ui"] = json!("playwright test");
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &before, &after);
    assert_eq!(delta["testTooling"]["manifestAllowed"], false);
    assert_eq!(
        task_file_edit_policy_violations(&run.tasks[0], &delta),
        vec!["package.json", "playwright.config.js"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_node_jsdom_lock_rejects_non_dev_or_unproven_dependencies() {
    let root = filesystem_evidence_fixture();
    let manifest = jsdom_test_package();
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    let run = test_config_policy_board(&root);
    let base = jsdom_test_lock(&manifest);
    for label in ["runtime", "orphan", "root", "version", "missing", "link", "path"] {
        let mut lock = base.clone();
        match label {
            "runtime" => lock["packages"]["node_modules/parse5"]["dev"] = json!(false),
            "orphan" => lock["packages"]["node_modules/unrelated"] = json!({"version": "1.0.0", "dev": true}),
            "root" => lock["packages"][""]["devDependencies"]["jsdom"] = json!("^99.0.0"),
            "version" => lock["packages"]["node_modules/jsdom"]["version"] = json!("99.0.0"),
            "missing" => { lock["packages"].as_object_mut().unwrap().remove("node_modules/parse5"); },
            "link" => lock["packages"]["node_modules/jsdom"]["link"] = json!(true),
            _ => lock["packages"]["node_modules/../outside"] = json!({"version": "1.0.0", "dev": true}),
        }
        fs::write(root.join("package-lock.json"), lock.to_string()).unwrap();
        let after = capture_workspace_snapshot(root.to_str().unwrap());
        let delta = summarize_workspace_delta("task-1", &before, &after);
        assert_eq!(
            task_file_edit_policy_violations(&run.tasks[0], &delta),
            vec!["package-lock.json"],
            "{label}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_node_jsdom_cannot_change_existing_production_lock_entry() {
    let root = filesystem_evidence_fixture();
    let mut manifest = jsdom_test_package();
    manifest["dependencies"] = json!({"production-lib": "1.0.0"});
    let mut lock = jsdom_test_lock(&manifest);
    lock["packages"]["node_modules/production-lib"] = json!({"version": "1.0.0"});
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    fs::write(root.join("package-lock.json"), lock.to_string()).unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    let run = test_config_policy_board(&root);
    // A valid test-only update remains allowed with unchanged production entries.
    lock["packages"]["node_modules/jsdom"]["version"] = json!("26.2.0");
    fs::write(root.join("package-lock.json"), lock.to_string()).unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &before, &after);
    assert!(task_file_edit_policy_violations(&run.tasks[0], &delta).is_empty());
    lock["packages"]["node_modules/production-lib"]["version"] = json!("2.0.0");
    fs::write(root.join("package-lock.json"), lock.to_string()).unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &before, &after);
    assert_eq!(
        task_file_edit_policy_violations(&run.tasks[0], &delta),
        vec!["package-lock.json"]
    );
    fs::remove_dir_all(root).unwrap();
}

fn playwright_test_lock(manifest: &Value) -> Value {
    json!({
        "name": "pocket-tasks", "version": "1.0.0", "lockfileVersion": 3, "requires": true,
        "packages": {
            "": lock_root_projection(manifest),
            "node_modules/@playwright/test": {"version": "1.50.1", "dev": true, "dependencies": {"playwright": "1.50.1"}},
            "node_modules/playwright": {"version": "1.50.1", "dev": true, "dependencies": {"playwright-core": "1.50.1"}},
            "node_modules/playwright-core": {"version": "1.50.1", "dev": true}
        }
    })
}

#[test]
fn test_config_policy_accepts_planned_playwright_harness_and_consistent_dev_lock() {
    let root = filesystem_evidence_fixture();
    fs::write(
        root.join("package.json"),
        minimal_test_package().to_string(),
    )
    .unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    let manifest = playwright_test_package();
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    fs::write(
        root.join("package-lock.json"),
        playwright_test_lock(&manifest).to_string(),
    )
    .unwrap();
    fs::write(
        root.join("playwright.config.js"),
        "export default {testDir: './tests'};",
    )
    .unwrap();
    fs::create_dir(root.join("tests")).unwrap();
    fs::write(root.join("tests/ui.spec.js"), "// test fixture").unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &before, &after);
    let mut run = test_config_policy_board(&root);
    run.tasks[0]
        .hierarchy
        .planned_files
        .push("playwright.config.js".to_string());
    assert_eq!(delta["testTooling"]["lockAllowed"], true);
    assert!(task_file_edit_policy_violations(&run.tasks[0], &delta).is_empty());
    run.tasks[0]
        .hierarchy
        .planned_files
        .retain(|path| path != "playwright.config.js");
    assert_eq!(
        task_file_edit_policy_violations(&run.tasks[0], &delta),
        vec!["playwright.config.js"]
    );
    run.tasks[0].task_type = TASK_KIND_QA.to_string();
    assert_eq!(
        task_file_edit_policy_violations(&run.tasks[0], &delta).len(),
        4
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_preserves_existing_production_and_lifecycle_fields() {
    let root = filesystem_evidence_fixture();
    let mut original = minimal_test_package();
    original["dependencies"] = json!({"production-lib": "1.0.0"});
    original["devDependencies"] = json!({"typescript": "5.0.0"});
    original["scripts"]["start"] = json!("node app.js");
    original["scripts"]["postinstall"] = json!("node setup.js");
    fs::write(root.join("package.json"), original.to_string()).unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    let run = test_config_policy_board(&root);
    let mut valid = original.clone();
    valid["devDependencies"]["@playwright/test"] = json!("^1.50.0");
    valid["scripts"]["test:ui"] = json!("playwright test");
    fs::write(root.join("package.json"), valid.to_string()).unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    assert!(
        task_file_edit_policy_violations(
            &run.tasks[0],
            &summarize_workspace_delta("task-1", &before, &after)
        )
        .is_empty()
    );
    for (field, value) in [
        ("dependencies", json!({"production-lib": "2.0.0"})),
        (
            "devDependencies",
            json!({"typescript": "6.0.0", "@playwright/test": "^1.50.0"}),
        ),
        ("main", json!("different.js")),
        (
            "scripts",
            json!({"test": "node --test", "start": "node other.js", "postinstall": "node setup.js"}),
        ),
        (
            "scripts",
            json!({"test": "node --test", "start": "node app.js", "postinstall": "node changed.js"}),
        ),
        (
            "scripts",
            json!({"test": "node --test; node malicious.js", "start": "node app.js", "postinstall": "node setup.js"}),
        ),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        fs::write(root.join("package.json"), invalid.to_string()).unwrap();
        let after = capture_workspace_snapshot(root.to_str().unwrap());
        assert_eq!(
            task_file_edit_policy_violations(
                &run.tasks[0],
                &summarize_workspace_delta("task-1", &before, &after)
            ),
            vec!["package.json"],
            "{field}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_rejects_unrelated_or_production_lock_changes_and_root_mismatch() {
    let root = filesystem_evidence_fixture();
    let manifest = playwright_test_package();
    fs::write(root.join("package.json"), manifest.to_string()).unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    let run = test_config_policy_board(&root);
    let base = playwright_test_lock(&manifest);
    for (label, mut lock) in [
        ("production", base.clone()),
        ("orphan", base.clone()),
        ("root", base.clone()),
        ("version", base.clone()),
        ("missing-required", base.clone()),
        ("old-format", base.clone()),
        ("top-level-name", base.clone()),
    ] {
        match label {
            "production" => lock["packages"]["node_modules/playwright"]["dev"] = json!(false),
            "orphan" => {
                lock["packages"]["node_modules/unrelated"] =
                    json!({"version": "1.0.0", "dev": true})
            }
            "root" => lock["packages"][""]["devDependencies"]["@playwright/test"] = json!("^9.0.0"),
            "version" => {
                lock["packages"]["node_modules/@playwright/test"]["version"] = json!("9.0.0")
            }
            "missing-required" => {
                lock["packages"]
                    .as_object_mut()
                    .unwrap()
                    .remove("node_modules/playwright-core");
            }
            "top-level-name" => lock["name"] = json!("unrelated-project"),
            _ => lock["lockfileVersion"] = json!(2),
        }
        fs::write(root.join("package-lock.json"), lock.to_string()).unwrap();
        let after = capture_workspace_snapshot(root.to_str().unwrap());
        let delta = summarize_workspace_delta("task-1", &before, &after);
        assert_eq!(
            task_file_edit_policy_violations(&run.tasks[0], &delta),
            vec!["package-lock.json"],
            "{label}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_playwright_artifacts_are_not_source_edits() {
    let root = filesystem_evidence_fixture();
    fs::write(root.join("app.js"), "unchanged").unwrap();
    fs::write(
        root.join("test-results"),
        "ordinary file, not a report directory",
    )
    .unwrap();
    assert!(!should_skip_path(&root.join("test-results"), &root));
    fs::remove_file(root.join("test-results")).unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    fs::create_dir(root.join("test-results")).unwrap();
    fs::create_dir(root.join("playwright-report")).unwrap();
    fs::write(root.join("test-results/.last-run.json"), "{}").unwrap();
    fs::write(root.join("playwright-report/index.html"), "report").unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    assert!(change_summary_paths(&summarize_workspace_delta("task-1", &before, &after)).is_empty());
    fs::write(root.join("app.js"), "changed source").unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let mut run = test_config_policy_board(&root);
    run.tasks[0].task_type = TASK_KIND_QA.to_string();
    assert_eq!(
        task_file_edit_policy_violations(
            &run.tasks[0],
            &summarize_workspace_delta("task-1", &before, &after)
        ),
        vec!["app.js"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_reuses_exact_same_task_scope_evidence_without_new_changes() {
    let root = filesystem_evidence_fixture();
    let mut run = test_config_policy_board(&root);
    let mut before = capture_workspace_snapshot(root.to_str().unwrap());
    before["scopeVersion"] = json!(run.tasks[0].hierarchy.scope_version.max(1));
    fs::write(
        root.join("package.json"),
        minimal_test_package().to_string(),
    )
    .unwrap();
    let first = record_task_workspace_changes(&mut run, "task-1", before);
    let mut before = capture_workspace_snapshot(root.to_str().unwrap());
    before["scopeVersion"] = first["scopeVersion"].clone();
    let second = record_task_workspace_changes(&mut run, "task-1", before);
    assert!(change_summary_paths(&second).is_empty());
    assert_eq!(second["touchedFileCount"], 0);
    assert_eq!(second["attributableChangedFileCount"], 0);
    assert_eq!(second["reusedEvidenceCount"], 1);
    assert_eq!(
        second["reusedEvidence"][0]["classification"],
        "reused_same_task"
    );
    assert_eq!(
        second["reusedEvidence"][0]["originCapturedAt"],
        first["capturedAt"]
    );
    assert_eq!(change_summary_attributable_file_count(&second), 1);
    let result = apply_completion_evidence_gate(
        &run,
        "task-1",
        json!({"status": "done", "qaResult": "pass", "commandsRun": ["npm test"]}),
        &second,
    );
    assert_eq!(result["evidenceGate"]["passed"], true);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_reused_evidence_rejects_stale_scope_hash_owner_and_incomplete_snapshots() {
    let root = filesystem_evidence_fixture();
    let mut run = test_config_policy_board(&root);
    let mut before = capture_workspace_snapshot(root.to_str().unwrap());
    before["scopeVersion"] = json!(run.tasks[0].hierarchy.scope_version.max(1));
    fs::write(
        root.join("package.json"),
        minimal_test_package().to_string(),
    )
    .unwrap();
    let first = record_task_workspace_changes(&mut run, "task-1", before);
    for case in [
        "scope",
        "legacy",
        "owner",
        "hash",
        "before-incomplete",
        "after-incomplete",
    ] {
        let mut prior = first.clone();
        let mut before = capture_workspace_snapshot(root.to_str().unwrap());
        before["scopeVersion"] = first["scopeVersion"].clone();
        let mut after = before.clone();
        match case {
            "scope" => prior["scopeVersion"] = json!(99),
            "legacy" => {
                prior.as_object_mut().unwrap().remove("scopeVersion");
            }
            "owner" => prior["taskId"] = json!("another-task"),
            "hash" => prior["changedBySubtask"][0]["afterHash"] = json!("tampered"),
            "before-incomplete" => before["complete"] = json!(false),
            _ => after["complete"] = json!(false),
        }
        run.tasks[0].changed_file_summary = Some(prior);
        let mut delta = summarize_workspace_delta("task-1", &before, &after);
        retain_same_task_workspace_evidence(&run.tasks[0], &before, &after, &mut delta);
        assert_eq!(change_summary_attributable_file_count(&delta), 0, "{case}");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_config_policy_retry_does_not_launder_original_production_manifest_edit() {
    let root = filesystem_evidence_fixture();
    fs::write(
        root.join("package.json"),
        minimal_test_package().to_string(),
    )
    .unwrap();
    let mut run = test_config_policy_board(&root);
    let mut before = capture_workspace_snapshot(root.to_str().unwrap());
    before["scopeVersion"] = json!(run.tasks[0].hierarchy.scope_version.max(1));
    let mut unsafe_manifest = minimal_test_package();
    unsafe_manifest["main"] = json!("production.js");
    fs::write(root.join("package.json"), unsafe_manifest.to_string()).unwrap();
    let first = record_task_workspace_changes(&mut run, "task-1", before);
    assert_eq!(first["testTooling"]["manifestAllowed"], false);
    let mut before = capture_workspace_snapshot(root.to_str().unwrap());
    before["scopeVersion"] = first["scopeVersion"].clone();
    let retry = record_task_workspace_changes(&mut run, "task-1", before);
    assert_eq!(retry["reusedEvidenceCount"], 1);
    assert_eq!(
        task_file_edit_policy_violations(&run.tasks[0], &retry),
        vec!["package.json"]
    );
    fs::remove_dir_all(root).unwrap();
}

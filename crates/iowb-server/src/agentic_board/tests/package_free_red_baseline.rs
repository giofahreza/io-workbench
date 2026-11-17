fn package_free_baseline_fixture() -> (PathBuf, AgenticBoard) {
    let root = filesystem_evidence_fixture();
    fs::create_dir(root.join("tests")).unwrap();
    fs::write(root.join("tests/checklist.test.js"), "const test = require('node:test');\nconst assert = require('node:assert/strict');\ntest('missing checklist shell', () => assert.equal(false, true));\n").unwrap();
    let mut run = red_phase_fixture();
    run.project_path = root.to_str().unwrap().into();
    let task = run.tasks.iter_mut().find(|task| task.id == "baseline").unwrap();
    task.task_type = TASK_KIND_TEST_IMPLEMENTATION.into();
    task.title = "Run node --test tests/checklist.test.js and capture the red-baseline TAP report".into();
    task.details = "Execute the existing test suite and capture the complete red-baseline TAP report. Do not modify index.html, tests/checklist.test.js, or any other source file during this task.".into();
    task.description = task.details.clone();
    task.hierarchy.planned_files.clear();
    run.latest_workspace_snapshot = Some(capture_workspace_snapshot(root.to_str().unwrap()));
    (root, run)
}

fn package_free_baseline_result() -> Value {
    json!({"status":"done", "qaResult":"pass", "changedFiles":[], "commandsRun":["node --test tests/checklist.test.js"], "evidence":["Captured the complete TAP report with expected assertion failures."], "remainingIssues":[]})
}

fn package_free_unchanged_delta(run: &AgenticBoard) -> Value {
    let snapshot = run.latest_workspace_snapshot.as_ref().unwrap();
    summarize_workspace_delta("baseline", snapshot, snapshot)
}

fn package_free_store_baseline(run: &mut AgenticBoard, validation: Value) {
    let task = run.tasks.iter_mut().find(|task| task.id == "baseline").unwrap();
    task.qa_baseline_validation = Some(validation.clone());
    task.deterministic_validation = Some(validation);
}

#[tokio::test]
async fn package_free_red_baseline_executes_existing_node_tests_without_manifest() {
    let (root, mut run) = package_free_baseline_fixture();
    let validation = run_tdd_validation(&run, &red_phase_task(&run, "baseline"), "feature").await;
    assert_eq!(validation["commands"].as_array().unwrap().len(), 1);
    assert_eq!(validation["commands"][0]["exitCode"], 1);
    assert_eq!(validation["commands"][0]["testReport"]["assertionFailures"], 1);
    assert_eq!(validation["expectedRedAccepted"], true);
    assert_eq!(validation["passed"], false);
    assert!(validation_gate_passed(&validation));
    assert!(validation["testSuiteFingerprint"]["tests/checklist.test.js"].is_string());
    let parsed = apply_task_validation_result(package_free_baseline_result(), &validation);
    package_free_store_baseline(&mut run, validation);
    let result = apply_completion_evidence_gate(&run, "baseline", parsed, &package_free_unchanged_delta(&run));
    assert_eq!(result["status"], "done");
    assert_eq!(result["evidenceGate"]["passed"], true);
    assert!(!root.join("package.json").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_free_red_baseline_verified_read_only_capture_does_not_require_edits() {
    let (root, mut run) = package_free_baseline_fixture();
    let validation = classify_hierarchical_test_validation(&run, &red_phase_task(&run, "baseline"), red_phase_validation(red_phase_command(&["missing checklist shell"], 0, true)));
    package_free_store_baseline(&mut run, validation);
    let result = apply_completion_evidence_gate(&run, "baseline", package_free_baseline_result(), &package_free_unchanged_delta(&run));
    assert_eq!(result["evidenceGate"]["passed"], true);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_free_red_baseline_fingerprint_requires_complete_test_snapshot() {
    let (root, mut run) = package_free_baseline_fixture();
    let before = hierarchical_validation_test_fingerprint(&run).expect("package-free suite fingerprint");
    run.latest_workspace_snapshot.as_mut().unwrap()["filesByPath"]["tests/checklist.test.js"]["hash"] = json!("changed");
    assert_ne!(hierarchical_validation_test_fingerprint(&run), Some(before));
    run.latest_workspace_snapshot.as_mut().unwrap()["complete"] = json!(false);
    assert!(hierarchical_validation_test_fingerprint(&run).is_none());
    run.latest_workspace_snapshot = Some(json!({"complete":true,"filesByPath":{"index.html":{"hash":"app"}}}));
    assert!(hierarchical_validation_test_fingerprint(&run).is_none());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn package_free_red_baseline_missing_broken_empty_and_green_suites_are_not_red() {
    for source in [None, Some("const test = require('node:test'); this is invalid syntax"), Some("require('node:test'); require('./missing.js');"), Some("require('node:test');"), Some("require('node:test')('green', () => {});")] {
        let (root, mut run) = package_free_baseline_fixture();
        if let Some(source) = source { fs::write(root.join("tests/checklist.test.js"), source).unwrap(); }
        else { fs::remove_file(root.join("tests/checklist.test.js")).unwrap(); }
        run.latest_workspace_snapshot = Some(capture_workspace_snapshot(root.to_str().unwrap()));
        let validation = run_tdd_validation(&run, &red_phase_task(&run, "baseline"), "feature").await;
        assert_ne!(validation["expectedRedAccepted"], true, "{source:?}");
        assert!(!validation_gate_passed(&validation), "{source:?}: {validation}");
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn package_free_red_baseline_claims_stale_evidence_and_ordinary_authors_still_fail() {
    let (root, run) = package_free_baseline_fixture();
    for defect in ["provider_only", "stale_scope", "stale_suite", "author", "author_and_capture", "not_read_only", "planned_edits", "incomplete_snapshot", "changed_test"] {
        let mut run = run.clone();
        let validation = classify_hierarchical_test_validation(&run, &red_phase_task(&run, "baseline"), red_phase_validation(red_phase_command(&["missing checklist shell"], 0, true)));
        package_free_store_baseline(&mut run, validation);
        let mut delta = package_free_unchanged_delta(&run);
        let task = run.tasks.iter_mut().find(|task| task.id == "baseline").unwrap();
        match defect {
            "provider_only" => { task.deterministic_validation = None; task.qa_baseline_validation = None; }
            "stale_scope" => task.hierarchy.scope_version += 1,
            "stale_suite" => { run.latest_workspace_snapshot.as_mut().unwrap()["filesByPath"]["tests/checklist.test.js"]["hash"] = json!("new"); }
            "author" => task.title = "Author tests for the shell".into(),
            "author_and_capture" => task.title = "Author tests and capture a baseline".into(),
            "not_read_only" => { task.details = "Capture red baseline".into(); task.description = task.details.clone(); }
            "planned_edits" => task.hierarchy.planned_files = vec!["tests/checklist.test.js".into()],
            "incomplete_snapshot" => delta["snapshotComplete"] = json!(false),
            "changed_test" => { delta["attributableChangedFileCount"] = json!(1); delta["changedBySubtask"] = json!([{"path":"tests/checklist.test.js"}]); delta["touchedFiles"] = json!([{"path":"tests/checklist.test.js"}]); }
            _ => unreachable!(),
        }
        assert_eq!(apply_completion_evidence_gate(&run, "baseline", package_free_baseline_result(), &delta)["evidenceGate"]["passed"], false, "{defect}");
    }
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn package_free_red_baseline_server_rerun_cannot_hide_source_writes() {
    let (root, mut run) = package_free_baseline_fixture();
    fs::write(root.join("tests/checklist.test.js"), "const test = require('node:test'); const assert = require('node:assert/strict'); require('node:fs').writeFileSync('index.html', 'forbidden write'); test('expected red', () => assert.equal(false, true));").unwrap();
    run.latest_workspace_snapshot = Some(capture_workspace_snapshot(root.to_str().unwrap()));
    let delta = package_free_unchanged_delta(&run);
    let validation = run_tdd_validation(&run, &red_phase_task(&run, "baseline"), "feature").await;
    assert_eq!(validation["expectedRedAccepted"], true);
    assert!(root.join("index.html").exists());
    package_free_store_baseline(&mut run, validation);
    let result = apply_completion_evidence_gate(&run, "baseline", package_free_baseline_result(), &delta);
    assert_eq!(result["evidenceGate"]["passed"], false);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn package_free_red_baseline_respects_configured_commands_and_zero_command_limit() {
    let (root, mut run) = package_free_baseline_fixture();
    run.validation_config = json!({"maxFeatureCommands":0});
    let validation = run_tdd_validation(&run, &red_phase_task(&run, "baseline"), "feature").await;
    assert!(validation["commands"].as_array().unwrap().is_empty());
    assert!(!validation_gate_passed(&validation));
    run.validation_config = json!({"featureCommands":["exit 7"]});
    let validation = run_tdd_validation(&run, &red_phase_task(&run, "baseline"), "feature").await;
    assert_eq!(validation["commands"].as_array().unwrap().len(), 1);
    assert_eq!(validation["commands"][0]["command"], "exit 7");
    assert_eq!(validation["commands"][0]["exitCode"], 7);
    assert!(!validation_gate_passed(&validation));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_free_red_baseline_discovery_requires_bounded_hash_matched_builtin_tests() {
    let (root, mut run) = package_free_baseline_fixture();
    assert!(package_free_node_test_paths(&run).is_some());
    fs::write(root.join("tests/checklist.test.js"), "require('node:test')('changed', () => {});").unwrap();
    assert!(package_free_node_test_paths(&run).is_none());
    run.latest_workspace_snapshot = Some(capture_workspace_snapshot(root.to_str().unwrap()));
    assert!(package_free_node_test_paths(&run).is_some());
    fs::write(root.join("tests/framework.test.js"), "test('unrecognized runner', () => {});").unwrap();
    run.latest_workspace_snapshot = Some(capture_workspace_snapshot(root.to_str().unwrap()));
    assert!(package_free_node_test_paths(&run).is_none());
    fs::remove_file(root.join("tests/framework.test.js")).unwrap();
    for index in 0..64 {
        fs::write(root.join(format!("tests/extra-{index}.test.js")), "require('node:test')('known', () => {});").unwrap();
    }
    run.latest_workspace_snapshot = Some(capture_workspace_snapshot(root.to_str().unwrap()));
    assert!(package_free_node_test_paths(&run).is_none());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn package_free_red_baseline_does_not_accept_red_implementation_or_final_qa() {
    let (root, run) = package_free_baseline_fixture();
    for kind in [TASK_KIND_IMPLEMENTATION, TASK_KIND_QA] {
        let mut task = red_phase_task(&run, "core");
        task.task_type = kind.into();
        task.final_qa_task = kind == TASK_KIND_QA;
        let validation = run_tdd_validation(&run, &task, "feature").await;
        assert!(!validation_gate_passed(&validation), "{kind}: {validation}");
        assert_ne!(validation["expectedRedAccepted"], true);
    }
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn package_free_red_baseline_file_names_are_arguments_not_shell_programs() {
    let (root, mut run) = package_free_baseline_fixture();
    fs::rename(root.join("tests/checklist.test.js"), root.join("tests/a;touch INJECTED;#.test.js")).unwrap();
    run.latest_workspace_snapshot = Some(capture_workspace_snapshot(root.to_str().unwrap()));
    let validation = run_tdd_validation(&run, &red_phase_task(&run, "baseline"), "feature").await;
    assert_eq!(validation["expectedRedAccepted"], true);
    assert!(!root.join("INJECTED").exists());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn package_free_red_baseline_does_not_discover_outside_symlinks_or_unlisted_files() {
    let (root, mut run) = package_free_baseline_fixture();
    let outside = filesystem_evidence_fixture();
    fs::rename(root.join("tests/checklist.test.js"), outside.join("outside.test.js")).unwrap();
    std::os::unix::fs::symlink(outside.join("outside.test.js"), root.join("tests/checklist.test.js")).unwrap();
    let validation = run_tdd_validation(&run, &red_phase_task(&run, "baseline"), "feature").await;
    assert!(validation["commands"].as_array().unwrap().is_empty());
    assert!(!validation_gate_passed(&validation));
    run.latest_workspace_snapshot = Some(json!({"complete":true,"filesByPath":{}}));
    fs::write(root.join("tests/unlisted.test.js"), "require('node:test')('green', () => {});").unwrap();
    let validation = run_tdd_validation(&run, &red_phase_task(&run, "baseline"), "feature").await;
    assert!(validation["commands"].as_array().unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

fn red_phase_fixture() -> AgenticBoard {
    let mut run =
        board_fixture(json!({"command": "Build Pocket Tasks", "projectPath": "/tmp/project"}));
    run.tdd_enabled = true;
    run.tdd_policy = json!({"requireFailingTestBeforeDev": true});
    run.tasks[0].status = TASK_STATUS_TODO.to_string();
    let seed = run.tasks[0].clone();
    for (id, kind, level, parent, blockers) in [
        (
            "tests",
            TASK_KIND_TEST_IMPLEMENTATION,
            TASK_LEVEL_TASK,
            "task-1",
            vec![],
        ),
        (
            "write-tests",
            TASK_KIND_TEST_IMPLEMENTATION,
            TASK_LEVEL_SUBTASK,
            "tests",
            vec![],
        ),
        (
            "baseline",
            TASK_KIND_MANUAL_TEST,
            TASK_LEVEL_SUBTASK,
            "tests",
            vec!["write-tests"],
        ),
        (
            "implementation",
            TASK_KIND_IMPLEMENTATION,
            TASK_LEVEL_TASK,
            "task-1",
            vec!["tests"],
        ),
        (
            "core",
            TASK_KIND_IMPLEMENTATION,
            TASK_LEVEL_SUBTASK,
            "implementation",
            vec!["tests"],
        ),
        (
            "persistence",
            TASK_KIND_IMPLEMENTATION,
            TASK_LEVEL_SUBTASK,
            "implementation",
            vec!["core"],
        ),
        (
            "qa",
            TASK_KIND_QA,
            TASK_LEVEL_SUBTASK,
            "implementation",
            vec!["core", "persistence"],
        ),
    ] {
        let mut task = seed.clone();
        task.id = id.to_string();
        task.title = if id == "baseline" {
            "Capture red baseline"
        } else {
            id
        }
        .to_string();
        task.task_type = kind.to_string();
        if kind == TASK_KIND_MANUAL_TEST {
            task.hierarchy.manual_verification = Some(json!({"version": 1, "mode": "cli"}));
        }
        task.hierarchy.level = level.to_string();
        task.hierarchy.parent_id = Some(parent.to_string());
        task.hierarchy.blocked_by = blockers.iter().map(|id| id.to_string()).collect();
        task.depends_on = task.hierarchy.blocked_by.clone();
        task.status = TASK_STATUS_TODO.to_string();
        task.hierarchy.executable = level == TASK_LEVEL_SUBTASK;
        run.tasks.push(task);
    }
    run.latest_workspace_snapshot = Some(json!({
        "complete": true,
        "filesByPath": {
            "package.json": {"hash": "runner-a"},
            "tests/model.test.js": {"hash": "tests-a"}
        }
    }));
    run
}

fn red_phase_task(run: &AgenticBoard, id: &str) -> BoardTask {
    run.tasks.iter().find(|task| task.id == id).unwrap().clone()
}

fn red_phase_command(failing_names: &[&str], passed: usize, tap: bool) -> Value {
    let mut output = String::new();
    let prefix = if tap { "#" } else { "ℹ" };
    output.push_str(&format!(
        "{prefix} tests {}\n{prefix} pass {passed}\n{prefix} fail {}\n{prefix} cancelled 0\n{prefix} skipped 0\n{prefix} todo 0\n",
        failing_names.len() + passed, failing_names.len()
    ));
    for name in failing_names {
        if tap {
            output.push_str(&format!("not ok 1 - {name}\n  location: '/tmp/project/tests/model.test.js:1:1'\n  code: 'ERR_ASSERTION'\n"));
        } else {
            output.push_str(&format!("test at /tmp/project/tests/model.test.js:1:1\n✖ {name} (1.0ms)\n  code: 'ERR_ASSERTION',\n"));
        }
    }
    let mut command = json!({
        "command": "npm run test", "scriptName": "test",
        "exitCode": if failing_names.is_empty() { 0 } else { 1 }, "output": output,
    });
    if let Some(report) = node_assertion_test_report(&command) {
        command["testReport"] = report;
    }
    command
}

fn red_phase_validation(command: Value) -> Value {
    json!({"stage": "feature", "passed": command["exitCode"] == 0, "commands": [command]})
}

fn red_phase_trust_baseline(run: &mut AgenticBoard) {
    let task = red_phase_task(run, "baseline");
    let baseline = classify_hierarchical_test_validation(
        run,
        &task,
        red_phase_validation(red_phase_command(&["add task", "persist task"], 0, true)),
    );
    let task = run
        .tasks
        .iter_mut()
        .find(|task| task.id == "baseline")
        .unwrap();
    task.status = TASK_STATUS_DONE.to_string();
    task.completed_at = Some(Utc::now());
    task.qa_baseline_validation = Some(baseline.clone());
    task.deterministic_validation = Some(baseline);
    let fingerprint = hierarchical_validation_test_fingerprint(run).unwrap();
    let task = run.tasks.iter_mut().find(|task| task.id == "core").unwrap();
    task.hierarchy.attempts.push(json!({
        "scopeVersion": task.hierarchy.scope_version,
        "testSuiteFingerprint": fingerprint,
        "preExecutionValidation": red_phase_validation(red_phase_command(&["add task", "persist task"], 0, true)),
    }));
}

#[test]
fn hierarchical_red_phase_recognizes_node_tap_and_spec_assertion_evidence() {
    for tap in [false, true] {
        let command = red_phase_command(&["add task", "persist task"], 1, tap);
        let report = node_assertion_test_report(&command).unwrap();
        assert_eq!(report["tests"], 3);
        assert_eq!(report["assertionFailures"], 2);
        assert_eq!(report["failedTestIds"].as_array().unwrap().len(), 2);
    }
}

#[test]
fn hierarchical_red_phase_rejects_infrastructure_and_incomplete_reports() {
    let valid = red_phase_command(&["add task"], 0, true);
    for code in [
        "ERR_MODULE_NOT_FOUND",
        "MODULE_NOT_FOUND",
        "ERR_TEST_FAILURE",
        "ENOENT",
    ] {
        let mut command = valid.clone();
        command["output"] = json!(
            command["output"]
                .as_str()
                .unwrap()
                .replace("ERR_ASSERTION", code)
        );
        assert!(node_assertion_test_report(&command).is_none());
    }
    for exit in [2, 124, 127] {
        let mut command = valid.clone();
        command["exitCode"] = json!(exit);
        assert!(node_assertion_test_report(&command).is_none());
    }
    for output in [
        "SyntaxError: missing )",
        "false exited 1",
        "# fail 1\ncode: 'ERR_ASSERTION'",
        "# tests 0\n# pass 0\n# fail 0\n# cancelled 0\n# skipped 0\n# todo 0",
    ] {
        let mut command = valid.clone();
        command["output"] = json!(output);
        assert!(node_assertion_test_report(&command).is_none());
    }
    let mut command = valid.clone();
    command["scriptName"] = json!("lint");
    assert!(node_assertion_test_report(&command).is_none());
    for counter in ["cancelled", "skipped", "todo"] {
        let mut command = valid.clone();
        command["output"] = json!(
            command["output"]
                .as_str()
                .unwrap()
                .replace(&format!("# {counter} 0"), &format!("# {counter} 1"))
        );
        assert!(node_assertion_test_report(&command).is_none());
    }
}

#[test]
fn hierarchical_red_phase_preserves_actual_failed_exit_and_separate_gate_success() {
    let run = red_phase_fixture();
    let task = red_phase_task(&run, "write-tests");
    let validation = classify_hierarchical_test_validation(
        &run,
        &task,
        red_phase_validation(red_phase_command(&["add task"], 0, true)),
    );
    assert_eq!(validation["passed"], false);
    assert_eq!(validation["gatePassed"], true);
    assert_eq!(validation["expectedRedAccepted"], true);
    assert_eq!(validation["commands"][0]["exitCode"], 1);
    let result =
        apply_task_validation_result(json!({"status":"done", "qaResult":"pass"}), &validation);
    assert_eq!(result["status"], "done");
    assert_eq!(result["deterministicValidation"]["passed"], false);
    assert!(
        result["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("validation: FAIL"))
    );
    assert!(
        !result["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("validation: PASS"))
    );
}

#[test]
fn hierarchical_red_phase_allows_green_runner_setup_but_baseline_requires_red() {
    let run = red_phase_fixture();
    let green = json!({"passed":true,"commands":[]});
    let setup = classify_hierarchical_test_validation(
        &run,
        &red_phase_task(&run, "write-tests"),
        green.clone(),
    );
    assert!(validation_gate_passed(&setup));
    let baseline =
        classify_hierarchical_test_validation(&run, &red_phase_task(&run, "baseline"), green);
    assert!(!validation_gate_passed(&baseline));
    let result = apply_task_validation_result(json!({"status":"done"}), &baseline);
    assert_eq!(result["status"], "needs_followup");
    assert!(!result["remainingIssues"].as_array().unwrap().is_empty());
}

#[test]
fn hierarchical_red_phase_covers_manual_baseline_ancestry_but_not_unrelated_kinds() {
    let mut run = red_phase_fixture();
    assert!(hierarchical_test_phase(
        &run,
        &red_phase_task(&run, "baseline")
    ));
    for id in ["core", "persistence", "qa"] {
        assert!(!hierarchical_test_phase(&run, &red_phase_task(&run, id)));
    }
    let mut unrelated = red_phase_task(&run, "baseline");
    unrelated.hierarchy.parent_id = Some("implementation".into());
    assert!(!hierarchical_test_phase(&run, &unrelated));
    run.tdd_enabled = false;
    assert!(!hierarchical_test_phase(
        &run,
        &red_phase_task(&run, "write-tests")
    ));
}

#[test]
fn hierarchical_red_phase_allows_only_demonstrated_intermediate_progress_with_qa_barrier() {
    let mut run = red_phase_fixture();
    red_phase_trust_baseline(&mut run);
    let validation = classify_hierarchical_test_validation(
        &run,
        &red_phase_task(&run, "core"),
        red_phase_validation(red_phase_command(&["persist task"], 1, true)),
    );
    assert_eq!(validation["passed"], false);
    assert_eq!(validation["gatePassed"], true);
    assert_eq!(validation["remainingRedAccepted"]["qaBarrierTaskId"], "qa");
    assert_eq!(
        validation["remainingRedAccepted"]["baselineTaskId"],
        "baseline"
    );
}

#[test]
fn hierarchical_red_phase_rejects_new_failures_unchanged_red_tests_and_changed_test_files() {
    let mut run = red_phase_fixture();
    red_phase_trust_baseline(&mut run);
    for failures in [vec!["new regression"], vec!["add task", "persist task"]] {
        let validation = classify_hierarchical_test_validation(
            &run,
            &red_phase_task(&run, "core"),
            red_phase_validation(red_phase_command(&failures, 0, true)),
        );
        assert!(!validation_gate_passed(&validation));
    }
    run.latest_workspace_snapshot.as_mut().unwrap()["filesByPath"]["tests/model.test.js"]["hash"] =
        json!("weakened-tests");
    let validation = classify_hierarchical_test_validation(
        &run,
        &red_phase_task(&run, "core"),
        red_phase_validation(red_phase_command(&["persist task"], 1, true)),
    );
    assert!(!validation_gate_passed(&validation));
}

#[test]
fn hierarchical_red_phase_requires_trusted_baseline_and_unfinished_explicit_barrier() {
    let mut run = red_phase_fixture();
    let current = red_phase_validation(red_phase_command(&["persist task"], 1, true));
    let validation =
        classify_hierarchical_test_validation(&run, &red_phase_task(&run, "core"), current.clone());
    assert!(!validation_gate_passed(&validation));
    red_phase_trust_baseline(&mut run);
    run.tasks.retain(|task| task.id != "qa");
    let validation =
        classify_hierarchical_test_validation(&run, &red_phase_task(&run, "core"), current);
    assert!(!validation_gate_passed(&validation));
}

#[test]
fn hierarchical_red_phase_last_implementation_qa_and_non_tdd_must_still_be_green() {
    let mut run = red_phase_fixture();
    red_phase_trust_baseline(&mut run);
    let current = red_phase_validation(red_phase_command(&["persist task"], 1, true));
    for id in ["persistence", "qa"] {
        let validation =
            classify_hierarchical_test_validation(&run, &red_phase_task(&run, id), current.clone());
        assert!(!validation_gate_passed(&validation));
        assert_eq!(
            apply_task_validation_result(json!({"status":"done"}), &validation)["status"],
            "needs_followup"
        );
    }
    run.tdd_enabled = false;
    let validation =
        classify_hierarchical_test_validation(&run, &red_phase_task(&run, "core"), current);
    assert!(!validation_gate_passed(&validation));
}

#[test]
fn hierarchical_red_phase_does_not_override_provider_reported_incomplete_scope() {
    let run = red_phase_fixture();
    let validation = classify_hierarchical_test_validation(
        &run,
        &red_phase_task(&run, "write-tests"),
        red_phase_validation(red_phase_command(&["add task"], 0, true)),
    );
    let result = apply_task_validation_result(
        json!({"status":"needs_followup", "remainingIssues":["Missing required priority tests"]}),
        &validation,
    );
    assert_eq!(result["status"], "needs_followup");
    assert_eq!(
        result["remainingIssues"][0],
        "Missing required priority tests"
    );
}

#[test]
fn hierarchical_red_phase_prompt_explains_red_evidence_without_false_green_claims() {
    let run = red_phase_fixture();
    let prompt = task_scope_block(&run, &red_phase_task(&run, "baseline"));
    assert!(prompt.contains("Never claim that the test suite is green"));
    assert!(prompt.contains("missing-import error"));
    let prompt = task_scope_block(&run, &red_phase_task(&run, "core"));
    assert!(prompt.contains("last implementation slice and QA must make the full suite green"));
}

fn red_phase_playwright_command(message: &str, status: &str) -> Value {
    let report = json!({
        "errors": [],
        "stats": {"expected":0,"unexpected":1,"skipped":0,"flaky":0},
        "suites": [{"specs":[{
            "title":"add task updates count", "file":"tests/ui.spec.js", "line":10,"column":1,
            "tests":[{"expectedStatus":"passed","projectName":"chromium","results":[{
                "status": status,"errors":[{"message":message}]
            }]}]
        }]}]
    });
    json!({"command":"npm run test:ui","scriptName":"test:ui","exitCode":1,
        "output":format!("\n> test:ui\n> playwright test --reporter=json\n{}", report)})
}

#[test]
fn hierarchical_red_phase_recognizes_playwright_assertion_json_not_browser_infrastructure() {
    let command =
        red_phase_playwright_command("Error: expect(locator).toBeVisible() failed", "failed");
    let report = playwright_assertion_test_report(&command).unwrap();
    assert_eq!(report["runner"], "playwright");
    assert_eq!(report["assertionFailures"], 1);
    for (message, status) in [
        ("browserType.launch: executable does not exist", "failed"),
        ("page.goto: net::ERR_CONNECTION_REFUSED", "failed"),
        ("locator.click: Timeout 30000ms exceeded", "timedOut"),
        (
            "Error: Test timeout of 30000ms exceeded while expect(locator).toBeVisible()",
            "timedOut",
        ),
        ("SyntaxError: Unexpected token", "failed"),
    ] {
        assert!(
            playwright_assertion_test_report(&red_phase_playwright_command(message, status))
                .is_none()
        );
    }
}

#[test]
fn hierarchical_red_phase_playwright_recognizes_colored_assertions_without_source_snippets() {
    // Playwright 1.63 inserts a reset between the dot and matcher. A source
    // snippet containing an uncolored matcher must not be needed to classify it.
    let message = "Error: \u{1b}[2mexpect(\u{1b}[22m\u{1b}[31mlocator\u{1b}[39m\u{1b}[2m).\u{1b}[22mtoHaveCount\u{1b}[2m(\u{1b}[22m\u{1b}[32mexpected\u{1b}[39m\u{1b}[2m)\u{1b}[22m failed\nExpected: 1\nReceived: 0\nTimeout: 5000ms";
    let command = red_phase_playwright_command(message, "failed");
    let original = command.clone();
    let report = playwright_assertion_test_report(&command).unwrap();
    assert_eq!(report["assertionFailures"], 1);
    assert_eq!(report["actualExitCode"], 1);
    assert_eq!(command, original, "classification must retain raw evidence");
}

#[test]
fn hierarchical_red_phase_playwright_colored_infrastructure_still_fails_closed() {
    for message in [
        "Error: expect(locator).\u{1b}[22mtoBeVisible() failed\nTest \u{1b}[31mtimeout\u{1b}[39m of 30000ms exceeded",
        "Error: expect(locator).\u{1b}[22mtoBeVisible() failed\npage.goto: net::ERR_\u{1b}[31mCONNECTION_REFUSED\u{1b}[39m",
        "Error: expect(locator).\u{1b}[22mtoBeVisible() failed\nbrowserType.\u{1b}[31mlaunch\u{1b}[39m: executable does not exist",
        "Error: expect(locator).\u{1b}[2JtoBeVisible() failed",
        "Error: expect(locator).\u{1b}[22toBeVisible() failed",
    ] {
        assert!(
            playwright_assertion_test_report(&red_phase_playwright_command(message, "failed"))
                .is_none(),
            "must reject non-assertion or malformed evidence: {message:?}",
        );
    }
}

#[test]
fn hierarchical_red_phase_playwright_rejects_global_errors_skips_and_incomplete_counts() {
    let original =
        red_phase_playwright_command("Error: expect(locator).toHaveText() failed", "failed");
    for field in ["skipped", "flaky", "unexpected"] {
        let mut command = original.clone();
        let mut report = parse_json_object(command["output"].as_str().unwrap()).unwrap();
        report["stats"][field] = json!(2);
        command["output"] = json!(report.to_string());
        assert!(playwright_assertion_test_report(&command).is_none());
    }
    let mut command = original.clone();
    let mut report = parse_json_object(command["output"].as_str().unwrap()).unwrap();
    report["errors"] = json!([{"message":"webServer failed to start"}]);
    command["output"] = json!(report.to_string());
    assert!(playwright_assertion_test_report(&command).is_none());
}

#[test]
fn hierarchical_red_phase_explicit_browser_baseline_cannot_pass_only_green_unit_tests() {
    let run = red_phase_fixture();
    let mut task = red_phase_task(&run, "write-tests");
    task.details = "Write browser tests and capture expected failing baseline".into();
    let green = red_phase_validation(red_phase_command(&[], 3, false));
    let validation = classify_hierarchical_test_validation(&run, &task, green);
    assert!(!validation_gate_passed(&validation));
    let mut command =
        red_phase_playwright_command("Error: expect(locator).toBeVisible() failed", "failed");
    command["testReport"] = playwright_assertion_test_report(&command).unwrap();
    let validation =
        classify_hierarchical_test_validation(&run, &task, red_phase_validation(command));
    assert_eq!(validation["passed"], false);
    assert!(validation_gate_passed(&validation));
}

#[test]
fn hierarchical_red_phase_fingerprint_includes_test_runner_configuration() {
    let mut run = red_phase_fixture();
    run.latest_workspace_snapshot.as_mut().unwrap()["filesByPath"]["playwright.config.js"] =
        json!({"hash":"config-a"});
    run.latest_workspace_snapshot.as_mut().unwrap()["filesByPath"]["package-lock.json"] =
        json!({"hash":"lock-a"});
    let before = hierarchical_validation_test_fingerprint(&run).unwrap();
    assert_eq!(before["playwright.config.js"], "config-a");
    assert_eq!(before["package-lock.json"], "lock-a");
    run.latest_workspace_snapshot.as_mut().unwrap()["filesByPath"]["playwright.config.js"]["hash"] =
        json!("config-b");
    assert_ne!(
        hierarchical_validation_test_fingerprint(&run).unwrap(),
        before
    );
}

#[test]
fn hierarchical_red_phase_barriers_must_be_required_live_approved_executable_work() {
    for id in ["persistence", "qa"] {
        for defect in [
            "optional",
            "backlog",
            "superseded",
            "nonexecutable",
            "blocked",
        ] {
            let mut run = red_phase_fixture();
            red_phase_trust_baseline(&mut run);
            let barrier = run.tasks.iter_mut().find(|task| task.id == id).unwrap();
            match defect {
                "optional" => {
                    barrier.hierarchy.required = false;
                    barrier.status = TASK_STATUS_BACKLOG.into();
                }
                "backlog" => barrier.status = TASK_STATUS_BACKLOG.into(),
                "superseded" => barrier.superseded_by = Some("replacement".into()),
                "nonexecutable" => barrier.hierarchy.executable = false,
                _ => barrier.status = TASK_STATUS_BLOCKED.into(),
            }
            assert!(
                hierarchical_intermediate_qa_barrier(&run, &red_phase_task(&run, "core")).is_none(),
                "{id}: {defect}"
            );
        }
    }
}

#[test]
fn hierarchical_red_phase_historical_red_does_not_excuse_a_new_regression_after_green() {
    let mut run = red_phase_fixture();
    red_phase_trust_baseline(&mut run);
    let core = run.tasks.iter_mut().find(|task| task.id == "core").unwrap();
    core.hierarchy.attempts.last_mut().unwrap()["preExecutionValidation"] =
        red_phase_validation(red_phase_command(&[], 2, true));
    let validation = classify_hierarchical_test_validation(
        &run,
        &red_phase_task(&run, "core"),
        red_phase_validation(red_phase_command(&["persist task"], 1, true)),
    );
    assert!(!validation_gate_passed(&validation));
}

#[test]
fn hierarchical_red_phase_baseline_requires_current_scope_latest_attempt_and_prerequisite_lineage()
{
    for defect in [
        "scope",
        "newer-green",
        "unrelated",
        "no-preflight",
        "preflight-scope",
    ] {
        let mut run = red_phase_fixture();
        red_phase_trust_baseline(&mut run);
        if defect.starts_with("preflight") || defect == "no-preflight" {
            let core = run.tasks.iter_mut().find(|task| task.id == "core").unwrap();
            if defect == "no-preflight" {
                core.hierarchy.attempts.clear();
            } else {
                core.hierarchy.attempts.last_mut().unwrap()["scopeVersion"] = json!(999);
            }
        } else {
            let baseline = run
                .tasks
                .iter_mut()
                .find(|task| task.id == "baseline")
                .unwrap();
            match defect {
                "scope" => baseline.hierarchy.scope_version += 1,
                "newer-green" => {
                    baseline.deterministic_validation =
                        Some(red_phase_validation(red_phase_command(&[], 2, true)))
                }
                _ => baseline.hierarchy.parent_id = Some("unrelated-test-parent".into()),
            }
        }
        let validation = classify_hierarchical_test_validation(
            &run,
            &red_phase_task(&run, "core"),
            red_phase_validation(red_phase_command(&["persist task"], 1, true)),
        );
        assert!(!validation_gate_passed(&validation), "{defect}");
    }
}

#[tokio::test]
async fn hierarchical_red_phase_executes_explicit_task_test_commands_and_fails_closed() {
    let root = filesystem_evidence_fixture();
    let mut run = red_phase_fixture();
    run.project_path = root.to_str().unwrap().into();
    let mut task = red_phase_task(&run, "write-tests");
    task.qa_test_commands = vec!["exit 9".into()];
    let validation = run_tdd_validation(&run, &task, "feature").await;
    assert!(!validation_gate_passed(&validation));
    assert_eq!(validation["commands"][0]["command"], "exit 9");
    assert_eq!(validation["commands"][0]["exitCode"], 9);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn hierarchical_red_phase_retains_only_approved_pure_dependency_waiting_barriers() {
    let mut run = red_phase_fixture();
    red_phase_trust_baseline(&mut run);
    for id in ["persistence", "qa"] {
        let task = run.tasks.iter_mut().find(|task| task.id == id).unwrap();
        task.status = TASK_STATUS_BLOCKED.into();
        task.error = Some(format!(
            "Waiting on dependency: {}",
            task_blockers(task).join(", ")
        ));
        task.hierarchy.scope_approval = Some(BoardTaskScopeApproval {
            scope_version: task.hierarchy.scope_version.max(1),
            approved: true,
        });
    }
    assert_eq!(
        hierarchical_intermediate_qa_barrier(&run, &red_phase_task(&run, "core")),
        Some("qa".into())
    );
    let task = run.tasks.iter_mut().find(|task| task.id == "qa").unwrap();
    task.hierarchy
        .scope_approval
        .as_mut()
        .unwrap()
        .scope_version += 1;
    assert!(hierarchical_intermediate_qa_barrier(&run, &red_phase_task(&run, "core")).is_none());
    let task = run.tasks.iter_mut().find(|task| task.id == "qa").unwrap();
    task.hierarchy
        .scope_approval
        .as_mut()
        .unwrap()
        .scope_version = task.hierarchy.scope_version.max(1);
    task.error = Some("Provider run failed".into());
    assert!(hierarchical_intermediate_qa_barrier(&run, &red_phase_task(&run, "core")).is_none());
}

#[test]
fn hierarchical_red_phase_preflight_rechecks_control_scope_and_approval_without_mutation() {
    let mut run = red_phase_fixture();
    run.status = "running".into();
    run.active = true;
    run.current_task_id = Some("core".into());
    run.tasks
        .iter_mut()
        .find(|task| task.id == "tests")
        .unwrap()
        .status = TASK_STATUS_DONE.into();
    let core = run.tasks.iter_mut().find(|task| task.id == "core").unwrap();
    core.status = TASK_STATUS_IN_PROGRESS.into();
    let scope = core.hierarchy.scope_version;
    let revision = run.control_revision;
    let project = run.project_path.clone();
    assert!(preflight_task_may_start(
        &run, "core", &project, revision, scope
    ));
    for change in [
        "pause",
        "abort",
        "resume",
        "scope",
        "project",
        "backlog",
        "parent",
        "deleted",
        "dependency",
    ] {
        let mut fresh = run.clone();
        match change {
            "pause" => request_board_pause(&mut fresh, Some("during preflight".into())),
            "abort" => {
                fresh.status = "cancelled".into();
                fresh.active = false;
            }
            "resume" => fresh.control_revision += 1,
            "scope" => {
                let task = fresh
                    .tasks
                    .iter_mut()
                    .find(|task| task.id == "core")
                    .unwrap();
                task.hierarchy.scope_version += 1;
                task.description = "New user scope".into();
            }
            "project" => fresh.project_path = "/tmp/new-project".into(),
            "backlog" => {
                fresh
                    .tasks
                    .iter_mut()
                    .find(|task| task.id == "core")
                    .unwrap()
                    .status = TASK_STATUS_BACKLOG.into()
            }
            "parent" => {
                fresh
                    .tasks
                    .iter_mut()
                    .find(|task| task.id == "implementation")
                    .unwrap()
                    .status = TASK_STATUS_BACKLOG.into()
            }
            "deleted" => fresh.tasks.retain(|task| task.id != "core"),
            _ => {
                fresh
                    .tasks
                    .iter_mut()
                    .find(|task| task.id == "tests")
                    .unwrap()
                    .status = TASK_STATUS_BACKLOG.into()
            }
        }
        let before = serde_json::to_value(&fresh).unwrap();
        assert!(
            !preflight_task_may_start(&fresh, "core", &project, revision, scope),
            "{change}"
        );
        assert_eq!(
            serde_json::to_value(&fresh).unwrap(),
            before,
            "must preserve fresh {change}"
        );
    }
    let mut fresh = run.clone();
    fresh.model = "new user model choice".into();
    assert!(preflight_task_may_start(
        &fresh, "core", &project, revision, scope
    ));
    assert_eq!(fresh.model, "new user model choice");
}

fn blocked_error_feedback_fixture() -> AgenticBoard {
    let mut run =
        board_fixture(json!({"command": "Build Pocket Tasks", "projectPath": "/tmp/project"}));
    run.tasks[0].hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
    run.tasks[0].hierarchy.executable = true;
    run.tasks[0].task_type = TASK_KIND_TEST_IMPLEMENTATION.to_string();
    run
}

#[test]
fn blocked_error_feedback_retains_actual_file_policy_failure_without_changing_gate_behavior() {
    let mut run = blocked_error_feedback_fixture();
    let task_id = run.tasks[0].id.clone();
    let result = apply_completion_evidence_gate(
        &run,
        &task_id,
        json!({"status": "done", "qaResult": "pass", "commandsRun": ["npm test"]}),
        &json!({
            "attributableChangedFileCount": 1,
            "changedBySubtask": [{"path": "app.js"}],
            "touchedFiles": [{"path": "app.js"}],
        }),
    );
    assert_eq!(result["status"], "needs_followup");
    assert_eq!(result["filePolicy"]["passed"], false);
    assert_eq!(result["evidenceGate"]["passed"], false);
    run.tasks[0].error = Some(
        "Completion evidence gate failed; provide valid evidence before this subtask can be done."
            .to_string(),
    );

    assert!(!append_followup_task_if_needed(&mut run, &task_id, &result));
    assert_eq!(run.tasks.len(), 1);
    assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
    let error = run.tasks[0].error.as_deref().unwrap();
    assert!(error.starts_with("File-edit policy failed for test_implementation:"));
    assert!(error.contains("workspace files: app.js"));
    assert!(!error.contains("Git-visible"));
    assert!(!error.contains("incomplete work inside"));
}

#[test]
fn blocked_error_feedback_retains_deterministic_failure_command_and_exit_code() {
    let mut run = blocked_error_feedback_fixture();
    let task_id = run.tasks[0].id.clone();
    let result = apply_deterministic_validation_result(
        json!({"status": "done", "qaResult": "pass"}),
        &json!({"passed": false, "commands": [{
            "command": "npm test", "exitCode": 1, "output": "task counts assertion failed"
        }]}),
    );
    run.tasks[0].error = Some("Incomplete work remains inside the approved scope.".to_string());

    assert!(!append_followup_task_if_needed(&mut run, &task_id, &result));
    assert_eq!(run.tasks.len(), 1);
    assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
    assert_eq!(
        run.tasks[0].error.as_deref(),
        Some("Deterministic validation failed: npm test exited 1: task counts assertion failed")
    );
}

#[test]
fn blocked_error_feedback_preserves_evidence_issue_and_existing_error_with_bounded_text() {
    let mut run = blocked_error_feedback_fixture();
    let task_id = run.tasks[0].id.clone();
    let result = json!({
        "status": "needs_followup",
        "evidenceGate": {"passed": false, "issues": ["Missing attributable workspace changes."]}
    });
    assert!(!append_followup_task_if_needed(&mut run, &task_id, &result));
    assert_eq!(
        run.tasks[0].error.as_deref(),
        Some("Missing attributable workspace changes.")
    );

    let reason = format!(
        "Provider configuration unavailable: {}",
        "details ".repeat(300)
    );
    run.tasks[0].error = Some(reason);
    let plain_result = json!({"status": "needs_followup"});
    assert!(!append_followup_task_if_needed(
        &mut run,
        &task_id,
        &plain_result
    ));
    let error = run.tasks[0].error.as_deref().unwrap();
    assert!(error.starts_with("Provider configuration unavailable:"));
    assert!(error.chars().count() <= 1200);
    assert!(error.ends_with("...[truncated]"));
}

#[test]
fn blocked_error_feedback_keeps_scope_fallback_when_no_specific_reason_exists() {
    let mut run = blocked_error_feedback_fixture();
    let task_id = run.tasks[0].id.clone();
    assert!(!append_followup_task_if_needed(
        &mut run,
        &task_id,
        &json!({"status": "needs_followup"})
    ));
    assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
    assert_eq!(run.tasks.len(), 1);
    assert!(
        run.tasks[0]
            .error
            .as_deref()
            .unwrap()
            .starts_with("Subtask reported incomplete work inside the approved scope;")
    );
}

#[test]
fn command_reports_do_not_forge_validation_verdicts_from_blocked_manual_outcome() {
    let mut run = blocked_error_feedback_fixture();
    let task_id = run.tasks[0].id.clone();
    let validation = json!({
        "taskId": task_id, "stage": "feature", "passed": true,
        "commands": [{"command": "node --test tests/checklist.test.js", "exitCode": 0,
            "testReport": {"tests": 10, "passed": 10}}]
    });
    let historical = json!({"taskId": "old-task", "command": "old command", "passed": false});
    run.validation_runs = vec![historical.clone(), validation.clone()];
    let parsed = json!({
        "status": "blocked", "qaResult": "blocked", "manualTestResult": "blocked",
        "commandsRun": ["curl localhost:18104/index.html", "browser navigate", "cleanup",
            "node --test tests/checklist.test.js"],
        "deterministicValidation": validation
    });

    apply_task_result_to_board(&mut run, &task_id, &parsed);

    assert_eq!(run.validation_runs, vec![historical, validation]);
    assert_eq!(parsed["commandsRun"].as_array().unwrap().len(), 4);
    assert_eq!(parsed["manualTestResult"], "blocked");
}

#[test]
fn command_reports_do_not_claim_success_without_individual_execution_evidence() {
    for qa_result in [json!("pass"), json!("fail"), json!("blocked"), Value::Null] {
        let mut run = blocked_error_feedback_fixture();
        let task_id = run.tasks[0].id.clone();
        let parsed = json!({
            "qaResult": qa_result,
            "commandsRun": ["inspect source", "curl localhost"],
            "changedFiles": ["index.html"]
        });

        apply_task_result_to_board(&mut run, &task_id, &parsed);

        assert!(run.validation_runs.is_empty(), "reported commands cannot establish their own outcomes");
        assert_eq!(run.change_ledger.len(), 1);
        assert_eq!(run.change_ledger[0]["path"], "index.html");
    }
}

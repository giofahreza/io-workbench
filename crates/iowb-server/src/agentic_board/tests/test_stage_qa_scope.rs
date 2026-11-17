fn test_stage_qa_fixture() -> (AgenticBoard, BoardTask) {
    let mut run = red_phase_fixture();
    red_phase_trust_baseline(&mut run);
    let source = red_phase_task(&run, "baseline");
    let qa = create_task_qa_task(&run, &source, "Verify the captured red baseline");
    run.tasks.push(qa.clone());
    (run, qa)
}

#[test]
fn test_stage_qa_accepts_only_the_unchanged_verified_source_red_baseline() {
    let (run, qa) = test_stage_qa_fixture();
    let validation = classify_hierarchical_test_validation(
        &run,
        &qa,
        red_phase_validation(red_phase_command(&["add task", "persist task"], 0, true)),
    );
    assert_eq!(validation["passed"], false);
    assert_eq!(validation["gatePassed"], true);
    assert_eq!(validation["expectedRedAccepted"], true);
    assert_eq!(validation["testPhase"], "test_validation");
    assert_eq!(validation["commands"][0]["exitCode"], 1);
    let result =
        apply_task_validation_result(json!({"status":"done", "qaResult":"pass"}), &validation);
    assert_eq!(result["status"], "done");
}

#[test]
fn test_stage_qa_red_does_not_exempt_untrusted_changed_or_unrelated_work() {
    let (original, qa) = test_stage_qa_fixture();
    for case in [
        "new-failure",
        "infra",
        "changed-tests",
        "stale-scope",
        "not-done",
        "missing-source",
        "final-qa",
        "ordinary-qa",
        "missing-baseline",
    ] {
        let mut run = original.clone();
        let mut qa = qa.clone();
        let mut command = red_phase_command(&["add task", "persist task"], 0, true);
        match case {
            "new-failure" => command = red_phase_command(&["different defect"], 0, true),
            "infra" => {
                command["output"] = json!("Cannot find module app.js");
                command.as_object_mut().unwrap().remove("testReport");
            }
            "changed-tests" => {
                run.latest_workspace_snapshot.as_mut().unwrap()["filesByPath"]["tests/model.test.js"]
                    ["hash"] = json!("changed-tests")
            }
            "stale-scope" => {
                run.tasks
                    .iter_mut()
                    .find(|task| task.id == "baseline")
                    .unwrap()
                    .hierarchy
                    .scope_version += 1
            }
            "not-done" => {
                run.tasks
                    .iter_mut()
                    .find(|task| task.id == "baseline")
                    .unwrap()
                    .status = "blocked".into()
            }
            "missing-source" => qa.source_task_id = Some("missing".into()),
            "final-qa" => qa.final_qa_task = true,
            "ordinary-qa" => qa.task_origin = "manual".into(),
            _ => {
                run.tasks
                    .iter_mut()
                    .find(|task| task.id == "baseline")
                    .unwrap()
                    .qa_baseline_validation = None
            }
        }
        let validation =
            classify_hierarchical_test_validation(&run, &qa, red_phase_validation(command));
        assert!(!validation_gate_passed(&validation), "{case}");
    }
}

#[test]
fn test_stage_qa_failure_cannot_auto_approve_a_production_fix() {
    let (mut run, qa) = test_stage_qa_fixture();
    let parsed = json!({"status":"needs_followup", "remainingIssues":["npm test exits 1"], "suggestedBacklogTasks":[{"title":"Implement the missing app", "plannedFiles":["app.js"]}]});
    assert!(append_followup_task_if_needed(&mut run, &qa.id, &parsed));
    let fix = run.tasks.iter().find(|task| task.followup_task).unwrap();
    assert_eq!(fix.status, TASK_STATUS_BACKLOG);
    assert!(fix.hierarchy.scope_approval.is_none());
    assert_eq!(fix.hierarchy.parent_id, qa.hierarchy.parent_id);
    assert_eq!(fix.hierarchy.planned_files, qa.hierarchy.planned_files);
    assert!(fix.error.as_deref().unwrap().contains("approval"));
}

#[test]
fn test_stage_qa_legacy_auto_fix_cannot_claim_unapproved_production_or_test_changes() {
    let (mut run, qa) = test_stage_qa_fixture();
    assert!(append_followup_task_if_needed(
        &mut run,
        &qa.id,
        &json!({"status":"needs_followup"})
    ));
    let fix = run
        .tasks
        .iter_mut()
        .find(|task| task.followup_task)
        .unwrap();
    fix.status = TASK_STATUS_IN_PROGRESS.into();
    fix.hierarchy.planned_files = vec!["tests/model.test.js".into()];
    let id = fix.id.clone();
    let result = apply_completion_evidence_gate(
        &run,
        &id,
        json!({"status":"done", "qaResult":"pass", "commandsRun":["npm test"]}),
        &json!({"attributableChangedFileCount":2,"touchedFiles":["app.js","tests/model.test.js"],"changedBySubtask":[{"path":"app.js"},{"path":"tests/model.test.js"}]}),
    );
    assert_eq!(result["status"], "needs_followup");
    assert_eq!(result["filePolicy"]["passed"], false);
    assert_eq!(
        result["filePolicy"]["violations"],
        json!(["app.js", "tests/model.test.js"])
    );
}

#[test]
fn test_stage_qa_production_fix_inherits_only_provably_approved_exact_paths() {
    let mut run = red_phase_fixture();
    let source = run.tasks.iter_mut().find(|task| task.id == "core").unwrap();
    source.status = TASK_STATUS_DONE.into();
    source.hierarchy.scope_version = 2;
    source.hierarchy.scope_approval = Some(BoardTaskScopeApproval {
        scope_version: 2,
        approved: true,
    });
    source.hierarchy.planned_files = vec!["app.js".into()];
    let source = source.clone();
    let qa = create_task_qa_task(&run, &source, "Check approved implementation");
    run.tasks.push(qa.clone());
    assert!(append_followup_task_if_needed(
        &mut run,
        &qa.id,
        &json!({"status":"needs_followup"})
    ));
    let fix = run
        .tasks
        .iter()
        .find(|task| task.followup_task)
        .unwrap()
        .clone();
    assert_eq!(fix.status, TASK_STATUS_TODO);
    assert!(automatic_qa_fix_scope_is_authorized(&run, &fix));
    assert!(
        automatic_qa_fix_file_violations(&run, &fix, &json!({"touchedFiles":["app.js"]}))
            .is_empty()
    );
    assert_eq!(
        automatic_qa_fix_file_violations(
            &run,
            &fix,
            &json!({"touchedFiles":["extra.js", "tests/model.test.js"]})
        ),
        vec!["extra.js", "tests/model.test.js"]
    );
    for case in [
        "unknown",
        "cycle",
        "test-origin",
        "stale",
        "unapproved",
        "outside-path",
        "test-path",
        "glob",
        "traversal",
    ] {
        let mut changed = run.clone();
        let mut fix = fix.clone();
        match case {
            "unknown" => fix.source_task_id = Some("missing".into()),
            "cycle" => fix.source_task_id = Some(fix.id.clone()),
            "test-origin" => {
                changed
                    .tasks
                    .iter_mut()
                    .find(|task| task.id == "core")
                    .unwrap()
                    .task_type = TASK_KIND_TEST_IMPLEMENTATION.into()
            }
            "stale" => {
                changed
                    .tasks
                    .iter_mut()
                    .find(|task| task.id == "core")
                    .unwrap()
                    .hierarchy
                    .scope_version += 1
            }
            "unapproved" => {
                changed
                    .tasks
                    .iter_mut()
                    .find(|task| task.id == "core")
                    .unwrap()
                    .hierarchy
                    .scope_approval = None
            }
            "outside-path" => fix.hierarchy.planned_files = vec!["new.js".into()],
            "test-path" => fix.hierarchy.planned_files = vec!["tests/model.test.js".into()],
            "glob" => fix.hierarchy.planned_files = vec!["*.js".into()],
            _ => fix.hierarchy.planned_files = vec!["../app.js".into()],
        }
        assert!(
            !automatic_qa_fix_scope_is_authorized(&changed, &fix),
            "{case}"
        );
    }
}

#[test]
fn test_stage_qa_explicit_current_leaf_repair_keeps_narrow_human_authority() {
    let (mut run, qa) = test_stage_qa_fixture();
    append_followup_task_if_needed(&mut run, &qa.id, &json!({"status":"needs_followup"}));
    let fix = run
        .tasks
        .iter_mut()
        .find(|task| task.followup_task)
        .unwrap();
    fix.status = TASK_STATUS_IN_PROGRESS.into();
    fix.hierarchy.scope_version = 4;
    fix.hierarchy.scope_approval = Some(BoardTaskScopeApproval {
        scope_version: 4,
        approved: true,
    });
    fix.hierarchy.planned_files = vec!["tests/model.test.js".into()];
    fix.acceptance_criteria = vec!["Approved test repair: tests/model.test.js".into()];
    let fix = fix.clone();
    assert!(!automatic_qa_fix_scope_is_authorized(&run, &fix));
    run.discussion_proposals.push(json!({
        "id":"reviewed-repair", "taskId":fix.id, "status":"applied", "action":"edit",
        "appliedAt":"2026-09-29T00:00:00Z", "after":{"task":discussion_task_scope(&fix),"descendants":[]}
    }));
    assert!(automatic_qa_fix_scope_is_authorized(&run, &fix));
    assert!(
        automatic_qa_fix_file_violations(
            &run,
            &fix,
            &json!({"touchedFiles":["tests/model.test.js"]})
        )
        .is_empty()
    );
    assert_eq!(
        automatic_qa_fix_file_violations(
            &run,
            &fix,
            &json!({"touchedFiles":["app.js", "tests/other.test.js"]})
        ),
        vec!["app.js", "tests/other.test.js"]
    );
    let prompt = build_task_execution_prompt(&run, &fix, 0);
    assert!(prompt.contains("Automatic QA-fix scope boundary:"));
    assert!(
        prompt.contains(
            "semantic test repairs only in these exact planned files: tests/model.test.js"
        )
    );
    run.discussion_proposals.clear();
    assert!(!automatic_qa_fix_scope_is_authorized(&run, &fix));
}

#[tokio::test(flavor = "current_thread")]
async fn test_stage_qa_legacy_unsafe_fix_is_rejected_before_provider_start() {
    let (root, state, _) = discussion_lifecycle_fixture().await;
    let (mut run, qa) = test_stage_qa_fixture();
    append_followup_task_if_needed(&mut run, &qa.id, &json!({"status":"needs_followup"}));
    let index = run
        .tasks
        .iter()
        .position(|task| task.followup_task)
        .unwrap();
    run.tasks[index].status = TASK_STATUS_IN_PROGRESS.into();
    run.tasks[index].hierarchy.scope_approval = Some(BoardTaskScopeApproval {
        scope_version: run.tasks[index].hierarchy.scope_version,
        approved: true,
    });
    // No live provider is configured; the deterministic authorization error
    // must be returned before shared runtime/session creation is attempted.
    let result = execute_provider_task(&state, &run, index)
        .await
        .unwrap_err();
    assert!(server_error_message(&result).contains("Automatic QA fix cannot start"));
    drop(state);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test(flavor = "current_thread")]
async fn test_stage_qa_real_validation_pipeline_classifies_red_but_final_qa_stays_strict() {
    let root = filesystem_evidence_fixture();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("package.json"),
        json!({
            "private":true,"scripts":{"test":"node --test --test-reporter=tap tests/model.test.js"}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(root.join("tests/model.test.js"),
        "const test = require('node:test'); const assert = require('node:assert/strict');\ntest('missing add contract', () => assert.equal(undefined, 'function'));\ntest('missing save contract', () => assert.equal(undefined, 'function'));\n").unwrap();
    let mut run = red_phase_fixture();
    run.project_path = root.display().to_string();
    run.validation_config = json!({"enabled":true});
    run.latest_workspace_snapshot = Some(capture_workspace_snapshot(&run.project_path));
    let writer = red_phase_task(&run, "write-tests");
    let baseline = run_tdd_validation(&run, &writer, "feature").await;
    assert_eq!(baseline["expectedRedAccepted"], true, "{baseline}");
    assert_eq!(
        baseline["commands"][0]["testReport"]["assertionFailures"],
        2
    );
    let writer = run
        .tasks
        .iter_mut()
        .find(|task| task.id == writer.id)
        .unwrap();
    writer.status = TASK_STATUS_DONE.into();
    writer.qa_baseline_validation = Some(baseline.clone());
    writer.deterministic_validation = Some(baseline);
    let qa = create_task_qa_task(
        &run,
        &red_phase_task(&run, "write-tests"),
        "Verify real red tests",
    );
    run.tasks.push(qa.clone());
    let checked = run_tdd_validation(&run, &qa, "feature").await;
    assert_eq!(checked["passed"], false);
    assert_eq!(checked["gatePassed"], true, "{checked}");
    assert_eq!(
        checked["testStageQaEvidence"]["sourceTaskId"],
        "write-tests"
    );
    assert!(task_scope_block(&run, &qa).contains("TDD test-stage QA:"));
    let mut final_qa = qa;
    final_qa.final_qa_task = true;
    let checked = run_tdd_validation(&run, &final_qa, "feature").await;
    assert!(!validation_gate_passed(&checked));
    fs::remove_dir_all(root).unwrap();
}

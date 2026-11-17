#[test]
fn review_routing_only_explicit_promotion_uses_rag() {
    let mut run = red_phase_fixture();
    let mut review = red_phase_task(&run, "qa");
    review.task_type = TASK_KIND_REVIEW.into();
    review.title = "Review UI security and accessibility".into();
    assert!(!is_promotion_review_task(&review));
    assert_eq!(
        build_task_execution_prompt(&run, &review, 0),
        build_validation_prompt(&run, &review, 0)
    );

    let source = red_phase_task(&run, "write-tests");
    let mut agents = create_agents_knowledge_task(&run, "Review test workflow", Some(&source));
    assert!(!is_promotion_review_task(&agents));
    agents.task_origin = "system_promotion".into();
    assert!(!is_promotion_review_task(&agents));

    review.task_type = "promotion".into();
    assert!(is_promotion_review_task(&review));
    run.rag_settings = json!({"enabled":true});
    run.promotion_candidates = vec![json!({"id":"candidate-1"})];
    assert!(append_promotion_review_task(
        &mut run,
        "Explicit candidate review"
    ));
    normalize_board_model(&mut run);
    let promotion = run
        .tasks
        .iter()
        .find(|task| task.id == PROMOTION_REVIEW_TASK_ID)
        .unwrap();
    assert!(is_promotion_review_task(promotion));
    let mut renamed = promotion.clone();
    renamed.id = "persisted-promotion-review".into();
    assert!(is_promotion_review_task(&renamed));
}

#[test]
fn review_routing_disabled_rag_does_not_queue_stale_candidates() {
    let mut run = red_phase_fixture();
    run.rag_settings = json!({"enabled":false});
    run.promotion_candidates = vec![json!({"id":"stale-candidate"})];
    let count = run.tasks.len();
    assert!(!append_promotion_review_task(&mut run, "RAG disabled"));
    assert_eq!(run.tasks.len(), count);
}

#[test]
fn review_routing_new_knowledge_is_optional_unapproved_read_only_work() {
    let mut run = red_phase_fixture();
    for task in run
        .tasks
        .iter_mut()
        .filter(|task| task.hierarchy.parent_id.as_deref() == Some("tests"))
    {
        task.status = TASK_STATUS_DONE.into();
    }
    let source = red_phase_task(&run, "write-tests");
    assert!(append_agents_knowledge_task(
        &mut run,
        "Stable workflow",
        Some(&source)
    ));
    let knowledge = run
        .tasks
        .iter()
        .find(|task| task.agents_knowledge_task)
        .unwrap();
    assert_eq!(knowledge.status, TASK_STATUS_BACKLOG);
    assert!(!knowledge.hierarchy.required);
    assert!(!task_scope_is_approved(knowledge));
    assert!(!task_is_runnable_in_board(&run, knowledge));
    assert_eq!(knowledge.tdd_phase, "disabled");
    assert!(knowledge.hierarchy.planned_files.is_empty());
    assert!(knowledge.details.contains("Do not create or edit files"));
    assert!(
        !task_file_edit_policy_violations(knowledge, &json!({"touchedFiles":["AGENTS.md"]}))
            .is_empty()
    );
    refresh_hierarchy_rollups(&mut run);
    assert_eq!(red_phase_task(&run, "tests").status, TASK_STATUS_DONE);
}

#[test]
fn review_routing_existing_required_knowledge_is_not_silently_demoted() {
    let mut run = red_phase_fixture();
    let source = red_phase_task(&run, "write-tests");
    let mut legacy = create_agents_knowledge_task(&run, "Existing approved work", Some(&source));
    legacy.status = TASK_STATUS_BLOCKED.into();
    legacy.hierarchy.required = true;
    run.tasks.push(legacy);
    normalize_board_model(&mut run);
    let legacy = run
        .tasks
        .iter()
        .find(|task| task.agents_knowledge_task)
        .unwrap();
    assert!(legacy.hierarchy.required);
    assert_eq!(legacy.status, TASK_STATUS_BLOCKED);
    assert!(!is_promotion_review_task(legacy));
}

#[test]
fn review_routing_intentional_red_does_not_trigger_knowledge_enrichment() {
    let run = red_phase_fixture();
    let mut task = red_phase_task(&run, "write-tests");
    assert!(task_needs_agents_knowledge_update(
        &task,
        &json!({"summary":"Added tests"})
    ));
    for validation in [
        json!({"passed":false,"gatePassed":true,"expectedRedAccepted":true}),
        json!({"passed":false,"gatePassed":true,"remainingRedAccepted":{"baselineTaskId":"baseline"}}),
    ] {
        task.deterministic_validation = Some(validation);
        assert!(!task_needs_agents_knowledge_update(
            &task,
            &json!({"summary":"Added tests"})
        ));
    }
    task.deterministic_validation = Some(json!({"passed":true}));
    assert!(task_needs_agents_knowledge_update(
        &task,
        &json!({"summary":"Verified test workflow"})
    ));
}

fn baseline_capture_result() -> Value {
    json!({
        "status":"done", "qaResult":"pass", "commandsRun":["npm test"],
        "evidence":["Recorded actual TAP report with exit 1 and two assertion failures."],
        "manualTestSteps":["Ran npm test; observed two expected assertion failures and captured the report."],
        "manualTestResult":"pass — full red baseline captured; all behaviors represented by failing tests, each failing via a real AssertionError",
        "manualTestEnvironment":{"deviceOrEmulator":"host shell","appVersion":"harness-only","backendUrl":"none/local-only"},
        "manualEvidence": manual_non_ui_evidence_fixture("cli", "command"),
        "changedFiles":[], "remainingIssues":[]
    })
}

#[test]
fn baseline_capture_verified_red_accepts_successful_capture_not_false_green() {
    let mut run = red_phase_fixture();
    red_phase_trust_baseline(&mut run);
    let result =
        apply_completion_evidence_gate(&run, "baseline", baseline_capture_result(), &json!({}));
    assert_eq!(result["evidenceGate"]["passed"], true);
    assert_eq!(result["status"], "done");
    let task = red_phase_task(&run, "baseline");
    assert_eq!(
        task.deterministic_validation.as_ref().unwrap()["passed"],
        false
    );
    assert!(!manual_test_result_is_successful(
        baseline_capture_result()["manualTestResult"]
            .as_str()
            .unwrap()
    ));
}

#[test]
fn baseline_capture_exception_requires_current_server_red_and_complete_manual_evidence() {
    let mut run = red_phase_fixture();
    red_phase_trust_baseline(&mut run);
    for missing in ["manualTestSteps", "manualTestEnvironment"] {
        let mut parsed = baseline_capture_result();
        parsed.as_object_mut().unwrap().remove(missing);
        assert_eq!(
            apply_completion_evidence_gate(&run, "baseline", parsed, &json!({}))["evidenceGate"]["passed"],
            false
        );
    }
    for change in [
        "missing",
        "stale",
        "unrecognized",
        "ordinary_manual",
        "non_tdd",
    ] {
        let mut changed = run.clone();
        if change == "non_tdd" {
            changed.tdd_enabled = false;
        }
        let task = changed
            .tasks
            .iter_mut()
            .find(|task| task.id == "baseline")
            .unwrap();
        match change {
            "missing" => task.deterministic_validation = None,
            "stale" => task.hierarchy.scope_version += 1,
            "unrecognized" => {
                task.deterministic_validation.as_mut().unwrap()["expectedRedAccepted"] =
                    json!(false)
            }
            "ordinary_manual" => {
                task.title = "Browser smoke test".into();
                task.hierarchy.parent_id = Some("implementation".into());
            }
            _ => {}
        }
        let parsed = apply_completion_evidence_gate(
            &changed,
            "baseline",
            baseline_capture_result(),
            &json!({}),
        );
        assert_eq!(parsed["evidenceGate"]["passed"], false, "{change}");
    }
}

#[test]
fn baseline_capture_failed_or_blocked_capture_is_not_accepted() {
    let mut run = red_phase_fixture();
    red_phase_trust_baseline(&mut run);
    for verdict in [
        "failed — baseline capture failed",
        "pass — full red baseline captured; follow-up capture failed",
        "pass — full red baseline captured; blocked by timeout",
        "pass — full red baseline captured; unable to save evidence",
        "pass — tests failed before implementation",
    ] {
        let mut parsed = baseline_capture_result();
        parsed["manualTestResult"] = json!(verdict);
        assert_eq!(
            apply_completion_evidence_gate(&run, "baseline", parsed, &json!({}))["evidenceGate"]["passed"],
            false,
            "{verdict}"
        );
    }
}

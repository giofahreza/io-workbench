struct ManualUiEvidenceFixture {
    project: PathBuf,
    board: AgenticBoard,
}

impl Drop for ManualUiEvidenceFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.project).expect("remove isolated manual evidence test fixture");
    }
}

fn manual_ui_evidence_fixture() -> ManualUiEvidenceFixture {
    let project = env::temp_dir().join(format!("iowb-manual-evidence-{}", Uuid::new_v4()));
    fs::create_dir_all(project.join("test-results/manual-evidence")).unwrap();
    // A real tiny PNG is sufficient for the structural artifact test. This
    // fixture deliberately does not assert semantic/visual image verification.
    let png = base64::Engine::decode(&base64::engine::general_purpose::STANDARD,
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a3z8AAAAASUVORK5CYII=").unwrap();
    fs::write(project.join("test-results/manual-evidence/step.png"), png).unwrap();
    let mut board = board_fixture(json!({"command": "Verify shopping UI", "projectPath": project}));
    let task = &mut board.tasks[0];
    task.task_type = TASK_KIND_MANUAL_TEST.into();
    task.hierarchy.level = TASK_LEVEL_SUBTASK.into();
    task.hierarchy.executable = true;
    task.hierarchy.manual_verification =
        Some(json!({"version":1,"mode":"ui","viewports":[{"width":360,"height":800}]}));
    task.acceptance_criteria = vec!["Adding Bread gives visible feedback".into()];
    ManualUiEvidenceFixture { project, board }
}

fn manual_ui_evidence_result() -> Value {
    json!({
        "status":"done", "qaResult":"pass", "summary":"Observed app behavior",
        "manualTestEnvironment":{"deviceOrEmulator":"emulator-5590","appVersion":"build-42","backendUrl":"none/local-only"},
        "manualTestSteps":["Tap Add: Bread is visible with quantity 1"],
        "manualTestResult":"pass",
        "manualEvidence":{
            "version":1,"scopeVersion":1,"mode":"ui",
            "environments":[{"id":"phone","deviceOrEmulator":"emulator-5590","appVersion":"build-42","backendUrl":"none/local-only","surface":"browser","browser":"Chrome 140","viewport":{"width":360,"height":800}}],
            "checks":[{"criterionIndex":0,"environmentId":"phone","status":"pass","expected":"Bread appears once","observed":"One Bread row, quantity 1, success feedback",
                "actions":[{"type":"tap","target":"Add","observed":"Bread appears once"}],
                "artifacts":["test-results/manual-evidence/step.png"]}]
        }
    })
}

fn manual_non_ui_evidence_fixture(mode: &str, action: &str) -> Value {
    json!({
        "version":1,"scopeVersion":1,"mode":mode,
        "environments":[{"id":"host","deviceOrEmulator":"host shell","appVersion":"harness-only","backendUrl":"none/local-only"}],
        "checks":[{"criterionIndex":0,"environmentId":"host","status":"pass","expected":"Capture actual result","observed":"Observed actual result recorded",
            "actions":[{"type":action,"target":"local verification target","observed":"Expected response captured"}]}]
    })
}

#[test]
fn manual_ui_evidence_rejects_original_nine_tests_css_and_curl_false_approval() {
    let fixture = manual_ui_evidence_fixture();
    let result = apply_completion_evidence_gate(
        &fixture.board,
        "task-1",
        json!({
            "status":"done","qaResult":"pass",
            "summary":"All acceptance met; touch-friendly controls at least 36px",
            "commandsRun":["npm test","grep -n min-height styles.css","curl localhost:8765"],
            "evidence":["9 passed; CSS uses 36/40/44px controls"],
            "manualTestEnvironment":{"deviceOrEmulator":"host shell","appVersion":"current","backendUrl":"none/local-only"},
            "manualTestSteps":["Ran 9 Playwright tests","Inspected CSS and source","Fetched app HTML"],
            "manualTestResult":"All checks passed"
        }),
        &json!({}),
    );
    assert_eq!(result["status"], "needs_followup");
    assert_eq!(result["qaResult"], "blocked");
    assert_eq!(result["evidenceGate"]["passed"], false);
    assert_eq!(result["manualEvidenceGate"]["passed"], false);
}

#[test]
fn manual_ui_evidence_accepts_scoped_actions_observations_and_real_artifacts() {
    let fixture = manual_ui_evidence_fixture();
    let result = apply_completion_evidence_gate(
        &fixture.board,
        "task-1",
        manual_ui_evidence_result(),
        &json!({}),
    );
    assert_eq!(result["status"], "done", "{result}");
    assert_eq!(result["evidenceGate"]["passed"], true);
    assert_eq!(
        result["manualEvidenceGate"]["artifacts"][0]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
}

#[test]
fn manual_ui_evidence_rejects_incomplete_stale_failed_and_source_only_structured_claims() {
    let fixture = manual_ui_evidence_fixture();
    for defect in [
        "mode",
        "scope",
        "viewport",
        "browser",
        "blocked",
        "failed",
        "criterion",
        "actions",
        "source_only",
        "screenshot",
        "empty_screenshot",
        "observation",
    ] {
        let mut result = manual_ui_evidence_result();
        match defect {
            "mode" => result["manualEvidence"]["mode"] = json!("inspection"),
            "scope" => result["manualEvidence"]["scopeVersion"] = json!(99),
            "viewport" => {
                result["manualEvidence"]["environments"][0]["viewport"]["width"] = json!(1280)
            }
            "browser" => result["manualEvidence"]["environments"][0]["browser"] = json!(""),
            "blocked" | "failed" => result["manualEvidence"]["checks"][0]["status"] = json!(defect),
            "criterion" => result["manualEvidence"]["checks"][0]["criterionIndex"] = json!(7),
            "actions" => result["manualEvidence"]["checks"][0]["actions"] = json!([]),
            "source_only" => {
                result["manualEvidence"]["checks"][0]["actions"][0]["type"] = json!("inspect")
            }
            "screenshot" => {
                result["manualEvidence"]["checks"][0]["artifacts"] =
                    json!(["test-results/manual-evidence/missing.png"])
            }
            "empty_screenshot" => result["manualEvidence"]["checks"][0]["artifacts"] = json!([]),
            "observation" => result["manualEvidence"]["checks"][0]["observed"] = json!(""),
            _ => unreachable!(),
        }
        let result = apply_completion_evidence_gate(&fixture.board, "task-1", result, &json!({}));
        assert_eq!(
            result["evidenceGate"]["passed"], false,
            "{defect}: {result}"
        );
    }
}

#[test]
fn manual_ui_evidence_requires_all_criteria_and_required_viewports() {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0]
        .acceptance_criteria
        .push("Bought flag survives reload".into());
    let result = apply_completion_evidence_gate(
        &fixture.board,
        "task-1",
        manual_ui_evidence_result(),
        &json!({}),
    );
    assert_eq!(result["evidenceGate"]["passed"], false);
    fixture.board.tasks[0].acceptance_criteria.pop();
    fixture.board.tasks[0]
        .hierarchy
        .manual_verification
        .as_mut()
        .unwrap()["viewports"]
        .as_array_mut()
        .unwrap()
        .push(json!({"width":1280,"height":800}));
    let result = apply_completion_evidence_gate(
        &fixture.board,
        "task-1",
        manual_ui_evidence_result(),
        &json!({}),
    );
    assert_eq!(result["evidenceGate"]["passed"], false);
}

#[test]
fn manual_ui_evidence_cli_api_and_inspection_do_not_require_gui() {
    for (mode, action) in [
        ("cli", "command"),
        ("api", "request"),
        ("inspection", "inspect"),
    ] {
        let mut fixture = manual_ui_evidence_fixture();
        fixture.board.tasks[0].hierarchy.manual_verification =
            Some(json!({"version":1,"mode":mode}));
        let mut result = manual_ui_evidence_result();
        result["manualEvidence"] = manual_non_ui_evidence_fixture(mode, action);
        let result = apply_completion_evidence_gate(&fixture.board, "task-1", result, &json!({}));
        assert_eq!(result["status"], "done", "{mode}: {result}");
    }
}

#[test]
fn manual_ui_evidence_qa_cannot_override_missing_source_evidence() {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0].status = TASK_STATUS_DONE.into();
    fixture.board.tasks[0].result = Some(
        json!({"status":"done","qaResult":"pass","commandsRun":["npm test"],"summary":"9 tests passed, mobile friendly"}),
    );
    let qa = create_task_qa_task(
        &fixture.board,
        &fixture.board.tasks[0],
        "Audit manual result",
    );
    let qa_id = qa.id.clone();
    assert!(qa.details.contains("Open and inspect the screenshots"));
    fixture.board.tasks.push(qa);
    let result = apply_completion_evidence_gate(
        &fixture.board,
        &qa_id,
        json!({"status":"done","qaResult":"pass","evidence":["Agreed with source summary"]}),
        &json!({}),
    );
    assert_eq!(result["evidenceGate"]["passed"], false);
    assert_eq!(result["qaResult"], "blocked");
    // Replacing the source with actual structured evidence permits review.
    fixture.board.tasks[0].result = Some(apply_completion_evidence_gate(
        &fixture.board,
        "task-1",
        manual_ui_evidence_result(),
        &json!({}),
    ));
    let result = apply_completion_evidence_gate(
        &fixture.board,
        &qa_id,
        json!({"status":"done","qaResult":"pass","evidence":["Reviewed observed criterion and screenshot"]}),
        &json!({}),
    );
    assert_eq!(result["status"], "done", "{result}");
}

#[test]
fn manual_ui_evidence_sandbox_rejects_paths_symlinks_and_false_image_files() {
    let fixture = manual_ui_evidence_fixture();
    for path in [
        "/etc/passwd",
        "test-results/manual-evidence/../../../etc/passwd",
        "test-results/step.png",
        "https://example.com/a.png",
    ] {
        assert!(manual_evidence_artifact(fixture.project.to_str().unwrap(), path).is_none());
    }
    fs::write(
        fixture
            .project
            .join("test-results/manual-evidence/text.png"),
        "not a screenshot, just a long successful summary claiming all ninety tests pass",
    )
    .unwrap();
    assert!(
        manual_evidence_artifact(
            fixture.project.to_str().unwrap(),
            "test-results/manual-evidence/text.png"
        )
        .is_none()
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            "/etc/passwd",
            fixture
                .project
                .join("test-results/manual-evidence/outside.png"),
        )
        .unwrap();
        assert!(
            manual_evidence_artifact(
                fixture.project.to_str().unwrap(),
                "test-results/manual-evidence/outside.png"
            )
            .is_none()
        );
    }
}

#[test]
fn manual_ui_evidence_explicit_qa_and_parent_review_cannot_bypass_source_gate() {
    for kind in [TASK_KIND_QA, TASK_KIND_REVIEW] {
        let mut fixture = manual_ui_evidence_fixture();
        let mut parent = fixture.board.tasks[0].clone();
        parent.id = "verification-parent".into();
        parent.task_type = TASK_KIND_IMPLEMENTATION.into();
        parent.hierarchy.level = TASK_LEVEL_TASK.into();
        parent.hierarchy.executable = false;
        parent.hierarchy.manual_verification = None;
        fixture.board.tasks[0].hierarchy.parent_id = Some(parent.id.clone());
        fixture.board.tasks[0].result = Some(json!({"status":"done","summary":"9 tests pass"}));
        fixture.board.tasks.push(parent);
        let mut qa = fixture.board.tasks[0].clone();
        qa.id = "explicit-review".into();
        qa.task_type = kind.into();
        qa.qa_task = false;
        qa.hierarchy.parent_id = None;
        qa.source_task_id = None;
        qa.hierarchy.blocked_by = vec!["verification-parent".into()];
        fixture.board.tasks.push(qa);
        let result = apply_completion_evidence_gate(
            &fixture.board,
            "explicit-review",
            json!({"status":"done","qaResult":"pass","evidence":["Reviewed all dependencies"]}),
            &json!({}),
        );
        assert_eq!(result["evidenceGate"]["passed"], false, "{kind}: {result}");
    }
}

#[test]
fn manual_ui_evidence_manual_parent_rollup_uses_valid_leaf_evidence() {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0].result = Some(apply_completion_evidence_gate(
        &fixture.board,
        "task-1",
        manual_ui_evidence_result(),
        &json!({}),
    ));
    fixture.board.tasks[0].status = TASK_STATUS_DONE.into();
    let mut parent = fixture.board.tasks[0].clone();
    parent.id = "manual-planning-parent".into();
    parent.hierarchy.level = TASK_LEVEL_TASK.into();
    parent.hierarchy.executable = false;
    parent.result = None;
    fixture.board.tasks[0].hierarchy.parent_id = Some(parent.id.clone());
    let qa = create_task_qa_task(&fixture.board, &parent, "Review completed manual scope");
    let qa_id = qa.id.clone();
    fixture.board.tasks.extend([parent, qa]);
    let result = apply_completion_evidence_gate(
        &fixture.board,
        &qa_id,
        json!({"status":"done","qaResult":"pass","evidence":["Reviewed required manual leaf evidence"]}),
        &json!({}),
    );
    assert_eq!(result["status"], "done", "{result}");
}

#[test]
fn manual_ui_evidence_legacy_scope_can_be_recovered_via_normal_discussion_edit() {
    let mut fixture = manual_ui_evidence_fixture();
    let scope = fixture.board.tasks[0]
        .hierarchy
        .manual_verification
        .take()
        .unwrap();
    fixture.board.tasks[0].status = TASK_STATUS_BACKLOG.into();
    let previous_version = fixture.board.tasks[0].hierarchy.scope_version;
    let before = discussion_task_scope(&fixture.board.tasks[0]);
    apply_discussion_action(
        &mut fixture.board,
        "task-1",
        "edit",
        &json!({"manualVerification":scope}),
    )
    .unwrap();
    let task = &fixture.board.tasks[0];
    assert_eq!(task.hierarchy.manual_verification, Some(scope));
    assert_eq!(task.hierarchy.scope_version, previous_version + 1);
    let diff = discussion_diff(&before, &discussion_task_scope(task));
    assert!(diff.to_string().contains("manualVerification"));
    assert!(task.result.is_none());
}

#[test]
fn manual_ui_evidence_qa_rejects_mutated_artifact() {
    let fixture = manual_ui_evidence_fixture();
    let result = apply_completion_evidence_gate(
        &fixture.board,
        "task-1",
        manual_ui_evidence_result(),
        &json!({}),
    );
    let path = fixture
        .project
        .join("test-results/manual-evidence/step.png");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(0);
    fs::write(path, bytes).unwrap();
    let validation =
        manual_verification_evidence_validation(&fixture.board, &fixture.board.tasks[0], &result);
    assert_eq!(validation["passed"], false);
}

#[test]
fn manual_ui_evidence_legacy_records_load_unchanged_but_new_attempts_need_scope() {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0].hierarchy.manual_verification = None;
    fixture.board.tasks[0].status = TASK_STATUS_DONE.into();
    let serialized = serde_json::to_value(&fixture.board.tasks[0]).unwrap();
    assert!(serialized.get("manualVerification").is_none());
    let restored: BoardTask = serde_json::from_value(serialized).unwrap();
    assert_eq!(restored.status, TASK_STATUS_DONE);
    assert!(restored.hierarchy.manual_verification.is_none());
    let result = apply_completion_evidence_gate(
        &fixture.board,
        "task-1",
        manual_ui_evidence_result(),
        &json!({}),
    );
    assert_eq!(result["status"], "needs_followup");
    assert!(result["remainingIssues"].to_string().contains("Backlog"));
}

#[test]
fn manual_ui_evidence_scope_survives_authoring_and_inherits_without_downgrade() {
    let fixture = manual_ui_evidence_fixture();
    let scope = fixture.board.tasks[0]
        .hierarchy
        .manual_verification
        .clone()
        .unwrap();
    let mut run = fixture.board.clone();
    let task = BoardTask::manual(
        &mut run,
        serde_json::from_value(
            json!({"title":"Verify browser","kind":"manual_test","manualVerification":scope}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(task.hierarchy.manual_verification, Some(scope.clone()));
    let drafts = sanitize_prompt_task_drafts(
        &json!({"tasks":[{"title":"Verify browser","kind":"manual_test","manualVerification":scope}]}),
        "UI verification",
    );
    let draft = prompt_task_from_draft(&mut run, drafts[0].clone(), "UI verification");
    assert_eq!(draft.hierarchy.manual_verification, Some(scope.clone()));
    let mut child = task.clone();
    child.hierarchy.manual_verification = Some(json!({"version":1,"mode":"inspection"}));
    inherit_hierarchy_breakdown_dependencies(&mut child, &task);
    assert_eq!(child.hierarchy.manual_verification, Some(scope));
    let prompt = build_manual_test_prompt(&run, &child, 0);
    assert!(prompt.contains("source/CSS inspection and curl"));
    assert!(prompt.contains("never a substitute for UI actions"));
    let breakdown = build_hierarchy_breakdown_prompt(&run, &task, TASK_LEVEL_SUBTASK);
    assert!(breakdown.contains("Split mixed UI, CLI/API, and source-inspection"));
}

#[test]
fn manual_touch_contract_rejects_mouse_relabelled_as_tap_and_missing_observations() {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0].hierarchy.manual_verification.as_mut().unwrap()["inputModality"] = json!("touch");
    for defect in ["missing_capability", "zero_touch_points", "missing_observation", "mouse", "untrusted", "wrong_event", "string_touch_points"] {
        let mut result = manual_ui_evidence_result();
        result["manualEvidence"]["environments"][0]["inputCapabilities"] = json!({"maxTouchPoints":1});
        result["manualEvidence"]["checks"][0]["actions"][0]["inputObservation"] =
            json!({"pointerType":"touch","eventType":"pointerup","isTrusted":true});
        match defect {
            "missing_capability" => result["manualEvidence"]["environments"][0]["inputCapabilities"] = Value::Null,
            "zero_touch_points" => result["manualEvidence"]["environments"][0]["inputCapabilities"]["maxTouchPoints"] = json!(0),
            "string_touch_points" => result["manualEvidence"]["environments"][0]["inputCapabilities"]["maxTouchPoints"] = json!("1"),
            "missing_observation" => result["manualEvidence"]["checks"][0]["actions"][0]["inputObservation"] = Value::Null,
            "mouse" => result["manualEvidence"]["checks"][0]["actions"][0]["inputObservation"]["pointerType"] = json!("mouse"),
            "untrusted" => result["manualEvidence"]["checks"][0]["actions"][0]["inputObservation"]["isTrusted"] = json!(false),
            "wrong_event" => result["manualEvidence"]["checks"][0]["actions"][0]["inputObservation"]["eventType"] = json!("click"),
            _ => unreachable!(),
        }
        let checked = apply_completion_evidence_gate(&fixture.board, "task-1", result, &json!({}));
        assert_eq!(checked["manualEvidenceGate"]["passed"], false, "{defect}: {checked}");
    }
}

#[test]
fn manual_touch_contract_accepts_reported_browser_and_native_touch_events() {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0].hierarchy.manual_verification.as_mut().unwrap()["inputModality"] = json!("touch");
    for surface in ["browser", "native"] {
        let mut result = manual_ui_evidence_result();
        result["manualEvidence"]["environments"][0]["surface"] = json!(surface);
        result["manualEvidence"]["environments"][0]["inputCapabilities"] = if surface == "browser" {
            json!({"maxTouchPoints":1})
        } else { json!({"touchscreen":true}) };
        result["manualEvidence"]["checks"][0]["actions"][0]["inputObservation"] = if surface == "browser" {
            json!({"pointerType":"touch","eventType":"pointerup","isTrusted":true})
        } else { json!({"motionEventSource":4098,"action":"up"}) };
        let checked = apply_completion_evidence_gate(&fixture.board, "task-1", result, &json!({}));
        assert_eq!(checked["manualEvidenceGate"]["passed"], true, "{surface}: {checked}");
    }
}

#[test]
fn manual_touch_contract_rejects_invalid_modality_and_non_ui_scope() {
    for contract in [
        json!({"version":1,"mode":"ui","viewports":[{"width":360,"height":800}],"inputModality":"toch"}),
        json!({"version":1,"mode":"cli","inputModality":"touch"}),
        json!({"version":1,"mode":"ui","viewports":[{"width":360,"height":800}],"inputModality":false}),
    ] {
        assert!(normalize_supplied_manual_verification(Some(contract.clone())).is_err(), "{contract}");
    }
}

#[test]
fn manual_touch_contract_rejects_partial_or_wrong_native_touch_evidence() {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0].hierarchy.manual_verification.as_mut().unwrap()["inputModality"] = json!("touch");
    for defect in ["mouse_source", "wrong_event", "missing_capability", "extra_mouse_action", "navigation_only"] {
        let mut result = manual_ui_evidence_result();
        result["manualEvidence"]["environments"][0]["surface"] = json!("native");
        result["manualEvidence"]["environments"][0]["inputCapabilities"] = json!({"touchscreen":true});
        result["manualEvidence"]["checks"][0]["actions"][0]["inputObservation"] = json!({"motionEventSource":4098,"action":"up"});
        match defect {
            "mouse_source" => result["manualEvidence"]["checks"][0]["actions"][0]["inputObservation"]["motionEventSource"] = json!(8194),
            "wrong_event" => result["manualEvidence"]["checks"][0]["actions"][0]["inputObservation"]["action"] = json!("move"),
            "missing_capability" => result["manualEvidence"]["environments"][0]["inputCapabilities"] = Value::Null,
            "extra_mouse_action" => result["manualEvidence"]["checks"][0]["actions"].as_array_mut().unwrap().push(json!({"type":"tap","target":"Other","observed":"Clicked"})),
            "navigation_only" => result["manualEvidence"]["checks"][0]["actions"][0]["type"] = json!("navigate"),
            _ => unreachable!(),
        }
        let checked = apply_completion_evidence_gate(&fixture.board, "task-1", result, &json!({}));
        assert_eq!(checked["manualEvidenceGate"]["passed"], false, "{defect}: {checked}");
    }
}

#[test]
fn manual_touch_contract_is_preserved_when_children_propose_weaker_scope() {
    let mut fixture = manual_ui_evidence_fixture();
    let parent = &mut fixture.board.tasks[0];
    parent.hierarchy.manual_verification.as_mut().unwrap()["inputModality"] = json!("touch");
    let children = normalize_hierarchy_manual_contracts(vec![json!({
        "title":"Verify child UI", "kind":"manual_test",
        "manualVerification":{"version":1,"mode":"ui","viewports":[{"width":360,"height":800}],"inputModality":"any"}
    })], parent).unwrap();
    assert_eq!(children[0]["manualVerification"]["inputModality"], "touch");
}

#[test]
fn manual_touch_contract_allows_static_navigation_check_after_touch_in_same_environment() {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0].hierarchy.manual_verification.as_mut().unwrap()["inputModality"] = json!("touch");
    fixture.board.tasks[0].acceptance_criteria.push("Heading is readable".into());
    let mut result = manual_ui_evidence_result();
    result["manualEvidence"]["environments"][0]["inputCapabilities"] = json!({"maxTouchPoints":1});
    result["manualEvidence"]["checks"][0]["actions"][0]["inputObservation"] = json!({"pointerType":"touch","eventType":"pointerup","isTrusted":true});
    let mut static_check = result["manualEvidence"]["checks"][0].clone();
    static_check["criterionIndex"] = json!(1);
    static_check["expected"] = json!("Heading is readable");
    static_check["observed"] = json!("Shopping List heading is fully visible");
    static_check["actions"] = json!([{"type":"navigate","target":"Shopping List page","observed":"Heading visible above list"}]);
    result["manualEvidence"]["checks"].as_array_mut().unwrap().push(static_check);
    let checked = apply_completion_evidence_gate(&fixture.board, "task-1", result, &json!({}));
    assert_eq!(checked["manualEvidenceGate"]["passed"], true, "{checked}");
}

#[test]
fn manual_touch_contract_inherits_into_source_kind_wrapper() {
    let mut fixture = manual_ui_evidence_fixture();
    let parent = &mut fixture.board.tasks[0];
    parent.hierarchy.manual_verification.as_mut().unwrap()["inputModality"] = json!("touch");
    for proposed in [Value::Null, json!({"version":1,"mode":"ui","viewports":[{"width":360,"height":800}],"inputModality":"any"})] {
        let children = normalize_hierarchy_manual_contracts(vec![json!({
            "title":"Design verification breakdown", "kind":"design", "sourceKind":"manual_test", "sourceLevel":"subtask",
            "manualVerification":proposed
        })], parent).unwrap();
        assert_eq!(children[0]["manualVerification"], parent.hierarchy.manual_verification.clone().unwrap());
    }
}

#[test]
fn manual_touch_contract_accepts_observed_touch_scroll_on_browser_and_native() {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0].hierarchy.manual_verification.as_mut().unwrap()["inputModality"] = json!("touch");
    for surface in ["browser", "native"] {
        let mut result = manual_ui_evidence_result();
        result["manualEvidence"]["environments"][0]["surface"] = json!(surface);
        result["manualEvidence"]["environments"][0]["inputCapabilities"] = if surface == "browser" { json!({"maxTouchPoints":1}) } else { json!({"touchscreen":true}) };
        result["manualEvidence"]["checks"][0]["actions"][0]["type"] = json!("scroll");
        result["manualEvidence"]["checks"][0]["actions"][0]["inputObservation"] = if surface == "browser" { json!({"pointerType":"touch","eventType":"pointermove","isTrusted":true}) } else { json!({"motionEventSource":4098,"action":"move"}) };
        let checked = apply_completion_evidence_gate(&fixture.board, "task-1", result, &json!({}));
        assert_eq!(checked["manualEvidenceGate"]["passed"], true, "{surface}: {checked}");
    }
}

#[test]
fn manual_touch_contract_checks_capability_and_observed_input_in_each_viewport() {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0].hierarchy.manual_verification = Some(json!({"version":1,"mode":"ui","inputModality":"touch","viewports":[{"width":360,"height":800},{"width":414,"height":896}]}));
    for defect in ["none", "zero_capability", "missing_touch"] {
        let mut result = manual_ui_evidence_result();
        result["manualEvidence"]["environments"][0]["inputCapabilities"] = json!({"maxTouchPoints":1});
        result["manualEvidence"]["checks"][0]["actions"][0]["inputObservation"] = json!({"pointerType":"touch","eventType":"pointerup","isTrusted":true});
        let mut second_environment = result["manualEvidence"]["environments"][0].clone();
        second_environment["id"] = json!("wide-phone");
        second_environment["viewport"] = json!({"width":414,"height":896});
        if defect == "zero_capability" { second_environment["inputCapabilities"]["maxTouchPoints"] = json!(0); }
        result["manualEvidence"]["environments"].as_array_mut().unwrap().push(second_environment);
        let mut second_check = result["manualEvidence"]["checks"][0].clone();
        second_check["environmentId"] = json!("wide-phone");
        if defect == "missing_touch" { second_check["actions"] = json!([{"type":"navigate","target":"List","observed":"Bread visible"}]); }
        result["manualEvidence"]["checks"].as_array_mut().unwrap().push(second_check);
        let checked = apply_completion_evidence_gate(&fixture.board, "task-1", result, &json!({}));
        assert_eq!(checked["manualEvidenceGate"]["passed"], defect == "none", "{defect}: {checked}");
    }
}

#[test]
fn manual_touch_contract_explicit_any_preserves_legacy_non_touch_evidence() {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0].hierarchy.manual_verification.as_mut().unwrap()["inputModality"] = json!("any");
    let mut result = manual_ui_evidence_result();
    result["manualEvidence"]["environments"][0]["inputCapabilities"] = json!({"maxTouchPoints":0});
    let checked = apply_completion_evidence_gate(&fixture.board, "task-1", result, &json!({}));
    assert_eq!(checked["manualEvidenceGate"]["passed"], true, "{checked}");
}

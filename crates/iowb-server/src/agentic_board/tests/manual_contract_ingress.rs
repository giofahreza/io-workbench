#[test]
fn manual_contract_ingress_normalizes_explicit_dimensions_without_scope_loss() {
    let mut run = board_fixture(json!({"command":"Verify UI","projectPath":"/tmp/project"}));
    let shorthand = json!({"version":1,"mode":"ui","viewports":["360x800", "414x896"]});
    let canonical = json!({"version":1,"mode":"ui","viewports":[{"width":360,"height":800},{"width":414,"height":896}]});
    let manual = BoardTask::manual(
        &mut run,
        serde_json::from_value(
            json!({"title":"Verify UI","kind":"manual_test","manualVerification":shorthand}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        manual.hierarchy.manual_verification,
        Some(canonical.clone())
    );
    let planned = task_from_json(
        &run,
        json!({"title":"Verify UI","kind":"manual_test","manualVerification":shorthand}),
        1,
        TASK_STATUS_BACKLOG,
    )
    .unwrap();
    assert_eq!(
        planned.hierarchy.manual_verification,
        Some(canonical.clone())
    );
    let drafts = sanitize_prompt_task_drafts(
        &json!({"tasks":[{"title":"Verify UI","kind":"manual_test","manualVerification":shorthand}]}),
        "Verify UI",
    );
    let trees = prepare_generated_prompt_task_trees(&mut run, drafts, "Verify UI").unwrap();
    assert_eq!(trees[0][0].hierarchy.manual_verification, Some(canonical));
}

#[test]
fn manual_contract_ingress_rejects_malformed_supplied_scope_before_allocating_id() {
    for scope in [
        json!({"version":1,"mode":"ui","viewports":["phone"]}),
        json!({"version":1,"mode":"ui","viewports":["0x800"]}),
        json!({"version":1,"mode":"ui","viewports":["360x800x2"]}),
        json!({"version":1,"mode":"ui","viewports":[{"width":360,"height":0}]}),
        json!({"version":1,"mode":"ui","viewports":[]}),
        json!({"version":2,"mode":"ui","viewports":["360x800"]}),
        json!({"version":1,"mode":"automatic","viewports":[]}),
    ] {
        let mut run = board_fixture(json!({"command":"Verify UI","projectPath":"/tmp/project"}));
        let before = serde_json::to_value(&run).unwrap();
        let result = BoardTask::manual(
            &mut run,
            serde_json::from_value(
                json!({"title":"Verify UI","kind":"manual_test","manualVerification":scope}),
            )
            .unwrap(),
        );
        assert!(result.is_err(), "invalid scope accepted: {scope}");
        assert_eq!(serde_json::to_value(&run).unwrap(), before);
    }
}

#[test]
fn manual_contract_ingress_rejects_invalid_edit_before_destroying_generated_descendants() {
    let mut run = board_fixture(json!({"command":"Verify UI","projectPath":"/tmp/project"}));
    run.tasks[0].status = TASK_STATUS_BACKLOG.into();
    let mut child = BoardTask::draft(
        &mut run,
        "Generated verification".into(),
        "Verify UI".into(),
    );
    child.hierarchy.parent_id = Some("task-1".into());
    child.hierarchy.level = TASK_LEVEL_TASK.into();
    child.task_origin = "hierarchy_breakdown".into();
    run.tasks.push(child);
    let before = serde_json::to_value(&run).unwrap();
    let request = serde_json::from_value(
        json!({"manualVerification":{"version":1,"mode":"ui","viewports":["phone"]}}),
    )
    .unwrap();
    assert!(edit_backlog_task(&mut run, "task-1", &request).is_err());
    assert_eq!(serde_json::to_value(&run).unwrap(), before);
}

#[test]
fn manual_contract_ingress_rejects_entire_generated_batch_before_allocation() {
    let mut run = board_fixture(json!({"command":"Verify UI","projectPath":"/tmp/project"}));
    let drafts = sanitize_prompt_task_drafts(
        &json!({"tasks":[
            {"title":"Valid shell","kind":"implementation"},
            {"title":"Invalid verification","kind":"manual_test","manualVerification":{"version":1,"mode":"ui","viewports":["phone"]}}
        ]}),
        "Verify UI",
    );
    let before = serde_json::to_value(&run).unwrap();
    assert!(prepare_generated_prompt_task_trees(&mut run, drafts, "Verify UI").is_err());
    assert_eq!(serde_json::to_value(&run).unwrap(), before);
}

#[tokio::test(flavor = "current_thread")]
async fn manual_contract_ingress_legacy_invalid_scope_stops_before_provider_attempt() {
    let (root, state, mut run, _, parent_id, leaf_id) = scheduler_approval_fixture().await;
    for task in &mut run.tasks {
        if task.id == parent_id || task.id == leaf_id {
            task.status = TASK_STATUS_TODO.into();
        }
        if task.id == leaf_id {
            task.task_type = TASK_KIND_MANUAL_TEST.into();
            task.hierarchy.manual_verification =
                Some(json!({"version":1,"mode":"ui","viewports":["360x800","414x896"]}));
        }
    }
    let original_scope = run
        .tasks
        .iter()
        .find(|task| task.id == leaf_id)
        .unwrap()
        .hierarchy
        .manual_verification
        .clone();
    refresh_hierarchy_rollups(&mut run);
    save_board(&state, &run).unwrap();
    let decision = prepare_board_execution_step(&state, "discussion-user", &run.id).unwrap();
    assert!(
        matches!(decision, BoardExecutionPreparation::Stop),
        "invalid persisted contract must not dispatch execution"
    );
    let current = load_user_board(&state, "discussion-user", &run.id)
        .unwrap()
        .board;
    let leaf = current
        .tasks
        .iter()
        .find(|task| task.id == leaf_id)
        .unwrap();
    assert_eq!(leaf.status, TASK_STATUS_BLOCKED);
    assert_eq!(leaf.attempt_count, 0);
    assert_eq!(
        leaf.hierarchy.manual_verification, original_scope,
        "no approved-history rewrite"
    );
    assert!(
        leaf.error
            .as_deref()
            .unwrap_or("")
            .contains("manualVerification")
    );
    assert!(current.provider_call_started_at.is_none());
    assert!(current.tasks.iter().all(|task| task.attempt_count == 0));
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn manual_contract_ingress_mcp_snapshots_are_bounded_runtime_artifacts_not_source_edits() {
    let mut fixture = manual_contract_runtime_fixture();
    let before = manual_contract_test_before_snapshot(&fixture);
    fs::create_dir_all(fixture.project.join(".playwright-mcp")).unwrap();
    fs::write(
        fixture
            .project
            .join(".playwright-mcp/page-2026-10-03T09-27-58-706Z.yml"),
        "",
    )
    .unwrap();
    fs::write(
        fixture
            .project
            .join(".playwright-mcp/page-2026-10-03T09-28-40-556Z.yml"),
        "- main [ref=e2]:\n  - heading \"Checklist\" [level=1]\n",
    )
    .unwrap();
    let delta = record_task_workspace_changes(&mut fixture.board, "task-1", before);
    let result = apply_completion_evidence_gate(
        &fixture.board,
        "task-1",
        manual_ui_evidence_result(),
        &delta,
    );
    assert_eq!(result["filePolicy"]["passed"], true, "{result}");
    assert_eq!(result["status"], "done");
    assert_eq!(
        result["manualEvidenceGate"]["artifacts"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "MCP YAML is not screenshot evidence"
    );
}

fn manual_contract_test_before_snapshot(fixture: &ManualUiEvidenceFixture) -> Value {
    let mut snapshot = capture_workspace_snapshot(fixture.project.to_str().unwrap());
    snapshot["scopeVersion"] = json!(fixture.board.tasks[0].hierarchy.scope_version);
    bind_manual_runtime_snapshot(&mut snapshot, &fixture.board.tasks[0]);
    snapshot
}

fn manual_contract_runtime_fixture() -> ManualUiEvidenceFixture {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0].attempt_count = 1;
    fixture.board.tasks[0].started_at = Some(Utc::now());
    fixture
}

#[test]
fn manual_contract_ingress_requires_generated_contract_but_allows_unapproved_manual_placeholder() {
    let mut run = board_fixture(json!({"command":"Verify UI","projectPath":"/tmp/project"}));
    for missing in [
        json!({"title":"Verify UI","kind":"manual_test"}),
        json!({"title":"Verify UI","kind":"manual_test","manualVerification":null}),
    ] {
        let before = serde_json::to_value(&run).unwrap();
        assert!(normalize_generated_manual_contracts(vec![missing.clone()]).is_err());
        let mut todo = missing.clone();
        todo["status"] = json!("todo");
        assert!(BoardTask::manual(&mut run, serde_json::from_value(todo).unwrap()).is_err());
        assert_eq!(serde_json::to_value(&run).unwrap(), before);
        let placeholder =
            BoardTask::manual(&mut run, serde_json::from_value(missing).unwrap()).unwrap();
        assert_eq!(placeholder.status, TASK_STATUS_BACKLOG);
        assert!(manual_task_contract_issue(&placeholder).is_some());
    }
    assert!(
        normalize_generated_manual_contracts(vec![
            json!({"title":"Build shell","kind":"implementation","manualVerification":null})
        ])
        .is_ok()
    );
    assert!(normalize_generated_manual_contracts(vec![json!({"title":"Wrapped verification","kind":"design","sourceKind":"manual_test","sourceLevel":"subtask","manualVerification":null})]).is_err());
}

#[test]
fn manual_contract_ingress_inherits_before_validation_and_never_repairs_approved_parent() {
    let fixture = manual_ui_evidence_fixture();
    let mut parent = fixture.board.tasks[0].clone();
    parent.hierarchy.manual_verification = Some(
        json!({"version":1,"mode":"ui","viewports":[{"width":360,"height":800},{"width":414,"height":896}]}),
    );
    for child_scope in [
        Value::Null,
        json!({"version":1,"mode":"inspection"}),
        json!({"version":1,"mode":"ui","viewports":["phone"]}),
    ] {
        let normalized = normalize_hierarchy_manual_contracts(
            vec![
                json!({"title":"Verify UI","kind":"manual_test","manualVerification":child_scope}),
            ],
            &parent,
        )
        .unwrap();
        assert_eq!(
            normalized[0]["manualVerification"],
            parent.hierarchy.manual_verification.clone().unwrap()
        );
    }
    parent.hierarchy.manual_verification =
        Some(json!({"version":1,"mode":"ui","viewports":["360x800","414x896"]}));
    let original = serde_json::to_value(&parent).unwrap();
    assert!(
        normalize_hierarchy_manual_contracts(
            vec![json!({"title":"Verify UI","kind":"manual_test"})],
            &parent
        )
        .is_err()
    );
    assert!(validate_hierarchy_breakdown_parent(&fixture.board, &parent).is_err());
    assert_eq!(serde_json::to_value(&parent).unwrap(), original);
}

#[test]
fn manual_contract_ingress_discussion_split_and_research_import_reject_invalid_batch_atomically() {
    let mut run = board_fixture(json!({"command":"Build UI","projectPath":"/tmp/project"}));
    run.tasks[0].status = TASK_STATUS_BACKLOG.into();
    let items = json!([
        {"title":"Build shell","kind":"implementation"},
        {"title":"Verify UI","kind":"manual_test","manualVerification":{"version":1,"mode":"ui","viewports":["phone"]}}
    ]);
    let before = serde_json::to_value(&run).unwrap();
    assert!(split_discussion_item(&mut run, "task-1", &json!({"items":items})).is_err());
    assert_eq!(serde_json::to_value(&run).unwrap(), before);
    let source = run.tasks[0].clone();
    assert!(
        append_research_planning_items(&mut run, &source, items.as_array().unwrap().clone())
            .is_err()
    );
    assert_eq!(serde_json::to_value(&run).unwrap(), before);
}

#[test]
fn manual_contract_ingress_repair_edit_versions_scope_and_completion_remains_strict() {
    let mut fixture = manual_ui_evidence_fixture();
    fixture.board.tasks[0].status = TASK_STATUS_BACKLOG.into();
    let old_scope = fixture.board.tasks[0].hierarchy.scope_version;
    let shorthand = json!({"version":1,"mode":"ui","viewports":["360x800","414x896"]});
    let request = serde_json::from_value(json!({"manualVerification":shorthand})).unwrap();
    edit_backlog_task(&mut fixture.board, "task-1", &request).unwrap();
    let task = &fixture.board.tasks[0];
    assert_eq!(task.hierarchy.scope_version, old_scope + 1);
    assert_eq!(task.status, TASK_STATUS_BACKLOG);
    assert_eq!(
        task.hierarchy.manual_verification.as_ref().unwrap()["viewports"][1],
        json!({"width":414,"height":896})
    );
    let mut result = manual_ui_evidence_result();
    result["manualEvidence"]["scopeVersion"] = json!(task.hierarchy.scope_version);
    result["manualEvidence"]["checks"][0]["actions"][0]["type"] = json!("inspect");
    let validation = manual_verification_evidence_validation(&fixture.board, task, &result);
    assert_eq!(validation["passed"], false);
    assert!(validation["issues"].to_string().contains("414x896"));
    assert!(
        validation["issues"]
            .to_string()
            .contains("actual mode-appropriate actions")
    );
    // Legacy approved strings still fail; only authoring is normalized.
    let mut historical = task.clone();
    historical.hierarchy.manual_verification = Some(shorthand);
    assert_eq!(
        manual_verification_evidence_validation(&fixture.board, &historical, &result)["passed"],
        false
    );
}

#[test]
fn manual_contract_ingress_mcp_exemption_rejects_stale_or_missing_before_identity() {
    for defect in [
        "missing",
        "task",
        "scope",
        "attempt",
        "started",
        "before_scope",
        "scope_absent",
    ] {
        let mut fixture = manual_ui_evidence_fixture();
        fixture.board.tasks[0].attempt_count = 1;
        fixture.board.tasks[0].started_at = Some(Utc::now());
        let mut before = manual_contract_test_before_snapshot(&fixture);
        match defect {
            "missing" => {
                before
                    .as_object_mut()
                    .unwrap()
                    .remove("manualRuntimeAttempt");
            }
            "task" => before["manualRuntimeAttempt"]["taskId"] = json!("another-task"),
            "scope" => before["manualRuntimeAttempt"]["scopeVersion"] = json!(0),
            "attempt" => before["manualRuntimeAttempt"]["attemptCount"] = json!(0),
            "started" => before["manualRuntimeAttempt"]["startedAt"] = Value::Null,
            "before_scope" => before["scopeVersion"] = json!(0),
            "scope_absent" => {
                before.as_object_mut().unwrap().remove("scopeVersion");
            }
            _ => unreachable!(),
        }
        fs::create_dir_all(fixture.project.join(".playwright-mcp")).unwrap();
        fs::write(
            fixture
                .project
                .join(".playwright-mcp/page-2026-10-03T09-28-40-556Z.yml"),
            "- main\n",
        )
        .unwrap();
        let delta = record_task_workspace_changes(&mut fixture.board, "task-1", before);
        assert!(
            delta.get("manualRuntimeAttempt").is_none(),
            "{defect}: no post-run attribution invented"
        );
        if matches!(defect, "before_scope" | "scope_absent") {
            assert!(delta.get("scopeVersion").is_none());
        }
        assert!(
            allowed_manual_mcp_snapshots(&fixture.board, &fixture.board.tasks[0], &delta)
                .is_empty(),
            "{defect}"
        );
        assert_eq!(
            apply_completion_evidence_gate(
                &fixture.board,
                "task-1",
                manual_ui_evidence_result(),
                &delta
            )["filePolicy"]["passed"],
            false,
            "{defect}"
        );
    }
}

#[test]
fn manual_contract_ingress_mcp_exemption_rejects_stale_delta_changes_and_nonmanual_kinds() {
    let mut fixture = manual_contract_runtime_fixture();
    let before = manual_contract_test_before_snapshot(&fixture);
    fs::create_dir_all(fixture.project.join(".playwright-mcp")).unwrap();
    let path = ".playwright-mcp/page-2026-10-03T09-28-40-556Z.yml";
    fs::write(fixture.project.join(path), "- main\n").unwrap();
    let original_delta = record_task_workspace_changes(&mut fixture.board, "task-1", before);
    assert!(
        allowed_manual_mcp_snapshots(&fixture.board, &fixture.board.tasks[0], &original_delta)
            .contains(path)
    );
    for defect in [
        "task",
        "scope",
        "attempt",
        "incomplete",
        "git",
        "hash",
        "previous",
        "deleted",
        "unknown",
        "reused",
    ] {
        let mut delta = original_delta.clone();
        match defect {
            "task" => delta["taskId"] = json!("another-task"),
            "scope" => delta["scopeVersion"] = json!(0),
            "attempt" => delta["manualRuntimeAttempt"]["attemptCount"] = json!(17),
            "incomplete" => delta["snapshotComplete"] = json!(false),
            "git" => delta["snapshotProvider"] = json!("git status"),
            "hash" => delta["changedBySubtask"][0]["afterHash"] = json!("wrong-hash"),
            "previous" => delta["changedBySubtask"][0]["beforeHash"] = json!("prior-content"),
            "deleted" => delta["changedBySubtask"][0]["afterStatus"] = json!(""),
            "unknown" => delta["changedBySubtask"][0]["classification"] = json!("unknown_change"),
            "reused" => {
                delta["reusedEvidence"] = json!([{"path":path,"classification":"reused_same_task"}])
            }
            _ => unreachable!(),
        }
        assert!(
            allowed_manual_mcp_snapshots(&fixture.board, &fixture.board.tasks[0], &delta)
                .is_empty(),
            "{defect}"
        );
    }
    for kind in [
        TASK_KIND_QA,
        TASK_KIND_REVIEW,
        TASK_KIND_RESEARCH,
        TASK_KIND_DESIGN,
        TASK_KIND_TEST_IMPLEMENTATION,
    ] {
        let mut task = fixture.board.tasks[0].clone();
        task.task_type = kind.into();
        assert!(
            allowed_manual_mcp_snapshots(&fixture.board, &task, &original_delta).is_empty(),
            "{kind}"
        );
    }
    fs::write(
        fixture.project.join(path),
        "- main: changed after snapshot\n",
    )
    .unwrap();
    assert!(
        allowed_manual_mcp_snapshots(&fixture.board, &fixture.board.tasks[0], &original_delta)
            .is_empty()
    );
}

#[test]
fn manual_contract_ingress_mcp_paths_content_and_size_fail_closed() {
    for (path, bytes) in [
        (".playwright-mcp/source.yml", b"- main\n".to_vec()),
        (
            ".playwright-mcp/page-2026-13-03T09-28-40-556Z.yml",
            b"- main\n".to_vec(),
        ),
        (
            ".playwright-mcp/page-2026-10-03T09-28-40-556Z.yml",
            b"module.exports = {};\n".to_vec(),
        ),
        (
            ".playwright-mcp/page-2026-10-03T09-28-40-556Z.yml",
            b"server:\n  password: something\n".to_vec(),
        ),
        (
            ".playwright-mcp/page-2026-10-03T09-28-40-556Z.yml",
            vec![0xff, 0xfe],
        ),
        (
            ".playwright-mcp/page-2026-10-03T09-28-40-556Z.yml",
            b"- main\0\n".to_vec(),
        ),
        (
            ".playwright-mcp/page-2026-10-03T09-28-40-556Z.yml",
            vec![b' '; 1_048_577],
        ),
        (
            ".playwright-mcp/nested/page-2026-10-03T09-28-40-556Z.yml",
            b"- main\n".to_vec(),
        ),
        (
            ".playwright-mcp/page-2026-10-03T09-28-40-556Z.js",
            b"- main\n".to_vec(),
        ),
    ] {
        let mut fixture = manual_contract_runtime_fixture();
        let before = manual_contract_test_before_snapshot(&fixture);
        fs::create_dir_all(fixture.project.join(path).parent().unwrap()).unwrap();
        fs::write(fixture.project.join(path), bytes).unwrap();
        let delta = record_task_workspace_changes(&mut fixture.board, "task-1", before);
        assert!(
            allowed_manual_mcp_snapshots(&fixture.board, &fixture.board.tasks[0], &delta)
                .is_empty(),
            "{path}"
        );
        assert_eq!(
            apply_completion_evidence_gate(
                &fixture.board,
                "task-1",
                manual_ui_evidence_result(),
                &delta
            )["filePolicy"]["passed"],
            false,
            "{path}"
        );
    }
    let fixture = manual_ui_evidence_fixture();
    for path in [
        "/tmp/page-2026-10-03T09-28-40-556Z.yml",
        ".playwright-mcp/../page-2026-10-03T09-28-40-556Z.yml",
        ".playwright-mcp\\page-2026-10-03T09-28-40-556Z.yml",
    ] {
        assert!(
            manual_mcp_snapshot_bytes(fixture.project.to_str().unwrap(), path, "anything")
                .is_none()
        );
    }
}

#[cfg(unix)]
#[test]
fn manual_contract_ingress_mcp_exemption_rejects_internal_and_external_symlinks() {
    let fixture = manual_ui_evidence_fixture();
    let path = ".playwright-mcp/page-2026-10-03T09-28-40-556Z.yml";
    fs::create_dir_all(fixture.project.join(".playwright-mcp")).unwrap();
    fs::write(fixture.project.join("snapshot.yml"), "- main\n").unwrap();
    let hash = format!("{:x}", Sha256::digest(b"- main\n"));
    std::os::unix::fs::symlink(
        fixture.project.join("snapshot.yml"),
        fixture.project.join(path),
    )
    .unwrap();
    assert!(manual_mcp_snapshot_bytes(fixture.project.to_str().unwrap(), path, &hash).is_none());
    fs::remove_file(fixture.project.join(path)).unwrap();
    let external = manual_ui_evidence_fixture();
    fs::write(external.project.join("snapshot.yml"), "- main\n").unwrap();
    std::os::unix::fs::symlink(
        external.project.join("snapshot.yml"),
        fixture.project.join(path),
    )
    .unwrap();
    assert!(manual_mcp_snapshot_bytes(fixture.project.to_str().unwrap(), path, &hash).is_none());
    fs::remove_file(fixture.project.join(path)).unwrap();
    fs::remove_dir(fixture.project.join(".playwright-mcp")).unwrap();
    fs::create_dir_all(fixture.project.join("runtime")).unwrap();
    fs::write(
        fixture
            .project
            .join("runtime/page-2026-10-03T09-28-40-556Z.yml"),
        "- main\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        fixture.project.join("runtime"),
        fixture.project.join(".playwright-mcp"),
    )
    .unwrap();
    assert!(manual_mcp_snapshot_bytes(fixture.project.to_str().unwrap(), path, &hash).is_none());
}

#[test]
fn manual_contract_ingress_rejects_invalid_suggestions_without_partial_tasks_or_panic() {
    let mut run = board_fixture(json!({"command":"Build UI","projectPath":"/tmp/project"}));
    let before_tasks = serde_json::to_value(&run.tasks).unwrap();
    let proposed = json!({"suggestedBacklogTasks":[
        {"title":"Build follow-up shell","kind":"implementation"},
        {"title":"Verify follow-up UI","kind":"manual_test","manualVerification":{"version":1,"mode":"ui","viewports":["phone"]}}
    ]});
    assert!(append_suggested_backlog_tasks_from_result(&mut run, "task-1", &proposed).is_empty());
    assert_eq!(serde_json::to_value(&run.tasks).unwrap(), before_tasks);
    assert!(
        serde_json::to_value(&run.logs)
            .unwrap()
            .to_string()
            .contains("manualVerification")
    );
}

#[test]
fn manual_contract_ingress_mcp_retry_keeps_auxiliary_baseline_without_hiding_edits() {
    for prior_edit in ["unchanged", "changed", "deleted", "forbidden_source"] {
        let mut fixture = manual_ui_evidence_fixture();
        fixture.board.tasks[0].attempt_count = 1;
        fixture.board.tasks[0].started_at = Some(Utc::now());
        let before = manual_contract_test_before_snapshot(&fixture);
        fs::create_dir_all(fixture.project.join(".playwright-mcp")).unwrap();
        let prior_path = ".playwright-mcp/page-2026-10-03T09-28-40-556Z.yml";
        fs::write(fixture.project.join(prior_path), "- main\n").unwrap();
        if prior_edit == "forbidden_source" {
            fs::write(fixture.project.join("index.html"), "forbidden edit").unwrap();
        }
        let original = record_task_workspace_changes(&mut fixture.board, "task-1", before);
        assert_eq!(original["runtimeArtifacts"][0]["path"], prior_path);
        fixture.board.tasks[0].attempt_count = 2;
        fixture.board.tasks[0].started_at = Some(Utc::now());
        let before_retry = manual_contract_test_before_snapshot(&fixture);
        match prior_edit {
            "changed" => fs::write(fixture.project.join(prior_path), "- main: edited\n").unwrap(),
            "deleted" => fs::remove_file(fixture.project.join(prior_path)).unwrap(),
            _ => {}
        }
        let new_path = ".playwright-mcp/page-2026-10-03T09-29-40-556Z.yml";
        fs::write(fixture.project.join(new_path), "- main\n").unwrap();
        let delta = record_task_workspace_changes(&mut fixture.board, "task-1", before_retry);
        assert!(
            !delta["reusedEvidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == prior_path)
        );
        assert_eq!(
            fixture.board.change_ledger[0], original,
            "prior attempt evidence is untouched"
        );
        let result = apply_completion_evidence_gate(
            &fixture.board,
            "task-1",
            manual_ui_evidence_result(),
            &delta,
        );
        assert_eq!(
            result["filePolicy"]["passed"],
            prior_edit == "unchanged",
            "{prior_edit}: {result}"
        );
        if prior_edit == "forbidden_source" {
            assert_eq!(result["filePolicy"]["violations"], json!(["index.html"]));
        }
        if matches!(prior_edit, "changed" | "deleted") {
            assert_eq!(result["filePolicy"]["violations"], json!([prior_path]));
        }
    }
}

#[test]
fn manual_contract_ingress_mcp_exemption_caps_aggregate_output_and_preserves_source_denial() {
    for (count, bytes) in [(65, 8), (5, 900_000)] {
        let mut fixture = manual_contract_runtime_fixture();
        let before = manual_contract_test_before_snapshot(&fixture);
        fs::create_dir_all(fixture.project.join(".playwright-mcp")).unwrap();
        for index in 0..count {
            fs::write(
                fixture.project.join(format!(
                    ".playwright-mcp/page-2026-10-03T09-28-40-{index:03}Z.yml"
                )),
                format!("- main {}\n", "x".repeat(bytes)),
            )
            .unwrap();
        }
        let delta = record_task_workspace_changes(&mut fixture.board, "task-1", before);
        assert!(
            allowed_manual_mcp_snapshots(&fixture.board, &fixture.board.tasks[0], &delta)
                .is_empty()
        );
    }
    let mut fixture = manual_contract_runtime_fixture();
    let before = manual_contract_test_before_snapshot(&fixture);
    fs::create_dir_all(fixture.project.join(".playwright-mcp")).unwrap();
    fs::write(
        fixture
            .project
            .join(".playwright-mcp/page-2026-10-03T09-28-40-556Z.yml"),
        "- main\n",
    )
    .unwrap();
    fs::write(
        fixture.project.join("index.html"),
        "unauthorized source edit",
    )
    .unwrap();
    let delta = record_task_workspace_changes(&mut fixture.board, "task-1", before);
    let result = apply_completion_evidence_gate(
        &fixture.board,
        "task-1",
        manual_ui_evidence_result(),
        &delta,
    );
    assert_eq!(result["filePolicy"]["violations"], json!(["index.html"]));
    assert_eq!(result["status"], "needs_followup");
}

#[test]
fn manual_contract_ingress_mcp_exemption_requires_an_actual_started_attempt() {
    for missing in ["attempt", "start"] {
        let mut fixture = manual_contract_runtime_fixture();
        if missing == "attempt" {
            fixture.board.tasks[0].attempt_count = 0;
        } else {
            fixture.board.tasks[0].started_at = None;
        }
        let before = manual_contract_test_before_snapshot(&fixture);
        fs::create_dir_all(fixture.project.join(".playwright-mcp")).unwrap();
        fs::write(
            fixture
                .project
                .join(".playwright-mcp/page-2026-10-03T09-28-40-556Z.yml"),
            "- main\n",
        )
        .unwrap();
        let delta = record_task_workspace_changes(&mut fixture.board, "task-1", before);
        assert!(
            allowed_manual_mcp_snapshots(&fixture.board, &fixture.board.tasks[0], &delta)
                .is_empty()
        );
        assert!(delta.get("runtimeArtifacts").is_none());
        assert_eq!(
            apply_completion_evidence_gate(
                &fixture.board,
                "task-1",
                manual_ui_evidence_result(),
                &delta
            )["filePolicy"]["passed"],
            false
        );
    }
}

#[test]
fn manual_contract_ingress_prompt_requires_real_app_artifact_not_reconstructed_data_url() {
    let fixture = manual_ui_evidence_fixture();
    let prompt = build_manual_test_prompt(&fixture.board, &fixture.board.tasks[0], 0);
    assert!(prompt.contains("on-disk SHA-256"));
    assert!(prompt.contains("browser-retrieved response bytes match that hash"));
    assert!(prompt.contains("not normalized DOM serialization"));
    assert!(prompt.contains("retyped or model-built data URL"));
    assert!(prompt.contains("page.setContent"));
    assert!(prompt.contains("identity cannot be verified, return blocked"));
}

#[tokio::test(flavor = "current_thread")]
async fn manual_contract_ingress_legacy_parent_stops_before_breakdown_provider() {
    let (root, state, mut run, _, parent_id, leaf_id) = scheduler_approval_fixture().await;
    run.tasks.retain(|task| task.id != leaf_id);
    let parent = run
        .tasks
        .iter_mut()
        .find(|task| task.id == parent_id)
        .unwrap();
    parent.status = TASK_STATUS_TODO.into();
    parent.task_type = TASK_KIND_MANUAL_TEST.into();
    parent.hierarchy.manual_verification =
        Some(json!({"version":1,"mode":"ui","viewports":["360x800","414x896"]}));
    let scope = parent.hierarchy.manual_verification.clone();
    refresh_hierarchy_rollups(&mut run);
    save_board(&state, &run).unwrap();
    assert!(matches!(
        prepare_board_execution_step(&state, "discussion-user", &run.id).unwrap(),
        BoardExecutionPreparation::Stop
    ));
    let current = load_user_board(&state, "discussion-user", &run.id)
        .unwrap()
        .board;
    let parent = current
        .tasks
        .iter()
        .find(|task| task.id == parent_id)
        .unwrap();
    assert_eq!(parent.status, TASK_STATUS_BLOCKED);
    assert_eq!(parent.hierarchy.manual_verification, scope);
    assert!(current.tasks.iter().all(|task| task.attempt_count == 0));
    assert!(current.provider_call_started_at.is_none());
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn manual_contract_ingress_mcp_console_runtime_output_is_narrowly_allowed() {
    for (name, content, expected) in [
        ("console-2026-10-04T13-28-12-478Z.log", "[      54ms] [ERROR] Failed to load resource: 404 @ http://127.0.0.1:18104/favicon.ico:0\n", true),
        ("console-2026-10-04T13-28-12-478Z.log", "", true),
        ("console-2026-10-04T13-28-12-478Z.log", "[       0ms] [LOG] hello\n[      12ms] [WARNING] warning\n", true),
        ("console-2026-13-04T13-28-12-478Z.log", "[       0ms] [ERROR] error\n", false),
        ("console-latest.log", "[       0ms] [ERROR] error\n", false),
        ("console-2026-10-04T13-28-12-478Z.js", "[       0ms] [ERROR] error\n", false),
        ("console-2026-10-04T13-28-12-478Z.log", "export default { enabled: true };\n", false),
        ("console-2026-10-04T13-28-12-478Z.log", "[       0ms] [EXEC] arbitrary\n", false),
        ("console-2026-10-04T13-28-12-478Z.log", "[       0ms] [ERROR] error\narbitrary second line\n", false),
    ] {
        let mut fixture = manual_contract_runtime_fixture();
        let before = manual_contract_test_before_snapshot(&fixture);
        fs::create_dir_all(fixture.project.join(".playwright-mcp")).unwrap();
        fs::write(fixture.project.join(".playwright-mcp").join(name), content).unwrap();
        let delta = record_task_workspace_changes(&mut fixture.board, "task-1", before);
        let result = apply_completion_evidence_gate(&fixture.board, "task-1", manual_ui_evidence_result(), &delta);
        assert_eq!(result["filePolicy"]["passed"], expected, "{name} {content:?}: {result}");
        if expected {
            assert_eq!(result["status"], "done");
            assert_eq!(result["manualEvidenceGate"]["artifacts"].as_array().unwrap().len(), 1);
        }
    }
}

#[test]
fn manual_contract_ingress_mcp_console_rejects_prior_edits_and_oversized_logs() {
    for scenario in ["changed", "deleted", "oversized"] {
        let mut fixture = manual_contract_runtime_fixture();
        fs::create_dir_all(fixture.project.join(".playwright-mcp")).unwrap();
        let path = ".playwright-mcp/console-2026-10-04T13-28-12-478Z.log";
        if scenario != "oversized" {
            fs::write(fixture.project.join(path), "[       0ms] [LOG] original\n").unwrap();
        }
        let before = manual_contract_test_before_snapshot(&fixture);
        match scenario {
            "deleted" => fs::remove_file(fixture.project.join(path)).unwrap(),
            "changed" => fs::write(fixture.project.join(path), "[       1ms] [LOG] changed\n").unwrap(),
            _ => fs::write(fixture.project.join(path), format!("[       0ms] [LOG] {}\n", "x".repeat(1_048_576))).unwrap(),
        }
        let delta = record_task_workspace_changes(&mut fixture.board, "task-1", before);
        let result = apply_completion_evidence_gate(&fixture.board, "task-1", manual_ui_evidence_result(), &delta);
        assert_eq!(result["filePolicy"]["passed"], false, "{scenario}: {result}");
        assert_eq!(result["filePolicy"]["violations"], json!([path]));
    }
}

#[test]
fn manual_contract_retry_prompt_preserves_current_scope_and_contract() {
    let mut fixture = manual_contract_runtime_fixture();
    fixture.board.tasks[0].hierarchy.scope_version = 5;
    for failed in [false, true] {
        fixture.board.tasks[0].tdd_phase = if failed { "" } else { "fix_pending" }.to_string();
        fixture.board.tasks[0].status = if failed { "failed" } else { "todo" }.to_string();
        let prompt = build_task_execution_prompt(&fixture.board, &fixture.board.tasks[0], 0);
        assert!(prompt.contains("Current scopeVersion: 5"), "retry lost current scope: {prompt}");
        assert!(prompt.contains("manualEvidence"));
        assert!(prompt.contains("manualVerification"));
        assert!(prompt.contains("test-results/manual-evidence/"));
        assert!(!prompt.contains("Prompt template: Fix"));
    }
    let mut implementation = fixture.board.tasks[0].clone();
    implementation.task_type = TASK_KIND_IMPLEMENTATION.to_string();
    implementation.qa_fix_task = true;
    implementation.tdd_phase = "fix_pending".to_string();
    let prompt = build_task_execution_prompt(&fixture.board, &implementation, 0);
    assert!(prompt.contains("Prompt template: Fix"), "true implementation QA fix routing changed");
}

#[test]
fn manual_contract_prompt_schema_uses_current_numeric_scope_version() {
    let mut fixture = manual_contract_runtime_fixture();
    for scope_version in [1, 6, 42] {
        fixture.board.tasks[0].hierarchy.scope_version = scope_version;
        for (status, phase) in [("todo", ""), ("todo", "fix_pending"), ("failed", "")] {
            fixture.board.tasks[0].status = status.to_string();
            fixture.board.tasks[0].tdd_phase = phase.to_string();
            let prompt = build_task_execution_prompt(&fixture.board, &fixture.board.tasks[0], 0);
            let schema = prompt
                .rsplit_once("Return JSON only, with this schema:")
                .unwrap()
                .1;
            let scope_line = schema
                .lines()
                .find(|line| line.trim_start().starts_with("\"scopeVersion\":"))
                .unwrap();
            let value_text = scope_line
                .split_once(':')
                .unwrap()
                .1
                .trim()
                .trim_end_matches(',');
            let value: Value = serde_json::from_str(value_text).unwrap();
            assert_eq!(
                value.as_u64(),
                Some(scope_version),
                "schema must show the current numeric scope, not a quoted placeholder: {scope_line}"
            );
        }
    }
}

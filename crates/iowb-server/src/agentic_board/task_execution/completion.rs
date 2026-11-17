fn apply_completion_evidence_gate(
    run: &AgenticBoard,
    task_id: &str,
    mut parsed: Value,
    change_summary: &Value,
) -> Value {
    if !parsed_status_done(Some(&parsed)) {
        return parsed;
    }
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id) else {
        return parsed;
    };
    if uses_hierarchical_orchestration(run) {
        let commands = normalize_string_list(parsed.get("commandsRun"));
        let evidence = normalize_string_list(parsed.get("evidence"));
        let external_effect_evidence = external_side_effect_evidence(&parsed);
        let attributable_file_count = change_summary_attributable_file_count(change_summary);
        let kind = canonical_task_kind(task);
        let manual_environment = manual_test_environment_evidence(&parsed);
        let manual_steps = manual_test_steps_evidence(&parsed);
        let manual_result = manual_test_result_evidence(&parsed);
        let manual_verification = (kind == TASK_KIND_MANUAL_TEST)
            .then(|| manual_verification_evidence_validation(run, task, &parsed));
        let mut structured_manual_issues = manual_verification.as_ref()
            .map(|validation| normalize_string_list(validation.get("issues")))
            .unwrap_or_default();
        structured_manual_issues.extend(source_manual_verification_issues(run, task));
        let allowed_runtime_artifacts = allowed_manual_mcp_snapshots(run, task, change_summary);
        let file_policy_violations = dedupe_strings(
            task_file_edit_policy_violations(task, change_summary)
                .into_iter()
                .filter(|path| !allowed_runtime_artifacts.contains(path))
                .chain(automatic_qa_fix_file_violations(run, task, change_summary))
                .chain(task_package_file_policy_violations(run, task, change_summary))
                .collect(),
        );
        let file_policy_valid = file_policy_violations.is_empty();
        let has_code_evidence = attributable_file_count > 0;
        let has_validation_evidence = !commands.is_empty() || !evidence.is_empty();
        let manual_evidence_valid = kind != TASK_KIND_MANUAL_TEST
            || (!manual_steps.is_empty()
                && manual_result
                    .as_deref()
                    .is_some_and(|result| manual_task_result_is_successful(run, task, result)));
        let read_only_baseline = read_only_red_baseline_capture(run, task);
        let kind_evidence_valid = match kind {
            TASK_KIND_TEST_IMPLEMENTATION if read_only_baseline => {
                verified_unchanged_red_baseline(run, task, change_summary)
            }
            TASK_KIND_IMPLEMENTATION
            | TASK_KIND_TEST_IMPLEMENTATION
            | TASK_KIND_FIX
            | TASK_KIND_MIGRATION
            | TASK_KIND_REVERT
            | TASK_KIND_CLEANUP
            | TASK_KIND_REVISION
            | TASK_KIND_REPLACEMENT => has_code_evidence,
            TASK_KIND_RESEARCH | TASK_KIND_DESIGN | TASK_KIND_QA | TASK_KIND_REVIEW => {
                has_validation_evidence
            }
            TASK_KIND_MANUAL_TEST => manual_evidence_valid,
            _ => has_code_evidence || has_validation_evidence,
        };
        let external_effect_evidence_valid =
            task.hierarchy.side_effects.is_empty() || !external_effect_evidence.is_empty();
        let manual_environment_valid = kind != TASK_KIND_MANUAL_TEST
            || manual_test_environment_is_complete(&manual_environment);
        let valid = kind_evidence_valid
            && external_effect_evidence_valid
            && manual_environment_valid
            && manual_evidence_valid
            && structured_manual_issues.is_empty()
            && file_policy_valid;
        if let Some(object) = parsed.as_object_mut() {
            if kind == TASK_KIND_MANUAL_TEST {
                object.insert("manualEvidenceGate".to_string(), manual_verification.clone().unwrap_or(Value::Null));
                object.insert("manualTestSteps".to_string(), json!(manual_steps.clone()));
                object.insert(
                    "manualTestResult".to_string(),
                    manual_result
                        .clone()
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                );
                if !manual_environment.is_null() {
                    object.insert(
                        "manualTestEnvironment".to_string(),
                        manual_environment.clone(),
                    );
                }
            }
            object.insert(
                "filePolicy".to_string(),
                json!({
                    "passed": file_policy_valid,
                    "violations": file_policy_violations.clone(),
                }),
            );
            if !valid {
                let mut issues = object
                    .get("remainingIssues")
                    .map(|value| normalize_string_list(Some(value)))
                    .unwrap_or_default();
                issues.extend(structured_manual_issues);
                issues.push(format!(
                    "Completion evidence gate failed for {kind}: provide the evidence required by the subtask kind."
                ));
                if read_only_baseline && !kind_evidence_valid {
                    issues.push("Read-only baseline capture requires current server-verified assertion-red evidence and a complete unchanged workspace; do not modify files to satisfy this task.".to_string());
                }
                if !external_effect_evidence_valid {
                    issues.push(
                            "Declared external side effects require explicit externalSideEffects evidence describing what changed or was not changed."
                                .to_string(),
                    );
                }
                if !manual_environment_valid {
                    issues.push(
                        "Manual-test completion requires manualTestEnvironment with deviceOrEmulator, appVersion, and backendUrl."
                            .to_string(),
                    );
                }
                if !manual_evidence_valid {
                    if manual_steps.is_empty() {
                        issues.push(
                            "Manual-test completion requires manualTestSteps with each observed step."
                                .to_string(),
                        );
                    }
                    match manual_result.as_deref() {
                        None => issues.push(
                            "Manual-test completion requires manualTestResult with the overall observed result."
                                .to_string(),
                        ),
                        Some(_) => issues.push(
                            "Manual-test completion cannot be done when manualTestResult reports a failure or blocked check."
                                .to_string(),
                        ),
                    }
                }
                if !file_policy_valid {
                    issues.push(format!(
                        "File-edit policy failed for {kind}: this subtask kind cannot modify these workspace files: {}.",
                        file_policy_violations.join(", ")
                    ));
                }
                let issues = dedupe_strings(issues);
                object.insert(
                    "evidenceGate".to_string(),
                    json!({"passed": false, "kind": kind, "issues": issues.clone()}),
                );
                object.insert("status".to_string(), json!("needs_followup"));
                object.insert("qaResult".to_string(), json!("blocked"));
                object.insert("remainingIssues".to_string(), json!(issues));
            } else {
                object.insert(
                    "evidenceGate".to_string(),
                    json!({"passed": true, "kind": kind}),
                );
            }
        }
        if !valid {
            return parsed;
        }
        return parsed;
    }
    if matches!(
        task.task_type.as_str(),
        "qa" | "test" | "validation" | "final_qa"
    ) {
        return parsed;
    }
    let changed_files = normalize_string_list(parsed.get("changedFiles"));
    let commands = normalize_string_list(parsed.get("commandsRun"));
    let evidence = normalize_string_list(parsed.get("evidence"));
    let touched_count = change_summary
        .get("touchedFileCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if touched_count > 0 || !changed_files.is_empty() || !commands.is_empty() {
        return parsed;
    }
    if evidence
        .iter()
        .any(|item| item.len() > 12 && !item.to_lowercase().contains("not run"))
    {
        return parsed;
    }
    if let Some(object) = parsed.as_object_mut() {
        let mut issues = object
            .get("remainingIssues")
            .map(|value| normalize_string_list(Some(value)))
            .unwrap_or_default();
        issues.push(
            "Completion evidence gate failed: no changed files, commands, or concrete evidence were reported."
                .to_string(),
        );
        object.insert("status".to_string(), json!("needs_followup"));
        object.insert("qaResult".to_string(), json!("blocked"));
        object.insert("remainingIssues".to_string(), json!(dedupe_strings(issues)));
    }
    parsed
}

// A generated test-writing branch can have a separate, explicitly read-only
// baseline capture leaf. It must not be forced to edit tests just to satisfy
// its kind, nor can merely claiming a red run replace server-owned evidence.
fn read_only_red_baseline_capture(run: &AgenticBoard, task: &BoardTask) -> bool {
    if canonical_task_kind(task) != TASK_KIND_TEST_IMPLEMENTATION
        || !hierarchical_task_requires_red_baseline(run, task)
        || !task.hierarchy.planned_files.is_empty()
    { return false; }
    let title = task.title.to_ascii_lowercase();
    let scope = format!("{} {}", task.details, task.description).to_ascii_lowercase();
    (title.starts_with("run ") || title.starts_with("capture "))
        && title.contains("capture") && title.contains("baseline")
        && (scope.contains("do not modify any source file")
            || (scope.contains("do not modify")
                && scope.contains("or any other source file during this task")))
}

fn verified_unchanged_red_baseline(run: &AgenticBoard, task: &BoardTask, delta: &Value) -> bool {
    let Some(validation) = task.deterministic_validation.as_ref() else { return false; };
    let Some(fingerprint) = hierarchical_validation_test_fingerprint(run) else { return false; };
    let verified = delta.get("snapshotComplete").and_then(Value::as_bool) == Some(true)
        && delta.get("touchedFileCount").and_then(Value::as_u64) == Some(0)
        && delta.get("unknownChangeCount").and_then(Value::as_u64) == Some(0)
        && change_summary_attributable_file_count(delta) == 0
        && validation.get("expectedRedAccepted").and_then(Value::as_bool) == Some(true)
        && validation.get("gatePassed").and_then(Value::as_bool) == Some(true)
        && validation.get("passed").and_then(Value::as_bool) == Some(false)
        && validation.get("scopeVersion").and_then(Value::as_u64) == Some(task.hierarchy.scope_version)
        && validation.get("testSuiteFingerprint") == Some(&fingerprint)
        && task.qa_baseline_validation.as_ref() == Some(validation)
        && validation.get("redBaselineEvidence").and_then(Value::as_array).is_some_and(|items| !items.is_empty());
    if !verified { return false; }
    // The provider delta predates deterministic validation. Even a direct
    // Node process can execute a test that writes files: verify its current
    // workspace too before accepting an explicitly read-only capture.
    let current = capture_workspace_snapshot(&run.project_path);
    complete_validation_snapshot(&current)
        && current.get("filesByPath")
            == run.latest_workspace_snapshot.as_ref().and_then(|snapshot| snapshot.get("filesByPath"))
}

fn manual_task_result_is_successful(run: &AgenticBoard, task: &BoardTask, result: &str) -> bool {
    let baseline_capture = hierarchical_task_requires_red_baseline(run, task);
    let text = result.to_ascii_lowercase();
    if baseline_capture
        && [
            "blocked",
            "not captured",
            "not actually captured",
            "not successfully captured",
            "capture failed",
            "failed capture",
            "capture failure",
            "did not capture",
            "failed to",
            "unable to",
            "could not",
            "couldn't",
            "cannot",
            "can't",
            "timed out",
            "timeout",
            "syntax error",
            "missing runner",
        ]
        .iter()
        .any(|problem| text.contains(problem))
    {
        return false;
    }
    if manual_test_result_is_successful(result) {
        return true;
    }
    if !baseline_capture {
        return false;
    }
    let Some(validation) = task.deterministic_validation.as_ref() else {
        return false;
    };
    if validation
        .get("expectedRedAccepted")
        .and_then(Value::as_bool)
        != Some(true)
        || validation.get("gatePassed").and_then(Value::as_bool) != Some(true)
        || validation.get("passed").and_then(Value::as_bool) != Some(false)
        || validation.get("scopeVersion").and_then(Value::as_u64)
            != Some(u64::from(task.hierarchy.scope_version))
        || task.qa_baseline_validation.as_ref() != Some(validation)
        || !validation
            .get("redBaselineEvidence")
            .and_then(Value::as_array)
            .is_some_and(|evidence| !evidence.is_empty())
    {
        return false;
    }
    // A verified RED capture may truthfully mention failing tests after an
    // explicit successful capture verdict. Do not treat that detail as a
    // failed manual operation, or extend this exception to ordinary app QA.
    let verdict = text.split([';', '\n']).next().unwrap_or("").trim();
    let leading = verdict
        .split(|c: char| !c.is_ascii_alphabetic())
        .next()
        .unwrap_or("");
    matches!(leading, "pass" | "passed")
        && verdict.contains("baseline")
        && verdict.contains("captured")
        && manual_test_result_is_successful(verdict)
}

fn parsed_qa_passed(parsed: Option<&Value>) -> bool {
    !matches!(
        parsed
            .and_then(|value| value.get("qaResult"))
            .and_then(Value::as_str),
        Some("fail" | "blocked")
    )
}

fn apply_task_result_to_board(board: &mut AgenticBoard, task_id: &str, parsed: &Value) {
    for file in normalize_string_list(parsed.get("changedFiles")) {
        board.change_ledger.push(json!({
            "taskId": task_id,
            "path": file,
            "reportedAt": Utc::now(),
        }));
    }
    // commandsRun is a provider-reported list, not per-command execution evidence.
    // Keep it in the task result/transcript; the execution path records actual
    // deterministic validation separately. A blocked manual outcome must not
    // turn a passing test command into a failed validation (or the reverse).
    // Do not rewrite historical validation records here.
}

fn record_task_workspace_changes(run: &mut AgenticBoard, task_id: &str, before: Value) -> Value {
    let after = capture_workspace_snapshot(&run.project_path);
    let mut summary = summarize_workspace_delta(task_id, &before, &after);
    if let Some(task) = run.tasks.iter().find(|task| task.id == task_id) {
        retain_same_task_workspace_evidence(task, &before, &after, &mut summary);
        if canonical_task_kind(task) == TASK_KIND_MANUAL_TEST
            && before.get("scopeVersion").and_then(Value::as_u64) == Some(task.hierarchy.scope_version)
            && before.get("manualRuntimeAttempt") == Some(&manual_runtime_attempt_identity(task))
        {
            // Identity was captured BEFORE execution, not invented when the
            // result arrives. Do not bless stale snapshots or reused history.
            summary["manualRuntimeAttempt"] = before["manualRuntimeAttempt"].clone();
        }
        let runtime_paths = allowed_manual_mcp_snapshots(run, task, &summary);
        if !runtime_paths.is_empty() {
            summary["runtimeArtifacts"] = json!(summary.get("changedBySubtask")
                .and_then(Value::as_array).into_iter().flatten()
                .filter(|entry| entry.get("path").and_then(Value::as_str).is_some_and(|path| runtime_paths.contains(path)))
                .map(|entry| json!({"path":entry["path"],"sha256":entry["afterHash"],"kind":"playwright_mcp_snapshot"}))
                .collect::<Vec<_>>());
        }
    }
    if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
        task.changed_file_summary = Some(summary.clone());
        let paths = change_summary_paths(&summary);
        task.changed_files = dedupe_strings(
            task.changed_files
                .clone()
                .into_iter()
                .chain(paths)
                .collect(),
        );
    }
    run.latest_workspace_snapshot = Some(after);
    run.change_ledger.push(summary.clone());
    if run.git_policy == "managed" {
        run.git_ledger.push(json!({
            "taskId": task_id,
            "policy": "managed",
            "branch": git_command_text(&run.project_path, &["branch", "--show-current"]),
            "shortStat": summary.get("shortStat").and_then(Value::as_str).unwrap_or(""),
            "touchedFiles": summary.get("touchedFiles").cloned().unwrap_or_else(|| json!([])),
            "recordedAt": Utc::now(),
            "historyMutation": false,
        }));
    }
    summary
}

fn finish_task_attempt(
    run: &mut AgenticBoard,
    task_id: &str,
    attempt_id: &str,
    status: &str,
    finished_at: DateTime<Utc>,
    workspace_changes: Option<&Value>,
) {
    let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) else {
        return;
    };
    let Some(attempt_index) =
        task.hierarchy.attempts.iter().rposition(|attempt| {
            attempt.get("attemptId").and_then(Value::as_str) == Some(attempt_id)
        })
    else {
        return;
    };
    let started_at = task.hierarchy.attempts[attempt_index]
        .get("startedAt")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_utc);
    let transcript_start_index = task.hierarchy.attempts[attempt_index]
        .get("transcriptStartIndex")
        .and_then(Value::as_u64)
        .map(|index| index as usize)
        .unwrap_or(0)
        .min(task.transcript.len());
    let attempt_transcript = task
        .transcript
        .iter()
        .skip(transcript_start_index)
        .cloned()
        .collect::<Vec<_>>();
    let attempt_commands = task.commands_run.clone();
    // Task-level changed_files is cumulative history. Only the workspace
    // snapshot captured by this caller belongs to the finishing attempt.
    // Do not fall back to a previous task summary or model-reported paths.
    let attempt_scope = task.hierarchy.attempts[attempt_index]
        .get("scopeVersion")
        .and_then(Value::as_u64)
        .filter(|scope| *scope > 0);
    let workspace_changes = workspace_changes.filter(|summary| {
        summary.get("taskId").and_then(Value::as_str) == Some(task_id)
            && attempt_scope.is_none_or(|scope| {
                summary.get("scopeVersion").and_then(Value::as_u64) == Some(scope)
            })
    });
    let attempt_files = workspace_changes
        .map(change_summary_paths)
        .map(dedupe_strings)
        .unwrap_or_default();
    let attempt_reused_evidence = workspace_changes
        .and_then(|summary| summary.get("reusedEvidence"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let attempt_evidence = task.evidence.clone();
    let attempt_side_effect_evidence = task.hierarchy.side_effect_evidence.clone();
    let attempt_environment = task.hierarchy.manual_test_environment.clone();
    let attempt_summary = task.summary.clone();
    let attempt_error = task.error.clone();
    let attempt = &mut task.hierarchy.attempts[attempt_index];
    if let Some(object) = attempt.as_object_mut() {
        object.insert("status".to_string(), json!(status));
        object.insert("finishedAt".to_string(), json!(finished_at));
        object.insert(
            "durationMs".to_string(),
            started_at
                .map(|started| (finished_at - started).num_milliseconds().max(0))
                .map(|duration| json!(duration))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "transcript".to_string(),
            sanitize_kanban_value(&json!(attempt_transcript)),
        );
        object.insert("commands".to_string(), json!(attempt_commands));
        object.insert("filesChanged".to_string(), json!(attempt_files));
        object.insert("reusedEvidence".to_string(), json!(attempt_reused_evidence));
        object.insert("evidence".to_string(), json!(attempt_evidence));
        object.insert(
            "externalSideEffects".to_string(),
            json!(attempt_side_effect_evidence),
        );
        object.insert(
            "manualTestEnvironment".to_string(),
            attempt_environment.unwrap_or(Value::Null),
        );
        object.insert("summary".to_string(), json!(attempt_summary));
        object.insert("error".to_string(), json!(attempt_error));
        object.remove("transcriptStartIndex");
    }
}

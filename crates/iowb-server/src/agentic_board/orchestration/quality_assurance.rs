fn is_qa_task(task: &BoardTask) -> bool {
    task.qa_task || task.final_qa_task || task.id == FINAL_QA_TASK_ID
}

fn is_qa_task_id(run: &AgenticBoard, task_id: &str) -> bool {
    run.tasks
        .iter()
        .find(|task| task.id == task_id)
        .map(is_qa_task)
        .unwrap_or(false)
}

fn is_qa_verdict_retry_task_id(run: &AgenticBoard, task_id: &str) -> bool {
    run.tasks
        .iter()
        .find(|task| task.id == task_id)
        .map(|task| task.qa_verdict_retry_task)
        .unwrap_or(false)
}

fn is_missing_final_json_result(parsed: &Value) -> bool {
    if parsed.get("parsedJson").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    if parsed.get("status").and_then(Value::as_str) != Some("needs_followup") {
        return false;
    }
    let text = [
        parsed.get("summary").and_then(Value::as_str).unwrap_or(""),
        &normalize_string_list(parsed.get("remainingIssues")).join("\n"),
    ]
    .join("\n")
    .to_lowercase();
    text.contains("final json") || text.contains("valid json") || text.contains("required json")
}

fn qa_needs_followup(parsed: &Value) -> bool {
    parsed.get("status").and_then(Value::as_str) == Some("needs_followup")
        || parsed.get("qaPassed").and_then(Value::as_bool) == Some(false)
        || (parsed_status_done(Some(parsed))
            && !normalize_string_list(parsed.get("remainingIssues")).is_empty())
}

fn should_queue_qa_verdict_retry(
    run: &AgenticBoard,
    task_id: &str,
    parsed: &Value,
    change_summary: &Value,
) -> bool {
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id) else {
        return false;
    };
    is_qa_task(task)
        && !task.qa_verdict_retry_task
        && is_missing_final_json_result(parsed)
        && change_summary
            .get("touchedFileCount")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
        && !run.tasks.iter().any(|candidate| {
            candidate.qa_verdict_retry_task
                && candidate.source_qa_task_id.as_deref() == Some(task_id)
        })
}

fn queue_qa_verdict_retry(run: &mut AgenticBoard, task_id: &str, parsed: &Value) -> bool {
    let Some(source_index) = run.tasks.iter().position(|task| task.id == task_id) else {
        return false;
    };
    let source_task = run.tasks[source_index].clone();
    if let Some(task) = run.tasks.get_mut(source_index) {
        task.status = TASK_STATUS_DONE.to_string();
        task.qa_passed = Some(false);
        task.error = Some(normalize_string_list(parsed.get("remainingIssues")).join("; "));
        task.summary = parsed
            .get("summary")
            .and_then(Value::as_str)
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or("QA completed checks but missed the required final JSON.")
            .to_string();
        task.completed_at = Some(Utc::now());
    }
    let id = format!("qa-verdict-retry-{}", run.tasks.len() + 1);
    let details = [
        format!("Source QA task: {} - {}", source_task.id, source_task.title),
        parsed
            .get("summary")
            .and_then(Value::as_str)
            .map(|summary| format!("Previous QA summary: {summary}"))
            .unwrap_or_default(),
        "Review the existing QA transcript and current files only as needed. Return the required final JSON verdict. Do not edit files.".to_string(),
    ]
    .into_iter()
    .filter(|part| !part.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    let hierarchy = executable_system_subtask_hierarchy(
        system_sibling_parent_id(&source_task),
        vec![task_id.to_string()],
        source_task.hierarchy.scope_version,
        source_task.hierarchy.rank.saturating_add(1),
        source_task.hierarchy.planned_files.clone(),
    );
    let retry = BoardTask {
        id: id.clone(),
        title: format!("QA verdict retry: {}", source_task.title),
        status: TASK_STATUS_TODO.to_string(),
        summary: String::new(),
        details: details.clone(),
        description: details.clone(),
        prompt: details,
        error: None,
        acceptance_criteria: vec![
            "Return only the required task result JSON contract.".to_string(),
            "Do not edit files or run unrelated implementation work.".to_string(),
            "If QA found actionable defects, return needs_followup with exact findings."
                .to_string(),
        ],
        references: vec![format!("Source QA task: {task_id}")],
        priority: source_task.priority.clone(),
        depends_on: vec![task_id.to_string()],
        manual_task: false,
        prompt_task: false,
        task_origin: "system_qa_verdict_retry".to_string(),
        task_type: TASK_KIND_QA.to_string(),
        backlog_generation_task: false,
        qa_task: true,
        final_qa_task: source_task.final_qa_task,
        followup_task: false,
        qa_fix_task: false,
        qa_verdict_retry_task: true,
        task_level_qa: source_task.task_level_qa,
        agents_knowledge_task: false,
        internal_validation: source_task.internal_validation,
        qa_round: source_task.qa_round.saturating_add(1),
        source_task_id: source_task.source_task_id.clone(),
        source_qa_task_id: Some(task_id.to_string()),
        superseded_by: None,
        transcript: Vec::new(),
        transcript_updated_at: None,
        started_at: None,
        completed_at: None,
        qa_passed: None,
        attempt_count: 0,
        provider_session_id: None,
        commands_run: Vec::new(),
        changed_files: Vec::new(),
        changed_file_summary: None,
        evidence: Vec::new(),
        remaining_issues: Vec::new(),
        result: None,
        result_validation: None,
        deterministic_validation: None,
        rag_context_refs: Vec::new(),
        rag_prompt_context: String::new(),
        tdd_phase: if source_task.qa_test_commands.is_empty() {
            default_tdd_phase()
        } else {
            "fix_pending".to_string()
        },
        qa_test_paths: source_task.qa_test_paths.clone(),
        qa_test_commands: source_task.qa_test_commands.clone(),
        qa_baseline_validation: source_task.qa_baseline_validation.clone(),
        fix_attempts: source_task.fix_attempts.saturating_add(1),
        coverage_evidence: source_task.coverage_evidence.clone(),
        group_id: Some(task_group_id_for_source(&source_task)),
        hierarchy,
    };
    run.tasks.insert(source_index + 1, retry);
    run.append_log(format!(
        "QA JSON contract missing for {task_id}; queued compact verdict retry {id}"
    ));
    true
}

fn mark_qa_verdict_retry_blocked(run: &mut AgenticBoard, task_id: &str, parsed: &Value) {
    if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
        task.status = "blocked".to_string();
        task.qa_passed = Some(false);
        task.error = Some(
            normalize_string_list(parsed.get("remainingIssues"))
                .join("; ")
                .chars()
                .take(1200)
                .collect(),
        );
        task.summary = parsed
            .get("summary")
            .and_then(Value::as_str)
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or("QA verdict retry could not produce the required JSON contract.")
            .to_string();
        task.completed_at = Some(Utc::now());
    }
    run.append_log(format!(
        "QA verdict retry {task_id} missed final JSON; not queueing implementation work"
    ));
}

fn append_followup_task_if_needed(
    run: &mut AgenticBoard,
    source_task_id: &str,
    parsed: &Value,
) -> bool {
    if parsed.get("status").and_then(Value::as_str) != Some("needs_followup") {
        return false;
    }
    let Some(source_task) = run
        .tasks
        .iter()
        .find(|task| task.id == source_task_id)
        .cloned()
    else {
        return false;
    };
    if uses_hierarchical_orchestration(run)
        && !matches!(
            canonical_task_kind(&source_task),
            TASK_KIND_QA | TASK_KIND_MANUAL_TEST | TASK_KIND_REVIEW
        )
    {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == source_task_id) {
            task.status = TASK_STATUS_BLOCKED.to_string();
            task.error = Some(
                "Subtask reported incomplete work inside the approved scope; create or discuss a concrete fix subtask under this parent."
                    .to_string(),
            );
        }
        return false;
    }
    let group_id = source_task
        .group_id
        .clone()
        .unwrap_or_else(|| source_task.id.clone());
    let existing_followups = run
        .tasks
        .iter()
        .filter(|task| task.followup_task && task.group_id.as_deref() == Some(&group_id))
        .count();
    let max_followups = max_followups_per_group(run);
    if existing_followups >= max_followups {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == source_task_id) {
            task.status = "blocked".to_string();
            task.error = Some(format!(
                "Follow-up limit reached for {group_id} ({max_followups})."
            ));
        }
        run.append_log(format!(
            "Task follow-up limit reached for {source_task_id}; marked blocked"
        ));
        return false;
    }
    let max_fix_attempts = max_tdd_fix_attempts(run);
    if !uses_hierarchical_orchestration(run)
        && !source_task.qa_test_commands.is_empty()
        && source_task.fix_attempts >= max_fix_attempts
    {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == source_task_id) {
            task.status = "blocked".to_string();
            task.tdd_phase = "blocked".to_string();
            task.error = Some(format!(
                "TDD max fix attempts reached ({max_fix_attempts})."
            ));
        }
        run.append_log(format!(
            "TDD max fix attempts reached for {source_task_id}; no further fix task queued"
        ));
        return false;
    }
    let followup_index = existing_followups + 1;
    let qa_fix = is_qa_task(&source_task)
        || matches!(
            canonical_task_kind(&source_task),
            TASK_KIND_MANUAL_TEST | TASK_KIND_REVIEW
        );
    let suggested = parsed
        .get("suggestedBacklogTasks")
        .or_else(|| parsed.get("suggestedTasks"))
        .and_then(Value::as_array)
        .and_then(|items| items.first());
    let suggested_title = suggested
        .and_then(|item| item.get("title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty());
    let title = if qa_fix {
        suggested_title
            .map(str::to_string)
            .unwrap_or_else(|| format!("Fix validation findings for: {}", source_task.title))
    } else {
        format!("Continue follow-up: {}", source_task.title)
    };
    let issues = normalize_string_list(parsed.get("remainingIssues"))
        .into_iter()
        .chain(normalize_string_list(parsed.get("remainingGaps")))
        .collect::<Vec<_>>();
    let suggested_details = suggested
        .and_then(|item| item.get("details").or_else(|| item.get("description")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|details| !details.is_empty());
    let details = [
        format!("Source task: {} - {}", source_task.id, source_task.title),
        suggested_details
            .map(|details| format!("Requested fix scope: {details}"))
            .unwrap_or_default(),
        parsed
            .get("summary")
            .and_then(Value::as_str)
            .map(|summary| format!("Source summary: {summary}"))
            .unwrap_or_default(),
        if issues.is_empty() {
            "Remaining issue: continue the incomplete work from the source task.".to_string()
        } else {
            format!("Remaining issues:\n- {}", issues.join("\n- "))
        },
    ]
    .into_iter()
    .filter(|part| !part.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    let id = format!("task-followup-{}-{}", run.tasks.len() + 1, followup_index);
    let followup = BoardTask {
        id: id.clone(),
        title,
        status: TASK_STATUS_TODO.to_string(),
        summary: String::new(),
        details: details.clone(),
        description: details.clone(),
        prompt: details,
        error: None,
        acceptance_criteria: source_task.acceptance_criteria.clone(),
        references: [
            source_task.references.clone(),
            vec![format!("Source task: {source_task_id}")],
        ]
        .concat(),
        priority: suggested
            .and_then(|item| item.get("priority").and_then(Value::as_str))
            .map(|priority| normalize_priority(Some(priority)).to_string())
            .unwrap_or_else(|| {
                if qa_fix {
                    TASK_PRIORITY_P1.to_string()
                } else {
                    source_task.priority.clone()
                }
            }),
        depends_on: vec![source_task_id.to_string()],
        manual_task: false,
        prompt_task: false,
        task_origin: if qa_fix {
            "system_qa_fix".to_string()
        } else {
            "system_followup".to_string()
        },
        task_type: if qa_fix {
            TASK_KIND_FIX.to_string()
        } else {
            TASK_KIND_FOLLOWUP.to_string()
        },
        backlog_generation_task: false,
        qa_task: false,
        final_qa_task: false,
        followup_task: true,
        qa_fix_task: qa_fix,
        qa_verdict_retry_task: false,
        task_level_qa: false,
        agents_knowledge_task: false,
        internal_validation: false,
        qa_round: 0,
        source_task_id: Some(source_task_id.to_string()),
        source_qa_task_id: qa_fix.then(|| source_task_id.to_string()),
        superseded_by: None,
        transcript: Vec::new(),
        transcript_updated_at: None,
        started_at: None,
        completed_at: None,
        qa_passed: None,
        attempt_count: 0,
        provider_session_id: None,
        commands_run: Vec::new(),
        changed_files: Vec::new(),
        changed_file_summary: None,
        evidence: Vec::new(),
        remaining_issues: Vec::new(),
        result: None,
        result_validation: None,
        deterministic_validation: None,
        rag_context_refs: Vec::new(),
        rag_prompt_context: String::new(),
        tdd_phase: default_tdd_phase(),
        qa_test_paths: Vec::new(),
        qa_test_commands: Vec::new(),
        qa_baseline_validation: None,
        fix_attempts: 0,
        coverage_evidence: Vec::new(),
        group_id: Some(group_id.clone()),
        hierarchy: BoardTaskHierarchy::default(),
    };
    let mut followup = followup;
    if uses_hierarchical_orchestration(run) {
        followup.task_type = TASK_KIND_FIX.to_string();
        followup.hierarchy = BoardTaskHierarchy {
            level: TASK_LEVEL_SUBTASK.to_string(),
            parent_id: source_task.hierarchy.parent_id.clone(),
            blocked_by: vec![source_task_id.to_string()],
            executable: true,
            required: true,
            scope_version: source_task.hierarchy.scope_version,
            rank: source_task.hierarchy.rank.saturating_add(1),
            attempts: Vec::new(),
            planned_files: source_task.hierarchy.planned_files.clone(),
            side_effects: Vec::new(),
            side_effects_approved: false,
            side_effect_approval: None,
            side_effect_evidence: Vec::new(),
            manual_test_environment: None,
            research_accepted: false,
            research_acceptance: None,
            discussion: Vec::new(),
        };
        followup.depends_on = vec![source_task_id.to_string()];
        followup.group_id = Some(group_id.clone());
    }
    let insert_index = run
        .tasks
        .iter()
        .position(|task| task.id == source_task_id)
        .map(|index| index + 1)
        .unwrap_or(run.tasks.len());
    run.tasks.insert(insert_index, followup);
    if source_task.final_qa_task {
        let _ = append_final_qa_task(run, &format!("Rerun after {id}"));
    }
    run.append_log(format!("Task requires follow-up; queued {id}"));
    true
}

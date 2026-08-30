fn summarize_provider_output(stdout: &str, stderr: &str, exit_code: i32) -> String {
    let source = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    let mut summary = source
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string();
    if summary.len() > 500 {
        summary.truncate(497);
        summary.push_str("...");
    }
    if summary.is_empty() {
        summary = format!("Provider exited with code {exit_code}");
    }
    summary
}

fn build_promotion_review_prompt(run: &AgenticBoard, candidates: &[Value]) -> String {
    format!(
        r#"You are reviewing RAG promotion candidates for io-workbench.

Project: {project_name}
Board id: {board_id}

Candidates:
{candidates}

Rules:
- Approve only reusable, generalizable implementation, testing, validation, or architecture standards.
- Reject project secrets, credentials, personal data, one-off file paths, speculative claims, and brittle implementation details.
- Prefer approving fewer candidates when uncertain.
- Do not edit files.
- Return JSON only:
{{
  "status": "done" | "blocked",
  "summary": "short review summary",
  "approvedCandidateIds": ["candidate ids safe to promote"],
  "rejectedCandidateIds": ["candidate ids rejected"],
  "notes": ["brief reason for approvals or rejections"]
}}"#,
        project_name = run.project_name,
        board_id = run.id,
        candidates = serde_json::to_string_pretty(candidates).unwrap_or_default(),
    )
}

fn build_codebase_recon_prompt(run: &AgenticBoard, local_snapshot: &Value) -> String {
    format!(
        r#"You are performing read-only codebase reconnaissance before Kanban planning.

User request:
{prompt}

{board_profile_block}

{git_policy_block}

Local static snapshot:
{snapshot}

Return JSON only. No markdown fence.
Schema:
{{
  "summary": "short architecture summary",
  "architecture": ["important modules, runtime boundaries, data flow, framework facts"],
  "implementedCapabilities": ["requested capabilities that appear already implemented"],
  "missingCapabilities": ["requested capabilities that appear missing or partial"],
  "conventions": ["coding, testing, routing, styling, data, or migration conventions to follow"],
  "runCommands": ["commands for running the app locally"],
  "testCommands": ["commands for focused verification"],
  "relevantFiles": ["files/directories future tasks should inspect first"],
  "risks": ["important risks or external dependencies"]
}}

Rules:
- Inspect only. Do not edit files.
- Prefer concrete files, scripts, and conventions over generic advice.
- Treat summaries as navigation hints; task executions still inspect files before editing."#,
        prompt = active_board_prompt(run),
        board_profile_block = board_profile_block(run),
        git_policy_block = git_policy_block(run),
        snapshot = serde_json::to_string_pretty(local_snapshot).unwrap_or_default(),
    )
}

fn completed_task_summary(run: &AgenticBoard) -> String {
    run.tasks
        .iter()
        .filter(|task| task_is_done(task))
        .map(|task| {
            format!(
                "{}: {}\nEvidence: {}",
                task.id,
                task.summary,
                task.evidence.join("; ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn task_result_summary(run: &AgenticBoard) -> String {
    run.tasks
        .iter()
        .map(|task| {
            format!(
                "{} [{}] {}\nEvidence: {}\nRemaining: {}",
                task.id,
                task.status,
                task.summary,
                task.evidence.join("; "),
                task.remaining_issues.join("; ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn task_from_json(
    run: &AgenticBoard,
    item: Value,
    index: usize,
    status: &str,
) -> Option<BoardTask> {
    let title = item.get("title").and_then(Value::as_str)?.trim();
    if title.is_empty() {
        return None;
    }
    let details = item
        .get("details")
        .or_else(|| item.get("description"))
        .and_then(Value::as_str)
        .unwrap_or(title)
        .trim()
        .to_string();
    let acceptance_criteria = normalize_string_list(item.get("acceptanceCriteria"));
    let task_type = prompt_task_kind_from_value(&item, title, &details, &acceptance_criteria);
    let parent_id = item
        .get("parentId")
        .or_else(|| item.get("parent_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let requested_level = normalize_task_level(
        item.get("level").and_then(Value::as_str),
        if parent_id.is_some() {
            TASK_LEVEL_SUBTASK
        } else {
            TASK_LEVEL_STORY
        },
    );
    let level = parent_id
        .as_deref()
        .and_then(|parent_id| {
            run.tasks
                .iter()
                .find(|candidate| candidate.id == parent_id)
                .and_then(|parent| next_hierarchy_level(task_level(parent)))
        })
        .unwrap_or_else(|| match requested_level {
            TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK => TASK_LEVEL_STORY,
            level => level,
        });
    let source_task_id = item
        .get("sourceTaskId")
        .or_else(|| item.get("source_task_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let source_qa_task_id = item
        .get("sourceQaTaskId")
        .or_else(|| item.get("source_qa_task_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let depends_on =
        normalize_string_list(item.get("dependsOn").or_else(|| item.get("dependencies")));
    let blocked_by =
        normalize_string_list(item.get("blockedBy").or_else(|| item.get("blocked_by")))
            .into_iter()
            .chain(depends_on.iter().cloned())
            .collect::<Vec<_>>();
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("task-{}", index + 1));
    let priority = task_priority_for_parent(
        run,
        parent_id.as_deref(),
        item.get("priority").and_then(Value::as_str),
    );
    Some(BoardTask {
        id: id.clone(),
        title: title.to_string(),
        status: status.to_string(),
        summary: String::new(),
        details: details.clone(),
        description: details.clone(),
        prompt: details,
        error: None,
        acceptance_criteria,
        references: normalize_string_list(item.get("references")),
        priority,
        depends_on,
        manual_task: false,
        prompt_task: false,
        task_origin: "planned".to_string(),
        task_type: task_type.to_string(),
        backlog_generation_task: false,
        qa_task: false,
        final_qa_task: false,
        followup_task: false,
        qa_fix_task: false,
        qa_verdict_retry_task: false,
        task_level_qa: false,
        agents_knowledge_task: false,
        internal_validation: false,
        qa_round: 0,
        source_task_id,
        source_qa_task_id,
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
        group_id: Some(id),
        hierarchy: BoardTaskHierarchy {
            level: level.to_string(),
            parent_id,
            blocked_by,
            executable: item
                .get("executable")
                .and_then(Value::as_bool)
                .unwrap_or(level == TASK_LEVEL_SUBTASK)
                && level == TASK_LEVEL_SUBTASK,
            required: item
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            scope_version: item
                .get("scopeVersion")
                .and_then(Value::as_u64)
                .unwrap_or(1),
            rank: item.get("rank").and_then(Value::as_i64).unwrap_or(0),
            attempts: Vec::new(),
            planned_files: normalize_string_list(
                item.get("plannedFiles").or_else(|| item.get("files")),
            ),
            side_effects: normalize_string_list(item.get("sideEffects")),
            side_effects_approved: false,
            side_effect_approval: None,
            side_effect_evidence: Vec::new(),
            manual_test_environment: None,
            research_accepted: false,
            research_acceptance: None,
            discussion: Vec::new(),
        },
    })
}

fn parse_execution_result(stdout: &str) -> Option<Value> {
    parse_json_object(stdout)
        .or_else(|| {
            stdout
                .lines()
                .rev()
                .find_map(|line| parse_json_object(line.trim()))
        })
        .map(mark_parsed_json_result)
}

fn resolved_execution_summary(parsed: &Value, provider_summary: &str) -> String {
    parsed
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .unwrap_or_else(|| provider_summary.trim())
        .to_string()
}

fn missing_json_task_result(output: &str) -> Value {
    json!({
        "status": "needs_followup",
        "summary": "Provider completed without returning the required task result JSON.",
        "parsedJson": false,
        "changedFiles": [],
        "commandsRun": [],
        "qaResult": "blocked",
        "evidence": [limit_text(output, 1200)],
        "remainingIssues": ["The task result was not machine-readable. The next attempt must return the required JSON contract."],
        "remainingGaps": ["Missing strict task result JSON."],
        "suggestedBacklogTasks": [],
    })
}

fn mark_parsed_json_result(mut parsed: Value) -> Value {
    if let Some(object) = parsed.as_object_mut() {
        object.insert("parsedJson".to_string(), json!(true));
    }
    parsed
}

fn should_treat_provider_errors_as_followup(
    result: &ProviderTaskResult,
    parsed: &Value,
    change_summary: &Value,
) -> bool {
    result.exit_code == 0
        && change_summary
            .get("touchedFileCount")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        && parsed.get("parsedJson").and_then(Value::as_bool) != Some(true)
        && !result.errors.is_empty()
}

fn convert_missing_json_provider_error_to_followup(
    parsed: &Value,
    result: &ProviderTaskResult,
) -> Value {
    let mut remaining = vec![
        "Review the current workspace changes, finish any missing task work, and return the required final JSON contract."
            .to_string(),
    ];
    remaining.extend(
        result
            .errors
            .iter()
            .map(|error| format!("Provider reported: {}", limit_text(error, 300))),
    );
    remaining.extend(normalize_string_list(parsed.get("remainingIssues")));
    let mut next = parsed.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("status".to_string(), json!("needs_followup"));
        object.insert(
            "summary".to_string(),
            parsed
                .get("summary")
                .and_then(Value::as_str)
                .filter(|summary| !summary.trim().is_empty())
                .map(|summary| json!(summary))
                .unwrap_or_else(|| {
                    json!("Task made workspace changes but did not return the required final JSON.")
                }),
        );
        object.insert("remainingIssues".to_string(), json!(remaining));
    }
    next
}

fn is_recoverable_self_reported_blocker(parsed: &Value) -> bool {
    let status = parsed.get("status").and_then(Value::as_str).unwrap_or("");
    if !matches!(status, "blocked" | "needs_followup") {
        return false;
    }
    let text = execution_result_text(parsed).to_lowercase();
    recoverable_blocker_match(&text) || tool_environment_blocker_match(&text)
}

fn is_tool_environment_self_reported_blocker(parsed: &Value) -> bool {
    let status = parsed.get("status").and_then(Value::as_str).unwrap_or("");
    if !matches!(status, "blocked" | "needs_followup") {
        return false;
    }
    tool_environment_blocker_match(&execution_result_text(parsed).to_lowercase())
}

fn execution_result_text(parsed: &Value) -> String {
    let mut parts = Vec::new();
    for key in ["status", "summary", "qaResult"] {
        if let Some(text) = parsed.get(key).and_then(Value::as_str) {
            parts.push(text.to_string());
        }
    }
    for key in ["evidence", "remainingIssues", "remainingGaps"] {
        parts.extend(normalize_string_list(parsed.get(key)));
    }
    parts.join("\n")
}

fn recoverable_blocker_match(text: &str) -> bool {
    (text.contains("json") && (text.contains("quote") || text.contains("valid json")))
        || text.contains("quote mangl")
        || text.contains("unmatched \"")
        || text.contains("unmatched '")
        || text.contains("exec_command")
        || text.contains("apply_patch")
        || text.contains("cannot write")
        || text.contains("could not write")
        || text.contains("failed to write")
        || text.contains("no cargo.toml")
        || (text.contains("workspace") && (text.contains("not built") || text.contains("missing")))
        || (text.contains("no crate") && text.contains("compile"))
        || (text.contains("no ") && text.contains("source files"))
        || (text.contains("dependencies") && text.contains("unverified"))
        || (text.contains("cannot") && text.contains("honestly") && text.contains("verif"))
        || (text.contains("postgres") && text.contains("not reachable"))
        || (text.contains("no postgres") && text.contains("reachable"))
        || text.contains("sqlx migrate")
}

fn tool_environment_blocker_match(text: &str) -> bool {
    text.contains("tool result missing due to internal error")
        || (text.contains("tool environment") && text.contains("fail"))
        || (text.contains("every") && text.contains("tool") && text.contains("internal error"))
        || (text.contains("every") && text.contains("command") && text.contains("internal error"))
        || (text.contains("every") && text.contains("shell") && text.contains("internal error"))
        || (text.contains("every") && text.contains("cat") && text.contains("internal error"))
        || text.contains("no inspection, edits, or verification ran")
        || text.contains("no implementation or qa evidence was added")
}

fn provider_events_have_tool_evidence(events: &[Value]) -> bool {
    events.iter().any(|event| {
        matches!(
            event.get("kind").and_then(Value::as_str),
            Some("tool_use" | "tool_result" | "tool")
        ) || event.get("toolName").is_some()
            || event.get("toolInput").is_some()
            || event.get("toolResult").is_some()
    })
}

fn reset_provider_session(run: &mut AgenticBoard, reason: &str) {
    let had_session = run.actual_session_id.is_some() || run.current_provider_session_id.is_some();
    run.actual_session_id = None;
    run.current_provider_session_id = None;
    run.provider_call_started_at = None;
    run.provider_call_label = None;
    if had_session {
        run.append_log(format!("Started a fresh provider session: {reason}"));
    }
}

fn filter_fatal_provider_errors(errors: &[String], exit_code: i32) -> Vec<String> {
    let normalized = errors
        .iter()
        .map(|error| error.trim())
        .filter(|error| !error.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if exit_code != 0 {
        return normalized;
    }
    normalized
        .into_iter()
        .filter(|error| !is_non_fatal_provider_error(error))
        .collect()
}

fn is_non_fatal_provider_error(message: &str) -> bool {
    let text = message.to_lowercase();
    (text.contains("long threads") && text.contains("multiple compactions"))
        || (text.contains("start a new thread") && text.contains("threads small"))
        || (text.contains("model metadata for")
            && text.contains("not found")
            && text.contains("fallback metadata"))
}

async fn repair_task_result_if_needed(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    task_id: &str,
    task_index: usize,
    assistant_output: &str,
    parsed: Value,
    change_summary: &Value,
) -> Value {
    let Ok(stored) = load_user_board(state, user_id, board_id) else {
        return parsed;
    };
    let Some(task) = stored.board.tasks.iter().find(|task| task.id == task_id) else {
        return parsed;
    };
    let issues = strict_result_schema_issues(&stored.board, task, &parsed, change_summary);
    if issues.is_empty() {
        return parsed;
    }
    let prompt = build_task_result_repair_prompt(
        &stored.board,
        task,
        task_index,
        &parsed,
        assistant_output,
        change_summary,
        &issues,
    );
    let repaired = execute_internal_prompt(
        state,
        user_id,
        board_id,
        &format!("result schema repair for {task_id}"),
        &prompt,
    )
    .await
    .ok()
    .and_then(|text| parse_json_object(&text));

    let mut stored = match load_user_board(state, user_id, board_id) {
        Ok(stored) => stored,
        Err(_) => return repaired.unwrap_or(parsed),
    };
    if let Some(index) = stored
        .board
        .tasks
        .iter()
        .position(|task| task.id == task_id)
    {
        stored.board.tasks[index].result_validation = Some(json!({
            "schemaIssues": issues,
            "repairAttemptedAt": Utc::now(),
            "repaired": repaired.is_some(),
        }));
        stored.board.append_log(format!(
            "Result schema repair {} for {task_id}",
            if repaired.is_some() {
                "succeeded"
            } else {
                "failed"
            }
        ));
        stored.board.touch();
        let _ = save_board(state, &stored.board);
    }
    repaired.unwrap_or(parsed)
}

fn should_repair_task_result(
    run: &AgenticBoard,
    task_id: &str,
    parsed: &Value,
    change_summary: &Value,
) -> bool {
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id) else {
        return false;
    };
    !strict_result_schema_issues(run, task, parsed, change_summary).is_empty()
}

fn strict_result_schema_issues(
    _run: &AgenticBoard,
    _task: &BoardTask,
    parsed: &Value,
    change_summary: &Value,
) -> Vec<String> {
    let mut issues = Vec::new();
    if !parsed.is_object() {
        issues.push("Result is not a JSON object.".to_string());
        return issues;
    }
    let status = parsed.get("status").and_then(Value::as_str).unwrap_or("");
    if !matches!(
        status,
        "done" | "blocked" | "needs_followup" | "completed" | "success"
    ) {
        issues.push("Missing or invalid status.".to_string());
    }
    if parsed
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        issues.push("Missing summary.".to_string());
    }
    if !matches!(
        parsed.get("qaResult").and_then(Value::as_str),
        Some("pass" | "fail" | "blocked" | "not_run")
    ) {
        issues.push("Missing or invalid qaResult.".to_string());
    }
    if parsed_status_done(Some(parsed)) {
        let changed_files = normalize_string_list(parsed.get("changedFiles"));
        let commands = normalize_string_list(parsed.get("commandsRun"));
        let evidence = normalize_string_list(parsed.get("evidence"));
        let attributable_count = change_summary_attributable_file_count(change_summary);
        let touched_count = change_summary
            .get("touchedFileCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let file_evidence = if uses_hierarchical_orchestration(_run) {
            attributable_count > 0
        } else {
            touched_count > 0 || !changed_files.is_empty()
        };
        if !file_evidence && commands.is_empty() && evidence.is_empty() {
            issues.push("Done result lacks changed files, commands, and evidence.".to_string());
        }
    }
    issues
}

fn build_task_result_repair_prompt(
    run: &AgenticBoard,
    task: &BoardTask,
    index: usize,
    parsed: &Value,
    assistant_output: &str,
    change_summary: &Value,
    issues: &[String],
) -> String {
    let scope = task_scope_block(run, task);
    let result_schema = {
        r#"{
  "status": "done" | "blocked" | "needs_followup",
  "summary": "short summary",
  "changedFiles": ["files changed or inspected as already correct"],
  "commandsRun": ["commands/checks actually shown in previous output"],
  "qaResult": "pass" | "fail" | "blocked" | "not_run",
  "evidence": ["specific evidence from previous output or workspace delta"],
  "remainingIssues": [],
  "remainingGaps": [],
  "suggestedBacklogTasks": []
}"#
    };
    format!(
        r#"Repair the previous agentic Kanban task result into the required JSON contract.

This is a reporting repair only.
- Do not edit files.
- Do not rerun implementation.
- Do not claim verification that is not present in the previous output, task transcript, ticket evidence, or workspace delta.
- If the previous output does not contain enough evidence to honestly mark the task done, return status "needs_followup".
- Return JSON only. No markdown fence.

User request:
{request}

Task {number} of {total}: {title}
Details:
{details}

Ticket scope:
{scope}

Schema/evidence issues:
{issues}

Workspace delta:
{delta}

Previous parsed result:
{parsed}

Previous assistant output:
{output}

Required schema:
{result_schema}"#,
        request = active_board_prompt(run),
        number = index + 1,
        total = run.tasks.len(),
        title = task.title,
        details = task.details,
        scope = scope,
        result_schema = result_schema,
        issues = issues
            .iter()
            .map(|issue| format!("- {issue}"))
            .collect::<Vec<_>>()
            .join("\n"),
        delta = serde_json::to_string_pretty(change_summary).unwrap_or_default(),
        parsed = serde_json::to_string_pretty(parsed).unwrap_or_default(),
        output = limit_text(assistant_output, 10_000),
    )
}

fn reusable_session_id(run: &AgenticBoard) -> Option<String> {
    run.actual_session_id
        .clone()
        .or_else(|| run.session_id.clone())
        .filter(|value| !value.trim().is_empty())
}

fn reusable_session_id_for_provider(run: &AgenticBoard, provider: &str) -> Option<String> {
    if provider == run.provider {
        reusable_session_id(run)
    } else {
        None
    }
}

fn board_task_id_for_label(run: &AgenticBoard, label: &str) -> Option<String> {
    let label = label.trim();
    run.tasks
        .iter()
        .find(|task| {
            label == task.id
                || label.ends_with(&format!(" for {}", task.id))
                || label.contains(&format!(" {} ", task.id))
        })
        .map(|task| task.id.clone())
        .or_else(|| run.current_task_id.clone())
}

fn should_resume_provider_session(run: &AgenticBoard) -> bool {
    matches!(
        normalize_session_policy(Some(&run.session_policy)).as_str(),
        "continuous"
    ) && run.provider == "claude"
}

fn uses_hierarchical_orchestration(run: &AgenticBoard) -> bool {
    run.orchestration_version >= 3
}

fn parse_json_object(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if value.is_object() {
            return Some(value);
        }
    }
    if let Some(start) = trimmed.find("```") {
        let rest = &trimmed[start + 3..];
        let rest = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("JSON"))
            .unwrap_or(rest);
        if let Some(end) = rest.find("```") {
            if let Some(value) = parse_json_object(&rest[..end]) {
                return Some(value);
            }
        }
    }
    let start = trimmed.find('{')?;
    let end = find_matching_json_brace(trimmed, start)?;
    serde_json::from_str::<Value>(&trimmed[start..=end])
        .ok()
        .filter(Value::is_object)
}

fn find_matching_json_brace(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn parsed_status_done(parsed: Option<&Value>) -> bool {
    parsed
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        .map(|status| matches!(status, "done" | "completed" | "success"))
        .unwrap_or(true)
}

fn completion_evidence_gate_failed(parsed: &Value) -> bool {
    parsed
        .get("evidenceGate")
        .and_then(|value| value.get("passed"))
        .and_then(Value::as_bool)
        == Some(false)
        || parsed
            .get("completionEvidenceGateFailed")
            .and_then(Value::as_bool)
            == Some(true)
}

fn apply_deterministic_validation_result(mut parsed: Value, validation: &Value) -> Value {
    let mut evidence = normalize_string_list(parsed.get("evidence"));
    evidence.push(format_validation_check(validation));
    let mut commands = normalize_string_list(parsed.get("commandsRun"));
    if let Some(items) = validation.get("commands").and_then(Value::as_array) {
        for item in items {
            if let Some(command) = item.get("command").and_then(Value::as_str) {
                commands.push(command.to_string());
            }
        }
    }
    if let Some(object) = parsed.as_object_mut() {
        object.insert("evidence".to_string(), json!(dedupe_strings(evidence)));
        object.insert("commandsRun".to_string(), json!(dedupe_strings(commands)));
        if validation.get("passed").and_then(Value::as_bool) == Some(false) {
            let mut issues = object
                .get("remainingIssues")
                .map(|value| normalize_string_list(Some(value)))
                .unwrap_or_default();
            for command in validation
                .get("commands")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if command.get("exitCode").and_then(Value::as_i64).unwrap_or(0) != 0 {
                    issues.push(format!(
                        "Deterministic validation failed: {} exited {}{}",
                        command
                            .get("command")
                            .and_then(Value::as_str)
                            .unwrap_or("command"),
                        command.get("exitCode").and_then(Value::as_i64).unwrap_or(1),
                        command
                            .get("output")
                            .and_then(Value::as_str)
                            .filter(|output| !output.trim().is_empty())
                            .map(|output| format!(": {}", limit_text(output, 700)))
                            .unwrap_or_default()
                    ));
                }
            }
            object.insert("status".to_string(), json!("needs_followup"));
            object.insert("qaResult".to_string(), json!("fail"));
            object.insert("remainingIssues".to_string(), json!(dedupe_strings(issues)));
        }
    }
    parsed
}

fn change_summary_touched_paths(summary: &Value) -> Vec<String> {
    summary
        .get("touchedFiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| match entry {
            Value::String(path) => Some(path.trim().to_string()),
            Value::Object(object) => object
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .map(str::to_string),
            _ => None,
        })
        .filter(|path| !path.is_empty())
        .collect()
}

fn is_test_or_fixture_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let components = normalized
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    let file_name = components.last().copied().unwrap_or_default();
    components.iter().any(|component| {
        matches!(
            *component,
            "test"
                | "tests"
                | "testing"
                | "fixtures"
                | "testfixtures"
                | "androidtest"
                | "commontest"
                | "jvmtetest"
                | "__tests__"
        ) || component.ends_with("test")
    }) || file_name.starts_with("test_")
        || file_name.ends_with("_test.rs")
        || file_name.ends_with("_test.kt")
        || file_name.ends_with("_test.kts")
        || file_name.contains(".test.")
        || file_name.contains(".spec.")
        || file_name.ends_with(".snap")
}

fn task_file_edit_policy_violations(task: &BoardTask, change_summary: &Value) -> Vec<String> {
    let touched = change_summary_touched_paths(change_summary);
    match canonical_task_kind(task) {
        TASK_KIND_QA
        | TASK_KIND_MANUAL_TEST
        | TASK_KIND_REVIEW
        | TASK_KIND_RESEARCH
        | TASK_KIND_DESIGN => touched,
        TASK_KIND_TEST_IMPLEMENTATION => touched
            .into_iter()
            .filter(|path| !is_test_or_fixture_path(path))
            .collect(),
        _ => Vec::new(),
    }
}

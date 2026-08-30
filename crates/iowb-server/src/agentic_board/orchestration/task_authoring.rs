fn is_user_authored_task(task: &BoardTask) -> bool {
    task.manual_task
        || task.prompt_task
        || matches!(
            task.task_origin.as_str(),
            "user_manual"
                | "user_prompt_generated"
                | "ai_suggested_backlog"
                | "manual"
                | "prompt_breakdown"
        )
}

fn unique_task_id(run: &AgenticBoard, base: &str) -> String {
    let existing = run
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<BTreeSet<_>>();
    if !existing.contains(base) {
        return base.to_string();
    }
    for index in 2..1000 {
        let candidate = format!("{base}-{index}");
        if !existing.contains(candidate.as_str()) {
            return candidate;
        }
    }
    format!("{base}-{}", Uuid::new_v4())
}

fn has_runnable_tasks(run: &AgenticBoard) -> bool {
    run.tasks
        .iter()
        .any(|task| task_is_runnable_in_board(run, task))
}

fn has_only_seed_prompt_task(run: &AgenticBoard) -> bool {
    run.tasks.len() == 1
        && run.tasks[0].id == "task-1"
        && run.tasks[0].prompt.trim() == run.source_prompt.trim()
        && (task_is_runnable_in_board(run, &run.tasks[0])
            || run.tasks[0].status == TASK_STATUS_BACKLOG)
}

fn executable_system_subtask_hierarchy(
    parent_id: Option<String>,
    blocked_by: Vec<String>,
    scope_version: u64,
    rank: i64,
    planned_files: Vec<String>,
) -> BoardTaskHierarchy {
    BoardTaskHierarchy {
        level: TASK_LEVEL_SUBTASK.to_string(),
        parent_id,
        blocked_by: dedupe_strings(blocked_by),
        executable: true,
        required: true,
        scope_version: scope_version.max(1),
        rank,
        attempts: Vec::new(),
        planned_files,
        side_effects: Vec::new(),
        side_effects_approved: false,
        side_effect_approval: None,
        side_effect_evidence: Vec::new(),
        manual_test_environment: None,
        research_accepted: false,
        research_acceptance: None,
        discussion: Vec::new(),
    }
}

fn system_sibling_parent_id(source_task: &BoardTask) -> Option<String> {
    (task_level(source_task) == TASK_LEVEL_SUBTASK)
        .then(|| source_task.hierarchy.parent_id.clone())
        .flatten()
}

fn append_final_qa_task(run: &mut AgenticBoard, reason: &str) -> bool {
    if run
        .tasks
        .iter()
        .any(|task| task.final_qa_task && task_is_runnable(task))
    {
        return false;
    }
    if run
        .tasks
        .iter()
        .any(|task| task.final_qa_task && task_is_done(task) && task.qa_passed == Some(true))
    {
        return false;
    }
    let round = run.tasks.iter().filter(|task| task.final_qa_task).count() as u32 + 1;
    let id = if round == 1 {
        FINAL_QA_TASK_ID.to_string()
    } else {
        format!("{FINAL_QA_TASK_ID}-{round}")
    };
    run.tasks.push(BoardTask {
        id,
        title: "Independent final validation".to_string(),
        status: TASK_STATUS_TODO.to_string(),
        summary: String::new(),
        details: format!(
            "Validate the ticket scope against current files, deterministic command evidence, and completed task results. Reason: {reason}"
        ),
        description: "Independent final validation".to_string(),
        prompt: "Run final QA validation and return the required JSON verdict.".to_string(),
        error: None,
        acceptance_criteria: vec![
            "Independently validate the ticket description and acceptance criteria against current files and deterministic command evidence.".to_string(),
            "Inspect implementation directly; do not trust feature summaries as proof.".to_string(),
            "Return done only when the ticket scope has concrete evidence and deterministic checks pass.".to_string(),
            "Do not edit files during this validation task and do not modify git history.".to_string(),
        ],
        references: vec![
            "Original user prompt".to_string(),
            "Changed files and local verification output".to_string(),
        ],
        priority: TASK_PRIORITY_P3.to_string(),
        depends_on: Vec::new(),
        manual_task: false,
        prompt_task: false,
        task_origin: "system_final_qa".to_string(),
        task_type: TASK_KIND_QA.to_string(),
        backlog_generation_task: false,
        qa_task: true,
        final_qa_task: true,
        followup_task: false,
        qa_fix_task: false,
        qa_verdict_retry_task: false,
        task_level_qa: false,
        agents_knowledge_task: false,
        internal_validation: true,
        qa_round: round,
        source_task_id: None,
        source_qa_task_id: None,
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
        tdd_phase: "final".to_string(),
        qa_test_paths: Vec::new(),
        qa_test_commands: Vec::new(),
        qa_baseline_validation: None,
        fix_attempts: 0,
        coverage_evidence: Vec::new(),
        group_id: Some(FINAL_QA_TASK_ID.to_string()),
        hierarchy: executable_system_subtask_hierarchy(None, Vec::new(), 1, 0, Vec::new()),
    });
    true
}

fn append_promotion_review_task(run: &mut AgenticBoard, reason: &str) -> bool {
    if run.promotion_candidates.is_empty() {
        return false;
    }
    if run.tasks.iter().any(|task| is_promotion_review_task(task)) {
        return false;
    }
    run.tasks.push(BoardTask {
        id: PROMOTION_REVIEW_TASK_ID.to_string(),
        title: "Review RAG promotion candidates".to_string(),
        status: TASK_STATUS_TODO.to_string(),
        summary: String::new(),
        details: format!(
            "Review validated project-specific RAG memories and approve only reusable, safe global standards. Reason: {reason}"
        ),
        description: "Review RAG promotion candidates".to_string(),
        prompt: "Review promotion candidates and return approvedCandidateIds JSON.".to_string(),
        error: None,
        acceptance_criteria: vec![
            "Reject unsafe, overly project-specific, secret-bearing, or speculative patterns."
                .to_string(),
            "Approve only reusable implementation, testing, or validation standards.".to_string(),
            "Do not edit files or modify git history.".to_string(),
        ],
        references: vec!["RAG project-specific promotion candidates".to_string()],
        priority: TASK_PRIORITY_P3.to_string(),
        depends_on: Vec::new(),
        manual_task: false,
        prompt_task: false,
        task_origin: "system_promotion".to_string(),
        task_type: TASK_KIND_REVIEW.to_string(),
        backlog_generation_task: false,
        qa_task: false,
        final_qa_task: false,
        followup_task: false,
        qa_fix_task: false,
        qa_verdict_retry_task: false,
        task_level_qa: false,
        agents_knowledge_task: false,
        internal_validation: true,
        qa_round: 0,
        source_task_id: None,
        source_qa_task_id: None,
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
        tdd_phase: "promotion_review".to_string(),
        qa_test_paths: Vec::new(),
        qa_test_commands: Vec::new(),
        qa_baseline_validation: None,
        fix_attempts: 0,
        coverage_evidence: Vec::new(),
        group_id: Some(PROMOTION_REVIEW_TASK_ID.to_string()),
        hierarchy: executable_system_subtask_hierarchy(None, Vec::new(), 1, 0, Vec::new()),
    });
    true
}

fn is_promotion_review_task(task: &BoardTask) -> bool {
    matches!(task.task_type.as_str(), "promotion" | TASK_KIND_REVIEW)
        || task.id == PROMOTION_REVIEW_TASK_ID
}

fn append_agents_knowledge_task(
    run: &mut AgenticBoard,
    reason: &str,
    source_task: Option<&BoardTask>,
) -> bool {
    if run.agents_knowledge_updated {
        return false;
    }
    let source_task_id = source_task.map(|task| task.id.as_str()).unwrap_or("");
    if run.tasks.iter().any(|task| {
        (task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID)
            && (source_task_id.is_empty() || task.source_task_id.as_deref() == Some(source_task_id))
    }) {
        return false;
    }
    if source_task_id.is_empty() {
        let has_completed_implementation = run
            .tasks
            .iter()
            .any(|task| task_is_done(task) && !is_qa_task(task) && !task.agents_knowledge_task);
        if !has_completed_implementation {
            return false;
        }
    }
    let task = create_agents_knowledge_task(run, reason, source_task);
    if let Some(source_task) = source_task {
        let insert_index = run
            .tasks
            .iter()
            .position(|task| task.id == source_task.id)
            .map(|index| index + 1)
            .unwrap_or(run.tasks.len());
        run.tasks.insert(insert_index, task);
    } else {
        run.tasks.push(task);
    }
    true
}

fn has_task_qa_for_source(run: &AgenticBoard, source_task_id: &str) -> bool {
    run.tasks
        .iter()
        .any(|task| task.task_level_qa && task.source_task_id.as_deref() == Some(source_task_id))
}

fn append_task_qa_task(run: &mut AgenticBoard, source_task: &BoardTask, reason: &str) -> bool {
    let task = create_task_qa_task(run, source_task, reason);
    let insert_index = run
        .tasks
        .iter()
        .position(|task| task.id == source_task.id)
        .map(|index| index + 1)
        .unwrap_or(run.tasks.len());
    run.tasks.insert(insert_index, task);
    true
}

fn create_task_qa_task(run: &AgenticBoard, source_task: &BoardTask, reason: &str) -> BoardTask {
    let title_seed = if source_task.title.trim().is_empty() {
        limit_text(&active_board_prompt(run), 180)
    } else {
        source_task.title.clone()
    };
    let details = [
        format!("Validate source task: {} - {}", source_task.id, source_task.title),
        "Inspect the current implementation before marking the task validated.".to_string(),
        "Cover the relevant happy path, failure path, and corner cases for the source task.".to_string(),
        "If locally actionable issues are found, return needs_followup with exact findings so the board can queue a fix.".to_string(),
        "Do not edit files during this validation task and do not modify git history.".to_string(),
        if reason.trim().is_empty() {
            String::new()
        } else {
            format!("Task QA reason: {reason}")
        },
    ]
    .into_iter()
    .filter(|item| !item.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n");
    let hierarchy = executable_system_subtask_hierarchy(
        system_sibling_parent_id(source_task),
        vec![source_task.id.clone()],
        source_task.hierarchy.scope_version,
        source_task.hierarchy.rank.saturating_add(1),
        source_task.hierarchy.planned_files.clone(),
    );
    BoardTask {
        id: unique_task_id(run, "task-qa"),
        title: format!("QA validate {}", limit_text(&title_seed, 120).replace('\n', " ")),
        status: TASK_STATUS_TODO.to_string(),
        summary: String::new(),
        details: details.clone(),
        description: details.clone(),
        prompt: details,
        error: None,
        acceptance_criteria: vec![
            "Validate the source task against its acceptance criteria, source references, evidence, and changed files.".to_string(),
            "Return done only when the source task is validated.".to_string(),
            "Return needs_followup with exact findings when defects are found.".to_string(),
        ],
        references: vec![
            format!("Source task: {}", source_task.id),
            format!("Source task title: {}", source_task.title),
            if source_task.summary.trim().is_empty() {
                String::new()
            } else {
                format!("Source task summary: {}", source_task.summary)
            },
        ]
        .into_iter()
        .filter(|item| !item.trim().is_empty())
        .collect(),
        priority: source_task.priority.clone(),
        depends_on: vec![source_task.id.clone()],
        manual_task: false,
        prompt_task: false,
        task_origin: "system_qa".to_string(),
        task_type: TASK_KIND_QA.to_string(),
        backlog_generation_task: false,
        qa_task: true,
        final_qa_task: false,
        followup_task: false,
        qa_fix_task: false,
        qa_verdict_retry_task: false,
        task_level_qa: true,
        agents_knowledge_task: false,
        internal_validation: false,
        qa_round: 0,
        source_task_id: Some(source_task.id.clone()),
        source_qa_task_id: None,
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
        tdd_phase: "final".to_string(),
        qa_test_paths: Vec::new(),
        qa_test_commands: Vec::new(),
        qa_baseline_validation: None,
        fix_attempts: 0,
        coverage_evidence: Vec::new(),
        group_id: Some(task_group_id_for_source(source_task)),
        hierarchy,
    }
}

fn task_needs_immediate_ai_qa(run: &AgenticBoard, task: &BoardTask, parsed: &Value) -> bool {
    if is_qa_task(task) || task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID {
        return false;
    }
    match run
        .qa_policy
        .get("taskQaMode")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("high_risk")
    {
        "off" => return false,
        "all" => return true,
        _ => {}
    }
    if matches!(task.priority.as_str(), TASK_PRIORITY_P0 | TASK_PRIORITY_P1)
        || task.qa_fix_task
        || task.followup_task
    {
        return true;
    }
    let text = task_risk_text(task, parsed).to_lowercase();
    [
        "auth",
        "login",
        "oauth",
        "permission",
        "security",
        "payment",
        "billing",
        "checkout",
        "database",
        "db",
        "migration",
        "schema",
        "sql",
        "api",
        "route",
        "endpoint",
        "server",
        "provider",
        "model",
        "token",
        "quota",
        "websocket",
        "critical",
        "data loss",
        "destructive",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn has_agents_knowledge_task_for_source(run: &AgenticBoard, source_task_id: &str) -> bool {
    run.tasks.iter().any(|task| {
        (task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID)
            && task.source_task_id.as_deref() == Some(source_task_id)
    })
}

fn task_needs_agents_knowledge_update(task: &BoardTask, parsed: &Value) -> bool {
    if is_qa_task(task) || task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID {
        return false;
    }
    let text = task_risk_text(task, parsed).to_lowercase();
    [
        "agents.md",
        "architecture",
        "convention",
        "command",
        "script",
        "setup",
        "build",
        "test",
        "lint",
        "database",
        "migration",
        "schema",
        "route",
        "api",
        "provider",
        "model",
        "env",
        "config",
        "docker",
        "playwright",
        "gotcha",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn task_risk_text(task: &BoardTask, parsed: &Value) -> String {
    [
        task.title.as_str(),
        task.summary.as_str(),
        task.details.as_str(),
        &task.acceptance_criteria.join("\n"),
        &task.references.join("\n"),
        parsed.get("summary").and_then(Value::as_str).unwrap_or(""),
        &normalize_string_list(parsed.get("evidence")).join("\n"),
        &normalize_string_list(parsed.get("remainingIssues")).join("\n"),
        &change_summary_paths(task.changed_file_summary.as_ref().unwrap_or(&Value::Null))
            .join("\n"),
    ]
    .join("\n")
}

fn create_agents_knowledge_task(
    run: &AgenticBoard,
    reason: &str,
    source_task: Option<&BoardTask>,
) -> BoardTask {
    let source_task_id = source_task.map(|task| task.id.clone());
    let source_group_id = source_task.map(task_group_id_for_source);
    let source_title = source_task
        .map(|task| task.title.clone())
        .unwrap_or_default();
    let id = if source_task_id.is_some() {
        unique_task_id(run, AGENTS_KNOWLEDGE_TASK_ID)
    } else {
        AGENTS_KNOWLEDGE_TASK_ID.to_string()
    };
    let title = if source_task_id.is_some() {
        if source_title.trim().is_empty() {
            "Update AGENTS.md with durable knowledge".to_string()
        } else {
            format!("Update AGENTS.md with durable knowledge after: {source_title}")
        }
    } else {
        "Update AGENTS.md with stable project knowledge from this agentic board".to_string()
    };
    let details = [
        "Read the applicable AGENTS.md file if it exists, or create a root AGENTS.md only when durable project guidance is available.".to_string(),
        "Record stable commands, architecture conventions, test workflows, or workflow rules that future coding agents should know.".to_string(),
        "Verify concrete project claims against the current filesystem, config, routes, migrations, command output, or QA evidence before writing them.".to_string(),
        "Do not add a task ledger, timestamps, transient run status, raw QA logs, or one-off implementation details.".to_string(),
        "Leave AGENTS.md unchanged if there is no stable project knowledge worth preserving.".to_string(),
        if reason.trim().is_empty() { String::new() } else { format!("Reason: {reason}") },
    ]
    .into_iter()
    .filter(|item| !item.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n");
    let hierarchy = executable_system_subtask_hierarchy(
        source_task.and_then(system_sibling_parent_id),
        source_task
            .map(|task| vec![task.id.clone()])
            .unwrap_or_default(),
        source_task
            .map(|task| task.hierarchy.scope_version)
            .unwrap_or(1),
        source_task
            .map(|task| task.hierarchy.rank.saturating_add(1))
            .unwrap_or(0),
        source_task
            .map(|task| task.hierarchy.planned_files.clone())
            .unwrap_or_default(),
    );
    BoardTask {
        id,
        title,
        status: TASK_STATUS_TODO.to_string(),
        summary: String::new(),
        details: details.clone(),
        description: details.clone(),
        prompt: details,
        error: None,
        acceptance_criteria: vec![
            "Read applicable AGENTS.md guidance before editing.".to_string(),
            "Preserve only stable project knowledge worth reusing in future coding tasks."
                .to_string(),
            "Return the required task result JSON contract.".to_string(),
        ],
        references: vec![
            "Applicable AGENTS.md files".to_string(),
            "Codebase recon summary".to_string(),
            "Completed implementation summaries".to_string(),
            source_task_id
                .as_ref()
                .map(|id| format!("Source task: {id}"))
                .unwrap_or_default(),
        ]
        .into_iter()
        .filter(|item| !item.trim().is_empty())
        .collect(),
        priority: source_task
            .map(|task| normalize_priority(Some(&task.priority)).to_string())
            .unwrap_or_else(|| TASK_PRIORITY_P3.to_string()),
        depends_on: source_task_id.iter().cloned().collect(),
        manual_task: false,
        prompt_task: false,
        task_origin: "system_agents".to_string(),
        task_type: TASK_KIND_REVIEW.to_string(),
        backlog_generation_task: false,
        qa_task: false,
        final_qa_task: false,
        followup_task: false,
        qa_fix_task: false,
        qa_verdict_retry_task: false,
        task_level_qa: false,
        agents_knowledge_task: true,
        internal_validation: false,
        qa_round: 0,
        source_task_id,
        source_qa_task_id: None,
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
        group_id: source_group_id.or_else(|| Some(AGENTS_KNOWLEDGE_TASK_ID.to_string())),
        hierarchy,
    }
}

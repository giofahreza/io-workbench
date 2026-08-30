fn allocate_task_id(run: &mut AgenticBoard) -> String {
    let max_existing_sequence = run
        .tasks
        .iter()
        .filter_map(|task| numeric_task_sequence(&task.id))
        .max()
        .unwrap_or(0);
    run.next_task_sequence = run.next_task_sequence.max(max_existing_sequence);
    loop {
        let Some(next_sequence) = run.next_task_sequence.checked_add(1) else {
            return format!("task-{}", Uuid::new_v4());
        };
        run.next_task_sequence = next_sequence;
        let candidate = format!("task-{next_sequence}");
        if !run.tasks.iter().any(|task| task.id == candidate) {
            return candidate;
        }
    }
}

fn numeric_task_sequence(task_id: &str) -> Option<u64> {
    task_id.strip_prefix("task-")?.parse().ok()
}

fn title_from_prompt(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return None;
    }
    let first = trimmed.lines().find(|line| !line.trim().is_empty())?.trim();
    let mut title = first
        .trim_start_matches(['-', '*', '•'])
        .trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.' || ch == ')')
        .trim()
        .to_string();
    if title.len() > 96 {
        title.truncate(93);
        title.push_str("...");
    }
    Some(title)
}

fn value_to_strings(value: Option<Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| item.as_str().map(str::trim).map(str::to_string))
            .filter(|item| !item.is_empty())
            .collect(),
        Some(Value::String(text)) => text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn task_counts(tasks: &[BoardTask], orchestration_version: u32) -> Value {
    count_statuses(
        tasks
            .iter()
            .filter(|task| !task.backlog_generation_task)
            .filter(|task| orchestration_version < 2 || !task.internal_validation)
            .map(|task| task.status.as_str()),
    )
}

fn task_group_counts(run: &AgenticBoard) -> Value {
    let groups = task_groups_for_counts(&run.tasks, run.orchestration_version);
    count_statuses(
        groups
            .iter()
            .map(|(_, tasks)| task_group_status_for_board(run, tasks)),
    )
}

fn task_groups_for_counts(
    tasks: &[BoardTask],
    orchestration_version: u32,
) -> Vec<(String, Vec<&BoardTask>)> {
    let mut groups: Vec<(String, Vec<&BoardTask>)> = Vec::new();
    for task in tasks
        .iter()
        .filter(|task| task_is_visible_work_item(task, orchestration_version))
    {
        let group_id = task_group_id_or_self(task);
        if let Some((_, group_tasks)) = groups.iter_mut().find(|(id, _)| id == &group_id) {
            group_tasks.push(task);
        } else {
            groups.push((group_id, vec![task]));
        }
    }
    groups
}

fn count_statuses<'a>(statuses: impl Iterator<Item = &'a str>) -> Value {
    let mut counts = serde_json::Map::new();
    let mut total = 0usize;
    for status in statuses {
        total += 1;
        let key = canonical_task_status(status);
        let next = counts.get(key).and_then(Value::as_u64).unwrap_or(0) + 1;
        counts.insert(key.to_string(), json!(next));
    }
    counts.insert("total".to_string(), json!(total));
    Value::Object(counts)
}

fn normalize_provider(provider: Option<&str>) -> Result<String> {
    let normalized = provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_PROVIDER);
    match normalized {
        "claude" | "codex" | "gemini" => Ok(normalized.to_string()),
        _ => Err(bad_request(
            "Provider must be one of: claude, codex, gemini",
        )),
    }
}

fn normalize_optional_provider(provider: Option<&str>) -> Result<String> {
    match provider.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => normalize_provider(Some(value)),
        None => Ok(String::new()),
    }
}

fn normalize_task_status(status: Option<&str>, default: &str) -> Result<String> {
    let status = status
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default);
    match status {
        TASK_STATUS_BACKLOG => Ok(TASK_STATUS_BACKLOG.to_string()),
        TASK_STATUS_TODO | "pending" | "planned" => Ok(TASK_STATUS_TODO.to_string()),
        TASK_STATUS_IN_PROGRESS | "running" | "pausing" | "cancelling" => {
            Ok(TASK_STATUS_IN_PROGRESS.to_string())
        }
        TASK_STATUS_BLOCKED => Ok(TASK_STATUS_BLOCKED.to_string()),
        TASK_STATUS_FAILED | "cancelled" | "backlog_failed" => Ok(TASK_STATUS_FAILED.to_string()),
        TASK_STATUS_DONE | "completed" => Ok(TASK_STATUS_DONE.to_string()),
        _ => Err(bad_request(
            "Task status must be one of: backlog, todo, in_progress, blocked, failed, done",
        )),
    }
}

fn normalized_task_kind_name(value: &str) -> Option<&'static str> {
    let normalized = value.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match normalized.as_str() {
        "" => None,
        "implementation" | "feature" | "dev" | "development" => Some(TASK_KIND_IMPLEMENTATION),
        "research" | "discovery" => Some(TASK_KIND_RESEARCH),
        "design" | "product_design" | "technical_design" => Some(TASK_KIND_DESIGN),
        "test_implementation" | "test" | "tests" | "test_code" => {
            Some(TASK_KIND_TEST_IMPLEMENTATION)
        }
        "manual_test"
        | "manual_qa"
        | "manual_verification"
        | "functional_verification"
        | "verification"
        | "smoke_test"
        | "smoke"
        | "smoke_pass"
        | "emulator_test" => Some(TASK_KIND_MANUAL_TEST),
        "qa" | "final_qa" | "task_qa" => Some(TASK_KIND_QA),
        "review" | "promotion" | "agents_knowledge" => Some(TASK_KIND_REVIEW),
        "fix" | "qa_fix" => Some(TASK_KIND_FIX),
        "followup" | "follow_up" => Some(TASK_KIND_FOLLOWUP),
        "migration" | "database_migration" => Some(TASK_KIND_MIGRATION),
        "revert" | "rollback" => Some(TASK_KIND_REVERT),
        "cleanup" | "clean_up" => Some(TASK_KIND_CLEANUP),
        "revision" | "revise" => Some(TASK_KIND_REVISION),
        "replacement" | "replace" => Some(TASK_KIND_REPLACEMENT),
        _ => None,
    }
}

fn normalize_task_kind(value: Option<&str>, default: &'static str) -> &'static str {
    value.and_then(normalized_task_kind_name).unwrap_or(default)
}

fn normalize_task_level(value: Option<&str>, default: &'static str) -> &'static str {
    let normalized = value
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_");
    match normalized.as_str() {
        TASK_LEVEL_INITIATIVE => TASK_LEVEL_INITIATIVE,
        TASK_LEVEL_EPIC => TASK_LEVEL_EPIC,
        TASK_LEVEL_STORY => TASK_LEVEL_STORY,
        TASK_LEVEL_TASK => TASK_LEVEL_TASK,
        TASK_LEVEL_SUBTASK => TASK_LEVEL_SUBTASK,
        _ => default,
    }
}

fn infer_prompt_task_kind(
    title: &str,
    details: &str,
    acceptance_criteria: &[String],
) -> Option<&'static str> {
    let text = std::iter::once(title)
        .chain(std::iter::once(details))
        .chain(acceptance_criteria.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    let manual_verification_signal = (text.contains("manual")
        && (text.contains("test")
            || text.contains("qa")
            || text.contains("verify")
            || text.contains("verification")
            || text.contains("smoke")))
        || text.contains("manual smoke")
        || text.contains("smoke test")
        || text.contains("smoke pass")
        || text.contains("functional verification")
        || text.contains("functional verify")
        || text.contains("run mobile verification")
        || text.contains("run verification")
        || text.contains("test on emulator")
        || text.contains("android emulator")
        || text.contains("ios simulator")
        || text.contains("mobile emulator");
    manual_verification_signal.then_some(TASK_KIND_MANUAL_TEST)
}

fn prompt_task_kind_from_value(
    value: &Value,
    title: &str,
    details: &str,
    acceptance_criteria: &[String],
) -> &'static str {
    let explicit = value
        .get("kind")
        .or_else(|| value.get("taskType"))
        .or_else(|| value.get("task_type"))
        .and_then(Value::as_str);
    explicit
        .and_then(normalized_task_kind_name)
        .or_else(|| infer_prompt_task_kind(title, details, acceptance_criteria))
        .unwrap_or(TASK_KIND_IMPLEMENTATION)
}

fn infer_legacy_user_task_kind(task: &BoardTask) -> Option<&'static str> {
    if canonical_task_kind(task) != TASK_KIND_IMPLEMENTATION || !is_user_authored_task(task) {
        return None;
    }
    let details = [
        task.details.as_str(),
        task.description.as_str(),
        task.prompt.as_str(),
    ]
    .into_iter()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join("\n");
    infer_prompt_task_kind(&task.title, &details, &task.acceptance_criteria)
}

fn canonical_task_status(status: &str) -> &'static str {
    match status.trim() {
        TASK_STATUS_BACKLOG => TASK_STATUS_BACKLOG,
        TASK_STATUS_TODO | "pending" | "planned" => TASK_STATUS_TODO,
        TASK_STATUS_IN_PROGRESS | "running" | "pausing" | "cancelling" | "backlog_generating" => {
            TASK_STATUS_IN_PROGRESS
        }
        TASK_STATUS_BLOCKED => TASK_STATUS_BLOCKED,
        TASK_STATUS_FAILED | "cancelled" | "backlog_failed" => TASK_STATUS_FAILED,
        TASK_STATUS_DONE | "completed" => TASK_STATUS_DONE,
        "qa" | "review" => TASK_STATUS_TODO,
        _ => TASK_STATUS_BACKLOG,
    }
}

fn task_status_is_todo(status: &str) -> bool {
    canonical_task_status(status) == TASK_STATUS_TODO
}

fn task_status_is_backlog(status: &str) -> bool {
    canonical_task_status(status) == TASK_STATUS_BACKLOG
}

fn task_status_is_active(status: &str) -> bool {
    canonical_task_status(status) == TASK_STATUS_IN_PROGRESS
}

fn task_status_is_done(status: &str) -> bool {
    canonical_task_status(status) == TASK_STATUS_DONE
}

fn clear_backlog_approval(task: &mut BoardTask) {
    task.hierarchy.side_effects_approved = false;
    task.hierarchy.side_effect_approval = None;
    task.hierarchy.research_accepted = false;
    task.hierarchy.research_acceptance = None;
}

fn task_level(task: &BoardTask) -> &'static str {
    match task.hierarchy.level.as_str() {
        TASK_LEVEL_INITIATIVE => TASK_LEVEL_INITIATIVE,
        TASK_LEVEL_EPIC => TASK_LEVEL_EPIC,
        TASK_LEVEL_STORY => TASK_LEVEL_STORY,
        TASK_LEVEL_TASK => TASK_LEVEL_TASK,
        TASK_LEVEL_SUBTASK => TASK_LEVEL_SUBTASK,
        _ if task.hierarchy.parent_id.is_some() => TASK_LEVEL_SUBTASK,
        _ => TASK_LEVEL_STORY,
    }
}

fn task_is_executable(task: &BoardTask) -> bool {
    task.hierarchy.executable && task_level(task) == TASK_LEVEL_SUBTASK
}

fn task_blockers(task: &BoardTask) -> Vec<String> {
    dedupe_strings(
        task.hierarchy
            .blocked_by
            .iter()
            .cloned()
            .chain(task.depends_on.iter().cloned())
            .collect(),
    )
}

fn task_is_runnable(task: &BoardTask) -> bool {
    !task.backlog_generation_task && task_is_executable(task) && task_status_is_todo(&task.status)
}

fn completed_hierarchy_ancestor_id(run: &AgenticBoard, parent_id: Option<&str>) -> Option<String> {
    let mut current_id = parent_id.map(str::to_string);
    let mut visited = BTreeSet::new();
    while let Some(id) = current_id {
        if !visited.insert(id.clone()) {
            return None;
        }
        let parent = run.tasks.iter().find(|task| task.id == id)?;
        if task_status_is_done(&parent.status) {
            return Some(parent.id.clone());
        }
        current_id = parent.hierarchy.parent_id.clone();
    }
    None
}

fn validate_parent_scope_not_completed(run: &AgenticBoard, parent_id: Option<&str>) -> Result<()> {
    if let Some(completed_id) = completed_hierarchy_ancestor_id(run, parent_id) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "Cannot add, edit, or detach a child beneath completed parent {completed_id}. Create a linked revision, fix, research, or replacement item instead."
            ),
        ));
    }
    Ok(())
}

fn validate_manual_task_status(run: &AgenticBoard, task: &BoardTask) -> Result<()> {
    if !uses_hierarchical_orchestration(run) {
        return Ok(());
    }
    validate_parent_scope_not_completed(run, task.hierarchy.parent_id.as_deref())?;
    if !matches!(
        canonical_task_status(&task.status),
        TASK_STATUS_BACKLOG | TASK_STATUS_TODO
    ) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "New Kanban items may only start in Backlog or Todo.",
        ));
    }
    if task_status_is_todo(&task.status) && !task_ancestors_are_approved(run, task) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Approve the parent planning item before adding a nested item to Todo.",
        ));
    }
    if task_status_is_todo(&task.status) && !task_side_effects_are_approved(task) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            task_side_effect_block_reason(task),
        ));
    }
    Ok(())
}

fn validate_task_dependency_references(run: &AgenticBoard, task: &BoardTask) -> Result<()> {
    let known_ids = run
        .tasks
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect::<BTreeSet<_>>();
    for dependency in task_blockers(task) {
        if dependency == task.id {
            return Err(bad_request(format!(
                "Task {} cannot depend on itself.",
                task.id
            )));
        }
        if !known_ids.contains(dependency.as_str()) {
            return Err(bad_request(format!(
                "Task {} depends on missing task {}.",
                task.id, dependency
            )));
        }
    }
    Ok(())
}

fn validate_manual_task_source(run: &AgenticBoard, task: &BoardTask) -> Result<()> {
    let Some(source_id) = task.source_task_id.as_deref() else {
        if task.manual_task && canonical_task_kind(task) == TASK_KIND_FIX {
            return Err(bad_request(
                "A user-created fix must include sourceTaskId for the failed or defective work it addresses.",
            ));
        }
        return Ok(());
    };
    let source = run
        .tasks
        .iter()
        .find(|candidate| candidate.id == source_id)
        .ok_or_else(|| bad_request(format!("Source task not found: {source_id}")))?;
    if source.id == task.id {
        return Err(bad_request("A task cannot link to itself as its source."));
    }
    if canonical_task_kind(task) != TASK_KIND_FIX {
        return Ok(());
    }
    let source_is_defective = matches!(
        canonical_task_status(&source.status),
        TASK_STATUS_FAILED | TASK_STATUS_BLOCKED
    ) || (task_is_done(source)
        && (source.qa_passed == Some(false)
            || source
                .result
                .as_ref()
                .and_then(|result| result.get("status"))
                .and_then(Value::as_str)
                == Some("needs_followup")));
    if !source_is_defective {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "A fix must link to failed, blocked, or explicitly defected QA/manual/review work.",
        ));
    }
    if source
        .hierarchy
        .parent_id
        .as_deref()
        .is_some_and(|parent_id| task.hierarchy.parent_id.as_deref() != Some(parent_id))
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "A fix subtask must be placed under the same parent as the failed subtask.",
        ));
    }
    if matches!(
        canonical_task_status(&source.status),
        TASK_STATUS_FAILED | TASK_STATUS_BLOCKED
    ) && task_blockers(task).iter().any(|id| id == source_id)
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "A fix must link to failed work through sourceTaskId, not depend on that failed work.",
        ));
    }
    Ok(())
}

fn task_ancestors_are_approved(run: &AgenticBoard, task: &BoardTask) -> bool {
    // `sourceTaskId`/`sourceQaTaskId` are links used to group legacy system
    // QA and follow-up work. They are not approval edges. Only an explicit
    // hierarchy parent can hold a child behind an approval boundary. A
    // completed parent is also a boundary for required work, but an optional
    // child is an explicitly approvable nice-to-have and may run without
    // reopening the completed scope.
    let mut current_id = task.hierarchy.parent_id.clone();
    let mut child_required = task.hierarchy.required;
    let mut visited = BTreeSet::new();
    while let Some(parent_id) = current_id {
        if !visited.insert(parent_id.clone()) {
            return false;
        }
        let Some(parent) = run.tasks.iter().find(|candidate| candidate.id == parent_id) else {
            return false;
        };
        if task_rollup_completion_is_satisfied(parent) {
            // A completed required child would reopen or rewrite completed
            // scope. Optional branches are separate, explicitly approved
            // nice-to-have work and remain runnable under that boundary.
            if child_required || parent.superseded_by.is_some() {
                return false;
            }
        } else if !matches!(
            canonical_task_status(&parent.status),
            TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS
        ) {
            return false;
        }
        child_required = parent.hierarchy.required;
        current_id = parent.hierarchy.parent_id.clone();
    }
    true
}

fn task_is_runnable_in_board(run: &AgenticBoard, task: &BoardTask) -> bool {
    task_is_runnable(task)
        && task_ancestors_are_approved(run, task)
        && task_side_effects_are_approved(task)
        && (!has_pending_research_acceptance(run)
            || canonical_task_kind(task) == TASK_KIND_RESEARCH)
}

fn task_is_done(task: &BoardTask) -> bool {
    task_status_is_done(&task.status)
}

fn task_is_visible_work_item(task: &BoardTask, orchestration_version: u32) -> bool {
    !task.backlog_generation_task && (orchestration_version < 2 || !task.internal_validation)
}

fn sanitize_kanban_value(value: &Value) -> Value {
    strip_removed_tracking_fields(&redact_transcript_value(value))
}

fn sanitize_kanban_structure(value: &Value) -> Value {
    strip_removed_tracking_fields(value)
}

fn strip_removed_tracking_fields(value: &Value) -> Value {
    match value {
        Value::String(text) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                let cleaned = strip_removed_tracking_fields(&parsed);
                if cleaned != parsed {
                    return serde_json::to_string(&cleaned)
                        .map(Value::String)
                        .unwrap_or_else(|_| Value::String(text.clone()));
                }
            }
            if is_removed_tracking_text(text) {
                Value::String("[legacy tracking removed]".to_string())
            } else {
                Value::String(text.clone())
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .filter(|item| !item.as_str().is_some_and(is_removed_tracking_text))
                .map(strip_removed_tracking_fields)
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !key.to_ascii_lowercase().contains("requirement"))
                .map(|(key, value)| (key.clone(), strip_removed_tracking_fields(value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn is_removed_tracking_text(text: &str) -> bool {
    let normalized = text.trim().to_ascii_lowercase();
    normalized.contains("requirement") || normalized.contains("req-")
}

fn task_group_id_or_self(task: &BoardTask) -> String {
    trim_string(task.group_id.clone()).unwrap_or_else(|| task.id.clone())
}

fn task_group_id_for_source(task: &BoardTask) -> String {
    task_group_id_or_self(task)
}

fn task_title_for_group(task: &BoardTask) -> String {
    [
        task.title.as_str(),
        task.details.as_str(),
        task.description.as_str(),
        task.prompt.as_str(),
        task.summary.as_str(),
    ]
    .into_iter()
    .map(str::trim)
    .find(|value| !value.is_empty())
    .unwrap_or("Work item")
    .to_string()
}

fn is_kanban_parent_task(task: &BoardTask) -> bool {
    !task.qa_task
        && !task.final_qa_task
        && !task.followup_task
        && !task.qa_fix_task
        && !task.qa_verdict_retry_task
        && !task.task_level_qa
        && !task.agents_knowledge_task
        && task.source_task_id.is_none()
        && task.source_qa_task_id.is_none()
        && matches!(
            canonical_task_kind(task),
            TASK_KIND_IMPLEMENTATION | TASK_KIND_MANUAL_TEST | TASK_KIND_REVIEW
        )
}

fn canonical_task_kind(task: &BoardTask) -> &'static str {
    if task.qa_fix_task || task.source_qa_task_id.is_some() || task.task_type == "qa_fix" {
        TASK_KIND_FIX
    } else if task.followup_task || task.task_type == "followup" {
        TASK_KIND_FOLLOWUP
    } else if task.qa_task
        || task.final_qa_task
        || task.task_level_qa
        || task.id == FINAL_QA_TASK_ID
        || matches!(task.task_type.as_str(), "qa" | "final_qa")
    {
        TASK_KIND_QA
    } else if task.agents_knowledge_task
        || task.id == AGENTS_KNOWLEDGE_TASK_ID
        || task.id == PROMOTION_REVIEW_TASK_ID
        || matches!(
            task.task_type.as_str(),
            "review" | "promotion" | "agents_knowledge"
        )
    {
        TASK_KIND_REVIEW
    } else {
        normalize_task_kind(Some(&task.task_type), TASK_KIND_IMPLEMENTATION)
    }
}

fn canonical_task_origin(origin: &str) -> &str {
    match origin.trim() {
        "manual" => "user_manual",
        "prompt_breakdown" => "user_prompt_generated",
        "planner" => "planned",
        value => value,
    }
}

fn normalize_board_provenance(run: &mut AgenticBoard) {
    for task in &mut run.tasks {
        let canonical = canonical_task_origin(&task.task_origin);
        task.task_origin = if canonical.is_empty() {
            infer_legacy_task_origin(task)
                .unwrap_or_default()
                .to_string()
        } else {
            canonical.to_string()
        };
        normalize_done_followup_task(task);
    }
}

fn normalize_done_followup_task(task: &mut BoardTask) {
    if canonical_task_status(&task.status) != TASK_STATUS_DONE {
        return;
    }
    if task
        .result
        .as_ref()
        .is_some_and(completion_evidence_gate_failed)
    {
        task.status = TASK_STATUS_BLOCKED.to_string();
        task.qa_passed = Some(false);
        task.error = Some(
            "Completion evidence gate failed; provide valid evidence before this subtask can be done."
                .to_string(),
        );
        task.completed_at = None;
        return;
    }
    let needs_followup = task
        .result
        .as_ref()
        .and_then(|result| result.get("status"))
        .and_then(Value::as_str)
        == Some("needs_followup");
    if !needs_followup {
        return;
    }
    task.error = None;
    if task.tdd_phase == "fix_pending" {
        task.tdd_phase = "followup_pending".to_string();
    }
}

fn normalize_board_model(run: &mut AgenticBoard) {
    if run.orchestration_version < 3 {
        run.orchestration_version = 3;
    }
    if run.model.trim().is_empty() {
        run.model = default_model_for_provider(&run.provider);
    }
    if run.primary_model.trim().is_empty() {
        run.primary_model = run.model.clone();
    }
    normalize_board_provenance(run);
    run.backlog_breakdown = sanitize_kanban_value(&run.backlog_breakdown);
    run.discussion_proposals = run
        .discussion_proposals
        .iter()
        .map(sanitize_kanban_value)
        .collect();
    let mut legacy_breakdown: Option<Value> = None;
    run.tasks.retain_mut(|task| {
        if task.backlog_generation_task {
            let status = canonical_task_status(&task.status);
            legacy_breakdown = Some(json!({
                "status": if status == TASK_STATUS_FAILED { TASK_STATUS_FAILED } else { "idle" },
                "legacyTaskId": task.id.clone(),
                "prompt": task.prompt.clone(),
                "error": task.error.clone(),
                "updatedAt": Utc::now(),
            }));
            return false;
        }
        let previous_status = task.status.clone();
        if previous_status == "qa" && !task.qa_task {
            task.qa_task = true;
        }
        task.status = canonical_task_status(&previous_status).to_string();
        task.task_type = infer_legacy_user_task_kind(task)
            .unwrap_or_else(|| canonical_task_kind(task))
            .to_string();
        task.transcript = task.transcript.iter().map(sanitize_kanban_value).collect();
        task.hierarchy.discussion = task
            .hierarchy
            .discussion
            .iter()
            .map(sanitize_kanban_value)
            .collect();
        task.hierarchy.attempts = task
            .hierarchy
            .attempts
            .iter()
            .map(sanitize_kanban_value)
            .collect();
        task.hierarchy.side_effect_approval = task
            .hierarchy
            .side_effect_approval
            .as_ref()
            .map(sanitize_kanban_value);
        task.hierarchy.research_acceptance = task
            .hierarchy
            .research_acceptance
            .as_ref()
            .map(sanitize_kanban_value);
        task.changed_file_summary = task
            .changed_file_summary
            .as_ref()
            .map(sanitize_kanban_value)
            .map(|summary| normalize_changed_file_summary(&summary));
        task.result = task.result.as_ref().map(sanitize_kanban_value);
        let ownership_is_known = task
            .changed_file_summary
            .as_ref()
            .and_then(|summary| summary.get("ownershipPolicy"))
            .and_then(Value::as_str)
            == Some(WORKSPACE_OWNERSHIP_POLICY);
        if !ownership_is_known {
            task.changed_files.clear();
            if let Some(result) = task.result.as_mut().and_then(Value::as_object_mut) {
                result.insert("changedFiles".to_string(), json!([]));
            }
        }
        task.result_validation = task.result_validation.as_ref().map(sanitize_kanban_value);
        task.deterministic_validation = task
            .deterministic_validation
            .as_ref()
            .map(sanitize_kanban_value);
        task.rag_context_refs = task
            .rag_context_refs
            .iter()
            .map(sanitize_kanban_value)
            .collect();
        task.qa_baseline_validation = task
            .qa_baseline_validation
            .as_ref()
            .map(sanitize_kanban_value);
        task.coverage_evidence = task
            .coverage_evidence
            .iter()
            .map(sanitize_kanban_value)
            .collect();
        true
    });
    normalize_board_hierarchy(run);
    normalize_board_task_groups(run);
    if run.backlog_breakdown.is_null()
        || !run.backlog_breakdown.is_object()
        || run
            .backlog_breakdown
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .is_empty()
    {
        run.backlog_breakdown = legacy_breakdown.unwrap_or_else(default_backlog_breakdown);
    }
}

fn normalize_board_hierarchy(run: &mut AgenticBoard) {
    // Parent levels can themselves be repaired from their parent. Several
    // passes make the operation converge for migrated trees and for a user
    // child detached from the middle of a hierarchy. Five levels are the
    // maximum supported depth, so this is bounded and remains safe for
    // malformed cyclic data.
    for _ in 0..5 {
        let task_snapshot = run.tasks.clone();
        for task in &mut run.tasks {
            // Legacy system cards may carry a source link for grouping, but
            // that link must not silently become a hierarchy/approval parent.
            // New hierarchy edges are explicit in `parent_id`.
            //
            // Older snapshots were normalized by copying the source id into
            // `parent_id`. Repair that one-way migration while retaining the
            // source link for group display. A legacy system card sourced from
            // a subtask becomes a sibling under the source's explicit parent;
            // cards sourced from a planning item remain root-level system work.
            if source_link_is_structural(task)
                && task
                    .hierarchy
                    .parent_id
                    .as_deref()
                    .is_some_and(|parent_id| {
                        task.source_task_id.as_deref() == Some(parent_id)
                            || task.source_qa_task_id.as_deref() == Some(parent_id)
                    })
            {
                let source_id = task
                    .source_task_id
                    .as_deref()
                    .filter(|source_id| task.hierarchy.parent_id.as_deref() == Some(*source_id))
                    .or_else(|| {
                        task.source_qa_task_id.as_deref().filter(|source_id| {
                            task.hierarchy.parent_id.as_deref() == Some(*source_id)
                        })
                    });
                task.hierarchy.parent_id = source_id
                    .and_then(|source_id| {
                        task_snapshot
                            .iter()
                            .find(|candidate| candidate.id == source_id)
                    })
                    .filter(|source| task_level(source) == TASK_LEVEL_SUBTASK)
                    .and_then(|source| source.hierarchy.parent_id.clone());
            }
            let inferred_level = if task.hierarchy.parent_id.is_some()
                || task.internal_validation
                || task.qa_task
                || task.final_qa_task
                || task.followup_task
                || task.qa_fix_task
                || task.qa_verdict_retry_task
                || task.task_level_qa
                || task.agents_knowledge_task
            {
                TASK_LEVEL_SUBTASK
            } else {
                TASK_LEVEL_STORY
            };
            let requested_level = normalize_task_level(
                (!task.hierarchy.level.trim().is_empty()).then_some(task.hierarchy.level.as_str()),
                inferred_level,
            );
            let level = task
                .hierarchy
                .parent_id
                .as_deref()
                .and_then(|parent_id| {
                    task_snapshot
                        .iter()
                        .find(|candidate| candidate.id == parent_id)
                        .and_then(|parent| next_hierarchy_level(task_level(parent)))
                })
                .unwrap_or_else(|| {
                    if task.hierarchy.parent_id.is_none()
                        && matches!(requested_level, TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK)
                        && !source_link_is_structural(task)
                    {
                        TASK_LEVEL_STORY
                    } else {
                        requested_level
                    }
                });
            task.hierarchy.level = level.to_string();
            task.hierarchy.executable = level == TASK_LEVEL_SUBTASK;
            if task.hierarchy.scope_version == 0 {
                task.hierarchy.scope_version = 1;
                task.hierarchy.required = true;
            }
            task.hierarchy.blocked_by = dedupe_strings(
                task.hierarchy
                    .blocked_by
                    .iter()
                    .cloned()
                    .chain(task.depends_on.iter().cloned())
                    .collect(),
            );
            task.depends_on = task.hierarchy.blocked_by.clone();
        }
    }
    reconcile_hierarchy_approval_states(run);
}

fn reconcile_hierarchy_approval_states(run: &mut AgenticBoard) {
    let snapshot = run.tasks.clone();
    let mut invalid = Vec::new();
    for task in &snapshot {
        if task_is_done(task)
            || !matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS
            )
        {
            continue;
        }
        let mut current_id = task.hierarchy.parent_id.clone();
        let mut child_required = task.hierarchy.required;
        let mut visited = BTreeSet::new();
        while let Some(parent_id) = current_id {
            if !visited.insert(parent_id.clone()) {
                invalid.push((task.id.clone(), parent_id, "hierarchy cycle".to_string()));
                break;
            }
            let Some(parent) = snapshot.iter().find(|candidate| candidate.id == parent_id) else {
                invalid.push((task.id.clone(), parent_id, "missing parent".to_string()));
                break;
            };
            let parent_status = canonical_task_status(&parent.status);
            let parent_is_completed = task_rollup_completion_is_satisfied(parent);
            if parent_is_completed {
                // Optional work is an explicit, independently approvable
                // branch. It may remain runnable after the required parent
                // scope is done; required work must not silently reopen it.
                if child_required || parent.superseded_by.is_some() {
                    invalid.push((
                        task.id.clone(),
                        parent.id.clone(),
                        parent_status.to_string(),
                    ));
                    break;
                }
            } else if !matches!(parent_status, TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS) {
                invalid.push((
                    task.id.clone(),
                    parent.id.clone(),
                    parent_status.to_string(),
                ));
                break;
            }
            child_required = parent.hierarchy.required;
            current_id = parent.hierarchy.parent_id.clone();
        }
    }
    for (task_id, parent_id, reason) in invalid {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
            task.status = TASK_STATUS_BLOCKED.to_string();
            task.started_at = None;
            task.completed_at = None;
            task.provider_session_id = None;
            task.error = Some(format!(
                "Parent {parent_id} is not approved ({reason}); approve the parent before running this item."
            ));
        }
    }
}

fn normalize_board_task_groups(run: &mut AgenticBoard) {
    let snapshot = run.tasks.clone();
    let group_ids = snapshot
        .iter()
        .enumerate()
        .map(|(index, _)| infer_task_group_id(&snapshot, index))
        .collect::<Vec<_>>();
    for (task, group_id) in run.tasks.iter_mut().zip(group_ids) {
        if task.backlog_generation_task {
            continue;
        }
        task.group_id = Some(group_id);
    }
}

fn infer_task_group_id(tasks: &[BoardTask], index: usize) -> String {
    let task = &tasks[index];
    if task.final_qa_task || task.id == FINAL_QA_TASK_ID {
        return FINAL_QA_TASK_ID.to_string();
    }
    if task.id == PROMOTION_REVIEW_TASK_ID || matches!(task.task_type.as_str(), "promotion") {
        return PROMOTION_REVIEW_TASK_ID.to_string();
    }
    let mut current_index = index;
    let mut visited = BTreeSet::new();
    loop {
        let current = &tasks[current_index];
        let Some(parent_id) = task_parent_id(current) else {
            break;
        };
        if !visited.insert(parent_id.to_string()) {
            break;
        }
        let Some(parent_index) = tasks.iter().position(|candidate| candidate.id == parent_id)
        else {
            return parent_id.to_string();
        };
        if let Some(group_id) = trim_string(tasks[parent_index].group_id.clone()) {
            return group_id;
        }
        current_index = parent_index;
    }

    trim_string(task.group_id.clone()).unwrap_or_else(|| tasks[current_index].id.clone())
}

fn infer_legacy_task_origin(task: &BoardTask) -> Option<&'static str> {
    if task.final_qa_task || task.id == FINAL_QA_TASK_ID {
        Some("system_final_qa")
    } else if task.qa_verdict_retry_task {
        Some("system_qa_verdict_retry")
    } else if task.qa_fix_task {
        Some("system_qa_fix")
    } else if task.task_level_qa || task.qa_task {
        Some("system_qa")
    } else if task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID {
        Some("system_agents")
    } else if task.followup_task {
        Some("system_followup")
    } else if task.references.iter().any(|reference| {
        reference
            .to_ascii_lowercase()
            .contains("suggested backlog task from")
    }) {
        Some("ai_suggested_backlog")
    } else if task.backlog_generation_task || (task.prompt_task && !task.manual_task) {
        Some("user_prompt_generated")
    } else if task.manual_task || task.prompt_task {
        Some("user_manual")
    } else {
        None
    }
}

fn normalize_git_policy(policy: Option<&str>) -> String {
    match policy.map(str::trim).filter(|value| !value.is_empty()) {
        Some("managed") | Some("managed_git") | Some("managed-workflow") => "managed".to_string(),
        _ => "read_only".to_string(),
    }
}

fn normalize_board_profile(profile: Option<&str>) -> String {
    match profile
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('-', "_")
        .as_str()
    {
        "minimal" | "strict" | "cheap" => "minimal".to_string(),
        "product_ready" | "productready" | "product" | "polished" | "expensive" | "quality" => {
            "product_ready".to_string()
        }
        _ => "complete_app".to_string(),
    }
}

fn normalize_board_profile_for_strategy(profile: Option<&str>, strategy: Option<&Value>) -> String {
    if profile
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return normalize_board_profile(profile);
    }
    match model_strategy_mode(strategy) {
        "cheap" => "minimal".to_string(),
        "expensive" => "product_ready".to_string(),
        _ => normalize_board_profile(None),
    }
}

fn project_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("project")
        .to_string()
}

fn trim_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn redact_transcript_value(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact_transcript_text(text)),
        Value::Array(items) => Value::Array(items.iter().map(redact_transcript_value).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let key_lower = key.to_ascii_lowercase();
                    let redacted = if [
                        "api_key",
                        "apikey",
                        "password",
                        "passwd",
                        "secret",
                        "access_token",
                        "refresh_token",
                    ]
                    .iter()
                    .any(|marker| key_lower.contains(marker))
                    {
                        Value::String("[REDACTED]".to_string())
                    } else {
                        redact_transcript_value(value)
                    };
                    (key.clone(), redacted)
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn redact_transcript_text(text: &str) -> String {
    let mut redacted = text.to_string();
    redacted = redact_secret_after_marker(&redacted, "bearer");
    for marker in [
        "api_key",
        "apikey",
        "password",
        "passwd",
        "secret",
        "access_token",
        "refresh_token",
        "minimax_api_key",
        "authorization",
    ] {
        redacted = redact_secret_after_marker(&redacted, marker);
    }
    redacted
}

fn redact_secret_after_marker(text: &str, marker: &str) -> String {
    let mut result = text.to_string();
    let marker = marker.to_ascii_lowercase();
    loop {
        let lower = result.to_ascii_lowercase();
        let Some(marker_start) = lower.find(&marker) else {
            break;
        };
        let mut value_start = marker_start + marker.len();
        while value_start < result.len()
            && (result.as_bytes()[value_start].is_ascii_whitespace()
                || matches!(result.as_bytes()[value_start], b'=' | b':' | b'"' | b'\''))
        {
            value_start += 1;
        }
        if value_start >= result.len() {
            break;
        }
        let value_end = result[value_start..]
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | '}' | ']'))
            .map(|(index, _)| value_start + index)
            .unwrap_or(result.len());
        if value_end <= value_start {
            break;
        }
        result.replace_range(value_start..value_end, "[REDACTED]");
    }
    result
}

fn default_orchestration_version() -> u32 {
    2
}

fn default_provider_string() -> String {
    DEFAULT_PROVIDER.to_string()
}

fn default_board_profile() -> String {
    normalize_board_profile(None)
}

fn default_git_policy() -> String {
    "read_only".to_string()
}

fn default_paused_status() -> String {
    "paused".to_string()
}

fn default_priority() -> String {
    TASK_PRIORITY_P2.to_string()
}

fn default_task_type() -> String {
    "implementation".to_string()
}

fn default_task_level() -> String {
    TASK_LEVEL_STORY.to_string()
}

fn default_required_task() -> bool {
    true
}

fn default_backlog_breakdown() -> Value {
    json!({ "status": "idle" })
}

fn default_tdd_enabled() -> bool {
    !matches!(
        env::var("IO_WORKBENCH_TDD_ENABLED")
            .unwrap_or_else(|_| "true".to_string())
            .trim()
            .to_lowercase()
            .as_str(),
        "false" | "0" | "no"
    )
}

fn default_tdd_phase() -> String {
    "qa_pending".to_string()
}

fn default_tdd_policy() -> Value {
    json!({
        "requireFailingTestBeforeDev": true,
        "maxFixAttempts": 3,
        "allowImplementationWithoutTests": false,
        "qaCommandStage": "qa",
        "featureCommandStage": "feature",
        "finalCommandStage": "final",
    })
}

fn default_provider_usage() -> Value {
    json!({
        "inputTokens": 0,
        "cachedInputTokens": 0,
        "outputTokens": 0,
        "totalTokens": 0,
        "invocationsWithUsage": 0,
    })
}

fn bad_request(message: impl Into<String>) -> ServerError {
    ServerError::new(StatusCode::BAD_REQUEST, message)
}

fn not_found(message: impl Into<String>) -> ServerError {
    ServerError::new(StatusCode::NOT_FOUND, message)
}

fn task_generation_error(details: impl Into<String>) -> ServerError {
    ServerError::with_details(
        StatusCode::BAD_GATEWAY,
        "Failed to generate task drafts",
        details,
    )
}

fn io_error(error: std::io::Error) -> ServerError {
    ServerError::with_details(
        StatusCode::INTERNAL_SERVER_ERROR,
        "failed to access agentic board storage",
        error.to_string(),
    )
}

async fn compact_provider_session_after_task_group(
    state: &AppState,
    run: &AgenticBoard,
    task_id: &str,
) -> Option<Value> {
    let task = run.tasks.iter().find(|task| task.id == task_id)?;
    let group_id = task.group_id.clone().unwrap_or_else(|| task.id.clone());
    if run
        .compaction_ledger
        .iter()
        .any(|entry| entry.get("groupId").and_then(Value::as_str) == Some(group_id.as_str()))
    {
        return None;
    }
    if normalize_session_policy(Some(&run.session_policy)) != "continuous" {
        return Some(json!({
            "groupId": group_id,
            "taskId": task_id,
            "status": "skipped",
            "reason": "Session policy is not continuous.",
            "createdAt": Utc::now(),
        }));
    }
    let Some(session_id) = reusable_session_id(run) else {
        return Some(json!({
            "groupId": group_id,
            "taskId": task_id,
            "status": "skipped",
            "reason": "No reusable provider session was available.",
            "createdAt": Utc::now(),
        }));
    };
    if run.provider != "claude" {
        return Some(json!({
            "groupId": group_id,
            "taskId": task_id,
            "status": "skipped",
            "reason": format!("Provider {} does not support automatic /compact.", run.provider),
            "sessionId": session_id,
            "createdAt": Utc::now(),
        }));
    }
    let started_at = Utc::now();
    match execute_provider_prompt(state, run, "context compaction", "/compact").await {
        Ok(output) => Some(json!({
            "groupId": group_id,
            "taskId": task_id,
            "status": "completed",
            "sessionId": session_id,
            "startedAt": started_at,
            "completedAt": Utc::now(),
            "summary": limit_text(&output.output, 600),
        })),
        Err(error) => Some(json!({
            "groupId": group_id,
            "taskId": task_id,
            "status": "failed",
            "sessionId": session_id,
            "startedAt": started_at,
            "completedAt": Utc::now(),
            "error": server_error_message(&error),
        })),
    }
}

fn pending_research_acceptance_ids(run: &AgenticBoard) -> Vec<String> {
    run.tasks
        .iter()
        .filter(|task| task_is_executable(task))
        .filter(|task| canonical_task_kind(task) == TASK_KIND_RESEARCH)
        .filter(|task| task_status_is_done(&task.status))
        .filter(|task| !task.hierarchy.research_accepted)
        .map(|task| task.id.clone())
        .collect()
}

fn has_pending_research_acceptance(run: &AgenticBoard) -> bool {
    !pending_research_acceptance_ids(run).is_empty()
}

fn has_backlog_planning_work(run: &AgenticBoard) -> bool {
    run.tasks.iter().any(|task| {
        !task.backlog_generation_task
            && task_status_is_backlog(&task.status)
            && !task_is_executable(task)
    })
}

fn pick_next_task_index(run: &AgenticBoard) -> Option<usize> {
    let mut ready = Vec::<(usize, u8, i64)>::new();
    for (index, task) in run.tasks.iter().enumerate() {
        if !task_is_runnable_in_board(run, task) {
            continue;
        }
        let unmet = unmet_task_dependencies(run, task);
        if unmet.is_empty() {
            ready.push((
                index,
                task_priority_rank(&task.priority),
                task.hierarchy.rank.max(0),
            ));
        }
    }
    ready
        .into_iter()
        .min_by_key(|(index, priority, rank)| (*priority, *rank, *index))
        .map(|(index, _, _)| index)
}

fn unmet_task_dependencies(run: &AgenticBoard, task: &BoardTask) -> Vec<String> {
    let mut dependencies = task_blockers(task);
    if let Some(id) = retry_fix_dependency(task) {
        if !dependencies.contains(&id) {
            dependencies.push(id);
        }
    }
    dependencies
        .into_iter()
        .filter(|id| id != &task.id)
        .filter(|id| {
            run.tasks
                .iter()
                .find(|candidate| candidate.id == *id)
                .is_none_or(|candidate| !task_dependency_is_satisfied(candidate))
        })
        .collect()
}

fn task_dependency_is_satisfied(task: &BoardTask) -> bool {
    task_is_done(task) && task.superseded_by.is_none()
}

fn dependency_block_reason(run: &AgenticBoard, dependencies: &[String]) -> String {
    let missing = dependencies
        .iter()
        .filter(|dependency| !run.tasks.iter().any(|task| task.id == **dependency))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return format!("Missing dependency: {}", missing.join(", "));
    }
    let superseded = dependencies
        .iter()
        .filter_map(|dependency| {
            run.tasks
                .iter()
                .find(|task| task.id == *dependency)
                .and_then(|task| {
                    task.superseded_by
                        .as_ref()
                        .map(|replacement| format!("{dependency} (replacement {replacement})"))
                })
        })
        .collect::<Vec<_>>();
    if !superseded.is_empty() {
        return format!(
            "Superseded dependency: {}. Choose the replacement dependency or remove the blocker.",
            superseded.join(", ")
        );
    }
    format!("Waiting on dependency: {}", dependencies.join(", "))
}

fn is_dependency_block_error(error: &str) -> bool {
    error.starts_with("Waiting on dependency:")
        || error.starts_with("Missing dependency:")
        || error.starts_with("Superseded dependency:")
}

fn retry_fix_dependency(task: &BoardTask) -> Option<String> {
    task.hierarchy
        .attempts
        .iter()
        .rev()
        .find(|attempt| attempt.get("kind").and_then(Value::as_str) == Some("retry_request"))
        .filter(|attempt| attempt.get("mode").and_then(Value::as_str) == Some(RETRY_MODE_FIX))
        .and_then(|attempt| attempt.get("fixTaskId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn hierarchy_validation_issues(run: &AgenticBoard) -> Vec<String> {
    let by_id = run
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect::<BTreeMap<_, _>>();
    let mut issues = Vec::new();
    for task in &run.tasks {
        let Some(parent_id) = task.hierarchy.parent_id.as_deref() else {
            continue;
        };
        if parent_id == task.id {
            issues.push(format!(
                "Hierarchy item {} cannot be its own parent.",
                task.id
            ));
            continue;
        }
        let Some(parent) = by_id.get(parent_id) else {
            issues.push(format!(
                "Hierarchy item {} has missing parent {}.",
                task.id, parent_id
            ));
            continue;
        };
        let valid_parent = match task_level(task) {
            TASK_LEVEL_EPIC => task_level(parent) == TASK_LEVEL_INITIATIVE,
            TASK_LEVEL_STORY => task_level(parent) == TASK_LEVEL_EPIC,
            TASK_LEVEL_TASK => task_level(parent) == TASK_LEVEL_STORY,
            TASK_LEVEL_SUBTASK => task_level(parent) == TASK_LEVEL_TASK,
            TASK_LEVEL_INITIATIVE => false,
            _ => false,
        };
        if !valid_parent {
            issues.push(format!(
                "Hierarchy item {} ({}) has invalid parent {} ({}).",
                task.id,
                task_level(task),
                parent.id,
                task_level(parent)
            ));
        }
        // Compare each item with every explicit hierarchy ancestor. A
        // neutral intermediate task must not hide a contradiction between a
        // story and a subtask farther down the plan.
        let mut ancestor_id = Some(parent.id.as_str());
        let mut seen_ancestors = BTreeSet::new();
        while let Some(ancestor_id_value) = ancestor_id {
            if !seen_ancestors.insert(ancestor_id_value) {
                break;
            }
            let Some(ancestor) = by_id.get(ancestor_id_value) else {
                break;
            };
            for ancestor_criterion in &ancestor.acceptance_criteria {
                for child_criterion in &task.acceptance_criteria {
                    if let Some(conflict) =
                        acceptance_criteria_conflict(ancestor_criterion, child_criterion)
                    {
                        issues.push(format!(
                            "Acceptance criteria conflict between {} and {}: {} Parent: {} Child: {}",
                            ancestor.id, task.id, conflict, ancestor_criterion, child_criterion,
                        ));
                    }
                }
            }
            ancestor_id = ancestor.hierarchy.parent_id.as_deref();
        }
    }
    for task in &run.tasks {
        let mut current = task.id.as_str();
        let mut seen = BTreeSet::new();
        while let Some(item) = by_id.get(current) {
            if !seen.insert(current) {
                issues.push(format!("Hierarchy cycle detected at {}.", current));
                break;
            }
            let Some(parent_id) = item.hierarchy.parent_id.as_deref() else {
                break;
            };
            current = parent_id;
        }
    }
    issues.sort();
    issues.dedup();
    issues
}

fn dependency_validation_issues(run: &AgenticBoard) -> Vec<String> {
    let known_ids = run
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut issues = Vec::new();
    for task in &run.tasks {
        for dependency in task_blockers(task) {
            if dependency == task.id {
                issues.push(format!("Task {} cannot depend on itself.", task.id));
            } else if !known_ids.contains(dependency.as_str()) {
                issues.push(format!(
                    "Task {} depends on missing task {}.",
                    task.id, dependency
                ));
            }
        }
    }
    issues.sort();
    issues.dedup();
    issues
}

fn acceptance_criteria_conflict(parent: &str, child: &str) -> Option<&'static str> {
    let parent_constraints = acceptance_constraint_flags(&parent.to_ascii_lowercase());
    let child_constraints = acceptance_constraint_flags(&child.to_ascii_lowercase());
    let conflicts = [
        ("zero is allowed", 0usize, 1usize),
        ("a positive value is required", 1usize, 0usize),
        ("negative values are allowed", 2usize, 3usize),
        ("a non-negative value is required", 3usize, 2usize),
        ("an empty value is allowed", 4usize, 5usize),
        ("a non-empty value is required", 5usize, 4usize),
        ("the value is optional", 6usize, 7usize),
        ("the value is required", 7usize, 6usize),
    ];
    conflicts
        .iter()
        .find(|(_, parent_index, child_index)| {
            parent_constraints[*parent_index] && child_constraints[*child_index]
        })
        .map(|(message, _, _)| *message)
}

fn acceptance_constraint_flags(value: &str) -> [bool; 8] {
    let zero_allowed = value.contains("can be zero")
        || value.contains("may be zero")
        || value.contains("allow zero")
        || value.contains("allows zero")
        || value.contains("zero is valid")
        || value.contains("non-negative");
    let positive_required = value.contains("must be positive")
        || value.contains("must have a positive")
        || value.contains("positive value required")
        || value.contains("greater than zero")
        || value.contains("above zero")
        || value.contains("at least one");
    let negative_allowed = value.contains("can be negative")
        || value.contains("may be negative")
        || value.contains("allow negative")
        || value.contains("negative values are valid");
    let non_negative_required = value.contains("must be non-negative")
        || value.contains("non-negative value required")
        || value.contains("zero or greater")
        || value.contains("not be negative");
    let empty_allowed = value.contains("can be empty")
        || value.contains("may be empty")
        || value.contains("allow empty")
        || value.contains("empty is valid");
    let non_empty_required = value.contains("must not be empty")
        || value.contains("cannot be empty")
        || value.contains("non-empty value required")
        || value.contains("must be non-empty");
    let optional = value.contains("is optional")
        || value.contains("are optional")
        || value.contains("may be omitted")
        || value.contains("can be omitted");
    let required = value.contains("is required")
        || value.contains("are required")
        || value.contains("must be provided")
        || value.contains("required input");
    [
        zero_allowed,
        positive_required,
        negative_allowed,
        non_negative_required,
        empty_allowed,
        non_empty_required,
        optional,
        required,
    ]
}

fn dependency_cycle(run: &AgenticBoard) -> Option<Vec<String>> {
    fn visit(
        run: &AgenticBoard,
        id: &str,
        visiting: &mut Vec<String>,
        visited: &mut BTreeSet<String>,
    ) -> Option<Vec<String>> {
        if let Some(index) = visiting.iter().position(|value| value == id) {
            let mut cycle = visiting[index..].to_vec();
            cycle.push(id.to_string());
            return Some(cycle);
        }
        if !visited.insert(id.to_string()) {
            return None;
        }
        visiting.push(id.to_string());
        let task = run.tasks.iter().find(|task| task.id == id)?;
        for dependency in task_blockers(task) {
            if run.tasks.iter().any(|candidate| candidate.id == dependency)
                && let Some(cycle) = visit(run, &dependency, visiting, visited)
            {
                return Some(cycle);
            }
        }
        visiting.pop();
        Some(Vec::new()).filter(|cycle| !cycle.is_empty())
    }

    let mut visited = BTreeSet::new();
    for task in &run.tasks {
        if let Some(cycle) = visit(run, &task.id, &mut Vec::new(), &mut visited)
            && !cycle.is_empty()
        {
            return Some(cycle);
        }
    }
    None
}

fn planning_error_task_ids(run: &AgenticBoard, issue: &str) -> Vec<String> {
    run.tasks
        .iter()
        .filter(|task| !task.id.trim().is_empty() && issue.contains(&task.id))
        .map(|task| task.id.clone())
        .collect()
}

fn has_persisted_planning_error(run: &AgenticBoard) -> bool {
    run.current_phase.as_deref() == Some(PLANNING_ERROR_PHASE)
        && run
            .phase_details
            .as_ref()
            .and_then(|details| details.get("kind"))
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "hierarchy" || kind == "dependency")
}

fn mark_planning_error(
    run: &mut AgenticBoard,
    affected_task_ids: &[String],
    kind: &str,
    issue: &str,
) {
    let now = Utc::now();
    let affected = affected_task_ids
        .iter()
        .filter(|id| run.tasks.iter().any(|task| task.id == **id))
        .cloned()
        .collect::<Vec<_>>();
    let message = format!("Planning error: {}", issue.trim());
    for task in &mut run.tasks {
        if affected.iter().any(|id| id == &task.id) {
            // Completed work is immutable. It is still included in the
            // board-level issue details, but only unfinished affected items
            // are moved to Blocked.
            if task_status_is_done(&task.status) {
                continue;
            }
            task.status = TASK_STATUS_BLOCKED.to_string();
            task.error = Some(message.clone());
            task.summary = message.clone();
            task.completed_at = None;
        }
    }
    run.status = TASK_STATUS_BLOCKED.to_string();
    run.active = false;
    run.loop_started = false;
    run.auto_run_enabled = false;
    run.pause_requested = false;
    run.current_task_id = None;
    run.current_task_title.clear();
    run.current_task_status.clear();
    run.current_phase = Some(PLANNING_ERROR_PHASE.to_string());
    run.phase_started_at = Some(now);
    run.phase_details = Some(json!({
        "kind": kind,
        "error": message,
        "affectedTaskIds": affected,
        "resolution": "Resolve the planning conflict, then regenerate or approve the affected plan.",
    }));
    run.pause_reason = Some(message.clone());
    run.append_log(format!("Blocked board on {kind} planning error: {message}"));
}

fn planning_error_conflict(
    run: &mut AgenticBoard,
    affected_task_ids: &[String],
    kind: &str,
    issue: impl Into<String>,
) -> ServerError {
    let issue = issue.into();
    mark_planning_error(run, affected_task_ids, kind, &issue);
    ServerError::new(StatusCode::CONFLICT, issue)
}

fn dependency_waiting_tasks(run: &AgenticBoard) -> Vec<String> {
    run.tasks
        .iter()
        .filter(|task| task_is_runnable_in_board(run, task))
        .filter(|task| !unmet_task_dependencies(run, task).is_empty())
        .map(|task| task.id.clone())
        .collect()
}

fn reconcile_dependency_statuses(run: &mut AgenticBoard) {
    let candidates = run
        .tasks
        .iter()
        .filter(|task| task_is_executable(task))
        .filter(|task| canonical_task_status(&task.status) == TASK_STATUS_BLOCKED)
        .filter(|task| task.error.as_deref().is_some_and(is_dependency_block_error))
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    for task_id in candidates {
        let Some(task) = run.tasks.iter().find(|task| task.id == task_id).cloned() else {
            continue;
        };
        if unmet_task_dependencies(run, &task).is_empty()
            && let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id)
        {
            task.status = TASK_STATUS_TODO.to_string();
            task.error = None;
            task.completed_at = None;
            run.append_log(format!(
                "Unblocked subtask {task_id}; all dependencies are complete"
            ));
        }
    }
}

fn has_hierarchical_attention_tasks(run: &AgenticBoard) -> bool {
    run.tasks.iter().any(|task| {
        task_is_executable(task)
            && matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
            )
            && !task.error.as_deref().is_some_and(is_dependency_block_error)
    })
}

fn has_dependency_blocked_tasks(run: &AgenticBoard) -> bool {
    run.tasks.iter().any(|task| {
        task_is_executable(task)
            && canonical_task_status(&task.status) == TASK_STATUS_BLOCKED
            && task.error.as_deref().is_some_and(is_dependency_block_error)
    })
}

fn hierarchical_work_is_complete(run: &AgenticBoard) -> bool {
    let executable = run
        .tasks
        .iter()
        .filter(|task| task_is_executable(task))
        .collect::<Vec<_>>();
    if executable.is_empty() {
        return run
            .tasks
            .iter()
            .filter(|task| !task.backlog_generation_task)
            .all(|task| {
                task_is_done(task)
                    || task_status_is_backlog(&task.status)
                    || !task_is_executable(task)
            })
            && run
                .tasks
                .iter()
                .any(|task| task_status_is_done(&task.status));
    }
    let approved = executable
        .iter()
        .filter(|task| !task_status_is_backlog(&task.status))
        .collect::<Vec<_>>();
    !approved.is_empty() && approved.iter().all(|task| task_is_done(task))
}

fn mark_dependency_blockers(run: &mut AgenticBoard) {
    let waiting = run
        .tasks
        .iter()
        // A missing dependency is a planning error even when the parent is
        // still waiting for approval. Keep the child explicitly blocked so
        // it cannot look like dormant approved work after the parent moves.
        .filter(|task| task_is_executable(task))
        .filter(|task| task_status_is_todo(&task.status))
        .map(|task| (task.id.clone(), unmet_task_dependencies(run, task)))
        .filter(|(_, dependencies)| !dependencies.is_empty())
        .collect::<Vec<_>>();
    for (task_id, dependencies) in waiting {
        let reason = dependency_block_reason(run, &dependencies);
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
            task.status = TASK_STATUS_BLOCKED.to_string();
            task.error = Some(reason);
        }
    }
}

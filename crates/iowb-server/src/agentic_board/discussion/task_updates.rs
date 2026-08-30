fn edit_backlog_task(
    run: &mut AgenticBoard,
    task_id: &str,
    request: &UpdateTaskRequest,
) -> Result<()> {
    let task = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if task_status_is_done(&task.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done items are immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    validate_parent_scope_not_completed(run, task.hierarchy.parent_id.as_deref())?;
    if request.parent_id.is_some() {
        let requested_parent_id = trim_string(request.parent_id.clone());
        validate_parent_scope_not_completed(run, requested_parent_id.as_deref())?;
    }
    if !task_scope_owner_is_backlog(run, task_id) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Only Backlog items or items owned by a Backlog scope can be edited. Move the scope back to Backlog first.",
        ));
    }
    let mut scope_changed = false;
    if request.title.is_some()
        || request.details.is_some()
        || request.description.is_some()
        || request.kind.is_some()
        || request.task_type.is_some()
        || request.level.is_some()
        || request.parent_id.is_some()
        || request.acceptance_criteria.is_some()
        || request.acceptance.is_some()
        || request.criteria.is_some()
        || request.blocked_by.is_some()
        || request.depends_on.is_some()
        || request.dependencies.is_some()
        || request.required.is_some()
        || request.planned_files.is_some()
        || request.side_effects.is_some()
    {
        scope_changed = true;
        delete_generated_descendants_for_scope_change(run, task_id)?;
    }
    let index = run
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task was removed while editing"))?;
    let task = run.tasks.get_mut(index).expect("task index checked");
    if let Some(title) = request
        .title
        .clone()
        .and_then(|value| trim_string(Some(value)))
    {
        task.title = title;
    }
    if let Some(details) = request
        .details
        .clone()
        .or_else(|| request.description.clone())
        .and_then(|value| trim_string(Some(value)))
    {
        task.details = details.clone();
        task.description = details.clone();
        task.prompt = details;
    }
    if let Some(kind) = request
        .kind
        .as_deref()
        .or(request.task_type.as_deref())
        .and_then(normalized_task_kind_name)
    {
        task.task_type = kind.to_string();
    }
    if let Some(level) = request.level.as_deref() {
        task.hierarchy.level = normalize_task_level(Some(level), TASK_LEVEL_STORY).to_string();
    }
    if request.parent_id.is_some() {
        task.hierarchy.parent_id = trim_string(request.parent_id.clone());
    }
    if let Some(criteria) = request
        .acceptance_criteria
        .clone()
        .or_else(|| request.acceptance.clone())
        .or_else(|| request.criteria.clone())
    {
        task.acceptance_criteria = value_to_strings(Some(criteria));
    }
    if let Some(priority) = request.priority.as_deref() {
        task.priority = normalize_priority(Some(priority)).to_string();
    }
    if let Some(rank) = request.rank {
        task.hierarchy.rank = rank;
    }
    let dependencies = request
        .blocked_by
        .clone()
        .or_else(|| request.depends_on.clone())
        .or_else(|| request.dependencies.clone())
        .map(|value| value_to_strings(Some(value)))
        .unwrap_or_else(|| task_blockers(task));
    task.hierarchy.blocked_by = dedupe_strings(dependencies.clone());
    task.depends_on = task.hierarchy.blocked_by.clone();
    if let Some(required) = request.required {
        task.hierarchy.required = required;
    }
    if let Some(files) = request.planned_files.clone() {
        task.hierarchy.planned_files = value_to_strings(Some(files));
    }
    if let Some(side_effects) = request.side_effects.clone() {
        task.hierarchy.side_effects = value_to_strings(Some(side_effects));
        task.hierarchy.side_effects_approved = false;
        task.hierarchy.side_effect_approval = None;
        task.hierarchy.side_effect_evidence.clear();
    }
    if task.hierarchy.parent_id.is_none()
        && matches!(task_level(task), TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK)
    {
        task.hierarchy.level = TASK_LEVEL_STORY.to_string();
    }
    task.hierarchy.executable = task_level(task) == TASK_LEVEL_SUBTASK;
    if scope_changed {
        task.hierarchy.scope_version = task.hierarchy.scope_version.saturating_add(1).max(1);
        task.error = None;
        task.result = None;
        task.result_validation = None;
        task.evidence.clear();
        task.hierarchy.side_effects_approved = false;
        task.hierarchy.side_effect_approval = None;
        task.hierarchy.side_effect_evidence.clear();
        task.hierarchy.research_accepted = false;
        task.hierarchy.research_acceptance = None;
        task.remaining_issues.clear();
        task.completed_at = None;
        task.hierarchy.discussion.push(json!({
            "kind": "scope_edit",
            "updatedAt": Utc::now(),
        }));
    }
    let updated_task = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .expect("edited task remains on the board");
    validate_task_dependency_references(run, &updated_task)?;
    if let Some(cycle) = dependency_cycle(run) {
        let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
        return Err(planning_error_conflict(run, &cycle, "dependency", issue));
    }
    if let Some(issue) = hierarchy_validation_issues(run).into_iter().next() {
        let affected = planning_error_task_ids(run, &issue);
        return Err(planning_error_conflict(run, &affected, "hierarchy", issue));
    }
    refresh_hierarchy_rollups(run);
    run.append_log(format!("Edited Backlog item {task_id}"));
    Ok(())
}

fn edit_task_priority_rank(
    run: &mut AgenticBoard,
    task_id: &str,
    request: &UpdateTaskRequest,
) -> Result<()> {
    let task_snapshot = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    let owner_done = top_level_parent_id(run, task_id)
        .map(str::to_string)
        .and_then(|owner_id| {
            run.tasks
                .iter()
                .find(|candidate| candidate.id == owner_id)
                .map(|owner| task_status_is_done(&owner.status))
        })
        .unwrap_or(false);
    if task_status_is_done(&task_snapshot.status) || owner_done {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done items are immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    let task = run
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if let Some(priority) = request.priority.as_deref() {
        task.priority = normalize_priority(Some(priority)).to_string();
    }
    if let Some(rank) = request.rank {
        task.hierarchy.rank = rank.max(0);
    }
    run.append_log(format!("Updated priority/rank for board task {task_id}"));
    Ok(())
}

fn update_task_status(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    task_ids: &[String],
    status: &str,
) -> Result<Json<Value>> {
    let status = normalize_task_status(Some(status), "")?;
    if !matches!(status.as_str(), TASK_STATUS_BACKLOG | TASK_STATUS_TODO) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "User status changes may only move items between Backlog and Todo.",
        ));
    }
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, board_id)?;
    let matching_ids = stored
        .board
        .tasks
        .iter()
        .filter(|task| {
            task_ids.iter().any(|id| id == &task.id)
                || (task_ids.is_empty()
                    && status == TASK_STATUS_TODO
                    && matches!(
                        canonical_task_status(&task.status),
                        TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
                    ))
        })
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    if matching_ids.is_empty() {
        return Err(not_found("Agentic board or task not found"));
    }
    let failed_ids = stored
        .board
        .tasks
        .iter()
        .filter(|task| {
            matching_ids.iter().any(|id| id == &task.id)
                && canonical_task_status(&task.status) == TASK_STATUS_FAILED
        })
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    if status == TASK_STATUS_TODO && !failed_ids.is_empty() {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "Failed item(s) {} require an explicit transient retry or a completed approved fix plan; they cannot be moved directly to Todo.",
                failed_ids.join(", ")
            ),
        ));
    }
    if status == TASK_STATUS_TODO && uses_hierarchical_orchestration(&stored.board) {
        if stored.board.tasks.iter().any(|task| {
            matching_ids.iter().any(|id| id == &task.id)
                && !task_ancestors_are_approved(&stored.board, task)
        }) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "Approve the parent planning item before moving a nested item to Todo.",
            ));
        }
        if let Some(task) = stored.board.tasks.iter().find(|task| {
            matching_ids.iter().any(|id| id == &task.id) && !task_side_effects_are_approved(task)
        }) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                task_side_effect_block_reason(task),
            ));
        }
    }
    if status == TASK_STATUS_BACKLOG {
        for task_id in &matching_ids {
            if let Some(task) = stored.board.tasks.iter().find(|task| task.id == *task_id)
                && task_status_is_done(&task.status)
            {
                return Err(ServerError::new(
                    StatusCode::CONFLICT,
                    "Done items are immutable. Create a linked revision, fix, research, or replacement item.",
                ));
            }
            delete_generated_descendants_for_scope_change(&mut stored.board, task_id)?;
        }
    }
    if matching_ids.iter().any(|task_id| {
        stored
            .board
            .tasks
            .iter()
            .find(|task| task.id == *task_id)
            .is_some_and(|task| task_status_is_done(&task.status))
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done items are immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    if matching_ids.iter().any(|task_id| {
        top_level_parent_id(&stored.board, task_id)
            .and_then(|owner_id| stored.board.tasks.iter().find(|task| task.id == owner_id))
            .is_some_and(|owner| task_status_is_done(&owner.status))
            && stored
                .board
                .tasks
                .iter()
                .find(|task| task.id == *task_id)
                .is_none_or(|task| !task_ancestors_are_approved(&stored.board, task))
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done item scope is immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    let mut updated = 0usize;
    for task in &mut stored.board.tasks {
        if matching_ids.iter().any(|id| id == &task.id) {
            if task_status_is_active(&task.status) {
                return Err(ServerError::new(
                    StatusCode::CONFLICT,
                    "Pause the active board before moving an in-progress item.",
                ));
            }
            task.status = status.clone();
            task.error = None;
            if matches!(status.as_str(), TASK_STATUS_TODO | TASK_STATUS_BACKLOG) {
                task.started_at = None;
                task.completed_at = None;
                task.provider_session_id = None;
            }
            if status == TASK_STATUS_BACKLOG {
                // Backlog is a fresh approval boundary. A previous external
                // side-effect approval must never carry across that boundary.
                clear_backlog_approval(task);
            }
            updated += 1;
        }
    }
    if updated == 0 {
        return Err(not_found("Agentic board or task not found"));
    }
    refresh_hierarchy_rollups(&mut stored.board);
    if let Some(cycle) = dependency_cycle(&stored.board) {
        let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
        let error = planning_error_conflict(&mut stored.board, &cycle, "dependency", issue);
        stored.board.touch();
        save_board(state, &stored.board)?;
        return Err(error);
    }
    if let Some(issue) = hierarchy_validation_issues(&stored.board)
        .into_iter()
        .next()
    {
        let affected = planning_error_task_ids(&stored.board, &issue);
        let error = planning_error_conflict(&mut stored.board, &affected, "hierarchy", issue);
        stored.board.touch();
        save_board(state, &stored.board)?;
        return Err(error);
    }
    stored
        .board
        .append_log(format!("Moved {updated} board task(s) to {status}"));
    stored.board.touch();
    save_board(state, &stored.board)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

fn research_proposal_items(task: &BoardTask, requested: Option<Value>) -> Result<Vec<Value>> {
    if let Some(value) = requested {
        if value.is_null() {
            return Ok(Vec::new());
        }
        if let Some(items) = value.as_array() {
            return Ok(items.clone());
        }
        if let Some(items) = value.get("items").and_then(Value::as_array) {
            return Ok(items.clone());
        }
        return Err(bad_request(
            "Research acceptance items must be an array of proposed planning items.",
        ));
    }
    for key in [
        "proposedPlanningItems",
        "proposedItems",
        "planningItems",
        "suggestedBacklogTasks",
    ] {
        if let Some(items) = task
            .result
            .as_ref()
            .and_then(|result| result.get(key))
            .and_then(Value::as_array)
        {
            return Ok(items.clone());
        }
    }
    Ok(Vec::new())
}

fn append_research_planning_items(
    run: &mut AgenticBoard,
    research: &BoardTask,
    items: Vec<Value>,
) -> Result<usize> {
    let mut seen_scope_keys = run
        .tasks
        .iter()
        .filter(|task| task_parent_id(task).is_none())
        .filter(|task| {
            matches!(
                task_level(task),
                TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC | TASK_LEVEL_STORY
            )
        })
        .map(|task| research_planning_scope_key(task_level(task), &task.title))
        .collect::<BTreeSet<_>>();
    let mut created = 0usize;
    for (index, mut item) in items.into_iter().enumerate() {
        let inherits_priority = item
            .get("priority")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_none_or(str::is_empty);
        if let Some(object) = item.as_object_mut() {
            object.remove("parentId");
            object.remove("parent_id");
            object.remove("executable");
        }
        let Some(mut planning) = task_from_json(run, item, index, TASK_STATUS_BACKLOG) else {
            continue;
        };
        if inherits_priority {
            planning.priority = research.priority.clone();
        }
        let title_key = normalize_suggested_task_key(&planning.title);
        let planning_level = match task_level(&planning) {
            TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC | TASK_LEVEL_STORY => task_level(&planning),
            _ => TASK_LEVEL_STORY,
        };
        let scope_key = research_planning_scope_key(planning_level, &planning.title);
        if title_key.is_empty() || !seen_scope_keys.insert(scope_key) {
            continue;
        }
        planning.id = unique_task_id(run, &format!("research-{}", planning.id));
        planning.status = TASK_STATUS_BACKLOG.to_string();
        planning.hierarchy.level = planning_level.to_string();
        planning.hierarchy.parent_id = None;
        planning.hierarchy.executable = false;
        planning.hierarchy.scope_version = research.hierarchy.scope_version.saturating_add(1);
        planning.group_id = Some(planning.id.clone());
        planning.task_origin = "research_accepted".to_string();
        planning.references.insert(
            0,
            format!(
                "Accepted research output from {}: {}",
                research.id, research.title
            ),
        );
        planning.prompt = planning.description.clone();
        run.tasks.push(planning);
        created += 1;
    }
    Ok(created)
}

fn research_planning_scope_key(level: &str, title: &str) -> String {
    format!("{}|{}", level, normalize_suggested_task_key(title))
}

fn accept_research_in_board(
    run: &mut AgenticBoard,
    task_id: &str,
    user_id: &str,
    requested_items: Option<Value>,
    note: Option<String>,
) -> Result<()> {
    let research = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| not_found("Research task not found"))?;
    if canonical_task_kind(&research) != TASK_KIND_RESEARCH {
        return Err(bad_request(
            "Only research subtasks can be accepted as research.",
        ));
    }
    if !task_status_is_done(&research.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Research output can only be accepted after the research subtask is done.",
        ));
    }
    if research.hierarchy.research_accepted {
        return Ok(());
    }
    let items = research_proposal_items(&research, requested_items)?;
    let created = append_research_planning_items(run, &research, items)?;
    if let Some(issue) = hierarchy_validation_issues(run).into_iter().next() {
        let affected = planning_error_task_ids(run, &issue);
        return Err(planning_error_conflict(run, &affected, "hierarchy", issue));
    }
    let audit = json!({
        "acceptedAt": Utc::now(),
        "acceptedBy": user_id,
        "note": note,
        "createdItemCount": created,
    });
    if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
        task.hierarchy.research_accepted = true;
        task.hierarchy.research_acceptance = Some(audit);
        task.error = None;
    }
    refresh_hierarchy_rollups(run);
    run.append_log(format!(
        "Accepted research output for {task_id}; created {created} Backlog planning item(s)"
    ));
    Ok(())
}

fn detach_user_created_child(run: &mut AgenticBoard, task_id: &str) -> Result<()> {
    let snapshot = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    let previous_parent = task_parent_id(&snapshot)
        .map(str::to_string)
        .ok_or_else(|| bad_request("Only nested children can be detached."))?;
    validate_parent_scope_not_completed(run, Some(previous_parent.as_str()))?;
    if !snapshot.manual_task && snapshot.task_origin != "user_manual" {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Only user-created children can be detached and preserved.",
        ));
    }
    if !task_status_is_backlog(&snapshot.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Move the user-created child to Backlog before detaching it.",
        ));
    }
    if !task_scope_owner_is_backlog(run, task_id) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Only children in a Backlog scope can be detached.",
        ));
    }
    let task = run
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .expect("task snapshot came from board");
    task.hierarchy.parent_id = None;
    task.hierarchy.level = TASK_LEVEL_STORY.to_string();
    task.hierarchy.executable = false;
    task.hierarchy.scope_version = task.hierarchy.scope_version.saturating_add(1).max(1);
    task.hierarchy
        .blocked_by
        .retain(|id| id != &previous_parent);
    task.depends_on.retain(|id| id != &previous_parent);
    task.group_id = Some(task.id.clone());
    task.hierarchy.discussion.push(json!({
        "kind": "detached_user_child",
        "previousParentId": previous_parent.clone(),
        "updatedAt": Utc::now(),
    }));
    task.references
        .push(format!("Detached from parent {previous_parent}"));
    run.append_log(format!(
        "Detached user-created child {task_id} into a preserved Backlog story"
    ));
    // The detached item may have been a Task with a Subtask child. Hierarchy
    // normalization converges parent levels before this response is built,
    // preserving the complete Story -> Task -> Subtask chain immediately.
    normalize_board_hierarchy(run);
    normalize_board_task_groups(run);
    if let Some(issue) = hierarchy_validation_issues(run).into_iter().next() {
        let affected = planning_error_task_ids(run, &issue);
        return Err(planning_error_conflict(run, &affected, "hierarchy", issue));
    }
    refresh_hierarchy_rollups(run);
    Ok(())
}

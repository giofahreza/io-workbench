fn apply_discussion_action(
    run: &mut AgenticBoard,
    task_id: &str,
    action: &str,
    payload: &Value,
) -> Result<()> {
    if discussion_action_requires_backlog(action) && !task_scope_owner_is_backlog(run, task_id) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Only Backlog scope can be changed. Move the scope owner back to Backlog first.",
        ));
    }
    match action {
        "message" => Ok(()),
        "edit" | "replace" => {
            let patch = serde_json::from_value::<UpdateTaskRequest>(payload.clone())
                .map_err(|error| bad_request(format!("Invalid discussion edit: {error}")))?;
            edit_backlog_task(run, task_id, &patch)
        }
        "reprioritize" => {
            let priority = payload
                .get("priority")
                .and_then(Value::as_str)
                .ok_or_else(|| bad_request("Reprioritize proposal must contain priority."))?;
            edit_task_priority_rank(
                run,
                task_id,
                &UpdateTaskRequest {
                    priority: Some(priority.to_string()),
                    ..UpdateTaskRequest::default()
                },
            )
        }
        "delete" => delete_board_task(run, task_id),
        "regenerate_children" => {
            delete_generated_descendants_for_scope_change(run, task_id)?;
            refresh_hierarchy_rollups(run);
            Ok(())
        }
        "split" => split_discussion_item(run, task_id, payload),
        "merge" => merge_discussion_item(run, task_id, payload),
        "re_research" | "research" | "revision" | "fix" | "replacement" => {
            append_linked_planning_item(run, task_id, action, payload)
        }
        _ => Err(bad_request(format!(
            "Unsupported discussion action: {action}"
        ))),
    }
}

fn split_discussion_item(run: &mut AgenticBoard, parent_id: &str, payload: &Value) -> Result<()> {
    let parent = run
        .tasks
        .iter()
        .find(|task| task.id == parent_id)
        .cloned()
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if !task_status_is_backlog(&parent.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Items can only be split while they are in Backlog.",
        ));
    }
    let Some(next_level) = next_hierarchy_level(task_level(&parent)) else {
        return Err(bad_request(
            "A subtask cannot be split into another hierarchy level.",
        ));
    };
    let items = payload
        .get("items")
        .or_else(|| payload.get("children"))
        .and_then(Value::as_array)
        .ok_or_else(|| bad_request("Discussion split requires an items array."))?;
    if items.is_empty() {
        return Err(bad_request(
            "Discussion split requires at least one child item.",
        ));
    }
    let snapshot = run.clone();
    let group_id = task_group_id_or_self(&parent);
    let mut children = Vec::new();
    for (index, item) in items.iter().cloned().enumerate() {
        let mut item = item;
        if let Some(object) = item.as_object_mut() {
            object.insert("level".to_string(), json!(next_level));
            object.insert("parentId".to_string(), json!(parent.id));
            object.insert("status".to_string(), json!(TASK_STATUS_BACKLOG));
        }
        let Some(mut child) = task_from_json(&snapshot, item, index, TASK_STATUS_BACKLOG) else {
            continue;
        };
        child.id = unique_task_id(run, &format!("{}-split-{}", parent.id, index + 1));
        child.hierarchy.level = next_level.to_string();
        child.hierarchy.parent_id = Some(parent.id.clone());
        child.hierarchy.executable = next_level == TASK_LEVEL_SUBTASK;
        child.hierarchy.scope_version = parent.hierarchy.scope_version.saturating_add(1);
        child.group_id = Some(group_id.clone());
        child.task_origin = "discussion_split".to_string();
        child.prompt = child.description.clone();
        children.push(child);
    }
    if children.is_empty() {
        return Err(bad_request(
            "Discussion split did not contain usable child items.",
        ));
    }

    // Validate the complete proposed tree before mutating the live board. A
    // split is a planning action, so malformed hierarchy, contradictory
    // acceptance criteria, or a dependency cycle must not leave half of the
    // proposed children applied.
    let mut candidate = run.clone();
    candidate.tasks.extend(children.iter().cloned());
    if let Some(cycle) = dependency_cycle(&candidate) {
        let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
        return Err(planning_error_conflict(
            run,
            std::slice::from_ref(&parent.id),
            "dependency",
            issue,
        ));
    }
    if let Some(issue) = hierarchy_validation_issues(&candidate).into_iter().next() {
        return Err(planning_error_conflict(
            run,
            std::slice::from_ref(&parent.id),
            "hierarchy",
            issue,
        ));
    }
    run.tasks.extend(children);
    refresh_hierarchy_rollups(run);
    Ok(())
}

fn merge_discussion_item(run: &mut AgenticBoard, source_id: &str, payload: &Value) -> Result<()> {
    let target_id = payload
        .get("targetId")
        .or_else(|| payload.get("target_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| bad_request("Discussion merge requires targetId."))?;
    if source_id == target_id {
        return Err(bad_request("A task cannot be merged into itself."));
    }
    let source = run
        .tasks
        .iter()
        .find(|task| task.id == source_id)
        .cloned()
        .ok_or_else(|| not_found("Source task not found"))?;
    let target = run
        .tasks
        .iter()
        .find(|task| task.id == target_id)
        .cloned()
        .ok_or_else(|| not_found("Target task not found"))?;
    if !task_status_is_backlog(&source.status) || !task_status_is_backlog(&target.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Both items must be in Backlog before they can be merged.",
        ));
    }
    delete_generated_descendants_for_scope_change(run, target_id)?;
    let merged_details = format!(
        "{}\n\nMerged scope from {}:\n{}",
        target.details, source.id, source.details
    );
    let patch = UpdateTaskRequest {
        details: Some(merged_details),
        acceptance_criteria: Some(json!(
            target
                .acceptance_criteria
                .into_iter()
                .chain(source.acceptance_criteria)
                .collect::<Vec<_>>()
        )),
        ..UpdateTaskRequest::default()
    };
    edit_backlog_task(run, target_id, &patch)?;
    delete_board_task(run, source_id)
}

fn append_linked_planning_item(
    run: &mut AgenticBoard,
    source_id: &str,
    requested_kind: &str,
    payload: &Value,
) -> Result<()> {
    let source = run
        .tasks
        .iter()
        .find(|task| task.id == source_id)
        .cloned()
        .ok_or_else(|| not_found("Source task not found"))?;
    let kind = match requested_kind {
        "re_research" | "research" => TASK_KIND_RESEARCH,
        "revision" => TASK_KIND_REVISION,
        "fix" => TASK_KIND_FIX,
        "replacement" => TASK_KIND_REPLACEMENT,
        other => {
            return Err(bad_request(format!(
                "Unsupported linked planning item kind: {other}"
            )));
        }
    };
    let supersede_source = kind == TASK_KIND_REPLACEMENT
        && payload
            .get("supersedeSource")
            .or_else(|| payload.get("supersede_source"))
            .and_then(Value::as_bool)
            == Some(true);
    if supersede_source {
        if !task_status_is_done(&source.status) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "Only a completed item can be marked superseded.",
            ));
        }
        if source.superseded_by.is_some() {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "This completed item is already superseded by a linked replacement.",
            ));
        }
    }
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(match kind {
            TASK_KIND_RESEARCH => "Research a revised direction",
            TASK_KIND_REVISION => "Revise the completed item",
            TASK_KIND_FIX => "Fix the completed item",
            TASK_KIND_REPLACEMENT => "Replace the completed item",
            _ => "Continue the completed item",
        });
    let details = payload
        .get("details")
        .or_else(|| payload.get("description"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&source.details);
    let default_acceptance = match kind {
        TASK_KIND_RESEARCH => "Record evidence and a proposed direction for review.",
        TASK_KIND_REVISION => "Implement the revised scope without reopening the completed item.",
        TASK_KIND_FIX => "Verify that the defect in the completed item is resolved.",
        TASK_KIND_REPLACEMENT => "Implement and verify the replacement behavior.",
        _ => "Complete the linked planning item.",
    };
    let item = json!({
        "id": unique_task_id(run, "research"),
        "title": title,
        "level": TASK_LEVEL_STORY,
        "kind": kind,
        "sourceTaskId": source_id,
        "details": details,
        "description": details,
        "acceptanceCriteria": payload.get("acceptanceCriteria").cloned().unwrap_or_else(|| json!([default_acceptance])),
        "references": [format!("Related item: {source_id}")],
        "priority": source.priority,
        "status": TASK_STATUS_BACKLOG,
    });
    let Some(mut linked) = task_from_json(run, item, run.tasks.len(), TASK_STATUS_BACKLOG) else {
        return Err(bad_request("Linked planning item could not be created."));
    };
    linked.id = unique_task_id(run, kind);
    let linked_id = linked.id.clone();
    linked.group_id = Some(linked.id.clone());
    linked.task_origin = format!("discussion_{kind}");
    linked.hierarchy.level = TASK_LEVEL_STORY.to_string();
    linked.hierarchy.executable = false;
    run.tasks.push(linked);
    if supersede_source {
        let source = run
            .tasks
            .iter_mut()
            .find(|task| task.id == source_id)
            .expect("source task came from board");
        source.superseded_by = Some(linked_id.clone());
        source
            .references
            .push(format!("Superseded by linked replacement {linked_id}"));
        source.hierarchy.discussion.push(json!({
            "kind": "superseded",
            "replacementId": linked_id,
            "updatedAt": Utc::now(),
        }));
    }
    Ok(())
}

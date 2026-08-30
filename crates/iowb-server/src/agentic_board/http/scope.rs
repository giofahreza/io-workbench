async fn delete_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    delete_board_task(&mut stored.board, &task_id)?;
    stored.board.append_log("Deleted board task");
    stored.board.touch();
    save_board(&state, &stored.board)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

fn delete_board_task(run: &mut AgenticBoard, task_id: &str) -> Result<()> {
    let ids = descendant_task_ids(run, task_id);
    if ids.is_empty() {
        return Err(not_found("Agentic board or backlog task not found"));
    }
    if ids
        .iter()
        .any(|id| run.current_task_id.as_deref() == Some(id))
        || run
            .tasks
            .iter()
            .any(|task| ids.contains(&task.id) && task_status_is_active(&task.status))
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "An executing task or descendant cannot be deleted. Pause the board first.",
        ));
    }
    if run
        .tasks
        .iter()
        .any(|task| ids.contains(&task.id) && task_status_is_done(&task.status))
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done items are immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    if ids.iter().any(|id| {
        top_level_parent_id(run, id)
            .and_then(|owner_id| run.tasks.iter().find(|task| task.id == owner_id))
            .is_some_and(|owner| task_status_is_done(&owner.status))
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done item scope is immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    if run
        .tasks
        .iter()
        .any(|task| ids.contains(&task.id) && task_has_recorded_effects(task))
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Deleted work has recorded code or external effects. Choose keep changes, create a revert subtask, or create a cleanup subtask.",
        ));
    }
    let missing_plan_parents = run
        .tasks
        .iter()
        .filter(|task| ids.contains(&task.id) && task.hierarchy.required)
        .filter_map(|task| task_parent_id(task).map(str::to_string))
        .filter(|parent_id| !ids.contains(parent_id))
        .collect::<BTreeSet<_>>();
    run.tasks.retain(|task| !ids.contains(&task.id));
    mark_deleted_dependencies(run, &ids);
    for parent_id in missing_plan_parents {
        if let Some(parent) = run.tasks.iter_mut().find(|task| task.id == parent_id) {
            let reason = format!(
                "Missing required plan: child {task_id} was deleted. Regenerate, replace, or explicitly remove this scope."
            );
            if !task_status_is_backlog(&parent.status) && !task_status_is_done(&parent.status) {
                parent.status = TASK_STATUS_BLOCKED.to_string();
            }
            if !task_status_is_done(&parent.status) {
                parent.error = Some(reason);
                parent.completed_at = None;
            }
        }
    }
    run.append_log(format!(
        "Deleted board item {task_id} and {} generated descendant(s)",
        ids.len().saturating_sub(1)
    ));
    Ok(())
}

fn task_parent_id(task: &BoardTask) -> Option<&str> {
    task.hierarchy.parent_id.as_deref().or_else(|| {
        // Older system-generated QA/follow-up cards used a source link as
        // their structural parent. User-created links (especially a fix
        // linked to a failed subtask) are references, not hierarchy edges.
        if !source_link_is_structural(task) {
            return None;
        }
        task.source_task_id
            .as_deref()
            .or(task.source_qa_task_id.as_deref())
    })
}

fn source_link_is_structural(task: &BoardTask) -> bool {
    task.qa_task
        || task.final_qa_task
        || task.followup_task
        || task.qa_fix_task
        || task.qa_verdict_retry_task
        || task.task_level_qa
        || task.agents_knowledge_task
        || task.source_qa_task_id.is_some()
}

fn task_scope_chain_ids(run: &AgenticBoard, task_id: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut current_id = Some(task_id.to_string());
    let mut visited = BTreeSet::new();
    while let Some(id) = current_id {
        if !visited.insert(id.clone()) {
            break;
        }
        let Some(task) = run.tasks.iter().find(|task| task.id == id) else {
            break;
        };
        ids.push(id);
        current_id = task_parent_id(task).map(str::to_string);
    }
    ids
}

fn top_level_parent_id<'a>(run: &'a AgenticBoard, task_id: &str) -> Option<&'a str> {
    let mut current_id = task_id;
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(current_id.to_string()) {
            return None;
        }
        let task = run.tasks.iter().find(|task| task.id == current_id)?;
        let Some(parent_id) = task_parent_id(task) else {
            return Some(task.id.as_str());
        };
        current_id = parent_id;
    }
}

fn descendant_task_ids(run: &AgenticBoard, root_id: &str) -> BTreeSet<String> {
    if !run.tasks.iter().any(|task| task.id == root_id) {
        return BTreeSet::new();
    }
    let mut ids = BTreeSet::from([root_id.to_string()]);
    loop {
        let before = ids.len();
        for task in &run.tasks {
            if task_parent_id(task).is_some_and(|parent| ids.contains(parent)) {
                ids.insert(task.id.clone());
            }
        }
        if ids.len() == before {
            break;
        }
    }
    ids
}

fn delete_generated_descendants_for_scope_change(
    run: &mut AgenticBoard,
    root_id: &str,
) -> Result<usize> {
    let ids = descendant_task_ids(run, root_id);
    if ids.len() <= 1 {
        return Ok(0);
    }
    if ids.iter().any(|id| {
        run.tasks.iter().any(|task| {
            task.id == *id
                && (task_status_is_active(&task.status)
                    || task.id == run.current_task_id.as_deref().unwrap_or_default())
        })
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Pause the board before changing a parent with executing children.",
        ));
    }
    if ids.iter().any(|id| {
        run.tasks
            .iter()
            .any(|task| task.id == *id && task_has_recorded_effects(task))
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "Generated children already changed code or external state. Choose keep changes, create a revert subtask, or create a cleanup subtask, then retry the scope change. Resolve this with the scope-effects action for {root_id}."
            ),
        ));
    }
    let removed = ids.len() - 1;
    let removed_ids = ids
        .iter()
        .filter(|id| id.as_str() != root_id)
        .cloned()
        .collect::<BTreeSet<_>>();
    run.tasks
        .retain(|task| !ids.contains(&task.id) || task.id == root_id);
    // The root remains part of the board. Only dependencies on descendants
    // that were actually removed should become missing dependencies.
    mark_deleted_dependencies(run, &removed_ids);
    Ok(removed)
}

fn scope_effect_descendants(run: &AgenticBoard, root_id: &str) -> Vec<BoardTask> {
    let ids = descendant_task_ids(run, root_id);
    run.tasks
        .iter()
        .filter(|task| ids.contains(&task.id) && task.id != root_id)
        .filter(|task| task_has_recorded_effects(task))
        .cloned()
        .collect()
}

fn task_has_recorded_effects(task: &BoardTask) -> bool {
    !task.changed_files.is_empty() || !task.hierarchy.side_effect_evidence.is_empty()
}

fn append_scope_effect_action(
    run: &mut AgenticBoard,
    root: &BoardTask,
    affected: &[BoardTask],
    kind: &str,
) -> Result<Vec<String>> {
    let action = if kind == TASK_KIND_REVERT {
        "Revert"
    } else {
        "Clean up"
    };
    let references = affected
        .iter()
        .map(|task| format!("Superseded child {}: {}", task.id, task.title))
        .collect::<Vec<_>>();
    let changed_files = affected
        .iter()
        .flat_map(|task| task.changed_files.iter().cloned())
        .collect::<Vec<_>>();
    let side_effects = affected
        .iter()
        .flat_map(|task| task.hierarchy.side_effect_evidence.iter().cloned())
        .collect::<Vec<_>>();
    let details = format!(
        "{action} recorded effects from superseded generated children of {}.\nAffected work:\n{}",
        root.title,
        affected
            .iter()
            .map(|task| format!("- {}: {}", task.id, task.title))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let mut parent_id = root.id.clone();
    let mut parent_level = task_level(root);
    let mut created = Vec::new();
    loop {
        let Some(level) = next_hierarchy_level(parent_level) else {
            return Err(bad_request(
                "Cannot create scope-effect work under an invalid hierarchy item.",
            ));
        };
        let executable = level == TASK_LEVEL_SUBTASK;
        let mut task = BoardTask::draft(run, format!("{action} superseded work"), details.clone());
        task.priority = task_priority_for_parent(run, Some(parent_id.as_str()), None);
        task.task_origin = "scope_effect_resolution".to_string();
        task.task_type = if executable {
            kind.to_string()
        } else {
            TASK_KIND_DESIGN.to_string()
        };
        task.status = TASK_STATUS_BACKLOG.to_string();
        task.references = references.clone();
        task.acceptance_criteria = vec![format!(
            "Recorded effects from the superseded generated children are {action_lower}ed without changing unrelated work.",
            action_lower = action.to_ascii_lowercase()
        )];
        task.hierarchy.level = level.to_string();
        task.hierarchy.parent_id = Some(parent_id.clone());
        task.hierarchy.executable = executable;
        task.hierarchy.required = true;
        task.hierarchy.scope_version = root.hierarchy.scope_version.saturating_add(1).max(1);
        task.hierarchy.rank = root.hierarchy.rank.saturating_add(1);
        task.hierarchy.planned_files = changed_files.clone();
        task.hierarchy.side_effects = if executable {
            dedupe_strings(side_effects.clone())
        } else {
            Vec::new()
        };
        task.group_id = Some(task_group_id_or_self(root));
        let task_id = task.id.clone();
        run.tasks.push(task);
        created.push(task_id.clone());
        if executable {
            break;
        }
        parent_id = task_id;
        parent_level = level;
    }
    Ok(created)
}

fn resolve_scope_effects_in_board(
    run: &mut AgenticBoard,
    root_id: &str,
    user_id: &str,
    decision: &str,
    note: Option<String>,
) -> Result<()> {
    if !matches!(decision, "keep" | "revert" | "cleanup") {
        return Err(bad_request(
            "Scope-effect decision must be keep, revert, or cleanup.",
        ));
    }
    let root = run
        .tasks
        .iter()
        .find(|task| task.id == root_id)
        .cloned()
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if task_status_is_done(&root.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done items are immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    let scope_chain = task_scope_chain_ids(run, root_id);
    if scope_chain.is_empty() {
        return Err(not_found("Agentic board task not found"));
    }
    if run.active
        || run.loop_started
        || run.status == "running"
        || scope_chain.iter().any(|id| {
            run.tasks.iter().any(|task| {
                task.id == *id
                    && (task_status_is_active(&task.status)
                        || task.id == run.current_task_id.as_deref().unwrap_or_default())
            })
        })
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Pause the board before resolving effects from executing scope work.",
        ));
    }
    if scope_chain.iter().any(|id| {
        run.tasks
            .iter()
            .any(|task| task.id == *id && task_status_is_done(&task.status))
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done scope is immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    // A failed attempt to demote an approved parent may have left it in Todo
    // while preserving the affected descendants so the user can decide what
    // to do with their effects. Treat this explicit resolution action as the
    // approval-clearing transition to Backlog, avoiding a dead-end workflow.
    let moved_to_backlog = scope_chain.iter().any(|id| {
        run.tasks
            .iter()
            .find(|task| task.id == *id)
            .is_some_and(|task| !task_status_is_backlog(&task.status))
    });
    if moved_to_backlog {
        for id in &scope_chain {
            if let Some(task) = run.tasks.iter_mut().find(|task| task.id == *id) {
                task.status = TASK_STATUS_BACKLOG.to_string();
                task.started_at = None;
                task.completed_at = None;
                task.provider_session_id = None;
                task.error = None;
                task.hierarchy.side_effects_approved = false;
                task.hierarchy.side_effect_approval = None;
                task.hierarchy.research_accepted = false;
                task.hierarchy.research_acceptance = None;
            }
        }
        run.append_log(format!(
            "Moved scope {root_id} back to Backlog before resolving recorded child effects"
        ));
    }
    let ids = descendant_task_ids(run, root_id);
    if ids.iter().any(|id| {
        run.tasks.iter().any(|task| {
            task.id == *id
                && (task_status_is_active(&task.status)
                    || task.id == run.current_task_id.as_deref().unwrap_or_default())
        })
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Pause the board before resolving effects from executing children.",
        ));
    }
    let affected = scope_effect_descendants(run, root_id);
    if affected.is_empty() {
        return Err(bad_request(
            "No recorded child code or external effects require a scope-effect decision.",
        ));
    }
    let affected_ids = affected
        .iter()
        .map(|task| task.id.clone())
        .collect::<BTreeSet<_>>();
    let created = if decision == "keep" {
        Vec::new()
    } else {
        append_scope_effect_action(
            run,
            &root,
            &affected,
            if decision == "revert" {
                TASK_KIND_REVERT
            } else {
                TASK_KIND_CLEANUP
            },
        )?
    };
    run.tasks.retain(|task| !affected_ids.contains(&task.id));
    mark_deleted_dependencies(run, &affected_ids);
    if let Some(root_task) = run.tasks.iter_mut().find(|task| task.id == root_id) {
        root_task.hierarchy.discussion.push(json!({
            "kind": "scope_effect_resolution",
            "decision": decision,
            "affectedTaskIds": affected_ids,
            "createdTaskIds": created,
            "resolvedAt": Utc::now(),
            "resolvedBy": user_id,
            "note": note,
        }));
    }
    run.append_log(format!(
        "Resolved superseded child effects for {root_id} with {decision}; created {} explicit cleanup task(s)",
        created.len()
    ));
    refresh_hierarchy_rollups(run);
    Ok(())
}

fn mark_deleted_dependencies(run: &mut AgenticBoard, deleted_ids: &BTreeSet<String>) {
    let affected = run
        .tasks
        .iter()
        .filter_map(|task| {
            let missing = task_blockers(task)
                .into_iter()
                .filter(|dependency| deleted_ids.contains(dependency))
                .collect::<Vec<_>>();
            (!missing.is_empty()).then(|| (task.id.clone(), missing))
        })
        .collect::<Vec<_>>();
    let affected_count = affected.len();
    for (task_id, missing) in affected {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
            if task_status_is_done(&task.status) {
                continue;
            }
            task.status = TASK_STATUS_BLOCKED.to_string();
            task.error = Some(format!("Missing dependency: {}", missing.join(", ")));
            task.completed_at = None;
        }
    }
    if affected_count > 0 {
        run.append_log(format!(
            "Marked {} dependent item(s) blocked because a dependency was deleted",
            affected_count
        ));
        refresh_hierarchy_rollups(run);
    }
}

fn task_external_effect_text(task: &BoardTask) -> String {
    [
        task.title.as_str(),
        task.details.as_str(),
        task.description.as_str(),
        task.prompt.as_str(),
        &task.acceptance_criteria.join("\n"),
        &task.references.join("\n"),
    ]
    .join("\n")
    .to_ascii_lowercase()
}

fn task_requires_external_side_effect_declaration(task: &BoardTask) -> bool {
    if canonical_task_kind(task) == TASK_KIND_MIGRATION {
        return true;
    }
    let text = task_external_effect_text(task);
    [
        "database migration",
        "db migration",
        "drop table",
        "truncate table",
        "delete data",
        "destroy data",
        "reset database",
        "production config",
        "production environment",
        "cloud resource",
        "remote api configuration",
        "remote config",
        "paid api",
        "third-party account",
        "third party account",
        "emulator data",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn task_requires_side_effect_approval(task: &BoardTask) -> bool {
    task_is_executable(task)
        && (!task.hierarchy.side_effects.is_empty()
            || task_requires_external_side_effect_declaration(task))
}

fn task_side_effects_are_approved(task: &BoardTask) -> bool {
    if !task_requires_side_effect_approval(task) {
        return true;
    }
    !task.hierarchy.side_effects.is_empty() && task.hierarchy.side_effects_approved
}

fn task_side_effect_block_reason(task: &BoardTask) -> String {
    if task.hierarchy.side_effects.is_empty() {
        return "Declare possible external side effects before approving this risky subtask."
            .to_string();
    }
    format!(
        "External side-effect approval required before running: {}",
        task.hierarchy.side_effects.join(", ")
    )
}

fn unapproved_side_effect_task_ids(run: &AgenticBoard) -> Vec<String> {
    run.tasks
        .iter()
        .filter(|task| task_status_is_todo(&task.status))
        .filter(|task| task_ancestors_are_approved(run, task))
        .filter(|task| !task_side_effects_are_approved(task))
        .map(|task| task.id.clone())
        .collect()
}

fn mark_side_effect_blockers(run: &mut AgenticBoard) {
    let blocked = run
        .tasks
        .iter()
        .filter(|task| task_status_is_todo(&task.status))
        .filter(|task| task_ancestors_are_approved(run, task))
        .filter(|task| !task_side_effects_are_approved(task))
        .map(|task| (task.id.clone(), task_side_effect_block_reason(task)))
        .collect::<Vec<_>>();
    for (task_id, reason) in blocked {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
            task.status = TASK_STATUS_BLOCKED.to_string();
            task.error = Some(reason);
            task.completed_at = None;
        }
    }
}

fn external_side_effect_evidence(parsed: &Value) -> Vec<String> {
    normalize_string_list(
        parsed
            .get("externalSideEffects")
            .or_else(|| parsed.get("external_side_effects"))
            .or_else(|| parsed.get("sideEffectsEvidence"))
            .or_else(|| parsed.get("side_effects_evidence")),
    )
}

fn manual_test_environment_evidence(parsed: &Value) -> Value {
    let Some(source) = parsed
        .get("manualTestEnvironment")
        .or_else(|| parsed.get("manual_test_environment"))
        .or_else(|| parsed.get("testEnvironment"))
        .or_else(|| parsed.get("environment"))
        .and_then(Value::as_object)
    else {
        return Value::Null;
    };
    let mut environment = serde_json::Map::new();
    for (canonical, aliases) in [
        (
            "deviceOrEmulator",
            &[
                "deviceOrEmulator",
                "device",
                "emulator",
                "simulator",
                "deviceModel",
            ][..],
        ),
        (
            "appVersion",
            &[
                "appVersion",
                "app_version",
                "version",
                "build",
                "buildVersion",
            ][..],
        ),
        (
            "backendUrl",
            &[
                "backendUrl",
                "backend_url",
                "apiUrl",
                "baseUrl",
                "serverUrl",
            ][..],
        ),
        (
            "osVersion",
            &["osVersion", "os_version", "platformVersion"][..],
        ),
    ] {
        let value = aliases
            .iter()
            .find_map(|alias| source.get(*alias))
            .and_then(value_to_trimmed_text);
        if let Some(value) = value {
            environment.insert(canonical.to_string(), json!(value));
        }
    }
    Value::Object(environment)
}

fn value_to_trimmed_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => trim_string(Some(value.clone())),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn manual_test_environment_is_complete(environment: &Value) -> bool {
    let Some(object) = environment.as_object() else {
        return false;
    };
    ["deviceOrEmulator", "appVersion", "backendUrl"]
        .iter()
        .all(|key| {
            object
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        })
}

fn approve_task_side_effects_in_board(
    run: &mut AgenticBoard,
    task_id: &str,
    user_id: &str,
    approved: bool,
    note: Option<String>,
) -> Result<()> {
    let task_snapshot = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if !task_is_executable(&task_snapshot) {
        return Err(bad_request(
            "External side-effect approval is only available for executable subtasks.",
        ));
    }
    if !task_requires_side_effect_approval(&task_snapshot) {
        return Err(bad_request(
            "This subtask has no declared or detected external side effects.",
        ));
    }
    if task_status_is_active(&task_snapshot.status) || task_status_is_done(&task_snapshot.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "External side effects can only be approved before a subtask starts or after it is blocked.",
        ));
    }
    if approved && task_snapshot.hierarchy.side_effects.is_empty() {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Declare the possible external side effects before approving this subtask.",
        ));
    }
    let audit = json!({
        "approved": approved,
        "approvedAt": Utc::now(),
        "approvedBy": user_id,
        "note": note,
        "sideEffects": task_snapshot.hierarchy.side_effects,
    });
    let task = run
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .expect("task snapshot came from board");
    let revoke_reason = (!approved && task_status_is_todo(&task.status))
        .then(|| task_side_effect_block_reason(task));
    task.hierarchy.side_effects_approved = approved;
    task.hierarchy.side_effect_approval = Some(audit);
    if approved
        && canonical_task_status(&task.status) == TASK_STATUS_BLOCKED
        && task
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("External side-effect approval required"))
    {
        task.status = TASK_STATUS_TODO.to_string();
        task.error = None;
    }
    if let Some(reason) = revoke_reason {
        task.status = TASK_STATUS_BLOCKED.to_string();
        task.error = Some(reason);
        task.completed_at = None;
    }
    run.append_log(format!(
        "{} external side effects for subtask {task_id}",
        if approved { "Approved" } else { "Rejected" }
    ));
    refresh_hierarchy_rollups(run);
    Ok(())
}

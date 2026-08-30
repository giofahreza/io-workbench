fn next_hierarchy_level(level: &str) -> Option<&'static str> {
    match level {
        TASK_LEVEL_INITIATIVE => Some(TASK_LEVEL_EPIC),
        TASK_LEVEL_EPIC => Some(TASK_LEVEL_STORY),
        TASK_LEVEL_STORY => Some(TASK_LEVEL_TASK),
        TASK_LEVEL_TASK => Some(TASK_LEVEL_SUBTASK),
        _ => None,
    }
}

fn direct_hierarchy_children<'a>(run: &'a AgenticBoard, parent_id: &str) -> Vec<&'a BoardTask> {
    run.tasks
        .iter()
        .filter(|task| task.hierarchy.parent_id.as_deref() == Some(parent_id))
        .collect()
}

fn hierarchy_breakdown_child_title_candidates(
    run: &AgenticBoard,
    parent_id: &str,
    next_level: &str,
) -> Vec<String> {
    let mut titles = direct_hierarchy_children(run, parent_id)
        .into_iter()
        .filter(|task| task_level(task) == next_level)
        .map(|task| normalize_suggested_task_key(&task.title))
        .filter(|title| !title.is_empty())
        .collect::<BTreeSet<_>>();

    // Risky or out-of-scope generated children are wrapped in an unparented
    // Backlog story. Keep their original title key tied to the source parent
    // so a later manual refinement cannot recreate the same proposal.
    for task in &run.tasks {
        if task.task_origin != "hierarchy_backlog_wrapper" {
            continue;
        }
        for entry in &task.hierarchy.discussion {
            if entry.get("kind").and_then(Value::as_str) != Some("generated_scope_wrapper")
                || entry.get("sourceParentId").and_then(Value::as_str) != Some(parent_id)
                || entry.get("wrappedLevel").and_then(Value::as_str) != Some(next_level)
            {
                continue;
            }
            if let Some(title) = entry.get("sourceTitleKey").and_then(Value::as_str) {
                let title = normalize_suggested_task_key(title);
                if !title.is_empty() {
                    titles.insert(title);
                }
            }
        }
    }

    titles.into_iter().collect()
}

fn hierarchy_breakdown_has_children(run: &AgenticBoard, parent_id: &str, next_level: &str) -> bool {
    !hierarchy_breakdown_child_title_candidates(run, parent_id, next_level).is_empty()
}

fn hierarchy_breakdown_max_new_children(existing_count: usize) -> usize {
    MAX_HIERARCHY_CHILDREN_PER_PARENT
        .saturating_sub(existing_count)
        .min(if existing_count > 0 {
            MAX_HIERARCHY_REFINEMENT_ADDITIONS
        } else {
            MAX_HIERARCHY_CHILDREN_PER_PARENT
        })
}

fn validate_hierarchy_breakdown_parent(run: &AgenticBoard, parent: &BoardTask) -> Result<()> {
    if task_status_is_todo(&parent.status) && !task_ancestors_are_approved(run, parent) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Approve every parent planning item before breaking down a nested Todo item.",
        ));
    }
    Ok(())
}

fn next_hierarchy_parent(run: &AgenticBoard) -> Option<BoardTask> {
    let mut candidates = run
        .tasks
        .iter()
        .filter(|task| task_status_is_todo(&task.status))
        .filter(|task| !task_is_executable(task))
        .filter(|task| task_ancestors_are_approved(run, task))
        .filter_map(|task| {
            let next_level = next_hierarchy_level(task_level(task))?;
            let has_children = hierarchy_breakdown_has_children(run, &task.id, next_level);
            (!has_children).then(|| (task_level(task), task.clone()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(level, task)| {
        let depth = match *level {
            TASK_LEVEL_TASK => 0,
            TASK_LEVEL_STORY => 1,
            TASK_LEVEL_EPIC => 2,
            TASK_LEVEL_INITIATIVE => 3,
            _ => 4,
        };
        (depth, task_priority_rank(&task.priority), task.id.clone())
    });
    candidates.into_iter().next().map(|(_, task)| task)
}

fn build_hierarchy_breakdown_prompt(
    run: &AgenticBoard,
    parent: &BoardTask,
    next_level: &str,
) -> String {
    let existing_titles = hierarchy_breakdown_child_title_candidates(run, &parent.id, next_level);
    let refinement = !existing_titles.is_empty();
    let max_new_children = hierarchy_breakdown_max_new_children(existing_titles.len());
    let breakdown_mode = if refinement {
        "refinement / gap check"
    } else {
        "initial breakdown"
    };
    let children = hierarchy_breakdown_child_title_candidates(run, &parent.id, next_level)
        .into_iter()
        .map(|title| format!("- {title}"))
        .collect::<Vec<_>>()
        .join("\n");
    let direct_children = direct_hierarchy_children(run, &parent.id)
        .into_iter()
        .filter(|child| task_level(child) == next_level)
        .map(|child| format!("{} [{}] {}", child.id, task_level(child), child.title))
        .collect::<Vec<_>>()
        .join("\n");
    let acceptance = if parent.acceptance_criteria.is_empty() {
        "None recorded; derive concrete criteria from the parent description.".to_string()
    } else {
        parent.acceptance_criteria.join("\n- ")
    };
    format!(
        r#"Break down exactly one approved Kanban item into its next hierarchy level.

Parent:
- id: {id}
- level: {level}
- title: {title}
- description: {description}
- acceptance criteria:
- {acceptance}

Next level: {next_level}
Breakdown mode: {breakdown_mode}
Existing next-level child titles:
{children}
Existing direct child records:
{direct_children}

Codebase context:
{codebase}

Return JSON only. No markdown fence.
Schema:
{{
  "items": [
    {{
      "level": "{next_level}",
      "title": "specific title",
      "kind": "research|design|implementation|test_implementation|qa|manual_test|review|fix|migration|revert|cleanup|revision|replacement",
      "description": "one-purpose scope",
      "acceptanceCriteria": ["specific verifiable outcome"],
      "priority": "p0|p1|p2|p3",
      "blockedBy": [],
      "required": true,
      "plannedFiles": [],
      "sideEffects": []
    }}
  ]
}}

Rules:
- Create only the next level, never skip a level.
- One subtask has one engineering purpose. Separate implementation, test-writing, QA, review, and manual testing into separate subtasks.
- Subtask titles must be concrete engineer execution tickets, such as `Add endpoint PATCH /budget/{{month}}` or `Run Android emulator smoke test for budget edit flow`.
- Do not create nice-to-have work as a required child. Put it in a separate Backlog story.
- Return no more than {max_new_children} genuinely new child item(s).
- If an existing child already covers the work, do not return it again, even when the title is worded differently.
- In refinement / gap check mode, return an empty items array when the existing children already cover the parent.
- Set `required` to false for optional or out-of-scope work, and list every
  possible external side effect in `sideEffects`; those children stay in Backlog
  until the user explicitly approves them.
- Inspect the supplied codebase context and use real project architecture; do not invent endpoints or frameworks.
- Use only the parent ticket description, acceptance criteria, dependencies, and codebase context."#,
        id = parent.id,
        level = task_level(parent),
        title = parent.title,
        description = parent.description,
        acceptance = acceptance,
        next_level = next_level,
        breakdown_mode = breakdown_mode,
        children = if children.is_empty() {
            "None"
        } else {
            &children
        },
        direct_children = if direct_children.is_empty() {
            "None"
        } else {
            &direct_children
        },
        max_new_children = max_new_children,
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
    )
}

fn generated_child_requires_backlog_approval(task: &BoardTask) -> bool {
    if !task.hierarchy.required
        || !task.hierarchy.side_effects.is_empty()
        || task_requires_external_side_effect_declaration(task)
    {
        return true;
    }
    let text = task_external_effect_text(task);
    [
        "nice-to-have",
        "nice to have",
        "optional",
        "out of scope",
        "out-of-scope",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn generated_scope_wrapper_exists(run: &AgenticBoard, source_parent_id: &str, key: &str) -> bool {
    run.tasks.iter().any(|task| {
        task.task_origin == "hierarchy_backlog_wrapper"
            && task.hierarchy.discussion.iter().any(|entry| {
                entry.get("kind").and_then(Value::as_str) == Some("generated_scope_wrapper")
                    && entry.get("sourceParentId").and_then(Value::as_str) == Some(source_parent_id)
                    && entry.get("sourceTitleKey").and_then(Value::as_str) == Some(key)
            })
    })
}

fn generated_hierarchy_wrapper_exists(
    run: &AgenticBoard,
    source_parent_id: &str,
    wrapped_level: &str,
) -> bool {
    run.tasks.iter().any(|task| {
        task.task_origin == "hierarchy_backlog_wrapper"
            && task.hierarchy.discussion.iter().any(|entry| {
                entry.get("kind").and_then(Value::as_str) == Some("generated_scope_wrapper")
                    && entry.get("sourceParentId").and_then(Value::as_str) == Some(source_parent_id)
                    && entry.get("wrappedLevel").and_then(Value::as_str) == Some(wrapped_level)
            })
    })
}

fn mark_generated_scope_wrapper(
    task: &mut BoardTask,
    source_parent: &BoardTask,
    source_title_key: &str,
    wrapped_level: &str,
) {
    task.hierarchy.discussion.push(json!({
        "kind": "generated_scope_wrapper",
        "sourceParentId": source_parent.id,
        "sourceParentTitle": source_parent.title,
        "sourceTitleKey": source_title_key,
        "wrappedLevel": wrapped_level,
        "reason": "risky_or_out_of_scope",
        "createdAt": Utc::now(),
    }));
}

fn wrap_generated_child_in_backlog(
    run: &mut AgenticBoard,
    source_parent: &BoardTask,
    mut child: BoardTask,
    next_level: &str,
    source_title_key: &str,
) -> Vec<BoardTask> {
    let child_title = child.title.clone();
    let child_details = if child.details.trim().is_empty() {
        child.description.clone()
    } else {
        child.details.clone()
    };
    let source_reference = format!(
        "Generated from source parent {}: {}",
        source_parent.id, source_parent.title
    );
    let wrapper_details = format!(
        "Review and approve the generated {next_level} scope before execution.\n\n{child_details}"
    );
    let mut story = BoardTask::draft(
        run,
        format!("Proposed scope: {child_title}"),
        wrapper_details.clone(),
    );
    story.priority = child.priority.clone();
    story.status = TASK_STATUS_BACKLOG.to_string();
    story.task_type = TASK_KIND_DESIGN.to_string();
    story.task_origin = "hierarchy_backlog_wrapper".to_string();
    story.prompt = wrapper_details.clone();
    story.description = wrapper_details.clone();
    story.details = wrapper_details;
    story.acceptance_criteria = child.acceptance_criteria.clone();
    story.references = child.references.clone();
    story.references.push(source_reference.clone());
    story.hierarchy.level = TASK_LEVEL_STORY.to_string();
    story.hierarchy.parent_id = None;
    story.hierarchy.executable = false;
    story.hierarchy.required = child.hierarchy.required;
    story.hierarchy.scope_version = source_parent.hierarchy.scope_version.saturating_add(1);
    story.hierarchy.rank = child.hierarchy.rank;
    story.hierarchy.planned_files = child.hierarchy.planned_files.clone();
    story.hierarchy.side_effects = child.hierarchy.side_effects.clone();
    story.hierarchy.side_effects_approved = false;
    story.hierarchy.side_effect_approval = None;
    story.group_id = Some(story.id.clone());
    mark_generated_scope_wrapper(&mut story, source_parent, source_title_key, next_level);
    let story_id = story.id.clone();

    child.id = unique_task_id(run, &format!("{story_id}-{next_level}"));
    child.status = TASK_STATUS_BACKLOG.to_string();
    child.hierarchy.scope_version = source_parent.hierarchy.scope_version.saturating_add(1);
    child.hierarchy.blocked_by = dedupe_strings(child.depends_on.clone());
    child.depends_on = child.hierarchy.blocked_by.clone();
    child.hierarchy.required = story.hierarchy.required;
    child.hierarchy.side_effects_approved = false;
    child.hierarchy.side_effect_approval = None;
    child.hierarchy.side_effect_evidence.clear();
    child.references.push(source_reference);
    child.task_origin = "hierarchy_breakdown_wrapped".to_string();
    child.prompt = child.description.clone();
    child.group_id = Some(story_id.clone());
    mark_generated_scope_wrapper(&mut child, source_parent, source_title_key, next_level);

    if next_level == TASK_LEVEL_SUBTASK {
        let mut task_wrapper = BoardTask::draft(
            run,
            format!("Planned task: {child_title}"),
            format!(
                "Keep the generated subtask under this approved task scope.\n\n{child_details}"
            ),
        );
        task_wrapper.priority = child.priority.clone();
        task_wrapper.status = TASK_STATUS_BACKLOG.to_string();
        task_wrapper.task_type = TASK_KIND_DESIGN.to_string();
        task_wrapper.task_origin = "hierarchy_backlog_wrapper".to_string();
        task_wrapper.prompt = task_wrapper.description.clone();
        task_wrapper.hierarchy.level = TASK_LEVEL_TASK.to_string();
        task_wrapper.hierarchy.parent_id = Some(story_id.clone());
        task_wrapper.hierarchy.executable = false;
        task_wrapper.hierarchy.required = story.hierarchy.required;
        task_wrapper.hierarchy.scope_version = child.hierarchy.scope_version;
        task_wrapper.hierarchy.rank = child.hierarchy.rank;
        task_wrapper.hierarchy.planned_files = child.hierarchy.planned_files.clone();
        task_wrapper.hierarchy.side_effects = child.hierarchy.side_effects.clone();
        task_wrapper.hierarchy.side_effects_approved = false;
        task_wrapper.hierarchy.side_effect_approval = None;
        task_wrapper.acceptance_criteria = child.acceptance_criteria.clone();
        task_wrapper.references = child.references.clone();
        task_wrapper.group_id = Some(story_id.clone());
        mark_generated_scope_wrapper(
            &mut task_wrapper,
            source_parent,
            source_title_key,
            TASK_LEVEL_TASK,
        );
        child.hierarchy.parent_id = Some(task_wrapper.id.clone());
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.executable = true;
        return vec![story, task_wrapper, child];
    }

    child.hierarchy.parent_id = Some(story_id);
    child.hierarchy.level = next_level.to_string();
    child.hierarchy.executable = false;
    vec![story, child]
}

async fn plan_hierarchy_children(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    parent_id: &str,
    manual: bool,
) -> Result<usize> {
    let snapshot = load_user_board(state, user_id, board_id)?.board;
    let Some(parent) = snapshot
        .tasks
        .iter()
        .find(|task| task.id == parent_id)
        .cloned()
    else {
        return Err(not_found("Hierarchy parent not found"));
    };
    let Some(next_level) = next_hierarchy_level(task_level(&parent)) else {
        return Ok(0);
    };
    validate_hierarchy_breakdown_parent(&snapshot, &parent)?;
    let refinement = hierarchy_breakdown_has_children(&snapshot, &parent.id, next_level);
    let prompt = build_hierarchy_breakdown_prompt(&snapshot, &parent, next_level);
    let breakdown_started_at = Utc::now();
    let output = execute_internal_prompt(
        state,
        user_id,
        board_id,
        &format!("hierarchy breakdown for {}", parent.id),
        &prompt,
    )
    .await;
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            let failure = format!(
                "Hierarchy breakdown provider call failed: {}",
                server_error_message(&error)
            );
            record_hierarchy_breakdown_failure(
                state,
                user_id,
                board_id,
                &parent,
                &prompt,
                breakdown_started_at,
                &failure,
                manual,
            )?;
            return Err(ServerError::new(StatusCode::BAD_GATEWAY, failure));
        }
    };
    let output_json = match parse_json_object(&output) {
        Some(output_json) => output_json,
        None => {
            let failure = "Hierarchy breakdown returned malformed JSON instead of the required items contract.";
            record_hierarchy_breakdown_failure(
                state,
                user_id,
                board_id,
                &parent,
                &prompt,
                breakdown_started_at,
                failure,
                manual,
            )?;
            return Err(bad_request(failure));
        }
    };
    let source_items = output_json
        .get("items")
        .or_else(|| output_json.get("tasks"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let returned_empty_items = output_json
        .get("items")
        .or_else(|| output_json.get("tasks"))
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty);
    let children = source_items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let inherits_priority = item
                .get("priority")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_none_or(str::is_empty);
            let inherits_rank = item.get("rank").and_then(Value::as_i64).is_none();
            let mut child = task_from_json(&snapshot, item, index, TASK_STATUS_TODO)?;
            if inherits_priority {
                child.priority = parent.priority.clone();
            }
            if inherits_rank {
                child.hierarchy.rank = index as i64;
            }
            let needs_backlog_approval = generated_child_requires_backlog_approval(&child);
            Some((child, needs_backlog_approval))
        })
        .collect::<Vec<_>>();
    if children.is_empty() && (!refinement || !returned_empty_items) {
        let failure = if refinement {
            "Hierarchy breakdown refinement returned no usable child items."
        } else {
            "Hierarchy breakdown returned no usable child items."
        };
        record_hierarchy_breakdown_failure(
            state,
            user_id,
            board_id,
            &parent,
            &prompt,
            breakdown_started_at,
            failure,
            manual,
        )?;
        return Err(bad_request(failure));
    }

    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, board_id)?;
    let current_parent = stored
        .board
        .tasks
        .iter()
        .find(|task| task.id == parent.id)
        .cloned()
        .ok_or_else(|| not_found("Hierarchy parent no longer exists"))?;
    if current_parent.hierarchy.scope_version != parent.hierarchy.scope_version
        || canonical_task_status(&current_parent.status) != canonical_task_status(&parent.status)
        || current_parent.title != parent.title
        || current_parent.description != parent.description
        || current_parent.acceptance_criteria != parent.acceptance_criteria
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The parent scope changed while breakdown was running; discard this result and run Breakdown again.",
        ));
    }
    let child_status = match canonical_task_status(&current_parent.status) {
        TASK_STATUS_BACKLOG => TASK_STATUS_BACKLOG,
        TASK_STATUS_TODO => TASK_STATUS_TODO,
        _ => {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "Hierarchy breakdown requires the parent to remain in Backlog or Todo.",
            ));
        }
    };
    let provider = effective_provider_for_phase(&snapshot, "hierarchy breakdown")
        .unwrap_or_else(|_| snapshot.provider.clone());
    let model = effective_model_for_phase(&snapshot, "hierarchy breakdown");
    let root_group = stored
        .board
        .tasks
        .iter()
        .find(|task| task.id == parent.id)
        .map(task_group_id_or_self)
        .unwrap_or_else(|| parent.id.clone());
    let existing_titles =
        hierarchy_breakdown_child_title_candidates(&stored.board, &parent.id, next_level);
    let existing_child_count = existing_titles.len();
    let max_new_children = hierarchy_breakdown_max_new_children(existing_child_count);
    let mut seen_titles = existing_titles;
    let mut created = 0usize;
    let candidate_count = children.len();
    let mut reused = 0usize;
    for (mut child, needs_backlog_approval) in children {
        let key = normalize_suggested_task_key(&child.title);
        if created >= max_new_children
            || key.is_empty()
            || hierarchy_breakdown_title_is_duplicate(&seen_titles, &child.title)
        {
            reused += 1;
            continue;
        }
        // Keep one semantic title key for both ordinary and wrapped children.
        // A provider can return the same work with different wording or once
        // as a wrapped story and once as a normal child; neither is duplicated.
        seen_titles.push(child.title.clone());
        if needs_backlog_approval {
            let wrapped = wrap_generated_child_in_backlog(
                &mut stored.board,
                &current_parent,
                child,
                next_level,
                &key,
            );
            stored.board.tasks.extend(wrapped);
            created += 1;
            continue;
        }
        child.id = unique_task_id(&stored.board, &format!("{}-{}", parent.id, child.id));
        child.hierarchy.level = next_level.to_string();
        child.hierarchy.parent_id = Some(parent.id.clone());
        child.hierarchy.executable = next_level == TASK_LEVEL_SUBTASK;
        child.hierarchy.scope_version = parent.hierarchy.scope_version.saturating_add(1);
        child.hierarchy.blocked_by = dedupe_strings(child.depends_on.clone());
        child.depends_on = child.hierarchy.blocked_by.clone();
        child.group_id = Some(root_group.clone());
        child.status = if child_status == TASK_STATUS_TODO && needs_backlog_approval {
            TASK_STATUS_BACKLOG.to_string()
        } else {
            child_status.to_string()
        };
        child.task_origin = "hierarchy_breakdown".to_string();
        child.prompt = parent.description.clone();
        stored.board.tasks.push(child);
        created += 1;
    }
    if created == 0 {
        if refinement || (candidate_count > 0 && reused == candidate_count) {
            let summary = if existing_child_count >= MAX_HIERARCHY_CHILDREN_PER_PARENT {
                format!(
                    "Breakdown checked; child limit reached ({MAX_HIERARCHY_CHILDREN_PER_PARENT})"
                )
            } else {
                format!("Breakdown checked; no new {next_level} child ticket(s) needed")
            };
            if let Some(parent_task) = stored
                .board
                .tasks
                .iter_mut()
                .find(|task| task.id == parent.id)
            {
                parent_task.error = None;
                parent_task.summary = summary.clone();
                append_hierarchy_breakdown_transcript(
                    parent_task,
                    breakdown_started_at,
                    &provider,
                    &model,
                    &prompt,
                    &output,
                );
            }
            stored.board.append_log(format!(
                "Hierarchy breakdown checked {parent_id}; no new {next_level} child ticket(s) added"
            ));
            refresh_hierarchy_rollups(&mut stored.board);
            stored.board.touch();
            save_board(state, &stored.board)?;
            return Ok(0);
        }
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!("No new {next_level} children could be created for {parent_id}"),
        ));
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
    if let Some(parent_task) = stored
        .board
        .tasks
        .iter_mut()
        .find(|task| task.id == parent.id)
    {
        parent_task.error = None;
        parent_task.summary = if refinement {
            format!("Breakdown refined; added {created} {next_level} child ticket(s)")
        } else {
            format!("Generated {created} {next_level} child ticket(s)")
        };
    }
    if let Some(cycle) = dependency_cycle(&stored.board) {
        let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
        let error = planning_error_conflict(&mut stored.board, &cycle, "dependency", issue);
        stored.board.touch();
        save_board(state, &stored.board)?;
        return Err(error);
    }
    if refinement {
        stored.board.append_log(format!(
            "Hierarchy breakdown refined {parent_id}; added {created} {next_level} child ticket(s)"
        ));
    } else {
        stored.board.append_log(format!(
            "Hierarchy breakdown created {created} {next_level} child ticket(s) under {parent_id}"
        ));
    }
    if let Some(parent_task) = stored
        .board
        .tasks
        .iter_mut()
        .find(|task| task.id == parent.id)
    {
        append_hierarchy_breakdown_transcript(
            parent_task,
            breakdown_started_at,
            &provider,
            &model,
            &prompt,
            &output,
        );
    }
    refresh_hierarchy_rollups(&mut stored.board);
    stored.board.touch();
    save_board(state, &stored.board)?;
    Ok(created)
}

fn append_hierarchy_breakdown_transcript(
    task: &mut BoardTask,
    started_at: DateTime<Utc>,
    provider: &str,
    model: &str,
    prompt: &str,
    output: &str,
) {
    task.transcript.push(redact_transcript_value(&json!({
        "timestamp": started_at,
        "role": "user",
        "kind": "user",
        "provider": provider,
        "model": model,
        "content": prompt,
    })));
    task.transcript.push(redact_transcript_value(&json!({
        "timestamp": Utc::now(),
        "role": "assistant",
        "kind": "assistant",
        "provider": provider,
        "model": model,
        "content": output,
    })));
    task.transcript_updated_at = Some(Utc::now());
}

fn record_hierarchy_breakdown_failure(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    parent: &BoardTask,
    prompt: &str,
    started_at: DateTime<Utc>,
    failure: &str,
    manual: bool,
) -> Result<()> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, board_id)?;
    // A breakdown is a user-invoked planning operation and commonly runs
    // while the board is paused. Persist its failure even in that state so a
    // provider/contract error cannot leave an apparently healthy planning
    // item with no retry or attention signal. Terminal boards are immutable.
    if matches!(stored.board.status.as_str(), "cancelled" | "completed") {
        return Ok(());
    }
    let provider = effective_provider_for_phase(&stored.board, "hierarchy breakdown")
        .unwrap_or_else(|_| stored.board.provider.clone());
    let model = effective_model_for_phase(&stored.board, "hierarchy breakdown");
    if let Some(task) = stored
        .board
        .tasks
        .iter_mut()
        .find(|task| task.id == parent.id)
    {
        // Manual Breakdown is a planning action. A provider/contract
        // failure must remain retryable and must not masquerade as a real
        // dependency blocker. Automatic hierarchy planning retains the
        // existing fail-closed behavior so the worker cannot spin forever.
        if !manual {
            task.status = TASK_STATUS_BLOCKED.to_string();
        }
        task.error = Some(failure.to_string());
        task.summary = failure.to_string();
        task.transcript.push(redact_transcript_value(&json!({
            "timestamp": started_at,
            "role": "user",
            "kind": "user",
            "provider": provider.clone(),
            "model": model.clone(),
            "content": prompt,
        })));
        task.transcript.push(redact_transcript_value(&json!({
            "timestamp": Utc::now(),
            "role": "system",
            "kind": "error",
            "provider": provider,
            "model": model,
            "content": failure,
        })));
        task.transcript_updated_at = Some(Utc::now());
    }
    if manual {
        stored.board.append_log(format!(
            "Manual hierarchy breakdown failed for {}; item remains retryable: {}",
            parent.id, failure
        ));
    } else {
        mark_planning_error(
            &mut stored.board,
            std::slice::from_ref(&parent.id),
            "hierarchy",
            failure,
        );
        if let Some(details) = stored
            .board
            .phase_details
            .as_mut()
            .and_then(Value::as_object_mut)
        {
            details.insert("parentId".to_string(), json!(parent.id));
            details.insert(
                "retry".to_string(),
                json!("Move the planning item to Todo and run Breakdown again after the provider or contract issue is fixed."),
            );
            details.insert("startedAt".to_string(), json!(started_at));
        }
    }
    stored.board.touch();
    save_board(state, &stored.board)
}

fn is_retryable_hierarchy_breakdown_message(message: &str) -> bool {
    message
        .trim()
        .strip_prefix("Planning error: ")
        .unwrap_or_else(|| message.trim())
        .starts_with("Hierarchy breakdown ")
}

fn is_retryable_hierarchy_breakdown_task(task: &BoardTask) -> bool {
    task.error
        .as_deref()
        .is_some_and(is_retryable_hierarchy_breakdown_message)
        || is_retryable_hierarchy_breakdown_message(&task.summary)
}

fn hierarchy_breakdown_planning_error_for(run: &AgenticBoard, task_id: &str) -> bool {
    run.current_phase.as_deref() == Some(PLANNING_ERROR_PHASE)
        && run
            .phase_details
            .as_ref()
            .and_then(|details| details.get("kind"))
            .and_then(Value::as_str)
            == Some("hierarchy")
        && run
            .phase_details
            .as_ref()
            .and_then(|details| details.get("parentId"))
            .and_then(Value::as_str)
            == Some(task_id)
        && run
            .phase_details
            .as_ref()
            .and_then(|details| details.get("error"))
            .and_then(Value::as_str)
            .is_some_and(is_retryable_hierarchy_breakdown_message)
}

fn restore_board_after_hierarchy_breakdown_failure(run: &mut AgenticBoard, task_id: &str) -> bool {
    if !hierarchy_breakdown_planning_error_for(run, task_id) {
        return false;
    }
    run.status = "paused".to_string();
    run.active = false;
    run.loop_started = false;
    run.auto_run_enabled = false;
    run.pause_requested = false;
    run.paused_at = Some(Utc::now());
    run.current_task_id = None;
    run.current_task_title.clear();
    run.current_task_status.clear();
    run.current_phase = Some("board".to_string());
    run.phase_started_at = Some(Utc::now());
    run.phase_details = Some(json!({
        "mode": "kanban_only",
        "breakdownRetryable": true,
        "parentId": task_id,
    }));
    run.pause_reason = Some("Manual hierarchy breakdown is available.".to_string());
    run.append_log(format!(
        "Restored board to paused planning state after hierarchy breakdown failure for {task_id}"
    ));
    true
}

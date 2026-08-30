fn mutate_board(
    state: &AppState,
    user_id: &str,
    id: &str,
    mutate: impl FnOnce(&mut AgenticBoard) -> Result<()>,
) -> Result<Json<Value>> {
    let stored = mutate_stored_board(state, user_id, id, mutate)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

fn mutate_stored_board(
    state: &AppState,
    user_id: &str,
    id: &str,
    mutate: impl FnOnce(&mut AgenticBoard) -> Result<()>,
) -> Result<StoredBoard> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, id)?;
    if let Err(error) = mutate(&mut stored.board) {
        if has_persisted_planning_error(&stored.board) {
            stored.board.touch();
            save_board(state, &stored.board)?;
        }
        return Err(error);
    }
    stored.board.touch();
    save_board(state, &stored.board)?;
    Ok(stored)
}

fn project_execution_owner_key(project_path: &str) -> String {
    fs::canonicalize(project_path)
        .unwrap_or_else(|_| PathBuf::from(project_path))
        .display()
        .to_string()
}

fn claim_project_execution(project_path: &str, board_id: &str) -> Result<bool> {
    let owners = PROJECT_EXECUTION_OWNERS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut owners = owners.lock().map_err(|_| {
        ServerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Project execution ownership lock is unavailable.",
        )
    })?;
    let key = project_execution_owner_key(project_path);
    if let Some(owner) = owners.get(&key)
        && owner != board_id
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "Project execution is already owned by board {owner}. Pause it before starting another board for this project."
            ),
        ));
    }
    let already_owned = owners.get(&key).is_some_and(|owner| owner == board_id);
    owners.insert(key, board_id.to_string());
    Ok(!already_owned)
}

fn release_project_execution(project_path: &str, board_id: &str) {
    let Some(owners) = PROJECT_EXECUTION_OWNERS.get() else {
        return;
    };
    let Ok(mut owners) = owners.lock() else {
        return;
    };
    let key = project_execution_owner_key(project_path);
    if owners.get(&key).is_some_and(|owner| owner == board_id) {
        owners.remove(&key);
    }
}

fn start_board_execution(state: &AppState, user_id: &str, id: &str) -> Result<StoredBoard> {
    let (should_spawn, stored) = {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(state, user_id, id)?;
        if let Some(task_id) = unapproved_side_effect_task_ids(&stored.board).first() {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                format!(
                    "Subtask {task_id} requires declared and approved external side effects before the board can run."
                ),
            ));
        }
        let claimed_new = claim_project_execution(&stored.board.project_path, &stored.board.id)?;
        let should_spawn = !stored.board.loop_started || claimed_new;
        clear_board_abort_state(&mut stored.board);
        stored.board.status = "running".to_string();
        stored.board.scheduled_start_at = None;
        stored.board.active = true;
        stored.board.loop_started = true;
        stored.board.auto_run_enabled = true;
        stored.board.pause_requested = false;
        stored.board.paused_at = None;
        stored.board.pause_reason = None;
        bump_control_revision(&mut stored.board);
        stored.board.current_phase = Some("task_execution".to_string());
        stored.board.phase_started_at = Some(Utc::now());
        stored.board.phase_details = Some(json!({ "source": "kanban_board" }));
        stored.board.append_log("Agentic board execution started");
        stored.board.touch();
        if let Err(error) = save_board(state, &stored.board) {
            release_project_execution(&stored.board.project_path, &stored.board.id);
            return Err(error);
        }
        (should_spawn, stored)
    };

    if should_spawn {
        let state = state.clone();
        let user_id = user_id.to_string();
        let board_id = id.to_string();
        tokio::spawn(async move {
            if let Err(error) = execute_board_loop(state, user_id, board_id).await {
                tracing::warn!(error = %server_error_message(&error), "agentic board worker failed");
            }
        });
    }

    Ok(stored)
}

async fn execute_board_loop(state: AppState, user_id: String, board_id: String) -> Result<()> {
    let project_path = load_user_board(&state, &user_id, &board_id)
        .ok()
        .map(|stored| stored.board.project_path);
    let result = execute_board_loop_inner(state, user_id, board_id.clone()).await;
    if let Some(project_path) = project_path {
        release_project_execution(&project_path, &board_id);
    }
    result
}

async fn execute_board_loop_inner(
    state: AppState,
    user_id: String,
    board_id: String,
) -> Result<()> {
    loop {
        let mut stored = load_user_board(&state, &user_id, &board_id)?;
        if matches!(
            stored.board.status.as_str(),
            "cancelled" | "failed" | "blocked" | "completed"
        ) {
            if matches!(stored.board.status.as_str(), "failed" | "blocked") {
                let status_for_retry = stored.board.status.clone();
                schedule_auto_retry_if_eligible(&mut stored.board, &status_for_retry);
            }
            stored.board.active = false;
            stored.board.loop_started = false;
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        if stored.board.status == "paused" || stored.board.pause_requested {
            settle_board_pause(&mut stored.board);
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }

        if !stored.board.bootstrap_complete {
            bootstrap_agentic_board(&state, &user_id, &board_id).await?;
            continue;
        }

        if let Some(issue) = hierarchy_validation_issues(&stored.board)
            .into_iter()
            .next()
        {
            let affected = planning_error_task_ids(&stored.board, &issue);
            mark_planning_error(&mut stored.board, &affected, "hierarchy", &issue);
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        if let Some(issue) = dependency_validation_issues(&stored.board)
            .into_iter()
            .next()
        {
            let affected = planning_error_task_ids(&stored.board, &issue);
            mark_planning_error(&mut stored.board, &affected, "dependency", &issue);
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        if let Some(cycle) = dependency_cycle(&stored.board) {
            let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
            mark_planning_error(&mut stored.board, &cycle, "dependency", &issue);
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        refresh_hierarchy_rollups(&mut stored.board);
        if let Some(parent) = next_hierarchy_parent(&stored.board) {
            stored.board.current_phase = Some("hierarchy_breakdown".to_string());
            stored.board.phase_started_at = Some(Utc::now());
            stored.board.phase_details = Some(json!({
                "parentId": parent.id,
                "parentLevel": task_level(&parent),
                "nextLevel": next_hierarchy_level(task_level(&parent)),
            }));
            stored.board.append_log(format!(
                "Breaking down {} {} into its next hierarchy level",
                task_level(&parent),
                parent.id
            ));
            stored.board.touch();
            save_board(&state, &stored.board)?;
            plan_hierarchy_children(&state, &user_id, &board_id, &parent.id, false).await?;
            continue;
        }

        reconcile_dependency_statuses(&mut stored.board);
        refresh_hierarchy_rollups(&mut stored.board);
        stored.board.touch();
        save_board(&state, &stored.board)?;

        let Some(task_index) = pick_next_task_index(&stored.board) else {
            if uses_hierarchical_orchestration(&stored.board) {
                reconcile_dependency_statuses(&mut stored.board);
                let waiting_tasks = dependency_waiting_tasks(&stored.board);
                let side_effect_tasks = unapproved_side_effect_task_ids(&stored.board);
                let pending_research = pending_research_acceptance_ids(&stored.board);
                if !side_effect_tasks.is_empty() {
                    mark_side_effect_blockers(&mut stored.board);
                    stored.board.status = TASK_STATUS_BLOCKED.to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_phase =
                        Some("waiting_for_side_effect_approval".to_string());
                    stored.board.pause_reason = Some(
                        "Approve declared external side effects before resuming execution."
                            .to_string(),
                    );
                    stored.board.append_log(format!(
                        "No runnable subtasks: {} require external side-effect approval ({})",
                        side_effect_tasks.len(),
                        side_effect_tasks.join(", ")
                    ));
                } else if !pending_research.is_empty() {
                    stored.board.status = "paused".to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_phase = Some("waiting_for_research_approval".to_string());
                    stored.board.pause_reason = Some(
                        "Accept completed research output before implementation can continue."
                            .to_string(),
                    );
                    stored.board.phase_details = Some(json!({
                        "researchTaskIds": pending_research,
                    }));
                    stored.board.append_log(
                        "Execution paused because completed research awaits user acceptance",
                    );
                } else if !waiting_tasks.is_empty() {
                    mark_dependency_blockers(&mut stored.board);
                    stored.board.status = TASK_STATUS_BLOCKED.to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_phase = Some("blocked".to_string());
                    stored.board.append_log(format!(
                        "No runnable subtasks: {} waiting on dependencies ({})",
                        waiting_tasks.len(),
                        waiting_tasks.join(", ")
                    ));
                } else if has_dependency_blocked_tasks(&stored.board) {
                    stored.board.status = TASK_STATUS_BLOCKED.to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_phase = Some("blocked".to_string());
                    stored.board.append_log(
                       "Hierarchy execution stopped on a dependency blocker; resolve the dependency before resuming",
                    );
                } else if has_hierarchical_attention_tasks(&stored.board) {
                    stored.board.status = TASK_STATUS_BLOCKED.to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_phase = Some("blocked".to_string());
                    stored.board.append_log(
                       "Hierarchy execution stopped at a failed or blocked subtask; retry it or approve a fix subtask",
                    );
                } else if hierarchical_work_is_complete(&stored.board)
                    && !has_backlog_planning_work(&stored.board)
                {
                    stored.board.status = "completed".to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_task_id = None;
                    stored.board.current_task_title.clear();
                    stored.board.current_task_status.clear();
                    stored.board.current_phase = Some("completed".to_string());
                    stored.board.phase_details = Some(json!({
                        "taskCount": stored.board.tasks.len(),
                        "execution": "subtasks_only",
                    }));
                    stored.board.final_review = Some(json!({
                        "complete": true,
                        "summary": "All approved executable subtasks completed.",
                    }));
                    stored.board.append_log(
                        "Hierarchy execution completed: all approved executable subtasks are done",
                    );
                } else {
                    stored.board.status = "paused".to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_phase = Some("waiting_for_approval".to_string());
                    stored.board.pause_reason = Some(
                       "No approved executable subtasks remain; move the next planning item to Todo."
                           .to_string(),
                   );
                    stored.board.append_log(
                        "Hierarchy execution paused because no approved executable subtasks remain",
                    );
                }
                stored.board.touch();
                save_board(&state, &stored.board)?;
                return Ok(());
            }
            if !stored.board.agents_knowledge_updated
                && append_agents_knowledge_task(
                    &mut stored.board,
                    "Implementation work completed before final QA",
                    None,
                )
            {
                stored.board.current_phase = Some("agents_update".to_string());
                stored.board.phase_started_at = Some(Utc::now());
                stored
                    .board
                    .append_log("Appended AGENTS.md knowledge update task");
                stored.board.touch();
                save_board(&state, &stored.board)?;
                continue;
            }
            if append_promotion_review_task(&mut stored.board, "Final promotion review") {
                stored.board.current_phase = Some("promotion_review".to_string());
                stored.board.phase_started_at = Some(Utc::now());
                stored
                    .board
                    .append_log("Appended RAG promotion review task");
                stored.board.touch();
                save_board(&state, &stored.board)?;
                continue;
            }
            stored.board.status = "completed".to_string();
            stored.board.active = false;
            stored.board.loop_started = false;
            stored.board.current_task_id = None;
            stored.board.current_task_title.clear();
            stored.board.current_task_status.clear();
            stored.board.current_phase = Some("completed".to_string());
            stored.board.phase_details = Some(json!({ "taskCount": stored.board.tasks.len() }));
            stored.board.final_qa_complete = true;
            stored.board.final_review = Some(json!({
                "complete": true,
                "summary": "All runnable board tasks completed.",
            }));
            stored.board.append_log("Agentic board execution completed");
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        };

        let task_id = stored.board.tasks[task_index].id.clone();
        let task_title = stored.board.tasks[task_index].title.clone();
        let started_at = Utc::now();
        stored.board.status = "running".to_string();
        stored.board.current_task_id = Some(task_id.clone());
        stored.board.current_task_title = task_title.clone();
        stored.board.current_task_status = "in_progress".to_string();
        let task_phase = stored
            .board
            .tasks
            .get(task_index)
            .map(|task| {
                if is_qa_task(task) {
                    "qa_task"
                } else if is_promotion_review_task(task) {
                    "promotion_review"
                } else if task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID {
                    "agents_update"
                } else {
                    "task_execution"
                }
            })
            .unwrap_or("task_execution");
        stored.board.current_phase = Some(task_phase.to_string());
        stored.board.phase_started_at = Some(started_at);
        stored.board.phase_details = Some(json!({ "taskId": task_id, "taskTitle": task_title }));
        apply_task_model_routing(&mut stored.board, task_index);
        if let Some(task) = stored.board.tasks.get_mut(task_index) {
            task.status = "in_progress".to_string();
            task.attempt_count = task.attempt_count.saturating_add(1);
            task.started_at = Some(started_at);
            task.completed_at = None;
            task.error = None;
            task.transcript.push(json!({
                "timestamp": started_at,
                "kind": "status",
                "status": "running",
                "content": "Task execution started",
            }));
            task.transcript_updated_at = Some(started_at);
        }
        stored.board.append_log(format!("Executing task {task_id}"));
        stored.board.touch();
        save_board(&state, &stored.board)?;

        if stored
            .board
            .tasks
            .get(task_index)
            .is_some_and(is_promotion_review_task)
        {
            let attempt_id = format!("attempt-{}", Uuid::new_v4());
            if let Some(task) = stored.board.tasks.get_mut(task_index) {
                let transcript_start_index = task.transcript.len().saturating_sub(1);
                task.hierarchy.attempts.push(json!({
                    "attemptId": attempt_id,
                    "attemptNumber": task.attempt_count,
                    "startedAt": started_at,
                    "status": "running",
                    "transcriptStartIndex": transcript_start_index,
                }));
            }
            stored.board.touch();
            save_board(&state, &stored.board)?;
            if let Err(error) = execute_promotion_review_task(
                &state,
                &user_id,
                &board_id,
                &mut stored.board,
                task_index,
            )
            .await
            {
                let now = Utc::now();
                if let Some(task) = stored.board.tasks.get_mut(task_index) {
                    task.status = TASK_STATUS_FAILED.to_string();
                    task.error = Some(server_error_message(&error));
                    task.completed_at = Some(now);
                    task.qa_passed = Some(false);
                }
                finish_task_attempt(
                    &mut stored.board,
                    &task_id,
                    &attempt_id,
                    TASK_STATUS_FAILED,
                    now,
                );
                stored.board.touch();
                save_board(&state, &stored.board)?;
                return Err(error);
            }
            let attempt_status = stored
                .board
                .tasks
                .get(task_index)
                .map(|task| canonical_task_status(&task.status))
                .unwrap_or(TASK_STATUS_FAILED);
            finish_task_attempt(
                &mut stored.board,
                &task_id,
                &attempt_id,
                attempt_status,
                Utc::now(),
            );
            stored.board.current_task_id = None;
            stored.board.current_task_title.clear();
            stored.board.current_task_status.clear();
            stored.board.touch();
            save_board(&state, &stored.board)?;
            continue;
        }

        let managed_git_ready =
            ensure_managed_git_branch_for_task_group(&mut stored.board, &task_id).await;
        if let Err(error) = managed_git_ready {
            let message = server_error_message(&error);
            if let Some(task) = stored
                .board
                .tasks
                .iter_mut()
                .find(|task| task.id == task_id)
            {
                task.status = "blocked".to_string();
                task.error = Some(message.clone());
                task.summary = message.clone();
                task.completed_at = Some(Utc::now());
                task.qa_passed = Some(false);
            }
            stored.board.status = "blocked".to_string();
            stored.board.active = false;
            stored.board.loop_started = false;
            stored.board.append_log(format!(
                "Task blocked before execution by managed git policy: {message}"
            ));
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        save_board(&state, &stored.board)?;

        if !ensure_tdd_baseline_for_task(&state, &user_id, &board_id, &mut stored.board, task_index)
            .await?
        {
            stored.board.current_task_id = None;
            stored.board.current_task_title.clear();
            stored.board.current_task_status.clear();
            stored.board.touch();
            save_board(&state, &stored.board)?;
            continue;
        }
        stored = load_user_board(&state, &user_id, &board_id)?;
        if stored.board.status == "paused" || stored.board.pause_requested {
            settle_board_pause(&mut stored.board);
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        let Some(task_index) = stored
            .board
            .tasks
            .iter()
            .position(|task| task.id == task_id)
        else {
            continue;
        };
        if let Some(task) = stored.board.tasks.get_mut(task_index) {
            if task.tdd_phase == "qa_failed_expected" {
                task.tdd_phase = "dev_pending".to_string();
            }
        }

        attach_rag_context_for_task(&mut stored.board, task_index).await;
        stored.board.touch();
        save_board(&state, &stored.board)?;
        stored = load_user_board(&state, &user_id, &board_id)?;
        if stored.board.status == "paused" || stored.board.pause_requested {
            settle_board_pause(&mut stored.board);
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        let Some(task_index) = stored
            .board
            .tasks
            .iter()
            .position(|task| task.id == task_id)
        else {
            continue;
        };

        let before_workspace = capture_workspace_snapshot(&stored.board.project_path);
        stored.board.provider_call_started_at = Some(Utc::now());
        stored.board.provider_call_label = Some(format!("task execution for {task_id}"));
        let attempt_id = format!("attempt-{}", Uuid::new_v4());
        if let Some(task) = stored
            .board
            .tasks
            .iter_mut()
            .find(|task| task.id == task_id)
        {
            let transcript_start_index = task.transcript.len().saturating_sub(1);
            task.hierarchy.attempts.push(json!({
                "attemptId": attempt_id,
                "attemptNumber": task.attempt_count,
                "startedAt": started_at,
                "status": "running",
                "transcriptStartIndex": transcript_start_index,
            }));
        }
        stored.board.touch();
        save_board(&state, &stored.board)?;
        let provider_attempt =
            execute_provider_task_with_fallback(&state, &stored.board, task_index).await;
        let mut stored = load_user_board(&state, &user_id, &board_id)?;
        let task_position = stored
            .board
            .tasks
            .iter()
            .position(|task| task.id == task_id);
        let now = Utc::now();

        if let Some(fallback) = provider_attempt.fallback {
            let previous_provider = stored.board.provider.clone();
            let previous_model = stored.board.model.clone();
            stored.board.provider = fallback.provider.clone();
            stored.board.model = fallback.model.clone();
            stored.board.last_effective_model = Some(fallback.model.clone());
            reset_provider_session(&mut stored.board, "provider fallback");
            stored.board.model_history.push(json!({
                "fromProvider": previous_provider,
                "from": previous_model,
                "toProvider": fallback.provider,
                "to": fallback.model,
                "changedAt": Utc::now(),
                "changedBy": "provider-fallback",
                "reason": fallback.reason,
                "taskId": task_id,
            }));
            stored
                .board
                .append_log("Primary provider call failed; activated configured fallback");
        }

        match provider_attempt.result {
            Ok(mut result) => {
                let mut parsed = parse_execution_result(&result.assistant_text)
                    .unwrap_or_else(|| missing_json_task_result(&result.assistant_text));
                let mut fatal_provider_errors =
                    filter_fatal_provider_errors(&result.errors, result.exit_code);
                if result.errors.len() > fatal_provider_errors.len() {
                    stored.board.append_log(format!(
                        "Ignored {} non-fatal provider advisory message(s) for {task_id}",
                        result.errors.len() - fatal_provider_errors.len()
                    ));
                }
                let mut failed_with_provider_error =
                    result.exit_code != 0 || !fatal_provider_errors.is_empty();

                if !failed_with_provider_error
                    && is_recoverable_self_reported_blocker(&parsed)
                    && stored
                        .board
                        .tasks
                        .get(task_position.unwrap_or(task_index))
                        .map(|task| task.attempt_count < max_task_attempts(&stored.board))
                        .unwrap_or(false)
                {
                    let stale_tool_blocker = is_tool_environment_self_reported_blocker(&parsed)
                        && !provider_events_have_tool_evidence(&result.stream_events);
                    if stale_tool_blocker {
                        reset_provider_session(
                            &mut stored.board,
                            &format!("stale tool-environment blocker reported by {task_id}"),
                        );
                    }
                    if let Some(index) = task_position {
                        if let Some(task) = stored.board.tasks.get_mut(index) {
                            task.attempt_count = task.attempt_count.saturating_add(1);
                            if stale_tool_blocker {
                                task.provider_session_id = None;
                            } else if let Some(session_id) = result.session_id.clone() {
                                task.provider_session_id = Some(session_id);
                            }
                            task.transcript.extend(result.stream_events.clone());
                            task.transcript.push(json!({
                                "timestamp": Utc::now(),
                                "kind": "status",
                                "status": "retrying",
                                "content": if stale_tool_blocker {
                                    "Retrying stale tool-environment blocker in a fresh provider session"
                                } else {
                                    "Retrying recoverable self-reported blocker"
                                },
                            }));
                            task.transcript_updated_at = Some(Utc::now());
                        }
                    }
                    stored.board.append_log(if stale_tool_blocker {
                        format!("Retrying stale tool-environment blocker for {task_id} in a fresh session")
                    } else {
                        format!("Retrying recoverable blocker for {task_id}")
                    });
                    stored.board.touch();
                    save_board(&state, &stored.board)?;

                    match execute_provider_task(&state, &stored.board, task_index).await {
                        Ok(retry_result) => {
                            result = retry_result;
                            parsed = parse_execution_result(&result.assistant_text).unwrap_or_else(
                                || missing_json_task_result(&result.assistant_text),
                            );
                            fatal_provider_errors =
                                filter_fatal_provider_errors(&result.errors, result.exit_code);
                            if result.errors.len() > fatal_provider_errors.len() {
                                stored.board.append_log(format!(
                                    "Ignored {} non-fatal provider advisory message(s) for {task_id}",
                                    result.errors.len() - fatal_provider_errors.len()
                                ));
                            }
                            failed_with_provider_error =
                                result.exit_code != 0 || !fatal_provider_errors.is_empty();
                        }
                        Err(error) => {
                            result = ProviderTaskResult::from_error(error);
                            parsed = missing_json_task_result(&result.assistant_text);
                            fatal_provider_errors =
                                filter_fatal_provider_errors(&result.errors, result.exit_code);
                            failed_with_provider_error = true;
                        }
                    }
                }

                let change_summary =
                    record_task_workspace_changes(&mut stored.board, &task_id, before_workspace);
                if failed_with_provider_error
                    && should_treat_provider_errors_as_followup(&result, &parsed, &change_summary)
                {
                    parsed = convert_missing_json_provider_error_to_followup(&parsed, &result);
                    failed_with_provider_error = false;
                    stored.board.append_log(format!(
                        "Converted provider error to follow-up for {task_id} because the task changed files but missed final JSON"
                    ));
                }
                if !failed_with_provider_error
                    && should_repair_task_result(&stored.board, &task_id, &parsed, &change_summary)
                {
                    parsed = repair_task_result_if_needed(
                        &state,
                        &user_id,
                        &board_id,
                        &task_id,
                        task_index,
                        &result.assistant_text,
                        parsed,
                        &change_summary,
                    )
                    .await;
                }
                let is_agents_knowledge_task = stored
                    .board
                    .tasks
                    .get(task_position.unwrap_or(task_index))
                    .map(|task| task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID)
                    .unwrap_or(false);
                if !failed_with_provider_error && !is_agents_knowledge_task {
                    let task_for_validation = stored
                        .board
                        .tasks
                        .get(task_position.unwrap_or(task_index))
                        .cloned();
                    let validation = if let Some(task) = task_for_validation.as_ref() {
                        run_tdd_validation(&stored.board, task, "feature").await
                    } else {
                        run_deterministic_validation(&stored.board, &task_id, "feature").await
                    };
                    parsed = apply_deterministic_validation_result(parsed, &validation);
                    stored.board.validation_runs.push(validation.clone());
                    if let Some(index) = task_position {
                        if let Some(task) = stored.board.tasks.get_mut(index) {
                            task.deterministic_validation = Some(validation);
                            if task.tdd_phase != "disabled" && !is_qa_task(task) {
                                if task
                                    .deterministic_validation
                                    .as_ref()
                                    .and_then(|value| value.get("passed"))
                                    .and_then(Value::as_bool)
                                    == Some(true)
                                {
                                    task.tdd_phase = "evidence_review".to_string();
                                } else {
                                    task.tdd_phase = "fix_pending".to_string();
                                    task.fix_attempts = task.fix_attempts.saturating_add(1);
                                }
                            }
                        }
                    }
                }
                if !failed_with_provider_error {
                    parsed = apply_completion_evidence_gate(
                        &stored.board,
                        &task_id,
                        parsed,
                        &change_summary,
                    );
                }
                refresh_codebase_context_after_task(&mut stored.board, &change_summary);
                let completion_summary = resolved_execution_summary(&parsed, &result.summary);
                let hierarchical_execution = uses_hierarchical_orchestration(&stored.board);
                if let Some(index) = task_position {
                    let task = &mut stored.board.tasks[index];
                    if result.session_id.is_some() {
                        task.provider_session_id = result.session_id.clone();
                    }
                    task.transcript.extend(result.stream_events.clone());
                    task.transcript.push(json!({
                        "timestamp": now,
                        "kind": "assistant",
                        "provider": stored.board.provider,
                        "content": result.assistant_text,
                    }));
                    if !result.stderr.trim().is_empty() {
                        task.transcript.push(json!({
                            "timestamp": now,
                            "kind": "stderr",
                            "provider": stored.board.provider,
                            "content": result.stderr,
                        }));
                    }
                    task.transcript.push(json!({
                        "timestamp": now,
                        "kind": "complete",
                        "exitCode": result.exit_code,
                        "content": completion_summary,
                    }));
                    task.transcript_updated_at = Some(now);
                    task.completed_at = Some(now);
                    task.summary = completion_summary;
                    task.result = Some(parsed.clone());
                    task.changed_file_summary = Some(change_summary.clone());
                    task.commands_run = value_to_strings(parsed.get("commandsRun").cloned());
                    let attributable_changed_files = change_summary_paths(&change_summary);
                    if hierarchical_execution {
                        if let Some(object) = parsed.as_object_mut() {
                            object.insert(
                                "changedFiles".to_string(),
                                json!(attributable_changed_files.clone()),
                            );
                        }
                        task.result = Some(parsed.clone());
                        task.changed_files = dedupe_strings(
                            task.changed_files
                                .clone()
                                .into_iter()
                                .chain(attributable_changed_files.clone())
                                .collect(),
                        );
                    } else {
                        task.changed_files = value_to_strings(parsed.get("changedFiles").cloned());
                        if task.changed_files.is_empty() {
                            task.changed_files = attributable_changed_files;
                        }
                    }
                    task.evidence = value_to_strings(parsed.get("evidence").cloned());
                    task.hierarchy.side_effect_evidence = external_side_effect_evidence(&parsed);
                    if canonical_task_kind(task) == TASK_KIND_MANUAL_TEST {
                        let environment = manual_test_environment_evidence(&parsed);
                        let has_environment = environment
                            .as_object()
                            .is_some_and(|object| !object.is_empty());
                        task.hierarchy.manual_test_environment =
                            has_environment.then_some(environment);
                    }
                    task.remaining_issues = value_to_strings(
                        parsed
                            .get("remainingIssues")
                            .cloned()
                            .or_else(|| parsed.get("remainingGaps").cloned()),
                    );
                    if failed_with_provider_error {
                        let error = if fatal_provider_errors.is_empty() {
                            format!("Provider exited with code {}", result.exit_code)
                        } else {
                            fatal_provider_errors.join("\n")
                        };
                        task.status = "blocked".to_string();
                        task.qa_passed = Some(false);
                        task.error = Some(limit_text(&error, 1200));
                        task.summary = parsed
                            .get("summary")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .filter(|summary| !summary.trim().is_empty())
                            .unwrap_or_else(|| task.error.clone().unwrap_or_default());
                    } else if completion_evidence_gate_failed(&parsed) {
                        task.status = TASK_STATUS_BLOCKED.to_string();
                        task.qa_passed = Some(false);
                        task.error = Some(
                            "Completion evidence gate failed; provide valid evidence before this subtask can be done."
                                .to_string(),
                        );
                        task.completed_at = None;
                        if task.tdd_phase != "disabled" && !is_qa_task(task) {
                            task.tdd_phase = "followup_pending".to_string();
                            task.fix_attempts = task.fix_attempts.saturating_add(1);
                        }
                    } else if parsed_status_done(Some(&parsed)) {
                        task.status = TASK_STATUS_DONE.to_string();
                        task.qa_passed = Some(parsed_qa_passed(Some(&parsed)));
                        task.error = None;
                        if task.tdd_phase != "disabled" && !is_qa_task(task) {
                            task.tdd_phase = "done".to_string();
                            if let Some(validation) = task.deterministic_validation.as_ref() {
                                task.coverage_evidence.push(json!({
                                    "kind": "feature_validation",
                                    "validation": validation,
                                    "recordedAt": Utc::now(),
                                }));
                            }
                        }
                        if task.final_qa_task && task.qa_passed == Some(true) {
                            stored.board.final_qa_complete = true;
                        }
                        if task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID {
                            stored.board.agents_knowledge_updated = true;
                            stored.board.agents_context =
                                Some(read_agents_context(&stored.board.project_path));
                        }
                    } else {
                        let needs_followup = parsed
                            .get("status")
                            .and_then(Value::as_str)
                            .is_some_and(|status| status == "needs_followup");
                        let requires_user_fix = hierarchical_execution
                            && !matches!(
                                canonical_task_kind(task),
                                TASK_KIND_QA | TASK_KIND_MANUAL_TEST | TASK_KIND_REVIEW
                            );
                        task.status = if needs_followup && !requires_user_fix {
                            TASK_STATUS_DONE.to_string()
                        } else if needs_followup {
                            TASK_STATUS_BLOCKED.to_string()
                        } else {
                            TASK_STATUS_FAILED.to_string()
                        };
                        if task.tdd_phase != "disabled" && !is_qa_task(task) {
                            task.tdd_phase = if needs_followup {
                                "followup_pending".to_string()
                            } else {
                                "fix_pending".to_string()
                            };
                            task.fix_attempts = task.fix_attempts.saturating_add(1);
                        }
                        task.qa_passed = Some(false);
                        task.error = if needs_followup && requires_user_fix {
                            Some(
                                "Incomplete work remains inside the approved scope. Create a concrete fix subtask under this parent."
                                    .to_string(),
                            )
                        } else if needs_followup {
                            None
                        } else {
                            Some(
                                parsed
                                    .get("summary")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                                    .unwrap_or_else(|| {
                                        format!("Provider exited with code {}", result.exit_code)
                                    }),
                            )
                        };
                    }
                }
                let attempt_status = task_position
                    .and_then(|index| stored.board.tasks.get(index))
                    .map(|task| canonical_task_status(&task.status))
                    .unwrap_or(TASK_STATUS_FAILED);
                finish_task_attempt(
                    &mut stored.board,
                    &task_id,
                    &attempt_id,
                    attempt_status,
                    now,
                );
                if let Some(session_id) = result.session_id.clone() {
                    stored.board.current_provider_session_id = Some(session_id.clone());
                    stored.board.actual_session_id = Some(session_id.clone());
                    if stored.board.session_id.is_none()
                        || should_resume_provider_session(&stored.board)
                    {
                        stored.board.session_id = Some(session_id);
                    }
                }
                if let Some(task_for_usage) = stored.board.tasks.get(task_index) {
                    let prompt_for_usage =
                        build_task_execution_prompt(&stored.board, task_for_usage, task_index);
                    increment_provider_usage(
                        &mut stored.board,
                        &prompt_for_usage,
                        &result.assistant_text,
                        result.session_id.as_deref(),
                        result.token_usage.as_ref(),
                    );
                }
                apply_task_result_to_board(&mut stored.board, &task_id, &parsed);
                if !failed_with_provider_error {
                    ingest_rag_task_outcome(&mut stored.board, &task_id, &parsed).await;
                }
                if !failed_with_provider_error {
                    append_suggested_backlog_tasks_from_result(
                        &mut stored.board,
                        &task_id,
                        &parsed,
                    );
                }
                let qa_followup_added = if failed_with_provider_error {
                    false
                } else if should_queue_qa_verdict_retry(
                    &stored.board,
                    &task_id,
                    &parsed,
                    &change_summary,
                ) {
                    queue_qa_verdict_retry(&mut stored.board, &task_id, &parsed)
                } else if is_qa_verdict_retry_task_id(&stored.board, &task_id)
                    && is_missing_final_json_result(&parsed)
                {
                    mark_qa_verdict_retry_blocked(&mut stored.board, &task_id, &parsed);
                    true
                } else if is_qa_task_id(&stored.board, &task_id) && qa_needs_followup(&parsed) {
                    append_followup_task_if_needed(&mut stored.board, &task_id, &parsed)
                } else {
                    false
                };
                let followup_added = if failed_with_provider_error || qa_followup_added {
                    false
                } else {
                    append_followup_task_if_needed(&mut stored.board, &task_id, &parsed)
                };
                if uses_hierarchical_orchestration(&stored.board) {
                    refresh_hierarchy_rollups(&mut stored.board);
                }
                let post_qa_added =
                    if failed_with_provider_error || qa_followup_added || followup_added {
                        false
                    } else {
                        let source_task = stored
                            .board
                            .tasks
                            .iter()
                            .find(|task| task.id == task_id)
                            .cloned();
                        source_task
                            .as_ref()
                            .filter(|task| {
                                task_is_done(task)
                                    && task_needs_immediate_ai_qa(&stored.board, task, &parsed)
                                    && !has_task_qa_for_source(&stored.board, &task.id)
                            })
                            .map(|task| {
                                append_task_qa_task(
                                    &mut stored.board,
                                    task,
                                    "Validate immediately after implementation task completion",
                                )
                            })
                            .unwrap_or(false)
                    };
                let post_agents_added = if failed_with_provider_error
                    || qa_followup_added
                    || followup_added
                    || post_qa_added
                {
                    false
                } else {
                    let source_task = stored
                        .board
                        .tasks
                        .iter()
                        .find(|task| task.id == task_id)
                        .cloned();
                    source_task
                        .as_ref()
                        .filter(|task| {
                            task_is_done(task)
                                && task_needs_agents_knowledge_update(task, &parsed)
                                && !has_agents_knowledge_task_for_source(&stored.board, &task.id)
                        })
                        .map(|task| {
                            append_agents_knowledge_task(
                                &mut stored.board,
                                "Preserve durable code structure, command, database, migration, or verification knowledge for later tasks",
                                Some(task),
                            )
                        })
                        .unwrap_or(false)
                };
                if !qa_followup_added && !followup_added {
                    if let Some(entry) =
                        compact_provider_session_after_task_group(&state, &stored.board, &task_id)
                            .await
                    {
                        stored.board.compaction_ledger.push(entry);
                    }
                }
                if post_agents_added {
                    stored
                        .board
                        .append_log(format!("Inserted post-task AGENTS work after {task_id}"));
                }
                if post_qa_added {
                    stored
                        .board
                        .append_log(format!("Inserted post-task QA work after {task_id}"));
                }
                let completed_for_git = stored
                    .board
                    .tasks
                    .iter()
                    .find(|task| task.id == task_id)
                    .is_some_and(task_is_done);
                if completed_for_git {
                    if let Err(error) =
                        finalize_managed_git_task_group(&mut stored.board, &task_id).await
                    {
                        let message = server_error_message(&error);
                        if let Some(task) = stored
                            .board
                            .tasks
                            .iter_mut()
                            .find(|task| task.id == task_id)
                        {
                            task.status = "blocked".to_string();
                            task.error = Some(message.clone());
                            task.summary = if task.summary.trim().is_empty() {
                                message.clone()
                            } else {
                                format!("{} {}", task.summary, message)
                            };
                        }
                        stored.board.status = "blocked".to_string();
                        stored.board.active = false;
                        stored.board.loop_started = false;
                        stored.board.append_log(format!(
                            "Blocked after completion by managed git policy: {message}"
                        ));
                        stored.board.touch();
                        save_board(&state, &stored.board)?;
                        return Ok(());
                    }
                }
                stored.board.append_log(format!(
                    "Task {task_id} finished with exit code {}",
                    result.exit_code
                ));
            }
            Err(error) => {
                let message = server_error_message(&error);
                if let Some(index) = task_position {
                    let task = &mut stored.board.tasks[index];
                    task.status = "failed".to_string();
                    task.error = Some(message.clone());
                    task.completed_at = Some(now);
                    task.qa_passed = Some(false);
                    task.transcript.push(json!({
                        "timestamp": now,
                        "kind": "error",
                        "isError": true,
                        "content": message,
                    }));
                    task.transcript_updated_at = Some(now);
                }
                finish_task_attempt(
                    &mut stored.board,
                    &task_id,
                    &attempt_id,
                    TASK_STATUS_FAILED,
                    now,
                );
                stored.board.append_log(format!(
                    "Task {task_id} failed: {}",
                    server_error_message(&error)
                ));
            }
        }

        stored.board.current_task_id = None;
        stored.board.current_task_title.clear();
        stored.board.current_task_status.clear();
        stored.board.provider_call_started_at = None;
        stored.board.provider_call_label = None;
        stored.board.current_provider_session_id = None;
        stored.board.touch();
        save_board(&state, &stored.board)?;
    }
}

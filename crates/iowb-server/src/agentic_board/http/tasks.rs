async fn add_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<TaskRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    let mut task = BoardTask::manual(&mut stored.board, request)?;
    validate_manual_task_source(&stored.board, &task)?;
    validate_manual_task_status(&stored.board, &task)?;
    validate_task_dependency_references(&stored.board, &task)?;
    let should_start = task_status_is_todo(&task.status);
    stored.board.tasks.push(task);
    normalize_board_hierarchy(&mut stored.board);
    normalize_board_task_groups(&mut stored.board);
    if let Some(cycle) = dependency_cycle(&stored.board) {
        let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
        let error = planning_error_conflict(&mut stored.board, &cycle, "dependency", issue);
        stored.board.touch();
        save_board(&state, &stored.board)?;
        return Err(error);
    }
    if let Some(issue) = hierarchy_validation_issues(&stored.board)
        .into_iter()
        .next()
    {
        let affected = planning_error_task_ids(&stored.board, &issue);
        let error = planning_error_conflict(&mut stored.board, &affected, "hierarchy", issue);
        stored.board.touch();
        save_board(&state, &stored.board)?;
        return Err(error);
    }
    refresh_hierarchy_rollups(&mut stored.board);
    stored.board.append_log("Added manual board task");
    stored.board.touch();
    save_board(&state, &stored.board)?;
    drop(_guard);
    if should_start {
        let stored = start_board_execution(&state, &user.0.id, &id)?;
        return Ok((
            StatusCode::CREATED,
            Json(
                json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
            ),
        ));
    }
    Ok((
        StatusCode::CREATED,
        Json(
            json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
        ),
    ))
}

async fn draft_tasks(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<PromptRequest>,
) -> Result<Json<Value>> {
    let stored = load_user_board(&state, &user.0.id, &id)?;
    let prompt =
        trim_string(request.prompt.clone()).ok_or_else(|| bad_request("Prompt is required"))?;
    let attempt = generate_prompt_task_drafts(
        &state,
        &stored.board,
        &prompt,
        request.provider.as_deref(),
        request.model.as_deref(),
        request.board_profile.as_deref(),
    )
    .await;
    {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(&state, &user.0.id, &id)?;
        record_prompt_task_generation_attempt(
            &mut stored.board,
            "Kanban task draft preview",
            &attempt,
        );
        stored.board.touch();
        save_board(&state, &stored.board)?;
    }
    let (tasks, warning) = attempt.result?;
    Ok(Json(
        json!({ "success": true, "tasks": tasks, "warning": warning }),
    ))
}

async fn breakdown_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((board_id, task_id)): AxumPath<(String, String)>,
    request: Option<Json<PromptRequest>>,
) -> Result<Json<Value>> {
    let _request = request.map(|Json(request)| request);
    let initial_snapshot = load_user_board(&state, &user.0.id, &board_id)?.board;
    if initial_snapshot.status == "cancelled" {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Resume the board before running a manual breakdown.",
        ));
    }
    let initial_parent = initial_snapshot
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if next_hierarchy_level(task_level(initial_parent)).is_none() {
        return Err(bad_request(
            "Only initiative, epic, story, or task items can be broken down.",
        ));
    }
    let retrying_failed_breakdown = is_retryable_hierarchy_breakdown_task(initial_parent);
    if !matches!(
        canonical_task_status(&initial_parent.status),
        TASK_STATUS_BACKLOG | TASK_STATUS_TODO
    ) && !retrying_failed_breakdown
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Move the planning item to Backlog or Todo before breaking it down.",
        ));
    }

    // Older failed breakdowns were persisted as Blocked and left the board's
    // historical abort marker behind. Convert that legacy attention state
    // back into a retryable planning item before invoking the provider.
    if retrying_failed_breakdown
        || hierarchy_breakdown_planning_error_for(&initial_snapshot, &task_id)
        || initial_snapshot.cancellation_reason.is_some()
        || initial_snapshot.abort_source.is_some()
        || initial_snapshot.abort_requested_at.is_some()
        || initial_snapshot.canceled_at.is_some()
    {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(&state, &user.0.id, &board_id)?;
        if let Some(task) = stored
            .board
            .tasks
            .iter_mut()
            .find(|task| task.id == task_id)
        {
            if retrying_failed_breakdown
                && matches!(
                    canonical_task_status(&task.status),
                    TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
                )
            {
                task.status = TASK_STATUS_BACKLOG.to_string();
                task.error = None;
                task.summary = "Retrying hierarchy breakdown".to_string();
                task.completed_at = None;
            }
        }
        restore_board_after_hierarchy_breakdown_failure(&mut stored.board, &task_id);
        clear_board_abort_state(&mut stored.board);
        stored.board.touch();
        save_board(&state, &stored.board)?;
    }

    let snapshot = load_user_board(&state, &user.0.id, &board_id)?.board;
    let parent = snapshot
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if !matches!(
        canonical_task_status(&parent.status),
        TASK_STATUS_BACKLOG | TASK_STATUS_TODO
    ) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Move the planning item to Backlog or Todo before breaking it down.",
        ));
    }
    validate_hierarchy_breakdown_parent(&snapshot, parent)?;

    let created = plan_hierarchy_children(&state, &user.0.id, &board_id, &task_id, true).await?;
    let stored = load_user_board(&state, &user.0.id, &board_id)?;
    Ok(Json(json!({
        "success": true,
        "created": created,
        "board": stored.board.detail_json(Some(stored.path.display().to_string())),
    })))
}

async fn approve_task_side_effects(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
    request: Option<Json<SideEffectsApprovalRequest>>,
) -> Result<Json<Value>> {
    let request = request.map(|Json(request)| request).unwrap_or_default();
    let approved = request.approved.unwrap_or(true);
    let note = trim_string(request.note);
    let stored = mutate_stored_board(&state, &user.0.id, &id, |run| {
        approve_task_side_effects_in_board(run, &task_id, &user.0.id, approved, note.clone())
    })?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn accept_research(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
    request: Option<Json<ResearchAcceptanceRequest>>,
) -> Result<Json<Value>> {
    let request = request.map(|Json(request)| request).unwrap_or_default();
    let stored = mutate_stored_board(&state, &user.0.id, &id, |run| {
        accept_research_in_board(
            run,
            &task_id,
            &user.0.id,
            request.items.clone(),
            trim_string(request.note.clone()),
        )
    })?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn detach_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    let stored = mutate_stored_board(&state, &user.0.id, &id, |run| {
        detach_user_created_child(run, &task_id)
    })?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn resolve_scope_effects(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
    Json(request): Json<ScopeEffectsResolutionRequest>,
) -> Result<Json<Value>> {
    let decision = request.decision.trim().to_ascii_lowercase();
    let note = trim_string(request.note);
    let stored = mutate_stored_board(&state, &user.0.id, &id, |run| {
        resolve_scope_effects_in_board(run, &task_id, &user.0.id, &decision, note.clone())
    })?;
    Ok(Json(json!({
        "success": true,
        "decision": decision,
        "board": stored.board.detail_json(Some(stored.path.display().to_string())),
    })))
}

async fn backlog_from_prompt(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<PromptRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    let prompt =
        trim_string(request.prompt.clone()).ok_or_else(|| bad_request("Prompt is required"))?;
    let model = trim_string(request.model.clone()).unwrap_or_default();
    let provider = normalize_optional_provider(request.provider.as_deref())?;
    let board_profile = request
        .board_profile
        .as_deref()
        .map(|value| normalize_board_profile(Some(value)))
        .unwrap_or_default();
    let (operation_id, response, effective_provider) = {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(&state, &user.0.id, &id)?;
        let operation_id = Uuid::new_v4().to_string();
        let effective_provider = if provider.trim().is_empty() {
            DEFAULT_BREAKDOWN_PROVIDER.to_string()
        } else {
            provider.trim().to_string()
        };
        let profile = if board_profile.trim().is_empty() {
            normalize_board_profile(Some(&stored.board.board_profile))
        } else {
            normalize_board_profile(Some(&board_profile))
        };
        let started_at = Utc::now();
        stored.board.backlog_breakdown = json!({
            "id": operation_id,
            "status": "running",
            "prompt": prompt,
            "provider": effective_provider,
            "model": model,
            "boardProfile": profile,
            "startedAt": started_at,
            "updatedAt": started_at,
            "transcript": prompt_task_generation_running_transcript(&prompt, &effective_provider, model.as_str(), started_at),
        });
        stored.board.append_log(format!(
            "Started backlog breakdown from prompt: {operation_id}"
        ));
        stored.board.touch();
        save_board(&state, &stored.board)?;
        (
            operation_id.clone(),
            json!({
                "success": true,
                "operationId": operation_id,
                "board": stored.board.detail_json(Some(stored.path.display().to_string())),
            }),
            effective_provider,
        )
    };
    spawn_backlog_prompt_generation(
        state.clone(),
        user.0.id.clone(),
        id,
        operation_id.clone(),
        prompt,
        effective_provider,
        model,
        board_profile,
    );
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn promote_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    let _ = update_task_status(&state, &user.0.id, &id, &[task_id], TASK_STATUS_TODO)?;
    let stored = start_board_execution(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn demote_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    update_task_status(&state, &user.0.id, &id, &[task_id], TASK_STATUS_BACKLOG)
}

async fn retry_attention_tasks(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<RetryTasksRequest>,
) -> Result<Json<Value>> {
    let ids = request.task_ids.unwrap_or_default();
    let mode = normalize_retry_mode(request.mode.as_deref())?;
    let reason = request.reason.unwrap_or_default();
    let fix_task_id = request.fix_task_id;
    mutate_stored_board(&state, &user.0.id, &id, |run| {
        retry_attention_tasks_in_board(run, &ids, mode, fix_task_id.as_deref(), &reason).map(|_| ())
    })?;
    let stored = start_board_execution(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn retry_backlog_breakdown(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>> {
    let (operation_id, prompt, provider, model, board_profile) = {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(&state, &user.0.id, &id)?;
        let breakdown = stored.board.backlog_breakdown.clone();
        if breakdown.get("status").and_then(Value::as_str) != Some(TASK_STATUS_FAILED) {
            return Err(not_found("No failed backlog breakdown to retry"));
        }
        let prompt = breakdown
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| bad_request("Failed backlog breakdown has no prompt to retry"))?;
        let model = breakdown
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default();
        let provider = breakdown
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| stored.board.provider.clone());
        let board_profile = breakdown
            .get("boardProfile")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default();
        let operation_id = Uuid::new_v4().to_string();
        let started_at = Utc::now();
        stored.board.backlog_breakdown = json!({
            "id": operation_id,
            "status": "running",
            "prompt": prompt,
            "provider": provider,
            "model": model,
            "boardProfile": board_profile,
            "startedAt": started_at,
            "updatedAt": started_at,
            "retryOf": breakdown.get("id").and_then(Value::as_str).unwrap_or_default(),
            "transcript": prompt_task_generation_running_transcript(&prompt, &provider, &model, started_at),
        });
        clear_board_abort_state(&mut stored.board);
        stored
            .board
            .append_log(format!("Retrying failed backlog breakdown: {operation_id}"));
        stored.board.touch();
        save_board(&state, &stored.board)?;
        (operation_id, prompt, provider, model, board_profile)
    };
    spawn_backlog_prompt_generation(
        state.clone(),
        user.0.id.clone(),
        id.clone(),
        operation_id,
        prompt,
        provider,
        model,
        board_profile,
    );
    let stored = load_user_board(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

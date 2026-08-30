async fn update_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
    Json(request): Json<UpdateTaskRequest>,
) -> Result<Json<Value>> {
    let has_scope_edit = request.title.is_some()
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
        || request.side_effects.is_some();
    if has_scope_edit {
        mutate_stored_board(&state, &user.0.id, &id, |run| {
            edit_backlog_task(run, &task_id, &request)
        })?;
    }
    if !has_scope_edit && (request.priority.is_some() || request.rank.is_some()) {
        mutate_stored_board(&state, &user.0.id, &id, |run| {
            edit_task_priority_rank(run, &task_id, &request)
        })?;
    }
    let status = request
        .status
        .as_deref()
        .map(|value| normalize_task_status(Some(value), ""))
        .transpose()?;
    if let Some(status) = status.as_deref() {
        let _ = update_task_status(&state, &user.0.id, &id, &[task_id], status)?;
    }
    if status.as_deref() == Some(TASK_STATUS_TODO) {
        let stored = start_board_execution(&state, &user.0.id, &id)?;
        return Ok(Json(
            json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
        ));
    }
    let stored = load_user_board(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn discuss_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
    Json(request): Json<DiscussionRequest>,
) -> Result<Json<Value>> {
    let message = trim_string(request.message).unwrap_or_default();
    let requested_action = normalize_discussion_action(request.action.as_deref());
    if message.is_empty() && requested_action.is_empty() {
        return Err(bad_request("Discussion message or action is required."));
    }
    let proposal_id = Uuid::new_v4().to_string();
    let (snapshot, provider, model, started_at) = {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(&state, &user.0.id, &id)?;
        let snapshot = stored.board.clone();
        if !snapshot.tasks.iter().any(|task| task.id == task_id) {
            return Err(not_found("Agentic board task not found"));
        }
        let provider = effective_provider_for_phase(&snapshot, "discussion proposal")?;
        let model = effective_model_for_phase(&snapshot, "discussion proposal");
        let started_at = Utc::now();
        let entry = discussion_running_entry(
            &proposal_id,
            &task_id,
            &message,
            &requested_action,
            request.payload.as_ref().unwrap_or(&Value::Null),
            provider.as_str(),
            model.as_str(),
            started_at,
        );
        append_discussion_proposal(&mut stored.board, &task_id, entry)?;
        stored.board.append_log(format!(
            "Started discussion proposal {proposal_id} for board item {task_id}"
        ));
        stored.board.touch();
        save_board(&state, &stored.board)?;
        (snapshot, provider, model, started_at)
    };

    let prompt = build_discussion_proposal_prompt(
        &snapshot,
        &task_id,
        &message,
        &requested_action,
        request.payload.as_ref().unwrap_or(&Value::Null),
    )?;
    let provider_result = execute_internal_prompt(
        &state,
        &user.0.id,
        &id,
        &format!("discussion proposal for {task_id}"),
        &prompt,
    )
    .await;

    let _guard = board_mutation_lock();
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    let proposal_index = stored
        .board
        .discussion_proposals
        .iter()
        .position(|proposal| {
            proposal.get("id").and_then(Value::as_str) == Some(proposal_id.as_str())
        })
        .ok_or_else(|| not_found("Discussion proposal no longer exists"))?;
    let mut proposal = stored.board.discussion_proposals[proposal_index].clone();
    let assistant_content = match provider_result {
        Ok(output) => output,
        Err(error) => {
            let error_text = server_error_message(&error);
            mark_discussion_proposal_failed(
                &mut proposal,
                &error_text,
                &provider,
                &model,
                started_at,
            );
            update_discussion_proposal(&mut stored.board, &task_id, proposal);
            stored.board.append_log(format!(
                "Discussion proposal {proposal_id} failed: {error_text}"
            ));
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(Json(json!({
                "success": false,
                "proposalId": proposal_id,
                "proposal": stored.board.discussion_proposals[proposal_index],
                "board": stored.board.detail_json(Some(stored.path.display().to_string())),
            })));
        }
    };
    let parsed = parse_json_object(&assistant_content).ok_or_else(|| {
        bad_request(format!(
            "Discussion provider returned malformed JSON: {}",
            limit_text(&assistant_content, 800)
        ))
    });
    let proposal_result = parsed.and_then(|parsed| {
        sanitize_discussion_proposal(
            &stored.board,
            &task_id,
            &proposal_id,
            &requested_action,
            &message,
            request.payload.as_ref().unwrap_or(&Value::Null),
            &parsed,
            &provider,
            &model,
            started_at,
        )
    });
    match proposal_result {
        Ok(mut completed) => {
            if let Some(object) = completed.as_object_mut() {
                object.insert(
                    "transcript".to_string(),
                    discussion_completed_transcript(
                        &message,
                        &assistant_content,
                        &provider,
                        &model,
                        started_at,
                    ),
                );
            }
            update_discussion_proposal(&mut stored.board, &task_id, completed);
            stored.board.append_log(format!(
                "Discussion proposal {proposal_id} is pending explicit approval"
            ));
        }
        Err(error) => {
            let error_text = server_error_message(&error);
            mark_discussion_proposal_failed(
                &mut proposal,
                &error_text,
                &provider,
                &model,
                started_at,
            );
            proposal["transcript"] = discussion_completed_transcript(
                &message,
                &assistant_content,
                &provider,
                &model,
                started_at,
            );
            update_discussion_proposal(&mut stored.board, &task_id, proposal);
            stored.board.append_log(format!(
                "Discussion proposal {proposal_id} could not be prepared: {error_text}"
            ));
        }
    }
    stored.board.touch();
    save_board(&state, &stored.board)?;
    let proposal = stored.board.discussion_proposals[proposal_index].clone();
    Ok(Json(json!({
        "success": proposal.get("status").and_then(Value::as_str) == Some("pending"),
        "proposalId": proposal_id,
        "proposal": proposal,
        "board": stored.board.detail_json(Some(stored.path.display().to_string())),
    })))
}

async fn apply_discussion_proposal(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id, proposal_id)): AxumPath<(String, String, String)>,
) -> Result<Json<Value>> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    let index = stored
        .board
        .discussion_proposals
        .iter()
        .position(|proposal| {
            proposal.get("id").and_then(Value::as_str) == Some(proposal_id.as_str())
                && proposal.get("taskId").and_then(Value::as_str) == Some(task_id.as_str())
        })
        .ok_or_else(|| not_found("Discussion proposal not found"))?;
    let proposal = stored.board.discussion_proposals[index].clone();
    if proposal.get("status").and_then(Value::as_str) != Some("pending") {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Only pending discussion proposals can be applied.",
        ));
    }
    let action = proposal
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("message");
    let payload = proposal
        .get("payload")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let before = discussion_scope_snapshot(&stored.board, &task_id);
    if let Err(error) = apply_discussion_action(&mut stored.board, &task_id, action, &payload) {
        if has_persisted_planning_error(&stored.board) {
            stored.board.touch();
            save_board(&state, &stored.board)?;
        }
        return Err(error);
    }
    let after = discussion_scope_snapshot(&stored.board, &task_id);
    let diff = discussion_diff(&before, &after);
    let mut applied = proposal;
    if let Some(object) = applied.as_object_mut() {
        object.insert("status".to_string(), json!("applied"));
        object.insert("appliedAt".to_string(), json!(Utc::now()));
        object.insert("before".to_string(), before);
        object.insert("after".to_string(), after);
        object.insert("diff".to_string(), diff);
    }
    update_discussion_proposal(&mut stored.board, &task_id, applied);
    refresh_hierarchy_rollups(&mut stored.board);
    stored.board.append_log(format!(
        "Applied discussion proposal {proposal_id} to {task_id}"
    ));
    stored.board.touch();
    save_board(&state, &stored.board)?;
    Ok(Json(json!({
        "success": true,
        "proposalId": proposal_id,
        "board": stored.board.detail_json(Some(stored.path.display().to_string())),
    })))
}

async fn reject_discussion_proposal(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id, proposal_id)): AxumPath<(String, String, String)>,
) -> Result<Json<Value>> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    let index = stored
        .board
        .discussion_proposals
        .iter()
        .position(|proposal| {
            proposal.get("id").and_then(Value::as_str) == Some(proposal_id.as_str())
                && proposal.get("taskId").and_then(Value::as_str) == Some(task_id.as_str())
        })
        .ok_or_else(|| not_found("Discussion proposal not found"))?;
    let mut rejected = stored.board.discussion_proposals[index].clone();
    if rejected.get("status").and_then(Value::as_str) != Some("pending") {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Only pending discussion proposals can be rejected.",
        ));
    }
    if let Some(object) = rejected.as_object_mut() {
        object.insert("status".to_string(), json!("rejected"));
        object.insert("rejectedAt".to_string(), json!(Utc::now()));
    }
    update_discussion_proposal(&mut stored.board, &task_id, rejected);
    stored.board.append_log(format!(
        "Rejected discussion proposal {proposal_id} for {task_id}"
    ));
    stored.board.touch();
    save_board(&state, &stored.board)?;
    Ok(Json(json!({
        "success": true,
        "proposalId": proposal_id,
        "board": stored.board.detail_json(Some(stored.path.display().to_string())),
    })))
}

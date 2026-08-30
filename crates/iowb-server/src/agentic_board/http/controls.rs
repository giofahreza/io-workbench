async fn pause_board(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    request: Option<Json<PauseRequest>>,
) -> Result<Json<Value>> {
    let request = request
        .map(|Json(request)| request)
        .unwrap_or(PauseRequest { reason: None });
    mutate_board(&state, &user.0.id, &id, |run| {
        request_board_pause(run, trim_string(request.reason));
        Ok(())
    })
}

async fn start_board(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>> {
    let body = body.map(|Json(body)| body).unwrap_or_else(|| json!({}));
    let _ = mutate_board(&state, &user.0.id, &id, |run| {
        if let Some(provider) = body.get("provider").and_then(Value::as_str) {
            run.provider = normalize_provider(Some(provider))?;
        }
        if let Some(model) = body
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            run.model = model.to_string();
        }
        if let Some(model) = body.get("nextModel").and_then(Value::as_str).map(str::trim) {
            run.next_model = model.to_string();
        }
        if let Some(provider) = body.get("nextProvider").and_then(Value::as_str) {
            run.next_provider = normalize_optional_provider(Some(provider))?;
        }
        prepare_board_resume(run);
        Ok(())
    })?;
    let stored = start_board_execution(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn schedule_board(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ScheduleRequest>,
) -> Result<Json<Value>> {
    let scheduled_start_at = trim_string(request.scheduled_start_at);
    let Some(scheduled_start_at) = scheduled_start_at else {
        return mutate_board(&state, &user.0.id, &id, |run| {
            clear_board_schedule(run);
            clear_board_abort_state(run);
            Ok(())
        });
    };
    let scheduled_start_at = parse_rfc3339_utc(&scheduled_start_at)
        .ok_or_else(|| bad_request("Scheduled start time is invalid"))?;
    if scheduled_start_at <= Utc::now() {
        let stored = start_board_execution(&state, &user.0.id, &id)?;
        return Ok(Json(
            json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
        ));
    }
    mutate_board(&state, &user.0.id, &id, |run| {
        if run.loop_started || run.active || run.status == "running" {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "Pause the active board before scheduling a future start.",
            ));
        }
        run.status = "scheduled".to_string();
        run.scheduled_start_at = Some(scheduled_start_at);
        run.auto_run_enabled = true;
        run.pause_requested = false;
        run.paused_at = None;
        run.pause_reason = None;
        clear_board_abort_state(run);
        run.current_provider_session_id = None;
        run.provider_call_started_at = None;
        run.provider_call_label = None;
        bump_control_revision(run);
        run.append_log(format!("Board scheduled to start at {scheduled_start_at}"));
        Ok(())
    })
}

fn clear_board_schedule(run: &mut AgenticBoard) {
    bump_control_revision(run);
    run.scheduled_start_at = None;
    if run.status == "scheduled" {
        run.status = "paused".to_string();
        run.active = false;
        run.loop_started = false;
        run.auto_run_enabled = false;
        run.pause_requested = false;
        run.paused_at = Some(Utc::now());
        run.pause_reason = Some("schedule cleared".to_string());
    }
    run.append_log("Cleared board scheduled start");
}

async fn abort_board(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    request: Option<Json<PauseRequest>>,
) -> Result<Json<Value>> {
    let request = request
        .map(|Json(request)| request)
        .unwrap_or(PauseRequest { reason: None });
    let reason = trim_string(request.reason).unwrap_or_else(|| "user request".to_string());
    mutate_board(&state, &user.0.id, &id, |run| {
        let now = Utc::now();
        bump_control_revision(run);
        reset_in_flight_board_tasks(run, "Task returned to Todo because the board was aborted");
        run.status = "cancelled".to_string();
        run.active = false;
        run.loop_started = false;
        run.cancellation_reason = Some(reason);
        run.abort_source = Some("Board".to_string());
        run.abort_requested_at = Some(now);
        run.canceled_at = Some(now);
        run.current_task_id = None;
        run.current_task_title.clear();
        run.current_task_status.clear();
        run.current_provider_session_id = None;
        run.provider_call_started_at = None;
        run.provider_call_label = None;
        run.append_log("Board aborted");
        Ok(())
    })
}

async fn update_model(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<UpdateModelRequest>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        if let Some(provider) = request.provider.as_deref() {
            run.provider = normalize_provider(Some(provider))?;
        }
        if let Some(model) = trim_string(request.model) {
            let previous = run.model.clone();
            run.model = model.clone();
            run.primary_model = model.clone();
            run.model_history.push(json!({
                "from": previous,
                "to": model,
                "changedAt": Utc::now(),
                "changedBy": "Agentic workspace",
            }));
        }
        if let Some(model) = request.next_model {
            run.next_model = model.trim().to_string();
        }
        if let Some(provider) = request.next_provider {
            run.next_provider = normalize_optional_provider(Some(&provider))?;
        }
        run.append_log("Updated board model");
        Ok(())
    })
}

async fn update_model_strategy(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        let strategy_patch = body
            .get("modelStrategy")
            .cloned()
            .or_else(|| body_has_model_strategy_keys(&body).then(|| body.clone()));
        let strategy_was_patched = strategy_patch.is_some();
        if strategy_was_patched {
            run.model_strategy = normalize_model_strategy(strategy_patch);
        }
        if let Some(profile) = body.get("boardProfile").and_then(Value::as_str) {
            run.board_profile = normalize_board_profile(Some(profile));
        }
        let strategy_overrides = task_model_overrides_for_strategy(run.model_strategy.as_ref());
        if let Some(overrides) = body.get("taskModelOverrides").cloned() {
            run.task_model_overrides = merge_task_model_overrides(
                strategy_overrides,
                normalize_task_model_overrides(overrides),
            );
        } else if !json_object_is_empty(&strategy_overrides) {
            run.task_model_overrides =
                merge_task_model_overrides(strategy_overrides, run.task_model_overrides.clone());
        }
        if let Some(model) = primary_model_for_strategy(run.model_strategy.as_ref()) {
            if run.primary_model.trim().is_empty() || strategy_was_patched {
                run.primary_model = model.clone();
                run.model = model;
            }
        }
        if let Some(policy) = body.get("sessionPolicy").and_then(Value::as_str) {
            run.session_policy = normalize_session_policy(Some(policy));
        }
        sync_session_policy_with_task_models(run, "model strategy update");
        if let Some(policy) = body.get("gitPolicy").and_then(Value::as_str) {
            run.git_policy = normalize_git_policy(Some(policy));
        }
        if let Some(model) = body.get("nextModel").and_then(Value::as_str) {
            run.next_model = model.trim().to_string();
        }
        if let Some(provider) = body.get("nextProvider").and_then(Value::as_str) {
            run.next_provider = normalize_optional_provider(Some(provider))?;
        }
        run.append_log("Updated board model strategy");
        Ok(())
    })
}

async fn update_git_policy(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        let policy = body
            .get("gitPolicy")
            .or_else(|| body.get("policy"))
            .and_then(Value::as_str);
        run.git_policy = normalize_git_policy(policy);
        run.append_log("Updated board git policy");
        Ok(())
    })
}

async fn update_tools_settings(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        run.tools_settings = body.get("toolsSettings").cloned().or_else(|| Some(body));
        run.append_log("Updated board tool settings");
        Ok(())
    })
}

async fn update_tdd_settings(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        if let Some(enabled) = body
            .get("tddEnabled")
            .or_else(|| body.get("enabled"))
            .and_then(Value::as_bool)
        {
            run.tdd_enabled = enabled;
        }
        let patch = body
            .get("tddPolicy")
            .or_else(|| body.get("policy"))
            .cloned()
            .unwrap_or_else(|| body.clone());
        let merged = merge_json_objects(run.tdd_policy.clone(), patch);
        run.tdd_policy = normalize_tdd_policy(Some(&merged));
        run.append_log("Updated board TDD policy");
        Ok(())
    })
}

async fn update_validation_config(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        let patch = body
            .get("validationConfig")
            .or_else(|| body.get("config"))
            .cloned()
            .unwrap_or_else(|| body.clone());
        let merged = merge_json_objects(run.validation_config.clone(), patch);
        run.validation_config = normalize_validation_config(Some(&merged));
        run.append_log("Updated board validation config");
        Ok(())
    })
}

async fn update_rag_settings(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        let patch = body
            .get("ragSettings")
            .or_else(|| body.get("settings"))
            .cloned()
            .unwrap_or_else(|| body.clone());
        let merged = merge_json_objects(run.rag_settings.clone(), patch);
        run.rag_settings = normalize_rag_settings(Some(&merged));
        run.rag_enabled = rag_enabled_from_settings(&run.rag_settings);
        run.append_log("Updated board RAG settings");
        Ok(())
    })
}

async fn update_qa_policy(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        let patch = body
            .get("qaPolicy")
            .or_else(|| body.get("policy"))
            .cloned()
            .unwrap_or_else(|| body.clone());
        let merged = merge_json_objects(run.qa_policy.clone(), patch);
        run.qa_policy = normalize_qa_policy(Some(&merged));
        run.append_log("Updated board QA policy");
        Ok(())
    })
}

async fn update_task_models(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        run.task_model_overrides = normalize_task_model_overrides(
            body.get("taskModelOverrides")
                .or_else(|| body.get("models"))
                .cloned()
                .unwrap_or(body),
        );
        sync_session_policy_with_task_models(run, "task model update");
        run.append_log("Updated board task models");
        Ok(())
    })
}

async fn update_auto_retry(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        let mut next = run.auto_retry.as_object().cloned().unwrap_or_default();
        for key in ["enabled", "delayMinutes", "maxAttempts", "resetAttempts"] {
            if let Some(value) = body.get(key) {
                if key == "resetAttempts" && value.as_bool() == Some(true) {
                    next.insert("attempts".to_string(), json!(0));
                } else if key != "resetAttempts" {
                    next.insert(key.to_string(), value.clone());
                }
            }
        }
        next.insert("updatedAt".to_string(), json!(Utc::now()));
        run.auto_retry = Value::Object(next);
        run.append_log("Updated board auto retry");
        Ok(())
    })
}

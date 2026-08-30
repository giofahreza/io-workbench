async fn create_board(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(mut request): Json<CreateBoardRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    let project_path = trim_string(request.project_path.clone())
        .ok_or_else(|| bad_request("Project path is required"))?;
    let project_path = state
        .path_validator
        .validate_path(PathBuf::from(project_path), false)
        .await?;
    let metadata = tokio::fs::metadata(&project_path)
        .await
        .map_err(FsError::Io)?;
    if !metadata.is_dir() {
        return Err(bad_request("Project path must be a directory"));
    }
    request.project_path = Some(project_path.display().to_string());

    let request_has_schedule = trim_string(request.scheduled_start_at.clone()).is_some();
    let scheduled_start_at = parse_optional_scheduled_start(request.scheduled_start_at.as_deref())?;
    let should_schedule = scheduled_start_at.is_some_and(|time| time > Utc::now());
    let (board_id, reused) = {
        let _guard = board_mutation_lock();
        let mut reused_board_id = None;
        if let Some(project_path) = trim_string(request.project_path.clone())
            && let Some(mut latest) = latest_board_for_project(&state, &user.0.id, &project_path)?
        {
            let board_was_active = latest.board.loop_started
                || latest.board.active
                || latest.board.status == "running";
            if should_schedule && board_was_active {
                return Err(ServerError::new(
                    StatusCode::CONFLICT,
                    "Pause the active board before scheduling a future start.",
                ));
            }
            apply_board_options(&mut latest.board, &request)?;
            let mut task = BoardTask::manual(
                &mut latest.board,
                TaskRequest {
                    prompt: request.command.clone().or(request.prompt.clone()),
                    command: None,
                    title: request.title.clone(),
                    details: request.details.clone(),
                    description: request.description.clone(),
                    kind: None,
                    task_type: None,
                    level: None,
                    parent_id: None,
                    blocked_by: None,
                    executable: None,
                    required: None,
                    source_task_id: None,
                    scope_version: None,
                    planned_files: None,
                    side_effects: None,
                    acceptance_criteria: None,
                    acceptance: None,
                    criteria: None,
                    references: None,
                    files: None,
                    paths: None,
                    priority: None,
                    rank: None,
                    depends_on: None,
                    dependencies: None,
                    status: Some(TASK_STATUS_TODO.to_string()),
                },
            )?;
            latest.board.tasks.push(task);
            let should_preserve_schedule = !should_schedule
                && !request_has_schedule
                && latest.board.status == "scheduled"
                && latest.board.scheduled_start_at.is_some();
            if should_schedule {
                latest.board.status = "scheduled".to_string();
                latest.board.active = false;
                latest.board.loop_started = false;
                latest.board.auto_run_enabled = false;
                latest.board.scheduled_start_at = scheduled_start_at;
                latest.board.paused_at = None;
                latest.board.pause_reason = None;
            } else if board_was_active {
                latest.board.status = "running".to_string();
                latest.board.active = true;
                latest.board.auto_run_enabled = true;
                latest.board.scheduled_start_at = None;
                latest.board.paused_at = None;
                latest.board.pause_reason = None;
            } else if should_preserve_schedule {
                latest.board.status = "scheduled".to_string();
                latest.board.active = false;
                latest.board.loop_started = false;
                latest.board.auto_run_enabled = false;
                latest.board.paused_at = None;
                latest.board.pause_reason = None;
            } else {
                latest.board.status = "paused".to_string();
                latest.board.active = false;
                latest.board.loop_started = false;
                latest.board.auto_run_enabled = false;
                latest.board.scheduled_start_at = None;
                latest.board.paused_at = Some(Utc::now());
                latest.board.pause_reason = Some("New task added to board.".to_string());
            }
            latest
                .board
                .append_log("Added a task to the project board from start request");
            latest.board.touch();
            let board_id = latest.board.id.clone();
            save_board(&state, &latest.board)?;
            reused_board_id = Some(board_id);
        }

        if let Some(board_id) = reused_board_id {
            (board_id, true)
        } else {
            let run = AgenticBoard::new(Some(user.0.id.clone()), request)?;
            let board_id = run.id.clone();
            save_board(&state, &run)?;
            (board_id, false)
        }
    };
    let stored = load_user_board(&state, &user.0.id, &board_id)?;
    Ok((
        if should_schedule {
            StatusCode::ACCEPTED
        } else if reused {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(
            json!({ "success": true, "reused": reused, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
        ),
    ))
}

async fn list_boards(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<BoardsQuery>,
) -> Result<Json<Value>> {
    let project_path = trim_string(query.project_path);
    let mut boards = load_boards(&state)?
        .into_iter()
        .filter(|stored| stored.board.user_id.as_deref() == Some(&user.0.id))
        .filter(|stored| {
            project_path
                .as_deref()
                .is_none_or(|path| stored.board.project_path == path)
        })
        .collect::<Vec<_>>();
    for stored in &mut boards {
        backfill_board_session_links(&state, &mut stored.board).await?;
    }
    boards.sort_by(|left, right| {
        right
            .board
            .updated_at
            .cmp(&left.board.updated_at)
            .then_with(|| right.board.id.cmp(&left.board.id))
    });
    let mut seen = BTreeMap::<String, ()>::new();
    boards.retain(|stored| seen.insert(stored.board.project_path.clone(), ()).is_none());
    let boards = boards
        .into_iter()
        .map(|stored| {
            stored
                .board
                .summary_json(Some(stored.path.display().to_string()))
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "success": true,
        "boards": boards,
    })))
}

async fn get_board(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>> {
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    backfill_board_session_links(&state, &mut stored.board).await?;
    Ok(Json(
        json!({ "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

/// Lazily classify sessions created before board-session metadata existed.
/// This intentionally runs only while serving a board list/detail request;
/// ordinary session reads remain read-only and never scan board snapshots.
async fn backfill_board_session_links(state: &AppState, run: &mut AgenticBoard) -> Result<()> {
    let mut changed_run = false;
    for (session_id, task_id) in known_board_session_refs(run) {
        let Some(session) = state.storage.get_session_summary(&session_id)? else {
            continue;
        };
        if session.board_id.as_deref().is_some_and(|id| id != run.id) {
            continue;
        }
        state
            .sessions
            .mark_board_session(&session_id, run.id.clone(), task_id.clone())
            .await?;
        if let Some(task_id) = task_id
            && let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id)
            && task_needs_legacy_session_backfill(task)
        {
            task.provider_session_id = Some(session_id);
            changed_run = true;
        }
    }

    let window_start = run.created_at - chrono::Duration::minutes(2);
    let window_end = run.updated_at + chrono::Duration::minutes(2);
    let sessions = state
        .storage
        .list_sessions_including_board()?
        .into_iter()
        .filter(|session| session.project_path == run.project_path)
        .filter(|session| {
            session.last_activity >= window_start && session.last_activity <= window_end
        })
        .collect::<Vec<_>>();
    for session in sessions {
        if session.board_id.as_deref().is_some_and(|id| id != run.id) {
            continue;
        }
        let messages = state.storage.list_messages(&session.id)?;
        let Some(first_user_message) = messages
            .iter()
            .find(|message| message.role == MessageRole::User)
        else {
            continue;
        };
        let prompt = first_user_message.content.as_str();
        let explicit_board = prompt.contains(&format!("Board id: {}", run.id));
        let signature = legacy_board_prompt_signature(prompt);
        if !explicit_board && !signature {
            continue;
        }
        // Signature-only prompts (bootstrap/planning) do not carry a board id.
        // Require proximity to one of this board's recorded provider calls so a
        // normal chat quoting board terminology cannot be classified.
        if !explicit_board
            && !legacy_prompt_matches_board_telemetry(run, first_user_message.timestamp)
        {
            continue;
        }
        let task_id = legacy_board_task_id(run, prompt);
        if !session.is_board_session()
            || session.board_id.as_deref() != Some(run.id.as_str())
            || (task_id.is_some() && session.board_task_id.as_deref() != task_id.as_deref())
        {
            state
                .sessions
                .mark_board_session(&session.id, run.id.clone(), task_id.clone())
                .await?;
        }
        if let Some(task_id) = task_id
            && let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id)
            && task_needs_legacy_session_backfill(task)
        {
            task.provider_session_id = Some(session.id.clone());
            changed_run = true;
        }
    }
    if changed_run {
        // The lazy migration is a deliberate compatibility write. Avoid
        // touching updated_at/control state so loading an old board does not
        // appear as user activity.
        save_board(state, run)?;
    }
    Ok(())
}

fn task_allows_legacy_session_backfill(task: &BoardTask) -> bool {
    matches!(
        canonical_task_status(&task.status),
        TASK_STATUS_BLOCKED | TASK_STATUS_FAILED | TASK_STATUS_DONE
    )
}

fn task_needs_legacy_session_backfill(task: &BoardTask) -> bool {
    task_allows_legacy_session_backfill(task)
        && task
            .provider_session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
}

fn known_board_session_refs(run: &AgenticBoard) -> Vec<(String, Option<String>)> {
    let mut refs = Vec::<(String, Option<String>)>::new();
    for session_id in [
        run.session_id.as_deref(),
        run.actual_session_id.as_deref(),
        run.current_provider_session_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    {
        upsert_known_board_session_ref(&mut refs, session_id.to_string(), None);
    }
    for task in &run.tasks {
        if let Some(session_id) = task
            .provider_session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            upsert_known_board_session_ref(
                &mut refs,
                session_id.to_string(),
                Some(task.id.clone()),
            );
        }
    }
    for entry in run.prompt_telemetry.iter().rev() {
        let Some(session_id) = entry
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let task_id = entry
            .get("label")
            .and_then(Value::as_str)
            .and_then(|label| board_task_id_for_label(run, label));
        upsert_known_board_session_ref(&mut refs, session_id.to_string(), task_id);
    }
    for artifact in run.qa_artifacts.iter().rev() {
        let Some(session_id) = artifact
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let task_id = artifact
            .get("taskId")
            .and_then(Value::as_str)
            .filter(|task_id| run.tasks.iter().any(|task| task.id == *task_id))
            .map(str::to_string);
        upsert_known_board_session_ref(&mut refs, session_id.to_string(), task_id);
    }
    refs
}

fn upsert_known_board_session_ref(
    refs: &mut Vec<(String, Option<String>)>,
    session_id: String,
    task_id: Option<String>,
) {
    if let Some((_, existing_task_id)) = refs
        .iter_mut()
        .find(|(existing_session_id, _)| existing_session_id == &session_id)
    {
        if existing_task_id.is_none() {
            *existing_task_id = task_id;
        }
        return;
    }
    refs.push((session_id, task_id));
}

fn legacy_board_prompt_signature(prompt: &str) -> bool {
    [
        "autonomous Kanban agent",
        "before Kanban planning",
        "io-workbench Kanban board",
        "io-workbench Kanban board worker",
        "agentic Kanban task result",
        "RAG promotion candidates for io-workbench",
        "autonomous Kanban board",
    ]
    .iter()
    .any(|signature| prompt.contains(signature))
}

fn legacy_prompt_matches_board_telemetry(run: &AgenticBoard, timestamp: DateTime<Utc>) -> bool {
    run.prompt_telemetry.iter().any(|entry| {
        entry
            .get("startedAt")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_utc)
            .is_some_and(|started_at| (timestamp - started_at).num_seconds().unsigned_abs() <= 30)
    })
}

fn legacy_board_task_id(run: &AgenticBoard, prompt: &str) -> Option<String> {
    run.tasks
        .iter()
        .filter(|task| {
            let marker = format!(": {}", task.id);
            prompt
                .lines()
                .any(|line| line.trim_start().starts_with("Task ") && line.ends_with(&marker))
                || prompt.contains(&format!("task result into the required JSON contract"))
                    && prompt.contains(&format!("Task id: {}", task.id))
        })
        .max_by_key(|task| task.id.len())
        .map(|task| task.id.clone())
}

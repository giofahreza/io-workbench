// The durable chat worker can resume independently at startup. Its board
// waiter cannot: validation and workspace attribution held local state.
// Keep project ownership until that provider stops, then expose a fresh,
// explicit retry. Never schedule another invocation or trust its unvalidated
// final response merely because the durable chat reports completion.
static BOARD_RESTART_RECOVERIES: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();

fn interrupted_execution_task_ids(run: &AgenticBoard) -> Vec<String> {
    run.tasks
        .iter()
        .filter(|task| {
            task_status_is_active(&task.status)
                && (!uses_hierarchical_orchestration(run) || task_is_executable(task))
        })
        .map(|task| task.id.clone())
        .collect()
}

fn restart_recovery_requires_explicit_retry(run: &AgenticBoard) -> bool {
    run.current_phase.as_deref() == Some("interrupted_execution")
}

fn ensure_restart_recovery_finished(board_id: &str) -> Result<()> {
    let recoveries = BOARD_RESTART_RECOVERIES.get_or_init(|| Mutex::new(BTreeSet::new()));
    let recovering = recoveries.lock().map_err(|_| {
        ServerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Board recovery ownership lock is unavailable.",
        )
    })?;
    if recovering.contains(board_id) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Board restart recovery is waiting for its previous provider execution to stop. Refresh before retrying.",
        ));
    }
    Ok(())
}

struct BoardRestartRecoveryGuard {
    board_id: String,
    project_path: String,
    settled: bool,
}

impl Drop for BoardRestartRecoveryGuard {
    fn drop(&mut self) {
        if !self.settled {
            // A storage/recovery failure is not proof the provider stopped.
            // Fail closed rather than release ownership for a second turn.
            tracing::warn!(board_id = %self.board_id, "board restart recovery did not settle; retaining execution ownership until recovery");
            return;
        }
        if let Some(recoveries) = BOARD_RESTART_RECOVERIES.get()
            && let Ok(mut recoveries) = recoveries.lock()
        {
            recoveries.remove(&self.board_id);
        }
        release_project_execution(&self.project_path, &self.board_id);
    }
}

fn close_interrupted_attempts(
    task: &mut BoardTask,
    status: &str,
    message: &str,
    now: DateTime<Utc>,
) {
    for attempt in &mut task.hierarchy.attempts {
        if attempt.get("status").and_then(Value::as_str) != Some("running")
            || attempt
                .get("finishedAt")
                .is_some_and(|value| !value.is_null())
        {
            continue;
        }
        let duration = attempt
            .get("startedAt")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_utc)
            .map(|started| (now - started).num_milliseconds().max(0));
        if let Some(object) = attempt.as_object_mut() {
            object.insert("status".to_string(), json!(status));
            object.insert("finishedAt".to_string(), json!(now));
            object.insert("durationMs".to_string(), json!(duration));
            object.insert("error".to_string(), json!(message));
            object.insert("summary".to_string(), json!(message));
            object.insert("interrupted".to_string(), json!(true));
            // Unknown is not an empty, verified change set. Do not copy a
            // previous attempt's commands, artifacts, or validation here.
            object.insert("workspaceAttributionUnavailable".to_string(), json!(true));
        }
    }
}

fn settle_interrupted_board_tasks(run: &mut AgenticBoard, task_ids: &[String]) -> usize {
    let now = Utc::now();
    let message = "Provider execution was interrupted by a server restart. The previous provider has stopped, but board validation did not finish. Review its chat and workspace, then retry this task; no completion has been accepted.";
    let mut count = 0;
    for task in &mut run.tasks {
        if !task_ids.contains(&task.id) || !task_status_is_active(&task.status) {
            continue;
        }
        close_interrupted_attempts(task, TASK_STATUS_BLOCKED, message, now);
        task.status = TASK_STATUS_BLOCKED.to_string();
        task.completed_at = Some(now);
        task.error = Some(message.to_string());
        task.summary = message.to_string();
        task.result = Some(json!({"status": "blocked", "summary": message, "interrupted": true}));
        task.commands_run.clear();
        task.evidence = vec![message.to_string()];
        task.changed_file_summary = None;
        task.transcript.push(json!({
            "timestamp": now, "kind": "error", "isError": true,
            "status": "blocked", "content": message,
        }));
        task.transcript_updated_at = Some(now);
        count += 1;
    }
    if count > 0 {
        run.status = TASK_STATUS_BLOCKED.to_string();
        run.pause_reason = Some(message.to_string());
        run.current_phase = Some("interrupted_execution".to_string());
        run.append_log(format!("Recovered {count} interrupted task(s); previous provider stopped, explicit retry required"));
    } else if matches!(run.status.as_str(), "running" | "pausing") {
        run.status = "paused".to_string();
        run.pause_reason = Some(
            "Previous provider stopped after restart recovery; resume when ready.".to_string(),
        );
        run.current_phase = Some("paused".to_string());
    }
    run.active = false;
    run.loop_started = false;
    run.pause_requested = false;
    run.phase_details =
        Some(json!({"taskIds": task_ids, "providerStopped": true, "retryRequired": count > 0}));
    run.current_task_id = None;
    run.current_task_title.clear();
    run.current_task_status.clear();
    run.current_provider_session_id = None;
    run.provider_call_started_at = None;
    run.provider_call_label = None;
    refresh_hierarchy_rollups(run);
    count
}

fn recovery_provider_is_active(session_active: bool, durable_status: Option<&str>) -> bool {
    session_active || matches!(durable_status, Some("running" | "recovering"))
}

async fn recover_interrupted_board_execution(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    project_path: &str,
    task_ids: Vec<String>,
) -> Result<()> {
    BOARD_RESTART_RECOVERIES
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .map_err(|_| {
            ServerError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Board recovery ownership lock is unavailable.",
            )
        })?
        .insert(board_id.to_string());
    let mut owner = BoardRestartRecoveryGuard {
        board_id: board_id.to_string(),
        project_path: project_path.to_string(),
        settled: false,
    };
    let session_ids = {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(state, user_id, board_id)?;
        let sessions = dedupe_strings(
            stored
                .board
                .tasks
                .iter()
                .filter(|task| task_ids.contains(&task.id))
                .filter_map(|task| task.provider_session_id.clone())
                .chain(stored.board.current_provider_session_id.clone())
                .collect(),
        );
        stored.board.status = "running".to_string();
        stored.board.active = true;
        stored.board.loop_started = true;
        if let Some(task) = stored
            .board
            .tasks
            .iter()
            .find(|task| task_ids.contains(&task.id))
        {
            stored.board.current_task_id = Some(task.id.clone());
            stored.board.current_task_title = task.title.clone();
            stored.board.current_task_status = task.status.clone();
        }
        stored.board.current_phase = Some("recovering_interrupted_task".to_string());
        stored.board.phase_details =
            Some(json!({"taskIds": task_ids, "providerSessionIds": sessions}));
        stored.board.append_log("Recovering interrupted board execution; waiting for the existing provider before allowing retry");
        stored.board.touch();
        save_board(state, &stored.board)?;
        sessions
    };
    loop {
        let board = load_user_board(state, user_id, board_id)?.board;
        let mut active = false;
        for session_id in &session_ids {
            let session = state.storage.get_session_summary(session_id)?;
            let durable = state
                .storage
                .latest_durable_chat_run_for_session(session_id)?;
            if recovery_provider_is_active(
                session.as_ref().is_some_and(|value| value.active),
                durable.as_ref().map(|value| value.status.as_str()),
            ) {
                active = true;
                if board.status == "cancelled"
                    && let Some(session) = session
                {
                    let _ = state
                        .abort_agent_session(session.provider, session_id)
                        .await;
                }
            }
        }
        if !active {
            let _guard = board_mutation_lock();
            let mut stored = load_user_board(state, user_id, board_id)?;
            settle_interrupted_board_tasks(&mut stored.board, &task_ids);
            stored.board.touch();
            save_board(state, &stored.board)?;
            owner.settled = true;
            return Ok(());
        }
        sleep(PROVIDER_POLL_INTERVAL).await;
    }
}

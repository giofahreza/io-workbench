#[derive(Debug)]
struct StoredBoard {
    path: PathBuf,
    board: AgenticBoard,
}

fn load_user_board(state: &AppState, user_id: &str, id: &str) -> Result<StoredBoard> {
    load_boards(state)?
        .into_iter()
        .find(|stored| stored.board.id == id && stored.board.user_id.as_deref() == Some(user_id))
        .ok_or_else(|| not_found("Agentic board not found"))
}

fn latest_board_for_project(
    state: &AppState,
    user_id: &str,
    project_path: &str,
) -> Result<Option<StoredBoard>> {
    let mut boards = load_boards(state)?
        .into_iter()
        .filter(|stored| {
            stored.board.user_id.as_deref() == Some(user_id)
                && stored.board.project_path == project_path
        })
        .collect::<Vec<_>>();
    boards.sort_by(|left, right| right.board.updated_at.cmp(&left.board.updated_at));
    Ok(boards.into_iter().next())
}

fn load_boards(state: &AppState) -> Result<Vec<StoredBoard>> {
    let dir = boards_dir(state);
    load_boards_from_dir(&dir)
}

fn load_boards_from_dir(dir: &Path) -> Result<Vec<StoredBoard>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut boards = Vec::new();
    for entry in fs::read_dir(&dir).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let content = fs::read_to_string(&path).map_err(io_error)?;
        match serde_json::from_str::<AgenticBoard>(&content) {
            Ok(mut run) => {
                normalize_board_model(&mut run);
                boards.push(StoredBoard { path, board: run });
            }
            Err(error) => {
                tracing::warn!(file = %path.display(), %error, "failed to read agentic board snapshot");
            }
        }
    }
    Ok(boards)
}

fn save_board(state: &AppState, run: &AgenticBoard) -> Result<()> {
    let _guard = BOARD_SAVE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = boards_dir(state);
    fs::create_dir_all(&dir).map_err(io_error)?;
    let path = board_file_path(state, run);
    let temp_path = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    let mut run_to_save = run.clone();
    if let Ok(content) = fs::read_to_string(&path)
        && let Ok(current) = serde_json::from_str::<AgenticBoard>(&content)
    {
        preserve_newer_control_state(&mut run_to_save, &current);
    }
    normalize_board_model(&mut run_to_save);
    let content = serde_json::to_string_pretty(&run_to_save).map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to serialize board",
            error.to_string(),
        )
    })?;
    fs::write(&temp_path, content).map_err(io_error)?;
    fs::rename(&temp_path, &path).map_err(io_error)?;
    Ok(())
}

fn preserve_newer_control_state(run: &mut AgenticBoard, current: &AgenticBoard) {
    if current.control_revision <= run.control_revision {
        return;
    }
    run.control_revision = current.control_revision;
    run.status = current.status.clone();
    run.active = current.active;
    run.loop_started = current.loop_started;
    run.auto_run_enabled = current.auto_run_enabled;
    run.pause_requested = current.pause_requested;
    run.paused_at = current.paused_at;
    run.pause_reason = current.pause_reason.clone();
    run.cancellation_reason = current.cancellation_reason.clone();
    run.abort_source = current.abort_source.clone();
    run.abort_requested_at = current.abort_requested_at;
    run.canceled_at = current.canceled_at;
    run.scheduled_start_at = current.scheduled_start_at;
    if matches!(current.status.as_str(), "paused" | "pausing" | "cancelled") {
        run.current_task_id = current.current_task_id.clone();
        run.current_task_title = current.current_task_title.clone();
        run.current_task_status = current.current_task_status.clone();
        run.current_provider_session_id = current.current_provider_session_id.clone();
        run.provider_call_started_at = current.provider_call_started_at;
        run.provider_call_label = current.provider_call_label.clone();
        for current_task in &current.tasks {
            let Some(task) = run.tasks.iter_mut().find(|task| task.id == current_task.id) else {
                continue;
            };
            if task_status_is_active(&task.status) && !task_status_is_active(&current_task.status) {
                task.status = canonical_task_status(&current_task.status).to_string();
                task.started_at = current_task.started_at;
                task.completed_at = current_task.completed_at;
                for entry in &current_task.transcript {
                    if !task.transcript.contains(entry) {
                        task.transcript.push(entry.clone());
                    }
                }
                task.transcript_updated_at = current_task.transcript_updated_at;
            }
        }
    }
    for log in &current.logs {
        if !run.logs.contains(log) {
            run.logs.push(log.clone());
        }
    }
    if run.logs.len() > 500 {
        let remove_count = run.logs.len() - 500;
        run.logs.drain(0..remove_count);
    }
}

fn board_mutation_lock() -> MutexGuard<'static, ()> {
    BOARD_MUTATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn boards_dir(state: &AppState) -> PathBuf {
    state.config.config_dir.join(BOARD_STORAGE_DIR)
}

fn board_file_path(state: &AppState, run: &AgenticBoard) -> PathBuf {
    boards_dir(state).join(format!(
        "{}.json",
        board_storage_key(run.user_id.as_deref(), &run.project_path)
    ))
}

fn board_storage_key(user_id: Option<&str>, project_path: &str) -> String {
    sha256_hex(format!("{}\0{}", user_id.unwrap_or("anonymous"), project_path).as_bytes())
}

#[cfg(test)]
fn prompt_to_task_drafts(run: &mut AgenticBoard, prompt: &str) -> Vec<BoardTask> {
    let mut tasks = prompt
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(12)
        .map(|line| {
            let title = line
                .trim_start_matches(['-', '*', '•'])
                .trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.' || ch == ')')
                .trim()
                .to_string();
            BoardTask::draft(
                run,
                title_from_prompt(&title).unwrap_or_else(|| "New board task".to_string()),
                line.to_string(),
            )
        })
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        tasks.push(BoardTask::draft(
            run,
            title_from_prompt(prompt).unwrap_or_else(|| "New board task".to_string()),
            prompt.to_string(),
        ));
    }
    tasks
}

#[derive(Debug)]
struct PromptTaskDraftAttempt {
    result: Result<(Vec<Value>, Option<String>)>,
    provider_prompt: String,
    provider_output: String,
    session_id: Option<String>,
    token_usage: Option<Value>,
    effective_provider: String,
    effective_model: String,
    started_at: DateTime<Utc>,
}

async fn generate_prompt_task_drafts(
    state: &AppState,
    run: &AgenticBoard,
    prompt: &str,
    provider: Option<&str>,
    model: Option<&str>,
    board_profile: Option<&str>,
) -> PromptTaskDraftAttempt {
    let profile = board_profile
        .map(|value| normalize_board_profile(Some(value)))
        .unwrap_or_else(|| normalize_board_profile(Some(&run.board_profile)));
    let selected_provider = provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| normalize_provider(Some(value)))
        .transpose();
    let selected_provider = match selected_provider {
        Ok(Some(provider)) => provider,
        Ok(None) => DEFAULT_BREAKDOWN_PROVIDER.to_string(),
        Err(error) => {
            return PromptTaskDraftAttempt {
                result: Err(error),
                provider_prompt: String::new(),
                provider_output: String::new(),
                session_id: None,
                token_usage: None,
                effective_provider: DEFAULT_BREAKDOWN_PROVIDER.to_string(),
                effective_model: model
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or(DEFAULT_BREAKDOWN_MODEL)
                    .to_string(),
                started_at: Utc::now(),
            };
        }
    };
    let selected_model = model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            run.task_model_overrides
                .get("breakdown")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            let board_model = trim_string(Some(run.primary_model.clone()))
                .or_else(|| trim_string(Some(run.model.clone())))?;
            let board_default = default_model_for_provider(&run.provider);
            (board_model != board_default).then_some(board_model)
        })
        .unwrap_or_else(|| {
            if selected_provider == DEFAULT_BREAKDOWN_PROVIDER {
                DEFAULT_BREAKDOWN_MODEL.to_string()
            } else {
                default_model_for_provider(&selected_provider)
            }
        });
    let mut generation_run = run.clone();
    generation_run.provider = selected_provider.clone();
    generation_run.board_profile = profile.clone();
    generation_run.actual_session_id = None;
    generation_run.current_provider_session_id = None;
    generation_run.session_id = None;
    generation_run.current_task_id = run.current_task_id.clone().or_else(|| {
        run.backlog_breakdown
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    if !selected_model.trim().is_empty() {
        generation_run.model = selected_model.clone();
        generation_run.primary_model = selected_model.clone();
    }
    let provider_prompt = build_prompt_task_draft_prompt(&generation_run, prompt, &profile);
    let started_at = Utc::now();
    let provider_result = execute_shared_provider_turn(
        state,
        &generation_run,
        &selected_provider,
        &selected_model,
        &provider_prompt,
        None,
        generation_run.current_task_id.as_deref(),
    )
    .await;
    match provider_result {
        Ok(result) if result.exit_code == 0 => {
            let parsed = parse_json_object(&result.assistant_text).unwrap_or_else(|| json!({}));
            let tasks = sanitize_prompt_task_drafts(&parsed, prompt);
            let generation_result = if tasks.is_empty() {
                Err(task_generation_error(
                    "AI returned valid output but no usable task drafts.",
                ))
            } else {
                Ok((tasks, None))
            };
            PromptTaskDraftAttempt {
                result: generation_result,
                provider_prompt,
                provider_output: result.assistant_text,
                session_id: Some(result.session_id),
                token_usage: result.token_usage,
                effective_provider: selected_provider,
                effective_model: selected_model,
                started_at,
            }
        }
        Ok(result) => PromptTaskDraftAttempt {
            result: Err(task_generation_error(format!(
                "AI task generation failed: {}",
                result.summary
            ))),
            provider_prompt,
            provider_output: result.assistant_text,
            session_id: Some(result.session_id),
            token_usage: result.token_usage,
            effective_provider: selected_provider,
            effective_model: selected_model,
            started_at,
        },
        Err(error) => PromptTaskDraftAttempt {
            result: Err(task_generation_error(format!(
                "AI task generation failed: {}",
                server_error_message(&error)
            ))),
            provider_prompt,
            provider_output: String::new(),
            session_id: None,
            token_usage: None,
            effective_provider: selected_provider,
            effective_model: selected_model,
            started_at,
        },
    }
}

fn record_prompt_task_generation_attempt(
    run: &mut AgenticBoard,
    label: &str,
    attempt: &PromptTaskDraftAttempt,
) {
    let controls = board_provider_controls(run);
    let mut telemetry = json!({
        "phase": "backlog_generation",
        "label": label,
        "provider": attempt.effective_provider,
        "chars": attempt.provider_prompt.chars().count(),
        "estimatedTokens": estimate_tokens(&attempt.provider_prompt),
        "startedAt": attempt.started_at,
        "reasoningEffort": controls.effort,
        "fast": controls.fast,
    });
    if let Some(error) = attempt.result.as_ref().err() {
        telemetry["error"] = json!(server_error_message(error));
        telemetry["outcome"] = json!("failed");
    } else {
        telemetry["outcome"] = json!("completed");
    }
    run.prompt_telemetry.push(telemetry);
    let telemetry_index = run.prompt_telemetry.len().saturating_sub(1);
    finalize_prompt_telemetry(
        run,
        telemetry_index,
        attempt.session_id.as_deref(),
        Some(&attempt.effective_model),
        attempt.token_usage.as_ref(),
    );
    if attempt.session_id.is_some() || !attempt.provider_output.is_empty() {
        increment_provider_usage(
            run,
            &attempt.provider_prompt,
            &attempt.provider_output,
            attempt.session_id.as_deref(),
            attempt.token_usage.as_ref(),
        );
    }
}

fn prompt_task_generation_running_transcript(
    prompt: &str,
    provider: &str,
    model: &str,
    started_at: DateTime<Utc>,
) -> Value {
    json!([
        {
            "timestamp": started_at,
            "kind": "message",
            "role": "user",
            "content": prompt,
        },
        {
            "timestamp": started_at,
            "kind": "status",
            "role": "assistant",
            "provider": provider,
            "model": model,
            "status": "running",
            "content": "Generating backlog tasks from the prompt.",
        }
    ])
}

fn prompt_task_generation_transcript(
    attempt: &PromptTaskDraftAttempt,
    prompt: &str,
    completed: bool,
    fallback_error: Option<&str>,
) -> Value {
    let completed_at = Utc::now();
    let mut assistant_content = attempt.provider_output.trim().to_string();
    if assistant_content.is_empty() {
        assistant_content = fallback_error
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("No provider output captured.")
            .to_string();
    }
    let mut assistant = json!({
        "timestamp": completed_at,
        "kind": if completed { "assistant" } else { "error" },
        "role": "assistant",
        "provider": attempt.effective_provider,
        "model": attempt.effective_model,
        "status": if completed { "completed" } else { "failed" },
        "content": assistant_content,
    });
    if let Some(object) = assistant.as_object_mut() {
        if let Some(session_id) = attempt.session_id.as_deref() {
            object.insert("sessionId".to_string(), json!(session_id));
        }
        if let Some(token_usage) = attempt.token_usage.as_ref() {
            object.insert("tokenUsage".to_string(), token_usage.clone());
        }
        if let Some(error) = fallback_error
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            object.insert("error".to_string(), json!(error));
        }
    }
    json!([
        {
            "timestamp": attempt.started_at,
            "kind": "message",
            "role": "user",
            "content": prompt,
        },
        assistant
    ])
}

fn build_prompt_task_draft_prompt(run: &AgenticBoard, prompt: &str, profile: &str) -> String {
    format!(
        r#"Create implementation-ready Kanban backlog cards for this focused follow-up prompt.

Prompt:
{prompt}

{board_profile_block}

{git_policy_block}

Current ticket scope:
{scope_context}

Existing board items:
{tasks}

Return JSON only. No markdown fence.
Schema:
{{
  "tasks": [
    {{
      "planKey": "local-card-key",
      "level": "initiative|epic|story",
      "title": "clear planning title",
      "kind": "research|design|review|implementation",
      "details": "scope and user-facing or strategic description",
      "acceptanceCriteria": ["verifiable outcome"],
      "references": ["relevant file or source"],
      "priority": "p0|p1|p2|p3",
      "dependsOn": ["plan:another-local-card", "board:task-42"]
    }}
  ]
}}

Rules:
- `planKey` is a unique local key for this response, not a persisted task ID. Use lowercase letters, digits, `_`, or `-` only.
- Every dependency must be in `dependsOn`, using either `plan:<planKey>` for another card in this response or `board:<existing-task-id>` for an item listed above. Use an empty array when there is no dependency.
- Never invent, predict, or emit persisted `task-*` IDs as `planKey` values or unprefixed dependencies.
- Generate only initiative, epic, or story cards directly needed for the prompt-matched feature area.
- The visible Backlog must never contain a top-level task or subtask.
- If the request is tiny, create one small story; do not force an initiative or epic.
- If you think in terms of a task or subtask, wrap it in the smallest useful story.
- Preserve explicit user scope; do not add unrelated cleanup or product ideas.
- Keep every planning card independently understandable and user-approved before execution.
- Do not create executable subtasks during board-level breakdown.
- Nice-to-have ideas must be separate Backlog cards and must not silently become required child work.
- Prefer a small number of complete cards over many vague cards."#,
        prompt = prompt,
        board_profile_block = format_board_profile_block(profile),
        git_policy_block = git_policy_block(run),
        scope_context = "Use ticket descriptions and acceptance criteria as the only scope source. Do not invent a second scope layer.".to_string(),
        tasks = run
            .tasks
            .iter()
            .filter(|task| !task.backlog_generation_task)
            .rev()
            .take(20)
            .map(|task| format!("{} [{}] {}", task.id, task.status, task.title))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn board_profile_block(run: &AgenticBoard) -> String {
    format_board_profile_block(&run.board_profile)
}

fn format_board_profile_block(profile: &str) -> String {
    match normalize_board_profile(Some(profile)).as_str() {
        "minimal" => [
            "Board profile: Minimal",
            "Implement the explicit ticket scope with minimal expansion, low context, and concrete verification.",
            "Scope guidance:",
            "- Preserve every explicit behavior and constraint from the prompt or source documents.",
            "- For broad app-generation prompts, infer only the core product data, CRUD/workflows, persistence, navigation, validation, and runnable local verification needed for the requested app to work.",
            "- Do not add optional dashboards, analytics, roles, integrations, or visual polish unless the prompt or source docs require them.",
            "Planning scope:",
            "- Create the smallest task set that fully satisfies the explicit ticket scope and required local glue.",
            "- Avoid optional enhancement tasks unless they are necessary for correctness or verification.",
            "Execution scope:",
            "- Prefer focused code changes and local verification over broad rewrites.",
            "- Do not expand scope beyond the attached ticket except for necessary wiring, error handling, and tests/checks.",
            "QA scope:",
            "- Verify explicit ticket behavior, necessary inferred glue, and functional happy/error paths.",
            "- Do not fail completion for optional polish or non-required enhancements.",
        ]
        .join("\n"),
        "product_ready" => [
            "Board profile: Product-Ready",
            "Deliver complete workflows with richer product detail, structural UX checks, and stricter edge-case validation.",
            "Scope guidance:",
            "- Preserve every explicit behavior and constraint from the prompt or source documents.",
            "- For broad app-generation prompts, infer a product-ready local app: core data model, CRUD/workflows, persistence, navigation, validation, useful dashboard/summaries, search/filter where useful, empty/loading/error states, responsive structure, and runnable verification.",
            "- Add richer workflow/detail scope only when it directly supports the requested product; do not invent unrelated integrations, payments, enterprise roles, or subjective redesign.",
            "Planning scope:",
            "- Plan the ticket hierarchy into complete user-facing workflows, not only isolated CRUD endpoints.",
            "- Include validation, useful summaries/comparisons, responsive structural checks, and concrete local QA for important flows.",
            "Execution scope:",
            "- Implement complete functional screens, forms, persistence paths, validation feedback, and state handling for the attached product workflow.",
            "- Keep subjective visual polish as backlog unless the task explicitly asks for it, but do not leave structurally broken or unusable UI.",
            "QA scope:",
            "- Verify core workflows plus validation failures, empty states, persisted state, and important responsive structure.",
            "- Fail QA for missing required workflow detail or unusable UI structure, not for subjective aesthetic preferences.",
        ]
        .join("\n"),
        _ => [
            "Board profile: Complete App",
            "Build a complete functional app with common app completeness and bounded token use.",
            "Scope guidance:",
            "- Preserve every explicit behavior and constraint from the prompt or source documents.",
            "- For broad app-generation prompts, infer the common complete-app pieces: core product data model, CRUD/workflows, persistence, navigation, validation, practical empty/error states, basic summaries, responsive layout, and runnable verification.",
            "- Do not invent unrelated integrations, payment flows, enterprise roles, or subjective UI polish unless the ticket requires them.",
            "Planning scope:",
            "- Plan enough tasks to make the requested app complete and locally verifiable without adding optional product expansion.",
            "- Include validation, persistence, and functional UI sanity checks where relevant.",
            "Execution scope:",
            "- Implement complete end-to-end behavior for the attached workflow, including useful validation and state handling.",
            "- Avoid broad redesigns or optional features that are not needed for a correct complete app.",
            "QA scope:",
            "- Verify main workflows, persistence, validation, and functional responsive usability.",
            "- Do not block on subjective polish unless it prevents required workflow use.",
        ]
        .join("\n"),
    }
}

fn git_policy_block(run: &AgenticBoard) -> String {
    match normalize_git_policy(Some(&run.git_policy)).as_str() {
        "managed" => [
            "Git policy: Managed Git Workflow",
            "The orchestrator handles git writes after each verified task group.",
            "- Read-only git inspection commands are allowed.",
            "- Do not run git write commands yourself. Do not create branches, add, commit, merge, rebase, reset, clean, stash, tag, or push from the provider.",
            "- The orchestrator creates a task branch before implementation, then commits, merges to main, and pushes only after the task group is complete and verified.",
            "- If git state prevents the managed workflow, report the blocker clearly instead of trying a manual workaround.",
        ]
        .join("\n"),
        _ => [
            "Git policy: Read-only Git",
            "Provider tasks may inspect git state but must not change git state.",
            "- Read-only git inspection commands are allowed, such as git status, git diff, git log, git show, git branch --show-current, and git remote -v.",
            "- Do not run git write commands: no add, commit, checkout, switch, branch creation/deletion, merge, rebase, reset, restore, clean, stash, tag, or push.",
            "- Do not plan git history tasks. Finish with code/test evidence only.",
        ]
        .join("\n"),
    }
}

fn sanitize_prompt_task_drafts(parsed: &Value, prompt: &str) -> Vec<Value> {
    parsed
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .take(12)
        .enumerate()
        .filter_map(|(index, task)| {
            let title = task
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let details = task
                .get("details")
                .or_else(|| task.get("description"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(title);
            let references = dedupe_strings(
                normalize_string_list(task.get("references"))
                    .into_iter()
                    .chain(normalize_string_list(task.get("files")))
                    .chain(normalize_string_list(task.get("paths")))
                    .collect(),
            );
            let acceptance_criteria = normalize_string_list(
                task.get("acceptanceCriteria")
                    .or_else(|| task.get("acceptance"))
                    .or_else(|| task.get("criteria")),
            );
            let kind = prompt_task_kind_from_value(&task, title, details, &acceptance_criteria);
            let generated_level =
                normalize_task_level(task.get("level").and_then(Value::as_str), TASK_LEVEL_STORY);
            let wrapped_level = if matches!(generated_level, TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK) {
                TASK_LEVEL_STORY
            } else {
                generated_level
            };
            let wrapped_details = if wrapped_level == TASK_LEVEL_STORY
                && matches!(generated_level, TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK)
            {
                format!(
                    "{}

This story wraps the generated {} and keeps executable engineering work nested below the story.",
                    details, generated_level
                )
            } else {
                details.to_string()
            };
            let plan_key = task
                .get("planKey")
                .or_else(|| task.get("plan_key"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("plan-{}", index + 1));
            let plan_dependencies = normalize_string_list(
                task.get("dependsOn")
                    .or_else(|| task.get("dependencies"))
                    .or_else(|| task.get("blockedBy"))
                    .or_else(|| task.get("blocked_by")),
            );
            Some(json!({
                "planKey": plan_key,
                "planDependencies": plan_dependencies,
                "title": title,
                "kind": kind,
                "taskType": kind,
                "level": wrapped_level,
                "sourceLevel": generated_level,
                "sourceKind": kind,
                "executable": false,
                "required": task.get("required").and_then(Value::as_bool).unwrap_or(true),
                "scopeVersion": 1,
                "details": wrapped_details,
                "prompt": prompt,
                "acceptanceCriteria": acceptance_criteria,
                "references": references,
                "priority": normalize_priority(task.get("priority").and_then(Value::as_str)),
                "blockedBy": [],
                "status": "backlog",
            }))
        })
        .collect()
}

fn prompt_task_tree_from_draft(
    run: &mut AgenticBoard,
    draft: Value,
    prompt: &str,
) -> Vec<BoardTask> {
    let source_level = normalize_task_level(
        draft
            .get("sourceLevel")
            .or_else(|| draft.get("level"))
            .and_then(Value::as_str),
        TASK_LEVEL_STORY,
    );
    let mut story_draft = draft.clone();
    if let Some(object) = story_draft.as_object_mut() {
        let planning_level = if matches!(source_level, TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC) {
            source_level
        } else {
            TASK_LEVEL_STORY
        };
        let planning_kind = object
            .get("sourceKind")
            .and_then(Value::as_str)
            .unwrap_or(TASK_KIND_DESIGN)
            .to_string();
        object.insert("level".to_string(), json!(planning_level));
        object.insert("kind".to_string(), json!(planning_kind.clone()));
        object.insert("taskType".to_string(), json!(planning_kind));
    }
    let mut story = prompt_task_from_draft(run, story_draft, prompt);
    story.hierarchy.level = if matches!(source_level, TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC) {
        source_level.to_string()
    } else {
        TASK_LEVEL_STORY.to_string()
    };
    story.hierarchy.parent_id = None;
    story.hierarchy.executable = false;
    if matches!(source_level, TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC) {
        return vec![story];
    }
    story.task_type = TASK_KIND_DESIGN.to_string();
    if source_level == TASK_LEVEL_STORY {
        return vec![story];
    }

    let story_id = story.id.clone();
    let mut task_draft = draft.clone();
    if let Some(object) = task_draft.as_object_mut() {
        object.insert("level".to_string(), json!(TASK_LEVEL_TASK));
        object.insert("kind".to_string(), json!(TASK_KIND_DESIGN));
        object.insert("taskType".to_string(), json!(TASK_KIND_DESIGN));
    }
    let mut system_task = task_from_json(run, task_draft, run.tasks.len(), TASK_STATUS_BACKLOG)
        .unwrap_or_else(|| {
            let mut fallback = BoardTask::draft(run, story.title.clone(), story.details.clone());
            fallback.task_type = TASK_KIND_DESIGN.to_string();
            fallback
        });
    system_task.id = allocate_task_id(run);
    system_task.hierarchy.level = TASK_LEVEL_TASK.to_string();
    system_task.hierarchy.parent_id = Some(story_id.clone());
    system_task.hierarchy.executable = false;
    system_task.task_type = TASK_KIND_DESIGN.to_string();
    system_task.task_origin = "user_prompt_generated".to_string();
    system_task.group_id = Some(story_id.clone());
    system_task.status = TASK_STATUS_BACKLOG.to_string();
    system_task.prompt = system_task.description.clone();
    if source_level == TASK_LEVEL_TASK {
        return vec![story, system_task];
    }

    let mut subtask_draft = draft;
    if let Some(object) = subtask_draft.as_object_mut() {
        object.insert("level".to_string(), json!(TASK_LEVEL_SUBTASK));
        object.insert(
            "kind".to_string(),
            json!(
                object
                    .get("sourceKind")
                    .and_then(Value::as_str)
                    .unwrap_or(TASK_KIND_IMPLEMENTATION)
            ),
        );
        object.insert(
            "taskType".to_string(),
            json!(
                object
                    .get("sourceKind")
                    .and_then(Value::as_str)
                    .unwrap_or(TASK_KIND_IMPLEMENTATION)
            ),
        );
    }
    let mut subtask = task_from_json(run, subtask_draft, run.tasks.len() + 1, TASK_STATUS_BACKLOG)
        .unwrap_or_else(|| {
            let mut fallback = BoardTask::draft(run, story.title.clone(), story.details.clone());
            fallback.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
            fallback.hierarchy.executable = true;
            fallback
        });
    subtask.id = allocate_task_id(run);
    subtask.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
    subtask.hierarchy.parent_id = Some(system_task.id.clone());
    subtask.hierarchy.executable = true;
    subtask.task_origin = "user_prompt_generated".to_string();
    subtask.group_id = Some(story_id);
    subtask.status = TASK_STATUS_BACKLOG.to_string();
    subtask.prompt = subtask.description.clone();
    vec![story, system_task, subtask]
}

#[derive(Debug)]
struct GeneratedPromptTaskTree {
    plan_key: String,
    dependency_refs: Vec<String>,
    tasks: Vec<BoardTask>,
}

fn prepare_generated_prompt_task_trees(
    run: &mut AgenticBoard,
    drafts: Vec<Value>,
    prompt: &str,
) -> Result<Vec<Vec<BoardTask>>> {
    let existing_ids = run
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<BTreeSet<_>>();
    let mut generated = Vec::new();
    let mut plan_keys = BTreeSet::new();
    for draft in drafts {
        let plan_key = prompt_plan_key(&draft)?;
        if !plan_keys.insert(plan_key.clone()) {
            return Err(invalid_generated_plan(format!(
                "Duplicate planKey: {plan_key}."
            )));
        }
        let dependency_refs = normalize_string_list(draft.get("planDependencies"));
        let tasks = prompt_task_tree_from_draft(run, draft, prompt);
        generated.push(GeneratedPromptTaskTree {
            plan_key,
            dependency_refs,
            tasks,
        });
    }

    let plan_ids = generated
        .iter()
        .filter_map(|tree| {
            tree.tasks
                .first()
                .map(|task| (tree.plan_key.clone(), task.id.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    for tree in &mut generated {
        let Some(root) = tree.tasks.first_mut() else {
            return Err(invalid_generated_plan(format!(
                "Plan item {} did not produce a task.",
                tree.plan_key
            )));
        };
        let mut dependencies = Vec::new();
        for reference in &tree.dependency_refs {
            let dependency = resolve_prompt_plan_dependency(
                reference,
                &tree.plan_key,
                &plan_ids,
                &existing_ids,
            )?;
            if dependency == root.id {
                return Err(invalid_generated_plan(format!(
                    "Plan item {} cannot depend on itself.",
                    tree.plan_key
                )));
            }
            dependencies.push(dependency);
        }
        root.depends_on = dedupe_strings(dependencies);
        root.hierarchy.blocked_by = root.depends_on.clone();
    }

    Ok(generated.into_iter().map(|tree| tree.tasks).collect())
}

fn prompt_plan_key(draft: &Value) -> Result<String> {
    let key = draft
        .get("planKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_generated_plan("Every generated task requires planKey."))?;
    if key.len() > 64
        || key.starts_with("task-")
        || !key.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
    {
        return Err(invalid_generated_plan(format!("Invalid planKey: {key}.")));
    }
    Ok(key.to_string())
}

fn resolve_prompt_plan_dependency(
    reference: &str,
    source_key: &str,
    plan_ids: &BTreeMap<String, String>,
    existing_ids: &BTreeSet<String>,
) -> Result<String> {
    let reference = reference.trim();
    let Some((kind, value)) = reference.split_once(':') else {
        return Err(invalid_generated_plan(format!(
            "Dependency {reference} for {source_key} must use plan: or board:."
        )));
    };
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid_generated_plan(format!(
            "Dependency {reference} for {source_key} is empty."
        )));
    }
    match kind {
        "plan" => plan_ids.get(value).cloned().ok_or_else(|| {
            invalid_generated_plan(format!(
                "Dependency {reference} for {source_key} does not match a generated planKey."
            ))
        }),
        "board" if existing_ids.contains(value) => Ok(value.to_string()),
        "board" => Err(invalid_generated_plan(format!(
            "Dependency {reference} for {source_key} does not match an existing board item."
        ))),
        _ => Err(invalid_generated_plan(format!(
            "Dependency {reference} for {source_key} must use plan: or board:."
        ))),
    }
}

fn invalid_generated_plan(details: impl Into<String>) -> ServerError {
    task_generation_error(format!("Invalid generated task plan: {}", details.into()))
}

fn keep_missing_prompt_task_trees(
    run: &AgenticBoard,
    trees: Vec<Vec<BoardTask>>,
) -> (Vec<BoardTask>, usize) {
    let mut existing_root_keys = run
        .tasks
        .iter()
        .filter(|task| {
            !task.backlog_generation_task
                && task.hierarchy.parent_id.is_none()
                && matches!(
                    task_level(task),
                    TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC | TASK_LEVEL_STORY
                )
        })
        .map(|task| normalize_suggested_task_key(&task.title))
        .filter(|key| !key.is_empty())
        .collect::<BTreeSet<_>>();
    let mut kept = Vec::new();
    let mut reused = 0usize;
    for tree in trees {
        let Some(root) = tree.first() else {
            continue;
        };
        let key = normalize_suggested_task_key(&root.title);
        if key.is_empty() || !existing_root_keys.insert(key) {
            reused = reused.saturating_add(1);
            continue;
        }
        kept.extend(tree);
    }
    (kept, reused)
}

fn spawn_backlog_prompt_generation(
    state: AppState,
    user_id: String,
    board_id: String,
    operation_id: String,
    prompt: String,
    provider: String,
    model: String,
    board_profile: String,
) {
    tokio::spawn(async move {
        if let Err(error) = complete_backlog_prompt_generation(
            &state,
            &user_id,
            &board_id,
            &operation_id,
            &prompt,
            &provider,
            &model,
            &board_profile,
        )
        .await
        {
            tracing::warn!(
                board_id = %board_id,
                operation_id = %operation_id,
                error = %server_error_message(&error),
                "backlog prompt generation failed"
            );
        }
    });
}

async fn complete_backlog_prompt_generation(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    operation_id: &str,
    prompt: &str,
    provider: &str,
    model: &str,
    board_profile: &str,
) -> Result<()> {
    let snapshot = load_user_board(state, user_id, board_id)?;
    let attempt = generate_prompt_task_drafts(
        state,
        &snapshot.board,
        prompt,
        (!provider.trim().is_empty()).then_some(provider),
        (!model.trim().is_empty() && model.trim() != "provider default").then_some(model),
        (!board_profile.trim().is_empty()).then_some(board_profile),
    )
    .await;
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, board_id)?;
    let current_operation_id = stored
        .board
        .backlog_breakdown
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let current_status = stored
        .board
        .backlog_breakdown
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if current_operation_id != operation_id || current_status != "running" {
        return Ok(());
    }
    record_prompt_task_generation_attempt(
        &mut stored.board,
        "Kanban backlog prompt generation",
        &attempt,
    );
    let generation_error = attempt.result.as_ref().err().map(server_error_message);
    let generation_transcript = prompt_task_generation_transcript(
        &attempt,
        prompt,
        attempt.result.is_ok(),
        generation_error.as_deref(),
    );
    match attempt.result {
        Ok((drafts, warning)) => {
            let mut candidate = stored.board.clone();
            let generated_result = (|| -> Result<(usize, usize)> {
                let generated_trees =
                    prepare_generated_prompt_task_trees(&mut candidate, drafts, prompt)?;
                let (mut generated, reused_tree_count) =
                    keep_missing_prompt_task_trees(&candidate, generated_trees);
                if let Some(warning) = warning.as_deref().filter(|value| !value.trim().is_empty()) {
                    if let Some(first) = generated.first_mut() {
                        first
                            .references
                            .push(format!("Task generation note: {warning}"));
                    }
                }
                let count = generated.len();
                candidate.tasks.extend(generated);
                normalize_board_hierarchy(&mut candidate);
                normalize_board_task_groups(&mut candidate);
                if let Some(issue) = dependency_validation_issues(&candidate).into_iter().next() {
                    return Err(invalid_generated_plan(issue));
                }
                if let Some(cycle) = dependency_cycle(&candidate) {
                    return Err(invalid_generated_plan(format!(
                        "Dependency cycle detected: {}",
                        cycle.join(" -> ")
                    )));
                }
                if let Some(issue) = hierarchy_validation_issues(&candidate).into_iter().next() {
                    return Err(invalid_generated_plan(issue));
                }
                refresh_hierarchy_rollups(&mut candidate);
                Ok((count, reused_tree_count))
            })();
            match generated_result {
                Ok((count, reused_tree_count)) => {
                    stored.board = candidate;
                    stored.board.backlog_breakdown = json!({
                        "status": "idle",
                        "lastOperationId": operation_id,
                        "prompt": prompt,
                        "provider": attempt.effective_provider,
                        "model": attempt.effective_model,
                        "boardProfile": board_profile,
                        "generatedTaskCount": count,
                        "reusedTaskTreeCount": reused_tree_count,
                        "warning": warning,
                        "completedAt": Utc::now(),
                        "updatedAt": Utc::now(),
                        "transcript": generation_transcript,
                    });
                    stored.board.append_log(format!(
                        "Backlog prompt generated {count} task(s) from {operation_id}"
                    ));
                }
                Err(error) => {
                    let message = server_error_message(&error);
                    stored.board.backlog_breakdown = json!({
                        "id": operation_id,
                        "status": TASK_STATUS_FAILED,
                        "prompt": prompt,
                        "provider": attempt.effective_provider,
                        "model": attempt.effective_model,
                        "boardProfile": board_profile,
                        "error": message.clone(),
                        "failedAt": Utc::now(),
                        "updatedAt": Utc::now(),
                        "transcript": generation_transcript,
                    });
                    stored.board.append_log(format!(
                        "Backlog prompt generation rejected for {operation_id}: {message}"
                    ));
                }
            }
        }
        Err(error) => {
            let message = generation_error.unwrap_or_else(|| server_error_message(&error));
            stored.board.backlog_breakdown = json!({
                "id": operation_id,
                "status": TASK_STATUS_FAILED,
                "prompt": prompt,
                "provider": attempt.effective_provider,
                "model": attempt.effective_model,
                "boardProfile": board_profile,
                "error": message.clone(),
                "failedAt": Utc::now(),
                "updatedAt": Utc::now(),
                "transcript": generation_transcript,
            });
            stored.board.append_log(format!(
                "Backlog prompt generation failed for {operation_id}: {message}"
            ));
        }
    }
    stored.board.touch();
    save_board(state, &stored.board)
}

fn prompt_task_from_draft(run: &mut AgenticBoard, draft: Value, prompt: &str) -> BoardTask {
    let title = draft
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("New board task")
        .to_string();
    let details = draft
        .get("details")
        .or_else(|| draft.get("description"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&title)
        .to_string();
    let generated_level =
        normalize_task_level(draft.get("level").and_then(Value::as_str), TASK_LEVEL_STORY);
    let level = if matches!(generated_level, TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK) {
        TASK_LEVEL_STORY
    } else {
        generated_level
    };
    let mut task = BoardTask::draft(run, title, details.clone());
    task.prompt = prompt.to_string();
    task.acceptance_criteria = normalize_string_list(draft.get("acceptanceCriteria"));
    if task.acceptance_criteria.is_empty() {
        task.acceptance_criteria = vec!["Complete the task described by this card.".to_string()];
    }
    task.task_type = prompt_task_kind_from_value(
        &draft,
        &task.title,
        &task.details,
        &task.acceptance_criteria,
    )
    .to_string();
    task.references = normalize_string_list(draft.get("references"));
    task.priority = normalize_priority(draft.get("priority").and_then(Value::as_str)).to_string();
    task.depends_on =
        normalize_string_list(draft.get("blockedBy").or_else(|| draft.get("dependsOn")));
    task.status = "backlog".to_string();
    task.manual_task = false;
    task.prompt_task = true;
    task.task_origin = "user_prompt_generated".to_string();
    task.backlog_generation_task = false;
    task.hierarchy = BoardTaskHierarchy {
        level: level.to_string(),
        parent_id: None,
        blocked_by: task.depends_on.clone(),
        executable: false,
        required: draft
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        scope_version: draft
            .get("scopeVersion")
            .and_then(Value::as_u64)
            .unwrap_or(1),
        rank: draft.get("rank").and_then(Value::as_i64).unwrap_or(0),
        attempts: Vec::new(),
        planned_files: normalize_string_list(
            draft.get("plannedFiles").or_else(|| draft.get("files")),
        ),
        side_effects: normalize_string_list(draft.get("sideEffects")),
        side_effects_approved: false,
        side_effect_approval: None,
        side_effect_evidence: Vec::new(),
        manual_test_environment: None,
        research_accepted: false,
        research_acceptance: None,
        discussion: Vec::new(),
    };
    task
}

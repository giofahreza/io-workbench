impl ProviderTaskResult {
    fn from_error(error: ServerError) -> Self {
        let message = server_error_message(&error);
        Self {
            stderr: message.clone(),
            assistant_text: message.clone(),
            stream_events: vec![json!({
                "timestamp": Utc::now(),
                "kind": "error",
                "isError": true,
                "content": message,
            })],
            errors: vec![message.clone()],
            session_id: None,
            token_usage: None,
            exit_code: 1,
            summary: message,
        }
    }
}

#[derive(Debug)]
struct ProviderPromptResult {
    output: String,
    session_id: Option<String>,
    token_usage: Option<Value>,
    effective_model: Option<String>,
}

async fn execute_provider_task(
    state: &AppState,
    run: &AgenticBoard,
    task_index: usize,
) -> Result<ProviderTaskResult> {
    execute_provider_task_with_retry_instruction(state, run, task_index, None).await
}

async fn execute_provider_task_with_retry_instruction(
    state: &AppState,
    run: &AgenticBoard,
    task_index: usize,
    retry_instruction: Option<&str>,
) -> Result<ProviderTaskResult> {
    let task = run
        .tasks
        .get(task_index)
        .ok_or_else(|| not_found("Danger task not found"))?;
    let mut prompt = build_task_execution_prompt(run, task, task_index);
    if let Some(instruction) =
        retry_instruction.filter(|instruction| !instruction.trim().is_empty())
    {
        prompt.push_str("\n\nProvider retry repair:\n");
        prompt.push_str(instruction.trim());
        prompt.push_str("\nReturn the required final JSON only after the task is complete.");
    }
    let provider = normalize_provider(Some(&run.provider))?;
    let model = effective_model_for_task(run, task);
    let execution_model = agentic_execution_model_for_provider(&provider, &model);
    let reusable_session = reusable_session_id(run);
    let session_id = task
        .provider_session_id
        .as_deref()
        .or(reusable_session.as_deref());
    let result = execute_shared_provider_turn(
        state,
        run,
        &provider,
        &execution_model,
        &prompt,
        session_id,
        Some(&task.id),
    )
    .await?;
    let stream_events = shared_provider_stream_events(&provider, &result);
    let errors = if result.exit_code == 0 {
        Vec::new()
    } else {
        vec![result.summary.clone()]
    };
    Ok(ProviderTaskResult {
        summary: result.summary,
        stderr: result.stderr,
        assistant_text: result.assistant_text,
        stream_events,
        errors,
        session_id: Some(result.session_id),
        token_usage: result.token_usage,
        exit_code: result.exit_code,
    })
}

async fn execute_provider_task_with_fallback(
    state: &AppState,
    run: &AgenticBoard,
    task_index: usize,
) -> ProviderExecutionAttempt {
    let mut primary_result = execute_provider_task(state, run, task_index).await;
    let repair_retries = malformed_tool_call_repair_retries(run);
    for attempt in 0..repair_retries {
        if !provider_result_should_attempt_malformed_tool_call_repair(&primary_result) {
            break;
        }
        let reason = provider_result_failure_summary(&primary_result);
        let mut retry_run = run.clone();
        if let Some(session_id) = provider_result_session_id(&primary_result)
            && let Some(task) = retry_run.tasks.get_mut(task_index)
        {
            task.provider_session_id = Some(session_id);
        }
        let instruction = malformed_tool_call_repair_instruction(&reason);
        let mut retry_result = execute_provider_task_with_retry_instruction(
            state,
            &retry_run,
            task_index,
            Some(&instruction),
        )
        .await;
        if let Ok(result) = &mut retry_result {
            result.stream_events.insert(
                0,
                json!({
                    "timestamp": Utc::now(),
                    "kind": "status",
                    "status": "malformed_tool_call_repair",
                    "content": format!(
                        "Retried provider task after malformed integer tool-call arguments ({}/{})",
                        attempt + 1,
                        repair_retries
                    ),
                    "previousFailure": limit_text(&reason, 1200),
                }),
            );
        }
        primary_result = retry_result;
    }
    if !provider_result_requires_fallback(&primary_result) {
        return ProviderExecutionAttempt {
            result: primary_result,
            fallback: None,
        };
    }
    let Some((provider, model)) = configured_provider_fallback(run) else {
        return ProviderExecutionAttempt {
            result: primary_result,
            fallback: None,
        };
    };
    let reason = provider_result_failure_summary(&primary_result);
    let mut fallback_run = run.clone();
    fallback_run.provider = provider.clone();
    fallback_run.model = model.clone();
    fallback_run.actual_session_id = None;
    fallback_run.current_provider_session_id = None;
    if let Some(task) = fallback_run.tasks.get_mut(task_index) {
        task.provider_session_id = None;
    }
    let mut fallback_result = execute_provider_task(state, &fallback_run, task_index).await;
    if let Ok(result) = &mut fallback_result {
        result.stream_events.insert(
            0,
            json!({
                "timestamp": Utc::now(),
                "kind": "status",
                "status": "provider_fallback",
                "content": format!("Primary provider call failed; retried with {provider} {model}"),
                "primaryFailure": reason,
            }),
        );
    }
    ProviderExecutionAttempt {
        result: fallback_result,
        fallback: Some(ProviderFallbackSelection {
            provider,
            model,
            reason,
        }),
    }
}

fn provider_result_session_id(result: &Result<ProviderTaskResult>) -> Option<String> {
    result.as_ref().ok()?.session_id.clone()
}

fn malformed_tool_call_repair_retries(run: &AgenticBoard) -> u64 {
    if run
        .qa_policy
        .get("repairMalformedToolCalls")
        .and_then(value_as_bool)
        .unwrap_or(true)
        == false
    {
        return 0;
    }
    run.qa_policy
        .get("malformedToolCallRepairRetries")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MALFORMED_TOOL_CALL_REPAIR_RETRIES)
        .clamp(0, MAX_MALFORMED_TOOL_CALL_REPAIR_RETRIES)
}

fn provider_result_has_repairable_integer_tool_arg_schema_error(
    result: &Result<ProviderTaskResult>,
) -> bool {
    match result {
        Err(error) => is_repairable_integer_tool_arg_schema_error(&server_error_message(error)),
        Ok(result) => provider_task_result_has_repairable_integer_tool_arg_schema_error(result),
    }
}

fn provider_result_should_attempt_malformed_tool_call_repair(
    result: &Result<ProviderTaskResult>,
) -> bool {
    provider_result_requires_fallback(result)
        && provider_result_has_repairable_integer_tool_arg_schema_error(result)
}

fn provider_task_result_has_repairable_integer_tool_arg_schema_error(
    result: &ProviderTaskResult,
) -> bool {
    let mut parts = vec![
        result.summary.as_str(),
        result.stderr.as_str(),
        result.assistant_text.as_str(),
    ];
    parts.extend(result.errors.iter().map(String::as_str));
    if parts
        .into_iter()
        .any(is_repairable_integer_tool_arg_schema_error)
    {
        return true;
    }
    result.stream_events.iter().any(|event| {
        is_repairable_integer_tool_arg_schema_error(&limit_text(&event.to_string(), 4000))
    })
}

fn malformed_tool_call_repair_instruction(reason: &str) -> String {
    format!(
        "The previous provider attempt failed because a tool call used an integer-valued floating-point JSON number where the tool schema requires an integer. For future tool calls, use strict JSON argument types: integer fields such as session_id, yield_time_ms, max_output_tokens, counts, limits, and offsets must be JSON integers like 60000, never floats like 60000.0. If a prior command session is stale, start a fresh tool call instead of reusing it. Previous failure: {}",
        limit_text(reason, 1200),
    )
}

fn is_repairable_integer_tool_arg_schema_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("failed to parse function arguments")
        && lower.contains("invalid type: floating point")
        && lower.contains("expected")
        && schema_error_expected_integer_type(&lower)
        && text_contains_integer_like_float_literal(text)
}

fn schema_error_expected_integer_type(lower: &str) -> bool {
    [
        "expected i8",
        "expected i16",
        "expected i32",
        "expected i64",
        "expected i128",
        "expected isize",
        "expected u8",
        "expected u16",
        "expected u32",
        "expected u64",
        "expected u128",
        "expected usize",
        "expected integer",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn text_contains_integer_like_float_literal(text: &str) -> bool {
    text.split('`')
        .skip(1)
        .step_by(2)
        .any(is_integer_like_float_literal)
        || text
            .split(|ch: char| !(ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.')))
            .any(is_integer_like_float_literal)
}

fn is_integer_like_float_literal(value: &str) -> bool {
    let value = value
        .trim()
        .trim_matches(|ch: char| matches!(ch, ',' | ';' | ':' | ')' | ']' | '}' | '"'));
    let value = value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value);
    let Some((whole, fraction)) = value.split_once('.') else {
        return false;
    };
    !whole.is_empty()
        && !fraction.is_empty()
        && whole.chars().all(|ch| ch.is_ascii_digit())
        && fraction.chars().all(|ch| ch == '0')
}

fn provider_result_requires_fallback(result: &Result<ProviderTaskResult>) -> bool {
    match result {
        Err(_) => true,
        Ok(result) => {
            result.exit_code != 0
                || !filter_fatal_provider_errors(&result.errors, result.exit_code).is_empty()
        }
    }
}

fn provider_result_failure_summary(result: &Result<ProviderTaskResult>) -> String {
    match result {
        Err(error) => server_error_message(error),
        Ok(result) => result
            .errors
            .first()
            .cloned()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| result.summary.clone()),
    }
}

fn configured_provider_fallback(run: &AgenticBoard) -> Option<(String, String)> {
    let strategy = run.model_strategy.as_ref();
    let provider = trim_string(Some(run.next_provider.clone()))
        .or_else(|| {
            strategy
                .and_then(|value| value.get("fallbackProvider"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| run.provider.clone());
    let model = trim_string(Some(run.next_model.clone()))
        .or_else(|| {
            strategy
                .and_then(|value| value.get("fallbackModel"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| run.model.clone());
    if provider == run.provider && model == run.model {
        return None;
    }
    normalize_optional_provider(Some(&provider))
        .ok()
        .map(|provider| (provider, model))
}

#[derive(Debug)]
struct SharedProviderTurnResult {
    session_id: String,
    assistant_text: String,
    stderr: String,
    token_usage: Option<Value>,
    exit_code: i32,
    summary: String,
}

#[derive(Debug, PartialEq)]
struct BoardProviderControls {
    effort: Option<String>,
    thinking: Option<bool>,
    fast: Option<bool>,
}

fn board_provider_controls(run: &AgenticBoard) -> BoardProviderControls {
    let strategy = run.model_strategy.as_ref();
    let effort = strategy
        .and_then(|value| {
            value
                .get("reasoningEffort")
                .or_else(|| value.get("reasoning_effort"))
                .or_else(|| value.get("effort"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let thinking = strategy.and_then(|value| {
        value
            .get("thinking")
            .or_else(|| value.get("enableThinking"))
            .and_then(Value::as_bool)
    });
    let explicit_fast = strategy.and_then(|value| {
        value
            .get("fast")
            .or_else(|| value.get("fastMode"))
            .or_else(|| value.get("fast_mode"))
            .and_then(value_as_bool)
    });
    let service_tier_fast = strategy
        .and_then(|value| {
            value
                .get("serviceTier")
                .or_else(|| value.get("service_tier"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| value.eq_ignore_ascii_case("fast"));

    BoardProviderControls {
        effort,
        thinking,
        fast: explicit_fast.or(service_tier_fast.then_some(true)),
    }
}

fn value_as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => value.as_u64().and_then(|value| match value {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }),
        Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" | "fast" | "priority" => Some(true),
            "false" | "no" | "off" | "0" | "default" | "standard" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

async fn execute_shared_provider_turn(
    state: &AppState,
    run: &AgenticBoard,
    provider: &str,
    model: &str,
    prompt: &str,
    session_id: Option<&str>,
    board_task_id: Option<&str>,
) -> Result<SharedProviderTurnResult> {
    let provider = provider_enum(provider)?;
    let user_id = run.user_id.clone();
    let runtime = user_id
        .as_deref()
        .map(|user_id| agentic_chat_runtime(state, user_id, provider, session_id, Some(model)))
        .unwrap_or(ChatRuntime::NativeCli);
    let model = trim_string(Some(model.to_string()));
    let direct_ai_config = if runtime == ChatRuntime::IoGateway {
        user_id.as_deref().and_then(|user_id| {
            agentic_direct_ai_runtime_config(state, user_id, provider, model.as_deref())
        })
    } else {
        None
    };
    let controls = board_provider_controls(run);
    // Allocate the Workbench id before start so a provider-start failure can
    // still be linked to its persisted board chat.
    let workbench_session_id = session_id
        .map(str::to_string)
        .unwrap_or_else(|| new_id("session"));
    let session = state
        .start_board_agent_session(
            provider,
            run.project_path.clone(),
            prompt.to_string(),
            Some(workbench_session_id.clone()),
            model.clone(),
            controls.effort,
            Some("bypass".to_string()),
            controls.thinking,
            controls.fast,
            runtime,
            direct_ai_config,
            user_id,
            run.id.clone(),
            board_task_id.map(str::to_string),
        )
        .await;
    let session = match session {
        Ok(session) => session,
        Err(error) => {
            if board_task_id.is_some()
                && state
                    .storage
                    .get_session_summary(&workbench_session_id)?
                    .is_some()
            {
                link_board_task_session(
                    state,
                    run,
                    board_task_id.unwrap_or_default(),
                    &workbench_session_id,
                )?;
            }
            return Err(ServerError::from(error));
        }
    };

    // Persist the task link as soon as the Workbench session exists. This
    // makes Open chat available while the provider is still running and also
    // preserves the id when the provider later fails.
    if let Some(task_id) = board_task_id {
        link_board_task_session(state, run, task_id, &session.id)?;
    }

    wait_for_shared_provider_turn(state, run, provider, session, model).await
}

fn link_board_task_session(
    state: &AppState,
    run: &AgenticBoard,
    task_id: &str,
    session_id: &str,
) -> Result<()> {
    let Some(user_id) = run.user_id.as_deref() else {
        return Ok(());
    };
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, &run.id)?;
    if let Some(task) = stored
        .board
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
    {
        task.provider_session_id = Some(session_id.to_string());
        task.transcript_updated_at = Some(Utc::now());
    }
    stored.board.current_provider_session_id = Some(session_id.to_string());
    stored.board.touch();
    save_board(state, &stored.board)
}

async fn wait_for_shared_provider_turn(
    state: &AppState,
    run: &AgenticBoard,
    provider: Provider,
    session: SessionSummary,
    model: Option<String>,
) -> Result<SharedProviderTurnResult> {
    loop {
        if board_interrupted(state, run) {
            let _ = state.abort_agent_session(provider, &session.id).await;
            return Ok(SharedProviderTurnResult {
                session_id: session.id,
                assistant_text: String::new(),
                stderr: "Provider task was interrupted by board pause/abort.".to_string(),
                token_usage: None,
                exit_code: 130,
                summary: "Provider task was interrupted by board pause/abort.".to_string(),
            });
        }

        let stored_session = state.storage.get_session_summary(&session.id)?;
        let durable_run = state
            .storage
            .latest_durable_chat_run_for_session(&session.id)?;
        let active = stored_session
            .as_ref()
            .is_some_and(|session| session.active);
        let durable_status = durable_run.as_ref().map(|run| run.status.as_str());
        let durable_terminal = durable_status
            .map(|status| !matches!(status, "running" | "recovering"))
            .unwrap_or(false);

        if !active && (durable_run.is_none() || durable_terminal) {
            let messages = state.storage.list_messages(&session.id)?;
            let assistant = messages
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::Assistant);
            let assistant_text = assistant
                .map(|message| limit_text(&message.content, MAX_PROVIDER_OUTPUT_CHARS))
                .unwrap_or_default();
            let token_usage = assistant
                .and_then(|message| message.metadata.get("tokenUsage").cloned())
                .or_else(|| {
                    stored_session
                        .as_ref()
                        .and_then(|session| session.token_usage.as_ref())
                        .and_then(|usage| serde_json::to_value(usage).ok())
                });
            let status = durable_status.unwrap_or(if assistant_text.trim().is_empty() {
                "failed"
            } else {
                "completed"
            });
            let exit_code = if status == "completed" { 0 } else { 1 };
            let mut summary = if exit_code == 0 {
                summarize_provider_output(&assistant_text, "", 0)
            } else {
                durable_run
                    .and_then(|run| run.last_error)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| summarize_provider_output(&assistant_text, "", 1))
            };
            if summary.trim().is_empty()
                && let Some(model) = model.as_deref()
            {
                summary = format!("Shared chat executor completed with model {model}");
            }
            return Ok(SharedProviderTurnResult {
                session_id: session.id,
                assistant_text,
                stderr: if exit_code == 0 {
                    String::new()
                } else {
                    summary.clone()
                },
                token_usage,
                exit_code,
                summary,
            });
        }

        sleep(PROVIDER_POLL_INTERVAL).await;
    }
}

fn board_interrupted(state: &AppState, run: &AgenticBoard) -> bool {
    load_user_board(state, run.user_id.as_deref().unwrap_or_default(), &run.id)
        .map(|stored| board_should_abort_provider(&stored.board))
        .unwrap_or(false)
}

fn board_should_abort_provider(run: &AgenticBoard) -> bool {
    // Abort metadata is retained for auditability. It is not itself an
    // active control signal: older boards can contain a stale canceledAt or
    // cancellationReason after they were resumed. Only the cancelled state
    // may interrupt a provider call.
    run.status == "cancelled"
}

fn board_has_in_flight_work(run: &AgenticBoard) -> bool {
    run.current_provider_session_id.is_some() || run.provider_call_started_at.is_some()
}

fn request_board_pause(run: &mut AgenticBoard, reason: Option<String>) {
    bump_control_revision(run);
    run.auto_run_enabled = false;
    run.pause_reason = reason.or_else(|| Some("user request".to_string()));
    if board_has_in_flight_work(run) {
        run.status = "pausing".to_string();
        run.active = true;
        run.pause_requested = true;
        run.paused_at = None;
        run.append_log("Board pause requested; waiting for current work to finish");
    } else {
        settle_board_pause(run);
        run.append_log("Board paused");
    }
}

fn prepare_board_resume(run: &mut AgenticBoard) {
    bump_control_revision(run);
    clear_board_abort_state(run);
    run.status = "running".to_string();
    run.scheduled_start_at = None;
    run.active = true;
    run.auto_run_enabled = true;
    run.pause_requested = false;
    run.paused_at = None;
    run.pause_reason = None;
    run.append_log("Board resume requested");
}

fn clear_board_abort_state(run: &mut AgenticBoard) {
    run.cancellation_reason = None;
    run.abort_source = None;
    run.abort_requested_at = None;
    run.canceled_at = None;
}

fn settle_board_pause(run: &mut AgenticBoard) {
    if let Some(current_task_id) = run.current_task_id.as_deref()
        && let Some(task) = run.tasks.iter_mut().find(|task| task.id == current_task_id)
        && task_status_is_active(&task.status)
    {
        task.status = TASK_STATUS_TODO.to_string();
        task.started_at = None;
        task.completed_at = None;
        task.provider_session_id = None;
        task.transcript.push(json!({
            "timestamp": Utc::now(),
            "kind": "status",
            "status": TASK_STATUS_TODO,
            "content": "Task returned to Todo because the board paused before provider execution completed",
        }));
        task.transcript_updated_at = Some(Utc::now());
    }
    run.status = "paused".to_string();
    run.active = false;
    run.loop_started = false;
    run.pause_requested = false;
    run.paused_at = Some(Utc::now());
    run.current_task_id = None;
    run.current_task_title.clear();
    run.current_task_status.clear();
    run.append_log("Agentic board execution paused");
}

fn reset_in_flight_board_tasks(run: &mut AgenticBoard, message: &str) {
    for task in &mut run.tasks {
        if !task_status_is_active(&task.status) {
            continue;
        }
        task.status = TASK_STATUS_TODO.to_string();
        task.started_at = None;
        task.completed_at = None;
        task.provider_session_id = None;
        task.transcript.push(json!({
            "timestamp": Utc::now(),
            "kind": "status",
            "status": TASK_STATUS_TODO,
            "content": message,
        }));
        task.transcript_updated_at = Some(Utc::now());
    }
}

fn bump_control_revision(run: &mut AgenticBoard) {
    run.control_revision = run.control_revision.saturating_add(1);
}

fn shared_provider_stream_events(provider: &str, result: &SharedProviderTurnResult) -> Vec<Value> {
    let mut events = Vec::new();
    if !result.assistant_text.trim().is_empty() {
        events.push(json!({
            "timestamp": Utc::now(),
            "provider": provider,
            "kind": "assistant",
            "isError": false,
            "content": result.assistant_text,
        }));
    }
    if result.exit_code != 0 {
        events.push(json!({
            "timestamp": Utc::now(),
            "provider": provider,
            "kind": "error",
            "isError": true,
            "content": result.summary,
        }));
    }
    events
}

fn provider_enum(provider: &str) -> Result<Provider> {
    match provider {
        "claude" => Ok(Provider::Claude),
        "codex" => Ok(Provider::Codex),
        "gemini" => Ok(Provider::Gemini),
        _ => Err(bad_request(
            "Provider must be one of: claude, codex, gemini",
        )),
    }
}

fn agentic_chat_runtime(
    state: &AppState,
    user_id: &str,
    provider: Provider,
    session_id: Option<&str>,
    model: Option<&str>,
) -> ChatRuntime {
    if provider == Provider::Gemini {
        return ChatRuntime::NativeCli;
    }
    if model.is_some_and(agentic_is_io_gateway_model) {
        return ChatRuntime::IoGateway;
    }
    if let Some(session) = session_id
        .and_then(|session_id| state.storage.get_session_summary(session_id).ok().flatten())
    {
        return session.runtime.unwrap_or_else(|| {
            if model
                .or(session.model.as_deref())
                .is_some_and(agentic_is_io_gateway_model)
            {
                ChatRuntime::IoGateway
            } else {
                ChatRuntime::NativeCli
            }
        });
    }
    let key = agentic_user_setting_key(user_id, "direct-ai");
    let config = state.storage.get_setting(&key).ok().flatten();
    config
        .as_ref()
        .and_then(|config| {
            config
                .get("chatRuntime")
                .or_else(|| config.get("chat_runtime"))
        })
        .and_then(Value::as_str)
        .and_then(agentic_parse_chat_runtime)
        .unwrap_or_else(|| {
            let has_legacy_key = config
                .as_ref()
                .is_some_and(|config| agentic_secret_value(config, "gatewayApiKey").is_some())
                || state
                    .storage
                    .get_active_credential_value_by_name(
                        user_id,
                        IO_GATEWAY_API_KEY_CREDENTIAL,
                        IO_GATEWAY_API_KEY_CREDENTIAL_TYPE,
                    )
                    .ok()
                    .flatten()
                    .is_some();
            if has_legacy_key {
                ChatRuntime::IoGateway
            } else {
                ChatRuntime::NativeCli
            }
        })
}

fn agentic_direct_ai_runtime_config(
    state: &AppState,
    user_id: &str,
    provider: Provider,
    model: Option<&str>,
) -> Option<DirectAiRuntimeConfig> {
    if provider == Provider::Claude && model.is_some_and(agentic_model_uses_minimax_gateway) {
        return agentic_minimax_runtime_config(state, user_id);
    }
    let mut config = state
        .storage
        .get_setting(&agentic_user_setting_key(user_id, "direct-ai"))
        .ok()
        .flatten()
        .unwrap_or_else(agentic_default_direct_ai_config);
    if let Some(obj) = config.as_object_mut()
        && let Ok(Some(secret)) = state.storage.get_active_credential_value_by_name(
            user_id,
            IO_GATEWAY_API_KEY_CREDENTIAL,
            IO_GATEWAY_API_KEY_CREDENTIAL_TYPE,
        )
    {
        obj.insert("gatewayApiKey".to_string(), Value::String(secret));
    }
    if matches!(provider, Provider::Codex | Provider::Claude) {
        agentic_apply_io_gateway_config(&mut config, provider);
    }
    let (base_url, api_key) = agentic_direct_ai_endpoint_config(&config)?;
    let max_tokens = config
        .get("maxTokens")
        .or_else(|| config.get("max_tokens"))
        .and_then(Value::as_u64);
    Some(DirectAiRuntimeConfig {
        base_url,
        api_key,
        max_tokens,
    })
}

fn agentic_apply_io_gateway_config(config: &mut Value, provider: Provider) {
    if !config.is_object() {
        *config = agentic_default_direct_ai_config();
    }
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    obj.insert("mode".to_string(), Value::String("aiproxy".to_string()));
    obj.remove("base_url");
    let endpoint = obj
        .get(if provider == Provider::Codex {
            "codexEndpoint"
        } else {
            "claudeEndpoint"
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(if provider == Provider::Codex {
            "codex"
        } else {
            "claude"
        });
    let gateway_root = obj
        .get("gatewayUrl")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .or_else(|| {
            obj.get("baseUrl")
                .and_then(Value::as_str)
                .and_then(|value| {
                    let value = value.trim().trim_end_matches('/');
                    if join_io_gateway_endpoint_url(value, endpoint) == value {
                        Some(value.to_string())
                    } else {
                        agentic_url_origin(value)
                    }
                })
        })
        .unwrap_or_else(|| {
            agentic_url_origin(DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL)
                .unwrap_or_else(|| DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL.to_string())
        });
    let base_url = join_io_gateway_endpoint_url(&gateway_root, endpoint);
    obj.insert("baseUrl".to_string(), Value::String(base_url));
    obj.remove("api_key_env");
    obj.remove("apiKeyEnv");
}

fn agentic_direct_ai_endpoint_config(config: &Value) -> Option<(String, String)> {
    let mode = config.get("mode").and_then(Value::as_str).unwrap_or("off");
    if mode == "off" || mode.is_empty() {
        return None;
    }
    let base_url = config
        .get("baseUrl")
        .or_else(|| config.get("base_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .or_else(|| match mode {
            "direct" | "anthropic" => Some("https://api.anthropic.com".to_string()),
            "minimax" => Some("https://api.minimax.io/anthropic".to_string()),
            "proxy" | "aiproxy" => Some(DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL.to_string()),
            _ => None,
        })?;
    let env_key = config
        .get("apiKeyEnv")
        .or_else(|| config.get("api_key_env"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let stored_gateway_key = agentic_secret_value(config, "gatewayApiKey");
    let api_key = if matches!(mode, "proxy" | "aiproxy") {
        stored_gateway_key
    } else {
        stored_gateway_key.or_else(|| {
            env_key
                .and_then(|key| env::var(key).ok())
                .or_else(|| match mode {
                    "direct" | "anthropic" => env::var("ANTHROPIC_API_KEY")
                        .or_else(|_| env::var("ANTHROPIC_AUTH_TOKEN"))
                        .ok(),
                    "minimax" => env::var("MINIMAX_API_KEY")
                        .or_else(|_| env::var("ANTHROPIC_API_KEY"))
                        .ok(),
                    _ => None,
                })
        })
    }
    .filter(|value| !value.trim().is_empty())?;
    Some((base_url, api_key))
}

fn agentic_model_uses_minimax_gateway(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase().replace('_', "-");
    normalized.starts_with("min:") || matches!(normalized.as_str(), "minimax-m3" | "minimaxm3")
}

fn agentic_minimax_runtime_config(
    state: &AppState,
    user_id: &str,
) -> Option<DirectAiRuntimeConfig> {
    let settings = state
        .storage
        .get_setting(&agentic_user_setting_key(user_id, "claude-settings"))
        .ok()
        .flatten()
        .unwrap_or_else(agentic_default_claude_agent_settings);
    let base_url = settings
        .get("minimaxBaseUrl")
        .or_else(|| settings.get("minimax_base_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("https://api.minimax.io/anthropic")
        .trim_end_matches('/')
        .to_string();
    let key_env = settings
        .get("minimaxApiKeyEnv")
        .or_else(|| settings.get("minimax_api_key_env"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("MINIMAX_API_KEY");
    let api_key = agentic_secret_value(&settings, "minimaxApiKey")
        .or_else(|| agentic_secret_value(&settings, "minimax_api_key"))
        .or_else(|| env::var(key_env).ok())
        .or_else(|| env::var("MINIMAX_API_KEY").ok())
        .filter(|value| !value.trim().is_empty())?;
    Some(DirectAiRuntimeConfig {
        base_url,
        api_key,
        max_tokens: None,
    })
}

fn agentic_default_claude_agent_settings() -> Value {
    json!({
        "minimaxBaseUrl": "https://api.minimax.io/anthropic",
        "minimaxApiKeyEnv": "MINIMAX_API_KEY",
        "minimaxModel": "MiniMax-M3",
    })
}

fn agentic_parse_chat_runtime(value: &str) -> Option<ChatRuntime> {
    match value.trim().to_ascii_lowercase().as_str() {
        "native_cli" | "native" | "cli" | "default" => Some(ChatRuntime::NativeCli),
        "io_gateway" | "gateway" | "custom_api" | "aiproxy" => Some(ChatRuntime::IoGateway),
        _ => None,
    }
}

fn agentic_is_io_gateway_model(model: &str) -> bool {
    let trimmed = model.trim();
    let Some((prefix, rest)) = trimmed.split_once(':') else {
        return false;
    };
    let normalized = prefix.to_ascii_lowercase();
    !rest.trim().is_empty()
        && matches!(
            normalized.as_str(),
            "agw"
                | "cod"
                | "proxy"
                | "gateway"
                | "aiproxy"
                | "cld"
                | "gem"
                | "cop"
                | "ctm"
                | "dsk"
                | "glm"
                | "grk"
                | "min"
        )
}

fn agentic_secret_value(config: &Value, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn agentic_url_origin(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let scheme_end = trimmed.find("://")?;
    let after_scheme = &trimmed[scheme_end + 3..];
    let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());
    Some(
        trimmed[..scheme_end + 3 + path_start]
            .trim_end_matches('/')
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

fn agentic_user_setting_key(user_id: &str, key: &str) -> String {
    format!("user:{user_id}:{key}")
}

fn agentic_default_direct_ai_config() -> Value {
    json!({
        "mode": "off",
        "chatRuntime": "native_cli",
        "baseUrl": null,
        "apiKeyEnv": null,
        "model": null
    })
}

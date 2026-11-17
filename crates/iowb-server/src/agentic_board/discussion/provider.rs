async fn execute_discussion_prompt(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    label: &str,
    prompt: &str,
) -> Result<String> {
    execute_discussion_prompt_with_provider(
        state,
        user_id,
        board_id,
        label,
        prompt,
        |state, run, provider, model, prompt, session_id, task_id| async move {
            execute_shared_provider_turn(
                &state,
                &run,
                &provider,
                &model,
                &prompt,
                session_id.as_deref(),
                task_id.as_deref(),
            )
            .await
        },
    )
    .await
}

async fn execute_discussion_prompt_with_provider<F, Fut>(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    label: &str,
    prompt: &str,
    provider_call: F,
) -> Result<String>
where
    F: FnOnce(
        AppState,
        AgenticBoard,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
    ) -> Fut,
    Fut: std::future::Future<Output = Result<SharedProviderTurnResult>>,
{
    let call_id = Uuid::new_v4().to_string();
    let (run, provider, execution_model) = {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(state, user_id, board_id)?;
        let provider = effective_provider_for_phase(&stored.board, label)?;
        let model = effective_model_for_phase(&stored.board, label);
        let execution_model = agentic_execution_model_for_provider(&provider, &model);
        let mut telemetry = json!({
            "callId": call_id,
            "phase": "discussion",
            "label": label,
            "provider": provider,
            "model": model,
            "chars": prompt.chars().count(),
            "estimatedTokens": estimate_tokens(prompt),
            "startedAt": Utc::now(),
        });
        if execution_model != model {
            telemetry["executionModel"] = json!(execution_model);
        }
        stored.board.prompt_telemetry.push(telemetry);
        stored.board.touch();
        save_board(state, &stored.board)?;
        (stored.board, provider, execution_model)
    };

    // A proposal is not an execution turn. Never resume a board/task session:
    // starting a second Codex turn there interrupts the executing task. Also
    // omit the task link, which would otherwise replace its execution chat and
    // the board's current_provider_session_id even with a fresh session.
    let result = provider_call(
        state.clone(),
        run,
        provider,
        execution_model.clone(),
        prompt.to_string(),
        None,
        None,
    )
    .await
    .and_then(|result| {
        if result.exit_code == 0 {
            Ok(result)
        } else {
            Err(ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("provider call failed during {label}"),
                result.summary,
            ))
        }
    });

    // Reload under the mutation lock: task execution may have advanced while
    // the proposal was generated. Its current-call pointers must stay intact
    // on both success and failure. A stable call id avoids finalizing another
    // operation's telemetry when concurrent work appends/removes entries.
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, board_id)?;
    let telemetry_index =
        stored.board.prompt_telemetry.iter().position(|entry| {
            entry.get("callId").and_then(Value::as_str) == Some(call_id.as_str())
        });
    match &result {
        Ok(output) => {
            if let Some(index) = telemetry_index {
                let execution_last_model = stored.board.last_effective_model.clone();
                finalize_prompt_telemetry(
                    &mut stored.board,
                    index,
                    Some(&output.session_id),
                    Some(&execution_model),
                    output.token_usage.as_ref(),
                );
                // The discussion has its own effectiveModel in telemetry; it
                // must not change the running execution model shown on board.
                stored.board.last_effective_model = execution_last_model;
                stored.board.prompt_telemetry[index]["outcome"] = json!("completed");
            }
            increment_provider_usage(
                &mut stored.board,
                prompt,
                &output.assistant_text,
                Some(&output.session_id),
                output.token_usage.as_ref(),
            );
            stored
                .board
                .append_log(format!("Discussion provider call completed: {label}"));
        }
        Err(error) => {
            let error = server_error_message(error);
            if let Some(index) = telemetry_index {
                stored.board.prompt_telemetry[index]["outcome"] = json!("failed");
                stored.board.prompt_telemetry[index]["error"] = json!(error);
                stored.board.prompt_telemetry[index]["completedAt"] = json!(Utc::now());
            }
            stored.board.append_log(format!(
                "Discussion provider call failed for {label}: {error}"
            ));
        }
    }
    stored.board.touch();
    save_board(state, &stored.board)?;
    result.map(|output| output.assistant_text)
}

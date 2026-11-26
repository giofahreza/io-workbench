async fn initialize_app_server(
    stdin: &mut AppServerWriter,
    lines: &mut AppServerLines,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
    shared: bool,
    codex_home: Option<&std::path::Path>,
) -> Result<()> {
    write_json_line(
        stdin,
        &json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "io_workbench",
                    "title": "io-workbench",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimentalApi": true,
                    "optOutNotificationMethods": ["thread/started"],
                },
            },
        }),
    )
    .await?;
    let result = read_response_with_events(lines, stdin, 1, events, None, shared).await?;
    if shared {
        validate_shared_codex_home(&result, codex_home)?;
    }
    write_json_line(stdin, &json!({ "method": "initialized", "params": {} })).await
}

async fn start_or_resume_thread(
    stdin: &mut AppServerWriter,
    lines: &mut AppServerLines,
    params: &CodexAppServerLiveTurnParams,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
    shared: bool,
) -> Result<String> {
    let existing_thread_id = params
        .thread_id
        .as_deref()
        .map(str::trim)
        .filter(|thread_id| !thread_id.is_empty());
    let inherit = shared && existing_thread_id.is_some();
    let mut request_params = json!({});
    if !inherit {
        request_params["cwd"] = json!(params.cwd);
    }
    if !inherit && let Some(policy) = params.approval_policy.as_ref() {
        request_params["approvalPolicy"] = policy.clone();
    }
    if !inherit {
        insert_optional_string(&mut request_params, "model", params.model.as_deref());
        insert_optional_string(
            &mut request_params,
            "modelProvider",
            params.model_provider.as_deref(),
        );
        // Thread start/resume accepts effort through config; only turn/start has
        // an `effort` field. Explicit config also overrides saved resume settings.
        if let Some(effort) = params.effort.as_deref() {
            request_params["config"] = json!({"model_reasoning_effort": effort});
        }
        insert_optional_string(
            &mut request_params,
            "serviceTier",
            params.service_tier.as_deref(),
        );
    }
    if let Some(thread_id) = existing_thread_id {
        request_params["threadId"] = Value::String(thread_id.to_string());
        // Live turns only need the thread id. Returning the entire saved
        // transcript can make resuming a long-lived chat exceed setup timeouts.
        request_params["excludeTurns"] = Value::Bool(true);
    }
    let method = if existing_thread_id.is_some() {
        "thread/resume"
    } else {
        "thread/start"
    };
    write_json_line(
        stdin,
        &json!({
            "method": method,
            "id": 2,
            "params": request_params,
        }),
    )
    .await?;
    let result =
        read_response_with_events(lines, stdin, 2, events, existing_thread_id, shared).await?;
    if shared
        && params.recovery.is_none()
        && result
            .pointer("/thread/status/type")
            .and_then(Value::as_str)
            == Some("active")
    {
        return Err(CoreError::Conflict("Codex is still running a turn in this chat. Wait for it to finish, then retry; your CLI can stay open.".into()));
    }
    extract_thread_id(&result)
        .or_else(|| existing_thread_id.map(str::to_string))
        .ok_or_else(|| {
            CoreError::InvalidInput("Codex app-server did not return a thread id".to_string())
        })
}

async fn start_turn(
    stdin: &mut AppServerWriter,
    lines: &mut AppServerLines,
    params: &CodexAppServerLiveTurnParams,
    thread_id: &str,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
    shared: bool,
    request_timeout: Duration,
    abort_rx: &mut oneshot::Receiver<()>,
) -> Result<(Option<String>, Option<Value>)> {
    if shared {
        return start_shared_turn(
            stdin,
            lines,
            params,
            thread_id,
            events,
            request_timeout,
            abort_rx,
        )
        .await;
    }
    persist_submission_intent(params, events).await?;
    let mut request_params = json!({
        "threadId": thread_id,
        "input": params.input,
        "cwd": params.cwd.display().to_string(),
    });
    if let Some(policy) = params.approval_policy.as_ref() {
        request_params["approvalPolicy"] = policy.clone();
    }
    insert_optional_string(
        &mut request_params,
        "clientUserMessageId",
        params.client_user_message_id.as_deref(),
    );
    insert_optional_string(&mut request_params, "model", params.model.as_deref());
    insert_optional_string(&mut request_params, "effort", params.effort.as_deref());
    insert_optional_string(
        &mut request_params,
        "serviceTier",
        params.service_tier.as_deref(),
    );
    if let Some(sandbox_policy) = params.sandbox_policy.clone() {
        request_params["sandboxPolicy"] = sandbox_policy;
    }
    write_json_line(
        stdin,
        &json!({
            "method": "turn/start",
            "id": 3,
            "params": request_params,
        }),
    )
    .await?;
    let result = read_response_with_events(lines, stdin, 3, events, None, shared).await?;
    Ok((
        result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .map(str::to_string),
        None,
    ))
}

fn insert_optional_string(target: &mut Value, key: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    target[key] = Value::String(value.to_string());
}

fn extract_thread_id(result: &Value) -> Option<String> {
    result
        .pointer("/thread/id")
        .or_else(|| result.get("threadId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|thread_id| !thread_id.is_empty())
        .map(str::to_string)
}

async fn read_response_with_events(
    lines: &mut AppServerLines,
    stdin: &mut AppServerWriter,
    expected_id: i64,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
    baseline_thread_id: Option<&str>,
    shared: bool,
) -> Result<Value> {
    let mut deferred = Vec::new();
    let mut deferred_bytes = 0usize;
    while let Some(line) = lines.next_line().await? {
        let value: Value = serde_json::from_str(&line)?;
        if value.get("method").is_none()
            && value.get("id").and_then(Value::as_i64) == Some(expected_id)
        {
            lines.prepend(deferred);
            if let Some(error) = value.get("error") {
                return Err(app_server_response_error(error));
            }
            let result = value.get("result").cloned().ok_or_else(|| {
                CoreError::InvalidInput(
                    "Codex app-server response did not contain a result".to_string(),
                )
            });
            return result;
        }
        if shared && matches!(expected_id, 10..=15) {
            deferred_bytes = deferred_bytes.saturating_add(line.len());
            if deferred_bytes > 8 * 1024 * 1024 {
                return Err(CoreError::InvalidInput("Codex produced too many notifications before acknowledging the request; it was not resubmitted".into()));
            }
            deferred.push(line);
            continue;
        }
        handle_nonmatching_jsonrpc_message(&value, stdin, events, baseline_thread_id, shared)
            .await?;
    }
    Err(CoreError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "Codex app-server closed before replying",
    )))
}

async fn handle_nonmatching_jsonrpc_message(
    value: &Value,
    stdin: &mut AppServerWriter,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
    baseline_thread_id: Option<&str>,
    shared: bool,
) -> Result<()> {
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return Ok(());
    };
    if let Some(id) = value.get("id").cloned() {
        if !shared {
            respond_to_server_request(stdin, id, method).await?;
        }
        return Ok(());
    }
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    if let Some(thread_id) = baseline_thread_id
        && method == "thread/tokenUsage/updated"
    {
        if !notification_matches_thread(&params, thread_id) {
            return Ok(());
        }
        if let Some(token_usage) = params
            .get("tokenUsage")
            .or_else(|| params.get("token_usage"))
        {
            let _ = events
                .send(CodexAppServerLiveTurnEvent::UsageBaseline {
                    token_usage: token_usage.clone(),
                })
                .await;
        }
        return Ok(());
    }
    if shared {
        return Ok(());
    }
    let _ = events
        .send(CodexAppServerLiveTurnEvent::Notification {
            method: method.to_string(),
            params,
        })
        .await;
    Ok(())
}

async fn handle_live_jsonrpc_line(
    line: &str,
    stdin: &mut AppServerWriter,
    expected_thread_id: &str,
    turn_id: &mut Option<String>,
    saw_current_turn_notification: &mut bool,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
    shared: bool,
) -> Result<Option<CodexAppServerLiveTurnOutcome>> {
    let value: Value = serde_json::from_str(line)?;
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return Ok(None);
    };
    if let Some(id) = value.get("id").cloned() {
        if !shared {
            respond_to_server_request(stdin, id, method).await?;
        } else if value.pointer("/params/threadId").and_then(Value::as_str)
            == Some(expected_thread_id)
            && value.pointer("/params/turnId").and_then(Value::as_str) == turn_id.as_deref()
        {
            let _ = events.send(CodexAppServerLiveTurnEvent::Notification {
                method: "warning".into(),
                params: json!({"threadId": expected_thread_id, "turnId": turn_id,
                    "message": format!("Codex needs approval or user input ({method}). Answer in the attached Codex CLI, or open `codex --remote unix:// resume {expected_thread_id}`. Workbench has not declined the request or changed your permission policy.")}),
            }).await;
        }
        return Ok(None);
    }
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    let matches_thread = notification_matches_thread(&params, expected_thread_id);
    if matches_thread {
        if turn_id.is_none()
            && let Some(observed_turn_id) = notification_turn_id(&params).map(str::to_string)
        {
            *turn_id = Some(observed_turn_id.clone());
            let _ = events
                .send(CodexAppServerLiveTurnEvent::TurnAssociated {
                    turn_id: observed_turn_id,
                })
                .await;
        }
    }
    let matches_turn = notification_matches_turn(&params, turn_id.as_deref());
    let is_thread_level = notification_is_thread_level(method);
    let has_notification_turn_id = notification_turn_id(&params).is_some();
    let should_forward = if is_thread_level {
        // A billed response can arrive before any visible assistant output.
        (method == "thread/tokenUsage/updated" && matches_turn && has_notification_turn_id)
            || (*saw_current_turn_notification && (matches_turn || !has_notification_turn_id))
    } else {
        matches_turn
    };
    if matches_thread && should_forward {
        let _ = events
            .send(CodexAppServerLiveTurnEvent::Notification {
                method: method.to_string(),
                params: params.clone(),
            })
            .await;
    }
    if matches_thread
        && matches_turn
        && has_notification_turn_id
        && notification_confirms_current_turn_activity(method, &params)
    {
        *saw_current_turn_notification = true;
    }
    if method != "turn/completed" || !matches_thread || !matches_turn {
        return Ok(None);
    }
    let turn = params.get("turn").cloned().unwrap_or(Value::Null);
    let status = match turn
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed")
    {
        "completed" => CodexAppServerTurnTerminalStatus::Completed,
        "interrupted" => CodexAppServerTurnTerminalStatus::Interrupted,
        _ => CodexAppServerTurnTerminalStatus::Failed,
    };
    Ok(Some(CodexAppServerLiveTurnOutcome {
        thread_id: expected_thread_id.to_string(),
        turn_id: turn_id.clone(),
        status,
        turn: Some(turn),
    }))
}

async fn respond_to_server_request(
    stdin: &mut AppServerWriter,
    id: Value,
    method: &str,
) -> Result<()> {
    let result = match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            json!({ "decision": "decline" })
        }
        "item/permissions/requestApproval" => json!({ "permissions": {} }),
        "item/tool/requestUserInput" => json!({ "answers": {} }),
        "mcpServer/elicitation/request" => json!({ "action": "decline", "content": null }),
        "item/tool/call" => {
            json!({
                "contentItems": [
                    {
                        "type": "inputText",
                        "text": "io-workbench does not provide client-side dynamic tools for this run",
                    }
                ],
                "success": false,
            })
        }
        "account/chatgptAuthTokens/refresh" => {
            write_json_line(
                stdin,
                &json!({
                    "id": id,
                    "error": {
                        "code": -32000,
                        "message": "io-workbench does not provide external ChatGPT auth tokens for Codex app-server",
                    },
                }),
            )
            .await?;
            return Ok(());
        }
        "attestation/generate" => {
            write_json_line(
                stdin,
                &json!({
                    "id": id,
                    "error": {
                        "code": -32000,
                        "message": "io-workbench does not provide client attestation for Codex app-server",
                    },
                }),
            )
            .await?;
            return Ok(());
        }
        "execCommandApproval" | "applyPatchApproval" => json!({ "decision": "denied" }),
        _ => {
            write_json_line(
                stdin,
                &json!({
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("io-workbench cannot satisfy app-server request: {method}"),
                    },
                }),
            )
            .await?;
            return Ok(());
        }
    };
    write_json_line(stdin, &json!({ "id": id, "result": result })).await
}

async fn persist_submission_intent(
    params: &CodexAppServerLiveTurnParams,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
) -> Result<()> {
    if !params.durable_submission {
        return Ok(());
    }
    let (acknowledgement, persisted) = oneshot::channel();
    events
        .send(CodexAppServerLiveTurnEvent::SubmissionStarted { acknowledgement })
        .await
        .map_err(|_| {
            CoreError::InvalidInput("Codex submission persistence is unavailable".into())
        })?;
    persisted
        .await
        .map_err(|_| {
            CoreError::InvalidInput("Codex submission persistence was interrupted".into())
        })?
        .map_err(CoreError::InvalidInput)
}

async fn shared_request(
    stdin: &mut AppServerWriter,
    lines: &mut AppServerLines,
    method: &str,
    request_id: i64,
    params: Value,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
    thread_id: &str,
    request_timeout: Duration,
) -> Result<Value> {
    timeout(request_timeout, async {
        write_json_line(
            stdin,
            &json!({"method": method, "id": request_id, "params": params}),
        )
        .await?;
        read_response_with_events(lines, stdin, request_id, events, Some(thread_id), true).await
    })
    .await
    .map_err(|_| live_turn_setup_timeout("native request", request_timeout))?
}

async fn pending_shared_submission(
    stdin: &mut AppServerWriter,
    lines: &mut AppServerLines,
    thread_id: &str,
    client_id: &str,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
    request_timeout: Duration,
) -> Result<Option<String>> {
    let mut cursor = Value::Null;
    let mut found = None;
    loop {
        let result = shared_request(
            stdin,
            lines,
            "thread/queue/list",
            10,
            json!({"threadId": thread_id, "limit": 100, "cursor": cursor}),
            events,
            thread_id,
            request_timeout,
        )
        .await?;
        for entry in result
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if entry.get("clientUserMessageId").and_then(Value::as_str) == Some(client_id) {
                let id = entry.get("id").and_then(Value::as_str).ok_or_else(|| {
                    CoreError::InvalidInput("Codex queued submission omitted its id".into())
                })?;
                if found.replace(id.to_string()).is_some() {
                    return Err(CoreError::Conflict("Codex has multiple queued copies of this request. No additional message was sent; inspect the CLI queue before retrying.".into()));
                }
            }
        }
        cursor = result.get("nextCursor").cloned().unwrap_or(Value::Null);
        if cursor.is_null() {
            break;
        }
    }
    Ok(found)
}

async fn start_shared_turn(
    stdin: &mut AppServerWriter,
    lines: &mut AppServerLines,
    params: &CodexAppServerLiveTurnParams,
    thread_id: &str,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
    request_timeout: Duration,
    abort_rx: &mut oneshot::Receiver<()>,
) -> Result<(Option<String>, Option<Value>)> {
    let client_id = params.client_user_message_id.as_deref().ok_or_else(|| {
        CoreError::InvalidInput("Shared Codex messages require a stable client message id".into())
    })?;
    let pending =
        pending_shared_submission(stdin, lines, thread_id, client_id, events, request_timeout)
            .await?;
    if pending.is_none()
        && params
            .recovery
            .as_ref()
            .is_some_and(|recovery| recovery.submission_started)
    {
        return Err(CoreError::Conflict("Codex submission outcome is uncertain. No matching native turn or queued message was found, so Workbench did not resend it. Inspect native history before sending again.".into()));
    }
    let mut settings = json!({"threadId": thread_id});
    insert_optional_string(&mut settings, "model", params.model.as_deref());
    insert_optional_string(&mut settings, "effort", params.effort.as_deref());
    insert_optional_string(&mut settings, "serviceTier", params.service_tier.as_deref());
    if let Some(policy) = params.approval_policy.as_ref() {
        settings["approvalPolicy"] = policy.clone();
    }
    if let Some(policy) = params.sandbox_policy.as_ref() {
        settings["sandboxPolicy"] = policy.clone();
    }
    if params.recovery.is_none()
        && settings
            .as_object()
            .is_some_and(|settings| settings.len() > 1)
    {
        shared_request(
            stdin,
            lines,
            "thread/settings/update",
            11,
            settings,
            events,
            thread_id,
            request_timeout,
        )
        .await?;
    }
    let queued_id = if let Some(pending) = pending {
        pending
    } else {
        persist_submission_intent(params, events).await?;
        let result = shared_request(
            stdin,
            lines,
            "thread/queue/add",
            12,
            json!({"threadId": thread_id, "clientUserMessageId": client_id, "input": params.input}),
            events,
            thread_id,
            request_timeout,
        )
        .await?;
        result
            .pointer("/queuedSubmission/id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                CoreError::InvalidInput(
                    "Codex did not return the queued submission id; it was not resubmitted".into(),
                )
            })?
    };
    let mut missing_since = None;
    let mut cancelled = false;
    loop {
        if !cancelled
            && !matches!(
                abort_rx.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            )
        {
            cancelled = true;
            shared_request(
                stdin,
                lines,
                "thread/queue/delete",
                14,
                json!({"threadId": thread_id, "queuedSubmissionId": queued_id}),
                events,
                thread_id,
                request_timeout,
            )
            .await?;
        }
        let result = if cancelled {
            None
        } else {
            Some(
                shared_request(
                    stdin,
                    lines,
                    "thread/queue/start",
                    13,
                    json!({"threadId": thread_id, "queuedSubmissionId": queued_id}),
                    events,
                    thread_id,
                    request_timeout,
                )
                .await,
            )
        };
        if let Some(Ok(result)) = result.as_ref() {
            return result.pointer("/turn/id").and_then(Value::as_str)
                .map(|id| (Some(id.to_string()), None))
                .ok_or_else(|| CoreError::InvalidInput("Codex accepted the queued message without a turn id; it was not resubmitted".into()));
        }
        if let Some(turn) =
            recover_existing_turn(stdin, lines, params, thread_id, events, request_timeout).await?
        {
            let turn_id = turn.get("id").and_then(Value::as_str).map(str::to_string);
            return Ok((turn_id, Some(turn)));
        }
        if cancelled {
            return Ok((None, Some(json!({"status":"interrupted", "items":[]}))));
        }
        let error = result
            .expect("queue start result")
            .expect_err("queue start error");
        let detail = error.to_string();
        if !detail.contains("queued submission not found:")
            && !detail.contains("thread already has an active or pending turn")
        {
            return Err(CoreError::Conflict(format!(
                "Codex could not confirm this queued message. Workbench did not resend or remove it. Inspect the native CLI queue and history before retrying. {error}"
            )));
        }
        if pending_shared_submission(stdin, lines, thread_id, client_id, events, request_timeout)
            .await?
            .is_some()
        {
            missing_since = None;
        } else if missing_since.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(5)
        {
            return Err(CoreError::Conflict("Codex submission outcome is uncertain. Another native client may have claimed this message; Workbench did not resend it. Inspect native history before sending again.".into()));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn recover_existing_turn(
    stdin: &mut AppServerWriter,
    lines: &mut AppServerLines,
    params: &CodexAppServerLiveTurnParams,
    thread_id: &str,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
    request_timeout: Duration,
) -> Result<Option<Value>> {
    let recovery = params.recovery.as_ref();
    let mut cursor = Value::Null;
    loop {
        let result = shared_request(stdin, lines, "thread/turns/list", 15,
            json!({"threadId": thread_id, "limit": 50, "itemsView": "full", "sortDirection": "desc", "cursor": cursor}), events, thread_id, request_timeout).await?;
        for turn in result
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let matches_id = recovery
                .and_then(|recovery| recovery.turn_id.as_deref())
                .is_some_and(|id| turn.get("id").and_then(Value::as_str) == Some(id));
            let matches_client =
                params
                    .client_user_message_id
                    .as_deref()
                    .is_some_and(|client_id| {
                        turn.get("items")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .any(|item| {
                                item.get("type").and_then(Value::as_str) == Some("userMessage")
                                    && (item.get("clientId").and_then(Value::as_str)
                                        == Some(client_id)
                                        || item.get("id").and_then(Value::as_str)
                                            == Some(client_id))
                            })
                    });
            if matches_id || matches_client {
                return Ok(Some(turn.clone()));
            }
        }
        cursor = result.get("nextCursor").cloned().unwrap_or(Value::Null);
        if cursor.is_null() || recovery.is_none() {
            return Ok(None);
        }
    }
}

async fn replay_recovered_turn(
    turn: &Value,
    thread_id: &str,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
) -> Result<String> {
    let turn_id = turn
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::InvalidInput("Recovered Codex turn omitted its id".into()))?
        .to_string();
    let _ = events
        .send(CodexAppServerLiveTurnEvent::TurnAssociated {
            turn_id: turn_id.clone(),
        })
        .await;
    for item in turn
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let _ = events
            .send(CodexAppServerLiveTurnEvent::Notification {
                method: "item/completed".into(),
                params: json!({"threadId": thread_id, "turnId": turn_id, "item": item}),
            })
            .await;
    }
    Ok(turn_id)
}

fn app_server_response_error(error: &Value) -> CoreError {
    let code = error.get("code").and_then(Value::as_i64);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown app-server error");
    let detail = error.get("data").map(Value::to_string);
    CoreError::InvalidInput(match (code, detail) {
        (Some(code), Some(detail)) => {
            format!("Codex app-server error {code}: {message} ({detail})")
        }
        (Some(code), None) => format!("Codex app-server error {code}: {message}"),
        (None, Some(detail)) => format!("Codex app-server error: {message} ({detail})"),
        (None, None) => format!("Codex app-server error: {message}"),
    })
}

fn app_server_command(command: &OsString, launch_options: &CodexAppServerLaunchOptions) -> Command {
    let mut child_command = Command::new(command);
    if launch_options.args.is_empty() {
        child_command.arg("app-server").arg("--stdio");
    } else {
        child_command.args(&launch_options.args);
    }
    child_command.env("PATH", augmented_user_path());
    for (key, value) in &launch_options.env {
        child_command.env(key, value);
    }
    child_command
}

fn start_stderr_capture(child: &mut Child, operation: &'static str) -> AppServerStderr {
    let captured = Arc::new(Mutex::new(String::new()));
    let Some(mut stderr) = child.stderr.take() else {
        return captured;
    };
    let captured_for_task = captured.clone();
    tokio::spawn(async move {
        let mut buffer = [0_u8; 4096];
        loop {
            match stderr.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    let chunk = String::from_utf8_lossy(&buffer[..read]);
                    let mut captured = captured_for_task.lock().await;
                    append_stderr_bounded(&mut captured, &chunk);
                }
                Err(error) => {
                    warn!(
                        error = %error,
                        operation,
                        "failed to read Codex app-server stderr"
                    );
                    break;
                }
            }
        }
    });
    captured
}

async fn log_captured_stderr(captured: &AppServerStderr, operation: &str) {
    let stderr = captured.lock().await.trim().to_string();
    if stderr.is_empty() {
        return;
    }
    warn!(
        operation,
        stderr = %stderr,
        "Codex app-server stderr captured for diagnostics"
    );
}

fn append_stderr_bounded(output: &mut String, chunk: &str) {
    output.push_str(chunk);
    if output.len() <= APP_SERVER_STDERR_MAX_BYTES {
        return;
    }
    let overflow = output.len() - APP_SERVER_STDERR_MAX_BYTES;
    let trim_at = output
        .char_indices()
        .map(|(index, _)| index)
        .find(|index| *index >= overflow)
        .unwrap_or(overflow);
    output.drain(..trim_at);
}

async fn write_json_line(stdin: &mut tokio::process::ChildStdin, value: &Value) -> Result<()> {
    let mut encoded = serde_json::to_vec(value)?;
    encoded.push(b'\n');
    stdin.write_all(&encoded).await?;
    stdin.flush().await?;
    Ok(())
}

async fn wait_for_context_compaction(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    thread_id: &str,
) -> Result<()> {
    while let Some(line) = lines.next_line().await? {
        let value: Value = serde_json::from_str(&line)?;
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        let params = value.get("params").unwrap_or(&Value::Null);
        if !notification_matches_thread(params, thread_id) {
            continue;
        }
        match method {
            "item/completed" => {
                let item = params.get("item").unwrap_or(params);
                if item.get("type").and_then(Value::as_str) == Some("contextCompaction") {
                    let status = item
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("completed");
                    if matches!(status, "failed" | "interrupted" | "declined") {
                        return Err(CoreError::InvalidInput(format!(
                            "Codex context compaction {status}"
                        )));
                    }
                    return Ok(());
                }
            }
            "turn/completed" => {
                let turn = params.get("turn").unwrap_or(params);
                match turn.get("status").and_then(Value::as_str) {
                    Some("failed") | Some("interrupted") => {
                        return Err(CoreError::InvalidInput(format!(
                            "Codex context compaction {}{}",
                            turn.get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("failed"),
                            app_server_turn_error_suffix(turn)
                        )));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Err(CoreError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "Codex app-server closed before context compaction completed",
    )))
}

fn notification_matches_thread(params: &Value, thread_id: &str) -> bool {
    let Some(notification_thread_id) = params
        .get("threadId")
        .or_else(|| params.get("thread_id"))
        .and_then(Value::as_str)
    else {
        return true;
    };
    notification_thread_id == thread_id
}

fn notification_turn_id(params: &Value) -> Option<&str> {
    params
        .get("turnId")
        .or_else(|| params.get("turn_id"))
        .or_else(|| params.pointer("/turn/id"))
        .and_then(Value::as_str)
}

fn notification_is_thread_level(method: &str) -> bool {
    matches!(
        method,
        "thread/tokenUsage/updated" | "thread/status/changed" | "thread/settings/updated"
    )
}

fn notification_confirms_current_turn_activity(method: &str, params: &Value) -> bool {
    match method {
        "item/agentMessage/delta"
        | "item/reasoning/summaryTextDelta"
        | "item/reasoning/textDelta"
        | "item/commandExecution/outputDelta"
        | "turn/plan/updated"
        | "turn/diff/updated"
        | "turn/completed"
        | "error"
        | "warning"
        | "configWarning" => true,
        "item/completed" => {
            let item = params.get("item").unwrap_or(params);
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
            !matches!(
                item_type,
                "" | "userMessage" | "contextCompaction" | "enteredReviewMode"
            )
        }
        _ => false,
    }
}

fn notification_matches_turn(params: &Value, turn_id: Option<&str>) -> bool {
    let Some(expected_turn_id) = turn_id else {
        return true;
    };
    let Some(notification_turn_id) = notification_turn_id(params) else {
        return false;
    };
    notification_turn_id == expected_turn_id
}

fn app_server_turn_error_suffix(turn: &Value) -> String {
    let Some(error) = turn.get("error") else {
        return String::new();
    };
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    if message.trim().is_empty() {
        String::new()
    } else {
        format!(": {message}")
    }
}

async fn read_response(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    expected_id: i64,
) -> Result<Value> {
    while let Some(line) = lines.next_line().await? {
        let value: Value = serde_json::from_str(&line)?;
        if value.get("id").and_then(Value::as_i64) != Some(expected_id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            let code = error.get("code").and_then(Value::as_i64);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown app-server error");
            let detail = error.get("data").map(Value::to_string);
            return Err(CoreError::InvalidInput(match (code, detail) {
                (Some(code), Some(detail)) => {
                    format!("Codex app-server error {code}: {message} ({detail})")
                }
                (Some(code), None) => format!("Codex app-server error {code}: {message}"),
                (None, Some(detail)) => format!("Codex app-server error: {message} ({detail})"),
                (None, None) => format!("Codex app-server error: {message}"),
            }));
        }
        return value.get("result").cloned().ok_or_else(|| {
            CoreError::InvalidInput(
                "Codex app-server response did not contain a result".to_string(),
            )
        });
    }
    Err(CoreError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "Codex app-server closed before replying",
    )))
}

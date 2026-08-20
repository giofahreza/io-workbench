use std::{ffi::OsString, path::PathBuf, process::Stdio, sync::Arc, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{Mutex, mpsc, oneshot},
    time::{Instant, Sleep, timeout},
};
use tracing::warn;

use crate::{CoreError, Result, augmented_user_path};

const APP_SERVER_STDERR_MAX_BYTES: usize = 32 * 1024;

type AppServerStderr = Arc<Mutex<String>>;

#[derive(Debug, Clone)]
pub(crate) struct CodexThreadSnapshot {
    pub(crate) id: String,
    pub(crate) turns: Vec<CodexThreadTurn>,
}

impl CodexThreadSnapshot {
    pub(crate) fn latest_forkable_turn_id(&self) -> Option<&str> {
        self.turns
            .iter()
            .rev()
            .find(|turn| turn.status != "inProgress")
            .map(|turn| turn.id.as_str())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CodexThreadTurn {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) user_item_ids: Vec<String>,
    pub(crate) user_text: String,
}

#[derive(Clone)]
pub(crate) struct CodexAppServerClient {
    command: OsString,
    request_timeout: Duration,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CodexAppServerLaunchOptions {
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexAppServerLiveTurnParams {
    pub(crate) thread_id: Option<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) input: Vec<Value>,
    pub(crate) client_user_message_id: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) service_tier: Option<String>,
    pub(crate) approval_policy: Option<Value>,
    pub(crate) sandbox_policy: Option<Value>,
}

#[derive(Debug)]
pub(crate) enum CodexAppServerLiveTurnEvent {
    ThreadAssociated { thread_id: String },
    TurnAssociated { turn_id: String },
    Notification { method: String, params: Value },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum CodexAppServerTurnTerminalStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexAppServerLiveTurnOutcome {
    pub(crate) thread_id: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) status: CodexAppServerTurnTerminalStatus,
    pub(crate) turn: Option<Value>,
}

impl CodexAppServerClient {
    pub(crate) fn new(command: impl Into<OsString>, request_timeout: Duration) -> Self {
        Self {
            command: command.into(),
            request_timeout,
        }
    }

    pub(crate) async fn read_thread(&self, thread_id: &str) -> Result<CodexThreadSnapshot> {
        self.read_thread_with_options(thread_id, None).await
    }

    pub(crate) async fn read_thread_with_options(
        &self,
        thread_id: &str,
        launch_options: Option<&CodexAppServerLaunchOptions>,
    ) -> Result<CodexThreadSnapshot> {
        let result = self
            .request(
                "thread/read",
                json!({
                    "threadId": thread_id,
                    "includeTurns": true,
                }),
                launch_options,
            )
            .await?;
        parse_thread_snapshot(&result)
    }

    pub(crate) async fn fork_thread(&self, thread_id: &str, last_turn_id: &str) -> Result<String> {
        self.fork_thread_with_options(thread_id, last_turn_id, None)
            .await
    }

    pub(crate) async fn fork_thread_with_options(
        &self,
        thread_id: &str,
        last_turn_id: &str,
        launch_options: Option<&CodexAppServerLaunchOptions>,
    ) -> Result<String> {
        let result = self
            .request(
                "thread/fork",
                json!({
                    "threadId": thread_id,
                    "lastTurnId": last_turn_id,
                }),
                launch_options,
            )
            .await?;
        result
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                CoreError::InvalidInput(
                    "Codex app-server returned a fork without a thread id".to_string(),
                )
            })
    }

    pub(crate) async fn delete_thread(&self, thread_id: &str) -> Result<()> {
        self.request("thread/delete", json!({ "threadId": thread_id }), None)
            .await?;
        Ok(())
    }

    pub(crate) async fn compact_thread_and_wait_with_options(
        &self,
        thread_id: &str,
        launch_options: Option<&CodexAppServerLaunchOptions>,
    ) -> Result<()> {
        let thread_id = thread_id.trim().to_string();
        if thread_id.is_empty() {
            return Err(CoreError::InvalidInput(
                "Codex thread id is required for compaction".to_string(),
            ));
        }
        let command = self.command.clone();
        let launch_options = launch_options.cloned().unwrap_or_default();
        let compact_timeout = self.request_timeout.max(Duration::from_secs(120));
        let mut child_command = app_server_command(&command, &launch_options);
        let mut child = child_command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stderr = start_stderr_capture(&mut child, "compaction");
        let mut stdin = child.stdin.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdin was unavailable",
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdout was unavailable",
            ))
        })?;
        let mut lines = BufReader::new(stdout).lines();

        let result = timeout(compact_timeout, async {
            write_json_line(
                &mut stdin,
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
                            "optOutNotificationMethods": ["thread/started", "item/agentMessage/delta"],
                        },
                    },
                }),
            )
            .await?;
            let _ = read_response(&mut lines, 1).await?;
            write_json_line(&mut stdin, &json!({ "method": "initialized" })).await?;
            write_json_line(
                &mut stdin,
                &json!({
                    "method": "thread/resume",
                    "id": 2,
                    "params": { "threadId": &thread_id },
                }),
            )
            .await?;
            let _ = read_response(&mut lines, 2).await?;
            write_json_line(
                &mut stdin,
                &json!({
                    "method": "thread/compact/start",
                    "id": 3,
                    "params": { "threadId": &thread_id },
                }),
            )
            .await?;
            let _ = read_response(&mut lines, 3).await?;
            let result = wait_for_context_compaction(&mut lines, &thread_id).await;
            result
        })
        .await;
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "Codex app-server compaction timed out after {} ms",
                    compact_timeout.as_millis()
                ),
            ))),
        };
        if result.is_err() {
            log_captured_stderr(&stderr, "compaction").await;
        }
        let _ = stdin.shutdown().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
        result
    }

    pub(crate) async fn run_live_turn_with_options(
        &self,
        params: CodexAppServerLiveTurnParams,
        launch_options: Option<&CodexAppServerLaunchOptions>,
        abort_rx: oneshot::Receiver<()>,
        events: mpsc::Sender<CodexAppServerLiveTurnEvent>,
    ) -> Result<CodexAppServerLiveTurnOutcome> {
        let command = self.command.clone();
        let launch_options = launch_options.cloned().unwrap_or_default();
        let request_timeout = self.request_timeout;
        let mut child_command = app_server_command(&command, &launch_options);
        let mut child = child_command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stderr = start_stderr_capture(&mut child, "live turn");
        let mut stdin = child.stdin.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdin was unavailable",
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdout was unavailable",
            ))
        })?;
        let mut lines = BufReader::new(stdout).lines();

        let result = timeout(request_timeout, async {
            initialize_app_server(&mut stdin, &mut lines, &events).await?;
            let thread_id =
                start_or_resume_thread(&mut stdin, &mut lines, &params, &events).await?;
            let _ = events
                .send(CodexAppServerLiveTurnEvent::ThreadAssociated {
                    thread_id: thread_id.clone(),
                })
                .await;
            let turn_id = start_turn(&mut stdin, &mut lines, &params, &thread_id, &events).await?;
            if let Some(turn_id) = turn_id.as_deref() {
                let _ = events
                    .send(CodexAppServerLiveTurnEvent::TurnAssociated {
                        turn_id: turn_id.to_string(),
                    })
                    .await;
            }
            Ok::<_, CoreError>((thread_id, turn_id))
        })
        .await;

        let (thread_id, mut turn_id) = match result {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                log_captured_stderr(&stderr, "live turn setup").await;
                let _ = stdin.shutdown().await;
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(error);
            }
            Err(_) => {
                let error = CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "Codex app-server live turn setup timed out after {} ms",
                        request_timeout.as_millis()
                    ),
                ));
                log_captured_stderr(&stderr, "live turn setup").await;
                let _ = stdin.shutdown().await;
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(error);
            }
        };

        let mut abort_rx = abort_rx;
        let mut interrupt_request_id = 4_i64;
        let mut abort_deadline: Option<std::pin::Pin<Box<Sleep>>> = None;
        let mut abort_requested = false;
        let mut saw_current_turn_notification = false;
        let outcome = loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Some(line) = line? else {
                        break Err(CoreError::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "Codex app-server closed before the turn completed",
                        )));
                    };
                    if let Some(outcome) = handle_live_jsonrpc_line(
                        &line,
                        &mut stdin,
                        &thread_id,
                        &mut turn_id,
                        &mut saw_current_turn_notification,
                        &events,
                    ).await? {
                        break Ok(outcome);
                    }
                }
                _ = &mut abort_rx, if !abort_requested => {
                    abort_requested = true;
                    if let Some(active_turn_id) = turn_id.as_deref() {
                        write_json_line(
                            &mut stdin,
                            &json!({
                                "method": "turn/interrupt",
                                "id": interrupt_request_id,
                                "params": {
                                    "threadId": &thread_id,
                                    "turnId": active_turn_id,
                                },
                            }),
                        )
                        .await?;
                        interrupt_request_id += 1;
                        abort_deadline = Some(Box::pin(tokio::time::sleep_until(
                            Instant::now() + Duration::from_secs(2),
                        )));
                    } else {
                        break Ok(CodexAppServerLiveTurnOutcome {
                            thread_id: thread_id.clone(),
                            turn_id: None,
                            status: CodexAppServerTurnTerminalStatus::Interrupted,
                            turn: None,
                        });
                    }
                }
                _ = async {
                    if let Some(deadline) = abort_deadline.as_mut() {
                        deadline.as_mut().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if abort_deadline.is_some() => {
                    break Ok(CodexAppServerLiveTurnOutcome {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        status: CodexAppServerTurnTerminalStatus::Interrupted,
                        turn: None,
                    });
                }
            }
        };

        if outcome.is_err() {
            log_captured_stderr(&stderr, "live turn").await;
        }
        let _ = stdin.shutdown().await;
        let _ = child.start_kill();
        let _ = timeout(Duration::from_secs(1), child.wait()).await;
        outcome
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        launch_options: Option<&CodexAppServerLaunchOptions>,
    ) -> Result<Value> {
        let method = method.to_string();
        let command = self.command.clone();
        let launch_options = launch_options.cloned().unwrap_or_default();
        let mut child_command = app_server_command(&command, &launch_options);
        let mut child = child_command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stderr = start_stderr_capture(&mut child, "request");
        let mut stdin = child.stdin.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdin was unavailable",
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdout was unavailable",
            ))
        })?;
        let mut lines = BufReader::new(stdout).lines();

        let result = timeout(self.request_timeout, async {
            write_json_line(
                &mut stdin,
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
                            "optOutNotificationMethods": ["thread/started"],
                        },
                    },
                }),
            )
            .await?;
            let _ = read_response(&mut lines, 1).await?;
            write_json_line(&mut stdin, &json!({ "method": "initialized" })).await?;
            write_json_line(
                &mut stdin,
                &json!({
                    "method": method,
                    "id": 2,
                    "params": params,
                }),
            )
            .await?;
            let result = read_response(&mut lines, 2).await;
            result
        })
        .await;
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "Codex app-server request timed out after {} ms",
                    self.request_timeout.as_millis()
                ),
            ))),
        };
        if result.is_err() {
            log_captured_stderr(&stderr, "request").await;
        }
        let _ = stdin.shutdown().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
        result
    }
}

async fn initialize_app_server(
    stdin: &mut tokio::process::ChildStdin,
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
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
    let _ = read_response_with_events(lines, stdin, 1, events).await?;
    write_json_line(stdin, &json!({ "method": "initialized", "params": {} })).await
}

async fn start_or_resume_thread(
    stdin: &mut tokio::process::ChildStdin,
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    params: &CodexAppServerLiveTurnParams,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
) -> Result<String> {
    let existing_thread_id = params
        .thread_id
        .as_deref()
        .map(str::trim)
        .filter(|thread_id| !thread_id.is_empty());
    let mut request_params = json!({
        "cwd": params.cwd.display().to_string(),
        "approvalPolicy": params.approval_policy.clone().unwrap_or_else(|| json!("never")),
    });
    insert_optional_string(&mut request_params, "model", params.model.as_deref());
    insert_optional_string(&mut request_params, "effort", params.effort.as_deref());
    insert_optional_string(
        &mut request_params,
        "serviceTier",
        params.service_tier.as_deref(),
    );
    if let Some(thread_id) = existing_thread_id {
        request_params["threadId"] = Value::String(thread_id.to_string());
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
    let result = read_response_with_events(lines, stdin, 2, events).await?;
    extract_thread_id(&result)
        .or_else(|| existing_thread_id.map(str::to_string))
        .ok_or_else(|| {
            CoreError::InvalidInput("Codex app-server did not return a thread id".to_string())
        })
}

async fn start_turn(
    stdin: &mut tokio::process::ChildStdin,
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    params: &CodexAppServerLiveTurnParams,
    thread_id: &str,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
) -> Result<Option<String>> {
    let mut request_params = json!({
        "threadId": thread_id,
        "input": params.input,
        "cwd": params.cwd.display().to_string(),
        "approvalPolicy": params.approval_policy.clone().unwrap_or_else(|| json!("never")),
    });
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
    let result = read_response_with_events(lines, stdin, 3, events).await?;
    Ok(result
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .map(str::to_string))
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
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    stdin: &mut tokio::process::ChildStdin,
    expected_id: i64,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
) -> Result<Value> {
    while let Some(line) = lines.next_line().await? {
        let value: Value = serde_json::from_str(&line)?;
        if value.get("id").and_then(Value::as_i64) == Some(expected_id) {
            if let Some(error) = value.get("error") {
                return Err(app_server_response_error(error));
            }
            return value.get("result").cloned().ok_or_else(|| {
                CoreError::InvalidInput(
                    "Codex app-server response did not contain a result".to_string(),
                )
            });
        }
        handle_nonmatching_jsonrpc_message(&value, stdin, events).await?;
    }
    Err(CoreError::Io(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "Codex app-server closed before replying",
    )))
}

async fn handle_nonmatching_jsonrpc_message(
    value: &Value,
    stdin: &mut tokio::process::ChildStdin,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
) -> Result<()> {
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return Ok(());
    };
    if let Some(id) = value.get("id").cloned() {
        respond_to_server_request(stdin, id, method).await?;
        return Ok(());
    }
    let params = value.get("params").cloned().unwrap_or(Value::Null);
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
    stdin: &mut tokio::process::ChildStdin,
    expected_thread_id: &str,
    turn_id: &mut Option<String>,
    saw_current_turn_notification: &mut bool,
    events: &mpsc::Sender<CodexAppServerLiveTurnEvent>,
) -> Result<Option<CodexAppServerLiveTurnOutcome>> {
    let value: Value = serde_json::from_str(line)?;
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return Ok(None);
    };
    if let Some(id) = value.get("id").cloned() {
        respond_to_server_request(stdin, id, method).await?;
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
        *saw_current_turn_notification && (matches_turn || !has_notification_turn_id)
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
    stdin: &mut tokio::process::ChildStdin,
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

fn parse_thread_snapshot(result: &Value) -> Result<CodexThreadSnapshot> {
    let thread = result.get("thread").ok_or_else(|| {
        CoreError::InvalidInput("Codex thread/read response omitted thread".to_string())
    })?;
    let id = thread
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            CoreError::InvalidInput("Codex thread/read response omitted thread id".to_string())
        })?;
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_thread_turn)
        .collect();
    Ok(CodexThreadSnapshot { id, turns })
}

fn parse_thread_turn(value: &Value) -> Option<CodexThreadTurn> {
    let id = value.get("id")?.as_str()?.trim();
    if id.is_empty() {
        return None;
    }
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed")
        .to_string();
    let mut user_item_ids = Vec::new();
    let mut user_texts = Vec::new();
    for item in value
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("userMessage"))
    {
        if let Some(item_id) = item.get("id").and_then(Value::as_str) {
            user_item_ids.push(item_id.to_string());
        }
        let text = item
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|content| content.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|content| content.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            user_texts.push(text);
        }
    }
    Some(CodexThreadTurn {
        id: id.to_string(),
        status,
        user_item_ids,
        user_text: user_texts.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_thread_turn_ids_and_user_items() {
        let snapshot = parse_thread_snapshot(&json!({
            "thread": {
                "id": "thread-1",
                "turns": [
                    {
                        "id": "turn-1",
                        "status": "completed",
                        "items": [
                            {
                                "type": "userMessage",
                                "id": "item-1",
                                "content": [{"type": "text", "text": "first prompt"}]
                            }
                        ]
                    },
                    {
                        "id": "turn-2",
                        "status": "inProgress",
                        "items": []
                    }
                ]
            }
        }))
        .expect("snapshot");

        assert_eq!(snapshot.id, "thread-1");
        assert_eq!(snapshot.turns[0].user_item_ids, ["item-1"]);
        assert_eq!(snapshot.turns[0].user_text, "first prompt");
        assert_eq!(snapshot.latest_forkable_turn_id(), Some("turn-1"));
    }

    #[test]
    fn latest_forkable_turn_keeps_failed_or_interrupted_boundaries() {
        let snapshot = parse_thread_snapshot(&json!({
            "thread": {
                "id": "thread-1",
                "turns": [
                    {"id": "turn-failed", "status": "failed", "items": []},
                    {"id": "turn-interrupted", "status": "interrupted", "items": []},
                    {"id": "turn-running", "status": "inProgress", "items": []}
                ]
            }
        }))
        .expect("snapshot");

        assert_eq!(snapshot.latest_forkable_turn_id(), Some("turn-interrupted"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn performs_initialize_and_read_handshake() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("fake-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nread first\nprintf '%s\\n' \"$first\" >> '{}'\nprintf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\",\"codexHome\":\"/tmp\",\"platformFamily\":\"unix\",\"platformOs\":\"linux\"}}}}'\nread second\nprintf '%s\\n' \"$second\" >> '{}'\nread third\nprintf '%s\\n' \"$third\" >> '{}'\nprintf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-1\",\"turns\":[]}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let snapshot = client.read_thread("thread-1").await.expect("read thread");
        assert_eq!(snapshot.id, "thread-1");
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"method\":\"initialize\""));
        assert!(requests.contains("\"method\":\"initialized\""));
        assert!(requests.contains("\"method\":\"thread/read\""));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn waits_for_context_compaction_completion_notification() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("compact-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-1\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/started\",\"params\":{{\"threadId\":\"thread-1\",\"item\":{{\"type\":\"contextCompaction\",\"id\":\"item-compact\"}}}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/completed\",\"params\":{{\"threadId\":\"thread-1\",\"item\":{{\"type\":\"contextCompaction\",\"id\":\"item-compact\"}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        client
            .compact_thread_and_wait_with_options("thread-1", None)
            .await
            .expect("compact thread");
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"method\":\"initialize\""));
        assert!(requests.contains("\"method\":\"thread/resume\""));
        assert!(requests.contains("\"method\":\"thread/compact/start\""));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reports_app_server_errors_and_timeouts() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let error_script = root.join("error-codex.sh");
        std::fs::write(
            &error_script,
            "#!/bin/sh\nread first\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nprintf '%s\\n' 'diagnostic secret' >&2\nread second\nread third\nprintf '%s\\n' '{\"id\":2,\"error\":{\"code\":-32600,\"message\":\"bad boundary\"}}'\n",
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&error_script)
            .expect("metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&error_script, permissions).expect("permissions");
        let error_client =
            CodexAppServerClient::new(error_script.as_os_str(), Duration::from_secs(2));
        let error = error_client
            .fork_thread("thread-1", "turn-1")
            .await
            .expect_err("fork should fail");
        assert!(error.to_string().contains("bad boundary"));
        assert!(!error.to_string().contains("diagnostic secret"));

        let timeout_script = root.join("timeout-codex.sh");
        std::fs::write(&timeout_script, "#!/bin/sh\nsleep 2\n").expect("script");
        let mut permissions = std::fs::metadata(&timeout_script)
            .expect("metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&timeout_script, permissions).expect("permissions");
        let timeout_client =
            CodexAppServerClient::new(timeout_script.as_os_str(), Duration::from_millis(30));
        let error = timeout_client
            .read_thread("thread-1")
            .await
            .expect_err("read should time out");
        assert!(error.to_string().contains("timed out"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_live_turn_start_sequence() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("live-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-live\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-live\",\"status\":\"inProgress\",\"items\":[],\"error\":null}}}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/agentMessage/delta\",\"params\":{{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\",\"itemId\":\"msg-1\",\"delta\":\"hello\"}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"turn/completed\",\"params\":{{\"threadId\":\"thread-live\",\"turn\":{{\"id\":\"turn-live\",\"status\":\"completed\",\"items\":[{{\"type\":\"agentMessage\",\"id\":\"msg-1\",\"text\":\"hello\",\"phase\":\"final_answer\"}}],\"error\":null}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let (_abort_tx, abort_rx) = oneshot::channel();
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let outcome = client
            .run_live_turn_with_options(live_turn_params(&root, None), None, abort_rx, event_tx)
            .await
            .expect("live turn");

        assert_eq!(outcome.thread_id, "thread-live");
        assert_eq!(outcome.turn_id.as_deref(), Some("turn-live"));
        assert_eq!(outcome.status, CodexAppServerTurnTerminalStatus::Completed);
        let mut saw_delta = false;
        while let Some(event) = event_rx.recv().await {
            if let CodexAppServerLiveTurnEvent::Notification { method, params } = event
                && method == "item/agentMessage/delta"
                && params.get("delta").and_then(Value::as_str) == Some("hello")
            {
                saw_delta = true;
            }
        }
        assert!(saw_delta);
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"method\":\"initialize\""));
        assert!(requests.contains("\"method\":\"initialized\""));
        assert!(requests.contains("\"method\":\"thread/start\""));
        assert!(requests.contains("\"method\":\"turn/start\""));
        assert!(requests.contains("\"input\":[{\"text\":\"hello\",\"type\":\"text\"}]"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_live_turn_resume_sequence() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("resume-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-existing\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-resume\",\"status\":\"inProgress\",\"items\":[]}}}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"turn/completed\",\"params\":{{\"threadId\":\"thread-existing\",\"turn\":{{\"id\":\"turn-resume\",\"status\":\"completed\",\"items\":[{{\"type\":\"agentMessage\",\"id\":\"msg-1\",\"text\":\"resumed\",\"phase\":\"final_answer\"}}]}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let (_abort_tx, abort_rx) = oneshot::channel();
        let (event_tx, _event_rx) = mpsc::channel(16);
        let outcome = client
            .run_live_turn_with_options(
                live_turn_params(&root, Some("thread-existing")),
                None,
                abort_rx,
                event_tx,
            )
            .await
            .expect("live turn");

        assert_eq!(outcome.thread_id, "thread-existing");
        assert_eq!(outcome.turn_id.as_deref(), Some("turn-resume"));
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"method\":\"thread/resume\""));
        assert!(!requests.contains("\"method\":\"thread/start\""));
        assert!(requests.contains("\"threadId\":\"thread-existing\""));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_turn_filters_notifications_from_other_turns() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("demux-codex.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\n\
             read first\nprintf '%s\\n' '{\"id\":1,\"result\":{\"userAgent\":\"test\"}}'\n\
             read second\n\
             read third\nprintf '%s\\n' '{\"id\":2,\"result\":{\"thread\":{\"id\":\"thread-live\"}}}'\n\
             read fourth\nprintf '%s\\n' '{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn-live\",\"status\":\"inProgress\",\"items\":[]}}}'\n\
             printf '%s\\n' '{\"method\":\"item/agentMessage/delta\",\"params\":{\"threadId\":\"thread-live\",\"turnId\":\"turn-other\",\"itemId\":\"msg-other\",\"delta\":\"wrong\"}}'\n\
             printf '%s\\n' '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thread-live\",\"turn\":{\"id\":\"turn-other\",\"status\":\"completed\",\"items\":[{\"type\":\"agentMessage\",\"id\":\"msg-other\",\"text\":\"wrong\",\"phase\":\"final_answer\"}]}}}'\n\
             printf '%s\\n' '{\"method\":\"turn/started\",\"params\":{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\"}}'\n\
             printf '%s\\n' '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\",\"item\":{\"type\":\"userMessage\",\"id\":\"user-live\",\"text\":\"hello\"}}}'\n\
             printf '%s\\n' '{\"method\":\"thread/tokenUsage/updated\",\"params\":{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\",\"tokenUsage\":{\"outputTokens\":111}}}'\n\
             printf '%s\\n' '{\"method\":\"item/agentMessage/delta\",\"params\":{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\",\"itemId\":\"msg-live\",\"delta\":\"right\"}}'\n\
             printf '%s\\n' '{\"method\":\"thread/tokenUsage/updated\",\"params\":{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\",\"tokenUsage\":{\"outputTokens\":222}}}'\n\
             printf '%s\\n' '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thread-live\",\"turn\":{\"id\":\"turn-live\",\"status\":\"completed\",\"items\":[{\"type\":\"agentMessage\",\"id\":\"msg-live\",\"text\":\"right\",\"phase\":\"final_answer\"}]}}}'\n",
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let (_abort_tx, abort_rx) = oneshot::channel();
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let outcome = client
            .run_live_turn_with_options(live_turn_params(&root, None), None, abort_rx, event_tx)
            .await
            .expect("live turn");

        assert_eq!(outcome.turn_id.as_deref(), Some("turn-live"));
        assert_eq!(outcome.status, CodexAppServerTurnTerminalStatus::Completed);
        let mut deltas = Vec::new();
        let mut output_tokens = Vec::new();
        while let Some(event) = event_rx.recv().await {
            if let CodexAppServerLiveTurnEvent::Notification { method, params } = event {
                if method == "item/agentMessage/delta"
                    && let Some(delta) = params.get("delta").and_then(Value::as_str)
                {
                    deltas.push(delta.to_string());
                }
                if method == "thread/tokenUsage/updated"
                    && let Some(tokens) = params
                        .pointer("/tokenUsage/outputTokens")
                        .and_then(Value::as_i64)
                {
                    output_tokens.push(tokens);
                }
            }
        }
        assert_eq!(deltas, ["right"]);
        assert_eq!(output_tokens, [222]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_turn_declines_server_requests_without_hanging() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("approval-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\n\
                 read second\n\
                 read third\nprintf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-live\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-live\",\"status\":\"inProgress\",\"items\":[]}}}}}}'\n\
                 printf '%s\\n' '{{\"id\":99,\"method\":\"item/commandExecution/requestApproval\",\"params\":{{\"threadId\":\"thread-live\",\"turnId\":\"turn-live\",\"itemId\":\"cmd-1\"}}}}'\n\
                 read approval\nprintf '%s\\n' \"$approval\" >> '{}'\n\
                 printf '%s\\n' '{{\"method\":\"turn/completed\",\"params\":{{\"threadId\":\"thread-live\",\"turn\":{{\"id\":\"turn-live\",\"status\":\"completed\",\"items\":[{{\"type\":\"agentMessage\",\"id\":\"msg-1\",\"text\":\"done\",\"phase\":\"final_answer\"}}]}}}}}}'\n",
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let (_abort_tx, abort_rx) = oneshot::channel();
        let (event_tx, _event_rx) = mpsc::channel(16);
        client
            .run_live_turn_with_options(live_turn_params(&root, None), None, abort_rx, event_tx)
            .await
            .expect("live turn");

        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"id\":99"));
        assert!(requests.contains("\"decision\":\"decline\""));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_turn_rejects_chatgpt_token_refresh_without_hanging() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("auth-refresh-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\n\
                 read second\n\
                 read third\nprintf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-live\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-live\",\"status\":\"inProgress\",\"items\":[]}}}}}}'\n\
                 printf '%s\\n' '{{\"id\":99,\"method\":\"account/chatgptAuthTokens/refresh\",\"params\":{{\"reason\":\"expired\",\"previousAccountId\":\"acct-1\"}}}}'\n\
                 read refresh\nprintf '%s\\n' \"$refresh\" >> '{}'\n\
                 printf '%s\\n' '{{\"method\":\"turn/completed\",\"params\":{{\"threadId\":\"thread-live\",\"turn\":{{\"id\":\"turn-live\",\"status\":\"completed\",\"items\":[{{\"type\":\"agentMessage\",\"id\":\"msg-1\",\"text\":\"done\",\"phase\":\"final_answer\"}}]}}}}}}'\n",
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let (_abort_tx, abort_rx) = oneshot::channel();
        let (event_tx, _event_rx) = mpsc::channel(16);
        client
            .run_live_turn_with_options(live_turn_params(&root, None), None, abort_rx, event_tx)
            .await
            .expect("live turn");

        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"id\":99"));
        assert!(requests.contains("\"code\":-32000"));
        assert!(requests.contains("does not provide external ChatGPT auth tokens"));
        assert!(!requests.contains("\"code\":-32601"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_turn_abort_sends_turn_interrupt() {
        use std::os::unix::fs::PermissionsExt;
        use tokio::time::sleep;

        let root = std::env::temp_dir().join(format!("iowb-app-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp dir");
        let script = root.join("abort-codex.sh");
        let log = root.join("requests.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\n\
                 read second\n\
                 read third\nprintf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-live\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-live\",\"status\":\"inProgress\",\"items\":[]}}}}}}'\n\
                 read interrupt\nprintf '%s\\n' \"$interrupt\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":4,\"result\":{{}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"turn/completed\",\"params\":{{\"threadId\":\"thread-live\",\"turn\":{{\"id\":\"turn-live\",\"status\":\"interrupted\",\"items\":[]}}}}}}'\n",
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let client = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        let (abort_tx, abort_rx) = oneshot::channel();
        let (event_tx, _event_rx) = mpsc::channel(16);
        let task_root = root.clone();
        let task = tokio::spawn(async move {
            client
                .run_live_turn_with_options(
                    live_turn_params(&task_root, None),
                    None,
                    abort_rx,
                    event_tx,
                )
                .await
        });
        sleep(Duration::from_millis(50)).await;
        abort_tx.send(()).expect("send abort");
        let outcome = task.await.expect("join").expect("live turn");
        assert_eq!(
            outcome.status,
            CodexAppServerTurnTerminalStatus::Interrupted
        );
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"method\":\"turn/interrupt\""));
        assert!(requests.contains("\"turnId\":\"turn-live\""));
        let _ = std::fs::remove_dir_all(root);
    }

    fn live_turn_params(
        root: &std::path::Path,
        thread_id: Option<&str>,
    ) -> CodexAppServerLiveTurnParams {
        CodexAppServerLiveTurnParams {
            thread_id: thread_id.map(str::to_string),
            cwd: root.to_path_buf(),
            input: vec![json!({ "type": "text", "text": "hello" })],
            client_user_message_id: None,
            model: None,
            effort: None,
            service_tier: None,
            approval_policy: Some(json!("never")),
            sandbox_policy: None,
        }
    }

    #[test]
    fn bounded_stderr_keeps_recent_diagnostics_on_utf8_boundary() {
        let mut output = String::new();
        append_stderr_bounded(&mut output, &"a".repeat(APP_SERVER_STDERR_MAX_BYTES + 10));
        append_stderr_bounded(&mut output, "final");

        assert!(output.len() <= APP_SERVER_STDERR_MAX_BYTES);
        assert!(output.ends_with("final"));
    }
}

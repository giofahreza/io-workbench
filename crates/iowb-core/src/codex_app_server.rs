use std::{ffi::OsString, process::Stdio, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::timeout,
};

use crate::{CoreError, Result, augmented_user_path};

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
        timeout(compact_timeout, async move {
            let mut child_command = app_server_command(&command, &launch_options);
            let mut child = child_command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()?;
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
            let _ = stdin.shutdown().await;
            let _ = child.start_kill();
            let _ = child.wait().await;
            result
        })
        .await
        .map_err(|_| {
            CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "Codex app-server compaction timed out after {} ms",
                    compact_timeout.as_millis()
                ),
            ))
        })?
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
        timeout(self.request_timeout, async move {
            let mut child_command = app_server_command(&command, &launch_options);
            let mut child = child_command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()?;
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
            let _ = stdin.shutdown().await;
            let _ = child.start_kill();
            let _ = child.wait().await;
            result
        })
        .await
        .map_err(|_| {
            CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "Codex app-server request timed out after {} ms",
                    self.request_timeout.as_millis()
                ),
            ))
        })?
    }
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
            "#!/bin/sh\nread first\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread second\nread third\nprintf '%s\\n' '{\"id\":2,\"error\":{\"code\":-32600,\"message\":\"bad boundary\"}}'\n",
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
}

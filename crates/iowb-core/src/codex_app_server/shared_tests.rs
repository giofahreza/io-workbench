#[cfg(unix)]
struct SharedSocketFixture {
    root: PathBuf,
    options: CodexAppServerLaunchOptions,
    requests: Arc<Mutex<Vec<Value>>>,
    server: tokio::task::JoinHandle<()>,
}

#[cfg(unix)]
impl SharedSocketFixture {
    async fn start(resume_status: &str, recovered_turn: Option<Value>, queue_busy: bool) -> Self {
        let root = std::env::temp_dir().join(format!("iowb-shared-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let socket = root.join("codex.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let recorded = requests.clone();
        let home = root.clone();
        let status = resume_status.to_string();
        let server = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let mut websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
                let mut queued = false;
                let mut queue_starts = 0;
                while let Some(Ok(message)) = websocket.next().await {
                    let tokio_tungstenite::tungstenite::Message::Text(message) = message else {
                        continue;
                    };
                    let request: Value = serde_json::from_str(&message).unwrap();
                    recorded.lock().await.push(request.clone());
                    let Some(method) = request.get("method").and_then(Value::as_str) else {
                        continue;
                    };
                    let request_id = request.get("id").cloned().unwrap_or(Value::Null);
                    let result = match method {
                        "initialize" => json!({"codexHome": home, "userAgent":"fixture"}),
                        "initialized" => continue,
                        "thread/resume" | "thread/start" => {
                            json!({"thread":{"id":"native-thread", "status":{"type":status}, "turns":[]}, "modelProvider":"native-provider"})
                        }
                        "thread/settings/update" | "thread/queue/delete" => json!({}),
                        "thread/turns/list" => {
                            json!({"data":recovered_turn.clone().into_iter().collect::<Vec<_>>(), "nextCursor":null})
                        }
                        "thread/queue/list" => {
                            json!({"data":if queued { vec![json!({"id":"queue-1", "clientUserMessageId":"request-user-message"})] } else { vec![] }, "nextCursor":null})
                        }
                        "thread/queue/add" => {
                            queued = true;
                            json!({"queuedSubmission":{"id":"queue-1", "clientUserMessageId":request["params"]["clientUserMessageId"]}})
                        }
                        "thread/queue/start" => {
                            queue_starts += 1;
                            if queue_busy && queue_starts == 1 {
                                let message = if recovered_turn.is_some() {
                                    queued = false;
                                    "queued submission not found: queue-1"
                                } else {
                                    "thread already has an active or pending turn"
                                };
                                websocket.send(tokio_tungstenite::tungstenite::Message::Text(
                                    json!({"id":request_id,"error":{"code":-32600,"message":message}}).to_string().into()
                                )).await.unwrap();
                                continue;
                            }
                            queued = false;
                            websocket
                                .send(tokio_tungstenite::tungstenite::Message::Ping(
                                    vec![1, 2, 3].into(),
                                ))
                                .await
                                .unwrap();
                            let unrelated = json!({"method":"turn/completed","params":{"threadId":"other-thread","turn":{"id":"other-turn","status":"completed","items":[]}}});
                            websocket
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    unrelated.to_string().into(),
                                ))
                                .await
                                .unwrap();
                            let approval = json!({"id":"approval-other","method":"item/commandExecution/requestApproval","params":{"threadId":"other-thread","turnId":"other-turn"}});
                            websocket
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    approval.to_string().into(),
                                ))
                                .await
                                .unwrap();
                            let completion = json!({"method":"turn/completed","params":{"threadId":"native-thread","turn":{"id":"native-turn","status":"completed","items":[{"id":"answer-1","type":"agentMessage","text":"continued","phase":"final_answer"}]}}});
                            websocket
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    completion.to_string().into(),
                                ))
                                .await
                                .unwrap();
                            json!({"turn":{"id":"native-turn","status":"inProgress","items":[]}})
                        }
                        _ => panic!("unexpected shared method: {method}"),
                    };
                    if websocket
                        .send(tokio_tungstenite::tungstenite::Message::Text(
                            json!({"id":request_id,"result":result}).to_string().into(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        Self {
            root: root.clone(),
            options: CodexAppServerLaunchOptions {
                transport: CodexAppServerTransport::UnixSocket(socket),
                codex_home: Some(root),
                ..Default::default()
            },
            requests,
            server,
        }
    }

    async fn run(
        &self,
        recovery: Option<CodexTurnRecovery>,
    ) -> Result<CodexAppServerLiveTurnOutcome> {
        let mut params = live_turn_params(&self.root, Some("native-thread"));
        params.client_user_message_id = Some("request-user-message".into());
        params.model_provider = Some("must-not-override-native".into());
        params.approval_policy = None;
        params.recovery = recovery;
        let (_abort, abort_rx) = oneshot::channel();
        let (events, mut receiver) = mpsc::channel(256);
        let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
        let result = timeout(
            Duration::from_secs(4),
            CodexAppServerClient::new("unused-command", Duration::from_secs(2))
                .run_live_turn_with_options(params, Some(&self.options), abort_rx, events),
        )
        .await
        .expect("bounded turn");
        drain.await.unwrap();
        result
    }
}

#[cfg(unix)]
impl Drop for SharedSocketFixture {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn shared_runtime_uses_atomic_queue_and_keeps_native_thread_settings() {
    let fixture = SharedSocketFixture::start("idle", None, false).await;
    let outcome = fixture.run(None).await.unwrap();
    assert_eq!(outcome.thread_id, "native-thread");
    assert_eq!(outcome.turn_id.as_deref(), Some("native-turn"));
    assert_eq!(outcome.status, CodexAppServerTurnTerminalStatus::Completed);
    assert!(!fixture.server.is_finished());
    let requests = fixture.requests.lock().await;
    let resume = requests
        .iter()
        .find(|request| request["method"] == "thread/resume")
        .unwrap();
    assert_eq!(
        resume["params"],
        json!({"threadId":"native-thread","excludeTurns":true})
    );
    assert!(
        !requests
            .iter()
            .any(|request| request["method"] == "turn/start")
    );
    assert!(
        !requests
            .iter()
            .any(|request| request["method"] == "thread/settings/update")
    );
    assert!(
        !requests
            .iter()
            .any(|request| request["id"] == "approval-other")
    );
    assert!(
        requests
            .iter()
            .any(|request| request["method"] == "thread/queue/start"
                && request["params"]["queuedSubmissionId"] == "queue-1")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn shared_runtime_rejects_active_cli_turn_without_queueing_or_interrupting() {
    let fixture = SharedSocketFixture::start("active", None, false).await;
    let error = fixture.run(None).await.unwrap_err();
    assert!(error.to_string().contains("CLI can stay open"));
    let requests = fixture.requests.lock().await;
    assert!(!requests.iter().any(|request| matches!(
        request["method"].as_str(),
        Some("turn/start" | "turn/interrupt" | "thread/queue/add" | "thread/settings/update")
    )));
}

#[cfg(unix)]
#[tokio::test]
async fn shared_runtime_waits_for_its_queued_message_after_busy_race_without_resubmitting() {
    let fixture = SharedSocketFixture::start("idle", None, true).await;
    let outcome = fixture.run(None).await.unwrap();
    assert_eq!(outcome.turn_id.as_deref(), Some("native-turn"));
    let requests = fixture.requests.lock().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["method"] == "thread/queue/add")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["method"] == "thread/queue/start")
            .count(),
        2
    );
    assert!(!requests.iter().any(|request| matches!(
        request["method"].as_str(),
        Some("turn/start" | "turn/interrupt" | "thread/queue/delete")
    )));
}

#[cfg(unix)]
#[tokio::test]
async fn shared_runtime_reconciles_a_queued_message_already_started_by_the_cli() {
    let turn = json!({"id":"cli-claimed-turn", "status":"completed", "items":[
        {"id":"native-user", "clientId":"request-user-message", "type":"userMessage", "content":[]},
        {"id":"native-answer", "type":"agentMessage", "text":"already accepted", "phase":"final_answer"}
    ]});
    let fixture = SharedSocketFixture::start("idle", Some(turn), true).await;
    let outcome = fixture.run(None).await.unwrap();
    assert_eq!(outcome.turn_id.as_deref(), Some("cli-claimed-turn"));
    assert_eq!(outcome.status, CodexAppServerTurnTerminalStatus::Completed);
    let requests = fixture.requests.lock().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["method"] == "thread/queue/add")
            .count(),
        1
    );
    assert!(
        !requests
            .iter()
            .any(|request| request["method"] == "thread/queue/delete")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn shared_runtime_reconciles_finished_native_turn_without_replaying_prompt() {
    let turn = json!({"id":"already-finished","status":"completed","items":[
        {"id":"native-user","clientId":"request-user-message","type":"userMessage","content":[]},
        {"id":"native-answer","type":"agentMessage","text":"already done","phase":"final_answer"}
    ]});
    let fixture = SharedSocketFixture::start("idle", Some(turn), false).await;
    let outcome = fixture
        .run(Some(CodexTurnRecovery {
            submission_started: true,
            turn_id: None,
        }))
        .await
        .unwrap();
    assert_eq!(outcome.turn_id.as_deref(), Some("already-finished"));
    let requests = fixture.requests.lock().await;
    assert!(!requests.iter().any(|request| matches!(
        request["method"].as_str(),
        Some("turn/start" | "thread/queue/add" | "thread/queue/start")
    )));
}

#[cfg(unix)]
#[tokio::test]
async fn shared_runtime_never_resubmits_an_uncertain_submission() {
    let fixture = SharedSocketFixture::start("idle", None, false).await;
    let error = fixture
        .run(Some(CodexTurnRecovery {
            submission_started: true,
            turn_id: None,
        }))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("outcome is uncertain"));
    assert!(
        !fixture
            .requests
            .lock()
            .await
            .iter()
            .any(|request| request["method"] == "thread/queue/add")
    );
}

#[test]
fn codex_active_writer_is_not_confused_with_other_invalid_requests() {
    assert!(matches!(
        app_server_response_error(
            &json!({"code":-32600,"message":"thread native already has an active writer"})
        ),
        CoreError::Conflict(_)
    ));
    assert!(matches!(
        app_server_response_error(&json!({"code":-32600,"message":"invalid model provider"})),
        CoreError::InvalidInput(_)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn shared_runtime_refuses_per_user_gateway_credentials() {
    let client = CodexAppServerClient::new("must-not-run", Duration::from_secs(1));
    let options = CodexAppServerLaunchOptions {
        transport: CodexAppServerTransport::NativeDaemon,
        env: vec![("IOWB_IO_GATEWAY_API_KEY".into(), "private-key".into())],
        ..Default::default()
    };
    let error = match client.connect(&options, "credential isolation").await {
        Err(error) => error,
        Ok(_) => panic!("credentials must be rejected"),
    };
    assert!(error.to_string().contains("per-run credentials"));
    assert!(!error.to_string().contains("private-key"));
}

#[cfg(unix)]
#[test]
fn shared_runtime_validates_native_home_before_resuming_any_thread() {
    let home = std::env::temp_dir();
    assert!(validate_shared_codex_home(&json!({"codexHome":home}), Some(&home)).is_ok());
    assert!(
        validate_shared_codex_home(&json!({"codexHome":"/nonexistent-codex-home"}), Some(&home))
            .is_err()
    );
}

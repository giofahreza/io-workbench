use super::*;

struct Fixture(Shared, HeaderMap, PathBuf);

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("iowb-claude-broker-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let manifest = Manifest {
            protocol: 1,
            session_id: uuid::Uuid::new_v4().to_string(),
            cwd: root.clone(),
            url: "http://127.0.0.1:1234".into(),
            token: "test-token".into(),
            instance: "test-instance".into(),
            pid: std::process::id(),
        };
        let broker = Broker {
            manifest,
            root: root.clone(),
            requests: HashMap::new(),
            connected: true,
            busy: false,
            stopped: false,
            last_seen: Instant::now(),
            pending: None,
            event_ack: 0,
        };
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-token".parse().unwrap());
        Self(Arc::new(Mutex::new(broker)), headers, root)
    }

    async fn submit(&self, id: &str, prompt: &str) -> Reply {
        submit(
            State(self.0.clone()),
            self.1.clone(),
            Json(Submission {
                id: id.into(),
                prompt: prompt.into(),
                ..Default::default()
            }),
        )
        .await
    }

    async fn exchange(&self, events: Value) -> Value {
        let manifest = self.0.lock().await.manifest.clone();
        exchange(State(self.0.clone()), self.1.clone(), Json(serde_json::json!({"sessionId":manifest.session_id,"instance":manifest.instance,"connected":true,"busy":false,"stopped":false,"events":events}))).await.unwrap().0
    }

    async fn abort(&self, id: &str) {
        let _ = abort(
            State(self.0.clone()),
            self.1.clone(),
            Json(serde_json::json!({"id":id})),
        )
        .await
        .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.2);
    }
}

#[tokio::test]
async fn submissions_are_deduplicated_before_and_after_dispatch() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.submit("r1", "hello").await.unwrap().0["status"],
        "queued"
    );
    assert_eq!(
        fixture.submit("r1", "hello").await.unwrap().0["status"],
        "queued"
    );
    assert!(fixture.submit("r1", "different").await.is_err());
    assert!(fixture.submit("r2", "new message").await.is_err());
    let first = fixture.exchange(serde_json::json!([])).await;
    let retry = fixture.exchange(serde_json::json!([])).await;
    assert_eq!(
        first["command"], retry["command"],
        "a lost command response must remain retrievable"
    );
    fixture
        .exchange(serde_json::json!([{"sequence":1,"kind":"dispatch","requestId":"r1"}]))
        .await;
    assert!(fixture.exchange(serde_json::json!([])).await["command"].is_null());
    assert_eq!(
        fixture.submit("r1", "hello").await.unwrap().0["status"],
        "dispatched"
    );
}

#[tokio::test]
async fn replayed_events_never_duplicate_output_or_receipts() {
    let fixture = Fixture::new();
    let _ = fixture.submit("r1", "hello").await.unwrap();
    fixture.exchange(serde_json::json!([])).await;
    let batch = serde_json::json!([
        {"sequence":1,"kind":"start","requestId":"r1","turnId":"native-turn"},
        {"sequence":2,"kind":"text","requestId":"r1","text":"answer"},
        {"sequence":3,"kind":"activity","requestId":"r1","text":"[Tool: Read]"},
        {"sequence":4,"kind":"complete","requestId":"r1","answer":"answer","reason":"answer","usage":{"input_tokens":12}}
    ]);
    assert_eq!(fixture.exchange(batch.clone()).await["ack"], 4);
    assert_eq!(fixture.exchange(batch).await["ack"], 4);
    let broker = fixture.0.lock().await;
    let receipt = &broker.requests["r1"];
    assert_eq!(receipt.status, "completed");
    assert_eq!(receipt.turn_id.as_deref(), Some("native-turn"));
    assert_eq!(receipt.output, "answer");
    assert_eq!(receipt.activity.len(), 1);
    drop(broker);
    let disk = load_receipts(&fixture.2).unwrap();
    assert_eq!(disk["r1"].status, "completed");
    assert_eq!(disk["r1"].usage.as_ref().unwrap()["input_tokens"], 12);
}

#[tokio::test]
async fn abort_before_dispatch_never_starts_a_turn() {
    let fixture = Fixture::new();
    let _ = fixture.submit("r1", "hello").await.unwrap();
    fixture.abort("r1").await;
    assert!(fixture.exchange(serde_json::json!([])).await["command"].is_null());
    assert_eq!(fixture.0.lock().await.requests["r1"].status, "aborted");
}

#[tokio::test]
async fn abort_dispatch_race_targets_only_the_associated_native_turn() {
    let fixture = Fixture::new();
    let _ = fixture.submit("r1", "hello").await.unwrap();
    fixture.exchange(serde_json::json!([])).await;
    fixture.abort("r1").await;
    let command = fixture.exchange(serde_json::json!([])).await;
    assert_eq!(command["command"]["kind"], "abort");
    assert_eq!(command["command"]["id"], "r1");
    assert!(command["command"]["turnId"].is_null());
    let command = fixture.exchange(serde_json::json!([{"sequence":1,"kind":"start","requestId":"r1","turnId":"owned-turn"}])).await;
    assert_eq!(command["command"]["turnId"], "owned-turn");
    fixture.exchange(serde_json::json!([{"sequence":2,"kind":"complete","requestId":"r1","reason":"aborted","isAborted":true}])).await;
    assert!(fixture.exchange(serde_json::json!([])).await["command"].is_null());
}

#[tokio::test]
async fn restart_marks_unfinished_receipts_uncertain_and_never_requeues_them() {
    let fixture = Fixture::new();
    let _ = fixture.submit("queued", "not delivered").await.unwrap();
    let requests = load_receipts(&fixture.2).unwrap();
    assert_eq!(requests["queued"].status, "uncertain");
    assert!(
        requests["queued"]
            .error
            .as_deref()
            .unwrap()
            .contains("not resent")
    );
    assert_eq!(
        load_receipts(&fixture.2).unwrap()["queued"].status,
        "uncertain"
    );
}

#[tokio::test]
async fn offline_busy_and_stopped_states_refuse_new_delivery() {
    let fixture = Fixture::new();
    fixture.0.lock().await.busy = true;
    assert!(fixture.submit("r1", "hello").await.is_err());
    {
        let mut broker = fixture.0.lock().await;
        broker.busy = false;
        broker.stopped = true;
    }
    assert!(fixture.submit("r1", "hello").await.is_err());
    {
        let mut broker = fixture.0.lock().await;
        broker.stopped = false;
        broker.connected = false;
    }
    assert!(fixture.submit("r1", "hello").await.is_err());
    assert!(fixture.0.lock().await.requests.is_empty());
}

#[tokio::test]
async fn browser_origins_bad_tokens_and_wrong_sessions_are_rejected() {
    let fixture = Fixture::new();
    let mut headers = fixture.1.clone();
    headers.insert("origin", "http://localhost".parse().unwrap());
    assert!(!authorized(&headers, &*fixture.0.lock().await));
    assert!(!authorized(&HeaderMap::new(), &*fixture.0.lock().await));
    assert!(
        read(
            State(fixture.0.clone()),
            fixture.1.clone(),
            Json(serde_json::json!({"sessionId":"other","instance":"test-instance"}))
        )
        .await
        .is_err()
    );
}

#[test]
fn receipt_names_and_utf8_output_are_bounded() {
    let path = request_file(Path::new("/registry"), "../../escape");
    assert_eq!(path.parent(), Some(Path::new("/registry")));
    let mut output = "x".repeat(256 * 1024 - 1);
    append_bounded(&mut output, "界");
    assert_eq!(output.len(), 256 * 1024 - 1);
}

#[tokio::test]
async fn receipt_cache_is_bounded_without_losing_delivery_deduplication() {
    let fixture = Fixture::new();
    for index in 0..48 {
        let id = format!("r{index}");
        let _ = fixture.submit(&id, "hello").await.unwrap();
        fixture.exchange(serde_json::json!([
            {"sequence":index * 2 + 1,"kind":"start","requestId":id,"turnId":format!("turn-{index}")},
            {"sequence":index * 2 + 2,"kind":"complete","requestId":id,"reason":"answer","answer":"done"}
        ])).await;
        assert!(fixture.0.lock().await.requests.len() <= RECEIPT_CACHE_LIMIT);
    }
    assert!(load_receipts(&fixture.2).unwrap().len() <= RECEIPT_CACHE_LIMIT);
    {
        let mut broker = fixture.0.lock().await;
        broker.requests.clear();
        broker.connected = false;
    }
    let duplicate = fixture.submit("r0", "hello").await.unwrap().0;
    assert_eq!(duplicate["status"], "completed");
    assert_eq!(duplicate["turnId"], "turn-0");
    assert!(fixture.0.lock().await.pending.is_none());
}

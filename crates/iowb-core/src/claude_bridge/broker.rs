use super::*;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

struct Broker {
    manifest: Manifest,
    root: PathBuf,
    requests: HashMap<String, Submission>,
    connected: bool,
    busy: bool,
    stopped: bool,
    last_seen: Instant,
    pending: Option<String>,
    event_ack: u64,
}

type Shared = Arc<Mutex<Broker>>;
type Reply = std::result::Result<Json<Value>, (StatusCode, String)>;
const RECEIPT_CACHE_LIMIT: usize = 32;

fn failure(error: impl ToString) -> (StatusCode, String) {
    (StatusCode::CONFLICT, error.to_string())
}

fn authorized(headers: &HeaderMap, broker: &Broker) -> bool {
    let expected = format!("Bearer {}", broker.manifest.token);
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        == Some(&expected)
        && !headers.contains_key("origin")
}

impl Broker {
    fn save(&self, request: &Submission) -> Result<()> {
        atomic_json(&request_file(&self.root, &request.id), request)
    }

    fn online(&self) -> bool {
        self.connected && self.last_seen.elapsed() < Duration::from_secs(8)
    }

    fn receipt(&self, id: &str) -> Result<Option<Submission>> {
        if let Some(request) = self.requests.get(id) {
            return Ok(Some(request.clone()));
        }
        let path = request_file(&self.root, id);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 {
            return Err(CoreError::Conflict(
                "Invalid Claude delivery receipt; inspect the native conversation before retrying"
                    .into(),
            ));
        }
        let request: Submission = serde_json::from_slice(&std::fs::read(path)?)?;
        if request.id != id {
            return Err(CoreError::Conflict(
                "Claude receipt identity mismatch".into(),
            ));
        }
        Ok(Some(request))
    }

    fn remember(&mut self, request: Submission) {
        if !self.requests.contains_key(&request.id) && self.requests.len() >= RECEIPT_CACHE_LIMIT {
            if let Some(old) = self
                .requests
                .iter()
                .find(|(_, receipt)| receipt.terminal())
                .map(|(id, _)| id.clone())
            {
                self.requests.remove(&old);
            }
        }
        self.requests.insert(request.id.clone(), request);
    }
}

/// Run only as the terminal mod's child. stdout is a single private handshake;
/// the bearer is never an argument, log entry, or Workbench server credential.
pub async fn serve(session_id: String, cwd: PathBuf) -> Result<()> {
    #[cfg(not(unix))]
    return Err(CoreError::InvalidInput(
        "The Claude terminal bridge currently requires a Unix platform".into(),
    ));
    validate_id(&session_id)?;
    let cwd = std::fs::canonicalize(cwd)?;
    let root = directory().join(&session_id);
    std::fs::create_dir_all(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(root.join("owner.lock"))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // This descriptor remains alive until serve exits.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(CoreError::Conflict(
                "This native Claude session already has a bridge owner".into(),
            ));
        }
    }
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let manifest = Manifest {
        protocol: 1,
        session_id,
        cwd,
        url: format!("http://127.0.0.1:{}", listener.local_addr()?.port()),
        token: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        instance: uuid::Uuid::new_v4().to_string(),
        pid: std::process::id(),
    };
    let requests = load_receipts(&root)?;
    atomic_json(&root.join("connection.json"), &manifest)?;
    println!("{}", serde_json::to_string(&manifest)?);
    let broker = Arc::new(Mutex::new(Broker {
        manifest,
        root,
        requests,
        connected: false,
        busy: false,
        stopped: false,
        last_seen: Instant::now(),
        pending: None,
        event_ack: 0,
    }));
    // A heartbeat lets the mod retire this child promptly on /clear or /resume,
    // even while the helper otherwise has no stdout to produce.
    let heartbeat = tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            println!();
        }
    });
    let router = Router::new()
        .route("/exchange", post(exchange))
        .route("/submit", post(submit))
        .route("/read", post(read))
        .route("/abort", post(abort))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(broker);
    let result = axum::serve(listener, router).await.map_err(CoreError::Io);
    heartbeat.abort();
    drop(lock);
    result
}

async fn submit(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(mut request): Json<Submission>,
) -> Reply {
    let mut broker = shared.lock().await;
    if !authorized(&headers, &broker) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized local bridge client".into(),
        ));
    }
    if request.id.is_empty()
        || request.id.len() > 256
        || request.prompt.is_empty()
        || request.prompt.len() > 256 * 1024
    {
        return Err(failure("Invalid Claude bridge submission"));
    }
    if let Some(existing) = broker.receipt(&request.id).map_err(failure)? {
        if existing.prompt != request.prompt
            || existing.model != request.model
            || existing.effort != request.effort
        {
            return Err(failure(
                "This Claude request ID already belongs to a different submission",
            ));
        }
        return Ok(Json(serde_json::to_value(existing).map_err(failure)?));
    }
    if !broker.online() {
        return Err(failure(
            "Claude bridge is disconnected; no message was sent",
        ));
    }
    if broker.stopped {
        return Err(failure(
            "Claude was stopped in the terminal. Type a message there to continue; no message was sent",
        ));
    }
    if broker.busy || broker.pending.is_some() || broker.requests.values().any(|r| !r.terminal()) {
        return Err(failure(
            "Claude is still running a turn in this chat. Wait for it to finish; no message was sent",
        ));
    }
    // Ignore caller-supplied receipts and output; only the mod may supply them.
    request = Submission {
        id: request.id,
        prompt: request.prompt,
        model: request.model,
        effort: request.effort,
        status: "queued".into(),
        ..Default::default()
    };
    broker.save(&request).map_err(failure)?;
    broker.pending = Some(request.id.clone());
    broker.remember(request.clone());
    Ok(Json(serde_json::to_value(request).map_err(failure)?))
}

async fn read(State(shared): State<Shared>, headers: HeaderMap, Json(value): Json<Value>) -> Reply {
    let broker = shared.lock().await;
    if !authorized(&headers, &broker) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized local bridge client".into(),
        ));
    }
    if value["sessionId"] != broker.manifest.session_id
        || value["instance"] != broker.manifest.instance
    {
        return Err(failure(
            "Claude session identity changed; no message was sent",
        ));
    }
    if let Some(id) = value["id"].as_str() {
        let request = broker
            .receipt(id)
            .map_err(failure)?
            .ok_or_else(|| failure("Claude bridge has no receipt for this request"))?;
        let mut request = request;
        if !request.terminal() && !broker.online() {
            request.status = "uncertain".into();
            request.error = Some("Claude stopped acknowledging the bridge. Inspect the terminal; this prompt will not be resent.".into());
        }
        return Ok(Json(serde_json::to_value(request).map_err(failure)?));
    }
    Ok(Json(
        serde_json::json!({"sessionId":broker.manifest.session_id,"instance":broker.manifest.instance,"cwd":broker.manifest.cwd,"connected":broker.online(),"busy":broker.busy,"stopped":broker.stopped}),
    ))
}

async fn abort(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Reply {
    let mut broker = shared.lock().await;
    if !authorized(&headers, &broker) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized local bridge client".into(),
        ));
    }
    let id = value["id"]
        .as_str()
        .ok_or_else(|| failure("Missing Claude request ID"))?;
    let request = broker
        .receipt(id)
        .map_err(failure)?
        .ok_or_else(|| failure("Unknown Claude request"))?;
    if !request.terminal() {
        let mut request = request.clone();
        request.cancel_requested = true;
        if request.status == "queued" {
            request.status = "aborted".into();
            broker.pending = None;
        }
        broker.save(&request).map_err(failure)?;
        broker.remember(request);
    }
    Ok(Json(serde_json::json!({})))
}

async fn exchange(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(value): Json<Value>,
) -> Reply {
    let mut broker = shared.lock().await;
    if !authorized(&headers, &broker) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Unauthorized local bridge client".into(),
        ));
    }
    if value["sessionId"] != broker.manifest.session_id
        || value["instance"] != broker.manifest.instance
    {
        return Err(failure("Claude bridge identity mismatch"));
    }
    broker.last_seen = Instant::now();
    broker.connected = value["connected"].as_bool().unwrap_or(true);
    broker.busy = value["busy"].as_bool().unwrap_or(false);
    broker.stopped = value["stopped"].as_bool().unwrap_or(false);
    for event in value["events"].as_array().into_iter().flatten().take(512) {
        let sequence = event["sequence"]
            .as_u64()
            .ok_or_else(|| failure("Missing Claude bridge event sequence"))?;
        if sequence <= broker.event_ack {
            continue;
        }
        if sequence != broker.event_ack + 1 {
            return Err(failure("Claude bridge event sequence gap"));
        }
        let Some(id) = event["requestId"].as_str() else {
            continue;
        };
        let Some(mut request) = broker.receipt(id).map_err(failure)? else {
            return Err(failure("Unknown Claude bridge request"));
        };
        if request.terminal() {
            broker.event_ack = sequence;
            continue;
        }
        match event["kind"].as_str().unwrap_or("") {
            "dispatch" => {}
            "start" => {
                request.turn_id = event["turnId"].as_str().map(str::to_string);
                request.user_message_id = event["userMessageId"].as_str().map(str::to_string);
                request.status = "running".into();
            }
            "text" => {
                if let Some(text) = event["text"].as_str() {
                    append_bounded(&mut request.output, text);
                }
            }
            "activity" => {
                if request.activity.len() < 96
                    && let Some(text) = event["text"].as_str()
                {
                    request.activity.push(text.chars().take(8192).collect());
                }
            }
            "complete" => {
                request.status = if event["isAborted"] == true {
                    "aborted"
                } else if event["reason"] == "error" || event["reason"] == "refusal" {
                    "failed"
                } else {
                    "completed"
                }
                .into();
                request.answer.clear();
                append_bounded(&mut request.answer, event["answer"].as_str().unwrap_or(""));
                request.usage = event.get("usage").filter(|v| !v.is_null()).cloned();
            }
            "reject" => {
                request.status = "rejected".into();
                request.error = event["error"].as_str().map(str::to_string);
            }
            "cancelled" => {
                request.status = "aborted".into();
            }
            "uncertain" => {
                request.status = "uncertain".into();
                request.error = event["error"].as_str().map(str::to_string);
            }
            _ => continue,
        }
        // Starts and terminal receipts are durable before acknowledging the mod.
        if event["kind"] != "text" && event["kind"] != "activity" {
            broker.save(&request).map_err(failure)?;
        }
        if broker.pending.as_deref() == Some(id) {
            broker.pending = None;
        }
        broker.remember(request);
        broker.event_ack = sequence;
    }
    let command = if let Some(request) = broker
        .requests
        .values()
        .find(|r| r.cancel_requested && !r.terminal())
    {
        serde_json::json!({"kind":"abort","id":request.id,"turnId":request.turn_id})
    } else if !broker.busy && !broker.stopped && broker.connected {
        if let Some(id) = broker.pending.clone() {
            let mut request = broker.requests[&id].clone();
            request.status = "dispatched".into();
            broker.save(&request).map_err(failure)?;
            broker.remember(request.clone());
            serde_json::json!({"kind":"submit","request":request})
        } else {
            Value::Null
        }
    } else {
        Value::Null
    };
    Ok(Json(
        serde_json::json!({"command":command,"ack":broker.event_ack}),
    ))
}

fn append_bounded(target: &mut String, text: &str) {
    if target.len() >= 256 * 1024 {
        return;
    }
    let remaining = 256 * 1024 - target.len();
    let mut end = text.len().min(remaining);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    target.push_str(&text[..end]);
}

fn load_receipts(root: &Path) -> Result<HashMap<String, Submission>> {
    let mut requests = HashMap::new();
    for entry in std::fs::read_dir(root)?.flatten() {
        let path = entry.path();
        if !entry.file_name().to_string_lossy().starts_with("request-") {
            continue;
        }
        if !entry.metadata().is_ok_and(|m| m.len() <= 2 * 1024 * 1024) {
            continue;
        }
        let mut request: Submission = match std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        {
            Some(request) => request,
            None => continue,
        };
        if !request.terminal() {
            request.status = "uncertain".into();
            request.error = Some("The Claude bridge restarted during delivery. Inspect the native conversation before retrying; this prompt was not resent.".into());
            atomic_json(&path, &request)?;
        }
        if requests.len() >= RECEIPT_CACHE_LIMIT {
            requests.clear();
        }
        requests.insert(request.id.clone(), request);
    }
    Ok(requests)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

// Serialize discussion generation per user/board to avoid competing proposals.
// Discussion provider sessions are isolated from the board execution session.
static DISCUSSION_OPERATIONS: OnceLock<Mutex<BTreeSet<(String, String)>>> = OnceLock::new();

#[derive(Debug)]
struct DiscussionOperation {
    key: (String, String),
}

impl Drop for DiscussionOperation {
    fn drop(&mut self) {
        if let Some(operations) = DISCUSSION_OPERATIONS.get()
            && let Ok(mut operations) = operations.lock()
        {
            operations.remove(&self.key);
        }
    }
}

fn claim_discussion_operation(user_id: &str, board_id: &str) -> Result<DiscussionOperation> {
    let key = (user_id.to_string(), board_id.to_string());
    let mut operations = DISCUSSION_OPERATIONS
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .map_err(|_| {
            ServerError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Discussion operation lock is unavailable.",
            )
        })?;
    if !operations.insert(key.clone()) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "A discussion proposal is already running for this board. Wait for it to finish and refresh the board.",
        ));
    }
    Ok(DiscussionOperation { key })
}

async fn await_discussion_completion<F>(
    operation: DiscussionOperation,
    work: F,
) -> Result<Json<Value>>
where
    F: std::future::Future<Output = Result<Json<Value>>> + Send + 'static,
{
    // The worker owns provider completion AND proposal persistence. Dropping
    // the HTTP waiter detaches this JoinHandle; it does not cancel the work.
    tokio::spawn(async move {
        let _operation = operation;
        work.await
    })
    .await
    .map_err(|error| {
        ServerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Discussion proposal worker failed: {error}"),
        )
    })?
}

fn recover_orphaned_discussion_proposals(run: &mut AgenticBoard, task_id: &str) {
    // Called only after acquiring the process-owned single-flight claim. Any
    // older running record has no remaining worker here (legacy disconnect or
    // server restart). Retain it as failed history; do not auto-apply raw chat.
    let orphaned = run
        .discussion_proposals
        .iter()
        .filter(|proposal| {
            proposal.get("taskId").and_then(Value::as_str) == Some(task_id)
                && proposal.get("status").and_then(Value::as_str) == Some("running")
        })
        .cloned()
        .collect::<Vec<_>>();
    let count = orphaned.len();
    for mut proposal in orphaned {
        let error = "Previous discussion request was interrupted before its proposal was saved. This retry starts a new proposal; nothing was applied.";
        let mut transcript = proposal
            .get("transcript")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        transcript.push(json!({"timestamp": Utc::now(), "kind": "status", "status": "failed", "content": error}));
        proposal["status"] = json!("failed");
        proposal["error"] = json!(error);
        proposal["completedAt"] = json!(Utc::now());
        proposal["transcript"] = json!(transcript);
        update_discussion_proposal(run, task_id, proposal);
    }
    if count > 0 {
        let label = format!("discussion proposal for {task_id}");
        if run.provider_call_label.as_deref() == Some(label.as_str()) {
            run.provider_call_started_at = None;
            run.provider_call_label = None;
            run.current_provider_session_id = None;
        }
        run.append_log(format!("Recovered {count} interrupted discussion proposal(s) for {task_id}; previous proposals were not applied"));
    }
}

fn discussion_source_identity(snapshot: &Value) -> Option<Value> {
    let target = snapshot.get("task")?;
    let mut identity = serde_json::Map::new();
    identity.insert(
        target.get("id")?.as_str()?.to_string(),
        json!(target.get("scopeVersion")?.as_u64()?),
    );
    for child in snapshot.get("descendants")?.as_array()? {
        if identity
            .insert(
                child.get("id")?.as_str()?.to_string(),
                json!(child.get("scopeVersion")?.as_u64()?),
            )
            .is_some()
        {
            return None;
        }
    }
    Some(Value::Object(identity))
}

fn validate_discussion_source_scope(
    run: &AgenticBoard,
    task_id: &str,
    proposal: &Value,
) -> Result<()> {
    let task = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    let source_version = proposal
        .get("sourceScopeVersion")
        .and_then(Value::as_u64)
        .or_else(|| {
            proposal
                .get("before")?
                .get("task")?
                .get("scopeVersion")?
                .as_u64()
        });
    let Some(source_version) = source_version else {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "This proposal has no recorded source scope version. Generate and review a fresh proposal before applying it.",
        ));
    };
    if source_version != task.hierarchy.scope_version {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Ticket scope changed after this discussion started. Generate and review a fresh proposal before applying it.",
        ));
    }
    let source_identity = proposal
        .get("sourceScopeIdentity")
        .cloned()
        .or_else(|| discussion_source_identity(proposal.get("before")?));
    let current_identity = discussion_source_identity(&discussion_scope_snapshot(run, task_id));
    if source_identity.is_none() || source_identity != current_identity {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Ticket or descendant scope changed, or this proposal lacks a complete source scope record. Generate and review a fresh proposal before applying it.",
        ));
    }
    Ok(())
}

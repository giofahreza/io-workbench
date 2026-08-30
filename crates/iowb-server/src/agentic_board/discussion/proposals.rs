fn normalize_discussion_action(value: Option<&str>) -> String {
    value
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_")
}

fn discussion_running_entry(
    proposal_id: &str,
    task_id: &str,
    message: &str,
    requested_action: &str,
    requested_payload: &Value,
    provider: &str,
    model: &str,
    started_at: DateTime<Utc>,
) -> Value {
    json!({
        "id": proposal_id,
        "proposalId": proposal_id,
        "taskId": task_id,
        "kind": "proposal",
        "message": redact_transcript_text(message),
        "requestedAction": requested_action,
        "requestedPayload": redact_transcript_value(requested_payload),
        "action": requested_action,
        "payload": {},
        "status": "running",
        "provider": provider,
        "model": model,
        "createdAt": started_at,
        "transcript": json!([
            {
                "timestamp": started_at,
                "kind": "message",
                "role": "user",
                "content": redact_transcript_text(message),
            },
            {
                "timestamp": started_at,
                "kind": "status",
                "role": "assistant",
                "provider": provider,
                "model": model,
                "status": "running",
                "content": "Preparing a discussion proposal."
            }
        ]),
    })
}

fn manual_test_steps_evidence(parsed: &Value) -> Vec<String> {
    [
        "manualTestSteps",
        "manual_test_steps",
        "manualSteps",
        "manual_steps",
    ]
    .into_iter()
    .find_map(|key| parsed.get(key))
    .map(|value| normalize_string_list(Some(value)))
    .unwrap_or_default()
}

fn manual_test_result_evidence(parsed: &Value) -> Option<String> {
    [
        "manualTestResult",
        "manual_test_result",
        "manualResult",
        "manual_result",
    ]
    .into_iter()
    .find_map(|key| parsed.get(key))
    .and_then(value_to_trimmed_text)
}

fn manual_test_result_is_successful(result: &str) -> bool {
    let normalized = result
        .trim()
        .to_ascii_lowercase()
        .replace('_', " ")
        .replace('-', " ");
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty()
        || [
            "not run",
            "not performed",
            "not executed",
            "not tested",
            "untested",
            "skipped",
            "skip",
            "unknown",
            "pending",
            "inconclusive",
            "unable to verify",
            "unable to be verified",
            "could not verify",
            "could not be verified",
            "cannot verify",
            "cannot be verified",
            "can't verify",
            "can't be verified",
            "not verified",
        ]
        .iter()
        .any(|needle| normalized.contains(needle))
    {
        return false;
    }

    let failure_is_clearly_negated = ["no fail", "no failure", "without fail", "without failure"]
        .iter()
        .any(|needle| normalized.contains(needle));
    let error_is_clearly_negated = ["no error", "no errors", "without error", "without errors"]
        .iter()
        .any(|needle| normalized.contains(needle));
    if (normalized.contains("fail") && !failure_is_clearly_negated)
        || (normalized.contains("error") && !error_is_clearly_negated)
        || [
            "broken",
            "blocked",
            "not working",
            "does not work",
            "regression",
            "defect",
        ]
        .iter()
        .any(|needle| normalized.contains(needle))
    {
        return false;
    }

    normalized == "ok"
        || normalized.starts_with("ok:")
        || normalized.starts_with("ok ")
        || normalized == "pass"
        || normalized == "passed"
        || normalized.starts_with("pass:")
        || normalized.starts_with("pass ")
        || normalized.starts_with("passed:")
        || normalized.starts_with("passed ")
        || normalized == "success"
        || normalized == "successful"
        || normalized.contains("successfully")
        || normalized.contains("verified successfully")
        || normalized.contains("verified")
        || normalized.contains("works as expected")
        || normalized.contains("worked as expected")
        || normalized.contains("all steps passed")
        || normalized.contains("all checks passed")
        || normalized.contains("meets acceptance")
        || normalized.contains("expected behavior")
        || normalized.contains("no issues")
        || normalized.contains("no problems")
        || failure_is_clearly_negated
        || error_is_clearly_negated
}

fn discussion_completed_transcript(
    message: &str,
    assistant: &str,
    provider: &str,
    model: &str,
    started_at: DateTime<Utc>,
) -> Value {
    json!([
        {
            "timestamp": started_at,
            "kind": "message",
            "role": "user",
            "content": redact_transcript_text(message),
        },
        {
            "timestamp": Utc::now(),
            "kind": "assistant",
            "role": "assistant",
            "provider": provider,
            "model": model,
            "status": "completed",
            "content": redact_transcript_text(assistant),
        }
    ])
}

fn mark_discussion_proposal_failed(
    proposal: &mut Value,
    error: &str,
    provider: &str,
    model: &str,
    started_at: DateTime<Utc>,
) {
    if let Some(object) = proposal.as_object_mut() {
        object.insert("status".to_string(), json!("failed"));
        object.insert("error".to_string(), json!(redact_transcript_text(error)));
        object.insert("provider".to_string(), json!(provider));
        object.insert("model".to_string(), json!(model));
        object.insert("completedAt".to_string(), json!(Utc::now()));
        object.insert(
            "transcript".to_string(),
            discussion_completed_transcript("", error, provider, model, started_at),
        );
    }
}

fn append_discussion_proposal(run: &mut AgenticBoard, task_id: &str, entry: Value) -> Result<()> {
    let task = run
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    task.hierarchy.discussion.push(entry.clone());
    run.discussion_proposals.push(entry);
    Ok(())
}

fn update_discussion_proposal(run: &mut AgenticBoard, task_id: &str, proposal: Value) {
    let proposal_id = proposal.get("id").and_then(Value::as_str);
    if let Some(index) = run.discussion_proposals.iter().position(|item| {
        proposal_id.is_some() && item.get("id").and_then(Value::as_str) == proposal_id
    }) {
        run.discussion_proposals[index] = proposal.clone();
    }
    if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id)
        && let Some(index) = task.hierarchy.discussion.iter().position(|item| {
            proposal_id.is_some() && item.get("id").and_then(Value::as_str) == proposal_id
        })
    {
        task.hierarchy.discussion[index] = proposal;
    }
}

fn discussion_task_scope(task: &BoardTask) -> Value {
    json!({
        "id": task.id,
        "title": task.title,
        "level": task_level(task),
        "kind": canonical_task_kind(task),
        "status": canonical_task_status(&task.status),
        "description": if task.description.trim().is_empty() { &task.details } else { &task.description },
        "acceptanceCriteria": task.acceptance_criteria,
        "parentId": task.hierarchy.parent_id,
        "blockedBy": task_blockers(task),
        "priority": normalize_priority(Some(&task.priority)),
        "rank": task.hierarchy.rank,
        "required": task.hierarchy.required,
        "scopeVersion": task.hierarchy.scope_version,
        "plannedFiles": task.hierarchy.planned_files,
        "sideEffects": task.hierarchy.side_effects,
    })
}

fn discussion_scope_snapshot(run: &AgenticBoard, task_id: &str) -> Value {
    let ids = descendant_task_ids(run, task_id);
    let task = run.tasks.iter().find(|task| task.id == task_id);
    let descendants = run
        .tasks
        .iter()
        .filter(|task| ids.contains(&task.id) && task.id != task_id)
        .map(discussion_task_scope)
        .collect::<Vec<_>>();
    json!({
        "task": task.map(discussion_task_scope),
        "descendants": descendants,
    })
}

fn discussion_diff(before: &Value, after: &Value) -> Value {
    let mut changes = Vec::new();
    if let (Some(before), Some(after)) = (before.as_object(), after.as_object()) {
        let keys = before
            .keys()
            .chain(after.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for key in keys {
            let previous = before.get(&key).cloned().unwrap_or(Value::Null);
            let next = after.get(&key).cloned().unwrap_or(Value::Null);
            if previous != next {
                changes.push(json!({
                    "path": key,
                    "before": previous,
                    "after": next,
                }));
            }
        }
    } else if before != after {
        changes.push(json!({ "path": "$", "before": before, "after": after }));
    }
    json!({
        "changed": !changes.is_empty(),
        "changes": changes,
        "before": before,
        "after": after,
    })
}

fn build_discussion_proposal_prompt(
    run: &AgenticBoard,
    task_id: &str,
    message: &str,
    requested_action: &str,
    requested_payload: &Value,
) -> Result<String> {
    let task = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    let actions = "message|edit|replace|delete|split|merge|regenerate_children|reprioritize|re_research|revision|fix|replacement";
    Ok(format!(
        r#"Prepare a structured proposal for a Kanban ticket discussion.

Do not edit files, run implementation, or apply any board mutation. The user
must explicitly approve the returned proposal through a separate Apply action.

Ticket:
{ticket}

Current ticket scope and descendants:
{scope}

User message:
{message}

Requested action (a hint, not an instruction to mutate): {requested_action}
Requested payload hint:
{requested_payload}

Return JSON only, without markdown:
{{
  "action": "{actions}",
  "summary": "what should change or what was discussed",
  "payload": {{}},
  "warnings": ["scope or approval warning"],
  "acceptanceCriteria": ["criteria affected by this proposal"]
}}

Rules:
- Use exactly one action from the list.
- If the user only asks a question, use `message`, leave payload empty, and answer in summary.
- For edit or replace, payload may contain only ticket scope fields: title, details,
  description, kind, taskType, level, parentId, acceptanceCriteria, priority,
  rank, blockedBy, dependsOn, required, plannedFiles, and sideEffects.
- For reprioritize, payload must contain a valid priority p0, p1, p2, or p3.
- For split, payload must contain an items array of one-purpose child tickets.
- For merge, payload must contain targetId.
- For re_research, revision, fix, or replacement, payload should describe the
  new linked Backlog planning item; never reopen the completed source item.
- For replacement, include `supersedeSource: true` only when the user explicitly
  wants the completed source item marked superseded after applying this proposal.
- A scope-changing proposal is still pending when the ticket is locked; warn that
  it cannot be applied until the scope owner is moved back to Backlog.
- Do not create executable work above the subtask level. If you propose children,
  preserve the next hierarchy level and make only subtasks executable.
- Do not introduce a separate tracking matrix or external IDs. Ticket fields are the source of truth.
- Do not include secrets, tokens, or raw environment values in the response.

Codebase context:
{codebase}
"#,
        ticket = serde_json::to_string_pretty(&discussion_task_scope(task)).unwrap_or_default(),
        scope = serde_json::to_string_pretty(&discussion_scope_snapshot(run, task_id))
            .unwrap_or_default(),
        message = redact_transcript_text(message),
        requested_action = if requested_action.is_empty() {
            "message"
        } else {
            requested_action
        },
        requested_payload =
            serde_json::to_string_pretty(&redact_transcript_value(requested_payload))
                .unwrap_or_default(),
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
    ))
}

fn sanitize_discussion_proposal(
    run: &AgenticBoard,
    task_id: &str,
    proposal_id: &str,
    requested_action: &str,
    message: &str,
    requested_payload: &Value,
    parsed: &Value,
    provider: &str,
    model: &str,
    started_at: DateTime<Utc>,
) -> Result<Value> {
    let parsed_action = normalize_discussion_action(parsed.get("action").and_then(Value::as_str));
    let action = if requested_action.is_empty() {
        if parsed_action.is_empty() {
            "message".to_string()
        } else {
            parsed_action
        }
    } else {
        requested_action.to_string()
    };
    if !matches!(
        action.as_str(),
        "message"
            | "edit"
            | "replace"
            | "delete"
            | "split"
            | "merge"
            | "regenerate_children"
            | "reprioritize"
            | "re_research"
            | "revision"
            | "fix"
            | "replacement"
            | "research"
    ) {
        return Err(bad_request(format!(
            "Unsupported discussion action: {action}"
        )));
    }
    let mut payload = sanitize_discussion_payload(&action, parsed)?;
    if action == "replacement"
        && requested_payload
            .get("supersedeSource")
            .and_then(Value::as_bool)
            == Some(true)
    {
        if let Some(object) = payload.as_object_mut() {
            object.insert("supersedeSource".to_string(), json!(true));
        }
    }
    validate_discussion_payload(&action, &payload)?;
    let before = discussion_scope_snapshot(run, task_id);
    let mut preview = run.clone();
    let mut warnings = normalize_string_list(parsed.get("warnings"));
    if let Err(error) = apply_discussion_action(&mut preview, task_id, &action, &payload) {
        warnings.push(server_error_message(&error));
    }
    let after = discussion_scope_snapshot(&preview, task_id);
    if discussion_action_requires_backlog(&action) && !task_scope_owner_is_backlog(run, task_id) {
        warnings.push(
            "This scope change remains pending because the ticket is approved or locked; move its scope owner back to Backlog before applying it.".to_string(),
        );
    }
    let summary = parsed
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if message.trim().is_empty() {
                "Discussion proposal"
            } else {
                message.trim()
            }
        });
    Ok(json!({
        "id": proposal_id,
        "proposalId": proposal_id,
        "taskId": task_id,
        "kind": "proposal",
        "message": redact_transcript_text(message),
        "action": action,
        "payload": redact_transcript_value(&payload),
        "summary": redact_transcript_text(summary),
        "warnings": dedupe_strings(warnings),
        "acceptanceCriteria": normalize_string_list(parsed.get("acceptanceCriteria")),
        "before": before,
        "after": after,
        "diff": discussion_diff(&before, &after),
        "status": "pending",
        "provider": provider,
        "model": model,
        "createdAt": started_at,
        "completedAt": Utc::now(),
    }))
}

fn sanitize_discussion_payload(action: &str, parsed: &Value) -> Result<Value> {
    if action == "message" || action == "delete" || action == "regenerate_children" {
        return Ok(json!({}));
    }
    if let Some(payload) = parsed.get("payload")
        && payload.is_object()
    {
        return Ok(payload.clone());
    }
    let Some(object) = parsed.as_object() else {
        return Ok(json!({}));
    };
    let allowed = [
        "title",
        "details",
        "description",
        "kind",
        "taskType",
        "level",
        "parentId",
        "acceptanceCriteria",
        "priority",
        "rank",
        "blockedBy",
        "dependsOn",
        "required",
        "plannedFiles",
        "sideEffects",
        "supersedeSource",
        "items",
        "children",
        "targetId",
    ];
    let mut payload = serde_json::Map::new();
    for key in allowed {
        if let Some(value) = object.get(key) {
            payload.insert(key.to_string(), value.clone());
        }
    }
    Ok(Value::Object(payload))
}

fn validate_discussion_payload(action: &str, payload: &Value) -> Result<()> {
    match action {
        "edit" | "replace" => {
            if !payload.is_object() || payload.as_object().is_none_or(|object| object.is_empty()) {
                return Err(bad_request(
                    "Discussion edit proposal must contain at least one scope field.",
                ));
            }
        }
        "reprioritize" => {
            let priority = payload
                .get("priority")
                .and_then(Value::as_str)
                .ok_or_else(|| bad_request("Reprioritize proposal must contain priority."))?;
            if !matches!(
                normalize_priority(Some(priority)),
                TASK_PRIORITY_P0 | TASK_PRIORITY_P1 | TASK_PRIORITY_P2 | TASK_PRIORITY_P3
            ) {
                return Err(bad_request("Priority must be p0, p1, p2, or p3."));
            }
        }
        "split" => {
            let items = payload
                .get("items")
                .or_else(|| payload.get("children"))
                .and_then(Value::as_array)
                .ok_or_else(|| bad_request("Split proposal must contain items."))?;
            if items.is_empty() {
                return Err(bad_request(
                    "Split proposal must contain at least one item.",
                ));
            }
        }
        "merge" => {
            if payload
                .get("targetId")
                .or_else(|| payload.get("target_id"))
                .and_then(Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(bad_request("Merge proposal must contain targetId."));
            }
        }
        "re_research" | "research" | "revision" | "fix" | "replacement" => {}
        _ => {}
    }
    Ok(())
}

fn discussion_action_requires_backlog(action: &str) -> bool {
    matches!(
        action,
        "edit" | "replace" | "delete" | "split" | "merge" | "regenerate_children"
    )
}

fn task_scope_owner_is_backlog(run: &AgenticBoard, task_id: &str) -> bool {
    let item_is_backlog = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .is_some_and(|task| task_status_is_backlog(&task.status));
    if item_is_backlog {
        return true;
    }
    let owner_id = top_level_parent_id(run, task_id).unwrap_or(task_id);
    run.tasks
        .iter()
        .find(|task| task.id == owner_id)
        .is_some_and(|task| task_status_is_backlog(&task.status))
}

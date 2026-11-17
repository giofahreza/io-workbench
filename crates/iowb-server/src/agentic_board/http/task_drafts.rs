#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcceptTaskDraftsRequest {
    prompt: String,
    drafts: Vec<Value>,
}

// A preview is a plan, not a list of unrelated manual tasks. Resolve its whole
// dependency graph and hierarchy before committing anything or allocating live
// IDs. Accepting a preview never calls a provider, approves scope, or starts work.
fn accept_previewed_task_drafts(run: &mut AgenticBoard, drafts: Vec<Value>, prompt: &str) -> Result<usize> {
    if prompt.trim().is_empty() {
        return Err(bad_request("The original task prompt is required."));
    }
    if drafts.is_empty() || drafts.len() > 100 {
        return Err(bad_request("Accept between 1 and 100 task drafts."));
    }
    for draft in &drafts {
        if !draft.is_object() || draft.get("title").and_then(Value::as_str).is_none_or(|title| title.trim().is_empty()) {
            return Err(invalid_generated_plan("Every draft requires a non-empty title."));
        }
        if !draft.get("planDependencies").and_then(Value::as_array).is_some_and(|refs| {
            refs.iter().all(|reference| reference.as_str().is_some_and(|text| !text.trim().is_empty()))
        }) {
            return Err(invalid_generated_plan("Every preview draft requires a planDependencies array of non-empty reference strings."));
        }
    }
    let mut candidate = run.clone();
    let trees = prepare_generated_prompt_task_trees(&mut candidate, drafts, prompt.trim())?;
    let generated = trees.into_iter().flatten().collect::<Vec<_>>();
    let count = generated.len();
    candidate.tasks.extend(generated);
    normalize_board_hierarchy(&mut candidate);
    normalize_board_task_groups(&mut candidate);
    if let Some(issue) = dependency_validation_issues(&candidate).into_iter().next() {
        return Err(invalid_generated_plan(issue));
    }
    if let Some(cycle) = dependency_cycle(&candidate) {
        return Err(invalid_generated_plan(format!("Dependency cycle detected: {}", cycle.join(" -> "))));
    }
    if let Some(issue) = hierarchy_validation_issues(&candidate).into_iter().next() {
        return Err(invalid_generated_plan(issue));
    }
    refresh_hierarchy_rollups(&mut candidate);
    candidate.append_log(format!("Accepted {count} previewed task record(s) into Backlog"));
    *run = candidate;
    Ok(count)
}

async fn accept_task_drafts(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<AcceptTaskDraftsRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    let count = accept_previewed_task_drafts(&mut stored.board, request.drafts, &request.prompt)?;
    stored.board.touch();
    save_board(&state, &stored.board)?;
    Ok((StatusCode::CREATED, Json(json!({
        "success": true,
        "createdTaskCount": count,
        "board": stored.board.detail_json(Some(stored.path.display().to_string())),
    }))))
}

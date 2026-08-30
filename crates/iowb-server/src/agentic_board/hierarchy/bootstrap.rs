async fn bootstrap_agentic_board(state: &AppState, user_id: &str, board_id: &str) -> Result<()> {
    let snapshot = load_user_board(state, user_id, board_id)?.board;
    if bootstrap_should_yield(&snapshot) {
        return Ok(());
    }
    let workspace_baseline = snapshot
        .workspace_baseline
        .is_none()
        .then(|| capture_workspace_snapshot(&snapshot.project_path));
    let agents_context = read_agents_context(&snapshot.project_path);
    mutate_stored_board(state, user_id, board_id, |run| {
        set_phase(
            run,
            "bootstrap_prepare",
            json!({ "step": "guidance_and_sources" }),
        );
        if let Some(workspace_baseline) = workspace_baseline.clone() {
            run.workspace_baseline = Some(workspace_baseline.clone());
            run.latest_workspace_snapshot = Some(workspace_baseline);
        }
        run.agents_context = Some(agents_context);
        run.append_log("Loaded AGENTS.md guidance and workspace baseline");
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, board_id)? {
        return Ok(());
    }

    let snapshot = load_user_board(state, user_id, board_id)?.board;
    let source_prompt = active_board_prompt(&snapshot);
    let source_bundle = build_source_bundle(&snapshot.project_path, &source_prompt);
    mutate_stored_board(state, user_id, board_id, |run| {
        run.source_references = source_bundle.references;
        run.source_manifest = source_bundle.manifest;
        run.source_chunks = source_bundle.chunks;
        run.append_log(format!(
            "Resolved {} source references, {} source files and {} source chunks",
            run.source_references.len(),
            run.source_manifest.len(),
            run.source_chunks.len()
        ));
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, board_id)? {
        return Ok(());
    }

    let mut rag_snapshot = load_user_board(state, user_id, board_id)?.board;
    let rag_ingestion_count = rag_snapshot.rag_ingestions.len();
    let rag_trace_count = rag_snapshot.rag_trace_refs.len();
    index_project_for_rag(&mut rag_snapshot).await;
    let rag_ingestions = rag_snapshot
        .rag_ingestions
        .into_iter()
        .skip(rag_ingestion_count)
        .collect::<Vec<_>>();
    let rag_trace_refs = rag_snapshot
        .rag_trace_refs
        .into_iter()
        .skip(rag_trace_count)
        .collect::<Vec<_>>();
    mutate_stored_board(state, user_id, board_id, |run| {
        run.rag_ingestions.extend(rag_ingestions);
        run.rag_trace_refs.extend(rag_trace_refs);
        trim_rag_history(run);
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, board_id)? {
        return Ok(());
    }

    let source_chunk_count = load_user_board(state, user_id, board_id)?
        .board
        .source_chunks
        .len();
    mutate_stored_board(state, user_id, board_id, |run| {
        set_phase(
            run,
            "codebase_manifest",
            json!({ "sourceChunks": source_chunk_count, "planning": "ticket_hierarchy" }),
        );
        Ok(())
    })?;

    let project_path = load_user_board(state, user_id, board_id)?
        .board
        .project_path;
    let codebase_bundle = build_codebase_bundle(&project_path);
    mutate_stored_board(state, user_id, board_id, |run| {
        set_phase(
            run,
            "codebase_manifest",
            json!({ "step": "build_manifest" }),
        );
        run.codebase_manifest = codebase_bundle.manifest;
        run.codebase_chunks = codebase_bundle.chunks;
        run.append_log(format!(
            "Loaded codebase manifest with {} files and {} chunks",
            run.codebase_manifest.len(),
            run.codebase_chunks.len()
        ));
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, board_id)? {
        return Ok(());
    }

    mutate_stored_board(state, user_id, board_id, |run| {
        set_phase(run, "codebase_recon", json!({}));
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, board_id)? {
        return Ok(());
    }
    let codebase_map = perform_codebase_recon(state, user_id, board_id).await?;
    mutate_stored_board(state, user_id, board_id, |run| {
        run.codebase_map = Some(codebase_map.clone());
        run.environment_state = Some(environment_from_codebase_map(&codebase_map));
        run.bootstrap_complete = true;
        set_phase(run, "task_execution", json!({ "bootstrapComplete": true }));
        run.append_log("Agentic bootstrap complete");
        Ok(())
    })?;
    Ok(())
}

fn bootstrap_checkpoint_requested(state: &AppState, user_id: &str, board_id: &str) -> Result<bool> {
    Ok(bootstrap_should_yield(
        &load_user_board(state, user_id, board_id)?.board,
    ))
}

fn bootstrap_should_yield(run: &AgenticBoard) -> bool {
    run.pause_requested
        || matches!(
            run.status.as_str(),
            "pausing" | "paused" | "cancelled" | "failed" | "blocked" | "completed"
        )
}

async fn perform_codebase_recon(state: &AppState, user_id: &str, board_id: &str) -> Result<Value> {
    let stored = load_user_board(state, user_id, board_id)?;
    let local_snapshot = local_codebase_snapshot(&stored.board);
    let prompt = build_codebase_recon_prompt(&stored.board, &local_snapshot);
    let parsed = execute_internal_prompt(state, user_id, board_id, "codebase recon", &prompt)
        .await
        .ok()
        .and_then(|text| parse_json_object(&text))
        .unwrap_or_else(|| json!({}));
    Ok(json!({
        "localSnapshot": local_snapshot,
        "summary": parsed.get("summary").and_then(Value::as_str).unwrap_or("Static codebase snapshot; task sessions inspect relevant files directly."),
        "architecture": normalize_string_list(parsed.get("architecture")),
        "implementedCapabilities": normalize_string_list(parsed.get("implementedCapabilities")),
        "missingCapabilities": normalize_string_list(parsed.get("missingCapabilities")),
        "conventions": normalize_string_list(parsed.get("conventions")),
        "runCommands": normalize_string_list(parsed.get("runCommands")),
        "testCommands": normalize_string_list(parsed.get("testCommands")),
        "relevantFiles": normalize_string_list(parsed.get("relevantFiles")),
        "risks": normalize_string_list(parsed.get("risks")),
        "completedAt": Utc::now(),
    }))
}

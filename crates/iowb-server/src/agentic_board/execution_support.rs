#[derive(Debug, Default)]
struct Bundle {
    references: Vec<Value>,
    manifest: Vec<Value>,
    chunks: Vec<Value>,
}

async fn execute_internal_prompt(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    label: &str,
    prompt: &str,
) -> Result<String> {
    let mut stored = load_user_board(state, user_id, board_id)?;
    let provider = effective_provider_for_phase(&stored.board, label)?;
    let model = effective_model_for_phase(&stored.board, label);
    let reusable_session_id = reusable_session_id_for_provider(&stored.board, &provider);
    stored.board.provider_call_started_at = Some(Utc::now());
    stored.board.provider_call_label = Some(label.to_string());
    stored.board.current_provider_session_id = reusable_session_id;
    let execution_model = agentic_execution_model_for_provider(&provider, &model);
    let mut telemetry = json!({
        "phase": stored.board.current_phase,
        "label": label,
        "provider": provider,
        "model": model,
        "chars": prompt.chars().count(),
        "estimatedTokens": estimate_tokens(prompt),
        "startedAt": Utc::now(),
    });
    if execution_model != model {
        telemetry["executionModel"] = json!(execution_model);
    }
    stored.board.prompt_telemetry.push(telemetry);
    let telemetry_index = stored.board.prompt_telemetry.len().saturating_sub(1);
    stored.board.touch();
    save_board(state, &stored.board)?;

    let result = execute_provider_prompt(state, &stored.board, label, prompt).await;
    let mut stored = load_user_board(state, user_id, board_id)?;
    stored.board.provider_call_started_at = None;
    stored.board.provider_call_label = None;
    stored.board.current_provider_session_id = None;
    match &result {
        Ok(output) => {
            finalize_prompt_telemetry(
                &mut stored.board,
                telemetry_index,
                output.session_id.as_deref(),
                output.effective_model.as_deref(),
                output.token_usage.as_ref(),
            );
            increment_provider_usage(
                &mut stored.board,
                prompt,
                &output.output,
                output.session_id.as_deref(),
                output.token_usage.as_ref(),
            );
            stored
                .board
                .append_log(format!("Internal provider call completed: {label}"));
        }
        Err(error) => {
            stored.board.append_log(format!(
                "Internal provider call failed for {label}: {}",
                server_error_message(error)
            ));
        }
    }
    stored.board.touch();
    save_board(state, &stored.board)?;
    result.map(|output| output.output)
}

async fn execute_provider_prompt(
    state: &AppState,
    run: &AgenticBoard,
    label: &str,
    prompt: &str,
) -> Result<ProviderPromptResult> {
    let provider = effective_provider_for_phase(run, label)?;
    let model = effective_model_for_phase(run, label);
    let execution_model = agentic_execution_model_for_provider(&provider, &model);
    let result = execute_shared_provider_turn(
        state,
        run,
        &provider,
        &execution_model,
        prompt,
        reusable_session_id_for_provider(run, &provider).as_deref(),
        board_task_id_for_label(run, label).as_deref(),
    )
    .await?;
    if result.exit_code == 0 {
        return Ok(ProviderPromptResult {
            output: result.assistant_text,
            session_id: Some(result.session_id),
            token_usage: result.token_usage,
            effective_model: if execution_model.trim().is_empty() {
                None
            } else {
                Some(execution_model)
            },
        });
    }
    Err(ServerError::with_details(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("provider call failed during {label}"),
        result.summary,
    ))
}

fn finalize_prompt_telemetry(
    run: &mut AgenticBoard,
    telemetry_index: usize,
    session_id: Option<&str>,
    effective_model: Option<&str>,
    token_usage: Option<&Value>,
) {
    let Some(entry) = run
        .prompt_telemetry
        .get_mut(telemetry_index)
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) {
        entry.insert("sessionId".to_string(), json!(session_id));
    }
    if let Some(model) = effective_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        entry.insert("effectiveModel".to_string(), json!(model));
        entry.insert("model".to_string(), json!(model));
        run.last_effective_model = Some(model.to_string());
    }
    if let Some(usage) = token_usage {
        entry.insert("tokenUsage".to_string(), usage.clone());
        entry.insert(
            "actualInputTokens".to_string(),
            json!(
                usage
                    .get("inputTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ),
        );
        entry.insert(
            "actualCachedInputTokens".to_string(),
            json!(
                usage
                    .get("cachedInputTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ),
        );
        entry.insert(
            "actualOutputTokens".to_string(),
            json!(
                usage
                    .get("outputTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ),
        );
        entry.insert(
            "actualTotalTokens".to_string(),
            json!(
                usage
                    .get("totalTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ),
        );
    }
    entry.insert("completedAt".to_string(), json!(Utc::now()));
}

fn build_source_bundle(project_path: &str, source_prompt: &str) -> Bundle {
    let references = resolve_source_references(project_path, source_prompt);
    let mut files = BTreeMap::<PathBuf, Vec<String>>::new();
    for reference in &references {
        if let Some(path) = reference.get("absolutePath").and_then(Value::as_str) {
            let reason = reference
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("prompt-reference")
                .to_string();
            collect_text_files(
                Path::new(path),
                Path::new(project_path),
                MAX_SOURCE_FILES,
                &mut files,
                reason,
            );
        }
    }
    let mut bundle = Bundle {
        references,
        ..Bundle::default()
    };
    let mut chunk_counter = 1usize;
    for (absolute, reasons) in files {
        let relative = relative_display(Path::new(project_path), &absolute);
        match fs::read_to_string(&absolute) {
            Ok(content) => {
                let chunks = split_into_chunks(
                    &relative,
                    &content,
                    "SRC",
                    &mut chunk_counter,
                    SOURCE_CHUNK_TARGET_LENGTH,
                );
                let chunk_ids = chunks
                    .iter()
                    .filter_map(|chunk| chunk.get("id").and_then(Value::as_str).map(str::to_string))
                    .collect::<Vec<_>>();
                bundle.chunks.extend(chunks);
                let metadata = fs::metadata(&absolute).ok();
                bundle.manifest.push(json!({
                    "path": relative,
                    "size": metadata.as_ref().map(|meta| meta.len()).unwrap_or(0),
                    "mtime": metadata.and_then(|meta| meta.modified().ok()).map(DateTime::<Utc>::from),
                    "sha256": sha256_hex(content.as_bytes()),
                    "reasons": reasons,
                    "status": "loaded",
                    "chunkIds": chunk_ids,
                }));
            }
            Err(error) => bundle.manifest.push(json!({
                "path": relative,
                "reasons": reasons,
                "status": "unreadable",
                "error": error.to_string(),
                "chunkIds": [],
            })),
        }
    }
    bundle
}

fn build_codebase_bundle(project_path: &str) -> Bundle {
    let root = Path::new(project_path);
    let mut bundle = Bundle::default();
    let mut chunk_counter = 1usize;
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !should_skip_path(entry.path(), root))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .take(MAX_CODEBASE_FILES)
    {
        let absolute = entry.path().to_path_buf();
        let relative = relative_display(root, &absolute);
        let metadata = fs::metadata(&absolute).ok();
        match fs::read(&absolute) {
            Ok(bytes) => {
                let textual = looks_textual(&bytes) && should_chunk_codebase_file(&relative);
                let mut chunk_ids = Vec::new();
                if textual && bundle.chunks.len() < MAX_CODEBASE_CHUNKS {
                    let content = String::from_utf8_lossy(&bytes).to_string();
                    let chunks = split_into_chunks(
                        &relative,
                        &content,
                        "CDB",
                        &mut chunk_counter,
                        CODEBASE_CHUNK_TARGET_LENGTH,
                    );
                    chunk_ids = chunks
                        .iter()
                        .filter_map(|chunk| {
                            chunk.get("id").and_then(Value::as_str).map(str::to_string)
                        })
                        .collect();
                    bundle.chunks.extend(
                        chunks
                            .into_iter()
                            .take(MAX_CODEBASE_CHUNKS - bundle.chunks.len()),
                    );
                }
                bundle.manifest.push(json!({
                    "path": relative,
                    "size": metadata.as_ref().map(|meta| meta.len()).unwrap_or(bytes.len() as u64),
                    "mtime": metadata.and_then(|meta| meta.modified().ok()).map(DateTime::<Utc>::from),
                    "sha256": sha256_hex(&bytes),
                    "textual": textual,
                    "status": "loaded",
                    "aiUnderstandingSkipped": !textual,
                    "skipReason": if textual { "" } else { "binary, generated, dependency, or oversized artifact" },
                    "chunkIds": chunk_ids,
                }));
            }
            Err(error) => bundle.manifest.push(json!({
                "path": relative,
                "textual": false,
                "status": "unreadable",
                "error": error.to_string(),
                "chunkIds": [],
            })),
        }
    }
    bundle
}

fn resolve_source_references(project_path: &str, prompt: &str) -> Vec<Value> {
    let root = Path::new(project_path);
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut references = Vec::new();
    let mut seen = BTreeSet::<PathBuf>::new();
    for cleaned in prompt_source_locator_candidates(prompt) {
        let candidate = if Path::new(&cleaned).is_absolute() {
            PathBuf::from(&cleaned)
        } else {
            root.join(&cleaned)
        };
        let resolved = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.clone());
        if candidate.exists()
            && resolved.starts_with(&canonical_root)
            && seen.insert(resolved.clone())
        {
            references.push(json!({
                "matchedFrom": cleaned,
                "path": relative_display(&canonical_root, &resolved),
                "absolutePath": resolved,
                "reason": "prompt-reference",
            }));
        }
    }
    references
}

fn prompt_has_explicit_source_locator(run: &AgenticBoard) -> bool {
    !prompt_source_locator_candidates(&active_board_prompt(run)).is_empty()
}

fn prompt_source_locator_candidates(prompt: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    prompt
        .split_whitespace()
        .filter_map(normalize_prompt_source_token)
        .filter(|token| is_prompt_source_locator(token))
        .filter(|token| seen.insert(token.clone()))
        .collect()
}

fn normalize_prompt_source_token(token: &str) -> Option<String> {
    let mut value = token
        .trim_matches(|ch: char| {
            matches!(
                ch,
                '"' | '\'' | '`' | ',' | ':' | ';' | ')' | '(' | '[' | ']' | '<' | '>'
            )
        })
        .trim()
        .to_string();
    if value.ends_with('.') && value.matches('.').count() > 1 {
        value.pop();
    }
    if let Some((prefix, suffix)) = value.rsplit_once(':') {
        if !prefix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
            value = prefix.to_string();
        }
    }
    if let Some((prefix, suffix)) = value.rsplit_once("#L") {
        if !prefix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
            value = prefix.to_string();
        }
    }
    (!value.is_empty()).then_some(value)
}

fn is_prompt_source_locator(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return false;
    }
    if matches!(
        lower.as_str(),
        "agents.md" | "readme.md" | "package.json" | "cargo.toml" | "pyproject.toml"
    ) {
        return true;
    }
    if token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('/')
        || token.contains('/')
        || token.contains('\\')
    {
        return true;
    }
    let Some(extension) = Path::new(token).extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "rs" | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "json"
            | "md"
            | "toml"
            | "yaml"
            | "yml"
            | "html"
            | "css"
            | "scss"
            | "kt"
            | "kts"
            | "java"
            | "swift"
            | "go"
            | "py"
            | "rb"
            | "php"
            | "cs"
            | "cpp"
            | "c"
            | "h"
            | "hpp"
            | "sql"
            | "sh"
            | "bash"
            | "zsh"
            | "env"
    )
}

fn collect_text_files(
    candidate: &Path,
    root: &Path,
    limit: usize,
    files: &mut BTreeMap<PathBuf, Vec<String>>,
    reason: String,
) {
    if files.len() >= limit || should_skip_path(candidate, root) {
        return;
    }
    if candidate.is_file() {
        if is_candidate_text_path(candidate) {
            files
                .entry(candidate.to_path_buf())
                .or_default()
                .push(reason);
        }
        return;
    }
    if candidate.is_dir() {
        for entry in WalkDir::new(candidate)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !should_skip_path(entry.path(), root))
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
        {
            if files.len() >= limit {
                break;
            }
            if is_candidate_text_path(entry.path()) {
                files
                    .entry(entry.path().to_path_buf())
                    .or_default()
                    .push(reason.clone());
            }
        }
    }
}

fn split_into_chunks(
    path: &str,
    content: &str,
    prefix: &str,
    counter: &mut usize,
    target_len: usize,
) -> Vec<Value> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut start_line = 1usize;
    let mut line_number = 0usize;
    for line in content.lines() {
        line_number += 1;
        if current.is_empty() {
            start_line = line_number;
        }
        current.push_str(line);
        current.push('\n');
        if current.len() >= target_len || current.len() >= SOURCE_CHUNK_MAX_LENGTH {
            chunks.push(json!({
                "id": format!("{prefix}-{:04}", *counter),
                "path": path,
                "chunkIndex": chunks.len() + 1,
                "startLine": start_line,
                "endLine": line_number,
                "content": current,
            }));
            *counter += 1;
            current = String::new();
        }
    }
    if !current.trim().is_empty() {
        chunks.push(json!({
            "id": format!("{prefix}-{:04}", *counter),
            "path": path,
            "chunkIndex": chunks.len() + 1,
            "startLine": start_line,
            "endLine": line_number,
            "content": current,
        }));
        *counter += 1;
    }
    chunks
}

#[derive(Debug)]
struct ProviderTaskResult {
    stderr: String,
    assistant_text: String,
    stream_events: Vec<Value>,
    errors: Vec<String>,
    session_id: Option<String>,
    token_usage: Option<Value>,
    exit_code: i32,
    summary: String,
}

#[derive(Debug)]
struct ProviderFallbackSelection {
    provider: String,
    model: String,
    reason: String,
}

struct ProviderExecutionAttempt {
    result: Result<ProviderTaskResult>,
    fallback: Option<ProviderFallbackSelection>,
}

async fn ensure_tdd_baseline_for_task(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    run: &mut AgenticBoard,
    task_index: usize,
) -> Result<bool> {
    let Some(task) = run.tasks.get(task_index).cloned() else {
        return Ok(true);
    };
    if !task_requires_tdd(run, &task) {
        if let Some(task) = run.tasks.get_mut(task_index) {
            task.tdd_phase = "disabled".to_string();
        }
        return Ok(true);
    }
    if tdd_baseline_is_ready(&task) {
        let max_fix_attempts = max_tdd_fix_attempts(run);
        if task.fix_attempts >= max_fix_attempts {
            if let Some(task) = run.tasks.get_mut(task_index) {
                task.status = "blocked".to_string();
                task.tdd_phase = "blocked".to_string();
                task.error = Some(format!(
                    "TDD max fix attempts reached ({max_fix_attempts})."
                ));
                task.completed_at = Some(Utc::now());
            }
            run.append_log(format!(
                "Blocked {} after reaching TDD max fix attempts",
                task.id
            ));
            return Ok(false);
        }
        if let Some(task) = run.tasks.get_mut(task_index) {
            task.tdd_phase = "dev_pending".to_string();
        }
        return Ok(true);
    }

    set_phase(
        run,
        "tdd_qa_generation",
        json!({ "taskId": task.id, "taskTitle": task.title }),
    );
    if let Some(task) = run.tasks.get_mut(task_index) {
        task.tdd_phase = "qa_generating".to_string();
        task.transcript.push(json!({
            "timestamp": Utc::now(),
            "kind": "status",
            "status": "qa_generating",
            "content": "Generating failing QA tests before implementation",
        }));
        task.transcript_updated_at = Some(Utc::now());
    }
    attach_rag_context_for_task(run, task_index).await;
    run.touch();
    save_board(state, run)?;
    if let Ok(stored) = load_user_board(state, user_id, board_id)
        && (stored.board.status == "paused"
            || stored.board.pause_requested
            || stored.board.status == "cancelled")
    {
        *run = stored.board;
        return Ok(false);
    }

    let prompt = build_qa_generation_prompt(run, &task, task_index);
    let before_workspace = capture_workspace_snapshot(&run.project_path);
    let output = execute_internal_prompt(
        state,
        user_id,
        board_id,
        &format!("tdd qa generation for {}", task.id),
        &prompt,
    )
    .await;

    if let Ok(stored) = load_user_board(state, user_id, board_id) {
        *run = stored.board;
    }
    let task_index = run
        .tasks
        .iter()
        .position(|candidate| candidate.id == task.id)
        .unwrap_or(task_index);
    let now = Utc::now();
    let mut provider_failure: Option<String> = None;
    let mut malformed_response: Option<String> = None;
    let parsed = match output {
        Ok(text) => match parse_json_object(&text) {
            Some(parsed) => parsed,
            None => {
                let excerpt = limit_text(&text, 1200);
                malformed_response = Some(excerpt.clone());
                json!({
                    "status": "malformed_response",
                    "summary": "QA generation did not return the required JSON contract.",
                    "testFiles": [],
                    "commands": [],
                    "notes": [excerpt],
                })
            }
        },
        Err(error) => {
            let message = server_error_message(&error);
            provider_failure = Some(message.clone());
            json!({
                "status": "provider_failed",
                "summary": message,
                "testFiles": [],
                "commands": [],
                "notes": [],
            })
        }
    };
    let test_files = normalize_string_list(
        parsed
            .get("testFiles")
            .or_else(|| parsed.get("qaTestPaths"))
            .or_else(|| parsed.get("changedFiles")),
    );
    let commands = normalize_string_list(
        parsed
            .get("commands")
            .or_else(|| parsed.get("testCommands"))
            .or_else(|| parsed.get("qaTestCommands")),
    );
    let workspace_delta = record_task_workspace_changes(run, &task.id, before_workspace);
    let allow_without_tests = tdd_allows_implementation_without_tests(run);
    let require_failing_baseline = tdd_requires_failing_baseline(run);
    let commands_empty = commands.is_empty();
    let baseline = if commands.is_empty() {
        json!({
            "stage": "qa_baseline",
            "taskId": task.id,
            "startedAt": now,
            "completedAt": Utc::now(),
            "passed": allow_without_tests,
            "commands": [],
            "blocked": !allow_without_tests,
            "skipped": allow_without_tests,
            "summary": if allow_without_tests {
                "QA generation returned no test commands; TDD policy allows implementation without generated tests."
            } else {
                "QA generation returned no test commands."
            },
        })
    } else {
        run_generated_test_commands(
            &run.project_path,
            &task.id,
            &commands,
            "qa_baseline",
            validation_timeout(run),
        )
        .await
    };
    let qa_generation_done = parsed_status_done(Some(&parsed));
    let baseline_failed = qa_generation_done && validation_has_failure(&baseline);
    let baseline_allowed_without_failure =
        !require_failing_baseline && !commands_empty && qa_generation_done;
    let implementation_allowed_without_tests = commands_empty && allow_without_tests;
    run.validation_runs.push(baseline.clone());
    run.qa_artifacts.push(json!({
        "taskId": task.id,
        "generatedAt": now,
        "testFiles": test_files,
        "commands": commands,
        "baseline": baseline,
        "workspaceDelta": workspace_delta,
        "qaResult": parsed,
    }));

    let task_id_for_log = task.id.clone();
    let outcome = if let Some(task) = run.tasks.get_mut(task_index) {
        task.qa_test_paths = test_files;
        task.qa_test_commands = commands;
        task.qa_baseline_validation = Some(baseline.clone());
        task.coverage_evidence.push(json!({
            "kind": "qa_baseline",
            "validation": baseline,
            "recordedAt": Utc::now(),
        }));
        task.transcript.push(json!({
            "timestamp": Utc::now(),
            "kind": "tdd_qa_result",
            "content": parsed,
        }));
        task.transcript_updated_at = Some(Utc::now());
        if baseline_failed
            || baseline_allowed_without_failure
            || implementation_allowed_without_tests
        {
            task.status = "in_progress".to_string();
            task.tdd_phase = if baseline_failed {
                "qa_failed_expected".to_string()
            } else if implementation_allowed_without_tests {
                "qa_skipped_allowed".to_string()
            } else {
                "qa_baseline_not_required".to_string()
            };
            task.error = None;
            true
        } else {
            task.status = "blocked".to_string();
            task.tdd_phase = "qa_needs_review".to_string();
            task.qa_passed = Some(false);
            task.error = Some(if let Some(message) = provider_failure.as_deref() {
                format!("QA generation provider call failed: {message}")
            } else if let Some(excerpt) = malformed_response.as_deref() {
                format!(
                    "QA generation returned malformed JSON instead of the required test contract: {}",
                    limit_text(excerpt, 500)
                )
            } else if commands_empty {
                "QA generation returned no test commands and TDD policy does not allow implementation without tests."
                    .to_string()
            } else {
                "Generated QA tests did not fail before implementation; tests may be weak or feature already exists."
                    .to_string()
            });
            task.completed_at = Some(Utc::now());
            false
        }
    } else {
        false
    };
    if outcome {
        run.append_log(format!(
            "TDD baseline accepted for {task_id_for_log}; implementation may start"
        ));
    } else if let Some(message) = provider_failure {
        run.append_log(format!(
            "Blocked {task_id_for_log} because QA provider call failed: {}",
            limit_text(&message, 300)
        ));
    } else {
        run.append_log(format!(
            "Blocked {task_id_for_log} because generated QA tests passed before implementation"
        ));
    }
    Ok(outcome)
}

fn task_requires_tdd(run: &AgenticBoard, task: &BoardTask) -> bool {
    run.tdd_enabled
        && !uses_hierarchical_orchestration(run)
        && !is_qa_task(task)
        && !task.agents_knowledge_task
        && !task.internal_validation
        && !task.backlog_generation_task
        && !task.qa_fix_task
        && matches!(task.task_type.as_str(), "implementation" | "feature")
}

fn tdd_baseline_is_ready(task: &BoardTask) -> bool {
    !task.qa_test_commands.is_empty()
        && task
            .qa_baseline_validation
            .as_ref()
            .is_some_and(validation_has_failure)
}

fn validation_has_failure(validation: &Value) -> bool {
    validation
        .get("commands")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|command| command.get("exitCode").and_then(Value::as_i64).unwrap_or(0) != 0)
}

fn max_tdd_fix_attempts(run: &AgenticBoard) -> u32 {
    run.tdd_policy
        .get("maxFixAttempts")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(3)
}

fn tdd_requires_failing_baseline(run: &AgenticBoard) -> bool {
    run.tdd_policy
        .get("requireFailingTestBeforeDev")
        .and_then(value_as_bool)
        .unwrap_or(true)
}

fn tdd_allows_implementation_without_tests(run: &AgenticBoard) -> bool {
    run.tdd_policy
        .get("allowImplementationWithoutTests")
        .and_then(value_as_bool)
        .unwrap_or(false)
}

fn max_followups_per_group(run: &AgenticBoard) -> usize {
    run.qa_policy
        .get("maxFollowupsPerGroup")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(MAX_FOLLOWUP_TASKS_PER_GROUP)
}

fn max_task_attempts(run: &AgenticBoard) -> u32 {
    run.qa_policy
        .get("maxTaskAttempts")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(MAX_TASK_ATTEMPTS)
}

async fn index_project_for_rag(run: &mut AgenticBoard) {
    if !rag_enabled_from_settings(&run.rag_settings) {
        run.rag_enabled = false;
        return;
    }
    if run
        .rag_settings
        .get("indexOnBootstrap")
        .and_then(value_as_bool)
        == Some(false)
    {
        run.rag_enabled = true;
        return;
    }
    let Some(client) = board_rag_client(run) else {
        return;
    };
    let project_id = rag_project_id(run);
    record_rag_trace_ref(run, None, "project_index", &project_id);
    let request = ProjectIndexRequest {
        project_id,
        project_path: run.project_path.clone(),
        run_id: Some(run.id.clone()),
        include_globs: Vec::new(),
        exclude_globs: Vec::new(),
    };
    let record = match client.index_project(&request).await {
        Ok(response) => json!({
            "kind": "project_index",
            "ingestedAt": Utc::now(),
            "ok": true,
            "response": response,
        }),
        Err(error) => json!({
            "kind": "project_index",
            "ingestedAt": Utc::now(),
            "ok": false,
            "error": error_record(error),
        }),
    };
    run.rag_ingestions.push(record);
    trim_rag_history(run);
}

async fn attach_rag_context_for_task(run: &mut AgenticBoard, task_index: usize) {
    let Some(task) = run.tasks.get(task_index).cloned() else {
        return;
    };
    if !run
        .rag_settings
        .get("queryEnabled")
        .and_then(value_as_bool)
        .unwrap_or(true)
    {
        return;
    }
    let Some(client) = board_rag_client(run) else {
        return;
    };
    let phase = rag_phase_for_task(&task);
    let project_id = rag_project_id(run);
    let context_max_chars = rag_context_max_chars(run);
    record_rag_trace_ref(run, Some(&task.id), "query", &project_id);
    let request = RagQueryRequest {
        project_id,
        run_id: run.id.clone(),
        task_id: task.id.clone(),
        phase,
        query: rag_task_query(&task),
        known_files: rag_known_files(&task),
        validation_error: task.deterministic_validation.clone(),
        scopes: rag_scopes(run),
    };
    match client.query(&request).await {
        Ok(response) => {
            let response_value = serde_json::to_value(&response).unwrap_or_else(|_| json!({}));
            run.rag_queries.push(json!({
                "taskId": task.id.clone(),
                "phase": request.phase,
                "queriedAt": Utc::now(),
                "ok": true,
                "contextCount": response.context_refs.len(),
                "response": response_value,
            }));
            if let Some(task) = run.tasks.get_mut(task_index) {
                task.rag_context_refs = json!(response.context_refs)
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                task.rag_prompt_context = limit_text(&response.prompt_context, context_max_chars);
                task.transcript.push(json!({
                    "timestamp": Utc::now(),
                    "kind": "rag_context",
                    "content": format!("Loaded {} RAG context reference(s)", task.rag_context_refs.len()),
                    "contextRefs": task.rag_context_refs.clone(),
                }));
                task.transcript_updated_at = Some(Utc::now());
            }
        }
        Err(error) => {
            let record = error_record(error);
            run.rag_queries.push(json!({
                "taskId": task.id.clone(),
                "phase": request.phase,
                "queriedAt": Utc::now(),
                "ok": false,
                "error": record,
            }));
            run.append_log("RAG context unavailable; continuing without retrieval");
        }
    }
    trim_rag_history(run);
}

async fn ingest_rag_task_outcome(run: &mut AgenticBoard, task_id: &str, parsed: &Value) {
    let ingest_task_results = run
        .rag_settings
        .get("ingestTaskResults")
        .and_then(value_as_bool)
        .unwrap_or(true);
    let ingest_validation_errors = run
        .rag_settings
        .get("ingestValidationErrors")
        .and_then(value_as_bool)
        .unwrap_or(true);
    if !ingest_task_results && !ingest_validation_errors {
        return;
    }
    let Some(client) = board_rag_client(run) else {
        return;
    };
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id).cloned() else {
        return;
    };
    let project_id = rag_project_id(run);
    if ingest_task_results && parsed_status_done(Some(parsed)) {
        record_rag_trace_ref(run, Some(&task.id), "task_result", &project_id);
        let request = TaskResultIngestRequest {
            project_id: project_id.clone(),
            run_id: run.id.clone(),
            task_id: task.id.clone(),
            changed_files: task.changed_files.clone(),
            test_files: task
                .changed_files
                .iter()
                .filter(|path| path.to_lowercase().contains("test"))
                .cloned()
                .collect(),
            commands: task.commands_run.clone(),
            validation: task.deterministic_validation.clone().unwrap_or(Value::Null),
            summary: task.summary.clone(),
        };
        let record = match client.ingest_task_result(&request).await {
            Ok(response) => json!({
                "taskId": task.id.clone(),
                "kind": "task_result",
                "ingestedAt": Utc::now(),
                "ok": true,
                "response": response,
            }),
            Err(error) => json!({
                "taskId": task.id.clone(),
                "kind": "task_result",
                "ingestedAt": Utc::now(),
                "ok": false,
                "error": error_record(error),
            }),
        };
        run.rag_ingestions.push(record);
        run.promotion_candidates.push(json!({
            "scope": "project_specific",
            "projectId": rag_project_id(run),
            "taskId": task.id.clone(),
            "title": task.title.clone(),
            "changedFiles": task.changed_files.clone(),
            "testFiles": task.qa_test_paths.clone(),
            "commands": task.commands_run.clone(),
            "validation": task.deterministic_validation.clone(),
            "summary": task.summary.clone(),
            "recordedAt": Utc::now(),
        }));
    }

    if ingest_validation_errors {
        if let Some(validation) = task.deterministic_validation.as_ref() {
            if validation.get("passed").and_then(Value::as_bool) == Some(false) {
                let (command, exit_code, output) = failed_validation_excerpt(validation);
                record_rag_trace_ref(run, Some(&task.id), "validation_error", &project_id);
                let request = ValidationErrorIngestRequest {
                    project_id: project_id.clone(),
                    run_id: run.id.clone(),
                    task_id: task.id.clone(),
                    phase: "fix".to_string(),
                    command,
                    exit_code,
                    output,
                    validation: validation.clone(),
                };
                let record = match client.ingest_validation_error(&request).await {
                    Ok(response) => json!({
                        "taskId": task.id.clone(),
                        "kind": "validation_error",
                        "ingestedAt": Utc::now(),
                        "ok": true,
                        "response": response,
                    }),
                    Err(error) => json!({
                        "taskId": task.id.clone(),
                        "kind": "validation_error",
                        "ingestedAt": Utc::now(),
                        "ok": false,
                        "error": error_record(error),
                    }),
                };
                run.rag_ingestions.push(record);
            }
        }
    }
    trim_rag_history(run);
}

async fn execute_promotion_review_task(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    run: &mut AgenticBoard,
    task_index: usize,
) -> Result<()> {
    let Some(task) = run.tasks.get(task_index).cloned() else {
        return Ok(());
    };
    let Some(client) = board_rag_client(run) else {
        mark_promotion_review_task(
            run,
            &task.id,
            json!({
                "status": "blocked",
                "summary": "RAG service is unavailable; promotion review skipped.",
                "approvedCandidateIds": [],
            }),
        );
        return Ok(());
    };
    let project_id = rag_project_id(run);
    record_rag_trace_ref(run, Some(&task.id), "promotion_candidates", &project_id);
    let candidates_request = PromotionCandidatesRequest {
        project_id: project_id.clone(),
        limit: 20,
    };
    let candidates_response = match client.promotion_candidates(&candidates_request).await {
        Ok(response) => response,
        Err(error) => {
            let record = error_record(error);
            run.rag_ingestions.push(json!({
                "taskId": task.id,
                "kind": "promotion_candidates",
                "ok": false,
                "ingestedAt": Utc::now(),
                "error": record,
            }));
            mark_promotion_review_task(
                run,
                &task.id,
                json!({
                    "status": "blocked",
                    "summary": "Failed to load promotion candidates.",
                    "approvedCandidateIds": [],
                }),
            );
            return Ok(());
        }
    };
    let candidates = candidates_response
        .get("candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if candidates.is_empty() {
        mark_promotion_review_task(
            run,
            &task.id,
            json!({
                "status": "done",
                "summary": "No RAG promotion candidates available.",
                "approvedCandidateIds": [],
            }),
        );
        return Ok(());
    }

    run.rag_ingestions.push(json!({
        "taskId": task.id,
        "kind": "promotion_candidates",
        "ok": true,
        "ingestedAt": Utc::now(),
        "candidateCount": candidates.len(),
    }));
    run.touch();
    save_board(state, run)?;

    let prompt = build_promotion_review_prompt(run, &candidates);
    let output =
        execute_internal_prompt(state, user_id, board_id, "rag promotion review", &prompt).await;
    if let Ok(stored) = load_user_board(state, user_id, board_id) {
        *run = stored.board;
    }
    let parsed = match output {
        Ok(text) => parse_json_object(&text).unwrap_or_else(|| {
            json!({
                "status": "blocked",
                "summary": "Promotion review did not return JSON.",
                "approvedCandidateIds": [],
                "notes": [limit_text(&text, 1200)],
            })
        }),
        Err(error) => json!({
            "status": "blocked",
            "summary": server_error_message(&error),
            "approvedCandidateIds": [],
        }),
    };
    let approved_ids = normalize_string_list(
        parsed
            .get("approvedCandidateIds")
            .or_else(|| parsed.get("approved_candidate_ids"))
            .or_else(|| parsed.get("candidateIds")),
    );
    let approval_response = if approved_ids.is_empty() {
        json!({
            "promoted": [],
            "reason": "No candidates approved by review gate.",
        })
    } else if let Some(client) = board_rag_client(run) {
        record_rag_trace_ref(run, Some(&task.id), "promotion_approve", &project_id);
        let request = PromotionApproveRequest {
            project_id: project_id.clone(),
            candidate_ids: approved_ids,
            reviewer: "io-workbench-promotion-review".to_string(),
            notes: parsed
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        };
        match client.approve_promotions(&request).await {
            Ok(response) => response,
            Err(error) => json!({
                "promoted": [],
                "error": error_record(error),
            }),
        }
    } else {
        json!({
            "promoted": [],
            "error": "RAG service unavailable during approval.",
        })
    };
    run.rag_ingestions.push(json!({
        "taskId": task.id,
        "kind": "promotion_approval",
        "ok": approval_response.get("error").is_none(),
        "ingestedAt": Utc::now(),
        "review": parsed,
        "response": approval_response,
    }));
    mark_promotion_review_task(run, &task.id, parsed);
    trim_rag_history(run);
    Ok(())
}

fn mark_promotion_review_task(run: &mut AgenticBoard, task_id: &str, result: Value) {
    let status_done = parsed_status_done(Some(&result));
    if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
        task.status = if status_done {
            "completed".to_string()
        } else {
            "blocked".to_string()
        };
        task.summary = result
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("Promotion review completed.")
            .to_string();
        task.result = Some(result.clone());
        task.completed_at = Some(Utc::now());
        task.qa_passed = Some(status_done);
        task.tdd_phase = if status_done {
            "done".to_string()
        } else {
            "blocked".to_string()
        };
        task.transcript.push(json!({
            "timestamp": Utc::now(),
            "kind": "promotion_review",
            "content": result,
        }));
        task.transcript_updated_at = Some(Utc::now());
    }
}

fn board_rag_client(run: &mut AgenticBoard) -> Option<RagClient> {
    if !rag_enabled_from_settings(&run.rag_settings) {
        run.rag_enabled = false;
        run.rag_service_url = RagClient::configured_descriptor();
        return None;
    }
    let Some(client_result) = RagClient::from_env() else {
        run.rag_enabled = false;
        run.rag_service_url = None;
        return None;
    };
    let client = match client_result {
        Ok(client) => client,
        Err(error) => {
            run.rag_enabled = false;
            run.rag_service_url = RagClient::configured_descriptor();
            run.rag_queries.push(json!({
                "queriedAt": Utc::now(),
                "ok": false,
                "error": error_record(error),
            }));
            return None;
        }
    };
    run.rag_enabled = true;
    run.rag_service_url = Some(client.descriptor());
    Some(client)
}

fn rag_scopes(run: &AgenticBoard) -> Vec<String> {
    let scopes = normalize_string_list(run.rag_settings.get("scopes"));
    if scopes.is_empty() {
        normalize_string_list(default_rag_settings().get("scopes"))
    } else {
        scopes
    }
}

fn rag_context_max_chars(run: &AgenticBoard) -> usize {
    run.rag_settings
        .get("contextMaxChars")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(12_000)
        .clamp(1_000, 80_000)
}

fn rag_project_id(run: &AgenticBoard) -> String {
    if run.project_name.trim().is_empty() {
        run.id.clone()
    } else {
        run.project_name.clone()
    }
}

fn rag_phase_for_task(task: &BoardTask) -> String {
    if task.final_qa_task {
        "final".to_string()
    } else if is_qa_task(task) {
        "qa".to_string()
    } else if task.tdd_phase.starts_with("qa") {
        "qa".to_string()
    } else if task.tdd_phase == "fix_pending" {
        "fix".to_string()
    } else if task.status == "failed" || task.qa_fix_task {
        "fix".to_string()
    } else {
        "dev".to_string()
    }
}

fn rag_task_query(task: &BoardTask) -> String {
    let mut parts = vec![
        task.title.clone(),
        task.details.clone(),
        task.prompt.clone(),
    ];
    parts.extend(task.acceptance_criteria.clone());
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn rag_known_files(task: &BoardTask) -> Vec<String> {
    let mut files = task.references.clone();
    files.extend(task.changed_files.clone());
    files.sort();
    files.dedup();
    files
}

fn failed_validation_excerpt(validation: &Value) -> (String, Option<i64>, String) {
    let Some(command) = validation
        .get("commands")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|command| command.get("passed").and_then(Value::as_bool) == Some(false))
    else {
        return (
            String::new(),
            None,
            limit_text(&validation.to_string(), 4_000),
        );
    };
    let command_text = command
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let exit_code = command.get("exitCode").and_then(Value::as_i64);
    let output = command
        .get("output")
        .or_else(|| command.get("stderr"))
        .or_else(|| command.get("stdout"))
        .and_then(Value::as_str)
        .map(|value| limit_text(value, 4_000))
        .unwrap_or_else(|| limit_text(&command.to_string(), 4_000));
    (command_text, exit_code, output)
}

fn record_rag_trace_ref(
    run: &mut AgenticBoard,
    task_id: Option<&str>,
    operation: &str,
    project_id: &str,
) {
    run.rag_trace_refs.push(json!({
        "operation": operation,
        "projectId": project_id,
        "runId": run.id,
        "taskId": task_id,
        "traceparent": rag_traceparent(project_id, Some(&run.id), task_id, operation),
        "recordedAt": Utc::now(),
    }));
}

fn trim_rag_history(run: &mut AgenticBoard) {
    if run.rag_queries.len() > 100 {
        let remove_count = run.rag_queries.len() - 100;
        run.rag_queries.drain(0..remove_count);
    }
    if run.rag_ingestions.len() > 100 {
        let remove_count = run.rag_ingestions.len() - 100;
        run.rag_ingestions.drain(0..remove_count);
    }
    if run.qa_artifacts.len() > 100 {
        let remove_count = run.qa_artifacts.len() - 100;
        run.qa_artifacts.drain(0..remove_count);
    }
    if run.promotion_candidates.len() > 100 {
        let remove_count = run.promotion_candidates.len() - 100;
        run.promotion_candidates.drain(0..remove_count);
    }
    if run.rag_trace_refs.len() > 100 {
        let remove_count = run.rag_trace_refs.len() - 100;
        run.rag_trace_refs.drain(0..remove_count);
    }
}

fn capture_workspace_snapshot(project_path: &str) -> Value {
    let output = std::process::Command::new("git")
        .arg("status")
        .arg("--short")
        .current_dir(project_path)
        .env("PATH", augmented_user_path())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let mut files = Vec::new();
            for line in String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
            {
                let status = line.get(..2).unwrap_or("").trim();
                let path = line.get(3..).unwrap_or(line).trim();
                extend_workspace_status_entries(project_path, status, path, &mut files);
                if files.len() >= MAX_WORKSPACE_SNAPSHOT_FILES {
                    break;
                }
            }
            if files.len() < MAX_WORKSPACE_SNAPSHOT_FILES {
                if let Ok(tracked_output) = std::process::Command::new("git")
                    .arg("ls-files")
                    .arg("-z")
                    .current_dir(project_path)
                    .env("PATH", augmented_user_path())
                    .output()
                {
                    if tracked_output.status.success() {
                        for path in String::from_utf8_lossy(&tracked_output.stdout)
                            .split('\0')
                            .map(str::trim)
                            .filter(|path| !path.is_empty())
                        {
                            if files
                                .iter()
                                .any(|file| file.get("path").and_then(Value::as_str) == Some(path))
                            {
                                continue;
                            }
                            files.push(json!({
                                "status": "clean",
                                "path": path,
                                "hash": hash_workspace_file(project_path, path),
                            }));
                            if files.len() >= MAX_WORKSPACE_SNAPSHOT_FILES {
                                break;
                            }
                        }
                    }
                }
            }
            let files_by_path = files
                .iter()
                .filter_map(|file| {
                    file.get("path")
                        .and_then(Value::as_str)
                        .map(|path| (path.to_string(), file.clone()))
                })
                .collect::<serde_json::Map<_, _>>();
            json!({
                "provider": "git status --short",
                "isGit": true,
                "files": files,
                "filesByPath": files_by_path,
                "shortStat": git_command_text(project_path, &["diff", "--shortstat"]),
                "stagedShortStat": git_command_text(project_path, &["diff", "--cached", "--shortstat"]),
                "capturedAt": Utc::now(),
            })
        }
        Ok(output) => json!({
            "provider": "git status --short",
            "isGit": false,
            "files": [],
            "filesByPath": {},
            "error": String::from_utf8_lossy(&output.stderr).trim(),
            "capturedAt": Utc::now(),
        }),
        Err(error) => json!({
            "provider": "git status --short",
            "isGit": false,
            "files": [],
            "filesByPath": {},
            "error": error.to_string(),
            "capturedAt": Utc::now(),
        }),
    }
}

fn extend_workspace_status_entries(
    project_path: &str,
    status: &str,
    path: &str,
    files: &mut Vec<Value>,
) {
    let clean_path = workspace_status_clean_path(path);
    let root = Path::new(project_path);
    let absolute = root.join(&clean_path);
    if absolute.is_dir() {
        let remaining = MAX_WORKSPACE_SNAPSHOT_FILES.saturating_sub(files.len());
        for entry in WalkDir::new(&absolute)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !should_skip_path(entry.path(), root))
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .take(remaining)
        {
            let path = relative_display(root, entry.path());
            files.push(json!({
                "status": status,
                "path": path,
                "hash": hash_workspace_file(project_path, &path),
            }));
        }
        return;
    }
    files.push(json!({
        "status": status,
        "path": clean_path,
        "hash": hash_workspace_file(project_path, &clean_path),
    }));
}

fn workspace_status_clean_path(path: &str) -> String {
    path.split(" -> ")
        .last()
        .unwrap_or(path)
        .trim()
        .trim_matches('"')
        .to_string()
}

fn hash_workspace_file(project_path: &str, path: &str) -> Option<String> {
    let clean_path = workspace_status_clean_path(path);
    let absolute = Path::new(project_path).join(clean_path);
    fs::read(absolute).ok().map(|bytes| sha256_hex(&bytes))
}

fn git_command_text(project_path: &str, args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .current_dir(project_path)
        .env("PATH", augmented_user_path())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

fn read_agents_context(project_path: &str) -> Value {
    let root = Path::new(project_path);
    let files = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !should_skip_path(entry.path(), root))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "AGENTS.md")
        .take(20)
        .filter_map(|entry| {
            fs::read_to_string(entry.path()).ok().map(|content| {
                json!({
                    "path": relative_display(root, entry.path()),
                    "content": limit_text(&content, 12_000),
                    "sha256": sha256_hex(content.as_bytes()),
                })
            })
        })
        .collect::<Vec<_>>();
    json!({
        "files": files,
        "loadedAt": Utc::now(),
    })
}

fn local_codebase_snapshot(run: &AgenticBoard) -> Value {
    let files = run
        .codebase_manifest
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    let config_files = files
        .iter()
        .filter(|file| is_config_file(file))
        .cloned()
        .collect::<Vec<_>>();
    let top_level = files
        .iter()
        .filter_map(|file| {
            file.split('/')
                .next()
                .filter(|part| !part.is_empty())
                .map(str::to_string)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let package_json = fs::read_to_string(Path::new(&run.project_path).join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    json!({
        "packageManager": package_json.as_ref().and_then(|value| value.get("packageManager")).and_then(Value::as_str).unwrap_or(if package_json.is_some() { "npm-compatible" } else { "" }),
        "scripts": package_json.as_ref().and_then(|value| value.get("scripts")).cloned().unwrap_or_else(|| json!({})),
        "dependencies": merge_dependencies(package_json.as_ref()),
        "topLevel": top_level,
        "configFiles": config_files,
        "fileCount": files.len(),
        "files": files.into_iter().take(500).collect::<Vec<_>>(),
        "fileListTruncated": run.codebase_manifest.len() > 500,
    })
}

fn environment_from_codebase_map(codebase_map: &Value) -> Value {
    json!({
        "runCommands": normalize_string_list(codebase_map.get("runCommands")),
        "testCommands": normalize_string_list(codebase_map.get("testCommands")),
        "packageManager": codebase_map
            .get("localSnapshot")
            .and_then(|value| value.get("packageManager"))
            .and_then(Value::as_str)
            .unwrap_or(""),
        "updatedAt": Utc::now(),
    })
}

fn merge_dependencies(package_json: Option<&Value>) -> Value {
    let mut map = serde_json::Map::new();
    for key in ["dependencies", "devDependencies"] {
        if let Some(object) = package_json
            .and_then(|value| value.get(key))
            .and_then(Value::as_object)
        {
            for (name, version) in object {
                map.insert(name.clone(), version.clone());
            }
        }
    }
    Value::Object(map)
}

fn should_skip_path(path: &Path, root: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(
            name.as_ref(),
            ".git"
                | "node_modules"
                | "target"
                | "dist"
                | "dist-server"
                | "build"
                | "coverage"
                | ".next"
                | ".nuxt"
                | ".gradle"
                | ".idea"
                | ".DS_Store"
        )
    })
}

fn should_chunk_codebase_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".lock")
        || lower.ends_with("package-lock.json")
        || lower.ends_with("yarn.lock")
        || lower.ends_with("pnpm-lock.yaml")
        || lower.ends_with(".min.js")
        || lower.ends_with(".map")
        || lower.contains("/generated/")
    {
        return false;
    }
    is_candidate_text_path(Path::new(path))
}

fn is_candidate_text_path(path: &Path) -> bool {
    let Some(ext) = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
    else {
        return path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "Dockerfile" | "Makefile" | "AGENTS.md"));
    };
    matches!(
        ext.as_str(),
        "rs" | "kt"
            | "kts"
            | "java"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "mjs"
            | "cjs"
            | "json"
            | "jsonc"
            | "toml"
            | "yaml"
            | "yml"
            | "md"
            | "html"
            | "css"
            | "scss"
            | "xml"
            | "gradle"
            | "properties"
            | "sh"
            | "py"
            | "go"
            | "sql"
            | "txt"
            | "env"
            | "sample"
            | "swift"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
    )
}

fn is_config_file(path: &str) -> bool {
    let file = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    matches!(
        file,
        "package.json"
            | "tsconfig.json"
            | "vite.config.ts"
            | "vite.config.js"
            | "next.config.js"
            | "Cargo.toml"
            | "pyproject.toml"
            | "requirements.txt"
            | "go.mod"
            | "pom.xml"
            | "build.gradle"
            | "settings.gradle"
            | "Dockerfile"
    )
}

fn looks_textual(bytes: &[u8]) -> bool {
    if bytes.len() > 1_000_000 || bytes.contains(&0) {
        return false;
    }
    std::str::from_utf8(bytes).is_ok()
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn limit_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut output = text
        .chars()
        .take(max_chars.saturating_sub(32))
        .collect::<String>();
    output.push_str("\n...[truncated]");
    output
}

fn set_phase(run: &mut AgenticBoard, phase: &str, details: Value) {
    run.current_phase = Some(phase.to_string());
    run.phase_started_at = Some(Utc::now());
    run.phase_details = Some(details);
}

fn latest_backlog_breakdown_prompt(run: &AgenticBoard) -> Option<String> {
    run.backlog_breakdown
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn latest_backlog_breakdown_provider(run: &AgenticBoard) -> Option<String> {
    run.backlog_breakdown
        .get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn task_scope_text(task: &BoardTask) -> String {
    let details = if task.details.trim().is_empty() {
        task.description.trim()
    } else {
        task.details.trim()
    };
    let prompt = task.prompt.trim();
    let mut parts = vec![format!("{}: {}", task.id, task.title.trim())];
    if !details.is_empty() {
        parts.push(format!("Details: {details}"));
    }
    if !prompt.is_empty() && prompt != details {
        parts.push(format!("Source prompt: {prompt}"));
    }
    parts.join("\n")
}

fn active_board_prompt(run: &AgenticBoard) -> String {
    let selected_scopes = run
        .tasks
        .iter()
        .filter(|task| is_user_authored_task(task))
        .filter(|task| {
            matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS
            )
        })
        .map(task_scope_text)
        .filter(|scope| !scope.trim().is_empty())
        .collect::<Vec<_>>();
    if !selected_scopes.is_empty() {
        return format!("Selected Kanban tasks:\n{}", selected_scopes.join("\n\n"));
    }
    let active_history_scopes = run
        .tasks
        .iter()
        .filter(|task| is_user_authored_task(task))
        .filter(|task| canonical_task_status(&task.status) != TASK_STATUS_BACKLOG)
        .map(task_scope_text)
        .filter(|scope| !scope.trim().is_empty())
        .collect::<Vec<_>>();
    if !active_history_scopes.is_empty() {
        return format!(
            "Selected Kanban tasks:\n{}",
            active_history_scopes.join("\n\n")
        );
    }
    latest_backlog_breakdown_prompt(run).unwrap_or_else(|| run.source_prompt.clone())
}

fn effective_provider_for_phase(run: &AgenticBoard, label: &str) -> Result<String> {
    if model_type_for_phase(label) == "breakdown" {
        if let Some(provider) = latest_backlog_breakdown_provider(run) {
            return normalize_provider(Some(&provider));
        }
        return normalize_provider(Some(DEFAULT_BREAKDOWN_PROVIDER));
    }
    normalize_provider(Some(&run.provider))
}

fn default_model_for_provider(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "claude" => DEFAULT_MODEL.to_string(),
        _ => String::new(),
    }
}

fn default_model_for_phase(run: &AgenticBoard, label: &str) -> String {
    if model_type_for_phase(label) == "breakdown" {
        let provider = effective_provider_for_phase(run, label)
            .unwrap_or_else(|_| DEFAULT_BREAKDOWN_PROVIDER.to_string());
        if provider == DEFAULT_BREAKDOWN_PROVIDER {
            return DEFAULT_BREAKDOWN_MODEL.to_string();
        }
        return default_model_for_provider(&provider);
    }
    default_model_for_provider(&run.provider)
}

fn effective_model_for_phase(run: &AgenticBoard, label: &str) -> String {
    let phase_type = model_type_for_phase(label);
    let phase_key = label.replace(' ', "_");
    let configured = run
        .task_model_overrides
        .get(&phase_key)
        .or_else(|| run.task_model_overrides.get(label))
        .or_else(|| run.task_model_overrides.get(model_type_for_phase(label)))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|model| !model.trim().is_empty())
        .or_else(|| {
            (model_type_for_phase(label) == "breakdown")
                .then(|| latest_backlog_breakdown_model(run))
                .flatten()
        });
    configured
        .or_else(|| {
            run.model_strategy
                .as_ref()
                .and_then(|strategy| strategy.get("model"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|model| !model.trim().is_empty())
        })
        .or_else(|| {
            let board_model = trim_string(Some(run.model.clone()))?;
            let board_default = default_model_for_provider(&run.provider);
            (board_model != board_default).then_some(board_model)
        })
        .unwrap_or_else(|| {
            if phase_type == "breakdown" {
                default_model_for_phase(run, label)
            } else {
                trim_string(Some(run.model.clone()))
                    .unwrap_or_else(|| default_model_for_phase(run, label))
            }
        })
}

fn latest_backlog_breakdown_model(run: &AgenticBoard) -> Option<String> {
    run.backlog_breakdown
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty() && *model != "provider default")
        .map(str::to_string)
}

fn effective_model_for_task(run: &AgenticBoard, task: &BoardTask) -> String {
    run.task_model_overrides
        .get(&task.id)
        .or_else(|| run.task_model_overrides.get(model_type_for_task(task)))
        .or_else(|| run.task_model_overrides.get(&task.task_type))
        .or_else(|| run.task_model_overrides.get("task_execution"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|model| !model.trim().is_empty())
        .or_else(|| {
            run.model_strategy
                .as_ref()
                .and_then(|strategy| strategy.get("taskModel"))
                .or_else(|| {
                    run.model_strategy
                        .as_ref()
                        .and_then(|strategy| strategy.get("model"))
                })
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|model| !model.trim().is_empty())
        })
        .unwrap_or_else(|| default_model_for_provider(&run.provider))
}

fn agentic_execution_model_for_provider(provider: &str, model: &str) -> String {
    let trimmed = model.trim();
    if !provider.eq_ignore_ascii_case("claude") || trimmed.is_empty() {
        return trimmed.to_string();
    }
    let normalized = trimmed.to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "minimax-m3" | "minimaxm3" => "min:MiniMax-M3".to_string(),
        _ => trimmed.to_string(),
    }
}

fn model_type_for_phase(label: &str) -> &'static str {
    let normalized = label.trim().to_ascii_lowercase();
    if normalized.contains("final review") || normalized.contains("final qa") {
        "final_qa"
    } else if normalized.contains("result schema repair") {
        "qa_fix"
    } else if normalized.contains("qa") || normalized.contains("promotion review") {
        "qa"
    } else {
        "breakdown"
    }
}

fn model_type_for_task(task: &BoardTask) -> &'static str {
    if task.final_qa_task {
        "final_qa"
    } else if task.qa_verdict_retry_task || task.qa_task || task.task_level_qa {
        "qa"
    } else if task.qa_fix_task || task.source_qa_task_id.is_some() {
        "qa_fix"
    } else if task.agents_knowledge_task || task.task_type == "agents_knowledge" {
        "agents"
    } else if canonical_task_kind(task) == TASK_KIND_MANUAL_TEST {
        "qa"
    } else {
        "implementation"
    }
}

fn collect_task_models(run: &AgenticBoard, overrides: Option<&Value>) -> BTreeSet<String> {
    let fallback = trim_string(Some(if run.primary_model.trim().is_empty() {
        run.model.clone()
    } else {
        run.primary_model.clone()
    }))
    .unwrap_or_default();
    let overrides = overrides.unwrap_or(&run.task_model_overrides);
    [
        "breakdown",
        "implementation",
        "qa",
        "qa_fix",
        "agents",
        "final_qa",
    ]
    .into_iter()
    .filter_map(|key| {
        overrides
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_string)
            .or_else(|| (!fallback.is_empty()).then(|| fallback.clone()))
    })
    .collect()
}

fn has_mixed_task_models(run: &AgenticBoard, overrides: Option<&Value>) -> bool {
    collect_task_models(run, overrides).len() > 1
}

fn sync_session_policy_with_task_models(run: &mut AgenticBoard, source: &str) -> bool {
    if normalize_session_policy(Some(&run.session_policy)) != "continuous" {
        return false;
    }
    if !has_mixed_task_models(run, None) {
        return false;
    }
    run.session_policy = "task-model".to_string();
    run.append_log(format!(
        "Session policy set to task-model from {source} because task routing uses multiple models"
    ));
    true
}

fn apply_task_model_routing(run: &mut AgenticBoard, task_index: usize) {
    let Some(task) = run.tasks.get(task_index) else {
        return;
    };
    let desired_model = effective_model_for_task(run, task);
    if desired_model.trim().is_empty() || desired_model == run.model {
        return;
    }
    if normalize_session_policy(Some(&run.session_policy)) == "continuous"
        && run.actual_session_id.is_some()
    {
        run.append_log(format!(
            "Continuous provider session kept current model for {}; configured task model {} will apply after pause/resume",
            task.id, desired_model
        ));
        return;
    }
    let previous = run.model.clone();
    run.model = desired_model.clone();
    run.next_model = desired_model.clone();
    run.actual_session_id = None;
    run.current_provider_session_id = None;
    run.model_history.push(json!({
        "from": previous,
        "to": desired_model,
        "changedAt": Utc::now(),
        "changedBy": "task-model-routing",
        "taskId": task.id,
    }));
}

fn normalize_session_policy(policy: Option<&str>) -> String {
    let normalized = policy
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase().replace(' ', "_"));
    match normalized.as_deref() {
        Some("continuous") | Some("single") | Some("one-session") | Some("one_session") => {
            "continuous".to_string()
        }
        Some("task-model")
        | Some("task_model")
        | Some("per-task")
        | Some("per_task")
        | Some("per-task-model")
        | Some("per_task_model") => "task-model".to_string(),
        _ if danger_continuous_session_default() => "continuous".to_string(),
        _ => "task-model".to_string(),
    }
}

fn danger_continuous_session_default() -> bool {
    env::var("DANGER_CONTINUOUS_SESSION")
        .or_else(|_| env::var("IO_WORKBENCH_DANGER_CONTINUOUS_SESSION"))
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "no"
            )
        })
        .unwrap_or(true)
}

fn increment_provider_usage(
    run: &mut AgenticBoard,
    prompt: &str,
    output: &str,
    session_id: Option<&str>,
    actual_usage: Option<&Value>,
) {
    let estimated = json!({
        "inputTokens": estimate_tokens(prompt) as u64,
        "cachedInputTokens": 0,
        "outputTokens": estimate_tokens(output) as u64,
        "totalTokens": (estimate_tokens(prompt) + estimate_tokens(output)) as u64,
        "cumulative": false,
    });
    let usage_source = actual_usage.cloned().unwrap_or(estimated);
    let session_key = session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let previous_session_usage = run
        .provider_usage_by_session
        .get(session_key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let cumulative = usage_source
        .get("cumulative")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut delta_map = serde_json::Map::new();
    for key in [
        "inputTokens",
        "cachedInputTokens",
        "outputTokens",
        "totalTokens",
    ] {
        let current = usage_source.get(key).and_then(Value::as_u64).unwrap_or(0);
        let value = if cumulative {
            let previous = previous_session_usage
                .get(key)
                .and_then(Value::as_u64)
                .unwrap_or(0);
            current.saturating_sub(previous)
        } else {
            current
        };
        delta_map.insert(key.to_string(), json!(value));
    }
    delta_map.insert("invocationsWithUsage".to_string(), json!(1));
    let delta = Value::Object(delta_map);
    let mut usage = run.provider_usage.as_object().cloned().unwrap_or_default();
    for key in [
        "inputTokens",
        "cachedInputTokens",
        "outputTokens",
        "totalTokens",
        "invocationsWithUsage",
    ] {
        let value = delta.get(key).and_then(Value::as_u64).unwrap_or(0);
        let next = usage.get(key).and_then(Value::as_u64).unwrap_or(0) + value;
        usage.insert(key.to_string(), json!(next));
    }
    run.provider_usage = Value::Object(usage);

    if !session_key.is_empty() {
        let mut by_session = run
            .provider_usage_by_session
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut session_usage = if cumulative {
            usage_source.as_object().cloned().unwrap_or_default()
        } else {
            by_session
                .get(session_key)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        };
        for key in [
            "inputTokens",
            "cachedInputTokens",
            "outputTokens",
            "totalTokens",
            "invocationsWithUsage",
        ] {
            if cumulative && key != "invocationsWithUsage" {
                session_usage
                    .entry(key.to_string())
                    .or_insert_with(|| usage_source.get(key).cloned().unwrap_or(json!(0)));
                continue;
            }
            let value = delta.get(key).and_then(Value::as_u64).unwrap_or(0);
            let next = session_usage.get(key).and_then(Value::as_u64).unwrap_or(0) + value;
            session_usage.insert(key.to_string(), json!(next));
        }
        by_session.insert(session_key.to_string(), Value::Object(session_usage));
        run.provider_usage_by_session = Value::Object(by_session);
    }
}

fn estimate_tokens(text: &str) -> usize {
    (text.chars().count() / 4).max(1)
}

fn prompt_telemetry_summary(entries: &[Value]) -> Value {
    let calls = entries.len();
    let chars = entries
        .iter()
        .filter_map(|entry| entry.get("chars").and_then(Value::as_u64))
        .sum::<u64>();
    let estimated_tokens = entries
        .iter()
        .filter_map(|entry| entry.get("estimatedTokens").and_then(Value::as_u64))
        .sum::<u64>();
    let actual_input_tokens = telemetry_token_sum(entries, "actualInputTokens");
    let actual_cached_input_tokens = telemetry_token_sum(entries, "actualCachedInputTokens");
    let actual_output_tokens = telemetry_token_sum(entries, "actualOutputTokens");
    let actual_tokens = telemetry_token_sum(entries, "actualTotalTokens");
    let mut by_phase = BTreeMap::<String, (usize, u64, u64, u64)>::new();
    for entry in entries {
        let phase = entry
            .get("phase")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("unknown")
            .to_string();
        let accumulator = by_phase.entry(phase).or_default();
        accumulator.0 += 1;
        accumulator.1 += entry.get("chars").and_then(Value::as_u64).unwrap_or(0);
        accumulator.2 += entry
            .get("estimatedTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        accumulator.3 += entry
            .get("actualTotalTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
    }
    let mut phases = by_phase
        .into_iter()
        .map(|(phase, (calls, chars, estimated_tokens, actual_tokens))| {
            json!({
                "phase": phase,
                "calls": calls,
                "chars": chars,
                "estimatedTokens": estimated_tokens,
                "actualTokens": actual_tokens,
            })
        })
        .collect::<Vec<_>>();
    phases.sort_by_key(|phase| {
        std::cmp::Reverse(
            phase
                .get("estimatedTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        )
    });
    let largest_call = entries
        .iter()
        .max_by_key(|entry| {
            entry
                .get("estimatedTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        })
        .cloned();
    json!({
        "calls": calls,
        "chars": chars,
        "estimatedTokens": estimated_tokens,
        "actualInputTokens": actual_input_tokens,
        "actualCachedInputTokens": actual_cached_input_tokens,
        "actualOutputTokens": actual_output_tokens,
        "actualTokens": actual_tokens,
        "phases": phases,
        "largestCall": largest_call,
    })
}

fn telemetry_token_sum(entries: &[Value], key: &str) -> u64 {
    entries
        .iter()
        .filter_map(|entry| entry.get(key).and_then(Value::as_u64))
        .sum()
}

fn validation_summary(entries: &[Value]) -> Value {
    let runs = entries.len();
    let passed = entries
        .iter()
        .filter(|entry| entry.get("passed").and_then(Value::as_bool) == Some(true))
        .count();
    let latest = entries.last();
    let commands = entries.iter().map(validation_command_count).sum::<usize>();
    json!({
        "runs": runs,
        "passed": passed,
        "failed": runs.saturating_sub(passed),
        "latestStage": latest
            .and_then(|entry| entry.get("stage"))
            .and_then(Value::as_str)
            .unwrap_or(""),
        "latestPassed": latest
            .and_then(|entry| entry.get("passed"))
            .and_then(Value::as_bool),
        "commands": commands,
    })
}

fn validation_command_count(entry: &Value) -> usize {
    if let Some(commands) = entry.get("commands").and_then(Value::as_array) {
        return commands.len();
    }
    if entry
        .get("commands")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return 1;
    }
    usize::from(
        entry
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty()),
    )
}

fn server_error_message(error: &ServerError) -> String {
    match error.body.details.as_deref() {
        Some(details) if !details.is_empty() => format!("{}: {details}", error.body.error),
        _ => error.body.error.clone(),
    }
}

fn apply_board_options(run: &mut AgenticBoard, request: &CreateBoardRequest) -> Result<()> {
    if let Some(provider) = request.provider.as_deref() {
        run.provider = normalize_provider(Some(provider))?;
    }
    if let Some(model) = trim_string(request.model.clone()) {
        run.model = model.clone();
        run.primary_model = model;
    }
    if request.next_model.is_some() {
        run.next_model = trim_string(request.next_model.clone()).unwrap_or_default();
    }
    if request.next_provider.is_some() {
        run.next_provider = normalize_optional_provider(request.next_provider.as_deref())?;
    }
    if request.model_strategy.is_some() {
        run.model_strategy = normalize_model_strategy(request.model_strategy.clone());
        let strategy_overrides = task_model_overrides_for_strategy(run.model_strategy.as_ref());
        if !json_object_is_empty(&strategy_overrides) {
            run.task_model_overrides =
                merge_task_model_overrides(strategy_overrides, run.task_model_overrides.clone());
        }
        if let Some(model) = primary_model_for_strategy(run.model_strategy.as_ref()) {
            run.primary_model = model.clone();
            run.model = model;
        }
    }
    if let Some(profile) = trim_string(request.board_profile.clone()) {
        run.board_profile = normalize_board_profile(Some(&profile));
    }
    if let Some(overrides) = request.task_model_overrides.clone() {
        let strategy_overrides = task_model_overrides_for_strategy(run.model_strategy.as_ref());
        run.task_model_overrides = merge_task_model_overrides(
            strategy_overrides,
            normalize_task_model_overrides(overrides),
        );
    }
    if let Some(policy) = request.session_policy.as_deref() {
        run.session_policy = normalize_session_policy(Some(policy));
    }
    if request.git_policy.is_some() {
        run.git_policy = normalize_git_policy(request.git_policy.as_deref());
    }
    if request.tools_settings.is_some() {
        run.tools_settings = request.tools_settings.clone();
    }
    if let Some(enabled) = request.tdd_enabled {
        run.tdd_enabled = enabled;
    }
    if request.tdd_policy.is_some() {
        run.tdd_policy = normalize_tdd_policy(request.tdd_policy.as_ref());
    }
    if request.validation_config.is_some() {
        run.validation_config = normalize_validation_config(request.validation_config.as_ref());
    }
    if request.rag_settings.is_some() {
        run.rag_settings = normalize_rag_settings(request.rag_settings.as_ref());
        run.rag_enabled = rag_enabled_from_settings(&run.rag_settings);
    }
    if request.qa_policy.is_some() {
        run.qa_policy = normalize_qa_policy(request.qa_policy.as_ref());
    }
    if request.auto_retry.is_some() {
        run.auto_retry = normalize_auto_retry(request.auto_retry.as_ref().unwrap_or(&Value::Null));
    }
    sync_session_policy_with_task_models(run, "board option update");
    Ok(())
}

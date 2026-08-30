fn workspace_delta_entry(
    path: &str,
    before_file: Option<&Value>,
    after_file: Option<&Value>,
    classification: &str,
) -> Value {
    json!({
        "path": path,
        "classification": classification,
        "beforeStatus": before_file.and_then(|value| value.get("status")).and_then(Value::as_str).unwrap_or(""),
        "afterStatus": after_file.and_then(|value| value.get("status")).and_then(Value::as_str).unwrap_or(""),
        "beforeHash": before_file.and_then(|value| value.get("hash")).and_then(Value::as_str),
        "afterHash": after_file.and_then(|value| value.get("hash")).and_then(Value::as_str),
    })
}

fn workspace_file_was_clean(file: &Value) -> bool {
    file.get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_none_or(|status| status.is_empty() || status.eq_ignore_ascii_case("clean"))
}

fn change_summary_paths(summary: &Value) -> Vec<String> {
    summary
        .get("changedBySubtask")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|file| file.get("path").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn change_summary_attributable_file_count(summary: &Value) -> u64 {
    summary
        .get("attributableChangedFileCount")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| change_summary_paths(summary).len() as u64)
}

fn normalize_changed_file_summary(summary: &Value) -> Value {
    if summary.get("ownershipPolicy").and_then(Value::as_str) == Some(WORKSPACE_OWNERSHIP_POLICY) {
        return summary.clone();
    }
    let unknown_changes = summary
        .get("touchedFiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|entry| match entry {
            Value::Object(object) => {
                let mut entry = object.clone();
                entry.insert("classification".to_string(), json!("unknown_change"));
                Value::Object(entry)
            }
            Value::String(path) => json!({
                "path": path,
                "classification": "unknown_change",
            }),
            other => other.clone(),
        })
        .collect::<Vec<_>>();
    let mut normalized = summary.as_object().cloned().unwrap_or_default();
    normalized.insert("touchedFiles".to_string(), json!(unknown_changes.clone()));
    normalized.insert("preExistingChanges".to_string(), json!([]));
    normalized.insert("preExistingChangeCount".to_string(), json!(0));
    normalized.insert("changedBySubtask".to_string(), json!([]));
    normalized.insert("attributableChangedFileCount".to_string(), json!(0));
    normalized.insert("unknownChanges".to_string(), json!(unknown_changes.clone()));
    normalized.insert(
        "unknownChangeCount".to_string(),
        json!(unknown_changes.len()),
    );
    normalized.insert(
        "ownershipPolicy".to_string(),
        json!(LEGACY_WORKSPACE_OWNERSHIP_POLICY),
    );
    Value::Object(normalized)
}

fn refresh_codebase_context_after_task(run: &mut AgenticBoard, change_summary: &Value) {
    let paths = change_summary_paths(change_summary);
    if paths.is_empty() {
        return;
    }
    let touched = paths.iter().cloned().collect::<BTreeSet<_>>();
    run.codebase_understanding.retain(|entry| {
        entry
            .get("path")
            .and_then(Value::as_str)
            .is_none_or(|path| !touched.contains(path))
    });
    let bundle = build_codebase_bundle(&run.project_path);
    run.codebase_manifest = bundle.manifest;
    run.codebase_chunks = bundle.chunks;
    let refreshed_snapshot = local_codebase_snapshot(run);
    if let Some(map) = run.codebase_map.as_mut().and_then(Value::as_object_mut) {
        map.insert("localSnapshot".to_string(), refreshed_snapshot);
        map.insert("refreshedAt".to_string(), json!(Utc::now()));
        map.insert("refreshReason".to_string(), json!("task workspace changes"));
    }
    run.append_log(format!(
        "Refreshed codebase context after workspace changes in {} file(s)",
        paths.len()
    ));
}

async fn run_deterministic_validation(run: &AgenticBoard, task_id: &str, stage: &str) -> Value {
    let started_at = Utc::now();
    if run.validation_config.get("enabled").and_then(value_as_bool) == Some(false) {
        return json!({
            "stage": stage,
            "taskId": task_id,
            "startedAt": started_at,
            "completedAt": Utc::now(),
            "passed": true,
            "skipped": true,
            "commands": [],
            "summary": "Deterministic validation disabled for this board.",
        });
    }
    let scripts = package_validation_scripts(run, stage);
    let validation_timeout = validation_timeout(run);
    let mut commands = Vec::new();
    for (script_name, command_text) in scripts {
        let command_started = Utc::now();
        let result =
            run_shell_validation_command(&run.project_path, &command_text, validation_timeout)
                .await;
        let duration_ms = (Utc::now() - command_started).num_milliseconds().max(0);
        let exit_code = result.as_ref().map(|result| result.0).unwrap_or(124);
        let output = result
            .map(|result| limit_text(&format!("{}\n{}", result.1, result.2), 1200))
            .unwrap_or_else(|error| error);
        commands.push(json!({
            "command": command_text,
            "scriptName": script_name,
            "exitCode": exit_code,
            "output": output,
            "durationMs": duration_ms,
        }));
        if exit_code != 0 && stage == "feature" {
            break;
        }
    }
    let passed = commands
        .iter()
        .all(|command| command.get("exitCode").and_then(Value::as_i64).unwrap_or(0) == 0);
    json!({
        "stage": stage,
        "taskId": task_id,
        "startedAt": started_at,
        "completedAt": Utc::now(),
        "passed": passed,
        "commands": commands,
    })
}

async fn run_tdd_validation(run: &AgenticBoard, task: &BoardTask, stage: &str) -> Value {
    if is_qa_task(task) || task.qa_test_commands.is_empty() {
        return run_deterministic_validation(run, &task.id, stage).await;
    }
    let generated = run_generated_test_commands(
        &run.project_path,
        &task.id,
        &task.qa_test_commands,
        stage,
        validation_timeout(run),
    )
    .await;
    let deterministic = run_deterministic_validation(run, &task.id, stage).await;
    let mut commands = generated
        .get("commands")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    commands.extend(
        deterministic
            .get("commands")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    );
    let passed = commands
        .iter()
        .all(|command| command.get("exitCode").and_then(Value::as_i64).unwrap_or(0) == 0);
    json!({
        "stage": stage,
        "taskId": task.id,
        "startedAt": generated.get("startedAt").cloned().unwrap_or_else(|| json!(Utc::now())),
        "completedAt": Utc::now(),
        "passed": passed,
        "commands": commands,
        "generatedTestCommands": task.qa_test_commands,
        "packageValidation": deterministic,
    })
}

async fn run_generated_test_commands(
    project_path: &str,
    task_id: &str,
    commands_to_run: &[String],
    stage: &str,
    validation_timeout: Duration,
) -> Value {
    let started_at = Utc::now();
    let mut commands = Vec::new();
    for command_text in commands_to_run {
        let command_started = Utc::now();
        let result =
            run_shell_validation_command(project_path, command_text, validation_timeout).await;
        let duration_ms = (Utc::now() - command_started).num_milliseconds().max(0);
        let exit_code = result.as_ref().map(|result| result.0).unwrap_or(124);
        let output = result
            .map(|result| limit_text(&format!("{}\n{}", result.1, result.2), 1200))
            .unwrap_or_else(|error| error);
        commands.push(json!({
            "command": command_text,
            "scriptName": "generated_tdd",
            "exitCode": exit_code,
            "output": output,
            "durationMs": duration_ms,
        }));
        if exit_code != 0 && stage != "qa_baseline" {
            break;
        }
    }
    let passed = commands
        .iter()
        .all(|command| command.get("exitCode").and_then(Value::as_i64).unwrap_or(0) == 0);
    json!({
        "stage": stage,
        "taskId": task_id,
        "startedAt": started_at,
        "completedAt": Utc::now(),
        "passed": passed,
        "commands": commands,
    })
}

async fn run_shell_validation_command(
    project_path: &str,
    command_text: &str,
    validation_timeout: Duration,
) -> std::result::Result<(i32, String, String), String> {
    let mut command = Command::new("sh");
    command
        .arg("-lc")
        .arg(command_text)
        .current_dir(project_path)
        .env("PATH", augmented_user_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command
        .spawn()
        .map_err(|error| format!("Failed to spawn validation command: {error}"))?;
    match timeout(validation_timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok((
            output.status.code().unwrap_or(1),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )),
        Ok(Err(error)) => Err(format!("Validation command failed: {error}")),
        Err(_) => Err(format!(
            "Validation command timed out after {} seconds",
            validation_timeout.as_secs()
        )),
    }
}

fn package_validation_scripts(run: &AgenticBoard, stage: &str) -> Vec<(String, String)> {
    let configured = validation_commands_for_stage(run, stage)
        .into_iter()
        .enumerate()
        .map(|(index, command)| (format!("configured_{}", index + 1), command))
        .collect::<Vec<_>>();
    if !configured.is_empty() {
        return configured;
    }
    let project_path = &run.project_path;
    let package_path = Path::new(project_path).join("package.json");
    let Some(package_json) = fs::read_to_string(package_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    else {
        return Vec::new();
    };
    let Some(scripts) = package_json.get("scripts").and_then(Value::as_object) else {
        return Vec::new();
    };
    let runner = infer_package_runner(project_path);
    let candidates = if stage == "final" {
        ["typecheck", "lint", "test", "build", "check"]
    } else {
        ["typecheck", "test", "lint", "build", "check"]
    };
    candidates
        .into_iter()
        .filter(|name| scripts.contains_key(*name))
        .take(validation_max_commands_for_stage(run, stage))
        .map(|name| (name.to_string(), format!("{runner} run {name}")))
        .collect()
}

fn validation_commands_for_stage(run: &AgenticBoard, stage: &str) -> Vec<String> {
    let key = match stage {
        "final" => "finalCommands",
        "qa" | "qa_baseline" => "qaCommands",
        _ => "featureCommands",
    };
    normalize_string_list(run.validation_config.get(key))
}

fn validation_max_commands_for_stage(run: &AgenticBoard, stage: &str) -> usize {
    let key = match stage {
        "final" => "maxFinalCommands",
        "qa" | "qa_baseline" => "maxQaCommands",
        _ => "maxFeatureCommands",
    };
    run.validation_config
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(if stage == "final" { 4 } else { 2 })
}

fn validation_timeout(run: &AgenticBoard) -> Duration {
    Duration::from_secs(
        run.validation_config
            .get("timeoutSeconds")
            .and_then(Value::as_u64)
            .unwrap_or(DETERMINISTIC_VALIDATION_TIMEOUT.as_secs())
            .clamp(5, 3600),
    )
}

fn infer_package_runner(project_path: &str) -> &'static str {
    let root = Path::new(project_path);
    if root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if root.join("yarn.lock").is_file() {
        "yarn"
    } else if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
        "bun"
    } else {
        "npm"
    }
}

fn format_validation_check(validation: &Value) -> String {
    let stage = validation
        .get("stage")
        .and_then(Value::as_str)
        .unwrap_or("feature");
    let passed = validation
        .get("passed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let commands = validation
        .get("commands")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    format!(
                        "{} exited {}",
                        item.get("command")
                            .and_then(Value::as_str)
                            .unwrap_or("command"),
                        item.get("exitCode").and_then(Value::as_i64).unwrap_or(0)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    if commands.is_empty() {
        format!("Deterministic {stage} validation: no supported package scripts were available.")
    } else {
        format!(
            "Deterministic {stage} validation: {} ({commands})",
            if passed { "PASS" } else { "FAIL" }
        )
    }
}

fn normalize_string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str())
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        Some(Value::String(text)) => text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn dedupe_strings(items: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    items
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

fn normalize_priority(value: Option<&str>) -> &'static str {
    match value
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "p0" | "critical" | "urgent" | "blocker" | "highest" => TASK_PRIORITY_P0,
        "p1" | "high" => TASK_PRIORITY_P1,
        "p3" | "low" => TASK_PRIORITY_P3,
        "p2" | "medium" | "normal" => TASK_PRIORITY_P2,
        _ => TASK_PRIORITY_P2,
    }
}

fn task_priority_for_parent(
    run: &AgenticBoard,
    parent_id: Option<&str>,
    requested_priority: Option<&str>,
) -> String {
    if let Some(priority) = requested_priority
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return normalize_priority(Some(priority)).to_string();
    }
    parent_id
        .and_then(|parent_id| run.tasks.iter().find(|task| task.id == parent_id))
        .map(|parent| normalize_priority(Some(&parent.priority)).to_string())
        .unwrap_or_else(|| TASK_PRIORITY_P2.to_string())
}

fn normalize_model_strategy(value: Option<Value>) -> Option<Value> {
    let source = match value? {
        Value::String(mode) => json!({ "mode": mode }),
        Value::Object(map) => Value::Object(map),
        _ => return None,
    };
    let Some(source) = source.as_object() else {
        return None;
    };
    if source.is_empty() {
        return None;
    }

    let raw_mode = source
        .get("mode")
        .or_else(|| source.get("strategy"))
        .or_else(|| source.get("value"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .replace('-', "_");
    let cheap_model = normalize_model_value(
        source
            .get("cheapModel")
            .or_else(|| source.get("cheap"))
            .or_else(|| source.get("budgetModel"))
            .or_else(|| source.get("budget")),
    )
    .or_else(default_cheap_model);
    let expensive_model = normalize_model_value(
        source
            .get("expensiveModel")
            .or_else(|| source.get("expensive"))
            .or_else(|| source.get("qualityModel"))
            .or_else(|| source.get("quality")),
    )
    .or_else(default_expensive_model);

    let has_strategy_inputs = matches!(
        raw_mode.as_str(),
        "cheap" | "hybrid" | "expensive" | "manual"
    ) || cheap_model.is_some()
        || expensive_model.is_some();
    let mode = match raw_mode.as_str() {
        "cheap" | "hybrid" | "expensive" | "manual" => raw_mode,
        _ if cheap_model.is_some() || expensive_model.is_some() => "hybrid".to_string(),
        _ => String::new(),
    };

    let mut normalized = source.clone();
    if !mode.is_empty() {
        normalized.insert("mode".to_string(), json!(mode));
    }
    if has_strategy_inputs {
        normalized.insert(
            "cheapModel".to_string(),
            json!(cheap_model.unwrap_or_default()),
        );
        normalized.insert(
            "expensiveModel".to_string(),
            json!(expensive_model.unwrap_or_default()),
        );
    }
    if normalized.is_empty() {
        None
    } else {
        Some(Value::Object(normalized))
    }
}

fn body_has_model_strategy_keys(value: &Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    [
        "mode",
        "strategy",
        "cheapModel",
        "cheap",
        "budgetModel",
        "expensiveModel",
        "expensive",
        "qualityModel",
        "fallbackProvider",
        "fallbackModel",
        "reasoningEffort",
        "reasoning_effort",
        "effort",
        "thinking",
        "enableThinking",
        "fast",
        "fastMode",
        "serviceTier",
        "model",
        "taskModel",
    ]
    .iter()
    .any(|key| map.contains_key(*key))
}

fn normalize_model_value(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn default_cheap_model() -> Option<String> {
    env::var("DANGER_CHEAP_MODEL")
        .ok()
        .or_else(|| env::var("IO_WORKBENCH_CHEAP_MODEL").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn default_expensive_model() -> Option<String> {
    env::var("DANGER_EXPENSIVE_MODEL")
        .ok()
        .or_else(|| env::var("IO_WORKBENCH_EXPENSIVE_MODEL").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn model_strategy_mode(strategy: Option<&Value>) -> &str {
    strategy
        .and_then(|value| value.get("mode"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
}

fn primary_model_for_strategy(strategy: Option<&Value>) -> Option<String> {
    let strategy = strategy?;
    let mode = model_strategy_mode(Some(strategy));
    let candidate = match mode {
        "cheap" => strategy.get("cheapModel"),
        "hybrid" | "expensive" => strategy.get("expensiveModel"),
        "manual" => None,
        _ => strategy
            .get("model")
            .or_else(|| strategy.get("primaryModel"))
            .or_else(|| strategy.get("taskModel")),
    };
    normalize_model_value(candidate)
}

fn task_model_overrides_for_strategy(strategy: Option<&Value>) -> Value {
    let Some(strategy) = strategy else {
        return json!({});
    };
    let mode = model_strategy_mode(Some(strategy));
    let cheap = normalize_model_value(strategy.get("cheapModel"));
    let expensive = normalize_model_value(strategy.get("expensiveModel"));
    let mut map = serde_json::Map::new();
    let mut insert = |key: &str, model: Option<&String>| {
        if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
            map.insert(key.to_string(), json!(model));
        }
    };
    match mode {
        "cheap" => {
            for key in [
                "breakdown",
                "implementation",
                "qa",
                "qa_fix",
                "agents",
                "final_qa",
            ] {
                insert(key, cheap.as_ref());
            }
        }
        "expensive" => {
            for key in [
                "breakdown",
                "implementation",
                "qa",
                "qa_fix",
                "agents",
                "final_qa",
            ] {
                insert(key, expensive.as_ref());
            }
        }
        "hybrid" => {
            insert("breakdown", expensive.as_ref());
            insert("implementation", cheap.as_ref());
            insert("qa", expensive.as_ref());
            insert("qa_fix", expensive.as_ref());
            insert("agents", cheap.as_ref());
            insert("final_qa", expensive.as_ref());
        }
        _ => {}
    }
    Value::Object(map)
}

fn normalize_task_model_overrides(value: Value) -> Value {
    let Some(source) = value.as_object() else {
        return json!({});
    };
    let mut map = serde_json::Map::new();
    for (key, value) in source {
        let Some(model) = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        map.insert(canonical_task_model_key(key), json!(model));
    }
    Value::Object(map)
}

fn canonical_task_model_key(key: &str) -> String {
    match key.trim().replace('-', "_").as_str() {
        "qaFix" | "qa_fix" => "qa_fix".to_string(),
        "finalQa" | "finalQA" | "final_qa" => "final_qa".to_string(),
        "taskExecution" | "task_execution" => "task_execution".to_string(),
        value => value.to_string(),
    }
}

fn merge_task_model_overrides(base: Value, overrides: Value) -> Value {
    let mut map = base.as_object().cloned().unwrap_or_default();
    if let Some(overrides) = overrides.as_object() {
        for (key, value) in overrides {
            map.insert(key.clone(), value.clone());
        }
    }
    Value::Object(map)
}

fn merge_json_objects(base: Value, patch: Value) -> Value {
    let mut map = base.as_object().cloned().unwrap_or_default();
    if let Some(patch) = patch.as_object() {
        for (key, value) in patch {
            map.insert(key.clone(), value.clone());
        }
    }
    Value::Object(map)
}

fn json_object_is_empty(value: &Value) -> bool {
    value.as_object().map(|map| map.is_empty()).unwrap_or(true)
}

fn normalize_validation_config(value: Option<&Value>) -> Value {
    let defaults = default_validation_config();
    let source = value.and_then(Value::as_object);
    json!({
        "enabled": source.and_then(|map| map.get("enabled")).and_then(value_as_bool).unwrap_or(true),
        "featureCommands": nonempty_string_list_or_default(source.and_then(|map| map.get("featureCommands")), defaults.get("featureCommands")),
        "finalCommands": nonempty_string_list_or_default(source.and_then(|map| map.get("finalCommands")), defaults.get("finalCommands")),
        "qaCommands": nonempty_string_list_or_default(source.and_then(|map| map.get("qaCommands")), defaults.get("qaCommands")),
        "maxFeatureCommands": source.and_then(|map| map.get("maxFeatureCommands")).and_then(Value::as_u64).unwrap_or(2).clamp(0, 20),
        "maxFinalCommands": source.and_then(|map| map.get("maxFinalCommands")).and_then(Value::as_u64).unwrap_or(4).clamp(0, 20),
        "maxQaCommands": source.and_then(|map| map.get("maxQaCommands")).and_then(Value::as_u64).unwrap_or(2).clamp(0, 20),
        "timeoutSeconds": source.and_then(|map| map.get("timeoutSeconds")).and_then(Value::as_u64).unwrap_or(120).clamp(5, 3600),
    })
}

fn default_validation_config() -> Value {
    json!({
        "enabled": true,
        "featureCommands": [],
        "finalCommands": [],
        "qaCommands": [],
        "maxFeatureCommands": 2,
        "maxFinalCommands": 4,
        "maxQaCommands": 2,
        "timeoutSeconds": 120,
    })
}

fn normalize_rag_settings(value: Option<&Value>) -> Value {
    let defaults = default_rag_settings();
    let source = value.and_then(Value::as_object);
    json!({
        "enabled": source.and_then(|map| map.get("enabled")).and_then(value_as_bool).unwrap_or(true),
        "indexOnBootstrap": source.and_then(|map| map.get("indexOnBootstrap")).and_then(value_as_bool).unwrap_or(true),
        "queryEnabled": source.and_then(|map| map.get("queryEnabled")).and_then(value_as_bool).unwrap_or(true),
        "ingestTaskResults": source.and_then(|map| map.get("ingestTaskResults")).and_then(value_as_bool).unwrap_or(true),
        "ingestValidationErrors": source.and_then(|map| map.get("ingestValidationErrors")).and_then(value_as_bool).unwrap_or(true),
        "scopes": nonempty_string_list_or_default(source.and_then(|map| map.get("scopes")), defaults.get("scopes")),
        "contextMaxChars": source.and_then(|map| map.get("contextMaxChars")).and_then(Value::as_u64).unwrap_or(12_000).clamp(1_000, 80_000),
    })
}

fn default_rag_settings() -> Value {
    json!({
        "enabled": true,
        "indexOnBootstrap": true,
        "queryEnabled": true,
        "ingestTaskResults": true,
        "ingestValidationErrors": true,
        "scopes": ["global_standard", "project_specific", "validation_error"],
        "contextMaxChars": 12_000,
    })
}

fn rag_enabled_from_settings(settings: &Value) -> bool {
    settings
        .get("enabled")
        .and_then(value_as_bool)
        .unwrap_or(true)
}

fn normalize_qa_policy(value: Option<&Value>) -> Value {
    let source = value.and_then(Value::as_object);
    json!({
        "maxFollowupsPerGroup": source.and_then(|map| map.get("maxFollowupsPerGroup")).and_then(Value::as_u64).unwrap_or(MAX_FOLLOWUP_TASKS_PER_GROUP as u64).clamp(0, 20),
        "maxTaskAttempts": source.and_then(|map| map.get("maxTaskAttempts")).and_then(Value::as_u64).unwrap_or(MAX_TASK_ATTEMPTS as u64).clamp(1, 10),
        "taskQaMode": normalize_task_qa_mode(source.and_then(|map| map.get("taskQaMode")).and_then(Value::as_str)),
        "repairMalformedToolCalls": source.and_then(|map| map.get("repairMalformedToolCalls")).and_then(value_as_bool).unwrap_or(true),
        "malformedToolCallRepairRetries": source.and_then(|map| map.get("malformedToolCallRepairRetries")).and_then(Value::as_u64).unwrap_or(DEFAULT_MALFORMED_TOOL_CALL_REPAIR_RETRIES).clamp(0, MAX_MALFORMED_TOOL_CALL_REPAIR_RETRIES),
    })
}

fn default_qa_policy() -> Value {
    normalize_qa_policy(None)
}

fn normalize_task_qa_mode(value: Option<&str>) -> String {
    match value
        .map(str::trim)
        .unwrap_or("high_risk")
        .to_ascii_lowercase()
        .replace('-', "_")
        .as_str()
    {
        "off" | "none" | "disabled" | "false" => "off".to_string(),
        "all" | "always" | "every" => "all".to_string(),
        _ => "high_risk".to_string(),
    }
}

fn normalize_tdd_policy(value: Option<&Value>) -> Value {
    let defaults = default_tdd_policy();
    let source = value.and_then(Value::as_object);
    json!({
        "requireFailingTestBeforeDev": source.and_then(|map| map.get("requireFailingTestBeforeDev")).and_then(value_as_bool).unwrap_or(true),
        "maxFixAttempts": source.and_then(|map| map.get("maxFixAttempts")).and_then(Value::as_u64).unwrap_or(3).clamp(0, 20),
        "allowImplementationWithoutTests": source.and_then(|map| map.get("allowImplementationWithoutTests")).and_then(value_as_bool).unwrap_or(false),
        "qaCommandStage": source.and_then(|map| map.get("qaCommandStage")).and_then(Value::as_str).unwrap_or_else(|| defaults.get("qaCommandStage").and_then(Value::as_str).unwrap_or("qa")),
        "featureCommandStage": source.and_then(|map| map.get("featureCommandStage")).and_then(Value::as_str).unwrap_or_else(|| defaults.get("featureCommandStage").and_then(Value::as_str).unwrap_or("feature")),
        "finalCommandStage": source.and_then(|map| map.get("finalCommandStage")).and_then(Value::as_str).unwrap_or_else(|| defaults.get("finalCommandStage").and_then(Value::as_str).unwrap_or("final")),
    })
}

fn nonempty_string_list_or_default(value: Option<&Value>, default: Option<&Value>) -> Vec<String> {
    let values = normalize_string_list(value);
    if values.is_empty() {
        normalize_string_list(default)
    } else {
        values
    }
}

fn normalize_auto_retry(value: &Value) -> Value {
    let object = value.as_object();
    json!({
        "enabled": object.and_then(|map| map.get("enabled")).and_then(Value::as_bool).unwrap_or(false),
        "delayMinutes": object.and_then(|map| map.get("delayMinutes")).and_then(Value::as_u64).unwrap_or(10),
        "maxAttempts": object.and_then(|map| map.get("maxAttempts")).and_then(Value::as_u64).unwrap_or(3),
        "attempts": object.and_then(|map| map.get("attempts")).and_then(Value::as_u64).unwrap_or(0),
        "nextRetryAt": object.and_then(|map| map.get("nextRetryAt")).cloned().unwrap_or(Value::Null),
        "lastRetryAt": object.and_then(|map| map.get("lastRetryAt")).cloned().unwrap_or(Value::Null),
        "lastError": object.and_then(|map| map.get("lastError")).and_then(Value::as_str).unwrap_or(""),
        "updatedAt": object.and_then(|map| map.get("updatedAt")).cloned().unwrap_or_else(|| json!(Utc::now())),
    })
}

fn merge_auto_retry(base: Value, patch: Value) -> Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    if let Some(patch) = patch.as_object() {
        for (key, value) in patch {
            object.insert(key.clone(), value.clone());
        }
    }
    Value::Object(object)
}

fn auto_retry_enabled(value: &Value) -> bool {
    normalize_auto_retry(value)
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn is_resumable_board(board: &AgenticBoard) -> bool {
    match board.status.as_str() {
        "paused" | "pausing" => true,
        "blocked" | "failed" | "cancelled" => board.tasks.iter().any(|task| {
            matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_TODO
                    | TASK_STATUS_IN_PROGRESS
                    | TASK_STATUS_BLOCKED
                    | TASK_STATUS_FAILED
            )
        }),
        _ => false,
    }
}

fn normalize_retry_mode(value: Option<&str>) -> Result<&'static str> {
    match value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(RETRY_MODE_TRANSIENT)
        .to_ascii_lowercase()
        .replace(['-', ' '], "_")
        .as_str()
    {
        "transient" | "retry" | "environment" => Ok(RETRY_MODE_TRANSIENT),
        "fix" | "defect" => Ok(RETRY_MODE_FIX),
        _ => Err(bad_request("Retry mode must be transient or fix.")),
    }
}

fn task_failure_text(task: &BoardTask) -> String {
    let mut parts = Vec::new();
    if let Some(error) = task.error.as_deref() {
        parts.push(error.to_string());
    }
    parts.push(task.summary.clone());
    if let Some(result) = task.result.as_ref() {
        for key in ["summary", "status", "qaResult"] {
            if let Some(value) = result.get(key).and_then(Value::as_str) {
                parts.push(value.to_string());
            }
        }
        for key in ["remainingIssues", "remainingGaps"] {
            parts.extend(normalize_string_list(result.get(key)));
        }
    }
    parts.join("\n").to_ascii_lowercase()
}

fn task_failure_is_dependency_blocked(task: &BoardTask) -> bool {
    task.error.as_deref().is_some_and(is_dependency_block_error)
}

fn task_failure_is_transient(task: &BoardTask) -> bool {
    if task_failure_is_dependency_blocked(task) {
        return false;
    }
    let text = task_failure_text(task);
    if text.contains("planning error")
        || text.contains("completion evidence gate")
        || text.contains("acceptance criteria")
        || text.contains("assertion")
        || text.contains("expected") && text.contains("actual")
        || text.contains("implementation defect")
    {
        return false;
    }
    text.contains("provider")
        || text.contains("transport")
        || text.contains("network")
        || text.contains("timed out")
        || text.contains("timeout")
        || text.contains("connection")
        || text.contains("unavailable")
        || text.contains("rate limit")
        || text.contains("too many requests")
        || text.contains("emulator not ready")
        || text.contains("test infrastructure")
        || text.contains("tool environment")
        || text.contains("internal error")
        || text.contains("could not start")
        || text.contains("failed to launch")
        || text.contains("exit code")
        || text.contains("status code 429")
        || text.contains("status code 502")
        || text.contains("status code 503")
}

fn fix_plan_is_linked_to(fix: &BoardTask, failed_task_id: &str) -> bool {
    fix.source_task_id.as_deref() == Some(failed_task_id)
        || fix.source_qa_task_id.as_deref() == Some(failed_task_id)
        || task_blockers(fix).iter().any(|id| id == failed_task_id)
        || fix.references.iter().any(|reference| {
            reference.contains(failed_task_id) && reference.to_ascii_lowercase().contains("source")
        })
}

fn approved_fix_plan_for<'a>(
    run: &'a AgenticBoard,
    failed_task_id: &str,
    requested_fix_task_id: Option<&str>,
) -> Option<&'a BoardTask> {
    run.tasks.iter().find(|task| {
        requested_fix_task_id.is_none_or(|id| task.id == id)
            && task.id != failed_task_id
            && task_is_executable(task)
            && canonical_task_kind(task) == TASK_KIND_FIX
            && matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS | TASK_STATUS_DONE
            )
            && task_ancestors_are_approved(run, task)
            && fix_plan_is_linked_to(task, failed_task_id)
    })
}

fn record_task_retry(task: &mut BoardTask, mode: &str, fix_task_id: Option<&str>, reason: &str) {
    let previous_status = canonical_task_status(&task.status).to_string();
    let retry_number = task
        .hierarchy
        .attempts
        .iter()
        .filter(|attempt| attempt.get("kind").and_then(Value::as_str) == Some("retry_request"))
        .count()
        + 1;
    task.hierarchy.attempts.push(json!({
        "kind": "retry_request",
        "retryNumber": retry_number,
        "mode": mode,
        "previousStatus": previous_status,
        "fixTaskId": fix_task_id,
        "reason": reason.trim(),
        "requestedAt": Utc::now(),
    }));
    task.status = TASK_STATUS_TODO.to_string();
    task.error = None;
    task.started_at = None;
    task.completed_at = None;
    task.provider_session_id = None;
}

fn retry_attention_tasks_in_board(
    run: &mut AgenticBoard,
    requested_ids: &[String],
    mode: &str,
    fix_task_id: Option<&str>,
    reason: &str,
) -> Result<usize> {
    reconcile_dependency_statuses(run);
    let ids = if requested_ids.is_empty() {
        run.tasks
            .iter()
            .filter(|task| {
                task_is_executable(task)
                    && matches!(
                        canonical_task_status(&task.status),
                        TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
                    )
            })
            .map(|task| task.id.clone())
            .collect::<Vec<_>>()
    } else {
        requested_ids.to_vec()
    };
    if ids.is_empty() {
        return Err(not_found(
            "No failed or blocked executable subtasks need retry.",
        ));
    }
    if mode == RETRY_MODE_TRANSIENT && fix_task_id.is_some_and(|id| !id.trim().is_empty()) {
        return Err(bad_request(
            "A transient retry cannot include fixTaskId; use fix mode for an approved fix plan.",
        ));
    }
    let snapshots = ids
        .iter()
        .map(|id| {
            run.tasks
                .iter()
                .find(|task| task.id == *id)
                .cloned()
                .ok_or_else(|| not_found(format!("Agentic board task not found: {id}")))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut fix_task_ids = BTreeMap::new();
    for task in &snapshots {
        if !task_is_executable(task)
            || !matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
            )
        {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                format!(
                    "Task {} is not a failed or blocked executable subtask.",
                    task.id
                ),
            ));
        }
        if task_failure_is_dependency_blocked(task) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                format!(
                    "Task {} is dependency-blocked. Resolve its dependency instead of retrying it.",
                    task.id
                ),
            ));
        }
        if task.error.as_deref().is_some_and(|error| {
            error.starts_with("External side-effect approval required")
                || error.starts_with("Planning error:")
        }) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                format!(
                    "Task {} has an approval or planning blocker that must be resolved first.",
                    task.id
                ),
            ));
        }
        if mode == RETRY_MODE_TRANSIENT && !task_failure_is_transient(task) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                format!(
                    "Task {} appears to have an implementation defect. Create and approve a linked fix subtask before retrying it.",
                    task.id
                ),
            ));
        }
        let approved_fix_id = if mode == RETRY_MODE_FIX {
            let Some(fix) = approved_fix_plan_for(run, &task.id, fix_task_id) else {
                return Err(ServerError::new(
                    StatusCode::CONFLICT,
                    format!(
                        "Task {} requires a linked approved fix subtask (kind=fix, status=Todo or later) before it can retry.",
                        task.id
                    ),
                ));
            };
            Some(fix.id.clone())
        } else {
            None
        };
        fix_task_ids.insert(task.id.clone(), approved_fix_id);
    }
    for task_id in &ids {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == *task_id) {
            let selected_fix_id = fix_task_ids.get(task_id).cloned().flatten();
            record_task_retry(task, mode, selected_fix_id.as_deref(), reason);
        }
    }
    run.status = "running".to_string();
    run.active = true;
    run.loop_started = false;
    run.paused_at = None;
    run.pause_reason = None;
    clear_board_abort_state(run);
    run.current_phase = Some("retry_requested".to_string());
    run.phase_started_at = Some(Utc::now());
    run.phase_details = Some(json!({
        "mode": mode,
        "taskIds": ids,
        "fixTaskId": fix_task_id,
        "fixTaskIds": fix_task_ids,
        "reason": reason.trim(),
    }));
    run.append_log(format!(
        "Explicit {mode} retry requested for {} task(s)",
        ids.len()
    ));
    Ok(ids.len())
}

fn schedule_auto_retry_if_eligible(board: &mut AgenticBoard, reason: &str) -> bool {
    let state = normalize_auto_retry(&board.auto_retry);
    if !state
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || !is_resumable_board(board)
        || !run_has_transient_retryable_attention(board)
    {
        board.auto_retry = state;
        return false;
    }
    let attempts = state.get("attempts").and_then(Value::as_u64).unwrap_or(0);
    let max_attempts = state
        .get("maxAttempts")
        .and_then(Value::as_u64)
        .unwrap_or(3);
    if attempts >= max_attempts {
        board.auto_retry = merge_auto_retry(
            state,
            json!({
                "nextRetryAt": null,
                "lastError": format!("Max auto retries reached ({attempts}/{max_attempts})"),
                "updatedAt": Utc::now(),
            }),
        );
        board.append_log("Auto retry stopped: max attempts reached");
        return true;
    }
    let delay_minutes = state
        .get("delayMinutes")
        .and_then(Value::as_i64)
        .unwrap_or(10)
        .max(1);
    let next_retry_at = Utc::now() + chrono::Duration::minutes(delay_minutes);
    board.auto_retry = merge_auto_retry(
        state,
        json!({
            "nextRetryAt": next_retry_at,
            "lastError": "",
            "updatedAt": Utc::now(),
        }),
    );
    board.append_log(format!(
        "Auto retry scheduled in {delay_minutes} minute(s) after {reason}"
    ));
    true
}

fn run_has_transient_retryable_attention(run: &AgenticBoard) -> bool {
    run.tasks.iter().any(|task| {
        task_is_executable(task)
            && matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
            )
            && !task_failure_is_dependency_blocked(task)
            && task_failure_is_transient(task)
    })
}

fn reset_attention_tasks_for_retry(run: &mut AgenticBoard) -> usize {
    let ids = run
        .tasks
        .iter()
        .filter(|task| {
            task_is_executable(task)
                && matches!(
                    canonical_task_status(&task.status),
                    TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
                )
                && !task_failure_is_dependency_blocked(task)
                && task_failure_is_transient(task)
        })
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    for task_id in &ids {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == *task_id) {
            record_task_retry(task, RETRY_MODE_TRANSIENT, None, "automatic retry");
        }
    }
    if ids.is_empty() {
        return 0;
    }
    run.status = "running".to_string();
    run.active = true;
    run.loop_started = false;
    run.paused_at = None;
    run.pause_reason = None;
    clear_board_abort_state(run);
    run.current_phase = Some("retry_requested".to_string());
    run.phase_started_at = Some(Utc::now());
    run.phase_details = Some(json!({
        "mode": RETRY_MODE_TRANSIENT,
        "taskIds": ids,
        "reason": "automatic retry",
    }));
    ids.len()
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn parse_optional_scheduled_start(value: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    parse_rfc3339_utc(value)
        .map(Some)
        .ok_or_else(|| bad_request("scheduledStartAt must be a valid RFC3339 timestamp"))
}

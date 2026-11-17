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
    let newly_changed = summary
        .get("attributableChangedFileCount")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| change_summary_paths(summary).len() as u64);
    let reused = summary
        .get("reusedEvidence")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry.get("classification").and_then(Value::as_str) == Some("reused_same_task")
        })
        .filter_map(|entry| entry.get("path").and_then(Value::as_str))
        .collect::<BTreeSet<_>>()
        .len() as u64;
    newly_changed.saturating_add(reused)
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
    run_deterministic_validation_with_output_limit(run, task_id, stage, 1200).await
}

async fn run_deterministic_validation_with_output_limit(
    run: &AgenticBoard,
    task_id: &str,
    stage: &str,
    output_limit: usize,
) -> Value {
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
        // The extra pre-attempt comparison must not rerun build, lint fixes,
        // migrations, or arbitrary configured commands. Only package test
        // suites are eligible; unsupported custom commands remain strict gates.
        if stage == "test_preflight"
            && !matches!(script_name.as_str(), "test" | "test:ui" | "test:e2e")
        {
            continue;
        }
        let command_started = Utc::now();
        let result =
            run_shell_validation_command(&run.project_path, &command_text, validation_timeout)
                .await;
        let duration_ms = (Utc::now() - command_started).num_milliseconds().max(0);
        let exit_code = result.as_ref().map(|result| result.0).unwrap_or(124);
        let output = result
            .map(|result| format!("{}\n{}", result.1, result.2))
            .unwrap_or_else(|error| error);
        let mut recorded_command = json!({
            "command": command_text,
            "scriptName": script_name,
            "exitCode": exit_code,
            "output": output,
            "durationMs": duration_ms,
        });
        // Extract identities from the complete server-observed report before
        // limiting the display transcript; missing tails must not look green.
        if let Some(report) = node_assertion_test_report(&recorded_command)
            .or_else(|| playwright_assertion_test_report(&recorded_command))
        {
            recorded_command["testReport"] = report;
        }
        recorded_command["output"] = json!(limit_text(&output, output_limit));
        commands.push(recorded_command);
        if exit_code != 0 && stage == "feature" && output_limit == 1200 {
            break;
        }
    }
    // A package-free project can deliberately use Node's built-in runner.
    // Discover only bounded, snapshot-owned tests; never interpret provider
    // prose as a shell command or install a runner to make validation work.
    if commands.is_empty()
        && validation_commands_for_stage(run, stage).is_empty()
        && validation_max_commands_for_stage(run, stage) > 0
    {
        if let Some(paths) = package_free_node_test_paths(run) {
            let command_started = Utc::now();
            let mut process = Command::new("node");
            process
                .args(["--test", "--test-reporter=tap", "--"])
                .args(paths.iter().map(|path| format!("./{path}")))
                .current_dir(&run.project_path)
                .env("PATH", augmented_user_path())
                .kill_on_drop(true)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let observed = timeout(validation_timeout, process.output()).await;
            let (exit_code, output) = match observed {
                Ok(Ok(output)) => (output.status.code().unwrap_or(1), format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))),
                Ok(Err(error)) => (127, format!("Failed to run built-in Node tests: {error}")),
                Err(_) => (124, format!("Built-in Node tests timed out after {} seconds", validation_timeout.as_secs())),
            };
            let mut recorded = json!({
                "command": format!("node --test --test-reporter=tap -- {}", paths.iter().map(|path| serde_json::to_string(&format!("./{path}")).unwrap()).collect::<Vec<_>>().join(" ")),
                "scriptName": "test", "runner": "node_builtin", "exitCode": exit_code,
                "output": output, "durationMs": (Utc::now() - command_started).num_milliseconds().max(0),
            });
            if let Some(report) = node_assertion_test_report(&recorded) {
                recorded["testReport"] = report;
            }
            recorded["output"] = json!(limit_text(&output, output_limit));
            commands.push(recorded);
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

fn node_builtin_test_path(path: &str) -> bool {
    [".test.js", ".test.cjs", ".test.mjs"]
        .iter()
        .any(|suffix| path.ends_with(suffix))
}

fn complete_validation_snapshot(snapshot: &Value) -> bool {
    snapshot.get("complete").and_then(Value::as_bool) == Some(true)
        || (snapshot.get("isGit").and_then(Value::as_bool) == Some(true)
            && snapshot.get("error").is_none()
            && snapshot.get("filesByPath").and_then(Value::as_object)
                .is_some_and(|files| files.len() < MAX_WORKSPACE_SNAPSHOT_FILES))
}

fn package_free_node_test_paths(run: &AgenticBoard) -> Option<Vec<String>> {
    use std::io::Read;
    if !run.tdd_enabled || !uses_hierarchical_orchestration(run) {
        return None;
    }
    let root = fs::canonicalize(&run.project_path).ok()?;
    if !matches!(fs::symlink_metadata(root.join("package.json")), Err(error) if error.kind() == std::io::ErrorKind::NotFound) {
        return None;
    }
    let snapshot = run.latest_workspace_snapshot.as_ref()?;
    if !complete_validation_snapshot(snapshot) { return None; }
    let files = snapshot.get("filesByPath")?.as_object()?;
    let candidates = files.keys().filter(|path| node_builtin_test_path(path)).collect::<Vec<_>>();
    if candidates.is_empty() || candidates.len() > 64 { return None; }
    let mut paths = Vec::new();
    for path in candidates {
        if !Path::new(path).components().all(|part| matches!(part, std::path::Component::Normal(_))) {
            return None;
        }
        let absolute = root.join(path);
        let metadata = fs::symlink_metadata(&absolute).ok()?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024
            || !fs::canonicalize(&absolute).ok()?.starts_with(&root)
            || hash_filesystem_snapshot_file(&absolute).ok()?.as_str() != files.get(path)?.get("hash")?.as_str()?
        { return None; }
        let mut source = String::new();
        fs::File::open(&absolute).ok()?.take(1024 * 1024 + 1).read_to_string(&mut source).ok()?;
        if source.len() > 1024 * 1024 || !(source.contains("'node:test'") || source.contains("\"node:test\"")) {
            return None;
        }
        paths.push(path.clone());
    }
    Some(paths)
}

async fn run_tdd_validation(run: &AgenticBoard, task: &BoardTask, stage: &str) -> Value {
    if stage == "feature"
        && (hierarchical_test_phase(run, task)
            || hierarchical_test_qa_source(run, task).is_some()
            || hierarchical_intermediate_qa_barrier(run, task).is_some())
    {
        let mut validation =
            run_deterministic_validation_with_output_limit(run, &task.id, stage, 65536).await;
        if !task.qa_test_commands.is_empty() {
            let generated = run_generated_test_commands(
                &run.project_path,
                &task.id,
                &task.qa_test_commands,
                stage,
                validation_timeout(run),
            )
            .await;
            merge_task_validation_commands(&mut validation, &generated);
        }
        return classify_hierarchical_test_validation(run, task, validation);
    }
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

// A test-writing branch may intentionally be red. This is not an exemption for
// implementation, QA, or unrelated manual checks, even when they have a test
// prerequisite. The manual child covers the separate baseline-recording ticket.
fn merge_task_validation_commands(validation: &mut Value, extra: &Value) {
    let mut commands = validation
        .get("commands")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    commands.extend(
        extra
            .get("commands")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    );
    validation["passed"] = json!(
        commands
            .iter()
            .all(|command| command.get("exitCode").and_then(Value::as_i64) == Some(0))
    );
    validation["commands"] = json!(commands);
}

fn hierarchical_test_phase(run: &AgenticBoard, task: &BoardTask) -> bool {
    if !run.tdd_enabled || !uses_hierarchical_orchestration(run) {
        return false;
    }
    match canonical_task_kind(task) {
        TASK_KIND_TEST_IMPLEMENTATION => return true,
        TASK_KIND_MANUAL_TEST
            if format!("{} {} {}", task.title, task.details, task.description)
                .to_ascii_lowercase()
                .contains("baseline") => {}
        _ => return false,
    }
    let mut parent_id = task.hierarchy.parent_id.as_deref();
    let mut seen = BTreeSet::new();
    while let Some(id) = parent_id {
        if !seen.insert(id) {
            break;
        }
        let Some(parent) = run.tasks.iter().find(|item| item.id == id) else {
            break;
        };
        if canonical_task_kind(parent) == TASK_KIND_TEST_IMPLEMENTATION {
            return true;
        }
        parent_id = parent.hierarchy.parent_id.as_deref();
    }
    false
}

// Only generated, source-linked QA can verify an existing red test stage.
// Ordinary/final QA and a mere dependency on tests still require green checks.
fn hierarchical_test_qa_source<'a>(
    run: &'a AgenticBoard,
    task: &BoardTask,
) -> Option<&'a BoardTask> {
    if !run.tdd_enabled
        || !uses_hierarchical_orchestration(run)
        || !is_qa_task(task)
        || task.final_qa_task
        || !matches!(
            task.task_origin.as_str(),
            "system_qa" | "system_qa_verdict_retry"
        )
    {
        return None;
    }
    let source = run
        .tasks
        .iter()
        .find(|item| Some(item.id.as_str()) == task.source_task_id.as_deref())?;
    if !task_status_is_done(&source.status)
        || !task_is_executable(source)
        || source.superseded_by.is_some()
        || !hierarchical_test_phase(run, source)
        || task.hierarchy.parent_id != source.hierarchy.parent_id
        || task.hierarchy.scope_version != source.hierarchy.scope_version
    {
        return None;
    }
    let baseline = source.qa_baseline_validation.as_ref()?;
    let fingerprint = hierarchical_validation_test_fingerprint(run)?;
    (baseline.get("expectedRedAccepted").and_then(Value::as_bool) == Some(true)
        && validation_gate_passed(baseline)
        && baseline.get("passed").and_then(Value::as_bool) == Some(false)
        && baseline.get("scopeVersion").and_then(Value::as_u64)
            == Some(source.hierarchy.scope_version)
        && baseline.get("testSuiteFingerprint") == Some(&fingerprint)
        && source.deterministic_validation.as_ref() == Some(baseline))
    .then_some(source)
}

fn trusted_test_qa_red_evidence(
    run: &AgenticBoard,
    task: &BoardTask,
    commands: &[Value],
) -> Option<Value> {
    let source = hierarchical_test_qa_source(run, task)?;
    let previous = source
        .qa_baseline_validation
        .as_ref()?
        .get("commands")?
        .as_array()?;
    if previous.len() != commands.len() || commands.is_empty() {
        return None;
    }
    let mut saw_red = false;
    for command in commands {
        let prior = previous.iter().find(|prior| {
            prior.get("command") == command.get("command")
                && prior.get("scriptName") == command.get("scriptName")
        })?;
        if command.get("exitCode") != prior.get("exitCode") {
            return None;
        }
        if command.get("exitCode").and_then(Value::as_i64) == Some(0) {
            continue;
        }
        let report = command.get("testReport")?;
        let prior_report = prior.get("testReport")?;
        if ["runner", "tests", "assertionFailures", "failedTestIds"]
            .iter()
            .any(|key| report.get(*key).is_none() || report.get(*key) != prior_report.get(*key))
        {
            return None;
        }
        if normalize_string_list(report.get("failedTestIds")).is_empty() {
            return None;
        }
        saw_red = true;
    }
    saw_red.then(|| {
        json!({
            "sourceTaskId": source.id,
            "sourceScopeVersion": source.hierarchy.scope_version,
            "unchangedTestSuite": true,
        })
    })
}

fn hierarchical_task_requires_red_baseline(run: &AgenticBoard, task: &BoardTask) -> bool {
    hierarchical_test_phase(run, task)
        && tdd_requires_failing_baseline(run)
        && format!("{} {} {}", task.title, task.details, task.description)
            .to_ascii_lowercase()
            .contains("baseline")
}

// Recognize actual Node test-runner assertion reports, not arbitrary nonzero
// exits or provider claims. Unsupported/truncated output fails closed. In
// particular missing imports, syntax errors, cancelled tests and absent runners
// are broken tests/environment, not demonstrated behavior-level red baselines.
fn node_assertion_test_report(command: &Value) -> Option<Value> {
    let exit_code = command.get("exitCode").and_then(Value::as_i64)?;
    if !matches!(exit_code, 0 | 1)
        || command.get("scriptName").and_then(Value::as_str) != Some("test")
    {
        return None;
    }
    let output = command.get("output")?.as_str()?;
    let counter = |name: &str| -> Option<u64> {
        output.lines().find_map(|line| {
            let line = line.trim();
            let line = line
                .strip_prefix("# ")
                .or_else(|| line.strip_prefix("ℹ "))?;
            line.strip_prefix(name)?
                .strip_prefix(' ')?
                .trim()
                .parse()
                .ok()
        })
    };
    let total = counter("tests")?;
    let failed = counter("fail")?;
    let passed = counter("pass")?;
    if total == 0
        || passed + failed != total
        || counter("cancelled")? != 0
        || counter("skipped")? != 0
        || counter("todo")? != 0
    {
        return None;
    }
    let error_codes = output
        .lines()
        .filter_map(|line| line.trim().strip_prefix("code:"))
        .map(|code| code.trim().trim_end_matches(',').trim_matches(['\'', '"']))
        .collect::<Vec<_>>();
    if error_codes.len() as u64 != failed || error_codes.iter().any(|code| *code != "ERR_ASSERTION")
    {
        return None;
    }
    if (failed == 0) != (exit_code == 0) {
        return None;
    }
    let mut failure_ids = BTreeSet::new();
    let mut spec_location = None;
    let mut tap_name = None;
    for line in output.lines() {
        let line = line.trim();
        if let Some(location) = line.strip_prefix("test at ") {
            spec_location = Some(location.to_string());
        } else if let Some(name) = line.strip_prefix("✖ ") {
            if let Some(location) = spec_location.take() {
                let name = name.rsplit_once(" (").map(|(name, _)| name).unwrap_or(name);
                failure_ids.insert(format!("{location}::{name}"));
            }
        } else if let Some(name) = line.strip_prefix("not ok ") {
            tap_name = name.split_once(" - ").map(|(_, name)| name.to_string());
        } else if let Some(location) = line.strip_prefix("location: ") {
            if let Some(name) = tap_name.take() {
                let location = location.trim_matches(['\'', '"']);
                failure_ids.insert(format!("{location}::{name}"));
            }
        }
    }
    if failure_ids.len() as u64 != failed {
        return None;
    }
    Some(json!({
        "runner": "node:test",
        "tests": total,
        "passed": passed,
        "assertionFailures": failed,
        "actualExitCode": exit_code,
        "failedTestIds": failure_ids,
    }))
}

// Playwright can embed SGR color/style escapes inside matcher names in its JSON
// error messages. Normalize only a classification copy, never the saved output.
// Leave malformed escapes and non-SGR controls intact so they fail closed.
fn assertion_message_without_ansi_sgr(message: &str) -> String {
    let mut normalized = String::with_capacity(message.len());
    let mut remaining = message;
    while let Some(start) = remaining.find("\u{1b}[") {
        normalized.push_str(&remaining[..start]);
        let parameters = &remaining[start + 2..];
        let end = parameters
            .find(|ch: char| !ch.is_ascii_digit() && ch != ';' && ch != ':')
            .unwrap_or(parameters.len());
        if parameters.as_bytes().get(end) == Some(&b'm') {
            remaining = &parameters[end + 1..];
        } else {
            normalized.push_str("\u{1b}[");
            remaining = parameters;
        }
    }
    normalized.push_str(remaining);
    normalized
}

fn playwright_assertion_test_report(command: &Value) -> Option<Value> {
    let exit_code = command.get("exitCode").and_then(Value::as_i64)?;
    if !matches!(exit_code, 0 | 1)
        || !matches!(
            command.get("scriptName").and_then(Value::as_str),
            Some("test" | "test:ui" | "test:e2e")
        )
    {
        return None;
    }
    let output = command.get("output")?.as_str()?;
    // Do not let a later JSON report hide an earlier failing Node suite.
    let json_start = output.find('{')?;
    if output[..json_start].contains("ERR_ASSERTION") {
        return None;
    }
    let report = parse_json_object(output)?;
    if !report.get("errors")?.as_array()?.is_empty() {
        return None;
    }
    let stats = report.get("stats")?;
    let expected = stats.get("expected")?.as_u64()?;
    let unexpected = stats.get("unexpected")?.as_u64()?;
    if expected + unexpected == 0
        || stats.get("skipped")?.as_u64()? != 0
        || stats.get("flaky")?.as_u64()? != 0
        || (unexpected == 0) != (exit_code == 0)
    {
        return None;
    }
    let mut pending = report.get("suites")?.as_array()?.iter().collect::<Vec<_>>();
    let mut passed = 0;
    let mut failure_ids = BTreeSet::new();
    while let Some(suite) = pending.pop() {
        if let Some(children) = suite.get("suites").and_then(Value::as_array) {
            pending.extend(children);
        }
        for spec in suite.get("specs")?.as_array()? {
            for test in spec.get("tests")?.as_array()? {
                if test.get("expectedStatus")?.as_str()? != "passed" {
                    return None;
                }
                let results = test.get("results")?.as_array()?;
                if results.len() != 1 {
                    return None;
                }
                let result = &results[0];
                match result.get("status")?.as_str()? {
                    "passed" => passed += 1,
                    "failed" => {
                        let errors = result.get("errors")?.as_array()?;
                        if errors.is_empty()
                            || errors.iter().any(|error| {
                                let message = assertion_message_without_ansi_sgr(
                                    error.get("message").and_then(Value::as_str).unwrap_or(""),
                                );
                                !message.contains("expect(")
                                    || ![
                                        ".toBe",
                                        ".toHave",
                                        ".toEqual",
                                        ".toStrictEqual",
                                        ".toContain",
                                        ".toMatch",
                                        ".toThrow",
                                    ]
                                    .iter()
                                    .any(|matcher| message.contains(matcher))
                                    || message.contains("Test timeout of")
                                    || message.contains("browserType.launch")
                                    || message.contains("ERR_CONNECTION_REFUSED")
                            })
                        {
                            return None;
                        }
                        failure_ids.insert(format!(
                            "{}:{}:{}::{}::{}",
                            spec.get("file")?.as_str()?,
                            spec.get("line")?.as_u64()?,
                            spec.get("column")?.as_u64()?,
                            spec.get("title")?.as_str()?,
                            test.get("projectName")?.as_str()?,
                        ));
                    }
                    _ => return None,
                }
            }
        }
    }
    if passed != expected || failure_ids.len() as u64 != unexpected {
        return None;
    }
    Some(json!({
        "runner": "playwright",
        "tests": expected + unexpected,
        "passed": expected,
        "assertionFailures": unexpected,
        "actualExitCode": exit_code,
        "failedTestIds": failure_ids,
    }))
}

fn hierarchical_validation_test_fingerprint(run: &AgenticBoard) -> Option<Value> {
    let snapshot = run.latest_workspace_snapshot.as_ref()?;
    let files = snapshot.get("filesByPath")?.as_object()?;
    if !complete_validation_snapshot(snapshot) {
        return None;
    }
    let mut hashes = serde_json::Map::new();
    for (path, file) in files {
        if path == "package.json"
            || matches!(
                path.as_str(),
                "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "bun.lock" | "bun.lockb"
            )
            || path.starts_with("playwright.config.")
            || is_test_or_fixture_path(path)
        {
            hashes.insert(path.clone(), json!(file.get("hash")?.as_str()?));
        }
    }
    let supported_suite = (hashes.len() > 1 && hashes.contains_key("package.json"))
        || (!hashes.contains_key("package.json") && hashes.keys().any(|path| node_builtin_test_path(path)));
    supported_suite.then(|| Value::Object(hashes))
}

fn hierarchical_task_depends_on(run: &AgenticBoard, task: &BoardTask, target: &str) -> bool {
    let mut pending = task_blockers(task);
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if id == target {
            return true;
        }
        if seen.insert(id.clone()) {
            if let Some(blocker) = run.tasks.iter().find(|item| item.id == id) {
                pending.extend(task_blockers(blocker));
            }
        }
    }
    false
}

// Only a genuinely intermediate implementation slice can leave previously
// observed red tests for its dependent sibling. The last implementation and the
// explicit QA barrier must still make the complete suite green.
fn hierarchical_intermediate_qa_barrier(run: &AgenticBoard, task: &BoardTask) -> Option<String> {
    if !run.tdd_enabled
        || !uses_hierarchical_orchestration(run)
        || canonical_task_kind(task) != TASK_KIND_IMPLEMENTATION
        || task.hierarchy.parent_id.is_none()
    {
        return None;
    }
    let sibling = run.tasks.iter().find(|item| {
        item.id != task.id
            && item.hierarchy.parent_id == task.hierarchy.parent_id
            && canonical_task_kind(item) == TASK_KIND_IMPLEMENTATION
            && mandatory_live_validation_barrier(run, item)
            && hierarchical_task_depends_on(run, item, &task.id)
    })?;
    run.tasks
        .iter()
        .find(|item| {
            item.hierarchy.parent_id == task.hierarchy.parent_id
                && canonical_task_kind(item) == TASK_KIND_QA
                && mandatory_live_validation_barrier(run, item)
                && hierarchical_task_depends_on(run, item, &task.id)
                && hierarchical_task_depends_on(run, item, &sibling.id)
        })
        .map(|item| item.id.clone())
}

fn mandatory_live_validation_barrier(run: &AgenticBoard, task: &BoardTask) -> bool {
    let dependency_wait = canonical_task_status(&task.status) == TASK_STATUS_BLOCKED
        && task
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("Waiting on dependency: "))
        && task
            .hierarchy
            .scope_approval
            .as_ref()
            .is_some_and(|approval| {
                approval.approved && approval.scope_version == task.hierarchy.scope_version.max(1)
            })
        && !unmet_task_dependencies(run, task).is_empty()
        && task_blockers(task).iter().all(|id| {
            run.tasks
                .iter()
                .any(|dependency| dependency.id == *id && dependency.superseded_by.is_none())
        })
        && task.result.as_ref().is_none_or(|result| {
            !completion_evidence_gate_failed(result)
                && !matches!(
                    result.get("status").and_then(Value::as_str),
                    Some("blocked" | "needs_followup")
                )
        });
    task.hierarchy.required
        && task_is_executable(task)
        && task.superseded_by.is_none()
        && (task_scope_is_approved(task) || dependency_wait)
        && task_side_effects_are_approved(task)
        && task_ancestors_are_approved(run, task)
}

fn hierarchical_baseline_is_prerequisite(
    run: &AgenticBoard,
    task: &BoardTask,
    baseline: &BoardTask,
) -> bool {
    let mut candidate = Some(baseline.id.as_str());
    let mut seen = BTreeSet::new();
    while let Some(id) = candidate {
        if !seen.insert(id) {
            break;
        }
        if hierarchical_task_depends_on(run, task, id) {
            return true;
        }
        let Some(parent) = run.tasks.iter().find(|item| item.id == id) else {
            break;
        };
        candidate = parent.hierarchy.parent_id.as_deref();
    }
    false
}

fn trusted_remaining_red_baseline(
    run: &AgenticBoard,
    task: &BoardTask,
    commands: &[Value],
) -> Option<Value> {
    let barrier = hierarchical_intermediate_qa_barrier(run, task)?;
    let fingerprint = hierarchical_validation_test_fingerprint(run)?;
    let attempt = task.hierarchy.attempts.last()?;
    if attempt.get("scopeVersion").and_then(Value::as_u64)
        != Some(u64::from(task.hierarchy.scope_version))
        || attempt.get("testSuiteFingerprint") != Some(&fingerprint)
    {
        return None;
    }
    let pre_commands = attempt
        .get("preExecutionValidation")?
        .get("commands")?
        .as_array()?;
    let baseline = run
        .tasks
        .iter()
        .filter(|item| {
            task_status_is_done(&item.status)
                && hierarchical_test_phase(run, item)
                && item.superseded_by.is_none()
                && hierarchical_baseline_is_prerequisite(run, task, item)
                && item
                    .qa_baseline_validation
                    .as_ref()
                    .is_some_and(|baseline| {
                        baseline.get("expectedRedAccepted").and_then(Value::as_bool) == Some(true)
                            && baseline.get("testSuiteFingerprint") == Some(&fingerprint)
                            && baseline.get("scopeVersion").and_then(Value::as_u64)
                                == Some(u64::from(item.hierarchy.scope_version))
                            && item.deterministic_validation.as_ref() == Some(baseline)
                    })
        })
        .max_by_key(|item| item.completed_at)?;
    let baseline_commands = baseline
        .qa_baseline_validation
        .as_ref()?
        .get("commands")?
        .as_array()?;
    let mut failures = 0;
    for command in commands {
        if command.get("exitCode").and_then(Value::as_i64) == Some(0) {
            continue;
        }
        let report = command.get("testReport")?;
        let current_ids = normalize_string_list(report.get("failedTestIds"));
        if current_ids.is_empty() {
            return None;
        }
        let baseline_command = baseline_commands.iter().find(|previous| {
            previous.get("command") == command.get("command")
                && previous.get("scriptName") == command.get("scriptName")
        })?;
        let previous_ids =
            normalize_string_list(baseline_command.get("testReport")?.get("failedTestIds"));
        let pre_command = pre_commands.iter().find(|previous| {
            previous.get("command") == command.get("command")
                && previous.get("scriptName") == command.get("scriptName")
        })?;
        let pre_ids = normalize_string_list(pre_command.get("testReport")?.get("failedTestIds"));
        if pre_ids.iter().any(|id| !previous_ids.contains(id))
            || current_ids.iter().any(|id| !pre_ids.contains(id))
        {
            return None;
        }
        // A production slice must demonstrate actual progress, not merely
        // preserve a completely red suite and declare itself implemented.
        if current_ids.len() >= pre_ids.len() {
            return None;
        }
        failures += current_ids.len();
    }
    (failures > 0).then(|| {
        json!({
            "baselineTaskId": baseline.id,
            "qaBarrierTaskId": barrier,
            "remainingKnownAssertionFailures": failures,
            "preAttemptValidationCompared": true,
        })
    })
}

fn classify_hierarchical_test_validation(
    run: &AgenticBoard,
    task: &BoardTask,
    mut validation: Value,
) -> Value {
    let commands = validation
        .get("commands")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let failed = commands
        .iter()
        .filter(|command| command.get("exitCode").and_then(Value::as_i64) != Some(0))
        .collect::<Vec<_>>();
    let red_evidence = failed
        .iter()
        .map(|command| command.get("testReport").cloned())
        .collect::<Option<Vec<_>>>();
    let test_phase = hierarchical_test_phase(run, task);
    let test_qa_evidence = trusted_test_qa_red_evidence(run, task, &commands);
    let expected_red = (test_phase || test_qa_evidence.is_some())
        && !failed.is_empty()
        && red_evidence.is_some();
    let requires_red = hierarchical_task_requires_red_baseline(run, task);
    let remaining_red = trusted_remaining_red_baseline(run, task, &commands);
    let fingerprint = hierarchical_validation_test_fingerprint(run);
    if let Some(object) = validation.as_object_mut() {
        object.insert(
            "scopeVersion".to_string(),
            json!(task.hierarchy.scope_version),
        );
        object.insert(
            "testPhase".to_string(),
            json!(if test_phase {
                "test_writing"
            } else if test_qa_evidence.is_some() {
                "test_validation"
            } else {
                "implementation"
            }),
        );
        object.insert("expectedRedRequired".to_string(), json!(requires_red));
        object.insert("expectedRedAccepted".to_string(), json!(expected_red));
        if expected_red {
            object.insert("gatePassed".to_string(), json!(true));
            object.insert("redBaselineEvidence".to_string(), json!(red_evidence));
            object.insert("testSuiteFingerprint".to_string(), json!(fingerprint));
            if let Some(evidence) = test_qa_evidence {
                object.insert("testStageQaEvidence".to_string(), evidence);
            }
            object.insert("summary".to_string(), json!(
                "Expected assertion failures verified for the test-writing phase; the test suite remains red."
            ));
        } else if let Some(evidence) = remaining_red {
            object.insert("gatePassed".to_string(), json!(true));
            object.insert("remainingRedAccepted".to_string(), json!(evidence));
            object.insert("summary".to_string(), json!(
                "Only unchanged, previously observed assertion failures remain for dependent implementation; the suite is not green and downstream QA is still required."
            ));
        } else if requires_red && failed.is_empty() {
            object.insert("gatePassed".to_string(), json!(false));
            object.insert("failureReason".to_string(), json!(
                "The baseline ticket requires executed, failing behavior assertions; no verified red baseline was observed."
            ));
        }
    }
    validation
}

fn apply_task_validation_result(parsed: Value, validation: &Value) -> Value {
    let mut gate_validation = validation.clone();
    gate_validation["passed"] = json!(validation_gate_passed(validation));
    let mut parsed = apply_deterministic_validation_result(parsed, &gate_validation);
    if let Some(object) = parsed.as_object_mut() {
        object.insert("deterministicValidation".to_string(), validation.clone());
        let gate_summary = format_validation_check(&gate_validation);
        let actual_summary = format_validation_check(validation);
        let evidence = normalize_string_list(object.get("evidence"))
            .into_iter()
            .map(|line| {
                if line == gate_summary {
                    actual_summary.clone()
                } else {
                    line
                }
            })
            .collect::<Vec<_>>();
        object.insert("evidence".to_string(), json!(evidence));
        if let Some(summary) = validation.get("summary").and_then(Value::as_str) {
            let mut evidence = normalize_string_list(object.get("evidence"));
            evidence.push(summary.to_string());
            object.insert("evidence".to_string(), json!(dedupe_strings(evidence)));
        }
    }
    if let Some(reason) = validation.get("failureReason").and_then(Value::as_str) {
        if let Some(object) = parsed.as_object_mut() {
            let mut issues = normalize_string_list(object.get("remainingIssues"));
            issues.push(reason.to_string());
            object.insert("remainingIssues".to_string(), json!(dedupe_strings(issues)));
        }
    }
    parsed
}

fn validation_gate_passed(validation: &Value) -> bool {
    validation
        .get("gatePassed")
        .or_else(|| validation.get("passed"))
        .and_then(Value::as_bool)
        == Some(true)
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
    let candidates: &[&str] = if run.tdd_enabled && uses_hierarchical_orchestration(run) {
        &[
            "typecheck",
            "test",
            "test:ui",
            "test:e2e",
            "lint",
            "build",
            "check",
        ]
    } else if stage == "final" {
        &["typecheck", "lint", "test", "build", "check"]
    } else {
        &["typecheck", "test", "lint", "build", "check"]
    };
    candidates
        .iter()
        .copied()
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
    reconcile_scope_approval_records(run);
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
        if !task_ancestors_are_approved(run, task) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                format!(
                    "Task {} has an unapproved, completed, or independently blocked parent; resolve the parent before retrying it.",
                    task.id
                ),
            ));
        }
        if !task_side_effects_are_approved(task) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                task_side_effect_block_reason(task),
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
    refresh_hierarchy_rollups(run);
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
            && task_ancestors_are_approved(run, task)
            && task_side_effects_are_approved(task)
    })
}

fn reset_attention_tasks_for_retry(run: &mut AgenticBoard) -> usize {
    reconcile_scope_approval_records(run);
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
                && task_ancestors_are_approved(run, task)
                && task_side_effects_are_approved(task)
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
    refresh_hierarchy_rollups(run);
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


/// This function must run inside mutate_board's lock. Validate every precondition
/// before assigning anything, so a late-section conflict cannot partly save.
fn apply_guarded_quality_settings(run: &mut AgenticBoard, body: &Value) -> Result<()> {
    let request = body.as_object().ok_or_else(|| bad_request("Quality save must be an object"))?;
    if request.len() != 2 || !request.contains_key("changes") || !request.contains_key("expected") {
        return Err(bad_request("Quality save requires only changes and expected objects"));
    }
    let changes = request["changes"].as_object().ok_or_else(|| bad_request("changes must be an object"))?;
    let expected = request["expected"].as_object().ok_or_else(|| bad_request("expected must be an object"))?;
    if changes.is_empty() || changes.keys().ne(expected.keys()) {
        return Err(bad_request("changes and expected must have the same nonempty field shape"));
    }
    let current = json!({
        "tddEnabled": run.tdd_enabled,
        "tddPolicy": run.tdd_policy,
        "validationConfig": run.validation_config,
        "ragSettings": run.rag_settings,
        "qaPolicy": run.qa_policy,
    });
    for (section, delta) in changes {
        let before = &expected[section];
        if section == "tddEnabled" {
            if !delta.is_boolean() || !before.is_boolean() {
                return Err(bad_request("tddEnabled must be boolean in changes and expected"));
            }
            continue;
        }
        if !matches!(section.as_str(), "tddPolicy" | "validationConfig" | "ragSettings" | "qaPolicy") {
            return Err(bad_request(format!("Unsupported quality field: {section}")));
        }
        let fields = delta.as_object().ok_or_else(|| bad_request(format!("{section} must be an object")))?;
        let guards = before.as_object().ok_or_else(|| bad_request(format!("expected.{section} must be an object")))?;
        if fields.is_empty() || fields.keys().ne(guards.keys()) {
            return Err(bad_request(format!("{section} requires the same nonempty field shape in changes and expected")));
        }
        for (field, value) in fields {
            validate_quality_field(section, field, value)?;
            validate_quality_field(section, field, &guards[field])?;
        }
    }
    // Separate validation and comparison passes: malformed requests are always 400.
    for (section, delta) in changes {
        if let Some(fields) = delta.as_object() {
            for field in fields.keys() {
                if current[section].get(field) != expected[section].get(field) {
                    return Err(quality_settings_conflict(&format!("{section}.{field}")));
                }
            }
        } else if current.get(section) != expected.get(section) {
            return Err(quality_settings_conflict(section));
        }
    }
    if let Some(value) = changes.get("tddEnabled") {
        run.tdd_enabled = value.as_bool().expect("validated boolean");
    }
    if let Some(delta) = changes.get("tddPolicy") {
        run.tdd_policy = normalize_tdd_policy(Some(&merge_json_objects(run.tdd_policy.clone(), delta.clone())));
    }
    if let Some(delta) = changes.get("validationConfig") {
        run.validation_config = normalize_validation_config(Some(&merge_json_objects(run.validation_config.clone(), delta.clone())));
    }
    if let Some(delta) = changes.get("ragSettings") {
        run.rag_settings = normalize_rag_settings(Some(&merge_json_objects(run.rag_settings.clone(), delta.clone())));
        run.rag_enabled = rag_enabled_from_settings(&run.rag_settings);
    }
    if let Some(delta) = changes.get("qaPolicy") {
        run.qa_policy = normalize_qa_policy(Some(&merge_json_objects(run.qa_policy.clone(), delta.clone())));
    }
    run.append_log("Updated guarded board quality settings");
    Ok(())
}

fn quality_settings_conflict(field: &str) -> ServerError {
    ServerError::new(StatusCode::CONFLICT, format!(
        "Quality settings changed in another client ({field}). Reload saved settings before saving again."
    ))
}

fn validate_quality_field(section: &str, field: &str, value: &Value) -> Result<()> {
    let valid = match (section, field) {
        ("tddPolicy", "requireFailingTestBeforeDev" | "allowImplementationWithoutTests") |
        ("validationConfig", "enabled") |
        ("ragSettings", "enabled" | "indexOnBootstrap" | "queryEnabled" | "ingestTaskResults" | "ingestValidationErrors") |
        ("qaPolicy", "repairMalformedToolCalls") => value.is_boolean(),
        ("tddPolicy", "maxFixAttempts") |
        ("validationConfig", "maxFeatureCommands" | "maxFinalCommands" | "maxQaCommands") |
        ("qaPolicy", "maxFollowupsPerGroup") => value.as_u64().is_some_and(|v| v <= 20),
        ("validationConfig", "timeoutSeconds") => value.as_u64().is_some_and(|v| (5..=3600).contains(&v)),
        ("ragSettings", "contextMaxChars") => value.as_u64().is_some_and(|v| (1000..=80000).contains(&v)),
        ("qaPolicy", "maxTaskAttempts") => value.as_u64().is_some_and(|v| (1..=10).contains(&v)),
        ("qaPolicy", "malformedToolCallRepairRetries") => value.as_u64().is_some_and(|v| v <= MAX_MALFORMED_TOOL_CALL_REPAIR_RETRIES),
        ("validationConfig", "featureCommands" | "finalCommands" | "qaCommands") |
        ("ragSettings", "scopes") => value.as_array().is_some_and(|items| items.iter().all(|item| item.as_str().is_some_and(|s| !s.trim().is_empty()))),
        ("tddPolicy", "qaCommandStage" | "featureCommandStage" | "finalCommandStage") => value.is_string(),
        ("qaPolicy", "taskQaMode") => value.as_str().is_some_and(|v| matches!(v, "off" | "all" | "high_risk")),
        _ => false,
    };
    if valid { Ok(()) } else { Err(bad_request(format!("Unsupported or invalid quality field: {section}.{field}"))) }
}

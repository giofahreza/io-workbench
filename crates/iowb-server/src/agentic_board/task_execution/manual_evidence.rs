// Structural evidence is necessary, not proof that an agent's observations are
// truthful. We bind evidence to approved scope and real local artifacts rather
// than attempting to infer UI execution from English summaries or shell names.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManualVerificationScope {
    version: u64,
    mode: String,
    #[serde(default)]
    input_modality: Option<String>,
    #[serde(default)]
    viewports: Vec<ManualVerificationViewport>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ManualVerificationViewport {
    width: u64,
    height: u64,
}

const MANUAL_VERIFICATION_CONTRACT_ERROR: &str = "Manual verification requires an approved manualVerification contract: {version:1, mode:ui|cli|api|inspection, viewports:[{width,height}]}. UI scope requires concrete positive viewports. Optional inputModality is any|touch; touch is valid only for UI and requires capability and input-event observations. Move the unfinished scope to Backlog, edit this field through task discussion/edit, and re-approve; do not downgrade UI acceptance to inspection. Existing Done history is not rewritten.";

// These are structured provider observations, not authenticated execution
// receipts. They reject missing/contradictory modality claims without guessing
// intent from prose or treating screenshots as proof of touch input.
fn manual_touch_capability(environment: &Value) -> bool {
    match environment.get("surface").and_then(Value::as_str) {
        Some("browser") => environment.pointer("/inputCapabilities/maxTouchPoints")
            .and_then(Value::as_u64).is_some_and(|points| points > 0),
        Some("native") => environment.pointer("/inputCapabilities/touchscreen")
            .and_then(Value::as_bool) == Some(true),
        _ => false,
    }
}

fn manual_touch_action_observed(action: &Value, surface: Option<&str>) -> bool {
    let action_type = action.get("type").and_then(Value::as_str);
    if !matches!(action_type, Some("tap" | "scroll")) { return false; }
    let observation = action.get("inputObservation").unwrap_or(&Value::Null);
    match surface {
        Some("browser") => {
            observation.get("pointerType").and_then(Value::as_str) == Some("touch")
                && observation.get("isTrusted").and_then(Value::as_bool) == Some(true)
                && matches!((action_type, observation.get("eventType").and_then(Value::as_str)),
                    (Some("tap"), Some("pointerdown" | "pointerup")) |
                    (Some("scroll"), Some("pointermove")))
        }
        Some("native") => {
            // Android InputDevice.SOURCE_TOUCHSCREEN, as observed on MotionEvent.
            observation.get("motionEventSource").and_then(Value::as_u64) == Some(4098)
                && matches!((action_type, observation.get("action").and_then(Value::as_str)),
                    (Some("tap"), Some("down" | "up")) |
                    (Some("scroll"), Some("move")))
        }
        _ => false,
    }
}

fn approved_manual_verification_scope(value: &Value) -> Option<ManualVerificationScope> {
    serde_json::from_value::<ManualVerificationScope>(value.clone())
        .ok()
        .filter(|scope| {
            scope.version == 1
                && matches!(scope.mode.as_str(), "ui" | "cli" | "api" | "inspection")
                && matches!(scope.input_modality.as_deref(), None | Some("any" | "touch"))
                && (scope.input_modality.as_deref() != Some("touch") || scope.mode == "ui")
                && (scope.mode != "ui" || !scope.viewports.is_empty())
                && scope.viewports.iter().all(|size| size.width > 0 && size.height > 0)
        })
}

// Authoring boundary only. Never apply this to persisted/approved tasks or
// completion evidence: those must be repaired and re-approved by the user.
fn normalize_supplied_manual_verification(value: Option<Value>) -> Result<Option<Value>> {
    let Some(mut value) = value.filter(|value| !value.is_null()) else { return Ok(None); };
    if let Some(viewports) = value.get_mut("viewports").and_then(Value::as_array_mut) {
        for viewport in viewports {
            if let Some(dimensions) = viewport.as_str() {
                let dimension = |text: &str| {
                    (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
                        .then(|| text.parse::<u64>().ok()).flatten().filter(|value| *value > 0)
                };
                let (width, height) = dimensions.split_once('x')
                    .and_then(|(width, height)| Some((dimension(width)?, dimension(height)?)))
                    .ok_or_else(|| bad_request(MANUAL_VERIFICATION_CONTRACT_ERROR))?;
                *viewport = json!({"width":width,"height":height});
            }
        }
    }
    if approved_manual_verification_scope(&value).is_none() {
        return Err(bad_request(MANUAL_VERIFICATION_CONTRACT_ERROR));
    }
    Ok(Some(value))
}

fn normalize_generated_manual_contracts(mut items: Vec<Value>) -> Result<Vec<Value>> {
    // Validate the whole batch before callers allocate IDs or persist children.
    for item in &mut items {
        let normalized = normalize_supplied_manual_verification(item.get("manualVerification").cloned())?;
        let title = item.get("title").and_then(Value::as_str).unwrap_or("");
        let details = item.get("details").or_else(|| item.get("description")).and_then(Value::as_str).unwrap_or(title);
        let criteria = normalize_string_list(item.get("acceptanceCriteria"));
        if normalized.is_none()
            && (prompt_task_kind_from_value(item, title, details, &criteria) == TASK_KIND_MANUAL_TEST
                || item.get("sourceKind").and_then(Value::as_str).and_then(normalized_task_kind_name) == Some(TASK_KIND_MANUAL_TEST))
        {
            return Err(bad_request(MANUAL_VERIFICATION_CONTRACT_ERROR));
        }
        if let Some(scope) = normalized {
            item["manualVerification"] = scope;
        }
    }
    Ok(items)
}

fn normalize_hierarchy_manual_contracts(mut items: Vec<Value>, parent: &BoardTask) -> Result<Vec<Value>> {
    if canonical_task_kind(parent) == TASK_KIND_MANUAL_TEST
        && let Some(scope) = &parent.hierarchy.manual_verification
    {
        // A child cannot shrink an inherited approval, including when a model
        // proposes inspection or a single viewport for a multi-viewport scope.
        // Existing malformed parent scope must be explicitly repaired first.
        if approved_manual_verification_scope(scope).is_none() {
            return Err(bad_request(MANUAL_VERIFICATION_CONTRACT_ERROR));
        }
        for item in &mut items {
            let title = item.get("title").and_then(Value::as_str).unwrap_or("");
            let details = item.get("details").or_else(|| item.get("description")).and_then(Value::as_str).unwrap_or(title);
            let criteria = normalize_string_list(item.get("acceptanceCriteria"));
            if prompt_task_kind_from_value(item, title, details, &criteria) == TASK_KIND_MANUAL_TEST
                || item.get("sourceKind").and_then(Value::as_str).and_then(normalized_task_kind_name) == Some(TASK_KIND_MANUAL_TEST)
            {
                item["manualVerification"] = scope.clone();
            }
        }
    }
    normalize_generated_manual_contracts(items)
}

fn manual_task_contract_issue(task: &BoardTask) -> Option<&'static str> {
    (canonical_task_kind(task) == TASK_KIND_MANUAL_TEST
        && task.hierarchy.manual_verification.as_ref()
            .and_then(approved_manual_verification_scope).is_none())
        .then_some(MANUAL_VERIFICATION_CONTRACT_ERROR)
}

fn manual_runtime_attempt_identity(task: &BoardTask) -> Value {
    json!({"taskId":task.id,"scopeVersion":task.hierarchy.scope_version,
        "attemptCount":task.attempt_count,"startedAt":task.started_at})
}

fn bind_manual_runtime_snapshot(snapshot: &mut Value, task: &BoardTask) {
    if canonical_task_kind(task) == TASK_KIND_MANUAL_TEST {
        snapshot["manualRuntimeAttempt"] = manual_runtime_attempt_identity(task);
    }
}

fn manual_mcp_snapshot_bytes(project_path: &str, reference: &str, expected_hash: &str) -> Option<usize> {
    let file_name = reference.strip_prefix(".playwright-mcp/")?;
    let (timestamp, console_log) = if let Some(timestamp) = file_name.strip_prefix("page-").and_then(|name| name.strip_suffix(".yml")) {
        (timestamp, false)
    } else {
        (file_name.strip_prefix("console-")?.strip_suffix(".log")?, true)
    };
    let timestamp_template = b"0000-00-00T00-00-00-000Z";
    if timestamp.len() != timestamp_template.len()
        || !timestamp.bytes().zip(timestamp_template).all(|(byte, template)| {
            if *template == b'0' { byte.is_ascii_digit() } else { byte == *template }
        })
        || chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H-%M-%S-%3fZ").is_err()
    {
        return None;
    }
    let project = fs::canonicalize(project_path).ok()?;
    let directory = project.join(".playwright-mcp");
    let path = directory.join(file_name);
    // Reject symlinks even when their destination is inside the workspace.
    if !fs::symlink_metadata(&directory).ok()?.is_dir()
        || fs::canonicalize(&directory).ok()? != directory
        || !fs::symlink_metadata(&path).ok()?.is_file()
        || fs::canonicalize(&path).ok()? != path
    {
        return None;
    }
    let mut file = std::io::Read::take(fs::File::open(&path).ok()?, 1_048_577);
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).ok()?;
    if bytes.len() > 1_048_576 || format!("{:x}", Sha256::digest(&bytes)) != expected_hash {
        return None;
    }
    let content = std::str::from_utf8(&bytes).ok()?;
    if content.chars().any(|character| character.is_control() && character != '\n' && character != '\r') {
        return None;
    }
    if console_log {
        // Automatically emitted browser diagnostics, not arbitrary .log files.
        // Keep the same creation-only attribution, path/hash and size bounds as
        // snapshots. These diagnostics never count as manual evidence.
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            let (elapsed, message) = line.strip_prefix('[')?.split_once("ms] [")?;
            let elapsed = elapsed.trim_start_matches(' ');
            if elapsed.is_empty() || !elapsed.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let (level, _) = message.split_once("] ")?;
            if !matches!(level, "ERROR" | "WARNING" | "INFO" | "LOG" | "DEBUG" | "ASSERT" | "TRACE" | "STARTGROUP" | "STARTGROUPCOLLAPSED" | "ENDGROUP" | "CLEAR" | "COUNT" | "DIR" | "DIRXML" | "TABLE" | "TIMEEND" | "PROFILE" | "PROFILEEND") {
                return None;
            }
        }
        return Some(bytes.len());
    }
    // Playwright writes an empty page snapshot while navigation is pending.
    // Otherwise accept only its accessibility-tree list shape, not arbitrary
    // YAML/config/code. This output is permitted runtime data, NEVER evidence
    // that any acceptance criterion passed or that screenshots were inspected.
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let text = line.trim_start_matches(' ');
        let indent = line.len() - text.len();
        let entry = text.strip_prefix("- ")?;
        let role = entry.split([' ', ':']).next()?;
        if indent % 2 != 0 || !matches!(role,
            "alert" | "alertdialog" | "application" | "article" | "banner" | "blockquote"
            | "button" | "caption" | "cell" | "checkbox" | "code" | "columnheader" | "combobox"
            | "complementary" | "contentinfo" | "definition" | "deletion" | "dialog" | "directory"
            | "document" | "emphasis" | "feed" | "figure" | "form" | "generic" | "grid" | "gridcell"
            | "group" | "heading" | "img" | "insertion" | "link" | "list" | "listbox" | "listitem"
            | "log" | "main" | "marquee" | "math" | "menu" | "menubar" | "menuitem"
            | "menuitemcheckbox" | "menuitemradio" | "meter" | "navigation" | "note" | "option"
            | "paragraph" | "presentation" | "progressbar" | "radio" | "radiogroup" | "region"
            | "row" | "rowgroup" | "rowheader" | "scrollbar" | "search" | "searchbox" | "separator"
            | "slider" | "spinbutton" | "status" | "strong" | "subscript" | "superscript" | "switch"
            | "tab" | "table" | "tablist" | "tabpanel" | "term" | "text" | "textbox" | "time"
            | "timer" | "toolbar" | "tooltip" | "tree" | "treegrid" | "treeitem"
            | "/url" | "/placeholder")
        {
            return None;
        }
    }
    Some(bytes.len())
}

fn allowed_manual_mcp_snapshots(run: &AgenticBoard, task: &BoardTask, delta: &Value) -> BTreeSet<String> {
    if canonical_task_kind(task) != TASK_KIND_MANUAL_TEST
        || task.attempt_count == 0
        || task.started_at.is_none()
        || delta.get("taskId").and_then(Value::as_str) != Some(task.id.as_str())
        || delta.get("scopeVersion").and_then(Value::as_u64) != Some(task.hierarchy.scope_version)
        || delta.get("manualRuntimeAttempt") != Some(&manual_runtime_attempt_identity(task))
        || delta.get("snapshotProvider").and_then(Value::as_str) != Some("filesystem sha256")
        || delta.get("snapshotComplete").and_then(Value::as_bool) != Some(true)
    {
        return BTreeSet::new();
    }
    let mut allowed = BTreeSet::new();
    let mut total_bytes = 0usize;
    for entry in delta.get("changedBySubtask").and_then(Value::as_array).into_iter().flatten() {
        let Some(path) = entry.get("path").and_then(Value::as_str) else { continue; };
        if !path.starts_with(".playwright-mcp/") { continue; }
        // Existing output belongs to earlier work; modifying/deleting/reusing
        // it is not the creation-only runtime-artifact allowance.
        if entry.get("classification").and_then(Value::as_str) != Some("changed_by_subtask")
            || entry.get("beforeHash").is_none_or(|value| !value.is_null())
            || entry.get("beforeStatus").and_then(Value::as_str) != Some("")
            || entry.get("afterStatus").and_then(Value::as_str) != Some("present")
            || delta.get("reusedEvidence").and_then(Value::as_array).into_iter().flatten()
                .any(|reused| reused.get("path").and_then(Value::as_str) == Some(path))
        { continue; }
        let Some(hash) = entry.get("afterHash").and_then(Value::as_str) else { continue; };
        if let Some(bytes) = manual_mcp_snapshot_bytes(&run.project_path, path, hash) {
            total_bytes += bytes;
            allowed.insert(path.to_string());
            if allowed.len() > 64 || total_bytes > 4_194_304 { return BTreeSet::new(); }
        }
    }
    allowed
}

fn manual_evidence_text(value: &Value, field: &str) -> bool {
    value
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty())
}

fn manual_evidence_viewport(value: &Value) -> Option<ManualVerificationViewport> {
    serde_json::from_value::<ManualVerificationViewport>(value.clone())
        .ok()
        .filter(|size| size.width > 0 && size.height > 0)
}

fn manual_evidence_artifact(project_path: &str, reference: &str) -> Option<Value> {
    // Evidence belongs under an ignored, project-local runtime output directory.
    // Reject absolute paths, traversal, and symlinks escaping that directory;
    // never open a provider-supplied arbitrary host path or fetch a remote URL.
    let relative = Path::new(reference);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        || !relative.starts_with("test-results/manual-evidence")
    {
        return None;
    }
    let project = fs::canonicalize(project_path).ok()?;
    let allowed = fs::canonicalize(project.join("test-results/manual-evidence")).ok()?;
    if allowed != project.join("test-results/manual-evidence") {
        return None;
    }
    let path = fs::canonicalize(project.join(relative)).ok()?;
    if !path.starts_with(&allowed) {
        return None;
    }
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() || !(64..=20_000_000).contains(&metadata.len()) {
        return None;
    }
    let file = fs::File::open(&path).ok()?;
    let mut bounded = std::io::Read::take(file, 20_000_001);
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut bounded, &mut bytes).ok()?;
    if !(64..=20_000_000).contains(&bytes.len()) {
        return None;
    }
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let image_header_valid = match extension.as_str() {
        "png" => {
            bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                && bytes.get(12..16) == Some(b"IHDR".as_slice())
                && bytes.windows(4).any(|window| window == b"IEND")
        }
        "jpg" | "jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]) && bytes.ends_with(&[0xff, 0xd9]),
        "webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()),
        _ => false,
    };
    image_header_valid.then(|| {
        json!({
            "path": reference,
            "bytes": bytes.len(),
            "sha256": format!("{:x}", Sha256::digest(&bytes)),
        })
    })
}

fn manual_verification_evidence_validation(
    run: &AgenticBoard,
    task: &BoardTask,
    parsed: &Value,
) -> Value {
    let mut issues = Vec::<String>::new();
    let mut artifacts = BTreeMap::<String, Value>::new();
    let scope = task
        .hierarchy
        .manual_verification
        .as_ref()
        .and_then(approved_manual_verification_scope);
    let Some(scope) = scope else {
        return json!({"passed": false, "issues": [MANUAL_VERIFICATION_CONTRACT_ERROR]});
    };
    let evidence = parsed.get("manualEvidence").unwrap_or(&Value::Null);
    if evidence.get("version").and_then(Value::as_u64) != Some(1)
        || evidence.get("scopeVersion").and_then(Value::as_u64)
            != Some(task.hierarchy.scope_version)
        || evidence.get("mode").and_then(Value::as_str) != Some(scope.mode.as_str())
    {
        issues.push("manualEvidence must declare version 1, the current scopeVersion, and the approved manualVerification mode.".to_string());
    }
    let environments = evidence
        .get("environments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let checks = evidence
        .get("checks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut valid_environments = BTreeMap::<String, Option<ManualVerificationViewport>>::new();
    for environment in &environments {
        let id = environment.get("id").and_then(Value::as_str).unwrap_or("");
        let viewport = environment
            .get("viewport")
            .and_then(manual_evidence_viewport);
        let surface = environment.get("surface").and_then(Value::as_str);
        if id.trim().is_empty()
            || valid_environments.contains_key(id)
            || !["deviceOrEmulator", "appVersion", "backendUrl"]
                .iter()
                .all(|key| manual_evidence_text(environment, key))
            || (scope.mode == "ui"
                && (viewport.is_none()
                    || !matches!(surface, Some("browser" | "native"))
                    || (surface == Some("browser")
                        && !manual_evidence_text(environment, "browser"))))
        {
            issues.push("Each manualEvidence environment needs a unique id, deviceOrEmulator, appVersion and backendUrl; UI also needs a positive viewport, surface browser/native, and browser version when applicable.".to_string());
            continue;
        }
        if scope.input_modality.as_deref() == Some("touch") && !manual_touch_capability(environment) {
            issues.push(format!("Touch-required environment {id} must report positive browser inputCapabilities.maxTouchPoints or native inputCapabilities.touchscreen=true; mouse-only or unknown capability cannot pass."));
            continue;
        }
        valid_environments.insert(id.to_string(), viewport);
    }
    if valid_environments.is_empty() {
        issues.push(
            "manualEvidence requires at least one observed runtime/inspection environment."
                .to_string(),
        );
    }
    for viewport in &scope.viewports {
        if !valid_environments
            .values()
            .any(|observed| observed.as_ref() == Some(viewport))
        {
            issues.push(format!(
                "Required viewport {}x{} has no manualEvidence environment.",
                viewport.width, viewport.height
            ));
        }
    }
    // A task without explicit criteria still has one description-level claim.
    let criterion_count = task.acceptance_criteria.len().max(1);
    let mut covered = BTreeSet::<(usize, String)>::new();
    let mut touch_observed_environments = BTreeSet::<String>::new();
    for check in &checks {
        let criterion = check
            .get("criterionIndex")
            .and_then(Value::as_u64)
            .map(|index| index as usize);
        let environment_id = check
            .get("environmentId")
            .and_then(Value::as_str)
            .unwrap_or("");
        let actions = check.get("actions").and_then(Value::as_array);
        let supported_action = |action: &Value| {
            let action_type = action.get("type").and_then(Value::as_str).unwrap_or("");
            let matches_mode = match scope.mode.as_str() {
                "ui" => matches!(
                    action_type,
                    "navigate" | "tap" | "type" | "key" | "scroll" | "reload"
                ),
                "cli" => action_type == "command",
                "api" => action_type == "request",
                "inspection" => action_type == "inspect",
                _ => false,
            };
            matches_mode
                && manual_evidence_text(action, "target")
                && manual_evidence_text(action, "observed")
        };
        let actions_valid = actions
            .is_some_and(|actions| !actions.is_empty() && actions.iter().all(supported_action));
        let touch_valid = scope.input_modality.as_deref() != Some("touch") || actions.is_some_and(|actions| {
            let surface = environments.iter().find(|environment|
                environment.get("id").and_then(Value::as_str) == Some(environment_id))
                .and_then(|environment| environment.get("surface")).and_then(Value::as_str);
            let touch_actions = actions.iter().filter(|action|
                matches!(action.get("type").and_then(Value::as_str), Some("tap" | "scroll")))
                .collect::<Vec<_>>();
            touch_actions.iter().all(|action| manual_touch_action_observed(action, surface))
        });
        if !touch_valid {
            issues.push(format!("Touch-required check in environment {environment_id} needs actual tap/scroll inputObservation for every pointer action: browser trusted touch pointer events, or native touchscreen MotionEvent observations. A tap label or screenshot alone cannot establish touch execution."));
        }
        let references = normalize_string_list(check.get("artifacts"));
        let mut artifact_valid = scope.mode != "ui";
        if scope.mode == "ui" {
            artifact_valid = !references.is_empty();
            for reference in references {
                if let Some(artifact) = manual_evidence_artifact(&run.project_path, &reference) {
                    artifacts.insert(reference, artifact);
                } else {
                    artifact_valid = false;
                    issues.push("UI evidence must reference existing PNG/JPEG/WebP screenshots under project-local test-results/manual-evidence/; missing, invalid, oversized, or escaping paths are rejected.".to_string());
                }
            }
        }
        if criterion.is_none_or(|index| index >= criterion_count)
            || !valid_environments.contains_key(environment_id)
            || check.get("status").and_then(Value::as_str) != Some("pass")
            || !manual_evidence_text(check, "expected")
            || !manual_evidence_text(check, "observed")
            || !actions_valid
            || !touch_valid
            || !artifact_valid
        {
            issues.push("Every manualEvidence check must pass, reference a valid zero-based criterionIndex and environmentId, state expected/observed outcomes, and include actual mode-appropriate actions. UI requires interaction actions and screenshots; test-suite output, source/CSS inspection, and curl are not UI evidence.".to_string());
            continue;
        }
        if scope.input_modality.as_deref() == Some("touch") && actions.is_some_and(|actions|
            actions.iter().any(|action| matches!(action.get("type").and_then(Value::as_str), Some("tap" | "scroll")))) {
            // Count only a fully valid passing check. Static criteria can share
            // this environment's touch preflight, but invalid/blocked checks
            // cannot provide its required observed touch execution.
            touch_observed_environments.insert(environment_id.to_string());
        }
        covered.insert((criterion.unwrap(), environment_id.to_string()));
    }
    if scope.input_modality.as_deref() == Some("touch") {
        for id in valid_environments.keys() {
            if !touch_observed_environments.contains(id) {
                issues.push(format!("Touch-required environment {id} lacks a valid passing check with observed touch input. Static navigation checks may reuse touch evidence only from the same environment."));
            }
        }
    }
    for criterion in 0..criterion_count {
        if scope.mode == "ui" {
            for (id, viewport) in &valid_environments {
                if viewport
                    .as_ref()
                    .is_some_and(|viewport| scope.viewports.contains(viewport))
                    && !covered.contains(&(criterion, id.clone()))
                {
                    issues.push(format!("Acceptance criterion {criterion} lacks a passing observed UI check in environment {id}."));
                }
            }
        } else if !covered.iter().any(|(index, _)| *index == criterion) {
            issues.push(format!(
                "Acceptance criterion {criterion} lacks a passing observed {} check.",
                scope.mode
            ));
        }
    }
    // Once recorded, QA cannot silently replace or reinterpret an artifact.
    if let Some(previous) = parsed
        .pointer("/manualEvidenceGate/artifacts")
        .and_then(Value::as_array)
    {
        for artifact in previous {
            let path = artifact.get("path").and_then(Value::as_str).unwrap_or("");
            if artifacts.get(path) != Some(artifact) {
                issues.push(format!(
                    "Recorded manual screenshot changed or disappeared: {path}"
                ));
            }
        }
    }
    let issues = dedupe_strings(issues);
    json!({"passed": issues.is_empty(), "scopeVersion": task.hierarchy.scope_version,
        "mode": scope.mode, "issues": issues, "artifacts": artifacts.into_values().collect::<Vec<_>>()})
}

fn source_manual_verification_issues(run: &AgenticBoard, task: &BoardTask) -> Vec<String> {
    if !is_qa_task(task) && !matches!(canonical_task_kind(task), TASK_KIND_QA | TASK_KIND_REVIEW) {
        return Vec::new();
    }
    let mut pending = task
        .source_task_id
        .iter()
        .chain(task.source_qa_task_id.iter())
        .cloned()
        .collect::<Vec<_>>();
    pending.extend(task_blockers(task));
    let mut seen = BTreeSet::new();
    let mut issues = Vec::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(source) = run.tasks.iter().find(|source| source.id == id) else {
            continue;
        };
        if canonical_task_kind(source) == TASK_KIND_MANUAL_TEST && task_is_executable(source) {
            let result = source.result.as_ref().unwrap_or(&Value::Null);
            let validation = manual_verification_evidence_validation(run, source, result);
            issues.extend(
                normalize_string_list(validation.get("issues"))
                    .into_iter()
                    .map(|issue| format!("Source manual task {id}: {issue}")),
            );
            if !parsed_status_done(Some(result))
                || result
                    .pointer("/evidenceGate/passed")
                    .and_then(Value::as_bool)
                    == Some(false)
            {
                issues.push(format!("Source manual task {id} did not pass its completion evidence gate; QA cannot override it."));
            }
        } else if is_qa_task(source)
            || matches!(canonical_task_kind(source), TASK_KIND_QA | TASK_KIND_REVIEW)
        {
            pending.extend(
                source
                    .source_task_id
                    .iter()
                    .chain(source.source_qa_task_id.iter())
                    .cloned(),
            );
            pending.extend(task_blockers(source));
        }
        if !task_is_executable(source) {
            pending.extend(
                run.tasks
                    .iter()
                    .filter(|child| {
                        child.hierarchy.parent_id.as_deref() == Some(id.as_str())
                            && child.hierarchy.required
                            && child.superseded_by.is_none()
                    })
                    .map(|child| child.id.clone()),
            );
        }
    }
    issues
}

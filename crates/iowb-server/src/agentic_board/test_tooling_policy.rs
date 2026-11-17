// Test-tooling policy evidence is derived from bounded, hash-matched workspace
// bytes. Only semantic digests/classifications are persisted, not script values
// or arbitrary manifest contents that could contain credentials.
fn captured_test_tooling(project_path: &str, files: &serde_json::Map<String, Value>) -> Value {
    let read = |name: &str, limit: u64| {
        let hash = files.get(name)?.get("hash")?.as_str()?;
        read_snapshot_json(&Path::new(project_path).join(name), hash, limit)
    };
    let Some(manifest) = read("package.json", 64 * 1024) else {
        return Value::Null;
    };
    let Some(manifest_evidence) = test_manifest_evidence(&manifest) else {
        return Value::Null;
    };
    let lock = read("package-lock.json", 2 * 1024 * 1024)
        .and_then(|lock| test_lock_evidence(&manifest, &lock));
    json!({"manifest": manifest_evidence, "lock": lock})
}

fn read_snapshot_json(path: &Path, expected_hash: &str, limit: u64) -> Option<Value> {
    use std::io::Read;
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > limit || sha256_hex(&bytes) != expected_hash {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn semantic_digest(value: &Value) -> String {
    sha256_hex(&serde_json::to_vec(value).unwrap_or_default())
}

fn playwright_dependency(name: &str) -> bool {
    matches!(name, "@playwright/test" | "playwright" | "playwright-core")
}

fn test_tooling_dependency(name: &str) -> bool {
    playwright_dependency(name) || name == "jsdom"
}

fn simple_npm_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let parsed = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(parsed)
}

fn test_dependency_version(spec: &str) -> bool {
    simple_npm_version(spec.strip_prefix(['^', '~']).unwrap_or(spec)).is_some()
}

fn lock_version_matches(spec: &str, version: &str) -> bool {
    let Some(actual) = simple_npm_version(version) else {
        return false;
    };
    let Some(minimum) = simple_npm_version(spec.strip_prefix(['^', '~']).unwrap_or(spec)) else {
        return false;
    };
    if actual < minimum {
        return false;
    }
    match spec.as_bytes().first() {
        Some(b'~') => actual.0 == minimum.0 && actual.1 == minimum.1,
        Some(b'^') if minimum.0 > 0 => actual.0 == minimum.0,
        Some(b'^') if minimum.1 > 0 => actual.0 == 0 && actual.1 == minimum.1,
        _ => actual == minimum,
    }
}

fn is_local_test_tool_command(command: &str) -> bool {
    if command.contains(" && ") {
        return command.split(" && ").all(is_local_test_tool_command);
    }
    if is_local_node_test_command(command) {
        return true;
    }
    if command.chars().any(|ch| ch.is_control() && ch != '\t') {
        return false;
    }
    let mut args = command.split_ascii_whitespace();
    if args.next() != Some("playwright") || args.next() != Some("test") {
        return false;
    }
    args.all(|arg| {
        matches!(
            arg,
            "--reporter=line" | "--reporter=list" | "--reporter=dot" | "--reporter=json"
        ) || arg
            .strip_prefix("--config=")
            .is_some_and(playwright_config_path)
            || (!arg.starts_with(['/', '-'])
                && arg
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_./*-".contains(&byte))
                && !arg.split('/').any(|part| part == "..")
                && is_test_or_fixture_path(arg))
    })
}

fn playwright_config_path(path: &str) -> bool {
    matches!(
        path.strip_prefix("./").unwrap_or(path),
        "playwright.config.js"
            | "playwright.config.ts"
            | "playwright.config.mjs"
            | "playwright.config.cjs"
            | "playwright.config.mts"
            | "playwright.config.cts"
    )
}

fn test_manifest_evidence(manifest: &Value) -> Option<Value> {
    let mut immutable = manifest.as_object()?.clone();
    let mut scripts = match immutable.remove("scripts") {
        Some(value) => value.as_object()?.clone(),
        None => serde_json::Map::new(),
    };
    let mut test_scripts = serde_json::Map::new();
    let script_names = scripts
        .keys()
        .filter(|key| key.as_str() == "test" || key.starts_with("test:"))
        .cloned()
        .collect::<Vec<_>>();
    for key in script_names {
        test_scripts.insert(key.clone(), scripts.remove(&key)?);
    }
    if !scripts.is_empty() {
        immutable.insert("scripts".to_string(), json!(scripts));
    }
    let mut dev_dependencies = match immutable.remove("devDependencies") {
        Some(value) => value.as_object()?.clone(),
        None => serde_json::Map::new(),
    };
    let names = dev_dependencies
        .keys()
        .filter(|key| test_tooling_dependency(key))
        .cloned()
        .collect::<Vec<_>>();
    let mut test_dependencies = serde_json::Map::new();
    for key in names {
        test_dependencies.insert(key.clone(), dev_dependencies.remove(&key)?);
    }
    if !dev_dependencies.is_empty() {
        immutable.insert("devDependencies".to_string(), json!(dev_dependencies));
    }
    // A DOM test dependency must not grant Playwright command/config privileges.
    let has_playwright = test_dependencies.keys().any(|name| playwright_dependency(name));
    let safe_scripts = !test_scripts.is_empty()
        && test_scripts.values().all(|value| {
            value.as_str().is_some_and(|script| {
                is_local_test_tool_command(script)
                    && (!script
                        .split_ascii_whitespace()
                        .any(|word| word == "playwright")
                        || has_playwright)
            })
        });
    let safe_dependencies = test_dependencies
        .values()
        .all(|value| value.as_str().is_some_and(test_dependency_version));
    let mut new_manifest = manifest.clone();
    if let Some(fields) = new_manifest.as_object_mut() {
        fields.remove("devDependencies");
        fields.insert("scripts".to_string(), json!({"test": "node --test"}));
    }
    // New manifests retain the original metadata-only boundary; pre-existing
    // production fields may remain, but may never change in a test-only task.
    let new_manifest_allowed = is_minimal_node_test_manifest(&new_manifest)
        && manifest
            .get("devDependencies")
            .and_then(Value::as_object)
            .is_none_or(|deps| deps.keys().all(|name| test_tooling_dependency(name)))
        && scripts.is_empty();
    Some(json!({
        "immutableDigest": semantic_digest(&json!(immutable)),
        "testScriptsDigest": semantic_digest(&json!(test_scripts)),
        "testDependenciesDigest": semantic_digest(&json!(test_dependencies)),
        "safeTestScripts": safe_scripts,
        "safeTestDependencies": safe_dependencies,
        "newManifestAllowed": new_manifest_allowed,
        "playwrightConfigured": has_playwright && safe_dependencies,
        "dependencyFingerprints": manifest_dependency_fingerprints(manifest),
    }))
}

// Hash both package identifiers and specifications: retain just enough evidence
// to distinguish additions/version changes from reuse/removal, without storing
// arbitrary manifest values (which may contain credentials).
fn manifest_dependency_fingerprints(manifest: &Value) -> Option<BTreeMap<String, String>> {
    let mut fingerprints = BTreeMap::new();
    for field in ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies", "overrides", "resolutions"] {
        if let Some(value) = manifest.get(field) {
            for (name, spec) in value.as_object()? {
                fingerprints.insert(semantic_digest(&json!([field, name])), semantic_digest(spec));
            }
        }
    }
    for field in ["bundledDependencies", "bundleDependencies"] {
        if let Some(value) = manifest.get(field) {
            for name in value.as_array()? {
                fingerprints.insert(semantic_digest(&json!([field, name.as_str()?])), semantic_digest(&json!(true)));
            }
        }
    }
    Some(fingerprints)
}

fn lock_root_projection(value: &Value) -> Value {
    let mut fields = serde_json::Map::new();
    for key in [
        "name",
        "version",
        "license",
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "engines",
        "os",
        "cpu",
    ] {
        if let Some(value) = value.get(key) {
            fields.insert(key.to_string(), value.clone());
        }
    }
    json!(fields)
}

fn valid_lock_package_path(path: &str) -> bool {
    let mut parts = path.split('/');
    loop {
        if parts.next() != Some("node_modules") {
            return false;
        }
        let Some(name) = parts.next() else {
            return false;
        };
        if name.is_empty() || name == "." || name == ".." || name.contains('\\') {
            return false;
        }
        if name.starts_with('@') {
            let Some(name) = parts.next() else {
                return false;
            };
            if name.is_empty() || name == "." || name == ".." || name.contains('\\') {
                return false;
            }
        }
        match parts.clone().next() {
            None => return true,
            Some(_) => {}
        }
    }
}

fn resolve_lock_dependency(
    packages: &serde_json::Map<String, Value>,
    from: &str,
    dependency: &str,
) -> Option<String> {
    let mut base = from;
    loop {
        let path = if base.is_empty() {
            format!("node_modules/{dependency}")
        } else {
            format!("{base}/node_modules/{dependency}")
        };
        if valid_lock_package_path(&path) && packages.contains_key(&path) {
            return Some(path);
        }
        if base.is_empty() {
            return None;
        }
        base = base
            .rfind("/node_modules/")
            .map(|index| &base[..index])
            .unwrap_or("");
    }
}

fn test_lock_evidence(manifest: &Value, lock: &Value) -> Option<Value> {
    let fields = lock.as_object()?;
    if fields.get("lockfileVersion")?.as_u64()? != 3
        || fields.keys().any(|key| {
            !matches!(
                key.as_str(),
                "name" | "version" | "lockfileVersion" | "requires" | "packages"
            )
        })
    {
        return None;
    }
    let packages = fields.get("packages")?.as_object()?;
    if packages.len() > MAX_WORKSPACE_SNAPSHOT_FILES
        || lock_root_projection(packages.get("")?) != lock_root_projection(manifest)
        || ["name", "version"].iter().any(|key| {
            manifest
                .get(*key)
                .is_some_and(|value| lock.get(*key) != Some(value))
        })
    {
        return None;
    }
    let mut pending = Vec::new();
    for (name, spec) in manifest
        .get("devDependencies")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(name, _)| test_tooling_dependency(name))
    {
        let path = resolve_lock_dependency(packages, "", name)?;
        if !lock_version_matches(
            spec.as_str()?,
            packages.get(&path)?.get("version")?.as_str()?,
        ) {
            return None;
        }
        pending.push(path);
    }
    let mut reachable = BTreeSet::new();
    while let Some(path) = pending.pop() {
        if !reachable.insert(path.clone()) {
            continue;
        }
        let package = packages.get(&path)?.as_object()?;
        if package.get("link").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        for key in ["dependencies", "optionalDependencies"] {
            for name in package
                .get(key)
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|deps| deps.keys())
            {
                match resolve_lock_dependency(packages, &path, name) {
                    Some(dependency) => pending.push(dependency),
                    None if key == "optionalDependencies" => {}
                    None => return None,
                }
            }
        }
    }
    let mut entries = serde_json::Map::new();
    for (path, package) in packages.iter().filter(|(path, _)| !path.is_empty()) {
        if !valid_lock_package_path(path) || !package.is_object() {
            return None;
        }
        entries.insert(
            path.clone(),
            json!({
                "digest": semantic_digest(package),
                "dev": package.get("dev").and_then(Value::as_bool) == Some(true),
                "testReachable": reachable.contains(path),
            }),
        );
    }
    let mut metadata = fields.clone();
    metadata.remove("packages");
    Some(json!({"metadataDigest": semantic_digest(&json!(metadata)), "packages": entries}))
}

fn test_tooling_delta(before: &Value, after: &Value) -> Value {
    if [before, after].iter().any(|snapshot| {
        snapshot.get("provider").and_then(Value::as_str) != Some("filesystem sha256")
            || snapshot.get("complete").and_then(Value::as_bool) != Some(true)
    }) {
        return Value::Null;
    }
    let Some(after_manifest) = after.pointer("/testTooling/manifest") else {
        return Value::Null;
    };
    let before_manifest = before.pointer("/testTooling/manifest");
    let dependency_additions_absent = after_manifest.get("dependencyFingerprints")
        .and_then(Value::as_object)
        .is_some_and(|after_dependencies| {
            if let Some(before_dependencies) = before_manifest
                .and_then(|manifest| manifest.get("dependencyFingerprints"))
                .and_then(Value::as_object)
            {
                after_dependencies.iter().all(|(name, digest)| before_dependencies.get(name) == Some(digest))
            } else {
                before.get("testRunnerManifestAbsent").and_then(Value::as_bool) == Some(true)
                    && after_dependencies.is_empty()
            }
        });
    // An unchanged manifest cannot prove that a lock-only update reused the
    // same packages. Retain an independent digest check for every locked entry.
    let lock_dependency_additions_absent = after
        .get("testRunnerLockAbsent").and_then(Value::as_bool) == Some(true)
        || after.pointer("/testTooling/lock/packages")
            .and_then(Value::as_object)
            .is_some_and(|new| {
                if let Some(old) = before.pointer("/testTooling/lock/packages")
                    .and_then(Value::as_object)
                {
                    new.iter().all(|(path, entry)| {
                        entry.get("digest").is_some()
                            && old.get(path).and_then(|prior| prior.get("digest")) == entry.get("digest")
                    })
                } else {
                    before.get("testRunnerLockAbsent").and_then(Value::as_bool) == Some(true)
                        && new.is_empty()
                }
            });
    let preserved = before_manifest
        .is_some_and(|prior| prior.get("immutableDigest") == after_manifest.get("immutableDigest"))
        || (before
            .get("testRunnerManifestAbsent")
            .and_then(Value::as_bool)
            == Some(true)
            && after_manifest
                .get("newManifestAllowed")
                .and_then(Value::as_bool)
                == Some(true));
    let manifest_allowed = preserved
        && after_manifest
            .get("safeTestScripts")
            .and_then(Value::as_bool)
            == Some(true)
        && after_manifest
            .get("safeTestDependencies")
            .and_then(Value::as_bool)
            == Some(true);
    let playwright = manifest_allowed
        && after_manifest
            .get("playwrightConfigured")
            .and_then(Value::as_bool)
            == Some(true);
    let lock_allowed = manifest_allowed
        && after
            .pointer("/testTooling/lock")
            .filter(|value| value.is_object())
            .is_some_and(|after_lock| {
                let before_lock = before
                    .pointer("/testTooling/lock")
                    .filter(|value| value.is_object());
                if before_lock.is_none()
                    && before.get("testRunnerLockAbsent").and_then(Value::as_bool) != Some(true)
                {
                    return false;
                }
                if before_lock.is_some_and(|prior| {
                    prior.get("metadataDigest") != after_lock.get("metadataDigest")
                }) {
                    return false;
                }
                let empty = serde_json::Map::new();
                let old = before_lock
                    .and_then(|value| value.get("packages"))
                    .and_then(Value::as_object)
                    .unwrap_or(&empty);
                let Some(new) = after_lock.get("packages").and_then(Value::as_object) else {
                    return false;
                };
                old.keys().chain(new.keys()).all(|path| {
                    let before_entry = old.get(path);
                    let after_entry = new.get(path);
                    if before_entry.and_then(|entry| entry.get("digest"))
                        == after_entry.and_then(|entry| entry.get("digest"))
                    {
                        return true;
                    }
                    [before_entry, after_entry]
                        .into_iter()
                        .flatten()
                        .all(|entry| {
                            entry.get("dev").and_then(Value::as_bool) == Some(true)
                                && entry.get("testReachable").and_then(Value::as_bool) == Some(true)
                        })
                })
            });
    json!({"manifestAllowed": manifest_allowed, "playwrightConfigured": playwright, "lockAllowed": lock_allowed, "dependencyAdditionsAbsent": dependency_additions_absent, "lockDependencyAdditionsAbsent": lock_dependency_additions_absent})
}

fn retain_same_task_workspace_evidence(
    task: &BoardTask,
    before: &Value,
    after: &Value,
    summary: &mut Value,
) {
    let scope_version = task.hierarchy.scope_version.max(1);
    if before.get("scopeVersion").and_then(Value::as_u64) != Some(scope_version) {
        return;
    }
    summary["scopeVersion"] = json!(scope_version);
    let Some(prior) = task.changed_file_summary.as_ref() else {
        return;
    };
    if prior.get("taskId").and_then(Value::as_str) != Some(task.id.as_str())
        || prior.get("scopeVersion").and_then(Value::as_u64) != Some(scope_version)
        || prior.get("ownershipPolicy").and_then(Value::as_str) != Some(WORKSPACE_OWNERSHIP_POLICY)
        || prior.get("snapshotComplete").and_then(Value::as_bool) != Some(true)
        || prior.get("snapshotProvider").and_then(Value::as_str) != Some("filesystem sha256")
        || [before, after].iter().any(|snapshot| {
            snapshot.get("provider").and_then(Value::as_str) != Some("filesystem sha256")
                || snapshot.get("complete").and_then(Value::as_bool) != Some(true)
        })
    {
        return;
    }
    let mut reused = Vec::new();
    let touched = change_summary_touched_paths(summary);
    for key in ["changedBySubtask", "reusedEvidence"] {
        for source in prior
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(path) = source.get("path").and_then(Value::as_str) else {
                continue;
            };
            let Some(hash) = source
                .get("afterHash")
                .and_then(Value::as_str)
                .filter(|hash| !hash.is_empty())
            else {
                continue;
            };
            let expected_classification = if key == "changedBySubtask" {
                "changed_by_subtask"
            } else {
                "reused_same_task"
            };
            if source.get("classification").and_then(Value::as_str) != Some(expected_classification)
                || touched.iter().any(|touched| touched == path)
                || [before, after].iter().any(|snapshot| {
                    snapshot
                        .get("filesByPath")
                        .and_then(|files| files.get(path))
                        .and_then(|file| file.get("hash"))
                        .and_then(Value::as_str)
                        != Some(hash)
                })
            {
                continue;
            }
            if canonical_task_kind(task) == TASK_KIND_MANUAL_TEST
                && prior.get("runtimeArtifacts").and_then(Value::as_array).into_iter().flatten().any(|artifact| {
                    artifact.get("kind").and_then(Value::as_str) == Some("playwright_mcp_snapshot")
                        && artifact.get("path").and_then(Value::as_str) == Some(path)
                        && artifact.get("sha256").and_then(Value::as_str) == Some(hash)
                })
            {
                // A previous attempt's unchanged, server-validated auxiliary
                // snapshot is baseline data, not reused implementation
                // evidence. Mutations/deletions are still in touchedFiles.
                continue;
            }
            let mut entry = source.clone();
            entry["classification"] = json!("reused_same_task");
            entry["originTaskId"] = json!(task.id);
            entry["originScopeVersion"] = json!(scope_version);
            entry["originCapturedAt"] = source
                .get("originCapturedAt")
                .cloned()
                .unwrap_or_else(|| prior.get("capturedAt").cloned().unwrap_or(Value::Null));
            entry["testToolingPolicy"] = source
                .get("testToolingPolicy")
                .cloned()
                .unwrap_or_else(|| prior.get("testTooling").cloned().unwrap_or(Value::Null));
            reused.push(entry);
        }
    }
    summary["reusedEvidence"] = json!(reused);
    summary["reusedEvidenceCount"] = json!(reused.len());
}

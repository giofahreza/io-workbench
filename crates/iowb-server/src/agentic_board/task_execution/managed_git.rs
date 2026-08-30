async fn ensure_managed_git_branch_for_task_group(
    run: &mut AgenticBoard,
    task_id: &str,
) -> Result<()> {
    if run.git_policy != "managed" {
        return Ok(());
    }
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id).cloned() else {
        return Ok(());
    };
    if !is_managed_git_group_root(&task) {
        return Ok(());
    }
    let group_id = task.group_id.clone().unwrap_or_else(|| task.id.clone());
    if let Some(entry) = managed_git_entry(run, &group_id) {
        if matches!(
            entry.get("status").and_then(Value::as_str),
            Some("running" | "completed")
        ) {
            return Ok(());
        }
    }
    let entry = json!({
        "groupId": group_id,
        "taskId": task.id,
        "taskTitle": task.title,
        "policy": "managed",
        "status": "running",
        "baseBranch": "",
        "branchName": "",
        "startedAt": Utc::now(),
        "completedAt": null,
        "error": "",
        "commands": [],
    });
    run.git_ledger.push(entry);

    let inside = run_managed_git_ledger_command(
        run,
        &group_id,
        &["rev-parse", "--is-inside-work-tree"],
        MANAGED_GIT_TIMEOUT,
    )
    .await?;
    if inside.stdout.trim() != "true" {
        mark_managed_git_failed(run, &group_id, "Project is not inside a git worktree.");
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Project is not inside a git worktree.",
        ));
    }
    let dirty =
        run_managed_git_ledger_command(run, &group_id, &["status", "--short"], MANAGED_GIT_TIMEOUT)
            .await?;
    if !dirty.stdout.trim().is_empty() {
        let message = format!(
            "Managed git requires a clean worktree before creating a task branch. Current git status:\n{}",
            dirty.stdout.trim()
        );
        mark_managed_git_failed(run, &group_id, &message);
        return Err(ServerError::new(StatusCode::CONFLICT, message));
    }
    let base_branch = current_git_branch(&run.project_path);
    if base_branch.is_empty() {
        mark_managed_git_failed(run, &group_id, "Could not determine current git branch.");
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Could not determine current git branch.",
        ));
    }
    set_managed_git_entry_field(run, &group_id, "baseBranch", json!(base_branch));
    run_managed_git_ledger_command(
        run,
        &group_id,
        &["switch", &base_branch],
        MANAGED_GIT_TIMEOUT,
    )
    .await?;
    let branch_name = build_managed_git_branch_name(&task, &group_id);
    set_managed_git_entry_field(run, &group_id, "branchName", json!(branch_name));
    run_managed_git_ledger_command(
        run,
        &group_id,
        &["switch", "-c", &branch_name],
        MANAGED_GIT_TIMEOUT,
    )
    .await?;
    run.append_log(format!(
        "Managed git created branch {branch_name} from {base_branch} for {group_id}"
    ));
    Ok(())
}

async fn finalize_managed_git_task_group(run: &mut AgenticBoard, task_id: &str) -> Result<()> {
    if run.git_policy != "managed" {
        return Ok(());
    }
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id).cloned() else {
        return Ok(());
    };
    if !task_is_done(&task) {
        return Ok(());
    }
    let group_id = task.group_id.clone().unwrap_or_else(|| task.id.clone());
    if task_group_has_unfinished_work(run, &group_id) {
        return Ok(());
    }
    let Some(entry) = managed_git_entry(run, &group_id).cloned() else {
        return Ok(());
    };
    if entry.get("status").and_then(Value::as_str) != Some("running") {
        return Ok(());
    }
    let branch_name = entry
        .get("branchName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let base_branch = entry
        .get("baseBranch")
        .and_then(Value::as_str)
        .unwrap_or("main")
        .to_string();
    let current_branch = current_git_branch(&run.project_path);
    if current_branch != branch_name {
        let message = format!(
            "Expected managed branch {branch_name}, but current branch is {}.",
            if current_branch.is_empty() {
                "[detached]"
            } else {
                current_branch.as_str()
            }
        );
        mark_managed_git_failed(run, &group_id, &message);
        return Err(ServerError::new(StatusCode::CONFLICT, message));
    }
    set_managed_git_entry_field(run, &group_id, "finalizingAt", json!(Utc::now()));
    let status =
        run_managed_git_ledger_command(run, &group_id, &["status", "--short"], MANAGED_GIT_TIMEOUT)
            .await?;
    if status.stdout.trim().is_empty() {
        set_managed_git_entry_field(
            run,
            &group_id,
            "noCommitReason",
            json!("No workspace changes to commit."),
        );
    } else {
        run_managed_git_ledger_command(run, &group_id, &["add", "-A"], MANAGED_GIT_TIMEOUT).await?;
        let diff = run_managed_git_command(
            &run.project_path,
            &["diff", "--cached", "--quiet"],
            MANAGED_GIT_TIMEOUT,
        )
        .await;
        append_managed_git_command(run, &group_id, &["diff", "--cached", "--quiet"], &diff);
        if diff.ok {
            set_managed_git_entry_field(
                run,
                &group_id,
                "noCommitReason",
                json!("No staged changes after git add -A."),
            );
        } else {
            let message = build_managed_git_commit_message(&task, &group_id);
            run_managed_git_ledger_command(
                run,
                &group_id,
                &["commit", "-m", &message],
                MANAGED_GIT_TIMEOUT,
            )
            .await?;
        }
    }
    run_managed_git_ledger_command(
        run,
        &group_id,
        &["switch", &base_branch],
        MANAGED_GIT_TIMEOUT,
    )
    .await?;
    let no_commit = managed_git_entry(run, &group_id)
        .and_then(|entry| entry.get("noCommitReason"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if no_commit.is_none() {
        run_managed_git_ledger_command(
            run,
            &group_id,
            &["merge", "--ff-only", &branch_name],
            MANAGED_GIT_TIMEOUT,
        )
        .await?;
        run_managed_git_ledger_command(
            run,
            &group_id,
            &["push", "origin", &base_branch],
            MANAGED_GIT_PUSH_TIMEOUT,
        )
        .await?;
    }
    set_managed_git_entry_field(run, &group_id, "status", json!("completed"));
    set_managed_git_entry_field(run, &group_id, "completedAt", json!(Utc::now()));
    if let Some(reason) = no_commit {
        run.append_log(format!(
            "Managed git completed {group_id} without a commit: {reason}"
        ));
    } else {
        run.append_log(format!(
            "Managed git committed, merged, and pushed {group_id} via {branch_name}"
        ));
    }
    Ok(())
}

fn is_managed_git_group_root(task: &BoardTask) -> bool {
    !is_qa_task(task)
        && !task.agents_knowledge_task
        && task.source_task_id.is_none()
        && task.task_type != "agents_knowledge"
}

fn task_group_has_unfinished_work(run: &AgenticBoard, group_id: &str) -> bool {
    run.tasks.iter().any(|task| {
        task.group_id.as_deref() == Some(group_id)
            && matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_BACKLOG | TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS
            )
    })
}

fn managed_git_entry<'a>(run: &'a AgenticBoard, group_id: &str) -> Option<&'a Value> {
    run.git_ledger
        .iter()
        .find(|entry| entry.get("groupId").and_then(Value::as_str) == Some(group_id))
}

fn managed_git_entry_mut<'a>(run: &'a mut AgenticBoard, group_id: &str) -> Option<&'a mut Value> {
    run.git_ledger
        .iter_mut()
        .find(|entry| entry.get("groupId").and_then(Value::as_str) == Some(group_id))
}

fn set_managed_git_entry_field(run: &mut AgenticBoard, group_id: &str, key: &str, value: Value) {
    if let Some(entry) = managed_git_entry_mut(run, group_id).and_then(Value::as_object_mut) {
        entry.insert(key.to_string(), value);
    }
}

fn mark_managed_git_failed(run: &mut AgenticBoard, group_id: &str, message: &str) {
    set_managed_git_entry_field(run, group_id, "status", json!("failed"));
    set_managed_git_entry_field(run, group_id, "completedAt", json!(Utc::now()));
    set_managed_git_entry_field(run, group_id, "error", json!(message));
    run.append_log(format!("Managed git failed for {group_id}: {message}"));
}

#[derive(Debug, Clone)]
struct ManagedGitCommandResult {
    ok: bool,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

async fn run_managed_git_ledger_command(
    run: &mut AgenticBoard,
    group_id: &str,
    args: &[&str],
    timeout_duration: Duration,
) -> Result<ManagedGitCommandResult> {
    let result = run_managed_git_command(&run.project_path, args, timeout_duration).await;
    append_managed_git_command(run, group_id, args, &result);
    if result.ok {
        Ok(result)
    } else {
        let message = format!(
            "git {} failed with exit {}{}",
            args.join(" "),
            result.exit_code,
            if result.stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", limit_text(result.stderr.trim(), 700))
            }
        );
        mark_managed_git_failed(run, group_id, &message);
        Err(ServerError::new(StatusCode::CONFLICT, message))
    }
}

async fn run_managed_git_command(
    project_path: &str,
    args: &[&str],
    timeout_duration: Duration,
) -> ManagedGitCommandResult {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(project_path)
        .env("PATH", augmented_user_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match timeout(timeout_duration, command.output()).await {
        Ok(Ok(output)) => ManagedGitCommandResult {
            ok: output.status.success(),
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Ok(Err(error)) => ManagedGitCommandResult {
            ok: false,
            exit_code: 1,
            stdout: String::new(),
            stderr: error.to_string(),
        },
        Err(_) => ManagedGitCommandResult {
            ok: false,
            exit_code: 124,
            stdout: String::new(),
            stderr: format!("git {} timed out", args.join(" ")),
        },
    }
}

fn append_managed_git_command(
    run: &mut AgenticBoard,
    group_id: &str,
    args: &[&str],
    result: &ManagedGitCommandResult,
) {
    if let Some(entry) = managed_git_entry_mut(run, group_id).and_then(Value::as_object_mut) {
        let commands = entry
            .entry("commands".to_string())
            .or_insert_with(|| json!([]));
        if let Some(items) = commands.as_array_mut() {
            items.push(json!({
                "command": format!("git {}", args.join(" ")),
                "args": args,
                "ok": result.ok,
                "exitCode": result.exit_code,
                "stdout": limit_text(&result.stdout, 4000),
                "stderr": limit_text(&result.stderr, 4000),
                "completedAt": Utc::now(),
            }));
        }
    }
}

fn current_git_branch(project_path: &str) -> String {
    git_command_text(project_path, &["branch", "--show-current"])
}

fn build_managed_git_branch_name(task: &BoardTask, group_id: &str) -> String {
    let slug = task
        .title
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    let suffix = if slug.is_empty() {
        group_id.to_string()
    } else {
        slug
    };
    format!("agentic/{group_id}-{suffix}")
        .chars()
        .take(96)
        .collect()
}

fn build_managed_git_commit_message(task: &BoardTask, group_id: &str) -> String {
    format!(
        "Agentic task {group_id}: {}",
        limit_text(&task.title, 120).replace('\n', " ")
    )
}

fn summarize_workspace_delta(task_id: &str, before: &Value, after: &Value) -> Value {
    let before_map = before
        .get("filesByPath")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let after_map = after
        .get("filesByPath")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let paths = before_map
        .keys()
        .chain(after_map.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut touched_files = Vec::new();
    let mut changed_by_subtask = Vec::new();
    let mut unknown_changes = Vec::new();
    for path in paths {
        let before_file = before_map.get(&path);
        let after_file = after_map.get(&path);
        if before_file == after_file {
            continue;
        }
        let classification =
            if before_file.is_none() || before_file.is_some_and(workspace_file_was_clean) {
                "changed_by_subtask"
            } else {
                "unknown_change"
            };
        let entry = workspace_delta_entry(&path, before_file, after_file, classification);
        touched_files.push(entry.clone());
        if classification == "changed_by_subtask" {
            changed_by_subtask.push(entry);
        } else {
            unknown_changes.push(entry);
        }
    }
    let pre_existing_changes = before_map
        .iter()
        .filter(|(_, before_file)| !workspace_file_was_clean(before_file))
        .map(|(path, before_file)| {
            workspace_delta_entry(
                path,
                Some(before_file),
                after_map.get(path),
                "pre_existing_change",
            )
        })
        .collect::<Vec<_>>();
    let touched_file_count = touched_files.len();
    let attributable_changed_file_count = changed_by_subtask.len();
    let unknown_change_count = unknown_changes.len();
    json!({
        "taskId": task_id,
        "capturedAt": Utc::now(),
        "isGit": after.get("isGit").and_then(Value::as_bool).unwrap_or(false),
        "touchedFiles": touched_files,
        "touchedFileCount": touched_file_count,
        "preExistingChanges": pre_existing_changes,
        "preExistingChangeCount": pre_existing_changes.len(),
        "changedBySubtask": changed_by_subtask,
        "attributableChangedFileCount": attributable_changed_file_count,
        "unknownChanges": unknown_changes,
        "unknownChangeCount": unknown_change_count,
        "ownershipPolicy": WORKSPACE_OWNERSHIP_POLICY,
        "currentWorkspaceFiles": after.get("files").cloned().unwrap_or_else(|| json!([])),
        "currentWorkspaceFileCount": after.get("files").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "shortStat": after.get("shortStat").and_then(Value::as_str).unwrap_or(""),
        "unavailableReason": after.get("error").and_then(Value::as_str).unwrap_or(""),
    })
}

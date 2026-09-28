async fn generate_commit_message(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<GenerateCommitMessageBody>,
) -> Result<Json<GitGenerateMessageResponse>> {
    if body.files.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Project name and files are required",
        ));
    }
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let repository_root = repository_root(&project_path).await?;
    let staged_files = validate_requested_staged_commit_files(&project_path, &body.files).await?;
    let diff_context = commit_message_diff_context(&project_path, &repository_root, &staged_files)
        .await
        .unwrap_or_default();
    let direct_ai_config = state
        .storage
        .get_setting(&format!("user:{}:direct-ai", user.0.id))?;
    let message =
        match generate_commit_message_with_ai(&body.files, &diff_context, direct_ai_config).await {
            Ok(message) if !message.trim().is_empty() => message,
            _ => fallback_commit_message(&body.files),
        };
    Ok(Json(GitGenerateMessageResponse { message }))
}

async fn remote_status(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitRemoteStatusResponse>> {
    let project_path = resolve_git_repository_path(&state, query.project_ref()?, query.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let branch = current_branch(&project_path)
        .await
        .unwrap_or_else(|_| "HEAD".to_string());
    let remotes = git(&project_path, ["remote"])
        .await?
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let has_remote = !remotes.is_empty();
    let fallback_remote = remotes
        .iter()
        .find(|remote| remote.as_str() == "origin")
        .or_else(|| remotes.first())
        .cloned();

    let has_commits = repository_has_commits(&project_path).await?;
    let push_target = if has_commits && branch != "HEAD" {
        // Push-target discovery is advisory status, not a prerequisite for
        // inspecting a repository.  A stale or malformed local Git config
        // should be surfaced as actionable guidance instead of making the
        // whole Git workspace fail to refresh.  The actual Push endpoint
        // remains strict and will still reject an unsafe implicit target.
        Some(match configured_git_push_target(&project_path, &branch).await {
            Ok(target) => target,
            Err(error) => no_git_push_target(
                "unavailable".to_string(),
                format!(
                    "Saved push configuration needs attention: {}. Choose a remote and branch explicitly.",
                    error.body.error,
                ),
            ),
        })
    } else {
        None
    };
    let push_remote_name = push_target
        .as_ref()
        .and_then(|resolution| resolution.target.as_ref())
        .map(|target| target.remote_name.clone());
    let push_remote_branch = push_target
        .as_ref()
        .and_then(|resolution| resolution.target.as_ref())
        .map(|target| target.remote_branch.clone());
    let push_mode = push_target
        .as_ref()
        .map(|resolution| resolution.push_mode.clone());
    let push_sets_upstream = push_target
        .as_ref()
        .and_then(|resolution| resolution.target.as_ref())
        .is_some_and(|target| target.set_upstream);
    let push_target_message = push_target
        .as_ref()
        .and_then(|resolution| resolution.message.clone());

    if !has_commits {
        return Ok(Json(GitRemoteStatusResponse {
            has_remote,
            has_upstream: false,
            branch,
            remote_branch: None,
            remote_name: fallback_remote,
            push_remote_name,
            push_remote_branch,
            push_mode,
            push_sets_upstream,
            push_target_message,
            ahead: 0,
            behind: 0,
            is_up_to_date: false,
            message: Some("Repository has no commits yet".to_string()),
        }));
    }

    let tracking = match git(
        &project_path,
        [
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .await
    {
        Ok(output) => output.stdout.trim().to_string(),
        Err(_) => {
            return Ok(Json(GitRemoteStatusResponse {
                has_remote,
                has_upstream: false,
                branch,
                remote_branch: None,
                remote_name: fallback_remote,
                push_remote_name,
                push_remote_branch,
                push_mode,
                push_sets_upstream,
                push_target_message,
                ahead: 0,
                behind: 0,
                is_up_to_date: false,
                message: Some("No remote tracking branch configured".to_string()),
            }));
        }
    };
    let configured_remotes = configured_remote_names(&project_path).await?;
    let remote_name = configured_local_tracking_branch(&project_path, &branch)
        .await?
        .map(|_| ".".to_string())
        .or_else(|| {
            split_configured_remote_tracking_name(&tracking, &configured_remotes)
                .map(|(remote, _)| remote)
        });
    let counts = git(
        &project_path,
        [
            "rev-list",
            "--count",
            "--left-right",
            &format!("{tracking}...HEAD"),
        ],
    )
    .await?
    .stdout;
    let mut parts = counts.split_whitespace();
    let behind = parts
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let ahead = parts
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);

    Ok(Json(GitRemoteStatusResponse {
        has_remote,
        has_upstream: true,
        branch,
        remote_branch: Some(tracking),
        remote_name,
        push_remote_name,
        push_remote_branch,
        push_mode,
        push_sets_upstream,
        push_target_message,
        ahead,
        behind,
        is_up_to_date: ahead == 0 && behind == 0,
        message: None,
    }))
}

async fn set_remote(
    State(state): State<AppState>,
    Json(body): Json<RemoteBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let remote_name = validate_remote_name(body.name.as_deref().unwrap_or("origin"))?;
    let remote_url = validate_remote_url(&body.url)?;
    let remotes = git(&project_path, ["remote"]).await?.stdout;
    let remote_exists = remotes.lines().any(|line| line.trim() == remote_name);
    let output = if remote_exists {
        git(
            &project_path,
            ["remote", "set-url", "--", &remote_name, &remote_url],
        )
        .await?
    } else {
        git(
            &project_path,
            ["remote", "add", "--", &remote_name, &remote_url],
        )
        .await?
    };

    let mut response = GitOperationResponse::success(join_output(&output));
    response.remote_name = Some(remote_name);
    // A remote URL may embed an HTTPS username/token. The request owner
    // already supplied it, but reflecting it into application state, logs,
    // or notifications would spread the secret beyond the Git config write.
    response.remote_url = Some(management_display_remote_url(&remote_url));
    Ok(Json(response))
}

async fn fetch(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let remote_name = upstream_remote_or_origin(&project_path).await?;
    if remote_name != "." {
        validate_remote_name(&remote_name)?;
    }
    // This legacy single-remote fetch is likewise a standalone refresh, not
    // a fetch-and-integrate command. Preserve FETCH_HEAD as LazyGit does so
    // it cannot unexpectedly replace another operation's reviewed input.
    let output = git(&project_path, ["fetch", "--no-write-fetch-head", &remote_name]).await?;
    let mut response =
        GitOperationResponse::success(join_output_or(&output, "Fetch completed successfully"));
    response.remote_name = Some(remote_name);
    Ok(Json(response))
}

async fn pull(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let branch = current_branch(&project_path).await?;
    let (remote_name, remote_branch) = configured_upstream_remote_branch(&project_path, &branch)
        .await?
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "No tracking branch is configured. Set this branch's upstream or use the contextual Pull form to choose a remote branch first.",
            )
        })?;
    if remote_name != "." {
        validate_remote_name(&remote_name)?;
    }
    validate_branch_name(&remote_branch)?;
    Ok(Json(
        pull_selected_remote(&project_path, remote_name, remote_branch, None).await?,
    ))
}

async fn pull_selected_remote(
    project_path: &Path,
    remote_name: String,
    remote_branch: String,
    requested_rebase: Option<&str>,
) -> Result<GitOperationResponse> {
    if remote_name != "." {
        validate_remote_name(&remote_name)?;
    }
    // LazyGit delegates Pull to Git, which gives branch.<name>.rebase
    // precedence over pull.rebase. Preserve that important per-branch
    // behavior while intentionally reading only repository-local settings:
    // this API must not let a host-global preference silently rewrite a
    // project opened through the mobile client. A one-shot request is the
    // equivalent of Git's explicit --[no-]rebase command-line override.
    let pull_rebase = effective_local_pull_rebase(project_path, requested_rebase).await?;
    let (rebase_argument, behavior_label) = match pull_rebase.as_str() {
        "true" => (Some("--rebase"), "rebase"),
        "merges" => (Some("--rebase=merges"), "rebase with merges"),
        "interactive" => (Some("--interactive"), "interactive rebase (default plan)"),
        _ => (None, "merge"),
    };
    if rebase_argument.is_some() {
        // Do not let rebase.autoStash create hidden state during a pull. The
        // native flow surfaces recoverable conflicts but not Git's temporary
        // autostash lifecycle, so callers must deliberately clean or stash
        // the worktree first.
        advanced_require_clean_worktree(project_path, "pull with rebase").await?;
    }

    // `git pull` hides the fetched object from us until it immediately starts
    // merging or rebasing. Fetch first so the exact incoming tree can be
    // checked against ignored local paths before Git gets an opportunity to
    // overwrite one. Qualify the selected branch exactly as LazyGit does:
    // `git fetch origin topic` can DWIM to a tag named `topic`, while the UI
    // promises to integrate `refs/heads/topic`. FETCH_HEAD is then the same
    // object `git pull <remote> refs/heads/<branch>` would integrate.
    let remote_ref = format!("refs/heads/{remote_branch}");
    git(project_path, ["fetch", &remote_name, &remote_ref]).await?;
    let fetched_head = resolve_commit_reference(project_path, "FETCH_HEAD").await?;
    let result = if let Some(rebase_argument) = rebase_argument {
        advanced_require_no_ignored_path_collision(
            project_path,
            &advanced_rebase_write_paths(project_path, &fetched_head).await?,
            "pull with rebase",
        )
        .await?;
        let mut args = vec!["rebase".to_string(), "--no-autostash".to_string()];
        if rebase_argument == "--rebase=merges" {
            args.push("--rebase-merges".to_string());
        } else if rebase_argument == "--interactive" {
            // Match LazyGit's interactive Pull behavior: Git receives its
            // normal interactive todo, while `git_with_noninteractive_editor`
            // supplies the no-op sequence editor that accepts the default
            // plan rather than waiting for a terminal editor.
            args.push("--interactive".to_string());
        }
        args.push(fetched_head);
        git_with_noninteractive_editor(project_path, args).await
    } else {
        let head = resolve_commit_reference(project_path, "HEAD").await?;
        advanced_require_no_ignored_path_collision(
            project_path,
            &advanced_tree_transition_write_paths(project_path, &head, &fetched_head).await?,
            "pull",
        )
        .await?;
        // A merge-style pull can invoke an editor to accept Git's generated
        // merge message. The native endpoint must remain noninteractive. Git
        // merge has an atomic no-overwrite-ignore switch, so retain it in
        // addition to the preflight above for the fetch-to-merge interval.
        // `pull.ff` overrides `merge.ff` for `git pull`. Since this endpoint
        // splits fetch from integration for ignored-path safety, carry that
        // narrow override through to the underlying merge explicitly.
        let mut args = vec![
            "merge".to_string(),
            "--no-edit".to_string(),
            "--no-autostash".to_string(),
            "--no-overwrite-ignore".to_string(),
        ];
        if let Some(argument) = local_pull_fast_forward_argument(project_path).await? {
            args.push(argument.to_string());
        }
        args.push(fetched_head);
        git_with_noninteractive_editor(project_path, args).await
    };
    let success_message = format!("Pull completed successfully using {behavior_label}");
    let mut response = workflow_response_after_command(
        project_path,
        result,
        &success_message,
    )
    .await?;
    response.remote_name = Some(remote_name);
    response.remote_branch = Some(remote_branch);
    Ok(response)
}

/// Resolve the effective local rebase preference for the currently checked
/// out branch. Git accepts boolean spellings plus the `m`/`i` aliases; the
/// shared validator normalizes them to the exact modes the native executor
/// understands.
async fn effective_local_pull_rebase(
    project_path: &Path,
    requested_rebase: Option<&str>,
) -> Result<String> {
    if let Some(value) = requested_rebase {
        return advanced_validate_pull_rebase(value);
    }

    let current_branch = current_branch(project_path).await?;
    if current_branch != "HEAD" {
        let branch = validate_branch_name(&current_branch)?;
        let branch_key = format!("branch.{branch}.rebase");
        if let Some(value) = advanced_local_config_value(project_path, &branch_key).await? {
            return advanced_validate_pull_rebase(&value);
        }
    }

    advanced_local_config_value(project_path, "pull.rebase")
        .await?
        .map(|value| advanced_validate_pull_rebase(&value))
        .transpose()
        .map(|value| value.unwrap_or_else(|| "false".to_string()))
}

/// Return the explicit merge flag that reproduces a repository-local
/// `pull.ff` setting after the selected remote branch has been fetched.
/// Absent `pull.ff` deliberately returns None so ordinary `merge.ff` keeps
/// its normal Git effect, just as it does for `git pull`.
async fn local_pull_fast_forward_argument(project_path: &Path) -> Result<Option<&'static str>> {
    let Some(value) = advanced_local_config_value(project_path, "pull.ff").await? else {
        return Ok(None);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "only" => Ok(Some("--ff-only")),
        value if advanced_parse_git_bool(value) == Some(true) => Ok(Some("--ff")),
        value if advanced_parse_git_bool(value) == Some(false) => Ok(Some("--no-ff")),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "pull.ff must be true, false, or only",
        )),
    }
}

async fn push(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let branch = current_branch(&project_path).await?;
    if branch == "HEAD" {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Cannot push from a detached HEAD. Check out a local branch first.",
        ));
    }
    let branch = validate_branch_name(&branch)?;
    let target = require_configured_git_push_target(
        configured_git_push_target(&project_path, &branch).await?,
    )?;
    let remote_name = target.remote_name;
    let remote_branch = target.remote_branch;
    let set_upstream = target.set_upstream;
    // `git push <remote> <name>` treats `<name>` as a *local source* ref.
    // When a local branch tracks a differently named remote branch, passing
    // the upstream name here either pushes an unrelated local branch or
    // fails.  Pin both sides of the refspec explicitly.
    let refspec = format!("refs/heads/{branch}:refs/heads/{remote_branch}");
    let mut args = vec!["push".to_string()];
    if set_upstream {
        args.push("--set-upstream".to_string());
    }
    args.push(remote_name.clone());
    args.push(refspec);
    let output = git(&project_path, args).await?;
    let mut response =
        GitOperationResponse::success(join_output_or(&output, "Push completed successfully"));
    response.remote_name = Some(remote_name);
    response.remote_branch = Some(remote_branch);
    response.branch = Some(branch);
    Ok(Json(response))
}

async fn publish(
    State(state): State<AppState>,
    Json(body): Json<BranchBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let branch = validate_branch_name(&body.branch)?;
    let current = current_branch(&project_path).await?;
    if current != branch {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!("Branch mismatch. Current branch is {current}, but trying to publish {branch}"),
        ));
    }
    // The legacy publish endpoint has no remote selector. Honor an explicit
    // Git pushRemote/remote.pushDefault resolution when it names one branch;
    // otherwise retain the old one-remote compatibility path and never pick
    // `origin` from a multi-remote repository. New mobile clients expose a
    // named destination through `/remote/push`.
    let resolution = configured_git_push_target(&project_path, &branch).await?;
    let target = match resolution.target.clone() {
        Some(target) => target,
        None => match only_configured_remote(&project_path).await? {
            Some(remote_name) => GitPushTarget {
                remote_name: validate_remote_name(&remote_name)?,
                remote_branch: branch.clone(),
                set_upstream: false,
            },
            None => require_configured_git_push_target(resolution)?,
        },
    };
    let remote_name = target.remote_name;
    let remote_branch = target.remote_branch;
    let refspec = format!("refs/heads/{branch}:refs/heads/{remote_branch}");
    let output = git(
        &project_path,
        ["push", "--set-upstream", &remote_name, &refspec],
    )
    .await?;
    let mut response =
        GitOperationResponse::success(join_output_or(&output, "Branch published successfully"));
    response.remote_name = Some(remote_name);
    response.remote_branch = Some(remote_branch);
    response.branch = Some(branch);
    Ok(Json(response))
}

async fn stage(
    State(state): State<AppState>,
    Json(body): Json<FileBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_git_file_target(
        &project_path,
        &body.file,
        GitFileTargetPolicy::Stage,
    )
    .await?;
    ensure_index_mutation_is_recovery_path_or_edit_stop_or_no_workflow(
        &resolved.repository_root,
        &resolved.repository_relative_file,
        true,
    )
    .await?;
    git(
        &resolved.repository_root,
        ["add", "--", &resolved.repository_relative_file],
    )
    .await?;
    Ok(Json(GitOperationResponse::message(format!(
        "Staged {}",
        resolved.repository_relative_file
    ))))
}

async fn unstage(
    State(state): State<AppState>,
    Json(body): Json<FileBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_git_file_target(
        &project_path,
        &body.file,
        GitFileTargetPolicy::Unstage,
    )
    .await?;
    ensure_index_mutation_is_recovery_path_or_edit_stop_or_no_workflow(
        &resolved.repository_root,
        &resolved.repository_relative_file,
        false,
    )
    .await?;

    if repository_has_commits(&resolved.repository_root).await? {
        git(
            &resolved.repository_root,
            [
                "restore",
                "--staged",
                "--",
                &resolved.repository_relative_file,
            ],
        )
        .await?;
    } else {
        git(
            &resolved.repository_root,
            [
                "rm",
                "--cached",
                "-r",
                "--ignore-unmatch",
                "--",
                &resolved.repository_relative_file,
            ],
        )
        .await?;
    }

    Ok(Json(GitOperationResponse::message(format!(
        "Unstaged {}",
        resolved.repository_relative_file
    ))))
}

async fn apply_hunks(
    State(state): State<AppState>,
    Json(body): Json<HunkBody>,
) -> Result<Json<GitOperationResponse>> {
    if body.hunk_indexes.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "At least one hunk must be selected",
        ));
    }

    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_git_file_target(
        &project_path,
        &body.file,
        GitFileTargetPolicy::Hunks,
    )
    .await?;
    ensure_index_mutation_is_recovery_path_or_edit_stop_or_no_workflow(
        &resolved.repository_root,
        &resolved.repository_relative_file,
        false,
    )
    .await?;
    let operation = validate_hunk_operation(&body.operation)?;

    let diff_args = if operation == "unstage" {
        vec![
            "diff".to_string(),
            "--cached".to_string(),
            "--".to_string(),
            resolved.repository_relative_file.clone(),
        ]
    } else {
        vec![
            "diff".to_string(),
            "--".to_string(),
            resolved.repository_relative_file.clone(),
        ]
    };
    let diff = git(&resolved.repository_root, diff_args).await?.stdout;
    if diff.trim().is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!("No {operation} hunks found for this file"),
        ));
    }

    let patch = selected_hunk_patch(&diff, &body.hunk_indexes)?;
    apply_patch_to_index(&resolved.repository_root, &patch, operation == "unstage").await?;

    Ok(Json(GitOperationResponse::message(format!(
        "{} {} selected hunk(s) for {}",
        if operation == "unstage" {
            "Unstaged"
        } else {
            "Staged"
        },
        body.hunk_indexes.len(),
        resolved.repository_relative_file
    ))))
}

async fn resolve_conflict(
    State(state): State<AppState>,
    Json(body): Json<ResolveConflictBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_git_file_target(
        &project_path,
        &body.file,
        GitFileTargetPolicy::Inspect,
    )
    .await?;
    let status = repository_file_status(
        &resolved.repository_root,
        &resolved.repository_relative_file,
    )
    .await?
    .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Conflict file not found"))?;

    if !is_conflict_status(&status) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "File is not currently conflicted",
        ));
    }

    let resolution = validate_conflict_resolution(&body.resolution)?;
    match resolution {
        "ours" | "theirs" => {
            git(
                &resolved.repository_root,
                [
                    "checkout",
                    if resolution == "ours" {
                        "--ours"
                    } else {
                        "--theirs"
                    },
                    "--",
                    &resolved.repository_relative_file,
                ],
            )
            .await?;
        }
        "manual" => {
            let content = body.content.ok_or_else(|| {
                ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Manual resolution content is required",
                )
            })?;
            if !extract_conflict_regions(&content).is_empty() {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Manual resolution still contains conflict markers",
                ));
            }
            let target = safe_repo_child(
                &resolved.repository_root,
                &resolved.repository_relative_file,
            )?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).await.map_err(io_server_error)?;
            }
            fs::write(&target, content).await.map_err(io_server_error)?;
        }
        _ => unreachable!(),
    }

    if body.stage {
        stage_resolved_path(
            &resolved.repository_root,
            &resolved.repository_relative_file,
        )
        .await?;
    }

    Ok(Json(GitOperationResponse::message(format!(
        "Resolved {} using {}{}",
        resolved.repository_relative_file,
        resolution,
        if body.stage { " and staged it" } else { "" }
    ))))
}

/// During a recoverable Git operation, the index is part of the operation's
/// state. Staging an unrelated file can silently include it in the eventual
/// merge/rebase/cherry-pick commit. Keep the escape hatch needed for a
/// manually resolved conflict, but reject every other generic index change.
///
/// An interactive-rebase `edit` stop is deliberately different: Git has
/// replayed the selected commit and paused specifically for normal index
/// edits followed by `commit --amend`. Allow stage, unstage, and partial-hunk
/// edits only for that marker-backed state; ordinary rebase conflicts retain
/// their existing conflict-path-only staging policy.
async fn ensure_index_mutation_is_recovery_path_or_edit_stop_or_no_workflow(
    repository_root: &Path,
    repository_relative_file: &str,
    allow_conflict_resolution: bool,
) -> Result<()> {
    let Some(workflow) = detect_git_workflow(repository_root).await? else {
        return Ok(());
    };
    if workflow.kind.is_edit_stop() {
        return Ok(());
    }
    if allow_conflict_resolution {
        let status = repository_file_status(repository_root, repository_relative_file).await?;
        if status.as_deref().is_some_and(is_conflict_status) {
            return Ok(());
        }
    }
    Err(ServerError::new(
        StatusCode::CONFLICT,
        format!(
            "A {} is in progress. Only a conflicted path may be staged while resolving it; resolve or abort the operation before changing the index.",
            workflow.kind.wire_name(),
        ),
    ))
}

async fn discard(
    State(state): State<AppState>,
    Json(body): Json<FileBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let resolved = resolve_git_file_target(
        &project_path,
        &body.file,
        GitFileTargetPolicy::Discard,
    )
    .await?;
    let discarded = discard_repository_path(
        &resolved.repository_root,
        &resolved.repository_relative_file,
    )
    .await?;
    if !discarded {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No changes to discard for this file",
        ));
    }
    Ok(Json(GitOperationResponse::message(format!(
        "Changes discarded for {}",
        resolved.repository_relative_file
    ))))
}

async fn discard_all(
    State(state): State<AppState>,
    Json(body): Json<GitDiscardAllRequest>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "discard all changes including staged content")?;
    let project_path = resolve_git_repository_path(&state, &body.project, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let resolved = resolve_git_file_target(
        &project_path,
        &body.file,
        GitFileTargetPolicy::Discard,
    )
    .await?;
    let discarded = discard_all_repository_path(
        &resolved.repository_root,
        &resolved.repository_relative_file,
    )
    .await?;
    if !discarded {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No changes to discard for this file",
        ));
    }
    Ok(Json(GitOperationResponse::message(format!(
        "All changes, including staged content, discarded for {}",
        resolved.repository_relative_file
    ))))
}

async fn delete_untracked(
    State(state): State<AppState>,
    Json(body): Json<FileBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let resolved = resolve_git_file_target(
        &project_path,
        &body.file,
        GitFileTargetPolicy::Discard,
    )
    .await?;
    let status = git(
        &resolved.repository_root,
        [
            "status",
            "--porcelain",
            "--",
            &resolved.repository_relative_file,
        ],
    )
    .await?
    .stdout;
    if !status.starts_with("??") {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "File is not untracked. Use discard for tracked files.",
        ));
    }

    let path = safe_repo_child(
        &resolved.repository_root,
        &resolved.repository_relative_file,
    )?;
    let metadata = fs::metadata(&path).await.map_err(io_server_error)?;
    if metadata.is_dir() {
        fs::remove_dir_all(&path).await.map_err(io_server_error)?;
    } else {
        fs::remove_file(&path).await.map_err(io_server_error)?;
    }
    Ok(Json(GitOperationResponse::message(format!(
        "Untracked path {} deleted successfully",
        resolved.repository_relative_file
    ))))
}

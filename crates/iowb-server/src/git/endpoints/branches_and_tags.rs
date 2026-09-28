async fn branches(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitBranchesResponse>> {
    let project_path = resolve_git_repository_path(&state, query.project_ref()?, query.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let output = git(
        &project_path,
        [
            "for-each-ref",
            "--sort=refname",
            // `for-each-ref` treats `%x1f` as literal text (unlike
            // `git log --format`), so put the control separator directly in
            // the argument. Git ref names cannot contain control characters.
            "--format=%(refname)\u{1f}%(symref)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .await?
    .stdout;
    let mut local_branches = Vec::new();
    let mut remote_branches = Vec::new();

    for line in output.lines().filter(|line| !line.is_empty()) {
        let Some((reference, symbolic_target)) = line.split_once('\u{1f}') else {
            continue;
        };
        // `for-each-ref` exposes remote HEAD aliases as symbolic refs. They
        // are navigation metadata, not checkout targets, so keep them out of
        // the branch picker.
        if !symbolic_target.is_empty() {
            continue;
        }
        if let Some(branch) = reference.strip_prefix("refs/heads/") {
            push_unique(&mut local_branches, branch.to_string());
        } else if let Some(remote_branch) = reference.strip_prefix("refs/remotes/")
            && let Some((remote, branch)) = remote_branch.split_once('/')
            && !remote.is_empty()
            && !branch.is_empty()
        {
            // A remote branch must retain its remote identity.  Returning only
            // `feature` would collapse `origin/feature` and
            // `upstream/feature`, and prevents the mobile client from safely
            // offering tracking checkout or remote deletion.
            push_unique(&mut remote_branches, format!("{remote}/{branch}"));
        }
    }

    let mut all = local_branches.clone();
    for branch in &remote_branches {
        push_unique(&mut all, branch.clone());
    }

    Ok(Json(GitBranchesResponse {
        branches: all,
        local_branches,
        remote_branches,
    }))
}

async fn checkout(
    State(state): State<AppState>,
    Json(body): Json<BranchBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    validate_branch_name(&body.branch)?;
    // Git checkout overwrites ignored files by default.  An ignored local
    // build artifact can still be valuable user data, so make ordinary branch
    // switching obey the same no-overwrite contract as the guarded
    // create-and-checkout route below.
    let output = git(
        &project_path,
        ["checkout", "--no-overwrite-ignore", body.branch.trim()],
    )
    .await?;
    Ok(Json(GitOperationResponse::success(join_output(&output))))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrackingCheckoutBody {
    #[serde(flatten)]
    project: ProjectBody,
    remote: String,
    branch: String,
}

/// Check out a local branch that tracks one explicitly selected remote ref.
///
/// `git checkout origin/topic` detaches HEAD, which is rarely what a touch UI
/// means by checking out a remote branch.  This narrow operation creates the
/// conventional local `topic` branch only when it does not exist; if it does
/// exist we merely switch to it rather than silently replacing its upstream.
async fn checkout_tracking_branch(
    State(state): State<AppState>,
    Json(body): Json<TrackingCheckoutBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(
        &state,
        body.project.project_ref()?,
        body.project.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let remote = validate_remote_name(&body.remote)?;
    let branch = validate_branch_name(&body.branch)?;
    let remote_ref = format!("{remote}/{branch}");
    let remote_object = format!("refs/remotes/{remote}/{branch}^{{commit}}");
    git(&project_path, ["rev-parse", "--verify", "--quiet", &remote_object])
        .await
        .map_err(|_| {
            ServerError::new(
                StatusCode::NOT_FOUND,
                format!("Remote branch {remote_ref} was not found. Fetch {remote} and try again."),
            )
        })?;

    let local_ref = format!("refs/heads/{branch}");
    let local_exists = git(&project_path, ["show-ref", "--verify", "--quiet", &local_ref])
        .await
        .is_ok();
    let output = if local_exists {
        git(&project_path, ["checkout", "--no-overwrite-ignore", &branch]).await?
    } else {
        git(
            &project_path,
            [
                "checkout",
                "--no-overwrite-ignore",
                "--track",
                "-b",
                &branch,
                &remote_ref,
            ],
        )
        .await?
    };
    let mut response = GitOperationResponse::success(join_output_or(
        &output,
        "Checked out local tracking branch successfully",
    ));
    response.branch = Some(branch);
    response.remote_name = Some(remote);
    response.remote_branch = Some(remote_ref);
    Ok(Json(response))
}

async fn create_branch(
    State(state): State<AppState>,
    Json(body): Json<BranchBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    validate_branch_name(&body.branch)?;
    let output = git(&project_path, ["checkout", "-b", body.branch.trim()]).await?;
    Ok(Json(GitOperationResponse::success(join_output(&output))))
}

/// The normal mobile "new branch" operation.  Unlike the legacy
/// `/api/git/create-branch` compatibility route, this pins the checkout the
/// user reviewed before it creates a branch and switches to it.  A branch
/// created from another ref can change files, so it requires a clean tree;
/// creating from the current HEAD is safe to do with ordinary local edits
/// intact because it only changes the branch name attached to that commit.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAndCheckoutBranchBody {
    #[serde(flatten)]
    project: ProjectBody,
    branch: String,
    #[serde(default)]
    start_point: Option<String>,
    #[serde(default)]
    expected_head: Option<String>,
    #[serde(default)]
    expected_target_ref: Option<String>,
    #[serde(default)]
    confirm: bool,
}

async fn create_and_checkout_branch(
    State(state): State<AppState>,
    Json(body): Json<CreateAndCheckoutBranchBody>,
) -> Result<Json<GitOperationResponse>> {
    advanced_require_confirmation(body.confirm, "create and check out a branch")?;
    let project_path = resolve_git_repository_path(
        &state,
        body.project.project_ref()?,
        body.project.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let branch = validate_branch_name(&body.branch)?;
    let has_commits = repository_has_commits(&project_path).await?;

    match (
        body.expected_head.as_deref().map(str::trim).filter(|value| !value.is_empty()),
        body.expected_target_ref
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    ) {
        (Some(expected_head), Some(expected_target_ref)) => {
            validate_workflow_target_snapshot(
                &project_path,
                expected_head,
                expected_target_ref,
                "new-branch checkout",
            )
            .await?;
        }
        (None, None) if !has_commits => {
            // An unborn branch has no immutable commit ID to pin. Git's
            // checkout itself still atomically changes the symbolic HEAD.
        }
        (None, None) => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "expectedHead and expectedTargetRef are required to create and check out a branch with commits",
            ));
        }
        _ => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "expectedHead and expectedTargetRef must be supplied together",
            ));
        }
    }

    let start_point = body
        .start_point
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let output = if let Some(start_point) = start_point {
        let target = resolve_commit_reference(&project_path, &start_point).await?;
        if has_commits && target != resolve_commit_reference(&project_path, "HEAD").await? {
            advanced_require_clean_worktree(
                &project_path,
                "create and check out a branch from another ref",
            )
            .await?;
        }
        // `git checkout` otherwise defaults to --overwrite-ignore and can
        // silently replace an ignored local file with a tracked file from the
        // selected start point. Native checkout never needs that destructive
        // shortcut, so keep the same protection for both branch-creation
        // forms.
        git(
            &project_path,
            ["checkout", "--no-overwrite-ignore", "-b", &branch, &target],
        )
        .await?
    } else {
        git(
            &project_path,
            ["checkout", "--no-overwrite-ignore", "-b", &branch],
        )
        .await?
    };

    let mut response = GitOperationResponse::success(join_output_or(
        &output,
        "Created and checked out branch successfully",
    ));
    response.branch = Some(branch);
    response.operation_state = Some(git_operation_state(&project_path).await?);
    Ok(Json(response))
}

async fn delete_branch(
    State(state): State<AppState>,
    Json(body): Json<BranchBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    validate_branch_name(&body.branch)?;
    let current = current_branch(&project_path).await?;
    if current == body.branch.trim() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Cannot delete the currently checked-out branch",
        ));
    }

    let output = git(&project_path, ["branch", "-d", body.branch.trim()]).await?;
    Ok(Json(GitOperationResponse::success(join_output(&output))))
}

async fn commits(
    State(state): State<AppState>,
    Query(query): Query<CommitsQuery>,
) -> Result<Json<GitCommitsResponse>> {
    let project_path = resolve_git_repository_path(&state, query.project_ref()?, query.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    if !repository_has_commits(&project_path).await? {
        return Ok(Json(GitCommitsResponse {
            commits: Vec::new(),
        }));
    }

    let limit = query.limit.unwrap_or(10).clamp(1, 100);
    let mut args = vec![
        "log".to_string(),
        "--pretty=format:%H|%an|%ae|%ad|%s".to_string(),
        "--date=iso-strict".to_string(),
        "-n".to_string(),
        limit.to_string(),
    ];
    if let Some(reference) = query.reference.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
        // Resolve up front so the log is tied to an immutable commit and an
        // invalid branch/ref is reported clearly rather than interpreted as
        // a log pathspec.
        args.push(resolve_commit_reference(&project_path, reference).await?);
    }
    let output = git(&project_path, args).await?.stdout;
    let mut commits = Vec::new();

    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let mut parts = line.split('|');
        let Some(hash) = parts.next() else { continue };
        let Some(author) = parts.next() else { continue };
        let Some(email) = parts.next() else { continue };
        let Some(date) = parts.next() else { continue };
        let message = parts.collect::<Vec<_>>().join("|");
        let stats = git(&project_path, ["show", "--stat", "--format=", hash])
            .await
            .ok()
            .and_then(|output| {
                output
                    .stdout
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .map(str::trim)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        commits.push(GitCommitSummary {
            hash: hash.to_string(),
            author: author.to_string(),
            email: email.to_string(),
            date: date.to_string(),
            message,
            stats,
        });
    }

    Ok(Json(GitCommitsResponse { commits }))
}

async fn commit_diff(
    State(state): State<AppState>,
    Query(query): Query<CommitDiffQuery>,
) -> Result<Json<GitDiffResponse>> {
    let project_path = resolve_git_repository_path(&state, query.project_ref()?, query.repository_id.as_deref()).await?;
    let commit = resolve_commit_reference(&project_path, &query.commit).await?;
    let output = git(&project_path, ["show", "--submodule=log", commit.as_str()])
        .await?
        .stdout;
    let is_truncated = output.len() > COMMIT_DIFF_CHARACTER_LIMIT;
    let diff = if is_truncated {
        format!(
            "{}\n\n... Diff truncated to keep the UI responsive ...",
            output
                .chars()
                .take(COMMIT_DIFF_CHARACTER_LIMIT)
                .collect::<String>()
        )
    } else {
        output
    };

    Ok(Json(GitDiffResponse { diff, is_truncated }))
}

async fn stashes(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitStashesResponse>> {
    let project_path = resolve_git_repository_path(&state, query.project_ref()?, query.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let output = git(
        &project_path,
        // A stash's visible name is its reflog subject (`%gs`), rather than
        // the immutable stash commit subject (`%s`). This is what `git stash
        // list` and LazyGit display, and it lets a later `stash store -m`
        // rename be visible without rewriting the stash object itself.
        ["stash", "list", "--format=%gd%x1f%H%x1f%an%x1f%aI%x1f%gs"],
    )
    .await?
    .stdout;
    let stashes = output
        .lines()
        .filter_map(parse_stash_summary)
        .collect::<Vec<_>>();
    Ok(Json(GitStashesResponse { stashes }))
}

async fn create_stash(
    State(state): State<AppState>,
    Json(body): Json<StashBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let message = validate_optional_git_message(body.message.as_deref())?;
    // Match Git and LazyGit's ordinary stash semantics: capture tracked
    // changes only.  Untracked and ignored files are intentionally opt-in in
    // the advanced stash flow, where the user can see that they will be
    // removed from the working tree.
    let output = if let Some(message) = message {
        git(&project_path, ["stash", "push", "-m", &message]).await?
    } else {
        git(&project_path, ["stash", "push"]).await?
    };
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Stash saved successfully",
    ))))
}

async fn apply_stash(
    State(state): State<AppState>,
    Json(body): Json<StashRefBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let reference = validate_stash_ref(&body.reference)?;
    Ok(Json(
        management_apply_stash_with_recovery(
            &project_path,
            &reference,
            false,
            false,
            body.expected_hash.as_deref(),
        )
        .await?,
    ))
}

async fn pop_stash(
    State(state): State<AppState>,
    Json(body): Json<StashRefBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let reference = validate_stash_ref(&body.reference)?;
    Ok(Json(
        management_apply_stash_with_recovery(
            &project_path,
            &reference,
            false,
            true,
            body.expected_hash.as_deref(),
        )
        .await?,
    ))
}

async fn drop_stash(
    State(state): State<AppState>,
    Json(body): Json<StashRefBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let reference = validate_stash_ref(&body.reference)?;
    let snapshot = management_stash_snapshot_for_action(
        &project_path,
        &reference,
        body.expected_hash.as_deref(),
    )
    .await?;
    let output = git(&project_path, ["stash", "drop", &snapshot.reference]).await?;
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Stash dropped successfully",
    ))))
}

async fn tags(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitTagsResponse>> {
    let project_path = resolve_git_repository_path(&state, query.project_ref()?, query.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let output = git(
        &project_path,
        [
            "for-each-ref",
            "refs/tags",
            "--sort=-creatordate",
            "--format=%(refname:short)\u{1f}%(objectname)\u{1f}%(objecttype)\u{1f}%(creatordate:iso-strict)\u{1f}%(subject)",
        ],
    )
    .await?
    .stdout;
    let tags = output
        .lines()
        .filter_map(parse_tag_summary)
        .collect::<Vec<_>>();
    Ok(Json(GitTagsResponse { tags }))
}

async fn create_tag(
    State(state): State<AppState>,
    Json(body): Json<TagBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    if body.force {
        advanced_require_confirmation(body.confirm, "replace an existing local tag")?;
    }
    let tag = validate_tag_name(&body.tag)?;
    ensure_new_local_tag_or_explicit_replacement(&project_path, &tag, body.force).await?;
    let message = validate_optional_git_message(body.message.as_deref())?;
    let mut args = vec!["tag".to_string()];
    if body.force {
        args.push("--force".to_string());
    }
    if let Some(message) = message {
        args.push("-a".to_string());
        args.push(tag.clone());
        args.push("-m".to_string());
        args.push(message);
    } else {
        // The UI promises a lightweight tag when no annotation message is
        // supplied. Explicitly override a host/global tag.gpgSign setting so
        // that setting cannot silently turn this into an editor-backed signed
        // annotated tag. A signed tag necessarily has annotation content and
        // is therefore created through the message form above.
        args.push("--no-sign".to_string());
        args.push(tag.clone());
    }
    let output = git(&project_path, args).await?;
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Tag created successfully",
    ))))
}

async fn delete_tag(
    State(state): State<AppState>,
    Json(body): Json<TagBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let tag = validate_tag_name(&body.tag)?;
    let output = git(&project_path, ["tag", "-d", &tag]).await?;
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Tag deleted successfully",
    ))))
}

fn parse_stash_summary(line: &str) -> Option<GitStashSummary> {
    let mut parts = line.splitn(5, '\u{1f}');
    Some(GitStashSummary {
        reference: parts.next()?.to_string(),
        hash: parts.next()?.to_string(),
        author: parts.next()?.to_string(),
        date: parts.next()?.to_string(),
        message: parts.next()?.to_string(),
    })
}

fn parse_tag_summary(line: &str) -> Option<GitTagSummary> {
    let mut parts = line.splitn(5, '\u{1f}');
    Some(GitTagSummary {
        name: parts.next()?.to_string(),
        hash: parts.next()?.to_string(),
        object_type: parts.next()?.to_string(),
        date: parts.next()?.to_string(),
        message: parts.next()?.to_string(),
    })
}

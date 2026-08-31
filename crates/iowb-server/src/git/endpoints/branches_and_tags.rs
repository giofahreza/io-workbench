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
            "--format=%(refname)%x1f%(symref)",
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
            && let Some((_, branch)) = remote_branch.split_once('/')
        {
            push_unique(&mut remote_branches, branch.to_string());
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
    validate_branch_name(&body.branch)?;
    let output = git(&project_path, ["checkout", body.branch.trim()]).await?;
    Ok(Json(GitOperationResponse::success(join_output(&output))))
}

async fn create_branch(
    State(state): State<AppState>,
    Json(body): Json<BranchBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_branch_name(&body.branch)?;
    let output = git(&project_path, ["checkout", "-b", body.branch.trim()]).await?;
    Ok(Json(GitOperationResponse::success(join_output(&output))))
}

async fn delete_branch(
    State(state): State<AppState>,
    Json(body): Json<BranchBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
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
    let output = git(
        &project_path,
        [
            "log",
            "--pretty=format:%H|%an|%ae|%ad|%s",
            "--date=iso-strict",
            "-n",
            &limit.to_string(),
        ],
    )
    .await?
    .stdout;
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
        ["stash", "list", "--format=%gd%x1f%H%x1f%an%x1f%aI%x1f%s"],
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
    let message = validate_optional_git_message(body.message.as_deref())?;
    let output = if let Some(message) = message {
        git(&project_path, ["stash", "push", "-u", "-m", &message]).await?
    } else {
        git(&project_path, ["stash", "push", "-u"]).await?
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
    let output = git(&project_path, ["stash", "apply", &reference]).await?;
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Stash applied successfully",
    ))))
}

async fn pop_stash(
    State(state): State<AppState>,
    Json(body): Json<StashRefBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let reference = validate_stash_ref(&body.reference)?;
    let output = git(&project_path, ["stash", "pop", &reference]).await?;
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Stash popped successfully",
    ))))
}

async fn drop_stash(
    State(state): State<AppState>,
    Json(body): Json<StashRefBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let reference = validate_stash_ref(&body.reference)?;
    let output = git(&project_path, ["stash", "drop", &reference]).await?;
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
            "--format=%(refname:short)%1f%(objectname)%1f%(objecttype)%1f%(creatordate:iso-strict)%1f%(subject)",
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
    let tag = validate_tag_name(&body.tag)?;
    let message = validate_optional_git_message(body.message.as_deref())?;
    let output = if let Some(message) = message {
        git(&project_path, ["tag", "-a", &tag, "-m", &message]).await?
    } else {
        git(&project_path, ["tag", &tag]).await?
    };
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
    let tag = validate_tag_name(&body.tag)?;
    let output = git(&project_path, ["tag", "-d", &tag]).await?;
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Tag deleted successfully",
    ))))
}

async fn push_tag(
    State(state): State<AppState>,
    Json(body): Json<TagBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let tag = validate_tag_name(&body.tag)?;
    let remote_name = first_remote(&project_path).await?.ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            "No remote repository configured. Add a remote first.",
        )
    })?;
    validate_remote_name(&remote_name)?;
    let output = git(&project_path, ["push", &remote_name, &tag]).await?;
    let mut response =
        GitOperationResponse::success(join_output_or(&output, "Tag pushed successfully"));
    response.remote_name = Some(remote_name);
    Ok(Json(response))
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

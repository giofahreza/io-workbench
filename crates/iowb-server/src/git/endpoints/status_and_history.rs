async fn status(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitStatusResponse>> {
    let (_, repository) = selected_git_repository(
        &state,
        query.project_ref()?,
        query.repository_id.as_deref(),
    )
    .await?;
    let project_path = repository.path.clone();
    validate_git_repository(&project_path).await?;
    let branch = current_branch(&project_path).await.ok();
    let has_commits = repository_has_commits(&project_path).await?;
    let output = git(
        &project_path,
        [
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )
    .await?;

    let mut modified = Vec::new();
    let mut added = Vec::new();
    let mut deleted = Vec::new();
    let mut untracked = Vec::new();
    let mut conflicted = Vec::new();
    let mut files = Vec::new();

    for (status, path, submodule_state) in parse_status_entries_detailed_bytes(&output.stdout_bytes) {
        if status == "??" {
            untracked.push(path.clone());
        } else if is_conflict_status(&status) {
            conflicted.push(path.clone());
        } else if status.contains('D') {
            deleted.push(path.clone());
        } else if status.contains('A') {
            added.push(path.clone());
        } else if status.contains('M') || status.contains('R') || status.contains('C') {
            modified.push(path.clone());
        }
        files.push(GitFileStatus {
            path,
            status,
            submodule_state,
        });
    }

    Ok(Json(GitStatusResponse {
        repository_id: Some(repository.id),
        repository_path: Some(repository.relative_path.clone()),
        repository_name: Some(repository.name),
        repository_kind: Some(repository.kind),
        branch,
        has_commits,
        modified,
        added,
        deleted,
        untracked,
        conflicted,
        clean: files.is_empty(),
        files,
        raw: output.stdout,
    }))
}

async fn conflicts(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitConflictsResponse>> {
    let project_path = resolve_git_repository_path(&state, query.project_ref()?, query.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let repository_root = repository_root(&project_path).await?;
    let output = git(
        &repository_root,
        [
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )
    .await?;
    let mut files = Vec::new();

    for (status, path, _) in parse_status_entries_detailed_bytes(&output.stdout_bytes)
        .into_iter()
        .filter(|(status, _, _)| is_conflict_status(status))
    {
        let content = read_repository_file_lossy(&repository_root, &path)
            .await
            .unwrap_or_default();
        files.push(GitConflictSummary {
            path,
            status,
            conflict_count: extract_conflict_regions(&content).len(),
        });
    }

    Ok(Json(GitConflictsResponse { files }))
}

async fn conflict_file(
    State(state): State<AppState>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<GitConflictFileResponse>> {
    let project_path = resolve_git_repository_path(&state, query.project_ref()?, query.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_git_file_target(
        &project_path,
        query.file_ref()?,
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

    let content = read_repository_file_lossy(
        &resolved.repository_root,
        &resolved.repository_relative_file,
    )
    .await
    .unwrap_or_default();
    let conflicts = extract_conflict_regions(&content);

    Ok(Json(GitConflictFileResponse {
        path: resolved.repository_relative_file,
        status,
        content,
        conflicts,
    }))
}

async fn init(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;

    // An uninitialized submodule is a valid catalog entry but is not itself a
    // worktree yet.  Initialize it from its owning repository rather than
    // running `git init` inside the checkout directory (which would create an
    // unrelated nested repository and break the parent gitlink).
    if let Some(repository_id) = body
        .repository_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        let catalog = discover_git_workspace(&project_path).await?;
        let repository = catalog
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)
            .ok_or_else(|| {
                ServerError::new(
                    StatusCode::NOT_FOUND,
                    "Git repository was not found in this workspace",
                )
            })?;
        if matches!(
            repository.kind,
            iowb_protocol::GitRepositoryKind::Uninitialized
        ) {
            let output = initialize_uninitialized_submodule(&catalog, repository).await?;
            return Ok(Json(GitOperationResponse::success(join_output_or(
                &output,
                "Submodule initialized successfully",
            ))));
        }
        if repository.initialized {
            return Ok(Json(GitOperationResponse::success(
                "Repository is already initialized",
            )));
        }
    }

    if let Some(ancestor) = enclosing_git_repository(&project_path).await {
        if !body.allow_workspace_init {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "This directory is inside the Git repository at {}. Confirm workspace initialization explicitly before creating a nested repository.",
                    ancestor.display()
                ),
            ));
        }
    }

    if validate_git_repository(&project_path).await.is_ok() {
        return Ok(Json(GitOperationResponse::success(
            "Repository is already initialized",
        )));
    }

    let catalog = discover_git_workspace(&project_path).await?;
    if !catalog.repositories.is_empty() && !body.allow_workspace_init {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "This directory is a Git workspace containing other repositories. Confirm workspace initialization explicitly before creating a parent repository.",
        ));
    }

    let output = match git(&project_path, ["init", "--initial-branch=main"]).await {
        Ok(output) => output,
        Err(error)
            if error
                .body
                .details
                .as_deref()
                .is_some_and(is_unknown_initial_branch) =>
        {
            let output = git(&project_path, ["init"]).await?;
            let _ = git(&project_path, ["symbolic-ref", "HEAD", "refs/heads/main"]).await;
            output
        }
        Err(error) => return Err(error),
    };

    Ok(Json(GitOperationResponse::success(join_output(&output))))
}

async fn diff(
    State(state): State<AppState>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<GitDiffResponse>> {
    let project_path = resolve_git_repository_path(&state, query.project_ref()?, query.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_git_file_target(
        &project_path,
        query.file_ref()?,
        GitFileTargetPolicy::Inspect,
    )
    .await?;
    let status_details = repository_file_status_details(
        &resolved.repository_root,
        &resolved.repository_relative_file,
    )
    .await?;

    if submodule_at_boundary(
        &resolved.repository_root,
        &resolved.repository_relative_file,
    )
    .await?
    .is_some()
    {
        return Ok(Json(GitDiffResponse {
            diff: submodule_diff(
                &resolved.repository_root,
                &resolved.repository_relative_file,
                query.staged,
            )
            .await?,
            is_truncated: false,
        }));
    }

    let status = status_details
        .as_ref()
        .map(|(status, _)| status.as_str())
        .unwrap_or_default();

    let diff = if status_is_untracked(status) {
        let file_path = resolved
            .repository_root
            .join(&resolved.repository_relative_file);
        let metadata = fs::metadata(&file_path).await.map_err(io_server_error)?;
        if metadata.is_dir() {
            format!(
                "Directory: {}\n(Cannot show diff for directories)",
                resolved.repository_relative_file
            )
        } else {
            let content = fs::read_to_string(&file_path)
                .await
                .map_err(io_server_error)?;
            let lines = content.lines().count().max(1);
            format!(
                "--- /dev/null\n+++ b/{}\n@@ -0,0 +1,{} @@\n{}",
                resolved.repository_relative_file,
                lines,
                content
                    .split('\n')
                    .map(|line| format!("+{line}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
    } else {
        let requested = match query.staged {
            Some(true) => git(
                &resolved.repository_root,
                [
                    "diff",
                    "--cached",
                    "--submodule=log",
                    "--",
                    &resolved.repository_relative_file,
                ],
            )
            .await?
            .stdout,
            Some(false) => git(
                &resolved.repository_root,
                [
                    "diff",
                    "--submodule=log",
                    "--",
                    &resolved.repository_relative_file,
                ],
            )
            .await?
            .stdout,
            None => git(
                &resolved.repository_root,
                [
                    "diff",
                    "--submodule=log",
                    "--",
                    &resolved.repository_relative_file,
                ],
            )
            .await?
            .stdout,
        };
        if !requested.trim().is_empty() || query.staged.is_some() {
            strip_diff_headers(&requested)
        } else {
            let cached = git(
                &resolved.repository_root,
                [
                    "diff",
                    "--cached",
                    "--submodule=log",
                    "--",
                    &resolved.repository_relative_file,
                ],
            )
            .await?
            .stdout;
            strip_diff_headers(&cached)
        }
    };

    // Keep the old synthetic representation for a deleted file only when the
    // caller did not request a specific side of a mixed index/worktree diff.
    let diff = if status_is_deleted(status) && query.staged.is_none() && diff.trim().is_empty() {
        let content = git(
            &resolved.repository_root,
            [
                "show",
                &format!("HEAD:{}", resolved.repository_relative_file),
            ],
        )
        .await?
        .stdout;
        let lines = content.lines().count().max(1);
        format!(
            "--- a/{}\n+++ /dev/null\n@@ -1,{} +0,0 @@\n{}",
            resolved.repository_relative_file,
            lines,
            content
                .split('\n')
                .map(|line| format!("-{line}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    } else {
        diff
    };

    Ok(Json(GitDiffResponse {
        diff,
        is_truncated: false,
    }))
}

async fn file_with_diff(
    State(state): State<AppState>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<GitFileWithDiffResponse>> {
    let project_path = resolve_git_repository_path(&state, query.project_ref()?, query.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_git_file_target(
        &project_path,
        query.file_ref()?,
        GitFileTargetPolicy::Inspect,
    )
    .await?;
    let status_details = repository_file_status_details(
        &resolved.repository_root,
        &resolved.repository_relative_file,
    )
    .await?;

    if submodule_at_boundary(
        &resolved.repository_root,
        &resolved.repository_relative_file,
    )
    .await?
    .is_some()
    {
        return Ok(Json(GitFileWithDiffResponse {
            current_content: String::new(),
            old_content: String::new(),
            is_deleted: false,
            is_untracked: false,
            submodule_state: status_details.and_then(|(_, state)| state),
            submodule_diff: Some(
            submodule_diff(
                &resolved.repository_root,
                &resolved.repository_relative_file,
                query.staged,
            )
                .await?,
            ),
        }));
    }

    let status = status_details
        .as_ref()
        .map(|(status, _)| status.as_str())
        .unwrap_or_default();
    let is_untracked = status_is_untracked(status);
    let is_deleted = status_is_deleted(status);

    let (current_content, old_content) = if is_deleted {
        let old = git(
            &resolved.repository_root,
            [
                "show",
                &format!("HEAD:{}", resolved.repository_relative_file),
            ],
        )
        .await?
        .stdout;
        (old.clone(), old)
    } else {
        let file_path = resolved
            .repository_root
            .join(&resolved.repository_relative_file);
        let metadata = fs::metadata(&file_path).await.map_err(io_server_error)?;
        if metadata.is_dir() {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Cannot show diff for directories",
            ));
        }
        let current = fs::read_to_string(&file_path)
            .await
            .map_err(io_server_error)?;
        let old = if is_untracked {
            String::new()
        } else {
            git(
                &resolved.repository_root,
                [
                    "show",
                    &format!("HEAD:{}", resolved.repository_relative_file),
                ],
            )
            .await
            .map(|output| output.stdout)
            .unwrap_or_default()
        };
        (current, old)
    };

    Ok(Json(GitFileWithDiffResponse {
        current_content,
        old_content,
        is_deleted,
        is_untracked,
        submodule_state: None,
        submodule_diff: None,
    }))
}

async fn initial_commit(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    if repository_has_commits(&project_path).await? {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Repository already has commits. Use regular commit instead.",
        ));
    }

    let catalog = discover_git_workspace(&project_path).await?;
    // An independent nested repository is its own worktree. Exclude those
    // boundaries in the pathspec so the parent's first commit cannot create a
    // gitlink, even transiently.
    let add_args = initial_commit_add_args(&catalog);
    git_with_pathspec_magic(&project_path, add_args).await?;
    let output = git(
        &project_path,
        ["commit", "--allow-empty", "-m", "Initial commit"],
    )
    .await?;
    let mut response = GitOperationResponse::success(join_output(&output));
    response.message = Some("Initial commit created successfully".to_string());
    Ok(Json(response))
}

fn initial_commit_add_args(catalog: &GitWorkspaceCatalog) -> Vec<String> {
    let mut args = vec![
        "add".to_string(),
        "-A".to_string(),
        "--".to_string(),
        ".".to_string(),
    ];
    args.extend(
        catalog
            .repositories
            .iter()
            .filter(|repository| {
                matches!(
                    repository.kind,
                    GitRepositoryKind::Nested
                        | GitRepositoryKind::Worktree
                        | GitRepositoryKind::Uninitialized
                )
            })
            .map(|repository| format!(":(top,literal,exclude){}", repository.relative_path)),
    );
    args
}

async fn commit(
    State(state): State<AppState>,
    Json(body): Json<CommitBody>,
) -> Result<Json<GitOperationResponse>> {
    if body.message.trim().is_empty() || body.files.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Project name, commit message, and files are required",
        ));
    }
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    let repository_root = repository_root(&project_path).await?;

    for file in &body.files {
        let resolved = resolve_git_file_target(&project_path, file, GitFileTargetPolicy::Commit).await?;
        git(
            &repository_root,
            ["add", "--", &resolved.repository_relative_file],
        )
        .await?;
    }

    let output = git(&repository_root, ["commit", "-m", body.message.trim()]).await?;
    Ok(Json(GitOperationResponse::success(join_output(&output))))
}

async fn revert_local_commit(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(&state, body.project_ref()?, body.repository_id.as_deref()).await?;
    validate_git_repository(&project_path).await?;
    if !repository_has_commits(&project_path).await? {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No local commit to revert",
        ));
    }

    match git(&project_path, ["reset", "--soft", "HEAD~1"]).await {
        Ok(_) => {}
        Err(error)
            if error
                .body
                .details
                .as_deref()
                .is_some_and(is_missing_head_parent) =>
        {
            git(&project_path, ["update-ref", "-d", "HEAD"]).await?;
        }
        Err(error) => return Err(error),
    }

    Ok(Json(GitOperationResponse::success(
        "Latest local commit reverted successfully. Changes were kept staged.",
    )))
}

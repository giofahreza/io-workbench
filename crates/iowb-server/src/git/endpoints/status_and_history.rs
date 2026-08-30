async fn status(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitStatusResponse>> {
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let branch = current_branch(&project_path).await.ok();
    let has_commits = repository_has_commits(&project_path).await?;
    let output = git(
        &project_path,
        ["status", "--porcelain", "--untracked-files=all"],
    )
    .await?;

    let mut modified = Vec::new();
    let mut added = Vec::new();
    let mut deleted = Vec::new();
    let mut untracked = Vec::new();
    let mut conflicted = Vec::new();
    let mut files = Vec::new();

    for (status, path) in parse_status_entries(&output.stdout) {
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
        files.push(GitFileStatus { path, status });
    }

    Ok(Json(GitStatusResponse {
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
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let repository_root = repository_root(&project_path).await?;
    let output = git(
        &repository_root,
        ["status", "--porcelain", "--untracked-files=all"],
    )
    .await?;
    let mut files = Vec::new();

    for (status, path) in parse_status_entries(&output.stdout)
        .into_iter()
        .filter(|(status, _)| is_conflict_status(status))
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
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_repository_file_path(&project_path, query.file_ref()?).await?;
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

    if validate_git_repository(&project_path).await.is_ok() {
        return Ok(Json(GitOperationResponse::success(
            "Repository is already initialized",
        )));
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
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_repository_file_path(&project_path, query.file_ref()?).await?;
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

    let diff = if status.starts_with("??") {
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
    } else if status.trim_start().starts_with('D') {
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
        let unstaged = git(
            &resolved.repository_root,
            ["diff", "--", &resolved.repository_relative_file],
        )
        .await?
        .stdout;
        if unstaged.trim().is_empty() {
            strip_diff_headers(
                &git(
                    &resolved.repository_root,
                    ["diff", "--cached", "--", &resolved.repository_relative_file],
                )
                .await?
                .stdout,
            )
        } else {
            strip_diff_headers(&unstaged)
        }
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
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_repository_file_path(&project_path, query.file_ref()?).await?;
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
    let is_untracked = status.starts_with("??");
    let is_deleted = status.trim_start().starts_with('D');

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
    }))
}

async fn initial_commit(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    if repository_has_commits(&project_path).await? {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Repository already has commits. Use regular commit instead.",
        ));
    }

    git(&project_path, ["add", "."]).await?;
    let output = git(&project_path, ["commit", "-m", "Initial commit"]).await?;
    let mut response = GitOperationResponse::success(join_output(&output));
    response.message = Some("Initial commit created successfully".to_string());
    Ok(Json(response))
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
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let repository_root = repository_root(&project_path).await?;

    for file in &body.files {
        let resolved = resolve_repository_file_path(&project_path, file).await?;
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
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
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

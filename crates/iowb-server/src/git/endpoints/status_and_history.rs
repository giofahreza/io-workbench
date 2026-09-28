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
    let target_ref = Some(checked_out_target_ref(&project_path).await?);
    let has_commits = repository_has_commits(&project_path).await?;
    let head = if has_commits {
        Some(resolve_commit_reference(&project_path, "HEAD").await?)
    } else {
        None
    };
    // An unresolved index cannot be written to a tree. Keep status available
    // during conflict recovery, but omit the commit snapshot until the index
    // is resolved and can be safely reviewed again.
    let index_tree = index_tree_snapshot(&project_path).await.ok();
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
    let (staged_line_stats, unstaged_line_stats) = tokio::join!(
        git_line_change_stats(&project_path, true),
        git_line_change_stats(&project_path, false),
    );
    // Status should remain useful even when a repository has an unusual diff
    // driver or an unreadable path. The line counts are supplemental UI data,
    // so fall back to zero rather than failing the entire status request.
    let staged_line_stats = staged_line_stats.unwrap_or_default();
    let unstaged_line_stats = unstaged_line_stats.unwrap_or_default();

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
        let (staged_additions, staged_deletions) =
            staged_line_stats.get(&path).copied().unwrap_or_default();
        let (mut unstaged_additions, unstaged_deletions) =
            unstaged_line_stats.get(&path).copied().unwrap_or_default();
        if status == "??" && unstaged_additions == 0 && unstaged_deletions == 0 {
            unstaged_additions = untracked_line_count(&project_path, &path).await.unwrap_or(0);
        }
        files.push(GitFileStatus {
            path,
            status,
            submodule_state,
            staged_additions,
            staged_deletions,
            unstaged_additions,
            unstaged_deletions,
        });
    }

    Ok(Json(GitStatusResponse {
        repository_id: Some(repository.id),
        repository_path: Some(repository.relative_path.clone()),
        repository_name: Some(repository.name),
        repository_kind: Some(repository.kind),
        branch,
        target_ref,
        has_commits,
        head,
        index_tree,
        modified,
        added,
        deleted,
        untracked,
        conflicted,
        clean: files.is_empty(),
        files,
        raw: output.stdout,
        operation_state: Some(git_operation_state(&project_path).await?),
    }))
}

/// Return the immutable tree object represented by the current index.
///
/// `git write-tree` deliberately fails for an unmerged index; callers that
/// render status may handle that as an unavailable snapshot, while commit
/// must require a valid tree before it can proceed.
async fn index_tree_snapshot(repository_path: &Path) -> Result<String> {
    let tree = git(repository_path, ["write-tree"]).await?.stdout.trim().to_string();
    if tree.is_empty() {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Git could not create an index snapshot. Resolve the index and refresh Git status before committing.",
        ));
    }
    Ok(tree)
}

async fn git_line_change_stats(
    repository_path: &Path,
    staged: bool,
) -> Result<HashMap<String, (usize, usize)>> {
    let args = if staged {
        vec![
            "diff".to_string(),
            "--cached".to_string(),
            "--numstat".to_string(),
            "-z".to_string(),
            "--no-renames".to_string(),
            "--".to_string(),
        ]
    } else {
        vec![
            "diff".to_string(),
            "--numstat".to_string(),
            "-z".to_string(),
            "--no-renames".to_string(),
            "--".to_string(),
        ]
    };
    let output = git(repository_path, args).await?;
    Ok(parse_git_numstat(&output.stdout_bytes))
}

fn parse_git_numstat(output: &[u8]) -> HashMap<String, (usize, usize)> {
    output
        .split(|byte| *byte == 0)
        .filter_map(|record| {
            let mut fields = record.splitn(3, |byte| *byte == b'\t');
            let additions = parse_numstat_count(fields.next()?);
            let deletions = parse_numstat_count(fields.next()?);
            let path = normalize_repo_relative_path(&String::from_utf8_lossy(fields.next()?));
            (!path.is_empty()).then_some((path, (additions, deletions)))
        })
        .collect()
}

fn parse_numstat_count(value: &[u8]) -> usize {
    std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
}

async fn untracked_line_count(repository_path: &Path, relative_path: &str) -> Result<usize> {
    let path = safe_repo_child(repository_path, relative_path)?;
    let metadata = fs::metadata(&path).await.map_err(io_server_error)?;
    if metadata.is_dir() {
        return Ok(0);
    }
    let content = fs::read(path).await.map_err(io_server_error)?;
    if content.is_empty() {
        return Ok(0);
    }
    Ok(content.iter().filter(|byte| **byte == b'\n').count()
        + usize::from(!content.ends_with(b"\n")))
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
    ensure_no_git_workflow_in_progress(&project_path).await?;
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
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let repository_root = repository_root(&project_path).await?;
    let expected_head = body
        .expected_head
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "expectedHead is required to commit. Refresh Git status and review the staged files before committing.",
            )
        })?;
    let expected_target_ref = body
        .expected_target_ref
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "expectedTargetRef is required to commit. Refresh Git status and review the staged files before committing.",
            )
        })?;
    // A commit mutates the checked-out branch as well as consuming the
    // reviewed index. Validate both target dimensions before checking the
    // index snapshot so a same-OID branch switch cannot commit the staged
    // files to a different branch.
    validate_workflow_target_snapshot(
        &project_path,
        expected_head,
        expected_target_ref,
        "commit",
    )
    .await?;
    let expected_index_tree = body
        .expected_index_tree
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "expectedIndexTree is required to commit. Refresh Git status and review the staged files before committing.",
            )
        })?;
    let current_index_tree = index_tree_snapshot(&project_path).await?;
    if current_index_tree != expected_index_tree {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The staged index changed after review. Refresh Git status and review the staged files before committing.",
        ));
    }
    validate_requested_staged_commit_files(&project_path, &body.files).await?;

    // The requested files must already be in the index. Do not run git add
    // here: doing so would silently include unstaged hunks from a partially
    // staged file and make the native diff/review UI lie about the commit.
    let output = git(&repository_root, ["commit", "-m", body.message.trim()]).await?;
    Ok(Json(GitOperationResponse::success(join_output(&output))))
}

#[derive(Debug, Deserialize)]
struct UndoLatestLocalCommitBody {
    #[serde(flatten)]
    project: ProjectBody,
    #[serde(rename = "expectedHead")]
    expected_head: Option<String>,
    /// The branch (or detached HEAD) identity displayed beside expectedHead.
    /// A full object ID alone cannot distinguish two local branches which
    /// currently point at the same commit.
    #[serde(rename = "expectedTargetRef")]
    expected_target_ref: Option<String>,
}

async fn revert_local_commit(
    State(state): State<AppState>,
    Json(body): Json<UndoLatestLocalCommitBody>,
) -> Result<Json<GitOperationResponse>> {
    let expected_head = body.expected_head.as_deref().ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            "expectedHead is required to undo the latest local commit. Refresh Git status and confirm again.",
        )
    })?;
    let expected_target_ref = body.expected_target_ref.as_deref().ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            "expectedTargetRef is required to undo the latest local commit. Refresh Git status and confirm again.",
        )
    })?;
    let project_path = resolve_git_repository_path(
        &state,
        body.project.project_ref()?,
        body.project.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    if !repository_has_commits(&project_path).await? {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No local commit to revert",
        ));
    }
    validate_workflow_target_snapshot(
        &project_path,
        expected_head,
        expected_target_ref,
        "undo",
    )
    .await?;
    let current_head = resolve_commit_reference(&project_path, "HEAD").await?;
    if undo_latest_commit_is_known_shared(&project_path, &current_head).await? {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Cannot undo this commit because it is already known to an upstream or remote-tracking branch. Use Revert commit to create a safe inverse commit instead.",
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
        "Undo latest local commit completed. Changes were kept staged.",
    )))
}

/// A soft reset rewrites history, so only let it act on a commit Git can
/// prove is local. A configured upstream catches the normal case; scanning
/// remote-tracking refs also protects detached/no-upstream branches whose
/// commit was fetched or pushed previously.
async fn undo_latest_commit_is_known_shared(repository: &Path, head: &str) -> Result<bool> {
    if let Ok(upstream) = git(
        repository,
        ["rev-parse", "--verify", "--end-of-options", "@{upstream}"],
    )
    .await
    {
        let upstream = upstream.stdout.trim();
        if !upstream.is_empty()
            && git(repository, ["merge-base", "--is-ancestor", head, upstream])
                .await
                .is_ok()
        {
            return Ok(true);
        }
    }

    let remote_refs = git(
        repository,
        [
            "for-each-ref",
            "--contains",
            head,
            "--format=%(refname)",
            "refs/remotes",
        ],
    )
    .await?;
    Ok(remote_refs.stdout.lines().any(|reference| !reference.trim().is_empty()))
}

async fn validate_requested_staged_commit_files(
    project_path: &Path,
    files: &[String],
) -> Result<Vec<String>> {
    let mut staged_paths = Vec::with_capacity(files.len());
    let mut seen = HashSet::new();

    for file in files {
        let resolved =
            resolve_git_file_target(project_path, file, GitFileTargetPolicy::Commit).await?;
        let status = repository_file_status(
            &resolved.repository_root,
            &resolved.repository_relative_file,
        )
        .await?;
        if !status
            .as_deref()
            .is_some_and(status_has_staged_change)
        {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "{} is not staged. Stage the exact changes you want to commit before committing.",
                    resolved.repository_relative_file
                ),
            ));
        }
        if seen.insert(resolved.repository_relative_file.clone()) {
            staged_paths.push(resolved.repository_relative_file);
        }
    }

    if staged_paths.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "At least one staged file is required",
        ));
    }
    Ok(staged_paths)
}

fn status_has_staged_change(status: &str) -> bool {
    let Some(index_status) = status.chars().next() else {
        return false;
    };
    index_status != '.' && index_status != ' ' && index_status != '?' && index_status != 'U'
}

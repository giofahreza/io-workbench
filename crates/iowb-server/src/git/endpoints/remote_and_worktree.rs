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
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let repository_root = repository_root(&project_path).await?;
    let diff_context = commit_message_diff_context(&project_path, &repository_root, &body.files)
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
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
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

    if !repository_has_commits(&project_path).await? {
        return Ok(Json(GitRemoteStatusResponse {
            has_remote,
            has_upstream: false,
            branch,
            remote_branch: None,
            remote_name: fallback_remote,
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
            &format!("{branch}@{{upstream}}"),
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
                ahead: 0,
                behind: 0,
                is_up_to_date: false,
                message: Some("No remote tracking branch configured".to_string()),
            }));
        }
    };
    let remote_name = tracking.split('/').next().map(str::to_string);
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
        has_remote: true,
        has_upstream: true,
        branch,
        remote_branch: Some(tracking),
        remote_name,
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
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let remote_name = validate_remote_name(body.name.as_deref().unwrap_or("origin"))?;
    let remote_url = validate_remote_url(&body.url)?;
    let remotes = git(&project_path, ["remote"]).await?.stdout;
    let remote_exists = remotes.lines().any(|line| line.trim() == remote_name);
    let output = if remote_exists {
        git(
            &project_path,
            ["remote", "set-url", &remote_name, &remote_url],
        )
        .await?
    } else {
        git(&project_path, ["remote", "add", &remote_name, &remote_url]).await?
    };

    let mut response = GitOperationResponse::success(join_output(&output));
    response.remote_name = Some(remote_name);
    response.remote_url = Some(remote_url);
    Ok(Json(response))
}

async fn fetch(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let remote_name = upstream_remote_or_origin(&project_path).await?;
    validate_remote_name(&remote_name)?;
    let output = git(&project_path, ["fetch", &remote_name]).await?;
    let mut response =
        GitOperationResponse::success(join_output_or(&output, "Fetch completed successfully"));
    response.remote_name = Some(remote_name);
    Ok(Json(response))
}

async fn pull(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let branch = current_branch(&project_path).await?;
    let (remote_name, remote_branch) = upstream_remote_branch_or(&project_path, &branch).await?;
    validate_remote_name(&remote_name)?;
    validate_branch_name(&remote_branch)?;
    let output = git(&project_path, ["pull", &remote_name, &remote_branch]).await?;
    let mut response =
        GitOperationResponse::success(join_output_or(&output, "Pull completed successfully"));
    response.remote_name = Some(remote_name);
    response.remote_branch = Some(remote_branch);
    Ok(Json(response))
}

async fn push(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let branch = current_branch(&project_path).await?;
    let (remote_name, remote_branch) = upstream_remote_branch_or(&project_path, &branch).await?;
    validate_remote_name(&remote_name)?;
    validate_branch_name(&remote_branch)?;
    let output = git(&project_path, ["push", &remote_name, &remote_branch]).await?;
    let mut response =
        GitOperationResponse::success(join_output_or(&output, "Push completed successfully"));
    response.remote_name = Some(remote_name);
    response.remote_branch = Some(remote_branch);
    Ok(Json(response))
}

async fn publish(
    State(state): State<AppState>,
    Json(body): Json<BranchBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let branch = validate_branch_name(&body.branch)?;
    let current = current_branch(&project_path).await?;
    if current != branch {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!("Branch mismatch. Current branch is {current}, but trying to publish {branch}"),
        ));
    }
    let remote_name = first_remote(&project_path).await?.ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            "No remote repository configured. Add a remote with: git remote add origin <url>",
        )
    })?;
    validate_remote_name(&remote_name)?;
    let output = git(
        &project_path,
        ["push", "--set-upstream", &remote_name, &branch],
    )
    .await?;
    let mut response =
        GitOperationResponse::success(join_output_or(&output, "Branch published successfully"));
    response.remote_name = Some(remote_name);
    response.branch = Some(branch);
    Ok(Json(response))
}

async fn stage(
    State(state): State<AppState>,
    Json(body): Json<FileBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_repository_file_path(&project_path, &body.file).await?;
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
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_repository_file_path(&project_path, &body.file).await?;

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

    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_repository_file_path(&project_path, &body.file).await?;
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
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_repository_file_path(&project_path, &body.file).await?;
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

async fn discard(
    State(state): State<AppState>,
    Json(body): Json<FileBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_repository_file_path(&project_path, &body.file).await?;
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

async fn delete_untracked(
    State(state): State<AppState>,
    Json(body): Json<FileBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_repository_file_path(&project_path, &body.file).await?;
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

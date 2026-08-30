#[derive(Debug)]
struct ResolvedRepositoryFile {
    repository_root: PathBuf,
    repository_relative_file: String,
}

async fn resolve_project_path(state: &AppState, project_ref: &str) -> Result<PathBuf> {
    if project_ref.contains('\0') {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid project path",
        ));
    }

    if let Ok(project) = state.projects.find_by_ref(project_ref) {
        return canonicalize_dir(PathBuf::from(project.path)).await;
    }

    let looks_like_path = project_ref.starts_with('/')
        || project_ref.starts_with('~')
        || project_ref.starts_with('.')
        || project_ref.contains('\\')
        || project_ref.contains('/');
    if !looks_like_path {
        return Err(ServerError::new(
            StatusCode::NOT_FOUND,
            format!("Project not found: {project_ref}"),
        ));
    }

    let path = state
        .path_validator
        .validate_path(PathBuf::from(project_ref), false)
        .await
        .map_err(iowb_fs::FsError::from)?;
    canonicalize_dir(path).await
}

async fn canonicalize_dir(path: PathBuf) -> Result<PathBuf> {
    let metadata = fs::metadata(&path).await.map_err(io_server_error)?;
    if !metadata.is_dir() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Project path must be a directory",
        ));
    }
    fs::canonicalize(path).await.map_err(io_server_error)
}

async fn validate_git_repository(project_path: &Path) -> Result<()> {
    let inside = git(project_path, ["rev-parse", "--is-inside-work-tree"]).await;
    match inside {
        Ok(output) if output.stdout.trim() == "true" => {
            repository_root(project_path).await?;
            Ok(())
        }
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Not a git repository. Initialize a git repository with \"git init\" to use source control features.",
        )),
    }
}

async fn repository_root(project_path: &Path) -> Result<PathBuf> {
    let output = git(project_path, ["rev-parse", "--show-toplevel"]).await?;
    let root = output.stdout.trim();
    if root.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Could not resolve git repository root",
        ));
    }
    Ok(PathBuf::from(root))
}

async fn current_branch(project_path: &Path) -> Result<String> {
    match git(project_path, ["symbolic-ref", "--short", "HEAD"]).await {
        Ok(output) if !output.stdout.trim().is_empty() => Ok(output.stdout.trim().to_string()),
        _ => Ok(git(project_path, ["rev-parse", "--abbrev-ref", "HEAD"])
            .await?
            .stdout
            .trim()
            .to_string()),
    }
}

async fn repository_has_commits(project_path: &Path) -> Result<bool> {
    match git(project_path, ["rev-parse", "--verify", "HEAD"]).await {
        Ok(_) => Ok(true),
        Err(error)
            if error
                .body
                .details
                .as_deref()
                .is_some_and(is_missing_head_revision) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

async fn resolve_repository_file_path(
    project_path: &Path,
    file_path: &str,
) -> Result<ResolvedRepositoryFile> {
    validate_file_path(file_path)?;
    let repository_root = repository_root(project_path).await?;
    let candidates = file_path_candidates(project_path, &repository_root, file_path);

    for candidate in &candidates {
        let status = git(&repository_root, ["status", "--porcelain", "--", candidate])
            .await?
            .stdout;
        if !status.trim().is_empty() {
            return Ok(ResolvedRepositoryFile {
                repository_root,
                repository_relative_file: candidate.clone(),
            });
        }

        if safe_repo_child(&repository_root, candidate).is_ok_and(|path| path.exists()) {
            return Ok(ResolvedRepositoryFile {
                repository_root,
                repository_relative_file: candidate.clone(),
            });
        }
    }

    let fallback = candidates
        .into_iter()
        .next()
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Invalid file path"))?;
    Ok(ResolvedRepositoryFile {
        repository_root,
        repository_relative_file: fallback,
    })
}

async fn discard_repository_path(
    repository_root: &Path,
    repository_relative_file: &str,
) -> Result<bool> {
    let status_output = git(
        repository_root,
        [
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            repository_relative_file,
        ],
    )
    .await?
    .stdout;

    if status_output.trim().is_empty() {
        return Ok(false);
    }

    let entries = parse_status_entries(&status_output);
    let has_tracked = entries.iter().any(|(status, _)| status != "??");
    let has_commits = repository_has_commits(repository_root).await?;
    let absolute_target = safe_repo_child(repository_root, repository_relative_file)?;

    if has_commits && has_tracked {
        let _ = git(
            repository_root,
            ["reset", "HEAD", "--", repository_relative_file],
        )
        .await;
    } else if has_tracked {
        let _ = git(
            repository_root,
            [
                "rm",
                "--cached",
                "-r",
                "--ignore-unmatch",
                "--",
                repository_relative_file,
            ],
        )
        .await;
    }

    if absolute_target.exists() {
        if absolute_target.is_dir() {
            fs::remove_dir_all(&absolute_target)
                .await
                .map_err(io_server_error)?;
        } else {
            fs::remove_file(&absolute_target)
                .await
                .map_err(io_server_error)?;
        }
    }

    if has_commits && has_tracked {
        let _ = git(
            repository_root,
            [
                "restore",
                "--source=HEAD",
                "--worktree",
                "--",
                repository_relative_file,
            ],
        )
        .await;
    }

    Ok(true)
}

async fn upstream_remote_or_origin(project_path: &Path) -> Result<String> {
    let branch = current_branch(project_path).await?;
    Ok(upstream_remote_branch_or(project_path, &branch).await?.0)
}

async fn upstream_remote_branch_or(project_path: &Path, branch: &str) -> Result<(String, String)> {
    if let Ok(output) = git(
        project_path,
        [
            "rev-parse",
            "--abbrev-ref",
            &format!("{branch}@{{upstream}}"),
        ],
    )
    .await
    {
        let tracking = output.stdout.trim();
        if let Some((remote, branch)) = tracking.split_once('/') {
            return Ok((remote.to_string(), branch.to_string()));
        }
    }

    Ok(("origin".to_string(), branch.to_string()))
}

async fn first_remote(project_path: &Path) -> Result<Option<String>> {
    let remotes = git(project_path, ["remote"]).await?.stdout;
    let mut values = remotes
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if let Some(index) = values.iter().position(|remote| remote == "origin") {
        return Ok(Some(values.swap_remove(index)));
    }
    Ok(values.into_iter().next())
}

async fn git<I, S>(cwd: &Path, args: I) -> Result<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string())
        .collect::<Vec<_>>();
    let output = Command::new("git")
        .args(&args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "git failed",
                error.to_string(),
            )
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        return Ok(GitOutput { stdout, stderr });
    }

    Err(ServerError::with_details(
        StatusCode::BAD_REQUEST,
        "Git operation failed",
        format!(
            "Command failed: git {}\n{}{}",
            args.join(" "),
            stdout,
            stderr
        ),
    ))
}

fn parse_status_entries(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            if line.len() < 3 {
                return None;
            }
            let status = line[0..2].to_string();
            let raw_path = line[3..].to_string();
            let path = raw_path
                .split(" -> ")
                .last()
                .map(normalize_repo_relative_path)
                .unwrap_or_default();
            (!path.is_empty()).then_some((status, path))
        })
        .collect()
}

fn strip_diff_headers(diff: &str) -> String {
    let mut filtered = Vec::new();
    let mut include = false;
    for line in diff.lines() {
        if line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("new file mode")
            || line.starts_with("deleted file mode")
            || line.starts_with("---")
            || line.starts_with("+++")
        {
            continue;
        }
        if line.starts_with("@@") || include {
            include = true;
            filtered.push(line);
        }
    }
    filtered.join("\n")
}

fn selected_hunk_patch(diff: &str, selected_indexes: &[usize]) -> Result<String> {
    let selected = selected_indexes.iter().copied().collect::<HashSet<_>>();
    let mut header = String::new();
    let mut hunks = Vec::new();
    let mut current_hunk = String::new();
    let mut in_hunk = false;

    for line in diff.lines() {
        if line.starts_with("@@") {
            if in_hunk {
                hunks.push(current_hunk);
                current_hunk = String::new();
            }
            in_hunk = true;
        }

        if in_hunk {
            current_hunk.push_str(line);
            current_hunk.push('\n');
        } else {
            header.push_str(line);
            header.push('\n');
        }
    }

    if in_hunk {
        hunks.push(current_hunk);
    }

    if hunks.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No selectable hunks found in diff",
        ));
    }

    for index in &selected {
        if *index >= hunks.len() {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Selected hunk index is out of range",
            ));
        }
    }

    let mut patch = header;
    for (index, hunk) in hunks.into_iter().enumerate() {
        if selected.contains(&index) {
            patch.push_str(&hunk);
        }
    }

    if patch.trim().is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Selected hunks produced an empty patch",
        ));
    }

    Ok(patch)
}

async fn apply_patch_to_index(repository_root: &Path, patch: &str, reverse: bool) -> Result<()> {
    let mut command = Command::new("git");
    command
        .arg("apply")
        .arg("--cached")
        .arg("--recount")
        .current_dir(repository_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if reverse {
        command.arg("--reverse");
    }

    let mut child = command.spawn().map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "git apply failed",
            error.to_string(),
        )
    })?;

    let Some(mut stdin) = child.stdin.take() else {
        return Err(ServerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to open git apply stdin",
        ));
    };
    stdin.write_all(patch.as_bytes()).await.map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to write patch",
            error.to_string(),
        )
    })?;
    drop(stdin);

    let output = child.wait_with_output().await.map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "git apply failed",
            error.to_string(),
        )
    })?;
    if output.status.success() {
        return Ok(());
    }

    Err(ServerError::with_details(
        StatusCode::BAD_REQUEST,
        "Git hunk operation failed",
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    ))
}

async fn repository_file_status(
    repository_root: &Path,
    repository_relative_file: &str,
) -> Result<Option<String>> {
    let output = git(
        repository_root,
        ["status", "--porcelain", "--", repository_relative_file],
    )
    .await?
    .stdout;
    Ok(parse_status_entries(&output)
        .into_iter()
        .find(|(_, path)| path == repository_relative_file)
        .map(|(status, _)| status))
}

async fn read_repository_file_lossy(
    repository_root: &Path,
    repository_relative_file: &str,
) -> Result<String> {
    let path = safe_repo_child(repository_root, repository_relative_file)?;
    match fs::read(&path).await {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(io_server_error(error)),
    }
}

async fn stage_resolved_path(repository_root: &Path, repository_relative_file: &str) -> Result<()> {
    let path = safe_repo_child(repository_root, repository_relative_file)?;
    if path.exists() {
        git(repository_root, ["add", "--", repository_relative_file]).await?;
    } else {
        git(
            repository_root,
            ["rm", "--ignore-unmatch", "--", repository_relative_file],
        )
        .await?;
    }
    Ok(())
}

fn is_conflict_status(status: &str) -> bool {
    matches!(status, "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU")
        || status.chars().take(2).any(|character| character == 'U')
}

fn extract_conflict_regions(content: &str) -> Vec<GitConflictRegion> {
    #[derive(Debug, Copy, Clone)]
    enum Section {
        Ours,
        Base,
        Theirs,
    }

    let mut regions = Vec::new();
    let mut start_line = 0;
    let mut section: Option<Section> = None;
    let mut ours = Vec::new();
    let mut base = Vec::new();
    let mut theirs = Vec::new();

    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        if line.starts_with("<<<<<<< ") {
            start_line = line_number;
            section = Some(Section::Ours);
            ours.clear();
            base.clear();
            theirs.clear();
            continue;
        }

        if section.is_some() && line.starts_with("||||||| ") {
            section = Some(Section::Base);
            continue;
        }

        if section.is_some() && line.starts_with("=======") {
            section = Some(Section::Theirs);
            continue;
        }

        if section.is_some() && line.starts_with(">>>>>>> ") {
            regions.push(GitConflictRegion {
                start_line,
                end_line: line_number,
                base: (!base.is_empty()).then(|| base.join("\n")),
                ours: ours.join("\n"),
                theirs: theirs.join("\n"),
            });
            section = None;
            continue;
        }

        match section {
            Some(Section::Ours) => ours.push(line.to_string()),
            Some(Section::Base) => base.push(line.to_string()),
            Some(Section::Theirs) => theirs.push(line.to_string()),
            None => {}
        }
    }

    regions
}

fn normalize_repo_relative_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim()
        .to_string()
}

fn file_path_candidates(
    project_path: &Path,
    repository_root: &Path,
    file_path: &str,
) -> Vec<String> {
    let normalized = normalize_repo_relative_path(file_path);
    let project_relative = pathdiff(project_path, repository_root)
        .map(|path| normalize_repo_relative_path(&path.to_string_lossy()))
        .unwrap_or_default();
    let mut candidates = vec![normalized.clone()];

    if !project_relative.is_empty()
        && project_relative != "."
        && !normalized.starts_with(&format!("{project_relative}/"))
    {
        candidates.push(format!("{project_relative}/{normalized}"));
    }

    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| !candidate.is_empty() && seen.insert(candidate.clone()))
        .collect()
}

fn safe_repo_child(repository_root: &Path, relative_path: &str) -> Result<PathBuf> {
    let normalized = normalize_repo_relative_path(relative_path);
    validate_file_path(&normalized)?;
    let candidate = repository_root.join(normalized);
    let normalized_candidate = normalize_path(&candidate);
    let normalized_root = normalize_path(repository_root);
    if normalized_candidate == normalized_root
        || !normalized_candidate.starts_with(&normalized_root)
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid file path: path traversal detected",
        ));
    }
    Ok(normalized_candidate)
}

fn validate_file_path(file: &str) -> Result<()> {
    if file.trim().is_empty() || file.contains('\0') || file.contains("..") {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid file path",
        ));
    }
    Ok(())
}

fn validate_commit_ref(commit: &str) -> Result<String> {
    validate_pattern(commit, "Invalid commit reference", |character| {
        character.is_ascii_alphanumeric()
            || matches!(
                character,
                '.' | '_' | '~' | '^' | '{' | '}' | '@' | '/' | '-'
            )
    })
}

fn validate_branch_name(branch: &str) -> Result<String> {
    validate_pattern(branch, "Invalid branch name", |character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '/' | '-')
    })
}

fn validate_tag_name(tag: &str) -> Result<String> {
    validate_pattern(tag, "Invalid tag name", |character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '/' | '-')
    })
}

fn validate_stash_ref(reference: &str) -> Result<String> {
    let trimmed = reference.trim();
    let valid = trimmed
        .strip_prefix("stash@{")
        .and_then(|value| value.strip_suffix('}'))
        .is_some_and(|index| {
            !index.is_empty() && index.chars().all(|character| character.is_ascii_digit())
        });
    if !valid {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid stash reference",
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_remote_name(remote: &str) -> Result<String> {
    validate_pattern(remote, "Invalid remote name", |character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
    })
}

fn validate_remote_url(url: &str) -> Result<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid remote URL",
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_optional_git_message(message: Option<&str>) -> Result<Option<String>> {
    let Some(message) = message else {
        return Ok(None);
    };
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.contains('\0') || trimmed.len() > 500 {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid git message",
        ));
    }
    Ok(Some(trimmed.to_string()))
}

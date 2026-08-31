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
            let root = repository_root(project_path).await?;
            if normalize_path(&root) != normalize_path(project_path) {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Project path is inside another Git repository. Select the repository root or initialize this workspace explicitly.",
                ));
            }
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
    fs::canonicalize(root).await.map_err(io_server_error)
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

async fn resolve_commit_reference(repository_path: &Path, value: &str) -> Result<String> {
    let reference = validate_commit_ref(value)?;
    let output = git(
        repository_path,
        [
            "rev-parse",
            "--verify",
            "--end-of-options",
            reference.as_str(),
        ],
    )
    .await?;
    let hash = output
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "Commit reference did not resolve to an object",
            )
        })?;
    let object_type = git(repository_path, ["cat-file", "-t", hash])
        .await?
        .stdout;
    if object_type.trim() != "commit" {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Commit reference must resolve to a commit",
        ));
    }
    Ok(hash.to_string())
}

async fn resolve_repository_file_path(
    project_path: &Path,
    file_path: &str,
) -> Result<ResolvedRepositoryFile> {
    validate_file_path(file_path)?;
    let repository_root = repository_root(project_path).await?;
    let candidates = file_path_candidates(project_path, &repository_root, file_path);

    for candidate in &candidates {
        if repository_file_status(&repository_root, candidate)
            .await?
            .is_some()
        {
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
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
            "--",
            repository_relative_file,
        ],
    )
    .await?
    ;

    let entries = parse_status_entries_detailed_bytes(&status_output.stdout_bytes);
    if entries.is_empty() {
        return Ok(false);
    }

    let has_tracked = entries.iter().any(|(status, _, _)| status != "??");
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
            "--symbolic-full-name",
            "@{upstream}",
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
    run_git(cwd, args, true).await
}

// The initial-commit path uses Git's exclude pathspec magic to keep
// independent nested repositories out of the parent tree.  All request data
// still goes through `git`, which enables literal pathspecs; this helper is
// reserved for the server-owned exclusion pathspecs.
async fn git_with_pathspec_magic<I, S>(cwd: &Path, args: I) -> Result<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    run_git(cwd, args, false).await
}

async fn run_git<I, S>(cwd: &Path, args: I, literal_pathspecs: bool) -> Result<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string())
        .collect::<Vec<_>>();
    let mut command = Command::new("git");
    command
        .args(&args)
        .current_dir(cwd)
        // Server requests must fail promptly when a remote needs credentials;
        // there is no interactive terminal available to answer a prompt.
        .env("GIT_TERMINAL_PROMPT", "0");
    if literal_pathspecs {
        command.env("GIT_LITERAL_PATHSPECS", "1");
    } else {
        command.env_remove("GIT_LITERAL_PATHSPECS");
    }
    let output = command.output().await.map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "git failed",
            error.to_string(),
        )
    })?;

    let stdout_bytes = output.stdout;
    let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        return Ok(GitOutput {
            stdout,
            stdout_bytes,
            stderr,
        });
    }

    Err(ServerError::with_details(
        StatusCode::BAD_REQUEST,
        "Git operation failed",
        redact_git_diagnostic(&format!(
            "Command failed: git {}\n{}{}",
            args.join(" "),
            stdout,
            stderr
        )),
    ))
}

fn redact_git_diagnostic(value: &str) -> String {
    let mut redacted = String::with_capacity(value.len());
    let mut cursor = 0;

    while let Some(scheme_offset) = value[cursor..].find("://") {
        let authority_start = cursor + scheme_offset + 3;
        let authority_tail = &value[authority_start..];
        let authority_length = authority_tail
            .find(|character: char| {
                character.is_whitespace()
                    || matches!(character, '/' | '?' | '#' | '\'' | '"' | ')' | ']' | '}' | ',')
            })
            .unwrap_or(authority_tail.len());
        let authority = &authority_tail[..authority_length];

        let Some(user_info_end) = authority.rfind('@') else {
            let next = authority_start + authority_length;
            redacted.push_str(&value[cursor..next]);
            cursor = next;
            continue;
        };

        redacted.push_str(&value[cursor..authority_start]);
        redacted.push_str("<redacted>@");
        cursor = authority_start + user_info_end + 1;
    }

    redacted.push_str(&value[cursor..]);
    redacted
}

#[cfg(test)]
fn parse_status_entries(output: &str) -> Vec<(String, String)> {
    parse_status_entries_detailed_bytes(output.as_bytes())
        .into_iter()
        .map(|(status, path, _)| (status, path))
        .collect()
}

fn parse_status_entries_detailed_bytes(output: &[u8]) -> Vec<(String, String, Option<String>)> {
    if !output.contains(&0) {
        return String::from_utf8_lossy(output)
            .lines()
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
                (!path.is_empty()).then_some((status, path, None))
            })
            .collect();
    }

    let records = output.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() || record.starts_with(b"!") {
            continue;
        }
        let Some(kind) = record.split(|byte| *byte == b' ').next() else {
            continue;
        };
        let parsed = match kind {
            b"1" => parse_v2_detailed_record_bytes(record, 9),
            b"2" => {
                let parsed = parse_v2_detailed_record_bytes(record, 10);
                // Rename/copy records carry the original path as a second NUL
                // record. The first path is the worktree-visible destination.
                index += 1;
                parsed
            }
            b"u" => parse_v2_detailed_record_bytes(record, 11),
            b"?" => Some((
                "??".to_string(),
                String::from_utf8_lossy(record.get(2..).unwrap_or_default()).to_string(),
                None,
            )),
            _ => None,
        };
        let Some((status, path, submodule_state)) = parsed else {
            continue;
        };
        let path = normalize_repo_relative_path(&path);
        if !path.is_empty() {
            entries.push((status, path, submodule_state));
        }
    }
    entries
}

fn parse_v2_detailed_record_bytes(
    record: &[u8],
    field_count: usize,
) -> Option<(String, String, Option<String>)> {
    let fields = record
        .splitn(field_count, |byte| *byte == b' ')
        .collect::<Vec<_>>();
    if fields.len() != field_count {
        return None;
    }
    let submodule_state = (!matches!(fields[2], b"N..." | b""))
        .then(|| String::from_utf8_lossy(fields[2]).to_string());
    Some((
        String::from_utf8_lossy(fields[1]).to_string(),
        String::from_utf8_lossy(fields[field_count - 1]).to_string(),
        submodule_state,
    ))
}

#[derive(Debug, Copy, Clone)]
enum GitFileTargetPolicy {
    Inspect,
    Stage,
    Unstage,
    Commit,
    Discard,
    Hunks,
}

// A repository boundary is a navigation boundary.  The parent repository may
// expose a submodule gitlink, but it must never treat files belonging to an
// independent child repository as its own files.
async fn resolve_git_file_target(
    project_path: &Path,
    file_path: &str,
    policy: GitFileTargetPolicy,
) -> Result<ResolvedRepositoryFile> {
    let resolved = resolve_repository_file_path(project_path, file_path).await?;
    let target = safe_repo_child(
        &resolved.repository_root,
        &resolved.repository_relative_file,
    )?;
    let canonical_target = fs::canonicalize(&target).await.unwrap_or(target.clone());
    let repository_root = normalize_path(&resolved.repository_root);
    let catalog = discover_git_workspace(&repository_root).await?;
    let child = catalog.repositories.into_iter().find(|repository| {
        if repository.path == repository_root {
            return false;
        }
        // Treat repository boundaries as overlapping paths in either
        // direction.  Checking only "target is inside child" would let a
        // request for a parent directory traverse into a nested repository
        // and accidentally stage, discard, or delete its contents.
        is_within(&canonical_target, &repository.path)
            || is_within(&target, &repository.path)
            || is_within(&repository.path, &canonical_target)
            || is_within(&repository.path, &target)
    });

    if let Some(child) = child {
        let exact_boundary = target == child.path || canonical_target == child.path;
        let allow_submodule_pointer = exact_boundary
            && match child.kind {
                iowb_protocol::GitRepositoryKind::Submodule => matches!(
                    policy,
                    GitFileTargetPolicy::Inspect
                        | GitFileTargetPolicy::Stage
                        | GitFileTargetPolicy::Unstage
                        | GitFileTargetPolicy::Commit
                ),
                // An uninitialized submodule can be inspected as a parent
                // gitlink, but it is not a worktree and therefore cannot be
                // staged, committed, discarded, or hunk-edited yet.
                iowb_protocol::GitRepositoryKind::Uninitialized =>
                    matches!(policy, GitFileTargetPolicy::Inspect),
                _ => false,
            };
        if !allow_submodule_pointer {
            let action = match policy {
                GitFileTargetPolicy::Inspect => "inspect",
                GitFileTargetPolicy::Stage => "stage",
                GitFileTargetPolicy::Unstage => "unstage",
                GitFileTargetPolicy::Commit => "commit",
                GitFileTargetPolicy::Discard => "discard",
                GitFileTargetPolicy::Hunks => "apply hunks to",
            };
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "Cannot {action} {file_path} from the parent repository. Select the {} repository at {} first.",
                    repository_kind_label(&child.kind), child.relative_path
                ),
            ));
        }
    }

    Ok(resolved)
}

async fn submodule_at_boundary(
    repository_root: &Path,
    repository_relative_file: &str,
) -> Result<Option<GitRepositoryRecord>> {
    let target = safe_repo_child(repository_root, repository_relative_file)?;
    let canonical_target = fs::canonicalize(&target).await.unwrap_or(target.clone());
    let repository_root = normalize_path(repository_root);
    let catalog = discover_git_workspace(&repository_root).await?;
    Ok(catalog.repositories.into_iter().find(|repository| {
        matches!(
            repository.kind,
            iowb_protocol::GitRepositoryKind::Submodule
                | iowb_protocol::GitRepositoryKind::Uninitialized
        )
            && (repository.path == canonical_target || repository.path == target)
    }))
}

async fn submodule_diff(
    repository_root: &Path,
    repository_relative_file: &str,
    staged: Option<bool>,
) -> Result<String> {
    if staged == Some(true) {
        return Ok(git(
            repository_root,
            [
                "diff",
                "--cached",
                "--submodule=log",
                "--",
                repository_relative_file,
            ],
        )
        .await?
        .stdout);
    }
    if staged == Some(false) {
        return Ok(git(
            repository_root,
            [
                "diff",
                "--submodule=log",
                "--",
                repository_relative_file,
            ],
        )
        .await?
        .stdout);
    }

    let unstaged = git(
        repository_root,
        [
            "diff",
            "--submodule=log",
            "--",
            repository_relative_file,
        ],
    )
    .await?
    .stdout;
    if !unstaged.trim().is_empty() {
        return Ok(unstaged);
    }
    Ok(git(
        repository_root,
        [
            "diff",
            "--cached",
            "--submodule=log",
            "--",
            repository_relative_file,
        ],
    )
    .await?
    .stdout)
}

fn repository_kind_label(kind: &iowb_protocol::GitRepositoryKind) -> &'static str {
    match kind {
        iowb_protocol::GitRepositoryKind::Root => "root",
        iowb_protocol::GitRepositoryKind::Submodule => "submodule",
        iowb_protocol::GitRepositoryKind::Nested => "nested",
        iowb_protocol::GitRepositoryKind::Worktree => "worktree",
        iowb_protocol::GitRepositoryKind::Uninitialized => "uninitialized submodule",
    }
}

fn strip_diff_headers(diff: &str) -> String {
    let mut filtered = Vec::new();
    let mut include = false;
    for line in diff.lines() {
        // Only discard the file-level preamble.  Once a hunk starts, lines
        // beginning with `---` or `+++` are valid user content and must stay
        // in the preview.
        if !include && line.starts_with("@@") {
            include = true;
        }
        if include {
            filtered.push(line);
        }
    }
    filtered.join("\n")
}

async fn enclosing_git_repository(project_path: &Path) -> Option<PathBuf> {
    let output = git(project_path, ["rev-parse", "--show-toplevel"])
        .await
        .ok()?;
    let root = output.stdout.trim();
    if root.is_empty() {
        return None;
    }
    let root = std::fs::canonicalize(root).ok()?;
    let project_path = normalize_path(project_path);
    let root = normalize_path(&root);
    (root != project_path && is_within(&project_path, &root)).then_some(root)
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
        redact_git_diagnostic(&format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )),
    ))
}

async fn repository_file_status(
    repository_root: &Path,
    repository_relative_file: &str,
) -> Result<Option<String>> {
    Ok(repository_file_status_details(repository_root, repository_relative_file)
        .await?
        .map(|(status, _)| status))
}

async fn repository_file_status_details(
    repository_root: &Path,
    repository_relative_file: &str,
) -> Result<Option<(String, Option<String>)>> {
    let output = git(
        repository_root,
        [
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
            "--",
            repository_relative_file,
        ],
    )
    .await?;
    Ok(parse_status_entries_detailed_bytes(&output.stdout_bytes)
        .into_iter()
        .find(|(_, path, _)| path == repository_relative_file)
        .map(|(status, _, submodule_state)| (status, submodule_state)))
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

fn status_contains(status: &str, expected: char) -> bool {
    status.chars().take(2).any(|character| character == expected)
}

fn status_is_untracked(status: &str) -> bool {
    status == "??"
}

fn status_is_deleted(status: &str) -> bool {
    status_contains(status, 'D')
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
    // Validate before normalizing.  Normalization intentionally removes a
    // leading `./`, but must never turn an absolute or backslash-rooted input
    // into an apparently relative path.
    validate_file_path(relative_path)?;
    let normalized = normalize_repo_relative_path(relative_path);
    validate_file_path(&normalized)?;
    let normalized_root = normalize_path(repository_root);
    let normalized_candidate = normalize_path(&normalized_root.join(normalized));
    if normalized_candidate == normalized_root || !is_within(&normalized_candidate, &normalized_root) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid file path: path traversal detected",
        ));
    }

    // Lexical checks do not protect against `link/file` when `link` points
    // outside the repository.  Canonicalize the deepest existing component so
    // both existing symlinks and symlinked parent directories are checked.
    let canonical_root = std::fs::canonicalize(&normalized_root).map_err(io_server_error)?;
    let mut existing = normalized_candidate.clone();
    loop {
        match std::fs::symlink_metadata(&existing) {
            Ok(_) => {
                let canonical_existing = std::fs::canonicalize(&existing).map_err(io_server_error)?;
                if !is_within(&canonical_existing, &canonical_root) {
                    return Err(ServerError::new(
                        StatusCode::BAD_REQUEST,
                        "Invalid file path: symlink escapes repository scope",
                    ));
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !existing.pop() {
                    return Err(ServerError::new(
                        StatusCode::BAD_REQUEST,
                        "Invalid file path",
                    ));
                }
            }
            Err(error) => return Err(io_server_error(error)),
        }
    }
    Ok(normalized_candidate)
}

fn validate_file_path(file: &str) -> Result<()> {
    let normalized = file.replace('\\', "/");
    if file.trim().is_empty()
        || file.contains('\0')
        || Path::new(file).is_absolute()
        || file.starts_with('\\')
        || normalized.split('/').any(|component| component == "..")
    {
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
    validate_git_ref_name(branch, "Invalid branch name")
}

fn validate_tag_name(tag: &str) -> Result<String> {
    validate_git_ref_name(tag, "Invalid tag name")
}

fn validate_git_ref_name(value: &str, message: &'static str) -> Result<String> {
    let trimmed = value.trim();
    let components = trimmed.split('/').collect::<Vec<_>>();
    let invalid = trimmed.is_empty()
        || trimmed.starts_with('-')
        || trimmed.len() > 1024
        || trimmed == "."
        || trimmed == ".."
        || trimmed.starts_with('/')
        || trimmed.ends_with('/')
        || trimmed.contains("//")
        || trimmed.contains("..")
        || trimmed.contains("@{")
        || trimmed.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
        || components.iter().any(|component| {
            component.is_empty()
                || component.starts_with('.')
                || component.ends_with('.')
                || component.ends_with(".lock")
        });
    if invalid {
        return Err(ServerError::new(StatusCode::BAD_REQUEST, message));
    }
    Ok(trimmed.to_string())
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
    let remote = validate_pattern(remote, "Invalid remote name", |character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
    })?;
    if remote == "."
        || remote == ".."
        || remote.starts_with('.')
        || remote.ends_with('.')
        || remote.contains("..")
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid remote name",
        ));
    }
    Ok(remote)
}

fn validate_remote_url(url: &str) -> Result<String> {
    let trimmed = url.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('-')
        || trimmed.len() > 4096
        || trimmed.chars().any(|character| character.is_control())
    {
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

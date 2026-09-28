// Advanced, deliberately opt-in Git operations.
//
// This file keeps commands which can rewrite history, alter checkout shape,
// or expose integration state separate from the everyday status/commit
// endpoints. Every mutation validates the selected repository, avoids Git
// editors and credential prompts, and requires an explicit confirmation when
// it can change history, checkout state, or object retention.

const ADVANCED_GIT_MAX_OUTPUT_CHARACTERS: usize = 500_000;
const ADVANCED_GIT_MAX_REBASE_COMMITS: usize = 100;
const ADVANCED_GIT_MAX_PATTERNS: usize = 100;
const ADVANCED_GIT_MAX_PATTERN_LENGTH: usize = 1_024;
const ADVANCED_GIT_MAX_MESSAGE_LENGTH: usize = 10_000;
const ADVANCED_GIT_HISTORY_DEFAULT_LIMIT: usize = 50;
const ADVANCED_GIT_HISTORY_MAX_LIMIT: usize = 100;
const ADVANCED_GIT_HISTORY_MAX_OFFSET: usize = 10_000;
const ADVANCED_GIT_HISTORY_MAX_SEARCH_LENGTH: usize = 1_024;

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedGitProjectScope {
    project: Option<String>,
    project_path: Option<String>,
    repository_id: Option<String>,
}

impl AdvancedGitProjectScope {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

async fn advanced_git_repository(
    state: &AppState,
    scope: &AdvancedGitProjectScope,
) -> Result<PathBuf> {
    let repository =
        resolve_git_repository_path(state, scope.project_ref()?, scope.repository_id.as_deref())
            .await?;
    validate_git_repository(&repository).await?;
    Ok(repository)
}

fn advanced_require_confirmation(confirmed: bool, action: &str) -> Result<()> {
    if confirmed {
        return Ok(());
    }
    Err(ServerError::new(
        StatusCode::BAD_REQUEST,
        format!("Confirmation is required to {action}"),
    ))
}

fn advanced_truncate_output(value: String) -> (String, bool) {
    let is_truncated = value.chars().count() > ADVANCED_GIT_MAX_OUTPUT_CHARACTERS;
    if !is_truncated {
        return (value, false);
    }
    (
        format!(
            "{}\n\n... Output truncated to keep the UI responsive ...",
            value
                .chars()
                .take(ADVANCED_GIT_MAX_OUTPUT_CHARACTERS)
                .collect::<String>()
        ),
        true,
    )
}

async fn advanced_operation_response(
    repository: &Path,
    output: &GitOutput,
    fallback: &str,
) -> Result<GitOperationResponse> {
    let mut response = GitOperationResponse::success(join_output_or(output, fallback));
    response.message = Some(fallback.to_string());
    response.operation_state = Some(git_operation_state(repository).await?);
    Ok(response)
}

async fn advanced_current_head(repository: &Path) -> Result<String> {
    resolve_commit_reference(repository, "HEAD").await
}

fn advanced_validate_expected_head(value: &str) -> Result<&str> {
    let value = value.trim();
    // A symbolic ref such as `HEAD` would resolve to whatever happens to be
    // current at execution time and defeat the optimistic-concurrency check.
    // Git currently supports SHA-1 and SHA-256 repositories, hence both
    // immutable object-ID lengths are accepted.
    if !matches!(value.len(), 40 | 64)
        || !value.chars().all(|character| character.is_ascii_hexdigit())
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "expectedHead must be the full immutable commit hash returned by Git status or a plan preview",
        ));
    }
    Ok(value)
}

async fn advanced_require_expected_head(repository: &Path, expected_head: &str) -> Result<String> {
    let current = advanced_current_head(repository).await?;
    let expected =
        resolve_commit_reference(repository, advanced_validate_expected_head(expected_head)?)
            .await?;
    if current != expected {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The current HEAD changed since this operation was prepared. Refresh Git status and confirm again.",
        ));
    }
    Ok(current)
}

async fn advanced_require_clean_worktree(repository: &Path, action: &str) -> Result<()> {
    let output = git(
        repository,
        [
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )
    .await?;
    if output.stdout_bytes.is_empty() {
        return Ok(());
    }
    Err(ServerError::new(
        StatusCode::CONFLICT,
        format!(
            "The working tree must be clean to {action}. Commit, stash, or discard all changes first."
        ),
    ))
}

/// Git and LazyGit both treat ordinary ignored build output as outside the
/// working-tree dirty state.  Do the same here, then protect only ignored
/// paths an operation can actually write.  A global `--ignored=matching`
/// clean check made everyday repositories with `build/`, `.gradle/`, or
/// generated output unable to cherry-pick, rebase, or apply a stash.
fn advanced_ignored_status_paths(output: &[u8]) -> Vec<String> {
    output
        .split(|byte| *byte == 0)
        .filter_map(|record| {
            let path = record.strip_prefix(b"! ")?;
            let path = normalize_repo_relative_path(&String::from_utf8_lossy(path));
            (!path.is_empty()).then_some(path)
        })
        .collect()
}

async fn advanced_ignored_worktree_paths(repository: &Path) -> Result<Vec<String>> {
    let output = git(
        repository,
        [
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignored=matching",
            "--ignore-submodules=none",
        ],
    )
    .await?;
    Ok(advanced_ignored_status_paths(&output.stdout_bytes))
}

fn advanced_paths_overlap(left: &str, right: &str) -> bool {
    let left = left.trim_end_matches('/');
    let right = right.trim_end_matches('/');
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

/// Refuse only a concrete overwrite risk, not every ignored file in the
/// repository.  Both a file and an ancestor/descendant path are considered a
/// collision because replacing a file with a directory (or the reverse) can
/// discard the ignored entry too.
async fn advanced_require_no_ignored_path_collision(
    repository: &Path,
    write_paths: &[String],
    action: &str,
) -> Result<()> {
    if write_paths.is_empty() {
        return Ok(());
    }
    let ignored_paths = advanced_ignored_worktree_paths(repository).await?;
    let collision = ignored_paths.iter().find_map(|ignored| {
        write_paths
            .iter()
            .find(|path| advanced_paths_overlap(ignored, path))
            .map(|path| (ignored, path))
    });
    if let Some((ignored, path)) = collision {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "Cannot {action} because it could overwrite ignored local path {ignored} while updating {path}. Move or remove the ignored path, then try again."
            ),
        ));
    }
    Ok(())
}

fn advanced_paths_from_nul_output(output: &[u8]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for raw_path in output.split(|byte| *byte == 0) {
        if raw_path.is_empty() {
            continue;
        }
        let path = normalize_repo_relative_path(&String::from_utf8_lossy(raw_path));
        if !path.is_empty() && seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    paths
}

/// Paths a non-merge commit can write while it is replayed.  The cherry-pick
/// endpoint handles merge commits separately with the user-selected mainline.
async fn advanced_commit_write_paths(
    repository: &Path,
    commit: &str,
    mainline: Option<usize>,
) -> Result<Vec<String>> {
    let mut args = vec![
        "diff-tree".to_string(),
        "--no-commit-id".to_string(),
        "--name-only".to_string(),
        "--no-renames".to_string(),
        "-r".to_string(),
        "-z".to_string(),
    ];
    if let Some(mainline) = mainline {
        args.push(format!("{commit}^{mainline}"));
        args.push(commit.to_string());
    } else {
        args.push("--root".to_string());
        args.push(commit.to_string());
    }
    let output = git(repository, args).await?;
    Ok(advanced_paths_from_nul_output(&output.stdout_bytes))
}

/// Paths Git can replace while moving the checked-out tree from `from` to
/// `to`. This deliberately includes deletions and file/directory changes so
/// an ignored child cannot be lost when the target turns its parent into a
/// tracked file (or vice versa).
async fn advanced_tree_transition_write_paths(
    repository: &Path,
    from: &str,
    to: &str,
) -> Result<Vec<String>> {
    let output = git(
        repository,
        [
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "--no-renames",
            "-r",
            "-z",
            from,
            to,
        ],
    )
    .await?;
    Ok(advanced_paths_from_nul_output(&output.stdout_bytes))
}

/// A rebase first replaces the checked-out tree with its upstream and then
/// replays every source-only commit. Looking only at the final tree diff is
/// not enough: an intermediate replay can overwrite an ignored local path
/// and a later replay can remove it again. Collect both phases before Git is
/// allowed to touch the worktree.
async fn advanced_rebase_write_paths(repository: &Path, upstream: &str) -> Result<Vec<String>> {
    let head = resolve_commit_reference(repository, "HEAD").await?;
    let upstream = resolve_commit_reference(repository, upstream).await?;
    let mut paths = advanced_tree_transition_write_paths(repository, &head, &upstream).await?;
    let commits = git(
        repository,
        ["rev-list", "--reverse", &format!("{upstream}..{head}")],
    )
    .await?
    .stdout;
    for commit in commits.lines().map(str::trim).filter(|commit| !commit.is_empty()) {
        paths.extend(advanced_commit_write_paths(repository, commit, None).await?);
    }
    Ok(paths)
}

async fn advanced_is_ancestor(repository: &Path, ancestor: &str, descendant: &str) -> bool {
    git(
        repository,
        ["merge-base", "--is-ancestor", ancestor, descendant],
    )
    .await
    .is_ok()
}

fn advanced_validate_message(value: &str, label: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > ADVANCED_GIT_MAX_MESSAGE_LENGTH
        || value.contains('\0')
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!("Invalid {label}"),
        ));
    }
    Ok(value.to_string())
}

fn advanced_validate_identity_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 256
        || value.contains('\0')
        || value.chars().any(char::is_control)
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid Git user name",
        ));
    }
    Ok(value.to_string())
}

fn advanced_validate_identity_email(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 320
        || !value.contains('@')
        || value.contains('\0')
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid Git user email",
        ));
    }
    Ok(value.to_string())
}

fn advanced_validate_signing_key_id(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 256
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '.' | '_' | '@' | '+' | '-' | '!')
        })
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid signing key ID",
        ));
    }
    Ok(value.to_string())
}

fn advanced_validate_pull_rebase(value: &str) -> Result<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "merges" | "m" => Ok("merges".to_string()),
        // LazyGit uses a no-op sequence editor for interactive Pull, which
        // accepts Git's generated todo unchanged and keeps the command from
        // waiting for an unavailable terminal editor. The native client
        // mirrors that behavior; its separate Fetch & plan action is the
        // explicitly reviewed alternative.
        "interactive" | "i" => Ok("interactive".to_string()),
        value => advanced_parse_git_bool(value)
            .map(|enabled| if enabled { "true" } else { "false" }.to_string())
            .ok_or_else(|| {
                ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "pullRebase must be true, false, merges, or interactive",
                )
            }),
    }
}

fn advanced_validate_gpg_format(value: &str) -> Result<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "openpgp" => Ok("openpgp".to_string()),
        "x509" => Ok("x509".to_string()),
        "ssh" => Ok("ssh".to_string()),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "gpgFormat must be openpgp, x509, or ssh",
        )),
    }
}

/// Git accepts only a small fixed set of SSH transport variants.  Unlike
/// `core.sshCommand`, this setting is data rather than a shell command, so it
/// is safe to expose as a repository-local preference.
fn advanced_validate_ssh_variant(value: &str) -> Result<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok("auto".to_string()),
        "simple" => Ok("simple".to_string()),
        "ssh" => Ok("ssh".to_string()),
        "plink" => Ok("plink".to_string()),
        "putty" => Ok("putty".to_string()),
        "tortoiseplink" => Ok("tortoiseplink".to_string()),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "sshVariant must be auto, simple, ssh, plink, putty, or tortoiseplink",
        )),
    }
}

/// A signing allow-list can be selected without accepting an arbitrary host
/// path.  Keeping it in the selected repository prevents the endpoint from
/// becoming a way to point Git's verifier at unrelated local files.  We only
/// return a boolean status later, never this path.
async fn advanced_validate_ssh_allowed_signers_path(
    repository: &Path,
    value: &str,
) -> Result<String> {
    validate_file_path(value)?;
    let path = normalize_repo_relative_path(value);
    if path == ".git" || path.starts_with(".git/") {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "SSH allowed signers file must be a regular file inside the repository",
        ));
    }
    let target = safe_repo_child(repository, &path)?;
    let metadata = fs::metadata(&target).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "SSH allowed signers file must already exist inside the repository",
            )
        } else {
            io_server_error(error)
        }
    })?;
    if !metadata.is_file() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "SSH allowed signers file must be a regular file inside the repository",
        ));
    }
    fs::canonicalize(target)
        .await
        .map(|path| path.to_string_lossy().to_string())
        .map_err(io_server_error)
}

fn advanced_parse_git_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

async fn advanced_local_config_value(repository: &Path, key: &str) -> Result<Option<String>> {
    match git(repository, ["config", "--local", "--get", key]).await {
        Ok(output) => Ok(output
            .stdout
            .lines()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .last()
            .map(str::to_string)),
        // Exit code 1 means the key is not present. Keeping an absent optional
        // preference as None is more useful than surfacing Git's generic
        // diagnostic, and all keys here are server-owned constants.
        Err(_) => Ok(None),
    }
}

async fn advanced_effective_config_value(repository: &Path, key: &str) -> Result<Option<String>> {
    match git(repository, ["config", "--get", key]).await {
        Ok(output) => Ok(output
            .stdout
            .lines()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .last()
            .map(str::to_string)),
        Err(_) => Ok(None),
    }
}

async fn advanced_effective_config_value_count(repository: &Path, key: &str) -> Result<usize> {
    match git(repository, ["config", "--get-all", key]).await {
        Ok(output) => Ok(output
            .stdout
            .lines()
            .filter(|value| !value.trim().is_empty())
            .count()),
        Err(_) => Ok(0),
    }
}

async fn advanced_local_config_value_count(repository: &Path, key: &str) -> Result<usize> {
    match git(repository, ["config", "--local", "--get-all", key]).await {
        Ok(output) => Ok(output
            .stdout
            .lines()
            .filter(|value| !value.trim().is_empty())
            .count()),
        Err(_) => Ok(0),
    }
}

async fn advanced_set_local_config(repository: &Path, key: &str, value: &str) -> Result<()> {
    git(repository, ["config", "--local", key, value])
        .await
        .map(|_| ())
}

async fn advanced_unset_local_config(repository: &Path, key: &str) -> Result<()> {
    if advanced_local_config_value(repository, key)
        .await?
        .is_some()
    {
        git(repository, ["config", "--local", "--unset-all", key]).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Checkout, branch deletion, and enhanced history

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedDetachedCheckoutBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    #[serde(rename = "ref", alias = "reference")]
    reference: String,
    expected_head: String,
    /// Pair the immutable commit with the checked-out branch identity. Two
    /// branches can share an OID, but a detached-checkout confirmation from
    /// one must never run after switching to the other.
    expected_target_ref: String,
    #[serde(default)]
    confirm: bool,
}

/// Explicitly detach HEAD at a resolved commit.  This is intentionally a
/// separate endpoint from branch checkout: mobile callers can make the mode
/// change unmistakable and require a user confirmation before it happens.
async fn advanced_detached_checkout(
    State(state): State<AppState>,
    Json(body): Json<AdvancedDetachedCheckoutBody>,
) -> Result<Json<GitOperationResponse>> {
    advanced_require_confirmation(body.confirm, "check out a detached commit")?;
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    advanced_require_clean_worktree(&repository, "check out a detached commit").await?;
    validate_workflow_target_snapshot(
        &repository,
        &body.expected_head,
        &body.expected_target_ref,
        "detached checkout",
    )
    .await?;
    let target = resolve_commit_reference(&repository, &body.reference).await?;
    // Like branch checkout, detached inspection must not discard an ignored
    // local path merely because the target commit starts tracking it.
    let output = git(
        &repository,
        ["checkout", "--no-overwrite-ignore", "--detach", target.as_str()],
    )
    .await?;
    Ok(Json(
        advanced_operation_response(&repository, &output, "Detached HEAD checked out successfully")
            .await?,
    ))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedForceDeleteLocalBranchBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    branch: String,
    #[serde(default)]
    confirm: bool,
}

/// Delete an unmerged local branch only after an explicit confirmation.
/// Git itself also guards against deleting a branch checked out by another
/// worktree; we preserve that protection by using `git branch -D` rather than
/// manipulating refs directly.
async fn advanced_force_delete_local_branch(
    State(state): State<AppState>,
    Json(body): Json<AdvancedForceDeleteLocalBranchBody>,
) -> Result<Json<GitOperationResponse>> {
    advanced_require_confirmation(body.confirm, "force-delete a local branch")?;
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let branch = validate_branch_name(&body.branch)?;
    management_local_branch_exists(&repository, &branch).await?;
    if current_branch(&repository).await? == branch {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Cannot delete the currently checked-out branch",
        ));
    }
    let output = git(&repository, ["branch", "-D", branch.as_str()]).await?;
    Ok(Json(
        advanced_operation_response(&repository, &output, "Local branch force-deleted successfully")
            .await?,
    ))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedEnhancedHistoryQuery {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    all_branches: Option<bool>,
    search: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}

fn advanced_history_search(value: Option<&str>) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > ADVANCED_GIT_HISTORY_MAX_SEARCH_LENGTH
        || value.contains('\0')
        || value.chars().any(char::is_control)
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "History search must be plain text without control characters and at most 1024 characters",
        ));
    }
    Ok(Some(value.to_string()))
}

fn advanced_history_offset(value: Option<usize>) -> Result<usize> {
    let offset = value.unwrap_or(0);
    if offset > ADVANCED_GIT_HISTORY_MAX_OFFSET {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "History offset cannot exceed {ADVANCED_GIT_HISTORY_MAX_OFFSET}. Refresh the history search before loading more commits."
            ),
        ));
    }
    Ok(offset)
}

fn advanced_history_limit(value: Option<usize>) -> usize {
    value
        .unwrap_or(ADVANCED_GIT_HISTORY_DEFAULT_LIMIT)
        .clamp(1, ADVANCED_GIT_HISTORY_MAX_LIMIT)
}

fn advanced_history_response(
    commits: Vec<GitHistoryCommit>,
    offset: usize,
    limit: usize,
    all_branches: bool,
    search: Option<String>,
) -> GitHistoryResponse {
    let has_more = commits.len() > limit;
    let commits = commits.into_iter().take(limit).collect::<Vec<_>>();
    GitHistoryResponse {
        next_offset: has_more.then_some(offset + commits.len()),
        commits,
        offset,
        limit,
        has_more,
        all_branches,
        search,
    }
}

fn advanced_parse_history_record(
    record: &str,
    refs_by_commit: &HashMap<String, Vec<String>>,
) -> Option<GitHistoryCommit> {
    // `--shortstat` emits its summary after the machine-readable first line.
    // Commit subjects are rendered with `%s`, so they cannot contain a line
    // break.  The unit separator keeps field parsing independent of ordinary
    // punctuation such as pipes in names or subjects.
    let record = record.trim_matches(['\r', '\n']);
    if record.is_empty() {
        return None;
    }
    let (metadata, stats) = record
        .split_once('\n')
        .map(|(metadata, stats)| (metadata, stats.trim()))
        .unwrap_or((record, ""));
    let mut fields = metadata.splitn(7, '\u{1f}');
    let hash = fields.next()?.trim();
    let short_hash = fields.next()?.trim();
    let author = fields.next()?;
    let email = fields.next()?;
    let date = fields.next()?.trim();
    let message = fields.next()?;
    let parents = fields
        .next()?
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if hash.is_empty() || short_hash.is_empty() || date.is_empty() {
        return None;
    }
    let stats = stats
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string();
    Some(GitHistoryCommit {
        hash: hash.to_string(),
        short_hash: short_hash.to_string(),
        author: author.to_string(),
        email: email.to_string(),
        date: date.to_string(),
        message: message.to_string(),
        stats,
        parents,
        refs: refs_by_commit.get(hash).cloned().unwrap_or_default(),
    })
}

async fn advanced_history_refs_by_commit(repository: &Path) -> Result<HashMap<String, Vec<String>>> {
    // Annotated tags point at tag objects, while graph rows are commits.  The
    // peeled object field makes those tag names appear on their target commit
    // alongside local and remote branches.
    let output = git(
        repository,
        [
            "for-each-ref",
            "--format=%(objectname)\u{1f}%(*objectname)\u{1f}%(refname:short)\u{1f}%(symref)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )
    .await?
    .stdout;
    let mut refs_by_commit = HashMap::<String, Vec<String>>::new();
    for line in output.lines().filter(|line| !line.is_empty()) {
        let mut fields = line.splitn(4, '\u{1f}');
        let object = fields.next().unwrap_or_default().trim();
        let peeled = fields.next().unwrap_or_default().trim();
        let label = fields.next().unwrap_or_default().trim();
        let symbolic_target = fields.next().unwrap_or_default().trim();
        if label.is_empty() || !symbolic_target.is_empty() {
            continue;
        }
        let target = if peeled.is_empty() { object } else { peeled };
        if target.is_empty() {
            continue;
        }
        let labels = refs_by_commit.entry(target.to_string()).or_default();
        if !labels.iter().any(|value| value == label) {
            labels.push(label.to_string());
        }
    }

    if repository_has_commits(repository).await? {
        let head = advanced_current_head(repository).await?;
        let label = match current_branch(repository).await?.as_str() {
            "HEAD" => "HEAD (detached)".to_string(),
            branch => format!("HEAD -> {branch}"),
        };
        let labels = refs_by_commit.entry(head).or_default();
        if !labels.iter().any(|value| value == &label) {
            labels.push(label);
        }
    }

    for labels in refs_by_commit.values_mut() {
        labels.sort();
    }
    Ok(refs_by_commit)
}

async fn advanced_enhanced_history(
    State(state): State<AppState>,
    Query(query): Query<AdvancedEnhancedHistoryQuery>,
) -> Result<Json<GitHistoryResponse>> {
    let repository = advanced_git_repository(&state, &query.scope).await?;
    let all_branches = query.all_branches.unwrap_or(false);
    let search = advanced_history_search(query.search.as_deref())?;
    let offset = advanced_history_offset(query.offset)?;
    let limit = advanced_history_limit(query.limit);
    if !repository_has_commits(&repository).await? {
        return Ok(Json(advanced_history_response(
            Vec::new(),
            offset,
            limit,
            all_branches,
            search,
        )));
    }

    let refs_by_commit = advanced_history_refs_by_commit(&repository).await?;
    let mut args = vec![
        "log".to_string(),
        "--no-color".to_string(),
        "--date=iso-strict".to_string(),
        "--shortstat".to_string(),
        "--format=%x1e%H%x1f%h%x1f%an%x1f%ae%x1f%aI%x1f%s%x1f%P".to_string(),
        format!("--skip={offset}"),
        // Unlike the long form, some Git versions do not accept `-n=<count>`.
        // Keep pagination portable across the server platforms we support.
        format!("--max-count={}", limit + 1),
    ];
    if all_branches {
        args.push("--all".to_string());
    }
    if let Some(search) = search.as_deref() {
        // `--fixed-strings` makes the query literal rather than a regular
        // expression.  The value remains one command argument, so it cannot
        // turn into a separate Git flag even if it begins with a dash.
        args.push("--fixed-strings".to_string());
        args.push(format!("--grep={search}"));
    }
    let output = git(&repository, args).await?.stdout;
    let commits = output
        .split('\u{1e}')
        .filter_map(|record| advanced_parse_history_record(record, &refs_by_commit))
        .collect::<Vec<_>>();
    Ok(Json(advanced_history_response(
        commits,
        offset,
        limit,
        all_branches,
        search,
    )))
}

// ---------------------------------------------------------------------------
// Amend and reword

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedAmendHeadBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    message: Option<String>,
    #[serde(default)]
    confirm: bool,
    expected_head: String,
    /// Pair the immutable commit ID with the checked-out target identity so a
    /// confirmation prepared on one same-OID branch cannot amend another.
    expected_target_ref: String,
}

async fn advanced_amend_head(
    State(state): State<AppState>,
    Json(body): Json<AdvancedAmendHeadBody>,
) -> Result<Json<GitOperationResponse>> {
    advanced_require_confirmation(body.confirm, "amend the current commit")?;
    let repository = advanced_git_repository(&state, &body.scope).await?;
    // An interactive-rebase `edit` stop is the one Git workflow where
    // `commit --amend` is the requested recovery action. Every other active
    // workflow still owns its index and remains locked.
    advanced_require_amendable_head(&repository).await?;
    validate_workflow_target_snapshot(
        &repository,
        &body.expected_head,
        &body.expected_target_ref,
        "amend",
    )
    .await?;
    let message = body
        .message
        .as_deref()
        .map(|value| advanced_validate_message(value, "commit message"))
        .transpose()?;

    // Honor the repository-local `commit.gpgSign` choice configured in the
    // native Git settings. `run_git_with_environment` already disables
    // terminal prompts, so an unavailable signing agent fails visibly instead
    // of turning this request into an interactive process.
    let mut args = vec!["commit".to_string(), "--amend".to_string()];
    if let Some(message) = message {
        args.push("-m".to_string());
        args.push(message);
    } else {
        args.push("--no-edit".to_string());
    }
    let output = git_with_noninteractive_editor(&repository, args).await?;
    Ok(Json(
        advanced_operation_response(&repository, &output, "Current commit amended successfully")
            .await?,
    ))
}

async fn advanced_require_amendable_head(repository: &Path) -> Result<()> {
    let Some(workflow) = detect_git_workflow(repository).await? else {
        return Ok(());
    };
    if workflow.kind.is_edit_stop() {
        return Ok(());
    }
    Err(ServerError::new(
        StatusCode::CONFLICT,
        format!(
            "A {} is in progress. Only an explicit interactive-rebase edit stop may amend the current commit; resolve or abort this operation first.",
            workflow.kind.wire_name(),
        ),
    ))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedRewordHeadBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    message: String,
    #[serde(default)]
    confirm: bool,
    expected_head: String,
    /// Pair the immutable commit ID with the checked-out target identity so a
    /// confirmation prepared on one same-OID branch cannot reword another.
    expected_target_ref: String,
}

async fn advanced_reword_head(
    State(state): State<AppState>,
    Json(body): Json<AdvancedRewordHeadBody>,
) -> Result<Json<GitOperationResponse>> {
    advanced_require_confirmation(body.confirm, "reword the current commit")?;
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    validate_workflow_target_snapshot(
        &repository,
        &body.expected_head,
        &body.expected_target_ref,
        "reword",
    )
    .await?;
    let message = advanced_validate_message(&body.message, "commit message")?;
    let output = git_with_noninteractive_editor(
        &repository,
        // Reword changes the commit message, not its contents.  Without
        // `--only`, Git folds every staged path into the amended commit.
        // Match LazyGit's reword invocation so a user can safely have a
        // separately staged next change while editing HEAD's message.
        [
            "commit",
            "--allow-empty",
            "--amend",
            "--only",
            "-m",
            message.as_str(),
        ],
    )
    .await?;
    Ok(Json(
        advanced_operation_response(&repository, &output, "Current commit reworded successfully")
            .await?,
    ))
}

/// Create the same `fixup!` commit that LazyGit offers from a selected
/// commit.  The selected commit is an immutable object ID rather than a
/// symbolic ref, so the confirmation cannot silently target a ref that moved
/// while the form was open.  The server deliberately does not stage anything:
/// the user reviews the index in Changes before creating the fixup.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedCreateFixupCommitBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    target: String,
    #[serde(default)]
    confirm: bool,
    expected_head: String,
    expected_target_ref: String,
}

async fn advanced_create_fixup_commit(
    State(state): State<AppState>,
    Json(body): Json<AdvancedCreateFixupCommitBody>,
) -> Result<Json<GitOperationResponse>> {
    advanced_require_confirmation(body.confirm, "create a fixup commit")?;
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    validate_workflow_target_snapshot(
        &repository,
        &body.expected_head,
        &body.expected_target_ref,
        "create a fixup commit",
    )
    .await?;
    let target = resolve_commit_reference(
        &repository,
        advanced_validate_expected_head(&body.target)?,
    )
    .await?;
    if git(&repository, ["diff", "--cached", "--name-only", "-z"])
        .await?
        .stdout_bytes
        .is_empty()
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Stage at least one change before creating a fixup commit",
        ));
    }
    let fixup_argument = format!("--fixup={target}");
    let output =
        git_with_noninteractive_editor(&repository, ["commit", fixup_argument.as_str()]).await?;
    Ok(Json(
        advanced_operation_response(&repository, &output, "Fixup commit created successfully")
            .await?,
    ))
}

// ---------------------------------------------------------------------------
// Structured interactive rebase

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedRebasePlanQuery {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    upstream: String,
}

#[derive(Debug, serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AdvancedRebasePlanCommit {
    commit: String,
    subject: String,
    action: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedRebasePlanPreviewResponse {
    upstream: String,
    expected_head: String,
    expected_target_ref: String,
    commits: Vec<AdvancedRebasePlanCommit>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedRebasePlanBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    upstream: String,
    expected_head: String,
    expected_target_ref: String,
    steps: Vec<AdvancedRebasePlanStepInput>,
    #[serde(default)]
    confirm: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedRebasePlanStepInput {
    commit: String,
    action: String,
    #[serde(alias = "newMessage")]
    message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdvancedRebaseAction {
    Pick,
    Edit,
    Reword,
    Squash,
    Fixup,
    Drop,
}

impl AdvancedRebaseAction {
    fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "pick" => Ok(Self::Pick),
            "edit" => Ok(Self::Edit),
            "reword" => Ok(Self::Reword),
            "squash" => Ok(Self::Squash),
            "fixup" => Ok(Self::Fixup),
            "drop" => Ok(Self::Drop),
            _ => Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Rebase action must be pick, edit, reword, squash, fixup, or drop",
            )),
        }
    }

    fn todo_name(self) -> &'static str {
        match self {
            Self::Pick | Self::Reword => "pick",
            Self::Edit => "edit",
            Self::Squash => "squash",
            Self::Fixup => "fixup",
            Self::Drop => "drop",
        }
    }

    fn is_applied(self) -> bool {
        !matches!(self, Self::Drop)
    }
}

#[derive(Debug)]
struct AdvancedValidatedRebaseStep {
    commit: String,
    action: AdvancedRebaseAction,
    reword_message: Option<String>,
}

async fn advanced_require_attached_head(repository: &Path) -> Result<()> {
    match git(repository, ["symbolic-ref", "--quiet", "--short", "HEAD"]).await {
        Ok(output) if !output.stdout.trim().is_empty() => Ok(()),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Structured rebase requires a checked-out local branch; detached HEAD is not supported.",
        )),
    }
}

async fn advanced_commit_parent_count(repository: &Path, commit: &str) -> Result<usize> {
    Ok(
        git(repository, ["rev-list", "--parents", "-n", "1", commit])
            .await?
            .stdout
            .split_whitespace()
            .count()
            .saturating_sub(1),
    )
}

async fn advanced_rebase_range(
    repository: &Path,
    upstream_input: &str,
) -> Result<(String, String, Vec<AdvancedRebasePlanCommit>)> {
    advanced_require_attached_head(repository).await?;
    let upstream = resolve_commit_reference(repository, upstream_input).await?;
    let head = advanced_current_head(repository).await?;
    let merge_base = git(repository, ["merge-base", upstream.as_str(), head.as_str()]).await;
    if merge_base
        .ok()
        .is_none_or(|output| output.stdout.trim().is_empty())
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "The selected upstream must share history with the current HEAD for a structured rebase.",
        ));
    }
    let range = format!("{upstream}..{head}");
    let output = git(
        repository,
        [
            "log",
            "--reverse",
            "--topo-order",
            "--format=%H%x1f%s",
            range.as_str(),
        ],
    )
    .await?;
    let commits = output
        .stdout
        .lines()
        .filter_map(|line| line.split_once('\u{1f}'))
        .map(|(commit, subject)| AdvancedRebasePlanCommit {
            commit: commit.to_string(),
            subject: subject.to_string(),
            action: "pick".to_string(),
        })
        .collect::<Vec<_>>();
    if commits.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "There are no commits to rebase above the selected upstream",
        ));
    }
    if commits.len() > ADVANCED_GIT_MAX_REBASE_COMMITS {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "Structured rebase supports at most {ADVANCED_GIT_MAX_REBASE_COMMITS} commits at once"
            ),
        ));
    }
    for commit in &commits {
        if advanced_commit_parent_count(repository, &commit.commit).await? > 1 {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "The editable rebase plan currently supports linear history only. Use the merge-preserving rebase action for a branch containing merge commits.",
            ));
        }
    }
    Ok((upstream, head, commits))
}

async fn advanced_rebase_plan_preview(
    State(state): State<AppState>,
    Query(query): Query<AdvancedRebasePlanQuery>,
) -> Result<Json<AdvancedRebasePlanPreviewResponse>> {
    let repository = advanced_git_repository(&state, &query.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let (upstream, expected_head, commits) =
        advanced_rebase_range(&repository, &query.upstream).await?;
    let expected_target_ref = checked_out_target_ref(&repository).await?;
    Ok(Json(AdvancedRebasePlanPreviewResponse {
        upstream,
        expected_head,
        expected_target_ref,
        commits,
    }))
}

async fn advanced_validate_rebase_plan(
    repository: &Path,
    body: &AdvancedRebasePlanBody,
) -> Result<(String, Vec<AdvancedValidatedRebaseStep>)> {
    advanced_require_confirmation(body.confirm, "rewrite history with the rebase plan")?;
    ensure_no_git_workflow_in_progress(repository).await?;
    advanced_require_clean_worktree(repository, "rewrite history with a rebase").await?;
    // A structured plan is a reviewed history rewrite, so its confirmation
    // must bind both the immutable HEAD and the attached branch identity.
    // The latter catches a checkout from `feature` to `release` when both
    // refs happen to point at the same commit.
    validate_workflow_target_snapshot(
        repository,
        &body.expected_head,
        &body.expected_target_ref,
        "structured rebase plan",
    )
    .await?;
    let (upstream, head, available) = advanced_rebase_range(repository, &body.upstream).await?;
    let expected_head = resolve_commit_reference(
        repository,
        advanced_validate_expected_head(&body.expected_head)?,
    )
    .await?;
    if expected_head != head {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The current HEAD changed since this rebase plan was prepared. Refresh the plan and confirm again.",
        ));
    }
    let expected_target_ref = validate_expected_target_ref(&body.expected_target_ref)?;
    let current_target_ref = checked_out_target_ref(repository).await?;
    if expected_target_ref != current_target_ref {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The checked-out branch changed since this rebase plan was prepared. Refresh the plan and confirm again.",
        ));
    }
    if body.steps.len() != available.len() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "The rebase plan must contain every commit in the selected range exactly once",
        ));
    }

    let expected = available
        .iter()
        .map(|commit| commit.commit.clone())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut has_previous_applied = false;
    let mut validated = Vec::with_capacity(body.steps.len());
    for step in &body.steps {
        let commit = resolve_commit_reference(repository, &step.commit).await?;
        if !expected.contains(&commit) || !seen.insert(commit.clone()) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "The rebase plan must contain every commit in the selected range exactly once",
            ));
        }
        let action = AdvancedRebaseAction::parse(&step.action)?;
        if matches!(
            action,
            AdvancedRebaseAction::Squash | AdvancedRebaseAction::Fixup
        ) && !has_previous_applied
        {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "A squash or fixup step must follow a commit that is kept by the rebase plan",
            ));
        }
        let reword_message = match (action, step.message.as_deref()) {
            (AdvancedRebaseAction::Reword, Some(message)) => {
                Some(advanced_validate_message(message, "reword message")?)
            }
            (AdvancedRebaseAction::Reword, None) => {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "A reword rebase step requires a message",
                ));
            }
            (_, Some(_)) => {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Only reword rebase steps may include a message",
                ));
            }
            (_, None) => None,
        };
        if action.is_applied() {
            has_previous_applied = true;
        }
        validated.push(AdvancedValidatedRebaseStep {
            commit,
            action,
            reword_message,
        });
    }
    if seen != expected {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "The rebase plan must contain every commit in the selected range exactly once",
        ));
    }
    Ok((upstream, validated))
}

fn advanced_shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\\"'\\\"'"))
}

#[cfg(unix)]
async fn advanced_make_script_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .await
        .map_err(io_server_error)?
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions)
        .await
        .map_err(io_server_error)
}

#[cfg(not(unix))]
async fn advanced_make_script_executable(_path: &Path) -> Result<()> {
    Err(ServerError::new(
        StatusCode::NOT_IMPLEMENTED,
        "Structured rebase is currently available on Unix Git hosts only",
    ))
}

async fn advanced_rebase_artifact_directory(repository: &Path) -> Result<PathBuf> {
    let git_dir = git_metadata_directory(repository).await?;
    let directory = git_dir
        .join("iowb-rebase")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&directory)
        .await
        .map_err(io_server_error)?;
    Ok(directory)
}

async fn advanced_remove_rebase_artifact(directory: &Path) {
    let _ = fs::remove_dir_all(directory).await;
}

async fn advanced_cleanup_orphaned_rebase_artifacts(repository: &Path) -> Result<()> {
    if detect_git_workflow(repository).await?.is_some() {
        return Ok(());
    }
    let directory = git_metadata_directory(repository)
        .await?
        .join("iowb-rebase");
    let mut entries = match fs::read_dir(&directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_server_error(error)),
    };
    while let Some(entry) = entries.next_entry().await.map_err(io_server_error)? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if uuid::Uuid::parse_str(name).is_ok()
            && entry.file_type().await.map_err(io_server_error)?.is_dir()
        {
            let _ = fs::remove_dir_all(entry.path()).await;
        }
    }
    Ok(())
}

async fn advanced_write_rebase_scripts(
    repository: &Path,
    steps: &[AdvancedValidatedRebaseStep],
) -> Result<(PathBuf, PathBuf)> {
    let directory = advanced_rebase_artifact_directory(repository).await?;
    let result = async {
        let mut todo = String::new();
        let mut reword_index = 0usize;
        for step in steps {
            todo.push_str(step.action.todo_name());
            todo.push(' ');
            todo.push_str(&step.commit);
            todo.push('\n');
            if let Some(message) = &step.reword_message {
                let message_path = directory.join(format!("reword-{reword_index}.txt"));
                let script_path = directory.join(format!("reword-{reword_index}.sh"));
                fs::write(&message_path, format!("{message}\n"))
                    .await
                    .map_err(io_server_error)?;
                fs::write(
                    &script_path,
                    format!(
                        "#!/bin/sh\nset -eu\ngit commit --amend --no-edit -F {}\n",
                        advanced_shell_quote(&message_path.to_string_lossy())
                    ),
                )
                .await
                .map_err(io_server_error)?;
                advanced_make_script_executable(&script_path).await?;
                todo.push_str("exec ");
                todo.push_str(&advanced_shell_quote(&script_path.to_string_lossy()));
                todo.push('\n');
                reword_index += 1;
            }
        }
        let todo_path = directory.join("todo");
        fs::write(&todo_path, todo).await.map_err(io_server_error)?;
        let sequence_editor = directory.join("sequence-editor.sh");
        fs::write(
            &sequence_editor,
            format!(
                "#!/bin/sh\nset -eu\ncat {} > \"$1\"\n",
                advanced_shell_quote(&todo_path.to_string_lossy())
            ),
        )
        .await
        .map_err(io_server_error)?;
        advanced_make_script_executable(&sequence_editor).await?;
        Ok::<_, ServerError>((directory.clone(), sequence_editor))
    }
    .await;
    if result.is_err() {
        advanced_remove_rebase_artifact(&directory).await;
    }
    result
}

/// Keep the managed rebase invocation explicit instead of inheriting
/// repository defaults that can change a plan after the user reviewed it.
/// These switches mirror the stable parts of LazyGit's managed interactive
/// rebase command while retaining the native flow's deliberate no-autostash
/// boundary.
fn advanced_structured_rebase_args(upstream: &str) -> Vec<String> {
    vec![
        "rebase".to_string(),
        "--no-autostash".to_string(),
        "--no-autosquash".to_string(),
        "--keep-empty".to_string(),
        "--interactive".to_string(),
        upstream.to_string(),
    ]
}

async fn advanced_apply_rebase_plan(
    State(state): State<AppState>,
    Json(body): Json<AdvancedRebasePlanBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = advanced_git_repository(&state, &body.scope).await?;
    advanced_cleanup_orphaned_rebase_artifacts(&repository).await?;
    let (upstream, steps) = advanced_validate_rebase_plan(&repository, &body).await?;
    let (artifact_directory, sequence_editor) =
        advanced_write_rebase_scripts(&repository, &steps).await?;
    let sequence_editor_command = advanced_shell_quote(&sequence_editor.to_string_lossy());
    let environment = [
        ("GIT_EDITOR", ":"),
        ("GIT_SEQUENCE_EDITOR", sequence_editor_command.as_str()),
    ];
    let command = git_with_environment(
        &repository,
        advanced_structured_rebase_args(&upstream),
        &environment,
    )
    .await;
    let response = workflow_response_after_command(
        &repository,
        command,
        "Structured rebase completed successfully",
    )
    .await;
    match response {
        Ok(response) => {
            if !response
                .operation_state
                .as_ref()
                .is_some_and(|state| state.in_progress)
            {
                advanced_remove_rebase_artifact(&artifact_directory).await;
            }
            Ok(Json(response))
        }
        Err(error) => {
            advanced_remove_rebase_artifact(&artifact_directory).await;
            Err(error)
        }
    }
}

// ---------------------------------------------------------------------------
// Repository-local safe Git configuration

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedGitConfigResponse {
    user_name: Option<String>,
    user_email: Option<String>,
    pull_rebase: Option<String>,
    commit_gpg_sign: Option<bool>,
    tag_gpg_sign: Option<bool>,
    signing_key_id: Option<String>,
    signing_key_configured: bool,
    gpg_format: Option<String>,
    ssh_variant: Option<String>,
    ssh_allowed_signers_configured: bool,
    ssh_command_configured: bool,
    credential_helper_configured: bool,
    credential_helper_count: usize,
}

async fn advanced_git_config_response(repository: &Path) -> Result<AdvancedGitConfigResponse> {
    let signing_key = advanced_local_config_value(repository, "user.signingKey").await?;
    let signing_key_id = signing_key
        .as_deref()
        .and_then(|value| advanced_validate_signing_key_id(value).ok());
    let pull_rebase = advanced_local_config_value(repository, "pull.rebase")
        .await?
        .and_then(|value| advanced_validate_pull_rebase(&value).ok());
    let gpg_format = advanced_local_config_value(repository, "gpg.format")
        .await?
        .and_then(|value| advanced_validate_gpg_format(&value).ok());
    let ssh_variant = advanced_local_config_value(repository, "ssh.variant")
        .await?
        .and_then(|value| advanced_validate_ssh_variant(&value).ok());
    let credential_helper_count =
        advanced_local_config_value_count(repository, "credential.helper").await?;
    Ok(AdvancedGitConfigResponse {
        user_name: advanced_local_config_value(repository, "user.name")
            .await?
            .and_then(|value| advanced_validate_identity_name(&value).ok()),
        user_email: advanced_local_config_value(repository, "user.email")
            .await?
            .and_then(|value| advanced_validate_identity_email(&value).ok()),
        pull_rebase,
        commit_gpg_sign: advanced_local_config_value(repository, "commit.gpgSign")
            .await?
            .as_deref()
            .and_then(advanced_parse_git_bool),
        tag_gpg_sign: advanced_local_config_value(repository, "tag.gpgSign")
            .await?
            .as_deref()
            .and_then(advanced_parse_git_bool),
        signing_key_configured: signing_key.is_some(),
        signing_key_id,
        gpg_format,
        ssh_variant,
        ssh_allowed_signers_configured: advanced_local_config_value(
            repository,
            "gpg.ssh.allowedSignersFile",
        )
        .await?
        .is_some(),
        ssh_command_configured: advanced_local_config_value(repository, "core.sshCommand")
            .await?
            .is_some(),
        credential_helper_configured: credential_helper_count > 0,
        credential_helper_count,
    })
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedGitConfigQuery {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
}

async fn advanced_git_config(
    State(state): State<AppState>,
    Query(query): Query<AdvancedGitConfigQuery>,
) -> Result<Json<AdvancedGitConfigResponse>> {
    let repository = advanced_git_repository(&state, &query.scope).await?;
    Ok(Json(advanced_git_config_response(&repository).await?))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedSetGitConfigBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    user_name: Option<String>,
    user_email: Option<String>,
    pull_rebase: Option<String>,
    commit_gpg_sign: Option<bool>,
    tag_gpg_sign: Option<bool>,
    signing_key_id: Option<String>,
    #[serde(default)]
    clear_signing_key: bool,
    gpg_format: Option<String>,
    ssh_variant: Option<String>,
    ssh_allowed_signers_path: Option<String>,
    #[serde(default)]
    clear_ssh_allowed_signers_path: bool,
    #[serde(default)]
    clear_ssh_command: bool,
    #[serde(default)]
    clear_credential_helpers: bool,
}

async fn advanced_set_git_config(
    State(state): State<AppState>,
    Json(body): Json<AdvancedSetGitConfigBody>,
) -> Result<Json<AdvancedGitConfigResponse>> {
    if body.user_name.is_none()
        && body.user_email.is_none()
        && body.pull_rebase.is_none()
        && body.commit_gpg_sign.is_none()
        && body.tag_gpg_sign.is_none()
        && body.signing_key_id.is_none()
        && !body.clear_signing_key
        && body.gpg_format.is_none()
        && body.ssh_variant.is_none()
        && body.ssh_allowed_signers_path.is_none()
        && !body.clear_ssh_allowed_signers_path
        && !body.clear_ssh_command
        && !body.clear_credential_helpers
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Choose at least one Git configuration value to update",
        ));
    }
    if body.signing_key_id.is_some() && body.clear_signing_key {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Set or clear signingKeyId, not both",
        ));
    }
    if body.ssh_allowed_signers_path.is_some() && body.clear_ssh_allowed_signers_path {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Set or clear sshAllowedSignersPath, not both",
        ));
    }
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    if let Some(value) = body.user_name.as_deref() {
        advanced_set_local_config(
            &repository,
            "user.name",
            &advanced_validate_identity_name(value)?,
        )
        .await?;
    }
    if let Some(value) = body.user_email.as_deref() {
        advanced_set_local_config(
            &repository,
            "user.email",
            &advanced_validate_identity_email(value)?,
        )
        .await?;
    }
    if let Some(value) = body.pull_rebase.as_deref() {
        advanced_set_local_config(
            &repository,
            "pull.rebase",
            &advanced_validate_pull_rebase(value)?,
        )
        .await?;
    }
    if let Some(value) = body.commit_gpg_sign {
        advanced_set_local_config(
            &repository,
            "commit.gpgSign",
            if value { "true" } else { "false" },
        )
        .await?;
    }
    if let Some(value) = body.tag_gpg_sign {
        advanced_set_local_config(
            &repository,
            "tag.gpgSign",
            if value { "true" } else { "false" },
        )
        .await?;
    }
    if let Some(value) = body.signing_key_id.as_deref() {
        advanced_set_local_config(
            &repository,
            "user.signingKey",
            &advanced_validate_signing_key_id(value)?,
        )
        .await?;
    }
    if body.clear_signing_key {
        advanced_unset_local_config(&repository, "user.signingKey").await?;
    }
    if let Some(value) = body.gpg_format.as_deref() {
        advanced_set_local_config(
            &repository,
            "gpg.format",
            &advanced_validate_gpg_format(value)?,
        )
        .await?;
    }
    if let Some(value) = body.ssh_variant.as_deref() {
        advanced_set_local_config(
            &repository,
            "ssh.variant",
            &advanced_validate_ssh_variant(value)?,
        )
        .await?;
    }
    if let Some(value) = body.ssh_allowed_signers_path.as_deref() {
        let path = advanced_validate_ssh_allowed_signers_path(&repository, value).await?;
        advanced_set_local_config(&repository, "gpg.ssh.allowedSignersFile", &path).await?;
    }
    if body.clear_ssh_allowed_signers_path {
        advanced_unset_local_config(&repository, "gpg.ssh.allowedSignersFile").await?;
    }
    // Shell-command settings are intentionally clear-only.  Accepting a
    // command string for either of these options would turn the HTTP API into
    // a remote command configuration surface and could expose credentials.
    if body.clear_ssh_command {
        advanced_unset_local_config(&repository, "core.sshCommand").await?;
    }
    if body.clear_credential_helpers {
        advanced_unset_local_config(&repository, "credential.helper").await?;
    }
    Ok(Json(advanced_git_config_response(&repository).await?))
}

// ---------------------------------------------------------------------------
// Git LFS

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedGitLfsResponse {
    available: bool,
    version: Option<String>,
    initialized: bool,
    tracked_patterns: Vec<String>,
    status: Option<String>,
    status_truncated: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedGitLfsQuery {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
}

fn advanced_parse_lfs_patterns(output: &str) -> Vec<String> {
    let mut patterns = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("Listing ") {
            continue;
        }
        let pattern = line
            .split_once(" (")
            .map(|(pattern, _)| pattern)
            .unwrap_or(line)
            .trim();
        if !pattern.is_empty() && !patterns.iter().any(|existing| existing == pattern) {
            patterns.push(pattern.to_string());
        }
    }
    patterns
}

async fn advanced_git_lfs_version(repository: &Path) -> Option<String> {
    git(repository, ["lfs", "version"])
        .await
        .ok()
        .and_then(|output| {
            output
                .stdout
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_string)
        })
}

async fn advanced_require_git_lfs(repository: &Path) -> Result<String> {
    advanced_git_lfs_version(repository).await.ok_or_else(|| {
        ServerError::new(
            StatusCode::NOT_IMPLEMENTED,
            "Git LFS is not installed on this server",
        )
    })
}

async fn advanced_git_lfs_response(repository: &Path) -> Result<AdvancedGitLfsResponse> {
    let Some(version) = advanced_git_lfs_version(repository).await else {
        return Ok(AdvancedGitLfsResponse {
            available: false,
            version: None,
            initialized: false,
            tracked_patterns: Vec::new(),
            status: None,
            status_truncated: false,
        });
    };
    let tracked_patterns = match git(repository, ["lfs", "track"]).await {
        Ok(output) => advanced_parse_lfs_patterns(&output.stdout),
        Err(_) => Vec::new(),
    };
    let initialized = advanced_effective_config_value(repository, "filter.lfs.process")
        .await?
        .is_some();
    let status = match git(repository, ["lfs", "status"]).await {
        Ok(output) => Some(advanced_truncate_output(join_output_or(
            &output,
            "Git LFS is ready",
        ))),
        Err(_) => None,
    };
    let (status, status_truncated) = match status {
        Some((status, truncated)) => (Some(status), truncated),
        None => (None, false),
    };
    Ok(AdvancedGitLfsResponse {
        available: true,
        version: Some(version),
        initialized,
        tracked_patterns,
        status,
        status_truncated,
    })
}

async fn advanced_git_lfs_status(
    State(state): State<AppState>,
    Query(query): Query<AdvancedGitLfsQuery>,
) -> Result<Json<AdvancedGitLfsResponse>> {
    let repository = advanced_git_repository(&state, &query.scope).await?;
    Ok(Json(advanced_git_lfs_response(&repository).await?))
}

fn advanced_validate_lfs_pattern(value: &str) -> Result<String> {
    let value = value.trim();
    let normalized = value.replace('\\', "/");
    if value.is_empty()
        || value.len() > ADVANCED_GIT_MAX_PATTERN_LENGTH
        || value.starts_with('-')
        || value.starts_with('/')
        || value.contains('\0')
        || value.chars().any(char::is_control)
        || value.chars().any(char::is_whitespace)
        || normalized.split('/').any(|component| component == "..")
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid Git LFS tracking pattern",
        ));
    }
    Ok(value.to_string())
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedGitLfsPatternBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    pattern: String,
}

async fn advanced_git_lfs_track(
    State(state): State<AppState>,
    Json(body): Json<AdvancedGitLfsPatternBody>,
) -> Result<Json<AdvancedGitLfsResponse>> {
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    advanced_require_git_lfs(&repository).await?;
    let pattern = advanced_validate_lfs_pattern(&body.pattern)?;
    // Configure only this repository. It does not install credentials or
    // keys; it simply registers the LFS filter Git needs for tracked files.
    git(&repository, ["lfs", "install", "--local"]).await?;
    git(&repository, ["lfs", "track", pattern.as_str()]).await?;
    Ok(Json(advanced_git_lfs_response(&repository).await?))
}

async fn advanced_git_lfs_untrack(
    State(state): State<AppState>,
    Json(body): Json<AdvancedGitLfsPatternBody>,
) -> Result<Json<AdvancedGitLfsResponse>> {
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    advanced_require_git_lfs(&repository).await?;
    let pattern = advanced_validate_lfs_pattern(&body.pattern)?;
    git(&repository, ["lfs", "untrack", pattern.as_str()]).await?;
    Ok(Json(advanced_git_lfs_response(&repository).await?))
}

// ---------------------------------------------------------------------------
// Sparse checkout

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedSparseCheckoutResponse {
    enabled: bool,
    cone_mode: Option<bool>,
    patterns: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedSparseCheckoutQuery {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
}

async fn advanced_sparse_checkout_response(
    repository: &Path,
) -> Result<AdvancedSparseCheckoutResponse> {
    let enabled = advanced_effective_config_value(repository, "core.sparseCheckout")
        .await?
        .as_deref()
        .and_then(advanced_parse_git_bool)
        .unwrap_or(false);
    if !enabled {
        return Ok(AdvancedSparseCheckoutResponse {
            enabled: false,
            cone_mode: None,
            patterns: Vec::new(),
        });
    }
    let cone_mode = advanced_effective_config_value(repository, "core.sparseCheckoutCone")
        .await?
        .as_deref()
        .and_then(advanced_parse_git_bool);
    let patterns = match git(repository, ["sparse-checkout", "list"]).await {
        Ok(output) => output
            .stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    };
    Ok(AdvancedSparseCheckoutResponse {
        enabled,
        cone_mode,
        patterns,
    })
}

async fn advanced_sparse_checkout_status(
    State(state): State<AppState>,
    Query(query): Query<AdvancedSparseCheckoutQuery>,
) -> Result<Json<AdvancedSparseCheckoutResponse>> {
    let repository = advanced_git_repository(&state, &query.scope).await?;
    Ok(Json(advanced_sparse_checkout_response(&repository).await?))
}

fn advanced_validate_sparse_pattern(value: &str, cone_mode: bool) -> Result<String> {
    let value = value.trim();
    let normalized = value.replace('\\', "/");
    let components = normalized.split('/').collect::<Vec<_>>();
    let has_glob = value
        .chars()
        .any(|character| matches!(character, '*' | '?' | '[' | ']'));
    if value.is_empty()
        || value.len() > ADVANCED_GIT_MAX_PATTERN_LENGTH
        || value.starts_with('-')
        || value.contains('\0')
        || value.chars().any(char::is_control)
        || normalized.split('/').any(|component| component == "..")
        || components.iter().any(|component| component == &".git")
        || (cone_mode
            && (value.starts_with('/')
                || value.starts_with('!')
                || has_glob
                || components
                    .iter()
                    .any(|component| component.is_empty() || component == &".")))
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            if cone_mode {
                "Cone sparse-checkout entries must be repository-relative directories without glob patterns"
            } else {
                "Invalid sparse-checkout pattern"
            },
        ));
    }
    Ok(value.to_string())
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedSetSparseCheckoutBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    patterns: Vec<String>,
    cone_mode: Option<bool>,
    #[serde(default)]
    confirm: bool,
}

async fn advanced_set_sparse_checkout(
    State(state): State<AppState>,
    Json(body): Json<AdvancedSetSparseCheckoutBody>,
) -> Result<Json<AdvancedSparseCheckoutResponse>> {
    advanced_require_confirmation(body.confirm, "change sparse-checkout patterns")?;
    if body.patterns.is_empty() || body.patterns.len() > ADVANCED_GIT_MAX_PATTERNS {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!("Choose between one and {ADVANCED_GIT_MAX_PATTERNS} sparse-checkout patterns"),
        ));
    }
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    advanced_require_clean_worktree(&repository, "change sparse-checkout patterns").await?;
    let cone_mode = body.cone_mode.unwrap_or(true);
    let mut patterns = Vec::with_capacity(body.patterns.len());
    let mut seen = HashSet::new();
    for pattern in &body.patterns {
        let pattern = advanced_validate_sparse_pattern(pattern, cone_mode)?;
        if !seen.insert(pattern.clone()) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Sparse-checkout patterns must not contain duplicates",
            ));
        }
        patterns.push(pattern);
    }
    let mut args = vec!["sparse-checkout".to_string(), "set".to_string()];
    args.push(if cone_mode {
        "--cone".to_string()
    } else {
        "--no-cone".to_string()
    });
    args.extend(patterns);
    git(&repository, args).await?;
    Ok(Json(advanced_sparse_checkout_response(&repository).await?))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedDisableSparseCheckoutBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    #[serde(default)]
    confirm: bool,
}

async fn advanced_disable_sparse_checkout(
    State(state): State<AppState>,
    Json(body): Json<AdvancedDisableSparseCheckoutBody>,
) -> Result<Json<AdvancedSparseCheckoutResponse>> {
    advanced_require_confirmation(body.confirm, "disable sparse checkout")?;
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    advanced_require_clean_worktree(&repository, "disable sparse checkout").await?;
    if advanced_sparse_checkout_response(&repository)
        .await?
        .enabled
    {
        git(&repository, ["sparse-checkout", "disable"]).await?;
    }
    Ok(Json(advanced_sparse_checkout_response(&repository).await?))
}

// ---------------------------------------------------------------------------
// .gitignore helper

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedGitignoreAppendResponse {
    added: bool,
    path: String,
    pattern: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedGitignoreAppendBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    pattern: String,
}

fn advanced_validate_gitignore_pattern(value: &str) -> Result<String> {
    let value = value.trim();
    let normalized = value.replace('\\', "/");
    if value.is_empty()
        || value.len() > ADVANCED_GIT_MAX_PATTERN_LENGTH
        || value.contains('\0')
        || value.chars().any(char::is_control)
        || value.starts_with('#')
        || normalized.split('/').any(|component| component == "..")
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid .gitignore pattern",
        ));
    }
    Ok(value.to_string())
}

async fn advanced_append_gitignore(
    State(state): State<AppState>,
    Json(body): Json<AdvancedGitignoreAppendBody>,
) -> Result<Json<AdvancedGitignoreAppendResponse>> {
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let pattern = advanced_validate_gitignore_pattern(&body.pattern)?;
    let ignore_path = safe_repo_child(&repository, ".gitignore")?;
    let existing = match fs::read(&ignore_path).await {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(io_server_error(error)),
    };
    let already_present = String::from_utf8_lossy(&existing)
        .lines()
        .any(|line| line.trim_end_matches('\r') == pattern);
    if !already_present {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&ignore_path)
            .await
            .map_err(io_server_error)?;
        if !existing.is_empty() && !existing.ends_with(b"\n") {
            file.write_all(b"\n").await.map_err(io_server_error)?;
        }
        file.write_all(pattern.as_bytes())
            .await
            .map_err(io_server_error)?;
        file.write_all(b"\n").await.map_err(io_server_error)?;
        file.flush().await.map_err(io_server_error)?;
    }
    Ok(Json(AdvancedGitignoreAppendResponse {
        added: !already_present,
        path: ".gitignore".to_string(),
        pattern,
    }))
}

// ---------------------------------------------------------------------------
// Submodule add/remove

fn advanced_validate_new_submodule_path(repository: &Path, value: &str) -> Result<String> {
    validate_file_path(value)?;
    let path = normalize_repo_relative_path(value);
    validate_file_path(&path)?;
    if path.split('/').any(|component| component == ".git") {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "A submodule path cannot contain a .git directory",
        ));
    }
    let destination = safe_repo_child(repository, &path)?;
    if destination.exists() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "The new submodule path already exists",
        ));
    }
    Ok(path)
}

async fn advanced_configured_submodule_path(repository: &Path, value: &str) -> Result<String> {
    validate_file_path(value)?;
    let path = normalize_repo_relative_path(value);
    validate_file_path(&path)?;
    let configured = read_submodule_paths(repository).await;
    if !configured.iter().any(|candidate| {
        pathdiff(candidate, repository)
            .map(|relative| normalize_repo_relative_path(&relative.to_string_lossy()) == path)
            .unwrap_or(false)
    }) {
        return Err(ServerError::new(
            StatusCode::NOT_FOUND,
            "The selected path is not a configured submodule",
        ));
    }
    safe_repo_child(repository, &path)?;
    Ok(path)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedSubmoduleAddBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    url: String,
    path: String,
    branch: Option<String>,
    #[serde(default)]
    confirm: bool,
}

async fn advanced_add_submodule(
    State(state): State<AppState>,
    Json(body): Json<AdvancedSubmoduleAddBody>,
) -> Result<Json<GitOperationResponse>> {
    advanced_require_confirmation(body.confirm, "add a submodule")?;
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let url = validate_remote_url(&body.url)?;
    let path = advanced_validate_new_submodule_path(&repository, &body.path)?;
    let branch = body
        .branch
        .as_deref()
        .map(validate_branch_name)
        .transpose()?;
    let mut args = vec!["submodule".to_string(), "add".to_string()];
    if let Some(branch) = branch {
        args.push("--branch".to_string());
        args.push(branch);
    }
    args.push("--".to_string());
    args.push(url);
    args.push(path);
    let output = git(&repository, args).await?;
    Ok(Json(
        advanced_operation_response(&repository, &output, "Submodule added successfully").await?,
    ))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedSubmoduleRemoveBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    path: String,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    purge_metadata: bool,
    #[serde(default)]
    confirm_purge_metadata: bool,
}

async fn advanced_purge_submodule_metadata(repository: &Path, path: &str) -> Result<()> {
    let modules_directory = git_metadata_directory(repository).await?.join("modules");
    if !modules_directory.exists() {
        return Ok(());
    }
    let metadata = safe_repo_child(&modules_directory, path)?;
    if metadata.exists() {
        fs::remove_dir_all(metadata)
            .await
            .map_err(io_server_error)?;
    }
    Ok(())
}

async fn advanced_remove_submodule(
    State(state): State<AppState>,
    Json(body): Json<AdvancedSubmoduleRemoveBody>,
) -> Result<Json<GitOperationResponse>> {
    advanced_require_confirmation(body.confirm, "remove a submodule")?;
    if body.purge_metadata {
        advanced_require_confirmation(
            body.confirm_purge_metadata,
            "permanently purge the removed submodule's local Git metadata",
        )?;
    }
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let path = advanced_configured_submodule_path(&repository, &body.path).await?;
    // `deinit` clears the repository-local configuration and checkout. `rm`
    // then updates .gitmodules and stages the gitlink removal. This preserves
    // a normal Git commit/review workflow rather than deleting it behind the
    // user's back.
    git(&repository, ["submodule", "deinit", "--force", "--", &path]).await?;
    let output = git(&repository, ["rm", "--force", "--", &path]).await?;
    if body.purge_metadata {
        advanced_purge_submodule_metadata(&repository, &path).await?;
    }
    Ok(Json(
        advanced_operation_response(
            &repository,
            &output,
            if body.purge_metadata {
                "Submodule removed and local metadata purged successfully"
            } else {
                "Submodule removal staged successfully"
            },
        )
        .await?,
    ))
}

// ---------------------------------------------------------------------------
// Read-only hooks, signing, SSH, and credential-helper integration status

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedGitIntegrationsStatusResponse {
    active_hooks: Vec<String>,
    custom_hooks_path_configured: bool,
    commit_signing_enabled: Option<bool>,
    tag_signing_enabled: Option<bool>,
    signing_key_id: Option<String>,
    signing_key_configured: bool,
    gpg_format: Option<String>,
    ssh_command_configured: bool,
    ssh_variant: Option<String>,
    ssh_allowed_signers_configured: bool,
    credential_helper_configured: bool,
    credential_helper_count: usize,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedGitIntegrationsStatusQuery {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
}

fn advanced_safe_status_value(value: &str, max_length: usize) -> Option<String> {
    let value = value.trim();
    (value.len() <= max_length
        && !value.is_empty()
        && value
            .chars()
            .all(|character| !character.is_control() && !character.is_whitespace()))
    .then(|| value.to_string())
}

async fn advanced_active_default_hooks(repository: &Path) -> Result<Vec<String>> {
    let hooks_directory = git_metadata_directory(repository).await?.join("hooks");
    let mut entries = match fs::read_dir(hooks_directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_server_error(error)),
    };
    let mut hooks = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(io_server_error)? {
        let file_type = entry.file_type().await.map_err(io_server_error)?;
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !name.ends_with(".sample") && advanced_safe_status_value(&name, 255).is_some() {
            hooks.push(name);
        }
    }
    hooks.sort();
    Ok(hooks)
}

async fn advanced_git_integrations_status_response(
    repository: &Path,
) -> Result<AdvancedGitIntegrationsStatusResponse> {
    let custom_hooks_path_configured =
        advanced_effective_config_value(repository, "core.hooksPath")
            .await?
            .is_some();
    let signing_key = advanced_effective_config_value(repository, "user.signingKey").await?;
    let gpg_format = advanced_effective_config_value(repository, "gpg.format")
        .await?
        .and_then(|value| advanced_validate_gpg_format(&value).ok());
    let ssh_variant = advanced_effective_config_value(repository, "ssh.variant")
        .await?
        .as_deref()
        .and_then(|value| advanced_safe_status_value(value, 64));
    let credential_helper_count =
        advanced_effective_config_value_count(repository, "credential.helper").await?;
    Ok(AdvancedGitIntegrationsStatusResponse {
        active_hooks: if custom_hooks_path_configured {
            // A custom hooks path may point outside the repository. Do not
            // enumerate it or reveal its path through a mobile client.
            Vec::new()
        } else {
            advanced_active_default_hooks(repository).await?
        },
        custom_hooks_path_configured,
        commit_signing_enabled: advanced_effective_config_value(repository, "commit.gpgSign")
            .await?
            .as_deref()
            .and_then(advanced_parse_git_bool),
        tag_signing_enabled: advanced_effective_config_value(repository, "tag.gpgSign")
            .await?
            .as_deref()
            .and_then(advanced_parse_git_bool),
        signing_key_id: signing_key
            .as_deref()
            .and_then(|value| advanced_validate_signing_key_id(value).ok()),
        signing_key_configured: signing_key.is_some(),
        gpg_format,
        ssh_command_configured: advanced_effective_config_value(repository, "core.sshCommand")
            .await?
            .is_some(),
        ssh_variant,
        ssh_allowed_signers_configured: advanced_effective_config_value(
            repository,
            "gpg.ssh.allowedSignersFile",
        )
        .await?
        .is_some(),
        // The value of credential.helper can be a shell command or include a
        // secret. Return only a count/status, never its configured value.
        credential_helper_configured: credential_helper_count > 0,
        credential_helper_count,
    })
}

async fn advanced_git_integrations_status(
    State(state): State<AppState>,
    Query(query): Query<AdvancedGitIntegrationsStatusQuery>,
) -> Result<Json<AdvancedGitIntegrationsStatusResponse>> {
    let repository = advanced_git_repository(&state, &query.scope).await?;
    Ok(Json(
        advanced_git_integrations_status_response(&repository).await?,
    ))
}

// ---------------------------------------------------------------------------
// Git bisect

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedBisectStatusResponse {
    in_progress: bool,
    current_commit: Option<String>,
    start_reference: Option<String>,
    good_commits: Vec<String>,
    bad_commits: Vec<String>,
    log: Option<String>,
    log_truncated: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedBisectStatusQuery {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
}

async fn advanced_bisect_in_progress(repository: &Path) -> Result<bool> {
    let git_dir = git_metadata_directory(repository).await?;
    git_metadata_exists(&git_dir.join("BISECT_START")).await
}

fn advanced_parse_bisect_log_refs(log: &str, term: &str) -> Vec<String> {
    let prefix = format!("git bisect {term} ");
    log.lines()
        .filter_map(|line| line.strip_prefix(&prefix))
        .filter_map(|value| value.split_whitespace().next())
        .filter_map(|value| validate_commit_ref(value).ok())
        .collect()
}

async fn advanced_bisect_status_response(
    repository: &Path,
) -> Result<AdvancedBisectStatusResponse> {
    let git_dir = git_metadata_directory(repository).await?;
    if !git_metadata_exists(&git_dir.join("BISECT_START")).await? {
        return Ok(AdvancedBisectStatusResponse {
            in_progress: false,
            current_commit: None,
            start_reference: None,
            good_commits: Vec::new(),
            bad_commits: Vec::new(),
            log: None,
            log_truncated: false,
        });
    }
    let output = git(repository, ["bisect", "log"]).await?;
    let (log, log_truncated) = advanced_truncate_output(output.stdout);
    Ok(AdvancedBisectStatusResponse {
        in_progress: true,
        current_commit: advanced_current_head(repository).await.ok(),
        start_reference: read_git_metadata_value(&git_dir.join("BISECT_START")).await?,
        good_commits: advanced_parse_bisect_log_refs(&log, "good"),
        bad_commits: advanced_parse_bisect_log_refs(&log, "bad"),
        log: Some(log),
        log_truncated,
    })
}

async fn advanced_git_bisect_status(
    State(state): State<AppState>,
    Query(query): Query<AdvancedBisectStatusQuery>,
) -> Result<Json<AdvancedBisectStatusResponse>> {
    let repository = advanced_git_repository(&state, &query.scope).await?;
    Ok(Json(advanced_bisect_status_response(&repository).await?))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedBisectStartBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    bad: String,
    good: Vec<String>,
    expected_head: String,
    /// The local branch (or detached HEAD) shown when the bisect form opened.
    expected_target_ref: String,
    #[serde(default)]
    confirm: bool,
}

async fn advanced_git_bisect_start(
    State(state): State<AppState>,
    Json(body): Json<AdvancedBisectStartBody>,
) -> Result<Json<AdvancedBisectStatusResponse>> {
    advanced_require_confirmation(body.confirm, "start Git bisect")?;
    if body.good.is_empty() || body.good.len() > ADVANCED_GIT_MAX_REBASE_COMMITS {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Choose between one and 100 known-good commits for Git bisect",
        ));
    }
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    if advanced_bisect_in_progress(&repository).await? {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Git bisect is already in progress. Mark the current commit or reset it first.",
        ));
    }
    advanced_require_clean_worktree(&repository, "start Git bisect").await?;
    validate_workflow_target_snapshot(
        &repository,
        &body.expected_head,
        &body.expected_target_ref,
        "Git bisect",
    )
    .await?;
    let bad = resolve_commit_reference(&repository, &body.bad).await?;
    let mut good = Vec::with_capacity(body.good.len());
    let mut seen = HashSet::new();
    for commit in &body.good {
        let commit = resolve_commit_reference(&repository, commit).await?;
        if commit == bad || !seen.insert(commit.clone()) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Known-good commits must be unique and different from the known-bad commit",
            ));
        }
        if !advanced_is_ancestor(&repository, &commit, &bad).await {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Every known-good commit must be an ancestor of the known-bad commit",
            ));
        }
        good.push(commit);
    }
    git(&repository, ["bisect", "start"]).await?;
    let result = async {
        git(&repository, ["bisect", "bad", bad.as_str()]).await?;
        for commit in &good {
            git(&repository, ["bisect", "good", commit.as_str()]).await?;
        }
        Ok::<_, ServerError>(())
    }
    .await;
    if let Err(error) = result {
        let _ = git(&repository, ["bisect", "reset"]).await;
        return Err(error);
    }
    Ok(Json(advanced_bisect_status_response(&repository).await?))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedBisectMarkBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    expected_head: String,
    #[serde(default)]
    confirm: bool,
}

async fn advanced_mark_bisect_current(
    state: AppState,
    body: AdvancedBisectMarkBody,
    term: &str,
) -> Result<Json<AdvancedBisectStatusResponse>> {
    advanced_require_confirmation(body.confirm, &format!("mark the current commit as {term}"))?;
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_non_bisect_git_workflow_in_progress(&repository).await?;
    if !advanced_bisect_in_progress(&repository).await? {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Git bisect is not in progress",
        ));
    }
    advanced_require_clean_worktree(&repository, "mark a Git bisect result").await?;
    let current = advanced_require_expected_head(&repository, &body.expected_head).await?;
    git(&repository, ["bisect", term, current.as_str()]).await?;
    Ok(Json(advanced_bisect_status_response(&repository).await?))
}

async fn advanced_git_bisect_good(
    State(state): State<AppState>,
    Json(body): Json<AdvancedBisectMarkBody>,
) -> Result<Json<AdvancedBisectStatusResponse>> {
    advanced_mark_bisect_current(state, body, "good").await
}

async fn advanced_git_bisect_bad(
    State(state): State<AppState>,
    Json(body): Json<AdvancedBisectMarkBody>,
) -> Result<Json<AdvancedBisectStatusResponse>> {
    advanced_mark_bisect_current(state, body, "bad").await
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedBisectResetBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    #[serde(default)]
    confirm: bool,
}

async fn advanced_git_bisect_reset(
    State(state): State<AppState>,
    Json(body): Json<AdvancedBisectResetBody>,
) -> Result<Json<AdvancedBisectStatusResponse>> {
    advanced_require_confirmation(body.confirm, "reset Git bisect")?;
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_non_bisect_git_workflow_in_progress(&repository).await?;
    if !advanced_bisect_in_progress(&repository).await? {
        return Ok(Json(advanced_bisect_status_response(&repository).await?));
    }
    advanced_require_clean_worktree(&repository, "reset Git bisect").await?;
    git(&repository, ["bisect", "reset"]).await?;
    Ok(Json(advanced_bisect_status_response(&repository).await?))
}

// ---------------------------------------------------------------------------
// Repository maintenance and garbage collection

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedMaintenanceBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    tasks: Vec<String>,
    #[serde(default)]
    confirm: bool,
}

fn advanced_validate_maintenance_task(value: &str) -> Result<&'static str> {
    match value.trim() {
        "loose-objects" => Ok("loose-objects"),
        "incremental-repack" => Ok("incremental-repack"),
        "pack-refs" => Ok("pack-refs"),
        "commit-graph" => Ok("commit-graph"),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Maintenance task must be loose-objects, incremental-repack, pack-refs, or commit-graph",
        )),
    }
}

async fn advanced_git_maintenance(
    State(state): State<AppState>,
    Json(body): Json<AdvancedMaintenanceBody>,
) -> Result<Json<GitOperationResponse>> {
    advanced_require_confirmation(body.confirm, "run Git repository maintenance")?;
    if body.tasks.is_empty() || body.tasks.len() > 4 {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Choose between one and four repository maintenance tasks",
        ));
    }
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let mut seen = HashSet::new();
    let mut args = vec!["maintenance".to_string(), "run".to_string()];
    for task in &body.tasks {
        let task = advanced_validate_maintenance_task(task)?;
        if !seen.insert(task) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Repository maintenance tasks must not contain duplicates",
            ));
        }
        args.push(format!("--task={task}"));
    }
    let output = git(&repository, args).await?;
    Ok(Json(
        advanced_operation_response(
            &repository,
            &output,
            "Repository maintenance completed successfully",
        )
        .await?,
    ))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdvancedGitGcBody {
    #[serde(flatten)]
    scope: AdvancedGitProjectScope,
    #[serde(default)]
    aggressive: bool,
    #[serde(default)]
    prune_now: bool,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    confirm_prune_now: bool,
}

async fn advanced_git_gc(
    State(state): State<AppState>,
    Json(body): Json<AdvancedGitGcBody>,
) -> Result<Json<GitOperationResponse>> {
    advanced_require_confirmation(body.confirm, "run Git garbage collection")?;
    if body.prune_now {
        advanced_require_confirmation(
            body.confirm_prune_now,
            "permanently prune unreachable Git objects now",
        )?;
    }
    let repository = advanced_git_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let mut args = vec!["gc".to_string()];
    if body.aggressive {
        args.push("--aggressive".to_string());
    }
    if body.prune_now {
        args.push("--prune=now".to_string());
    }
    let output = git(&repository, args).await?;
    Ok(Json(
        advanced_operation_response(
            &repository,
            &output,
            "Git garbage collection completed successfully",
        )
        .await?,
    ))
}

#[cfg(test)]
mod advanced_tests {
    use super::*;

    fn advanced_test_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("iowb-git-advanced-{name}-{}", uuid::Uuid::new_v4()))
    }

    fn advanced_test_command<I, S>(repository: &std::path::Path, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repository)
            .output()
            .expect("run Git test command");
        assert!(
            output.status.success(),
            "Git test command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn advanced_test_output<I, S>(repository: &std::path::Path, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repository)
            .output()
            .expect("run Git test command");
        assert!(
            output.status.success(),
            "Git test command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("Git test output is UTF-8")
    }

    fn advanced_test_repository(root: &std::path::Path) -> std::path::PathBuf {
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).expect("create repository directory");
        advanced_test_command(&repository, ["init", "--initial-branch=main"]);
        advanced_test_command(&repository, ["config", "user.name", "Advanced Test"]);
        advanced_test_command(
            &repository,
            ["config", "user.email", "advanced@example.test"],
        );
        // Keep real-Git tests independent of the machine running them. Tests
        // that need signing toggle it explicitly through the repository-local
        // config endpoint instead of inheriting a developer's global policy.
        advanced_test_command(&repository, ["config", "commit.gpgSign", "false"]);
        advanced_test_command(&repository, ["config", "tag.gpgSign", "false"]);
        repository
    }

    async fn advanced_test_state(root: &std::path::Path) -> AppState {
        let config_dir = root.join("config");
        AppState::initialize(iowb_core::AppConfig {
            host: "127.0.0.1".parse().expect("test host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.to_path_buf(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 4,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("create test app state")
    }

    fn advanced_test_scope(repository: &std::path::Path) -> AdvancedGitProjectScope {
        AdvancedGitProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        }
    }

    fn advanced_test_head(repository: &std::path::Path) -> String {
        advanced_test_output(repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string()
    }

    fn advanced_test_commit(
        repository: &std::path::Path,
        file: &str,
        content: &str,
        message: &str,
    ) {
        std::fs::write(repository.join(file), content).expect("write test file");
        advanced_test_command(repository, ["add", file]);
        advanced_test_command(repository, ["commit", "-m", message]);
    }

    struct AdvancedTestGitDaemon {
        child: std::process::Child,
    }

    impl Drop for AdvancedTestGitDaemon {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn advanced_start_test_git_daemon(root: &std::path::Path) -> (AdvancedTestGitDaemon, u16) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve daemon port");
        let port = listener.local_addr().expect("daemon address").port();
        drop(listener);
        let daemon = AdvancedTestGitDaemon {
            child: std::process::Command::new("git")
                .arg("daemon")
                .arg("--reuseaddr")
                .arg("--export-all")
                .arg(format!("--base-path={}", root.display()))
                .arg("--listen=127.0.0.1")
                .arg(format!("--port={port}"))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("start local Git daemon"),
        };
        for _ in 0..20 {
            if std::net::TcpStream::connect_timeout(
                &format!("127.0.0.1:{port}")
                    .parse()
                    .expect("daemon socket address"),
                std::time::Duration::from_millis(50),
            )
            .is_ok()
            {
                return (daemon, port);
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("local Git daemon did not start");
    }

    #[test]
    fn advanced_validators_reject_unsafe_or_stale_reference_forms() {
        assert!(advanced_validate_expected_head("HEAD").is_err());
        assert!(advanced_validate_expected_head("abc123").is_err());
        assert!(advanced_validate_expected_head(&"a".repeat(40)).is_ok());
        assert!(advanced_validate_signing_key_id("../../private.key").is_err());
        assert!(advanced_validate_ssh_variant("ssh -i ~/.ssh/id_key").is_err());
        assert_eq!(advanced_validate_ssh_variant("PuTTY").unwrap(), "putty");
        assert!(advanced_validate_lfs_pattern("--help").is_err());
        assert!(advanced_validate_lfs_pattern("../secret").is_err());
        assert!(advanced_validate_sparse_pattern("../secret", true).is_err());
        assert!(advanced_validate_sparse_pattern("src/**/*.rs", true).is_err());
        assert!(advanced_validate_gitignore_pattern("# comment").is_err());
        assert!(advanced_history_search(Some("not\na safe literal")).is_err());
        assert!(advanced_history_offset(Some(ADVANCED_GIT_HISTORY_MAX_OFFSET + 1)).is_err());
        assert_eq!(advanced_validate_pull_rebase("interactive").unwrap(), "interactive");
        assert_eq!(advanced_validate_pull_rebase("merges").unwrap(), "merges");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_amend_reword_config_gitignore_and_integration_status_use_real_git() {
        let root = advanced_test_root("amend-config");
        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "README.md", "first\n", "initial subject");
        let state = advanced_test_state(&root).await;

        std::fs::write(repository.join("README.md"), "first\namended\n").expect("modify file");
        advanced_test_command(&repository, ["add", "README.md"]);
        let amended = advanced_amend_head(
            State(state.clone()),
            Json(AdvancedAmendHeadBody {
                scope: advanced_test_scope(&repository),
                message: None,
                confirm: true,
                expected_head: advanced_test_head(&repository),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("amend current commit")
        .0;
        assert!(amended.success);
        assert!(advanced_test_output(&repository, ["show", "HEAD:README.md"]).contains("amended"));

        let reworded = advanced_reword_head(
            State(state.clone()),
            Json(AdvancedRewordHeadBody {
                scope: advanced_test_scope(&repository),
                message: "reworded subject\n\nWith a body.".to_string(),
                confirm: true,
                expected_head: advanced_test_head(&repository),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("reword current commit")
        .0;
        assert!(reworded.success);
        assert_eq!(
            advanced_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "reworded subject"
        );

        std::fs::write(repository.join("allowed_signers"), "principal ssh-ed25519 AAAA\n")
            .expect("write repository-scoped SSH allowed signers");
        advanced_test_command(
            &repository,
            ["config", "--local", "core.sshCommand", "ssh -i private-key"],
        );
        advanced_test_command(
            &repository,
            [
                "config",
                "--local",
                "--add",
                "credential.helper",
                "!private-helper",
            ],
        );

        let config = advanced_set_git_config(
            State(state.clone()),
            Json(AdvancedSetGitConfigBody {
                scope: advanced_test_scope(&repository),
                user_name: Some("Updated User".to_string()),
                user_email: Some("updated@example.test".to_string()),
                pull_rebase: Some("merges".to_string()),
                commit_gpg_sign: Some(false),
                tag_gpg_sign: Some(false),
                signing_key_id: Some("ABCD1234".to_string()),
                clear_signing_key: false,
                gpg_format: Some("ssh".to_string()),
                ssh_variant: Some("ssh".to_string()),
                ssh_allowed_signers_path: Some("allowed_signers".to_string()),
                clear_ssh_allowed_signers_path: false,
                clear_ssh_command: true,
                clear_credential_helpers: true,
            }),
        )
        .await
        .expect("set safe Git config")
        .0;
        assert_eq!(config.user_name.as_deref(), Some("Updated User"));
        assert_eq!(config.pull_rebase.as_deref(), Some("merges"));
        assert_eq!(config.signing_key_id.as_deref(), Some("ABCD1234"));
        assert_eq!(config.gpg_format.as_deref(), Some("ssh"));
        assert_eq!(config.ssh_variant.as_deref(), Some("ssh"));
        assert!(config.ssh_allowed_signers_configured);
        assert!(!config.ssh_command_configured);
        assert!(!config.credential_helper_configured);
        assert_eq!(config.credential_helper_count, 0);
        assert_eq!(
            advanced_test_output(
                &repository,
                ["config", "--local", "--get", "user.signingKey"]
            )
            .trim(),
            "ABCD1234"
        );
        assert!(git(&repository, ["config", "--local", "--get", "core.sshCommand"])
            .await
            .is_err());
        assert!(git(
            &repository,
            ["config", "--local", "--get-all", "credential.helper"]
        )
        .await
        .is_err());
        let cleared_signers = advanced_set_git_config(
            State(state.clone()),
            Json(AdvancedSetGitConfigBody {
                scope: advanced_test_scope(&repository),
                user_name: None,
                user_email: None,
                pull_rebase: None,
                commit_gpg_sign: None,
                tag_gpg_sign: None,
                signing_key_id: None,
                clear_signing_key: false,
                gpg_format: None,
                ssh_variant: None,
                ssh_allowed_signers_path: None,
                clear_ssh_allowed_signers_path: true,
                clear_ssh_command: false,
                clear_credential_helpers: false,
            }),
        )
        .await
        .expect("clear repository-scoped allowed signers setting")
        .0;
        assert!(!cleared_signers.ssh_allowed_signers_configured);

        let first_append = advanced_append_gitignore(
            State(state.clone()),
            Json(AdvancedGitignoreAppendBody {
                scope: advanced_test_scope(&repository),
                pattern: "*.local".to_string(),
            }),
        )
        .await
        .expect("append .gitignore")
        .0;
        assert!(first_append.added);
        let duplicate_append = advanced_append_gitignore(
            State(state.clone()),
            Json(AdvancedGitignoreAppendBody {
                scope: advanced_test_scope(&repository),
                pattern: "*.local".to_string(),
            }),
        )
        .await
        .expect("avoid duplicate .gitignore pattern")
        .0;
        assert!(!duplicate_append.added);
        assert_eq!(
            std::fs::read_to_string(repository.join(".gitignore"))
                .expect("read .gitignore")
                .lines()
                .filter(|line| *line == "*.local")
                .count(),
            1
        );

        let hooks_directory = repository.join(".git/hooks");
        std::fs::write(hooks_directory.join("pre-commit"), "#!/bin/sh\nexit 0\n")
            .expect("write hook");
        let integrations = advanced_git_integrations_status_response(&repository)
            .await
            .expect("read integration status");
        assert!(
            integrations
                .active_hooks
                .iter()
                .any(|hook| hook == "pre-commit")
        );
        assert!(integrations.signing_key_configured);
        assert!(!integrations.credential_helper_configured);

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_reword_preserves_separately_staged_content() {
        let root = advanced_test_root("reword-preserves-index");
        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "README.md", "initial\n", "initial subject");
        let state = advanced_test_state(&root).await;

        let tree_before_reword = advanced_test_output(&repository, ["rev-parse", "HEAD^{tree}"])
            .trim()
            .to_string();
        std::fs::write(repository.join("next-change.txt"), "keep this staged\n")
            .expect("stage a separate next change");
        advanced_test_command(&repository, ["add", "next-change.txt"]);

        let reworded = advanced_reword_head(
            State(state.clone()),
            Json(AdvancedRewordHeadBody {
                scope: advanced_test_scope(&repository),
                message: "renamed subject".to_string(),
                confirm: true,
                expected_head: advanced_test_head(&repository),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("reword current commit without consuming the index")
        .0;

        assert!(reworded.success);
        assert_eq!(
            advanced_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "renamed subject"
        );
        assert_eq!(
            advanced_test_output(&repository, ["rev-parse", "HEAD^{tree}"]).trim(),
            tree_before_reword,
            "a reword must not alter HEAD's tree"
        );
        assert_eq!(
            advanced_test_output(&repository, ["diff", "--cached", "--name-only"]).trim(),
            "next-change.txt",
            "a separately staged next change must stay staged"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_create_fixup_commit_uses_the_selected_immutable_commit() {
        let root = advanced_test_root("create-fixup-commit");
        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "README.md", "base\n", "base subject");
        let selected = advanced_test_head(&repository);
        advanced_test_commit(
            &repository,
            "README.md",
            "current\n",
            "current branch commit",
        );
        let expected_head = advanced_test_head(&repository);
        let state = advanced_test_state(&root).await;

        std::fs::write(repository.join("fixup.txt"), "staged fixup\n")
            .expect("write staged fixup");
        advanced_test_command(&repository, ["add", "fixup.txt"]);
        let created = advanced_create_fixup_commit(
            State(state.clone()),
            Json(AdvancedCreateFixupCommitBody {
                scope: advanced_test_scope(&repository),
                target: selected.clone(),
                confirm: true,
                expected_head: expected_head.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("create selected-commit fixup")
        .0;
        assert!(created.success);
        assert_eq!(
            advanced_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "fixup! base subject"
        );
        assert_eq!(
            advanced_test_output(&repository, ["rev-parse", "HEAD^"]).trim(),
            expected_head
        );
        assert_eq!(
            advanced_test_output(&repository, ["show", "HEAD:fixup.txt"]).trim(),
            "staged fixup"
        );
        assert!(
            advanced_test_output(&repository, ["diff", "--cached", "--name-only"])
                .trim()
                .is_empty(),
            "the fixup must consume the reviewed staged index"
        );

        let no_staged_change = advanced_create_fixup_commit(
            State(state.clone()),
            Json(AdvancedCreateFixupCommitBody {
                scope: advanced_test_scope(&repository),
                target: selected.clone(),
                confirm: true,
                expected_head: advanced_test_head(&repository),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect_err("fixup requires an explicitly staged change");
        assert_eq!(no_staged_change.status, StatusCode::BAD_REQUEST);

        let stale_head = advanced_test_head(&repository);
        advanced_test_commit(&repository, "later.txt", "later\n", "external advance");
        let stale_target = advanced_create_fixup_commit(
            State(state.clone()),
            Json(AdvancedCreateFixupCommitBody {
                scope: advanced_test_scope(&repository),
                target: selected,
                confirm: true,
                expected_head: stale_head,
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect_err("fixup rejects a stale target snapshot");
        assert_eq!(stale_target.status, StatusCode::CONFLICT);

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_amend_and_reword_reject_same_oid_branch_swaps() {
        let root = advanced_test_root("amend-reword-target-snapshot");
        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "README.md", "initial\n", "initial subject");
        let shared_head = advanced_test_head(&repository);
        advanced_test_command(&repository, ["branch", "release"]);
        advanced_test_command(&repository, ["checkout", "release"]);
        std::fs::write(repository.join("README.md"), "staged on release\n")
            .expect("stage a release-only amendment");
        advanced_test_command(&repository, ["add", "README.md"]);
        let state = advanced_test_state(&root).await;

        // `main` and `release` deliberately share this object ID. A HEAD-only
        // guard would amend/reword release after a confirmation prepared on
        // main, rewriting the wrong branch.
        let amend = advanced_amend_head(
            State(state.clone()),
            Json(AdvancedAmendHeadBody {
                scope: advanced_test_scope(&repository),
                message: Some("must not amend release".to_string()),
                confirm: true,
                expected_head: shared_head.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect_err("amend rejects a same-OID checkout on another branch");
        assert_eq!(amend.status, StatusCode::CONFLICT);
        assert_eq!(advanced_test_head(&repository), shared_head);

        let reword = advanced_reword_head(
            State(state.clone()),
            Json(AdvancedRewordHeadBody {
                scope: advanced_test_scope(&repository),
                message: "must not reword release".to_string(),
                confirm: true,
                expected_head: shared_head.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect_err("reword rejects a same-OID checkout on another branch");
        assert_eq!(reword.status, StatusCode::CONFLICT);
        assert_eq!(advanced_test_head(&repository), shared_head);
        assert_eq!(
            advanced_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "initial subject"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_detached_checkout_force_delete_and_enhanced_history_use_real_git() {
        let root = advanced_test_root("detached-history");
        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "base.txt", "base\n", "base");
        let base = advanced_test_head(&repository);

        advanced_test_command(&repository, ["checkout", "-b", "feature"]);
        advanced_test_commit(
            &repository,
            "feature.txt",
            "feature\n",
            "feature literal a.b",
        );
        let feature_head = advanced_test_head(&repository);
        advanced_test_command(&repository, ["tag", "-a", "feature-tag", "-m", "feature tag"]);

        advanced_test_command(&repository, ["checkout", "main"]);
        advanced_test_commit(
            &repository,
            "main.txt",
            "main\n",
            "main literal axb",
        );
        let main_head = advanced_test_head(&repository);
        advanced_test_command(&repository, ["checkout", "-b", "discard-me", &base]);
        advanced_test_commit(
            &repository,
            "discard.txt",
            "discard\n",
            "discard this unmerged branch",
        );
        advanced_test_command(&repository, ["checkout", "main"]);
        let state = advanced_test_state(&root).await;

        // Keep the existing branch/tag pickers covered too. `for-each-ref`
        // has a different formatting language from `git log`; this caught a
        // real separator bug while adding graph ref metadata.
        let branch_list = branches(
            State(state.clone()),
            Query(ProjectQuery {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
            }),
        )
        .await
        .expect("list branches")
        .0;
        assert!(branch_list.local_branches.iter().any(|value| value == "main"));
        assert!(branch_list.local_branches.iter().any(|value| value == "feature"));
        let tag_list = tags(
            State(state.clone()),
            Query(ProjectQuery {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
            }),
        )
        .await
        .expect("list tags")
        .0;
        assert!(tag_list.tags.iter().any(|tag| tag.name == "feature-tag"));

        let first_page = advanced_enhanced_history(
            State(state.clone()),
            Query(AdvancedEnhancedHistoryQuery {
                scope: advanced_test_scope(&repository),
                all_branches: Some(true),
                search: Some("literal a.b".to_string()),
                offset: Some(0),
                limit: Some(1),
            }),
        )
        .await
        .expect("fetch first history page")
        .0;
        // Fixed-string search must not treat the dot as a regular-expression
        // wildcard, so the similarly named `axb` commit is excluded.
        assert_eq!(first_page.commits.len(), 1);
        assert_eq!(first_page.commits[0].hash, feature_head);
        assert!(!first_page.has_more);
        assert_eq!(first_page.next_offset, None);
        assert!(first_page.commits[0].stats.contains("file changed"));
        assert_eq!(first_page.commits[0].parents, vec![base.clone()]);
        assert!(
            first_page.commits[0].refs.iter().any(|value| value == "feature"),
            "feature ref metadata was missing: {:?}",
            first_page.commits[0].refs
        );
        assert!(
            first_page.commits[0]
                .refs
                .iter()
                .any(|value| value == "feature-tag")
        );

        let all_history_first_page = advanced_enhanced_history(
            State(state.clone()),
            Query(AdvancedEnhancedHistoryQuery {
                scope: advanced_test_scope(&repository),
                all_branches: Some(true),
                search: None,
                offset: Some(0),
                limit: Some(1),
            }),
        )
        .await
        .expect("page all history")
        .0;
        assert_eq!(all_history_first_page.commits.len(), 1);
        assert!(all_history_first_page.has_more);
        assert_eq!(all_history_first_page.next_offset, Some(1));
        let all_history_second_page = advanced_enhanced_history(
            State(state.clone()),
            Query(AdvancedEnhancedHistoryQuery {
                scope: advanced_test_scope(&repository),
                all_branches: Some(true),
                search: None,
                offset: all_history_first_page.next_offset,
                limit: Some(1),
            }),
        )
        .await
        .expect("page all history again")
        .0;
        assert_eq!(all_history_second_page.commits.len(), 1);
        assert_ne!(
            all_history_first_page.commits[0].hash,
            all_history_second_page.commits[0].hash
        );

        let current_branch_only = advanced_enhanced_history(
            State(state.clone()),
            Query(AdvancedEnhancedHistoryQuery {
                scope: advanced_test_scope(&repository),
                all_branches: Some(false),
                search: Some("literal".to_string()),
                offset: None,
                limit: Some(10),
            }),
        )
        .await
        .expect("fetch current branch history")
        .0;
        assert_eq!(current_branch_only.commits.len(), 1);
        assert_eq!(current_branch_only.commits[0].hash, main_head);
        assert!(
            current_branch_only.commits[0]
                .refs
                .iter()
                .any(|value| value == "HEAD -> main")
        );

        let unconfirmed_detach = advanced_detached_checkout(
            State(state.clone()),
            Json(AdvancedDetachedCheckoutBody {
                scope: advanced_test_scope(&repository),
                reference: feature_head.clone(),
                expected_head: main_head.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
                confirm: false,
            }),
        )
        .await;
        assert!(unconfirmed_detach.is_err());
        let stale_detach = advanced_detached_checkout(
            State(state.clone()),
            Json(AdvancedDetachedCheckoutBody {
                scope: advanced_test_scope(&repository),
                reference: feature_head.clone(),
                expected_head: base.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
                confirm: true,
            }),
        )
        .await;
        assert!(stale_detach.is_err());
        std::fs::write(repository.join("uncommitted.txt"), "do not discard\n")
            .expect("make worktree dirty");
        let dirty_detach = advanced_detached_checkout(
            State(state.clone()),
            Json(AdvancedDetachedCheckoutBody {
                scope: advanced_test_scope(&repository),
                reference: feature_head.clone(),
                expected_head: main_head.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
                confirm: true,
            }),
        )
        .await;
        assert!(dirty_detach.is_err());
        std::fs::remove_file(repository.join("uncommitted.txt")).expect("restore clean worktree");
        let detached = advanced_detached_checkout(
            State(state.clone()),
            Json(AdvancedDetachedCheckoutBody {
                scope: advanced_test_scope(&repository),
                reference: feature_head.clone(),
                expected_head: main_head.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
                confirm: true,
            }),
        )
        .await
        .expect("detach at selected commit")
        .0;
        assert!(detached.success);
        assert_eq!(advanced_test_head(&repository), feature_head);
        assert_eq!(current_branch(&repository).await.expect("detached branch"), "HEAD");

        advanced_test_command(&repository, ["checkout", "main"]);
        let unconfirmed_delete = advanced_force_delete_local_branch(
            State(state.clone()),
            Json(AdvancedForceDeleteLocalBranchBody {
                scope: advanced_test_scope(&repository),
                branch: "discard-me".to_string(),
                confirm: false,
            }),
        )
        .await;
        assert!(unconfirmed_delete.is_err());
        let deleted = advanced_force_delete_local_branch(
            State(state.clone()),
            Json(AdvancedForceDeleteLocalBranchBody {
                scope: advanced_test_scope(&repository),
                branch: "discard-me".to_string(),
                confirm: true,
            }),
        )
        .await
        .expect("force-delete unmerged local branch")
        .0;
        assert!(deleted.success);
        assert!(git(&repository, ["show-ref", "--verify", "refs/heads/discard-me"])
            .await
            .is_err());
        let current_branch_delete = advanced_force_delete_local_branch(
            State(state),
            Json(AdvancedForceDeleteLocalBranchBody {
                scope: advanced_test_scope(&repository),
                branch: "main".to_string(),
                confirm: true,
            }),
        )
        .await;
        assert!(current_branch_delete.is_err());

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_history_detached_checkout_and_force_delete_are_wired_over_http() {
        let root = advanced_test_root("wire-contract");
        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "base.txt", "base\n", "base");
        let main_head = advanced_test_head(&repository);
        advanced_test_command(&repository, ["checkout", "-b", "wire-discard"]);
        advanced_test_commit(
            &repository,
            "discard.txt",
            "discard\n",
            "discard this branch through the HTTP route",
        );
        let discard_head = advanced_test_head(&repository);
        advanced_test_command(&repository, ["checkout", "main"]);
        let state = advanced_test_state(&root).await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test HTTP server");
        let address = listener.local_addr().expect("test HTTP address");
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router().with_state(server_state)).await;
        });
        let client = reqwest::Client::new();
        let project_path = repository.display().to_string();

        let history = client
            .get(format!("http://{address}/api/git/history/enhanced"))
            .query(&[
                ("projectPath", project_path.as_str()),
                ("allBranches", "true"),
                ("limit", "1"),
            ])
            .send()
            .await
            .expect("request enhanced history");
        assert!(history.status().is_success());
        let history = history
            .json::<serde_json::Value>()
            .await
            .expect("decode enhanced history");
        assert_eq!(history["limit"], 1);
        assert_eq!(history["allBranches"], true);
        assert!(history["commits"][0]["shortHash"].is_string());
        assert!(history["commits"][0]["parents"].is_array());
        assert!(history["commits"][0]["refs"].is_array());

        let detached = client
            .post(format!("http://{address}/api/git/checkout/detached"))
            .json(&serde_json::json!({
                "projectPath": project_path,
                "ref": discard_head,
                "expectedHead": main_head,
                "expectedTargetRef": "refs/heads/main",
                "confirm": true,
            }))
            .send()
            .await
            .expect("request detached checkout");
        assert!(detached.status().is_success());
        assert!(
            detached
                .json::<serde_json::Value>()
                .await
                .expect("decode detached checkout response")["success"]
                .as_bool()
                .unwrap_or(false)
        );
        assert_eq!(advanced_test_head(&repository), discard_head);

        let force_delete = client
            .post(format!("http://{address}/api/git/branch/delete-force"))
            .json(&serde_json::json!({
                "projectPath": repository.display().to_string(),
                "branch": "wire-discard",
                "confirm": true,
            }))
            .send()
            .await
            .expect("request force branch deletion");
        assert!(force_delete.status().is_success());
        assert!(
            force_delete
                .json::<serde_json::Value>()
                .await
                .expect("decode force branch deletion response")["success"]
                .as_bool()
                .unwrap_or(false)
        );
        assert!(git(&repository, ["show-ref", "--verify", "refs/heads/wire-discard"])
            .await
            .is_err());

        server.abort();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_sparse_bisect_maintenance_and_gc_use_real_git() {
        let root = advanced_test_root("sparse-bisect-maintenance");
        let repository = advanced_test_repository(&root);
        std::fs::create_dir_all(repository.join("app")).expect("create app directory");
        std::fs::create_dir_all(repository.join("docs")).expect("create docs directory");
        advanced_test_commit(&repository, "app/main.txt", "good\n", "good");
        let good = advanced_test_head(&repository);
        advanced_test_commit(&repository, "docs/readme.txt", "bad\n", "bad");
        let bad = advanced_test_head(&repository);
        let state = advanced_test_state(&root).await;

        let sparse = advanced_set_sparse_checkout(
            State(state.clone()),
            Json(AdvancedSetSparseCheckoutBody {
                scope: advanced_test_scope(&repository),
                patterns: vec!["app".to_string()],
                cone_mode: Some(true),
                confirm: true,
            }),
        )
        .await
        .expect("enable cone sparse checkout")
        .0;
        assert!(sparse.enabled);
        assert_eq!(sparse.cone_mode, Some(true));
        assert!(repository.join("app/main.txt").exists());
        let sparse_disabled = advanced_disable_sparse_checkout(
            State(state.clone()),
            Json(AdvancedDisableSparseCheckoutBody {
                scope: advanced_test_scope(&repository),
                confirm: true,
            }),
        )
        .await
        .expect("disable sparse checkout")
        .0;
        assert!(!sparse_disabled.enabled);
        assert!(repository.join("docs/readme.txt").exists());

        let bisect = advanced_git_bisect_start(
            State(state.clone()),
            Json(AdvancedBisectStartBody {
                scope: advanced_test_scope(&repository),
                bad: bad.clone(),
                good: vec![good],
                expected_head: bad.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
                confirm: true,
            }),
        )
        .await
        .expect("start bisect")
        .0;
        assert!(bisect.in_progress);
        assert!(bisect.bad_commits.iter().any(|commit| commit == &bad));
        let reset = advanced_git_bisect_reset(
            State(state.clone()),
            Json(AdvancedBisectResetBody {
                scope: advanced_test_scope(&repository),
                confirm: true,
            }),
        )
        .await
        .expect("reset bisect")
        .0;
        assert!(!reset.in_progress);
        assert_eq!(advanced_test_head(&repository), bad);

        let maintenance = advanced_git_maintenance(
            State(state.clone()),
            Json(AdvancedMaintenanceBody {
                scope: advanced_test_scope(&repository),
                tasks: vec!["commit-graph".to_string()],
                confirm: true,
            }),
        )
        .await
        .expect("run maintenance")
        .0;
        assert!(maintenance.success);
        let gc = advanced_git_gc(
            State(state.clone()),
            Json(AdvancedGitGcBody {
                scope: advanced_test_scope(&repository),
                aggressive: false,
                prune_now: false,
                confirm: true,
                confirm_prune_now: false,
            }),
        )
        .await
        .expect("run Git gc")
        .0;
        assert!(gc.success);

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_lfs_status_is_safe_with_or_without_a_local_lfs_installation() {
        let root = advanced_test_root("lfs");
        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "README.md", "LFS test\n", "initial");

        let status = advanced_git_lfs_response(&repository)
            .await
            .expect("read LFS status");
        if status.available {
            advanced_require_git_lfs(&repository)
                .await
                .expect("LFS is available");
            let pattern = advanced_validate_lfs_pattern("*.bin").expect("safe LFS pattern");
            git(&repository, ["lfs", "install", "--local"])
                .await
                .expect("install LFS in repository");
            git(&repository, ["lfs", "track", pattern.as_str()])
                .await
                .expect("track LFS pattern");
            let tracked = advanced_git_lfs_response(&repository)
                .await
                .expect("read tracked LFS patterns");
            assert!(
                tracked
                    .tracked_patterns
                    .iter()
                    .any(|value| value == "*.bin")
            );
            git(&repository, ["lfs", "untrack", pattern.as_str()])
                .await
                .expect("untrack LFS pattern");
        } else {
            assert!(advanced_require_git_lfs(&repository).await.is_err());
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_structured_rebase_reorders_rewords_and_squashes_with_real_git() {
        let root = advanced_test_root("rebase");
        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "base.txt", "base\n", "base");
        let upstream = advanced_test_head(&repository);
        advanced_test_command(&repository, ["checkout", "-b", "feature"]);
        advanced_test_commit(&repository, "a.txt", "a\n", "A");
        let commit_a = advanced_test_head(&repository);
        advanced_test_commit(&repository, "b.txt", "b\n", "B");
        let commit_b = advanced_test_head(&repository);
        advanced_test_commit(&repository, "c.txt", "c\n", "C");
        let commit_c = advanced_test_head(&repository);
        let state = advanced_test_state(&root).await;

        let preview = advanced_rebase_plan_preview(
            State(state.clone()),
            Query(AdvancedRebasePlanQuery {
                scope: advanced_test_scope(&repository),
                upstream: upstream.clone(),
            }),
        )
        .await
        .expect("rebase plan preview")
        .0;
        assert_eq!(preview.commits.len(), 3);
        assert_eq!(preview.expected_head, commit_c);
        assert_eq!(preview.expected_target_ref, "refs/heads/feature");

        let response = advanced_apply_rebase_plan(
            State(state.clone()),
            Json(AdvancedRebasePlanBody {
                scope: advanced_test_scope(&repository),
                upstream,
                expected_head: preview.expected_head,
                expected_target_ref: preview.expected_target_ref,
                steps: vec![
                    AdvancedRebasePlanStepInput {
                        commit: commit_c,
                        action: "pick".to_string(),
                        message: None,
                    },
                    AdvancedRebasePlanStepInput {
                        commit: commit_a,
                        action: "reword".to_string(),
                        message: Some("A reworded".to_string()),
                    },
                    AdvancedRebasePlanStepInput {
                        commit: commit_b,
                        action: "squash".to_string(),
                        message: None,
                    },
                ],
                confirm: true,
            }),
        )
        .await
        .expect("apply structured rebase plan")
        .0;
        assert!(response.success);
        assert!(
            !response
                .operation_state
                .expect("rebase operation state")
                .in_progress
        );
        assert!(repository.join("a.txt").exists());
        assert!(repository.join("b.txt").exists());
        assert!(repository.join("c.txt").exists());
        let subjects = advanced_test_output(&repository, ["log", "--format=%s", "-2"]);
        assert_eq!(subjects.lines().next(), Some("A reworded"));
        assert!(subjects.lines().any(|subject| subject == "C"));

        let invalid = advanced_validate_rebase_plan(
            &repository,
            &AdvancedRebasePlanBody {
                scope: advanced_test_scope(&repository),
                upstream: advanced_test_output(&repository, ["rev-parse", "HEAD~2"])
                    .trim()
                    .to_string(),
                expected_head: advanced_test_head(&repository),
                expected_target_ref: "refs/heads/feature".to_string(),
                steps: vec![
                    AdvancedRebasePlanStepInput {
                        commit: advanced_test_output(&repository, ["rev-parse", "HEAD~1"])
                            .trim()
                            .to_string(),
                        action: "squash".to_string(),
                        message: None,
                    },
                    AdvancedRebasePlanStepInput {
                        commit: advanced_test_head(&repository),
                        action: "pick".to_string(),
                        message: None,
                    },
                ],
                confirm: true,
            },
        )
        .await;
        assert!(invalid.is_err());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_structured_rebase_edit_pauses_for_stage_amend_continue_and_abort() {
        let root = advanced_test_root("rebase-edit");
        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "base.txt", "base\n", "base");
        advanced_test_command(&repository, ["checkout", "-b", "feature"]);
        advanced_test_commit(&repository, "editable.txt", "original\n", "editable commit");
        let editable_commit = advanced_test_head(&repository);
        advanced_test_commit(&repository, "tail.txt", "tail\n", "tail commit");
        let original_feature_head = advanced_test_head(&repository);
        advanced_test_command(&repository, ["checkout", "main"]);
        advanced_test_commit(&repository, "upstream.txt", "upstream\n", "upstream commit");
        let upstream = advanced_test_head(&repository);
        advanced_test_command(&repository, ["checkout", "feature"]);
        let state = advanced_test_state(&root).await;

        async fn apply_edit_plan(
            state: &AppState,
            repository: &std::path::Path,
            upstream: &str,
            editable_commit: &str,
        ) -> GitOperationResponse {
            let preview = advanced_rebase_plan_preview(
                State(state.clone()),
                Query(AdvancedRebasePlanQuery {
                    scope: advanced_test_scope(repository),
                    upstream: upstream.to_string(),
                }),
            )
            .await
            .expect("editable rebase preview")
            .0;
            assert_eq!(preview.commits.len(), 2);
            advanced_apply_rebase_plan(
                State(state.clone()),
                Json(AdvancedRebasePlanBody {
                    scope: advanced_test_scope(repository),
                    upstream: preview.upstream,
                    expected_head: preview.expected_head,
                    expected_target_ref: preview.expected_target_ref,
                    steps: preview
                        .commits
                        .into_iter()
                        .map(|commit| AdvancedRebasePlanStepInput {
                            action: if commit.commit == editable_commit {
                                "edit".to_string()
                            } else {
                                "pick".to_string()
                            },
                            commit: commit.commit,
                            message: None,
                        })
                        .collect(),
                    confirm: true,
                }),
            )
            .await
            .expect("editable rebase pause")
            .0
        }

        let paused = apply_edit_plan(&state, &repository, &upstream, &editable_commit).await;
        let paused_state = paused.operation_state.expect("edit rebase state");
        assert!(paused.success);
        assert!(paused_state.in_progress);
        assert_eq!(paused_state.operation, "rebase");
        assert!(paused_state.is_edit_stop, "edit must not masquerade as a conflict");
        assert!(paused_state.can_continue);
        assert!(paused_state.can_abort);
        assert!(
            !paused_state.can_skip,
            "Skip at a Git edit stop advances an already-applied commit, so the native UI must not advertise its generic skip wording"
        );

        // Git deliberately detaches HEAD while an interactive rebase is
        // paused. The stage/amend snapshot therefore uses the explicit HEAD
        // target returned by status, not the branch being rebuilt.
        std::fs::write(repository.join("edit-note.txt"), "included in edited commit\n")
            .expect("write amendment");
        stage(
            State(state.clone()),
            Json(FileBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                file: "edit-note.txt".to_string(),
            }),
        )
        .await
        .expect("stage is allowed only at the edit stop");
        let edited_head = advanced_test_head(&repository);
        let amended = advanced_amend_head(
            State(state.clone()),
            Json(AdvancedAmendHeadBody {
                scope: advanced_test_scope(&repository),
                message: Some("edited commit".to_string()),
                confirm: true,
                expected_head: edited_head,
                expected_target_ref: "HEAD".to_string(),
            }),
        )
        .await
        .expect("amend is allowed only at the edit stop")
        .0;
        assert!(
            amended
                .operation_state
                .as_ref()
                .is_some_and(|operation| operation.is_edit_stop),
            "amending must leave the user at the same explicit edit stop until Continue"
        );

        // Abort must discard the staged amendment and restore the exact
        // branch HEAD that was reviewed before the rebase started.
        let aborted = abort_operation(
            State(state.clone()),
            Json(ProjectBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                allow_workspace_init: false,
            }),
        )
        .await
        .expect("abort edit stop")
        .0;
        assert!(
            !aborted
                .operation_state
                .expect("aborted edit operation state")
                .in_progress
        );
        assert_eq!(advanced_test_head(&repository), original_feature_head);
        assert!(!repository.join("edit-note.txt").exists());

        let paused_again = apply_edit_plan(&state, &repository, &upstream, &editable_commit).await;
        assert!(
            paused_again
                .operation_state
                .as_ref()
                .is_some_and(|operation| operation.is_edit_stop)
        );
        std::fs::write(repository.join("continued-note.txt"), "kept\n")
            .expect("write continued amendment");
        stage(
            State(state.clone()),
            Json(FileBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                file: "continued-note.txt".to_string(),
            }),
        )
        .await
        .expect("stage continued amendment");
        let continued_head = advanced_test_head(&repository);
        advanced_amend_head(
            State(state.clone()),
            Json(AdvancedAmendHeadBody {
                scope: advanced_test_scope(&repository),
                message: Some("continued edit".to_string()),
                confirm: true,
                expected_head: continued_head,
                expected_target_ref: "HEAD".to_string(),
            }),
        )
        .await
        .expect("amend continued edit");
        let completed = continue_operation(
            State(state.clone()),
            Json(ProjectBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                allow_workspace_init: false,
            }),
        )
        .await
        .expect("continue edit stop")
        .0;
        assert!(
            !completed
                .operation_state
                .expect("completed rebase state")
                .in_progress
        );
        assert_eq!(
            advanced_test_output(&repository, ["symbolic-ref", "--short", "HEAD"]).trim(),
            "feature"
        );
        assert!(advanced_test_output(&repository, ["show", "HEAD:continued-note.txt"]).contains("kept"));
        assert_eq!(
            advanced_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "tail commit"
        );

        advanced_cleanup_orphaned_rebase_artifacts(&repository)
            .await
            .expect("clean completed rebase artifacts");
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn advanced_structured_rebase_arguments_keep_a_reviewed_plan_stable() {
        assert_eq!(
            advanced_structured_rebase_args("refs/heads/main"),
            vec![
                "rebase",
                "--no-autostash",
                "--no-autosquash",
                "--keep-empty",
                "--interactive",
                "refs/heads/main",
            ]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_structured_rebase_preserves_an_intentional_empty_commit() {
        let root = advanced_test_root("rebase-empty");
        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "base.txt", "base\n", "base");
        let upstream = advanced_test_head(&repository);
        advanced_test_command(&repository, ["checkout", "-b", "feature"]);
        advanced_test_command(
            &repository,
            ["commit", "--allow-empty", "-m", "intentional empty"],
        );
        let empty_commit = advanced_test_head(&repository);
        advanced_test_commit(&repository, "tail.txt", "tail\n", "tail");
        let tail_commit = advanced_test_head(&repository);
        // This local preference must not be allowed to transform the
        // already-reviewed native todo. The command arguments above opt out
        // explicitly, as LazyGit does for its managed rebase workflow.
        advanced_test_command(
            &repository,
            ["config", "--local", "rebase.autoSquash", "true"],
        );
        let state = advanced_test_state(&root).await;

        let preview = advanced_rebase_plan_preview(
            State(state.clone()),
            Query(AdvancedRebasePlanQuery {
                scope: advanced_test_scope(&repository),
                upstream: upstream.clone(),
            }),
        )
        .await
        .expect("empty-commit rebase plan preview")
        .0;
        assert_eq!(
            preview
                .commits
                .iter()
                .map(|commit| commit.commit.as_str())
                .collect::<Vec<_>>(),
            vec![empty_commit.as_str(), tail_commit.as_str()]
        );

        let response = advanced_apply_rebase_plan(
            State(state.clone()),
            Json(AdvancedRebasePlanBody {
                scope: advanced_test_scope(&repository),
                upstream,
                expected_head: preview.expected_head,
                expected_target_ref: preview.expected_target_ref,
                steps: vec![
                    AdvancedRebasePlanStepInput {
                        commit: empty_commit,
                        action: "pick".to_string(),
                        message: None,
                    },
                    AdvancedRebasePlanStepInput {
                        commit: tail_commit,
                        action: "pick".to_string(),
                        message: None,
                    },
                ],
                confirm: true,
            }),
        )
        .await
        .expect("apply empty-commit rebase plan")
        .0;
        assert!(response.success);
        let subjects = advanced_test_output(&repository, ["log", "--format=%s", "-2"]);
        assert_eq!(
            subjects.lines().collect::<Vec<_>>(),
            vec!["tail", "intentional empty"]
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_structured_rebase_rejects_a_same_oid_branch_swap_after_preview() {
        let root = advanced_test_root("rebase-target-snapshot");
        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "base.txt", "base\n", "base");
        advanced_test_command(&repository, ["checkout", "-b", "feature"]);
        advanced_test_commit(&repository, "feature.txt", "feature\n", "feature commit");
        let feature_head = advanced_test_head(&repository);
        advanced_test_command(&repository, ["checkout", "main"]);
        advanced_test_commit(&repository, "main.txt", "main\n", "main commit");
        let main_head = advanced_test_head(&repository);
        advanced_test_command(&repository, ["checkout", "feature"]);

        let state = advanced_test_state(&root).await;
        let preview = advanced_rebase_plan_preview(
            State(state.clone()),
            Query(AdvancedRebasePlanQuery {
                scope: advanced_test_scope(&repository),
                upstream: main_head,
            }),
        )
        .await
        .expect("feature rebase preview")
        .0;
        assert_eq!(preview.expected_head, feature_head);
        assert_eq!(preview.expected_target_ref, "refs/heads/feature");

        // `release` deliberately points at the same commit as `feature`.
        // A HEAD-only check would pass here and rewrite the wrong branch.
        advanced_test_command(&repository, ["branch", "release"]);
        advanced_test_command(&repository, ["checkout", "release"]);
        let rejected = advanced_apply_rebase_plan(
            State(state.clone()),
            Json(AdvancedRebasePlanBody {
                scope: advanced_test_scope(&repository),
                upstream: preview.upstream,
                expected_head: preview.expected_head,
                expected_target_ref: preview.expected_target_ref,
                steps: preview
                    .commits
                    .into_iter()
                    .map(|commit| AdvancedRebasePlanStepInput {
                        commit: commit.commit,
                        action: "pick".to_string(),
                        message: None,
                    })
                    .collect(),
                confirm: true,
            }),
        )
        .await
        .expect_err("a preview prepared on feature must not rewrite release");
        assert_eq!(rejected.status, StatusCode::CONFLICT);
        assert_eq!(
            advanced_test_output(&repository, ["symbolic-ref", "--short", "HEAD"]).trim(),
            "release"
        );
        assert_eq!(advanced_test_head(&repository), feature_head);

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_structured_rebase_conflict_uses_existing_abort_recovery() {
        let root = advanced_test_root("rebase-conflict-recovery");
        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "conflict.txt", "base\n", "base");
        advanced_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("conflict.txt"), "feature\n").expect("feature change");
        advanced_test_command(&repository, ["commit", "-am", "feature change"]);
        let feature_head = advanced_test_head(&repository);
        advanced_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("conflict.txt"), "main\n").expect("main change");
        advanced_test_command(&repository, ["commit", "-am", "main change"]);
        let main_head = advanced_test_head(&repository);
        advanced_test_command(&repository, ["checkout", "feature"]);
        let state = advanced_test_state(&root).await;

        let preview = advanced_rebase_plan_preview(
            State(state.clone()),
            Query(AdvancedRebasePlanQuery {
                scope: advanced_test_scope(&repository),
                upstream: main_head,
            }),
        )
        .await
        .expect("conflicting rebase plan preview")
        .0;
        let response = advanced_apply_rebase_plan(
            State(state.clone()),
            Json(AdvancedRebasePlanBody {
                scope: advanced_test_scope(&repository),
                upstream: preview.upstream,
                expected_head: preview.expected_head,
                expected_target_ref: preview.expected_target_ref,
                steps: vec![AdvancedRebasePlanStepInput {
                    commit: feature_head.clone(),
                    action: "pick".to_string(),
                    message: None,
                }],
                confirm: true,
            }),
        )
        .await
        .expect("conflict is an operation-state response")
        .0;
        assert!(response.success);
        let conflict_state = response.operation_state.expect("rebase operation state");
        assert!(conflict_state.in_progress);
        assert!(
            !conflict_state.is_edit_stop,
            "a normal rebase conflict must retain its conflict-only recovery policy"
        );
        let aborted = abort_operation(
            State(state.clone()),
            Json(ProjectBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                allow_workspace_init: false,
            }),
        )
        .await
        .expect("abort conflicting structured rebase")
        .0;
        assert!(
            !aborted
                .operation_state
                .expect("aborted operation state")
                .in_progress
        );
        assert_eq!(advanced_test_head(&repository), feature_head);
        assert_eq!(
            std::fs::read_to_string(repository.join("conflict.txt"))
                .expect("restored feature file"),
            "feature\n"
        );
        advanced_cleanup_orphaned_rebase_artifacts(&repository)
            .await
            .expect("clean completed rebase artifacts");

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_add_and_remove_submodule_use_safe_real_git_commands() {
        let root = advanced_test_root("submodule");
        let source = root.join("source");
        std::fs::create_dir_all(&source).expect("create submodule source");
        advanced_test_command(&source, ["init", "--initial-branch=main"]);
        advanced_test_command(&source, ["config", "user.name", "Submodule Source"]);
        advanced_test_command(&source, ["config", "user.email", "source@example.test"]);
        advanced_test_commit(&source, "README.md", "source\n", "source initial");
        let (daemon, port) = advanced_start_test_git_daemon(&root);

        let repository = advanced_test_repository(&root);
        advanced_test_commit(&repository, "README.md", "parent\n", "parent initial");
        let state = advanced_test_state(&root).await;
        let added = advanced_add_submodule(
            State(state.clone()),
            Json(AdvancedSubmoduleAddBody {
                scope: advanced_test_scope(&repository),
                url: format!("git://127.0.0.1:{port}/source"),
                path: "modules/source".to_string(),
                branch: Some("main".to_string()),
                confirm: true,
            }),
        )
        .await
        .expect("add local test submodule")
        .0;
        assert!(added.success);
        assert!(repository.join("modules/source/README.md").exists());
        assert!(
            std::fs::read_to_string(repository.join(".gitmodules"))
                .expect("read .gitmodules")
                .contains("modules/source")
        );

        let removed = advanced_remove_submodule(
            State(state.clone()),
            Json(AdvancedSubmoduleRemoveBody {
                scope: advanced_test_scope(&repository),
                path: "modules/source".to_string(),
                confirm: true,
                purge_metadata: false,
                confirm_purge_metadata: false,
            }),
        )
        .await
        .expect("remove submodule")
        .0;
        assert!(removed.success);
        assert!(!repository.join("modules/source").exists());
        assert!(
            !std::fs::read_to_string(repository.join(".gitmodules"))
                .unwrap_or_default()
                .contains("modules/source")
        );

        drop(state);
        drop(daemon);
        let _ = std::fs::remove_dir_all(root);
    }
}

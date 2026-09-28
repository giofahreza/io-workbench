// Advanced Git management endpoints.
//
// Keep these request/response types private to the server module for now.  A
// client only reaches an endpoint once it has been deliberately wired into the
// router, which lets the mobile rollout remain incremental while preserving
// strict validation at the server boundary.

const MANAGEMENT_OUTPUT_CHARACTER_LIMIT: usize = 500_000;
const MANAGEMENT_MAX_HISTORY_LIMIT: usize = 200;
const MANAGEMENT_MAX_BLAME_LINES: usize = 2_000;
// Selected stash restoration deliberately has a much smaller bound than a
// whole-stash apply.  A native client must name every path it wants to
// overwrite, rather than accidentally turning an empty or huge selection
// into a broad worktree mutation.
const MANAGEMENT_MAX_STASH_RESTORE_FILES: usize = 100;
const MANAGEMENT_MAX_STASH_HUNK_SELECTIONS: usize = 100;
const MANAGEMENT_MAX_STASH_INVENTORY_FILES: usize = 10_000;
const MANAGEMENT_STASH_RECOVERY_VERSION: u8 = 1;
const MANAGEMENT_STASH_RECOVERY_MARKER: &str = "iowb-stash-recovery.json";
const MANAGEMENT_STASH_UNSTAGED_RECOVERY_VERSION: u8 = 1;
const MANAGEMENT_STASH_UNSTAGED_RECOVERY_MARKER: &str = "iowb-stash-unstaged-recovery.json";
const MANAGEMENT_MAX_STASH_RECOVERY_MARKER_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementProjectScope {
    project: Option<String>,
    project_path: Option<String>,
    repository_id: Option<String>,
}

impl ManagementProjectScope {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

async fn management_repository(
    state: &AppState,
    scope: &ManagementProjectScope,
) -> Result<PathBuf> {
    let repository =
        resolve_git_repository_path(state, scope.project_ref()?, scope.repository_id.as_deref())
            .await?;
    validate_git_repository(&repository).await?;
    Ok(repository)
}

fn require_management_confirmation(confirmed: bool, action: &str) -> Result<()> {
    if confirmed {
        return Ok(());
    }
    Err(ServerError::new(
        StatusCode::BAD_REQUEST,
        format!("Confirmation is required to {action}"),
    ))
}

fn management_default_true() -> bool {
    true
}

fn management_output_or(output: &GitOutput, fallback: &str) -> String {
    join_output_or(output, fallback)
}

fn management_truncate_output(value: String) -> (String, bool) {
    let is_truncated = value.chars().count() > MANAGEMENT_OUTPUT_CHARACTER_LIMIT;
    if is_truncated {
        (
            format!(
                "{}\n\n... Output truncated to keep the UI responsive ...",
                value
                    .chars()
                    .take(MANAGEMENT_OUTPUT_CHARACTER_LIMIT)
                    .collect::<String>()
            ),
            true,
        )
    } else {
        (value, false)
    }
}

async fn management_remote_exists(repository: &Path, remote: &str) -> Result<()> {
    let remote = validate_remote_name(remote)?;
    let remotes = git(repository, ["remote"]).await?.stdout;
    if remotes.lines().any(|line| line.trim() == remote) {
        return Ok(());
    }
    Err(ServerError::new(
        StatusCode::NOT_FOUND,
        format!("Remote {remote} was not found"),
    ))
}

/// A branch may use Git's special `.` remote to pull directly from another
/// local branch. It is not a configured network remote, so do not make the
/// generic remote manager accept it. The Pull form may use it only when it is
/// exactly the current branch's configured local upstream and names that
/// configured source branch.
async fn management_validate_local_pull_source(
    repository: &Path,
    remote: &str,
    remote_branch: &str,
) -> Result<bool> {
    if remote != "." {
        return Ok(false);
    }
    let current = current_branch(repository).await?;
    let (configured_remote, configured_branch) = upstream_remote_branch_or(repository, &current).await?;
    if configured_remote != "." || configured_branch != remote_branch {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "The local source must match this branch's configured local upstream. Refresh Git status before pulling.",
        ));
    }
    Ok(true)
}

async fn management_local_branch_exists(repository: &Path, branch: &str) -> Result<()> {
    let branch = validate_branch_name(branch)?;
    git(
        repository,
        [
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .await
    .map(|_| ())
    .map_err(|_| {
        ServerError::new(
            StatusCode::NOT_FOUND,
            format!("Local branch {branch} was not found"),
        )
    })
}

fn management_display_remote_url(value: &str) -> String {
    // Remote URLs can contain an embedded HTTP credential or an access token
    // in a query/fragment. Never return either to a mobile client merely
    // because it happens to be stored in .git/config. Query and fragment
    // components are display-irrelevant, so drop them even for non-HTTP URL
    // forms instead of trying to recognize every provider's token key.
    let value = value.trim();
    let value = &value[..value.find(['?', '#']).unwrap_or(value.len())];
    let Some(scheme_offset) = value.find("://") else {
        return value.to_string();
    };
    let authority_start = scheme_offset + 3;
    let authority_end = value[authority_start..]
        .find(|character: char| matches!(character, '/' | '?' | '#'))
        .map(|offset| authority_start + offset)
        .unwrap_or(value.len());
    let authority = &value[authority_start..authority_end];
    let Some(at) = authority.rfind('@') else {
        return value.to_string();
    };
    format!(
        "{}<redacted>@{}",
        &value[..authority_start],
        &value[authority_start + at + 1..]
    )
}

/// Read the literal remote config values rather than `git remote get-url`'s
/// resolved output.  `get-url --push` falls back to fetch URLs when no
/// `pushurl` is configured and URL rewrite rules can change its output, both
/// of which make it unsuitable for an index-addressed editor.
async fn management_raw_remote_urls(
    repository: &Path,
    remote: &str,
    push: bool,
) -> Result<Vec<String>> {
    let remote = validate_remote_name(remote)?;
    let key = format!(
        "remote.{remote}.{}",
        if push { "pushurl" } else { "url" }
    );
    match git(
        repository,
        ["config", "--local", "--null", "--get-all", "--", &key],
    )
    .await
    {
        Ok(output) => Ok(output
            .stdout
            .split('\0')
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect()),
        // `git config --get-all` exits non-zero when the key is absent.  An
        // absent pushurl is meaningful: Git pushes to the fetch URL instead.
        Err(_) => Ok(Vec::new()),
    }
}

async fn management_remote_urls(
    repository: &Path,
    remote: &str,
    push: bool,
) -> Result<Vec<String>> {
    let configured = management_raw_remote_urls(repository, remote, push).await?;
    let raw_urls = if push && configured.is_empty() {
        // Match Git/LazyGit's effective destination: without an explicit
        // pushurl, every fetch URL is also a push URL.  The response separately
        // carries `hasExplicitPushUrls` so UI can distinguish this fallback.
        management_raw_remote_urls(repository, remote, false).await?
    } else {
        configured
    };
    Ok(raw_urls
        .into_iter()
        .map(|value| management_display_remote_url(&value))
        .collect())
}

/// Replace the complete list while preserving every other remote URL and
/// config key. `git remote set-url`
/// only identifies a URL by a regex and cannot select a later duplicate URL
/// by index; rebuilding this one multivar key lets the native editor honor the
/// exact row the user chose. If a local config write fails, restore the list
/// best-effort before surfacing the original error.
async fn management_write_remote_urls(
    repository: &Path,
    remote: &str,
    push: bool,
    previous: &[String],
    next: &[String],
) -> Result<()> {
    let remote = validate_remote_name(remote)?;
    let key = format!(
        "remote.{remote}.{}",
        if push { "pushurl" } else { "url" }
    );
    if previous == next {
        return Ok(());
    }

    let apply = async {
        if !previous.is_empty() {
            git(repository, ["config", "--local", "--unset-all", "--", &key]).await?;
        }
        for value in next {
            git(repository, ["config", "--local", "--add", "--", &key, value]).await?;
        }
        Ok(())
    }
    .await;
    if let Err(error) = apply {
        // Do not hide the original write failure, but do our best not to leave
        // a remote with a partially rebuilt URL list.
        let _ = git(repository, ["config", "--local", "--unset-all", "--", &key]).await;
        for value in previous {
            let _ = git(repository, ["config", "--local", "--add", "--", &key, value]).await;
        }
        return Err(error);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Branches and remotes

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementBranchRenameBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    old_branch: String,
    new_branch: String,
}

async fn management_rename_branch(
    State(state): State<AppState>,
    Json(body): Json<ManagementBranchRenameBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let old_branch = validate_branch_name(&body.old_branch)?;
    let new_branch = validate_branch_name(&body.new_branch)?;
    management_local_branch_exists(&repository, &old_branch).await?;
    let output = git(&repository, ["branch", "-m", &old_branch, &new_branch]).await?;
    let mut response =
        GitOperationResponse::success(management_output_or(&output, "Branch renamed successfully"));
    response.branch = Some(new_branch);
    Ok(Json(response))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementBranchCreateFromRefBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    branch: String,
    #[serde(alias = "sourceRef")]
    start_point: String,
}

async fn management_create_branch_from_ref(
    State(state): State<AppState>,
    Json(body): Json<ManagementBranchCreateFromRefBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let branch = validate_branch_name(&body.branch)?;
    let start_point = resolve_commit_reference(&repository, &body.start_point).await?;
    let output = git(&repository, ["branch", &branch, &start_point]).await?;
    let mut response =
        GitOperationResponse::success(management_output_or(&output, "Branch created successfully"));
    response.branch = Some(branch);
    Ok(Json(response))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementBranchUpstreamBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    branch: String,
    remote: String,
    remote_branch: String,
}

async fn management_set_branch_upstream(
    State(state): State<AppState>,
    Json(body): Json<ManagementBranchUpstreamBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let branch = validate_branch_name(&body.branch)?;
    let remote = validate_remote_name(&body.remote)?;
    let remote_branch = validate_branch_name(&body.remote_branch)?;
    management_local_branch_exists(&repository, &branch).await?;
    management_remote_exists(&repository, &remote).await?;
    let upstream = format!("{remote}/{remote_branch}");
    let output = git(
        &repository,
        ["branch", "--set-upstream-to", &upstream, &branch],
    )
    .await?;
    let mut response = GitOperationResponse::success(management_output_or(
        &output,
        "Branch upstream updated successfully",
    ));
    response.branch = Some(branch);
    response.remote_name = Some(remote);
    response.remote_branch = Some(remote_branch);
    Ok(Json(response))
}

/// Remove a local branch's configured pull/tracking destination without
/// touching either branch pointer. This deliberately does not change a
/// branch-specific `pushRemote` or `remote.pushDefault`: Git (and LazyGit)
/// resolve Push separately from the tracking upstream. This is its own narrow
/// operation rather than overloading `set upstream` with a magic remote
/// value: Git's `--unset-upstream` only changes the two branch.* tracking
/// keys and is the operation LazyGit exposes from a tracked branch.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementBranchUnsetUpstreamBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    branch: String,
}

async fn management_unset_branch_upstream(
    State(state): State<AppState>,
    Json(body): Json<ManagementBranchUnsetUpstreamBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let branch = validate_branch_name(&body.branch)?;
    management_local_branch_exists(&repository, &branch).await?;

    // Let Git verify that the named branch genuinely has an upstream.  This
    // protects callers that held a stale Branches screen from receiving a
    // success response for a no-op configuration change.
    let output = git(&repository, ["branch", "--unset-upstream", &branch]).await?;
    let mut response = GitOperationResponse::success(management_output_or(
        &output,
        "Branch upstream removed successfully",
    ));
    response.branch = Some(branch);
    Ok(Json(response))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemoteBranchDeleteBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    remote: String,
    branch: String,
    #[serde(default)]
    confirm: bool,
}

async fn management_delete_remote_branch(
    State(state): State<AppState>,
    Json(body): Json<ManagementRemoteBranchDeleteBody>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "delete a remote branch")?;
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let remote = validate_remote_name(&body.remote)?;
    let branch = validate_branch_name(&body.branch)?;
    management_remote_exists(&repository, &remote).await?;
    let remote_ref = format!("refs/heads/{branch}");
    let output = git(&repository, ["push", &remote, "--delete", &remote_ref]).await?;
    let mut response = GitOperationResponse::success(management_output_or(
        &output,
        "Remote branch deleted successfully",
    ));
    response.remote_name = Some(remote);
    response.remote_branch = Some(branch);
    Ok(Json(response))
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemoteSummary {
    name: String,
    fetch_urls: Vec<String>,
    push_urls: Vec<String>,
    /// `git remote get-url --push` falls back to fetch URLs, so this bit is
    /// required to tell the client whether changing a push URL should create
    /// a dedicated `remote.<name>.pushurl` entry or edit an existing one.
    has_explicit_push_urls: bool,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemotesResponse {
    remotes: Vec<ManagementRemoteSummary>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemotesQuery {
    #[serde(flatten)]
    scope: ManagementProjectScope,
}

async fn management_remotes(
    State(state): State<AppState>,
    Query(query): Query<ManagementRemotesQuery>,
) -> Result<Json<ManagementRemotesResponse>> {
    let repository = management_repository(&state, &query.scope).await?;
    let names = git(&repository, ["remote"])
        .await?
        .stdout
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut remotes = Vec::with_capacity(names.len());
    for name in names {
        // Names originated in Git config, but validate them before using them
        // as arguments in subsequent commands.
        let name = validate_remote_name(&name)?;
        let fetch_urls = management_remote_urls(&repository, &name, false).await?;
        let has_explicit_push_urls = !management_raw_remote_urls(&repository, &name, true)
            .await?
            .is_empty();
        let push_urls = management_remote_urls(&repository, &name, true).await?;
        remotes.push(ManagementRemoteSummary {
            name,
            fetch_urls,
            push_urls,
            has_explicit_push_urls,
        });
    }
    Ok(Json(ManagementRemotesResponse { remotes }))
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum ManagementRemoteUrlRole {
    Fetch,
    Push,
}

impl ManagementRemoteUrlRole {
    fn label(self) -> &'static str {
        match self {
            Self::Fetch => "fetch",
            Self::Push => "push",
        }
    }

    fn is_push(self) -> bool {
        matches!(self, Self::Push)
    }
}

/// A role-aware URL editor.  The client identifies an existing URL by its
/// position, never by its displayed value, because credential-bearing values
/// are intentionally redacted before they reach the UI.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemoteUrlUpdateBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    remote: String,
    role: ManagementRemoteUrlRole,
    /// Required when replacing an existing entry; ignored for append mode.
    index: Option<usize>,
    /// Append another URL instead of replacing one.  This is how a remote
    /// that currently pushes to its fetch URL gains an explicit push URL.
    #[serde(default)]
    add: bool,
    url: String,
}

async fn management_update_remote_url(
    State(state): State<AppState>,
    Json(body): Json<ManagementRemoteUrlUpdateBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let remote = validate_remote_name(&body.remote)?;
    management_remote_exists(&repository, &remote).await?;
    let url = validate_remote_url(&body.url)?;
    let push = body.role.is_push();
    let previous = management_raw_remote_urls(&repository, &remote, push).await?;
    let mut next = previous.clone();

    if body.add {
        if body.index.is_some() {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Do not send an index when adding a remote URL",
            ));
        }
        next.push(url.clone());
    } else {
        let index = body.index.ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "Choose the remote URL entry to replace",
            )
        })?;
        if previous.is_empty() && push {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "This remote has no explicit push URL. Add one instead of replacing the fetch fallback.",
            ));
        }
        if index >= next.len() {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "The selected {} URL no longer exists. Refresh remotes and try again.",
                    body.role.label()
                ),
            ));
        }
        next[index] = url.clone();
    }

    // A named remote always needs at least one fetch URL.  The UI only edits
    // existing fetch rows, but retain this guard at the HTTP boundary.
    if !push && next.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "A remote must keep at least one fetch URL",
        ));
    }
    management_write_remote_urls(&repository, &remote, push, &previous, &next).await?;

    let mut response = GitOperationResponse::success(format!(
        "{} {} URL {} for remote {}",
        if body.add { "Added" } else { "Updated" },
        body.role.label(),
        if body.add { "entry" } else { "entry" },
        remote,
    ));
    response.remote_name = Some(remote);
    response.remote_url = Some(management_display_remote_url(&url));
    Ok(Json(response))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemoteRemoveBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    remote: String,
    #[serde(default)]
    confirm: bool,
}

async fn management_remove_remote(
    State(state): State<AppState>,
    Json(body): Json<ManagementRemoteRemoveBody>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "remove a remote")?;
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let remote = validate_remote_name(&body.remote)?;
    management_remote_exists(&repository, &remote).await?;
    let output = git(&repository, ["remote", "remove", &remote]).await?;
    let mut response =
        GitOperationResponse::success(management_output_or(&output, "Remote removed successfully"));
    response.remote_name = Some(remote);
    Ok(Json(response))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemoteRenameBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    old_remote: String,
    new_remote: String,
}

async fn management_rename_remote(
    State(state): State<AppState>,
    Json(body): Json<ManagementRemoteRenameBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let old_remote = validate_remote_name(&body.old_remote)?;
    let new_remote = validate_remote_name(&body.new_remote)?;
    management_remote_exists(&repository, &old_remote).await?;
    let output = git(&repository, ["remote", "rename", &old_remote, &new_remote]).await?;
    let mut response =
        GitOperationResponse::success(management_output_or(&output, "Remote renamed successfully"));
    response.remote_name = Some(new_remote);
    Ok(Json(response))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemoteFetchBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    remote: String,
    #[serde(default)]
    prune: bool,
    tags: Option<bool>,
}

async fn management_fetch_remote(
    State(state): State<AppState>,
    Json(body): Json<ManagementRemoteFetchBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let remote = validate_remote_name(&body.remote)?;
    management_remote_exists(&repository, &remote).await?;
    // A standalone fetch refreshes remote-tracking refs but is not a
    // preview/input to another operation, so match LazyGit and leave any
    // existing FETCH_HEAD untouched. Fetch-and-plan and Pull intentionally
    // omit this flag because they immediately resolve FETCH_HEAD themselves.
    let mut args = vec!["fetch".to_string(), "--no-write-fetch-head".to_string()];
    if body.prune {
        args.push("--prune".to_string());
    }
    match body.tags {
        Some(true) => args.push("--tags".to_string()),
        Some(false) => args.push("--no-tags".to_string()),
        None => {}
    }
    args.push(remote.clone());
    let output = git(&repository, args).await?;
    let mut response = GitOperationResponse::success(management_output_or(
        &output,
        "Fetch completed successfully",
    ));
    response.remote_name = Some(remote);
    Ok(Json(response))
}

/// Refresh every configured remote without turning the result into input for
/// another Git workflow.  This is LazyGit's Fetch All command: it deliberately
/// leaves FETCH_HEAD alone, so a reviewed fetch/rebase plan cannot be replaced
/// by a background-style refresh from another remote.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementFetchAllRemotesBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
}

async fn management_fetch_all_remotes(
    State(state): State<AppState>,
    Json(body): Json<ManagementFetchAllRemotesBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    // Do not resolve a preferred remote here. `--all` is semantically
    // important: a project can intentionally keep independent forge, mirror,
    // and review remotes, and LazyGit's Fetch All updates every one of them.
    let output = git(
        &repository,
        ["fetch", "--all", "--no-write-fetch-head"],
    )
    .await?;
    Ok(Json(GitOperationResponse::success(management_output_or(
        &output,
        "Fetched all remotes successfully",
    ))))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemotePullBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    remote: String,
    remote_branch: String,
    /// Optional one-shot override for this pull. When omitted, use the
    /// repository-local `pull.rebase` setting just like the ordinary Pull
    /// action. Accepted values are `true`, `false`, `merges`, and
    /// `interactive`. The interactive setting matches LazyGit's direct Pull:
    /// Git's default todo is accepted without waiting for a terminal editor.
    /// The separate fetch-and-plan route is the explicitly reviewed option.
    rebase: Option<String>,
}

async fn management_pull_remote(
    State(state): State<AppState>,
    Json(body): Json<ManagementRemotePullBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let requested_remote = body.remote.trim();
    let remote_branch = validate_branch_name(&body.remote_branch)?;
    let remote = if management_validate_local_pull_source(
        &repository,
        requested_remote,
        &remote_branch,
    )
    .await?
    {
        ".".to_string()
    } else {
        let remote = validate_remote_name(requested_remote)?;
        management_remote_exists(&repository, &remote).await?;
        remote
    };
    Ok(Json(
        pull_selected_remote(&repository, remote, remote_branch, body.rebase.as_deref()).await?,
    ))
}

/// Fetch one explicit remote branch, then build the same immutable structured
/// rebase plan used by the native editor. This is the explicit reviewed
/// complement to LazyGit-compatible interactive Pull: fetching never changes
/// the checked-out tree, and applying the plan is a later, separately
/// confirmed operation.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemoteRebasePlanBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    remote: String,
    remote_branch: String,
}

async fn management_remote_rebase_plan(
    State(state): State<AppState>,
    Json(body): Json<ManagementRemoteRebasePlanBody>,
) -> Result<Json<AdvancedRebasePlanPreviewResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let remote = validate_remote_name(&body.remote)?;
    let remote_branch = validate_branch_name(&body.remote_branch)?;
    management_remote_exists(&repository, &remote).await?;
    // Do not defer this until after a user edits a plan: the final rebase
    // must not rely on Git's hidden autostash lifecycle. Fetch itself is
    // worktree-safe, but requiring a clean tree here keeps the preview and
    // the subsequent confirmed rewrite on the same safety boundary.
    advanced_require_clean_worktree(&repository, "fetch and plan an editable rebase").await?;
    // This is a branch picker, not a generic ref picker. Keep its fetch
    // target fully qualified so a same-named remote tag cannot become
    // FETCH_HEAD through Git's short-name DWIM lookup.
    let remote_ref = format!("refs/heads/{remote_branch}");
    git(&repository, ["fetch", remote.as_str(), remote_ref.as_str()]).await?;
    let fetched_head = resolve_commit_reference(&repository, "FETCH_HEAD").await?;
    let current_head = advanced_current_head(&repository).await?;
    let replay_range = format!("{fetched_head}..{current_head}");
    let replay_count = git(&repository, ["rev-list", "--count", replay_range.as_str()])
        .await?
        .stdout
        .trim()
        .parse::<usize>()
        .unwrap_or(0);
    if replay_count == 0 {
        let guidance = if current_head == fetched_head {
            format!(
                "{remote}/{remote_branch} is already current. There are no local commits to edit."
            )
        } else {
            format!(
                "No local commits are available to edit after fetching {remote}/{remote_branch}. Use direct Pull to fast-forward the current branch."
            )
        };
        return Err(ServerError::new(StatusCode::BAD_REQUEST, guidance));
    }
    let (upstream, expected_head, commits) = advanced_rebase_range(&repository, "FETCH_HEAD").await?;
    let expected_target_ref = checked_out_target_ref(&repository).await?;
    Ok(Json(AdvancedRebasePlanPreviewResponse {
        upstream,
        expected_head,
        expected_target_ref,
        commits,
    }))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemotePushBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    remote: String,
    branch: Option<String>,
    remote_branch: Option<String>,
    #[serde(default)]
    set_upstream: bool,
    #[serde(default)]
    force_with_lease: bool,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    confirm: bool,
}

async fn management_push_remote(
    State(state): State<AppState>,
    Json(body): Json<ManagementRemotePushBody>,
) -> Result<Json<GitOperationResponse>> {
    if body.force && body.force_with_lease {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "force and forceWithLease are mutually exclusive; choose one push safety policy",
        ));
    }
    if body.force {
        require_management_confirmation(body.confirm, "force-push")?;
    } else if body.force_with_lease {
        require_management_confirmation(body.confirm, "force-push with lease")?;
    }
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let remote = validate_remote_name(&body.remote)?;
    management_remote_exists(&repository, &remote).await?;
    let current = current_branch(&repository).await?;
    let branch = match body.branch.as_deref() {
        Some(branch) => validate_branch_name(branch)?,
        None => validate_branch_name(&current).map_err(|_| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "Cannot push from a detached HEAD. Select a local branch first.",
            )
        })?,
    };
    management_local_branch_exists(&repository, &branch).await?;
    let (remote_branch, inherited_set_upstream) = match body.remote_branch.as_deref() {
        Some(remote_branch) => (validate_branch_name(remote_branch)?, false),
        // Publishing a branch and establishing a new upstream is explicit
        // user intent. Keep the established API behavior for that dedicated
        // flow; native Publish always shows the named remote branch first.
        None if body.set_upstream => (branch.clone(), false),
        None => {
            if current != branch {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Choose a remote branch explicitly when pushing a non-current local branch",
                ));
            }
            let target = require_configured_git_push_target(
                configured_git_push_target(&repository, &branch).await?,
            )?;
            if target.remote_name != remote {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Git's configured push target is {}/{}. Choose that remote or enter a remote branch explicitly for {remote}.",
                        target.remote_name, target.remote_branch,
                    ),
                ));
            }
            (target.remote_branch, target.set_upstream)
        }
    };
    // An empty target means "ordinary Push", so preserve Git's
    // push.autoSetupRemote side effect when the resolver identified one. An
    // explicit remote branch stays under the caller's visible checkbox.
    let set_upstream = body.set_upstream || inherited_set_upstream;
    if set_upstream && current != branch {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Upstream can only be set for the currently checked-out branch",
        ));
    }
    let refspec = format!("refs/heads/{branch}:refs/heads/{remote_branch}");
    let mut args = vec!["push".to_string()];
    if body.force {
        args.push("--force".to_string());
    } else if body.force_with_lease {
        args.push("--force-with-lease".to_string());
    }
    if set_upstream {
        args.push("--set-upstream".to_string());
    }
    args.push(remote.clone());
    args.push(refspec);
    let output = git(&repository, args).await?;
    let mut response =
        GitOperationResponse::success(management_output_or(&output, "Push completed successfully"));
    response.remote_name = Some(remote);
    response.remote_branch = Some(remote_branch);
    response.branch = Some(branch);
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// Stashes and tags

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashShowQuery {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    reference: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashDetailResponse {
    reference: String,
    stats: String,
    diff: String,
    is_truncated: bool,
}

async fn management_show_stash(
    State(state): State<AppState>,
    Query(query): Query<ManagementStashShowQuery>,
) -> Result<Json<ManagementStashDetailResponse>> {
    let repository = management_repository(&state, &query.scope).await?;
    let reference = validate_stash_ref(&query.reference)?;
    // Resolve first so an out-of-range stash index is a clear client error.
    git(
        &repository,
        ["rev-parse", "--verify", "--end-of-options", &reference],
    )
    .await?;
    let stats = git(
        &repository,
        ["stash", "show", "--include-untracked", "--stat", &reference],
    )
    .await?
    .stdout;
    let output = git(
        &repository,
        [
            "stash",
            "show",
            "--include-untracked",
            "--format=",
            "--patch",
            &reference,
        ],
    )
    .await?
    .stdout;
    let (diff, is_truncated) = management_truncate_output(output);
    Ok(Json(ManagementStashDetailResponse {
        reference,
        stats,
        diff,
        is_truncated,
    }))
}

// A stash is a merge-like commit.  Its first parent is the base commit, its
// own tree holds tracked worktree changes, and (when `-u`/`-a` was used) its
// third parent stores untracked content.  Keep that distinction explicit so a
// selective restore never accidentally treats every file in the stash base as
// selected content.
#[derive(Debug, Clone)]
struct ManagementStashSnapshot {
    reference: String,
    hash: String,
    base_hash: String,
    untracked_tree: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ManagementStashFileSource {
    Tracked,
    Untracked,
}

impl ManagementStashFileSource {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Tracked => "tracked",
            Self::Untracked => "untracked",
        }
    }
}

#[derive(Debug, Clone)]
struct ManagementStashFile {
    path: String,
    status: String,
    source: ManagementStashFileSource,
}

/// Git does not leave an operation marker behind for a conflicted
/// `stash apply`/`stash pop`.  Preserve only the small amount of
/// server-owned state needed to present the conflict resolver and safely
/// abandon an application which began from a pristine worktree.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashRecoveryMarker {
    version: u8,
    reference: String,
    stash_hash: String,
    pop: bool,
    untracked_paths: Vec<String>,
}

/// Durable state for LazyGit's "stash unstaged changes" sequence.  Git has
/// no operation metadata for this workflow: it temporarily commits the index,
/// saves only the remaining worktree delta, then soft-resets the temporary
/// commit.  Persist every boundary so an interrupted request never strands a
/// hidden commit or loses the unstaged delta.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashUnstagedRecoveryMarker {
    version: u8,
    original_head: String,
    target_ref: String,
    /// A random value persisted before the synthetic commit is created.  Its
    /// matching trailer lets recovery distinguish the narrow post-commit,
    /// pre-marker-write crash window from an unrelated external commit.
    #[serde(default)]
    transaction_id: Option<String>,
    temporary_head: Option<String>,
    /// The immutable object created by `git stash create`. This is persisted
    /// before `stash store` so a process crash cannot make a successfully
    /// stored stash anonymous and cause a duplicate on retry.
    stash_hash: Option<String>,
    /// `stash_hash` exists as an object before it has a visible reflog entry.
    /// Keep that boundary explicit: `stash store` is not idempotent, but
    /// checking the stash reflog by immutable OID is.
    #[serde(default)]
    stash_stored: bool,
    message: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashFileSummary {
    path: String,
    status: String,
    source: String,
}

impl From<&ManagementStashFile> for ManagementStashFileSummary {
    fn from(file: &ManagementStashFile) -> Self {
        Self {
            path: file.path.clone(),
            status: file.status.clone(),
            source: file.source.wire_name().to_string(),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashFilesQuery {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    reference: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashFilesResponse {
    reference: String,
    files: Vec<ManagementStashFileSummary>,
}

async fn management_stash_files(
    State(state): State<AppState>,
    Query(query): Query<ManagementStashFilesQuery>,
) -> Result<Json<ManagementStashFilesResponse>> {
    let repository = management_repository(&state, &query.scope).await?;
    let snapshot = management_stash_snapshot(&repository, &query.reference).await?;
    let files = management_stash_file_inventory(&repository, &snapshot).await?;
    Ok(Json(ManagementStashFilesResponse {
        reference: snapshot.reference,
        files: files.iter().map(ManagementStashFileSummary::from).collect(),
    }))
}

async fn management_stash_snapshot(
    repository: &Path,
    reference: &str,
) -> Result<ManagementStashSnapshot> {
    let reference = validate_stash_ref(reference)?;
    let commit_spec = format!("{reference}^{{commit}}");
    let hash = git(
        repository,
        [
            "rev-parse",
            "--verify",
            "--end-of-options",
            commit_spec.as_str(),
        ],
    )
    .await?
    .stdout
    .lines()
    .map(str::trim)
    .find(|value| !value.is_empty())
    .map(str::to_string)
    .ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            "Stash reference did not resolve to a commit",
        )
    })?;
    let parents = git(repository, ["rev-list", "--parents", "-n", "1", &hash])
        .await?
        .stdout
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parents.first().is_none_or(|value| value != &hash) || parents.len() < 2 {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Stash commit is missing its base parent",
        ));
    }

    Ok(ManagementStashSnapshot {
        reference,
        hash,
        base_hash: parents[1].clone(),
        // A conventional stash has base/index/untracked parents.  `--all`
        // uses the same untracked parent for ignored content, which is still
        // restored as an explicitly selected worktree path.
        untracked_tree: parents.get(3).cloned(),
    })
}

/// Resolve a positional stash selector and, when a native row supplied the
/// immutable object ID it displayed, ensure it still identifies that exact
/// entry. A new stash shifts every `stash@{N}` selector, so mutating by the
/// selector alone can otherwise affect a different saved change.
async fn management_stash_snapshot_for_action(
    repository: &Path,
    reference: &str,
    expected_hash: Option<&str>,
) -> Result<ManagementStashSnapshot> {
    let expected_hash = expected_hash
        .map(management_expected_stash_hash)
        .transpose()?;
    let snapshot = management_stash_snapshot(repository, reference).await?;
    if let Some(expected_hash) = expected_hash
        && !snapshot.hash.eq_ignore_ascii_case(expected_hash)
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The selected stash changed since this action was prepared. Refresh the stash list and try again.",
        ));
    }
    Ok(snapshot)
}

fn management_stash_status(status: &str) -> String {
    match status.chars().next() {
        Some('A') => "added",
        Some('D') => "deleted",
        Some('M') => "modified",
        Some('T') => "type-changed",
        _ => "changed",
    }
    .to_string()
}

async fn management_stash_file_target(repository: &Path, path: &str) -> Result<String> {
    validate_file_path(path)?;
    let normalized = normalize_repo_relative_path(path);
    if normalized == ".git" || normalized.starts_with(".git/") {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "A stash path cannot target Git metadata",
        ));
    }
    let resolved = resolve_git_file_target(repository, &normalized, GitFileTargetPolicy::Discard)
        .await?;
    Ok(resolved.repository_relative_file)
}

async fn management_stash_file_inventory(
    repository: &Path,
    snapshot: &ManagementStashSnapshot,
) -> Result<Vec<ManagementStashFile>> {
    let tracked = git(
        repository,
        [
            "diff",
            "--no-ext-diff",
            "--no-renames",
            "--name-status",
            "-z",
            &snapshot.base_hash,
            &snapshot.hash,
        ],
    )
    .await?
    .stdout_bytes;
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    let tracked_records = tracked.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut index = 0;
    while index + 1 < tracked_records.len() {
        let status = String::from_utf8_lossy(tracked_records[index]).to_string();
        let raw_path = String::from_utf8_lossy(tracked_records[index + 1]).to_string();
        index += 2;
        if status.is_empty() || raw_path.is_empty() {
            continue;
        }
        let path = management_stash_file_target(repository, &raw_path).await?;
        if seen.insert(path.clone()) {
            files.push(ManagementStashFile {
                path,
                status: management_stash_status(&status),
                source: ManagementStashFileSource::Tracked,
            });
        }
        if files.len() > MANAGEMENT_MAX_STASH_INVENTORY_FILES {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "Stash contains more than {MANAGEMENT_MAX_STASH_INVENTORY_FILES} paths; restore it from the terminal instead"
                ),
            ));
        }
    }

    if let Some(untracked_tree) = &snapshot.untracked_tree {
        let untracked = git(
            repository,
            ["ls-tree", "-r", "-z", "--name-only", untracked_tree],
        )
        .await?
        .stdout_bytes;
        for raw_path in untracked.split(|byte| *byte == 0) {
            if raw_path.is_empty() {
                continue;
            }
            let raw_path = String::from_utf8_lossy(raw_path).to_string();
            let path = management_stash_file_target(repository, &raw_path).await?;
            if seen.insert(path.clone()) {
                files.push(ManagementStashFile {
                    path,
                    status: "untracked".to_string(),
                    source: ManagementStashFileSource::Untracked,
                });
            }
            if files.len() > MANAGEMENT_MAX_STASH_INVENTORY_FILES {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Stash contains more than {MANAGEMENT_MAX_STASH_INVENTORY_FILES} paths; restore it from the terminal instead"
                    ),
                ));
            }
        }
    }

    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

async fn management_stash_recovery_marker_path(repository: &Path) -> Result<PathBuf> {
    Ok(git_metadata_directory(repository)
        .await?
        .join(MANAGEMENT_STASH_RECOVERY_MARKER))
}

fn management_invalid_stash_recovery_marker() -> ServerError {
    ServerError::new(
        StatusCode::CONFLICT,
        "Stash recovery data is invalid. Resolve or clean the conflicted worktree from a terminal before starting another Git operation.",
    )
}

async fn management_validate_stash_recovery_marker(
    repository: &Path,
    marker: &ManagementStashRecoveryMarker,
) -> Result<()> {
    if marker.version != MANAGEMENT_STASH_RECOVERY_VERSION
        || marker.untracked_paths.len() > MANAGEMENT_MAX_STASH_INVENTORY_FILES
        || !matches!(marker.stash_hash.len(), 40 | 64)
        || !marker
            .stash_hash
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(management_invalid_stash_recovery_marker());
    }
    validate_stash_ref(&marker.reference).map_err(|_| management_invalid_stash_recovery_marker())?;
    for path in &marker.untracked_paths {
        validate_file_path(path).map_err(|_| management_invalid_stash_recovery_marker())?;
        let normalized = normalize_repo_relative_path(path);
        if normalized != *path || normalized == ".git" || normalized.starts_with(".git/") {
            return Err(management_invalid_stash_recovery_marker());
        }
        safe_repo_child(repository, &normalized).map_err(|_| management_invalid_stash_recovery_marker())?;
    }
    Ok(())
}

async fn management_read_stash_recovery_marker(
    repository: &Path,
) -> Result<Option<ManagementStashRecoveryMarker>> {
    let path = management_stash_recovery_marker_path(repository).await?;
    let metadata = match fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_server_error(error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MANAGEMENT_MAX_STASH_RECOVERY_MARKER_BYTES as u64
    {
        return Err(management_invalid_stash_recovery_marker());
    }
    let bytes = fs::read(&path).await.map_err(io_server_error)?;
    let marker = serde_json::from_slice::<ManagementStashRecoveryMarker>(&bytes)
        .map_err(|_| management_invalid_stash_recovery_marker())?;
    management_validate_stash_recovery_marker(repository, &marker).await?;
    Ok(Some(marker))
}

async fn management_write_stash_recovery_marker(
    repository: &Path,
    marker: &ManagementStashRecoveryMarker,
) -> Result<()> {
    management_validate_stash_recovery_marker(repository, marker).await?;
    let encoded = serde_json::to_vec(marker).map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not persist stash recovery data",
            error.to_string(),
        )
    })?;
    if encoded.len() > MANAGEMENT_MAX_STASH_RECOVERY_MARKER_BYTES {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "This stash contains too much recovery metadata to apply safely from the mobile app",
        ));
    }
    let path = management_stash_recovery_marker_path(repository).await?;
    if let Ok(metadata) = fs::symlink_metadata(&path).await {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(management_invalid_stash_recovery_marker());
        }
    }
    fs::write(path, encoded).await.map_err(io_server_error)
}

async fn management_clear_stash_recovery_marker(repository: &Path) -> Result<()> {
    let path = management_stash_recovery_marker_path(repository).await?;
    let metadata = match fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_server_error(error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(management_invalid_stash_recovery_marker());
    }
    fs::remove_file(path).await.map_err(io_server_error)
}

async fn management_stash_unstaged_recovery_marker_path(repository: &Path) -> Result<PathBuf> {
    Ok(git_metadata_directory(repository)
        .await?
        .join(MANAGEMENT_STASH_UNSTAGED_RECOVERY_MARKER))
}

fn management_invalid_stash_unstaged_recovery_marker() -> ServerError {
    ServerError::new(
        StatusCode::CONFLICT,
        "Stash-unstaged recovery data is invalid. Restore the repository from a terminal before starting another Git operation.",
    )
}

fn management_validate_stash_unstaged_recovery_marker(
    marker: &ManagementStashUnstagedRecoveryMarker,
) -> Result<()> {
    if marker.version != MANAGEMENT_STASH_UNSTAGED_RECOVERY_VERSION
        || advanced_validate_expected_head(&marker.original_head).is_err()
        || validate_expected_target_ref(&marker.target_ref).is_err()
        || marker
            .temporary_head
            .as_deref()
            .is_some_and(|value| advanced_validate_expected_head(value).is_err())
        || marker
            .stash_hash
            .as_deref()
            .is_some_and(|value| advanced_validate_expected_head(value).is_err())
        || marker
            .transaction_id
            .as_deref()
            .is_some_and(|value| uuid::Uuid::parse_str(value).is_err())
        || (marker.stash_hash.is_some() && marker.temporary_head.is_none())
        || (marker.stash_stored && marker.stash_hash.is_none())
        || validate_optional_git_message(marker.message.as_deref()).is_err()
    {
        return Err(management_invalid_stash_unstaged_recovery_marker());
    }
    Ok(())
}

fn management_stash_unstaged_synthetic_commit_message(
    marker: &ManagementStashUnstagedRecoveryMarker,
) -> String {
    let subject = "[iowb] stashing unstaged changes";
    marker
        .transaction_id
        .as_deref()
        .map(|transaction_id| {
            format!(
                "{subject}\n\nIowb-Stash-Unstaged-Transaction: {transaction_id}"
            )
        })
        // Keep an already-persisted marker from an older server build
        // resumable if it stopped before the temporary commit was made.  A
        // legacy marker cannot safely reconcile a post-commit crash because
        // it has no unique proof that HEAD is ours.
        .unwrap_or_else(|| subject.to_string())
}

/// Recover the single durable boundary Git itself cannot represent: the
/// synthetic index commit completed, but the process stopped before its OID
/// was written into the marker.  Only a direct child of the recorded HEAD
/// carrying this marker's unguessable transaction trailer is accepted.  Any
/// other changed HEAD remains locked for terminal recovery rather than being
/// mistaken for a server-owned temporary commit.
async fn management_reconcile_stash_unstaged_temporary_head(
    repository: &Path,
    marker: &mut ManagementStashUnstagedRecoveryMarker,
) -> Result<()> {
    if marker.temporary_head.is_some() {
        return Ok(());
    }

    let current_head = advanced_current_head(repository).await?;
    if current_head == marker.original_head {
        return Ok(());
    }
    let transaction_id = marker.transaction_id.as_deref().ok_or_else(|| {
        ServerError::new(
            StatusCode::CONFLICT,
            "A legacy stash-unstaged recovery stopped after its temporary commit. Restore the repository from a terminal instead of guessing which commit to reset.",
        )
    })?;
    uuid::Uuid::parse_str(transaction_id).map_err(|_| management_invalid_stash_unstaged_recovery_marker())?;
    management_validate_stash_unstaged_target(repository, marker, &current_head).await?;

    let parents = git(repository, ["rev-list", "--parents", "-n", "1", &current_head])
        .await?
        .stdout
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let has_expected_parent = parents.len() == 2
        && parents.first().is_some_and(|head| head == &current_head)
        && parents.get(1).is_some_and(|parent| parent == &marker.original_head);
    let expected_message = management_stash_unstaged_synthetic_commit_message(marker);
    let committed_message = git(repository, ["show", "-s", "--format=%B", &current_head])
        .await?
        .stdout;
    if !has_expected_parent || committed_message.trim_end() != expected_message {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "HEAD changed while stashing unstaged changes, and it is not the validated temporary commit. Restore the repository from a terminal instead.",
        ));
    }

    marker.temporary_head = Some(current_head);
    management_write_stash_unstaged_recovery_marker(repository, marker).await
}

async fn management_read_stash_unstaged_recovery_marker(
    repository: &Path,
) -> Result<Option<ManagementStashUnstagedRecoveryMarker>> {
    let path = management_stash_unstaged_recovery_marker_path(repository).await?;
    let metadata = match fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_server_error(error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MANAGEMENT_MAX_STASH_RECOVERY_MARKER_BYTES as u64
    {
        return Err(management_invalid_stash_unstaged_recovery_marker());
    }
    let bytes = fs::read(&path).await.map_err(io_server_error)?;
    let marker = serde_json::from_slice::<ManagementStashUnstagedRecoveryMarker>(&bytes)
        .map_err(|_| management_invalid_stash_unstaged_recovery_marker())?;
    management_validate_stash_unstaged_recovery_marker(&marker)?;
    Ok(Some(marker))
}

async fn management_write_stash_unstaged_recovery_marker(
    repository: &Path,
    marker: &ManagementStashUnstagedRecoveryMarker,
) -> Result<()> {
    management_validate_stash_unstaged_recovery_marker(marker)?;
    let encoded = serde_json::to_vec(marker).map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not persist stash-unstaged recovery data",
            error.to_string(),
        )
    })?;
    if encoded.len() > MANAGEMENT_MAX_STASH_RECOVERY_MARKER_BYTES {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "This stash-unstaged recovery record is too large to persist safely",
        ));
    }
    let path = management_stash_unstaged_recovery_marker_path(repository).await?;
    if let Ok(metadata) = fs::symlink_metadata(&path).await {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(management_invalid_stash_unstaged_recovery_marker());
        }
    }
    fs::write(path, encoded).await.map_err(io_server_error)
}

async fn management_clear_stash_unstaged_recovery_marker(repository: &Path) -> Result<()> {
    let path = management_stash_unstaged_recovery_marker_path(repository).await?;
    let metadata = match fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_server_error(error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(management_invalid_stash_unstaged_recovery_marker());
    }
    fs::remove_file(path).await.map_err(io_server_error)
}

async fn management_stash_recovery_workflow(
    repository: &Path,
) -> Result<Option<DetectedGitWorkflow>> {
    Ok(management_read_stash_recovery_marker(repository)
        .await?
        .map(|marker| DetectedGitWorkflow {
            kind: GitWorkflowKind::Stash,
            current_commit: Some(marker.stash_hash),
            current_step: None,
            total_steps: None,
            empty_step_recovery: EmptyStepRecovery::None,
        }))
}

async fn management_stash_unstaged_recovery_workflow(
    repository: &Path,
) -> Result<Option<DetectedGitWorkflow>> {
    Ok(management_read_stash_unstaged_recovery_marker(repository)
        .await?
        .map(|marker| DetectedGitWorkflow {
            kind: GitWorkflowKind::StashUnstaged,
            current_commit: marker
                .temporary_head
                .or(marker.stash_hash)
                .or(Some(marker.original_head)),
            current_step: None,
            total_steps: None,
            empty_step_recovery: EmptyStepRecovery::None,
        }))
}

async fn management_stash_unstaged_has_staged_changes(repository: &Path) -> Result<bool> {
    Ok(!git(
        repository,
        [
            "diff",
            "--cached",
            "--no-ext-diff",
            "--name-only",
            "-z",
            "--ignore-submodules=none",
        ],
    )
    .await?
    .stdout_bytes
    .is_empty())
}

async fn management_stash_unstaged_has_unstaged_changes(repository: &Path) -> Result<bool> {
    Ok(!git(
        repository,
        [
            "diff",
            "--no-ext-diff",
            "--name-only",
            "-z",
            "--ignore-submodules=none",
        ],
    )
    .await?
    .stdout_bytes
    .is_empty())
}

async fn management_validate_stash_unstaged_target(
    repository: &Path,
    marker: &ManagementStashUnstagedRecoveryMarker,
    expected_head: &str,
) -> Result<()> {
    let expected_head = advanced_validate_expected_head(expected_head)?;
    let current_head = advanced_current_head(repository).await?;
    if current_head != expected_head {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "HEAD changed while stashing unstaged changes. Do not continue this recovery on a different history; restore it from a terminal instead.",
        ));
    }
    let current_target_ref = checked_out_target_ref(repository).await?;
    if current_target_ref != marker.target_ref {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The checked-out branch changed while stashing unstaged changes. Do not continue this recovery on a different branch; restore it from a terminal instead.",
        ));
    }
    Ok(())
}

async fn management_validate_stash_unstaged_temporary_commit(
    repository: &Path,
    marker: &ManagementStashUnstagedRecoveryMarker,
    temporary_head: &str,
) -> Result<()> {
    management_validate_stash_unstaged_target(repository, marker, temporary_head).await?;
    management_validate_stash_unstaged_temporary_commit_object(repository, marker, temporary_head)
        .await
}

/// Validate the immutable relationship of the temporary commit without
/// assuming it is still checked out. Abort must be able to do this after the
/// final soft reset has already moved HEAD back to `original_head`.
async fn management_validate_stash_unstaged_temporary_commit_object(
    repository: &Path,
    marker: &ManagementStashUnstagedRecoveryMarker,
    temporary_head: &str,
) -> Result<()> {
    advanced_validate_expected_head(temporary_head)?;
    let parent = git(
        repository,
        ["rev-parse", "--verify", &format!("{temporary_head}^")],
    )
    .await?
    .stdout
    .trim()
    .to_string();
    if parent != marker.original_head {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The temporary stash commit no longer has the expected original HEAD. Restore this repository from a terminal instead.",
        ));
    }
    Ok(())
}

async fn management_validate_stash_unstaged_saved_stash(
    repository: &Path,
    marker: &ManagementStashUnstagedRecoveryMarker,
    stash_hash: &str,
    temporary_head: &str,
) -> Result<()> {
    advanced_validate_expected_head(stash_hash)?;
    let parents = git(repository, ["rev-list", "--parents", "-n", "1", stash_hash])
        .await?
        .stdout
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parents.first().is_none_or(|value| value != stash_hash)
        || parents.get(1).is_none_or(|value| value != temporary_head)
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The saved stash no longer belongs to this stash-unstaged recovery. Restore the repository from a terminal instead.",
        ));
    }
    management_validate_stash_unstaged_temporary_commit_object(repository, marker, temporary_head)
        .await
}

/// Make the `stash store` transition idempotent across a process crash. The
/// object ID is persisted before store runs; if the process dies immediately
/// afterward, the next attempt finds that exact object in the stash reflog
/// instead of adding a second stash entry.
async fn management_stash_unstaged_is_stored(
    repository: &Path,
    stash_hash: &str,
) -> Result<bool> {
    advanced_validate_expected_head(stash_hash)?;
    let entries = git(repository, ["stash", "list", "--format=%H"]).await?;
    Ok(entries
        .stdout
        .lines()
        .map(str::trim)
        .any(|entry| entry.eq_ignore_ascii_case(stash_hash)))
}

async fn management_store_stash_unstaged_if_needed(
    repository: &Path,
    marker: &mut ManagementStashUnstagedRecoveryMarker,
    temporary_head: &str,
) -> Result<String> {
    let stash_hash = marker
        .stash_hash
        .clone()
        .ok_or_else(management_invalid_stash_unstaged_recovery_marker)?;
    management_validate_stash_unstaged_saved_stash(
        repository,
        marker,
        &stash_hash,
        temporary_head,
    )
    .await?;
    let already_stored = management_stash_unstaged_is_stored(repository, &stash_hash).await?;
    if marker.stash_stored && !already_stored {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The recovery stash was removed after it was saved. Restore the repository from a terminal instead.",
        ));
    }
    if !already_stored {
        let stash_message = management_stash_unstaged_message(marker);
        management_store_stash(repository, &stash_hash, &stash_message).await?;
    }
    if !marker.stash_stored {
        marker.stash_stored = true;
        management_write_stash_unstaged_recovery_marker(repository, marker).await?;
    }
    Ok(stash_hash)
}

fn management_stash_unstaged_message(marker: &ManagementStashUnstagedRecoveryMarker) -> String {
    marker
        .message
        .clone()
        .unwrap_or_else(|| "stashed unstaged changes".to_string())
}

fn management_stash_unstaged_success_response() -> GitOperationResponse {
    let message = "Stashed only unstaged changes. Your staged changes remain in the index.";
    let mut response = GitOperationResponse::success(message);
    response.message = Some(message.to_string());
    response
}

async fn management_complete_stash_unstaged_recovery(
    repository: &Path,
    marker: &mut ManagementStashUnstagedRecoveryMarker,
) -> Result<GitOperationResponse> {
    management_validate_stash_unstaged_recovery_marker(marker)?;
    management_reconcile_stash_unstaged_temporary_head(repository, marker).await?;
    let current_head = advanced_current_head(repository).await?;
    if marker.temporary_head.is_some() && current_head == marker.original_head {
        if marker.stash_hash.is_none() {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "The temporary stash commit was reset before an unstaged stash was saved. Use Abort to clear this recovery after verifying the original changes.",
            ));
        }
        let current_target_ref = checked_out_target_ref(repository).await?;
        if current_target_ref != marker.target_ref {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "The checked-out branch changed while stashing unstaged changes. Restore the repository from a terminal instead.",
            ));
        }
        let temporary_head = marker
            .temporary_head
            .clone()
            .ok_or_else(management_invalid_stash_unstaged_recovery_marker)?;
        // A crash can happen after `stash store` but before its marker bit is
        // persisted. Finish that durable bookkeeping before considering the
        // prior soft reset successful.
        let _ = management_store_stash_unstaged_if_needed(repository, marker, &temporary_head)
            .await?;
        if management_stash_unstaged_has_unstaged_changes(repository).await? {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "Tracked changes appeared after the stash-unstaged recovery reached its final reset. Restore the repository from a terminal instead of discarding them.",
            ));
        }
        // The final soft reset completed but the process stopped before it
        // could remove the marker. The saved stash is already the successful
        // outcome, so only unlock the repository.
        management_clear_stash_unstaged_recovery_marker(repository).await?;
        let mut response = management_stash_unstaged_success_response();
        response.operation_state = Some(git_operation_state(repository).await?);
        return Ok(response);
    }

    if marker.temporary_head.is_none() {
        management_validate_stash_unstaged_target(repository, marker, &marker.original_head)
            .await?;
        let temporary_commit_message = management_stash_unstaged_synthetic_commit_message(marker);
        let commit_result = git_with_noninteractive_editor(
            repository,
            [
                "commit",
                "--no-verify",
                "--no-gpg-sign",
                "-m",
                temporary_commit_message.as_str(),
            ],
        )
        .await;
        if let Err(error) = commit_result {
            // A normal commit rejection (for example a missing author
            // identity) leaves HEAD untouched. Do not lock the user out of
            // correcting that configuration just because no transaction step
            // actually ran.
            if advanced_current_head(repository)
                .await
                .ok()
                .as_deref()
                == Some(marker.original_head.as_str())
            {
                let _ = management_clear_stash_unstaged_recovery_marker(repository).await;
            }
            return Err(error);
        }
        let temporary_head = advanced_current_head(repository).await?;
        if temporary_head == marker.original_head {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "Git did not create the temporary commit needed to stash unstaged changes",
            ));
        }
        marker.temporary_head = Some(temporary_head.clone());
        management_validate_stash_unstaged_temporary_commit(repository, marker, &temporary_head)
            .await?;
        management_write_stash_unstaged_recovery_marker(repository, marker).await?;
    }

    let temporary_head = marker
        .temporary_head
        .clone()
        .ok_or_else(management_invalid_stash_unstaged_recovery_marker)?;
    management_validate_stash_unstaged_temporary_commit(repository, marker, &temporary_head).await?;

    if marker.stash_hash.is_none() {
        // `stash create` + `stash store` is the script-safe form of the same
        // stash object LazyGit creates with `stash push`. It gives us the
        // immutable object ID before any worktree cleanup, which makes a
        // crash-safe recovery marker possible.
        let stash_hash = git(repository, ["stash", "create"])
            .await?
            .stdout
            .lines()
            .map(str::trim)
            .find(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                ServerError::new(
                    StatusCode::CONFLICT,
                    "There are no unstaged tracked changes left to stash. Abort this recovery after verifying the index.",
                )
        })?;
        let stash_hash = resolve_commit_reference(repository, &stash_hash).await?;
        advanced_validate_expected_head(&stash_hash)?;
        marker.stash_hash = Some(stash_hash);
        // Persist the object before it becomes visible in refs/stash. On a
        // crash after store, continuation can detect the exact OID and avoid
        // adding a duplicate stash entry.
        marker.stash_stored = false;
        management_write_stash_unstaged_recovery_marker(repository, marker).await?;
    }

    management_store_stash_unstaged_if_needed(repository, marker, &temporary_head).await?;
    // The reset must only occur while the original temporary commit remains
    // checked out. This rejects an external branch/HEAD move before a hard
    // reset could discard a different worktree.
    management_validate_stash_unstaged_temporary_commit(repository, marker, &temporary_head)
        .await?;
    // Restore the temporary commit's tree first to remove only the tracked
    // unstaged delta, then move the branch back while retaining that commit's
    // index. This is the same observable result as LazyGit's temporary
    // commit → stash → soft reset sequence; untracked and ignored paths stay
    // untouched because no stash option included them.
    git(repository, ["reset", "--hard", temporary_head.as_str()]).await?;
    management_validate_stash_unstaged_target(repository, marker, &temporary_head).await?;
    git(repository, ["reset", "--soft", marker.original_head.as_str()]).await?;
    management_clear_stash_unstaged_recovery_marker(repository).await?;
    let mut response = management_stash_unstaged_success_response();
    response.operation_state = Some(git_operation_state(repository).await?);
    Ok(response)
}

async fn management_stash_unstaged_response_after_error(
    repository: &Path,
    error: ServerError,
) -> Result<GitOperationResponse> {
    if management_read_stash_unstaged_recovery_marker(repository)
        .await?
        .is_none()
    {
        return Err(error);
    }
    let state = git_operation_state(repository).await?;
    let detail = error
        .body
        .details
        .clone()
        .unwrap_or_else(|| error.body.error.clone());
    let mut response = GitOperationResponse::success(detail);
    response.message = state.message.clone();
    response.operation_state = Some(state);
    Ok(response)
}

async fn management_create_stash_unstaged_only(
    repository: &Path,
    message: Option<String>,
) -> Result<GitOperationResponse> {
    if !repository_has_commits(repository).await? {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Stashing only unstaged changes requires an existing commit. Create the initial commit first.",
        ));
    }
    if !management_stash_unstaged_has_unstaged_changes(repository).await? {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "There are no unstaged tracked changes to stash. Use a staged-only stash or commit the index instead.",
        ));
    }
    if !management_stash_unstaged_has_staged_changes(repository).await? {
        // This is LazyGit's exact fallback: when there is no index to
        // preserve, "stash unstaged changes" becomes an ordinary tracked
        // stash. A temporary commit would add risk without changing the
        // result.
        let mut args = vec!["stash".to_string(), "push".to_string()];
        if let Some(message) = message {
            args.push("--message".to_string());
            args.push(message);
        }
        let output = git_with_default_pathspecs(repository, args).await?;
        let message = "Stashed unstaged changes.";
        let mut response = GitOperationResponse::success(management_output_or(&output, message));
        response.message = Some(message.to_string());
        response.operation_state = Some(git_operation_state(repository).await?);
        return Ok(response);
    }
    let marker = ManagementStashUnstagedRecoveryMarker {
        version: MANAGEMENT_STASH_UNSTAGED_RECOVERY_VERSION,
        original_head: advanced_current_head(repository).await?,
        target_ref: checked_out_target_ref(repository).await?,
        transaction_id: Some(uuid::Uuid::new_v4().to_string()),
        temporary_head: None,
        stash_hash: None,
        stash_stored: false,
        message,
    };
    management_write_stash_unstaged_recovery_marker(repository, &marker).await?;
    let mut marker = marker;
    match management_complete_stash_unstaged_recovery(repository, &mut marker).await {
        Ok(response) => Ok(response),
        Err(error) => management_stash_unstaged_response_after_error(repository, error).await,
    }
}

async fn management_finish_stash_unstaged_recovery(
    repository: &Path,
) -> Result<GitOperationResponse> {
    let mut marker = management_read_stash_unstaged_recovery_marker(repository)
        .await?
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "There is no stash-unstaged recovery to finish",
            )
        })?;
    match management_complete_stash_unstaged_recovery(repository, &mut marker).await {
        Ok(response) => Ok(response),
        Err(error) => management_stash_unstaged_response_after_error(repository, error).await,
    }
}

async fn management_abort_stash_unstaged_recovery(
    repository: &Path,
) -> Result<GitOperationResponse> {
    let mut marker = management_read_stash_unstaged_recovery_marker(repository)
        .await?
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "There is no stash-unstaged recovery to restore",
            )
        })?;
    management_reconcile_stash_unstaged_temporary_head(repository, &mut marker).await?;
    let current_target_ref = checked_out_target_ref(repository).await?;
    if current_target_ref != marker.target_ref {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The checked-out branch changed while stashing unstaged changes. Restore the repository from a terminal instead.",
        ));
    }
    let current_head = advanced_current_head(repository).await?;
    let temporary_head = marker.temporary_head.clone();
    if current_head != marker.original_head
        && temporary_head
            .as_deref()
            .is_none_or(|temporary| current_head != temporary)
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "HEAD changed while stashing unstaged changes. Restore the repository from a terminal instead.",
        ));
    }

    // If the saved stash exists and the unstaged delta is no longer present,
    // the request stopped after worktree cleanup. Reapply that exact stash
    // after the soft reset to reconstruct the user’s original mixed state.
    let restore_saved_delta = marker.stash_stored
        && !management_stash_unstaged_has_unstaged_changes(repository).await?;
    if let Some(temporary_head) = temporary_head.as_deref() {
        if current_head == temporary_head {
            management_validate_stash_unstaged_temporary_commit(
                repository,
                &marker,
                temporary_head,
            )
            .await?;
            git(repository, ["reset", "--soft", marker.original_head.as_str()]).await?;
        }
    }

    if restore_saved_delta {
        let stash_hash = marker
            .stash_hash
            .as_deref()
            .ok_or_else(management_invalid_stash_unstaged_recovery_marker)?;
        let temporary_head = temporary_head
            .as_deref()
            .ok_or_else(management_invalid_stash_unstaged_recovery_marker)?;
        // This validates the immutable stash/temporary-commit relationship
        // after HEAD has been soft-reset to the original commit. Do not use
        // the checked-out-target validator here: that would incorrectly
        // require HEAD to still equal the temporary commit.
        management_validate_stash_unstaged_saved_stash(
            repository,
            &marker,
            stash_hash,
            temporary_head,
        )
        .await?;
        git(repository, ["stash", "apply", "--index", stash_hash]).await?;
    }
    management_clear_stash_unstaged_recovery_marker(repository).await?;
    let message = if marker.stash_hash.is_some() {
        "Original staged and unstaged changes were restored. The recovery stash remains saved for verification."
    } else {
        "The temporary stash preparation was cancelled and the original changes remain in place."
    };
    let mut response = GitOperationResponse::success(message);
    response.message = Some(message.to_string());
    response.operation_state = Some(git_operation_state(repository).await?);
    Ok(response)
}

async fn management_require_pristine_stash_apply(
    repository: &Path,
    snapshot: &ManagementStashSnapshot,
) -> Result<()> {
    // `git stash apply` is not atomic when a dirty worktree contains an
    // overlapping path: Git can restore an untracked file and then fail on a
    // tracked one. Starting from a pristine worktree makes conflict abandon
    // deterministic and prevents this API from silently leaving partial work.
    // Ignored build output is not ordinary worktree dirtiness; reject it only
    // if this particular stash can write over it.
    advanced_require_clean_worktree(repository, "apply or pop a stash").await?;
    let write_paths = management_stash_file_inventory(repository, snapshot)
        .await?
        .into_iter()
        .map(|file| file.path)
        .collect::<Vec<_>>();
    advanced_require_no_ignored_path_collision(
        repository,
        &write_paths,
        "apply or pop a stash",
    )
    .await
}

async fn management_require_pristine_stash_branch(
    repository: &Path,
    snapshot: &ManagementStashSnapshot,
) -> Result<()> {
    // LazyGit creates this branch with `git checkout -b <branch>
    // refs/stash@{N}`. That checks out the stash commit; it does not run
    // `git stash branch`, apply the stash, or drop it. We cannot offer
    // LazyGit's interactive autostash prompt on this API, so fail before a
    // dirty checkout can create a branch and then leave the worktree partial.
    advanced_require_clean_worktree(repository, "create a branch from a stash").await?;

    // Guard ignored paths Git would actually replace while switching from the
    // current commit to the stash commit. Looking only at files stored in the
    // stash is insufficient: an unchanged base file can still be introduced
    // by this checkout if the current branch no longer contains it.
    let write_paths = git(
        repository,
        [
            "diff",
            "--no-ext-diff",
            "--name-only",
            "-z",
            "HEAD",
            &snapshot.hash,
        ],
    )
    .await?
    .stdout_bytes;
    let write_paths = advanced_paths_from_nul_output(&write_paths);
    advanced_require_no_ignored_path_collision(
        repository,
        &write_paths,
        "create a branch from a stash",
    )
    .await
}

async fn management_stash_recovery_marker_for_snapshot(
    repository: &Path,
    snapshot: &ManagementStashSnapshot,
    pop: bool,
) -> Result<ManagementStashRecoveryMarker> {
    let untracked_paths = management_stash_file_inventory(repository, snapshot)
        .await?
        .into_iter()
        .filter(|file| file.source == ManagementStashFileSource::Untracked)
        .map(|file| file.path)
        .collect();
    Ok(ManagementStashRecoveryMarker {
        version: MANAGEMENT_STASH_RECOVERY_VERSION,
        reference: snapshot.reference.clone(),
        stash_hash: snapshot.hash.clone(),
        pop,
        untracked_paths,
    })
}

async fn management_apply_stash_with_recovery(
    repository: &Path,
    reference: &str,
    index: bool,
    pop: bool,
    expected_hash: Option<&str>,
) -> Result<GitOperationResponse> {
    ensure_no_git_workflow_in_progress(repository).await?;
    let snapshot =
        management_stash_snapshot_for_action(repository, reference, expected_hash).await?;
    management_require_pristine_stash_apply(repository, &snapshot).await?;
    let recovery_marker =
        management_stash_recovery_marker_for_snapshot(repository, &snapshot, pop).await?;

    let mut args = vec![
        "stash".to_string(),
        if pop { "pop" } else { "apply" }.to_string(),
    ];
    if index {
        args.push("--index".to_string());
    }
    args.push(snapshot.reference.clone());

    match git(repository, args).await {
        Ok(output) => Ok(GitOperationResponse::success(management_output_or(
            &output,
            if pop {
                "Stash popped successfully"
            } else {
                "Stash applied successfully"
            },
        ))),
        Err(error) => {
            if management_stash_conflicted_paths(repository).await?.is_empty() {
                return Err(error);
            }
            management_write_stash_recovery_marker(repository, &recovery_marker).await?;
            let operation_state = git_operation_state(repository).await?;
            let mut response = GitOperationResponse::success(
                error
                    .body
                    .details
                    .clone()
                    .unwrap_or_else(|| error.body.error.clone()),
            );
            response.message = Some(if pop {
                "Stash pop has conflicts. Resolve and stage every conflicted path, then finish stash recovery. The stash is retained until you verify and drop it explicitly."
                    .to_string()
            } else {
                "Stash apply has conflicts. Resolve and stage every conflicted path, then finish stash recovery."
                    .to_string()
            });
            response.operation_state = Some(operation_state);
            Ok(response)
        }
    }
}

async fn management_finish_stash_recovery(repository: &Path) -> Result<GitOperationResponse> {
    let marker = management_read_stash_recovery_marker(repository)
        .await?
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "There is no stash recovery to finish",
            )
        })?;
    let conflicts = management_stash_conflicted_paths(repository).await?;
    if !conflicts.is_empty() {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "Resolve and stage all {} conflicted stash path(s) before finishing recovery",
                conflicts.len()
            ),
        ));
    }
    management_clear_stash_recovery_marker(repository).await?;
    let message = if marker.pop {
        "Stash conflicts are resolved. The stash was retained after the conflicted pop so you can verify the result; drop it explicitly when ready."
    } else {
        "Stash conflicts are resolved. The stash remains available for future use."
    };
    let mut response = GitOperationResponse::success(message);
    response.message = Some(message.to_string());
    response.operation_state = Some(git_operation_state(repository).await?);
    Ok(response)
}

async fn management_remove_stash_recovery_untracked_paths(
    repository: &Path,
    paths: &[String],
) -> Result<()> {
    for path in paths {
        // Do not remove a path which has become tracked while the user was
        // resolving the conflict. Abandoning recovery must never delete a
        // newly committed or staged file.
        if git(repository, ["ls-files", "--error-unmatch", "--", path]).await.is_ok() {
            continue;
        }
        let target = safe_repo_child(repository, path)
            .map_err(|_| management_invalid_stash_recovery_marker())?;
        let metadata = match fs::symlink_metadata(&target).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_server_error(error)),
        };
        // `ls-tree -r` inventories file-like entries only. Refuse to recurse
        // if a path was later replaced with a directory, rather than deleting
        // content which was not part of the original stash application.
        if metadata.is_dir() {
            continue;
        }
        fs::remove_file(target).await.map_err(io_server_error)?;
    }
    Ok(())
}

async fn management_abort_stash_recovery(repository: &Path) -> Result<GitOperationResponse> {
    let marker = management_read_stash_recovery_marker(repository)
        .await?
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "There is no stash recovery to abandon",
            )
        })?;
    if management_stash_conflicted_paths(repository).await?.is_empty() {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "No unresolved stash conflicts remain. Finish stash recovery instead of abandoning it.",
        ));
    }
    // The shared apply helper only begins from a pristine worktree. A hard
    // reset therefore returns every tracked path and the index to the exact
    // pre-apply HEAD without risking unrelated user changes.
    git(repository, ["reset", "--hard", "HEAD"]).await?;
    management_remove_stash_recovery_untracked_paths(repository, &marker.untracked_paths).await?;
    management_clear_stash_recovery_marker(repository).await?;
    let message = "Stash recovery was abandoned and the pristine pre-apply worktree was restored.";
    let mut response = GitOperationResponse::success(message);
    response.message = Some(message.to_string());
    response.operation_state = Some(git_operation_state(repository).await?);
    Ok(response)
}

async fn management_stash_tracked_file_diff(
    repository: &Path,
    snapshot: &ManagementStashSnapshot,
    path: &str,
) -> Result<String> {
    Ok(git(
        repository,
        [
            "diff",
            "--no-ext-diff",
            "--no-renames",
            "--full-index",
            &snapshot.base_hash,
            &snapshot.hash,
            "--",
            path,
        ],
    )
    .await?
    .stdout)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashFileDiffQuery {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    reference: String,
    file: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashFileDiffResponse {
    reference: String,
    path: String,
    diff: String,
    is_truncated: bool,
    can_restore_hunks: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

async fn management_stash_file_diff(
    State(state): State<AppState>,
    Query(query): Query<ManagementStashFileDiffQuery>,
) -> Result<Json<ManagementStashFileDiffResponse>> {
    let repository = management_repository(&state, &query.scope).await?;
    let snapshot = management_stash_snapshot(&repository, &query.reference).await?;
    let path = management_stash_file_target(&repository, &query.file).await?;
    let files = management_stash_file_inventory(&repository, &snapshot).await?;
    let file = files
        .iter()
        .find(|file| file.path == path)
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "File is not stored in this stash"))?;
    if file.source == ManagementStashFileSource::Untracked {
        return Ok(Json(ManagementStashFileDiffResponse {
            reference: snapshot.reference,
            path,
            diff: String::new(),
            is_truncated: false,
            can_restore_hunks: false,
            message: Some(
                "Hunk restore is unavailable for untracked stash content; restore this file explicitly instead."
                    .to_string(),
            ),
        }));
    }

    let full_diff = management_stash_tracked_file_diff(&repository, &snapshot, &path).await?;
    let (diff, is_truncated) = management_truncate_output(full_diff);
    let can_restore_hunks = !is_truncated && diff.lines().any(|line| line.starts_with("@@"));
    Ok(Json(ManagementStashFileDiffResponse {
        reference: snapshot.reference,
        path,
        diff,
        is_truncated,
        can_restore_hunks,
        message: (!can_restore_hunks).then(|| {
            "This change has no selectable text hunks. Restore the full file to apply it."
                .to_string()
        }),
    }))
}

async fn management_ensure_stash_restore_is_safe(repository: &Path) -> Result<()> {
    ensure_no_git_workflow_in_progress(repository).await?;
    if !management_stash_conflicted_paths(repository).await?.is_empty() {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Resolve existing Git conflicts before restoring selected stash content",
        ));
    }
    Ok(())
}

/// `git stash apply` does not leave one of Git's normal operation markers
/// behind when it conflicts.  It does, however, leave unmerged index entries.
/// Check those explicitly before a selective restore so we never combine two
/// unrelated conflict recoveries in the same worktree.
async fn management_stash_conflicted_paths(repository: &Path) -> Result<Vec<String>> {
    let output = git(
        repository,
        [
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=no",
            "--ignore-submodules=none",
        ],
    )
    .await?;
    Ok(parse_status_entries_detailed_bytes(&output.stdout_bytes)
        .into_iter()
        .filter_map(|(status, path, _)| is_conflict_status(&status).then_some(path))
        .collect())
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashRestoreFilesBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    reference: String,
    expected_hash: Option<String>,
    files: Vec<String>,
    #[serde(default)]
    confirm: bool,
}

async fn management_restore_stash_files(
    State(state): State<AppState>,
    Json(body): Json<ManagementStashRestoreFilesBody>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "restore selected stash files over the working tree")?;
    if body.files.is_empty() || body.files.len() > MANAGEMENT_MAX_STASH_RESTORE_FILES {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "Choose between one and {MANAGEMENT_MAX_STASH_RESTORE_FILES} stash files to restore"
            ),
        ));
    }
    let repository = management_repository(&state, &body.scope).await?;
    management_ensure_stash_restore_is_safe(&repository).await?;
    let snapshot = management_stash_snapshot_for_action(
        &repository,
        &body.reference,
        body.expected_hash.as_deref(),
    )
    .await?;
    let inventory = management_stash_file_inventory(&repository, &snapshot).await?;
    let mut selected = Vec::with_capacity(body.files.len());
    let mut seen = HashSet::new();
    for requested_path in &body.files {
        let path = management_stash_file_target(&repository, requested_path).await?;
        if !seen.insert(path.clone()) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Each stash file may only be restored once per request",
            ));
        }
        let file = inventory
            .iter()
            .find(|file| file.path == path)
            .cloned()
            .ok_or_else(|| {
                ServerError::new(StatusCode::NOT_FOUND, "File is not stored in this stash")
            })?;
        selected.push(file);
    }

    let tracked = selected
        .iter()
        .filter(|file| file.source == ManagementStashFileSource::Tracked)
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let untracked = selected
        .iter()
        .filter(|file| file.source == ManagementStashFileSource::Untracked)
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    management_restore_stash_paths_from_tree(&repository, &snapshot.hash, &tracked).await?;
    if let Some(untracked_tree) = &snapshot.untracked_tree {
        management_restore_stash_paths_from_tree(&repository, untracked_tree, &untracked).await?;
    }

    Ok(Json(GitOperationResponse::message(format!(
        "Restored {} selected stash file(s) to the working tree. The index was unchanged.",
        selected.len()
    ))))
}

async fn management_restore_stash_paths_from_tree(
    repository: &Path,
    source_tree: &str,
    paths: &[String],
) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = vec![
        "restore".to_string(),
        format!("--source={source_tree}"),
        "--worktree".to_string(),
        "--".to_string(),
    ];
    args.extend(paths.iter().cloned());
    git(repository, args).await?;
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashRestoreHunksBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    reference: String,
    expected_hash: Option<String>,
    file: String,
    hunk_indexes: Vec<usize>,
    #[serde(default)]
    confirm: bool,
}

async fn management_restore_stash_hunks(
    State(state): State<AppState>,
    Json(body): Json<ManagementStashRestoreHunksBody>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "restore selected stash hunks over the working tree")?;
    if body.hunk_indexes.is_empty() || body.hunk_indexes.len() > MANAGEMENT_MAX_STASH_HUNK_SELECTIONS {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "Choose between one and {MANAGEMENT_MAX_STASH_HUNK_SELECTIONS} stash hunks to restore"
            ),
        ));
    }
    let repository = management_repository(&state, &body.scope).await?;
    management_ensure_stash_restore_is_safe(&repository).await?;
    let snapshot = management_stash_snapshot_for_action(
        &repository,
        &body.reference,
        body.expected_hash.as_deref(),
    )
    .await?;
    let path = management_stash_file_target(&repository, &body.file).await?;
    let inventory = management_stash_file_inventory(&repository, &snapshot).await?;
    let file = inventory
        .iter()
        .find(|file| file.path == path)
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "File is not stored in this stash"))?;
    if file.source != ManagementStashFileSource::Tracked {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Hunk restore is only available for tracked stash changes; restore the full untracked file instead",
        ));
    }

    let diff = management_stash_tracked_file_diff(&repository, &snapshot, &path).await?;
    if diff.chars().count() > MANAGEMENT_OUTPUT_CHARACTER_LIMIT {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "This stash diff is too large for safe hunk restoration; restore the full file instead",
        ));
    }
    let patch = selected_hunk_patch(&diff, &body.hunk_indexes)?;
    management_apply_stash_patch_to_worktree(&repository, &patch).await?;
    Ok(Json(GitOperationResponse::message(format!(
        "Restored {} selected stash hunk(s) to {}. The index was unchanged.",
        body.hunk_indexes.len(),
        path
    ))))
}

async fn management_apply_stash_patch_to_worktree(repository: &Path, patch: &str) -> Result<()> {
    if let Err(details) = management_run_worktree_patch(repository, patch, true).await {
        return Err(ServerError::with_details(
            StatusCode::CONFLICT,
            "Selected stash hunks no longer apply cleanly",
            format!(
                "No changes were made. Refresh the file, resolve its current changes, or use explicit full-file restore after confirmation.\n{details}"
            ),
        ));
    }
    if let Err(details) = management_run_worktree_patch(repository, patch, false).await {
        return Err(ServerError::with_details(
            StatusCode::CONFLICT,
            "Selected stash hunks could not be applied",
            format!(
                "The working tree changed after preflight. Refresh it before retrying; no index changes were made.\n{details}"
            ),
        ));
    }
    Ok(())
}

async fn management_run_worktree_patch(
    repository: &Path,
    patch: &str,
    check_only: bool,
) -> std::result::Result<(), String> {
    let mut command = Command::new("git");
    command
        .arg("apply")
        .arg("--recount")
        .current_dir(repository)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if check_only {
        command.arg("--check");
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start git apply: {error}"))?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err("Could not open git apply input".to_string());
    };
    stdin
        .write_all(patch.as_bytes())
        .await
        .map_err(|error| format!("Could not write the selected patch: {error}"))?;
    drop(stdin);
    let output = child
        .wait_with_output()
        .await
        .map_err(|error| format!("Could not wait for git apply: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(redact_git_diagnostic(&format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashCreateBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    message: Option<String>,
    /// Advanced stash has historically included untracked files by default.
    /// LazyGit's dedicated "stash unstaged" action is different: with this
    /// option omitted it performs a normal tracked-only stash when the index
    /// is empty. Keep the wire default contextual instead of making callers
    /// know this implementation detail.
    include_untracked: Option<bool>,
    #[serde(default)]
    include_ignored: bool,
    #[serde(default)]
    keep_index: bool,
    #[serde(default)]
    staged_only: bool,
    /// Match LazyGit’s "stash unstaged changes" action. This is deliberately
    /// distinct from `--keep-index`: when a file has both staged and
    /// unstaged edits, the saved stash contains only the unstaged delta.
    #[serde(default)]
    unstaged_only: bool,
    #[serde(default)]
    confirm: bool,
}

async fn management_create_stash(
    State(state): State<AppState>,
    Json(body): Json<ManagementStashCreateBody>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "stash and remove working-tree changes")?;
    let include_untracked = body.include_untracked.unwrap_or(!body.unstaged_only);
    if body.staged_only && (include_untracked || body.include_ignored) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "A staged-only stash cannot include untracked or ignored files",
        ));
    }
    if body.staged_only && body.keep_index {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "A staged-only stash cannot also keep the index",
        ));
    }
    if body.unstaged_only
        && (include_untracked || body.include_ignored || body.keep_index || body.staged_only)
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Stash unstaged changes cannot include untracked or ignored files, keep the index, or combine with staged-only stash",
        ));
    }
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let message = validate_optional_git_message(body.message.as_deref())?;
    if body.unstaged_only {
        return Ok(Json(
            management_create_stash_unstaged_only(&repository, message).await?,
        ));
    }
    let mut args = vec!["stash".to_string(), "push".to_string()];
    if body.staged_only {
        args.push("--staged".to_string());
    } else {
        if body.include_ignored {
            args.push("--all".to_string());
        } else if include_untracked {
            args.push("--include-untracked".to_string());
        }
        if body.keep_index {
            args.push("--keep-index".to_string());
        }
    }
    if let Some(message) = message {
        args.push("--message".to_string());
        args.push(message);
    }
    // `stash push --include-untracked` needs normal Git pathspec semantics
    // for its own internal cleanup. All arguments here are server-owned
    // options or explicitly-delimited message text, never a requested path.
    let output = git_with_default_pathspecs(&repository, args).await?;
    Ok(Json(GitOperationResponse::success(management_output_or(
        &output,
        "Stash saved successfully",
    ))))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashApplyBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    reference: String,
    expected_hash: Option<String>,
    #[serde(default)]
    index: bool,
    #[serde(default)]
    pop: bool,
    #[serde(default)]
    confirm: bool,
}

async fn management_apply_stash(
    State(state): State<AppState>,
    Json(body): Json<ManagementStashApplyBody>,
) -> Result<Json<GitOperationResponse>> {
    if body.pop {
        require_management_confirmation(body.confirm, "pop a stash")?;
    }
    let repository = management_repository(&state, &body.scope).await?;
    let reference = validate_stash_ref(&body.reference)?;
    Ok(Json(
        management_apply_stash_with_recovery(
            &repository,
            &reference,
            body.index,
            body.pop,
            body.expected_hash.as_deref(),
        )
        .await?,
    ))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashBranchBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    branch: String,
    reference: String,
    expected_hash: Option<String>,
    #[serde(default)]
    confirm: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStashRenameBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    reference: String,
    /// The full object ID displayed with the selected row. A stash selector
    /// such as `stash@{1}` is positional and can point to another entry once
    /// a new stash is added, so never rename based on that selector alone.
    expected_hash: String,
    message: String,
}

fn management_stash_reference_index(reference: &str) -> Result<usize> {
    let reference = validate_stash_ref(reference)?;
    reference
        .strip_prefix("stash@{")
        .and_then(|value| value.strip_suffix('}'))
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Invalid stash reference"))
}

fn management_expected_stash_hash(value: &str) -> Result<&str> {
    let value = value.trim();
    // Stash rows return a complete object ID. Reject a selector or shortened
    // hash here so this optimistic-concurrency check cannot be redirected by
    // later ref movement or by an ambiguous abbreviation.
    if !matches!(value.len(), 40 | 64)
        || !value.chars().all(|character| character.is_ascii_hexdigit())
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "expectedHash must be the full immutable stash hash returned by the stash list",
        ));
    }
    Ok(value)
}

fn management_required_stash_message(value: &str) -> Result<String> {
    validate_optional_git_message(Some(value))?.ok_or_else(|| {
        ServerError::new(StatusCode::BAD_REQUEST, "A non-empty stash name is required")
    })
}

async fn management_stash_reflog_message(repository: &Path, reference: &str) -> Result<String> {
    let output = git(
        repository,
        ["stash", "list", "--format=%gs", "-1", reference],
    )
    .await?;
    output
        .stdout
        .lines()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::CONFLICT,
                "The selected stash no longer has a reflog message. Refresh the stash list and try again.",
            )
        })
}

async fn management_stash_entry_count(repository: &Path) -> Result<usize> {
    let output = git(repository, ["stash", "list", "--format=%H"]).await?;
    Ok(output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .count())
}

async fn management_store_stash(
    repository: &Path,
    stash_hash: &str,
    message: &str,
) -> Result<GitOutput> {
    // `stash store` is the porcelain LazyGit uses for rename. It preserves
    // the stash commit (and therefore its tracked/index/untracked trees),
    // while the new reflog entry supplies the visible name.
    git(repository, ["stash", "store", "-m", message, stash_hash]).await
}

async fn management_stash_rename_backup_ref(
    repository: &Path,
    stash_hash: &str,
) -> Result<String> {
    // Git cannot append a duplicate of the current `refs/stash` OID, so a
    // top-entry rename must temporarily drop it before storing it with the
    // new reflog message. Keep a private ref first so an interrupted or
    // failed store cannot make the stash object unreachable.
    let reference = format!("refs/iowb/stash-rename/{}", uuid::Uuid::new_v4().simple());
    git(repository, ["update-ref", &reference, stash_hash]).await?;
    Ok(reference)
}

async fn management_clear_stash_rename_backup(
    repository: &Path,
    reference: &str,
    stash_hash: &str,
) -> Result<()> {
    // Include the expected OID so a hypothetical external process cannot
    // repoint this temporary ref and have the cleanup delete its value.
    git(repository, ["update-ref", "-d", reference, stash_hash])
        .await
        .map(|_| ())
}

async fn management_rename_stash(
    State(state): State<AppState>,
    Json(body): Json<ManagementStashRenameBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let reference = validate_stash_ref(&body.reference)?;
    let selected_index = management_stash_reference_index(&reference)?;
    let expected_hash = management_expected_stash_hash(&body.expected_hash)?;
    let message = management_required_stash_message(&body.message)?;
    let snapshot = management_stash_snapshot(&repository, &reference).await?;
    if !snapshot.hash.eq_ignore_ascii_case(expected_hash) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The selected stash changed since this rename was prepared. Refresh the stash list and try again.",
        ));
    }
    let previous_message = management_stash_reflog_message(&repository, &snapshot.reference).await?;

    if selected_index > 0 {
        // Unlike the newest stash, an older selected entry can be stored
        // first. That gives us a durable renamed copy before removing the old
        // reflog entry, which is safer than LazyGit's drop-then-store order.
        let entry_count = management_stash_entry_count(&repository).await?;
        management_store_stash(&repository, &snapshot.hash, &message).await?;
        let expected_entry_count = entry_count.checked_add(1).ok_or_else(|| {
            ServerError::new(StatusCode::BAD_REQUEST, "Too many stash entries to rename safely")
        })?;
        if management_stash_entry_count(&repository).await? != expected_entry_count {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "The selected stash is already duplicated or the stash list changed while renaming. Refresh the stash list before trying again.",
            ));
        }
        let stored = management_stash_snapshot(&repository, "stash@{0}").await?;
        let original_index = selected_index.checked_add(1).ok_or_else(|| {
            ServerError::new(StatusCode::BAD_REQUEST, "Stash reference is too large")
        })?;
        let original_reference = format!("stash@{{{original_index}}}");
        let original = management_stash_snapshot(&repository, &original_reference).await?;
        if stored.hash != snapshot.hash || original.hash != snapshot.hash {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "The stash list changed while renaming. The renamed copy is retained safely at the top; refresh before removing any duplicate entry.",
            ));
        }
        git(&repository, ["stash", "drop", &original.reference]).await?;
    } else {
        // `git stash store` deliberately ignores a duplicate of its current
        // top entry. Protect the original OID, then use the same drop/store
        // sequence LazyGit uses. If storing the new name fails, put the
        // original entry back before returning the error.
        let backup_reference = management_stash_rename_backup_ref(&repository, &snapshot.hash).await?;
        if let Err(error) = git(&repository, ["stash", "drop", &snapshot.reference]).await {
            let _ = management_clear_stash_rename_backup(
                &repository,
                &backup_reference,
                &snapshot.hash,
            )
            .await;
            return Err(error);
        }
        if let Err(error) = management_store_stash(&repository, &snapshot.hash, &message).await {
            match management_store_stash(&repository, &snapshot.hash, &previous_message).await {
                Ok(_) => {
                    let _ = management_clear_stash_rename_backup(
                        &repository,
                        &backup_reference,
                        &snapshot.hash,
                    )
                    .await;
                    return Err(ServerError::with_details(
                        error.status,
                        "Stash rename failed; the original stash was restored",
                        error
                            .body
                            .details
                            .clone()
                            .unwrap_or_else(|| error.body.error.clone()),
                    ));
                }
                Err(restore_error) => {
                    return Err(ServerError::with_details(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Could not rename the stash safely; its content is retained for recovery",
                        format!(
                            "The original stash is protected by a server recovery ref. Rename failure: {}; restore failure: {}",
                            error.body.error, restore_error.body.error,
                        ),
                    ));
                }
            }
        }
        // A failed cleanup leaves only an internal ref pointing at the same
        // already-renamed stash object. Treat the visible rename as complete
        // rather than asking the user to retry and accidentally create a
        // duplicate stash entry.
        let _ = management_clear_stash_rename_backup(
            &repository,
            &backup_reference,
            &snapshot.hash,
        )
        .await;
    }

    let renamed = management_stash_snapshot(&repository, "stash@{0}").await?;
    if renamed.hash != snapshot.hash {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The renamed stash is no longer at the top of the stash list. Refresh the stash list before continuing.",
        ));
    }
    Ok(Json(GitOperationResponse::success("Stash renamed successfully")))
}

async fn management_create_branch_from_stash(
    State(state): State<AppState>,
    Json(body): Json<ManagementStashBranchBody>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "create a branch from a stash")?;
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let branch = validate_branch_name(&body.branch)?;
    let snapshot = management_stash_snapshot_for_action(
        &repository,
        &body.reference,
        body.expected_hash.as_deref(),
    )
    .await?;
    management_require_pristine_stash_branch(&repository, &snapshot).await?;
    // Match LazyGit exactly: its Stash → New branch action branches directly
    // from the stash ref, preserving that stash entry for later inspection or
    // application. `git stash branch` would instead apply and drop it.
    let stash_ref = format!("refs/{}", snapshot.reference);
    // `checkout` overwrites ignored paths by default.  A stash can contain a
    // now-tracked path that collides with local ignored output, so preserve the
    // same protection used by every mobile checkout route.
    let output = git(
        &repository,
        [
            "checkout",
            "--no-overwrite-ignore",
            "-b",
            &branch,
            &stash_ref,
        ],
    )
    .await?;
    let mut response = GitOperationResponse::success(management_output_or(
        &output,
        "Branch created at stash successfully",
    ));
    response.branch = Some(branch);
    Ok(Json(response))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementTagCreateBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    tag: String,
    message: Option<String>,
    target: Option<String>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    confirm: bool,
}

async fn management_create_tag_at_target(
    State(state): State<AppState>,
    Json(body): Json<ManagementTagCreateBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    if body.force {
        require_management_confirmation(body.confirm, "replace an existing local tag")?;
    }
    let tag = validate_tag_name(&body.tag)?;
    ensure_new_local_tag_or_explicit_replacement(&repository, &tag, body.force).await?;
    let message = validate_optional_git_message(body.message.as_deref())?;
    let target =
        resolve_commit_reference(&repository, body.target.as_deref().unwrap_or("HEAD")).await?;
    let mut args = vec!["tag".to_string()];
    if body.force {
        args.push("--force".to_string());
    }
    if let Some(message) = message {
        args.push("--annotate".to_string());
        args.push("--message".to_string());
        args.push(message);
    } else {
        // A blank message is explicitly a lightweight tag in the native
        // dialog. Do not inherit a host/global tag.gpgSign setting here: that
        // would unexpectedly open a signed annotated-tag workflow despite
        // the user choosing the lightweight form.
        args.push("--no-sign".to_string());
    }
    args.push(tag.clone());
    args.push(target);
    let output = git(&repository, args).await?;
    Ok(Json(GitOperationResponse::success(management_output_or(
        &output,
        "Tag created successfully",
    ))))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemoteTagDeleteBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    remote: String,
    tag: String,
    #[serde(default)]
    confirm: bool,
}

/// A tag has no upstream of its own, so a push must always carry the selected
/// remote.  This deliberately does not fall back to `origin` or the first
/// configured remote: a repository can legitimately publish branches and
/// releases to different destinations.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRemoteTagPushBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    remote: String,
    tag: String,
}

async fn management_push_remote_tag(
    State(state): State<AppState>,
    Json(body): Json<ManagementRemoteTagPushBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let remote = validate_remote_name(&body.remote)?;
    let tag = validate_tag_name(&body.tag)?;
    management_remote_exists(&repository, &remote).await?;
    let remote_ref = format!("refs/tags/{tag}");
    let output = git(&repository, ["push", &remote, &remote_ref]).await?;
    let mut response = GitOperationResponse::success(management_output_or(
        &output,
        "Tag pushed successfully",
    ));
    response.remote_name = Some(remote);
    Ok(Json(response))
}

async fn management_delete_remote_tag(
    State(state): State<AppState>,
    Json(body): Json<ManagementRemoteTagDeleteBody>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "delete a remote tag")?;
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let remote = validate_remote_name(&body.remote)?;
    let tag = validate_tag_name(&body.tag)?;
    management_remote_exists(&repository, &remote).await?;
    let remote_ref = format!("refs/tags/{tag}");
    let output = git(&repository, ["push", &remote, "--delete", &remote_ref]).await?;
    let mut response = GitOperationResponse::success(management_output_or(
        &output,
        "Remote tag deleted successfully",
    ));
    response.remote_name = Some(remote);
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// History and recovery

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementFileLogQuery {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    file: String,
    reference: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementCommitsResponse {
    commits: Vec<GitCommitSummary>,
}

fn management_parse_commit_summary(line: &str) -> Option<GitCommitSummary> {
    let mut parts = line.splitn(5, '\u{1f}');
    Some(GitCommitSummary {
        hash: parts.next()?.to_string(),
        author: parts.next()?.to_string(),
        email: parts.next()?.to_string(),
        date: parts.next()?.to_string(),
        message: parts.next()?.to_string(),
        stats: String::new(),
    })
}

async fn management_history_file_target(
    repository: &Path,
    file: &str,
) -> Result<ResolvedRepositoryFile> {
    // Resolve through the existing repository-boundary guard so an arbitrary
    // history request cannot inspect files from an independent nested repo.
    resolve_git_file_target(repository, file, GitFileTargetPolicy::Inspect).await
}

async fn management_file_log(
    State(state): State<AppState>,
    Query(query): Query<ManagementFileLogQuery>,
) -> Result<Json<ManagementCommitsResponse>> {
    let repository = management_repository(&state, &query.scope).await?;
    let file = management_history_file_target(&repository, &query.file).await?;
    let reference = match query.reference.as_deref() {
        Some(value) => Some(resolve_commit_reference(&file.repository_root, value).await?),
        None => None,
    };
    let limit = query
        .limit
        .unwrap_or(50)
        .clamp(1, MANAGEMENT_MAX_HISTORY_LIMIT);
    let mut args = vec![
        "log".to_string(),
        "--follow".to_string(),
        "--format=%H%x1f%an%x1f%ae%x1f%aI%x1f%s".to_string(),
        "-n".to_string(),
        limit.to_string(),
    ];
    if let Some(reference) = reference {
        args.push(reference);
    }
    args.push("--".to_string());
    args.push(file.repository_relative_file);
    let output = git(&file.repository_root, args).await?.stdout;
    let commits = output
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(management_parse_commit_summary)
        .collect();
    Ok(Json(ManagementCommitsResponse { commits }))
}

#[derive(Debug, Clone)]
struct ManagementBlameMetadata {
    author: String,
    author_email: String,
    author_time: String,
    summary: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementBlameLine {
    hash: String,
    original_line: usize,
    final_line: usize,
    author: String,
    author_email: String,
    author_time: String,
    summary: String,
    content: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementBlameResponse {
    lines: Vec<ManagementBlameLine>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementBlameQuery {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    file: String,
    reference: Option<String>,
    start_line: Option<usize>,
    end_line: Option<usize>,
}

#[derive(Debug)]
struct ManagementPendingBlameLine {
    hash: String,
    original_line: usize,
    final_line: usize,
    metadata: ManagementBlameMetadata,
}

fn management_parse_blame_header(value: &str) -> Option<(String, usize, usize)> {
    let mut values = value.split_whitespace();
    let hash = values.next()?;
    let is_object_id = (40..=64).contains(&hash.len())
        && hash.chars().all(|character| character.is_ascii_hexdigit());
    if !is_object_id {
        return None;
    }
    Some((
        hash.to_string(),
        values.next()?.parse().ok()?,
        values.next()?.parse().ok()?,
    ))
}

fn management_parse_blame(value: &str) -> Vec<ManagementBlameLine> {
    let mut cache = std::collections::HashMap::<String, ManagementBlameMetadata>::new();
    let mut current: Option<ManagementPendingBlameLine> = None;
    let mut lines = Vec::new();

    for value in value.lines() {
        if let Some(content) = value.strip_prefix('\t') {
            if let Some(current) = current.take() {
                cache.insert(current.hash.clone(), current.metadata.clone());
                lines.push(ManagementBlameLine {
                    hash: current.hash,
                    original_line: current.original_line,
                    final_line: current.final_line,
                    author: current.metadata.author,
                    author_email: current.metadata.author_email,
                    author_time: current.metadata.author_time,
                    summary: current.metadata.summary,
                    content: content.to_string(),
                });
            }
            continue;
        }

        if let Some((hash, original_line, final_line)) = management_parse_blame_header(value) {
            let metadata = cache
                .get(&hash)
                .cloned()
                .unwrap_or(ManagementBlameMetadata {
                    author: String::new(),
                    author_email: String::new(),
                    author_time: String::new(),
                    summary: String::new(),
                });
            current = Some(ManagementPendingBlameLine {
                hash,
                original_line,
                final_line,
                metadata,
            });
            continue;
        }

        let Some(current) = current.as_mut() else {
            continue;
        };
        if let Some(author) = value.strip_prefix("author ") {
            current.metadata.author = author.to_string();
        } else if let Some(email) = value.strip_prefix("author-mail ") {
            current.metadata.author_email = email
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string();
        } else if let Some(time) = value.strip_prefix("author-time ") {
            current.metadata.author_time = time.to_string();
        } else if let Some(summary) = value.strip_prefix("summary ") {
            current.metadata.summary = summary.to_string();
        }
    }
    lines
}

fn management_blame_range(
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<(usize, usize)> {
    let start = start_line.unwrap_or(1);
    let end = end_line.unwrap_or_else(|| start.saturating_add(199));
    if start == 0 || end < start || end - start + 1 > MANAGEMENT_MAX_BLAME_LINES {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "Blame line range must contain between 1 and {MANAGEMENT_MAX_BLAME_LINES} lines"
            ),
        ));
    }
    Ok((start, end))
}

async fn management_blame(
    State(state): State<AppState>,
    Query(query): Query<ManagementBlameQuery>,
) -> Result<Json<ManagementBlameResponse>> {
    let repository = management_repository(&state, &query.scope).await?;
    let file = management_history_file_target(&repository, &query.file).await?;
    let (start, end) = management_blame_range(query.start_line, query.end_line)?;
    let reference = match query.reference.as_deref() {
        Some(value) => Some(resolve_commit_reference(&file.repository_root, value).await?),
        None => None,
    };
    let mut args = vec![
        "blame".to_string(),
        "--line-porcelain".to_string(),
        "-L".to_string(),
        format!("{start},{end}"),
    ];
    if let Some(reference) = reference {
        args.push(reference);
    }
    args.push("--".to_string());
    args.push(file.repository_relative_file);
    let output = git(&file.repository_root, args).await?.stdout;
    Ok(Json(ManagementBlameResponse {
        lines: management_parse_blame(&output),
    }))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementReflogQuery {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    reference: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementReflogEntry {
    hash: String,
    selector: String,
    actor: String,
    email: String,
    message: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementReflogResponse {
    entries: Vec<ManagementReflogEntry>,
}

fn management_validate_reflog_reference(value: &str) -> Result<String> {
    let value = value.trim();
    if value == "HEAD" {
        return Ok(value.to_string());
    }
    validate_git_ref_name(value, "Invalid reflog reference")
}

fn management_parse_reflog_entry(value: &str) -> Option<ManagementReflogEntry> {
    let mut values = value.splitn(5, '\u{1f}');
    Some(ManagementReflogEntry {
        hash: values.next()?.to_string(),
        selector: values.next()?.to_string(),
        actor: values.next()?.to_string(),
        email: values.next()?.to_string(),
        message: values.next()?.to_string(),
    })
}

async fn management_reflog(
    State(state): State<AppState>,
    Query(query): Query<ManagementReflogQuery>,
) -> Result<Json<ManagementReflogResponse>> {
    let repository = management_repository(&state, &query.scope).await?;
    let reference =
        management_validate_reflog_reference(query.reference.as_deref().unwrap_or("HEAD"))?;
    let limit = query
        .limit
        .unwrap_or(50)
        .clamp(1, MANAGEMENT_MAX_HISTORY_LIMIT);
    let output = git(
        &repository,
        [
            "reflog",
            "show",
            "--date=iso-strict",
            "--format=%H%x1f%gd%x1f%gN%x1f%gE%x1f%gs",
            "-n",
            &limit.to_string(),
            &reference,
        ],
    )
    .await?
    .stdout;
    let entries = output
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(management_parse_reflog_entry)
        .collect();
    Ok(Json(ManagementReflogResponse { entries }))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementCompareRefsBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    base: String,
    head: String,
    file: Option<String>,
}

async fn management_compare_refs(
    State(state): State<AppState>,
    Json(body): Json<ManagementCompareRefsBody>,
) -> Result<Json<GitDiffResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    let base = resolve_commit_reference(&repository, &body.base).await?;
    let head = resolve_commit_reference(&repository, &body.head).await?;
    let mut args = vec![
        "diff".to_string(),
        "--submodule=log".to_string(),
        base,
        head,
    ];
    let repository = if let Some(file) = body.file.as_deref() {
        let file = management_history_file_target(&repository, file).await?;
        args.push("--".to_string());
        args.push(file.repository_relative_file);
        file.repository_root
    } else {
        repository
    };
    let output = git(&repository, args).await?.stdout;
    let (diff, is_truncated) = management_truncate_output(output);
    Ok(Json(GitDiffResponse { diff, is_truncated }))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRestoreFileBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    file: String,
    source: Option<String>,
    #[serde(default)]
    stage: bool,
    #[serde(default)]
    confirm: bool,
}

async fn management_restore_file(
    State(state): State<AppState>,
    Json(body): Json<ManagementRestoreFileBody>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "replace a file from history")?;
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let file = management_history_file_target(&repository, &body.file).await?;
    let source = resolve_commit_reference(
        &file.repository_root,
        body.source.as_deref().unwrap_or("HEAD"),
    )
    .await?;
    let mut args = vec![
        "restore".to_string(),
        "--source".to_string(),
        source,
        "--worktree".to_string(),
    ];
    if body.stage {
        args.push("--staged".to_string());
    }
    args.push("--".to_string());
    args.push(file.repository_relative_file.clone());
    let output = git(&file.repository_root, args).await?;
    Ok(Json(GitOperationResponse::success(management_output_or(
        &output,
        &format!(
            "Restored {} from history{}",
            file.repository_relative_file,
            if body.stage {
                " and updated the index"
            } else {
                ""
            }
        ),
    ))))
}

// ---------------------------------------------------------------------------
// Submodules and worktrees

async fn management_submodule_paths(repository: &Path) -> Result<Vec<String>> {
    // Reuse the catalog's defensive parser. It rejects malformed declarations,
    // path traversal, and symlink escapes before a path can become an update
    // target, including for an uninitialized checkout.
    let root = normalize_path(repository);
    let mut paths = read_submodule_paths(&root)
        .await
        .into_iter()
        .filter_map(|path| {
            path.strip_prefix(&root)
                .ok()
                .map(|path| normalize_repo_relative_path(&path.to_string_lossy()))
        })
        .filter(|path| validate_file_path(path).is_ok())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

async fn management_submodule_target(
    repository: &Path,
    path: Option<&str>,
) -> Result<Option<String>> {
    let paths = management_submodule_paths(repository).await?;
    if paths.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "This repository has no configured submodules",
        ));
    }
    let Some(path) = path else {
        return Ok(None);
    };
    validate_file_path(path)?;
    let path = normalize_repo_relative_path(path);
    if !paths.iter().any(|candidate| candidate == &path) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "The selected path is not a configured submodule",
        ));
    }
    // Verify the path is still safely contained inside the selected repository
    // before passing it to Git, including when its checkout is uninitialized.
    safe_repo_child(repository, &path)?;
    Ok(Some(path))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementSubmoduleUpdateBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    path: Option<String>,
    #[serde(default = "management_default_true")]
    init: bool,
    #[serde(default)]
    recursive: bool,
    #[serde(default)]
    remote: bool,
    #[serde(default)]
    confirm: bool,
}

async fn management_update_submodule(
    State(state): State<AppState>,
    Json(body): Json<ManagementSubmoduleUpdateBody>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "update submodule checkouts")?;
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let path = management_submodule_target(&repository, body.path.as_deref()).await?;
    let mut args = vec!["submodule".to_string(), "update".to_string()];
    if body.init {
        args.push("--init".to_string());
    }
    if body.recursive {
        args.push("--recursive".to_string());
    }
    if body.remote {
        args.push("--remote".to_string());
    }
    if let Some(path) = path {
        args.push("--".to_string());
        args.push(path);
    }
    let output = git(&repository, args).await?;
    Ok(Json(GitOperationResponse::success(management_output_or(
        &output,
        "Submodule update completed successfully",
    ))))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementSubmoduleSyncBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    path: Option<String>,
    #[serde(default)]
    recursive: bool,
}

async fn management_sync_submodule(
    State(state): State<AppState>,
    Json(body): Json<ManagementSubmoduleSyncBody>,
) -> Result<Json<GitOperationResponse>> {
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let path = management_submodule_target(&repository, body.path.as_deref()).await?;
    let mut args = vec!["submodule".to_string(), "sync".to_string()];
    if body.recursive {
        args.push("--recursive".to_string());
    }
    if let Some(path) = path {
        args.push("--".to_string());
        args.push(path);
    }
    let output = git(&repository, args).await?;
    Ok(Json(GitOperationResponse::success(management_output_or(
        &output,
        "Submodule URLs synchronized successfully",
    ))))
}

#[derive(Debug, serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ManagementWorktreeSummary {
    path: String,
    head: Option<String>,
    branch: Option<String>,
    bare: bool,
    detached: bool,
    locked: Option<String>,
    prunable: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementWorktreesResponse {
    worktrees: Vec<ManagementWorktreeSummary>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementWorktreesQuery {
    #[serde(flatten)]
    scope: ManagementProjectScope,
}

fn management_parse_worktrees(value: &str) -> Vec<ManagementWorktreeSummary> {
    let mut worktrees = Vec::new();
    let mut current: Option<ManagementWorktreeSummary> = None;
    for line in value.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if let Some(worktree) = current.take() {
                worktrees.push(worktree);
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(worktree) = current.take() {
                worktrees.push(worktree);
            }
            current = Some(ManagementWorktreeSummary {
                path: path.to_string(),
                head: None,
                branch: None,
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
            });
            continue;
        }
        let Some(worktree) = current.as_mut() else {
            continue;
        };
        if let Some(head) = line.strip_prefix("HEAD ") {
            worktree.head = Some(head.to_string());
        } else if let Some(branch) = line.strip_prefix("branch ") {
            worktree.branch = Some(
                branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_string(),
            );
        } else if line == "bare" {
            worktree.bare = true;
        } else if line == "detached" {
            worktree.detached = true;
        } else if let Some(locked) = line.strip_prefix("locked") {
            worktree.locked = (!locked.trim().is_empty()).then(|| locked.trim().to_string());
        } else if let Some(prunable) = line.strip_prefix("prunable") {
            worktree.prunable = (!prunable.trim().is_empty()).then(|| prunable.trim().to_string());
        }
    }
    worktrees
}

async fn management_list_worktrees(repository: &Path) -> Result<Vec<ManagementWorktreeSummary>> {
    let output = git(repository, ["worktree", "list", "--porcelain"])
        .await?
        .stdout;
    Ok(management_parse_worktrees(&output))
}

async fn management_worktrees(
    State(state): State<AppState>,
    Query(query): Query<ManagementWorktreesQuery>,
) -> Result<Json<ManagementWorktreesResponse>> {
    let repository = management_repository(&state, &query.scope).await?;
    Ok(Json(ManagementWorktreesResponse {
        worktrees: management_list_worktrees(&repository).await?,
    }))
}

async fn management_validate_worktree_path(
    state: &AppState,
    repository: &Path,
    requested_path: &str,
    must_not_exist: bool,
) -> Result<PathBuf> {
    let requested_path = requested_path.trim();
    if requested_path.is_empty()
        || requested_path.contains('\0')
        || requested_path.chars().any(char::is_control)
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid worktree path",
        ));
    }
    let path = state
        .path_validator
        .validate_path(PathBuf::from(requested_path), false)
        .await
        .map_err(iowb_fs::FsError::from)?;
    let path = normalize_path(&path);
    let repository = normalize_path(repository);
    if path == repository || is_within(&path, &repository) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "A linked worktree must be outside the selected repository directory",
        ));
    }
    if must_not_exist && path.exists() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "The new worktree path already exists",
        ));
    }
    Ok(path)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementWorktreeAddBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    path: String,
    branch: Option<String>,
    start_point: Option<String>,
    #[serde(default)]
    create_branch: bool,
    #[serde(default)]
    detach: bool,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    confirm: bool,
}

async fn management_add_worktree(
    State(state): State<AppState>,
    Json(body): Json<ManagementWorktreeAddBody>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "create a linked worktree")?;
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let path = management_validate_worktree_path(&state, &repository, &body.path, true).await?;
    if body.detach && body.branch.is_some() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "A detached worktree cannot also select a branch",
        ));
    }
    if body.detach && body.create_branch {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "A detached worktree cannot create a branch",
        ));
    }
    if !body.detach && body.branch.is_none() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Select an existing branch, create a new branch, or choose detached HEAD",
        ));
    }
    if !body.create_branch && body.start_point.is_some() && !body.detach {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "A start point is only valid when creating a branch or a detached worktree",
        ));
    }
    let start_point = match body.start_point.as_deref() {
        Some(value) => Some(resolve_commit_reference(&repository, value).await?),
        None => None,
    };
    let branch = body
        .branch
        .as_deref()
        .map(validate_branch_name)
        .transpose()?;
    if let Some(branch) = branch.as_deref()
        && !body.create_branch
    {
        management_local_branch_exists(&repository, branch).await?;
    }
    let mut args = vec!["worktree".to_string(), "add".to_string()];
    if body.force {
        args.push("--force".to_string());
    }
    if body.detach {
        args.push("--detach".to_string());
    }
    if body.create_branch {
        let branch = branch.as_deref().ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "A branch name is required to create a branch",
            )
        })?;
        args.push("-b".to_string());
        args.push(branch.to_string());
    }
    args.push("--".to_string());
    args.push(path.display().to_string());
    if let Some(start_point) = start_point {
        args.push(start_point);
    } else if let Some(branch) = branch {
        args.push(branch);
    }
    let output = git(&repository, args).await?;
    Ok(Json(GitOperationResponse::success(management_output_or(
        &output,
        "Linked worktree created successfully",
    ))))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementWorktreeRemoveBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    path: String,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    confirm: bool,
}

async fn management_remove_worktree(
    State(state): State<AppState>,
    Json(body): Json<ManagementWorktreeRemoveBody>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "remove a linked worktree")?;
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let path = management_validate_worktree_path(&state, &repository, &body.path, false).await?;
    let listed = management_list_worktrees(&repository).await?;
    let listed_path = listed
        .iter()
        .any(|worktree| normalize_path(Path::new(&worktree.path)) == path);
    if !listed_path {
        return Err(ServerError::new(
            StatusCode::NOT_FOUND,
            "The selected path is not a registered linked worktree",
        ));
    }
    if path == normalize_path(&repository) || path.join(".git").is_dir() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "The primary repository worktree cannot be removed",
        ));
    }
    let mut args = vec!["worktree".to_string(), "remove".to_string()];
    if body.force {
        args.push("--force".to_string());
    }
    args.push("--".to_string());
    args.push(path.display().to_string());
    let output = git(&repository, args).await?;
    Ok(Json(GitOperationResponse::success(management_output_or(
        &output,
        "Linked worktree removed successfully",
    ))))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementWorktreePruneBody {
    #[serde(flatten)]
    scope: ManagementProjectScope,
    #[serde(default)]
    confirm: bool,
}

async fn management_prune_worktrees(
    State(state): State<AppState>,
    Json(body): Json<ManagementWorktreePruneBody>,
) -> Result<Json<GitOperationResponse>> {
    require_management_confirmation(body.confirm, "prune stale worktree records")?;
    let repository = management_repository(&state, &body.scope).await?;
    ensure_no_git_workflow_in_progress(&repository).await?;
    let output = git(&repository, ["worktree", "prune", "--verbose"]).await?;
    Ok(Json(GitOperationResponse::success(management_output_or(
        &output,
        "Stale worktree records pruned successfully",
    ))))
}

#[cfg(test)]
mod management_tests {
    use super::*;

    #[test]
    fn parses_worktree_porcelain_records() {
        let worktrees = management_parse_worktrees(
            "worktree /workspace/main\nHEAD abc123\nbranch refs/heads/main\n\nworktree /workspace/feature\nHEAD def456\ndetached\nlocked review in progress\nprunable stale path\n\n",
        );
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert!(worktrees[1].detached);
        assert_eq!(worktrees[1].locked.as_deref(), Some("review in progress"));
        assert_eq!(worktrees[1].prunable.as_deref(), Some("stale path"));
    }

    #[test]
    fn parses_line_porcelain_blame_without_leaking_metadata_between_hashes() {
        let lines = management_parse_blame(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 1 1 1\nauthor Ada\nauthor-mail <ada@example.test>\nauthor-time 100\nsummary initial\nfilename file.txt\n\tfirst\nbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb 2 2 1\nauthor Bea\nauthor-mail <bea@example.test>\nauthor-time 200\nsummary second\nfilename file.txt\n\tsecond\n",
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].author, "Ada");
        assert_eq!(lines[1].author_email, "bea@example.test");
        assert_eq!(lines[1].content, "second");
    }

    #[test]
    fn requires_confirmation_for_destructive_management_actions() {
        assert!(require_management_confirmation(false, "remove a remote").is_err());
        assert!(require_management_confirmation(true, "remove a remote").is_ok());
    }

    #[test]
    fn enforces_a_bounded_blame_range() {
        assert_eq!(management_blame_range(Some(4), Some(8)).unwrap(), (4, 8));
        assert!(management_blame_range(Some(0), Some(1)).is_err());
        assert!(management_blame_range(Some(1), Some(MANAGEMENT_MAX_BLAME_LINES + 1)).is_err());
    }

    #[test]
    fn redacts_credentials_from_remote_urls() {
        assert_eq!(
            management_display_remote_url("https://token@example.test/owner/repo.git"),
            "https://<redacted>@example.test/owner/repo.git"
        );
        assert_eq!(
            management_display_remote_url("git@example.test:owner/repo.git"),
            "git@example.test:owner/repo.git"
        );
        assert_eq!(
            management_display_remote_url(
                "https://token@example.test/owner/repo.git?access_token=secret#fragment-secret"
            ),
            "https://<redacted>@example.test/owner/repo.git"
        );
        assert_eq!(
            management_display_remote_url("https://example.test/owner/repo.git?token=secret"),
            "https://example.test/owner/repo.git"
        );
        assert_eq!(
            management_display_remote_url("git@example.test:owner/repo.git?token=secret"),
            "git@example.test:owner/repo.git"
        );
    }
}

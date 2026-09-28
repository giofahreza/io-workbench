// Recoverable Git history workflows. Each command is exposed as a narrow
// operation so the mobile UI can explain the target and recover from
// conflicts without dropping the user into a terminal.

#[derive(Debug, Deserialize)]
struct CherryPickBody {
    #[serde(flatten)]
    project: ProjectBody,
    commits: Vec<String>,
    #[serde(rename = "noCommit", default)]
    no_commit: bool,
    #[serde(default)]
    mainline: Option<usize>,
    /// The immutable target snapshot shown by the source-branch picker.
    /// Requiring it avoids applying a reviewed range after HEAD has moved.
    #[serde(rename = "expectedHead")]
    expected_head: String,
    /// The checked-out branch identity captured with expectedHead. A commit
    /// OID alone cannot distinguish two local branches at the same commit.
    #[serde(rename = "expectedTargetRef")]
    expected_target_ref: String,
    /// A fully qualified local or remote-tracking branch ref when this is a
    /// contextual cherry-pick.  Raw single-hash fallback requests omit it.
    #[serde(rename = "sourceRef")]
    source_ref: Option<String>,
    #[serde(rename = "expectedSourceHead")]
    expected_source_head: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CherryPickPreviewQuery {
    #[serde(flatten)]
    project: ProjectQuery,
    source_ref: String,
}

const CHERRY_PICK_MAX_COMMITS: usize = 100;
/// Keep a range small enough that its recovery state remains understandable
/// on a touch device, while matching the cherry-pick range limit.
const REVERT_MAX_COMMITS: usize = 100;
const SQUASH_MERGE_RECOVERY_VERSION: u8 = 1;
const SQUASH_MERGE_RECOVERY_MARKER: &str = "iowb-squash-merge-recovery.json";
const SQUASH_MERGE_RECOVERY_MARKER_MAX_BYTES: usize = 32 * 1024;

/// The four merge variants exposed by LazyGit's branch merge menu. Keep the
/// wire values camel-cased for new clients while accepting the labels used in
/// its terminal UI and older hand-written API calls.
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
enum MergeMode {
    #[serde(rename = "default", alias = "Default")]
    Default,
    #[serde(
        rename = "fastForward",
        alias = "FastForward",
        alias = "fast-forward"
    )]
    FastForward,
    #[serde(
        rename = "noFastForward",
        alias = "NoFastForward",
        alias = "no-fast-forward"
    )]
    NoFastForward,
    #[serde(rename = "squash", alias = "Squash")]
    Squash,
}

#[derive(Debug, Deserialize)]
struct MergeBody {
    #[serde(flatten)]
    project: ProjectBody,
    source: String,
    /// New LazyGit-compatible explicit mode. Omitted requests retain the
    /// legacy `noFastForward` behavior below.
    #[serde(rename = "mergeMode", default)]
    merge_mode: Option<MergeMode>,
    #[serde(rename = "noFastForward", default)]
    no_fast_forward: bool,
    /// Opt-in counterpart to LazyGit's separate "Squash merge and commit"
    /// action. Plain `mergeMode: squash` intentionally leaves the result
    /// staged and uncommitted.
    #[serde(rename = "commitSquash", default)]
    commit_squash: bool,
    message: Option<String>,
    /// Immutable target snapshot captured before the confirmation UI opened.
    #[serde(rename = "expectedHead")]
    expected_head: String,
    /// Checked-out ref identity paired with expected_head. This prevents a
    /// same-OID checkout from applying a reviewed merge to a different branch.
    #[serde(rename = "expectedTargetRef")]
    expected_target_ref: String,
}

/// Server-owned state for a squash merge that stopped before its requested
/// outcome was complete. Git deliberately does not write `MERGE_HEAD` for a
/// squash merge, so the normal `merge --continue` / `merge --abort` protocol
/// cannot describe or safely recover it.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SquashMergeRecoveryMarker {
    version: u8,
    /// Immutable HEAD from immediately before `git merge --squash` ran.
    pre_merge_head: String,
    /// Checkout identity paired with pre_merge_head. It prevents a finish or
    /// abort after an external same-OID branch switch.
    target_ref: String,
    /// Resolved source object shown to the user before the merge began.
    source: String,
    /// LazyGit exposes an uncommitted squash and an explicit squash+commit
    /// action. Keep their recovery outcomes distinct too.
    commit_squash: bool,
    /// Only meaningful for the explicit squash+commit action.
    message: Option<String>,
}

/// Reconcile the new explicit LazyGit-style menu choice with clients which
/// shipped the earlier `noFastForward` boolean. A request must describe one
/// merge policy; accepting contradictory fields would make the confirmation
/// screen lie about the Git command that will run.
fn resolve_merge_mode(body: &MergeBody) -> Result<MergeMode> {
    let mode = body.merge_mode.unwrap_or(if body.no_fast_forward {
        MergeMode::NoFastForward
    } else {
        MergeMode::Default
    });
    if body.no_fast_forward
        && body
            .merge_mode
            .is_some_and(|requested| requested != MergeMode::NoFastForward)
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "noFastForward cannot be combined with a different mergeMode",
        ));
    }
    if body.commit_squash && mode != MergeMode::Squash {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "commitSquash is only valid with mergeMode squash",
        ));
    }
    Ok(mode)
}

#[derive(Debug, Deserialize)]
struct RebaseBody {
    #[serde(flatten)]
    project: ProjectBody,
    upstream: String,
    #[serde(default)]
    interactive: bool,
    #[serde(rename = "expectedHead")]
    expected_head: String,
    #[serde(rename = "expectedTargetRef")]
    expected_target_ref: String,
}

#[derive(Debug, Deserialize)]
struct RevertBody {
    #[serde(flatten)]
    project: ProjectBody,
    /// Legacy single-commit shape. Keep accepting it for desktop/web clients
    /// that shipped before Commit-tab range selection existed.
    #[serde(default)]
    commit: Option<String>,
    /// Ordered range from the native Commit tab. Git applies `git revert`
    /// arguments in this exact order, so never sort this list on the server.
    #[serde(default)]
    commits: Vec<String>,
    #[serde(default)]
    mainline: Option<usize>,
    #[serde(rename = "expectedHead")]
    expected_head: String,
    #[serde(rename = "expectedTargetRef")]
    expected_target_ref: String,
}

#[derive(Debug, Deserialize)]
struct ResetBody {
    #[serde(flatten)]
    project: ProjectBody,
    target: String,
    mode: String,
    #[serde(rename = "confirmHard", default)]
    confirm_hard: bool,
    #[serde(rename = "expectedHead")]
    expected_head: String,
    #[serde(rename = "expectedTargetRef")]
    expected_target_ref: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitWorkflowKind {
    Merge,
    SquashMerge,
    CommittedSquashMerge,
    Rebase,
    /// An interactive-rebase `edit` command has replayed its commit and
    /// deliberately stopped so the user can change the index and amend it.
    /// Keep this distinct from a conflicted rebase: both use `rebase
    /// --continue`/`--abort`, but only this state may safely expose ordinary
    /// Changes staging and amend controls.
    RebaseEdit,
    CherryPick,
    Revert,
    Stash,
    StashUnstaged,
    Bisect,
}

impl GitWorkflowKind {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::SquashMerge | Self::CommittedSquashMerge => "squash-merge",
            Self::Rebase | Self::RebaseEdit => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
            Self::Stash => "stash",
            Self::StashUnstaged => "stash-unstaged",
            Self::Bisect => "bisect",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Merge => "Merge",
            Self::SquashMerge | Self::CommittedSquashMerge => "Squash merge",
            Self::Rebase | Self::RebaseEdit => "Rebase",
            Self::CherryPick => "Cherry-pick",
            Self::Revert => "Revert",
            Self::Stash => "Stash recovery",
            Self::StashUnstaged => "Stash unstaged changes",
            Self::Bisect => "Git bisect",
        }
    }

    fn can_skip(self) -> bool {
        matches!(
            self,
            Self::Rebase | Self::CherryPick | Self::Revert
        )
    }

    fn is_edit_stop(self) -> bool {
        matches!(self, Self::RebaseEdit)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmptyStepRecovery {
    None,
    /// Git leaves `CHERRY_PICK_HEAD` behind for an already-applied patch, but
    /// there is no index or worktree content to commit. In that state
    /// `--continue` only produces another error; the useful recovery is Skip
    /// (or Abort).
    Skip,
    /// A multi-commit revert can pause in `.git/sequencer` on an empty
    /// inverse patch without writing `REVERT_HEAD`. Git's own recovery
    /// instruction is `revert --continue`, which advances past that empty
    /// entry and carries on with the reviewed range.
    Continue,
}

#[derive(Debug, Clone)]
struct DetectedGitWorkflow {
    kind: GitWorkflowKind,
    current_commit: Option<String>,
    current_step: Option<usize>,
    total_steps: Option<usize>,
    empty_step_recovery: EmptyStepRecovery,
}

impl DetectedGitWorkflow {
    fn response(&self) -> GitOperationStateResponse {
        let operation = self.kind.wire_name();
        let message = if self.kind.is_edit_stop() {
            "Rebase paused at an edit step. Review changes in Changes, stage any files to include, amend the stopped commit if needed, then continue or abort."
                .to_string()
        } else {
            match self.empty_step_recovery {
            EmptyStepRecovery::Skip =>
                "This cherry-pick has no changes to apply, usually because the patch is already present. Skip it to continue without creating a duplicate commit, or abort to leave history unchanged."
                    .to_string(),
            EmptyStepRecovery::Continue =>
                "This revert step has no changes to apply. Continue moves past the empty inverse patch and applies the remaining reviewed reverts; Skip current has the same effect for this empty step, or abort to leave history unchanged."
                    .to_string(),
            EmptyStepRecovery::None => match self.kind {
            GitWorkflowKind::Stash =>
                "Stash application has conflicts. Resolve and stage every conflicted path, then finish stash recovery or abandon it."
                    .to_string(),
            GitWorkflowKind::StashUnstaged =>
                "Stashing only unstaged changes paused safely. Finish to save the unstaged delta while retaining the index, or restore the original mixed changes."
                    .to_string(),
            GitWorkflowKind::SquashMerge =>
                "Squash merge has conflicts. Resolve and stage every conflicted path, then finish recovery to leave the squash changes staged for review and commit, or abort to restore the pre-merge worktree."
                    .to_string(),
            GitWorkflowKind::CommittedSquashMerge =>
                "Squash merge has conflicts. Resolve and stage every conflicted path, then finish recovery to create the requested squash commit, or abort to restore the pre-merge worktree."
                    .to_string(),
            GitWorkflowKind::Bisect =>
                "Git bisect is in progress. Open Git bisect to mark the checked-out revision good or bad, or reset the bisect."
                    .to_string(),
            _ => format!(
                "{} is in progress. Resolve conflicts, stage the resolved paths, then continue or abort.",
                self.kind.display_name()
            ),
            },
            }
        };
        GitOperationStateResponse {
            operation: operation.to_string(),
            in_progress: true,
            can_continue: !matches!(self.kind, GitWorkflowKind::Bisect)
                && !matches!(self.empty_step_recovery, EmptyStepRecovery::Skip),
            can_abort: !matches!(self.kind, GitWorkflowKind::Bisect),
            can_skip: self.kind.can_skip(),
            current_commit: self.current_commit.clone(),
            current_step: self.current_step,
            total_steps: self.total_steps,
            is_edit_stop: self.kind.is_edit_stop(),
            message: Some(message),
        }
    }
}

/// Only canonical branch refs are valid sources for the touch-oriented
/// cherry-pick picker.  A display name such as `feature` is ambiguous when a
/// repository has both a local and a remote-tracking branch with that name;
/// callers therefore send `refs/heads/feature` or
/// `refs/remotes/origin/feature`.
fn validate_cherry_pick_source_ref(value: &str, remotes: &[String]) -> Result<String> {
    let value = value.trim();
    let valid = value
        .strip_prefix("refs/heads/")
        .map(validate_branch_name)
        .transpose()?
        .map(|branch| format!("refs/heads/{branch}"))
        .or_else(|| {
            value.strip_prefix("refs/remotes/").and_then(|remote_branch| {
                // Remote names themselves may contain slashes (for example
                // `team/review`). Reuse the same longest-configured-name
                // parsing used for upstream tracking rather than treating the
                // first slash as a hard separator.
                let (remote, branch) =
                    split_configured_remote_tracking_name(remote_branch, remotes)?;
                let branch = validate_branch_name(&branch).ok()?;
                Some(format!("refs/remotes/{remote}/{branch}"))
            })
        });
    valid.ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            "sourceRef must be a fully qualified local or remote-tracking branch ref",
        )
    })
}

async fn resolve_cherry_pick_source_ref(
    repository: &Path,
    source_ref: &str,
) -> Result<(String, String)> {
    let remotes = configured_remote_names(repository).await?;
    let source_ref = validate_cherry_pick_source_ref(source_ref, &remotes)?;
    let source_head = resolve_commit_reference(repository, &source_ref).await?;
    Ok((source_ref, source_head))
}

/// The target must be pinned by both object ID and checkout identity. Two
/// branch names can temporarily point at the same commit, in which case an
/// expected HEAD check alone would let a confirmation prepared on `main`
/// accidentally mutate `release` after a checkout.
async fn checked_out_target_ref(repository: &Path) -> Result<String> {
    match git(repository, ["symbolic-ref", "--quiet", "HEAD"]).await {
        Ok(output) => {
            let value = output.stdout.trim();
            let branch = value.strip_prefix("refs/heads/").ok_or_else(|| {
                ServerError::new(
                    StatusCode::CONFLICT,
                    "The checked-out Git target is not a local branch. Refresh Git status and try again.",
                )
            })?;
            Ok(format!("refs/heads/{}", validate_branch_name(branch)?))
        }
        // `symbolic-ref --quiet HEAD` exits with status 1 for detached HEAD.
        // The full immutable expectedHead still pins that detached checkout.
        Err(_) => Ok("HEAD".to_string()),
    }
}

fn validate_expected_target_ref(value: &str) -> Result<String> {
    let value = value.trim();
    if value == "HEAD" {
        return Ok(value.to_string());
    }
    let branch = value.strip_prefix("refs/heads/").ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            "expectedTargetRef must be the fully qualified checked-out local branch ref or HEAD",
        )
    })?;
    Ok(format!("refs/heads/{}", validate_branch_name(branch)?))
}

/// Verify both dimensions of the target a user reviewed. A full commit ID
/// catches ordinary stale forms, while the canonical checked-out ref catches
/// a branch checkout that happens to retain the same commit ID. Every
/// history-mutating workflow uses this before resolving or running its
/// source command so a stale form cannot act on a newly checked-out branch.
async fn validate_workflow_target_snapshot(
    repository: &Path,
    expected_head: &str,
    expected_target_ref: &str,
    workflow: &str,
) -> Result<()> {
    let current_head = resolve_commit_reference(repository, "HEAD").await?;
    let expected_head = resolve_commit_reference(
        repository,
        validate_immutable_expected_head(expected_head)?,
    )
    .await?;
    if current_head != expected_head {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "The current HEAD changed since this {workflow} was prepared. Refresh Git status and confirm again."
            ),
        ));
    }

    let expected_target_ref = validate_expected_target_ref(expected_target_ref)?;
    let current_target_ref = checked_out_target_ref(repository).await?;
    if current_target_ref != expected_target_ref {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "The checked-out branch changed since this {workflow} was prepared. Refresh Git status and confirm again."
            ),
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct CherryPickSourceCommit {
    summary: GitCommitSummary,
    parent_count: usize,
}

fn parse_cherry_pick_source_commit(line: &str) -> Option<CherryPickSourceCommit> {
    let mut parts = line.splitn(6, '\u{1f}');
    let hash = parts.next()?.to_string();
    let parent_count = parts.next()?.split_whitespace().count();
    Some(CherryPickSourceCommit {
        summary: GitCommitSummary {
            hash,
            author: parts.next()?.to_string(),
            email: parts.next()?.to_string(),
            date: parts.next()?.to_string(),
            message: parts.next()?.to_string(),
            stats: String::new(),
        },
        parent_count,
    })
}

/// Return the commits unique to `source_head`, including merge commits, in
/// the order Git should apply them. The picker and mutation endpoint
/// intentionally share this derivation: a client cannot reorder a dependent
/// stack or smuggle in a commit that was not shown by its selected source
/// branch.
async fn cherry_pick_source_commits(
    repository: &Path,
    source_head: &str,
    target_head: &str,
) -> Result<Vec<CherryPickSourceCommit>> {
    let limit = (CHERRY_PICK_MAX_COMMITS + 1).to_string();
    let output = git(
        repository,
        [
            "log",
            "--reverse",
            "--topo-order",
            "--pretty=format:%H\u{1f}%P\u{1f}%an\u{1f}%ae\u{1f}%ad\u{1f}%s",
            "--date=iso-strict",
            "-n",
            limit.as_str(),
            source_head,
            "--not",
            target_head,
        ],
    )
    .await?
    .stdout;
    let mut commits = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(parse_cherry_pick_source_commit)
        .collect::<Vec<_>>();
    if commits.len() > CHERRY_PICK_MAX_COMMITS {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "The selected branch has more than {CHERRY_PICK_MAX_COMMITS} unique commits. Split the cherry-pick into smaller branch ranges first."
            ),
        ));
    }
    for commit in &mut commits {
        commit.summary.stats = git(repository, ["show", "--stat", "--format=", &commit.summary.hash])
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
    }
    Ok(commits)
}

async fn cherry_pick_preview(
    State(state): State<AppState>,
    Query(query): Query<CherryPickPreviewQuery>,
) -> Result<Json<GitCherryPickPreviewResponse>> {
    let project_path = resolve_git_repository_path(
        &state,
        query.project.project_ref()?,
        query.project.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    let target_head = resolve_commit_reference(&project_path, "HEAD").await?;
    let target_ref = checked_out_target_ref(&project_path).await?;
    let (source_ref, source_head) =
        resolve_cherry_pick_source_ref(&project_path, &query.source_ref).await?;
    let source_commits =
        cherry_pick_source_commits(&project_path, &source_head, &target_head).await?;
    let merge_commit_hashes = source_commits
        .iter()
        .filter(|commit| commit.parent_count > 1)
        .map(|commit| commit.summary.hash.clone())
        .collect::<Vec<_>>();
    let default_mainline = (!merge_commit_hashes.is_empty()).then_some(1);
    let commits = source_commits
        .into_iter()
        .map(|commit| commit.summary)
        .collect::<Vec<_>>();
    Ok(Json(GitCherryPickPreviewResponse {
        source_ref,
        source_head,
        target_ref,
        target_head,
        commits,
        merge_commit_hashes,
        default_mainline,
    }))
}

async fn operation_state(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitOperationStateResponse>> {
    let project_path = resolve_git_repository_path(
        &state,
        query.project_ref()?,
        query.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    Ok(Json(git_operation_state(&project_path).await?))
}

async fn cherry_pick(
    State(state): State<AppState>,
    Json(body): Json<CherryPickBody>,
) -> Result<Json<GitOperationResponse>> {
    if body.no_commit {
        // Git does not leave CHERRY_PICK_HEAD for `--no-commit` conflicts, so
        // its normal continue/abort protocol cannot safely recover a mobile
        // client. Do not let an API caller strand an unmerged index while the
        // dedicated marker-backed no-commit workflow is still absent.
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Cherry-pick without creating a commit is unavailable because conflict recovery would be unsafe. Cherry-pick the commit normally instead.",
        ));
    }
    if body.commits.is_empty() || body.commits.len() > CHERRY_PICK_MAX_COMMITS {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Choose between one and 100 commits to cherry-pick",
        ));
    }
    let project_path = resolve_git_repository_path(
        &state,
        body.project.project_ref()?,
        body.project.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    validate_workflow_target_snapshot(
        &project_path,
        &body.expected_head,
        &body.expected_target_ref,
        "cherry-pick",
    )
    .await?;
    let current_head = resolve_commit_reference(&project_path, "HEAD").await?;

    let mut requested = Vec::with_capacity(body.commits.len());
    let mut seen = HashSet::new();
    for commit in &body.commits {
        let hash = resolve_commit_reference(&project_path, commit).await?;
        if !seen.insert(hash.clone()) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "A commit can only be cherry-picked once in the same request",
            ));
        }
        requested.push(hash);
    }

    let hashes = match (body.source_ref.as_deref(), body.expected_source_head.as_deref()) {
        (Some(source_ref), Some(expected_source_head)) => {
            let (source_ref, source_head) =
                resolve_cherry_pick_source_ref(&project_path, source_ref).await?;
            let expected_source_head = resolve_commit_reference(
                &project_path,
                validate_immutable_expected_head(expected_source_head)?,
            )
            .await?;
            if source_head != expected_source_head {
                return Err(ServerError::new(
                    StatusCode::CONFLICT,
                    format!(
                        "{source_ref} changed since this cherry-pick was prepared. Refresh its commits and confirm again."
                    ),
                ));
            }
            let available =
                cherry_pick_source_commits(&project_path, &source_head, &current_head).await?;
            let available_hashes = available
                .iter()
                .map(|commit| commit.summary.hash.as_str())
                .collect::<HashSet<_>>();
            if requested
                .iter()
                .any(|commit| !available_hashes.contains(commit.as_str()))
            {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Every selected commit must still be a unique commit from the reviewed source branch.",
                ));
            }
            // Git must receive the source topology order, not the order in
            // which rows happened to be tapped on a small touch screen.
            available
                .into_iter()
                .filter(|commit| seen.contains(&commit.summary.hash))
                .map(|commit| commit.summary.hash)
                .collect::<Vec<_>>()
        }
        (Some(_), None) => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "expectedSourceHead is required for a source-branch cherry-pick. Refresh the source branch and try again.",
            ));
        }
        (None, Some(_)) => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "expectedSourceHead is only valid with sourceRef",
            ));
        }
        (None, None) => {
            if requested.len() != 1 {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Choose commits from a source branch for an ordered stack. The hash fallback accepts exactly one commit.",
                ));
            }
            requested
        }
    };
    // LazyGit sends `-m 1` when a selected set includes a merge commit. Git
    // accepts one mainline for the whole invocation, so validate a custom
    // value against every selected merge and otherwise make first-parent the
    // explicit, deterministic default.
    let mainline = resolve_merge_mainline(&project_path, &hashes, body.mainline).await?;
    advanced_require_clean_worktree(&project_path, "cherry-pick commits").await?;
    let mut write_paths = Vec::new();
    for hash in &hashes {
        write_paths.extend(
            cherry_pick_commit_write_paths(&project_path, hash, mainline).await?,
        );
    }
    advanced_require_no_ignored_path_collision(
        &project_path,
        &write_paths,
        "cherry-pick commits",
    )
    .await?;

    let mut args = vec!["cherry-pick".to_string(), "--no-edit".to_string()];
    args.extend(cherry_pick_empty_preservation_args(&project_path).await);
    if let Some(mainline) = mainline {
        args.push("--mainline".to_string());
        args.push(mainline.to_string());
    }
    args.extend(hashes);

    let response = workflow_response_after_command(
        &project_path,
        git_with_noninteractive_editor(&project_path, args).await,
        "Cherry-pick completed successfully",
    )
    .await?;
    Ok(Json(response))
}

async fn merge(
    State(state): State<AppState>,
    Json(body): Json<MergeBody>,
) -> Result<Json<GitOperationResponse>> {
    let merge_mode = resolve_merge_mode(&body)?;
    let message = validate_optional_git_message(body.message.as_deref())?;
    if merge_mode == MergeMode::Squash && !body.commit_squash && message.is_some() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "A message is only used when committing a squash merge. Use commitSquash or commit the staged squash changes explicitly after review.",
        ));
    }
    let project_path = resolve_git_repository_path(
        &state,
        body.project.project_ref()?,
        body.project.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    validate_workflow_target_snapshot(
        &project_path,
        &body.expected_head,
        &body.expected_target_ref,
        "merge",
    )
    .await?;
    let source = resolve_commit_reference(&project_path, &body.source).await?;

    // LazyGit presents its Fast-forward menu choice only when the current
    // HEAD is an ancestor of the selected ref. `git merge --ff` alone would
    // silently create an ordinary merge for divergent histories, making the
    // touch UI's selected operation dishonest. Preserve the actual `--ff`
    // invocation for the compatible case while rejecting an impossible fast
    // forward before any worktree mutation.
    if merge_mode == MergeMode::FastForward
        && !advanced_is_ancestor(&project_path, "HEAD", &source).await
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Fast-forward merge is only available when the current branch is an ancestor of the selected source. Use Default or No fast-forward to merge divergent branches.",
        ));
    }

    // LazyGit's ordinary merge variants retain Git's own dirty-worktree
    // behavior (with autostash deliberately disabled below). A squash merge
    // is different: without MERGE_HEAD we need a pristine starting point so
    // our marker-backed abort can restore exactly the reviewed pre-merge
    // state rather than risking unrelated local edits.
    let squash_marker = if merge_mode == MergeMode::Squash {
        advanced_require_clean_worktree(&project_path, "squash merge").await?;
        let marker = SquashMergeRecoveryMarker {
            version: SQUASH_MERGE_RECOVERY_VERSION,
            pre_merge_head: resolve_commit_reference(&project_path, "HEAD").await?,
            target_ref: checked_out_target_ref(&project_path).await?,
            source: source.clone(),
            commit_squash: body.commit_squash,
            message: message.clone(),
        };
        write_squash_merge_recovery_marker(&project_path, &marker).await?;
        Some(marker)
    } else {
        None
    };

    // Never let a repository-local merge.autoStash mutate state behind the
    // confirmation UI. Git can still merge a non-overlapping dirty worktree,
    // but it must report conflicts instead of manufacturing a hidden stash.
    let mut args = vec![
        "merge".to_string(),
        "--no-edit".to_string(),
        "--no-autostash".to_string(),
        // Git merge normally replaces ignored files when the merged tree
        // starts tracking their paths. Keep ignored local output safe just as
        // the checkout and cherry-pick workflows do.
        "--no-overwrite-ignore".to_string(),
    ];
    match merge_mode {
        MergeMode::Default => {}
        MergeMode::FastForward => args.push("--ff".to_string()),
        MergeMode::NoFastForward => args.push("--no-ff".to_string()),
        // This is the exact flag pair LazyGit uses for its squash choice.
        MergeMode::Squash => {
            args.push("--squash".to_string());
            args.push("--ff".to_string());
        }
    }
    // A squash's message belongs to its later explicit `git commit`, not to
    // `git merge --squash`; otherwise a conflict recovery could silently
    // lose or misrepresent the requested commit text.
    if let Some(message) = message.filter(|_| merge_mode != MergeMode::Squash) {
        args.push("-m".to_string());
        args.push(message);
    }
    args.push(source);

    let command = git_with_noninteractive_editor(&project_path, args).await;
    let response = if let Some(marker) = squash_marker {
        squash_merge_response_after_command(&project_path, command, &marker).await?
    } else {
        workflow_response_after_command(&project_path, command, "Merge completed successfully").await?
    };
    Ok(Json(response))
}

fn invalid_squash_merge_recovery_marker() -> ServerError {
    ServerError::new(
        StatusCode::CONFLICT,
        "Squash merge recovery data is invalid. Resolve or clean the worktree from a terminal before starting another Git operation.",
    )
}

fn is_full_git_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.chars().all(|character| character.is_ascii_hexdigit())
}

fn validate_squash_merge_recovery_marker(marker: &SquashMergeRecoveryMarker) -> Result<()> {
    if marker.version != SQUASH_MERGE_RECOVERY_VERSION
        || !is_full_git_object_id(&marker.pre_merge_head)
        || !is_full_git_object_id(&marker.source)
    {
        return Err(invalid_squash_merge_recovery_marker());
    }
    let target_ref = validate_expected_target_ref(&marker.target_ref)
        .map_err(|_| invalid_squash_merge_recovery_marker())?;
    if target_ref != marker.target_ref {
        return Err(invalid_squash_merge_recovery_marker());
    }
    let message = validate_optional_git_message(marker.message.as_deref())
        .map_err(|_| invalid_squash_merge_recovery_marker())?;
    if message != marker.message || (!marker.commit_squash && marker.message.is_some()) {
        return Err(invalid_squash_merge_recovery_marker());
    }
    Ok(())
}

async fn squash_merge_recovery_marker_path(repository: &Path) -> Result<PathBuf> {
    Ok(git_metadata_directory(repository)
        .await?
        .join(SQUASH_MERGE_RECOVERY_MARKER))
}

async fn read_squash_merge_recovery_marker(
    repository: &Path,
) -> Result<Option<SquashMergeRecoveryMarker>> {
    let path = squash_merge_recovery_marker_path(repository).await?;
    let metadata = match fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_server_error(error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > SQUASH_MERGE_RECOVERY_MARKER_MAX_BYTES as u64
    {
        return Err(invalid_squash_merge_recovery_marker());
    }
    let bytes = fs::read(path).await.map_err(io_server_error)?;
    let marker = serde_json::from_slice::<SquashMergeRecoveryMarker>(&bytes)
        .map_err(|_| invalid_squash_merge_recovery_marker())?;
    validate_squash_merge_recovery_marker(&marker)?;
    Ok(Some(marker))
}

async fn write_squash_merge_recovery_marker(
    repository: &Path,
    marker: &SquashMergeRecoveryMarker,
) -> Result<()> {
    validate_squash_merge_recovery_marker(marker)?;
    let encoded = serde_json::to_vec(marker).map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not persist squash merge recovery data",
            error.to_string(),
        )
    })?;
    if encoded.len() > SQUASH_MERGE_RECOVERY_MARKER_MAX_BYTES {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "This squash merge contains too much recovery metadata to continue safely",
        ));
    }
    let path = squash_merge_recovery_marker_path(repository).await?;
    if let Ok(metadata) = fs::symlink_metadata(&path).await
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(invalid_squash_merge_recovery_marker());
    }
    fs::write(path, encoded).await.map_err(io_server_error)
}

async fn clear_squash_merge_recovery_marker(repository: &Path) -> Result<()> {
    let path = squash_merge_recovery_marker_path(repository).await?;
    let metadata = match fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_server_error(error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid_squash_merge_recovery_marker());
    }
    fs::remove_file(path).await.map_err(io_server_error)
}

async fn squash_merge_conflicted_paths(repository: &Path) -> Result<Vec<String>> {
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

async fn squash_merge_has_staged_changes(repository: &Path) -> Result<bool> {
    Ok(!git(repository, ["diff", "--cached", "--name-only", "-z"])
        .await?
        .stdout_bytes
        .is_empty())
}

fn squash_merge_recovery_workflow_for_marker(
    marker: &SquashMergeRecoveryMarker,
) -> DetectedGitWorkflow {
    DetectedGitWorkflow {
        kind: if marker.commit_squash {
            GitWorkflowKind::CommittedSquashMerge
        } else {
            GitWorkflowKind::SquashMerge
        },
        current_commit: Some(marker.source.clone()),
        current_step: None,
        total_steps: None,
        empty_step_recovery: EmptyStepRecovery::None,
    }
}

/// Git does not publish a normal operation marker for `merge --squash`. A
/// marker created before the command lets a conflict remain visible and keeps
/// ordinary mutations locked until the user explicitly finishes or abandons
/// the reviewed operation.
async fn squash_merge_recovery_workflow(
    repository: &Path,
) -> Result<Option<DetectedGitWorkflow>> {
    let Some(marker) = read_squash_merge_recovery_marker(repository).await? else {
        return Ok(None);
    };
    if !squash_merge_conflicted_paths(repository).await?.is_empty()
        || squash_merge_has_staged_changes(repository).await?
    {
        return Ok(Some(squash_merge_recovery_workflow_for_marker(&marker)));
    }

    // A command which did not produce a conflict or staged result must not
    // leave a phantom workflow lock behind. Successful uncommitted squash
    // merges clear the marker earlier; this is only a crash/error fallback.
    clear_squash_merge_recovery_marker(repository).await?;
    Ok(None)
}

async fn validate_squash_merge_recovery_target(
    repository: &Path,
    marker: &SquashMergeRecoveryMarker,
) -> Result<()> {
    let current_head = resolve_commit_reference(repository, "HEAD").await?;
    if current_head != marker.pre_merge_head {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "HEAD changed since this squash merge began. Do not finish or abandon it from a different history; resolve it from a terminal instead.",
        ));
    }
    let current_target_ref = checked_out_target_ref(repository).await?;
    if current_target_ref != marker.target_ref {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The checked-out branch changed since this squash merge began. Do not finish or abandon it from a different branch; resolve it from a terminal instead.",
        ));
    }
    Ok(())
}

async fn squash_merge_response_after_command(
    repository: &Path,
    command: Result<GitOutput>,
    marker: &SquashMergeRecoveryMarker,
) -> Result<GitOperationResponse> {
    match command {
        Ok(_) if marker.commit_squash => finish_squash_merge_recovery(repository).await,
        Ok(output) => {
            clear_squash_merge_recovery_marker(repository).await?;
            let message = "Squash merge completed successfully. Its changes are staged for review and an explicit commit.";
            let mut response = GitOperationResponse::success(join_output_or(&output, message));
            response.message = Some(message.to_string());
            response.operation_state = Some(git_operation_state(repository).await?);
            Ok(response)
        }
        Err(error) => {
            let state = git_operation_state(repository).await?;
            if state.in_progress {
                let mut response = GitOperationResponse::success(
                    error
                        .body
                        .details
                        .clone()
                        .unwrap_or_else(|| error.body.error.clone()),
                );
                response.message = state.message.clone();
                response.operation_state = Some(state);
                Ok(response)
            } else {
                // `git_operation_state` removes a stale marker when the
                // command left neither conflicts nor an index change.
                Err(error)
            }
        }
    }
}

async fn finish_squash_merge_recovery(repository: &Path) -> Result<GitOperationResponse> {
    let marker = read_squash_merge_recovery_marker(repository)
        .await?
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "There is no squash merge recovery to finish",
            )
        })?;
    validate_squash_merge_recovery_target(repository, &marker).await?;
    let conflicts = squash_merge_conflicted_paths(repository).await?;
    if !conflicts.is_empty() {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "Resolve and stage all {} conflicted squash merge path(s) before finishing recovery",
                conflicts.len()
            ),
        ));
    }
    if !squash_merge_has_staged_changes(repository).await? {
        clear_squash_merge_recovery_marker(repository).await?;
        let message = "Squash merge recovery was closed because no staged squash changes remain.";
        let mut response = GitOperationResponse::success(message);
        response.message = Some(message.to_string());
        response.operation_state = Some(git_operation_state(repository).await?);
        return Ok(response);
    }
    if !marker.commit_squash {
        clear_squash_merge_recovery_marker(repository).await?;
        let message = "Squash merge conflicts are resolved. The resolved squash changes remain staged for review and an explicit commit.";
        let mut response = GitOperationResponse::success(message);
        response.message = Some(message.to_string());
        response.operation_state = Some(git_operation_state(repository).await?);
        return Ok(response);
    }

    let mut args = vec!["commit".to_string()];
    if let Some(message) = marker.message.as_deref() {
        args.push("-m".to_string());
        args.push(message.to_string());
    } else {
        args.push("--no-edit".to_string());
    }
    match git_with_noninteractive_editor(repository, args).await {
        Ok(output) => {
            clear_squash_merge_recovery_marker(repository).await?;
            let message = "Squash merge commit completed successfully.";
            let mut response = GitOperationResponse::success(join_output_or(&output, message));
            response.message = Some(message.to_string());
            response.operation_state = Some(git_operation_state(repository).await?);
            Ok(response)
        }
        Err(error) => workflow_response_after_command(
            repository,
            Err(error),
            "Squash merge commit completed successfully.",
        )
        .await,
    }
}

async fn abort_squash_merge_recovery(repository: &Path) -> Result<GitOperationResponse> {
    let marker = read_squash_merge_recovery_marker(repository)
        .await?
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "There is no squash merge recovery to abandon",
            )
        })?;
    validate_squash_merge_recovery_target(repository, &marker).await?;
    let output = git(repository, ["reset", "--merge", marker.pre_merge_head.as_str()]).await?;
    clear_squash_merge_recovery_marker(repository).await?;
    let message = "Squash merge recovery was abandoned and the clean pre-merge worktree was restored.";
    let mut response = GitOperationResponse::success(join_output_or(&output, message));
    response.message = Some(message.to_string());
    response.operation_state = Some(git_operation_state(repository).await?);
    Ok(response)
}

async fn rebase(
    State(state): State<AppState>,
    Json(body): Json<RebaseBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(
        &state,
        body.project.project_ref()?,
        body.project.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    if body.interactive {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Interactive rebase needs an editable plan. Use the structured rebase-plan endpoint to review, reorder, squash, fixup, reword, or drop commits before applying it.",
        ));
    }
    validate_workflow_target_snapshot(
        &project_path,
        &body.expected_head,
        &body.expected_target_ref,
        "rebase",
    )
    .await?;
    // A configured rebase.autoStash would otherwise create and later remove a
    // stash behind the user's back. LazyGit makes that state visible in its
    // interactive workflow; the native normal rebase has no such surface, so
    // require an explicitly clean worktree before rewriting history.
    advanced_require_clean_worktree(&project_path, "rebase").await?;
    let upstream = resolve_commit_reference(&project_path, &body.upstream).await?;
    advanced_require_no_ignored_path_collision(
        &project_path,
        &advanced_rebase_write_paths(&project_path, &upstream).await?,
        "rebase",
    )
    .await?;

    // LazyGit drives branch rebases through Git's interactive, merge-
    // preserving mode.  The native client has no terminal todo editor for
    // this quick path, so `git_with_noninteractive_editor` accepts Git's
    // generated default todo unchanged.  That retains merge topology and
    // intentional empty commits rather than silently flattening history.
    //
    // We deliberately keep `--no-autostash`: unlike LazyGit's terminal
    // session, the touch client cannot show and recover a hidden autostash
    // conflict, and the clean-worktree precondition above makes that
    // difference explicit and safe.
    let args = vec![
        "rebase".to_string(),
        "--interactive".to_string(),
        "--no-autostash".to_string(),
        "--keep-empty".to_string(),
        "--no-autosquash".to_string(),
        "--rebase-merges".to_string(),
        upstream,
    ];

    let result = git_with_noninteractive_editor(&project_path, args).await;
    let response = workflow_response_after_command(
        &project_path,
        result,
        "Rebase completed successfully with merge topology preserved",
    )
    .await?;
    Ok(Json(response))
}

async fn revert_commit(
    State(state): State<AppState>,
    Json(body): Json<RevertBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(
        &state,
        body.project.project_ref()?,
        body.project.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;
    validate_workflow_target_snapshot(
        &project_path,
        &body.expected_head,
        &body.expected_target_ref,
        "revert",
    )
    .await?;

    let requested = normalize_revert_commits(&body)?;
    let mut commits = Vec::with_capacity(requested.len());
    let mut seen = HashSet::new();
    for requested_commit in requested {
        let commit = resolve_commit_reference(&project_path, &requested_commit).await?;
        if !seen.insert(commit.clone()) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "A commit can only be reverted once in the same request",
            ));
        }
        commits.push(commit);
    }

    // LazyGit defaults every merge in a selected range to its first parent.
    // Keep that behavior explicit and deterministic here. `--mainline` is an
    // invocation-wide Git option, so a custom parent must be valid for every
    // selected merge rather than silently applying different parents.
    let mainline = resolve_merge_mainline(&project_path, &commits, body.mainline).await?;

    // LazyGit offers a visible auto-stash prompt when an inverse commit needs
    // a clean tree. The native client deliberately has no hidden auto-stash;
    // require users to commit, stash, or discard their edits first instead.
    advanced_require_clean_worktree(&project_path, "revert commits").await?;

    let mut args = vec!["revert".to_string(), "--no-edit".to_string()];
    if let Some(mainline) = mainline {
        args.push("--mainline".to_string());
        args.push(mainline.to_string());
    }
    args.extend(commits);

    let response = workflow_response_after_command(
        &project_path,
        git_with_noninteractive_editor(&project_path, args).await,
        "Revert commit(s) created successfully",
    )
    .await?;
    Ok(Json(response))
}

/// Normalize the old `{ commit }` request and the contextual `{ commits }`
/// request without changing the supplied range order. An empty string is not
/// a valid fallback: it makes error handling dependent on Git's parser.
fn normalize_revert_commits(body: &RevertBody) -> Result<Vec<String>> {
    let legacy = body
        .commit
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if legacy.is_some() && !body.commits.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Send either commit or commits when reverting history, not both",
        ));
    }
    let commits = if let Some(commit) = legacy {
        vec![commit]
    } else {
        body.commits
            .iter()
            .map(|commit| commit.trim().to_string())
            .collect::<Vec<_>>()
    };
    if commits.is_empty() || commits.len() > REVERT_MAX_COMMITS {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Choose between one and 100 commits to revert",
        ));
    }
    if commits.iter().any(|commit| commit.is_empty()) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Every selected commit must be a non-empty commit hash or reference",
        ));
    }
    Ok(commits)
}

/// Resolve one invocation-wide mainline for a selected Git range. Git accepts
/// one `--mainline` value for the entire command. We therefore use
/// first-parent by default when any selected entry is a merge, exactly as
/// LazyGit does, and reject a custom parent that is invalid for one of them.
async fn resolve_merge_mainline(
    repository_path: &Path,
    commits: &[String],
    requested_mainline: Option<usize>,
) -> Result<Option<usize>> {
    let mut merge_parent_counts = Vec::new();
    for commit in commits {
        let parent_count = commit_parent_count(repository_path, commit).await?;
        if parent_count > 1 {
            merge_parent_counts.push(parent_count);
        }
    }

    if merge_parent_counts.is_empty() {
        if requested_mainline.is_some() {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "mainline is only valid when at least one selected commit is a merge commit",
            ));
        }
        return Ok(None);
    }

    let mainline = requested_mainline.unwrap_or(1);
    if mainline == 0 || merge_parent_counts.iter().any(|count| mainline > *count) {
        let smallest_parent_count = merge_parent_counts.into_iter().min().unwrap_or(1);
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "mainline must be between 1 and {smallest_parent_count} for every selected merge commit"
            ),
        ));
    }
    Ok(Some(mainline))
}

async fn commit_parent_count(repository_path: &Path, commit: &str) -> Result<usize> {
    Ok(git(
        repository_path,
        ["rev-list", "--parents", "-n", "1", commit],
    )
    .await?
    .stdout
    .split_whitespace()
    .count()
    .saturating_sub(1))
}

/// `--mainline` is meaningful only for merge commits. Keep the ignored-path
/// collision preflight faithful to the command Git will run without asking
/// `commit^1` of an ordinary/root commit in a mixed selection.
async fn cherry_pick_commit_write_paths(
    repository_path: &Path,
    commit: &str,
    mainline: Option<usize>,
) -> Result<Vec<String>> {
    let mainline = (commit_parent_count(repository_path, commit).await? > 1)
        .then_some(mainline)
        .flatten();
    advanced_commit_write_paths(repository_path, commit, mainline).await
}

/// `--empty=keep` replaced the older spelling in Git 2.45. Keep parsing
/// intentionally conservative: unrecognised vendor output takes the legacy
/// path, which is supported by the older Git releases this server targets.
fn git_supports_cherry_pick_empty_keep(version: &str) -> bool {
    let mut components = version
        .trim()
        .strip_prefix("git version ")
        .unwrap_or(version.trim())
        .split(|character: char| !character.is_ascii_digit())
        .filter(|component| !component.is_empty())
        .filter_map(|component| component.parse::<u64>().ok());
    let Some(major) = components.next() else {
        return false;
    };
    let Some(minor) = components.next() else {
        return false;
    };
    major > 2 || (major == 2 && minor >= 45)
}

/// Preserve intentionally empty and already-applied commits like LazyGit.
/// This avoids stranding a source-selection range in empty-pick recovery just
/// because an earlier selected commit made a later one redundant.
async fn cherry_pick_empty_preservation_args(repository_path: &Path) -> Vec<String> {
    let supports_empty_keep = git(repository_path, ["--version"])
        .await
        .map(|output| git_supports_cherry_pick_empty_keep(&output.stdout))
        .unwrap_or(false);
    vec![
        "--allow-empty".to_string(),
        if supports_empty_keep {
            "--empty=keep".to_string()
        } else {
            "--keep-redundant-commits".to_string()
        },
    ]
}

async fn reset(
    State(state): State<AppState>,
    Json(body): Json<ResetBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(
        &state,
        body.project.project_ref()?,
        body.project.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    ensure_no_git_workflow_in_progress(&project_path).await?;

    validate_workflow_target_snapshot(
        &project_path,
        &body.expected_head,
        &body.expected_target_ref,
        "reset",
    )
    .await?;

    let mode = validate_reset_mode(&body.mode)?;
    if mode == "hard" && !body.confirm_hard {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Hard reset replaces tracked worktree changes. Set confirmHard to true after showing a destructive-action confirmation.",
        ));
    }
    let target = resolve_commit_reference(&project_path, &body.target).await?;
    if mode == "hard" {
        let head = resolve_commit_reference(&project_path, "HEAD").await?;
        advanced_require_no_ignored_path_collision(
            &project_path,
            &advanced_tree_transition_write_paths(&project_path, &head, &target).await?,
            "hard reset",
        )
        .await?;
    }
    let output = git(
        &project_path,
        ["reset", &format!("--{mode}"), target.as_str()],
    )
    .await?;
    let message = format!("{mode} reset completed successfully");
    let mut response = GitOperationResponse::success(join_output_or(&output, &message));
    response.message = Some(message);
    response.operation_state = Some(git_operation_state(&project_path).await?);
    Ok(Json(response))
}

async fn continue_operation(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(
        &state,
        body.project_ref()?,
        body.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    let workflow = detect_git_workflow(&project_path).await?.ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            "There is no Git operation to continue",
        )
    })?;
    if matches!(
        workflow.kind,
        GitWorkflowKind::SquashMerge | GitWorkflowKind::CommittedSquashMerge
    ) {
        return Ok(Json(finish_squash_merge_recovery(&project_path).await?));
    }
    if workflow.kind == GitWorkflowKind::Stash {
        return Ok(Json(management_finish_stash_recovery(&project_path).await?));
    }
    if workflow.kind == GitWorkflowKind::StashUnstaged {
        return Ok(Json(
            management_finish_stash_unstaged_recovery(&project_path).await?,
        ));
    }
    if workflow.kind == GitWorkflowKind::Bisect {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Git bisect has its own controls. Mark the current revision good or bad, or reset the bisect.",
        ));
    }
    let message = format!("{} completed successfully", workflow.kind.display_name());
    let response = workflow_response_after_command(
        &project_path,
        git_with_noninteractive_editor(
            &project_path,
            [workflow_command_name(workflow.kind), "--continue"],
        )
        .await,
        &message,
    )
    .await?;
    Ok(Json(response))
}

async fn abort_operation(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(
        &state,
        body.project_ref()?,
        body.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    let workflow = detect_git_workflow(&project_path).await?.ok_or_else(|| {
        ServerError::new(StatusCode::BAD_REQUEST, "There is no Git operation to abort")
    })?;
    if matches!(
        workflow.kind,
        GitWorkflowKind::SquashMerge | GitWorkflowKind::CommittedSquashMerge
    ) {
        return Ok(Json(abort_squash_merge_recovery(&project_path).await?));
    }
    if workflow.kind == GitWorkflowKind::Stash {
        return Ok(Json(management_abort_stash_recovery(&project_path).await?));
    }
    if workflow.kind == GitWorkflowKind::StashUnstaged {
        return Ok(Json(
            management_abort_stash_unstaged_recovery(&project_path).await?,
        ));
    }
    if workflow.kind == GitWorkflowKind::Bisect {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Git bisect has its own controls. Mark the current revision good or bad, or reset the bisect.",
        ));
    }
    let output = git_with_noninteractive_editor(
        &project_path,
        [workflow_command_name(workflow.kind), "--abort"],
    )
    .await?;
    let message = format!("{} aborted successfully", workflow.kind.display_name());
    let mut response = GitOperationResponse::success(join_output_or(&output, &message));
    response.message = Some(message);
    response.operation_state = Some(git_operation_state(&project_path).await?);
    Ok(Json(response))
}

async fn skip_operation(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_git_repository_path(
        &state,
        body.project_ref()?,
        body.repository_id.as_deref(),
    )
    .await?;
    validate_git_repository(&project_path).await?;
    let workflow = detect_git_workflow(&project_path).await?.ok_or_else(|| {
        ServerError::new(StatusCode::BAD_REQUEST, "There is no Git operation to skip")
    })?;
    if !workflow.kind.can_skip() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Skip is only available for rebase, cherry-pick, or revert",
        ));
    }
    let message = format!("{} skip completed successfully", workflow.kind.display_name());
    let response = workflow_response_after_command(
        &project_path,
        git_with_noninteractive_editor(
            &project_path,
            [workflow_command_name(workflow.kind), "--skip"],
        )
        .await,
        &message,
    )
    .await?;
    Ok(Json(response))
}

fn workflow_command_name(kind: GitWorkflowKind) -> &'static str {
    match kind {
        GitWorkflowKind::Merge => "merge",
        // Squash merges have no MERGE_HEAD, so their marker-backed recovery
        // is handled before this generic Git command dispatcher.
        GitWorkflowKind::SquashMerge | GitWorkflowKind::CommittedSquashMerge => {
            unreachable!("squash merge recovery uses dedicated handlers")
        }
        GitWorkflowKind::Rebase | GitWorkflowKind::RebaseEdit => "rebase",
        GitWorkflowKind::CherryPick => "cherry-pick",
        GitWorkflowKind::Revert => "revert",
        // Stash application has no `git stash --continue` / `--abort`
        // command. The generic endpoints dispatch it to the dedicated,
        // marker-backed recovery handlers above before reaching this match.
        GitWorkflowKind::Stash | GitWorkflowKind::StashUnstaged => {
            unreachable!("stash recovery uses dedicated handlers")
        }
        // Bisect is an intentionally separate workflow: good/bad/reset have
        // their own guarded endpoints and must never be sent through the
        // generic sequencer controls.
        GitWorkflowKind::Bisect => unreachable!("bisect uses dedicated controls"),
    }
}

async fn git_with_noninteractive_editor<I, S>(cwd: &Path, args: I) -> Result<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    // Git invokes the value through its editor command path, where ':' is a
    // no-op on the POSIX shell Git uses for command editors. This preserves
    // Git's generated messages while preventing a server request from waiting
    // for an unavailable editor.
    // Interactive rebase and `pull.rebase=interactive` additionally invoke
    // GIT_SEQUENCE_EDITOR for the todo list. LazyGit uses the same no-op
    // command for Pull, so direct mobile Pull accepts Git's default plan
    // rather than waiting for a terminal editor. Dedicated native rebase-plan
    // editing supplies its own explicit sequence-editor path after review.
    git_with_environment(cwd, args, &[("GIT_EDITOR", ":"), ("GIT_SEQUENCE_EDITOR", ":")]).await
}

async fn ensure_no_git_workflow_in_progress(repository_path: &Path) -> Result<()> {
    if let Some(workflow) = detect_git_workflow(repository_path).await? {
        let instruction = if workflow.kind == GitWorkflowKind::Bisect {
            "Mark the current revision good or bad, or reset the bisect before starting another Git action."
        } else {
            "Resolve it and use the Git operation controls to continue, skip, or abort before starting another workflow."
        };
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "A {} is already in progress. {instruction}",
                workflow.kind.wire_name(),
            ),
        ));
    }
    Ok(())
}

/// The dedicated bisect controls are the sole exception to the normal
/// workflow lock. A mark/reset call must be able to operate *within* the
/// active bisect, but it must never run on top of a merge, rebase,
/// cherry-pick, revert, or stash recovery.
async fn ensure_no_non_bisect_git_workflow_in_progress(repository_path: &Path) -> Result<()> {
    if let Some(workflow) = detect_git_workflow(repository_path).await?
        && workflow.kind != GitWorkflowKind::Bisect
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "A {} is already in progress. Resolve it before using Git bisect controls.",
                workflow.kind.wire_name(),
            ),
        ));
    }
    Ok(())
}

async fn workflow_response_after_command(
    repository_path: &Path,
    command: Result<GitOutput>,
    complete_message: &str,
) -> Result<GitOperationResponse> {
    match command {
        Ok(output) => {
            let state = git_operation_state(repository_path).await?;
            let mut response = GitOperationResponse::success(join_output_or(&output, complete_message));
            response.message = Some(if state.in_progress {
                state
                    .message
                    .clone()
                    .unwrap_or_else(|| "Git operation is in progress".to_string())
            } else {
                complete_message.to_string()
            });
            response.operation_state = Some(state);
            Ok(response)
        }
        Err(error) => {
            let state = git_operation_state(repository_path).await?;
            // A conflict is an expected, recoverable result of these commands.
            // Return a successful transport response with operation metadata so
            // clients can render the resolver rather than treating the workflow
            // as a terminal server error.
            if state.in_progress {
                let mut response = GitOperationResponse::success(
                    error
                        .body
                        .details
                        .clone()
                        .unwrap_or_else(|| error.body.error.clone()),
                );
                response.message = state
                    .message
                    .clone()
                    .or_else(|| Some("Git operation is in progress".to_string()));
                response.operation_state = Some(state);
                Ok(response)
            } else {
                Err(error)
            }
        }
    }
}

async fn git_operation_state(repository_path: &Path) -> Result<GitOperationStateResponse> {
    Ok(detect_git_workflow(repository_path)
        .await?
        .map(|workflow| workflow.response())
        .unwrap_or_default())
}

async fn detect_git_workflow(repository_path: &Path) -> Result<Option<DetectedGitWorkflow>> {
    let git_dir = git_metadata_directory(repository_path).await?;
    let rebase_merge = git_dir.join("rebase-merge");
    let rebase_apply = git_dir.join("rebase-apply");
    if git_metadata_exists(&rebase_merge).await? || git_metadata_exists(&rebase_apply).await? {
        let (current_step, total_steps) = rebase_progress(&rebase_merge, &rebase_apply).await?;
        return Ok(Some(DetectedGitWorkflow {
            kind: if rebase_merge_is_edit_stop(&git_dir, &rebase_merge).await? {
                GitWorkflowKind::RebaseEdit
            } else {
                GitWorkflowKind::Rebase
            },
            current_commit: read_git_metadata_value(&git_dir.join("REBASE_HEAD")).await?,
            current_step,
            total_steps,
            empty_step_recovery: EmptyStepRecovery::None,
        }));
    }

    for (marker, kind) in [
        ("CHERRY_PICK_HEAD", GitWorkflowKind::CherryPick),
        ("REVERT_HEAD", GitWorkflowKind::Revert),
        ("MERGE_HEAD", GitWorkflowKind::Merge),
    ] {
        let marker_path = git_dir.join(marker);
        if git_metadata_exists(&marker_path).await? {
            return Ok(Some(DetectedGitWorkflow {
                kind,
                current_commit: read_git_metadata_value(&marker_path).await?,
                current_step: None,
                total_steps: None,
                empty_step_recovery: if kind == GitWorkflowKind::CherryPick
                    && git_worktree_is_clean(repository_path).await?
                {
                    EmptyStepRecovery::Skip
                } else {
                    EmptyStepRecovery::None
                },
            }));
        }
    }

    if let Some(workflow) = sequencer_only_revert_workflow(repository_path, &git_dir).await? {
        return Ok(Some(workflow));
    }

    if let Some(workflow) = squash_merge_recovery_workflow(repository_path).await? {
        return Ok(Some(workflow));
    }

    if let Some(workflow) = management_stash_recovery_workflow(repository_path).await? {
        return Ok(Some(workflow));
    }

    if let Some(workflow) = management_stash_unstaged_recovery_workflow(repository_path).await? {
        return Ok(Some(workflow));
    }

    // A bisect checks out a succession of revisions while it is active. It
    // is a repository-wide workflow just like rebase, so it must lock normal
    // mutations and remain visible even when the user navigates away from its
    // dedicated dialog.
    if git_metadata_exists(&git_dir.join("BISECT_START")).await? {
        let current_commit = git(repository_path, ["rev-parse", "HEAD"])
            .await
            .ok()
            .and_then(|output| {
                let value = output.stdout.trim();
                (!value.is_empty()).then(|| value.to_string())
            });
        return Ok(Some(DetectedGitWorkflow {
            kind: GitWorkflowKind::Bisect,
            current_commit,
            current_step: None,
            total_steps: None,
            empty_step_recovery: EmptyStepRecovery::None,
        }));
    }

    Ok(None)
}

/// Git creates `rebase-merge/amend` only after an interactive `edit` todo
/// command has successfully replayed its commit and intentionally paused for
/// an amendment. A normal conflict also has `REBASE_HEAD` and `stopped-sha`,
/// but no `amend` marker. The `amend` marker records the replayed commit
/// while `stopped-sha` and `REBASE_HEAD` retain the original source commit.
/// Require the final todo entry to be the matching `edit`, valid immutable
/// IDs, and that source pair. Treat malformed or partial metadata as an
/// ordinary recovery state instead of guessing that it is safe to amend.
async fn rebase_merge_is_edit_stop(git_dir: &Path, rebase_merge: &Path) -> Result<bool> {
    if !git_metadata_exists(rebase_merge).await? {
        return Ok(false);
    }
    let (Some(amend), Some(stopped), Some(rebase_head), Some(done)) = (
        read_git_metadata_value(&rebase_merge.join("amend")).await?,
        read_git_metadata_value(&rebase_merge.join("stopped-sha")).await?,
        read_git_metadata_value(&git_dir.join("REBASE_HEAD")).await?,
        read_git_metadata_value(&rebase_merge.join("done")).await?,
    ) else {
        return Ok(false);
    };
    let is_full_object_id = |value: &str| {
        matches!(value.len(), 40 | 64) && value.chars().all(|character| character.is_ascii_hexdigit())
    };
    let last_done = done
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'));
    let is_matching_edit = last_done.is_some_and(|line| {
        let mut fields = line.split_whitespace();
        fields.next() == Some("edit") && fields.next() == Some(stopped.as_str())
    });
    Ok(is_matching_edit
        && is_full_object_id(&amend)
        && is_full_object_id(&stopped)
        && is_full_object_id(&rebase_head)
        && stopped == rebase_head)
}

/// Return the first pending `revert <commit>` from Git's sequencer todo file.
/// A conflict has a `REVERT_HEAD` and is handled before this fallback. An
/// empty step in a range has only this sequencer data, so ignoring it leaves
/// ordinary mutations unlocked and the recovery controls unreachable.
fn sequencer_todo_revert_commit(todo: &str) -> Option<String> {
    let line = todo
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;
    let mut fields = line.split_whitespace();
    (fields.next()? == "revert")
        .then(|| fields.next().map(str::to_string))
        .flatten()
}

async fn sequencer_only_revert_workflow(
    repository_path: &Path,
    git_dir: &Path,
) -> Result<Option<DetectedGitWorkflow>> {
    let todo_path = git_dir.join("sequencer").join("todo");
    let Some(todo) = read_git_metadata_value(&todo_path).await? else {
        return Ok(None);
    };
    let Some(current_commit) = sequencer_todo_revert_commit(&todo) else {
        return Ok(None);
    };

    Ok(Some(DetectedGitWorkflow {
        kind: GitWorkflowKind::Revert,
        current_commit: Some(current_commit),
        current_step: None,
        total_steps: None,
        empty_step_recovery: if git_worktree_is_clean(repository_path).await? {
            EmptyStepRecovery::Continue
        } else {
            EmptyStepRecovery::None
        },
    }))
}

/// A clean worktree while Git still has `CHERRY_PICK_HEAD` is Git's
/// already-applied/empty-patch state. It is not a conflict to resolve and
/// cannot be completed with `git cherry-pick --continue` without manually
/// creating an empty commit. The same clean state paired with a revert
/// sequencer means `git revert --continue` can safely advance the empty step.
async fn git_worktree_is_clean(repository_path: &Path) -> Result<bool> {
    let output = git(
        repository_path,
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .await?;
    Ok(output.stdout_bytes.is_empty())
}

async fn git_metadata_directory(repository_path: &Path) -> Result<PathBuf> {
    let output = git(repository_path, ["rev-parse", "--git-dir"]).await?;
    let value = output.stdout.trim();
    if value.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Could not resolve the Git metadata directory",
        ));
    }
    let path = PathBuf::from(value);
    Ok(if path.is_absolute() {
        path
    } else {
        repository_path.join(path)
    })
}

async fn git_metadata_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_server_error(error)),
    }
}

async fn read_git_metadata_value(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path).await {
        Ok(value) => Ok((!value.trim().is_empty()).then(|| value.trim().to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_server_error(error)),
    }
}

async fn rebase_progress(
    rebase_merge: &Path,
    rebase_apply: &Path,
) -> Result<(Option<usize>, Option<usize>)> {
    if git_metadata_exists(rebase_merge).await? {
        return Ok((
            read_git_metadata_number(&rebase_merge.join("msgnum")).await?,
            read_git_metadata_number(&rebase_merge.join("end")).await?,
        ));
    }
    Ok((
        read_git_metadata_number(&rebase_apply.join("next")).await?,
        read_git_metadata_number(&rebase_apply.join("last")).await?,
    ))
}

async fn read_git_metadata_number(path: &Path) -> Result<Option<usize>> {
    Ok(read_git_metadata_value(path)
        .await?
        .and_then(|value| value.parse::<usize>().ok()))
}

fn validate_reset_mode(mode: &str) -> Result<&str> {
    match mode.trim() {
        "soft" => Ok("soft"),
        "mixed" => Ok("mixed"),
        "hard" => Ok("hard"),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Reset mode must be soft, mixed, or hard",
        )),
    }
}

fn validate_immutable_expected_head(value: &str) -> Result<&str> {
    let value = value.trim();
    // Git supports SHA-1 and SHA-256 repositories. Require a full object ID
    // rather than accepting an abbreviated hash or ref name, both of which
    // could resolve differently between confirmation and execution.
    if !matches!(value.len(), 40 | 64)
        || !value.chars().all(|character| character.is_ascii_hexdigit())
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "expectedHead must be the full immutable commit hash returned by Git status",
        ));
    }
    Ok(value)
}

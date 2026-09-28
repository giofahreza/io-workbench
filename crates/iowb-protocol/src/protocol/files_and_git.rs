#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: FileKind,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContentResponse {
    pub path: String,
    pub content: String,
    pub size: u64,
    #[serde(
        rename = "contentEncoding",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub content_encoding: Option<String>,
    #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileRequest {
    #[serde(rename = "filePath")]
    pub file_path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFileRequest {
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub directory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameFileRequest {
    #[serde(rename = "oldPath")]
    pub old_path: String,
    #[serde(rename = "newPath")]
    pub new_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRenameFileRequest {
    pub entries: Vec<RenameFileRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyFileRequest {
    #[serde(rename = "sourcePath")]
    pub source_path: String,
    #[serde(rename = "targetPath")]
    pub target_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCopyFileRequest {
    pub entries: Vec<CopyFileRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFileRequest {
    #[serde(rename = "filePath")]
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchDeleteFileRequest {
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseFilesystemResponse {
    pub path: String,
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatusRequest {
    #[serde(rename = "projectPath")]
    pub project_path: String,
    #[serde(
        rename = "repositoryId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub repository_id: Option<String>,
}

/// Explicit destructive counterpart to a worktree-only file discard. The
/// caller must confirm because this resets both the index and working tree,
/// including any staged content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiscardAllRequest {
    pub project: String,
    pub file: String,
    #[serde(default)]
    pub confirm: bool,
    #[serde(
        rename = "repositoryId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub repository_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitRepositoryKind {
    Root,
    Submodule,
    Nested,
    Worktree,
    Uninitialized,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRepositorySummary {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(rename = "relativePath")]
    pub relative_path: String,
    pub kind: GitRepositoryKind,
    pub initialized: bool,
    #[serde(rename = "isDefault")]
    pub is_default: bool,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitWorkspaceResponse {
    #[serde(rename = "projectPath")]
    pub project_path: String,
    #[serde(rename = "hasRootRepository")]
    pub has_root_repository: bool,
    #[serde(rename = "defaultRepositoryId")]
    pub default_repository_id: Option<String>,
    pub repositories: Vec<GitRepositorySummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatusResponse {
    #[serde(rename = "repositoryId", skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<String>,
    #[serde(rename = "repositoryPath", skip_serializing_if = "Option::is_none")]
    pub repository_path: Option<String>,
    #[serde(rename = "repositoryName", skip_serializing_if = "Option::is_none")]
    pub repository_name: Option<String>,
    #[serde(rename = "repositoryKind", skip_serializing_if = "Option::is_none")]
    pub repository_kind: Option<GitRepositoryKind>,
    pub branch: Option<String>,
    /// Canonical checkout identity paired with `head`. This is a fully
    /// qualified local branch ref (for example `refs/heads/main`) or `HEAD`
    /// for detached checkouts, so clients can reject same-OID branch swaps.
    #[serde(rename = "targetRef", skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<String>,
    #[serde(rename = "hasCommits")]
    pub has_commits: bool,
    /// Immutable object ID for the currently checked-out commit, when one
    /// exists. Clients use this as an optimistic-concurrency guard before
    /// history-rewriting actions such as reset, amend, and rebase-plan apply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Immutable tree object ID produced from the current Git index. A
    /// commit request must echo this snapshot so the server can reject an
    /// index that changed after the user reviewed the staged files.
    #[serde(rename = "indexTree", skip_serializing_if = "Option::is_none")]
    pub index_tree: Option<String>,
    #[serde(default)]
    pub modified: Vec<String>,
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub deleted: Vec<String>,
    #[serde(default)]
    pub untracked: Vec<String>,
    #[serde(default)]
    pub conflicted: Vec<String>,
    pub clean: bool,
    pub files: Vec<GitFileStatus>,
    pub raw: String,
    #[serde(rename = "operationState", skip_serializing_if = "Option::is_none")]
    pub operation_state: Option<GitOperationStateResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileStatus {
    pub path: String,
    pub status: String,
    #[serde(rename = "submoduleState", default, skip_serializing_if = "Option::is_none")]
    pub submodule_state: Option<String>,
    #[serde(rename = "stagedAdditions", default)]
    pub staged_additions: usize,
    #[serde(rename = "stagedDeletions", default)]
    pub staged_deletions: usize,
    #[serde(rename = "unstagedAdditions", default)]
    pub unstaged_additions: usize,
    #[serde(rename = "unstagedDeletions", default)]
    pub unstaged_deletions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConflictsResponse {
    pub files: Vec<GitConflictSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConflictSummary {
    pub path: String,
    pub status: String,
    #[serde(rename = "conflictCount")]
    pub conflict_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConflictFileResponse {
    pub path: String,
    pub status: String,
    pub content: String,
    pub conflicts: Vec<GitConflictRegion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConflictRegion {
    #[serde(rename = "startLine")]
    pub start_line: usize,
    #[serde(rename = "endLine")]
    pub end_line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub ours: String,
    pub theirs: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitOperationResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(rename = "remoteName", skip_serializing_if = "Option::is_none")]
    pub remote_name: Option<String>,
    #[serde(rename = "remoteUrl", skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    #[serde(rename = "remoteBranch", skip_serializing_if = "Option::is_none")]
    pub remote_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(rename = "operationState", skip_serializing_if = "Option::is_none")]
    pub operation_state: Option<GitOperationStateResponse>,
}

impl GitOperationResponse {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: Some(output.into()),
            message: None,
            error: None,
            details: None,
            remote_name: None,
            remote_url: None,
            remote_branch: None,
            branch: None,
            operation_state: None,
        }
    }

    pub fn message(message: impl Into<String>) -> Self {
        Self {
            success: true,
            output: None,
            message: Some(message.into()),
            error: None,
            details: None,
            remote_name: None,
            remote_url: None,
            remote_branch: None,
            branch: None,
            operation_state: None,
        }
    }
}

/// The recoverable Git operation currently owning the repository worktree,
/// including a running bisect that must keep ordinary Git mutations locked.
///
/// Git intentionally leaves a repository in this state after a conflict so a
/// client can resolve files and explicitly continue, skip, or abort. Keeping
/// this as a response object rather than inferring it from conflict files is
/// important: a conflict can come from a merge, rebase, cherry-pick, revert,
/// or stash application, and each operation has different safe recovery
/// actions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitOperationStateResponse {
    /// One of `none`, `merge`, `rebase`, `cherry-pick`, `revert`, or `stash`.
    pub operation: String,
    #[serde(rename = "inProgress")]
    pub in_progress: bool,
    #[serde(rename = "canContinue")]
    pub can_continue: bool,
    #[serde(rename = "canAbort")]
    pub can_abort: bool,
    #[serde(rename = "canSkip")]
    pub can_skip: bool,
    #[serde(rename = "currentCommit", skip_serializing_if = "Option::is_none")]
    pub current_commit: Option<String>,
    #[serde(rename = "currentStep", skip_serializing_if = "Option::is_none")]
    pub current_step: Option<usize>,
    #[serde(rename = "totalSteps", skip_serializing_if = "Option::is_none")]
    pub total_steps: Option<usize>,
    /// True only when an interactive rebase has stopped at an explicit
    /// `edit` todo item. This is not a conflict: clients may expose the
    /// narrowly scoped stage/amend controls needed before `rebase --continue`.
    #[serde(rename = "isEditStop", default)]
    pub is_edit_stop: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Default for GitOperationStateResponse {
    fn default() -> Self {
        Self {
            operation: "none".to_string(),
            in_progress: false,
            can_continue: false,
            can_abort: false,
            can_skip: false,
            current_commit: None,
            current_step: None,
            total_steps: None,
            is_edit_stop: false,
            message: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiffResponse {
    pub diff: String,
    #[serde(rename = "isTruncated", default)]
    pub is_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileWithDiffResponse {
    #[serde(rename = "currentContent")]
    pub current_content: String,
    #[serde(rename = "oldContent")]
    pub old_content: String,
    #[serde(rename = "isDeleted")]
    pub is_deleted: bool,
    #[serde(rename = "isUntracked")]
    pub is_untracked: bool,
    #[serde(rename = "submoduleState", skip_serializing_if = "Option::is_none")]
    pub submodule_state: Option<String>,
    #[serde(rename = "submoduleDiff", skip_serializing_if = "Option::is_none")]
    pub submodule_diff: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitBranchesResponse {
    pub branches: Vec<String>,
    #[serde(rename = "localBranches")]
    pub local_branches: Vec<String>,
    #[serde(rename = "remoteBranches")]
    pub remote_branches: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommitSummary {
    pub hash: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub message: String,
    #[serde(default)]
    pub stats: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommitsResponse {
    pub commits: Vec<GitCommitSummary>,
}

/// An immutable, source-scoped set of commits that can be cherry-picked onto
/// the currently checked-out branch.
///
/// `source_ref` is deliberately a fully qualified local or remote-tracking
/// ref.  The two object IDs are captured when the user opens the picker and
/// must be sent back when the operation is confirmed, so a later fetch,
/// checkout, or force-push cannot make the confirmation apply a different
/// range than the one the user reviewed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCherryPickPreviewResponse {
    #[serde(rename = "sourceRef")]
    pub source_ref: String,
    #[serde(rename = "sourceHead")]
    pub source_head: String,
    /// `refs/heads/...` for a branch checkout, or `HEAD` for detached state.
    #[serde(rename = "targetRef")]
    pub target_ref: String,
    #[serde(rename = "targetHead")]
    pub target_head: String,
    pub commits: Vec<GitCommitSummary>,
    /// Immutable hashes among `commits` that are merge commits. The native
    /// source picker uses this to make Git's required mainline choice visible
    /// before confirmation, rather than discovering it after a failed pick.
    #[serde(rename = "mergeCommitHashes", default)]
    pub merge_commit_hashes: Vec<String>,
    /// LazyGit's default mainline for a selected merge commit. Omitted when
    /// the source range contains no merges.
    #[serde(rename = "defaultMainline", skip_serializing_if = "Option::is_none")]
    pub default_mainline: Option<usize>,
}

/// A commit record suitable for a pageable repository-wide history view.
///
/// `parents` and `refs` deliberately contain immutable object IDs and
/// display-only ref labels respectively.  Together they let a client render
/// an accurate commit graph without trying to parse Git's terminal-oriented
/// `--graph` output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHistoryCommit {
    pub hash: String,
    #[serde(rename = "shortHash")]
    pub short_hash: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub message: String,
    #[serde(default)]
    pub stats: String,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub refs: Vec<String>,
}

/// Page of enhanced Git history.
///
/// The API intentionally uses an offset cursor because a free-text search can
/// change the result set between requests.  Clients must keep the returned
/// `allBranches` and `search` values when requesting `nextOffset`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHistoryResponse {
    pub commits: Vec<GitHistoryCommit>,
    pub offset: usize,
    pub limit: usize,
    #[serde(rename = "hasMore")]
    pub has_more: bool,
    #[serde(rename = "nextOffset", skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    #[serde(rename = "allBranches")]
    pub all_branches: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStashSummary {
    pub reference: String,
    pub hash: String,
    pub author: String,
    pub date: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStashesResponse {
    pub stashes: Vec<GitStashSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitTagSummary {
    pub name: String,
    pub hash: String,
    #[serde(rename = "objectType")]
    pub object_type: String,
    pub date: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitTagsResponse {
    pub tags: Vec<GitTagSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRemoteStatusResponse {
    #[serde(rename = "hasRemote")]
    pub has_remote: bool,
    #[serde(rename = "hasUpstream")]
    pub has_upstream: bool,
    pub branch: String,
    #[serde(rename = "remoteBranch", skip_serializing_if = "Option::is_none")]
    pub remote_branch: Option<String>,
    #[serde(rename = "remoteName", skip_serializing_if = "Option::is_none")]
    pub remote_name: Option<String>,
    /// The single remote Git would use for an ordinary push of the checked-out
    /// branch after applying `branch.<name>.pushRemote`, `remote.pushDefault`,
    /// the branch's tracking remote, and any representable
    /// `remote.<name>.push` refspec before `push.default`.
    ///
    /// This intentionally stays separate from `remoteName`: the latter is the
    /// pull/upstream remote, while a repository can send pushes to a dedicated
    /// review or release remote.
    #[serde(rename = "pushRemoteName", skip_serializing_if = "Option::is_none")]
    pub push_remote_name: Option<String>,
    /// The named branch paired with `pushRemoteName` for that ordinary push.
    #[serde(rename = "pushRemoteBranch", skip_serializing_if = "Option::is_none")]
    pub push_remote_branch: Option<String>,
    /// The effective single-branch `push.default` mode (`simple`, `current`,
    /// or `upstream`) that produced the reported push target, or
    /// `remote.<name>.push` when a single explicit remote refspec overrides
    /// the default mode.
    #[serde(rename = "pushMode", skip_serializing_if = "Option::is_none")]
    pub push_mode: Option<String>,
    /// True when Git's `push.autoSetupRemote=true` makes this ordinary push
    /// establish the selected branch's tracking upstream.
    #[serde(rename = "pushSetsUpstream", default)]
    pub push_sets_upstream: bool,
    /// Why no single safe default target is available, for example when
    /// `push.default=matching` would update multiple branches.
    #[serde(rename = "pushTargetMessage", skip_serializing_if = "Option::is_none")]
    pub push_target_message: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    #[serde(rename = "isUpToDate")]
    pub is_up_to_date: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

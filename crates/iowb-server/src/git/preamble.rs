use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use axum::{
    Extension, Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
};
use iowb_core::AppState;
use iowb_protocol::{
    GitBranchesResponse, GitCherryPickPreviewResponse, GitCommitSummary, GitCommitsResponse, GitConflictFileResponse,
    GitConflictRegion, GitConflictSummary, GitConflictsResponse, GitDiffResponse, GitFileStatus,
    GitDiscardAllRequest, GitFileWithDiffResponse, GitGenerateMessageResponse, GitHistoryCommit, GitHistoryResponse,
    GitOperationResponse, GitOperationStateResponse, GitRemoteStatusResponse, GitRepositoryKind,
    GitStashSummary, GitStashesResponse, GitStatusResponse, GitTagSummary, GitTagsResponse,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::{fs, io::AsyncWriteExt, process::Command};

use crate::{AuthenticatedUser, Result, ServerError};

const COMMIT_DIFF_CHARACTER_LIMIT: usize = 500_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/git/workspace", get(git_workspace))
        .route("/api/git/status", get(status))
        .route("/api/git/conflicts", get(conflicts))
        .route("/api/git/conflict-file", get(conflict_file))
        .route("/api/git/init", post(init))
        .route("/api/git/diff", get(diff))
        .route("/api/git/file-with-diff", get(file_with_diff))
        .route("/api/git/initial-commit", post(initial_commit))
        .route("/api/git/commit", post(commit))
        .route("/api/git/commit/amend", post(advanced_amend_head))
        .route("/api/git/commit/reword", post(advanced_reword_head))
        .route("/api/git/commit/fixup", post(advanced_create_fixup_commit))
        .route("/api/git/revert-local-commit", post(revert_local_commit))
        .route("/api/git/revert", post(revert_commit))
        .route("/api/git/reset", post(reset))
        .route("/api/git/operation-state", get(operation_state))
        .route("/api/git/cherry-pick/preview", get(cherry_pick_preview))
        .route("/api/git/cherry-pick", post(cherry_pick))
        .route("/api/git/merge", post(merge))
        .route("/api/git/rebase", post(rebase))
        .route(
            "/api/git/rebase/plan",
            get(advanced_rebase_plan_preview).post(advanced_apply_rebase_plan),
        )
        .route("/api/git/operation/continue", post(continue_operation))
        .route("/api/git/operation/abort", post(abort_operation))
        .route("/api/git/operation/skip", post(skip_operation))
        .route("/api/git/branches", get(branches))
        .route("/api/git/checkout", post(checkout))
        .route(
            "/api/git/checkout/tracking",
            post(checkout_tracking_branch),
        )
        .route("/api/git/checkout/detached", post(advanced_detached_checkout))
        .route(
            "/api/git/branch/create-and-checkout",
            post(create_and_checkout_branch),
        )
        .route("/api/git/create-branch", post(create_branch))
        .route("/api/git/delete-branch", post(delete_branch))
        .route(
            "/api/git/branch/delete-force",
            post(advanced_force_delete_local_branch),
        )
        .route("/api/git/branch/rename", post(management_rename_branch))
        .route(
            "/api/git/branch/create-from-ref",
            post(management_create_branch_from_ref),
        )
        .route(
            "/api/git/branch/upstream",
            post(management_set_branch_upstream),
        )
        .route(
            "/api/git/branch/unset-upstream",
            post(management_unset_branch_upstream),
        )
        .route(
            "/api/git/branch/delete-remote",
            post(management_delete_remote_branch),
        )
        .route("/api/git/commits", get(commits))
        .route("/api/git/history/enhanced", get(advanced_enhanced_history))
        .route("/api/git/commit-diff", get(commit_diff))
        .route("/api/git/history/file", get(management_file_log))
        .route("/api/git/history/blame", get(management_blame))
        .route("/api/git/history/reflog", get(management_reflog))
        .route("/api/git/history/compare", post(management_compare_refs))
        .route(
            "/api/git/history/restore-file",
            post(management_restore_file),
        )
        .route("/api/git/stashes", get(stashes))
        .route("/api/git/stash", post(create_stash))
        .route("/api/git/stash/apply", post(apply_stash))
        .route("/api/git/stash/pop", post(pop_stash))
        .route("/api/git/stash/drop", post(drop_stash))
        .route("/api/git/stash/rename", post(management_rename_stash))
        .route("/api/git/stash/show", get(management_show_stash))
        .route("/api/git/stash/files", get(management_stash_files))
        .route(
            "/api/git/stash/file-diff",
            get(management_stash_file_diff),
        )
        .route(
            "/api/git/stash/restore-files",
            post(management_restore_stash_files),
        )
        .route(
            "/api/git/stash/restore-hunks",
            post(management_restore_stash_hunks),
        )
        .route(
            "/api/git/stash/create-advanced",
            post(management_create_stash),
        )
        .route(
            "/api/git/stash/apply-advanced",
            post(management_apply_stash),
        )
        .route(
            "/api/git/stash/branch",
            post(management_create_branch_from_stash),
        )
        .route("/api/git/tags", get(tags))
        .route("/api/git/tag", post(create_tag))
        .route("/api/git/tag/delete", post(delete_tag))
        // Pushing a tag must name its destination explicitly.  Unlike a
        // branch with an established upstream, a tag has no natural remote;
        // choosing `origin` (or the first configured remote) here can publish
        // a release to the wrong forge when a repository has multiple remotes.
        .route("/api/git/tag/push", post(management_push_remote_tag))
        .route(
            "/api/git/tag/create-at",
            post(management_create_tag_at_target),
        )
        .route(
            "/api/git/tag/delete-remote",
            post(management_delete_remote_tag),
        )
        .route(
            "/api/git/generate-commit-message",
            post(generate_commit_message),
        )
        .route(
            "/api/git/config",
            get(advanced_git_config).post(advanced_set_git_config),
        )
        .route("/api/git/lfs", get(advanced_git_lfs_status))
        .route("/api/git/lfs/track", post(advanced_git_lfs_track))
        .route("/api/git/lfs/untrack", post(advanced_git_lfs_untrack))
        .route(
            "/api/git/sparse-checkout",
            get(advanced_sparse_checkout_status).post(advanced_set_sparse_checkout),
        )
        .route(
            "/api/git/sparse-checkout/disable",
            post(advanced_disable_sparse_checkout),
        )
        .route("/api/git/gitignore", post(advanced_append_gitignore))
        .route(
            "/api/git/integrations/status",
            get(advanced_git_integrations_status),
        )
        .route("/api/git/bisect", get(advanced_git_bisect_status))
        .route("/api/git/bisect/start", post(advanced_git_bisect_start))
        .route("/api/git/bisect/good", post(advanced_git_bisect_good))
        .route("/api/git/bisect/bad", post(advanced_git_bisect_bad))
        .route("/api/git/bisect/reset", post(advanced_git_bisect_reset))
        .route("/api/git/maintenance", post(advanced_git_maintenance))
        .route("/api/git/gc", post(advanced_git_gc))
        .route("/api/git/remote-status", get(remote_status))
        .route("/api/git/remote", post(set_remote))
        .route("/api/git/remotes", get(management_remotes))
        .route("/api/git/remote/url", post(management_update_remote_url))
        .route("/api/git/remote/remove", post(management_remove_remote))
        .route("/api/git/remote/rename", post(management_rename_remote))
        .route("/api/git/remote/fetch", post(management_fetch_remote))
        .route(
            "/api/git/remote/fetch-all",
            post(management_fetch_all_remotes),
        )
        .route("/api/git/remote/pull", post(management_pull_remote))
        .route("/api/git/remote/push", post(management_push_remote))
        .route(
            "/api/git/pull/rebase-plan",
            post(management_remote_rebase_plan),
        )
        .route("/api/git/fetch", post(fetch))
        .route("/api/git/pull", post(pull))
        .route("/api/git/push", post(push))
        .route("/api/git/publish", post(publish))
        .route("/api/git/stage", post(stage))
        .route("/api/git/unstage", post(unstage))
        .route("/api/git/apply-hunks", post(apply_hunks))
        .route("/api/git/resolve-conflict", post(resolve_conflict))
        .route("/api/git/discard", post(discard))
        .route("/api/git/discard-all", post(discard_all))
        .route("/api/git/delete-untracked", post(delete_untracked))
        .route("/api/git/submodule/add", post(advanced_add_submodule))
        .route("/api/git/submodule/remove", post(advanced_remove_submodule))
        .route(
            "/api/git/submodule/update",
            post(management_update_submodule),
        )
        .route("/api/git/submodule/sync", post(management_sync_submodule))
        .route("/api/git/worktrees", get(management_worktrees))
        .route("/api/git/worktree/add", post(management_add_worktree))
        .route("/api/git/worktree/remove", post(management_remove_worktree))
        .route("/api/git/worktree/prune", post(management_prune_worktrees))
}

#[derive(Debug, Deserialize)]
struct ProjectQuery {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
}

impl ProjectQuery {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug, Deserialize)]
struct DiffQuery {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    #[serde(rename = "staged")]
    staged: Option<bool>,
    file: Option<String>,
}

impl DiffQuery {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }

    fn file_ref(&self) -> Result<&str> {
        self.file
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "File path is required"))
    }
}

#[derive(Debug, Deserialize)]
struct CommitDiffQuery {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    commit: String,
}

impl CommitDiffQuery {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug, Deserialize)]
struct CommitsQuery {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    limit: Option<usize>,
    /// Optional local/remote branch, tag, or immutable commit to inspect
    /// without checking it out.  This keeps Branch -> View commits
    /// contextual instead of forcing a checkout merely to browse history.
    #[serde(rename = "ref")]
    reference: Option<String>,
}

impl CommitsQuery {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug, Deserialize)]
struct ProjectBody {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    #[serde(rename = "allowWorkspaceInit", default)]
    allow_workspace_init: bool,
}

impl ProjectBody {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug, Deserialize)]
struct BranchBody {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    branch: String,
}

impl BranchBody {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug, Deserialize)]
struct RemoteBody {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    name: Option<String>,
    url: String,
}

impl RemoteBody {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug, Deserialize)]
struct StashBody {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    message: Option<String>,
}

impl StashBody {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug, Deserialize)]
struct StashRefBody {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    reference: String,
    /// An optional immutable stash object ID supplied by native clients. The
    /// displayed `stash@{N}` selector is positional and can move while a
    /// confirmation sheet is open; legacy callers may still omit this.
    #[serde(rename = "expectedHash", default)]
    expected_hash: Option<String>,
}

impl StashRefBody {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug, Deserialize)]
struct TagBody {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    tag: String,
    message: Option<String>,
    /// Replacing a tag rewrites a local ref that may already have been
    /// published, so it is intentionally opt-in and confirmation-bound.
    #[serde(default)]
    force: bool,
    #[serde(default)]
    confirm: bool,
}

impl TagBody {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug, Deserialize)]
struct CommitBody {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    message: String,
    files: Vec<String>,
    #[serde(rename = "expectedIndexTree")]
    expected_index_tree: Option<String>,
    /// The immutable HEAD object shown alongside the staged-index review.
    /// This must be a full object ID so a stale commit form cannot resolve a
    /// moving ref at execution time.
    #[serde(rename = "expectedHead")]
    expected_head: Option<String>,
    /// The checked-out branch identity paired with `expected_head`. A full
    /// object ID alone cannot distinguish two local branches that currently
    /// point at the same commit.
    #[serde(rename = "expectedTargetRef")]
    expected_target_ref: Option<String>,
}

impl CommitBody {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug, Deserialize)]
struct FileBody {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    file: String,
}

impl FileBody {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug, Deserialize)]
struct HunkBody {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    file: String,
    operation: String,
    #[serde(rename = "hunkIndexes")]
    hunk_indexes: Vec<usize>,
}

impl HunkBody {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug, Deserialize)]
struct ResolveConflictBody {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    file: String,
    resolution: String,
    content: Option<String>,
    #[serde(rename = "stage", default = "default_true")]
    stage: bool,
}

impl ResolveConflictBody {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct GenerateCommitMessageBody {
    project: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(rename = "repositoryId")]
    repository_id: Option<String>,
    files: Vec<String>,
}

impl GenerateCommitMessageBody {
    fn project_ref(&self) -> Result<&str> {
        self.project
            .as_deref()
            .or(self.project_path.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Project name is required"))
    }
}

#[derive(Debug)]
struct GitOutput {
    stdout: String,
    // Keep the original bytes for porcelain output. Git paths are allowed to
    // contain arbitrary bytes, so status parsing must not depend on lossy
    // UTF-8 conversion.
    stdout_bytes: Vec<u8>,
    stderr: String,
}

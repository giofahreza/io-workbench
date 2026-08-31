use std::{
    collections::HashSet,
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
    GitBranchesResponse, GitCommitSummary, GitCommitsResponse, GitConflictFileResponse,
    GitConflictRegion, GitConflictSummary, GitConflictsResponse, GitDiffResponse, GitFileStatus,
    GitFileWithDiffResponse, GitGenerateMessageResponse, GitOperationResponse,
    GitRemoteStatusResponse, GitStashSummary, GitStashesResponse, GitStatusResponse, GitTagSummary,
    GitTagsResponse, GitRepositoryKind,
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
        .route("/api/git/revert-local-commit", post(revert_local_commit))
        .route("/api/git/branches", get(branches))
        .route("/api/git/checkout", post(checkout))
        .route("/api/git/create-branch", post(create_branch))
        .route("/api/git/delete-branch", post(delete_branch))
        .route("/api/git/commits", get(commits))
        .route("/api/git/commit-diff", get(commit_diff))
        .route("/api/git/stashes", get(stashes))
        .route("/api/git/stash", post(create_stash))
        .route("/api/git/stash/apply", post(apply_stash))
        .route("/api/git/stash/pop", post(pop_stash))
        .route("/api/git/stash/drop", post(drop_stash))
        .route("/api/git/tags", get(tags))
        .route("/api/git/tag", post(create_tag))
        .route("/api/git/tag/delete", post(delete_tag))
        .route("/api/git/tag/push", post(push_tag))
        .route(
            "/api/git/generate-commit-message",
            post(generate_commit_message),
        )
        .route("/api/git/remote-status", get(remote_status))
        .route("/api/git/remote", post(set_remote))
        .route("/api/git/fetch", post(fetch))
        .route("/api/git/pull", post(pull))
        .route("/api/git/push", post(push))
        .route("/api/git/publish", post(publish))
        .route("/api/git/stage", post(stage))
        .route("/api/git/unstage", post(unstage))
        .route("/api/git/apply-hunks", post(apply_hunks))
        .route("/api/git/resolve-conflict", post(resolve_conflict))
        .route("/api/git/discard", post(discard))
        .route("/api/git/delete-untracked", post(delete_untracked))
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

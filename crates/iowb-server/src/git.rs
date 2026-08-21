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
    GitTagsResponse,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::{fs, io::AsyncWriteExt, process::Command};

use crate::{AuthenticatedUser, Result, ServerError};

const COMMIT_DIFF_CHARACTER_LIMIT: usize = 500_000;

pub fn router() -> Router<AppState> {
    Router::new()
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
    stderr: String,
}

async fn status(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitStatusResponse>> {
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let branch = current_branch(&project_path).await.ok();
    let has_commits = repository_has_commits(&project_path).await?;
    let output = git(
        &project_path,
        ["status", "--porcelain", "--untracked-files=all"],
    )
    .await?;

    let mut modified = Vec::new();
    let mut added = Vec::new();
    let mut deleted = Vec::new();
    let mut untracked = Vec::new();
    let mut conflicted = Vec::new();
    let mut files = Vec::new();

    for (status, path) in parse_status_entries(&output.stdout) {
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
        files.push(GitFileStatus { path, status });
    }

    Ok(Json(GitStatusResponse {
        branch,
        has_commits,
        modified,
        added,
        deleted,
        untracked,
        conflicted,
        clean: files.is_empty(),
        files,
        raw: output.stdout,
    }))
}

async fn conflicts(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitConflictsResponse>> {
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let repository_root = repository_root(&project_path).await?;
    let output = git(
        &repository_root,
        ["status", "--porcelain", "--untracked-files=all"],
    )
    .await?;
    let mut files = Vec::new();

    for (status, path) in parse_status_entries(&output.stdout)
        .into_iter()
        .filter(|(status, _)| is_conflict_status(status))
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
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_repository_file_path(&project_path, query.file_ref()?).await?;
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

    if validate_git_repository(&project_path).await.is_ok() {
        return Ok(Json(GitOperationResponse::success(
            "Repository is already initialized",
        )));
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
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_repository_file_path(&project_path, query.file_ref()?).await?;
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

    let diff = if status.starts_with("??") {
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
    } else if status.trim_start().starts_with('D') {
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
        let unstaged = git(
            &resolved.repository_root,
            ["diff", "--", &resolved.repository_relative_file],
        )
        .await?
        .stdout;
        if unstaged.trim().is_empty() {
            strip_diff_headers(
                &git(
                    &resolved.repository_root,
                    ["diff", "--cached", "--", &resolved.repository_relative_file],
                )
                .await?
                .stdout,
            )
        } else {
            strip_diff_headers(&unstaged)
        }
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
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let resolved = resolve_repository_file_path(&project_path, query.file_ref()?).await?;
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
    let is_untracked = status.starts_with("??");
    let is_deleted = status.trim_start().starts_with('D');

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
    }))
}

async fn initial_commit(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    if repository_has_commits(&project_path).await? {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Repository already has commits. Use regular commit instead.",
        ));
    }

    git(&project_path, ["add", "."]).await?;
    let output = git(&project_path, ["commit", "-m", "Initial commit"]).await?;
    let mut response = GitOperationResponse::success(join_output(&output));
    response.message = Some("Initial commit created successfully".to_string());
    Ok(Json(response))
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
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let repository_root = repository_root(&project_path).await?;

    for file in &body.files {
        let resolved = resolve_repository_file_path(&project_path, file).await?;
        git(
            &repository_root,
            ["add", "--", &resolved.repository_relative_file],
        )
        .await?;
    }

    let output = git(&repository_root, ["commit", "-m", body.message.trim()]).await?;
    Ok(Json(GitOperationResponse::success(join_output(&output))))
}

async fn revert_local_commit(
    State(state): State<AppState>,
    Json(body): Json<ProjectBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    if !repository_has_commits(&project_path).await? {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No local commit to revert",
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
        "Latest local commit reverted successfully. Changes were kept staged.",
    )))
}

async fn branches(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitBranchesResponse>> {
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let output = git(&project_path, ["branch", "-a"]).await?.stdout;
    let mut local_branches = Vec::new();
    let mut remote_branches = Vec::new();

    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if line.contains("->") {
            continue;
        }
        let clean = line.strip_prefix("* ").unwrap_or(line).trim();
        if clean.starts_with("remotes/") {
            if let Some((_, branch)) = clean
                .strip_prefix("remotes/")
                .and_then(|value| value.split_once('/'))
            {
                if !local_branches.iter().any(|local| local == branch) {
                    push_unique(&mut remote_branches, branch.to_string());
                }
            }
        } else {
            push_unique(&mut local_branches, clean.to_string());
        }
    }

    let mut all = local_branches.clone();
    for branch in &remote_branches {
        push_unique(&mut all, branch.clone());
    }

    Ok(Json(GitBranchesResponse {
        branches: all,
        local_branches,
        remote_branches,
    }))
}

async fn checkout(
    State(state): State<AppState>,
    Json(body): Json<BranchBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_branch_name(&body.branch)?;
    let output = git(&project_path, ["checkout", body.branch.trim()]).await?;
    Ok(Json(GitOperationResponse::success(join_output(&output))))
}

async fn create_branch(
    State(state): State<AppState>,
    Json(body): Json<BranchBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_branch_name(&body.branch)?;
    let output = git(&project_path, ["checkout", "-b", body.branch.trim()]).await?;
    Ok(Json(GitOperationResponse::success(join_output(&output))))
}

async fn delete_branch(
    State(state): State<AppState>,
    Json(body): Json<BranchBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    validate_branch_name(&body.branch)?;
    let current = current_branch(&project_path).await?;
    if current == body.branch.trim() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Cannot delete the currently checked-out branch",
        ));
    }

    let output = git(&project_path, ["branch", "-d", body.branch.trim()]).await?;
    Ok(Json(GitOperationResponse::success(join_output(&output))))
}

async fn commits(
    State(state): State<AppState>,
    Query(query): Query<CommitsQuery>,
) -> Result<Json<GitCommitsResponse>> {
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    if !repository_has_commits(&project_path).await? {
        return Ok(Json(GitCommitsResponse {
            commits: Vec::new(),
        }));
    }

    let limit = query.limit.unwrap_or(10).clamp(1, 100);
    let output = git(
        &project_path,
        [
            "log",
            "--pretty=format:%H|%an|%ae|%ad|%s",
            "--date=iso-strict",
            "-n",
            &limit.to_string(),
        ],
    )
    .await?
    .stdout;
    let mut commits = Vec::new();

    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let mut parts = line.split('|');
        let Some(hash) = parts.next() else { continue };
        let Some(author) = parts.next() else { continue };
        let Some(email) = parts.next() else { continue };
        let Some(date) = parts.next() else { continue };
        let message = parts.collect::<Vec<_>>().join("|");
        let stats = git(&project_path, ["show", "--stat", "--format=", hash])
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
        commits.push(GitCommitSummary {
            hash: hash.to_string(),
            author: author.to_string(),
            email: email.to_string(),
            date: date.to_string(),
            message,
            stats,
        });
    }

    Ok(Json(GitCommitsResponse { commits }))
}

async fn commit_diff(
    State(state): State<AppState>,
    Query(query): Query<CommitDiffQuery>,
) -> Result<Json<GitDiffResponse>> {
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_commit_ref(&query.commit)?;
    let output = git(&project_path, ["show", query.commit.trim()])
        .await?
        .stdout;
    let is_truncated = output.len() > COMMIT_DIFF_CHARACTER_LIMIT;
    let diff = if is_truncated {
        format!(
            "{}\n\n... Diff truncated to keep the UI responsive ...",
            output
                .chars()
                .take(COMMIT_DIFF_CHARACTER_LIMIT)
                .collect::<String>()
        )
    } else {
        output
    };

    Ok(Json(GitDiffResponse { diff, is_truncated }))
}

async fn stashes(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitStashesResponse>> {
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let output = git(
        &project_path,
        ["stash", "list", "--format=%gd%x1f%H%x1f%an%x1f%aI%x1f%s"],
    )
    .await?
    .stdout;
    let stashes = output
        .lines()
        .filter_map(parse_stash_summary)
        .collect::<Vec<_>>();
    Ok(Json(GitStashesResponse { stashes }))
}

async fn create_stash(
    State(state): State<AppState>,
    Json(body): Json<StashBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let message = validate_optional_git_message(body.message.as_deref())?;
    let output = if let Some(message) = message {
        git(&project_path, ["stash", "push", "-u", "-m", &message]).await?
    } else {
        git(&project_path, ["stash", "push", "-u"]).await?
    };
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Stash saved successfully",
    ))))
}

async fn apply_stash(
    State(state): State<AppState>,
    Json(body): Json<StashRefBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let reference = validate_stash_ref(&body.reference)?;
    let output = git(&project_path, ["stash", "apply", &reference]).await?;
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Stash applied successfully",
    ))))
}

async fn pop_stash(
    State(state): State<AppState>,
    Json(body): Json<StashRefBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let reference = validate_stash_ref(&body.reference)?;
    let output = git(&project_path, ["stash", "pop", &reference]).await?;
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Stash popped successfully",
    ))))
}

async fn drop_stash(
    State(state): State<AppState>,
    Json(body): Json<StashRefBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let reference = validate_stash_ref(&body.reference)?;
    let output = git(&project_path, ["stash", "drop", &reference]).await?;
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Stash dropped successfully",
    ))))
}

async fn tags(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<GitTagsResponse>> {
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let output = git(
        &project_path,
        [
            "for-each-ref",
            "refs/tags",
            "--sort=-creatordate",
            "--format=%(refname:short)%1f%(objectname)%1f%(objecttype)%1f%(creatordate:iso-strict)%1f%(subject)",
        ],
    )
    .await?
    .stdout;
    let tags = output
        .lines()
        .filter_map(parse_tag_summary)
        .collect::<Vec<_>>();
    Ok(Json(GitTagsResponse { tags }))
}

async fn create_tag(
    State(state): State<AppState>,
    Json(body): Json<TagBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let tag = validate_tag_name(&body.tag)?;
    let message = validate_optional_git_message(body.message.as_deref())?;
    let output = if let Some(message) = message {
        git(&project_path, ["tag", "-a", &tag, "-m", &message]).await?
    } else {
        git(&project_path, ["tag", &tag]).await?
    };
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Tag created successfully",
    ))))
}

async fn delete_tag(
    State(state): State<AppState>,
    Json(body): Json<TagBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let tag = validate_tag_name(&body.tag)?;
    let output = git(&project_path, ["tag", "-d", &tag]).await?;
    Ok(Json(GitOperationResponse::success(join_output_or(
        &output,
        "Tag deleted successfully",
    ))))
}

async fn push_tag(
    State(state): State<AppState>,
    Json(body): Json<TagBody>,
) -> Result<Json<GitOperationResponse>> {
    let project_path = resolve_project_path(&state, body.project_ref()?).await?;
    validate_git_repository(&project_path).await?;
    let tag = validate_tag_name(&body.tag)?;
    let remote_name = first_remote(&project_path).await?.ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            "No remote repository configured. Add a remote first.",
        )
    })?;
    validate_remote_name(&remote_name)?;
    let output = git(&project_path, ["push", &remote_name, &tag]).await?;
    let mut response =
        GitOperationResponse::success(join_output_or(&output, "Tag pushed successfully"));
    response.remote_name = Some(remote_name);
    Ok(Json(response))
}

fn parse_stash_summary(line: &str) -> Option<GitStashSummary> {
    let mut parts = line.splitn(5, '\u{1f}');
    Some(GitStashSummary {
        reference: parts.next()?.to_string(),
        hash: parts.next()?.to_string(),
        author: parts.next()?.to_string(),
        date: parts.next()?.to_string(),
        message: parts.next()?.to_string(),
    })
}

fn parse_tag_summary(line: &str) -> Option<GitTagSummary> {
    let mut parts = line.splitn(5, '\u{1f}');
    Some(GitTagSummary {
        name: parts.next()?.to_string(),
        hash: parts.next()?.to_string(),
        object_type: parts.next()?.to_string(),
        date: parts.next()?.to_string(),
        message: parts.next()?.to_string(),
    })
}

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

fn validate_hunk_operation(operation: &str) -> Result<&str> {
    match operation.trim() {
        "stage" => Ok("stage"),
        "unstage" => Ok("unstage"),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid hunk operation",
        )),
    }
}

fn validate_conflict_resolution(resolution: &str) -> Result<&str> {
    match resolution.trim() {
        "ours" => Ok("ours"),
        "theirs" => Ok("theirs"),
        "manual" => Ok("manual"),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid conflict resolution",
        )),
    }
}

fn validate_pattern(
    value: &str,
    message: &'static str,
    allowed: impl Fn(char) -> bool,
) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || !trimmed.chars().all(allowed) {
        return Err(ServerError::new(StatusCode::BAD_REQUEST, message));
    }
    Ok(trimmed.to_string())
}

fn is_unknown_initial_branch(details: &str) -> bool {
    let details = details.to_lowercase();
    details.contains("unknown option") || details.contains("unrecognized option")
}

fn is_missing_head_revision(details: &str) -> bool {
    let details = details.to_lowercase();
    details.contains("unknown revision")
        || details.contains("ambiguous argument")
        || details.contains("needed a single revision")
        || details.contains("bad revision")
}

fn is_missing_head_parent(details: &str) -> bool {
    is_missing_head_revision(details) && details.to_lowercase().contains("head~1")
}

async fn commit_message_diff_context(
    project_path: &Path,
    repository_root: &Path,
    files: &[String],
) -> Result<String> {
    let mut context = String::new();
    for file in files {
        let resolved = match resolve_repository_file_path(project_path, file).await {
            Ok(resolved) => resolved,
            Err(error) => {
                warn_git_context(&mut context, file, &error.body.error);
                continue;
            }
        };

        if let Ok(output) = git(
            repository_root,
            ["diff", "HEAD", "--", &resolved.repository_relative_file],
        )
        .await
        {
            if !output.stdout.trim().is_empty() {
                push_diff_section(
                    &mut context,
                    &resolved.repository_relative_file,
                    output.stdout.trim(),
                );
                continue;
            }
        }

        let absolute_target =
            match safe_repo_child(repository_root, &resolved.repository_relative_file) {
                Ok(path) => path,
                Err(error) => {
                    warn_git_context(&mut context, file, &error.body.error);
                    continue;
                }
            };
        match fs::metadata(&absolute_target).await {
            Ok(metadata) if metadata.is_dir() => {
                context.push_str(&format!(
                    "\n--- {} (new directory) ---\n",
                    resolved.repository_relative_file
                ));
            }
            Ok(_) => match fs::read_to_string(&absolute_target).await {
                Ok(content) => {
                    let excerpt = content.chars().take(1000).collect::<String>();
                    context.push_str(&format!(
                        "\n--- {} (new file) ---\n{}\n",
                        resolved.repository_relative_file, excerpt
                    ));
                }
                Err(_) => {
                    context.push_str(&format!(
                        "\n--- {} (binary or unreadable file) ---\n",
                        resolved.repository_relative_file
                    ));
                }
            },
            Err(_) => warn_git_context(&mut context, file, "file not found"),
        }
    }
    Ok(context)
}

fn push_diff_section(context: &mut String, file: &str, diff: &str) {
    context.push_str(&format!("\n--- {file} ---\n{diff}\n"));
}

fn warn_git_context(context: &mut String, file: &str, message: &str) {
    context.push_str(&format!("\n--- {file} (skipped: {message}) ---\n"));
}

#[derive(Debug, Clone)]
struct DirectAiConfig {
    mode: String,
    base_url: Option<String>,
    api_key_env: Option<String>,
    model: Option<String>,
}

impl DirectAiConfig {
    fn from_value(value: Option<Value>) -> Self {
        let value = value.unwrap_or(Value::Null);
        Self {
            mode: value
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("off")
                .to_string(),
            base_url: value
                .get("baseUrl")
                .or_else(|| value.get("base_url"))
                .and_then(Value::as_str)
                .map(str::to_string),
            api_key_env: value
                .get("apiKeyEnv")
                .or_else(|| value.get("api_key_env"))
                .and_then(Value::as_str)
                .map(str::to_string),
            model: value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }

    fn is_enabled(&self) -> bool {
        !matches!(self.mode.as_str(), "off" | "")
    }

    fn base_url(&self) -> Result<String> {
        if let Some(base_url) = self
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(base_url.trim_end_matches('/').to_string());
        }

        match self.mode.as_str() {
            "direct" | "anthropic" => Ok("https://api.anthropic.com".to_string()),
            "minimax" => Ok("https://api.minimax.io/anthropic".to_string()),
            "proxy" | "aiproxy" => Ok("http://141.144.197.96:8319/claude".to_string()),
            _ => Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Direct AI baseUrl is required",
            )),
        }
    }

    fn api_key(&self) -> Option<String> {
        let configured_key = self
            .api_key_env
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|key| std::env::var(key).ok());
        configured_key
            .or_else(|| match self.mode.as_str() {
                "direct" | "anthropic" => std::env::var("ANTHROPIC_API_KEY")
                    .or_else(|_| std::env::var("ANTHROPIC_AUTH_TOKEN"))
                    .ok(),
                "minimax" => std::env::var("MINIMAX_API_KEY")
                    .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
                    .ok(),
                _ => std::env::var("CODEX_GATEWAY_KEY").ok(),
            })
            .filter(|value| !value.trim().is_empty())
    }

    fn model(&self) -> String {
        self.model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("claude-haiku-4-5-20251001")
            .to_string()
    }
}

async fn generate_commit_message_with_ai(
    files: &[String],
    diff_context: &str,
    config: Option<Value>,
) -> Result<String> {
    let config = DirectAiConfig::from_value(config);
    if !config.is_enabled() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Direct AI is off",
        ));
    }

    let prompt = commit_message_prompt(files, diff_context);
    let raw = call_direct_ai(&config, &prompt, 512).await?;
    Ok(clean_commit_message(&raw))
}

fn commit_message_prompt(files: &[String], diff_context: &str) -> String {
    let files = files
        .iter()
        .map(|file| format!("- {file}"))
        .collect::<Vec<_>>()
        .join("\n");
    let diff_context = diff_context.chars().take(6000).collect::<String>();
    format!(
        "Generate a conventional commit message for these changes.\n\n\
REQUIREMENTS:\n\
- Format: type(scope): subject\n\
- Include body explaining what changed and why\n\
- Types: feat, fix, docs, style, refactor, perf, test, build, ci, chore\n\
- Subject under 50 chars, body wrapped at 72 chars\n\
- Focus on user-facing changes, not implementation details\n\
- Return ONLY the commit message (no markdown, explanations, or code blocks)\n\n\
FILES CHANGED:\n{files}\n\n\
DIFFS:\n{diff_context}\n\n\
Commit message:"
    )
}

async fn call_direct_ai(config: &DirectAiConfig, prompt: &str, max_tokens: u64) -> Result<String> {
    let api_key = config.api_key().ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            "Direct AI API key is not available in the server environment",
        )
    })?;
    let base_url = config.base_url()?;
    let model = config.model();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create Direct AI client",
                error.to_string(),
            )
        })?;

    let messages_body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let response = post_direct_ai_json(
        &client,
        &format!("{base_url}/v1/messages"),
        &api_key,
        &messages_body,
    )
    .await?;

    let value = if response.status().is_success() {
        response
            .json::<Value>()
            .await
            .map_err(direct_ai_json_error)?
    } else if matches!(response.status().as_u16(), 400 | 404 | 405)
        && matches!(config.mode.as_str(), "proxy" | "aiproxy")
    {
        let chat_body = serde_json::json!({
            "model": config.model(),
            "max_tokens": max_tokens,
            "messages": [{ "role": "user", "content": prompt }],
        });
        let chat_response = post_direct_ai_json(
            &client,
            &format!("{base_url}/v1/chat/completions"),
            &api_key,
            &chat_body,
        )
        .await?;
        if !chat_response.status().is_success() {
            return Err(direct_ai_http_error(chat_response).await);
        }
        chat_response
            .json::<Value>()
            .await
            .map_err(direct_ai_json_error)?
    } else {
        return Err(direct_ai_http_error(response).await);
    };

    Ok(extract_response_text(&value))
}

async fn post_direct_ai_json(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &Value,
) -> std::result::Result<reqwest::Response, ServerError> {
    client
        .post(url)
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .bearer_auth(api_key)
        .header("x-api-key", api_key)
        .json(body)
        .send()
        .await
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::BAD_GATEWAY,
                "Direct AI request failed",
                error.to_string(),
            )
        })
}

async fn direct_ai_http_error(response: reqwest::Response) -> ServerError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    ServerError::with_details(
        StatusCode::BAD_GATEWAY,
        format!("Direct AI HTTP {status}"),
        body.chars().take(300).collect::<String>(),
    )
}

fn direct_ai_json_error(error: reqwest::Error) -> ServerError {
    ServerError::with_details(
        StatusCode::BAD_GATEWAY,
        "Direct AI returned invalid JSON",
        error.to_string(),
    )
}

fn extract_response_text(value: &Value) -> String {
    collect_text(value.get("content"))
        .or_else(|| {
            value
                .get("choices")
                .and_then(Value::as_array)
                .map(|choices| {
                    choices
                        .iter()
                        .filter_map(|choice| {
                            collect_text(
                                choice
                                    .get("message")
                                    .and_then(|message| message.get("content")),
                            )
                            .or_else(|| {
                                collect_text(
                                    choice.get("delta").and_then(|delta| delta.get("content")),
                                )
                            })
                            .or_else(|| {
                                choice
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                        })
                        .collect::<Vec<_>>()
                        .join("")
                })
        })
        .or_else(|| {
            value
                .get("output_text")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn collect_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .or_else(|| item.get("text").and_then(Value::as_str).map(str::to_string))
                        .or_else(|| collect_text(item.get("content")))
                        .or_else(|| {
                            item.get("output_text")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                })
                .collect::<Vec<_>>()
                .join("");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn clean_commit_message(text: &str) -> String {
    let mut cleaned = text.trim().replace("```text", "").replace("```", "");
    while cleaned.starts_with('#') {
        cleaned = cleaned.trim_start_matches('#').trim_start().to_string();
    }
    cleaned = cleaned.trim_matches(['"', '\'']).to_string();
    while cleaned.contains("\n\n\n") {
        cleaned = cleaned.replace("\n\n\n", "\n\n");
    }
    if let Some(index) = conventional_commit_index(&cleaned) {
        cleaned = cleaned[index..].to_string();
    }
    cleaned.trim().to_string()
}

fn conventional_commit_index(text: &str) -> Option<usize> {
    const TYPES: &[&str] = &[
        "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore",
    ];
    TYPES
        .iter()
        .filter_map(|kind| {
            text.find(&format!("{kind}:"))
                .or_else(|| text.find(&format!("{kind}(")))
        })
        .min()
}

fn fallback_commit_message(files: &[String]) -> String {
    let kind = if files.len() == 1 { "file" } else { "files" };
    format!("chore: update {} {kind}", files.len())
}

fn join_output(output: &GitOutput) -> String {
    format!("{}\n{}", output.stdout, output.stderr)
        .trim()
        .to_string()
}

fn join_output_or(output: &GitOutput, fallback: &str) -> String {
    let joined = join_output(output);
    if joined.is_empty() {
        fallback.to_string()
    } else {
        joined
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn io_server_error(error: std::io::Error) -> ServerError {
    if error.kind() == std::io::ErrorKind::NotFound {
        ServerError::new(StatusCode::NOT_FOUND, "path not found")
    } else {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "io error",
            error.to_string(),
        )
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn pathdiff(path: &Path, base: &Path) -> Option<PathBuf> {
    path.strip_prefix(base).ok().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_commit_message_from_markdown() {
        let raw = "Here is a message:\n```text\nfix(auth): handle login\n\nKeep token checks strict.\n```";

        assert_eq!(
            clean_commit_message(raw),
            "fix(auth): handle login\n\nKeep token checks strict."
        );
    }

    #[test]
    fn extracts_anthropic_response_text() {
        let value = serde_json::json!({
            "content": [
                { "type": "text", "text": "feat(ui): add shell" }
            ]
        });

        assert_eq!(extract_response_text(&value), "feat(ui): add shell");
    }

    #[test]
    fn extracts_chat_completion_text() {
        let value = serde_json::json!({
            "choices": [
                { "message": { "content": "chore: update files" } }
            ]
        });

        assert_eq!(extract_response_text(&value), "chore: update files");
    }

    #[test]
    fn selected_hunk_patch_keeps_file_headers_and_requested_hunks() {
        let diff = "diff --git a/file.txt b/file.txt\nindex 111..222 100644\n--- a/file.txt\n+++ b/file.txt\n@@ -1,2 +1,2 @@\n old\n+new\n@@ -8,2 +8,2 @@\n old2\n+new2\n";

        let patch = selected_hunk_patch(diff, &[1]).expect("patch can be selected");

        assert!(patch.contains("diff --git a/file.txt b/file.txt"));
        assert!(!patch.contains("@@ -1,2 +1,2 @@"));
        assert!(patch.contains("@@ -8,2 +8,2 @@"));
        assert!(patch.contains("+new2"));
    }

    #[test]
    fn detects_unmerged_git_statuses() {
        for status in ["UU", "AA", "DD", "AU", "UD", "UA", "DU", " U", "U "] {
            assert!(is_conflict_status(status), "{status} should be conflicted");
        }

        for status in [" M", "M ", "A ", " D", "??"] {
            assert!(
                !is_conflict_status(status),
                "{status} should not be conflicted"
            );
        }
    }

    #[test]
    fn extracts_conflict_regions_with_base_sections() {
        let content =
            "keep\n<<<<<<< HEAD\nours\n||||||| base\nbase\n=======\ntheirs\n>>>>>>> branch\nkeep\n";

        let regions = extract_conflict_regions(content);

        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].start_line, 2);
        assert_eq!(regions[0].end_line, 8);
        assert_eq!(regions[0].ours, "ours");
        assert_eq!(regions[0].base.as_deref(), Some("base"));
        assert_eq!(regions[0].theirs, "theirs");
    }

    #[test]
    fn parses_stash_and_tag_rows_without_losing_message_text() {
        let stash =
            parse_stash_summary("stash@{0}\u{1f}abc123\u{1f}Gio\u{1f}2026-07-30T12:00:00+07:00\u{1f}WIP: keep | separators")
                .expect("stash row");
        assert_eq!(stash.reference, "stash@{0}");
        assert_eq!(stash.message, "WIP: keep | separators");

        let tag = parse_tag_summary(
            "v1.0.0\u{1f}def456\u{1f}tag\u{1f}2026-07-30T12:00:00+07:00\u{1f}Release 1.0",
        )
        .expect("tag row");
        assert_eq!(tag.name, "v1.0.0");
        assert_eq!(tag.object_type, "tag");
    }
}

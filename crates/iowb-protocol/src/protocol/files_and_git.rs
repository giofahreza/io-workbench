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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatusResponse {
    pub branch: Option<String>,
    #[serde(rename = "hasCommits")]
    pub has_commits: bool,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileStatus {
    pub path: String,
    pub status: String,
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
    pub ahead: u32,
    pub behind: u32,
    #[serde(rename = "isUpToDate")]
    pub is_up_to_date: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use chrono::{DateTime, Utc};
use iowb_protocol::{FileContentResponse, FileEntry, FileKind, WorkspaceValidation};
use thiserror::Error;
use tokio::fs;

#[derive(Debug, Error)]
pub enum FsError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    InvalidPath(String),
    #[error("path is outside the allowed root")]
    OutsideRoot,
    #[error("binary file reading is not supported by this endpoint")]
    BinaryFile,
}

pub type Result<T> = std::result::Result<T, FsError>;

#[derive(Debug, Clone)]
pub struct WorkspacePathValidator {
    workspace_root: PathBuf,
}

impl WorkspacePathValidator {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: expand_tilde(workspace_root.into()),
        }
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub async fn validate(
        &self,
        requested_path: impl AsRef<Path>,
        allow_workspace_root: bool,
    ) -> WorkspaceValidation {
        match self
            .validate_path(requested_path.as_ref(), allow_workspace_root)
            .await
        {
            Ok(path) => WorkspaceValidation {
                valid: true,
                resolved_path: Some(path.display().to_string()),
                error: None,
            },
            Err(error) => WorkspaceValidation {
                valid: false,
                resolved_path: None,
                error: Some(error.to_string()),
            },
        }
    }

    pub async fn validate_path(
        &self,
        requested_path: impl AsRef<Path>,
        allow_workspace_root: bool,
    ) -> Result<PathBuf> {
        let expanded = expand_tilde(requested_path.as_ref().to_path_buf());
        let absolute = absolutize(&expanded)?;
        let workspace_root = self.resolved_workspace_root().await?;
        let normalized = normalize_path(&absolute);
        let normalized_root = normalize_path(&workspace_root);

        if allow_workspace_root && normalized == normalized_root {
            return Ok(workspace_root);
        }

        if is_forbidden_path(&normalized, &normalized_root) {
            return Err(FsError::InvalidPath(
                "cannot use system-critical directories as workspace locations".to_string(),
            ));
        }

        let real_or_future = resolve_existing_or_parent(&absolute).await?;
        if !is_path_within(&normalized_root, &normalize_path(&real_or_future)) {
            return Err(FsError::InvalidPath(format!(
                "workspace path must be within the allowed workspace root: {}",
                self.workspace_root.display()
            )));
        }

        if let Ok(metadata) = fs::symlink_metadata(&absolute).await {
            if metadata.file_type().is_symlink() {
                let target = fs::read_link(&absolute).await?;
                let resolved_target = if target.is_absolute() {
                    target
                } else {
                    absolute
                        .parent()
                        .map(|parent| parent.join(&target))
                        .unwrap_or(target)
                };
                let target_real = fs::canonicalize(resolved_target).await?;
                if !is_path_within(&normalized_root, &normalize_path(&target_real)) {
                    return Err(FsError::InvalidPath(
                        "symlink target is outside the allowed workspace root".to_string(),
                    ));
                }
            }
        }

        Ok(real_or_future)
    }

    async fn resolved_workspace_root(&self) -> Result<PathBuf> {
        match fs::canonicalize(&self.workspace_root).await {
            Ok(path) => Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(absolutize(&self.workspace_root)?)
            }
            Err(error) => Err(error.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileService {
    max_scan_depth: usize,
    max_file_read_bytes: u64,
}

impl Default for FileService {
    fn default() -> Self {
        Self {
            max_scan_depth: 6,
            max_file_read_bytes: 2 * 1024 * 1024,
        }
    }
}

impl FileService {
    pub fn new(max_scan_depth: usize, max_file_read_bytes: u64) -> Self {
        Self {
            max_scan_depth,
            max_file_read_bytes,
        }
    }

    pub async fn browse_directories(&self, path: impl AsRef<Path>) -> Result<Vec<FileEntry>> {
        let root = absolutize(&expand_tilde(path.as_ref().to_path_buf()))?;
        let mut entries = Vec::new();
        let mut reader = fs::read_dir(&root).await?;

        while let Some(entry) = reader.next_entry().await? {
            let metadata = entry.metadata().await?;
            if !metadata.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if should_skip_name(&name) {
                continue;
            }
            entries.push(file_entry_from_metadata(
                &root,
                &entry.path(),
                &metadata,
                true,
            ));
        }

        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(entries)
    }

    pub async fn list_tree(
        &self,
        project_root: impl AsRef<Path>,
        requested_path: impl AsRef<Path>,
    ) -> Result<Vec<FileEntry>> {
        self.list_tree_with_depth(project_root, requested_path, self.max_scan_depth)
            .await
    }

    pub async fn list_tree_with_depth(
        &self,
        project_root: impl AsRef<Path>,
        requested_path: impl AsRef<Path>,
        max_depth: usize,
    ) -> Result<Vec<FileEntry>> {
        let root = fs::canonicalize(project_root.as_ref()).await?;
        let target = resolve_child_path(&root, requested_path.as_ref()).await?;
        let mut entries = Vec::new();
        self.read_dir_recursive(
            &root,
            &target,
            0,
            max_depth.min(self.max_scan_depth),
            &mut entries,
        )
        .await?;
        Ok(entries)
    }

    pub async fn read_file(
        &self,
        project_root: impl AsRef<Path>,
        file_path: impl AsRef<Path>,
    ) -> Result<FileContentResponse> {
        let root = fs::canonicalize(project_root.as_ref()).await?;
        let target = resolve_child_path(&root, file_path.as_ref()).await?;
        let metadata = fs::metadata(&target).await?;
        if !metadata.is_file() {
            return Err(FsError::InvalidPath("path is not a file".to_string()));
        }
        if metadata.len() > self.max_file_read_bytes {
            return Err(FsError::InvalidPath(format!(
                "file exceeds read limit of {} bytes",
                self.max_file_read_bytes
            )));
        }

        let bytes = fs::read(&target).await?;
        if bytes.contains(&0) {
            return Err(FsError::BinaryFile);
        }
        let content = String::from_utf8(bytes).map_err(|_| FsError::BinaryFile)?;

        Ok(FileContentResponse {
            path: display_path(&root, &target),
            content,
            size: metadata.len(),
            modified: metadata.modified().ok().map(DateTime::<Utc>::from),
        })
    }

    pub async fn write_file(
        &self,
        project_root: impl AsRef<Path>,
        file_path: impl AsRef<Path>,
        content: &str,
    ) -> Result<FileContentResponse> {
        let root = fs::canonicalize(project_root.as_ref()).await?;
        let target = resolve_child_path_for_create(&root, file_path.as_ref())?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&target, content).await?;
        self.read_file(root, target).await
    }

    pub async fn write_bytes(
        &self,
        project_root: impl AsRef<Path>,
        file_path: impl AsRef<Path>,
        bytes: &[u8],
    ) -> Result<FileEntry> {
        let root = fs::canonicalize(project_root.as_ref()).await?;
        let target = resolve_child_path_for_create(&root, file_path.as_ref())?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&target, bytes).await?;
        let metadata = fs::metadata(&target).await?;
        Ok(file_entry_from_metadata(&root, &target, &metadata, false))
    }

    pub async fn create_path(
        &self,
        project_root: impl AsRef<Path>,
        file_path: impl AsRef<Path>,
        content: &str,
        directory: bool,
    ) -> Result<FileEntry> {
        let root = fs::canonicalize(project_root.as_ref()).await?;
        let target = resolve_child_path_for_create(&root, file_path.as_ref())?;

        if directory {
            fs::create_dir_all(&target).await?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(&target, content).await?;
        }

        let metadata = fs::metadata(&target).await?;
        Ok(file_entry_from_metadata(&root, &target, &metadata, false))
    }

    pub async fn rename_path(
        &self,
        project_root: impl AsRef<Path>,
        old_path: impl AsRef<Path>,
        new_path: impl AsRef<Path>,
    ) -> Result<FileEntry> {
        let root = fs::canonicalize(project_root.as_ref()).await?;
        let old_target = resolve_child_path(&root, old_path.as_ref()).await?;
        let new_target = resolve_child_path_for_create(&root, new_path.as_ref())?;
        if let Some(parent) = new_target.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::rename(&old_target, &new_target).await?;
        let metadata = fs::metadata(&new_target).await?;
        Ok(file_entry_from_metadata(
            &root,
            &new_target,
            &metadata,
            false,
        ))
    }

    pub async fn copy_path(
        &self,
        project_root: impl AsRef<Path>,
        source_path: impl AsRef<Path>,
        target_path: impl AsRef<Path>,
    ) -> Result<FileEntry> {
        let root = fs::canonicalize(project_root.as_ref()).await?;
        let source = resolve_child_path(&root, source_path.as_ref()).await?;
        let target = resolve_child_path_for_create(&root, target_path.as_ref())?;
        let metadata = fs::metadata(&source).await?;

        if metadata.is_dir() {
            if target.starts_with(&source) {
                return Err(FsError::InvalidPath(
                    "cannot copy a directory into itself".to_string(),
                ));
            }
            let source_clone = source.clone();
            let target_clone = target.clone();
            tokio::task::spawn_blocking(move || copy_dir_recursive(&source_clone, &target_clone))
                .await
                .map_err(|error| FsError::InvalidPath(error.to_string()))??;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::copy(&source, &target).await?;
        }

        let copied_metadata = fs::metadata(&target).await?;
        Ok(file_entry_from_metadata(
            &root,
            &target,
            &copied_metadata,
            false,
        ))
    }

    pub async fn delete_path(
        &self,
        project_root: impl AsRef<Path>,
        file_path: impl AsRef<Path>,
    ) -> Result<()> {
        let root = fs::canonicalize(project_root.as_ref()).await?;
        let target = resolve_child_path(&root, file_path.as_ref()).await?;
        let metadata = fs::metadata(&target).await?;
        if metadata.is_dir() {
            fs::remove_dir_all(target).await?;
        } else {
            fs::remove_file(target).await?;
        }
        Ok(())
    }

    async fn read_dir_recursive(
        &self,
        root: &Path,
        path: &Path,
        depth: usize,
        max_depth: usize,
        output: &mut Vec<FileEntry>,
    ) -> Result<()> {
        let mut reader = fs::read_dir(path).await?;
        let mut entries = Vec::new();

        while let Some(entry) = reader.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if should_skip_name(&name) {
                continue;
            }

            let metadata = entry.metadata().await?;
            let is_dir = metadata.is_dir();
            let entry_path = entry.path();
            let mut file_entry = file_entry_from_metadata(root, &entry_path, &metadata, false);

            if is_dir && depth < max_depth {
                let mut children = Vec::new();
                Box::pin(self.read_dir_recursive(
                    root,
                    &entry_path,
                    depth + 1,
                    max_depth,
                    &mut children,
                ))
                .await?;
                file_entry.children = children;
            }

            entries.push(file_entry);
        }

        entries.sort_by(|a, b| {
            let left_dir = a.kind == FileKind::Directory;
            let right_dir = b.kind == FileKind::Directory;
            right_dir
                .cmp(&left_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        output.extend(entries);
        Ok(())
    }
}

fn copy_dir_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else {
            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&source_path, &target_path)?;
        }
    }
    Ok(())
}

fn file_entry_from_metadata(
    root: &Path,
    path: &Path,
    metadata: &std::fs::Metadata,
    absolute_path: bool,
) -> FileEntry {
    let kind = if metadata.is_dir() {
        FileKind::Directory
    } else {
        FileKind::File
    };

    FileEntry {
        name: path
            .file_name()
            .unwrap_or_else(|| OsStr::new(""))
            .to_string_lossy()
            .into_owned(),
        path: if absolute_path {
            path.display().to_string()
        } else {
            display_path(root, path)
        },
        kind,
        size: metadata.len(),
        modified: metadata.modified().ok().map(DateTime::<Utc>::from),
        children: Vec::new(),
    }
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .trim_start_matches(std::path::MAIN_SEPARATOR)
        .to_string()
}

async fn resolve_child_path(root: &Path, requested: &Path) -> Result<PathBuf> {
    let target = if requested.as_os_str().is_empty() || requested == Path::new(".") {
        root.to_path_buf()
    } else if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };

    let canonical = fs::canonicalize(&target).await?;
    if !is_path_within(root, &canonical) {
        return Err(FsError::OutsideRoot);
    }
    Ok(canonical)
}

fn resolve_child_path_for_create(root: &Path, requested: &Path) -> Result<PathBuf> {
    if requested
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(FsError::OutsideRoot);
    }

    let target = if requested.as_os_str().is_empty() || requested == Path::new(".") {
        return Err(FsError::InvalidPath("file path is required".to_string()));
    } else if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };

    let normalized = normalize_path(&target);
    if !is_path_within(root, &normalized) {
        return Err(FsError::OutsideRoot);
    }
    Ok(normalized)
}

fn expand_tilde(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return dirs::home_dir().unwrap_or(path);
    }
    if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    path
}

fn absolutize(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(normalize_path(path));
    }
    Ok(normalize_path(&std::env::current_dir()?.join(path)))
}

async fn resolve_existing_or_parent(path: &Path) -> Result<PathBuf> {
    match fs::canonicalize(path).await {
        Ok(real) => Ok(real),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| FsError::InvalidPath("path has no parent".to_string()))?;
            match fs::canonicalize(parent).await {
                Ok(parent_real) => Ok(parent_real
                    .join(path.file_name().ok_or_else(|| {
                        FsError::InvalidPath("path has no file name".to_string())
                    })?)),
                Err(parent_error) if parent_error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(normalize_path(path))
                }
                Err(parent_error) => Err(parent_error.into()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn is_path_within(root: &Path, candidate: &Path) -> bool {
    candidate == root || candidate.starts_with(root)
}

fn is_forbidden_path(path: &Path, workspace_root: &Path) -> bool {
    let forbidden = [
        "/",
        "/etc",
        "/bin",
        "/sbin",
        "/usr",
        "/dev",
        "/proc",
        "/sys",
        "/var",
        "/boot",
        "/root",
        "/lib",
        "/lib64",
        "/opt",
        "/tmp",
        "/run",
        "C:\\Windows",
        "C:\\Program Files",
        "C:\\Program Files (x86)",
        "C:\\ProgramData",
        "C:\\System Volume Information",
        "C:\\$Recycle.Bin",
    ];

    forbidden.iter().any(|raw| {
        let forbidden_path = Path::new(raw);
        if path != forbidden_path && !path.starts_with(forbidden_path) {
            return false;
        }

        if forbidden_path == Path::new("/var")
            && (path.starts_with("/var/tmp") || path.starts_with("/var/folders"))
        {
            return false;
        }

        let root_inside_forbidden = is_path_within(forbidden_path, workspace_root);
        let path_inside_root = is_path_within(workspace_root, path);
        !(root_inside_forbidden && path_inside_root)
    })
}

fn should_skip_name(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "target" | "dist" | "build" | ".DS_Store"
    ) || name.ends_with(".tmp")
        || name.ends_with(".swp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_parent_components() {
        assert_eq!(
            normalize_path(Path::new("/tmp/example/../project")),
            PathBuf::from("/tmp/project")
        );
    }

    #[test]
    fn rejects_forbidden_root_outside_workspace() {
        assert!(is_forbidden_path(
            Path::new("/etc"),
            Path::new("/home/user")
        ));
    }

    #[tokio::test]
    async fn limits_file_tree_depth_per_request() {
        let root = std::env::temp_dir().join(format!(
            "iowb-fs-depth-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        ));
        tokio::fs::create_dir_all(root.join("src/nested"))
            .await
            .unwrap();
        tokio::fs::write(root.join("src/main.rs"), "fn main() {}")
            .await
            .unwrap();
        tokio::fs::write(root.join("src/nested/deep.rs"), "fn deep() {}")
            .await
            .unwrap();

        let service = FileService::new(6, 1024 * 1024);
        let shallow = service.list_tree_with_depth(&root, ".", 0).await.unwrap();
        let src = shallow.iter().find(|entry| entry.name == "src").unwrap();
        assert!(src.children.is_empty());

        let one_level = service.list_tree_with_depth(&root, ".", 1).await.unwrap();
        let src = one_level.iter().find(|entry| entry.name == "src").unwrap();
        assert!(src.children.iter().any(|entry| entry.name == "main.rs"));
        let nested = src
            .children
            .iter()
            .find(|entry| entry.name == "nested")
            .unwrap();
        assert!(nested.children.is_empty());

        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}

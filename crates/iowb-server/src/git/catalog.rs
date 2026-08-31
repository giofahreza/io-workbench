// Repository discovery and selection for a project workspace.
//
// A project is a container. Git commands always target one concrete worktree
// selected from this catalog; the container itself is never treated as a
// repository when it is not one.

#[derive(Debug, Clone)]
struct GitRepositoryRecord {
    id: String,
    name: String,
    path: std::path::PathBuf,
    relative_path: String,
    kind: iowb_protocol::GitRepositoryKind,
    initialized: bool,
    branch: Option<String>,
}

async fn initialize_uninitialized_submodule(
    catalog: &GitWorkspaceCatalog,
    repository: &GitRepositoryRecord,
) -> crate::Result<GitOutput> {
    if !matches!(
        repository.kind,
        iowb_protocol::GitRepositoryKind::Uninitialized
    ) || repository.initialized
    {
        return Err(crate::ServerError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "The selected Git entry is not an uninitialized submodule",
        ));
    }

    // The submodule checkout has no .git marker yet, so the command must run
    // in the nearest initialized repository that owns its gitlink.  Choosing
    // the deepest ancestor also handles submodules nested inside initialized
    // submodules without ever handing a workspace path to Git.
    let parent = catalog
        .repositories
        .iter()
        .filter(|candidate| {
            candidate.initialized
                && candidate.path != repository.path
                && is_within(&repository.path, &candidate.path)
        })
        .max_by_key(|candidate| candidate.path.components().count())
        .ok_or_else(|| {
            crate::ServerError::new(
                axum::http::StatusCode::BAD_REQUEST,
                format!(
                    "Cannot initialize submodule {} because its parent repository is unavailable",
                    repository.relative_path
                ),
            )
        })?;
    let relative_path = repository
        .path
        .strip_prefix(&parent.path)
        .ok()
        .map(|path| normalize_repo_relative_path(&path.to_string_lossy()))
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            crate::ServerError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "Invalid submodule path",
            )
        })?;
    let declared_paths = read_submodule_paths(&parent.path).await;
    let gitlink_paths = read_gitlink_paths(&parent.path).await;
    let declared_path = safe_repo_child(&parent.path, &relative_path)?;
    if !declared_paths
        .iter()
        .chain(gitlink_paths.iter())
        .any(|path| normalize_path(path) == normalize_path(&declared_path))
    {
        return Err(crate::ServerError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "The selected path is not a submodule of its parent repository",
        ));
    }

    git(
        &parent.path,
        ["submodule", "update", "--init", "--", relative_path.as_str()],
    )
    .await
}

#[derive(Debug)]
struct GitWorkspaceCatalog {
    workspace_path: std::path::PathBuf,
    repositories: Vec<GitRepositoryRecord>,
    default_repository_id: Option<String>,
}

#[axum::debug_handler]
async fn git_workspace(
    axum::extract::State(state): axum::extract::State<iowb_core::AppState>,
    query: axum::extract::Query<ProjectQuery>,
) -> crate::Result<axum::Json<iowb_protocol::GitWorkspaceResponse>> {
    let project_path = resolve_project_path(&state, query.project_ref()?).await?;
    let catalog = discover_git_workspace(&project_path).await?;
    Ok(axum::Json(git_workspace_response(&catalog)))
}

fn git_workspace_response(catalog: &GitWorkspaceCatalog) -> iowb_protocol::GitWorkspaceResponse {
    iowb_protocol::GitWorkspaceResponse {
        project_path: catalog.workspace_path.display().to_string(),
        has_root_repository: catalog
            .repositories
            .iter()
            .any(|repository| matches!(repository.kind, iowb_protocol::GitRepositoryKind::Root)),
        default_repository_id: catalog.default_repository_id.clone(),
        repositories: catalog
            .repositories
            .iter()
            .map(|repository| iowb_protocol::GitRepositorySummary {
                id: repository.id.clone(),
                name: repository.name.clone(),
                path: repository.path.display().to_string(),
                relative_path: repository.relative_path.clone(),
                kind: repository.kind.clone(),
                initialized: repository.initialized,
                is_default: catalog
                    .default_repository_id
                    .as_deref()
                    .is_some_and(|id| id == repository.id),
                branch: repository.branch.clone(),
            })
            .collect(),
    }
}

async fn discover_git_workspace(
    workspace_path: &std::path::Path,
) -> crate::Result<GitWorkspaceCatalog> {
    let workspace_path = tokio::fs::canonicalize(workspace_path)
        .await
        .map_err(io_server_error)?;
    // `git rev-parse` also succeeds when the requested directory is merely a
    // child of a repository.  That repository is outside this project scope
    // and must not be exposed or mutated through this workspace.
    let root_path = git(&workspace_path, ["rev-parse", "--show-toplevel"])
        .await
        .ok()
        .and_then(|output| {
            let value = output.stdout.trim();
            (!value.is_empty()).then(|| std::path::PathBuf::from(value))
        })
        .map(|path| normalize_path(&path))
        .filter(|path| is_within(path, &workspace_path));

    let mut known = std::collections::BTreeMap::<std::path::PathBuf, GitRepositoryRecord>::new();
    let mut repository_paths = Vec::new();
    if let Some(root) = root_path.as_ref() {
        repository_paths.push(root.clone());
    }

    let candidates = walkdir::WalkDir::new(&workspace_path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_discovery_excluded(entry.path()))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_dir() && has_git_marker(entry.path()))
        .map(|entry| entry.path().to_path_buf())
        .collect::<Vec<_>>();
    for candidate_path in candidates {
        let candidate = match tokio::fs::canonicalize(candidate_path).await {
            Ok(path) => normalize_path(path.as_path()),
            Err(_) => continue,
        };
        if !is_within(&candidate, &workspace_path) {
            continue;
        }
        if root_path.as_ref().is_some_and(|root| root == &candidate) {
            continue;
        }
        let Ok(repository_root_output) = git(&candidate, ["rev-parse", "--show-toplevel"]).await
        else {
            continue;
        };
        let Some(repository_root) = non_empty_path(repository_root_output.stdout.trim()) else {
            continue;
        };
        let repository_root = normalize_path(std::path::Path::new(repository_root));
        if repository_root != candidate || !is_within(&repository_root, &workspace_path) {
            continue;
        }
        if !repository_paths.contains(&repository_root) {
            repository_paths.push(repository_root);
        }
    }

    // Collect all repository metadata before classifying any child.  This is
    // important for nested submodules: a child can be encountered by the
    // filesystem walk before its initialized parent has contributed the
    // child's gitlink to the catalog.
    let mut submodule_paths = std::collections::HashSet::new();
    for repository_path in &repository_paths {
        submodule_paths.extend(read_submodule_paths(repository_path).await);
        submodule_paths.extend(read_gitlink_paths(repository_path).await);
    }

    for repository_path in repository_paths {
        let is_linked_worktree = is_worktree_marker(&repository_path);
        let is_root = root_path
            .as_ref()
            .is_some_and(|root| root == &repository_path)
            && !is_linked_worktree;
        let kind = if is_root {
            iowb_protocol::GitRepositoryKind::Root
        } else if is_linked_worktree {
            iowb_protocol::GitRepositoryKind::Worktree
        } else if submodule_paths.contains(&repository_path) {
            iowb_protocol::GitRepositoryKind::Submodule
        } else {
            iowb_protocol::GitRepositoryKind::Nested
        };
        add_repository_record(
            &mut known,
            &repository_path,
            &workspace_path,
            kind,
            true,
            is_root,
        )
        .await?;
    }

    // .gitmodules can name a submodule whose checkout has not been initialized.
    // Keep it visible so the mobile client can explain why it cannot be opened.
    for submodule_path in submodule_paths {
        if known.contains_key(&submodule_path) || !is_within(&submodule_path, &workspace_path) {
            continue;
        }
        add_repository_record(
            &mut known,
            &submodule_path,
            &workspace_path,
            iowb_protocol::GitRepositoryKind::Uninitialized,
            false,
            false,
        )
        .await?;
    }

    let mut repositories = known.into_values().collect::<Vec<_>>();
    repositories.sort_by_key(|repository| {
        (
            !matches!(repository.kind, iowb_protocol::GitRepositoryKind::Root),
            repository.relative_path.clone(),
        )
    });
    let default_repository_id = repositories
        .iter()
        .find(|repository| matches!(repository.kind, iowb_protocol::GitRepositoryKind::Root))
        .map(|repository| repository.id.clone())
        .or_else(|| {
            let initialized = repositories.iter().filter(|repository| repository.initialized);
            let mut only = initialized.take(2);
            let first = only.next()?;
            only.next().is_none().then(|| first.id.clone())
        });

    Ok(GitWorkspaceCatalog {
        workspace_path,
        repositories,
        default_repository_id,
    })
}

async fn resolve_git_repository_path(
    state: &iowb_core::AppState,
    project_ref: &str,
    repository_id: Option<&str>,
) -> crate::Result<std::path::PathBuf> {
    Ok(selected_git_repository(state, project_ref, repository_id)
        .await?
        .1
        .path)
}

async fn selected_git_repository(
    state: &iowb_core::AppState,
    project_ref: &str,
    repository_id: Option<&str>,
) -> crate::Result<(GitWorkspaceCatalog, GitRepositoryRecord)> {
    let project_path = resolve_project_path(state, project_ref).await?;
    let catalog = discover_git_workspace(&project_path).await?;
    let selected_id = repository_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .or(catalog.default_repository_id.as_deref())
        .ok_or_else(|| {
            let message = if catalog.repositories.is_empty() {
                "No Git repositories were found in this workspace. Initialize a repository explicitly before using Git."
            } else {
                "This workspace contains multiple repositories and no safe default. Select a repository first."
            };
            crate::ServerError::new(
                axum::http::StatusCode::BAD_REQUEST,
                message,
            )
        })?;
    let repository = catalog
        .repositories
        .iter()
        .find(|repository| repository.id == selected_id)
        .cloned()
        .ok_or_else(|| {
            crate::ServerError::new(
                axum::http::StatusCode::NOT_FOUND,
                "Git repository was not found in this workspace",
            )
        })?;
    if !repository.initialized {
        return Err(crate::ServerError::new(
            axum::http::StatusCode::BAD_REQUEST,
            format!("Repository {} is not initialized", repository.relative_path),
        ));
    }
    Ok((catalog, repository))
}

async fn add_repository_record(
    known: &mut std::collections::BTreeMap<std::path::PathBuf, GitRepositoryRecord>,
    repository_path: &std::path::Path,
    workspace_path: &std::path::Path,
    kind: iowb_protocol::GitRepositoryKind,
    initialized: bool,
    is_root: bool,
) -> crate::Result<()> {
    let path = normalize_path(repository_path);
    if !is_within(&path, workspace_path) {
        return Ok(());
    }
    if known.contains_key(&path) {
        return Ok(());
    }
    let relative_path = relative_repository_path(&path, workspace_path);
    // The ID is derived only from the worktree's path.  Classification can
    // change (for example, when a nested checkout becomes a gitlink), but a
    // caller holding a repository selection must continue to address the same
    // worktree after that refresh.
    let id = if is_root {
        "root".to_string()
    } else {
        format!("repository:{relative_path}")
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Repository")
        .to_string();
    let branch = if initialized {
        current_branch(&path).await.ok()
    } else {
        None
    };
    known.insert(
        path.clone(),
        GitRepositoryRecord {
            id,
            name,
            path,
            relative_path,
            kind,
            initialized,
            branch,
        },
    );
    Ok(())
}

async fn read_submodule_paths(repository_path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(output) = git(
        repository_path,
        ["config", "--file", ".gitmodules", "--get-regexp", "^submodule\\..*\\.path$"],
    )
    .await
    else {
        return Vec::new();
    };
    let repository_path = normalize_path(repository_path);
    let mut paths = Vec::new();
    for path in output.stdout.lines().filter_map(|line| {
        line.split_once(char::is_whitespace)
            .map(|(_, path)| path.trim())
    }) {
        if path.is_empty() || path.contains('\0') {
            continue;
        }
        // .gitmodules is untrusted repository data. Absolute paths, parent
        // segments, and paths that resolve through an escaping symlink must
        // never become repository records or mutation targets.
        let normalized = path.replace('\\', "/");
        if std::path::Path::new(path).is_absolute()
            || normalized.split('/').any(|component| component == "..")
        {
            continue;
        }
        let Ok(candidate) = safe_repo_child(&repository_path, &normalized) else {
            continue;
        };
        if candidate == repository_path {
            continue;
        }
        let checked = match tokio::fs::canonicalize(&candidate).await {
            Ok(path) if is_within(&path, &repository_path) => normalize_path(&path),
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => candidate,
            Err(_) => continue,
        };
        if is_within(&checked, &repository_path) && !paths.contains(&checked) {
            paths.push(checked);
        }
    }
    paths
}

/// Return paths that Git records as mode 160000 entries.  `.gitmodules` is
/// useful metadata, but it is not the authoritative index: a repository can
/// contain a staged or committed gitlink while the file is absent, stale, or
/// intentionally ignored.  Treating the gitlink as a repository boundary is
/// what prevents parent operations from traversing into that checkout.
async fn read_gitlink_paths(repository_path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(output) = git(repository_path, ["ls-files", "--stage", "-z"]).await else {
        return Vec::new();
    };
    let repository_path = normalize_path(repository_path);
    let mut paths = Vec::new();
    for record in output.stdout_bytes.split(|byte| *byte == 0) {
        let Some(separator) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        let (metadata, path) = record.split_at(separator);
        let path = &path[1..];
        let Some(mode) = metadata.split(|byte| *byte == b' ').next() else {
            continue;
        };
        if mode != b"160000" {
            continue;
        }
        let path = String::from_utf8_lossy(path).to_string();
        if path.is_empty() {
            continue;
        }
        let Ok(candidate) = safe_repo_child(&repository_path, &path) else {
            continue;
        };
        let checked = match tokio::fs::canonicalize(&candidate).await {
            Ok(path) if is_within(&path, &repository_path) => normalize_path(&path),
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => candidate,
            Err(_) => continue,
        };
        if is_within(&checked, &repository_path) && !paths.contains(&checked) {
            paths.push(checked);
        }
    }
    paths
}

fn has_git_marker(path: &std::path::Path) -> bool {
    let marker = path.join(".git");
    marker.is_dir() || marker.is_file()
}

fn is_worktree_marker(path: &std::path::Path) -> bool {
    let marker = path.join(".git");
    if !marker.is_file() {
        return false;
    }
    let Ok(content) = std::fs::read_to_string(marker) else {
        return false;
    };
    let Some(line) = content.lines().next().map(str::trim) else {
        return false;
    };
    let Some(gitdir) = line.strip_prefix("gitdir:").map(str::trim) else {
        return false;
    };
    gitdir.contains("/worktrees/") || gitdir.contains("\\worktrees\\")
}

fn is_discovery_excluded(path: &std::path::Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name,
        ".git" | "target" | "node_modules" | "build" | ".gradle" | ".idea" | ".venv" | "__pycache__"
    )
}

fn is_within(path: &std::path::Path, parent: &std::path::Path) -> bool {
    path == parent || path.strip_prefix(parent).is_ok()
}

fn relative_repository_path(
    repository_path: &std::path::Path,
    workspace_path: &std::path::Path,
) -> String {
    repository_path
        .strip_prefix(workspace_path)
        .ok()
        .map(|path| normalize_repo_relative_path(&path.to_string_lossy()))
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| ".".to_string())
}

fn non_empty_path(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

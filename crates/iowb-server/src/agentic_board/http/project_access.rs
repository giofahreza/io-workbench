/// An unavailable workspace must not look like a valid project with no board.
/// This guard is intentionally only for project-scoped reads. The unfiltered
/// history list and stored snapshots remain available without changing records.
async fn validate_board_read_project_access(state: &AppState, project_path: &str) -> Result<()> {
    let resolved = state
        .path_validator
        .validate_path(Path::new(project_path), false)
        .await
        .map_err(|error| match error {
            FsError::Io(error) => board_read_project_io_error(project_path, error),
            error => error.into(),
        })?;
    let metadata = tokio::fs::metadata(&resolved)
        .await
        .map_err(|error| board_read_project_io_error(project_path, error))?;
    if !metadata.is_dir() {
        return Err(board_read_project_io_error(
            project_path,
            std::io::Error::from(std::io::ErrorKind::NotADirectory),
        ));
    }
    // read_dir checks read access; resolving '.' also checks directory traversal
    // access on Unix, which metadata(path) alone does not require.
    tokio::fs::metadata(resolved.join("."))
        .await
        .map_err(|error| board_read_project_io_error(project_path, error))?;
    let _readable_directory = tokio::fs::read_dir(&resolved)
        .await
        .map_err(|error| board_read_project_io_error(project_path, error))?;
    Ok(())
}

fn board_read_project_io_error(project_path: &str, error: std::io::Error) -> ServerError {
    let (status, problem, next_step) = match error.kind() {
        std::io::ErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            "does not exist",
            "Restore the directory or open the project at its new location.",
        ),
        std::io::ErrorKind::PermissionDenied => (
            StatusCode::FORBIDDEN,
            "cannot be accessed because permission was denied",
            "Restore directory permissions for the Workbench server or open an accessible project.",
        ),
        std::io::ErrorKind::NotADirectory => (
            StatusCode::BAD_REQUEST,
            "is not a directory",
            "Open the project's directory instead of a file.",
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot currently be accessed",
            "Check the project's filesystem or mount, then retry.",
        ),
    };
    ServerError::new(
        status,
        format!(
            "Project directory '{project_path}' {problem}. {next_step} Saved Kanban history is preserved."
        ),
    )
}

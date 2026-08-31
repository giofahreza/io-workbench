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
    if trimmed.is_empty()
        || trimmed.starts_with('-')
        || trimmed.len() > 1024
        || trimmed
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || !trimmed.chars().all(allowed)
    {
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
        let resolved = match resolve_git_file_target(project_path, file, GitFileTargetPolicy::Inspect).await {
            Ok(resolved) => resolved,
            Err(error) => {
                warn_git_context(&mut context, file, &error.body.error);
                continue;
            }
        };

        if let Ok(output) = git(
            repository_root,
            [
                "diff",
                "HEAD",
                "--submodule=log",
                "--",
                &resolved.repository_relative_file,
            ],
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

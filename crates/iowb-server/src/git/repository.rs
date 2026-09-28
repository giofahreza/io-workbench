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
            let root = repository_root(project_path).await?;
            if normalize_path(&root) != normalize_path(project_path) {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Project path is inside another Git repository. Select the repository root or initialize this workspace explicitly.",
                ));
            }
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
    fs::canonicalize(root).await.map_err(io_server_error)
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

async fn resolve_commit_reference(repository_path: &Path, value: &str) -> Result<String> {
    let reference = validate_commit_ref(value)?;
    // A lightweight tag already names a commit, but an annotated tag names a
    // tag object which then points at its commit.  Git's ordinary ref-aware
    // workflows (and LazyGit's tag actions) accept both forms
    // transparently.  Ask Git to peel only as far as a commit so tags work
    // while trees, blobs, and other object types remain invalid inputs.
    let commit_reference = format!("{reference}^{{commit}}");
    let output = git(
        repository_path,
        [
            "rev-parse",
            "--verify",
            "--end-of-options",
            commit_reference.as_str(),
        ],
    )
    .await?;
    let hash = output
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "Commit reference did not resolve to an object",
            )
        })?;
    let object_type = git(repository_path, ["cat-file", "-t", hash])
        .await?
        .stdout;
    if object_type.trim() != "commit" {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Commit reference must resolve to a commit",
        ));
    }
    Ok(hash.to_string())
}

async fn resolve_repository_file_path(
    project_path: &Path,
    file_path: &str,
) -> Result<ResolvedRepositoryFile> {
    validate_file_path(file_path)?;
    let repository_root = repository_root(project_path).await?;
    let candidates = file_path_candidates(project_path, &repository_root, file_path);

    for candidate in &candidates {
        if repository_file_status(&repository_root, candidate)
            .await?
            .is_some()
        {
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
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
            "--",
            repository_relative_file,
        ],
    )
    .await?
    ;

    let entries = parse_status_entries_detailed_bytes(&status_output.stdout_bytes);
    if entries.is_empty() {
        return Ok(false);
    }

    let has_tracked = entries.iter().any(|(status, _, _)| status != "??");

    if has_tracked {
        // Match LazyGit's "Discard unstaged changes" operation: restore the
        // worktree from the index and leave the index untouched.  Resetting
        // first would silently turn an `MM`/`AM` path into ` M`/`??`, losing
        // the user's staged version even though the UI only promised to
        // discard working-tree changes.
        git(
            repository_root,
            ["restore", "--worktree", "--", repository_relative_file],
        )
        .await?;
        return Ok(true);
    }

    // The native UI sends untracked paths to the separately named delete
    // route. Retain the historical direct-route behavior for callers that
    // still use discard on an untracked path, but never combine it with a
    // tracked restore above: a mixed directory must not lose staged content.
    let absolute_target = safe_repo_child(repository_root, repository_relative_file)?;
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

    Ok(true)
}

/// Reset one path back to `HEAD`, including its index entry. This is the
/// deliberately destructive counterpart to `discard_repository_path`, which
/// only restores the working tree from the index. Keeping the operations
/// separate mirrors LazyGit's explicit "Discard all changes" versus
/// "Discard unstaged changes" choices.
async fn discard_all_repository_path(
    repository_root: &Path,
    repository_relative_file: &str,
) -> Result<bool> {
    let status_output = git(
        repository_root,
        [
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
            "--",
            repository_relative_file,
        ],
    )
    .await?;
    let entries = parse_status_entries_detailed_bytes(&status_output.stdout_bytes);
    if entries.is_empty() {
        return Ok(false);
    }

    let has_tracked = entries.iter().any(|(status, _, _)| status != "??");
    let has_commits = repository_has_commits(repository_root).await?;
    // A staged addition has no HEAD blob to restore after we remove it from
    // the index. Avoid asking `git restore --source=HEAD` for a path that is
    // intentionally absent from HEAD; deletion below is its full reset.
    let has_head_backed_entry = entries
        .iter()
        .any(|(status, _, _)| status != "??" && !status.starts_with('A'));

    if has_tracked {
        if has_commits {
            git(
                repository_root,
                ["reset", "HEAD", "--", repository_relative_file],
            )
            .await?;
        } else {
            git(
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
            .await?;
        }
    }

    let absolute_target = safe_repo_child(repository_root, repository_relative_file)?;
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

    if has_commits && has_head_backed_entry {
        git(
            repository_root,
            [
                "restore",
                "--source=HEAD",
                "--worktree",
                "--",
                repository_relative_file,
            ],
        )
        .await?;
    }

    Ok(true)
}

async fn upstream_remote_or_origin(project_path: &Path) -> Result<String> {
    let branch = current_branch(project_path).await?;
    Ok(upstream_remote_branch_or(project_path, &branch).await?.0)
}

/// Git permits a local branch to track another local branch by configuring
/// `branch.<name>.remote = .`. `@{upstream}` then renders only the source
/// branch name (rather than `./source`), so it cannot be resolved by the
/// normal remote-tracking-name parser below. LazyGit delegates this case to
/// `git pull . refs/heads/<source>`; retain that useful native Git workflow
/// instead of silently falling back to `origin`.
async fn configured_local_tracking_branch(
    project_path: &Path,
    branch: &str,
) -> Result<Option<String>> {
    let branch = validate_branch_name(branch)?;
    let remote_key = format!("branch.{branch}.remote");
    let Some(remote) = advanced_local_config_value(project_path, &remote_key).await? else {
        return Ok(None);
    };
    if remote.trim() != "." {
        return Ok(None);
    }

    let merge_key = format!("branch.{branch}.merge");
    let merge = advanced_local_config_value(project_path, &merge_key)
        .await?
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "Local tracking for branch {branch} is incomplete. Configure {merge_key} as refs/heads/<branch>."
                ),
            )
        })?;
    let source = merge.trim().strip_prefix("refs/heads/").ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "Local tracking for branch {branch} must name a local branch as refs/heads/<branch>."
            ),
        )
    })?;
    Ok(Some(validate_branch_name(source)?))
}

async fn configured_remote_names(project_path: &Path) -> Result<Vec<String>> {
    let output = git(project_path, ["remote"]).await?.stdout;
    output
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(validate_remote_name)
        .collect()
}

/// Split a remote-tracking display name using the longest configured remote
/// prefix. A plain `split_once('/')` mistakes `team/review/feature` for the
/// remote `team` and branch `review/feature`, even when the actual remote is
/// named `team/review`.
fn split_configured_remote_tracking_name(
    tracking: &str,
    remotes: &[String],
) -> Option<(String, String)> {
    remotes
        .iter()
        .filter_map(|remote| {
            tracking
                .strip_prefix(remote)
                .and_then(|suffix| suffix.strip_prefix('/'))
                .filter(|branch| !branch.is_empty())
                .map(|branch| (remote.clone(), branch.to_string()))
        })
        .max_by_key(|(remote, _)| remote.len())
}

/// Resolve the current branch's configured pull upstream without inventing a
/// remote.  LazyGit's Pull flow prompts for an upstream when the branch is
/// not tracking one; callers that intentionally retain the historical
/// `origin/<branch>` fallback (such as the legacy Fetch route) use
/// `upstream_remote_branch_or` below instead.
async fn configured_upstream_remote_branch(
    project_path: &Path,
    branch: &str,
) -> Result<Option<(String, String)>> {
    if let Some(local_branch) = configured_local_tracking_branch(project_path, branch).await? {
        return Ok(Some((".".to_string(), local_branch)));
    }
    if let Ok(output) = git(
        project_path,
        [
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .await
    {
        let tracking = output.stdout.trim();
        let remotes = configured_remote_names(project_path).await?;
        if let Some((remote, branch)) = split_configured_remote_tracking_name(tracking, &remotes) {
            return Ok(Some((remote, branch)));
        }
    }

    Ok(None)
}

/// Compatibility resolution for callers which historically use
/// `origin/<current-branch>` when no pull upstream exists.  Do not use this
/// for Pull itself: unlike Fetch, a pull integrates history and must make a
/// missing upstream explicit, matching LazyGit's upstream chooser.
async fn upstream_remote_branch_or(project_path: &Path, branch: &str) -> Result<(String, String)> {
    Ok(configured_upstream_remote_branch(project_path, branch)
        .await?
        .unwrap_or_else(|| ("origin".to_string(), branch.to_string())))
}

/// A single, explicit destination for the checked-out branch's ordinary
/// `git push`.  Pull/upstream state and push state are deliberately separate:
/// teams commonly fetch from `upstream` while publishing a review branch to a
/// fork or release remote.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GitPushTarget {
    remote_name: String,
    remote_branch: String,
    /// `push.autoSetupRemote=true` makes a plain Git push establish this
    /// branch's tracking upstream.  Keep that semantic separate from an
    /// explicitly selected Publish action so native callers can show the
    /// resulting configuration change before sending the push.
    set_upstream: bool,
}

/// Git's `matching` mode can update several refs and `nothing` deliberately
/// updates none.  The mobile UI operates on one named branch at a time, so it
/// only receives [GitPushTarget] when Git's configuration identifies one safe
/// branch destination.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GitPushTargetResolution {
    target: Option<GitPushTarget>,
    push_mode: String,
    message: Option<String>,
}

fn no_git_push_target(push_mode: String, message: impl Into<String>) -> GitPushTargetResolution {
    GitPushTargetResolution {
        target: None,
        push_mode,
        message: Some(message.into()),
    }
}

fn require_configured_git_push_target(
    resolution: GitPushTargetResolution,
) -> Result<GitPushTarget> {
    let GitPushTargetResolution {
        target, message, ..
    } = resolution;
    target.ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            message.unwrap_or_else(|| {
                "No configured single-branch push target is available. Choose a remote branch explicitly."
                    .to_string()
            }),
        )
    })
}

async fn configured_network_tracking_target(
    project_path: &Path,
    branch: &str,
) -> Result<Option<GitPushTarget>> {
    let branch = validate_branch_name(branch)?;
    let remote_key = format!("branch.{branch}.remote");
    let Some(remote) = advanced_effective_config_value(project_path, &remote_key).await? else {
        return Ok(None);
    };
    let remote = remote.trim();
    // `.` is Git's useful local pull-upstream spelling, but is never a
    // configured network push destination.
    if remote == "." {
        return Ok(None);
    }
    let remote = validate_remote_name(remote)?;
    let merge_key = format!("branch.{branch}.merge");
    let Some(merge) = advanced_effective_config_value(project_path, &merge_key).await? else {
        return Ok(None);
    };
    let remote_branch = merge
        .trim()
        .strip_prefix("refs/heads/")
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "The configured upstream for branch {branch} must use refs/heads/<branch>"
                ),
            )
        })?;
    Ok(Some(GitPushTarget {
        remote_name: remote,
        remote_branch: validate_branch_name(remote_branch)?,
        set_upstream: false,
    }))
}

/// Return all effective `remote.<name>.push` refspecs for a selected remote.
/// Git applies these before consulting `push.default`; a blank ordinary push
/// can therefore target a differently named branch even when the default mode
/// is `nothing`.  They are configuration values, not request input, but use
/// NUL output so a malformed embedded newline cannot look like two rules.
async fn configured_remote_push_refspecs(
    project_path: &Path,
    remote_name: &str,
) -> Result<Vec<String>> {
    let remote_name = validate_remote_name(remote_name)?;
    let key = format!("remote.{remote_name}.push");
    match git(project_path, ["config", "--null", "--get-all", key.as_str()]).await {
        Ok(output) => Ok(output
            .stdout_bytes
            .split(|byte| *byte == 0)
            .filter_map(|value| std::str::from_utf8(value).ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect()),
        // `git config --get-all` exits with 1 when a remote has no dedicated
        // push refspec. That is the ordinary case, not a configuration error.
        Err(_) => Ok(Vec::new()),
    }
}

/// Interpret the small, safe subset of a configured remote refspec that the
/// touch client can faithfully show as one current-branch push. LazyGit hands
/// the complete configuration to `git push`; the native UI must not quietly
/// reduce a multi-ref, wildcard, deletion, tag, or force refspec to a
/// different single push.
fn configured_current_branch_push_refspec_target(
    refspecs: &[String],
    branch: &str,
) -> std::result::Result<String, String> {
    if refspecs.len() != 1 {
        return Err(format!(
            "The selected remote has {} configured push refspecs, which can update more than this branch",
            refspecs.len()
        ));
    }
    let refspec = refspecs[0].trim();
    if refspec.starts_with('+') {
        return Err(
            "The selected remote has a force push refspec. Choose Force push explicitly instead of inheriting it silently."
                .to_string(),
        );
    }
    let Some((source, destination)) = refspec.split_once(':') else {
        return Err(
            "The selected remote has a configured push refspec that is not one explicit current-branch destination."
                .to_string(),
        );
    };
    let expected_source = format!("refs/heads/{branch}");
    if source != expected_source && source != "HEAD" {
        return Err(format!(
            "The selected remote's configured push refspec sends {source}, not the checked-out branch {branch}"
        ));
    }
    let Some(destination) = destination.strip_prefix("refs/heads/") else {
        return Err(
            "The selected remote's configured push refspec is not a named remote branch. Choose a remote branch explicitly."
                .to_string(),
        );
    };
    validate_branch_name(destination)
        .map_err(|_| "The selected remote's configured push destination is not a valid branch.".to_string())
}

async fn configured_push_auto_setup_remote(project_path: &Path) -> Result<bool> {
    let Some(value) = advanced_effective_config_value(project_path, "push.autoSetupRemote").await?
    else {
        return Ok(false);
    };
    advanced_parse_git_bool(&value).ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            "push.autoSetupRemote must be a Git boolean",
        )
    })
}

/// Resolve the one branch destination Git would use for an ordinary push,
/// without performing a network call.  This follows Git's remote precedence
/// (`branch.<name>.pushRemote`, `remote.pushDefault`, tracking remote, then
/// `origin`) and its single-branch `push.default` modes.  A caller must ask
/// the user for an explicit target when the saved configuration cannot name
/// one safely.
async fn configured_git_push_target(
    project_path: &Path,
    branch: &str,
) -> Result<GitPushTargetResolution> {
    let branch = validate_branch_name(branch)?;
    let remotes = configured_remote_names(project_path).await?;
    let push_mode = advanced_effective_config_value(project_path, "push.default")
        .await?
        .unwrap_or_else(|| "simple".to_string())
        .trim()
        .to_ascii_lowercase();
    if remotes.is_empty() {
        return Ok(no_git_push_target(
            push_mode,
            "No network remote is configured. Add or choose a remote before pushing.",
        ));
    }

    let configured_tracking = configured_network_tracking_target(project_path, &branch).await?;
    let stale_tracking_remote = configured_tracking
        .as_ref()
        .filter(|target| !remotes.iter().any(|remote| remote == &target.remote_name))
        .map(|target| target.remote_name.clone());
    let tracking = configured_tracking.filter(|target| {
        remotes
            .iter()
            .any(|remote| remote == &target.remote_name)
    });
    let branch_push_remote_key = format!("branch.{branch}.pushRemote");
    let configured_push_remote = match advanced_effective_config_value(project_path, &branch_push_remote_key).await? {
        Some(remote) => Some(remote),
        None => advanced_effective_config_value(project_path, "remote.pushDefault").await?,
    };
    let has_configured_push_remote = configured_push_remote.is_some();
    let remote_name = if let Some(remote) = configured_push_remote.as_deref() {
        let remote = remote.trim();
        if remote == "." {
            return Ok(no_git_push_target(
                push_mode,
                "The configured push remote is local (`.`). Choose a network remote and branch explicitly.",
            ));
        }
        let remote = validate_remote_name(remote)?;
        if !remotes.iter().any(|configured| configured == &remote) {
            return Ok(no_git_push_target(
                push_mode,
                format!(
                    "Configured push remote {remote} is not available. Choose a current remote and branch explicitly."
                ),
            ));
        }
        remote
    } else if stale_tracking_remote.is_some() {
        return Ok(no_git_push_target(
            push_mode,
            format!(
                "The configured tracking remote {} is no longer available. Choose a current remote and branch explicitly.",
                stale_tracking_remote.as_deref().unwrap_or_default(),
            ),
        ));
    } else if let Some(tracking) = &tracking {
        tracking.remote_name.clone()
    } else if remotes.iter().any(|remote| remote == "origin") {
        // This is Git's conventional fallback, but `simple` still refuses to
        // invent an upstream branch below.  Do not choose a merely first
        // remote: multi-remote repositories need an explicit destination.
        "origin".to_string()
    } else {
        return Ok(no_git_push_target(
            push_mode,
            "No push remote is configured. Choose a remote and branch explicitly.",
        ));
    };

    let refspecs = configured_remote_push_refspecs(project_path, &remote_name).await?;
    if !refspecs.is_empty() {
        return match configured_current_branch_push_refspec_target(&refspecs, &branch) {
            Ok(remote_branch) => Ok(GitPushTargetResolution {
                target: Some(GitPushTarget {
                    remote_name,
                    remote_branch,
                    set_upstream: false,
                }),
                push_mode: "remote.<name>.push".to_string(),
                message: None,
            }),
            Err(message) => Ok(no_git_push_target(
                "remote.<name>.push".to_string(),
                message,
            )),
        };
    }

    let auto_setup_remote = configured_push_auto_setup_remote(project_path).await?;
    let target = match push_mode.as_str() {
        "current" => Some(GitPushTarget {
            remote_name,
            remote_branch: branch,
            set_upstream: tracking.is_none() && auto_setup_remote,
        }),
        "upstream" | "tracking" => match tracking {
            Some(target) if target.remote_name == remote_name => Some(target),
            Some(_) => {
                return Ok(no_git_push_target(
                    push_mode,
                    "push.default=upstream can only push to this branch's tracking remote. Choose a target explicitly or change the push configuration.",
                ));
            }
            // Git's push.autoSetupRemote applies to upstream mode as well.
            // A branch.pushRemote or remote.pushDefault still chooses the
            // destination before Git records it as the new tracking upstream.
            // Do not mistake a configured push remote for an existing
            // upstream: this is precisely the new-branch case the setting is
            // meant to handle.
            None if auto_setup_remote => Some(GitPushTarget {
                remote_name,
                remote_branch: branch,
                set_upstream: true,
            }),
            None => return Ok(no_git_push_target(
                push_mode,
                "push.default=upstream requires a network tracking branch. Set an upstream or choose a target explicitly.",
            )),
        },
        "simple" => match tracking {
            Some(target) if target.remote_name == remote_name && target.remote_branch == branch => {
                Some(target)
            }
            Some(target) if target.remote_name == remote_name => {
                return Ok(no_git_push_target(
                    push_mode,
                    format!(
                        "push.default=simple will not choose between {remote_name}/{branch} and the differently named upstream {remote_name}/{}. Choose the remote branch explicitly.",
                        target.remote_branch,
                    ),
                ));
            }
            // An explicit pushRemote or remote.pushDefault deliberately
            // changes the destination remote. Git's simple mode then behaves
            // like current, so report that target instead of pretending the
            // fetch upstream is still where this branch will be published.
            Some(_) => Some(GitPushTarget {
                remote_name,
                remote_branch: branch,
                set_upstream: false,
            }),
            None if has_configured_push_remote => Some(GitPushTarget {
                remote_name,
                remote_branch: branch,
                // Git's push.autoSetupRemote applies to `simple` even when
                // branch.pushRemote or remote.pushDefault selected the
                // destination. Without it, the same ordinary push remains a
                // current-branch push but leaves tracking unset.
                set_upstream: auto_setup_remote,
            }),
            None if auto_setup_remote => Some(GitPushTarget {
                remote_name,
                remote_branch: branch,
                set_upstream: true,
            }),
            None => return Ok(no_git_push_target(
                push_mode,
                "push.default=simple requires an upstream before it can choose a branch. Publish this branch to an explicit remote and branch first.",
            )),
        },
        "matching" => {
            return Ok(no_git_push_target(
                push_mode,
                "push.default=matching can update multiple branches. Choose one remote branch explicitly in this app.",
            ));
        }
        "nothing" => {
            return Ok(no_git_push_target(
                push_mode,
                "push.default=nothing disables automatic push targets. Choose one remote branch explicitly.",
            ));
        }
        _ => {
            return Ok(no_git_push_target(
                push_mode,
                "Git's configured push.default is not a supported single-branch mode. Choose a remote branch explicitly.",
            ));
        }
    };

    Ok(GitPushTargetResolution {
        target,
        push_mode,
        message: None,
    })
}

/// Return a remote only when the repository has exactly one configured
/// destination. Workflows with no remote selector must not pick `origin` (or
/// alphabetical first) when a project has separate upstream and release
/// remotes.
async fn only_configured_remote(project_path: &Path) -> Result<Option<String>> {
    let remotes = git(project_path, ["remote"]).await?.stdout;
    let values = remotes
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    match values.as_slice() {
        [] => Ok(None),
        [remote] => Ok(Some(remote.clone())),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Multiple remotes are configured. Choose an explicit remote before publishing.",
        )),
    }
}

async fn git<I, S>(cwd: &Path, args: I) -> Result<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    run_git(cwd, args, true).await
}

// Some Git workflows invoke an editor while recovering a conflict. The API
// has no interactive terminal, so callers can provide narrowly scoped editor
// settings without changing the server process environment.
async fn git_with_environment<I, S>(
    cwd: &Path,
    args: I,
    environment: &[(&str, &str)],
) -> Result<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    run_git_with_environment(cwd, args, true, environment).await
}

// The initial-commit path uses Git's exclude pathspec magic to keep
// independent nested repositories out of the parent tree.  All request data
// still goes through `git`, which enables literal pathspecs; this helper is
// reserved for the server-owned exclusion pathspecs.
async fn git_with_pathspec_magic<I, S>(cwd: &Path, args: I) -> Result<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    run_git(cwd, args, false).await
}

// Most commands use literal pathspecs so request-provided file names cannot
// gain Git pathspec meaning. `git stash push --include-untracked` is an
// exception: its internal cleanup uses Git's default pathspec behavior, and
// forcing GIT_LITERAL_PATHSPECS makes Git save untracked files without
// removing them from the worktree. Call this only for commands that do not
// forward request values as pathspecs.
async fn git_with_default_pathspecs<I, S>(cwd: &Path, args: I) -> Result<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    run_git(cwd, args, false).await
}

async fn run_git<I, S>(cwd: &Path, args: I, literal_pathspecs: bool) -> Result<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    run_git_with_environment(cwd, args, literal_pathspecs, &[]).await
}

async fn run_git_with_environment<I, S>(
    cwd: &Path,
    args: I,
    literal_pathspecs: bool,
    environment: &[(&str, &str)],
) -> Result<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string())
        .collect::<Vec<_>>();
    let mut command = Command::new("git");
    command
        .args(&args)
        .current_dir(cwd)
        // Server requests must fail promptly when a remote needs credentials;
        // there is no interactive terminal available to answer a prompt.
        .env("GIT_TERMINAL_PROMPT", "0");
    if literal_pathspecs {
        command.env("GIT_LITERAL_PATHSPECS", "1");
    } else {
        command.env_remove("GIT_LITERAL_PATHSPECS");
    }
    for &(key, value) in environment {
        command.env(key, value);
    }
    let output = command.output().await.map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "git failed",
            error.to_string(),
        )
    })?;

    let stdout_bytes = output.stdout;
    let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        return Ok(GitOutput {
            stdout,
            stdout_bytes,
            stderr,
        });
    }

    Err(ServerError::with_details(
        StatusCode::BAD_REQUEST,
        "Git operation failed",
        redact_git_diagnostic(&format!(
            "Command failed: git {}\n{}{}",
            args.join(" "),
            stdout,
            stderr
        )),
    ))
}

fn redact_git_diagnostic(value: &str) -> String {
    let mut redacted = String::with_capacity(value.len());
    let mut cursor = 0;

    while let Some(scheme_offset) = value[cursor..].find("://") {
        let authority_start = cursor + scheme_offset + 3;
        let url_end = value[authority_start..]
            .find(|character: char| {
                character.is_whitespace()
                    || matches!(character, '\'' | '"' | ')' | ']' | '}' | ',')
            })
            .map(|offset| authority_start + offset)
            .unwrap_or(value.len());
        // Query strings and fragments often hold one-time access tokens. They
        // are not useful in a diagnostic, so omit them even if the URL has no
        // user-info component to redact.
        let display_end = value[authority_start..url_end]
            .find(['?', '#'])
            .map(|offset| authority_start + offset)
            .unwrap_or(url_end);
        let authority_end = value[authority_start..display_end]
            .find('/')
            .map(|offset| authority_start + offset)
            .unwrap_or(display_end);
        let authority = &value[authority_start..authority_end];
        redacted.push_str(&value[cursor..authority_start]);
        if let Some(user_info_end) = authority.rfind('@') {
            redacted.push_str("<redacted>@");
            redacted.push_str(&value[authority_start + user_info_end + 1..authority_end]);
        } else {
            redacted.push_str(authority);
        }
        redacted.push_str(&value[authority_end..display_end]);
        cursor = url_end;
    }

    redacted.push_str(&value[cursor..]);
    redacted
}

#[cfg(test)]
fn parse_status_entries(output: &str) -> Vec<(String, String)> {
    parse_status_entries_detailed_bytes(output.as_bytes())
        .into_iter()
        .map(|(status, path, _)| (status, path))
        .collect()
}

fn parse_status_entries_detailed_bytes(output: &[u8]) -> Vec<(String, String, Option<String>)> {
    if !output.contains(&0) {
        return String::from_utf8_lossy(output)
            .lines()
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
                (!path.is_empty()).then_some((status, path, None))
            })
            .collect();
    }

    let records = output.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() || record.starts_with(b"!") {
            continue;
        }
        let Some(kind) = record.split(|byte| *byte == b' ').next() else {
            continue;
        };
        let parsed = match kind {
            b"1" => parse_v2_detailed_record_bytes(record, 9),
            b"2" => {
                let parsed = parse_v2_detailed_record_bytes(record, 10);
                // Rename/copy records carry the original path as a second NUL
                // record. The first path is the worktree-visible destination.
                index += 1;
                parsed
            }
            b"u" => parse_v2_detailed_record_bytes(record, 11),
            b"?" => Some((
                "??".to_string(),
                String::from_utf8_lossy(record.get(2..).unwrap_or_default()).to_string(),
                None,
            )),
            _ => None,
        };
        let Some((status, path, submodule_state)) = parsed else {
            continue;
        };
        let path = normalize_repo_relative_path(&path);
        if !path.is_empty() {
            entries.push((status, path, submodule_state));
        }
    }
    entries
}

fn parse_v2_detailed_record_bytes(
    record: &[u8],
    field_count: usize,
) -> Option<(String, String, Option<String>)> {
    let fields = record
        .splitn(field_count, |byte| *byte == b' ')
        .collect::<Vec<_>>();
    if fields.len() != field_count {
        return None;
    }
    let submodule_state = (!matches!(fields[2], b"N..." | b""))
        .then(|| String::from_utf8_lossy(fields[2]).to_string());
    Some((
        String::from_utf8_lossy(fields[1]).to_string(),
        String::from_utf8_lossy(fields[field_count - 1]).to_string(),
        submodule_state,
    ))
}

#[derive(Debug, Copy, Clone)]
enum GitFileTargetPolicy {
    Inspect,
    Stage,
    Unstage,
    Commit,
    Discard,
    Hunks,
}

// A repository boundary is a navigation boundary.  The parent repository may
// expose a submodule gitlink, but it must never treat files belonging to an
// independent child repository as its own files.
async fn resolve_git_file_target(
    project_path: &Path,
    file_path: &str,
    policy: GitFileTargetPolicy,
) -> Result<ResolvedRepositoryFile> {
    let resolved = resolve_repository_file_path(project_path, file_path).await?;
    let target = safe_repo_child(
        &resolved.repository_root,
        &resolved.repository_relative_file,
    )?;
    let canonical_target = fs::canonicalize(&target).await.unwrap_or(target.clone());
    let repository_root = normalize_path(&resolved.repository_root);
    let catalog = discover_git_workspace(&repository_root).await?;
    let child = catalog.repositories.into_iter().find(|repository| {
        if repository.path == repository_root {
            return false;
        }
        // Treat repository boundaries as overlapping paths in either
        // direction.  Checking only "target is inside child" would let a
        // request for a parent directory traverse into a nested repository
        // and accidentally stage, discard, or delete its contents.
        is_within(&canonical_target, &repository.path)
            || is_within(&target, &repository.path)
            || is_within(&repository.path, &canonical_target)
            || is_within(&repository.path, &target)
    });

    if let Some(child) = child {
        let exact_boundary = target == child.path || canonical_target == child.path;
        let allow_submodule_pointer = exact_boundary
            && match child.kind {
                iowb_protocol::GitRepositoryKind::Submodule => matches!(
                    policy,
                    GitFileTargetPolicy::Inspect
                        | GitFileTargetPolicy::Stage
                        | GitFileTargetPolicy::Unstage
                        | GitFileTargetPolicy::Commit
                ),
                // An uninitialized submodule can be inspected as a parent
                // gitlink, but it is not a worktree and therefore cannot be
                // staged, committed, discarded, or hunk-edited yet.
                iowb_protocol::GitRepositoryKind::Uninitialized =>
                    matches!(policy, GitFileTargetPolicy::Inspect),
                _ => false,
            };
        if !allow_submodule_pointer {
            let action = match policy {
                GitFileTargetPolicy::Inspect => "inspect",
                GitFileTargetPolicy::Stage => "stage",
                GitFileTargetPolicy::Unstage => "unstage",
                GitFileTargetPolicy::Commit => "commit",
                GitFileTargetPolicy::Discard => "discard",
                GitFileTargetPolicy::Hunks => "apply hunks to",
            };
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "Cannot {action} {file_path} from the parent repository. Select the {} repository at {} first.",
                    repository_kind_label(&child.kind), child.relative_path
                ),
            ));
        }
    }

    Ok(resolved)
}

async fn submodule_at_boundary(
    repository_root: &Path,
    repository_relative_file: &str,
) -> Result<Option<GitRepositoryRecord>> {
    let target = safe_repo_child(repository_root, repository_relative_file)?;
    let canonical_target = fs::canonicalize(&target).await.unwrap_or(target.clone());
    let repository_root = normalize_path(repository_root);
    let catalog = discover_git_workspace(&repository_root).await?;
    Ok(catalog.repositories.into_iter().find(|repository| {
        matches!(
            repository.kind,
            iowb_protocol::GitRepositoryKind::Submodule
                | iowb_protocol::GitRepositoryKind::Uninitialized
        )
            && (repository.path == canonical_target || repository.path == target)
    }))
}

async fn submodule_diff(
    repository_root: &Path,
    repository_relative_file: &str,
    staged: Option<bool>,
) -> Result<String> {
    if staged == Some(true) {
        return Ok(git(
            repository_root,
            [
                "diff",
                "--cached",
                "--submodule=log",
                "--",
                repository_relative_file,
            ],
        )
        .await?
        .stdout);
    }
    if staged == Some(false) {
        return Ok(git(
            repository_root,
            [
                "diff",
                "--submodule=log",
                "--",
                repository_relative_file,
            ],
        )
        .await?
        .stdout);
    }

    let unstaged = git(
        repository_root,
        [
            "diff",
            "--submodule=log",
            "--",
            repository_relative_file,
        ],
    )
    .await?
    .stdout;
    if !unstaged.trim().is_empty() {
        return Ok(unstaged);
    }
    Ok(git(
        repository_root,
        [
            "diff",
            "--cached",
            "--submodule=log",
            "--",
            repository_relative_file,
        ],
    )
    .await?
    .stdout)
}

fn repository_kind_label(kind: &iowb_protocol::GitRepositoryKind) -> &'static str {
    match kind {
        iowb_protocol::GitRepositoryKind::Root => "root",
        iowb_protocol::GitRepositoryKind::Submodule => "submodule",
        iowb_protocol::GitRepositoryKind::Nested => "nested",
        iowb_protocol::GitRepositoryKind::Worktree => "worktree",
        iowb_protocol::GitRepositoryKind::Uninitialized => "uninitialized submodule",
    }
}

fn strip_diff_headers(diff: &str) -> String {
    let mut filtered = Vec::new();
    let mut include = false;
    for line in diff.lines() {
        // Only discard the file-level preamble.  Once a hunk starts, lines
        // beginning with `---` or `+++` are valid user content and must stay
        // in the preview.
        if !include && line.starts_with("@@") {
            include = true;
        }
        if include {
            filtered.push(line);
        }
    }
    filtered.join("\n")
}

async fn enclosing_git_repository(project_path: &Path) -> Option<PathBuf> {
    let output = git(project_path, ["rev-parse", "--show-toplevel"])
        .await
        .ok()?;
    let root = output.stdout.trim();
    if root.is_empty() {
        return None;
    }
    let root = std::fs::canonicalize(root).ok()?;
    let project_path = normalize_path(project_path);
    let root = normalize_path(&root);
    (root != project_path && is_within(&project_path, &root)).then_some(root)
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
        redact_git_diagnostic(&format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )),
    ))
}

async fn repository_file_status(
    repository_root: &Path,
    repository_relative_file: &str,
) -> Result<Option<String>> {
    Ok(repository_file_status_details(repository_root, repository_relative_file)
        .await?
        .map(|(status, _)| status))
}

async fn repository_file_status_details(
    repository_root: &Path,
    repository_relative_file: &str,
) -> Result<Option<(String, Option<String>)>> {
    let output = git(
        repository_root,
        [
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
            "--",
            repository_relative_file,
        ],
    )
    .await?;
    Ok(parse_status_entries_detailed_bytes(&output.stdout_bytes)
        .into_iter()
        .find(|(_, path, _)| path == repository_relative_file)
        .map(|(status, _, submodule_state)| (status, submodule_state)))
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

fn status_contains(status: &str, expected: char) -> bool {
    status.chars().take(2).any(|character| character == expected)
}

fn status_is_untracked(status: &str) -> bool {
    status == "??"
}

fn status_is_deleted(status: &str) -> bool {
    status_contains(status, 'D')
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
    // Validate before normalizing.  Normalization intentionally removes a
    // leading `./`, but must never turn an absolute or backslash-rooted input
    // into an apparently relative path.
    validate_file_path(relative_path)?;
    let normalized = normalize_repo_relative_path(relative_path);
    validate_file_path(&normalized)?;
    let normalized_root = normalize_path(repository_root);
    let normalized_candidate = normalize_path(&normalized_root.join(normalized));
    if normalized_candidate == normalized_root || !is_within(&normalized_candidate, &normalized_root) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid file path: path traversal detected",
        ));
    }

    // Lexical checks do not protect against `link/file` when `link` points
    // outside the repository.  Canonicalize the deepest existing component so
    // both existing symlinks and symlinked parent directories are checked.
    let canonical_root = std::fs::canonicalize(&normalized_root).map_err(io_server_error)?;
    let mut existing = normalized_candidate.clone();
    loop {
        match std::fs::symlink_metadata(&existing) {
            Ok(_) => {
                let canonical_existing = std::fs::canonicalize(&existing).map_err(io_server_error)?;
                if !is_within(&canonical_existing, &canonical_root) {
                    return Err(ServerError::new(
                        StatusCode::BAD_REQUEST,
                        "Invalid file path: symlink escapes repository scope",
                    ));
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !existing.pop() {
                    return Err(ServerError::new(
                        StatusCode::BAD_REQUEST,
                        "Invalid file path",
                    ));
                }
            }
            Err(error) => return Err(io_server_error(error)),
        }
    }
    Ok(normalized_candidate)
}

fn validate_file_path(file: &str) -> Result<()> {
    let normalized = file.replace('\\', "/");
    if file.trim().is_empty()
        || file.contains('\0')
        || Path::new(file).is_absolute()
        || file.starts_with('\\')
        || normalized.split('/').any(|component| component == "..")
    {
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
    validate_git_ref_name(branch, "Invalid branch name")
}

fn validate_tag_name(tag: &str) -> Result<String> {
    validate_git_ref_name(tag, "Invalid tag name")
}

/// A mobile tag form only opts into `git tag --force` after the user has
/// reviewed a local tag from its loaded tag list.  Keep the server-side
/// default just as strict: a stale client that did not know about an existing
/// tag must never turn a normal create request into an overwrite.  Returning
/// a focused conflict here also avoids exposing Git's raw "tag already
/// exists" diagnostic to the UI.
async fn ensure_new_local_tag_or_explicit_replacement(
    repository: &Path,
    tag: &str,
    force: bool,
) -> Result<()> {
    if force {
        return Ok(());
    }
    let reference = format!("refs/tags/{tag}");
    if git(
        repository,
        ["show-ref", "--verify", "--quiet", reference.as_str()],
    )
    .await
    .is_ok()
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "A local tag named {tag} already exists. Refresh the tag list and explicitly confirm replacing it."
            ),
        ));
    }
    Ok(())
}

fn validate_git_ref_name(value: &str, message: &'static str) -> Result<String> {
    let trimmed = value.trim();
    let components = trimmed.split('/').collect::<Vec<_>>();
    let invalid = trimmed.is_empty()
        || trimmed.starts_with('-')
        || trimmed.len() > 1024
        || trimmed == "."
        || trimmed == ".."
        || trimmed.starts_with('/')
        || trimmed.ends_with('/')
        || trimmed.contains("//")
        || trimmed.contains("..")
        || trimmed.contains("@{")
        || trimmed.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
        || components.iter().any(|component| {
            component.is_empty()
                || component.starts_with('.')
                || component.ends_with('.')
                || component.ends_with(".lock")
        });
    if invalid {
        return Err(ServerError::new(StatusCode::BAD_REQUEST, message));
    }
    Ok(trimmed.to_string())
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
    let remote = validate_pattern(remote, "Invalid remote name", |character| {
        // Git remote names can contain path separators (for example
        // `team/review`). They namespace remote-tracking refs just like
        // branch names, and rejecting them makes existing repositories
        // impossible to manage from the native client.
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '/')
    })?;
    if remote == "."
        || remote == ".."
        || remote.starts_with('.')
        || remote.ends_with('.')
        || remote.starts_with('/')
        || remote.ends_with('/')
        || remote.contains("//")
        || remote.contains("..")
        || remote.split('/').any(|component| {
            component.is_empty()
                || component.starts_with('.')
                || component.ends_with('.')
                || component.ends_with(".lock")
        })
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid remote name",
        ));
    }
    Ok(remote)
}

fn validate_remote_url(url: &str) -> Result<String> {
    let trimmed = url.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('-')
        || trimmed.len() > 4096
        || trimmed.chars().any(|character| character.is_control())
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid remote URL",
        ));
    }
    if let Some(authority) = http_remote_authority(trimmed) {
        // Never let secrets or opaque bearer tokens become durable Git config
        // via a remote or submodule URL. SSH userinfo remains valid because
        // this guard is intentionally scoped to HTTP(S).
        if authority.contains('@') {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "HTTP(S) remote URLs cannot contain embedded credentials. Use SSH or a configured credential helper",
            ));
        }
        if trimmed.contains(['?', '#']) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "HTTP(S) remote URLs cannot contain a query or fragment. Use SSH or a configured credential helper",
            ));
        }
    }
    Ok(trimmed.to_string())
}

fn http_remote_authority(value: &str) -> Option<&str> {
    let scheme = ["https://", "http://"]
        .into_iter()
        .find(|scheme| {
            value
                .get(..scheme.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
        })?;
    Some(value[scheme.len()..].split('/').next().unwrap_or_default())
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

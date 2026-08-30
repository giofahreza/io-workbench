async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response> {
    let token = request_token(request.headers(), request.uri().query());
    let user = state.auth.require_user(token.as_deref())?;
    request.extensions_mut().insert(AuthenticatedUser(user));
    Ok(next.run(request).await)
}

async fn auth_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AuthStatusResponse>> {
    Ok(Json(state.auth.status(bearer_token(&headers).as_deref())?))
}

async fn auth_register(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<iowb_protocol::AuthTokenResponse>> {
    Ok(Json(
        state.auth.register(&request.username, &request.password)?,
    ))
}

async fn auth_login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<iowb_protocol::AuthTokenResponse>> {
    Ok(Json(
        state.auth.login(&request.username, &request.password)?,
    ))
}

async fn auth_logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<PlaceholderResponse>> {
    state.auth.logout(bearer_token(&headers).as_deref())?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "logged out successfully".to_string(),
    }))
}

async fn auth_user(Extension(user): Extension<AuthenticatedUser>) -> Result<Json<UserEnvelope>> {
    Ok(Json(UserEnvelope { user: user.0 }))
}

#[derive(serde::Serialize)]
struct UserEnvelope {
    user: iowb_protocol::UserProfile,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListProjectsQuery {
    include_sessions: Option<bool>,
}

async fn list_projects(
    State(state): State<AppState>,
    Query(query): Query<ListProjectsQuery>,
) -> Result<Json<ProjectListResponse>> {
    let mut projects = if query.include_sessions.unwrap_or(true) {
        state.projects.list(&state.sessions).await?
    } else {
        state.storage.list_projects()?
    };
    populate_repository_names(&mut projects).await;
    Ok(Json(ProjectListResponse { projects }))
}

async fn create_project(
    State(state): State<AppState>,
    Json(request): Json<CreateProjectRequest>,
) -> Result<Json<ProjectSummary>> {
    let path = state
        .path_validator
        .validate_path(PathBuf::from(request.path), false)
        .await?;
    let metadata = tokio::fs::metadata(&path).await.map_err(FsError::Io)?;
    if !metadata.is_dir() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "project path must be a directory",
        ));
    }

    let mut project = state.projects.add_project(&path)?;
    project.repo_name = project_repository_name(&project.path).await;
    publish_projects(&state).await;
    Ok(Json(project))
}

async fn populate_repository_names(projects: &mut [ProjectSummary]) {
    for project in projects {
        project.repo_name = project_repository_name(&project.path).await;
    }
}

async fn project_repository_name(project_path: &str) -> Option<String> {
    let config = tokio::fs::read_to_string(Path::new(project_path).join(".git/config"))
        .await
        .ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let value = line.trim();
        if value.starts_with('[') {
            in_origin = value == r#"[remote "origin"]"#;
            continue;
        }
        if !in_origin {
            continue;
        }
        let Some((key, remote)) = value.split_once('=') else {
            continue;
        };
        if key.trim() != "url" {
            continue;
        }
        return repository_name_from_remote(remote.trim());
    }
    None
}

fn repository_name_from_remote(remote: &str) -> Option<String> {
    remote
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .map(|name| name.trim_end_matches(".git").trim())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

async fn create_workspace(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<CreateWorkspaceRequest>,
) -> Result<Json<Value>> {
    let path = state
        .path_validator
        .validate_path(PathBuf::from(&request.path), false)
        .await?;

    match request.workspace_type {
        WorkspaceType::Existing => {
            let metadata = tokio::fs::metadata(&path).await.map_err(FsError::Io)?;
            if !metadata.is_dir() {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "existing workspace path must be a directory",
                ));
            }
        }
        WorkspaceType::New => {
            tokio::fs::create_dir_all(&path)
                .await
                .map_err(FsError::Io)?;
        }
    }

    let project_path = if let Some(github_url) = request
        .github_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        if request.workspace_type != WorkspaceType::New {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Git clone is only supported for new workspaces",
            ));
        }
        let github_token = resolve_github_token(
            &state,
            &user.0.id,
            request.github_token_id,
            request.new_github_token.as_deref(),
        )?;
        clone_repository(github_url, &path, github_token.as_deref()).await?
    } else {
        path
    };

    let project = state.projects.add_project(&project_path)?;
    publish_projects(&state).await;
    Ok(Json(serde_json::json!({
        "success": true,
        "project": project,
        "message": if request.github_url.is_some() {
            "New workspace created and repository cloned successfully"
        } else if request.workspace_type == WorkspaceType::New {
            "New workspace created successfully"
        } else {
            "Existing workspace added successfully"
        },
    })))
}

async fn clone_progress(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<CloneProgressQuery>,
) -> Response {
    let (tx, rx) = mpsc::channel::<Event>(64);

    tokio::spawn(async move {
        if let Err(error) = run_clone_progress(state, user, query, tx.clone()).await {
            send_sse_json(
                &tx,
                serde_json::json!({
                    "type": "error",
                    "message": error.body.error,
                    "details": error.body.details,
                }),
            )
            .await;
        }
    });

    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|event| (Ok::<_, Infallible>(event), rx))
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[derive(Debug, Deserialize)]
struct CloneProgressQuery {
    path: Option<String>,
    #[serde(rename = "workspacePath")]
    workspace_path: Option<String>,
    #[serde(rename = "githubUrl")]
    github_url: Option<String>,
    #[serde(rename = "githubTokenId")]
    github_token_id: Option<i64>,
    #[serde(rename = "newGithubToken")]
    new_github_token: Option<String>,
}

impl CloneProgressQuery {
    fn workspace_path(&self) -> Option<&str> {
        self.workspace_path
            .as_deref()
            .or(self.path.as_deref())
            .map(str::trim)
            .filter(|path| !path.is_empty())
    }
}

async fn run_clone_progress(
    state: AppState,
    user: AuthenticatedUser,
    query: CloneProgressQuery,
    tx: mpsc::Sender<Event>,
) -> Result<()> {
    let workspace_path = query
        .workspace_path()
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "workspacePath is required"))?;
    let github_url = query
        .github_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "githubUrl is required"))?;

    let path = state
        .path_validator
        .validate_path(PathBuf::from(workspace_path), false)
        .await?;
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(FsError::Io)?;

    let github_token = resolve_github_token(
        &state,
        &user.0.id,
        query.github_token_id,
        query.new_github_token.as_deref(),
    )?;
    clone_repository_with_progress(github_url, &path, github_token.as_deref(), &tx, &state).await
}

async fn send_sse_json(tx: &mpsc::Sender<Event>, value: Value) {
    let _ = tx.send(Event::default().data(value.to_string())).await;
}

fn resolve_github_token(
    state: &AppState,
    user_id: &str,
    credential_id: Option<i64>,
    one_time_token: Option<&str>,
) -> Result<Option<String>> {
    if let Some(credential_id) = credential_id {
        return state
            .storage
            .get_active_credential_value(user_id, credential_id, "github_token")?
            .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "GitHub token not found"))
            .map(Some);
    }

    Ok(one_time_token
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string))
}

async fn clone_repository(
    github_url: &str,
    workspace_path: &Path,
    github_token: Option<&str>,
) -> Result<PathBuf> {
    let repo_name = repository_name(github_url);
    let clone_path = workspace_path.join(&repo_name);
    ensure_clone_destination_available(&clone_path).await?;

    let clone_url = clone_url_with_token(github_url, github_token);
    let output = Command::new("git")
        .args([
            "clone",
            "--progress",
            &clone_url,
            &clone_path.display().to_string(),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Git is not installed or not in PATH",
                )
            } else {
                ServerError::with_details(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to start git clone",
                    error.to_string(),
                )
            }
        })?;

    if output.status.success() {
        return Ok(clone_path);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let message = clone_error_message(&format!("{stderr}{stdout}"), github_token);
    let _ = tokio::fs::remove_dir_all(&clone_path).await;
    Err(ServerError::new(StatusCode::BAD_REQUEST, message))
}

async fn clone_repository_with_progress(
    github_url: &str,
    workspace_path: &Path,
    github_token: Option<&str>,
    tx: &mpsc::Sender<Event>,
    state: &AppState,
) -> Result<()> {
    let repo_name = repository_name(github_url);
    let clone_path = workspace_path.join(&repo_name);
    ensure_clone_destination_available(&clone_path).await?;

    let clone_url = clone_url_with_token(github_url, github_token);
    send_sse_json(
        tx,
        serde_json::json!({
            "type": "progress",
            "message": format!("Cloning into '{repo_name}'...")
        }),
    )
    .await;

    let mut child = Command::new("git")
        .args([
            "clone",
            "--progress",
            &clone_url,
            &clone_path.display().to_string(),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Git is not installed or not in PATH",
                )
            } else {
                ServerError::with_details(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to start git clone",
                    error.to_string(),
                )
            }
        })?;

    let mut last_output = String::new();
    let (line_tx, mut line_rx) = mpsc::channel::<String>(64);
    if let Some(stdout) = child.stdout.take() {
        spawn_clone_line_reader(stdout, line_tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_clone_line_reader(stderr, line_tx.clone());
    }
    drop(line_tx);

    loop {
        tokio::select! {
            line = line_rx.recv() => {
                if let Some(line) = line {
                    last_output = sanitize_secret(&line, github_token);
                    send_sse_json(tx, serde_json::json!({
                        "type": "progress",
                        "message": last_output,
                    })).await;
                }
            }
            status = child.wait() => {
                match status {
                    Ok(status) if status.success() => {
                        let project = state.projects.add_project(&clone_path)?;
                        publish_projects(state).await;
                        send_sse_json(tx, serde_json::json!({
                            "type": "complete",
                            "project": project,
                            "message": "Repository cloned successfully",
                        })).await;
                        return Ok(());
                    }
                    Ok(_) => {
                        let message = clone_error_message(&last_output, github_token);
                        let _ = tokio::fs::remove_dir_all(&clone_path).await;
                        return Err(ServerError::new(StatusCode::BAD_REQUEST, message));
                    }
                    Err(error) => {
                        let _ = tokio::fs::remove_dir_all(&clone_path).await;
                        return Err(ServerError::with_details(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "git clone failed",
                            error.to_string(),
                        ));
                    }
                }
            }
        }
    }
}

fn spawn_clone_line_reader<R>(reader: R, tx: mpsc::Sender<String>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();
            if !line.is_empty() && tx.send(line).await.is_err() {
                break;
            }
        }
    });
}

async fn ensure_clone_destination_available(path: &Path) -> Result<()> {
    match tokio::fs::try_exists(path).await {
        Ok(false) => Ok(()),
        Ok(true) => Err(ServerError::new(
            StatusCode::CONFLICT,
            format!("Directory already exists: {}", path.display()),
        )),
        Err(error) => Err(ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to check clone destination",
            error.to_string(),
        )),
    }
}

fn repository_name(github_url: &str) -> String {
    let normalized = github_url.trim().trim_end_matches('/');
    normalized
        .strip_suffix(".git")
        .unwrap_or(normalized)
        .rsplit(['/', ':'])
        .next()
        .map(sanitize_repo_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "repository".to_string())
}

fn sanitize_repo_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .collect()
}

fn clone_url_with_token(github_url: &str, github_token: Option<&str>) -> String {
    let Some(token) = github_token.filter(|token| !token.is_empty()) else {
        return github_url.to_string();
    };
    let Some(rest) = github_url.strip_prefix("https://") else {
        return github_url.to_string();
    };
    format!("https://{}@{}", token, rest)
}

fn clone_error_message(raw: &str, github_token: Option<&str>) -> String {
    let sanitized = sanitize_secret(raw, github_token);
    if sanitized.contains("Authentication failed") || sanitized.contains("could not read Username")
    {
        "Authentication failed. Please check your credentials.".to_string()
    } else if sanitized.contains("Repository not found") {
        "Repository not found. Please check the URL and ensure you have access.".to_string()
    } else if sanitized.contains("already exists") {
        "Directory already exists".to_string()
    } else if sanitized.trim().is_empty() {
        "Git clone failed".to_string()
    } else {
        sanitized
    }
}

fn sanitize_secret(message: &str, secret: Option<&str>) -> String {
    let mut sanitized = message.trim().to_string();
    if let Some(secret) = secret.filter(|secret| !secret.is_empty()) {
        sanitized = sanitized.replace(secret, "***");
    }
    sanitized
}

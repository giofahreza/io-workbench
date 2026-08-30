#[derive(Debug, Clone)]
struct FcmConfig {
    project_id: String,
    service_account_json: String,
}

const FCM_ANDROID_OPEN_CHAT_ACTION: &str = "io.workbench.mobile.OPEN_CHAT";

#[derive(Debug, Deserialize)]
struct FirebaseServiceAccount {
    #[serde(default)]
    project_id: String,
    client_email: String,
    private_key: String,
    #[serde(default)]
    token_uri: Option<String>,
}

#[derive(Debug, Serialize)]
struct FcmJwtClaims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    iat: i64,
    exp: i64,
}

#[derive(Debug, Deserialize)]
struct FcmAccessTokenResponse {
    access_token: String,
}

fn fcm_config_from_env() -> Option<FcmConfig> {
    if !env_bool_local("IO_WORKBENCH_FCM_ENABLED", false) {
        return None;
    }
    let service_account_json = env::var("IO_WORKBENCH_FCM_SERVICE_ACCOUNT_JSON")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("IO_WORKBENCH_FCM_SERVICE_ACCOUNT_PATH")
                .ok()
                .or_else(|| env::var("GOOGLE_APPLICATION_CREDENTIALS").ok())
                .and_then(|path| std::fs::read_to_string(path).ok())
        })?;
    let service_account =
        serde_json::from_str::<FirebaseServiceAccount>(&service_account_json).ok()?;
    let project_id = env::var("IO_WORKBENCH_FCM_PROJECT_ID")
        .ok()
        .or_else(|| env::var("FIREBASE_PROJECT_ID").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(service_account.project_id)
        .trim()
        .to_string();
    if project_id.is_empty() {
        return None;
    }
    Some(FcmConfig {
        project_id,
        service_account_json,
    })
}

fn env_bool_local(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

async fn fcm_access_token(
    client: &reqwest::Client,
    service_account: &FirebaseServiceAccount,
) -> anyhow::Result<String> {
    let token_uri = service_account
        .token_uri
        .as_deref()
        .unwrap_or("https://oauth2.googleapis.com/token");
    let now = Utc::now().timestamp();
    let claims = FcmJwtClaims {
        iss: &service_account.client_email,
        scope: "https://www.googleapis.com/auth/firebase.messaging",
        aud: token_uri,
        iat: now,
        exp: now + 3600,
    };
    let assertion = jsonwebtoken::encode(
        &Header::new(Algorithm::RS256),
        &claims,
        &EncodingKey::from_rsa_pem(service_account.private_key.as_bytes())?,
    )?;
    let response = client
        .post(token_uri)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion.as_str()),
        ])
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("FCM OAuth token request failed: {status} {text}");
    }
    let token = serde_json::from_str::<FcmAccessTokenResponse>(&text)?;
    Ok(token.access_token)
}

async fn send_fcm_notification_to_token(
    client: &reqwest::Client,
    config: &FcmConfig,
    access_token: &str,
    token: &str,
    title: &str,
    body: &str,
    data: Value,
) -> anyhow::Result<()> {
    let url = format!(
        "https://fcm.googleapis.com/v1/projects/{}/messages:send",
        config.project_id
    );
    let response = client
        .post(url)
        .bearer_auth(access_token)
        .json(&fcm_notification_message(token, title, body, data))
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("FCM send failed: {status} {text}");
    }
    Ok(())
}

fn fcm_notification_message(token: &str, title: &str, body: &str, data: Value) -> Value {
    serde_json::json!({
            "message": {
                "token": token,
                "notification": {
                    "title": title,
                    "body": body,
                },
                "data": data,
                "android": {
                    "priority": "HIGH",
                    "notification": {
                        "channel_id": "io_workbench_activity",
                        "click_action": FCM_ANDROID_OPEN_CHAT_ACTION,
                        "default_sound": true,
                        "default_vibrate_timings": true,
                    }
                }
            }
        })
}

fn spawn_fcm_notification_bridge(state: AppState) {
    let Some(config) = fcm_config_from_env() else {
        info!("FCM push notifications are disabled");
        return;
    };
    let mut hub_rx = state.ws_hub.subscribe();
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                warn!(error = %error, "failed to create FCM HTTP client");
                return;
            }
        };
        loop {
            match hub_rx.recv().await {
                Ok(WsServerEvent::SessionStatus {
                    provider,
                    session_id,
                    status: iowb_protocol::SessionRuntimeStatus::Completed,
                    latest_user_prompt,
                    ..
                }) => {
                    if let Err(error) = send_fcm_chat_completed(
                        &state,
                        &client,
                        &config,
                        provider,
                        &session_id,
                        latest_user_prompt.as_deref(),
                    )
                    .await
                    {
                        warn!(error = %error, session_id = %session_id, "failed to send FCM chat completion notification");
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "FCM notification bridge lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

async fn send_fcm_chat_completed(
    state: &AppState,
    client: &reqwest::Client,
    config: &FcmConfig,
    provider: Provider,
    session_id: &str,
    latest_user_prompt: Option<&str>,
) -> anyhow::Result<()> {
    let session = state.sessions.get(session_id).await.ok();
    if !chat_completion_push_allowed(session.as_ref()) {
        // Board task activity is intentionally absent from ordinary chat
        // navigation/history, so it must not create an ordinary chat push.
        return Ok(());
    }
    let run = state
        .storage
        .latest_durable_chat_run_for_session(session_id)?;
    let user_id = run.as_ref().and_then(|run| run.user_id.as_deref());
    let tokens = if let Some(user_id) = user_id {
        state.storage.list_fcm_tokens_for_user(user_id)?
    } else {
        state.storage.list_all_fcm_tokens()?
    };
    if tokens.is_empty() {
        return Ok(());
    }

    let service_account =
        serde_json::from_str::<FirebaseServiceAccount>(&config.service_account_json)?;
    let access_token = fcm_access_token(client, &service_account).await?;
    let project_folder = session
        .as_ref()
        .and_then(|session| Path::new(&session.project_path).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("io-workbench");
    let prompt = latest_user_prompt
        .filter(|prompt| !prompt.trim().is_empty())
        .map(str::trim)
        .or_else(|| {
            run.as_ref()
                .map(|run| run.prompt.trim())
                .filter(|prompt| !prompt.is_empty())
        })
        .unwrap_or("latest prompt");
    let title = format!("{project_folder} | {}", provider.as_str());
    let body = format!("finished: {}", truncate_notification_text(prompt, 180));
    let data = serde_json::json!({
        "event": "chat_completed",
        "sessionId": session_id,
        "provider": provider.as_str(),
        "serverId": state.config.server_id(),
    });
    for stored in tokens {
        if let Err(error) = send_fcm_notification_to_token(
            client,
            config,
            &access_token,
            &stored.token,
            &title,
            &body,
            data.clone(),
        )
        .await
        {
            warn!(
                error = %error,
                user_id = %stored.user_id,
                platform = ?stored.platform,
                "failed to send FCM notification to token"
            );
        }
    }
    Ok(())
}

fn chat_completion_push_allowed(session: Option<&SessionSummary>) -> bool {
    !session.is_some_and(SessionSummary::is_board_session)
}

fn truncate_notification_text(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= max_chars {
            output.push('…');
            return output;
        }
        output.push(ch);
    }
    output
}

#[cfg(test)]
mod notification_payload_tests {
    use super::*;

    #[test]
    fn chat_completion_message_routes_notification_taps_to_the_chat_action() {
        let payload = fcm_notification_message(
            "device-token",
            "Project | codex",
            "finished: test",
            serde_json::json!({
                "event": "chat_completed",
                "sessionId": "session-123",
                "provider": "codex",
                "serverId": "iowb_test",
            }),
        );

        assert_eq!(
            payload["message"]["android"]["notification"]["click_action"],
            FCM_ANDROID_OPEN_CHAT_ACTION,
        );
        assert_eq!(payload["message"]["data"]["sessionId"], "session-123");
        assert_eq!(payload["message"]["data"]["serverId"], "iowb_test");
    }
}

fn spawn_process_event_bridge(state: AppState) {
    let mut rx = state.processes.subscribe();
    let hub = state.ws_hub.clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ProcessEvent::Output {
                    process_id,
                    stream,
                    data,
                }) => hub.publish(WsServerEvent::ProcessOutput {
                    process_id,
                    stream,
                    data,
                }),
                Ok(ProcessEvent::Exited { process_id, code }) => {
                    hub.publish(WsServerEvent::ProcessExited { process_id, code });
                }
                Ok(ProcessEvent::Failed {
                    process_id,
                    message,
                }) => hub.publish(WsServerEvent::Error {
                    message: format!("process {process_id} failed"),
                    details: Some(message),
                    session_id: None,
                }),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "process event bridge lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn spawn_project_watch_bridge(state: AppState) {
    let (tx, mut rx) = mpsc::channel::<notify::Result<notify::Event>>(256);
    let debounce = Duration::from_millis(state.watch.debounce_ms());

    tokio::spawn(async move {
        let mut registration: Option<ProjectWatchRegistration> = None;
        let mut discovery = tokio::time::interval(Duration::from_secs(5));
        discovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut pending_updates = HashMap::<String, HashSet<String>>::new();

        loop {
            tokio::select! {
                _ = discovery.tick() => {
                    refresh_project_watcher(&state, &tx, &mut registration);
                }
                Some(event) = rx.recv() => {
                    match event {
                        Ok(event) => {
                            if let Some(registration) = registration.as_mut() {
                                registration.watch_new_directories(&event);
                                for (project_path, event) in registration.events_by_project(&event) {
                                    let paths = interesting_project_watch_paths(&project_path, &event);
                                    if !paths.is_empty() {
                                        pending_updates
                                            .entry(project_path)
                                            .or_default()
                                            .extend(paths);
                                    }
                                }
                            }
                        }
                        Err(error) => warn!(error = %error, "project watcher error"),
                    }
                }
                _ = tokio::time::sleep(debounce), if !pending_updates.is_empty() => {
                    for (project_path, changed_paths) in std::mem::take(&mut pending_updates) {
                        let mut paths = changed_paths.into_iter().collect::<Vec<_>>();
                        paths.sort();
                        state.ws_hub.publish(WsServerEvent::ProjectFilesChanged {
                            project_path,
                            paths,
                        });
                    }
                }
                else => break,
            }
        }
    });
}

struct ProjectWatchRegistration {
    watcher: RecommendedWatcher,
    project_roots: Vec<PathBuf>,
    broad_roots: HashSet<PathBuf>,
    watched_directories: HashSet<PathBuf>,
}

impl ProjectWatchRegistration {
    fn events_by_project(&self, event: &notify::Event) -> Vec<(String, notify::Event)> {
        let mut paths_by_project = HashMap::<PathBuf, Vec<PathBuf>>::new();
        for path in &event.paths {
            let Some(project_root) = self
                .project_roots
                .iter()
                .filter(|root| path.starts_with(root))
                .max_by_key(|root| root.components().count())
            else {
                continue;
            };
            paths_by_project
                .entry(project_root.clone())
                .or_default()
                .push(path.clone());
        }
        paths_by_project
            .into_iter()
            .map(|(project_root, paths)| {
                let mut scoped = event.clone();
                scoped.paths = paths;
                (project_root.display().to_string(), scoped)
            })
            .collect()
    }

    fn watch_new_directories(&mut self, event: &notify::Event) {
        if !matches!(event.kind, notify::EventKind::Create(_)) {
            return;
        }
        for path in &event.paths {
            if !path.is_dir() {
                continue;
            }
            let Some(project_root) = self
                .project_roots
                .iter()
                .filter(|root| path.starts_with(root))
                .max_by_key(|root| root.components().count())
                .cloned()
            else {
                continue;
            };
            if self.broad_roots.contains(&project_root)
                || project_relative_path_is_excluded(&project_root, path)
            {
                continue;
            }
            register_filtered_project_tree(
                &mut self.watcher,
                &project_root,
                path,
                &mut self.watched_directories,
            );
        }
    }
}

fn refresh_project_watcher(
    state: &AppState,
    tx: &mpsc::Sender<notify::Result<notify::Event>>,
    registration: &mut Option<ProjectWatchRegistration>,
) {
    let mut project_roots = match state.storage.list_projects() {
        Ok(projects) => projects,
        Err(error) => {
            warn!(error = %error, "failed to load projects for watcher refresh");
            return;
        }
    }
    .into_iter()
    .map(|project| PathBuf::from(project.path))
    .filter(|path| path.is_dir())
    .collect::<Vec<_>>();
    project_roots.sort();
    project_roots.dedup();
    if registration
        .as_ref()
        .is_some_and(|current| current.project_roots == project_roots)
    {
        return;
    }

    let event_tx = tx.clone();
    let Ok(mut watcher) = notify::recommended_watcher(move |event| {
        // Registration can emit many events synchronously. Never wait for the
        // async bridge while the notify thread is building its watch set.
        let _ = event_tx.try_send(event);
    }) else {
        warn!("failed to create shared project watcher");
        return;
    };

    let home_dir = env::var_os("HOME").map(PathBuf::from);
    let broad_roots = project_roots
        .iter()
        .filter(|project_root| {
            project_watch_is_broad_root(
                project_root,
                &state.config.workspace_root,
                home_dir.as_deref(),
            )
        })
        .cloned()
        .collect::<HashSet<_>>();
    let mut watched_directories = HashSet::new();
    for project_root in &project_roots {
        if watched_directories.len() >= PROJECT_WATCH_MAX_DIRECTORIES_TOTAL {
            warn!(
                max_directories = PROJECT_WATCH_MAX_DIRECTORIES_TOTAL,
                "project watch directory budget reached"
            );
            break;
        }
        if broad_roots.contains(project_root) {
            if watcher
                .watch(project_root, RecursiveMode::NonRecursive)
                .is_ok()
            {
                watched_directories.insert(project_root.clone());
            }
            info!(
                project_path = %project_root.display(),
                "using root-only watch for broad project"
            );
            continue;
        }
        register_filtered_project_tree(
            &mut watcher,
            project_root,
            project_root,
            &mut watched_directories,
        );
    }

    info!(
        projects = project_roots.len(),
        directories = watched_directories.len(),
        "project watcher index ready"
    );
    *registration = Some(ProjectWatchRegistration {
        watcher,
        project_roots,
        broad_roots,
        watched_directories,
    });
}

fn register_filtered_project_tree(
    watcher: &mut RecommendedWatcher,
    project_root: &Path,
    start: &Path,
    watched_directories: &mut HashSet<PathBuf>,
) {
    let remaining_total =
        PROJECT_WATCH_MAX_DIRECTORIES_TOTAL.saturating_sub(watched_directories.len());
    let limit = PROJECT_WATCH_MAX_DIRECTORIES_PER_PROJECT.min(remaining_total);
    if limit == 0 {
        return;
    }
    let mut registered = 0usize;
    let walker = WalkDir::new(start)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !project_watch_directory_is_excluded(entry.file_name())
        });
    for entry in walker.filter_map(std::result::Result::ok) {
        if !entry.file_type().is_dir() {
            continue;
        }
        let path = entry.into_path();
        if project_relative_path_is_excluded(project_root, &path)
            || !watched_directories.insert(path.clone())
        {
            continue;
        }
        if let Err(error) = watcher.watch(&path, RecursiveMode::NonRecursive) {
            watched_directories.remove(&path);
            warn!(path = %path.display(), %error, "failed to watch project directory");
            continue;
        }
        registered += 1;
        if registered >= limit {
            warn!(
                project_path = %project_root.display(),
                max_directories = limit,
                "project watch directory budget reached for project"
            );
            break;
        }
    }
}

fn project_watch_directory_is_excluded(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| PROJECT_WATCH_EXCLUDED_DIRECTORIES.contains(&name))
}

fn project_watch_is_broad_root(
    project_root: &Path,
    workspace_root: &Path,
    home_dir: Option<&Path>,
) -> bool {
    home_dir.is_some_and(|home| project_root == home)
        || (workspace_root.starts_with(project_root) && workspace_root != project_root)
}

fn project_relative_path_is_excluded(project_root: &Path, path: &Path) -> bool {
    path.strip_prefix(project_root)
        .ok()
        .into_iter()
        .flat_map(Path::components)
        .any(|component| project_watch_directory_is_excluded(component.as_os_str()))
}

fn is_interesting_watch_event(event: &notify::Event) -> bool {
    use notify::EventKind;
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

fn interesting_project_watch_paths(project_path: &str, event: &notify::Event) -> Vec<String> {
    if !is_interesting_watch_event(event) {
        return Vec::new();
    }
    event
        .paths
        .iter()
        .filter_map(|path| project_relative_watch_path(project_path, path))
        .collect()
}

fn project_relative_watch_path(project_path: &str, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(project_path).ok()?;
    if relative
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == ".git")
    {
        return None;
    }
    let normalized = relative.to_string_lossy().replace('\\', "/");
    Some(if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized
    })
}

async fn publish_projects(state: &AppState) {
    match state.projects.list(&state.sessions).await {
        Ok(mut projects) => {
            populate_repository_names(&mut projects).await;
            state
                .ws_hub
                .publish(WsServerEvent::ProjectsUpdated { projects });
        }
        Err(error) => warn!(error = %error, "failed to publish project list"),
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_string)
}

fn request_token(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    bearer_token(headers).or_else(|| {
        query.and_then(|query| {
            query.split('&').find_map(|part| {
                let (key, value) = part.split_once('=')?;
                (key == "token" && !value.is_empty()).then(|| value.to_string())
            })
        })
    })
}

fn user_setting_key(user_id: &str, key: &str) -> String {
    format!("user:{user_id}:{key}")
}

fn is_io_gateway_setting_key(key: &str) -> bool {
    key == "direct-ai" || key.ends_with(":direct-ai")
}

fn public_settings(settings: Vec<iowb_protocol::SettingEntry>) -> Vec<iowb_protocol::SettingEntry> {
    settings
        .into_iter()
        .map(|mut setting| {
            if is_io_gateway_setting_key(&setting.key) {
                setting.value = public_direct_ai_config(&setting.value);
            }
            setting
        })
        .collect()
}

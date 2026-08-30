async fn list_settings(State(state): State<AppState>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "success": true,
        "settings": public_settings(state.storage.list_settings()?),
    })))
}

#[derive(Debug, Default, Deserialize)]
struct MobileSettingsOverviewQuery {
    section: Option<String>,
}

async fn mobile_settings_overview(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<MobileSettingsOverviewQuery>,
) -> Result<Json<Value>> {
    let user_id = &user.0.id;
    match query.section.as_deref() {
        Some("agents") => {
            let (claude_status, codex_status, gemini_status, claude_mcp_config, codex_mcp_config) = tokio::join!(
                provider_cli_status(Provider::Claude),
                provider_cli_status(Provider::Codex),
                provider_cli_status(Provider::Gemini),
                claude_mcp_config_overview(&state.config.workspace_root),
                codex_mcp_config_overview(),
            );
            let providers = serde_json::json!({
                "claude": claude_status,
                "codex": codex_status,
                "gemini": gemini_status,
            });
            return Ok(Json(serde_json::json!({
                "success": true,
                "agents": {
                    "providers": providers,
                    "permissions": {
                        "claude": state.storage
                            .get_setting(&user_setting_key(user_id, "claude-settings"))?
                            .unwrap_or_else(default_claude_agent_settings),
                        "codex": state.storage
                            .get_setting(&user_setting_key(user_id, "codex-settings"))?
                            .unwrap_or_else(default_codex_agent_settings),
                        "gemini": state.storage
                            .get_setting(&user_setting_key(user_id, "gemini-settings"))?
                            .unwrap_or_else(default_gemini_agent_settings),
                    },
                    "mcp": {
                        "servers": load_mcp_servers(&state, user_id)?,
                        "claude": { "config": claude_mcp_config },
                        "codex": { "config": codex_mcp_config },
                    },
                    "models": {
                        "claude": fallback_models(Provider::Claude),
                        "codex": fallback_models(Provider::Codex),
                        "gemini": fallback_models(Provider::Gemini),
                    }
                }
            })));
        }
        Some("appearance") => {
            return Ok(Json(serde_json::json!({
                "success": true,
                "appearance": state.storage
                    .get_setting(&user_setting_key(user_id, "appearance-settings"))?
                    .unwrap_or_else(default_appearance_settings),
            })));
        }
        Some("tasks") => {
            return Ok(Json(serde_json::json!({
                "success": true,
                "tasks": state.storage
                    .get_setting(&user_setting_key(user_id, "tasks-settings"))?
                    .unwrap_or_else(default_tasks_settings),
            })));
        }
        Some("plugins") => {
            return Ok(Json(serde_json::json!({
                "success": true,
                "plugins": compat_value(
                    &state,
                    user_id,
                    "plugins",
                    "/api/plugins",
                    serde_json::json!({
                        "plugins": [],
                        "namespace": "plugins",
                        "path": "/api/plugins"
                    }),
                )?,
            })));
        }
        _ => {}
    }

    let direct_ai_status = io_gateway_settings_status(&state, user_id)?;
    let direct_ai_effective = direct_ai_status
        .get("effective")
        .cloned()
        .unwrap_or_else(default_direct_ai_config);
    let (runtime, claude_status, codex_status, gemini_status, direct_ai_models) = tokio::join!(
        runtime_metrics_payload(&state),
        provider_cli_status(Provider::Claude),
        provider_cli_status(Provider::Codex),
        provider_cli_status(Provider::Gemini),
        timeout(
            Duration::from_secs(2),
            fetch_direct_ai_models(&direct_ai_effective),
        ),
    );
    let runtime = runtime?;
    let mut providers = serde_json::Map::new();
    providers.insert(Provider::Claude.as_str().to_string(), claude_status);
    providers.insert(Provider::Codex.as_str().to_string(), codex_status);
    providers.insert(Provider::Gemini.as_str().to_string(), gemini_status);
    let direct_ai_models = match direct_ai_models {
        Ok(Ok(models)) => models,
        _ => Vec::new(),
    };
    let git = current_git_config_overview(&state, user_id)?;
    let api_keys = state.storage.list_api_keys(user_id)?;
    let credentials = state.storage.list_credentials(user_id, None)?;
    let github_credentials = state
        .storage
        .list_credentials(user_id, Some("github_token"))?;
    let notification_preferences = state
        .storage
        .get_setting(&user_setting_key(user_id, "notification-preferences"))?
        .unwrap_or_else(default_notification_preferences);
    let plugins = compat_value(
        &state,
        user_id,
        "plugins",
        "/api/plugins",
        serde_json::json!({
            "plugins": [],
            "namespace": "plugins",
            "path": "/api/plugins"
        }),
    )?;
    let claude_mcp_config = claude_mcp_config_overview(&state.config.workspace_root).await;
    let codex_config = codex_config_overview().await;
    let codex_mcp_config = codex_mcp_config_overview().await;
    let claude_mcp_cli = compat_value(
        &state,
        user_id,
        "mcp",
        "/api/mcp/cli/list",
        serde_json::json!({ "success": true, "servers": [] }),
    )?;
    let codex_mcp_cli = compat_value(
        &state,
        user_id,
        "provider",
        "/api/codex/mcp/cli/list",
        serde_json::json!({ "success": true, "servers": [] }),
    )?;
    let mcp_utils_all = compat_value(
        &state,
        user_id,
        "mcp-utils",
        "/api/mcp-utils/all-servers",
        serde_json::json!({ "success": true, "servers": {} }),
    )?;
    let runtime_resources = runtime.get("resources").cloned().unwrap_or(Value::Null);
    let process_uptime_seconds = runtime_resources
        .get("processUptimeSeconds")
        .cloned()
        .unwrap_or(Value::Null);

    Ok(Json(serde_json::json!({
        "success": true,
        "server": {
            "status": state.config.server_status(VERSION),
            "runtime": runtime,
            "webStatus": {
                "success": true,
                "server": {
                    "status": "ok",
                    "timestamp": Utc::now(),
                    "appRoot": state.config.workspace_root.display().to_string(),
                    "installMode": "rust",
                    "packageName": PRODUCT_NAME,
                    "version": VERSION,
                    "uptimeSeconds": process_uptime_seconds,
                    "platform": std::env::consts::OS,
                    "arch": std::env::consts::ARCH,
                    "pid": std::process::id(),
                    "port": state.config.port.to_string(),
                    "environment": env::var("IO_WORKBENCH_ENV").unwrap_or_else(|_| "local".to_string()),
                    "resources": runtime_resources,
                }
            },
        },
        "agents": {
            "providers": providers,
            "permissions": {
                "claude": state.storage
                    .get_setting(&user_setting_key(user_id, "claude-settings"))?
                    .unwrap_or_else(default_claude_agent_settings),
                "codex": state.storage
                    .get_setting(&user_setting_key(user_id, "codex-settings"))?
                    .unwrap_or_else(default_codex_agent_settings),
                "gemini": state.storage
                    .get_setting(&user_setting_key(user_id, "gemini-settings"))?
                    .unwrap_or_else(default_gemini_agent_settings),
            },
            "mcp": {
                "servers": load_mcp_servers(&state, user_id)?,
                "claude": {
                    "config": claude_mcp_config,
                    "cli": claude_mcp_cli,
                },
                "codex": {
                    "config": codex_mcp_config,
                    "cli": codex_mcp_cli,
                },
                "utils": {
                    "allServers": mcp_utils_all,
                },
            },
            "models": {
                "claude": fallback_models(Provider::Claude),
                "codex": fallback_models(Provider::Codex),
                "gemini": fallback_models(Provider::Gemini),
            }
        },
        "codex": {
            "config": codex_config,
            "mcp": codex_mcp_config,
        },
        "appearance": state.storage
            .get_setting(&user_setting_key(user_id, "appearance-settings"))?
            .unwrap_or_else(default_appearance_settings),
        "git": git,
        "api": {
            "apiKeys": api_keys,
            "credentials": credentials,
            "githubCredentials": github_credentials,
        },
        "credentials": {
            "all": credentials,
            "github": github_credentials,
        },
        "tasks": state.storage
            .get_setting(&user_setting_key(user_id, "tasks-settings"))?
            .unwrap_or_else(default_tasks_settings),
        "notifications": {
            "preferences": notification_preferences,
            "fcm": {
                "enabled": env_bool_local("IO_WORKBENCH_FCM_ENABLED", false),
                "configured": fcm_config_from_env().is_some()
            },
        },
        "plugins": plugins,
        "directAi": {
            "config": direct_ai_status.get("config").cloned().unwrap_or(Value::Null),
            "effective": direct_ai_status.get("effective").cloned().unwrap_or(Value::Null),
            "runtimeReady": direct_ai_status.get("runtimeReady").cloned().unwrap_or(Value::Bool(false)),
            "apiKeyConfigured": direct_ai_status.get("apiKeyConfigured").cloned().unwrap_or(Value::Bool(false)),
            "auth": direct_ai_status.get("auth").cloned().unwrap_or(Value::Null),
            "models": direct_ai_models,
            "modelsEndpoint": "/api/settings/direct-ai/models",
        },
        "settings": {
            "all": public_settings(state.storage.list_settings()?),
        },
        "about": {
            "product": PRODUCT_NAME,
            "version": VERSION,
        }
    })))
}

async fn get_setting(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<String>,
) -> Result<Json<Value>> {
    let mut value = state
        .storage
        .get_setting(&key)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "setting not found"))?;
    if is_io_gateway_setting_key(&key) {
        value = public_direct_ai_config(&value);
    }
    Ok(Json(serde_json::json!({
        "success": true,
        "key": key,
        "value": value,
    })))
}

async fn set_setting(
    State(state): State<AppState>,
    AxumPath(key): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let value = body.get("value").cloned().unwrap_or(body);
    state.storage.set_setting(&key, &value)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "key": key,
        "value": value,
    })))
}

async fn get_notification_preferences(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "notification-preferences");
    let preferences = state
        .storage
        .get_setting(&key)?
        .unwrap_or_else(default_notification_preferences);
    Ok(Json(serde_json::json!({
        "success": true,
        "preferences": preferences,
    })))
}

async fn set_notification_preferences(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(preferences): Json<Value>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "notification-preferences");
    state.storage.set_setting(&key, &preferences)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "preferences": preferences,
    })))
}

async fn set_agent_preferences(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(provider): AxumPath<String>,
    Json(settings): Json<Value>,
) -> Result<Json<Value>> {
    if !settings.is_object() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "agent settings must be a JSON object",
        ));
    }
    let setting_name = match provider.as_str() {
        "claude" => "claude-settings",
        "codex" => "codex-settings",
        "gemini" => "gemini-settings",
        _ => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "provider must be claude, codex, or gemini",
            ));
        }
    };
    let key = user_setting_key(&user.0.id, setting_name);
    state.storage.set_setting(&key, &settings)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "provider": provider,
        "settings": settings,
    })))
}

async fn get_sidebar_active_sessions(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    let (pinned_sessions, initialized) = load_sidebar_active_sessions(&state, &user.0.id)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "pinnedSessions": pinned_sessions,
        "initialized": initialized,
    })))
}

async fn set_sidebar_active_sessions(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let pinned_sessions = filter_sidebar_active_sessions(
        &state,
        normalize_sidebar_active_sessions(
            body.get("pinnedSessions")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        ),
    )?;
    state
        .storage
        .set_setting(SIDEBAR_ACTIVE_SESSIONS_KEY, &pinned_sessions)?;
    state.storage.set_setting(
        &user_setting_key(&user.0.id, SIDEBAR_ACTIVE_SESSIONS_KEY),
        &pinned_sessions,
    )?;
    Ok(Json(serde_json::json!({
        "success": true,
        "pinnedSessions": pinned_sessions,
        "initialized": true,
    })))
}

const SIDEBAR_ACTIVE_SESSIONS_KEY: &str = "sidebar-active-sessions";

fn load_sidebar_active_sessions(state: &AppState, user_id: &str) -> Result<(Value, bool)> {
    let user_key = user_setting_key(user_id, SIDEBAR_ACTIVE_SESSIONS_KEY);
    if let Some(value) = state.storage.get_setting(&user_key)? {
        let pinned_sessions =
            filter_sidebar_active_sessions(state, normalize_sidebar_active_sessions(value))?;
        state.storage.set_setting(&user_key, &pinned_sessions)?;
        return Ok((pinned_sessions, true));
    }

    if let Some(value) = state.storage.get_setting(SIDEBAR_ACTIVE_SESSIONS_KEY)? {
        let pinned_sessions =
            filter_sidebar_active_sessions(state, normalize_sidebar_active_sessions(value))?;
        state.storage.set_setting(&user_key, &pinned_sessions)?;
        return Ok((pinned_sessions, true));
    }

    Ok((serde_json::json!([]), false))
}

fn sidebar_active_session_key(value: &Value) -> Option<String> {
    let session_id = value
        .get("sessionId")
        .or_else(|| value.get("session_id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
        .unwrap_or("")
        .trim();

    (!session_id.is_empty()).then(|| session_id.to_string())
}

fn normalize_sidebar_active_sessions(value: Value) -> Value {
    let Value::Array(items) = value else {
        return serde_json::json!([]);
    };
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    for item in items {
        let Some(key) = sidebar_active_session_key(&item) else {
            continue;
        };
        if !seen.insert(key) {
            continue;
        }
        normalized.push(item);
    }
    Value::Array(normalized)
}

fn filter_sidebar_active_sessions(state: &AppState, value: Value) -> Result<Value> {
    let Value::Array(items) = value else {
        return Ok(serde_json::json!([]));
    };
    let mut visible = Vec::with_capacity(items.len());
    for item in items {
        let Some(session_id) = sidebar_active_session_key(&item) else {
            continue;
        };
        if state
            .storage
            .get_session_summary(&session_id)?
            .is_some_and(|session| session.is_board_session())
        {
            continue;
        }
        visible.push(item);
    }
    Ok(Value::Array(visible))
}

#[derive(Debug, Default, serde::Deserialize)]
struct DirectAiSettingsQuery {
    #[serde(default, rename = "revealSecrets")]
    reveal_secrets: bool,
}

async fn get_direct_ai(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<DirectAiSettingsQuery>,
) -> Result<impl IntoResponse> {
    let mut status = io_gateway_settings_status(&state, &user.0.id)?;
    if query.reveal_secrets {
        let resolved = resolved_direct_ai_config_for_user(&state, &user.0.id);
        add_direct_ai_secrets(&mut status, &resolved);
    }
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(status)))
}

fn add_direct_ai_secrets(status: &mut Value, resolved: &Value) {
    if let Some(obj) = status.as_object_mut() {
        obj.insert(
            "secrets".to_string(),
            serde_json::json!({
                "gatewayApiKey": resolved
                    .get("gatewayApiKey")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                "gatewayOtpSecret": resolved
                    .get("gatewayOtpSecret")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            }),
        );
    }
}

async fn set_direct_ai(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(mut config): Json<Value>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "direct-ai");
    if let Some(stored) = state.storage.get_setting(&key)? {
        preserve_direct_ai_secrets(&stored, &mut config);
    }
    validate_direct_ai_config(&config)?;
    persist_io_gateway_secrets(&state, &user.0.id, &mut config)?;
    state.storage.set_setting(&key, &config)?;
    let status = io_gateway_settings_status(&state, &user.0.id)?;
    Ok(Json(serde_json::json!({
        "success": true,
        "config": status.get("config").cloned().unwrap_or(Value::Null),
    })))
}

#[derive(Debug, Default, serde::Deserialize)]
struct ChatModelsQuery {
    #[serde(default)]
    provider: Option<String>,
}

/// Per-provider model catalog used by the chat UI's model picker. Native CLI
/// mode reads local/fallback models, while IO Gateway mode reads the
/// configured gateway catalog without failing the picker when unavailable.
async fn chat_provider_models(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<ChatModelsQuery>,
) -> Result<Json<Value>> {
    let provider = match query.provider.as_deref() {
        Some(name) => parse_provider_param(name)?,
        None => Provider::Codex,
    };
    let runtime = configured_chat_runtime(&state, &user.0.id);
    let mut models: Vec<String> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut gateway_available = true;

    if runtime == ChatRuntime::IoGateway && matches!(provider, Provider::Codex | Provider::Claude) {
        if let Some(gateway_models) = direct_ai_models_for_user(&state, &user.0.id, provider).await
        {
            for model in gateway_models {
                push_chat_model(&mut models, &mut seen, model);
            }
        } else {
            gateway_available = false;
        }
    } else if matches!(provider, Provider::Codex) {
        let mut base_models = Vec::new();
        if let Some(model) = configured_codex_model()
            .await
            .filter(|model| is_local_codex_cli_model(model))
        {
            base_models.push(model);
        }
        base_models.extend(
            cached_codex_models()
                .await
                .into_iter()
                .filter(|model| is_local_codex_cli_model(model)),
        );
        push_codex_chat_models(&mut models, &mut seen, base_models);
    } else {
        for model in fallback_models(provider) {
            push_chat_model(&mut models, &mut seen, model);
        }
    }

    let mut model_values: Vec<Value> = Vec::new();
    if runtime == ChatRuntime::NativeCli && matches!(provider, Provider::Codex | Provider::Claude) {
        model_values.push(serde_json::json!({
            "value": "",
            "label": "CLI default",
        }));
    }
    model_values.extend(models.into_iter().map(|value| {
        serde_json::json!({
            "label": value.clone(),
            "value": value,
        })
    }));

    Ok(Json(serde_json::json!({
        "success": true,
        "provider": provider.as_str(),
        "runtime": runtime,
        "gatewayAvailable": gateway_available,
        "models": model_values,
    })))
}

fn push_chat_model(
    models: &mut Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
    model: impl AsRef<str>,
) {
    let value = model.as_ref().trim();
    if value.is_empty() {
        return;
    }
    let key = value.to_ascii_lowercase();
    if seen.insert(key) {
        models.push(value.to_string());
    }
}

fn push_codex_chat_models(
    models: &mut Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
    base_models: Vec<String>,
) {
    for model in base_models {
        if is_local_codex_cli_model(&model) {
            push_chat_model(models, seen, model);
        }
    }
}

fn is_local_codex_cli_model(model: &str) -> bool {
    let trimmed = model.trim();
    looks_like_model_id(trimmed)
        && !trimmed.contains(':')
        && !trimmed.eq_ignore_ascii_case("gpt-5-codex")
}

fn is_io_gateway_model(model: &str) -> bool {
    let trimmed = model.trim();
    let Some((prefix, rest)) = trimmed.split_once(':') else {
        return false;
    };
    !rest.trim().is_empty()
        && looks_like_model_id(trimmed)
        && matches!(
            prefix.to_ascii_lowercase().as_str(),
            "agw"
                | "cod"
                | "proxy"
                | "gateway"
                | "aiproxy"
                | "cld"
                | "gem"
                | "cop"
                | "ctm"
                | "dsk"
                | "glm"
                | "grk"
                | "min"
        )
}

async fn cached_codex_models() -> Vec<String> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let path = home.join(".codex").join("models_cache.json");
    let Some(content) = read_text_path(&path).await else {
        return Vec::new();
    };
    let Ok(root) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    root.get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|model| model.get("visibility").and_then(Value::as_str) != Some("hide"))
        .filter_map(|model| model.get("slug").and_then(Value::as_str))
        .filter(|model| looks_like_model_id(model))
        .map(str::to_string)
        .collect()
}

async fn configured_codex_model() -> Option<String> {
    let path = home_dir()?.join(".codex").join("config.toml");
    let content = read_text_path(&path).await?;
    parse_top_level_toml_values(&content)
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| looks_like_model_id(model))
        .map(str::to_string)
}

/// Fetch the configured gateway's model list for a chat provider.
async fn direct_ai_models_for_user(
    state: &AppState,
    user_id: &str,
    provider: Provider,
) -> Option<Vec<String>> {
    let config = chat_ai_config_for_user(state, user_id, provider);
    let raw = fetch_direct_ai_models(&config).await.unwrap_or_default();
    if raw.is_empty() {
        return None;
    }
    let ids: Vec<String> = raw
        .into_iter()
        .filter_map(|model| {
            model
                .get("value")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    model
                        .get("label")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        })
        .collect();
    if ids.is_empty() { None } else { Some(ids) }
}

fn direct_ai_runtime_config_for_user(
    state: &AppState,
    user_id: &str,
    provider: Provider,
) -> Option<DirectAiRuntimeConfig> {
    let config = chat_ai_config_for_user(state, user_id, provider);
    let (base_url, api_key) = direct_ai_endpoint_config(&config)?;
    let max_tokens = config
        .get("maxTokens")
        .or_else(|| config.get("max_tokens"))
        .and_then(Value::as_u64);
    Some(DirectAiRuntimeConfig {
        base_url,
        api_key,
        max_tokens,
    })
}

/// Resolve the gateway credential/config used for model discovery and gateway
/// CLI turns.
fn chat_ai_config_for_user(state: &AppState, user_id: &str, provider: Provider) -> Value {
    let mut config = resolved_direct_ai_config_for_user(state, user_id);
    if matches!(provider, Provider::Codex | Provider::Claude) {
        apply_io_gateway_config(&mut config, provider);
    }
    config
}

fn configured_chat_runtime(state: &AppState, user_id: &str) -> ChatRuntime {
    let key = user_setting_key(user_id, "direct-ai");
    let config = state.storage.get_setting(&key).ok().flatten();
    config
        .as_ref()
        .and_then(|config| {
            config
                .get("chatRuntime")
                .or_else(|| config.get("chat_runtime"))
        })
        .and_then(Value::as_str)
        .and_then(parse_chat_runtime)
        .unwrap_or_else(|| {
            let has_legacy_key = config
                .as_ref()
                .is_some_and(|config| direct_ai_secret_configured(config, "gatewayApiKey"))
                || state
                    .storage
                    .get_active_credential_value_by_name(
                        user_id,
                        IO_GATEWAY_API_KEY_CREDENTIAL,
                        IO_GATEWAY_API_KEY_CREDENTIAL_TYPE,
                    )
                    .ok()
                    .flatten()
                    .is_some();
            if has_legacy_key {
                ChatRuntime::IoGateway
            } else {
                ChatRuntime::NativeCli
            }
        })
}

fn parse_chat_runtime(value: &str) -> Option<ChatRuntime> {
    match value.trim().to_ascii_lowercase().as_str() {
        "native_cli" | "native" | "cli" | "default" => Some(ChatRuntime::NativeCli),
        "io_gateway" | "gateway" | "custom_api" | "aiproxy" => Some(ChatRuntime::IoGateway),
        _ => None,
    }
}

fn io_gateway_settings_status(state: &AppState, user_id: &str) -> Result<Value> {
    let key = user_setting_key(user_id, "direct-ai");
    let config = state
        .storage
        .get_setting(&key)?
        .unwrap_or_else(default_direct_ai_config);
    let mut effective = resolved_direct_ai_config_for_user(state, user_id);
    apply_io_gateway_config(&mut effective, Provider::Claude);

    let api_key_configured = direct_ai_secret_configured(&effective, "gatewayApiKey");
    let otp_configured = direct_ai_secret_configured(&effective, "gatewayOtpSecret");
    let auth = state.auth.status(None)?;

    let mut public_config = public_direct_ai_config(&config);
    if let Some(obj) = public_config.as_object_mut() {
        obj.insert(
            "gatewayApiKeyConfigured".to_string(),
            Value::Bool(api_key_configured),
        );
        obj.insert(
            "gatewayOtpConfigured".to_string(),
            Value::Bool(otp_configured),
        );
    }

    Ok(serde_json::json!({
        "success": true,
        "config": public_config,
        "effective": {
            "chatRuntime": configured_chat_runtime(state, user_id),
            "mode": effective.get("mode").cloned().unwrap_or(Value::Null),
            "baseUrl": effective.get("baseUrl").cloned().unwrap_or(Value::Null),
            "apiKeyEnv": effective.get("apiKeyEnv").cloned().unwrap_or(Value::Null),
            "model": effective.get("model").cloned().unwrap_or(Value::Null),
        },
        "runtimeReady": direct_ai_endpoint_config(&effective).is_some(),
        "apiKeyConfigured": api_key_configured,
        "gatewayOtpConfigured": otp_configured,
        "auth": {
            "mode": auth.auth_mode,
            "otpConfigured": state.config.otp_secret.is_some(),
            "tokenConfigured": state.config.local_token.is_some(),
        },
    }))
}

fn apply_io_gateway_config(config: &mut Value, provider: Provider) {
    if !config.is_object() {
        *config = default_direct_ai_config();
    }

    let Some(obj) = config.as_object_mut() else {
        return;
    };
    obj.insert("mode".to_string(), Value::String("aiproxy".to_string()));
    obj.remove("base_url");
    let endpoint_key = if provider == Provider::Codex {
        "codexEndpoint"
    } else {
        "claudeEndpoint"
    };
    let default_endpoint = if provider == Provider::Codex {
        "codex"
    } else {
        "claude"
    };
    let endpoint = obj
        .get(endpoint_key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_endpoint);
    let gateway_root = obj
        .get("gatewayUrl")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .or_else(|| {
            obj.get("baseUrl")
                .and_then(Value::as_str)
                .and_then(|value| {
                    let value = value.trim().trim_end_matches('/');
                    if io_gateway_url_has_endpoint_path(value, endpoint) {
                        Some(value.to_string())
                    } else {
                        url_origin(value)
                    }
                })
        })
        .unwrap_or_else(|| {
            url_origin(DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL)
                .unwrap_or_else(|| DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL.to_string())
        });
    let base_url = join_io_gateway_endpoint_url(&gateway_root, endpoint);
    obj.insert("baseUrl".to_string(), Value::String(base_url));
    obj.remove("api_key_env");
    obj.remove("apiKeyEnv");
}

pub(crate) fn join_io_gateway_endpoint_url(gateway_root: &str, endpoint: &str) -> String {
    let gateway_root = gateway_root.trim().trim_end_matches('/');
    let endpoint = endpoint.trim();
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return endpoint.trim_end_matches('/').to_string();
    }
    let endpoint = endpoint.trim_matches('/');
    if endpoint.is_empty() || io_gateway_url_has_endpoint_path(gateway_root, endpoint) {
        gateway_root.to_string()
    } else {
        format!("{gateway_root}/{endpoint}")
    }
}

fn io_gateway_url_has_endpoint_path(url: &str, endpoint: &str) -> bool {
    let without_fragment = url.split_once('#').map_or(url, |(value, _)| value);
    let without_suffix = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(value, _)| value);
    let path = without_suffix
        .split_once("://")
        .map_or(without_suffix, |(_, remainder)| {
            remainder
                .find('/')
                .map_or("", |path_start| &remainder[path_start + 1..])
        });
    let path_segments: Vec<_> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let endpoint_segments: Vec<_> = endpoint
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    !endpoint_segments.is_empty() && path_segments.ends_with(&endpoint_segments)
}

fn direct_ai_secret_configured(config: &Value, key: &str) -> bool {
    config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

fn preserve_direct_ai_secrets(stored: &Value, config: &mut Value) {
    let (Some(stored), Some(config)) = (stored.as_object(), config.as_object_mut()) else {
        return;
    };
    for key in ["gatewayApiKey", "gatewayOtpSecret"] {
        if !config.contains_key(key) {
            if let Some(value) = stored.get(key) {
                config.insert(key.to_string(), value.clone());
            }
        }
    }
}

fn persist_io_gateway_secrets(state: &AppState, user_id: &str, config: &mut Value) -> Result<()> {
    let Some(obj) = config.as_object_mut() else {
        return Ok(());
    };
    if let Some(secret) = obj
        .remove("gatewayApiKey")
        .and_then(|value| value.as_str().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty())
    {
        state.storage.upsert_named_credential(
            user_id,
            IO_GATEWAY_API_KEY_CREDENTIAL,
            IO_GATEWAY_API_KEY_CREDENTIAL_TYPE,
            &secret,
            Some("IO Gateway API key"),
        )?;
    }
    if let Some(secret) = obj
        .remove("gatewayOtpSecret")
        .and_then(|value| value.as_str().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty())
    {
        state.storage.upsert_named_credential(
            user_id,
            IO_GATEWAY_OTP_CREDENTIAL,
            IO_GATEWAY_OTP_CREDENTIAL_TYPE,
            &secret,
            Some("IO Gateway TOTP secret"),
        )?;
    }
    Ok(())
}

fn public_direct_ai_config(config: &Value) -> Value {
    let mut public = config.clone();
    let api_key_configured = direct_ai_secret_configured(config, "gatewayApiKey");
    let otp_configured = direct_ai_secret_configured(config, "gatewayOtpSecret");
    if let Some(obj) = public.as_object_mut() {
        obj.remove("gatewayApiKey");
        obj.remove("gatewayOtpSecret");
        obj.insert(
            "gatewayApiKeyConfigured".to_string(),
            Value::Bool(api_key_configured),
        );
        obj.insert(
            "gatewayOtpConfigured".to_string(),
            Value::Bool(otp_configured),
        );
    }
    public
}

fn resolved_direct_ai_config_for_user(state: &AppState, user_id: &str) -> Value {
    let key = user_setting_key(user_id, "direct-ai");
    let mut config = state
        .storage
        .get_setting(&key)
        .ok()
        .flatten()
        .unwrap_or_else(default_direct_ai_config);
    if direct_ai_secret_configured(&config, "gatewayApiKey")
        || direct_ai_secret_configured(&config, "gatewayOtpSecret")
    {
        let mut sanitized = config.clone();
        if sanitized
            .get("chatRuntime")
            .or_else(|| sanitized.get("chat_runtime"))
            .is_none()
            && direct_ai_secret_configured(&sanitized, "gatewayApiKey")
            && let Some(obj) = sanitized.as_object_mut()
        {
            obj.insert(
                "chatRuntime".to_string(),
                Value::String("io_gateway".to_string()),
            );
        }
        if persist_io_gateway_secrets(state, user_id, &mut sanitized).is_ok() {
            let _ = state.storage.set_setting(&key, &sanitized);
            config = sanitized;
        }
    }
    if let Some(obj) = config.as_object_mut() {
        if let Ok(Some(secret)) = state.storage.get_active_credential_value_by_name(
            user_id,
            IO_GATEWAY_API_KEY_CREDENTIAL,
            IO_GATEWAY_API_KEY_CREDENTIAL_TYPE,
        ) {
            obj.insert("gatewayApiKey".to_string(), Value::String(secret));
        }
        if let Ok(Some(secret)) = state.storage.get_active_credential_value_by_name(
            user_id,
            IO_GATEWAY_OTP_CREDENTIAL,
            IO_GATEWAY_OTP_CREDENTIAL_TYPE,
        ) {
            obj.insert("gatewayOtpSecret".to_string(), Value::String(secret));
        }
    }
    config
}

/// Heuristic check that a token is a plausible model identifier. Model ids
/// from the supported providers are short slugs (e.g. `gpt-5-codex`,
/// `claude-opus-4-1`, `gemini-2.5-pro`) that contain letters or digits plus
/// `-`, `.`, `_`, `:`, or `/`. Real identifiers are at least 3 characters
/// and always contain either a digit or one of the separator characters
/// (otherwise prose tokens like `A` or `If` from interactive prompts would
/// be misclassified as model ids).
fn looks_like_model_id(token: &str) -> bool {
    if token.len() < 3 || token.len() > 80 {
        return false;
    }
    if token.chars().any(char::is_whitespace) {
        return false;
    }
    let mut has_alnum = false;
    let mut has_separator = false;
    for ch in token.chars() {
        if ch.is_alphanumeric() {
            has_alnum = true;
            continue;
        }
        if matches!(ch, '-' | '.' | '_' | ':' | '/') {
            has_separator = true;
            continue;
        }
        return false;
    }
    has_alnum && (has_separator || token.chars().any(|c| c.is_ascii_digit()))
}

fn fallback_models(provider: Provider) -> Vec<String> {
    match provider {
        Provider::Codex => vec![
            "gpt-5".to_string(),
            "gpt-5-codex".to_string(),
            "gpt-5-mini".to_string(),
            "gpt-5-nano".to_string(),
            "gpt-4.1".to_string(),
            "gpt-4.1-mini".to_string(),
            "gpt-4o".to_string(),
            "o4-mini".to_string(),
        ],
        Provider::Claude => vec![
            "sonnet".to_string(),
            "opus".to_string(),
            "haiku".to_string(),
            "fable".to_string(),
            "claude-sonnet-4-5".to_string(),
            "claude-sonnet-4".to_string(),
            "claude-opus-4".to_string(),
            "claude-3-7-sonnet-latest".to_string(),
            "claude-3-5-sonnet-latest".to_string(),
            "claude-3-5-haiku-latest".to_string(),
        ],
        Provider::Gemini => vec![
            "gemini-2.5-pro".to_string(),
            "gemini-2.5-flash".to_string(),
            "gemini-2.0-flash".to_string(),
            "gemini-1.5-pro-latest".to_string(),
            "gemini-1.5-flash-latest".to_string(),
        ],
    }
}

async fn direct_ai_models(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Json<Value> {
    let mut config = resolved_direct_ai_config_for_user(&state, &user.0.id);
    apply_io_gateway_config(&mut config, Provider::Claude);
    let models = fetch_direct_ai_models(&config).await.unwrap_or_default();
    Json(serde_json::json!({
        "success": true,
        "models": models,
    }))
}

async fn get_git_config(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "git-config");
    let stored = state.storage.get_setting(&key)?;
    let git_name = stored
        .as_ref()
        .and_then(|value| value.get("gitName"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| stored_git_alias(&stored, "git_name"))
        .or_else(|| blocking_git_config("user.name"));
    let git_email = stored
        .as_ref()
        .and_then(|value| value.get("gitEmail"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| stored_git_alias(&stored, "git_email"))
        .or_else(|| blocking_git_config("user.email"));

    if stored.is_none() && (git_name.is_some() || git_email.is_some()) {
        state.storage.set_setting(
            &key,
            &serde_json::json!({
                "gitName": git_name,
                "gitEmail": git_email,
            }),
        )?;
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "gitName": git_name,
        "gitEmail": git_email,
    })))
}

async fn set_git_config(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<GitConfigRequest>,
) -> Result<Json<Value>> {
    let git_name = request.git_name.trim();
    let git_email = request.git_email.trim();
    if git_name.is_empty() || git_email.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Git name and email are required",
        ));
    }
    if !looks_like_email(git_email) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid email format",
        ));
    }

    let key = user_setting_key(&user.0.id, "git-config");
    state.storage.set_setting(
        &key,
        &serde_json::json!({
            "gitName": git_name,
            "gitEmail": git_email,
        }),
    )?;

    let name_result = set_git_global_config("user.name", git_name).await;
    let email_result = set_git_global_config("user.email", git_email).await;
    let git_applied = name_result.is_ok() && email_result.is_ok();

    Ok(Json(serde_json::json!({
        "success": true,
        "gitName": git_name,
        "gitEmail": git_email,
        "gitApplied": git_applied,
        "gitError": name_result
            .err()
            .or_else(|| email_result.err())
            .map(|error| error.to_string()),
    })))
}

async fn onboarding_status(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "onboarding-complete");
    let has_completed = state
        .storage
        .get_setting(&key)?
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    Ok(Json(serde_json::json!({
        "success": true,
        "hasCompletedOnboarding": has_completed,
    })))
}

async fn complete_onboarding(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    let key = user_setting_key(&user.0.id, "onboarding-complete");
    state.storage.set_setting(&key, &Value::Bool(true))?;
    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Onboarding completed successfully",
    })))
}

async fn user_settings_overview(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "success": true,
        "user": user.0,
        "gitConfig": state.storage.get_setting(&user_setting_key(&user.0.id, "git-config"))?,
        "hasCompletedOnboarding": state
            .storage
            .get_setting(&user_setting_key(&user.0.id, "onboarding-complete"))?
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
    })))
}

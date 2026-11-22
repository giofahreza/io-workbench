// Documentation is deliberately data, never a command or an installer.
fn provider_setup_urls(provider: Provider) -> (&'static str, &'static str) {
    match provider {
        Provider::Codex => (
            "https://developers.openai.com/codex/cli/",
            "https://developers.openai.com/codex/auth/",
        ),
        Provider::Claude => (
            "https://code.claude.com/docs/en/setup",
            "https://code.claude.com/docs/en/authentication",
        ),
        Provider::Gemini => (
            "https://geminicli.com/docs/get-started/installation/",
            "https://geminicli.com/docs/get-started/authentication/",
        ),
    }
}
const NODE_INSTALL_URL: &str = "https://nodejs.org/en/download";
type ProviderProbeCache = tokio::sync::Mutex<Option<(Instant, String, Value)>>;
static PROVIDER_READINESS: OnceLock<[ProviderProbeCache; 3]> = OnceLock::new();

// At most three subprocesses at a time. Cache and coalesce checks, including
// double-clicked Recheck, without caching user-specific gateway credentials.
async fn cached_provider_readiness(provider: Provider, refresh: bool) -> Value {
    let index = match provider {
        Provider::Codex => 0,
        Provider::Claude => 1,
        Provider::Gemini => 2,
    };
    let cache =
        PROVIDER_READINESS.get_or_init(|| std::array::from_fn(|_| tokio::sync::Mutex::new(None)));
    let mut entry = cache[index].lock().await;
    let command = provider_command(provider);
    if let Some((at, previous_command, status)) = entry.as_ref()
        && previous_command == &command
        && at.elapsed() < Duration::from_secs(if refresh { 1 } else { 10 })
    {
        return status.clone();
    }
    let status = probe_provider_readiness(provider, &command).await;
    *entry = Some((Instant::now(), command, status.clone()));
    status
}

// Capture bounded stdout only. Status stderr may contain credentials; drain it
// to the null device and never include provider output in errors or logs.
async fn readiness_command(command: &str, args: &[&str]) -> Option<String> {
    readiness_command_with_timeout(command, args, Duration::from_secs(8)).await
}

async fn readiness_command_with_timeout(
    command: &str,
    args: &[&str],
    deadline: Duration,
) -> Option<String> {
    let operation = async {
        let mut child = Command::new(command)
            .args(args)
            .env("PATH", augmented_user_path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        let mut bytes = Vec::new();
        tokio::io::AsyncReadExt::take(stdout, 16_385)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > 16_384 {
            return None;
        }
        child
            .wait()
            .await
            .ok()?
            .success()
            .then(|| String::from_utf8_lossy(&bytes).into_owned())
    };
    timeout(deadline, operation).await.ok().flatten()
}

async fn probe_provider_readiness(provider: Provider, command: &str) -> Value {
    let (install_url, auth_url) = provider_setup_urls(provider);
    let version = readiness_command(command, &["--version"])
        .await
        .and_then(|output| {
            output
                .split_whitespace()
                .find(|token| {
                    token.len() <= 64
                        && token.starts_with(|c: char| c.is_ascii_digit())
                        && token.contains('.')
                        && token
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
                })
                .map(str::to_string)
        });
    let installed = version.is_some();
    let auth_state = if !installed {
        "unavailable"
    } else {
        match provider {
            Provider::Codex => {
                // Codex writes login status to stderr in some versions. Its exit
                // code is the contract; no output needs to be displayed.
                if readiness_command(command, &["login", "status"])
                    .await
                    .is_some()
                {
                    "authenticated"
                } else {
                    "required"
                }
            }
            Provider::Claude => {
                match readiness_command(command, &["auth", "status", "--json"])
                    .await
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|v| v.get("loggedIn").and_then(Value::as_bool))
                {
                    Some(true) => "authenticated",
                    Some(false) => "required",
                    None => "unknown",
                }
            }
            // Gemini has no stable, non-interactive login-status command. Do
            // not call a paid prompt or equate a credentials file with validity.
            Provider::Gemini => {
                if gemini_auth_configured() {
                    "configured_unverified"
                } else {
                    "required"
                }
            }
        }
    };
    let native_ready = matches!(auth_state, "authenticated" | "configured_unverified");
    let error = if !installed {
        Some(
            "CLI could not be verified for the Workbench service user. Install or repair it on the server and Recheck.",
        )
    } else if !native_ready {
        Some(
            "Native authentication could not be verified. Follow the official authentication guide on the server, or choose IO Gateway.",
        )
    } else {
        None
    };
    let node_available = if !installed {
        readiness_command("node", &["--version"]).await.is_some()
    } else {
        true
    };
    let npm_available = if !installed {
        readiness_command("npm", &["--version"]).await.is_some()
    } else {
        true
    };
    serde_json::json!({ "installed": installed, "version": version,
        "authenticated": auth_state == "authenticated", "authState": auth_state, "nativeReady": native_ready,
        "error": error, "email": null, "method": if native_ready { Some("cli_status") } else { None },
        "installUrl": install_url, "authUrl": auth_url, "nodeInstallUrl": NODE_INSTALL_URL,
        "nodeAvailable": node_available, "npmAvailable": npm_available,
        "scope": "Install and authenticate on the Workbench server, as the user running the Workbench service. Installing on your phone or another SSH user does not configure this service." })
}

fn gemini_auth_configured() -> bool {
    first_present_auth_env_key(&["GEMINI_API_KEY", "GOOGLE_API_KEY"]).is_some()
        || home_dir().is_some_and(|home| {
            gemini_credentials_configured(&home.join(".gemini/oauth_creds.json"))
        })
}

fn gemini_credentials_configured(path: &Path) -> bool {
    if !std::fs::metadata(path).is_ok_and(|metadata| metadata.len() <= 1024 * 1024) {
        return false;
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|value| {
            ["refresh_token", "access_token"].iter().any(|key| {
                value
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty())
            })
        })
}

fn chat_setup_problem(code: &str, message: &str, action: &str, docs: Option<&str>) -> Value {
    serde_json::json!({ "code": code, "message": message, "action": action, "docsUrl": docs })
}

fn native_model_problem(runtime: ChatRuntime, model: Option<&str>) -> Option<Value> {
    let model = model.map(str::trim).filter(|s| !s.is_empty())?;
    if runtime == ChatRuntime::NativeCli && is_io_gateway_model(model) {
        return Some(chat_setup_problem(
            "chat_runtime_mismatch",
            "This chat uses Native CLI. Gateway models require a new chat using IO Gateway. Your existing transcript will be kept.",
            "new_chat",
            None,
        ));
    }
    if !valid_chat_model_id(model) || (runtime == ChatRuntime::NativeCli && model.contains(':')) {
        return Some(chat_setup_problem(
            "chat_model_unavailable",
            "This model is unavailable for the selected route. Recheck the model list.",
            "recheck",
            None,
        ));
    }
    None
}

fn valid_chat_model_id(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= MAX_SESSION_MODEL_LENGTH
        && model.starts_with(|ch: char| ch.is_ascii_alphanumeric())
        && model
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | ':' | '/'))
}

fn checked_route_model(
    runtime: ChatRuntime,
    requested: Option<&str>,
    catalog: &Value,
    stored: Option<&str>,
) -> std::result::Result<Option<String>, Value> {
    let requested = requested.map(str::trim).filter(|id| !id.is_empty());
    if let Some(problem) = native_model_problem(runtime, requested) {
        return Err(problem);
    }
    if runtime == ChatRuntime::NativeCli && requested.is_none() {
        return Ok(None);
    }
    let canonical = requested.and_then(|id| {
        catalog["models"].as_array().and_then(|models| {
            models
                .iter()
                .filter_map(|model| model["value"].as_str())
                .find(|value| value.eq_ignore_ascii_case(id))
        })
    });
    if let Some(id) = canonical {
        return Ok(Some(id.to_string()));
    }
    // An old native chat may use a valid model not present in today's cache.
    // Continue that exact saved choice without accepting arbitrary new IDs.
    if runtime == ChatRuntime::NativeCli
        && let (Some(id), Some(saved)) = (requested, stored)
        && id.eq_ignore_ascii_case(saved.trim())
    {
        return Ok(Some(saved.trim().to_string()));
    }
    Err(chat_setup_problem(
        "chat_model_unavailable",
        "Select an available model for this chat route and retry.",
        "recheck",
        None,
    ))
}

async fn mobile_chat_catalogs(state: &AppState, user_id: &str, providers: &Value) -> Value {
    let runtime = configured_chat_runtime(state, user_id);
    let (claude, codex, gemini) = tokio::join!(
        chat_route_catalog_with_status(
            state,
            user_id,
            Provider::Claude,
            runtime,
            providers["claude"].clone()
        ),
        chat_route_catalog_with_status(
            state,
            user_id,
            Provider::Codex,
            runtime,
            providers["codex"].clone()
        ),
        chat_route_catalog_with_status(
            state,
            user_id,
            Provider::Gemini,
            ChatRuntime::NativeCli,
            providers["gemini"].clone()
        ),
    );
    let routes = serde_json::json!({ "claude": claude, "codex": codex, "gemini": gemini });
    let mut models = serde_json::Map::new();
    for provider in ["claude", "codex", "gemini"] {
        let ids = routes[provider]["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item["value"].as_str())
            .filter(|id| !id.is_empty())
            .collect::<Vec<_>>();
        models.insert(provider.to_string(), serde_json::json!(ids));
    }
    serde_json::json!({ "routes": routes, "models": models })
}

async fn chat_route_catalog(
    state: &AppState,
    user_id: &str,
    provider: Provider,
    runtime: ChatRuntime,
    refresh: bool,
) -> Value {
    let status = cached_provider_readiness(provider, refresh).await;
    chat_route_catalog_with_status(state, user_id, provider, runtime, status).await
}

async fn chat_route_catalog_with_status(
    state: &AppState,
    user_id: &str,
    provider: Provider,
    runtime: ChatRuntime,
    status: Value,
) -> Value {
    let mut problem = None;
    let mut models = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut values = Vec::new();
    let mut gateway_available = false;
    if runtime == ChatRuntime::IoGateway {
        if provider == Provider::Gemini {
            problem = Some(chat_setup_problem(
                "chat_route_unavailable",
                "Gemini chat currently uses Native CLI. Choose Native CLI for Gemini.",
                "choose_route",
                None,
            ));
        } else if direct_ai_runtime_config_for_user(state, user_id, provider).is_some() {
            let config = chat_ai_config_for_user(state, user_id, provider);
            match fetch_direct_ai_models(&config).await {
                Ok(catalog) if !catalog.is_empty() => {
                    gateway_available = true;
                    for item in catalog {
                        if let Some(id) = item
                            .get("value")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| valid_chat_model_id(s))
                        {
                            push_chat_model(&mut models, &mut seen, id);
                        }
                    }
                    if models.is_empty() {
                        problem = Some(chat_setup_problem(
                            "gateway_catalog_empty",
                            "IO Gateway returned no usable models. Check gateway settings and Recheck.",
                            "gateway_settings",
                            None,
                        ));
                    }
                }
                Ok(_) => {
                    problem = Some(chat_setup_problem(
                        "gateway_catalog_empty",
                        "IO Gateway returned no models. Check gateway settings and Recheck.",
                        "gateway_settings",
                        None,
                    ))
                }
                Err(error) => {
                    problem = Some(chat_setup_problem(
                        if error.status == StatusCode::UNAUTHORIZED {
                            "gateway_auth_required"
                        } else {
                            "gateway_catalog_unavailable"
                        },
                        &error.body.error,
                        "gateway_settings",
                        None,
                    ))
                }
            }
        } else {
            problem = Some(chat_setup_problem(
                "gateway_configuration_required",
                "IO Gateway is disabled or missing its URL/key. Configure IO Gateway to continue this route.",
                "gateway_settings",
                None,
            ));
        }
    }
    if !status["installed"].as_bool().unwrap_or(false) {
        problem = Some(chat_setup_problem(
            "provider_cli_unavailable",
            "The provider CLI could not be verified on the Workbench server. Install or repair it for the service user, then Recheck. IO Gateway also needs this CLI.",
            "install",
            status["installUrl"].as_str(),
        ));
    } else if runtime == ChatRuntime::NativeCli {
        if !status["nativeReady"].as_bool().unwrap_or(false) {
            problem = Some(chat_setup_problem(
                "provider_auth_required",
                "The CLI is installed, but native authentication could not be verified. Follow the official authentication guide on the server, or choose IO Gateway.",
                "authenticate",
                status["authUrl"].as_str(),
            ));
        } else {
            if provider == Provider::Codex {
                if let Some(model) = configured_codex_model()
                    .await
                    .filter(|s| is_local_codex_cli_model(s))
                {
                    push_chat_model(&mut models, &mut seen, model);
                }
                for model in cached_codex_models()
                    .await
                    .into_iter()
                    .filter(|s| is_local_codex_cli_model(s))
                {
                    push_chat_model(&mut models, &mut seen, model);
                }
            } else if provider == Provider::Claude {
                // Stable CLI aliases; account entitlements are resolved by the
                // CLI. Do not fabricate a list of dated, potentially retired IDs.
                for model in ["sonnet", "opus", "haiku"] {
                    push_chat_model(&mut models, &mut seen, model);
                }
            }
            values.push(serde_json::json!({"value": "", "label": "Native CLI default"}));
        }
    }
    if problem.is_none() {
        values.extend(
            models
                .into_iter()
                .map(|id| serde_json::json!({"value": id, "label": id})),
        );
    } else {
        values.clear();
    }
    serde_json::json!({ "success": true, "provider": provider.as_str(), "runtime": runtime,
        "ready": problem.is_none(), "readiness": status, "problem": problem,
        "gatewayAvailable": gateway_available, "models": values,
        "verification": if runtime == ChatRuntime::IoGateway { "Catalog connectivity checked; individual model execution is not verified." } else { "Local CLI status; provider credentials can expire." } })
}

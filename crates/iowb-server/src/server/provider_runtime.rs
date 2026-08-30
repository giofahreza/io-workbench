fn validate_direct_ai_config(config: &Value) -> Result<()> {
    let allowed_modes = ["off", "proxy", "direct", "anthropic", "minimax", "aiproxy"];
    if let Some(mode) = config.get("mode").and_then(Value::as_str) {
        if !allowed_modes.contains(&mode) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "invalid IO Gateway mode",
            ));
        }
    }
    if let Some(runtime) = config
        .get("chatRuntime")
        .or_else(|| config.get("chat_runtime"))
        .and_then(Value::as_str)
        && parse_chat_runtime(runtime).is_none()
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "invalid chat runtime",
        ));
    }
    Ok(())
}

fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Invalid sessionId",
        ));
    }
    Ok(())
}

fn validate_provider_name(provider: &str) -> Result<()> {
    parse_provider_param(provider).map(|_| ())
}

fn parse_provider_param(provider: &str) -> Result<Provider> {
    match provider {
        "claude" => Ok(Provider::Claude),
        "codex" => Ok(Provider::Codex),
        "gemini" => Ok(Provider::Gemini),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Provider must be one of: claude, codex, gemini",
        )),
    }
}

fn stored_git_alias(stored: &Option<Value>, key: &str) -> Option<String> {
    stored
        .as_ref()
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn blocking_git_config(key: &str) -> Option<String> {
    std::process::Command::new("git")
        .args(["config", "--global", "--get", key])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn set_git_global_config(key: &str, value: &str) -> std::io::Result<()> {
    let status = Command::new("git")
        .args(["config", "--global", key, value])
        .status()
        .await?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "git config exited with status {status}"
        )))
    }
}

fn looks_like_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

async fn provider_cli_status(provider: Provider) -> Value {
    let command = provider_command(provider);
    let version = command_version(&command).await;
    let installed = version.is_some();
    let auth = provider_auth_hint(provider);
    let authenticated = auth.is_some();
    let error = if installed {
        (!authenticated).then_some("Not authenticated")
    } else {
        Some("CLI not installed or not in PATH")
    };

    serde_json::json!({
        "authenticated": authenticated,
        "email": auth.as_deref(),
        "method": auth_method(provider, auth.as_deref()),
        "error": error,
        "installed": installed,
        "command": command,
        "version": version,
    })
}

fn provider_command(provider: Provider) -> String {
    let command = match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::Gemini => "gemini",
    };
    preferred_user_command(command).unwrap_or_else(|| command.to_string())
}

fn preferred_user_command(command: &str) -> Option<String> {
    let home = home_dir()?;
    let candidate = home.join(".local").join("bin").join(command);
    if candidate.is_file() {
        Some(candidate.display().to_string())
    } else {
        None
    }
}

async fn command_version(command: &str) -> Option<String> {
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        Command::new(command)
            .arg("--version")
            .env("PATH", augmented_user_path())
            .output(),
    )
    .await;
    let output = match result {
        Ok(Ok(output)) if output.status.success() => output,
        _ => return None,
    };
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    [stdout, stderr].into_iter().find(|value| !value.is_empty())
}

fn provider_auth_hint(provider: Provider) -> Option<String> {
    let env_key = match provider {
        Provider::Claude => ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"]
            .into_iter()
            .find(|key| env_has_value(key)),
        Provider::Codex => ["OPENAI_API_KEY"]
            .into_iter()
            .find(|key| env_has_value(key)),
        Provider::Gemini => ["GEMINI_API_KEY", "GOOGLE_API_KEY"]
            .into_iter()
            .find(|key| env_has_value(key)),
    };
    if let Some(env_key) = env_key {
        return Some(format!("API Key Auth ({env_key})"));
    }

    let home = home_dir()?;
    let configured = match provider {
        Provider::Claude => home.join(".claude").join(".credentials.json").is_file(),
        Provider::Codex => home.join(".codex").join("auth.json").is_file(),
        Provider::Gemini => home.join(".gemini").join("oauth_creds.json").is_file(),
    };
    configured.then(|| "Configured on disk".to_string())
}

fn auth_method(provider: Provider, auth: Option<&str>) -> Option<&'static str> {
    auth.map(|auth| {
        if auth.starts_with("API Key Auth") {
            "api_key"
        } else {
            match provider {
                Provider::Claude | Provider::Codex | Provider::Gemini => "credentials_file",
            }
        }
    })
}

fn env_has_value(key: &str) -> bool {
    env::var(key)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

async fn claude_token_usage(project_path: &str, session_id: &str) -> Result<TokenUsageSnapshot> {
    let home = home_dir()
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "home directory not found"))?;
    let encoded_project = encode_claude_project_path(project_path);
    let session_file = home
        .join(".claude")
        .join("projects")
        .join(encoded_project)
        .join(format!("{session_id}.jsonl"));

    let content = tokio::fs::read_to_string(&session_file)
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ServerError::with_details(
                    StatusCode::NOT_FOUND,
                    "Session file not found",
                    session_file.display().to_string(),
                )
            } else {
                ServerError::with_details(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to read session token usage",
                    error.to_string(),
                )
            }
        })?;

    let total = env::var("CONTEXT_WINDOW")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(160_000);

    Ok(TokenUsageSnapshot {
        usage: parse_claude_usage(&content),
        total,
    })
}

async fn codex_token_usage(session_file: &Path) -> Result<TokenUsageSnapshot> {
    let metadata = tokio::fs::metadata(session_file).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ServerError::with_details(
                StatusCode::NOT_FOUND,
                "Session file not found",
                session_file.display().to_string(),
            )
        } else {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to inspect session token usage",
                error.to_string(),
            )
        }
    })?;
    let file_len = metadata.len();
    let modified_at = metadata.modified().ok();
    let path = session_file.to_path_buf();
    let cache = CODEX_TOKEN_USAGE_CACHE
        .get_or_init(|| tokio::sync::Mutex::new(CodexTokenUsageCache::default()));
    let file_lock = {
        let mut cache = cache.lock().await;
        if let Some(cached) = cache
            .entries
            .get_mut(session_file)
            .filter(|cached| cached.file_len == file_len && cached.modified_at == modified_at)
        {
            cached.last_access = Instant::now();
            return Ok(cached.snapshot.clone());
        }
        cache
            .file_locks
            .entry(path.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };

    // Serialize reads of the same rollout while allowing unrelated sessions
    // to load concurrently. Recheck after waiting so all duplicate requests
    // reuse the first completed read.
    let _file_guard = file_lock.lock().await;
    {
        let mut cache = cache.lock().await;
        if let Some(cached) = cache
            .entries
            .get_mut(session_file)
            .filter(|cached| cached.file_len == file_len && cached.modified_at == modified_at)
        {
            cached.last_access = Instant::now();
            return Ok(cached.snapshot.clone());
        }
    }

    let read_path = path.clone();
    let content = tokio::task::spawn_blocking(move || {
        read_file_tail(&read_path, CODEX_TOKEN_USAGE_TAIL_MAX_BYTES)
    })
    .await
    .map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to read session token usage",
            error.to_string(),
        )
    })?
    .map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to read session token usage",
            error.to_string(),
        )
    })?;
    let snapshot = parse_codex_usage(&content);
    let mut cache = cache.lock().await;
    if cache.entries.len() >= CODEX_TOKEN_USAGE_CACHE_MAX_ENTRIES
        && !cache.entries.contains_key(&path)
    {
        if let Some(oldest) = cache
            .entries
            .iter()
            .min_by_key(|(_, cached)| cached.last_access)
            .map(|(path, _)| path.clone())
        {
            cache.entries.remove(&oldest);
            cache.file_locks.remove(&oldest);
        }
    }
    cache.entries.insert(
        path,
        CachedCodexTokenUsage {
            file_len,
            modified_at,
            last_access: Instant::now(),
            snapshot: snapshot.clone(),
        },
    );
    Ok(snapshot)
}

fn read_file_tail(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let start = file_len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(file_len.saturating_sub(start) as usize);
    file.read_to_end(&mut bytes)?;
    if start > 0 {
        if let Some(first_newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=first_newline);
        } else {
            bytes.clear();
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn token_usage_response(snapshot: &TokenUsageSnapshot) -> Value {
    serde_json::json!({
        "used": snapshot.usage.used,
        "total": snapshot.total,
        "breakdown": {
            "input": snapshot.usage.input,
            "output": snapshot.usage.output,
            "cacheCreation": snapshot.usage.cache_creation,
            "cacheRead": snapshot.usage.cache_read,
        },
        "costUsd": snapshot.usage.cost_usd,
    })
}

fn encode_claude_project_path(project_path: &str) -> String {
    project_path
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn parse_claude_usage(content: &str) -> SessionTokenUsage {
    let mut usage_by_message = HashMap::new();
    for (line_index, line) in content.lines().enumerate() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let input = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_creation = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_read = usage
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let key = message
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("line-{line_index}"));
        usage_by_message.insert(
            key,
            SessionTokenUsage {
                used: input
                    .saturating_add(output)
                    .saturating_add(cache_creation)
                    .saturating_add(cache_read),
                input,
                output,
                cache_creation,
                cache_read,
                reasoning: 0,
                cost_usd: usage
                    .get("cost_usd")
                    .or_else(|| usage.get("costUsd"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
            },
        );
    }

    usage_by_message
        .into_values()
        .fold(SessionTokenUsage::default(), |mut total, usage| {
            total.used = total.used.saturating_add(usage.used);
            total.input = total.input.saturating_add(usage.input);
            total.output = total.output.saturating_add(usage.output);
            total.cache_creation = total.cache_creation.saturating_add(usage.cache_creation);
            total.cache_read = total.cache_read.saturating_add(usage.cache_read);
            total.cost_usd += usage.cost_usd;
            total
        })
}

fn parse_codex_usage(content: &str) -> TokenUsageSnapshot {
    for line in content.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(info) = value
            .get("payload")
            .filter(|_| value.get("type").and_then(Value::as_str) == Some("event_msg"))
            .and_then(|payload| {
                (payload.get("type").and_then(Value::as_str) == Some("token_count"))
                    .then_some(payload)
            })
            .and_then(|payload| payload.get("info"))
        else {
            continue;
        };

        let usage = info.get("total_token_usage").unwrap_or(info);
        let input = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let total = info
            .get("model_context_window")
            .and_then(Value::as_u64)
            .unwrap_or(200_000);
        return TokenUsageSnapshot {
            usage: SessionTokenUsage {
                used: usage
                    .get("total_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_else(|| input.saturating_add(output)),
                input,
                output,
                cache_creation: usage
                    .get("cache_write_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cache_read: usage
                    .get("cached_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                reasoning: usage
                    .get("reasoning_output_tokens")
                    .or_else(|| usage.get("reasoning_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cost_usd: usage
                    .get("cost_usd")
                    .or_else(|| usage.get("costUsd"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0),
            },
            total,
        };
    }
    TokenUsageSnapshot {
        usage: SessionTokenUsage::default(),
        total: 200_000,
    }
}

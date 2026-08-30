fn effective_agent_command_provider(provider: Provider, model: Option<&str>) -> Provider {
    if model.is_some_and(uses_codex_aiproxy_cli_runtime) {
        Provider::Codex
    } else {
        provider
    }
}

fn should_use_direct_ai_gateway_runtime(provider: Provider, model: Option<&str>) -> bool {
    match provider {
        // Claude provider runs gateway-prefixed models through Claude Code CLI
        // so the session keeps workspace tools. Use unprefixed aliases such as
        // `sonnet`, `opus`, `haiku`, or `fable` for the local Claude config.
        Provider::Claude => false,
        Provider::Gemini => model.is_some_and(|model| {
            looks_like_proxy_model(model) && !uses_codex_aiproxy_cli_runtime(model)
        }),
        Provider::Codex => false,
    }
}

fn runtime_label(runtime: ChatRuntime) -> &'static str {
    match runtime {
        ChatRuntime::NativeCli => "native_cli",
        ChatRuntime::IoGateway => "io_gateway",
    }
}

fn validate_recovered_agent_runtime_config(
    provider: Provider,
    runtime: ChatRuntime,
    direct_ai_config: Option<&DirectAiRuntimeConfig>,
) -> Result<()> {
    if provider == Provider::Codex
        && runtime == ChatRuntime::IoGateway
        && direct_ai_config.is_none()
    {
        return Err(CoreError::InvalidInput(
            "IO Gateway is not configured for this session".to_string(),
        ));
    }
    Ok(())
}

fn runtime_status_label(status: iowb_protocol::SessionRuntimeStatus) -> &'static str {
    match status {
        iowb_protocol::SessionRuntimeStatus::Starting => "starting",
        iowb_protocol::SessionRuntimeStatus::Running => "running",
        iowb_protocol::SessionRuntimeStatus::WaitingForInput => "waiting_for_input",
        iowb_protocol::SessionRuntimeStatus::Completed => "completed",
        iowb_protocol::SessionRuntimeStatus::Aborted => "aborted",
        iowb_protocol::SessionRuntimeStatus::Failed => "failed",
    }
}

fn uses_codex_aiproxy_cli_runtime(model: &str) -> bool {
    gateway_model_prefix(model).is_some_and(|prefix| prefix == "cod")
}

#[cfg(test)]
fn uses_claude_aiproxy_cli_runtime(model: &str) -> bool {
    let Some(prefix) = gateway_model_prefix(model) else {
        return false;
    };
    prefix != "cod"
}

#[cfg(test)]
fn should_force_claude_cli_io_gateway(provider: Provider, model: Option<&str>) -> bool {
    provider == Provider::Claude && model.is_some_and(uses_claude_aiproxy_cli_runtime)
}

#[cfg(test)]
fn should_force_codex_cli_io_gateway(model: Option<&str>) -> bool {
    model.is_some_and(looks_like_proxy_model)
}

fn legacy_chat_runtime(model: Option<&str>) -> ChatRuntime {
    if model.is_some_and(looks_like_proxy_model) {
        ChatRuntime::IoGateway
    } else {
        ChatRuntime::NativeCli
    }
}

fn default_agent_command(provider: Provider) -> String {
    let command = match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::Gemini => "gemini",
    };
    preferred_user_command(command).unwrap_or_else(|| command.to_string())
}

fn configured_codex_command() -> String {
    env::var("IO_WORKBENCH_CODEX_COMMAND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_agent_command(Provider::Codex))
}

fn default_codex_app_server_client() -> CodexAppServerClient {
    CodexAppServerClient::new(
        configured_codex_command(),
        Duration::from_secs(
            env::var("IO_WORKBENCH_CODEX_APP_SERVER_TIMEOUT_SECS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(15)
                .clamp(1, 120),
        ),
    )
}

fn preferred_user_command(command: &str) -> Option<String> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    let candidate = home.join(".local").join("bin").join(command);
    if candidate.is_file() {
        Some(candidate.display().to_string())
    } else {
        None
    }
}

pub fn augmented_user_path() -> OsString {
    let original = env::var_os("PATH").unwrap_or_default();
    let mut paths = Vec::<PathBuf>::new();
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        for candidate in [
            home.join(".local/bin"),
            home.join(".volta/bin"),
            home.join(".fnm/current/bin"),
            home.join(".asdf/shims"),
            home.join(".local/share/mise/shims"),
            home.join(".bun/bin"),
        ] {
            push_unique_directory(&mut paths, candidate);
        }

        let mut nvm_node_bins = std::fs::read_dir(home.join(".nvm/versions/node"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path().join("bin"))
            .filter(|path| path.join("node").is_file())
            .collect::<Vec<_>>();
        nvm_node_bins.sort_by(|left, right| {
            node_version_components(right).cmp(&node_version_components(left))
        });
        for candidate in nvm_node_bins {
            push_unique_directory(&mut paths, candidate);
        }
    }
    for path in env::split_paths(&original) {
        push_unique_directory(&mut paths, path);
    }
    env::join_paths(paths).unwrap_or(original)
}

fn push_unique_directory(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.is_dir() && !paths.contains(&candidate) {
        paths.push(candidate);
    }
}

fn node_version_components(path: &Path) -> Vec<u64> {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse().unwrap_or_default())
        .collect()
}

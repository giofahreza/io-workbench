#[cfg(test)]
fn default_agent_args_with(
    provider: Provider,
    prompt: &str,
    mode: Option<&str>,
    effort: Option<&str>,
    thinking: Option<bool>,
    model: Option<&str>,
) -> Vec<String> {
    let runtime = if model.is_some_and(looks_like_proxy_model) {
        ChatRuntime::IoGateway
    } else {
        ChatRuntime::NativeCli
    };
    default_agent_args_with_resume(
        provider, prompt, mode, effort, thinking, None, model, None, runtime,
    )
}

#[allow(clippy::too_many_arguments)]
fn default_agent_args_with_resume(
    provider: Provider,
    prompt: &str,
    mode: Option<&str>,
    effort: Option<&str>,
    thinking: Option<bool>,
    fast: Option<bool>,
    model: Option<&str>,
    resume_session_id: Option<&str>,
    runtime: ChatRuntime,
) -> Vec<String> {
    let mut args: Vec<String> = match provider {
        Provider::Claude => {
            // Use NDJSON streaming so the chat UI receives partial output the
            // way it does for Codex. Claude requires `--print` for
            // `--output-format stream-json`, and partial assistant deltas only
            // show up when `--include-partial-messages` is enabled.
            let mut args = vec![
                "--print".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--verbose".to_string(),
                "--include-partial-messages".to_string(),
            ];
            if runtime == ChatRuntime::IoGateway {
                args.push("--setting-sources".to_string());
                args.push("project,local".to_string());
            }
            args
        }
        // Codex exec options must precede the `resume` subcommand. The prompt
        // and resume arguments are appended after all shared options below.
        Provider::Codex => vec!["exec".to_string(), "--json".to_string()],
        Provider::Gemini => {
            let mut args = vec![
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--prompt".to_string(),
                prompt.to_string(),
            ];
            if let Some(session_id) = resume_session_id {
                args.extend(["--resume".to_string(), session_id.to_string()]);
            }
            args
        }
    };
    // Always relax Codex's directory / git repo enforcement so it can run in
    // arbitrary project directories, which is the common case for an embedded
    // workspace UI. Native runs inherit the CLI provider configuration; IO
    // Gateway runs receive an isolated provider definition below.
    if matches!(provider, Provider::Codex) {
        args.push("--skip-git-repo-check".to_string());
        if runtime == ChatRuntime::IoGateway {
            push_codex_provider_override(&mut args, "iowb_gateway");
        }
    }
    // Per-mode flags (Claude supports --permission-mode; Codex exec uses
    // --sandbox / --dangerously-bypass-approvals-and-sandbox). Provider-specific.
    if let Some(mode) = mode {
        match normalize_agent_mode(mode).as_deref() {
            Some("bypass") => match provider {
                Provider::Claude => {
                    args.push("--permission-mode".to_string());
                    args.push("bypassPermissions".to_string());
                    args.push("--dangerously-skip-permissions".to_string());
                    args.push("--tools".to_string());
                    args.push("default".to_string());
                }
                Provider::Codex => {
                    args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
                }
                Provider::Gemini => {
                    args.push("--yolo".to_string());
                }
            },
            Some("accept-edits") => match provider {
                Provider::Claude => {
                    args.push("--permission-mode".to_string());
                    args.push("acceptEdits".to_string());
                }
                Provider::Codex => {
                    args.push("--sandbox".to_string());
                    args.push("workspace-write".to_string());
                }
                Provider::Gemini => {}
            },
            Some("plan") => match provider {
                Provider::Claude => {
                    args.push("--permission-mode".to_string());
                    args.push("plan".to_string());
                }
                Provider::Codex => {
                    args.push("--sandbox".to_string());
                    args.push("read-only".to_string());
                }
                Provider::Gemini => {}
            },
            Some("read-only") => match provider {
                Provider::Claude => {
                    args.push("--permission-mode".to_string());
                    args.push("readonly".to_string());
                }
                Provider::Codex => {
                    args.push("--sandbox".to_string());
                    args.push("read-only".to_string());
                }
                Provider::Gemini => {}
            },
            _ => {}
        }
    }
    // The user-selected model is independent from reasoning effort so the
    // dropdown value the user actually picked is what gets sent. Native Codex
    // can omit stale legacy model ids and inherit the model from its own config.
    let selected_model = model.map(str::trim).filter(|value| !value.is_empty());
    if let Some(trimmed) = selected_model
        && !args.iter().any(|a| a == "--model")
        && let Some(cli_model) = agent_cli_model_arg(provider, trimmed)
    {
        args.push("--model".to_string());
        args.push(cli_model);
    }
    if matches!(provider, Provider::Codex) {
        if let Some(reasoning_effort) =
            effective_codex_reasoning_effort(effort, thinking.unwrap_or(false))
        {
            push_codex_config_override(
                &mut args,
                "model_reasoning_effort",
                &format!("\"{reasoning_effort}\""),
            );
        }
        if let Some(fast) = fast {
            push_codex_config_override(&mut args, "features.fast_mode", "true");
            push_codex_config_override(
                &mut args,
                "service_tier",
                if fast { "\"fast\"" } else { "\"default\"" },
            );
        }
    } else if let Some(effort) = effort
        && matches!(provider, Provider::Claude)
    {
        args.push("--effort".to_string());
        args.push(effort.to_string());
    }
    if thinking.unwrap_or(false) && matches!(provider, Provider::Gemini) {
        args.push("--thinking".to_string());
    }
    if matches!(provider, Provider::Codex) {
        if let Some(session_id) = resume_session_id {
            args.extend(["resume".to_string(), session_id.to_string()]);
        }
        args.push(prompt.to_string());
    } else if matches!(provider, Provider::Claude) {
        if let Some(session_id) = resume_session_id {
            args.extend(["--resume".to_string(), session_id.to_string()]);
        }
        args.push(prompt.to_string());
    }
    args
}

fn effective_codex_reasoning_effort(effort: Option<&str>, thinking: bool) -> Option<&str> {
    let selected = effort.and_then(|value| match value {
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra" => Some(value),
        _ => None,
    });
    if thinking && !matches!(selected, Some("xhigh" | "max" | "ultra")) {
        Some("xhigh")
    } else {
        selected
    }
}

/// Returns true when a model id belongs to the IO gateway namespace.
fn looks_like_proxy_model(model: &str) -> bool {
    gateway_model_prefix(model).is_some()
}

fn gateway_model_prefix(model: &str) -> Option<String> {
    let trimmed = model.trim();
    let (prefix, rest) = trimmed.split_once(':')?;
    let normalized = prefix.to_ascii_lowercase();
    if prefix.len() < 2
        || prefix.len() > 12
        || rest.trim().is_empty()
        || !prefix
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        return None;
    }
    Some(match normalized.as_str() {
        "agw" | "cod" | "proxy" | "gateway" | "aiproxy" | "cld" | "gem" | "cop" | "ctm" | "dsk"
        | "glm" | "grk" | "min" => normalized,
        _ => return None,
    })
}

fn agent_cli_model_arg(provider: Provider, model: &str) -> Option<String> {
    if provider == Provider::Codex {
        normalize_codex_cli_model(model).map(str::to_string)
    } else {
        Some(model.to_string())
    }
}

fn normalize_codex_cli_model(model: &str) -> Option<&str> {
    let trimmed = model.trim();
    if is_local_codex_cli_model(trimmed) || looks_like_proxy_model(trimmed) {
        Some(trimmed)
    } else {
        None
    }
}

fn is_local_codex_cli_model(model: &str) -> bool {
    let trimmed = model.trim();
    !trimmed.is_empty()
        && !trimmed.contains(':')
        && gateway_model_prefix(trimmed).is_none()
        && !trimmed.eq_ignore_ascii_case("gpt-5-codex")
}

fn normalize_agent_mode(mode: &str) -> Option<&'static str> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "bypass" | "bypass-permissions" | "bypasspermissions" | "danger" | "no-approvals"
        | "no_approvals" => Some("bypass"),
        "accept-edits" | "acceptedits" | "accept" => Some("accept-edits"),
        "plan" | "plan-only" => Some("plan"),
        "read-only" | "readonly" | "read" => Some("read-only"),
        "default" | "" => Some("default"),
        _ => None,
    }
}

fn apply_claude_cli_io_gateway_env(command: &mut Command, config: &DirectAiRuntimeConfig) {
    command
        .env("ANTHROPIC_BASE_URL", config.base_url.trim_end_matches('/'))
        .env("ANTHROPIC_API_KEY", &config.api_key)
        .env("ANTHROPIC_AUTH_TOKEN", &config.api_key)
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN");
}

fn apply_codex_cli_io_gateway_args(args: &mut Vec<String>, base_url: &str) {
    push_codex_config_override(args, "model_provider", "iowb_gateway");
    push_codex_config_override(args, "model_providers.iowb_gateway.name", "\"IO Gateway\"");
    push_codex_config_override(
        args,
        "model_providers.iowb_gateway.base_url",
        &format!("{:?}", base_url.trim_end_matches('/')),
    );
    push_codex_config_override(
        args,
        "model_providers.iowb_gateway.env_key",
        &format!("\"{IO_WORKBENCH_GATEWAY_KEY_ENV}\""),
    );
    push_codex_config_override(
        args,
        "model_providers.iowb_gateway.wire_api",
        "\"responses\"",
    );
}

fn codex_app_server_launch_options(
    runtime: ChatRuntime,
    config: Option<&DirectAiRuntimeConfig>,
) -> Result<Option<CodexAppServerLaunchOptions>> {
    if runtime != ChatRuntime::IoGateway {
        return Ok(None);
    }
    let config = config.ok_or_else(|| {
        CoreError::InvalidInput(
            "IO Gateway is not configured for Codex context compaction".to_string(),
        )
    })?;
    let mut args = vec!["app-server".to_string()];
    apply_codex_cli_io_gateway_args(&mut args, &config.base_url);
    args.push("--stdio".to_string());
    Ok(Some(CodexAppServerLaunchOptions {
        args,
        env: vec![(
            IO_WORKBENCH_GATEWAY_KEY_ENV.to_string(),
            config.api_key.clone(),
        )],
    }))
}

fn codex_app_server_live_enabled(context: &AgentStartContext) -> bool {
    codex_app_server_live_enabled_for(
        context.provider,
        context.runtime,
        env_bool(CODEX_APP_SERVER_LIVE_ENV, false),
        env_bool(CODEX_APP_SERVER_LIVE_IO_GATEWAY_ENV, false),
        codex_app_server_live_cli_override_configured(),
    )
}

fn codex_app_server_live_enabled_for(
    provider: Provider,
    runtime: ChatRuntime,
    live_enabled: bool,
    io_gateway_enabled: bool,
    cli_override_configured: bool,
) -> bool {
    if provider != Provider::Codex || !live_enabled {
        return false;
    }
    if cli_override_configured {
        return false;
    }
    match runtime {
        ChatRuntime::NativeCli => true,
        ChatRuntime::IoGateway => io_gateway_enabled,
    }
}

fn codex_app_server_live_cli_override_configured() -> bool {
    [
        "IO_WORKBENCH_CODEX_ARGS_JSON",
        "IO_WORKBENCH_AGENT_ARGS_JSON",
    ]
    .into_iter()
    .any(env_var_nonempty)
        || env_bool("IO_WORKBENCH_CODEX_STDIN", false)
        || env_bool("IO_WORKBENCH_AGENT_STDIN", false)
}

fn env_var_nonempty(name: &str) -> bool {
    env::var(name)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

fn codex_app_server_live_turn_params(context: &AgentStartContext) -> CodexAppServerLiveTurnParams {
    let selected_model = context
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|model| agent_cli_model_arg(Provider::Codex, model));
    let effort = effective_codex_reasoning_effort(
        context.effort.as_deref(),
        context.thinking.unwrap_or(false),
    )
    .map(str::to_string);
    let service_tier = context.fast.map(|fast| {
        if fast {
            "fast".to_string()
        } else {
            "default".to_string()
        }
    });
    let client_user_message_id = context
        .durable_run_id
        .as_deref()
        .and_then(|run_id| context.storage.get_durable_chat_run(run_id).ok().flatten())
        .and_then(|run| run.user_message_id);
    CodexAppServerLiveTurnParams {
        thread_id: context.native_resume_session_id.clone(),
        cwd: context.project_path.clone(),
        input: codex_app_server_prompt_input(&context.prompt, &context.project_path),
        client_user_message_id,
        model: selected_model,
        effort,
        service_tier,
        approval_policy: Some(serde_json::json!("never")),
        sandbox_policy: codex_app_server_sandbox_policy(
            context.mode.as_deref(),
            &context.project_path,
        ),
    }
}

fn codex_app_server_sandbox_policy(mode: Option<&str>, project_path: &Path) -> Option<Value> {
    match mode.and_then(normalize_agent_mode) {
        Some("bypass") => Some(serde_json::json!({ "type": "dangerFullAccess" })),
        Some("accept-edits") => Some(serde_json::json!({
            "type": "workspaceWrite",
            "writableRoots": [project_path.display().to_string()],
            "networkAccess": false,
        })),
        Some("plan") | Some("read-only") => Some(serde_json::json!({
            "type": "readOnly",
            "networkAccess": false,
        })),
        _ => None,
    }
}

fn codex_app_server_prompt_input(prompt: &str, project_path: &Path) -> Vec<Value> {
    let mut text_lines = Vec::new();
    let mut image_paths = Vec::new();
    for line in prompt.lines() {
        if let Some(marker_path) = parse_attached_image_marker(line)
            && let Some(path) = resolve_prompt_local_image_path(marker_path, project_path)
        {
            image_paths.push(path);
            continue;
        }
        text_lines.push(line);
    }
    let mut input = Vec::new();
    let text = text_lines.join("\n").trim().to_string();
    if !text.is_empty() {
        input.push(serde_json::json!({ "type": "text", "text": text }));
    }
    for path in image_paths {
        input.push(serde_json::json!({
            "type": "localImage",
            "path": path.display().to_string(),
        }));
    }
    if input.is_empty() {
        input.push(serde_json::json!({ "type": "text", "text": prompt.trim() }));
    }
    input
}

fn parse_attached_image_marker(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("Attached image file: `")?;
    let (path, _) = rest.split_once('`')?;
    let path = path.trim();
    (!path.is_empty()).then_some(path)
}

fn resolve_prompt_local_image_path(path: &str, project_path: &Path) -> Option<PathBuf> {
    let candidate = PathBuf::from(path);
    let candidate = if candidate.is_absolute() {
        candidate
    } else {
        project_path.join(candidate)
    };
    let project = std::fs::canonicalize(project_path).ok()?;
    let candidate = std::fs::canonicalize(candidate).ok()?;
    if candidate.is_file() && candidate.starts_with(project) {
        Some(candidate)
    } else {
        None
    }
}

/// Keep `-c key=value` overrides before the `resume` positional so Codex
/// applies the ephemeral provider configuration to resumed turns.
fn push_codex_provider_override(args: &mut Vec<String>, provider: &str) {
    push_codex_config_override(args, "model_provider", provider);
}

fn push_codex_config_override(args: &mut Vec<String>, key: &str, value: &str) {
    let needle = format!("{key}=");
    if let Some(index) = args
        .windows(2)
        .position(|pair| pair[0] == "-c" && pair[1].starts_with(&needle))
    {
        args[index + 1] = format!("{key}={value}");
    } else {
        let insert_at = args
            .iter()
            .position(|arg| arg == "resume")
            .unwrap_or(args.len());
        args.splice(
            insert_at..insert_at,
            ["-c".to_string(), format!("{key}={value}")],
        );
    }
}

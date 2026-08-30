#[allow(clippy::too_many_arguments)]
fn resolve_agent_command(
    provider: Provider,
    prompt: &str,
    session_id: &str,
    model: Option<&str>,
    effort: Option<&str>,
    mode: Option<&str>,
    thinking: Option<bool>,
    fast: Option<bool>,
    native_resume_session_id: Option<&str>,
    runtime: ChatRuntime,
) -> Result<AgentCommandSpec> {
    let command_provider = if runtime == ChatRuntime::IoGateway {
        provider
    } else {
        effective_agent_command_provider(provider, model)
    };
    let provider_prefix = format!(
        "IO_WORKBENCH_{}_",
        command_provider.as_str().to_ascii_uppercase()
    );
    let command = env::var(format!("{provider_prefix}COMMAND"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("IO_WORKBENCH_AGENT_COMMAND")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| default_agent_command(command_provider));

    let args_template = env::var(format!("{provider_prefix}ARGS_JSON"))
        .ok()
        .or_else(|| env::var("IO_WORKBENCH_AGENT_ARGS_JSON").ok());
    let cli_prompt = resolve_cli_slash_prompt(command_provider, prompt)?;
    let args = if let Some(args_template) = args_template {
        let raw_args: Vec<String> = serde_json::from_str(&args_template).map_err(|error| {
            CoreError::InvalidInput(format!("invalid agent args JSON: {error}"))
        })?;
        raw_args
            .into_iter()
            .map(|arg| {
                expand_agent_template(
                    arg,
                    &cli_prompt,
                    session_id,
                    model,
                    native_resume_session_id,
                )
            })
            .collect()
    } else {
        default_agent_args_with_resume(
            command_provider,
            &cli_prompt,
            mode,
            effort,
            thinking,
            fast,
            model,
            native_resume_session_id,
            runtime,
        )
    };

    let stdin_prompt = env_bool(&format!("{provider_prefix}STDIN"), false)
        || env_bool("IO_WORKBENCH_AGENT_STDIN", false);

    Ok(AgentCommandSpec {
        command,
        args,
        stdin_prompt,
        prompt: cli_prompt,
    })
}

fn resolve_cli_slash_prompt(provider: Provider, prompt: &str) -> Result<String> {
    if provider != Provider::Codex || !prompt.trim_start().starts_with('/') {
        return Ok(prompt.to_string());
    }
    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
    resolve_codex_slash_prompt(prompt, codex_home.as_deref())
}

fn resolve_codex_slash_prompt(prompt: &str, codex_home: Option<&Path>) -> Result<String> {
    let trimmed = prompt.trim();
    let Some(command_token) = trimmed.split_whitespace().next() else {
        return Ok(prompt.to_string());
    };
    let arguments = trimmed
        .strip_prefix(command_token)
        .unwrap_or_default()
        .trim();

    if let Some(name) = command_token.strip_prefix("/prompts:") {
        let Some(codex_home) = codex_home else {
            return Err(CoreError::InvalidInput(
                "CODEX_HOME is unavailable for custom slash commands".to_string(),
            ));
        };
        return expand_codex_custom_prompt(codex_home, name, arguments);
    }

    let name = command_token.trim_start_matches('/');
    if valid_codex_extension_name(name)
        && codex_home.is_some_and(|home| codex_skill_exists(home, name))
    {
        return Ok(if arguments.is_empty() {
            format!("${name}")
        } else {
            format!("${name} {arguments}")
        });
    }

    Ok(prompt.to_string())
}

fn codex_skill_exists(codex_home: &Path, name: &str) -> bool {
    [
        codex_home.join("skills").join(name).join("SKILL.md"),
        codex_home
            .join("skills")
            .join(".system")
            .join(name)
            .join("SKILL.md"),
        codex_home
            .parent()
            .map(|home| {
                home.join(".agents")
                    .join("skills")
                    .join(name)
                    .join("SKILL.md")
            })
            .unwrap_or_default(),
    ]
    .iter()
    .any(|path| path.is_file())
}

fn expand_codex_custom_prompt(codex_home: &Path, name: &str, arguments: &str) -> Result<String> {
    if !valid_codex_extension_name(name) {
        return Err(CoreError::InvalidInput(format!(
            "invalid Codex slash command name: {name}"
        )));
    }
    let path = codex_home.join("prompts").join(format!("{name}.md"));
    let content = std::fs::read_to_string(&path).map_err(|error| {
        CoreError::InvalidInput(format!(
            "Codex custom slash command /prompts:{name} is unavailable: {error}"
        ))
    })?;
    let template = strip_markdown_frontmatter(&content);
    let values = parse_slash_arguments(arguments)?;
    let positional = values
        .iter()
        .filter(|value| !value.contains('='))
        .cloned()
        .collect::<Vec<_>>();
    let named = values
        .iter()
        .filter_map(|value| value.split_once('='))
        .filter(|(key, _)| {
            !key.is_empty()
                && key
                    .chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
        })
        .collect::<HashMap<_, _>>();

    let dollar_placeholder = "\u{0}IOWB_DOLLAR\u{0}";
    let mut expanded = template.replace("$$", dollar_placeholder);
    expanded = expanded.replace("$ARGUMENTS", arguments);
    for index in 1..=9 {
        expanded = expanded.replace(
            &format!("${index}"),
            positional.get(index - 1).map(String::as_str).unwrap_or(""),
        );
    }
    for (key, value) in named {
        expanded = expanded.replace(&format!("${key}"), value);
    }
    Ok(expanded.replace(dollar_placeholder, "$"))
}

fn valid_codex_extension_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

fn strip_markdown_frontmatter(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("---\n") else {
        return content;
    };
    rest.split_once("\n---\n")
        .map(|(_, body)| body)
        .unwrap_or(content)
}

fn parse_slash_arguments(arguments: &str) -> Result<Vec<String>> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in arguments.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                values.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if quote.is_some() {
        return Err(CoreError::InvalidInput(
            "slash command contains an unterminated quote".to_string(),
        ));
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        values.push(current);
    }
    Ok(values)
}

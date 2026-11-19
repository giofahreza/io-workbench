/// App-server has no --profile option. Share the non-secret model defaults
/// with exec using explicit overrides, below the choices made in chat.
fn apply_codex_profile_args(args: &mut Vec<String>, runtime: ChatRuntime) -> Result<()> {
    let home = dirs::home_dir();
    let codex_home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".codex")));
    let Some(codex_home) = codex_home else {
        return Ok(());
    };
    let selected = env::var("IO_WORKBENCH_CODEX_PROFILE").ok();
    let overrides =
        codex_profile_overrides(&codex_home, home.as_deref(), selected.as_deref(), runtime)?;
    insert_codex_profile_args(args, overrides);
    Ok(())
}

fn codex_profile_overrides(
    codex_home: &Path,
    home: Option<&Path>,
    selected: Option<&str>,
    runtime: ChatRuntime,
) -> Result<Vec<(String, String)>> {
    let name = selected.unwrap_or("default").trim();
    if name.is_empty() {
        return Ok(Vec::new());
    }
    if !name
        .bytes()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_'))
    {
        return Err(CoreError::InvalidInput(
            "invalid Codex profile name".to_string(),
        ));
    }
    let path = codex_home.join(format!("{name}.config.toml"));
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if selected.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(CoreError::InvalidInput(format!(
                "cannot read Codex profile {}: {error}",
                path.display()
            )));
        }
    };
    let table: toml::Table = toml::from_str(&content).map_err(|_| {
        CoreError::InvalidInput(format!("invalid TOML in Codex profile {}", path.display()))
    })?;
    let mut overrides = Vec::new();
    for key in [
        "model",
        "model_provider",
        "model_reasoning_effort",
        "model_reasoning_summary",
        "model_verbosity",
        "service_tier",
        "model_catalog_json",
        "model_context_window",
        "model_auto_compact_token_limit",
    ] {
        // Gateway authentication and routing belong to Workbench's gateway
        // configuration, even if this profile selects a different provider.
        if key == "model_provider" && runtime == ChatRuntime::IoGateway {
            continue;
        }
        let Some(value) = table.get(key) else {
            continue;
        };
        let mut value = value.clone();
        if key == "model_catalog_json"
            && let Some(raw_path) = value.as_str()
        {
            let catalog = if let Some(rest) = raw_path.strip_prefix("~/") {
                home.ok_or_else(|| {
                    CoreError::InvalidInput(
                        "cannot expand Codex catalog home directory".to_string(),
                    )
                })?
                .join(rest)
            } else {
                let candidate = PathBuf::from(raw_path);
                if candidate.is_absolute() {
                    candidate
                } else {
                    codex_home.join(candidate)
                }
            };
            value = toml::Value::String(catalog.display().to_string());
        }
        overrides.push((key.to_string(), value.to_string()));
    }
    Ok(overrides)
}

fn insert_codex_profile_args(args: &mut Vec<String>, overrides: Vec<(String, String)>) {
    let mut defaults = Vec::new();
    for (key, value) in overrides {
        if (key == "model" && args.iter().any(|arg| arg == "--model" || arg == "-m"))
            || args
                .windows(2)
                .any(|pair| pair[0] == "-c" && pair[1].starts_with(&format!("{key}=")))
        {
            continue;
        }
        defaults.extend(["-c".to_string(), format!("{key}={value}")]);
    }
    // Options must precede `resume` and its positional arguments.
    args.splice(1.min(args.len())..1.min(args.len()), defaults);
}

fn apply_codex_live_profile_defaults(
    params: &mut CodexAppServerLiveTurnParams,
    options: Option<&CodexAppServerLaunchOptions>,
) {
    let Some(options) = options else {
        return;
    };
    // Resuming restores saved thread settings that can supersede process
    // defaults. Forward the same defaults through the thread/turn API too.
    for (key, target) in [
        ("model", &mut params.model),
        ("model_provider", &mut params.model_provider),
        ("model_reasoning_effort", &mut params.effort),
        ("service_tier", &mut params.service_tier),
    ] {
        if target.is_some() {
            continue;
        }
        let Some(encoded) = options.args.windows(2).rev().find_map(|pair| {
            (pair[0] == "-c")
                .then(|| pair[1].split_once('='))
                .flatten()
                .filter(|(candidate, _)| *candidate == key)
                .map(|(_, value)| value)
        }) else {
            continue;
        };
        let parsed = toml::from_str::<toml::Table>(&format!("value={encoded}"));
        *target = match parsed {
            Ok(table) => table
                .get("value")
                .and_then(toml::Value::as_str)
                .map(str::to_string),
            // Codex accepts unquoted strings in -c overrides as a fallback.
            Err(_) => Some(encoded.to_string()),
        };
    }
}

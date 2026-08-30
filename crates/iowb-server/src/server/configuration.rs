fn current_git_config_overview(state: &AppState, user_id: &str) -> Result<Value> {
    let stored = state
        .storage
        .get_setting(&user_setting_key(user_id, "git-config"))?;
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
    let source = if stored.is_some() {
        "server-setting"
    } else if git_name.is_some() || git_email.is_some() {
        "git-global"
    } else {
        "unset"
    };

    Ok(serde_json::json!({
        "gitName": git_name,
        "gitEmail": git_email,
        "source": source,
    }))
}

fn default_claude_agent_settings() -> Value {
    serde_json::json!({
        "allowedTools": [],
        "disallowedTools": [],
        "skipPermissions": false,
        "providerMode": "anthropic",
        "aiProxyBaseUrl": "",
        "aiProxyApiKeyEnv": "",
        "minimaxBaseUrl": "https://api.minimax.io/anthropic",
        "minimaxApiKeyEnv": "MINIMAX_API_KEY",
        "minimaxModel": "MiniMax-M3"
    })
}

fn default_codex_agent_settings() -> Value {
    serde_json::json!({
        "permissionMode": "default"
    })
}

fn default_gemini_agent_settings() -> Value {
    serde_json::json!({
        "permissionMode": "default"
    })
}

fn default_appearance_settings() -> Value {
    serde_json::json!({
        "projectSortOrder": "name",
        "codeEditor": {
            "theme": "dark",
            "wordWrap": false,
            "showMinimap": true,
            "lineNumbers": true,
            "fontSize": "14"
        }
    })
}

fn default_tasks_settings() -> Value {
    serde_json::json!({
        "enabled": true,
        "runEndpoint": "/api/taskmaster/run",
        "commandsEndpoint": "/api/commands/run"
    })
}

fn default_notification_preferences() -> Value {
    serde_json::json!({
        "channels": {
            "inApp": true,
            "fcm": true,
            "webPush": false,
            "telegram": false,
            "googleChat": false
        },
        "telegram": {
            "botToken": "",
            "chatId": ""
        },
        "googleChat": {
            "webhookUrl": ""
        },
        "events": {
            "actionRequired": true,
            "stop": true,
            "error": true,
            "agenticBoardStarted": true,
            "agenticTaskUpdated": false,
            "agenticBoardCompleted": true,
            "agenticBoardNeedsAttention": true
        }
    })
}

fn default_direct_ai_config() -> Value {
    serde_json::json!({
        "mode": "off",
        "chatRuntime": "native_cli",
        "baseUrl": null,
        "apiKeyEnv": null,
        "model": null
    })
}

async fn read_json_file(path: &Path) -> Option<Value> {
    let content = read_text_path(path).await?;
    serde_json::from_str::<Value>(&content).ok()
}

async fn claude_mcp_config_overview(workspace_root: &Path) -> Value {
    let Some(home) = home_dir() else {
        return serde_json::json!({
            "success": false,
            "message": "home directory not found",
            "servers": []
        });
    };
    let config_paths = [
        home.join(".claude.json"),
        home.join(".claude").join("settings.json"),
    ];
    let mut config_data = None;
    let mut config_path = None;
    for path in config_paths {
        if let Some(value) = read_json_file(&path).await {
            config_data = Some(value);
            config_path = Some(path);
            break;
        }
    }
    let Some(config) = config_data else {
        return serde_json::json!({
            "success": false,
            "message": "No Claude configuration file found",
            "servers": []
        });
    };

    let mut servers = Vec::new();
    if let Some(root_servers) = config.get("mcpServers").and_then(Value::as_object) {
        servers.extend(mcp_servers_from_object(root_servers, "user", None));
    }
    let workspace_key = workspace_root.display().to_string();
    if let Some(project_servers) = config
        .get("projects")
        .and_then(Value::as_object)
        .and_then(|projects| projects.get(&workspace_key))
        .and_then(|project| project.get("mcpServers"))
        .and_then(Value::as_object)
    {
        servers.extend(mcp_servers_from_object(
            project_servers,
            "local",
            Some(workspace_key),
        ));
    }

    serde_json::json!({
        "success": true,
        "configPath": config_path.map(|path| path.display().to_string()),
        "servers": servers
    })
}

fn mcp_servers_from_object(
    servers: &serde_json::Map<String, Value>,
    scope: &str,
    project_path: Option<String>,
) -> Vec<Value> {
    servers
        .iter()
        .map(|(name, config)| mcp_server_record(name, config, scope, project_path.clone()))
        .collect()
}

fn mcp_server_record(
    name: &str,
    config: &Value,
    scope: &str,
    project_path: Option<String>,
) -> Value {
    let server_type = if config.get("command").is_some() {
        "stdio".to_string()
    } else {
        config
            .get("transport")
            .and_then(Value::as_str)
            .unwrap_or("http")
            .to_string()
    };
    let config_details = if server_type == "stdio" {
        serde_json::json!({
            "command": config.get("command").and_then(Value::as_str).unwrap_or_default(),
            "args": config.get("args").cloned().unwrap_or_else(|| serde_json::json!([])),
            "env": config.get("env").cloned().unwrap_or_else(|| serde_json::json!({})),
        })
    } else {
        serde_json::json!({
            "url": config.get("url").and_then(Value::as_str).unwrap_or_default(),
            "headers": config.get("headers").cloned().unwrap_or_else(|| serde_json::json!({})),
        })
    };
    serde_json::json!({
        "id": if scope == "local" { format!("local:{name}") } else { name.to_string() },
        "name": name,
        "type": server_type,
        "scope": scope,
        "projectPath": project_path,
        "config": config_details,
        "raw": config,
    })
}

async fn codex_config_overview() -> Value {
    let Some(home) = home_dir() else {
        return default_codex_config_overview(Value::Null);
    };
    let config_path = home.join(".codex").join("config.toml");
    let Some(content) = read_text_path(&config_path).await else {
        return default_codex_config_overview(Value::String(config_path.display().to_string()));
    };
    let top_values = parse_top_level_toml_values(&content);
    let model = top_values.get("model").cloned().unwrap_or(Value::Null);
    let reasoning_effort = top_values
        .get("model_reasoning_effort")
        .cloned()
        .unwrap_or(Value::Null);
    let approval_mode = top_values
        .get("approval_mode")
        .cloned()
        .unwrap_or_else(|| Value::String("suggest".to_string()));
    let profile_name = env::var("CODEX_PROFILE").unwrap_or_else(|_| "default".to_string());

    serde_json::json!({
        "success": true,
        "configPath": config_path.display().to_string(),
        "config": {
            "model": model,
            "profileModel": Value::Null,
            "resolvedModel": top_values.get("model").cloned().unwrap_or(Value::Null),
            "activeProfile": profile_name,
            "profiles": parse_toml_section_names(&content, "profiles"),
            "mcpServers": codex_mcp_servers_map(&content),
            "approvalMode": approval_mode,
            "modelReasoningEffort": reasoning_effort,
        }
    })
}

fn default_codex_config_overview(config_path: Value) -> Value {
    serde_json::json!({
        "success": true,
        "configPath": config_path,
        "config": {
            "model": Value::Null,
            "profileModel": Value::Null,
            "resolvedModel": Value::Null,
            "activeProfile": "default",
            "profiles": [],
            "mcpServers": {},
            "approvalMode": "suggest"
        }
    })
}

async fn codex_mcp_config_overview() -> Value {
    let Some(home) = home_dir() else {
        return serde_json::json!({
            "success": false,
            "error": "home directory not found",
            "servers": []
        });
    };
    let config_path = home.join(".codex").join("config.toml");
    let Some(content) = read_text_path(&config_path).await else {
        return serde_json::json!({
            "success": true,
            "configPath": config_path.display().to_string(),
            "servers": []
        });
    };
    serde_json::json!({
        "success": true,
        "configPath": config_path.display().to_string(),
        "servers": parse_codex_mcp_servers(&content)
    })
}

fn parse_top_level_toml_values(content: &str) -> serde_json::Map<String, Value> {
    let mut values = serde_json::Map::new();
    for line in content.lines().map(str::trim) {
        if line.starts_with('[') {
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if let Some(parsed) = parse_simple_toml_value(value.trim()) {
            values.insert(key.trim().to_string(), parsed);
        }
    }
    values
}

fn parse_toml_section_names(content: &str, prefix: &str) -> Vec<Value> {
    let section_prefix = format!("{prefix}.");
    content
        .lines()
        .filter_map(|line| {
            let section = line.trim().strip_prefix('[')?.strip_suffix(']')?;
            let name = section.strip_prefix(&section_prefix)?;
            (!name.contains('.')).then(|| {
                serde_json::json!({
                    "name": unquote_toml_key(name),
                    "model": Value::Null,
                    "modelProvider": Value::Null,
                })
            })
        })
        .collect()
}

fn codex_mcp_servers_map(content: &str) -> Value {
    let mut map = serde_json::Map::new();
    for server in parse_codex_mcp_servers(content) {
        if let Some(name) = server.get("name").and_then(Value::as_str) {
            if let Some(raw) = server.get("raw") {
                map.insert(name.to_string(), raw.clone());
            }
        }
    }
    Value::Object(map)
}

fn parse_codex_mcp_servers(content: &str) -> Vec<Value> {
    let mut servers = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_config = serde_json::Map::new();
    let mut current_env = false;

    for line in content.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(section) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            if let Some(rest) = section.strip_prefix("mcp_servers.") {
                if let Some(name) = rest.strip_suffix(".env") {
                    let name = unquote_toml_key(name);
                    if current_name.as_deref() != Some(name.as_str()) {
                        flush_codex_mcp_server(
                            &mut servers,
                            &mut current_name,
                            &mut current_config,
                        );
                        current_name = Some(name);
                    }
                    current_env = true;
                } else {
                    flush_codex_mcp_server(&mut servers, &mut current_name, &mut current_config);
                    current_name = Some(unquote_toml_key(rest));
                    current_env = false;
                }
            } else {
                flush_codex_mcp_server(&mut servers, &mut current_name, &mut current_config);
                current_env = false;
            }
            continue;
        }

        if current_name.is_none() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        let Some(parsed) = parse_simple_toml_value(value.trim()) else {
            continue;
        };
        if current_env {
            let env = current_config
                .entry("env".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if let Value::Object(env) = env {
                env.insert(key, parsed);
            }
        } else {
            current_config.insert(key, parsed);
        }
    }
    flush_codex_mcp_server(&mut servers, &mut current_name, &mut current_config);
    servers
}

fn flush_codex_mcp_server(
    servers: &mut Vec<Value>,
    current_name: &mut Option<String>,
    current_config: &mut serde_json::Map<String, Value>,
) {
    let Some(name) = current_name.take() else {
        return;
    };
    let raw = Value::Object(std::mem::take(current_config));
    servers.push(mcp_server_record(&name, &raw, "user", None));
}

fn parse_simple_toml_value(value: &str) -> Option<Value> {
    let value = value.split('#').next().unwrap_or(value).trim();
    if let Some(string) = parse_toml_string(value) {
        return Some(Value::String(string));
    }
    if value.starts_with('[') && value.ends_with(']') {
        let inner = &value[1..value.len().saturating_sub(1)];
        let values = inner
            .split(',')
            .filter_map(|item| parse_toml_string(item.trim()).map(Value::String))
            .collect::<Vec<_>>();
        return Some(Value::Array(values));
    }
    match value {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        _ => value.parse::<i64>().ok().map(Value::from),
    }
}

fn parse_toml_string(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        Some(value[1..value.len() - 1].replace("\\\"", "\""))
    } else {
        None
    }
}

fn unquote_toml_key(value: &str) -> String {
    parse_toml_string(value).unwrap_or_else(|| value.trim().to_string())
}

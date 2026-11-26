use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use iowb_protocol::{
    ChatCommandAction, ChatCommandCatalog, ChatCommandContext, ChatCommandDescriptor,
    ChatCommandKind, ChatRuntime, PrepareChatCommandRequest, PreparedChatCommand, Provider,
    leading_chat_command,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    AppState, CoreError, Result, effective_agent_command_provider,
    should_use_direct_ai_gateway_runtime,
};

pub(crate) type CommandCatalogCache = HashMap<String, (Instant, ChatCommandCatalog)>;

fn descriptor(
    name: &str,
    description: &str,
    kind: ChatCommandKind,
    source: &str,
    action: ChatCommandAction,
) -> ChatCommandDescriptor {
    ChatCommandDescriptor {
        id: format!("{source}:{name}"),
        name: name.to_string(),
        description: description.chars().take(512).collect(),
        argument_hint: String::new(),
        kind,
        source: source.to_string(),
        action,
        available: true,
        reason: None,
        verified: true,
    }
}

fn builtin_commands() -> BTreeMap<String, ChatCommandDescriptor> {
    [
        ("help", "Show supported commands", ChatCommandAction::Help),
        (
            "model",
            "Choose a model without sending a message",
            ChatCommandAction::Model,
        ),
        (
            "new",
            "Start a fresh chat; keep this chat's history",
            ChatCommandAction::New,
        ),
        (
            "clear",
            "Start a fresh chat; keep this chat's history",
            ChatCommandAction::New,
        ),
        (
            "status",
            "Show this chat's provider and state",
            ChatCommandAction::Status,
        ),
    ]
    .into_iter()
    .map(|(name, description, action)| {
        (
            name.to_string(),
            descriptor(
                name,
                description,
                ChatCommandKind::AppAction,
                "workbench",
                action,
            ),
        )
    })
    .collect()
}

fn effective_provider(context: &ChatCommandContext) -> Provider {
    if context.runtime == ChatRuntime::IoGateway {
        context.provider
    } else {
        effective_agent_command_provider(context.provider, context.model.as_deref())
    }
}

fn literal_prompt(prompt: &str) -> PreparedChatCommand {
    PreparedChatCommand {
        action: ChatCommandAction::Prompt,
        prompt: prompt.to_string(),
        command: None,
        message: None,
    }
}

impl AppState {
    pub async fn chat_command_catalog(
        &self,
        context: &ChatCommandContext,
        user: &str,
        refresh: bool,
    ) -> Result<ChatCommandCatalog> {
        let cwd = self
            .path_validator
            .validate_path(PathBuf::from(&context.project_path), false)
            .await?;
        if !cwd.is_dir() {
            return Err(CoreError::InvalidInput(
                "project path must be a directory".into(),
            ));
        }
        let session = if let Some(id) = context.session_id.as_deref() {
            let session = self.sessions.get(id).await?;
            if session.provider != context.provider
                || !crate::same_project_path(&session.project_path, &cwd.to_string_lossy())
            {
                return Err(CoreError::InvalidInput(
                    "command context does not match the session".into(),
                ));
            }
            Some(session)
        } else {
            None
        };
        let provider = effective_provider(context);
        let native =
            !should_use_direct_ai_gateway_runtime(context.provider, context.model.as_deref());
        let capability_revision = self
            .storage
            .get_setting("chat.commands.revision")?
            .unwrap_or(Value::Null);
        let key = format!(
            "{user}:{provider:?}:{:?}:{}:{:?}:{:?}:{:?}:{capability_revision}",
            context.runtime,
            cwd.display(),
            context.session_id,
            context.model,
            (
                std::env::var_os("CODEX_HOME"),
                std::env::var_os("CLAUDE_CONFIG_DIR"),
                std::env::var_os("IO_WORKBENCH_AGENT_COMMAND"),
                std::env::var_os(format!(
                    "IO_WORKBENCH_{}_COMMAND",
                    provider.as_str().to_ascii_uppercase()
                ))
            )
        );
        let cached = if refresh {
            None
        } else {
            self.chat_command_cache
                .lock()
                .await
                .get(&key)
                .filter(|(at, _)| at.elapsed() < Duration::from_secs(15))
                .map(|(_, catalog)| catalog.clone())
        };
        let mut catalog = if let Some(catalog) = cached {
            catalog
        } else {
            let mut commands = builtin_commands();
            let mut warnings = Vec::new();
            let authoritative = if native && provider == Provider::Claude {
                self.claude_catalog(context, &cwd, &mut commands, &mut warnings)
                    .await?
            } else if native && provider == Provider::Codex {
                self.codex_catalog(&cwd, &mut commands, &mut warnings)
                    .await?
            } else {
                false
            };
            if native && matches!(provider, Provider::Codex | Provider::Claude) {
                commands.insert(
                    "compact".into(),
                    descriptor(
                        "compact",
                        "Compact context; preserve visible chat history",
                        ChatCommandKind::NativeOperation,
                        provider.as_str(),
                        if provider == Provider::Codex {
                            ChatCommandAction::Compact
                        } else {
                            ChatCommandAction::Prompt
                        },
                    ),
                );
                commands.insert(
                    "mcp".into(),
                    descriptor(
                        "mcp",
                        "Show MCP server status",
                        ChatCommandKind::AppAction,
                        "workbench",
                        ChatCommandAction::Mcp,
                    ),
                );
            }
            let commands = commands.into_values().collect::<Vec<_>>();
            let revision = format!("{:x}", Sha256::digest(serde_json::to_vec(&commands)?));
            let catalog = ChatCommandCatalog {
                commands,
                revision,
                warnings,
                authoritative,
            };
            let mut cache = self.chat_command_cache.lock().await;
            if cache.len() >= 128 {
                cache.clear();
            }
            cache.insert(key, (Instant::now(), catalog.clone()));
            catalog
        };
        let busy = session.as_ref().is_some_and(|session| session.active);
        for command in &mut catalog.commands {
            if command.name == "compact"
                && (session.is_none()
                    || session
                        .as_ref()
                        .is_some_and(|session| session.message_count == 0))
            {
                command.available = false;
                command.reason = Some("Open a chat with history before compacting.".into());
            } else if busy && command.kind != ChatCommandKind::AppAction {
                command.available = false;
                command.reason = Some("Wait for the active response to finish.".into());
            }
        }
        Ok(catalog)
    }

    async fn claude_catalog(
        &self,
        context: &ChatCommandContext,
        cwd: &Path,
        commands: &mut BTreeMap<String, ChatCommandDescriptor>,
        warnings: &mut Vec<String>,
    ) -> Result<bool> {
        let home = claude_home();
        let scan_cwd = cwd.to_path_buf();
        let extensions = tokio::task::spawn_blocking(move || claude_extensions(&home, &scan_cwd))
            .await
            .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
        let extensions_revision = extension_revision(&extensions);
        commands.extend(extensions);
        let metadata = context
            .session_id
            .as_deref()
            .and_then(|id| {
                self.storage
                    .get_setting(&format!("chat.commands.claude.{id}"))
                    .ok()
                    .flatten()
            })
            .filter(|metadata| {
                metadata.get("cwd").and_then(Value::as_str) == cwd.to_str()
                    && metadata.get("runtime") == Some(&json!(context.runtime))
                    && metadata.get("model") == Some(&json!(context.model))
                    && metadata.get("extensionsRevision").and_then(Value::as_str)
                        == Some(extensions_revision.as_str())
            });
        let Some(names) = metadata
            .as_ref()
            .and_then(|metadata| metadata.get("names"))
            .and_then(Value::as_array)
        else {
            warnings.push(
                "Claude extensions are provisional until the session reports its loaded commands."
                    .into(),
            );
            return Ok(false);
        };
        let names: Vec<&str> = names.iter().filter_map(Value::as_str).collect();
        commands.retain(|name, command| {
            command.source == "workbench" || names.contains(&name.as_str())
        });
        for name in names {
            if !valid_command_name(name) {
                continue;
            }
            commands
                .entry(name.to_string())
                .or_insert_with(|| {
                    descriptor(
                        name,
                        "Native Claude command",
                        ChatCommandKind::NativeOperation,
                        "claude",
                        ChatCommandAction::Prompt,
                    )
                })
                .verified = true;
        }
        let terminal_only = metadata
            .as_ref()
            .and_then(|metadata| metadata.get("terminal"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        for name in terminal_only {
            if let Some(command) = commands.get_mut(name)
                && command.source != "workbench"
            {
                command.available = false;
                command.reason = Some("Terminal-only; not available in chat.".into());
            }
        }
        Ok(true)
    }

    async fn codex_catalog(
        &self,
        cwd: &Path,
        commands: &mut BTreeMap<String, ChatCommandDescriptor>,
        warnings: &mut Vec<String>,
    ) -> Result<bool> {
        let native = match tokio::time::timeout(
            Duration::from_secs(8),
            self.codex_app_server.list_skills(cwd),
        )
        .await
        {
            Ok(Ok(result)) => {
                if result
                    .get("data")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|group| {
                        group
                            .get("errors")
                            .and_then(Value::as_array)
                            .is_some_and(|errors| !errors.is_empty())
                    })
                {
                    warnings.push("Some Codex skill directories could not be read.".into());
                }
                for skill in codex_skills(&result) {
                    if skill.get("enabled").and_then(Value::as_bool) == Some(false) {
                        continue;
                    }
                    let Some(name) = skill
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|name| valid_command_name(name))
                    else {
                        continue;
                    };
                    let mut command = descriptor(
                        name,
                        skill
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("Codex skill"),
                        ChatCommandKind::Skill,
                        "codex",
                        ChatCommandAction::Prompt,
                    );
                    command.argument_hint = "[arguments]".into();
                    commands.entry(name.to_string()).or_insert(command);
                }
                true
            }
            _ => {
                warnings.push(
                    "Native Codex discovery unavailable; showing provisional filesystem skills."
                        .into(),
                );
                let scan_cwd = cwd.to_path_buf();
                commands.extend(
                    tokio::task::spawn_blocking(move || codex_extensions(&scan_cwd))
                        .await
                        .map_err(|error| CoreError::InvalidInput(error.to_string()))?,
                );
                false
            }
        };
        scan_markdown(
            &codex_home().join("prompts"),
            "prompts:",
            ChatCommandKind::LegacyPrompt,
            "codex-prompt",
            false,
            commands,
            0,
        );
        Ok(native)
    }

    async fn chat_mcp_status(&self, context: &ChatCommandContext) -> Result<String> {
        let servers: Option<Vec<String>> = if effective_provider(context) == Provider::Codex {
            let result = tokio::time::timeout(
                Duration::from_secs(8),
                self.codex_app_server.list_mcp_status(),
            )
            .await
            .map_err(|_| CoreError::InvalidInput("Codex MCP status timed out.".into()))??;
            let mut lines = result
                .get("data")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|server| {
                    let name = server.get("name")?.as_str()?;
                    let status = server
                        .get("runtimeStatus")
                        .and_then(Value::as_str)
                        .unwrap_or("configured");
                    Some(format!("{name} — {status}"))
                })
                .collect::<Vec<_>>();
            if result
                .get("nextCursor")
                .is_some_and(|cursor| !cursor.is_null())
            {
                lines.push("More servers are not shown.".into());
            }
            Some(lines)
        } else {
            context
                .session_id
                .as_deref()
                .and_then(|id| {
                    self.storage
                        .get_setting(&format!("chat.commands.claude.{id}"))
                        .ok()
                        .flatten()
                })
                .and_then(|metadata| {
                    metadata
                        .get("mcpServers")
                        .and_then(Value::as_array)
                        .cloned()
                })
                .map(|servers| {
                    servers
                        .iter()
                        .filter_map(|server| {
                            Some(format!(
                                "{} — {}",
                                server.get("name")?.as_str()?,
                                server.get("status")?.as_str()?
                            ))
                        })
                        .collect()
                })
        };
        Ok(match servers {
            None => "MCP status is reported after the first message in this chat.".into(),
            Some(lines) if lines.is_empty() => "No MCP servers are configured.".into(),
            Some(lines) => lines.join("\n"),
        })
    }

    pub async fn prepare_chat_command(
        &self,
        request: &PrepareChatCommandRequest,
        user: &str,
    ) -> Result<PreparedChatCommand> {
        let Some((name, arguments)) = leading_chat_command(&request.prompt) else {
            return Ok(literal_prompt(&request.prompt));
        };
        let catalog = self
            .chat_command_catalog(&request.context, user, false)
            .await?;
        let Some(command) = catalog
            .commands
            .into_iter()
            .find(|command| command.name == name)
        else {
            // Only a list the provider reported proves a name is unknown, and
            // MCP prompt names are owned by the CLI, so both otherwise forward.
            if catalog.authoritative && !name.starts_with("mcp__") {
                return Err(CoreError::InvalidInput(format!(
                    "Unknown or unsupported command /{name}. Use /help. To send literal text, start with a backslash: \\/{name}"
                )));
            }
            return Ok(literal_prompt(&request.prompt));
        };
        if request
            .command_id
            .as_deref()
            .is_some_and(|id| id != command.id)
        {
            return Err(CoreError::Conflict(
                "the command catalog changed; select the command again".into(),
            ));
        }
        if !command.available {
            return Err(CoreError::Conflict(
                command
                    .reason
                    .clone()
                    .unwrap_or_else(|| "command unavailable".into()),
            ));
        }
        if command.action != ChatCommandAction::Prompt && !arguments.trim().is_empty() {
            return Err(CoreError::InvalidInput(format!(
                "/{} does not accept arguments",
                command.name
            )));
        }
        let message = if command.action == ChatCommandAction::Mcp {
            Some(self.chat_mcp_status(&request.context).await?)
        } else {
            None
        };
        Ok(PreparedChatCommand {
            action: command.action,
            prompt: request.prompt.clone(),
            command: Some(command),
            message,
        })
    }

    pub(crate) async fn resolve_chat_command_prompt(
        &self,
        context: ChatCommandContext,
        prompt: &str,
        user: &str,
    ) -> Result<String> {
        if prompt.starts_with("\\/") {
            return Ok(prompt.to_string());
        }
        let Some((name, arguments)) = leading_chat_command(prompt) else {
            return Ok(prompt.to_string());
        };
        let prepared = self
            .prepare_chat_command(
                &PrepareChatCommandRequest {
                    context: context.clone(),
                    prompt: prompt.into(),
                    command_id: None,
                },
                user,
            )
            .await?;
        let Some(command) = prepared.command else {
            return Ok(prompt.to_string());
        };
        if prepared.action != ChatCommandAction::Prompt {
            return Err(CoreError::InvalidInput(format!(
                "/{} is an app action; use the command menu",
                command.name
            )));
        }
        if effective_provider(&context) == Provider::Codex {
            if name.starts_with("prompts:") {
                return crate::resolve_codex_slash_prompt(prompt, Some(&codex_home()));
            }
            return Ok(format!("${name} {arguments}").trim_end().to_string());
        }
        Ok(prompt.trim_start().to_string())
    }
}

fn claude_home() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".claude"))
}

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".codex"))
}

fn frontmatter(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    lines
        .take_while(|line| line.trim() != "---")
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name.trim() == key).then(|| value.trim().trim_matches(['\'', '"']).to_string())
        })
}

fn scan_markdown(
    root: &Path,
    prefix: &str,
    kind: ChatCommandKind,
    source: &str,
    skills: bool,
    commands: &mut BTreeMap<String, ChatCommandDescriptor>,
    depth: usize,
) {
    if depth > 6 || commands.len() >= 512 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries = entries.flatten().take(4096).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if commands.len() >= 512 {
            break;
        }
        let path = entry.path();
        if path.is_dir() && entry.file_type().is_ok_and(|kind| !kind.is_symlink()) {
            if source == "codex-prompt" {
                continue;
            }
            let directory = entry.file_name().to_string_lossy().to_string();
            let next_prefix = if skills {
                prefix.to_string()
            } else {
                format!("{prefix}{directory}:")
            };
            scan_markdown(
                &path,
                &next_prefix,
                kind,
                source,
                skills,
                commands,
                depth + 1,
            );
        } else if path.extension().is_some_and(|extension| extension == "md")
            && (!skills || entry.file_name() == "SKILL.md")
        {
            let Ok(metadata) = path.metadata() else {
                continue;
            };
            if metadata.len() > 256 * 1024 {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if frontmatter(&text, "user-invocable").as_deref() == Some("false") {
                continue;
            }
            let stem = if skills {
                path.parent().and_then(Path::file_name)
            } else {
                path.file_stem()
            }
            .unwrap_or_default()
            .to_string_lossy();
            let name = format!(
                "{prefix}{}",
                if skills {
                    frontmatter(&text, "name").unwrap_or_else(|| stem.to_string())
                } else {
                    stem.to_string()
                }
            );
            if !valid_command_name(&name) || builtin_commands().contains_key(&name) {
                continue;
            }
            let mut command = descriptor(
                &name,
                &frontmatter(&text, "description").unwrap_or_else(|| {
                    format!("{} from {source}", if skills { "skill" } else { "command" })
                }),
                kind,
                source,
                ChatCommandAction::Prompt,
            );
            command.argument_hint =
                frontmatter(&text, "argument-hint").unwrap_or_else(|| "[arguments]".into());
            command.description = command.description.chars().take(512).collect();
            command.argument_hint = command.argument_hint.chars().take(256).collect();
            command.verified = false;
            commands.insert(name, command);
        }
    }
}

fn claude_extensions(home: &Path, cwd: &Path) -> BTreeMap<String, ChatCommandDescriptor> {
    let mut commands = BTreeMap::new();
    let mut enabled = BTreeMap::new();
    let mut roots = vec![home.to_path_buf()];
    let mut ancestors = Vec::new();
    for ancestor in cwd.ancestors() {
        ancestors.push(ancestor.join(".claude"));
        if ancestor.join(".git").exists() {
            break;
        }
    }
    ancestors.reverse();
    roots.extend(ancestors);
    for root in &roots {
        for filename in ["settings.json", "settings.local.json"] {
            if let Ok(text) = std::fs::read_to_string(root.join(filename))
                && let Ok(settings) = serde_json::from_str::<Value>(&text)
                && let Some(plugins) = settings.get("enabledPlugins").and_then(Value::as_object)
            {
                for (plugin, enabled_value) in plugins {
                    enabled.insert(plugin.clone(), enabled_value == true);
                }
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(home.join("plugins/installed_plugins.json"))
        && let Ok(installed) = serde_json::from_str::<Value>(&text)
    {
        for (plugin, installations) in installed
            .get("plugins")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if enabled.get(plugin) != Some(&true) {
                continue;
            }
            let namespace = plugin.split('@').next().unwrap_or(plugin);
            for installation in installations.as_array().into_iter().flatten() {
                if installation
                    .get("projectPath")
                    .and_then(Value::as_str)
                    .is_some_and(|project| !cwd.starts_with(project))
                {
                    continue;
                }
                if let Some(path) = installation.get("installPath").and_then(Value::as_str) {
                    scan_markdown(
                        &Path::new(path).join("commands"),
                        &format!("{namespace}:"),
                        ChatCommandKind::LegacyPrompt,
                        "claude-plugin",
                        false,
                        &mut commands,
                        0,
                    );
                    scan_markdown(
                        &Path::new(path).join("skills"),
                        &format!("{namespace}:"),
                        ChatCommandKind::Skill,
                        "claude-plugin",
                        true,
                        &mut commands,
                        0,
                    );
                }
            }
        }
    }
    for root in roots {
        scan_markdown(
            &root.join("commands"),
            "",
            ChatCommandKind::LegacyPrompt,
            "claude-command",
            false,
            &mut commands,
            0,
        );
        scan_markdown(
            &root.join("skills"),
            "",
            ChatCommandKind::Skill,
            "claude-skill",
            true,
            &mut commands,
            0,
        );
    }
    commands
}

fn codex_extensions(cwd: &Path) -> BTreeMap<String, ChatCommandDescriptor> {
    let mut commands = BTreeMap::new();
    let home = codex_home();
    let mut roots = vec![home.join("skills"), home.join("skills/.system")];
    if let Some(user) = dirs::home_dir() {
        roots.push(user.join(".agents/skills"));
    }
    let mut ancestors = Vec::new();
    for ancestor in cwd.ancestors() {
        ancestors.push(ancestor.join(".agents/skills"));
        if ancestor.join(".git").exists() {
            break;
        }
    }
    ancestors.reverse();
    roots.extend(ancestors);
    for root in roots {
        scan_markdown(
            &root,
            "",
            ChatCommandKind::Skill,
            "codex",
            true,
            &mut commands,
            0,
        );
    }
    if let Ok(text) = std::fs::read_to_string(home.join("config.toml"))
        && let Ok(config) = text.parse::<toml::Value>()
    {
        for disabled in config
            .get("skills")
            .and_then(|skills| skills.get("config"))
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            if disabled.get("enabled").and_then(toml::Value::as_bool) != Some(false) {
                continue;
            }
            if let Some(name) = disabled
                .get("path")
                .and_then(toml::Value::as_str)
                .and_then(|path| Path::new(path).parent())
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
            {
                commands.remove(name);
            }
        }
    }
    commands
}

pub(crate) async fn persist_claude_commands(
    context: &crate::AgentStartContext,
    init: Option<Value>,
) {
    let Some(init) = init else {
        return;
    };
    let cwd = context.project_path.clone();
    let revision = tokio::task::spawn_blocking(move || claude_extension_revision(&cwd))
        .await
        .ok();
    let key = format!("chat.commands.claude.{}", context.session_id);
    let value = json!({"names": init["names"], "terminal": init["terminal"], "mcpServers": init["mcpServers"], "cwd": context.project_path, "runtime": context.runtime, "model": context.model, "extensionsRevision": revision});
    let previous = context.storage.get_setting(&key).ok().flatten();
    if previous.as_ref() == Some(&value) {
        return;
    }
    let _ = context.storage.set_setting(&key, &value);
    // MCP status does not shape the catalog, so only a changed command list,
    // scope or extension revision invalidates cached catalogs.
    let catalog_changed = previous.as_ref().is_none_or(|previous| {
        [
            "names",
            "terminal",
            "cwd",
            "runtime",
            "model",
            "extensionsRevision",
        ]
        .iter()
        .any(|field| previous.get(field) != value.get(field))
    });
    if catalog_changed {
        invalidate_command_catalog(&context.storage);
    }
}

fn extension_revision(commands: &BTreeMap<String, ChatCommandDescriptor>) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(commands).unwrap_or_default())
    )
}

pub(crate) fn claude_extension_revision(cwd: &Path) -> String {
    extension_revision(&claude_extensions(&claude_home(), cwd))
}

pub(crate) fn invalidate_command_catalog(storage: &iowb_storage::Storage) {
    let _ = storage.set_setting(
        "chat.commands.revision",
        &json!(uuid::Uuid::new_v4().to_string()),
    );
}

fn valid_command_name(name: &str) -> bool {
    leading_chat_command(&format!("/{name}"))
        .is_some_and(|(token, arguments)| token == name && arguments.is_empty())
}

fn codex_skills(skills: &Value) -> impl Iterator<Item = &Value> {
    skills
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|group| {
            group
                .get("skills")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
}

pub(crate) fn codex_skill_input(skills: &Value, name: &str) -> Option<Value> {
    let skill = codex_skills(skills).find(|skill| {
        skill.get("name").and_then(Value::as_str) == Some(name)
            && skill.get("enabled").and_then(Value::as_bool) != Some(false)
    })?;
    let path = skill.get("path")?.as_str()?;
    Some(json!({"type": "skill", "name": name, "path": path}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_commands_claude_project_precedence_and_enabled_plugin_namespaces() {
        let root = std::env::temp_dir().join(format!("iowb-command-scan-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let project = root.join("project");
        let plugin = root.join("plugin");
        for directory in [
            home.join("commands"),
            home.join("plugins"),
            project.join(".git"),
            project.join(".claude/skills/inspect"),
            project.join(".claude/skills/hidden"),
            plugin.join("commands"),
        ] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(
            home.join("commands/inspect.md"),
            "---\ndescription: User command\n---\nUser",
        )
        .unwrap();
        std::fs::write(
            project.join(".claude/skills/inspect/SKILL.md"),
            "---\ndescription: Project skill\n---\nProject",
        )
        .unwrap();
        std::fs::write(
            project.join(".claude/skills/hidden/SKILL.md"),
            "---\nuser-invocable: false\n---\nHidden",
        )
        .unwrap();
        std::fs::write(plugin.join("commands/check.md"), "Check").unwrap();
        std::fs::write(
            home.join("settings.json"),
            r#"{"enabledPlugins":{"fixture@market":true}}"#,
        )
        .unwrap();
        std::fs::write(
            home.join("plugins/installed_plugins.json"),
            json!({"plugins":{"fixture@market":[{"installPath":plugin}]}}).to_string(),
        )
        .unwrap();
        let commands = claude_extensions(&home, &project);
        assert_eq!(commands["inspect"].description, "Project skill");
        assert_eq!(commands["inspect"].kind, ChatCommandKind::Skill);
        assert!(commands.contains_key("fixture:check"));
        assert!(!commands.contains_key("hidden"));
        std::fs::write(
            project.join(".claude/settings.local.json"),
            r#"{"enabledPlugins":{"fixture@market":false}}"#,
        )
        .unwrap();
        assert!(!claude_extensions(&home, &project).contains_key("fixture:check"));
        std::fs::remove_dir_all(root).unwrap();
    }
}

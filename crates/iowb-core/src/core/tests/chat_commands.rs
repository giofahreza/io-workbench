fn command_context(
    project: &Path,
    session_id: Option<String>,
) -> iowb_protocol::ChatCommandContext {
    iowb_protocol::ChatCommandContext {
        provider: Provider::Claude,
        project_path: project.display().to_string(),
        session_id,
        model: None,
        runtime: ChatRuntime::NativeCli,
    }
}

#[tokio::test]
async fn chat_commands_available_before_first_prompt_and_discovery_does_not_run_shell() {
    let (state, root, project) = temporary_app_state("commands-empty").await;
    let directory = project.join(".claude/skills/check");
    std::fs::create_dir_all(&directory).unwrap();
    let marker = root.join("must-not-exist");
    std::fs::write(directory.join("SKILL.md"), format!("---\ndescription: Check this project\nargument-hint: [file]\n---\n!`touch {}`\n$ARGUMENTS", marker.display())).unwrap();
    let context = command_context(&project, None);
    let catalog = state
        .chat_command_catalog(&context, "user-1", true)
        .await
        .unwrap();
    assert!(
        catalog
            .commands
            .iter()
            .any(|command| command.name == "help" && command.available)
    );
    assert!(
        catalog
            .commands
            .iter()
            .any(|command| command.name == "compact" && !command.available)
    );
    let skill = catalog
        .commands
        .iter()
        .find(|command| command.name == "check")
        .unwrap();
    assert!(!skill.verified);
    assert_eq!(skill.argument_hint, "[file]");
    assert!(!marker.exists());
    assert!(state.storage.list_sessions().unwrap().is_empty());
    let resolved = state
        .resolve_chat_command_prompt(context, "/check 'a b'", "user-1")
        .await
        .unwrap();
    assert_eq!(resolved, "/check 'a b'");
    assert!(!marker.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn chat_commands_native_empty_catalog_hides_provisional_extensions() {
    let (state, root, project) = temporary_app_state("commands-native").await;
    let directory = project.join(".claude/commands");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("not-loaded.md"), "A prompt").unwrap();
    let session = state
        .sessions
        .create_or_update(
            Provider::Claude,
            project.display().to_string(),
            None,
            false,
            None,
            Some(ChatRuntime::NativeCli),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    state.sessions.set_active(&session.id, false).await.unwrap();
    state.storage.set_setting(&format!("chat.commands.claude.{}", session.id), &serde_json::json!({"names": [], "cwd": project, "runtime": "native_cli", "model": null, "extensionsRevision": chat_commands::claude_extension_revision(&project)})).unwrap();
    let context = command_context(&project, Some(session.id));
    let catalog = state
        .chat_command_catalog(&context, "user-1", true)
        .await
        .unwrap();
    assert!(
        !catalog
            .commands
            .iter()
            .any(|command| command.name == "not-loaded")
    );
    assert!(
        catalog
            .commands
            .iter()
            .any(|command| command.name == "help")
    );
    assert!(
        !catalog
            .warnings
            .iter()
            .any(|warning| warning.contains("provisional"))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn chat_commands_validate_identity_unknown_names_busy_and_context() {
    let (state, root, project) = temporary_app_state("commands-validation").await;
    let context = command_context(&project, None);
    let request =
        |prompt: &str, command_id: Option<String>| iowb_protocol::PrepareChatCommandRequest {
            context: context.clone(),
            prompt: prompt.into(),
            command_id,
        };
    let forwarded = state
        .prepare_chat_command(&request("/unknown-actual-command", None), "user-1")
        .await
        .unwrap();
    assert_eq!(forwarded.action, iowb_protocol::ChatCommandAction::Prompt);
    assert!(forwarded.command.is_none());
    assert!(
        state
            .prepare_chat_command(&request("/model extra", None), "user-1")
            .await
            .is_err()
    );
    assert!(
        state
            .prepare_chat_command(&request("/help", Some("forged".into())), "user-1")
            .await
            .is_err()
    );
    assert_eq!(
        state
            .prepare_chat_command(&request("/help", None), "user-1")
            .await
            .unwrap()
            .action,
        iowb_protocol::ChatCommandAction::Help
    );
    assert!(
        state
            .resolve_chat_command_prompt(context.clone(), "/clear", "user-1")
            .await
            .is_err()
    );
    assert_eq!(
        state
            .resolve_chat_command_prompt(context, "\\/model", "user-1")
            .await
            .unwrap(),
        "\\/model"
    );
    let session = state
        .sessions
        .create_or_update(
            Provider::Claude,
            project.display().to_string(),
            None,
            false,
            None,
            Some(ChatRuntime::NativeCli),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let mut context = command_context(&project, Some(session.id));
    let catalog = state
        .chat_command_catalog(&context, "user-1", true)
        .await
        .unwrap();
    assert!(
        catalog
            .commands
            .iter()
            .any(|command| command.name == "compact" && !command.available)
    );
    context.provider = Provider::Codex;
    assert!(
        state
            .chat_command_catalog(&context, "user-1", true)
            .await
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn chat_commands_capture_init_and_compaction_boundary_without_assistant_fabrication() {
    let mut normalizer = ClaudeLiveOutputNormalizer::default();
    normalizer.push_chunks(
        "{\"type\":\"system\",\"subtype\":\"init\",\"slash_commands\":[\"check\",\"compact\",\"doctor\"],\"terminal_slash_commands\":[\"doctor\"],\"mcp_servers\":[{\"name\":\"github\",\"status\":\"connected\",\"source\":\"user\"}]}\n",
    );
    assert_eq!(
        normalizer.pending_init.take().unwrap(),
        serde_json::json!({
            "names": ["check", "compact", "doctor"],
            "terminal": ["doctor"],
            "mcpServers": [{"name": "github", "status": "connected"}]
        })
    );
    normalizer.push_chunks("{\"type\":\"system\",\"subtype\":\"compact_boundary\",\"compact_metadata\":{\"trigger\":\"manual\"}}\n");
    assert!(normalizer.take_final_assistant_message().is_none());
    let history = normalizer.take_history_messages(None);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, MessageRole::System);
    assert_eq!(history[0].kind, "context_compaction");
}

#[tokio::test]
async fn chat_commands_invalidate_stale_claude_init_when_extensions_change() {
    let (state, root, project) = temporary_app_state("commands-stale-init").await;
    let directory = project.join(".claude/skills/stale-fixture");
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join("SKILL.md");
    std::fs::write(&file, "---\ndescription: Visible fixture\n---\nInspect").unwrap();
    let session = state
        .sessions
        .create_or_update(
            Provider::Claude,
            project.display().to_string(),
            None,
            false,
            None,
            Some(ChatRuntime::NativeCli),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    state.sessions.set_active(&session.id, false).await.unwrap();
    state
        .storage
        .set_setting(
            &format!("chat.commands.claude.{}", session.id),
            &serde_json::json!({
                "names": ["stale-fixture"], "cwd": project, "runtime": "native_cli", "model": null,
                "extensionsRevision": chat_commands::claude_extension_revision(&project)
            }),
        )
        .unwrap();
    let context = command_context(&project, Some(session.id));
    let catalog = state
        .chat_command_catalog(&context, "user-1", true)
        .await
        .unwrap();
    assert!(
        catalog
            .commands
            .iter()
            .any(|command| command.name == "stale-fixture" && command.verified)
    );
    std::fs::write(&file, "---\nuser-invocable: false\n---\nHidden fixture").unwrap();
    let catalog = state
        .chat_command_catalog(&context, "user-1", true)
        .await
        .unwrap();
    assert!(
        !catalog
            .commands
            .iter()
            .any(|command| command.name == "stale-fixture")
    );
    assert!(!catalog.authoritative);
    assert_eq!(
        state
            .resolve_chat_command_prompt(context, "/stale-fixture", "user-1")
            .await
            .unwrap(),
        "/stale-fixture"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn chat_commands_codex_native_discovery_filters_disabled_skills_and_uses_typed_identity() {
    use std::os::unix::fs::PermissionsExt;

    let (mut state, root, project) = temporary_app_state("commands-codex-native").await;
    let script = root.join("fake-codex.sh");
    let log = root.join("requests.log");
    let skills = serde_json::json!({"data": [{"skills": [
        {"name": "fixture-native", "description": "Native skill", "path": "/server/SKILL.md", "enabled": true},
        {"name": "fixture-disabled", "path": "/disabled/SKILL.md", "enabled": false},
        {"name": "invalid name", "path": "/invalid/SKILL.md", "enabled": true},
        {"name": "help", "path": "/shadow/SKILL.md", "enabled": true}
    ], "errors": []}]});
    let response = serde_json::json!({"id": 2, "result": skills});
    std::fs::write(&script, format!("#!/bin/sh\nread first\nprintf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"fixture\"}}}}'\nread second\nread third\nprintf '%s\\n' \"$third\" >> '{}'\nprintf '%s\\n' '{}'\n", log.display(), response)).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    state.codex_app_server = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
    let mut context = command_context(&project, None);
    context.provider = Provider::Codex;
    let catalog = state
        .chat_command_catalog(&context, "user-1", true)
        .await
        .unwrap();
    assert!(
        catalog
            .commands
            .iter()
            .any(|command| command.name == "fixture-native" && command.verified)
    );
    assert!(
        !catalog
            .commands
            .iter()
            .any(|command| command.name == "fixture-disabled" || command.name == "invalid name")
    );
    assert!(
        catalog
            .commands
            .iter()
            .any(|command| command.name == "help" && command.source == "workbench")
    );
    assert_eq!(
        chat_commands::codex_skill_input(&skills, "fixture-native"),
        Some(
            serde_json::json!({"type": "skill", "name": "fixture-native", "path": "/server/SKILL.md"})
        )
    );
    assert!(chat_commands::codex_skill_input(&skills, "fixture-disabled").is_none());
    assert_eq!(
        state
            .resolve_chat_command_prompt(context, "/fixture-native 'a b'", "user-1")
            .await
            .unwrap(),
        "$fixture-native 'a b'"
    );
    let requests = std::fs::read_to_string(log).unwrap();
    assert!(
        requests
            .lines()
            .all(|request| request.contains("\"method\":\"skills/list\"")
                && request.contains(project.to_str().unwrap()))
    );
    assert!(state.storage.list_sessions().unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

async fn claude_command_session(state: &AppState, project: &Path) -> SessionSummary {
    let session = state
        .sessions
        .create_or_update(
            Provider::Claude,
            project.display().to_string(),
            None,
            false,
            None,
            Some(ChatRuntime::NativeCli),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    state.sessions.set_active(&session.id, false).await.unwrap();
    session
}

fn claude_command_start_context(
    state: &AppState,
    project: &Path,
    session_id: &str,
) -> AgentStartContext {
    AgentStartContext {
        request_id: None,
        provider: Provider::Claude,
        session_id: session_id.to_string(),
        durable_run_id: None,
        attempt_id: None,
        response_id: "command-init-response".to_string(),
        sequence: Arc::new(AtomicU64::new(0)),
        project_path: project.to_path_buf(),
        prompt: "hello".to_string(),
        model: None,
        runtime: ChatRuntime::NativeCli,
        effort: None,
        mode: None,
        thinking: None,
        fast: None,
        native_resume_session_id: None,
        native_rollout_owned_by_provider: false,
        context_rollover_id: None,
        direct_ai_config: None,
        direct_ai_messages: Vec::new(),
        sessions: state.sessions.clone(),
        storage: state.storage.clone(),
        hub: WsHub::new(),
    }
}

fn prepare_request(
    context: &iowb_protocol::ChatCommandContext,
    prompt: &str,
) -> iowb_protocol::PrepareChatCommandRequest {
    iowb_protocol::PrepareChatCommandRequest {
        context: context.clone(),
        prompt: prompt.into(),
        command_id: None,
    }
}

#[tokio::test]
async fn chat_commands_claude_init_round_trip_makes_the_catalog_authoritative() {
    use iowb_protocol::ChatCommandAction::{Mcp, Prompt};

    let (state, root, project) = temporary_app_state("commands-init-round-trip").await;
    let directory = project.join(".claude/skills/check");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("SKILL.md"), "---\ndescription: Check\n---\nCheck").unwrap();
    let fresh = command_context(&project, None);
    let catalog = state
        .chat_command_catalog(&fresh, "user-1", true)
        .await
        .unwrap();
    assert!(!catalog.authoritative);
    for forwarded in ["/init", "/review 12", "/etc", "/mcp__github__issue title"] {
        let prepared = state
            .prepare_chat_command(&prepare_request(&fresh, forwarded), "user-1")
            .await
            .unwrap();
        assert_eq!((prepared.action, prepared.command.is_none()), (Prompt, true));
        assert_eq!(
            state
                .resolve_chat_command_prompt(fresh.clone(), forwarded, "user-1")
                .await
                .unwrap(),
            forwarded
        );
    }

    let session = claude_command_session(&state, &project).await;
    let start = claude_command_start_context(&state, &project, &session.id);
    chat_commands::persist_claude_commands(
        &start,
        Some(serde_json::json!({
            "names": ["check", "init", "compact", "mcp", "doctor"],
            "terminal": ["doctor"],
            "mcpServers": [{"name": "github", "status": "connected"}]
        })),
    )
    .await;
    let context = command_context(&project, Some(session.id));
    let catalog = state
        .chat_command_catalog(&context, "user-1", true)
        .await
        .unwrap();
    assert!(catalog.authoritative);
    assert!(catalog.warnings.is_empty());
    let find = |name: &str| catalog.commands.iter().find(|command| command.name == name);
    assert!(find("init").is_some_and(|command| command.verified && command.available));
    assert!(find("check").is_some_and(|command| command.verified));
    let doctor = find("doctor").unwrap();
    assert!(!doctor.available);
    assert_eq!(
        doctor.reason.as_deref(),
        Some("Terminal-only; not available in chat.")
    );

    let init = state
        .prepare_chat_command(&prepare_request(&context, "/init"), "user-1")
        .await
        .unwrap();
    assert_eq!(init.action, Prompt);
    assert!(init.command.is_some());
    for rejected in ["/totally-made-up", "/etc", "/doctor"] {
        assert!(
            state
                .prepare_chat_command(&prepare_request(&context, rejected), "user-1")
                .await
                .is_err(),
            "{rejected}"
        );
    }
    let mcp_prompt = state
        .prepare_chat_command(
            &prepare_request(&context, "/mcp__github__issue title"),
            "user-1",
        )
        .await
        .unwrap();
    assert_eq!((mcp_prompt.action, mcp_prompt.command.is_none()), (Prompt, true));
    let mcp = state
        .prepare_chat_command(&prepare_request(&context, "/mcp"), "user-1")
        .await
        .unwrap();
    assert_eq!(mcp.action, Mcp);
    assert_eq!(mcp.message.as_deref(), Some("github — connected"));
    assert!(
        state
            .prepare_chat_command(&prepare_request(&context, "/mcp reconnect"), "user-1")
            .await
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn chat_commands_mcp_status_is_unknown_until_claude_reports_it() {
    let (state, root, project) = temporary_app_state("commands-mcp-unknown").await;
    let session = claude_command_session(&state, &project).await;
    let context = command_context(&project, Some(session.id.clone()));
    let before = state
        .prepare_chat_command(&prepare_request(&context, "/mcp"), "user-1")
        .await
        .unwrap();
    assert_eq!(
        before.message.as_deref(),
        Some("MCP status is reported after the first message in this chat.")
    );
    let start = claude_command_start_context(&state, &project, &session.id);
    chat_commands::persist_claude_commands(
        &start,
        Some(serde_json::json!({"names": ["mcp"], "terminal": [], "mcpServers": []})),
    )
    .await;
    state
        .chat_command_catalog(&context, "user-1", true)
        .await
        .unwrap();
    let after = state
        .prepare_chat_command(&prepare_request(&context, "/mcp"), "user-1")
        .await
        .unwrap();
    assert_eq!(
        after.message.as_deref(),
        Some("No MCP servers are configured.")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn chat_commands_persisted_init_only_invalidates_catalogs_when_the_catalog_changes() {
    let (state, root, project) = temporary_app_state("commands-persist").await;
    let session = claude_command_session(&state, &project).await;
    let start = claude_command_start_context(&state, &project, &session.id);
    let init = |status: &str, names: &[&str]| {
        Some(serde_json::json!({
            "names": names, "terminal": [], "mcpServers": [{"name": "github", "status": status}]
        }))
    };
    let revision = || state.storage.get_setting("chat.commands.revision").unwrap();
    chat_commands::persist_claude_commands(&start, init("pending", &["init"])).await;
    let first = revision();
    assert!(first.is_some());
    chat_commands::persist_claude_commands(&start, init("pending", &["init"])).await;
    assert_eq!(revision(), first);
    chat_commands::persist_claude_commands(&start, init("connected", &["init"])).await;
    assert_eq!(revision(), first);
    let stored = state
        .storage
        .get_setting(&format!("chat.commands.claude.{}", session.id))
        .unwrap()
        .unwrap();
    assert_eq!(stored["mcpServers"][0]["status"], "connected");
    chat_commands::persist_claude_commands(&start, init("connected", &["init", "review"])).await;
    assert_ne!(revision(), first);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn chat_commands_gemini_never_validates_slash_names() {
    let (state, root, project) = temporary_app_state("commands-gemini").await;
    let mut context = command_context(&project, None);
    context.provider = Provider::Gemini;
    let catalog = state
        .chat_command_catalog(&context, "user-1", true)
        .await
        .unwrap();
    assert!(!catalog.authoritative);
    assert!(catalog.commands.iter().all(|command| command.name != "mcp"));
    let prepared = state
        .prepare_chat_command(&prepare_request(&context, "/memory show"), "user-1")
        .await
        .unwrap();
    assert_eq!(prepared.action, iowb_protocol::ChatCommandAction::Prompt);
    assert!(prepared.command.is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn chat_commands_busy_session_disables_native_commands_but_not_app_actions() {
    let (state, root, project) = temporary_app_state("commands-busy").await;
    let directory = project.join(".claude/commands");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("check.md"), "Check").unwrap();
    let session = claude_command_session(&state, &project).await;
    let context = command_context(&project, Some(session.id.clone()));
    state.sessions.set_active(&session.id, true).await.unwrap();
    let catalog = state
        .chat_command_catalog(&context, "user-1", true)
        .await
        .unwrap();
    let find = |name: &str| catalog.commands.iter().find(|command| command.name == name);
    let check = find("check").unwrap();
    assert!(!check.available);
    assert_eq!(
        check.reason.as_deref(),
        Some("Wait for the active response to finish.")
    );
    assert!(find("help").unwrap().available);
    assert!(find("mcp").unwrap().available);
    assert!(
        state
            .prepare_chat_command(&prepare_request(&context, "/check now"), "user-1")
            .await
            .is_err()
    );
    assert!(
        state
            .prepare_chat_command(&prepare_request(&context, "/help"), "user-1")
            .await
            .is_ok()
    );
    state.sessions.set_active(&session.id, false).await.unwrap();
    let catalog = state
        .chat_command_catalog(&context, "user-1", true)
        .await
        .unwrap();
    assert!(
        catalog
            .commands
            .iter()
            .any(|command| command.name == "check" && command.available)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn chat_commands_app_actions_cannot_be_shadowed_by_extension_files() {
    use iowb_protocol::{ChatCommandAction, ChatCommandKind};

    let (state, root, project) = temporary_app_state("commands-shadow").await;
    let directory = project.join(".claude/commands");
    std::fs::create_dir_all(&directory).unwrap();
    for name in ["help", "mcp", "compact", "clear"] {
        std::fs::write(directory.join(format!("{name}.md")), "Shadow").unwrap();
    }
    let catalog = state
        .chat_command_catalog(&command_context(&project, None), "user-1", true)
        .await
        .unwrap();
    let find = |name: &str| {
        catalog
            .commands
            .iter()
            .find(|command| command.name == name)
            .unwrap()
    };
    assert_eq!(find("help").source, "workbench");
    assert_eq!(find("clear").action, ChatCommandAction::New);
    assert_eq!((find("mcp").source.as_str(), find("mcp").action), ("workbench", ChatCommandAction::Mcp));
    assert_eq!(find("compact").source, "claude");
    assert_eq!(find("compact").kind, ChatCommandKind::NativeOperation);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn chat_commands_codex_mcp_reports_runtime_status() {
    use std::os::unix::fs::PermissionsExt;

    let (mut state, root, project) = temporary_app_state("commands-codex-mcp").await;
    let script = root.join("fake-codex-mcp.sh");
    let status = serde_json::json!({"id": 2, "result": {"data": [
        {"name": "fixture-mcp", "runtimeStatus": "connected", "authStatus": "unsupported", "tools": {}, "resources": [], "resourceTemplates": []},
        {"name": "idle-mcp", "runtimeStatus": null, "authStatus": "unknown", "tools": {}, "resources": [], "resourceTemplates": []}
    ], "nextCursor": "more"}});
    std::fs::write(&script, format!("#!/bin/sh\nread first\nprintf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"fixture\"}}}}'\nread second\nread third\nprintf '%s\\n' '{status}'\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    state.codex_app_server = CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
    let mut context = command_context(&project, None);
    context.provider = Provider::Codex;
    let mcp = state
        .prepare_chat_command(&prepare_request(&context, "/mcp"), "user-1")
        .await
        .unwrap();
    assert_eq!(mcp.action, iowb_protocol::ChatCommandAction::Mcp);
    assert_eq!(
        mcp.message.as_deref(),
        Some("fixture-mcp — connected\nidle-mcp — configured\nMore servers are not shown.")
    );
    std::fs::remove_dir_all(root).unwrap();
}

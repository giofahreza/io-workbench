    #[test]
    fn claude_prefixed_minimax_model_uses_cli_runtime_with_gateway_env() {
        assert_eq!(
            effective_agent_command_provider(Provider::Claude, Some("min:MiniMax-M3")),
            Provider::Claude
        );
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("min:MiniMax-M3")
        ));
        assert!(should_force_claude_cli_io_gateway(
            Provider::Claude,
            Some("min:MiniMax-M3")
        ));

        let args = default_agent_args_with(
            Provider::Claude,
            "pwd",
            Some("bypass"),
            None,
            None,
            Some("min:MiniMax-M3"),
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", "min:MiniMax-M3"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--setting-sources", "project,local"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--permission-mode", "bypassPermissions"]),
            "args: {args:?}"
        );
    }

    #[test]
    fn claude_unprefixed_model_uses_local_cli_runtime() {
        assert_eq!(
            effective_agent_command_provider(Provider::Claude, Some("claude-sonnet-4-5")),
            Provider::Claude
        );
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("claude-sonnet-4-5")
        ));

        let args = default_agent_args_with(
            Provider::Claude,
            "inspect the repo",
            Some("accept-edits"),
            Some("high"),
            None,
            Some("claude-sonnet-4-5"),
        );
        assert!(args.contains(&"--print".to_string()), "args: {args:?}");
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", "claude-sonnet-4-5"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--permission-mode", "acceptEdits"]),
            "args: {args:?}"
        );
        assert_eq!(args.last().map(String::as_str), Some("inspect the repo"));
    }

    #[test]
    fn claude_prefixed_model_uses_cli_runtime_with_prefixed_gateway_model_arg() {
        assert_eq!(
            effective_agent_command_provider(Provider::Claude, Some("cld:claude-sonnet-5")),
            Provider::Claude
        );
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("cld:claude-sonnet-5")
        ));
        assert!(should_force_claude_cli_io_gateway(
            Provider::Claude,
            Some("cld:claude-sonnet-5")
        ));

        let args = default_agent_args_with(
            Provider::Claude,
            "pwd",
            Some("bypass"),
            None,
            None,
            Some("cld:claude-sonnet-5"),
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", "cld:claude-sonnet-5"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--setting-sources", "project,local"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--permission-mode", "bypassPermissions"]),
            "args: {args:?}"
        );
        assert!(
            args.windows(2).any(|pair| pair == ["--tools", "default"]),
            "args: {args:?}"
        );
        assert!(
            args.iter().any(|arg| arg == "cld:claude-sonnet-5"),
            "args: {args:?}"
        );
    }

    #[test]
    fn claude_bypass_permissions_alias_enables_bypass_mode() {
        let args = default_agent_args_with(
            Provider::Claude,
            "pwd",
            Some("bypass-permissions"),
            None,
            None,
            Some("cld:claude-sonnet-5"),
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--permission-mode", "bypassPermissions"]),
            "args: {args:?}"
        );
        assert!(
            args.contains(&"--dangerously-skip-permissions".to_string()),
            "args: {args:?}"
        );
        assert!(
            args.windows(2).any(|pair| pair == ["--tools", "default"]),
            "args: {args:?}"
        );
    }

    #[test]
    fn claude_unprefixed_alias_uses_local_cli_runtime() {
        let args = default_agent_args_with(
            Provider::Claude,
            "pwd",
            Some("bypass"),
            None,
            None,
            Some("sonnet"),
        );
        assert!(
            args.windows(2).any(|pair| pair == ["--model", "sonnet"]),
            "args: {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg == "--setting-sources"),
            "args: {args:?}"
        );
        assert!(
            !should_use_direct_ai_gateway_runtime(Provider::Claude, Some("sonnet")),
            "args: {args:?}"
        );
        assert_eq!(args.last().map(String::as_str), Some("pwd"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn gemini_gateway_model_calls_direct_ai_api() {
        assert_gateway_model_calls_direct_ai_api(
            Provider::Gemini,
            "agw:gemini-3.6-flash-medium",
            "/v1/chat/completions",
        )
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn direct_ai_failure_persists_assistant_error_message() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake gateway");
        let gateway_addr = listener.local_addr().expect("gateway address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept gateway request");
            let mut buffer = [0u8; 1024];
            let _ = stream
                .read(&mut buffer)
                .await
                .expect("read gateway request");
            let body = r#"{"error":"upstream unavailable"}"#;
            let response = format!(
                "HTTP/1.1 502 Bad Gateway\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write gateway error response");
        });

        let root = std::env::temp_dir().join(format!("iowb-direct-ai-fail-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");
        let state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");

        let session = state
            .start_agent_session(
                Provider::Gemini,
                project.display().to_string(),
                "trigger failure",
                None,
                Some("agw:gemini-3.6-flash-medium".to_string()),
                None,
                None,
                None,
                None,
                ChatRuntime::NativeCli,
                Some(DirectAiRuntimeConfig {
                    base_url: format!("http://{gateway_addr}"),
                    api_key: "test-key".to_string(),
                    max_tokens: Some(32),
                }),
                None,
            )
            .await
            .expect("direct gateway session starts");

        let mut persisted_error = None;
        for _ in 0..20 {
            let messages = state.sessions.messages(&session.id).expect("messages");
            persisted_error = messages.into_iter().find(|message| {
                message.role == MessageRole::Assistant
                    && message.content.contains("Direct AI gateway request failed")
            });
            if persisted_error.is_some() {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }

        let assistant = persisted_error.expect("persisted assistant error");
        assert!(assistant.content.contains("502 Bad Gateway"));
        assert_eq!(assistant.metadata["status"], "failed");
        assert_eq!(assistant.metadata["cli"], "gemini");

        let _ = std::fs::remove_dir_all(root);
    }

    async fn assert_gateway_model_calls_direct_ai_api(
        provider: Provider,
        model: &str,
        expected_path: &str,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake gateway");
        let gateway_addr = listener.local_addr().expect("gateway address");
        let (request_tx, request_rx) = oneshot::channel::<String>();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept gateway request");
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 1024];
            let mut header_end = None;
            let mut content_length = 0usize;
            loop {
                let read = stream.read(&mut chunk).await.expect("read gateway request");
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if header_end.is_none() {
                    if let Some(index) = buffer.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        header_end = Some(index + 4);
                        let headers = String::from_utf8_lossy(&buffer[..index]);
                        content_length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                    }
                }
                if let Some(end) = header_end {
                    if buffer.len() >= end + content_length {
                        break;
                    }
                }
            }

            let request = String::from_utf8_lossy(&buffer).to_string();
            let _ = request_tx.send(request);
            let body = r#"{"content":[{"type":"text","text":"direct:ok"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write gateway response");
        });

        let root = std::env::temp_dir().join(format!("iowb-direct-ai-test-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        std::fs::create_dir_all(&project).expect("project dir");
        let state = AppState::initialize(AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");

        let existing_session = state
            .sessions
            .create_or_update(
                provider,
                project.display().to_string(),
                None,
                false,
                Some(model.to_string()),
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("existing session");
        state
            .sessions
            .append_message(&existing_session.id, MessageRole::User, "earlier question")
            .await
            .expect("earlier user message");
        state
            .sessions
            .append_message(
                &existing_session.id,
                MessageRole::Assistant,
                "earlier answer",
            )
            .await
            .expect("earlier assistant message");

        let session = state
            .start_agent_session(
                provider,
                project.display().to_string(),
                "reply ok",
                Some(existing_session.id.clone()),
                Some(model.to_string()),
                None,
                None,
                None,
                None,
                ChatRuntime::NativeCli,
                Some(DirectAiRuntimeConfig {
                    base_url: format!("http://{gateway_addr}"),
                    api_key: "test-key".to_string(),
                    max_tokens: Some(32),
                }),
                None,
            )
            .await
            .expect("direct gateway session starts");

        let request = request_rx.await.expect("captured gateway request");
        assert!(
            request.starts_with(&format!("POST {expected_path} ")),
            "{request}"
        );
        assert!(
            request.contains("authorization: Bearer test-key"),
            "{request}"
        );
        assert!(
            request.contains(&format!(r#""model":"{model}""#)),
            "{request}"
        );
        assert!(request.contains(r#""max_tokens":32"#), "{request}");
        let request_body = request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .expect("request body");
        let request_body: Value = serde_json::from_str(request_body).expect("gateway request JSON");
        assert_eq!(
            request_body["messages"],
            serde_json::json!([
                {"role": "user", "content": "earlier question"},
                {"role": "assistant", "content": "earlier answer"},
                {"role": "user", "content": "reply ok"},
            ])
        );

        let mut saw_output = false;
        for _ in 0..20 {
            let messages = state.sessions.messages(&session.id).expect("messages");
            if messages.iter().any(|message| {
                message.role == MessageRole::Assistant && message.content.contains("direct:ok")
            }) {
                saw_output = true;
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }

        let messages = state.sessions.messages(&session.id).expect("messages");
        let assistant = messages
            .iter()
            .find(|message| {
                message.role == MessageRole::Assistant && message.content.contains("direct:ok")
            })
            .expect("assistant message");
        assert_eq!(assistant.metadata["cli"], provider.as_str());
        assert_eq!(assistant.metadata["model"], model);

        let _ = std::fs::remove_dir_all(root);
        assert!(saw_output);
    }

    #[test]
    fn codex_accept_edits_uses_sandbox_flag() {
        let args = default_agent_args_with(
            Provider::Codex,
            "hi",
            Some("accept-edits"),
            None,
            None,
            None,
        );
        eprintln!("codex accept-edits args: {:?}", args);
        assert!(
            !args.iter().any(|a| a == "--approval-mode"),
            "must not pass --approval-mode: {:?}",
            args
        );
        assert!(args.contains(&"--sandbox".to_string()), "args: {:?}", args);
        assert!(
            args.contains(&"workspace-write".to_string()),
            "args: {:?}",
            args
        );
    }

    #[test]
    fn external_provider_sessions_use_native_resume_arguments() {
        let session_id = "11111111-1111-4111-8111-111111111111";

        let claude = default_agent_args_with_resume(
            Provider::Claude,
            "continue",
            Some("plan"),
            None,
            None,
            None,
            None,
            Some(session_id),
            ChatRuntime::NativeCli,
        );
        assert!(
            claude
                .windows(2)
                .any(|args| args == ["--resume", session_id])
        );
        assert_eq!(
            &claude[..5],
            [
                "--print",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
            ]
        );
        let prompt_index = claude.iter().position(|arg| arg == "continue").unwrap();
        let resume_index = claude
            .windows(2)
            .position(|args| args == ["--resume", session_id])
            .unwrap();
        assert_eq!(
            &claude[claude.len() - 3..],
            ["--resume", session_id, "continue"],
            "claude args: {claude:?}"
        );
        let permission_index = claude
            .windows(2)
            .position(|args| args == ["--permission-mode", "plan"])
            .unwrap();
        assert!(
            permission_index < resume_index && resume_index + 2 == prompt_index,
            "claude args: {claude:?}"
        );

        let codex = default_agent_args_with_resume(
            Provider::Codex,
            "continue",
            Some("plan"),
            None,
            None,
            None,
            None,
            Some(session_id),
            ChatRuntime::NativeCli,
        );
        let resume_index = codex.iter().position(|arg| arg == "resume").unwrap();
        let sandbox_index = codex.iter().position(|arg| arg == "--sandbox").unwrap();
        assert!(sandbox_index < resume_index, "codex args: {codex:?}");
        assert_eq!(&codex[resume_index..], ["resume", session_id, "continue"]);

        let gemini = default_agent_args_with_resume(
            Provider::Gemini,
            "continue",
            None,
            None,
            None,
            None,
            None,
            Some(session_id),
            ChatRuntime::NativeCli,
        );
        assert!(
            gemini
                .windows(2)
                .any(|args| args == ["--resume", session_id])
        );
        assert!(
            gemini
                .windows(2)
                .any(|args| args == ["--prompt", "continue"])
        );
    }

    #[test]
    fn claude_and_gemini_keep_native_slash_commands_unchanged() {
        for provider in [Provider::Claude, Provider::Gemini] {
            assert_eq!(
                resolve_cli_slash_prompt(provider, "/compact preserve decisions").unwrap(),
                "/compact preserve decisions"
            );
        }
    }

    #[test]
    fn codex_expands_custom_slash_prompts_for_headless_exec() {
        let root =
            std::env::temp_dir().join(format!("iowb-codex-slash-prompt-{}", uuid::Uuid::new_v4()));
        let prompts = root.join("prompts");
        std::fs::create_dir_all(&prompts).expect("prompt directory");
        std::fs::write(
            prompts.join("draftpr.md"),
            "---\ndescription: Draft a PR\n---\nReview $1 for $FOCUS. Args: $ARGUMENTS. Cost: $$5.",
        )
        .expect("custom prompt");

        let expanded = resolve_codex_slash_prompt(
            "/prompts:draftpr src/lib.rs FOCUS=\"error handling\"",
            Some(&root),
        )
        .expect("slash prompt expands");

        assert_eq!(
            expanded,
            "Review src/lib.rs for error handling. Args: src/lib.rs FOCUS=\"error handling\". Cost: $5."
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_slash_skill_uses_headless_skill_mention() {
        let root =
            std::env::temp_dir().join(format!("iowb-codex-slash-skill-{}", uuid::Uuid::new_v4()));
        let skill = root.join("skills").join("security-review");
        std::fs::create_dir_all(&skill).expect("skill directory");
        std::fs::write(skill.join("SKILL.md"), "# Security review").expect("skill");

        assert_eq!(
            resolve_codex_slash_prompt("/security-review staged changes", Some(&root)).unwrap(),
            "$security-review staged changes"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_bypass_uses_bypass_flag() {
        let args = default_agent_args_with(Provider::Codex, "hi", Some("bypass"), None, None, None);
        eprintln!("codex bypass args: {:?}", args);
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    }

    #[test]
    fn codex_proxy_model_uses_isolated_gateway_cli_provider() {
        let args = default_agent_args_with(
            Provider::Codex,
            "hi",
            None,
            Some("medium"),
            None,
            Some("agw:claude-opus-4-6-thinking"),
        );
        eprintln!("codex proxy model args: {:?}", args);
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"agw:claude-opus-4-6-thinking".to_string()));
        assert!(args.iter().any(|a| a == "model_provider=iowb_gateway"));
        assert!(!args.iter().any(|a| a == "model_provider=openai"));
    }

    #[test]
    fn codex_minimax_alias_uses_isolated_gateway_cli_provider() {
        let args = default_agent_args_with(
            Provider::Codex,
            "hi",
            None,
            Some("medium"),
            None,
            Some("min:MiniMax-M3"),
        );
        assert!(args.iter().any(|arg| arg == "--model"));
        assert!(args.iter().any(|arg| arg == "model_provider=iowb_gateway"));
        assert!(!args.iter().any(|arg| arg == "model_provider=openai"));
        assert!(args.iter().any(|arg| arg == "min:MiniMax-M3"));
        assert!(!args.iter().any(|arg| arg == "model_provider=minimax"));
    }

    #[test]
    fn codex_effort_uses_reasoning_config_without_forcing_an_old_model() {
        let args = default_agent_args_with(Provider::Codex, "hi", None, Some("medium"), None, None);
        assert!(!args.iter().any(|arg| arg == "model_provider=aiproxy"));
        assert!(!args.iter().any(|arg| arg.starts_with("model_provider=")));
        assert!(!args.iter().any(|arg| arg == "--model"));
        assert!(
            args.windows(2)
                .any(|pair| { pair == ["-c", "model_reasoning_effort=\"medium\""] })
        );

        let thinking_args = default_agent_args_with(
            Provider::Codex,
            "hi",
            None,
            Some("medium"),
            Some(true),
            None,
        );
        assert!(
            thinking_args
                .windows(2)
                .any(|pair| { pair == ["-c", "model_reasoning_effort=\"xhigh\""] })
        );
        assert!(!thinking_args.iter().any(|arg| arg == "--reasoning-effort"));
    }

    #[test]
    fn codex_extended_efforts_are_forwarded_without_downgrade() {
        for effort in ["xhigh", "max", "ultra"] {
            let args = default_agent_args_with(
                Provider::Codex,
                "hi",
                None,
                Some(effort),
                Some(true),
                Some("gpt-5.6++"),
            );
            let expected = format!("model_reasoning_effort=\"{effort}\"");
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["-c", expected.as_str()])
            );
            assert_eq!(
                args.iter()
                    .filter(|arg| arg.starts_with("model_reasoning_effort="))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn codex_unprefixed_model_uses_local_cli_provider() {
        let args =
            default_agent_args_with(Provider::Codex, "hi", None, None, None, Some("gpt-5.4"));
        eprintln!("codex real model args: {:?}", args);
        assert!(args.contains(&"gpt-5.4".to_string()));
        assert!(!args.iter().any(|a| a == "model_provider=aiproxy"));
        assert!(!args.iter().any(|arg| arg.starts_with("model_provider=")));
        assert!(args.contains(&"--skip-git-repo-check".to_string()));
    }

    #[test]
    fn codex_legacy_model_is_ignored_for_local_cli() {
        let args =
            default_agent_args_with(Provider::Codex, "hi", None, None, None, Some("gpt-5-codex"));

        assert!(!args.iter().any(|arg| arg == "--model"));
        assert!(!args.iter().any(|arg| arg == "gpt-5-codex"));
        assert!(!args.iter().any(|arg| arg == "model_provider=aiproxy"));
        assert!(!args.iter().any(|arg| arg.starts_with("model_provider=")));
    }

    #[test]
    fn gateway_model_routes_by_model_family_for_claude_and_gemini_selection() {
        assert_eq!(
            effective_agent_command_provider(Provider::Claude, Some("cod:gpt-5.5")),
            Provider::Codex
        );
        assert_eq!(
            effective_agent_command_provider(Provider::Gemini, Some("agw:gemini-3.6-flash-medium")),
            Provider::Gemini
        );
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("cod:gpt-5.5")
        ));
        assert!(should_use_direct_ai_gateway_runtime(
            Provider::Gemini,
            Some("agw:gemini-3.6-flash-medium")
        ));
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("cld:claude-haiku-4-5-20251001")
        ));
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("min:MiniMax-M3")
        ));
        assert!(should_force_claude_cli_io_gateway(
            Provider::Claude,
            Some("gateway:claude-haiku-4-5-20251001")
        ));
        assert!(should_force_claude_cli_io_gateway(
            Provider::Claude,
            Some("min:MiniMax-M3")
        ));
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            Some("claude-sonnet-4-5")
        ));
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Claude,
            None
        ));
        assert!(!should_use_direct_ai_gateway_runtime(
            Provider::Codex,
            Some("cod:gpt-5.5")
        ));
        assert!(should_force_codex_cli_io_gateway(Some("cod:gpt-5.5")));
        let args = default_agent_args_with(
            effective_agent_command_provider(Provider::Codex, Some("cod:gpt-5.5")),
            "hi",
            None,
            None,
            None,
            Some("cod:gpt-5.5"),
        );
        assert!(args.contains(&"exec".to_string()));
        assert!(args.contains(&"cod:gpt-5.5".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.iter().any(|a| a == "model_provider=iowb_gateway"));
        assert!(!args.iter().any(|a| a == "model_provider=openai"));
    }

    #[test]
    fn codex_gateway_runtime_builds_complete_ephemeral_provider_config() {
        let mut args = default_agent_args_with_resume(
            Provider::Codex,
            "hi",
            None,
            None,
            None,
            None,
            Some("gpt-custom"),
            None,
            ChatRuntime::IoGateway,
        );
        apply_codex_cli_io_gateway_args(&mut args, "https://gateway.example.com/codex/");

        for expected in [
            "model_provider=iowb_gateway",
            "model_providers.iowb_gateway.name=\"IO Gateway\"",
            "model_providers.iowb_gateway.base_url=\"https://gateway.example.com/codex\"",
            "model_providers.iowb_gateway.env_key=\"IOWB_IO_GATEWAY_API_KEY\"",
            "model_providers.iowb_gateway.wire_api=\"responses\"",
        ] {
            assert!(args.iter().any(|arg| arg == expected), "args: {args:?}");
        }
    }

    #[test]
    fn codex_gateway_unprefixed_sol_keeps_gateway_provider_and_fast_tier() {
        let mut args = default_agent_args_with_resume(
            Provider::Codex,
            "hi",
            None,
            Some("medium"),
            None,
            Some(true),
            Some("gpt-5.6-sol"),
            None,
            ChatRuntime::IoGateway,
        );
        apply_codex_cli_io_gateway_args(&mut args, "https://ai.qif.us/codex");

        let model_index = args
            .iter()
            .position(|arg| arg == "--model")
            .expect("model flag");
        assert_eq!(
            args.get(model_index + 1).map(String::as_str),
            Some("gpt-5.6-sol")
        );
        for expected in [
            "model_provider=iowb_gateway",
            "model_providers.iowb_gateway.base_url=\"https://ai.qif.us/codex\"",
            "features.fast_mode=true",
            "service_tier=\"fast\"",
        ] {
            assert!(args.iter().any(|arg| arg == expected), "args: {args:?}");
        }
    }

    #[test]
    fn codex_gateway_provider_config_precedes_resume_positional() {
        let mut args = default_agent_args_with_resume(
            Provider::Codex,
            "continue",
            Some("bypass"),
            Some("medium"),
            None,
            None,
            Some("min:MiniMax-M3"),
            Some("native-session-id"),
            ChatRuntime::IoGateway,
        );
        apply_codex_cli_io_gateway_args(&mut args, "https://gateway.example.com/codex");

        let resume_index = args
            .iter()
            .position(|arg| arg == "resume")
            .expect("resume positional");
        for key in [
            "model_provider=",
            "model_providers.iowb_gateway.name=",
            "model_providers.iowb_gateway.base_url=",
            "model_providers.iowb_gateway.env_key=",
            "model_providers.iowb_gateway.wire_api=",
        ] {
            let config_index = args
                .iter()
                .position(|arg| arg.starts_with(key))
                .unwrap_or_else(|| panic!("missing {key} in {args:?}"));
            assert!(config_index < resume_index, "args: {args:?}");
        }
        assert_eq!(
            &args[resume_index..resume_index + 3],
            ["resume", "native-session-id", "continue"]
        );
    }

    #[test]
    fn codex_fast_setting_selects_fast_or_standard_before_resume() {
        for (fast, expected_tier) in [(true, "\"fast\""), (false, "\"default\"")] {
            let args = default_agent_args_with_resume(
                Provider::Codex,
                "continue",
                None,
                Some("medium"),
                None,
                Some(fast),
                Some("cod:gpt-5.6-sol"),
                Some("native-session-id"),
                ChatRuntime::IoGateway,
            );

            let resume_index = args
                .iter()
                .position(|arg| arg == "resume")
                .expect("resume positional");
            let fast_feature_index = args
                .iter()
                .position(|arg| arg == "features.fast_mode=true")
                .unwrap_or_else(|| panic!("missing Fast feature override in {args:?}"));
            let tier = format!("service_tier={expected_tier}");
            let tier_index = args
                .iter()
                .position(|arg| arg == &tier)
                .unwrap_or_else(|| panic!("missing {tier} in {args:?}"));

            assert!(fast_feature_index < resume_index, "args: {args:?}");
            assert!(tier_index < resume_index, "args: {args:?}");
            assert_eq!(
                &args[resume_index..resume_index + 3],
                ["resume", "native-session-id", "continue"]
            );
        }
    }

    #[test]
    fn codex_unspecified_fast_setting_inherits_cli_configuration() {
        let args = default_agent_args_with_resume(
            Provider::Codex,
            "hi",
            None,
            None,
            None,
            None,
            Some("gpt-5.6-sol"),
            None,
            ChatRuntime::NativeCli,
        );

        assert!(!args.iter().any(|arg| arg.starts_with("service_tier=")));
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("features.fast_mode="))
        );
    }

    #[test]
    fn native_runtime_does_not_override_codex_provider() {
        let args = default_agent_args_with_resume(
            Provider::Codex,
            "hi",
            None,
            None,
            None,
            None,
            None,
            None,
            ChatRuntime::NativeCli,
        );

        assert!(!args.iter().any(|arg| arg.starts_with("model_provider=")));
        assert!(!args.iter().any(|arg| arg == "--model"));
    }

    #[test]
    fn extracts_direct_ai_text_from_common_response_shapes() {
        let anthropic = serde_json::json!({
            "content": [{ "type": "text", "text": "hello" }]
        });
        assert_eq!(extract_direct_ai_response_text(&anthropic), "hello");

        let chat = serde_json::json!({
            "choices": [{ "message": { "content": "world" } }]
        });
        assert_eq!(extract_direct_ai_response_text(&chat), "world");

        let responses = serde_json::json!({
            "output": [{ "content": [{ "text": "done" }] }]
        });
        assert_eq!(extract_direct_ai_response_text(&responses), "done");
    }

    #[test]
    fn extracts_direct_ai_stream_deltas_from_common_sse_shapes() {
        let chat = serde_json::json!({
            "choices": [{ "delta": { "content": "hel" } }]
        });
        assert_eq!(extract_direct_ai_stream_delta(&chat), "hel");

        let anthropic = serde_json::json!({
            "type": "content_block_delta",
            "delta": { "type": "text_delta", "text": "lo" }
        });
        assert_eq!(extract_direct_ai_stream_delta(&anthropic), "lo");

        let responses = serde_json::json!({
            "type": "response.output_text.delta",
            "delta": "!"
        });
        assert_eq!(extract_direct_ai_stream_delta(&responses), "!");
    }

    #[test]
    fn direct_ai_display_chunks_round_trip_text() {
        let text =
            "alpha beta gamma\nSTREAM_MOBILE_LINE_1\nSTREAM_MOBILE_LINE_2\nunicode: cepat bisa";
        let chunks = direct_ai_display_chunks(text);
        assert!(chunks.len() > 1);
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn native_models_keep_selected_provider_runtime() {
        assert_eq!(
            effective_agent_command_provider(Provider::Claude, Some("claude-sonnet-4-5")),
            Provider::Claude
        );
        assert_eq!(
            effective_agent_command_provider(Provider::Gemini, Some("gemini-2.5-pro")),
            Provider::Gemini
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replay_history_is_bounded_by_bytes() {
        let mut manager = AgentRuntimeManager::new(1);
        manager.max_replay_bytes = 900;
        let (abort_tx, _abort_rx) = oneshot::channel();
        let key = "codex:replay-test".to_string();
        manager.register(key.clone(), abort_tx).await;
        let hub = WsHub::new();
        for sequence in 1..=4 {
            manager
                .publish(
                    &hub,
                    &key,
                    WsServerEvent::Output {
                        provider: Provider::Codex,
                        session_id: "replay-test".to_string(),
                        response_id: Some("response-1".to_string()),
                        sequence: Some(sequence),
                        content: "x".repeat(400),
                        done: false,
                    },
                )
                .await;
        }

        let replay = manager.replay_events().await;
        assert_eq!(replay.len(), 1);
        assert!(
            replay.iter().map(ws_event_estimated_bytes).sum::<usize>() <= manager.max_replay_bytes
        );
        assert!(matches!(
            replay.last(),
            Some(WsServerEvent::Output {
                sequence: Some(4),
                ..
            })
        ));
    }

    #[test]
    fn looks_like_proxy_model_recognizes_known_prefixes() {
        assert!(looks_like_proxy_model("agw:claude-opus-4-6-thinking"));
        assert!(looks_like_proxy_model("cod:gpt-5.4-mini"));
        assert!(looks_like_proxy_model("AGW:foo"));
        assert!(looks_like_proxy_model("cld:claude-haiku-4-5-20251001"));
        assert!(looks_like_proxy_model("gem:gemini-2.5-pro"));
        assert!(looks_like_proxy_model("cop:gpt-4o"));
        assert!(looks_like_proxy_model("proxy:bar"));
        assert!(!looks_like_proxy_model("gpt-5-codex"));
        assert!(!looks_like_proxy_model("claude-sonnet-4-5"));
        assert!(!looks_like_proxy_model("o4-mini"));
        assert!(!looks_like_proxy_model("unknown:model"));
    }

    #[test]
    fn server_id_is_persisted_per_config_directory() {
        let root = env::temp_dir().join(format!("iowb-server-id-{}", Uuid::new_v4()));
        let config_dir = root.join("config");
        let config = AppConfig {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        };

        let server_id = config.server_id();
        assert!(server_id.starts_with("iowb_"));
        assert_eq!(config.server_id(), server_id);
        assert!(config_dir.join("server-id").is_file());

        let second_config = AppConfig {
            config_dir: root.join("other-config"),
            database_path: root.join("other-config/test.db"),
            ..config
        };
        assert_ne!(second_config.server_id(), server_id);

        std::fs::remove_dir_all(root).expect("test config cleanup");
    }

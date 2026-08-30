    #[test]
    fn bounds_pathological_history_for_clients_and_prioritizes_newest_messages() {
        let now = Utc::now();
        let messages = vec![
            ChatMessage {
                id: "older-tool".to_string(),
                role: MessageRole::Tool,
                content: format!("<style>bad</style>\0{}", "a".repeat(900_000)),
                timestamp: now,
                metadata: serde_json::json!({"huge": "m".repeat(100_000)}),
            },
            ChatMessage {
                id: "latest-assistant".to_string(),
                role: MessageRole::Assistant,
                content: format!("{}FINAL-TAIL", "b".repeat(600_000)),
                timestamp: now,
                metadata: Value::Null,
            },
        ];

        let bounded = bound_session_messages_for_response(messages);
        assert_eq!(bounded.len(), 2);
        assert!(
            bounded
                .iter()
                .map(|message| message.content.len())
                .sum::<usize>()
                <= SESSION_RESPONSE_MAX_CONTENT_BYTES
        );
        assert!(bounded[0].content.len() <= SESSION_RESPONSE_TOOL_MAX_BYTES);
        assert!(bounded[1].content.len() <= SESSION_RESPONSE_ASSISTANT_MAX_BYTES);
        assert!(bounded[1].content.contains("FINAL-TAIL"));
        assert!(!bounded[0].content.contains('\0'));
        assert!(bounded.iter().all(|message| {
            message
                .content
                .lines()
                .all(|line| line.chars().count() <= SESSION_RESPONSE_MAX_LINE_CHARS + 80)
        }));
        assert_eq!(
            bounded[0]
                .metadata
                .get("contentTruncated")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            bounded[0]
                .metadata
                .get("metadataTruncated")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn io_gateway_config_uses_provider_specific_endpoint_without_env_key() {
        let mut config = serde_json::json!({
            "mode": "anthropic",
            "gatewayUrl": "https://gateway.example.com",
            "apiKeyEnv": "ANTHROPIC_API_KEY",
            "maxTokens": 1234,
        });

        apply_io_gateway_config(&mut config, Provider::Claude);

        assert_eq!(config.get("mode").and_then(Value::as_str), Some("aiproxy"));
        assert_eq!(
            config.get("baseUrl").and_then(Value::as_str),
            Some("https://gateway.example.com/claude")
        );
        assert!(config.get("apiKeyEnv").is_none());
        assert_eq!(config.get("maxTokens").and_then(Value::as_u64), Some(1234));
        apply_io_gateway_config(&mut config, Provider::Codex);
        assert_eq!(
            config.get("baseUrl").and_then(Value::as_str),
            Some("https://gateway.example.com/codex")
        );
    }

    #[test]
    fn io_gateway_config_does_not_duplicate_endpoint_inclusive_urls() {
        let mut codex = serde_json::json!({
            "gatewayUrl": "https://ai.qif.us/codex/",
        });
        apply_io_gateway_config(&mut codex, Provider::Codex);
        assert_eq!(
            codex.get("baseUrl").and_then(Value::as_str),
            Some("https://ai.qif.us/codex")
        );

        let mut claude = serde_json::json!({
            "gatewayUrl": "https://ai.qif.us/claude",
        });
        apply_io_gateway_config(&mut claude, Provider::Claude);
        assert_eq!(
            claude.get("baseUrl").and_then(Value::as_str),
            Some("https://ai.qif.us/claude")
        );

        let mut legacy_base = serde_json::json!({
            "baseUrl": "https://gateway.example.com/api/codex/",
        });
        apply_io_gateway_config(&mut legacy_base, Provider::Codex);
        assert_eq!(
            legacy_base.get("baseUrl").and_then(Value::as_str),
            Some("https://gateway.example.com/api/codex")
        );
    }

    #[test]
    fn io_gateway_config_preserves_custom_and_absolute_endpoint_overrides() {
        let mut relative = serde_json::json!({
            "gatewayUrl": "https://gateway.example.com/root",
            "codexEndpoint": "api/codex",
        });
        apply_io_gateway_config(&mut relative, Provider::Codex);
        assert_eq!(
            relative.get("baseUrl").and_then(Value::as_str),
            Some("https://gateway.example.com/root/api/codex")
        );

        let mut absolute = serde_json::json!({
            "gatewayUrl": "https://gateway.example.com/root",
            "codexEndpoint": "https://codex.example.com/custom/",
        });
        apply_io_gateway_config(&mut absolute, Provider::Codex);
        assert_eq!(
            absolute.get("baseUrl").and_then(Value::as_str),
            Some("https://codex.example.com/custom")
        );

        assert_eq!(
            join_io_gateway_endpoint_url("https://gateway.example.com/mycodex", "codex"),
            "https://gateway.example.com/mycodex/codex"
        );
    }

    #[test]
    fn io_gateway_runtime_does_not_fall_back_to_environment_credentials() {
        let config = serde_json::json!({
            "mode": "aiproxy",
            "baseUrl": "https://gateway.example.com/claude",
            "apiKeyEnv": "PATH",
        });

        assert_eq!(direct_ai_endpoint_config(&config), None);
    }

    #[test]
    fn parses_supported_chat_runtime_values() {
        assert_eq!(
            parse_chat_runtime("native_cli"),
            Some(ChatRuntime::NativeCli)
        );
        assert_eq!(
            parse_chat_runtime("io_gateway"),
            Some(ChatRuntime::IoGateway)
        );
        assert_eq!(parse_chat_runtime("invalid"), None);
    }

    #[test]
    fn claude_fallback_models_include_local_cli_aliases() {
        let models = fallback_models(Provider::Claude);

        for alias in ["sonnet", "opus", "haiku", "fable"] {
            assert!(
                models.iter().any(|model| model == alias),
                "models: {models:?}"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn io_gateway_chat_models_returns_empty_success_when_catalog_unavailable() {
        let root =
            std::env::temp_dir().join(format!("iowb-server-chat-models-{}", uuid::Uuid::new_v4()));
        let config_dir = root.join("config");
        std::fs::create_dir_all(&config_dir).expect("config directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
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
        let user_id = "model-test-user";
        state
            .storage
            .set_setting(
                &user_setting_key(user_id, "direct-ai"),
                &serde_json::json!({
                    "chatRuntime": "io_gateway",
                    "mode": "aiproxy",
                    "gatewayUrl": "http://127.0.0.1:1",
                    "gatewayApiKey": "test-key",
                }),
            )
            .expect("setting");

        let Json(body) = chat_provider_models(
            State(state),
            Extension(AuthenticatedUser(iowb_protocol::UserProfile {
                id: user_id.to_string(),
                username: "model-test".to_string(),
                email: None,
                created_at: chrono::Utc::now(),
            })),
            Query(ChatModelsQuery {
                provider: Some("codex".to_string()),
            }),
        )
        .await
        .expect("models response");

        assert_eq!(body.get("success").and_then(Value::as_bool), Some(true));
        assert_eq!(
            body.get("gatewayAvailable").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            body.get("models").and_then(Value::as_array).map(Vec::len),
            Some(0)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn io_gateway_codex_chat_models_preserve_slug_ids_and_prefixed_aliases() {
        async fn models(headers: HeaderMap) -> impl IntoResponse {
            let bearer = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok());
            let api_key = headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok());
            if bearer != Some("Bearer stored-key") || api_key != Some("stored-key") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            Json(serde_json::json!({
                "models": [
                    {"slug": "gpt-5.6-sol", "display_name": "GPT-5.6-Sol"},
                    {"slug": "cod:gpt-5.6-sol", "display_name": "GPT-5.6-Sol (alias)"},
                    {
                        "slug": "gpt-5.6-sol-wm",
                        "display_name": "GPT-5.6-Sol-WM",
                        "visibility": "hide"
                    }
                ]
            }))
            .into_response()
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/codex/models", get(models))).await
        });

        let root = std::env::temp_dir().join(format!(
            "iowb-server-codex-chat-models-{}",
            uuid::Uuid::new_v4()
        ));
        let config_dir = root.join("config");
        std::fs::create_dir_all(&config_dir).expect("config directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
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
        let user_id = "codex-model-test-user";
        state
            .storage
            .set_setting(
                &user_setting_key(user_id, "direct-ai"),
                &serde_json::json!({
                    "chatRuntime": "io_gateway",
                    "mode": "aiproxy",
                    "gatewayUrl": format!("http://{address}"),
                    "gatewayApiKey": "stored-key",
                }),
            )
            .expect("setting");

        let Json(body) = chat_provider_models(
            State(state),
            Extension(AuthenticatedUser(iowb_protocol::UserProfile {
                id: user_id.to_string(),
                username: "codex-model-test".to_string(),
                email: None,
                created_at: chrono::Utc::now(),
            })),
            Query(ChatModelsQuery {
                provider: Some("codex".to_string()),
            }),
        )
        .await
        .expect("models response");

        let model_values: Vec<_> = body
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| {
                Some((model.get("value")?.as_str()?, model.get("label")?.as_str()?))
            })
            .collect();
        assert_eq!(
            model_values,
            [
                ("gpt-5.6-sol", "gpt-5.6-sol"),
                ("cod:gpt-5.6-sol", "cod:gpt-5.6-sol"),
            ],
            "gateway model ids must remain byte-for-byte visible and selectable"
        );
        assert_eq!(
            body.get("gatewayAvailable").and_then(Value::as_bool),
            Some(true)
        );

        server.abort();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_chat_models_merge_local_and_gateway_models() {
        let mut models = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let base_models = vec![
            "gpt-5".to_string(),
            "gpt-5.4".to_string(),
            "cod:gpt-5.5".to_string(),
            "min:MiniMax-M3".to_string(),
            "gpt-5-codex".to_string(),
        ];

        push_codex_chat_models(&mut models, &mut seen, base_models);
        for model in ["cod:gpt-5.5", "min:MiniMax-M3", "unknown:model"] {
            if is_io_gateway_model(model) {
                push_chat_model(&mut models, &mut seen, model);
            }
        }

        assert_eq!(
            models,
            vec![
                "gpt-5".to_string(),
                "gpt-5.4".to_string(),
                "cod:gpt-5.5".to_string(),
                "min:MiniMax-M3".to_string(),
            ]
        );
    }

    #[test]
    fn io_gateway_config_replaces_legacy_env_reference() {
        let mut config = serde_json::json!({
            "mode": "aiproxy",
            "baseUrl": "http://127.0.0.1:8319/claude",
            "apiKeyEnv": "WRONG_KEY",
        });

        apply_io_gateway_config(&mut config, Provider::Claude);

        assert_eq!(
            config.get("baseUrl").and_then(Value::as_str),
            Some("http://127.0.0.1:8319/claude")
        );
        assert!(config.get("apiKeyEnv").is_none());
    }

    #[test]
    fn forced_io_gateway_config_uses_stored_gateway_url() {
        let mut config = serde_json::json!({
            "mode": "aiproxy",
            "gatewayUrl": "https://gateway.example.com/root/",
            "gatewayApiKey": "stored-key",
        });

        apply_io_gateway_config(&mut config, Provider::Claude);

        assert_eq!(
            config.get("baseUrl").and_then(Value::as_str),
            Some("https://gateway.example.com/root/claude")
        );
        assert_eq!(
            direct_ai_endpoint_config(&config),
            Some((
                "https://gateway.example.com/root/claude".to_string(),
                "stored-key".to_string(),
            ))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn io_gateway_model_catalog_uses_stored_key_and_keeps_full_list() {
        async fn models(headers: HeaderMap) -> impl IntoResponse {
            let bearer = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok());
            let api_key = headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok());
            if bearer != Some("Bearer stored-key") || api_key != Some("stored-key") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            Json(serde_json::json!({
                "data": [
                    {"id": "gpt-5.4"},
                    {"id": "claude-sonnet-4-5"},
                    {"id": "minimax-m3"}
                ]
            }))
            .into_response()
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/claude/models", get(models))).await
        });
        let config = serde_json::json!({
            "mode": "aiproxy",
            "baseUrl": format!("http://{address}/claude"),
            "gatewayApiKey": "stored-key",
        });

        let models = fetch_direct_ai_models(&config).await.expect("models");
        let values: Vec<_> = models
            .iter()
            .filter_map(|model| model.get("value").and_then(Value::as_str))
            .collect();
        assert_eq!(values, ["gpt-5.4", "claude-sonnet-4-5", "minimax-m3"]);

        server.abort();
    }

    #[test]
    fn public_direct_ai_config_redacts_stored_secrets() {
        let config = serde_json::json!({
            "mode": "aiproxy",
            "gatewayUrl": "https://gateway.example.com",
            "gatewayApiKey": "private-key",
            "gatewayOtpSecret": "PRIVATEOTP",
        });

        let public = public_direct_ai_config(&config);

        assert!(public.get("gatewayApiKey").is_none());
        assert!(public.get("gatewayOtpSecret").is_none());
        assert_eq!(
            public
                .get("gatewayApiKeyConfigured")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            public.get("gatewayOtpConfigured").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn direct_ai_secret_reveal_is_explicit_and_scoped() {
        let resolved = serde_json::json!({
            "gatewayApiKey": "private-key",
            "gatewayOtpSecret": "PRIVATEOTP",
        });
        let mut status = serde_json::json!({"success": true});

        assert!(status.get("secrets").is_none());
        add_direct_ai_secrets(&mut status, &resolved);

        assert_eq!(
            status
                .get("secrets")
                .and_then(|secrets| secrets.get("gatewayApiKey"))
                .and_then(Value::as_str),
            Some("private-key")
        );
        assert_eq!(
            status
                .get("secrets")
                .and_then(|secrets| secrets.get("gatewayOtpSecret"))
                .and_then(Value::as_str),
            Some("PRIVATEOTP")
        );
    }

    #[test]
    fn direct_ai_updates_preserve_omitted_stored_secrets() {
        let stored = serde_json::json!({
            "gatewayApiKey": "private-key",
            "gatewayOtpSecret": "PRIVATEOTP",
        });
        let mut update = serde_json::json!({
            "mode": "aiproxy",
            "gatewayUrl": "https://new-gateway.example.com",
        });

        preserve_direct_ai_secrets(&stored, &mut update);

        assert_eq!(
            update.get("gatewayApiKey").and_then(Value::as_str),
            Some("private-key")
        );
        assert_eq!(
            update.get("gatewayOtpSecret").and_then(Value::as_str),
            Some("PRIVATEOTP")
        );
    }

    #[test]
    fn parses_total_claude_usage_without_double_counting_stream_updates() {
        let content = r#"
{"type":"assistant","message":{"id":"msg-1","usage":{"input_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0}}}
{"type":"assistant","message":{"id":"msg-1","usage":{"input_tokens":10,"cache_creation_input_tokens":20,"cache_read_input_tokens":30,"output_tokens":40}}}
{"type":"assistant","message":{"id":"msg-2","usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4}}}
"#;

        let usage = parse_claude_usage(content);
        assert_eq!(usage.used, 110);
        assert_eq!(usage.input, 11);
        assert_eq!(usage.output, 44);
        assert_eq!(usage.cache_creation, 22);
        assert_eq!(usage.cache_read, 33);
    }

    #[test]
    fn parses_latest_codex_usage() {
        let content = r#"
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":4,"cache_write_input_tokens":2,"output_tokens":2,"total_tokens":12},"model_context_window":1000}}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":30,"cached_input_tokens":12,"cache_write_input_tokens":3,"output_tokens":12,"total_tokens":42},"model_context_window":2000}}}
"#;

        let snapshot = parse_codex_usage(content);
        assert_eq!(snapshot.usage.used, 42);
        assert_eq!(snapshot.usage.input, 30);
        assert_eq!(snapshot.usage.output, 12);
        assert_eq!(snapshot.usage.cache_creation, 3);
        assert_eq!(snapshot.usage.cache_read, 12);
        assert_eq!(snapshot.total, 2000);
    }

    #[test]
    fn bounded_codex_tail_keeps_complete_recent_token_record() {
        let root =
            std::env::temp_dir().join(format!("iowb-codex-token-tail-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temporary directory");
        let path = root.join("rollout.jsonl");
        let token_line = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":30,"output_tokens":12,"total_tokens":42},"model_context_window":2000}}}"#;
        std::fs::write(
            &path,
            format!("{}\n{token_line}\n{{}}\n", "x".repeat(32 * 1024)),
        )
        .expect("rollout");

        let tail = read_file_tail(&path, token_line.len() as u64 + 128).expect("tail");
        assert!(tail.len() < 1024);
        let snapshot = parse_codex_usage(&tail);
        assert_eq!(snapshot.usage.used, 42);
        assert_eq!(snapshot.total, 2000);

        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn sanitizes_repository_names() {
        assert_eq!(
            repository_name("https://github.com/example/io-workbench.git"),
            "io-workbench"
        );
        assert_eq!(repository_name("git@github.com:example/repo.git"), "repo");
        assert_eq!(
            repository_name("https://example.com/a bad repo.git"),
            "abadrepo"
        );
        assert_eq!(
            repository_name_from_remote("git@github.com:example/mobile-app.git").as_deref(),
            Some("mobile-app")
        );
    }

    #[test]
    fn normalizes_project_watch_paths_and_ignores_git_metadata() {
        let root = "/tmp/project";
        assert_eq!(
            project_relative_watch_path(root, Path::new("/tmp/project/src/main.rs")).as_deref(),
            Some("src/main.rs"),
        );
        assert_eq!(
            project_relative_watch_path(root, Path::new("/tmp/project")).as_deref(),
            Some("."),
        );
        assert_eq!(
            project_relative_watch_path(root, Path::new("/tmp/project/.git/index")),
            None,
        );
        assert_eq!(
            project_relative_watch_path(root, Path::new("/tmp/another/file.txt")),
            None,
        );
    }

    #[test]
    fn project_watcher_bounds_home_and_generated_directories() {
        let home = Path::new("/home/tester");
        assert!(project_watch_is_broad_root(home, home, Some(home)));
        assert!(project_watch_is_broad_root(
            Path::new("/home"),
            home,
            Some(home),
        ));
        assert!(!project_watch_is_broad_root(
            Path::new("/home/tester/project"),
            home,
            Some(home),
        ));
        assert!(project_relative_path_is_excluded(
            Path::new("/work/project"),
            Path::new("/work/project/target/debug/build"),
        ));
        assert!(project_relative_path_is_excluded(
            Path::new("/work/project"),
            Path::new("/work/project/web/node_modules/package"),
        ));
        assert!(!project_relative_path_is_excluded(
            Path::new("/work/project"),
            Path::new("/work/project/src/features"),
        ));
    }

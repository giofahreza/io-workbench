#[test]
fn chat_model_preflight_handles_prefix_case_whitespace_and_injection() {
    for id in ["cod:gpt-6.1-sol", " COD:gpt-6.1-sol ", "cld:sonnet-4"] {
        assert_eq!(
            native_model_problem(ChatRuntime::NativeCli, Some(id)).unwrap()["code"],
            "chat_runtime_mismatch"
        );
    }
    for id in ["gpt-6.1", "sonnet", "opus", "gemini-2.5-pro", ""] {
        assert!(native_model_problem(ChatRuntime::NativeCli, Some(id)).is_none());
    }
    for id in [
        "unknown:foo",
        "$(touch /tmp/pwned)",
        "--model",
        "<script>",
        "a b",
    ] {
        assert!(native_model_problem(ChatRuntime::NativeCli, Some(id)).is_some());
    }
}

#[test]
fn route_model_checks_reject_stale_ids_and_preserve_existing_native_models() {
    let native = serde_json::json!({"models": [{"value": "", "label": "Native default"}, {"value": "gpt-test"}]});
    let gateway = serde_json::json!({"models": [{"value": "cod:gpt-test"}]});
    assert_eq!(
        checked_route_model(ChatRuntime::NativeCli, None, &native, None).unwrap(),
        None
    );
    assert_eq!(
        checked_route_model(
            ChatRuntime::NativeCli,
            Some("gpt-legacy"),
            &native,
            Some("gpt-legacy")
        )
        .unwrap()
        .as_deref(),
        Some("gpt-legacy")
    );
    assert!(
        checked_route_model(
            ChatRuntime::NativeCli,
            Some("gpt-unavailable"),
            &native,
            None
        )
        .is_err()
    );
    assert!(
        checked_route_model(
            ChatRuntime::NativeCli,
            Some("cod:gpt-test"),
            &native,
            Some("cod:gpt-test")
        )
        .is_err()
    );
    assert!(checked_route_model(ChatRuntime::IoGateway, None, &gateway, None).is_err());
    assert!(
        checked_route_model(
            ChatRuntime::IoGateway,
            Some("unknown:model"),
            &gateway,
            None
        )
        .is_err()
    );
    assert_eq!(
        checked_route_model(
            ChatRuntime::IoGateway,
            Some(" COD:GPT-TEST "),
            &gateway,
            None
        )
        .unwrap()
        .as_deref(),
        Some("cod:gpt-test")
    );
}

#[tokio::test]
async fn missing_native_routes_never_offer_cli_default() {
    let (root, state) = board_scope_test_state().await;
    for provider in [Provider::Codex, Provider::Claude, Provider::Gemini] {
        for installed in [false, true] {
            let body = chat_route_catalog_with_status(
                &state,
                "test",
                provider,
                ChatRuntime::NativeCli,
                serde_json::json!({"installed": installed, "nativeReady": false}),
            )
            .await;
            assert_eq!(body["ready"], false);
            assert_eq!(body["models"], serde_json::json!([]));
            assert_eq!(
                body["problem"]["code"],
                if installed {
                    "provider_auth_required"
                } else {
                    "provider_cli_unavailable"
                }
            );
        }
    }
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn gateway_readiness_is_user_scoped_and_needs_cli_but_not_native_login() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let gateway = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/{*path}", get(|headers: HeaderMap| async move {
            assert_eq!(headers["authorization"], "Bearer fake-key");
            Json(serde_json::json!({ "data": [
                {"id": "cod:gpt-test", "label": "First"}, {"id": "COD:GPT-TEST", "label": "Duplicate"},
                {"id": "cod:hidden", "visibility": "HIDE"}, {"id": "cld:sonnet-test"}
            ] }))
        }))).await.unwrap();
    });
    let (root, state) = board_scope_test_state().await;
    state.storage.set_setting(&user_setting_key("alice", "direct-ai"), &serde_json::json!({
        "mode": "aiproxy", "gatewayUrl": format!("http://{address}"), "gatewayApiKey": "fake-key"
    })).unwrap();
    for provider in [Provider::Codex, Provider::Claude] {
        let configured = chat_route_catalog_with_status(
            &state,
            "alice",
            provider,
            ChatRuntime::IoGateway,
            serde_json::json!({"installed": true, "nativeReady": false}),
        )
        .await;
        assert_eq!(configured["ready"], true);
        assert_eq!(configured["models"].as_array().unwrap().len(), 2);
        assert!(!configured.to_string().contains("fake-key"));
        let missing = chat_route_catalog_with_status(
            &state,
            "alice",
            provider,
            ChatRuntime::IoGateway,
            serde_json::json!({"installed": false, "nativeReady": false}),
        )
        .await;
        assert_eq!(missing["gatewayAvailable"], true);
        assert_eq!(missing["problem"]["code"], "provider_cli_unavailable");
        assert_eq!(missing["models"], serde_json::json!([]));
        let other = chat_route_catalog_with_status(
            &state,
            "bob",
            provider,
            ChatRuntime::IoGateway,
            serde_json::json!({"installed": true, "nativeReady": true}),
        )
        .await;
        assert_eq!(other["ready"], false);
        assert_eq!(other["models"], serde_json::json!([]));
    }
    gateway.abort();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn gateway_catalog_reports_auth_empty_and_malformed_without_secrets() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let gateway = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/{*path}",
                get(|headers: HeaderMap| async move {
                    match headers["x-api-key"].to_str().unwrap() {
                        "fake-bad" => {
                            (StatusCode::UNAUTHORIZED, "secret rejected: fake-bad").into_response()
                        }
                        "fake-empty" => Json(serde_json::json!({"data": []})).into_response(),
                        "fake-malformed" => {
                            (StatusCode::OK, "{broken-json-secret}").into_response()
                        }
                        "fake-schema" => {
                            Json(serde_json::json!({"unknown": ["no"]})).into_response()
                        }
                        _ => (StatusCode::FORBIDDEN, "fake-denied").into_response(),
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    for key in [
        "fake-bad",
        "fake-empty",
        "fake-malformed",
        "fake-schema",
        "fake-denied",
    ] {
        let result = fetch_direct_ai_models(&serde_json::json!({
            "mode": "aiproxy", "baseUrl": format!("http://{address}"), "gatewayApiKey": key,
        }))
        .await;
        if key == "fake-empty" {
            assert!(result.unwrap().is_empty());
        } else {
            let error = result.unwrap_err();
            assert!(!error.body.error.contains(key));
            assert!(!error.body.error.contains("broken-json-secret"));
        }
    }
    gateway.abort();
}

#[tokio::test]
async fn gateway_disabled_and_gemini_incompatible_routes_fail_before_launch() {
    let (root, state) = board_scope_test_state().await;
    state
        .storage
        .set_setting(
            &user_setting_key("test", "direct-ai"),
            &serde_json::json!({
                "mode": "off", "gatewayApiKey": "fake-key", "gatewayUrl": "http://127.0.0.1:1"
            }),
        )
        .unwrap();
    for provider in [Provider::Codex, Provider::Claude, Provider::Gemini] {
        let body = chat_route_catalog_with_status(
            &state,
            "test",
            provider,
            ChatRuntime::IoGateway,
            serde_json::json!({"installed": true, "nativeReady": true}),
        )
        .await;
        assert_eq!(body["ready"], false);
        assert_eq!(
            body["problem"]["code"],
            if provider == Provider::Gemini {
                "chat_route_unavailable"
            } else {
                "gateway_configuration_required"
            }
        );
    }
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn mismatch_event_preserves_request_identity_and_does_not_create_session() {
    let (root, state) = board_scope_test_state().await;
    let (tx, mut rx) = mpsc::channel(4);
    let user = iowb_protocol::UserProfile {
        id: "test".into(),
        username: "test".into(),
        email: None,
        created_at: Utc::now(),
    };
    for provider in [Provider::Codex, Provider::Claude] {
        let id = format!("mismatch-{}", provider.as_str());
        handle_ws_command(
            &state,
            &tx,
            &user,
            WsClientCommand::StartSession {
                provider,
                request_id: Some("attempt-2".into()),
                runtime: Some(ChatRuntime::NativeCli),
                project_path: root.display().to_string(),
                prompt: "must not execute".into(),
                session_id: Some(id.clone()),
                model: Some(" COD:gpt-test ".into()),
                effort: None,
                mode: None,
                thinking: None,
                fast: None,
            },
            &mut HashSet::new(),
            &mut None,
        )
        .await;
        let event = rx.recv().await.unwrap();
        match event {
            WsServerEvent::ChatSetupRequired {
                request_id,
                session_id,
                code,
                ..
            } => {
                assert_eq!(request_id.as_deref(), Some("attempt-2"));
                assert_eq!(session_id, id);
                assert_eq!(code, "chat_runtime_mismatch");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(state.storage.get_session_summary(&id).unwrap().is_none());
        assert!(state.sessions.list_active().await.is_empty());
    }
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn fake_cli_checks_executable_version_and_status_without_returning_secrets() {
    use std::os::unix::fs::PermissionsExt;
    let root = env::temp_dir().join(format!("iowb-readiness-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let fake = root.join("fake-cli");
    for (provider, auth, ready) in [
        (Provider::Codex, "exit 0", true),
        (Provider::Codex, "exit 1", false),
        (Provider::Claude, "echo '{\"loggedIn\":true}'; exit 0", true),
        (
            Provider::Claude,
            "echo '{\"loggedIn\":false}'; exit 0",
            false,
        ),
        (Provider::Claude, "echo malformed; exit 0", false),
    ] {
        std::fs::write(&fake, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'fake-cli 1.2.3'; exit 0; fi\necho 'secret-never-return' >&2\n{auth}\n")).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        let status = probe_provider_readiness(provider, fake.to_str().unwrap()).await;
        assert_eq!(status["installed"], true);
        assert_eq!(status["version"], "1.2.3");
        assert_eq!(status["nativeReady"], ready);
        assert!(!status.to_string().contains("secret-never-return"));
    }
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        readiness_command(fake.to_str().unwrap(), &["--version"])
            .await
            .is_none()
    );
    assert!(
        readiness_command(root.join("missing").to_str().unwrap(), &["--version"])
            .await
            .is_none()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn documentation_links_are_official_https_and_never_shell_commands() {
    for provider in [Provider::Codex, Provider::Claude, Provider::Gemini] {
        let (install, auth) = provider_setup_urls(provider);
        for raw in [install, auth, NODE_INSTALL_URL] {
            let url = reqwest::Url::parse(raw).unwrap();
            assert_eq!(url.scheme(), "https");
            assert!(url.username().is_empty());
            assert!(url.password().is_none());
            assert!(!raw.contains(' '));
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn status_hangs_and_excessive_output_are_bounded() {
    let started = Instant::now();
    assert!(
        readiness_command_with_timeout(
            "/bin/sh",
            &["-c", "exec sleep 30"],
            Duration::from_millis(40)
        )
        .await
        .is_none()
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        readiness_command("/bin/sh", &["-c", "printf '%020000d' 0"])
            .await
            .is_none()
    );
    assert!(
        readiness_command("/bin/sh", &["-c", "echo 1.2.3; exit 1"])
            .await
            .is_none()
    );
}

#[test]
fn malformed_gemini_credentials_are_not_authentication_evidence() {
    let path = env::temp_dir().join(format!("iowb-gemini-{}.json", uuid::Uuid::new_v4()));
    for text in ["{invalid", "{}", "{\"access_token\":\"\"}"] {
        std::fs::write(&path, text).unwrap();
        assert!(!gemini_credentials_configured(&path));
    }
    std::fs::write(&path, "{\"refresh_token\":\"fake-only\"}").unwrap();
    assert!(gemini_credentials_configured(&path));
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn readiness_routes_require_authentication_when_host_auth_is_enabled() {
    let (root, original) = board_scope_test_state().await;
    let mut config = (*original.config).clone();
    config.auth_required = true;
    let state = AppState::initialize(config).await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, build_router(state)).await.unwrap();
    });
    for route in [
        "/api/chat/models?provider=codex",
        "/api/cli/codex/status",
        "/api/cli",
    ] {
        let response = reqwest::get(format!("http://{address}{route}"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    server.abort();
    drop(original);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn chat_gateway_requires_an_explicit_url_even_with_a_saved_credential() {
    let (root, state) = board_scope_test_state().await;
    for provider in [Provider::Codex, Provider::Claude] {
        for config in [
            serde_json::json!({"mode":"aiproxy", "gatewayApiKey":"fake-only"}),
            serde_json::json!({"mode":"aiproxy", "gatewayApiKey":"fake-only", "gatewayUrl":" ", "baseUrl":""}),
        ] {
            state
                .storage
                .set_setting(&user_setting_key("url-user", "direct-ai"), &config)
                .unwrap();
            assert!(direct_ai_runtime_config_for_user(&state, "url-user", provider).is_none());
            let body = chat_route_catalog_with_status(
                &state,
                "url-user",
                provider,
                ChatRuntime::IoGateway,
                serde_json::json!({"installed":true,"nativeReady":true}),
            )
            .await;
            assert_eq!(body["problem"]["code"], "gateway_configuration_required");
            assert_eq!(body["models"], serde_json::json!([]));
        }
        for url_key in ["gatewayUrl", "baseUrl", "base_url"] {
            let mut config = serde_json::json!({"mode":"aiproxy", "gatewayApiKey":"fake-only"});
            config[url_key] = serde_json::json!("http://127.0.0.1:1");
            state
                .storage
                .set_setting(&user_setting_key("url-user", "direct-ai"), &config)
                .unwrap();
            let configured =
                direct_ai_runtime_config_for_user(&state, "url-user", provider).unwrap();
            assert!(configured.base_url.starts_with("http://127.0.0.1:1/"));
        }
        let endpoint_key = if provider == Provider::Codex {
            "codexEndpoint"
        } else {
            "claudeEndpoint"
        };
        let mut config = serde_json::json!({"mode":"aiproxy", "gatewayApiKey":"fake-only"});
        config[endpoint_key] = serde_json::json!("http://127.0.0.1:1/custom");
        state
            .storage
            .set_setting(&user_setting_key("url-user", "direct-ai"), &config)
            .unwrap();
        assert_eq!(
            direct_ai_runtime_config_for_user(&state, "url-user", provider)
                .unwrap()
                .base_url,
            "http://127.0.0.1:1/custom"
        );
    }
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

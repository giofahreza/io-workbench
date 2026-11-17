#[tokio::test(flavor = "current_thread")]
async fn chat_runtime_legacy_sessions_inherit_configured_gateway() {
    let (root, state) = board_scope_test_state().await;
    let user_id = "runtime-routing-user";
    state
        .storage
        .set_setting(
            &user_setting_key(user_id, "direct-ai"),
            &serde_json::json!({ "chatRuntime": "io_gateway" }),
        )
        .unwrap();

    for (provider, model) in [(Provider::Codex, "gpt-6.1-sol"), (Provider::Claude, "opus")] {
        let session = SessionSummary {
            id: format!("legacy-{}", provider.as_str()),
            provider,
            project_path: root.display().to_string(),
            model: Some(model.to_string()),
            runtime: None,
            ..Default::default()
        };
        state.storage.upsert_session(&session).unwrap();
        for requested_model in [None, Some(model)] {
            assert_eq!(
                resolve_session_chat_runtime(
                    &state,
                    user_id,
                    provider,
                    Some(&session.id),
                    requested_model,
                ),
                ChatRuntime::IoGateway,
                "legacy {} chat must use the same route as a new chat",
                provider.as_str(),
            );
        }
    }

    // Changing the configured route also applies to legacy chats until their
    // first Workbench turn explicitly persists a runtime.
    state
        .storage
        .set_setting(
            &user_setting_key(user_id, "direct-ai"),
            &serde_json::json!({ "chatRuntime": "native_cli" }),
        )
        .unwrap();
    assert_eq!(
        resolve_session_chat_runtime(
            &state,
            user_id,
            Provider::Codex,
            Some("legacy-codex"),
            Some("gpt-6.1-sol"),
        ),
        ChatRuntime::NativeCli,
    );
    assert_eq!(
        resolve_session_chat_runtime(
            &state,
            user_id,
            Provider::Codex,
            Some("legacy-codex"),
            Some("cod:gpt-6.1-sol"),
        ),
        ChatRuntime::IoGateway,
        "an explicit gateway model still identifies the gateway route",
    );

    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn chat_runtime_preserves_explicit_session_routes() {
    let (root, state) = board_scope_test_state().await;
    let user_id = "runtime-routing-user";
    for (runtime, configured) in [
        (ChatRuntime::NativeCli, "io_gateway"),
        (ChatRuntime::IoGateway, "native_cli"),
    ] {
        state
            .storage
            .set_setting(
                &user_setting_key(user_id, "direct-ai"),
                &serde_json::json!({ "chatRuntime": configured }),
            )
            .unwrap();
        let session = SessionSummary {
            id: "pinned-runtime".to_string(),
            provider: Provider::Codex,
            project_path: root.display().to_string(),
            model: Some("gpt-6.1-sol".to_string()),
            runtime: Some(runtime),
            ..Default::default()
        };
        state.storage.upsert_session(&session).unwrap();
        assert_eq!(
            resolve_session_chat_runtime(
                &state,
                user_id,
                Provider::Codex,
                Some(&session.id),
                Some("gpt-6.1-sol"),
            ),
            runtime,
        );
    }

    assert_eq!(
        resolve_session_chat_runtime(&state, user_id, Provider::Gemini, None, None),
        ChatRuntime::NativeCli,
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

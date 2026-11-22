#[tokio::test(flavor = "current_thread")]
async fn chat_runtime_legacy_sessions_use_stored_models_not_global_defaults() {
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
                ChatRuntime::NativeCli,
                "legacy {} chat must not be rerouted by a new default",
                provider.as_str(),
            );
        }
    }

    // A saved gateway prefix is evidence for a legacy gateway route. A newly
    // submitted model is not evidence for the old thread's origin.
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
        ChatRuntime::NativeCli,
        "a newly submitted model must not change legacy route inference",
    );
    state.storage.upsert_session(&SessionSummary {
        id: "legacy-gateway".to_string(), provider: Provider::Codex,
        model: Some("cod:gpt-6.1-sol".to_string()), runtime: None, ..Default::default()
    }).unwrap();
    assert_eq!(resolve_session_chat_runtime(&state, user_id, Provider::Codex, Some("legacy-gateway"), None), ChatRuntime::IoGateway);

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
        assert_eq!(requested_chat_runtime(&state, user_id, Provider::Codex, Some(&session.id), Some(ChatRuntime::NativeCli)), runtime);
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
    state.storage.upsert_session(&SessionSummary {
        id: "gemini-pinned".to_string(), provider: Provider::Gemini,
        runtime: Some(ChatRuntime::IoGateway), ..Default::default()
    }).unwrap();
    assert_eq!(requested_chat_runtime(&state, user_id, Provider::Gemini, Some("gemini-pinned"), Some(ChatRuntime::NativeCli)), ChatRuntime::IoGateway,
        "unsupported saved routes must be explained, never silently downgraded");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

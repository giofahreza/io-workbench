#[test]
fn codex_thinking_without_effort_preserves_configured_default() {
    for effort in [None, Some("invalid")] {
        let args = default_agent_args_with(Provider::Codex, "hi", None, effort, Some(true), None);
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("model_reasoning_effort="))
        );
        assert_eq!(effective_codex_reasoning_effort(effort, true), None);
    }
}

#[test]
fn codex_profile_defaults_preserve_chat_choices_and_gateway_routing() {
    let root = env::temp_dir().join(format!("iowb-profile-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("default.config.toml"),
        r#"
model = "profile-model"
model_provider = "profile-provider"
model_reasoning_effort = "high"
service_tier = "default"
model_catalog_json = "~/catalogs/models.json"
model_context_window = 250000
model_auto_compact_token_limit = 230000
api_key = "must-not-be-forwarded"
[model_providers.profile-provider]
base_url = "https://must-not-be-forwarded.invalid"
"#,
    )
    .unwrap();

    let overrides =
        codex_profile_overrides(&root, Some(&root), None, ChatRuntime::NativeCli).unwrap();
    assert!(
        overrides
            .iter()
            .any(|(key, value)| key == "model_provider" && value == "\"profile-provider\"")
    );
    assert!(overrides.iter().any(|(key, value)| {
        key == "model_catalog_json"
            && value
                == &toml::Value::String(root.join("catalogs/models.json").display().to_string())
                    .to_string()
    }));
    assert!(!format!("{overrides:?}").contains("must-not-be-forwarded"));

    let mut args = default_agent_args_with_resume(
        Provider::Codex,
        "hi",
        None,
        Some("medium"),
        Some(true),
        Some(true),
        Some("chat-model"),
        Some("existing-thread"),
        ChatRuntime::NativeCli,
    );
    insert_codex_profile_args(&mut args, overrides);
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--model", "chat-model"])
    );
    assert!(
        !args
            .iter()
            .any(|arg| arg.starts_with("model=profile") || arg == "model=\"profile-model\"")
    );
    assert!(
        args.iter()
            .any(|arg| arg == "model_reasoning_effort=\"medium\"")
    );
    assert!(
        !args
            .iter()
            .any(|arg| arg == "model_reasoning_effort=\"high\"")
    );
    assert!(!args.iter().any(|arg| arg == "service_tier=\"default\""));
    assert!(args.iter().any(|arg| arg == "service_tier=\"fast\""));
    let catalog_index = args
        .iter()
        .position(|arg| arg.starts_with("model_catalog_json="))
        .unwrap();
    assert!(catalog_index < args.iter().position(|arg| arg == "resume").unwrap());

    let gateway =
        codex_profile_overrides(&root, Some(&root), None, ChatRuntime::IoGateway).unwrap();
    assert!(!gateway.iter().any(|(key, _)| key == "model_provider"));
    let mut args = default_agent_args_with_resume(
        Provider::Codex,
        "hi",
        None,
        None,
        None,
        None,
        Some("cod:chat-model"),
        None,
        ChatRuntime::IoGateway,
    );
    insert_codex_profile_args(&mut args, gateway);
    assert!(args.iter().any(|arg| arg == "model_provider=iowb_gateway"));
    assert!(!args.iter().any(|arg| arg.contains("profile-provider")));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn codex_profile_selection_handles_missing_disabled_relative_and_invalid_profiles() {
    let root = env::temp_dir().join(format!("iowb-profile-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let load =
        |selected| codex_profile_overrides(&root, Some(&root), selected, ChatRuntime::NativeCli);
    assert!(load(None).unwrap().is_empty());
    assert!(load(Some("missing")).is_err());
    assert!(load(Some("../outside")).is_err());
    assert!(load(Some(" ")).unwrap().is_empty());
    std::fs::write(
        root.join("custom.config.toml"),
        "model_catalog_json = 'catalogs/models.json'\n",
    )
    .unwrap();
    let custom = load(Some("custom")).unwrap();
    assert_eq!(
        custom,
        vec![(
            "model_catalog_json".to_string(),
            toml::Value::String(root.join("catalogs/models.json").display().to_string())
                .to_string()
        )]
    );
    let mut app_server_args = vec!["app-server".to_string()];
    insert_codex_profile_args(&mut app_server_args, custom);
    assert_eq!(app_server_args[1], "-c");
    std::fs::write(root.join("default.config.toml"), "secret = 'not-closed").unwrap();
    let error = load(None).unwrap_err().to_string();
    assert!(!error.contains("not-closed"));
    assert!(error.contains("invalid TOML"));
    assert!(load(Some("")).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn codex_live_profile_defaults_override_saved_settings_below_chat_choices() {
    let mut params = CodexAppServerLiveTurnParams {
        thread_id: Some("saved-thread".to_string()),
        cwd: PathBuf::from("/tmp"),
        input: Vec::new(),
        client_user_message_id: None,
        model: None,
        model_provider: None,
        effort: None,
        service_tier: None,
        approval_policy: None,
        sandbox_policy: None,
    };
    let mut options = CodexAppServerLaunchOptions::default();
    options.args = vec!["app-server".to_string()];
    insert_codex_profile_args(
        &mut options.args,
        vec![
            ("model".to_string(), "\"profile-model\"".to_string()),
            ("model_provider".to_string(), "iowb_gateway".to_string()),
            ("model_reasoning_effort".to_string(), "\"high\"".to_string()),
            ("service_tier".to_string(), "\"default\"".to_string()),
        ],
    );
    apply_codex_live_profile_defaults(&mut params, Some(&options));
    assert_eq!(params.model.as_deref(), Some("profile-model"));
    assert_eq!(params.model_provider.as_deref(), Some("iowb_gateway"));
    assert_eq!(params.effort.as_deref(), Some("high"));
    assert_eq!(params.service_tier.as_deref(), Some("default"));
    params.model = Some("chat-model".to_string());
    params.effort = Some("medium".to_string());
    params.service_tier = Some("fast".to_string());
    apply_codex_live_profile_defaults(&mut params, Some(&options));
    assert_eq!(params.model.as_deref(), Some("chat-model"));
    assert_eq!(params.effort.as_deref(), Some("medium"));
    assert_eq!(params.service_tier.as_deref(), Some("fast"));
}

fn codex_usage_report(input: u64, output: u64, cached: u64, reasoning: u64) -> Value {
    serde_json::json!({
        "inputTokens": input, "outputTokens": output, "totalTokens": input + output,
        "cachedInputTokens": cached, "reasoningOutputTokens": reasoning,
        "cacheWriteInputTokens": 0,
    })
}

#[test]
fn codex_resumed_turn_usage_counts_all_requests_once_after_baseline() {
    let baseline = codex_usage_report(1000, 100, 800, 20);
    let mut normalizer = CodexAppServerLiveOutputNormalizer::for_live_turn(true);
    normalizer
        .usage_tracker
        .set_baseline(&serde_json::json!({"total": baseline}));
    // The first current-turn usage can be interleaved before turn/start's response.
    let first = serde_json::json!({"turnId": "current", "tokenUsage": {
        "last": codex_usage_report(100, 10, 90, 3),
        "total": codex_usage_report(1100, 110, 890, 23),
    }});
    normalizer.push_notification("thread/tokenUsage/updated", &first);
    assert!(normalizer.final_usage.is_none());
    normalizer.associate_usage_turn("current".to_string());
    normalizer.push_notification("thread/tokenUsage/updated", &first);
    // Context size can drop after compaction; accounting uses the monotonic total.
    let second = serde_json::json!({"turnId": "current", "tokenUsage": {
        "last": codex_usage_report(50, 5, 40, 2),
        "total": codex_usage_report(1150, 115, 930, 25),
    }});
    normalizer.push_notification("thread/tokenUsage/updated", &second);
    normalizer.push_notification("thread/tokenUsage/updated", &second);
    normalizer.push_notification(
        "thread/tokenUsage/updated",
        &serde_json::json!({
            "turnId": "previous", "tokenUsage": {"total": codex_usage_report(9000, 900, 0, 0)}
        }),
    );
    normalizer.push_notification(
        "turn/completed",
        &serde_json::json!({
            "turn": {"id": "current", "status": "interrupted", "items": []}
        }),
    );
    let usage = normalizer.take_final_usage().unwrap();
    assert_eq!(
        (usage.usage.used, usage.usage.input, usage.usage.output),
        (165, 150, 15)
    );
    assert_eq!((usage.usage.cache_read, usage.usage.reasoning), (130, 5));
    assert_eq!(usage.completeness, TokenUsageCompleteness::Complete);
}

#[test]
fn codex_setup_usage_from_previous_turn_seeds_baseline_without_charging_it() {
    let mut normalizer = CodexAppServerLiveOutputNormalizer::for_live_turn(true);
    normalizer.push_notification(
        "thread/tokenUsage/updated",
        &serde_json::json!({
            "turnId": "previous", "tokenUsage": {"total": codex_usage_report(1000, 100, 800, 20)}
        }),
    );
    normalizer.associate_usage_turn("current".to_string());
    assert!(normalizer.take_final_usage().is_none());
    normalizer.push_notification(
        "thread/tokenUsage/updated",
        &serde_json::json!({
            "turnId": "current", "tokenUsage": {
                "last": codex_usage_report(100, 10, 90, 3),
                "total": codex_usage_report(1100, 110, 890, 23)
            }
        }),
    );
    let usage = normalizer.take_final_usage().unwrap();
    assert_eq!(usage.usage.used, 110);
    assert_eq!(usage.completeness, TokenUsageCompleteness::Complete);
}

#[test]
fn codex_usage_missing_baseline_or_counter_reset_is_partial() {
    let mut normalizer = CodexAppServerLiveOutputNormalizer::for_live_turn(true);
    normalizer.associate_usage_turn("current".to_string());
    let report = |input, output| {
        serde_json::json!({"turnId": "current", "tokenUsage": {
            "last": codex_usage_report(100, 10, 90, 3),
            "total": codex_usage_report(input, output, 900, 23)
        }})
    };
    normalizer.push_notification("thread/tokenUsage/updated", &report(1100, 110));
    normalizer.push_notification("thread/tokenUsage/updated", &report(1200, 120));
    // A reset or out-of-order report cannot turn historic usage into a charge.
    normalizer.push_notification("thread/tokenUsage/updated", &report(100, 10));
    let usage = normalizer.take_final_usage().unwrap();
    assert_eq!(usage.usage.used, 220);
    assert_eq!(usage.completeness, TokenUsageCompleteness::Partial);
}

#[test]
fn codex_usage_legacy_reports_do_not_double_count_when_totals_appear() {
    let mut tracker = CodexTurnUsageTracker::default();
    let legacy = serde_json::json!({"last": codex_usage_report(100, 10, 90, 3)});
    tracker.update(&legacy);
    assert_eq!(tracker.update(&legacy).unwrap().usage.used, 110);
    let cumulative = serde_json::json!({
        "last": codex_usage_report(100, 10, 90, 3),
        "total": codex_usage_report(1100, 110, 900, 23),
    });
    let usage = tracker.update(&cumulative).unwrap();
    assert_eq!(usage.usage.used, 110);
    assert_eq!(usage.completeness, TokenUsageCompleteness::Partial);
    assert_eq!(tracker.update(&legacy).unwrap().usage.used, 110);
    assert_eq!(tracker.update(&cumulative).unwrap().usage.used, 110);
}

#[test]
fn codex_failed_or_interrupted_turn_retains_usage_without_visible_output() {
    for status in ["failed", "interrupted"] {
        let mut normalizer = CodexAppServerLiveOutputNormalizer::for_live_turn(false);
        normalizer.associate_usage_turn("current".to_string());
        assert!(normalizer.take_final_usage().is_none());
        for (input, output) in [(100, 10), (150, 15)] {
            normalizer.push_notification(
                "thread/tokenUsage/updated",
                &serde_json::json!({
                    "turnId": "current", "tokenUsage": {
                        "last": codex_usage_report(50, 5, 40, 2),
                        "total": codex_usage_report(input, output, 100, 5),
                    }
                }),
            );
        }
        normalizer.push_notification(
            "turn/completed",
            &serde_json::json!({
                "turn": {"id": "current", "status": status, "items": []}
            }),
        );
        let usage = normalizer.take_final_usage().unwrap();
        assert_eq!(usage.usage.used, 165);
        assert_eq!(usage.completeness, TokenUsageCompleteness::Complete);
    }
}

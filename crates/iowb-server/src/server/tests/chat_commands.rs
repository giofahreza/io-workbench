#[test]
fn chat_command_query_decodes_optional_context_and_refresh() {
    let uri: Uri = "/api/chat/commands?provider=claude&projectPath=%2Ftmp%2Fproject&refresh=true"
        .parse()
        .unwrap();
    let Query(query) = Query::<ChatCommandsQuery>::try_from_uri(&uri).unwrap();
    assert_eq!(query.context.provider, Provider::Claude);
    assert_eq!(query.context.project_path, "/tmp/project");
    assert!(query.refresh);
    assert!(query.context.session_id.is_none());
}

#[tokio::test]
async fn chat_command_api_discovers_and_validates_without_creating_sessions() {
    let (root, state) = test_app_state().await;
    let commands = root.join(".claude/commands");
    std::fs::create_dir_all(&commands).unwrap();
    std::fs::write(
        commands.join("inspect-fixture.md"),
        "---\ndescription: Inspect fixture\n---\n$ARGUMENTS",
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = build_router(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let client = reqwest::Client::new();
    let url = format!("http://{address}/api/chat/commands");
    let response = client
        .get(&url)
        .query(&[
            ("provider", "claude"),
            ("projectPath", root.to_str().unwrap()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let catalog: iowb_protocol::ChatCommandCatalog = response.json().await.unwrap();
    assert!(
        catalog
            .commands
            .iter()
            .any(|command| command.name == "inspect-fixture")
    );
    for (prompt, expected) in [
        ("/help", StatusCode::OK),
        ("/model extra", StatusCode::BAD_REQUEST),
        ("/compact", StatusCode::CONFLICT),
    ] {
        let response = client
            .post(format!("{url}/prepare"))
            .json(&serde_json::json!({"provider":"claude", "projectPath":root, "prompt":prompt}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{prompt}");
    }
    // A fresh chat has no provider-reported command list, so unknown names
    // are forwarded to the CLI and `/mcp` explains that status is not known yet.
    for (prompt, action, message) in [
        ("/unknown-fixture-command", "prompt", None),
        (
            "/mcp",
            "mcp",
            Some("MCP status is reported after the first message in this chat."),
        ),
    ] {
        let prepared: serde_json::Value = client
            .post(format!("{url}/prepare"))
            .json(&serde_json::json!({"provider":"claude", "projectPath":root, "prompt":prompt}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(prepared["action"], action, "{prompt}");
        assert_eq!(prepared["message"].as_str(), message, "{prompt}");
    }
    assert!(state.storage.list_sessions().unwrap().is_empty());
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn project_sessions_pages_newest_first_and_keeps_the_legacy_array() {
    let (root, state) = board_scope_test_state().await;
    let project_path = root.join("paged-project");
    std::fs::create_dir_all(&project_path).expect("project directory");
    state
        .projects
        .add_project(&project_path)
        .expect("add project");
    for index in 0..5 {
        state
            .storage
            .upsert_session(&SessionSummary {
                id: format!("paged-{index}"),
                provider: Provider::Codex,
                project_path: project_path.display().to_string(),
                last_activity: Utc::now() - chrono::Duration::seconds(index),
                ..Default::default()
            })
            .unwrap();
    }

    async fn fetch(state: &AppState, limit: Option<usize>, offset: Option<usize>) -> Value {
        let response = project_sessions(
            State(state.clone()),
            AxumPath("paged-project".to_string()),
            Query(ProjectSessionsQuery { limit, offset }),
        )
        .await
        .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }
    fn ids(page: &Value) -> Vec<&str> {
        page["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|session| session["id"].as_str().unwrap())
            .collect()
    }

    let legacy = fetch(&state, None, None).await;
    assert_eq!(legacy.as_array().expect("legacy array").len(), 5);

    let first = fetch(&state, Some(2), None).await;
    assert_eq!(ids(&first), ["paged-0", "paged-1"]);
    assert_eq!(first["hasMore"], true);
    assert_eq!(first["totalCount"], 5);

    let second = fetch(&state, Some(2), Some(2)).await;
    assert_eq!(ids(&second), ["paged-2", "paged-3"]);
    assert_eq!(second["hasMore"], true);

    let last = fetch(&state, Some(2), Some(4)).await;
    assert_eq!(ids(&last), ["paged-4"]);
    assert_eq!(last["hasMore"], false);

    let past_end = fetch(&state, Some(2), Some(10)).await;
    assert!(ids(&past_end).is_empty());
    assert_eq!(past_end["hasMore"], false);
    assert_eq!(past_end["totalCount"], 5);

    let only_offset = fetch(&state, None, Some(3)).await;
    assert_eq!(ids(&only_offset), ["paged-3", "paged-4"]);

    assert_eq!(ids(&fetch(&state, Some(0), None).await), ["paged-0"]);

    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

struct BoardReadAccessFixture {
    root: PathBuf,
    project: PathBuf,
    state: AppState,
    user: AuthenticatedUser,
}

impl BoardReadAccessFixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("iowb-board-read-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        fs::create_dir_all(&project).unwrap();
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().unwrap(),
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
        .unwrap();
        let user = AuthenticatedUser(iowb_protocol::UserProfile {
            id: "board-read-user".to_string(),
            username: "board-read-user".to_string(),
            email: None,
            created_at: Utc::now(),
        });
        Self {
            root,
            project,
            state,
            user,
        }
    }

    async fn list(&self, scoped: bool) -> Result<Json<Value>> {
        list_boards(
            State(self.state.clone()),
            Extension(self.user.clone()),
            Query(BoardsQuery {
                project_path: scoped.then(|| self.project.display().to_string()),
            }),
        )
        .await
    }

    async fn detail(&self, board_id: &str) -> Result<Json<Value>> {
        get_board(
            State(self.state.clone()),
            Extension(self.user.clone()),
            AxumPath(board_id.to_string()),
        )
        .await
    }
}

impl Drop for BoardReadAccessFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn board_read_project_access_valid_empty_directory_is_an_empty_board_list() {
    let fixture = BoardReadAccessFixture::new().await;
    let Json(result) = fixture.list(true).await.unwrap();
    assert_eq!(result["success"], true);
    assert_eq!(result["boards"], json!([]));
}

#[tokio::test(flavor = "current_thread")]
async fn board_read_project_access_missing_directory_is_not_a_no_board_state() {
    let fixture = BoardReadAccessFixture::new().await;
    fs::remove_dir(&fixture.project).unwrap();
    let error = fixture
        .list(true)
        .await
        .expect_err("missing project must not appear empty");
    assert_eq!(error.status, StatusCode::NOT_FOUND);
    assert!(error.body.error.contains("does not exist"));
    assert!(error.body.error.contains("Restore"));
    assert!(error.body.error.contains("preserved"));
    assert!(
        !fixture.project.exists(),
        "reading must not recreate a missing project"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn board_read_project_access_file_is_not_a_project_directory() {
    let fixture = BoardReadAccessFixture::new().await;
    fs::remove_dir(&fixture.project).unwrap();
    fs::write(&fixture.project, "not a directory").unwrap();
    let error = fixture
        .list(true)
        .await
        .expect_err("file must not appear as empty project");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert!(error.body.error.contains("not a directory"));
    assert_eq!(
        fs::read_to_string(&fixture.project).unwrap(),
        "not a directory"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn board_read_project_access_unreadable_directory_is_actionable() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = BoardReadAccessFixture::new().await;
    let original = fs::metadata(&fixture.project).unwrap().permissions();
    for mode in [0o000, 0o400, 0o100] {
        fs::set_permissions(&fixture.project, fs::Permissions::from_mode(mode)).unwrap();
        // An administrator may bypass permissions; do not mistake that for an
        // inaccessible fixture. Test both read-only and traversal-only directories.
        let inaccessible = fs::read_dir(&fixture.project).is_err()
            || fs::metadata(fixture.project.join(".")).is_err();
        let result = fixture.list(true).await;
        fs::set_permissions(&fixture.project, original.clone()).unwrap();
        if inaccessible {
            let error = result.expect_err("inaccessible project must not appear empty");
            assert_eq!(error.status, StatusCode::FORBIDDEN);
            assert!(error.body.error.contains("permission"));
            assert!(error.body.error.contains("preserved"));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn board_read_project_access_missing_workspace_preserves_saved_history_and_recovers() {
    let fixture = BoardReadAccessFixture::new().await;
    let mut board = board_fixture(json!({
        "projectPath": fixture.project.display().to_string(),
        "command": "Preserve this approved historical board"
    }));
    board.user_id = Some(fixture.user.0.id.clone());
    save_board(&fixture.state, &board).unwrap();
    let saved_path = board_file_path(&fixture.state, &board);
    let saved_before = fs::read(&saved_path).unwrap();
    let held = fixture.root.join("held-project");
    fs::rename(&fixture.project, &held).unwrap();

    assert_eq!(
        fixture.list(true).await.unwrap_err().status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture.detail(&board.id).await.unwrap_err().status,
        StatusCode::NOT_FOUND
    );
    let Json(history) = fixture.list(false).await.unwrap();
    assert_eq!(history["boards"][0]["id"], board.id);
    assert_eq!(fs::read(&saved_path).unwrap(), saved_before);

    fs::rename(&held, &fixture.project).unwrap();
    let Json(restored) = fixture.detail(&board.id).await.unwrap();
    assert_eq!(restored["board"]["id"], board.id);
    assert_eq!(fs::read(&saved_path).unwrap(), saved_before);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn board_read_project_access_keeps_workspace_boundaries_and_user_ownership() {
    let fixture = BoardReadAccessFixture::new().await;
    let outside = BoardReadAccessFixture::new().await;
    fs::remove_dir(&fixture.project).unwrap();
    std::os::unix::fs::symlink(&outside.project, &fixture.project).unwrap();
    let error = fixture.list(true).await.unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert!(error.body.error.contains("allowed workspace root"));

    let mut board = board_fixture(json!({
        "projectPath": fixture.root.join("missing-private-path").display().to_string(),
        "command": "Another user's history"
    }));
    board.user_id = Some("another-user".to_string());
    save_board(&fixture.state, &board).unwrap();
    let error = fixture.detail(&board.id).await.unwrap_err();
    assert_eq!(error.status, StatusCode::NOT_FOUND);
    assert_eq!(error.body.error, "Agentic board not found");
    let Json(history) = fixture.list(false).await.unwrap();
    assert_eq!(history["boards"], json!([]));
}

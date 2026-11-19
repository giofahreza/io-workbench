#[test]
fn empty_board_creation_without_prompt_is_paused_and_has_no_seed_task() {
    let run = board_fixture(json!({
        "projectPath": "/tmp/project",
        "provider": "codex",
        "model": "selected-model"
    }));

    assert_eq!(run.status, "paused");
    assert!(run.tasks.is_empty());
    assert!(run.source_prompt.is_empty());
    assert_eq!(run.next_task_sequence, 0);
    assert_eq!(run.provider, "codex");
    assert_eq!(run.model, "selected-model");
    assert!(!run.active);
    assert!(!run.loop_started);
    assert!(!run.auto_run_enabled);
    assert!(!run.bootstrap_complete);
    assert_eq!(
        run.pause_reason.as_deref(),
        Some("Board created without tasks.")
    );
    assert_eq!(run.provider_call_started_at, None);
    assert_eq!(run.current_provider_session_id, None);
}

#[test]
fn empty_board_creation_still_rejects_explicit_blank_prompts() {
    for field in ["command", "prompt"] {
        let mut value = json!({ "projectPath": "/tmp/project" });
        value[field] = json!(" \n ");
        let request = serde_json::from_value::<CreateBoardRequest>(value).unwrap();
        let error = AgenticBoard::new(None, request).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.body.error.contains("Prompt is required"));
    }
}

#[test]
fn empty_board_creation_preserves_optional_legacy_prompt_support() {
    for field in ["command", "prompt"] {
        let mut value = json!({ "projectPath": "/tmp/project" });
        value[field] = json!("Initial task");
        let run = board_fixture(value);
        assert_eq!(run.source_prompt, "Initial task");
        assert_eq!(run.tasks.len(), 1);
        assert_eq!(run.tasks[0].status, TASK_STATUS_BACKLOG);
        assert_eq!(task_level(&run.tasks[0]), TASK_LEVEL_STORY);
        assert!(!task_is_executable(&run.tasks[0]));
        assert!(!run.active);
    }
}

#[test]
fn empty_board_creation_accepts_generated_backlog_without_a_seed_or_global_prompt() {
    let mut run = board_fixture(json!({ "projectPath": "/tmp/project" }));
    let prompt = "Add budgets\nValidate budget input";
    let drafts = sanitize_prompt_task_drafts(
        &json!({ "tasks": [
            { "planKey": "budgets", "level": "story", "title": "Manage budgets" },
            { "planKey": "validation", "level": "story", "title": "Validate budget input",
              "dependsOn": ["plan:budgets"] }
        ] }),
        prompt,
    );
    let trees = prepare_generated_prompt_task_trees(&mut run, drafts, prompt).unwrap();
    run.tasks.extend(trees.into_iter().flatten());
    normalize_board_hierarchy(&mut run);
    refresh_hierarchy_rollups(&mut run);

    assert_eq!(run.tasks.len(), 2);
    assert!(
        run.tasks
            .iter()
            .all(|task| task.status == TASK_STATUS_BACKLOG)
    );
    assert!(run.tasks.iter().all(|task| !task_scope_is_approved(task)));
    assert_eq!(task_blockers(&run.tasks[1]), vec![run.tasks[0].id.clone()]);
    assert!(run.source_prompt.is_empty());
    assert!(pick_next_task_index(&run).is_none());
    assert!(!run.active);
    assert!(!run.loop_started);
}

struct EmptyBoardFixture {
    root: PathBuf,
    project: PathBuf,
    state: AppState,
    user: AuthenticatedUser,
}

impl EmptyBoardFixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("iowb-empty-board-{}", Uuid::new_v4()));
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
            id: "empty-board-user".to_string(),
            username: "empty-board-user".to_string(),
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

    async fn create(&self, mut options: Value) -> (StatusCode, Value) {
        options["projectPath"] = json!(self.project.display().to_string());
        let request = serde_json::from_value::<CreateBoardRequest>(options).unwrap();
        let (status, Json(result)) = create_board(
            State(self.state.clone()),
            Extension(self.user.clone()),
            Json(request),
        )
        .await
        .unwrap();
        (status, result)
    }
}

impl Drop for EmptyBoardFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn empty_board_creation_first_task_uses_normal_backlog_addition_without_execution() {
    let fixture = EmptyBoardFixture::new().await;
    let (status, result) = fixture.create(json!({ "provider": "codex" })).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(result["reused"], false);
    assert_eq!(result["board"]["tasks"], json!([]));
    let board_id = result["board"]["id"].as_str().unwrap().to_string();

    for prompt in ["First task", "Second task"] {
        let request = serde_json::from_value::<TaskRequest>(json!({
            "prompt": prompt,
            "status": "backlog"
        }))
        .unwrap();
        let (status, Json(result)) = add_task(
            State(fixture.state.clone()),
            Extension(fixture.user.clone()),
            AxumPath(board_id.clone()),
            Json(request),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(result["board"]["status"], "paused");
    }

    let stored = load_user_board(&fixture.state, &fixture.user.0.id, &board_id).unwrap();
    assert_eq!(stored.board.tasks.len(), 2);
    for task in &stored.board.tasks {
        assert_eq!(task.status, TASK_STATUS_BACKLOG);
        assert_eq!(task_level(task), TASK_LEVEL_STORY);
        assert!(!task_is_executable(task));
        assert!(!task_scope_is_approved(task));
    }
    assert!(!stored.board.active);
    assert!(!stored.board.loop_started);
    assert!(!stored.board.bootstrap_complete);
    assert_eq!(stored.board.provider_call_started_at, None);
}

#[tokio::test(flavor = "current_thread")]
async fn empty_board_creation_reuses_existing_project_board_without_mutating_it() {
    let fixture = EmptyBoardFixture::new().await;
    let (_, result) = fixture
        .create(json!({
            "command": "Existing story",
            "provider": "codex",
            "model": "existing-model"
        }))
        .await;
    let board_id = result["board"]["id"].as_str().unwrap().to_string();

    for board_status in ["paused", "running", "scheduled", "completed", "cancelled"] {
        let mut stored = load_user_board(&fixture.state, &fixture.user.0.id, &board_id).unwrap();
        stored.board.status = board_status.to_string();
        stored.board.active = board_status == "running";
        stored.board.loop_started = board_status == "running";
        stored.board.auto_run_enabled = board_status == "running";
        stored.board.scheduled_start_at =
            (board_status == "scheduled").then(|| Utc::now() + chrono::Duration::hours(1));
        save_board(&fixture.state, &stored.board).unwrap();
        let before = serde_json::to_value(&stored.board).unwrap();

        let (status, result) = fixture
            .create(json!({
                "provider": "claude",
                "model": "must-not-replace-existing-model",
                "scheduledStartAt": (Utc::now() + chrono::Duration::hours(2)).to_rfc3339()
            }))
            .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(result["reused"], true);
        assert_eq!(result["board"]["id"], board_id);
        let after = load_user_board(&fixture.state, &fixture.user.0.id, &board_id).unwrap();
        assert_eq!(serde_json::to_value(&after.board).unwrap(), before);
    }
}

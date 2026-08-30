    #[test]
    fn blank_provider_normalizes_to_default_provider() {
        assert_eq!(normalize_provider(Some("")).unwrap(), DEFAULT_PROVIDER);
        assert_eq!(normalize_provider(Some("   ")).unwrap(), DEFAULT_PROVIDER);
        assert_eq!(normalize_provider(None).unwrap(), DEFAULT_PROVIDER);
    }

    #[test]
    fn pause_during_provider_turn_keeps_single_board_worker_owner() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.status = "running".to_string();
        run.active = true;
        run.loop_started = true;
        run.current_task_id = Some("task-1".to_string());
        run.current_task_status = "in_progress".to_string();
        run.current_provider_session_id = Some("provider-session".to_string());

        request_board_pause(&mut run, Some("user request".to_string()));

        assert_eq!(run.status, "pausing");
        assert!(run.active);
        assert!(run.loop_started);
        assert!(run.pause_requested);
        assert!(!run.auto_run_enabled);
        assert!(!board_should_abort_provider(&run));
    }

    #[test]
    fn pause_before_provider_turn_returns_current_card_to_todo() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.status = "running".to_string();
        run.active = true;
        run.loop_started = true;
        run.current_task_id = Some("task-1".to_string());
        run.current_task_status = "in_progress".to_string();
        run.tasks[0].status = "in_progress".to_string();
        run.tasks[0].started_at = Some(Utc::now());
        run.tasks[0].provider_session_id = Some("stale-provider-session".to_string());

        request_board_pause(&mut run, Some("user request".to_string()));

        assert_eq!(run.status, "paused");
        assert!(!run.active);
        assert!(!run.loop_started);
        assert!(!run.pause_requested);
        assert_eq!(run.tasks[0].status, TASK_STATUS_TODO);
        assert_eq!(run.tasks[0].started_at, None);
        assert_eq!(run.tasks[0].provider_session_id, None);
        assert_eq!(run.current_task_id, None);
        assert_eq!(run.control_revision, 1);
    }

    #[test]
    fn stale_board_worker_save_preserves_newer_pause_control_state() {
        let mut stale = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        stale.status = "running".to_string();
        stale.active = true;
        stale.loop_started = true;
        stale.current_task_id = Some("task-1".to_string());
        stale.current_task_status = "in_progress".to_string();
        stale.tasks[0].status = "in_progress".to_string();

        let mut paused = stale.clone();
        request_board_pause(&mut paused, Some("user request".to_string()));

        preserve_newer_control_state(&mut stale, &paused);

        assert_eq!(stale.status, "paused");
        assert!(!stale.active);
        assert!(!stale.loop_started);
        assert_eq!(stale.tasks[0].status, TASK_STATUS_TODO);
        assert_eq!(stale.control_revision, paused.control_revision);
        assert_eq!(stale.pause_reason.as_deref(), Some("user request"));
    }

    #[test]
    fn abort_resets_in_flight_cards_and_control_pointers() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.status = "running".to_string();
        run.active = true;
        run.loop_started = true;
        run.current_task_id = Some("task-1".to_string());
        run.current_task_title = run.tasks[0].title.clone();
        run.current_task_status = "in_progress".to_string();
        run.current_provider_session_id = Some("provider-session".to_string());
        run.provider_call_started_at = Some(Utc::now());
        run.provider_call_label = Some("task execution".to_string());
        run.tasks[0].status = "in_progress".to_string();
        run.tasks[0].provider_session_id = Some("stale-provider-session".to_string());

        reset_in_flight_board_tasks(&mut run, "aborted");
        run.status = "cancelled".to_string();
        run.active = false;
        run.loop_started = false;
        run.current_task_id = None;
        run.current_task_title.clear();
        run.current_task_status.clear();
        run.current_provider_session_id = None;
        run.provider_call_started_at = None;
        run.provider_call_label = None;

        assert_eq!(run.tasks[0].status, TASK_STATUS_TODO);
        assert_eq!(run.tasks[0].provider_session_id, None);
        assert_eq!(run.current_task_id, None);
        assert_eq!(run.current_provider_session_id, None);
        assert_eq!(run.provider_call_started_at, None);
    }

    #[test]
    fn legacy_session_backfill_skips_non_terminal_tasks() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        assert!(!task_allows_legacy_session_backfill(&run.tasks[0]));
        assert!(!task_needs_legacy_session_backfill(&run.tasks[0]));

        run.tasks[0].status = TASK_STATUS_IN_PROGRESS.to_string();
        assert!(!task_allows_legacy_session_backfill(&run.tasks[0]));
        assert!(!task_needs_legacy_session_backfill(&run.tasks[0]));

        run.tasks[0].status = TASK_STATUS_BLOCKED.to_string();
        assert!(task_allows_legacy_session_backfill(&run.tasks[0]));
        assert!(task_needs_legacy_session_backfill(&run.tasks[0]));

        run.tasks[0].provider_session_id = Some("current-provider-session".to_string());
        assert!(task_allows_legacy_session_backfill(&run.tasks[0]));
        assert!(!task_needs_legacy_session_backfill(&run.tasks[0]));
    }

    #[test]
    fn known_session_refs_prefer_latest_prompt_telemetry_for_missing_task_links() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.prompt_telemetry.push(json!({
            "label": "tdd qa generation for task-1",
            "sessionId": "older-session",
            "startedAt": Utc::now() - chrono::Duration::minutes(5),
        }));
        run.prompt_telemetry.push(json!({
            "label": "tdd qa generation for task-1",
            "sessionId": "newer-session",
            "startedAt": Utc::now(),
        }));

        let refs = known_board_session_refs(&run);

        assert_eq!(
            refs.iter()
                .find(|(_, task_id)| task_id.as_deref() == Some("task-1"))
                .map(|(session_id, _)| session_id.as_str()),
            Some("newer-session")
        );
    }

    #[test]
    fn workspace_snapshot_marks_preexisting_untracked_edits_as_unknown() {
        let root = std::env::temp_dir().join(format!(
            "iowb-server-untracked-dir-delta-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("app/src/main/java/example")).expect("create source dir");
        fs::write(
            root.join("app/src/main/java/example/Budget.kt"),
            "class Budget\n",
        )
        .expect("write source");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&root)
            .output()
            .expect("git init");

        let before = capture_workspace_snapshot(root.to_str().expect("root path"));
        fs::write(
            root.join("app/src/main/java/example/Budget.kt"),
            "class Budget(val limit: Double)\n",
        )
        .expect("update source");
        let after = capture_workspace_snapshot(root.to_str().expect("root path"));
        let delta = summarize_workspace_delta("task-1", &before, &after);
        let touched_paths = change_summary_paths(&delta);

        assert!(
            touched_paths.is_empty(),
            "delta: {}",
            serde_json::to_string_pretty(&delta).unwrap()
        );
        assert_eq!(delta["unknownChangeCount"], 1);
        assert_eq!(
            delta["unknownChanges"][0]["classification"],
            "unknown_change"
        );
        assert_eq!(
            delta["preExistingChanges"][0]["classification"],
            "pre_existing_change"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_delta_attributes_new_or_clean_files_to_subtask() {
        let before = json!({
            "isGit": true,
            "filesByPath": {},
            "files": []
        });
        let after = json!({
            "isGit": true,
            "filesByPath": {
                "src/budget.rs": {
                    "status": "??",
                    "hash": "after"
                }
            },
            "files": [{
                "status": "??",
                "path": "src/budget.rs",
                "hash": "after"
            }]
        });

        let delta = summarize_workspace_delta("task-1", &before, &after);

        assert_eq!(change_summary_paths(&delta), vec!["src/budget.rs"]);
        assert_eq!(delta["attributableChangedFileCount"], 1);
        assert_eq!(
            delta["changedBySubtask"][0]["classification"],
            "changed_by_subtask"
        );
        assert_eq!(delta["unknownChangeCount"], 0);
    }

    #[test]
    fn workspace_delta_attributes_modified_clean_tracked_files_to_subtask() {
        let before = json!({
            "isGit": true,
            "filesByPath": {
                "src/budget.rs": {
                    "status": "clean",
                    "hash": "before"
                }
            },
            "files": [{
                "status": "clean",
                "path": "src/budget.rs",
                "hash": "before"
            }]
        });
        let after = json!({
            "isGit": true,
            "filesByPath": {
                "src/budget.rs": {
                    "status": "M",
                    "hash": "after"
                }
            },
            "files": [{
                "status": "M",
                "path": "src/budget.rs",
                "hash": "after"
            }]
        });

        let delta = summarize_workspace_delta("task-1", &before, &after);

        assert_eq!(change_summary_paths(&delta), vec!["src/budget.rs"]);
        assert_eq!(delta["preExistingChangeCount"], 0);
        assert_eq!(delta["attributableChangedFileCount"], 1);
        assert_eq!(delta["unknownChangeCount"], 0);
    }

    #[test]
    fn legacy_changed_file_claims_are_downgraded_to_unknown() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].changed_files = vec!["src/budget.rs".to_string()];
        run.tasks[0].result = Some(json!({
            "status": "done",
            "changedFiles": ["src/budget.rs"]
        }));
        run.tasks[0].changed_file_summary = Some(json!({
            "touchedFiles": [{"path": "src/budget.rs", "beforeStatus": " M", "afterStatus": " M"}]
        }));

        normalize_board_model(&mut run);

        assert!(run.tasks[0].changed_files.is_empty());
        assert_eq!(
            run.tasks[0].changed_file_summary.as_ref().unwrap()["ownershipPolicy"],
            LEGACY_WORKSPACE_OWNERSHIP_POLICY
        );
        assert_eq!(
            run.tasks[0].changed_file_summary.as_ref().unwrap()["unknownChanges"][0]["classification"],
            "unknown_change"
        );
        assert_eq!(
            run.tasks[0].result.as_ref().unwrap()["changedFiles"],
            json!([])
        );
    }

    #[test]
    fn normalize_board_clears_error_on_done_tasks_with_followups() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let task = &mut run.tasks[0];
        task.status = TASK_STATUS_DONE.to_string();
        task.tdd_phase = "fix_pending".to_string();
        task.error = Some("Needs more work".to_string());
        task.result = Some(json!({
            "status": "needs_followup",
            "summary": "Created a follow-up task",
        }));

        normalize_board_model(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
        assert_eq!(run.tasks[0].error, None);
        assert_eq!(run.tasks[0].tdd_phase, "followup_pending");
    }

    #[test]
    fn immediate_resume_reuses_existing_board_worker_owner() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.status = "running".to_string();
        run.active = true;
        run.loop_started = true;
        run.current_task_id = Some("task-1".to_string());
        run.current_provider_session_id = Some("provider-session".to_string());
        request_board_pause(&mut run, Some("user request".to_string()));

        prepare_board_resume(&mut run);

        assert_eq!(run.status, "running");
        assert!(run.active);
        assert!(run.loop_started);
        assert!(!run.pause_requested);
        assert!(run.auto_run_enabled);
    }

    #[test]
    fn idle_pause_transitions_directly_to_paused() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));

        request_board_pause(&mut run, Some("user request".to_string()));

        assert_eq!(run.status, "paused");
        assert!(!run.active);
        assert!(!run.loop_started);
        assert!(!run.pause_requested);
        assert!(!run.auto_run_enabled);
    }

    #[test]
    fn only_cancellation_interrupts_active_provider_turn() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.canceled_at = Some(Utc::now());
        run.abort_source = Some("Board".to_string());
        run.abort_requested_at = Some(Utc::now());
        assert!(!board_should_abort_provider(&run));

        run.status = "pausing".to_string();
        run.pause_requested = true;
        assert!(!board_should_abort_provider(&run));

        run.status = "cancelled".to_string();
        assert!(board_should_abort_provider(&run));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn manual_hierarchy_breakdown_failure_keeps_parent_retryable() {
        let root = std::env::temp_dir().join(format!(
            "iowb-server-manual-breakdown-failure-{}",
            Uuid::new_v4()
        ));
        let project = root.join("project");
        let config_dir = root.join("config");
        fs::create_dir_all(&project).expect("project directory");
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

        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": project.display().to_string(),
            "provider": "codex"
        }));
        run.id = "manual-breakdown-board".to_string();
        run.user_id = Some("user-1".to_string());
        run.status = "paused".to_string();
        run.tasks[0].status = TASK_STATUS_BACKLOG.to_string();
        let parent = run.tasks[0].clone();
        save_board(&state, &run).expect("persist board");

        record_hierarchy_breakdown_failure(
            &state,
            "user-1",
            &run.id,
            &parent,
            "break down this story",
            Utc::now(),
            "Hierarchy breakdown provider call failed: provider unavailable",
            true,
        )
        .expect("record manual failure");

        let stored = load_user_board(&state, "user-1", &run.id)
            .expect("load board")
            .board;
        assert_eq!(stored.status, "paused");
        assert_eq!(stored.tasks[0].status, TASK_STATUS_BACKLOG);
        assert_eq!(
            stored.tasks[0].error.as_deref(),
            Some("Hierarchy breakdown provider call failed: provider unavailable")
        );
        assert_ne!(stored.status, TASK_STATUS_BLOCKED);

        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn task_ids_remain_unique_after_deletion() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        for title in ["Second task", "Third task"] {
            let request = serde_json::from_value::<TaskRequest>(json!({ "title": title })).unwrap();
            let task = BoardTask::manual(&mut run, request).unwrap();
            run.tasks.push(task);
        }

        delete_board_task(&mut run, "task-2").unwrap();
        let request =
            serde_json::from_value::<TaskRequest>(json!({ "title": "Fourth task" })).unwrap();
        let task = BoardTask::manual(&mut run, request).unwrap();

        assert_eq!(task.id, "task-4");
        assert!(!run.tasks.iter().any(|existing| existing.id == task.id));
    }

    #[test]
    fn board_detail_scrubs_removed_tracking_fields_from_legacy_values() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        run.backlog_breakdown = json!({
            "status": "idle",
            "transcript": [{
                "content": serde_json::to_string(&json!({
                    "requirementIds": ["REQ-1"],
                    "references": ["REQ-1", "src/main.rs"],
                    "required": true
                })).unwrap()
            }]
        });

        normalize_board_model(&mut run);

        let serialized = run.detail_json(None).to_string();
        assert!(!serialized.contains("requirementIds"));
        assert!(!serialized.contains("REQ-1"));
        assert_eq!(
            run.backlog_breakdown["transcript"][0]["content"],
            "{\"references\":[\"src/main.rs\"],\"required\":true}"
        );
    }

    #[test]
    fn prompt_task_or_subtask_requests_are_wrapped_at_story_level() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));

        let task_tree = prompt_task_tree_from_draft(
            &mut run,
            json!({
                "title": "Add UserSummaryCard to user page",
                "level": "subtask",
                "kind": "implementation",
                "details": "Add the UserSummaryCard composable to the user page."
            }),
            "Add card summary user in user page",
        );

        assert_eq!(task_tree.len(), 3);
        assert_eq!(task_level(&task_tree[0]), TASK_LEVEL_STORY);
        assert_eq!(task_level(&task_tree[1]), TASK_LEVEL_TASK);
        assert_eq!(task_level(&task_tree[2]), TASK_LEVEL_SUBTASK);
        assert!(!task_is_executable(&task_tree[0]));
        assert!(!task_is_executable(&task_tree[1]));
        assert!(task_is_executable(&task_tree[2]));
        assert_eq!(
            task_tree[1].hierarchy.parent_id.as_deref(),
            Some(task_tree[0].id.as_str())
        );
        assert_eq!(
            task_tree[2].hierarchy.parent_id.as_deref(),
            Some(task_tree[1].id.as_str())
        );
    }

    #[test]
    fn child_level_is_inferred_from_parent_instead_of_trusting_model_level() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let story_id = run.tasks[0].id.clone();
        let task = task_from_json(
            &run,
            json!({
                "title": "Persist budget limits",
                "level": "subtask",
                "parentId": story_id,
                "kind": "implementation"
            }),
            1,
            TASK_STATUS_TODO,
        )
        .expect("child ticket");

        assert_eq!(task_level(&task), TASK_LEVEL_TASK);
        assert!(!task_is_executable(&task));
    }

    #[test]
    fn parent_todo_approval_survives_backlog_children_until_they_are_approved() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Persist budget limits".to_string(),
            "Persist the monthly limit.".to_string(),
        );
        child.status = TASK_STATUS_BACKLOG.to_string();
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = false;
        run.tasks.push(child);

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_TODO);
    }

    #[test]
    fn only_approved_subtasks_are_runnable() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Add budget repository".to_string(),
            "Add the repository method.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(run.tasks[0].id.clone());
        child.hierarchy.executable = true;
        run.tasks.push(child);

        assert!(!task_is_runnable(&run.tasks[0]));
        assert_eq!(pick_next_task_index(&run), Some(1));
    }

    #[test]
    fn scheduler_uses_priority_then_rank_for_approved_subtasks() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        for (title, priority, rank) in [
            ("Low priority", TASK_PRIORITY_P2, 0),
            ("High priority later rank", TASK_PRIORITY_P1, 8),
            ("High priority first rank", TASK_PRIORITY_P1, 2),
        ] {
            let mut child = BoardTask::draft(&mut run, title.to_string(), title.to_string());
            child.status = TASK_STATUS_TODO.to_string();
            child.priority = priority.to_string();
            child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
            child.hierarchy.parent_id = Some(parent_id.clone());
            child.hierarchy.executable = true;
            child.hierarchy.rank = rank;
            run.tasks.push(child);
        }

        assert_eq!(pick_next_task_index(&run), Some(3));
        assert_eq!(run.tasks[3].title, "High priority first rank");
    }

    #[test]
    fn discussion_proposal_has_structured_diff_without_mutating_ticket() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let task_id = run.tasks[0].id.clone();
        let before_title = run.tasks[0].title.clone();
        let proposal = sanitize_discussion_proposal(
            &run,
            &task_id,
            "proposal-1",
            "edit",
            "Rename the story",
            &Value::Null,
            &json!({
                "action": "edit",
                "summary": "Use a clearer story title.",
                "payload": {"title": "User can manage monthly budgets"}
            }),
            "codex",
            "gpt-5.5",
            Utc::now(),
        )
        .expect("proposal");

        assert_eq!(run.tasks[0].title, before_title);
        assert_eq!(proposal["status"], "pending");
        assert_eq!(proposal["diff"]["changed"], true);
        assert!(!proposal["diff"]["changes"].as_array().unwrap().is_empty());
        assert_eq!(
            proposal["payload"]["title"],
            "User can manage monthly budgets"
        );
    }

    #[test]
    fn rejected_discussion_proposal_leaves_ticket_unchanged_until_apply() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let task_id = run.tasks[0].id.clone();
        let original = run.tasks[0].title.clone();
        let proposal = sanitize_discussion_proposal(
            &run,
            &task_id,
            "proposal-2",
            "edit",
            "Rename the story",
            &Value::Null,
            &json!({
                "action": "edit",
                "payload": {"title": "User can manage monthly budgets"}
            }),
            "codex",
            "gpt-5.5",
            Utc::now(),
        )
        .expect("proposal");
        append_discussion_proposal(&mut run, &task_id, proposal.clone()).expect("store proposal");

        let mut rejected = proposal;
        rejected["status"] = json!("rejected");
        update_discussion_proposal(&mut run, &task_id, rejected);
        assert_eq!(run.tasks[0].title, original);

        let proposal = sanitize_discussion_proposal(
            &run,
            &task_id,
            "proposal-3",
            "edit",
            "Rename the story",
            &Value::Null,
            &json!({
                "action": "edit",
                "payload": {"title": "User can manage monthly budgets"}
            }),
            "codex",
            "gpt-5.5",
            Utc::now(),
        )
        .expect("proposal");
        apply_discussion_action(
            &mut run,
            &task_id,
            proposal["action"].as_str().unwrap(),
            &proposal["payload"],
        )
        .expect("apply proposal");
        assert_eq!(run.tasks[0].title, "User can manage monthly budgets");
    }

    #[test]
    fn locked_scope_discussion_proposal_stays_pending_with_warning() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let task_id = run.tasks[0].id.clone();
        let proposal = sanitize_discussion_proposal(
            &run,
            &task_id,
            "proposal-locked",
            "edit",
            "Change the approved story",
            &Value::Null,
            &json!({
                "action": "edit",
                "payload": {"title": "Changed after approval"}
            }),
            "codex",
            "gpt-5.5",
            Utc::now(),
        )
        .expect("proposal remains pending");

        assert_eq!(proposal["status"], "pending");
        assert!(
            proposal["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|warning| warning.as_str().unwrap_or_default().contains("locked"))
        );
        assert!(apply_discussion_action(&mut run, &task_id, "edit", &proposal["payload"]).is_err());
        assert_eq!(run.tasks[0].title, "Implement budget controls");
    }

    #[test]
    fn dependency_blocker_returns_to_todo_after_completion() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let blocker_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let mut dependent = BoardTask::draft(
            &mut run,
            "Add budget repository".to_string(),
            "Add the repository method.".to_string(),
        );
        dependent.status = TASK_STATUS_BLOCKED.to_string();
        dependent.error = Some(format!("Waiting on dependency: {blocker_id}"));
        dependent.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        dependent.hierarchy.parent_id = Some(blocker_id.clone());
        dependent.hierarchy.executable = true;
        dependent.hierarchy.blocked_by = vec![blocker_id.clone()];
        dependent.depends_on = vec![blocker_id];
        run.tasks.push(dependent);

        reconcile_dependency_statuses(&mut run);

        assert_eq!(run.tasks[1].status, TASK_STATUS_TODO);
        assert_eq!(run.tasks[1].error, None);
    }

    #[test]
    fn missing_dependency_stays_blocked_with_explicit_reason() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let mut dependent = BoardTask::draft(
            &mut run,
            "Add budget repository".to_string(),
            "Add the repository method.".to_string(),
        );
        dependent.status = TASK_STATUS_TODO.to_string();
        dependent.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        dependent.hierarchy.parent_id = Some(run.tasks[0].id.clone());
        dependent.hierarchy.executable = true;
        dependent.hierarchy.blocked_by = vec!["deleted-blocker".to_string()];
        dependent.depends_on = dependent.hierarchy.blocked_by.clone();
        run.tasks.push(dependent);

        mark_dependency_blockers(&mut run);

        assert_eq!(run.tasks[1].status, TASK_STATUS_BLOCKED);
        assert_eq!(
            run.tasks[1].error.as_deref(),
            Some("Missing dependency: deleted-blocker")
        );
    }

    #[test]
    fn superseded_dependency_stays_blocked_until_user_relinks_it() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let source_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        run.tasks[0].superseded_by = Some("replacement-1".to_string());

        let mut dependent = BoardTask::draft(
            &mut run,
            "Use the approved budget behavior".to_string(),
            "Continue only after the source behavior is selected.".to_string(),
        );
        dependent.status = TASK_STATUS_TODO.to_string();
        dependent.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        dependent.hierarchy.parent_id = Some(source_id.clone());
        dependent.hierarchy.executable = true;
        dependent.hierarchy.blocked_by = vec![source_id.clone()];
        dependent.depends_on = vec![source_id];
        let dependent_id = dependent.id.clone();
        run.tasks.push(dependent);

        assert_eq!(
            unmet_task_dependencies(
                &run,
                run.tasks
                    .iter()
                    .find(|task| task.id == dependent_id)
                    .unwrap(),
            ),
            vec!["task-1"]
        );
        mark_dependency_blockers(&mut run);

        let dependent = run
            .tasks
            .iter()
            .find(|task| task.id == dependent_id)
            .unwrap();
        assert_eq!(dependent.status, TASK_STATUS_BLOCKED);
        assert!(dependent.error.as_deref().is_some_and(|error| {
            error.contains("Superseded dependency:") && error.contains("replacement-1")
        }));
        assert!(task_failure_is_dependency_blocked(dependent));

        reconcile_dependency_statuses(&mut run);
        assert_eq!(
            run.tasks
                .iter()
                .find(|task| task.id == dependent_id)
                .unwrap()
                .status,
            TASK_STATUS_BLOCKED
        );
    }

    #[test]
    fn scope_change_does_not_mark_dependency_on_retained_root_missing() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        let mut child = BoardTask::draft(
            &mut run,
            "Old generated plan".to_string(),
            "Old generated plan.".to_string(),
        );
        child.hierarchy.parent_id = Some(root_id.clone());
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.executable = false;
        run.tasks.push(child);

        let mut dependent = BoardTask::draft(
            &mut run,
            "Continue the root scope".to_string(),
            "This item depends on the retained root scope.".to_string(),
        );
        dependent.status = TASK_STATUS_TODO.to_string();
        dependent.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        dependent.hierarchy.executable = true;
        dependent.hierarchy.blocked_by = vec![root_id.clone()];
        dependent.depends_on = vec![root_id.clone()];
        let dependent_id = dependent.id.clone();
        run.tasks.push(dependent);

        let removed = delete_generated_descendants_for_scope_change(&mut run, &root_id).unwrap();
        assert_eq!(removed, 1);
        let dependent = run
            .tasks
            .iter()
            .find(|task| task.id == dependent_id)
            .unwrap();
        assert_eq!(dependent.status, TASK_STATUS_TODO);
        assert_eq!(dependent.error, None);
    }

    #[test]
    fn hierarchy_cycle_is_reported_as_a_planning_error() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        run.tasks[0].hierarchy.level = TASK_LEVEL_STORY.to_string();
        run.tasks[0].hierarchy.parent_id = Some("task-2".to_string());
        let mut child = BoardTask::draft(
            &mut run,
            "Nested story".to_string(),
            "Nested story scope.".to_string(),
        );
        child.id = "task-2".to_string();
        child.hierarchy.level = TASK_LEVEL_STORY.to_string();
        child.hierarchy.parent_id = Some(root_id);
        run.tasks.push(child);

        let issues = hierarchy_validation_issues(&run);

        assert!(issues.iter().any(|issue| issue.contains("Hierarchy cycle")));
    }

    #[test]
    fn changing_backlog_parent_removes_unexecuted_descendants() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_BACKLOG.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Add budget repository".to_string(),
            "Add the repository method.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id.clone());
        child.hierarchy.executable = true;
        run.tasks.push(child);

        edit_backlog_task(
            &mut run,
            &parent_id,
            &UpdateTaskRequest {
                title: Some("Implement revised budget controls".to_string()),
                ..UpdateTaskRequest::default()
            },
        )
        .expect("backlog parent edit succeeds");

        assert_eq!(run.tasks.len(), 1);
        assert_eq!(run.tasks[0].title, "Implement revised budget controls");
    }

    #[test]
    fn backlog_child_can_be_edited_when_parent_is_already_approved() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Run the risky migration".to_string(),
            "Apply the database migration.".to_string(),
        );
        child.status = TASK_STATUS_BACKLOG.to_string();
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = false;
        let child_id = child.id.clone();
        run.tasks.push(child);

        assert!(task_scope_owner_is_backlog(&run, &child_id));
        edit_backlog_task(
            &mut run,
            &child_id,
            &UpdateTaskRequest {
                title: Some("Run the approved migration".to_string()),
                side_effects: Some(json!(["database schema"])),
                ..UpdateTaskRequest::default()
            },
        )
        .expect("backlog child edit succeeds");

        let child = run.tasks.iter().find(|task| task.id == child_id).unwrap();
        assert_eq!(child.title, "Run the approved migration");
        assert_eq!(child.hierarchy.side_effects, vec!["database schema"]);
        assert!(!child.hierarchy.side_effects_approved);
    }

    #[test]
    fn generic_evidence_does_not_count_as_recorded_scope_effect() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_BACKLOG.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Inspect the budget flow".to_string(),
            "Inspect the current budget flow.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id.clone());
        child.hierarchy.executable = true;
        child.evidence = vec!["The current flow was inspected.".to_string()];
        let child_id = child.id.clone();
        run.tasks.push(child);

        edit_backlog_task(
            &mut run,
            &parent_id,
            &UpdateTaskRequest {
                title: Some("Implement revised budget controls".to_string()),
                ..UpdateTaskRequest::default()
            },
        )
        .expect("scope edit ignores non-effect evidence");

        assert!(!run.tasks.iter().any(|task| task.id == child_id));
    }

    #[test]
    fn completion_evidence_gate_blocks_empty_subtask_result() {
        let run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let mut task = run.tasks[0].clone();
        task.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        task.hierarchy.executable = true;
        let mut scoped = run.clone();
        scoped.tasks[0] = task;

        let result = apply_completion_evidence_gate(
            &scoped,
            "task-1",
            json!({"status": "done", "summary": "finished"}),
            &json!({"touchedFileCount": 0}),
        );

        assert_eq!(result["status"], "needs_followup");
        assert_eq!(result["qaResult"], "blocked");
        assert_eq!(result["evidenceGate"]["passed"], false);
    }

    #[test]
    fn evidence_gate_failure_cannot_be_normalized_back_to_done() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        run.tasks[0].result = Some(json!({
            "status": "needs_followup",
            "summary": "No evidence",
            "evidenceGate": {
                "passed": false,
                "kind": "implementation"
            }
        }));

        normalize_done_followup_task(&mut run.tasks[0]);

        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
        assert_eq!(run.tasks[0].qa_passed, Some(false));
        assert!(run.tasks[0].completed_at.is_none());
    }

    #[test]
    fn completed_attempt_keeps_its_execution_snapshot() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let started_at = Utc::now();
        let finished_at = started_at + chrono::Duration::seconds(3);
        let task = &mut run.tasks[0];
        task.transcript = vec![json!({
            "timestamp": started_at,
            "kind": "status",
            "content": "Task execution started"
        })];
        task.commands_run = vec!["cargo test".to_string()];
        task.changed_files = vec!["src/budget.rs".to_string()];
        task.evidence = vec!["The budget flow passed.".to_string()];
        task.summary = "Budget flow implemented".to_string();
        task.hierarchy.attempts.push(json!({
            "attemptId": "attempt-1",
            "attemptNumber": 1,
            "startedAt": started_at,
            "status": "running",
            "transcriptStartIndex": 0
        }));

        finish_task_attempt(
            &mut run,
            "task-1",
            "attempt-1",
            TASK_STATUS_DONE,
            finished_at,
        );

        let attempt = &run.tasks[0].hierarchy.attempts[0];
        assert_eq!(attempt["status"], TASK_STATUS_DONE);
        assert_eq!(attempt["commands"][0], "cargo test");
        assert_eq!(attempt["filesChanged"][0], "src/budget.rs");
        assert_eq!(attempt["evidence"][0], "The budget flow passed.");
        assert_eq!(
            attempt["transcript"][0]["content"],
            "Task execution started"
        );
        assert_eq!(attempt["summary"], "Budget flow implemented");
        assert!(attempt.get("transcriptStartIndex").is_none());
    }

    #[test]
    fn external_side_effects_require_declaration_and_approval_before_todo() {
        let mut run = board_fixture(json!({
            "command": "Implement database migration",
            "projectPath": "/tmp/project"
        }));
        let mut task = BoardTask::draft(
            &mut run,
            "Run database migration".to_string(),
            "Apply the database migration.".to_string(),
        );
        task.status = TASK_STATUS_TODO.to_string();
        task.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        task.hierarchy.parent_id = Some(run.tasks[0].id.clone());
        task.hierarchy.executable = true;
        task.hierarchy.side_effects = vec!["database schema".to_string()];
        run.tasks[0].status = TASK_STATUS_TODO.to_string();

        let error = validate_manual_task_status(&run, &task).unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(error.body.error.contains("approval"));

        task.hierarchy.side_effects_approved = true;
        validate_manual_task_status(&run, &task).expect("approved side effects are runnable");
    }

    #[test]
    fn revoking_side_effect_approval_blocks_a_todo_subtask() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Run approved migration".to_string(),
            "Apply the database migration after approval.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.task_type = TASK_KIND_MIGRATION.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = true;
        child.hierarchy.side_effects = vec!["database schema".to_string()];
        child.hierarchy.side_effects_approved = true;
        let child_id = child.id.clone();
        run.tasks.push(child);

        approve_task_side_effects_in_board(&mut run, &child_id, "user-1", false, None)
            .expect("revocation should be accepted");

        let child = run
            .tasks
            .iter()
            .find(|task| task.id == child_id)
            .expect("migration subtask");
        assert_eq!(child.status, TASK_STATUS_BLOCKED);
        assert!(
            child
                .error
                .as_deref()
                .is_some_and(|error| error.contains("approval"))
        );
    }

    #[test]
    fn completion_evidence_gate_requires_external_side_effect_evidence() {
        let run = board_fixture(json!({
            "command": "Implement migration",
            "projectPath": "/tmp/project"
        }));
        let mut task = run.tasks[0].clone();
        task.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        task.hierarchy.executable = true;
        task.hierarchy.side_effects = vec!["database schema".to_string()];
        task.hierarchy.side_effects_approved = true;
        let mut scoped = run.clone();
        scoped.tasks[0] = task;

        let missing = apply_completion_evidence_gate(
            &scoped,
            "task-1",
            json!({
                "status": "done",
                "summary": "migration applied",
                "changedFiles": ["migrations/001.sql"],
                "commandsRun": ["migration command"],
                "evidence": ["migration completed"]
            }),
            &json!({
                "touchedFileCount": 1,
                "attributableChangedFileCount": 1,
                "changedBySubtask": [{"path": "migrations/001.sql"}]
            }),
        );
        assert_eq!(missing["status"], "needs_followup");
        assert_eq!(missing["evidenceGate"]["passed"], false);
        assert!(
            missing["remainingIssues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue
                    .as_str()
                    .unwrap_or_default()
                    .contains("externalSideEffects"))
        );

        let recorded = apply_completion_evidence_gate(
            &scoped,
            "task-1",
            json!({
                "status": "done",
                "summary": "migration applied",
                "changedFiles": ["migrations/001.sql"],
                "commandsRun": ["migration command"],
                "evidence": ["migration completed"],
                "externalSideEffects": ["Database schema changed as planned."]
            }),
            &json!({
                "touchedFileCount": 1,
                "attributableChangedFileCount": 1,
                "changedBySubtask": [{"path": "migrations/001.sql"}]
            }),
        );
        assert_eq!(recorded["status"], "done");
    }

    #[test]
    fn manual_test_completion_requires_and_records_environment() {
        let run = board_fixture(json!({
            "command": "Verify budget controls",
            "projectPath": "/tmp/project"
        }));
        let mut task = run.tasks[0].clone();
        task.task_type = TASK_KIND_MANUAL_TEST.to_string();
        task.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        task.hierarchy.executable = true;
        let mut scoped = run.clone();
        scoped.tasks[0] = task;

        let missing = apply_completion_evidence_gate(
            &scoped,
            "task-1",
            json!({
                "status": "done",
                "summary": "The flow works",
                "evidence": ["Opened the budget screen"]
            }),
            &json!({ "touchedFileCount": 0 }),
        );
        assert_eq!(missing["status"], "needs_followup");
        assert!(
            missing["remainingIssues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue
                    .as_str()
                    .unwrap_or_default()
                    .contains("manualTestEnvironment"))
        );

        let recorded = apply_completion_evidence_gate(
            &scoped,
            "task-1",
            json!({
                "status": "done",
                "summary": "The flow works",
                "evidence": ["Opened the budget screen"],
                "manualTestEnvironment": {
                    "device": "emulator-5560",
                    "version": "debug-42",
                    "baseUrl": "http://10.0.2.2:8100"
                },
                "manualSteps": [
                    "Open the budget screen",
                    "Enter a valid amount and save it"
                ],
                "manualResult": "Pass: the saved amount is shown after reopening the screen."
            }),
            &json!({ "touchedFileCount": 0 }),
        );
        assert_eq!(recorded["status"], "done");
        assert_eq!(
            recorded["manualTestEnvironment"]["deviceOrEmulator"],
            "emulator-5560"
        );
        assert_eq!(recorded["manualTestEnvironment"]["appVersion"], "debug-42");
        assert_eq!(
            recorded["manualTestEnvironment"]["backendUrl"],
            "http://10.0.2.2:8100"
        );
        assert_eq!(recorded["manualTestSteps"][0], "Open the budget screen");
        assert!(
            recorded["manualTestResult"]
                .as_str()
                .unwrap()
                .starts_with("Pass:")
        );
    }

    #[test]
    fn scope_effect_resolution_demotes_scope_and_creates_explicit_revert_work() {
        let mut run = board_fixture(json!({
            "command": "Update budget controls",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        run.tasks[0].priority = TASK_PRIORITY_P1.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Update budget persistence".to_string(),
            "Change the budget persistence implementation.".to_string(),
        );
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(root_id.clone());
        child.hierarchy.executable = false;
        child.status = TASK_STATUS_TODO.to_string();
        child.changed_files = vec!["src/budget.rs".to_string()];
        let child_id = child.id.clone();
        run.tasks.push(child);

        resolve_scope_effects_in_board(
            &mut run,
            &root_id,
            "user-1",
            "revert",
            Some("The parent scope changed.".to_string()),
        )
        .expect("scope effects should be resolvable while paused");

        let root = run.tasks.iter().find(|task| task.id == root_id).unwrap();
        assert_eq!(root.status, TASK_STATUS_BACKLOG);
        assert!(!run.tasks.iter().any(|task| task.id == child_id));
        let revert = run
            .tasks
            .iter()
            .find(|task| task.task_type == TASK_KIND_REVERT)
            .expect("explicit revert work");
        assert_eq!(revert.status, TASK_STATUS_BACKLOG);
        assert_eq!(revert.priority, TASK_PRIORITY_P1);
        assert!(revert.hierarchy.executable);
        assert!(
            revert
                .hierarchy
                .planned_files
                .contains(&"src/budget.rs".to_string())
        );
    }

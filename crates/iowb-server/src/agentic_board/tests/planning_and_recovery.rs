    #[test]
    fn project_execution_owner_allows_one_board_at_a_time() {
        let project_path = format!("/tmp/iowb-project-{}", Uuid::new_v4());
        assert!(claim_project_execution(&project_path, "board-a").unwrap());
        assert!(!claim_project_execution(&project_path, "board-a").unwrap());
        let error = claim_project_execution(&project_path, "board-b").unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        release_project_execution(&project_path, "board-a");
        assert!(claim_project_execution(&project_path, "board-b").unwrap());
        release_project_execution(&project_path, "board-b");
    }

    #[test]
    fn legacy_task_sequence_migrates_from_existing_ids() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].id = "task-3".to_string();
        run.next_task_sequence = 0;

        assert_eq!(allocate_task_id(&mut run), "task-4");
        assert_eq!(run.next_task_sequence, 4);
    }

    #[test]
    fn prompt_drafts_allocate_unique_ids_after_gaps() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.tasks.push(BoardTask::draft(
            &mut run.clone(),
            "Third task".to_string(),
            "Third task".to_string(),
        ));
        run.tasks[1].id = "task-3".to_string();
        run.next_task_sequence = 0;

        let drafts = prompt_to_task_drafts(&mut run, "One\nTwo\nThree");
        let ids = drafts
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["task-4", "task-5", "task-6"]);
    }

    #[test]
    fn task_dependencies_keep_missing_cards_as_blockers() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let mut dependent =
            BoardTask::draft(&mut run, "Dependent".to_string(), "Dependent".to_string());
        dependent.depends_on = vec![
            dependent.id.clone(),
            "missing-task".to_string(),
            "task-1".to_string(),
        ];
        run.tasks.push(dependent.clone());

        assert_eq!(
            unmet_task_dependencies(&run, &dependent),
            vec!["missing-task", "task-1"]
        );

        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        assert_eq!(
            unmet_task_dependencies(&run, &dependent),
            vec!["missing-task"]
        );
    }

    #[test]
    fn generated_plan_dependencies_map_local_keys_to_server_ids() {
        let run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let drafts = sanitize_prompt_task_drafts(
            &json!({
                "tasks": [
                    {
                        "planKey": "storage",
                        "title": "Add storage layer",
                        "details": "Persist account records.",
                        "dependsOn": []
                    },
                    {
                        "planKey": "reports",
                        "title": "Add reporting",
                        "details": "Show account reports.",
                        "dependsOn": ["plan:storage"]
                    }
                ]
            }),
            "Generate account backlog",
        );
        let mut candidate = run.clone();
        let trees =
            prepare_generated_prompt_task_trees(&mut candidate, drafts, "Generate account backlog");
        let trees = trees.expect("generated plan should be valid");
        let storage_id = trees[0][0].id.clone();

        assert_eq!(trees[1][0].depends_on, vec![storage_id]);
        assert_eq!(run.tasks[0].id, "task-1");
        assert_eq!(trees[0][0].id, "task-2");
        assert_eq!(trees[1][0].id, "task-3");
    }

    #[test]
    fn hierarchy_breakdown_preserves_blocked_by_and_inherits_parent_prerequisites() {
        let mut run = board_fixture(json!({
            "command": "Build a small app",
            "projectPath": "/tmp/project"
        }));
        let prerequisite = BoardTask::draft(
            &mut run,
            "Agree on the data format".to_string(),
            "Record the data format.".to_string(),
        );
        let prerequisite_id = prerequisite.id.clone();
        run.tasks.push(prerequisite);
        run.tasks[0].hierarchy.blocked_by = vec![prerequisite_id.clone()];
        let parent = run.tasks[0].clone();
        let first_new = run.tasks.len();
        let mut local_ids = BTreeMap::new();
        for (index, item) in [
            json!({"id": "verify", "title": "Verify rendered content", "blockedBy": ["plan:render"]}),
            json!({"id": "render", "title": "Render content"}),
        ]
        .into_iter()
        .enumerate()
        {
            let mut child = task_from_json(&run, item, index, TASK_STATUS_TODO).unwrap();
            let local_id = child.id.clone();
            inherit_hierarchy_breakdown_dependencies(&mut child, &parent);
            child.id = unique_task_id(&run, &format!("{}-{}", parent.id, local_id));
            child.hierarchy.parent_id = Some(parent.id.clone());
            local_ids.insert(local_id, child.id.clone());
            run.tasks.push(child);
        }

        remap_hierarchy_breakdown_dependencies(&mut run, first_new, &local_ids).unwrap();

        assert_eq!(
            task_blockers(&run.tasks[first_new]),
            vec![local_ids["render"].clone(), prerequisite_id.clone()]
        );
        assert_eq!(
            task_blockers(&run.tasks[first_new + 1]),
            vec![prerequisite_id]
        );
        assert_eq!(
            run.tasks[first_new].depends_on,
            run.tasks[first_new].hierarchy.blocked_by
        );

        // A further breakdown must retain the prerequisite on the eventual
        // executable subtask, not just on its planning parent.
        let parent = run.tasks[first_new].clone();
        let mut grandchild = task_from_json(
            &run,
            json!({"id": "browser", "title": "Run browser verification"}),
            0,
            TASK_STATUS_TODO,
        )
        .unwrap();
        inherit_hierarchy_breakdown_dependencies(&mut grandchild, &parent);
        grandchild.hierarchy.parent_id = Some(parent.id.clone());
        let first_new = run.tasks.len();
        run.tasks.push(grandchild);
        remap_hierarchy_breakdown_dependencies(&mut run, first_new, &BTreeMap::new()).unwrap();
        assert_eq!(task_blockers(&run.tasks[first_new]), task_blockers(&parent));
    }

    #[test]
    fn hierarchy_breakdown_remaps_legacy_ids_after_unique_server_assignment() {
        let mut run = board_fixture(json!({
            "command": "Build an app",
            "projectPath": "/tmp/project"
        }));
        let parent = run.tasks[0].clone();
        let mut existing = BoardTask::draft(&mut run, "Existing work".into(), "Existing work".into());
        existing.id = format!("{}-render", parent.id);
        run.tasks.push(existing);
        let first_new = run.tasks.len();
        let mut local_ids = BTreeMap::new();
        for (index, item) in [
            json!({"id": "render", "title": "Render new content"}),
            json!({"id": "verify", "title": "Verify new content", "blockedBy": ["render"], "dependsOn": ["board:task-1-render"]}),
        ]
        .into_iter()
        .enumerate()
        {
            let mut child = task_from_json(&run, item, index, TASK_STATUS_TODO).unwrap();
            let local_id = child.id.clone();
            child.id = unique_task_id(&run, &format!("{}-{}", parent.id, child.id));
            local_ids.insert(local_id, child.id.clone());
            run.tasks.push(child);
        }

        remap_hierarchy_breakdown_dependencies(&mut run, first_new, &local_ids).unwrap();

        assert_eq!(local_ids["render"], "task-1-render-2");
        assert_eq!(
            task_blockers(&run.tasks[first_new + 1]),
            vec!["task-1-render-2".to_string(), "task-1-render".to_string()]
        );
    }

    #[test]
    fn hierarchy_breakdown_preserves_wrapped_child_dependencies_and_approval() {
        let mut run = board_fixture(json!({
            "command": "Build an app",
            "projectPath": "/tmp/project"
        }));
        let parent = run.tasks[0].clone();
        let first_new = run.tasks.len();
        let mut ordinary = task_from_json(
            &run,
            json!({
                "id": "prepare", "title": "Prepare migration scripts",
            }),
            0,
            TASK_STATUS_TODO,
        )
        .unwrap();
        ordinary.id = "task-1-prepare".to_string();
        run.tasks.push(ordinary);
        let risky = task_from_json(
            &run,
            json!({
                "id": "migrate", "title": "Apply production migration",
                "blockedBy": ["plan:prepare"],
                "sideEffects": ["Change production database schema"],
            }),
            1,
            TASK_STATUS_TODO,
        )
        .unwrap();
        assert!(generated_child_requires_backlog_approval(&risky));
        let wrapped = wrap_generated_child_in_backlog(
            &mut run,
            &parent,
            risky,
            TASK_LEVEL_TASK,
            "apply production migration",
        );
        let risky_id = wrapped.last().unwrap().id.clone();
        run.tasks.extend(wrapped);
        let mut verification = task_from_json(
            &run,
            json!({
                "id": "verify", "title": "Verify production migration",
                "blockedBy": ["plan:migrate"],
            }),
            2,
            TASK_STATUS_TODO,
        )
        .unwrap();
        verification.id = "task-1-verify".to_string();
        run.tasks.push(verification);
        let local_ids = BTreeMap::from([
            ("prepare".to_string(), "task-1-prepare".to_string()),
            ("migrate".to_string(), risky_id.clone()),
            ("verify".to_string(), "task-1-verify".to_string()),
        ]);

        remap_hierarchy_breakdown_dependencies(&mut run, first_new, &local_ids).unwrap();

        let risky = run.tasks.iter().find(|task| task.id == risky_id).unwrap();
        assert_eq!(task_blockers(risky), vec!["task-1-prepare"]);
        assert_eq!(risky.status, TASK_STATUS_BACKLOG);
        assert!(!risky.hierarchy.side_effects_approved);
        assert_eq!(task_blockers(run.tasks.last().unwrap()), vec![risky_id]);
    }

    #[test]
    fn hierarchy_breakdown_rejects_unknown_self_ancestor_and_ambiguous_dependencies() {
        for reference in [
            "plan:discarded",
            "board:missing",
            "plan:verify",
            "board:task-1",
            "task-1",
        ] {
            let mut run = board_fixture(json!({
                "command": "Build an app",
                "projectPath": "/tmp/project"
            }));
            let first_new = run.tasks.len();
            let mut task = task_from_json(
                &run,
                json!({
                    "id": "verify", "title": "Verify app", "blockedBy": [reference],
                }),
                0,
                TASK_STATUS_TODO,
            )
            .unwrap();
            task.id = "task-1-verify".to_string();
            task.hierarchy.parent_id = Some("task-1".to_string());
            run.tasks.push(task);
            let local_ids = BTreeMap::from([
                ("verify".to_string(), "task-1-verify".to_string()),
                ("task-1".to_string(), "task-1-render".to_string()),
            ]);
            let before = task_blockers(&run.tasks[first_new]);

            assert!(
                remap_hierarchy_breakdown_dependencies(&mut run, first_new, &local_ids).is_err(),
                "{reference}"
            );
            assert_eq!(task_blockers(&run.tasks[first_new]), before);
        }
    }

    #[test]
    fn hierarchy_breakdown_prompt_explains_dependencies_external_effects_and_tdd() {
        let mut run = board_fixture(json!({
            "command": "Build Pocket Tasks with localStorage",
            "projectPath": "/tmp/project",
            "tddEnabled": true
        }));
        let prompt = build_hierarchy_breakdown_prompt(&run, &run.tasks[0], TASK_LEVEL_TASK);
        assert!(prompt.contains("plan:<id>"));
        assert!(prompt.contains("verification depends on the implementation"));
        assert!(prompt.contains("Ordinary\n  project-file edits"));
        assert!(prompt.contains("localStorage behavior"));
        assert!(prompt.contains("outside the isolated test environment"));
        assert!(prompt.contains("TDD is enabled"));
        assert!(prompt.contains("executable automated tests, not a README/manual checklist"));

        run.tdd_enabled = false;
        let prompt = build_hierarchy_breakdown_prompt(&run, &run.tasks[0], TASK_LEVEL_TASK);
        assert!(prompt.contains("TDD is disabled"));
        assert!(!prompt.contains("recording the expected failing result"));
    }

    #[test]
    fn hierarchical_execution_prompts_scope_result_fields_to_current_ticket() {
        let mut run = board_fixture(json!({
            "command": "Build a complete app",
            "projectPath": "/tmp/project"
        }));
        let task = &mut run.tasks[0];
        task.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        task.hierarchy.executable = true;
        task.description = "Create the static document shell only.".to_string();
        task.acceptance_criteria = vec!["A title and empty list container exist.".to_string()];
        for kind in [TASK_KIND_IMPLEMENTATION, TASK_KIND_TEST_IMPLEMENTATION, TASK_KIND_QA, TASK_KIND_MANUAL_TEST] {
            run.tasks[0].task_type = kind.to_string();
            let prompt = build_task_execution_prompt(&run, &run.tasks[0], 0);
            assert!(prompt.contains("Create the static document shell only."), "{kind}");
            assert!(prompt.contains("not completion of the entire app or parent scope"), "{kind}");
            assert!(prompt.contains("They are not a roadmap of later work"), "{kind}");
            assert!(prompt.contains("do not put it in remainingIssues, remainingGaps, or duplicate suggestedBacklogTasks"), "{kind}");
            assert!(prompt.contains("Do not excuse an unmet acceptance criterion as downstream work"), "{kind}");
        }
        let repair = build_task_result_repair_prompt(
            &run, &run.tasks[0], 0, &json!({}), "previous output", &json!({}), &["Missing status".to_string()],
        );
        assert!(repair.contains("Scope-local result reporting"));
    }

    #[test]
    fn scoped_downstream_notes_do_not_override_evidence_or_real_incompleteness() {
        let mut run = board_fixture(json!({
            "command": "Build a complete app",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        run.tasks[0].hierarchy.executable = true;
        run.tasks[0].task_type = TASK_KIND_IMPLEMENTATION.to_string();
        let task_id = run.tasks[0].id.clone();
        let shell_result = json!({
            "status": "done",
            "qaResult": "pass",
            "summary": "Static shell complete; behavior and styling belong to later tickets.",
            "changedFiles": ["index.html"],
            "commandsRun": ["validate HTML"],
            "evidence": ["Title and empty list container exist."],
            "remainingIssues": [],
            "remainingGaps": [],
        });
        let change_summary = json!({
            "touchedFileCount": 1,
            "attributableChangedFileCount": 1,
            "changedBySubtask": [{"path": "index.html"}],
        });
        let accepted = apply_completion_evidence_gate(&run, &task_id, shell_result.clone(), &change_summary);
        assert_eq!(accepted["status"], "done");
        assert_eq!(accepted["evidenceGate"]["passed"], true);
        assert!(!append_followup_task_if_needed(&mut run, &task_id, &accepted));

        // Older executors used remainingGaps for roadmap notes. The reported
        // live failure must not be misdiagnosed as a prose-based blocking gate.
        let mut legacy_result = shell_result.clone();
        legacy_result["remainingGaps"] = json!(["Behavior and styling are assigned to later tickets."]);
        let legacy_accepted = apply_completion_evidence_gate(&run, &task_id, legacy_result, &change_summary);
        assert_eq!(legacy_accepted["status"], "done");
        assert!(!append_followup_task_if_needed(&mut run, &task_id, &legacy_accepted));

        let missing_evidence = apply_completion_evidence_gate(&run, &task_id, shell_result, &json!({"attributableChangedFileCount": 0}));
        assert_eq!(missing_evidence["status"], "needs_followup");
        assert_eq!(missing_evidence["evidenceGate"]["passed"], false);

        let real_defect = json!({
            "status": "needs_followup",
            "qaResult": "fail",
            "remainingIssues": ["Required title and task-list container are missing."],
        });
        assert!(!append_followup_task_if_needed(&mut run, &task_id, &real_defect));
        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
    }

    #[test]
    fn hierarchy_breakdown_reuses_existing_and_wrapped_dependency_targets() {
        let mut run = board_fixture(json!({
            "command": "Build an app",
            "projectPath": "/tmp/project"
        }));
        let parent = run.tasks[0].clone();
        let mut existing = task_from_json(&run, json!({
            "id": "existing-shell", "title": "Implement app shell",
        }), 0, TASK_STATUS_TODO).unwrap();
        existing.hierarchy.level = TASK_LEVEL_TASK.to_string();
        existing.hierarchy.parent_id = Some(parent.id.clone());
        run.tasks.push(existing);
        let risky = task_from_json(&run, json!({
            "id": "migration", "title": "Apply production migration",
            "sideEffects": ["Change production database schema"],
        }), 1, TASK_STATUS_TODO).unwrap();
        let wrapped = wrap_generated_child_in_backlog(
            &mut run, &parent, risky, TASK_LEVEL_TASK, "apply production migration",
        );
        let wrapped_id = wrapped.last().unwrap().id.clone();
        run.tasks.extend(wrapped);

        let retained = retained_hierarchy_breakdown_child_id(
            &run, &parent.id, TASK_LEVEL_TASK, "Implement app shell",
        ).unwrap();
        assert_eq!(retained, "existing-shell");
        assert_eq!(retained_hierarchy_breakdown_child_id(
            &run, &parent.id, TASK_LEVEL_TASK, "Apply production migration",
        ), Some(wrapped_id.clone()));

        let first_new = run.tasks.len();
        let verification = task_from_json(&run, json!({
            "id": "verify", "title": "Verify retained work",
            "blockedBy": ["plan:shell", "Apply production migration"],
        }), 2, TASK_STATUS_TODO).unwrap();
        run.tasks.push(verification);
        remap_hierarchy_breakdown_dependencies(
            &mut run, first_new, &BTreeMap::from([("shell".to_string(), retained)]),
        ).unwrap();
        assert_eq!(task_blockers(run.tasks.last().unwrap()), vec!["existing-shell".to_string(), wrapped_id]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hierarchy_breakdown_contract_failure_is_persisted_and_clears_own_provider_activity() {
        let root = std::env::temp_dir().join(format!("iowb-breakdown-contract-{}", Uuid::new_v4()));
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
        }).await.unwrap();
        let mut run = board_fixture(json!({
            "command": "Build an app",
            "projectPath": project.display().to_string(),
            "provider": "codex"
        }));
        run.id = "breakdown-contract".to_string();
        run.user_id = Some("user-1".to_string());
        run.status = "paused".to_string();
        run.tasks[0].status = TASK_STATUS_BACKLOG.to_string();
        let parent = run.tasks[0].clone();
        for failure in [
            "Hierarchy breakdown returned duplicate item id: shell.",
            "Hierarchy breakdown returned invalid dependencies: missing prerequisite.",
        ] {
            run.provider_call_label = Some(format!("hierarchy breakdown for {}", parent.id));
            run.provider_call_started_at = Some(Utc::now());
            run.current_provider_session_id = Some("planning-session".to_string());
            save_board(&state, &run).unwrap();
            record_hierarchy_breakdown_failure(
                &state, "user-1", &run.id, &parent, "Break down the app", Utc::now(), failure, true,
            ).unwrap();
            let stored = load_user_board(&state, "user-1", &run.id).unwrap().board;
            assert_eq!(stored.tasks.len(), 1);
            assert_eq!(stored.tasks[0].error.as_deref(), Some(failure));
            assert_eq!(stored.tasks[0].status, TASK_STATUS_BACKLOG);
            assert!(is_retryable_hierarchy_breakdown_task(&stored.tasks[0]));
            assert!(stored.provider_call_started_at.is_none());
            assert!(stored.provider_call_label.is_none());
            assert!(stored.current_provider_session_id.is_none());
            assert_eq!(stored.tasks[0].transcript.last().unwrap()["content"], failure);
        }
        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn manual_breakdown_persists_after_http_waiter_is_cancelled() {
        let root = std::env::temp_dir().join(format!("iowb-breakdown-lifecycle-{}", Uuid::new_v4()));
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
        }).await.unwrap();
        let mut run = board_fixture(json!({
            "command": "Build an app",
            "projectPath": project.display().to_string(),
        }));
        run.user_id = Some("lifecycle-user".to_string());
        run.provider_call_started_at = Some(Utc::now());
        run.provider_call_label = Some("hierarchy breakdown for task-1".to_string());
        save_board(&state, &run).unwrap();
        let board_id = run.id.clone();
        let operation = claim_manual_breakdown_operation("lifecycle-user", &board_id).unwrap();
        let operation_state = state.clone();
        let operation_board_id = board_id.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let http_waiter = tokio::spawn(async move {
            await_manual_breakdown_completion(operation, async move {
                started_tx.send(()).unwrap();
                release_rx.await.unwrap();
                // A delayed provider result reaches the same persistence
                // boundary after the HTTP client has stopped waiting.
                let _guard = board_mutation_lock();
                let mut stored = load_user_board(&operation_state, "lifecycle-user", &operation_board_id)?;
                let mut child = BoardTask::draft(
                    &mut stored.board, "Generated delivery task".to_string(), "Build the app shell".to_string(),
                );
                child.hierarchy.parent_id = Some(stored.board.tasks[0].id.clone());
                child.hierarchy.level = TASK_LEVEL_TASK.to_string();
                stored.board.tasks.push(child);
                stored.board.provider_call_started_at = None;
                stored.board.provider_call_label = None;
                stored.board.touch();
                save_board(&operation_state, &stored.board)?;
                finished_tx.send(()).unwrap();
                Ok(1)
            }).await
        });
        timeout(Duration::from_secs(2), started_rx).await.unwrap().unwrap();
        http_waiter.abort();
        assert!(http_waiter.await.unwrap_err().is_cancelled());
        assert_eq!(claim_manual_breakdown_operation("lifecycle-user", &board_id).unwrap_err().status, StatusCode::CONFLICT);
        release_tx.send(()).unwrap();
        timeout(Duration::from_secs(2), finished_rx).await.unwrap().unwrap();

        let stored = load_user_board(&state, "lifecycle-user", &board_id).unwrap().board;
        assert_eq!(stored.tasks.len(), 2);
        assert_eq!(stored.tasks[1].title, "Generated delivery task");
        assert!(stored.provider_call_started_at.is_none());
        assert!(stored.provider_call_label.is_none());
        assert!(claim_manual_breakdown_operation("lifecycle-user", &board_id).is_ok());
        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn manual_breakdown_returns_outcome_and_releases_single_flight_claim() {
        let board_id = Uuid::new_v4().to_string();
        let operation = claim_manual_breakdown_operation("user", &board_id).unwrap();
        assert_eq!(await_manual_breakdown_completion(operation, async { Ok(3) }).await.unwrap(), 3);
        let operation = claim_manual_breakdown_operation("user", &board_id).unwrap();
        let failure = await_manual_breakdown_completion(operation, async {
            Err(bad_request("invalid generated plan"))
        }).await.unwrap_err();
        assert_eq!(failure.status, StatusCode::BAD_REQUEST);
        assert_eq!(failure.body.error, "invalid generated plan");
        assert!(claim_manual_breakdown_operation("user", &board_id).is_ok());
    }

    #[test]
    fn generated_plan_rejects_discarded_seed_dependency_without_mutating_source_board() {
        let run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let drafts = sanitize_prompt_task_drafts(
            &json!({
                "tasks": [{
                    "planKey": "reports",
                    "title": "Add reporting",
                    "details": "Show account reports.",
                    "dependsOn": ["task-1"]
                }]
            }),
            "Generate account backlog",
        );
        let mut candidate = run.clone();

        let error =
            prepare_generated_prompt_task_trees(&mut candidate, drafts, "Generate account backlog")
                .expect_err("discarded seed dependency must reject the generated plan");

        assert!(server_error_message(&error).contains("task-1"));
        assert_eq!(run.tasks.len(), 1);
        assert_eq!(run.next_task_sequence, 1);
    }

    #[test]
    fn prompt_draft_context_excludes_generation_placeholders() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.backlog_breakdown = json!({
            "id": "breakdown-1",
            "status": "running",
            "prompt": "Generate a task",
            "model": "gpt-5.6-sol",
            "boardProfile": "minimal"
        });

        let prompt = build_prompt_task_draft_prompt(&run, "Generate a task", "minimal");

        assert!(!prompt.contains("breakdown-1 ["));
        assert!(prompt.contains("task-1 [backlog] Initial task"));
        assert!(prompt.contains("\"planKey\""));
        assert!(prompt.contains("plan:<planKey>"));
    }

    #[test]
    fn deleting_parent_deletes_descendants_completely() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let mut dependent =
            BoardTask::draft(&mut run, "Dependent".to_string(), "Dependent".to_string());
        dependent.depends_on = vec!["task-1".to_string(), "external".to_string()];
        dependent.source_task_id = Some("task-1".to_string());
        dependent.source_qa_task_id = Some("task-1".to_string());
        run.tasks.push(dependent);

        delete_board_task(&mut run, "task-1").unwrap();

        assert!(run.tasks.is_empty());
    }

    #[test]
    fn deleting_current_task_is_rejected() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.current_task_id = Some("task-1".to_string());

        let error = delete_board_task(&mut run, "task-1").unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(run.tasks.len(), 1);
    }

    #[test]
    fn invalid_create_schedule_is_rejected() {
        let request = serde_json::from_value::<CreateBoardRequest>(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project",
            "scheduledStartAt": "not-a-timestamp"
        }))
        .unwrap();

        let error = AgenticBoard::new(None, request).unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.body.error.contains("valid RFC3339"));
    }

    #[test]
    fn new_board_without_schedule_starts_as_backlog_planning_board() {
        let run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));

        assert_eq!(run.status, "paused");
        assert!(!run.active);
        assert!(!run.loop_started);
        assert!(!run.auto_run_enabled);
        assert_eq!(run.scheduled_start_at, None);
        assert_eq!(
            run.pause_reason.as_deref(),
            Some("Board created with backlog planning item.")
        );
        assert_eq!(run.provider_call_started_at, None);
        assert_eq!(run.current_provider_session_id, None);
        assert_eq!(run.tasks.len(), 1);
        assert_eq!(run.tasks[0].status, TASK_STATUS_BACKLOG);
    }

    #[test]
    fn board_strategy_enables_gpt_5_6_sol_fast_mode() {
        let run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project",
            "provider": "codex",
            "model": "gpt-5.6-sol",
            "modelStrategy": {
                "reasoningEffort": "low",
                "serviceTier": "fast"
            }
        }));

        assert_eq!(
            board_provider_controls(&run),
            BoardProviderControls {
                effort: Some("low".to_string()),
                thinking: None,
                fast: Some(true),
            }
        );
    }

    #[test]
    fn prompt_task_generation_records_model_fast_mode_and_usage() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project",
            "provider": "codex",
            "model": "gpt-5.6-sol",
            "modelStrategy": {
                "reasoningEffort": "low",
                "serviceTier": "fast"
            }
        }));
        let attempt = PromptTaskDraftAttempt {
            result: Ok((vec![json!({ "title": "Generated task" })], None)),
            provider_prompt: "Generate one focused task".to_string(),
            provider_output: r#"{"tasks":[{"title":"Generated task"}]}"#.to_string(),
            session_id: Some("session-1".to_string()),
            token_usage: Some(json!({
                "inputTokens": 12,
                "cachedInputTokens": 2,
                "outputTokens": 8,
                "totalTokens": 20,
            })),
            effective_provider: "codex".to_string(),
            effective_model: "gpt-5.6-sol".to_string(),
            started_at: Utc::now(),
        };

        record_prompt_task_generation_attempt(
            &mut run,
            "Kanban backlog prompt generation",
            &attempt,
        );

        let telemetry = run.prompt_telemetry.last().unwrap();
        assert_eq!(telemetry["provider"], "codex");
        assert_eq!(telemetry["effectiveModel"], "gpt-5.6-sol");
        assert_eq!(telemetry["reasoningEffort"], "low");
        assert_eq!(telemetry["fast"], true);
        assert_eq!(telemetry["outcome"], "completed");
        assert_eq!(telemetry["actualTotalTokens"], 20);
        assert_eq!(run.provider_usage["totalTokens"], 20);
        assert_eq!(run.provider_usage["invocationsWithUsage"], 1);
        assert_eq!(run.last_effective_model.as_deref(), Some("gpt-5.6-sol"));
    }

    #[test]
    fn prompt_task_generation_transcript_preserves_prompt_and_output() {
        let attempt = PromptTaskDraftAttempt {
            result: Ok((vec![json!({ "title": "Generated task" })], None)),
            provider_prompt: "Generate one focused task".to_string(),
            provider_output: "Created two backlog cards.".to_string(),
            session_id: Some("session-1".to_string()),
            token_usage: Some(json!({
                "inputTokens": 12,
                "outputTokens": 8,
                "totalTokens": 20,
            })),
            effective_provider: "codex".to_string(),
            effective_model: "gpt-5.6-sol".to_string(),
            started_at: Utc::now(),
        };

        let transcript = prompt_task_generation_transcript(
            &attempt,
            "Add mobile transcript visibility",
            true,
            None,
        );
        let entries = transcript.as_array().unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["role"], "user");
        assert_eq!(entries[0]["content"], "Add mobile transcript visibility");
        assert_eq!(entries[1]["role"], "assistant");
        assert_eq!(entries[1]["content"], "Created two backlog cards.");
        assert_eq!(entries[1]["provider"], "codex");
        assert_eq!(entries[1]["model"], "gpt-5.6-sol");
        assert_eq!(entries[1]["sessionId"], "session-1");
        assert_eq!(entries[1]["tokenUsage"]["totalTokens"], 20);
    }

    #[test]
    fn clearing_schedule_returns_scheduled_board_to_paused_state() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "scheduledStartAt": "2099-08-09T01:00:00Z"
        }));
        run.status = "scheduled".to_string();
        run.active = false;
        run.loop_started = false;
        run.auto_run_enabled = true;

        clear_board_schedule(&mut run);

        assert_eq!(run.status, "paused");
        assert_eq!(run.scheduled_start_at, None);
        assert!(!run.auto_run_enabled);
        assert_eq!(run.pause_reason.as_deref(), Some("schedule cleared"));
    }

    #[test]
    fn future_scheduled_board_does_not_arm_auto_retry() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "scheduledStartAt": "2099-08-09T01:00:00Z",
            "autoRetry": {
                "enabled": true,
                "delayMinutes": 10,
                "maxAttempts": 3
            }
        }));

        assert_eq!(run.status, "scheduled");
        assert!(!is_resumable_board(&run));
        assert!(!schedule_auto_retry_if_eligible(
            &mut run,
            "resumable status"
        ));
        assert_eq!(run.auto_retry["nextRetryAt"], Value::Null);
    }

    #[test]
    fn board_detail_exposes_every_mobile_evidence_collection() {
        let run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let detail = run.detail_json(None);
        for key in [
            "logs",
            "tasks",
            "sourceManifest",
            "sourceReferences",
            "sourceChunks",
            "codebaseManifest",
            "codebaseMap",
            "codebaseChunks",
            "codebaseUnderstanding",
            "agentsContext",
            "workspaceBaseline",
            "latestWorkspaceSnapshot",
            "environmentState",
            "promptTelemetry",
            "providerUsage",
            "providerUsageBySession",
            "compactionLedger",
            "changeLedger",
            "gitLedger",
            "validationRuns",
            "ragQueries",
            "ragIngestions",
            "ragTraceRefs",
            "tddPolicy",
            "qaArtifacts",
            "promotionCandidates",
            "finalReview",
        ] {
            assert!(detail.get(key).is_some(), "missing evidence key {key}");
        }
    }

    #[test]
    fn configured_fallback_prefers_explicit_next_provider_and_model() {
        let run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "provider": "claude",
            "model": "sonnet",
            "nextProvider": "codex",
            "nextModel": "gpt-5",
            "modelStrategy": {
                "fallbackProvider": "gemini",
                "fallbackModel": "gemini-2.5-pro"
            }
        }));

        assert_eq!(
            configured_provider_fallback(&run),
            Some(("codex".to_string(), "gpt-5".to_string()))
        );
    }

    #[test]
    fn configured_fallback_uses_strategy_and_ignores_same_runtime() {
        let strategy_run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "provider": "claude",
            "model": "sonnet",
            "modelStrategy": {
                "fallbackProvider": "gemini",
                "fallbackModel": "gemini-2.5-pro"
            }
        }));
        let same_run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "provider": "claude",
            "model": "sonnet",
            "nextProvider": "claude",
            "nextModel": "sonnet"
        }));

        assert_eq!(
            configured_provider_fallback(&strategy_run),
            Some(("gemini".to_string(), "gemini-2.5-pro".to_string()))
        );
        assert_eq!(configured_provider_fallback(&same_run), None);
    }

    #[test]
    fn fallback_is_required_for_transport_and_process_failures() {
        let successful = ProviderTaskResult {
            summary: "done".to_string(),
            stderr: String::new(),
            assistant_text: "done".to_string(),
            stream_events: Vec::new(),
            errors: Vec::new(),
            session_id: None,
            token_usage: None,
            exit_code: 0,
        };
        let failed = ProviderTaskResult {
            summary: "provider failed".to_string(),
            stderr: "provider failed".to_string(),
            assistant_text: String::new(),
            stream_events: Vec::new(),
            exit_code: 1,
            errors: vec!["provider failed".to_string()],
            session_id: None,
            token_usage: None,
        };

        assert!(!provider_result_requires_fallback(&Ok(successful)));
        assert!(provider_result_requires_fallback(&Ok(failed)));
        assert!(provider_result_requires_fallback(&Err(bad_request(
            "offline"
        ))));
    }

    #[test]
    fn malformed_tool_call_repair_detects_integer_like_float_schema_errors() {
        assert!(is_repairable_integer_tool_arg_schema_error(
            "failed to parse function arguments: invalid type: floating point `59077.0`, expected i32 at line 1 column 22"
        ));
        assert!(is_repairable_integer_tool_arg_schema_error(
            "failed to parse function arguments: invalid type: floating point `180000.0`, expected u64 at line 1 column 243"
        ));
        assert!(is_repairable_integer_tool_arg_schema_error(
            "failed to parse function arguments: invalid type: floating point `15000.0`, expected usize at line 1 column 29"
        ));
        assert!(!is_repairable_integer_tool_arg_schema_error(
            "failed to parse function arguments: invalid type: floating point `1.5`, expected u64 at line 1 column 20"
        ));
        assert!(!is_repairable_integer_tool_arg_schema_error(
            "failed to parse function arguments: invalid type: string `59077`, expected i32 at line 1 column 22"
        ));
    }

    #[test]
    fn provider_result_detects_malformed_integer_tool_args_in_events() {
        let failed = ProviderTaskResult {
            summary: "provider failed".to_string(),
            stderr: String::new(),
            assistant_text: String::new(),
            stream_events: vec![json!({
                "kind": "tool_result",
                "content": "failed to parse function arguments: invalid type: floating point `60000.0`, expected u64 at line 1 column 43",
            })],
            exit_code: 1,
            errors: Vec::new(),
            session_id: Some("board-chat".to_string()),
            token_usage: None,
        };

        assert!(provider_result_has_repairable_integer_tool_arg_schema_error(&Ok(failed)));
    }

    #[test]
    fn malformed_tool_call_repair_requires_a_failing_provider_result() {
        let recovered = ProviderTaskResult {
            summary: "done".to_string(),
            stderr: String::new(),
            assistant_text: r#"{"status":"done","summary":"Recovered after failed to parse function arguments: invalid type: floating point `60000.0`, expected u64."}"#.to_string(),
            stream_events: Vec::new(),
            errors: Vec::new(),
            session_id: Some("board-chat".to_string()),
            token_usage: None,
            exit_code: 0,
        };

        assert!(provider_result_has_repairable_integer_tool_arg_schema_error(&Ok(recovered)));
        assert!(!provider_result_should_attempt_malformed_tool_call_repair(&Ok(
            ProviderTaskResult {
                summary: "done".to_string(),
                stderr: String::new(),
                assistant_text: r#"{"status":"done","summary":"Recovered after failed to parse function arguments: invalid type: floating point `60000.0`, expected u64."}"#.to_string(),
                stream_events: Vec::new(),
                errors: Vec::new(),
                session_id: Some("board-chat".to_string()),
                token_usage: None,
                exit_code: 0,
            }
        )));
    }

    #[test]
    fn qa_policy_normalizes_malformed_tool_call_repair_settings() {
        let defaults = normalize_qa_policy(None);
        assert_eq!(
            defaults
                .get("repairMalformedToolCalls")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            defaults
                .get("malformedToolCallRepairRetries")
                .and_then(Value::as_u64),
            Some(1)
        );

        let disabled = normalize_qa_policy(Some(&json!({
            "repairMalformedToolCalls": false,
            "malformedToolCallRepairRetries": 99,
        })));
        assert_eq!(
            disabled
                .get("repairMalformedToolCalls")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            disabled
                .get("malformedToolCallRepairRetries")
                .and_then(Value::as_u64),
            Some(MAX_MALFORMED_TOOL_CALL_REPAIR_RETRIES)
        );
    }

    #[test]
    fn legacy_tcd_qa_prompt_maps_to_exact_board_task() {
        let mut run = board_fixture(json!({
            "command": "Implement all written in docs folder",
            "projectPath": "/tmp/TCD-Meida-new"
        }));
        run.id = "28c3b53f-e616-43d9-b4dc-8353fdac7249".to_string();
        run.tasks = ["feature-repair-1", "feature-repair-1-2"]
            .into_iter()
            .map(|id| {
                let mut task =
                    BoardTask::draft(&mut run, "User request".to_string(), id.to_string());
                task.id = id.to_string();
                task
            })
            .collect();
        let prompt = r#"You are the QA phase of a TDD-first io-workbench Kanban board worker.

Project: TCD-Meida-new
Board id: 28c3b53f-e616-43d9-b4dc-8353fdac7249
Task 2: feature-repair-1-2
Title: User request"#;

        assert!(legacy_board_prompt_signature(prompt));
        assert_eq!(
            legacy_board_task_id(&run, prompt).as_deref(),
            Some("feature-repair-1-2")
        );
    }

    #[test]
    fn legacy_internal_prompt_requires_nearby_board_telemetry() {
        let mut run = board_fixture(json!({
            "command": "Implement all written in docs folder",
            "projectPath": "/tmp/TCD-Meida-new"
        }));
        let started_at = Utc::now();
        run.prompt_telemetry = vec![json!({
            "label": "codebase recon",
            "startedAt": started_at,
        })];
        let prompt = "You are performing read-only codebase reconnaissance before Kanban planning.";

        assert!(legacy_board_prompt_signature(prompt));
        assert!(legacy_prompt_matches_board_telemetry(
            &run,
            started_at + chrono::Duration::seconds(20)
        ));
        assert!(!legacy_prompt_matches_board_telemetry(
            &run,
            started_at + chrono::Duration::minutes(2)
        ));
        assert_eq!(legacy_board_task_id(&run, prompt), None);
    }

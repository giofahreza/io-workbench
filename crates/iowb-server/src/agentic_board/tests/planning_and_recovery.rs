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

    #[test]
    fn structured_execution_summary_overrides_transport_json_tail() {
        let parsed = parse_execution_result(
            r#"{"status":"done","summary":"Created STATUS.md from retrieved context."}"#,
        )
        .unwrap();

        assert_eq!(
            resolved_execution_summary(&parsed, "}"),
            "Created STATUS.md from retrieved context."
        );
    }

    #[test]
    fn execution_summary_falls_back_to_transport_summary() {
        let parsed = json!({"status": "done", "summary": "  "});

        assert_eq!(
            resolved_execution_summary(&parsed, "Provider completed successfully"),
            "Provider completed successfully"
        );
    }

    #[test]
    fn agentic_gateway_config_does_not_duplicate_endpoint_inclusive_urls() {
        let mut codex = json!({
            "gatewayUrl": "https://ai.qif.us/codex/",
        });
        agentic_apply_io_gateway_config(&mut codex, Provider::Codex);
        assert_eq!(
            codex.get("baseUrl").and_then(Value::as_str),
            Some("https://ai.qif.us/codex")
        );

        let mut claude = json!({
            "gatewayUrl": "https://ai.qif.us/claude",
        });
        agentic_apply_io_gateway_config(&mut claude, Provider::Claude);
        assert_eq!(
            claude.get("baseUrl").and_then(Value::as_str),
            Some("https://ai.qif.us/claude")
        );
    }

    #[test]
    fn new_board_preserves_every_mobile_configuration_field() {
        let run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "projectName": "project",
            "provider": "gemini",
            "model": "gemini-2.5-pro",
            "nextProvider": "codex",
            "nextModel": "gpt-5",
            "modelStrategy": {"mode": "fallback"},
            "boardProfile": "complete_app",
            "taskModelOverrides": {"qa": "sonnet"},
            "sessionPolicy": "task-model",
            "gitPolicy": "managed",
            "toolsSettings": {"shell": true},
            "qaPolicy": {
                "taskQaMode": "all",
                "maxFollowupsPerGroup": 5,
                "maxTaskAttempts": 4,
                "repairMalformedToolCalls": false,
                "malformedToolCallRepairRetries": 2
            }
        }));

        assert_eq!(run.provider, "gemini");
        assert_eq!(run.model, "gemini-2.5-pro");
        assert_eq!(run.next_provider, "codex");
        assert_eq!(run.next_model, "gpt-5");
        assert_eq!(run.model_strategy, Some(json!({"mode": "fallback"})));
        assert_eq!(run.board_profile, "complete_app");
        assert_eq!(run.task_model_overrides, json!({"qa": "sonnet"}));
        assert_eq!(run.session_policy, "task-model");
        assert_eq!(run.git_policy, "managed");
        assert_eq!(run.tools_settings, Some(json!({"shell": true})));
        assert_eq!(
            run.qa_policy,
            json!({
                "taskQaMode": "all",
                "maxFollowupsPerGroup": 5,
                "maxTaskAttempts": 4,
                "repairMalformedToolCalls": false,
                "malformedToolCallRepairRetries": 2
            })
        );
    }

    #[test]
    fn orchestration_v2_summary_hides_internal_validation_tasks() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let mut validation = run.tasks[0].clone();
        validation.id = "task-final-qa".to_string();
        validation.internal_validation = true;
        validation.status = TASK_STATUS_DONE.to_string();
        run.tasks.push(validation);

        let summary = run.summary_json(None);

        assert_eq!(summary["taskCounts"]["total"], 1);
        assert_eq!(summary["taskCounts"]["done"], 1);
    }

    #[test]
    fn summary_reports_actual_telemetry_validation_and_resumability() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.status = "running".to_string();
        run.auto_run_enabled = true;
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        run.prompt_telemetry = vec![
            json!({
                "phase": "implementation",
                "label": "First",
                "chars": 400,
                "estimatedTokens": 100,
                "actualInputTokens": 60,
                "actualCachedInputTokens": 10,
                "actualOutputTokens": 30,
                "actualTotalTokens": 90,
                "effectiveModel": "gpt-5.6-sol"
            }),
            json!({
                "phase": "qa",
                "label": "Second",
                "chars": 800,
                "estimatedTokens": 200,
                "actualInputTokens": 100,
                "actualOutputTokens": 50,
                "actualTotalTokens": 150
            }),
        ];
        run.validation_runs = vec![
            json!({"stage": "feature", "passed": true, "commands": ["cargo test", "cargo fmt"]}),
            json!({"stage": "final", "passed": true, "command": "cargo check"}),
        ];

        let summary = run.summary_json(None);

        assert_eq!(summary["promptTelemetrySummary"]["actualInputTokens"], 160);
        assert_eq!(
            summary["promptTelemetrySummary"]["actualCachedInputTokens"],
            10
        );
        assert_eq!(summary["promptTelemetrySummary"]["actualOutputTokens"], 80);
        assert_eq!(summary["promptTelemetrySummary"]["actualTokens"], 240);
        assert_eq!(
            summary["promptTelemetrySummary"]["phases"][0]["phase"],
            "qa"
        );
        assert_eq!(
            summary["promptTelemetrySummary"]["largestCall"]["label"],
            "Second"
        );
        assert_eq!(summary["validationSummary"]["latestStage"], "final");
        assert_eq!(summary["validationSummary"]["latestPassed"], true);
        assert_eq!(summary["validationSummary"]["commands"], 3);
        assert_eq!(summary["resumable"], false);

        run.status = "paused".to_string();
        assert_eq!(run.summary_json(None)["resumable"], true);
    }

    #[test]
    fn manual_task_preserves_all_rich_authoring_fields() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let request = serde_json::from_value::<TaskRequest>(json!({
            "title": "Rich task",
            "details": "Implement the feature",
            "priority": TASK_PRIORITY_P1,
            "status": "backlog",
            "acceptanceCriteria": ["Tests pass", "UI renders"],
            "references": ["README.md"],
            "files": ["src/main.rs"],
            "paths": ["tests/main.rs"],
            "dependsOn": ["task-0"]
        }))
        .unwrap();

        let task = BoardTask::manual(&mut run, request).unwrap();

        assert_eq!(task.title, "Rich task");
        assert_eq!(task.details, "Implement the feature");
        assert_eq!(task.priority, TASK_PRIORITY_P1);
        assert_eq!(task.status, "backlog");
        assert_eq!(task.acceptance_criteria, vec!["Tests pass", "UI renders"]);
        assert_eq!(
            task.references,
            vec!["README.md", "src/main.rs", "tests/main.rs"]
        );
        assert_eq!(task.depends_on, vec!["task-0"]);
        assert_eq!(task.task_origin, "user_manual");
    }

    #[test]
    fn legacy_task_origins_normalize_without_changing_system_origins() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let mut prompt_task = BoardTask::draft(
            &mut run,
            "Prompt task".to_string(),
            "Prompt details".to_string(),
        );
        prompt_task.task_origin = "prompt_breakdown".to_string();
        let mut planned_task = prompt_task.clone();
        planned_task.id = "task-3".to_string();
        planned_task.task_origin = "planner".to_string();
        let mut system_task = prompt_task.clone();
        system_task.id = "task-4".to_string();
        system_task.task_origin = "system_followup".to_string();
        run.tasks[0].task_origin = "manual".to_string();
        run.tasks.extend([prompt_task, planned_task, system_task]);

        normalize_board_provenance(&mut run);

        assert_eq!(
            run.tasks
                .iter()
                .map(|task| task.task_origin.as_str())
                .collect::<Vec<_>>(),
            vec![
                "user_manual",
                "user_prompt_generated",
                "planned",
                "system_followup",
            ]
        );
    }

    #[test]
    fn prompt_task_draft_infers_manual_test_kind_for_functional_verification() {
        let mut run = board_fixture(json!({
            "command": "Add budget alerts",
            "projectPath": "/tmp/project"
        }));
        let drafts = sanitize_prompt_task_drafts(
            &json!({
                "tasks": [
                    {
                        "title": "Run mobile functional verification for budget alerts",
                        "details": "Verify the budget alerts workflow locally using the existing Android project commands and a focused manual smoke pass.",
                        "acceptanceCriteria": ["Budget alert workflow is verified on the Android emulator."]
                    }
                ]
            }),
            "Break down budget alerts",
        );

        assert_eq!(drafts[0]["kind"], TASK_KIND_MANUAL_TEST);
        assert_eq!(drafts[0]["taskType"], TASK_KIND_MANUAL_TEST);

        let task = prompt_task_from_draft(&mut run, drafts[0].clone(), "Break down budget alerts");

        assert_eq!(task.task_type, TASK_KIND_MANUAL_TEST);
        assert_eq!(canonical_task_kind(&task), TASK_KIND_MANUAL_TEST);
        assert_eq!(model_type_for_task(&task), "qa");
        assert!(is_kanban_parent_task(&task));
    }

    #[test]
    fn manual_test_task_skips_tdd_and_uses_verification_prompt() {
        let mut run = board_fixture(json!({
            "command": "Add budget alerts",
            "projectPath": "/tmp/project",
            "tddEnabled": true
        }));
        let task = prompt_task_from_draft(
            &mut run,
            json!({
                "title": "Run mobile functional verification for budget alerts",
                "kind": "manual_test",
                "details": "Run a manual Android emulator smoke test for budget alerts.",
                "acceptanceCriteria": ["Manual verification evidence is recorded."]
            }),
            "Break down budget alerts",
        );

        assert!(!task_requires_tdd(&run, &task));

        let prompt = build_task_execution_prompt(&run, &task, 0);

        assert!(prompt.contains("Prompt template: Manual Test"));
        assert!(prompt.contains(
            "Do not edit source, production, generated, test, config, lock, or documentation files"
        ));
        assert!(prompt.contains("\"suggestedBacklogTasks\""));
        assert!(prompt.contains("\"kind\": \"implementation\""));
        assert!(!prompt.contains("Make minimal changes needed to complete this task correctly."));
    }

    #[test]
    fn manual_test_result_requires_an_explicit_success_signal() {
        for result in [
            "not_run",
            "skipped",
            "unknown",
            "blocked by an unavailable emulator",
            "The flow could not be verified.",
            "Pass, but one step failed.",
        ] {
            assert!(
                !manual_test_result_is_successful(result),
                "indeterminate or failed result should not pass: {result}"
            );
        }

        for result in [
            "Pass: the saved amount is shown after reopening the screen.",
            "All steps passed; the workflow behaved as expected.",
            "Verified successfully with no failures.",
            "No issues observed.",
        ] {
            assert!(
                manual_test_result_is_successful(result),
                "explicit success result should pass: {result}"
            );
        }
    }

    #[test]
    fn legacy_verification_cards_migrate_to_manual_test_kind() {
        let mut run = board_fixture(json!({
            "command": "Add budget alerts",
            "projectPath": "/tmp/project"
        }));
        let mut verification = BoardTask::draft(
            &mut run,
            "Run mobile functional verification for budget alerts".to_string(),
            "Verify the budget alerts workflow locally using Android emulator smoke testing."
                .to_string(),
        );
        verification.task_origin = "user_prompt_generated".to_string();
        verification.prompt_task = true;
        verification.task_type = TASK_KIND_IMPLEMENTATION.to_string();
        let mut unit_tests = BoardTask::draft(
            &mut run,
            "Add unit tests for budget alerts and monthly reset".to_string(),
            "Add focused JVM tests for budget validation and monthly reset.".to_string(),
        );
        unit_tests.task_origin = "user_prompt_generated".to_string();
        unit_tests.prompt_task = true;
        unit_tests.task_type = TASK_KIND_IMPLEMENTATION.to_string();
        run.tasks = vec![verification, unit_tests];

        normalize_board_model(&mut run);

        assert_eq!(run.tasks[0].task_type, TASK_KIND_MANUAL_TEST);
        assert_eq!(run.tasks[1].task_type, TASK_KIND_IMPLEMENTATION);
    }

    #[test]
    fn successful_task_adds_deduplicated_optional_backlog_suggestions() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].title = "Existing card".to_string();
        let status_before = run.status.clone();
        let created = append_suggested_backlog_tasks_from_result(
            &mut run,
            "task-1",
            &json!({
                "suggestedBacklogTasks": [
                    {"title": "Add metrics", "details": "Expose queue metrics."},
                    {"title": "  add   metrics  ", "details": "Duplicate."},
                    {"title": "Existing card", "details": "Already exists."}
                ]
            }),
        );

        assert_eq!(created.len(), 1);
        let task = run.tasks.iter().find(|task| task.id == created[0]).unwrap();
        assert_eq!(task.status, "backlog");
        assert_eq!(task.task_origin, "ai_suggested_backlog");
        assert!(!task.manual_task);
        assert!(!task.prompt_task);
        assert!(task.references[0].contains("Suggested backlog task from task-1"));
        assert_eq!(run.status, status_before);
    }

    #[test]
    fn prompt_backlog_generation_reuses_existing_root_task_trees() {
        let mut run = board_fixture(json!({
            "command": "Existing story",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].title = "Existing story".to_string();

        let duplicate = prompt_task_tree_from_draft(
            &mut run,
            json!({
                "title": " existing   story ",
                "level": "story",
                "details": "A regenerated copy of the existing story."
            }),
            "Regenerate the backlog",
        );
        let missing = prompt_task_tree_from_draft(
            &mut run,
            json!({
                "title": "New reporting story",
                "level": "story",
                "details": "A story that is not yet on the board."
            }),
            "Regenerate the backlog",
        );

        let (kept, reused) = keep_missing_prompt_task_trees(&run, vec![duplicate, missing]);

        assert_eq!(reused, 1);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].title, "New reporting story");
        assert_eq!(run.tasks.len(), 1);
    }

    #[test]
    fn generated_system_validation_work_is_executable_subtask_work() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let story_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();

        let mut parent = BoardTask::draft(
            &mut run,
            "Plan budget persistence".to_string(),
            "Plan the budget persistence scope.".to_string(),
        );
        parent.status = TASK_STATUS_TODO.to_string();
        parent.hierarchy.level = TASK_LEVEL_TASK.to_string();
        parent.hierarchy.parent_id = Some(story_id);
        parent.hierarchy.executable = false;
        let parent_id = parent.id.clone();
        run.tasks.push(parent);

        let mut source = BoardTask::draft(
            &mut run,
            "Implement budget persistence".to_string(),
            "Implement the budget persistence repository.".to_string(),
        );
        source.status = TASK_STATUS_DONE.to_string();
        source.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        source.hierarchy.parent_id = Some(parent_id.clone());
        source.hierarchy.executable = true;
        source.hierarchy.scope_version = 4;
        source.hierarchy.rank = 2;
        source.hierarchy.planned_files = vec!["src/budget.rs".to_string()];
        run.tasks.push(source.clone());

        let qa = create_task_qa_task(&run, &source, "Validate the implementation");
        assert_eq!(task_level(&qa), TASK_LEVEL_SUBTASK);
        assert!(task_is_executable(&qa));
        assert_eq!(qa.hierarchy.parent_id.as_deref(), Some(parent_id.as_str()));
        assert!(task_is_runnable_in_board(&run, &qa));

        let agents =
            create_agents_knowledge_task(&run, "Capture durable project knowledge", Some(&source));
        assert_eq!(task_level(&agents), TASK_LEVEL_SUBTASK);
        assert!(task_is_executable(&agents));
        assert_eq!(
            agents.hierarchy.parent_id.as_deref(),
            Some(parent_id.as_str())
        );
        assert!(task_is_runnable_in_board(&run, &agents));

        run.tasks.push(qa.clone());
        assert!(queue_qa_verdict_retry(
            &mut run,
            &qa.id,
            &json!({
                "status": "needs_followup",
                "summary": "The QA response omitted the final JSON verdict.",
                "remainingIssues": ["Return the required verdict JSON."]
            })
        ));
        let retry = run
            .tasks
            .iter()
            .find(|task| task.qa_verdict_retry_task)
            .expect("QA verdict retry");
        assert_eq!(task_level(retry), TASK_LEVEL_SUBTASK);
        assert!(task_is_executable(retry));
        assert_eq!(
            retry.hierarchy.parent_id.as_deref(),
            Some(parent_id.as_str())
        );
        assert!(retry.depends_on.contains(&qa.id));

        assert!(append_final_qa_task(&mut run, "Final board validation"));
        let final_qa = run
            .tasks
            .iter()
            .find(|task| task.final_qa_task)
            .expect("final QA task");
        assert_eq!(task_level(final_qa), TASK_LEVEL_SUBTASK);
        assert!(task_is_executable(final_qa));
        assert!(task_is_runnable_in_board(&run, final_qa));

        run.promotion_candidates = vec![json!({"id": "candidate-1"})];
        assert!(append_promotion_review_task(&mut run, "Review promotion"));
        let promotion = run
            .tasks
            .iter()
            .find(|task| task.id == PROMOTION_REVIEW_TASK_ID)
            .expect("promotion review task");
        assert_eq!(task_level(promotion), TASK_LEVEL_SUBTASK);
        assert!(task_is_executable(promotion));
    }

    #[test]
    fn board_detail_exposes_parent_task_groups_with_dynamic_subtasks() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let source = run.tasks[0].clone();

        assert!(append_task_qa_task(
            &mut run,
            &source,
            "Validate after implementation"
        ));

        let detail = run.detail_json(None);
        let groups = detail["taskGroups"].as_array().unwrap();
        let group = groups
            .iter()
            .find(|group| group["id"] == "task-1")
            .expect("parent task group exists");

        assert_eq!(detail["taskGroupCounts"]["total"], 1);
        assert_eq!(detail["taskGroupCounts"]["in_progress"], 1);
        assert_eq!(group["status"], TASK_STATUS_IN_PROGRESS);
        assert_eq!(group["primaryTaskId"], "task-1");
        assert_eq!(group["currentSubtaskKind"], TASK_KIND_QA);
        assert_eq!(group["subtaskCounts"]["done"], 1);
        assert_eq!(group["subtaskCounts"]["todo"], 1);
        assert_eq!(group["subtasks"].as_array().unwrap().len(), 2);
        assert!(
            group["subtasks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|task| task["groupId"] == "task-1")
        );
    }

    #[test]
    fn legacy_source_linked_subtasks_inherit_parent_group_id() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].group_id = None;
        let mut qa = BoardTask::draft(&mut run, "QA".to_string(), "Validate".to_string());
        qa.id = "task-qa-1".to_string();
        qa.group_id = None;
        qa.qa_task = true;
        qa.source_task_id = Some("task-1".to_string());
        run.tasks.push(qa);

        normalize_board_model(&mut run);

        assert_eq!(run.tasks[0].group_id.as_deref(), Some("task-1"));
        assert_eq!(run.tasks[1].group_id.as_deref(), Some("task-1"));
        assert_eq!(
            run.detail_json(None)["taskGroups"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn legacy_source_parent_is_not_an_approval_ancestor() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let source_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let mut qa = BoardTask::draft(
            &mut run,
            "QA legacy source link".to_string(),
            "Validate the completed source work.".to_string(),
        );
        qa.id = "task-qa-legacy".to_string();
        qa.status = TASK_STATUS_TODO.to_string();
        qa.qa_task = true;
        qa.task_level_qa = true;
        qa.task_type = TASK_KIND_QA.to_string();
        qa.source_task_id = Some(source_id.clone());
        qa.hierarchy.parent_id = Some(source_id);
        qa.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        qa.hierarchy.executable = true;
        run.tasks.push(qa);

        normalize_board_model(&mut run);

        let qa = run
            .tasks
            .iter()
            .find(|task| task.id == "task-qa-legacy")
            .expect("legacy QA task");
        assert!(qa.hierarchy.parent_id.is_none());
        assert!(task_is_runnable_in_board(&run, qa));
    }

    #[test]
    fn nested_manual_items_inherit_their_parent_group_id() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        let parent_group_id = task_group_id_or_self(&run.tasks[0]);
        let mut child = BoardTask::draft(
            &mut run,
            "Nested manual task".to_string(),
            "Keep this planning item inside its parent feature.".to_string(),
        );
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.group_id = Some(child.id.clone());
        run.tasks.push(child);

        normalize_board_task_groups(&mut run);

        assert_eq!(
            run.tasks[1].group_id.as_deref(),
            Some(parent_group_id.as_str())
        );
        assert_eq!(
            run.detail_json(None)["taskGroups"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn nested_children_inherit_parent_priority_unless_overridden() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].priority = TASK_PRIORITY_P1.to_string();

        let inherited_manual = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Persist budget limits",
                "parentId": parent_id,
                "level": "task"
            }))
            .unwrap(),
        )
        .expect("nested manual child");
        assert_eq!(inherited_manual.priority, TASK_PRIORITY_P1);

        let explicit_manual = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Prioritize budget validation",
                "parentId": "task-1",
                "priority": "p0"
            }))
            .unwrap(),
        )
        .expect("nested manual child with override");
        assert_eq!(explicit_manual.priority, TASK_PRIORITY_P0);

        let inherited_planned = task_from_json(
            &run,
            json!({
                "title": "Add budget repository",
                "parentId": "task-1",
                "level": "subtask"
            }),
            1,
            TASK_STATUS_BACKLOG,
        )
        .expect("nested planned child");
        assert_eq!(inherited_planned.priority, TASK_PRIORITY_P1);
    }

    #[test]
    fn discussion_split_children_inherit_parent_priority_and_allow_overrides() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].priority = TASK_PRIORITY_P0.to_string();

        split_discussion_item(
            &mut run,
            &parent_id,
            &json!({
                "items": [
                    {"title": "Persist budget limits"},
                    {"title": "Validate budget input", "priority": "p3"}
                ]
            }),
        )
        .expect("discussion split");

        let inherited = run
            .tasks
            .iter()
            .find(|task| task.title == "Persist budget limits")
            .expect("inherited split child");
        assert_eq!(inherited.priority, TASK_PRIORITY_P0);
        let overridden = run
            .tasks
            .iter()
            .find(|task| task.title == "Validate budget input")
            .expect("overridden split child");
        assert_eq!(overridden.priority, TASK_PRIORITY_P3);
    }

    #[test]
    fn discussion_split_preserves_optional_children() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();

        split_discussion_item(
            &mut run,
            &parent_id,
            &json!({
                "items": [
                    {"title": "Add optional chart", "required": false},
                    {"title": "Persist budget limits"}
                ]
            }),
        )
        .expect("discussion split");

        let optional = run
            .tasks
            .iter()
            .find(|task| task.title == "Add optional chart")
            .expect("optional split child");
        let required = run
            .tasks
            .iter()
            .find(|task| task.title == "Persist budget limits")
            .expect("required split child");
        assert!(!optional.hierarchy.required);
        assert!(required.hierarchy.required);
    }

    #[test]
    fn discussion_split_validates_acceptance_criteria_before_applying_children() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].acceptance_criteria = vec!["Budget amount can be zero.".to_string()];

        let error = split_discussion_item(
            &mut run,
            &parent_id,
            &json!({
                "items": [{
                    "title": "Persist budget limits",
                    "acceptanceCriteria": ["Budget amount must be positive."]
                }]
            }),
        )
        .unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(run.tasks.len(), 1, "invalid split must be atomic");
        assert!(
            run.tasks[0]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("Acceptance criteria conflict")
        );
    }

    #[test]
    fn manual_task_rejects_unknown_status() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let request = serde_json::from_value::<TaskRequest>(json!({
            "title": "Invalid task",
            "status": "banana"
        }))
        .unwrap();

        let error = BoardTask::manual(&mut run, request).unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.body.error.contains("Task status must be one of"));
    }

    #[test]
    fn hierarchical_manual_task_rejects_runtime_statuses() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let request = serde_json::from_value::<TaskRequest>(json!({
            "title": "Invalid runtime task",
            "status": "done"
        }))
        .unwrap();
        let task = BoardTask::manual(&mut run, request).unwrap();

        let error = validate_manual_task_status(&run, &task).unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn nested_manual_todo_requires_approved_parent() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let request = serde_json::from_value::<TaskRequest>(json!({
            "title": "Nested task",
            "parentId": "task-1",
            "status": "todo"
        }))
        .unwrap();
        let task = BoardTask::manual(&mut run, request).unwrap();

        let error = validate_manual_task_status(&run, &task).unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
    }

    #[test]
    fn nested_todo_breakdown_requires_every_ancestor_to_be_approved() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        run.tasks[0].hierarchy.level = TASK_LEVEL_EPIC.to_string();
        run.tasks[0].hierarchy.executable = false;

        let mut parent = BoardTask::draft(
            &mut run,
            "Plan feature delivery".to_string(),
            "Plan the feature delivery tasks.".to_string(),
        );
        parent.status = TASK_STATUS_TODO.to_string();
        parent.hierarchy.level = TASK_LEVEL_STORY.to_string();
        parent.hierarchy.parent_id = Some(root_id);
        parent.hierarchy.executable = false;
        let parent_id = parent.id.clone();
        run.tasks.push(parent);

        let parent = run
            .tasks
            .iter()
            .find(|task| task.id == parent_id)
            .cloned()
            .expect("nested planning parent");
        let error = validate_hierarchy_breakdown_parent(&run, &parent).unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(next_hierarchy_parent(&run).is_none());

        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        assert!(validate_hierarchy_breakdown_parent(&run, &parent).is_ok());
        assert_eq!(next_hierarchy_parent(&run).unwrap().id, parent_id);
    }

    #[test]
    fn hierarchy_breakdown_detects_reworded_duplicate_titles() {
        assert!(suggested_task_titles_are_semantic_duplicates(
            "Add endpoint PATCH /budget/{month}",
            "Implement PATCH budget month endpoint",
        ));
        assert!(suggested_task_titles_are_semantic_duplicates(
            "Run Android emulator smoke test for budget edit flow",
            "Run smoke testing of the budget editing flow on Android emulator",
        ));
        assert!(!suggested_task_titles_are_semantic_duplicates(
            "Add budget repository",
            "Add budget validation",
        ));
    }

    #[test]
    fn hierarchy_breakdown_refinement_has_a_small_total_child_budget() {
        assert_eq!(hierarchy_breakdown_max_new_children(0), 12);
        assert_eq!(hierarchy_breakdown_max_new_children(1), 4);
        assert_eq!(hierarchy_breakdown_max_new_children(8), 4);
        assert_eq!(hierarchy_breakdown_max_new_children(11), 1);
        assert_eq!(hierarchy_breakdown_max_new_children(12), 0);
        assert_eq!(hierarchy_breakdown_max_new_children(20), 0);
    }

    #[test]
    fn hierarchy_breakdown_prompt_switches_to_gap_check_after_children_exist() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].hierarchy.level = TASK_LEVEL_STORY.to_string();
        run.tasks[0].hierarchy.executable = false;

        let mut child = BoardTask::draft(
            &mut run,
            "Persist budget limits".to_string(),
            "Persist the monthly budget limits.".to_string(),
        );
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id.clone());
        child.hierarchy.executable = false;
        run.tasks.push(child);

        let prompt = build_hierarchy_breakdown_prompt(
            &run,
            run.tasks.iter().find(|task| task.id == parent_id).unwrap(),
            TASK_LEVEL_TASK,
        );

        assert!(prompt.contains("Breakdown mode: refinement / gap check"));
        assert!(prompt.contains("Return no more than 4 genuinely new child item(s)."));
        assert!(prompt.contains("return an empty items array"));
        assert!(prompt.contains("persist budget limits"));
    }

    #[test]
    fn hierarchy_breakdown_child_candidates_include_wrapped_proposals_once() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        let mut wrapper = BoardTask::draft(
            &mut run,
            "Proposed scope: Add budget export".to_string(),
            "Review the generated task scope.".to_string(),
        );
        wrapper.task_origin = "hierarchy_backlog_wrapper".to_string();
        mark_generated_scope_wrapper(
            &mut wrapper,
            &run.tasks[0].clone(),
            "add budget export",
            TASK_LEVEL_TASK,
        );
        run.tasks.push(wrapper);

        let titles = hierarchy_breakdown_child_title_candidates(&run, &parent_id, TASK_LEVEL_TASK);

        assert_eq!(titles, vec!["add budget export".to_string()]);
        assert!(hierarchy_breakdown_has_children(
            &run,
            &parent_id,
            TASK_LEVEL_TASK
        ));
    }

    #[test]
    fn acceptance_criteria_conflict_blocks_hierarchy_planning() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].acceptance_criteria = vec!["Budget amount can be zero.".to_string()];
        let mut child = BoardTask::draft(
            &mut run,
            "Persist budget limits".to_string(),
            "Persist the monthly limit.".to_string(),
        );
        child.id = "task-child".to_string();
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.acceptance_criteria = vec!["Budget amount must be positive.".to_string()];
        run.tasks.push(child);

        let issues = hierarchy_validation_issues(&run);

        assert!(issues.iter().any(|issue| {
            issue.contains("Acceptance criteria conflict")
                && issue.contains("task-1")
                && issue.contains("task-child")
        }));
    }

    #[test]
    fn acceptance_criteria_conflict_is_checked_across_all_ancestors() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let story_id = run.tasks[0].id.clone();
        run.tasks[0].acceptance_criteria = vec!["Budget amount can be zero.".to_string()];

        let mut task = BoardTask::draft(
            &mut run,
            "Persist budget limits".to_string(),
            "Persist the monthly limit.".to_string(),
        );
        task.hierarchy.level = TASK_LEVEL_TASK.to_string();
        task.hierarchy.parent_id = Some(story_id);
        task.acceptance_criteria = vec!["Persist the supplied amount.".to_string()];
        let task_id = task.id.clone();
        run.tasks.push(task);

        let mut subtask = BoardTask::draft(
            &mut run,
            "Validate budget amount".to_string(),
            "Validate the amount before persistence.".to_string(),
        );
        subtask.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        subtask.hierarchy.parent_id = Some(task_id);
        subtask.hierarchy.executable = true;
        subtask.acceptance_criteria = vec!["Budget amount must be positive.".to_string()];
        let subtask_id = subtask.id.clone();
        run.tasks.push(subtask);

        let issues = hierarchy_validation_issues(&run);

        assert!(issues.iter().any(|issue| {
            issue.contains("Acceptance criteria conflict")
                && issue.contains("task-1")
                && issue.contains(subtask_id.as_str())
        }));
    }

    #[test]
    fn board_profiles_normalize_to_the_three_supported_journeys() {
        assert_eq!(normalize_board_profile(Some("minimal")), "minimal");
        assert_eq!(normalize_board_profile(Some("strict")), "minimal");
        assert_eq!(normalize_board_profile(Some("complete")), "complete_app");
        assert_eq!(
            normalize_board_profile(Some("product-ready")),
            "product_ready"
        );
        assert_eq!(normalize_board_profile(Some("quality")), "product_ready");
    }

    #[test]
    fn default_session_policy_is_continuous_for_single_model_boards() {
        let _lock = ENV_TEST_LOCK.lock().unwrap();
        let _env = TestEnvGuard::set(vec![
            ("DANGER_CONTINUOUS_SESSION", None),
            ("IO_WORKBENCH_DANGER_CONTINUOUS_SESSION", None),
        ]);
        let run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "model": "sonnet"
        }));

        assert_eq!(run.session_policy, "continuous");
        assert!(!has_mixed_task_models(&run, None));
    }

    #[test]
    fn hybrid_strategy_switches_continuous_session_to_task_model() {
        let _lock = ENV_TEST_LOCK.lock().unwrap();
        let _env = TestEnvGuard::set(vec![
            ("DANGER_CONTINUOUS_SESSION", None),
            ("IO_WORKBENCH_DANGER_CONTINUOUS_SESSION", None),
        ]);
        let run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "modelStrategy": {
                "mode": "hybrid",
                "cheapModel": "gpt-5-mini",
                "expensiveModel": "gpt-5.6-sol"
            }
        }));

        assert_eq!(run.model, "gpt-5.6-sol");
        assert_eq!(run.board_profile, "complete_app");
        assert_eq!(run.session_policy, "task-model");
        assert_eq!(run.task_model_overrides["implementation"], "gpt-5-mini");
        assert_eq!(run.task_model_overrides["qa"], "gpt-5.6-sol");
    }

    #[test]
    fn manual_task_model_overrides_win_over_strategy_presets() {
        let run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "modelStrategy": {
                "mode": "hybrid",
                "cheapModel": "cheap-model",
                "expensiveModel": "expensive-model"
            },
            "taskModelOverrides": {
                "implementation": "manual-implementation",
                "finalQa": "manual-final"
            }
        }));

        assert_eq!(
            run.task_model_overrides["implementation"],
            "manual-implementation"
        );
        assert_eq!(run.task_model_overrides["final_qa"], "manual-final");
        assert_eq!(run.task_model_overrides["qa"], "expensive-model");
    }

    #[test]
    fn breakdown_phase_routes_provider_with_breakdown_model() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project",
            "provider": "claude",
            "model": "minimax-m3",
            "taskModelOverrides": {
                "breakdown": "gpt-5.5",
                "implementation": "minimax-m3",
                "qa": "minimax-m3"
            }
        }));
        run.backlog_breakdown = json!({
            "status": "idle",
            "prompt": "Break down budget alerts",
            "provider": "codex",
            "model": "gpt-5.5",
            "generatedTaskCount": 3
        });

        assert_eq!(
            effective_provider_for_phase(&run, "breakdown").unwrap(),
            "codex"
        );
        assert_eq!(effective_model_for_phase(&run, "breakdown"), "gpt-5.5");
        assert_eq!(effective_provider_for_phase(&run, "qa").unwrap(), "claude");
        assert_eq!(effective_model_for_phase(&run, "qa"), "minimax-m3");
        assert_eq!(
            agentic_execution_model_for_provider(
                &effective_provider_for_phase(&run, "qa").unwrap(),
                &effective_model_for_phase(&run, "qa"),
            ),
            "min:MiniMax-M3"
        );
        assert_eq!(
            agentic_execution_model_for_provider("codex", "minimax-m3"),
            "minimax-m3"
        );
        assert!(agentic_model_uses_minimax_gateway("min:MiniMax-M3"));
        assert!(agentic_model_uses_minimax_gateway("minimax-m3"));
        assert!(!agentic_model_uses_minimax_gateway("cld:claude-sonnet"));
    }

    #[test]
    fn backlog_breakdown_is_board_state_not_a_task() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "boardProfile": "minimal"
        }));

        run.backlog_breakdown = json!({
            "id": "breakdown-1",
            "status": "running",
            "prompt": "Add export filters",
            "model": "gpt-5.6-sol",
            "boardProfile": "product_ready"
        });
        let detail = run.detail_json(None);

        assert_eq!(detail["backlogBreakdown"]["status"], "running");
        assert_eq!(detail["tasks"].as_array().unwrap().len(), 1);
        assert!(
            detail["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|task| { task.get("id").and_then(Value::as_str) != Some("breakdown-1") })
        );
    }

    #[test]
    fn six_stage_model_routes_select_the_expected_override() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "model": "fallback",
            "taskModelOverrides": {
                "breakdown": "breakdown-model",
                "implementation": "implementation-model",
                "qa": "qa-model",
                "qa_fix": "qa-fix-model",
                "agents": "agents-model",
                "final_qa": "final-qa-model"
            }
        }));
        assert_eq!(
            effective_model_for_phase(&run, "planning breakdown"),
            "breakdown-model"
        );

        let mut implementation = BoardTask::draft(
            &mut run,
            "Implementation".to_string(),
            "Build it".to_string(),
        );
        implementation.status = TASK_STATUS_TODO.to_string();
        let mut qa = implementation.clone();
        qa.qa_task = true;
        let mut qa_fix = implementation.clone();
        qa_fix.qa_fix_task = true;
        let mut agents = implementation.clone();
        agents.agents_knowledge_task = true;
        let mut final_qa = implementation.clone();
        final_qa.final_qa_task = true;

        assert_eq!(
            effective_model_for_task(&run, &implementation),
            "implementation-model"
        );
        assert_eq!(effective_model_for_task(&run, &qa), "qa-model");
        assert_eq!(effective_model_for_task(&run, &qa_fix), "qa-fix-model");
        assert_eq!(effective_model_for_task(&run, &agents), "agents-model");
        assert_eq!(effective_model_for_task(&run, &final_qa), "final-qa-model");
    }

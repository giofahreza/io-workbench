    #[test]
    fn terminal_ancestor_rejects_nested_todo_item() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Nested implementation".to_string(),
            "Implement the nested work.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = true;

        let error = validate_manual_task_status(&run, &child).unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(error.body.error.contains("parent"));
    }

    #[test]
    fn optional_child_can_be_approved_after_required_parent_is_done() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();

        let mut child = BoardTask::draft(
            &mut run,
            "Add optional chart".to_string(),
            "Add a nice-to-have chart without changing the completed scope.".to_string(),
        );
        child.status = TASK_STATUS_BACKLOG.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = true;
        child.hierarchy.required = false;
        run.tasks.push(child);

        let child_id = run.tasks[1].id.clone();
        assert!(task_ancestors_are_approved(&run, &run.tasks[1]));
        assert!(!task_is_runnable_in_board(&run, &run.tasks[1]));

        run.tasks[1].status = TASK_STATUS_TODO.to_string();
        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
        assert!(task_is_runnable_in_board(
            &run,
            run.tasks.iter().find(|task| task.id == child_id).unwrap()
        ));
    }

    #[test]
    fn completed_parent_rejects_nested_backlog_item_creation() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let task = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Late nested planning item",
                "parentId": parent_id,
                "status": "backlog"
            }))
            .unwrap(),
        )
        .expect("manual child");

        let error = validate_manual_task_status(&run, &task).unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(error.body.error.contains("completed parent"));
    }

    #[test]
    fn completed_parent_rejects_nested_scope_edits() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Preserved child".to_string(),
            "Keep this child under the completed parent.".to_string(),
        );
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.status = TASK_STATUS_BACKLOG.to_string();
        let child_id = child.id.clone();
        run.tasks.push(child);

        let error = edit_backlog_task(
            &mut run,
            &child_id,
            &UpdateTaskRequest {
                title: Some("Edited child".to_string()),
                ..UpdateTaskRequest::default()
            },
        )
        .unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(error.body.error.contains("completed parent"));
    }

    #[test]
    fn completed_parent_rejects_user_child_detachment() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let child = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Preserve child",
                "parentId": parent_id,
                "status": "backlog"
            }))
            .unwrap(),
        )
        .expect("manual child");
        let child_id = child.id.clone();
        run.tasks.push(child);

        let error = detach_user_created_child(&mut run, &child_id).unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(error.body.error.contains("completed parent"));
    }

    #[test]
    fn parent_rollup_blocks_on_unmet_dependency() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Wait for dependency".to_string(),
            "Run only after the missing dependency is resolved.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = true;
        child.hierarchy.blocked_by = vec!["missing-task".to_string()];
        child.depends_on = child.hierarchy.blocked_by.clone();
        run.tasks.push(child);

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
    }

    #[test]
    fn parent_rollup_blocks_on_unapproved_side_effects() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Run the migration".to_string(),
            "Apply the database migration.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.task_type = TASK_KIND_MIGRATION.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = true;
        child.hierarchy.side_effects = vec!["database schema".to_string()];
        run.tasks.push(child);

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
    }

    #[test]
    fn parent_rollup_waits_for_research_acceptance_before_done() {
        let mut run = board_fixture(json!({
            "command": "Explore feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut research = BoardTask::draft(
            &mut run,
            "Compare approaches".to_string(),
            "Compare the viable approaches.".to_string(),
        );
        research.status = TASK_STATUS_DONE.to_string();
        research.task_type = TASK_KIND_RESEARCH.to_string();
        research.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        research.hierarchy.parent_id = Some(parent_id);
        research.hierarchy.executable = true;
        run.tasks.push(research);

        refresh_hierarchy_rollups(&mut run);
        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);

        run.tasks[1].hierarchy.research_accepted = true;
        refresh_hierarchy_rollups(&mut run);
        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
    }

    #[test]
    fn optional_children_do_not_block_required_parent_and_optional_only_rollups_stay_consistent() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        run.tasks[0].hierarchy.level = TASK_LEVEL_TASK.to_string();
        run.tasks[0].hierarchy.executable = false;

        let mut optional = BoardTask::draft(
            &mut run,
            "Add optional chart".to_string(),
            "Add a nice-to-have chart.".to_string(),
        );
        optional.status = TASK_STATUS_BACKLOG.to_string();
        optional.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        optional.hierarchy.parent_id = Some(parent_id);
        optional.hierarchy.executable = true;
        optional.hierarchy.required = false;
        run.tasks.push(optional);

        // A backlog nice-to-have is not implicit work and does not hold an
        // approved parent open when it is the only remaining child.
        refresh_hierarchy_rollups(&mut run);
        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
        assert!(!hierarchical_work_is_complete(&run));

        // A user must approve the optional executable item while its parent
        // is still an approved planning scope; a completed parent is an
        // immutable boundary.
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        run.tasks[1].status = TASK_STATUS_TODO.to_string();
        refresh_hierarchy_rollups(&mut run);
        assert_eq!(run.tasks[0].status, TASK_STATUS_IN_PROGRESS);
        assert!(!hierarchical_work_is_complete(&run));

        run.tasks[1].status = TASK_STATUS_DONE.to_string();
        refresh_hierarchy_rollups(&mut run);
        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
        assert!(hierarchical_work_is_complete(&run));
    }

    #[test]
    fn optional_children_do_not_block_a_required_parent_from_becoming_done() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        run.tasks[0].hierarchy.level = TASK_LEVEL_TASK.to_string();
        run.tasks[0].hierarchy.executable = false;

        let mut required = BoardTask::draft(
            &mut run,
            "Implement core behavior".to_string(),
            "Implement the required behavior.".to_string(),
        );
        required.status = TASK_STATUS_DONE.to_string();
        required.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        required.hierarchy.parent_id = Some(parent_id.clone());
        required.hierarchy.executable = true;
        required.hierarchy.required = true;

        let mut optional = BoardTask::draft(
            &mut run,
            "Add optional chart".to_string(),
            "Add a nice-to-have chart.".to_string(),
        );
        optional.status = TASK_STATUS_BACKLOG.to_string();
        optional.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        optional.hierarchy.parent_id = Some(parent_id);
        optional.hierarchy.executable = true;
        optional.hierarchy.required = false;
        run.tasks.extend([required, optional]);

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
    }

    #[test]
    fn parent_rollup_blocks_when_ancestor_is_not_approved() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        let mut parent = BoardTask::draft(
            &mut run,
            "Plan feature".to_string(),
            "Plan the feature implementation.".to_string(),
        );
        parent.status = TASK_STATUS_TODO.to_string();
        parent.hierarchy.level = TASK_LEVEL_TASK.to_string();
        parent.hierarchy.parent_id = Some(root_id.clone());
        parent.hierarchy.executable = false;
        let parent_id = parent.id.clone();
        run.tasks.push(parent);
        let mut child = BoardTask::draft(
            &mut run,
            "Implement nested feature".to_string(),
            "Implement the nested feature.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = true;
        run.tasks.push(child);

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[1].status, TASK_STATUS_BLOCKED);
        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
    }

    #[test]
    fn revision_and_replacement_kinds_are_normalized() {
        assert_eq!(
            normalized_task_kind_name("revision"),
            Some(TASK_KIND_REVISION)
        );
        assert_eq!(
            normalized_task_kind_name("replacement"),
            Some(TASK_KIND_REPLACEMENT)
        );
        assert_eq!(
            normalized_task_kind_name("replace"),
            Some(TASK_KIND_REPLACEMENT)
        );
    }

    #[test]
    fn applying_an_explicit_replacement_marks_done_source_superseded() {
        let mut run = board_fixture(json!({
            "command": "Ship budget controls",
            "projectPath": "/tmp/project"
        }));
        let source_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();

        apply_discussion_action(
            &mut run,
            &source_id,
            "replacement",
            &json!({
                "title": "Replace budget controls",
                "details": "Replace the completed budget behavior.",
                "kind": "replacement",
                "supersedeSource": true
            }),
        )
        .expect("replacement should be created");

        let source = run.tasks.iter().find(|task| task.id == source_id).unwrap();
        let replacement = run
            .tasks
            .iter()
            .find(|task| task.task_type == TASK_KIND_REPLACEMENT)
            .expect("replacement item");
        assert_eq!(
            source.superseded_by.as_deref(),
            Some(replacement.id.as_str())
        );
        assert!(
            source
                .references
                .iter()
                .any(|reference| { reference.contains("Superseded by linked replacement") })
        );
        assert_eq!(replacement.status, TASK_STATUS_BACKLOG);
        assert!(!task_is_executable(replacement));
    }

    #[test]
    fn replacement_without_explicit_supersession_keeps_source_history_open() {
        let mut run = board_fixture(json!({
            "command": "Ship budget controls",
            "projectPath": "/tmp/project"
        }));
        let source_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();

        apply_discussion_action(
            &mut run,
            &source_id,
            "replacement",
            &json!({
                "title": "Explore another budget behavior",
                "details": "Plan an alternative without superseding the source.",
                "kind": "replacement"
            }),
        )
        .expect("replacement should be created");

        let source = run.tasks.iter().find(|task| task.id == source_id).unwrap();
        assert_eq!(source.superseded_by, None);
    }

    #[test]
    fn moving_backlog_clears_external_side_effect_approval() {
        let mut task = BoardTask::draft(
            &mut board_fixture(json!({
                "command": "Implement migration",
                "projectPath": "/tmp/project"
            })),
            "Run migration".to_string(),
            "Run the migration.".to_string(),
        );
        task.hierarchy.side_effects_approved = true;
        task.hierarchy.side_effect_approval = Some(json!({"approved": true}));

        clear_backlog_approval(&mut task);

        assert!(!task.hierarchy.side_effects_approved);
        assert_eq!(task.hierarchy.side_effect_approval, None);
        assert!(!task.hierarchy.research_accepted);
        assert_eq!(task.hierarchy.research_acceptance, None);
    }

    #[test]
    fn blocked_parent_rollup_preserves_planning_attention_with_backlog_children() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_BLOCKED.to_string();
        run.tasks[0].error = Some("Planning error: acceptance criteria conflict".to_string());
        let mut child = BoardTask::draft(
            &mut run,
            "Re-plan budget controls".to_string(),
            "Resolve the conflicting scope.".to_string(),
        );
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.status = TASK_STATUS_BACKLOG.to_string();
        run.tasks.push(child);

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
        assert_eq!(
            run.tasks[0].error.as_deref(),
            Some("Planning error: acceptance criteria conflict")
        );
    }

    #[test]
    fn risky_or_optional_generated_children_stay_in_backlog() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let mut risky = BoardTask::draft(
            &mut run,
            "Run database migration".to_string(),
            "Apply the database migration.".to_string(),
        );
        risky.task_type = TASK_KIND_MIGRATION.to_string();
        risky.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        risky.hierarchy.executable = true;
        assert!(generated_child_requires_backlog_approval(&risky));

        let mut optional = BoardTask::draft(
            &mut run,
            "Add optional chart".to_string(),
            "Add a nice-to-have chart.".to_string(),
        );
        optional.hierarchy.required = false;
        assert!(generated_child_requires_backlog_approval(&optional));

        let ordinary = BoardTask::draft(
            &mut run,
            "Add budget repository".to_string(),
            "Add the repository method.".to_string(),
        );
        assert!(!generated_child_requires_backlog_approval(&ordinary));
    }

    #[test]
    fn wrapped_generated_children_satisfy_their_parent_breakdown_level() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent = run.tasks[0].clone();
        let parent_id = parent.id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        run.tasks[0].hierarchy.level = TASK_LEVEL_STORY.to_string();
        run.tasks[0].hierarchy.executable = false;

        let mut wrapper = BoardTask::draft(
            &mut run,
            "Proposed scope: Run database migration".to_string(),
            "Review the generated migration scope.".to_string(),
        );
        wrapper.status = TASK_STATUS_BACKLOG.to_string();
        wrapper.task_origin = "hierarchy_backlog_wrapper".to_string();
        mark_generated_scope_wrapper(
            &mut wrapper,
            &parent,
            "run database migration",
            TASK_LEVEL_TASK,
        );
        run.tasks.push(wrapper);

        assert!(generated_hierarchy_wrapper_exists(
            &run,
            &parent_id,
            TASK_LEVEL_TASK
        ));
        assert!(next_hierarchy_parent(&run).is_none());
    }

    #[test]
    fn accepted_research_creates_only_backlog_planning_items() {
        let mut run = board_fixture(json!({
            "command": "Explore budget reporting",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut parent = BoardTask::draft(
            &mut run,
            "Explore reporting architecture".to_string(),
            "Plan the reporting architecture.".to_string(),
        );
        parent.status = TASK_STATUS_TODO.to_string();
        parent.hierarchy.level = TASK_LEVEL_TASK.to_string();
        parent.hierarchy.parent_id = Some(run.tasks[0].id.clone());
        parent.hierarchy.executable = false;
        let parent_id = parent.id.clone();
        run.tasks.push(parent);
        let mut research = BoardTask::draft(
            &mut run,
            "Compare reporting approaches".to_string(),
            "Compare viable reporting approaches.".to_string(),
        );
        research.status = TASK_STATUS_DONE.to_string();
        research.task_type = TASK_KIND_RESEARCH.to_string();
        research.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        research.hierarchy.parent_id = Some(parent_id);
        research.hierarchy.executable = true;
        research.result = Some(json!({
            "proposedPlanningItems": [{
                "level": "task",
                "kind": "implementation",
                "title": "Build reporting story",
                "details": "Create the approved reporting flow."
            }]
        }));
        let research_id = research.id.clone();
        run.tasks.push(research);

        accept_research_in_board(&mut run, &research_id, "user-1", None, None)
            .expect("research acceptance succeeds");

        let accepted = run
            .tasks
            .iter()
            .find(|task| task.id == research_id)
            .unwrap();
        assert!(accepted.hierarchy.research_accepted);
        let planning = run
            .tasks
            .iter()
            .find(|task| task.task_origin == "research_accepted")
            .expect("accepted planning item");
        assert_eq!(task_level(planning), TASK_LEVEL_STORY);
        assert_eq!(planning.status, TASK_STATUS_BACKLOG);
        assert!(!task_is_executable(planning));
    }

    #[test]
    fn done_item_linked_revision_never_reopens_source() {
        let mut run = board_fixture(json!({
            "command": "Ship budget controls",
            "projectPath": "/tmp/project"
        }));
        let source_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();

        apply_discussion_action(
            &mut run,
            &source_id,
            "revision",
            &json!({
                "title": "Revise budget controls",
                "details": "Support the revised budget behavior.",
                "kind": "revision"
            }),
        )
        .expect("linked revision should be created");

        let source = run.tasks.iter().find(|task| task.id == source_id).unwrap();
        assert_eq!(source.status, TASK_STATUS_DONE);
        let revision = run
            .tasks
            .iter()
            .find(|task| task.task_type == TASK_KIND_REVISION)
            .expect("revision item");
        assert_eq!(revision.status, TASK_STATUS_BACKLOG);
        assert_eq!(task_level(revision), TASK_LEVEL_STORY);
        assert!(!task_is_executable(revision));
        assert!(
            revision
                .references
                .iter()
                .any(|reference| reference.contains(&source_id))
        );
    }

    #[test]
    fn deleting_required_child_marks_parent_with_missing_plan_reason() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        let mut child = BoardTask::draft(
            &mut run,
            "Persist budget control".to_string(),
            "Persist the control.".to_string(),
        );
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id.clone());
        child.hierarchy.required = true;
        let child_id = child.id.clone();
        run.tasks.push(child);
        run.tasks[0].status = TASK_STATUS_TODO.to_string();

        delete_board_task(&mut run, &child_id).expect("backlog child can be deleted");

        let parent = run.tasks.iter().find(|task| task.id == parent_id).unwrap();
        assert_eq!(parent.status, TASK_STATUS_BLOCKED);
        assert!(
            parent
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("Missing required plan")
        );
    }

    #[test]
    fn user_created_child_can_be_detached_as_a_backlog_story() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        let child = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Preserve custom budget rule",
                "parentId": parent_id,
                "status": "backlog"
            }))
            .unwrap(),
        )
        .expect("manual child");
        let child_id = child.id.clone();
        run.tasks.push(child);

        detach_user_created_child(&mut run, &child_id).expect("detach succeeds");

        let detached = run.tasks.iter().find(|task| task.id == child_id).unwrap();
        assert_eq!(task_level(detached), TASK_LEVEL_STORY);
        assert_eq!(detached.hierarchy.parent_id, None);
        assert_eq!(detached.status, TASK_STATUS_BACKLOG);
        assert_eq!(detached.task_origin, "user_manual");
    }

    #[test]
    fn detaching_user_child_preserves_descendant_hierarchy() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        let child = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Preserve custom budget rule",
                "parentId": root_id,
                "status": "backlog"
            }))
            .unwrap(),
        )
        .expect("manual child");
        let child_id = child.id.clone();
        run.tasks.push(child);

        let grandchild = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Keep custom rule implementation",
                "parentId": child_id,
                "status": "backlog"
            }))
            .unwrap(),
        )
        .expect("manual grandchild");
        let grandchild_id = grandchild.id.clone();
        run.tasks.push(grandchild);

        let great_grandchild = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Verify custom rule",
                "parentId": grandchild_id,
                "level": "subtask",
                "status": "backlog"
            }))
            .unwrap(),
        )
        .expect("manual great-grandchild");
        let great_grandchild_id = great_grandchild.id.clone();
        run.tasks.push(great_grandchild);

        detach_user_created_child(&mut run, &child_id).expect("detach succeeds");

        let detached = run.tasks.iter().find(|task| task.id == child_id).unwrap();
        assert_eq!(task_level(detached), TASK_LEVEL_STORY);
        assert_eq!(detached.hierarchy.parent_id, None);
        assert!(!task_is_executable(detached));

        let task = run
            .tasks
            .iter()
            .find(|task| task.id == grandchild_id)
            .unwrap();
        assert_eq!(task_level(task), TASK_LEVEL_TASK);
        assert_eq!(task.hierarchy.parent_id.as_deref(), Some(child_id.as_str()));
        assert!(!task_is_executable(task));

        let subtask = run
            .tasks
            .iter()
            .find(|task| task.id == great_grandchild_id)
            .unwrap();
        assert_eq!(task_level(subtask), TASK_LEVEL_SUBTASK);
        assert_eq!(
            subtask.hierarchy.parent_id.as_deref(),
            Some(grandchild_id.as_str())
        );
        assert!(task_is_executable(subtask));
        assert!(hierarchy_validation_issues(&run).is_empty());
    }

    #[test]
    fn normalization_keeps_optional_work_runnable_under_completed_scope() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();

        let mut optional_task = BoardTask::draft(
            &mut run,
            "Optional budget polish".to_string(),
            "Keep the optional polish separate from the completed scope.".to_string(),
        );
        optional_task.status = TASK_STATUS_DONE.to_string();
        optional_task.hierarchy.level = TASK_LEVEL_TASK.to_string();
        optional_task.hierarchy.parent_id = Some(root_id.clone());
        optional_task.hierarchy.executable = false;
        optional_task.hierarchy.required = false;
        let optional_task_id = optional_task.id.clone();
        run.tasks.push(optional_task);

        let mut optional_subtask = BoardTask::draft(
            &mut run,
            "Run optional budget polish".to_string(),
            "Run the optional polish work.".to_string(),
        );
        optional_subtask.status = TASK_STATUS_TODO.to_string();
        optional_subtask.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        optional_subtask.hierarchy.parent_id = Some(optional_task_id);
        optional_subtask.hierarchy.executable = true;
        optional_subtask.hierarchy.required = false;
        let optional_subtask_id = optional_subtask.id.clone();
        run.tasks.push(optional_subtask);

        normalize_board_hierarchy(&mut run);

        let subtask = run
            .tasks
            .iter()
            .find(|task| task.id == optional_subtask_id)
            .unwrap();
        assert_eq!(subtask.status, TASK_STATUS_TODO);
        assert!(task_ancestors_are_approved(&run, subtask));
        assert!(task_is_runnable_in_board(&run, subtask));
    }

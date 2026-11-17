fn discussion_execution_state(run: &AgenticBoard) -> Value {
    json!({
        "status": run.status,
        "active": run.active,
        "currentTaskId": run.current_task_id,
        "currentTaskTitle": run.current_task_title,
        "currentTaskStatus": run.current_task_status,
        "currentPhase": run.current_phase,
        "providerCallStartedAt": run.provider_call_started_at,
        "providerCallLabel": run.provider_call_label,
        "currentProviderSessionId": run.current_provider_session_id,
        "actualSessionId": run.actual_session_id,
        "sessionId": run.session_id,
        "lastEffectiveModel": run.last_effective_model,
        "taskProviderSessionId": run.tasks[0].provider_session_id,
        "taskTranscriptUpdatedAt": run.tasks[0].transcript_updated_at,
    })
}

async fn discussion_session_isolation_fixture() -> (PathBuf, AppState, AgenticBoard) {
    let (root, state, mut run) = discussion_lifecycle_fixture().await;
    run.status = "running".into();
    run.active = true;
    run.provider = "codex".into();
    run.model = "min:MiniMax-M3".into();
    run.primary_model = run.model.clone();
    run.session_policy = "continuous".into();
    run.current_task_id = Some(run.tasks[0].id.clone());
    run.current_task_title = run.tasks[0].title.clone();
    run.current_task_status = "in_progress".into();
    run.current_phase = Some("execute_tasks".into());
    run.provider_call_started_at = Some(Utc::now());
    run.provider_call_label = Some("execute task-1".into());
    run.current_provider_session_id = Some("session-executing".into());
    run.actual_session_id = Some("session-continuous".into());
    run.session_id = Some("session-original".into());
    run.last_effective_model = Some("execution-model".into());
    run.tasks[0].provider_session_id = Some("session-executing".into());
    run.tasks[0].transcript_updated_at = Some(Utc::now());
    run.prompt_telemetry
        .push(json!({"label":"execution", "sessionId":"session-executing"}));
    save_board(&state, &run).unwrap();
    let run = load_user_board(&state, "discussion-user", &run.id)
        .unwrap()
        .board;
    (root, state, run)
}

#[tokio::test(flavor = "current_thread")]
async fn discussion_session_isolation_preserves_active_execution_and_selected_minimax_route() {
    let (root, state, run) = discussion_session_isolation_fixture().await;
    let before = discussion_execution_state(&run);
    let original_execution_telemetry = run.prompt_telemetry[0].clone();
    let expected_project = run.project_path.clone();
    let before_in_provider = before.clone();
    let output = execute_discussion_prompt_with_provider(
        &state,
        "discussion-user",
        &run.id,
        "discussion proposal for task-1",
        "Change the proposal only, not the app",
        move |state, snapshot, provider, model, prompt, session_id, task_id| async move {
            assert_eq!(provider, "codex");
            assert_eq!(model, "min:MiniMax-M3");
            assert_eq!(snapshot.project_path, expected_project);
            assert_eq!(snapshot.user_id.as_deref(), Some("discussion-user"));
            assert_eq!(prompt, "Change the proposal only, not the app");
            // These are the exact arguments forwarded to the shared provider
            // runtime: no resumption and no execution-task chat linking.
            assert_eq!(session_id, None);
            assert_eq!(task_id, None);
            let current = load_user_board(&state, "discussion-user", &snapshot.id)?.board;
            assert_eq!(discussion_execution_state(&current), before_in_provider);
            Ok(SharedProviderTurnResult {
                session_id: "session-fresh-discussion".into(),
                assistant_text: "{\"proposal\":true}".into(),
                stderr: String::new(),
                token_usage: Some(json!({"inputTokens":10,"outputTokens":3,"totalTokens":13})),
                exit_code: 0,
                summary: "Proposal generated".into(),
            })
        },
    )
    .await
    .unwrap();
    assert_eq!(output, "{\"proposal\":true}");
    let stored = load_user_board(&state, "discussion-user", &run.id)
        .unwrap()
        .board;
    assert_eq!(discussion_execution_state(&stored), before);
    assert_eq!(stored.prompt_telemetry[0], original_execution_telemetry);
    assert_eq!(stored.prompt_telemetry.len(), 2);
    let telemetry = &stored.prompt_telemetry[1];
    assert_eq!(telemetry["phase"], "discussion");
    assert_eq!(telemetry["sessionId"], "session-fresh-discussion");
    assert_eq!(telemetry["effectiveModel"], "min:MiniMax-M3");
    assert_eq!(telemetry["outcome"], "completed");
    assert!(telemetry["completedAt"].is_string());
    assert_eq!(
        stored.provider_usage_by_session["session-fresh-discussion"]["totalTokens"],
        13
    );
    drop(state);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test(flavor = "current_thread")]
async fn discussion_session_isolation_failure_does_not_clear_newer_execution_call() {
    for provider_start_failure in [true, false] {
        let (root, state, run) = discussion_session_isolation_fixture().await;
        let result = execute_discussion_prompt_with_provider(
            &state,
            "discussion-user",
            &run.id,
            "discussion proposal for task-1",
            "Edit future work",
            move |state, snapshot, _, _, _, session_id, task_id| async move {
                assert!(session_id.is_none());
                assert!(task_id.is_none());
                // Simulate the execution loop advancing while discussion runs.
                mutate_stored_board(&state, "discussion-user", &snapshot.id, |run| {
                    run.provider_call_label = Some("next execution call".into());
                    run.current_provider_session_id = Some("session-next-execution".into());
                    run.actual_session_id = Some("session-next-execution".into());
                    run.tasks[0].provider_session_id = Some("session-next-execution".into());
                    run.prompt_telemetry
                        .push(json!({"label":"next execution call"}));
                    Ok(())
                })?;
                if provider_start_failure {
                    Err(bad_request("provider cannot start"))
                } else {
                    Ok(SharedProviderTurnResult {
                        session_id: "failed-discussion".into(),
                        assistant_text: String::new(),
                        stderr: "Provider failed".into(),
                        token_usage: None,
                        exit_code: 1,
                        summary: "Provider failed".into(),
                    })
                }
            },
        )
        .await;
        assert!(result.is_err());
        let stored = load_user_board(&state, "discussion-user", &run.id)
            .unwrap()
            .board;
        assert_eq!(
            stored.provider_call_started_at,
            run.provider_call_started_at
        );
        assert_eq!(
            stored.provider_call_label.as_deref(),
            Some("next execution call")
        );
        assert_eq!(
            stored.current_provider_session_id.as_deref(),
            Some("session-next-execution")
        );
        assert_eq!(
            stored.actual_session_id.as_deref(),
            Some("session-next-execution")
        );
        assert_eq!(
            stored.tasks[0].provider_session_id.as_deref(),
            Some("session-next-execution")
        );
        assert_eq!(stored.current_task_id, run.current_task_id);
        assert_eq!(stored.last_effective_model, run.last_effective_model);
        assert_eq!(stored.status, "running");
        assert!(stored.active);
        assert_eq!(stored.prompt_telemetry[1]["outcome"], "failed");
        assert!(stored.prompt_telemetry[1]["error"].is_string());
        assert!(stored.prompt_telemetry[1]["completedAt"].is_string());
        assert_eq!(
            stored.prompt_telemetry[2],
            json!({"label":"next execution call"})
        );
        drop(state);
        let _ = fs::remove_dir_all(root);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn discussion_session_isolation_success_preserves_newer_execution_and_uses_stable_telemetry_id()
 {
    let (root, state, run) = discussion_session_isolation_fixture().await;
    execute_discussion_prompt_with_provider(
        &state,
        "discussion-user",
        &run.id,
        "discussion proposal for task-1",
        "Edit future work",
        |state, snapshot, _, _, _, session_id, task_id| async move {
            assert!(session_id.is_none());
            assert!(task_id.is_none());
            mutate_stored_board(&state, "discussion-user", &snapshot.id, |run| {
                run.prompt_telemetry.remove(0);
                run.prompt_telemetry.push(json!({"label":"next call"}));
                run.current_provider_session_id = Some("session-newer-execution".into());
                run.last_effective_model = Some("newer-execution-model".into());
                Ok(())
            })?;
            Ok(SharedProviderTurnResult {
                session_id: "session-discussion".into(),
                assistant_text: "proposal".into(),
                stderr: String::new(),
                token_usage: None,
                exit_code: 0,
                summary: "Proposal generated".into(),
            })
        },
    )
    .await
    .unwrap();
    let stored = load_user_board(&state, "discussion-user", &run.id)
        .unwrap()
        .board;
    assert_eq!(
        stored.current_provider_session_id.as_deref(),
        Some("session-newer-execution")
    );
    assert_eq!(
        stored.last_effective_model.as_deref(),
        Some("newer-execution-model")
    );
    assert_eq!(stored.provider_call_label, run.provider_call_label);
    assert_eq!(
        stored.tasks[0].provider_session_id,
        run.tasks[0].provider_session_id
    );
    assert_eq!(
        stored.prompt_telemetry[0]["sessionId"],
        "session-discussion"
    );
    assert_eq!(stored.prompt_telemetry[0]["outcome"], "completed");
    assert_eq!(stored.prompt_telemetry[1], json!({"label":"next call"}));
    drop(state);
    let _ = fs::remove_dir_all(root);
}

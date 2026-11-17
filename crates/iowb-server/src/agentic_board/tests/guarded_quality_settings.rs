    #[test]
    fn guarded_quality_preserves_unrelated_remote_changes() {
        let mut run = board_fixture(json!({"command":"Quality", "projectPath":"/tmp/quality"}));
        run.tdd_policy["maxFixAttempts"] = json!(4);
        run.validation_config["timeoutSeconds"] = json!(240);
        run.qa_policy["repairMalformedToolCalls"] = json!(false);
        apply_guarded_quality_settings(&mut run, &json!({
            "changes":{"validationConfig":{"maxFeatureCommands":3}},
            "expected":{"validationConfig":{"maxFeatureCommands":2}}
        })).unwrap();
        assert_eq!(run.tdd_policy["maxFixAttempts"], 4);
        assert_eq!(run.validation_config["timeoutSeconds"], 240);
        assert_eq!(run.validation_config["maxFeatureCommands"], 3);
        assert_eq!(run.qa_policy["repairMalformedToolCalls"], false);
    }

    #[test]
    fn guarded_quality_conflict_is_atomic_across_sections() {
        let mut run = board_fixture(json!({"command":"Quality", "projectPath":"/tmp/quality"}));
        run.validation_config["maxFeatureCommands"] = json!(4);
        let before = serde_json::to_value(&run).unwrap();
        let error = apply_guarded_quality_settings(&mut run, &json!({
            "changes":{"tddPolicy":{"maxFixAttempts":5},"validationConfig":{"maxFeatureCommands":3}},
            "expected":{"tddPolicy":{"maxFixAttempts":3},"validationConfig":{"maxFeatureCommands":2}}
        })).unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(serde_json::to_value(&run).unwrap(), before);
    }

    #[test]
    fn guarded_quality_rejects_unguarded_and_malformed_requests_without_mutation() {
        let mut run = board_fixture(json!({"command":"Quality", "projectPath":"/tmp/quality"}));
        let before = serde_json::to_value(&run).unwrap();
        let bodies = [
            json!({"changes":{"tddEnabled":false}}),
            json!({"changes":{},"expected":{}}),
            json!({"changes":{"active":true},"expected":{"active":false}}),
            json!({"changes":{"tddEnabled":false},"expected":{"tddEnabled":true},"extra":true}),
            json!({"changes":{"tddPolicy":{"maxFixAttempts":4}},"expected":{"tddPolicy":{}}}),
            json!({"changes":{"qaPolicy":{}},"expected":{"qaPolicy":{}}}),
            json!({"changes":{"tddPolicy":{"maxFixAttempts":21}},"expected":{"tddPolicy":{"maxFixAttempts":3}}}),
            json!({"changes":{"tddEnabled":"false"},"expected":{"tddEnabled":true}}),
            json!({"changes":{"ragSettings":{"scopes":[2]}},"expected":{"ragSettings":{"scopes":[]}}}),
            json!({"changes":{"qaPolicy":{"taskQaMode":"typo"}},"expected":{"qaPolicy":{"taskQaMode":"all"}}}),
            json!({"changes":{"tddPolicy":{"unknown":3}},"expected":{"tddPolicy":{"unknown":3}}}),
            json!({"changes":{"validationConfig":{"timeoutSeconds":null}},"expected":{"validationConfig":{"timeoutSeconds":120}}}),
        ];
        for body in bodies {
            assert_eq!(apply_guarded_quality_settings(&mut run, &body).unwrap_err().status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(serde_json::to_value(&run).unwrap(), before);
        }
    }

    #[test]
    fn guarded_quality_saves_multiple_sections_and_syncs_rag_enabled() {
        let mut run = board_fixture(json!({"command":"Quality", "projectPath":"/tmp/quality"}));
        let old_enabled = run.tdd_enabled;
        let old_rag_enabled = run.rag_settings["enabled"].clone();
        let old_qa_mode = run.qa_policy["taskQaMode"].clone();
        apply_guarded_quality_settings(&mut run, &json!({
            "changes":{"tddEnabled":!old_enabled,"tddPolicy":{"maxFixAttempts":0},"ragSettings":{"enabled":false},"qaPolicy":{"taskQaMode":"off"}},
            "expected":{"tddEnabled":old_enabled,"tddPolicy":{"maxFixAttempts":3},"ragSettings":{"enabled":old_rag_enabled},"qaPolicy":{"taskQaMode":old_qa_mode}}
        })).unwrap();
        assert_eq!(run.tdd_enabled, !old_enabled);
        assert_eq!(run.tdd_policy["maxFixAttempts"], 0);
        assert_eq!(run.rag_enabled, false);
        assert_eq!(run.qa_policy["taskQaMode"], "off");
    }

    #[test]
    fn guarded_quality_arrays_are_guarded_as_whole_values() {
        let mut run = board_fixture(json!({"command":"Quality", "projectPath":"/tmp/quality"}));
        run.validation_config["featureCommands"] = json!(["remote-command"]);
        let before = serde_json::to_value(&run).unwrap();
        let error = apply_guarded_quality_settings(&mut run, &json!({
            "changes":{"validationConfig":{"featureCommands":["local-command"]}},
            "expected":{"validationConfig":{"featureCommands":[]}}
        })).unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(serde_json::to_value(&run).unwrap(), before);
    }

    #[test]
    fn guarded_quality_success_cannot_be_replayed_with_stale_guard() {
        let mut run = board_fixture(json!({"command":"Quality", "projectPath":"/tmp/quality"}));
        let body = json!({"changes":{"tddPolicy":{"maxFixAttempts":4}},"expected":{"tddPolicy":{"maxFixAttempts":3}}});
        apply_guarded_quality_settings(&mut run, &body).unwrap();
        let before = serde_json::to_value(&run).unwrap();
        assert_eq!(apply_guarded_quality_settings(&mut run, &body).unwrap_err().status, StatusCode::CONFLICT);
        assert_eq!(serde_json::to_value(&run).unwrap(), before);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn guarded_quality_handler_rejection_does_not_persist_existing_planning_error() {
        let fixture = BoardReadAccessFixture::new().await;
        let mut run = board_fixture(json!({"command":"Quality", "projectPath":fixture.project.display().to_string()}));
        run.user_id = Some(fixture.user.0.id.clone());
        run.current_phase = Some(PLANNING_ERROR_PHASE.to_string());
        run.phase_details = Some(json!({"kind":"hierarchy", "issue":"existing planning error"}));
        run.tdd_policy["maxFixAttempts"] = json!(4);
        save_board(&fixture.state, &run).unwrap();
        let path = board_file_path(&fixture.state, &run);
        let before = fs::read(&path).unwrap();
        assert!(has_persisted_planning_error(&load_user_board(&fixture.state, &fixture.user.0.id, &run.id).unwrap().board));
        for (body, status) in [
            (json!({"changes":{"tddPolicy":{"maxFixAttempts":5}}, "expected":{"tddPolicy":{"maxFixAttempts":3}}}), StatusCode::CONFLICT),
            (json!({"changes":{"tddPolicy":{"maxFixAttempts":5}}}), StatusCode::BAD_REQUEST),
        ] {
            let error = update_quality_settings(
                State(fixture.state.clone()), Extension(fixture.user.clone()),
                AxumPath(run.id.clone()), Json(body),
            ).await.unwrap_err();
            assert_eq!(error.status, status);
            assert_eq!(axum::response::IntoResponse::into_response(error).status(), status);
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn guarded_quality_handler_persists_success_and_rejects_other_owner() {
        let fixture = BoardReadAccessFixture::new().await;
        let mut run = board_fixture(json!({"command":"Quality", "projectPath":fixture.project.display().to_string()}));
        run.user_id = Some(fixture.user.0.id.clone());
        save_board(&fixture.state, &run).unwrap();
        let request = json!({"changes":{"tddPolicy":{"maxFixAttempts":4}},"expected":{"tddPolicy":{"maxFixAttempts":3}}});
        let mut outsider = fixture.user.clone();
        outsider.0.id = "different-user".to_string();
        let error = update_quality_settings(
            State(fixture.state.clone()), Extension(outsider),
            AxumPath(run.id.clone()), Json(request.clone()),
        ).await.unwrap_err();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(load_user_board(&fixture.state, &fixture.user.0.id, &run.id).unwrap().board.tdd_policy["maxFixAttempts"], 3);
        let Json(response) = update_quality_settings(
            State(fixture.state.clone()), Extension(fixture.user.clone()),
            AxumPath(run.id.clone()), Json(request),
        ).await.unwrap();
        assert_eq!(response["success"], true);
        assert_eq!(response["board"]["tddPolicy"]["maxFixAttempts"], 4);
        assert_eq!(load_user_board(&fixture.state, &fixture.user.0.id, &run.id).unwrap().board.tdd_policy["maxFixAttempts"], 4);
    }

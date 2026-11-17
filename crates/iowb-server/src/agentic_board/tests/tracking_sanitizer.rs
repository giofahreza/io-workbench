#[test]
fn tracking_sanitizer_preserves_ordinary_scope_prose_arrays_and_paths() {
    let value = json!({
        "description": "Test structure requirements: assert the API inside every named test.",
        "acceptanceCriteria": [
            "Preserve all behavior requirements and execute every test.",
            "A req-header string is rendered as text.",
            "This note discusses legacy REQ-1 without being a tracking reference."
        ],
        "plannedFiles": ["requirements.txt", "test/req-parser.test.js"],
        "filesByPath": {"requirements.txt": {"hash": "example"}},
        "requirementSummary": "An ordinary user-authored field is not retired metadata.",
        "required": true
    });
    assert_eq!(sanitize_kanban_structure(&value), value);
    assert_eq!(sanitize_kanban_value(&value), value);
}

#[test]
fn tracking_sanitizer_keeps_unmodified_embedded_json_bytes() {
    let text = "{ \"payload\": { \"description\": \"Test structure requirements:\", \"acceptanceCriteria\": [\"Every requirement remains covered\"] } }";
    let value = json!({"content": text});
    assert_eq!(sanitize_kanban_value(&value), value);
}

#[test]
fn tracking_sanitizer_removes_known_retired_fields_recursively() {
    let fields = [
        "requirementMatrix", "requirement_matrix", "requirementBaseline",
        "requirement_baseline", "requirementIds", "requirement_ids",
        "requirementCounts", "requirementExtraction", "requirementUpdates",
        "coveredRequirements", "requirement", "requirements", "requirementId",
    ];
    for field in fields {
        let mut retired = serde_json::Map::new();
        retired.insert(field.to_string(), json!(["REQ-0001"]));
        retired.insert("description".to_string(), json!("Preserve test requirements."));
        let value = json!({"nested": Value::Object(retired)});
        assert_eq!(
            sanitize_kanban_structure(&value),
            json!({"nested": {"description": "Preserve test requirements."}}),
            "retired field {field}"
        );
    }
}

#[test]
fn tracking_sanitizer_removes_only_whole_legacy_reference_ids() {
    let ids = [
        "REQ-1", " req-0004 ", "REQ-MANUAL-2",
        "REQ-MANUAL-550e8400-e29b-41d4-a716-446655440000",
    ];
    for id in ids {
        assert_eq!(sanitize_kanban_structure(&json!(id)), json!("[legacy tracking removed]"));
        assert_eq!(sanitize_kanban_structure(&json!([id, "src/main.rs"])), json!(["src/main.rs"]));
    }
    for text in ["requirement", "REQ-parser", "REQ-MANUAL-not-an-id", "REQ-1.js", "See REQ-1 for context"] {
        assert_eq!(sanitize_kanban_structure(&json!(text)), json!(text));
    }
}

#[test]
fn tracking_sanitizer_strips_legacy_metadata_in_embedded_json_without_losing_scope() {
    let content = json!({
        "requirementIds": ["REQ-0001"],
        "references": ["REQ-MANUAL-2", "requirements.txt"],
        "description": "Preserve the requirements of every named test."
    }).to_string();
    let cleaned = sanitize_kanban_value(&json!({"content": content}));
    let parsed: Value = serde_json::from_str(cleaned["content"].as_str().unwrap()).unwrap();
    assert_eq!(parsed, json!({
        "references": ["requirements.txt"],
        "description": "Preserve the requirements of every named test."
    }));
}

#[test]
fn tracking_sanitizer_preserves_discussion_scope_through_board_roundtrip() {
    let mut run = board_fixture(json!({"command": "Build a shopping list", "projectPath": "/tmp/project"}));
    let proposal = json!({
        "proposalId": "scope-regression", "taskId": "task-1", "action": "edit", "status": "pending",
        "payload": {
            "description": "Test structure requirements:\nEvery API assertion belongs inside a named callback.",
            "acceptanceCriteria": ["All shopping requirements retain complete test coverage."],
            "plannedFiles": ["test/req-parser.test.js"]
        }
    });
    run.discussion_proposals = vec![proposal.clone()];
    normalize_board_model(&mut run);
    assert_eq!(run.discussion_proposals[0], proposal);
    let serialized = serde_json::to_string(&run).unwrap();
    let mut restored: AgenticBoard = serde_json::from_str(&serialized).unwrap();
    normalize_board_model(&mut restored);
    assert_eq!(restored.discussion_proposals[0], proposal);
    assert_eq!(restored.detail_json(None)["discussionProposals"][0], proposal);
}

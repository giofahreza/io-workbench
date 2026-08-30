    use super::{
        AUTO_SESSION_TITLE_MAX_CHARS, ForkSessionRequest, SessionLifetimeTokenUsage, SessionMode,
        SessionSpentTokenUsage, SessionSummary, WsClientCommand, session_title_from_prompt,
    };
    use chrono::{DateTime, Utc};
    use serde_json::json;

    #[test]
    fn start_session_fast_round_trips_and_defaults_to_unspecified() {
        let enabled: WsClientCommand = serde_json::from_value(json!({
            "type": "start_session",
            "provider": "codex",
            "projectPath": "/tmp/project",
            "prompt": "ship it",
            "fast": true
        }))
        .expect("deserialize fast start session");
        match enabled {
            WsClientCommand::StartSession { fast, .. } => assert_eq!(fast, Some(true)),
            _ => panic!("expected start_session"),
        }

        let legacy: WsClientCommand = serde_json::from_value(json!({
            "type": "start_session",
            "provider": "codex",
            "projectPath": "/tmp/project",
            "prompt": "ship it"
        }))
        .expect("deserialize legacy start session");
        match legacy {
            WsClientCommand::StartSession { fast, .. } => assert_eq!(fast, None),
            _ => panic!("expected start_session"),
        }
    }

    #[test]
    fn subscribe_board_session_scope_uses_camel_case_and_defaults_empty() {
        let scoped: WsClientCommand = serde_json::from_value(json!({
            "type": "subscribe",
            "topics": ["sessions"],
            "sessionIds": ["board-session"]
        }))
        .expect("deserialize scoped subscription");
        match scoped {
            WsClientCommand::Subscribe {
                topics,
                session_ids,
                chat_session_ids,
            } => {
                assert_eq!(topics, ["sessions"]);
                assert_eq!(session_ids, ["board-session"]);
                assert_eq!(chat_session_ids, None);
            }
            _ => panic!("expected subscribe"),
        }

        let ordinary: WsClientCommand = serde_json::from_value(json!({
            "type": "subscribe",
            "topics": ["sessions"]
        }))
        .expect("deserialize ordinary subscription");
        match ordinary {
            WsClientCommand::Subscribe {
                session_ids,
                chat_session_ids,
                ..
            } => {
                assert!(session_ids.is_empty());
                assert_eq!(chat_session_ids, None);
            }
            _ => panic!("expected subscribe"),
        }

        let mobile_scoped: WsClientCommand = serde_json::from_value(json!({
            "type": "subscribe",
            "topics": ["sessions"],
            "chatSessionIds": ["ordinary-session"]
        }))
        .expect("deserialize mobile-scoped subscription");
        match mobile_scoped {
            WsClientCommand::Subscribe {
                chat_session_ids, ..
            } => assert_eq!(chat_session_ids, Some(vec!["ordinary-session".to_string()])),
            _ => panic!("expected subscribe"),
        }

        let mobile_hidden: WsClientCommand = serde_json::from_value(json!({
            "type": "subscribe",
            "topics": ["sessions"],
            "chatSessionIds": []
        }))
        .expect("deserialize hidden mobile subscription");
        match mobile_hidden {
            WsClientCommand::Subscribe {
                chat_session_ids, ..
            } => assert_eq!(chat_session_ids, Some(Vec::new())),
            _ => panic!("expected subscribe"),
        }
    }

    #[test]
    fn session_summary_serializes_explicit_fast_state() {
        let enabled = SessionSummary {
            fast: Some(true),
            ..Default::default()
        };
        let disabled = SessionSummary {
            fast: Some(false),
            ..Default::default()
        };
        let unspecified = SessionSummary::default();

        assert_eq!(
            serde_json::to_value(enabled).expect("serialize enabled")["fast"],
            true
        );
        assert_eq!(
            serde_json::to_value(disabled).expect("serialize disabled")["fast"],
            false
        );
        assert!(
            serde_json::to_value(unspecified)
                .expect("serialize unspecified")
                .get("fast")
                .is_none()
        );
    }

    #[test]
    fn board_session_summary_uses_camel_case_scope_fields() {
        let summary = SessionSummary {
            board_session: true,
            board_id: Some("board-1".to_string()),
            board_task_id: Some("task-1".to_string()),
            native_session_id: Some("native-1".to_string()),
            ..Default::default()
        };
        let value = serde_json::to_value(summary).expect("serialize board session");
        assert_eq!(value["boardSession"], true);
        assert_eq!(value["boardId"], "board-1");
        assert_eq!(value["boardTaskId"], "task-1");
        assert_eq!(value["nativeSessionId"], "native-1");
    }

    #[test]
    fn board_session_summary_recognizes_legacy_and_partial_scope_metadata() {
        let legacy: SessionSummary = serde_json::from_value(serde_json::json!({
            "id": "legacy-board-session",
            "boardRunId": "board-legacy"
        }))
        .expect("deserialize legacy board scope");
        assert_eq!(legacy.board_id.as_deref(), Some("board-legacy"));
        assert!(legacy.is_board_session());

        let board_id_only = SessionSummary {
            board_id: Some("board-1".to_string()),
            ..Default::default()
        };
        let task_id_only = SessionSummary {
            board_task_id: Some("task-1".to_string()),
            ..Default::default()
        };
        let ordinary = SessionSummary::default();

        assert!(board_id_only.is_board_session());
        assert!(task_id_only.is_board_session());
        assert!(!ordinary.is_board_session());
    }

    #[test]
    fn session_summary_serializes_spent_token_usage_scope_fields() {
        let summary = SessionSummary {
            spent_token_usage: Some(SessionSpentTokenUsage {
                whole_session: SessionLifetimeTokenUsage {
                    total: 1_000,
                    input: 700,
                    output: 300,
                    ..Default::default()
                },
                since_compact: Some(SessionLifetimeTokenUsage {
                    total: 250,
                    input: 175,
                    output: 75,
                    ..Default::default()
                }),
                compacted_at: Some(
                    DateTime::parse_from_rfc3339("2026-08-16T00:00:00Z")
                        .expect("timestamp")
                        .with_timezone(&Utc),
                ),
            }),
            ..Default::default()
        };
        let value = serde_json::to_value(summary).expect("serialize session");
        assert_eq!(value["spentTokenUsage"]["wholeSession"]["total"], 1_000);
        assert_eq!(value["spentTokenUsage"]["sinceCompact"]["total"], 250);
        assert_eq!(
            value["spentTokenUsage"]["compactedAt"],
            "2026-08-16T00:00:00Z"
        );
    }

    #[test]
    fn fork_session_request_defaults_to_non_replacing_and_accepts_edited_draft() {
        let legacy: ForkSessionRequest = serde_json::from_value(json!({
            "beforeMessageId": "message-1",
            "requestId": "request-1"
        }))
        .expect("legacy fork request");
        assert!(!legacy.replace);
        assert_eq!(legacy.draft_content, None);

        let replacement: ForkSessionRequest = serde_json::from_value(json!({
            "beforeMessageId": "message-1",
            "requestId": "request-2",
            "replace": true,
            "draftContent": "edited prompt"
        }))
        .expect("replacement fork request");
        assert!(replacement.replace);
        assert_eq!(replacement.draft_content.as_deref(), Some("edited prompt"));
    }

    #[test]
    fn session_mode_parse_accepts_bypass_permissions_alias() {
        assert_eq!(
            SessionMode::parse(Some("bypass-permissions")),
            SessionMode::Bypass
        );
        assert_eq!(
            SessionMode::parse(Some("bypassPermissions")),
            SessionMode::Bypass
        );
    }

    #[test]
    fn session_title_normalizes_multiline_unicode_prompt() {
        assert_eq!(
            session_title_from_prompt("  Build a café page\n\nwith responsive cards  "),
            Some("Build a café page with responsive cards".to_string())
        );
    }

    #[test]
    fn session_title_truncates_by_unicode_characters() {
        let prompt = "界".repeat(110);
        assert_eq!(
            session_title_from_prompt(&prompt),
            Some(format!("{}...", "界".repeat(AUTO_SESSION_TITLE_MAX_CHARS)))
        );
    }

    #[test]
    fn session_title_preserves_two_line_display_budget() {
        let prompt = "Design a compact mobile chat session list title that uses the full two line area before truncating";
        assert_eq!(session_title_from_prompt(prompt), Some(prompt.to_string()));
    }

    #[test]
    fn session_title_replaces_inline_image_payload() {
        assert_eq!(
            session_title_from_prompt("![diagram.png](data:image/png;base64,QUJDRA==)"),
            Some("Attached image: diagram.png".to_string())
        );
        assert_eq!(
            session_title_from_prompt("Review this\n\n![screen](data:image/png;base64,QUJDRA==)"),
            Some("Review this Attached image: screen".to_string())
        );
    }

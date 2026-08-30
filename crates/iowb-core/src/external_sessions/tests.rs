    use super::*;
    use std::io::Write as _;
    use uuid::Uuid;

    #[test]
    fn visible_user_text_strips_codex_context_wrappers() {
        let hidden = concat!(
            "<recommended_plugins>\nplugins\n</recommended_plugins>\n",
            "<environment_context>\ncontext\n</environment_context>\n"
        );
        assert_eq!(visible_user_text(hidden), "");
        assert_eq!(
            visible_user_text(&format!("{hidden}\nActual prompt")),
            "Actual prompt"
        );
    }

    #[test]
    fn discovers_and_loads_all_supported_cli_histories() {
        let root = std::env::temp_dir().join(format!("iowb-external-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();

        let claude_id = "11111111-1111-4111-8111-111111111111";
        let claude_file = root
            .join(".claude/projects/test")
            .join(format!("{claude_id}.jsonl"));
        write_jsonl(
            &claude_file,
            &[
                json!({"type":"user","sessionId":claude_id,"cwd":project,"timestamp":"2026-07-29T10:00:00Z","message":{"role":"user","content":"Claude question"}}),
                json!({"type":"assistant","sessionId":claude_id,"cwd":project,"timestamp":"2026-07-29T10:00:01Z","message":{"role":"assistant","model":"claude-test","content":[{"type":"text","text":"Claude answer"}]}}),
            ],
        );

        let codex_id = "22222222-2222-4222-8222-222222222222";
        let codex_file = root
            .join(".codex/sessions/2026/07/29")
            .join(format!("rollout-2026-07-29T10-00-00-{codex_id}.jsonl"));
        write_jsonl(
            &codex_file,
            &[
                json!({"timestamp":"2026-07-29T10:01:00Z","type":"session_meta","payload":{"id":codex_id,"cwd":project}}),
                json!({"timestamp":"2026-07-29T10:01:01Z","type":"event_msg","payload":{"type":"user_message","message":"Codex question","kind":"plain"}}),
                json!({"timestamp":"2026-07-29T10:01:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Codex answer"}]}}),
            ],
        );

        let gemini_id = "33333333-3333-4333-8333-333333333333";
        let gemini_root = root.join(".gemini/tmp/project-hash");
        fs::create_dir_all(gemini_root.join("chats")).unwrap();
        fs::write(
            gemini_root.join(".project_root"),
            project.to_string_lossy().as_bytes(),
        )
        .unwrap();
        fs::write(
            gemini_root.join("chats").join(format!("{gemini_id}.json")),
            serde_json::to_vec(&json!({
                "sessionId": gemini_id,
                "lastUpdated": "2026-07-29T10:02:02Z",
                "messages": [
                    {"type":"user","timestamp":"2026-07-29T10:02:01Z","content":"Gemini question"},
                    {"type":"gemini","timestamp":"2026-07-29T10:02:02Z","content":[{"text":"Gemini answer"}]}
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let records = discover_external_sessions(&root);
        assert_eq!(records.len(), 3, "{records:#?}");
        for (provider, session_id, expected_question, expected_answer) in [
            (
                Provider::Claude,
                claude_id,
                "Claude question",
                "Claude answer",
            ),
            (Provider::Codex, codex_id, "Codex question", "Codex answer"),
            (
                Provider::Gemini,
                gemini_id,
                "Gemini question",
                "Gemini answer",
            ),
        ] {
            let record = records
                .iter()
                .find(|record| {
                    record.summary.provider == provider && record.summary.id == session_id
                })
                .unwrap();
            assert!(record.summary.external);
            assert!(same_project_path(
                &record.summary.project_path,
                project.to_str().unwrap()
            ));
            let messages = load_external_messages(record);
            assert_eq!(messages.len(), 2, "{messages:#?}");
            assert_eq!(
                record.summary.message_count,
                messages.len(),
                "summary count must match visible messages for {provider:?}",
            );
            assert_eq!(messages[0].content, expected_question);
            assert_eq!(messages[1].content, expected_answer);
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_claude_index_resumes_from_last_complete_jsonl_offset() {
        let root =
            std::env::temp_dir().join(format!("iowb-external-incremental-{}", Uuid::new_v4()));
        let project = root.join("project");
        let session_id = "44444444-4444-4444-8444-444444444444";
        let history = root
            .join(".claude/projects/test")
            .join(format!("{session_id}.jsonl"));
        fs::create_dir_all(history.parent().expect("history parent")).expect("history directory");
        fs::create_dir_all(&project).expect("project directory");
        write_jsonl(
            &history,
            &[
                json!({"type":"user","sessionId":session_id,"cwd":project,"timestamp":"2026-08-14T01:00:00Z","message":{"role":"user","content":"first"}}),
                json!({"type":"assistant","sessionId":session_id,"cwd":project,"timestamp":"2026-08-14T01:00:01Z","message":{"role":"assistant","content":"second"}}),
            ],
        );
        let database = root.join("index.db");
        let storage = Storage::open(&database).expect("storage");
        let initial = sync_external_sessions(&root, &storage).expect("initial sync");
        let initial = initial
            .iter()
            .find(|record| record.summary.id == session_id)
            .expect("initial record");
        assert_eq!(initial.summary.message_count, 2);
        drop(storage);

        let partial = format!(
            "{{\"type\":\"assistant\",\"sessionId\":\"{session_id}\",\"cwd\":\"{}\",",
            project.display()
        );
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&history)
            .expect("append partial");
        file.write_all(partial.as_bytes()).expect("partial line");
        drop(file);
        let storage = Storage::open(&database).expect("reopened storage");
        let partial_sync = sync_external_sessions(&root, &storage).expect("partial sync");
        assert_eq!(
            partial_sync
                .iter()
                .find(|record| record.summary.id == session_id)
                .expect("partial record")
                .summary
                .message_count,
            2,
        );

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&history)
            .expect("complete partial");
        file.write_all(
            b"\"timestamp\":\"2026-08-14T01:00:02Z\",\"message\":{\"role\":\"assistant\",\"content\":\"third\"}}\n",
        )
        .expect("complete line");
        drop(file);
        let completed = sync_external_sessions(&root, &storage).expect("completed sync");
        assert_eq!(
            completed
                .iter()
                .find(|record| record.summary.id == session_id)
                .expect("completed record")
                .summary
                .message_count,
            3,
        );

        drop(storage);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn ignores_internal_and_malformed_history_rows() {
        let root = std::env::temp_dir().join(format!("iowb-external-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let session_id = "44444444-4444-4444-8444-444444444444";
        let file = root
            .join(".codex/sessions")
            .join(format!("rollout-{session_id}.jsonl"));
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(
            &file,
            format!(
                "not-json\n{}\n{}\n",
                json!({"timestamp":"2026-07-29T10:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-07-29T10:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"<environment_context>internal</environment_context>","kind":"plain"}}),
            ),
        )
        .unwrap();

        assert!(discover_external_sessions(&root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_codex_reasoning_tools_and_patch_file_operations() {
        let root = std::env::temp_dir().join(format!("iowb-external-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let session_id = "44444444-4444-4444-8444-444444444444";
        let file = root
            .join(".codex/sessions/2026/07/30")
            .join(format!("rollout-2026-07-30T00-00-00-{session_id}.jsonl"));
        write_jsonl(
            &file,
            &[
                json!({"timestamp":"2026-07-30T00:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-07-30T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Change files","kind":"plain"}}),
                json!({"timestamp":"2026-07-30T00:00:02Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"Inspecting the project"}]}}),
                json!({"timestamp":"2026-07-30T00:00:03Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"call-exec","arguments":"{\"cmd\":\"pwd\"}"}}),
                json!({"timestamp":"2026-07-30T00:00:04Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call-exec","output":"Chunk ID: one\nProcess exited with code 0"}}),
                json!({"timestamp":"2026-07-30T00:00:05Z","type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","call_id":"call-patch","input":"*** Begin Patch\n*** Add File: created.txt\n+created\n*** Update File: updated.txt\n-old\n+new\n*** Delete File: deleted.txt\n*** Move to: moved.txt\n*** End Patch"}}),
                json!({"timestamp":"2026-07-30T00:00:06Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call-patch","output":"Success"}}),
                json!({"timestamp":"2026-07-30T00:00:07Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Finished"}]}}),
            ],
        );

        let record = discover_external_sessions(&root)
            .into_iter()
            .find(|record| record.summary.id == session_id)
            .unwrap();
        let messages = load_external_messages(&record);

        assert_eq!(7, messages.len(), "{messages:#?}");
        assert_eq!(7, record.summary.message_count);
        assert_eq!(MessageRole::Assistant, messages[1].role);
        assert!(messages[1].content.starts_with("thinking\n"));
        assert_eq!(MessageRole::Tool, messages[2].role);
        assert!(messages[2].content.contains("### Command"));
        assert_eq!(messages[2].metadata["toolName"], "exec_command");
        assert_eq!(MessageRole::Tool, messages[4].role);
        assert!(messages[4].content.contains("apply_patch"));
        assert!(messages[4].content.contains("created.txt"));
        assert!(messages[4].content.contains("updated.txt"));
        assert!(messages[4].content.contains("deleted.txt"));
        assert!(messages[4].content.contains("moved.txt"));
        assert_eq!(
            messages[4].metadata["fileOperations"]
                .as_array()
                .map(Vec::len),
            Some(4),
        );
        assert_eq!("Finished", messages[6].content);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_codex_task_failure_after_reasoning_as_terminal_assistant_message() {
        let root = std::env::temp_dir().join(format!("iowb-external-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let session_id = "88888888-8888-4888-8888-888888888888";
        let file = root
            .join(".codex/sessions/2026/08/11")
            .join(format!("rollout-2026-08-11T08-00-00-{session_id}.jsonl"));
        write_jsonl(
            &file,
            &[
                json!({"timestamp":"2026-08-11T08:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-08-11T08:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Run the full audit","kind":"plain"}}),
                json!({"timestamp":"2026-08-11T08:00:02Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"Inspecting tenant isolation"}]}}),
                json!({"timestamp":"2026-08-11T08:00:03Z","type":"event_msg","payload":{"type":"task_complete","last_agent_message":null,"error":{"message":"{\"detail\":\"The 'gpt-5.6-sol' model is not supported when using Codex with a ChatGPT account.\"}","codex_error_info":"other"}}}),
            ],
        );

        let record = discover_external_sessions(&root)
            .into_iter()
            .find(|record| record.summary.id == session_id)
            .unwrap();
        let messages = load_external_messages(&record);
        let terminal = messages.last().expect("terminal failure message");

        assert_eq!(3, messages.len(), "{messages:#?}");
        assert_eq!(MessageRole::Assistant, terminal.role);
        assert_eq!(terminal.metadata["kind"], "terminal_status");
        assert_eq!(terminal.metadata["status"], "failed");
        assert_eq!(
            terminal.metadata["errorDetail"],
            "The 'gpt-5.6-sol' model is not supported when using Codex with a ChatGPT account.",
        );
        assert!(terminal.content.starts_with("ERROR: The 'gpt-5.6-sol'"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hides_legacy_workbench_transcript_when_native_final_exists() {
        let root = std::env::temp_dir().join(format!("iowb-external-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let session_id = "99999999-9999-4999-8999-999999999999";
        let file = root
            .join(".codex/sessions/2026/08/11")
            .join(format!("rollout-2026-08-11T09-00-00-{session_id}.jsonl"));
        let transcript = format!(
            "thinking\n{}\n\nexec / Parameters\n**Tool:** `command_execution`\n\ncodex\nOnly the native final should remain.\n\ntokens used\n{{\"output_tokens\":12}}",
            "x".repeat(103_000)
        );
        write_jsonl(
            &file,
            &[
                json!({"timestamp":"2026-08-11T09:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-08-11T09:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Explain the fix","kind":"plain"}}),
                json!({"timestamp":"2026-08-11T09:00:02Z","type":"response_item","payload":{"type":"message","id":"msg-native","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"Only the native final should remain."}]}}),
                json!({"timestamp":"2026-08-11T09:00:03Z","type":"response_item","payload":{"type":"message","id":"msg-workbench","role":"assistant","source":"io-workbench","content":[{"type":"output_text","text":transcript}]}}),
            ],
        );

        let record = discover_external_sessions(&root)
            .into_iter()
            .find(|record| record.summary.id == session_id)
            .unwrap();
        let messages = load_external_messages(&record);

        assert_eq!(2, messages.len(), "{messages:#?}");
        assert_eq!(MessageRole::Assistant, messages[1].role);
        assert_eq!("Only the native final should remain.", messages[1].content);
        assert_eq!(Some("final_answer"), messages[1].metadata["phase"].as_str());
        assert_eq!(
            Some("msg-native"),
            messages[1].metadata["nativeMessageId"].as_str()
        );
        assert!(
            messages
                .iter()
                .all(|message| message.metadata["source"].as_str() != Some("io-workbench"))
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_structured_and_plain_codex_task_errors() {
        assert_eq!(
            codex_task_error_detail(&json!({
                "message": "{\"detail\":\"model unsupported\"}",
                "codex_error_info": "other",
            }))
            .as_deref(),
            Some("model unsupported"),
        );
        assert_eq!(
            codex_task_error_detail(&json!({
                "message": "This content was flagged for possible cybersecurity risk.",
                "codex_error_info": "cyber_policy",
            }))
            .as_deref(),
            Some("This content was flagged for possible cybersecurity risk."),
        );
    }

    #[test]
    fn omits_inline_tool_data_and_bounds_external_tool_output() {
        let root = std::env::temp_dir().join(format!("iowb-external-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let session_id = "77777777-7777-4777-8777-777777777777";
        let file = root
            .join(".codex/sessions/2026/08/01")
            .join(format!("rollout-2026-08-01T00-00-00-{session_id}.jsonl"));
        let image = format!("data:image/png;base64,{}", "A".repeat(300_000));
        let long_text = format!("{}TAIL", "B".repeat(180_000));
        write_jsonl(
            &file,
            &[
                json!({"timestamp":"2026-08-01T00:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-08-01T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Inspect images","kind":"plain"}}),
                json!({"timestamp":"2026-08-01T00:00:02Z","type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"call-image","input":"view image"}}),
                json!({"timestamp":"2026-08-01T00:00:03Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call-image","output":[{"type":"input_image","image_url":image},{"type":"input_text","text":long_text}]}}),
            ],
        );

        let record = discover_external_sessions(&root)
            .into_iter()
            .find(|record| record.summary.id == session_id)
            .unwrap();
        let messages = load_external_messages(&record);
        let tool_use = &messages[1];
        let tool_output = &messages[2];

        assert!(tool_use.metadata.get("payload").is_none());
        assert!(tool_output.metadata.get("payload").is_none());
        assert!(!tool_output.content.contains("data:image/png;base64"));
        assert!(tool_output.content.contains("inline image/png omitted"));
        assert!(tool_output.content.contains("tool output truncated"));
        assert!(tool_output.content.contains("TAIL"));
        assert!(tool_output.content.len() <= MAX_EXTERNAL_TOOL_CONTENT_BYTES + 128);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignores_codex_subagent_rollouts_in_json_fallback() {
        let root = std::env::temp_dir().join(format!("iowb-codex-subagent-{}", Uuid::new_v4()));
        let project = root.join("project");
        let sessions_dir = root.join(".codex/sessions/2026/08/11");
        let parent_id = "11111111-1111-4111-8111-111111111111";
        let subagent_id = "22222222-2222-4222-8222-222222222222";
        fs::create_dir_all(&project).unwrap();
        write_jsonl(
            &sessions_dir.join(format!("rollout-parent-{parent_id}.jsonl")),
            &[
                json!({"timestamp":"2026-08-11T00:00:00Z","type":"session_meta","payload":{"id":parent_id,"cwd":project,"thread_source":"user"}}),
                json!({"timestamp":"2026-08-11T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Visible parent","kind":"plain"}}),
            ],
        );
        write_jsonl(
            &sessions_dir.join(format!("rollout-subagent-{subagent_id}.jsonl")),
            &[
                json!({"timestamp":"2026-08-11T00:00:02Z","type":"session_meta","payload":{"id":subagent_id,"cwd":project,"thread_source":"subagent","source":{"subagent":{"thread_spawn":{"parent_thread_id":parent_id}}}}}),
                json!({"timestamp":"2026-08-11T00:00:03Z","type":"event_msg","payload":{"type":"user_message","message":"Hidden child","kind":"plain"}}),
            ],
        );

        let records = discover_external_sessions(&root);
        assert!(records.iter().any(|record| record.summary.id == parent_id));
        assert!(
            records
                .iter()
                .all(|record| record.summary.id != subagent_id)
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hides_resumed_codex_ancestors_in_json_fallback() {
        let root = std::env::temp_dir().join(format!("iowb-codex-resume-{}", Uuid::new_v4()));
        let project = root.join("project");
        let sessions_dir = root.join(".codex/sessions/2026/08/13");
        let parent_id = "11111111-1111-4111-8111-111111111111";
        let resumed_id = "22222222-2222-4222-8222-222222222222";
        let sibling_id = "33333333-3333-4333-8333-333333333333";
        fs::create_dir_all(&project).unwrap();
        write_jsonl(
            &sessions_dir.join(format!("rollout-parent-{parent_id}.jsonl")),
            &[
                json!({"timestamp":"2026-08-13T00:00:00Z","type":"session_meta","payload":{"id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Original question","kind":"plain"}}),
            ],
        );
        write_jsonl(
            &sessions_dir.join(format!("rollout-resumed-{resumed_id}.jsonl")),
            &[
                json!({"timestamp":"2026-08-13T00:01:00Z","type":"session_meta","payload":{"id":resumed_id,"forked_from_id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:01:00Z","type":"session_meta","payload":{"id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:01:01Z","type":"event_msg","payload":{"type":"user_message","message":"Original question","kind":"plain"}}),
                json!({"timestamp":"2026-08-13T00:01:02Z","type":"event_msg","payload":{"type":"user_message","message":"Continue the original chat","kind":"plain"}}),
            ],
        );
        write_jsonl(
            &sessions_dir.join(format!("rollout-sibling-{sibling_id}.jsonl")),
            &[
                json!({"timestamp":"2026-08-13T00:02:00Z","type":"session_meta","payload":{"id":sibling_id,"forked_from_id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:02:01Z","type":"event_msg","payload":{"type":"user_message","message":"Explore another branch","kind":"plain"}}),
            ],
        );

        let ids = discover_external_sessions(&root)
            .into_iter()
            .map(|record| record.summary.id)
            .collect::<HashSet<_>>();
        assert_eq!(
            HashSet::from([resumed_id.to_string(), sibling_id.to_string()]),
            ids,
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_index_discovery_defers_rollout_message_loading() {
        let root = std::env::temp_dir().join(format!("iowb-codex-index-{}", Uuid::new_v4()));
        let project = root.join("project");
        let codex_dir = root.join(".codex");
        let session_id = "55555555-5555-4555-8555-555555555555";
        let subagent_id = "66666666-6666-4666-8666-666666666666";
        let rollout = codex_dir
            .join("sessions/2026/07/31")
            .join(format!("rollout-{session_id}.jsonl"));
        let subagent_rollout = codex_dir
            .join("sessions/2026/07/31")
            .join(format!("rollout-{subagent_id}.jsonl"));
        fs::create_dir_all(&project).unwrap();
        write_jsonl(
            &rollout,
            &[
                json!({"timestamp":"2026-07-31T00:00:00Z","type":"session_meta","payload":{"id":session_id,"cwd":project}}),
                json!({"timestamp":"2026-07-31T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Indexed question","kind":"plain"}}),
                json!({"timestamp":"2026-07-31T00:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Indexed answer"}]}}),
            ],
        );
        write_jsonl(
            &subagent_rollout,
            &[
                json!({"timestamp":"2026-07-31T00:00:03Z","type":"session_meta","payload":{"id":subagent_id,"cwd":project,"thread_source":"subagent","source":{"subagent":{"thread_spawn":{"parent_thread_id":session_id}}}}}),
                json!({"timestamp":"2026-07-31T00:00:04Z","type":"event_msg","payload":{"type":"user_message","message":"Indexed child","kind":"plain"}}),
            ],
        );

        let connection = Connection::open(codex_dir.join("state_5.sqlite")).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT NOT NULL,
                    cwd TEXT NOT NULL,
                    title TEXT NOT NULL,
                    first_user_message TEXT NOT NULL,
                    updated_at_ms INTEGER,
                    updated_at INTEGER NOT NULL,
                    model TEXT,
                    source TEXT NOT NULL DEFAULT 'exec',
                    thread_source TEXT,
                    archived INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE thread_spawn_edges (
                    parent_thread_id TEXT NOT NULL,
                    child_thread_id TEXT NOT NULL PRIMARY KEY,
                    status TEXT NOT NULL
                );
                "#,
            )
            .unwrap();
        connection
            .execute(
                r#"
                INSERT INTO threads (
                    id, rollout_path, cwd, title, first_user_message,
                    updated_at_ms, updated_at, model, thread_source, archived
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)
                "#,
                rusqlite::params![
                    session_id,
                    rollout.display().to_string(),
                    project.display().to_string(),
                    "Indexed session",
                    "Indexed question",
                    1_785_459_602_000_i64,
                    1_785_459_602_i64,
                    "gpt-test",
                    "user",
                ],
            )
            .unwrap();
        connection
            .execute(
                r#"
                INSERT INTO threads (
                    id, rollout_path, cwd, title, first_user_message,
                    updated_at_ms, updated_at, model, thread_source, archived
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)
                "#,
                rusqlite::params![
                    subagent_id,
                    subagent_rollout.display().to_string(),
                    project.display().to_string(),
                    "Indexed subagent",
                    "Indexed child",
                    1_785_459_603_000_i64,
                    1_785_459_603_i64,
                    "gpt-test",
                    Option::<String>::None,
                ],
            )
            .unwrap();
        connection
            .execute(
                r#"
                INSERT INTO thread_spawn_edges (
                    parent_thread_id, child_thread_id, status
                ) VALUES (?1, ?2, 'running')
                "#,
                rusqlite::params![session_id, subagent_id],
            )
            .unwrap();
        drop(connection);

        let records = discover_external_sessions(&root);
        let record = records
            .iter()
            .find(|record| record.summary.id == session_id)
            .unwrap();
        assert_eq!(1, record.summary.message_count);
        assert_eq!(2, load_external_messages(record).len());
        assert!(
            records
                .iter()
                .all(|record| record.summary.id != subagent_id)
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_index_hides_resumed_ancestors() {
        let root = std::env::temp_dir().join(format!("iowb-codex-index-resume-{}", Uuid::new_v4()));
        let project = root.join("project");
        let codex_dir = root.join(".codex");
        let parent_id = "11111111-1111-4111-8111-111111111111";
        let resumed_id = "22222222-2222-4222-8222-222222222222";
        let parent_rollout = codex_dir
            .join("sessions/2026/08/13")
            .join(format!("rollout-{parent_id}.jsonl"));
        let resumed_rollout = codex_dir
            .join("sessions/2026/08/13")
            .join(format!("rollout-{resumed_id}.jsonl"));
        fs::create_dir_all(&project).unwrap();
        write_jsonl(
            &parent_rollout,
            &[
                json!({"timestamp":"2026-08-13T00:00:00Z","type":"session_meta","payload":{"id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Original question","kind":"plain"}}),
            ],
        );
        write_jsonl(
            &resumed_rollout,
            &[
                json!({"timestamp":"2026-08-13T00:01:00Z","type":"session_meta","payload":{"id":resumed_id,"forked_from_id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:01:00Z","type":"session_meta","payload":{"id":parent_id,"cwd":project}}),
                json!({"timestamp":"2026-08-13T00:01:01Z","type":"event_msg","payload":{"type":"user_message","message":"Continue the original chat","kind":"plain"}}),
            ],
        );

        let connection = Connection::open(codex_dir.join("state_5.sqlite")).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT NOT NULL,
                    cwd TEXT NOT NULL,
                    title TEXT NOT NULL,
                    first_user_message TEXT NOT NULL,
                    updated_at_ms INTEGER,
                    updated_at INTEGER NOT NULL,
                    model TEXT,
                    source TEXT NOT NULL DEFAULT 'exec',
                    thread_source TEXT,
                    archived INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE thread_spawn_edges (
                    parent_thread_id TEXT NOT NULL,
                    child_thread_id TEXT NOT NULL PRIMARY KEY,
                    status TEXT NOT NULL
                );
                "#,
            )
            .unwrap();
        for (id, rollout, updated) in [
            (parent_id, &parent_rollout, 1_786_579_200_i64),
            (resumed_id, &resumed_rollout, 1_786_579_260_i64),
        ] {
            connection
                .execute(
                    r#"
                    INSERT INTO threads (
                        id, rollout_path, cwd, title, first_user_message,
                        updated_at_ms, updated_at, model, thread_source, archived
                    ) VALUES (?1, ?2, ?3, 'Same title', 'Original question', ?4, ?5, 'gpt-test', 'user', 0)
                    "#,
                    rusqlite::params![
                        id,
                        rollout.display().to_string(),
                        project.display().to_string(),
                        updated * 1_000,
                        updated,
                    ],
                )
                .unwrap();
        }
        drop(connection);

        let records = discover_external_sessions(&root);
        assert_eq!(1, records.len(), "{records:#?}");
        assert_eq!(resumed_id, records[0].summary.id);

        fs::remove_dir_all(root).unwrap();
    }

    fn write_jsonl(path: &Path, entries: &[Value]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let content = entries
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{content}\n")).unwrap();
    }

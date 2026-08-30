    #[test]
    fn codex_app_server_live_flag_gates_native_gateway_and_overrides() {
        assert!(!codex_app_server_live_enabled_for(
            Provider::Codex,
            ChatRuntime::NativeCli,
            false,
            false,
            false,
        ));
        assert!(codex_app_server_live_enabled_for(
            Provider::Codex,
            ChatRuntime::NativeCli,
            true,
            false,
            false,
        ));
        assert!(!codex_app_server_live_enabled_for(
            Provider::Codex,
            ChatRuntime::NativeCli,
            true,
            false,
            true,
        ));
        assert!(!codex_app_server_live_enabled_for(
            Provider::Claude,
            ChatRuntime::NativeCli,
            true,
            false,
            false,
        ));
        assert!(!codex_app_server_live_enabled_for(
            Provider::Codex,
            ChatRuntime::IoGateway,
            true,
            false,
            false,
        ));
        assert!(codex_app_server_live_enabled_for(
            Provider::Codex,
            ChatRuntime::IoGateway,
            true,
            true,
            false,
        ));
    }

    #[test]
    fn normalizes_codex_app_server_live_notifications() {
        let mut normalizer = CodexAppServerLiveOutputNormalizer::default();
        let mut output = String::new();
        output.push_str(&normalizer.push_notification(
            "item/reasoning/summaryTextDelta",
            &serde_json::json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "reason-1",
                "delta": "Inspecting files",
            }),
        ));
        output.push_str(&normalizer.push_notification(
            "item/commandExecution/outputDelta",
            &serde_json::json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "cmd-1",
                "delta": "/tmp/project\n",
            }),
        ));
        output.push_str(&normalizer.push_notification(
            "item/completed",
            &serde_json::json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": {
                    "type": "commandExecution",
                    "id": "cmd-1",
                    "command": "pwd",
                    "cwd": "/tmp/project",
                    "commandActions": [],
                    "aggregatedOutput": "/tmp/project\n",
                    "exitCode": 0,
                    "status": "completed"
                }
            }),
        ));
        output.push_str(&normalizer.push_notification(
            "thread/tokenUsage/updated",
            &serde_json::json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "tokenUsage": {
                    "last": {
                        "inputTokens": 20,
                        "cachedInputTokens": 5,
                        "cacheWriteInputTokens": 2,
                        "outputTokens": 8,
                        "reasoningOutputTokens": 3,
                        "totalTokens": 28
                    },
                    "total": {
                        "inputTokens": 20,
                        "cachedInputTokens": 5,
                        "outputTokens": 8,
                        "reasoningOutputTokens": 3,
                        "totalTokens": 28
                    }
                }
            }),
        ));
        output.push_str(&normalizer.push_notification(
            "item/agentMessage/delta",
            &serde_json::json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "msg-1",
                "delta": "Done.",
            }),
        ));
        output.push_str(&normalizer.push_notification(
            "turn/completed",
            &serde_json::json!({
                "threadId": "thread-1",
                "turn": {
                    "id": "turn-1",
                    "status": "completed",
                    "items": [
                        {
                            "type": "agentMessage",
                            "id": "msg-1",
                            "text": "Done.",
                            "phase": "final_answer"
                        }
                    ],
                    "error": null
                }
            }),
        ));
        output.push_str(&normalizer.finish());

        assert!(output.contains("thinking\nInspecting files"), "{output}");
        assert!(output.contains("exec / Details"), "{output}");
        assert!(output.contains("exec / Parameters"), "{output}");
        assert!(output.contains("### Command\n```sh\npwd"), "{output}");
        assert!(output.contains("tokens used"), "{output}");
        assert!(output.contains("codex\nDone."), "{output}");
        assert_eq!(output.matches("codex\nDone.").count(), 1);
        assert_eq!(
            normalizer.take_final_assistant_message().as_deref(),
            Some("Done.")
        );
        let usage = normalizer.take_final_usage().expect("usage");
        assert_eq!(usage.source, "codex.app_server.turn.usage");
        assert_eq!(usage.usage.used, 28);
        assert_eq!(usage.usage.cache_read, 5);
        assert_eq!(usage.usage.cache_creation, 2);
        let tools = normalizer.take_tool_messages();
        assert!(tools.iter().any(|tool| tool.name == "command_execution"));
    }

    #[test]
    fn codex_app_server_normalizer_suppresses_usage_until_visible_output() {
        let mut normalizer = CodexAppServerLiveOutputNormalizer::default();
        let stale = normalizer.push_notification(
            "thread/tokenUsage/updated",
            &serde_json::json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "tokenUsage": {
                    "last": { "inputTokens": 10, "outputTokens": 1, "totalTokens": 11 },
                    "total": { "inputTokens": 10, "outputTokens": 1, "totalTokens": 11 }
                }
            }),
        );
        assert_eq!(stale, "");

        let mut output = String::new();
        output.push_str(&normalizer.push_notification(
            "item/agentMessage/delta",
            &serde_json::json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "msg-1",
                "delta": "Done.",
            }),
        ));
        output.push_str(&normalizer.push_notification(
            "thread/tokenUsage/updated",
            &serde_json::json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "tokenUsage": {
                    "last": { "inputTokens": 20, "outputTokens": 2, "totalTokens": 22 },
                    "total": { "inputTokens": 20, "outputTokens": 2, "totalTokens": 22 }
                }
            }),
        ));

        assert!(output.contains("codex\nDone."), "{output}");
        assert!(output.contains("tokens used"), "{output}");
        let usage = normalizer.take_final_usage().expect("usage");
        assert_eq!(usage.usage.used, 22);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn codex_app_server_live_runtime_persists_assistant_and_thread_id() {
        use std::os::unix::fs::PermissionsExt;

        let (mut state, root, project) = temporary_app_state("app-server-live-runtime").await;
        state.sessions.external_home = Arc::new(root.clone());
        let script = root.join("live-codex.sh");
        let log = root.join("live-requests.log");
        let rollout = root
            .join(".codex/sessions/2026/08/19")
            .join("rollout-2026-08-19T00-00-00-thread-runtime.jsonl");
        std::fs::create_dir_all(rollout.parent().expect("rollout parent")).expect("rollout dir");
        std::fs::write(
            &rollout,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({
                    "timestamp": Utc::now(),
                    "type": "session_meta",
                    "payload": {"id": "thread-runtime", "cwd": project}
                }),
                serde_json::json!({
                    "timestamp": Utc::now(),
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "native prompt before Workbench",
                        "kind": "plain"
                    }
                }),
                serde_json::json!({
                    "timestamp": Utc::now(),
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "native answer before Workbench"}]
                    }
                })
            ),
        )
        .expect("rollout");
        let original_rollout = std::fs::read_to_string(&rollout).expect("original rollout");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 read first\nprintf '%s\\n' \"$first\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":1,\"result\":{{\"userAgent\":\"test\"}}}}'\n\
                 read second\nprintf '%s\\n' \"$second\" >> '{}'\n\
                 read third\nprintf '%s\\n' \"$third\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"thread-runtime\"}}}}}}'\n\
                 read fourth\nprintf '%s\\n' \"$fourth\" >> '{}'\n\
                 printf '%s\\n' '{{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"turn-runtime\",\"status\":\"inProgress\",\"items\":[]}}}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"item/agentMessage/delta\",\"params\":{{\"threadId\":\"thread-runtime\",\"turnId\":\"turn-runtime\",\"itemId\":\"msg-runtime\",\"delta\":\"runtime answer\"}}}}'\n\
                 printf '%s\\n' '{{\"method\":\"turn/completed\",\"params\":{{\"threadId\":\"thread-runtime\",\"turn\":{{\"id\":\"turn-runtime\",\"status\":\"completed\",\"items\":[{{\"type\":\"agentMessage\",\"id\":\"msg-runtime\",\"text\":\"runtime answer\",\"phase\":\"final_answer\"}}]}}}}}}'\n",
                log.display(),
                log.display(),
                log.display(),
                log.display(),
            ),
        )
        .expect("script");
        let mut permissions = std::fs::metadata(&script).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("permissions");

        let session = state
            .sessions
            .create_or_update(
                Provider::Codex,
                project.display().to_string(),
                Some("session-app-server-live-runtime".to_string()),
                false,
                None,
                Some(ChatRuntime::NativeCli),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("session");
        state
            .sessions
            .append_message(&session.id, MessageRole::User, "hello runtime")
            .await
            .expect("user message");
        let run_id = "run-app-server-live-runtime".to_string();
        let attempt_id = "attempt-app-server-live-runtime".to_string();
        let durable_run = StoredDurableChatRun::new(
            run_id.clone(),
            None,
            session.id.clone(),
            Provider::Codex.as_str(),
            "hello runtime",
            project.display().to_string(),
        );
        state
            .storage
            .create_durable_chat_run(&durable_run)
            .expect("durable run");
        state
            .storage
            .create_chat_run_attempt(&StoredChatRunAttempt::new(
                attempt_id.clone(),
                run_id.clone(),
                session.id.clone(),
                None,
                Provider::Codex.as_str(),
                runtime_label(ChatRuntime::NativeCli),
                None,
                None,
            ))
            .expect("attempt");

        let mut manager = AgentRuntimeManager::new(10);
        manager.codex_app_server =
            CodexAppServerClient::new(script.as_os_str(), Duration::from_secs(2));
        manager
            .start_codex_app_server_live(AgentStartContext {
                provider: Provider::Codex,
                session_id: session.id.clone(),
                durable_run_id: Some(run_id.clone()),
                attempt_id: Some(attempt_id.clone()),
                response_id: "response-runtime".to_string(),
                sequence: Arc::new(AtomicU64::new(0)),
                project_path: project.clone(),
                prompt: "hello runtime".to_string(),
                model: None,
                runtime: ChatRuntime::NativeCli,
                effort: None,
                mode: None,
                thinking: None,
                fast: None,
                native_resume_session_id: None,
                native_rollout_owned_by_provider: false,
                context_rollover_id: None,
                direct_ai_config: None,
                direct_ai_messages: Vec::new(),
                sessions: state.sessions.clone(),
                storage: state.storage.clone(),
                hub: state.ws_hub.clone(),
            })
            .await
            .expect("start app-server runtime");

        timeout(Duration::from_secs(2), async {
            loop {
                let messages = state.storage.list_messages(&session.id).expect("messages");
                if messages
                    .iter()
                    .any(|message| message.role == MessageRole::Assistant)
                {
                    break;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("assistant persisted");

        let messages = state.storage.list_messages(&session.id).expect("messages");
        assert!(
            messages.iter().any(|message| {
                message.role == MessageRole::Assistant && message.content == "runtime answer"
            }),
            "{messages:?}"
        );
        let stored = state
            .storage
            .get_session_summary(&session.id)
            .expect("session lookup")
            .expect("session");
        assert!(!stored.active);
        assert_eq!(stored.native_session_id.as_deref(), Some("thread-runtime"));
        assert!(stored.native_rollout_owned_by_provider);
        let visible_messages = state
            .sessions
            .messages_including_external(&session.id)
            .await
            .expect("visible messages");
        assert_eq!(
            visible_messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            ["hello runtime", "runtime answer"],
            "app-server-owned native rollout context must not leak into Workbench messages"
        );
        let (tail_messages, tail_total) = state
            .sessions
            .messages_tail_including_external(&session.id, 20)
            .await
            .expect("visible tail");
        assert_eq!(tail_total, 2);
        assert_eq!(
            tail_messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            ["hello runtime", "runtime answer"],
            "app-server-owned native rollout context must not leak into Workbench message tail"
        );
        let durable = state
            .storage
            .get_durable_chat_run(&run_id)
            .expect("durable lookup")
            .expect("durable run");
        assert_eq!(durable.native_session_id.as_deref(), Some("thread-runtime"));
        assert_eq!(
            state
                .storage
                .chat_run_attempt_native_session_id(&attempt_id)
                .expect("attempt lookup")
                .as_deref(),
            Some("thread-runtime")
        );
        let requests = std::fs::read_to_string(log).expect("requests");
        assert!(requests.contains("\"method\":\"thread/start\""));
        assert!(requests.contains("\"method\":\"turn/start\""));
        assert!(requests.contains("\"approvalPolicy\":\"never\""));
        assert_eq!(
            original_rollout,
            std::fs::read_to_string(&rollout).expect("rollout after app-server live turn"),
            "app-server-owned native rollout must not be appended by legacy Workbench sync"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn normalizes_codex_agent_messages_and_apply_patch_without_duplicates() {
        let mut normalizer = CodexLiveOutputNormalizer::default();
        let output = normalizer.push(concat!(
            "{\"type\":\"item.started\",\"item\":{\"id\":\"patch-1\",\"type\":\"custom_tool_call\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"message-1\",\"type\":\"agent_message\",",
            "\"text\":\"I will update the file.\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"patch-1\",\"type\":\"custom_tool_call\",",
            "\"name\":\"apply_patch\",\"input\":\"*** Begin Patch\\n*** Add File: new.txt\\n+new\\n",
            "*** Update File: old.txt\\n-old\\n+updated\\n*** End Patch\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"message-2\",\"type\":\"agent_message\",",
            "\"text\":\"Both files are ready.\"}}\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":12,\"output_tokens\":8}}\n"
        ));

        assert!(
            output.contains("thinking\nI will update the file."),
            "{output}"
        );
        assert!(output.contains("apply_patch"), "{output}");
        assert!(output.contains("create / new.txt"), "{output}");
        assert!(output.contains("edit / old.txt"), "{output}");
        assert!(output.contains("```diff"), "{output}");
        assert!(output.contains("codex\nBoth files are ready."), "{output}");
        assert!(output.contains("tokens used"), "{output}");
        assert_eq!(output.matches("apply_patch").count(), 1, "{output}");
        assert_eq!(
            normalizer.take_final_assistant_message().as_deref(),
            Some("Both files are ready.")
        );
        let tools = normalizer.take_tool_messages();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "apply_patch");
    }

    #[test]
    fn codex_unphased_final_survives_trailing_todo_before_completion() {
        let mut normalizer = CodexLiveOutputNormalizer::default();
        let mut output = normalizer.push(concat!(
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"message-final\",\"type\":\"agent_message\",",
            "\"text\":\"Only one clean final response.\"}}\n"
        ));
        assert!(output.is_empty(), "{output}");

        output.push_str(&normalizer.push(concat!(
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"todo-final\",\"type\":\"todo_list\",",
            "\"items\":[{\"text\":\"Done\",\"completed\":true}]}}\n"
        )));
        assert!(
            !output.contains("Only one clean final response."),
            "{output}"
        );

        output.push_str(&normalizer.push(
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":6}}\n",
        ));

        assert!(
            output.contains("codex\nOnly one clean final response."),
            "{output}"
        );
        assert!(
            !output.contains("thinking\nOnly one clean final response."),
            "{output}"
        );
        assert_eq!(output.matches("Only one clean final response.").count(), 1);
        assert_eq!(
            normalizer.take_final_assistant_message().as_deref(),
            Some("Only one clean final response.")
        );
    }

    #[test]
    fn codex_explicit_final_remains_canonical_across_trailing_tools() {
        let mut normalizer = CodexLiveOutputNormalizer::default();
        let output = normalizer.push(concat!(
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"commentary\",\"type\":\"agent_message\",",
            "\"phase\":\"commentary\",\"text\":\"Checking the result.\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"command\",\"type\":\"command_execution\",",
            "\"command\":\"true\",\"aggregated_output\":\"\",\"exit_code\":0,\"status\":\"completed\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"final\",\"type\":\"agent_message\",",
            "\"phase\":\"final_answer\",\"text\":\"The final answer is stable.\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"todo\",\"type\":\"todo_list\",\"items\":[]}}\n",
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":12,\"output_tokens\":8}}\n"
        ));

        assert!(
            output.contains("thinking\nChecking the result."),
            "{output}"
        );
        assert!(
            output.contains("codex\nThe final answer is stable."),
            "{output}"
        );
        assert_eq!(output.matches("The final answer is stable.").count(), 1);
        assert_eq!(
            normalizer.take_final_assistant_message().as_deref(),
            Some("The final answer is stable.")
        );
    }

    #[test]
    fn successful_codex_output_never_falls_back_to_live_transcript() {
        let transcript = concat!(
            "thinking\nInspecting files\n\n",
            "exec / Parameters\n**Tool:** `command_execution`\n\n",
            "codex\nThe actual final.\n\n",
            "tokens used\n{\"output_tokens\":8}"
        );

        assert_eq!(
            select_completed_agent_output(Provider::Codex, None, transcript, true),
            Err(CODEX_MISSING_FINAL_RESPONSE.to_string())
        );
        assert_eq!(
            select_completed_agent_output(
                Provider::Codex,
                Some("The actual final.".to_string()),
                transcript,
                true,
            ),
            Ok("The actual final.".to_string())
        );
        assert_eq!(
            select_completed_agent_output(Provider::Claude, None, transcript, false),
            Ok(transcript.to_string())
        );
        assert_eq!(
            select_completed_agent_output(Provider::Codex, None, "plain custom output", false),
            Ok("plain custom output".to_string())
        );
    }

    #[test]
    fn bounds_pathological_codex_tool_output_and_websocket_chunks() {
        let pathological = format!(
            "<style>body{{display:none}}</style><script>bad()</script>\0{}TAIL",
            "x".repeat(1_246_298)
        );
        let item = serde_json::json!({
            "type": "custom_tool_call_output",
            "name": "browser_output",
            "output": pathological,
        });
        let formatted = format_codex_live_tool_result(&item, "custom_tool_call");
        assert!(formatted.len() <= AGENT_TOOL_MESSAGE_MAX_BYTES);
        assert!(formatted.contains("truncated tool output"), "{formatted}");
        assert!(formatted.contains("TAIL"), "{formatted}");
        assert!(!formatted.contains('\0'));
        assert!(
            formatted
                .lines()
                .map(|line| line.chars().count())
                .max()
                .unwrap_or(0)
                <= AGENT_DISPLAY_MAX_LINE_CHARS + 80
        );

        let chunks = websocket_text_chunks(&formatted);
        assert!(chunks.len() >= 2);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.len() <= AGENT_WEBSOCKET_CHUNK_MAX_BYTES)
        );
        assert_eq!(chunks.concat(), formatted);
    }

    #[test]
    fn codex_normalizer_separates_bounded_tool_rows_from_final_answer() {
        let event = serde_json::json!({
            "type": "item.completed",
            "item": {
                "type": "custom_tool_call_output",
                "name": "large_tool",
                "output": "z".repeat(200_000),
            }
        });
        let final_event = serde_json::json!({
            "type": "item.completed",
            "item": {
                "type": "agent_message",
                "phase": "final_answer",
                "text": "Only this is the final answer.",
            }
        });
        let mut normalizer = CodexLiveOutputNormalizer::default();
        let visible = normalizer.push(&format!("{event}\n{final_event}\n"));
        assert!(visible.contains("large_tool"));
        assert!(visible.contains("Only this is the final answer."));
        let tools = normalizer.take_tool_messages();
        assert_eq!(tools.len(), 1);
        assert!(tools[0].content.len() <= AGENT_TOOL_MESSAGE_MAX_BYTES);
        assert_eq!(
            normalizer.take_final_assistant_message().as_deref(),
            Some("Only this is the final answer.")
        );
    }

    #[test]
    fn codex_live_normalizer_preserves_plain_output_and_partial_last_line() {
        let mut normalizer = CodexLiveOutputNormalizer::default();
        assert_eq!(normalizer.push("plain out"), "");
        assert_eq!(normalizer.push("put\n"), "plain output\n");
        assert_eq!(normalizer.push("last line"), "");
        assert_eq!(normalizer.finish(), "last line\n");
    }

    #[test]
    fn run_usage_normalizers_keep_total_and_subset_fields_separate() {
        let codex = normalize_codex_run_usage(&serde_json::json!({
            "input_tokens": 30,
            "cached_input_tokens": 12,
            "cache_write_input_tokens": 3,
            "output_tokens": 12,
            "reasoning_output_tokens": 5,
            "total_tokens": 42
        }));
        assert_eq!(codex.usage.used, 42);
        assert_eq!(codex.usage.input, 30);
        assert_eq!(codex.usage.output, 12);
        assert_eq!(codex.usage.cache_read, 12);
        assert_eq!(codex.usage.cache_creation, 3);
        assert_eq!(codex.usage.reasoning, 5);

        let claude = normalize_claude_run_usage(&serde_json::json!({
            "type": "result",
            "modelUsage": {
                "sonnet": {
                    "input_tokens": 10,
                    "cache_creation_input_tokens": 20,
                    "cache_read_input_tokens": 30,
                    "output_tokens": 40
                },
                "haiku": {
                    "input_tokens": 1,
                    "output_tokens": 4
                }
            },
            "total_cost_usd": 0.02
        }));
        assert_eq!(claude.usage.used, 55);
        assert_eq!(claude.usage.input, 11);
        assert_eq!(claude.usage.output, 44);
        assert_eq!(claude.usage.cache_creation, 20);
        assert_eq!(claude.usage.cache_read, 30);
        assert_eq!(claude.usage.cost_usd, 0.02);

        let gemini = normalize_gemini_run_usage(&serde_json::json!({
            "type": "result",
            "stats": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 25,
                "cachedContentTokenCount": 80,
                "thoughtsTokenCount": 7,
                "totalTokenCount": 125
            }
        }))
        .expect("gemini usage");
        assert_eq!(gemini.usage.used, 125);
        assert_eq!(gemini.usage.input, 100);
        assert_eq!(gemini.usage.output, 25);
        assert_eq!(gemini.usage.cache_read, 80);
        assert_eq!(gemini.usage.reasoning, 7);
    }

    #[test]
    fn claude_and_gemini_normalizers_capture_native_session_ids() {
        let mut claude = ClaudeLiveOutputNormalizer::default();
        let claude_output = claude.push_chunks(concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"claude-native\"}\n",
            "{\"type\":\"stream_event\",\"session_id\":\"claude-native\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"continued\"}}}\n"
        ));
        assert_eq!(claude_output, ["claude\ncontinued"]);
        assert_eq!(claude.take_session_id().as_deref(), Some("claude-native"));
        assert!(
            claude
                .push_chunks(
                    "{\"type\":\"stream_event\",\"session_id\":\"claude-native\",\"event\":{\"type\":\"ping\"}}\n"
                )
                .is_empty()
        );
        assert_eq!(claude.take_session_id(), None);

        let mut gemini = GeminiLiveOutputNormalizer::default();
        let gemini_output = gemini.push(concat!(
            "{\"type\":\"init\",\"session_id\":\"gemini-native\"}\n",
            "{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"continued\",\"delta\":true}\n"
        ));
        assert_eq!(gemini_output, "continued");
        assert_eq!(gemini.take_session_id().as_deref(), Some("gemini-native"));
    }

    #[test]
    fn claude_normalizer_streams_wrapped_deltas_without_repeating_final_result() {
        let mut claude = ClaudeLiveOutputNormalizer::default();

        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello \"}}}\n"
            ),
            ["claude\nHello "]
        );
        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"mobile\"}}}\n"
            ),
            ["mobile"]
        );
        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"Hello mobile\"}\n"
            ),
            Vec::<String>::new()
        );
        assert_eq!(claude.finish(), "");
        assert_eq!(
            claude.take_final_assistant_message().as_deref(),
            Some("Hello mobile")
        );
    }

    #[test]
    fn claude_normalizer_streams_thinking_before_final_text() {
        let mut claude = ClaudeLiveOutputNormalizer::default();

        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Inspecting files\"}}}\n"
            ),
            ["thinking\nInspecting files"]
        );
        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\" now\"}}}\n"
            ),
            [" now"]
        );
        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Finished.\"}}}\n"
            ),
            ["\n\nclaude\nFinished."]
        );
        assert_eq!(
            claude.push_chunks(
                "{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"Finished.\"}\n"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn claude_normalizer_formats_tool_use_sections() {
        let mut claude = ClaudeLiveOutputNormalizer::default();

        let output = claude.push_chunks(concat!(
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"Bash\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"pwd\\\"}\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_stop\"}}\n",
            "{\"type\":\"tool_result\",\"tool_use_id\":\"toolu_1\",\"name\":\"Bash\",\"content\":\"/tmp/project\\n\"}\n"
        ));

        assert_eq!(output.len(), 2);
        assert!(output[0].contains("exec / Parameters"), "{output:?}");
        assert!(output[0].contains("**Tool:** `Bash`"), "{output:?}");
        assert!(output[0].contains("### Command"), "{output:?}");
        assert!(output[0].contains("pwd"), "{output:?}");
        assert!(output[1].contains("exec / Details"), "{output:?}");
        assert!(output[1].contains("/tmp/project"), "{output:?}");
    }

    #[test]
    fn claude_normalizer_formats_message_enveloped_thinking_and_tools() {
        let mut claude = ClaudeLiveOutputNormalizer::default();

        let output = claude.push_chunks(concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":[",
            "{\"type\":\"thinking\",\"thinking\":\"Checking files\"},",
            "{\"type\":\"tool_use\",\"id\":\"toolu_2\",\"name\":\"Read\",\"input\":{\"file_path\":\"Cargo.toml\"}},",
            "{\"type\":\"text\",\"text\":\"Done.\"}",
            "]}}\n"
        ));

        assert_eq!(output.len(), 1);
        assert!(output[0].contains("thinking\nChecking files"), "{output:?}");
        assert!(output[0].contains("tool / Parameters"), "{output:?}");
        assert!(output[0].contains("**Tool:** `Read`"), "{output:?}");
        assert!(
            output[0].contains("\"file_path\": \"Cargo.toml\""),
            "{output:?}"
        );
        assert!(output[0].contains("Done."), "{output:?}");
    }

    #[test]
    fn claude_normalizer_prefers_stream_events_over_duplicate_message_envelopes() {
        let mut claude = ClaudeLiveOutputNormalizer::default();

        let output = claude.push_chunks(concat!(
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Checking Cargo.toml presence\"}}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[",
            "{\"type\":\"thinking\",\"thinking\":\"Checking Cargo.toml presence\"},",
            "{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"Bash\",\"input\":{\"command\":\"pwd && ls Cargo.toml\"}},",
            "{\"type\":\"text\",\"text\":\"Cargo.toml exists.\"}",
            "]}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"Bash\",\"input\":{}}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"pwd && ls Cargo.toml\\\"}\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_stop\"}}\n",
            "{\"type\":\"tool_result\",\"tool_use_id\":\"call_1\",\"content\":\"/tmp/project\\nCargo.toml\\n\"}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Cargo.toml exists.\"}}}\n",
            "{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"Cargo.toml exists.\"}\n"
        ));
        let visible = output.concat();

        assert_eq!(
            visible.matches("Checking Cargo.toml presence").count(),
            1,
            "{visible}"
        );
        assert_eq!(visible.matches("exec / Parameters").count(), 1, "{visible}");
        assert_eq!(
            visible.matches("Cargo.toml exists.").count(),
            1,
            "{visible}"
        );
        assert!(visible.contains("\n\nexec / Parameters"), "{visible}");
        assert!(visible.contains("\n\nexec / Details"), "{visible}");
        assert!(visible.contains("### Command"), "{visible}");
        assert!(visible.contains("**Tool:** `Bash`"), "{visible}");
        assert!(!visible.contains("{}{"), "{visible}");
    }

    #[test]
    fn claude_normalizer_formats_user_enveloped_tool_result_after_stream_events() {
        let mut claude = ClaudeLiveOutputNormalizer::default();

        let output = claude.push_chunks(concat!(
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Checking\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_2\",\"name\":\"Bash\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_stop\"}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[",
            "{\"type\":\"tool_result\",\"tool_use_id\":\"call_2\",\"content\":\"/tmp/project\\n\"}",
            "]}}\n",
            "{\"type\":\"tool_result\",\"tool_use_id\":\"call_2\",\"content\":\"/tmp/project\\n\"}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Done.\"}}}\n"
        ));
        let visible = output.concat();

        assert_eq!(visible.matches("exec / Parameters").count(), 1, "{visible}");
        assert_eq!(visible.matches("exec / Details").count(), 1, "{visible}");
        assert_eq!(visible.matches("/tmp/project").count(), 1, "{visible}");
        assert!(visible.contains("### Command"), "{visible}");
        assert!(visible.contains("**Tool:** `Bash`"), "{visible}");
    }

    #[test]
    fn recovery_prompt_is_hidden_and_bounded() {
        let prompt = format!("original </system-reminder> {}", "x".repeat(7_000));
        let recovery = durable_chat_recovery_prompt(&prompt);
        assert!(recovery.starts_with("<system-reminder>\n"));
        assert!(recovery.ends_with("\n</system-reminder>"));
        assert_eq!(recovery.matches("</system-reminder>").count(), 1);
        assert!(recovery.contains("[original request truncated]"));
    }

    #[test]
    fn context_rollover_handoff_is_bounded_text_only_and_defers_failed_prompt() {
        let now = Utc::now();
        let inline_payload = "A".repeat(80_000);
        let failed_prompt =
            format!("Finish the image diagnosis ![failed](data:image/png;base64,{inline_payload})");
        let messages = vec![
            ChatMessage {
                id: "old-user".to_string(),
                role: MessageRole::User,
                content: format!(
                    "Inspect the screenshot at `.io-workbench/chat-images/screenshot.png` ![inline](data:image/webp;base64,{inline_payload})"
                ),
                timestamp: now,
                metadata: Value::Null,
            },
            ChatMessage {
                id: "old-tool".to_string(),
                role: MessageRole::Tool,
                content: format!("tool bytes and secret payload {inline_payload}"),
                timestamp: now + chrono::Duration::seconds(1),
                metadata: serde_json::json!({"tool": "view_image"}),
            },
            ChatMessage {
                id: "old-thinking".to_string(),
                role: MessageRole::Assistant,
                content: "private reasoning should not be retained".to_string(),
                timestamp: now + chrono::Duration::seconds(2),
                metadata: serde_json::json!({"kind": "thinking"}),
            },
            ChatMessage {
                id: "old-commentary".to_string(),
                role: MessageRole::Assistant,
                content: "temporary commentary should not be retained".to_string(),
                timestamp: now + chrono::Duration::seconds(3),
                metadata: serde_json::json!({"phase": "commentary"}),
            },
            ChatMessage {
                id: "old-assistant".to_string(),
                role: MessageRole::Assistant,
                content:
                    "The request failed because the native context exceeded the gateway limit."
                        .to_string(),
                timestamp: now + chrono::Duration::seconds(4),
                metadata: Value::Null,
            },
            ChatMessage {
                id: "failed-user".to_string(),
                role: MessageRole::User,
                content: failed_prompt.clone(),
                timestamp: now + chrono::Duration::seconds(5),
                metadata: Value::Null,
            },
        ];

        let handoff = build_context_rollover_handoff(messages, &failed_prompt);

        assert!(handoff.starts_with("<system-reminder>\n"));
        assert!(handoff.contains("Recent text-only handoff:"));
        assert!(handoff.contains(".io-workbench/chat-images/screenshot.png"));
        assert!(handoff.contains("native context exceeded the gateway limit"));
        assert!(handoff.contains("[inline image omitted; use its local file path if available]"));
        assert!(!handoff.contains(";base64,"));
        assert!(!handoff.contains(&inline_payload));
        assert!(!handoff.contains("tool bytes and secret payload"));
        assert!(!handoff.contains("private reasoning should not be retained"));
        assert!(!handoff.contains("temporary commentary should not be retained"));
        assert_eq!(handoff.matches("Finish the image diagnosis").count(), 0);
        assert!(
            handoff.len() <= CONTEXT_ROLLOVER_HANDOFF_MAX_BYTES + 2 * 1024,
            "handoff unexpectedly large: {} bytes",
            handoff.len()
        );
    }

    #[test]
    fn context_rollover_handoff_keeps_newest_text_that_fits() {
        let now = Utc::now();
        let mut messages = (0..40)
            .map(|index| ChatMessage {
                id: format!("message-{index:02}"),
                role: if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                content: format!("history-{index:02} {}", "x".repeat(3_000)),
                timestamp: now + chrono::Duration::seconds(index),
                metadata: Value::Null,
            })
            .collect::<Vec<_>>();
        messages.push(ChatMessage {
            id: "failed-user".to_string(),
            role: MessageRole::User,
            content: "retry newest request".to_string(),
            timestamp: now + chrono::Duration::seconds(41),
            metadata: Value::Null,
        });

        let handoff = build_context_rollover_handoff(messages, "retry newest request");

        assert!(handoff.contains("history-39"));
        assert!(!handoff.contains("history-00"));
        assert_eq!(handoff.matches("retry newest request").count(), 0);
        assert!(
            handoff.len() <= CONTEXT_ROLLOVER_HANDOFF_MAX_BYTES + 2 * 1024,
            "handoff unexpectedly large: {} bytes",
            handoff.len()
        );
    }

# Codex App-Server Migration Plan

## Goal

Migrate live Codex chat execution from `codex exec --json` to `codex app-server --stdio` while keeping the existing io-workbench client protocol stable.

The public Workbench contract must remain:

- Clients send `start_session` and `abort_session`.
- Clients receive the same `session_status`, `output`, `session_metadata`, `error`, and `active_sessions` events.
- Web, mobile, and desktop rendering must continue to work without client-side protocol changes.
- Existing `native_cli` and `io_gateway` runtime labels must remain valid.
- The old `codex exec --json` path must remain available as rollback/fallback.

## Non-Goals For The First Migration

- Do not expose a new public `app_server` runtime value to web/mobile/desktop yet.
- Do not redesign chat UI, mobile UI, or desktop UI.
- Do not require JavaScript Codex SDK usage.
- Do not replace Workbench WebSocket protocol with Codex app-server protocol.
- Do not implement full approval UI in the first pass unless the backend cannot avoid blocking approval requests safely.
- Do not migrate Claude or Gemini runtime behavior.

## Compatibility Rule

The app-server implementation is internal backend plumbing. It must adapt app-server JSON-RPC events into the existing Workbench event and transcript format.

Current path:

```text
client -> Workbench WS -> codex exec --json -> CodexLiveOutputNormalizer -> Workbench WS events
```

Target path:

```text
client -> Workbench WS -> codex app-server JSON-RPC -> AppServerLiveTurnAdapter -> Workbench WS events
```

## Feature Flag And Rollback

1. Add an internal feature flag for Codex app-server live turns.
2. Keep `codex exec --json` as the default until the app-server path passes tests.
3. Make rollback immediate by disabling the feature flag.
4. Do not persist a new public runtime enum value that older mobile clients cannot decode.
5. Preserve existing `native_cli` / `io_gateway` labels in session summaries and metadata.

## Files Likely To Change

- `crates/iowb-core/src/codex_app_server.rs`
- `crates/iowb-core/src/lib.rs`
- `crates/iowb-core/src/external_sessions.rs`
- `crates/iowb-protocol/src/lib.rs` only if an internal enum/metadata field is absolutely necessary
- `crates/iowb-storage/src/lib.rs` if app-server attempt/source handling needs token usage or native session fixes
- `crates/iowb-server/src/agentic_board.rs` only if board runtime selection needs an internal branch
- Tests near the changed Rust modules

Avoid changing these in the first pass unless a verified compatibility bug forces it:

- `crates/iowb-ui/static/app.js`
- `apps/mobile/shared/...`
- `apps/desktop/native/...`

## Implementation Steps

### 1. Capture Current Behavior

1. Record the current Codex live output behavior from `codex exec --json`.
2. Preserve current visible section names:
   - `thinking`
   - `codex`
   - `exec / Parameters`
   - `exec / Details`
   - `tokens used`
3. Preserve ordered streaming semantics:
   - starting status
   - running status
   - ordered output chunks with `responseId` and `sequence`
   - final empty `done: true` output
   - final `session_metadata`
   - completed/failed/aborted status

### 2. Add App-Server JSON-RPC Transport

1. Extend `CodexAppServerClient` or add a new live-turn client in `codex_app_server.rs`.
2. Start `codex app-server --stdio`.
3. Send `initialize`.
4. Send `initialized`.
5. Maintain request ids and match responses by id.
6. Continuously read stdout for:
   - JSON-RPC responses
   - JSON-RPC notifications
   - JSON-RPC server-initiated requests
7. Keep stderr captured for diagnostics without leaking noisy output to chat.
8. Ensure child process cleanup on completion, failure, abort, and dropped task.

### 3. Start Or Resume Thread

1. For a new Workbench Codex session, call `thread/start`.
2. For a continued Workbench session with `native_session_id`, call `thread/resume`.
3. Use the Workbench project path as app-server `cwd`.
4. Preserve model, effort, mode, thinking, fast, and gateway settings as much as app-server supports.
5. Store returned app-server thread id as Workbench `native_session_id`.
6. Update the active durable run native session id once known.
7. Do not infer thread id from CLI `thread.started` lines in the app-server path; get it from app-server responses/notifications.

### 4. Start Turn

1. Convert the Workbench prompt string into app-server `turn/start.input`.
2. Add a text item for normal prompt text.
3. Convert recognized image attachment markers into `localImage` items.
4. Start the turn with the target `threadId`.
5. Capture returned/started `turnId` for interrupt and event routing.
6. Keep request and notification routing scoped by `threadId` and `turnId`.

### 5. Image Input Handling

1. Parse web/mobile attachment lines:

   ```text
   Attached image file: `path` (name, mime)
   ```

2. Resolve uploaded image paths against the session project root because upload returns project-relative paths.
3. Convert each existing image marker into:

   ```json
   { "type": "localImage", "path": "/absolute/path" }
   ```

4. Remove recognized image marker lines from the text prompt sent as the app-server text item.
5. Preserve unrecognized markdown or text as normal prompt text.
6. Add tests for:
   - one image
   - multiple images
   - prompt-only
   - image-only
   - unsafe path outside project root
   - missing file behavior
7. Keep desktop mismatch noted separately: desktop currently appends markdown using `image.data`, while upload returns `path`. Do not rely on desktop sending the same marker until fixed.

### 6. Event Normalization

Add an app-server normalizer that converts JSON-RPC methods/items into the same visible transcript shape as the current `CodexLiveOutputNormalizer`.

Map at least:

- `item/agentMessage/delta`
- `item/completed` with `agentMessage`
- `item/completed` with `reasoning`
- `item/reasoning/summaryTextDelta`
- `item/reasoning/textDelta`
- `item/completed` with `commandExecution`
- `item/commandExecution/outputDelta`
- `item/completed` with `fileChange`
- `turn/diff/updated`
- `turn/plan/updated`
- `thread/tokenUsage/updated`
- `turn/completed`
- app-server `error`
- `warning` / `configWarning`
- `contextCompaction` item lifecycle

Normalizer output must preserve:

- final assistant message extraction
- reasoning/thinking visibility
- command parameter/details sections
- token usage section
- tool/file-change messages where currently persisted
- bounded output limits
- structured failure message extraction

### 7. Workbench Event Publishing

The app-server live runner must publish the same Workbench events as the current runtime manager:

1. `SessionStatus(Starting)` with `latestUserPrompt`.
2. `SessionStatus(Running)` with `latestUserPrompt`.
3. Incremental `Output` chunks with response id and sequence.
4. Final `Output { done: true }`.
5. `SessionMetadata`.
6. Terminal `SessionStatus(Completed | Failed | Aborted)`.
7. `ActiveSessions`.
8. `Error` for failures that should be user-visible.

The ordering must match current web/mobile expectations.

### 8. Persistence

1. Append the user message exactly as today before provider start.
2. Persist assistant message with the same metadata fields:
   - `cli`
   - `durableRunId`
   - `model`
   - `runtime`
   - `effort`
   - `mode`
   - `thinking`
   - `fast`
   - `receivedAt`
   - `sentAt`
   - `elapsedMs`
   - `status`
3. Update durable run status on complete/fail/abort.
4. Update chat run attempt status and token usage.
5. Store app-server native thread id on:
   - session summary
   - durable run
   - chat run attempt
6. Do not call `sync_codex_turn_to_native_rollout` for app-server turns, because app-server writes the native rollout itself.
7. Keep existing rollout sync for the old `codex exec --json` fallback path.

### 9. Token Usage

1. Read token usage from app-server `thread/tokenUsage/updated` and/or final turn data.
2. Convert it into existing `SessionTokenUsage` / `SessionLifetimeTokenUsage` shapes.
3. Review storage filters that exclude `source = 'codex_app_server'`.
4. Do not reuse `codex_app_server` source label for both compaction-only attempts and normal live turns if that breaks usage accounting.
5. Add tests for:
   - normal completed app-server turn usage
   - compacted-session usage after app-server turn
   - missing/partial usage

### 10. Abort

1. Store active app-server `threadId` and `turnId` in the runtime record.
2. On Workbench `abort_session`, send `turn/interrupt`.
3. Wait for `turn/completed` with interrupted status.
4. If app-server does not respond in time, kill the child process as fallback.
5. Publish Workbench aborted status exactly like current runtime.
6. Persist aborted durable run and attempt.

### 11. Approval And Server Requests

1. Detect server-initiated JSON-RPC requests, including:
   - `item/commandExecution/requestApproval`
   - `item/fileChange/requestApproval`
   - `item/permissions/requestApproval`
   - `item/tool/requestUserInput`
   - `mcpServer/elicitation/request`
   - `account/chatgptAuthTokens/refresh`
2. First migration should avoid hanging:
   - prefer `approvalPolicy: "never"` where compatible with selected mode, or
   - auto-decline/cancel requests with a clear Workbench error/status.
3. Do not silently wait forever for approval UI that does not exist.
4. If selected Workbench mode requires approval behavior that app-server cannot handle backend-only, fail clearly and fall back to old runtime if possible.
5. Add tests for a fake server request to confirm the turn does not hang.

### 12. Mode And Sandbox Mapping

Map current Workbench modes to app-server settings conservatively:

- `bypass`: app-server dangerous/full-access equivalent only when user selected bypass.
- `accept-edits`: workspace-write style behavior.
- `plan`: read-only style behavior.
- `read-only`: read-only style behavior.
- `default`: app-server/Codex configured defaults.

Preserve current Codex CLI behavior as closely as possible:

- `--skip-git-repo-check`
- model override
- reasoning effort
- fast mode/service tier
- IO Gateway provider overrides when runtime is `io_gateway`

### 13. IO Gateway Compatibility

1. Preserve existing `io_gateway` user setting behavior.
2. If app-server is used for IO Gateway-backed Codex, pass the same provider/base URL/API key config currently injected into Codex CLI.
3. Keep `IO_WORKBENCH_GATEWAY_KEY_ENV` handling.
4. Ensure app-server compaction launch options and live-turn launch options do not conflict.
5. If IO Gateway app-server live turns are not stable in the first pass, gate them separately and keep IO Gateway on the existing CLI path.

### 14. Compact And Retry

1. Keep existing manual compaction path working.
2. Preserve recent fixes that allow compact without failing when the failed message id is missing.
3. Ensure app-server live turns do not break:
   - manual compact
   - compact and retry
   - context rollover activation
   - context rollover follow-up run
4. Verify compaction attempts are still marked with the correct source.
5. Verify the clean context native thread id is activated on the Workbench session.

### 15. External Codex Session Import

1. Review `external_sessions.rs` path resolution.
2. Support Codex homes where `CODEX_HOME` points directly to a Codex directory containing `sessions`.
3. Keep support for the current default `~/.codex/sessions`.
4. Ensure app-server-created rollouts can be discovered/imported.
5. Ensure Workbench-owned native sessions are not duplicated as separate external sessions.

### 16. Agentic Board

1. Keep board-owned sessions isolated from ordinary chat lists.
2. Preserve board subscriptions and event replay filtering.
3. Ensure board Codex provider turns can use the internal app-server path.
4. Preserve board-selected controls:
   - model
   - effort
   - thinking
   - fast
   - `mode = bypass`
5. Verify board task/session linking still works if provider start fails after session allocation.

### 17. Desktop Native Client

No desktop protocol migration in the first pass.

Backend must keep desktop-compatible Workbench events:

- `session_status`
- `session_metadata`
- `output`
- `error`

Track separately:

- Desktop image upload currently appears to consume `image.data` while server upload returns `image.path`.
- Do not make app-server image support depend on desktop sending web/mobile marker lines until desktop is fixed.

### 18. Mobile Client Compatibility

No mobile protocol migration in the first pass.

Backend must avoid:

- new `ChatRuntime` enum values in session summaries
- new required `StartSession` fields
- changed `Output` shape
- changed `SessionMetadata` shape

Track separately:

- Mobile currently keeps `thinking` in UI state, but its serialized `StartSession` model does not include `thinking`. This is a separate compatibility bug, not required for initial app-server migration.

### 19. Web Client Compatibility

No web protocol migration in the first pass.

Backend must preserve:

- markdown section headings parsed by web renderer
- token usage footer behavior
- recovery and manual compaction cards
- ordered response id / sequence handling
- image attachment marker convention

### 20. Tests To Add

Add Rust tests for:

1. App-server initialize/start/resume/turn JSON-RPC sequence using a fake process.
2. App-server notification demux.
3. App-server server-request handling does not hang.
4. Normalizer maps `agentMessage` final answer into `codex`.
5. Normalizer maps reasoning into `thinking`.
6. Normalizer maps command execution into `exec / Parameters` and `exec / Details`.
7. Normalizer maps command output deltas.
8. Normalizer maps file changes or diffs into visible sections.
9. Normalizer extracts final assistant message.
10. Normalizer extracts token usage.
11. Normalizer extracts error and HTTP status details.
12. Image marker parsing and absolute `localImage` conversion.
13. Abort sends `turn/interrupt`.
14. Native thread id is persisted.
15. App-server turns do not call manual rollout append.
16. Existing compaction tests still pass.
17. Existing durable recovery tests still pass.
18. Existing external session import tests still pass.
19. Existing board session behavior still passes.

### 21. Manual Verification

Before updating the running app, manually test:

1. Start a new Codex chat.
2. Continue the same Codex chat.
3. Reload/reconnect while a response is streaming.
4. Abort an active turn.
5. Upload one image and ask about it.
6. Upload multiple images and ask about them.
7. Start a text-only prompt after image prompt.
8. Compact a chat.
9. Compact and retry a failed chat.
10. Run an agentic board Codex task.
11. Check that the session appears in Codex CLI history/session storage.
12. Confirm old `codex exec --json` fallback still works with the feature flag disabled.
13. Confirm web UI formatting still collapses tool/thinking sections.
14. Confirm mobile chat streaming still works.
15. Confirm desktop chat streaming still works if desktop is in scope for the manual run.

### 22. Rollout Criteria

Only update the app running on this machine after:

1. Rust tests pass for changed crates.
2. Existing compact/retry tests pass.
3. Manual smoke test passes with app-server flag on.
4. Fallback smoke test passes with app-server flag off.
5. No client protocol shape changes are introduced.
6. No new mobile enum value is persisted.
7. Existing dirty user changes are preserved.

### 23. Rollback Plan

If app-server live turns break:

1. Disable the internal app-server feature flag.
2. Restart the Workbench server.
3. Verify Codex returns to `codex exec --json`.
4. Keep any app-server-created native sessions in storage; they should remain normal Codex native thread ids.
5. Do not delete user chat messages or native Codex rollout files.

## Open Questions Before Coding

1. Should app-server be enabled only for `native_cli` first, leaving `io_gateway` on the old path until separately verified?
2. What exact feature flag name should be used?
3. Should approval requests initially auto-decline, fail the turn, or force fallback to old CLI path?
4. Should desktop image upload mismatch be fixed before or after app-server image support?
5. Should `CODEX_HOME` be explicitly configurable through Workbench, or should import just auto-detect both `CODEX_HOME` and `~/.codex` shapes?

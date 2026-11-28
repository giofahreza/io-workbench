# Kanban and Agentic Removal Record

Status: complete. The old Kanban Board, the native desktop app, and the RAG subsystem are removed
from the server, the browser client, and the mobile clients. The replacement Board that this
document originally planned was started and then removed at the operator's direction: the product
now focuses on chat sessions, so no Board replacement is being built.

This file is the record of that removal: what was removed, how it was verified, and what was
deliberately kept.

## Direction change

The original plan (requirements R01-R17, implementation work P1-P7, acceptance tests T01-T70)
described a ticket-driven agentic workflow with its own Board. That work is cancelled. What
survives is the removal-only milestone, which is complete.

- The replacement was briefly started as a separate `iowb-board` crate with its own records,
  persistence, rules, and HTTP surface. It was removed again rather than left half-built.
- The retired engine, its routes, its clients, the desktop app, and RAG are gone. No flag,
  fallback, compatibility adapter, or legacy endpoint remains.
- Ordinary chat sessions, provider runtimes, authentication, the terminal, the database workspace,
  Git services, and shared storage are untouched.

## Delivery status

- [x] Existing Kanban Board implementation completely removed.
- [x] Native desktop app and its packaging/documentation completely removed.
- [x] RAG implementation, plugin API, dependencies, and integrations removed.
- [x] Old Board data and snapshot directories deleted from the existing installations.
- [x] Browser and mobile Board surfaces removed.
- [x] Affected product documentation reconciled with the implemented behavior.
- [x] Removal verified by the checks recorded in section 14.

## 10. Legacy Removal Inventory and Shared Boundaries

Static research baseline: November 24, 2026. The Board-specific inventory contains
122 whole-file deletion candidates plus six obsolete documents/assets. The newly
approved full desktop/RAG retirement expands that scope; 122 is not the combined
removal total. Counts describe the inspected checkout, not executed deletions.

Existing Board code is removal inventory, not a foundation to keep running.
Shared services remain dependencies only where they support retained features.
"Danger" is the old Board API/client name, not a second independent engine:
`crates/iowb-server/src/agentic_board.rs` registers `/api/danger/boards...`, and
desktop `RunDanger`/`DangerBoards` call those same endpoints.

### 10.1 Existing behavior to replace

| Area | Existing Board behavior | Removal/replacement requirement |
| --- | --- | --- |
| Hierarchy | Fixed Initiative -> Epic -> Story -> Task -> Subtask execution. | Remove old models/normalization/execution assumptions and implement root Epic/Task plus internal steps afresh. |
| Approval and scheduling | Todo mutations can activate Board-wide processing. | Remove old edit/start handlers and scheduling loops; implement revision-bound root queuing and serial ownership. |
| Discussion | Board proposal sessions use Board-specific provider wrappers. | Remove that discussion/proposal path; connect fresh ticket routing to shared chat/session/provider services with explicit modes. |
| Scope persistence | Legacy sanitizers remove retired requirement-related fields. | Remove the old Board serializer, normalizer, and snapshot discovery; write the new format in its own namespace. |
| Phase history | Board-specific task/session pointers and transcript management. | Remove old Board bindings and implement distinct new phase/attempt references using shared message storage. |
| Verification | Old Board gates/discovery can report generic success or skipped validation. | Replace Board-only validators with explicit required-case evidence gates; do not inherit empty-pass behavior. |
| Recovery and ownership | Board startup recovery, project locks, and current-run pointers. | Remove old Board startup hooks; implement new root/phase recovery using generic durable-run and process-control primitives. |
| Clients | Old Board views, editing flows, contracts, and controllers. | Remove old Board-specific code on maintained clients and wire the replacement to the new contract; retain shared chat/rendering components. |

### 10.2 Dedicated Board code: delete whole files/directories

Delete these together with their imports, registrations, and callers. The file
patterns below identify verified current matches, not commands to blindly apply
to future replacement files.

| Area | File/directory to delete | Current files |
| --- | --- | --- |
| Backend entry point | `crates/iowb-server/src/agentic_board.rs` | 1 |
| Backend engine and tests | `crates/iowb-server/src/agentic_board/` in its entirety, including discussion, hierarchy, HTTP, orchestration, execution, validation, persistence, and recovery | 80 |
| Server RAG bridge | `crates/iowb-server/src/rag_client.rs`; its only production consumer is the old Board | 1 |
| Browser Board code | `crates/iowb-ui/static/app/board.js` | 1 |
| Browser Board styles | `crates/iowb-ui/static/styles/board.css` | 1 |
| Android details | `apps/mobile/androidApp/src/main/kotlin/io/workbench/mobile/WorkbenchKanbanDetailDialogs.kt` | 1 |
| Android dialogs | `apps/mobile/androidApp/src/main/kotlin/io/workbench/mobile/ui/board-dialogs.kt` | 1 |
| Android task dialogs | `apps/mobile/androidApp/src/main/kotlin/io/workbench/mobile/ui/board-task-dialogs.kt` | 1 |
| Android delete confirmation | `apps/mobile/androidApp/src/main/kotlin/io/workbench/mobile/ui/kanban-delete-confirmation.kt` | 1 |
| Android numeric input | `apps/mobile/androidApp/src/main/kotlin/io/workbench/mobile/ui/kanban-numeric-input.kt` | 1 |
| Android tap guard | `apps/mobile/androidApp/src/main/kotlin/io/workbench/mobile/ui/kanban-tap-guard.kt` | 1 |
| Shared Kotlin Board helpers | `apps/mobile/shared/src/commonMain/kotlin/io/workbench/mobile/shared/Kanban*.kt` | 9 |
| Shared Kotlin quality-save controller | `apps/mobile/shared/src/commonMain/kotlin/io/workbench/mobile/shared/controller/kanban-quality-save.kt` | 1 |
| Shared Kotlin Board tests | `apps/mobile/shared/src/commonTest/kotlin/io/workbench/mobile/shared/Kanban*Test.kt` | 21 |
| Kotlin web-harness Board actions | `apps/mobile/webHarness/src/jsMain/kotlin/io/workbench/mobile/web/board-actions.kt` | 1 |
| Total | Dedicated Board implementation/test files, including its server RAG bridge | 122 |

The nine shared helper matches are `KanbanBoardViewport.kt`,
`KanbanConfiguration.kt`, `KanbanDeleteSelection.kt`, `KanbanNumericSettings.kt`,
`KanbanQualitySave.kt`, `KanbanSemantics.kt`, `KanbanSessionPolicy.kt`,
`KanbanStatusPresentation.kt`, and `KanbanTapGesture.kt`. Keep unrelated files in
the enclosing source/test directories.

### 10.3 Expanded retirement: delete the entire desktop app and RAG

This supersedes the earlier suggestion to retain the desktop API client and RAG
plugin API. Both products are now removal scope, not preserved shared services.

| Target | Required removal |
| --- | --- |
| `apps/desktop/` | Delete the entire native desktop app: all 30 currently tracked files, C++ sources, CMake targets, README files, launcher, `desktop-package.json`, and `package-release.sh`. Do not extract its unrelated desktop features for continued support. |
| `rag/` | Delete the entire RAG submodule/worktree from the product checkout and its tracked gitlink. Its 12 currently tracked files include indexing, retrieval, embeddings, BM25, vector-store, runtime, configuration, and README/manifest. Remove its `.gitmodules` entry; do not leave an empty required workspace member. |
| `crates/iowb-rag-plugin-api/` | Delete the dedicated plugin API crate, including its manifest and ABI/protocol definitions. It is no longer needed once both RAG implementation and server bridge are removed. |
| `crates/iowb-ui/static/docs/desktop-client/` | Remove the retired generated desktop guide after updating the documentation generator. |
| `crates/iowb-ui/static/docs/topic/native-desktop-client/` | Remove the retired generated desktop topic page and its navigation/search/category links. |

The server RAG bridge is already counted in section 10.2; do not count it twice.
Remove desktop files inside the `apps` repository/submodule, but retain that
submodule, its mobile clients, and unrelated app docs. Remove only the `rag`
submodule registration; do not delete the root `.gitmodules` file while `apps`
still needs it, remove unrelated Git metadata, or rewrite repository history.

Complete the associated integration cleanup:

- `Cargo.toml`: remove `rag` and `crates/iowb-rag-plugin-api` from workspace
  membership, remove the plugin API workspace dependency, and remove `libloading`
  if no retained consumer needs it. Regenerate `Cargo.lock` for the retained
  workspace; do not delete the lockfile or shared dependencies indiscriminately.
- `crates/iowb-server/Cargo.toml` and `crates/iowb-server/src/lib.rs`: remove the
  plugin API/loader dependencies and `rag_client` module registration together
  with the old Board registration.
- Remove RAG-only indexing, retrieval, promotion, ingestion, and dynamic-plugin
  loading contracts. Remove application handling/documentation of
  `IO_WORKBENCH_RAG_MODE`, `IO_WORKBENCH_RAG_PLUGIN`, and
  `IO_WORKBENCH_RAG_PLUGIN_PATH`; do not replace them with an "off" compatibility
  mode or require a plugin for the replacement.
- `scripts/install-termux.sh`: remove the RAG-only workspace-member pruning,
  temporary-manifest/lock backup and restoration workaround, RAG messages, and
  `IO_WORKBENCH_RAG_MODE=off` export. Preserve the installer and unrelated install,
  recovery, and provider setup behavior; source builds should use the real
  retained workspace directly.
- Remove desktop-only build-output rules from `apps/.gitignore` and audit build,
  install, packaging, release, and publishing scripts for retired targets/assets.
  Do not remove server/CLI release builds or mobile packaging.
- Update `README.md`, `scripts/generate-docs.mjs`, documentation navigation,
  categories, search indexes, and cross-links to stop offering a native desktop
  app or RAG integration. Regenerate affected docs and remove stale generated
  desktop pages; regeneration must not leave the old pages accessible.
- Reconcile mixed architecture/migration docs, including
  `docs/architecture/rust-rewrite-plan.md` and
  `docs/migrations/codex-app-server.md`, without deleting their unrelated session
  and provider contracts or presenting retired desktop requirements as current.

Removing RAG source is not permission to destroy external Qdrant instances,
shared model caches, or unrelated data. Any application-owned RAG artifact reset
must identify its exact ownership and scope. No replacement retrieval platform
is required for the MVP.

### 10.4 Mixed files: extract or selectively remove first

These files are not unconditional whole-file deletions and are outside the
122-file count.

| File | Shared functionality to preserve |
| --- | --- |
| `apps/mobile/androidApp/src/main/kotlin/io/workbench/mobile/ui/board-screen.kt:266` | Contains `buildTerminalScreen` and, at line 347, `updateTerminalScreen`, called by `ui/lifecycle-and-shell.kt`. Extract Terminal UI before deleting the remaining old Board file, or remove only its Board sections. |
| `apps/mobile/webHarness/src/jsMain/kotlin/io/workbench/mobile/web/render-board-and-config.kt:757` | `navButton` begins shared navigation/settings content; the file also contains server metrics, configuration rendering, and JSON helpers. Extract those before deleting the Board renderer, or remove only Board sections. |
| `crates/iowb-storage/src/storage/tests/board_session_backfill.rs:1` | Contains session-summary identity/fork/timestamp and first-user-message ordering/role/content coverage alongside Board-specific tests. Relocate still-relevant shared assertions. Remove helpers/tests that are genuinely unused after retirement rather than preserving obsolete backfill behavior. |

### 10.5 Shared files: remove Board hooks, not retained functionality

Deleting the dedicated files alone is incomplete. Clean up at least these
integration areas; audit remaining callers rather than treating this as an
exhaustive list of references.

| Area | Files/boundaries to edit, not delete wholesale |
| --- | --- |
| Server startup/routing | `crates/iowb-server/src/lib.rs`, `crates/iowb-server/src/server/preamble.rs`, and `crates/iowb-server/src/server/recovery_and_routing.rs`: remove Board module/router bindings, legacy-session backfill, worker recovery, and poller startup. |
| Server shared handlers | `crates/iowb-server/src/server/processes_and_websocket.rs`, `crates/iowb-server/src/server/settings.rs`, and `crates/iowb-server/src/server/notifications_and_watch.rs`: remove Board-only subscriptions, session exceptions, and notification filters. Preserve generic WebSocket, settings, and notifications. |
| Core sessions | `crates/iowb-core/src/core/app_state/initialization.rs`, `crates/iowb-core/src/core/app_state/agent_sessions.rs`, and `crates/iowb-core/src/core/sessions/`: remove Board session creation, ownership caches, metadata, and filtering; preserve generic session/provider lifecycle and recovery. |
| Protocol/storage | `crates/iowb-protocol/src/protocol/foundations.rs`, `crates/iowb-protocol/src/protocol/sessions.rs`, and `crates/iowb-storage/src/storage/`: remove Board fields, metadata serialization, query/search exclusions, and obsolete test includes. Preserve shared session/message/durable-run storage and relevant regression tests. |
| Browser shell/assets | `crates/iowb-ui/src/lib.rs`, `crates/iowb-ui/static/index.html`, `crates/iowb-ui/static/app.js`, `crates/iowb-ui/static/styles.css`, `crates/iowb-ui/static/sw.js`, and `crates/iowb-ui/static/openapi.json`: remove old Board asset routes, embedded tests, navigation, HTML, imports, precaching, and schemas. |
| Browser shared scripts | Remove Board state/callers from navigation, startup, commands, core/sidebar/forms, shared chat scripts, and workspace files/Git/settings/WebSocket handlers under `crates/iowb-ui/static/app/`. Keep ordinary chat and workspace behavior. |
| Kotlin shared clients/controllers | Edit `IowbClient.kt`, `MobileController.kt`, `Models.kt`, and shared connection/chat/cache/files/Git/terminal/settings controllers under `apps/mobile/shared/`; remove Board API methods, models, state, and handlers only. |
| Android shell | Edit `MainActivity.kt`, numeric inputs, and navigation/keyboard/settings/chat files under `apps/mobile/androidApp/`; remove Board state/imports/actions, not the app or shared chat/Terminal UI. |
| Kotlin web-harness shell | Edit `Main.kt`, lifecycle/navigation/events, input/storage, chat rendering, and project editor files under `apps/mobile/webHarness/`; remove Board bindings while retaining the harness and shared helpers. |
| Shared tests | Remove Board-only cases from mixed core/server/protocol/storage/UI test files. Keep ordinary streaming, recovery, storage, clipboard, and chat regression coverage. |

Do not keyword-delete shared code. In particular:

- Ordinary-chat bypass mode accepts `danger` as an alias in
  `crates/iowb-core/src/core/agent_commands/launch_arguments.rs:250` and
  `crates/iowb-protocol/src/protocol/foundations.rs:366`; bypass support remains
  required for the new execution phases.
- Mobile `palette.danger` is a general destructive/error color, not the retired
  automation feature.
- Vendored Mermaid may include Kanban support; preserve the shared renderer used
  by ordinary chat and the replacement process diagram.
- "Desktop Web" layouts, browser keyboard bindings, native-provider session IDs,
  and Rust CLI/TUI are not the retired native desktop application.
- A helper named `list_sessions_including_board` also serves generic startup and
  recovery. Refactor its retired filtering semantics without deleting the needed
  generic session-list/recovery capability.
- Keep generic plugin/MCP/tool infrastructure where retained features use it;
  remove the dedicated RAG plugin API, not every plugin facility by name.

### 10.6 Obsolete Board documents and assets: six candidates

- `docs/design/kanban.md`
- `docs/kanban-flow.md`
- `kanban-test.md`
- `kanban-test-results.md`
- `crates/iowb-ui/static/images/landing/agentic-board-dark-20260907.webp`
- `crates/iowb-ui/static/images/landing/agentic-board-dark-mobile-20260907.webp`

Delete these as obsolete old-Board material and remove their inbound references.
Update asset bindings, tests, and publishing checks in
`crates/iowb-ui/src/lib.rs`, `scripts/serve-landing.mjs`, and
`.github/workflows/pages.yml`, plus affected READMEs and landing copy. Keep
`revamp-kanban-flow.md` as the replacement requirements baseline.

The generated "Boards and tools" documentation also covers unrelated MCP,
plugins, and commands. Update its source in `scripts/generate-docs.mjs` and
regenerate; do not delete that entire mixed page or the documentation generator.
Historical docs/test reports are not normative where they conflict with the new
hierarchy, chat-only edits, approval, or retirement decisions.

### 10.7 Removal-only milestone and runtime/data safety

Deleting source does not stop persisted agents. Generic recovery in
`crates/iowb-server/src/server/recovery_and_routing.rs:131` synthesizes durable
runs from active sessions, including Board sessions, and
`crates/iowb-core/src/core/sessions/lifecycle.rs:6` restores persisted sessions.
Removing ownership markers first can allow old Board sessions to look ordinary
or resume through generic recovery.

Perform P0 in this order before implementing the replacement:

1. Identify Board-owned workers, provider sessions, and durable runs while their
   ownership metadata still exists. Stop execution and prevent restart recovery
   from scheduling them.
2. Retire the identified Board-owned active records before removing ownership
   fields/hooks. Any retained old records must stay non-runnable and out of
   ordinary-chat discovery. Do not deactivate or erase unrelated user sessions.
3. Extract the retained Terminal/settings/navigation/test functionality from
   mixed files described above.
4. Delete dedicated Board code and the entire desktop/RAG implementations, then
   remove their shared hooks, manifests, submodule bindings, dependencies,
   installer workarounds, generated pages, and asset/doc references.
5. If resetting old snapshots, restrict it to the resolved
   `<config_dir>/agentic-boards/` directory and explicitly identified Board-owned
   records. Do not wipe the shared database, project files, credentials, Git
   repositories, or external/shared RAG data. Old snapshots may instead remain
   inert and undiscovered; the replacement uses its own namespace.
6. Verify the removal-only build, retired-route/symbol audit, installer/build
   paths, retained-client/shared-feature regressions, and no old-run resumption.
   Record results before marking P0 complete.
7. Only after that checkpoint begin the replacement. Do not preserve disabled
   engines, empty required submodules, hidden desktop distribution paths, or a
   RAG "off" adapter as temporary production fallbacks.

Breaking pre-alpha APIs and ignoring old records is acceptable; preserving
unrelated user data and the replacement's future run history is still required.

## 14. P0 Execution Record

Recorded November 27-28, 2026 against the uncommitted working tree of `main`. This is evidence for
the removal only. The replacement Board (the former P1-P7 work) was started and then removed, so
its acceptance cases no longer apply; the cases below cover the delivered state.

### 14.1 Inventory validated against the checkout

The section 10 baseline was a static estimate. Execution found these differences:

- Native desktop: `apps/desktop/` and its generated pages were already gone (`48ce6a4`,
  `a31642b`), and `apps/.gitignore` already had no desktop-only rules, so retirement was an audit
  (T67), not a deletion.
- Backend and RAG matched the baseline: 80 files under `agentic_board/`, `agentic_board.rs`,
  `rag_client.rs`, the 2-file `iowb-rag-plugin-api` crate, and the `rag/` submodule.
- Kotlin had Board-owned files the baseline missed: `BoardConfiguration.kt`,
  `BoardConfigurationEditTest.kt`, `MobileControllerBoardCreationRecoveryTest.kt`,
  `MobileControllerBoardRefreshTest.kt`, `MobileControllerEmptyBoardTest.kt` (pending: confirm
  against the final Kotlin audit).
- `WorkbenchNumericInputs.kt`, listed in 10.5 as a file to edit, held only
  `validateKanbanNumericInputs` and was called only from Board dialogs, so it was deleted whole.
- Hooks beyond the 10.5 list: the four `agentic*` notification defaults in
  `server/configuration.rs` (also rendered by the Android settings screen), the sidebar pin filter
  in `server/settings.rs`, WebSocket `Subscribe.sessionIds` (protocol, TUI, clients), the `rag/`
  source root in `scripts/quality/check_line_limits.sh`, and the docs generator's "Boards and
  tools" page, now `tools` (slug and title).
- Board-backfill-only helpers had no retained caller and were removed with their tests
  (`Storage::first_user_message`, `Storage::list_session_summaries_for_project`). Their still-relevant
  fork and timestamp assertions now cover `list_sessions_for_project`.
- Unrelated and left alone: `scripts/termux-source-overlay.patch` is opt-in
  (`IO_WORKBENCH_TERMUX_SOURCE_OVERLAY`) and 96% of its added lines are already in `main`;
  `scripts/quality/check_line_limits.sh` is not wired into CI and flags 8 files over 2000 lines that
  this work did not touch; clippy has existing warnings. `cargo fmt --check` was failing at HEAD on
  one import order in `codex_app_server/mod.rs` and was fixed.

### 14.2 Decision: delete the old Board data, keep no migration shim

An earlier pass retired the old Board chats in place through a `retired_sessions` table and a
startup cleanup, so that no old record could resurface as an ordinary chat. The operator chose
outright deletion instead ("just delete old related file and code"), which also keeps the retired
code out of the delivered app. The shim was therefore removed again: there is no `retired_sessions`
table, no retirement function, no startup cleanup and no schema bump (`schema_version` stays 7).

The old data was deleted from the two real installations, scoped strictly to what the Board owned
and identified by the same metadata predicate as before:

| Database | Board sessions deleted | Messages | Durable runs | Attempts | Ordinary sessions kept |
| --- | --- | --- | --- | --- | --- |
| `~/.io-workbench/io-workbench.db` (1.7 GB) | 21 | 209 | 25 | 19 | 155 |
| `~/.iowb/io-workbench.db` (4.2 GB, live instance) | 104 | 1,430 | 205 | 201 | 234 |

Both databases were backed up first (`*.before-board-cleanup-20261128-003607.bak`). The deletion
removed the Board-owned sessions and the rows referencing them (`messages`, `durable_chat_runs`,
`chat_run_attempts`); no ordinary session, message, credential, project or setting was touched.
The old snapshot directories (`agentic-boards`, `agentic-runs`), the empty `rag-edge` directories and
the stale `iowb-rag-*.log` files were deleted from both installations. No replacement code reads
any of them.

A read-only check before the deletion confirmed the predicate matched an independent
reimplementation of the removed Rust semantics exactly (0 mismatches either way, 0 look-alike rows)
and that the selection took 1 ms over 338 sessions.

### 14.3 Results by test ID

| ID | Result | Command and evidence |
| --- | --- | --- |
| T61 | Passed | Classified grep over the whole tree (excluding vendor, build output and this record): no Board or agentic implementation, module, route, model, or test remains in the server, browser, or mobile clients; unrelated words (onboarding, clipboard, keyboard) are intact; no feature flag, fallback or compatibility path remains. |
| T62 | Passed | Scratch server, auth off, temp dirs: 14 former Board method/path combinations (GET, POST, PATCH, PUT, DELETE, including retired mutation payloads) all return `404 {"error":"route not found"}`; control `/api/auth/status` returns 200. |
| T63 | Passed | The old Board records were deleted outright (section 14.2) instead of being retired, so there is no ownership marker left for generic recovery to act on: the retained code contains no Board-specific session, run or ownership logic at all. The retired-symbol audit finds no Board branch in session listing, search, recovery or startup. The old snapshot directories are deleted from both installations. |
| T65 | Passed | `cargo test --workspace --offline`: 728 passed, 0 failed (baseline before removal: 1,145 passed plus 1 failure caused by the RAG dependency tree, which no longer occurs). Server tests 721 to 287 = 432 deleted Board-module tests plus 2 removed Board-only server tests. Browser: `cargo test -p iowb-ui` 24 passed (3 Board-only embedded tests removed, one renamed to keep its snapshot assertions); 103/103 Playwright tests pass, unchanged from baseline; every changed script parses. Kotlin: `:shared:jvmTest` 534 tests, 0 failures (baseline 822; 288 Board-only tests removed), `:androidApp:compileDebugKotlin`, `:androidApp:testDebugUnitTest` and `:androidApp:assembleDebug` all succeed. Shared messages, credentials, project files and unrelated configuration were not touched by the cleanup. |
| T67 | Passed | No desktop artifact names anywhere, `apps/desktop` has no tracked files, no desktop pages or links; the only matches are accurate text ("not a notarized desktop application bundle") and the Android NDK `CMAKE_TOOLCHAIN_FILE`. Generated docs were regenerated after the "Boards and tools" page became "Tools": the old slug, its directory and every link to it are gone, `node scripts/generate-docs.mjs --check` passes, and no generated page mentions the Board. The retired URL now returns the same generic application shell as any unknown path (byte-identical to `/docs/this-page-never-existed/`), so the removed page is unreachable. |
| T68 | Passed | Scratch server with `IO_WORKBENCH_RAG_MODE=on`, `IO_WORKBENCH_RAG_PLUGIN` and `IO_WORKBENCH_RAG_PLUGIN_PATH` pointing at nothing: starts normally, no plugin or RAG activity in the log. `rag/`, the plugin API crate, `rag_client.rs` and `libloading` are gone; `Cargo.lock` 646 to 484 packages. |
| T69 | Passed | Clean copy of the retained workspace (no `rag/`, no `apps/`, working-tree sources with the UI crate from HEAD): `cargo check --workspace --all-targets --locked --offline` exits 0 with no lock complaints. In the working tree the same check passes and the workspace members are exactly the nine retained crates; `sh -n scripts/install-termux.sh` passes and the pruning, backup, restore and `IO_WORKBENCH_RAG_MODE` pieces are gone (-117 lines); `node --test scripts/tests/install-workspace.test.mjs` 4/4. |
| T70 | Passed | The Android Terminal builders and updaters moved out of the deleted Board screen file into `ui/terminal-screen.kt` (both `MainActivity` extension functions, still called from `ui/lifecycle-and-shell.kt`); the web harness's shared navigation, settings, metrics and JSON helpers moved into `web/render-config-and-metrics.kt`. Both extracted files compile, `:androidApp:assembleDebug` succeeds, and the shared test suite is green. |
| T64 | Passed | The replacement's `kanban/` namespace was removed with it, so no Board snapshot can be discovered, normalized, or executed; the only snapshot namespace that remains is the shared SQLite database for chat. |
| T66 | Not applicable | This case tested the replacement Board end to end. The replacement was cancelled, so the delivered app is the retained chat application with no Board, no desktop client, and no RAG service. |

### 14.4 The replacement was started, then removed

The replacement Board was briefly implemented as a separate `crates/iowb-board` crate holding
canonical work item, plan, run, attempt, and event records, their JSON persistence under
`<config_dir>/kanban/`, the approval and status rules, and a small HTTP surface under
`/api/board/items`. It carried fourteen crate tests and two server tests, and the full workspace
suited 744 tests with zero failures.

It was then removed in full at the operator's direction, because the product direction changed to
focus on chat sessions instead of an agentic workflow. The crate, the server module, its tests, the
workspace membership, the crate dependency, the module registration, the route merge, and the
generated snapshot namespace were all deleted. No Board replacement code remains, and no Board
route or record type is served. The removal checks in section 14.3 cover the delivered state.

### 14.5 The replacement was removed again, and the final state verified

After the direction change, every artifact of the replacement was deleted: the `crates/iowb-board`
crate, `crates/iowb-server/src/board.rs` and its tests, the workspace membership and crate
dependency, the module registration, the `/api/board/items` route merge, and the `kanban/`
snapshot namespace. The workspace is back to its nine original crates.

Final state, all verified:

| Check | Result |
| --- | --- |
| `cargo test --workspace --offline` | 728 passed, 0 failed |
| `cargo fmt --all --check` | passes |
| Browser: `cargo test -p iowb-ui` | 24 passed, 0 failed |
| Browser: Playwright suite | 103 tests, 103 pass, 0 fail |
| Kotlin: `:shared:jvmTest` | 66 classes, 534 tests, 0 failures |
| Kotlin: `mobileCheck`, `:androidApp:compileDebugKotlin`, `:androidApp:testDebugUnitTest`, `:androidApp:assembleDebug` | all succeed |
| `node scripts/generate-docs.mjs --check` | generated docs are up to date |
| `node --test scripts/tests/install-workspace.test.mjs` | 4 passed |
| `sh -n scripts/install-termux.sh` | passes |
| Retired routes (14 method/path combinations) | all `404 route not found` |
| Classification grep over the server, browser, and mobile sources | no Board or agentic implementation, module, route, model, or test remains |

The only surviving mentions of the word are unrelated: `onboarding`, `clipboard`, `keyboard`, the
IO Gateway "dashboard", the vendored Mermaid renderer (which draws diagrams inside ordinary chats),
and the unrelated third-party "Vibe Kanban" research note.

A second, adversarial pass re-checked the result without the keyword filters used by the first
one, and found and fixed one real leftover: `apps/mobile/assets/install-termux.sh`, an unpacked-APK
artifact that still pruned the `rag` workspace member and exported `IO_WORKBENCH_RAG_MODE=off`. It
is gitignored and not read by the build (the APK sources the installer from `scripts/`, and the
assembled APK asset was already byte-identical to the cleaned script), but it was removed anyway.

That pass also confirmed, rather than assumed:

- No code anywhere reads or writes the retired `boardSession`, `boardId`, `boardRunId`, or
  `boardTaskId` metadata keys, and no SQL still branches on them.
- Every path in the section 10.2 inventory is absent from disk.
- The Android Terminal functions extracted out of the deleted Board screen are byte-identical to
  the originals, and all 40 non-Board declarations from the deleted web-harness renderer were
  kept while the 22 Board ones were dropped.
- Four public functions have no call site (`materialize_session_messages`,
  `visit_messages_ordered`, `browse_directories`, `not_implemented`). Each had exactly one
  reference at HEAD, its own definition, and the retired Board never called them: they were
  already dead before this work and are not part of this removal.
- Behaviourally, against a server seeded with a session that still carries the retired Board
  metadata: the session is absent from the listing, `/api/danger/boards` and `/api/board/items`
  both return `404`, a browser with `iowb.lastView=board` stored falls back to the chat view with
  no console error, and the server logs no panic.

### 14.6 Client removals completed

The browser and Kotlin removals were executed and re-verified independently of the work that
produced them. Browser: `board.js`, `board.css` and the two landing images are deleted; the Board
tab, panel, modals, navigation, forms, sidebar, chat, workspace and landing references are gone;
the precache list and `openapi.json` no longer mention them; the workspace shortcut views are now
`files`, `chat`, `git`, and a stored `board` last-view falls back to a retained view. Kotlin: 46
Board-only files are deleted (the only non-Board-named one, `WorkbenchNumericInputs.kt`, held just
`validateKanbanNumericInputs` and was called only from Board dialogs), the Board API methods,
models, state, navigation and the four `agentic*` notification checkboxes are gone, and the two
mixed files were extracted rather than deleted. The only removed keyboard bindings were the Board
column navigation (the `BoardTabs` context with Left/Right/Home/End), which the regenerated
documentation no longer advertises.

Open before P0 can close: nothing is outstanding in the code. The old Board data, snapshot
directories and RAG leftovers were deleted on the operator's instruction, and the retirement shim
that briefly protected them was removed with it, so the delivered app carries no Board-specific
compatibility path. Section 11 P0 stays unchecked until the operator confirms the removal-only
build in use (the live instance still runs the pre-removal release binary until a new release is
published).

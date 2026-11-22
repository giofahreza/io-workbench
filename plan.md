# io-workbench implementation plan

## Existing chats and IO Gateway model selection

Approved direction:

- Add a persistent note beside “Use IO Gateway for chat”: IO Gateway applies to new chats. Existing chats keep the runtime they were created with. Existing native chats may continue using Native CLI, but gateway-prefixed models such as `cod:gpt-6.1-sol` require a new chat.
- Keep existing session runtimes pinned. Do not silently force an old Native CLI session through IO Gateway because its native provider thread may not be resumable through the gateway.
- Add a server-side preflight check before launching the CLI. If an existing `native_cli` session selects a gateway-prefixed model, return a typed `chat_runtime_mismatch` error instead of spawning Codex.
- Scope the error to the affected session and send a request/response identifier so the UI cannot attach a stale error to a later request or another chat.
- Render a chat-local explanation with a “Start new chat” action. Do not globally match raw provider error text.
- Preserve generic error handling for unrelated provider failures.
- Add server and UI tests for runtime mismatch, new-chat gateway routing, native-model compatibility, session scoping, and stale-event suppression.

## First-run provider setup — approved direction

Goal: make a fresh installation usable when Codex and Claude are absent, and make the choice between Native CLI credentials and IO Gateway explicit when the CLIs are installed but not authenticated.

The design should cover these states independently for Codex and Claude:

1. CLI missing.
2. CLI installed and version-detectable, but not authenticated.
3. CLI authenticated and usable through Native CLI.
4. IO Gateway configured and usable.
5. Both Native CLI and IO Gateway usable, with an explicit per-chat route.

Implementation notes:

- Treat provider readiness as capability state, not as a single “installed/not installed” flag.
- Preserve the current architecture constraint in the UX: Codex and Claude IO Gateway sessions still launch the corresponding CLI with gateway overrides. Gateway credentials remove the need for native provider login, but they do not remove the CLI executable prerequisite. A future CLI-independent gateway runtime would be a separate, larger feature because it would also need to replace the CLI’s workspace/tool behavior.
- Keep the app usable without either CLI. The first screen should explain what is unavailable and offer the next action rather than failing when the user opens Chat.
- Do not install provider CLIs, Node.js, npm, or provider credentials from inside Workbench. Workbench should detect the missing dependency and provide only the provider’s official installation URL.
- When a CLI is missing, show a provider-specific setup card that says installation must happen on the server, links to the official installation guide, and offers a “Recheck” action after the user completes installation. If Node.js/npm is missing, provide the official Node.js installation URL instead.
- Keep installation URLs centralized and updateable. Do not embed a shell installer or silently execute a package-manager command from the app.
- When a CLI is installed but native authentication is missing, provide the official provider authentication URL/instructions. Do not launch or collect native credentials from Workbench.
- Make Native CLI and IO Gateway separate, visible choices. Do not silently copy IO Gateway credentials into CLI configuration, and do not require CLI credentials when the user chooses IO Gateway.
- Detect executable availability and version separately from authentication. A missing CLI should show installation guidance; an unauthenticated CLI should show the provider’s login/authentication action; a gateway should show URL/key readiness and a connection/model check.
- Only expose a model in a route’s picker when that route can actually execute it. Keep route-specific model catalogs separate so a gateway model cannot be sent to a ChatGPT-account CLI session.
- Provide a clear “Use Native CLI” / “Use IO Gateway” route status near the chat controls and retain the selected route per new chat. Existing sessions should show their pinned route.
- On first send, fail before spawning a process when the chosen route is unavailable, with a scoped recovery action. Avoid generic “CLI default” entries when no usable CLI or gateway route exists.
- Preserve a no-credentials path for browsing projects, files, Git, terminal, and settings. Agent chat should explain the missing capability without blocking the rest of the workbench.

Before implementation, verify the complete readiness flow for both providers, including fresh-install behavior, CLI login failures, gateway key failures, model discovery failures, switching routes, new sessions, existing sessions, mobile settings, and concurrent/stale WebSocket errors.

## Test cases

These cases cover every requirement above. Run the Codex and Claude cases independently unless a case explicitly names one provider. Run the CLI-install/readiness cases for Gemini as well. Use fake CLIs and a local fake gateway for automated tests; do not use production credentials or paid live requests.

### Existing chats and IO Gateway routing

| ID | Scenario and setup | Action | Expected result |
|---|---|---|---|
| RT-001 | IO Gateway is disabled | Open the IO Gateway settings tab | The checkbox is off and no enabled-state warning is shown. |
| RT-002 | IO Gateway is configured and enabled | Open settings | A persistent note says the route applies to new chats and existing chats keep their original runtime. |
| RT-003 | Gateway is enabled, no current chat | Start a new chat and send a prompt | The session is stored as `io_gateway`; the provider command contains the gateway provider override. |
| RT-004 | Gateway is disabled, no current chat, native CLI ready | Start a new chat and send a prompt | The session is stored as `native_cli`; no gateway override or gateway key is passed. |
| RT-005 | Existing native Codex/Claude chat with a native model | Send another prompt after enabling the gateway globally | The old session remains native and continues using its original route. |
| RT-006 | Existing gateway chat with a gateway model | Send another prompt | The session remains gateway-backed and uses the configured gateway. |
| RT-007 | Existing native session and gateway-prefixed model selected | Send a prompt | The server rejects the request before spawning a CLI and returns `chat_runtime_mismatch`. |
| RT-008 | Same as RT-007 | Inspect the server process list and provider logs | No Codex or Claude process is started and no provider request is sent. |
| RT-009 | Same as RT-007 | Inspect the chat UI | A chat-local explanation recommends starting a new chat and provides a new-chat action. |
| RT-010 | Existing native session with an unprefixed native model | Send a prompt while gateway is enabled globally | No mismatch is reported; the session continues through Native CLI. |
| RT-011 | Existing gateway session after the gateway is disabled | Send a prompt | The session is not silently downgraded to Native CLI; it receives a scoped configuration error with recovery guidance. |
| RT-012 | New chat with a stale gateway model in browser storage but gateway disabled | Open the model picker and send | The stale model is removed or rejected before process launch; it cannot reach the native ChatGPT-account path. |
| RT-013 | Gateway model selected in an old native chat | Click the new-chat action in the scoped error | A fresh pending session is created for the same project, the old transcript is preserved, and the next send uses the gateway route. |
| RT-014 | User changes the global route after an old chat exists | Reload and reopen the old chat | The old session runtime is unchanged; only subsequently created sessions inherit the new route. |
| RT-015 | Codex and Claude chats run concurrently and one fails | Produce a runtime mismatch in only one chat | Only the affected session receives the mismatch card; the other chat continues normally. |
| RT-016 | Error event belongs to another session | Switch chats before the error arrives | The inactive chat’s error is not appended to the selected chat. |
| RT-017 | Two requests are made rapidly in the same session | Deliver the older error after the newer request starts | The older error is ignored using the request/response identifier and cannot overwrite the newer attempt. |
| RT-018 | Error arrives after the user changes project or chat | Deliver the delayed error | The error is discarded unless both session and request identity match the active attempt. |
| RT-019 | WebSocket reconnects while a mismatch is in flight | Reconnect and replay events | The mismatch is delivered at most once, remains scoped, and does not create a duplicate card. |
| RT-020 | A legacy client sends an error event without a request identifier | Deliver the event | The server and UI remain backward-compatible; session filtering still works and no crash occurs. |
| RT-021 | Generic provider failure has text similar to, but not equal to, the mismatch | Trigger the failure | It remains a normal provider error; the runtime-mismatch UI is not shown. |
| RT-022 | Provider emits the exact old ChatGPT-account error text from a later CLI version | Trigger the raw provider error without a server preflight mismatch | The UI does not globally reinterpret the raw text; it remains a normal scoped provider error. |
| RT-023 | Gateway model prefix has leading/trailing whitespace or different case | Select or submit the model | Normalization is consistent, the correct route is selected, and invalid prefixes are not treated as trusted gateway routes. |
| RT-024 | Unknown or malicious model prefix is submitted manually | Send the request | The model is rejected or treated as an unavailable model; it cannot select an arbitrary provider or command. |
| RT-025 | Session metadata is missing a runtime but has a gateway model | Resume the session | The legacy fallback is deterministic and tested; the inferred route is persisted only according to the documented migration rule. |

### Provider readiness and URL-only setup

| ID | Scenario and setup | Action | Expected result |
|---|---|---|---|
| PR-001 | Codex CLI is missing | Select Codex or open the readiness card | The app says Codex is not installed on the server and shows only the official installation URL plus Recheck. |
| PR-002 | Claude CLI is missing | Select Claude or open the readiness card | The app gives the equivalent Claude installation URL and does not attempt installation. |
| PR-003 | Gemini CLI is missing | Select Gemini | The app gives the official Gemini CLI installation URL and does not attempt installation. |
| PR-004 | Node.js/npm is missing | Check a missing provider | The app shows the official Node.js installation URL and does not execute a package manager or shell command. |
| PR-005 | Provider is missing and user taps the installation link on mobile | Open the link | The card clearly says the installation must be performed on the Workbench server, not the phone. |
| PR-006 | User presses Recheck before installing | Press Recheck | Status remains missing, no false success is shown, and no error is attached to chat. |
| PR-007 | User installs the CLI manually while the app is open | Press Recheck | Version detection updates and the provider changes from missing to installed. |
| PR-008 | CLI is installed for the SSH user but not the Workbench service user | Recheck from the running server | The status remains unavailable for the service user and explains the server-user/PATH scope. |
| PR-009 | CLI `--version` exits non-zero | Refresh readiness | The provider is not marked installed; the failure is bounded and actionable. |
| PR-010 | CLI `--version` hangs | Refresh readiness | The check times out, does not block the UI or server, and reports that the CLI could not be verified. |
| PR-011 | CLI is present in a supported user-local path | Refresh readiness | The same path used for actual agent spawning is detected. |
| PR-012 | CLI exists but is not executable | Refresh readiness | The provider is reported unavailable with a repair/install URL. |
| PR-013 | CLI is installed and native login is absent | Choose Native CLI | The app reports “installed, authentication required,” shows official authentication guidance, and does not collect or launch credentials automatically. |
| PR-014 | Auth file exists but is expired, malformed, or rejected by the provider status command | Check Native CLI readiness | Native CLI is not marked usable solely because a file exists. |
| PR-015 | Native authentication status command succeeds | Refresh readiness | Native CLI becomes usable and its models may be exposed. |
| PR-016 | Native authentication status command hangs or fails | Refresh readiness | Native CLI remains unavailable or unknown; the app gives a retry/login action without exposing command output secrets. |
| PR-017 | IO Gateway URL/key are absent | Choose IO Gateway | The route is not ready and the UI points to IO Gateway configuration. |
| PR-018 | IO Gateway URL/key are syntactically configured but unreachable | Load gateway models or test readiness | The route is not marked verified; the timeout/error is visible and the native route is unaffected. |
| PR-019 | IO Gateway returns 401/403 | Load gateway models | The route reports authentication failure without exposing the API key. |
| PR-020 | IO Gateway returns an empty model catalog | Load gateway models | The route reports no models available and does not fall back to unrelated native models. |
| PR-021 | IO Gateway returns malformed JSON or an unexpected schema | Load gateway models | The route reports catalog failure safely and does not crash or display fabricated models. |
| PR-022 | IO Gateway is reachable and returns models, but Codex CLI is missing | Choose Codex through IO Gateway | The UI says the gateway is configured but the required Codex CLI binary is missing; it does not claim Codex chat is ready. |
| PR-023 | IO Gateway is reachable and returns models, but native Codex/Claude login is absent | Choose IO Gateway | The provider is usable through IO Gateway; missing native login is not incorrectly presented as a gateway failure. |
| PR-024 | Gateway API key is configured | Inspect logs, UI, and readiness responses | The key is never rendered, placed in a URL, persisted in browser preferences, or written to ordinary logs. |
| PR-025 | Official installation URL configuration is changed | Refresh the app | New links are used consistently without changing installation logic or executing arbitrary URLs. |
| PR-026 | Official URL is unavailable or malformed | Open setup card | The app shows a safe fallback/manual message and never constructs or executes a shell command from the URL. |
| PR-027 | User has no provider CLI, credentials, or gateway | Open the app | Projects, Files, Git, Database, Terminal, and Settings remain usable; only agent chat shows setup guidance. |
| PR-028 | User has only Codex ready | Open provider picker | Codex is usable and Claude/Gemini are independently marked unavailable; one missing provider does not block the others. |
| PR-029 | User has only Claude ready | Open provider picker | Claude is usable and other unavailable providers remain independently actionable. |
| PR-030 | User has both native providers ready | Open the chat setup | Native route is available and the selected provider/model does not leak settings into the other provider. |
| PR-031 | User has both native and gateway routes ready | Create new chats under each route | Each new session records the selected route and displays it in session metadata. |
| PR-032 | User changes route while a chat is running | Save the setting | The running session is not rerouted mid-request; the new route applies only to later sessions. |
| PR-033 | Workbench restarts after readiness configuration | Restart the service and reload the app | Provider route configuration, gateway readiness, and existing session runtimes are restored correctly. |
| PR-034 | Multiple users use one server | One user opens readiness/setup while another chats | Readiness and route choices are user-scoped where appropriate; no credentials, models, or error cards leak between users. |

### Model catalogs and first-send preflight

| ID | Scenario and setup | Action | Expected result |
|---|---|---|---|
| MC-001 | Native route and provider is not ready | Open the model picker | No executable models or misleading `CLI default` option are presented. |
| MC-002 | Native route and provider is ready | Open the model picker | Only models usable by that native CLI are shown, including a clearly labeled native default if supported. |
| MC-003 | IO Gateway route and gateway catalog is ready | Open the model picker | Only gateway catalog models are shown, with their route/prefix clearly identifiable. |
| MC-004 | Gateway catalog contains a model hidden by gateway visibility metadata | Open the model picker | Hidden models are omitted. |
| MC-005 | Gateway catalog contains duplicate IDs with different labels | Open the model picker | One deterministic model entry is shown and selection is stable. |
| MC-006 | Gateway model catalog request races with provider switching | Switch Codex to Claude or back quickly | A late catalog response cannot overwrite the current provider’s model list. |
| MC-007 | Model catalog request fails after the picker opens | Wait for failure | The UI shows a route-specific retry/error state, not a fake native default. |
| MC-008 | User has a stale model saved in browser preferences | Change provider or route | The stale model is cleared or preserved only as a visibly unavailable value; it is never silently sent to another route. |
| MC-009 | Selected route is missing at first send | Submit a prompt | The server performs a preflight failure before spawning a process and returns a scoped recovery action. |
| MC-010 | CLI disappears between readiness check and first send | Submit a prompt | Spawn failure is converted into an actionable, session-scoped provider-unavailable error. |
| MC-011 | Gateway key becomes invalid between model loading and first send | Submit a prompt | The request fails safely, the key is not exposed, and native sessions remain unaffected. |
| MC-012 | Codex gateway request | Inspect the spawned command/environment in a test harness | The isolated gateway provider, correct base URL, wire API, and gateway key environment are used; native ChatGPT credentials are not selected. |
| MC-013 | Claude gateway request | Inspect the spawned command/environment in a test harness | The Claude CLI receives gateway base URL/API credentials and native OAuth token variables are removed. |
| MC-014 | Native request with gateway credentials configured globally | Submit a native-model prompt in a pinned native chat | Gateway credentials are not injected and the native provider remains selected. |
| MC-015 | Gemini native request with missing CLI | Submit a prompt | The request is blocked with Gemini-specific installation/auth guidance and does not affect Codex/Claude route state. |
| MC-016 | Gateway model is selected with an incompatible provider | Submit the prompt | The model/provider mismatch is rejected before launch with a clear correction path. |

### Error, race, security, and regression coverage

| ID | Scenario and setup | Action | Expected result |
|---|---|---|---|
| ER-001 | Runtime mismatch error is emitted for an inactive session | Switch to another session before delivery | The event is ignored by the visible chat but remains available for the correct session’s reconciliation if supported. |
| ER-002 | Two tabs show the same session | Trigger a mismatch in one tab | Both tabs may show the same session-scoped state, but neither shows it in another session. |
| ER-003 | Old WebSocket event arrives after a new connection is established | Replay delayed events | Connection generation and request/response checks prevent stale UI mutation. |
| ER-004 | A generic WebSocket error has no session ID | Deliver the error | It is treated as a connection/system error and never appears as a provider-specific chat mismatch. |
| ER-005 | A provider error contains user-controlled model text | Render the error | Text is escaped; no HTML, script, URL, or command injection occurs. |
| ER-006 | Installation URL contains query or fragment data | Render and open the link | The link is validated/escaped and does not become an executable shell instruction. |
| ER-007 | Unauthenticated user requests provider readiness | Call the endpoint | Protected readiness details and gateway configuration are not disclosed. |
| ER-008 | User is authenticated but lacks server-administrator capability | View setup | The user may see safe documentation links but cannot trigger installation or privileged host changes. |
| ER-009 | Gateway API key is entered in settings and the page is refreshed | Inspect browser storage and network payloads | The secret is not placed in local preferences or URLs; only the intended protected request carries it. |
| ER-010 | A provider CLI writes secrets to stderr during status | Refresh readiness | The UI and ordinary logs do not display the raw output. |
| ER-011 | Provider status checks run concurrently for Codex, Claude, and Gemini | Open settings repeatedly | Checks are bounded, do not race into the wrong provider card, and do not exhaust process limits. |
| ER-012 | Gateway model discovery times out while native status is loading | Open settings and Chat simultaneously | Each status resolves independently; one timeout does not block the rest of the Workbench. |
| ER-013 | Existing session database contains old runtime labels or null runtime | Upgrade/start the server | Migration preserves valid `native_cli` and `io_gateway` semantics and applies the documented legacy fallback. |
| ER-014 | App is offline after setup card is rendered | Open provider URL and press Recheck | The URL remains available; Recheck reports failure without losing current route settings. |
| ER-015 | UI is opened on a narrow mobile viewport | Open readiness, route, and error cards | Links, status text, Recheck, and Start new chat remain readable and tappable. |
| ER-016 | Server process restarts during a provider request | Reconnect and inspect the session | The session ends in a deterministic failed/recoverable state; no stale error is attached to a later chat. |
| ER-017 | Provider package/version changes after manual installation | Recheck and send a harmless test prompt | Version is reported, compatibility is evaluated, and an unsupported version produces actionable guidance rather than an opaque failure. |
| ER-018 | All automated tests run with fake credentials and fake provider binaries | Run the complete suite | No test contacts production providers, writes real credentials, or changes the developer/server environment. |

### Acceptance criteria

| Area | Pass condition |
|---|---|
| Runtime safety | No existing session is silently rerouted, and a mismatched gateway model never reaches the wrong provider. |
| Error isolation | A stale or unrelated event cannot alter the currently selected chat. |
| First-run usability | A user can understand why a provider is unavailable, open the official setup URL, return, and recheck without needing to inspect server logs. |
| Route clarity | Native CLI and IO Gateway readiness are shown separately; missing native credentials do not incorrectly block a verified gateway route. |
| Model correctness | Every displayed model belongs to a route/provider that can execute it; unavailable routes do not show misleading defaults. |
| Security | Workbench does not install software, run shell installers, collect native credentials, expose gateway secrets, or grant ordinary users host-level installation authority. |
| Resilience | Timeouts, malformed catalogs, missing PATH entries, service-user differences, restarts, reconnects, and concurrent requests fail predictably and recoverably. |
| Regression | Existing native chats, existing gateway chats, project/file/Git/terminal features, mobile layout, and generic provider errors continue to work. |

## Implementation and verification — 2026-11-22

Implemented in the server, web client, Android/shared mobile client, and terminal client:

- Route selection for new chats, pinned routes for saved chats, route-specific models, and a persistent enabled-state note in gateway settings.
- Preflight checks before session mutation or CLI launch. Setup failures use a separate `chat_setup_required` event with a typed code, session ID, and sending request ID. Generic provider errors retain their existing handling; legacy requests without a request ID receive a normal session-scoped error.
- Chat-local recovery, official setup links, Recheck, and draft-preserving Start new chat. WebSocket generation checks, catalog request generations, request matching, and duplicate suppression prevent stale UI changes. Changing mobile accounts clears user-specific catalogs.
- Shared executable resolution for launches and readiness, bounded subprocess output/timeouts, coalesced provider checks, bounded gateway checks, sanitized catalog failures, and no native model defaults for unavailable routes. Gateway credentials are independent of native login. Neither installation nor native login runs inside Workbench.
- Matching mobile settings catalogs and setup guidance. Gateway secrets are cleared from form inputs after save and are no longer copied into Android preferences.

Migration and capability details:

- Saved runtime values always win over a client's requested route. For legacy sessions with no runtime, a saved gateway-prefixed model identifies the gateway route; other legacy sessions stay native. A newly submitted model or a changed global default cannot reroute a legacy thread. Starting the session persists the inferred runtime.
- The gateway checkbox changes the default for new chats. A configured gateway remains available for explicitly selected or pinned gateway chats. RT-011 means the gateway configuration itself is disabled (`mode: off`) or its required credentials are unavailable; choosing Native CLI as the new-chat default alone does not erase a working gateway configuration.
- Codex and Claude native status comes from their non-interactive status commands, rather than credentials-file existence. Gemini has no equivalent stable command: valid configuration is explicitly labeled **authentication unverified**, and malformed credential JSON is not accepted as configuration evidence. Gemini chat currently offers Native CLI; an unsupported saved gateway route is explained rather than silently downgraded.
- Native Codex models come from its configuration/cache, including `CODEX_HOME`; Claude offers its stable aliases; Gemini offers its native default. Existing native chats retain their saved model choice. Gateway model IDs must be present in the selected gateway's current catalog. Catalog connectivity and local CLI status do not prove live inference or account entitlement for every model.

### Automated verification

| Area | Result / evidence |
|---|---|
| Rust core, protocol, server, and terminal client | 1,026 tests passed using Rust 1.93.1. One existing Git fetch test was excluded and one existing test remained ignored. Follow-up runtime tests cover legacy inference and pinned-route overrides. |
| Web unit and browser regression tests | All 72 passed, with Playwright enabled. Includes the new catalog-race tests and real application setup/recovery flow at 1280px and 390px. |
| Mobile shared tests | All 792 passed. New tests cover event identity, draft recovery, protocol compatibility, account isolation, route races, cached routes, and the board model picker. |
| Android source build | `:androidApp:compileDebugKotlin` passed against Android SDK 36. |
| Running Workbench with fake CLIs and a local gateway | `crates/iowb-server/tests/provider-setup.test.cjs`: fresh install, Recheck after installation, missing native login, native execution, both gateway CLI launches, pinned-route mismatch, concurrent probe coalescing, CLI disappearance between preflight and launch, disabled gateway configuration, and invalid gateway key. The harness isolates configuration and blocks non-loopback HTTP egress. |
| Focused server coverage | `crates/iowb-server/src/server/tests/chat_readiness.rs`: malformed/empty catalogs, 401/403, hidden/duplicate models, missing CLI/auth, user isolation, protected endpoints, bounded/hanging status commands, malformed Gemini configuration, model validation, and secret-safe responses. |
| Browser error isolation | `crates/iowb-ui/tests/chat-readiness*.test.cjs`: late requests, provider/route/session changes, duplicate events, missing request IDs, official URL validation, escaped error text, and draft-preserving new-chat recovery. |
| Whitespace / patch checks | `git diff --check` passed in the main repository and `apps` submodule. Existing unrelated mobile edits were preserved. |

The 93 scenario rows above remain the acceptance checklist; grouped automated tests are not a claim that every scenario has received a separate manual sign-off. Real vendor/account compatibility, physical-device behavior, and signed APK update compatibility still require their respective environments.

Reproduction:

- Rust: use Rust 1.93.1, a workspace-safe `TMPDIR` (macOS `/tmp` resolves into the protected `/private` tree), and isolated Git system/global configuration. Run `cargo +1.93.1 test -p iowb-server -p iowb-core -p iowb-protocol -p iowb-tui --lib -- --skip fetch_all_remotes_matches_lazygit_and_preserves_fetch_head`.
- Web: set `PLAYWRIGHT_MODULE` to the installed Playwright module and run `node --test crates/iowb-ui/tests/*.test.cjs`.
- Mobile: with JDK 21 and Android SDK 36 configured, run `gradle :androidApp:compileDebugKotlin :shared:jvmTest` in `apps/mobile`.
- Running server: build `io-workbench`, set `IOWB_TEST_BINARY` to that executable, and run `node --test crates/iowb-server/tests/provider-setup.test.cjs`.

Known verification/delivery constraints:

- The installed Rust 1.100 rejects an existing SQLx 0.8.0 dependency; Rust 1.93.1 builds and tests this workspace successfully.
- The excluded existing Git test expects Fetch All not to alter `FETCH_HEAD`; the installed Git appends remote records. This change does not modify that Git implementation. Isolating Git system/global configuration also avoids the existing integration test inheriting the machine's credential helper.
- A signed ARM64 APK and Gofile upload are blocked by the missing required signing key `/home/giofahreza/.android/debug.keystore`. `apps/mobile/AGENTS.md` requires this exact signing identity and verification against the installed app/version. No substitute signing key or unsigned APK is presented as a compatible update.

## Historical outstanding test cases — before acceptance execution, 2026-11-22

This historical audit preceded the acceptance execution recorded below. At that point, of the 93 acceptance cases, **22 had no direct scenario test**, **53 were partially tested**, and **18 had targeted automated coverage with no further specific gap identified**. The following 75 gaps are retained for traceability; the current execution matrix below supersedes their outstanding status. These findings described verification gaps, not confirmed implementation failures.

“No direct scenario test” means the stated interaction has no matching execution evidence, even where related components were tested. “Partially tested” means a component, provider variant, or part of the flow passed, but the remaining checks below were not demonstrated. A helper test, mocked response, compilation, or aggregate suite total does not establish a complete user flow. Provider variants required by the original matrix remain required. None of these classifications supplies manual sign-off or proves live vendor/account compatibility.

Evidence reviewed:

- Server readiness/model tests: `crates/iowb-server/src/server/tests/chat_readiness.rs` and `models_and_configuration.rs`; route inference: `chat_runtime_routing.rs` in the same directory.
- Running-server harness: `crates/iowb-server/tests/provider-setup.test.cjs`, using fake Codex/Claude executables and a loopback gateway.
- Web unit/browser tests: `crates/iowb-ui/tests/chat-readiness.test.cjs` and `chat-readiness-browser.test.cjs`.
- Mobile: `apps/mobile/shared/src/commonTest/kotlin/io/workbench/mobile/shared/ChatReadinessTest.kt` and its nine passing JVM test results.
- Existing command/runtime and restart-recovery regression tests under `crates/iowb-core/src/core/tests`.
- Recorded runs: `/tmp/iowb-rust-verified.log`, `/tmp/iowb-routing-final.log` (the final legacy-inference correction), `/tmp/iowb-web-final.log`, `/tmp/iowb-provider-e2e-final.log`, and `/tmp/iowb-android-final.log`. These temporary logs are local evidence, not committed artifacts.

### No direct scenario test — 22 cases

| ID | Untested interaction / required check |
|---|---|
| RT-001 | Open gateway settings with the checkbox disabled and verify the enabled-state note is absent. |
| RT-002 | Open enabled gateway settings and verify the persistent new-chat/existing-runtime explanation. |
| RT-015 | Run Codex and Claude concurrently, fail only one with a mismatch, and verify the other continues without receiving its error. |
| RT-021 | Deliver a similar-but-different raw provider error through normal error handling and verify no mismatch card appears. |
| PR-006 | Click Recheck while the CLI is still missing; verify missing status remains and no chat error is appended. |
| PR-008 | Run Workbench as a different service user from the SSH installer; verify unavailable status and accurate user/PATH guidance. |
| PR-011 | Install each fake CLI only in a supported user-local location and verify readiness and actual launch resolve the same executable. |
| PR-025 | Change a centralized official documentation URL, refresh clients, and verify every setup surface uses the updated link. |
| PR-027 | Walk through Projects, Files, Git, Database, Terminal, and Settings on an installation with no CLIs or credentials; verify chat alone needs setup. |
| PR-028 | Open the provider picker with only Codex ready and verify Claude/Gemini remain independently actionable. |
| PR-029 | Open the provider picker with only Claude ready and verify Codex/Gemini remain independently actionable. |
| PR-032 | Change and save the default route during an active request; verify the live process stays on its original route and later chats inherit the change. |
| MC-011 | Load models with a valid gateway key, invalidate it, then actually send a prompt; verify scoped failure, secret safety, and unaffected native sessions. The harness currently only reloads the catalog after invalidation. |
| MC-014 | Configure gateway credentials, then send a native prompt in an existing pinned native chat and inspect its command/environment for credential isolation. Existing native launch coverage runs before gateway configuration. |
| MC-015 | Actually submit a Gemini prompt with its CLI missing and verify Gemini-specific scoped guidance and unchanged Codex/Claude route state. |
| ER-002 | Open two tabs on the same session and verify mismatch delivery stays scoped in both tabs. |
| ER-003 | Establish a replacement WebSocket, deliver a setup event from the old connection, and verify it cannot mutate current state. |
| ER-004 | Deliver a generic sessionless WebSocket error and verify it follows connection/system handling without a provider mismatch card. |
| ER-008 | Use an authenticated nonadministrator account; verify safe documentation access and inability to trigger privileged installation/host changes. |
| ER-009 | Save a gateway key through the settings UI, refresh, and inspect browser storage and network requests for secret leakage. Backend settings redaction alone does not test this interaction. |
| ER-014 | Go offline after rendering setup guidance, open the documentation link, and click Recheck; verify failure preserves route settings and the link remains available. |
| ER-017 | Replace a CLI package/version, Recheck, and send through a fake compatibility fixture; verify actionable unsupported-version behavior. |

### Partially tested — 53 cases

| ID | Existing evidence | Remaining untested check |
|---|---|---|
| RT-003 | New gateway Codex/Claude sends, saved gateway runtime, and gateway overrides. | Create through the normal new-chat flow without explicitly supplying a runtime; verify inheritance from the enabled global default. |
| RT-004 | A new native Codex send has no gateway key/override. | Equivalent Claude execution, saved native runtime assertion, and inheritance when the request omits its runtime. |
| RT-005 | Saved/legacy native route resolution ignores a changed global default. | Successfully continue existing native Codex and Claude threads after enabling the gateway; inspect the launched route. |
| RT-006 | Explicit saved gateway routes win over client/global choices. | Successfully resume existing gateway Codex and Claude threads, preserving their route and conversation. |
| RT-007 | Both providers reject a fresh native/prefixed-model request; running-server Codex rejects an existing native chat mismatch. | Repeat the existing-chat WebSocket scenario for Claude. |
| RT-008 | Codex mismatch leaves fake launch count unchanged; both providers' handler tests create no session. | Instrument both provider launches and inference requests for existing-chat mismatches; assert neither occurs. |
| RT-010 | Native route/model resolution is tested. | Successfully send an unprefixed model in an existing native chat while the gateway is enabled, for both providers. |
| RT-011 | Existing gateway Claude fails safely after configuration is disabled; both providers have catalog checks. | Equivalent existing gateway Codex send and rendered recovery guidance. |
| RT-012 | Stale preference cleanup and server route/model validation are tested separately. | Seed actual browser storage with a gateway model, disable gateway configuration, and exercise picker plus send. |
| RT-013 | Browser recovery preserves draft/transcript and prepares a new gateway chat. | Send the recovered draft, verify the same project and gateway runtime, then reopen the old transcript. |
| RT-014 | Saved-route resolution and mobile cache serialization are tested. | Change the default, reload/reopen actual existing chats, and create subsequent chats through the UI. |
| RT-018 | Session/request filtering and stale catalog navigation guards are tested. | Switch projects during a pending send and deliver the old setup error through the actual event dispatcher. |
| RT-019 | Duplicate setup events are suppressed on the same connection. | Disconnect/reconnect and replay the mismatch; verify exactly one correctly scoped card. |
| RT-020 | Missing-ID setup events are ignored safely; generic legacy error decoding is tested. | Send from a legacy client without a request ID and verify the server's session-scoped generic fallback through the UI. |
| RT-022 | Mobile decodes the exact historical message as a generic error. | Emit the raw text from a fake CLI and verify normal scoped error rendering in running clients. |
| RT-025 | Legacy inference uses the stored model rather than global/submitted values. | Resume the legacy session, verify inferred runtime persistence, and reload it. |
| PR-001 | Missing Codex endpoint/catalog and official URL checks. | Codex-specific rendered setup card and working documentation/Recheck actions. |
| PR-002 | Missing Claude endpoint plus desktop/narrow-browser setup card. | Click its documentation and Recheck actions; verify the equivalent native mobile flow. |
| PR-003 | Missing Gemini endpoint/catalog and official URL checks. | Gemini-specific rendered setup card and working documentation/Recheck actions. |
| PR-004 | Mocked missing Node/npm status renders a Node.js link. | Run with dependencies actually absent and verify accurate detection and absence of installer/package-manager execution. |
| PR-005 | A 390px browser card says installation belongs on the server. | Tap the documentation link from the native mobile app and verify the guidance and external-link behavior. |
| PR-007 | Fake Codex/Claude installation is detected by refreshed endpoints. | Click Recheck in an already-open client; cover Gemini installation and updated version/status display too. |
| PR-009 | Command helper rejects nonzero version output. | Full provider readiness response and actionable UI state for each provider. |
| PR-010 | Command helper terminates a hanging command within a bounded timeout. | Hang each provider's version probe through the endpoint while verifying responsive UI/server and verification-failure guidance. |
| PR-012 | Command helper rejects a nonexecutable file. | Full readiness/card behavior, including repair/install URL, for each provider. |
| PR-013 | Codex/Claude endpoints distinguish installed from unauthenticated; native Codex send is blocked. | Render and use official login guidance, test Claude first-send blocking, and verify Gemini's explicit configured/unverified state. |
| PR-014 | Rejected/malformed fake status results and malformed Gemini JSON are tested. | Expired credentials and provider-specific malformed/rejected credential fixtures through full readiness. Gemini must remain authentication-unverified. |
| PR-015 | Successful fake Codex/Claude status commands and native Codex execution. | Native Claude model exposure/execution and client readiness updates; check Gemini according to its documented unverified capability. |
| PR-016 | Failed status commands and generic command timeout/output bounds. | Hang the actual auth-status probe for each supported command; verify retry/login guidance, responsiveness, and secret-safe UI/logs. |
| PR-017 | Unconfigured-user gateway catalog is unavailable. | Separately remove URL/key and verify the selected gateway route points to settings in each client. |
| PR-018 | An unreachable loopback gateway returns unavailable with no models. | A stalled request timing out while native readiness/chat remains responsive, including visible route-specific error state. |
| PR-020 | Empty catalog parsing returns an empty list. | Full route response and rendered no-models guidance with no native fallback. |
| PR-021 | Malformed JSON/unexpected schema fail safely in the fetch helper. | Full readiness/model-picker failure state and recovery for both response types. |
| PR-022 | Backend distinguishes reachable gateway from missing CLI for Codex/Claude. | Render that combined state from the real response and verify chat cannot start. |
| PR-024 | Responses/settings redact fake keys; the harness checks server logs. | Inspect rendered UI, URL construction, and browser/mobile preferences after settings save/reload. |
| PR-026 | Invalid/foreign/non-HTTPS documentation URLs are rejected by validation. | Render malformed URL fallback and simulate unavailable official documentation; verify useful manual guidance. |
| PR-030 | Independent provider status and route/catalog isolation components are tested. | Both native providers ready in the same UI; execute native chats and verify provider/model choices do not leak. |
| PR-031 | Gateway Codex/Claude snapshots record runtime; a native Codex launch succeeds. | Create both route types through clients and verify visible session metadata for each provider. |
| PR-033 | Mobile cache round-trip retains runtime; stored route resolution is tested. | Restart the actual Workbench service and reload clients; verify configuration, fresh readiness, and existing routes. |
| PR-034 | Per-user catalog helpers and mobile account-switch cache clearing. | Simultaneous authenticated users: one setting up, one chatting; verify credentials/models/events never cross accounts. |
| MC-002 | Native default syntax, native Codex readiness/default, and existing model preservation. | Ready native Claude/Gemini catalogs and picker labels, including Gemini's unverified-auth explanation. This does not require paid model-entitlement tests. |
| MC-005 | Backend collapses duplicate catalog IDs in a fixture. | Assert the deterministic winning entry/label and stable picker selection across refreshes. |
| MC-009 | Native Codex missing-auth and gateway Claude disabled-configuration sends fail before launch. | Other required provider/route-unavailable variants and matching scoped client recovery actions. |
| MC-010 | Codex disappears during the real preflight-to-spawn gap and returns a scoped error. | Equivalent Claude/Gemini disappearance and rendered recovery behavior. |
| MC-016 | Unsupported Gemini gateway route and invalid model IDs are rejected by helpers. | Submit incompatible provider/model combinations through WebSocket; assert zero launches and a clear correction action. |
| ER-006 | URL validation accepts safe query/fragment data and rejects unsafe URLs. | Render and open such a documentation link; verify escaping and destination preservation. |
| ER-010 | Fake status stderr secrets do not appear in returned status. | Assert full endpoint/client output and captured ordinary logs omit secrets on success, failure, and timeout. |
| ER-011 | Twelve simultaneous Codex refreshes share one version/auth probe. | Repeated mixed Codex/Claude/Gemini checks; assert bounded process count and correctly assigned cards. |
| ER-012 | Catalog failures and status bounds are tested independently. | Stall gateway discovery while native status/chat loads; verify independent completion and responsive settings/chat. |
| ER-013 | Valid runtime parsing, null-runtime fallback, and cache round-trip are tested. | Upgrade an actual database fixture with old/unknown labels and null runtimes; verify documented migration/persistence semantics. |
| ER-015 | Claude readiness/error/recovery cards render at 390px and 1280px; Android compiles. | Route controls, long text/links, all providers, and native Android readability/tap targets. |
| ER-016 | Existing core tests reconcile interrupted active sessions after startup. | Restart the actual process mid-provider request, reconnect clients, then start another chat and verify no stale setup error leaks. |
| ER-018 | New running-server harness uses fake credentials/CLIs, temporary configuration, and non-loopback HTTP proxy isolation. | Audit and enforce isolation for the complete suite, including all external-call paths and real credential/environment writes. |

For completeness, the other 18 IDs have targeted automated coverage without another specific outstanding check identified in this audit: **RT-009, RT-016, RT-017, RT-023, RT-024; PR-019, PR-023; MC-001, MC-003, MC-004, MC-006, MC-007, MC-008, MC-012, MC-013; ER-001, ER-005, ER-007**. Their evidence is limited to the fake-provider, unit, and browser scopes described above; this is not a blanket manual or production sign-off.

Prioritize existing-chat continuation and native credential isolation (RT-005/006/010, MC-014), first-send key failure (MC-011), concurrent/stale connection isolation (RT-015/019, ER-002/003/016), and the Recheck/setup flow (PR-006/007/008/011). These checks can largely use fake CLIs and a local gateway; keep production credentials and paid requests out of the automated acceptance run.

## Acceptance execution after the audit — 2026-11-22

Current coverage is **86 automated passes, 5 partial cases, and 2 unsupported scenarios**. Every original ID is accounted for below. An automated pass means the specified behavior was exercised with controlled CLIs/gateway or client fixtures, as the original matrix requires. It does not assert live vendor/account entitlement, a native Android device sign-off, or an unsupported account/role capability. The feature must not be described as fully verified across those remaining environments.

The acceptance work found and fixed:

- Sessionless WebSocket errors were appended to the currently selected chat. Web and mobile now report them as system/connection errors while keeping ordinary session-scoped provider errors unchanged.
- Mobile events from a replaced WebSocket could fail the current request. Events and disconnect handling now check both stream generation and connection identity. Real CIO WebSocket tests over loopback reproduce the old failure and verify the fix, including switching accounts.
- A cleared gateway URL could fall back to the legacy default host despite a stored key. Chat now requires an explicit destination; legacy `base_url` is preserved when normalizing configured endpoints.
- Failed offline Recheck removed the documentation link. Web and mobile retain guidance only for the same catalog scope while marking the route unavailable and clearing executable models. Cross-provider/session/route changes do not inherit that guidance.

Evidence keys used in the execution matrix:

| Key | Executable evidence |
|---|---|
| S | `crates/iowb-server/tests/provider-acceptance.test.cjs`: actual Workbench processes, temporary HOME/config/database, fake Codex/Claude/Gemini CLIs, loopback gateway, HTTP and WebSocket requests, recorded launches/probes, real process restarts. |
| B | `crates/iowb-server/tests/provider-browser.test.cjs`: Chromium against that running Workbench, actual API/WebSocket traffic, recovery followed by send, transcript reopening, two tabs, no-provider non-chat views. |
| W | `crates/iowb-ui/tests/chat-readiness-acceptance-browser.test.cjs`: real application DOM/handlers with controlled I/O at 1280px and 390px; settings, Recheck, unavailable documentation popups, offline checks and connection replay. |
| U | Existing server `chat_readiness.rs`, `chat_runtime_routing.rs`, `models_and_configuration.rs`, and web `chat-readiness.test.cjs`: focused catalog, model-validation, event identity, authentication and race regression tests. Includes the new explicit-URL test. |
| M | Mobile `ChatReadinessTest.kt` and `ChatReadinessAcceptanceTest.kt`: shared controller/API tests, pinned routes, settings changes during requests, drafts, account/provider races, offline links and ordinary/system errors. |
| J | Mobile JVM `ChatReadinessSocketTest.kt`: real CIO connections to a local WebSocket fixture; obsolete connection events/disconnects and previous-account events cannot alter the current state. |
| I | `scripts/test-provider-acceptance.mjs`: complete validation commands in a temporary child-process HOME with credential variables removed, unavailable provider stubs, blocked non-loopback HTTP proxy configuration, offline dependency resolution, and mandatory browser execution. |

Final isolated regression run completed successfully on 2026-11-22:

| Validation | Passed | Failures | Notes |
|---|---:|---:|---|
| Rust library suites (core, protocol, server, storage, TUI, UI) | 1,084 | 0 | One pre-existing ignored test and one explicitly excluded Git `FETCH_HEAD` test; neither is counted as a pass. |
| Web unit and Chromium suites | 76 | 0 | No cancelled or skipped tests; desktop and 390px viewport coverage. |
| Running-server and browser acceptance suites | 32 | 0 | Actual isolated Workbench processes with controlled provider CLIs and gateway; no skipped tests. |
| Mobile shared JVM suites | 801 | 0 | No failures, errors or skipped tests; includes real loopback CIO WebSocket regressions. |
| Android source compilation | Build succeeded | 0 | `:androidApp:compileDebugKotlin`; this is not a signed APK or native device test. |
| Total automated tests | **1,993** | **0** | Suite test counts are separate from the 93 acceptance-case IDs below. |

The final added assertion that migrated explicit native/gateway runtime labels survive a second restart also passed in a focused rerun (11 tests, zero failures/skips), recorded in `/tmp/iowb-acceptance-restart-final.log`. These repeat executions are not added to the unique suite total. Both main-repository and mobile-submodule `git diff --check` checks passed.

### Current execution matrix

| ID | Status | Evidence / remaining check |
|---|---|---|
| RT-001 | Automated pass | W: disabled checkbox hides enabled-state note. |
| RT-002 | Automated pass | W: enabling shows the persistent route note; saved state survives refresh. |
| RT-003 | Automated pass | S: both providers inherit gateway default when runtime is omitted; runtime and overrides inspected. |
| RT-004 | Automated pass | S: both providers inherit native default; no gateway key/override in launch. |
| RT-005 | Automated pass | S: existing native Codex/Claude continue after enabling gateway. |
| RT-006 | Automated pass | S: existing gateway Codex/Claude continue despite opposite new-chat defaults. |
| RT-007 | Automated pass | S/B: existing native chats reject gateway-prefixed models with typed scoped mismatch. |
| RT-008 | Automated pass | S: mismatch changes neither launch count nor gateway request count, for both providers. |
| RT-009 | Automated pass | B/W: chat-local explanation and new-chat action render. |
| RT-010 | Automated pass | S: existing native chats execute while gateway is enabled. |
| RT-011 | Automated pass | S: disabling configuration blocks both saved gateway routes without downgrading. |
| RT-012 | Automated pass | B/U: stale saved gateway selection is cleared before a native send; manually supplied stale IDs are rejected. |
| RT-013 | Automated pass | B: new-chat action preserves project/draft, next send completes through gateway, old persisted transcript remains unchanged and reopens. |
| RT-014 | Automated pass | S/B/M: server restart, browser reload/reopen and cached route restoration preserve existing routes; new sends inherit defaults. |
| RT-015 | Automated pass | S: concurrent Claude execution continues while Codex mismatch fails independently. |
| RT-016 | Automated pass | W/U/M: another session's setup error is ignored. |
| RT-017 | Automated pass | W/U/M: older request error cannot overwrite the current attempt. |
| RT-018 | Automated pass | W/M: navigation and project changes discard old attempt events/catalogs. |
| RT-019 | Automated pass | W/J/U: reconnect/replay remains scoped; duplicate setup events are suppressed. |
| RT-020 | Automated pass | S/U/W: request-ID-less legacy command gets ordinary session-scoped fallback; clients handle legacy/generic events safely. |
| RT-021 | Automated pass | S/W/M: similar raw provider failures remain ordinary errors. |
| RT-022 | Automated pass | S/W/M: exact historical raw error is delivered/rendered without global mismatch conversion. |
| RT-023 | Automated pass | U: whitespace/case prefix validation and canonical catalog ID matching. |
| RT-024 | Automated pass | U: unknown prefixes and shell/HTML/flag-shaped IDs rejected. |
| RT-025 | Automated pass | S/U: actual persisted legacy gateway metadata is inferred, resumed and saved with an explicit runtime. |
| PR-001 | Automated pass | S/W: missing Codex, official link, no executable models, Recheck. |
| PR-002 | Automated pass | S/W: equivalent Claude setup flow. |
| PR-003 | Automated pass | S/W: equivalent Gemini setup flow. |
| PR-004 | Automated pass | S/W: Node/npm absent from service PATH, Node documentation shown, no installer launched. |
| PR-005 | Partial | W verifies server-not-phone guidance and link opening at 390px. Native Android external-link tapping remains unverified. |
| PR-006 | Automated pass | W/M/S: repeated Recheck before installation remains missing and creates no chat error. |
| PR-007 | Automated pass | S/W: manual fixture installation/version changes and client Recheck update readiness, including Gemini. |
| PR-008 | Partial | S verifies distinct simulated SSH/service HOME/PATH environments and repair guidance. A real Linux service under another Unix UID is not exercised here. |
| PR-009 | Automated pass | S: nonzero version probes make each provider unavailable. |
| PR-010 | Automated pass | S: all three version probes time out while another provider and health endpoint stay responsive. |
| PR-011 | Automated pass | S: user-local CLI resolution is used for readiness and actual launches for all three providers. |
| PR-012 | Automated pass | S: actual nonexecutable CLI files are unavailable, with setup guidance. |
| PR-013 | Automated pass | S/U/M: installed versus unauthenticated states, official authentication links, and scoped native first-send rejection. |
| PR-014 | Automated pass | S/U: expired fixture files do not override rejected status commands; malformed Claude status/Gemini credential JSON is not treated as authentication. |
| PR-015 | Automated pass | S: successful fake native status allows Codex/Claude execution; Gemini configuration stays explicitly unverified. |
| PR-016 | Automated pass | S/U: hung/failed auth probes, malformed output and output bounds fail safely without status secrets. |
| PR-017 | Automated pass | S/U/W: missing configuration/cleared URL blocks gateway and offers configuration guidance; no implicit default host. |
| PR-018 | Automated pass | S/W: unavailable or stalled gateway discovery cannot block native checks/execution. |
| PR-019 | Automated pass | S/U/W: 401/403 produce authentication failure, empty models and secret-safe messages. |
| PR-020 | Automated pass | S/W: empty catalog displays no-model guidance with no native fallback. |
| PR-021 | Automated pass | S/W: malformed JSON/schema fail safely with no fabricated models. |
| PR-022 | Automated pass | U/W: reachable gateway and missing CLI are distinct; required-binary guidance remains actionable and chat is unavailable. |
| PR-023 | Automated pass | S/U: both gateway CLI routes execute without native login. |
| PR-024 | Partial | S/W verify default responses, server logs, browser storage, settings refresh and URLs. Native Android preference/storage inspection after save is still unverified. |
| PR-025 | Automated pass | W/U: refreshed documented URLs propagate into rendered/opened links; official URL validation is enforced. |
| PR-026 | Automated pass | W/U: unavailable documentation is simulated with HTTP 503; malformed URLs are omitted while manual server guidance remains, with no shell execution. |
| PR-027 | Automated pass | B/S: Projects, Files, Git, Database, Terminal and Settings work with no provider CLI/auth/gateway. |
| PR-028 | Automated pass | W: only-Codex-ready catalog matrix leaves other providers independently actionable. |
| PR-029 | Automated pass | W: equivalent only-Claude-ready matrix. |
| PR-030 | Automated pass | B/S/M: both native providers execute independently; provider/model catalogs do not alter another chat route. |
| PR-031 | Automated pass | B/S: native/gateway sessions store distinct runtimes and expose route state in chat. |
| PR-032 | Automated pass | S/M: saving gateway default during a native run does not change that request or its credentials. |
| PR-033 | Automated pass | S/B/M: configuration, catalogs and stored routes survive actual service restart and client/cache restoration. |
| PR-034 | Unsupported | S confirms a second account registration is rejected with 403: Workbench is currently single-user. U/M still verify user-keyed catalogs and cache clearing; simultaneous supported multi-user operation cannot be signed off. |
| MC-001 | Automated pass | S/U/W: missing/unready providers never offer executable models or a fabricated native default. |
| MC-002 | Automated pass | S/U/W: native defaults/catalogs are separated; Gemini's authentication remains unverified. Vendor entitlement is outside fake-provider verification. |
| MC-003 | Automated pass | S/U/W: gateway route exposes its catalog IDs without native model fallback. |
| MC-004 | Automated pass | S/U: hidden gateway models omitted. |
| MC-005 | Automated pass | S/U/W: duplicate IDs collapse to a deterministic stable entry; selection uses current catalog values. |
| MC-006 | Automated pass | U/M: delayed catalogs cannot cross provider/route/session selections. |
| MC-007 | Automated pass | W/U/M: catalog failure gives unavailable/retry state, without a fake default. |
| MC-008 | Automated pass | B/U/M: stale selections are cleared or rejected when route/provider changes. |
| MC-009 | Automated pass | S/B: unavailable first-send routes return scoped failures before launch. |
| MC-010 | Automated pass | S: Codex, Claude and Gemini disappear between probe and launch; all yield scoped provider-unavailable failures. |
| MC-011 | Automated pass | S: key invalidated after model load is rejected on actual send, with no launch/key leak; native sends still succeed for both providers. |
| MC-012 | Automated pass | S/U: Codex gateway provider override, wire API and gateway credential environment inspected. |
| MC-013 | Automated pass | S/U: Claude gateway URL/key inspected and native OAuth removed. |
| MC-014 | Automated pass | S: existing native Codex/Claude launch without gateway credentials even when gateway is configured. |
| MC-015 | Automated pass | S: missing Gemini send receives provider-specific setup guidance without blocking other providers. |
| MC-016 | Automated pass | S/U: unsupported Gemini gateway send and invalid route/model choices cannot launch. |
| ER-001 | Automated pass | W/U/M: inactive session cannot mutate visible chat. No inactive setup-error reconciliation feature is claimed. |
| ER-002 | Automated pass | B: two actual tabs on the same chat remain isolated across sending attempts and navigation. |
| ER-003 | Automated pass | W/J: obsolete browser and real mobile WebSocket events cannot mutate the replacement connection's state. |
| ER-004 | Automated pass | W/M: sessionless errors affect system/connection state, not the selected chat transcript. |
| ER-005 | Automated pass | W: user-controlled error HTML is rendered as text; unsafe documentation schemes rejected. |
| ER-006 | Automated pass | W/U: query/fragment documentation links render and open intact with controlled unavailable responses. |
| ER-007 | Automated pass | S/U: protected readiness endpoints return 401 without authentication. |
| ER-008 | Unsupported | No administrator/nonadministrator role model exists. S confirms even the authenticated owner has no provider installer endpoint (404/405); role-based setup cannot be exercised as written. |
| ER-009 | Automated pass | W/S: gateway save uses intended request body, clears fields, and does not put the key in browser storage/URLs after refresh. |
| ER-010 | Automated pass | S/U: status stderr secrets do not reach readiness responses or ordinary captured logs. |
| ER-011 | Automated pass | S: mixed Codex/Claude/Gemini refreshes coalesce independently; probe counts and provider identities verified. |
| ER-012 | Automated pass | S: held gateway discovery times out independently while native status and native chat complete. |
| ER-013 | Automated pass | S/U: persisted unknown/null runtime metadata, valid runtime parsing, legacy fallback, resume and persistence. |
| ER-014 | Automated pass | W/U/M: offline Recheck preserves the scoped documentation URL and route while marking models unavailable. |
| ER-015 | Partial | W verifies real web controls/actions at 390px; native Android layout/tap targets and device behavior remain unverified. |
| ER-016 | Automated pass | S/W/J: real server kill/recovery plus later chat and stale-connection guards; no setup failure leaks into a subsequent chat. |
| ER-017 | Partial | S tests version updates, unparseable/nonzero version results and failing status capabilities. Compatibility with actual vendor package upgrades/unsupported semantic versions is not established; no supported-version range is declared. |
| ER-018 | Automated pass | I/S/W/J: isolated commands use fake credentials/binaries, controlled external browser I/O and loopback-only provider fixtures; dependency resolution is offline. |

### Remaining checks and capability limits

| IDs | Required environment or decision |
|---|---|
| PR-005, PR-024, ER-015 | Native Android/device interaction and storage inspection. No device is attached. `apps/mobile/AGENTS.md` requires the exact signing identity at `/home/giofahreza/.android/debug.keystore`; that key is missing. The existing macOS debug keystore was checked and has a different certificate, so it cannot substitute for the required identity. No compatible signed APK or Gofile delivery is claimed. Shared JVM tests and Android source compilation do not replace these checks. |
| PR-008 | Run the CLI-install/service-user check with genuinely different Unix users under the deployment's service manager. Temporary HOME/PATH isolation is already tested locally. |
| ER-017 | Define supported vendor CLI versions or a capability/upgrade policy, then run the corresponding real-package compatibility matrix using isolated configuration and harmless fixtures. |
| PR-034, ER-008 | Multi-user accounts and administrator roles are outside the current supported application model. Their absence was verified; these cases are not counted as passes. |

Run the full automated validation from the repository root with JDK 21, Android SDK 36, Rust 1.93.1, Gradle and a separately installed Playwright/Chromium available:

```sh
PLAYWRIGHT_MODULE=/path/to/playwright node scripts/test-provider-acceptance.mjs
```

The runner requires browser execution, serializes Rust tests that mutate process-global environment, and limits Node test-file concurrency. One pre-existing Git `FETCH_HEAD` compatibility test remains explicitly excluded and one pre-existing Rust test remains ignored; neither is counted as provider-feature acceptance coverage. All commands and their exact totals are recorded in `/tmp/iowb-acceptance-isolated-full.log` for this machine.

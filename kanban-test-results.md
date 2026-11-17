# Kanban manual test results

Started 1 October 2026. Source: [kanban-test.md](kanban-test.md). All 198 named cases are tracked below. This is an in-progress run, not a completion claim.

## Method

Each case records the first manual observation, any fix, and an independent retest. One logical UI action is followed by a screenshot and inspection. Read-only server data corroborates the UI. Automated regressions do not count as manual passes. Existing unrelated work and historical boards are preserved; destructive tests use disposable test projects.

## Environment

- Continuation 5 October2026,16:34:33WIB: the already-started backfill-performance deployment completed with `DEPLOYED_AND_VERIFIED`; live PID1770863 matches SHA256 `a4b02921ca66202909dae6cbda89b92024bcbfcd8246e454fe0304930947fd61`. Health returned `status:ok` for the expected configuration directory/database. Evidence: `server-backfill-deploy-1005/deployment-result-backfill.txt`, `deployment-backfill.log`, and `health-backfill.json`. This verifies deployment only; subsequent ISSUE-047 live/native and ISSUE-049 v98 confirmation retests are recorded below. The completed deployment must not be replayed.

- Resumed 4 October 2026: verified live PID160390 and installed binary both match SHA2569542e20d561d816277e8c9166241e1bb6c3cb4560b27b680f13836096103ebc8. Deployment attempt3 completed on3October17:41:58WIB; it was not repeated. Installed the previously verified APK79 on dedicated emulator5594, retaining its login. Relay18101 has no active fault rules. Fresh evidence starts1105. ISSUE027 fresh existing-board dependency/origin retest passed on F (1117–1126); its original three records remain unchanged. Multi-draft forward-reference verification follows.

- Current deployment: combined015follow-on/019/020 DEPLOYED_AND_VERIFIED3October01:56:44WIB. Resumed16:15WIB after host reboot09:57:52; livePID902 and installed/processSHA681f86c3f06d48a4f4aa6f9a4c367a35db1747e8bf51de4816e9eac9f0389411 match. Expected8100health/config/database identity. All A–H remain byte-identical to predeployment snapshots: no implicit test-board recovery, data restoration or repeated deployment. Dedicated5594emulator/18101relay are being restored; no other device controlled. Evidence server-issue015019020-deploy-09VhL3.
- Current deployment: combined017+018 DEPLOYED_AND_VERIFIED3October01:03:59WIB, PID222103, liveSHAf2f051d63766bf37a496f0f4862365278597a14febd474f267b336516a9a5978. Health/config/database correct.482full/17criticalinputs stable;355Kanbanpass/1existingignore. Exact inactiveG/H guards passed; no board/database restoration or automatic test execution. Evidence server-issue017-018-deploy-uatzdq. Native retests pending below.
- Current deployment: combined015+016 DEPLOYED_AND_VERIFIED3October00:23:01WIB, PID185750, live/installedSHA4df056ad606e45a1e2f0f9d624b116c60501c0a58df2cbf5399240e018b1eca6. Expected8100health/config/database; G remained blocked/inactive with15records/revision3. Detached runner preserved all data; no restoration.481buildinputs/15criticalstable,332Kanbantestspass/1existingignored. Unrelated shared DB-transfer changes included and disclosed;43storage/startup/recoveryinputs and canonicalSQLite schema unchanged. Evidence server-issue015-016-deploy-cS8LTn.
- Current live server: combined013follow-on/014deployment verified2October23:36:20WIB, PID107781, installed/live SHAd0c8ed3fd857ae6dc8d44ac2a551b0b0678a6fc531f83d681584d691ae43e85e, expected8100health/config/database. First three attempts safely refused before any live change as active-session IDs changed; attempt4 succeeded. Refusal logs preserved. Runner permits completed acknowledged sessions but still rejects new unacknowledged sessions. No data restoration.
- Current dedicated Android: emulator5594/Android35ATD/720×1600, APK76/0.1.76-consistent-typography, installed SHAeb06826c1201765a175d457570cdaa0a60706abbf013bf45212e712d66781353. Manually connected/signed in768–778 through transparent18101relay to8100. Another typography workflow controls5592/18100; do not revert or control it. Its install76 between762/763 explains the externally forced relaunch;763–767 are environment collision evidence, not a Kanban navigation failure.
- Continuation 2 October23:25WIB: the host rebooted at19:38WIB. The old013server restarted asPID894 and automatically resumed G planning, then hit the same014redaction hang. Latest user explicitly authorizes restarting port8100; the earlier permission hold below is superseded. Android/proxy restored, APK75 reinstalled and login completed through UI726–732. ATD's disabled drawing property caused black screenshots723–724; enabling rendering restored725, an emulator setup issue rather than an app defect. G reopened734–736 without submitting work again.
- Current server: ISSUE-013 deployed2October15:58:28WIB, detached runner result DEPLOYED_AND_VERIFIED. LivePID2184853 and installed binary SHA9749ba04543973a920f162dcaf7d64659ed0619d21d670f6ab32343883219a16 verified after restart; health/config/database identity correct. All A–G test boards preserved, no data restoration. Manual retest starts on fresh G, not historical A's contaminated workspace.
- Target server: http://127.0.0.1:8100. ISSUE-007 deployed 2 October at13:14:54 WIB through a detached user unit; result DEPLOYED_AND_VERIFIED, live PID1850609 SHA-25603a2e96224ce0aa9d093d50cf31330f1929630cae7b67559745d71c7e6cfe93a. Health, config and database identity verified after restart; no database or board restoration performed.
- Live and installed server SHA-256 at start: b189e67e2b776b2ce0c6cc0606cba633ad8384fac09a9eb8281995fbf2421115.
- Android: dedicated headless emulator-5592, Android 35 ATD, 720×1600 physical pixels at density 320. Original baseline was APK43 (SHA-256 f38b905868ada1f32e9b939ab0e7075406822e6c0221c252e203b3ebdee59354). On 2 October, restored the stopped emulator and installed verified APK45 (SHA-256 e8caac29eee1f7bb91e43282d704e37d40e6a3c2ecf7c623ae2c8f4069ace914). Direct connection to port8100, signed in through UI; screenshots079–085. No old relay was assumed to be running.
- Evidence directory: artifacts/kanban-manual-20261001.
- Status: 58 passed, 1 failed, 2 unavailable on this UI, 34 in progress, 103 not yet tested. Counts are case results, not individual assertions. Additional prerequisite defects are tracked separately below.
- Current Android: APK75 /0.1.75-kanban-status-feedback, installed and version/hash verified542; SHAcd91b9ba253894f9a7a694223bd792c8d0391a9623d3238a4f6a71a532475af7. Fresh full shared suite609passed, no exclusions,345 mobile inputs and18 critical files stable. ISSUE012 manual retests545–555 pass. ARM64 delivered at https://gofile.io/d/8tPEdyyU ; upload status OK and public page HTTP200, remote size10,189,642bytes and MD5dd105dd498680c3a2e0154d360cfb8da match locally. APK74 had592 full tests and passed ISSUE009–011 manual retests. Previous verified ARM64 delivery73: https://gofile.io/d/khO7Yip8 .
- Earlier Android checkpoint: installed staged APK72, version0.1.72-kanban-ui-feedback, SHA7aedd4f9a0716a6dad9808f4fc8c228a2bef0499e3fa374620a98e89f495051d (258). Its full shared suite passed555 tests with no failures or skips; all341 recorded mobile inputs and13 critical Kanban files stayed identical during the successful build. ARM64 delivery: https://gofile.io/d/NqioN0mM ; remote size10,189,638bytes and MD5 match locally. Test relay18100 transparently forwards to8100 except explicitly recorded one-shot faults.
- Interruption: server restarted externally at12:43:20 WIB on2October. Live and installed binary SHA remained217b1812; A/B boards and registered C survived. APK72's interrupted build was retried only after confirming it was no longer running. Screens240–241 inspected before resuming; no creation was resubmitted.

## Issues and repair history

### ISSUE-001 Delayed board responses cross project boundaries

- [x] Code regression before fix: 7 of 8 tests failed because stale responses restored the old project/board or leaked obsolete failures.
- [x] Narrow fix: invalidate board request generations on project/connection changes and ignore obsolete results; current errors still propagate.
- [x] Regression after fix: 11 new tests pass; full shared JVM suite 471 passed with no failures or skips.
- [x] Build: Android versions 44 and 45 built for x86_64 and ARM64, signed and source manifests checked; v45 includes ISSUE-002 and passed all 476 current shared tests. Evidence in android-v45-isolation.
- [ ] Manual failure reproduction on original version 43 — screenshots 060–071 show attempted long/short delay races, but the response timeout masked the code defect; these are not a claimed reproduction.
- [x] Install version70 (includes this fix) and repeat a conclusive Android race: delayed the real392749-byte historical board detail, navigated to the empty fixture while it remained in flight, then released it44.166seconds later. Proxy confirms complete delivery with no transport error; the new project remained correct after release (155–159).
- Status: FIXED AND MANUALLY RETESTED. Original-version manual attempts remain inconclusive; the failing source regressions establish the pre-fix defect separately.

### ISSUE-002 New project registration is not visible after success

- [x] First manual observation: UI registered the empty kanban-manual-a-1001 directory, but the project stayed absent from the list even after explicit refresh. Screens 021–030 preserve the stale list.
- [x] Read-only confirmation: project persisted at 07:36:28 UTC, ID project_a46efd79312740fa8e1c9c3917d97df4.
- [x] Diagnosis: full project/history GET took 21.04 seconds versus 0.66 seconds for metadata-only GET; ordinary Android read timeout is 10 seconds. Registration waits for a full broadcast, followed by another full refresh.
- [x] Workaround observed: relaunching Android uses a lightweight initial connection and made the already-created project visible (034). No duplicate registration was submitted.
- [x] Fix: return persisted registration independently of the expensive broadcast, then refresh metadata without requiring history hydration. Use normal project selection to avoid retaining the previous chat path.
- [x] Regression cycle: server red then 2/2 green; mobile first red, first fix 55/56 exposed stale chat path, second fix 56/56 green; combined shared suite 476/476.
- [x] Deploy server and install client — server verified live on 2 October; APK45 installed and signed in through Android UI.
- [x] Manual retest: on 2 October created and registered kanban-manual-b-1002 entirely through the folder picker (108–113). It appeared expanded at the bottom of the saved project ordering (116), opened the correct empty board (117), and refreshed correctly (118), without app restart or duplicate registration. Database registration time04:33:13UTC corroborates this one submission.
- Status: FIXED AND MANUALLY RETESTED. UX note: the list does not automatically reveal the newly added project; its bottom placement initially made success hard to find. The subsequent Loading projects label was background history hydration, not failed registration.

### ISSUE-003 Board load failures have no persistent Android explanation

- [x] Manual retest after server repair: missing directory no longer opens a bogus empty board, but leaves Projects without readable persistent error feedback (096–098).
- [x] Additional reproduction: permission loss and replacement with a regular file while an empty board is open leave the stale No board state and enabled Create board action (101–103; accessibility dump102 confirms enabled=true).
- [x] Control: restoring the original empty0755directory allows normal opening/refresh (099–100 and104).
- [x] Fix: persistent project-scoped load error, explicit Retry, and no actionable unconfirmed/stale board state; old failures/cancellation cannot leak across project selection. Seven scoped mobile files changed.
- [x] Regression cycle: five failures before fix;19 board-refresh tests and all523 shared tests pass afterward. Evidence files use prefix issue004-board-load-error; they refer to this ledger ISSUE-003.
- [x] Rebuild/install and manual retest — APK70: missing404, denied403, and non-directory400 each show project-specific persistent guidance, disabled board actions, and Retry (143–148); restoring the original0755directory recovers correctly each time (145,147,149). Initial and refresh503 failures likewise show persistent errors and recover through Retry (150–154).
- Status: FIXED AND MANUALLY RETESTED for OPEN-05/08. System-toast behavior on ATD is not assumed to match every device; absence of persistent feedback and stale enabled actions on the old build were directly observed.
- Setup note: asynchronous pinned-chat hydration moved the list before tap086, opening an unrelated existing chat. No message/action was submitted; immediately returned to Projects (087–088), restored the fixture, and re-located the intended card. Screens096 onward target the verified fixture card.

### ISSUE-004 Cancel retains unsaved nested configuration

- [x] First manual test: APK70, Thinking initially off (161), enabled (162), then Cancel. Parent creation summary still says Thinking default (163), but reopening shows Thinking checked (164). No board was submitted.
- [x] Fix: isolate the nested configuration draft, including mutable model maps; commit only on Save.
- [x] Targeted regression: seven failing tests become seven passing tests. APK71 built, but is not installed; waiting for combined APK72. The71 full suite did not run because an unrelated database test failed compilation, so targeted green is not a full-suite pass.
- [x] Manual retest on APK72: Thinking change discarded by Cancel (270–273); model-map edit discarded by Android Back (274–277); Back discards a second toggle edit (278–281); profile change discarded by Cancel (282–285). Outside tap is ignored by this modal (279), not a claimed dismissal.
- [x] Positive Save control: Product-Ready, Thinking on, and named Breakdown route persist on reopening (286–293). Outer Cancel still leaves C with no board (302, read-only corroboration).
- Status: FIXED AND MANUALLY RETESTED for CREATE-08. Existing-board dismissal paths were subsequently tested under CONFIG-14 (635–656).

### ISSUE-005 Root task is labeled as its own parent

- [x] Manual observation: newly created root Story has no parentTaskId, but its group details show its own title under Parent (201).
- [x] Diagnosis: kanbanTaskBreadcrumb includes the current task; both detail renderers label that complete path Parent.
- [x] Narrow correction: both headings now say Hierarchy path. Breadcrumb regression suite67/67 green, including four new root/nested/missing/cyclic checks. No hierarchy data was changed.
- [x] Rebuild and root retest: APK72 root Story now says Hierarchy path and shows the correct description/origin (262).
- [x] Nested task renderer retested with read-only response fixtures: blocked and ready child details correctly show parent > child under Hierarchy path (362,364), while group details use the correct root path (366,369).
- Status: LABEL FIXED AND MANUALLY RETESTED in both renderers. Complete DETAIL-02 action coverage remains pending; source regression checks also cover deep/missing/cyclic paths.

### ISSUE-006 Creation summary misses model callback refresh

- [x] Source audit: provider and main-model callbacks mutate stage routes without refreshing the summary. Both callbacks now refresh it.
- [x] Controlled manual reproduction on APK70: explicitly set named Breakdown and Implementation routes (251), Save shows2 (252); change the main model (254) and the summary incorrectly retains2. Reopening confirms Breakdown cleared (255); only explicit nested Save refreshes the count to1 (256).
- [x] Independent manual retest on APK72: provider switching immediately updates route counts2→0→2 (293–297); main-model change immediately changes2→1 (298–299), and reopened settings confirm Breakdown cleared/Implementation reassigned with other saved choices intact (300).
- Evidence limitation: screenshots211/213/214 do not establish a wrong count. Claude's nonblank minimax-m3 default is displayed as CLI default and legitimately counts as one route; screenshot217 still shows one after explicit Save.
- Status: FIXED AND MANUALLY RETESTED for creation summary callbacks. Existing-board model semantics remain a separate CONFIG-03 subcase. No board was created in this reproduction.

### ISSUE-007 App-building scope misclassified as Manual Test

- [x] Manual observation: B's Pocket Notes app-building prompt creates a Story with a Manual Test badge (221–225); persisted taskType is manual_test.
- [x] Diagnosis: legacy inference scans the entire description for manual plus verify. The acceptance sentence then manually verify the complete workflow triggers it even though the title requests building an app.
- [x] Narrow fix and red/green regressions: primary implementation intent is not overridden by later manual-acceptance wording. Two red failures reproduced; nine distinct focused checks passed after repair, including genuine verification and historical-card controls. Full Kanban suite297passed/one existing native-RAG test ignored.
- [x] Deploy and independently repeat creation: fresh C uses the identical Unicode prompt after deployment (313–315). Card correctly shows Story separately from Implementation; persisted level=story, taskType=implementation, executable=false. Original B remains unchanged as pre-fix evidence.
- Status: CLASSIFICATION FIXED AND MANUALLY RETESTED. No planning/execution was started in this retest; downstream execution is not claimed. Remaining VIEW-02 hierarchy/kind combinations still need coverage.

### ISSUE-008 Stage model descriptions are clipped

- [x] Manual observation: upper halves of the stage-help text are clipped consistently in creation settings on APK70 and72 (249,270,308).
- [x] Diagnosis: negative top padding moves10sp text above its own drawing bounds. The same pattern exists in both creation configuration and existing-board strategy dialogs.
- [x] Narrow fix: use nonnegative top padding in both renderers; APK73 full shared suite555/555 green. Installed verified x86_64 build321; actual rendered retest pending.
- [x] Build/install APK73 and inspect all six descriptions in both dialogs: existing-board326–327 and creation342–343. Breakdown text and the wrapped Final QA description are fully readable; scrolling does not clip their glyphs.
- Status: FIXED AND MANUALLY RETESTED. APK73 ARM64 delivered and remote size/MD5 checked.

### ISSUE-009 Lost creation response leaves a misleading empty board

- [x] First manual test: D's valid Codex/MiniMax-M3 request was forwarded once, but its response was intentionally dropped by the scoped relay (347–349). Telemetry confirms dropped_after_upstream, no forwarding error. Server persisted one paused board and one Backlog Story.
- [x] Observed failure: settled Android still says No board for this project with Create enabled, no persistent explanation and no automatic reconciliation GET. A real user could reasonably submit again.
- [x] Recovery control: manually pressing Refresh discovers the existing board and original task exactly once (350–351). No second Create was submitted; duplicate execution is not a claimed observation.
- [x] Source fix: reconcile the current project's authoritative state once after uncertain creation, never repeating the POST. Retain original-error guidance; disable stale Create if recovery is unverified. Initial regression8/11 failing; additional path-roundtrip regression1/22 failing; final creation22 + refresh19 + semantics71 =112/112 green. APK74 and manual retest remain pending.
- [x] Independent lost-response retest on APK74: fresh project E created through UI500–515. One Create516 is forwarded once and its response is dropped. Without manual Refresh, the actual one-feature paused board appears with persistent original-error guidance517. Task details518 preserve the complete unique prompt; closing details519 retains the warning. Read-only persistence confirms exactly one task, no execution attempts. Repeated failed-read recovery control remains pending.
- [x] Repeated recovery failure retest: fresh F UI520–534, one lost POST followed by two scoped GET503 responses. First failure535 removes stale Create and explains the unverified outcome. Explicit Retry536 retains the original POST error and replaces only the latest GET error. Next Retry537–538 reads the real saved board, removes stale read failure, and retains review guidance. Details539 and read-only persistence confirm the unique full prompt, one Story, paused/inactive, zero attempts. No duplicate POST in relay telemetry.
- Status: FIXED AND MANUALLY RETESTED on APK74, including successful and failed reconciliation. Original D and new E/F boards preserved; tests cover creation, not every task-mutation retry path.

### ISSUE-010 Supported Fix work is labeled Planned

- [x] First manual test: read-only canonical taskType=fix response renders Subtask · Planned in the nested row (381) and a Planned badge in task details (390). Origin remains correctly User prompt-generated.
- [x] Contract check: fix is a supported canonical server kind and qa_fix alias; shared kind detection recognizes it. The label function lacks the fix branch and falls through to origin labels.
- [x] Narrow label correction: canonical fix/qa_fix displays Fix, while legacy QA-fix flags retain QA Fix. Two semantic tests failed before repair; all71 semantic tests and combined112 targeted tests now pass. APK74 and manual retest remain pending.
- [x] Rebuild APK74 and repeat the exact original hierarchy fixture: row497 and detail498 correctly show Fix, retaining level/path/origin. Additional legacy QA Fix control482/486 remains correct.
- Status: FIXED AND MANUALLY RETESTED. This is a rendering defect, not evidence of execution routing failure.

### ISSUE-011 Legacy acceptance criteria disappear from details

- [x] First manual test: metadata fixture supplies acceptance_criteria, but the complete group detail dialog omits Acceptance criteria while correctly showing legacy level, Review kind, and User manual origin (423).
- [x] Source corroboration: shared semantics recognizes the alias, but both native group and leaf detail renderers read only acceptanceCriteria.
- [x] Narrow alias-aware extraction fix with canonical/malformed controls: genuine one-of-eight regression failure, then8/8 green. Both native renderers use the shared accessor with canonical precedence.
- [x] Rebuild APK74 (592 full tests pass) and independent manual retest: canonical root criteria483, legacy nested Fix criteria484, canonical QA Fix criteria486, and the unchanged original legacy group now correctly displays its missing criterion492.
- Status: FIXED AND MANUALLY RETESTED. No server data was changed; this is supplied-response rendering coverage.

### ISSUE-012 Unknown status is misleadingly shown as ready Todo work

- [x] First manual test: supplied unsupported status is labeled Todo in its card/details, beneath Ready to start425–426,491. The item is excluded from selection; this is a presentation defect, not an observed permission bypass.
- [x] Narrow repair: neutral Unknown badge, raw unsupported value in details, warning on affected mixed groups, and qualified Todo subtitle. Existing column membership/counts, rollups, eligibility and action predicates are unchanged. Genuine RED13 tests/9 failures, then108 targeted tests pass; independent source review found no blocker.
- [x] Rebuild/install APK75 and independent manual retest: original unknown card545 has neutral Unknown badge/warning and qualified Todo subtitle; detail546 shows the original raw status. Legacy pending control548 retains Todo, Review, User manual and restored criteria. Mixed group552–553 retains In Progress, distinct Todo/Unknown children and identifies the unsupported child; leaf554 exposes Status received: TEST-future-child. Counts and selection eligibility remain unchanged.
- Status: FIXED AND MANUALLY RETESTED. Unknown-only work remains in the compatibility Todo container/count but is no longer labeled ready. Synthetic responses prove client presentation, not live scheduler behavior.

### ISSUE-013 Generated test tooling violates the user's explicit package restriction

- [x] First manual test: real Codex/min:MiniMax-M3 run from MANUAL01 task-2. Original details583/587 explicitly say “No backend, external services, packages, deployment, or accounts.” Generated test task598 instead requires package.json, @playwright/test, playwright.config.js and npm installation. Parent/child relationships and tests→implementation→QA dependency order exist, but inherited scope is incorrect.
- [x] Containment requested: Pause reason593–594 submitted595. Board correctly reports Pausing596–597 while the already-started harness attempt1 finishes; this is not yet a fully paused board. Execution began during the interruption before Pause confirmation. Package files, tests and node_modules now exist in the disposable project and are preserved as evidence. No claim that the pause prevented writes.
- [x] Narrow repair and source regressions — inherit restrictions through actual parent lineage; reject recognized conflicting package operations before provider dispatch, breakdown, or child persistence; reject forbidden npm dependency/lock additions in completion evidence. Strict TDD remains enabled. Three RED cycles exposed original, review-boundary, and candidate-ID faults; final Kanban suite313passed/0failed/1pre-existing ignored native-RAG test, including16new scope checks. Independent review passed; ten-file source freeze recorded in issue013-fix-evidence.json. This is a bounded scope check, not a command sandbox; unannounced writes are not prevented universally.
- [x] Deploy — guarded binary-only replacement verified15:58:28WIB; source/candidate/rollback manifests and A–G snapshots checked, no data restoration.
- [ ] Fresh manual retest — G74e96737-3904-4518-84cc-4274e12995ab created through UI659–677 with identical original board prompt/Minimal/Codex/MiniMax/strict TDD. One unapproved Backlog story and no files before manual task submission. Original A plan/files preserved. Post-deployment scope retest pending.
- [x] Stop the out-of-scope execution: Manage→Abort confirmation601, reason602–603, submit604. Settled605 and later623 show Cancelled/no running task; saved status remains cancelled/active=false/currentTaskId=null. The interrupted harness ultimately became blocked, attempt1, with package.json/package-lock.json/playwright.config.js/tests/shell.spec.js recorded as changed. These files are retained; no shell implementation was generated or independently tested.
- Status: OPEN. Scope failure confirmed in the native task description and actual workspace; implementation of the requested shell is not verified.

### ISSUE-014 Transcript redaction hangs before saving generated children

- [x] First manual observation after013deployment: G exact task scope verified685, submitted once686, running/recon687 then hierarchy planning691. The real provider returned a four-task plan09:04:49UTC, but Todo702 and details703 still show no nested work. Explicit Refresh705 and settled706 do not recover progress; saved board remains running/active with only two original tasks and no provider call.
- [x] Diagnosis and bounded reproduction: transcript recording runs an existing redactor under the global board mutation lock. The new013 prompt's ordinary phrase “scope authorization” exposes a loop that re-finds its own replacement at the same marker instead of advancing. A synthetic four-iteration reproduction repeatedly adds closing brackets; no real credential was used. This is a013 regression trigger in an existing helper, not model noncompliance.
- [x] Narrow fix and regressions — monotonic scan cursor, already-redacted/idempotent values, continued scanning after empty values, empty-marker guard. Genuine bounded isolated-child RED timeout, then315 Kanban tests pass/0fail/1pre-existing ignored native-RAG test; tests cover repeated markers, mixed case, UTF-8 neighbors, structured keys and the actual ordinary scope sentence. Broad conservative redaction syntax remains unchanged. Source freeze/evidence in issue014-fix-evidence.json; independent source review passed.
- [x] Build — staged014 candidate SHAa650f99cb2efcd823acc8a4139a87ca02092c90636887529f01d515d8e39cd8a, single-job20.43seconds;478inputs/12critical files stable. New scoped runner independently reviewed; rollback targets current9749ba04. No source changes from other work discarded.
- [x] Deploy and independent native retest — combined candidate d0c8ed3f verified23:36:20WIB. Android restart502745 → Retry746 → recovered planning747. G automatically regenerated its plan; actual saved children appear754–755, followed by saved Task→Subtask breakdowns and first executable attempt. No task creation was resubmitted. Schedule retest754 now returns409 promptly; schedule remains null. Original transcript/save hang is resolved in the real workflow.
- Consequence: Schedule Save700 while G active remained in flight and eventually produced relay502 after180seconds09:09:28; it was not an observed409 rejection. Stored scheduledStartAt remains null/controlRevision1. Android returned to a plain board701 without persistent error; transient-toast visibility is not established by these screenshots.
- Status: FIXED AND MANUALLY RETESTED. The saved plan uses built-in node:test and real dependencies; independent issues with unnecessary scope wrappers are tracked as015, and app completion remains pending.

### ISSUE-013 follow-on: negative comma-list loses its governing prohibition

- [x] Read-only provider-output diagnosis: the recovered G plan explicitly says “Do not introduce package.json, dependencies, devDependencies, npm install, or any third-party package.” The package detector split this list and incorrectly treated its bare npm-install entry as a positive instruction. This was found before deployment, not observed as a native blocked-card failure.
- [x] Genuine failing regression using that exact sentence, then a narrow correction: carry explicit package prohibitions across known bare list entries, but reset for new instructions, contrasts, shell conjunctions and sentence boundaries. No prompt rewriting or weakened TDD/scope policy.
- [x] Independent source review and final regression suite:316passed,0failed,1existing ignored RAG integration test, including17package-scope tests. Evidence: issue013-negative-list-fix-evidence.json.
- [x] Combined013follow-on/014 deployed; native saved children754–758 and actual no-install test authoring779–780 observed. The prohibition no longer blocks saving this plan. Full app execution is separately blocked by016; no overall completion claim.

### ISSUE-015 Explicit exclusions falsely create new approval scope

- [x] First manual test — after014repair the requested index.html task appears as a separate Backlog Story756, despite matching the approved original request. Native details757–758 show the correct static HTML/CSS scope, no scripts/packages/persistence/interactions and the sentence “Persistence and task interactions are explicitly out of scope for this task and must not be implemented.”
- [x] Root cause — required=true, sideEffects=[] and plannedFiles=[index.html] did not request expanded work. Server generated_child_requires_backlog_approval matches the bare phrase “out of scope,” misreading an exclusion as expansion. A second Task→Subtask breakdown repeats the same false wrapper for the requested HTML leaf. Dependencies are remapped correctly, but parentage/approval flow is unnecessarily split.
- [x] Real-user workaround observed — reviewed wrapper Story760, Actions761, Move2toTodo762, approving only the already-requested shell parent/child. Original whole-app task-1 remains unapproved Backlog. This is not a fix or a successful retest of015; the second wrapper still requires attention.
- [x] Narrow source fix — recognize only the two complete observed negative clauses, with and without “for this task.” Original RED4tests/2fail, second actual-variant RED1fail; final321pass/0fail/1existingignore. Real expanded/optional/external-risk approval gates retained and independently reviewed. Final source freeze: issue015-final-source-freeze.sha256.
- [x] Deploy — combined015+016 verified3October00:23:01WIB. Existing G wrappers remain preserved, not retroactively rewritten.
- [x] Independent fresh-plan retest FAILED — H first plan890 was rejected before routing by018. After017018deployment, unchanged retry911 saves five tasks916 but again adds two artificial approval Stories. Manual wrapper917 explicitly says not to exercise interactions because they are out of scope; HTML wrapper920 explicitly excludes interactions/persistence. Both are required with no external side effects. Result8records/3Stories instead of one Story with five direct Tasks; no app files or execution.
- [x] Follow-on source correction — bounded negative declarations/instructions and clause boundaries cover the actual H variants, while genuine expansion/optional/external gates remain. Genuine RED9tests/3fail; combined final372pass/0fail/1existingignore and peer approval. Combined source manifest issue015019020-source-freeze.sha256; deployment and fresh native plan retest still pending.
- Status: REOPENED AFTER FAILED MANUAL RETEST. Earlier literal-clause correction was too narrow. G/H existing wrappers remain preserved as evidence.

### ISSUE-016 Read-only expected-red capture cannot complete in a package-free project

- [x] First manual test —779–783 show test authoring and its QA Done, but the explicit read-only baseline-capture subtask Blocked after attempt1. Its scope forbids changing any source. Actions correctly does not present transient Retry for a quality/evidence failure.
- [x] Diagnosis — provider claimed ten expected assertion failures, but server discovered zero validation commands without package.json. It therefore had no independently accepted red baseline, and the generic test-implementation evidence gate also required source edits from this read-only task.
- [x] Narrow source fix — bounded/hash-matched existing Node built-in tests run directly without packages or shell interpolation. No-edit completion is allowed only for explicit read-only capture with current server-verified assertion-red, matching scope/suite, and unchanged complete workspace after validation. Ordinary authors, green/broken/missing baselines, implementation and final QA remain strict.
- [x] Regression/review — genuine RED8tests/6fail; boundary RED11tests/3fail; final332pass/0fail/1existingignored,11new tests. Independent review approved. Evidence: issue016-diagnosis.json and issue016-source-freeze.sha256.
- [x] Deploy — combined015+016verified00:23:01WIB; original blocked G/history/files preserved.
- [x] Independent native retest — exact leaf830, Move to Backlog confirmation832/submit833, preserved Backlog scope839/history840, reapprove once841 without editing. Genuine attempt2Running842–845 then Done846–849. Fresh server-owned node --test --test-reporter=tap command exits1 with ten named assertion failures, expectedRedAccepted=true/gatePassed=true/passed=false and completion evidenceGate passed. Same scopeVersion3, original attempt1 and unchanged test-file fingerprint preserved; no source/package additions.
- Status: FIXED AND MANUALLY RETESTED. Capturing the expected failing baseline succeeded; this is not a green application suite. Project still has tests/checklist.test.js only, with later work subject to ordinary approvals/dependencies.

### ISSUE-017 Successful descendant approval is lost during sequential parent approval

- [x] First manual test — reviewed the old015-generated HTML leaf852–854 and exact matching parent856. Actions857 explicitly offers “Move3toTodo”; submitted once858. Settled859–860 remains Blocked/no task running. Filename859 says running but the actual pixels are Blocked; it is not a successful-execution screenshot.
- [x] Corroboration — transparent relay records task-4/promote20017:34:51.556UTC, task-5/promote20017:34:53.848, then task-4-subtask/promote40917:34:55.042. Saved task-5 is nevertheless Backlog/unapproved; only task-4 stayed approved. Original task1 remained Backlog. No HTML file or executable attempt was created.
- [x] Initial diagnosis — native parent action sends sequential promotions; each can start the scheduler. An older scheduler snapshot is saved outside the board-mutation lock, overwriting newer task approval. File-write serialization/control-field merging alone does not preserve task mutations.
- [x] Narrow failing regression, fix and independent source review — real threaded RED lost an acknowledged promotion; synchronous scheduler preparation now holds the mutation lock through its decision/initial claim and releases it before async work. Three targeted tests pass; full suite353passed/0failed/1existingignored. Independent review approved; current G remains untouched.
- [x] Deploy and repeat normal UI approval — combined017018 verified01:03:59WIB. Unchanged G parent927→Move2toTodo928→submit once929. Both descendant requests succeed;930 shows one actual implementation leaf running. All intended ancestors/leaf retain approval at scopeVersion3; only task-4-subtask has attempt1. Original task1 remains unapproved Backlog/attempt0. No board/file edits by tester.
- Status: FIXED AND MANUALLY RETESTED. Approval retention/executable-leaf scheduling passed. App completion and downstream QA are separate checks.

### ISSUE-018 Correct package prohibition falsely rejects the complete plan

- [x] First manual test — H Story→Break down work890, settled892 still one Story/no children. Details893–894 persist a package-scope-conflict error. The actual Codex/min:MiniMax-M3 plan explicitly requires only built-in Node modules and says “do not introduce package.json dependencies, npm install, devDependencies, jsdom, or Playwright.” This is not a model request to install packages.
- [x] Root cause — the negative comma-list recognizer knew the separate words but not the noun phrase “package.json dependencies,” so the following prohibited npm-install entry was misread as positive.
- [x] Narrow correction and regression — add that exact noun phrase to the prohibition vocabulary. Genuine RED2tests/1failed, then355Kanbanpassed/0failed/1existingignored; ten positive-install/contrast controls remain blocked. No TDD or package restriction waiver.
- [x] Deploy — combined017018verified01:03:59WIB.
- [x] Workflow retry — unchanged H Story909, Break down once911; completed18:08:16UTC, five unique model Tasks saved916, original error cleared. No package conflict or new package files. The regenerated wording differs from the first response; exact original wording remains directly covered by the regression, not a deterministic replay of identical model output.
- [ ] Overall plan acceptance — separate015parentage failure remains; it does not mean018 blocked this retry.
- Status: DEPLOYED; unchanged failed-planning workflow recovered in the native UI. H remains paused/inactive with no execution attempts or files.

## Generated-app checkpoint, 3 October

G's approved static shell was genuinely generated by Codex/MiniMax-M3, not written by the tester. Implementation and automatic QA each passed server-owned10/10Node assertions. Independent Playwright checks935–945 passed at360×800,414×896 and1280×800: correct labeled controls/counters/empty list, no horizontal overflow,44px control heights, keyboard focus and emulated-touch input focus. Add deliberately does nothing because interaction/persistence were explicitly excluded. A favicon404 is recorded as minor polish, not concealed. Evidence: `artifacts/kanban-manual-20261001/g-generated-app-independent-manual-check.json`.

### ISSUE-019 Completed generated wrapper leaves its original prerequisite permanently unfinished

- [x] First manual observation — actual implementation+automaticQA Done946, while source implementation parent remains Todo941 and downstream QA remains Blocked by task-3-task951. The original requested shell file exists and passes tests; this is not a provider failure.
- [x] Read-only diagnosis — planning recognizes generated wrapper metadata as existing children, preventing re-planning, but completion rollup considers only direct parentId children. The source task has none after its required work was moved into the separate wrapper, so completed wrapper descendants never satisfy it.
- [x] Conservative regression/fix/review — actual nested-G RED and boundary RED captured; combined final372pass/0fail/1existingignore. Peer-approved childless-orphan recovery requires current matched required wrapper provenance, original wrapped child, approved completed subtree, real current-scope Done attempts and accepted evidence including required QA. No executable history or data backfill. Existing Todo-as-approval normalization is unchanged, explicitly not advertised as a new authorization-provenance system; see issue019-fix-evidence.json.
- [x] Deploy — combined015019020 verified01:56:44WIB, independently reverified after host reboot16:15WIB; no automatic G execution or changed records before the retest.
- [x] Native retest — Start1011 opens confirmation1012 with Codex/min:MiniMax-M3/no fallback; Apply1013 submitted once, resume200 at09:25:22UTC. Source task-3-task becomes Done at09:25:24, both true dependencies release, QA actually runs1014, and source group1016 now reads Done1/1. The completed HTML/QA records and unrelated unapproved Story are byte-equivalent JSON to their predeployment snapshots; no rerun, no parent attempt, no evidence rewrite.
- [x] Downstream check — green-baseline QA genuinely finishes with ten passing assertions, then manual-verification leaf actually runs1017. Independent browser checks were not used to mark that leaf Done.
- Status: FIXED AND MANUALLY RETESTED for the observed current-scope orphan completion defect.

### ISSUE-020 Dependency error continues to list an already-completed prerequisite

- [x] First manual test — QA waiter955 displays “Waiting on dependency: task-2-task-3, task-3-task.” Test parent task-2-task-3 is already Done and unsuperseded, visible954; only task-3-task remains unfinished.
- [x] Diagnosis — initial blocker text is computed from unmet dependencies, but reconciliation clears the text only when all complete; it does not refresh a partially reduced list. This is misleading error text, not an observed scheduling bypass.
- [x] Narrow source correction — recompute only the current dependency reason while some dependencies remain unmet; attempts, approval and state are untouched. Genuine RED3fail/2pass → GREEN5/5; peer-approved combined372pass/1existingignore.
- [ ] Deployed native partial-resolution retest — pending. If019 releases G's last blocker and clears the whole error, that alone does not prove020's partial-list branch.
- Status: OPEN. G error/state retained; no transient retry submitted.

### ISSUE-021 Accepted expected-red baseline is labeled as a failed baseline

- [x] First manual test — G's completed baseline leaf962–964 has preserved Attempt1 Blocked/Attempt2 Done and exit1 assertion evidence. Horizontal chip scroll966 exposes the group label `Baseline failed`. Current persisted baseline explicitly has `expectedRedAccepted=true`, `gatePassed=true`, `passed=false`; the task completed correctly, but the card reduces these fields to `passed` alone.
- [x] Confirm cause — native board-screen.kt computes baseline booleans and maps any false to `Baseline failed`, discarding expected-red acceptance. This is not a request to turn failing tests green or erase earlier failures.
- [x] Source fix — evidence-aware shared baseline aggregation and task-detail summaries preserve actual failures/unknown states, accepted expected/remaining red, latest validation and separate history. Genuine RED11tests/8fail → GREEN18new+71existing=89/89; independent peer approved. APK77 build/manual retest pending.
- [x] Deploy — immutable APK77 installed on restored5594, package/version confirmed;636full shared tests passed. ARM64 upload verified at https://gofile.io/d/YDxtWBxp (10,206,038bytes, MD52ba8dff7aec0b4cf1da2b3b52755ab91).
- [x] Manual retest — same saved real baseline1006 now clearly says Accepted expected red (tests remain red), separates latest validation and retains History:1failed gate/1accepted red. Group badge1009 correctly says Baseline accepted(expected red), not failed or green. Native1004/1005 captured an independent horizontal-offset reset, not an incorrect new verdict; tracked as023.
- Status: FIXED AND MANUALLY RETESTED for the accepted-red display defect; history/evidence retained.

### ISSUE-022 Completed prerequisites are still labeled as current blockers

- [x] First manual test — Done baseline leaf963–964 says `Blocked by` its author prerequisite, even though that author is also Done and no longer blocks execution.
- [x] Cause — both native detail renderers label retained declared dependency edges as current blockers without resolving their state. Those stored edges are intentional history, not records that should be deleted.
- [x] Source fix — both preserved dependency sections now say `Depends on`; existing separately computed current-waiting indicators remain unchanged. Exact two-label review completed; no native test harness existed, so source checks are not claimed as unit or manual passes. APK77 compilation pending.
- [x] Rebuild/manual retest — APK77 same completed G leaf1007 correctly says Depends on without removing the edge. Independent F root-group control1031–1033 has unfinished Backlog prerequisite task-1 and also correctly says Depends on; title, P1, Story, Backlog, root breadcrumb and User prompt-generated origin match saved records. F remains byte-identical and unexecuted.
- Status: FIXED AND MANUALLY RETESTED in both native detail renderers with completed/unfinished dependency controls.

### ISSUE-023 Polling resets a horizontally scrolled group signal row

- [x] First manual test — APK77 G group1009 immediately after a horizontal swipe shows the full accepted-red badge. With no further input, successful ordinary polling resets the row to its left edge1010, hiding the text under inspection; vertical offset remains stable.1004XML also had moved bounds while its later screenshot already showed the reset.
- [x] Diagnosis — board refresh saves only outer vertical position, rebuilding each horizontal signal row at scrollX0.
- [x] Fix/regression — scoped project/board/column/group/attention-section offsets captured from the existing view; pending offsets survive hidden-row refresh and clamp after measurement.18targeted tests/Android compile/independent peer passed. Initial manual1009–1010 is the genuine failure; new helper tests are not falsely described as an automated RED run.
- [x] Rebuild and independent native retest — APK78 installed1052,658full shared tests pass. Horizontal swipe1064 exposes accepted-red badge; settled XML bounds185,1238–543,1284 survive multiple successful ordinaryGET polls through1065. New user swipe1066 changes badge left to377; several more polls preserve that new position1067, with the same vertical task bounds. Column switch1068 starts Done at its own top.
- Status: FIXED AND MANUALLY RETESTED for the observed visible-row reset and subsequent-user-scroll branch. Hidden attention rows and dynamic content shrink have automated coverage but are not claimed as native observations.

### ISSUE-024 Invalid generated manual-verification contract is accepted until completion

- [x] First manual test — real G manual leaf executes at1017 and becomes Blocked1018–1021 after producing screenshots. Parent and leaf both store `viewports:["360x800","414x896"]`; completion requires objects containing positive width/height. Invalid scope was persisted and executed instead of being normalized or rejected before the run.
- [x] Diagnosis — ingress stores raw JSON while completion uses a strict typed parser. The provider also returned independently invalid evidence: UI checks use unsupported inspection-only actions and the second viewport lacks most criterion checks. Those defects must not be concealed by merely normalizing the contract.
- [x] Fix/regressions — normalize only unambiguous dimensions at new ingress, reject malformed supplied scope atomically, and preflight legacy invalid manual scope before dispatch. Six initial genuine failures become green; final combined401pass/1existingRAGignore includes20contract/artifact/prompt checks and9draft-acceptance checks. Independent peer/frozen hashes verified. Historical records and strict runtime-evidence checks preserved.
- [ ] Deploy and fresh native scope-reapproval/retest — pending.
- Status: OPEN. Real screenshots exist, but this attempt is not a verified completion.

### ISSUE-025 Playwright-generated runtime snapshots are classified as forbidden source edits

- [x] Follow-on attempt3 —legitimate timestamped .playwright-mcp console log rejected. Narrow creation-only bounded console-diagnostic allowance implemented with timestamp/content/hash/baseline checks; logs do not count as screenshot evidence. RED then10 MCP tests pass, included406-test candidate. Deployment/manual retest pending.

- [x] First manual observation — G manual result1021–1022 fails file policy for three `.playwright-mcp/page-<timestamp>.yml` files created by browser tooling. App HTML and Node tests were unchanged; four PNG artifacts were saved in the approved manual-evidence directory. One snapshot is empty and two contain the accessibility snapshot.
- [x] Narrow repair/regressions — allow only attributable newly-created, timestamped, bounded accessibility-snapshot files in complete non-Git snapshots, bound to task/scope/attempt identity captured before execution. Symlink/source-file/reuse/stale identity controls preserved; automated tests pass with the combined401suite. No blanket permission for manual-test source edits.
- [ ] Fresh native retest — pending.
- Status: OPEN. This is separate from the invalid contract and incomplete UI evidence.

### ISSUE-026 Failed manual completion is summarized as a passed gate

- [x] First manual test — native1021 shows Blocked and a completion-evidence error, but its new Validation outcome reads `Latest validation: Passed` and `Validation history: Latest gate passed`. Read-only record has deterministic tests passed, but both evidenceGate and manualEvidenceGate failed.
- [x] Diagnosis — deterministic command success must remain distinct from the task's completion/manual-evidence verdict. The board summary1018 also presents the latest deterministic gate as passed.
- [x] Fix/regressions — display server completion/manual/file-policy verdicts independently before explicitly automated results/history; missing/invalid evidence is never inferred from status/provider claims. Genuine RED9fail/1pass → GREEN99/99, independent review passed. Header also explicitly says Automated validation.
- [x] Rebuild/install — immutable APK78/0.1.78-kanban-scroll-preservation installed1052, SHA f504e807ee160f9088cb9e10aff1f1f55f329eff207f9a1c59487d495ff97cb9.658full shared tests pass; input manifests stable. ARM64 https://gofile.io/d/vrupcLfZ verified10,206,042bytes/MD5fe53695762837778f9efd3ae61b57cd1.
- [x] Same-record native retest —1058 clearly separates failed Completion evidence, Manual evidence and File policy from Passed automated validation; task remains Blocked/attempt1. Real green QA control1060 shows completion/file-policy/automated pass. Accepted-red control1062 still says tests remain red and preserves earlier failed-gate history. Header1056 is explicitly automated.
- Status: FIXED AND MANUALLY RETESTED. Successful automated assertions no longer imply accepted manual completion in task details.

### ISSUE-027 Accepting a generated preview loses dependencies and generated origin

- [x] First manual test — F exact prompt1039 explicitly depends on board:task-2. First Preview1042 is sensible and Close1043 saves nothing. Repeated Preview1044 returns one new draft1046; actual Codex/MiniMax session session_41622a85616f48488a2061cc5f940ccd includes dependsOn:[board:task-2].
- [x] Accept/duplicate control — rapid Add drafts taps1047 yield exactly one POST/tasks201, one new task-3 and three total Backlog Stories1048. Board remains paused/inactive with all attempts0.
- [x] Failure inspection — details1049–1050 label this generated task User manual and omit Depends on; read-only persistence confirms blockedBy=[]/dependsOn=[] while references still mention task-2. Description and acceptance checks survive, but a reference is not an execution dependency.
- [x] Cause corroboration — prompt drafts use planDependencies and empty blockedBy pending canonical resolution. Android routes Add drafts through ordinary manual-task creation, which discards those planning fields and assigns manual origin.
- [x] Narrow fix/regressions — one canonical atomic preview-acceptance endpoint preserves dependency remapping, wrapping and generated origin without changing manual creation or approving/starting execution. Genuine serverRED5/clientRED1 then serverGREEN9/client67GREEN, Android compile and full659shared tests pass. Peer/frozen input verification passed. Explicit repeated submissions are not idempotent; ambiguous-response behavior is still untested.
- [x] Deploy and independent existing-board dependency retest — verified deployed binary and APK79. Fresh preview1117 → one Add drafts1118 → four Backlog tasks1119. New task4 shows User prompt-generated1120 and Depends on task-2 with seven criteria1121. Refresh1123/reopen1124–1126 preserves both. Read-only resume1004-f-issue027-accept-readonly.json confirms exact preview fields, one canonical POST201, no legacy manual POST, unchanged original three records and no execution.
- [x] Local/forward branch — actual preview1138 returns bounds-warning first, depending on plan:bounds-config and board:task-2; bounds-config second depends on board:task-1. One acceptance1139 yields six Backlog tasks1140. Native1141–1145 confirms generated origin and canonical dependencies task5→task6/task2, task6→task1. Refresh1147/reopen1148–1149 retains forward edge. Read-only resume1004-f-plan010-accept-readonly.json confirms preview fidelity, one canonical POST201 and no execution.
- Status: FIXED AND MANUALLY RETESTED for existing-board and forward/local dependency acceptance. Ambiguous-response retry remains a separate untested branch.

### ISSUE-028 Provider verifies an altered HTML copy instead of the actual artifact

- [x] Attempt3 scope5 —real18104HTTP200/2082bytes/hash verified and16 real-page screenshots. Correctly Blocked for evidence scopeVersion4 rather than5 and runtime console log file policy. Audit also finds synthetic focus/events substituted for failed real touch; successful manual interaction still UNVERIFIED. Old screenshots renamed to legacy paths by provider, requires evidence-path repair. Native1457–1460 confirms Blocked/failed evidence, actual automated gate Passed, all attempt records retained.

- [x] Scope4 execution retest —1367–1370 promoted exact leaf; attempt2 ran12:12:37–12:14:57UTC, then Failed/boardBlocked. Provider launched8765within one shell, curl/hash passed within that command but browser later refused connection. No independent post-launch liveness check; process trees do not prove network isolation. Provider correctly returned blocked without dataURL/reconstructedHTML/new false screenshots, so safe-failure branch passes; successful real-artifact verification still pending. Original scope3attempt retained. Audit g-attempt2-scope4-readonly-audit.json.
- [x] Native result review —1394Failed child,1395automatedPassed explicitly distinguished,1396both historical attempts,1397attempt2 report readable.1401–1403 move exactleafBacklog to revise;1414edit-scope request;1433review/1434apply/1435description confirms persistent18104URL and no server start/stop. Scope5 remains unapproved until promotion; no source edits requested.

- [x] Manual finding — provider screenshot of G's static shell had 66px controls while independent actual-file checks935–945 had44px controls. Decoding the recorded browser navigation data URL proves it is not the generated file:2077bytes/SHA2566a267e064801f72188db281fb3c9e648806b8dd31d890f36753306c07f555312 versus actual2082bytes/SHA25686edba34d90ca20f2c074840c75be9abd7c94f82ff99a489d26832beee56d4d3. The copy changes the box-sizing selector and100vh to10vh, among other whitespace differences.
- [x] Disposition — original attempt remains failed; no source file or historical evidence is rewritten. This is a provider verification-quality failure, not proof that structural server evidence checks certify screenshot truth.
- [x] Reachability diagnosis — the first provider HTTP server8765 was alive only inside its launch command; the next separate command found no process/listener and curl failed. The later nohup attempt was not independently checked after launch, so its exact lifetime remains unproven. Local Playwright configuration does not establish a container-network restriction. Root's successful18104 tests used a retained foreground command session. Evidence: g-provider-runtime-reachability-diagnosis.json. Fresh testing must verify continued server liveness rather than substituting a copied page.
- [x] Narrow instruction repair — requires actual runtime artifact, exact response-byte versus filesystem SHA for static files, no reconstructed HTML/data URL substitute, and blocked outcome if unreachable. Genuine transforming dev servers require verified build/runtime identity instead of impossible source-byte equality. Regression and peer review pass; this is guidance, not server truth certification.
- [ ] Native scope revision and fresh real-provider verification — pending; no claimed completion from the altered copy.
- [x] Native scope revision — reviewed proposal applied1096; scope4 now explicitly requires the real HTTP-served file/hash, both viewport criteria and blocked outcome on unreachable/mismatched content. Original scope3 failed attempt preserved1102. Fresh execution still pending deployment/reapproval.
- [x] Runtime control — detached root-owned localhost18104 server remains alive across separate calls. Playwright screenshot1104 inspected; exact browser response2082bytes/SHA86edba34d90ca20f2c074840c75be9abd7c94f82ff99a489d26832beee56d4d3 equals both curl and on-disk file. No source edits or data URL. Evidence g-persistent-runtime-identity-1003.json; not a retroactive provider pass.
- Status: OPEN. Independent actual-file verification remains valid, but does not retroactively validate the provider attempt.

## Cases

### ISSUE-029 Numeric configuration silently substitutes invalid input and closes forms

- [x] First manual test — F retry delay10 cleared1188–1190; Save1191 closes dialog and sends update instead of validation. Reopen1193 silently shows10. Retry disabled throughout; no execution. Zero entered1195–1196 and Save1197 also closes form rather than retaining invalid input.
- [x] Diagnosis — retry form uses toIntOrNull fallback10/3; quality settings and creation configuration similarly substitute defaults/previous values. AlertDialog positive callback dismisses automatically. Server clamping does not communicate valid ranges to the user.
- [x] Fix — shared strict numeric validation and native post-show Save listeners across retry, quality/retrieval and creation configuration; all fields validated before mutation/request. Source freeze issue029-source-freeze.sha256; independent review found no blocker.
- [x] Rebuild/install — v80, 663 shared tests pass.
- [x] Native retry-delay retest —1207–1214 blank, zero and2147483648 preserve input/dialog and show explicit range error. Corrected1 saves1216 and persists1218. Restore10 submitted1221. Retry disabled throughout.
- [x] Native quality form —1232–1239 blank and21 Max fixes rejected;0 accepted1241/persisted1243;3 restored1247/reopened1249. Timeout4/3601 rejected1254/1256. Context999/80001 rejected1261/1263. Task attempts0/11 rejected1269/1271. Each error preserves input and dialog. Correct originals reentered, Back1273 cancels remaining draft.
- [ ] Remaining field/form boundaries and invalid branches — retry attempt limit, command/follow-up limits, creation form and malformed/paste branches still pending.
- Status: FIX IMPLEMENTED; MANUAL RETEST IN PROGRESS.

### ISSUE-030 Rapid taps stack duplicate task-detail windows

- [x] First manual test —1199 rapid column changes end in correct Backlog6;1200 triple tap Configure counter bounds stacks details. First Close1201 leaves identical detail visible; second Close1202 finally returns board.
- [x] Fix — Activity-owned detail window/identity, shared group/task guard, same-item suppression, replacement for other detail, identity-safe dismissal cleanup. Independent review and Android compilation pass. Source freeze issue030-source-freeze.sha256.
- [x] Rebuild — v81 both architectures, 663 shared tests pass; ARM64 uploaded with matching size/MD5. Receipt android-v81-detail-dialog-guard/gofile-delivery.json.
- [x] Native original reproduction retest — v81 installed; triple tap same F card1226 opens detail, one Close1227 returns board. Reopen1228, Actions1229, Cancel1230 returns board.
- [x] Individual/navigation retest —1280 triple tap child opens one detail;1281 one Close returns board.1282–1286 child Actions→parent→direct child then Close releases identities correctly.1288–1290 parent Actions→Open nested work opens the correct child.1305 transcript Back returns directly to board.
- [ ] Remaining native retest — actual Activity recreation pending;1306–1308 Home/return preserved detail, but Android reported task brought to front, so this is not claimed as recreation. Temporary always_finish_activities restored to original absent setting.
- Status: FIX IMPLEMENTED; MANUAL RETEST PENDING.

### ISSUE-031 Assistant JSON transcript hides literal HTML tags

- [x] First manual test —1303/1304 assistant JSON omits `<label>`, `<button>`, and `<ul id='task-list'>` from recorded test instructions.
- [x] Diagnosis — read-only A persisted task `task-2-task-2-tests`, transcript[1].content retains all three literals; assistant content is valid unfenced JSON and is passed through Markdown rendering. Prompt plain text displays tags correctly.
- [x] Fix — strict standalone JSON object/array detection before Markdown section parsing; preserve exact source as selectable literal text. Focused regression tests cover tags/backticks/arrays/whitespace and non-JSON fallbacks.
- [x] Rebuild — v82 both architectures;674 shared tests pass. Source freeze issue031-source-freeze.sha256; upload receipt android-v82-literal-json-transcript/gofile-delivery.json.
- [x] Native retest —1326 visibly retains all three HTML literals and inline backticks.1327 long-press selects `<label>` and exposes Copy/Share/Select all;1328 Back dismisses selection.1329 scroll reaches full JSON tail and abort record;1330 Back returns correct board without stacked detail.
- Status: FIXED; ORIGINAL REPRODUCTION RETEST PASSED.

### ISSUE-032 Abandoned board read indicator leaks onto other screens

- [x] First manual test —1347/1348 Projects retains Loading board while old A read held;1355/1356 Projects retains Refreshing board while F read held.1358 already-loaded A still displays F's Refreshing board until release1359.
- [x] Diagnosis — Activity-global loading token has no screen ownership; stale board data itself was correctly rejected.
- [x] Fix — scoped read-only board indicator ownership, preserving unrelated mutation feedback. Independent review found two missed refresh callers; corrected header Refresh and selected Board navigation in v84. All six foreground read entry points now scoped.
- [x] Rebuild — v84 /0.1.84-all-board-read-loading,679 shared tests pass; both APK signatures/architectures verified. ARM64 https://gofile.io/d/nNuTdVdp,10,222,414 bytes and MD5 b9fbfee77f9a545297d306209cf7796c match upload. Installed x86_64 on5594.
- [x] Native retest initial read —1374 held real A response;1375/1376 Projects has no abandoned indicator.1377 newer F read held;1378 explicit late A delivery leaves F title and its own Loading board intact.1379 release F shows correct Paused/Backlog6 with indicator cleared. Telemetry confirms A34930bytes/154.689sec and F8100bytes/75.326sec fully delivered.
- [x] Native retest header refresh —1380 F header Refresh shows indicator;1381/1382 Projects has none;1383/1384 A opens and loads correct Cancelled/Blocked1 without F indicator;1385 remains correct. Header request actually disconnected after45.853sec (BrokenPipe, delivery_completed=false); this branch proves indicator retirement, not late-response rejection. The initial-read overlap above supplies confirmed late delivery.
- [x] Native retest shared current-page refresh —1386Ctrl+K,1387 search Refresh,1388 Refresh Board with held real A response;1389Back Projects has no indicator;1390 remains Projects on explicit release (34930bytes/90.391sec/full delivery). This exercises refreshBottomNavigationPage through the command palette; the board footer has no Board tab. Independent v84 audit verified all six foreground read callers and frozen hashes.
- Status: FIXED; original, header, overlapping-read, and current-page refresh reproductions pass.

### ISSUE-033 Command palette leaves the entire content area grey

- [x] First manual test —1388 Refresh Board from Ctrl+K palette,1389–1395 board/Projects content remains grey while bottom navigation retains its normal color.1390 XML shows focused inert FrameLayout spanning0,0–720,1504; no dialog window remains.
- [x] Diagnosis — screenHost receives focus when keyboard/palette closes; Android default focus highlight paints its whole area. This is unrelated to the scoped loading indicator.
- [x] Fix — disable default focus highlight on screenHost only; preserve focusability/touch-mode focus and actionable controls' focus indicators.
- [x] Build —85/0.1.85-screen-focus-highlight;679 shared tests pass, both APK signatures/ABIs verified. ARM64 https://gofile.io/d/b1Cx4T57,10,222,418bytes and MD5 13e3ec6d685d81b97910bac10f83c62e match Gofile. Installed on5594.
- [x] Native original reproduction retest —1420 normal board;1421palette,1422Refresh search retains visible selected command;1423Refresh Board returns normal cream board;1424Back returns normal cream Projects. Focused screenHost remains present, without tint. No blank page or unwanted text keyboard reopening observed.
- Status: FIXED; original reproduction passed.

### ISSUE-034 Board details invents per-command failure from overall task outcome

- [x] Fresh runtime retest —attempt3/scope5 adds exactly one actual deterministic gate,10/10pass; no fabricated commandsRun records. Audit g-attempt3-scope5-readonly-audit.json. Native1458 separates Failed completion/manual/file-policy from Passed automated validation;1459 retains all three attempts;1460 attempt3 error readable.

- [x] Resume verification 4 October —detached deployment completed, live binary0848d854 matches staged candidate;403 server tests pass/1 ignored; A–H unchanged. Native1442 v86 now shows actual latest gates Passed. Fresh scope5 attempt promoted via1445–1448 to test new runtime records; result pending.

- [x] First manual test —1427 G Board details says the deterministic Node command failed, while1395 task detail and1393header correctly say automated gate passed. Actual recorded deterministic run exited0 with10tests passed; manual attempt2 blocked when its new server URL became unreachable.
- [x] Diagnosis — server appends commandsRun strings to validationRuns using overall qaResult for each command. Android details takes last4raw runs and assumes missing passed means pass. This overwrites neither history nor actual gate, but displays false verdicts.
- [x] Server fix — stop creating fabricated per-command validation entries; retain provider command strings in task result/transcript and preserve historical records.2regressions fail before/pass after,385Kanban tests pass/1existingignored. Deployment pending.
- [x] Mobile fix — canonical gate selection before limiting4; legacy command-only records unverified, missing/malformed verdict Unknown.7new regressions,686shared tests pass. APK86 built/verified/uploaded https://gofile.io/d/MblJNkKK (10,222,410bytes,MD5 03ba80eaeb9e12ae9ab486b8f8755a06 matches).
- [x] Native retest and deployed server execution —1442/1458–1460 and fresh attempt3 complete above.
- Status: FIXED; native and runtime reproduction passed.

### ISSUE-035 Board overview multiline values break section indentation

- [x] First manual test —1442 on v86 shows first validation indented and subsequent lines flush left; other headings are also shifted.
- [x] Fix — replace interpolated indented template/trimIndent with explicit lines joined by newline, retaining dynamic multiline content.
- [x] Build —v87/0.1.87-overview-layout;686 shared tests pass; ARM64 Gofile https://gofile.io/d/zbXKjuvk verified10,222,410bytes/MD5 82eb74c7905f1ce17ef996e52e9e0636.
- [x] Native retest —1455 overview all sections and four validation rows aligned, literal content retained.
- Status: FIXED; original reproduction passed.

### ISSUE-036 Manual-test retry uses generic Fix prompt and omits current evidence contract

- [x] First runtime test —G scope5 attempt3 returned stale scope4. Prompt audit proved tdd_phase fix_pending selected generic Fix before manual-test routing;41265-character new prompt contained zero scopeVersion. Provider compaction reused historical4. Completion correctly rejected mismatch.
- [x] Fix —canonical manual-test routing before generic Fix; true implementation QA-fix and finalQA keep correct templates.
- [x] Regression —RED then GREEN covers fix_pending/failed scope5 manual task current contract/schema/runtime policy and genuine QA-fix route. Full server suite406passed/1existingignore. Peer review passed.
- [x] Deployment036 verified live eaa77707; native1509–1517 applies scope6 proposal and starts exact leaf only. Attempt4 actual prompt is Manual Test/current numeric scope6 with preserved8criteria/bothviewports despite fix_pending.
- [x] Runtime routing retest —passed; attempt4 remains Blocked for separate malformed provider scope type (ISSUE037) and unsatisfied real-touch requirement. Console diagnostics allowance passed, no file violations; prior artifacts unchanged. See g-attempt4-scope6-readonly-audit.json.
- Status: FIXED prompt routing; complete manual task verification remains incomplete.

### ISSUE-037 Manual evidence schema example quotes a numeric scope value

- [x] Runtime observation —G attempt4 emitted scopeVersion as string "6"; strict gate correctly rejected it. Prompt example misleadingly used a quoted prose placeholder.
- [x] Fix —render actual current scopeVersion as an unquoted JSON integer. Regression parses example for1/6/42 through fresh/fix_pending/failed routes; RED then24manual-contract tests GREEN.
- [x] Deploy —issue037 deployed and verified on8100, liveSHA9fc6ee413047149a2979f503088b56ca89b547b9c9d0804bb09e995679b0bb26; board snapshots unchanged.
- [x] Runtime retest —native1617/1620 revise/reapprove exact scope6 leaf, attempt5 prompt/output use numeric6; schema gate passes. g-attempt5-issue037-readonly-audit.json records actual evidence.
- Status: FIXED numeric schema. Touch claims remain unproven and are tracked separately as ISSUE039.

### ISSUE-038 Android numeric input silently changes invalid values

- [x] First manual test —1524–1527 retry delay keyboard input -1 becomes1 and1.5 becomes15. Clipboard helper1529 confirms exact -1; native paste1531 again becomes1. No Save submitted, original disabled/delay10/max3 preserved; draft canceled1532.
- [x] Fix —all20Kanban numeric fields preserve text key listener and request numeric IME with setRawInputType; existing field validation receives exact text.
- [x] Initial regression —Android Kotlin compilation and4numeric parser tests pass.
- [x] Native retest —v88 screenshots1546–1556 preserve and explicitly reject negative, decimal, pasted negative, text, and overflow;1560 reopens exact maximum2147483647;1564 restores delay10. Quality Max fixes pasted-negative rejection1568–1570 also passes.
- [x] Build/delivery —v88 ARM64 uploaded https://gofile.io/d/fXUhOLOm;686 shared tests pass. Upload size/MD5 and APK hashes verified in android-v88-numeric-input artifacts.
- Status: FIXED; original native reproduction and retest pass. Broader numeric-field coverage remains CONFIG-13/RETRY-11.

### ISSUE-039 Touch-required manual verification accepts mouse-only evidence

- [x] Runtime reproduction —G attempt5 used mouse browser_click in maxTouchPoints0 environment, labeled actions tap and claimed PASS. Scope-envelope fix037 correctly passed, exposing missing structured modality enforcement. Automatic QA also accepted screenshot/HTTP/source/Node evidence without resolving touch limitation.
- [x] Fix —explicit manualVerification inputModality touch requires positive capability per environment and structured touch observations for claimed pointer actions; at least one valid touch action per environment. Static criteria may use navigation evidence. Inheritance includes manual-test design wrappers.
- [x] Regression —RED reproduced invalid-evidence acceptance, static false rejection, wrapper inheritance;417Kanban tests pass/1existingignored after fixes.
- [x] Final peer review and deployment —receipt DEPLOYED_AND_VERIFIED; October5 live PID765217 SHA256c1ec06125f00ca50dfd04b9af97c6d5dd722207a392e769017d936e05511216a matches candidate. Health/proxy200; native1650–1651 Retry recovers F correctly.
- [ ] Native new linked scope/reapproval and runtime retest —pending; Done scope cannot be edited. Existing Done history is unchanged. Observations remain provider-reported, not authenticated execution receipts.
- Status: IMPLEMENTED; runtime retest pending.

### ISSUE-040 Mixed stage models silently change session policy on Save

- [x] First manual reproduction —F1655–1669 changes six stage model selectors only; Continuous session remains displayed. Save1670; reopen1672–1673 shows Per-task session. Read-only before/after JSON verifies exact6 model values and forced policy change.
- [x] Diagnosis —server intentionally normalizes mixed effective stage models to separate sessions, but native draft never previews/explains it.
- [x] Fix implementation —shared resolver mirrors effective stage model routes; native control shows effective policy and explanation before Save, retaining requested draft policy. Creation summary/payload also aligned.
- [x] Peer review/build —v89, 692 shared tests pass; stable build input manifests.
- [x] Native retest —1699–1701 and1702–1732: mixed policy explanation and restricted chooser; explicit Per-task survives reset to a single route1717; requested Continuous1719 switches immediately when Breakdown differs1722–1723 and returns after reset1726. Save1727/reopen1729 retains Continuous. Hybrid with unknown defaults1731 preserves Continuous and displays server-default caveat; Back1732 discards draft.
- [x] ARM64 delivery —v89 https://gofile.io/d/0EYKt1ag;10222410bytes and MD5 match upload receipt.
- Status: FIXED; native retest passed.

### ISSUE-041 Stage model overrides cannot be reset to default routing

- [x] First manual reproduction —after saving6 overrides, native Breakdown selector1674–1676 scrolls to final option with no default/reset entry. Original5 blank overrides cannot be restored through this selector.
- [x] Fix implementation —board optional stage/cheap/expensive selectors expose Use default routing; main board/chat model selectors unchanged.
- [x] Peer review/build —v89,692 shared tests pass.
- [x] Native reset/save/reopen —1703–1713 reset all five optional stage overrides;1714–1717 restore Implementation;1727 saves,1729 reopens original values. Read-only config02-restoration-1732.json confirms all routes and policy exactly restored; tasks unchanged.
- [x] ARM64 delivery —v89 https://gofile.io/d/0EYKt1ag, verified size/MD5.
- Status: FIXED; native retest passed.

### ISSUE-042 Partial quality save has no durable section-level outcome

- [x] First manual cycles —1687–1693 and1740–1745 dropped the validation response after acceptance; TDD/validation persisted and Retrieval/QA were never submitted. Transient toast wording was not captured; no assertion about that wording.
- [x] Source diagnosis — dialog closed before sequential saves; generic transient failure could not preserve acknowledged, uncertain, and unattempted section outcomes.
- [x] Fix — immutable-target coordinator tracks each PATCH acknowledgment, stops at first failure, reconciles via a read without replaying writes, and presents a persistent section-level result with read-only Reload saved settings. Cancellation remains cancellation.
- [x] Regression/build —9 focused regressions; full v91 shared suite705 tests across72 suites, zero failures/errors/skips. Source manifests and both ABI signatures verified.
- [x] Native retest —1772–1774 on isolated emulator5596/v91: one Save restoring Max fixes3/Feature limit2, exact validation response dropped after forwarding. Persistent result correctly shows TDD acknowledged, Validation matching requested values with acknowledgment unavailable, Retrieval/QA not attempted.
- [x] Recovery —1775 Reload saved settings retained the result; proxy evidence contains only the original two PATCH requests, no repeated writes.1779 reopened values3/2. Read-only baseline comparison differs only in logs/updatedAt; all task/configuration data restored.
- [x] Delivery —ARM64 v91 uploaded https://gofile.io/d/bGip9p4x ; API/landing HTTP200, size10238790 and MD5aaba17f70244f472e193d50ba889dd59 match. Receipt android-v91-quality-save/gofile-delivery.json.
- Status: FIXED AND MANUALLY RETESTED for the reproduced lost-validation-response branch. All four lost-response positions and failed verification/recovery branches passed natively through1805 (CONFIG-15).

### ISSUE-043 Stale quality dialog silently overwrites another client's changes

- [x] First native reproduction —1816 opens F quality dialog at Max fixes3/Feature limit2. Controlled second-client HTTP PATCH changes Max fixes4 (HTTP200 and stored-state verified).1817–1819 native edits only Feature limit2→3 and saves.1820 normal board returns with no conflict; saved Max fixes is silently reverted to3. Evidence config17-before-1816.json, config17-second-client-1816.json, config17-overwritten-1819.json.
- [x] Diagnosis —all four sections were submitted from stale opening values; existing endpoints have no expected-value precondition.
- [x] Fix —field-level changes plus dedicated atomic server endpoint with expected-value guards; older servers fail safely without legacy fallback.
- [x] Server regression/build/deploy —8 focused tests including handler-level planning-error rejection persistence and ownership; full425 pass/1 existing ignored native dependency test. Corrected candidate00933a0af9cd deployed and verified5October12:18:38WIB; allA–H board files unchanged across restart.
- [x] Mobile regression —709 shared tests/73suites pass.
- [x] Native retest —1854–1859 preserves remote Max fixes4 while native changes Feature limit2→3;1860–1867 native changes Max fixes4→3 and Feature3→2 while preserving remote timeout130 in same validation object.1872–1874 same-field conflict returns persistent409; entire board JSON unchanged from remote state, including untouched timeout130.
- [x] Lost-response retest —1883–1885 dropped atomic quality acknowledgment after acceptance; result correctly shows matching Validation values and unchanged other sections. Reload is read-only, no duplicate writes.
- [x] Reverted draft/no-op —1888–1894 Max fixes3→5→3 in draft, then remote changes to4. Save issues no PATCH; reopened value4 retained.1895–1897 native restore3; final state matches original configuration/tasks except logs/updatedAt.
- [x] Delivery —ARM64v92 https://gofile.io/d/iogtpsJD ; size10255178 and MD5ea932b1cf13a0204726243ef983be964 match upload service, API/landingHTTP200.
- Status: FIXED AND MANUALLY RETESTED for quality-settings concurrency.

### ISSUE-044 Unknown board status enables Pause and unknown hierarchy is presented as Story

- [x] First native reproduction —GET-only compatibility fixture supplies board status `future_board_state_v99` with auto-run enabled and task status/level `future_task_state_v99`/`future_task_kind_v99`.1912/1916 board renders the raw new board state but Pause remains enabled and clickable (1916 XML confirms both attributes); no unsupported-board warning is shown. No Pause or other mutating action was tapped.
- [x] Hierarchy reproduction —1916 cards say Story;1917 details badge says Story and Hierarchy says `story` despite the supplied unknown hierarchy. Unknown task status is explicitly warned and routes to Todo;1918 task Actions offers only Discuss, so execution actions for these unknown task statuses are correctly absent in this fixture.
- [x] Discussion-action bypass —1919 Discuss opens the composer;1920 Action picker exposes Edit scope, Replace scope, Reprioritize, Delete, Split, Merge, Regenerate children, Re-research, Create revision/fix/replacement despite the unknown task state/level.1921–1922 Cancel exits without Send. This exposes mutating choices; no claim is made that the server would accept them, since none was submitted.
- [x] Fixture cleanup —proxy rules cleared after1922; persisted F exactly matches the1897 baseline snapshot (`android20-persisted-board-1922.json`).
- [x] Initial fix/native v94 retest —2006–2023 older/missing-field rendering and unknown board/task compatibility controls retested. Known legacy records remain usable; unknown board/task mutation controls are guarded, unknown hierarchy is displayed truthfully, and detail views remain readable. This addresses the original controls/discussion paths, not every bulk path.
- [x] Additional defect during retest —2026–2027 known paused board/Backlog status with unknown task kind still exposes bulk `Todo (6)` and `Remove (6)` as enabled/clickable after Select all; XML confirms both.2029 individual details is correctly read-only. No bulk mutation tapped.
- [x] Follow-up bulk fix/native v95 retest —2051 all six unknown-kind cards expose no selection or bulk actions;2052 details remains read-only and2053 closes.2055 mixed fixture contains three supported development-alias tasks plus three unknown-kind tasks: Select all3 targets only compatible records,2056 shows Todo(3)/Remove(3).2057–2058 refresh back to all unknown kinds clears previous selection counts/actions. No bulk mutation was submitted.
- [x] Regression/build —full724 shared tests/76suites pass; verified v95 build source freeze. Unit guards cover additional combinations including stale-dialog protection; those are regression-test evidence, not substituted native coverage.
- [x] Cleanup/delivery —proxy cleared2059; `android20-restored-2058.json` exactly equals1984 persisted F snapshot. Verified ARM64v95 https://gofile.io/d/r3RRZLIp ; size10255174bytes and MD555a69d20f527be6a48f1b0a5929c0f9b. Receipt: `android-v95-compatibility-guards/gofile-delivery.json`.
- Evidence:1912/1916–1922,2006–2029 and2051–2059 PNG/XML and `TEST-ANDROID20-unknown-statuses-1005.json` in artifacts/kanban-manual-20261001. This is response-fixture evidence, not persisted board mutation or a real newer server deployment.
- Status: FIXED AND MANUALLY RETESTED for reproduced unknown-board/task controls, hierarchy, discussion, and bulk-selection paths. Broader compatibility remains tracked under ANDROID-20.

### ISSUE-045 Missing linked provider session silently opens a new-chat composer

- [x] First native reproduction —2107 opens the GET-fixture task with linked nonexistent session `TEST-nonexistent-kanban-session-20261005`;2108 Actions and2109 Open chat navigate to Chat.2110 shows No messages/New chat, enabled Send, no missing-session error and no Back to board control. No message was sent.
- [x] Request evidence —real session snapshot GET returns404 at2026-10-05T07:43:16UTC; messages GET returns200 at07:43:17UTC with an empty response. The task reference is synthetic, but the missing-session HTTP behavior is real, not a fabricated error response.
- [x] Fix —missing linked-session handling now preserves board context and reports unavailable chat with Retry/Back to board instead of silently presenting a new conversation.
- [x] Build/delivery —v96 verified with733 shared tests/77suites and zero failures; ARM64 https://gofile.io/d/wqhxZOXi . Build/upload artifacts: `android-v96-task-chat-recovery-retry1`.
- [x] Native v96 missing-session retest —2146 shows Task chat unavailable, explicit not-found/deleted/inaccessible explanation, Back to board and Retry.2147 Retry repeats the accurate error;2148 Back to board returns to F. Receipt `issue045-v96-missing-session-events.json` records404 snapshot reads at08:21:09UTC and08:21:48UTC with no messages fallback afterward; its earlier07:43 entries belong to original reproduction.
- [x] Valid actual-session regression —fixtures cleared before2149. Live G initially times out2151; read-only Retry2152 recovers2153.2154 opens the linked chat for Add Node built-in test runner suite for the index.html structural contract;2155 shows actual transcript with Kanban task chat/Back to board, and2156 returns to correct G. No message sent.
- Evidence:2107–2110,2146–2148 and2149–2156 PNG/XML; `TEST-DETAIL15-legacy-evidence.json`, `issue045-v96-missing-session-events.json`, and fault-events-5594.jsonl session requests.
- Status: FIXED AND MANUALLY RETESTED on v96 for missing-session error/retry/board return and existing-session navigation.

### ISSUE-046 Horizontal swipe on nested task opens details as if tapped

- [x] Native v96 reproduction —live G2158 vertical swipe from nested child body scrolls safely;2159 vertical swipe beginning on Chat (x583,y747→y320) likewise scrolls without opening chat.2160 horizontal swipe x365→86 at y1100 over500ms instead opens Manually verify checklist shell UI at360x800 and414x896 phone viewports details.
- [x] Repeat —2161 Close returns to board;2162 opposite horizontal swipe x180→470 at y1100 reproducibly opens the same details. No task mutation submitted.
- [x] Fix —nested touch handling now distinguishes horizontal/vertical drag gestures from ordinary taps; included in v97 and retained in v98.
- [x] Partial native v97 fixture retest —2195–2196 horizontal child swipes in both directions no longer open details;2197 horizontal checkbox swipe leaves selection unchanged.2198 small3px/2px jitter tap still opens correct child details and2199 closes.
- [x] Partial live G controls —2212 vertical child-body swipe and2213 vertical swipe beginning on Chat behave safely;2215 original leftward horizontal gesture no longer opens details.
- [x] Original live G v98 retest —fixtures cleared before2239;2243–2244 load actual server state successfully.2245 vertical swipe beginning on Chat scrolls without opening chat;2246 child-body vertical swipe scrolls safely.2247 repeats original leftward x365→86/y1100 gesture and2248 original rightward x180→470/y1100 gesture; neither opens details.2249 horizontal swipe beginning on Chat x550→615/y1100 likewise opens nothing.
- [x] Ordinary-tap regression —2250 directly taps the same Manually verify checklist shell UI child and opens its correct details;2251 Close returns to the same board and scroll location.
- Scope —original reproduced live-G swipe conflict and nearby Chat/ordinary-tap controls are fixed. The final live retest uses actual server responses, with no GET mock; broader ANDROID-14 combinations remain separately pending.
- Evidence:2158–2162 original v96 PNG/XML,2195–2199 v97 fixture controls, and2243–2251 live G v98 PNG/XML on emulator5596.
- Status: FIXED AND MANUALLY RETESTED on v98 for the original bidirectional horizontal-swipe defect, with live vertical/Chat swipe and ordinary-tap controls.

### ISSUE-047 Recurring board load timeout requires explicit retry

- [x] Native observation —after clearing compatibility fixtures, live G load2151 times out; read-only Retry2152 succeeds2153. Earlier recovery/timeouts remain documented under Android lifecycle/network cases. This observation alone does not identify a definitive root cause.
- [x] Candidate fix/build —targeted Rust backfill performance candidate built;426 Rust tests pass/1 existing ignored native dependency test.
- [x] Deployment verification —5October16:34:33WIB detached runner records `DEPLOYED_AND_VERIFIED`, live PID1770863/SHA256 `a4b02921ca66202909dae6cbda89b92024bcbfcd8246e454fe0304930947fd61`, and healthy expected configuration/database. Evidence: `server-backfill-deploy-1005/deployment-result-backfill.txt`, `deployment-backfill.log`, `health-backfill.json`. This is not a latency or native-success result; real-board retest remains below.
- [x] Continuing native limitation —v98 installed5596 at2218; proxy Failed connect2221, Retry2223 briefly loads, then2224 fails again. No task mutation submitted. These failures block stable native049/full046 retest until recovery.
- [x] Postdeployment live-read retest —`issue047-postdeploy-latency-1005.json` records five authenticated cycles of F list/detail and G list/detail:20/20 requests return200 with correct board IDs, all below one second. Median durations are F list0.297s, F detail0.320s, G list0.312s and G detail0.388s; maximum0.709s. Fixture controls were verified empty for every request. All A–H board files remain byte-identical and inactive.
- [x] Native v98 live retest —2243–2244 load real G without an error; read-only navigation2245–2254 remains stable. This observes successful native recovery after deployment without submitting work or replacing server responses.
- Scope —the reproduced recurring-load problem is fixed for the observed native navigation and five live-read cycles. This is a bounded retest, not a long soak or a controlled before/after speedup measurement; no causal speedup ratio is claimed.
- Evidence:2151–2153 original PNG/XML,2243–2254 native v98 PNG/XML, `issue047-postdeploy-latency-1005.json`, and `server-backfill-deploy-1005` deployment receipts.
- Status: FIXED AND MANUALLY RETESTED for the observed native load/navigation path and five successful live-read cycles; long-running stability remains ANDROID-19 coverage.

### ISSUE-048 Runtime-failure attention panel is unreachable

- [x] Native reproduction —2171/2174/2177 mixed runtime-failed leaf/dependency waiter renders only Blocked/Waiting on other tasks and no Needs attention panel.2172/2175 individual details correctly distinguish provider error from dependency wait;2173/2176 show different permitted actions.
- [x] Diagnosis —buildKanbanBoardFromGroups filtered attention groups by an attention column, but kanbanColumnId returns only the five workflow columns. The attention projection therefore could not include runtime failures.
- [x] Fix —preserve existing workflow columns and separately project qualifying runtime failures, reusing the existing action-eligibility predicate. Approval permissions are unchanged.
- [x] Shared regression —743 tests/78suites pass; combined046+048 v97 build underway.
- [x] Native v97 mixed-fixture retest —2200 shows Needs attention/View1 while preserving Blocked1 workflow grouping.2201 expansion retains both nested children as contextual group content, without an unauthorized Retry button.2202 opens actual runtime-failure details with correct error;2203 closes.
- [x] Dependency-only control —2204 changes failed leaf to Todo, leaving the ordinary dependency waiter blocked. Needs attention disappears entirely while the blocked group remains visible. Response-only test; no execution or task mutation submitted.
- Evidence:2171–2177 PNG/XML; mobile attention projection source/regression tests.
- Status: FIXED AND MANUALLY RETESTED on v97 for mixed runtime-failure inclusion and dependency-only exclusion, preserving workflow grouping and existing action permissions.

### ISSUE-049 Delete confirmation omits structurally cascaded descendants

- [x] Native reproduction —2190 selects a parent with a nested child;2191 confirmation reports only one selected Backlog task without disclosing affected descendants.2192 Cancel preserves parent/child and selection; no deletion submitted.
- [x] Diagnosis —server deletion structurally cascades to descendants, confirming the displayed selected-count-only prompt omits material consequences.
- [x] Fix —shared unique-impact calculation plus bulk/single confirmation details and stale-approval guard implemented.
- [x] Regression/build/delivery —748 shared tests/79suites pass. v98 ARM64 https://gofile.io/d/RozfIpE6 ; installed on5596 at2218.
- [x] Native v98 bulk confirmation retest —2230 selects one parent and opens Remove: confirmation explicitly reports total2 affected tasks, selected1 and additional descendant1, with cascade consequences present in full XML text.2231 Cancel preserves both tasks and the selected parent.
- [x] Native stale-approval retest —2233 reopens confirmation, then a GET-only fixture changes the child title; proxy confirms delivery at09:43:00UTC.2234 OK closes/rejects the obsolete confirmation without deletion; the selected parent and changed child remain. `issue049-stale-confirmation-write-events.json` records zero write requests from09:40UTC onward. No rejection toast was captured, so its wording or visible presentation is not claimed.
- [x] Native detail/subtree confirmation retest —2235 parent details →2236 Delete2→2237 confirmation reports total2, selected2, additional descendants0 without double-counting.2238 Cancel preserves the task subtree.
- Scope —controlled synthetic GET-response native confirmation, cancellation and stale-approval branches. No deletion was executed; real server cascade and duplicate-network-request behavior remain separate cases.
- Evidence:2230–2238 PNG/XML, corresponding GET-fixture delivery events, and `issue049-stale-confirmation-write-events.json`.
- Status: FIXED AND MANUALLY RETESTED on v98 for the reproduced consequence-display defect, unique counts, cancellation and stale-approval guard.

### OPEN-01

Project shortcut: Open Board from a project card; load that project’s board.

- [x] First manual test — tapped the Pocket Shopping project card's Kanban shortcut. Loading feedback appeared, followed by that project's completed board, correct Codex/MiniMax model, one Done feature and 14 nested records.
- [x] Fix if needed — no defect observed.
- [x] Retest — loaded state inspected after the request settled, not merely after tapping.
- Evidence: screenshots 006–008 in artifacts/kanban-manual-20261001.
- Result: PASS.

### OPEN-02

Board tab: Open through app navigation; retain the correct selected project.

- [x] First manual test — inspected Projects navigation and the selected project's Files workspace. Neither exposes a Board tab. The project-card shortcut works separately under OPEN-01.
- [ ] Fix if needed — not classified as a defect: current native navigation intentionally exposes only project shortcuts for Board. Do not add a new navigation design solely to force this case to pass.
- [ ] Retest — requires a build with the specified Board-tab entry point.
- Evidence: screenshots 011–014; native buildBottomNavigation exposes Terminal, Database, Projects, IO Gateway, Settings, and workspace Files/Chat/Git, but no Board entry.
- Result: UNAVAILABLE ON CURRENT UI, not PASS.

### OPEN-03

No existing board: Show the empty state and Create board action.

- [x] First manual test — newly registered kanban-manual-a-1001 opens as No Board Yet, zero work items, a clear explanation, and a Create board action. The action opens the creation form; Cancel returns to the unchanged empty state.
- [x] Fix if needed — empty-board behavior itself needs no fix. Project registration visibility failed during setup and is tracked as ISSUE-002 below.
- [x] Retest — cancelling the form still shows no board; no creation was submitted.
- Evidence: screenshots 035–037.
- Result: PASS for empty-board behavior; setup failure remains open separately.

### OPEN-04

Existing board: Restore its tasks, status, configuration, and available controls.

- [x] First manual test — reopened historical Pocket Shopping after visiting the empty test project. Correct completed board, one feature, 14 nested Done records, Codex/MiniMax-M3, TDD on, RAG off and validation history restored. Run is disabled in both pixels and accessibility state.
- [x] Fix if needed — no defect observed for this completed-board fixture.
- [x] Retest — repeated navigation restored the same board; read-only stored state confirms 15/15 Done, completed/inactive, same provider/model and read-only Git.
- Evidence: screenshots 008 and 038–040; no mutations to historical board.
- Result: PASS.

### OPEN-05

Missing project: Handle deleted, inaccessible, or invalid project directories with a useful error.

- [x] First manual test — after checking the disposable project directory was empty, temporarily renamed it to kanban-manual-a-1001-held, then opened its still-registered project card. Android incorrectly showed ordinary No Board Yet and an enabled Create board action, with no missing-directory explanation.
- [x] Fix — project-scoped list/detail reads now validate missing, unreadable, non-directory, allowed-root and ownership constraints without recreating paths or deleting board history. Initial regression had four failures; focused green followed by full Kanban suite 293 passed, one native-RAG test ignored.
- [x] Retest1 — deployed server + APK45: missing-path open returns to Projects; unreadable-path and regular-file refresh leave stale empty-board UI with enabled Create. User-facing feedback is still inadequate (ISSUE-003).
- [x] Fix2 and Retest2 — APK70 shows actionable404/403/400 states with disabled board actions. Retry while still missing retains the error; restoration recovers each variant without restart. Screens143–149.
- Evidence: screenshots041–045 and095–104. Original empty directory restored0755 after each variant. Temporary regular-file fixture preserved separately at /home/giofahreza/kanban-manual-a-1001-file-fixture. No real user project was moved or deleted.
- Result: PASS after second repair. Original directory restored0755; no board created during invalid-path testing.

### OPEN-06

Rapid project switching: Switch A → B while A is loading; A’s response must not replace B’s board.

- [x] First manual attempts — original v43, initial project shortcut A delayed, then opened B; B remained visible. Long and eight-second delay attempts were not conclusive because ordinary reads can time out before stale data reaches the controller. Screens 060–071. Historical boards were only read.
- [x] Fix — independently failing controller regressions establish the stale-response defect; request-generation guard implemented (ISSUE-001).
- [x] Retest — APK70, real historical Pocket Shopping response held using original-body trickling. Navigated back then opened kanban-manual-a-1001; it showed the correct empty board before release. Complete392749-byte response delivered after44.166seconds without error, and the selected empty project remained unchanged afterward (155–159; TEST-OPEN-06-v70-trickle-history telemetry).
- Result: PASS after fix, with confirmed late delivery. UI note: the global Loading board overlay stayed visible while the old request was pending even though the new project had loaded; no wrong-project board or actions appeared.

### OPEN-07

Project isolation: Selected cards, expanded groups, task actions, and dialogs must not affect another project.

- [x] First manual test — selected A, opened empty B (203–210); after B creation selected its task-1, returned to A with its own task-1, and saw no leaked selection or B content (225–231).
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: IN PROGRESS; nested expansion, task-action, and dialog isolation still need coverage.

### OPEN-08

Loading failure: Initial failure and retry must not leave a permanent spinner, blank screen, or misleading empty board.

- [x] First manual test — APK45, exact project-scoped HTTP503 injected through a loopback relay forwarding to8100. Proxy confirms full delivery of TEST injected failure; Android retains misleading No Board/Create state (132). Successful refresh returns the same-looking empty state (133).
- [x] Fix — shared with ISSUE-003 persistent board-load feedback repair.
- [x] Retest — APK70 refresh failure150 and initial failure153 both show persistent HTTP503 feedback and no misleading Create action; Retry151/154 restores the actual empty board. Proxy telemetry confirms the exact fault responses were delivered.
- Result: PASS after repair. This fixture changes only the read response, never server board data.

### CREATE-01

Valid prompt: Create with a normal prompt; show the resulting board and planning state.

- [x] First manual test — exact prompt in artifacts/kanban-manual-20261001/prompt-a.txt pasted into A (197), Codex/min:MiniMax-M3, Minimal profile, then one Create (198).
- [x] Fix if needed — no creation defect observed.
- [x] Retest — created paused/inactive board7b967b5b-cae0-470d-9b1a-7d093893f8b1 with one root Backlog Story; refresh and details retain full prompt (199–201). Read-only saved state matches provider, model, profile, gates and one201creation response.
- Result: PASS. Creation stores the submitted scope; it does not itself generate a task plan or start implementation.

### CREATE-02

Blank prompt: Empty or whitespace-only input is rejected without submitting.

- [x] First manual test — empty Create shows Board prompt is required and focuses the field (134–135).
- [x] Fix if needed — none observed.
- [x] Retest — three spaces are also rejected (136–137); Cancel returns to no board (138). Relay log contains no board-creation POST for either invalid submission.
- Result: PASS on APK45.

### CREATE-03

Complex prompt: Preserve multiline text, Unicode, emoji, quotes, and long descriptions.

- [x] First manual test — pasted1743 characters with multiple scripts, emoji, quotes, angle brackets, blank lines and an end marker (219); accessibility field matched the complete input exactly.
- [x] Fix if needed — no preservation defect observed. Incorrect inferred task kind is tracked separately as ISSUE-007.
- [x] Retest — B created paused with one Story; stored description equals the input after trailing-newline trimming. Details preserve readable Unicode and scroll to the final marker (221–223).
- Result: PASS for complex-prompt preservation. Board05576382-8fd8-4265-9cf1-6caf77474c9f, exact input in artifacts/kanban-manual-20261001/prompt-b-unicode.txt.

### CREATE-04

Provider selection: Test each exposed CLI agent: Claude, Codex, and Gemini.

- [x] First manual test — all three agents exposed and selected; Codex creation persisted correctly (173–199).
- [x] Fix if needed — no provider-selection persistence defect observed.
- [x] Retest — Gemini/gemini-2.5-flash created C correctly (315), with the same values after refresh and in stored board state.
- Result: PASS for creation with all three exposed providers (199,221,315). Model execution is not established by selection alone.

### CREATE-05

Model selection: Test default, selected, custom, unavailable, and incompatible models; errors must be actionable.

- [x] First manual test — default and selected models rendered; Codex/MiniMax-M3 persisted. Claude/Codex catalog mixes coding, image and video entries, lacks search/custom-ID entry; Gemini exposes five native options (170–186).
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: IN PROGRESS; unavailable/incompatible execution and custom-entry availability still need final disposition.

### CREATE-06

Provider switching: Change provider repeatedly; each provider retains its own model choices.

- [x] First manual test — selected Claude/agw:claude-sonnet-4-6, Codex/min:MiniMax-M3, Gemini/gemini-2.5-flash in the same creation draft (169–186).
- [x] Fix if needed — none observed for provider-specific board-model choices.
- [x] Retest — switching back to each provider restores its own selection without leaking another provider’s model (187–192). Codex implementation route remains MiniMax-M3 (193).
- Result: PASS for creation-dialog provider/model switching. Cross-restart and all-stage preference coverage belongs to CONFIG-05.

### CREATE-07

Profiles: Create with Minimal, Complete App, and Product-Ready; saved settings match the selection.

- [x] First manual test — Minimal selected and explicitly saved (193–196); persisted boardProfile=minimal with unchanged chosen gates (199–201). Profiles guide planning scope, not TDD/RAG toggle presets.
- [x] Fix if needed — no profile persistence defect observed.
- [x] Retest — Product-Ready persisted in C and is restored in the existing-board strategy dialog (319); stored boardProfile=product_ready.
- Result: PASS for Minimal, Complete App and Product-Ready (199,221,319). These are selection/persistence checks, not generated-scope-quality claims.

### CREATE-08

Cancel: Dismiss creation or nested configuration dialogs; verify which draft values are retained and that no board is created.

- [x] First manual test — outer Cancel creates no board (138). Nested configuration Cancel retains the changed Thinking toggle, while the parent summary remains stale (161–164).
- [x] Fix — ISSUE-004 isolates configuration drafts, including the mutable model map; commit only on Save.
- [x] Retest — APK72 Cancel and Back discard toggle/profile/model-map edits; positive Save retains them (270–293); outer Cancel creates nothing (302).
- Result: PASS after repair. Outside tap is ignored by this modal; no dismissal behavior is invented.

### CREATE-09

Duplicate submission: Double-tap Create or retry after a slow response; no unintended duplicate work.

- [x] First manual test — double-tapped Create on C's populated draft as one logical gesture (313–314).
- [x] Fix if needed — no duplicate observed for this gesture.
- [x] Retest — settled board and details contain exactly one root task with the prompt (315–319); read-only persisted data has only task-1, and relay metadata contains one creation POST at06:16:11UTC.
- Result: PASS for double-tap submission. This does not establish idempotency after an ambiguous network response; CREATE-11/ANDROID-13 cover that separately.

### CREATE-10

Existing board reuse: Creating again for the same project can reuse the latest board; verify appended work and retained running/paused/scheduled state are clear.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### CREATE-11

Interrupted creation: Disconnect or background the app after submission; reconnect and discover the actual server outcome.

- [x] First manual test — exact one-shot lost response after server accepted D's creation (347–349); Android stays on misleading No board/Create state.
- [x] Fix — ISSUE-009 authoritative reconciliation and persistent recovery guidance, APK74.
- [x] Recovery control — manual Refresh reveals the saved paused board with one task and correct provider/model (350–351); this is not a fix retest.
- [x] Retest — E516–519 auto-recovers after lost POST; F534–539 retains unverified-outcome guidance across two failed reads and then recovers through Retry. Each project has one board and one original task, with one POST only.
- Result: PASS for the tested interrupted-response path and read-retry recovery. No execution or generated-plan claim.

### VIEW-01

Column placement: Backlog, Todo, active work, Blocked, and Done cards appear in the correct displayed columns.

- [x] First manual test — explicitly marked read-only mixed-state GET fixture, all five columns inspected (354–359).
- [x] Fix if needed — no placement defect observed.
- [x] Retest — Backlog1, Todo1, Active2, Blocked1 and Done1 match all six supplied groups; active mixed and eligible-sibling groups both visible (356–357).
- Result: PASS for client rendering of supplied states. No live scheduling or model-generation claim.

### VIEW-02

Level versus kind: Initiative/Epic/Story/Task/Subtask hierarchy remains distinct from implementation, QA, review, and other work kinds.

- [x] First manual test — B correctly renders Story hierarchy separately from kind, but incorrectly calls the app-building scope Manual Test (221). Stored taskType corroborates the incorrect inference.
- [x] Fix — ISSUE-007 preserves primary implementation intent despite later manual-verification acceptance wording.
- [x] Retest — fresh identical-prompt creation after deployment correctly renders Story plus Implementation (315), confirmed in persisted data.
- [x] Extended manual coverage — five levels render independently from kind; Epic/Story/Task and six typed leaf details inspected (374–392), including complete deep paths. Implementation, QA, Review, Research and Manual Test labels are correct.
- [x] Fix and independent retest — APK74 original Fix row/detail now correct497–498, with legacy QA Fix controls482/486 unchanged.
- Result: PASS for level/kind presentation. Real primary-intent classification was verified separately under ISSUE-007; supplied hierarchy fixtures do not prove model decomposition.

### VIEW-03

Grouped work: Related implementation and validation subtasks appear under the correct feature/group.

- [x] First manual test — mixed implementation/QA/review leaves stay beneath their feature with four nested records (368–370).
- [x] Fix if needed — no grouping defect observed; Fix kind-label problem is separately tracked as ISSUE-010.
- [x] Retest — five-level hierarchy with ten records renders one Backlog feature and exactly nine nested records, including six typed leaves (374,381–392).
- Result: PASS for grouping supplied read-only records, not proof of real provider decomposition.

### VIEW-04

Explicit group IDs: Explicit grouping wins over legacy task-number naming; unrelated similarly named tasks remain separate.

- [x] First manual test — six read-only fixture records render five groups: unrelated similar names remain separate, while the genuine legacy pair groups together (396–398).
- [x] Fix if needed — no grouping defect observed. Incorrect Fix labels here are the separately tracked ISSUE-010.
- [x] Retest — legacy details contain only the intended child; explicit groups remain separate despite shared legacy source IDs, with no invented nested work (399–403).
- Result: PASS for client rendering of supplied fixture records; no provider or backend grouping mutation was tested.

### VIEW-05

Mixed child states: A group with Todo, executing, blocked, and completed children displays an accurate overall state.

- [x] First manual test — mixed group has Todo, In Progress, Blocked and Done children (368).
- [x] Fix if needed — no rollup-rendering defect observed.
- [x] Retest — group and details remain In Progress, P1, 1/4 nested done with four distinct children; Current activity shows only the one executing leaf (354,369–370).
- Result: PASS for read-only fixture rendering; backend state transitions are tested separately.

### VIEW-06

Eligible sibling: A blocked child must not hide another child that is ready to run.

- [x] First manual test — blocked parent/sibling with an independent Todo sibling remains visible in Active (356,361).
- [x] Fix if needed — no hidden-ready-work defect observed.
- [x] Retest — individually opened both children; their Blocked/Todo statuses and correct hierarchy paths remain distinct. Group details select the Todo sibling as current step (362–366).
- Result: PASS for visibility/rollup of supplied fixture data. This does not prove the real scheduler executes that sibling.

### VIEW-07

Card details: Title, priority, status, kind, origin, progress, and nested-work counts agree with the task data.

- [x] First manual test — mixed/eligible groups and leaf details compared against fixture titles, priorities, statuses, kinds, origins, hierarchy and counts (361–370).
- [x] Fix if needed — prior hierarchy-label repair now verified for nested details (ISSUE-005).
- [x] Retest — mixed P1/User prompt-generated/1-of-4 done and eligible P0/Planner/0-of-2 done match data; four mixed children retain distinct states/kinds, no inflated feature counts.
- Result: PASS for supplied card/detail metadata; fixture origins are labels, not proof of real model generation.

### VIEW-08

Long content: Long titles, unbroken text, Unicode, and multiline content remain readable without covering actions.

- [x] First manual test — read-only long-content card wraps its full selection label and uses ellipsis for the display title; Open details remains reachable (407).
- [x] Fix if needed — no clipping/overlap defect observed. Repeated full title in the selection label makes the card tall, a usability observation rather than lost content.
- [x] Retest — details preserve the complete hierarchy title, Unicode, line breaks, literal HTML-looking text, description end marker, criteria, reference, and full file path while scrolling. Fixed Actions/Close remain unobscured; Close returns to the card (408–411).
- Result: PASS for Android rendering of the supplied fixture, not generated-app behavior.

### VIEW-09

Missing metadata: Missing title, unknown status/kind, or older field names render safely with meaningful fallbacks.

- [x] First manual test — missing title uses details, absent display text uses Work item, malformed values render safely, and legacy Review/User manual fields work (414–423). Legacy acceptance criteria are missing (ISSUE-011).
- [x] Fix and retest — supported legacy acceptance criteria restored in both details renderers; original group492 and nested alias484 pass, with canonical controls483/486.
- [x] Unknown-state follow-up — ISSUE012 repaired on APK75 and manually verified545–555: Unknown badge/raw detail/qualified readiness, legacy controls preserved, mixed known/unknown states remain distinct. Unknown-only work stays excluded from selection.
- Result: PASS for safe and meaningful rendering of supplied missing/malformed/legacy/unknown metadata. No scheduler mutation was performed with these fixtures.

### VIEW-10

Pagination: Test 0, 1, 9, 10, 11, 20, and 21 groups; Show more reveals the correct next items without duplication.

- [x] Zero-group subcase — existing empty read-only board shows zero features, No features, and no Load more in Todo and Backlog (431–432); it is not confused with an absent board.
- [x] One-group subcase — exactly Row01, one feature, and no Load more (435).
- [x] Nine-group subcase — all nine distinct rows appear in descending order09→01 with no Load more at the end (440–445). Intermediate filename441 overstates its contents; actual pixels show09/08 only, with07/06 first shown442.
- [x] Ten-group subcase — all ten distinct rows10→01 appear without Load more (448–453).
- [x] Eleven-group subcase — initial ten rows11→02, Load more(1); one tap appends only Row01 without resetting scroll. End contains no remaining Load more (456–462).
- [x] Twenty-group subcase — first ten rows20→11, Load more(10); one tap preserves position and appends the remaining ten distinct rows10→01, with no remaining control at the end (465–476), APK73.
- [x] Twenty-one-group subcase on APK75 —558–562 show distinct21→12, then Load more(11). First tap563 appends11→02 without resetting position; all ten inspected564–568, ending with Load more(1). Second tap569 appends only01;570 shows complete final card with no remaining control.
- [x] Fix if needed — no pagination defect observed.
- [x] Retest — both21-card boundaries preserve position and append only the expected next items; all seven requested cardinalities now inspected individually.
- Result: PASS for client pagination of supplied records; no live provider-generation claim.

### VIEW-11

Expand/collapse: Repeatedly expand nested work; no duplicated children, incorrect counts, or accidental actions.

- [x] First manual inspection — nested work is rendered eagerly with no nested Expand/Collapse control (361,368). Group/child details open normally; only Current activity has that toggle (360).
- [ ] Fix if needed — no new interaction added solely to force a pass.
- [ ] Retest — requires a UI exposing the specified nested-work toggle.
- Result: UNAVAILABLE ON CURRENT UI, not PASS. Current-activity expansion is not substituted for the requested nested-work operation.

### VIEW-12

Simplified board: Internal validation records do not inflate visible feature counts or appear as unrelated user features.

- [x] First manual test — five supplied records, including canonical/legacy internal-validation flags, render exactly two features and1/2completed573.
- [x] Fix if needed — no inflation or extra-card defect observed.
- [x] Retest — Done contains only the visible Done feature574; Blocked remains empty despite the hidden failed internal validation record575. Backlog contains only its visible feature573.
- Result: PASS for synthetic client filtering/counts, not proof of actual validation execution or failure handling.

### MANUAL-01

One task: Submit a valid manual task; it enters Todo.

- [x] First manual test — real A, no fixtures: one UI submission584 creates task-2 with unique title MANUAL01-A-1002 Create a local checklist page shell. No dependencies; default P2.
- [x] Fix if needed — no task-entry defect observed.
- [x] Retest — Todo card586 and reopened details587 retain the full title/description, Story/Implementation/User manual/P2. Read-only persisted data agrees; original unapproved task-1 remains Backlog.
- Result: PASS for manual entry into Todo. Subsequent model planning/execution is recorded separately, not assumed completed.

### MANUAL-02

Execution consequence: If adding Todo work starts execution, the board controls and activity immediately reflect that.

- [x] First manual test — after584 submission, board becomes Running and bottom control becomes Pause585. Activity correctly says Reviewing project context while no implementation subtask is executing.
- [x] Fix if needed — no stale-start or false-no-activity defect observed in this run.
- [x] Retest — changing column586 and reopening task details587 preserves correct Running/activity feedback; persisted active=true/status=running agrees.
- Result: PASS for immediate manual-submission execution feedback. Provider success and completed implementation are not implied.

### MANUAL-03

Multiple tasks: Add several cards in one submission; preserve each card’s fields and ordering where applicable.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MANUAL-04

Add/remove editor: Add another editor, remove one, and try removing the last remaining editor.

- [x] First manual test — add second editor609, remove it613; first editor's unique title is retained. Remove the final editor614 leaves one editor and its contents intact, matching the earlier blank-editor check591.
- [x] Fix if needed — no functional defect observed.
- [x] Retest — reopening resets to one empty editor617; repeated add618 and footer Add another621 append further draft content622 without submitting. Footer actions remain reachable by a separate vertical scroll620; this is awkward but not an unreachable-action failure.
- Result: PASS for editor add/remove behavior; no persisted multi-card submission claimed.

### MANUAL-05

Required title: One blank title among valid cards prevents an invalid submission and focuses the correct field.

- [x] First manual test — one valid title plus a second empty title: Add tasks610 shows Task title is required and focuses the second title; the first draft remains intact.
- [x] Fix if needed — no defect observed.
- [x] Retest — second title containing three spaces611 is rejected identically612. Read-only board remains cancelled/inactive with14 tasks and no FORMONLY draft marker610/615/623.
- Result: PASS.

### MANUAL-06

Optional details: Blank details use the intended title fallback without losing the title.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MANUAL-07

Rich fields: Acceptance criteria, references, and file paths preserve meaningful lines and handle blank lines.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MANUAL-08

Priority: Test P0–P3 and the default P2; verify persistence after refresh.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MANUAL-09

Dependencies: Test one/multiple IDs, supported separators, whitespace, and duplicate IDs.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MANUAL-10

Invalid dependencies: Unknown, self-referencing, cyclic, ancestor, and cross-board references produce understandable errors/blockers.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MANUAL-11

Partial/error outcome: A failed submission must refresh actual state rather than assuming no tasks were created.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MANUAL-12

Cancel/repeat: Cancelling creates nothing; rapid submission or retry does not silently duplicate tasks.

- [x] First manual test — Cancel populated draft615; no task created. Reopen617 shows a fresh empty editor, not a silently submitted or retained draft.
- [ ] Fix if needed — none needed for cancellation; rapid valid submission not yet assessed.
- [x] Retest — repeat add-editor sequence618–622 and Cancel623; board remains14 tasks, cancelled/inactive.
- Result: IN PROGRESS; cancellation passes, rapid valid submission/retry still pending.

### PLAN-01

Generate: Valid prompt creates planning work in Backlog.

- [x] First manual test — F797–806: submitted once “PLAN01-F-1003: Add keyboard controls to the local counter web app. ArrowUp increments, ArrowDown decrements, and R resets. Keep the counter accessible. Use no packages or external services.” Codex/min:MiniMax-M3/Minimal; POST20217:03:52UTC.
- [x] Fix if needed — no generation/persistence defect observed.
- [x] Retest — after leaving and returning,811–814 show exactly one new Backlog Story with requested mappings, focusable existing controls, no dependency installation/external services, and Blocked by task-1. Original counter story retained; paused/inactive, zero attempts. Unnecessary package.json test-script scaffolding is a plan-quality caveat, not an observed package installation.
- Result: PASS for generation into Backlog, not implementation or end-to-end app correctness.

### PLAN-02

Prompt validation: Empty/whitespace prompt is rejected; long and multiline prompts remain intact.

- [x] First manual test — blank Preview709 focuses Prompt and shows “Prompt is required.” Three spaces verified710; Add to Backlog711 and Preview712 reject them with the same field error. Clear713; empty Add to Backlog714 also rejected. Cancel715, no generation submitted.
- [x] Fix if needed — no defect observed in these client-local validation branches; no live mutation was attempted while ISSUE-014 held the server lock.
- [ ] Retest — long/multiline generation and persisted text still need coverage after server repair.
- Evidence: screenshots707–715. Per-request form default gpt-5.5 was merely displayed, not used for a provider request; G board remains MiniMax-M3.
- Result: IN PROGRESS.

### PLAN-03

Per-request settings: Breakdown provider/model/profile apply to that generation request without unexpectedly changing board configuration.

- [x] First manual test — F799–805 explicitly selected min:MiniMax-M3 and per-request Minimal instead of the displayed defaults; submitted once806.
- [x] Fix if needed — no unexpected board-configuration mutation observed.
- [x] Retest — saved session3d0073f262dd411e8b64e9ac39c88647 uses Codex/min:MiniMax-M3/io_gateway; actual request prompt says Minimal while persisted board stays Product-Ready and retains its implementation override. Native811/816 retains Codex/MiniMax, TDD on, no execution.
- Result: PASS for this explicit per-request model/profile override; not a claim of running every available provider/model.

### PLAN-04

Preview: Preview displays drafts without adding executable work.

- [x] First manual test — F Generate from prompt1035–1039, Codex/min:MiniMax-M3/Minimal; exact prompt: “PLAN04-F-1003: Add a short keyboard shortcut hint below the local counter: ArrowUp increases, ArrowDown decreases, and R resets. Do not change existing behavior, add packages, or use external services. Create one planning Story depending on board:task-2.”
- [x] Result inspection — one Preview1040 produces a real draft1042, titled Display keyboard shortcut hint below the local counter, Backlog/P1. It correctly keeps display-only scope, existing behavior and no-package restrictions; raw provider plan has the intended task-2 dependency and eight meaningful acceptance checks. Preview UI exposes title/status/priority/details, not those structured criteria/dependency fields.
- [x] Fix/retest — no preview mutation defect observed. Close1043 retains the prompt; read-only F still has exactly its two original Backlog records, both attempt0 and board paused/inactive. No app files or executable task were added by Preview.
- Result: PASS.

### PLAN-05

Accept preview: Add drafts once; closing preview alone must not add them.

- [x] Close-only control — closing actual draft1043 creates nothing; two original tasks remain paused/unexecuted. Original preview session session_d21133debe9a48a6ac3b52d66ffb55dd completed normally.
- [x] Accept control — second actual draft1046 inspected before rapid Add drafts taps1047. Exactly one POST/tasks201 and one new Backlog Story1048; no execution. However saved details1049–1050 lost the required dependency and incorrectly label its origin User manual.
- [x] Fix/retest — ISSUE027 canonical acceptance repaired. Fresh preview1117 accepted once1118; saved task1119–1121 has correct generated origin, seven criteria and canonical task-2 dependency. Refresh/reopen1123–1126 retains all fields; one POST201 and exactly one new task, no execution. Earlier close-only1043 remains valid.
- Result: PASS.

### PLAN-06

Empty/invalid generation: Empty output, malformed plans, or provider failures produce a useful failure state.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### PLAN-07

Hierarchy levels: Breakdown Initiative, Epic, Story, and Task into valid next-level children.

- [x] First manual test — G Story breakdown starts691; provider returns a plan but no children persist, confirmed in native details703 and refresh706. ISSUE-014 explains the stalled transcript/persistence path.
- [x] Fix — ISSUE-014 repair deployed and actual transcript/save path no longer hangs.
- [x] Partial retest — saved Story→Task children visible754–755 and actual Task→Subtask children corroborated after recovery. Scope/parentage false wrappers are a separate015/PLAN09failure.
- [ ] Remaining manual coverage — Initiative/Epic branches and native inspection of all generated Task children.
- Result: IN PROGRESS; original Story-persistence failure repaired, not an all-level pass.

### PLAN-08

Executable leaf: Subtasks cannot be broken down or executed as planning parents incorrectly.

- [x] First manual test — G running implementation leaf931/task-4-subtask, Actions932 has no Breakdown, Move, Delete or Retry; only parent/chat/transcript/discussion/attempt. Actual execution is leaf-only930, parents have0attempts.
- [x] Fix if needed — none observed.
- [x] Retest — after real implementation+QA completion, same leaf947Done and Actions948 still has no Breakdown or invalid execution actions. Attempt1 updates to Done on reopening; no children generated beneath the executable implementation.
- Result: PASS.

### PLAN-09

Inheritance: Generated children retain applicable scope, prerequisites, and parent relationships.

- [x] First manual test — real task-2 original scope583/587 versus generated test child598: prohibited packages are explicitly required by the child. See ISSUE-013. Related dependencies and parent IDs were preserved, but scope inheritance failed.
- [x] Fix if needed — ISSUE-013 source repair and independent regression review complete; manual deployment retest still required.
- [ ] Retest — fresh G saved plan uses no-install built-in Node tests and preserves baseline→implementation→QA/manual dependencies. Native757–758 confirms original exclusions. Actual completion remains pending, and015incorrectly moves requested work into new approval parentage. No full inheritance pass yet.
- Result: FAIL.

### PLAN-10

Generated dependencies: Temporary/generated references resolve to the correct persisted task IDs.

- [x] First manual test — accepted generated F preview1046–1050 explicitly names existing board:task-2 in provider dependsOn, but stored task-3 has no dependency. References retain board:task-2 but cannot substitute for its execution edge.
- [x] Fix — ISSUE027 canonical draft acceptance deployed.
- [x] Existing-ID retest —1117–1126 saves board:task-2 as task-2, preserves generated origin and survives refresh.
- [x] Retest —1138–1149: two actual drafts accepted once, first forward reference resolves to second saved task6 plus existing task2; second resolves to task1. Both generated origins/criteria preserved, six total Backlog tasks, no execution. Refresh/reopen retains mapping. Scoped provider/persistence/request evidence: resume1004-f-plan010-accept-readonly.json.
- Result: PASS.

### PLAN-11

Duplicate breakdown: Double-tapping Breakdown creates only one operation/plan.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### PLAN-12

Surviving navigation: Leave the screen or lose the HTTP connection during breakdown; later refresh reveals its actual result.

- [x] First manual test — left F808 at17:04:54UTC while the single prompt generation was still pending; did not resubmit.
- [x] Fix if needed — no lost-operation/duplicate-work defect observed.
- [x] Retest — returned810; settled811 and details812–814 show the actual task2 saved17:05:39UTC, no lingering provider-call state/error, original task1 retained and board paused. One submitted operation produced one Story.
- Result: PASS for the leave-screen branch. HTTP-disconnection fault injection is not claimed by this run.

### PLAN-13

Failed breakdown retry: Retry releases stale operation state and does not duplicate previously created children.

- [x] First manual test — H890 fails package guard after provider output, retains one Story and retryable error893. Post-fix unchanged native retry911 starts a new Codex/MiniMax-M3 session and saves exactly five distinct model Tasks916; prior error and provider-call flag clear, no duplicate generation submission. Parent wrappers are separately incorrect under015.
- [x] Fix for observed blocker —018 deployed; failed-operation state did not prevent retry.
- [ ] Remaining coverage — failed refinement with previously persisted children, to directly test their preservation/nonduplication.
- Result: IN PROGRESS.

### PLAN-14

Invalid board/task state: Breakdown on cancelled or otherwise ineligible work is unavailable or rejected clearly.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MOVE-01

Backlog → Todo: Approve eligible scope; card and execution eligibility update correctly.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MOVE-02

Todo → Backlog: Withdraw approval as required; task does not continue as newly approved work.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MOVE-03

Parent approval: Approving a planning parent affects descendants according to hierarchy rules; the parent itself is not executed.

- [x] First manual test — G reviewed wrapperparent856→Actions857“Move3toTodo”→one tap858. Parent promotion200, childTask promotion200, executable-leaf promotion409; saved childTask is unexpectedly Backlog again. Only the parent approval persists; no implementation attempt starts. Screens859–860.
- [x] Fix — ISSUE017 scheduler preparation/promotion transaction deployed with genuine RED/GREEN and independent review.
- [x] Retest — G927–930: original approved parent retains state; Move2toTodo once929 approves both remaining descendants; all intended approvals persist, one executable leaf starts and parents remain attempt0. Original unrelated Story stays Backlog/unapproved. Screens931–932 identify the actual leaf and disallow planning/invalid moves while it runs.
- Result: PASS.

### MOVE-04

Dependency wait: Approved work with unfinished prerequisites remains blocked.

- [x] First manual test — approved G QA/manual leaves955/958 remain Blocked with attempt0 while their actual source prerequisite task-3-task is unfinished; no execution consumed while waiting.
- [x] Fix if needed — dependency gating itself is correct; separate019source completion and020stale wording fixes are tracked above.
- [x] Retest — after deployment/reboot1001–1010 the same waiters remain blocked until explicit Start1013 reconciles real completed prerequisite evidence; only then they release1014.
- Result: PASS.

### MOVE-05

Dependency completion: Eligible work becomes runnable when its actual prerequisites complete.

- [x] First manual test — after source completion1014, both approved QA and manual waiters change from Blocked to eligible; QA runs first while manual waits Todo/attempt0.
- [x] Fix — ISSUE019 restores the real missing source rollup, without bypassing its evidence or approvals.
- [x] Retest — QA completes and manual verification genuinely starts1017 with attempt1; completed implementation is not rerun and unrelated Backlog work stays unapproved.
- Result: PASS.

### MOVE-06

Missing/superseded dependency: Remains a visible blocker rather than being treated as completed.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MOVE-07

Parent rollup: Parent state accurately follows required child completion/failure.

- [x] First manual test — G's normal test subtree has Done parent and four Done executable records, while the original Story remains Blocked5/9, as expected. However982–983 still show original required HTML Task task-3-task as Todo and its parent active0/1 after its generated wrapped HTML and required QA both completed (946–948). That orphan source prevents downstream work forever.
- [x] Fix — ISSUE-019 narrow current-scope wrapped-work rollup repair has genuine failing regression and peer-reviewed GREEN, including current evidence, required QA, approval and provenance guards. It does not execute a parent or rewrite leaf history.
- [x] Deploy/retest — source group1016 now correctly Done1/1 after normal native resume1011–1014; dependent execution follows1017. Full serialized completed HTML/QA records and unrelated Backlog Story compare identical with the predeployment snapshot, parent attemptCount remains0. Ordinary pending required work still prevents original Story completion.
- Result: PASS for the tested canonical completion/failure rollups, including the repaired wrapped-orphan branch.

### MOVE-08

Optional work: Optional children under a completed parent do not unnecessarily reopen the parent.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MOVE-09

Required follow-up: Required QA/follow-up work prevents a premature Done state.

- [x] First manual test — after green-baseline task completion, required automaticQA4 remained Todo and its parent stayed In Progress1017. After QA4 genuinely finished, that parent became Done1020. Failed manual verification and its required Backlog follow-up keep the original Story unfinished8/11; it is not prematurely Done1018–1020.
- [ ] Fix if needed — no premature Done observed; confusing parent Todo/Active-versus-Blocked presentation is being checked separately.
- [ ] Retest — inspect the generated required follow-up and actual final rollup once its valid work completes.
- Result: IN PROGRESS.

### MOVE-10

Stale action: Task status changes while the action dialog is open; revalidate before applying.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### MOVE-11

Invalid transitions: Running, completed, superseded, or protected tasks cannot bypass state restrictions.

- [x] Running-subtask branch — genuine baseline attempt2 shows In Progress843; Actions844 offers history/chat/discussion only, with no Move/Delete/Breakdown/Retry controls.
- [x] Completed-subtask branch — actual HTML implementation Done1070 retains accepted completion evidence and automated pass; Actions1071 likewise offers parent/chat/transcript/discussion/Attempt1Done only, no Move/Delete/Breakdown/Retry. Closing1072 changes nothing.
- [x] Fix if needed — no invalid transition exposed in this branch.
- [ ] Retest — completed, superseded and other protected-state branches remain untested.
- Result: IN PROGRESS.

### MOVE-12

Scope revision: Moving an eligible failed/waiting subtask to Backlog preserves files, history, and prerequisites while withdrawing scope approval.

- [x] First manual test — real failed baseline leaf830–833, confirmation explicitly states approval withdrawal and preservation of files/history/prerequisites. One confirmation moves it to Backlog without executing.
- [x] Fix if needed — no data-preservation defect observed. UX caveat: revised leaf stays nested in its parent feature under Active837–838, not the Backlog column; it is still accessible with a Backlog badge.
- [x] Retest — reopened839 retains exact scope,840 retains failed attempt1 and allows Move to Todo. Scoped saved comparison confirms same scopeVersion3, original attempt digest, dependency and only test-file hash; board remained inactive until explicit reapproval841. Attempt2 then runs without editing scope.
- Result: PASS for failed-subtask scope withdrawal/preservation and same-scope reapproval.

### BULK-01

Individual selection: Select/deselect cards; counts and action availability update immediately.

- [x] First manual test — selected B's only Backlog card; count immediately became1 and Todo/Remove actions appeared (225).
- [x] Fix if needed — no selection defect observed.
- [x] Retest — deselection removed the count/actions (227), then reselection restored them (228). Screenshot226 was interrupted by emulator sleep_button, not an app crash; awake state was checked independently.
- Additional build76 regression788–790: F singleton still updates0→1→0 and shows/hides corresponding Todo/Remove actions. This repeats an existing pass and does not add a new passed case.
- Result: PASS for individual Backlog selection/deselection; bulk eligibility and multi-page behavior are separate cases.

### BULK-02

Select all/clear all: Operate on the intended eligible set, including checking behavior beyond the first displayed page.

- [x] Singleton test — F Select all791 checks the one eligible Backlog card and shows1selected; Clear all796 clears it and removes bulk actions.
- [x] Fix for observed branch — no defect observed.
- [x] Multi-card branch —1335 Select all selects6 of6 and both bulk actions show6;1340 Clear all removes selection/actions.
- [x] Beyond-first-page native selection —GET-only1000-task fixture:2062 Select all shows1000selected;2066 after Load more confirms previously unrendered0990/0989/0988 checkboxes checked.2071 Clear all removes selected count and bulk actions.
- [x] Eligibility filtering —2055–2056 mixed six-task fixture selects only three compatible task kinds (Select all3 and Todo/Remove3); unsupported kinds remain outside the eligible set.
- Evidence:2055–2056,2062–2071 PNG/XML. These native selection checks use controlled GET responses; no bulk execution or persisted task mutation is claimed.
- Result: PASS for select-all/clear-all eligibility and beyond-first-page selection.

### BULK-03

Stale selection: Refresh moves/deletes selected tasks; invalid IDs no longer remain selected.

- [x] Removed-ID refresh branch —2074 native Select all marks1000 tasks. Controlled GET response shrinks that same board to three retained IDs;2076 selection becomes exactly3, with Todo(3)/Remove(3), discarding997 absent IDs.
- [x] Changed-eligibility refresh branch —change those same three returned tasks to Done;2078 removes selected count and bulk actions, clearing now-ineligible IDs.
- [x] Fix if needed —no defect observed; selection reconciliation tracks refreshed response content.
- Evidence:2074–2078 PNG/XML and controlled GET fixtures. No real deletion/move or bulk execution was submitted: this verifies the native response-driven stale-selection requirement, while mutation execution remains covered separately by BULK-04 and deletion cases.
- Result: PASS for refreshed response-driven stale-selection UI.

### BULK-04

Bulk move: Move eligible Backlog tasks to Todo or Todo tasks to Backlog without affecting unselected tasks.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### BULK-05

Delete confirmation: Correct task count and cascade consequences are understandable; Cancel deletes nothing.

- [x] First manual test — F Remove1 opens “Delete1selected backlog task(s)?”792. Cancel793 returns with the card selected and present.
- [x] Retest — manual Refresh794, settled795 preserves original task-1/Backlog/attempt0; isolated relay has no mutation request during selection/Cancel/refresh.
- [x] Nontrivial count —1336 removal confirmation correctly shows6;1337 landscape and1338 portrait retain6 and reachable Cancel/OK.1339 Cancel preserves all six;1340 clear selection,1341 refresh.
- [x] Nested confirmation observation —2190 selects a parent in a parent/child GET fixture;2191 confirmation says only “Delete1selected backlog task(s)?” without child/cascade consequences.2192 Cancel preserves both cards and parent selection; no deletion submitted.
- [x] Diagnosis/fix —ISSUE-049 confirmed structural server cascade; unique-impact display, bulk/single confirmations and stale-approval guard implemented.748 shared tests/79suites pass, v98 installed2218.
- [x] Native v98 bulk retest —2230 confirmation shows total2 affected tasks: selected parent1 plus additional descendant1, with cascade consequences visible in full XML.2231 Cancel retains both tasks and parent selection.
- [x] Stale-confirmation guard —2233 opens confirmation; a GET-only child-title update arrives at09:43:00UTC.2234 OK does not delete anything, retains the changed child and selected parent, and generates no write request (`issue049-stale-confirmation-write-events.json`, zero writes from09:40UTC onward). No toast text was captured or asserted.
- [x] Detail/subtree unique-count control —2235–2237 parent detail Delete2 confirmation shows total2/selected2/additional0;2238 Cancel preserves both tasks.
- Scope —PASS covers understandable unique counts/cascade consequences and cancellation for observed native confirmations, plus the response-driven stale-approval guard. Synthetic GET fixtures were used; no actual deletion executed. BULK-06 remains NOT RUN for redundant-request behavior during real parent-plus-child deletion.
- Result: PASS for observed native confirmation counts, cascade disclosure and cancellation.

### BULK-06

Parent plus child: Delete a selected parent and selected descendants without redundant requests for already-cascaded children.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### BULK-07

Child only: Delete a selected child while preserving its unselected parent.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### BULK-08

Siblings only: Delete selected siblings without promoting deletion to their parent.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### BULK-09

Visual grouping: A visually grouped card must not imply deletion of an unrelated or unselected structural parent.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### BULK-10

Linked fix versus descendant: A user-created fix linked through a source reference is not automatically treated as a deletion child.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### BULK-11

Malformed hierarchy: Missing ancestors, cycles, self-parenting, duplicate IDs, and blank IDs do not hang or suppress every deletion.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### BULK-12

Ineligible deletion: Running/non-Backlog work cannot be deleted through a stale action.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### BULK-13

Partial deletion failure: Refresh successful deletions and still report the original failure.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### BULK-14

Dependency deletion: Remaining dependent tasks show missing prerequisites accurately.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RUN-01

Start: Start an eligible paused board; preserve its history and approved scope.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RUN-02

Duplicate Start: Repeated taps do not create multiple execution loops.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RUN-03

Pause timing: Pause during a provider call and between tasks; show the actual pause state.

- [x] First manual test — Pause during an active provider task593–595; UI and saved board show Pausing596–597 with the one executable task still running. Reason persisted exactly.
- [ ] Fix if needed — no pause-state mismatch established. Abort604 stopped the long current attempt before natural pause completion, so graceful completion is not claimed.
- [ ] Retest — between-task pause still pending.
- Result: IN PROGRESS.

### RUN-04

Resume: Resume paused work with the selected provider/model and applicable fallback settings.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RUN-05

Abort: Confirm and provide a reason; board becomes cancelled and interrupted tasks reflect the server’s resulting state.

- [x] First manual test — Abort confirmation601, reason602–603, submit604. Native board settles Cancelled with no current task605; server active=false/currentTaskId=null agrees.
- [x] Fix if needed — no cancellation-state failure observed. Interrupted task initially returned Todo604, then blocked by623; error626 explicitly states interrupted by board pause/abort.
- [x] Retest — task details626–627 and attempt dialog631 preserve one interrupted attempt, changed files and timestamp. Board remains Cancelled/inactive after result processing; there is no new execution attempt or success claim.
- Result: PASS for confirmed Abort with reason and resulting interrupted-task state. Other cancellation/late-response variants have separate cases.

### RUN-06

Abort cancellation: Cancel either confirmation or reason entry; board continues unchanged.

- [x] First manual test — while real G recon was active, Manage688→Abort689→Back690. Board remained running; no abort timestamp or cancellation reason.
- [x] Fix if needed — no defect observed.
- [x] Retest — reopen Abort691, proceed to reason692, enter unique unsaved marker693, press Back694. Board continued real hierarchy planning; controlRevision remained1, cancellationReason/canceledAt/abortRequestedAt all null. Neither confirmation nor reason cancellation submitted an abort.
- Evidence: screenshots688–694 and scoped read-only G state.
- Result: PASS.

### RUN-07

Late response: Provider completion after pause/abort must not overwrite newer control state.

- [x] First manual test — abort604 clears current task. Subsequent provider-result processing changes the interrupted leaf Todo→blocked and records changed files, but later623 retains board Cancelled/inactive and no running activity.
- [ ] Fix if needed — no newer-control overwrite observed in this abort path.
- [ ] Retest — successful late completion and graceful pause path still pending.
- Result: IN PROGRESS.

### RUN-08

Completed board: Stale flags must not expose an invalid Pause/Start action.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RUN-09

Future schedule: Pick date/time; show the correct intended local time and start once due.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RUN-10

Date/time picker cancellation: Cancel at either picker; preserve the prior schedule.

- [x] First manual test — A initially Not scheduled641. Change date to October3 then Cancel643–644: unchanged. Reopen645 resets to October2; select October3 and confirm646–647, then Cancel the time picker648: unchanged.
- [x] Fix if needed — no defect observed for a null prior schedule; saved scheduledStartAt remains null, board cancelled/inactive.
- [ ] Retest — cancellation while an existing future schedule is saved remains untested.
- Evidence: screenshots641–649. Filename641 incorrectly says date picker; actual date picker is642.
- Result: IN PROGRESS.

### RUN-11

Clear schedule: Clear a future schedule; verify the paused state and removed timestamp.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RUN-12

Start Now: Start a scheduled board immediately without another run starting at the old scheduled time.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RUN-13

Past/near-now time: Handle elapsed timestamps and minute-boundary races correctly.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RUN-14

Active board scheduling: Explain rejection when an active board must first be paused.

- [x] First manual test — active G: Schedule695, tomorrow October3 chosen696–697, time698, draft699, Save700. Settled701 has no persistent error and original running state/null schedule; read-only controlRevision remains1.
- [x] Fix for the blocking prerequisite — ISSUE-014 global mutation-lock stall repaired. Original relay502 after180seconds was not successful active-board validation.
- [x] Server rejection retest — new future draft October3 23:48 selected through748–753 and submitted once754. Relay records409 at16:39:23UTC; schedule remains null/controlRevision1. This is a fresh test, not replay of the old timed-out request.
- [ ] User-facing explanation retest — screenshot754caught busy transition; settled755has no persistent error. A transient toast may have occurred between captures; do not claim absence or successful explanation without observing it.
- Result: IN PROGRESS.

### RUN-15

Time boundaries: Test midnight, month/year rollover, leap day, timezone changes, and daylight-saving transitions where applicable.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RUN-16

Restart near schedule: App/server restart before or after the due time neither loses nor duplicates execution.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RETRY-01

Transient failure: Retry an eligible transport/provider failure.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RETRY-02

Defect versus transient: Failed quality/completion gates must not be misclassified as ordinary transient failures.

- [x] Completion-gate branch — G red-capture leaf781 is blocked by a completion gate. Actions782 exposes attempt history and Move to Backlog to revise, but no ordinary transient Retry. Attempt783 is correctly numbered1/Blocked.
- [x] Independent manual-evidence control — G's actual manual leaf fails both evidence and file-policy gates1021–1024. Actions1025 again has no ordinary Retry; it preserves Attempt1 Blocked and offers explicit Move to Backlog to revise. This is not classified as a transport failure.
- [ ] Fix/remaining checks — expected-red capture itself has a separate prerequisite defect under investigation; broader quality-gate and transient controls still needed.
- [ ] Independent retest — pending.
- Result: IN PROGRESS.

### RETRY-03

Dependency waiter: Waiting for dependencies must not increase transient-retry counts.

- [x] First manual test — G dependent QA leaf955–956 remains Blocked, with no transient Retry or attempt action. It has never executed.
- [x] Fix if needed — retry classification is correct; stale dependency wording is tracked independently as ISSUE-020.
- [x] Retest — another real dependency waiter, the manual leaf958–960, likewise offers only parent/discussion/revision actions. Read-only recheck after both observations confirms both attemptCount0, empty attempts and board autoRetry.attempts0; elapsed dependency waiting did not consume a retry.
- Result: PASS.

### RETRY-04

Parent versus leaf: Rolled-up parent failures are not executed as retryable subtasks.

- [x] First manual test — G blocked QA planning parent951–952 offers no transient Retry or direct execution. Stored attemptCount0/empty attempts remain unchanged at960.
- [x] Fix if needed — no inappropriate parent retry observed for this dependency-rollup branch.
- [ ] Retest — provider/quality-failed child rollup branch still needs manual coverage; do not equate a dependency-blocked parent with every failure mode.
- Result: IN PROGRESS.

### RETRY-05

Approval preserved: Retry retains valid approval without approving unrelated Backlog scope.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RETRY-06

Other gates preserved: Retry cannot bypass dependencies, side-effect approval, or planning blockers.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RETRY-07

Bulk retry: Only currently eligible tasks are submitted, even if eligibility changed while the dialog was open.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RETRY-08

Approved fix: “Retry with approved fix” uses a valid executable fix linked to the failed subtask.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RETRY-09

Invalid fix: Missing, unrelated, unapproved, or non-executable fix cannot authorize retry.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RETRY-10

Automatic retry: Enable/disable, delay, maximum attempts, and counter reset behave correctly.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RETRY-11

Numeric boundaries: Blank, zero, negative pasted values, nonnumeric input, overflow, and very large retry values produce explicit/default behavior.

- [x] First manual tests —existing CONFIG13 screenshots1187–1197 and1467–1487 cover blank/zero/overflow/min/max;1526–1531 reveal silent negative/decimal stripping, including actual clipboard paste (ISSUE038).
- [x] Fix —ISSUE029 validator plus ISSUE038 exact-input preservation implemented.
- [x] Retest —v88 screenshots1546–1564 confirm exact negative/decimal/text/overflow input rejection, real clipboard paste rejection, max-value persistence and original delay restoration.
- [x] Attempt-limit remainder —1628–1641 negative, decimal, text, actual clipboard-negative, blank preserve input and explicitly reject Save. Prior1467–1487 cover zero, overflow, min/max persistence and restore.
- [x] Delay remainder —1643–1646 zero and blank explicitly rejected;1546–1564 cover negative, decimal, text, overflow, clipboard and max persistence/restore.1648 cancels draft; retry11-after-1648.json confirms disabled/delay10/max3/counter0.
- Result: PASS.

### RETRY-12

Attempt history: Retry requests alone do not falsely increment execution-attempt labels.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RETRY-13

Fallback: Provider failure follows configured fallback behavior; invalid/unavailable fallback errors remain visible.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### RETRY-14

Recovery: Restart during execution distinguishes an interrupted worker from one still alive and prevents duplicate execution.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-01

Plain discussion: Submit a nonblank message; preserve history and display the correct task context.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-02

Blank discussion: Empty plain discussion is rejected.

- [x] First manual test — G task-2 details737 → Actions738 → Discuss739. Empty Send740 focuses Message and shows “Write a discussion message.”
- [x] Fix if needed — no validation defect observed.
- [x] Retest — entered exactly three spaces (XML741), Send742 gives the same error. Back744 dismisses without mutation. Cancel tap743 landed beneath the error popup and did not dismiss; not recorded as successful Cancel.
- Evidence: screenshots737–744; relay has no non-GET requests in this interval, original G hash/state unchanged. This is local validation and does not depend on the stalled server accepting a request.
- Result: PASS.

### DISCUSS-03

Action switching: Switching actions retains text appropriately and submits the selected action’s payload.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-04

Edit scope: Proposed edits preserve unrelated scope.

- [x] First manual test started — G failed manual leaf moved to Backlog1079–1080; files/history/dependencies unchanged by hashes. Selected Edit scope1085, entered exact verified534-character prompt1089–1090, submitted once1091 through Codex/min:MiniMax-M3. Proposal requested only title/details/criteria/valid manual contract; parent/priority/dependency/required/no-source-edit constraints retained. No edit applied yet.
- [x] Fix if needed — no scope-preservation defect observed in this edit flow. ADB truncated an earlier long entry1087; operator appended the wrong suffix1088, then replaced it with the fully verified shorter prompt1089 before submitting. Neither malformed draft was sent or attributed to the app.
- [x] Retest — reviewed native proposal summary1095 and corroborating full stored payload, then Apply1096 once.1097–1100 show both viewport criteria, same P0/Manual Test/parent path/dependency and no-source-write rule. Saved scope3→4 remains unapproved Backlog; old failed attempt digest and HTML/test hashes are unchanged. Current result cleared without reclassifying the old failure as passed. No other scope approved or executed.
- Result: PASS for the requested narrow edit and unrelated-scope preservation.

### DISCUSS-05

Replace scope: Replacement scope clearly shows what will be removed/replaced.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-06

Reprioritize: Valid priorities apply through review; invalid/blank values are handled clearly.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-07

Delete proposal: Review and apply deletion to the intended task and descendants.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-08

Split: Test one/many lines, blanks, duplicate titles, and a large list of proposed items.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-09

Merge: Test valid target, missing target, self-merge, incompatible target, and cross-board target.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-10

Regenerate children: Preserve/protect relevant existing child state and recorded effects.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-11

Re-research: Create the appropriate research workflow without silently starting implementation.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-12

Revision/fix/replacement: Each creates the correct linked work; replacement supersession is represented accurately.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-13

Required action inputs: Non-discussion actions with blank details/target/items must surface server validation errors.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-14

Proposal review: Show summary, before/after diff, warnings, errors, and status.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-15

Apply/reject: Affect only the selected pending proposal; Cancel performs neither.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-16

Already resolved proposal: Applied/rejected/failed proposals cannot be reapplied through stale UI.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-17

Stale scope: Task or descendants change after generation; outdated structural proposals are rejected safely.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-18

Concurrent decisions: Apply on one device and Reject on another; Android reconciles the authoritative result.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-19

During execution: Discussion must not overwrite the executing task’s provider/session state.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### DISCUSS-20

Interrupted generation: Disconnect/background during generation; recover the eventual proposal or failure without duplicate submission.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-01

Declaration: Save one/multiple external side-effect descriptions; reject blank declarations.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-02

Declaration versus approval: Saving a declaration alone does not approve execution.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-03

Approval: Explicit approval applies to the intended current scope/effects.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-04

Changed declaration: Changed scope/effects cannot silently reuse obsolete approval.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-05

Existing declarations: Reopening/editing must not accidentally discard previously recorded effects.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-06

Research acceptance: Display proposed planning items and optional note.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-07

Accepted research: Creates Backlog-only planning stories rather than immediately executing implementation.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-08

No research items: Record acceptance without inventing planning cards.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-09

Repeated acceptance: Duplicate taps/concurrent acceptance do not create duplicate planning items.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-10

Detach child: Detach an eligible user-created child into a Backlog story while preserving its content.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-11

Resolve recorded effects: Show affected descendants accurately; exclude unrelated siblings and handle cyclic data safely.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-12

Keep/revert/cleanup: Each sends the correct decision; Keep does not claim to undo changes, and revert/cleanup create the appropriate work.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### SAFE-13

Cancel/stale effects: Cancelling resolution causes no mutation; newly recorded effects require current review.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### CONFIG-01

Current/fallback provider: Update provider/model choices and verify displayed versus effective settings.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### CONFIG-02

Stage models: Configure Breakdown, Implementation, QA, QA Fix, AGENTS, and Final QA independently.

- [x] First manual test —1655–1673 selects6distinct stage values, saves and reopens all6exactly; primary model unchanged and no task execution. config02-after-save-1670.json corroborates.
- [x] Defects —ISSUE040 session-policy preview mismatch and ISSUE041 missing override reset reproduced during this test.
- [x] Retest/restoration —v89 native1702–1732 validates both fixes and restores original stage overrides and Continuous policy. config-restoration-1738.json compared with config02-before-1005.json differs only in logs/updatedAt; all tasks/settings restored, paused6Backlog.
- Result: PASS.

### CONFIG-03

Main-model change: Verify any cleared/reassigned stage overrides match the intended behavior and are visible.

- [x] First manual test — creation main-model change clears/reassigns stage routes, but summary retains the old count (251–256).
- [x] Fix — ISSUE-006 refreshes the summary from the changed configuration in both model/provider callbacks.
- [ ] Retest — not yet run.
- Result: IN PROGRESS after fixing and retesting the creation summary (293–300); existing-board main-model updates still need separate coverage before the entire case can pass. They preserve explicit stage overrides.

### CONFIG-04

Strategy: Manual, Cheap, Hybrid, and Expensive route work according to configuration.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### CONFIG-05

Provider-specific preferences: Switching providers and restarting does not leak another provider’s model choices.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### CONFIG-06

Reasoning/session policy: Default/Low/Medium/High and continuous/per-task settings persist correctly.

- [x] First manual test — native1163–1167 selects Low, saves and reopens showing Low.1168–1172 repeats with Medium;1173–1177 repeats with High. Each picker shows the persisted selected radio.
- [x] Fix if needed — none observed.
- [x] Retest —1178–1182 restores Default and verifies it after save/reopen. Continuous→Per-task→Continuous save/reopen coverage1152–1162 is independently corroborated under CONFIG16. No execution triggered.
- [x] Unsaved-control check — toggle Thinking1183, leave with Back1184 and reopen1185; original unchecked value retained.
- Result: PASS.

### CONFIG-07

Git policy: Read-only versus managed policy is reflected in execution and evidence.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### CONFIG-08

Tool access: CLI defaults, shell enabled, and shell disabled persist and affect execution correctly.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### CONFIG-09

TDD: Test enabled/disabled, failing-baseline requirement, allowing no generated tests, and fix-attempt limits.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### CONFIG-10

Validation: Feature/final/QA commands, limits, and timeout values persist without corrupting multiline commands.

- [x] First manual test —1573/1575/1577 enter two distinct lines in each command field;1579 saves and1581 reopens all six exact lines.
- [x] Numeric persistence —1582–1590 set limits3/5/4 and timeout121;1592–1593 reopen and visually confirm all commands and values.
- [x] Restoration —1594–1604 restore empty commands, limits2/4/2 and timeout120 using native UI; config10-restoration-1604.json confirms validation/TDD/RAG/QA exactly match the original snapshot. Retry values also restored; only its update timestamp differs. F remains paused/inactive; no command executed.
- [x] Fix if needed —no defect observed for this case.
- Result: PASS.

### CONFIG-11

Retrieval: RAG enabled/disabled, scopes, and context-character limit behave as configured.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### CONFIG-12

QA policy: High risk, all tasks, and off; verify follow-up and attempt limits.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### CONFIG-13

Invalid numeric values: Blank, zero, negative, noninteger, and overflow inputs are handled explicitly without silent substitution.

- [x] First manual test —1187–1197 retry delay blank silently becomes10 on save/reopen; zero closes the form. See ISSUE029.
- [x] Fix — ISSUE029 native field validation implemented in v80/81.
- [x] Additional retry attempt-limit retest on v87 —1467–1471 zero rejected with explicit1..2147483647 error and retained draft;1472–1473 overflow2147483648 likewise rejected.1474–1477 maximum2147483647 saved/reopened exactly;1478–1483 minimum1 saved/reopened exactly.1484–1487 restored original3. Read-only board confirms disabled/delay10/max3/counter0,Paused/inactive.
- [x] October5 additional Max fixes checks —1680–1682 blank and1683–1685 decimal1.5 remain intact and reject explicitly as whole number0..20;1686Back cancels. No quality save submitted.
- [x] Additional Max fixes rejection matrix on v92 —1925–1929 above-limit21 is rejected;1932 overflow2147483648,1935 negative-1, and1938 nonnumeric `abc` each retain the exact draft and show the whole-number0..20 error. Blank/decimal rejection is covered separately by1680–1685. No silent substitution or additional defect observed.
- [x] Max fixes endpoints —1939–1943 save0/reopen0 and1944–1948 save20/reopen20; stored-state receipts `config13-maxfix-min-1941.json` and `config13-maxfix-max-1946.json` corroborate each endpoint.1949–1951 restore original3; `config13-maxfix-restored-1951.json` confirms3 and matches1897 baseline except logs/updatedAt.
- [x] Feature command limit rejection matrix on v93 —1954–1971 explicitly reject above-limit21 (1956), blank (1959), decimal1.5 (1962), negative-1 (1965), nonnumeric `abc` (1968), and overflow2147483648 (1971). Each shows the whole-number0..20 error and retains the entered draft; no silent substitution or additional defect observed.
- [x] Feature command limit endpoints/restoration —1972–1976 save0/reopen0;1977–1981 save20/reopen20;1982–1984 restore original2. `config13-feature-restored-1984.json` confirms2 and matches1897 baseline except logs/updatedAt.
- [x] Final command limit rejection matrix on v98 —2287–2304 tests21, blank,1.5,-1,abc,2147483648 individually. Save retains each exact draft and shows “Final command limit must be a whole number from0to20” (screenshots2289,2292,2295,2298,2301,2304 visually inspected). No silent substitution observed.
- [x] QA command limit rejection matrix on v98 —2305–2324 restores draft Final4, then individually tests21, blank,1.5,-1,abc,2147483648 in QA. Screenshots2309,2312,2315,2318,2321,2324 show the explicit whole-number0..20 error and retained values.2325 Android Back cancels; `config13-final-qa-cancelled-2325.json` is exactly equal to1984 persisted baseline, including logs/updatedAt.
- [ ] Remaining — Final/QA accepted endpoints and other field-specific boundaries (timeout, context characters, QA followups/task attempts); prior retry/negative-paste evidence retained.
- Result: IN PROGRESS.

### CONFIG-14

Cancel semantics: Back/Cancel from nested settings must not unexpectedly apply draft changes.

- [x] First manual test — existing-board Quality gates636 has TDD enabled. Uncheck637, press dialog Back638, reopen639: enabled again; saved TDD remains true.
- [x] Fix if needed — no additional defect observed; creation-specific draft fix was already tested under ISSUE-004.
- [x] Retest — Board strategy650 shows Manual; choose Cheap651–652, dismiss using Android Back653, then reopen654–655: Manual again. Main model, implementation override and unrelated TDD settings remain unchanged. No Save was submitted.
- Evidence: screenshots635–640 and650–656, corroborated by read-only A board state. Covers button Back and system Back in existing-board settings, not concurrent Save behavior.
- Result: PASS.

### CONFIG-15

Partial save failure: Some settings use multiple requests; if a later request fails, accurately show which changes persisted.

- [x] First manual cycle —1687–1693 edits Max fixes3→4 and Feature limit2→3, submits once with exact validation PATCH response dropped after forwarding. TDD and validation persisted; RAG/QA requests did not follow. config15-fault-events-1005.json and config15-after-lost-1693.json corroborate.
- [x] Fix and native retest —ISSUE042 adds durable outcomes;1772–1775 verified acknowledgment vs reconciled matching values vs unattempted sections, and read-only recovery without duplicate PATCH.
- [x] Restoration —1779 reopened Max fixes3/Feature limit2; config15-v91-restored-1779.json matches baseline except logs/updatedAt.
- [x] Additional native failure positions —1780–1781 TDD response lost: matching TDD, three unattempted sections;1785–1786 Retrieval response lost: two acknowledged, Retrieval matching, QA unattempted;1790–1791 QA response lost: three acknowledged, QA matching. Each exact one-shot fault forwarded once. These branch tests saved unchanged baseline values.
- [x] Verification-read failure —1795–1796 all four PATCHes acknowledged, subsequent GET503: all four acknowledgments retained with explicit reload failure.1797–1798 read-only reload recovers board and clears read error.
- [x] Unknown write plus read failure —1802–1803 TDD response lost plus GET503: TDD correctly unconfirmed, remaining sections unattempted.1804–1805 read-only reload changes TDD to matching requested settings without resubmission. Proxy complete-events evidence confirms expected PATCH counts1/3/4/4/1 across additional cycles and no reload writes. Final state matches original tasks/configuration; only logs/updatedAt differ.
- Result: PASS.

### CONFIG-16

Unrelated settings: Updating one policy preserves the other settings.

- [x] First manual test — F strategy1152–1155: change only Continuous session to Per-task session and save. Reopen1157 confirms selected value. Read-only baseline/after comparison confirms only sessionPolicy changes to task-model; all other configuration preserved.
- [x] Fix if needed — none observed.
- [x] Retest — restore Continuous session1158–1160; reopen1162 confirms persistence. All19 compared configuration fields match original baseline exactly. Six tasks remain paused/unapproved, unchanged dependencies/attempts. Evidence: resume1004-f-config16-before.json, resume1004-f-config16-after-save.json, resume1004-f-config16-restored.json.
- Result: PASS.

### CONFIG-17

Concurrent update: Changes from another client while a settings dialog is open do not produce an unexplained stale overwrite.

- [x] First manual test —1816–1820 reproduced stale overwrite; native changes Feature limit only but silently overwrites second-client Max fixes. Controlled second client used authenticated scoped HTTP fixture; native Save and result manually observed. See ISSUE043.
- [x] Fix —ISSUE043 atomic guarded field deltas.
- [x] Quality-settings retest —1854–1897 verifies cross-section and same-section preservation, whole-request409 rejection, read-only lost-response recovery, reverted-edit/no-op with no PATCH, and baseline restoration.
- [ ] Remaining —strategy, main model, tools and retry dialogs still require concurrent-edit branches; this quality fix does not establish their behavior.
- Result: IN PROGRESS.

### DETAIL-01

Task details: Correct title, description, hierarchy breadcrumb, priority, status, dependencies, and origin.

- [x] First manual test — baseline962/967 and manual958 details have correct task IDs, titles, descriptions, hierarchy breadcrumbs, P0 and current statuses. But963–964 labels a completed dependency `Blocked by`.
- [x] Fix — ISSUE022 replaces misleading Blocked by with Depends on while retaining actual edges and distinct current blocker errors.
- [x] Retest — completed G leaf1007 and unfinished F group1032–1033 both show accurate dependency semantics. F's title/description/root breadcrumb/P1/Backlog/User prompt-generated origin match persistence; G has the correct nested path, status and P0. Read-only F hash unchanged, no execution.
- [x] Reopened by a distinct ingress defect — accepted AI preview1049–1050 is wrongly persisted/rendered as User manual and has no dependency. ISSUE022 label fix remains verified; ISSUE027 must repair generated-draft origin/dependency preservation.
- [x] Ingress repair retest —1120–1121 and refresh/reopen1123–1126 show accurate generated origin and Depends on task-2 for fresh task4, with preview-matching title/details/criteria and P1/Backlog. Original failed task3 preserved. Earlier hierarchy/status/label coverage remains valid.
- Result: PASS.

### DETAIL-02

Group details: Correct nested tasks, completion counts, and available actions.

- [x] First manual test — root-story group shows correct Backlog/P2/no-nested-work/full description, but incorrectly calls its own breadcrumb Parent (201).
- [x] Fix — ISSUE-005 headings now say Hierarchy path.
- [x] Retest — real root262 and nested fixture leaves362/364 use correct paths. Group details366/369–370 show the correct two/four nested records, zero/one completed and origins.
- Result: IN PROGRESS; label and counts fixed/retested, complete available-action coverage remains pending.

### DETAIL-03

Current activity: Display actual executing subtasks rather than parent status rollups.

- [x] First manual test — real active harness595–597 shows exactly one running Subtask with matching title/start time, despite both its Story and Task parents also rolling up In Progress.
- [x] Fix if needed — no duplicate parent activity observed.
- [x] Retest — after Abort,605/623 show no task running; no parent or stale current-task activity remains. Persisted currentTaskId is null.
- Result: PASS for this sequential real-provider run; parallel activity is DETAIL-04.

### DETAIL-04

Parallel activity: Show all executing work once; stale current-task IDs do not create phantom activity.

- [x] Multiple-executing response branch —2166 GET-only fixture renders exactly two Current activity entries, TEST parallel one/two, with distinct start timestamps. The in-progress Story is excluded from executing-leaf count, and nonexistent currentTaskId `TEST-stale-missing` creates no phantom entry.
- [x] Current-task preference branch —2167 setting currentTaskId to task two reorders it first while still rendering exactly two executing entries, once each.
- [x] Stale-completed current task —2168 changes task two to Done while retaining it as currentTaskId. Current activity correctly shows only task one and1task running; completed task two remains visible in nested work without phantom activity.
- [x] Fix if needed —no display/count defect observed in these branches.
- Evidence:2166–2168 PNG/XML and `TEST-DETAIL04-parallel.json` response fixture. This verifies native activity projection from returned state, not real parallel provider execution or scheduling.
- Result: PASS for response-driven multi-executing/stale-current-task activity UI.

### DETAIL-05

Planning activity: Clearly identify provider planning/research activity when no implementation subtask is running.

- [x] First manual test — actual Codex/min:MiniMax-M3 reconnaissance585 is labeled Reviewing project context while no implementation subtask runs.
- [x] Fix if needed — no misleading idle state in this run.
- [x] Retest — story breakdown588 and deeper child planning592 are labeled Breaking down work with the current planning target; execution595–597 then switches to the actual running subtask.
- Result: PASS.

### DETAIL-06

Attention panel: Counts/actions exclude ordinary dependency waiters where appropriate.

- [x] Dependency-only branch — after return/refresh980–981, G has two real dependency-blocked executable leaves plus rolled-up blocked parents. UI correctly says Waiting on other tasks, offers no Needs attention runtime-failure panel and shows no executing task. Read-only state confirms no ordinary provider errors on these current leaves.
- [x] Fix if needed — no inflated runtime-attention count observed in this branch.
- [x] Mixed response-fixture native observation —2171/2174/2177 combines an error-bearing runtime-failed leaf and ordinary dependency waiter. Board renders Blocked/Waiting on other tasks with no Needs attention panel.2172 failing-leaf details correctly shows `TEST provider command exited 17`;2173 Actions offers Move to Backlog to revise.2175 waiter details correctly shows `Waiting on dependency: TEST-parallel-one` and its dependsOn;2176 Actions offers only Open parent/Discuss.
- [x] Diagnosis/fix —ISSUE-048 confirmed: attention projection expected a column never returned by kanbanColumnId. Separate qualifying-failure projection implemented while preserving workflow columns and existing action eligibility;743 shared tests/78suites pass. v97 build/native retest pending.
- [x] Native v97 repair retest —2200 mixed fixture shows Needs attention/View1 and unchanged Blocked1.2201 expanded group retains nested contextual children without unauthorized Retry;2202 actual-failure detail opens correctly,2203 closes.2204 dependency-only control removes attention entirely while retaining blocked group.
- Scope —native response-driven inclusion/counts/navigation and permission-preserving action presentation verified; no actual provider execution inferred from fixture state.
- Result: PASS for observed mixed-failure attention and dependency-only exclusion UI.

### DETAIL-07

Attempt details: Correct stored execution numbers, errors, usage, and evidence for each attempt.

- [x] First manual test — interrupted task627 correctly labels Attempt1 Blocked. Actions630→View Attempt1 shows the corresponding attempt ID, number, start/finish timestamps,330114ms duration, interrupted=true, error, four changed files, empty command/evidence lists and chronological transcript631.
- [ ] Fix if needed — no attempt-number/error mismatch observed; raw JSON is readable but dense.
- [x] Populated usage/multiple-attempt native retest —2181–2182 lists the correct two attempt labels.2183 opens failed Attempt1 with distinct ID, compiler error, usage101input/11output/112total and command exit17.2187 opens completed Attempt2 with its own ID, null error, usage202/22/224 and command exit0. `detail07-attempt-payload-comparison.json` confirms both rendered payloads exactly match the supplied records.
- Scope —combines earlier real stored attempt metadata with controlled `TEST-DETAIL07-attempt-usage.json` UI rendering. Synthetic values verify record isolation and presentation; they are not real provider token measurements.
- Result: PASS for observed attempt-specific numbers, errors, usage and evidence presentation.

### DETAIL-08

Validation history: A later pass does not erase earlier failures or imply every command passed.

- [x] First manual test — G baseline964/968 has both Attempt1 Blocked and Attempt2 Done. Opening Attempt1 at969 preserves its exact attempt ID, error, exit1 commands and ten failing assertions; scrolling972 still shows its original report after later implementation/QA have passed.
- [x] Retest — reopened current Attempt2 at974–976: distinct ID/number, null error, unchanged read-only test scope, explicitly retained exit1 and named failing assertions. It has not overwritten Attempt1 or claimed every command passed.
- [x] Synthetic fail-then-green attempt branch —2183 preserves failed first-attempt compiler error/exit17 while2187 independently shows later completed attempt/null error/exit0 for the same fixture task. Exact payload comparison confirms no overwrite. This is controlled response history, not an actual new validation run.
- [ ] Remaining —readable aggregate validation history after ISSUE-021 and actual same-task fail-then-green validation execution remain. Accepted expected-red is not itself green tests.
- Result: IN PROGRESS.

### DETAIL-09

Expected failing baseline: Accepted expected-red results are distinguished from both defects and passing tests.

- [x] First manual test — G962–964 retains real exit1 assertion failures for the accepted expected-red task, but its group chip966 says `Baseline failed` without distinguishing accepted red from a defect.
- [x] Fix — ISSUE021 evidence-aware summaries,89focused/636full tests, deployed APK77.
- [x] Retest — same real saved leaf1006 distinguishes accepted expected-red from green and keeps the earlier failed gate in history. Actual group badge1009 also says Baseline accepted(expected red). No evidence was changed to achieve this presentation.
- Result: PASS.

### DETAIL-10

Missing evidence: Unknown results are not displayed as passed or borrowed from a previous task/attempt.

- [x] First manual test — never-executed G manual leaf958–959 shows Blocked/TDD QA Pending and explicitly says no transcript, changed-file record or QA evidence has been recorded. It does not borrow the neighboring HTML/QA success.
- [x] Fix if needed — none for this case.
- [x] Retest — Actions960 exposes no attempt/transcript result; persisted attemptCount0, empty attempts and null validation corroborate the UI after adjacent tasks have completed. Required viewport metadata is not presented as completed verification.
- Result: PASS.

### DETAIL-11

Manual verification: Show applicable environment and evidence; a success label alone must not imply verified completion.

- [x] First manual test — real manual execution1017 produced local screenshots, but the actual completion gate rejected invalid scope and edits1018–1022. Task details1021 misleadingly say Latest validation:Passed/Latest gate passed while the task and evidence gate are Blocked/failed.
- [x] Evidence visibility —1023 shows the supplied manual-test environment and precise changed files;1024 shows ordered browser actions, dimensions and the disclosed data-URL/network limitation. These remain provider claims, not proof the failed evidence contract was satisfied. Root inspected the generated360×800 full-page PNG separately; its pixels match the static shell, not a complete interactive app.
- [x] Fix — ISSUE026 separates server completion/manual/file-policy verdicts from automated validation; real native1058/1060/1062 controls pass without rewriting evidence.
- [ ] Remaining retest — fresh valid manual execution after024/025; inspect its environment/criterion evidence instead of trusting the provider's success summary. Failed task environment/actions were already visible1023–1024.
- Result: IN PROGRESS.

### DETAIL-12

Chat link: Opens the correct linked provider session; absent/deleted sessions fail gracefully.

- [x] First manual test — baseline Chat978–979 opens exactly linked session_b67f4c9023e84966ad533f64da6f6e9b, corroborated by relay GETs and read-only session record: Codex/min:MiniMax-M3, project G. Correct board-owned chat header and Back to board control appear; no message sent.
- [x] Retest —980 returns to G with its same four feature groups, counts and blocked state. Session is shared with QA under configured session policy, so its latest message is the QA turn; this is not falsely reported as a leaf-only transcript.
- [x] Missing/deleted-session branch —2107–2110 synthetic task references a nonexistent session. Real snapshot404 then messages200/empty silently opens No messages/New chat with Send enabled, no error and no Back to board: ISSUE-045. No message submitted.
- [x] Native v96 missing-session retest —2146 explicit Task chat unavailable explanation with Retry/Back to board;2147 Retry preserves accurate error;2148 returns to F. Actual missing-session snapshot404 reads have no messages fallback in the retest receipt.
- [x] Existing-session regression —after clearing fixtures, transient G load timeout2151 recovers through Retry2152–2153;2154–2155 actual nested task chat opens with transcript and board-return control;2156 returns to correct G. No message sent.
- Scope —covers linked existing session and nonexistent/deleted-equivalent404 recovery; no claim of every transport or permission-error branch.
- Result: PASS for existing linked-session navigation and absent/deleted-equivalent404 recovery.

### DETAIL-13

Transcript: Correct task/session entries, ordering, roles, and complete readable content.

- [x] Board-generation transcript branch — F816 links its exact provider session;817–818 show full original prompt then complete model task JSON in order, correct Codex/min:MiniMax-M3 attribution, timestamp and1m45s duration. Tail dependency matches the saved task.
- [x] Fix if needed — no lost/incorrect text in this branch, although raw JSON is laborious to read on a phone.
- [x] Additional task transcript —1292 correct A child prompt;1293 selection handles/Copy/Share/Select all,1294 Back dismisses selection.1295–1304 individually inspected scrolling reaches model JSON and final abort record; fixed Back remains reachable,1305 closes.
- [x] Defect found —1303/1304 omit literal HTML tag strings from assistant JSON; stored transcript retains them. See ISSUE-031.
- [x] Literal-content/multi-attempt native retest —2125 renders four transcript entries in exact source order: first prompt/result, then second prompt/result. `detail13-literal-comparison-2125.json` reports allExact=true with increasing indices13/22/28/37. Both assistant JSON strings preserve literal HTML/script tags, escapes and Unicode content as text; no script execution observed. This is controlled transcript-fixture evidence, not verification of the fixture’s claimed provider/model identity.
- [ ] Remaining —complete case-wide attribution/session checks beyond the already verified actual provider session; literal preservation and two-attempt ordering branches now pass.
- Result: IN PROGRESS.

### DETAIL-14

Large evidence: Long JSON, command output, diffs, file lists, and transcripts remain scrollable/selectable.

- [x] First manual test — long raw attempt JSON/command evidence969 wraps and scrolls972 without losing the fixed Close control. Long-press970 selects test-path text and exposes Copy/Share/Select all; dismissal971 returns cleanly without sending/copying anything.
- [x] Fix if needed — no crash, inaccessible Close or nonselectable evidence in this branch; raw JSON remains laborious on a phone.
- [x] Transcript-specific large payload —1292–1305 prompt selection and all scroll segments through assistant JSON to final abort message; Back remains reachable. JSON tag omission tracked separately as ISSUE-031.
- [x] Long diff/file-list native retest —controlled nested TEST fixture:2130 Actions orders Attempt1 Failed before Attempt2 Completed;2131 opens exact FIRST ATTEMPT FAILED payload and2135 opens correct Attempt2 title/body.2136 scroll reaches diff lines30–40;2137 reaches file040 and END LONG EVIDENCE while Close remains reachable.2138 long-press exposes native Copy/Share/Select all;2139 Select all highlights the body. No Copy/Share or mutation submitted.
- Scope —earlier actual command-output/JSON/transcript evidence plus this controlled long diff/file-list branch establishes observed scrolling/selection and reachable dismissal. This is not a stress test of arbitrarily large payloads.
- Result: PASS for observed long JSON, command output, diff, file-list and transcript readability/selection.

### DETAIL-15

Malformed evidence: Missing/null/legacy fields do not crash the details dialog.

- [x] Missing-field branch —2007 older/missing-field task details renders defaults safely, without crash.
- [x] Legacy malformed-evidence branch —2106 controlled three-card GET fixture;2114 details tolerates object-shaped attempts, array result, string QA/validation values and legacy object references. The actual transcript field was absent in2114, so that screenshot does not establish scalar-transcript handling. It renders validation Unknown/QA Not set and readable reference text rather than crashing.2115 Actions remains reachable;2116 Cancel returns.
- [x] Corrected transcript-shape retest —after supplying the actual transcript field as scalar/null rather than an unused field,2119 explicitly repeats the scalar-transcript details branch safely without a crash or invented history.
- [x] Null-evidence branch —2117 null-field task details opens safely with no invented attempt/history, and Close/Actions remain available.
- [x] Fix if needed —no details-rendering defect observed for these fixture types. Missing linked session is separately tracked as ISSUE-045/DETAIL-12.
- Evidence:2007,2106 and2114–2119 PNG/XML; `TEST-DETAIL15-legacy-evidence.json`. Response-only fixtures; no stored task mutation. This bounded set covers observed missing/null/legacy shapes, not every possible malformed payload.
- Result: PASS for tested missing/null/legacy detail field shapes.

### ANDROID-01

Polling lifecycle: Enter Board repeatedly; only one refresh loop runs.

- [x] First manual test — v91 on dedicated5596: leave F for Projects1807–1808, reenter1809; repeat exit/reentry1811–1812 and1813–1814. All three native cycles return to the same paused F/Backlog6 without submitting task actions.
- [x] Request evidence — before navigation,17 successful F detail reads04:51:02–04:52:38UTC have median6.005s cadence. After exit, trailing list completion04:52:44 is followed by no board reads until reentry list04:53:59/detail04:54:03. Final reentry1814 screenshot is04:56:19.988; subsequent detail sequence through04:57:21.765 spans more than60s after that marker, alternates list/detail, and contains no duplicate request bursts. Excluding the initial entry read,10 detail reads have median6.098s, range5.813–6.873s; repeated entries did not multiply the cadence.
- [x] Fix if needed — no defect observed. Source corroborates a single active-job guard and3s delay plus sequential list/detail request duration;3s is not a fixed completion-to-completion interval. Leaving Board cancels the job.
- [x] Evidence review — native1814 visibly shows paused F,6Backlog and no running task; proxy report contains no mutations. Ordinary proxy timestamps record completion, so this is observed cadence evidence rather than direct instrumentation of coroutine or simultaneous in-flight counts. Sanitized report: android01-polling-lifecycle-1814.json.
- Result: PASS for repeated native Board entry/exit and single observed polling cadence. Busy-action and process-recreation behavior remain their separate cases.

### ANDROID-02

Leave/disconnect: Leaving the tab or disconnecting stops irrelevant refresh/re-render activity.

- [x] Leave-screen branch — F808 returned to Projects during generation. Last F detail GET17:04:51UTC; no further F polling through17:06:10 while on Projects. Reentry810–811 restores actual saved state.
- [x] Fix if needed — no leave-screen polling defect observed.
- [ ] Retest — explicit server-disconnect branch remains untested.
- Result: IN PROGRESS.

### ANDROID-03

In-flight refresh: Switch project/board during refresh; stale data cannot replace the current screen.

- [x] First attempt —1346 A delayed read,1347–1348 navigation to Projects; hold timed out before1349 opened F, so not counted as a successful race test.
- [x] Actual race —1354 held real F detail GET with TEST-ANDROID03-F-to-A-1004-1353-long;1355–1356 leave to Projects,1357 open A,1358 correct A cancelled/Blocked1 while F response still held. Explicitly release F;1359 remains A with correct title/count/cards, no stale F replacement.
- [x] Fix if needed — stale-result guard passes observed cross-project race. Abandoned read loading indicator remains visible on Projects and A until F completion; separate ISSUE-032.
- [x] Retest — v84 initial-read overlap1374–1379 confirms old A completion cannot replace F or clear F loading; F header refresh1380–1385 does not leak indicator onto Projects/A. Header stream disconnected; late-delivery proof comes from1378 telemetry.
- Result: PASS after fix.

### ANDROID-04

Action versus polling: Refresh pauses during actions and resumes after both success and failure.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### ANDROID-05

Unchanged data: Polling unchanged state does not flicker or repeatedly rebuild the UI.

- [x] First observation — G mid-list986 and987 are byte-identical screenshots (SHA b7f0c975...), spanning several successful detail polls18:46:38–18:47:02UTC without user input or visible offset change.
- [x] Fix if needed — no endpoint loading spinner/rebuilt-position change observed in these captures.
- [ ] Retest — continuous observation/video is still needed to rule out brief flicker between sampled screenshots; identical endpoints alone are not proof of every frame.
- Recording limitation — the20-second Android screenrecord990 contains only one encoded frame/zero stream duration. Preserve it as a tooling limitation, not continuous video proof; final PNG990 is valid and inspected.
- Result: IN PROGRESS.

### ANDROID-06

Scroll preservation: Same-project, same-board, same-column refresh preserves scroll position.

- [x] First manual test — scrolled the real G Blocked list985→986, then allowed multiple successful automatic refreshes.987 retains the exact item positions and pixel-identical image.
- [x] Additional failing manual branch — horizontal signal row1009 resets left1010 on ordinary polling, while the same group's vertical offset remains stable.
- [x] Fix — ISSUE023 adds scoped native horizontal restoration; APK78 verified after full658test pass.
- [x] Retest — real G badge row1064→1065 retains exact settled bounds across successful polls;1066→1067 preserves a second user-selected offset, not a stale saved value. Vertical positions remain fixed. Done1068 starts at its own top. No server data was changed for the presentation test.
- Result: PASS for observed same-project/board/column vertical and horizontal refresh preservation. Hidden-row and changing-width native branches remain separate extended checks, not implied by helper tests.

### ANDROID-07

Scroll isolation: Different projects, boards, and columns do not inherit unrelated offsets.

- [x] Column branch — from scrolled G Blocked987, switch to Done988: starts at its own top with the correct completed group, not the unrelated Blocked offset. Return989 likewise starts the correct Blocked column, with no blank space or accidental action.
- [x] Fix if needed — no cross-column offset leakage observed.
- [ ] Retest — cross-project and cross-board offsets remain to cover.
- Result: IN PROGRESS.

### ANDROID-08

Shrinking list: Deleting/moving cards while scrolled near the bottom does not leave blank inaccessible space.

- [x] Read-only shrink branch —after scrolling the1000-task synthetic GET fixture near item0993, replace response with the same board containing only three tasks.2045 viewport clamps to the three remaining cards rather than leaving blank inaccessible space.2046 swipe reaches header;2047 Refresh remains reachable.
- [x] Fix if needed —no defect observed in this response-shrink branch; no real task deletion/move submitted.
- [ ] Remaining —actual delete/move end-to-end while scrolled, selection reconciliation and other column/shrink combinations. Response replacement alone does not complete those action paths.
- Evidence:2042–2047 PNG/XML; `TEST-ANDROID19-large-1000-1005.json` and `TEST-ANDROID08-shrunk-3-1005.json` GET-only fixtures.
- Result: IN PROGRESS.

### ANDROID-09

Background/resume: Restore authoritative board state without duplicate actions or polling loops.

- [x] Paused-board branch — H897 shows one original Backlog Story. Home898 backgrounds the activity (ATD EmptyHomeActivity is intentionally black, confirmed by dumpsys). Resume899 briefly shows a connection timeout with actions disabled; without tapping Retry,900 restores the same authoritative paused board. H saved hash remains61e8ebb6; no mutation was resubmitted. Detail polling paused after17:58:02UTC and resumed17:58:33 as a single roughly5.6-second sequence.
- [x] Fix if needed — no persistent resume defect observed in this branch; the transient connection warning is recorded rather than omitted.
- [x] Busy-board branch — H Breakdown911 was submitted once; Home912 and resume913–914 showed a persistent connection warning. Explicit read-only Retry loading915 recovered the actual saved five-task plan916 after the provider finished. Relay confirms one breakdown POST completed200 at18:08:17UTC, then ordinary board reads; no duplicate submission. A transparent relay cannot prove whether Android recreated the activity during backgrounding.
- [ ] Remaining manual coverage — active executable-work changes and process-recreation distinction remain; transient/explicit recovery observations are recorded without claiming every lifecycle path.
- Result: IN PROGRESS.

### ANDROID-10

Rotation/recreation: Test with forms, confirmations, details, and proposals open; no crash, duplicate submission, or wrong target.

- [x] First manual test — manual task draft718 with unique title. Rotate portrait→landscape719: title/dialog/G target preserved; footer actions visible. Scroll720, restore portrait721: title still intact, no submission or crash. Cancel722; board remains two records.
- [x] Fix if needed — no defect observed for this form rotation branch. Device settings restored exactly to original accelerometer_rotation1/user_rotation0 and verified722.
- [x] Details branch — H Story details901 survive portrait→landscape902, show the full unchanged description after scrolling903, and return to portrait904 with error/transcript preserved. Actions/Close remain reachable;905 closes normally. Original device rotation settings restored and verified (accelerometer_rotation1/user_rotation0); no mutation submitted.
- [x] Confirmation rotation —1336–1339 six-task removal confirmation retains count/target and accessible buttons across landscape/portrait; Cancel preserves all tasks. Original rotation settings1/0 restored.
- [ ] Retest — proposals and actual activity recreation during in-flight work remain untested; orientation is not substituted for recreation.
- Evidence: screenshots716–722, no server mutation submitted.
- Result: IN PROGRESS.

### ANDROID-11

Process death: Restart after Android kills the app; reconcile submitted operations with server state.

- [x] Idle forced-stop branch —1312 detail open;1313 force-stop;1314 restart shows native loader;1315 authenticated Projects.1320–1322 reopen A, same cancelled board and correct child. This is forced-stop coverage, not OS memory-pressure or in-flight reconciliation.
- [x] Fix if needed — no crash or lost server data in observed idle restart branch.
- [ ] Retest — kill during submitted operation and reconcile outcome.
- Result: IN PROGRESS.

### ANDROID-12

Network changes: Offline, slow network, Wi-Fi/mobile switching, and reconnect show accurate state and useful recovery.

- [x] HTTP-failure branch —1351 injected exact F detail HTTP503 shows error and disabled actions. Clear fault;1352 Retry shows loading,1353 returns correct paused F/Backlog6 with restored actions.
- [x] Slow-read branch —1346/1354 loading stays responsive to Back; abandoned indicator defect ISSUE-032 found.
- [ ] Remaining — actual offline/Wi-Fi/mobile switching and reconnect.
- Result: IN PROGRESS.

### ANDROID-13

Ambiguous response: Server accepts an action but response is lost; retry must not silently duplicate work.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### ANDROID-14

Touch conflicts: Scrolling, tapping cards, expanding groups, selecting cards, and pressing buttons do not trigger one another accidentally.

- [x] Partial native touch test —1898 swiping from card body scrolls without opening details;1899 swiping from an unchecked checkbox scrolls without selecting it (visible checkboxes remain unchecked).1900 direct checkbox tap selects only Show counter bounds warning and does not open details.1901 swipe starting on its checked checkbox scrolls while preserving exactly1 selected.1902 direct deselection clears it and removes the bulk-selection bar.1903 direct Open details tap opens Configure counter bounds;1904 Close returns to board.
- [x] Fix if needed —no defect observed in the tested flat-card touch branches; no task mutation submitted.
- [x] Nested native touch continuation —live G2158 vertical child-body swipe and2159 swipe beginning on Chat scroll safely without opening details/chat.2160 horizontal swipe opens child details incorrectly;2161 closes,2162 opposite horizontal swipe repeats. ISSUE-046; no task mutation submitted.
- [x] Partial v97 gesture retest —2195–2197 horizontal child/checkbox swipes no longer trigger taps;2198 small jitter tap opens details correctly and2199 closes.
- [x] Original live G v98 retest —with fixtures cleared before2239,2243–2244 load actual server state;2245–2246 vertical Chat/body gestures scroll safely,2247–2248 original horizontal gestures in both directions do not open details,2249 horizontal Chat swipe does not open chat,2250 ordinary tap opens the correct same child, and2251 closes to the same board/scroll position. ISSUE-046 fixed and manually retested.
- [ ] Remaining —nested-group expansion/selection and other button/column combinations; completing the reproduced gesture regression does not establish every touch path.
- Evidence: screenshots/XML1898–1904 in artifacts/kanban-manual-20261001.
- Result: IN PROGRESS.

### ANDROID-15

Rapid interaction: Repeated taps, rapid column changes, and opening/closing dialogs do not stack duplicate requests/dialogs.

- [x] First manual test —1199 rapid column changes retain correct final column;1200–1202 duplicate details confirmed, see ISSUE030.
- [x] Fix — shared detail-window guard implemented and built in v81.
- [x] Detail-window retest —1226–1230 repeated group taps/close/reopen and1280–1290 repeated child taps/parent navigation/menu route pass on v81.
- [ ] Remaining — other rapid actions/requests and actual Activity recreation.
- Result: IN PROGRESS.

### ANDROID-16

Keyboard: Input fields remain visible above the keyboard; multiline entry, paste, Back, and IME actions behave correctly.

- [x] Partial manual test —1573/1575/1577 multiline entry and1578 Back preserve six lines;1581/1592 reopening confirms persistence.1551 real clipboard paste retains exact negative value.1600–1603 clear multiline fields and Back retains empty draft.
- [ ] Remaining —IME actions and other low fields; not yet complete.
- Result: IN PROGRESS.

### ANDROID-17

Small/large displays: Portrait, landscape, split-screen, display scaling, and large font sizes keep actions reachable.

- [x] Partial native layout test —1826–1835 landscape1600x720, density320/font1.0: Board footer reachable and Quality gates scrolls to Task attempts with Back/Save fixed and reachable.1842–1846 portrait720x1600/font2.0: Board and quality dialog render enlarged text, lower fields/actions reachable by scrolling. Some compact Board labels ellipsize at2x.
- [x] Restore environment —1847–1848 close without edits; font1.0/user_rotation0/accelerometer_rotation1 restored. No setting save or task action during display tests.
- [x] Additional native detail/layout checks —2252–2254 actual G task details at font1.0 landscape1600x720 scroll with Actions/Close reachable.2255–2274 font2.0 recreates Activity; F board, task details, Actions and Quality gates tested in landscape. Detail actions and dialog Back/Save are present; invalid Max fixes21 displays a readable validation popup (2274), overlapping some dialog action area.2275–2277 Save leaves the draft/dialog, then Android Back dismisses without saving. This establishes an exit route, not that every button remains tappable while the popup is present.
- [x] Restore environment —2278 font_scale1.0/user_rotation0/accelerometer_rotation1;2325 restores screen_off_timeout1800000. Persisted F at2325 exactly equals1984 baseline. No valid configuration save or task mutation performed during this continuation.
- [ ] Remaining —display density/split screen and broader validation-error layouts. Font-scale configuration change recreated Activity and returned to Projects; draft restoration remains a separate ANDROID10/11 concern.
- Result: IN PROGRESS.

### ANDROID-18

Accessibility: TalkBack labels, focus order, selected state, status meaning, and dialog focus are understandable.

- [ ] First manual test — not yet run.
- [ ] Fix if needed — not assessed.
- [ ] Retest — not yet run.
- Result: NOT RUN.

### ANDROID-19

Long-running board: Hundreds/thousands of tasks and repeated refreshes do not cause excessive memory growth, unresponsive scrolling, or crashes.

- [x] Partial native scale test —2032 renders1000 synthetic Backlog tasks from a GET-only response fixture.2033–2034 scrolling responds;2035 item0997 details opens and2036 closes.2037 Todo/Backlog interaction returns to board; a transitional UI dump failed, so intermediate column state is not asserted.2038–2039 Refresh and2040–2042 scrolling reach `Load more990`.2043 Load more appends item0990 while preserving the current scroll location, verified by a fresh capture after a transient dump failure.
- [x] Bounded memory observations —baseline2031 PSS93227KB; large-board2036 PSS138130KB; after-refresh2040 PSS119426KB. Recorded view count437 reflects paginated rendering. These snapshots do not establish absence of a long-term leak or full1000-card expansion behavior.
- [x] Fix if needed —no app defect observed in this limited scale/refresh/detail/pagination run; response fixtures do not mutate stored boards.
- [ ] Remaining —long soak/repeated refresh memory trend, expanding all1000 tasks, larger datasets and deeply nested/mixed states. No claim of exhaustive performance coverage.
- Evidence:2032–2043 PNG/XML and `TEST-ANDROID19-large-1000-1005.json`.
- Result: IN PROGRESS.

### ANDROID-20

Server compatibility: Older/missing fields and newly introduced status values render safely without enabling invalid actions.

- [x] Older-field response fixture —1907 native Refresh renders six legacy work items with default badges/counts and no crash.1908 details opens with default Story/Backlog/P2 and no nested work.1909 Actions shows Break down work, Discuss, Move to Todo, Delete Backlog task;1910 Cancel returns to board. No action was executed. The GET-only fixture omits orchestrationVersion/config/counts/approval/phase and other optional fields; this is not an actual older-server execution test.
- [x] Unknown-state response fixture —1912/1916 renders unknown task status warning and Todo6;1917 details retains the exact `future_task_state_v99` warning.1918 Actions contains only Discuss. However, unknown board state leaves Pause enabled/clickable, and unknown hierarchy is incorrectly represented as Story/`story`: ISSUE-044.1919–1920 further exposes mutating actions through Discuss’s Action picker;1921–1922 cancel without Send.
- [x] Fixture cleanup —proxy rules cleared; saved persisted board1922 exactly equals1897 baseline.
- [x] Initial native v94 retest —2006–2023 confirms legacy display and original unknown-state controls/detail branches.
- [x] Follow-up bulk defect/fix/retest —2026–2027 unknown task kind with known Backlog status exposed bulk Todo/Remove;2029 individual details was read-only. v95 fixes the bulk path:2051–2053 all unknown kinds have no selection/bulk actions and read-only details;2055–2056 mixed fixture selects only three supported aliases;2057–2058 changing to all unknown kinds clears selection/actions. ISSUE-044 fixed for reproduced paths; no mutation submitted against fixtures.
- [x] Cleanup —proxy cleared2059; persisted F in `android20-restored-2058.json` exactly matches1984 baseline.
- [x] Missing-configuration native dialogs —2084–2086 Quality gates renders/scrolls default Max fixes3, Feature2, Final4, QA2, timeout120, context12000 and QA policy.2088 Board strategy shows Complete App/Manual, default models/reasoning, Continuous session, Read only Git, no fallback.2090 Automatic retry shows delay10/attempts3;2092 Tools shows Use CLI defaults. Dialogs dismissed without Save; no configuration mutation submitted.
- [x] Native stale-dialog guard —2096–2102 uses nonexistent `TEST-compat-stale` task IDs in GET-only fixtures.2098 opens Discuss for a supported task,2099 enters draft and2100 dismisses IME. Polling then delivers an unsupported-status response while composer remains open.2101 Send is rejected inline with “This board or task is no longer supported for changes. Close and refresh the board” and preserves the draft.2102 Cancel was intercepted by the inline-error popup;2103–2105 still show the composer.2106 Android Back actually dismisses it. `android20-stale-dialog-events-2101.json` confirms unknown-state fixture delivery and zero board-write requests in the captured window.
- Scope —PASS covers observed missing optional/configuration fields, new board/status/level/kind values, compatible alias selection, and stale-dialog mutation guard using controlled GET fixtures. It does not certify every historical/future server version or server execution behavior. Missing optional-field defaults were inspected and dismissed; they were not saved against an old server.
- Evidence:1907–1912,1916–1922,2006–2029 and2051–2059 PNG/XML; `TEST-ANDROID20-older-missing-fields-1005.json`, `TEST-ANDROID20-unknown-statuses-1005.json`, and fixture notes in artifacts/kanban-manual-20261001.
- Result: PASS for observed native missing-field/unknown-state compatibility and stale-dialog guard branches.

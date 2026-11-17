1. Opening the board and project isolation


    OPEN-01 — Project shortcut: Open Board from a project card; load that project’s board.

    OPEN-02 — Board tab: Open through app navigation; retain the correct selected project.

    OPEN-03 — No existing board: Show the empty state and Create board action.

    OPEN-04 — Existing board: Restore its tasks, status, configuration, and available controls.

    OPEN-05 — Missing project: Handle deleted, inaccessible, or invalid project directories with a useful error.

    OPEN-06 — Rapid project switching: Switch A → B while A is loading; A’s response must not replace B’s board.

    OPEN-07 — Project isolation: Selected cards, expanded groups, task actions, and dialogs must not affect another project.

    OPEN-08 — Loading failure: Initial failure and retry must not leave a permanent spinner, blank screen, or misleading empty board.


2. Board creation


    CREATE-01 — Valid prompt: Create with a normal prompt; show the resulting board and planning state.

    CREATE-02 — Blank prompt: Empty or whitespace-only input is rejected without submitting.

    CREATE-03 — Complex prompt: Preserve multiline text, Unicode, emoji, quotes, and long descriptions.

    CREATE-04 — Provider selection: Test each exposed CLI agent: Claude, Codex, and Gemini.

    CREATE-05 — Model selection: Test default, selected, custom, unavailable, and incompatible models; errors must be actionable.

    CREATE-06 — Provider switching: Change provider repeatedly; each provider retains its own model choices.

    CREATE-07 — Profiles: Create with Minimal, Complete App, and Product-Ready; saved settings match the selection.

    CREATE-08 — Cancel: Dismiss creation or nested configuration dialogs; verify which draft values are retained and that no board is created.

    CREATE-09 — Duplicate submission: Double-tap Create or retry after a slow response; no unintended duplicate work.

    CREATE-10 — Existing board reuse: Creating again for the same project can reuse the latest board; verify appended work and retained running/paused/scheduled state are clear.

    CREATE-11 — Interrupted creation: Disconnect or background the app after submission; reconnect and discover the actual server outcome.


3. Board columns, cards, and grouping


    VIEW-01 — Column placement: Backlog, Todo, active work, Blocked, and Done cards appear in the correct displayed columns.

    VIEW-02 — Level versus kind: Initiative/Epic/Story/Task/Subtask hierarchy remains distinct from implementation, QA, review, and other work kinds.

    VIEW-03 — Grouped work: Related implementation and validation subtasks appear under the correct feature/group.

    VIEW-04 — Explicit group IDs: Explicit grouping wins over legacy task-number naming; unrelated similarly named tasks remain separate.

    VIEW-05 — Mixed child states: A group with Todo, executing, blocked, and completed children displays an accurate overall state.

    VIEW-06 — Eligible sibling: A blocked child must not hide another child that is ready to run.

    VIEW-07 — Card details: Title, priority, status, kind, origin, progress, and nested-work counts agree with the task data.

    VIEW-08 — Long content: Long titles, unbroken text, Unicode, and multiline content remain readable without covering actions.

    VIEW-09 — Missing metadata: Missing title, unknown status/kind, or older field names render safely with meaningful fallbacks.

    VIEW-10 — Pagination: Test 0, 1, 9, 10, 11, 20, and 21 groups; Show more reveals the correct next items without duplication.

    VIEW-11 — Expand/collapse: Repeatedly expand nested work; no duplicated children, incorrect counts, or accidental actions.

    VIEW-12 — Simplified board: Internal validation records do not inflate visible feature counts or appear as unrelated user features.


4. Manual task creation


    MANUAL-01 — One task: Submit a valid manual task; it enters Todo.

    MANUAL-02 — Execution consequence: If adding Todo work starts execution, the board controls and activity immediately reflect that.

    MANUAL-03 — Multiple tasks: Add several cards in one submission; preserve each card’s fields and ordering where applicable.

    MANUAL-04 — Add/remove editor: Add another editor, remove one, and try removing the last remaining editor.

    MANUAL-05 — Required title: One blank title among valid cards prevents an invalid submission and focuses the correct field.

    MANUAL-06 — Optional details: Blank details use the intended title fallback without losing the title.

    MANUAL-07 — Rich fields: Acceptance criteria, references, and file paths preserve meaningful lines and handle blank lines.

    MANUAL-08 — Priority: Test P0–P3 and the default P2; verify persistence after refresh.

    MANUAL-09 — Dependencies: Test one/multiple IDs, supported separators, whitespace, and duplicate IDs.

    MANUAL-10 — Invalid dependencies: Unknown, self-referencing, cyclic, ancestor, and cross-board references produce understandable errors/blockers.

    MANUAL-11 — Partial/error outcome: A failed submission must refresh actual state rather than assuming no tasks were created.

    MANUAL-12 — Cancel/repeat: Cancelling creates nothing; rapid submission or retry does not silently duplicate tasks.


5. Prompt-generated tasks and hierarchy breakdown


    PLAN-01 — Generate: Valid prompt creates planning work in Backlog.

    PLAN-02 — Prompt validation: Empty/whitespace prompt is rejected; long and multiline prompts remain intact.

    PLAN-03 — Per-request settings: Breakdown provider/model/profile apply to that generation request without unexpectedly changing board configuration.

    PLAN-04 — Preview: Preview displays drafts without adding executable work.

    PLAN-05 — Accept preview: Add drafts once; closing preview alone must not add them.

    PLAN-06 — Empty/invalid generation: Empty output, malformed plans, or provider failures produce a useful failure state.

    PLAN-07 — Hierarchy levels: Breakdown Initiative, Epic, Story, and Task into valid next-level children.

    PLAN-08 — Executable leaf: Subtasks cannot be broken down or executed as planning parents incorrectly.

    PLAN-09 — Inheritance: Generated children retain applicable scope, prerequisites, and parent relationships.

    PLAN-10 — Generated dependencies: Temporary/generated references resolve to the correct persisted task IDs.

    PLAN-11 — Duplicate breakdown: Double-tapping Breakdown creates only one operation/plan.

    PLAN-12 — Surviving navigation: Leave the screen or lose the HTTP connection during breakdown; later refresh reveals its actual result.

    PLAN-13 — Failed breakdown retry: Retry releases stale operation state and does not duplicate previously created children.

    PLAN-14 — Invalid board/task state: Breakdown on cancelled or otherwise ineligible work is unavailable or rejected clearly.


6. Approval, moving tasks, and dependencies


    MOVE-01 — Backlog → Todo: Approve eligible scope; card and execution eligibility update correctly.

    MOVE-02 — Todo → Backlog: Withdraw approval as required; task does not continue as newly approved work.

    MOVE-03 — Parent approval: Approving a planning parent affects descendants according to hierarchy rules; the parent itself is not executed.

    MOVE-04 — Dependency wait: Approved work with unfinished prerequisites remains blocked.

    MOVE-05 — Dependency completion: Eligible work becomes runnable when its actual prerequisites complete.

    MOVE-06 — Missing/superseded dependency: Remains a visible blocker rather than being treated as completed.

    MOVE-07 — Parent rollup: Parent state accurately follows required child completion/failure.

    MOVE-08 — Optional work: Optional children under a completed parent do not unnecessarily reopen the parent.

    MOVE-09 — Required follow-up: Required QA/follow-up work prevents a premature Done state.

    MOVE-10 — Stale action: Task status changes while the action dialog is open; revalidate before applying.

    MOVE-11 — Invalid transitions: Running, completed, superseded, or protected tasks cannot bypass state restrictions.

    MOVE-12 — Scope revision: Moving an eligible failed/waiting subtask to Backlog preserves files, history, and prerequisites while withdrawing scope approval.


7. Selection, bulk actions, and deletion


    BULK-01 — Individual selection: Select/deselect cards; counts and action availability update immediately.

    BULK-02 — Select all/clear all: Operate on the intended eligible set, including checking behavior beyond the first displayed page.

    BULK-03 — Stale selection: Refresh moves/deletes selected tasks; invalid IDs no longer remain selected.

    BULK-04 — Bulk move: Move eligible Backlog tasks to Todo or Todo tasks to Backlog without affecting unselected tasks.

    BULK-05 — Delete confirmation: Correct task count and cascade consequences are understandable; Cancel deletes nothing.

    BULK-06 — Parent plus child: Delete a selected parent and selected descendants without redundant requests for already-cascaded children.

    BULK-07 — Child only: Delete a selected child while preserving its unselected parent.

    BULK-08 — Siblings only: Delete selected siblings without promoting deletion to their parent.

    BULK-09 — Visual grouping: A visually grouped card must not imply deletion of an unrelated or unselected structural parent.

    BULK-10 — Linked fix versus descendant: A user-created fix linked through a source reference is not automatically treated as a deletion child.

    BULK-11 — Malformed hierarchy: Missing ancestors, cycles, self-parenting, duplicate IDs, and blank IDs do not hang or suppress every deletion.

    BULK-12 — Ineligible deletion: Running/non-Backlog work cannot be deleted through a stale action.

    BULK-13 — Partial deletion failure: Refresh successful deletions and still report the original failure.

    BULK-14 — Dependency deletion: Remaining dependent tasks show missing prerequisites accurately.


8. Start, pause, abort, and scheduling


    RUN-01 — Start: Start an eligible paused board; preserve its history and approved scope.

    RUN-02 — Duplicate Start: Repeated taps do not create multiple execution loops.

    RUN-03 — Pause timing: Pause during a provider call and between tasks; show the actual pause state.

    RUN-04 — Resume: Resume paused work with the selected provider/model and applicable fallback settings.

    RUN-05 — Abort: Confirm and provide a reason; board becomes cancelled and interrupted tasks reflect the server’s resulting state.

    RUN-06 — Abort cancellation: Cancel either confirmation or reason entry; board continues unchanged.

    RUN-07 — Late response: Provider completion after pause/abort must not overwrite newer control state.

    RUN-08 — Completed board: Stale flags must not expose an invalid Pause/Start action.

    RUN-09 — Future schedule: Pick date/time; show the correct intended local time and start once due.

    RUN-10 — Date/time picker cancellation: Cancel at either picker; preserve the prior schedule.

    RUN-11 — Clear schedule: Clear a future schedule; verify the paused state and removed timestamp.

    RUN-12 — Start Now: Start a scheduled board immediately without another run starting at the old scheduled time.

    RUN-13 — Past/near-now time: Handle elapsed timestamps and minute-boundary races correctly.

    RUN-14 — Active board scheduling: Explain rejection when an active board must first be paused.

    RUN-15 — Time boundaries: Test midnight, month/year rollover, leap day, timezone changes, and daylight-saving transitions where applicable.

    RUN-16 — Restart near schedule: App/server restart before or after the due time neither loses nor duplicates execution.


9. Failures, retry, and revision


    RETRY-01 — Transient failure: Retry an eligible transport/provider failure.

    RETRY-02 — Defect versus transient: Failed quality/completion gates must not be misclassified as ordinary transient failures.

    RETRY-03 — Dependency waiter: Waiting for dependencies must not increase transient-retry counts.

    RETRY-04 — Parent versus leaf: Rolled-up parent failures are not executed as retryable subtasks.

    RETRY-05 — Approval preserved: Retry retains valid approval without approving unrelated Backlog scope.

    RETRY-06 — Other gates preserved: Retry cannot bypass dependencies, side-effect approval, or planning blockers.

    RETRY-07 — Bulk retry: Only currently eligible tasks are submitted, even if eligibility changed while the dialog was open.

    RETRY-08 — Approved fix: “Retry with approved fix” uses a valid executable fix linked to the failed subtask.

    RETRY-09 — Invalid fix: Missing, unrelated, unapproved, or non-executable fix cannot authorize retry.

    RETRY-10 — Automatic retry: Enable/disable, delay, maximum attempts, and counter reset behave correctly.

    RETRY-11 — Numeric boundaries: Blank, zero, negative pasted values, nonnumeric input, overflow, and very large retry values produce explicit/default behavior.

    RETRY-12 — Attempt history: Retry requests alone do not falsely increment execution-attempt labels.

    RETRY-13 — Fallback: Provider failure follows configured fallback behavior; invalid/unavailable fallback errors remain visible.

    RETRY-14 — Recovery: Restart during execution distinguishes an interrupted worker from one still alive and prevents duplicate execution.


10. Discussions and proposals


    DISCUSS-01 — Plain discussion: Submit a nonblank message; preserve history and display the correct task context.

    DISCUSS-02 — Blank discussion: Empty plain discussion is rejected.

    DISCUSS-03 — Action switching: Switching actions retains text appropriately and submits the selected action’s payload.

    DISCUSS-04 — Edit scope: Proposed edits preserve unrelated scope.

    DISCUSS-05 — Replace scope: Replacement scope clearly shows what will be removed/replaced.

    DISCUSS-06 — Reprioritize: Valid priorities apply through review; invalid/blank values are handled clearly.

    DISCUSS-07 — Delete proposal: Review and apply deletion to the intended task and descendants.

    DISCUSS-08 — Split: Test one/many lines, blanks, duplicate titles, and a large list of proposed items.

    DISCUSS-09 — Merge: Test valid target, missing target, self-merge, incompatible target, and cross-board target.

    DISCUSS-10 — Regenerate children: Preserve/protect relevant existing child state and recorded effects.

    DISCUSS-11 — Re-research: Create the appropriate research workflow without silently starting implementation.

    DISCUSS-12 — Revision/fix/replacement: Each creates the correct linked work; replacement supersession is represented accurately.

    DISCUSS-13 — Required action inputs: Non-discussion actions with blank details/target/items must surface server validation errors.

    DISCUSS-14 — Proposal review: Show summary, before/after diff, warnings, errors, and status.

    DISCUSS-15 — Apply/reject: Affect only the selected pending proposal; Cancel performs neither.

    DISCUSS-16 — Already resolved proposal: Applied/rejected/failed proposals cannot be reapplied through stale UI.

    DISCUSS-17 — Stale scope: Task or descendants change after generation; outdated structural proposals are rejected safely.

    DISCUSS-18 — Concurrent decisions: Apply on one device and Reject on another; Android reconciles the authoritative result.

    DISCUSS-19 — During execution: Discussion must not overwrite the executing task’s provider/session state.

    DISCUSS-20 — Interrupted generation: Disconnect/background during generation; recover the eventual proposal or failure without duplicate submission.


11. Side effects, research, and recorded changes


    SAFE-01 — Declaration: Save one/multiple external side-effect descriptions; reject blank declarations.

    SAFE-02 — Declaration versus approval: Saving a declaration alone does not approve execution.

    SAFE-03 — Approval: Explicit approval applies to the intended current scope/effects.

    SAFE-04 — Changed declaration: Changed scope/effects cannot silently reuse obsolete approval.

    SAFE-05 — Existing declarations: Reopening/editing must not accidentally discard previously recorded effects.

    SAFE-06 — Research acceptance: Display proposed planning items and optional note.

    SAFE-07 — Accepted research: Creates Backlog-only planning stories rather than immediately executing implementation.

    SAFE-08 — No research items: Record acceptance without inventing planning cards.

    SAFE-09 — Repeated acceptance: Duplicate taps/concurrent acceptance do not create duplicate planning items.

    SAFE-10 — Detach child: Detach an eligible user-created child into a Backlog story while preserving its content.

    SAFE-11 — Resolve recorded effects: Show affected descendants accurately; exclude unrelated siblings and handle cyclic data safely.

    SAFE-12 — Keep/revert/cleanup: Each sends the correct decision; Keep does not claim to undo changes, and revert/cleanup create the appropriate work.

    SAFE-13 — Cancel/stale effects: Cancelling resolution causes no mutation; newly recorded effects require current review.


12. Configuration and policies


    CONFIG-01 — Current/fallback provider: Update provider/model choices and verify displayed versus effective settings.

    CONFIG-02 — Stage models: Configure Breakdown, Implementation, QA, QA Fix, AGENTS, and Final QA independently.

    CONFIG-03 — Main-model change: Verify any cleared/reassigned stage overrides match the intended behavior and are visible.

    CONFIG-04 — Strategy: Manual, Cheap, Hybrid, and Expensive route work according to configuration.

    CONFIG-05 — Provider-specific preferences: Switching providers and restarting does not leak another provider’s model choices.

    CONFIG-06 — Reasoning/session policy: Default/Low/Medium/High and continuous/per-task settings persist correctly.

    CONFIG-07 — Git policy: Read-only versus managed policy is reflected in execution and evidence.

    CONFIG-08 — Tool access: CLI defaults, shell enabled, and shell disabled persist and affect execution correctly.

    CONFIG-09 — TDD: Test enabled/disabled, failing-baseline requirement, allowing no generated tests, and fix-attempt limits.

    CONFIG-10 — Validation: Feature/final/QA commands, limits, and timeout values persist without corrupting multiline commands.

    CONFIG-11 — Retrieval: RAG enabled/disabled, scopes, and context-character limit behave as configured.

    CONFIG-12 — QA policy: High risk, all tasks, and off; verify follow-up and attempt limits.

    CONFIG-13 — Numeric validation: Test blanks, zero, negative pasted values, decimals, text, overflow, and large values across all numeric settings.

    CONFIG-14 — Cancel semantics: Back/Cancel from nested settings must not unexpectedly apply draft changes.

    CONFIG-15 — Partial save failure: Some settings use multiple requests; if a later request fails, accurately show which changes persisted.

    CONFIG-16 — Unrelated settings: Updating one policy preserves the other settings.

    CONFIG-17 — Concurrent update: Changes from another client while a settings dialog is open do not produce an unexplained stale overwrite.


13. Details, evidence, chat, and transcripts


    DETAIL-01 — Task details: Correct title, description, hierarchy breadcrumb, priority, status, dependencies, and origin.

    DETAIL-02 — Group details: Correct nested tasks, completion counts, and available actions.

    DETAIL-03 — Current activity: Display actual executing subtasks rather than parent status rollups.

    DETAIL-04 — Parallel activity: Show all executing work once; stale current-task IDs do not create phantom activity.

    DETAIL-05 — Planning activity: Clearly identify provider planning/research activity when no implementation subtask is running.

    DETAIL-06 — Attention panel: Counts/actions exclude ordinary dependency waiters where appropriate.

    DETAIL-07 — Attempt details: Correct stored execution numbers, errors, usage, and evidence for each attempt.

    DETAIL-08 — Validation history: A later pass does not erase earlier failures or imply every command passed.

    DETAIL-09 — Expected failing baseline: Accepted expected-red results are distinguished from both defects and passing tests.

    DETAIL-10 — Missing evidence: Unknown results are not displayed as passed or borrowed from a previous task/attempt.

    DETAIL-11 — Manual verification: Show applicable environment and evidence; a success label alone must not imply verified completion.

    DETAIL-12 — Chat link: Opens the correct linked provider session; absent/deleted sessions fail gracefully.

    DETAIL-13 — Transcript: Correct task/session entries, ordering, roles, and complete readable content.

    DETAIL-14 — Large evidence: Long JSON, command output, diffs, file lists, and transcripts remain scrollable/selectable.

    DETAIL-15 — Malformed evidence: Missing/null/legacy fields do not crash the details dialog.


14. Android lifecycle, refresh, touch, and accessibility


    ANDROID-01 — Polling lifecycle: Enter Board repeatedly; only one refresh loop runs.

    ANDROID-02 — Leave/disconnect: Leaving the tab or disconnecting stops irrelevant refresh/re-render activity.

    ANDROID-03 — In-flight refresh: Switch project/board during refresh; stale data cannot replace the current screen.

    ANDROID-04 — Action versus polling: Refresh pauses during actions and resumes after both success and failure.

    ANDROID-05 — Unchanged data: Polling unchanged state does not flicker or repeatedly rebuild the UI.

    ANDROID-06 — Scroll preservation: Same-project, same-board, same-column refresh preserves scroll position.

    ANDROID-07 — Scroll isolation: Different projects, boards, and columns do not inherit unrelated offsets.

    ANDROID-08 — Shrinking list: Deleting/moving cards while scrolled near the bottom does not leave blank inaccessible space.

    ANDROID-09 — Background/resume: Restore authoritative board state without duplicate actions or polling loops.

    ANDROID-10 — Rotation/recreation: Test with forms, confirmations, details, and proposals open; no crash, duplicate submission, or wrong target.

    ANDROID-11 — Process death: Restart after Android kills the app; reconcile submitted operations with server state.

    ANDROID-12 — Network changes: Offline, slow network, Wi-Fi/mobile switching, and reconnect show accurate state and useful recovery.

    ANDROID-13 — Ambiguous response: Server accepts an action but response is lost; retry must not silently duplicate work.

    ANDROID-14 — Touch conflicts: Scrolling, tapping cards, expanding groups, selecting cards, and pressing buttons do not trigger one another accidentally.

    ANDROID-15 — Rapid interaction: Repeated taps, rapid column changes, and opening/closing dialogs do not stack duplicate requests/dialogs.

    ANDROID-16 — Keyboard: Input fields remain visible above the keyboard; multiline entry, paste, Back, and IME actions behave correctly.

    ANDROID-17 — Small/large displays: Portrait, landscape, split-screen, display scaling, and large font sizes keep actions reachable.

    ANDROID-18 — Accessibility: TalkBack labels, focus order, selected state, status meaning, and dialog focus are understandable.

    ANDROID-19 — Long-running board: Hundreds/thousands of tasks and repeated refreshes do not cause excessive memory growth, unresponsive scrolling, or crashes.

    ANDROID-20 — Server compatibility: Older/missing fields and newly introduced status values render safely without enabling invalid actions.

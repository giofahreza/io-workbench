# Kanban Design Notes

This document summarizes the Kanban flow we discussed.

The main idea is:

> Backlog is for planning and approval. Execution happens only in subtasks.

## Core Concepts

Every board item should have three separate meanings:

- `level`: where the item sits in the hierarchy.
- `kind`: what type of work it represents.
- `status`: where it is in the workflow.

These should not be mixed together.

Every item should also have a normal `description`. The description explains
the purpose of the item at its own hierarchy level; it is not a hidden
requirement record.

Example:

```text
level = subtask
kind = implementation
status = todo
```

## Levels

The board should support this hierarchy:

```text
Initiative -> Epic -> Story -> Task -> Subtask
```

### Initiative

An initiative is a broad goal or business direction.

Example:

```text
Increase unique selling point to increase sales
```

An initiative may need research, comparison, product thinking, or multiple epics before implementation starts.

### Epic

An epic is a large product capability or strategic feature group.

Example:

```text
Monthly budget controls
```

An epic explains the big picture, but it should not execute directly.

### Story

A story describes user-facing behavior from the user's point of view.

Example:

```text
User can set monthly category budgets
```

A story should describe what the user can do and what should be true from the user's perspective.

### Task

A task describes the system or technical delivery plan for a story.

Example:

```text
Persist category budget limits
```

A task can have the same title as a story or subtask if the request is small, but its description should explain the system/design scope.

### Subtask

A subtask is the only executable work item.

Subtasks are concrete, single-purpose work units.

Example:

```text
Add endpoint PATCH /budget/{month}
```

The engine should never execute initiative, epic, story, or task directly. It should only execute subtasks.

Subtask titles should sound like engineering execution tickets.

Good subtask titles:

```text
Add endpoint PATCH /budget/{month}
Add Room entity BudgetLimit
Add repository method updateMonthlyBudget
Add Compose input field BudgetAmountTextField
Add unit tests for updateMonthlyBudget validation
Run Android emulator smoke test for budget edit flow
```

Bad subtask titles:

```text
User can edit monthly budget
Improve budget feature
Handle budget correctly
Add budget entity and repository tests
Implement and verify budget editing
```

If a developer cannot tell exactly what to do from the subtask title, the title is too vague.

If a subtask title contains `and`, it is probably doing more than one thing and should be split.

## Statuses

Statuses should stay Kanban-only:

```text
backlog
todo
in_progress
blocked
failed
done
```

Status should not be used for task kind.

For example, `qa`, `manual_test`, `review`, and `implementation` are not statuses. They are kinds.

## Kinds

Kinds explain what type of work a subtask does.

Recommended kinds:

```text
research
design
implementation
test_implementation
qa
manual_test
review
fix
migration
revert
cleanup
```

Each executable subtask should have exactly one kind.

Do not combine multiple kinds in one subtask.

Bad:

```text
Add budget entity and repository tests
```

Better:

```text
Add Room entity BudgetLimit
Add repository method createBudgetLimit
Add unit tests for createBudgetLimit
Run budget repository test suite
Run Android emulator smoke test for budget persistence
```

## Kind Rules

### Implementation

`implementation` can edit production code.

Example:

```text
Add Room entity BudgetLimit
```

### Test Implementation

`test_implementation` can edit test files.

Example:

```text
Add budget repository unit tests
```

### QA

`qa` validates the result by running checks, tests, or reviewing behavior.

It should not edit production files.

Example:

```text
Run budget repository tests
```

### Manual Test

`manual_test` validates the app manually, such as using an Android emulator.

It should not edit source files.

Example:

```text
Smoke verify budget alert on Android emulator
```

### Review

`review` inspects code, design, behavior, or risk.

It should not edit source files unless the user explicitly approves a fix.

### Fix

`fix` changes code to solve a specific failed QA, failed manual test, or defect.

Fix work should usually be created under the same parent task as the failed subtask.

### Revert

`revert` undoes code changes from a previous executable subtask.

It should be explicit and user-approved.

Example:

```text
Revert endpoint PATCH /budget/{month}
```

### Cleanup

`cleanup` repairs project state after cancelled, superseded, or partially completed work.

Example:

```text
Remove unused BudgetLimit migration files
```

## Prompt Size Should Be Adaptive

The same board should handle small, normal, and large prompts.

### Very Small Prompt

Example:

```text
Add card summary user in user page
```

This may create:

```text
Story -> Task -> Subtask
```

Even a tiny prompt should have a story wrapper in the visible Backlog.

The story, task, and subtask may have the same core title, but each level has a different purpose:

```text
Story: User can view card summary on user page
Task: Add card summary user in user page
Subtask: Add UserSummaryCard to user page
```

### Small Feature Prompt

Example:

```text
Add feature repayment
```

This may create:

```text
Story -> Task -> Subtasks
```

### Normal Product Feature Prompt

Example:

```text
User can set monthly category budgets
```

This may create:

```text
Story -> Tasks -> Subtasks
```

### Large Strategic Prompt

Example:

```text
Increase unique selling point to increase sales
```

This may create:

```text
Initiative -> Research tasks -> Epics -> Stories -> Tasks -> Subtasks
```

Implementation should wait until research and planning are clear enough.

## Breakdown Flow

There should be multiple breakdown levels:

```text
User prompt -> Initiative/Epic/Story -> Task -> Subtask
```

The board should not assume every prompt starts at the same level.

The model should choose the right level based on prompt size and ambiguity.

But the visible Backlog should never start below story level.

Breakdown happens one level at a time:

```text
board prompt
-> initiative, epic, or story
-> epic or story children
-> task children
-> subtask children
```

The exact transition depends on the selected item's level:

```text
initiative -> epics
epic       -> stories
story      -> tasks
task       -> subtasks
```

The board-level breakdown must not jump directly to executable work in the
visible Backlog. A generated task or subtask is nested under its parent, or is
wrapped in the required story hierarchy before it is shown.

Moving a planning item to Todo approves its current scope. It does not execute
the initiative, epic, story, or task. The engine may perform the next planning
breakdown needed to produce subtasks, then the scheduler runs only the
approved executable subtasks.

## Backlog Behavior

Backlog list can show these planning levels:

- initiative
- epic
- story

The lowest visible ticket kind in the Backlog list is story.

Task and subtask should never appear as top-level Backlog cards.

Tasks and subtasks live inside parent detail views.

They can have their own status, priority, transcript, evidence, and execution state, but they should not clutter the main Backlog list.

If an initiative generates epics or stories, those generated children should be added to Backlog first.

If a story generates tasks or subtasks, they should be nested under the story.

The user should decide what to approve.

Nice-to-have work should also go to Backlog.

It should not be treated as an optional child that silently runs under an approved parent.

Example:

```text
Approved story: User can set monthly category budgets
Nice-to-have idea: Add chart for budget usage trend
```

The chart idea should become a Backlog story, epic, or initiative.

It starts only when the user moves that item to Todo.

## Moving Items To Todo

Moving an item to Todo means:

```text
The user approves this item's current scope.
```

But execution still happens only in subtasks.

If the Todo item is not a subtask, the engine should first ensure executable subtasks exist under it.

Todo should not contain stale or dormant work.

Everything in Todo is approved work and can be picked by the scheduler.

If an item should not run, it should be in Backlog or Blocked, not Todo.

The normal executable transition is:

```text
backlog -> todo -> in_progress -> done
                         |         |
                         v         v
                      blocked     failed
                         |
                         v
                        todo
```

`blocked` never jumps directly to `in_progress`. `failed` requires either a
transient retry or a new approved fix plan before it runs again.

Example:

```text
Story moved to Todo
-> create or reuse tasks
-> create or reuse subtasks
-> execute approved subtasks
```

## Approval Boundary

The engine must know the difference between in-scope and out-of-scope work.

### In-Scope Work

If the engine discovers extra work that is clearly required by the approved Todo scope, it can create child or sibling subtasks under the same approved parent.

Example:

```text
Approved task: Persist category budget limits
Discovered work: Add migration for budget table
```

This can become an in-scope subtask.

### Out-of-Scope Work

If the engine discovers work that changes product behavior, adds a new feature, is nice-to-have, or requires a user decision, it should not auto-run it.

It should create a proposal in Backlog or Discussion.

Example:

```text
Approved task: Persist category budget limits
Discovered idea: Add notification when budget is exceeded
```

This should be a new Backlog proposal, not automatic implementation.

## Partial Implementation

Partial implementation should not automatically create random top-level follow-up tasks.

If the remaining work is inside the approved scope, create or continue child subtasks under the same parent.

If the remaining work is outside the approved scope, create a Backlog proposal and block the current item if needed.

Todo is an approval boundary, so incomplete work inside the approved scope is
not a reason to create an unrelated top-level follow-up. The engine should
continue the existing plan, create a missing subtask under the same approved
parent, or create a `fix` subtask when a defect is found. A new feature or
changed product behavior must return to Backlog for user approval.

## No Requirements Layer

Requirements are not a board concept and must be removed completely.

The system must not keep a requirement matrix, requirement extraction stage,
requirement IDs, requirement screens, or a hidden requirement model as a
second source of truth. This prevents the ticket hierarchy and a separate
requirements hierarchy from disagreeing.

The planning contract is made from the ticket fields themselves:

```text
description
acceptanceCriteria
parentId
blockedBy
priority
scopeVersion
```

Acceptance criteria belong to the relevant story, task, or subtask. When a
parent changes, its description and acceptance criteria are the scope that
must be re-planned.

## Acceptance Criteria

Each level should describe acceptance criteria at the right level.

Use item descriptions, acceptance criteria, and evidence as the complete planning and validation contract.

### Story Criteria

Story criteria describe user behavior.

Example:

```text
User can enter a monthly budget for a category.
User can see when spending exceeds the budget.
```

### Task Criteria

Task criteria describe system behavior or design scope.

Example:

```text
Budget limits are persisted locally.
Budget totals are calculated from existing transaction data.
```

### Subtask Criteria

Subtask criteria describe executable verification.

Example:

```text
The endpoint PATCH /budget/{month} accepts valid budget updates.
The endpoint rejects negative budget amounts.
The endpoint returns the updated monthly budget payload.
```

## Dependencies And Priority

Priority and blocking are different.

Priority answers:

```text
If multiple items can run, which one should run first?
```

Blocking answers:

```text
Can this item run at all?
```

Recommended fields:

```text
priority: p0 | p1 | p2 | p3
rank: number
blockedBy: item ids
```

Required order should use `blockedBy`, not priority.

Example:

```text
Subtask A: Implement budget entity
Status: todo

Subtask B: Implement budget repository
Status: blocked
Blocked by: Subtask A

Subtask C: Add repository tests
Status: blocked
Blocked by: Subtask B
```

When Subtask A is done, Subtask B should move:

```text
blocked -> todo
```

Then the scheduler can choose it if it is the highest priority eligible work.

Do not jump directly:

```text
blocked -> in_progress
```

Use:

```text
blocked -> todo -> in_progress
```

## Priority Inheritance

Children should inherit priority from their parent by default.

Example:

```text
Story priority = p1
Task priority = inherit p1
Subtask priority = inherit p1
```

The user can override child priority if needed.

Dependency still wins over priority.

If a `p0` subtask is blocked, it cannot run until its blockers are done.

## Parent Status Rollup

Parent status should be computed from children.

Rules:

- parent is `done` when all required children are done
- parent is `in_progress` when any child is running or eligible
- parent is `blocked` only when all remaining required paths are blocked
- parent should not be `done` if required subtasks are unfinished
- Backlog nice-to-have items do not block the parent from becoming `done`

If one child is blocked but another child can still run, the parent can stay `in_progress`.

## Discussion Flow

Every item should support `Discuss`.

Discussion is where the user can ask about the plan before approving or changing it.

Inside discussion, the user should be able to:

- edit
- replace
- delete
- split
- merge
- regenerate children
- reprioritize
- re-research

The AI should propose a diff.

The user applies or rejects the diff.

Edit actions are only allowed while the item, or its top-level parent, is in Backlog.

If an item is already approved or running, the user must move it back to Backlog before editing scope.

Moving an item back to Backlog clears approval.

## Breakdown As A Planning Action

`Breakdown` should be a planning action, not implementation.

Breakdown can be used when the user still does not understand what will be implemented.

Breakdown should create visible planning items in Backlog by default.

Visible Backlog items must be initiative, epic, or story.

Task and subtask children should be nested under their parent detail, not shown as Backlog cards.

If the parent is already approved in Todo, generated in-scope subtasks can be created under that approved parent.

Risky or out-of-scope children should stay in Backlog for approval.

If risky or out-of-scope work is below story level, wrap it in a story before showing it in Backlog.

## Done Items And Re-Research

If an initiative, epic, story, or task is already done but later seems wrong, the system should not silently reopen or rewrite it.

Better options:

- create a new `research` item linked to the done item
- create a `revision` item
- create a `fix` item
- mark the old item as `superseded` only if the user approves

This keeps history understandable.

The same rule applies to a completed initiative, epic, story, or task that
needs fresh research. Create a linked research or revision item in Backlog;
do not reopen the completed item silently.

## Transcripts

Transcripts should be hidden by default.

Each item can show a `View transcript` action.

Clicking it should open a modal that shows the complete transcript like a chat session.

Parent and child transcripts should not be mixed by default.

Recommended behavior:

- parent transcript shows planning and discussion for that parent
- subtask transcript shows execution for that subtask
- parent can link to child transcripts
- parent can optionally show an aggregate view, but not by default

## Task Transcript

A task can have its own transcript for planning, breakdown, discussion, and approval.

The executable transcript belongs to the subtask.

So a task should not automatically include all subtask execution logs as if they were the task's own transcript.

## UI Notes

Cards should clearly show hierarchy so same-title items are not confusing.

Example:

```text
Story > Task > Subtask
```

Cards can show:

- level
- kind
- priority
- blocked state
- parent breadcrumb

Cards should not show too much execution detail by default.

Details and transcript modals can show deeper information.

## Important Corner Cases

### Parent Moved To Todo

When a parent item moves to Todo, decide what is approved:

- in-scope generated subtasks can be approved under that parent
- risky, nice-to-have, or out-of-scope work should stay in Backlog as story, epic, or initiative

### No Dormant Todo

Todo means approved work that is allowed to run.

There should be no old Todo that is not meant to execute.

If the user does not want an item to run, it should be moved back to Backlog or marked Blocked with a clear blocker.

When the board starts, the scheduler can pick any Todo subtask based on dependencies, priority, and rank.

### Duplicate Breakdown

If an item already has children, the engine should not generate duplicate children.

It should reuse existing children and only create missing subtasks.

### Dependency Cycle

The system must detect dependency cycles.

Bad:

```text
Subtask A blocked by Subtask B
Subtask B blocked by Subtask A
```

If this happens, the affected items should be blocked as a planning error.

The AI should propose a dependency fix instead of trying to execute.

### Deleted Blocker

If an item listed in `blockedBy` is deleted, rejected, or superseded, dependent items should not silently unblock.

They should stay blocked with a clear reason:

```text
missing dependency
```

Then the user can discuss the dependent item and either replace the dependency or remove the blocker.

### Nice-To-Have Work

Nice-to-have work belongs in Backlog.

It should be linked to the parent item, but it should not count as required work for parent completion.

It should only start when the user explicitly moves it to Todo.

### QA Failure

Failed QA should create `fix` subtasks under the same parent task.

It should not create unrelated top-level follow-ups.

### Cross-Parent Dependency

A subtask under one story may depend on a subtask under another story.

`blockedBy` should support links across hierarchy, not only same parent.

### Editing Locked Items

Scope edits should not happen during execution.

Only Backlog items are editable.

Locked statuses:

```text
todo
in_progress
blocked
failed
done
```

If the user wants to change scope for a locked item, the item must be moved back to Backlog first.

If work is running, the engine should pause or abort affected execution before the item moves back to Backlog.

Moving an item back to Backlog means:

```text
approval is cleared
old execution plan is no longer trusted
old generated children are deleted
```

After the parent is edited in Backlog, the system should generate fresh child tickets from the new parent scope.

This avoids stale children that no longer match the approved plan.

If deleted children already changed code or produced external side effects, deleting the ticket does not automatically undo that work.

The system must show the user what happened and ask what to do:

```text
keep changes
create revert subtask
create cleanup subtask
```

The system should not silently revert files.

### Undo And Rollback

Undo should be explicit work.

The system should not silently roll back files or external state.

If the user wants to undo completed or partial work, create a new executable subtask:

```text
kind = revert
kind = cleanup
kind = fix
```

Example:

```text
Revert endpoint PATCH /budget/{month}
```

### Done Item Changes

Done items should not be edited directly.

If a done item is wrong or needs a new direction, create a linked item instead:

```text
research
revision
fix
replacement
```

If the user wants the old done item replaced, mark it as superseded only after user approval.

### User-Created Children

If a parent changes in Backlog, generated children from the previous scope are deleted.

If the user manually created a child and wants to keep it, the user should detach or move it before changing the parent.

Otherwise, children under the changed parent are treated as part of the old scope and removed with the old plan.

### Acceptance Criteria Drift

Implementation should not silently change story or task acceptance criteria.

If behavior needs to change, propose a criteria diff for user approval.

### Acceptance Criteria Conflict

Before generating executable subtasks, the system should check that child acceptance criteria do not contradict parent acceptance criteria.

If story, task, and subtask criteria disagree, planning should block.

The system should show the conflict and ask the user which behavior is correct.

Example:

```text
Story says budget amount can be zero.
Task says budget amount must be positive.
```

This should be resolved before implementation.

### Tiny Prompt Overkill

Do not force initiative or epic for every small request.

Small request can be:

```text
Story -> Task -> Subtask
```

The story can be small and simple.

This keeps the visible Backlog consistent because task and subtask never appear as top-level Backlog cards.

### Top-Level Task Or Subtask Generated

If the model generates a top-level task or subtask, the system should wrap it before showing it in the Backlog list.

Example:

```text
Generated task: Add UserSummaryCard to user page
Backlog story: User can view card summary on user page
Nested task: Add card summary user in user page
Nested subtask: Add UserSummaryCard to user page
```

This keeps the lowest visible Backlog ticket at story level.

### Large Prompt Research

For broad business prompts, first executable subtasks may be research subtasks.

Implementation should start only after the research output is accepted or converted into clearer planning items.

### Retry Attempts

Retrying a subtask should not overwrite old evidence or transcript.

Each retry should create a new attempt record.

Recommended attempt fields:

```text
attemptId
startedAt
finishedAt
status
transcript
commands
filesChanged
evidence
```

The task detail can show the latest attempt by default and allow viewing older attempts.

### Retry Versus Fix

Retry should be used for transient or environment failures.

Examples:

```text
network timeout
provider error
emulator not ready
test infrastructure failure
```

If the failure is caused by a real implementation defect, create a `fix` subtask instead of retrying the same failed work.

Examples:

```text
test assertion failed because behavior is wrong
manual test found broken UI flow
review found unsafe logic
```

### Priority Changes While Running

If the user changes priority while a subtask is running, the current subtask should continue.

Priority changes apply to the next scheduler decision.

If the user wants the current work to stop immediately, they should use Pause or Abort.

### Parallel Execution Conflicts

Two subtasks may conflict if they edit the same files or related code paths.

Safe default:

```text
Only run one executable subtask per project at a time.
```

If parallel execution is added later, the scheduler should avoid running subtasks together when their planned files overlap.

### Research Output Approval

Research subtasks can produce recommendations, comparisons, or proposed product direction.

Research output should not automatically become implementation.

Flow:

```text
Research done
-> proposed initiative/epic/story/task diff
-> user approves
-> new approved work can move to Todo
```

### Dirty Worktree Ownership

The engine must know which changes existed before a subtask started.

Before each executable subtask, capture a workspace snapshot.

After execution, compare the result and classify changes:

```text
pre_existing_change
changed_by_subtask
unknown_change
```

The engine should not claim user changes as its own evidence.

### External Side Effects

Some subtasks can change state outside the workspace.

Examples:

```text
database migration
remote API configuration
cloud resource
local emulator data
third-party account setting
```

Before running a risky subtask, the system should declare possible external side effects.

After running, evidence should state what external state changed.

Workspace snapshots are not enough for this case.

### Evidence Before Done

A subtask should not become `done` only because the model says it is done.

Done requires evidence that matches the subtask kind.

Examples:

```text
implementation -> changed production files
test_implementation -> changed test files
qa -> command results
manual_test -> manual steps and result
review -> review findings
fix -> changed files plus evidence that failure is resolved
revert -> reverted files or cleanup evidence
```

### One Purpose, Not One File

One subtask means one engineering purpose.

It does not mean one file.

Example:

```text
Add endpoint PATCH /budget/{month}
```

This may touch route, controller, service, and validation if the codebase structure requires it.

The subtask is still valid because it has one purpose.

However, implementation and test-writing should still be separate kinds.

Example:

```text
Add endpoint PATCH /budget/{month}
Add unit tests for PATCH /budget/{month}
```

### Architecture-Aware Subtask Titles

Subtask titles should match the actual project architecture.

Bad:

```text
Add endpoint PATCH /budget/{month}
```

if the project is local-only Android and has no backend.

Before generating engineering subtasks, the system should inspect enough codebase structure to avoid fake endpoints, wrong frameworks, or wrong file names.

### Transcript Redaction

Transcripts may contain sensitive logs, tokens, environment values, or copied command output.

Before storing or showing transcript text, the system should redact obvious secrets.

Examples:

```text
API keys
Bearer tokens
password values
private env vars
```

### Deleted Tickets

If the user deletes a planned item, it should disappear from the active board.

If the deleted item had generated children, those child tickets should be deleted too.

The board should not keep deleted child tickets as hidden executable work.

Deleting a required child does not make its parent done. The parent remains
incomplete or blocked with a missing-plan reason until the user regenerates,
replaces, or explicitly removes that scope.

Deleting a child is also different from deleting its parent:

- deleting a parent deletes its generated descendants
- deleting a child leaves the parent and sibling history intact
- deleting a child with recorded code or external side effects does not undo
  those effects; create explicit revert or cleanup work when needed

## Provider Routing

Provider selection is part of the board configuration, not a task status.

The default routing for this workflow is:

```text
breakdown and planning proposals -> Codex CLI, gpt-5.5
implementation and every other execution kind -> Claude CLI, minimax-m3
```

The stored model name may remain `minimax-m3`, but the runtime must use the
configured MiniMax-compatible Claude gateway. Provider and model must be
recorded together in telemetry so a breakdown cannot accidentally reuse an
implementation session.

## Recommended Implementation Order

1. Add explicit fields:
   - `level`
   - `kind`
   - `parentId`
   - `blockedBy`
   - `priority`
   - `rank`
   - `executable`
   - `description`
   - `acceptanceCriteria`
   - `scopeVersion`
   - `attempts`
   - `plannedFiles`
   - `changedFiles`
   - `evidence`
   - `sideEffects`
2. Update breakdown prompt and schema to support hierarchy.
3. Wrap generated top-level task or subtask into a story before showing it in Backlog.
4. Filter the visible Backlog list to initiative, epic, and story only.
5. Update codebase inspection before engineering subtask generation.
6. Update scheduler so only executable subtasks run.
7. Add dependency handling with `blockedBy`, cycle detection, and missing blocker handling.
8. Add parent status rollup.
9. Add evidence gates before marking subtasks done.
10. Add retry attempt history and retry-versus-fix behavior.
11. Add external side-effect tracking.
12. Remove the requirements concept completely, including the old matrix
    model, extraction flow, and screens.
13. Add item-scoped discussion actions.
14. Delete and regenerate children when parent scope changes.
15. Hide transcripts by default and show them in a modal.
16. Add transcript redaction.
17. Update UI to show hierarchy and breadcrumbs clearly.
18. Add migration for old task data.

## Final Rule

The engine should be strict about this:

```text
Only subtasks execute.
Everything above subtask is planning, scope, approval, and rollup.
```

That keeps the Kanban global, easy to reason about, and safer for automatic execution.

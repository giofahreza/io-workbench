# Mobile Kanban Board Flow

This document describes the native Android app, not the web board.

## Why was an initial prompt required?

Previously, the mobile screen combined board initialization and the first work
item in a **Create board** dialog. The shared controller rejected an empty
prompt, and Add task stayed disabled until a board existed. That was a UI/API
coupling, not a requirement for planning or execution: each task already has its
own prompt, details, and acceptance criteria.

## New first-open behavior

- Opening a project's Board shows its normal columns, counts, and empty states.
- **Add task** works before and after the first task. There is no required
  project-wide prompt and no separate Create board step.
- The first submission initializes an empty paused board transparently, then
  uses the same task endpoint as subsequent submissions. No seed card is added.
- **Manage** is available on the empty board for optional provider, model, and
  board configuration. Saving those settings stores an empty board without
  adding work or starting execution. Otherwise, saved defaults apply on first use.
- Both manual cards and prompt-generated work enter **Backlog** for review.
  Moving reviewed work to **Todo** approves it and starts processing.
- Loading failures are not treated as empty boards: retry loading first.

### Mobile interaction flow

```mermaid
flowchart TD
    Open[Open project Board] --> Load[Read project board]
    Load --> ReadOK{Load succeeded?}
    ReadOK -->|No| Error[Show load error and Retry]
    Error --> Load
    ReadOK -->|Yes| Board[Show normal board columns and Add task]
    Board --> Manage[Optional Manage settings]
    Manage --> Save[Save configuration without a prompt or seed task]
    Save --> Board
    Board --> Add[Tap Add task]
    Add --> Choice{How to add work?}
    Choice -->|Manual| Manual[Enter one or more rich task cards]
    Choice -->|Generate from prompt| Prompt[Describe work and choose breakdown settings]
    Manual --> Submit[Submit]
    Prompt --> Submit
    Prompt --> Preview[Optional Preview]
    Preview --> ReviewDrafts[Review draft plan]
    ReviewDrafts -->|Accept drafts| Submit
    ReviewDrafts -->|Close| Prompt
    Submit --> Ensure{Stored board exists?}
    Ensure -->|No| Init[Initialize empty paused board with configured defaults]
    Init --> NormalAdd[Use normal task endpoint]
    Ensure -->|Yes| NormalAdd
    NormalAdd --> Saved{Submission succeeded?}
    Saved -->|Yes| Backlog[Refresh board and review Backlog]
    Saved -->|No| Keep[Keep dialog and draft; show error]
    Keep --> Submit
    Backlog --> Board
```

Preview also initializes the empty board when necessary because the draft API
is board-scoped. Previewing alone does not accept the generated cards.

### Initialization and task API flow

```mermaid
sequenceDiagram
    actor User
    participant Android as Native Android UI
    participant Controller as Shared mobile controller
    participant API as Rust server
    User->>Android: Open project Board
    Android->>Controller: refreshAgenticBoard
    Controller->>API: GET boards filtered by project path
    API-->>Controller: Board list or empty list
    opt Existing board
        Controller->>API: GET board detail
        API-->>Controller: Board snapshot
    end
    Controller-->>Android: Confirmed board state
    Android-->>User: Normal board and enabled Add task
    User->>Android: Add manual tasks or generate from prompt
    Android->>Controller: ensureAgenticBoard
    opt No stored board
        Controller->>API: POST boards with project and configuration, no command or prompt
        API-->>Controller: Empty board, or existing project board unchanged
        Controller->>API: Refresh board snapshot
        API-->>Controller: Board detail
    end
    alt Manual cards
        Android->>Controller: addAgenticBoardTasks
        Controller->>API: POST board tasks with status backlog
    else Prompt generation
        Android->>Controller: generateAgenticBoardTasks
        Controller->>API: POST tasks/backlog-from-prompt
    else Accepted preview
        Android->>Controller: acceptAgenticBoardTaskDrafts
        Controller->>API: POST tasks/draft/accept
    end
    API-->>Controller: Task submission accepted
    Controller->>API: Refresh board snapshot
    API-->>Controller: Updated board
    Controller-->>Android: Render updated columns
    Note over Android,API: Later submissions reuse the board and skip initialization
```

## Task usage after addition

```mermaid
flowchart TD
    Backlog[Backlog: inspect scope and acceptance criteria] --> Edit[Discuss or edit planning work]
    Edit --> Approve[Move reviewed work to Todo]
    Backlog --> Approve
    Backlog --> Breakdown[Optional planning breakdown]
    Breakdown --> Backlog
    Approve --> Level{Executable Subtask?}
    Level -->|Story or Task| Plan[Break down approved parent scope]
    Plan --> Children[Review and approve child scope as required]
    Children --> Ready[Approved executable Subtask]
    Level -->|Yes| Ready
    Ready --> Dependencies{Dependencies and execution policy satisfied?}
    Dependencies -->|No| Wait[Wait for prerequisites or user action]
    Wait --> Dependencies
    Dependencies -->|Yes| Execute[Execute in progress]
    Execute --> Validate[Run required validation and evidence checks]
    Validate --> Result{Outcome}
    Result -->|Complete| Done[Done and parent rollup]
    Result -->|Needs review| Review[Review results and required approvals]
    Review --> Done
    Result -->|Failure| Fix[Inspect cause and use eligible retry or approved fix]
    Fix --> Ready
```

Only approved executable Subtasks run. Story and Task cards describe scope and
planning; adding Backlog cards does not grant execution approval. Start resumes
approved work; it is not a substitute for reviewing and approving Backlog.

## Reliability and compatibility

- Initialization is serialized within the mobile controller. Repeated first-use
  actions reuse the initialized board instead of creating extra seed cards.
- The server reuses an existing project board for prompt-free initialization
  without changing its tasks, configuration, or running/paused/scheduled state.
- If initialization succeeds but task submission fails, retry uses the existing
  board. Submission buttons are disabled while a request is in flight, and the
  dialog closes only after success.
- Interrupted initialization is reconciled by reading the board, not blindly
  retrying the create request. Project/connection changes invalidate pending
  initialization, including switching away and back.
- A lost task response can have an uncertain server outcome. Refresh and inspect
  the board before resubmitting; task writes are not guaranteed idempotent.
  Multiple manual cards are submitted individually, so earlier cards may have
  succeeded even if a later card fails.
- The mobile build requires the corresponding server change that permits
  prompt-free initialization. Existing clients may still send a legacy initial
  prompt; those requests continue to create their initial Backlog work item.

## Implementation

- Android board UI: `apps/mobile/androidApp/src/main/kotlin/io/workbench/mobile/ui/board-screen.kt`
- Android navigation: `apps/mobile/androidApp/src/main/kotlin/io/workbench/mobile/ui/lifecycle-and-shell.kt`
- Android dialogs: `apps/mobile/androidApp/src/main/kotlin/io/workbench/mobile/ui/board-dialogs.kt`
- Shared initialization: `apps/mobile/shared/src/commonMain/kotlin/io/workbench/mobile/shared/controller/connection-and-projects.kt`
- Empty-board tests: `apps/mobile/shared/src/commonTest/kotlin/io/workbench/mobile/shared/MobileControllerEmptyBoardTest.kt`
- Server initialization: `crates/iowb-server/src/agentic_board/http/boards.rs` and `crates/iowb-server/src/agentic_board/types.rs`

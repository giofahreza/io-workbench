fn build_task_execution_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    if task.final_qa_task {
        return build_final_qa_prompt(run, task, index);
    }
    if rag_phase_for_task(task) == "fix" {
        return build_fix_prompt(run, task, index);
    }
    if canonical_task_kind(task) == TASK_KIND_MANUAL_TEST {
        return build_manual_test_prompt(run, task, index);
    }
    if matches!(
        canonical_task_kind(task),
        TASK_KIND_QA | TASK_KIND_REVIEW | TASK_KIND_RESEARCH | TASK_KIND_DESIGN
    ) {
        return build_validation_prompt(run, task, index);
    }
    if canonical_task_kind(task) == TASK_KIND_TEST_IMPLEMENTATION {
        return build_test_implementation_prompt(run, task, index);
    }
    build_dev_prompt(run, task, index)
}

fn task_scope_block(_run: &AgenticBoard, task: &BoardTask) -> String {
    let acceptance = if task.acceptance_criteria.is_empty() {
        "None recorded.".to_string()
    } else {
        task.acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "Ticket scope (source of truth):\nDescription:\n{}\nAcceptance criteria:\n{}",
        if task.description.trim().is_empty() {
            &task.details
        } else {
            &task.description
        },
        acceptance,
    )
}

fn build_dev_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    build_execution_prompt_with_mode(
        run,
        task,
        index,
        "Dev",
        "Implement the smallest production change that satisfies the generated tests and acceptance criteria.",
    )
}

fn build_fix_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    build_execution_prompt_with_mode(
        run,
        task,
        index,
        "Fix",
        "Fix the latest validation failure first; preserve generated tests and use the failure logs as the repair target.",
    )
}

fn build_final_qa_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    build_execution_prompt_with_mode(
        run,
        task,
        index,
        "Final QA",
        "Validate the ticket description and acceptance criteria against current files, generated-test evidence, and deterministic validation history before returning done.",
    )
}

fn build_manual_test_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    let acceptance = if task.acceptance_criteria.is_empty() {
        "- Verify the card exactly as described.".to_string()
    } else {
        task.acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let references = if task.references.is_empty() {
        "None".to_string()
    } else {
        task.references.join("\n")
    };
    let scope = task_scope_block(run, task);
    let result_schema = {
        r#"{
  "status": "done" | "blocked" | "needs_followup",
  "summary": "short manual verification summary",
  "changedFiles": [],
  "commandsRun": ["commands/checks actually run"],
  "qaResult": "pass" | "fail" | "blocked" | "not_run",
  "evidence": ["specific observed app behavior, command output, logs, or file inspection evidence"],
  "manualTestEnvironment": {
    "deviceOrEmulator": "device, emulator, or simulator identifier",
    "appVersion": "app/build version under test",
    "backendUrl": "backend URL, or none/local-only when no backend is used",
    "osVersion": "optional OS version"
  },
  "manualTestSteps": ["each manual step followed by the observed outcome"],
  "manualTestResult": "explicit observed pass/result for the manual verification",
  "externalSideEffects": ["what declared external state changed, or explicitly state that it did not change"],
  "remainingIssues": [],
  "remainingGaps": [],
  "suggestedBacklogTasks": [
    {
      "title": "one concrete engineering fix",
      "kind": "implementation",
      "level": "story",
      "details": "code-changing fix needed because verification found a defect",
      "acceptanceCriteria": ["verifiable outcome"],
      "priority": "p0|p1|p2|p3"
    }
  ]
}"#
    };
    format!(
        r#"You are running a manual verification task for an io-workbench Kanban board.

Prompt template: Manual Test
Project: {project_name}
Project path: {project_path}
Board id: {board_id}
{board_profile_block}

{git_policy_block}

Task {task_number}: {task_id}
Title: {title}
Priority: {priority}

Details:
{details}

Prompt:
{prompt}

Acceptance criteria:
{acceptance}

References:
{references}

{scope}

Applicable AGENTS.md advisory guidance:
{agents_context}

RAG context:
{rag_context}

Codebase reconnaissance:
{codebase}

Workspace baseline:
{workspace_baseline}

Completed implementation task summaries:
{completed_tasks}

Instructions:
- Treat this as verification/manual testing work, not an implementation task.
- Do not edit source, production, generated, test, config, lock, or documentation files for this card.
- Run read-only inspection, build, test, app, emulator, browser, or log commands when practical to verify the described workflow.
- If the workflow is already correct, return status "done" with concrete evidence.
- Always record manualTestEnvironment with the device/emulator, app version, and backend URL used. Use "none/local-only" when the app has no backend.
- Always record manualTestSteps as the ordered manual actions and their observed outcomes, and manualTestResult as the explicit overall result. A failed or blocked manual check must not be reported as done.
- If a product defect or missing implementation is found, return status "needs_followup" and add code-changing fixes to "suggestedBacklogTasks" with kind "implementation".
- If verification cannot run because of environment, credentials, dependency, or tool failure, return status "blocked" with exact blocker evidence.
- Never run git commit, git push, create tags, or otherwise change git history.
- Do not ask for user confirmation.
Return JSON only, with this schema:
{result_schema}
"#,
        project_name = run.project_name,
        project_path = run.project_path,
        board_id = run.id,
        board_profile_block = board_profile_block(run),
        git_policy_block = git_policy_block(run),
        task_number = index + 1,
        task_id = task.id,
        title = task.title,
        priority = task.priority,
        details = if task.details.trim().is_empty() {
            &task.description
        } else {
            &task.details
        },
        prompt = task.prompt,
        acceptance = acceptance,
        references = references,
        scope = scope,
        result_schema = result_schema,
        agents_context = serde_json::to_string_pretty(&run.agents_context).unwrap_or_default(),
        rag_context = if task.rag_prompt_context.trim().is_empty() {
            "None".to_string()
        } else {
            task.rag_prompt_context.clone()
        },
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
        workspace_baseline =
            serde_json::to_string_pretty(&run.workspace_baseline).unwrap_or_default(),
        completed_tasks = completed_task_summary(run),
    )
}

fn build_validation_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    let kind = canonical_task_kind(task);
    let acceptance = if task.acceptance_criteria.is_empty() {
        "- Verify the card exactly as described.".to_string()
    } else {
        task.acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let scope = task_scope_block(run, task);
    format!(
        r#"You are executing a read-only {kind} subtask for an io-workbench Kanban board.

Project: {project_name}
Project path: {project_path}
Board id: {board_id}
Task {task_number}: {task_id}
Title: {title}

Description:
{details}

Acceptance criteria:
{acceptance}

{scope}

Codebase reconnaissance:
{codebase}

Instructions:
- This is one {kind} purpose only. Do not implement production behavior.
- Do not edit source, production, generated, test, config, lock, or documentation files.
- Use read-only inspection and run the smallest relevant checks, tests, emulator, browser, or log commands.
- If this is research, do not implement the recommendation. Put proposed initiative, epic, story, or task work in proposedPlanningItems; it remains Backlog-only until the user accepts it.
- For a defect, report the exact observed failure and return `needs_followup`; the board will create a separate concrete fix subtask under the same parent.
- Return JSON only, with no markdown fence:
{{
  "status": "done" | "blocked" | "needs_followup",
  "summary": "short validation summary",
  "changedFiles": [],
  "commandsRun": ["commands actually run"],
  "qaResult": "pass" | "fail" | "blocked" | "not_run",
  "evidence": ["specific observed evidence"],
  "externalSideEffects": ["what declared external state changed, or explicitly state that it did not change"],
  "remainingIssues": ["exact defects or blockers"],
  "remainingGaps": [],
  "proposedPlanningItems": [
    {{
      "level": "initiative|epic|story|task",
      "title": "proposed planning item",
      "kind": "implementation|design|research",
      "details": "scope that still needs user approval",
      "acceptanceCriteria": ["verifiable outcome"],
      "priority": "p0|p1|p2|p3"
    }}
  ]
}}"#,
        project_name = run.project_name,
        project_path = run.project_path,
        board_id = run.id,
        task_number = index + 1,
        task_id = task.id,
        title = task.title,
        details = if task.details.trim().is_empty() {
            &task.description
        } else {
            &task.details
        },
        acceptance = acceptance,
        scope = scope,
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
    )
}

fn build_test_implementation_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    build_execution_prompt_with_mode(
        run,
        task,
        index,
        "Test Implementation",
        "Add or update only automated test files and the smallest test fixtures required to verify this subtask. Do not edit production behavior.",
    )
}

fn build_execution_prompt_with_mode(
    run: &AgenticBoard,
    task: &BoardTask,
    index: usize,
    prompt_mode: &str,
    phase_instruction: &str,
) -> String {
    let acceptance = if task.acceptance_criteria.is_empty() {
        "- Complete the card exactly as described.".to_string()
    } else {
        task.acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let references = if task.references.is_empty() {
        "None".to_string()
    } else {
        task.references.join("\n")
    };
    let tdd_context = if task.qa_test_commands.is_empty() && task.qa_test_paths.is_empty() {
        "No generated TDD tests are attached to this task.".to_string()
    } else {
        format!(
            "TDD phase: {}\nGenerated test files:\n{}\nGenerated test commands:\n{}\nBaseline validation:\n{}\nLatest deterministic validation:\n{}",
            task.tdd_phase,
            if task.qa_test_paths.is_empty() {
                "None".to_string()
            } else {
                task.qa_test_paths.join("\n")
            },
            if task.qa_test_commands.is_empty() {
                "None".to_string()
            } else {
                task.qa_test_commands.join("\n")
            },
            serde_json::to_string_pretty(&task.qa_baseline_validation).unwrap_or_default(),
            serde_json::to_string_pretty(&task.deterministic_validation).unwrap_or_default(),
        )
    };
    let scope = task_scope_block(run, task);
    let result_schema = {
        r#"{
  "status": "done" | "blocked" | "needs_followup",
  "summary": "short summary",
  "changedFiles": ["files changed"],
  "commandsRun": ["commands/checks actually run"],
  "qaResult": "pass" | "fail" | "blocked" | "not_run",
  "evidence": ["specific file, command, or behavior evidence"],
  "externalSideEffects": ["what declared external state changed, or explicitly state that it did not change"],
  "remainingIssues": [],
  "remainingGaps": [],
  "suggestedBacklogTasks": []
}"#
    };
    format!(
        r#"You are running an autonomous implementation queue in danger mode for an io-workbench Kanban board.

Prompt template: {prompt_mode}
Project: {project_name}
Project path: {project_path}
Board id: {board_id}
{board_profile_block}

{git_policy_block}

Task {task_number}: {task_id}
Title: {title}
Priority: {priority}

Details:
{details}

Prompt:
{prompt}

Acceptance criteria:
{acceptance}

External side effects:
{side_effects}

References:
{references}

{scope}

Applicable AGENTS.md advisory guidance:
{agents_context}

RAG context:
{rag_context}

TDD generated tests:
{tdd_context}

Codebase reconnaissance:
{codebase}

Workspace baseline:
{workspace_baseline}

Completed implementation task summaries:
{completed_tasks}

Instructions:
- Work directly in the project directory.
- Inspect current files before editing; do not rebuild already completed work.
- {phase_instruction}
- Make minimal changes needed to complete this task correctly.
- Implement only this Kanban subtask and its ticket scope. Do not implement workspace user experience.
- Do not weaken, skip, delete, or rewrite generated TDD tests except to fix broken test syntax or align with existing test harness conventions.
- Run focused local checks when practical.
- When calling tools, use strict JSON argument types. Integer fields such as session_id, yield_time_ms, max_output_tokens, counts, limits, and offsets must be JSON integers like 60000, not floats like 60000.0.
- Never run git commit, git push, create tags, or otherwise change git history.
- Do not ask for user confirmation.
Return JSON only, with this schema:
{result_schema}
"#,
        project_name = run.project_name,
        project_path = run.project_path,
        board_id = run.id,
        board_profile_block = board_profile_block(run),
        git_policy_block = git_policy_block(run),
        task_number = index + 1,
        task_id = task.id,
        title = task.title,
        priority = task.priority,
        details = if task.details.trim().is_empty() {
            &task.description
        } else {
            &task.details
        },
        prompt = task.prompt,
        acceptance = acceptance,
        side_effects = if task.hierarchy.side_effects.is_empty() {
            "None declared. If the task discovers an external side effect, stop and report it instead of silently proceeding.".to_string()
        } else {
            task.hierarchy.side_effects.join("\n")
        },
        references = references,
        scope = scope,
        result_schema = result_schema,
        agents_context = serde_json::to_string_pretty(&run.agents_context).unwrap_or_default(),
        rag_context = if task.rag_prompt_context.trim().is_empty() {
            "None".to_string()
        } else {
            task.rag_prompt_context.clone()
        },
        tdd_context = tdd_context,
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
        workspace_baseline =
            serde_json::to_string_pretty(&run.workspace_baseline).unwrap_or_default(),
        completed_tasks = completed_task_summary(run),
        prompt_mode = prompt_mode,
        phase_instruction = phase_instruction,
    )
}

fn build_qa_generation_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    let acceptance = if task.acceptance_criteria.is_empty() {
        "- Complete the card exactly as described.".to_string()
    } else {
        task.acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let scope = task_scope_block(run, task);
    let result_schema = {
        r#"{
  "status": "done" | "blocked",
  "summary": "short QA summary",
  "testFiles": ["test file paths created or updated"],
  "commands": ["focused commands to run these generated tests"],
  "notes": []
}"#
    };
    format!(
        r#"You are the QA phase of a TDD-first io-workbench Kanban board worker.

Goal: create failing tests before implementation. Do not implement the feature.

Project: {project_name}
Project path: {project_path}
Board id: {board_id}
{board_profile_block}

{git_policy_block}

Task {task_number}: {task_id}
Title: {title}

Details:
{details}

Acceptance criteria:
{acceptance}

{scope}

RAG context:
{rag_context}

Codebase reconnaissance:
{codebase}

Rules:
- Inspect current test conventions before adding tests.
- Create or update only test files and small fixtures required by tests.
- Do not implement production behavior to make tests pass.
- Generated tests must fail against the current baseline for the expected missing behavior.
- Return JSON only. No markdown fence.

Schema:
{result_schema}"#,
        project_name = run.project_name,
        project_path = run.project_path,
        board_id = run.id,
        board_profile_block = board_profile_block(run),
        git_policy_block = git_policy_block(run),
        task_number = index + 1,
        task_id = task.id,
        title = task.title,
        details = if task.details.trim().is_empty() {
            &task.description
        } else {
            &task.details
        },
        acceptance = acceptance,
        scope = scope,
        result_schema = result_schema,
        rag_context = if task.rag_prompt_context.trim().is_empty() {
            "None"
        } else {
            &task.rag_prompt_context
        },
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
    )
}

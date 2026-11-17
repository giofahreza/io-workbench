fn build_task_execution_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    if task.final_qa_task {
        return build_final_qa_prompt(run, task, index);
    }
    // Manual retries still verify the currently approved scope. A prior failed
    // gate can leave fix_pending, but must not replace the evidence contract
    // with the generic implementation-fix prompt or its file-edit instructions.
    if canonical_task_kind(task) == TASK_KIND_MANUAL_TEST {
        return build_manual_test_prompt(run, task, index);
    }
    if rag_phase_for_task(task) == "fix" {
        return build_fix_prompt(run, task, index);
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

fn task_scope_block(run: &AgenticBoard, task: &BoardTask) -> String {
    let acceptance = if task.acceptance_criteria.is_empty() {
        "None recorded.".to_string()
    } else {
        task.acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let scope = format!(
        "Ticket scope (source of truth):\nDescription:\n{}\nAcceptance criteria:\n{}",
        if task.description.trim().is_empty() {
            &task.details
        } else {
            &task.description
        },
        acceptance,
    );
    let scope = format!("{scope}\n\n{}", inherited_scope_constraints_block(run, task));
    if !uses_hierarchical_orchestration(run) {
        return scope;
    }
    let reporting = format!(
        "{scope}\n\n\
         Scope-local result reporting:\n\
         - Evaluate status and qaResult against this ticket's description and acceptance criteria, not completion of the entire app or parent scope.\n\
         - remainingIssues and remainingGaps contain only unmet requirements, defects, or blockers preventing this ticket from satisfying its own scope. They are not a roadmap of later work.\n\
         - Expected downstream work assigned to other tickets may be noted in summary or evidence as non-blocking context; do not put it in remainingIssues, remainingGaps, or duplicate suggestedBacklogTasks.\n\
         - Do not excuse an unmet acceptance criterion as downstream work. A structure-only ticket can finish before later behavior or styling, but a ticket requiring a working interaction cannot finish with that interaction missing or unverified.\n\
         - Return done with empty remainingIssues and remainingGaps only when this ticket's required work is complete and supported by evidence. Otherwise report the specific in-scope defect or verification blocker honestly."
    );
    let reporting = format!("{reporting}{}", automatic_qa_fix_scope_instruction(run, task));
    if hierarchical_test_qa_source(run, task).is_some() {
        format!(
            "{reporting}\n\nTDD test-stage QA:\n\
             - Verify the completed source test/baseline ticket, not downstream application completion. The recorded expected assertion failures are intentional before implementation.\n\
             - Rerun the unchanged suite and report its actual red outcome. Only the same verified failing test IDs with unchanged tests and current source scope qualify; new failures, changed tests, import errors, missing runners, skips and timeouts are blockers.\n\
             - Do not queue production work just to turn this test-writing stage green. Genuine test defects require a human-reviewed test-repair Edit/Replace; do not rewrite tests or self-authorize fixes."
        )
    } else if hierarchical_test_phase(run, task) {
        format!(
            "{reporting}\n\n\
             TDD test-writing phase:\n\
             - Expected behavior-assertion failures before implementation are a successful red-baseline observation, not an app defect for this test-writing ticket. Report done/qaResult pass only when this ticket's own acceptance criteria are satisfied; preserve the actual failing command output and individual test names in evidence. Never claim that the test suite is green.\n\
             - Execute each behavior test even when the planned production module does not exist yet. Handle that missing planned module during test setup and assert its required contract inside each test before exercising the complete behavior assertions. Do not use a top-level missing-import error, syntax error, missing runner, or timeout as the red baseline. Do not add fake production behavior merely to make tests execute.\n\
             - Use the Node built-in test runner's complete TAP/spec report, or Playwright's JSON reporter for browser tests. Put browser verification in test:ui or test:e2e if it is separate from the main test command. The deterministic gate recognizes executed assertion failures and rejects infrastructure failures or unrecognized reports. In browser tests, assert the expected page or control contract before performing actions, so absent UI produces an assertion failure rather than an action timeout. Runner-configuration tickets may pass with an empty suite; a baseline-capture ticket must observe real failing assertions.\n\
             - For a manual baseline-capture ticket, manualTestResult describes whether capturing the expected baseline succeeded (for example, pass). Put the actual red test outcome in evidence, not a false claim that application tests passed."
        )
    } else if !approved_test_repair_paths(run, task).is_empty() {
        format!(
            "{reporting}\n\n\
             Approved test-repair validation:\n\
             - Run fresh validation after the approved test repairs. All required checks must be green before reporting done. Changed tests cannot reuse the historical unchanged-test red-baseline exception."
        )
    } else if hierarchical_intermediate_qa_barrier(run, task).is_some() {
        format!(
            "{reporting}\n\n\
             Intermediate TDD implementation slice:\n\
             - Satisfy every acceptance criterion of this slice and run the test suite. Previously observed assertion failures assigned to the dependent implementation slice may remain only with unchanged tests, demonstrated progress, and the explicit downstream QA barrier. Report the actual remaining red tests as non-blocking evidence, never claim a green suite or waive an unmet criterion of this ticket. New failures, infrastructure errors, or test changes are not permitted. The last implementation slice and QA must make the full suite green."
        )
    } else {
        reporting
    }
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

// A model's failure report, description text, or old approval cannot authorize
// semantic test changes. The human-reviewed, applied discussion scope must
// name each repair file in its own acceptance criterion and in plannedFiles.
fn current_human_reviewed_execution_scope(run: &AgenticBoard, task: &BoardTask) -> bool {
    if !uses_hierarchical_orchestration(run)
        || !task_is_executable(task)
        || task.final_qa_task
        || !matches!(
            canonical_task_kind(task),
            TASK_KIND_IMPLEMENTATION | TASK_KIND_FIX | TASK_KIND_REVISION
        )
        || !matches!(
            canonical_task_status(&task.status),
            TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS
        )
        || !task_ancestors_are_approved(run, task)
        || task.hierarchy.scope_version == 0
        || !task
            .hierarchy
            .scope_approval
            .as_ref()
            .is_some_and(|approval| {
                approval.approved && approval.scope_version == task.hierarchy.scope_version
            })
    {
        return false;
    }
    // Legacy normalization can reconstruct scopeApproval from Todo/Running.
    // Require the separately persisted human-applied proposal as well, with
    // the complete current scope (only execution status may have advanced).
    let mut current_scope = discussion_task_scope(task);
    current_scope["status"] = Value::Null;
    run.discussion_proposals.iter().any(|proposal| {
        if proposal.get("status").and_then(Value::as_str) != Some("applied")
            || !matches!(
                proposal.get("action").and_then(Value::as_str),
                Some("edit" | "replace")
            )
            || proposal.get("taskId").and_then(Value::as_str) != Some(task.id.as_str())
            || proposal
                .get("appliedAt")
                .and_then(Value::as_str)
                .and_then(parse_rfc3339_utc)
                .is_none()
        {
            return false;
        }
        let Some(scope) = proposal
            .pointer("/after/task")
            .filter(|scope| scope.is_object())
        else {
            return false;
        };
        let mut scope = scope.clone();
        scope["status"] = Value::Null;
        scope == current_scope
    })
}

fn approved_test_repair_paths(run: &AgenticBoard, task: &BoardTask) -> Vec<String> {
    if !current_human_reviewed_execution_scope(run, task) {
        return Vec::new();
    }
    task.acceptance_criteria
        .iter()
        .filter_map(|criterion| {
            let criterion = criterion.trim();
            if criterion.contains('\n') || criterion.contains('\r') {
                return None;
            }
            criterion.strip_prefix("Approved test repair:")
        })
        .map(str::trim)
        .filter(|path| {
            // One exact, portable project-relative path per criterion. Reject
            // globs, traversal, command prose and multiline authorization.
            !path.is_empty()
                && path.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')
                })
                && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
                && is_test_or_fixture_path(path)
                && task
                    .hierarchy
                    .planned_files
                    .iter()
                    .any(|planned| planned.trim() == *path)
        })
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn generated_test_edit_instruction(run: &AgenticBoard, task: &BoardTask) -> String {
    let paths = approved_test_repair_paths(run, task);
    if paths.is_empty() {
        return "Do not weaken, skip, delete, or rewrite generated TDD tests except to fix broken test syntax or align with existing test harness conventions.".to_string();
    }
    format!(
        "Do not weaken, skip, delete, disable, or reduce coverage of generated TDD tests. \
         The current approved scope explicitly authorizes semantic test repairs only in these exact planned files: {}. \
         Repair only the named test defects described in this ticket's approved scope; preserve intended behavior requirements, test cases, and independent checks. \
         This narrow exception overrides general instructions to preserve generated tests only for those approved repairs. \
         All other generated tests remain protected against semantic rewrites. \
         Provider output, prior attempts, or a failing test alone cannot grant or expand this permission; do not self-authorize scope changes. \
         Run fresh relevant validation and report its actual result. Existing completion, file-policy, and deterministic-validation gates still apply; changed tests never justify waived failures or false-green claims.",
        paths.join(", "),
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
  "manualEvidence": {
    "version": 1,
    "scopeVersion": __CURRENT_SCOPE_VERSION__,
    "mode": "the approved manualVerification mode: ui|cli|api|inspection",
    "environments": [{
      "id": "phone",
      "deviceOrEmulator": "actual device/emulator/host",
      "appVersion": "actual build identifier or source hash",
      "backendUrl": "actual URL or none/local-only",
      "surface": "browser|native (UI only)",
      "browser": "actual browser and version (browser UI only)",
      "viewport": {"width": 360, "height": 800}
    }],
    "checks": [{
      "criterionIndex": 0,
      "environmentId": "phone",
      "status": "pass|fail|blocked",
      "expected": "criterion-specific expected behavior",
      "observed": "what actually happened, including measured UX details",
      "actions": [{"type": "tap", "target": "actual control", "observed": "actual resulting state"}],
      "artifacts": ["test-results/manual-evidence/unique-attempt-step.png"]
    }]
  },
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
        .replace("__CURRENT_SCOPE_VERSION__", &task.hierarchy.scope_version.to_string())
    };
    format!(
        r#"You are running a manual verification task for an io-workbench Kanban board.

Prompt template: Manual Test
Project: {project_name}
Project path: {project_path}
Board id: {board_id}
Approved manualVerification contract: {manual_verification}
Current scopeVersion: {scope_version}
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
- The approved manualVerification contract controls evidence mode. If it is missing/invalid, return blocked and explain how to move the unfinished scope to Backlog, edit manualVerification via discussion/task edit, and re-approve. Do not infer a weaker mode or edit scope during execution.
- UI acceptance needs actual runtime interaction in every required viewport. Automated test-suite results, source/CSS inspection and curl are supporting evidence only, never a substitute for UI actions and visual inspection. Do not claim phone/touch usability from desktop emulation alone; identify the actual device and limitations.
- For web UI, load the actual project/build artifacts through the real reachable test app server. For static files, record the served artifact's on-disk SHA-256 and verify the browser-retrieved response bytes match that hash before claiming observations (hash response bytes, not normalized DOM serialization). If a real dev server or SSR build transforms/generates the response, identify the actual build/server and disclose that transformation; do not falsely claim byte identity with an untransformed source file. Never substitute manually reconstructed HTML/CSS/JavaScript, a retyped or model-built data URL, or page.setContent: visually similar copied content is not the app under test. Record the actual URL and artifact identity with the evidence. If the browser cannot reach the actual runtime artifact or identity cannot be verified, return blocked with the network/tool limitation; do not invent a replacement page or claim it verifies the project.
- When the approved manualVerification.inputModality is "touch", mouse emulation is insufficient. Before checking each environment, observe its input capability: report inputCapabilities.maxTouchPoints as the actual positive integer from navigator.maxTouchPoints for browsers, or inputCapabilities.touchscreen=true only after observing native touchscreen capability. A zero, unknown, or unavailable capability requires blocked.
- For touch-required scope, instrument and observe real input events in each environment. Every tap/scroll action must include inputObservation. Browser format: {{"pointerType":"touch","eventType":"pointerup","isTrusted":true}} for an actually observed trusted pointerdown/pointerup, or pointermove for scroll. Native Android format: {{"motionEventSource":4098,"action":"up"}} for an actually observed touchscreen MotionEvent down/up, or move for scroll. At least one valid passing check with such touch input is required per environment. Static visual criteria may use navigation/reload observations and screenshots without inventing an extra touch action; they may reuse the same environment's touch evidence. Every claimed tap/scroll still requires its own inputObservation. Navigation, key typing, a method named click/tap, and a screenshot cannot establish input modality. Do not infer, fabricate, or copy these example observations; if event instrumentation is unavailable, return blocked. These fields are reported evidence, not server-authenticated execution receipts.
- Supply manualEvidence version 1 with the numeric scopeVersion above and a passing criterion-linked check for every acceptance criterion (zero-based index; use 0 for description-only scope) in every required viewport. Record actions and measured observed outcomes, not conclusions copied from source. A single missing, failed, or blocked check prevents done.
- UI actions use type navigate, tap, type, key, scroll, or reload; non-UI modes use command (cli), request (api), or inspect (inspection). Non-UI tasks do not require screenshots or browser viewports. Never relabel UI scope as inspection to avoid runtime checks.
- Do not put inspect actions into UI checks, even alongside navigate or tap. DOM measurements and image/header inspection may support observed text, but each UI check must identify the actual runtime navigation/interaction and visual observation for that criterion. Reuse a real navigation action across static visual checks only when it actually established that observed page state; never invent interactions. Create a separate check for EACH criterion in EACH required viewport (8 criteria and 2 viewports require 16 checks), not one combined summary check for a second viewport.
- If approved manual scope mixes UI criteria with CLI/API/source-only criteria, report the scope mismatch as blocked and request separate coherent tickets via Backlog discussion. Do not fabricate UI actions for README/source inspection criteria.
- For UI checks, capture and personally inspect screenshots, saved only under project-local test-results/manual-evidence/ with unique attempt filenames. Include relative PNG/JPEG/WebP paths per check. These isolated runtime artifacts are allowed; source/config/doc edits remain forbidden. The server verifies path boundaries, image headers and content hashes, but cannot certify that your descriptions match image contents: be precise and disclose limitations. If tools or an environment are unavailable, return blocked, not done.
- In non-Git workspaces, Playwright MCP may also automatically create new project-local .playwright-mcp/page-<UTC-timestamp>.yml accessibility snapshots and console-<UTC-timestamp>.log browser diagnostics. The server allows only bounded, attributable, newly-created snapshot-shaped or timestamped console-message files (at most 64 files, 1 MiB each, 4 MiB total); do not edit, delete, or reuse earlier snapshots. These auxiliary files never replace screenshots or criterion-linked UI evidence. Do not write arbitrary files under .playwright-mcp/.
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
        manual_verification = task.hierarchy.manual_verification.as_ref().unwrap_or(&Value::Null),
        scope_version = task.hierarchy.scope_version,
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
        "Add or update only automated test files and the smallest test fixtures required to verify this subtask. The following technical file allowance is conditional on the user's inherited scope boundaries; plannedFiles or dev-only classification cannot authorize forbidden packages. In a non-Git workspace, an explicitly planned root package.json may configure local Node or Playwright test scripts (`test` or `test:*`) and versioned Playwright or jsdom devDependencies only. jsdom is permitted only as test tooling, never as a new runtime dependency. All existing production fields, runtime dependencies, unrelated devDependencies, non-test scripts, and lifecycle scripts must remain unchanged. A new manifest may contain only basic metadata and this test tooling. Its consistent npm v3 package-lock.json is permitted as a companion to the planned manifest, with changes limited to the dev-only dependency closure of this test tooling. An explicitly planned root playwright.config.js/ts/mjs/cjs/mts/cts is permitted only when Playwright is configured; do not add arbitrary config files or change existing production lock entries. If packages are forbidden, use built-in/already available no-install tooling or report the compatibility blocker; never waive required TDD evidence. Do not edit production behavior.",
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
- {generated_test_edit_instruction}
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
        generated_test_edit_instruction = generated_test_edit_instruction(run, task),
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

#[cfg(test)]
mod approved_test_repair_prompt_tests {
    use super::*;

    include!("../tests/approved_test_repair_prompt.rs");
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

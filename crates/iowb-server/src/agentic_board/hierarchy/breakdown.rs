fn next_hierarchy_level(level: &str) -> Option<&'static str> {
    match level {
        TASK_LEVEL_INITIATIVE => Some(TASK_LEVEL_EPIC),
        TASK_LEVEL_EPIC => Some(TASK_LEVEL_STORY),
        TASK_LEVEL_STORY => Some(TASK_LEVEL_TASK),
        TASK_LEVEL_TASK => Some(TASK_LEVEL_SUBTASK),
        _ => None,
    }
}

fn direct_hierarchy_children<'a>(run: &'a AgenticBoard, parent_id: &str) -> Vec<&'a BoardTask> {
    run.tasks
        .iter()
        .filter(|task| task.hierarchy.parent_id.as_deref() == Some(parent_id))
        .collect()
}

fn hierarchy_breakdown_child_title_candidates(
    run: &AgenticBoard,
    parent_id: &str,
    next_level: &str,
) -> Vec<String> {
    let mut titles = direct_hierarchy_children(run, parent_id)
        .into_iter()
        .filter(|task| task_level(task) == next_level)
        .map(|task| normalize_suggested_task_key(&task.title))
        .filter(|title| !title.is_empty())
        .collect::<BTreeSet<_>>();

    // Risky or out-of-scope generated children are wrapped in an unparented
    // Backlog story. Keep their original title key tied to the source parent
    // so a later manual refinement cannot recreate the same proposal.
    for task in &run.tasks {
        if task.task_origin != "hierarchy_backlog_wrapper" {
            continue;
        }
        for entry in &task.hierarchy.discussion {
            if entry.get("kind").and_then(Value::as_str) != Some("generated_scope_wrapper")
                || entry.get("sourceParentId").and_then(Value::as_str) != Some(parent_id)
                || entry.get("wrappedLevel").and_then(Value::as_str) != Some(next_level)
            {
                continue;
            }
            if let Some(title) = entry.get("sourceTitleKey").and_then(Value::as_str) {
                let title = normalize_suggested_task_key(title);
                if !title.is_empty() {
                    titles.insert(title);
                }
            }
        }
    }

    titles.into_iter().collect()
}

fn hierarchy_breakdown_has_children(run: &AgenticBoard, parent_id: &str, next_level: &str) -> bool {
    !hierarchy_breakdown_child_title_candidates(run, parent_id, next_level).is_empty()
}

fn hierarchy_breakdown_max_new_children(existing_count: usize) -> usize {
    MAX_HIERARCHY_CHILDREN_PER_PARENT
        .saturating_sub(existing_count)
        .min(if existing_count > 0 {
            MAX_HIERARCHY_REFINEMENT_ADDITIONS
        } else {
            MAX_HIERARCHY_CHILDREN_PER_PARENT
        })
}

fn validate_hierarchy_breakdown_parent(run: &AgenticBoard, parent: &BoardTask) -> Result<()> {
    if parent.hierarchy.manual_verification.is_some()
        && let Some(issue) = manual_task_contract_issue(parent)
    {
        return Err(bad_request(issue));
    }
    if task_status_is_todo(&parent.status) && !task_ancestors_are_approved(run, parent) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Approve every parent planning item before breaking down a nested Todo item.",
        ));
    }
    Ok(())
}

fn next_hierarchy_parent(run: &AgenticBoard) -> Option<BoardTask> {
    let mut candidates = run
        .tasks
        .iter()
        .filter(|task| task_status_is_todo(&task.status))
        .filter(|task| !task_is_executable(task))
        .filter(|task| task_ancestors_are_approved(run, task))
        .filter_map(|task| {
            let next_level = next_hierarchy_level(task_level(task))?;
            let has_children = hierarchy_breakdown_has_children(run, &task.id, next_level);
            (!has_children).then(|| (task_level(task), task.clone()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(level, task)| {
        let depth = match *level {
            TASK_LEVEL_TASK => 0,
            TASK_LEVEL_STORY => 1,
            TASK_LEVEL_EPIC => 2,
            TASK_LEVEL_INITIATIVE => 3,
            _ => 4,
        };
        (depth, task_priority_rank(&task.priority), task.id.clone())
    });
    candidates.into_iter().next().map(|(_, task)| task)
}

fn hierarchy_breakdown_testing_policy(run: &AgenticBoard, parent: &BoardTask) -> String {
    if !run.tdd_enabled {
        return "TDD is disabled. Still plan verification appropriate to the requested behavior."
            .to_string();
    }

    // A task may already be one stage of an approved tests -> implementation
    // -> QA plan. Show the actual dependency scopes rather than demanding a
    // second copy of the whole lifecycle below every planning item. These are
    // planning records, not an assertion that their execution gates passed.
    let related_stages = run
        .tasks
        .iter()
        .filter(|task| task.id != parent.id && task.superseded_by.is_none())
        .filter_map(|task| {
            let relation = if hierarchical_task_depends_on(run, parent, &task.id) {
                "Existing prerequisite"
            } else if matches!(canonical_task_kind(task), TASK_KIND_QA | TASK_KIND_MANUAL_TEST)
                && hierarchical_task_depends_on(run, task, &parent.id)
            {
                "Existing downstream verification"
            } else {
                return None;
            };
            Some(format!(
                "{relation}: {} [{}]\nTitle: {}\nScope: {}\nAcceptance: {}\nPrerequisites: {}",
                task.id,
                canonical_task_kind(task),
                task.title,
                task.description,
                task.acceptance_criteria.join("; "),
                task_blockers(task).join(", ")
            ))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let stage_scope = if task_level(parent) == TASK_LEVEL_TASK {
        match canonical_task_kind(parent) {
            TASK_KIND_TEST_IMPLEMENTATION =>
                "Refine only this test-writing/baseline stage. Do not add production implementation or post-implementation QA children. Preserve the required executable tests and baseline evidence in this stage.",
            TASK_KIND_IMPLEMENTATION =>
                "Refine the implementation scope, reusing the prerequisite tests/baseline and downstream QA that cover it. Do not duplicate an existing test/baseline prerequisite or downstream QA stage inside this parent. Preserve an explicit one-implementation-subtask scope; producing several app files does not by itself require separate tickets. Running the existing suite to prove this implementation is green is completion verification, not a reason to add a duplicate QA stage.",
            TASK_KIND_QA | TASK_KIND_MANUAL_TEST | TASK_KIND_REVIEW =>
                "Refine only this verification stage. Do not create a fresh pre-development baseline or implementation stage. Preserve the checks, evidence, and dependencies assigned to this parent.",
            _ => "Keep the children within the parent's assigned engineering purpose.",
        }
    } else {
        "For behavior-changing scope not already covered by existing stages, plan separate test_implementation work before implementation and QA after implementation, with blockedBy edges preserving that order."
    };
    let evidence_contract = tdd_planning_evidence_contract(run);
    format!(
        "TDD is enabled. Policy: {}. Test-writing means executable automated tests, not a README/manual checklist. \
         When the policy requires a failing baseline, its acceptance criteria must include running the tests and \
         recording the expected failing result before development. Do not make test-writing depend on the \
         implementation it is meant to precede. Documentation-only work does not require a failing test.\n\n\
         {stage_scope}\n\n\
         Existing stages are reusable only for behavior their recorded scope and acceptance criteria cover. \
         If a required stage is missing, identify the missing prerequisite; do not invent completed evidence \
         or expand an explicitly restricted parent into other stages. Planned prerequisites are not evidence \
         of a completed baseline: their dependencies and required red evidence remain mandatory, and all applicable completion and green-test gates still apply.\n\n\
         {evidence_contract}\n\n\
         Relevant existing dependency scopes:\n{}",
        run.tdd_policy,
        if related_stages.is_empty() { "None recorded in this dependency chain." } else { &related_stages },
    )
}

fn build_hierarchy_breakdown_prompt(
    run: &AgenticBoard,
    parent: &BoardTask,
    next_level: &str,
) -> String {
    let existing_titles = hierarchy_breakdown_child_title_candidates(run, &parent.id, next_level);
    let refinement = !existing_titles.is_empty();
    let max_new_children = hierarchy_breakdown_max_new_children(existing_titles.len());
    let breakdown_mode = if refinement {
        "refinement / gap check"
    } else {
        "initial breakdown"
    };
    let children = hierarchy_breakdown_child_title_candidates(run, &parent.id, next_level)
        .into_iter()
        .map(|title| format!("- {title}"))
        .collect::<Vec<_>>()
        .join("\n");
    let direct_children = direct_hierarchy_children(run, &parent.id)
        .into_iter()
        .filter(|child| task_level(child) == next_level)
        .map(|child| format!("{} [{}] {}", child.id, task_level(child), child.title))
        .collect::<Vec<_>>()
        .join("\n");
    let acceptance = if parent.acceptance_criteria.is_empty() {
        "None recorded; derive concrete criteria from the parent description.".to_string()
    } else {
        parent.acceptance_criteria.join("\n- ")
    };
    let testing_policy = hierarchy_breakdown_testing_policy(run, parent);
    format!(
        r#"Break down exactly one approved Kanban item into its next hierarchy level.

Parent:
- id: {id}
- level: {level}
- kind: {kind}
- title: {title}
- description: {description}
- acceptance criteria:
- {acceptance}
- manual verification contract: {manual_verification}
- prerequisites: {dependencies}

Next level: {next_level}
Breakdown mode: {breakdown_mode}
Existing next-level child titles:
{children}
Existing direct child records:
{direct_children}

Codebase context:
{codebase}

Testing policy:
{testing_policy}

{scope_boundaries}

Return JSON only. No markdown fence.
Schema:
{{
  "items": [
    {{
      "id": "short-local-key",
      "level": "{next_level}",
      "title": "specific title",
      "kind": "research|design|implementation|test_implementation|qa|manual_test|review|fix|migration|revert|cleanup|revision|replacement",
      "description": "one-purpose scope",
      "acceptanceCriteria": ["specific verifiable outcome"],
      "priority": "p0|p1|p2|p3",
      "blockedBy": [],
      "required": true,
      "plannedFiles": [],
      "sideEffects": [],
      "manualVerification": {{"version": 1, "mode": "ui", "viewports": [{{"width": 360, "height": 800}}, {{"width": 414, "height": 896}}]}}
    }}
  ]
}}

Rules:
- Create only the next level, never skip a level.
- Give each new item a unique `id` within this response. In `blockedBy`, use
  `plan:<id>` for another new item, or `board:<id>` for an existing direct child.
  Record real prerequisites: verification depends on the implementation it checks,
  and work that consumes an artifact depends on the work that creates it. Do not
  leave every dependency empty when work must happen in order. Never depend on
  the parent itself; its prerequisites are inherited by the generated children.
- One subtask has one engineering purpose. Separate implementation, test-writing, QA, review, and manual testing into separate subtasks.
- For every manual_test item, declare manualVerification version 1 and the actual required mode. UI, browser, phone, touch, keyboard, or visual acceptance requires mode ui and concrete required viewports as objects with positive integer width and height, not dimension strings or device names. The schema dimensions are examples; use the actual approved scope. CLI/API verification uses cli/api with an empty viewports array; inspection is only for explicitly source/document inspection, never a substitute for runtime UI acceptance. Omit this field on non-manual items. Preserve the parent's manual verification contract when refining its manual scope. When acceptance explicitly requires genuine touch input, declare inputModality: "touch" in the UI manualVerification contract; omitted/"any" does not require touch. Touch scope requires observed touchscreen capability and touch events, and cannot pass using mouse clicks alone.
- UI manual acceptance requires real runtime interactions with criterion-linked observations and screenshots; automated-suite success, CSS/source inspection, and curl alone are insufficient. Plan a blocked outcome if the required interaction tools/environment are unavailable; never downgrade the acceptance criteria to bypass evidence.
- Split mixed UI, CLI/API, and source-inspection manual acceptance into separate coherent tickets with their own modes. Do not attach README setup or source-only inspection criteria to a UI ticket and then invent UI evidence for them. Every UI criterion is checked in every required viewport.
- Subtask titles must be concrete engineer execution tickets, such as `Add endpoint PATCH /budget/{{month}}` or `Run Android emulator smoke test for budget edit flow`.
- Do not create nice-to-have work as a required child. Put it in a separate Backlog story.
- Return no more than {max_new_children} genuinely new child item(s).
- If an existing child already covers the work, do not return it again, even when the title is worded differently.
- In refinement / gap check mode, return an empty items array when the existing children already cover the parent.
- Set `required` to false for optional or out-of-scope work, and list every
  possible external side effect in `sideEffects`; those children stay in Backlog
  until the user explicitly approves them.
- `sideEffects` means actions that mutate resources outside the project workspace,
  such as production data, shared services, accounts, or infrastructure. Ordinary
  project-file edits (including README and tests), implementing requested browser
  localStorage behavior, and isolated local verification are not external side
  effects. Do not list these in `sideEffects`. Actually changing existing user
  browser data or other resources outside the isolated test environment still
  requires an explicit side-effect declaration and approval.
- Inspect the supplied codebase context and use real project architecture; do not invent endpoints or frameworks.
- Use only the parent ticket description, acceptance criteria, dependencies, inherited scope boundaries, and codebase context."#,
        id = parent.id,
        level = task_level(parent),
        kind = canonical_task_kind(parent),
        title = parent.title,
        description = parent.description,
        acceptance = acceptance,
        manual_verification = parent.hierarchy.manual_verification.as_ref().unwrap_or(&Value::Null),
        dependencies = task_blockers(parent).join(", "),
        next_level = next_level,
        breakdown_mode = breakdown_mode,
        children = if children.is_empty() {
            "None"
        } else {
            &children
        },
        direct_children = if direct_children.is_empty() {
            "None"
        } else {
            &direct_children
        },
        max_new_children = max_new_children,
        testing_policy = testing_policy,
        scope_boundaries = inherited_scope_constraints_block(run, parent),
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
    )
}

fn scope_exclusion_action_word(word: &str) -> bool {
    matches!(word, "add" | "implement" | "build" | "create" | "introduce"
        | "extend" | "expand" | "include" | "enable" | "ship" | "deploy" | "make"
        | "provide" | "deliver" | "perform" | "execute" | "run" | "exercise"
        | "change" | "modify" | "remove" | "delete" | "update" | "install" | "download")
}

fn scope_exclusion_negative_instruction(instruction: &str) -> bool {
    // A causal explanation is an exclusion only when its governing instruction
    // is itself negative. A nearby "do not" must not hide a new positive verb.
    let Some(action) = ["do not ", "must not ", "never ", "don't "]
        .iter().find_map(|prefix| instruction.trim().strip_prefix(prefix)) else {
        return false;
    };
    if action.chars().any(|ch| matches!(ch, ';' | ':' | '!' | '?' | '&' | '|')) {
        return false;
    }
    let words = action.split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty()).collect::<Vec<_>>();
    if words.len() < 2 || !scope_exclusion_action_word(words[0]) {
        return false;
    }
    !words.iter().enumerate().skip(1).any(|(index, word)| {
        matches!(*word, "but" | "then" | "however" | "instead" | "unless" | "if" | "except")
            || (scope_exclusion_action_word(word)
                // "the Add button" names a control, not a second command.
                && !(*word == "add" && words.get(index + 1) == Some(&"button")
                    && matches!(words[index - 1], "the" | "an")))
    })
}

fn explicit_scope_exclusion_clause(before: &str, after: &str) -> bool {
    // Recognize bounded grammatical exclusions, not whole-sentence examples:
    // subject is/are [explicitly] out of scope [for this local item], optionally
    // reinforced by "and must not be implemented". Reject any other tail.
    let mut tail = after.trim().trim_end_matches(['.', '!', ';']).trim();
    if let Some(context) = tail.strip_prefix("for this ") {
        let mut words = context.splitn(2, char::is_whitespace);
        if !matches!(words.next(), Some("task" | "story" | "ticket" | "feature")) {
            return false;
        }
        tail = words.next().unwrap_or_default().trim();
    }
    if !matches!(tail, "" | "and must not be implemented") {
        return false;
    }
    let before = before.trim_end();
    let Some(mut subject) = [" are explicitly", " is explicitly", " are", " is"]
        .iter()
        .find_map(|suffix| before.strip_suffix(suffix))
        .map(str::trim)
    else {
        return false;
    };
    if let Some((instruction, reason_subject)) = subject.rsplit_once(" because ") {
        if !scope_exclusion_negative_instruction(instruction) {
            return false;
        }
        subject = reason_subject.trim();
    }
    if subject.is_empty() || subject.chars().any(|ch| matches!(ch, '.' | ';' | ':' | '!' | '?')) {
        return false;
    }
    !subject.split(|ch: char| !ch.is_alphanumeric()).any(|word| {
        scope_exclusion_action_word(word)
            || matches!(word, "if" | "unless" | "but" | "then" | "however" | "instead" | "except")
    })
}

fn generated_child_requires_backlog_approval(task: &BoardTask) -> bool {
    if !task.hierarchy.required
        || !task.hierarchy.side_effects.is_empty()
        || task_requires_external_side_effect_declaration(task)
    {
        return true;
    }
    let text = task_external_effect_text(task);
    if ["nice-to-have", "nice to have", "optional"]
        .iter()
        .any(|needle| text.contains(needle))
    {
        return true;
    }
    // A scope boundary can mention excluded work without proposing it. Keep
    // the fallback conservative: exempt only bounded explicit exclusions or
    // the existing elsewhere-responsibility sentence, not the whole task.
    // Structured and external-effect guards above remain authoritative.
    text.lines().flat_map(|line| line.split(';'))
        .flat_map(|clause| clause.split(". ")).any(|sentence| {
        ["out of scope", "out-of-scope"].iter().any(|marker| {
            sentence.match_indices(marker).any(|(index, _)| {
                let (before, after) = (&sentence[..index], &sentence[index + marker.len()..]);
                let boundary =
                (before.trim_start().starts_with("broader ")
                    && before.contains(" remains the responsibility of ")
                    && before.trim_end().ends_with(" and is")
                    && after.trim().trim_end_matches(['.', '!', ';']).trim() == "here")
                    || explicit_scope_exclusion_clause(before, after);
                !boundary
            })
        })
    })
}

fn generated_scope_wrapper_exists(run: &AgenticBoard, source_parent_id: &str, key: &str) -> bool {
    run.tasks.iter().any(|task| {
        task.task_origin == "hierarchy_backlog_wrapper"
            && task.hierarchy.discussion.iter().any(|entry| {
                entry.get("kind").and_then(Value::as_str) == Some("generated_scope_wrapper")
                    && entry.get("sourceParentId").and_then(Value::as_str) == Some(source_parent_id)
                    && entry.get("sourceTitleKey").and_then(Value::as_str) == Some(key)
            })
    })
}

fn generated_hierarchy_wrapper_exists(
    run: &AgenticBoard,
    source_parent_id: &str,
    wrapped_level: &str,
) -> bool {
    run.tasks.iter().any(|task| {
        task.task_origin == "hierarchy_backlog_wrapper"
            && task.hierarchy.discussion.iter().any(|entry| {
                entry.get("kind").and_then(Value::as_str) == Some("generated_scope_wrapper")
                    && entry.get("sourceParentId").and_then(Value::as_str) == Some(source_parent_id)
                    && entry.get("wrappedLevel").and_then(Value::as_str) == Some(wrapped_level)
            })
    })
}

fn mark_generated_scope_wrapper(
    task: &mut BoardTask,
    source_parent: &BoardTask,
    source_title_key: &str,
    wrapped_level: &str,
) {
    task.hierarchy.discussion.push(json!({
        "kind": "generated_scope_wrapper",
        "sourceParentId": source_parent.id,
        "sourceScopeVersion": source_parent.hierarchy.scope_version.max(1),
        "sourceParentTitle": source_parent.title,
        "sourceTitleKey": source_title_key,
        "wrappedLevel": wrapped_level,
        "reason": "risky_or_out_of_scope",
        "createdAt": Utc::now(),
    }));
}

fn wrap_generated_child_in_backlog(
    run: &mut AgenticBoard,
    source_parent: &BoardTask,
    mut child: BoardTask,
    next_level: &str,
    source_title_key: &str,
) -> Vec<BoardTask> {
    let child_title = child.title.clone();
    let child_details = if child.details.trim().is_empty() {
        child.description.clone()
    } else {
        child.details.clone()
    };
    let source_reference = format!(
        "Generated from source parent {}: {}",
        source_parent.id, source_parent.title
    );
    let wrapper_details = format!(
        "Review and approve the generated {next_level} scope before execution.\n\n{child_details}"
    );
    let mut story = BoardTask::draft(
        run,
        format!("Proposed scope: {child_title}"),
        wrapper_details.clone(),
    );
    story.priority = child.priority.clone();
    story.status = TASK_STATUS_BACKLOG.to_string();
    story.task_type = TASK_KIND_DESIGN.to_string();
    story.task_origin = "hierarchy_backlog_wrapper".to_string();
    story.prompt = wrapper_details.clone();
    story.description = wrapper_details.clone();
    story.details = wrapper_details;
    story.acceptance_criteria = child.acceptance_criteria.clone();
    story.references = child.references.clone();
    story.references.push(source_reference.clone());
    story.hierarchy.level = TASK_LEVEL_STORY.to_string();
    story.hierarchy.parent_id = None;
    story.hierarchy.executable = false;
    story.hierarchy.required = child.hierarchy.required;
    story.hierarchy.scope_version = source_parent.hierarchy.scope_version.saturating_add(1);
    story.hierarchy.rank = child.hierarchy.rank;
    story.hierarchy.planned_files = child.hierarchy.planned_files.clone();
    story.hierarchy.side_effects = child.hierarchy.side_effects.clone();
    story.hierarchy.side_effects_approved = false;
    story.hierarchy.side_effect_approval = None;
    story.group_id = Some(story.id.clone());
    mark_generated_scope_wrapper(&mut story, source_parent, source_title_key, next_level);
    let story_id = story.id.clone();

    child.id = unique_task_id(run, &format!("{story_id}-{next_level}"));
    child.status = TASK_STATUS_BACKLOG.to_string();
    child.hierarchy.scope_version = source_parent.hierarchy.scope_version.saturating_add(1);
    child.hierarchy.blocked_by = task_blockers(&child);
    child.depends_on = child.hierarchy.blocked_by.clone();
    child.hierarchy.required = story.hierarchy.required;
    child.hierarchy.side_effects_approved = false;
    child.hierarchy.side_effect_approval = None;
    child.hierarchy.side_effect_evidence.clear();
    child.references.push(source_reference);
    child.task_origin = "hierarchy_breakdown_wrapped".to_string();
    child.prompt = child.description.clone();
    child.group_id = Some(story_id.clone());
    mark_generated_scope_wrapper(&mut child, source_parent, source_title_key, next_level);

    if next_level == TASK_LEVEL_SUBTASK {
        let mut task_wrapper = BoardTask::draft(
            run,
            format!("Planned task: {child_title}"),
            format!(
                "Keep the generated subtask under this approved task scope.\n\n{child_details}"
            ),
        );
        task_wrapper.priority = child.priority.clone();
        task_wrapper.status = TASK_STATUS_BACKLOG.to_string();
        task_wrapper.task_type = TASK_KIND_DESIGN.to_string();
        task_wrapper.task_origin = "hierarchy_backlog_wrapper".to_string();
        task_wrapper.prompt = task_wrapper.description.clone();
        task_wrapper.hierarchy.level = TASK_LEVEL_TASK.to_string();
        task_wrapper.hierarchy.parent_id = Some(story_id.clone());
        task_wrapper.hierarchy.executable = false;
        task_wrapper.hierarchy.required = story.hierarchy.required;
        task_wrapper.hierarchy.scope_version = child.hierarchy.scope_version;
        task_wrapper.hierarchy.rank = child.hierarchy.rank;
        task_wrapper.hierarchy.planned_files = child.hierarchy.planned_files.clone();
        task_wrapper.hierarchy.side_effects = child.hierarchy.side_effects.clone();
        task_wrapper.hierarchy.side_effects_approved = false;
        task_wrapper.hierarchy.side_effect_approval = None;
        task_wrapper.acceptance_criteria = child.acceptance_criteria.clone();
        task_wrapper.references = child.references.clone();
        task_wrapper.group_id = Some(story_id.clone());
        mark_generated_scope_wrapper(
            &mut task_wrapper,
            source_parent,
            source_title_key,
            TASK_LEVEL_TASK,
        );
        child.hierarchy.parent_id = Some(task_wrapper.id.clone());
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.executable = true;
        return vec![story, task_wrapper, child];
    }

    child.hierarchy.parent_id = Some(story_id);
    child.hierarchy.level = next_level.to_string();
    child.hierarchy.executable = false;
    vec![story, child]
}

fn inherit_hierarchy_breakdown_dependencies(child: &mut BoardTask, parent: &BoardTask) {
    if canonical_task_kind(child) == TASK_KIND_MANUAL_TEST
        && canonical_task_kind(parent) == TASK_KIND_MANUAL_TEST
        && parent.hierarchy.manual_verification.is_some()
    {
        child.hierarchy.manual_verification = parent.hierarchy.manual_verification.clone();
    }
    // Dependencies on a planning item must also constrain its executable
    // descendants. Mark inherited server IDs explicitly so they cannot collide
    // with response-local IDs during the remapping pass.
    child.hierarchy.blocked_by = dedupe_strings(
        task_blockers(child)
            .into_iter()
            .chain(
                task_blockers(parent)
                    .into_iter()
                    .map(|id| format!("board:{id}")),
            )
            .collect(),
    );
    child.depends_on = child.hierarchy.blocked_by.clone();
}

fn retained_hierarchy_breakdown_child_id(
    run: &AgenticBoard,
    parent_id: &str,
    next_level: &str,
    title: &str,
) -> Option<String> {
    let mut candidates = run.tasks.iter().filter(|task| {
        task_level(task) == next_level
            && suggested_task_titles_are_semantic_duplicates(&task.title, title)
            && (task.hierarchy.parent_id.as_deref() == Some(parent_id)
                || (task.task_origin == "hierarchy_breakdown_wrapped"
                    && task.hierarchy.discussion.iter().any(|entry| {
                        entry.get("kind").and_then(Value::as_str) == Some("generated_scope_wrapper")
                            && entry.get("sourceParentId").and_then(Value::as_str)
                                == Some(parent_id)
                            && entry.get("wrappedLevel").and_then(Value::as_str) == Some(next_level)
                    })))
    });
    let first = candidates.next()?;
    candidates.next().is_none().then(|| first.id.clone())
}

fn remap_hierarchy_breakdown_dependencies(
    run: &mut AgenticBoard,
    first_new_task: usize,
    local_ids: &BTreeMap<String, String>,
) -> Result<()> {
    let existing_ids = run.tasks[..first_new_task]
        .iter()
        .map(|task| task.id.clone())
        .collect::<BTreeSet<_>>();
    let is_ancestor = |ancestor_id: &str, task: &BoardTask| {
        let mut parent_id = task.hierarchy.parent_id.as_deref();
        let mut visited = BTreeSet::new();
        while let Some(id) = parent_id {
            if id == ancestor_id {
                return true;
            }
            if !visited.insert(id) {
                break;
            }
            parent_id = run
                .tasks
                .iter()
                .find(|candidate| candidate.id == id)
                .and_then(|parent| parent.hierarchy.parent_id.as_deref());
        }
        false
    };
    let mut resolved = Vec::new();
    for task in &run.tasks[first_new_task..] {
        let mut dependencies = Vec::new();
        for reference in task_blockers(task) {
            let dependency = if let Some(local_id) = reference.strip_prefix("plan:") {
                local_ids.get(local_id).cloned()
            } else if let Some(board_id) = reference.strip_prefix("board:") {
                existing_ids.contains(board_id).then(|| board_id.to_string())
            } else {
                // Older providers returned unprefixed IDs. Retain support, but
                // do not guess when an ID can mean either a board item or a
                // different response-local item.
                match (local_ids.get(&reference), existing_ids.contains(&reference)) {
                    (Some(mapped), true) if mapped != &reference => {
                        return Err(invalid_generated_plan(format!(
                            "Ambiguous dependency {reference} for {}; use plan: or board:.",
                            task.id
                        )));
                    }
                    (Some(mapped), _) => Some(mapped.clone()),
                    (None, true) => Some(reference.clone()),
                    (None, false) => {
                        // The earlier prompt had no item IDs, and providers
                        // sometimes returned exact ticket titles as blockers.
                        // Accept only an unambiguous exact match, never a
                        // guessed/fuzzy dependency target.
                        let mut matches = run.tasks.iter().filter(|candidate| {
                            candidate.title.trim() == reference.trim()
                        });
                        matches.next().and_then(|candidate| {
                            matches.next().is_none().then(|| candidate.id.clone())
                        })
                    }
                }
            }
            .ok_or_else(|| {
                invalid_generated_plan(format!(
                    "Dependency {reference} for {} does not match a retained plan item or existing board item.",
                    task.id
                ))
            })?;
            let dependency_task = run
                .tasks
                .iter()
                .find(|candidate| candidate.id == dependency);
            if dependency == task.id
                || is_ancestor(&dependency, task)
                || dependency_task.is_some_and(|candidate| is_ancestor(&task.id, candidate))
            {
                return Err(invalid_generated_plan(format!(
                    "Task {} cannot depend on itself, an ancestor, or a descendant ({dependency}).",
                    task.id
                )));
            }
            dependencies.push(dependency);
        }
        resolved.push(dedupe_strings(dependencies));
    }
    // Resolve the whole batch before changing any task, so invalid references
    // cannot leave a partially rewritten dependency graph.
    for (task, dependencies) in run.tasks[first_new_task..].iter_mut().zip(resolved) {
        task.hierarchy.blocked_by = dependencies.clone();
        task.depends_on = dependencies;
    }
    Ok(())
}

async fn plan_hierarchy_children(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    parent_id: &str,
    manual: bool,
) -> Result<usize> {
    let snapshot = load_user_board(state, user_id, board_id)?.board;
    let Some(parent) = snapshot
        .tasks
        .iter()
        .find(|task| task.id == parent_id)
        .cloned()
    else {
        return Err(not_found("Hierarchy parent not found"));
    };
    let Some(next_level) = next_hierarchy_level(task_level(&parent)) else {
        return Ok(0);
    };
    validate_hierarchy_breakdown_parent(&snapshot, &parent)?;
    let refinement = hierarchy_breakdown_has_children(&snapshot, &parent.id, next_level);
    let prompt = build_hierarchy_breakdown_prompt(&snapshot, &parent, next_level);
    let breakdown_started_at = Utc::now();
    if let Some(failure) = task_package_scope_conflict(&snapshot, &parent) {
        record_hierarchy_breakdown_failure(state, user_id, board_id, &parent, &prompt, breakdown_started_at, &failure, manual)?;
        return Err(bad_request(failure));
    }
    let output = execute_internal_prompt(
        state,
        user_id,
        board_id,
        &format!("hierarchy breakdown for {}", parent.id),
        &prompt,
    )
    .await;
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            let failure = format!(
                "Hierarchy breakdown provider call failed: {}",
                server_error_message(&error)
            );
            record_hierarchy_breakdown_failure(
                state,
                user_id,
                board_id,
                &parent,
                &prompt,
                breakdown_started_at,
                &failure,
                manual,
            )?;
            return Err(ServerError::new(StatusCode::BAD_GATEWAY, failure));
        }
    };
    let output_json = match parse_json_object(&output) {
        Some(output_json) => output_json,
        None => {
            let failure = "Hierarchy breakdown returned malformed JSON instead of the required items contract.";
            record_hierarchy_breakdown_failure(
                state,
                user_id,
                board_id,
                &parent,
                &prompt,
                breakdown_started_at,
                failure,
                manual,
            )?;
            return Err(bad_request(failure));
        }
    };
    let source_items = output_json
        .get("items")
        .or_else(|| output_json.get("tasks"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let returned_empty_items = output_json
        .get("items")
        .or_else(|| output_json.get("tasks"))
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty);
    let source_items = match normalize_hierarchy_manual_contracts(source_items, &parent) {
        Ok(items) => items,
        Err(error) => {
            let failure = server_error_message(&error);
            record_hierarchy_breakdown_failure(state, user_id, board_id, &parent, &prompt, breakdown_started_at, &failure, manual)?;
            return Err(error);
        }
    };
    let children = source_items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let inherits_priority = item
                .get("priority")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_none_or(str::is_empty);
            let inherits_rank = item.get("rank").and_then(Value::as_i64).is_none();
            let mut child = task_from_json(&snapshot, item, index, TASK_STATUS_TODO)?;
            if inherits_priority {
                child.priority = parent.priority.clone();
            }
            if inherits_rank {
                child.hierarchy.rank = index as i64;
            }
            let needs_backlog_approval = generated_child_requires_backlog_approval(&child);
            Some((child, needs_backlog_approval))
        })
        .collect::<Vec<_>>();
    let mut returned_ids = BTreeSet::new();
    for (child, _) in &children {
        if !returned_ids.insert(child.id.clone()) {
            let failure = format!(
                "Hierarchy breakdown returned duplicate item id: {}.",
                child.id
            );
            record_hierarchy_breakdown_failure(
                state,
                user_id,
                board_id,
                &parent,
                &prompt,
                breakdown_started_at,
                &failure,
                manual,
            )?;
            return Err(bad_request(failure));
        }
    }
    if children.is_empty() && (!refinement || !returned_empty_items) {
        let failure = if refinement {
            "Hierarchy breakdown refinement returned no usable child items."
        } else {
            "Hierarchy breakdown returned no usable child items."
        };
        record_hierarchy_breakdown_failure(
            state,
            user_id,
            board_id,
            &parent,
            &prompt,
            breakdown_started_at,
            failure,
            manual,
        )?;
        return Err(bad_request(failure));
    }

    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, board_id)?;
    let current_parent = stored
        .board
        .tasks
        .iter()
        .find(|task| task.id == parent.id)
        .cloned()
        .ok_or_else(|| not_found("Hierarchy parent no longer exists"))?;
    if current_parent.hierarchy.scope_version != parent.hierarchy.scope_version
        || canonical_task_status(&current_parent.status) != canonical_task_status(&parent.status)
        || current_parent.title != parent.title
        || current_parent.description != parent.description
        || current_parent.acceptance_criteria != parent.acceptance_criteria
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The parent scope changed while breakdown was running; discard this result and run Breakdown again.",
        ));
    }
    let child_status = match canonical_task_status(&current_parent.status) {
        TASK_STATUS_BACKLOG => TASK_STATUS_BACKLOG,
        TASK_STATUS_TODO => TASK_STATUS_TODO,
        _ => {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "Hierarchy breakdown requires the parent to remain in Backlog or Todo.",
            ));
        }
    };
    // Validate every candidate against the current lineage before persisting any
    // of them. A generated test-tooling plan cannot grant itself broader scope.
    if let Some(failure) = children.iter().find_map(|(child, _)| {
        hierarchy_package_scope_conflict(&stored.board, &current_parent, child)
    }) {
        drop(_guard);
        record_hierarchy_breakdown_failure(state, user_id, board_id, &parent, &prompt, breakdown_started_at, &failure, manual)?;
        return Err(bad_request(failure));
    }
    let provider = effective_provider_for_phase(&snapshot, "hierarchy breakdown")
        .unwrap_or_else(|_| snapshot.provider.clone());
    let model = effective_model_for_phase(&snapshot, "hierarchy breakdown");
    let root_group = stored
        .board
        .tasks
        .iter()
        .find(|task| task.id == parent.id)
        .map(task_group_id_or_self)
        .unwrap_or_else(|| parent.id.clone());
    let existing_titles =
        hierarchy_breakdown_child_title_candidates(&stored.board, &parent.id, next_level);
    let existing_child_count = existing_titles.len();
    let max_new_children = hierarchy_breakdown_max_new_children(existing_child_count);
    let mut seen_titles = existing_titles;
    let mut created = 0usize;
    let candidate_count = children.len();
    let mut reused = 0usize;
    let first_new_task = stored.board.tasks.len();
    let mut local_ids = BTreeMap::new();
    for (mut child, needs_backlog_approval) in children {
        let key = normalize_suggested_task_key(&child.title);
        if hierarchy_breakdown_title_is_duplicate(&seen_titles, &child.title) {
            if let Some(retained_id) = retained_hierarchy_breakdown_child_id(
                &stored.board,
                &parent.id,
                next_level,
                &child.title,
            ) {
                local_ids.insert(child.id.clone(), retained_id);
            }
            reused += 1;
            continue;
        }
        if created >= max_new_children || key.is_empty() {
            reused += 1;
            continue;
        }
        // Keep one semantic title key for both ordinary and wrapped children.
        // A provider can return the same work with different wording or once
        // as a wrapped story and once as a normal child; neither is duplicated.
        seen_titles.push(child.title.clone());
        let local_id = child.id.clone();
        inherit_hierarchy_breakdown_dependencies(&mut child, &current_parent);
        if needs_backlog_approval {
            let wrapped = wrap_generated_child_in_backlog(
                &mut stored.board,
                &current_parent,
                child,
                next_level,
                &key,
            );
            if let Some(wrapped_child) = wrapped.last() {
                local_ids.insert(local_id, wrapped_child.id.clone());
            }
            stored.board.tasks.extend(wrapped);
            created += 1;
            continue;
        }
        child.id = unique_task_id(&stored.board, &format!("{}-{}", parent.id, child.id));
        local_ids.insert(local_id, child.id.clone());
        child.hierarchy.level = next_level.to_string();
        child.hierarchy.parent_id = Some(parent.id.clone());
        child.hierarchy.executable = next_level == TASK_LEVEL_SUBTASK;
        child.hierarchy.scope_version = parent.hierarchy.scope_version.saturating_add(1);
        child.hierarchy.blocked_by = task_blockers(&child);
        child.depends_on = child.hierarchy.blocked_by.clone();
        child.group_id = Some(root_group.clone());
        child.status = if child_status == TASK_STATUS_TODO && needs_backlog_approval {
            TASK_STATUS_BACKLOG.to_string()
        } else {
            child_status.to_string()
        };
        child.task_origin = "hierarchy_breakdown".to_string();
        child.prompt = parent.description.clone();
        stored.board.tasks.push(child);
        created += 1;
    }
    if let Err(error) =
        remap_hierarchy_breakdown_dependencies(&mut stored.board, first_new_task, &local_ids)
    {
        let failure = format!(
            "Hierarchy breakdown returned invalid dependencies: {}",
            server_error_message(&error)
        );
        // Do not persist the candidate children with invalid edges. The
        // failure recorder reloads the unchanged board and acquires its own
        // mutation lock, leaving a visible retryable planning error.
        drop(_guard);
        record_hierarchy_breakdown_failure(
            state,
            user_id,
            board_id,
            &parent,
            &prompt,
            breakdown_started_at,
            &failure,
            manual,
        )?;
        return Err(bad_request(failure));
    }
    if created == 0 {
        if refinement || (candidate_count > 0 && reused == candidate_count) {
            let summary = if existing_child_count >= MAX_HIERARCHY_CHILDREN_PER_PARENT {
                format!(
                    "Breakdown checked; child limit reached ({MAX_HIERARCHY_CHILDREN_PER_PARENT})"
                )
            } else {
                format!("Breakdown checked; no new {next_level} child ticket(s) needed")
            };
            if let Some(parent_task) = stored
                .board
                .tasks
                .iter_mut()
                .find(|task| task.id == parent.id)
            {
                parent_task.error = None;
                parent_task.summary = summary.clone();
                append_hierarchy_breakdown_transcript(
                    parent_task,
                    breakdown_started_at,
                    &provider,
                    &model,
                    &prompt,
                    &output,
                );
            }
            stored.board.append_log(format!(
                "Hierarchy breakdown checked {parent_id}; no new {next_level} child ticket(s) added"
            ));
            refresh_hierarchy_rollups(&mut stored.board);
            stored.board.touch();
            save_board(state, &stored.board)?;
            return Ok(0);
        }
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!("No new {next_level} children could be created for {parent_id}"),
        ));
    }
    if let Some(issue) = hierarchy_validation_issues(&stored.board)
        .into_iter()
        .next()
    {
        let affected = planning_error_task_ids(&stored.board, &issue);
        let error = planning_error_conflict(&mut stored.board, &affected, "hierarchy", issue);
        stored.board.touch();
        save_board(state, &stored.board)?;
        return Err(error);
    }
    if let Some(parent_task) = stored
        .board
        .tasks
        .iter_mut()
        .find(|task| task.id == parent.id)
    {
        parent_task.error = None;
        parent_task.summary = if refinement {
            format!("Breakdown refined; added {created} {next_level} child ticket(s)")
        } else {
            format!("Generated {created} {next_level} child ticket(s)")
        };
    }
    if let Some(cycle) = dependency_cycle(&stored.board) {
        let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
        let error = planning_error_conflict(&mut stored.board, &cycle, "dependency", issue);
        stored.board.touch();
        save_board(state, &stored.board)?;
        return Err(error);
    }
    if refinement {
        stored.board.append_log(format!(
            "Hierarchy breakdown refined {parent_id}; added {created} {next_level} child ticket(s)"
        ));
    } else {
        stored.board.append_log(format!(
            "Hierarchy breakdown created {created} {next_level} child ticket(s) under {parent_id}"
        ));
    }
    if let Some(parent_task) = stored
        .board
        .tasks
        .iter_mut()
        .find(|task| task.id == parent.id)
    {
        append_hierarchy_breakdown_transcript(
            parent_task,
            breakdown_started_at,
            &provider,
            &model,
            &prompt,
            &output,
        );
    }
    refresh_hierarchy_rollups(&mut stored.board);
    stored.board.touch();
    save_board(state, &stored.board)?;
    Ok(created)
}

fn append_hierarchy_breakdown_transcript(
    task: &mut BoardTask,
    started_at: DateTime<Utc>,
    provider: &str,
    model: &str,
    prompt: &str,
    output: &str,
) {
    task.transcript.push(redact_transcript_value(&json!({
        "timestamp": started_at,
        "role": "user",
        "kind": "user",
        "provider": provider,
        "model": model,
        "content": prompt,
    })));
    task.transcript.push(redact_transcript_value(&json!({
        "timestamp": Utc::now(),
        "role": "assistant",
        "kind": "assistant",
        "provider": provider,
        "model": model,
        "content": output,
    })));
    task.transcript_updated_at = Some(Utc::now());
}

fn record_hierarchy_breakdown_failure(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    parent: &BoardTask,
    prompt: &str,
    started_at: DateTime<Utc>,
    failure: &str,
    manual: bool,
) -> Result<()> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, board_id)?;
    // A breakdown is a user-invoked planning operation and commonly runs
    // while the board is paused. Persist its failure even in that state so a
    // provider/contract error cannot leave an apparently healthy planning
    // item with no retry or attention signal. Terminal boards are immutable.
    if matches!(stored.board.status.as_str(), "cancelled" | "completed") {
        return Ok(());
    }
    if stored.board.provider_call_label.as_deref()
        == Some(format!("hierarchy breakdown for {}", parent.id).as_str())
    {
        stored.board.provider_call_started_at = None;
        stored.board.provider_call_label = None;
        stored.board.current_provider_session_id = None;
    }
    let provider = effective_provider_for_phase(&stored.board, "hierarchy breakdown")
        .unwrap_or_else(|_| stored.board.provider.clone());
    let model = effective_model_for_phase(&stored.board, "hierarchy breakdown");
    if let Some(task) = stored
        .board
        .tasks
        .iter_mut()
        .find(|task| task.id == parent.id)
    {
        // Manual Breakdown is a planning action. A provider/contract
        // failure must remain retryable and must not masquerade as a real
        // dependency blocker. Automatic hierarchy planning retains the
        // existing fail-closed behavior so the worker cannot spin forever.
        if !manual {
            task.status = TASK_STATUS_BLOCKED.to_string();
        }
        task.error = Some(failure.to_string());
        task.summary = failure.to_string();
        task.transcript.push(redact_transcript_value(&json!({
            "timestamp": started_at,
            "role": "user",
            "kind": "user",
            "provider": provider.clone(),
            "model": model.clone(),
            "content": prompt,
        })));
        task.transcript.push(redact_transcript_value(&json!({
            "timestamp": Utc::now(),
            "role": "system",
            "kind": "error",
            "provider": provider,
            "model": model,
            "content": failure,
        })));
        task.transcript_updated_at = Some(Utc::now());
    }
    if manual {
        stored.board.append_log(format!(
            "Manual hierarchy breakdown failed for {}; item remains retryable: {}",
            parent.id, failure
        ));
    } else {
        mark_planning_error(
            &mut stored.board,
            std::slice::from_ref(&parent.id),
            "hierarchy",
            failure,
        );
        if let Some(details) = stored
            .board
            .phase_details
            .as_mut()
            .and_then(Value::as_object_mut)
        {
            details.insert("parentId".to_string(), json!(parent.id));
            details.insert(
                "retry".to_string(),
                json!("Move the planning item to Todo and run Breakdown again after the provider or contract issue is fixed."),
            );
            details.insert("startedAt".to_string(), json!(started_at));
        }
    }
    stored.board.touch();
    save_board(state, &stored.board)
}

fn is_retryable_hierarchy_breakdown_message(message: &str) -> bool {
    message
        .trim()
        .strip_prefix("Planning error: ")
        .unwrap_or_else(|| message.trim())
        .starts_with("Hierarchy breakdown ")
}

fn is_retryable_hierarchy_breakdown_task(task: &BoardTask) -> bool {
    task.error
        .as_deref()
        .is_some_and(is_retryable_hierarchy_breakdown_message)
        || is_retryable_hierarchy_breakdown_message(&task.summary)
}

fn hierarchy_breakdown_planning_error_for(run: &AgenticBoard, task_id: &str) -> bool {
    run.current_phase.as_deref() == Some(PLANNING_ERROR_PHASE)
        && run
            .phase_details
            .as_ref()
            .and_then(|details| details.get("kind"))
            .and_then(Value::as_str)
            == Some("hierarchy")
        && run
            .phase_details
            .as_ref()
            .and_then(|details| details.get("parentId"))
            .and_then(Value::as_str)
            == Some(task_id)
        && run
            .phase_details
            .as_ref()
            .and_then(|details| details.get("error"))
            .and_then(Value::as_str)
            .is_some_and(is_retryable_hierarchy_breakdown_message)
}

fn restore_board_after_hierarchy_breakdown_failure(run: &mut AgenticBoard, task_id: &str) -> bool {
    if !hierarchy_breakdown_planning_error_for(run, task_id) {
        return false;
    }
    run.status = "paused".to_string();
    run.active = false;
    run.loop_started = false;
    run.auto_run_enabled = false;
    run.pause_requested = false;
    run.paused_at = Some(Utc::now());
    run.current_task_id = None;
    run.current_task_title.clear();
    run.current_task_status.clear();
    run.current_phase = Some("board".to_string());
    run.phase_started_at = Some(Utc::now());
    run.phase_details = Some(json!({
        "mode": "kanban_only",
        "breakdownRetryable": true,
        "parentId": task_id,
    }));
    run.pause_reason = Some("Manual hierarchy breakdown is available.".to_string());
    run.append_log(format!(
        "Restored board to paused planning state after hierarchy breakdown failure for {task_id}"
    ));
    true
}

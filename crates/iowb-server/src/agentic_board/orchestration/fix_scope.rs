fn is_automatic_qa_fix(task: &BoardTask) -> bool {
    task.qa_fix_task
        || matches!(
            task.task_origin.as_str(),
            "system_qa_fix" | "system_followup"
        )
}

// Follow only persisted system provenance, never a model's description or
// suggested files. Unknown/cyclic lineage is not permission to write code.
fn automatic_qa_fix_origin<'a>(run: &'a AgenticBoard, task: &BoardTask) -> Option<&'a BoardTask> {
    let mut source_id = task.source_task_id.as_deref()?;
    let mut seen = BTreeSet::from([task.id.as_str()]);
    loop {
        if !seen.insert(source_id) {
            return None;
        }
        let source = run.tasks.iter().find(|item| item.id == source_id)?;
        if source.superseded_by.is_some()
            || source.hierarchy.scope_version != task.hierarchy.scope_version
            || source.hierarchy.parent_id != task.hierarchy.parent_id
        {
            return None;
        }
        if is_automatic_qa_fix(source)
            || (is_qa_task(source)
                && matches!(
                    source.task_origin.as_str(),
                    "system_qa" | "system_qa_verdict_retry"
                ))
        {
            source_id = source.source_task_id.as_deref()?;
            continue;
        }
        return Some(source);
    }
}

fn exact_scope_path(path: &str) -> Option<&str> {
    let path = path.trim().strip_prefix("./").unwrap_or(path.trim());
    (!path.is_empty()
        && !path.contains('\\')
        && !path.contains(['*', '?', '[', ']'])
        && path.split('/').all(|part| !matches!(part, "" | "." | "..")))
    .then_some(path)
}

fn automatic_qa_fix_scope_is_authorized(run: &AgenticBoard, task: &BoardTask) -> bool {
    if !uses_hierarchical_orchestration(run) || !is_automatic_qa_fix(task) {
        return true;
    }
    let test_repairs = approved_test_repair_paths(run, task);
    if task.hierarchy.planned_files.iter().any(|path| {
        is_test_or_fixture_path(path)
            && !exact_scope_path(path)
                .is_some_and(|path| test_repairs.iter().any(|allowed| allowed == path))
    }) {
        return false;
    }
    if current_human_reviewed_execution_scope(run, task) {
        return !task.hierarchy.planned_files.is_empty()
            && task
                .hierarchy
                .planned_files
                .iter()
                .all(|path| exact_scope_path(path).is_some());
    }
    let Some(origin) = automatic_qa_fix_origin(run, task) else {
        return false;
    };
    // A test-only source and a read-only QA parent cannot authorize production
    // work. Those fixes must be reviewed as a new leaf scope first.
    matches!(
        canonical_task_kind(origin),
        TASK_KIND_IMPLEMENTATION | TASK_KIND_FIX | TASK_KIND_REVISION
    ) && task_status_is_done(&origin.status)
        && task_is_executable(origin)
        && task_ancestors_are_approved(run, origin)
        && origin
            .hierarchy
            .scope_approval
            .as_ref()
            .is_some_and(|approval| {
                approval.approved && approval.scope_version == origin.hierarchy.scope_version
            })
        && !task.hierarchy.planned_files.is_empty()
        && task.hierarchy.planned_files.iter().all(|path| {
            exact_scope_path(path).is_some_and(|path| {
                origin
                    .hierarchy
                    .planned_files
                    .iter()
                    .any(|original| exact_scope_path(original) == Some(path))
            })
        })
}

fn automatic_qa_fix_file_violations(
    run: &AgenticBoard,
    task: &BoardTask,
    change_summary: &Value,
) -> Vec<String> {
    if !uses_hierarchical_orchestration(run) || !is_automatic_qa_fix(task) {
        return Vec::new();
    }
    let touched = change_summary_touched_paths(change_summary);
    if !automatic_qa_fix_scope_is_authorized(run, task) {
        return touched;
    }
    let test_repairs = approved_test_repair_paths(run, task);
    touched
        .into_iter()
        .filter(|path| {
            let Some(path) = exact_scope_path(path) else {
                return true;
            };
            !task
                .hierarchy
                .planned_files
                .iter()
                .any(|planned| exact_scope_path(planned) == Some(path))
                || (is_test_or_fixture_path(path)
                    && !test_repairs.iter().any(|allowed| allowed == path))
        })
        .collect()
}

fn automatic_qa_fix_scope_instruction(run: &AgenticBoard, task: &BoardTask) -> String {
    if !uses_hierarchical_orchestration(run) || !is_automatic_qa_fix(task) {
        return String::new();
    }
    format!(
        "\n\nAutomatic QA-fix scope boundary:\n\
         - Edit only these exact approved planned files: {}. Do not create or change files outside that list.\n\
         - Test-only or read-only work does not authorize production implementation. A failed test is not permission to expand scope.\n\
         - Do not edit any test or fixture, even to fix syntax or harness conventions, unless this leaf has the explicit current human-applied Approved test repair permission for that exact file.\n\
         - If the repair requires another file or a semantic test change not approved here, report blocked and request a reviewed Edit/Replace scope; do not make that change.",
        task.hierarchy.planned_files.join(", "),
    )
}

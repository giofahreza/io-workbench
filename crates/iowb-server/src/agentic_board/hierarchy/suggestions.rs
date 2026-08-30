fn normalize_suggested_task_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn suggested_task_title_tokens(value: &str) -> BTreeSet<String> {
    let normalized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>();

    normalized
        .split_whitespace()
        .filter(|token| {
            !matches!(
                *token,
                "a" | "an"
                    | "and"
                    | "as"
                    | "at"
                    | "by"
                    | "for"
                    | "from"
                    | "in"
                    | "into"
                    | "of"
                    | "on"
                    | "or"
                    | "the"
                    | "to"
                    | "under"
                    | "using"
                    | "via"
                    | "with"
            )
        })
        .map(canonical_suggested_task_token)
        .collect()
}

fn canonical_suggested_task_token(token: &str) -> String {
    let canonical = match token {
        "add" | "adds" | "added" | "create" | "creates" | "created" | "implement"
        | "implements" | "implemented" | "implementation" => "implement",
        "persist" | "persists" | "persisted" | "save" | "saves" | "saved" | "storage" | "store"
        | "stores" | "stored" => "persist",
        "validate" | "validates" | "validated" | "validation" | "validating" => "validate",
        "test" | "tests" | "tested" | "testing" => "test",
        "execute" | "executes" | "executed" | "execution" | "run" | "runs" | "running" => "run",
        "build" | "builds" | "built" | "building" => "build",
        "configure" | "configures" | "configured" | "configuration" => "configure",
        "update" | "updates" | "updated" | "updating" => "update",
        "remove" | "removes" | "removed" | "removing" => "remove",
        "delete" | "deletes" | "deleted" | "deleting" => "delete",
        "fix" | "fixes" | "fixed" | "fixing" => "fix",
        "task" | "tasks" => "task",
        "file" | "files" => "file",
        _ => token,
    };
    canonical.to_string()
}

fn suggested_task_titles_are_semantic_duplicates(left: &str, right: &str) -> bool {
    let left_key = normalize_suggested_task_key(left);
    let right_key = normalize_suggested_task_key(right);
    if left_key.is_empty() || right_key.is_empty() {
        return false;
    }
    if left_key == right_key {
        return true;
    }

    let left_tokens = suggested_task_title_tokens(left);
    let right_tokens = suggested_task_title_tokens(right);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return false;
    }

    let overlap = left_tokens.intersection(&right_tokens).count();
    let smaller = left_tokens.len().min(right_tokens.len());
    let union = left_tokens.union(&right_tokens).count();
    overlap >= 3 && overlap * 100 >= smaller * 80 && overlap * 100 >= union * 60
}

fn hierarchy_breakdown_title_is_duplicate(existing_titles: &[String], candidate: &str) -> bool {
    existing_titles
        .iter()
        .any(|existing| suggested_task_titles_are_semantic_duplicates(existing, candidate))
}

fn append_suggested_backlog_tasks_from_result(
    run: &mut AgenticBoard,
    source_task_id: &str,
    parsed: &Value,
) -> Vec<String> {
    let suggestions = parsed
        .get("suggestedBacklogTasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if suggestions.is_empty() {
        return Vec::new();
    }
    let Some(source_task) = run
        .tasks
        .iter()
        .find(|task| task.id == source_task_id)
        .cloned()
    else {
        return Vec::new();
    };
    if canonical_task_kind(&source_task) == TASK_KIND_RESEARCH {
        return Vec::new();
    }
    let mut existing_scope_keys = run
        .tasks
        .iter()
        .filter_map(|task| {
            let title_key = normalize_suggested_task_key(&task.title);
            if title_key.is_empty() {
                return None;
            }
            let parent_id = task_parent_id(task).unwrap_or_default();
            let level = task_level(task);
            if parent_id.is_empty()
                && !matches!(
                    level,
                    TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC | TASK_LEVEL_STORY
                )
            {
                return None;
            }
            Some(suggested_scope_key(parent_id, level, &title_key))
        })
        .collect::<BTreeSet<_>>();
    let mut created = Vec::new();
    for suggestion in suggestions {
        let requested_level = normalize_task_level(
            suggestion.get("level").and_then(Value::as_str),
            TASK_LEVEL_STORY,
        );
        let requested_parent_id = suggestion
            .get("parentId")
            .or_else(|| suggestion.get("parent_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        let title = ["title", "details", "description", "summary"]
            .into_iter()
            .filter_map(|key| suggestion.get(key).and_then(Value::as_str))
            .map(str::trim)
            .find(|value| !value.is_empty())
            .map(str::to_string);
        let Some(title) = title else {
            continue;
        };
        let key = normalize_suggested_task_key(&title);
        let target_level = requested_parent_id
            .is_empty()
            .then_some(match requested_level {
                TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK => TASK_LEVEL_STORY,
                level => level,
            })
            .or_else(|| {
                run.tasks
                    .iter()
                    .find(|task| task.id == requested_parent_id)
                    .and_then(|parent| next_hierarchy_level(task_level(parent)))
            })
            .unwrap_or(requested_level);
        let scope_key = suggested_scope_key(requested_parent_id, target_level, &key);
        if key.is_empty() || !existing_scope_keys.insert(scope_key) {
            continue;
        }
        let details = suggestion
            .get("details")
            .or_else(|| suggestion.get("description"))
            .or_else(|| suggestion.get("summary"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&title)
            .to_string();
        let mut task = BoardTask::manual(
            run,
            TaskRequest {
                prompt: Some(details.clone()),
                command: None,
                title: Some(title),
                details: Some(details),
                description: None,
                kind: suggestion
                    .get("kind")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                task_type: suggestion
                    .get("taskType")
                    .or_else(|| suggestion.get("task_type"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                level: suggestion
                    .get("level")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                parent_id: suggestion
                    .get("parentId")
                    .or_else(|| suggestion.get("parent_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                blocked_by: suggestion
                    .get("blockedBy")
                    .or_else(|| suggestion.get("blocked_by"))
                    .cloned(),
                executable: suggestion.get("executable").and_then(Value::as_bool),
                required: suggestion.get("required").and_then(Value::as_bool),
                source_task_id: suggestion
                    .get("sourceTaskId")
                    .or_else(|| suggestion.get("source_task_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                scope_version: suggestion.get("scopeVersion").and_then(Value::as_u64),
                planned_files: suggestion
                    .get("plannedFiles")
                    .or_else(|| suggestion.get("files"))
                    .cloned(),
                side_effects: suggestion.get("sideEffects").cloned(),
                acceptance_criteria: suggestion.get("acceptanceCriteria").cloned(),
                acceptance: suggestion.get("acceptance").cloned(),
                criteria: suggestion.get("criteria").cloned(),
                references: suggestion.get("references").cloned(),
                files: suggestion.get("files").cloned(),
                paths: suggestion.get("paths").cloned(),
                priority: suggestion
                    .get("priority")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                rank: suggestion.get("rank").and_then(Value::as_i64),
                depends_on: suggestion.get("dependsOn").cloned(),
                dependencies: suggestion.get("dependencies").cloned(),
                status: Some("backlog".to_string()),
            },
        )
        .expect("suggested backlog task has a title");
        task.references.insert(
            0,
            format!(
                "Suggested backlog task from {}: {}",
                source_task.id, source_task.title
            ),
        );
        task.task_origin = "ai_suggested_backlog".to_string();
        task.manual_task = false;
        task.prompt_task = false;
        task.task_type = prompt_task_kind_from_value(
            &suggestion,
            &task.title,
            &task.details,
            &task.acceptance_criteria,
        )
        .to_string();
        // BoardTask::manual intentionally normalizes a top-level task/subtask
        // to a visible story. Keep the model's original level here so a
        // result suggestion cannot accidentally bypass the required story
        // wrapper before it reaches the Backlog.
        let generated_level = if task.hierarchy.parent_id.is_none()
            && matches!(requested_level, TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK)
        {
            requested_level.to_string()
        } else {
            task_level(&task).to_string()
        };
        let needs_backlog_wrapper = generated_child_requires_backlog_approval(&task)
            || (task.hierarchy.parent_id.is_none()
                && matches!(
                    generated_level.as_str(),
                    TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK
                ));
        if needs_backlog_wrapper
            && matches!(
                generated_level.as_str(),
                TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK
            )
        {
            let source_title_key = normalize_suggested_task_key(&task.title);
            let wrapped = wrap_generated_child_in_backlog(
                run,
                &source_task,
                task,
                &generated_level,
                &source_title_key,
            );
            let wrapper_id = wrapped.first().map(|task| task.id.clone());
            run.tasks.extend(wrapped);
            if let Some(wrapper_id) = wrapper_id {
                created.push(wrapper_id);
            }
            continue;
        }
        let task_id = task.id.clone();
        run.tasks.push(task);
        created.push(task_id);
    }
    if !created.is_empty() {
        run.append_log(format!(
            "Added {} suggested backlog task(s) from {}: {}",
            created.len(),
            source_task_id,
            created.join(", ")
        ));
    }
    created
}

fn suggested_scope_key(parent_id: &str, level: &str, title_key: &str) -> String {
    format!("{}|{}|{}", parent_id, level, title_key)
}

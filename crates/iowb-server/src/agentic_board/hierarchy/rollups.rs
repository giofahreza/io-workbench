fn refresh_hierarchy_rollups(run: &mut AgenticBoard) {
    let mut parent_ids = run
        .tasks
        .iter()
        .filter(|task| !task_is_executable(task))
        .map(|task| {
            let mut depth = 0usize;
            let mut current = task.hierarchy.parent_id.as_deref();
            let mut seen = BTreeSet::new();
            while let Some(parent_id) = current {
                if !seen.insert(parent_id) {
                    break;
                }
                depth = depth.saturating_add(1);
                current = run
                    .tasks
                    .iter()
                    .find(|candidate| candidate.id == parent_id)
                    .and_then(|parent| parent.hierarchy.parent_id.as_deref());
            }
            (task.id.clone(), depth)
        })
        .collect::<Vec<_>>();
    parent_ids.sort_by_key(|(_, depth)| std::cmp::Reverse(*depth));
    for (parent_id, _) in parent_ids {
        let children = run
            .tasks
            .iter()
            .filter(|task| task.hierarchy.parent_id.as_deref() == Some(parent_id.as_str()))
            .collect::<Vec<_>>();
        if children.is_empty() {
            continue;
        }
        let required_children = children
            .iter()
            .copied()
            .filter(|task| task.hierarchy.required)
            .collect::<Vec<_>>();
        let optional_only = required_children.is_empty();
        // Optional children never block a parent that has required work. If a
        // parent has only optional children, still derive a useful status from
        // them so a completed optional-only plan does not leave its parent
        // indefinitely in Todo. The optional work remains independently
        // approvable and executable.
        let relevant_children = if optional_only {
            children.clone()
        } else {
            required_children.clone()
        };
        let all_done = relevant_children
            .iter()
            .all(|task| task_rollup_completion_is_satisfied(task));
        let has_eligible_child = relevant_children
            .iter()
            .any(|task| task_rollup_child_is_eligible(run, task));
        let has_active_child = relevant_children.iter().any(|task| {
            task_status_is_active(&task.status) && task_ancestors_are_approved(run, task)
        });
        let all_remaining_blocked = relevant_children
            .iter()
            .filter(|task| !task_rollup_completion_is_satisfied(task))
            .all(|task| task_rollup_child_is_blocked(run, task));
        let all_backlog = relevant_children
            .iter()
            .all(|task| task_status_is_backlog(&task.status));
        let parent = run.tasks.iter().find(|task| task.id == parent_id);
        let parent_was_approved =
            parent.is_some_and(|parent| !task_status_is_backlog(&parent.status));
        let parent_was_done = parent.is_some_and(|parent| task_status_is_done(&parent.status));
        let parent_attention_status = parent
            .filter(|parent| {
                matches!(
                    canonical_task_status(&parent.status),
                    TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
                ) && parent
                    .error
                    .as_deref()
                    .is_some_and(|error| !error.trim().is_empty())
            })
            .map(|parent| canonical_task_status(&parent.status));
        let next_status = if optional_only && parent_was_done {
            // Optional work is independently approvable and must not silently
            // reopen a completed required scope when the user later moves the
            // nice-to-have child to Todo.
            TASK_STATUS_DONE
        } else if all_done {
            TASK_STATUS_DONE
        } else if has_active_child || has_eligible_child {
            TASK_STATUS_IN_PROGRESS
        } else if !optional_only && all_remaining_blocked {
            TASK_STATUS_BLOCKED
        } else if all_backlog {
            // A parent moved to Todo is already an approval decision. Keep
            // that approval while its manually authored children are still
            // waiting in Backlog; otherwise a rollup would silently revoke
            // the user's approval.
            if let Some(status) = parent_attention_status {
                status
            } else if optional_only && parent_was_approved {
                // No required path remains. A backlog nice-to-have must not
                // keep an approved parent open or turn into implicit work.
                TASK_STATUS_DONE
            } else if parent_was_approved {
                TASK_STATUS_TODO
            } else {
                TASK_STATUS_BACKLOG
            }
        } else if optional_only {
            // Optional work is not a required path. If it is neither ready
            // nor running, the parent can still be complete; any blocked or
            // failed optional item remains visible in its own group/task.
            parent_attention_status.unwrap_or(TASK_STATUS_DONE)
        } else {
            TASK_STATUS_TODO
        };
        if let Some(parent) = run.tasks.iter_mut().find(|task| task.id == parent_id) {
            parent.status = next_status.to_string();
            if matches!(next_status, TASK_STATUS_IN_PROGRESS | TASK_STATUS_TODO)
                && parent
                    .error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("Planning error:"))
            {
                parent.error = None;
            } else if next_status == TASK_STATUS_DONE {
                parent.error = None;
            }
        }
    }
}

fn task_rollup_completion_is_satisfied(task: &BoardTask) -> bool {
    task_status_is_done(&task.status)
        && !(canonical_task_kind(task) == TASK_KIND_RESEARCH && !task.hierarchy.research_accepted)
}

fn task_rollup_child_is_eligible(run: &AgenticBoard, task: &BoardTask) -> bool {
    if !task_status_is_todo(&task.status) || !task_ancestors_are_approved(run, task) {
        return false;
    }
    if !unmet_task_dependencies(run, task).is_empty() {
        return false;
    }
    if task_is_executable(task) {
        task_side_effects_are_approved(task)
            && (!has_pending_research_acceptance(run)
                || canonical_task_kind(task) == TASK_KIND_RESEARCH)
    } else {
        true
    }
}

fn task_rollup_child_is_blocked(run: &AgenticBoard, task: &BoardTask) -> bool {
    match canonical_task_status(&task.status) {
        TASK_STATUS_BLOCKED | TASK_STATUS_FAILED => true,
        TASK_STATUS_DONE => {
            canonical_task_kind(task) == TASK_KIND_RESEARCH && !task.hierarchy.research_accepted
        }
        TASK_STATUS_TODO => !task_rollup_child_is_eligible(run, task),
        _ => false,
    }
}

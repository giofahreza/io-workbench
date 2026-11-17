fn refresh_hierarchy_rollups(run: &mut AgenticBoard) {
    // Use the approval records present at entry as an extra per-pass defense.
    // This is not a new authorization-provenance store: existing normalization
    // intentionally treats Todo/InProgress/Done as approval on later calls.
    // Canonically unapproved Backlog and stale wrapper scope stay ineligible.
    let explicitly_approved = run.tasks.iter().filter(|task| {
        task.hierarchy.scope_approval.as_ref().is_some_and(|approval| {
            approval.approved && approval.scope_version == task.hierarchy.scope_version.max(1)
        })
    }).map(|task| task.id.clone()).collect::<BTreeSet<_>>();
    // Every extra pass completes at least one previously unfinished orphan;
    // leaves, approval records, scope and evidence are never rewritten here.
    for _ in 0..=run.tasks.len() {
        refresh_direct_hierarchy_rollups(run);
        if !complete_verified_wrapped_orphans(run, &explicitly_approved) {
            break;
        }
    }
}

fn wrapper_matches_current_source(wrapper: &BoardTask, source: &BoardTask) -> bool {
    if wrapper.task_origin != "hierarchy_backlog_wrapper"
        || task_level(wrapper) != TASK_LEVEL_STORY
        || wrapper.hierarchy.parent_id.is_some()
    {
        return false;
    }
    current_wrapped_scope_provenance(wrapper, source).is_some()
}

fn current_wrapped_scope_provenance<'a>(task: &'a BoardTask, source: &BoardTask) -> Option<&'a Value> {
    if task.hierarchy.scope_version != source.hierarchy.scope_version.saturating_add(1)
        || task.hierarchy.discussion.iter().any(|entry| entry.get("kind").and_then(Value::as_str) == Some("scope_edit"))
    {
        return None;
    }
    task.hierarchy.discussion.iter().find(|entry| {
        entry.get("kind").and_then(Value::as_str) == Some("generated_scope_wrapper")
            && entry.get("sourceParentId").and_then(Value::as_str) == Some(source.id.as_str())
            && entry.get("wrappedLevel").and_then(Value::as_str) == next_hierarchy_level(task_level(source))
            && entry.get("sourceTitleKey").and_then(Value::as_str).is_some_and(|key| !key.trim().is_empty())
            && match entry.get("sourceScopeVersion") {
                Some(version) => version.as_u64() == Some(source.hierarchy.scope_version.max(1)),
                // Legacy wrappers were always created one version above their
                // source. Require unchanged wrapper scope and matching source
                // title as well; unknown or edited provenance cannot complete.
                None => entry.get("sourceParentTitle").and_then(Value::as_str) == Some(source.title.as_str()),
            }
    })
}

fn wrapped_required_subtree_is_verified(
    run: &AgenticBoard,
    wrapper: &BoardTask,
    source: &BoardTask,
    explicitly_approved: &BTreeSet<String>,
) -> bool {
    let source_key = current_wrapped_scope_provenance(wrapper, source)
        .and_then(|entry| entry.get("sourceTitleKey"));
    let mut pending = vec![wrapper];
    let mut seen = BTreeSet::new();
    let mut executed_leaf = false;
    let mut original_wrapped_child = false;
    while let Some(task) = pending.pop() {
        if !seen.insert(task.id.as_str())
            || !explicitly_approved.contains(&task.id)
            || !task_rollup_completion_is_satisfied(task)
            || task.superseded_by.is_some()
            || task.error.as_deref().is_some_and(|error| !error.trim().is_empty())
            || !task_side_effects_are_approved(task)
        {
            return false;
        }
        if task.task_origin == "hierarchy_breakdown_wrapped"
            && Some(task_level(task)) == next_hierarchy_level(task_level(source))
            && current_wrapped_scope_provenance(task, source).is_some_and(|entry| entry.get("sourceTitleKey") == source_key)
        {
            original_wrapped_child = true;
        }
        let children = run.tasks.iter().filter(|child| {
            child.hierarchy.parent_id.as_deref() == Some(task.id.as_str()) && child.hierarchy.required
        }).collect::<Vec<_>>();
        if task_is_executable(task) {
            let recorded_done = task.hierarchy.attempts.last().is_some_and(|attempt| {
                attempt.get("status").and_then(Value::as_str) == Some(TASK_STATUS_DONE)
                    && attempt.get("scopeVersion").and_then(Value::as_u64) == Some(task.hierarchy.scope_version.max(1))
                    && attempt.get("finishedAt").and_then(Value::as_str).is_some_and(|value| !value.is_empty())
            });
            let accepted_evidence = task.result.as_ref().is_some_and(|result| {
                result.get("status").and_then(Value::as_str) == Some(TASK_STATUS_DONE)
                    && result.pointer("/evidenceGate/passed").and_then(Value::as_bool) == Some(true)
            });
            if task.attempt_count == 0 || task.completed_at.is_none() || !recorded_done || !accepted_evidence {
                return false;
            }
            executed_leaf = true;
        } else if children.is_empty() {
            return false;
        }
        pending.extend(children);
    }
    executed_leaf && original_wrapped_child
}

fn wrapped_source_ancestors_explicitly_approved(run: &AgenticBoard, source: &BoardTask, explicitly_approved: &BTreeSet<String>) -> bool {
    let mut parent_id = source.hierarchy.parent_id.as_deref();
    let mut seen = BTreeSet::new();
    while let Some(id) = parent_id {
        if !seen.insert(id) || !explicitly_approved.contains(id) {
            return false;
        }
        let Some(parent) = run.tasks.iter().find(|task| task.id == id) else { return false; };
        parent_id = parent.hierarchy.parent_id.as_deref();
    }
    true
}

fn complete_verified_wrapped_orphans(run: &mut AgenticBoard, explicitly_approved: &BTreeSet<String>) -> bool {
    let completed = run.tasks.iter().filter(|source| {
        !task_is_executable(source)
            && !task_status_is_done(&source.status)
            && explicitly_approved.contains(&source.id)
            && task_scope_is_approved(source)
            && task_ancestors_are_approved(run, source)
            && wrapped_source_ancestors_explicitly_approved(run, source, explicitly_approved)
            && source.superseded_by.is_none()
            && source.error.as_deref().is_none_or(|error| error.trim().is_empty())
            && !run.tasks.iter().any(|task| task.hierarchy.parent_id.as_deref() == Some(source.id.as_str()))
    }).filter(|source| {
        let wrappers = run.tasks.iter().filter(|wrapper| {
            wrapper.hierarchy.required && wrapper_matches_current_source(wrapper, source)
        }).collect::<Vec<_>>();
        !wrappers.is_empty() && wrappers.iter().all(|wrapper| wrapped_required_subtree_is_verified(run, wrapper, source, explicitly_approved))
    }).map(|source| source.id.clone()).collect::<Vec<_>>();
    for id in &completed {
        run.tasks.iter_mut().find(|task| task.id == *id).unwrap().status = TASK_STATUS_DONE.into();
        run.append_log(format!("Completed planning item {id} from verified required wrapped work; preserved executable history"));
    }
    !completed.is_empty()
}

fn refresh_direct_hierarchy_rollups(run: &mut AgenticBoard) {
    let approved_done_rollups = run
        .tasks
        .iter()
        .filter(|task| {
            !task_is_executable(task)
                && task_status_is_done(&task.status)
                && task.superseded_by.is_none()
                && task
                    .error
                    .as_deref()
                    .is_none_or(|error| error.trim().is_empty())
                && task
                    .hierarchy
                    .scope_approval
                    .as_ref()
                    .is_some_and(|approval| {
                        approval.approved
                            && approval.scope_version == task.hierarchy.scope_version.max(1)
                    })
        })
        .map(|task| task.id.clone())
        .collect::<BTreeSet<_>>();
    // Capture approval before deriving progress from children. In particular,
    // a child failure must not revoke the parent's approved scope.
    reconcile_scope_approval_records(run);
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
    // Older completion paths could persist a Done rollup before appending its
    // required QA child. Repair only that proven derived completion, never
    // arbitrary unfinished scope under an intentional Done boundary. Do this
    // bottom-up before checking eligibility so a stale Done ancestor cannot
    // make its required descendants appear blocked. Optional work must not
    // reopen completed required scope.
    let mut recovered_rollups = BTreeSet::new();
    for (parent_id, _) in &parent_ids {
        let unfinished_required_children = run
            .tasks
            .iter()
            .filter(|task| {
                task.hierarchy.parent_id.as_deref() == Some(parent_id.as_str())
                    && task.hierarchy.required
                    && !task_rollup_completion_is_satisfied(task)
            })
            .collect::<Vec<_>>();
        let only_pending_post_task_qa = !unfinished_required_children.is_empty()
            && unfinished_required_children.iter().all(|task| {
                recovered_rollups.contains(&task.id)
                    || task_is_recoverable_post_completion_qa(run, task)
            });
        if approved_done_rollups.contains(parent_id) && only_pending_post_task_qa {
            if let Some(parent) = run.tasks.iter_mut().find(|task| task.id == *parent_id) {
                // Only status is derived; scope, completed leaf evidence,
                // approval, and history stay intact.
                parent.status = TASK_STATUS_TODO.to_string();
                recovered_rollups.insert(parent_id.clone());
            }
        }
    }
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
        let parent_was_approved = parent.is_some_and(task_scope_is_approved);
        let parent_was_backlog =
            parent.is_some_and(|parent| task_status_is_backlog(&parent.status));
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
        let next_status = if parent_was_backlog {
            // Progress on an invalid/unapproved child cannot approve a parent.
            TASK_STATUS_BACKLOG
        } else if let Some(status) = parent_attention_status {
            status
        } else if parent_was_done {
            // Preserve completed scope unless the pre-pass established the
            // narrow persisted post-task QA ordering defect. Optional work
            // and arbitrary failed required work cannot reopen this boundary.
            TASK_STATUS_DONE
        } else if !parent_was_approved {
            // Unknown/explicitly unapproved attention states cannot acquire
            // approval merely because a child changes progress.
            parent.map_or(TASK_STATUS_BLOCKED, |parent| {
                canonical_task_status(&parent.status)
            })
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

fn task_is_recoverable_post_completion_qa(run: &AgenticBoard, task: &BoardTask) -> bool {
    task_is_executable(task)
        && task.task_level_qa
        && task.task_origin == "system_qa"
        && is_qa_task(task)
        && task.superseded_by.is_none()
        && task.source_task_id.as_deref().is_some_and(|source_id| {
            run.tasks.iter().any(|source| {
                source.id == source_id
                    && task_is_executable(source)
                    && task_is_done(source)
                    && source.superseded_by.is_none()
                    && source.attempt_count > 0
                    && source.hierarchy.parent_id == task.hierarchy.parent_id
                    && source.hierarchy.scope_version == task.hierarchy.scope_version
                    && task_blockers(task).iter().any(|id| id == source_id)
            })
        })
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

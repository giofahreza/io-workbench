// Scope boundaries are inherited along this ticket's hierarchy, not from
// unrelated board work. This is a conservative check of explicit package bans
// and proposed operations, not a natural-language sandbox for arbitrary tools.
fn package_scope_lineage<'a>(run: &'a AgenticBoard, task: &'a BoardTask) -> Vec<&'a BoardTask> {
    let mut lineage = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = Some(task);
    while let Some(task) = current {
        if !seen.insert(task.id.as_str()) {
            break;
        }
        lineage.push(task);
        current = task
            .hierarchy
            .parent_id
            .as_deref()
            .and_then(|id| run.tasks.iter().find(|candidate| candidate.id == id));
    }
    lineage
}

fn package_scope_texts(task: &BoardTask) -> Vec<&str> {
    [&task.title, &task.description, &task.details, &task.prompt]
        .into_iter()
        .chain(task.acceptance_criteria.iter())
        .chain(task.qa_test_commands.iter())
        .chain(task.hierarchy.side_effects.iter())
        .map(String::as_str)
        .collect()
}

fn package_scope_clauses(text: &str) -> Vec<&str> {
    let mut clauses = Vec::new();
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        let end = index + ch.len_utf8();
        if matches!(ch, ';' | '\n' | '\r')
            || (ch == '.' && text[end..].chars().next().is_none_or(char::is_whitespace))
        {
            let clause = text[start..index].trim();
            if !clause.is_empty() {
                clauses.push(clause);
            }
            start = end;
        }
    }
    let last = text[start..].trim();
    if !last.is_empty() {
        clauses.push(last);
    }
    clauses
}

fn package_ban_target(item: &str) -> bool {
    let words = item.split_whitespace().collect::<Vec<_>>();
    // An explicit runtime/production-only ban does not forbid test tooling.
    if words
        .iter()
        .any(|word| matches!(*word, "runtime" | "production"))
    {
        return false;
    }
    let item = words
        .into_iter()
        .skip_while(|word| {
            matches!(
                *word,
                "and"
                    | "or"
                    | "no"
                    | "any"
                    | "new"
                    | "additional"
                    | "external"
                    | "third-party"
                    | "third"
                    | "party"
                    | "test"
                    | "dev"
                    | "development"
                    | "npm"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    matches!(
        item.as_str(),
        "packages" | "dependencies" | "package installations" | "package installation"
    ) || [
        "packages are ",
        "packages should ",
        "dependencies are ",
        "dependencies should ",
    ]
    .iter()
    .any(|prefix| item.starts_with(prefix))
}

fn scope_disallows_new_packages(text: &str) -> bool {
    package_scope_clauses(text).into_iter().any(|clause| {
        let lower = clause.to_ascii_lowercase();
        let lower = lower.replace(" and ", ",").replace(" or ", ",");
        [
            "no ",
            "without ",
            "do not add ",
            "do not install ",
            "must not add ",
            "must not install ",
            "don't add ",
            "don't install ",
            "never add ",
            "never install ",
        ]
        .iter()
        .any(|prefix| {
            lower.match_indices(prefix).any(|(index, _)| {
                if index > 0 && lower.as_bytes()[index - 1].is_ascii_alphanumeric() {
                    return false;
                }
                let rest = &lower[index + prefix.len()..];
                let rest = rest.split(" but ").next().unwrap_or(rest);
                rest.split(',').map(str::trim).any(package_ban_target)
            })
        })
    })
}

fn task_package_ban_source<'a>(
    run: &'a AgenticBoard,
    task: &'a BoardTask,
) -> Option<&'a BoardTask> {
    package_scope_lineage(run, task).into_iter().find(|scope| {
        package_scope_texts(scope)
            .into_iter()
            .any(scope_disallows_new_packages)
    })
}

fn package_operation_is_negated(prefix: &str) -> bool {
    ["do not ", "must not ", "don't ", "never ", "without "]
        .iter()
        .any(|negation| prefix.contains(negation))
}

fn bare_package_list_entry(value: &str) -> bool {
    matches!(
        value,
        "package.json"
            | "package.json dependencies"
            | "package-lock.json"
            | "packages"
            | "dependencies"
            | "devdependencies"
            | "new packages"
            | "new dependencies"
            | "third-party packages"
            | "any third-party package"
    ) || [
        "npm install",
        "npm ci",
        "npm exec",
        "npx",
        "pnpm add",
        "pnpm install",
        "yarn add",
        "yarn install",
        "bun add",
        "bun install",
        "pip install",
        "pip3 install",
        "cargo add",
    ]
    .iter()
    .any(|command| {
        value == *command
            || value
                .strip_prefix(command)
                .is_some_and(|tail| tail.starts_with(' '))
    })
}

fn starts_package_prohibition_list(operation: &str) -> bool {
    for prefix in ["no ", "without "] {
        if operation
            .strip_prefix(prefix)
            .is_some_and(bare_package_list_entry)
        {
            return true;
        }
    }
    ["do not ", "must not ", "don't ", "never "]
        .iter()
        .any(|prefix| {
            operation.strip_prefix(prefix).is_some_and(|instruction| {
                [
                    "add ",
                    "introduce ",
                    "install ",
                    "run ",
                    "use ",
                    "download ",
                    "create ",
                ]
                .iter()
                .any(|verb| {
                    instruction
                        .strip_prefix(verb)
                        .is_some_and(bare_package_list_entry)
                })
            })
        })
}

fn package_scope_operations(clause: &str) -> Vec<String> {
    let mut operations = Vec::new();
    for independent in clause
        .split(" but ")
        .flat_map(|part| part.split(" && "))
        .flat_map(|part| part.split(" then "))
    {
        let mut prohibited_list = false;
        for part in independent.split(',').flat_map(|part| part.split(" and ")) {
            let operation = part.trim();
            let operation = operation.strip_prefix("or ").unwrap_or(operation);
            if starts_package_prohibition_list(operation) {
                prohibited_list = true;
                operations.push(format!("do not {operation}"));
            } else if prohibited_list && bare_package_list_entry(operation) {
                // A comma/and-separated bare item inherits the list's explicit
                // prohibition. New prose/verbs (e.g. "run npm install") reset
                // it; unrelated negative clauses cannot hide a later request.
                operations.push(format!("do not {operation}"));
            } else {
                prohibited_list = false;
                operations.push(operation.to_string());
            }
        }
    }
    operations
}

fn task_requests_new_packages(task: &BoardTask) -> bool {
    package_scope_texts(task).into_iter().any(|text| {
        package_scope_clauses(text).into_iter().any(|clause| {
            let lower = clause.to_ascii_lowercase();
            package_scope_operations(&lower)
                .into_iter()
                .any(|operation| {
                    let explicit_install = [
                        "npm install",
                        "npm ci",
                        "pnpm add",
                        "pnpm install",
                        "yarn add",
                        "yarn install",
                        "bun add",
                        "bun install",
                        "pip install",
                        "pip3 install",
                        "cargo add",
                    ]
                    .iter()
                    .any(|command| {
                        operation
                            .match_indices(command)
                            .any(|(index, _)| !package_operation_is_negated(&operation[..index]))
                    });
                    let possible_download = ["npx ", "npm exec "].iter().any(|command| {
                        operation.match_indices(command).any(|(index, _)| {
                            !package_operation_is_negated(&operation[..index])
                                && !operation[index + command.len()..]
                                    .split_whitespace()
                                    .take_while(|arg| arg.starts_with('-') && *arg != "--")
                                    .any(|arg| matches!(arg, "--no-install" | "--offline"))
                        })
                    });
                    let dependency_change = (operation.contains("devdependenc")
                        || operation.contains("test dependenc")
                        || operation.contains("package dependenc")
                        || operation.contains("new packages")
                        || operation.contains("new dependencies"))
                        && [
                            "add",
                            "install",
                            "introduce",
                            "declare",
                            "create",
                            "configure",
                            "update",
                        ]
                        .iter()
                        .any(|verb| {
                            operation.match_indices(verb).any(|(index, _)| {
                                (index == 0
                                    || !operation.as_bytes()[index - 1].is_ascii_alphanumeric())
                                    && !package_operation_is_negated(&operation[..index])
                                    && !operation[..index].trim_start().starts_with("no ")
                            })
                        });
                    explicit_install || possible_download || dependency_change
                })
        })
    })
}

fn task_package_scope_conflict(run: &AgenticBoard, task: &BoardTask) -> Option<String> {
    let source = task_package_ban_source(run, task)?;
    package_scope_conflict_for_source(task, source)
}

fn package_scope_conflict_for_source(task: &BoardTask, source: &BoardTask) -> Option<String> {
    task_requests_new_packages(task).then(|| format!(
        "Package scope conflict: task {} proposes installing/downloading or adding package dependencies, but inherited scope {} forbids new packages. Test/dev-only tooling and generated plannedFiles do not override that boundary. Use built-in or already available no-install tooling, or leave this work blocked for an explicit scope revision; do not waive required TDD evidence.",
        task.id, source.id,
    ))
}

fn hierarchy_package_scope_conflict(
    run: &AgenticBoard,
    parent: &BoardTask,
    child: &BoardTask,
) -> Option<String> {
    // Provider-local IDs are not persisted IDs: a child called "task-1" may
    // share its parent's ID before remapping. Walk the real parent explicitly
    // so cycle protection cannot accidentally discard the ancestor boundary.
    let source = if package_scope_texts(child)
        .into_iter()
        .any(scope_disallows_new_packages)
    {
        child
    } else {
        task_package_ban_source(run, parent)?
    };
    package_scope_conflict_for_source(child, source)
}

fn inherited_scope_constraints_block(run: &AgenticBoard, task: &BoardTask) -> String {
    let mut boundaries = Vec::new();
    for scope in package_scope_lineage(run, task) {
        for text in package_scope_texts(scope) {
            for clause in package_scope_clauses(text) {
                let lower = clause.to_ascii_lowercase();
                if [
                    "no ",
                    "do not ",
                    "don't ",
                    "must not ",
                    "never ",
                    "without ",
                    "only ",
                ]
                .iter()
                .any(|marker| {
                    lower.match_indices(marker).any(|(index, _)| {
                        index == 0 || !lower.as_bytes()[index - 1].is_ascii_alphanumeric()
                    })
                }) {
                    let boundary = format!("- {}: {}", scope.id, clause);
                    if !boundaries.contains(&boundary) {
                        boundaries.push(boundary);
                    }
                }
            }
        }
    }
    if boundaries.is_empty() {
        return String::new();
    }
    format!(
        "Inherited scope boundaries (not additional completion criteria):\n{}\n\nGeneric tooling allowances and generated child requirements do not override these boundaries. Satisfy this ticket only; do not implement the whole ancestor feature. An unqualified no-packages/no-new-dependencies boundary also forbids test/dev dependencies and automatic downloads through npm/npx. Use built-in or already available no-install tooling. If required verification cannot be performed within the boundary, return blocked with the compatibility conflict; do not disable TDD, fabricate evidence, or silently reinterpret the user's restriction.\n",
        boundaries.join("\n"),
    )
}

fn task_package_file_policy_violations(
    run: &AgenticBoard,
    task: &BoardTask,
    summary: &Value,
) -> Vec<String> {
    if task_package_ban_source(run, task).is_none() {
        return Vec::new();
    }
    ["changedBySubtask", "reusedEvidence"]
        .into_iter()
        .flat_map(|key| {
            summary
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(move |entry| {
                    let path = entry.get("path").and_then(Value::as_str)?;
                    if !matches!(path, "package.json" | "package-lock.json") {
                        return None;
                    }
                    let policy = if key == "reusedEvidence" {
                        entry.get("testToolingPolicy")
                    } else {
                        summary.get("testTooling")
                    };
                    let manifest_safe = policy
                        .and_then(|policy| policy.get("dependencyAdditionsAbsent"))
                        .and_then(Value::as_bool)
                        == Some(true);
                    let lock_safe = path != "package-lock.json"
                        || policy
                            .and_then(|policy| policy.get("lockDependencyAdditionsAbsent"))
                            .and_then(Value::as_bool)
                            == Some(true);
                    (!manifest_safe || !lock_safe).then(|| path.to_string())
                })
        })
        .collect()
}

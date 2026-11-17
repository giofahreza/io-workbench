const GENERATED_SCOPE_EXCLUSION: &str = "Persistence and task interactions are explicitly out of scope for this task and must not be implemented.";
const GENERATED_SHORT_SCOPE_EXCLUSION: &str =
    "Persistence and task interactions are explicitly out of scope and must not be implemented.";
const GENERATED_H_HTML_DESCRIPTION: &str = "Add a single index.html at the project root containing a Checklist heading, a labeled text input (for example a New item label), an Add button, an empty unordered list, and two zero-valued counters labelled Total and Remaining. Inline a <style> block that defines a responsive layout and include a viewport meta tag plus at least one @media rule targeting a phone-sized breakpoint (such as max-width: 600px) so the page is usable on a 360px wide phone. No JavaScript, no Add-button click handler, no external resources, no package files, no backend, no persistence logic, and no localStorage usage may be added; task interactions and persistence are explicitly out of scope.";
const GENERATED_H_MANUAL_DESCRIPTION: &str = "Open the static index.html in a real browser at a 360x800 phone viewport and a 1280x800 desktop viewport. Confirm the Checklist heading, the labeled input, the Add button, the empty list, and the Total 0 and Remaining 0 counters are all rendered legibly with no horizontal scrolling on the phone. Save screenshots under a verification/ folder for the change record. Do not exercise the Add button or any other interaction because they are out of scope for this story.";

fn scope_exclusion_task(description: &str) -> BoardTask {
    let run = board_fixture(json!({
        "command": "Create a static checklist shell; no interactions or persistence.",
        "projectPath": "/tmp/scope-exclusion-fixture"
    }));
    task_from_json(&run, json!({
        "id": "shell", "title": "Create index.html static checklist shell with phone-responsive CSS",
        "kind": "implementation", "description": description,
        "required": true, "plannedFiles": ["index.html"], "sideEffects": []
    }), 0, TASK_STATUS_TODO).unwrap()
}

#[test]
fn scope_exclusion_actual_generated_boundary_does_not_require_a_new_approval() {
    let task = scope_exclusion_task(&format!(
        "Create index.html as a single self-contained file. Use plain HTML5 plus inline CSS. Do NOT include script tags, localStorage usage, service worker, fetch calls, external resources, package manifest, build tooling, or accounts. {GENERATED_SCOPE_EXCLUSION}"
    ));
    assert!(!generated_child_requires_backlog_approval(&task));
    assert!(task.hierarchy.required);
    assert!(task.hierarchy.side_effects.is_empty());
    assert_eq!(task.hierarchy.planned_files, vec!["index.html"]);
}

#[test]
fn scope_exclusion_can_be_an_acceptance_boundary_without_broadly_exempting_the_task() {
    for boundary in [GENERATED_SCOPE_EXCLUSION, GENERATED_SHORT_SCOPE_EXCLUSION] {
        let mut task = scope_exclusion_task("Create index.html using plain HTML and CSS.");
        task.acceptance_criteria = vec![boundary.into()];
        assert!(!generated_child_requires_backlog_approval(&task));
        task.title = "Implement out of scope analytics".into();
        assert!(generated_child_requires_backlog_approval(&task));
    }
}

#[test]
fn scope_exclusion_does_not_hide_other_expanded_or_optional_work() {
    for boundary in [GENERATED_SCOPE_EXCLUSION, GENERATED_SHORT_SCOPE_EXCLUSION] {
        for extra in [
            " Add out of scope analytics.",
            ". Add out of scope analytics.",
            "\nAdd an out-of-scope theme switcher.",
            "; Add out of scope analytics.",
            " but implement out-of-scope analytics.",
            ", but implement out-of-scope analytics.",
            " Add optional charting.",
            " Add a nice-to-have export button.",
        ] {
            let task = scope_exclusion_task(&format!("{}{extra}", boundary.trim_end_matches('.')));
            assert!(generated_child_requires_backlog_approval(&task), "{extra}");
        }
    }
    for mixed in [
        "Implement out of scope analytics that must not be implemented incorrectly.",
        "Add analytics that are explicitly out of scope for this task and must not be implemented.",
        "Create an out of scope feature for this task and must not be implemented.",
        "Persistence is explicitly out of scope for this task and must not be implemented until later.",
        "Add analytics that are explicitly out of scope and must not be implemented.",
        "Persistence is out of scope and must not be implemented incorrectly.",
        "Persistence is out of scope and must not be implemented until later.",
        "Persistence is out of scope and must not be implemented but add out-of-scope analytics.",
    ] {
        assert!(
            generated_child_requires_backlog_approval(&scope_exclusion_task(mixed)),
            "{mixed}"
        );
    }
}

#[test]
fn scope_exclusion_preserves_the_supported_marker_and_subject_variants() {
    for boundary in [
        GENERATED_SCOPE_EXCLUSION,
        GENERATED_SHORT_SCOPE_EXCLUSION,
        "Persistence is out-of-scope for this task and must not be implemented.",
        "Persistence and task interactions are out of scope for this task and must not be implemented.",
    ] {
        assert!(
            !generated_child_requires_backlog_approval(&scope_exclusion_task(boundary)),
            "{boundary}"
        );
    }
}

#[test]
fn scope_exclusion_keeps_structured_and_external_approval_gates() {
    let mut task = scope_exclusion_task(GENERATED_SCOPE_EXCLUSION);
    task.hierarchy.required = false;
    assert!(generated_child_requires_backlog_approval(&task));
    task.hierarchy.required = true;
    task.hierarchy.side_effects = vec!["Update shared deployment".into()];
    task.hierarchy.side_effects_approved = true;
    assert!(generated_child_requires_backlog_approval(&task));
    task.hierarchy.side_effects.clear();
    task.description.push_str(" Change production config.");
    assert!(generated_child_requires_backlog_approval(&task));
    task.description = GENERATED_SCOPE_EXCLUSION.into();
    task.task_type = TASK_KIND_MIGRATION.into();
    assert!(generated_child_requires_backlog_approval(&task));
}

#[test]
fn scope_exclusion_actual_h_html_plan_stays_under_its_parent() {
    let task = scope_exclusion_task(GENERATED_H_HTML_DESCRIPTION);
    assert!(!generated_child_requires_backlog_approval(&task));
    assert!(task.hierarchy.required);
    assert!(task.hierarchy.side_effects.is_empty());
}

#[test]
fn scope_exclusion_actual_h_manual_plan_stays_under_its_parent() {
    let mut task = scope_exclusion_task(GENERATED_H_MANUAL_DESCRIPTION);
    task.title = "Verify Checklist layout on phone and desktop viewports".into();
    task.task_type = TASK_KIND_MANUAL_TEST.into();
    task.hierarchy.planned_files = vec!["verification/phone-360x800.png".into(), "verification/desktop-1280x800.png".into()];
    assert!(!generated_child_requires_backlog_approval(&task));
    assert!(task.hierarchy.required);
    assert!(task.hierarchy.side_effects.is_empty());
}

#[test]
fn scope_exclusion_declarative_and_negative_rationale_forms_are_clause_local() {
    for boundary in [
        "Task interactions and persistence are explicitly out of scope.",
        "Persistence is out-of-scope for this story.",
        "Accounts are out of scope for this feature.",
        "\"Persistence\", accounts, and task interactions are explicitly out of scope.",
        "- Accounts and persistence are out of scope for this ticket.",
        "Do not exercise the Add button or any other interaction because they are out of scope for this story.",
        "Do not exercise the \"Add\" button because interactions are out of scope.",
        "Never implement persistence because it is out of scope for this task.",
        "Must not add accounts because they are explicitly out-of-scope for this ticket.",
    ] {
        assert!(!generated_child_requires_backlog_approval(&scope_exclusion_task(boundary)), "{boundary}");
        let mut acceptance = scope_exclusion_task("Create the static shell.");
        acceptance.acceptance_criteria = vec![boundary.into()];
        assert!(!generated_child_requires_backlog_approval(&acceptance), "acceptance: {boundary}");
        acceptance.title = "Implement out of scope analytics".into();
        assert!(generated_child_requires_backlog_approval(&acceptance), "title: {boundary}");
        for extra in [" Add out of scope analytics.", "\nImplement out-of-scope analytics.", "; Implement out of scope analytics.", " But implement out of scope analytics."] {
            assert!(generated_child_requires_backlog_approval(&scope_exclusion_task(&format!("{boundary}{extra}"))), "{boundary}{extra}");
        }
    }
}

#[test]
fn scope_exclusion_does_not_negate_positive_or_ambiguous_rationale() {
    for mixed in [
        "Implement analytics because they are out of scope for this story.",
        "Add analytics that are explicitly out of scope.",
        "Persistence is out of scope but implement it anyway.",
        "Persistence is out of scope until later.",
        "If persistence is out of scope, implement accounts.",
        "Do not change the UI and implement analytics because they are out of scope.",
        "Do not exercise the Add button, run analytics because they are out of scope.",
        "Do not exercise the Add button or run analytics because they are out of scope.",
        "Do not exercise the Add button but implement analytics because they are out of scope.",
        "Do not exercise the Add button then implement analytics because they are out of scope.",
        "Do not exercise the Add button && implement analytics because they are out of scope.",
        "Do not exercise the Add button because interactions are out of scope, but implement analytics.",
        "Do not exercise the Add button because interactions are out of scope, add analytics.",
        "Persistence is out of scope; implement out-of-scope analytics.",
        "Persistence is out of scope and analytics are out of scope but implement them.",
    ] {
        assert!(generated_child_requires_backlog_approval(&scope_exclusion_task(mixed)), "{mixed}");
    }
}

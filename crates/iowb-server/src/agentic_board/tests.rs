#[cfg(test)]
mod tests {
    // Shared fixtures and environment guards come first because the focused
    // test files below use them through this common private namespace.
    include!("tests/fixtures_and_recovery.rs");
    include!("tests/restart_recovery.rs");
    include!("tests/authoring_and_hierarchy.rs");
    include!("tests/execution_and_rollups.rs");
    include!("tests/post_task_qa_rollups.rs");
    include!("tests/scope_and_normalization.rs");
    include!("tests/tracking_sanitizer.rs");
    include!("tests/retry_approval.rs");
    include!("tests/scheduler_approval_race.rs");
    include!("tests/dependency_blocker_message.rs");
    include!("tests/workspace_evidence.rs");
    include!("tests/hierarchical_red_phase.rs");
    include!("tests/package_free_red_baseline.rs");
    include!("tests/test_stage_qa_scope.rs");
    include!("tests/discussion_lifecycle.rs");
    include!("tests/discussion_session_isolation.rs");
    include!("tests/review_routing_and_baseline.rs");
    include!("tests/test_config_policy.rs");
    include!("tests/package_scope_constraints.rs");
    include!("tests/transcript_redaction_progress.rs");
    include!("tests/blocked_error_feedback.rs");
    include!("tests/planning_and_recovery.rs");
    include!("tests/hierarchy_planning_contract.rs");
    include!("tests/scope_exclusion_boundary.rs");
    include!("tests/wrapped_scope_completion.rs");
    include!("tests/planning_evidence_contract.rs");
    include!("tests/manual_ui_evidence.rs");
    include!("tests/manual_contract_ingress.rs");
    include!("tests/prompt_draft_acceptance.rs");
    include!("tests/validation_summary.rs");
    include!("tests/board_read_project_access.rs");
    include!("tests/session_backfill_reads.rs");
    include!("tests/guarded_quality_settings.rs");
}

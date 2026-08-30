#[cfg(test)]
mod tests {
    // Shared fixtures and environment guards come first because the focused
    // test files below use them through this common private namespace.
    include!("tests/fixtures_and_recovery.rs");
    include!("tests/authoring_and_hierarchy.rs");
    include!("tests/execution_and_rollups.rs");
    include!("tests/scope_and_normalization.rs");
    include!("tests/planning_and_recovery.rs");
}

// Storage models, persistence operations, mappings, and tests are grouped by
// data domain. The façade keeps the existing Storage API unchanged.
include!("storage/preamble.rs");
include!("storage/lifecycle_and_auth.rs");
include!("storage/projects_and_external_history.rs");
include!("storage/sessions.rs");
include!("storage/durable_runs_and_usage.rs");
include!("storage/context_rollovers.rs");
include!("storage/durable_messages_and_forks.rs");
include!("storage/messages_and_search.rs");
include!("storage/settings_and_credentials.rs");
include!("storage/database_connections_and_jobs.rs");
include!("storage/row_mapping_and_usage.rs");
include!("storage/metadata_and_values.rs");

#[cfg(test)]
mod tests {
    include!("storage/tests/fixtures_and_history.rs");
    include!("storage/tests/schema_and_usage.rs");
    include!("storage/tests/usage_and_forks.rs");
    include!("storage/tests/rollovers_and_sessions.rs");
    include!("storage/tests/credentials_and_runs.rs");
}

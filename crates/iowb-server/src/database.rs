// Database API, live adapters, SQLite support, and transfer jobs are kept in
// separate feature files but share this module's private namespace.
include!("database/preamble.rs");
include!("database/endpoints_and_connections.rs");
include!("database/table_operations.rs");
include!("database/transfer_jobs.rs");
include!("database/row_operations.rs");
include!("database/live_connections.rs");
include!("database/live_schema.rs");
include!("database/live_queries.rs");
include!("database/live_value_conversion.rs");
include!("database/sqlite.rs");
include!("database/transfer_runtime.rs");

#[cfg(test)]
mod tests {
    include!("database/tests/fixtures_and_live.rs");
    include!("database/tests/live_and_contracts.rs");
    include!("database/tests/sqlite_transfers.rs");
    include!("database/tests/errors_and_progress.rs");
}

#![recursion_limit = "256"]

// The server crate is a façade over feature-oriented implementation files.
// Keeping this list explicit makes the runtime composition easy to scan.
mod agentic_board;
mod database;
mod git;
mod rag_client;

include!("server/preamble.rs");
include!("server/recovery_and_routing.rs");
include!("server/system_metrics.rs");
include!("server/projects_and_workspaces.rs");
include!("server/files_and_uploads.rs");
include!("server/settings.rs");
include!("server/provider_settings.rs");
include!("server/session_api.rs");
include!("server/processes_and_websocket.rs");
include!("server/notifications_and_watch.rs");
include!("server/configuration.rs");
include!("server/direct_ai.rs");
include!("server/provider_runtime.rs");
include!("server/tools_and_compat.rs");

#[cfg(test)]
mod tests {
    include!("server/tests/streaming_and_workspace.rs");
    include!("server/tests/models_and_configuration.rs");
    include!("server/tests/recovery.rs");
}

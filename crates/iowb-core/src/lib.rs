// Core runtime state, sessions, provider execution, and recovery are split
// by responsibility below while remaining one private module namespace.
mod codex_app_server;
mod external_sessions;

include!("core/preamble.rs");
include!("core/session_lifecycle.rs");
include!("core/app_state/mod.rs");
include!("core/auth.rs");
include!("core/sessions/mod.rs");
include!("core/session_helpers.rs");
include!("core/agent_runtime/mod.rs");
include!("core/managers.rs");
include!("core/direct_ai.rs");
include!("core/agent_commands/mod.rs");
include!("core/codex_normalizer.rs");
include!("core/claude_gemini_normalizers.rs");
include!("core/runtime_and_auth_utils.rs");

#[cfg(test)]
mod tests {
    include!("core/tests/fixtures_and_sessions.rs");
    include!("core/tests/normalizers.rs");
    include!("core/tests/context_rollover.rs");
    include!("core/tests/session_recovery.rs");
    include!("core/tests/runtime_and_models.rs");
}

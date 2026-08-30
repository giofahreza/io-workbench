// Provider-owned session discovery and transcript normalization are organized
// by source and message lifecycle.
include!("external_sessions/preamble.rs");
include!("external_sessions/discovery.rs");
include!("external_sessions/provider_sources.rs");
include!("external_sessions/messages.rs");
include!("external_sessions/gemini_and_formatting.rs");
include!("external_sessions/message_utils.rs");

#[cfg(test)]
mod tests {
    include!("external_sessions/tests.rs");
}

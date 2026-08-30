// Wire types are grouped by API domain so protocol changes stay localized.
include!("protocol/preamble.rs");
include!("protocol/foundations.rs");
include!("protocol/sessions.rs");
include!("protocol/files_and_git.rs");
include!("protocol/auth_and_process.rs");
include!("protocol/database.rs");
include!("protocol/websocket.rs");

#[cfg(test)]
mod tests {
    include!("protocol/tests.rs");
}

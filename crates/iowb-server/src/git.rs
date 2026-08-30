// Git handlers are organized by endpoint family and shared repository
// helpers. The private namespace keeps the public router contract unchanged.
include!("git/preamble.rs");
include!("git/endpoints/status_and_history.rs");
include!("git/endpoints/branches_and_tags.rs");
include!("git/endpoints/remote_and_worktree.rs");
include!("git/repository.rs");
include!("git/patches_and_conflicts.rs");
include!("git/commit_message.rs");

#[cfg(test)]
mod tests {
    include!("git/tests.rs");
}

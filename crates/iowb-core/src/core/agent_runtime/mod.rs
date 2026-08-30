// Agent execution is grouped by state, lifecycle, usage persistence, and
// process ownership so each implementation file remains easy to navigate.
include!("types.rs");
include!("manager.rs");
include!("usage.rs");
include!("process_ownership.rs");
include!("process_control.rs");
include!("defaults.rs");

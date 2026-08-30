// Agent command construction is organized by lifecycle responsibility.
include!("command_resolution.rs");
include!("runtime_selection.rs");
include!("launch_arguments.rs");
include!("recovery.rs");
include!("output_limits.rs");
include!("event_processing.rs");
include!("persistence.rs");

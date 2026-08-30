// HTTP handlers are grouped by the resource they serve. Keeping these files
// in one private module preserves the existing handler visibility while making
// the API surface easy to scan.
include!("http/boards.rs");
include!("http/controls.rs");
include!("http/tasks.rs");
include!("http/scope.rs");
include!("http/research.rs");

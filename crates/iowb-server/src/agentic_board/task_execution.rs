// Task execution is organized as a pipeline: prompt construction, provider
// result processing, completion evidence, and managed-git bookkeeping.
include!("task_execution/prompts.rs");
include!("task_execution/result_processing.rs");
include!("task_execution/completion.rs");
include!("task_execution/managed_git.rs");

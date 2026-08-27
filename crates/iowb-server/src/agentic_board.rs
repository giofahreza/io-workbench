use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Extension, Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    routing::{get, patch, post},
};
use chrono::{DateTime, Utc};
use iowb_core::{AppState, DirectAiRuntimeConfig, augmented_user_path};
use iowb_fs::FsError;
use iowb_protocol::{ChatRuntime, MessageRole, Provider, SessionSummary, new_id};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    time::{sleep, timeout},
};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    AuthenticatedUser, Result, ServerError, join_io_gateway_endpoint_url,
    rag_client::{
        ProjectIndexRequest, PromotionApproveRequest, PromotionCandidatesRequest, RagClient,
        RagQueryRequest, TaskResultIngestRequest, ValidationErrorIngestRequest, error_record,
        rag_traceparent,
    },
};

const BOARD_STORAGE_DIR: &str = "agentic-boards";
const DEFAULT_PROVIDER: &str = "claude";
const DEFAULT_MODEL: &str = "minimax-m3";
const DEFAULT_BREAKDOWN_PROVIDER: &str = "codex";
const DEFAULT_BREAKDOWN_MODEL: &str = "gpt-5.5";
const PROVIDER_POLL_INTERVAL: Duration = Duration::from_millis(750);
const SOURCE_CHUNK_TARGET_LENGTH: usize = 12_000;
const SOURCE_CHUNK_MAX_LENGTH: usize = 18_000;
const PROMOTION_REVIEW_TASK_ID: &str = "promotion-review";
const CODEBASE_CHUNK_TARGET_LENGTH: usize = 10_000;
const MAX_SOURCE_FILES: usize = 160;
const MAX_CODEBASE_FILES: usize = 2_500;
const MAX_CODEBASE_CHUNKS: usize = 600;
const MAX_WORKSPACE_SNAPSHOT_FILES: usize = 2_000;
const WORKSPACE_OWNERSHIP_POLICY: &str = "conservative_snapshot_attribution";
const LEGACY_WORKSPACE_OWNERSHIP_POLICY: &str = "legacy_unattributed";
const MAX_PROVIDER_OUTPUT_CHARS: usize = 160_000;
const AUTO_RETRY_POLL_INTERVAL: Duration = Duration::from_secs(30);
const DETERMINISTIC_VALIDATION_TIMEOUT: Duration = Duration::from_secs(120);
const MANAGED_GIT_TIMEOUT: Duration = Duration::from_secs(30);
const MANAGED_GIT_PUSH_TIMEOUT: Duration = Duration::from_secs(180);
const FINAL_QA_TASK_ID: &str = "final-qa";
const AGENTS_KNOWLEDGE_TASK_ID: &str = "agents-knowledge";
const MAX_FOLLOWUP_TASKS_PER_GROUP: usize = 3;
const MAX_TASK_ATTEMPTS: u32 = 2;
const MAX_HIERARCHY_CHILDREN_PER_PARENT: usize = 12;
const MAX_HIERARCHY_REFINEMENT_ADDITIONS: usize = 4;
const DEFAULT_MALFORMED_TOOL_CALL_REPAIR_RETRIES: u64 = 1;
const MAX_MALFORMED_TOOL_CALL_REPAIR_RETRIES: u64 = 3;
const DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL: &str = "http://141.144.197.96:8319/claude";
const IO_GATEWAY_API_KEY_CREDENTIAL: &str = "io-workbench-io-gateway-api-key";
const IO_GATEWAY_API_KEY_CREDENTIAL_TYPE: &str = "io_gateway_api_key";
const CURSOR_CLI_COMMAND: &str = "cursor-agent";
const TASK_STATUS_BACKLOG: &str = "backlog";
const TASK_STATUS_TODO: &str = "todo";
const TASK_STATUS_IN_PROGRESS: &str = "in_progress";
const TASK_STATUS_BLOCKED: &str = "blocked";
const TASK_STATUS_FAILED: &str = "failed";
const TASK_STATUS_DONE: &str = "done";
const TASK_LEVEL_INITIATIVE: &str = "initiative";
const TASK_LEVEL_EPIC: &str = "epic";
const TASK_LEVEL_STORY: &str = "story";
const TASK_LEVEL_TASK: &str = "task";
const TASK_LEVEL_SUBTASK: &str = "subtask";
const TASK_KIND_IMPLEMENTATION: &str = "implementation";
const TASK_KIND_RESEARCH: &str = "research";
const TASK_KIND_DESIGN: &str = "design";
const TASK_KIND_TEST_IMPLEMENTATION: &str = "test_implementation";
const TASK_KIND_MANUAL_TEST: &str = "manual_test";
const TASK_KIND_QA: &str = "qa";
const TASK_KIND_REVIEW: &str = "review";
const TASK_KIND_FIX: &str = "fix";
const TASK_KIND_FOLLOWUP: &str = "followup";
const TASK_KIND_MIGRATION: &str = "migration";
const TASK_KIND_REVERT: &str = "revert";
const TASK_KIND_CLEANUP: &str = "cleanup";
const TASK_KIND_REVISION: &str = "revision";
const TASK_KIND_REPLACEMENT: &str = "replacement";
const RETRY_MODE_TRANSIENT: &str = "transient";
const RETRY_MODE_FIX: &str = "fix";
const PLANNING_ERROR_PHASE: &str = "planning_error";
const TASK_PRIORITY_P0: &str = "p0";
const TASK_PRIORITY_P1: &str = "p1";
const TASK_PRIORITY_P2: &str = "p2";
const TASK_PRIORITY_P3: &str = "p3";
static AUTO_RETRY_POLLER_STARTED: AtomicBool = AtomicBool::new(false);
static BOARD_MUTATION_LOCK: Mutex<()> = Mutex::new(());
static BOARD_SAVE_LOCK: Mutex<()> = Mutex::new(());
static PROJECT_EXECUTION_OWNERS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/danger/boards", get(list_boards).post(create_board))
        .route("/api/danger/boards/{id}", get(get_board))
        .route("/api/danger/boards/{id}/pause", post(pause_board))
        .route("/api/danger/boards/{id}/resume", post(start_board))
        .route("/api/danger/boards/{id}/schedule", post(schedule_board))
        .route("/api/danger/boards/{id}/abort", post(abort_board))
        .route("/api/danger/boards/{id}/model", patch(update_model))
        .route(
            "/api/danger/boards/{id}/model-strategy",
            patch(update_model_strategy),
        )
        .route(
            "/api/danger/boards/{id}/git-policy",
            patch(update_git_policy),
        )
        .route(
            "/api/danger/boards/{id}/tools",
            patch(update_tools_settings),
        )
        .route("/api/danger/boards/{id}/tdd", patch(update_tdd_settings))
        .route(
            "/api/danger/boards/{id}/validation",
            patch(update_validation_config),
        )
        .route("/api/danger/boards/{id}/rag", patch(update_rag_settings))
        .route("/api/danger/boards/{id}/qa-policy", patch(update_qa_policy))
        .route(
            "/api/danger/boards/{id}/task-models",
            patch(update_task_models),
        )
        .route(
            "/api/danger/boards/{id}/auto-retry",
            patch(update_auto_retry),
        )
        .route("/api/danger/boards/{id}/tasks", post(add_task))
        .route("/api/danger/boards/{id}/tasks/draft", post(draft_tasks))
        .route(
            "/api/danger/boards/{id}/tasks/backlog-from-prompt",
            post(backlog_from_prompt),
        )
        .route(
            "/api/danger/boards/{id}/tasks/{task_id}/breakdown",
            post(breakdown_task),
        )
        .route(
            "/api/danger/boards/{id}/tasks/{task_id}/side-effects/approve",
            post(approve_task_side_effects),
        )
        .route(
            "/api/danger/boards/{id}/tasks/{task_id}/research/accept",
            post(accept_research),
        )
        .route(
            "/api/danger/boards/{id}/tasks/{task_id}/detach",
            post(detach_task),
        )
        .route(
            "/api/danger/boards/{id}/tasks/{task_id}/scope-effects/resolve",
            post(resolve_scope_effects),
        )
        .route(
            "/api/danger/boards/{id}/tasks/retry-attention",
            post(retry_attention_tasks),
        )
        .route(
            "/api/danger/boards/{id}/backlog-breakdown/retry",
            post(retry_backlog_breakdown),
        )
        .route(
            "/api/danger/boards/{id}/tasks/{task_id}/promote",
            post(promote_task),
        )
        .route(
            "/api/danger/boards/{id}/tasks/{task_id}/demote",
            post(demote_task),
        )
        .route(
            "/api/danger/boards/{id}/tasks/{task_id}",
            patch(update_task).delete(delete_task),
        )
        .route(
            "/api/danger/boards/{id}/tasks/{task_id}/discussion",
            post(discuss_task),
        )
        .route(
            "/api/danger/boards/{id}/tasks/{task_id}/discussion/{proposal_id}/apply",
            post(apply_discussion_proposal),
        )
        .route(
            "/api/danger/boards/{id}/tasks/{task_id}/discussion/{proposal_id}/reject",
            post(reject_discussion_proposal),
        )
}

pub(crate) fn recover_active_boards(state: &AppState) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    start_auto_retry_poller(handle.clone(), state.clone());
    let Ok(boards) = load_boards(state) else {
        return;
    };
    for stored in boards {
        if !matches!(stored.board.status.as_str(), "running")
            && !stored.board.active
            && !stored.board.loop_started
        {
            continue;
        }
        let Some(user_id) = stored.board.user_id.clone() else {
            continue;
        };
        let board_id = stored.board.id.clone();
        let state = state.clone();
        handle.spawn(async move {
            match load_user_board(&state, &user_id, &board_id) {
                Ok(mut stored) => {
                    if let Err(error) =
                        claim_project_execution(&stored.board.project_path, &stored.board.id)
                    {
                        stored.board.status = TASK_STATUS_BLOCKED.to_string();
                        stored.board.active = false;
                        stored.board.loop_started = false;
                        stored.board.current_phase = Some("blocked".to_string());
                        stored.board.pause_reason = Some(server_error_message(&error));
                        stored.board.append_log(
                            "Active board recovery stopped because another board owns the project",
                        );
                        stored.board.touch();
                        if let Err(save_error) = save_board(&state, &stored.board) {
                            tracing::warn!(error = %server_error_message(&save_error), "failed to persist recovered board ownership conflict");
                        }
                        return;
                    }
                    clear_board_abort_state(&mut stored.board);
                    stored.board.status = "running".to_string();
                    stored.board.active = true;
                    stored.board.loop_started = true;
                    stored.board.pause_requested = false;
                    stored.board.current_task_id = None;
                    stored.board.current_task_title.clear();
                    stored.board.current_task_status.clear();
                    stored
                        .board
                        .append_log("Recovered active agentic board after server restart");
                    stored.board.touch();
                    if let Err(error) = save_board(&state, &stored.board) {
                        release_project_execution(&stored.board.project_path, &stored.board.id);
                        tracing::warn!(error = %server_error_message(&error), "failed to persist recovered agentic board");
                        return;
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %server_error_message(&error), "failed to load active agentic board for recovery");
                    return;
                }
            }
            if let Err(error) = execute_board_loop(state, user_id, board_id).await {
                tracing::warn!(error = %server_error_message(&error), "recovered agentic board worker failed");
            }
        });
    }
}

/// Classify sessions written by older board versions before any ordinary
/// project/session discovery or chat recovery can expose them. The lazy
/// list/detail repair remains in place for snapshots copied in while the
/// server is already running.
pub(crate) async fn backfill_legacy_board_sessions(state: &AppState) -> Result<()> {
    for mut stored in load_boards(state)? {
        backfill_board_session_links(state, &mut stored.board).await?;
    }
    Ok(())
}

fn start_auto_retry_poller(handle: tokio::runtime::Handle, state: AppState) {
    if AUTO_RETRY_POLLER_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    handle.spawn(async move {
        loop {
        if let Err(error) = process_auto_retries(&state).await {
                tracing::warn!(error = %server_error_message(&error), "agentic board auto retry poll failed");
            }
            if let Err(error) = process_scheduled_starts(&state).await {
                tracing::warn!(error = %server_error_message(&error), "agentic board scheduled start poll failed");
            }
            sleep(AUTO_RETRY_POLL_INTERVAL).await;
        }
    });
}

async fn process_scheduled_starts(state: &AppState) -> Result<()> {
    let now = Utc::now();
    for mut stored in load_boards(state)? {
        if stored.board.status != "scheduled" {
            continue;
        }
        let Some(scheduled_start_at) = stored.board.scheduled_start_at else {
            stored.board.status = "paused".to_string();
            stored.board.paused_at = Some(now);
            stored.board.pause_reason = Some("Scheduled start time was invalid".to_string());
            stored
                .board
                .append_log("Scheduled start time was invalid; board paused");
            stored.board.touch();
            save_board(state, &stored.board)?;
            continue;
        };
        if scheduled_start_at > now {
            continue;
        }
        let Some(user_id) = stored.board.user_id.clone() else {
            continue;
        };
        stored.board.scheduled_start_at = None;
        stored.board.append_log("Scheduled start time reached");
        stored.board.touch();
        save_board(state, &stored.board)?;
        let _ = start_board_execution(state, &user_id, &stored.board.id)?;
    }
    Ok(())
}

async fn process_auto_retries(state: &AppState) -> Result<()> {
    let now = Utc::now();
    for mut stored in load_boards(state)? {
        if stored.board.loop_started || !auto_retry_enabled(&stored.board.auto_retry) {
            continue;
        }
        if !is_resumable_board(&stored.board) {
            continue;
        }
        let retry_state = normalize_auto_retry(&stored.board.auto_retry);
        let attempts = retry_state
            .get("attempts")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let previous_last_retry_at = retry_state
            .get("lastRetryAt")
            .cloned()
            .unwrap_or(Value::Null);
        let max_attempts = retry_state
            .get("maxAttempts")
            .and_then(Value::as_u64)
            .unwrap_or(3);
        if attempts >= max_attempts {
            stored.board.auto_retry = merge_auto_retry(
                retry_state,
                json!({
                    "nextRetryAt": null,
                    "lastError": format!("Max auto retries reached ({attempts}/{max_attempts})"),
                    "updatedAt": now,
                }),
            );
            stored
                .board
                .append_log("Auto retry stopped: max attempts reached");
            stored.board.touch();
            save_board(state, &stored.board)?;
            continue;
        }
        let next_retry_at = retry_state
            .get("nextRetryAt")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_utc);
        if next_retry_at.is_none() {
            schedule_auto_retry_if_eligible(&mut stored.board, "resumable status");
            stored.board.touch();
            save_board(state, &stored.board)?;
            continue;
        }
        if next_retry_at.is_some_and(|time| time > now) {
            continue;
        }
        let Some(user_id) = stored.board.user_id.clone() else {
            continue;
        };
        let retry_state = normalize_auto_retry(&stored.board.auto_retry);
        stored.board.auto_retry = merge_auto_retry(
            retry_state,
            json!({
                "attempts": attempts + 1,
                "nextRetryAt": null,
                "lastRetryAt": now,
                "lastError": "",
                "updatedAt": now,
            }),
        );
        stored.board.append_log(format!(
            "Auto retry {}/{} starting",
            attempts + 1,
            max_attempts
        ));
        let reset_count = reset_attention_tasks_for_retry(&mut stored.board);
        if reset_count == 0 {
            // The board may have changed between scheduling and the poller
            // waking up (for example, a user resolved the failure). Do not
            // start an empty execution loop or consume an auto-retry attempt
            // when there is no longer a transient task to retry.
            stored.board.auto_retry = merge_auto_retry(
                normalize_auto_retry(&stored.board.auto_retry),
                json!({
                    "attempts": attempts,
                    "nextRetryAt": null,
                    "lastRetryAt": previous_last_retry_at,
                    "lastError": "No transient failed subtasks remain to retry.",
                    "updatedAt": now,
                }),
            );
            stored
                .board
                .append_log("Auto retry skipped: no transient attention tasks remain");
            stored.board.touch();
            save_board(state, &stored.board)?;
            continue;
        }
        stored.board.touch();
        save_board(state, &stored.board)?;
        let _ = start_board_execution(state, &user_id, &stored.board.id)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoardsQuery {
    project_path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBoardRequest {
    command: Option<String>,
    prompt: Option<String>,
    title: Option<String>,
    details: Option<String>,
    description: Option<String>,
    project_path: Option<String>,
    project_name: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    next_model: Option<String>,
    next_provider: Option<String>,
    model_strategy: Option<Value>,
    board_profile: Option<String>,
    task_model_overrides: Option<Value>,
    session_policy: Option<String>,
    git_policy: Option<String>,
    tools_settings: Option<Value>,
    tdd_enabled: Option<bool>,
    tdd_policy: Option<Value>,
    validation_config: Option<Value>,
    rag_settings: Option<Value>,
    qa_policy: Option<Value>,
    auto_retry: Option<Value>,
    scheduled_start_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskRequest {
    prompt: Option<String>,
    command: Option<String>,
    title: Option<String>,
    details: Option<String>,
    description: Option<String>,
    kind: Option<String>,
    task_type: Option<String>,
    level: Option<String>,
    parent_id: Option<String>,
    blocked_by: Option<Value>,
    executable: Option<bool>,
    required: Option<bool>,
    source_task_id: Option<String>,
    scope_version: Option<u64>,
    planned_files: Option<Value>,
    side_effects: Option<Value>,
    acceptance_criteria: Option<Value>,
    acceptance: Option<Value>,
    criteria: Option<Value>,
    references: Option<Value>,
    files: Option<Value>,
    paths: Option<Value>,
    priority: Option<String>,
    rank: Option<i64>,
    depends_on: Option<Value>,
    dependencies: Option<Value>,
    status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptRequest {
    prompt: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    board_profile: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetryTasksRequest {
    task_ids: Option<Vec<String>>,
    mode: Option<String>,
    fix_task_id: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SideEffectsApprovalRequest {
    approved: Option<bool>,
    note: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResearchAcceptanceRequest {
    items: Option<Value>,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScopeEffectsResolutionRequest {
    decision: String,
    note: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateTaskRequest {
    status: Option<String>,
    title: Option<String>,
    details: Option<String>,
    description: Option<String>,
    kind: Option<String>,
    task_type: Option<String>,
    level: Option<String>,
    parent_id: Option<String>,
    acceptance_criteria: Option<Value>,
    acceptance: Option<Value>,
    criteria: Option<Value>,
    priority: Option<String>,
    rank: Option<i64>,
    blocked_by: Option<Value>,
    depends_on: Option<Value>,
    dependencies: Option<Value>,
    required: Option<bool>,
    planned_files: Option<Value>,
    side_effects: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscussionRequest {
    message: Option<String>,
    action: Option<String>,
    payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct PauseRequest {
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleRequest {
    scheduled_start_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateModelRequest {
    provider: Option<String>,
    model: Option<String>,
    next_model: Option<String>,
    next_provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgenticBoard {
    id: String,
    #[serde(default = "default_orchestration_version")]
    orchestration_version: u32,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default = "default_provider_string")]
    provider: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    primary_model: String,
    #[serde(default)]
    next_model: String,
    #[serde(default)]
    next_provider: String,
    #[serde(default)]
    last_effective_model: Option<String>,
    #[serde(default)]
    model_history: Vec<Value>,
    #[serde(default)]
    model_strategy: Option<Value>,
    #[serde(default = "default_board_profile")]
    board_profile: String,
    #[serde(default)]
    task_model_overrides: Value,
    #[serde(default)]
    session_policy: String,
    #[serde(default = "default_git_policy")]
    git_policy: String,
    #[serde(default)]
    tools_settings: Option<Value>,
    #[serde(default)]
    quality_profile: Value,
    #[serde(default = "default_validation_config")]
    validation_config: Value,
    #[serde(default = "default_rag_settings")]
    rag_settings: Value,
    #[serde(default = "default_qa_policy")]
    qa_policy: Value,
    #[serde(default)]
    project_path: String,
    #[serde(default)]
    project_name: String,
    #[serde(default)]
    source_prompt: String,
    #[serde(default = "default_paused_status")]
    status: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    actual_session_id: Option<String>,
    #[serde(default = "Utc::now")]
    created_at: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    updated_at: DateTime<Utc>,
    #[serde(default)]
    current_task_id: Option<String>,
    #[serde(default)]
    current_task_title: String,
    #[serde(default)]
    current_task_status: String,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    loop_started: bool,
    #[serde(default)]
    auto_run_enabled: bool,
    #[serde(default)]
    control_revision: u64,
    #[serde(default)]
    pause_requested: bool,
    #[serde(default)]
    paused_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pause_reason: Option<String>,
    #[serde(default)]
    cancellation_reason: Option<String>,
    #[serde(default)]
    abort_source: Option<String>,
    #[serde(default)]
    abort_requested_at: Option<DateTime<Utc>>,
    #[serde(default)]
    canceled_at: Option<DateTime<Utc>>,
    #[serde(default)]
    scheduled_start_at: Option<DateTime<Utc>>,
    #[serde(default)]
    current_phase: Option<String>,
    #[serde(default)]
    phase_started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    phase_details: Option<Value>,
    #[serde(default)]
    current_provider_session_id: Option<String>,
    #[serde(default)]
    provider_call_started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    provider_call_label: Option<String>,
    #[serde(default)]
    final_qa_complete: bool,
    #[serde(default)]
    auto_retry: Value,
    #[serde(default = "default_backlog_breakdown")]
    backlog_breakdown: Value,
    #[serde(default)]
    logs: Vec<String>,
    #[serde(default)]
    next_task_sequence: u64,
    #[serde(default)]
    tasks: Vec<BoardTask>,
    #[serde(default)]
    source_references: Vec<Value>,
    #[serde(default)]
    source_manifest: Vec<Value>,
    #[serde(default)]
    source_chunks: Vec<Value>,
    #[serde(default)]
    codebase_manifest: Vec<Value>,
    #[serde(default)]
    codebase_chunks: Vec<Value>,
    #[serde(default)]
    codebase_understanding: Vec<Value>,
    #[serde(default)]
    codebase_map: Option<Value>,
    #[serde(default)]
    agents_context: Option<Value>,
    #[serde(default)]
    workspace_baseline: Option<Value>,
    #[serde(default)]
    latest_workspace_snapshot: Option<Value>,
    #[serde(default)]
    environment_state: Option<Value>,
    #[serde(default)]
    prompt_telemetry: Vec<Value>,
    #[serde(default)]
    discussion_proposals: Vec<Value>,
    #[serde(default = "default_provider_usage")]
    provider_usage: Value,
    #[serde(default)]
    provider_usage_by_session: Value,
    #[serde(default)]
    compaction_ledger: Vec<Value>,
    #[serde(default)]
    change_ledger: Vec<Value>,
    #[serde(default)]
    git_ledger: Vec<Value>,
    #[serde(default)]
    validation_runs: Vec<Value>,
    #[serde(default)]
    rag_enabled: bool,
    #[serde(default)]
    rag_service_url: Option<String>,
    #[serde(default)]
    rag_queries: Vec<Value>,
    #[serde(default)]
    rag_ingestions: Vec<Value>,
    #[serde(default)]
    rag_trace_refs: Vec<Value>,
    #[serde(default)]
    tdd_enabled: bool,
    #[serde(default = "default_tdd_policy")]
    tdd_policy: Value,
    #[serde(default)]
    qa_artifacts: Vec<Value>,
    #[serde(default)]
    promotion_candidates: Vec<Value>,
    #[serde(default)]
    planning_round: u32,
    #[serde(default)]
    review_round: u32,
    #[serde(default)]
    bootstrap_complete: bool,
    #[serde(default)]
    agents_knowledge_updated: bool,
    #[serde(default)]
    final_review: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoardTaskHierarchy {
    #[serde(default = "default_task_level")]
    level: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    blocked_by: Vec<String>,
    #[serde(default)]
    executable: bool,
    #[serde(default = "default_required_task")]
    required: bool,
    #[serde(default)]
    scope_version: u64,
    #[serde(default)]
    rank: i64,
    #[serde(default)]
    attempts: Vec<Value>,
    #[serde(default)]
    planned_files: Vec<String>,
    #[serde(default)]
    side_effects: Vec<String>,
    #[serde(default)]
    side_effects_approved: bool,
    #[serde(default)]
    side_effect_approval: Option<Value>,
    #[serde(default)]
    side_effect_evidence: Vec<String>,
    #[serde(default)]
    manual_test_environment: Option<Value>,
    #[serde(default)]
    research_accepted: bool,
    #[serde(default)]
    research_acceptance: Option<Value>,
    #[serde(default)]
    discussion: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoardTask {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    details: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    acceptance_criteria: Vec<String>,
    #[serde(default)]
    references: Vec<String>,
    #[serde(default = "default_priority")]
    priority: String,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    manual_task: bool,
    #[serde(default)]
    prompt_task: bool,
    #[serde(default)]
    task_origin: String,
    #[serde(default = "default_task_type")]
    task_type: String,
    #[serde(default)]
    backlog_generation_task: bool,
    #[serde(default)]
    qa_task: bool,
    #[serde(default)]
    final_qa_task: bool,
    #[serde(default)]
    followup_task: bool,
    #[serde(default)]
    qa_fix_task: bool,
    #[serde(default)]
    qa_verdict_retry_task: bool,
    #[serde(default)]
    task_level_qa: bool,
    #[serde(default)]
    agents_knowledge_task: bool,
    #[serde(default)]
    internal_validation: bool,
    #[serde(default)]
    qa_round: u32,
    #[serde(default)]
    source_task_id: Option<String>,
    #[serde(default)]
    source_qa_task_id: Option<String>,
    #[serde(default)]
    superseded_by: Option<String>,
    #[serde(default)]
    transcript: Vec<Value>,
    #[serde(default)]
    transcript_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    qa_passed: Option<bool>,
    #[serde(default)]
    attempt_count: u32,
    #[serde(default)]
    provider_session_id: Option<String>,
    #[serde(default)]
    commands_run: Vec<String>,
    #[serde(default)]
    changed_files: Vec<String>,
    #[serde(default)]
    changed_file_summary: Option<Value>,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    remaining_issues: Vec<String>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    result_validation: Option<Value>,
    #[serde(default)]
    deterministic_validation: Option<Value>,
    #[serde(default)]
    rag_context_refs: Vec<Value>,
    #[serde(default)]
    rag_prompt_context: String,
    #[serde(default = "default_tdd_phase")]
    tdd_phase: String,
    #[serde(default)]
    qa_test_paths: Vec<String>,
    #[serde(default)]
    qa_test_commands: Vec<String>,
    #[serde(default)]
    qa_baseline_validation: Option<Value>,
    #[serde(default)]
    fix_attempts: u32,
    #[serde(default)]
    coverage_evidence: Vec<Value>,
    #[serde(default)]
    group_id: Option<String>,
    #[serde(flatten)]
    hierarchy: BoardTaskHierarchy,
}

impl AgenticBoard {
    fn new(user_id: Option<String>, request: CreateBoardRequest) -> Result<Self> {
        let prompt = trim_string(request.command.or(request.prompt))
            .ok_or_else(|| bad_request("Prompt is required"))?;
        let project_path = trim_string(request.project_path)
            .ok_or_else(|| bad_request("Project path is required"))?;
        let provider = normalize_provider(request.provider.as_deref())?;
        let model_strategy = normalize_model_strategy(request.model_strategy);
        let strategy_overrides = task_model_overrides_for_strategy(model_strategy.as_ref());
        let request_overrides = normalize_task_model_overrides(
            request.task_model_overrides.unwrap_or_else(|| json!({})),
        );
        let task_model_overrides =
            merge_task_model_overrides(strategy_overrides, request_overrides);
        let model = trim_string(request.model)
            .or_else(|| primary_model_for_strategy(model_strategy.as_ref()))
            .unwrap_or_else(|| default_model_for_provider(&provider));
        let board_profile = normalize_board_profile_for_strategy(
            request.board_profile.as_deref(),
            model_strategy.as_ref(),
        );
        let validation_config = normalize_validation_config(request.validation_config.as_ref());
        let rag_settings = normalize_rag_settings(request.rag_settings.as_ref());
        let qa_policy = normalize_qa_policy(request.qa_policy.as_ref());
        let auto_retry = normalize_auto_retry(request.auto_retry.as_ref().unwrap_or(&Value::Null));
        let now = Utc::now();
        let requested_scheduled_start_at =
            parse_optional_scheduled_start(request.scheduled_start_at.as_deref())?;
        let should_schedule = requested_scheduled_start_at.is_some_and(|time| time > now);
        let scheduled_start_at = if should_schedule {
            requested_scheduled_start_at
        } else {
            None
        };
        let mut run = Self {
            id: Uuid::new_v4().to_string(),
            orchestration_version: 3,
            user_id,
            provider,
            model: model.clone(),
            primary_model: model,
            next_model: trim_string(request.next_model).unwrap_or_default(),
            next_provider: normalize_optional_provider(request.next_provider.as_deref())?,
            last_effective_model: None,
            model_history: Vec::new(),
            model_strategy,
            board_profile,
            task_model_overrides,
            session_policy: normalize_session_policy(request.session_policy.as_deref()),
            git_policy: normalize_git_policy(request.git_policy.as_deref()),
            tools_settings: request.tools_settings,
            quality_profile: json!({}),
            validation_config,
            rag_settings: rag_settings.clone(),
            qa_policy,
            project_name: trim_string(request.project_name)
                .unwrap_or_else(|| project_name_from_path(&project_path)),
            project_path,
            source_prompt: prompt.clone(),
            status: if should_schedule {
                "scheduled".to_string()
            } else {
                "paused".to_string()
            },
            session_id: None,
            actual_session_id: None,
            created_at: now,
            updated_at: now,
            current_task_id: None,
            current_task_title: String::new(),
            current_task_status: String::new(),
            active: false,
            loop_started: false,
            auto_run_enabled: false,
            control_revision: 0,
            pause_requested: false,
            paused_at: if should_schedule { None } else { Some(now) },
            pause_reason: if should_schedule {
                None
            } else {
                Some("Board created with backlog planning item.".to_string())
            },
            cancellation_reason: None,
            abort_source: None,
            abort_requested_at: None,
            canceled_at: None,
            scheduled_start_at,
            current_phase: Some("board".to_string()),
            phase_started_at: Some(now),
            phase_details: Some(json!({ "mode": "kanban_only" })),
            current_provider_session_id: None,
            provider_call_started_at: None,
            provider_call_label: None,
            final_qa_complete: false,
            auto_retry,
            backlog_breakdown: default_backlog_breakdown(),
            logs: vec!["Created agentic board".to_string()],
            next_task_sequence: 0,
            tasks: Vec::new(),
            source_references: Vec::new(),
            source_manifest: Vec::new(),
            source_chunks: Vec::new(),
            codebase_manifest: Vec::new(),
            codebase_chunks: Vec::new(),
            codebase_understanding: Vec::new(),
            codebase_map: None,
            agents_context: None,
            workspace_baseline: None,
            latest_workspace_snapshot: None,
            environment_state: None,
            prompt_telemetry: Vec::new(),
            discussion_proposals: Vec::new(),
            provider_usage: default_provider_usage(),
            provider_usage_by_session: json!({}),
            compaction_ledger: Vec::new(),
            change_ledger: Vec::new(),
            git_ledger: Vec::new(),
            validation_runs: Vec::new(),
            rag_enabled: rag_enabled_from_settings(&rag_settings),
            rag_service_url: RagClient::configured_descriptor(),
            rag_queries: Vec::new(),
            rag_ingestions: Vec::new(),
            rag_trace_refs: Vec::new(),
            tdd_enabled: request.tdd_enabled.unwrap_or_else(default_tdd_enabled),
            tdd_policy: normalize_tdd_policy(request.tdd_policy.as_ref()),
            qa_artifacts: Vec::new(),
            promotion_candidates: Vec::new(),
            planning_round: 0,
            review_round: 0,
            bootstrap_complete: false,
            agents_knowledge_updated: false,
            final_review: None,
        };
        sync_session_policy_with_task_models(&mut run, "board creation");
        let task = BoardTask::manual(
            &mut run,
            TaskRequest {
                prompt: Some(prompt),
                command: None,
                title: request.title,
                details: request.details,
                description: request.description,
                kind: None,
                task_type: None,
                level: None,
                parent_id: None,
                blocked_by: None,
                executable: None,
                required: None,
                source_task_id: None,
                scope_version: None,
                planned_files: None,
                side_effects: None,
                acceptance_criteria: None,
                acceptance: None,
                criteria: None,
                references: None,
                files: None,
                paths: None,
                priority: None,
                rank: None,
                depends_on: None,
                dependencies: None,
                status: Some(TASK_STATUS_TODO.to_string()),
            },
        )?;
        let task = {
            let mut task = task;
            task.status = TASK_STATUS_BACKLOG.to_string();
            task
        };
        run.tasks.push(task);
        Ok(run)
    }

    fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    fn append_log(&mut self, message: impl Into<String>) {
        self.logs
            .push(format!("{} {}", Utc::now().to_rfc3339(), message.into()));
        if self.logs.len() > 500 {
            let remove_count = self.logs.len() - 500;
            self.logs.drain(0..remove_count);
        }
    }

    fn summary_json(&self, file_path: Option<String>) -> Value {
        let task_counts = task_counts(&self.tasks, self.orchestration_version);
        let task_group_counts = task_group_counts(self);
        let mut value = json!({
            "id": self.id,
            "orchestrationVersion": self.orchestration_version,
            "provider": self.provider,
            "model": self.model,
            "primaryModel": self.primary_model,
            "nextModel": self.next_model,
            "nextProvider": self.next_provider,
            "lastEffectiveModel": self.last_effective_model,
            "modelStrategy": self.model_strategy,
            "boardProfile": self.board_profile,
            "sessionPolicy": self.session_policy,
            "gitPolicy": self.git_policy,
            "modelHistory": self.model_history,
            "taskModelOverrides": self.task_model_overrides,
            "validationConfig": self.validation_config,
            "ragSettings": self.rag_settings,
            "qaPolicy": self.qa_policy,
            "projectPath": self.project_path,
            "projectName": self.project_name,
            "sourcePrompt": self.source_prompt,
            "status": self.status,
            "sessionId": self.session_id,
            "actualSessionId": self.actual_session_id,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
            "currentTaskId": self.current_task_id,
            "currentTaskTitle": self.current_task_title,
            "currentTaskStatus": self.current_task_status,
            "taskCounts": task_counts,
            "taskGroupCounts": task_group_counts,
            "sourceFileCount": self.source_manifest.len(),
            "sourceChunkCount": self.source_chunks.len(),
            "codebaseFileCount": self.codebase_manifest.len(),
            "codebaseChunkCount": self.codebase_chunks.len(),
            "codebaseUnderstandingCount": self.codebase_understanding.len(),
            "finalQaComplete": self.final_qa_complete,
            "bootstrapComplete": self.bootstrap_complete,
            "planningRound": self.planning_round,
            "reviewRound": self.review_round,
            "active": self.active,
            "loopStarted": self.loop_started,
            "autoRunEnabled": self.auto_run_enabled,
            "controlRevision": self.control_revision,
            "pauseRequested": self.pause_requested,
            "pausedAt": self.paused_at,
            "pauseReason": self.pause_reason,
            "cancellationReason": self.cancellation_reason,
            "abortSource": self.abort_source,
            "abortRequestedAt": self.abort_requested_at,
            "canceledAt": self.canceled_at,
            "scheduledStartAt": self.scheduled_start_at,
            "currentPhase": self.current_phase,
            "phaseStartedAt": self.phase_started_at,
            "phaseDetails": self.phase_details,
            "currentProviderSessionId": self.current_provider_session_id,
            "providerCallStartedAt": self.provider_call_started_at,
            "providerCallLabel": self.provider_call_label,
            "promptTelemetrySummary": prompt_telemetry_summary(&self.prompt_telemetry),
            "discussionProposalCount": self.discussion_proposals.len(),
            "providerUsage": self.provider_usage,
            "providerUsageBySession": self.provider_usage_by_session,
            "compactionLedger": self.compaction_ledger,
            "validationSummary": validation_summary(&self.validation_runs),
            "ragEnabled": self.rag_enabled,
            "ragServiceUrl": self.rag_service_url,
            "ragQueryCount": self.rag_queries.len(),
            "ragIngestionCount": self.rag_ingestions.len(),
            "tddEnabled": self.tdd_enabled,
            "tddPolicy": self.tdd_policy,
            "qaArtifactCount": self.qa_artifacts.len(),
            "promotionCandidateCount": self.promotion_candidates.len(),
            "resumable": is_resumable_board(self),
            "toolsSettings": self.tools_settings,
            "autoRetry": self.auto_retry,
            "backlogBreakdown": self.backlog_breakdown,
            "filePath": file_path,
        });
        sanitize_kanban_structure(&value)
    }

    fn detail_json(&self, file_path: Option<String>) -> Value {
        let mut value = self.summary_json(file_path);
        if let Some(object) = value.as_object_mut() {
            object.insert("logs".to_string(), json!(self.logs));
            object.insert("tasks".to_string(), tasks_detail_json(&self.tasks));
            object.insert("taskGroups".to_string(), task_groups_detail_json(self));
            object.insert("sourceManifest".to_string(), json!(self.source_manifest));
            object.insert(
                "sourceReferences".to_string(),
                json!(self.source_references),
            );
            object.insert("sourceChunks".to_string(), json!(self.source_chunks));
            object.insert(
                "codebaseManifest".to_string(),
                json!(self.codebase_manifest),
            );
            object.insert("codebaseChunks".to_string(), json!(self.codebase_chunks));
            object.insert(
                "codebaseUnderstanding".to_string(),
                json!(self.codebase_understanding),
            );
            object.insert("codebaseMap".to_string(), json!(self.codebase_map));
            object.insert("agentsContext".to_string(), json!(self.agents_context));
            object.insert(
                "workspaceBaseline".to_string(),
                json!(self.workspace_baseline),
            );
            object.insert(
                "latestWorkspaceSnapshot".to_string(),
                json!(self.latest_workspace_snapshot),
            );
            object.insert(
                "environmentState".to_string(),
                json!(self.environment_state),
            );
            object.insert("promptTelemetry".to_string(), json!(self.prompt_telemetry));
            object.insert(
                "discussionProposals".to_string(),
                Value::Array(
                    self.discussion_proposals
                        .iter()
                        .map(sanitize_kanban_value)
                        .collect(),
                ),
            );
            object.insert("providerUsage".to_string(), json!(self.provider_usage));
            object.insert(
                "providerUsageBySession".to_string(),
                json!(self.provider_usage_by_session),
            );
            object.insert(
                "compactionLedger".to_string(),
                json!(self.compaction_ledger),
            );
            object.insert("changeLedger".to_string(), json!(self.change_ledger));
            object.insert("gitLedger".to_string(), json!(self.git_ledger));
            object.insert("validationRuns".to_string(), json!(self.validation_runs));
            object.insert("ragQueries".to_string(), json!(self.rag_queries));
            object.insert("ragIngestions".to_string(), json!(self.rag_ingestions));
            object.insert("ragTraceRefs".to_string(), json!(self.rag_trace_refs));
            object.insert("tddPolicy".to_string(), json!(self.tdd_policy));
            object.insert("qaArtifacts".to_string(), json!(self.qa_artifacts));
            object.insert(
                "promotionCandidates".to_string(),
                json!(self.promotion_candidates),
            );
            object.insert("finalReview".to_string(), json!(self.final_review));
        }
        sanitize_kanban_structure(&value)
    }
}

fn tasks_detail_json(tasks: &[BoardTask]) -> Value {
    Value::Array(
        tasks
            .iter()
            .filter(|task| !task.backlog_generation_task)
            .map(task_detail_json)
            .collect(),
    )
}

fn task_groups_detail_json(run: &AgenticBoard) -> Value {
    let mut groups: Vec<(String, Vec<&BoardTask>)> = Vec::new();
    for task in run
        .tasks
        .iter()
        .filter(|task| task_is_visible_work_item(task, run.orchestration_version))
    {
        let group_id = task_group_id_or_self(task);
        if let Some((_, group_tasks)) = groups.iter_mut().find(|(id, _)| id == &group_id) {
            group_tasks.push(task);
        } else {
            groups.push((group_id, vec![task]));
        }
    }
    Value::Array(
        groups
            .into_iter()
            .map(|(group_id, group_tasks)| task_group_detail_json(run, &group_id, &group_tasks))
            .collect(),
    )
}

fn task_group_detail_json(run: &AgenticBoard, group_id: &str, tasks: &[&BoardTask]) -> Value {
    let primary = tasks
        .iter()
        .copied()
        .find(|task| is_kanban_parent_task(task))
        .or_else(|| tasks.first().copied());
    let current = tasks
        .iter()
        .copied()
        .find(|task| task_status_is_active(&task.status))
        .or_else(|| {
            tasks
                .iter()
                .copied()
                .find(|task| task_status_is_todo(&task.status))
        })
        .or_else(|| {
            tasks.iter().copied().find(|task| {
                matches!(
                    canonical_task_status(&task.status),
                    TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
                )
            })
        })
        .or(primary);
    let status = task_group_status_for_board(run, tasks);
    let title = primary
        .map(task_title_for_group)
        .unwrap_or_else(|| "Work item".to_string());
    let priority = primary
        .map(|task| normalize_priority(Some(&task.priority)).to_string())
        .unwrap_or_else(|| TASK_PRIORITY_P2.to_string());
    let completed = tasks
        .iter()
        .filter(|task| task_status_is_done(&task.status))
        .count();
    json!({
        "id": group_id,
        "title": title,
        "status": status,
        "priority": priority,
        "primaryTaskId": primary.map(|task| task.id.clone()),
        "currentSubtaskId": current.map(|task| task.id.clone()),
        "currentSubtaskKind": current.map(canonical_task_kind),
        "currentSubtaskTitle": current.map(task_title_for_group),
        "subtaskCounts": count_statuses(tasks.iter().map(|task| task.status.as_str())),
        "completedSubtasks": completed,
        "totalSubtasks": tasks.len(),
        "taskIds": tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>(),
        "subtasks": tasks.iter().map(|task| task_detail_json(task)).collect::<Vec<_>>(),
    })
}

fn task_group_status(tasks: &[&BoardTask]) -> &'static str {
    let required = tasks
        .iter()
        .filter(|task| task.hierarchy.required)
        .copied()
        .collect::<Vec<_>>();
    let relevant = if required.is_empty() {
        tasks
    } else {
        &required
    };
    if relevant
        .iter()
        .any(|task| task_status_is_active(&task.status))
    {
        TASK_STATUS_IN_PROGRESS
    } else if relevant.iter().any(|task| {
        task_status_is_todo(&task.status)
            && task_blockers(task).is_empty()
            && task_side_effects_are_approved(task)
    }) {
        TASK_STATUS_TODO
    } else if relevant
        .iter()
        .any(|task| canonical_task_status(&task.status) == TASK_STATUS_BLOCKED)
    {
        TASK_STATUS_BLOCKED
    } else if relevant
        .iter()
        .any(|task| canonical_task_status(&task.status) == TASK_STATUS_FAILED)
    {
        TASK_STATUS_FAILED
    } else if !relevant.is_empty()
        && relevant
            .iter()
            .all(|task| canonical_task_status(&task.status) == TASK_STATUS_BACKLOG)
    {
        TASK_STATUS_BACKLOG
    } else if relevant
        .iter()
        .any(|task| canonical_task_status(&task.status) == TASK_STATUS_TODO)
    {
        TASK_STATUS_TODO
    } else if !relevant.is_empty()
        && relevant.iter().all(|task| {
            task_status_is_done(&task.status)
                && !(canonical_task_kind(task) == TASK_KIND_RESEARCH
                    && !task.hierarchy.research_accepted)
        })
    {
        TASK_STATUS_DONE
    } else {
        TASK_STATUS_BACKLOG
    }
}

fn task_group_status_for_board(run: &AgenticBoard, tasks: &[&BoardTask]) -> &'static str {
    let required = tasks
        .iter()
        .filter(|task| task.hierarchy.required)
        .copied()
        .collect::<Vec<_>>();
    let relevant = if required.is_empty() {
        tasks
    } else {
        &required
    };
    let all_done = relevant
        .iter()
        .all(|task| task_rollup_completion_is_satisfied(task));
    let has_active = relevant
        .iter()
        .any(|task| task_status_is_active(&task.status) && task_ancestors_are_approved(run, task));
    let has_child_work = relevant.len() > 1;
    // A root-level system subtask can still belong to a feature group (older
    // QA/follow-up cards used source links instead of a structural parent).
    // Eligibility is defined by the task contract, not by whether the parent
    // id happens to be populated.
    let has_eligible = relevant
        .iter()
        .any(|task| has_child_work && task_rollup_child_is_eligible(run, task));
    let all_remaining_blocked = relevant
        .iter()
        .filter(|task| !task_rollup_completion_is_satisfied(task))
        .all(|task| task_rollup_child_is_blocked(run, task));
    if all_done {
        TASK_STATUS_DONE
    } else if has_active || has_eligible {
        TASK_STATUS_IN_PROGRESS
    } else if all_remaining_blocked {
        TASK_STATUS_BLOCKED
    } else if !relevant.is_empty()
        && relevant
            .iter()
            .all(|task| task_status_is_backlog(&task.status))
    {
        TASK_STATUS_BACKLOG
    } else {
        TASK_STATUS_TODO
    }
}

fn task_detail_json(task: &BoardTask) -> Value {
    let mut value = serde_json::to_value(task).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "status".to_string(),
            json!(canonical_task_status(&task.status)),
        );
        let kind = canonical_task_kind(task);
        object.insert("kind".to_string(), json!(kind));
        object.insert("taskType".to_string(), json!(kind));
        object.insert("level".to_string(), json!(task_level(task)));
        object.insert("executable".to_string(), json!(task_is_executable(task)));
        object.insert("blockedBy".to_string(), json!(task_blockers(task)));
        object.insert("groupId".to_string(), json!(task_group_id_or_self(task)));
        object.insert(
            "requiresSideEffectDeclaration".to_string(),
            json!(task_requires_external_side_effect_declaration(task)),
        );
        object.insert(
            "superseded".to_string(),
            json!(task.superseded_by.is_some()),
        );
    }
    sanitize_kanban_value(&value)
}

impl BoardTask {
    fn manual(run: &mut AgenticBoard, request: TaskRequest) -> Result<Self> {
        let task_type = normalize_task_kind(
            request.kind.as_deref().or(request.task_type.as_deref()),
            TASK_KIND_IMPLEMENTATION,
        );
        let parent_id = trim_string(request.parent_id.clone());
        let requested_level = request
            .level
            .as_deref()
            .map(|value| normalize_task_level(Some(value), TASK_LEVEL_STORY));
        let level = if let Some(parent_id) = parent_id.as_deref() {
            run.tasks
                .iter()
                .find(|candidate| candidate.id == parent_id)
                .and_then(|parent| next_hierarchy_level(task_level(parent)))
                .unwrap_or_else(|| requested_level.unwrap_or(TASK_LEVEL_SUBTASK))
        } else {
            match requested_level.unwrap_or(TASK_LEVEL_STORY) {
                TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK => TASK_LEVEL_STORY,
                level => level,
            }
        };
        let prompt = trim_string(request.prompt.or(request.command)).unwrap_or_default();
        let title = trim_string(request.title)
            .or_else(|| title_from_prompt(&prompt))
            .ok_or_else(|| bad_request("Manual task title or prompt is required."))?;
        let details =
            trim_string(request.details.or(request.description)).unwrap_or_else(|| prompt.clone());
        let status = normalize_task_status(request.status.as_deref(), "backlog")?;
        let priority =
            task_priority_for_parent(run, parent_id.as_deref(), request.priority.as_deref());
        let id = allocate_task_id(run);
        let references = [request.references, request.files, request.paths]
            .into_iter()
            .flat_map(value_to_strings)
            .collect();
        let depends_on = value_to_strings(request.depends_on.or(request.dependencies));
        let blocked_by = value_to_strings(request.blocked_by)
            .into_iter()
            .chain(depends_on.iter().cloned())
            .collect::<Vec<_>>();
        Ok(Self {
            id: id.clone(),
            title,
            status,
            summary: String::new(),
            details: details.clone(),
            description: details,
            prompt,
            error: None,
            acceptance_criteria: value_to_strings(
                request
                    .acceptance_criteria
                    .or(request.acceptance)
                    .or(request.criteria),
            ),
            references,
            priority,
            depends_on,
            manual_task: true,
            prompt_task: false,
            task_origin: "user_manual".to_string(),
            task_type: task_type.to_string(),
            backlog_generation_task: false,
            qa_task: false,
            final_qa_task: false,
            followup_task: false,
            qa_fix_task: false,
            qa_verdict_retry_task: false,
            task_level_qa: false,
            agents_knowledge_task: false,
            internal_validation: false,
            qa_round: 0,
            source_task_id: trim_string(request.source_task_id),
            source_qa_task_id: None,
            superseded_by: None,
            transcript: Vec::new(),
            transcript_updated_at: None,
            started_at: None,
            completed_at: None,
            qa_passed: None,
            attempt_count: 0,
            provider_session_id: None,
            commands_run: Vec::new(),
            changed_files: Vec::new(),
            changed_file_summary: None,
            evidence: Vec::new(),
            remaining_issues: Vec::new(),
            result: None,
            result_validation: None,
            deterministic_validation: None,
            rag_context_refs: Vec::new(),
            rag_prompt_context: String::new(),
            tdd_phase: default_tdd_phase(),
            qa_test_paths: Vec::new(),
            qa_test_commands: Vec::new(),
            qa_baseline_validation: None,
            fix_attempts: 0,
            coverage_evidence: Vec::new(),
            group_id: Some(id),
            hierarchy: BoardTaskHierarchy {
                level: level.to_string(),
                parent_id,
                blocked_by,
                executable: request.executable.unwrap_or(level == TASK_LEVEL_SUBTASK)
                    && level == TASK_LEVEL_SUBTASK,
                required: request.required.unwrap_or(true),
                scope_version: request.scope_version.unwrap_or(1),
                rank: request.rank.unwrap_or(0),
                attempts: Vec::new(),
                planned_files: value_to_strings(request.planned_files),
                side_effects: value_to_strings(request.side_effects),
                side_effects_approved: false,
                side_effect_approval: None,
                side_effect_evidence: Vec::new(),
                manual_test_environment: None,
                research_accepted: false,
                research_acceptance: None,
                discussion: Vec::new(),
            },
        })
    }

    fn draft(run: &mut AgenticBoard, title: String, details: String) -> Self {
        let id = allocate_task_id(run);
        Self {
            id: id.clone(),
            title,
            status: "backlog".to_string(),
            summary: String::new(),
            description: details.clone(),
            details,
            prompt: String::new(),
            error: None,
            acceptance_criteria: vec!["Complete the task described by this card.".to_string()],
            references: Vec::new(),
            priority: TASK_PRIORITY_P2.to_string(),
            depends_on: Vec::new(),
            manual_task: false,
            prompt_task: true,
            task_origin: "user_prompt_generated".to_string(),
            task_type: "implementation".to_string(),
            backlog_generation_task: false,
            qa_task: false,
            final_qa_task: false,
            followup_task: false,
            qa_fix_task: false,
            qa_verdict_retry_task: false,
            task_level_qa: false,
            agents_knowledge_task: false,
            internal_validation: false,
            qa_round: 0,
            source_task_id: None,
            source_qa_task_id: None,
            superseded_by: None,
            transcript: Vec::new(),
            transcript_updated_at: None,
            started_at: None,
            completed_at: None,
            qa_passed: None,
            attempt_count: 0,
            provider_session_id: None,
            commands_run: Vec::new(),
            changed_files: Vec::new(),
            changed_file_summary: None,
            evidence: Vec::new(),
            remaining_issues: Vec::new(),
            result: None,
            result_validation: None,
            deterministic_validation: None,
            rag_context_refs: Vec::new(),
            rag_prompt_context: String::new(),
            tdd_phase: default_tdd_phase(),
            qa_test_paths: Vec::new(),
            qa_test_commands: Vec::new(),
            qa_baseline_validation: None,
            fix_attempts: 0,
            coverage_evidence: Vec::new(),
            group_id: Some(id),
            hierarchy: BoardTaskHierarchy {
                level: TASK_LEVEL_STORY.to_string(),
                parent_id: None,
                blocked_by: Vec::new(),
                executable: false,
                required: true,
                scope_version: 1,
                rank: 0,
                attempts: Vec::new(),
                planned_files: Vec::new(),
                side_effects: Vec::new(),
                side_effects_approved: false,
                side_effect_approval: None,
                side_effect_evidence: Vec::new(),
                manual_test_environment: None,
                research_accepted: false,
                research_acceptance: None,
                discussion: Vec::new(),
            },
        }
    }
}

async fn create_board(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(mut request): Json<CreateBoardRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    let project_path = trim_string(request.project_path.clone())
        .ok_or_else(|| bad_request("Project path is required"))?;
    let project_path = state
        .path_validator
        .validate_path(PathBuf::from(project_path), false)
        .await?;
    let metadata = tokio::fs::metadata(&project_path)
        .await
        .map_err(FsError::Io)?;
    if !metadata.is_dir() {
        return Err(bad_request("Project path must be a directory"));
    }
    request.project_path = Some(project_path.display().to_string());

    let request_has_schedule = trim_string(request.scheduled_start_at.clone()).is_some();
    let scheduled_start_at = parse_optional_scheduled_start(request.scheduled_start_at.as_deref())?;
    let should_schedule = scheduled_start_at.is_some_and(|time| time > Utc::now());
    let (board_id, reused) = {
        let _guard = board_mutation_lock();
        let mut reused_board_id = None;
        if let Some(project_path) = trim_string(request.project_path.clone())
            && let Some(mut latest) = latest_board_for_project(&state, &user.0.id, &project_path)?
        {
            let board_was_active = latest.board.loop_started
                || latest.board.active
                || latest.board.status == "running";
            if should_schedule && board_was_active {
                return Err(ServerError::new(
                    StatusCode::CONFLICT,
                    "Pause the active board before scheduling a future start.",
                ));
            }
            apply_board_options(&mut latest.board, &request)?;
            let mut task = BoardTask::manual(
                &mut latest.board,
                TaskRequest {
                    prompt: request.command.clone().or(request.prompt.clone()),
                    command: None,
                    title: request.title.clone(),
                    details: request.details.clone(),
                    description: request.description.clone(),
                    kind: None,
                    task_type: None,
                    level: None,
                    parent_id: None,
                    blocked_by: None,
                    executable: None,
                    required: None,
                    source_task_id: None,
                    scope_version: None,
                    planned_files: None,
                    side_effects: None,
                    acceptance_criteria: None,
                    acceptance: None,
                    criteria: None,
                    references: None,
                    files: None,
                    paths: None,
                    priority: None,
                    rank: None,
                    depends_on: None,
                    dependencies: None,
                    status: Some(TASK_STATUS_TODO.to_string()),
                },
            )?;
            latest.board.tasks.push(task);
            let should_preserve_schedule = !should_schedule
                && !request_has_schedule
                && latest.board.status == "scheduled"
                && latest.board.scheduled_start_at.is_some();
            if should_schedule {
                latest.board.status = "scheduled".to_string();
                latest.board.active = false;
                latest.board.loop_started = false;
                latest.board.auto_run_enabled = false;
                latest.board.scheduled_start_at = scheduled_start_at;
                latest.board.paused_at = None;
                latest.board.pause_reason = None;
            } else if board_was_active {
                latest.board.status = "running".to_string();
                latest.board.active = true;
                latest.board.auto_run_enabled = true;
                latest.board.scheduled_start_at = None;
                latest.board.paused_at = None;
                latest.board.pause_reason = None;
            } else if should_preserve_schedule {
                latest.board.status = "scheduled".to_string();
                latest.board.active = false;
                latest.board.loop_started = false;
                latest.board.auto_run_enabled = false;
                latest.board.paused_at = None;
                latest.board.pause_reason = None;
            } else {
                latest.board.status = "paused".to_string();
                latest.board.active = false;
                latest.board.loop_started = false;
                latest.board.auto_run_enabled = false;
                latest.board.scheduled_start_at = None;
                latest.board.paused_at = Some(Utc::now());
                latest.board.pause_reason = Some("New task added to board.".to_string());
            }
            latest
                .board
                .append_log("Added a task to the project board from start request");
            latest.board.touch();
            let board_id = latest.board.id.clone();
            save_board(&state, &latest.board)?;
            reused_board_id = Some(board_id);
        }

        if let Some(board_id) = reused_board_id {
            (board_id, true)
        } else {
            let run = AgenticBoard::new(Some(user.0.id.clone()), request)?;
            let board_id = run.id.clone();
            save_board(&state, &run)?;
            (board_id, false)
        }
    };
    let stored = load_user_board(&state, &user.0.id, &board_id)?;
    Ok((
        if should_schedule {
            StatusCode::ACCEPTED
        } else if reused {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(
            json!({ "success": true, "reused": reused, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
        ),
    ))
}

async fn list_boards(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<BoardsQuery>,
) -> Result<Json<Value>> {
    let project_path = trim_string(query.project_path);
    let mut boards = load_boards(&state)?
        .into_iter()
        .filter(|stored| stored.board.user_id.as_deref() == Some(&user.0.id))
        .filter(|stored| {
            project_path
                .as_deref()
                .is_none_or(|path| stored.board.project_path == path)
        })
        .collect::<Vec<_>>();
    for stored in &mut boards {
        backfill_board_session_links(&state, &mut stored.board).await?;
    }
    boards.sort_by(|left, right| {
        right
            .board
            .updated_at
            .cmp(&left.board.updated_at)
            .then_with(|| right.board.id.cmp(&left.board.id))
    });
    let mut seen = BTreeMap::<String, ()>::new();
    boards.retain(|stored| seen.insert(stored.board.project_path.clone(), ()).is_none());
    let boards = boards
        .into_iter()
        .map(|stored| {
            stored
                .board
                .summary_json(Some(stored.path.display().to_string()))
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "success": true,
        "boards": boards,
    })))
}

async fn get_board(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>> {
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    backfill_board_session_links(&state, &mut stored.board).await?;
    Ok(Json(
        json!({ "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

/// Lazily classify sessions created before board-session metadata existed.
/// This intentionally runs only while serving a board list/detail request;
/// ordinary session reads remain read-only and never scan board snapshots.
async fn backfill_board_session_links(state: &AppState, run: &mut AgenticBoard) -> Result<()> {
    if run.provider == "cursor" {
        // Cursor board turns are native Cursor CLI sessions, not Workbench
        // sessions, and therefore cannot be opened through chat snapshots.
        return Ok(());
    }
    let mut changed_run = false;
    for (session_id, task_id) in known_board_session_refs(run) {
        let Some(session) = state.storage.get_session_summary(&session_id)? else {
            continue;
        };
        if session.board_id.as_deref().is_some_and(|id| id != run.id) {
            continue;
        }
        state
            .sessions
            .mark_board_session(&session_id, run.id.clone(), task_id.clone())
            .await?;
        if let Some(task_id) = task_id
            && let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id)
            && task_needs_legacy_session_backfill(task)
        {
            task.provider_session_id = Some(session_id);
            changed_run = true;
        }
    }

    let window_start = run.created_at - chrono::Duration::minutes(2);
    let window_end = run.updated_at + chrono::Duration::minutes(2);
    let sessions = state
        .storage
        .list_sessions_including_board()?
        .into_iter()
        .filter(|session| session.project_path == run.project_path)
        .filter(|session| {
            session.last_activity >= window_start && session.last_activity <= window_end
        })
        .collect::<Vec<_>>();
    for session in sessions {
        if session.board_id.as_deref().is_some_and(|id| id != run.id) {
            continue;
        }
        let messages = state.storage.list_messages(&session.id)?;
        let Some(first_user_message) = messages
            .iter()
            .find(|message| message.role == MessageRole::User)
        else {
            continue;
        };
        let prompt = first_user_message.content.as_str();
        let explicit_board = prompt.contains(&format!("Board id: {}", run.id));
        let signature = legacy_board_prompt_signature(prompt);
        if !explicit_board && !signature {
            continue;
        }
        // Signature-only prompts (bootstrap/planning) do not carry a board id.
        // Require proximity to one of this board's recorded provider calls so a
        // normal chat quoting board terminology cannot be classified.
        if !explicit_board
            && !legacy_prompt_matches_board_telemetry(run, first_user_message.timestamp)
        {
            continue;
        }
        let task_id = legacy_board_task_id(run, prompt);
        if !session.is_board_session()
            || session.board_id.as_deref() != Some(run.id.as_str())
            || (task_id.is_some() && session.board_task_id.as_deref() != task_id.as_deref())
        {
            state
                .sessions
                .mark_board_session(&session.id, run.id.clone(), task_id.clone())
                .await?;
        }
        if let Some(task_id) = task_id
            && let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id)
            && task_needs_legacy_session_backfill(task)
        {
            task.provider_session_id = Some(session.id.clone());
            changed_run = true;
        }
    }
    if changed_run {
        // The lazy migration is a deliberate compatibility write. Avoid
        // touching updated_at/control state so loading an old board does not
        // appear as user activity.
        save_board(state, run)?;
    }
    Ok(())
}

fn task_allows_legacy_session_backfill(task: &BoardTask) -> bool {
    matches!(
        canonical_task_status(&task.status),
        TASK_STATUS_BLOCKED | TASK_STATUS_FAILED | TASK_STATUS_DONE
    )
}

fn task_needs_legacy_session_backfill(task: &BoardTask) -> bool {
    task_allows_legacy_session_backfill(task)
        && task
            .provider_session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
}

fn known_board_session_refs(run: &AgenticBoard) -> Vec<(String, Option<String>)> {
    let mut refs = Vec::<(String, Option<String>)>::new();
    for session_id in [
        run.session_id.as_deref(),
        run.actual_session_id.as_deref(),
        run.current_provider_session_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    {
        upsert_known_board_session_ref(&mut refs, session_id.to_string(), None);
    }
    for task in &run.tasks {
        if let Some(session_id) = task
            .provider_session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            upsert_known_board_session_ref(
                &mut refs,
                session_id.to_string(),
                Some(task.id.clone()),
            );
        }
    }
    for entry in run.prompt_telemetry.iter().rev() {
        let Some(session_id) = entry
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let task_id = entry
            .get("label")
            .and_then(Value::as_str)
            .and_then(|label| board_task_id_for_label(run, label));
        upsert_known_board_session_ref(&mut refs, session_id.to_string(), task_id);
    }
    for artifact in run.qa_artifacts.iter().rev() {
        let Some(session_id) = artifact
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let task_id = artifact
            .get("taskId")
            .and_then(Value::as_str)
            .filter(|task_id| run.tasks.iter().any(|task| task.id == *task_id))
            .map(str::to_string);
        upsert_known_board_session_ref(&mut refs, session_id.to_string(), task_id);
    }
    refs
}

fn upsert_known_board_session_ref(
    refs: &mut Vec<(String, Option<String>)>,
    session_id: String,
    task_id: Option<String>,
) {
    if let Some((_, existing_task_id)) = refs
        .iter_mut()
        .find(|(existing_session_id, _)| existing_session_id == &session_id)
    {
        if existing_task_id.is_none() {
            *existing_task_id = task_id;
        }
        return;
    }
    refs.push((session_id, task_id));
}

fn legacy_board_prompt_signature(prompt: &str) -> bool {
    [
        "autonomous Kanban agent",
        "before Kanban planning",
        "io-workbench Kanban board",
        "io-workbench Kanban board worker",
        "agentic Kanban task result",
        "RAG promotion candidates for io-workbench",
        "autonomous Kanban board",
    ]
    .iter()
    .any(|signature| prompt.contains(signature))
}

fn legacy_prompt_matches_board_telemetry(run: &AgenticBoard, timestamp: DateTime<Utc>) -> bool {
    run.prompt_telemetry.iter().any(|entry| {
        entry
            .get("startedAt")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_utc)
            .is_some_and(|started_at| (timestamp - started_at).num_seconds().unsigned_abs() <= 30)
    })
}

fn legacy_board_task_id(run: &AgenticBoard, prompt: &str) -> Option<String> {
    run.tasks
        .iter()
        .filter(|task| {
            let marker = format!(": {}", task.id);
            prompt
                .lines()
                .any(|line| line.trim_start().starts_with("Task ") && line.ends_with(&marker))
                || prompt.contains(&format!("task result into the required JSON contract"))
                    && prompt.contains(&format!("Task id: {}", task.id))
        })
        .max_by_key(|task| task.id.len())
        .map(|task| task.id.clone())
}

async fn pause_board(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    request: Option<Json<PauseRequest>>,
) -> Result<Json<Value>> {
    let request = request
        .map(|Json(request)| request)
        .unwrap_or(PauseRequest { reason: None });
    mutate_board(&state, &user.0.id, &id, |run| {
        request_board_pause(run, trim_string(request.reason));
        Ok(())
    })
}

async fn start_board(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>> {
    let body = body.map(|Json(body)| body).unwrap_or_else(|| json!({}));
    let _ = mutate_board(&state, &user.0.id, &id, |run| {
        if let Some(provider) = body.get("provider").and_then(Value::as_str) {
            run.provider = normalize_provider(Some(provider))?;
        }
        if let Some(model) = body
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            run.model = model.to_string();
        }
        if let Some(model) = body.get("nextModel").and_then(Value::as_str).map(str::trim) {
            run.next_model = model.to_string();
        }
        if let Some(provider) = body.get("nextProvider").and_then(Value::as_str) {
            run.next_provider = normalize_optional_provider(Some(provider))?;
        }
        prepare_board_resume(run);
        Ok(())
    })?;
    let stored = start_board_execution(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn schedule_board(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ScheduleRequest>,
) -> Result<Json<Value>> {
    let scheduled_start_at = trim_string(request.scheduled_start_at);
    let Some(scheduled_start_at) = scheduled_start_at else {
        return mutate_board(&state, &user.0.id, &id, |run| {
            clear_board_schedule(run);
            clear_board_abort_state(run);
            Ok(())
        });
    };
    let scheduled_start_at = parse_rfc3339_utc(&scheduled_start_at)
        .ok_or_else(|| bad_request("Scheduled start time is invalid"))?;
    if scheduled_start_at <= Utc::now() {
        let stored = start_board_execution(&state, &user.0.id, &id)?;
        return Ok(Json(
            json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
        ));
    }
    mutate_board(&state, &user.0.id, &id, |run| {
        if run.loop_started || run.active || run.status == "running" {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "Pause the active board before scheduling a future start.",
            ));
        }
        run.status = "scheduled".to_string();
        run.scheduled_start_at = Some(scheduled_start_at);
        run.auto_run_enabled = true;
        run.pause_requested = false;
        run.paused_at = None;
        run.pause_reason = None;
        clear_board_abort_state(run);
        run.current_provider_session_id = None;
        run.provider_call_started_at = None;
        run.provider_call_label = None;
        bump_control_revision(run);
        run.append_log(format!("Board scheduled to start at {scheduled_start_at}"));
        Ok(())
    })
}

fn clear_board_schedule(run: &mut AgenticBoard) {
    bump_control_revision(run);
    run.scheduled_start_at = None;
    if run.status == "scheduled" {
        run.status = "paused".to_string();
        run.active = false;
        run.loop_started = false;
        run.auto_run_enabled = false;
        run.pause_requested = false;
        run.paused_at = Some(Utc::now());
        run.pause_reason = Some("schedule cleared".to_string());
    }
    run.append_log("Cleared board scheduled start");
}

async fn abort_board(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    request: Option<Json<PauseRequest>>,
) -> Result<Json<Value>> {
    let request = request
        .map(|Json(request)| request)
        .unwrap_or(PauseRequest { reason: None });
    let reason = trim_string(request.reason).unwrap_or_else(|| "user request".to_string());
    mutate_board(&state, &user.0.id, &id, |run| {
        let now = Utc::now();
        bump_control_revision(run);
        reset_in_flight_board_tasks(run, "Task returned to Todo because the board was aborted");
        run.status = "cancelled".to_string();
        run.active = false;
        run.loop_started = false;
        run.cancellation_reason = Some(reason);
        run.abort_source = Some("Board".to_string());
        run.abort_requested_at = Some(now);
        run.canceled_at = Some(now);
        run.current_task_id = None;
        run.current_task_title.clear();
        run.current_task_status.clear();
        run.current_provider_session_id = None;
        run.provider_call_started_at = None;
        run.provider_call_label = None;
        run.append_log("Board aborted");
        Ok(())
    })
}

async fn update_model(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<UpdateModelRequest>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        if let Some(provider) = request.provider.as_deref() {
            run.provider = normalize_provider(Some(provider))?;
        }
        if let Some(model) = trim_string(request.model) {
            let previous = run.model.clone();
            run.model = model.clone();
            run.primary_model = model.clone();
            run.model_history.push(json!({
                "from": previous,
                "to": model,
                "changedAt": Utc::now(),
                "changedBy": "Agentic workspace",
            }));
        }
        if let Some(model) = request.next_model {
            run.next_model = model.trim().to_string();
        }
        if let Some(provider) = request.next_provider {
            run.next_provider = normalize_optional_provider(Some(&provider))?;
        }
        run.append_log("Updated board model");
        Ok(())
    })
}

async fn update_model_strategy(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        let strategy_patch = body
            .get("modelStrategy")
            .cloned()
            .or_else(|| body_has_model_strategy_keys(&body).then(|| body.clone()));
        let strategy_was_patched = strategy_patch.is_some();
        if strategy_was_patched {
            run.model_strategy = normalize_model_strategy(strategy_patch);
        }
        if let Some(profile) = body.get("boardProfile").and_then(Value::as_str) {
            run.board_profile = normalize_board_profile(Some(profile));
        }
        let strategy_overrides = task_model_overrides_for_strategy(run.model_strategy.as_ref());
        if let Some(overrides) = body.get("taskModelOverrides").cloned() {
            run.task_model_overrides = merge_task_model_overrides(
                strategy_overrides,
                normalize_task_model_overrides(overrides),
            );
        } else if !json_object_is_empty(&strategy_overrides) {
            run.task_model_overrides =
                merge_task_model_overrides(strategy_overrides, run.task_model_overrides.clone());
        }
        if let Some(model) = primary_model_for_strategy(run.model_strategy.as_ref()) {
            if run.primary_model.trim().is_empty() || strategy_was_patched {
                run.primary_model = model.clone();
                run.model = model;
            }
        }
        if let Some(policy) = body.get("sessionPolicy").and_then(Value::as_str) {
            run.session_policy = normalize_session_policy(Some(policy));
        }
        sync_session_policy_with_task_models(run, "model strategy update");
        if let Some(policy) = body.get("gitPolicy").and_then(Value::as_str) {
            run.git_policy = normalize_git_policy(Some(policy));
        }
        if let Some(model) = body.get("nextModel").and_then(Value::as_str) {
            run.next_model = model.trim().to_string();
        }
        if let Some(provider) = body.get("nextProvider").and_then(Value::as_str) {
            run.next_provider = normalize_optional_provider(Some(provider))?;
        }
        run.append_log("Updated board model strategy");
        Ok(())
    })
}

async fn update_git_policy(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        let policy = body
            .get("gitPolicy")
            .or_else(|| body.get("policy"))
            .and_then(Value::as_str);
        run.git_policy = normalize_git_policy(policy);
        run.append_log("Updated board git policy");
        Ok(())
    })
}

async fn update_tools_settings(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        run.tools_settings = body.get("toolsSettings").cloned().or_else(|| Some(body));
        run.append_log("Updated board tool settings");
        Ok(())
    })
}

async fn update_tdd_settings(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        if let Some(enabled) = body
            .get("tddEnabled")
            .or_else(|| body.get("enabled"))
            .and_then(Value::as_bool)
        {
            run.tdd_enabled = enabled;
        }
        let patch = body
            .get("tddPolicy")
            .or_else(|| body.get("policy"))
            .cloned()
            .unwrap_or_else(|| body.clone());
        let merged = merge_json_objects(run.tdd_policy.clone(), patch);
        run.tdd_policy = normalize_tdd_policy(Some(&merged));
        run.append_log("Updated board TDD policy");
        Ok(())
    })
}

async fn update_validation_config(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        let patch = body
            .get("validationConfig")
            .or_else(|| body.get("config"))
            .cloned()
            .unwrap_or_else(|| body.clone());
        let merged = merge_json_objects(run.validation_config.clone(), patch);
        run.validation_config = normalize_validation_config(Some(&merged));
        run.append_log("Updated board validation config");
        Ok(())
    })
}

async fn update_rag_settings(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        let patch = body
            .get("ragSettings")
            .or_else(|| body.get("settings"))
            .cloned()
            .unwrap_or_else(|| body.clone());
        let merged = merge_json_objects(run.rag_settings.clone(), patch);
        run.rag_settings = normalize_rag_settings(Some(&merged));
        run.rag_enabled = rag_enabled_from_settings(&run.rag_settings);
        run.append_log("Updated board RAG settings");
        Ok(())
    })
}

async fn update_qa_policy(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        let patch = body
            .get("qaPolicy")
            .or_else(|| body.get("policy"))
            .cloned()
            .unwrap_or_else(|| body.clone());
        let merged = merge_json_objects(run.qa_policy.clone(), patch);
        run.qa_policy = normalize_qa_policy(Some(&merged));
        run.append_log("Updated board QA policy");
        Ok(())
    })
}

async fn update_task_models(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        run.task_model_overrides = normalize_task_model_overrides(
            body.get("taskModelOverrides")
                .or_else(|| body.get("models"))
                .cloned()
                .unwrap_or(body),
        );
        sync_session_policy_with_task_models(run, "task model update");
        run.append_log("Updated board task models");
        Ok(())
    })
}

async fn update_auto_retry(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_board(&state, &user.0.id, &id, |run| {
        let mut next = run.auto_retry.as_object().cloned().unwrap_or_default();
        for key in ["enabled", "delayMinutes", "maxAttempts", "resetAttempts"] {
            if let Some(value) = body.get(key) {
                if key == "resetAttempts" && value.as_bool() == Some(true) {
                    next.insert("attempts".to_string(), json!(0));
                } else if key != "resetAttempts" {
                    next.insert(key.to_string(), value.clone());
                }
            }
        }
        next.insert("updatedAt".to_string(), json!(Utc::now()));
        run.auto_retry = Value::Object(next);
        run.append_log("Updated board auto retry");
        Ok(())
    })
}

async fn add_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<TaskRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    let mut task = BoardTask::manual(&mut stored.board, request)?;
    validate_manual_task_source(&stored.board, &task)?;
    validate_manual_task_status(&stored.board, &task)?;
    let should_start = task_status_is_todo(&task.status);
    stored.board.tasks.push(task);
    normalize_board_hierarchy(&mut stored.board);
    normalize_board_task_groups(&mut stored.board);
    if let Some(cycle) = dependency_cycle(&stored.board) {
        let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
        let error = planning_error_conflict(&mut stored.board, &cycle, "dependency", issue);
        stored.board.touch();
        save_board(&state, &stored.board)?;
        return Err(error);
    }
    if let Some(issue) = hierarchy_validation_issues(&stored.board)
        .into_iter()
        .next()
    {
        let affected = planning_error_task_ids(&stored.board, &issue);
        let error = planning_error_conflict(&mut stored.board, &affected, "hierarchy", issue);
        stored.board.touch();
        save_board(&state, &stored.board)?;
        return Err(error);
    }
    refresh_hierarchy_rollups(&mut stored.board);
    stored.board.append_log("Added manual board task");
    stored.board.touch();
    save_board(&state, &stored.board)?;
    drop(_guard);
    if should_start {
        let stored = start_board_execution(&state, &user.0.id, &id)?;
        return Ok((
            StatusCode::CREATED,
            Json(
                json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
            ),
        ));
    }
    Ok((
        StatusCode::CREATED,
        Json(
            json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
        ),
    ))
}

async fn draft_tasks(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<PromptRequest>,
) -> Result<Json<Value>> {
    let stored = load_user_board(&state, &user.0.id, &id)?;
    let prompt =
        trim_string(request.prompt.clone()).ok_or_else(|| bad_request("Prompt is required"))?;
    let attempt = generate_prompt_task_drafts(
        &state,
        &stored.board,
        &prompt,
        request.provider.as_deref(),
        request.model.as_deref(),
        request.board_profile.as_deref(),
    )
    .await;
    {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(&state, &user.0.id, &id)?;
        record_prompt_task_generation_attempt(
            &mut stored.board,
            "Kanban task draft preview",
            &attempt,
        );
        stored.board.touch();
        save_board(&state, &stored.board)?;
    }
    let (tasks, warning) = attempt.result?;
    Ok(Json(
        json!({ "success": true, "tasks": tasks, "warning": warning }),
    ))
}

async fn breakdown_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((board_id, task_id)): AxumPath<(String, String)>,
    request: Option<Json<PromptRequest>>,
) -> Result<Json<Value>> {
    let _request = request.map(|Json(request)| request);
    let initial_snapshot = load_user_board(&state, &user.0.id, &board_id)?.board;
    if initial_snapshot.status == "cancelled" {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Resume the board before running a manual breakdown.",
        ));
    }
    let initial_parent = initial_snapshot
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if next_hierarchy_level(task_level(initial_parent)).is_none() {
        return Err(bad_request(
            "Only initiative, epic, story, or task items can be broken down.",
        ));
    }
    let retrying_failed_breakdown = is_retryable_hierarchy_breakdown_task(initial_parent);
    if !matches!(
        canonical_task_status(&initial_parent.status),
        TASK_STATUS_BACKLOG | TASK_STATUS_TODO
    ) && !retrying_failed_breakdown
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Move the planning item to Backlog or Todo before breaking it down.",
        ));
    }

    // Older failed breakdowns were persisted as Blocked and left the board's
    // historical abort marker behind. Convert that legacy attention state
    // back into a retryable planning item before invoking the provider.
    if retrying_failed_breakdown
        || hierarchy_breakdown_planning_error_for(&initial_snapshot, &task_id)
        || initial_snapshot.cancellation_reason.is_some()
        || initial_snapshot.abort_source.is_some()
        || initial_snapshot.abort_requested_at.is_some()
        || initial_snapshot.canceled_at.is_some()
    {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(&state, &user.0.id, &board_id)?;
        if let Some(task) = stored
            .board
            .tasks
            .iter_mut()
            .find(|task| task.id == task_id)
        {
            if retrying_failed_breakdown
                && matches!(
                    canonical_task_status(&task.status),
                    TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
                )
            {
                task.status = TASK_STATUS_BACKLOG.to_string();
                task.error = None;
                task.summary = "Retrying hierarchy breakdown".to_string();
                task.completed_at = None;
            }
        }
        restore_board_after_hierarchy_breakdown_failure(&mut stored.board, &task_id);
        clear_board_abort_state(&mut stored.board);
        stored.board.touch();
        save_board(&state, &stored.board)?;
    }

    let snapshot = load_user_board(&state, &user.0.id, &board_id)?.board;
    let parent = snapshot
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if !matches!(
        canonical_task_status(&parent.status),
        TASK_STATUS_BACKLOG | TASK_STATUS_TODO
    ) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Move the planning item to Backlog or Todo before breaking it down.",
        ));
    }
    validate_hierarchy_breakdown_parent(&snapshot, parent)?;

    let created = plan_hierarchy_children(&state, &user.0.id, &board_id, &task_id, true).await?;
    let stored = load_user_board(&state, &user.0.id, &board_id)?;
    Ok(Json(json!({
        "success": true,
        "created": created,
        "board": stored.board.detail_json(Some(stored.path.display().to_string())),
    })))
}

async fn approve_task_side_effects(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
    request: Option<Json<SideEffectsApprovalRequest>>,
) -> Result<Json<Value>> {
    let request = request.map(|Json(request)| request).unwrap_or_default();
    let approved = request.approved.unwrap_or(true);
    let note = trim_string(request.note);
    let stored = mutate_stored_board(&state, &user.0.id, &id, |run| {
        approve_task_side_effects_in_board(run, &task_id, &user.0.id, approved, note.clone())
    })?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn accept_research(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
    request: Option<Json<ResearchAcceptanceRequest>>,
) -> Result<Json<Value>> {
    let request = request.map(|Json(request)| request).unwrap_or_default();
    let stored = mutate_stored_board(&state, &user.0.id, &id, |run| {
        accept_research_in_board(
            run,
            &task_id,
            &user.0.id,
            request.items.clone(),
            trim_string(request.note.clone()),
        )
    })?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn detach_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    let stored = mutate_stored_board(&state, &user.0.id, &id, |run| {
        detach_user_created_child(run, &task_id)
    })?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn resolve_scope_effects(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
    Json(request): Json<ScopeEffectsResolutionRequest>,
) -> Result<Json<Value>> {
    let decision = request.decision.trim().to_ascii_lowercase();
    let note = trim_string(request.note);
    let stored = mutate_stored_board(&state, &user.0.id, &id, |run| {
        resolve_scope_effects_in_board(run, &task_id, &user.0.id, &decision, note.clone())
    })?;
    Ok(Json(json!({
        "success": true,
        "decision": decision,
        "board": stored.board.detail_json(Some(stored.path.display().to_string())),
    })))
}

async fn backlog_from_prompt(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<PromptRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    let prompt =
        trim_string(request.prompt.clone()).ok_or_else(|| bad_request("Prompt is required"))?;
    let model = trim_string(request.model.clone()).unwrap_or_default();
    let provider = normalize_optional_provider(request.provider.as_deref())?;
    let board_profile = request
        .board_profile
        .as_deref()
        .map(|value| normalize_board_profile(Some(value)))
        .unwrap_or_default();
    let (operation_id, response, effective_provider) = {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(&state, &user.0.id, &id)?;
        let operation_id = Uuid::new_v4().to_string();
        let effective_provider = if provider.trim().is_empty() {
            DEFAULT_BREAKDOWN_PROVIDER.to_string()
        } else {
            provider.trim().to_string()
        };
        let profile = if board_profile.trim().is_empty() {
            normalize_board_profile(Some(&stored.board.board_profile))
        } else {
            normalize_board_profile(Some(&board_profile))
        };
        let started_at = Utc::now();
        stored.board.backlog_breakdown = json!({
            "id": operation_id,
            "status": "running",
            "prompt": prompt,
            "provider": effective_provider,
            "model": model,
            "boardProfile": profile,
            "startedAt": started_at,
            "updatedAt": started_at,
            "transcript": prompt_task_generation_running_transcript(&prompt, &effective_provider, model.as_str(), started_at),
        });
        stored.board.append_log(format!(
            "Started backlog breakdown from prompt: {operation_id}"
        ));
        stored.board.touch();
        save_board(&state, &stored.board)?;
        (
            operation_id.clone(),
            json!({
                "success": true,
                "operationId": operation_id,
                "board": stored.board.detail_json(Some(stored.path.display().to_string())),
            }),
            effective_provider,
        )
    };
    spawn_backlog_prompt_generation(
        state.clone(),
        user.0.id.clone(),
        id,
        operation_id.clone(),
        prompt,
        effective_provider,
        model,
        board_profile,
    );
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn promote_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    let _ = update_task_status(&state, &user.0.id, &id, &[task_id], TASK_STATUS_TODO)?;
    let stored = start_board_execution(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn demote_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    update_task_status(&state, &user.0.id, &id, &[task_id], TASK_STATUS_BACKLOG)
}

async fn retry_attention_tasks(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<RetryTasksRequest>,
) -> Result<Json<Value>> {
    let ids = request.task_ids.unwrap_or_default();
    let mode = normalize_retry_mode(request.mode.as_deref())?;
    let reason = request.reason.unwrap_or_default();
    let fix_task_id = request.fix_task_id;
    mutate_stored_board(&state, &user.0.id, &id, |run| {
        retry_attention_tasks_in_board(run, &ids, mode, fix_task_id.as_deref(), &reason).map(|_| ())
    })?;
    let stored = start_board_execution(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn retry_backlog_breakdown(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>> {
    let (operation_id, prompt, provider, model, board_profile) = {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(&state, &user.0.id, &id)?;
        let breakdown = stored.board.backlog_breakdown.clone();
        if breakdown.get("status").and_then(Value::as_str) != Some(TASK_STATUS_FAILED) {
            return Err(not_found("No failed backlog breakdown to retry"));
        }
        let prompt = breakdown
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| bad_request("Failed backlog breakdown has no prompt to retry"))?;
        let model = breakdown
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default();
        let provider = breakdown
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| stored.board.provider.clone());
        let board_profile = breakdown
            .get("boardProfile")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default();
        let operation_id = Uuid::new_v4().to_string();
        let started_at = Utc::now();
        stored.board.backlog_breakdown = json!({
            "id": operation_id,
            "status": "running",
            "prompt": prompt,
            "provider": provider,
            "model": model,
            "boardProfile": board_profile,
            "startedAt": started_at,
            "updatedAt": started_at,
            "retryOf": breakdown.get("id").and_then(Value::as_str).unwrap_or_default(),
            "transcript": prompt_task_generation_running_transcript(&prompt, &provider, &model, started_at),
        });
        clear_board_abort_state(&mut stored.board);
        stored
            .board
            .append_log(format!("Retrying failed backlog breakdown: {operation_id}"));
        stored.board.touch();
        save_board(&state, &stored.board)?;
        (operation_id, prompt, provider, model, board_profile)
    };
    spawn_backlog_prompt_generation(
        state.clone(),
        user.0.id.clone(),
        id.clone(),
        operation_id,
        prompt,
        provider,
        model,
        board_profile,
    );
    let stored = load_user_board(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn delete_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    delete_board_task(&mut stored.board, &task_id)?;
    stored.board.append_log("Deleted board task");
    stored.board.touch();
    save_board(&state, &stored.board)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

fn delete_board_task(run: &mut AgenticBoard, task_id: &str) -> Result<()> {
    let ids = descendant_task_ids(run, task_id);
    if ids.is_empty() {
        return Err(not_found("Agentic board or backlog task not found"));
    }
    if ids
        .iter()
        .any(|id| run.current_task_id.as_deref() == Some(id))
        || run
            .tasks
            .iter()
            .any(|task| ids.contains(&task.id) && task_status_is_active(&task.status))
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "An executing task or descendant cannot be deleted. Pause the board first.",
        ));
    }
    if run
        .tasks
        .iter()
        .any(|task| ids.contains(&task.id) && task_status_is_done(&task.status))
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done items are immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    if ids.iter().any(|id| {
        top_level_parent_id(run, id)
            .and_then(|owner_id| run.tasks.iter().find(|task| task.id == owner_id))
            .is_some_and(|owner| task_status_is_done(&owner.status))
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done item scope is immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    if run
        .tasks
        .iter()
        .any(|task| ids.contains(&task.id) && task_has_recorded_effects(task))
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Deleted work has recorded code or external effects. Choose keep changes, create a revert subtask, or create a cleanup subtask.",
        ));
    }
    let missing_plan_parents = run
        .tasks
        .iter()
        .filter(|task| ids.contains(&task.id) && task.hierarchy.required)
        .filter_map(|task| task_parent_id(task).map(str::to_string))
        .filter(|parent_id| !ids.contains(parent_id))
        .collect::<BTreeSet<_>>();
    run.tasks.retain(|task| !ids.contains(&task.id));
    mark_deleted_dependencies(run, &ids);
    for parent_id in missing_plan_parents {
        if let Some(parent) = run.tasks.iter_mut().find(|task| task.id == parent_id) {
            let reason = format!(
                "Missing required plan: child {task_id} was deleted. Regenerate, replace, or explicitly remove this scope."
            );
            if !task_status_is_backlog(&parent.status) && !task_status_is_done(&parent.status) {
                parent.status = TASK_STATUS_BLOCKED.to_string();
            }
            if !task_status_is_done(&parent.status) {
                parent.error = Some(reason);
                parent.completed_at = None;
            }
        }
    }
    run.append_log(format!(
        "Deleted board item {task_id} and {} generated descendant(s)",
        ids.len().saturating_sub(1)
    ));
    Ok(())
}

fn task_parent_id(task: &BoardTask) -> Option<&str> {
    task.hierarchy.parent_id.as_deref().or_else(|| {
        // Older system-generated QA/follow-up cards used a source link as
        // their structural parent. User-created links (especially a fix
        // linked to a failed subtask) are references, not hierarchy edges.
        if !source_link_is_structural(task) {
            return None;
        }
        task.source_task_id
            .as_deref()
            .or(task.source_qa_task_id.as_deref())
    })
}

fn source_link_is_structural(task: &BoardTask) -> bool {
    task.qa_task
        || task.final_qa_task
        || task.followup_task
        || task.qa_fix_task
        || task.qa_verdict_retry_task
        || task.task_level_qa
        || task.agents_knowledge_task
        || task.source_qa_task_id.is_some()
}

fn task_scope_chain_ids(run: &AgenticBoard, task_id: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut current_id = Some(task_id.to_string());
    let mut visited = BTreeSet::new();
    while let Some(id) = current_id {
        if !visited.insert(id.clone()) {
            break;
        }
        let Some(task) = run.tasks.iter().find(|task| task.id == id) else {
            break;
        };
        ids.push(id);
        current_id = task_parent_id(task).map(str::to_string);
    }
    ids
}

fn top_level_parent_id<'a>(run: &'a AgenticBoard, task_id: &str) -> Option<&'a str> {
    let mut current_id = task_id;
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(current_id.to_string()) {
            return None;
        }
        let task = run.tasks.iter().find(|task| task.id == current_id)?;
        let Some(parent_id) = task_parent_id(task) else {
            return Some(task.id.as_str());
        };
        current_id = parent_id;
    }
}

fn descendant_task_ids(run: &AgenticBoard, root_id: &str) -> BTreeSet<String> {
    if !run.tasks.iter().any(|task| task.id == root_id) {
        return BTreeSet::new();
    }
    let mut ids = BTreeSet::from([root_id.to_string()]);
    loop {
        let before = ids.len();
        for task in &run.tasks {
            if task_parent_id(task).is_some_and(|parent| ids.contains(parent)) {
                ids.insert(task.id.clone());
            }
        }
        if ids.len() == before {
            break;
        }
    }
    ids
}

fn delete_generated_descendants_for_scope_change(
    run: &mut AgenticBoard,
    root_id: &str,
) -> Result<usize> {
    let ids = descendant_task_ids(run, root_id);
    if ids.len() <= 1 {
        return Ok(0);
    }
    if ids.iter().any(|id| {
        run.tasks.iter().any(|task| {
            task.id == *id
                && (task_status_is_active(&task.status)
                    || task.id == run.current_task_id.as_deref().unwrap_or_default())
        })
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Pause the board before changing a parent with executing children.",
        ));
    }
    if ids.iter().any(|id| {
        run.tasks
            .iter()
            .any(|task| task.id == *id && task_has_recorded_effects(task))
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "Generated children already changed code or external state. Choose keep changes, create a revert subtask, or create a cleanup subtask, then retry the scope change. Resolve this with the scope-effects action for {root_id}."
            ),
        ));
    }
    let removed = ids.len() - 1;
    let removed_ids = ids
        .iter()
        .filter(|id| id.as_str() != root_id)
        .cloned()
        .collect::<BTreeSet<_>>();
    run.tasks
        .retain(|task| !ids.contains(&task.id) || task.id == root_id);
    // The root remains part of the board. Only dependencies on descendants
    // that were actually removed should become missing dependencies.
    mark_deleted_dependencies(run, &removed_ids);
    Ok(removed)
}

fn scope_effect_descendants(run: &AgenticBoard, root_id: &str) -> Vec<BoardTask> {
    let ids = descendant_task_ids(run, root_id);
    run.tasks
        .iter()
        .filter(|task| ids.contains(&task.id) && task.id != root_id)
        .filter(|task| task_has_recorded_effects(task))
        .cloned()
        .collect()
}

fn task_has_recorded_effects(task: &BoardTask) -> bool {
    !task.changed_files.is_empty() || !task.hierarchy.side_effect_evidence.is_empty()
}

fn append_scope_effect_action(
    run: &mut AgenticBoard,
    root: &BoardTask,
    affected: &[BoardTask],
    kind: &str,
) -> Result<Vec<String>> {
    let action = if kind == TASK_KIND_REVERT {
        "Revert"
    } else {
        "Clean up"
    };
    let references = affected
        .iter()
        .map(|task| format!("Superseded child {}: {}", task.id, task.title))
        .collect::<Vec<_>>();
    let changed_files = affected
        .iter()
        .flat_map(|task| task.changed_files.iter().cloned())
        .collect::<Vec<_>>();
    let side_effects = affected
        .iter()
        .flat_map(|task| task.hierarchy.side_effect_evidence.iter().cloned())
        .collect::<Vec<_>>();
    let details = format!(
        "{action} recorded effects from superseded generated children of {}.\nAffected work:\n{}",
        root.title,
        affected
            .iter()
            .map(|task| format!("- {}: {}", task.id, task.title))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let mut parent_id = root.id.clone();
    let mut parent_level = task_level(root);
    let mut created = Vec::new();
    loop {
        let Some(level) = next_hierarchy_level(parent_level) else {
            return Err(bad_request(
                "Cannot create scope-effect work under an invalid hierarchy item.",
            ));
        };
        let executable = level == TASK_LEVEL_SUBTASK;
        let mut task = BoardTask::draft(run, format!("{action} superseded work"), details.clone());
        task.priority = task_priority_for_parent(run, Some(parent_id.as_str()), None);
        task.task_origin = "scope_effect_resolution".to_string();
        task.task_type = if executable {
            kind.to_string()
        } else {
            TASK_KIND_DESIGN.to_string()
        };
        task.status = TASK_STATUS_BACKLOG.to_string();
        task.references = references.clone();
        task.acceptance_criteria = vec![format!(
            "Recorded effects from the superseded generated children are {action_lower}ed without changing unrelated work.",
            action_lower = action.to_ascii_lowercase()
        )];
        task.hierarchy.level = level.to_string();
        task.hierarchy.parent_id = Some(parent_id.clone());
        task.hierarchy.executable = executable;
        task.hierarchy.required = true;
        task.hierarchy.scope_version = root.hierarchy.scope_version.saturating_add(1).max(1);
        task.hierarchy.rank = root.hierarchy.rank.saturating_add(1);
        task.hierarchy.planned_files = changed_files.clone();
        task.hierarchy.side_effects = if executable {
            dedupe_strings(side_effects.clone())
        } else {
            Vec::new()
        };
        task.group_id = Some(task_group_id_or_self(root));
        let task_id = task.id.clone();
        run.tasks.push(task);
        created.push(task_id.clone());
        if executable {
            break;
        }
        parent_id = task_id;
        parent_level = level;
    }
    Ok(created)
}

fn resolve_scope_effects_in_board(
    run: &mut AgenticBoard,
    root_id: &str,
    user_id: &str,
    decision: &str,
    note: Option<String>,
) -> Result<()> {
    if !matches!(decision, "keep" | "revert" | "cleanup") {
        return Err(bad_request(
            "Scope-effect decision must be keep, revert, or cleanup.",
        ));
    }
    let root = run
        .tasks
        .iter()
        .find(|task| task.id == root_id)
        .cloned()
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if task_status_is_done(&root.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done items are immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    let scope_chain = task_scope_chain_ids(run, root_id);
    if scope_chain.is_empty() {
        return Err(not_found("Agentic board task not found"));
    }
    if run.active
        || run.loop_started
        || run.status == "running"
        || scope_chain.iter().any(|id| {
            run.tasks.iter().any(|task| {
                task.id == *id
                    && (task_status_is_active(&task.status)
                        || task.id == run.current_task_id.as_deref().unwrap_or_default())
            })
        })
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Pause the board before resolving effects from executing scope work.",
        ));
    }
    if scope_chain.iter().any(|id| {
        run.tasks
            .iter()
            .any(|task| task.id == *id && task_status_is_done(&task.status))
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done scope is immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    // A failed attempt to demote an approved parent may have left it in Todo
    // while preserving the affected descendants so the user can decide what
    // to do with their effects. Treat this explicit resolution action as the
    // approval-clearing transition to Backlog, avoiding a dead-end workflow.
    let moved_to_backlog = scope_chain.iter().any(|id| {
        run.tasks
            .iter()
            .find(|task| task.id == *id)
            .is_some_and(|task| !task_status_is_backlog(&task.status))
    });
    if moved_to_backlog {
        for id in &scope_chain {
            if let Some(task) = run.tasks.iter_mut().find(|task| task.id == *id) {
                task.status = TASK_STATUS_BACKLOG.to_string();
                task.started_at = None;
                task.completed_at = None;
                task.provider_session_id = None;
                task.error = None;
                task.hierarchy.side_effects_approved = false;
                task.hierarchy.side_effect_approval = None;
                task.hierarchy.research_accepted = false;
                task.hierarchy.research_acceptance = None;
            }
        }
        run.append_log(format!(
            "Moved scope {root_id} back to Backlog before resolving recorded child effects"
        ));
    }
    let ids = descendant_task_ids(run, root_id);
    if ids.iter().any(|id| {
        run.tasks.iter().any(|task| {
            task.id == *id
                && (task_status_is_active(&task.status)
                    || task.id == run.current_task_id.as_deref().unwrap_or_default())
        })
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Pause the board before resolving effects from executing children.",
        ));
    }
    let affected = scope_effect_descendants(run, root_id);
    if affected.is_empty() {
        return Err(bad_request(
            "No recorded child code or external effects require a scope-effect decision.",
        ));
    }
    let affected_ids = affected
        .iter()
        .map(|task| task.id.clone())
        .collect::<BTreeSet<_>>();
    let created = if decision == "keep" {
        Vec::new()
    } else {
        append_scope_effect_action(
            run,
            &root,
            &affected,
            if decision == "revert" {
                TASK_KIND_REVERT
            } else {
                TASK_KIND_CLEANUP
            },
        )?
    };
    run.tasks.retain(|task| !affected_ids.contains(&task.id));
    mark_deleted_dependencies(run, &affected_ids);
    if let Some(root_task) = run.tasks.iter_mut().find(|task| task.id == root_id) {
        root_task.hierarchy.discussion.push(json!({
            "kind": "scope_effect_resolution",
            "decision": decision,
            "affectedTaskIds": affected_ids,
            "createdTaskIds": created,
            "resolvedAt": Utc::now(),
            "resolvedBy": user_id,
            "note": note,
        }));
    }
    run.append_log(format!(
        "Resolved superseded child effects for {root_id} with {decision}; created {} explicit cleanup task(s)",
        created.len()
    ));
    refresh_hierarchy_rollups(run);
    Ok(())
}

fn mark_deleted_dependencies(run: &mut AgenticBoard, deleted_ids: &BTreeSet<String>) {
    let affected = run
        .tasks
        .iter()
        .filter_map(|task| {
            let missing = task_blockers(task)
                .into_iter()
                .filter(|dependency| deleted_ids.contains(dependency))
                .collect::<Vec<_>>();
            (!missing.is_empty()).then(|| (task.id.clone(), missing))
        })
        .collect::<Vec<_>>();
    let affected_count = affected.len();
    for (task_id, missing) in affected {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
            if task_status_is_done(&task.status) {
                continue;
            }
            task.status = TASK_STATUS_BLOCKED.to_string();
            task.error = Some(format!("Missing dependency: {}", missing.join(", ")));
            task.completed_at = None;
        }
    }
    if affected_count > 0 {
        run.append_log(format!(
            "Marked {} dependent item(s) blocked because a dependency was deleted",
            affected_count
        ));
        refresh_hierarchy_rollups(run);
    }
}

fn task_external_effect_text(task: &BoardTask) -> String {
    [
        task.title.as_str(),
        task.details.as_str(),
        task.description.as_str(),
        task.prompt.as_str(),
        &task.acceptance_criteria.join("\n"),
        &task.references.join("\n"),
    ]
    .join("\n")
    .to_ascii_lowercase()
}

fn task_requires_external_side_effect_declaration(task: &BoardTask) -> bool {
    if canonical_task_kind(task) == TASK_KIND_MIGRATION {
        return true;
    }
    let text = task_external_effect_text(task);
    [
        "database migration",
        "db migration",
        "drop table",
        "truncate table",
        "delete data",
        "destroy data",
        "reset database",
        "production config",
        "production environment",
        "cloud resource",
        "remote api configuration",
        "remote config",
        "paid api",
        "third-party account",
        "third party account",
        "emulator data",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn task_requires_side_effect_approval(task: &BoardTask) -> bool {
    task_is_executable(task)
        && (!task.hierarchy.side_effects.is_empty()
            || task_requires_external_side_effect_declaration(task))
}

fn task_side_effects_are_approved(task: &BoardTask) -> bool {
    if !task_requires_side_effect_approval(task) {
        return true;
    }
    !task.hierarchy.side_effects.is_empty() && task.hierarchy.side_effects_approved
}

fn task_side_effect_block_reason(task: &BoardTask) -> String {
    if task.hierarchy.side_effects.is_empty() {
        return "Declare possible external side effects before approving this risky subtask."
            .to_string();
    }
    format!(
        "External side-effect approval required before running: {}",
        task.hierarchy.side_effects.join(", ")
    )
}

fn unapproved_side_effect_task_ids(run: &AgenticBoard) -> Vec<String> {
    run.tasks
        .iter()
        .filter(|task| task_status_is_todo(&task.status))
        .filter(|task| task_ancestors_are_approved(run, task))
        .filter(|task| !task_side_effects_are_approved(task))
        .map(|task| task.id.clone())
        .collect()
}

fn mark_side_effect_blockers(run: &mut AgenticBoard) {
    let blocked = run
        .tasks
        .iter()
        .filter(|task| task_status_is_todo(&task.status))
        .filter(|task| task_ancestors_are_approved(run, task))
        .filter(|task| !task_side_effects_are_approved(task))
        .map(|task| (task.id.clone(), task_side_effect_block_reason(task)))
        .collect::<Vec<_>>();
    for (task_id, reason) in blocked {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
            task.status = TASK_STATUS_BLOCKED.to_string();
            task.error = Some(reason);
            task.completed_at = None;
        }
    }
}

fn external_side_effect_evidence(parsed: &Value) -> Vec<String> {
    normalize_string_list(
        parsed
            .get("externalSideEffects")
            .or_else(|| parsed.get("external_side_effects"))
            .or_else(|| parsed.get("sideEffectsEvidence"))
            .or_else(|| parsed.get("side_effects_evidence")),
    )
}

fn manual_test_environment_evidence(parsed: &Value) -> Value {
    let Some(source) = parsed
        .get("manualTestEnvironment")
        .or_else(|| parsed.get("manual_test_environment"))
        .or_else(|| parsed.get("testEnvironment"))
        .or_else(|| parsed.get("environment"))
        .and_then(Value::as_object)
    else {
        return Value::Null;
    };
    let mut environment = serde_json::Map::new();
    for (canonical, aliases) in [
        (
            "deviceOrEmulator",
            &[
                "deviceOrEmulator",
                "device",
                "emulator",
                "simulator",
                "deviceModel",
            ][..],
        ),
        (
            "appVersion",
            &[
                "appVersion",
                "app_version",
                "version",
                "build",
                "buildVersion",
            ][..],
        ),
        (
            "backendUrl",
            &[
                "backendUrl",
                "backend_url",
                "apiUrl",
                "baseUrl",
                "serverUrl",
            ][..],
        ),
        (
            "osVersion",
            &["osVersion", "os_version", "platformVersion"][..],
        ),
    ] {
        let value = aliases
            .iter()
            .find_map(|alias| source.get(*alias))
            .and_then(value_to_trimmed_text);
        if let Some(value) = value {
            environment.insert(canonical.to_string(), json!(value));
        }
    }
    Value::Object(environment)
}

fn value_to_trimmed_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => trim_string(Some(value.clone())),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn manual_test_environment_is_complete(environment: &Value) -> bool {
    let Some(object) = environment.as_object() else {
        return false;
    };
    ["deviceOrEmulator", "appVersion", "backendUrl"]
        .iter()
        .all(|key| {
            object
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        })
}

fn approve_task_side_effects_in_board(
    run: &mut AgenticBoard,
    task_id: &str,
    user_id: &str,
    approved: bool,
    note: Option<String>,
) -> Result<()> {
    let task_snapshot = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if !task_is_executable(&task_snapshot) {
        return Err(bad_request(
            "External side-effect approval is only available for executable subtasks.",
        ));
    }
    if !task_requires_side_effect_approval(&task_snapshot) {
        return Err(bad_request(
            "This subtask has no declared or detected external side effects.",
        ));
    }
    if task_status_is_active(&task_snapshot.status) || task_status_is_done(&task_snapshot.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "External side effects can only be approved before a subtask starts or after it is blocked.",
        ));
    }
    if approved && task_snapshot.hierarchy.side_effects.is_empty() {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Declare the possible external side effects before approving this subtask.",
        ));
    }
    let audit = json!({
        "approved": approved,
        "approvedAt": Utc::now(),
        "approvedBy": user_id,
        "note": note,
        "sideEffects": task_snapshot.hierarchy.side_effects,
    });
    let task = run
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .expect("task snapshot came from board");
    let revoke_reason = (!approved && task_status_is_todo(&task.status))
        .then(|| task_side_effect_block_reason(task));
    task.hierarchy.side_effects_approved = approved;
    task.hierarchy.side_effect_approval = Some(audit);
    if approved
        && canonical_task_status(&task.status) == TASK_STATUS_BLOCKED
        && task
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("External side-effect approval required"))
    {
        task.status = TASK_STATUS_TODO.to_string();
        task.error = None;
    }
    if let Some(reason) = revoke_reason {
        task.status = TASK_STATUS_BLOCKED.to_string();
        task.error = Some(reason);
        task.completed_at = None;
    }
    run.append_log(format!(
        "{} external side effects for subtask {task_id}",
        if approved { "Approved" } else { "Rejected" }
    ));
    refresh_hierarchy_rollups(run);
    Ok(())
}

fn research_proposal_items(task: &BoardTask, requested: Option<Value>) -> Result<Vec<Value>> {
    if let Some(value) = requested {
        if value.is_null() {
            return Ok(Vec::new());
        }
        if let Some(items) = value.as_array() {
            return Ok(items.clone());
        }
        if let Some(items) = value.get("items").and_then(Value::as_array) {
            return Ok(items.clone());
        }
        return Err(bad_request(
            "Research acceptance items must be an array of proposed planning items.",
        ));
    }
    for key in [
        "proposedPlanningItems",
        "proposedItems",
        "planningItems",
        "suggestedBacklogTasks",
    ] {
        if let Some(items) = task
            .result
            .as_ref()
            .and_then(|result| result.get(key))
            .and_then(Value::as_array)
        {
            return Ok(items.clone());
        }
    }
    Ok(Vec::new())
}

fn append_research_planning_items(
    run: &mut AgenticBoard,
    research: &BoardTask,
    items: Vec<Value>,
) -> Result<usize> {
    let mut seen_scope_keys = run
        .tasks
        .iter()
        .filter(|task| task_parent_id(task).is_none())
        .filter(|task| {
            matches!(
                task_level(task),
                TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC | TASK_LEVEL_STORY
            )
        })
        .map(|task| research_planning_scope_key(task_level(task), &task.title))
        .collect::<BTreeSet<_>>();
    let mut created = 0usize;
    for (index, mut item) in items.into_iter().enumerate() {
        let inherits_priority = item
            .get("priority")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_none_or(str::is_empty);
        if let Some(object) = item.as_object_mut() {
            object.remove("parentId");
            object.remove("parent_id");
            object.remove("executable");
        }
        let Some(mut planning) = task_from_json(run, item, index, TASK_STATUS_BACKLOG) else {
            continue;
        };
        if inherits_priority {
            planning.priority = research.priority.clone();
        }
        let title_key = normalize_suggested_task_key(&planning.title);
        let planning_level = match task_level(&planning) {
            TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC | TASK_LEVEL_STORY => task_level(&planning),
            _ => TASK_LEVEL_STORY,
        };
        let scope_key = research_planning_scope_key(planning_level, &planning.title);
        if title_key.is_empty() || !seen_scope_keys.insert(scope_key) {
            continue;
        }
        planning.id = unique_task_id(run, &format!("research-{}", planning.id));
        planning.status = TASK_STATUS_BACKLOG.to_string();
        planning.hierarchy.level = planning_level.to_string();
        planning.hierarchy.parent_id = None;
        planning.hierarchy.executable = false;
        planning.hierarchy.scope_version = research.hierarchy.scope_version.saturating_add(1);
        planning.group_id = Some(planning.id.clone());
        planning.task_origin = "research_accepted".to_string();
        planning.references.insert(
            0,
            format!(
                "Accepted research output from {}: {}",
                research.id, research.title
            ),
        );
        planning.prompt = planning.description.clone();
        run.tasks.push(planning);
        created += 1;
    }
    Ok(created)
}

fn research_planning_scope_key(level: &str, title: &str) -> String {
    format!("{}|{}", level, normalize_suggested_task_key(title))
}

fn accept_research_in_board(
    run: &mut AgenticBoard,
    task_id: &str,
    user_id: &str,
    requested_items: Option<Value>,
    note: Option<String>,
) -> Result<()> {
    let research = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| not_found("Research task not found"))?;
    if canonical_task_kind(&research) != TASK_KIND_RESEARCH {
        return Err(bad_request(
            "Only research subtasks can be accepted as research.",
        ));
    }
    if !task_status_is_done(&research.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Research output can only be accepted after the research subtask is done.",
        ));
    }
    if research.hierarchy.research_accepted {
        return Ok(());
    }
    let items = research_proposal_items(&research, requested_items)?;
    let created = append_research_planning_items(run, &research, items)?;
    if let Some(issue) = hierarchy_validation_issues(run).into_iter().next() {
        let affected = planning_error_task_ids(run, &issue);
        return Err(planning_error_conflict(run, &affected, "hierarchy", issue));
    }
    let audit = json!({
        "acceptedAt": Utc::now(),
        "acceptedBy": user_id,
        "note": note,
        "createdItemCount": created,
    });
    if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
        task.hierarchy.research_accepted = true;
        task.hierarchy.research_acceptance = Some(audit);
        task.error = None;
    }
    refresh_hierarchy_rollups(run);
    run.append_log(format!(
        "Accepted research output for {task_id}; created {created} Backlog planning item(s)"
    ));
    Ok(())
}

fn detach_user_created_child(run: &mut AgenticBoard, task_id: &str) -> Result<()> {
    let snapshot = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    let previous_parent = task_parent_id(&snapshot)
        .map(str::to_string)
        .ok_or_else(|| bad_request("Only nested children can be detached."))?;
    validate_parent_scope_not_completed(run, Some(previous_parent.as_str()))?;
    if !snapshot.manual_task && snapshot.task_origin != "user_manual" {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Only user-created children can be detached and preserved.",
        ));
    }
    if !task_status_is_backlog(&snapshot.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Move the user-created child to Backlog before detaching it.",
        ));
    }
    if !task_scope_owner_is_backlog(run, task_id) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Only children in a Backlog scope can be detached.",
        ));
    }
    let task = run
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .expect("task snapshot came from board");
    task.hierarchy.parent_id = None;
    task.hierarchy.level = TASK_LEVEL_STORY.to_string();
    task.hierarchy.executable = false;
    task.hierarchy.scope_version = task.hierarchy.scope_version.saturating_add(1).max(1);
    task.hierarchy
        .blocked_by
        .retain(|id| id != &previous_parent);
    task.depends_on.retain(|id| id != &previous_parent);
    task.group_id = Some(task.id.clone());
    task.hierarchy.discussion.push(json!({
        "kind": "detached_user_child",
        "previousParentId": previous_parent.clone(),
        "updatedAt": Utc::now(),
    }));
    task.references
        .push(format!("Detached from parent {previous_parent}"));
    run.append_log(format!(
        "Detached user-created child {task_id} into a preserved Backlog story"
    ));
    // The detached item may have been a Task with a Subtask child. Hierarchy
    // normalization converges parent levels before this response is built,
    // preserving the complete Story -> Task -> Subtask chain immediately.
    normalize_board_hierarchy(run);
    normalize_board_task_groups(run);
    if let Some(issue) = hierarchy_validation_issues(run).into_iter().next() {
        let affected = planning_error_task_ids(run, &issue);
        return Err(planning_error_conflict(run, &affected, "hierarchy", issue));
    }
    refresh_hierarchy_rollups(run);
    Ok(())
}

async fn update_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
    Json(request): Json<UpdateTaskRequest>,
) -> Result<Json<Value>> {
    let has_scope_edit = request.title.is_some()
        || request.details.is_some()
        || request.description.is_some()
        || request.kind.is_some()
        || request.task_type.is_some()
        || request.level.is_some()
        || request.parent_id.is_some()
        || request.acceptance_criteria.is_some()
        || request.acceptance.is_some()
        || request.criteria.is_some()
        || request.blocked_by.is_some()
        || request.depends_on.is_some()
        || request.dependencies.is_some()
        || request.required.is_some()
        || request.planned_files.is_some()
        || request.side_effects.is_some();
    if has_scope_edit {
        mutate_stored_board(&state, &user.0.id, &id, |run| {
            edit_backlog_task(run, &task_id, &request)
        })?;
    }
    if !has_scope_edit && (request.priority.is_some() || request.rank.is_some()) {
        mutate_stored_board(&state, &user.0.id, &id, |run| {
            edit_task_priority_rank(run, &task_id, &request)
        })?;
    }
    let status = request
        .status
        .as_deref()
        .map(|value| normalize_task_status(Some(value), ""))
        .transpose()?;
    if let Some(status) = status.as_deref() {
        let _ = update_task_status(&state, &user.0.id, &id, &[task_id], status)?;
    }
    if status.as_deref() == Some(TASK_STATUS_TODO) {
        let stored = start_board_execution(&state, &user.0.id, &id)?;
        return Ok(Json(
            json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
        ));
    }
    let stored = load_user_board(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn discuss_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
    Json(request): Json<DiscussionRequest>,
) -> Result<Json<Value>> {
    let message = trim_string(request.message).unwrap_or_default();
    let requested_action = normalize_discussion_action(request.action.as_deref());
    if message.is_empty() && requested_action.is_empty() {
        return Err(bad_request("Discussion message or action is required."));
    }
    let proposal_id = Uuid::new_v4().to_string();
    let (snapshot, provider, model, started_at) = {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(&state, &user.0.id, &id)?;
        let snapshot = stored.board.clone();
        if !snapshot.tasks.iter().any(|task| task.id == task_id) {
            return Err(not_found("Agentic board task not found"));
        }
        let provider = effective_provider_for_phase(&snapshot, "discussion proposal")?;
        let model = effective_model_for_phase(&snapshot, "discussion proposal");
        let started_at = Utc::now();
        let entry = discussion_running_entry(
            &proposal_id,
            &task_id,
            &message,
            &requested_action,
            request.payload.as_ref().unwrap_or(&Value::Null),
            provider.as_str(),
            model.as_str(),
            started_at,
        );
        append_discussion_proposal(&mut stored.board, &task_id, entry)?;
        stored.board.append_log(format!(
            "Started discussion proposal {proposal_id} for board item {task_id}"
        ));
        stored.board.touch();
        save_board(&state, &stored.board)?;
        (snapshot, provider, model, started_at)
    };

    let prompt = build_discussion_proposal_prompt(
        &snapshot,
        &task_id,
        &message,
        &requested_action,
        request.payload.as_ref().unwrap_or(&Value::Null),
    )?;
    let provider_result = execute_internal_prompt(
        &state,
        &user.0.id,
        &id,
        &format!("discussion proposal for {task_id}"),
        &prompt,
    )
    .await;

    let _guard = board_mutation_lock();
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    let proposal_index = stored
        .board
        .discussion_proposals
        .iter()
        .position(|proposal| {
            proposal.get("id").and_then(Value::as_str) == Some(proposal_id.as_str())
        })
        .ok_or_else(|| not_found("Discussion proposal no longer exists"))?;
    let mut proposal = stored.board.discussion_proposals[proposal_index].clone();
    let assistant_content = match provider_result {
        Ok(output) => output,
        Err(error) => {
            let error_text = server_error_message(&error);
            mark_discussion_proposal_failed(
                &mut proposal,
                &error_text,
                &provider,
                &model,
                started_at,
            );
            update_discussion_proposal(&mut stored.board, &task_id, proposal);
            stored.board.append_log(format!(
                "Discussion proposal {proposal_id} failed: {error_text}"
            ));
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(Json(json!({
                "success": false,
                "proposalId": proposal_id,
                "proposal": stored.board.discussion_proposals[proposal_index],
                "board": stored.board.detail_json(Some(stored.path.display().to_string())),
            })));
        }
    };
    let parsed = parse_json_object(&assistant_content).ok_or_else(|| {
        bad_request(format!(
            "Discussion provider returned malformed JSON: {}",
            limit_text(&assistant_content, 800)
        ))
    });
    let proposal_result = parsed.and_then(|parsed| {
        sanitize_discussion_proposal(
            &stored.board,
            &task_id,
            &proposal_id,
            &requested_action,
            &message,
            request.payload.as_ref().unwrap_or(&Value::Null),
            &parsed,
            &provider,
            &model,
            started_at,
        )
    });
    match proposal_result {
        Ok(mut completed) => {
            if let Some(object) = completed.as_object_mut() {
                object.insert(
                    "transcript".to_string(),
                    discussion_completed_transcript(
                        &message,
                        &assistant_content,
                        &provider,
                        &model,
                        started_at,
                    ),
                );
            }
            update_discussion_proposal(&mut stored.board, &task_id, completed);
            stored.board.append_log(format!(
                "Discussion proposal {proposal_id} is pending explicit approval"
            ));
        }
        Err(error) => {
            let error_text = server_error_message(&error);
            mark_discussion_proposal_failed(
                &mut proposal,
                &error_text,
                &provider,
                &model,
                started_at,
            );
            proposal["transcript"] = discussion_completed_transcript(
                &message,
                &assistant_content,
                &provider,
                &model,
                started_at,
            );
            update_discussion_proposal(&mut stored.board, &task_id, proposal);
            stored.board.append_log(format!(
                "Discussion proposal {proposal_id} could not be prepared: {error_text}"
            ));
        }
    }
    stored.board.touch();
    save_board(&state, &stored.board)?;
    let proposal = stored.board.discussion_proposals[proposal_index].clone();
    Ok(Json(json!({
        "success": proposal.get("status").and_then(Value::as_str) == Some("pending"),
        "proposalId": proposal_id,
        "proposal": proposal,
        "board": stored.board.detail_json(Some(stored.path.display().to_string())),
    })))
}

async fn apply_discussion_proposal(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id, proposal_id)): AxumPath<(String, String, String)>,
) -> Result<Json<Value>> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    let index = stored
        .board
        .discussion_proposals
        .iter()
        .position(|proposal| {
            proposal.get("id").and_then(Value::as_str) == Some(proposal_id.as_str())
                && proposal.get("taskId").and_then(Value::as_str) == Some(task_id.as_str())
        })
        .ok_or_else(|| not_found("Discussion proposal not found"))?;
    let proposal = stored.board.discussion_proposals[index].clone();
    if proposal.get("status").and_then(Value::as_str) != Some("pending") {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Only pending discussion proposals can be applied.",
        ));
    }
    let action = proposal
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("message");
    let payload = proposal
        .get("payload")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let before = discussion_scope_snapshot(&stored.board, &task_id);
    if let Err(error) = apply_discussion_action(&mut stored.board, &task_id, action, &payload) {
        if has_persisted_planning_error(&stored.board) {
            stored.board.touch();
            save_board(&state, &stored.board)?;
        }
        return Err(error);
    }
    let after = discussion_scope_snapshot(&stored.board, &task_id);
    let diff = discussion_diff(&before, &after);
    let mut applied = proposal;
    if let Some(object) = applied.as_object_mut() {
        object.insert("status".to_string(), json!("applied"));
        object.insert("appliedAt".to_string(), json!(Utc::now()));
        object.insert("before".to_string(), before);
        object.insert("after".to_string(), after);
        object.insert("diff".to_string(), diff);
    }
    update_discussion_proposal(&mut stored.board, &task_id, applied);
    refresh_hierarchy_rollups(&mut stored.board);
    stored.board.append_log(format!(
        "Applied discussion proposal {proposal_id} to {task_id}"
    ));
    stored.board.touch();
    save_board(&state, &stored.board)?;
    Ok(Json(json!({
        "success": true,
        "proposalId": proposal_id,
        "board": stored.board.detail_json(Some(stored.path.display().to_string())),
    })))
}

async fn reject_discussion_proposal(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id, proposal_id)): AxumPath<(String, String, String)>,
) -> Result<Json<Value>> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(&state, &user.0.id, &id)?;
    let index = stored
        .board
        .discussion_proposals
        .iter()
        .position(|proposal| {
            proposal.get("id").and_then(Value::as_str) == Some(proposal_id.as_str())
                && proposal.get("taskId").and_then(Value::as_str) == Some(task_id.as_str())
        })
        .ok_or_else(|| not_found("Discussion proposal not found"))?;
    let mut rejected = stored.board.discussion_proposals[index].clone();
    if rejected.get("status").and_then(Value::as_str) != Some("pending") {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Only pending discussion proposals can be rejected.",
        ));
    }
    if let Some(object) = rejected.as_object_mut() {
        object.insert("status".to_string(), json!("rejected"));
        object.insert("rejectedAt".to_string(), json!(Utc::now()));
    }
    update_discussion_proposal(&mut stored.board, &task_id, rejected);
    stored.board.append_log(format!(
        "Rejected discussion proposal {proposal_id} for {task_id}"
    ));
    stored.board.touch();
    save_board(&state, &stored.board)?;
    Ok(Json(json!({
        "success": true,
        "proposalId": proposal_id,
        "board": stored.board.detail_json(Some(stored.path.display().to_string())),
    })))
}

fn normalize_discussion_action(value: Option<&str>) -> String {
    value
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_")
}

fn discussion_running_entry(
    proposal_id: &str,
    task_id: &str,
    message: &str,
    requested_action: &str,
    requested_payload: &Value,
    provider: &str,
    model: &str,
    started_at: DateTime<Utc>,
) -> Value {
    json!({
        "id": proposal_id,
        "proposalId": proposal_id,
        "taskId": task_id,
        "kind": "proposal",
        "message": redact_transcript_text(message),
        "requestedAction": requested_action,
        "requestedPayload": redact_transcript_value(requested_payload),
        "action": requested_action,
        "payload": {},
        "status": "running",
        "provider": provider,
        "model": model,
        "createdAt": started_at,
        "transcript": json!([
            {
                "timestamp": started_at,
                "kind": "message",
                "role": "user",
                "content": redact_transcript_text(message),
            },
            {
                "timestamp": started_at,
                "kind": "status",
                "role": "assistant",
                "provider": provider,
                "model": model,
                "status": "running",
                "content": "Preparing a discussion proposal."
            }
        ]),
    })
}

fn manual_test_steps_evidence(parsed: &Value) -> Vec<String> {
    [
        "manualTestSteps",
        "manual_test_steps",
        "manualSteps",
        "manual_steps",
    ]
    .into_iter()
    .find_map(|key| parsed.get(key))
    .map(|value| normalize_string_list(Some(value)))
    .unwrap_or_default()
}

fn manual_test_result_evidence(parsed: &Value) -> Option<String> {
    [
        "manualTestResult",
        "manual_test_result",
        "manualResult",
        "manual_result",
    ]
    .into_iter()
    .find_map(|key| parsed.get(key))
    .and_then(value_to_trimmed_text)
}

fn manual_test_result_is_successful(result: &str) -> bool {
    let normalized = result
        .trim()
        .to_ascii_lowercase()
        .replace('_', " ")
        .replace('-', " ");
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty()
        || [
            "not run",
            "not performed",
            "not executed",
            "not tested",
            "untested",
            "skipped",
            "skip",
            "unknown",
            "pending",
            "inconclusive",
            "unable to verify",
            "unable to be verified",
            "could not verify",
            "could not be verified",
            "cannot verify",
            "cannot be verified",
            "can't verify",
            "can't be verified",
            "not verified",
        ]
        .iter()
        .any(|needle| normalized.contains(needle))
    {
        return false;
    }

    let failure_is_clearly_negated = ["no fail", "no failure", "without fail", "without failure"]
        .iter()
        .any(|needle| normalized.contains(needle));
    let error_is_clearly_negated = ["no error", "no errors", "without error", "without errors"]
        .iter()
        .any(|needle| normalized.contains(needle));
    if (normalized.contains("fail") && !failure_is_clearly_negated)
        || (normalized.contains("error") && !error_is_clearly_negated)
        || [
            "broken",
            "blocked",
            "not working",
            "does not work",
            "regression",
            "defect",
        ]
        .iter()
        .any(|needle| normalized.contains(needle))
    {
        return false;
    }

    normalized == "ok"
        || normalized.starts_with("ok:")
        || normalized.starts_with("ok ")
        || normalized == "pass"
        || normalized == "passed"
        || normalized.starts_with("pass:")
        || normalized.starts_with("pass ")
        || normalized.starts_with("passed:")
        || normalized.starts_with("passed ")
        || normalized == "success"
        || normalized == "successful"
        || normalized.contains("successfully")
        || normalized.contains("verified successfully")
        || normalized.contains("verified")
        || normalized.contains("works as expected")
        || normalized.contains("worked as expected")
        || normalized.contains("all steps passed")
        || normalized.contains("all checks passed")
        || normalized.contains("meets acceptance")
        || normalized.contains("expected behavior")
        || normalized.contains("no issues")
        || normalized.contains("no problems")
        || failure_is_clearly_negated
        || error_is_clearly_negated
}

fn discussion_completed_transcript(
    message: &str,
    assistant: &str,
    provider: &str,
    model: &str,
    started_at: DateTime<Utc>,
) -> Value {
    json!([
        {
            "timestamp": started_at,
            "kind": "message",
            "role": "user",
            "content": redact_transcript_text(message),
        },
        {
            "timestamp": Utc::now(),
            "kind": "assistant",
            "role": "assistant",
            "provider": provider,
            "model": model,
            "status": "completed",
            "content": redact_transcript_text(assistant),
        }
    ])
}

fn mark_discussion_proposal_failed(
    proposal: &mut Value,
    error: &str,
    provider: &str,
    model: &str,
    started_at: DateTime<Utc>,
) {
    if let Some(object) = proposal.as_object_mut() {
        object.insert("status".to_string(), json!("failed"));
        object.insert("error".to_string(), json!(redact_transcript_text(error)));
        object.insert("provider".to_string(), json!(provider));
        object.insert("model".to_string(), json!(model));
        object.insert("completedAt".to_string(), json!(Utc::now()));
        object.insert(
            "transcript".to_string(),
            discussion_completed_transcript("", error, provider, model, started_at),
        );
    }
}

fn append_discussion_proposal(run: &mut AgenticBoard, task_id: &str, entry: Value) -> Result<()> {
    let task = run
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    task.hierarchy.discussion.push(entry.clone());
    run.discussion_proposals.push(entry);
    Ok(())
}

fn update_discussion_proposal(run: &mut AgenticBoard, task_id: &str, proposal: Value) {
    let proposal_id = proposal.get("id").and_then(Value::as_str);
    if let Some(index) = run.discussion_proposals.iter().position(|item| {
        proposal_id.is_some() && item.get("id").and_then(Value::as_str) == proposal_id
    }) {
        run.discussion_proposals[index] = proposal.clone();
    }
    if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id)
        && let Some(index) = task.hierarchy.discussion.iter().position(|item| {
            proposal_id.is_some() && item.get("id").and_then(Value::as_str) == proposal_id
        })
    {
        task.hierarchy.discussion[index] = proposal;
    }
}

fn discussion_task_scope(task: &BoardTask) -> Value {
    json!({
        "id": task.id,
        "title": task.title,
        "level": task_level(task),
        "kind": canonical_task_kind(task),
        "status": canonical_task_status(&task.status),
        "description": if task.description.trim().is_empty() { &task.details } else { &task.description },
        "acceptanceCriteria": task.acceptance_criteria,
        "parentId": task.hierarchy.parent_id,
        "blockedBy": task_blockers(task),
        "priority": normalize_priority(Some(&task.priority)),
        "rank": task.hierarchy.rank,
        "required": task.hierarchy.required,
        "scopeVersion": task.hierarchy.scope_version,
        "plannedFiles": task.hierarchy.planned_files,
        "sideEffects": task.hierarchy.side_effects,
    })
}

fn discussion_scope_snapshot(run: &AgenticBoard, task_id: &str) -> Value {
    let ids = descendant_task_ids(run, task_id);
    let task = run.tasks.iter().find(|task| task.id == task_id);
    let descendants = run
        .tasks
        .iter()
        .filter(|task| ids.contains(&task.id) && task.id != task_id)
        .map(discussion_task_scope)
        .collect::<Vec<_>>();
    json!({
        "task": task.map(discussion_task_scope),
        "descendants": descendants,
    })
}

fn discussion_diff(before: &Value, after: &Value) -> Value {
    let mut changes = Vec::new();
    if let (Some(before), Some(after)) = (before.as_object(), after.as_object()) {
        let keys = before
            .keys()
            .chain(after.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for key in keys {
            let previous = before.get(&key).cloned().unwrap_or(Value::Null);
            let next = after.get(&key).cloned().unwrap_or(Value::Null);
            if previous != next {
                changes.push(json!({
                    "path": key,
                    "before": previous,
                    "after": next,
                }));
            }
        }
    } else if before != after {
        changes.push(json!({ "path": "$", "before": before, "after": after }));
    }
    json!({
        "changed": !changes.is_empty(),
        "changes": changes,
        "before": before,
        "after": after,
    })
}

fn build_discussion_proposal_prompt(
    run: &AgenticBoard,
    task_id: &str,
    message: &str,
    requested_action: &str,
    requested_payload: &Value,
) -> Result<String> {
    let task = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    let actions = "message|edit|replace|delete|split|merge|regenerate_children|reprioritize|re_research|revision|fix|replacement";
    Ok(format!(
        r#"Prepare a structured proposal for a Kanban ticket discussion.

Do not edit files, run implementation, or apply any board mutation. The user
must explicitly approve the returned proposal through a separate Apply action.

Ticket:
{ticket}

Current ticket scope and descendants:
{scope}

User message:
{message}

Requested action (a hint, not an instruction to mutate): {requested_action}
Requested payload hint:
{requested_payload}

Return JSON only, without markdown:
{{
  "action": "{actions}",
  "summary": "what should change or what was discussed",
  "payload": {{}},
  "warnings": ["scope or approval warning"],
  "acceptanceCriteria": ["criteria affected by this proposal"]
}}

Rules:
- Use exactly one action from the list.
- If the user only asks a question, use `message`, leave payload empty, and answer in summary.
- For edit or replace, payload may contain only ticket scope fields: title, details,
  description, kind, taskType, level, parentId, acceptanceCriteria, priority,
  rank, blockedBy, dependsOn, required, plannedFiles, and sideEffects.
- For reprioritize, payload must contain a valid priority p0, p1, p2, or p3.
- For split, payload must contain an items array of one-purpose child tickets.
- For merge, payload must contain targetId.
- For re_research, revision, fix, or replacement, payload should describe the
  new linked Backlog planning item; never reopen the completed source item.
- For replacement, include `supersedeSource: true` only when the user explicitly
  wants the completed source item marked superseded after applying this proposal.
- A scope-changing proposal is still pending when the ticket is locked; warn that
  it cannot be applied until the scope owner is moved back to Backlog.
- Do not create executable work above the subtask level. If you propose children,
  preserve the next hierarchy level and make only subtasks executable.
- Do not introduce a separate tracking matrix or external IDs. Ticket fields are the source of truth.
- Do not include secrets, tokens, or raw environment values in the response.

Codebase context:
{codebase}
"#,
        ticket = serde_json::to_string_pretty(&discussion_task_scope(task)).unwrap_or_default(),
        scope = serde_json::to_string_pretty(&discussion_scope_snapshot(run, task_id))
            .unwrap_or_default(),
        message = redact_transcript_text(message),
        requested_action = if requested_action.is_empty() {
            "message"
        } else {
            requested_action
        },
        requested_payload =
            serde_json::to_string_pretty(&redact_transcript_value(requested_payload))
                .unwrap_or_default(),
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
    ))
}

fn sanitize_discussion_proposal(
    run: &AgenticBoard,
    task_id: &str,
    proposal_id: &str,
    requested_action: &str,
    message: &str,
    requested_payload: &Value,
    parsed: &Value,
    provider: &str,
    model: &str,
    started_at: DateTime<Utc>,
) -> Result<Value> {
    let parsed_action = normalize_discussion_action(parsed.get("action").and_then(Value::as_str));
    let action = if requested_action.is_empty() {
        if parsed_action.is_empty() {
            "message".to_string()
        } else {
            parsed_action
        }
    } else {
        requested_action.to_string()
    };
    if !matches!(
        action.as_str(),
        "message"
            | "edit"
            | "replace"
            | "delete"
            | "split"
            | "merge"
            | "regenerate_children"
            | "reprioritize"
            | "re_research"
            | "revision"
            | "fix"
            | "replacement"
            | "research"
    ) {
        return Err(bad_request(format!(
            "Unsupported discussion action: {action}"
        )));
    }
    let mut payload = sanitize_discussion_payload(&action, parsed)?;
    if action == "replacement"
        && requested_payload
            .get("supersedeSource")
            .and_then(Value::as_bool)
            == Some(true)
    {
        if let Some(object) = payload.as_object_mut() {
            object.insert("supersedeSource".to_string(), json!(true));
        }
    }
    validate_discussion_payload(&action, &payload)?;
    let before = discussion_scope_snapshot(run, task_id);
    let mut preview = run.clone();
    let mut warnings = normalize_string_list(parsed.get("warnings"));
    if let Err(error) = apply_discussion_action(&mut preview, task_id, &action, &payload) {
        warnings.push(server_error_message(&error));
    }
    let after = discussion_scope_snapshot(&preview, task_id);
    if discussion_action_requires_backlog(&action) && !task_scope_owner_is_backlog(run, task_id) {
        warnings.push(
            "This scope change remains pending because the ticket is approved or locked; move its scope owner back to Backlog before applying it.".to_string(),
        );
    }
    let summary = parsed
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if message.trim().is_empty() {
                "Discussion proposal"
            } else {
                message.trim()
            }
        });
    Ok(json!({
        "id": proposal_id,
        "proposalId": proposal_id,
        "taskId": task_id,
        "kind": "proposal",
        "message": redact_transcript_text(message),
        "action": action,
        "payload": redact_transcript_value(&payload),
        "summary": redact_transcript_text(summary),
        "warnings": dedupe_strings(warnings),
        "acceptanceCriteria": normalize_string_list(parsed.get("acceptanceCriteria")),
        "before": before,
        "after": after,
        "diff": discussion_diff(&before, &after),
        "status": "pending",
        "provider": provider,
        "model": model,
        "createdAt": started_at,
        "completedAt": Utc::now(),
    }))
}

fn sanitize_discussion_payload(action: &str, parsed: &Value) -> Result<Value> {
    if action == "message" || action == "delete" || action == "regenerate_children" {
        return Ok(json!({}));
    }
    if let Some(payload) = parsed.get("payload")
        && payload.is_object()
    {
        return Ok(payload.clone());
    }
    let Some(object) = parsed.as_object() else {
        return Ok(json!({}));
    };
    let allowed = [
        "title",
        "details",
        "description",
        "kind",
        "taskType",
        "level",
        "parentId",
        "acceptanceCriteria",
        "priority",
        "rank",
        "blockedBy",
        "dependsOn",
        "required",
        "plannedFiles",
        "sideEffects",
        "supersedeSource",
        "items",
        "children",
        "targetId",
    ];
    let mut payload = serde_json::Map::new();
    for key in allowed {
        if let Some(value) = object.get(key) {
            payload.insert(key.to_string(), value.clone());
        }
    }
    Ok(Value::Object(payload))
}

fn validate_discussion_payload(action: &str, payload: &Value) -> Result<()> {
    match action {
        "edit" | "replace" => {
            if !payload.is_object() || payload.as_object().is_none_or(|object| object.is_empty()) {
                return Err(bad_request(
                    "Discussion edit proposal must contain at least one scope field.",
                ));
            }
        }
        "reprioritize" => {
            let priority = payload
                .get("priority")
                .and_then(Value::as_str)
                .ok_or_else(|| bad_request("Reprioritize proposal must contain priority."))?;
            if !matches!(
                normalize_priority(Some(priority)),
                TASK_PRIORITY_P0 | TASK_PRIORITY_P1 | TASK_PRIORITY_P2 | TASK_PRIORITY_P3
            ) {
                return Err(bad_request("Priority must be p0, p1, p2, or p3."));
            }
        }
        "split" => {
            let items = payload
                .get("items")
                .or_else(|| payload.get("children"))
                .and_then(Value::as_array)
                .ok_or_else(|| bad_request("Split proposal must contain items."))?;
            if items.is_empty() {
                return Err(bad_request(
                    "Split proposal must contain at least one item.",
                ));
            }
        }
        "merge" => {
            if payload
                .get("targetId")
                .or_else(|| payload.get("target_id"))
                .and_then(Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(bad_request("Merge proposal must contain targetId."));
            }
        }
        "re_research" | "research" | "revision" | "fix" | "replacement" => {}
        _ => {}
    }
    Ok(())
}

fn discussion_action_requires_backlog(action: &str) -> bool {
    matches!(
        action,
        "edit" | "replace" | "delete" | "split" | "merge" | "regenerate_children"
    )
}

fn task_scope_owner_is_backlog(run: &AgenticBoard, task_id: &str) -> bool {
    let item_is_backlog = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .is_some_and(|task| task_status_is_backlog(&task.status));
    if item_is_backlog {
        return true;
    }
    let owner_id = top_level_parent_id(run, task_id).unwrap_or(task_id);
    run.tasks
        .iter()
        .find(|task| task.id == owner_id)
        .is_some_and(|task| task_status_is_backlog(&task.status))
}

fn apply_discussion_action(
    run: &mut AgenticBoard,
    task_id: &str,
    action: &str,
    payload: &Value,
) -> Result<()> {
    if discussion_action_requires_backlog(action) && !task_scope_owner_is_backlog(run, task_id) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Only Backlog scope can be changed. Move the scope owner back to Backlog first.",
        ));
    }
    match action {
        "message" => Ok(()),
        "edit" | "replace" => {
            let patch = serde_json::from_value::<UpdateTaskRequest>(payload.clone())
                .map_err(|error| bad_request(format!("Invalid discussion edit: {error}")))?;
            edit_backlog_task(run, task_id, &patch)
        }
        "reprioritize" => {
            let priority = payload
                .get("priority")
                .and_then(Value::as_str)
                .ok_or_else(|| bad_request("Reprioritize proposal must contain priority."))?;
            edit_task_priority_rank(
                run,
                task_id,
                &UpdateTaskRequest {
                    priority: Some(priority.to_string()),
                    ..UpdateTaskRequest::default()
                },
            )
        }
        "delete" => delete_board_task(run, task_id),
        "regenerate_children" => {
            delete_generated_descendants_for_scope_change(run, task_id)?;
            refresh_hierarchy_rollups(run);
            Ok(())
        }
        "split" => split_discussion_item(run, task_id, payload),
        "merge" => merge_discussion_item(run, task_id, payload),
        "re_research" | "research" | "revision" | "fix" | "replacement" => {
            append_linked_planning_item(run, task_id, action, payload)
        }
        _ => Err(bad_request(format!(
            "Unsupported discussion action: {action}"
        ))),
    }
}

fn split_discussion_item(run: &mut AgenticBoard, parent_id: &str, payload: &Value) -> Result<()> {
    let parent = run
        .tasks
        .iter()
        .find(|task| task.id == parent_id)
        .cloned()
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if !task_status_is_backlog(&parent.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Items can only be split while they are in Backlog.",
        ));
    }
    let Some(next_level) = next_hierarchy_level(task_level(&parent)) else {
        return Err(bad_request(
            "A subtask cannot be split into another hierarchy level.",
        ));
    };
    let items = payload
        .get("items")
        .or_else(|| payload.get("children"))
        .and_then(Value::as_array)
        .ok_or_else(|| bad_request("Discussion split requires an items array."))?;
    if items.is_empty() {
        return Err(bad_request(
            "Discussion split requires at least one child item.",
        ));
    }
    let snapshot = run.clone();
    let group_id = task_group_id_or_self(&parent);
    let mut children = Vec::new();
    for (index, item) in items.iter().cloned().enumerate() {
        let mut item = item;
        if let Some(object) = item.as_object_mut() {
            object.insert("level".to_string(), json!(next_level));
            object.insert("parentId".to_string(), json!(parent.id));
            object.insert("status".to_string(), json!(TASK_STATUS_BACKLOG));
        }
        let Some(mut child) = task_from_json(&snapshot, item, index, TASK_STATUS_BACKLOG) else {
            continue;
        };
        child.id = unique_task_id(run, &format!("{}-split-{}", parent.id, index + 1));
        child.hierarchy.level = next_level.to_string();
        child.hierarchy.parent_id = Some(parent.id.clone());
        child.hierarchy.executable = next_level == TASK_LEVEL_SUBTASK;
        child.hierarchy.scope_version = parent.hierarchy.scope_version.saturating_add(1);
        child.group_id = Some(group_id.clone());
        child.task_origin = "discussion_split".to_string();
        child.prompt = child.description.clone();
        children.push(child);
    }
    if children.is_empty() {
        return Err(bad_request(
            "Discussion split did not contain usable child items.",
        ));
    }

    // Validate the complete proposed tree before mutating the live board. A
    // split is a planning action, so malformed hierarchy, contradictory
    // acceptance criteria, or a dependency cycle must not leave half of the
    // proposed children applied.
    let mut candidate = run.clone();
    candidate.tasks.extend(children.iter().cloned());
    if let Some(cycle) = dependency_cycle(&candidate) {
        let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
        return Err(planning_error_conflict(
            run,
            std::slice::from_ref(&parent.id),
            "dependency",
            issue,
        ));
    }
    if let Some(issue) = hierarchy_validation_issues(&candidate).into_iter().next() {
        return Err(planning_error_conflict(
            run,
            std::slice::from_ref(&parent.id),
            "hierarchy",
            issue,
        ));
    }
    run.tasks.extend(children);
    refresh_hierarchy_rollups(run);
    Ok(())
}

fn merge_discussion_item(run: &mut AgenticBoard, source_id: &str, payload: &Value) -> Result<()> {
    let target_id = payload
        .get("targetId")
        .or_else(|| payload.get("target_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| bad_request("Discussion merge requires targetId."))?;
    if source_id == target_id {
        return Err(bad_request("A task cannot be merged into itself."));
    }
    let source = run
        .tasks
        .iter()
        .find(|task| task.id == source_id)
        .cloned()
        .ok_or_else(|| not_found("Source task not found"))?;
    let target = run
        .tasks
        .iter()
        .find(|task| task.id == target_id)
        .cloned()
        .ok_or_else(|| not_found("Target task not found"))?;
    if !task_status_is_backlog(&source.status) || !task_status_is_backlog(&target.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Both items must be in Backlog before they can be merged.",
        ));
    }
    delete_generated_descendants_for_scope_change(run, target_id)?;
    let merged_details = format!(
        "{}\n\nMerged scope from {}:\n{}",
        target.details, source.id, source.details
    );
    let patch = UpdateTaskRequest {
        details: Some(merged_details),
        acceptance_criteria: Some(json!(
            target
                .acceptance_criteria
                .into_iter()
                .chain(source.acceptance_criteria)
                .collect::<Vec<_>>()
        )),
        ..UpdateTaskRequest::default()
    };
    edit_backlog_task(run, target_id, &patch)?;
    delete_board_task(run, source_id)
}

fn append_linked_planning_item(
    run: &mut AgenticBoard,
    source_id: &str,
    requested_kind: &str,
    payload: &Value,
) -> Result<()> {
    let source = run
        .tasks
        .iter()
        .find(|task| task.id == source_id)
        .cloned()
        .ok_or_else(|| not_found("Source task not found"))?;
    let kind = match requested_kind {
        "re_research" | "research" => TASK_KIND_RESEARCH,
        "revision" => TASK_KIND_REVISION,
        "fix" => TASK_KIND_FIX,
        "replacement" => TASK_KIND_REPLACEMENT,
        other => {
            return Err(bad_request(format!(
                "Unsupported linked planning item kind: {other}"
            )));
        }
    };
    let supersede_source = kind == TASK_KIND_REPLACEMENT
        && payload
            .get("supersedeSource")
            .or_else(|| payload.get("supersede_source"))
            .and_then(Value::as_bool)
            == Some(true);
    if supersede_source {
        if !task_status_is_done(&source.status) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "Only a completed item can be marked superseded.",
            ));
        }
        if source.superseded_by.is_some() {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "This completed item is already superseded by a linked replacement.",
            ));
        }
    }
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(match kind {
            TASK_KIND_RESEARCH => "Research a revised direction",
            TASK_KIND_REVISION => "Revise the completed item",
            TASK_KIND_FIX => "Fix the completed item",
            TASK_KIND_REPLACEMENT => "Replace the completed item",
            _ => "Continue the completed item",
        });
    let details = payload
        .get("details")
        .or_else(|| payload.get("description"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&source.details);
    let default_acceptance = match kind {
        TASK_KIND_RESEARCH => "Record evidence and a proposed direction for review.",
        TASK_KIND_REVISION => "Implement the revised scope without reopening the completed item.",
        TASK_KIND_FIX => "Verify that the defect in the completed item is resolved.",
        TASK_KIND_REPLACEMENT => "Implement and verify the replacement behavior.",
        _ => "Complete the linked planning item.",
    };
    let item = json!({
        "id": unique_task_id(run, "research"),
        "title": title,
        "level": TASK_LEVEL_STORY,
        "kind": kind,
        "sourceTaskId": source_id,
        "details": details,
        "description": details,
        "acceptanceCriteria": payload.get("acceptanceCriteria").cloned().unwrap_or_else(|| json!([default_acceptance])),
        "references": [format!("Related item: {source_id}")],
        "priority": source.priority,
        "status": TASK_STATUS_BACKLOG,
    });
    let Some(mut linked) = task_from_json(run, item, run.tasks.len(), TASK_STATUS_BACKLOG) else {
        return Err(bad_request("Linked planning item could not be created."));
    };
    linked.id = unique_task_id(run, kind);
    let linked_id = linked.id.clone();
    linked.group_id = Some(linked.id.clone());
    linked.task_origin = format!("discussion_{kind}");
    linked.hierarchy.level = TASK_LEVEL_STORY.to_string();
    linked.hierarchy.executable = false;
    run.tasks.push(linked);
    if supersede_source {
        let source = run
            .tasks
            .iter_mut()
            .find(|task| task.id == source_id)
            .expect("source task came from board");
        source.superseded_by = Some(linked_id.clone());
        source
            .references
            .push(format!("Superseded by linked replacement {linked_id}"));
        source.hierarchy.discussion.push(json!({
            "kind": "superseded",
            "replacementId": linked_id,
            "updatedAt": Utc::now(),
        }));
    }
    Ok(())
}

fn edit_backlog_task(
    run: &mut AgenticBoard,
    task_id: &str,
    request: &UpdateTaskRequest,
) -> Result<()> {
    let task = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if task_status_is_done(&task.status) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done items are immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    validate_parent_scope_not_completed(run, task.hierarchy.parent_id.as_deref())?;
    if request.parent_id.is_some() {
        let requested_parent_id = trim_string(request.parent_id.clone());
        validate_parent_scope_not_completed(run, requested_parent_id.as_deref())?;
    }
    if !task_scope_owner_is_backlog(run, task_id) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Only Backlog items or items owned by a Backlog scope can be edited. Move the scope back to Backlog first.",
        ));
    }
    let mut scope_changed = false;
    if request.title.is_some()
        || request.details.is_some()
        || request.description.is_some()
        || request.kind.is_some()
        || request.task_type.is_some()
        || request.level.is_some()
        || request.parent_id.is_some()
        || request.acceptance_criteria.is_some()
        || request.acceptance.is_some()
        || request.criteria.is_some()
        || request.blocked_by.is_some()
        || request.depends_on.is_some()
        || request.dependencies.is_some()
        || request.required.is_some()
        || request.planned_files.is_some()
        || request.side_effects.is_some()
    {
        scope_changed = true;
        delete_generated_descendants_for_scope_change(run, task_id)?;
    }
    let index = run
        .tasks
        .iter()
        .position(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task was removed while editing"))?;
    let task = run.tasks.get_mut(index).expect("task index checked");
    if let Some(title) = request
        .title
        .clone()
        .and_then(|value| trim_string(Some(value)))
    {
        task.title = title;
    }
    if let Some(details) = request
        .details
        .clone()
        .or_else(|| request.description.clone())
        .and_then(|value| trim_string(Some(value)))
    {
        task.details = details.clone();
        task.description = details.clone();
        task.prompt = details;
    }
    if let Some(kind) = request
        .kind
        .as_deref()
        .or(request.task_type.as_deref())
        .and_then(normalized_task_kind_name)
    {
        task.task_type = kind.to_string();
    }
    if let Some(level) = request.level.as_deref() {
        task.hierarchy.level = normalize_task_level(Some(level), TASK_LEVEL_STORY).to_string();
    }
    if request.parent_id.is_some() {
        task.hierarchy.parent_id = trim_string(request.parent_id.clone());
    }
    if let Some(criteria) = request
        .acceptance_criteria
        .clone()
        .or_else(|| request.acceptance.clone())
        .or_else(|| request.criteria.clone())
    {
        task.acceptance_criteria = value_to_strings(Some(criteria));
    }
    if let Some(priority) = request.priority.as_deref() {
        task.priority = normalize_priority(Some(priority)).to_string();
    }
    if let Some(rank) = request.rank {
        task.hierarchy.rank = rank;
    }
    let dependencies = request
        .blocked_by
        .clone()
        .or_else(|| request.depends_on.clone())
        .or_else(|| request.dependencies.clone())
        .map(|value| value_to_strings(Some(value)))
        .unwrap_or_else(|| task_blockers(task));
    task.hierarchy.blocked_by = dedupe_strings(dependencies.clone());
    task.depends_on = task.hierarchy.blocked_by.clone();
    if let Some(required) = request.required {
        task.hierarchy.required = required;
    }
    if let Some(files) = request.planned_files.clone() {
        task.hierarchy.planned_files = value_to_strings(Some(files));
    }
    if let Some(side_effects) = request.side_effects.clone() {
        task.hierarchy.side_effects = value_to_strings(Some(side_effects));
        task.hierarchy.side_effects_approved = false;
        task.hierarchy.side_effect_approval = None;
        task.hierarchy.side_effect_evidence.clear();
    }
    if task.hierarchy.parent_id.is_none()
        && matches!(task_level(task), TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK)
    {
        task.hierarchy.level = TASK_LEVEL_STORY.to_string();
    }
    task.hierarchy.executable = task_level(task) == TASK_LEVEL_SUBTASK;
    if scope_changed {
        task.hierarchy.scope_version = task.hierarchy.scope_version.saturating_add(1).max(1);
        task.error = None;
        task.result = None;
        task.result_validation = None;
        task.evidence.clear();
        task.hierarchy.side_effects_approved = false;
        task.hierarchy.side_effect_approval = None;
        task.hierarchy.side_effect_evidence.clear();
        task.hierarchy.research_accepted = false;
        task.hierarchy.research_acceptance = None;
        task.remaining_issues.clear();
        task.completed_at = None;
        task.hierarchy.discussion.push(json!({
            "kind": "scope_edit",
            "updatedAt": Utc::now(),
        }));
    }
    if let Some(cycle) = dependency_cycle(run) {
        let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
        return Err(planning_error_conflict(run, &cycle, "dependency", issue));
    }
    if let Some(issue) = hierarchy_validation_issues(run).into_iter().next() {
        let affected = planning_error_task_ids(run, &issue);
        return Err(planning_error_conflict(run, &affected, "hierarchy", issue));
    }
    refresh_hierarchy_rollups(run);
    run.append_log(format!("Edited Backlog item {task_id}"));
    Ok(())
}

fn edit_task_priority_rank(
    run: &mut AgenticBoard,
    task_id: &str,
    request: &UpdateTaskRequest,
) -> Result<()> {
    let task_snapshot = run
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    let owner_done = top_level_parent_id(run, task_id)
        .map(str::to_string)
        .and_then(|owner_id| {
            run.tasks
                .iter()
                .find(|candidate| candidate.id == owner_id)
                .map(|owner| task_status_is_done(&owner.status))
        })
        .unwrap_or(false);
    if task_status_is_done(&task_snapshot.status) || owner_done {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done items are immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    let task = run
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .ok_or_else(|| not_found("Agentic board task not found"))?;
    if let Some(priority) = request.priority.as_deref() {
        task.priority = normalize_priority(Some(priority)).to_string();
    }
    if let Some(rank) = request.rank {
        task.hierarchy.rank = rank.max(0);
    }
    run.append_log(format!("Updated priority/rank for board task {task_id}"));
    Ok(())
}

fn update_task_status(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    task_ids: &[String],
    status: &str,
) -> Result<Json<Value>> {
    let status = normalize_task_status(Some(status), "")?;
    if !matches!(status.as_str(), TASK_STATUS_BACKLOG | TASK_STATUS_TODO) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "User status changes may only move items between Backlog and Todo.",
        ));
    }
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, board_id)?;
    let matching_ids = stored
        .board
        .tasks
        .iter()
        .filter(|task| {
            task_ids.iter().any(|id| id == &task.id)
                || (task_ids.is_empty()
                    && status == TASK_STATUS_TODO
                    && matches!(
                        canonical_task_status(&task.status),
                        TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
                    ))
        })
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    if matching_ids.is_empty() {
        return Err(not_found("Agentic board or task not found"));
    }
    let failed_ids = stored
        .board
        .tasks
        .iter()
        .filter(|task| {
            matching_ids.iter().any(|id| id == &task.id)
                && canonical_task_status(&task.status) == TASK_STATUS_FAILED
        })
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    if status == TASK_STATUS_TODO && !failed_ids.is_empty() {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "Failed item(s) {} require an explicit transient retry or a completed approved fix plan; they cannot be moved directly to Todo.",
                failed_ids.join(", ")
            ),
        ));
    }
    if status == TASK_STATUS_TODO && uses_hierarchical_orchestration(&stored.board) {
        if stored.board.tasks.iter().any(|task| {
            matching_ids.iter().any(|id| id == &task.id)
                && !task_ancestors_are_approved(&stored.board, task)
        }) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "Approve the parent planning item before moving a nested item to Todo.",
            ));
        }
        if let Some(task) = stored.board.tasks.iter().find(|task| {
            matching_ids.iter().any(|id| id == &task.id) && !task_side_effects_are_approved(task)
        }) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                task_side_effect_block_reason(task),
            ));
        }
    }
    if status == TASK_STATUS_BACKLOG {
        for task_id in &matching_ids {
            if let Some(task) = stored.board.tasks.iter().find(|task| task.id == *task_id)
                && task_status_is_done(&task.status)
            {
                return Err(ServerError::new(
                    StatusCode::CONFLICT,
                    "Done items are immutable. Create a linked revision, fix, research, or replacement item.",
                ));
            }
            delete_generated_descendants_for_scope_change(&mut stored.board, task_id)?;
        }
    }
    if matching_ids.iter().any(|task_id| {
        stored
            .board
            .tasks
            .iter()
            .find(|task| task.id == *task_id)
            .is_some_and(|task| task_status_is_done(&task.status))
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done items are immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    if matching_ids.iter().any(|task_id| {
        top_level_parent_id(&stored.board, task_id)
            .and_then(|owner_id| stored.board.tasks.iter().find(|task| task.id == owner_id))
            .is_some_and(|owner| task_status_is_done(&owner.status))
            && stored
                .board
                .tasks
                .iter()
                .find(|task| task.id == *task_id)
                .is_none_or(|task| !task_ancestors_are_approved(&stored.board, task))
    }) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Done item scope is immutable. Create a linked revision, fix, research, or replacement item.",
        ));
    }
    let mut updated = 0usize;
    for task in &mut stored.board.tasks {
        if matching_ids.iter().any(|id| id == &task.id) {
            if task_status_is_active(&task.status) {
                return Err(ServerError::new(
                    StatusCode::CONFLICT,
                    "Pause the active board before moving an in-progress item.",
                ));
            }
            task.status = status.clone();
            task.error = None;
            if matches!(status.as_str(), TASK_STATUS_TODO | TASK_STATUS_BACKLOG) {
                task.started_at = None;
                task.completed_at = None;
                task.provider_session_id = None;
            }
            if status == TASK_STATUS_BACKLOG {
                // Backlog is a fresh approval boundary. A previous external
                // side-effect approval must never carry across that boundary.
                clear_backlog_approval(task);
            }
            updated += 1;
        }
    }
    if updated == 0 {
        return Err(not_found("Agentic board or task not found"));
    }
    refresh_hierarchy_rollups(&mut stored.board);
    if let Some(cycle) = dependency_cycle(&stored.board) {
        let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
        let error = planning_error_conflict(&mut stored.board, &cycle, "dependency", issue);
        stored.board.touch();
        save_board(state, &stored.board)?;
        return Err(error);
    }
    if let Some(issue) = hierarchy_validation_issues(&stored.board)
        .into_iter()
        .next()
    {
        let affected = planning_error_task_ids(&stored.board, &issue);
        let error = planning_error_conflict(&mut stored.board, &affected, "hierarchy", issue);
        stored.board.touch();
        save_board(state, &stored.board)?;
        return Err(error);
    }
    stored
        .board
        .append_log(format!("Moved {updated} board task(s) to {status}"));
    stored.board.touch();
    save_board(state, &stored.board)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

fn mutate_board(
    state: &AppState,
    user_id: &str,
    id: &str,
    mutate: impl FnOnce(&mut AgenticBoard) -> Result<()>,
) -> Result<Json<Value>> {
    let stored = mutate_stored_board(state, user_id, id, mutate)?;
    Ok(Json(
        json!({ "success": true, "board": stored.board.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

fn mutate_stored_board(
    state: &AppState,
    user_id: &str,
    id: &str,
    mutate: impl FnOnce(&mut AgenticBoard) -> Result<()>,
) -> Result<StoredBoard> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, id)?;
    if let Err(error) = mutate(&mut stored.board) {
        if has_persisted_planning_error(&stored.board) {
            stored.board.touch();
            save_board(state, &stored.board)?;
        }
        return Err(error);
    }
    stored.board.touch();
    save_board(state, &stored.board)?;
    Ok(stored)
}

fn project_execution_owner_key(project_path: &str) -> String {
    fs::canonicalize(project_path)
        .unwrap_or_else(|_| PathBuf::from(project_path))
        .display()
        .to_string()
}

fn claim_project_execution(project_path: &str, board_id: &str) -> Result<bool> {
    let owners = PROJECT_EXECUTION_OWNERS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut owners = owners.lock().map_err(|_| {
        ServerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Project execution ownership lock is unavailable.",
        )
    })?;
    let key = project_execution_owner_key(project_path);
    if let Some(owner) = owners.get(&key)
        && owner != board_id
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "Project execution is already owned by board {owner}. Pause it before starting another board for this project."
            ),
        ));
    }
    let already_owned = owners.get(&key).is_some_and(|owner| owner == board_id);
    owners.insert(key, board_id.to_string());
    Ok(!already_owned)
}

fn release_project_execution(project_path: &str, board_id: &str) {
    let Some(owners) = PROJECT_EXECUTION_OWNERS.get() else {
        return;
    };
    let Ok(mut owners) = owners.lock() else {
        return;
    };
    let key = project_execution_owner_key(project_path);
    if owners.get(&key).is_some_and(|owner| owner == board_id) {
        owners.remove(&key);
    }
}

fn start_board_execution(state: &AppState, user_id: &str, id: &str) -> Result<StoredBoard> {
    let (should_spawn, stored) = {
        let _guard = board_mutation_lock();
        let mut stored = load_user_board(state, user_id, id)?;
        if let Some(task_id) = unapproved_side_effect_task_ids(&stored.board).first() {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                format!(
                    "Subtask {task_id} requires declared and approved external side effects before the board can run."
                ),
            ));
        }
        let claimed_new = claim_project_execution(&stored.board.project_path, &stored.board.id)?;
        let should_spawn = !stored.board.loop_started || claimed_new;
        clear_board_abort_state(&mut stored.board);
        stored.board.status = "running".to_string();
        stored.board.scheduled_start_at = None;
        stored.board.active = true;
        stored.board.loop_started = true;
        stored.board.auto_run_enabled = true;
        stored.board.pause_requested = false;
        stored.board.paused_at = None;
        stored.board.pause_reason = None;
        bump_control_revision(&mut stored.board);
        stored.board.current_phase = Some("task_execution".to_string());
        stored.board.phase_started_at = Some(Utc::now());
        stored.board.phase_details = Some(json!({ "source": "kanban_board" }));
        stored.board.append_log("Agentic board execution started");
        stored.board.touch();
        if let Err(error) = save_board(state, &stored.board) {
            release_project_execution(&stored.board.project_path, &stored.board.id);
            return Err(error);
        }
        (should_spawn, stored)
    };

    if should_spawn {
        let state = state.clone();
        let user_id = user_id.to_string();
        let board_id = id.to_string();
        tokio::spawn(async move {
            if let Err(error) = execute_board_loop(state, user_id, board_id).await {
                tracing::warn!(error = %server_error_message(&error), "agentic board worker failed");
            }
        });
    }

    Ok(stored)
}

async fn execute_board_loop(state: AppState, user_id: String, board_id: String) -> Result<()> {
    let project_path = load_user_board(&state, &user_id, &board_id)
        .ok()
        .map(|stored| stored.board.project_path);
    let result = execute_board_loop_inner(state, user_id, board_id.clone()).await;
    if let Some(project_path) = project_path {
        release_project_execution(&project_path, &board_id);
    }
    result
}

async fn execute_board_loop_inner(
    state: AppState,
    user_id: String,
    board_id: String,
) -> Result<()> {
    loop {
        let mut stored = load_user_board(&state, &user_id, &board_id)?;
        if matches!(
            stored.board.status.as_str(),
            "cancelled" | "failed" | "blocked" | "completed"
        ) {
            if matches!(stored.board.status.as_str(), "failed" | "blocked") {
                let status_for_retry = stored.board.status.clone();
                schedule_auto_retry_if_eligible(&mut stored.board, &status_for_retry);
            }
            stored.board.active = false;
            stored.board.loop_started = false;
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        if stored.board.status == "paused" || stored.board.pause_requested {
            settle_board_pause(&mut stored.board);
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }

        if !stored.board.bootstrap_complete {
            bootstrap_agentic_board(&state, &user_id, &board_id).await?;
            continue;
        }

        if let Some(issue) = hierarchy_validation_issues(&stored.board)
            .into_iter()
            .next()
        {
            let affected = planning_error_task_ids(&stored.board, &issue);
            mark_planning_error(&mut stored.board, &affected, "hierarchy", &issue);
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        if let Some(cycle) = dependency_cycle(&stored.board) {
            let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
            mark_planning_error(&mut stored.board, &cycle, "dependency", &issue);
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        refresh_hierarchy_rollups(&mut stored.board);
        if let Some(parent) = next_hierarchy_parent(&stored.board) {
            stored.board.current_phase = Some("hierarchy_breakdown".to_string());
            stored.board.phase_started_at = Some(Utc::now());
            stored.board.phase_details = Some(json!({
                "parentId": parent.id,
                "parentLevel": task_level(&parent),
                "nextLevel": next_hierarchy_level(task_level(&parent)),
            }));
            stored.board.append_log(format!(
                "Breaking down {} {} into its next hierarchy level",
                task_level(&parent),
                parent.id
            ));
            stored.board.touch();
            save_board(&state, &stored.board)?;
            plan_hierarchy_children(&state, &user_id, &board_id, &parent.id, false).await?;
            continue;
        }

        reconcile_dependency_statuses(&mut stored.board);
        refresh_hierarchy_rollups(&mut stored.board);
        stored.board.touch();
        save_board(&state, &stored.board)?;

        let Some(task_index) = pick_next_task_index(&stored.board) else {
            if uses_hierarchical_orchestration(&stored.board) {
                reconcile_dependency_statuses(&mut stored.board);
                let waiting_tasks = dependency_waiting_tasks(&stored.board);
                let side_effect_tasks = unapproved_side_effect_task_ids(&stored.board);
                let pending_research = pending_research_acceptance_ids(&stored.board);
                if !side_effect_tasks.is_empty() {
                    mark_side_effect_blockers(&mut stored.board);
                    stored.board.status = TASK_STATUS_BLOCKED.to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_phase =
                        Some("waiting_for_side_effect_approval".to_string());
                    stored.board.pause_reason = Some(
                        "Approve declared external side effects before resuming execution."
                            .to_string(),
                    );
                    stored.board.append_log(format!(
                        "No runnable subtasks: {} require external side-effect approval ({})",
                        side_effect_tasks.len(),
                        side_effect_tasks.join(", ")
                    ));
                } else if !pending_research.is_empty() {
                    stored.board.status = "paused".to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_phase = Some("waiting_for_research_approval".to_string());
                    stored.board.pause_reason = Some(
                        "Accept completed research output before implementation can continue."
                            .to_string(),
                    );
                    stored.board.phase_details = Some(json!({
                        "researchTaskIds": pending_research,
                    }));
                    stored.board.append_log(
                        "Execution paused because completed research awaits user acceptance",
                    );
                } else if !waiting_tasks.is_empty() {
                    mark_dependency_blockers(&mut stored.board);
                    stored.board.status = TASK_STATUS_BLOCKED.to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_phase = Some("blocked".to_string());
                    stored.board.append_log(format!(
                        "No runnable subtasks: {} waiting on dependencies ({})",
                        waiting_tasks.len(),
                        waiting_tasks.join(", ")
                    ));
                } else if has_dependency_blocked_tasks(&stored.board) {
                    stored.board.status = TASK_STATUS_BLOCKED.to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_phase = Some("blocked".to_string());
                    stored.board.append_log(
                       "Hierarchy execution stopped on a dependency blocker; resolve the dependency before resuming",
                    );
                } else if has_hierarchical_attention_tasks(&stored.board) {
                    stored.board.status = TASK_STATUS_BLOCKED.to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_phase = Some("blocked".to_string());
                    stored.board.append_log(
                       "Hierarchy execution stopped at a failed or blocked subtask; retry it or approve a fix subtask",
                    );
                } else if hierarchical_work_is_complete(&stored.board)
                    && !has_backlog_planning_work(&stored.board)
                {
                    stored.board.status = "completed".to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_task_id = None;
                    stored.board.current_task_title.clear();
                    stored.board.current_task_status.clear();
                    stored.board.current_phase = Some("completed".to_string());
                    stored.board.phase_details = Some(json!({
                        "taskCount": stored.board.tasks.len(),
                        "execution": "subtasks_only",
                    }));
                    stored.board.final_review = Some(json!({
                        "complete": true,
                        "summary": "All approved executable subtasks completed.",
                    }));
                    stored.board.append_log(
                        "Hierarchy execution completed: all approved executable subtasks are done",
                    );
                } else {
                    stored.board.status = "paused".to_string();
                    stored.board.active = false;
                    stored.board.loop_started = false;
                    stored.board.current_phase = Some("waiting_for_approval".to_string());
                    stored.board.pause_reason = Some(
                       "No approved executable subtasks remain; move the next planning item to Todo."
                           .to_string(),
                   );
                    stored.board.append_log(
                        "Hierarchy execution paused because no approved executable subtasks remain",
                    );
                }
                stored.board.touch();
                save_board(&state, &stored.board)?;
                return Ok(());
            }
            if !stored.board.agents_knowledge_updated
                && append_agents_knowledge_task(
                    &mut stored.board,
                    "Implementation work completed before final QA",
                    None,
                )
            {
                stored.board.current_phase = Some("agents_update".to_string());
                stored.board.phase_started_at = Some(Utc::now());
                stored
                    .board
                    .append_log("Appended AGENTS.md knowledge update task");
                stored.board.touch();
                save_board(&state, &stored.board)?;
                continue;
            }
            if append_promotion_review_task(&mut stored.board, "Final promotion review") {
                stored.board.current_phase = Some("promotion_review".to_string());
                stored.board.phase_started_at = Some(Utc::now());
                stored
                    .board
                    .append_log("Appended RAG promotion review task");
                stored.board.touch();
                save_board(&state, &stored.board)?;
                continue;
            }
            stored.board.status = "completed".to_string();
            stored.board.active = false;
            stored.board.loop_started = false;
            stored.board.current_task_id = None;
            stored.board.current_task_title.clear();
            stored.board.current_task_status.clear();
            stored.board.current_phase = Some("completed".to_string());
            stored.board.phase_details = Some(json!({ "taskCount": stored.board.tasks.len() }));
            stored.board.final_qa_complete = true;
            stored.board.final_review = Some(json!({
                "complete": true,
                "summary": "All runnable board tasks completed.",
            }));
            stored.board.append_log("Agentic board execution completed");
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        };

        let task_id = stored.board.tasks[task_index].id.clone();
        let task_title = stored.board.tasks[task_index].title.clone();
        let started_at = Utc::now();
        stored.board.status = "running".to_string();
        stored.board.current_task_id = Some(task_id.clone());
        stored.board.current_task_title = task_title.clone();
        stored.board.current_task_status = "in_progress".to_string();
        let task_phase = stored
            .board
            .tasks
            .get(task_index)
            .map(|task| {
                if is_qa_task(task) {
                    "qa_task"
                } else if is_promotion_review_task(task) {
                    "promotion_review"
                } else if task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID {
                    "agents_update"
                } else {
                    "task_execution"
                }
            })
            .unwrap_or("task_execution");
        stored.board.current_phase = Some(task_phase.to_string());
        stored.board.phase_started_at = Some(started_at);
        stored.board.phase_details = Some(json!({ "taskId": task_id, "taskTitle": task_title }));
        apply_task_model_routing(&mut stored.board, task_index);
        if let Some(task) = stored.board.tasks.get_mut(task_index) {
            task.status = "in_progress".to_string();
            task.attempt_count = task.attempt_count.saturating_add(1);
            task.started_at = Some(started_at);
            task.completed_at = None;
            task.error = None;
            task.transcript.push(json!({
                "timestamp": started_at,
                "kind": "status",
                "status": "running",
                "content": "Task execution started",
            }));
            task.transcript_updated_at = Some(started_at);
        }
        stored.board.append_log(format!("Executing task {task_id}"));
        stored.board.touch();
        save_board(&state, &stored.board)?;

        if stored
            .board
            .tasks
            .get(task_index)
            .is_some_and(is_promotion_review_task)
        {
            let attempt_id = format!("attempt-{}", Uuid::new_v4());
            if let Some(task) = stored.board.tasks.get_mut(task_index) {
                let transcript_start_index = task.transcript.len().saturating_sub(1);
                task.hierarchy.attempts.push(json!({
                    "attemptId": attempt_id,
                    "attemptNumber": task.attempt_count,
                    "startedAt": started_at,
                    "status": "running",
                    "transcriptStartIndex": transcript_start_index,
                }));
            }
            stored.board.touch();
            save_board(&state, &stored.board)?;
            if let Err(error) = execute_promotion_review_task(
                &state,
                &user_id,
                &board_id,
                &mut stored.board,
                task_index,
            )
            .await
            {
                let now = Utc::now();
                if let Some(task) = stored.board.tasks.get_mut(task_index) {
                    task.status = TASK_STATUS_FAILED.to_string();
                    task.error = Some(server_error_message(&error));
                    task.completed_at = Some(now);
                    task.qa_passed = Some(false);
                }
                finish_task_attempt(
                    &mut stored.board,
                    &task_id,
                    &attempt_id,
                    TASK_STATUS_FAILED,
                    now,
                );
                stored.board.touch();
                save_board(&state, &stored.board)?;
                return Err(error);
            }
            let attempt_status = stored
                .board
                .tasks
                .get(task_index)
                .map(|task| canonical_task_status(&task.status))
                .unwrap_or(TASK_STATUS_FAILED);
            finish_task_attempt(
                &mut stored.board,
                &task_id,
                &attempt_id,
                attempt_status,
                Utc::now(),
            );
            stored.board.current_task_id = None;
            stored.board.current_task_title.clear();
            stored.board.current_task_status.clear();
            stored.board.touch();
            save_board(&state, &stored.board)?;
            continue;
        }

        let managed_git_ready =
            ensure_managed_git_branch_for_task_group(&mut stored.board, &task_id).await;
        if let Err(error) = managed_git_ready {
            let message = server_error_message(&error);
            if let Some(task) = stored
                .board
                .tasks
                .iter_mut()
                .find(|task| task.id == task_id)
            {
                task.status = "blocked".to_string();
                task.error = Some(message.clone());
                task.summary = message.clone();
                task.completed_at = Some(Utc::now());
                task.qa_passed = Some(false);
            }
            stored.board.status = "blocked".to_string();
            stored.board.active = false;
            stored.board.loop_started = false;
            stored.board.append_log(format!(
                "Task blocked before execution by managed git policy: {message}"
            ));
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        save_board(&state, &stored.board)?;

        if !ensure_tdd_baseline_for_task(&state, &user_id, &board_id, &mut stored.board, task_index)
            .await?
        {
            stored.board.current_task_id = None;
            stored.board.current_task_title.clear();
            stored.board.current_task_status.clear();
            stored.board.touch();
            save_board(&state, &stored.board)?;
            continue;
        }
        stored = load_user_board(&state, &user_id, &board_id)?;
        if stored.board.status == "paused" || stored.board.pause_requested {
            settle_board_pause(&mut stored.board);
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        let Some(task_index) = stored
            .board
            .tasks
            .iter()
            .position(|task| task.id == task_id)
        else {
            continue;
        };
        if let Some(task) = stored.board.tasks.get_mut(task_index) {
            if task.tdd_phase == "qa_failed_expected" {
                task.tdd_phase = "dev_pending".to_string();
            }
        }

        attach_rag_context_for_task(&mut stored.board, task_index).await;
        stored.board.touch();
        save_board(&state, &stored.board)?;
        stored = load_user_board(&state, &user_id, &board_id)?;
        if stored.board.status == "paused" || stored.board.pause_requested {
            settle_board_pause(&mut stored.board);
            stored.board.touch();
            save_board(&state, &stored.board)?;
            return Ok(());
        }
        let Some(task_index) = stored
            .board
            .tasks
            .iter()
            .position(|task| task.id == task_id)
        else {
            continue;
        };

        let before_workspace = capture_workspace_snapshot(&stored.board.project_path);
        stored.board.provider_call_started_at = Some(Utc::now());
        stored.board.provider_call_label = Some(format!("task execution for {task_id}"));
        let attempt_id = format!("attempt-{}", Uuid::new_v4());
        if let Some(task) = stored
            .board
            .tasks
            .iter_mut()
            .find(|task| task.id == task_id)
        {
            let transcript_start_index = task.transcript.len().saturating_sub(1);
            task.hierarchy.attempts.push(json!({
                "attemptId": attempt_id,
                "attemptNumber": task.attempt_count,
                "startedAt": started_at,
                "status": "running",
                "transcriptStartIndex": transcript_start_index,
            }));
        }
        stored.board.touch();
        save_board(&state, &stored.board)?;
        let provider_attempt =
            execute_provider_task_with_fallback(&state, &stored.board, task_index).await;
        let mut stored = load_user_board(&state, &user_id, &board_id)?;
        let task_position = stored
            .board
            .tasks
            .iter()
            .position(|task| task.id == task_id);
        let now = Utc::now();

        if let Some(fallback) = provider_attempt.fallback {
            let previous_provider = stored.board.provider.clone();
            let previous_model = stored.board.model.clone();
            stored.board.provider = fallback.provider.clone();
            stored.board.model = fallback.model.clone();
            stored.board.last_effective_model = Some(fallback.model.clone());
            reset_provider_session(&mut stored.board, "provider fallback");
            stored.board.model_history.push(json!({
                "fromProvider": previous_provider,
                "from": previous_model,
                "toProvider": fallback.provider,
                "to": fallback.model,
                "changedAt": Utc::now(),
                "changedBy": "provider-fallback",
                "reason": fallback.reason,
                "taskId": task_id,
            }));
            stored
                .board
                .append_log("Primary provider call failed; activated configured fallback");
        }

        match provider_attempt.result {
            Ok(mut result) => {
                let mut parsed = parse_execution_result(&result.assistant_text)
                    .unwrap_or_else(|| missing_json_task_result(&result.assistant_text));
                let mut fatal_provider_errors =
                    filter_fatal_provider_errors(&result.errors, result.exit_code);
                if result.errors.len() > fatal_provider_errors.len() {
                    stored.board.append_log(format!(
                        "Ignored {} non-fatal provider advisory message(s) for {task_id}",
                        result.errors.len() - fatal_provider_errors.len()
                    ));
                }
                let mut failed_with_provider_error =
                    result.exit_code != 0 || !fatal_provider_errors.is_empty();

                if !failed_with_provider_error
                    && is_recoverable_self_reported_blocker(&parsed)
                    && stored
                        .board
                        .tasks
                        .get(task_position.unwrap_or(task_index))
                        .map(|task| task.attempt_count < max_task_attempts(&stored.board))
                        .unwrap_or(false)
                {
                    let stale_tool_blocker = is_tool_environment_self_reported_blocker(&parsed)
                        && !provider_events_have_tool_evidence(&result.stream_events);
                    if stale_tool_blocker {
                        reset_provider_session(
                            &mut stored.board,
                            &format!("stale tool-environment blocker reported by {task_id}"),
                        );
                    }
                    if let Some(index) = task_position {
                        if let Some(task) = stored.board.tasks.get_mut(index) {
                            task.attempt_count = task.attempt_count.saturating_add(1);
                            if stale_tool_blocker {
                                task.provider_session_id = None;
                            } else if let Some(session_id) = result.session_id.clone() {
                                task.provider_session_id = Some(session_id);
                            }
                            task.transcript.extend(result.stream_events.clone());
                            task.transcript.push(json!({
                                "timestamp": Utc::now(),
                                "kind": "status",
                                "status": "retrying",
                                "content": if stale_tool_blocker {
                                    "Retrying stale tool-environment blocker in a fresh provider session"
                                } else {
                                    "Retrying recoverable self-reported blocker"
                                },
                            }));
                            task.transcript_updated_at = Some(Utc::now());
                        }
                    }
                    stored.board.append_log(if stale_tool_blocker {
                        format!("Retrying stale tool-environment blocker for {task_id} in a fresh session")
                    } else {
                        format!("Retrying recoverable blocker for {task_id}")
                    });
                    stored.board.touch();
                    save_board(&state, &stored.board)?;

                    match execute_provider_task(&state, &stored.board, task_index).await {
                        Ok(retry_result) => {
                            result = retry_result;
                            parsed = parse_execution_result(&result.assistant_text).unwrap_or_else(
                                || missing_json_task_result(&result.assistant_text),
                            );
                            fatal_provider_errors =
                                filter_fatal_provider_errors(&result.errors, result.exit_code);
                            if result.errors.len() > fatal_provider_errors.len() {
                                stored.board.append_log(format!(
                                    "Ignored {} non-fatal provider advisory message(s) for {task_id}",
                                    result.errors.len() - fatal_provider_errors.len()
                                ));
                            }
                            failed_with_provider_error =
                                result.exit_code != 0 || !fatal_provider_errors.is_empty();
                        }
                        Err(error) => {
                            result = ProviderTaskResult::from_error(error);
                            parsed = missing_json_task_result(&result.assistant_text);
                            fatal_provider_errors =
                                filter_fatal_provider_errors(&result.errors, result.exit_code);
                            failed_with_provider_error = true;
                        }
                    }
                }

                let change_summary =
                    record_task_workspace_changes(&mut stored.board, &task_id, before_workspace);
                if failed_with_provider_error
                    && should_treat_provider_errors_as_followup(&result, &parsed, &change_summary)
                {
                    parsed = convert_missing_json_provider_error_to_followup(&parsed, &result);
                    failed_with_provider_error = false;
                    stored.board.append_log(format!(
                        "Converted provider error to follow-up for {task_id} because the task changed files but missed final JSON"
                    ));
                }
                if !failed_with_provider_error
                    && should_repair_task_result(&stored.board, &task_id, &parsed, &change_summary)
                {
                    parsed = repair_task_result_if_needed(
                        &state,
                        &user_id,
                        &board_id,
                        &task_id,
                        task_index,
                        &result.assistant_text,
                        parsed,
                        &change_summary,
                    )
                    .await;
                }
                let is_agents_knowledge_task = stored
                    .board
                    .tasks
                    .get(task_position.unwrap_or(task_index))
                    .map(|task| task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID)
                    .unwrap_or(false);
                if !failed_with_provider_error && !is_agents_knowledge_task {
                    let task_for_validation = stored
                        .board
                        .tasks
                        .get(task_position.unwrap_or(task_index))
                        .cloned();
                    let validation = if let Some(task) = task_for_validation.as_ref() {
                        run_tdd_validation(&stored.board, task, "feature").await
                    } else {
                        run_deterministic_validation(&stored.board, &task_id, "feature").await
                    };
                    parsed = apply_deterministic_validation_result(parsed, &validation);
                    stored.board.validation_runs.push(validation.clone());
                    if let Some(index) = task_position {
                        if let Some(task) = stored.board.tasks.get_mut(index) {
                            task.deterministic_validation = Some(validation);
                            if task.tdd_phase != "disabled" && !is_qa_task(task) {
                                if task
                                    .deterministic_validation
                                    .as_ref()
                                    .and_then(|value| value.get("passed"))
                                    .and_then(Value::as_bool)
                                    == Some(true)
                                {
                                    task.tdd_phase = "evidence_review".to_string();
                                } else {
                                    task.tdd_phase = "fix_pending".to_string();
                                    task.fix_attempts = task.fix_attempts.saturating_add(1);
                                }
                            }
                        }
                    }
                }
                if !failed_with_provider_error {
                    parsed = apply_completion_evidence_gate(
                        &stored.board,
                        &task_id,
                        parsed,
                        &change_summary,
                    );
                }
                refresh_codebase_context_after_task(&mut stored.board, &change_summary);
                let completion_summary = resolved_execution_summary(&parsed, &result.summary);
                let hierarchical_execution = uses_hierarchical_orchestration(&stored.board);
                if let Some(index) = task_position {
                    let task = &mut stored.board.tasks[index];
                    if result.session_id.is_some() {
                        task.provider_session_id = result.session_id.clone();
                    }
                    task.transcript.extend(result.stream_events.clone());
                    task.transcript.push(json!({
                        "timestamp": now,
                        "kind": "assistant",
                        "provider": stored.board.provider,
                        "content": result.assistant_text,
                    }));
                    if !result.stderr.trim().is_empty() {
                        task.transcript.push(json!({
                            "timestamp": now,
                            "kind": "stderr",
                            "provider": stored.board.provider,
                            "content": result.stderr,
                        }));
                    }
                    task.transcript.push(json!({
                        "timestamp": now,
                        "kind": "complete",
                        "exitCode": result.exit_code,
                        "content": completion_summary,
                    }));
                    task.transcript_updated_at = Some(now);
                    task.completed_at = Some(now);
                    task.summary = completion_summary;
                    task.result = Some(parsed.clone());
                    task.changed_file_summary = Some(change_summary.clone());
                    task.commands_run = value_to_strings(parsed.get("commandsRun").cloned());
                    let attributable_changed_files = change_summary_paths(&change_summary);
                    if hierarchical_execution {
                        if let Some(object) = parsed.as_object_mut() {
                            object.insert(
                                "changedFiles".to_string(),
                                json!(attributable_changed_files.clone()),
                            );
                        }
                        task.result = Some(parsed.clone());
                        task.changed_files = dedupe_strings(
                            task.changed_files
                                .clone()
                                .into_iter()
                                .chain(attributable_changed_files.clone())
                                .collect(),
                        );
                    } else {
                        task.changed_files = value_to_strings(parsed.get("changedFiles").cloned());
                        if task.changed_files.is_empty() {
                            task.changed_files = attributable_changed_files;
                        }
                    }
                    task.evidence = value_to_strings(parsed.get("evidence").cloned());
                    task.hierarchy.side_effect_evidence = external_side_effect_evidence(&parsed);
                    if canonical_task_kind(task) == TASK_KIND_MANUAL_TEST {
                        let environment = manual_test_environment_evidence(&parsed);
                        let has_environment = environment
                            .as_object()
                            .is_some_and(|object| !object.is_empty());
                        task.hierarchy.manual_test_environment =
                            has_environment.then_some(environment);
                    }
                    task.remaining_issues = value_to_strings(
                        parsed
                            .get("remainingIssues")
                            .cloned()
                            .or_else(|| parsed.get("remainingGaps").cloned()),
                    );
                    if failed_with_provider_error {
                        let error = if fatal_provider_errors.is_empty() {
                            format!("Provider exited with code {}", result.exit_code)
                        } else {
                            fatal_provider_errors.join("\n")
                        };
                        task.status = "blocked".to_string();
                        task.qa_passed = Some(false);
                        task.error = Some(limit_text(&error, 1200));
                        task.summary = parsed
                            .get("summary")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .filter(|summary| !summary.trim().is_empty())
                            .unwrap_or_else(|| task.error.clone().unwrap_or_default());
                    } else if completion_evidence_gate_failed(&parsed) {
                        task.status = TASK_STATUS_BLOCKED.to_string();
                        task.qa_passed = Some(false);
                        task.error = Some(
                            "Completion evidence gate failed; provide valid evidence before this subtask can be done."
                                .to_string(),
                        );
                        task.completed_at = None;
                        if task.tdd_phase != "disabled" && !is_qa_task(task) {
                            task.tdd_phase = "followup_pending".to_string();
                            task.fix_attempts = task.fix_attempts.saturating_add(1);
                        }
                    } else if parsed_status_done(Some(&parsed)) {
                        task.status = TASK_STATUS_DONE.to_string();
                        task.qa_passed = Some(parsed_qa_passed(Some(&parsed)));
                        task.error = None;
                        if task.tdd_phase != "disabled" && !is_qa_task(task) {
                            task.tdd_phase = "done".to_string();
                            if let Some(validation) = task.deterministic_validation.as_ref() {
                                task.coverage_evidence.push(json!({
                                    "kind": "feature_validation",
                                    "validation": validation,
                                    "recordedAt": Utc::now(),
                                }));
                            }
                        }
                        if task.final_qa_task && task.qa_passed == Some(true) {
                            stored.board.final_qa_complete = true;
                        }
                        if task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID {
                            stored.board.agents_knowledge_updated = true;
                            stored.board.agents_context =
                                Some(read_agents_context(&stored.board.project_path));
                        }
                    } else {
                        let needs_followup = parsed
                            .get("status")
                            .and_then(Value::as_str)
                            .is_some_and(|status| status == "needs_followup");
                        let requires_user_fix = hierarchical_execution
                            && !matches!(
                                canonical_task_kind(task),
                                TASK_KIND_QA | TASK_KIND_MANUAL_TEST | TASK_KIND_REVIEW
                            );
                        task.status = if needs_followup && !requires_user_fix {
                            TASK_STATUS_DONE.to_string()
                        } else if needs_followup {
                            TASK_STATUS_BLOCKED.to_string()
                        } else {
                            TASK_STATUS_FAILED.to_string()
                        };
                        if task.tdd_phase != "disabled" && !is_qa_task(task) {
                            task.tdd_phase = if needs_followup {
                                "followup_pending".to_string()
                            } else {
                                "fix_pending".to_string()
                            };
                            task.fix_attempts = task.fix_attempts.saturating_add(1);
                        }
                        task.qa_passed = Some(false);
                        task.error = if needs_followup && requires_user_fix {
                            Some(
                                "Incomplete work remains inside the approved scope. Create a concrete fix subtask under this parent."
                                    .to_string(),
                            )
                        } else if needs_followup {
                            None
                        } else {
                            Some(
                                parsed
                                    .get("summary")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                                    .unwrap_or_else(|| {
                                        format!("Provider exited with code {}", result.exit_code)
                                    }),
                            )
                        };
                    }
                }
                let attempt_status = task_position
                    .and_then(|index| stored.board.tasks.get(index))
                    .map(|task| canonical_task_status(&task.status))
                    .unwrap_or(TASK_STATUS_FAILED);
                finish_task_attempt(
                    &mut stored.board,
                    &task_id,
                    &attempt_id,
                    attempt_status,
                    now,
                );
                if let Some(session_id) = result.session_id.clone() {
                    stored.board.current_provider_session_id = Some(session_id.clone());
                    stored.board.actual_session_id = Some(session_id.clone());
                    if stored.board.session_id.is_none()
                        || should_resume_provider_session(&stored.board)
                    {
                        stored.board.session_id = Some(session_id);
                    }
                }
                if let Some(task_for_usage) = stored.board.tasks.get(task_index) {
                    let prompt_for_usage =
                        build_task_execution_prompt(&stored.board, task_for_usage, task_index);
                    increment_provider_usage(
                        &mut stored.board,
                        &prompt_for_usage,
                        &result.assistant_text,
                        result.session_id.as_deref(),
                        result.token_usage.as_ref(),
                    );
                }
                apply_task_result_to_board(&mut stored.board, &task_id, &parsed);
                if !failed_with_provider_error {
                    ingest_rag_task_outcome(&mut stored.board, &task_id, &parsed).await;
                }
                if !failed_with_provider_error {
                    append_suggested_backlog_tasks_from_result(
                        &mut stored.board,
                        &task_id,
                        &parsed,
                    );
                }
                let qa_followup_added = if failed_with_provider_error {
                    false
                } else if should_queue_qa_verdict_retry(
                    &stored.board,
                    &task_id,
                    &parsed,
                    &change_summary,
                ) {
                    queue_qa_verdict_retry(&mut stored.board, &task_id, &parsed)
                } else if is_qa_verdict_retry_task_id(&stored.board, &task_id)
                    && is_missing_final_json_result(&parsed)
                {
                    mark_qa_verdict_retry_blocked(&mut stored.board, &task_id, &parsed);
                    true
                } else if is_qa_task_id(&stored.board, &task_id) && qa_needs_followup(&parsed) {
                    append_followup_task_if_needed(&mut stored.board, &task_id, &parsed)
                } else {
                    false
                };
                let followup_added = if failed_with_provider_error || qa_followup_added {
                    false
                } else {
                    append_followup_task_if_needed(&mut stored.board, &task_id, &parsed)
                };
                if uses_hierarchical_orchestration(&stored.board) {
                    refresh_hierarchy_rollups(&mut stored.board);
                }
                let post_qa_added =
                    if failed_with_provider_error || qa_followup_added || followup_added {
                        false
                    } else {
                        let source_task = stored
                            .board
                            .tasks
                            .iter()
                            .find(|task| task.id == task_id)
                            .cloned();
                        source_task
                            .as_ref()
                            .filter(|task| {
                                task_is_done(task)
                                    && task_needs_immediate_ai_qa(&stored.board, task, &parsed)
                                    && !has_task_qa_for_source(&stored.board, &task.id)
                            })
                            .map(|task| {
                                append_task_qa_task(
                                    &mut stored.board,
                                    task,
                                    "Validate immediately after implementation task completion",
                                )
                            })
                            .unwrap_or(false)
                    };
                let post_agents_added = if failed_with_provider_error
                    || qa_followup_added
                    || followup_added
                    || post_qa_added
                {
                    false
                } else {
                    let source_task = stored
                        .board
                        .tasks
                        .iter()
                        .find(|task| task.id == task_id)
                        .cloned();
                    source_task
                        .as_ref()
                        .filter(|task| {
                            task_is_done(task)
                                && task_needs_agents_knowledge_update(task, &parsed)
                                && !has_agents_knowledge_task_for_source(&stored.board, &task.id)
                        })
                        .map(|task| {
                            append_agents_knowledge_task(
                                &mut stored.board,
                                "Preserve durable code structure, command, database, migration, or verification knowledge for later tasks",
                                Some(task),
                            )
                        })
                        .unwrap_or(false)
                };
                if !qa_followup_added && !followup_added {
                    if let Some(entry) =
                        compact_provider_session_after_task_group(&state, &stored.board, &task_id)
                            .await
                    {
                        stored.board.compaction_ledger.push(entry);
                    }
                }
                if post_agents_added {
                    stored
                        .board
                        .append_log(format!("Inserted post-task AGENTS work after {task_id}"));
                }
                if post_qa_added {
                    stored
                        .board
                        .append_log(format!("Inserted post-task QA work after {task_id}"));
                }
                let completed_for_git = stored
                    .board
                    .tasks
                    .iter()
                    .find(|task| task.id == task_id)
                    .is_some_and(task_is_done);
                if completed_for_git {
                    if let Err(error) =
                        finalize_managed_git_task_group(&mut stored.board, &task_id).await
                    {
                        let message = server_error_message(&error);
                        if let Some(task) = stored
                            .board
                            .tasks
                            .iter_mut()
                            .find(|task| task.id == task_id)
                        {
                            task.status = "blocked".to_string();
                            task.error = Some(message.clone());
                            task.summary = if task.summary.trim().is_empty() {
                                message.clone()
                            } else {
                                format!("{} {}", task.summary, message)
                            };
                        }
                        stored.board.status = "blocked".to_string();
                        stored.board.active = false;
                        stored.board.loop_started = false;
                        stored.board.append_log(format!(
                            "Blocked after completion by managed git policy: {message}"
                        ));
                        stored.board.touch();
                        save_board(&state, &stored.board)?;
                        return Ok(());
                    }
                }
                stored.board.append_log(format!(
                    "Task {task_id} finished with exit code {}",
                    result.exit_code
                ));
            }
            Err(error) => {
                let message = server_error_message(&error);
                if let Some(index) = task_position {
                    let task = &mut stored.board.tasks[index];
                    task.status = "failed".to_string();
                    task.error = Some(message.clone());
                    task.completed_at = Some(now);
                    task.qa_passed = Some(false);
                    task.transcript.push(json!({
                        "timestamp": now,
                        "kind": "error",
                        "isError": true,
                        "content": message,
                    }));
                    task.transcript_updated_at = Some(now);
                }
                finish_task_attempt(
                    &mut stored.board,
                    &task_id,
                    &attempt_id,
                    TASK_STATUS_FAILED,
                    now,
                );
                stored.board.append_log(format!(
                    "Task {task_id} failed: {}",
                    server_error_message(&error)
                ));
            }
        }

        stored.board.current_task_id = None;
        stored.board.current_task_title.clear();
        stored.board.current_task_status.clear();
        stored.board.provider_call_started_at = None;
        stored.board.provider_call_label = None;
        stored.board.current_provider_session_id = None;
        stored.board.touch();
        save_board(&state, &stored.board)?;
    }
}

async fn bootstrap_agentic_board(state: &AppState, user_id: &str, board_id: &str) -> Result<()> {
    let snapshot = load_user_board(state, user_id, board_id)?.board;
    if bootstrap_should_yield(&snapshot) {
        return Ok(());
    }
    let workspace_baseline = snapshot
        .workspace_baseline
        .is_none()
        .then(|| capture_workspace_snapshot(&snapshot.project_path));
    let agents_context = read_agents_context(&snapshot.project_path);
    mutate_stored_board(state, user_id, board_id, |run| {
        set_phase(
            run,
            "bootstrap_prepare",
            json!({ "step": "guidance_and_sources" }),
        );
        if let Some(workspace_baseline) = workspace_baseline.clone() {
            run.workspace_baseline = Some(workspace_baseline.clone());
            run.latest_workspace_snapshot = Some(workspace_baseline);
        }
        run.agents_context = Some(agents_context);
        run.append_log("Loaded AGENTS.md guidance and workspace baseline");
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, board_id)? {
        return Ok(());
    }

    let snapshot = load_user_board(state, user_id, board_id)?.board;
    let source_prompt = active_board_prompt(&snapshot);
    let source_bundle = build_source_bundle(&snapshot.project_path, &source_prompt);
    mutate_stored_board(state, user_id, board_id, |run| {
        run.source_references = source_bundle.references;
        run.source_manifest = source_bundle.manifest;
        run.source_chunks = source_bundle.chunks;
        run.append_log(format!(
            "Resolved {} source references, {} source files and {} source chunks",
            run.source_references.len(),
            run.source_manifest.len(),
            run.source_chunks.len()
        ));
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, board_id)? {
        return Ok(());
    }

    let mut rag_snapshot = load_user_board(state, user_id, board_id)?.board;
    let rag_ingestion_count = rag_snapshot.rag_ingestions.len();
    let rag_trace_count = rag_snapshot.rag_trace_refs.len();
    index_project_for_rag(&mut rag_snapshot).await;
    let rag_ingestions = rag_snapshot
        .rag_ingestions
        .into_iter()
        .skip(rag_ingestion_count)
        .collect::<Vec<_>>();
    let rag_trace_refs = rag_snapshot
        .rag_trace_refs
        .into_iter()
        .skip(rag_trace_count)
        .collect::<Vec<_>>();
    mutate_stored_board(state, user_id, board_id, |run| {
        run.rag_ingestions.extend(rag_ingestions);
        run.rag_trace_refs.extend(rag_trace_refs);
        trim_rag_history(run);
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, board_id)? {
        return Ok(());
    }

    let source_chunk_count = load_user_board(state, user_id, board_id)?
        .board
        .source_chunks
        .len();
    mutate_stored_board(state, user_id, board_id, |run| {
        set_phase(
            run,
            "codebase_manifest",
            json!({ "sourceChunks": source_chunk_count, "planning": "ticket_hierarchy" }),
        );
        Ok(())
    })?;

    let project_path = load_user_board(state, user_id, board_id)?
        .board
        .project_path;
    let codebase_bundle = build_codebase_bundle(&project_path);
    mutate_stored_board(state, user_id, board_id, |run| {
        set_phase(
            run,
            "codebase_manifest",
            json!({ "step": "build_manifest" }),
        );
        run.codebase_manifest = codebase_bundle.manifest;
        run.codebase_chunks = codebase_bundle.chunks;
        run.append_log(format!(
            "Loaded codebase manifest with {} files and {} chunks",
            run.codebase_manifest.len(),
            run.codebase_chunks.len()
        ));
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, board_id)? {
        return Ok(());
    }

    mutate_stored_board(state, user_id, board_id, |run| {
        set_phase(run, "codebase_recon", json!({}));
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, board_id)? {
        return Ok(());
    }
    let codebase_map = perform_codebase_recon(state, user_id, board_id).await?;
    mutate_stored_board(state, user_id, board_id, |run| {
        run.codebase_map = Some(codebase_map.clone());
        run.environment_state = Some(environment_from_codebase_map(&codebase_map));
        run.bootstrap_complete = true;
        set_phase(run, "task_execution", json!({ "bootstrapComplete": true }));
        run.append_log("Agentic bootstrap complete");
        Ok(())
    })?;
    Ok(())
}

fn bootstrap_checkpoint_requested(state: &AppState, user_id: &str, board_id: &str) -> Result<bool> {
    Ok(bootstrap_should_yield(
        &load_user_board(state, user_id, board_id)?.board,
    ))
}

fn bootstrap_should_yield(run: &AgenticBoard) -> bool {
    run.pause_requested
        || matches!(
            run.status.as_str(),
            "pausing" | "paused" | "cancelled" | "failed" | "blocked" | "completed"
        )
}

async fn perform_codebase_recon(state: &AppState, user_id: &str, board_id: &str) -> Result<Value> {
    let stored = load_user_board(state, user_id, board_id)?;
    let local_snapshot = local_codebase_snapshot(&stored.board);
    let prompt = build_codebase_recon_prompt(&stored.board, &local_snapshot);
    let parsed = execute_internal_prompt(state, user_id, board_id, "codebase recon", &prompt)
        .await
        .ok()
        .and_then(|text| parse_json_object(&text))
        .unwrap_or_else(|| json!({}));
    Ok(json!({
        "localSnapshot": local_snapshot,
        "summary": parsed.get("summary").and_then(Value::as_str).unwrap_or("Static codebase snapshot; task sessions inspect relevant files directly."),
        "architecture": normalize_string_list(parsed.get("architecture")),
        "implementedCapabilities": normalize_string_list(parsed.get("implementedCapabilities")),
        "missingCapabilities": normalize_string_list(parsed.get("missingCapabilities")),
        "conventions": normalize_string_list(parsed.get("conventions")),
        "runCommands": normalize_string_list(parsed.get("runCommands")),
        "testCommands": normalize_string_list(parsed.get("testCommands")),
        "relevantFiles": normalize_string_list(parsed.get("relevantFiles")),
        "risks": normalize_string_list(parsed.get("risks")),
        "completedAt": Utc::now(),
    }))
}

fn next_hierarchy_level(level: &str) -> Option<&'static str> {
    match level {
        TASK_LEVEL_INITIATIVE => Some(TASK_LEVEL_EPIC),
        TASK_LEVEL_EPIC => Some(TASK_LEVEL_STORY),
        TASK_LEVEL_STORY => Some(TASK_LEVEL_TASK),
        TASK_LEVEL_TASK => Some(TASK_LEVEL_SUBTASK),
        _ => None,
    }
}

fn direct_hierarchy_children<'a>(run: &'a AgenticBoard, parent_id: &str) -> Vec<&'a BoardTask> {
    run.tasks
        .iter()
        .filter(|task| task.hierarchy.parent_id.as_deref() == Some(parent_id))
        .collect()
}

fn hierarchy_breakdown_child_title_candidates(
    run: &AgenticBoard,
    parent_id: &str,
    next_level: &str,
) -> Vec<String> {
    let mut titles = direct_hierarchy_children(run, parent_id)
        .into_iter()
        .filter(|task| task_level(task) == next_level)
        .map(|task| normalize_suggested_task_key(&task.title))
        .filter(|title| !title.is_empty())
        .collect::<BTreeSet<_>>();

    // Risky or out-of-scope generated children are wrapped in an unparented
    // Backlog story. Keep their original title key tied to the source parent
    // so a later manual refinement cannot recreate the same proposal.
    for task in &run.tasks {
        if task.task_origin != "hierarchy_backlog_wrapper" {
            continue;
        }
        for entry in &task.hierarchy.discussion {
            if entry.get("kind").and_then(Value::as_str) != Some("generated_scope_wrapper")
                || entry.get("sourceParentId").and_then(Value::as_str) != Some(parent_id)
                || entry.get("wrappedLevel").and_then(Value::as_str) != Some(next_level)
            {
                continue;
            }
            if let Some(title) = entry.get("sourceTitleKey").and_then(Value::as_str) {
                let title = normalize_suggested_task_key(title);
                if !title.is_empty() {
                    titles.insert(title);
                }
            }
        }
    }

    titles.into_iter().collect()
}

fn hierarchy_breakdown_has_children(run: &AgenticBoard, parent_id: &str, next_level: &str) -> bool {
    !hierarchy_breakdown_child_title_candidates(run, parent_id, next_level).is_empty()
}

fn hierarchy_breakdown_max_new_children(existing_count: usize) -> usize {
    MAX_HIERARCHY_CHILDREN_PER_PARENT
        .saturating_sub(existing_count)
        .min(if existing_count > 0 {
            MAX_HIERARCHY_REFINEMENT_ADDITIONS
        } else {
            MAX_HIERARCHY_CHILDREN_PER_PARENT
        })
}

fn validate_hierarchy_breakdown_parent(run: &AgenticBoard, parent: &BoardTask) -> Result<()> {
    if task_status_is_todo(&parent.status) && !task_ancestors_are_approved(run, parent) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Approve every parent planning item before breaking down a nested Todo item.",
        ));
    }
    Ok(())
}

fn next_hierarchy_parent(run: &AgenticBoard) -> Option<BoardTask> {
    let mut candidates = run
        .tasks
        .iter()
        .filter(|task| task_status_is_todo(&task.status))
        .filter(|task| !task_is_executable(task))
        .filter(|task| task_ancestors_are_approved(run, task))
        .filter_map(|task| {
            let next_level = next_hierarchy_level(task_level(task))?;
            let has_children = hierarchy_breakdown_has_children(run, &task.id, next_level);
            (!has_children).then(|| (task_level(task), task.clone()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(level, task)| {
        let depth = match *level {
            TASK_LEVEL_TASK => 0,
            TASK_LEVEL_STORY => 1,
            TASK_LEVEL_EPIC => 2,
            TASK_LEVEL_INITIATIVE => 3,
            _ => 4,
        };
        (depth, task_priority_rank(&task.priority), task.id.clone())
    });
    candidates.into_iter().next().map(|(_, task)| task)
}

fn build_hierarchy_breakdown_prompt(
    run: &AgenticBoard,
    parent: &BoardTask,
    next_level: &str,
) -> String {
    let existing_titles = hierarchy_breakdown_child_title_candidates(run, &parent.id, next_level);
    let refinement = !existing_titles.is_empty();
    let max_new_children = hierarchy_breakdown_max_new_children(existing_titles.len());
    let breakdown_mode = if refinement {
        "refinement / gap check"
    } else {
        "initial breakdown"
    };
    let children = hierarchy_breakdown_child_title_candidates(run, &parent.id, next_level)
        .into_iter()
        .map(|title| format!("- {title}"))
        .collect::<Vec<_>>()
        .join("\n");
    let direct_children = direct_hierarchy_children(run, &parent.id)
        .into_iter()
        .filter(|child| task_level(child) == next_level)
        .map(|child| format!("{} [{}] {}", child.id, task_level(child), child.title))
        .collect::<Vec<_>>()
        .join("\n");
    let acceptance = if parent.acceptance_criteria.is_empty() {
        "None recorded; derive concrete criteria from the parent description.".to_string()
    } else {
        parent.acceptance_criteria.join("\n- ")
    };
    format!(
        r#"Break down exactly one approved Kanban item into its next hierarchy level.

Parent:
- id: {id}
- level: {level}
- title: {title}
- description: {description}
- acceptance criteria:
- {acceptance}

Next level: {next_level}
Breakdown mode: {breakdown_mode}
Existing next-level child titles:
{children}
Existing direct child records:
{direct_children}

Codebase context:
{codebase}

Return JSON only. No markdown fence.
Schema:
{{
  "items": [
    {{
      "level": "{next_level}",
      "title": "specific title",
      "kind": "research|design|implementation|test_implementation|qa|manual_test|review|fix|migration|revert|cleanup|revision|replacement",
      "description": "one-purpose scope",
      "acceptanceCriteria": ["specific verifiable outcome"],
      "priority": "p0|p1|p2|p3",
      "blockedBy": [],
      "required": true,
      "plannedFiles": [],
      "sideEffects": []
    }}
  ]
}}

Rules:
- Create only the next level, never skip a level.
- One subtask has one engineering purpose. Separate implementation, test-writing, QA, review, and manual testing into separate subtasks.
- Subtask titles must be concrete engineer execution tickets, such as `Add endpoint PATCH /budget/{{month}}` or `Run Android emulator smoke test for budget edit flow`.
- Do not create nice-to-have work as a required child. Put it in a separate Backlog story.
- Return no more than {max_new_children} genuinely new child item(s).
- If an existing child already covers the work, do not return it again, even when the title is worded differently.
- In refinement / gap check mode, return an empty items array when the existing children already cover the parent.
- Set `required` to false for optional or out-of-scope work, and list every
  possible external side effect in `sideEffects`; those children stay in Backlog
  until the user explicitly approves them.
- Inspect the supplied codebase context and use real project architecture; do not invent endpoints or frameworks.
- Use only the parent ticket description, acceptance criteria, dependencies, and codebase context."#,
        id = parent.id,
        level = task_level(parent),
        title = parent.title,
        description = parent.description,
        acceptance = acceptance,
        next_level = next_level,
        breakdown_mode = breakdown_mode,
        children = if children.is_empty() {
            "None"
        } else {
            &children
        },
        direct_children = if direct_children.is_empty() {
            "None"
        } else {
            &direct_children
        },
        max_new_children = max_new_children,
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
    )
}

fn generated_child_requires_backlog_approval(task: &BoardTask) -> bool {
    if !task.hierarchy.required
        || !task.hierarchy.side_effects.is_empty()
        || task_requires_external_side_effect_declaration(task)
    {
        return true;
    }
    let text = task_external_effect_text(task);
    [
        "nice-to-have",
        "nice to have",
        "optional",
        "out of scope",
        "out-of-scope",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn generated_scope_wrapper_exists(run: &AgenticBoard, source_parent_id: &str, key: &str) -> bool {
    run.tasks.iter().any(|task| {
        task.task_origin == "hierarchy_backlog_wrapper"
            && task.hierarchy.discussion.iter().any(|entry| {
                entry.get("kind").and_then(Value::as_str) == Some("generated_scope_wrapper")
                    && entry.get("sourceParentId").and_then(Value::as_str) == Some(source_parent_id)
                    && entry.get("sourceTitleKey").and_then(Value::as_str) == Some(key)
            })
    })
}

fn generated_hierarchy_wrapper_exists(
    run: &AgenticBoard,
    source_parent_id: &str,
    wrapped_level: &str,
) -> bool {
    run.tasks.iter().any(|task| {
        task.task_origin == "hierarchy_backlog_wrapper"
            && task.hierarchy.discussion.iter().any(|entry| {
                entry.get("kind").and_then(Value::as_str) == Some("generated_scope_wrapper")
                    && entry.get("sourceParentId").and_then(Value::as_str) == Some(source_parent_id)
                    && entry.get("wrappedLevel").and_then(Value::as_str) == Some(wrapped_level)
            })
    })
}

fn mark_generated_scope_wrapper(
    task: &mut BoardTask,
    source_parent: &BoardTask,
    source_title_key: &str,
    wrapped_level: &str,
) {
    task.hierarchy.discussion.push(json!({
        "kind": "generated_scope_wrapper",
        "sourceParentId": source_parent.id,
        "sourceParentTitle": source_parent.title,
        "sourceTitleKey": source_title_key,
        "wrappedLevel": wrapped_level,
        "reason": "risky_or_out_of_scope",
        "createdAt": Utc::now(),
    }));
}

fn wrap_generated_child_in_backlog(
    run: &mut AgenticBoard,
    source_parent: &BoardTask,
    mut child: BoardTask,
    next_level: &str,
    source_title_key: &str,
) -> Vec<BoardTask> {
    let child_title = child.title.clone();
    let child_details = if child.details.trim().is_empty() {
        child.description.clone()
    } else {
        child.details.clone()
    };
    let source_reference = format!(
        "Generated from source parent {}: {}",
        source_parent.id, source_parent.title
    );
    let wrapper_details = format!(
        "Review and approve the generated {next_level} scope before execution.\n\n{child_details}"
    );
    let mut story = BoardTask::draft(
        run,
        format!("Proposed scope: {child_title}"),
        wrapper_details.clone(),
    );
    story.priority = child.priority.clone();
    story.status = TASK_STATUS_BACKLOG.to_string();
    story.task_type = TASK_KIND_DESIGN.to_string();
    story.task_origin = "hierarchy_backlog_wrapper".to_string();
    story.prompt = wrapper_details.clone();
    story.description = wrapper_details.clone();
    story.details = wrapper_details;
    story.acceptance_criteria = child.acceptance_criteria.clone();
    story.references = child.references.clone();
    story.references.push(source_reference.clone());
    story.hierarchy.level = TASK_LEVEL_STORY.to_string();
    story.hierarchy.parent_id = None;
    story.hierarchy.executable = false;
    story.hierarchy.required = child.hierarchy.required;
    story.hierarchy.scope_version = source_parent.hierarchy.scope_version.saturating_add(1);
    story.hierarchy.rank = child.hierarchy.rank;
    story.hierarchy.planned_files = child.hierarchy.planned_files.clone();
    story.hierarchy.side_effects = child.hierarchy.side_effects.clone();
    story.hierarchy.side_effects_approved = false;
    story.hierarchy.side_effect_approval = None;
    story.group_id = Some(story.id.clone());
    mark_generated_scope_wrapper(&mut story, source_parent, source_title_key, next_level);
    let story_id = story.id.clone();

    child.id = unique_task_id(run, &format!("{story_id}-{next_level}"));
    child.status = TASK_STATUS_BACKLOG.to_string();
    child.hierarchy.scope_version = source_parent.hierarchy.scope_version.saturating_add(1);
    child.hierarchy.blocked_by = dedupe_strings(child.depends_on.clone());
    child.depends_on = child.hierarchy.blocked_by.clone();
    child.hierarchy.required = story.hierarchy.required;
    child.hierarchy.side_effects_approved = false;
    child.hierarchy.side_effect_approval = None;
    child.hierarchy.side_effect_evidence.clear();
    child.references.push(source_reference);
    child.task_origin = "hierarchy_breakdown_wrapped".to_string();
    child.prompt = child.description.clone();
    child.group_id = Some(story_id.clone());
    mark_generated_scope_wrapper(&mut child, source_parent, source_title_key, next_level);

    if next_level == TASK_LEVEL_SUBTASK {
        let mut task_wrapper = BoardTask::draft(
            run,
            format!("Planned task: {child_title}"),
            format!(
                "Keep the generated subtask under this approved task scope.\n\n{child_details}"
            ),
        );
        task_wrapper.priority = child.priority.clone();
        task_wrapper.status = TASK_STATUS_BACKLOG.to_string();
        task_wrapper.task_type = TASK_KIND_DESIGN.to_string();
        task_wrapper.task_origin = "hierarchy_backlog_wrapper".to_string();
        task_wrapper.prompt = task_wrapper.description.clone();
        task_wrapper.hierarchy.level = TASK_LEVEL_TASK.to_string();
        task_wrapper.hierarchy.parent_id = Some(story_id.clone());
        task_wrapper.hierarchy.executable = false;
        task_wrapper.hierarchy.required = story.hierarchy.required;
        task_wrapper.hierarchy.scope_version = child.hierarchy.scope_version;
        task_wrapper.hierarchy.rank = child.hierarchy.rank;
        task_wrapper.hierarchy.planned_files = child.hierarchy.planned_files.clone();
        task_wrapper.hierarchy.side_effects = child.hierarchy.side_effects.clone();
        task_wrapper.hierarchy.side_effects_approved = false;
        task_wrapper.hierarchy.side_effect_approval = None;
        task_wrapper.acceptance_criteria = child.acceptance_criteria.clone();
        task_wrapper.references = child.references.clone();
        task_wrapper.group_id = Some(story_id.clone());
        mark_generated_scope_wrapper(
            &mut task_wrapper,
            source_parent,
            source_title_key,
            TASK_LEVEL_TASK,
        );
        child.hierarchy.parent_id = Some(task_wrapper.id.clone());
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.executable = true;
        return vec![story, task_wrapper, child];
    }

    child.hierarchy.parent_id = Some(story_id);
    child.hierarchy.level = next_level.to_string();
    child.hierarchy.executable = false;
    vec![story, child]
}

async fn plan_hierarchy_children(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    parent_id: &str,
    manual: bool,
) -> Result<usize> {
    let snapshot = load_user_board(state, user_id, board_id)?.board;
    let Some(parent) = snapshot
        .tasks
        .iter()
        .find(|task| task.id == parent_id)
        .cloned()
    else {
        return Err(not_found("Hierarchy parent not found"));
    };
    let Some(next_level) = next_hierarchy_level(task_level(&parent)) else {
        return Ok(0);
    };
    validate_hierarchy_breakdown_parent(&snapshot, &parent)?;
    let refinement = hierarchy_breakdown_has_children(&snapshot, &parent.id, next_level);
    let prompt = build_hierarchy_breakdown_prompt(&snapshot, &parent, next_level);
    let breakdown_started_at = Utc::now();
    let output = execute_internal_prompt(
        state,
        user_id,
        board_id,
        &format!("hierarchy breakdown for {}", parent.id),
        &prompt,
    )
    .await;
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            let failure = format!(
                "Hierarchy breakdown provider call failed: {}",
                server_error_message(&error)
            );
            record_hierarchy_breakdown_failure(
                state,
                user_id,
                board_id,
                &parent,
                &prompt,
                breakdown_started_at,
                &failure,
                manual,
            )?;
            return Err(ServerError::new(StatusCode::BAD_GATEWAY, failure));
        }
    };
    let output_json = match parse_json_object(&output) {
        Some(output_json) => output_json,
        None => {
            let failure = "Hierarchy breakdown returned malformed JSON instead of the required items contract.";
            record_hierarchy_breakdown_failure(
                state,
                user_id,
                board_id,
                &parent,
                &prompt,
                breakdown_started_at,
                failure,
                manual,
            )?;
            return Err(bad_request(failure));
        }
    };
    let source_items = output_json
        .get("items")
        .or_else(|| output_json.get("tasks"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let returned_empty_items = output_json
        .get("items")
        .or_else(|| output_json.get("tasks"))
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty);
    let children = source_items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let inherits_priority = item
                .get("priority")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_none_or(str::is_empty);
            let inherits_rank = item.get("rank").and_then(Value::as_i64).is_none();
            let mut child = task_from_json(&snapshot, item, index, TASK_STATUS_TODO)?;
            if inherits_priority {
                child.priority = parent.priority.clone();
            }
            if inherits_rank {
                child.hierarchy.rank = index as i64;
            }
            let needs_backlog_approval = generated_child_requires_backlog_approval(&child);
            Some((child, needs_backlog_approval))
        })
        .collect::<Vec<_>>();
    if children.is_empty() && (!refinement || !returned_empty_items) {
        let failure = if refinement {
            "Hierarchy breakdown refinement returned no usable child items."
        } else {
            "Hierarchy breakdown returned no usable child items."
        };
        record_hierarchy_breakdown_failure(
            state,
            user_id,
            board_id,
            &parent,
            &prompt,
            breakdown_started_at,
            failure,
            manual,
        )?;
        return Err(bad_request(failure));
    }

    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, board_id)?;
    let current_parent = stored
        .board
        .tasks
        .iter()
        .find(|task| task.id == parent.id)
        .cloned()
        .ok_or_else(|| not_found("Hierarchy parent no longer exists"))?;
    if current_parent.hierarchy.scope_version != parent.hierarchy.scope_version
        || canonical_task_status(&current_parent.status) != canonical_task_status(&parent.status)
        || current_parent.title != parent.title
        || current_parent.description != parent.description
        || current_parent.acceptance_criteria != parent.acceptance_criteria
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The parent scope changed while breakdown was running; discard this result and run Breakdown again.",
        ));
    }
    let child_status = match canonical_task_status(&current_parent.status) {
        TASK_STATUS_BACKLOG => TASK_STATUS_BACKLOG,
        TASK_STATUS_TODO => TASK_STATUS_TODO,
        _ => {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "Hierarchy breakdown requires the parent to remain in Backlog or Todo.",
            ));
        }
    };
    let provider = effective_provider_for_phase(&snapshot, "hierarchy breakdown")
        .unwrap_or_else(|_| snapshot.provider.clone());
    let model = effective_model_for_phase(&snapshot, "hierarchy breakdown");
    let root_group = stored
        .board
        .tasks
        .iter()
        .find(|task| task.id == parent.id)
        .map(task_group_id_or_self)
        .unwrap_or_else(|| parent.id.clone());
    let existing_titles =
        hierarchy_breakdown_child_title_candidates(&stored.board, &parent.id, next_level);
    let existing_child_count = existing_titles.len();
    let max_new_children = hierarchy_breakdown_max_new_children(existing_child_count);
    let mut seen_titles = existing_titles;
    let mut created = 0usize;
    let candidate_count = children.len();
    let mut reused = 0usize;
    for (mut child, needs_backlog_approval) in children {
        let key = normalize_suggested_task_key(&child.title);
        if created >= max_new_children
            || key.is_empty()
            || hierarchy_breakdown_title_is_duplicate(&seen_titles, &child.title)
        {
            reused += 1;
            continue;
        }
        // Keep one semantic title key for both ordinary and wrapped children.
        // A provider can return the same work with different wording or once
        // as a wrapped story and once as a normal child; neither is duplicated.
        seen_titles.push(child.title.clone());
        if needs_backlog_approval {
            let wrapped = wrap_generated_child_in_backlog(
                &mut stored.board,
                &current_parent,
                child,
                next_level,
                &key,
            );
            stored.board.tasks.extend(wrapped);
            created += 1;
            continue;
        }
        child.id = unique_task_id(&stored.board, &format!("{}-{}", parent.id, child.id));
        child.hierarchy.level = next_level.to_string();
        child.hierarchy.parent_id = Some(parent.id.clone());
        child.hierarchy.executable = next_level == TASK_LEVEL_SUBTASK;
        child.hierarchy.scope_version = parent.hierarchy.scope_version.saturating_add(1);
        child.hierarchy.blocked_by = dedupe_strings(child.depends_on.clone());
        child.depends_on = child.hierarchy.blocked_by.clone();
        child.group_id = Some(root_group.clone());
        child.status = if child_status == TASK_STATUS_TODO && needs_backlog_approval {
            TASK_STATUS_BACKLOG.to_string()
        } else {
            child_status.to_string()
        };
        child.task_origin = "hierarchy_breakdown".to_string();
        child.prompt = parent.description.clone();
        stored.board.tasks.push(child);
        created += 1;
    }
    if created == 0 {
        if refinement || (candidate_count > 0 && reused == candidate_count) {
            let summary = if existing_child_count >= MAX_HIERARCHY_CHILDREN_PER_PARENT {
                format!(
                    "Breakdown checked; child limit reached ({MAX_HIERARCHY_CHILDREN_PER_PARENT})"
                )
            } else {
                format!("Breakdown checked; no new {next_level} child ticket(s) needed")
            };
            if let Some(parent_task) = stored
                .board
                .tasks
                .iter_mut()
                .find(|task| task.id == parent.id)
            {
                parent_task.error = None;
                parent_task.summary = summary.clone();
                append_hierarchy_breakdown_transcript(
                    parent_task,
                    breakdown_started_at,
                    &provider,
                    &model,
                    &prompt,
                    &output,
                );
            }
            stored.board.append_log(format!(
                "Hierarchy breakdown checked {parent_id}; no new {next_level} child ticket(s) added"
            ));
            refresh_hierarchy_rollups(&mut stored.board);
            stored.board.touch();
            save_board(state, &stored.board)?;
            return Ok(0);
        }
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!("No new {next_level} children could be created for {parent_id}"),
        ));
    }
    if let Some(issue) = hierarchy_validation_issues(&stored.board)
        .into_iter()
        .next()
    {
        let affected = planning_error_task_ids(&stored.board, &issue);
        let error = planning_error_conflict(&mut stored.board, &affected, "hierarchy", issue);
        stored.board.touch();
        save_board(state, &stored.board)?;
        return Err(error);
    }
    if let Some(parent_task) = stored
        .board
        .tasks
        .iter_mut()
        .find(|task| task.id == parent.id)
    {
        parent_task.error = None;
        parent_task.summary = if refinement {
            format!("Breakdown refined; added {created} {next_level} child ticket(s)")
        } else {
            format!("Generated {created} {next_level} child ticket(s)")
        };
    }
    if let Some(cycle) = dependency_cycle(&stored.board) {
        let issue = format!("Dependency cycle detected: {}", cycle.join(" -> "));
        let error = planning_error_conflict(&mut stored.board, &cycle, "dependency", issue);
        stored.board.touch();
        save_board(state, &stored.board)?;
        return Err(error);
    }
    if refinement {
        stored.board.append_log(format!(
            "Hierarchy breakdown refined {parent_id}; added {created} {next_level} child ticket(s)"
        ));
    } else {
        stored.board.append_log(format!(
            "Hierarchy breakdown created {created} {next_level} child ticket(s) under {parent_id}"
        ));
    }
    if let Some(parent_task) = stored
        .board
        .tasks
        .iter_mut()
        .find(|task| task.id == parent.id)
    {
        append_hierarchy_breakdown_transcript(
            parent_task,
            breakdown_started_at,
            &provider,
            &model,
            &prompt,
            &output,
        );
    }
    refresh_hierarchy_rollups(&mut stored.board);
    stored.board.touch();
    save_board(state, &stored.board)?;
    Ok(created)
}

fn append_hierarchy_breakdown_transcript(
    task: &mut BoardTask,
    started_at: DateTime<Utc>,
    provider: &str,
    model: &str,
    prompt: &str,
    output: &str,
) {
    task.transcript.push(redact_transcript_value(&json!({
        "timestamp": started_at,
        "role": "user",
        "kind": "user",
        "provider": provider,
        "model": model,
        "content": prompt,
    })));
    task.transcript.push(redact_transcript_value(&json!({
        "timestamp": Utc::now(),
        "role": "assistant",
        "kind": "assistant",
        "provider": provider,
        "model": model,
        "content": output,
    })));
    task.transcript_updated_at = Some(Utc::now());
}

fn record_hierarchy_breakdown_failure(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    parent: &BoardTask,
    prompt: &str,
    started_at: DateTime<Utc>,
    failure: &str,
    manual: bool,
) -> Result<()> {
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, board_id)?;
    // A breakdown is a user-invoked planning operation and commonly runs
    // while the board is paused. Persist its failure even in that state so a
    // provider/contract error cannot leave an apparently healthy planning
    // item with no retry or attention signal. Terminal boards are immutable.
    if matches!(stored.board.status.as_str(), "cancelled" | "completed") {
        return Ok(());
    }
    let provider = effective_provider_for_phase(&stored.board, "hierarchy breakdown")
        .unwrap_or_else(|_| stored.board.provider.clone());
    let model = effective_model_for_phase(&stored.board, "hierarchy breakdown");
    if let Some(task) = stored
        .board
        .tasks
        .iter_mut()
        .find(|task| task.id == parent.id)
    {
        // Manual Breakdown is a planning action. A provider/contract
        // failure must remain retryable and must not masquerade as a real
        // dependency blocker. Automatic hierarchy planning retains the
        // existing fail-closed behavior so the worker cannot spin forever.
        if !manual {
            task.status = TASK_STATUS_BLOCKED.to_string();
        }
        task.error = Some(failure.to_string());
        task.summary = failure.to_string();
        task.transcript.push(redact_transcript_value(&json!({
            "timestamp": started_at,
            "role": "user",
            "kind": "user",
            "provider": provider.clone(),
            "model": model.clone(),
            "content": prompt,
        })));
        task.transcript.push(redact_transcript_value(&json!({
            "timestamp": Utc::now(),
            "role": "system",
            "kind": "error",
            "provider": provider,
            "model": model,
            "content": failure,
        })));
        task.transcript_updated_at = Some(Utc::now());
    }
    if manual {
        stored.board.append_log(format!(
            "Manual hierarchy breakdown failed for {}; item remains retryable: {}",
            parent.id, failure
        ));
    } else {
        mark_planning_error(
            &mut stored.board,
            std::slice::from_ref(&parent.id),
            "hierarchy",
            failure,
        );
        if let Some(details) = stored
            .board
            .phase_details
            .as_mut()
            .and_then(Value::as_object_mut)
        {
            details.insert("parentId".to_string(), json!(parent.id));
            details.insert(
                "retry".to_string(),
                json!("Move the planning item to Todo and run Breakdown again after the provider or contract issue is fixed."),
            );
            details.insert("startedAt".to_string(), json!(started_at));
        }
    }
    stored.board.touch();
    save_board(state, &stored.board)
}

fn is_retryable_hierarchy_breakdown_message(message: &str) -> bool {
    message
        .trim()
        .strip_prefix("Planning error: ")
        .unwrap_or_else(|| message.trim())
        .starts_with("Hierarchy breakdown ")
}

fn is_retryable_hierarchy_breakdown_task(task: &BoardTask) -> bool {
    task.error
        .as_deref()
        .is_some_and(is_retryable_hierarchy_breakdown_message)
        || is_retryable_hierarchy_breakdown_message(&task.summary)
}

fn hierarchy_breakdown_planning_error_for(run: &AgenticBoard, task_id: &str) -> bool {
    run.current_phase.as_deref() == Some(PLANNING_ERROR_PHASE)
        && run
            .phase_details
            .as_ref()
            .and_then(|details| details.get("kind"))
            .and_then(Value::as_str)
            == Some("hierarchy")
        && run
            .phase_details
            .as_ref()
            .and_then(|details| details.get("parentId"))
            .and_then(Value::as_str)
            == Some(task_id)
        && run
            .phase_details
            .as_ref()
            .and_then(|details| details.get("error"))
            .and_then(Value::as_str)
            .is_some_and(is_retryable_hierarchy_breakdown_message)
}

fn restore_board_after_hierarchy_breakdown_failure(run: &mut AgenticBoard, task_id: &str) -> bool {
    if !hierarchy_breakdown_planning_error_for(run, task_id) {
        return false;
    }
    run.status = "paused".to_string();
    run.active = false;
    run.loop_started = false;
    run.auto_run_enabled = false;
    run.pause_requested = false;
    run.paused_at = Some(Utc::now());
    run.current_task_id = None;
    run.current_task_title.clear();
    run.current_task_status.clear();
    run.current_phase = Some("board".to_string());
    run.phase_started_at = Some(Utc::now());
    run.phase_details = Some(json!({
        "mode": "kanban_only",
        "breakdownRetryable": true,
        "parentId": task_id,
    }));
    run.pause_reason = Some("Manual hierarchy breakdown is available.".to_string());
    run.append_log(format!(
        "Restored board to paused planning state after hierarchy breakdown failure for {task_id}"
    ));
    true
}

fn refresh_hierarchy_rollups(run: &mut AgenticBoard) {
    let mut parent_ids = run
        .tasks
        .iter()
        .filter(|task| !task_is_executable(task))
        .map(|task| {
            let mut depth = 0usize;
            let mut current = task.hierarchy.parent_id.as_deref();
            let mut seen = BTreeSet::new();
            while let Some(parent_id) = current {
                if !seen.insert(parent_id) {
                    break;
                }
                depth = depth.saturating_add(1);
                current = run
                    .tasks
                    .iter()
                    .find(|candidate| candidate.id == parent_id)
                    .and_then(|parent| parent.hierarchy.parent_id.as_deref());
            }
            (task.id.clone(), depth)
        })
        .collect::<Vec<_>>();
    parent_ids.sort_by_key(|(_, depth)| std::cmp::Reverse(*depth));
    for (parent_id, _) in parent_ids {
        let children = run
            .tasks
            .iter()
            .filter(|task| task.hierarchy.parent_id.as_deref() == Some(parent_id.as_str()))
            .collect::<Vec<_>>();
        if children.is_empty() {
            continue;
        }
        let required_children = children
            .iter()
            .copied()
            .filter(|task| task.hierarchy.required)
            .collect::<Vec<_>>();
        let optional_only = required_children.is_empty();
        // Optional children never block a parent that has required work. If a
        // parent has only optional children, still derive a useful status from
        // them so a completed optional-only plan does not leave its parent
        // indefinitely in Todo. The optional work remains independently
        // approvable and executable.
        let relevant_children = if optional_only {
            children.clone()
        } else {
            required_children.clone()
        };
        let all_done = relevant_children
            .iter()
            .all(|task| task_rollup_completion_is_satisfied(task));
        let has_eligible_child = relevant_children
            .iter()
            .any(|task| task_rollup_child_is_eligible(run, task));
        let has_active_child = relevant_children.iter().any(|task| {
            task_status_is_active(&task.status) && task_ancestors_are_approved(run, task)
        });
        let all_remaining_blocked = relevant_children
            .iter()
            .filter(|task| !task_rollup_completion_is_satisfied(task))
            .all(|task| task_rollup_child_is_blocked(run, task));
        let all_backlog = relevant_children
            .iter()
            .all(|task| task_status_is_backlog(&task.status));
        let parent = run.tasks.iter().find(|task| task.id == parent_id);
        let parent_was_approved =
            parent.is_some_and(|parent| !task_status_is_backlog(&parent.status));
        let parent_was_done = parent.is_some_and(|parent| task_status_is_done(&parent.status));
        let parent_attention_status = parent
            .filter(|parent| {
                matches!(
                    canonical_task_status(&parent.status),
                    TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
                ) && parent
                    .error
                    .as_deref()
                    .is_some_and(|error| !error.trim().is_empty())
            })
            .map(|parent| canonical_task_status(&parent.status));
        let next_status = if optional_only && parent_was_done {
            // Optional work is independently approvable and must not silently
            // reopen a completed required scope when the user later moves the
            // nice-to-have child to Todo.
            TASK_STATUS_DONE
        } else if all_done {
            TASK_STATUS_DONE
        } else if has_active_child || has_eligible_child {
            TASK_STATUS_IN_PROGRESS
        } else if !optional_only && all_remaining_blocked {
            TASK_STATUS_BLOCKED
        } else if all_backlog {
            // A parent moved to Todo is already an approval decision. Keep
            // that approval while its manually authored children are still
            // waiting in Backlog; otherwise a rollup would silently revoke
            // the user's approval.
            if let Some(status) = parent_attention_status {
                status
            } else if optional_only && parent_was_approved {
                // No required path remains. A backlog nice-to-have must not
                // keep an approved parent open or turn into implicit work.
                TASK_STATUS_DONE
            } else if parent_was_approved {
                TASK_STATUS_TODO
            } else {
                TASK_STATUS_BACKLOG
            }
        } else if optional_only {
            // Optional work is not a required path. If it is neither ready
            // nor running, the parent can still be complete; any blocked or
            // failed optional item remains visible in its own group/task.
            parent_attention_status.unwrap_or(TASK_STATUS_DONE)
        } else {
            TASK_STATUS_TODO
        };
        if let Some(parent) = run.tasks.iter_mut().find(|task| task.id == parent_id) {
            parent.status = next_status.to_string();
            if matches!(next_status, TASK_STATUS_IN_PROGRESS | TASK_STATUS_TODO)
                && parent
                    .error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("Planning error:"))
            {
                parent.error = None;
            } else if next_status == TASK_STATUS_DONE {
                parent.error = None;
            }
        }
    }
}

fn task_rollup_completion_is_satisfied(task: &BoardTask) -> bool {
    task_status_is_done(&task.status)
        && !(canonical_task_kind(task) == TASK_KIND_RESEARCH && !task.hierarchy.research_accepted)
}

fn task_rollup_child_is_eligible(run: &AgenticBoard, task: &BoardTask) -> bool {
    if !task_status_is_todo(&task.status) || !task_ancestors_are_approved(run, task) {
        return false;
    }
    if !unmet_task_dependencies(run, task).is_empty() {
        return false;
    }
    if task_is_executable(task) {
        task_side_effects_are_approved(task)
            && (!has_pending_research_acceptance(run)
                || canonical_task_kind(task) == TASK_KIND_RESEARCH)
    } else {
        true
    }
}

fn task_rollup_child_is_blocked(run: &AgenticBoard, task: &BoardTask) -> bool {
    match canonical_task_status(&task.status) {
        TASK_STATUS_BLOCKED | TASK_STATUS_FAILED => true,
        TASK_STATUS_DONE => {
            canonical_task_kind(task) == TASK_KIND_RESEARCH && !task.hierarchy.research_accepted
        }
        TASK_STATUS_TODO => !task_rollup_child_is_eligible(run, task),
        _ => false,
    }
}

fn normalize_suggested_task_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn suggested_task_title_tokens(value: &str) -> BTreeSet<String> {
    let normalized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>();

    normalized
        .split_whitespace()
        .filter(|token| {
            !matches!(
                *token,
                "a" | "an"
                    | "and"
                    | "as"
                    | "at"
                    | "by"
                    | "for"
                    | "from"
                    | "in"
                    | "into"
                    | "of"
                    | "on"
                    | "or"
                    | "the"
                    | "to"
                    | "under"
                    | "using"
                    | "via"
                    | "with"
            )
        })
        .map(canonical_suggested_task_token)
        .collect()
}

fn canonical_suggested_task_token(token: &str) -> String {
    let canonical = match token {
        "add" | "adds" | "added" | "create" | "creates" | "created" | "implement"
        | "implements" | "implemented" | "implementation" => "implement",
        "persist" | "persists" | "persisted" | "save" | "saves" | "saved" | "storage" | "store"
        | "stores" | "stored" => "persist",
        "validate" | "validates" | "validated" | "validation" | "validating" => "validate",
        "test" | "tests" | "tested" | "testing" => "test",
        "execute" | "executes" | "executed" | "execution" | "run" | "runs" | "running" => "run",
        "build" | "builds" | "built" | "building" => "build",
        "configure" | "configures" | "configured" | "configuration" => "configure",
        "update" | "updates" | "updated" | "updating" => "update",
        "remove" | "removes" | "removed" | "removing" => "remove",
        "delete" | "deletes" | "deleted" | "deleting" => "delete",
        "fix" | "fixes" | "fixed" | "fixing" => "fix",
        "task" | "tasks" => "task",
        "file" | "files" => "file",
        _ => token,
    };
    canonical.to_string()
}

fn suggested_task_titles_are_semantic_duplicates(left: &str, right: &str) -> bool {
    let left_key = normalize_suggested_task_key(left);
    let right_key = normalize_suggested_task_key(right);
    if left_key.is_empty() || right_key.is_empty() {
        return false;
    }
    if left_key == right_key {
        return true;
    }

    let left_tokens = suggested_task_title_tokens(left);
    let right_tokens = suggested_task_title_tokens(right);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return false;
    }

    let overlap = left_tokens.intersection(&right_tokens).count();
    let smaller = left_tokens.len().min(right_tokens.len());
    let union = left_tokens.union(&right_tokens).count();
    overlap >= 3 && overlap * 100 >= smaller * 80 && overlap * 100 >= union * 60
}

fn hierarchy_breakdown_title_is_duplicate(existing_titles: &[String], candidate: &str) -> bool {
    existing_titles
        .iter()
        .any(|existing| suggested_task_titles_are_semantic_duplicates(existing, candidate))
}

fn append_suggested_backlog_tasks_from_result(
    run: &mut AgenticBoard,
    source_task_id: &str,
    parsed: &Value,
) -> Vec<String> {
    let suggestions = parsed
        .get("suggestedBacklogTasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if suggestions.is_empty() {
        return Vec::new();
    }
    let Some(source_task) = run
        .tasks
        .iter()
        .find(|task| task.id == source_task_id)
        .cloned()
    else {
        return Vec::new();
    };
    if canonical_task_kind(&source_task) == TASK_KIND_RESEARCH {
        return Vec::new();
    }
    let mut existing_scope_keys = run
        .tasks
        .iter()
        .filter_map(|task| {
            let title_key = normalize_suggested_task_key(&task.title);
            if title_key.is_empty() {
                return None;
            }
            let parent_id = task_parent_id(task).unwrap_or_default();
            let level = task_level(task);
            if parent_id.is_empty()
                && !matches!(
                    level,
                    TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC | TASK_LEVEL_STORY
                )
            {
                return None;
            }
            Some(suggested_scope_key(parent_id, level, &title_key))
        })
        .collect::<BTreeSet<_>>();
    let mut created = Vec::new();
    for suggestion in suggestions {
        let requested_level = normalize_task_level(
            suggestion.get("level").and_then(Value::as_str),
            TASK_LEVEL_STORY,
        );
        let requested_parent_id = suggestion
            .get("parentId")
            .or_else(|| suggestion.get("parent_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_default();
        let title = ["title", "details", "description", "summary"]
            .into_iter()
            .filter_map(|key| suggestion.get(key).and_then(Value::as_str))
            .map(str::trim)
            .find(|value| !value.is_empty())
            .map(str::to_string);
        let Some(title) = title else {
            continue;
        };
        let key = normalize_suggested_task_key(&title);
        let target_level = requested_parent_id
            .is_empty()
            .then_some(match requested_level {
                TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK => TASK_LEVEL_STORY,
                level => level,
            })
            .or_else(|| {
                run.tasks
                    .iter()
                    .find(|task| task.id == requested_parent_id)
                    .and_then(|parent| next_hierarchy_level(task_level(parent)))
            })
            .unwrap_or(requested_level);
        let scope_key = suggested_scope_key(requested_parent_id, target_level, &key);
        if key.is_empty() || !existing_scope_keys.insert(scope_key) {
            continue;
        }
        let details = suggestion
            .get("details")
            .or_else(|| suggestion.get("description"))
            .or_else(|| suggestion.get("summary"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(&title)
            .to_string();
        let mut task = BoardTask::manual(
            run,
            TaskRequest {
                prompt: Some(details.clone()),
                command: None,
                title: Some(title),
                details: Some(details),
                description: None,
                kind: suggestion
                    .get("kind")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                task_type: suggestion
                    .get("taskType")
                    .or_else(|| suggestion.get("task_type"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                level: suggestion
                    .get("level")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                parent_id: suggestion
                    .get("parentId")
                    .or_else(|| suggestion.get("parent_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                blocked_by: suggestion
                    .get("blockedBy")
                    .or_else(|| suggestion.get("blocked_by"))
                    .cloned(),
                executable: suggestion.get("executable").and_then(Value::as_bool),
                required: suggestion.get("required").and_then(Value::as_bool),
                source_task_id: suggestion
                    .get("sourceTaskId")
                    .or_else(|| suggestion.get("source_task_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                scope_version: suggestion.get("scopeVersion").and_then(Value::as_u64),
                planned_files: suggestion
                    .get("plannedFiles")
                    .or_else(|| suggestion.get("files"))
                    .cloned(),
                side_effects: suggestion.get("sideEffects").cloned(),
                acceptance_criteria: suggestion.get("acceptanceCriteria").cloned(),
                acceptance: suggestion.get("acceptance").cloned(),
                criteria: suggestion.get("criteria").cloned(),
                references: suggestion.get("references").cloned(),
                files: suggestion.get("files").cloned(),
                paths: suggestion.get("paths").cloned(),
                priority: suggestion
                    .get("priority")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                rank: suggestion.get("rank").and_then(Value::as_i64),
                depends_on: suggestion.get("dependsOn").cloned(),
                dependencies: suggestion.get("dependencies").cloned(),
                status: Some("backlog".to_string()),
            },
        )
        .expect("suggested backlog task has a title");
        task.references.insert(
            0,
            format!(
                "Suggested backlog task from {}: {}",
                source_task.id, source_task.title
            ),
        );
        task.task_origin = "ai_suggested_backlog".to_string();
        task.manual_task = false;
        task.prompt_task = false;
        task.task_type = prompt_task_kind_from_value(
            &suggestion,
            &task.title,
            &task.details,
            &task.acceptance_criteria,
        )
        .to_string();
        // BoardTask::manual intentionally normalizes a top-level task/subtask
        // to a visible story. Keep the model's original level here so a
        // result suggestion cannot accidentally bypass the required story
        // wrapper before it reaches the Backlog.
        let generated_level = if task.hierarchy.parent_id.is_none()
            && matches!(requested_level, TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK)
        {
            requested_level.to_string()
        } else {
            task_level(&task).to_string()
        };
        let needs_backlog_wrapper = generated_child_requires_backlog_approval(&task)
            || (task.hierarchy.parent_id.is_none()
                && matches!(
                    generated_level.as_str(),
                    TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK
                ));
        if needs_backlog_wrapper
            && matches!(
                generated_level.as_str(),
                TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK
            )
        {
            let source_title_key = normalize_suggested_task_key(&task.title);
            let wrapped = wrap_generated_child_in_backlog(
                run,
                &source_task,
                task,
                &generated_level,
                &source_title_key,
            );
            let wrapper_id = wrapped.first().map(|task| task.id.clone());
            run.tasks.extend(wrapped);
            if let Some(wrapper_id) = wrapper_id {
                created.push(wrapper_id);
            }
            continue;
        }
        let task_id = task.id.clone();
        run.tasks.push(task);
        created.push(task_id);
    }
    if !created.is_empty() {
        run.append_log(format!(
            "Added {} suggested backlog task(s) from {}: {}",
            created.len(),
            source_task_id,
            created.join(", ")
        ));
    }
    created
}

fn suggested_scope_key(parent_id: &str, level: &str, title_key: &str) -> String {
    format!("{}|{}|{}", parent_id, level, title_key)
}

fn is_user_authored_task(task: &BoardTask) -> bool {
    task.manual_task
        || task.prompt_task
        || matches!(
            task.task_origin.as_str(),
            "user_manual"
                | "user_prompt_generated"
                | "ai_suggested_backlog"
                | "manual"
                | "prompt_breakdown"
        )
}

fn unique_task_id(run: &AgenticBoard, base: &str) -> String {
    let existing = run
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<BTreeSet<_>>();
    if !existing.contains(base) {
        return base.to_string();
    }
    for index in 2..1000 {
        let candidate = format!("{base}-{index}");
        if !existing.contains(candidate.as_str()) {
            return candidate;
        }
    }
    format!("{base}-{}", Uuid::new_v4())
}

fn has_runnable_tasks(run: &AgenticBoard) -> bool {
    run.tasks
        .iter()
        .any(|task| task_is_runnable_in_board(run, task))
}

fn has_only_seed_prompt_task(run: &AgenticBoard) -> bool {
    run.tasks.len() == 1
        && run.tasks[0].id == "task-1"
        && run.tasks[0].prompt.trim() == run.source_prompt.trim()
        && (task_is_runnable_in_board(run, &run.tasks[0])
            || run.tasks[0].status == TASK_STATUS_BACKLOG)
}

fn executable_system_subtask_hierarchy(
    parent_id: Option<String>,
    blocked_by: Vec<String>,
    scope_version: u64,
    rank: i64,
    planned_files: Vec<String>,
) -> BoardTaskHierarchy {
    BoardTaskHierarchy {
        level: TASK_LEVEL_SUBTASK.to_string(),
        parent_id,
        blocked_by: dedupe_strings(blocked_by),
        executable: true,
        required: true,
        scope_version: scope_version.max(1),
        rank,
        attempts: Vec::new(),
        planned_files,
        side_effects: Vec::new(),
        side_effects_approved: false,
        side_effect_approval: None,
        side_effect_evidence: Vec::new(),
        manual_test_environment: None,
        research_accepted: false,
        research_acceptance: None,
        discussion: Vec::new(),
    }
}

fn system_sibling_parent_id(source_task: &BoardTask) -> Option<String> {
    (task_level(source_task) == TASK_LEVEL_SUBTASK)
        .then(|| source_task.hierarchy.parent_id.clone())
        .flatten()
}

fn append_final_qa_task(run: &mut AgenticBoard, reason: &str) -> bool {
    if run
        .tasks
        .iter()
        .any(|task| task.final_qa_task && task_is_runnable(task))
    {
        return false;
    }
    if run
        .tasks
        .iter()
        .any(|task| task.final_qa_task && task_is_done(task) && task.qa_passed == Some(true))
    {
        return false;
    }
    let round = run.tasks.iter().filter(|task| task.final_qa_task).count() as u32 + 1;
    let id = if round == 1 {
        FINAL_QA_TASK_ID.to_string()
    } else {
        format!("{FINAL_QA_TASK_ID}-{round}")
    };
    run.tasks.push(BoardTask {
        id,
        title: "Independent final validation".to_string(),
        status: TASK_STATUS_TODO.to_string(),
        summary: String::new(),
        details: format!(
            "Validate the ticket scope against current files, deterministic command evidence, and completed task results. Reason: {reason}"
        ),
        description: "Independent final validation".to_string(),
        prompt: "Run final QA validation and return the required JSON verdict.".to_string(),
        error: None,
        acceptance_criteria: vec![
            "Independently validate the ticket description and acceptance criteria against current files and deterministic command evidence.".to_string(),
            "Inspect implementation directly; do not trust feature summaries as proof.".to_string(),
            "Return done only when the ticket scope has concrete evidence and deterministic checks pass.".to_string(),
            "Do not edit files during this validation task and do not modify git history.".to_string(),
        ],
        references: vec![
            "Original user prompt".to_string(),
            "Changed files and local verification output".to_string(),
        ],
        priority: TASK_PRIORITY_P3.to_string(),
        depends_on: Vec::new(),
        manual_task: false,
        prompt_task: false,
        task_origin: "system_final_qa".to_string(),
        task_type: TASK_KIND_QA.to_string(),
        backlog_generation_task: false,
        qa_task: true,
        final_qa_task: true,
        followup_task: false,
        qa_fix_task: false,
        qa_verdict_retry_task: false,
        task_level_qa: false,
        agents_knowledge_task: false,
        internal_validation: true,
        qa_round: round,
        source_task_id: None,
        source_qa_task_id: None,
        superseded_by: None,
        transcript: Vec::new(),
        transcript_updated_at: None,
        started_at: None,
        completed_at: None,
        qa_passed: None,
        attempt_count: 0,
        provider_session_id: None,
        commands_run: Vec::new(),
        changed_files: Vec::new(),
        changed_file_summary: None,
        evidence: Vec::new(),
        remaining_issues: Vec::new(),
        result: None,
        result_validation: None,
        deterministic_validation: None,
        rag_context_refs: Vec::new(),
        rag_prompt_context: String::new(),
        tdd_phase: "final".to_string(),
        qa_test_paths: Vec::new(),
        qa_test_commands: Vec::new(),
        qa_baseline_validation: None,
        fix_attempts: 0,
        coverage_evidence: Vec::new(),
        group_id: Some(FINAL_QA_TASK_ID.to_string()),
        hierarchy: executable_system_subtask_hierarchy(None, Vec::new(), 1, 0, Vec::new()),
    });
    true
}

fn append_promotion_review_task(run: &mut AgenticBoard, reason: &str) -> bool {
    if run.promotion_candidates.is_empty() {
        return false;
    }
    if run.tasks.iter().any(|task| is_promotion_review_task(task)) {
        return false;
    }
    run.tasks.push(BoardTask {
        id: PROMOTION_REVIEW_TASK_ID.to_string(),
        title: "Review RAG promotion candidates".to_string(),
        status: TASK_STATUS_TODO.to_string(),
        summary: String::new(),
        details: format!(
            "Review validated project-specific RAG memories and approve only reusable, safe global standards. Reason: {reason}"
        ),
        description: "Review RAG promotion candidates".to_string(),
        prompt: "Review promotion candidates and return approvedCandidateIds JSON.".to_string(),
        error: None,
        acceptance_criteria: vec![
            "Reject unsafe, overly project-specific, secret-bearing, or speculative patterns."
                .to_string(),
            "Approve only reusable implementation, testing, or validation standards.".to_string(),
            "Do not edit files or modify git history.".to_string(),
        ],
        references: vec!["RAG project-specific promotion candidates".to_string()],
        priority: TASK_PRIORITY_P3.to_string(),
        depends_on: Vec::new(),
        manual_task: false,
        prompt_task: false,
        task_origin: "system_promotion".to_string(),
        task_type: TASK_KIND_REVIEW.to_string(),
        backlog_generation_task: false,
        qa_task: false,
        final_qa_task: false,
        followup_task: false,
        qa_fix_task: false,
        qa_verdict_retry_task: false,
        task_level_qa: false,
        agents_knowledge_task: false,
        internal_validation: true,
        qa_round: 0,
        source_task_id: None,
        source_qa_task_id: None,
        superseded_by: None,
        transcript: Vec::new(),
        transcript_updated_at: None,
        started_at: None,
        completed_at: None,
        qa_passed: None,
        attempt_count: 0,
        provider_session_id: None,
        commands_run: Vec::new(),
        changed_files: Vec::new(),
        changed_file_summary: None,
        evidence: Vec::new(),
        remaining_issues: Vec::new(),
        result: None,
        result_validation: None,
        deterministic_validation: None,
        rag_context_refs: Vec::new(),
        rag_prompt_context: String::new(),
        tdd_phase: "promotion_review".to_string(),
        qa_test_paths: Vec::new(),
        qa_test_commands: Vec::new(),
        qa_baseline_validation: None,
        fix_attempts: 0,
        coverage_evidence: Vec::new(),
        group_id: Some(PROMOTION_REVIEW_TASK_ID.to_string()),
        hierarchy: executable_system_subtask_hierarchy(None, Vec::new(), 1, 0, Vec::new()),
    });
    true
}

fn is_promotion_review_task(task: &BoardTask) -> bool {
    matches!(task.task_type.as_str(), "promotion" | TASK_KIND_REVIEW)
        || task.id == PROMOTION_REVIEW_TASK_ID
}

fn append_agents_knowledge_task(
    run: &mut AgenticBoard,
    reason: &str,
    source_task: Option<&BoardTask>,
) -> bool {
    if run.agents_knowledge_updated {
        return false;
    }
    let source_task_id = source_task.map(|task| task.id.as_str()).unwrap_or("");
    if run.tasks.iter().any(|task| {
        (task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID)
            && (source_task_id.is_empty() || task.source_task_id.as_deref() == Some(source_task_id))
    }) {
        return false;
    }
    if source_task_id.is_empty() {
        let has_completed_implementation = run
            .tasks
            .iter()
            .any(|task| task_is_done(task) && !is_qa_task(task) && !task.agents_knowledge_task);
        if !has_completed_implementation {
            return false;
        }
    }
    let task = create_agents_knowledge_task(run, reason, source_task);
    if let Some(source_task) = source_task {
        let insert_index = run
            .tasks
            .iter()
            .position(|task| task.id == source_task.id)
            .map(|index| index + 1)
            .unwrap_or(run.tasks.len());
        run.tasks.insert(insert_index, task);
    } else {
        run.tasks.push(task);
    }
    true
}

fn has_task_qa_for_source(run: &AgenticBoard, source_task_id: &str) -> bool {
    run.tasks
        .iter()
        .any(|task| task.task_level_qa && task.source_task_id.as_deref() == Some(source_task_id))
}

fn append_task_qa_task(run: &mut AgenticBoard, source_task: &BoardTask, reason: &str) -> bool {
    let task = create_task_qa_task(run, source_task, reason);
    let insert_index = run
        .tasks
        .iter()
        .position(|task| task.id == source_task.id)
        .map(|index| index + 1)
        .unwrap_or(run.tasks.len());
    run.tasks.insert(insert_index, task);
    true
}

fn create_task_qa_task(run: &AgenticBoard, source_task: &BoardTask, reason: &str) -> BoardTask {
    let title_seed = if source_task.title.trim().is_empty() {
        limit_text(&active_board_prompt(run), 180)
    } else {
        source_task.title.clone()
    };
    let details = [
        format!("Validate source task: {} - {}", source_task.id, source_task.title),
        "Inspect the current implementation before marking the task validated.".to_string(),
        "Cover the relevant happy path, failure path, and corner cases for the source task.".to_string(),
        "If locally actionable issues are found, return needs_followup with exact findings so the board can queue a fix.".to_string(),
        "Do not edit files during this validation task and do not modify git history.".to_string(),
        if reason.trim().is_empty() {
            String::new()
        } else {
            format!("Task QA reason: {reason}")
        },
    ]
    .into_iter()
    .filter(|item| !item.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n");
    let hierarchy = executable_system_subtask_hierarchy(
        system_sibling_parent_id(source_task),
        vec![source_task.id.clone()],
        source_task.hierarchy.scope_version,
        source_task.hierarchy.rank.saturating_add(1),
        source_task.hierarchy.planned_files.clone(),
    );
    BoardTask {
        id: unique_task_id(run, "task-qa"),
        title: format!("QA validate {}", limit_text(&title_seed, 120).replace('\n', " ")),
        status: TASK_STATUS_TODO.to_string(),
        summary: String::new(),
        details: details.clone(),
        description: details.clone(),
        prompt: details,
        error: None,
        acceptance_criteria: vec![
            "Validate the source task against its acceptance criteria, source references, evidence, and changed files.".to_string(),
            "Return done only when the source task is validated.".to_string(),
            "Return needs_followup with exact findings when defects are found.".to_string(),
        ],
        references: vec![
            format!("Source task: {}", source_task.id),
            format!("Source task title: {}", source_task.title),
            if source_task.summary.trim().is_empty() {
                String::new()
            } else {
                format!("Source task summary: {}", source_task.summary)
            },
        ]
        .into_iter()
        .filter(|item| !item.trim().is_empty())
        .collect(),
        priority: source_task.priority.clone(),
        depends_on: vec![source_task.id.clone()],
        manual_task: false,
        prompt_task: false,
        task_origin: "system_qa".to_string(),
        task_type: TASK_KIND_QA.to_string(),
        backlog_generation_task: false,
        qa_task: true,
        final_qa_task: false,
        followup_task: false,
        qa_fix_task: false,
        qa_verdict_retry_task: false,
        task_level_qa: true,
        agents_knowledge_task: false,
        internal_validation: false,
        qa_round: 0,
        source_task_id: Some(source_task.id.clone()),
        source_qa_task_id: None,
        superseded_by: None,
        transcript: Vec::new(),
        transcript_updated_at: None,
        started_at: None,
        completed_at: None,
        qa_passed: None,
        attempt_count: 0,
        provider_session_id: None,
        commands_run: Vec::new(),
        changed_files: Vec::new(),
        changed_file_summary: None,
        evidence: Vec::new(),
        remaining_issues: Vec::new(),
        result: None,
        result_validation: None,
        deterministic_validation: None,
        rag_context_refs: Vec::new(),
        rag_prompt_context: String::new(),
        tdd_phase: "final".to_string(),
        qa_test_paths: Vec::new(),
        qa_test_commands: Vec::new(),
        qa_baseline_validation: None,
        fix_attempts: 0,
        coverage_evidence: Vec::new(),
        group_id: Some(task_group_id_for_source(source_task)),
        hierarchy,
    }
}

fn task_needs_immediate_ai_qa(run: &AgenticBoard, task: &BoardTask, parsed: &Value) -> bool {
    if is_qa_task(task) || task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID {
        return false;
    }
    match run
        .qa_policy
        .get("taskQaMode")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("high_risk")
    {
        "off" => return false,
        "all" => return true,
        _ => {}
    }
    if matches!(task.priority.as_str(), TASK_PRIORITY_P0 | TASK_PRIORITY_P1)
        || task.qa_fix_task
        || task.followup_task
    {
        return true;
    }
    let text = task_risk_text(task, parsed).to_lowercase();
    [
        "auth",
        "login",
        "oauth",
        "permission",
        "security",
        "payment",
        "billing",
        "checkout",
        "database",
        "db",
        "migration",
        "schema",
        "sql",
        "api",
        "route",
        "endpoint",
        "server",
        "provider",
        "model",
        "token",
        "quota",
        "websocket",
        "critical",
        "data loss",
        "destructive",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn has_agents_knowledge_task_for_source(run: &AgenticBoard, source_task_id: &str) -> bool {
    run.tasks.iter().any(|task| {
        (task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID)
            && task.source_task_id.as_deref() == Some(source_task_id)
    })
}

fn task_needs_agents_knowledge_update(task: &BoardTask, parsed: &Value) -> bool {
    if is_qa_task(task) || task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID {
        return false;
    }
    let text = task_risk_text(task, parsed).to_lowercase();
    [
        "agents.md",
        "architecture",
        "convention",
        "command",
        "script",
        "setup",
        "build",
        "test",
        "lint",
        "database",
        "migration",
        "schema",
        "route",
        "api",
        "provider",
        "model",
        "env",
        "config",
        "docker",
        "playwright",
        "gotcha",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn task_risk_text(task: &BoardTask, parsed: &Value) -> String {
    [
        task.title.as_str(),
        task.summary.as_str(),
        task.details.as_str(),
        &task.acceptance_criteria.join("\n"),
        &task.references.join("\n"),
        parsed.get("summary").and_then(Value::as_str).unwrap_or(""),
        &normalize_string_list(parsed.get("evidence")).join("\n"),
        &normalize_string_list(parsed.get("remainingIssues")).join("\n"),
        &change_summary_paths(task.changed_file_summary.as_ref().unwrap_or(&Value::Null))
            .join("\n"),
    ]
    .join("\n")
}

fn create_agents_knowledge_task(
    run: &AgenticBoard,
    reason: &str,
    source_task: Option<&BoardTask>,
) -> BoardTask {
    let source_task_id = source_task.map(|task| task.id.clone());
    let source_group_id = source_task.map(task_group_id_for_source);
    let source_title = source_task
        .map(|task| task.title.clone())
        .unwrap_or_default();
    let id = if source_task_id.is_some() {
        unique_task_id(run, AGENTS_KNOWLEDGE_TASK_ID)
    } else {
        AGENTS_KNOWLEDGE_TASK_ID.to_string()
    };
    let title = if source_task_id.is_some() {
        if source_title.trim().is_empty() {
            "Update AGENTS.md with durable knowledge".to_string()
        } else {
            format!("Update AGENTS.md with durable knowledge after: {source_title}")
        }
    } else {
        "Update AGENTS.md with stable project knowledge from this agentic board".to_string()
    };
    let details = [
        "Read the applicable AGENTS.md file if it exists, or create a root AGENTS.md only when durable project guidance is available.".to_string(),
        "Record stable commands, architecture conventions, test workflows, or workflow rules that future coding agents should know.".to_string(),
        "Verify concrete project claims against the current filesystem, config, routes, migrations, command output, or QA evidence before writing them.".to_string(),
        "Do not add a task ledger, timestamps, transient run status, raw QA logs, or one-off implementation details.".to_string(),
        "Leave AGENTS.md unchanged if there is no stable project knowledge worth preserving.".to_string(),
        if reason.trim().is_empty() { String::new() } else { format!("Reason: {reason}") },
    ]
    .into_iter()
    .filter(|item| !item.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n");
    let hierarchy = executable_system_subtask_hierarchy(
        source_task.and_then(system_sibling_parent_id),
        source_task
            .map(|task| vec![task.id.clone()])
            .unwrap_or_default(),
        source_task
            .map(|task| task.hierarchy.scope_version)
            .unwrap_or(1),
        source_task
            .map(|task| task.hierarchy.rank.saturating_add(1))
            .unwrap_or(0),
        source_task
            .map(|task| task.hierarchy.planned_files.clone())
            .unwrap_or_default(),
    );
    BoardTask {
        id,
        title,
        status: TASK_STATUS_TODO.to_string(),
        summary: String::new(),
        details: details.clone(),
        description: details.clone(),
        prompt: details,
        error: None,
        acceptance_criteria: vec![
            "Read applicable AGENTS.md guidance before editing.".to_string(),
            "Preserve only stable project knowledge worth reusing in future coding tasks."
                .to_string(),
            "Return the required task result JSON contract.".to_string(),
        ],
        references: vec![
            "Applicable AGENTS.md files".to_string(),
            "Codebase recon summary".to_string(),
            "Completed implementation summaries".to_string(),
            source_task_id
                .as_ref()
                .map(|id| format!("Source task: {id}"))
                .unwrap_or_default(),
        ]
        .into_iter()
        .filter(|item| !item.trim().is_empty())
        .collect(),
        priority: source_task
            .map(|task| normalize_priority(Some(&task.priority)).to_string())
            .unwrap_or_else(|| TASK_PRIORITY_P3.to_string()),
        depends_on: source_task_id.iter().cloned().collect(),
        manual_task: false,
        prompt_task: false,
        task_origin: "system_agents".to_string(),
        task_type: TASK_KIND_REVIEW.to_string(),
        backlog_generation_task: false,
        qa_task: false,
        final_qa_task: false,
        followup_task: false,
        qa_fix_task: false,
        qa_verdict_retry_task: false,
        task_level_qa: false,
        agents_knowledge_task: true,
        internal_validation: false,
        qa_round: 0,
        source_task_id,
        source_qa_task_id: None,
        superseded_by: None,
        transcript: Vec::new(),
        transcript_updated_at: None,
        started_at: None,
        completed_at: None,
        qa_passed: None,
        attempt_count: 0,
        provider_session_id: None,
        commands_run: Vec::new(),
        changed_files: Vec::new(),
        changed_file_summary: None,
        evidence: Vec::new(),
        remaining_issues: Vec::new(),
        result: None,
        result_validation: None,
        deterministic_validation: None,
        rag_context_refs: Vec::new(),
        rag_prompt_context: String::new(),
        tdd_phase: default_tdd_phase(),
        qa_test_paths: Vec::new(),
        qa_test_commands: Vec::new(),
        qa_baseline_validation: None,
        fix_attempts: 0,
        coverage_evidence: Vec::new(),
        group_id: source_group_id.or_else(|| Some(AGENTS_KNOWLEDGE_TASK_ID.to_string())),
        hierarchy,
    }
}

fn is_qa_task(task: &BoardTask) -> bool {
    task.qa_task || task.final_qa_task || task.id == FINAL_QA_TASK_ID
}

fn is_qa_task_id(run: &AgenticBoard, task_id: &str) -> bool {
    run.tasks
        .iter()
        .find(|task| task.id == task_id)
        .map(is_qa_task)
        .unwrap_or(false)
}

fn is_qa_verdict_retry_task_id(run: &AgenticBoard, task_id: &str) -> bool {
    run.tasks
        .iter()
        .find(|task| task.id == task_id)
        .map(|task| task.qa_verdict_retry_task)
        .unwrap_or(false)
}

fn is_missing_final_json_result(parsed: &Value) -> bool {
    if parsed.get("parsedJson").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    if parsed.get("status").and_then(Value::as_str) != Some("needs_followup") {
        return false;
    }
    let text = [
        parsed.get("summary").and_then(Value::as_str).unwrap_or(""),
        &normalize_string_list(parsed.get("remainingIssues")).join("\n"),
    ]
    .join("\n")
    .to_lowercase();
    text.contains("final json") || text.contains("valid json") || text.contains("required json")
}

fn qa_needs_followup(parsed: &Value) -> bool {
    parsed.get("status").and_then(Value::as_str) == Some("needs_followup")
        || parsed.get("qaPassed").and_then(Value::as_bool) == Some(false)
        || (parsed_status_done(Some(parsed))
            && !normalize_string_list(parsed.get("remainingIssues")).is_empty())
}

fn should_queue_qa_verdict_retry(
    run: &AgenticBoard,
    task_id: &str,
    parsed: &Value,
    change_summary: &Value,
) -> bool {
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id) else {
        return false;
    };
    is_qa_task(task)
        && !task.qa_verdict_retry_task
        && is_missing_final_json_result(parsed)
        && change_summary
            .get("touchedFileCount")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
        && !run.tasks.iter().any(|candidate| {
            candidate.qa_verdict_retry_task
                && candidate.source_qa_task_id.as_deref() == Some(task_id)
        })
}

fn queue_qa_verdict_retry(run: &mut AgenticBoard, task_id: &str, parsed: &Value) -> bool {
    let Some(source_index) = run.tasks.iter().position(|task| task.id == task_id) else {
        return false;
    };
    let source_task = run.tasks[source_index].clone();
    if let Some(task) = run.tasks.get_mut(source_index) {
        task.status = TASK_STATUS_DONE.to_string();
        task.qa_passed = Some(false);
        task.error = Some(normalize_string_list(parsed.get("remainingIssues")).join("; "));
        task.summary = parsed
            .get("summary")
            .and_then(Value::as_str)
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or("QA completed checks but missed the required final JSON.")
            .to_string();
        task.completed_at = Some(Utc::now());
    }
    let id = format!("qa-verdict-retry-{}", run.tasks.len() + 1);
    let details = [
        format!("Source QA task: {} - {}", source_task.id, source_task.title),
        parsed
            .get("summary")
            .and_then(Value::as_str)
            .map(|summary| format!("Previous QA summary: {summary}"))
            .unwrap_or_default(),
        "Review the existing QA transcript and current files only as needed. Return the required final JSON verdict. Do not edit files.".to_string(),
    ]
    .into_iter()
    .filter(|part| !part.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    let hierarchy = executable_system_subtask_hierarchy(
        system_sibling_parent_id(&source_task),
        vec![task_id.to_string()],
        source_task.hierarchy.scope_version,
        source_task.hierarchy.rank.saturating_add(1),
        source_task.hierarchy.planned_files.clone(),
    );
    let retry = BoardTask {
        id: id.clone(),
        title: format!("QA verdict retry: {}", source_task.title),
        status: TASK_STATUS_TODO.to_string(),
        summary: String::new(),
        details: details.clone(),
        description: details.clone(),
        prompt: details,
        error: None,
        acceptance_criteria: vec![
            "Return only the required task result JSON contract.".to_string(),
            "Do not edit files or run unrelated implementation work.".to_string(),
            "If QA found actionable defects, return needs_followup with exact findings."
                .to_string(),
        ],
        references: vec![format!("Source QA task: {task_id}")],
        priority: source_task.priority.clone(),
        depends_on: vec![task_id.to_string()],
        manual_task: false,
        prompt_task: false,
        task_origin: "system_qa_verdict_retry".to_string(),
        task_type: TASK_KIND_QA.to_string(),
        backlog_generation_task: false,
        qa_task: true,
        final_qa_task: source_task.final_qa_task,
        followup_task: false,
        qa_fix_task: false,
        qa_verdict_retry_task: true,
        task_level_qa: source_task.task_level_qa,
        agents_knowledge_task: false,
        internal_validation: source_task.internal_validation,
        qa_round: source_task.qa_round.saturating_add(1),
        source_task_id: source_task.source_task_id.clone(),
        source_qa_task_id: Some(task_id.to_string()),
        superseded_by: None,
        transcript: Vec::new(),
        transcript_updated_at: None,
        started_at: None,
        completed_at: None,
        qa_passed: None,
        attempt_count: 0,
        provider_session_id: None,
        commands_run: Vec::new(),
        changed_files: Vec::new(),
        changed_file_summary: None,
        evidence: Vec::new(),
        remaining_issues: Vec::new(),
        result: None,
        result_validation: None,
        deterministic_validation: None,
        rag_context_refs: Vec::new(),
        rag_prompt_context: String::new(),
        tdd_phase: if source_task.qa_test_commands.is_empty() {
            default_tdd_phase()
        } else {
            "fix_pending".to_string()
        },
        qa_test_paths: source_task.qa_test_paths.clone(),
        qa_test_commands: source_task.qa_test_commands.clone(),
        qa_baseline_validation: source_task.qa_baseline_validation.clone(),
        fix_attempts: source_task.fix_attempts.saturating_add(1),
        coverage_evidence: source_task.coverage_evidence.clone(),
        group_id: Some(task_group_id_for_source(&source_task)),
        hierarchy,
    };
    run.tasks.insert(source_index + 1, retry);
    run.append_log(format!(
        "QA JSON contract missing for {task_id}; queued compact verdict retry {id}"
    ));
    true
}

fn mark_qa_verdict_retry_blocked(run: &mut AgenticBoard, task_id: &str, parsed: &Value) {
    if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
        task.status = "blocked".to_string();
        task.qa_passed = Some(false);
        task.error = Some(
            normalize_string_list(parsed.get("remainingIssues"))
                .join("; ")
                .chars()
                .take(1200)
                .collect(),
        );
        task.summary = parsed
            .get("summary")
            .and_then(Value::as_str)
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or("QA verdict retry could not produce the required JSON contract.")
            .to_string();
        task.completed_at = Some(Utc::now());
    }
    run.append_log(format!(
        "QA verdict retry {task_id} missed final JSON; not queueing implementation work"
    ));
}

fn append_followup_task_if_needed(
    run: &mut AgenticBoard,
    source_task_id: &str,
    parsed: &Value,
) -> bool {
    if parsed.get("status").and_then(Value::as_str) != Some("needs_followup") {
        return false;
    }
    let Some(source_task) = run
        .tasks
        .iter()
        .find(|task| task.id == source_task_id)
        .cloned()
    else {
        return false;
    };
    if uses_hierarchical_orchestration(run)
        && !matches!(
            canonical_task_kind(&source_task),
            TASK_KIND_QA | TASK_KIND_MANUAL_TEST | TASK_KIND_REVIEW
        )
    {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == source_task_id) {
            task.status = TASK_STATUS_BLOCKED.to_string();
            task.error = Some(
                "Subtask reported incomplete work inside the approved scope; create or discuss a concrete fix subtask under this parent."
                    .to_string(),
            );
        }
        return false;
    }
    let group_id = source_task
        .group_id
        .clone()
        .unwrap_or_else(|| source_task.id.clone());
    let existing_followups = run
        .tasks
        .iter()
        .filter(|task| task.followup_task && task.group_id.as_deref() == Some(&group_id))
        .count();
    let max_followups = max_followups_per_group(run);
    if existing_followups >= max_followups {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == source_task_id) {
            task.status = "blocked".to_string();
            task.error = Some(format!(
                "Follow-up limit reached for {group_id} ({max_followups})."
            ));
        }
        run.append_log(format!(
            "Task follow-up limit reached for {source_task_id}; marked blocked"
        ));
        return false;
    }
    let max_fix_attempts = max_tdd_fix_attempts(run);
    if !uses_hierarchical_orchestration(run)
        && !source_task.qa_test_commands.is_empty()
        && source_task.fix_attempts >= max_fix_attempts
    {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == source_task_id) {
            task.status = "blocked".to_string();
            task.tdd_phase = "blocked".to_string();
            task.error = Some(format!(
                "TDD max fix attempts reached ({max_fix_attempts})."
            ));
        }
        run.append_log(format!(
            "TDD max fix attempts reached for {source_task_id}; no further fix task queued"
        ));
        return false;
    }
    let followup_index = existing_followups + 1;
    let qa_fix = is_qa_task(&source_task)
        || matches!(
            canonical_task_kind(&source_task),
            TASK_KIND_MANUAL_TEST | TASK_KIND_REVIEW
        );
    let suggested = parsed
        .get("suggestedBacklogTasks")
        .or_else(|| parsed.get("suggestedTasks"))
        .and_then(Value::as_array)
        .and_then(|items| items.first());
    let suggested_title = suggested
        .and_then(|item| item.get("title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty());
    let title = if qa_fix {
        suggested_title
            .map(str::to_string)
            .unwrap_or_else(|| format!("Fix validation findings for: {}", source_task.title))
    } else {
        format!("Continue follow-up: {}", source_task.title)
    };
    let issues = normalize_string_list(parsed.get("remainingIssues"))
        .into_iter()
        .chain(normalize_string_list(parsed.get("remainingGaps")))
        .collect::<Vec<_>>();
    let suggested_details = suggested
        .and_then(|item| item.get("details").or_else(|| item.get("description")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|details| !details.is_empty());
    let details = [
        format!("Source task: {} - {}", source_task.id, source_task.title),
        suggested_details
            .map(|details| format!("Requested fix scope: {details}"))
            .unwrap_or_default(),
        parsed
            .get("summary")
            .and_then(Value::as_str)
            .map(|summary| format!("Source summary: {summary}"))
            .unwrap_or_default(),
        if issues.is_empty() {
            "Remaining issue: continue the incomplete work from the source task.".to_string()
        } else {
            format!("Remaining issues:\n- {}", issues.join("\n- "))
        },
    ]
    .into_iter()
    .filter(|part| !part.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    let id = format!("task-followup-{}-{}", run.tasks.len() + 1, followup_index);
    let followup = BoardTask {
        id: id.clone(),
        title,
        status: TASK_STATUS_TODO.to_string(),
        summary: String::new(),
        details: details.clone(),
        description: details.clone(),
        prompt: details,
        error: None,
        acceptance_criteria: source_task.acceptance_criteria.clone(),
        references: [
            source_task.references.clone(),
            vec![format!("Source task: {source_task_id}")],
        ]
        .concat(),
        priority: suggested
            .and_then(|item| item.get("priority").and_then(Value::as_str))
            .map(|priority| normalize_priority(Some(priority)).to_string())
            .unwrap_or_else(|| {
                if qa_fix {
                    TASK_PRIORITY_P1.to_string()
                } else {
                    source_task.priority.clone()
                }
            }),
        depends_on: vec![source_task_id.to_string()],
        manual_task: false,
        prompt_task: false,
        task_origin: if qa_fix {
            "system_qa_fix".to_string()
        } else {
            "system_followup".to_string()
        },
        task_type: if qa_fix {
            TASK_KIND_FIX.to_string()
        } else {
            TASK_KIND_FOLLOWUP.to_string()
        },
        backlog_generation_task: false,
        qa_task: false,
        final_qa_task: false,
        followup_task: true,
        qa_fix_task: qa_fix,
        qa_verdict_retry_task: false,
        task_level_qa: false,
        agents_knowledge_task: false,
        internal_validation: false,
        qa_round: 0,
        source_task_id: Some(source_task_id.to_string()),
        source_qa_task_id: qa_fix.then(|| source_task_id.to_string()),
        superseded_by: None,
        transcript: Vec::new(),
        transcript_updated_at: None,
        started_at: None,
        completed_at: None,
        qa_passed: None,
        attempt_count: 0,
        provider_session_id: None,
        commands_run: Vec::new(),
        changed_files: Vec::new(),
        changed_file_summary: None,
        evidence: Vec::new(),
        remaining_issues: Vec::new(),
        result: None,
        result_validation: None,
        deterministic_validation: None,
        rag_context_refs: Vec::new(),
        rag_prompt_context: String::new(),
        tdd_phase: default_tdd_phase(),
        qa_test_paths: Vec::new(),
        qa_test_commands: Vec::new(),
        qa_baseline_validation: None,
        fix_attempts: 0,
        coverage_evidence: Vec::new(),
        group_id: Some(group_id.clone()),
        hierarchy: BoardTaskHierarchy::default(),
    };
    let mut followup = followup;
    if uses_hierarchical_orchestration(run) {
        followup.task_type = TASK_KIND_FIX.to_string();
        followup.hierarchy = BoardTaskHierarchy {
            level: TASK_LEVEL_SUBTASK.to_string(),
            parent_id: source_task.hierarchy.parent_id.clone(),
            blocked_by: vec![source_task_id.to_string()],
            executable: true,
            required: true,
            scope_version: source_task.hierarchy.scope_version,
            rank: source_task.hierarchy.rank.saturating_add(1),
            attempts: Vec::new(),
            planned_files: source_task.hierarchy.planned_files.clone(),
            side_effects: Vec::new(),
            side_effects_approved: false,
            side_effect_approval: None,
            side_effect_evidence: Vec::new(),
            manual_test_environment: None,
            research_accepted: false,
            research_acceptance: None,
            discussion: Vec::new(),
        };
        followup.depends_on = vec![source_task_id.to_string()];
        followup.group_id = Some(group_id.clone());
    }
    let insert_index = run
        .tasks
        .iter()
        .position(|task| task.id == source_task_id)
        .map(|index| index + 1)
        .unwrap_or(run.tasks.len());
    run.tasks.insert(insert_index, followup);
    if source_task.final_qa_task {
        let _ = append_final_qa_task(run, &format!("Rerun after {id}"));
    }
    run.append_log(format!("Task requires follow-up; queued {id}"));
    true
}

async fn compact_provider_session_after_task_group(
    state: &AppState,
    run: &AgenticBoard,
    task_id: &str,
) -> Option<Value> {
    let task = run.tasks.iter().find(|task| task.id == task_id)?;
    let group_id = task.group_id.clone().unwrap_or_else(|| task.id.clone());
    if run
        .compaction_ledger
        .iter()
        .any(|entry| entry.get("groupId").and_then(Value::as_str) == Some(group_id.as_str()))
    {
        return None;
    }
    if normalize_session_policy(Some(&run.session_policy)) != "continuous" {
        return Some(json!({
            "groupId": group_id,
            "taskId": task_id,
            "status": "skipped",
            "reason": "Session policy is not continuous.",
            "createdAt": Utc::now(),
        }));
    }
    let Some(session_id) = reusable_session_id(run) else {
        return Some(json!({
            "groupId": group_id,
            "taskId": task_id,
            "status": "skipped",
            "reason": "No reusable provider session was available.",
            "createdAt": Utc::now(),
        }));
    };
    if run.provider != "claude" {
        return Some(json!({
            "groupId": group_id,
            "taskId": task_id,
            "status": "skipped",
            "reason": format!("Provider {} does not support automatic /compact.", run.provider),
            "sessionId": session_id,
            "createdAt": Utc::now(),
        }));
    }
    let started_at = Utc::now();
    match execute_provider_prompt(state, run, "context compaction", "/compact").await {
        Ok(output) => Some(json!({
            "groupId": group_id,
            "taskId": task_id,
            "status": "completed",
            "sessionId": session_id,
            "startedAt": started_at,
            "completedAt": Utc::now(),
            "summary": limit_text(&output.output, 600),
        })),
        Err(error) => Some(json!({
            "groupId": group_id,
            "taskId": task_id,
            "status": "failed",
            "sessionId": session_id,
            "startedAt": started_at,
            "completedAt": Utc::now(),
            "error": server_error_message(&error),
        })),
    }
}

fn pending_research_acceptance_ids(run: &AgenticBoard) -> Vec<String> {
    run.tasks
        .iter()
        .filter(|task| task_is_executable(task))
        .filter(|task| canonical_task_kind(task) == TASK_KIND_RESEARCH)
        .filter(|task| task_status_is_done(&task.status))
        .filter(|task| !task.hierarchy.research_accepted)
        .map(|task| task.id.clone())
        .collect()
}

fn has_pending_research_acceptance(run: &AgenticBoard) -> bool {
    !pending_research_acceptance_ids(run).is_empty()
}

fn has_backlog_planning_work(run: &AgenticBoard) -> bool {
    run.tasks.iter().any(|task| {
        !task.backlog_generation_task
            && task_status_is_backlog(&task.status)
            && !task_is_executable(task)
    })
}

fn pick_next_task_index(run: &AgenticBoard) -> Option<usize> {
    let mut ready = Vec::<(usize, u8, i64)>::new();
    for (index, task) in run.tasks.iter().enumerate() {
        if !task_is_runnable_in_board(run, task) {
            continue;
        }
        let unmet = unmet_task_dependencies(run, task);
        if unmet.is_empty() {
            ready.push((
                index,
                task_priority_rank(&task.priority),
                task.hierarchy.rank.max(0),
            ));
        }
    }
    ready
        .into_iter()
        .min_by_key(|(index, priority, rank)| (*priority, *rank, *index))
        .map(|(index, _, _)| index)
}

fn unmet_task_dependencies(run: &AgenticBoard, task: &BoardTask) -> Vec<String> {
    let mut dependencies = task_blockers(task);
    if let Some(id) = retry_fix_dependency(task) {
        if !dependencies.contains(&id) {
            dependencies.push(id);
        }
    }
    dependencies
        .into_iter()
        .filter(|id| id != &task.id)
        .filter(|id| {
            run.tasks
                .iter()
                .find(|candidate| candidate.id == *id)
                .is_none_or(|candidate| !task_dependency_is_satisfied(candidate))
        })
        .collect()
}

fn task_dependency_is_satisfied(task: &BoardTask) -> bool {
    task_is_done(task) && task.superseded_by.is_none()
}

fn dependency_block_reason(run: &AgenticBoard, dependencies: &[String]) -> String {
    let missing = dependencies
        .iter()
        .filter(|dependency| !run.tasks.iter().any(|task| task.id == **dependency))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return format!("Missing dependency: {}", missing.join(", "));
    }
    let superseded = dependencies
        .iter()
        .filter_map(|dependency| {
            run.tasks
                .iter()
                .find(|task| task.id == *dependency)
                .and_then(|task| {
                    task.superseded_by
                        .as_ref()
                        .map(|replacement| format!("{dependency} (replacement {replacement})"))
                })
        })
        .collect::<Vec<_>>();
    if !superseded.is_empty() {
        return format!(
            "Superseded dependency: {}. Choose the replacement dependency or remove the blocker.",
            superseded.join(", ")
        );
    }
    format!("Waiting on dependency: {}", dependencies.join(", "))
}

fn is_dependency_block_error(error: &str) -> bool {
    error.starts_with("Waiting on dependency:")
        || error.starts_with("Missing dependency:")
        || error.starts_with("Superseded dependency:")
}

fn retry_fix_dependency(task: &BoardTask) -> Option<String> {
    task.hierarchy
        .attempts
        .iter()
        .rev()
        .find(|attempt| attempt.get("kind").and_then(Value::as_str) == Some("retry_request"))
        .filter(|attempt| attempt.get("mode").and_then(Value::as_str) == Some(RETRY_MODE_FIX))
        .and_then(|attempt| attempt.get("fixTaskId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn hierarchy_validation_issues(run: &AgenticBoard) -> Vec<String> {
    let by_id = run
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect::<BTreeMap<_, _>>();
    let mut issues = Vec::new();
    for task in &run.tasks {
        let Some(parent_id) = task.hierarchy.parent_id.as_deref() else {
            continue;
        };
        if parent_id == task.id {
            issues.push(format!(
                "Hierarchy item {} cannot be its own parent.",
                task.id
            ));
            continue;
        }
        let Some(parent) = by_id.get(parent_id) else {
            issues.push(format!(
                "Hierarchy item {} has missing parent {}.",
                task.id, parent_id
            ));
            continue;
        };
        let valid_parent = match task_level(task) {
            TASK_LEVEL_EPIC => task_level(parent) == TASK_LEVEL_INITIATIVE,
            TASK_LEVEL_STORY => task_level(parent) == TASK_LEVEL_EPIC,
            TASK_LEVEL_TASK => task_level(parent) == TASK_LEVEL_STORY,
            TASK_LEVEL_SUBTASK => task_level(parent) == TASK_LEVEL_TASK,
            TASK_LEVEL_INITIATIVE => false,
            _ => false,
        };
        if !valid_parent {
            issues.push(format!(
                "Hierarchy item {} ({}) has invalid parent {} ({}).",
                task.id,
                task_level(task),
                parent.id,
                task_level(parent)
            ));
        }
        // Compare each item with every explicit hierarchy ancestor. A
        // neutral intermediate task must not hide a contradiction between a
        // story and a subtask farther down the plan.
        let mut ancestor_id = Some(parent.id.as_str());
        let mut seen_ancestors = BTreeSet::new();
        while let Some(ancestor_id_value) = ancestor_id {
            if !seen_ancestors.insert(ancestor_id_value) {
                break;
            }
            let Some(ancestor) = by_id.get(ancestor_id_value) else {
                break;
            };
            for ancestor_criterion in &ancestor.acceptance_criteria {
                for child_criterion in &task.acceptance_criteria {
                    if let Some(conflict) =
                        acceptance_criteria_conflict(ancestor_criterion, child_criterion)
                    {
                        issues.push(format!(
                            "Acceptance criteria conflict between {} and {}: {} Parent: {} Child: {}",
                            ancestor.id, task.id, conflict, ancestor_criterion, child_criterion,
                        ));
                    }
                }
            }
            ancestor_id = ancestor.hierarchy.parent_id.as_deref();
        }
    }
    for task in &run.tasks {
        let mut current = task.id.as_str();
        let mut seen = BTreeSet::new();
        while let Some(item) = by_id.get(current) {
            if !seen.insert(current) {
                issues.push(format!("Hierarchy cycle detected at {}.", current));
                break;
            }
            let Some(parent_id) = item.hierarchy.parent_id.as_deref() else {
                break;
            };
            current = parent_id;
        }
    }
    issues.sort();
    issues.dedup();
    issues
}

fn acceptance_criteria_conflict(parent: &str, child: &str) -> Option<&'static str> {
    let parent_constraints = acceptance_constraint_flags(&parent.to_ascii_lowercase());
    let child_constraints = acceptance_constraint_flags(&child.to_ascii_lowercase());
    let conflicts = [
        ("zero is allowed", 0usize, 1usize),
        ("a positive value is required", 1usize, 0usize),
        ("negative values are allowed", 2usize, 3usize),
        ("a non-negative value is required", 3usize, 2usize),
        ("an empty value is allowed", 4usize, 5usize),
        ("a non-empty value is required", 5usize, 4usize),
        ("the value is optional", 6usize, 7usize),
        ("the value is required", 7usize, 6usize),
    ];
    conflicts
        .iter()
        .find(|(_, parent_index, child_index)| {
            parent_constraints[*parent_index] && child_constraints[*child_index]
        })
        .map(|(message, _, _)| *message)
}

fn acceptance_constraint_flags(value: &str) -> [bool; 8] {
    let zero_allowed = value.contains("can be zero")
        || value.contains("may be zero")
        || value.contains("allow zero")
        || value.contains("allows zero")
        || value.contains("zero is valid")
        || value.contains("non-negative");
    let positive_required = value.contains("must be positive")
        || value.contains("must have a positive")
        || value.contains("positive value required")
        || value.contains("greater than zero")
        || value.contains("above zero")
        || value.contains("at least one");
    let negative_allowed = value.contains("can be negative")
        || value.contains("may be negative")
        || value.contains("allow negative")
        || value.contains("negative values are valid");
    let non_negative_required = value.contains("must be non-negative")
        || value.contains("non-negative value required")
        || value.contains("zero or greater")
        || value.contains("not be negative");
    let empty_allowed = value.contains("can be empty")
        || value.contains("may be empty")
        || value.contains("allow empty")
        || value.contains("empty is valid");
    let non_empty_required = value.contains("must not be empty")
        || value.contains("cannot be empty")
        || value.contains("non-empty value required")
        || value.contains("must be non-empty");
    let optional = value.contains("is optional")
        || value.contains("are optional")
        || value.contains("may be omitted")
        || value.contains("can be omitted");
    let required = value.contains("is required")
        || value.contains("are required")
        || value.contains("must be provided")
        || value.contains("required input");
    [
        zero_allowed,
        positive_required,
        negative_allowed,
        non_negative_required,
        empty_allowed,
        non_empty_required,
        optional,
        required,
    ]
}

fn dependency_cycle(run: &AgenticBoard) -> Option<Vec<String>> {
    fn visit(
        run: &AgenticBoard,
        id: &str,
        visiting: &mut Vec<String>,
        visited: &mut BTreeSet<String>,
    ) -> Option<Vec<String>> {
        if let Some(index) = visiting.iter().position(|value| value == id) {
            let mut cycle = visiting[index..].to_vec();
            cycle.push(id.to_string());
            return Some(cycle);
        }
        if !visited.insert(id.to_string()) {
            return None;
        }
        visiting.push(id.to_string());
        let task = run.tasks.iter().find(|task| task.id == id)?;
        for dependency in task_blockers(task) {
            if run.tasks.iter().any(|candidate| candidate.id == dependency)
                && let Some(cycle) = visit(run, &dependency, visiting, visited)
            {
                return Some(cycle);
            }
        }
        visiting.pop();
        Some(Vec::new()).filter(|cycle| !cycle.is_empty())
    }

    let mut visited = BTreeSet::new();
    for task in &run.tasks {
        if let Some(cycle) = visit(run, &task.id, &mut Vec::new(), &mut visited)
            && !cycle.is_empty()
        {
            return Some(cycle);
        }
    }
    None
}

fn planning_error_task_ids(run: &AgenticBoard, issue: &str) -> Vec<String> {
    run.tasks
        .iter()
        .filter(|task| !task.id.trim().is_empty() && issue.contains(&task.id))
        .map(|task| task.id.clone())
        .collect()
}

fn has_persisted_planning_error(run: &AgenticBoard) -> bool {
    run.current_phase.as_deref() == Some(PLANNING_ERROR_PHASE)
        && run
            .phase_details
            .as_ref()
            .and_then(|details| details.get("kind"))
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "hierarchy" || kind == "dependency")
}

fn mark_planning_error(
    run: &mut AgenticBoard,
    affected_task_ids: &[String],
    kind: &str,
    issue: &str,
) {
    let now = Utc::now();
    let affected = affected_task_ids
        .iter()
        .filter(|id| run.tasks.iter().any(|task| task.id == **id))
        .cloned()
        .collect::<Vec<_>>();
    let message = format!("Planning error: {}", issue.trim());
    for task in &mut run.tasks {
        if affected.iter().any(|id| id == &task.id) {
            // Completed work is immutable. It is still included in the
            // board-level issue details, but only unfinished affected items
            // are moved to Blocked.
            if task_status_is_done(&task.status) {
                continue;
            }
            task.status = TASK_STATUS_BLOCKED.to_string();
            task.error = Some(message.clone());
            task.summary = message.clone();
            task.completed_at = None;
        }
    }
    run.status = TASK_STATUS_BLOCKED.to_string();
    run.active = false;
    run.loop_started = false;
    run.auto_run_enabled = false;
    run.pause_requested = false;
    run.current_task_id = None;
    run.current_task_title.clear();
    run.current_task_status.clear();
    run.current_phase = Some(PLANNING_ERROR_PHASE.to_string());
    run.phase_started_at = Some(now);
    run.phase_details = Some(json!({
        "kind": kind,
        "error": message,
        "affectedTaskIds": affected,
        "resolution": "Resolve the planning conflict, then regenerate or approve the affected plan.",
    }));
    run.pause_reason = Some(message.clone());
    run.append_log(format!("Blocked board on {kind} planning error: {message}"));
}

fn planning_error_conflict(
    run: &mut AgenticBoard,
    affected_task_ids: &[String],
    kind: &str,
    issue: impl Into<String>,
) -> ServerError {
    let issue = issue.into();
    mark_planning_error(run, affected_task_ids, kind, &issue);
    ServerError::new(StatusCode::CONFLICT, issue)
}

fn dependency_waiting_tasks(run: &AgenticBoard) -> Vec<String> {
    run.tasks
        .iter()
        .filter(|task| task_is_runnable_in_board(run, task))
        .filter(|task| !unmet_task_dependencies(run, task).is_empty())
        .map(|task| task.id.clone())
        .collect()
}

fn reconcile_dependency_statuses(run: &mut AgenticBoard) {
    let candidates = run
        .tasks
        .iter()
        .filter(|task| task_is_executable(task))
        .filter(|task| canonical_task_status(&task.status) == TASK_STATUS_BLOCKED)
        .filter(|task| task.error.as_deref().is_some_and(is_dependency_block_error))
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    for task_id in candidates {
        let Some(task) = run.tasks.iter().find(|task| task.id == task_id).cloned() else {
            continue;
        };
        if unmet_task_dependencies(run, &task).is_empty()
            && let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id)
        {
            task.status = TASK_STATUS_TODO.to_string();
            task.error = None;
            task.completed_at = None;
            run.append_log(format!(
                "Unblocked subtask {task_id}; all dependencies are complete"
            ));
        }
    }
}

fn has_hierarchical_attention_tasks(run: &AgenticBoard) -> bool {
    run.tasks.iter().any(|task| {
        task_is_executable(task)
            && matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
            )
            && !task.error.as_deref().is_some_and(is_dependency_block_error)
    })
}

fn has_dependency_blocked_tasks(run: &AgenticBoard) -> bool {
    run.tasks.iter().any(|task| {
        task_is_executable(task)
            && canonical_task_status(&task.status) == TASK_STATUS_BLOCKED
            && task.error.as_deref().is_some_and(is_dependency_block_error)
    })
}

fn hierarchical_work_is_complete(run: &AgenticBoard) -> bool {
    let executable = run
        .tasks
        .iter()
        .filter(|task| task_is_executable(task))
        .collect::<Vec<_>>();
    if executable.is_empty() {
        return run
            .tasks
            .iter()
            .filter(|task| !task.backlog_generation_task)
            .all(|task| {
                task_is_done(task)
                    || task_status_is_backlog(&task.status)
                    || !task_is_executable(task)
            })
            && run
                .tasks
                .iter()
                .any(|task| task_status_is_done(&task.status));
    }
    let approved = executable
        .iter()
        .filter(|task| !task_status_is_backlog(&task.status))
        .collect::<Vec<_>>();
    !approved.is_empty() && approved.iter().all(|task| task_is_done(task))
}

fn mark_dependency_blockers(run: &mut AgenticBoard) {
    let waiting = run
        .tasks
        .iter()
        // A missing dependency is a planning error even when the parent is
        // still waiting for approval. Keep the child explicitly blocked so
        // it cannot look like dormant approved work after the parent moves.
        .filter(|task| task_is_executable(task))
        .filter(|task| task_status_is_todo(&task.status))
        .map(|task| (task.id.clone(), unmet_task_dependencies(run, task)))
        .filter(|(_, dependencies)| !dependencies.is_empty())
        .collect::<Vec<_>>();
    for (task_id, dependencies) in waiting {
        let reason = dependency_block_reason(run, &dependencies);
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
            task.status = TASK_STATUS_BLOCKED.to_string();
            task.error = Some(reason);
        }
    }
}

fn task_priority_rank(priority: &str) -> u8 {
    match normalize_priority(Some(priority)) {
        TASK_PRIORITY_P0 => 0,
        TASK_PRIORITY_P1 => 1,
        TASK_PRIORITY_P2 => 2,
        TASK_PRIORITY_P3 => 3,
        _ => 3,
    }
}

#[derive(Debug, Default)]
struct Bundle {
    references: Vec<Value>,
    manifest: Vec<Value>,
    chunks: Vec<Value>,
}

async fn execute_internal_prompt(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    label: &str,
    prompt: &str,
) -> Result<String> {
    let mut stored = load_user_board(state, user_id, board_id)?;
    let provider = effective_provider_for_phase(&stored.board, label)?;
    let model = effective_model_for_phase(&stored.board, label);
    let reusable_session_id = reusable_session_id_for_provider(&stored.board, &provider);
    stored.board.provider_call_started_at = Some(Utc::now());
    stored.board.provider_call_label = Some(label.to_string());
    stored.board.current_provider_session_id = reusable_session_id;
    let execution_model = agentic_execution_model_for_provider(&provider, &model);
    let mut telemetry = json!({
        "phase": stored.board.current_phase,
        "label": label,
        "provider": provider,
        "model": model,
        "chars": prompt.chars().count(),
        "estimatedTokens": estimate_tokens(prompt),
        "startedAt": Utc::now(),
    });
    if execution_model != model {
        telemetry["executionModel"] = json!(execution_model);
    }
    stored.board.prompt_telemetry.push(telemetry);
    let telemetry_index = stored.board.prompt_telemetry.len().saturating_sub(1);
    stored.board.touch();
    save_board(state, &stored.board)?;

    let result = execute_provider_prompt(state, &stored.board, label, prompt).await;
    let mut stored = load_user_board(state, user_id, board_id)?;
    stored.board.provider_call_started_at = None;
    stored.board.provider_call_label = None;
    stored.board.current_provider_session_id = None;
    match &result {
        Ok(output) => {
            finalize_prompt_telemetry(
                &mut stored.board,
                telemetry_index,
                output.session_id.as_deref(),
                output.effective_model.as_deref(),
                output.token_usage.as_ref(),
            );
            increment_provider_usage(
                &mut stored.board,
                prompt,
                &output.output,
                output.session_id.as_deref(),
                output.token_usage.as_ref(),
            );
            stored
                .board
                .append_log(format!("Internal provider call completed: {label}"));
        }
        Err(error) => {
            stored.board.append_log(format!(
                "Internal provider call failed for {label}: {}",
                server_error_message(error)
            ));
        }
    }
    stored.board.touch();
    save_board(state, &stored.board)?;
    result.map(|output| output.output)
}

async fn execute_provider_prompt(
    state: &AppState,
    run: &AgenticBoard,
    label: &str,
    prompt: &str,
) -> Result<ProviderPromptResult> {
    let provider = effective_provider_for_phase(run, label)?;
    let model = effective_model_for_phase(run, label);
    let execution_model = agentic_execution_model_for_provider(&provider, &model);
    let result = execute_shared_provider_turn(
        state,
        run,
        &provider,
        &execution_model,
        prompt,
        reusable_session_id_for_provider(run, &provider).as_deref(),
        board_task_id_for_label(run, label).as_deref(),
    )
    .await?;
    if result.exit_code == 0 {
        return Ok(ProviderPromptResult {
            output: result.assistant_text,
            session_id: Some(result.session_id),
            token_usage: result.token_usage,
            effective_model: if execution_model.trim().is_empty() {
                None
            } else {
                Some(execution_model)
            },
        });
    }
    Err(ServerError::with_details(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("provider call failed during {label}"),
        result.summary,
    ))
}

fn finalize_prompt_telemetry(
    run: &mut AgenticBoard,
    telemetry_index: usize,
    session_id: Option<&str>,
    effective_model: Option<&str>,
    token_usage: Option<&Value>,
) {
    let Some(entry) = run
        .prompt_telemetry
        .get_mut(telemetry_index)
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) {
        entry.insert("sessionId".to_string(), json!(session_id));
    }
    if let Some(model) = effective_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        entry.insert("effectiveModel".to_string(), json!(model));
        entry.insert("model".to_string(), json!(model));
        run.last_effective_model = Some(model.to_string());
    }
    if let Some(usage) = token_usage {
        entry.insert("tokenUsage".to_string(), usage.clone());
        entry.insert(
            "actualInputTokens".to_string(),
            json!(
                usage
                    .get("inputTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ),
        );
        entry.insert(
            "actualCachedInputTokens".to_string(),
            json!(
                usage
                    .get("cachedInputTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ),
        );
        entry.insert(
            "actualOutputTokens".to_string(),
            json!(
                usage
                    .get("outputTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ),
        );
        entry.insert(
            "actualTotalTokens".to_string(),
            json!(
                usage
                    .get("totalTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ),
        );
    }
    entry.insert("completedAt".to_string(), json!(Utc::now()));
}

fn build_source_bundle(project_path: &str, source_prompt: &str) -> Bundle {
    let references = resolve_source_references(project_path, source_prompt);
    let mut files = BTreeMap::<PathBuf, Vec<String>>::new();
    for reference in &references {
        if let Some(path) = reference.get("absolutePath").and_then(Value::as_str) {
            let reason = reference
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("prompt-reference")
                .to_string();
            collect_text_files(
                Path::new(path),
                Path::new(project_path),
                MAX_SOURCE_FILES,
                &mut files,
                reason,
            );
        }
    }
    let mut bundle = Bundle {
        references,
        ..Bundle::default()
    };
    let mut chunk_counter = 1usize;
    for (absolute, reasons) in files {
        let relative = relative_display(Path::new(project_path), &absolute);
        match fs::read_to_string(&absolute) {
            Ok(content) => {
                let chunks = split_into_chunks(
                    &relative,
                    &content,
                    "SRC",
                    &mut chunk_counter,
                    SOURCE_CHUNK_TARGET_LENGTH,
                );
                let chunk_ids = chunks
                    .iter()
                    .filter_map(|chunk| chunk.get("id").and_then(Value::as_str).map(str::to_string))
                    .collect::<Vec<_>>();
                bundle.chunks.extend(chunks);
                let metadata = fs::metadata(&absolute).ok();
                bundle.manifest.push(json!({
                    "path": relative,
                    "size": metadata.as_ref().map(|meta| meta.len()).unwrap_or(0),
                    "mtime": metadata.and_then(|meta| meta.modified().ok()).map(DateTime::<Utc>::from),
                    "sha256": sha256_hex(content.as_bytes()),
                    "reasons": reasons,
                    "status": "loaded",
                    "chunkIds": chunk_ids,
                }));
            }
            Err(error) => bundle.manifest.push(json!({
                "path": relative,
                "reasons": reasons,
                "status": "unreadable",
                "error": error.to_string(),
                "chunkIds": [],
            })),
        }
    }
    bundle
}

fn build_codebase_bundle(project_path: &str) -> Bundle {
    let root = Path::new(project_path);
    let mut bundle = Bundle::default();
    let mut chunk_counter = 1usize;
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !should_skip_path(entry.path(), root))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .take(MAX_CODEBASE_FILES)
    {
        let absolute = entry.path().to_path_buf();
        let relative = relative_display(root, &absolute);
        let metadata = fs::metadata(&absolute).ok();
        match fs::read(&absolute) {
            Ok(bytes) => {
                let textual = looks_textual(&bytes) && should_chunk_codebase_file(&relative);
                let mut chunk_ids = Vec::new();
                if textual && bundle.chunks.len() < MAX_CODEBASE_CHUNKS {
                    let content = String::from_utf8_lossy(&bytes).to_string();
                    let chunks = split_into_chunks(
                        &relative,
                        &content,
                        "CDB",
                        &mut chunk_counter,
                        CODEBASE_CHUNK_TARGET_LENGTH,
                    );
                    chunk_ids = chunks
                        .iter()
                        .filter_map(|chunk| {
                            chunk.get("id").and_then(Value::as_str).map(str::to_string)
                        })
                        .collect();
                    bundle.chunks.extend(
                        chunks
                            .into_iter()
                            .take(MAX_CODEBASE_CHUNKS - bundle.chunks.len()),
                    );
                }
                bundle.manifest.push(json!({
                    "path": relative,
                    "size": metadata.as_ref().map(|meta| meta.len()).unwrap_or(bytes.len() as u64),
                    "mtime": metadata.and_then(|meta| meta.modified().ok()).map(DateTime::<Utc>::from),
                    "sha256": sha256_hex(&bytes),
                    "textual": textual,
                    "status": "loaded",
                    "aiUnderstandingSkipped": !textual,
                    "skipReason": if textual { "" } else { "binary, generated, dependency, or oversized artifact" },
                    "chunkIds": chunk_ids,
                }));
            }
            Err(error) => bundle.manifest.push(json!({
                "path": relative,
                "textual": false,
                "status": "unreadable",
                "error": error.to_string(),
                "chunkIds": [],
            })),
        }
    }
    bundle
}

fn resolve_source_references(project_path: &str, prompt: &str) -> Vec<Value> {
    let root = Path::new(project_path);
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut references = Vec::new();
    let mut seen = BTreeSet::<PathBuf>::new();
    for cleaned in prompt_source_locator_candidates(prompt) {
        let candidate = if Path::new(&cleaned).is_absolute() {
            PathBuf::from(&cleaned)
        } else {
            root.join(&cleaned)
        };
        let resolved = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.clone());
        if candidate.exists()
            && resolved.starts_with(&canonical_root)
            && seen.insert(resolved.clone())
        {
            references.push(json!({
                "matchedFrom": cleaned,
                "path": relative_display(&canonical_root, &resolved),
                "absolutePath": resolved,
                "reason": "prompt-reference",
            }));
        }
    }
    references
}

fn prompt_has_explicit_source_locator(run: &AgenticBoard) -> bool {
    !prompt_source_locator_candidates(&active_board_prompt(run)).is_empty()
}

fn prompt_source_locator_candidates(prompt: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    prompt
        .split_whitespace()
        .filter_map(normalize_prompt_source_token)
        .filter(|token| is_prompt_source_locator(token))
        .filter(|token| seen.insert(token.clone()))
        .collect()
}

fn normalize_prompt_source_token(token: &str) -> Option<String> {
    let mut value = token
        .trim_matches(|ch: char| {
            matches!(
                ch,
                '"' | '\'' | '`' | ',' | ':' | ';' | ')' | '(' | '[' | ']' | '<' | '>'
            )
        })
        .trim()
        .to_string();
    if value.ends_with('.') && value.matches('.').count() > 1 {
        value.pop();
    }
    if let Some((prefix, suffix)) = value.rsplit_once(':') {
        if !prefix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
            value = prefix.to_string();
        }
    }
    if let Some((prefix, suffix)) = value.rsplit_once("#L") {
        if !prefix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
            value = prefix.to_string();
        }
    }
    (!value.is_empty()).then_some(value)
}

fn is_prompt_source_locator(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return false;
    }
    if matches!(
        lower.as_str(),
        "agents.md" | "readme.md" | "package.json" | "cargo.toml" | "pyproject.toml"
    ) {
        return true;
    }
    if token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with('/')
        || token.contains('/')
        || token.contains('\\')
    {
        return true;
    }
    let Some(extension) = Path::new(token).extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "rs" | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "json"
            | "md"
            | "toml"
            | "yaml"
            | "yml"
            | "html"
            | "css"
            | "scss"
            | "kt"
            | "kts"
            | "java"
            | "swift"
            | "go"
            | "py"
            | "rb"
            | "php"
            | "cs"
            | "cpp"
            | "c"
            | "h"
            | "hpp"
            | "sql"
            | "sh"
            | "bash"
            | "zsh"
            | "env"
    )
}

fn collect_text_files(
    candidate: &Path,
    root: &Path,
    limit: usize,
    files: &mut BTreeMap<PathBuf, Vec<String>>,
    reason: String,
) {
    if files.len() >= limit || should_skip_path(candidate, root) {
        return;
    }
    if candidate.is_file() {
        if is_candidate_text_path(candidate) {
            files
                .entry(candidate.to_path_buf())
                .or_default()
                .push(reason);
        }
        return;
    }
    if candidate.is_dir() {
        for entry in WalkDir::new(candidate)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !should_skip_path(entry.path(), root))
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
        {
            if files.len() >= limit {
                break;
            }
            if is_candidate_text_path(entry.path()) {
                files
                    .entry(entry.path().to_path_buf())
                    .or_default()
                    .push(reason.clone());
            }
        }
    }
}

fn split_into_chunks(
    path: &str,
    content: &str,
    prefix: &str,
    counter: &mut usize,
    target_len: usize,
) -> Vec<Value> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut start_line = 1usize;
    let mut line_number = 0usize;
    for line in content.lines() {
        line_number += 1;
        if current.is_empty() {
            start_line = line_number;
        }
        current.push_str(line);
        current.push('\n');
        if current.len() >= target_len || current.len() >= SOURCE_CHUNK_MAX_LENGTH {
            chunks.push(json!({
                "id": format!("{prefix}-{:04}", *counter),
                "path": path,
                "chunkIndex": chunks.len() + 1,
                "startLine": start_line,
                "endLine": line_number,
                "content": current,
            }));
            *counter += 1;
            current = String::new();
        }
    }
    if !current.trim().is_empty() {
        chunks.push(json!({
            "id": format!("{prefix}-{:04}", *counter),
            "path": path,
            "chunkIndex": chunks.len() + 1,
            "startLine": start_line,
            "endLine": line_number,
            "content": current,
        }));
        *counter += 1;
    }
    chunks
}

#[derive(Debug)]
struct ProviderTaskResult {
    stderr: String,
    assistant_text: String,
    stream_events: Vec<Value>,
    errors: Vec<String>,
    session_id: Option<String>,
    token_usage: Option<Value>,
    exit_code: i32,
    summary: String,
}

#[derive(Debug)]
struct ProviderFallbackSelection {
    provider: String,
    model: String,
    reason: String,
}

struct ProviderExecutionAttempt {
    result: Result<ProviderTaskResult>,
    fallback: Option<ProviderFallbackSelection>,
}

async fn ensure_tdd_baseline_for_task(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    run: &mut AgenticBoard,
    task_index: usize,
) -> Result<bool> {
    let Some(task) = run.tasks.get(task_index).cloned() else {
        return Ok(true);
    };
    if !task_requires_tdd(run, &task) {
        if let Some(task) = run.tasks.get_mut(task_index) {
            task.tdd_phase = "disabled".to_string();
        }
        return Ok(true);
    }
    if tdd_baseline_is_ready(&task) {
        let max_fix_attempts = max_tdd_fix_attempts(run);
        if task.fix_attempts >= max_fix_attempts {
            if let Some(task) = run.tasks.get_mut(task_index) {
                task.status = "blocked".to_string();
                task.tdd_phase = "blocked".to_string();
                task.error = Some(format!(
                    "TDD max fix attempts reached ({max_fix_attempts})."
                ));
                task.completed_at = Some(Utc::now());
            }
            run.append_log(format!(
                "Blocked {} after reaching TDD max fix attempts",
                task.id
            ));
            return Ok(false);
        }
        if let Some(task) = run.tasks.get_mut(task_index) {
            task.tdd_phase = "dev_pending".to_string();
        }
        return Ok(true);
    }

    set_phase(
        run,
        "tdd_qa_generation",
        json!({ "taskId": task.id, "taskTitle": task.title }),
    );
    if let Some(task) = run.tasks.get_mut(task_index) {
        task.tdd_phase = "qa_generating".to_string();
        task.transcript.push(json!({
            "timestamp": Utc::now(),
            "kind": "status",
            "status": "qa_generating",
            "content": "Generating failing QA tests before implementation",
        }));
        task.transcript_updated_at = Some(Utc::now());
    }
    attach_rag_context_for_task(run, task_index).await;
    run.touch();
    save_board(state, run)?;
    if let Ok(stored) = load_user_board(state, user_id, board_id)
        && (stored.board.status == "paused"
            || stored.board.pause_requested
            || stored.board.status == "cancelled")
    {
        *run = stored.board;
        return Ok(false);
    }

    let prompt = build_qa_generation_prompt(run, &task, task_index);
    let before_workspace = capture_workspace_snapshot(&run.project_path);
    let output = execute_internal_prompt(
        state,
        user_id,
        board_id,
        &format!("tdd qa generation for {}", task.id),
        &prompt,
    )
    .await;

    if let Ok(stored) = load_user_board(state, user_id, board_id) {
        *run = stored.board;
    }
    let task_index = run
        .tasks
        .iter()
        .position(|candidate| candidate.id == task.id)
        .unwrap_or(task_index);
    let now = Utc::now();
    let mut provider_failure: Option<String> = None;
    let mut malformed_response: Option<String> = None;
    let parsed = match output {
        Ok(text) => match parse_json_object(&text) {
            Some(parsed) => parsed,
            None => {
                let excerpt = limit_text(&text, 1200);
                malformed_response = Some(excerpt.clone());
                json!({
                    "status": "malformed_response",
                    "summary": "QA generation did not return the required JSON contract.",
                    "testFiles": [],
                    "commands": [],
                    "notes": [excerpt],
                })
            }
        },
        Err(error) => {
            let message = server_error_message(&error);
            provider_failure = Some(message.clone());
            json!({
                "status": "provider_failed",
                "summary": message,
                "testFiles": [],
                "commands": [],
                "notes": [],
            })
        }
    };
    let test_files = normalize_string_list(
        parsed
            .get("testFiles")
            .or_else(|| parsed.get("qaTestPaths"))
            .or_else(|| parsed.get("changedFiles")),
    );
    let commands = normalize_string_list(
        parsed
            .get("commands")
            .or_else(|| parsed.get("testCommands"))
            .or_else(|| parsed.get("qaTestCommands")),
    );
    let workspace_delta = record_task_workspace_changes(run, &task.id, before_workspace);
    let allow_without_tests = tdd_allows_implementation_without_tests(run);
    let require_failing_baseline = tdd_requires_failing_baseline(run);
    let commands_empty = commands.is_empty();
    let baseline = if commands.is_empty() {
        json!({
            "stage": "qa_baseline",
            "taskId": task.id,
            "startedAt": now,
            "completedAt": Utc::now(),
            "passed": allow_without_tests,
            "commands": [],
            "blocked": !allow_without_tests,
            "skipped": allow_without_tests,
            "summary": if allow_without_tests {
                "QA generation returned no test commands; TDD policy allows implementation without generated tests."
            } else {
                "QA generation returned no test commands."
            },
        })
    } else {
        run_generated_test_commands(
            &run.project_path,
            &task.id,
            &commands,
            "qa_baseline",
            validation_timeout(run),
        )
        .await
    };
    let qa_generation_done = parsed_status_done(Some(&parsed));
    let baseline_failed = qa_generation_done && validation_has_failure(&baseline);
    let baseline_allowed_without_failure =
        !require_failing_baseline && !commands_empty && qa_generation_done;
    let implementation_allowed_without_tests = commands_empty && allow_without_tests;
    run.validation_runs.push(baseline.clone());
    run.qa_artifacts.push(json!({
        "taskId": task.id,
        "generatedAt": now,
        "testFiles": test_files,
        "commands": commands,
        "baseline": baseline,
        "workspaceDelta": workspace_delta,
        "qaResult": parsed,
    }));

    let task_id_for_log = task.id.clone();
    let outcome = if let Some(task) = run.tasks.get_mut(task_index) {
        task.qa_test_paths = test_files;
        task.qa_test_commands = commands;
        task.qa_baseline_validation = Some(baseline.clone());
        task.coverage_evidence.push(json!({
            "kind": "qa_baseline",
            "validation": baseline,
            "recordedAt": Utc::now(),
        }));
        task.transcript.push(json!({
            "timestamp": Utc::now(),
            "kind": "tdd_qa_result",
            "content": parsed,
        }));
        task.transcript_updated_at = Some(Utc::now());
        if baseline_failed
            || baseline_allowed_without_failure
            || implementation_allowed_without_tests
        {
            task.status = "in_progress".to_string();
            task.tdd_phase = if baseline_failed {
                "qa_failed_expected".to_string()
            } else if implementation_allowed_without_tests {
                "qa_skipped_allowed".to_string()
            } else {
                "qa_baseline_not_required".to_string()
            };
            task.error = None;
            true
        } else {
            task.status = "blocked".to_string();
            task.tdd_phase = "qa_needs_review".to_string();
            task.qa_passed = Some(false);
            task.error = Some(if let Some(message) = provider_failure.as_deref() {
                format!("QA generation provider call failed: {message}")
            } else if let Some(excerpt) = malformed_response.as_deref() {
                format!(
                    "QA generation returned malformed JSON instead of the required test contract: {}",
                    limit_text(excerpt, 500)
                )
            } else if commands_empty {
                "QA generation returned no test commands and TDD policy does not allow implementation without tests."
                    .to_string()
            } else {
                "Generated QA tests did not fail before implementation; tests may be weak or feature already exists."
                    .to_string()
            });
            task.completed_at = Some(Utc::now());
            false
        }
    } else {
        false
    };
    if outcome {
        run.append_log(format!(
            "TDD baseline accepted for {task_id_for_log}; implementation may start"
        ));
    } else if let Some(message) = provider_failure {
        run.append_log(format!(
            "Blocked {task_id_for_log} because QA provider call failed: {}",
            limit_text(&message, 300)
        ));
    } else {
        run.append_log(format!(
            "Blocked {task_id_for_log} because generated QA tests passed before implementation"
        ));
    }
    Ok(outcome)
}

fn task_requires_tdd(run: &AgenticBoard, task: &BoardTask) -> bool {
    run.tdd_enabled
        && !uses_hierarchical_orchestration(run)
        && !is_qa_task(task)
        && !task.agents_knowledge_task
        && !task.internal_validation
        && !task.backlog_generation_task
        && !task.qa_fix_task
        && matches!(task.task_type.as_str(), "implementation" | "feature")
}

fn tdd_baseline_is_ready(task: &BoardTask) -> bool {
    !task.qa_test_commands.is_empty()
        && task
            .qa_baseline_validation
            .as_ref()
            .is_some_and(validation_has_failure)
}

fn validation_has_failure(validation: &Value) -> bool {
    validation
        .get("commands")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|command| command.get("exitCode").and_then(Value::as_i64).unwrap_or(0) != 0)
}

fn max_tdd_fix_attempts(run: &AgenticBoard) -> u32 {
    run.tdd_policy
        .get("maxFixAttempts")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(3)
}

fn tdd_requires_failing_baseline(run: &AgenticBoard) -> bool {
    run.tdd_policy
        .get("requireFailingTestBeforeDev")
        .and_then(value_as_bool)
        .unwrap_or(true)
}

fn tdd_allows_implementation_without_tests(run: &AgenticBoard) -> bool {
    run.tdd_policy
        .get("allowImplementationWithoutTests")
        .and_then(value_as_bool)
        .unwrap_or(false)
}

fn max_followups_per_group(run: &AgenticBoard) -> usize {
    run.qa_policy
        .get("maxFollowupsPerGroup")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(MAX_FOLLOWUP_TASKS_PER_GROUP)
}

fn max_task_attempts(run: &AgenticBoard) -> u32 {
    run.qa_policy
        .get("maxTaskAttempts")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(MAX_TASK_ATTEMPTS)
}

async fn index_project_for_rag(run: &mut AgenticBoard) {
    if !rag_enabled_from_settings(&run.rag_settings) {
        run.rag_enabled = false;
        return;
    }
    if run
        .rag_settings
        .get("indexOnBootstrap")
        .and_then(value_as_bool)
        == Some(false)
    {
        run.rag_enabled = true;
        return;
    }
    let Some(client) = board_rag_client(run) else {
        return;
    };
    let project_id = rag_project_id(run);
    record_rag_trace_ref(run, None, "project_index", &project_id);
    let request = ProjectIndexRequest {
        project_id,
        project_path: run.project_path.clone(),
        run_id: Some(run.id.clone()),
        include_globs: Vec::new(),
        exclude_globs: Vec::new(),
    };
    let record = match client.index_project(&request).await {
        Ok(response) => json!({
            "kind": "project_index",
            "ingestedAt": Utc::now(),
            "ok": true,
            "response": response,
        }),
        Err(error) => json!({
            "kind": "project_index",
            "ingestedAt": Utc::now(),
            "ok": false,
            "error": error_record(error),
        }),
    };
    run.rag_ingestions.push(record);
    trim_rag_history(run);
}

async fn attach_rag_context_for_task(run: &mut AgenticBoard, task_index: usize) {
    let Some(task) = run.tasks.get(task_index).cloned() else {
        return;
    };
    if !run
        .rag_settings
        .get("queryEnabled")
        .and_then(value_as_bool)
        .unwrap_or(true)
    {
        return;
    }
    let Some(client) = board_rag_client(run) else {
        return;
    };
    let phase = rag_phase_for_task(&task);
    let project_id = rag_project_id(run);
    let context_max_chars = rag_context_max_chars(run);
    record_rag_trace_ref(run, Some(&task.id), "query", &project_id);
    let request = RagQueryRequest {
        project_id,
        run_id: run.id.clone(),
        task_id: task.id.clone(),
        phase,
        query: rag_task_query(&task),
        known_files: rag_known_files(&task),
        validation_error: task.deterministic_validation.clone(),
        scopes: rag_scopes(run),
    };
    match client.query(&request).await {
        Ok(response) => {
            let response_value = serde_json::to_value(&response).unwrap_or_else(|_| json!({}));
            run.rag_queries.push(json!({
                "taskId": task.id.clone(),
                "phase": request.phase,
                "queriedAt": Utc::now(),
                "ok": true,
                "contextCount": response.context_refs.len(),
                "response": response_value,
            }));
            if let Some(task) = run.tasks.get_mut(task_index) {
                task.rag_context_refs = json!(response.context_refs)
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                task.rag_prompt_context = limit_text(&response.prompt_context, context_max_chars);
                task.transcript.push(json!({
                    "timestamp": Utc::now(),
                    "kind": "rag_context",
                    "content": format!("Loaded {} RAG context reference(s)", task.rag_context_refs.len()),
                    "contextRefs": task.rag_context_refs.clone(),
                }));
                task.transcript_updated_at = Some(Utc::now());
            }
        }
        Err(error) => {
            let record = error_record(error);
            run.rag_queries.push(json!({
                "taskId": task.id.clone(),
                "phase": request.phase,
                "queriedAt": Utc::now(),
                "ok": false,
                "error": record,
            }));
            run.append_log("RAG context unavailable; continuing without retrieval");
        }
    }
    trim_rag_history(run);
}

async fn ingest_rag_task_outcome(run: &mut AgenticBoard, task_id: &str, parsed: &Value) {
    let ingest_task_results = run
        .rag_settings
        .get("ingestTaskResults")
        .and_then(value_as_bool)
        .unwrap_or(true);
    let ingest_validation_errors = run
        .rag_settings
        .get("ingestValidationErrors")
        .and_then(value_as_bool)
        .unwrap_or(true);
    if !ingest_task_results && !ingest_validation_errors {
        return;
    }
    let Some(client) = board_rag_client(run) else {
        return;
    };
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id).cloned() else {
        return;
    };
    let project_id = rag_project_id(run);
    if ingest_task_results && parsed_status_done(Some(parsed)) {
        record_rag_trace_ref(run, Some(&task.id), "task_result", &project_id);
        let request = TaskResultIngestRequest {
            project_id: project_id.clone(),
            run_id: run.id.clone(),
            task_id: task.id.clone(),
            changed_files: task.changed_files.clone(),
            test_files: task
                .changed_files
                .iter()
                .filter(|path| path.to_lowercase().contains("test"))
                .cloned()
                .collect(),
            commands: task.commands_run.clone(),
            validation: task.deterministic_validation.clone().unwrap_or(Value::Null),
            summary: task.summary.clone(),
        };
        let record = match client.ingest_task_result(&request).await {
            Ok(response) => json!({
                "taskId": task.id.clone(),
                "kind": "task_result",
                "ingestedAt": Utc::now(),
                "ok": true,
                "response": response,
            }),
            Err(error) => json!({
                "taskId": task.id.clone(),
                "kind": "task_result",
                "ingestedAt": Utc::now(),
                "ok": false,
                "error": error_record(error),
            }),
        };
        run.rag_ingestions.push(record);
        run.promotion_candidates.push(json!({
            "scope": "project_specific",
            "projectId": rag_project_id(run),
            "taskId": task.id.clone(),
            "title": task.title.clone(),
            "changedFiles": task.changed_files.clone(),
            "testFiles": task.qa_test_paths.clone(),
            "commands": task.commands_run.clone(),
            "validation": task.deterministic_validation.clone(),
            "summary": task.summary.clone(),
            "recordedAt": Utc::now(),
        }));
    }

    if ingest_validation_errors {
        if let Some(validation) = task.deterministic_validation.as_ref() {
            if validation.get("passed").and_then(Value::as_bool) == Some(false) {
                let (command, exit_code, output) = failed_validation_excerpt(validation);
                record_rag_trace_ref(run, Some(&task.id), "validation_error", &project_id);
                let request = ValidationErrorIngestRequest {
                    project_id: project_id.clone(),
                    run_id: run.id.clone(),
                    task_id: task.id.clone(),
                    phase: "fix".to_string(),
                    command,
                    exit_code,
                    output,
                    validation: validation.clone(),
                };
                let record = match client.ingest_validation_error(&request).await {
                    Ok(response) => json!({
                        "taskId": task.id.clone(),
                        "kind": "validation_error",
                        "ingestedAt": Utc::now(),
                        "ok": true,
                        "response": response,
                    }),
                    Err(error) => json!({
                        "taskId": task.id.clone(),
                        "kind": "validation_error",
                        "ingestedAt": Utc::now(),
                        "ok": false,
                        "error": error_record(error),
                    }),
                };
                run.rag_ingestions.push(record);
            }
        }
    }
    trim_rag_history(run);
}

async fn execute_promotion_review_task(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    run: &mut AgenticBoard,
    task_index: usize,
) -> Result<()> {
    let Some(task) = run.tasks.get(task_index).cloned() else {
        return Ok(());
    };
    let Some(client) = board_rag_client(run) else {
        mark_promotion_review_task(
            run,
            &task.id,
            json!({
                "status": "blocked",
                "summary": "RAG service is unavailable; promotion review skipped.",
                "approvedCandidateIds": [],
            }),
        );
        return Ok(());
    };
    let project_id = rag_project_id(run);
    record_rag_trace_ref(run, Some(&task.id), "promotion_candidates", &project_id);
    let candidates_request = PromotionCandidatesRequest {
        project_id: project_id.clone(),
        limit: 20,
    };
    let candidates_response = match client.promotion_candidates(&candidates_request).await {
        Ok(response) => response,
        Err(error) => {
            let record = error_record(error);
            run.rag_ingestions.push(json!({
                "taskId": task.id,
                "kind": "promotion_candidates",
                "ok": false,
                "ingestedAt": Utc::now(),
                "error": record,
            }));
            mark_promotion_review_task(
                run,
                &task.id,
                json!({
                    "status": "blocked",
                    "summary": "Failed to load promotion candidates.",
                    "approvedCandidateIds": [],
                }),
            );
            return Ok(());
        }
    };
    let candidates = candidates_response
        .get("candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if candidates.is_empty() {
        mark_promotion_review_task(
            run,
            &task.id,
            json!({
                "status": "done",
                "summary": "No RAG promotion candidates available.",
                "approvedCandidateIds": [],
            }),
        );
        return Ok(());
    }

    run.rag_ingestions.push(json!({
        "taskId": task.id,
        "kind": "promotion_candidates",
        "ok": true,
        "ingestedAt": Utc::now(),
        "candidateCount": candidates.len(),
    }));
    run.touch();
    save_board(state, run)?;

    let prompt = build_promotion_review_prompt(run, &candidates);
    let output =
        execute_internal_prompt(state, user_id, board_id, "rag promotion review", &prompt).await;
    if let Ok(stored) = load_user_board(state, user_id, board_id) {
        *run = stored.board;
    }
    let parsed = match output {
        Ok(text) => parse_json_object(&text).unwrap_or_else(|| {
            json!({
                "status": "blocked",
                "summary": "Promotion review did not return JSON.",
                "approvedCandidateIds": [],
                "notes": [limit_text(&text, 1200)],
            })
        }),
        Err(error) => json!({
            "status": "blocked",
            "summary": server_error_message(&error),
            "approvedCandidateIds": [],
        }),
    };
    let approved_ids = normalize_string_list(
        parsed
            .get("approvedCandidateIds")
            .or_else(|| parsed.get("approved_candidate_ids"))
            .or_else(|| parsed.get("candidateIds")),
    );
    let approval_response = if approved_ids.is_empty() {
        json!({
            "promoted": [],
            "reason": "No candidates approved by review gate.",
        })
    } else if let Some(client) = board_rag_client(run) {
        record_rag_trace_ref(run, Some(&task.id), "promotion_approve", &project_id);
        let request = PromotionApproveRequest {
            project_id: project_id.clone(),
            candidate_ids: approved_ids,
            reviewer: "io-workbench-promotion-review".to_string(),
            notes: parsed
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        };
        match client.approve_promotions(&request).await {
            Ok(response) => response,
            Err(error) => json!({
                "promoted": [],
                "error": error_record(error),
            }),
        }
    } else {
        json!({
            "promoted": [],
            "error": "RAG service unavailable during approval.",
        })
    };
    run.rag_ingestions.push(json!({
        "taskId": task.id,
        "kind": "promotion_approval",
        "ok": approval_response.get("error").is_none(),
        "ingestedAt": Utc::now(),
        "review": parsed,
        "response": approval_response,
    }));
    mark_promotion_review_task(run, &task.id, parsed);
    trim_rag_history(run);
    Ok(())
}

fn mark_promotion_review_task(run: &mut AgenticBoard, task_id: &str, result: Value) {
    let status_done = parsed_status_done(Some(&result));
    if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
        task.status = if status_done {
            "completed".to_string()
        } else {
            "blocked".to_string()
        };
        task.summary = result
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("Promotion review completed.")
            .to_string();
        task.result = Some(result.clone());
        task.completed_at = Some(Utc::now());
        task.qa_passed = Some(status_done);
        task.tdd_phase = if status_done {
            "done".to_string()
        } else {
            "blocked".to_string()
        };
        task.transcript.push(json!({
            "timestamp": Utc::now(),
            "kind": "promotion_review",
            "content": result,
        }));
        task.transcript_updated_at = Some(Utc::now());
    }
}

fn board_rag_client(run: &mut AgenticBoard) -> Option<RagClient> {
    if !rag_enabled_from_settings(&run.rag_settings) {
        run.rag_enabled = false;
        run.rag_service_url = RagClient::configured_descriptor();
        return None;
    }
    let Some(client_result) = RagClient::from_env() else {
        run.rag_enabled = false;
        run.rag_service_url = None;
        return None;
    };
    let client = match client_result {
        Ok(client) => client,
        Err(error) => {
            run.rag_enabled = false;
            run.rag_service_url = RagClient::configured_descriptor();
            run.rag_queries.push(json!({
                "queriedAt": Utc::now(),
                "ok": false,
                "error": error_record(error),
            }));
            return None;
        }
    };
    run.rag_enabled = true;
    run.rag_service_url = Some(client.descriptor());
    Some(client)
}

fn rag_scopes(run: &AgenticBoard) -> Vec<String> {
    let scopes = normalize_string_list(run.rag_settings.get("scopes"));
    if scopes.is_empty() {
        normalize_string_list(default_rag_settings().get("scopes"))
    } else {
        scopes
    }
}

fn rag_context_max_chars(run: &AgenticBoard) -> usize {
    run.rag_settings
        .get("contextMaxChars")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(12_000)
        .clamp(1_000, 80_000)
}

fn rag_project_id(run: &AgenticBoard) -> String {
    if run.project_name.trim().is_empty() {
        run.id.clone()
    } else {
        run.project_name.clone()
    }
}

fn rag_phase_for_task(task: &BoardTask) -> String {
    if task.final_qa_task {
        "final".to_string()
    } else if is_qa_task(task) {
        "qa".to_string()
    } else if task.tdd_phase.starts_with("qa") {
        "qa".to_string()
    } else if task.tdd_phase == "fix_pending" {
        "fix".to_string()
    } else if task.status == "failed" || task.qa_fix_task {
        "fix".to_string()
    } else {
        "dev".to_string()
    }
}

fn rag_task_query(task: &BoardTask) -> String {
    let mut parts = vec![
        task.title.clone(),
        task.details.clone(),
        task.prompt.clone(),
    ];
    parts.extend(task.acceptance_criteria.clone());
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn rag_known_files(task: &BoardTask) -> Vec<String> {
    let mut files = task.references.clone();
    files.extend(task.changed_files.clone());
    files.sort();
    files.dedup();
    files
}

fn failed_validation_excerpt(validation: &Value) -> (String, Option<i64>, String) {
    let Some(command) = validation
        .get("commands")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|command| command.get("passed").and_then(Value::as_bool) == Some(false))
    else {
        return (
            String::new(),
            None,
            limit_text(&validation.to_string(), 4_000),
        );
    };
    let command_text = command
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let exit_code = command.get("exitCode").and_then(Value::as_i64);
    let output = command
        .get("output")
        .or_else(|| command.get("stderr"))
        .or_else(|| command.get("stdout"))
        .and_then(Value::as_str)
        .map(|value| limit_text(value, 4_000))
        .unwrap_or_else(|| limit_text(&command.to_string(), 4_000));
    (command_text, exit_code, output)
}

fn record_rag_trace_ref(
    run: &mut AgenticBoard,
    task_id: Option<&str>,
    operation: &str,
    project_id: &str,
) {
    run.rag_trace_refs.push(json!({
        "operation": operation,
        "projectId": project_id,
        "runId": run.id,
        "taskId": task_id,
        "traceparent": rag_traceparent(project_id, Some(&run.id), task_id, operation),
        "recordedAt": Utc::now(),
    }));
}

fn trim_rag_history(run: &mut AgenticBoard) {
    if run.rag_queries.len() > 100 {
        let remove_count = run.rag_queries.len() - 100;
        run.rag_queries.drain(0..remove_count);
    }
    if run.rag_ingestions.len() > 100 {
        let remove_count = run.rag_ingestions.len() - 100;
        run.rag_ingestions.drain(0..remove_count);
    }
    if run.qa_artifacts.len() > 100 {
        let remove_count = run.qa_artifacts.len() - 100;
        run.qa_artifacts.drain(0..remove_count);
    }
    if run.promotion_candidates.len() > 100 {
        let remove_count = run.promotion_candidates.len() - 100;
        run.promotion_candidates.drain(0..remove_count);
    }
    if run.rag_trace_refs.len() > 100 {
        let remove_count = run.rag_trace_refs.len() - 100;
        run.rag_trace_refs.drain(0..remove_count);
    }
}

impl ProviderTaskResult {
    fn from_error(error: ServerError) -> Self {
        let message = server_error_message(&error);
        Self {
            stderr: message.clone(),
            assistant_text: message.clone(),
            stream_events: vec![json!({
                "timestamp": Utc::now(),
                "kind": "error",
                "isError": true,
                "content": message,
            })],
            errors: vec![message.clone()],
            session_id: None,
            token_usage: None,
            exit_code: 1,
            summary: message,
        }
    }
}

#[derive(Debug)]
struct ProviderPromptResult {
    output: String,
    session_id: Option<String>,
    token_usage: Option<Value>,
    effective_model: Option<String>,
}

async fn execute_provider_task(
    state: &AppState,
    run: &AgenticBoard,
    task_index: usize,
) -> Result<ProviderTaskResult> {
    execute_provider_task_with_retry_instruction(state, run, task_index, None).await
}

async fn execute_provider_task_with_retry_instruction(
    state: &AppState,
    run: &AgenticBoard,
    task_index: usize,
    retry_instruction: Option<&str>,
) -> Result<ProviderTaskResult> {
    let task = run
        .tasks
        .get(task_index)
        .ok_or_else(|| not_found("Danger task not found"))?;
    let mut prompt = build_task_execution_prompt(run, task, task_index);
    if let Some(instruction) =
        retry_instruction.filter(|instruction| !instruction.trim().is_empty())
    {
        prompt.push_str("\n\nProvider retry repair:\n");
        prompt.push_str(instruction.trim());
        prompt.push_str("\nReturn the required final JSON only after the task is complete.");
    }
    let provider = normalize_provider(Some(&run.provider))?;
    let model = effective_model_for_task(run, task);
    let execution_model = agentic_execution_model_for_provider(&provider, &model);
    let reusable_session = reusable_session_id(run);
    let session_id = task
        .provider_session_id
        .as_deref()
        .or(reusable_session.as_deref());
    let result = execute_shared_provider_turn(
        state,
        run,
        &provider,
        &execution_model,
        &prompt,
        session_id,
        Some(&task.id),
    )
    .await?;
    let stream_events = shared_provider_stream_events(&provider, &result);
    let errors = if result.exit_code == 0 {
        Vec::new()
    } else {
        vec![result.summary.clone()]
    };
    Ok(ProviderTaskResult {
        summary: result.summary,
        stderr: result.stderr,
        assistant_text: result.assistant_text,
        stream_events,
        errors,
        session_id: Some(result.session_id),
        token_usage: result.token_usage,
        exit_code: result.exit_code,
    })
}

async fn execute_provider_task_with_fallback(
    state: &AppState,
    run: &AgenticBoard,
    task_index: usize,
) -> ProviderExecutionAttempt {
    let mut primary_result = execute_provider_task(state, run, task_index).await;
    let repair_retries = malformed_tool_call_repair_retries(run);
    for attempt in 0..repair_retries {
        if !provider_result_should_attempt_malformed_tool_call_repair(&primary_result) {
            break;
        }
        let reason = provider_result_failure_summary(&primary_result);
        let mut retry_run = run.clone();
        if let Some(session_id) = provider_result_session_id(&primary_result)
            && let Some(task) = retry_run.tasks.get_mut(task_index)
        {
            task.provider_session_id = Some(session_id);
        }
        let instruction = malformed_tool_call_repair_instruction(&reason);
        let mut retry_result = execute_provider_task_with_retry_instruction(
            state,
            &retry_run,
            task_index,
            Some(&instruction),
        )
        .await;
        if let Ok(result) = &mut retry_result {
            result.stream_events.insert(
                0,
                json!({
                    "timestamp": Utc::now(),
                    "kind": "status",
                    "status": "malformed_tool_call_repair",
                    "content": format!(
                        "Retried provider task after malformed integer tool-call arguments ({}/{})",
                        attempt + 1,
                        repair_retries
                    ),
                    "previousFailure": limit_text(&reason, 1200),
                }),
            );
        }
        primary_result = retry_result;
    }
    if !provider_result_requires_fallback(&primary_result) {
        return ProviderExecutionAttempt {
            result: primary_result,
            fallback: None,
        };
    }
    let Some((provider, model)) = configured_provider_fallback(run) else {
        return ProviderExecutionAttempt {
            result: primary_result,
            fallback: None,
        };
    };
    let reason = provider_result_failure_summary(&primary_result);
    let mut fallback_run = run.clone();
    fallback_run.provider = provider.clone();
    fallback_run.model = model.clone();
    fallback_run.actual_session_id = None;
    fallback_run.current_provider_session_id = None;
    if let Some(task) = fallback_run.tasks.get_mut(task_index) {
        task.provider_session_id = None;
    }
    let mut fallback_result = execute_provider_task(state, &fallback_run, task_index).await;
    if let Ok(result) = &mut fallback_result {
        result.stream_events.insert(
            0,
            json!({
                "timestamp": Utc::now(),
                "kind": "status",
                "status": "provider_fallback",
                "content": format!("Primary provider call failed; retried with {provider} {model}"),
                "primaryFailure": reason,
            }),
        );
    }
    ProviderExecutionAttempt {
        result: fallback_result,
        fallback: Some(ProviderFallbackSelection {
            provider,
            model,
            reason,
        }),
    }
}

fn provider_result_session_id(result: &Result<ProviderTaskResult>) -> Option<String> {
    result.as_ref().ok()?.session_id.clone()
}

fn malformed_tool_call_repair_retries(run: &AgenticBoard) -> u64 {
    if run
        .qa_policy
        .get("repairMalformedToolCalls")
        .and_then(value_as_bool)
        .unwrap_or(true)
        == false
    {
        return 0;
    }
    run.qa_policy
        .get("malformedToolCallRepairRetries")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MALFORMED_TOOL_CALL_REPAIR_RETRIES)
        .clamp(0, MAX_MALFORMED_TOOL_CALL_REPAIR_RETRIES)
}

fn provider_result_has_repairable_integer_tool_arg_schema_error(
    result: &Result<ProviderTaskResult>,
) -> bool {
    match result {
        Err(error) => is_repairable_integer_tool_arg_schema_error(&server_error_message(error)),
        Ok(result) => provider_task_result_has_repairable_integer_tool_arg_schema_error(result),
    }
}

fn provider_result_should_attempt_malformed_tool_call_repair(
    result: &Result<ProviderTaskResult>,
) -> bool {
    provider_result_requires_fallback(result)
        && provider_result_has_repairable_integer_tool_arg_schema_error(result)
}

fn provider_task_result_has_repairable_integer_tool_arg_schema_error(
    result: &ProviderTaskResult,
) -> bool {
    let mut parts = vec![
        result.summary.as_str(),
        result.stderr.as_str(),
        result.assistant_text.as_str(),
    ];
    parts.extend(result.errors.iter().map(String::as_str));
    if parts
        .into_iter()
        .any(is_repairable_integer_tool_arg_schema_error)
    {
        return true;
    }
    result.stream_events.iter().any(|event| {
        is_repairable_integer_tool_arg_schema_error(&limit_text(&event.to_string(), 4000))
    })
}

fn malformed_tool_call_repair_instruction(reason: &str) -> String {
    format!(
        "The previous provider attempt failed because a tool call used an integer-valued floating-point JSON number where the tool schema requires an integer. For future tool calls, use strict JSON argument types: integer fields such as session_id, yield_time_ms, max_output_tokens, counts, limits, and offsets must be JSON integers like 60000, never floats like 60000.0. If a prior command session is stale, start a fresh tool call instead of reusing it. Previous failure: {}",
        limit_text(reason, 1200),
    )
}

fn is_repairable_integer_tool_arg_schema_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("failed to parse function arguments")
        && lower.contains("invalid type: floating point")
        && lower.contains("expected")
        && schema_error_expected_integer_type(&lower)
        && text_contains_integer_like_float_literal(text)
}

fn schema_error_expected_integer_type(lower: &str) -> bool {
    [
        "expected i8",
        "expected i16",
        "expected i32",
        "expected i64",
        "expected i128",
        "expected isize",
        "expected u8",
        "expected u16",
        "expected u32",
        "expected u64",
        "expected u128",
        "expected usize",
        "expected integer",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn text_contains_integer_like_float_literal(text: &str) -> bool {
    text.split('`')
        .skip(1)
        .step_by(2)
        .any(is_integer_like_float_literal)
        || text
            .split(|ch: char| !(ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.')))
            .any(is_integer_like_float_literal)
}

fn is_integer_like_float_literal(value: &str) -> bool {
    let value = value
        .trim()
        .trim_matches(|ch: char| matches!(ch, ',' | ';' | ':' | ')' | ']' | '}' | '"'));
    let value = value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value);
    let Some((whole, fraction)) = value.split_once('.') else {
        return false;
    };
    !whole.is_empty()
        && !fraction.is_empty()
        && whole.chars().all(|ch| ch.is_ascii_digit())
        && fraction.chars().all(|ch| ch == '0')
}

fn provider_result_requires_fallback(result: &Result<ProviderTaskResult>) -> bool {
    match result {
        Err(_) => true,
        Ok(result) => {
            result.exit_code != 0
                || !filter_fatal_provider_errors(&result.errors, result.exit_code).is_empty()
        }
    }
}

fn provider_result_failure_summary(result: &Result<ProviderTaskResult>) -> String {
    match result {
        Err(error) => server_error_message(error),
        Ok(result) => result
            .errors
            .first()
            .cloned()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| result.summary.clone()),
    }
}

fn configured_provider_fallback(run: &AgenticBoard) -> Option<(String, String)> {
    let strategy = run.model_strategy.as_ref();
    let provider = trim_string(Some(run.next_provider.clone()))
        .or_else(|| {
            strategy
                .and_then(|value| value.get("fallbackProvider"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| run.provider.clone());
    let model = trim_string(Some(run.next_model.clone()))
        .or_else(|| {
            strategy
                .and_then(|value| value.get("fallbackModel"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| run.model.clone());
    if provider == run.provider && model == run.model {
        return None;
    }
    normalize_optional_provider(Some(&provider))
        .ok()
        .map(|provider| (provider, model))
}

#[derive(Debug)]
struct SharedProviderTurnResult {
    session_id: String,
    assistant_text: String,
    stderr: String,
    token_usage: Option<Value>,
    exit_code: i32,
    summary: String,
}

#[derive(Debug, PartialEq)]
struct BoardProviderControls {
    effort: Option<String>,
    thinking: Option<bool>,
    fast: Option<bool>,
}

fn board_provider_controls(run: &AgenticBoard) -> BoardProviderControls {
    let strategy = run.model_strategy.as_ref();
    let effort = strategy
        .and_then(|value| {
            value
                .get("reasoningEffort")
                .or_else(|| value.get("reasoning_effort"))
                .or_else(|| value.get("effort"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let thinking = strategy.and_then(|value| {
        value
            .get("thinking")
            .or_else(|| value.get("enableThinking"))
            .and_then(Value::as_bool)
    });
    let explicit_fast = strategy.and_then(|value| {
        value
            .get("fast")
            .or_else(|| value.get("fastMode"))
            .or_else(|| value.get("fast_mode"))
            .and_then(value_as_bool)
    });
    let service_tier_fast = strategy
        .and_then(|value| {
            value
                .get("serviceTier")
                .or_else(|| value.get("service_tier"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| value.eq_ignore_ascii_case("fast"));

    BoardProviderControls {
        effort,
        thinking,
        fast: explicit_fast.or(service_tier_fast.then_some(true)),
    }
}

fn value_as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => value.as_u64().and_then(|value| match value {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }),
        Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" | "fast" | "priority" => Some(true),
            "false" | "no" | "off" | "0" | "default" | "standard" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

async fn execute_shared_provider_turn(
    state: &AppState,
    run: &AgenticBoard,
    provider: &str,
    model: &str,
    prompt: &str,
    session_id: Option<&str>,
    board_task_id: Option<&str>,
) -> Result<SharedProviderTurnResult> {
    if provider == "cursor" {
        return execute_cursor_provider_turn(state, run, model, prompt, session_id).await;
    }
    let provider = provider_enum(provider)?;
    let user_id = run.user_id.clone();
    let runtime = user_id
        .as_deref()
        .map(|user_id| agentic_chat_runtime(state, user_id, provider, session_id, Some(model)))
        .unwrap_or(ChatRuntime::NativeCli);
    let model = trim_string(Some(model.to_string()));
    let direct_ai_config = if runtime == ChatRuntime::IoGateway {
        user_id.as_deref().and_then(|user_id| {
            agentic_direct_ai_runtime_config(state, user_id, provider, model.as_deref())
        })
    } else {
        None
    };
    let controls = board_provider_controls(run);
    // Allocate the Workbench id before start so a provider-start failure can
    // still be linked to its persisted board chat.
    let workbench_session_id = session_id
        .map(str::to_string)
        .unwrap_or_else(|| new_id("session"));
    let session = state
        .start_board_agent_session(
            provider,
            run.project_path.clone(),
            prompt.to_string(),
            Some(workbench_session_id.clone()),
            model.clone(),
            controls.effort,
            Some("bypass".to_string()),
            controls.thinking,
            controls.fast,
            runtime,
            direct_ai_config,
            user_id,
            run.id.clone(),
            board_task_id.map(str::to_string),
        )
        .await;
    let session = match session {
        Ok(session) => session,
        Err(error) => {
            if board_task_id.is_some()
                && state
                    .storage
                    .get_session_summary(&workbench_session_id)?
                    .is_some()
            {
                link_board_task_session(
                    state,
                    run,
                    board_task_id.unwrap_or_default(),
                    &workbench_session_id,
                )?;
            }
            return Err(ServerError::from(error));
        }
    };

    // Persist the task link as soon as the Workbench session exists. This
    // makes Open chat available while the provider is still running and also
    // preserves the id when the provider later fails.
    if let Some(task_id) = board_task_id {
        link_board_task_session(state, run, task_id, &session.id)?;
    }

    wait_for_shared_provider_turn(state, run, provider, session, model).await
}

fn link_board_task_session(
    state: &AppState,
    run: &AgenticBoard,
    task_id: &str,
    session_id: &str,
) -> Result<()> {
    let Some(user_id) = run.user_id.as_deref() else {
        return Ok(());
    };
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, &run.id)?;
    if let Some(task) = stored
        .board
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
    {
        task.provider_session_id = Some(session_id.to_string());
        task.transcript_updated_at = Some(Utc::now());
    }
    stored.board.current_provider_session_id = Some(session_id.to_string());
    stored.board.touch();
    save_board(state, &stored.board)
}

#[derive(Debug)]
struct CursorProcessOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
    interrupted: bool,
}

fn cursor_cli_args(
    prompt: &str,
    model: &str,
    session_id: Option<&str>,
    trust_workspace: bool,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) {
        args.push(format!("--resume={session_id}"));
    }
    args.push("-p".to_string());
    args.push(prompt.to_string());
    if session_id.is_none() && !model.trim().is_empty() {
        args.push("--model".to_string());
        args.push(model.trim().to_string());
    }
    args.push("--output-format".to_string());
    args.push("stream-json".to_string());
    args.push("-f".to_string());
    if trust_workspace {
        args.push("--trust".to_string());
    }
    args
}

async fn execute_cursor_provider_turn(
    state: &AppState,
    run: &AgenticBoard,
    model: &str,
    prompt: &str,
    session_id: Option<&str>,
) -> Result<SharedProviderTurnResult> {
    let mut output = run_cursor_cli_process(state, run, model, prompt, session_id, false).await?;
    if !output.interrupted && cursor_workspace_trust_required(&output.stdout, &output.stderr) {
        output = run_cursor_cli_process(state, run, model, prompt, session_id, true).await?;
    }
    Ok(parse_cursor_cli_output(output, session_id))
}

async fn run_cursor_cli_process(
    state: &AppState,
    run: &AgenticBoard,
    model: &str,
    prompt: &str,
    session_id: Option<&str>,
    trust_workspace: bool,
) -> Result<CursorProcessOutput> {
    let mut command = Command::new(CURSOR_CLI_COMMAND);
    command
        .args(cursor_cli_args(prompt, model, session_id, trust_workspace))
        .current_dir(&run.project_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ServerError::with_details(
                StatusCode::SERVICE_UNAVAILABLE,
                "Cursor CLI is not installed",
                format!("{CURSOR_CLI_COMMAND} was not found in PATH"),
            )
        } else {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start Cursor CLI",
                error.to_string(),
            )
        }
    })?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        ServerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Cursor CLI stdout was unavailable",
        )
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        ServerError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Cursor CLI stderr was unavailable",
        )
    })?;
    let stdout_task = tokio::spawn(async move {
        let mut output = String::new();
        stdout.read_to_string(&mut output).await.map(|_| output)
    });
    let stderr_task = tokio::spawn(async move {
        let mut output = String::new();
        stderr.read_to_string(&mut output).await.map(|_| output)
    });

    let (status, interrupted) = loop {
        tokio::select! {
            status = child.wait() => {
                break (status.map_err(|error| ServerError::with_details(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed while waiting for Cursor CLI",
                    error.to_string(),
                ))?, false);
            }
            _ = sleep(PROVIDER_POLL_INTERVAL) => {
                if board_interrupted(state, run) {
                    let _ = child.kill().await;
                    let status = child.wait().await.map_err(|error| ServerError::with_details(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to stop Cursor CLI",
                        error.to_string(),
                    ))?;
                    break (status, true);
                }
            }
        }
    };
    let stdout = stdout_task
        .await
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to collect Cursor CLI stdout",
                error.to_string(),
            )
        })?
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to read Cursor CLI stdout",
                error.to_string(),
            )
        })?;
    let stderr = stderr_task
        .await
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to collect Cursor CLI stderr",
                error.to_string(),
            )
        })?
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to read Cursor CLI stderr",
                error.to_string(),
            )
        })?;
    Ok(CursorProcessOutput {
        stdout: limit_text(&stdout, MAX_PROVIDER_OUTPUT_CHARS),
        stderr: limit_text(&stderr, MAX_PROVIDER_OUTPUT_CHARS),
        exit_code: if interrupted {
            130
        } else {
            status.code().unwrap_or(1)
        },
        interrupted,
    })
}

fn cursor_workspace_trust_required(stdout: &str, stderr: &str) -> bool {
    let text = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    [
        "workspace trust required",
        "do you trust the contents of this directory",
        "working with untrusted contents",
        "pass --trust, --yolo, or -f",
    ]
    .iter()
    .any(|pattern| text.contains(pattern))
}

fn parse_cursor_cli_output(
    output: CursorProcessOutput,
    requested_session_id: Option<&str>,
) -> SharedProviderTurnResult {
    if output.interrupted {
        return SharedProviderTurnResult {
            session_id: requested_session_id
                .map(str::to_string)
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            assistant_text: String::new(),
            stderr: "Provider task was interrupted by board abort.".to_string(),
            token_usage: None,
            exit_code: 130,
            summary: "Provider task was interrupted by board abort.".to_string(),
        };
    }

    let mut session_id = requested_session_id.map(str::to_string);
    let mut assistant_parts = Vec::new();
    let mut result_text = String::new();
    let mut result_error = String::new();
    let mut result_exit_code = None;
    let mut token_usage = None;
    let mut unparsed = Vec::new();
    for line in output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            unparsed.push(line.to_string());
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("system") if event.get("subtype").and_then(Value::as_str) == Some("init") => {
                if let Some(value) = event
                    .get("session_id")
                    .or_else(|| event.get("sessionId"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    session_id = Some(value.to_string());
                }
            }
            Some("assistant") => {
                assistant_parts.extend(cursor_assistant_text(&event));
            }
            Some("result") => {
                result_exit_code = Some(
                    if event.get("subtype").and_then(Value::as_str) == Some("success") {
                        0
                    } else {
                        1
                    },
                );
                result_text = event
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                result_error = event
                    .get("error")
                    .or_else(|| event.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                token_usage = cursor_token_usage(&event);
            }
            _ => {}
        }
    }
    let streamed_assistant_text = assistant_parts.join("");
    let assistant_text = limit_text(
        if result_text.trim().is_empty() {
            &streamed_assistant_text
        } else {
            &result_text
        },
        MAX_PROVIDER_OUTPUT_CHARS,
    );
    let mut stderr_parts = Vec::new();
    if !output.stderr.trim().is_empty() {
        stderr_parts.push(output.stderr.trim().to_string());
    }
    if !result_error.trim().is_empty() {
        stderr_parts.push(result_error.trim().to_string());
    }
    if !unparsed.is_empty() {
        stderr_parts.push(unparsed.join("\n"));
    }
    let stderr = limit_text(&stderr_parts.join("\n"), MAX_PROVIDER_OUTPUT_CHARS);
    let exit_code = result_exit_code.unwrap_or(output.exit_code);
    SharedProviderTurnResult {
        session_id: session_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        summary: summarize_provider_output(&assistant_text, &stderr, exit_code),
        assistant_text,
        stderr,
        token_usage,
        exit_code,
    }
}

fn cursor_assistant_text(event: &Value) -> Vec<String> {
    let Some(content) = event
        .get("message")
        .and_then(|message| message.get("content"))
    else {
        return Vec::new();
    };
    match content {
        Value::String(text) => vec![text.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| item.as_str())
                    .map(str::to_string)
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn cursor_token_usage(event: &Value) -> Option<Value> {
    let usage = event
        .get("usage")
        .or_else(|| event.get("token_usage"))
        .or_else(|| event.get("tokenUsage"))?;
    let input_tokens = json_u64(usage, &["inputTokens", "input_tokens", "input"]);
    let cached_input_tokens = json_u64(
        usage,
        &["cachedInputTokens", "cached_input_tokens", "cached_input"],
    );
    let output_tokens = json_u64(usage, &["outputTokens", "output_tokens", "output"]);
    let total_tokens = json_u64(usage, &["totalTokens", "total_tokens", "total"])
        .unwrap_or(input_tokens.unwrap_or(0) + output_tokens.unwrap_or(0));
    if input_tokens.is_none()
        && cached_input_tokens.is_none()
        && output_tokens.is_none()
        && total_tokens == 0
    {
        return None;
    }
    Some(json!({
        "inputTokens": input_tokens.unwrap_or(0),
        "cachedInputTokens": cached_input_tokens.unwrap_or(0),
        "outputTokens": output_tokens.unwrap_or(0),
        "totalTokens": total_tokens,
    }))
}

fn json_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|entry| {
            entry
                .as_u64()
                .or_else(|| entry.as_i64().and_then(|value| u64::try_from(value).ok()))
                .or_else(|| entry.as_str().and_then(|value| value.parse().ok()))
        })
    })
}

async fn wait_for_shared_provider_turn(
    state: &AppState,
    run: &AgenticBoard,
    provider: Provider,
    session: SessionSummary,
    model: Option<String>,
) -> Result<SharedProviderTurnResult> {
    loop {
        if board_interrupted(state, run) {
            let _ = state.abort_agent_session(provider, &session.id).await;
            return Ok(SharedProviderTurnResult {
                session_id: session.id,
                assistant_text: String::new(),
                stderr: "Provider task was interrupted by board pause/abort.".to_string(),
                token_usage: None,
                exit_code: 130,
                summary: "Provider task was interrupted by board pause/abort.".to_string(),
            });
        }

        let stored_session = state.storage.get_session_summary(&session.id)?;
        let durable_run = state
            .storage
            .latest_durable_chat_run_for_session(&session.id)?;
        let active = stored_session
            .as_ref()
            .is_some_and(|session| session.active);
        let durable_status = durable_run.as_ref().map(|run| run.status.as_str());
        let durable_terminal = durable_status
            .map(|status| !matches!(status, "running" | "recovering"))
            .unwrap_or(false);

        if !active && (durable_run.is_none() || durable_terminal) {
            let messages = state.storage.list_messages(&session.id)?;
            let assistant = messages
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::Assistant);
            let assistant_text = assistant
                .map(|message| limit_text(&message.content, MAX_PROVIDER_OUTPUT_CHARS))
                .unwrap_or_default();
            let token_usage = assistant
                .and_then(|message| message.metadata.get("tokenUsage").cloned())
                .or_else(|| {
                    stored_session
                        .as_ref()
                        .and_then(|session| session.token_usage.as_ref())
                        .and_then(|usage| serde_json::to_value(usage).ok())
                });
            let status = durable_status.unwrap_or(if assistant_text.trim().is_empty() {
                "failed"
            } else {
                "completed"
            });
            let exit_code = if status == "completed" { 0 } else { 1 };
            let mut summary = if exit_code == 0 {
                summarize_provider_output(&assistant_text, "", 0)
            } else {
                durable_run
                    .and_then(|run| run.last_error)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| summarize_provider_output(&assistant_text, "", 1))
            };
            if summary.trim().is_empty()
                && let Some(model) = model.as_deref()
            {
                summary = format!("Shared chat executor completed with model {model}");
            }
            return Ok(SharedProviderTurnResult {
                session_id: session.id,
                assistant_text,
                stderr: if exit_code == 0 {
                    String::new()
                } else {
                    summary.clone()
                },
                token_usage,
                exit_code,
                summary,
            });
        }

        sleep(PROVIDER_POLL_INTERVAL).await;
    }
}

fn board_interrupted(state: &AppState, run: &AgenticBoard) -> bool {
    load_user_board(state, run.user_id.as_deref().unwrap_or_default(), &run.id)
        .map(|stored| board_should_abort_provider(&stored.board))
        .unwrap_or(false)
}

fn board_should_abort_provider(run: &AgenticBoard) -> bool {
    // Abort metadata is retained for auditability. It is not itself an
    // active control signal: older boards can contain a stale canceledAt or
    // cancellationReason after they were resumed. Only the cancelled state
    // may interrupt a provider call.
    run.status == "cancelled"
}

fn board_has_in_flight_work(run: &AgenticBoard) -> bool {
    run.current_provider_session_id.is_some() || run.provider_call_started_at.is_some()
}

fn request_board_pause(run: &mut AgenticBoard, reason: Option<String>) {
    bump_control_revision(run);
    run.auto_run_enabled = false;
    run.pause_reason = reason.or_else(|| Some("user request".to_string()));
    if board_has_in_flight_work(run) {
        run.status = "pausing".to_string();
        run.active = true;
        run.pause_requested = true;
        run.paused_at = None;
        run.append_log("Board pause requested; waiting for current work to finish");
    } else {
        settle_board_pause(run);
        run.append_log("Board paused");
    }
}

fn prepare_board_resume(run: &mut AgenticBoard) {
    bump_control_revision(run);
    clear_board_abort_state(run);
    run.status = "running".to_string();
    run.scheduled_start_at = None;
    run.active = true;
    run.auto_run_enabled = true;
    run.pause_requested = false;
    run.paused_at = None;
    run.pause_reason = None;
    run.append_log("Board resume requested");
}

fn clear_board_abort_state(run: &mut AgenticBoard) {
    run.cancellation_reason = None;
    run.abort_source = None;
    run.abort_requested_at = None;
    run.canceled_at = None;
}

fn settle_board_pause(run: &mut AgenticBoard) {
    if let Some(current_task_id) = run.current_task_id.as_deref()
        && let Some(task) = run.tasks.iter_mut().find(|task| task.id == current_task_id)
        && task_status_is_active(&task.status)
    {
        task.status = TASK_STATUS_TODO.to_string();
        task.started_at = None;
        task.completed_at = None;
        task.provider_session_id = None;
        task.transcript.push(json!({
            "timestamp": Utc::now(),
            "kind": "status",
            "status": TASK_STATUS_TODO,
            "content": "Task returned to Todo because the board paused before provider execution completed",
        }));
        task.transcript_updated_at = Some(Utc::now());
    }
    run.status = "paused".to_string();
    run.active = false;
    run.loop_started = false;
    run.pause_requested = false;
    run.paused_at = Some(Utc::now());
    run.current_task_id = None;
    run.current_task_title.clear();
    run.current_task_status.clear();
    run.append_log("Agentic board execution paused");
}

fn reset_in_flight_board_tasks(run: &mut AgenticBoard, message: &str) {
    for task in &mut run.tasks {
        if !task_status_is_active(&task.status) {
            continue;
        }
        task.status = TASK_STATUS_TODO.to_string();
        task.started_at = None;
        task.completed_at = None;
        task.provider_session_id = None;
        task.transcript.push(json!({
            "timestamp": Utc::now(),
            "kind": "status",
            "status": TASK_STATUS_TODO,
            "content": message,
        }));
        task.transcript_updated_at = Some(Utc::now());
    }
}

fn bump_control_revision(run: &mut AgenticBoard) {
    run.control_revision = run.control_revision.saturating_add(1);
}

fn shared_provider_stream_events(provider: &str, result: &SharedProviderTurnResult) -> Vec<Value> {
    let mut events = Vec::new();
    if !result.assistant_text.trim().is_empty() {
        events.push(json!({
            "timestamp": Utc::now(),
            "provider": provider,
            "kind": "assistant",
            "isError": false,
            "content": result.assistant_text,
        }));
    }
    if result.exit_code != 0 {
        events.push(json!({
            "timestamp": Utc::now(),
            "provider": provider,
            "kind": "error",
            "isError": true,
            "content": result.summary,
        }));
    }
    events
}

fn provider_enum(provider: &str) -> Result<Provider> {
    match provider {
        "claude" => Ok(Provider::Claude),
        "codex" => Ok(Provider::Codex),
        "gemini" => Ok(Provider::Gemini),
        _ => Err(bad_request(
            "Provider must be one of: claude, codex, gemini",
        )),
    }
}

fn agentic_chat_runtime(
    state: &AppState,
    user_id: &str,
    provider: Provider,
    session_id: Option<&str>,
    model: Option<&str>,
) -> ChatRuntime {
    if provider == Provider::Gemini {
        return ChatRuntime::NativeCli;
    }
    if model.is_some_and(agentic_is_io_gateway_model) {
        return ChatRuntime::IoGateway;
    }
    if let Some(session) = session_id
        .and_then(|session_id| state.storage.get_session_summary(session_id).ok().flatten())
    {
        return session.runtime.unwrap_or_else(|| {
            if model
                .or(session.model.as_deref())
                .is_some_and(agentic_is_io_gateway_model)
            {
                ChatRuntime::IoGateway
            } else {
                ChatRuntime::NativeCli
            }
        });
    }
    let key = agentic_user_setting_key(user_id, "direct-ai");
    let config = state.storage.get_setting(&key).ok().flatten();
    config
        .as_ref()
        .and_then(|config| {
            config
                .get("chatRuntime")
                .or_else(|| config.get("chat_runtime"))
        })
        .and_then(Value::as_str)
        .and_then(agentic_parse_chat_runtime)
        .unwrap_or_else(|| {
            let has_legacy_key = config
                .as_ref()
                .is_some_and(|config| agentic_secret_value(config, "gatewayApiKey").is_some())
                || state
                    .storage
                    .get_active_credential_value_by_name(
                        user_id,
                        IO_GATEWAY_API_KEY_CREDENTIAL,
                        IO_GATEWAY_API_KEY_CREDENTIAL_TYPE,
                    )
                    .ok()
                    .flatten()
                    .is_some();
            if has_legacy_key {
                ChatRuntime::IoGateway
            } else {
                ChatRuntime::NativeCli
            }
        })
}

fn agentic_direct_ai_runtime_config(
    state: &AppState,
    user_id: &str,
    provider: Provider,
    model: Option<&str>,
) -> Option<DirectAiRuntimeConfig> {
    if provider == Provider::Claude && model.is_some_and(agentic_model_uses_minimax_gateway) {
        return agentic_minimax_runtime_config(state, user_id);
    }
    let mut config = state
        .storage
        .get_setting(&agentic_user_setting_key(user_id, "direct-ai"))
        .ok()
        .flatten()
        .unwrap_or_else(agentic_default_direct_ai_config);
    if let Some(obj) = config.as_object_mut()
        && let Ok(Some(secret)) = state.storage.get_active_credential_value_by_name(
            user_id,
            IO_GATEWAY_API_KEY_CREDENTIAL,
            IO_GATEWAY_API_KEY_CREDENTIAL_TYPE,
        )
    {
        obj.insert("gatewayApiKey".to_string(), Value::String(secret));
    }
    if matches!(provider, Provider::Codex | Provider::Claude) {
        agentic_apply_io_gateway_config(&mut config, provider);
    }
    let (base_url, api_key) = agentic_direct_ai_endpoint_config(&config)?;
    let max_tokens = config
        .get("maxTokens")
        .or_else(|| config.get("max_tokens"))
        .and_then(Value::as_u64);
    Some(DirectAiRuntimeConfig {
        base_url,
        api_key,
        max_tokens,
    })
}

fn agentic_apply_io_gateway_config(config: &mut Value, provider: Provider) {
    if !config.is_object() {
        *config = agentic_default_direct_ai_config();
    }
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    obj.insert("mode".to_string(), Value::String("aiproxy".to_string()));
    obj.remove("base_url");
    let endpoint = obj
        .get(if provider == Provider::Codex {
            "codexEndpoint"
        } else {
            "claudeEndpoint"
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(if provider == Provider::Codex {
            "codex"
        } else {
            "claude"
        });
    let gateway_root = obj
        .get("gatewayUrl")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .or_else(|| {
            obj.get("baseUrl")
                .and_then(Value::as_str)
                .and_then(|value| {
                    let value = value.trim().trim_end_matches('/');
                    if join_io_gateway_endpoint_url(value, endpoint) == value {
                        Some(value.to_string())
                    } else {
                        agentic_url_origin(value)
                    }
                })
        })
        .unwrap_or_else(|| {
            agentic_url_origin(DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL)
                .unwrap_or_else(|| DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL.to_string())
        });
    let base_url = join_io_gateway_endpoint_url(&gateway_root, endpoint);
    obj.insert("baseUrl".to_string(), Value::String(base_url));
    obj.remove("api_key_env");
    obj.remove("apiKeyEnv");
}

fn agentic_direct_ai_endpoint_config(config: &Value) -> Option<(String, String)> {
    let mode = config.get("mode").and_then(Value::as_str).unwrap_or("off");
    if mode == "off" || mode.is_empty() {
        return None;
    }
    let base_url = config
        .get("baseUrl")
        .or_else(|| config.get("base_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .or_else(|| match mode {
            "direct" | "anthropic" => Some("https://api.anthropic.com".to_string()),
            "minimax" => Some("https://api.minimax.io/anthropic".to_string()),
            "proxy" | "aiproxy" => Some(DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL.to_string()),
            _ => None,
        })?;
    let env_key = config
        .get("apiKeyEnv")
        .or_else(|| config.get("api_key_env"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let stored_gateway_key = agentic_secret_value(config, "gatewayApiKey");
    let api_key = if matches!(mode, "proxy" | "aiproxy") {
        stored_gateway_key
    } else {
        stored_gateway_key.or_else(|| {
            env_key
                .and_then(|key| env::var(key).ok())
                .or_else(|| match mode {
                    "direct" | "anthropic" => env::var("ANTHROPIC_API_KEY")
                        .or_else(|_| env::var("ANTHROPIC_AUTH_TOKEN"))
                        .ok(),
                    "minimax" => env::var("MINIMAX_API_KEY")
                        .or_else(|_| env::var("ANTHROPIC_API_KEY"))
                        .ok(),
                    _ => None,
                })
        })
    }
    .filter(|value| !value.trim().is_empty())?;
    Some((base_url, api_key))
}

fn agentic_model_uses_minimax_gateway(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase().replace('_', "-");
    normalized.starts_with("min:") || matches!(normalized.as_str(), "minimax-m3" | "minimaxm3")
}

fn agentic_minimax_runtime_config(
    state: &AppState,
    user_id: &str,
) -> Option<DirectAiRuntimeConfig> {
    let settings = state
        .storage
        .get_setting(&agentic_user_setting_key(user_id, "claude-settings"))
        .ok()
        .flatten()
        .unwrap_or_else(agentic_default_claude_agent_settings);
    let base_url = settings
        .get("minimaxBaseUrl")
        .or_else(|| settings.get("minimax_base_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("https://api.minimax.io/anthropic")
        .trim_end_matches('/')
        .to_string();
    let key_env = settings
        .get("minimaxApiKeyEnv")
        .or_else(|| settings.get("minimax_api_key_env"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("MINIMAX_API_KEY");
    let api_key = agentic_secret_value(&settings, "minimaxApiKey")
        .or_else(|| agentic_secret_value(&settings, "minimax_api_key"))
        .or_else(|| env::var(key_env).ok())
        .or_else(|| env::var("MINIMAX_API_KEY").ok())
        .filter(|value| !value.trim().is_empty())?;
    Some(DirectAiRuntimeConfig {
        base_url,
        api_key,
        max_tokens: None,
    })
}

fn agentic_default_claude_agent_settings() -> Value {
    json!({
        "minimaxBaseUrl": "https://api.minimax.io/anthropic",
        "minimaxApiKeyEnv": "MINIMAX_API_KEY",
        "minimaxModel": "MiniMax-M3",
    })
}

fn agentic_parse_chat_runtime(value: &str) -> Option<ChatRuntime> {
    match value.trim().to_ascii_lowercase().as_str() {
        "native_cli" | "native" | "cli" | "default" => Some(ChatRuntime::NativeCli),
        "io_gateway" | "gateway" | "custom_api" | "aiproxy" => Some(ChatRuntime::IoGateway),
        _ => None,
    }
}

fn agentic_is_io_gateway_model(model: &str) -> bool {
    let trimmed = model.trim();
    let Some((prefix, rest)) = trimmed.split_once(':') else {
        return false;
    };
    let normalized = prefix.to_ascii_lowercase();
    !rest.trim().is_empty()
        && matches!(
            normalized.as_str(),
            "agw"
                | "cod"
                | "proxy"
                | "gateway"
                | "aiproxy"
                | "cld"
                | "gem"
                | "cop"
                | "ctm"
                | "dsk"
                | "glm"
                | "grk"
                | "min"
        )
}

fn agentic_secret_value(config: &Value, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn agentic_url_origin(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let scheme_end = trimmed.find("://")?;
    let after_scheme = &trimmed[scheme_end + 3..];
    let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());
    Some(
        trimmed[..scheme_end + 3 + path_start]
            .trim_end_matches('/')
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

fn agentic_user_setting_key(user_id: &str, key: &str) -> String {
    format!("user:{user_id}:{key}")
}

fn agentic_default_direct_ai_config() -> Value {
    json!({
        "mode": "off",
        "chatRuntime": "native_cli",
        "baseUrl": null,
        "apiKeyEnv": null,
        "model": null
    })
}

fn build_task_execution_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    if task.final_qa_task {
        return build_final_qa_prompt(run, task, index);
    }
    if rag_phase_for_task(task) == "fix" {
        return build_fix_prompt(run, task, index);
    }
    if canonical_task_kind(task) == TASK_KIND_MANUAL_TEST {
        return build_manual_test_prompt(run, task, index);
    }
    if matches!(
        canonical_task_kind(task),
        TASK_KIND_QA | TASK_KIND_REVIEW | TASK_KIND_RESEARCH | TASK_KIND_DESIGN
    ) {
        return build_validation_prompt(run, task, index);
    }
    if canonical_task_kind(task) == TASK_KIND_TEST_IMPLEMENTATION {
        return build_test_implementation_prompt(run, task, index);
    }
    build_dev_prompt(run, task, index)
}

fn task_scope_block(_run: &AgenticBoard, task: &BoardTask) -> String {
    let acceptance = if task.acceptance_criteria.is_empty() {
        "None recorded.".to_string()
    } else {
        task.acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "Ticket scope (source of truth):\nDescription:\n{}\nAcceptance criteria:\n{}",
        if task.description.trim().is_empty() {
            &task.details
        } else {
            &task.description
        },
        acceptance,
    )
}

fn build_dev_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    build_execution_prompt_with_mode(
        run,
        task,
        index,
        "Dev",
        "Implement the smallest production change that satisfies the generated tests and acceptance criteria.",
    )
}

fn build_fix_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    build_execution_prompt_with_mode(
        run,
        task,
        index,
        "Fix",
        "Fix the latest validation failure first; preserve generated tests and use the failure logs as the repair target.",
    )
}

fn build_final_qa_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    build_execution_prompt_with_mode(
        run,
        task,
        index,
        "Final QA",
        "Validate the ticket description and acceptance criteria against current files, generated-test evidence, and deterministic validation history before returning done.",
    )
}

fn build_manual_test_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    let acceptance = if task.acceptance_criteria.is_empty() {
        "- Verify the card exactly as described.".to_string()
    } else {
        task.acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let references = if task.references.is_empty() {
        "None".to_string()
    } else {
        task.references.join("\n")
    };
    let scope = task_scope_block(run, task);
    let result_schema = {
        r#"{
  "status": "done" | "blocked" | "needs_followup",
  "summary": "short manual verification summary",
  "changedFiles": [],
  "commandsRun": ["commands/checks actually run"],
  "qaResult": "pass" | "fail" | "blocked" | "not_run",
  "evidence": ["specific observed app behavior, command output, logs, or file inspection evidence"],
  "manualTestEnvironment": {
    "deviceOrEmulator": "device, emulator, or simulator identifier",
    "appVersion": "app/build version under test",
    "backendUrl": "backend URL, or none/local-only when no backend is used",
    "osVersion": "optional OS version"
  },
  "manualTestSteps": ["each manual step followed by the observed outcome"],
  "manualTestResult": "explicit observed pass/result for the manual verification",
  "externalSideEffects": ["what declared external state changed, or explicitly state that it did not change"],
  "remainingIssues": [],
  "remainingGaps": [],
  "suggestedBacklogTasks": [
    {
      "title": "one concrete engineering fix",
      "kind": "implementation",
      "level": "story",
      "details": "code-changing fix needed because verification found a defect",
      "acceptanceCriteria": ["verifiable outcome"],
      "priority": "p0|p1|p2|p3"
    }
  ]
}"#
    };
    format!(
        r#"You are running a manual verification task for an io-workbench Kanban board.

Prompt template: Manual Test
Project: {project_name}
Project path: {project_path}
Board id: {board_id}
{board_profile_block}

{git_policy_block}

Task {task_number}: {task_id}
Title: {title}
Priority: {priority}

Details:
{details}

Prompt:
{prompt}

Acceptance criteria:
{acceptance}

References:
{references}

{scope}

Applicable AGENTS.md advisory guidance:
{agents_context}

RAG context:
{rag_context}

Codebase reconnaissance:
{codebase}

Workspace baseline:
{workspace_baseline}

Completed implementation task summaries:
{completed_tasks}

Instructions:
- Treat this as verification/manual testing work, not an implementation task.
- Do not edit source, production, generated, test, config, lock, or documentation files for this card.
- Run read-only inspection, build, test, app, emulator, browser, or log commands when practical to verify the described workflow.
- If the workflow is already correct, return status "done" with concrete evidence.
- Always record manualTestEnvironment with the device/emulator, app version, and backend URL used. Use "none/local-only" when the app has no backend.
- Always record manualTestSteps as the ordered manual actions and their observed outcomes, and manualTestResult as the explicit overall result. A failed or blocked manual check must not be reported as done.
- If a product defect or missing implementation is found, return status "needs_followup" and add code-changing fixes to "suggestedBacklogTasks" with kind "implementation".
- If verification cannot run because of environment, credentials, dependency, or tool failure, return status "blocked" with exact blocker evidence.
- Never run git commit, git push, create tags, or otherwise change git history.
- Do not ask for user confirmation.
Return JSON only, with this schema:
{result_schema}
"#,
        project_name = run.project_name,
        project_path = run.project_path,
        board_id = run.id,
        board_profile_block = board_profile_block(run),
        git_policy_block = git_policy_block(run),
        task_number = index + 1,
        task_id = task.id,
        title = task.title,
        priority = task.priority,
        details = if task.details.trim().is_empty() {
            &task.description
        } else {
            &task.details
        },
        prompt = task.prompt,
        acceptance = acceptance,
        references = references,
        scope = scope,
        result_schema = result_schema,
        agents_context = serde_json::to_string_pretty(&run.agents_context).unwrap_or_default(),
        rag_context = if task.rag_prompt_context.trim().is_empty() {
            "None".to_string()
        } else {
            task.rag_prompt_context.clone()
        },
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
        workspace_baseline =
            serde_json::to_string_pretty(&run.workspace_baseline).unwrap_or_default(),
        completed_tasks = completed_task_summary(run),
    )
}

fn build_validation_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    let kind = canonical_task_kind(task);
    let acceptance = if task.acceptance_criteria.is_empty() {
        "- Verify the card exactly as described.".to_string()
    } else {
        task.acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let scope = task_scope_block(run, task);
    format!(
        r#"You are executing a read-only {kind} subtask for an io-workbench Kanban board.

Project: {project_name}
Project path: {project_path}
Board id: {board_id}
Task {task_number}: {task_id}
Title: {title}

Description:
{details}

Acceptance criteria:
{acceptance}

{scope}

Codebase reconnaissance:
{codebase}

Instructions:
- This is one {kind} purpose only. Do not implement production behavior.
- Do not edit source, production, generated, test, config, lock, or documentation files.
- Use read-only inspection and run the smallest relevant checks, tests, emulator, browser, or log commands.
- If this is research, do not implement the recommendation. Put proposed initiative, epic, story, or task work in proposedPlanningItems; it remains Backlog-only until the user accepts it.
- For a defect, report the exact observed failure and return `needs_followup`; the board will create a separate concrete fix subtask under the same parent.
- Return JSON only, with no markdown fence:
{{
  "status": "done" | "blocked" | "needs_followup",
  "summary": "short validation summary",
  "changedFiles": [],
  "commandsRun": ["commands actually run"],
  "qaResult": "pass" | "fail" | "blocked" | "not_run",
  "evidence": ["specific observed evidence"],
  "externalSideEffects": ["what declared external state changed, or explicitly state that it did not change"],
  "remainingIssues": ["exact defects or blockers"],
  "remainingGaps": [],
  "proposedPlanningItems": [
    {{
      "level": "initiative|epic|story|task",
      "title": "proposed planning item",
      "kind": "implementation|design|research",
      "details": "scope that still needs user approval",
      "acceptanceCriteria": ["verifiable outcome"],
      "priority": "p0|p1|p2|p3"
    }}
  ]
}}"#,
        project_name = run.project_name,
        project_path = run.project_path,
        board_id = run.id,
        task_number = index + 1,
        task_id = task.id,
        title = task.title,
        details = if task.details.trim().is_empty() {
            &task.description
        } else {
            &task.details
        },
        acceptance = acceptance,
        scope = scope,
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
    )
}

fn build_test_implementation_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    build_execution_prompt_with_mode(
        run,
        task,
        index,
        "Test Implementation",
        "Add or update only automated test files and the smallest test fixtures required to verify this subtask. Do not edit production behavior.",
    )
}

fn build_execution_prompt_with_mode(
    run: &AgenticBoard,
    task: &BoardTask,
    index: usize,
    prompt_mode: &str,
    phase_instruction: &str,
) -> String {
    let acceptance = if task.acceptance_criteria.is_empty() {
        "- Complete the card exactly as described.".to_string()
    } else {
        task.acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let references = if task.references.is_empty() {
        "None".to_string()
    } else {
        task.references.join("\n")
    };
    let tdd_context = if task.qa_test_commands.is_empty() && task.qa_test_paths.is_empty() {
        "No generated TDD tests are attached to this task.".to_string()
    } else {
        format!(
            "TDD phase: {}\nGenerated test files:\n{}\nGenerated test commands:\n{}\nBaseline validation:\n{}\nLatest deterministic validation:\n{}",
            task.tdd_phase,
            if task.qa_test_paths.is_empty() {
                "None".to_string()
            } else {
                task.qa_test_paths.join("\n")
            },
            if task.qa_test_commands.is_empty() {
                "None".to_string()
            } else {
                task.qa_test_commands.join("\n")
            },
            serde_json::to_string_pretty(&task.qa_baseline_validation).unwrap_or_default(),
            serde_json::to_string_pretty(&task.deterministic_validation).unwrap_or_default(),
        )
    };
    let scope = task_scope_block(run, task);
    let result_schema = {
        r#"{
  "status": "done" | "blocked" | "needs_followup",
  "summary": "short summary",
  "changedFiles": ["files changed"],
  "commandsRun": ["commands/checks actually run"],
  "qaResult": "pass" | "fail" | "blocked" | "not_run",
  "evidence": ["specific file, command, or behavior evidence"],
  "externalSideEffects": ["what declared external state changed, or explicitly state that it did not change"],
  "remainingIssues": [],
  "remainingGaps": [],
  "suggestedBacklogTasks": []
}"#
    };
    format!(
        r#"You are running an autonomous implementation queue in danger mode for an io-workbench Kanban board.

Prompt template: {prompt_mode}
Project: {project_name}
Project path: {project_path}
Board id: {board_id}
{board_profile_block}

{git_policy_block}

Task {task_number}: {task_id}
Title: {title}
Priority: {priority}

Details:
{details}

Prompt:
{prompt}

Acceptance criteria:
{acceptance}

External side effects:
{side_effects}

References:
{references}

{scope}

Applicable AGENTS.md advisory guidance:
{agents_context}

RAG context:
{rag_context}

TDD generated tests:
{tdd_context}

Codebase reconnaissance:
{codebase}

Workspace baseline:
{workspace_baseline}

Completed implementation task summaries:
{completed_tasks}

Instructions:
- Work directly in the project directory.
- Inspect current files before editing; do not rebuild already completed work.
- {phase_instruction}
- Make minimal changes needed to complete this task correctly.
- Implement only this Kanban subtask and its ticket scope. Do not implement workspace user experience.
- Do not weaken, skip, delete, or rewrite generated TDD tests except to fix broken test syntax or align with existing test harness conventions.
- Run focused local checks when practical.
- When calling tools, use strict JSON argument types. Integer fields such as session_id, yield_time_ms, max_output_tokens, counts, limits, and offsets must be JSON integers like 60000, not floats like 60000.0.
- Never run git commit, git push, create tags, or otherwise change git history.
- Do not ask for user confirmation.
Return JSON only, with this schema:
{result_schema}
"#,
        project_name = run.project_name,
        project_path = run.project_path,
        board_id = run.id,
        board_profile_block = board_profile_block(run),
        git_policy_block = git_policy_block(run),
        task_number = index + 1,
        task_id = task.id,
        title = task.title,
        priority = task.priority,
        details = if task.details.trim().is_empty() {
            &task.description
        } else {
            &task.details
        },
        prompt = task.prompt,
        acceptance = acceptance,
        side_effects = if task.hierarchy.side_effects.is_empty() {
            "None declared. If the task discovers an external side effect, stop and report it instead of silently proceeding.".to_string()
        } else {
            task.hierarchy.side_effects.join("\n")
        },
        references = references,
        scope = scope,
        result_schema = result_schema,
        agents_context = serde_json::to_string_pretty(&run.agents_context).unwrap_or_default(),
        rag_context = if task.rag_prompt_context.trim().is_empty() {
            "None".to_string()
        } else {
            task.rag_prompt_context.clone()
        },
        tdd_context = tdd_context,
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
        workspace_baseline =
            serde_json::to_string_pretty(&run.workspace_baseline).unwrap_or_default(),
        completed_tasks = completed_task_summary(run),
        prompt_mode = prompt_mode,
        phase_instruction = phase_instruction,
    )
}

fn build_qa_generation_prompt(run: &AgenticBoard, task: &BoardTask, index: usize) -> String {
    let acceptance = if task.acceptance_criteria.is_empty() {
        "- Complete the card exactly as described.".to_string()
    } else {
        task.acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let scope = task_scope_block(run, task);
    let result_schema = {
        r#"{
  "status": "done" | "blocked",
  "summary": "short QA summary",
  "testFiles": ["test file paths created or updated"],
  "commands": ["focused commands to run these generated tests"],
  "notes": []
}"#
    };
    format!(
        r#"You are the QA phase of a TDD-first io-workbench Kanban board worker.

Goal: create failing tests before implementation. Do not implement the feature.

Project: {project_name}
Project path: {project_path}
Board id: {board_id}
{board_profile_block}

{git_policy_block}

Task {task_number}: {task_id}
Title: {title}

Details:
{details}

Acceptance criteria:
{acceptance}

{scope}

RAG context:
{rag_context}

Codebase reconnaissance:
{codebase}

Rules:
- Inspect current test conventions before adding tests.
- Create or update only test files and small fixtures required by tests.
- Do not implement production behavior to make tests pass.
- Generated tests must fail against the current baseline for the expected missing behavior.
- Return JSON only. No markdown fence.

Schema:
{result_schema}"#,
        project_name = run.project_name,
        project_path = run.project_path,
        board_id = run.id,
        board_profile_block = board_profile_block(run),
        git_policy_block = git_policy_block(run),
        task_number = index + 1,
        task_id = task.id,
        title = task.title,
        details = if task.details.trim().is_empty() {
            &task.description
        } else {
            &task.details
        },
        acceptance = acceptance,
        scope = scope,
        result_schema = result_schema,
        rag_context = if task.rag_prompt_context.trim().is_empty() {
            "None"
        } else {
            &task.rag_prompt_context
        },
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
    )
}

fn summarize_provider_output(stdout: &str, stderr: &str, exit_code: i32) -> String {
    let source = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    let mut summary = source
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string();
    if summary.len() > 500 {
        summary.truncate(497);
        summary.push_str("...");
    }
    if summary.is_empty() {
        summary = format!("Provider exited with code {exit_code}");
    }
    summary
}

fn build_promotion_review_prompt(run: &AgenticBoard, candidates: &[Value]) -> String {
    format!(
        r#"You are reviewing RAG promotion candidates for io-workbench.

Project: {project_name}
Board id: {board_id}

Candidates:
{candidates}

Rules:
- Approve only reusable, generalizable implementation, testing, validation, or architecture standards.
- Reject project secrets, credentials, personal data, one-off file paths, speculative claims, and brittle implementation details.
- Prefer approving fewer candidates when uncertain.
- Do not edit files.
- Return JSON only:
{{
  "status": "done" | "blocked",
  "summary": "short review summary",
  "approvedCandidateIds": ["candidate ids safe to promote"],
  "rejectedCandidateIds": ["candidate ids rejected"],
  "notes": ["brief reason for approvals or rejections"]
}}"#,
        project_name = run.project_name,
        board_id = run.id,
        candidates = serde_json::to_string_pretty(candidates).unwrap_or_default(),
    )
}

fn build_codebase_recon_prompt(run: &AgenticBoard, local_snapshot: &Value) -> String {
    format!(
        r#"You are performing read-only codebase reconnaissance before Kanban planning.

User request:
{prompt}

{board_profile_block}

{git_policy_block}

Local static snapshot:
{snapshot}

Return JSON only. No markdown fence.
Schema:
{{
  "summary": "short architecture summary",
  "architecture": ["important modules, runtime boundaries, data flow, framework facts"],
  "implementedCapabilities": ["requested capabilities that appear already implemented"],
  "missingCapabilities": ["requested capabilities that appear missing or partial"],
  "conventions": ["coding, testing, routing, styling, data, or migration conventions to follow"],
  "runCommands": ["commands for running the app locally"],
  "testCommands": ["commands for focused verification"],
  "relevantFiles": ["files/directories future tasks should inspect first"],
  "risks": ["important risks or external dependencies"]
}}

Rules:
- Inspect only. Do not edit files.
- Prefer concrete files, scripts, and conventions over generic advice.
- Treat summaries as navigation hints; task executions still inspect files before editing."#,
        prompt = active_board_prompt(run),
        board_profile_block = board_profile_block(run),
        git_policy_block = git_policy_block(run),
        snapshot = serde_json::to_string_pretty(local_snapshot).unwrap_or_default(),
    )
}

fn completed_task_summary(run: &AgenticBoard) -> String {
    run.tasks
        .iter()
        .filter(|task| task_is_done(task))
        .map(|task| {
            format!(
                "{}: {}\nEvidence: {}",
                task.id,
                task.summary,
                task.evidence.join("; ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn task_result_summary(run: &AgenticBoard) -> String {
    run.tasks
        .iter()
        .map(|task| {
            format!(
                "{} [{}] {}\nEvidence: {}\nRemaining: {}",
                task.id,
                task.status,
                task.summary,
                task.evidence.join("; "),
                task.remaining_issues.join("; ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn task_from_json(
    run: &AgenticBoard,
    item: Value,
    index: usize,
    status: &str,
) -> Option<BoardTask> {
    let title = item.get("title").and_then(Value::as_str)?.trim();
    if title.is_empty() {
        return None;
    }
    let details = item
        .get("details")
        .or_else(|| item.get("description"))
        .and_then(Value::as_str)
        .unwrap_or(title)
        .trim()
        .to_string();
    let acceptance_criteria = normalize_string_list(item.get("acceptanceCriteria"));
    let task_type = prompt_task_kind_from_value(&item, title, &details, &acceptance_criteria);
    let parent_id = item
        .get("parentId")
        .or_else(|| item.get("parent_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let requested_level = normalize_task_level(
        item.get("level").and_then(Value::as_str),
        if parent_id.is_some() {
            TASK_LEVEL_SUBTASK
        } else {
            TASK_LEVEL_STORY
        },
    );
    let level = parent_id
        .as_deref()
        .and_then(|parent_id| {
            run.tasks
                .iter()
                .find(|candidate| candidate.id == parent_id)
                .and_then(|parent| next_hierarchy_level(task_level(parent)))
        })
        .unwrap_or_else(|| match requested_level {
            TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK => TASK_LEVEL_STORY,
            level => level,
        });
    let source_task_id = item
        .get("sourceTaskId")
        .or_else(|| item.get("source_task_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let source_qa_task_id = item
        .get("sourceQaTaskId")
        .or_else(|| item.get("source_qa_task_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let depends_on =
        normalize_string_list(item.get("dependsOn").or_else(|| item.get("dependencies")));
    let blocked_by =
        normalize_string_list(item.get("blockedBy").or_else(|| item.get("blocked_by")))
            .into_iter()
            .chain(depends_on.iter().cloned())
            .collect::<Vec<_>>();
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("task-{}", index + 1));
    let priority = task_priority_for_parent(
        run,
        parent_id.as_deref(),
        item.get("priority").and_then(Value::as_str),
    );
    Some(BoardTask {
        id: id.clone(),
        title: title.to_string(),
        status: status.to_string(),
        summary: String::new(),
        details: details.clone(),
        description: details.clone(),
        prompt: details,
        error: None,
        acceptance_criteria,
        references: normalize_string_list(item.get("references")),
        priority,
        depends_on,
        manual_task: false,
        prompt_task: false,
        task_origin: "planned".to_string(),
        task_type: task_type.to_string(),
        backlog_generation_task: false,
        qa_task: false,
        final_qa_task: false,
        followup_task: false,
        qa_fix_task: false,
        qa_verdict_retry_task: false,
        task_level_qa: false,
        agents_knowledge_task: false,
        internal_validation: false,
        qa_round: 0,
        source_task_id,
        source_qa_task_id,
        superseded_by: None,
        transcript: Vec::new(),
        transcript_updated_at: None,
        started_at: None,
        completed_at: None,
        qa_passed: None,
        attempt_count: 0,
        provider_session_id: None,
        commands_run: Vec::new(),
        changed_files: Vec::new(),
        changed_file_summary: None,
        evidence: Vec::new(),
        remaining_issues: Vec::new(),
        result: None,
        result_validation: None,
        deterministic_validation: None,
        rag_context_refs: Vec::new(),
        rag_prompt_context: String::new(),
        tdd_phase: default_tdd_phase(),
        qa_test_paths: Vec::new(),
        qa_test_commands: Vec::new(),
        qa_baseline_validation: None,
        fix_attempts: 0,
        coverage_evidence: Vec::new(),
        group_id: Some(id),
        hierarchy: BoardTaskHierarchy {
            level: level.to_string(),
            parent_id,
            blocked_by,
            executable: item
                .get("executable")
                .and_then(Value::as_bool)
                .unwrap_or(level == TASK_LEVEL_SUBTASK)
                && level == TASK_LEVEL_SUBTASK,
            required: item
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            scope_version: item
                .get("scopeVersion")
                .and_then(Value::as_u64)
                .unwrap_or(1),
            rank: item.get("rank").and_then(Value::as_i64).unwrap_or(0),
            attempts: Vec::new(),
            planned_files: normalize_string_list(
                item.get("plannedFiles").or_else(|| item.get("files")),
            ),
            side_effects: normalize_string_list(item.get("sideEffects")),
            side_effects_approved: false,
            side_effect_approval: None,
            side_effect_evidence: Vec::new(),
            manual_test_environment: None,
            research_accepted: false,
            research_acceptance: None,
            discussion: Vec::new(),
        },
    })
}

fn parse_execution_result(stdout: &str) -> Option<Value> {
    parse_json_object(stdout)
        .or_else(|| {
            stdout
                .lines()
                .rev()
                .find_map(|line| parse_json_object(line.trim()))
        })
        .map(mark_parsed_json_result)
}

fn resolved_execution_summary(parsed: &Value, provider_summary: &str) -> String {
    parsed
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .unwrap_or_else(|| provider_summary.trim())
        .to_string()
}

fn missing_json_task_result(output: &str) -> Value {
    json!({
        "status": "needs_followup",
        "summary": "Provider completed without returning the required task result JSON.",
        "parsedJson": false,
        "changedFiles": [],
        "commandsRun": [],
        "qaResult": "blocked",
        "evidence": [limit_text(output, 1200)],
        "remainingIssues": ["The task result was not machine-readable. The next attempt must return the required JSON contract."],
        "remainingGaps": ["Missing strict task result JSON."],
        "suggestedBacklogTasks": [],
    })
}

fn mark_parsed_json_result(mut parsed: Value) -> Value {
    if let Some(object) = parsed.as_object_mut() {
        object.insert("parsedJson".to_string(), json!(true));
    }
    parsed
}

fn should_treat_provider_errors_as_followup(
    result: &ProviderTaskResult,
    parsed: &Value,
    change_summary: &Value,
) -> bool {
    result.exit_code == 0
        && change_summary
            .get("touchedFileCount")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0
        && parsed.get("parsedJson").and_then(Value::as_bool) != Some(true)
        && !result.errors.is_empty()
}

fn convert_missing_json_provider_error_to_followup(
    parsed: &Value,
    result: &ProviderTaskResult,
) -> Value {
    let mut remaining = vec![
        "Review the current workspace changes, finish any missing task work, and return the required final JSON contract."
            .to_string(),
    ];
    remaining.extend(
        result
            .errors
            .iter()
            .map(|error| format!("Provider reported: {}", limit_text(error, 300))),
    );
    remaining.extend(normalize_string_list(parsed.get("remainingIssues")));
    let mut next = parsed.clone();
    if let Some(object) = next.as_object_mut() {
        object.insert("status".to_string(), json!("needs_followup"));
        object.insert(
            "summary".to_string(),
            parsed
                .get("summary")
                .and_then(Value::as_str)
                .filter(|summary| !summary.trim().is_empty())
                .map(|summary| json!(summary))
                .unwrap_or_else(|| {
                    json!("Task made workspace changes but did not return the required final JSON.")
                }),
        );
        object.insert("remainingIssues".to_string(), json!(remaining));
    }
    next
}

fn is_recoverable_self_reported_blocker(parsed: &Value) -> bool {
    let status = parsed.get("status").and_then(Value::as_str).unwrap_or("");
    if !matches!(status, "blocked" | "needs_followup") {
        return false;
    }
    let text = execution_result_text(parsed).to_lowercase();
    recoverable_blocker_match(&text) || tool_environment_blocker_match(&text)
}

fn is_tool_environment_self_reported_blocker(parsed: &Value) -> bool {
    let status = parsed.get("status").and_then(Value::as_str).unwrap_or("");
    if !matches!(status, "blocked" | "needs_followup") {
        return false;
    }
    tool_environment_blocker_match(&execution_result_text(parsed).to_lowercase())
}

fn execution_result_text(parsed: &Value) -> String {
    let mut parts = Vec::new();
    for key in ["status", "summary", "qaResult"] {
        if let Some(text) = parsed.get(key).and_then(Value::as_str) {
            parts.push(text.to_string());
        }
    }
    for key in ["evidence", "remainingIssues", "remainingGaps"] {
        parts.extend(normalize_string_list(parsed.get(key)));
    }
    parts.join("\n")
}

fn recoverable_blocker_match(text: &str) -> bool {
    (text.contains("json") && (text.contains("quote") || text.contains("valid json")))
        || text.contains("quote mangl")
        || text.contains("unmatched \"")
        || text.contains("unmatched '")
        || text.contains("exec_command")
        || text.contains("apply_patch")
        || text.contains("cannot write")
        || text.contains("could not write")
        || text.contains("failed to write")
        || text.contains("no cargo.toml")
        || (text.contains("workspace") && (text.contains("not built") || text.contains("missing")))
        || (text.contains("no crate") && text.contains("compile"))
        || (text.contains("no ") && text.contains("source files"))
        || (text.contains("dependencies") && text.contains("unverified"))
        || (text.contains("cannot") && text.contains("honestly") && text.contains("verif"))
        || (text.contains("postgres") && text.contains("not reachable"))
        || (text.contains("no postgres") && text.contains("reachable"))
        || text.contains("sqlx migrate")
}

fn tool_environment_blocker_match(text: &str) -> bool {
    text.contains("tool result missing due to internal error")
        || (text.contains("tool environment") && text.contains("fail"))
        || (text.contains("every") && text.contains("tool") && text.contains("internal error"))
        || (text.contains("every") && text.contains("command") && text.contains("internal error"))
        || (text.contains("every") && text.contains("shell") && text.contains("internal error"))
        || (text.contains("every") && text.contains("cat") && text.contains("internal error"))
        || text.contains("no inspection, edits, or verification ran")
        || text.contains("no implementation or qa evidence was added")
}

fn provider_events_have_tool_evidence(events: &[Value]) -> bool {
    events.iter().any(|event| {
        matches!(
            event.get("kind").and_then(Value::as_str),
            Some("tool_use" | "tool_result" | "tool")
        ) || event.get("toolName").is_some()
            || event.get("toolInput").is_some()
            || event.get("toolResult").is_some()
    })
}

fn reset_provider_session(run: &mut AgenticBoard, reason: &str) {
    let had_session = run.actual_session_id.is_some() || run.current_provider_session_id.is_some();
    run.actual_session_id = None;
    run.current_provider_session_id = None;
    run.provider_call_started_at = None;
    run.provider_call_label = None;
    if had_session {
        run.append_log(format!("Started a fresh provider session: {reason}"));
    }
}

fn filter_fatal_provider_errors(errors: &[String], exit_code: i32) -> Vec<String> {
    let normalized = errors
        .iter()
        .map(|error| error.trim())
        .filter(|error| !error.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if exit_code != 0 {
        return normalized;
    }
    normalized
        .into_iter()
        .filter(|error| !is_non_fatal_provider_error(error))
        .collect()
}

fn is_non_fatal_provider_error(message: &str) -> bool {
    let text = message.to_lowercase();
    (text.contains("long threads") && text.contains("multiple compactions"))
        || (text.contains("start a new thread") && text.contains("threads small"))
        || (text.contains("model metadata for")
            && text.contains("not found")
            && text.contains("fallback metadata"))
}

async fn repair_task_result_if_needed(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    task_id: &str,
    task_index: usize,
    assistant_output: &str,
    parsed: Value,
    change_summary: &Value,
) -> Value {
    let Ok(stored) = load_user_board(state, user_id, board_id) else {
        return parsed;
    };
    let Some(task) = stored.board.tasks.iter().find(|task| task.id == task_id) else {
        return parsed;
    };
    let issues = strict_result_schema_issues(&stored.board, task, &parsed, change_summary);
    if issues.is_empty() {
        return parsed;
    }
    let prompt = build_task_result_repair_prompt(
        &stored.board,
        task,
        task_index,
        &parsed,
        assistant_output,
        change_summary,
        &issues,
    );
    let repaired = execute_internal_prompt(
        state,
        user_id,
        board_id,
        &format!("result schema repair for {task_id}"),
        &prompt,
    )
    .await
    .ok()
    .and_then(|text| parse_json_object(&text));

    let mut stored = match load_user_board(state, user_id, board_id) {
        Ok(stored) => stored,
        Err(_) => return repaired.unwrap_or(parsed),
    };
    if let Some(index) = stored
        .board
        .tasks
        .iter()
        .position(|task| task.id == task_id)
    {
        stored.board.tasks[index].result_validation = Some(json!({
            "schemaIssues": issues,
            "repairAttemptedAt": Utc::now(),
            "repaired": repaired.is_some(),
        }));
        stored.board.append_log(format!(
            "Result schema repair {} for {task_id}",
            if repaired.is_some() {
                "succeeded"
            } else {
                "failed"
            }
        ));
        stored.board.touch();
        let _ = save_board(state, &stored.board);
    }
    repaired.unwrap_or(parsed)
}

fn should_repair_task_result(
    run: &AgenticBoard,
    task_id: &str,
    parsed: &Value,
    change_summary: &Value,
) -> bool {
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id) else {
        return false;
    };
    !strict_result_schema_issues(run, task, parsed, change_summary).is_empty()
}

fn strict_result_schema_issues(
    _run: &AgenticBoard,
    _task: &BoardTask,
    parsed: &Value,
    change_summary: &Value,
) -> Vec<String> {
    let mut issues = Vec::new();
    if !parsed.is_object() {
        issues.push("Result is not a JSON object.".to_string());
        return issues;
    }
    let status = parsed.get("status").and_then(Value::as_str).unwrap_or("");
    if !matches!(
        status,
        "done" | "blocked" | "needs_followup" | "completed" | "success"
    ) {
        issues.push("Missing or invalid status.".to_string());
    }
    if parsed
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        issues.push("Missing summary.".to_string());
    }
    if !matches!(
        parsed.get("qaResult").and_then(Value::as_str),
        Some("pass" | "fail" | "blocked" | "not_run")
    ) {
        issues.push("Missing or invalid qaResult.".to_string());
    }
    if parsed_status_done(Some(parsed)) {
        let changed_files = normalize_string_list(parsed.get("changedFiles"));
        let commands = normalize_string_list(parsed.get("commandsRun"));
        let evidence = normalize_string_list(parsed.get("evidence"));
        let attributable_count = change_summary_attributable_file_count(change_summary);
        let touched_count = change_summary
            .get("touchedFileCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let file_evidence = if uses_hierarchical_orchestration(_run) {
            attributable_count > 0
        } else {
            touched_count > 0 || !changed_files.is_empty()
        };
        if !file_evidence && commands.is_empty() && evidence.is_empty() {
            issues.push("Done result lacks changed files, commands, and evidence.".to_string());
        }
    }
    issues
}

fn build_task_result_repair_prompt(
    run: &AgenticBoard,
    task: &BoardTask,
    index: usize,
    parsed: &Value,
    assistant_output: &str,
    change_summary: &Value,
    issues: &[String],
) -> String {
    let scope = task_scope_block(run, task);
    let result_schema = {
        r#"{
  "status": "done" | "blocked" | "needs_followup",
  "summary": "short summary",
  "changedFiles": ["files changed or inspected as already correct"],
  "commandsRun": ["commands/checks actually shown in previous output"],
  "qaResult": "pass" | "fail" | "blocked" | "not_run",
  "evidence": ["specific evidence from previous output or workspace delta"],
  "remainingIssues": [],
  "remainingGaps": [],
  "suggestedBacklogTasks": []
}"#
    };
    format!(
        r#"Repair the previous agentic Kanban task result into the required JSON contract.

This is a reporting repair only.
- Do not edit files.
- Do not rerun implementation.
- Do not claim verification that is not present in the previous output, task transcript, ticket evidence, or workspace delta.
- If the previous output does not contain enough evidence to honestly mark the task done, return status "needs_followup".
- Return JSON only. No markdown fence.

User request:
{request}

Task {number} of {total}: {title}
Details:
{details}

Ticket scope:
{scope}

Schema/evidence issues:
{issues}

Workspace delta:
{delta}

Previous parsed result:
{parsed}

Previous assistant output:
{output}

Required schema:
{result_schema}"#,
        request = active_board_prompt(run),
        number = index + 1,
        total = run.tasks.len(),
        title = task.title,
        details = task.details,
        scope = scope,
        result_schema = result_schema,
        issues = issues
            .iter()
            .map(|issue| format!("- {issue}"))
            .collect::<Vec<_>>()
            .join("\n"),
        delta = serde_json::to_string_pretty(change_summary).unwrap_or_default(),
        parsed = serde_json::to_string_pretty(parsed).unwrap_or_default(),
        output = limit_text(assistant_output, 10_000),
    )
}

fn reusable_session_id(run: &AgenticBoard) -> Option<String> {
    run.actual_session_id
        .clone()
        .or_else(|| run.session_id.clone())
        .filter(|value| !value.trim().is_empty())
}

fn reusable_session_id_for_provider(run: &AgenticBoard, provider: &str) -> Option<String> {
    if provider == run.provider {
        reusable_session_id(run)
    } else {
        None
    }
}

fn board_task_id_for_label(run: &AgenticBoard, label: &str) -> Option<String> {
    let label = label.trim();
    run.tasks
        .iter()
        .find(|task| {
            label == task.id
                || label.ends_with(&format!(" for {}", task.id))
                || label.contains(&format!(" {} ", task.id))
        })
        .map(|task| task.id.clone())
        .or_else(|| run.current_task_id.clone())
}

fn should_resume_provider_session(run: &AgenticBoard) -> bool {
    matches!(
        normalize_session_policy(Some(&run.session_policy)).as_str(),
        "continuous"
    ) && run.provider == "claude"
}

fn uses_hierarchical_orchestration(run: &AgenticBoard) -> bool {
    run.orchestration_version >= 3
}

fn parse_json_object(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if value.is_object() {
            return Some(value);
        }
    }
    if let Some(start) = trimmed.find("```") {
        let rest = &trimmed[start + 3..];
        let rest = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("JSON"))
            .unwrap_or(rest);
        if let Some(end) = rest.find("```") {
            if let Some(value) = parse_json_object(&rest[..end]) {
                return Some(value);
            }
        }
    }
    let start = trimmed.find('{')?;
    let end = find_matching_json_brace(trimmed, start)?;
    serde_json::from_str::<Value>(&trimmed[start..=end])
        .ok()
        .filter(Value::is_object)
}

fn find_matching_json_brace(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn parsed_status_done(parsed: Option<&Value>) -> bool {
    parsed
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        .map(|status| matches!(status, "done" | "completed" | "success"))
        .unwrap_or(true)
}

fn completion_evidence_gate_failed(parsed: &Value) -> bool {
    parsed
        .get("evidenceGate")
        .and_then(|value| value.get("passed"))
        .and_then(Value::as_bool)
        == Some(false)
        || parsed
            .get("completionEvidenceGateFailed")
            .and_then(Value::as_bool)
            == Some(true)
}

fn apply_deterministic_validation_result(mut parsed: Value, validation: &Value) -> Value {
    let mut evidence = normalize_string_list(parsed.get("evidence"));
    evidence.push(format_validation_check(validation));
    let mut commands = normalize_string_list(parsed.get("commandsRun"));
    if let Some(items) = validation.get("commands").and_then(Value::as_array) {
        for item in items {
            if let Some(command) = item.get("command").and_then(Value::as_str) {
                commands.push(command.to_string());
            }
        }
    }
    if let Some(object) = parsed.as_object_mut() {
        object.insert("evidence".to_string(), json!(dedupe_strings(evidence)));
        object.insert("commandsRun".to_string(), json!(dedupe_strings(commands)));
        if validation.get("passed").and_then(Value::as_bool) == Some(false) {
            let mut issues = object
                .get("remainingIssues")
                .map(|value| normalize_string_list(Some(value)))
                .unwrap_or_default();
            for command in validation
                .get("commands")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if command.get("exitCode").and_then(Value::as_i64).unwrap_or(0) != 0 {
                    issues.push(format!(
                        "Deterministic validation failed: {} exited {}{}",
                        command
                            .get("command")
                            .and_then(Value::as_str)
                            .unwrap_or("command"),
                        command.get("exitCode").and_then(Value::as_i64).unwrap_or(1),
                        command
                            .get("output")
                            .and_then(Value::as_str)
                            .filter(|output| !output.trim().is_empty())
                            .map(|output| format!(": {}", limit_text(output, 700)))
                            .unwrap_or_default()
                    ));
                }
            }
            object.insert("status".to_string(), json!("needs_followup"));
            object.insert("qaResult".to_string(), json!("fail"));
            object.insert("remainingIssues".to_string(), json!(dedupe_strings(issues)));
        }
    }
    parsed
}

fn change_summary_touched_paths(summary: &Value) -> Vec<String> {
    summary
        .get("touchedFiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| match entry {
            Value::String(path) => Some(path.trim().to_string()),
            Value::Object(object) => object
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .map(str::to_string),
            _ => None,
        })
        .filter(|path| !path.is_empty())
        .collect()
}

fn is_test_or_fixture_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let components = normalized
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    let file_name = components.last().copied().unwrap_or_default();
    components.iter().any(|component| {
        matches!(
            *component,
            "test"
                | "tests"
                | "testing"
                | "fixtures"
                | "testfixtures"
                | "androidtest"
                | "commontest"
                | "jvmtetest"
                | "__tests__"
        ) || component.ends_with("test")
    }) || file_name.starts_with("test_")
        || file_name.ends_with("_test.rs")
        || file_name.ends_with("_test.kt")
        || file_name.ends_with("_test.kts")
        || file_name.contains(".test.")
        || file_name.contains(".spec.")
        || file_name.ends_with(".snap")
}

fn task_file_edit_policy_violations(task: &BoardTask, change_summary: &Value) -> Vec<String> {
    let touched = change_summary_touched_paths(change_summary);
    match canonical_task_kind(task) {
        TASK_KIND_QA
        | TASK_KIND_MANUAL_TEST
        | TASK_KIND_REVIEW
        | TASK_KIND_RESEARCH
        | TASK_KIND_DESIGN => touched,
        TASK_KIND_TEST_IMPLEMENTATION => touched
            .into_iter()
            .filter(|path| !is_test_or_fixture_path(path))
            .collect(),
        _ => Vec::new(),
    }
}

fn apply_completion_evidence_gate(
    run: &AgenticBoard,
    task_id: &str,
    mut parsed: Value,
    change_summary: &Value,
) -> Value {
    if !parsed_status_done(Some(&parsed)) {
        return parsed;
    }
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id) else {
        return parsed;
    };
    if uses_hierarchical_orchestration(run) {
        let commands = normalize_string_list(parsed.get("commandsRun"));
        let evidence = normalize_string_list(parsed.get("evidence"));
        let external_effect_evidence = external_side_effect_evidence(&parsed);
        let attributable_file_count = change_summary_attributable_file_count(change_summary);
        let kind = canonical_task_kind(task);
        let manual_environment = manual_test_environment_evidence(&parsed);
        let manual_steps = manual_test_steps_evidence(&parsed);
        let manual_result = manual_test_result_evidence(&parsed);
        let file_policy_violations = task_file_edit_policy_violations(task, change_summary);
        let file_policy_valid = file_policy_violations.is_empty();
        let has_code_evidence = attributable_file_count > 0;
        let has_validation_evidence = !commands.is_empty() || !evidence.is_empty();
        let manual_evidence_valid = kind != TASK_KIND_MANUAL_TEST
            || (!manual_steps.is_empty()
                && manual_result
                    .as_deref()
                    .is_some_and(manual_test_result_is_successful));
        let kind_evidence_valid = match kind {
            TASK_KIND_IMPLEMENTATION
            | TASK_KIND_TEST_IMPLEMENTATION
            | TASK_KIND_FIX
            | TASK_KIND_MIGRATION
            | TASK_KIND_REVERT
            | TASK_KIND_CLEANUP
            | TASK_KIND_REVISION
            | TASK_KIND_REPLACEMENT => has_code_evidence,
            TASK_KIND_RESEARCH | TASK_KIND_DESIGN | TASK_KIND_QA | TASK_KIND_REVIEW => {
                has_validation_evidence
            }
            TASK_KIND_MANUAL_TEST => manual_evidence_valid,
            _ => has_code_evidence || has_validation_evidence,
        };
        let external_effect_evidence_valid =
            task.hierarchy.side_effects.is_empty() || !external_effect_evidence.is_empty();
        let manual_environment_valid = kind != TASK_KIND_MANUAL_TEST
            || manual_test_environment_is_complete(&manual_environment);
        let valid = kind_evidence_valid
            && external_effect_evidence_valid
            && manual_environment_valid
            && manual_evidence_valid
            && file_policy_valid;
        if let Some(object) = parsed.as_object_mut() {
            if kind == TASK_KIND_MANUAL_TEST {
                object.insert("manualTestSteps".to_string(), json!(manual_steps.clone()));
                object.insert(
                    "manualTestResult".to_string(),
                    manual_result
                        .clone()
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                );
                if !manual_environment.is_null() {
                    object.insert(
                        "manualTestEnvironment".to_string(),
                        manual_environment.clone(),
                    );
                }
            }
            object.insert(
                "filePolicy".to_string(),
                json!({
                    "passed": file_policy_valid,
                    "violations": file_policy_violations.clone(),
                }),
            );
            if !valid {
                let mut issues = object
                    .get("remainingIssues")
                    .map(|value| normalize_string_list(Some(value)))
                    .unwrap_or_default();
                issues.push(format!(
                    "Completion evidence gate failed for {kind}: provide the evidence required by the subtask kind."
                ));
                if !external_effect_evidence_valid {
                    issues.push(
                            "Declared external side effects require explicit externalSideEffects evidence describing what changed or was not changed."
                                .to_string(),
                    );
                }
                if !manual_environment_valid {
                    issues.push(
                        "Manual-test completion requires manualTestEnvironment with deviceOrEmulator, appVersion, and backendUrl."
                            .to_string(),
                    );
                }
                if !manual_evidence_valid {
                    if manual_steps.is_empty() {
                        issues.push(
                            "Manual-test completion requires manualTestSteps with each observed step."
                                .to_string(),
                        );
                    }
                    match manual_result.as_deref() {
                        None => issues.push(
                            "Manual-test completion requires manualTestResult with the overall observed result."
                                .to_string(),
                        ),
                        Some(_) => issues.push(
                            "Manual-test completion cannot be done when manualTestResult reports a failure or blocked check."
                                .to_string(),
                        ),
                    }
                }
                if !file_policy_valid {
                    issues.push(format!(
                        "File-edit policy failed for {kind}: this subtask kind cannot modify these Git-visible files: {}.",
                        file_policy_violations.join(", ")
                    ));
                }
                let issues = dedupe_strings(issues);
                object.insert(
                    "evidenceGate".to_string(),
                    json!({"passed": false, "kind": kind, "issues": issues.clone()}),
                );
                object.insert("status".to_string(), json!("needs_followup"));
                object.insert("qaResult".to_string(), json!("blocked"));
                object.insert("remainingIssues".to_string(), json!(issues));
            } else {
                object.insert(
                    "evidenceGate".to_string(),
                    json!({"passed": true, "kind": kind}),
                );
            }
        }
        if !valid {
            return parsed;
        }
        return parsed;
    }
    if matches!(
        task.task_type.as_str(),
        "qa" | "test" | "validation" | "final_qa"
    ) {
        return parsed;
    }
    let changed_files = normalize_string_list(parsed.get("changedFiles"));
    let commands = normalize_string_list(parsed.get("commandsRun"));
    let evidence = normalize_string_list(parsed.get("evidence"));
    let touched_count = change_summary
        .get("touchedFileCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if touched_count > 0 || !changed_files.is_empty() || !commands.is_empty() {
        return parsed;
    }
    if evidence
        .iter()
        .any(|item| item.len() > 12 && !item.to_lowercase().contains("not run"))
    {
        return parsed;
    }
    if let Some(object) = parsed.as_object_mut() {
        let mut issues = object
            .get("remainingIssues")
            .map(|value| normalize_string_list(Some(value)))
            .unwrap_or_default();
        issues.push(
            "Completion evidence gate failed: no changed files, commands, or concrete evidence were reported."
                .to_string(),
        );
        object.insert("status".to_string(), json!("needs_followup"));
        object.insert("qaResult".to_string(), json!("blocked"));
        object.insert("remainingIssues".to_string(), json!(dedupe_strings(issues)));
    }
    parsed
}

fn parsed_qa_passed(parsed: Option<&Value>) -> bool {
    !matches!(
        parsed
            .and_then(|value| value.get("qaResult"))
            .and_then(Value::as_str),
        Some("fail" | "blocked")
    )
}

fn apply_task_result_to_board(board: &mut AgenticBoard, task_id: &str, parsed: &Value) {
    for file in normalize_string_list(parsed.get("changedFiles")) {
        board.change_ledger.push(json!({
            "taskId": task_id,
            "path": file,
            "reportedAt": Utc::now(),
        }));
    }
    for command in normalize_string_list(parsed.get("commandsRun")) {
        let passed = !matches!(
            parsed.get("qaResult").and_then(Value::as_str),
            Some("fail" | "blocked")
        );
        board.validation_runs.push(json!({
            "taskId": task_id,
            "command": command,
            "passed": passed,
            "completedAt": Utc::now(),
        }));
    }
}

fn record_task_workspace_changes(run: &mut AgenticBoard, task_id: &str, before: Value) -> Value {
    let after = capture_workspace_snapshot(&run.project_path);
    let summary = summarize_workspace_delta(task_id, &before, &after);
    if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
        task.changed_file_summary = Some(summary.clone());
        let paths = change_summary_paths(&summary);
        task.changed_files = dedupe_strings(
            task.changed_files
                .clone()
                .into_iter()
                .chain(paths)
                .collect(),
        );
    }
    run.latest_workspace_snapshot = Some(after);
    run.change_ledger.push(summary.clone());
    if run.git_policy == "managed" {
        run.git_ledger.push(json!({
            "taskId": task_id,
            "policy": "managed",
            "branch": git_command_text(&run.project_path, &["branch", "--show-current"]),
            "shortStat": summary.get("shortStat").and_then(Value::as_str).unwrap_or(""),
            "touchedFiles": summary.get("touchedFiles").cloned().unwrap_or_else(|| json!([])),
            "recordedAt": Utc::now(),
            "historyMutation": false,
        }));
    }
    summary
}

fn finish_task_attempt(
    run: &mut AgenticBoard,
    task_id: &str,
    attempt_id: &str,
    status: &str,
    finished_at: DateTime<Utc>,
) {
    let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) else {
        return;
    };
    let Some(attempt_index) =
        task.hierarchy.attempts.iter().rposition(|attempt| {
            attempt.get("attemptId").and_then(Value::as_str) == Some(attempt_id)
        })
    else {
        return;
    };
    let started_at = task.hierarchy.attempts[attempt_index]
        .get("startedAt")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_utc);
    let transcript_start_index = task.hierarchy.attempts[attempt_index]
        .get("transcriptStartIndex")
        .and_then(Value::as_u64)
        .map(|index| index as usize)
        .unwrap_or(0)
        .min(task.transcript.len());
    let attempt_transcript = task
        .transcript
        .iter()
        .skip(transcript_start_index)
        .cloned()
        .collect::<Vec<_>>();
    let attempt_commands = task.commands_run.clone();
    let attempt_files = task.changed_files.clone();
    let attempt_evidence = task.evidence.clone();
    let attempt_side_effect_evidence = task.hierarchy.side_effect_evidence.clone();
    let attempt_environment = task.hierarchy.manual_test_environment.clone();
    let attempt_summary = task.summary.clone();
    let attempt_error = task.error.clone();
    let attempt = &mut task.hierarchy.attempts[attempt_index];
    if let Some(object) = attempt.as_object_mut() {
        object.insert("status".to_string(), json!(status));
        object.insert("finishedAt".to_string(), json!(finished_at));
        object.insert(
            "durationMs".to_string(),
            started_at
                .map(|started| (finished_at - started).num_milliseconds().max(0))
                .map(|duration| json!(duration))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "transcript".to_string(),
            sanitize_kanban_value(&json!(attempt_transcript)),
        );
        object.insert("commands".to_string(), json!(attempt_commands));
        object.insert("filesChanged".to_string(), json!(attempt_files));
        object.insert("evidence".to_string(), json!(attempt_evidence));
        object.insert(
            "externalSideEffects".to_string(),
            json!(attempt_side_effect_evidence),
        );
        object.insert(
            "manualTestEnvironment".to_string(),
            attempt_environment.unwrap_or(Value::Null),
        );
        object.insert("summary".to_string(), json!(attempt_summary));
        object.insert("error".to_string(), json!(attempt_error));
        object.remove("transcriptStartIndex");
    }
}

async fn ensure_managed_git_branch_for_task_group(
    run: &mut AgenticBoard,
    task_id: &str,
) -> Result<()> {
    if run.git_policy != "managed" {
        return Ok(());
    }
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id).cloned() else {
        return Ok(());
    };
    if !is_managed_git_group_root(&task) {
        return Ok(());
    }
    let group_id = task.group_id.clone().unwrap_or_else(|| task.id.clone());
    if let Some(entry) = managed_git_entry(run, &group_id) {
        if matches!(
            entry.get("status").and_then(Value::as_str),
            Some("running" | "completed")
        ) {
            return Ok(());
        }
    }
    let entry = json!({
        "groupId": group_id,
        "taskId": task.id,
        "taskTitle": task.title,
        "policy": "managed",
        "status": "running",
        "baseBranch": "",
        "branchName": "",
        "startedAt": Utc::now(),
        "completedAt": null,
        "error": "",
        "commands": [],
    });
    run.git_ledger.push(entry);

    let inside = run_managed_git_ledger_command(
        run,
        &group_id,
        &["rev-parse", "--is-inside-work-tree"],
        MANAGED_GIT_TIMEOUT,
    )
    .await?;
    if inside.stdout.trim() != "true" {
        mark_managed_git_failed(run, &group_id, "Project is not inside a git worktree.");
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Project is not inside a git worktree.",
        ));
    }
    let dirty =
        run_managed_git_ledger_command(run, &group_id, &["status", "--short"], MANAGED_GIT_TIMEOUT)
            .await?;
    if !dirty.stdout.trim().is_empty() {
        let message = format!(
            "Managed git requires a clean worktree before creating a task branch. Current git status:\n{}",
            dirty.stdout.trim()
        );
        mark_managed_git_failed(run, &group_id, &message);
        return Err(ServerError::new(StatusCode::CONFLICT, message));
    }
    let base_branch = current_git_branch(&run.project_path);
    if base_branch.is_empty() {
        mark_managed_git_failed(run, &group_id, "Could not determine current git branch.");
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Could not determine current git branch.",
        ));
    }
    set_managed_git_entry_field(run, &group_id, "baseBranch", json!(base_branch));
    run_managed_git_ledger_command(
        run,
        &group_id,
        &["switch", &base_branch],
        MANAGED_GIT_TIMEOUT,
    )
    .await?;
    let branch_name = build_managed_git_branch_name(&task, &group_id);
    set_managed_git_entry_field(run, &group_id, "branchName", json!(branch_name));
    run_managed_git_ledger_command(
        run,
        &group_id,
        &["switch", "-c", &branch_name],
        MANAGED_GIT_TIMEOUT,
    )
    .await?;
    run.append_log(format!(
        "Managed git created branch {branch_name} from {base_branch} for {group_id}"
    ));
    Ok(())
}

async fn finalize_managed_git_task_group(run: &mut AgenticBoard, task_id: &str) -> Result<()> {
    if run.git_policy != "managed" {
        return Ok(());
    }
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id).cloned() else {
        return Ok(());
    };
    if !task_is_done(&task) {
        return Ok(());
    }
    let group_id = task.group_id.clone().unwrap_or_else(|| task.id.clone());
    if task_group_has_unfinished_work(run, &group_id) {
        return Ok(());
    }
    let Some(entry) = managed_git_entry(run, &group_id).cloned() else {
        return Ok(());
    };
    if entry.get("status").and_then(Value::as_str) != Some("running") {
        return Ok(());
    }
    let branch_name = entry
        .get("branchName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let base_branch = entry
        .get("baseBranch")
        .and_then(Value::as_str)
        .unwrap_or("main")
        .to_string();
    let current_branch = current_git_branch(&run.project_path);
    if current_branch != branch_name {
        let message = format!(
            "Expected managed branch {branch_name}, but current branch is {}.",
            if current_branch.is_empty() {
                "[detached]"
            } else {
                current_branch.as_str()
            }
        );
        mark_managed_git_failed(run, &group_id, &message);
        return Err(ServerError::new(StatusCode::CONFLICT, message));
    }
    set_managed_git_entry_field(run, &group_id, "finalizingAt", json!(Utc::now()));
    let status =
        run_managed_git_ledger_command(run, &group_id, &["status", "--short"], MANAGED_GIT_TIMEOUT)
            .await?;
    if status.stdout.trim().is_empty() {
        set_managed_git_entry_field(
            run,
            &group_id,
            "noCommitReason",
            json!("No workspace changes to commit."),
        );
    } else {
        run_managed_git_ledger_command(run, &group_id, &["add", "-A"], MANAGED_GIT_TIMEOUT).await?;
        let diff = run_managed_git_command(
            &run.project_path,
            &["diff", "--cached", "--quiet"],
            MANAGED_GIT_TIMEOUT,
        )
        .await;
        append_managed_git_command(run, &group_id, &["diff", "--cached", "--quiet"], &diff);
        if diff.ok {
            set_managed_git_entry_field(
                run,
                &group_id,
                "noCommitReason",
                json!("No staged changes after git add -A."),
            );
        } else {
            let message = build_managed_git_commit_message(&task, &group_id);
            run_managed_git_ledger_command(
                run,
                &group_id,
                &["commit", "-m", &message],
                MANAGED_GIT_TIMEOUT,
            )
            .await?;
        }
    }
    run_managed_git_ledger_command(
        run,
        &group_id,
        &["switch", &base_branch],
        MANAGED_GIT_TIMEOUT,
    )
    .await?;
    let no_commit = managed_git_entry(run, &group_id)
        .and_then(|entry| entry.get("noCommitReason"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if no_commit.is_none() {
        run_managed_git_ledger_command(
            run,
            &group_id,
            &["merge", "--ff-only", &branch_name],
            MANAGED_GIT_TIMEOUT,
        )
        .await?;
        run_managed_git_ledger_command(
            run,
            &group_id,
            &["push", "origin", &base_branch],
            MANAGED_GIT_PUSH_TIMEOUT,
        )
        .await?;
    }
    set_managed_git_entry_field(run, &group_id, "status", json!("completed"));
    set_managed_git_entry_field(run, &group_id, "completedAt", json!(Utc::now()));
    if let Some(reason) = no_commit {
        run.append_log(format!(
            "Managed git completed {group_id} without a commit: {reason}"
        ));
    } else {
        run.append_log(format!(
            "Managed git committed, merged, and pushed {group_id} via {branch_name}"
        ));
    }
    Ok(())
}

fn is_managed_git_group_root(task: &BoardTask) -> bool {
    !is_qa_task(task)
        && !task.agents_knowledge_task
        && task.source_task_id.is_none()
        && task.task_type != "agents_knowledge"
}

fn task_group_has_unfinished_work(run: &AgenticBoard, group_id: &str) -> bool {
    run.tasks.iter().any(|task| {
        task.group_id.as_deref() == Some(group_id)
            && matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_BACKLOG | TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS
            )
    })
}

fn managed_git_entry<'a>(run: &'a AgenticBoard, group_id: &str) -> Option<&'a Value> {
    run.git_ledger
        .iter()
        .find(|entry| entry.get("groupId").and_then(Value::as_str) == Some(group_id))
}

fn managed_git_entry_mut<'a>(run: &'a mut AgenticBoard, group_id: &str) -> Option<&'a mut Value> {
    run.git_ledger
        .iter_mut()
        .find(|entry| entry.get("groupId").and_then(Value::as_str) == Some(group_id))
}

fn set_managed_git_entry_field(run: &mut AgenticBoard, group_id: &str, key: &str, value: Value) {
    if let Some(entry) = managed_git_entry_mut(run, group_id).and_then(Value::as_object_mut) {
        entry.insert(key.to_string(), value);
    }
}

fn mark_managed_git_failed(run: &mut AgenticBoard, group_id: &str, message: &str) {
    set_managed_git_entry_field(run, group_id, "status", json!("failed"));
    set_managed_git_entry_field(run, group_id, "completedAt", json!(Utc::now()));
    set_managed_git_entry_field(run, group_id, "error", json!(message));
    run.append_log(format!("Managed git failed for {group_id}: {message}"));
}

#[derive(Debug, Clone)]
struct ManagedGitCommandResult {
    ok: bool,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

async fn run_managed_git_ledger_command(
    run: &mut AgenticBoard,
    group_id: &str,
    args: &[&str],
    timeout_duration: Duration,
) -> Result<ManagedGitCommandResult> {
    let result = run_managed_git_command(&run.project_path, args, timeout_duration).await;
    append_managed_git_command(run, group_id, args, &result);
    if result.ok {
        Ok(result)
    } else {
        let message = format!(
            "git {} failed with exit {}{}",
            args.join(" "),
            result.exit_code,
            if result.stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", limit_text(result.stderr.trim(), 700))
            }
        );
        mark_managed_git_failed(run, group_id, &message);
        Err(ServerError::new(StatusCode::CONFLICT, message))
    }
}

async fn run_managed_git_command(
    project_path: &str,
    args: &[&str],
    timeout_duration: Duration,
) -> ManagedGitCommandResult {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(project_path)
        .env("PATH", augmented_user_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match timeout(timeout_duration, command.output()).await {
        Ok(Ok(output)) => ManagedGitCommandResult {
            ok: output.status.success(),
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Ok(Err(error)) => ManagedGitCommandResult {
            ok: false,
            exit_code: 1,
            stdout: String::new(),
            stderr: error.to_string(),
        },
        Err(_) => ManagedGitCommandResult {
            ok: false,
            exit_code: 124,
            stdout: String::new(),
            stderr: format!("git {} timed out", args.join(" ")),
        },
    }
}

fn append_managed_git_command(
    run: &mut AgenticBoard,
    group_id: &str,
    args: &[&str],
    result: &ManagedGitCommandResult,
) {
    if let Some(entry) = managed_git_entry_mut(run, group_id).and_then(Value::as_object_mut) {
        let commands = entry
            .entry("commands".to_string())
            .or_insert_with(|| json!([]));
        if let Some(items) = commands.as_array_mut() {
            items.push(json!({
                "command": format!("git {}", args.join(" ")),
                "args": args,
                "ok": result.ok,
                "exitCode": result.exit_code,
                "stdout": limit_text(&result.stdout, 4000),
                "stderr": limit_text(&result.stderr, 4000),
                "completedAt": Utc::now(),
            }));
        }
    }
}

fn current_git_branch(project_path: &str) -> String {
    git_command_text(project_path, &["branch", "--show-current"])
}

fn build_managed_git_branch_name(task: &BoardTask, group_id: &str) -> String {
    let slug = task
        .title
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    let suffix = if slug.is_empty() {
        group_id.to_string()
    } else {
        slug
    };
    format!("agentic/{group_id}-{suffix}")
        .chars()
        .take(96)
        .collect()
}

fn build_managed_git_commit_message(task: &BoardTask, group_id: &str) -> String {
    format!(
        "Agentic task {group_id}: {}",
        limit_text(&task.title, 120).replace('\n', " ")
    )
}

fn summarize_workspace_delta(task_id: &str, before: &Value, after: &Value) -> Value {
    let before_map = before
        .get("filesByPath")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let after_map = after
        .get("filesByPath")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let paths = before_map
        .keys()
        .chain(after_map.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut touched_files = Vec::new();
    let mut changed_by_subtask = Vec::new();
    let mut unknown_changes = Vec::new();
    for path in paths {
        let before_file = before_map.get(&path);
        let after_file = after_map.get(&path);
        if before_file == after_file {
            continue;
        }
        let classification =
            if before_file.is_none() || before_file.is_some_and(workspace_file_was_clean) {
                "changed_by_subtask"
            } else {
                "unknown_change"
            };
        let entry = workspace_delta_entry(&path, before_file, after_file, classification);
        touched_files.push(entry.clone());
        if classification == "changed_by_subtask" {
            changed_by_subtask.push(entry);
        } else {
            unknown_changes.push(entry);
        }
    }
    let pre_existing_changes = before_map
        .iter()
        .filter(|(_, before_file)| !workspace_file_was_clean(before_file))
        .map(|(path, before_file)| {
            workspace_delta_entry(
                path,
                Some(before_file),
                after_map.get(path),
                "pre_existing_change",
            )
        })
        .collect::<Vec<_>>();
    let touched_file_count = touched_files.len();
    let attributable_changed_file_count = changed_by_subtask.len();
    let unknown_change_count = unknown_changes.len();
    json!({
        "taskId": task_id,
        "capturedAt": Utc::now(),
        "isGit": after.get("isGit").and_then(Value::as_bool).unwrap_or(false),
        "touchedFiles": touched_files,
        "touchedFileCount": touched_file_count,
        "preExistingChanges": pre_existing_changes,
        "preExistingChangeCount": pre_existing_changes.len(),
        "changedBySubtask": changed_by_subtask,
        "attributableChangedFileCount": attributable_changed_file_count,
        "unknownChanges": unknown_changes,
        "unknownChangeCount": unknown_change_count,
        "ownershipPolicy": WORKSPACE_OWNERSHIP_POLICY,
        "currentWorkspaceFiles": after.get("files").cloned().unwrap_or_else(|| json!([])),
        "currentWorkspaceFileCount": after.get("files").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "shortStat": after.get("shortStat").and_then(Value::as_str).unwrap_or(""),
        "unavailableReason": after.get("error").and_then(Value::as_str).unwrap_or(""),
    })
}

fn workspace_delta_entry(
    path: &str,
    before_file: Option<&Value>,
    after_file: Option<&Value>,
    classification: &str,
) -> Value {
    json!({
        "path": path,
        "classification": classification,
        "beforeStatus": before_file.and_then(|value| value.get("status")).and_then(Value::as_str).unwrap_or(""),
        "afterStatus": after_file.and_then(|value| value.get("status")).and_then(Value::as_str).unwrap_or(""),
        "beforeHash": before_file.and_then(|value| value.get("hash")).and_then(Value::as_str),
        "afterHash": after_file.and_then(|value| value.get("hash")).and_then(Value::as_str),
    })
}

fn workspace_file_was_clean(file: &Value) -> bool {
    file.get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_none_or(|status| status.is_empty() || status.eq_ignore_ascii_case("clean"))
}

fn change_summary_paths(summary: &Value) -> Vec<String> {
    summary
        .get("changedBySubtask")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|file| file.get("path").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn change_summary_attributable_file_count(summary: &Value) -> u64 {
    summary
        .get("attributableChangedFileCount")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| change_summary_paths(summary).len() as u64)
}

fn normalize_changed_file_summary(summary: &Value) -> Value {
    if summary.get("ownershipPolicy").and_then(Value::as_str) == Some(WORKSPACE_OWNERSHIP_POLICY) {
        return summary.clone();
    }
    let unknown_changes = summary
        .get("touchedFiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|entry| match entry {
            Value::Object(object) => {
                let mut entry = object.clone();
                entry.insert("classification".to_string(), json!("unknown_change"));
                Value::Object(entry)
            }
            Value::String(path) => json!({
                "path": path,
                "classification": "unknown_change",
            }),
            other => other.clone(),
        })
        .collect::<Vec<_>>();
    let mut normalized = summary.as_object().cloned().unwrap_or_default();
    normalized.insert("touchedFiles".to_string(), json!(unknown_changes.clone()));
    normalized.insert("preExistingChanges".to_string(), json!([]));
    normalized.insert("preExistingChangeCount".to_string(), json!(0));
    normalized.insert("changedBySubtask".to_string(), json!([]));
    normalized.insert("attributableChangedFileCount".to_string(), json!(0));
    normalized.insert("unknownChanges".to_string(), json!(unknown_changes.clone()));
    normalized.insert(
        "unknownChangeCount".to_string(),
        json!(unknown_changes.len()),
    );
    normalized.insert(
        "ownershipPolicy".to_string(),
        json!(LEGACY_WORKSPACE_OWNERSHIP_POLICY),
    );
    Value::Object(normalized)
}

fn refresh_codebase_context_after_task(run: &mut AgenticBoard, change_summary: &Value) {
    let paths = change_summary_paths(change_summary);
    if paths.is_empty() {
        return;
    }
    let touched = paths.iter().cloned().collect::<BTreeSet<_>>();
    run.codebase_understanding.retain(|entry| {
        entry
            .get("path")
            .and_then(Value::as_str)
            .is_none_or(|path| !touched.contains(path))
    });
    let bundle = build_codebase_bundle(&run.project_path);
    run.codebase_manifest = bundle.manifest;
    run.codebase_chunks = bundle.chunks;
    let refreshed_snapshot = local_codebase_snapshot(run);
    if let Some(map) = run.codebase_map.as_mut().and_then(Value::as_object_mut) {
        map.insert("localSnapshot".to_string(), refreshed_snapshot);
        map.insert("refreshedAt".to_string(), json!(Utc::now()));
        map.insert("refreshReason".to_string(), json!("task workspace changes"));
    }
    run.append_log(format!(
        "Refreshed codebase context after workspace changes in {} file(s)",
        paths.len()
    ));
}

async fn run_deterministic_validation(run: &AgenticBoard, task_id: &str, stage: &str) -> Value {
    let started_at = Utc::now();
    if run.validation_config.get("enabled").and_then(value_as_bool) == Some(false) {
        return json!({
            "stage": stage,
            "taskId": task_id,
            "startedAt": started_at,
            "completedAt": Utc::now(),
            "passed": true,
            "skipped": true,
            "commands": [],
            "summary": "Deterministic validation disabled for this board.",
        });
    }
    let scripts = package_validation_scripts(run, stage);
    let validation_timeout = validation_timeout(run);
    let mut commands = Vec::new();
    for (script_name, command_text) in scripts {
        let command_started = Utc::now();
        let result =
            run_shell_validation_command(&run.project_path, &command_text, validation_timeout)
                .await;
        let duration_ms = (Utc::now() - command_started).num_milliseconds().max(0);
        let exit_code = result.as_ref().map(|result| result.0).unwrap_or(124);
        let output = result
            .map(|result| limit_text(&format!("{}\n{}", result.1, result.2), 1200))
            .unwrap_or_else(|error| error);
        commands.push(json!({
            "command": command_text,
            "scriptName": script_name,
            "exitCode": exit_code,
            "output": output,
            "durationMs": duration_ms,
        }));
        if exit_code != 0 && stage == "feature" {
            break;
        }
    }
    let passed = commands
        .iter()
        .all(|command| command.get("exitCode").and_then(Value::as_i64).unwrap_or(0) == 0);
    json!({
        "stage": stage,
        "taskId": task_id,
        "startedAt": started_at,
        "completedAt": Utc::now(),
        "passed": passed,
        "commands": commands,
    })
}

async fn run_tdd_validation(run: &AgenticBoard, task: &BoardTask, stage: &str) -> Value {
    if is_qa_task(task) || task.qa_test_commands.is_empty() {
        return run_deterministic_validation(run, &task.id, stage).await;
    }
    let generated = run_generated_test_commands(
        &run.project_path,
        &task.id,
        &task.qa_test_commands,
        stage,
        validation_timeout(run),
    )
    .await;
    let deterministic = run_deterministic_validation(run, &task.id, stage).await;
    let mut commands = generated
        .get("commands")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    commands.extend(
        deterministic
            .get("commands")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    );
    let passed = commands
        .iter()
        .all(|command| command.get("exitCode").and_then(Value::as_i64).unwrap_or(0) == 0);
    json!({
        "stage": stage,
        "taskId": task.id,
        "startedAt": generated.get("startedAt").cloned().unwrap_or_else(|| json!(Utc::now())),
        "completedAt": Utc::now(),
        "passed": passed,
        "commands": commands,
        "generatedTestCommands": task.qa_test_commands,
        "packageValidation": deterministic,
    })
}

async fn run_generated_test_commands(
    project_path: &str,
    task_id: &str,
    commands_to_run: &[String],
    stage: &str,
    validation_timeout: Duration,
) -> Value {
    let started_at = Utc::now();
    let mut commands = Vec::new();
    for command_text in commands_to_run {
        let command_started = Utc::now();
        let result =
            run_shell_validation_command(project_path, command_text, validation_timeout).await;
        let duration_ms = (Utc::now() - command_started).num_milliseconds().max(0);
        let exit_code = result.as_ref().map(|result| result.0).unwrap_or(124);
        let output = result
            .map(|result| limit_text(&format!("{}\n{}", result.1, result.2), 1200))
            .unwrap_or_else(|error| error);
        commands.push(json!({
            "command": command_text,
            "scriptName": "generated_tdd",
            "exitCode": exit_code,
            "output": output,
            "durationMs": duration_ms,
        }));
        if exit_code != 0 && stage != "qa_baseline" {
            break;
        }
    }
    let passed = commands
        .iter()
        .all(|command| command.get("exitCode").and_then(Value::as_i64).unwrap_or(0) == 0);
    json!({
        "stage": stage,
        "taskId": task_id,
        "startedAt": started_at,
        "completedAt": Utc::now(),
        "passed": passed,
        "commands": commands,
    })
}

async fn run_shell_validation_command(
    project_path: &str,
    command_text: &str,
    validation_timeout: Duration,
) -> std::result::Result<(i32, String, String), String> {
    let mut command = Command::new("sh");
    command
        .arg("-lc")
        .arg(command_text)
        .current_dir(project_path)
        .env("PATH", augmented_user_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command
        .spawn()
        .map_err(|error| format!("Failed to spawn validation command: {error}"))?;
    match timeout(validation_timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok((
            output.status.code().unwrap_or(1),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )),
        Ok(Err(error)) => Err(format!("Validation command failed: {error}")),
        Err(_) => Err(format!(
            "Validation command timed out after {} seconds",
            validation_timeout.as_secs()
        )),
    }
}

fn package_validation_scripts(run: &AgenticBoard, stage: &str) -> Vec<(String, String)> {
    let configured = validation_commands_for_stage(run, stage)
        .into_iter()
        .enumerate()
        .map(|(index, command)| (format!("configured_{}", index + 1), command))
        .collect::<Vec<_>>();
    if !configured.is_empty() {
        return configured;
    }
    let project_path = &run.project_path;
    let package_path = Path::new(project_path).join("package.json");
    let Some(package_json) = fs::read_to_string(package_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    else {
        return Vec::new();
    };
    let Some(scripts) = package_json.get("scripts").and_then(Value::as_object) else {
        return Vec::new();
    };
    let runner = infer_package_runner(project_path);
    let candidates = if stage == "final" {
        ["typecheck", "lint", "test", "build", "check"]
    } else {
        ["typecheck", "test", "lint", "build", "check"]
    };
    candidates
        .into_iter()
        .filter(|name| scripts.contains_key(*name))
        .take(validation_max_commands_for_stage(run, stage))
        .map(|name| (name.to_string(), format!("{runner} run {name}")))
        .collect()
}

fn validation_commands_for_stage(run: &AgenticBoard, stage: &str) -> Vec<String> {
    let key = match stage {
        "final" => "finalCommands",
        "qa" | "qa_baseline" => "qaCommands",
        _ => "featureCommands",
    };
    normalize_string_list(run.validation_config.get(key))
}

fn validation_max_commands_for_stage(run: &AgenticBoard, stage: &str) -> usize {
    let key = match stage {
        "final" => "maxFinalCommands",
        "qa" | "qa_baseline" => "maxQaCommands",
        _ => "maxFeatureCommands",
    };
    run.validation_config
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(if stage == "final" { 4 } else { 2 })
}

fn validation_timeout(run: &AgenticBoard) -> Duration {
    Duration::from_secs(
        run.validation_config
            .get("timeoutSeconds")
            .and_then(Value::as_u64)
            .unwrap_or(DETERMINISTIC_VALIDATION_TIMEOUT.as_secs())
            .clamp(5, 3600),
    )
}

fn infer_package_runner(project_path: &str) -> &'static str {
    let root = Path::new(project_path);
    if root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if root.join("yarn.lock").is_file() {
        "yarn"
    } else if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
        "bun"
    } else {
        "npm"
    }
}

fn format_validation_check(validation: &Value) -> String {
    let stage = validation
        .get("stage")
        .and_then(Value::as_str)
        .unwrap_or("feature");
    let passed = validation
        .get("passed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let commands = validation
        .get("commands")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    format!(
                        "{} exited {}",
                        item.get("command")
                            .and_then(Value::as_str)
                            .unwrap_or("command"),
                        item.get("exitCode").and_then(Value::as_i64).unwrap_or(0)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    if commands.is_empty() {
        format!("Deterministic {stage} validation: no supported package scripts were available.")
    } else {
        format!(
            "Deterministic {stage} validation: {} ({commands})",
            if passed { "PASS" } else { "FAIL" }
        )
    }
}

fn normalize_string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str())
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        Some(Value::String(text)) => text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn dedupe_strings(items: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    items
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

fn normalize_priority(value: Option<&str>) -> &'static str {
    match value
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "p0" | "critical" | "urgent" | "blocker" | "highest" => TASK_PRIORITY_P0,
        "p1" | "high" => TASK_PRIORITY_P1,
        "p3" | "low" => TASK_PRIORITY_P3,
        "p2" | "medium" | "normal" => TASK_PRIORITY_P2,
        _ => TASK_PRIORITY_P2,
    }
}

fn task_priority_for_parent(
    run: &AgenticBoard,
    parent_id: Option<&str>,
    requested_priority: Option<&str>,
) -> String {
    if let Some(priority) = requested_priority
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return normalize_priority(Some(priority)).to_string();
    }
    parent_id
        .and_then(|parent_id| run.tasks.iter().find(|task| task.id == parent_id))
        .map(|parent| normalize_priority(Some(&parent.priority)).to_string())
        .unwrap_or_else(|| TASK_PRIORITY_P2.to_string())
}

fn normalize_model_strategy(value: Option<Value>) -> Option<Value> {
    let source = match value? {
        Value::String(mode) => json!({ "mode": mode }),
        Value::Object(map) => Value::Object(map),
        _ => return None,
    };
    let Some(source) = source.as_object() else {
        return None;
    };
    if source.is_empty() {
        return None;
    }

    let raw_mode = source
        .get("mode")
        .or_else(|| source.get("strategy"))
        .or_else(|| source.get("value"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .replace('-', "_");
    let cheap_model = normalize_model_value(
        source
            .get("cheapModel")
            .or_else(|| source.get("cheap"))
            .or_else(|| source.get("budgetModel"))
            .or_else(|| source.get("budget")),
    )
    .or_else(default_cheap_model);
    let expensive_model = normalize_model_value(
        source
            .get("expensiveModel")
            .or_else(|| source.get("expensive"))
            .or_else(|| source.get("qualityModel"))
            .or_else(|| source.get("quality")),
    )
    .or_else(default_expensive_model);

    let has_strategy_inputs = matches!(
        raw_mode.as_str(),
        "cheap" | "hybrid" | "expensive" | "manual"
    ) || cheap_model.is_some()
        || expensive_model.is_some();
    let mode = match raw_mode.as_str() {
        "cheap" | "hybrid" | "expensive" | "manual" => raw_mode,
        _ if cheap_model.is_some() || expensive_model.is_some() => "hybrid".to_string(),
        _ => String::new(),
    };

    let mut normalized = source.clone();
    if !mode.is_empty() {
        normalized.insert("mode".to_string(), json!(mode));
    }
    if has_strategy_inputs {
        normalized.insert(
            "cheapModel".to_string(),
            json!(cheap_model.unwrap_or_default()),
        );
        normalized.insert(
            "expensiveModel".to_string(),
            json!(expensive_model.unwrap_or_default()),
        );
    }
    if normalized.is_empty() {
        None
    } else {
        Some(Value::Object(normalized))
    }
}

fn body_has_model_strategy_keys(value: &Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    [
        "mode",
        "strategy",
        "cheapModel",
        "cheap",
        "budgetModel",
        "expensiveModel",
        "expensive",
        "qualityModel",
        "fallbackProvider",
        "fallbackModel",
        "reasoningEffort",
        "reasoning_effort",
        "effort",
        "thinking",
        "enableThinking",
        "fast",
        "fastMode",
        "serviceTier",
        "model",
        "taskModel",
    ]
    .iter()
    .any(|key| map.contains_key(*key))
}

fn normalize_model_value(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn default_cheap_model() -> Option<String> {
    env::var("DANGER_CHEAP_MODEL")
        .ok()
        .or_else(|| env::var("IO_WORKBENCH_CHEAP_MODEL").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn default_expensive_model() -> Option<String> {
    env::var("DANGER_EXPENSIVE_MODEL")
        .ok()
        .or_else(|| env::var("IO_WORKBENCH_EXPENSIVE_MODEL").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn model_strategy_mode(strategy: Option<&Value>) -> &str {
    strategy
        .and_then(|value| value.get("mode"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
}

fn primary_model_for_strategy(strategy: Option<&Value>) -> Option<String> {
    let strategy = strategy?;
    let mode = model_strategy_mode(Some(strategy));
    let candidate = match mode {
        "cheap" => strategy.get("cheapModel"),
        "hybrid" | "expensive" => strategy.get("expensiveModel"),
        "manual" => None,
        _ => strategy
            .get("model")
            .or_else(|| strategy.get("primaryModel"))
            .or_else(|| strategy.get("taskModel")),
    };
    normalize_model_value(candidate)
}

fn task_model_overrides_for_strategy(strategy: Option<&Value>) -> Value {
    let Some(strategy) = strategy else {
        return json!({});
    };
    let mode = model_strategy_mode(Some(strategy));
    let cheap = normalize_model_value(strategy.get("cheapModel"));
    let expensive = normalize_model_value(strategy.get("expensiveModel"));
    let mut map = serde_json::Map::new();
    let mut insert = |key: &str, model: Option<&String>| {
        if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
            map.insert(key.to_string(), json!(model));
        }
    };
    match mode {
        "cheap" => {
            for key in [
                "breakdown",
                "implementation",
                "qa",
                "qa_fix",
                "agents",
                "final_qa",
            ] {
                insert(key, cheap.as_ref());
            }
        }
        "expensive" => {
            for key in [
                "breakdown",
                "implementation",
                "qa",
                "qa_fix",
                "agents",
                "final_qa",
            ] {
                insert(key, expensive.as_ref());
            }
        }
        "hybrid" => {
            insert("breakdown", expensive.as_ref());
            insert("implementation", cheap.as_ref());
            insert("qa", expensive.as_ref());
            insert("qa_fix", expensive.as_ref());
            insert("agents", cheap.as_ref());
            insert("final_qa", expensive.as_ref());
        }
        _ => {}
    }
    Value::Object(map)
}

fn normalize_task_model_overrides(value: Value) -> Value {
    let Some(source) = value.as_object() else {
        return json!({});
    };
    let mut map = serde_json::Map::new();
    for (key, value) in source {
        let Some(model) = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        map.insert(canonical_task_model_key(key), json!(model));
    }
    Value::Object(map)
}

fn canonical_task_model_key(key: &str) -> String {
    match key.trim().replace('-', "_").as_str() {
        "qaFix" | "qa_fix" => "qa_fix".to_string(),
        "finalQa" | "finalQA" | "final_qa" => "final_qa".to_string(),
        "taskExecution" | "task_execution" => "task_execution".to_string(),
        value => value.to_string(),
    }
}

fn merge_task_model_overrides(base: Value, overrides: Value) -> Value {
    let mut map = base.as_object().cloned().unwrap_or_default();
    if let Some(overrides) = overrides.as_object() {
        for (key, value) in overrides {
            map.insert(key.clone(), value.clone());
        }
    }
    Value::Object(map)
}

fn merge_json_objects(base: Value, patch: Value) -> Value {
    let mut map = base.as_object().cloned().unwrap_or_default();
    if let Some(patch) = patch.as_object() {
        for (key, value) in patch {
            map.insert(key.clone(), value.clone());
        }
    }
    Value::Object(map)
}

fn json_object_is_empty(value: &Value) -> bool {
    value.as_object().map(|map| map.is_empty()).unwrap_or(true)
}

fn normalize_validation_config(value: Option<&Value>) -> Value {
    let defaults = default_validation_config();
    let source = value.and_then(Value::as_object);
    json!({
        "enabled": source.and_then(|map| map.get("enabled")).and_then(value_as_bool).unwrap_or(true),
        "featureCommands": nonempty_string_list_or_default(source.and_then(|map| map.get("featureCommands")), defaults.get("featureCommands")),
        "finalCommands": nonempty_string_list_or_default(source.and_then(|map| map.get("finalCommands")), defaults.get("finalCommands")),
        "qaCommands": nonempty_string_list_or_default(source.and_then(|map| map.get("qaCommands")), defaults.get("qaCommands")),
        "maxFeatureCommands": source.and_then(|map| map.get("maxFeatureCommands")).and_then(Value::as_u64).unwrap_or(2).clamp(0, 20),
        "maxFinalCommands": source.and_then(|map| map.get("maxFinalCommands")).and_then(Value::as_u64).unwrap_or(4).clamp(0, 20),
        "maxQaCommands": source.and_then(|map| map.get("maxQaCommands")).and_then(Value::as_u64).unwrap_or(2).clamp(0, 20),
        "timeoutSeconds": source.and_then(|map| map.get("timeoutSeconds")).and_then(Value::as_u64).unwrap_or(120).clamp(5, 3600),
    })
}

fn default_validation_config() -> Value {
    json!({
        "enabled": true,
        "featureCommands": [],
        "finalCommands": [],
        "qaCommands": [],
        "maxFeatureCommands": 2,
        "maxFinalCommands": 4,
        "maxQaCommands": 2,
        "timeoutSeconds": 120,
    })
}

fn normalize_rag_settings(value: Option<&Value>) -> Value {
    let defaults = default_rag_settings();
    let source = value.and_then(Value::as_object);
    json!({
        "enabled": source.and_then(|map| map.get("enabled")).and_then(value_as_bool).unwrap_or(true),
        "indexOnBootstrap": source.and_then(|map| map.get("indexOnBootstrap")).and_then(value_as_bool).unwrap_or(true),
        "queryEnabled": source.and_then(|map| map.get("queryEnabled")).and_then(value_as_bool).unwrap_or(true),
        "ingestTaskResults": source.and_then(|map| map.get("ingestTaskResults")).and_then(value_as_bool).unwrap_or(true),
        "ingestValidationErrors": source.and_then(|map| map.get("ingestValidationErrors")).and_then(value_as_bool).unwrap_or(true),
        "scopes": nonempty_string_list_or_default(source.and_then(|map| map.get("scopes")), defaults.get("scopes")),
        "contextMaxChars": source.and_then(|map| map.get("contextMaxChars")).and_then(Value::as_u64).unwrap_or(12_000).clamp(1_000, 80_000),
    })
}

fn default_rag_settings() -> Value {
    json!({
        "enabled": true,
        "indexOnBootstrap": true,
        "queryEnabled": true,
        "ingestTaskResults": true,
        "ingestValidationErrors": true,
        "scopes": ["global_standard", "project_specific", "validation_error"],
        "contextMaxChars": 12_000,
    })
}

fn rag_enabled_from_settings(settings: &Value) -> bool {
    settings
        .get("enabled")
        .and_then(value_as_bool)
        .unwrap_or(true)
}

fn normalize_qa_policy(value: Option<&Value>) -> Value {
    let source = value.and_then(Value::as_object);
    json!({
        "maxFollowupsPerGroup": source.and_then(|map| map.get("maxFollowupsPerGroup")).and_then(Value::as_u64).unwrap_or(MAX_FOLLOWUP_TASKS_PER_GROUP as u64).clamp(0, 20),
        "maxTaskAttempts": source.and_then(|map| map.get("maxTaskAttempts")).and_then(Value::as_u64).unwrap_or(MAX_TASK_ATTEMPTS as u64).clamp(1, 10),
        "taskQaMode": normalize_task_qa_mode(source.and_then(|map| map.get("taskQaMode")).and_then(Value::as_str)),
        "repairMalformedToolCalls": source.and_then(|map| map.get("repairMalformedToolCalls")).and_then(value_as_bool).unwrap_or(true),
        "malformedToolCallRepairRetries": source.and_then(|map| map.get("malformedToolCallRepairRetries")).and_then(Value::as_u64).unwrap_or(DEFAULT_MALFORMED_TOOL_CALL_REPAIR_RETRIES).clamp(0, MAX_MALFORMED_TOOL_CALL_REPAIR_RETRIES),
    })
}

fn default_qa_policy() -> Value {
    normalize_qa_policy(None)
}

fn normalize_task_qa_mode(value: Option<&str>) -> String {
    match value
        .map(str::trim)
        .unwrap_or("high_risk")
        .to_ascii_lowercase()
        .replace('-', "_")
        .as_str()
    {
        "off" | "none" | "disabled" | "false" => "off".to_string(),
        "all" | "always" | "every" => "all".to_string(),
        _ => "high_risk".to_string(),
    }
}

fn normalize_tdd_policy(value: Option<&Value>) -> Value {
    let defaults = default_tdd_policy();
    let source = value.and_then(Value::as_object);
    json!({
        "requireFailingTestBeforeDev": source.and_then(|map| map.get("requireFailingTestBeforeDev")).and_then(value_as_bool).unwrap_or(true),
        "maxFixAttempts": source.and_then(|map| map.get("maxFixAttempts")).and_then(Value::as_u64).unwrap_or(3).clamp(0, 20),
        "allowImplementationWithoutTests": source.and_then(|map| map.get("allowImplementationWithoutTests")).and_then(value_as_bool).unwrap_or(false),
        "qaCommandStage": source.and_then(|map| map.get("qaCommandStage")).and_then(Value::as_str).unwrap_or_else(|| defaults.get("qaCommandStage").and_then(Value::as_str).unwrap_or("qa")),
        "featureCommandStage": source.and_then(|map| map.get("featureCommandStage")).and_then(Value::as_str).unwrap_or_else(|| defaults.get("featureCommandStage").and_then(Value::as_str).unwrap_or("feature")),
        "finalCommandStage": source.and_then(|map| map.get("finalCommandStage")).and_then(Value::as_str).unwrap_or_else(|| defaults.get("finalCommandStage").and_then(Value::as_str).unwrap_or("final")),
    })
}

fn nonempty_string_list_or_default(value: Option<&Value>, default: Option<&Value>) -> Vec<String> {
    let values = normalize_string_list(value);
    if values.is_empty() {
        normalize_string_list(default)
    } else {
        values
    }
}

fn normalize_auto_retry(value: &Value) -> Value {
    let object = value.as_object();
    json!({
        "enabled": object.and_then(|map| map.get("enabled")).and_then(Value::as_bool).unwrap_or(false),
        "delayMinutes": object.and_then(|map| map.get("delayMinutes")).and_then(Value::as_u64).unwrap_or(10),
        "maxAttempts": object.and_then(|map| map.get("maxAttempts")).and_then(Value::as_u64).unwrap_or(3),
        "attempts": object.and_then(|map| map.get("attempts")).and_then(Value::as_u64).unwrap_or(0),
        "nextRetryAt": object.and_then(|map| map.get("nextRetryAt")).cloned().unwrap_or(Value::Null),
        "lastRetryAt": object.and_then(|map| map.get("lastRetryAt")).cloned().unwrap_or(Value::Null),
        "lastError": object.and_then(|map| map.get("lastError")).and_then(Value::as_str).unwrap_or(""),
        "updatedAt": object.and_then(|map| map.get("updatedAt")).cloned().unwrap_or_else(|| json!(Utc::now())),
    })
}

fn merge_auto_retry(base: Value, patch: Value) -> Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    if let Some(patch) = patch.as_object() {
        for (key, value) in patch {
            object.insert(key.clone(), value.clone());
        }
    }
    Value::Object(object)
}

fn auto_retry_enabled(value: &Value) -> bool {
    normalize_auto_retry(value)
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn is_resumable_board(board: &AgenticBoard) -> bool {
    match board.status.as_str() {
        "paused" | "pausing" => true,
        "blocked" | "failed" | "cancelled" => board.tasks.iter().any(|task| {
            matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_TODO
                    | TASK_STATUS_IN_PROGRESS
                    | TASK_STATUS_BLOCKED
                    | TASK_STATUS_FAILED
            )
        }),
        _ => false,
    }
}

fn normalize_retry_mode(value: Option<&str>) -> Result<&'static str> {
    match value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(RETRY_MODE_TRANSIENT)
        .to_ascii_lowercase()
        .replace(['-', ' '], "_")
        .as_str()
    {
        "transient" | "retry" | "environment" => Ok(RETRY_MODE_TRANSIENT),
        "fix" | "defect" => Ok(RETRY_MODE_FIX),
        _ => Err(bad_request("Retry mode must be transient or fix.")),
    }
}

fn task_failure_text(task: &BoardTask) -> String {
    let mut parts = Vec::new();
    if let Some(error) = task.error.as_deref() {
        parts.push(error.to_string());
    }
    parts.push(task.summary.clone());
    if let Some(result) = task.result.as_ref() {
        for key in ["summary", "status", "qaResult"] {
            if let Some(value) = result.get(key).and_then(Value::as_str) {
                parts.push(value.to_string());
            }
        }
        for key in ["remainingIssues", "remainingGaps"] {
            parts.extend(normalize_string_list(result.get(key)));
        }
    }
    parts.join("\n").to_ascii_lowercase()
}

fn task_failure_is_dependency_blocked(task: &BoardTask) -> bool {
    task.error.as_deref().is_some_and(is_dependency_block_error)
}

fn task_failure_is_transient(task: &BoardTask) -> bool {
    if task_failure_is_dependency_blocked(task) {
        return false;
    }
    let text = task_failure_text(task);
    if text.contains("planning error")
        || text.contains("completion evidence gate")
        || text.contains("acceptance criteria")
        || text.contains("assertion")
        || text.contains("expected") && text.contains("actual")
        || text.contains("implementation defect")
    {
        return false;
    }
    text.contains("provider")
        || text.contains("transport")
        || text.contains("network")
        || text.contains("timed out")
        || text.contains("timeout")
        || text.contains("connection")
        || text.contains("unavailable")
        || text.contains("rate limit")
        || text.contains("too many requests")
        || text.contains("emulator not ready")
        || text.contains("test infrastructure")
        || text.contains("tool environment")
        || text.contains("internal error")
        || text.contains("could not start")
        || text.contains("failed to launch")
        || text.contains("exit code")
        || text.contains("status code 429")
        || text.contains("status code 502")
        || text.contains("status code 503")
}

fn fix_plan_is_linked_to(fix: &BoardTask, failed_task_id: &str) -> bool {
    fix.source_task_id.as_deref() == Some(failed_task_id)
        || fix.source_qa_task_id.as_deref() == Some(failed_task_id)
        || task_blockers(fix).iter().any(|id| id == failed_task_id)
        || fix.references.iter().any(|reference| {
            reference.contains(failed_task_id) && reference.to_ascii_lowercase().contains("source")
        })
}

fn approved_fix_plan_for<'a>(
    run: &'a AgenticBoard,
    failed_task_id: &str,
    requested_fix_task_id: Option<&str>,
) -> Option<&'a BoardTask> {
    run.tasks.iter().find(|task| {
        requested_fix_task_id.is_none_or(|id| task.id == id)
            && task.id != failed_task_id
            && task_is_executable(task)
            && canonical_task_kind(task) == TASK_KIND_FIX
            && matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS | TASK_STATUS_DONE
            )
            && task_ancestors_are_approved(run, task)
            && fix_plan_is_linked_to(task, failed_task_id)
    })
}

fn record_task_retry(task: &mut BoardTask, mode: &str, fix_task_id: Option<&str>, reason: &str) {
    let previous_status = canonical_task_status(&task.status).to_string();
    let retry_number = task
        .hierarchy
        .attempts
        .iter()
        .filter(|attempt| attempt.get("kind").and_then(Value::as_str) == Some("retry_request"))
        .count()
        + 1;
    task.hierarchy.attempts.push(json!({
        "kind": "retry_request",
        "retryNumber": retry_number,
        "mode": mode,
        "previousStatus": previous_status,
        "fixTaskId": fix_task_id,
        "reason": reason.trim(),
        "requestedAt": Utc::now(),
    }));
    task.status = TASK_STATUS_TODO.to_string();
    task.error = None;
    task.started_at = None;
    task.completed_at = None;
    task.provider_session_id = None;
}

fn retry_attention_tasks_in_board(
    run: &mut AgenticBoard,
    requested_ids: &[String],
    mode: &str,
    fix_task_id: Option<&str>,
    reason: &str,
) -> Result<usize> {
    reconcile_dependency_statuses(run);
    let ids = if requested_ids.is_empty() {
        run.tasks
            .iter()
            .filter(|task| {
                task_is_executable(task)
                    && matches!(
                        canonical_task_status(&task.status),
                        TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
                    )
            })
            .map(|task| task.id.clone())
            .collect::<Vec<_>>()
    } else {
        requested_ids.to_vec()
    };
    if ids.is_empty() {
        return Err(not_found(
            "No failed or blocked executable subtasks need retry.",
        ));
    }
    if mode == RETRY_MODE_TRANSIENT && fix_task_id.is_some_and(|id| !id.trim().is_empty()) {
        return Err(bad_request(
            "A transient retry cannot include fixTaskId; use fix mode for an approved fix plan.",
        ));
    }
    let snapshots = ids
        .iter()
        .map(|id| {
            run.tasks
                .iter()
                .find(|task| task.id == *id)
                .cloned()
                .ok_or_else(|| not_found(format!("Agentic board task not found: {id}")))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut fix_task_ids = BTreeMap::new();
    for task in &snapshots {
        if !task_is_executable(task)
            || !matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
            )
        {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                format!(
                    "Task {} is not a failed or blocked executable subtask.",
                    task.id
                ),
            ));
        }
        if task_failure_is_dependency_blocked(task) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                format!(
                    "Task {} is dependency-blocked. Resolve its dependency instead of retrying it.",
                    task.id
                ),
            ));
        }
        if task.error.as_deref().is_some_and(|error| {
            error.starts_with("External side-effect approval required")
                || error.starts_with("Planning error:")
        }) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                format!(
                    "Task {} has an approval or planning blocker that must be resolved first.",
                    task.id
                ),
            ));
        }
        if mode == RETRY_MODE_TRANSIENT && !task_failure_is_transient(task) {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                format!(
                    "Task {} appears to have an implementation defect. Create and approve a linked fix subtask before retrying it.",
                    task.id
                ),
            ));
        }
        let approved_fix_id = if mode == RETRY_MODE_FIX {
            let Some(fix) = approved_fix_plan_for(run, &task.id, fix_task_id) else {
                return Err(ServerError::new(
                    StatusCode::CONFLICT,
                    format!(
                        "Task {} requires a linked approved fix subtask (kind=fix, status=Todo or later) before it can retry.",
                        task.id
                    ),
                ));
            };
            Some(fix.id.clone())
        } else {
            None
        };
        fix_task_ids.insert(task.id.clone(), approved_fix_id);
    }
    for task_id in &ids {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == *task_id) {
            let selected_fix_id = fix_task_ids.get(task_id).cloned().flatten();
            record_task_retry(task, mode, selected_fix_id.as_deref(), reason);
        }
    }
    run.status = "running".to_string();
    run.active = true;
    run.loop_started = false;
    run.paused_at = None;
    run.pause_reason = None;
    clear_board_abort_state(run);
    run.current_phase = Some("retry_requested".to_string());
    run.phase_started_at = Some(Utc::now());
    run.phase_details = Some(json!({
        "mode": mode,
        "taskIds": ids,
        "fixTaskId": fix_task_id,
        "fixTaskIds": fix_task_ids,
        "reason": reason.trim(),
    }));
    run.append_log(format!(
        "Explicit {mode} retry requested for {} task(s)",
        ids.len()
    ));
    Ok(ids.len())
}

fn schedule_auto_retry_if_eligible(board: &mut AgenticBoard, reason: &str) -> bool {
    let state = normalize_auto_retry(&board.auto_retry);
    if !state
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || !is_resumable_board(board)
        || !run_has_transient_retryable_attention(board)
    {
        board.auto_retry = state;
        return false;
    }
    let attempts = state.get("attempts").and_then(Value::as_u64).unwrap_or(0);
    let max_attempts = state
        .get("maxAttempts")
        .and_then(Value::as_u64)
        .unwrap_or(3);
    if attempts >= max_attempts {
        board.auto_retry = merge_auto_retry(
            state,
            json!({
                "nextRetryAt": null,
                "lastError": format!("Max auto retries reached ({attempts}/{max_attempts})"),
                "updatedAt": Utc::now(),
            }),
        );
        board.append_log("Auto retry stopped: max attempts reached");
        return true;
    }
    let delay_minutes = state
        .get("delayMinutes")
        .and_then(Value::as_i64)
        .unwrap_or(10)
        .max(1);
    let next_retry_at = Utc::now() + chrono::Duration::minutes(delay_minutes);
    board.auto_retry = merge_auto_retry(
        state,
        json!({
            "nextRetryAt": next_retry_at,
            "lastError": "",
            "updatedAt": Utc::now(),
        }),
    );
    board.append_log(format!(
        "Auto retry scheduled in {delay_minutes} minute(s) after {reason}"
    ));
    true
}

fn run_has_transient_retryable_attention(run: &AgenticBoard) -> bool {
    run.tasks.iter().any(|task| {
        task_is_executable(task)
            && matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
            )
            && !task_failure_is_dependency_blocked(task)
            && task_failure_is_transient(task)
    })
}

fn reset_attention_tasks_for_retry(run: &mut AgenticBoard) -> usize {
    let ids = run
        .tasks
        .iter()
        .filter(|task| {
            task_is_executable(task)
                && matches!(
                    canonical_task_status(&task.status),
                    TASK_STATUS_BLOCKED | TASK_STATUS_FAILED
                )
                && !task_failure_is_dependency_blocked(task)
                && task_failure_is_transient(task)
        })
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    for task_id in &ids {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == *task_id) {
            record_task_retry(task, RETRY_MODE_TRANSIENT, None, "automatic retry");
        }
    }
    if ids.is_empty() {
        return 0;
    }
    run.status = "running".to_string();
    run.active = true;
    run.loop_started = false;
    run.paused_at = None;
    run.pause_reason = None;
    clear_board_abort_state(run);
    run.current_phase = Some("retry_requested".to_string());
    run.phase_started_at = Some(Utc::now());
    run.phase_details = Some(json!({
        "mode": RETRY_MODE_TRANSIENT,
        "taskIds": ids,
        "reason": "automatic retry",
    }));
    ids.len()
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn parse_optional_scheduled_start(value: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    parse_rfc3339_utc(value)
        .map(Some)
        .ok_or_else(|| bad_request("scheduledStartAt must be a valid RFC3339 timestamp"))
}

fn capture_workspace_snapshot(project_path: &str) -> Value {
    let output = std::process::Command::new("git")
        .arg("status")
        .arg("--short")
        .current_dir(project_path)
        .env("PATH", augmented_user_path())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let mut files = Vec::new();
            for line in String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
            {
                let status = line.get(..2).unwrap_or("").trim();
                let path = line.get(3..).unwrap_or(line).trim();
                extend_workspace_status_entries(project_path, status, path, &mut files);
                if files.len() >= MAX_WORKSPACE_SNAPSHOT_FILES {
                    break;
                }
            }
            if files.len() < MAX_WORKSPACE_SNAPSHOT_FILES {
                if let Ok(tracked_output) = std::process::Command::new("git")
                    .arg("ls-files")
                    .arg("-z")
                    .current_dir(project_path)
                    .env("PATH", augmented_user_path())
                    .output()
                {
                    if tracked_output.status.success() {
                        for path in String::from_utf8_lossy(&tracked_output.stdout)
                            .split('\0')
                            .map(str::trim)
                            .filter(|path| !path.is_empty())
                        {
                            if files
                                .iter()
                                .any(|file| file.get("path").and_then(Value::as_str) == Some(path))
                            {
                                continue;
                            }
                            files.push(json!({
                                "status": "clean",
                                "path": path,
                                "hash": hash_workspace_file(project_path, path),
                            }));
                            if files.len() >= MAX_WORKSPACE_SNAPSHOT_FILES {
                                break;
                            }
                        }
                    }
                }
            }
            let files_by_path = files
                .iter()
                .filter_map(|file| {
                    file.get("path")
                        .and_then(Value::as_str)
                        .map(|path| (path.to_string(), file.clone()))
                })
                .collect::<serde_json::Map<_, _>>();
            json!({
                "provider": "git status --short",
                "isGit": true,
                "files": files,
                "filesByPath": files_by_path,
                "shortStat": git_command_text(project_path, &["diff", "--shortstat"]),
                "stagedShortStat": git_command_text(project_path, &["diff", "--cached", "--shortstat"]),
                "capturedAt": Utc::now(),
            })
        }
        Ok(output) => json!({
            "provider": "git status --short",
            "isGit": false,
            "files": [],
            "filesByPath": {},
            "error": String::from_utf8_lossy(&output.stderr).trim(),
            "capturedAt": Utc::now(),
        }),
        Err(error) => json!({
            "provider": "git status --short",
            "isGit": false,
            "files": [],
            "filesByPath": {},
            "error": error.to_string(),
            "capturedAt": Utc::now(),
        }),
    }
}

fn extend_workspace_status_entries(
    project_path: &str,
    status: &str,
    path: &str,
    files: &mut Vec<Value>,
) {
    let clean_path = workspace_status_clean_path(path);
    let root = Path::new(project_path);
    let absolute = root.join(&clean_path);
    if absolute.is_dir() {
        let remaining = MAX_WORKSPACE_SNAPSHOT_FILES.saturating_sub(files.len());
        for entry in WalkDir::new(&absolute)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !should_skip_path(entry.path(), root))
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .take(remaining)
        {
            let path = relative_display(root, entry.path());
            files.push(json!({
                "status": status,
                "path": path,
                "hash": hash_workspace_file(project_path, &path),
            }));
        }
        return;
    }
    files.push(json!({
        "status": status,
        "path": clean_path,
        "hash": hash_workspace_file(project_path, &clean_path),
    }));
}

fn workspace_status_clean_path(path: &str) -> String {
    path.split(" -> ")
        .last()
        .unwrap_or(path)
        .trim()
        .trim_matches('"')
        .to_string()
}

fn hash_workspace_file(project_path: &str, path: &str) -> Option<String> {
    let clean_path = workspace_status_clean_path(path);
    let absolute = Path::new(project_path).join(clean_path);
    fs::read(absolute).ok().map(|bytes| sha256_hex(&bytes))
}

fn git_command_text(project_path: &str, args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .current_dir(project_path)
        .env("PATH", augmented_user_path())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

fn read_agents_context(project_path: &str) -> Value {
    let root = Path::new(project_path);
    let files = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !should_skip_path(entry.path(), root))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "AGENTS.md")
        .take(20)
        .filter_map(|entry| {
            fs::read_to_string(entry.path()).ok().map(|content| {
                json!({
                    "path": relative_display(root, entry.path()),
                    "content": limit_text(&content, 12_000),
                    "sha256": sha256_hex(content.as_bytes()),
                })
            })
        })
        .collect::<Vec<_>>();
    json!({
        "files": files,
        "loadedAt": Utc::now(),
    })
}

fn local_codebase_snapshot(run: &AgenticBoard) -> Value {
    let files = run
        .codebase_manifest
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    let config_files = files
        .iter()
        .filter(|file| is_config_file(file))
        .cloned()
        .collect::<Vec<_>>();
    let top_level = files
        .iter()
        .filter_map(|file| {
            file.split('/')
                .next()
                .filter(|part| !part.is_empty())
                .map(str::to_string)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let package_json = fs::read_to_string(Path::new(&run.project_path).join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    json!({
        "packageManager": package_json.as_ref().and_then(|value| value.get("packageManager")).and_then(Value::as_str).unwrap_or(if package_json.is_some() { "npm-compatible" } else { "" }),
        "scripts": package_json.as_ref().and_then(|value| value.get("scripts")).cloned().unwrap_or_else(|| json!({})),
        "dependencies": merge_dependencies(package_json.as_ref()),
        "topLevel": top_level,
        "configFiles": config_files,
        "fileCount": files.len(),
        "files": files.into_iter().take(500).collect::<Vec<_>>(),
        "fileListTruncated": run.codebase_manifest.len() > 500,
    })
}

fn environment_from_codebase_map(codebase_map: &Value) -> Value {
    json!({
        "runCommands": normalize_string_list(codebase_map.get("runCommands")),
        "testCommands": normalize_string_list(codebase_map.get("testCommands")),
        "packageManager": codebase_map
            .get("localSnapshot")
            .and_then(|value| value.get("packageManager"))
            .and_then(Value::as_str)
            .unwrap_or(""),
        "updatedAt": Utc::now(),
    })
}

fn merge_dependencies(package_json: Option<&Value>) -> Value {
    let mut map = serde_json::Map::new();
    for key in ["dependencies", "devDependencies"] {
        if let Some(object) = package_json
            .and_then(|value| value.get(key))
            .and_then(Value::as_object)
        {
            for (name, version) in object {
                map.insert(name.clone(), version.clone());
            }
        }
    }
    Value::Object(map)
}

fn should_skip_path(path: &Path, root: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(
            name.as_ref(),
            ".git"
                | "node_modules"
                | "target"
                | "dist"
                | "dist-server"
                | "build"
                | "coverage"
                | ".next"
                | ".nuxt"
                | ".gradle"
                | ".idea"
                | ".DS_Store"
        )
    })
}

fn should_chunk_codebase_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".lock")
        || lower.ends_with("package-lock.json")
        || lower.ends_with("yarn.lock")
        || lower.ends_with("pnpm-lock.yaml")
        || lower.ends_with(".min.js")
        || lower.ends_with(".map")
        || lower.contains("/generated/")
    {
        return false;
    }
    is_candidate_text_path(Path::new(path))
}

fn is_candidate_text_path(path: &Path) -> bool {
    let Some(ext) = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
    else {
        return path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "Dockerfile" | "Makefile" | "AGENTS.md"));
    };
    matches!(
        ext.as_str(),
        "rs" | "kt"
            | "kts"
            | "java"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "mjs"
            | "cjs"
            | "json"
            | "jsonc"
            | "toml"
            | "yaml"
            | "yml"
            | "md"
            | "html"
            | "css"
            | "scss"
            | "xml"
            | "gradle"
            | "properties"
            | "sh"
            | "py"
            | "go"
            | "sql"
            | "txt"
            | "env"
            | "sample"
            | "swift"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
    )
}

fn is_config_file(path: &str) -> bool {
    let file = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    matches!(
        file,
        "package.json"
            | "tsconfig.json"
            | "vite.config.ts"
            | "vite.config.js"
            | "next.config.js"
            | "Cargo.toml"
            | "pyproject.toml"
            | "requirements.txt"
            | "go.mod"
            | "pom.xml"
            | "build.gradle"
            | "settings.gradle"
            | "Dockerfile"
    )
}

fn looks_textual(bytes: &[u8]) -> bool {
    if bytes.len() > 1_000_000 || bytes.contains(&0) {
        return false;
    }
    std::str::from_utf8(bytes).is_ok()
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn limit_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut output = text
        .chars()
        .take(max_chars.saturating_sub(32))
        .collect::<String>();
    output.push_str("\n...[truncated]");
    output
}

fn set_phase(run: &mut AgenticBoard, phase: &str, details: Value) {
    run.current_phase = Some(phase.to_string());
    run.phase_started_at = Some(Utc::now());
    run.phase_details = Some(details);
}

fn latest_backlog_breakdown_prompt(run: &AgenticBoard) -> Option<String> {
    run.backlog_breakdown
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn latest_backlog_breakdown_provider(run: &AgenticBoard) -> Option<String> {
    run.backlog_breakdown
        .get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn task_scope_text(task: &BoardTask) -> String {
    let details = if task.details.trim().is_empty() {
        task.description.trim()
    } else {
        task.details.trim()
    };
    let prompt = task.prompt.trim();
    let mut parts = vec![format!("{}: {}", task.id, task.title.trim())];
    if !details.is_empty() {
        parts.push(format!("Details: {details}"));
    }
    if !prompt.is_empty() && prompt != details {
        parts.push(format!("Source prompt: {prompt}"));
    }
    parts.join("\n")
}

fn active_board_prompt(run: &AgenticBoard) -> String {
    let selected_scopes = run
        .tasks
        .iter()
        .filter(|task| is_user_authored_task(task))
        .filter(|task| {
            matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS
            )
        })
        .map(task_scope_text)
        .filter(|scope| !scope.trim().is_empty())
        .collect::<Vec<_>>();
    if !selected_scopes.is_empty() {
        return format!("Selected Kanban tasks:\n{}", selected_scopes.join("\n\n"));
    }
    let active_history_scopes = run
        .tasks
        .iter()
        .filter(|task| is_user_authored_task(task))
        .filter(|task| canonical_task_status(&task.status) != TASK_STATUS_BACKLOG)
        .map(task_scope_text)
        .filter(|scope| !scope.trim().is_empty())
        .collect::<Vec<_>>();
    if !active_history_scopes.is_empty() {
        return format!(
            "Selected Kanban tasks:\n{}",
            active_history_scopes.join("\n\n")
        );
    }
    latest_backlog_breakdown_prompt(run).unwrap_or_else(|| run.source_prompt.clone())
}

fn effective_provider_for_phase(run: &AgenticBoard, label: &str) -> Result<String> {
    if model_type_for_phase(label) == "breakdown" {
        if let Some(provider) = latest_backlog_breakdown_provider(run) {
            return normalize_provider(Some(&provider));
        }
        return normalize_provider(Some(DEFAULT_BREAKDOWN_PROVIDER));
    }
    normalize_provider(Some(&run.provider))
}

fn default_model_for_provider(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "claude" => DEFAULT_MODEL.to_string(),
        "cursor" => "gpt-5.3-codex".to_string(),
        _ => String::new(),
    }
}

fn default_model_for_phase(run: &AgenticBoard, label: &str) -> String {
    if model_type_for_phase(label) == "breakdown" {
        let provider = effective_provider_for_phase(run, label)
            .unwrap_or_else(|_| DEFAULT_BREAKDOWN_PROVIDER.to_string());
        if provider == DEFAULT_BREAKDOWN_PROVIDER {
            return DEFAULT_BREAKDOWN_MODEL.to_string();
        }
        return default_model_for_provider(&provider);
    }
    default_model_for_provider(&run.provider)
}

fn effective_model_for_phase(run: &AgenticBoard, label: &str) -> String {
    let phase_type = model_type_for_phase(label);
    let phase_key = label.replace(' ', "_");
    let configured = run
        .task_model_overrides
        .get(&phase_key)
        .or_else(|| run.task_model_overrides.get(label))
        .or_else(|| run.task_model_overrides.get(model_type_for_phase(label)))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|model| !model.trim().is_empty())
        .or_else(|| {
            (model_type_for_phase(label) == "breakdown")
                .then(|| latest_backlog_breakdown_model(run))
                .flatten()
        });
    configured
        .or_else(|| {
            run.model_strategy
                .as_ref()
                .and_then(|strategy| strategy.get("model"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|model| !model.trim().is_empty())
        })
        .or_else(|| {
            let board_model = trim_string(Some(run.model.clone()))?;
            let board_default = default_model_for_provider(&run.provider);
            (board_model != board_default).then_some(board_model)
        })
        .unwrap_or_else(|| {
            if phase_type == "breakdown" {
                default_model_for_phase(run, label)
            } else {
                trim_string(Some(run.model.clone()))
                    .unwrap_or_else(|| default_model_for_phase(run, label))
            }
        })
}

fn latest_backlog_breakdown_model(run: &AgenticBoard) -> Option<String> {
    run.backlog_breakdown
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty() && *model != "provider default")
        .map(str::to_string)
}

fn effective_model_for_task(run: &AgenticBoard, task: &BoardTask) -> String {
    run.task_model_overrides
        .get(&task.id)
        .or_else(|| run.task_model_overrides.get(model_type_for_task(task)))
        .or_else(|| run.task_model_overrides.get(&task.task_type))
        .or_else(|| run.task_model_overrides.get("task_execution"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|model| !model.trim().is_empty())
        .or_else(|| {
            run.model_strategy
                .as_ref()
                .and_then(|strategy| strategy.get("taskModel"))
                .or_else(|| {
                    run.model_strategy
                        .as_ref()
                        .and_then(|strategy| strategy.get("model"))
                })
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|model| !model.trim().is_empty())
        })
        .unwrap_or_else(|| default_model_for_provider(&run.provider))
}

fn agentic_execution_model_for_provider(provider: &str, model: &str) -> String {
    let trimmed = model.trim();
    if !provider.eq_ignore_ascii_case("claude") || trimmed.is_empty() {
        return trimmed.to_string();
    }
    let normalized = trimmed.to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "minimax-m3" | "minimaxm3" => "min:MiniMax-M3".to_string(),
        _ => trimmed.to_string(),
    }
}

fn model_type_for_phase(label: &str) -> &'static str {
    let normalized = label.trim().to_ascii_lowercase();
    if normalized.contains("final review") || normalized.contains("final qa") {
        "final_qa"
    } else if normalized.contains("result schema repair") {
        "qa_fix"
    } else if normalized.contains("qa") || normalized.contains("promotion review") {
        "qa"
    } else {
        "breakdown"
    }
}

fn model_type_for_task(task: &BoardTask) -> &'static str {
    if task.final_qa_task {
        "final_qa"
    } else if task.qa_verdict_retry_task || task.qa_task || task.task_level_qa {
        "qa"
    } else if task.qa_fix_task || task.source_qa_task_id.is_some() {
        "qa_fix"
    } else if task.agents_knowledge_task || task.task_type == "agents_knowledge" {
        "agents"
    } else if canonical_task_kind(task) == TASK_KIND_MANUAL_TEST {
        "qa"
    } else {
        "implementation"
    }
}

fn collect_task_models(run: &AgenticBoard, overrides: Option<&Value>) -> BTreeSet<String> {
    let fallback = trim_string(Some(if run.primary_model.trim().is_empty() {
        run.model.clone()
    } else {
        run.primary_model.clone()
    }))
    .unwrap_or_default();
    let overrides = overrides.unwrap_or(&run.task_model_overrides);
    [
        "breakdown",
        "implementation",
        "qa",
        "qa_fix",
        "agents",
        "final_qa",
    ]
    .into_iter()
    .filter_map(|key| {
        overrides
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_string)
            .or_else(|| (!fallback.is_empty()).then(|| fallback.clone()))
    })
    .collect()
}

fn has_mixed_task_models(run: &AgenticBoard, overrides: Option<&Value>) -> bool {
    collect_task_models(run, overrides).len() > 1
}

fn sync_session_policy_with_task_models(run: &mut AgenticBoard, source: &str) -> bool {
    if normalize_session_policy(Some(&run.session_policy)) != "continuous" {
        return false;
    }
    if !has_mixed_task_models(run, None) {
        return false;
    }
    run.session_policy = "task-model".to_string();
    run.append_log(format!(
        "Session policy set to task-model from {source} because task routing uses multiple models"
    ));
    true
}

fn apply_task_model_routing(run: &mut AgenticBoard, task_index: usize) {
    let Some(task) = run.tasks.get(task_index) else {
        return;
    };
    let desired_model = effective_model_for_task(run, task);
    if desired_model.trim().is_empty() || desired_model == run.model {
        return;
    }
    if normalize_session_policy(Some(&run.session_policy)) == "continuous"
        && run.actual_session_id.is_some()
    {
        run.append_log(format!(
            "Continuous provider session kept current model for {}; configured task model {} will apply after pause/resume",
            task.id, desired_model
        ));
        return;
    }
    let previous = run.model.clone();
    run.model = desired_model.clone();
    run.next_model = desired_model.clone();
    run.actual_session_id = None;
    run.current_provider_session_id = None;
    run.model_history.push(json!({
        "from": previous,
        "to": desired_model,
        "changedAt": Utc::now(),
        "changedBy": "task-model-routing",
        "taskId": task.id,
    }));
}

fn normalize_session_policy(policy: Option<&str>) -> String {
    let normalized = policy
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase().replace(' ', "_"));
    match normalized.as_deref() {
        Some("continuous") | Some("single") | Some("one-session") | Some("one_session") => {
            "continuous".to_string()
        }
        Some("task-model")
        | Some("task_model")
        | Some("per-task")
        | Some("per_task")
        | Some("per-task-model")
        | Some("per_task_model") => "task-model".to_string(),
        _ if danger_continuous_session_default() => "continuous".to_string(),
        _ => "task-model".to_string(),
    }
}

fn danger_continuous_session_default() -> bool {
    env::var("DANGER_CONTINUOUS_SESSION")
        .or_else(|_| env::var("IO_WORKBENCH_DANGER_CONTINUOUS_SESSION"))
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "no"
            )
        })
        .unwrap_or(true)
}

fn increment_provider_usage(
    run: &mut AgenticBoard,
    prompt: &str,
    output: &str,
    session_id: Option<&str>,
    actual_usage: Option<&Value>,
) {
    let estimated = json!({
        "inputTokens": estimate_tokens(prompt) as u64,
        "cachedInputTokens": 0,
        "outputTokens": estimate_tokens(output) as u64,
        "totalTokens": (estimate_tokens(prompt) + estimate_tokens(output)) as u64,
        "cumulative": false,
    });
    let usage_source = actual_usage.cloned().unwrap_or(estimated);
    let session_key = session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let previous_session_usage = run
        .provider_usage_by_session
        .get(session_key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let cumulative = usage_source
        .get("cumulative")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut delta_map = serde_json::Map::new();
    for key in [
        "inputTokens",
        "cachedInputTokens",
        "outputTokens",
        "totalTokens",
    ] {
        let current = usage_source.get(key).and_then(Value::as_u64).unwrap_or(0);
        let value = if cumulative {
            let previous = previous_session_usage
                .get(key)
                .and_then(Value::as_u64)
                .unwrap_or(0);
            current.saturating_sub(previous)
        } else {
            current
        };
        delta_map.insert(key.to_string(), json!(value));
    }
    delta_map.insert("invocationsWithUsage".to_string(), json!(1));
    let delta = Value::Object(delta_map);
    let mut usage = run.provider_usage.as_object().cloned().unwrap_or_default();
    for key in [
        "inputTokens",
        "cachedInputTokens",
        "outputTokens",
        "totalTokens",
        "invocationsWithUsage",
    ] {
        let value = delta.get(key).and_then(Value::as_u64).unwrap_or(0);
        let next = usage.get(key).and_then(Value::as_u64).unwrap_or(0) + value;
        usage.insert(key.to_string(), json!(next));
    }
    run.provider_usage = Value::Object(usage);

    if !session_key.is_empty() {
        let mut by_session = run
            .provider_usage_by_session
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut session_usage = if cumulative {
            usage_source.as_object().cloned().unwrap_or_default()
        } else {
            by_session
                .get(session_key)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        };
        for key in [
            "inputTokens",
            "cachedInputTokens",
            "outputTokens",
            "totalTokens",
            "invocationsWithUsage",
        ] {
            if cumulative && key != "invocationsWithUsage" {
                session_usage
                    .entry(key.to_string())
                    .or_insert_with(|| usage_source.get(key).cloned().unwrap_or(json!(0)));
                continue;
            }
            let value = delta.get(key).and_then(Value::as_u64).unwrap_or(0);
            let next = session_usage.get(key).and_then(Value::as_u64).unwrap_or(0) + value;
            session_usage.insert(key.to_string(), json!(next));
        }
        by_session.insert(session_key.to_string(), Value::Object(session_usage));
        run.provider_usage_by_session = Value::Object(by_session);
    }
}

fn estimate_tokens(text: &str) -> usize {
    (text.chars().count() / 4).max(1)
}

fn prompt_telemetry_summary(entries: &[Value]) -> Value {
    let calls = entries.len();
    let chars = entries
        .iter()
        .filter_map(|entry| entry.get("chars").and_then(Value::as_u64))
        .sum::<u64>();
    let estimated_tokens = entries
        .iter()
        .filter_map(|entry| entry.get("estimatedTokens").and_then(Value::as_u64))
        .sum::<u64>();
    let actual_input_tokens = telemetry_token_sum(entries, "actualInputTokens");
    let actual_cached_input_tokens = telemetry_token_sum(entries, "actualCachedInputTokens");
    let actual_output_tokens = telemetry_token_sum(entries, "actualOutputTokens");
    let actual_tokens = telemetry_token_sum(entries, "actualTotalTokens");
    let mut by_phase = BTreeMap::<String, (usize, u64, u64, u64)>::new();
    for entry in entries {
        let phase = entry
            .get("phase")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("unknown")
            .to_string();
        let accumulator = by_phase.entry(phase).or_default();
        accumulator.0 += 1;
        accumulator.1 += entry.get("chars").and_then(Value::as_u64).unwrap_or(0);
        accumulator.2 += entry
            .get("estimatedTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        accumulator.3 += entry
            .get("actualTotalTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
    }
    let mut phases = by_phase
        .into_iter()
        .map(|(phase, (calls, chars, estimated_tokens, actual_tokens))| {
            json!({
                "phase": phase,
                "calls": calls,
                "chars": chars,
                "estimatedTokens": estimated_tokens,
                "actualTokens": actual_tokens,
            })
        })
        .collect::<Vec<_>>();
    phases.sort_by_key(|phase| {
        std::cmp::Reverse(
            phase
                .get("estimatedTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        )
    });
    let largest_call = entries
        .iter()
        .max_by_key(|entry| {
            entry
                .get("estimatedTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        })
        .cloned();
    json!({
        "calls": calls,
        "chars": chars,
        "estimatedTokens": estimated_tokens,
        "actualInputTokens": actual_input_tokens,
        "actualCachedInputTokens": actual_cached_input_tokens,
        "actualOutputTokens": actual_output_tokens,
        "actualTokens": actual_tokens,
        "phases": phases,
        "largestCall": largest_call,
    })
}

fn telemetry_token_sum(entries: &[Value], key: &str) -> u64 {
    entries
        .iter()
        .filter_map(|entry| entry.get(key).and_then(Value::as_u64))
        .sum()
}

fn validation_summary(entries: &[Value]) -> Value {
    let runs = entries.len();
    let passed = entries
        .iter()
        .filter(|entry| entry.get("passed").and_then(Value::as_bool) == Some(true))
        .count();
    let latest = entries.last();
    let commands = entries.iter().map(validation_command_count).sum::<usize>();
    json!({
        "runs": runs,
        "passed": passed,
        "failed": runs.saturating_sub(passed),
        "latestStage": latest
            .and_then(|entry| entry.get("stage"))
            .and_then(Value::as_str)
            .unwrap_or(""),
        "latestPassed": latest
            .and_then(|entry| entry.get("passed"))
            .and_then(Value::as_bool),
        "commands": commands,
    })
}

fn validation_command_count(entry: &Value) -> usize {
    if let Some(commands) = entry.get("commands").and_then(Value::as_array) {
        return commands.len();
    }
    if entry
        .get("commands")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return 1;
    }
    usize::from(
        entry
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty()),
    )
}

fn server_error_message(error: &ServerError) -> String {
    match error.body.details.as_deref() {
        Some(details) if !details.is_empty() => format!("{}: {details}", error.body.error),
        _ => error.body.error.clone(),
    }
}

fn apply_board_options(run: &mut AgenticBoard, request: &CreateBoardRequest) -> Result<()> {
    if let Some(provider) = request.provider.as_deref() {
        run.provider = normalize_provider(Some(provider))?;
    }
    if let Some(model) = trim_string(request.model.clone()) {
        run.model = model.clone();
        run.primary_model = model;
    }
    if request.next_model.is_some() {
        run.next_model = trim_string(request.next_model.clone()).unwrap_or_default();
    }
    if request.next_provider.is_some() {
        run.next_provider = normalize_optional_provider(request.next_provider.as_deref())?;
    }
    if request.model_strategy.is_some() {
        run.model_strategy = normalize_model_strategy(request.model_strategy.clone());
        let strategy_overrides = task_model_overrides_for_strategy(run.model_strategy.as_ref());
        if !json_object_is_empty(&strategy_overrides) {
            run.task_model_overrides =
                merge_task_model_overrides(strategy_overrides, run.task_model_overrides.clone());
        }
        if let Some(model) = primary_model_for_strategy(run.model_strategy.as_ref()) {
            run.primary_model = model.clone();
            run.model = model;
        }
    }
    if let Some(profile) = trim_string(request.board_profile.clone()) {
        run.board_profile = normalize_board_profile(Some(&profile));
    }
    if let Some(overrides) = request.task_model_overrides.clone() {
        let strategy_overrides = task_model_overrides_for_strategy(run.model_strategy.as_ref());
        run.task_model_overrides = merge_task_model_overrides(
            strategy_overrides,
            normalize_task_model_overrides(overrides),
        );
    }
    if let Some(policy) = request.session_policy.as_deref() {
        run.session_policy = normalize_session_policy(Some(policy));
    }
    if request.git_policy.is_some() {
        run.git_policy = normalize_git_policy(request.git_policy.as_deref());
    }
    if request.tools_settings.is_some() {
        run.tools_settings = request.tools_settings.clone();
    }
    if let Some(enabled) = request.tdd_enabled {
        run.tdd_enabled = enabled;
    }
    if request.tdd_policy.is_some() {
        run.tdd_policy = normalize_tdd_policy(request.tdd_policy.as_ref());
    }
    if request.validation_config.is_some() {
        run.validation_config = normalize_validation_config(request.validation_config.as_ref());
    }
    if request.rag_settings.is_some() {
        run.rag_settings = normalize_rag_settings(request.rag_settings.as_ref());
        run.rag_enabled = rag_enabled_from_settings(&run.rag_settings);
    }
    if request.qa_policy.is_some() {
        run.qa_policy = normalize_qa_policy(request.qa_policy.as_ref());
    }
    if request.auto_retry.is_some() {
        run.auto_retry = normalize_auto_retry(request.auto_retry.as_ref().unwrap_or(&Value::Null));
    }
    sync_session_policy_with_task_models(run, "board option update");
    Ok(())
}

#[derive(Debug)]
struct StoredBoard {
    path: PathBuf,
    board: AgenticBoard,
}

fn load_user_board(state: &AppState, user_id: &str, id: &str) -> Result<StoredBoard> {
    load_boards(state)?
        .into_iter()
        .find(|stored| stored.board.id == id && stored.board.user_id.as_deref() == Some(user_id))
        .ok_or_else(|| not_found("Agentic board not found"))
}

fn latest_board_for_project(
    state: &AppState,
    user_id: &str,
    project_path: &str,
) -> Result<Option<StoredBoard>> {
    let mut boards = load_boards(state)?
        .into_iter()
        .filter(|stored| {
            stored.board.user_id.as_deref() == Some(user_id)
                && stored.board.project_path == project_path
        })
        .collect::<Vec<_>>();
    boards.sort_by(|left, right| right.board.updated_at.cmp(&left.board.updated_at));
    Ok(boards.into_iter().next())
}

fn load_boards(state: &AppState) -> Result<Vec<StoredBoard>> {
    let dir = boards_dir(state);
    load_boards_from_dir(&dir)
}

fn load_boards_from_dir(dir: &Path) -> Result<Vec<StoredBoard>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut boards = Vec::new();
    for entry in fs::read_dir(&dir).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let content = fs::read_to_string(&path).map_err(io_error)?;
        match serde_json::from_str::<AgenticBoard>(&content) {
            Ok(mut run) => {
                normalize_board_model(&mut run);
                boards.push(StoredBoard { path, board: run });
            }
            Err(error) => {
                tracing::warn!(file = %path.display(), %error, "failed to read agentic board snapshot");
            }
        }
    }
    Ok(boards)
}

fn save_board(state: &AppState, run: &AgenticBoard) -> Result<()> {
    let _guard = BOARD_SAVE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = boards_dir(state);
    fs::create_dir_all(&dir).map_err(io_error)?;
    let path = board_file_path(state, run);
    let temp_path = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    let mut run_to_save = run.clone();
    if let Ok(content) = fs::read_to_string(&path)
        && let Ok(current) = serde_json::from_str::<AgenticBoard>(&content)
    {
        preserve_newer_control_state(&mut run_to_save, &current);
    }
    normalize_board_model(&mut run_to_save);
    let content = serde_json::to_string_pretty(&run_to_save).map_err(|error| {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to serialize board",
            error.to_string(),
        )
    })?;
    fs::write(&temp_path, content).map_err(io_error)?;
    fs::rename(&temp_path, &path).map_err(io_error)?;
    Ok(())
}

fn preserve_newer_control_state(run: &mut AgenticBoard, current: &AgenticBoard) {
    if current.control_revision <= run.control_revision {
        return;
    }
    run.control_revision = current.control_revision;
    run.status = current.status.clone();
    run.active = current.active;
    run.loop_started = current.loop_started;
    run.auto_run_enabled = current.auto_run_enabled;
    run.pause_requested = current.pause_requested;
    run.paused_at = current.paused_at;
    run.pause_reason = current.pause_reason.clone();
    run.cancellation_reason = current.cancellation_reason.clone();
    run.abort_source = current.abort_source.clone();
    run.abort_requested_at = current.abort_requested_at;
    run.canceled_at = current.canceled_at;
    run.scheduled_start_at = current.scheduled_start_at;
    if matches!(current.status.as_str(), "paused" | "pausing" | "cancelled") {
        run.current_task_id = current.current_task_id.clone();
        run.current_task_title = current.current_task_title.clone();
        run.current_task_status = current.current_task_status.clone();
        run.current_provider_session_id = current.current_provider_session_id.clone();
        run.provider_call_started_at = current.provider_call_started_at;
        run.provider_call_label = current.provider_call_label.clone();
        for current_task in &current.tasks {
            let Some(task) = run.tasks.iter_mut().find(|task| task.id == current_task.id) else {
                continue;
            };
            if task_status_is_active(&task.status) && !task_status_is_active(&current_task.status) {
                task.status = canonical_task_status(&current_task.status).to_string();
                task.started_at = current_task.started_at;
                task.completed_at = current_task.completed_at;
                for entry in &current_task.transcript {
                    if !task.transcript.contains(entry) {
                        task.transcript.push(entry.clone());
                    }
                }
                task.transcript_updated_at = current_task.transcript_updated_at;
            }
        }
    }
    for log in &current.logs {
        if !run.logs.contains(log) {
            run.logs.push(log.clone());
        }
    }
    if run.logs.len() > 500 {
        let remove_count = run.logs.len() - 500;
        run.logs.drain(0..remove_count);
    }
}

fn board_mutation_lock() -> MutexGuard<'static, ()> {
    BOARD_MUTATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn boards_dir(state: &AppState) -> PathBuf {
    state.config.config_dir.join(BOARD_STORAGE_DIR)
}

fn board_file_path(state: &AppState, run: &AgenticBoard) -> PathBuf {
    boards_dir(state).join(format!(
        "{}.json",
        board_storage_key(run.user_id.as_deref(), &run.project_path)
    ))
}

fn board_storage_key(user_id: Option<&str>, project_path: &str) -> String {
    sha256_hex(format!("{}\0{}", user_id.unwrap_or("anonymous"), project_path).as_bytes())
}

#[cfg(test)]
fn prompt_to_task_drafts(run: &mut AgenticBoard, prompt: &str) -> Vec<BoardTask> {
    let mut tasks = prompt
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(12)
        .map(|line| {
            let title = line
                .trim_start_matches(['-', '*', '•'])
                .trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.' || ch == ')')
                .trim()
                .to_string();
            BoardTask::draft(
                run,
                title_from_prompt(&title).unwrap_or_else(|| "New board task".to_string()),
                line.to_string(),
            )
        })
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        tasks.push(BoardTask::draft(
            run,
            title_from_prompt(prompt).unwrap_or_else(|| "New board task".to_string()),
            prompt.to_string(),
        ));
    }
    tasks
}

#[derive(Debug)]
struct PromptTaskDraftAttempt {
    result: Result<(Vec<Value>, Option<String>)>,
    provider_prompt: String,
    provider_output: String,
    session_id: Option<String>,
    token_usage: Option<Value>,
    effective_provider: String,
    effective_model: String,
    started_at: DateTime<Utc>,
}

async fn generate_prompt_task_drafts(
    state: &AppState,
    run: &AgenticBoard,
    prompt: &str,
    provider: Option<&str>,
    model: Option<&str>,
    board_profile: Option<&str>,
) -> PromptTaskDraftAttempt {
    let profile = board_profile
        .map(|value| normalize_board_profile(Some(value)))
        .unwrap_or_else(|| normalize_board_profile(Some(&run.board_profile)));
    let selected_provider = provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| normalize_provider(Some(value)))
        .transpose();
    let selected_provider = match selected_provider {
        Ok(Some(provider)) => provider,
        Ok(None) => DEFAULT_BREAKDOWN_PROVIDER.to_string(),
        Err(error) => {
            return PromptTaskDraftAttempt {
                result: Err(error),
                provider_prompt: String::new(),
                provider_output: String::new(),
                session_id: None,
                token_usage: None,
                effective_provider: DEFAULT_BREAKDOWN_PROVIDER.to_string(),
                effective_model: model
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or(DEFAULT_BREAKDOWN_MODEL)
                    .to_string(),
                started_at: Utc::now(),
            };
        }
    };
    let selected_model = model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            run.task_model_overrides
                .get("breakdown")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            let board_model = trim_string(Some(run.primary_model.clone()))
                .or_else(|| trim_string(Some(run.model.clone())))?;
            let board_default = default_model_for_provider(&run.provider);
            (board_model != board_default).then_some(board_model)
        })
        .unwrap_or_else(|| {
            if selected_provider == DEFAULT_BREAKDOWN_PROVIDER {
                DEFAULT_BREAKDOWN_MODEL.to_string()
            } else {
                default_model_for_provider(&selected_provider)
            }
        });
    let mut generation_run = run.clone();
    generation_run.provider = selected_provider.clone();
    generation_run.board_profile = profile.clone();
    generation_run.actual_session_id = None;
    generation_run.current_provider_session_id = None;
    generation_run.session_id = None;
    generation_run.current_task_id = run.current_task_id.clone().or_else(|| {
        run.backlog_breakdown
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    if !selected_model.trim().is_empty() {
        generation_run.model = selected_model.clone();
        generation_run.primary_model = selected_model.clone();
    }
    let provider_prompt = build_prompt_task_draft_prompt(&generation_run, prompt, &profile);
    let started_at = Utc::now();
    let provider_result = execute_shared_provider_turn(
        state,
        &generation_run,
        &selected_provider,
        &selected_model,
        &provider_prompt,
        None,
        generation_run.current_task_id.as_deref(),
    )
    .await;
    match provider_result {
        Ok(result) if result.exit_code == 0 => {
            let parsed = parse_json_object(&result.assistant_text).unwrap_or_else(|| json!({}));
            let tasks = sanitize_prompt_task_drafts(&parsed, prompt);
            let generation_result = if tasks.is_empty() {
                Err(task_generation_error(
                    "AI returned valid output but no usable task drafts.",
                ))
            } else {
                Ok((tasks, None))
            };
            PromptTaskDraftAttempt {
                result: generation_result,
                provider_prompt,
                provider_output: result.assistant_text,
                session_id: Some(result.session_id),
                token_usage: result.token_usage,
                effective_provider: selected_provider,
                effective_model: selected_model,
                started_at,
            }
        }
        Ok(result) => PromptTaskDraftAttempt {
            result: Err(task_generation_error(format!(
                "AI task generation failed: {}",
                result.summary
            ))),
            provider_prompt,
            provider_output: result.assistant_text,
            session_id: Some(result.session_id),
            token_usage: result.token_usage,
            effective_provider: selected_provider,
            effective_model: selected_model,
            started_at,
        },
        Err(error) => PromptTaskDraftAttempt {
            result: Err(task_generation_error(format!(
                "AI task generation failed: {}",
                server_error_message(&error)
            ))),
            provider_prompt,
            provider_output: String::new(),
            session_id: None,
            token_usage: None,
            effective_provider: selected_provider,
            effective_model: selected_model,
            started_at,
        },
    }
}

fn record_prompt_task_generation_attempt(
    run: &mut AgenticBoard,
    label: &str,
    attempt: &PromptTaskDraftAttempt,
) {
    let controls = board_provider_controls(run);
    let mut telemetry = json!({
        "phase": "backlog_generation",
        "label": label,
        "provider": attempt.effective_provider,
        "chars": attempt.provider_prompt.chars().count(),
        "estimatedTokens": estimate_tokens(&attempt.provider_prompt),
        "startedAt": attempt.started_at,
        "reasoningEffort": controls.effort,
        "fast": controls.fast,
    });
    if let Some(error) = attempt.result.as_ref().err() {
        telemetry["error"] = json!(server_error_message(error));
        telemetry["outcome"] = json!("failed");
    } else {
        telemetry["outcome"] = json!("completed");
    }
    run.prompt_telemetry.push(telemetry);
    let telemetry_index = run.prompt_telemetry.len().saturating_sub(1);
    finalize_prompt_telemetry(
        run,
        telemetry_index,
        attempt.session_id.as_deref(),
        Some(&attempt.effective_model),
        attempt.token_usage.as_ref(),
    );
    if attempt.session_id.is_some() || !attempt.provider_output.is_empty() {
        increment_provider_usage(
            run,
            &attempt.provider_prompt,
            &attempt.provider_output,
            attempt.session_id.as_deref(),
            attempt.token_usage.as_ref(),
        );
    }
}

fn prompt_task_generation_running_transcript(
    prompt: &str,
    provider: &str,
    model: &str,
    started_at: DateTime<Utc>,
) -> Value {
    json!([
        {
            "timestamp": started_at,
            "kind": "message",
            "role": "user",
            "content": prompt,
        },
        {
            "timestamp": started_at,
            "kind": "status",
            "role": "assistant",
            "provider": provider,
            "model": model,
            "status": "running",
            "content": "Generating backlog tasks from the prompt.",
        }
    ])
}

fn prompt_task_generation_transcript(
    attempt: &PromptTaskDraftAttempt,
    prompt: &str,
    completed: bool,
    fallback_error: Option<&str>,
) -> Value {
    let completed_at = Utc::now();
    let mut assistant_content = attempt.provider_output.trim().to_string();
    if assistant_content.is_empty() {
        assistant_content = fallback_error
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("No provider output captured.")
            .to_string();
    }
    let mut assistant = json!({
        "timestamp": completed_at,
        "kind": if completed { "assistant" } else { "error" },
        "role": "assistant",
        "provider": attempt.effective_provider,
        "model": attempt.effective_model,
        "status": if completed { "completed" } else { "failed" },
        "content": assistant_content,
    });
    if let Some(object) = assistant.as_object_mut() {
        if let Some(session_id) = attempt.session_id.as_deref() {
            object.insert("sessionId".to_string(), json!(session_id));
        }
        if let Some(token_usage) = attempt.token_usage.as_ref() {
            object.insert("tokenUsage".to_string(), token_usage.clone());
        }
        if let Some(error) = fallback_error
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            object.insert("error".to_string(), json!(error));
        }
    }
    json!([
        {
            "timestamp": attempt.started_at,
            "kind": "message",
            "role": "user",
            "content": prompt,
        },
        assistant
    ])
}

fn build_prompt_task_draft_prompt(run: &AgenticBoard, prompt: &str, profile: &str) -> String {
    format!(
        r#"Create implementation-ready Kanban backlog cards for this focused follow-up prompt.

Prompt:
{prompt}

{board_profile_block}

{git_policy_block}

Current ticket scope:
{scope_context}

Existing board items:
{tasks}

Return JSON only. No markdown fence.
Schema:
{{
  "tasks": [
    {{
      "level": "initiative|epic|story",
      "title": "clear planning title",
      "kind": "research|design|review|implementation",
      "details": "scope and user-facing or strategic description",
      "acceptanceCriteria": ["verifiable outcome"],
      "references": ["relevant file or source"],
      "priority": "p0|p1|p2|p3",
      "blockedBy": []
    }}
  ]
}}

Rules:
- Generate only initiative, epic, or story cards directly needed for the prompt-matched feature area.
- The visible Backlog must never contain a top-level task or subtask.
- If the request is tiny, create one small story; do not force an initiative or epic.
- If you think in terms of a task or subtask, wrap it in the smallest useful story.
- Preserve explicit user scope; do not add unrelated cleanup or product ideas.
- Keep every planning card independently understandable and user-approved before execution.
- Do not create executable subtasks during board-level breakdown.
- Nice-to-have ideas must be separate Backlog cards and must not silently become required child work.
- Prefer a small number of complete cards over many vague cards."#,
        prompt = prompt,
        board_profile_block = format_board_profile_block(profile),
        git_policy_block = git_policy_block(run),
        scope_context = "Use ticket descriptions and acceptance criteria as the only scope source. Do not invent a second scope layer.".to_string(),
        tasks = run
            .tasks
            .iter()
            .filter(|task| !task.backlog_generation_task)
            .rev()
            .take(20)
            .map(|task| format!("{} [{}] {}", task.id, task.status, task.title))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn board_profile_block(run: &AgenticBoard) -> String {
    format_board_profile_block(&run.board_profile)
}

fn format_board_profile_block(profile: &str) -> String {
    match normalize_board_profile(Some(profile)).as_str() {
        "minimal" => [
            "Board profile: Minimal",
            "Implement the explicit ticket scope with minimal expansion, low context, and concrete verification.",
            "Scope guidance:",
            "- Preserve every explicit behavior and constraint from the prompt or source documents.",
            "- For broad app-generation prompts, infer only the core product data, CRUD/workflows, persistence, navigation, validation, and runnable local verification needed for the requested app to work.",
            "- Do not add optional dashboards, analytics, roles, integrations, or visual polish unless the prompt or source docs require them.",
            "Planning scope:",
            "- Create the smallest task set that fully satisfies the explicit ticket scope and required local glue.",
            "- Avoid optional enhancement tasks unless they are necessary for correctness or verification.",
            "Execution scope:",
            "- Prefer focused code changes and local verification over broad rewrites.",
            "- Do not expand scope beyond the attached ticket except for necessary wiring, error handling, and tests/checks.",
            "QA scope:",
            "- Verify explicit ticket behavior, necessary inferred glue, and functional happy/error paths.",
            "- Do not fail completion for optional polish or non-required enhancements.",
        ]
        .join("\n"),
        "product_ready" => [
            "Board profile: Product-Ready",
            "Deliver complete workflows with richer product detail, structural UX checks, and stricter edge-case validation.",
            "Scope guidance:",
            "- Preserve every explicit behavior and constraint from the prompt or source documents.",
            "- For broad app-generation prompts, infer a product-ready local app: core data model, CRUD/workflows, persistence, navigation, validation, useful dashboard/summaries, search/filter where useful, empty/loading/error states, responsive structure, and runnable verification.",
            "- Add richer workflow/detail scope only when it directly supports the requested product; do not invent unrelated integrations, payments, enterprise roles, or subjective redesign.",
            "Planning scope:",
            "- Plan the ticket hierarchy into complete user-facing workflows, not only isolated CRUD endpoints.",
            "- Include validation, useful summaries/comparisons, responsive structural checks, and concrete local QA for important flows.",
            "Execution scope:",
            "- Implement complete functional screens, forms, persistence paths, validation feedback, and state handling for the attached product workflow.",
            "- Keep subjective visual polish as backlog unless the task explicitly asks for it, but do not leave structurally broken or unusable UI.",
            "QA scope:",
            "- Verify core workflows plus validation failures, empty states, persisted state, and important responsive structure.",
            "- Fail QA for missing required workflow detail or unusable UI structure, not for subjective aesthetic preferences.",
        ]
        .join("\n"),
        _ => [
            "Board profile: Complete App",
            "Build a complete functional app with common app completeness and bounded token use.",
            "Scope guidance:",
            "- Preserve every explicit behavior and constraint from the prompt or source documents.",
            "- For broad app-generation prompts, infer the common complete-app pieces: core product data model, CRUD/workflows, persistence, navigation, validation, practical empty/error states, basic summaries, responsive layout, and runnable verification.",
            "- Do not invent unrelated integrations, payment flows, enterprise roles, or subjective UI polish unless the ticket requires them.",
            "Planning scope:",
            "- Plan enough tasks to make the requested app complete and locally verifiable without adding optional product expansion.",
            "- Include validation, persistence, and functional UI sanity checks where relevant.",
            "Execution scope:",
            "- Implement complete end-to-end behavior for the attached workflow, including useful validation and state handling.",
            "- Avoid broad redesigns or optional features that are not needed for a correct complete app.",
            "QA scope:",
            "- Verify main workflows, persistence, validation, and functional responsive usability.",
            "- Do not block on subjective polish unless it prevents required workflow use.",
        ]
        .join("\n"),
    }
}

fn git_policy_block(run: &AgenticBoard) -> String {
    match normalize_git_policy(Some(&run.git_policy)).as_str() {
        "managed" => [
            "Git policy: Managed Git Workflow",
            "The orchestrator handles git writes after each verified task group.",
            "- Read-only git inspection commands are allowed.",
            "- Do not run git write commands yourself. Do not create branches, add, commit, merge, rebase, reset, clean, stash, tag, or push from the provider.",
            "- The orchestrator creates a task branch before implementation, then commits, merges to main, and pushes only after the task group is complete and verified.",
            "- If git state prevents the managed workflow, report the blocker clearly instead of trying a manual workaround.",
        ]
        .join("\n"),
        _ => [
            "Git policy: Read-only Git",
            "Provider tasks may inspect git state but must not change git state.",
            "- Read-only git inspection commands are allowed, such as git status, git diff, git log, git show, git branch --show-current, and git remote -v.",
            "- Do not run git write commands: no add, commit, checkout, switch, branch creation/deletion, merge, rebase, reset, restore, clean, stash, tag, or push.",
            "- Do not plan git history tasks. Finish with code/test evidence only.",
        ]
        .join("\n"),
    }
}

fn sanitize_prompt_task_drafts(parsed: &Value, prompt: &str) -> Vec<Value> {
    parsed
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .take(12)
        .filter_map(|task| {
            let title = task
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let details = task
                .get("details")
                .or_else(|| task.get("description"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(title);
            let references = dedupe_strings(
                normalize_string_list(task.get("references"))
                    .into_iter()
                    .chain(normalize_string_list(task.get("files")))
                    .chain(normalize_string_list(task.get("paths")))
                    .collect(),
            );
            let acceptance_criteria = normalize_string_list(
                task.get("acceptanceCriteria")
                    .or_else(|| task.get("acceptance"))
                    .or_else(|| task.get("criteria")),
            );
            let kind = prompt_task_kind_from_value(&task, title, details, &acceptance_criteria);
            let generated_level =
                normalize_task_level(task.get("level").and_then(Value::as_str), TASK_LEVEL_STORY);
            let wrapped_level = if matches!(generated_level, TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK) {
                TASK_LEVEL_STORY
            } else {
                generated_level
            };
            let wrapped_details = if wrapped_level == TASK_LEVEL_STORY
                && matches!(generated_level, TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK)
            {
                format!(
                    "{}

This story wraps the generated {} and keeps executable engineering work nested below the story.",
                    details, generated_level
                )
            } else {
                details.to_string()
            };
            Some(json!({
                "title": title,
                "kind": kind,
                "taskType": kind,
                "level": wrapped_level,
                "sourceLevel": generated_level,
                "sourceKind": kind,
                "executable": false,
                "required": task.get("required").and_then(Value::as_bool).unwrap_or(true),
                "scopeVersion": 1,
                "details": wrapped_details,
                "prompt": prompt,
                "acceptanceCriteria": acceptance_criteria,
                "references": references,
                "priority": normalize_priority(task.get("priority").and_then(Value::as_str)),
                "blockedBy": normalize_string_list(
                    task.get("blockedBy")
                        .or_else(|| task.get("blocked_by"))
                        .or_else(|| task.get("dependsOn"))
                        .or_else(|| task.get("dependencies")),
                ),
                "status": "backlog",
            }))
        })
        .collect()
}

fn prompt_task_tree_from_draft(
    run: &mut AgenticBoard,
    draft: Value,
    prompt: &str,
) -> Vec<BoardTask> {
    let source_level = normalize_task_level(
        draft
            .get("sourceLevel")
            .or_else(|| draft.get("level"))
            .and_then(Value::as_str),
        TASK_LEVEL_STORY,
    );
    let mut story_draft = draft.clone();
    if let Some(object) = story_draft.as_object_mut() {
        let planning_level = if matches!(source_level, TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC) {
            source_level
        } else {
            TASK_LEVEL_STORY
        };
        let planning_kind = object
            .get("sourceKind")
            .and_then(Value::as_str)
            .unwrap_or(TASK_KIND_DESIGN)
            .to_string();
        object.insert("level".to_string(), json!(planning_level));
        object.insert("kind".to_string(), json!(planning_kind.clone()));
        object.insert("taskType".to_string(), json!(planning_kind));
    }
    let mut story = prompt_task_from_draft(run, story_draft, prompt);
    story.hierarchy.level = if matches!(source_level, TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC) {
        source_level.to_string()
    } else {
        TASK_LEVEL_STORY.to_string()
    };
    story.hierarchy.parent_id = None;
    story.hierarchy.executable = false;
    if matches!(source_level, TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC) {
        return vec![story];
    }
    story.task_type = TASK_KIND_DESIGN.to_string();
    if source_level == TASK_LEVEL_STORY {
        return vec![story];
    }

    let story_id = story.id.clone();
    let mut task_draft = draft.clone();
    if let Some(object) = task_draft.as_object_mut() {
        object.insert("level".to_string(), json!(TASK_LEVEL_TASK));
        object.insert("kind".to_string(), json!(TASK_KIND_DESIGN));
        object.insert("taskType".to_string(), json!(TASK_KIND_DESIGN));
    }
    let mut system_task = task_from_json(run, task_draft, run.tasks.len(), TASK_STATUS_BACKLOG)
        .unwrap_or_else(|| {
            let mut fallback = BoardTask::draft(run, story.title.clone(), story.details.clone());
            fallback.task_type = TASK_KIND_DESIGN.to_string();
            fallback
        });
    system_task.id = allocate_task_id(run);
    system_task.hierarchy.level = TASK_LEVEL_TASK.to_string();
    system_task.hierarchy.parent_id = Some(story_id.clone());
    system_task.hierarchy.executable = false;
    system_task.task_type = TASK_KIND_DESIGN.to_string();
    system_task.task_origin = "user_prompt_generated".to_string();
    system_task.group_id = Some(story_id.clone());
    system_task.status = TASK_STATUS_BACKLOG.to_string();
    system_task.prompt = system_task.description.clone();
    if source_level == TASK_LEVEL_TASK {
        return vec![story, system_task];
    }

    let mut subtask_draft = draft;
    if let Some(object) = subtask_draft.as_object_mut() {
        object.insert("level".to_string(), json!(TASK_LEVEL_SUBTASK));
        object.insert(
            "kind".to_string(),
            json!(
                object
                    .get("sourceKind")
                    .and_then(Value::as_str)
                    .unwrap_or(TASK_KIND_IMPLEMENTATION)
            ),
        );
        object.insert(
            "taskType".to_string(),
            json!(
                object
                    .get("sourceKind")
                    .and_then(Value::as_str)
                    .unwrap_or(TASK_KIND_IMPLEMENTATION)
            ),
        );
    }
    let mut subtask = task_from_json(run, subtask_draft, run.tasks.len() + 1, TASK_STATUS_BACKLOG)
        .unwrap_or_else(|| {
            let mut fallback = BoardTask::draft(run, story.title.clone(), story.details.clone());
            fallback.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
            fallback.hierarchy.executable = true;
            fallback
        });
    subtask.id = allocate_task_id(run);
    subtask.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
    subtask.hierarchy.parent_id = Some(system_task.id.clone());
    subtask.hierarchy.executable = true;
    subtask.task_origin = "user_prompt_generated".to_string();
    subtask.group_id = Some(story_id);
    subtask.status = TASK_STATUS_BACKLOG.to_string();
    subtask.prompt = subtask.description.clone();
    vec![story, system_task, subtask]
}

fn keep_missing_prompt_task_trees(
    run: &AgenticBoard,
    trees: Vec<Vec<BoardTask>>,
) -> (Vec<BoardTask>, usize) {
    let mut existing_root_keys = run
        .tasks
        .iter()
        .filter(|task| {
            !task.backlog_generation_task
                && task.hierarchy.parent_id.is_none()
                && matches!(
                    task_level(task),
                    TASK_LEVEL_INITIATIVE | TASK_LEVEL_EPIC | TASK_LEVEL_STORY
                )
        })
        .map(|task| normalize_suggested_task_key(&task.title))
        .filter(|key| !key.is_empty())
        .collect::<BTreeSet<_>>();
    let mut kept = Vec::new();
    let mut reused = 0usize;
    for tree in trees {
        let Some(root) = tree.first() else {
            continue;
        };
        let key = normalize_suggested_task_key(&root.title);
        if key.is_empty() || !existing_root_keys.insert(key) {
            reused = reused.saturating_add(1);
            continue;
        }
        kept.extend(tree);
    }
    (kept, reused)
}

fn spawn_backlog_prompt_generation(
    state: AppState,
    user_id: String,
    board_id: String,
    operation_id: String,
    prompt: String,
    provider: String,
    model: String,
    board_profile: String,
) {
    tokio::spawn(async move {
        if let Err(error) = complete_backlog_prompt_generation(
            &state,
            &user_id,
            &board_id,
            &operation_id,
            &prompt,
            &provider,
            &model,
            &board_profile,
        )
        .await
        {
            tracing::warn!(
                board_id = %board_id,
                operation_id = %operation_id,
                error = %server_error_message(&error),
                "backlog prompt generation failed"
            );
        }
    });
}

async fn complete_backlog_prompt_generation(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    operation_id: &str,
    prompt: &str,
    provider: &str,
    model: &str,
    board_profile: &str,
) -> Result<()> {
    let snapshot = load_user_board(state, user_id, board_id)?;
    let attempt = generate_prompt_task_drafts(
        state,
        &snapshot.board,
        prompt,
        (!provider.trim().is_empty()).then_some(provider),
        (!model.trim().is_empty() && model.trim() != "provider default").then_some(model),
        (!board_profile.trim().is_empty()).then_some(board_profile),
    )
    .await;
    let _guard = board_mutation_lock();
    let mut stored = load_user_board(state, user_id, board_id)?;
    let current_operation_id = stored
        .board
        .backlog_breakdown
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let current_status = stored
        .board
        .backlog_breakdown
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if current_operation_id != operation_id || current_status != "running" {
        return Ok(());
    }
    record_prompt_task_generation_attempt(
        &mut stored.board,
        "Kanban backlog prompt generation",
        &attempt,
    );
    let generation_error = attempt.result.as_ref().err().map(server_error_message);
    let generation_transcript = prompt_task_generation_transcript(
        &attempt,
        prompt,
        attempt.result.is_ok(),
        generation_error.as_deref(),
    );
    match attempt.result {
        Ok((drafts, warning)) => {
            let generated_trees = drafts
                .into_iter()
                .map(|draft| prompt_task_tree_from_draft(&mut stored.board, draft, prompt))
                .collect::<Vec<_>>();
            let (mut generated, reused_tree_count) =
                keep_missing_prompt_task_trees(&stored.board, generated_trees);
            sanitize_generated_task_dependencies(&stored.board, &mut generated, "");
            if let Some(warning) = warning.as_deref().filter(|value| !value.trim().is_empty()) {
                if let Some(first) = generated.first_mut() {
                    first
                        .references
                        .push(format!("Task generation note: {warning}"));
                }
            }
            let count = generated.len();
            stored.board.tasks.extend(generated);
            normalize_board_hierarchy(&mut stored.board);
            normalize_board_task_groups(&mut stored.board);
            if let Some(issue) = hierarchy_validation_issues(&stored.board)
                .into_iter()
                .next()
            {
                let affected = planning_error_task_ids(&stored.board, &issue);
                stored.board.backlog_breakdown = json!({
                    "status": TASK_STATUS_FAILED,
                    "prompt": prompt,
                    "provider": attempt.effective_provider,
                    "model": attempt.effective_model,
                    "error": issue,
                    "updatedAt": Utc::now(),
                    "transcript": generation_transcript,
                });
                mark_planning_error(&mut stored.board, &affected, "hierarchy", &issue);
                stored.board.touch();
                return save_board(state, &stored.board);
            }
            refresh_hierarchy_rollups(&mut stored.board);
            stored.board.backlog_breakdown = json!({
                "status": "idle",
                "lastOperationId": operation_id,
                "prompt": prompt,
                "provider": attempt.effective_provider,
                "model": attempt.effective_model,
                "boardProfile": board_profile,
                "generatedTaskCount": count,
                "reusedTaskTreeCount": reused_tree_count,
                "warning": warning,
                "completedAt": Utc::now(),
                "updatedAt": Utc::now(),
                "transcript": generation_transcript,
            });
            stored.board.append_log(format!(
                "Backlog prompt generated {count} task(s) from {operation_id}"
            ));
        }
        Err(error) => {
            let message = generation_error.unwrap_or_else(|| server_error_message(&error));
            stored.board.backlog_breakdown = json!({
                "id": operation_id,
                "status": TASK_STATUS_FAILED,
                "prompt": prompt,
                "provider": attempt.effective_provider,
                "model": attempt.effective_model,
                "boardProfile": board_profile,
                "error": message.clone(),
                "failedAt": Utc::now(),
                "updatedAt": Utc::now(),
                "transcript": generation_transcript,
            });
            stored.board.append_log(format!(
                "Backlog prompt generation failed for {operation_id}: {message}"
            ));
        }
    }
    stored.board.touch();
    save_board(state, &stored.board)
}

fn sanitize_generated_task_dependencies(
    _run: &AgenticBoard,
    generated: &mut [BoardTask],
    _placeholder_task_id: &str,
) {
    for task in generated {
        task.depends_on.retain(|dependency| dependency != &task.id);
        task.hierarchy.blocked_by = task.depends_on.clone();
    }
}

fn prompt_task_from_draft(run: &mut AgenticBoard, draft: Value, prompt: &str) -> BoardTask {
    let title = draft
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("New board task")
        .to_string();
    let details = draft
        .get("details")
        .or_else(|| draft.get("description"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&title)
        .to_string();
    let generated_level =
        normalize_task_level(draft.get("level").and_then(Value::as_str), TASK_LEVEL_STORY);
    let level = if matches!(generated_level, TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK) {
        TASK_LEVEL_STORY
    } else {
        generated_level
    };
    let mut task = BoardTask::draft(run, title, details.clone());
    task.prompt = prompt.to_string();
    task.acceptance_criteria = normalize_string_list(draft.get("acceptanceCriteria"));
    if task.acceptance_criteria.is_empty() {
        task.acceptance_criteria = vec!["Complete the task described by this card.".to_string()];
    }
    task.task_type = prompt_task_kind_from_value(
        &draft,
        &task.title,
        &task.details,
        &task.acceptance_criteria,
    )
    .to_string();
    task.references = normalize_string_list(draft.get("references"));
    task.priority = normalize_priority(draft.get("priority").and_then(Value::as_str)).to_string();
    task.depends_on =
        normalize_string_list(draft.get("blockedBy").or_else(|| draft.get("dependsOn")));
    task.status = "backlog".to_string();
    task.manual_task = false;
    task.prompt_task = true;
    task.task_origin = "user_prompt_generated".to_string();
    task.backlog_generation_task = false;
    task.hierarchy = BoardTaskHierarchy {
        level: level.to_string(),
        parent_id: None,
        blocked_by: task.depends_on.clone(),
        executable: false,
        required: draft
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        scope_version: draft
            .get("scopeVersion")
            .and_then(Value::as_u64)
            .unwrap_or(1),
        rank: draft.get("rank").and_then(Value::as_i64).unwrap_or(0),
        attempts: Vec::new(),
        planned_files: normalize_string_list(
            draft.get("plannedFiles").or_else(|| draft.get("files")),
        ),
        side_effects: normalize_string_list(draft.get("sideEffects")),
        side_effects_approved: false,
        side_effect_approval: None,
        side_effect_evidence: Vec::new(),
        manual_test_environment: None,
        research_accepted: false,
        research_acceptance: None,
        discussion: Vec::new(),
    };
    task
}

fn allocate_task_id(run: &mut AgenticBoard) -> String {
    let max_existing_sequence = run
        .tasks
        .iter()
        .filter_map(|task| numeric_task_sequence(&task.id))
        .max()
        .unwrap_or(0);
    run.next_task_sequence = run.next_task_sequence.max(max_existing_sequence);
    loop {
        let Some(next_sequence) = run.next_task_sequence.checked_add(1) else {
            return format!("task-{}", Uuid::new_v4());
        };
        run.next_task_sequence = next_sequence;
        let candidate = format!("task-{next_sequence}");
        if !run.tasks.iter().any(|task| task.id == candidate) {
            return candidate;
        }
    }
}

fn numeric_task_sequence(task_id: &str) -> Option<u64> {
    task_id.strip_prefix("task-")?.parse().ok()
}

fn title_from_prompt(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return None;
    }
    let first = trimmed.lines().find(|line| !line.trim().is_empty())?.trim();
    let mut title = first
        .trim_start_matches(['-', '*', '•'])
        .trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.' || ch == ')')
        .trim()
        .to_string();
    if title.len() > 96 {
        title.truncate(93);
        title.push_str("...");
    }
    Some(title)
}

fn value_to_strings(value: Option<Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| item.as_str().map(str::trim).map(str::to_string))
            .filter(|item| !item.is_empty())
            .collect(),
        Some(Value::String(text)) => text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn task_counts(tasks: &[BoardTask], orchestration_version: u32) -> Value {
    count_statuses(
        tasks
            .iter()
            .filter(|task| !task.backlog_generation_task)
            .filter(|task| orchestration_version < 2 || !task.internal_validation)
            .map(|task| task.status.as_str()),
    )
}

fn task_group_counts(run: &AgenticBoard) -> Value {
    let groups = task_groups_for_counts(&run.tasks, run.orchestration_version);
    count_statuses(
        groups
            .iter()
            .map(|(_, tasks)| task_group_status_for_board(run, tasks)),
    )
}

fn task_groups_for_counts(
    tasks: &[BoardTask],
    orchestration_version: u32,
) -> Vec<(String, Vec<&BoardTask>)> {
    let mut groups: Vec<(String, Vec<&BoardTask>)> = Vec::new();
    for task in tasks
        .iter()
        .filter(|task| task_is_visible_work_item(task, orchestration_version))
    {
        let group_id = task_group_id_or_self(task);
        if let Some((_, group_tasks)) = groups.iter_mut().find(|(id, _)| id == &group_id) {
            group_tasks.push(task);
        } else {
            groups.push((group_id, vec![task]));
        }
    }
    groups
}

fn count_statuses<'a>(statuses: impl Iterator<Item = &'a str>) -> Value {
    let mut counts = serde_json::Map::new();
    let mut total = 0usize;
    for status in statuses {
        total += 1;
        let key = canonical_task_status(status);
        let next = counts.get(key).and_then(Value::as_u64).unwrap_or(0) + 1;
        counts.insert(key.to_string(), json!(next));
    }
    counts.insert("total".to_string(), json!(total));
    Value::Object(counts)
}

fn normalize_provider(provider: Option<&str>) -> Result<String> {
    let normalized = provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_PROVIDER);
    match normalized {
        "claude" | "cursor" | "codex" | "gemini" => Ok(normalized.to_string()),
        _ => Err(bad_request(
            "Provider must be one of: claude, cursor, codex, gemini",
        )),
    }
}

fn normalize_optional_provider(provider: Option<&str>) -> Result<String> {
    match provider.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => normalize_provider(Some(value)),
        None => Ok(String::new()),
    }
}

fn normalize_task_status(status: Option<&str>, default: &str) -> Result<String> {
    let status = status
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default);
    match status {
        TASK_STATUS_BACKLOG => Ok(TASK_STATUS_BACKLOG.to_string()),
        TASK_STATUS_TODO | "pending" | "planned" => Ok(TASK_STATUS_TODO.to_string()),
        TASK_STATUS_IN_PROGRESS | "running" | "pausing" | "cancelling" => {
            Ok(TASK_STATUS_IN_PROGRESS.to_string())
        }
        TASK_STATUS_BLOCKED => Ok(TASK_STATUS_BLOCKED.to_string()),
        TASK_STATUS_FAILED | "cancelled" | "backlog_failed" => Ok(TASK_STATUS_FAILED.to_string()),
        TASK_STATUS_DONE | "completed" => Ok(TASK_STATUS_DONE.to_string()),
        _ => Err(bad_request(
            "Task status must be one of: backlog, todo, in_progress, blocked, failed, done",
        )),
    }
}

fn normalized_task_kind_name(value: &str) -> Option<&'static str> {
    let normalized = value.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match normalized.as_str() {
        "" => None,
        "implementation" | "feature" | "dev" | "development" => Some(TASK_KIND_IMPLEMENTATION),
        "research" | "discovery" => Some(TASK_KIND_RESEARCH),
        "design" | "product_design" | "technical_design" => Some(TASK_KIND_DESIGN),
        "test_implementation" | "test" | "tests" | "test_code" => {
            Some(TASK_KIND_TEST_IMPLEMENTATION)
        }
        "manual_test"
        | "manual_qa"
        | "manual_verification"
        | "functional_verification"
        | "verification"
        | "smoke_test"
        | "smoke"
        | "smoke_pass"
        | "emulator_test" => Some(TASK_KIND_MANUAL_TEST),
        "qa" | "final_qa" | "task_qa" => Some(TASK_KIND_QA),
        "review" | "promotion" | "agents_knowledge" => Some(TASK_KIND_REVIEW),
        "fix" | "qa_fix" => Some(TASK_KIND_FIX),
        "followup" | "follow_up" => Some(TASK_KIND_FOLLOWUP),
        "migration" | "database_migration" => Some(TASK_KIND_MIGRATION),
        "revert" | "rollback" => Some(TASK_KIND_REVERT),
        "cleanup" | "clean_up" => Some(TASK_KIND_CLEANUP),
        "revision" | "revise" => Some(TASK_KIND_REVISION),
        "replacement" | "replace" => Some(TASK_KIND_REPLACEMENT),
        _ => None,
    }
}

fn normalize_task_kind(value: Option<&str>, default: &'static str) -> &'static str {
    value.and_then(normalized_task_kind_name).unwrap_or(default)
}

fn normalize_task_level(value: Option<&str>, default: &'static str) -> &'static str {
    let normalized = value
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace(['-', ' '], "_");
    match normalized.as_str() {
        TASK_LEVEL_INITIATIVE => TASK_LEVEL_INITIATIVE,
        TASK_LEVEL_EPIC => TASK_LEVEL_EPIC,
        TASK_LEVEL_STORY => TASK_LEVEL_STORY,
        TASK_LEVEL_TASK => TASK_LEVEL_TASK,
        TASK_LEVEL_SUBTASK => TASK_LEVEL_SUBTASK,
        _ => default,
    }
}

fn infer_prompt_task_kind(
    title: &str,
    details: &str,
    acceptance_criteria: &[String],
) -> Option<&'static str> {
    let text = std::iter::once(title)
        .chain(std::iter::once(details))
        .chain(acceptance_criteria.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    let manual_verification_signal = (text.contains("manual")
        && (text.contains("test")
            || text.contains("qa")
            || text.contains("verify")
            || text.contains("verification")
            || text.contains("smoke")))
        || text.contains("manual smoke")
        || text.contains("smoke test")
        || text.contains("smoke pass")
        || text.contains("functional verification")
        || text.contains("functional verify")
        || text.contains("run mobile verification")
        || text.contains("run verification")
        || text.contains("test on emulator")
        || text.contains("android emulator")
        || text.contains("ios simulator")
        || text.contains("mobile emulator");
    manual_verification_signal.then_some(TASK_KIND_MANUAL_TEST)
}

fn prompt_task_kind_from_value(
    value: &Value,
    title: &str,
    details: &str,
    acceptance_criteria: &[String],
) -> &'static str {
    let explicit = value
        .get("kind")
        .or_else(|| value.get("taskType"))
        .or_else(|| value.get("task_type"))
        .and_then(Value::as_str);
    explicit
        .and_then(normalized_task_kind_name)
        .or_else(|| infer_prompt_task_kind(title, details, acceptance_criteria))
        .unwrap_or(TASK_KIND_IMPLEMENTATION)
}

fn infer_legacy_user_task_kind(task: &BoardTask) -> Option<&'static str> {
    if canonical_task_kind(task) != TASK_KIND_IMPLEMENTATION || !is_user_authored_task(task) {
        return None;
    }
    let details = [
        task.details.as_str(),
        task.description.as_str(),
        task.prompt.as_str(),
    ]
    .into_iter()
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join("\n");
    infer_prompt_task_kind(&task.title, &details, &task.acceptance_criteria)
}

fn canonical_task_status(status: &str) -> &'static str {
    match status.trim() {
        TASK_STATUS_BACKLOG => TASK_STATUS_BACKLOG,
        TASK_STATUS_TODO | "pending" | "planned" => TASK_STATUS_TODO,
        TASK_STATUS_IN_PROGRESS | "running" | "pausing" | "cancelling" | "backlog_generating" => {
            TASK_STATUS_IN_PROGRESS
        }
        TASK_STATUS_BLOCKED => TASK_STATUS_BLOCKED,
        TASK_STATUS_FAILED | "cancelled" | "backlog_failed" => TASK_STATUS_FAILED,
        TASK_STATUS_DONE | "completed" => TASK_STATUS_DONE,
        "qa" | "review" => TASK_STATUS_TODO,
        _ => TASK_STATUS_BACKLOG,
    }
}

fn task_status_is_todo(status: &str) -> bool {
    canonical_task_status(status) == TASK_STATUS_TODO
}

fn task_status_is_backlog(status: &str) -> bool {
    canonical_task_status(status) == TASK_STATUS_BACKLOG
}

fn task_status_is_active(status: &str) -> bool {
    canonical_task_status(status) == TASK_STATUS_IN_PROGRESS
}

fn task_status_is_done(status: &str) -> bool {
    canonical_task_status(status) == TASK_STATUS_DONE
}

fn clear_backlog_approval(task: &mut BoardTask) {
    task.hierarchy.side_effects_approved = false;
    task.hierarchy.side_effect_approval = None;
    task.hierarchy.research_accepted = false;
    task.hierarchy.research_acceptance = None;
}

fn task_level(task: &BoardTask) -> &'static str {
    match task.hierarchy.level.as_str() {
        TASK_LEVEL_INITIATIVE => TASK_LEVEL_INITIATIVE,
        TASK_LEVEL_EPIC => TASK_LEVEL_EPIC,
        TASK_LEVEL_STORY => TASK_LEVEL_STORY,
        TASK_LEVEL_TASK => TASK_LEVEL_TASK,
        TASK_LEVEL_SUBTASK => TASK_LEVEL_SUBTASK,
        _ if task.hierarchy.parent_id.is_some() => TASK_LEVEL_SUBTASK,
        _ => TASK_LEVEL_STORY,
    }
}

fn task_is_executable(task: &BoardTask) -> bool {
    task.hierarchy.executable && task_level(task) == TASK_LEVEL_SUBTASK
}

fn task_blockers(task: &BoardTask) -> Vec<String> {
    dedupe_strings(
        task.hierarchy
            .blocked_by
            .iter()
            .cloned()
            .chain(task.depends_on.iter().cloned())
            .collect(),
    )
}

fn task_is_runnable(task: &BoardTask) -> bool {
    !task.backlog_generation_task && task_is_executable(task) && task_status_is_todo(&task.status)
}

fn completed_hierarchy_ancestor_id(run: &AgenticBoard, parent_id: Option<&str>) -> Option<String> {
    let mut current_id = parent_id.map(str::to_string);
    let mut visited = BTreeSet::new();
    while let Some(id) = current_id {
        if !visited.insert(id.clone()) {
            return None;
        }
        let parent = run.tasks.iter().find(|task| task.id == id)?;
        if task_status_is_done(&parent.status) {
            return Some(parent.id.clone());
        }
        current_id = parent.hierarchy.parent_id.clone();
    }
    None
}

fn validate_parent_scope_not_completed(run: &AgenticBoard, parent_id: Option<&str>) -> Result<()> {
    if let Some(completed_id) = completed_hierarchy_ancestor_id(run, parent_id) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!(
                "Cannot add, edit, or detach a child beneath completed parent {completed_id}. Create a linked revision, fix, research, or replacement item instead."
            ),
        ));
    }
    Ok(())
}

fn validate_manual_task_status(run: &AgenticBoard, task: &BoardTask) -> Result<()> {
    if !uses_hierarchical_orchestration(run) {
        return Ok(());
    }
    validate_parent_scope_not_completed(run, task.hierarchy.parent_id.as_deref())?;
    if !matches!(
        canonical_task_status(&task.status),
        TASK_STATUS_BACKLOG | TASK_STATUS_TODO
    ) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "New Kanban items may only start in Backlog or Todo.",
        ));
    }
    if task_status_is_todo(&task.status) && !task_ancestors_are_approved(run, task) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "Approve the parent planning item before adding a nested item to Todo.",
        ));
    }
    if task_status_is_todo(&task.status) && !task_side_effects_are_approved(task) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            task_side_effect_block_reason(task),
        ));
    }
    Ok(())
}

fn validate_manual_task_source(run: &AgenticBoard, task: &BoardTask) -> Result<()> {
    let Some(source_id) = task.source_task_id.as_deref() else {
        if task.manual_task && canonical_task_kind(task) == TASK_KIND_FIX {
            return Err(bad_request(
                "A user-created fix must include sourceTaskId for the failed or defective work it addresses.",
            ));
        }
        return Ok(());
    };
    let source = run
        .tasks
        .iter()
        .find(|candidate| candidate.id == source_id)
        .ok_or_else(|| bad_request(format!("Source task not found: {source_id}")))?;
    if source.id == task.id {
        return Err(bad_request("A task cannot link to itself as its source."));
    }
    if canonical_task_kind(task) != TASK_KIND_FIX {
        return Ok(());
    }
    let source_is_defective = matches!(
        canonical_task_status(&source.status),
        TASK_STATUS_FAILED | TASK_STATUS_BLOCKED
    ) || (task_is_done(source)
        && (source.qa_passed == Some(false)
            || source
                .result
                .as_ref()
                .and_then(|result| result.get("status"))
                .and_then(Value::as_str)
                == Some("needs_followup")));
    if !source_is_defective {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "A fix must link to failed, blocked, or explicitly defected QA/manual/review work.",
        ));
    }
    if source
        .hierarchy
        .parent_id
        .as_deref()
        .is_some_and(|parent_id| task.hierarchy.parent_id.as_deref() != Some(parent_id))
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "A fix subtask must be placed under the same parent as the failed subtask.",
        ));
    }
    if matches!(
        canonical_task_status(&source.status),
        TASK_STATUS_FAILED | TASK_STATUS_BLOCKED
    ) && task_blockers(task).iter().any(|id| id == source_id)
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "A fix must link to failed work through sourceTaskId, not depend on that failed work.",
        ));
    }
    Ok(())
}

fn task_ancestors_are_approved(run: &AgenticBoard, task: &BoardTask) -> bool {
    // `sourceTaskId`/`sourceQaTaskId` are links used to group legacy system
    // QA and follow-up work. They are not approval edges. Only an explicit
    // hierarchy parent can hold a child behind an approval boundary. A
    // completed parent is also a boundary for required work, but an optional
    // child is an explicitly approvable nice-to-have and may run without
    // reopening the completed scope.
    let mut current_id = task.hierarchy.parent_id.clone();
    let mut child_required = task.hierarchy.required;
    let mut visited = BTreeSet::new();
    while let Some(parent_id) = current_id {
        if !visited.insert(parent_id.clone()) {
            return false;
        }
        let Some(parent) = run.tasks.iter().find(|candidate| candidate.id == parent_id) else {
            return false;
        };
        if task_rollup_completion_is_satisfied(parent) {
            // A completed required child would reopen or rewrite completed
            // scope. Optional branches are separate, explicitly approved
            // nice-to-have work and remain runnable under that boundary.
            if child_required || parent.superseded_by.is_some() {
                return false;
            }
        } else if !matches!(
            canonical_task_status(&parent.status),
            TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS
        ) {
            return false;
        }
        child_required = parent.hierarchy.required;
        current_id = parent.hierarchy.parent_id.clone();
    }
    true
}

fn task_is_runnable_in_board(run: &AgenticBoard, task: &BoardTask) -> bool {
    task_is_runnable(task)
        && task_ancestors_are_approved(run, task)
        && task_side_effects_are_approved(task)
        && (!has_pending_research_acceptance(run)
            || canonical_task_kind(task) == TASK_KIND_RESEARCH)
}

fn task_is_done(task: &BoardTask) -> bool {
    task_status_is_done(&task.status)
}

fn task_is_visible_work_item(task: &BoardTask, orchestration_version: u32) -> bool {
    !task.backlog_generation_task && (orchestration_version < 2 || !task.internal_validation)
}

fn sanitize_kanban_value(value: &Value) -> Value {
    strip_removed_tracking_fields(&redact_transcript_value(value))
}

fn sanitize_kanban_structure(value: &Value) -> Value {
    strip_removed_tracking_fields(value)
}

fn strip_removed_tracking_fields(value: &Value) -> Value {
    match value {
        Value::String(text) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                let cleaned = strip_removed_tracking_fields(&parsed);
                if cleaned != parsed {
                    return serde_json::to_string(&cleaned)
                        .map(Value::String)
                        .unwrap_or_else(|_| Value::String(text.clone()));
                }
            }
            if is_removed_tracking_text(text) {
                Value::String("[legacy tracking removed]".to_string())
            } else {
                Value::String(text.clone())
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .filter(|item| !item.as_str().is_some_and(is_removed_tracking_text))
                .map(strip_removed_tracking_fields)
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !key.to_ascii_lowercase().contains("requirement"))
                .map(|(key, value)| (key.clone(), strip_removed_tracking_fields(value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn is_removed_tracking_text(text: &str) -> bool {
    let normalized = text.trim().to_ascii_lowercase();
    normalized.contains("requirement") || normalized.contains("req-")
}

fn task_group_id_or_self(task: &BoardTask) -> String {
    trim_string(task.group_id.clone()).unwrap_or_else(|| task.id.clone())
}

fn task_group_id_for_source(task: &BoardTask) -> String {
    task_group_id_or_self(task)
}

fn task_title_for_group(task: &BoardTask) -> String {
    [
        task.title.as_str(),
        task.details.as_str(),
        task.description.as_str(),
        task.prompt.as_str(),
        task.summary.as_str(),
    ]
    .into_iter()
    .map(str::trim)
    .find(|value| !value.is_empty())
    .unwrap_or("Work item")
    .to_string()
}

fn is_kanban_parent_task(task: &BoardTask) -> bool {
    !task.qa_task
        && !task.final_qa_task
        && !task.followup_task
        && !task.qa_fix_task
        && !task.qa_verdict_retry_task
        && !task.task_level_qa
        && !task.agents_knowledge_task
        && task.source_task_id.is_none()
        && task.source_qa_task_id.is_none()
        && matches!(
            canonical_task_kind(task),
            TASK_KIND_IMPLEMENTATION | TASK_KIND_MANUAL_TEST | TASK_KIND_REVIEW
        )
}

fn canonical_task_kind(task: &BoardTask) -> &'static str {
    if task.qa_fix_task || task.source_qa_task_id.is_some() || task.task_type == "qa_fix" {
        TASK_KIND_FIX
    } else if task.followup_task || task.task_type == "followup" {
        TASK_KIND_FOLLOWUP
    } else if task.qa_task
        || task.final_qa_task
        || task.task_level_qa
        || task.id == FINAL_QA_TASK_ID
        || matches!(task.task_type.as_str(), "qa" | "final_qa")
    {
        TASK_KIND_QA
    } else if task.agents_knowledge_task
        || task.id == AGENTS_KNOWLEDGE_TASK_ID
        || task.id == PROMOTION_REVIEW_TASK_ID
        || matches!(
            task.task_type.as_str(),
            "review" | "promotion" | "agents_knowledge"
        )
    {
        TASK_KIND_REVIEW
    } else {
        normalize_task_kind(Some(&task.task_type), TASK_KIND_IMPLEMENTATION)
    }
}

fn canonical_task_origin(origin: &str) -> &str {
    match origin.trim() {
        "manual" => "user_manual",
        "prompt_breakdown" => "user_prompt_generated",
        "planner" => "planned",
        value => value,
    }
}

fn normalize_board_provenance(run: &mut AgenticBoard) {
    for task in &mut run.tasks {
        let canonical = canonical_task_origin(&task.task_origin);
        task.task_origin = if canonical.is_empty() {
            infer_legacy_task_origin(task)
                .unwrap_or_default()
                .to_string()
        } else {
            canonical.to_string()
        };
        normalize_done_followup_task(task);
    }
}

fn normalize_done_followup_task(task: &mut BoardTask) {
    if canonical_task_status(&task.status) != TASK_STATUS_DONE {
        return;
    }
    if task
        .result
        .as_ref()
        .is_some_and(completion_evidence_gate_failed)
    {
        task.status = TASK_STATUS_BLOCKED.to_string();
        task.qa_passed = Some(false);
        task.error = Some(
            "Completion evidence gate failed; provide valid evidence before this subtask can be done."
                .to_string(),
        );
        task.completed_at = None;
        return;
    }
    let needs_followup = task
        .result
        .as_ref()
        .and_then(|result| result.get("status"))
        .and_then(Value::as_str)
        == Some("needs_followup");
    if !needs_followup {
        return;
    }
    task.error = None;
    if task.tdd_phase == "fix_pending" {
        task.tdd_phase = "followup_pending".to_string();
    }
}

fn normalize_board_model(run: &mut AgenticBoard) {
    if run.orchestration_version < 3 {
        run.orchestration_version = 3;
    }
    if run.model.trim().is_empty() {
        run.model = default_model_for_provider(&run.provider);
    }
    if run.primary_model.trim().is_empty() {
        run.primary_model = run.model.clone();
    }
    normalize_board_provenance(run);
    run.backlog_breakdown = sanitize_kanban_value(&run.backlog_breakdown);
    run.discussion_proposals = run
        .discussion_proposals
        .iter()
        .map(sanitize_kanban_value)
        .collect();
    let mut legacy_breakdown: Option<Value> = None;
    run.tasks.retain_mut(|task| {
        if task.backlog_generation_task {
            let status = canonical_task_status(&task.status);
            legacy_breakdown = Some(json!({
                "status": if status == TASK_STATUS_FAILED { TASK_STATUS_FAILED } else { "idle" },
                "legacyTaskId": task.id.clone(),
                "prompt": task.prompt.clone(),
                "error": task.error.clone(),
                "updatedAt": Utc::now(),
            }));
            return false;
        }
        let previous_status = task.status.clone();
        if previous_status == "qa" && !task.qa_task {
            task.qa_task = true;
        }
        task.status = canonical_task_status(&previous_status).to_string();
        task.task_type = infer_legacy_user_task_kind(task)
            .unwrap_or_else(|| canonical_task_kind(task))
            .to_string();
        task.transcript = task.transcript.iter().map(sanitize_kanban_value).collect();
        task.hierarchy.discussion = task
            .hierarchy
            .discussion
            .iter()
            .map(sanitize_kanban_value)
            .collect();
        task.hierarchy.attempts = task
            .hierarchy
            .attempts
            .iter()
            .map(sanitize_kanban_value)
            .collect();
        task.hierarchy.side_effect_approval = task
            .hierarchy
            .side_effect_approval
            .as_ref()
            .map(sanitize_kanban_value);
        task.hierarchy.research_acceptance = task
            .hierarchy
            .research_acceptance
            .as_ref()
            .map(sanitize_kanban_value);
        task.changed_file_summary = task
            .changed_file_summary
            .as_ref()
            .map(sanitize_kanban_value)
            .map(|summary| normalize_changed_file_summary(&summary));
        task.result = task.result.as_ref().map(sanitize_kanban_value);
        let ownership_is_known = task
            .changed_file_summary
            .as_ref()
            .and_then(|summary| summary.get("ownershipPolicy"))
            .and_then(Value::as_str)
            == Some(WORKSPACE_OWNERSHIP_POLICY);
        if !ownership_is_known {
            task.changed_files.clear();
            if let Some(result) = task.result.as_mut().and_then(Value::as_object_mut) {
                result.insert("changedFiles".to_string(), json!([]));
            }
        }
        task.result_validation = task.result_validation.as_ref().map(sanitize_kanban_value);
        task.deterministic_validation = task
            .deterministic_validation
            .as_ref()
            .map(sanitize_kanban_value);
        task.rag_context_refs = task
            .rag_context_refs
            .iter()
            .map(sanitize_kanban_value)
            .collect();
        task.qa_baseline_validation = task
            .qa_baseline_validation
            .as_ref()
            .map(sanitize_kanban_value);
        task.coverage_evidence = task
            .coverage_evidence
            .iter()
            .map(sanitize_kanban_value)
            .collect();
        true
    });
    normalize_board_hierarchy(run);
    normalize_board_task_groups(run);
    if run.backlog_breakdown.is_null()
        || !run.backlog_breakdown.is_object()
        || run
            .backlog_breakdown
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .is_empty()
    {
        run.backlog_breakdown = legacy_breakdown.unwrap_or_else(default_backlog_breakdown);
    }
}

fn normalize_board_hierarchy(run: &mut AgenticBoard) {
    // Parent levels can themselves be repaired from their parent. Several
    // passes make the operation converge for migrated trees and for a user
    // child detached from the middle of a hierarchy. Five levels are the
    // maximum supported depth, so this is bounded and remains safe for
    // malformed cyclic data.
    for _ in 0..5 {
        let task_snapshot = run.tasks.clone();
        for task in &mut run.tasks {
            // Legacy system cards may carry a source link for grouping, but
            // that link must not silently become a hierarchy/approval parent.
            // New hierarchy edges are explicit in `parent_id`.
            //
            // Older snapshots were normalized by copying the source id into
            // `parent_id`. Repair that one-way migration while retaining the
            // source link for group display. A legacy system card sourced from
            // a subtask becomes a sibling under the source's explicit parent;
            // cards sourced from a planning item remain root-level system work.
            if source_link_is_structural(task)
                && task
                    .hierarchy
                    .parent_id
                    .as_deref()
                    .is_some_and(|parent_id| {
                        task.source_task_id.as_deref() == Some(parent_id)
                            || task.source_qa_task_id.as_deref() == Some(parent_id)
                    })
            {
                let source_id = task
                    .source_task_id
                    .as_deref()
                    .filter(|source_id| task.hierarchy.parent_id.as_deref() == Some(*source_id))
                    .or_else(|| {
                        task.source_qa_task_id.as_deref().filter(|source_id| {
                            task.hierarchy.parent_id.as_deref() == Some(*source_id)
                        })
                    });
                task.hierarchy.parent_id = source_id
                    .and_then(|source_id| {
                        task_snapshot
                            .iter()
                            .find(|candidate| candidate.id == source_id)
                    })
                    .filter(|source| task_level(source) == TASK_LEVEL_SUBTASK)
                    .and_then(|source| source.hierarchy.parent_id.clone());
            }
            let inferred_level = if task.hierarchy.parent_id.is_some()
                || task.internal_validation
                || task.qa_task
                || task.final_qa_task
                || task.followup_task
                || task.qa_fix_task
                || task.qa_verdict_retry_task
                || task.task_level_qa
                || task.agents_knowledge_task
            {
                TASK_LEVEL_SUBTASK
            } else {
                TASK_LEVEL_STORY
            };
            let requested_level = normalize_task_level(
                (!task.hierarchy.level.trim().is_empty()).then_some(task.hierarchy.level.as_str()),
                inferred_level,
            );
            let level = task
                .hierarchy
                .parent_id
                .as_deref()
                .and_then(|parent_id| {
                    task_snapshot
                        .iter()
                        .find(|candidate| candidate.id == parent_id)
                        .and_then(|parent| next_hierarchy_level(task_level(parent)))
                })
                .unwrap_or_else(|| {
                    if task.hierarchy.parent_id.is_none()
                        && matches!(requested_level, TASK_LEVEL_TASK | TASK_LEVEL_SUBTASK)
                        && !source_link_is_structural(task)
                    {
                        TASK_LEVEL_STORY
                    } else {
                        requested_level
                    }
                });
            task.hierarchy.level = level.to_string();
            task.hierarchy.executable = level == TASK_LEVEL_SUBTASK;
            if task.hierarchy.scope_version == 0 {
                task.hierarchy.scope_version = 1;
                task.hierarchy.required = true;
            }
            task.hierarchy.blocked_by = dedupe_strings(
                task.hierarchy
                    .blocked_by
                    .iter()
                    .cloned()
                    .chain(task.depends_on.iter().cloned())
                    .collect(),
            );
            task.depends_on = task.hierarchy.blocked_by.clone();
        }
    }
    reconcile_hierarchy_approval_states(run);
}

fn reconcile_hierarchy_approval_states(run: &mut AgenticBoard) {
    let snapshot = run.tasks.clone();
    let mut invalid = Vec::new();
    for task in &snapshot {
        if task_is_done(task)
            || !matches!(
                canonical_task_status(&task.status),
                TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS
            )
        {
            continue;
        }
        let mut current_id = task.hierarchy.parent_id.clone();
        let mut child_required = task.hierarchy.required;
        let mut visited = BTreeSet::new();
        while let Some(parent_id) = current_id {
            if !visited.insert(parent_id.clone()) {
                invalid.push((task.id.clone(), parent_id, "hierarchy cycle".to_string()));
                break;
            }
            let Some(parent) = snapshot.iter().find(|candidate| candidate.id == parent_id) else {
                invalid.push((task.id.clone(), parent_id, "missing parent".to_string()));
                break;
            };
            let parent_status = canonical_task_status(&parent.status);
            let parent_is_completed = task_rollup_completion_is_satisfied(parent);
            if parent_is_completed {
                // Optional work is an explicit, independently approvable
                // branch. It may remain runnable after the required parent
                // scope is done; required work must not silently reopen it.
                if child_required || parent.superseded_by.is_some() {
                    invalid.push((
                        task.id.clone(),
                        parent.id.clone(),
                        parent_status.to_string(),
                    ));
                    break;
                }
            } else if !matches!(parent_status, TASK_STATUS_TODO | TASK_STATUS_IN_PROGRESS) {
                invalid.push((
                    task.id.clone(),
                    parent.id.clone(),
                    parent_status.to_string(),
                ));
                break;
            }
            child_required = parent.hierarchy.required;
            current_id = parent.hierarchy.parent_id.clone();
        }
    }
    for (task_id, parent_id, reason) in invalid {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
            task.status = TASK_STATUS_BLOCKED.to_string();
            task.started_at = None;
            task.completed_at = None;
            task.provider_session_id = None;
            task.error = Some(format!(
                "Parent {parent_id} is not approved ({reason}); approve the parent before running this item."
            ));
        }
    }
}

fn normalize_board_task_groups(run: &mut AgenticBoard) {
    let snapshot = run.tasks.clone();
    let group_ids = snapshot
        .iter()
        .enumerate()
        .map(|(index, _)| infer_task_group_id(&snapshot, index))
        .collect::<Vec<_>>();
    for (task, group_id) in run.tasks.iter_mut().zip(group_ids) {
        if task.backlog_generation_task {
            continue;
        }
        task.group_id = Some(group_id);
    }
}

fn infer_task_group_id(tasks: &[BoardTask], index: usize) -> String {
    let task = &tasks[index];
    if task.final_qa_task || task.id == FINAL_QA_TASK_ID {
        return FINAL_QA_TASK_ID.to_string();
    }
    if task.id == PROMOTION_REVIEW_TASK_ID || matches!(task.task_type.as_str(), "promotion") {
        return PROMOTION_REVIEW_TASK_ID.to_string();
    }
    let mut current_index = index;
    let mut visited = BTreeSet::new();
    loop {
        let current = &tasks[current_index];
        let Some(parent_id) = task_parent_id(current) else {
            break;
        };
        if !visited.insert(parent_id.to_string()) {
            break;
        }
        let Some(parent_index) = tasks.iter().position(|candidate| candidate.id == parent_id)
        else {
            return parent_id.to_string();
        };
        if let Some(group_id) = trim_string(tasks[parent_index].group_id.clone()) {
            return group_id;
        }
        current_index = parent_index;
    }

    trim_string(task.group_id.clone()).unwrap_or_else(|| tasks[current_index].id.clone())
}

fn infer_legacy_task_origin(task: &BoardTask) -> Option<&'static str> {
    if task.final_qa_task || task.id == FINAL_QA_TASK_ID {
        Some("system_final_qa")
    } else if task.qa_verdict_retry_task {
        Some("system_qa_verdict_retry")
    } else if task.qa_fix_task {
        Some("system_qa_fix")
    } else if task.task_level_qa || task.qa_task {
        Some("system_qa")
    } else if task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID {
        Some("system_agents")
    } else if task.followup_task {
        Some("system_followup")
    } else if task.references.iter().any(|reference| {
        reference
            .to_ascii_lowercase()
            .contains("suggested backlog task from")
    }) {
        Some("ai_suggested_backlog")
    } else if task.backlog_generation_task || (task.prompt_task && !task.manual_task) {
        Some("user_prompt_generated")
    } else if task.manual_task || task.prompt_task {
        Some("user_manual")
    } else {
        None
    }
}

fn normalize_git_policy(policy: Option<&str>) -> String {
    match policy.map(str::trim).filter(|value| !value.is_empty()) {
        Some("managed") | Some("managed_git") | Some("managed-workflow") => "managed".to_string(),
        _ => "read_only".to_string(),
    }
}

fn normalize_board_profile(profile: Option<&str>) -> String {
    match profile
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('-', "_")
        .as_str()
    {
        "minimal" | "strict" | "cheap" => "minimal".to_string(),
        "product_ready" | "productready" | "product" | "polished" | "expensive" | "quality" => {
            "product_ready".to_string()
        }
        _ => "complete_app".to_string(),
    }
}

fn normalize_board_profile_for_strategy(profile: Option<&str>, strategy: Option<&Value>) -> String {
    if profile
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return normalize_board_profile(profile);
    }
    match model_strategy_mode(strategy) {
        "cheap" => "minimal".to_string(),
        "expensive" => "product_ready".to_string(),
        _ => normalize_board_profile(None),
    }
}

fn project_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("project")
        .to_string()
}

fn trim_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn redact_transcript_value(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact_transcript_text(text)),
        Value::Array(items) => Value::Array(items.iter().map(redact_transcript_value).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let key_lower = key.to_ascii_lowercase();
                    let redacted = if [
                        "api_key",
                        "apikey",
                        "password",
                        "passwd",
                        "secret",
                        "access_token",
                        "refresh_token",
                    ]
                    .iter()
                    .any(|marker| key_lower.contains(marker))
                    {
                        Value::String("[REDACTED]".to_string())
                    } else {
                        redact_transcript_value(value)
                    };
                    (key.clone(), redacted)
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn redact_transcript_text(text: &str) -> String {
    let mut redacted = text.to_string();
    redacted = redact_secret_after_marker(&redacted, "bearer");
    for marker in [
        "api_key",
        "apikey",
        "password",
        "passwd",
        "secret",
        "access_token",
        "refresh_token",
        "minimax_api_key",
        "authorization",
    ] {
        redacted = redact_secret_after_marker(&redacted, marker);
    }
    redacted
}

fn redact_secret_after_marker(text: &str, marker: &str) -> String {
    let mut result = text.to_string();
    let marker = marker.to_ascii_lowercase();
    loop {
        let lower = result.to_ascii_lowercase();
        let Some(marker_start) = lower.find(&marker) else {
            break;
        };
        let mut value_start = marker_start + marker.len();
        while value_start < result.len()
            && (result.as_bytes()[value_start].is_ascii_whitespace()
                || matches!(result.as_bytes()[value_start], b'=' | b':' | b'"' | b'\''))
        {
            value_start += 1;
        }
        if value_start >= result.len() {
            break;
        }
        let value_end = result[value_start..]
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | '}' | ']'))
            .map(|(index, _)| value_start + index)
            .unwrap_or(result.len());
        if value_end <= value_start {
            break;
        }
        result.replace_range(value_start..value_end, "[REDACTED]");
    }
    result
}

fn default_orchestration_version() -> u32 {
    2
}

fn default_provider_string() -> String {
    DEFAULT_PROVIDER.to_string()
}

fn default_board_profile() -> String {
    normalize_board_profile(None)
}

fn default_git_policy() -> String {
    "read_only".to_string()
}

fn default_paused_status() -> String {
    "paused".to_string()
}

fn default_priority() -> String {
    TASK_PRIORITY_P2.to_string()
}

fn default_task_type() -> String {
    "implementation".to_string()
}

fn default_task_level() -> String {
    TASK_LEVEL_STORY.to_string()
}

fn default_required_task() -> bool {
    true
}

fn default_backlog_breakdown() -> Value {
    json!({ "status": "idle" })
}

fn default_tdd_enabled() -> bool {
    !matches!(
        env::var("IO_WORKBENCH_TDD_ENABLED")
            .unwrap_or_else(|_| "true".to_string())
            .trim()
            .to_lowercase()
            .as_str(),
        "false" | "0" | "no"
    )
}

fn default_tdd_phase() -> String {
    "qa_pending".to_string()
}

fn default_tdd_policy() -> Value {
    json!({
        "requireFailingTestBeforeDev": true,
        "maxFixAttempts": 3,
        "allowImplementationWithoutTests": false,
        "qaCommandStage": "qa",
        "featureCommandStage": "feature",
        "finalCommandStage": "final",
    })
}

fn default_provider_usage() -> Value {
    json!({
        "inputTokens": 0,
        "cachedInputTokens": 0,
        "outputTokens": 0,
        "totalTokens": 0,
        "invocationsWithUsage": 0,
    })
}

fn bad_request(message: impl Into<String>) -> ServerError {
    ServerError::new(StatusCode::BAD_REQUEST, message)
}

fn not_found(message: impl Into<String>) -> ServerError {
    ServerError::new(StatusCode::NOT_FOUND, message)
}

fn task_generation_error(details: impl Into<String>) -> ServerError {
    ServerError::with_details(
        StatusCode::BAD_GATEWAY,
        "Failed to generate task drafts",
        details,
    )
}

fn io_error(error: std::io::Error) -> ServerError {
    ServerError::with_details(
        StatusCode::INTERNAL_SERVER_ERROR,
        "failed to access agentic board storage",
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use iowb_core::AppConfig;
    use iowb_protocol::ChatMessage;

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());
    static RAG_TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct TestEnvGuard {
        previous: Vec<(&'static str, Option<String>)>,
    }

    impl TestEnvGuard {
        fn set(changes: Vec<(&'static str, Option<String>)>) -> Self {
            let previous = changes
                .iter()
                .map(|(key, _)| (*key, std::env::var(key).ok()))
                .collect::<Vec<_>>();
            unsafe {
                for (key, value) in changes {
                    if let Some(value) = value {
                        std::env::set_var(key, value);
                    } else {
                        std::env::remove_var(key);
                    }
                }
            }
            Self { previous }
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            unsafe {
                for (key, value) in &self.previous {
                    if let Some(value) = value {
                        std::env::set_var(key, value);
                    } else {
                        std::env::remove_var(key);
                    }
                }
            }
        }
    }

    fn board_fixture(value: Value) -> AgenticBoard {
        let request = serde_json::from_value::<CreateBoardRequest>(value).unwrap();
        AgenticBoard::new(None, request).unwrap()
    }

    fn native_rag_plugin_path_for_test() -> Option<PathBuf> {
        if let Ok(path) = std::env::var("IO_WORKBENCH_RAG_PLUGIN")
            && !path.trim().is_empty()
        {
            return Some(PathBuf::from(path));
        }
        if let Ok(path) = std::env::var("IO_WORKBENCH_RAG_PLUGIN_PATH")
            && !path.trim().is_empty()
        {
            return Some(PathBuf::from(path));
        }

        #[cfg(target_os = "windows")]
        const LIB_NAME: &str = "iowb_rag_native.dll";
        #[cfg(target_os = "macos")]
        const LIB_NAME: &str = "libiowb_rag_native.dylib";
        #[cfg(all(unix, not(target_os = "macos")))]
        const LIB_NAME: &str = "libiowb_rag_native.so";

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or(manifest_dir);
        let target_dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root.join("target"));
        let path = target_dir.join("debug").join(LIB_NAME);
        if path.exists() {
            Some(path)
        } else {
            eprintln!(
                "skipping native RAG Kanban test; build the plugin first with `cargo build -p iowb-rag-native` or set IO_WORKBENCH_RAG_PLUGIN"
            );
            None
        }
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires built native RAG plugin and FastEmbed model availability"]
    async fn kanban_board_attaches_context_from_native_rag_plugin() {
        let _env_lock = RAG_TEST_ENV_LOCK.lock().expect("RAG test env lock");
        let Some(plugin_path) = native_rag_plugin_path_for_test() else {
            return;
        };
        let root = std::env::temp_dir().join(format!("iowb-kanban-rag-{}", Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(project.join("src")).expect("project source directory");
        fs::write(
            project.join("src/auth.rs"),
            r#"
pub struct SessionTokenStore;

impl SessionTokenStore {
    pub fn rotate_refresh_token(&self, csrf_nonce: &str) -> bool {
        csrf_nonce == "kanban-rag-csrf-nonce"
    }
}

pub const KANBAN_RAG_SENTINEL: &str = "SessionTokenStore validates refresh token rotation with csrf nonce";
"#,
        )
        .expect("write source fixture");

        let _env = TestEnvGuard::set(vec![
            ("IO_WORKBENCH_RAG_MODE", Some("native-plugin".to_string())),
            (
                "IO_WORKBENCH_RAG_PLUGIN",
                Some(plugin_path.display().to_string()),
            ),
            (
                "IOWB_RAG_STORAGE_DIR",
                Some(root.join("rag-store").display().to_string()),
            ),
            (
                "IOWB_RAG_FASTEMBED_CACHE_DIR",
                Some(root.join("fastembed-cache").display().to_string()),
            ),
            ("IOWB_RAG_EMBEDDING_MODEL", Some("bge-small".to_string())),
            ("IOWB_RAG_DENSE_WEIGHT", Some("0.60".to_string())),
            ("IOWB_RAG_BM25_WEIGHT", Some("0.40".to_string())),
        ]);

        let mut run = board_fixture(json!({
            "command": "Implement SessionTokenStore refresh token rotation",
            "projectPath": project.display().to_string(),
            "projectName": format!("kanban-rag-{}", Uuid::new_v4()),
            "provider": "codex",
            "title": "Wire SessionTokenStore token rotation",
            "details": "Use the existing csrf nonce refresh token rotation code."
        }));
        if let Some(task) = run.tasks.get_mut(0) {
            task.acceptance_criteria = vec![
                "Reuse SessionTokenStore for refresh token rotation.".to_string(),
                "Preserve csrf nonce validation behavior.".to_string(),
            ];
        }

        index_project_for_rag(&mut run).await;

        assert!(run.rag_enabled);
        let ingestion = run
            .rag_ingestions
            .iter()
            .find(|value| value.get("kind").and_then(Value::as_str) == Some("project_index"))
            .expect("project index ingestion recorded");
        assert_eq!(
            ingestion.get("ok").and_then(Value::as_bool),
            Some(true),
            "project index failed: {ingestion}"
        );
        assert!(
            ingestion
                .pointer("/response/chunksIndexed")
                .and_then(Value::as_u64)
                .unwrap_or_default()
                > 0,
            "project index did not ingest chunks: {ingestion}"
        );

        attach_rag_context_for_task(&mut run, 0).await;

        let query = run.rag_queries.last().expect("RAG query recorded");
        assert_eq!(
            query.get("ok").and_then(Value::as_bool),
            Some(true),
            "RAG query failed: {query}"
        );
        let task = &run.tasks[0];
        assert!(
            !task.rag_context_refs.is_empty(),
            "RAG query returned no context refs: {query}"
        );
        assert!(
            task.rag_prompt_context.contains("SessionTokenStore")
                || task.rag_prompt_context.contains("KANBAN_RAG_SENTINEL"),
            "RAG prompt context did not include the indexed auth source: {}",
            task.rag_prompt_context
        );
        assert!(
            task.rag_prompt_context.contains("src/auth.rs"),
            "RAG prompt context did not include the source path: {}",
            task.rag_prompt_context
        );

        drop(_env);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn startup_backfill_hides_legacy_board_session_before_discovery() {
        let root =
            std::env::temp_dir().join(format!("iowb-server-board-backfill-{}", Uuid::new_v4()));
        let project = root.join("project");
        let config_dir = root.join("config");
        fs::create_dir_all(&project).expect("project directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");

        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": project.display().to_string(),
            "provider": "codex"
        }));
        run.id = "legacy-board".to_string();
        run.user_id = Some("user-1".to_string());
        run.tasks[0].provider_session_id = Some("legacy-board-chat".to_string());
        save_board(&state, &run).expect("persist legacy board");

        let session = SessionSummary {
            id: "legacy-board-chat".to_string(),
            provider: Provider::Codex,
            project_path: project.display().to_string(),
            title: "Legacy board chat".to_string(),
            last_activity: Utc::now(),
            ..Default::default()
        };
        state
            .storage
            .upsert_session(&session)
            .expect("persist session");
        state
            .storage
            .append_message(
                &session.id,
                &ChatMessage {
                    id: "message-1".to_string(),
                    role: MessageRole::User,
                    content: format!("Board id: {}\nTask 1: {}", run.id, run.tasks[0].id),
                    timestamp: Utc::now(),
                    metadata: Value::Null,
                },
            )
            .expect("persist board prompt");

        assert_eq!(
            state
                .storage
                .list_sessions()
                .expect("pre-backfill list")
                .len(),
            1
        );
        backfill_legacy_board_sessions(&state)
            .await
            .expect("backfill succeeds");

        let classified = state
            .storage
            .get_session(&session.id)
            .expect("read session")
            .expect("session exists");
        assert!(classified.board_session);
        assert_eq!(classified.board_id.as_deref(), Some("legacy-board"));
        assert_eq!(
            classified.board_task_id.as_deref(),
            Some(run.tasks[0].id.as_str())
        );
        assert!(
            state
                .storage
                .list_sessions()
                .expect("post-backfill list")
                .is_empty()
        );
        assert!(
            state
                .sessions
                .list_for_project(&session.project_path)
                .await
                .unwrap()
                .is_empty()
        );

        drop(state);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn structured_execution_summary_overrides_transport_json_tail() {
        let parsed = parse_execution_result(
            r#"{"status":"done","summary":"Created STATUS.md from retrieved context."}"#,
        )
        .unwrap();

        assert_eq!(
            resolved_execution_summary(&parsed, "}"),
            "Created STATUS.md from retrieved context."
        );
    }

    #[test]
    fn execution_summary_falls_back_to_transport_summary() {
        let parsed = json!({"status": "done", "summary": "  "});

        assert_eq!(
            resolved_execution_summary(&parsed, "Provider completed successfully"),
            "Provider completed successfully"
        );
    }

    #[test]
    fn agentic_gateway_config_does_not_duplicate_endpoint_inclusive_urls() {
        let mut codex = json!({
            "gatewayUrl": "https://ai.qif.us/codex/",
        });
        agentic_apply_io_gateway_config(&mut codex, Provider::Codex);
        assert_eq!(
            codex.get("baseUrl").and_then(Value::as_str),
            Some("https://ai.qif.us/codex")
        );

        let mut claude = json!({
            "gatewayUrl": "https://ai.qif.us/claude",
        });
        agentic_apply_io_gateway_config(&mut claude, Provider::Claude);
        assert_eq!(
            claude.get("baseUrl").and_then(Value::as_str),
            Some("https://ai.qif.us/claude")
        );
    }

    #[test]
    fn new_board_preserves_every_mobile_configuration_field() {
        let run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "projectName": "project",
            "provider": "gemini",
            "model": "gemini-2.5-pro",
            "nextProvider": "codex",
            "nextModel": "gpt-5",
            "modelStrategy": {"mode": "fallback"},
            "boardProfile": "complete_app",
            "taskModelOverrides": {"qa": "sonnet"},
            "sessionPolicy": "task-model",
            "gitPolicy": "managed",
            "toolsSettings": {"shell": true},
            "qaPolicy": {
                "taskQaMode": "all",
                "maxFollowupsPerGroup": 5,
                "maxTaskAttempts": 4,
                "repairMalformedToolCalls": false,
                "malformedToolCallRepairRetries": 2
            }
        }));

        assert_eq!(run.provider, "gemini");
        assert_eq!(run.model, "gemini-2.5-pro");
        assert_eq!(run.next_provider, "codex");
        assert_eq!(run.next_model, "gpt-5");
        assert_eq!(run.model_strategy, Some(json!({"mode": "fallback"})));
        assert_eq!(run.board_profile, "complete_app");
        assert_eq!(run.task_model_overrides, json!({"qa": "sonnet"}));
        assert_eq!(run.session_policy, "task-model");
        assert_eq!(run.git_policy, "managed");
        assert_eq!(run.tools_settings, Some(json!({"shell": true})));
        assert_eq!(
            run.qa_policy,
            json!({
                "taskQaMode": "all",
                "maxFollowupsPerGroup": 5,
                "maxTaskAttempts": 4,
                "repairMalformedToolCalls": false,
                "malformedToolCallRepairRetries": 2
            })
        );
    }

    #[test]
    fn orchestration_v2_summary_hides_internal_validation_tasks() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let mut validation = run.tasks[0].clone();
        validation.id = "task-final-qa".to_string();
        validation.internal_validation = true;
        validation.status = TASK_STATUS_DONE.to_string();
        run.tasks.push(validation);

        let summary = run.summary_json(None);

        assert_eq!(summary["taskCounts"]["total"], 1);
        assert_eq!(summary["taskCounts"]["done"], 1);
    }

    #[test]
    fn summary_reports_actual_telemetry_validation_and_resumability() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.status = "running".to_string();
        run.auto_run_enabled = true;
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        run.prompt_telemetry = vec![
            json!({
                "phase": "implementation",
                "label": "First",
                "chars": 400,
                "estimatedTokens": 100,
                "actualInputTokens": 60,
                "actualCachedInputTokens": 10,
                "actualOutputTokens": 30,
                "actualTotalTokens": 90,
                "effectiveModel": "gpt-5.6-sol"
            }),
            json!({
                "phase": "qa",
                "label": "Second",
                "chars": 800,
                "estimatedTokens": 200,
                "actualInputTokens": 100,
                "actualOutputTokens": 50,
                "actualTotalTokens": 150
            }),
        ];
        run.validation_runs = vec![
            json!({"stage": "feature", "passed": true, "commands": ["cargo test", "cargo fmt"]}),
            json!({"stage": "final", "passed": true, "command": "cargo check"}),
        ];

        let summary = run.summary_json(None);

        assert_eq!(summary["promptTelemetrySummary"]["actualInputTokens"], 160);
        assert_eq!(
            summary["promptTelemetrySummary"]["actualCachedInputTokens"],
            10
        );
        assert_eq!(summary["promptTelemetrySummary"]["actualOutputTokens"], 80);
        assert_eq!(summary["promptTelemetrySummary"]["actualTokens"], 240);
        assert_eq!(
            summary["promptTelemetrySummary"]["phases"][0]["phase"],
            "qa"
        );
        assert_eq!(
            summary["promptTelemetrySummary"]["largestCall"]["label"],
            "Second"
        );
        assert_eq!(summary["validationSummary"]["latestStage"], "final");
        assert_eq!(summary["validationSummary"]["latestPassed"], true);
        assert_eq!(summary["validationSummary"]["commands"], 3);
        assert_eq!(summary["resumable"], false);

        run.status = "paused".to_string();
        assert_eq!(run.summary_json(None)["resumable"], true);
    }

    #[test]
    fn manual_task_preserves_all_rich_authoring_fields() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let request = serde_json::from_value::<TaskRequest>(json!({
            "title": "Rich task",
            "details": "Implement the feature",
            "priority": TASK_PRIORITY_P1,
            "status": "backlog",
            "acceptanceCriteria": ["Tests pass", "UI renders"],
            "references": ["README.md"],
            "files": ["src/main.rs"],
            "paths": ["tests/main.rs"],
            "dependsOn": ["task-0"]
        }))
        .unwrap();

        let task = BoardTask::manual(&mut run, request).unwrap();

        assert_eq!(task.title, "Rich task");
        assert_eq!(task.details, "Implement the feature");
        assert_eq!(task.priority, TASK_PRIORITY_P1);
        assert_eq!(task.status, "backlog");
        assert_eq!(task.acceptance_criteria, vec!["Tests pass", "UI renders"]);
        assert_eq!(
            task.references,
            vec!["README.md", "src/main.rs", "tests/main.rs"]
        );
        assert_eq!(task.depends_on, vec!["task-0"]);
        assert_eq!(task.task_origin, "user_manual");
    }

    #[test]
    fn legacy_task_origins_normalize_without_changing_system_origins() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let mut prompt_task = BoardTask::draft(
            &mut run,
            "Prompt task".to_string(),
            "Prompt details".to_string(),
        );
        prompt_task.task_origin = "prompt_breakdown".to_string();
        let mut planned_task = prompt_task.clone();
        planned_task.id = "task-3".to_string();
        planned_task.task_origin = "planner".to_string();
        let mut system_task = prompt_task.clone();
        system_task.id = "task-4".to_string();
        system_task.task_origin = "system_followup".to_string();
        run.tasks[0].task_origin = "manual".to_string();
        run.tasks.extend([prompt_task, planned_task, system_task]);

        normalize_board_provenance(&mut run);

        assert_eq!(
            run.tasks
                .iter()
                .map(|task| task.task_origin.as_str())
                .collect::<Vec<_>>(),
            vec![
                "user_manual",
                "user_prompt_generated",
                "planned",
                "system_followup",
            ]
        );
    }

    #[test]
    fn prompt_task_draft_infers_manual_test_kind_for_functional_verification() {
        let mut run = board_fixture(json!({
            "command": "Add budget alerts",
            "projectPath": "/tmp/project"
        }));
        let drafts = sanitize_prompt_task_drafts(
            &json!({
                "tasks": [
                    {
                        "title": "Run mobile functional verification for budget alerts",
                        "details": "Verify the budget alerts workflow locally using the existing Android project commands and a focused manual smoke pass.",
                        "acceptanceCriteria": ["Budget alert workflow is verified on the Android emulator."]
                    }
                ]
            }),
            "Break down budget alerts",
        );

        assert_eq!(drafts[0]["kind"], TASK_KIND_MANUAL_TEST);
        assert_eq!(drafts[0]["taskType"], TASK_KIND_MANUAL_TEST);

        let task = prompt_task_from_draft(&mut run, drafts[0].clone(), "Break down budget alerts");

        assert_eq!(task.task_type, TASK_KIND_MANUAL_TEST);
        assert_eq!(canonical_task_kind(&task), TASK_KIND_MANUAL_TEST);
        assert_eq!(model_type_for_task(&task), "qa");
        assert!(is_kanban_parent_task(&task));
    }

    #[test]
    fn manual_test_task_skips_tdd_and_uses_verification_prompt() {
        let mut run = board_fixture(json!({
            "command": "Add budget alerts",
            "projectPath": "/tmp/project",
            "tddEnabled": true
        }));
        let task = prompt_task_from_draft(
            &mut run,
            json!({
                "title": "Run mobile functional verification for budget alerts",
                "kind": "manual_test",
                "details": "Run a manual Android emulator smoke test for budget alerts.",
                "acceptanceCriteria": ["Manual verification evidence is recorded."]
            }),
            "Break down budget alerts",
        );

        assert!(!task_requires_tdd(&run, &task));

        let prompt = build_task_execution_prompt(&run, &task, 0);

        assert!(prompt.contains("Prompt template: Manual Test"));
        assert!(prompt.contains(
            "Do not edit source, production, generated, test, config, lock, or documentation files"
        ));
        assert!(prompt.contains("\"suggestedBacklogTasks\""));
        assert!(prompt.contains("\"kind\": \"implementation\""));
        assert!(!prompt.contains("Make minimal changes needed to complete this task correctly."));
    }

    #[test]
    fn manual_test_result_requires_an_explicit_success_signal() {
        for result in [
            "not_run",
            "skipped",
            "unknown",
            "blocked by an unavailable emulator",
            "The flow could not be verified.",
            "Pass, but one step failed.",
        ] {
            assert!(
                !manual_test_result_is_successful(result),
                "indeterminate or failed result should not pass: {result}"
            );
        }

        for result in [
            "Pass: the saved amount is shown after reopening the screen.",
            "All steps passed; the workflow behaved as expected.",
            "Verified successfully with no failures.",
            "No issues observed.",
        ] {
            assert!(
                manual_test_result_is_successful(result),
                "explicit success result should pass: {result}"
            );
        }
    }

    #[test]
    fn legacy_verification_cards_migrate_to_manual_test_kind() {
        let mut run = board_fixture(json!({
            "command": "Add budget alerts",
            "projectPath": "/tmp/project"
        }));
        let mut verification = BoardTask::draft(
            &mut run,
            "Run mobile functional verification for budget alerts".to_string(),
            "Verify the budget alerts workflow locally using Android emulator smoke testing."
                .to_string(),
        );
        verification.task_origin = "user_prompt_generated".to_string();
        verification.prompt_task = true;
        verification.task_type = TASK_KIND_IMPLEMENTATION.to_string();
        let mut unit_tests = BoardTask::draft(
            &mut run,
            "Add unit tests for budget alerts and monthly reset".to_string(),
            "Add focused JVM tests for budget validation and monthly reset.".to_string(),
        );
        unit_tests.task_origin = "user_prompt_generated".to_string();
        unit_tests.prompt_task = true;
        unit_tests.task_type = TASK_KIND_IMPLEMENTATION.to_string();
        run.tasks = vec![verification, unit_tests];

        normalize_board_model(&mut run);

        assert_eq!(run.tasks[0].task_type, TASK_KIND_MANUAL_TEST);
        assert_eq!(run.tasks[1].task_type, TASK_KIND_IMPLEMENTATION);
    }

    #[test]
    fn successful_task_adds_deduplicated_optional_backlog_suggestions() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].title = "Existing card".to_string();
        let status_before = run.status.clone();
        let created = append_suggested_backlog_tasks_from_result(
            &mut run,
            "task-1",
            &json!({
                "suggestedBacklogTasks": [
                    {"title": "Add metrics", "details": "Expose queue metrics."},
                    {"title": "  add   metrics  ", "details": "Duplicate."},
                    {"title": "Existing card", "details": "Already exists."}
                ]
            }),
        );

        assert_eq!(created.len(), 1);
        let task = run.tasks.iter().find(|task| task.id == created[0]).unwrap();
        assert_eq!(task.status, "backlog");
        assert_eq!(task.task_origin, "ai_suggested_backlog");
        assert!(!task.manual_task);
        assert!(!task.prompt_task);
        assert!(task.references[0].contains("Suggested backlog task from task-1"));
        assert_eq!(run.status, status_before);
    }

    #[test]
    fn prompt_backlog_generation_reuses_existing_root_task_trees() {
        let mut run = board_fixture(json!({
            "command": "Existing story",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].title = "Existing story".to_string();

        let duplicate = prompt_task_tree_from_draft(
            &mut run,
            json!({
                "title": " existing   story ",
                "level": "story",
                "details": "A regenerated copy of the existing story."
            }),
            "Regenerate the backlog",
        );
        let missing = prompt_task_tree_from_draft(
            &mut run,
            json!({
                "title": "New reporting story",
                "level": "story",
                "details": "A story that is not yet on the board."
            }),
            "Regenerate the backlog",
        );

        let (kept, reused) = keep_missing_prompt_task_trees(&run, vec![duplicate, missing]);

        assert_eq!(reused, 1);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].title, "New reporting story");
        assert_eq!(run.tasks.len(), 1);
    }

    #[test]
    fn generated_system_validation_work_is_executable_subtask_work() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let story_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();

        let mut parent = BoardTask::draft(
            &mut run,
            "Plan budget persistence".to_string(),
            "Plan the budget persistence scope.".to_string(),
        );
        parent.status = TASK_STATUS_TODO.to_string();
        parent.hierarchy.level = TASK_LEVEL_TASK.to_string();
        parent.hierarchy.parent_id = Some(story_id);
        parent.hierarchy.executable = false;
        let parent_id = parent.id.clone();
        run.tasks.push(parent);

        let mut source = BoardTask::draft(
            &mut run,
            "Implement budget persistence".to_string(),
            "Implement the budget persistence repository.".to_string(),
        );
        source.status = TASK_STATUS_DONE.to_string();
        source.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        source.hierarchy.parent_id = Some(parent_id.clone());
        source.hierarchy.executable = true;
        source.hierarchy.scope_version = 4;
        source.hierarchy.rank = 2;
        source.hierarchy.planned_files = vec!["src/budget.rs".to_string()];
        run.tasks.push(source.clone());

        let qa = create_task_qa_task(&run, &source, "Validate the implementation");
        assert_eq!(task_level(&qa), TASK_LEVEL_SUBTASK);
        assert!(task_is_executable(&qa));
        assert_eq!(qa.hierarchy.parent_id.as_deref(), Some(parent_id.as_str()));
        assert!(task_is_runnable_in_board(&run, &qa));

        let agents =
            create_agents_knowledge_task(&run, "Capture durable project knowledge", Some(&source));
        assert_eq!(task_level(&agents), TASK_LEVEL_SUBTASK);
        assert!(task_is_executable(&agents));
        assert_eq!(
            agents.hierarchy.parent_id.as_deref(),
            Some(parent_id.as_str())
        );
        assert!(task_is_runnable_in_board(&run, &agents));

        run.tasks.push(qa.clone());
        assert!(queue_qa_verdict_retry(
            &mut run,
            &qa.id,
            &json!({
                "status": "needs_followup",
                "summary": "The QA response omitted the final JSON verdict.",
                "remainingIssues": ["Return the required verdict JSON."]
            })
        ));
        let retry = run
            .tasks
            .iter()
            .find(|task| task.qa_verdict_retry_task)
            .expect("QA verdict retry");
        assert_eq!(task_level(retry), TASK_LEVEL_SUBTASK);
        assert!(task_is_executable(retry));
        assert_eq!(
            retry.hierarchy.parent_id.as_deref(),
            Some(parent_id.as_str())
        );
        assert!(retry.depends_on.contains(&qa.id));

        assert!(append_final_qa_task(&mut run, "Final board validation"));
        let final_qa = run
            .tasks
            .iter()
            .find(|task| task.final_qa_task)
            .expect("final QA task");
        assert_eq!(task_level(final_qa), TASK_LEVEL_SUBTASK);
        assert!(task_is_executable(final_qa));
        assert!(task_is_runnable_in_board(&run, final_qa));

        run.promotion_candidates = vec![json!({"id": "candidate-1"})];
        assert!(append_promotion_review_task(&mut run, "Review promotion"));
        let promotion = run
            .tasks
            .iter()
            .find(|task| task.id == PROMOTION_REVIEW_TASK_ID)
            .expect("promotion review task");
        assert_eq!(task_level(promotion), TASK_LEVEL_SUBTASK);
        assert!(task_is_executable(promotion));
    }

    #[test]
    fn board_detail_exposes_parent_task_groups_with_dynamic_subtasks() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let source = run.tasks[0].clone();

        assert!(append_task_qa_task(
            &mut run,
            &source,
            "Validate after implementation"
        ));

        let detail = run.detail_json(None);
        let groups = detail["taskGroups"].as_array().unwrap();
        let group = groups
            .iter()
            .find(|group| group["id"] == "task-1")
            .expect("parent task group exists");

        assert_eq!(detail["taskGroupCounts"]["total"], 1);
        assert_eq!(detail["taskGroupCounts"]["in_progress"], 1);
        assert_eq!(group["status"], TASK_STATUS_IN_PROGRESS);
        assert_eq!(group["primaryTaskId"], "task-1");
        assert_eq!(group["currentSubtaskKind"], TASK_KIND_QA);
        assert_eq!(group["subtaskCounts"]["done"], 1);
        assert_eq!(group["subtaskCounts"]["todo"], 1);
        assert_eq!(group["subtasks"].as_array().unwrap().len(), 2);
        assert!(
            group["subtasks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|task| task["groupId"] == "task-1")
        );
    }

    #[test]
    fn legacy_source_linked_subtasks_inherit_parent_group_id() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].group_id = None;
        let mut qa = BoardTask::draft(&mut run, "QA".to_string(), "Validate".to_string());
        qa.id = "task-qa-1".to_string();
        qa.group_id = None;
        qa.qa_task = true;
        qa.source_task_id = Some("task-1".to_string());
        run.tasks.push(qa);

        normalize_board_model(&mut run);

        assert_eq!(run.tasks[0].group_id.as_deref(), Some("task-1"));
        assert_eq!(run.tasks[1].group_id.as_deref(), Some("task-1"));
        assert_eq!(
            run.detail_json(None)["taskGroups"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn legacy_source_parent_is_not_an_approval_ancestor() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let source_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let mut qa = BoardTask::draft(
            &mut run,
            "QA legacy source link".to_string(),
            "Validate the completed source work.".to_string(),
        );
        qa.id = "task-qa-legacy".to_string();
        qa.status = TASK_STATUS_TODO.to_string();
        qa.qa_task = true;
        qa.task_level_qa = true;
        qa.task_type = TASK_KIND_QA.to_string();
        qa.source_task_id = Some(source_id.clone());
        qa.hierarchy.parent_id = Some(source_id);
        qa.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        qa.hierarchy.executable = true;
        run.tasks.push(qa);

        normalize_board_model(&mut run);

        let qa = run
            .tasks
            .iter()
            .find(|task| task.id == "task-qa-legacy")
            .expect("legacy QA task");
        assert!(qa.hierarchy.parent_id.is_none());
        assert!(task_is_runnable_in_board(&run, qa));
    }

    #[test]
    fn nested_manual_items_inherit_their_parent_group_id() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        let parent_group_id = task_group_id_or_self(&run.tasks[0]);
        let mut child = BoardTask::draft(
            &mut run,
            "Nested manual task".to_string(),
            "Keep this planning item inside its parent feature.".to_string(),
        );
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.group_id = Some(child.id.clone());
        run.tasks.push(child);

        normalize_board_task_groups(&mut run);

        assert_eq!(
            run.tasks[1].group_id.as_deref(),
            Some(parent_group_id.as_str())
        );
        assert_eq!(
            run.detail_json(None)["taskGroups"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn nested_children_inherit_parent_priority_unless_overridden() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].priority = TASK_PRIORITY_P1.to_string();

        let inherited_manual = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Persist budget limits",
                "parentId": parent_id,
                "level": "task"
            }))
            .unwrap(),
        )
        .expect("nested manual child");
        assert_eq!(inherited_manual.priority, TASK_PRIORITY_P1);

        let explicit_manual = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Prioritize budget validation",
                "parentId": "task-1",
                "priority": "p0"
            }))
            .unwrap(),
        )
        .expect("nested manual child with override");
        assert_eq!(explicit_manual.priority, TASK_PRIORITY_P0);

        let inherited_planned = task_from_json(
            &run,
            json!({
                "title": "Add budget repository",
                "parentId": "task-1",
                "level": "subtask"
            }),
            1,
            TASK_STATUS_BACKLOG,
        )
        .expect("nested planned child");
        assert_eq!(inherited_planned.priority, TASK_PRIORITY_P1);
    }

    #[test]
    fn discussion_split_children_inherit_parent_priority_and_allow_overrides() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].priority = TASK_PRIORITY_P0.to_string();

        split_discussion_item(
            &mut run,
            &parent_id,
            &json!({
                "items": [
                    {"title": "Persist budget limits"},
                    {"title": "Validate budget input", "priority": "p3"}
                ]
            }),
        )
        .expect("discussion split");

        let inherited = run
            .tasks
            .iter()
            .find(|task| task.title == "Persist budget limits")
            .expect("inherited split child");
        assert_eq!(inherited.priority, TASK_PRIORITY_P0);
        let overridden = run
            .tasks
            .iter()
            .find(|task| task.title == "Validate budget input")
            .expect("overridden split child");
        assert_eq!(overridden.priority, TASK_PRIORITY_P3);
    }

    #[test]
    fn discussion_split_preserves_optional_children() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();

        split_discussion_item(
            &mut run,
            &parent_id,
            &json!({
                "items": [
                    {"title": "Add optional chart", "required": false},
                    {"title": "Persist budget limits"}
                ]
            }),
        )
        .expect("discussion split");

        let optional = run
            .tasks
            .iter()
            .find(|task| task.title == "Add optional chart")
            .expect("optional split child");
        let required = run
            .tasks
            .iter()
            .find(|task| task.title == "Persist budget limits")
            .expect("required split child");
        assert!(!optional.hierarchy.required);
        assert!(required.hierarchy.required);
    }

    #[test]
    fn discussion_split_validates_acceptance_criteria_before_applying_children() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].acceptance_criteria = vec!["Budget amount can be zero.".to_string()];

        let error = split_discussion_item(
            &mut run,
            &parent_id,
            &json!({
                "items": [{
                    "title": "Persist budget limits",
                    "acceptanceCriteria": ["Budget amount must be positive."]
                }]
            }),
        )
        .unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(run.tasks.len(), 1, "invalid split must be atomic");
        assert!(
            run.tasks[0]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("Acceptance criteria conflict")
        );
    }

    #[test]
    fn manual_task_rejects_unknown_status() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let request = serde_json::from_value::<TaskRequest>(json!({
            "title": "Invalid task",
            "status": "banana"
        }))
        .unwrap();

        let error = BoardTask::manual(&mut run, request).unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.body.error.contains("Task status must be one of"));
    }

    #[test]
    fn hierarchical_manual_task_rejects_runtime_statuses() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let request = serde_json::from_value::<TaskRequest>(json!({
            "title": "Invalid runtime task",
            "status": "done"
        }))
        .unwrap();
        let task = BoardTask::manual(&mut run, request).unwrap();

        let error = validate_manual_task_status(&run, &task).unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn nested_manual_todo_requires_approved_parent() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let request = serde_json::from_value::<TaskRequest>(json!({
            "title": "Nested task",
            "parentId": "task-1",
            "status": "todo"
        }))
        .unwrap();
        let task = BoardTask::manual(&mut run, request).unwrap();

        let error = validate_manual_task_status(&run, &task).unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
    }

    #[test]
    fn nested_todo_breakdown_requires_every_ancestor_to_be_approved() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        run.tasks[0].hierarchy.level = TASK_LEVEL_EPIC.to_string();
        run.tasks[0].hierarchy.executable = false;

        let mut parent = BoardTask::draft(
            &mut run,
            "Plan feature delivery".to_string(),
            "Plan the feature delivery tasks.".to_string(),
        );
        parent.status = TASK_STATUS_TODO.to_string();
        parent.hierarchy.level = TASK_LEVEL_STORY.to_string();
        parent.hierarchy.parent_id = Some(root_id);
        parent.hierarchy.executable = false;
        let parent_id = parent.id.clone();
        run.tasks.push(parent);

        let parent = run
            .tasks
            .iter()
            .find(|task| task.id == parent_id)
            .cloned()
            .expect("nested planning parent");
        let error = validate_hierarchy_breakdown_parent(&run, &parent).unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(next_hierarchy_parent(&run).is_none());

        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        assert!(validate_hierarchy_breakdown_parent(&run, &parent).is_ok());
        assert_eq!(next_hierarchy_parent(&run).unwrap().id, parent_id);
    }

    #[test]
    fn hierarchy_breakdown_detects_reworded_duplicate_titles() {
        assert!(suggested_task_titles_are_semantic_duplicates(
            "Add endpoint PATCH /budget/{month}",
            "Implement PATCH budget month endpoint",
        ));
        assert!(suggested_task_titles_are_semantic_duplicates(
            "Run Android emulator smoke test for budget edit flow",
            "Run smoke testing of the budget editing flow on Android emulator",
        ));
        assert!(!suggested_task_titles_are_semantic_duplicates(
            "Add budget repository",
            "Add budget validation",
        ));
    }

    #[test]
    fn hierarchy_breakdown_refinement_has_a_small_total_child_budget() {
        assert_eq!(hierarchy_breakdown_max_new_children(0), 12);
        assert_eq!(hierarchy_breakdown_max_new_children(1), 4);
        assert_eq!(hierarchy_breakdown_max_new_children(8), 4);
        assert_eq!(hierarchy_breakdown_max_new_children(11), 1);
        assert_eq!(hierarchy_breakdown_max_new_children(12), 0);
        assert_eq!(hierarchy_breakdown_max_new_children(20), 0);
    }

    #[test]
    fn hierarchy_breakdown_prompt_switches_to_gap_check_after_children_exist() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].hierarchy.level = TASK_LEVEL_STORY.to_string();
        run.tasks[0].hierarchy.executable = false;

        let mut child = BoardTask::draft(
            &mut run,
            "Persist budget limits".to_string(),
            "Persist the monthly budget limits.".to_string(),
        );
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id.clone());
        child.hierarchy.executable = false;
        run.tasks.push(child);

        let prompt = build_hierarchy_breakdown_prompt(
            &run,
            run.tasks.iter().find(|task| task.id == parent_id).unwrap(),
            TASK_LEVEL_TASK,
        );

        assert!(prompt.contains("Breakdown mode: refinement / gap check"));
        assert!(prompt.contains("Return no more than 4 genuinely new child item(s)."));
        assert!(prompt.contains("return an empty items array"));
        assert!(prompt.contains("persist budget limits"));
    }

    #[test]
    fn hierarchy_breakdown_child_candidates_include_wrapped_proposals_once() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        let mut wrapper = BoardTask::draft(
            &mut run,
            "Proposed scope: Add budget export".to_string(),
            "Review the generated task scope.".to_string(),
        );
        wrapper.task_origin = "hierarchy_backlog_wrapper".to_string();
        mark_generated_scope_wrapper(
            &mut wrapper,
            &run.tasks[0].clone(),
            "add budget export",
            TASK_LEVEL_TASK,
        );
        run.tasks.push(wrapper);

        let titles = hierarchy_breakdown_child_title_candidates(&run, &parent_id, TASK_LEVEL_TASK);

        assert_eq!(titles, vec!["add budget export".to_string()]);
        assert!(hierarchy_breakdown_has_children(
            &run,
            &parent_id,
            TASK_LEVEL_TASK
        ));
    }

    #[test]
    fn acceptance_criteria_conflict_blocks_hierarchy_planning() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].acceptance_criteria = vec!["Budget amount can be zero.".to_string()];
        let mut child = BoardTask::draft(
            &mut run,
            "Persist budget limits".to_string(),
            "Persist the monthly limit.".to_string(),
        );
        child.id = "task-child".to_string();
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.acceptance_criteria = vec!["Budget amount must be positive.".to_string()];
        run.tasks.push(child);

        let issues = hierarchy_validation_issues(&run);

        assert!(issues.iter().any(|issue| {
            issue.contains("Acceptance criteria conflict")
                && issue.contains("task-1")
                && issue.contains("task-child")
        }));
    }

    #[test]
    fn acceptance_criteria_conflict_is_checked_across_all_ancestors() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let story_id = run.tasks[0].id.clone();
        run.tasks[0].acceptance_criteria = vec!["Budget amount can be zero.".to_string()];

        let mut task = BoardTask::draft(
            &mut run,
            "Persist budget limits".to_string(),
            "Persist the monthly limit.".to_string(),
        );
        task.hierarchy.level = TASK_LEVEL_TASK.to_string();
        task.hierarchy.parent_id = Some(story_id);
        task.acceptance_criteria = vec!["Persist the supplied amount.".to_string()];
        let task_id = task.id.clone();
        run.tasks.push(task);

        let mut subtask = BoardTask::draft(
            &mut run,
            "Validate budget amount".to_string(),
            "Validate the amount before persistence.".to_string(),
        );
        subtask.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        subtask.hierarchy.parent_id = Some(task_id);
        subtask.hierarchy.executable = true;
        subtask.acceptance_criteria = vec!["Budget amount must be positive.".to_string()];
        let subtask_id = subtask.id.clone();
        run.tasks.push(subtask);

        let issues = hierarchy_validation_issues(&run);

        assert!(issues.iter().any(|issue| {
            issue.contains("Acceptance criteria conflict")
                && issue.contains("task-1")
                && issue.contains(subtask_id.as_str())
        }));
    }

    #[test]
    fn board_profiles_normalize_to_the_three_supported_journeys() {
        assert_eq!(normalize_board_profile(Some("minimal")), "minimal");
        assert_eq!(normalize_board_profile(Some("strict")), "minimal");
        assert_eq!(normalize_board_profile(Some("complete")), "complete_app");
        assert_eq!(
            normalize_board_profile(Some("product-ready")),
            "product_ready"
        );
        assert_eq!(normalize_board_profile(Some("quality")), "product_ready");
    }

    #[test]
    fn default_session_policy_is_continuous_for_single_model_boards() {
        let _lock = ENV_TEST_LOCK.lock().unwrap();
        let _env = TestEnvGuard::set(vec![
            ("DANGER_CONTINUOUS_SESSION", None),
            ("IO_WORKBENCH_DANGER_CONTINUOUS_SESSION", None),
        ]);
        let run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "model": "sonnet"
        }));

        assert_eq!(run.session_policy, "continuous");
        assert!(!has_mixed_task_models(&run, None));
    }

    #[test]
    fn hybrid_strategy_switches_continuous_session_to_task_model() {
        let _lock = ENV_TEST_LOCK.lock().unwrap();
        let _env = TestEnvGuard::set(vec![
            ("DANGER_CONTINUOUS_SESSION", None),
            ("IO_WORKBENCH_DANGER_CONTINUOUS_SESSION", None),
        ]);
        let run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "modelStrategy": {
                "mode": "hybrid",
                "cheapModel": "gpt-5-mini",
                "expensiveModel": "gpt-5.6-sol"
            }
        }));

        assert_eq!(run.model, "gpt-5.6-sol");
        assert_eq!(run.board_profile, "complete_app");
        assert_eq!(run.session_policy, "task-model");
        assert_eq!(run.task_model_overrides["implementation"], "gpt-5-mini");
        assert_eq!(run.task_model_overrides["qa"], "gpt-5.6-sol");
    }

    #[test]
    fn manual_task_model_overrides_win_over_strategy_presets() {
        let run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "modelStrategy": {
                "mode": "hybrid",
                "cheapModel": "cheap-model",
                "expensiveModel": "expensive-model"
            },
            "taskModelOverrides": {
                "implementation": "manual-implementation",
                "finalQa": "manual-final"
            }
        }));

        assert_eq!(
            run.task_model_overrides["implementation"],
            "manual-implementation"
        );
        assert_eq!(run.task_model_overrides["final_qa"], "manual-final");
        assert_eq!(run.task_model_overrides["qa"], "expensive-model");
    }

    #[test]
    fn breakdown_phase_routes_provider_with_breakdown_model() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project",
            "provider": "claude",
            "model": "minimax-m3",
            "taskModelOverrides": {
                "breakdown": "gpt-5.5",
                "implementation": "minimax-m3",
                "qa": "minimax-m3"
            }
        }));
        run.backlog_breakdown = json!({
            "status": "idle",
            "prompt": "Break down budget alerts",
            "provider": "codex",
            "model": "gpt-5.5",
            "generatedTaskCount": 3
        });

        assert_eq!(
            effective_provider_for_phase(&run, "breakdown").unwrap(),
            "codex"
        );
        assert_eq!(effective_model_for_phase(&run, "breakdown"), "gpt-5.5");
        assert_eq!(effective_provider_for_phase(&run, "qa").unwrap(), "claude");
        assert_eq!(effective_model_for_phase(&run, "qa"), "minimax-m3");
        assert_eq!(
            agentic_execution_model_for_provider(
                &effective_provider_for_phase(&run, "qa").unwrap(),
                &effective_model_for_phase(&run, "qa"),
            ),
            "min:MiniMax-M3"
        );
        assert_eq!(
            agentic_execution_model_for_provider("codex", "minimax-m3"),
            "minimax-m3"
        );
        assert!(agentic_model_uses_minimax_gateway("min:MiniMax-M3"));
        assert!(agentic_model_uses_minimax_gateway("minimax-m3"));
        assert!(!agentic_model_uses_minimax_gateway("cld:claude-sonnet"));
    }

    #[test]
    fn backlog_breakdown_is_board_state_not_a_task() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "boardProfile": "minimal"
        }));

        run.backlog_breakdown = json!({
            "id": "breakdown-1",
            "status": "running",
            "prompt": "Add export filters",
            "model": "gpt-5.6-sol",
            "boardProfile": "product_ready"
        });
        let detail = run.detail_json(None);

        assert_eq!(detail["backlogBreakdown"]["status"], "running");
        assert_eq!(detail["tasks"].as_array().unwrap().len(), 1);
        assert!(
            detail["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|task| { task.get("id").and_then(Value::as_str) != Some("breakdown-1") })
        );
    }

    #[test]
    fn six_stage_model_routes_select_the_expected_override() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "model": "fallback",
            "taskModelOverrides": {
                "breakdown": "breakdown-model",
                "implementation": "implementation-model",
                "qa": "qa-model",
                "qa_fix": "qa-fix-model",
                "agents": "agents-model",
                "final_qa": "final-qa-model"
            }
        }));
        assert_eq!(
            effective_model_for_phase(&run, "planning breakdown"),
            "breakdown-model"
        );

        let mut implementation = BoardTask::draft(
            &mut run,
            "Implementation".to_string(),
            "Build it".to_string(),
        );
        implementation.status = TASK_STATUS_TODO.to_string();
        let mut qa = implementation.clone();
        qa.qa_task = true;
        let mut qa_fix = implementation.clone();
        qa_fix.qa_fix_task = true;
        let mut agents = implementation.clone();
        agents.agents_knowledge_task = true;
        let mut final_qa = implementation.clone();
        final_qa.final_qa_task = true;

        assert_eq!(
            effective_model_for_task(&run, &implementation),
            "implementation-model"
        );
        assert_eq!(effective_model_for_task(&run, &qa), "qa-model");
        assert_eq!(effective_model_for_task(&run, &qa_fix), "qa-fix-model");
        assert_eq!(effective_model_for_task(&run, &agents), "agents-model");
        assert_eq!(effective_model_for_task(&run, &final_qa), "final-qa-model");
    }

    #[test]
    fn blank_provider_normalizes_to_default_provider() {
        assert_eq!(normalize_provider(Some("")).unwrap(), DEFAULT_PROVIDER);
        assert_eq!(normalize_provider(Some("   ")).unwrap(), DEFAULT_PROVIDER);
        assert_eq!(normalize_provider(None).unwrap(), DEFAULT_PROVIDER);
    }

    #[test]
    fn pause_during_provider_turn_keeps_single_board_worker_owner() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.status = "running".to_string();
        run.active = true;
        run.loop_started = true;
        run.current_task_id = Some("task-1".to_string());
        run.current_task_status = "in_progress".to_string();
        run.current_provider_session_id = Some("provider-session".to_string());

        request_board_pause(&mut run, Some("user request".to_string()));

        assert_eq!(run.status, "pausing");
        assert!(run.active);
        assert!(run.loop_started);
        assert!(run.pause_requested);
        assert!(!run.auto_run_enabled);
        assert!(!board_should_abort_provider(&run));
    }

    #[test]
    fn pause_before_provider_turn_returns_current_card_to_todo() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.status = "running".to_string();
        run.active = true;
        run.loop_started = true;
        run.current_task_id = Some("task-1".to_string());
        run.current_task_status = "in_progress".to_string();
        run.tasks[0].status = "in_progress".to_string();
        run.tasks[0].started_at = Some(Utc::now());
        run.tasks[0].provider_session_id = Some("stale-provider-session".to_string());

        request_board_pause(&mut run, Some("user request".to_string()));

        assert_eq!(run.status, "paused");
        assert!(!run.active);
        assert!(!run.loop_started);
        assert!(!run.pause_requested);
        assert_eq!(run.tasks[0].status, TASK_STATUS_TODO);
        assert_eq!(run.tasks[0].started_at, None);
        assert_eq!(run.tasks[0].provider_session_id, None);
        assert_eq!(run.current_task_id, None);
        assert_eq!(run.control_revision, 1);
    }

    #[test]
    fn stale_board_worker_save_preserves_newer_pause_control_state() {
        let mut stale = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        stale.status = "running".to_string();
        stale.active = true;
        stale.loop_started = true;
        stale.current_task_id = Some("task-1".to_string());
        stale.current_task_status = "in_progress".to_string();
        stale.tasks[0].status = "in_progress".to_string();

        let mut paused = stale.clone();
        request_board_pause(&mut paused, Some("user request".to_string()));

        preserve_newer_control_state(&mut stale, &paused);

        assert_eq!(stale.status, "paused");
        assert!(!stale.active);
        assert!(!stale.loop_started);
        assert_eq!(stale.tasks[0].status, TASK_STATUS_TODO);
        assert_eq!(stale.control_revision, paused.control_revision);
        assert_eq!(stale.pause_reason.as_deref(), Some("user request"));
    }

    #[test]
    fn abort_resets_in_flight_cards_and_control_pointers() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.status = "running".to_string();
        run.active = true;
        run.loop_started = true;
        run.current_task_id = Some("task-1".to_string());
        run.current_task_title = run.tasks[0].title.clone();
        run.current_task_status = "in_progress".to_string();
        run.current_provider_session_id = Some("provider-session".to_string());
        run.provider_call_started_at = Some(Utc::now());
        run.provider_call_label = Some("task execution".to_string());
        run.tasks[0].status = "in_progress".to_string();
        run.tasks[0].provider_session_id = Some("stale-provider-session".to_string());

        reset_in_flight_board_tasks(&mut run, "aborted");
        run.status = "cancelled".to_string();
        run.active = false;
        run.loop_started = false;
        run.current_task_id = None;
        run.current_task_title.clear();
        run.current_task_status.clear();
        run.current_provider_session_id = None;
        run.provider_call_started_at = None;
        run.provider_call_label = None;

        assert_eq!(run.tasks[0].status, TASK_STATUS_TODO);
        assert_eq!(run.tasks[0].provider_session_id, None);
        assert_eq!(run.current_task_id, None);
        assert_eq!(run.current_provider_session_id, None);
        assert_eq!(run.provider_call_started_at, None);
    }

    #[test]
    fn legacy_session_backfill_skips_non_terminal_tasks() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        assert!(!task_allows_legacy_session_backfill(&run.tasks[0]));
        assert!(!task_needs_legacy_session_backfill(&run.tasks[0]));

        run.tasks[0].status = TASK_STATUS_IN_PROGRESS.to_string();
        assert!(!task_allows_legacy_session_backfill(&run.tasks[0]));
        assert!(!task_needs_legacy_session_backfill(&run.tasks[0]));

        run.tasks[0].status = TASK_STATUS_BLOCKED.to_string();
        assert!(task_allows_legacy_session_backfill(&run.tasks[0]));
        assert!(task_needs_legacy_session_backfill(&run.tasks[0]));

        run.tasks[0].provider_session_id = Some("current-provider-session".to_string());
        assert!(task_allows_legacy_session_backfill(&run.tasks[0]));
        assert!(!task_needs_legacy_session_backfill(&run.tasks[0]));
    }

    #[test]
    fn known_session_refs_prefer_latest_prompt_telemetry_for_missing_task_links() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.prompt_telemetry.push(json!({
            "label": "tdd qa generation for task-1",
            "sessionId": "older-session",
            "startedAt": Utc::now() - chrono::Duration::minutes(5),
        }));
        run.prompt_telemetry.push(json!({
            "label": "tdd qa generation for task-1",
            "sessionId": "newer-session",
            "startedAt": Utc::now(),
        }));

        let refs = known_board_session_refs(&run);

        assert_eq!(
            refs.iter()
                .find(|(_, task_id)| task_id.as_deref() == Some("task-1"))
                .map(|(session_id, _)| session_id.as_str()),
            Some("newer-session")
        );
    }

    #[test]
    fn workspace_snapshot_marks_preexisting_untracked_edits_as_unknown() {
        let root = std::env::temp_dir().join(format!(
            "iowb-server-untracked-dir-delta-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("app/src/main/java/example")).expect("create source dir");
        fs::write(
            root.join("app/src/main/java/example/Budget.kt"),
            "class Budget\n",
        )
        .expect("write source");
        std::process::Command::new("git")
            .arg("init")
            .current_dir(&root)
            .output()
            .expect("git init");

        let before = capture_workspace_snapshot(root.to_str().expect("root path"));
        fs::write(
            root.join("app/src/main/java/example/Budget.kt"),
            "class Budget(val limit: Double)\n",
        )
        .expect("update source");
        let after = capture_workspace_snapshot(root.to_str().expect("root path"));
        let delta = summarize_workspace_delta("task-1", &before, &after);
        let touched_paths = change_summary_paths(&delta);

        assert!(
            touched_paths.is_empty(),
            "delta: {}",
            serde_json::to_string_pretty(&delta).unwrap()
        );
        assert_eq!(delta["unknownChangeCount"], 1);
        assert_eq!(
            delta["unknownChanges"][0]["classification"],
            "unknown_change"
        );
        assert_eq!(
            delta["preExistingChanges"][0]["classification"],
            "pre_existing_change"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_delta_attributes_new_or_clean_files_to_subtask() {
        let before = json!({
            "isGit": true,
            "filesByPath": {},
            "files": []
        });
        let after = json!({
            "isGit": true,
            "filesByPath": {
                "src/budget.rs": {
                    "status": "??",
                    "hash": "after"
                }
            },
            "files": [{
                "status": "??",
                "path": "src/budget.rs",
                "hash": "after"
            }]
        });

        let delta = summarize_workspace_delta("task-1", &before, &after);

        assert_eq!(change_summary_paths(&delta), vec!["src/budget.rs"]);
        assert_eq!(delta["attributableChangedFileCount"], 1);
        assert_eq!(
            delta["changedBySubtask"][0]["classification"],
            "changed_by_subtask"
        );
        assert_eq!(delta["unknownChangeCount"], 0);
    }

    #[test]
    fn workspace_delta_attributes_modified_clean_tracked_files_to_subtask() {
        let before = json!({
            "isGit": true,
            "filesByPath": {
                "src/budget.rs": {
                    "status": "clean",
                    "hash": "before"
                }
            },
            "files": [{
                "status": "clean",
                "path": "src/budget.rs",
                "hash": "before"
            }]
        });
        let after = json!({
            "isGit": true,
            "filesByPath": {
                "src/budget.rs": {
                    "status": "M",
                    "hash": "after"
                }
            },
            "files": [{
                "status": "M",
                "path": "src/budget.rs",
                "hash": "after"
            }]
        });

        let delta = summarize_workspace_delta("task-1", &before, &after);

        assert_eq!(change_summary_paths(&delta), vec!["src/budget.rs"]);
        assert_eq!(delta["preExistingChangeCount"], 0);
        assert_eq!(delta["attributableChangedFileCount"], 1);
        assert_eq!(delta["unknownChangeCount"], 0);
    }

    #[test]
    fn legacy_changed_file_claims_are_downgraded_to_unknown() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].changed_files = vec!["src/budget.rs".to_string()];
        run.tasks[0].result = Some(json!({
            "status": "done",
            "changedFiles": ["src/budget.rs"]
        }));
        run.tasks[0].changed_file_summary = Some(json!({
            "touchedFiles": [{"path": "src/budget.rs", "beforeStatus": " M", "afterStatus": " M"}]
        }));

        normalize_board_model(&mut run);

        assert!(run.tasks[0].changed_files.is_empty());
        assert_eq!(
            run.tasks[0].changed_file_summary.as_ref().unwrap()["ownershipPolicy"],
            LEGACY_WORKSPACE_OWNERSHIP_POLICY
        );
        assert_eq!(
            run.tasks[0].changed_file_summary.as_ref().unwrap()["unknownChanges"][0]["classification"],
            "unknown_change"
        );
        assert_eq!(
            run.tasks[0].result.as_ref().unwrap()["changedFiles"],
            json!([])
        );
    }

    #[test]
    fn normalize_board_clears_error_on_done_tasks_with_followups() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let task = &mut run.tasks[0];
        task.status = TASK_STATUS_DONE.to_string();
        task.tdd_phase = "fix_pending".to_string();
        task.error = Some("Needs more work".to_string());
        task.result = Some(json!({
            "status": "needs_followup",
            "summary": "Created a follow-up task",
        }));

        normalize_board_model(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
        assert_eq!(run.tasks[0].error, None);
        assert_eq!(run.tasks[0].tdd_phase, "followup_pending");
    }

    #[test]
    fn immediate_resume_reuses_existing_board_worker_owner() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.status = "running".to_string();
        run.active = true;
        run.loop_started = true;
        run.current_task_id = Some("task-1".to_string());
        run.current_provider_session_id = Some("provider-session".to_string());
        request_board_pause(&mut run, Some("user request".to_string()));

        prepare_board_resume(&mut run);

        assert_eq!(run.status, "running");
        assert!(run.active);
        assert!(run.loop_started);
        assert!(!run.pause_requested);
        assert!(run.auto_run_enabled);
    }

    #[test]
    fn idle_pause_transitions_directly_to_paused() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));

        request_board_pause(&mut run, Some("user request".to_string()));

        assert_eq!(run.status, "paused");
        assert!(!run.active);
        assert!(!run.loop_started);
        assert!(!run.pause_requested);
        assert!(!run.auto_run_enabled);
    }

    #[test]
    fn only_cancellation_interrupts_active_provider_turn() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.canceled_at = Some(Utc::now());
        run.abort_source = Some("Board".to_string());
        run.abort_requested_at = Some(Utc::now());
        assert!(!board_should_abort_provider(&run));

        run.status = "pausing".to_string();
        run.pause_requested = true;
        assert!(!board_should_abort_provider(&run));

        run.status = "cancelled".to_string();
        assert!(board_should_abort_provider(&run));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn manual_hierarchy_breakdown_failure_keeps_parent_retryable() {
        let root = std::env::temp_dir().join(format!(
            "iowb-server-manual-breakdown-failure-{}",
            Uuid::new_v4()
        ));
        let project = root.join("project");
        let config_dir = root.join("config");
        fs::create_dir_all(&project).expect("project directory");
        let state = AppState::initialize(AppConfig {
            host: "127.0.0.1".parse().expect("host"),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 10,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .expect("state initializes");

        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": project.display().to_string(),
            "provider": "codex"
        }));
        run.id = "manual-breakdown-board".to_string();
        run.user_id = Some("user-1".to_string());
        run.status = "paused".to_string();
        run.tasks[0].status = TASK_STATUS_BACKLOG.to_string();
        let parent = run.tasks[0].clone();
        save_board(&state, &run).expect("persist board");

        record_hierarchy_breakdown_failure(
            &state,
            "user-1",
            &run.id,
            &parent,
            "break down this story",
            Utc::now(),
            "Hierarchy breakdown provider call failed: provider unavailable",
            true,
        )
        .expect("record manual failure");

        let stored = load_user_board(&state, "user-1", &run.id)
            .expect("load board")
            .board;
        assert_eq!(stored.status, "paused");
        assert_eq!(stored.tasks[0].status, TASK_STATUS_BACKLOG);
        assert_eq!(
            stored.tasks[0].error.as_deref(),
            Some("Hierarchy breakdown provider call failed: provider unavailable")
        );
        assert_ne!(stored.status, TASK_STATUS_BLOCKED);

        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cursor_cli_arguments_match_new_and_resume_journeys() {
        assert_eq!(
            cursor_cli_args("Implement it", "gpt-5.3-codex", None, false),
            vec![
                "-p",
                "Implement it",
                "--model",
                "gpt-5.3-codex",
                "--output-format",
                "stream-json",
                "-f",
            ]
        );
        assert_eq!(
            cursor_cli_args(
                "Continue",
                "ignored-on-resume",
                Some("cursor-session"),
                true,
            ),
            vec![
                "--resume=cursor-session",
                "-p",
                "Continue",
                "--output-format",
                "stream-json",
                "-f",
                "--trust",
            ]
        );
    }

    #[test]
    fn cursor_ndjson_parser_extracts_session_result_and_usage() {
        let result = parse_cursor_cli_output(
            CursorProcessOutput {
                stdout: [
                    r#"{"type":"system","subtype":"init","session_id":"cursor-1"}"#,
                    r#"{"type":"assistant","message":{"content":[{"text":"partial"}]}}"#,
                    r#"{"type":"result","subtype":"success","result":"final answer","usage":{"input_tokens":12,"cached_input_tokens":3,"output_tokens":8}}"#,
                ]
                .join("\n"),
                stderr: String::new(),
                exit_code: 0,
                interrupted: false,
            },
            None,
        );

        assert_eq!(result.session_id, "cursor-1");
        assert_eq!(result.assistant_text, "final answer");
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.token_usage,
            Some(json!({
                "inputTokens": 12,
                "cachedInputTokens": 3,
                "outputTokens": 8,
                "totalTokens": 20,
            }))
        );
    }

    #[test]
    fn cursor_workspace_trust_output_requests_one_retry() {
        assert!(cursor_workspace_trust_required(
            "Workspace trust required. Pass --trust, --yolo, or -f",
            ""
        ));
        assert!(!cursor_workspace_trust_required(
            "implementation completed",
            ""
        ));
    }

    #[test]
    fn task_ids_remain_unique_after_deletion() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        for title in ["Second task", "Third task"] {
            let request = serde_json::from_value::<TaskRequest>(json!({ "title": title })).unwrap();
            let task = BoardTask::manual(&mut run, request).unwrap();
            run.tasks.push(task);
        }

        delete_board_task(&mut run, "task-2").unwrap();
        let request =
            serde_json::from_value::<TaskRequest>(json!({ "title": "Fourth task" })).unwrap();
        let task = BoardTask::manual(&mut run, request).unwrap();

        assert_eq!(task.id, "task-4");
        assert!(!run.tasks.iter().any(|existing| existing.id == task.id));
    }

    #[test]
    fn board_detail_scrubs_removed_tracking_fields_from_legacy_values() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        run.backlog_breakdown = json!({
            "status": "idle",
            "transcript": [{
                "content": serde_json::to_string(&json!({
                    "requirementIds": ["REQ-1"],
                    "references": ["REQ-1", "src/main.rs"],
                    "required": true
                })).unwrap()
            }]
        });

        normalize_board_model(&mut run);

        let serialized = run.detail_json(None).to_string();
        assert!(!serialized.contains("requirementIds"));
        assert!(!serialized.contains("REQ-1"));
        assert_eq!(
            run.backlog_breakdown["transcript"][0]["content"],
            "{\"references\":[\"src/main.rs\"],\"required\":true}"
        );
    }

    #[test]
    fn prompt_task_or_subtask_requests_are_wrapped_at_story_level() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));

        let task_tree = prompt_task_tree_from_draft(
            &mut run,
            json!({
                "title": "Add UserSummaryCard to user page",
                "level": "subtask",
                "kind": "implementation",
                "details": "Add the UserSummaryCard composable to the user page."
            }),
            "Add card summary user in user page",
        );

        assert_eq!(task_tree.len(), 3);
        assert_eq!(task_level(&task_tree[0]), TASK_LEVEL_STORY);
        assert_eq!(task_level(&task_tree[1]), TASK_LEVEL_TASK);
        assert_eq!(task_level(&task_tree[2]), TASK_LEVEL_SUBTASK);
        assert!(!task_is_executable(&task_tree[0]));
        assert!(!task_is_executable(&task_tree[1]));
        assert!(task_is_executable(&task_tree[2]));
        assert_eq!(
            task_tree[1].hierarchy.parent_id.as_deref(),
            Some(task_tree[0].id.as_str())
        );
        assert_eq!(
            task_tree[2].hierarchy.parent_id.as_deref(),
            Some(task_tree[1].id.as_str())
        );
    }

    #[test]
    fn child_level_is_inferred_from_parent_instead_of_trusting_model_level() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let story_id = run.tasks[0].id.clone();
        let task = task_from_json(
            &run,
            json!({
                "title": "Persist budget limits",
                "level": "subtask",
                "parentId": story_id,
                "kind": "implementation"
            }),
            1,
            TASK_STATUS_TODO,
        )
        .expect("child ticket");

        assert_eq!(task_level(&task), TASK_LEVEL_TASK);
        assert!(!task_is_executable(&task));
    }

    #[test]
    fn parent_todo_approval_survives_backlog_children_until_they_are_approved() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Persist budget limits".to_string(),
            "Persist the monthly limit.".to_string(),
        );
        child.status = TASK_STATUS_BACKLOG.to_string();
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = false;
        run.tasks.push(child);

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_TODO);
    }

    #[test]
    fn only_approved_subtasks_are_runnable() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Add budget repository".to_string(),
            "Add the repository method.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(run.tasks[0].id.clone());
        child.hierarchy.executable = true;
        run.tasks.push(child);

        assert!(!task_is_runnable(&run.tasks[0]));
        assert_eq!(pick_next_task_index(&run), Some(1));
    }

    #[test]
    fn scheduler_uses_priority_then_rank_for_approved_subtasks() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        for (title, priority, rank) in [
            ("Low priority", TASK_PRIORITY_P2, 0),
            ("High priority later rank", TASK_PRIORITY_P1, 8),
            ("High priority first rank", TASK_PRIORITY_P1, 2),
        ] {
            let mut child = BoardTask::draft(&mut run, title.to_string(), title.to_string());
            child.status = TASK_STATUS_TODO.to_string();
            child.priority = priority.to_string();
            child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
            child.hierarchy.parent_id = Some(parent_id.clone());
            child.hierarchy.executable = true;
            child.hierarchy.rank = rank;
            run.tasks.push(child);
        }

        assert_eq!(pick_next_task_index(&run), Some(3));
        assert_eq!(run.tasks[3].title, "High priority first rank");
    }

    #[test]
    fn discussion_proposal_has_structured_diff_without_mutating_ticket() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let task_id = run.tasks[0].id.clone();
        let before_title = run.tasks[0].title.clone();
        let proposal = sanitize_discussion_proposal(
            &run,
            &task_id,
            "proposal-1",
            "edit",
            "Rename the story",
            &Value::Null,
            &json!({
                "action": "edit",
                "summary": "Use a clearer story title.",
                "payload": {"title": "User can manage monthly budgets"}
            }),
            "codex",
            "gpt-5.5",
            Utc::now(),
        )
        .expect("proposal");

        assert_eq!(run.tasks[0].title, before_title);
        assert_eq!(proposal["status"], "pending");
        assert_eq!(proposal["diff"]["changed"], true);
        assert!(!proposal["diff"]["changes"].as_array().unwrap().is_empty());
        assert_eq!(
            proposal["payload"]["title"],
            "User can manage monthly budgets"
        );
    }

    #[test]
    fn rejected_discussion_proposal_leaves_ticket_unchanged_until_apply() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let task_id = run.tasks[0].id.clone();
        let original = run.tasks[0].title.clone();
        let proposal = sanitize_discussion_proposal(
            &run,
            &task_id,
            "proposal-2",
            "edit",
            "Rename the story",
            &Value::Null,
            &json!({
                "action": "edit",
                "payload": {"title": "User can manage monthly budgets"}
            }),
            "codex",
            "gpt-5.5",
            Utc::now(),
        )
        .expect("proposal");
        append_discussion_proposal(&mut run, &task_id, proposal.clone()).expect("store proposal");

        let mut rejected = proposal;
        rejected["status"] = json!("rejected");
        update_discussion_proposal(&mut run, &task_id, rejected);
        assert_eq!(run.tasks[0].title, original);

        let proposal = sanitize_discussion_proposal(
            &run,
            &task_id,
            "proposal-3",
            "edit",
            "Rename the story",
            &Value::Null,
            &json!({
                "action": "edit",
                "payload": {"title": "User can manage monthly budgets"}
            }),
            "codex",
            "gpt-5.5",
            Utc::now(),
        )
        .expect("proposal");
        apply_discussion_action(
            &mut run,
            &task_id,
            proposal["action"].as_str().unwrap(),
            &proposal["payload"],
        )
        .expect("apply proposal");
        assert_eq!(run.tasks[0].title, "User can manage monthly budgets");
    }

    #[test]
    fn locked_scope_discussion_proposal_stays_pending_with_warning() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let task_id = run.tasks[0].id.clone();
        let proposal = sanitize_discussion_proposal(
            &run,
            &task_id,
            "proposal-locked",
            "edit",
            "Change the approved story",
            &Value::Null,
            &json!({
                "action": "edit",
                "payload": {"title": "Changed after approval"}
            }),
            "codex",
            "gpt-5.5",
            Utc::now(),
        )
        .expect("proposal remains pending");

        assert_eq!(proposal["status"], "pending");
        assert!(
            proposal["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|warning| warning.as_str().unwrap_or_default().contains("locked"))
        );
        assert!(apply_discussion_action(&mut run, &task_id, "edit", &proposal["payload"]).is_err());
        assert_eq!(run.tasks[0].title, "Implement budget controls");
    }

    #[test]
    fn dependency_blocker_returns_to_todo_after_completion() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let blocker_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let mut dependent = BoardTask::draft(
            &mut run,
            "Add budget repository".to_string(),
            "Add the repository method.".to_string(),
        );
        dependent.status = TASK_STATUS_BLOCKED.to_string();
        dependent.error = Some(format!("Waiting on dependency: {blocker_id}"));
        dependent.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        dependent.hierarchy.parent_id = Some(blocker_id.clone());
        dependent.hierarchy.executable = true;
        dependent.hierarchy.blocked_by = vec![blocker_id.clone()];
        dependent.depends_on = vec![blocker_id];
        run.tasks.push(dependent);

        reconcile_dependency_statuses(&mut run);

        assert_eq!(run.tasks[1].status, TASK_STATUS_TODO);
        assert_eq!(run.tasks[1].error, None);
    }

    #[test]
    fn missing_dependency_stays_blocked_with_explicit_reason() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let mut dependent = BoardTask::draft(
            &mut run,
            "Add budget repository".to_string(),
            "Add the repository method.".to_string(),
        );
        dependent.status = TASK_STATUS_TODO.to_string();
        dependent.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        dependent.hierarchy.parent_id = Some(run.tasks[0].id.clone());
        dependent.hierarchy.executable = true;
        dependent.hierarchy.blocked_by = vec!["deleted-blocker".to_string()];
        dependent.depends_on = dependent.hierarchy.blocked_by.clone();
        run.tasks.push(dependent);

        mark_dependency_blockers(&mut run);

        assert_eq!(run.tasks[1].status, TASK_STATUS_BLOCKED);
        assert_eq!(
            run.tasks[1].error.as_deref(),
            Some("Missing dependency: deleted-blocker")
        );
    }

    #[test]
    fn superseded_dependency_stays_blocked_until_user_relinks_it() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let source_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        run.tasks[0].superseded_by = Some("replacement-1".to_string());

        let mut dependent = BoardTask::draft(
            &mut run,
            "Use the approved budget behavior".to_string(),
            "Continue only after the source behavior is selected.".to_string(),
        );
        dependent.status = TASK_STATUS_TODO.to_string();
        dependent.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        dependent.hierarchy.parent_id = Some(source_id.clone());
        dependent.hierarchy.executable = true;
        dependent.hierarchy.blocked_by = vec![source_id.clone()];
        dependent.depends_on = vec![source_id];
        let dependent_id = dependent.id.clone();
        run.tasks.push(dependent);

        assert_eq!(
            unmet_task_dependencies(
                &run,
                run.tasks
                    .iter()
                    .find(|task| task.id == dependent_id)
                    .unwrap(),
            ),
            vec!["task-1"]
        );
        mark_dependency_blockers(&mut run);

        let dependent = run
            .tasks
            .iter()
            .find(|task| task.id == dependent_id)
            .unwrap();
        assert_eq!(dependent.status, TASK_STATUS_BLOCKED);
        assert!(dependent.error.as_deref().is_some_and(|error| {
            error.contains("Superseded dependency:") && error.contains("replacement-1")
        }));
        assert!(task_failure_is_dependency_blocked(dependent));

        reconcile_dependency_statuses(&mut run);
        assert_eq!(
            run.tasks
                .iter()
                .find(|task| task.id == dependent_id)
                .unwrap()
                .status,
            TASK_STATUS_BLOCKED
        );
    }

    #[test]
    fn scope_change_does_not_mark_dependency_on_retained_root_missing() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        let mut child = BoardTask::draft(
            &mut run,
            "Old generated plan".to_string(),
            "Old generated plan.".to_string(),
        );
        child.hierarchy.parent_id = Some(root_id.clone());
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.executable = false;
        run.tasks.push(child);

        let mut dependent = BoardTask::draft(
            &mut run,
            "Continue the root scope".to_string(),
            "This item depends on the retained root scope.".to_string(),
        );
        dependent.status = TASK_STATUS_TODO.to_string();
        dependent.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        dependent.hierarchy.executable = true;
        dependent.hierarchy.blocked_by = vec![root_id.clone()];
        dependent.depends_on = vec![root_id.clone()];
        let dependent_id = dependent.id.clone();
        run.tasks.push(dependent);

        let removed = delete_generated_descendants_for_scope_change(&mut run, &root_id).unwrap();
        assert_eq!(removed, 1);
        let dependent = run
            .tasks
            .iter()
            .find(|task| task.id == dependent_id)
            .unwrap();
        assert_eq!(dependent.status, TASK_STATUS_TODO);
        assert_eq!(dependent.error, None);
    }

    #[test]
    fn hierarchy_cycle_is_reported_as_a_planning_error() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        run.tasks[0].hierarchy.level = TASK_LEVEL_STORY.to_string();
        run.tasks[0].hierarchy.parent_id = Some("task-2".to_string());
        let mut child = BoardTask::draft(
            &mut run,
            "Nested story".to_string(),
            "Nested story scope.".to_string(),
        );
        child.id = "task-2".to_string();
        child.hierarchy.level = TASK_LEVEL_STORY.to_string();
        child.hierarchy.parent_id = Some(root_id);
        run.tasks.push(child);

        let issues = hierarchy_validation_issues(&run);

        assert!(issues.iter().any(|issue| issue.contains("Hierarchy cycle")));
    }

    #[test]
    fn changing_backlog_parent_removes_unexecuted_descendants() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_BACKLOG.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Add budget repository".to_string(),
            "Add the repository method.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id.clone());
        child.hierarchy.executable = true;
        run.tasks.push(child);

        edit_backlog_task(
            &mut run,
            &parent_id,
            &UpdateTaskRequest {
                title: Some("Implement revised budget controls".to_string()),
                ..UpdateTaskRequest::default()
            },
        )
        .expect("backlog parent edit succeeds");

        assert_eq!(run.tasks.len(), 1);
        assert_eq!(run.tasks[0].title, "Implement revised budget controls");
    }

    #[test]
    fn backlog_child_can_be_edited_when_parent_is_already_approved() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Run the risky migration".to_string(),
            "Apply the database migration.".to_string(),
        );
        child.status = TASK_STATUS_BACKLOG.to_string();
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = false;
        let child_id = child.id.clone();
        run.tasks.push(child);

        assert!(task_scope_owner_is_backlog(&run, &child_id));
        edit_backlog_task(
            &mut run,
            &child_id,
            &UpdateTaskRequest {
                title: Some("Run the approved migration".to_string()),
                side_effects: Some(json!(["database schema"])),
                ..UpdateTaskRequest::default()
            },
        )
        .expect("backlog child edit succeeds");

        let child = run.tasks.iter().find(|task| task.id == child_id).unwrap();
        assert_eq!(child.title, "Run the approved migration");
        assert_eq!(child.hierarchy.side_effects, vec!["database schema"]);
        assert!(!child.hierarchy.side_effects_approved);
    }

    #[test]
    fn generic_evidence_does_not_count_as_recorded_scope_effect() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_BACKLOG.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Inspect the budget flow".to_string(),
            "Inspect the current budget flow.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id.clone());
        child.hierarchy.executable = true;
        child.evidence = vec!["The current flow was inspected.".to_string()];
        let child_id = child.id.clone();
        run.tasks.push(child);

        edit_backlog_task(
            &mut run,
            &parent_id,
            &UpdateTaskRequest {
                title: Some("Implement revised budget controls".to_string()),
                ..UpdateTaskRequest::default()
            },
        )
        .expect("scope edit ignores non-effect evidence");

        assert!(!run.tasks.iter().any(|task| task.id == child_id));
    }

    #[test]
    fn completion_evidence_gate_blocks_empty_subtask_result() {
        let run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let mut task = run.tasks[0].clone();
        task.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        task.hierarchy.executable = true;
        let mut scoped = run.clone();
        scoped.tasks[0] = task;

        let result = apply_completion_evidence_gate(
            &scoped,
            "task-1",
            json!({"status": "done", "summary": "finished"}),
            &json!({"touchedFileCount": 0}),
        );

        assert_eq!(result["status"], "needs_followup");
        assert_eq!(result["qaResult"], "blocked");
        assert_eq!(result["evidenceGate"]["passed"], false);
    }

    #[test]
    fn evidence_gate_failure_cannot_be_normalized_back_to_done() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        run.tasks[0].result = Some(json!({
            "status": "needs_followup",
            "summary": "No evidence",
            "evidenceGate": {
                "passed": false,
                "kind": "implementation"
            }
        }));

        normalize_done_followup_task(&mut run.tasks[0]);

        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
        assert_eq!(run.tasks[0].qa_passed, Some(false));
        assert!(run.tasks[0].completed_at.is_none());
    }

    #[test]
    fn completed_attempt_keeps_its_execution_snapshot() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let started_at = Utc::now();
        let finished_at = started_at + chrono::Duration::seconds(3);
        let task = &mut run.tasks[0];
        task.transcript = vec![json!({
            "timestamp": started_at,
            "kind": "status",
            "content": "Task execution started"
        })];
        task.commands_run = vec!["cargo test".to_string()];
        task.changed_files = vec!["src/budget.rs".to_string()];
        task.evidence = vec!["The budget flow passed.".to_string()];
        task.summary = "Budget flow implemented".to_string();
        task.hierarchy.attempts.push(json!({
            "attemptId": "attempt-1",
            "attemptNumber": 1,
            "startedAt": started_at,
            "status": "running",
            "transcriptStartIndex": 0
        }));

        finish_task_attempt(
            &mut run,
            "task-1",
            "attempt-1",
            TASK_STATUS_DONE,
            finished_at,
        );

        let attempt = &run.tasks[0].hierarchy.attempts[0];
        assert_eq!(attempt["status"], TASK_STATUS_DONE);
        assert_eq!(attempt["commands"][0], "cargo test");
        assert_eq!(attempt["filesChanged"][0], "src/budget.rs");
        assert_eq!(attempt["evidence"][0], "The budget flow passed.");
        assert_eq!(
            attempt["transcript"][0]["content"],
            "Task execution started"
        );
        assert_eq!(attempt["summary"], "Budget flow implemented");
        assert!(attempt.get("transcriptStartIndex").is_none());
    }

    #[test]
    fn external_side_effects_require_declaration_and_approval_before_todo() {
        let mut run = board_fixture(json!({
            "command": "Implement database migration",
            "projectPath": "/tmp/project"
        }));
        let mut task = BoardTask::draft(
            &mut run,
            "Run database migration".to_string(),
            "Apply the database migration.".to_string(),
        );
        task.status = TASK_STATUS_TODO.to_string();
        task.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        task.hierarchy.parent_id = Some(run.tasks[0].id.clone());
        task.hierarchy.executable = true;
        task.hierarchy.side_effects = vec!["database schema".to_string()];
        run.tasks[0].status = TASK_STATUS_TODO.to_string();

        let error = validate_manual_task_status(&run, &task).unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(error.body.error.contains("approval"));

        task.hierarchy.side_effects_approved = true;
        validate_manual_task_status(&run, &task).expect("approved side effects are runnable");
    }

    #[test]
    fn revoking_side_effect_approval_blocks_a_todo_subtask() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Run approved migration".to_string(),
            "Apply the database migration after approval.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.task_type = TASK_KIND_MIGRATION.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = true;
        child.hierarchy.side_effects = vec!["database schema".to_string()];
        child.hierarchy.side_effects_approved = true;
        let child_id = child.id.clone();
        run.tasks.push(child);

        approve_task_side_effects_in_board(&mut run, &child_id, "user-1", false, None)
            .expect("revocation should be accepted");

        let child = run
            .tasks
            .iter()
            .find(|task| task.id == child_id)
            .expect("migration subtask");
        assert_eq!(child.status, TASK_STATUS_BLOCKED);
        assert!(
            child
                .error
                .as_deref()
                .is_some_and(|error| error.contains("approval"))
        );
    }

    #[test]
    fn completion_evidence_gate_requires_external_side_effect_evidence() {
        let run = board_fixture(json!({
            "command": "Implement migration",
            "projectPath": "/tmp/project"
        }));
        let mut task = run.tasks[0].clone();
        task.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        task.hierarchy.executable = true;
        task.hierarchy.side_effects = vec!["database schema".to_string()];
        task.hierarchy.side_effects_approved = true;
        let mut scoped = run.clone();
        scoped.tasks[0] = task;

        let missing = apply_completion_evidence_gate(
            &scoped,
            "task-1",
            json!({
                "status": "done",
                "summary": "migration applied",
                "changedFiles": ["migrations/001.sql"],
                "commandsRun": ["migration command"],
                "evidence": ["migration completed"]
            }),
            &json!({
                "touchedFileCount": 1,
                "attributableChangedFileCount": 1,
                "changedBySubtask": [{"path": "migrations/001.sql"}]
            }),
        );
        assert_eq!(missing["status"], "needs_followup");
        assert_eq!(missing["evidenceGate"]["passed"], false);
        assert!(
            missing["remainingIssues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue
                    .as_str()
                    .unwrap_or_default()
                    .contains("externalSideEffects"))
        );

        let recorded = apply_completion_evidence_gate(
            &scoped,
            "task-1",
            json!({
                "status": "done",
                "summary": "migration applied",
                "changedFiles": ["migrations/001.sql"],
                "commandsRun": ["migration command"],
                "evidence": ["migration completed"],
                "externalSideEffects": ["Database schema changed as planned."]
            }),
            &json!({
                "touchedFileCount": 1,
                "attributableChangedFileCount": 1,
                "changedBySubtask": [{"path": "migrations/001.sql"}]
            }),
        );
        assert_eq!(recorded["status"], "done");
    }

    #[test]
    fn manual_test_completion_requires_and_records_environment() {
        let run = board_fixture(json!({
            "command": "Verify budget controls",
            "projectPath": "/tmp/project"
        }));
        let mut task = run.tasks[0].clone();
        task.task_type = TASK_KIND_MANUAL_TEST.to_string();
        task.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        task.hierarchy.executable = true;
        let mut scoped = run.clone();
        scoped.tasks[0] = task;

        let missing = apply_completion_evidence_gate(
            &scoped,
            "task-1",
            json!({
                "status": "done",
                "summary": "The flow works",
                "evidence": ["Opened the budget screen"]
            }),
            &json!({ "touchedFileCount": 0 }),
        );
        assert_eq!(missing["status"], "needs_followup");
        assert!(
            missing["remainingIssues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue
                    .as_str()
                    .unwrap_or_default()
                    .contains("manualTestEnvironment"))
        );

        let recorded = apply_completion_evidence_gate(
            &scoped,
            "task-1",
            json!({
                "status": "done",
                "summary": "The flow works",
                "evidence": ["Opened the budget screen"],
                "manualTestEnvironment": {
                    "device": "emulator-5560",
                    "version": "debug-42",
                    "baseUrl": "http://10.0.2.2:8100"
                },
                "manualSteps": [
                    "Open the budget screen",
                    "Enter a valid amount and save it"
                ],
                "manualResult": "Pass: the saved amount is shown after reopening the screen."
            }),
            &json!({ "touchedFileCount": 0 }),
        );
        assert_eq!(recorded["status"], "done");
        assert_eq!(
            recorded["manualTestEnvironment"]["deviceOrEmulator"],
            "emulator-5560"
        );
        assert_eq!(recorded["manualTestEnvironment"]["appVersion"], "debug-42");
        assert_eq!(
            recorded["manualTestEnvironment"]["backendUrl"],
            "http://10.0.2.2:8100"
        );
        assert_eq!(recorded["manualTestSteps"][0], "Open the budget screen");
        assert!(
            recorded["manualTestResult"]
                .as_str()
                .unwrap()
                .starts_with("Pass:")
        );
    }

    #[test]
    fn scope_effect_resolution_demotes_scope_and_creates_explicit_revert_work() {
        let mut run = board_fixture(json!({
            "command": "Update budget controls",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        run.tasks[0].priority = TASK_PRIORITY_P1.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Update budget persistence".to_string(),
            "Change the budget persistence implementation.".to_string(),
        );
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(root_id.clone());
        child.hierarchy.executable = false;
        child.status = TASK_STATUS_TODO.to_string();
        child.changed_files = vec!["src/budget.rs".to_string()];
        let child_id = child.id.clone();
        run.tasks.push(child);

        resolve_scope_effects_in_board(
            &mut run,
            &root_id,
            "user-1",
            "revert",
            Some("The parent scope changed.".to_string()),
        )
        .expect("scope effects should be resolvable while paused");

        let root = run.tasks.iter().find(|task| task.id == root_id).unwrap();
        assert_eq!(root.status, TASK_STATUS_BACKLOG);
        assert!(!run.tasks.iter().any(|task| task.id == child_id));
        let revert = run
            .tasks
            .iter()
            .find(|task| task.task_type == TASK_KIND_REVERT)
            .expect("explicit revert work");
        assert_eq!(revert.status, TASK_STATUS_BACKLOG);
        assert_eq!(revert.priority, TASK_PRIORITY_P1);
        assert!(revert.hierarchy.executable);
        assert!(
            revert
                .hierarchy
                .planned_files
                .contains(&"src/budget.rs".to_string())
        );
    }

    #[test]
    fn terminal_ancestor_rejects_nested_todo_item() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Nested implementation".to_string(),
            "Implement the nested work.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = true;

        let error = validate_manual_task_status(&run, &child).unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(error.body.error.contains("parent"));
    }

    #[test]
    fn optional_child_can_be_approved_after_required_parent_is_done() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();

        let mut child = BoardTask::draft(
            &mut run,
            "Add optional chart".to_string(),
            "Add a nice-to-have chart without changing the completed scope.".to_string(),
        );
        child.status = TASK_STATUS_BACKLOG.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = true;
        child.hierarchy.required = false;
        run.tasks.push(child);

        let child_id = run.tasks[1].id.clone();
        assert!(task_ancestors_are_approved(&run, &run.tasks[1]));
        assert!(!task_is_runnable_in_board(&run, &run.tasks[1]));

        run.tasks[1].status = TASK_STATUS_TODO.to_string();
        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
        assert!(task_is_runnable_in_board(
            &run,
            run.tasks.iter().find(|task| task.id == child_id).unwrap()
        ));
    }

    #[test]
    fn completed_parent_rejects_nested_backlog_item_creation() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let task = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Late nested planning item",
                "parentId": parent_id,
                "status": "backlog"
            }))
            .unwrap(),
        )
        .expect("manual child");

        let error = validate_manual_task_status(&run, &task).unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(error.body.error.contains("completed parent"));
    }

    #[test]
    fn completed_parent_rejects_nested_scope_edits() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Preserved child".to_string(),
            "Keep this child under the completed parent.".to_string(),
        );
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.status = TASK_STATUS_BACKLOG.to_string();
        let child_id = child.id.clone();
        run.tasks.push(child);

        let error = edit_backlog_task(
            &mut run,
            &child_id,
            &UpdateTaskRequest {
                title: Some("Edited child".to_string()),
                ..UpdateTaskRequest::default()
            },
        )
        .unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(error.body.error.contains("completed parent"));
    }

    #[test]
    fn completed_parent_rejects_user_child_detachment() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        let child = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Preserve child",
                "parentId": parent_id,
                "status": "backlog"
            }))
            .unwrap(),
        )
        .expect("manual child");
        let child_id = child.id.clone();
        run.tasks.push(child);

        let error = detach_user_created_child(&mut run, &child_id).unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(error.body.error.contains("completed parent"));
    }

    #[test]
    fn parent_rollup_blocks_on_unmet_dependency() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Wait for dependency".to_string(),
            "Run only after the missing dependency is resolved.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = true;
        child.hierarchy.blocked_by = vec!["missing-task".to_string()];
        child.depends_on = child.hierarchy.blocked_by.clone();
        run.tasks.push(child);

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
    }

    #[test]
    fn parent_rollup_blocks_on_unapproved_side_effects() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut child = BoardTask::draft(
            &mut run,
            "Run the migration".to_string(),
            "Apply the database migration.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.task_type = TASK_KIND_MIGRATION.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = true;
        child.hierarchy.side_effects = vec!["database schema".to_string()];
        run.tasks.push(child);

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
    }

    #[test]
    fn parent_rollup_waits_for_research_acceptance_before_done() {
        let mut run = board_fixture(json!({
            "command": "Explore feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut research = BoardTask::draft(
            &mut run,
            "Compare approaches".to_string(),
            "Compare the viable approaches.".to_string(),
        );
        research.status = TASK_STATUS_DONE.to_string();
        research.task_type = TASK_KIND_RESEARCH.to_string();
        research.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        research.hierarchy.parent_id = Some(parent_id);
        research.hierarchy.executable = true;
        run.tasks.push(research);

        refresh_hierarchy_rollups(&mut run);
        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);

        run.tasks[1].hierarchy.research_accepted = true;
        refresh_hierarchy_rollups(&mut run);
        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
    }

    #[test]
    fn optional_children_do_not_block_required_parent_and_optional_only_rollups_stay_consistent() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        run.tasks[0].hierarchy.level = TASK_LEVEL_TASK.to_string();
        run.tasks[0].hierarchy.executable = false;

        let mut optional = BoardTask::draft(
            &mut run,
            "Add optional chart".to_string(),
            "Add a nice-to-have chart.".to_string(),
        );
        optional.status = TASK_STATUS_BACKLOG.to_string();
        optional.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        optional.hierarchy.parent_id = Some(parent_id);
        optional.hierarchy.executable = true;
        optional.hierarchy.required = false;
        run.tasks.push(optional);

        // A backlog nice-to-have is not implicit work and does not hold an
        // approved parent open when it is the only remaining child.
        refresh_hierarchy_rollups(&mut run);
        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
        assert!(!hierarchical_work_is_complete(&run));

        // A user must approve the optional executable item while its parent
        // is still an approved planning scope; a completed parent is an
        // immutable boundary.
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        run.tasks[1].status = TASK_STATUS_TODO.to_string();
        refresh_hierarchy_rollups(&mut run);
        assert_eq!(run.tasks[0].status, TASK_STATUS_IN_PROGRESS);
        assert!(!hierarchical_work_is_complete(&run));

        run.tasks[1].status = TASK_STATUS_DONE.to_string();
        refresh_hierarchy_rollups(&mut run);
        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
        assert!(hierarchical_work_is_complete(&run));
    }

    #[test]
    fn optional_children_do_not_block_a_required_parent_from_becoming_done() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        run.tasks[0].hierarchy.level = TASK_LEVEL_TASK.to_string();
        run.tasks[0].hierarchy.executable = false;

        let mut required = BoardTask::draft(
            &mut run,
            "Implement core behavior".to_string(),
            "Implement the required behavior.".to_string(),
        );
        required.status = TASK_STATUS_DONE.to_string();
        required.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        required.hierarchy.parent_id = Some(parent_id.clone());
        required.hierarchy.executable = true;
        required.hierarchy.required = true;

        let mut optional = BoardTask::draft(
            &mut run,
            "Add optional chart".to_string(),
            "Add a nice-to-have chart.".to_string(),
        );
        optional.status = TASK_STATUS_BACKLOG.to_string();
        optional.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        optional.hierarchy.parent_id = Some(parent_id);
        optional.hierarchy.executable = true;
        optional.hierarchy.required = false;
        run.tasks.extend([required, optional]);

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_DONE);
    }

    #[test]
    fn parent_rollup_blocks_when_ancestor_is_not_approved() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        let mut parent = BoardTask::draft(
            &mut run,
            "Plan feature".to_string(),
            "Plan the feature implementation.".to_string(),
        );
        parent.status = TASK_STATUS_TODO.to_string();
        parent.hierarchy.level = TASK_LEVEL_TASK.to_string();
        parent.hierarchy.parent_id = Some(root_id.clone());
        parent.hierarchy.executable = false;
        let parent_id = parent.id.clone();
        run.tasks.push(parent);
        let mut child = BoardTask::draft(
            &mut run,
            "Implement nested feature".to_string(),
            "Implement the nested feature.".to_string(),
        );
        child.status = TASK_STATUS_TODO.to_string();
        child.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.hierarchy.executable = true;
        run.tasks.push(child);

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[1].status, TASK_STATUS_BLOCKED);
        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
    }

    #[test]
    fn revision_and_replacement_kinds_are_normalized() {
        assert_eq!(
            normalized_task_kind_name("revision"),
            Some(TASK_KIND_REVISION)
        );
        assert_eq!(
            normalized_task_kind_name("replacement"),
            Some(TASK_KIND_REPLACEMENT)
        );
        assert_eq!(
            normalized_task_kind_name("replace"),
            Some(TASK_KIND_REPLACEMENT)
        );
    }

    #[test]
    fn applying_an_explicit_replacement_marks_done_source_superseded() {
        let mut run = board_fixture(json!({
            "command": "Ship budget controls",
            "projectPath": "/tmp/project"
        }));
        let source_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();

        apply_discussion_action(
            &mut run,
            &source_id,
            "replacement",
            &json!({
                "title": "Replace budget controls",
                "details": "Replace the completed budget behavior.",
                "kind": "replacement",
                "supersedeSource": true
            }),
        )
        .expect("replacement should be created");

        let source = run.tasks.iter().find(|task| task.id == source_id).unwrap();
        let replacement = run
            .tasks
            .iter()
            .find(|task| task.task_type == TASK_KIND_REPLACEMENT)
            .expect("replacement item");
        assert_eq!(
            source.superseded_by.as_deref(),
            Some(replacement.id.as_str())
        );
        assert!(
            source
                .references
                .iter()
                .any(|reference| { reference.contains("Superseded by linked replacement") })
        );
        assert_eq!(replacement.status, TASK_STATUS_BACKLOG);
        assert!(!task_is_executable(replacement));
    }

    #[test]
    fn replacement_without_explicit_supersession_keeps_source_history_open() {
        let mut run = board_fixture(json!({
            "command": "Ship budget controls",
            "projectPath": "/tmp/project"
        }));
        let source_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();

        apply_discussion_action(
            &mut run,
            &source_id,
            "replacement",
            &json!({
                "title": "Explore another budget behavior",
                "details": "Plan an alternative without superseding the source.",
                "kind": "replacement"
            }),
        )
        .expect("replacement should be created");

        let source = run.tasks.iter().find(|task| task.id == source_id).unwrap();
        assert_eq!(source.superseded_by, None);
    }

    #[test]
    fn moving_backlog_clears_external_side_effect_approval() {
        let mut task = BoardTask::draft(
            &mut board_fixture(json!({
                "command": "Implement migration",
                "projectPath": "/tmp/project"
            })),
            "Run migration".to_string(),
            "Run the migration.".to_string(),
        );
        task.hierarchy.side_effects_approved = true;
        task.hierarchy.side_effect_approval = Some(json!({"approved": true}));

        clear_backlog_approval(&mut task);

        assert!(!task.hierarchy.side_effects_approved);
        assert_eq!(task.hierarchy.side_effect_approval, None);
        assert!(!task.hierarchy.research_accepted);
        assert_eq!(task.hierarchy.research_acceptance, None);
    }

    #[test]
    fn blocked_parent_rollup_preserves_planning_attention_with_backlog_children() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_BLOCKED.to_string();
        run.tasks[0].error = Some("Planning error: acceptance criteria conflict".to_string());
        let mut child = BoardTask::draft(
            &mut run,
            "Re-plan budget controls".to_string(),
            "Resolve the conflicting scope.".to_string(),
        );
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id);
        child.status = TASK_STATUS_BACKLOG.to_string();
        run.tasks.push(child);

        refresh_hierarchy_rollups(&mut run);

        assert_eq!(run.tasks[0].status, TASK_STATUS_BLOCKED);
        assert_eq!(
            run.tasks[0].error.as_deref(),
            Some("Planning error: acceptance criteria conflict")
        );
    }

    #[test]
    fn risky_or_optional_generated_children_stay_in_backlog() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let mut risky = BoardTask::draft(
            &mut run,
            "Run database migration".to_string(),
            "Apply the database migration.".to_string(),
        );
        risky.task_type = TASK_KIND_MIGRATION.to_string();
        risky.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        risky.hierarchy.executable = true;
        assert!(generated_child_requires_backlog_approval(&risky));

        let mut optional = BoardTask::draft(
            &mut run,
            "Add optional chart".to_string(),
            "Add a nice-to-have chart.".to_string(),
        );
        optional.hierarchy.required = false;
        assert!(generated_child_requires_backlog_approval(&optional));

        let ordinary = BoardTask::draft(
            &mut run,
            "Add budget repository".to_string(),
            "Add the repository method.".to_string(),
        );
        assert!(!generated_child_requires_backlog_approval(&ordinary));
    }

    #[test]
    fn wrapped_generated_children_satisfy_their_parent_breakdown_level() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent = run.tasks[0].clone();
        let parent_id = parent.id.clone();
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        run.tasks[0].hierarchy.level = TASK_LEVEL_STORY.to_string();
        run.tasks[0].hierarchy.executable = false;

        let mut wrapper = BoardTask::draft(
            &mut run,
            "Proposed scope: Run database migration".to_string(),
            "Review the generated migration scope.".to_string(),
        );
        wrapper.status = TASK_STATUS_BACKLOG.to_string();
        wrapper.task_origin = "hierarchy_backlog_wrapper".to_string();
        mark_generated_scope_wrapper(
            &mut wrapper,
            &parent,
            "run database migration",
            TASK_LEVEL_TASK,
        );
        run.tasks.push(wrapper);

        assert!(generated_hierarchy_wrapper_exists(
            &run,
            &parent_id,
            TASK_LEVEL_TASK
        ));
        assert!(next_hierarchy_parent(&run).is_none());
    }

    #[test]
    fn accepted_research_creates_only_backlog_planning_items() {
        let mut run = board_fixture(json!({
            "command": "Explore budget reporting",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = TASK_STATUS_TODO.to_string();
        let mut parent = BoardTask::draft(
            &mut run,
            "Explore reporting architecture".to_string(),
            "Plan the reporting architecture.".to_string(),
        );
        parent.status = TASK_STATUS_TODO.to_string();
        parent.hierarchy.level = TASK_LEVEL_TASK.to_string();
        parent.hierarchy.parent_id = Some(run.tasks[0].id.clone());
        parent.hierarchy.executable = false;
        let parent_id = parent.id.clone();
        run.tasks.push(parent);
        let mut research = BoardTask::draft(
            &mut run,
            "Compare reporting approaches".to_string(),
            "Compare viable reporting approaches.".to_string(),
        );
        research.status = TASK_STATUS_DONE.to_string();
        research.task_type = TASK_KIND_RESEARCH.to_string();
        research.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        research.hierarchy.parent_id = Some(parent_id);
        research.hierarchy.executable = true;
        research.result = Some(json!({
            "proposedPlanningItems": [{
                "level": "task",
                "kind": "implementation",
                "title": "Build reporting story",
                "details": "Create the approved reporting flow."
            }]
        }));
        let research_id = research.id.clone();
        run.tasks.push(research);

        accept_research_in_board(&mut run, &research_id, "user-1", None, None)
            .expect("research acceptance succeeds");

        let accepted = run
            .tasks
            .iter()
            .find(|task| task.id == research_id)
            .unwrap();
        assert!(accepted.hierarchy.research_accepted);
        let planning = run
            .tasks
            .iter()
            .find(|task| task.task_origin == "research_accepted")
            .expect("accepted planning item");
        assert_eq!(task_level(planning), TASK_LEVEL_STORY);
        assert_eq!(planning.status, TASK_STATUS_BACKLOG);
        assert!(!task_is_executable(planning));
    }

    #[test]
    fn done_item_linked_revision_never_reopens_source() {
        let mut run = board_fixture(json!({
            "command": "Ship budget controls",
            "projectPath": "/tmp/project"
        }));
        let source_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();

        apply_discussion_action(
            &mut run,
            &source_id,
            "revision",
            &json!({
                "title": "Revise budget controls",
                "details": "Support the revised budget behavior.",
                "kind": "revision"
            }),
        )
        .expect("linked revision should be created");

        let source = run.tasks.iter().find(|task| task.id == source_id).unwrap();
        assert_eq!(source.status, TASK_STATUS_DONE);
        let revision = run
            .tasks
            .iter()
            .find(|task| task.task_type == TASK_KIND_REVISION)
            .expect("revision item");
        assert_eq!(revision.status, TASK_STATUS_BACKLOG);
        assert_eq!(task_level(revision), TASK_LEVEL_STORY);
        assert!(!task_is_executable(revision));
        assert!(
            revision
                .references
                .iter()
                .any(|reference| reference.contains(&source_id))
        );
    }

    #[test]
    fn deleting_required_child_marks_parent_with_missing_plan_reason() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        let mut child = BoardTask::draft(
            &mut run,
            "Persist budget control".to_string(),
            "Persist the control.".to_string(),
        );
        child.hierarchy.level = TASK_LEVEL_TASK.to_string();
        child.hierarchy.parent_id = Some(parent_id.clone());
        child.hierarchy.required = true;
        let child_id = child.id.clone();
        run.tasks.push(child);
        run.tasks[0].status = TASK_STATUS_TODO.to_string();

        delete_board_task(&mut run, &child_id).expect("backlog child can be deleted");

        let parent = run.tasks.iter().find(|task| task.id == parent_id).unwrap();
        assert_eq!(parent.status, TASK_STATUS_BLOCKED);
        assert!(
            parent
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("Missing required plan")
        );
    }

    #[test]
    fn user_created_child_can_be_detached_as_a_backlog_story() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let parent_id = run.tasks[0].id.clone();
        let child = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Preserve custom budget rule",
                "parentId": parent_id,
                "status": "backlog"
            }))
            .unwrap(),
        )
        .expect("manual child");
        let child_id = child.id.clone();
        run.tasks.push(child);

        detach_user_created_child(&mut run, &child_id).expect("detach succeeds");

        let detached = run.tasks.iter().find(|task| task.id == child_id).unwrap();
        assert_eq!(task_level(detached), TASK_LEVEL_STORY);
        assert_eq!(detached.hierarchy.parent_id, None);
        assert_eq!(detached.status, TASK_STATUS_BACKLOG);
        assert_eq!(detached.task_origin, "user_manual");
    }

    #[test]
    fn detaching_user_child_preserves_descendant_hierarchy() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        let child = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Preserve custom budget rule",
                "parentId": root_id,
                "status": "backlog"
            }))
            .unwrap(),
        )
        .expect("manual child");
        let child_id = child.id.clone();
        run.tasks.push(child);

        let grandchild = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Keep custom rule implementation",
                "parentId": child_id,
                "status": "backlog"
            }))
            .unwrap(),
        )
        .expect("manual grandchild");
        let grandchild_id = grandchild.id.clone();
        run.tasks.push(grandchild);

        let great_grandchild = BoardTask::manual(
            &mut run,
            serde_json::from_value(json!({
                "title": "Verify custom rule",
                "parentId": grandchild_id,
                "level": "subtask",
                "status": "backlog"
            }))
            .unwrap(),
        )
        .expect("manual great-grandchild");
        let great_grandchild_id = great_grandchild.id.clone();
        run.tasks.push(great_grandchild);

        detach_user_created_child(&mut run, &child_id).expect("detach succeeds");

        let detached = run.tasks.iter().find(|task| task.id == child_id).unwrap();
        assert_eq!(task_level(detached), TASK_LEVEL_STORY);
        assert_eq!(detached.hierarchy.parent_id, None);
        assert!(!task_is_executable(detached));

        let task = run
            .tasks
            .iter()
            .find(|task| task.id == grandchild_id)
            .unwrap();
        assert_eq!(task_level(task), TASK_LEVEL_TASK);
        assert_eq!(task.hierarchy.parent_id.as_deref(), Some(child_id.as_str()));
        assert!(!task_is_executable(task));

        let subtask = run
            .tasks
            .iter()
            .find(|task| task.id == great_grandchild_id)
            .unwrap();
        assert_eq!(task_level(subtask), TASK_LEVEL_SUBTASK);
        assert_eq!(
            subtask.hierarchy.parent_id.as_deref(),
            Some(grandchild_id.as_str())
        );
        assert!(task_is_executable(subtask));
        assert!(hierarchy_validation_issues(&run).is_empty());
    }

    #[test]
    fn normalization_keeps_optional_work_runnable_under_completed_scope() {
        let mut run = board_fixture(json!({
            "command": "Implement budget controls",
            "projectPath": "/tmp/project"
        }));
        let root_id = run.tasks[0].id.clone();
        run.tasks[0].status = TASK_STATUS_DONE.to_string();

        let mut optional_task = BoardTask::draft(
            &mut run,
            "Optional budget polish".to_string(),
            "Keep the optional polish separate from the completed scope.".to_string(),
        );
        optional_task.status = TASK_STATUS_DONE.to_string();
        optional_task.hierarchy.level = TASK_LEVEL_TASK.to_string();
        optional_task.hierarchy.parent_id = Some(root_id.clone());
        optional_task.hierarchy.executable = false;
        optional_task.hierarchy.required = false;
        let optional_task_id = optional_task.id.clone();
        run.tasks.push(optional_task);

        let mut optional_subtask = BoardTask::draft(
            &mut run,
            "Run optional budget polish".to_string(),
            "Run the optional polish work.".to_string(),
        );
        optional_subtask.status = TASK_STATUS_TODO.to_string();
        optional_subtask.hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
        optional_subtask.hierarchy.parent_id = Some(optional_task_id);
        optional_subtask.hierarchy.executable = true;
        optional_subtask.hierarchy.required = false;
        let optional_subtask_id = optional_subtask.id.clone();
        run.tasks.push(optional_subtask);

        normalize_board_hierarchy(&mut run);

        let subtask = run
            .tasks
            .iter()
            .find(|task| task.id == optional_subtask_id)
            .unwrap();
        assert_eq!(subtask.status, TASK_STATUS_TODO);
        assert!(task_ancestors_are_approved(&run, subtask));
        assert!(task_is_runnable_in_board(&run, subtask));
    }

    #[test]
    fn project_execution_owner_allows_one_board_at_a_time() {
        let project_path = format!("/tmp/iowb-project-{}", Uuid::new_v4());
        assert!(claim_project_execution(&project_path, "board-a").unwrap());
        assert!(!claim_project_execution(&project_path, "board-a").unwrap());
        let error = claim_project_execution(&project_path, "board-b").unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        release_project_execution(&project_path, "board-a");
        assert!(claim_project_execution(&project_path, "board-b").unwrap());
        release_project_execution(&project_path, "board-b");
    }

    #[test]
    fn legacy_task_sequence_migrates_from_existing_ids() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].id = "task-3".to_string();
        run.next_task_sequence = 0;

        assert_eq!(allocate_task_id(&mut run), "task-4");
        assert_eq!(run.next_task_sequence, 4);
    }

    #[test]
    fn prompt_drafts_allocate_unique_ids_after_gaps() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.tasks.push(BoardTask::draft(
            &mut run.clone(),
            "Third task".to_string(),
            "Third task".to_string(),
        ));
        run.tasks[1].id = "task-3".to_string();
        run.next_task_sequence = 0;

        let drafts = prompt_to_task_drafts(&mut run, "One\nTwo\nThree");
        let ids = drafts
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["task-4", "task-5", "task-6"]);
    }

    #[test]
    fn task_dependencies_keep_missing_cards_as_blockers() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let mut dependent =
            BoardTask::draft(&mut run, "Dependent".to_string(), "Dependent".to_string());
        dependent.depends_on = vec![
            dependent.id.clone(),
            "missing-task".to_string(),
            "task-1".to_string(),
        ];
        run.tasks.push(dependent.clone());

        assert_eq!(
            unmet_task_dependencies(&run, &dependent),
            vec!["missing-task", "task-1"]
        );

        run.tasks[0].status = TASK_STATUS_DONE.to_string();
        assert_eq!(
            unmet_task_dependencies(&run, &dependent),
            vec!["missing-task"]
        );
    }

    #[test]
    fn generated_placeholder_card_drops_self_dependency() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let placeholder_id = "task-2".to_string();
        let mut task = prompt_task_from_draft(
            &mut run,
            json!({
                "title": "Generated task",
                "details": "Generated task details",
                "dependsOn": [placeholder_id],
            }),
            "Generate a task",
        );
        task.id = placeholder_id;
        let mut generated = vec![task];
        sanitize_generated_task_dependencies(&run, &mut generated, "task-2");

        assert!(generated[0].depends_on.is_empty());
    }

    #[test]
    fn prompt_draft_context_excludes_generation_placeholders() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.backlog_breakdown = json!({
            "id": "breakdown-1",
            "status": "running",
            "prompt": "Generate a task",
            "model": "gpt-5.6-sol",
            "boardProfile": "minimal"
        });

        let prompt = build_prompt_task_draft_prompt(&run, "Generate a task", "minimal");

        assert!(!prompt.contains("breakdown-1 ["));
        assert!(prompt.contains("task-1 [backlog] Initial task"));
    }

    #[test]
    fn deleting_parent_deletes_descendants_completely() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let mut dependent =
            BoardTask::draft(&mut run, "Dependent".to_string(), "Dependent".to_string());
        dependent.depends_on = vec!["task-1".to_string(), "external".to_string()];
        dependent.source_task_id = Some("task-1".to_string());
        dependent.source_qa_task_id = Some("task-1".to_string());
        run.tasks.push(dependent);

        delete_board_task(&mut run, "task-1").unwrap();

        assert!(run.tasks.is_empty());
    }

    #[test]
    fn deleting_current_task_is_rejected() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.current_task_id = Some("task-1".to_string());

        let error = delete_board_task(&mut run, "task-1").unwrap_err();

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(run.tasks.len(), 1);
    }

    #[test]
    fn invalid_create_schedule_is_rejected() {
        let request = serde_json::from_value::<CreateBoardRequest>(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project",
            "scheduledStartAt": "not-a-timestamp"
        }))
        .unwrap();

        let error = AgenticBoard::new(None, request).unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.body.error.contains("valid RFC3339"));
    }

    #[test]
    fn new_board_without_schedule_starts_as_backlog_planning_board() {
        let run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));

        assert_eq!(run.status, "paused");
        assert!(!run.active);
        assert!(!run.loop_started);
        assert!(!run.auto_run_enabled);
        assert_eq!(run.scheduled_start_at, None);
        assert_eq!(
            run.pause_reason.as_deref(),
            Some("Board created with backlog planning item.")
        );
        assert_eq!(run.provider_call_started_at, None);
        assert_eq!(run.current_provider_session_id, None);
        assert_eq!(run.tasks.len(), 1);
        assert_eq!(run.tasks[0].status, TASK_STATUS_BACKLOG);
    }

    #[test]
    fn board_strategy_enables_gpt_5_6_sol_fast_mode() {
        let run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project",
            "provider": "codex",
            "model": "gpt-5.6-sol",
            "modelStrategy": {
                "reasoningEffort": "low",
                "serviceTier": "fast"
            }
        }));

        assert_eq!(
            board_provider_controls(&run),
            BoardProviderControls {
                effort: Some("low".to_string()),
                thinking: None,
                fast: Some(true),
            }
        );
    }

    #[test]
    fn prompt_task_generation_records_model_fast_mode_and_usage() {
        let mut run = board_fixture(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project",
            "provider": "codex",
            "model": "gpt-5.6-sol",
            "modelStrategy": {
                "reasoningEffort": "low",
                "serviceTier": "fast"
            }
        }));
        let attempt = PromptTaskDraftAttempt {
            result: Ok((vec![json!({ "title": "Generated task" })], None)),
            provider_prompt: "Generate one focused task".to_string(),
            provider_output: r#"{"tasks":[{"title":"Generated task"}]}"#.to_string(),
            session_id: Some("session-1".to_string()),
            token_usage: Some(json!({
                "inputTokens": 12,
                "cachedInputTokens": 2,
                "outputTokens": 8,
                "totalTokens": 20,
            })),
            effective_provider: "codex".to_string(),
            effective_model: "gpt-5.6-sol".to_string(),
            started_at: Utc::now(),
        };

        record_prompt_task_generation_attempt(
            &mut run,
            "Kanban backlog prompt generation",
            &attempt,
        );

        let telemetry = run.prompt_telemetry.last().unwrap();
        assert_eq!(telemetry["provider"], "codex");
        assert_eq!(telemetry["effectiveModel"], "gpt-5.6-sol");
        assert_eq!(telemetry["reasoningEffort"], "low");
        assert_eq!(telemetry["fast"], true);
        assert_eq!(telemetry["outcome"], "completed");
        assert_eq!(telemetry["actualTotalTokens"], 20);
        assert_eq!(run.provider_usage["totalTokens"], 20);
        assert_eq!(run.provider_usage["invocationsWithUsage"], 1);
        assert_eq!(run.last_effective_model.as_deref(), Some("gpt-5.6-sol"));
    }

    #[test]
    fn prompt_task_generation_transcript_preserves_prompt_and_output() {
        let attempt = PromptTaskDraftAttempt {
            result: Ok((vec![json!({ "title": "Generated task" })], None)),
            provider_prompt: "Generate one focused task".to_string(),
            provider_output: "Created two backlog cards.".to_string(),
            session_id: Some("session-1".to_string()),
            token_usage: Some(json!({
                "inputTokens": 12,
                "outputTokens": 8,
                "totalTokens": 20,
            })),
            effective_provider: "codex".to_string(),
            effective_model: "gpt-5.6-sol".to_string(),
            started_at: Utc::now(),
        };

        let transcript = prompt_task_generation_transcript(
            &attempt,
            "Add mobile transcript visibility",
            true,
            None,
        );
        let entries = transcript.as_array().unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["role"], "user");
        assert_eq!(entries[0]["content"], "Add mobile transcript visibility");
        assert_eq!(entries[1]["role"], "assistant");
        assert_eq!(entries[1]["content"], "Created two backlog cards.");
        assert_eq!(entries[1]["provider"], "codex");
        assert_eq!(entries[1]["model"], "gpt-5.6-sol");
        assert_eq!(entries[1]["sessionId"], "session-1");
        assert_eq!(entries[1]["tokenUsage"]["totalTokens"], 20);
    }

    #[test]
    fn clearing_schedule_returns_scheduled_board_to_paused_state() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "scheduledStartAt": "2099-08-09T01:00:00Z"
        }));
        run.status = "scheduled".to_string();
        run.active = false;
        run.loop_started = false;
        run.auto_run_enabled = true;

        clear_board_schedule(&mut run);

        assert_eq!(run.status, "paused");
        assert_eq!(run.scheduled_start_at, None);
        assert!(!run.auto_run_enabled);
        assert_eq!(run.pause_reason.as_deref(), Some("schedule cleared"));
    }

    #[test]
    fn future_scheduled_board_does_not_arm_auto_retry() {
        let mut run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "scheduledStartAt": "2099-08-09T01:00:00Z",
            "autoRetry": {
                "enabled": true,
                "delayMinutes": 10,
                "maxAttempts": 3
            }
        }));

        assert_eq!(run.status, "scheduled");
        assert!(!is_resumable_board(&run));
        assert!(!schedule_auto_retry_if_eligible(
            &mut run,
            "resumable status"
        ));
        assert_eq!(run.auto_retry["nextRetryAt"], Value::Null);
    }

    #[test]
    fn board_detail_exposes_every_mobile_evidence_collection() {
        let run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let detail = run.detail_json(None);
        for key in [
            "logs",
            "tasks",
            "sourceManifest",
            "sourceReferences",
            "sourceChunks",
            "codebaseManifest",
            "codebaseMap",
            "codebaseChunks",
            "codebaseUnderstanding",
            "agentsContext",
            "workspaceBaseline",
            "latestWorkspaceSnapshot",
            "environmentState",
            "promptTelemetry",
            "providerUsage",
            "providerUsageBySession",
            "compactionLedger",
            "changeLedger",
            "gitLedger",
            "validationRuns",
            "ragQueries",
            "ragIngestions",
            "ragTraceRefs",
            "tddPolicy",
            "qaArtifacts",
            "promotionCandidates",
            "finalReview",
        ] {
            assert!(detail.get(key).is_some(), "missing evidence key {key}");
        }
    }

    #[test]
    fn configured_fallback_prefers_explicit_next_provider_and_model() {
        let run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "provider": "claude",
            "model": "sonnet",
            "nextProvider": "codex",
            "nextModel": "gpt-5",
            "modelStrategy": {
                "fallbackProvider": "gemini",
                "fallbackModel": "gemini-2.5-pro"
            }
        }));

        assert_eq!(
            configured_provider_fallback(&run),
            Some(("codex".to_string(), "gpt-5".to_string()))
        );
    }

    #[test]
    fn configured_fallback_uses_strategy_and_ignores_same_runtime() {
        let strategy_run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "provider": "claude",
            "model": "sonnet",
            "modelStrategy": {
                "fallbackProvider": "gemini",
                "fallbackModel": "gemini-2.5-pro"
            }
        }));
        let same_run = board_fixture(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "provider": "claude",
            "model": "sonnet",
            "nextProvider": "claude",
            "nextModel": "sonnet"
        }));

        assert_eq!(
            configured_provider_fallback(&strategy_run),
            Some(("gemini".to_string(), "gemini-2.5-pro".to_string()))
        );
        assert_eq!(configured_provider_fallback(&same_run), None);
    }

    #[test]
    fn fallback_is_required_for_transport_and_process_failures() {
        let successful = ProviderTaskResult {
            summary: "done".to_string(),
            stderr: String::new(),
            assistant_text: "done".to_string(),
            stream_events: Vec::new(),
            errors: Vec::new(),
            session_id: None,
            token_usage: None,
            exit_code: 0,
        };
        let failed = ProviderTaskResult {
            summary: "provider failed".to_string(),
            stderr: "provider failed".to_string(),
            assistant_text: String::new(),
            stream_events: Vec::new(),
            exit_code: 1,
            errors: vec!["provider failed".to_string()],
            session_id: None,
            token_usage: None,
        };

        assert!(!provider_result_requires_fallback(&Ok(successful)));
        assert!(provider_result_requires_fallback(&Ok(failed)));
        assert!(provider_result_requires_fallback(&Err(bad_request(
            "offline"
        ))));
    }

    #[test]
    fn malformed_tool_call_repair_detects_integer_like_float_schema_errors() {
        assert!(is_repairable_integer_tool_arg_schema_error(
            "failed to parse function arguments: invalid type: floating point `59077.0`, expected i32 at line 1 column 22"
        ));
        assert!(is_repairable_integer_tool_arg_schema_error(
            "failed to parse function arguments: invalid type: floating point `180000.0`, expected u64 at line 1 column 243"
        ));
        assert!(is_repairable_integer_tool_arg_schema_error(
            "failed to parse function arguments: invalid type: floating point `15000.0`, expected usize at line 1 column 29"
        ));
        assert!(!is_repairable_integer_tool_arg_schema_error(
            "failed to parse function arguments: invalid type: floating point `1.5`, expected u64 at line 1 column 20"
        ));
        assert!(!is_repairable_integer_tool_arg_schema_error(
            "failed to parse function arguments: invalid type: string `59077`, expected i32 at line 1 column 22"
        ));
    }

    #[test]
    fn provider_result_detects_malformed_integer_tool_args_in_events() {
        let failed = ProviderTaskResult {
            summary: "provider failed".to_string(),
            stderr: String::new(),
            assistant_text: String::new(),
            stream_events: vec![json!({
                "kind": "tool_result",
                "content": "failed to parse function arguments: invalid type: floating point `60000.0`, expected u64 at line 1 column 43",
            })],
            exit_code: 1,
            errors: Vec::new(),
            session_id: Some("board-chat".to_string()),
            token_usage: None,
        };

        assert!(provider_result_has_repairable_integer_tool_arg_schema_error(&Ok(failed)));
    }

    #[test]
    fn malformed_tool_call_repair_requires_a_failing_provider_result() {
        let recovered = ProviderTaskResult {
            summary: "done".to_string(),
            stderr: String::new(),
            assistant_text: r#"{"status":"done","summary":"Recovered after failed to parse function arguments: invalid type: floating point `60000.0`, expected u64."}"#.to_string(),
            stream_events: Vec::new(),
            errors: Vec::new(),
            session_id: Some("board-chat".to_string()),
            token_usage: None,
            exit_code: 0,
        };

        assert!(provider_result_has_repairable_integer_tool_arg_schema_error(&Ok(recovered)));
        assert!(!provider_result_should_attempt_malformed_tool_call_repair(&Ok(
            ProviderTaskResult {
                summary: "done".to_string(),
                stderr: String::new(),
                assistant_text: r#"{"status":"done","summary":"Recovered after failed to parse function arguments: invalid type: floating point `60000.0`, expected u64."}"#.to_string(),
                stream_events: Vec::new(),
                errors: Vec::new(),
                session_id: Some("board-chat".to_string()),
                token_usage: None,
                exit_code: 0,
            }
        )));
    }

    #[test]
    fn qa_policy_normalizes_malformed_tool_call_repair_settings() {
        let defaults = normalize_qa_policy(None);
        assert_eq!(
            defaults
                .get("repairMalformedToolCalls")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            defaults
                .get("malformedToolCallRepairRetries")
                .and_then(Value::as_u64),
            Some(1)
        );

        let disabled = normalize_qa_policy(Some(&json!({
            "repairMalformedToolCalls": false,
            "malformedToolCallRepairRetries": 99,
        })));
        assert_eq!(
            disabled
                .get("repairMalformedToolCalls")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            disabled
                .get("malformedToolCallRepairRetries")
                .and_then(Value::as_u64),
            Some(MAX_MALFORMED_TOOL_CALL_REPAIR_RETRIES)
        );
    }

    #[test]
    fn legacy_tcd_qa_prompt_maps_to_exact_board_task() {
        let mut run = board_fixture(json!({
            "command": "Implement all written in docs folder",
            "projectPath": "/tmp/TCD-Meida-new"
        }));
        run.id = "28c3b53f-e616-43d9-b4dc-8353fdac7249".to_string();
        run.tasks = ["feature-repair-1", "feature-repair-1-2"]
            .into_iter()
            .map(|id| {
                let mut task =
                    BoardTask::draft(&mut run, "User request".to_string(), id.to_string());
                task.id = id.to_string();
                task
            })
            .collect();
        let prompt = r#"You are the QA phase of a TDD-first io-workbench Kanban board worker.

Project: TCD-Meida-new
Board id: 28c3b53f-e616-43d9-b4dc-8353fdac7249
Task 2: feature-repair-1-2
Title: User request"#;

        assert!(legacy_board_prompt_signature(prompt));
        assert_eq!(
            legacy_board_task_id(&run, prompt).as_deref(),
            Some("feature-repair-1-2")
        );
    }

    #[test]
    fn legacy_internal_prompt_requires_nearby_board_telemetry() {
        let mut run = board_fixture(json!({
            "command": "Implement all written in docs folder",
            "projectPath": "/tmp/TCD-Meida-new"
        }));
        let started_at = Utc::now();
        run.prompt_telemetry = vec![json!({
            "label": "codebase recon",
            "startedAt": started_at,
        })];
        let prompt = "You are performing read-only codebase reconnaissance before Kanban planning.";

        assert!(legacy_board_prompt_signature(prompt));
        assert!(legacy_prompt_matches_board_telemetry(
            &run,
            started_at + chrono::Duration::seconds(20)
        ));
        assert!(!legacy_prompt_matches_board_telemetry(
            &run,
            started_at + chrono::Duration::minutes(2)
        ));
        assert_eq!(legacy_board_task_id(&run, prompt), None);
    }
}

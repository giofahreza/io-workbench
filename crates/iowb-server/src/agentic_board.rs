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

// Keep the board implementation in one private namespace while separating
// the domains below into files that are easier to scan and review.
include!("agentic_board/types.rs");
include!("agentic_board/http.rs");
include!("agentic_board/discussion/mod.rs");
include!("agentic_board/execution.rs");
include!("agentic_board/hierarchy/mod.rs");
include!("agentic_board/orchestration/mod.rs");
include!("agentic_board/execution_support.rs");
include!("agentic_board/provider_runtime.rs");
include!("agentic_board/task_execution.rs");
include!("agentic_board/validation_and_settings.rs");
include!("agentic_board/workspace_and_routing.rs");
include!("agentic_board/persistence_and_prompt_generation.rs");
include!("agentic_board/normalization.rs");

#[cfg(test)]
include!("agentic_board/tests.rs");

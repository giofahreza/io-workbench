use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Mutex, MutexGuard,
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

const BOARD_RUNS_DIR: &str = "agentic-runs";
const DEFAULT_PROVIDER: &str = "claude";
const DEFAULT_MODEL: &str = "";
const PROVIDER_POLL_INTERVAL: Duration = Duration::from_millis(750);
const SOURCE_CHUNK_TARGET_LENGTH: usize = 12_000;
const SOURCE_CHUNK_MAX_LENGTH: usize = 18_000;
const PROMOTION_REVIEW_TASK_ID: &str = "promotion-review";
const CODEBASE_CHUNK_TARGET_LENGTH: usize = 10_000;
const MAX_SOURCE_FILES: usize = 160;
const MAX_CODEBASE_FILES: usize = 2_500;
const MAX_CODEBASE_CHUNKS: usize = 600;
const MAX_PROVIDER_OUTPUT_CHARS: usize = 160_000;
const AUTO_RETRY_POLL_INTERVAL: Duration = Duration::from_secs(30);
const DETERMINISTIC_VALIDATION_TIMEOUT: Duration = Duration::from_secs(120);
const MANAGED_GIT_TIMEOUT: Duration = Duration::from_secs(30);
const MANAGED_GIT_PUSH_TIMEOUT: Duration = Duration::from_secs(180);
const FINAL_QA_TASK_ID: &str = "final-qa";
const AGENTS_KNOWLEDGE_TASK_ID: &str = "agents-knowledge";
const MAX_FOLLOWUP_TASKS_PER_GROUP: usize = 3;
const MAX_TASK_ATTEMPTS: u32 = 2;
const DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL: &str = "http://141.144.197.96:8319/claude";
const IO_GATEWAY_API_KEY_CREDENTIAL: &str = "io-workbench-io-gateway-api-key";
const IO_GATEWAY_API_KEY_CREDENTIAL_TYPE: &str = "io_gateway_api_key";
const CURSOR_CLI_COMMAND: &str = "cursor-agent";
static AUTO_RETRY_POLLER_STARTED: AtomicBool = AtomicBool::new(false);
static BOARD_RUN_MUTATION_LOCK: Mutex<()> = Mutex::new(());
static BOARD_RUN_SAVE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/danger", post(create_run))
        .route("/api/danger/", post(create_run))
        .route("/api/danger/run", post(create_run))
        .route("/api/danger/start", post(create_run))
        .route(
            "/api/danger/runs",
            get(list_runs)
                .post(create_run)
                .delete(delete_runs_for_project),
        )
        .route("/api/danger/runs/{id}", get(get_run))
        .route("/api/danger/runs/{id}/pause", post(pause_run))
        .route("/api/danger/runs/{id}/resume", post(resume_run))
        .route("/api/danger/runs/{id}/schedule", post(schedule_run))
        .route("/api/danger/runs/{id}/abort", post(abort_run))
        .route("/api/danger/runs/{id}/model", patch(update_model))
        .route(
            "/api/danger/runs/{id}/model-strategy",
            patch(update_model_strategy),
        )
        .route("/api/danger/runs/{id}/git-policy", patch(update_git_policy))
        .route("/api/danger/runs/{id}/tools", patch(update_tools_settings))
        .route(
            "/api/danger/runs/{id}/task-models",
            patch(update_task_models),
        )
        .route("/api/danger/runs/{id}/auto-retry", patch(update_auto_retry))
        .route("/api/danger/runs/{id}/tasks", post(add_task))
        .route("/api/danger/runs/{id}/tasks/draft", post(draft_tasks))
        .route(
            "/api/danger/runs/{id}/tasks/backlog-from-prompt",
            post(backlog_from_prompt),
        )
        .route(
            "/api/danger/runs/{id}/tasks/retry-attention",
            post(retry_attention_tasks),
        )
        .route(
            "/api/danger/runs/{id}/tasks/retry-backlog-failed",
            post(retry_backlog_failed_tasks),
        )
        .route(
            "/api/danger/runs/{id}/tasks/{task_id}/promote",
            post(promote_task),
        )
        .route(
            "/api/danger/runs/{id}/tasks/{task_id}/demote",
            post(demote_task),
        )
        .route(
            "/api/danger/runs/{id}/tasks/{task_id}",
            patch(update_task).delete(delete_task),
        )
}

pub(crate) fn recover_active_runs(state: &AppState) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    start_auto_retry_poller(handle.clone(), state.clone());
    let Ok(runs) = load_runs(state) else {
        return;
    };
    for stored in runs {
        if !matches!(stored.run.status.as_str(), "running")
            && !stored.run.active
            && !stored.run.loop_started
        {
            continue;
        }
        let Some(user_id) = stored.run.user_id.clone() else {
            continue;
        };
        let run_id = stored.run.id.clone();
        let state = state.clone();
        handle.spawn(async move {
            match load_user_run(&state, &user_id, &run_id) {
                Ok(mut stored) => {
                    stored.run.status = "running".to_string();
                    stored.run.active = true;
                    stored.run.loop_started = true;
                    stored.run.pause_requested = false;
                    stored.run.current_task_id = None;
                    stored.run.current_task_title.clear();
                    stored.run.current_task_status.clear();
                    stored
                        .run
                        .append_log("Recovered active agentic board run after server restart");
                    stored.run.touch();
                    if let Err(error) = save_run(&state, &stored.run) {
                        tracing::warn!(error = %server_error_message(&error), "failed to persist recovered agentic board run");
                        return;
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %server_error_message(&error), "failed to load active agentic board run for recovery");
                    return;
                }
            }
            if let Err(error) = run_board_loop(state, user_id, run_id).await {
                tracing::warn!(error = %server_error_message(&error), "recovered agentic board runner failed");
            }
        });
    }
}

/// Classify sessions written by older board versions before any ordinary
/// project/session discovery or chat-run recovery can expose them. The lazy
/// list/detail repair remains in place for snapshots copied in while the
/// server is already running.
pub(crate) async fn backfill_legacy_board_sessions(state: &AppState) -> Result<()> {
    for mut stored in load_runs(state)? {
        backfill_board_session_links(state, &mut stored.run).await?;
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
    for mut stored in load_runs(state)? {
        if stored.run.status != "scheduled" {
            continue;
        }
        let Some(scheduled_start_at) = stored.run.scheduled_start_at else {
            stored.run.status = "paused".to_string();
            stored.run.paused_at = Some(now);
            stored.run.pause_reason = Some("Scheduled start time was invalid".to_string());
            stored
                .run
                .append_log("Scheduled start time was invalid; run paused");
            stored.run.touch();
            save_run(state, &stored.run)?;
            continue;
        };
        if scheduled_start_at > now {
            continue;
        }
        let Some(user_id) = stored.run.user_id.clone() else {
            continue;
        };
        stored.run.scheduled_start_at = None;
        stored.run.append_log("Scheduled start time reached");
        stored.run.touch();
        save_run(state, &stored.run)?;
        let _ = begin_run(state, &user_id, &stored.run.id)?;
    }
    Ok(())
}

async fn process_auto_retries(state: &AppState) -> Result<()> {
    let now = Utc::now();
    for mut stored in load_runs(state)? {
        if stored.run.loop_started || !auto_retry_enabled(&stored.run.auto_retry) {
            continue;
        }
        if !is_resumable_run(&stored.run) {
            continue;
        }
        let retry_state = normalize_auto_retry(&stored.run.auto_retry);
        let attempts = retry_state
            .get("attempts")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let max_attempts = retry_state
            .get("maxAttempts")
            .and_then(Value::as_u64)
            .unwrap_or(3);
        if attempts >= max_attempts {
            stored.run.auto_retry = merge_auto_retry(
                retry_state,
                json!({
                    "nextRetryAt": null,
                    "lastError": format!("Max auto retries reached ({attempts}/{max_attempts})"),
                    "updatedAt": now,
                }),
            );
            stored
                .run
                .append_log("Auto retry stopped: max attempts reached");
            stored.run.touch();
            save_run(state, &stored.run)?;
            continue;
        }
        let next_retry_at = retry_state
            .get("nextRetryAt")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_utc);
        if next_retry_at.is_none() {
            schedule_auto_retry_if_eligible(&mut stored.run, "resumable status");
            stored.run.touch();
            save_run(state, &stored.run)?;
            continue;
        }
        if next_retry_at.is_some_and(|time| time > now) {
            continue;
        }
        let Some(user_id) = stored.run.user_id.clone() else {
            continue;
        };
        let retry_state = normalize_auto_retry(&stored.run.auto_retry);
        stored.run.auto_retry = merge_auto_retry(
            retry_state,
            json!({
                "attempts": attempts + 1,
                "nextRetryAt": null,
                "lastRetryAt": now,
                "lastError": "",
                "updatedAt": now,
            }),
        );
        stored.run.append_log(format!(
            "Auto retry {}/{} starting",
            attempts + 1,
            max_attempts
        ));
        reset_attention_tasks_for_retry(&mut stored.run);
        stored.run.touch();
        save_run(state, &stored.run)?;
        let _ = begin_run(state, &user_id, &stored.run.id)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunsQuery {
    project_path: Option<String>,
    history: Option<String>,
    include_history: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRunRequest {
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
    run_profile: Option<String>,
    task_model_overrides: Option<Value>,
    session_policy: Option<String>,
    git_policy: Option<String>,
    tools_settings: Option<Value>,
    force_new_run: Option<bool>,
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
    acceptance_criteria: Option<Value>,
    acceptance: Option<Value>,
    criteria: Option<Value>,
    references: Option<Value>,
    files: Option<Value>,
    paths: Option<Value>,
    requirement_ids: Option<Value>,
    requirements: Option<Value>,
    priority: Option<String>,
    depends_on: Option<Value>,
    dependencies: Option<Value>,
    status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptRequest {
    prompt: Option<String>,
    model: Option<String>,
    run_profile: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TaskIdsRequest {
    #[serde(rename = "taskIds")]
    task_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct UpdateTaskRequest {
    status: Option<String>,
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
struct BoardRun {
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
    #[serde(default = "default_run_profile")]
    run_profile: String,
    #[serde(default)]
    task_model_overrides: Value,
    #[serde(default)]
    session_policy: String,
    #[serde(default = "default_git_policy")]
    git_policy: String,
    #[serde(default)]
    tools_settings: Option<Value>,
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
    final_matrix_qa_complete: bool,
    #[serde(default)]
    auto_retry: Value,
    #[serde(default)]
    logs: Vec<String>,
    #[serde(default)]
    next_task_sequence: u64,
    #[serde(default)]
    tasks: Vec<BoardTask>,
    #[serde(default)]
    requirement_matrix: Vec<Value>,
    #[serde(default)]
    requirement_baseline: Vec<Value>,
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
    matrix_gap_review_complete: bool,
    #[serde(default)]
    agents_knowledge_updated: bool,
    #[serde(default)]
    v2_coverage_fallback_added: bool,
    #[serde(default)]
    final_review: Option<Value>,
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
    #[serde(default)]
    requirement_ids: Vec<String>,
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
}

impl BoardRun {
    fn new(user_id: Option<String>, request: CreateRunRequest) -> Result<Self> {
        let prompt = trim_string(request.command.or(request.prompt))
            .ok_or_else(|| bad_request("Prompt is required"))?;
        let project_path = trim_string(request.project_path)
            .ok_or_else(|| bad_request("Project path is required"))?;
        let provider = normalize_provider(request.provider.as_deref())?;
        let model = trim_string(request.model).unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let now = Utc::now();
        let scheduled_start_at =
            parse_optional_scheduled_start(request.scheduled_start_at.as_deref())?;
        let should_schedule = scheduled_start_at.is_some_and(|time| time > now);
        let mut run = Self {
            id: Uuid::new_v4().to_string(),
            orchestration_version: 2,
            user_id,
            provider,
            model: model.clone(),
            primary_model: model,
            next_model: trim_string(request.next_model).unwrap_or_default(),
            next_provider: normalize_optional_provider(request.next_provider.as_deref())?,
            last_effective_model: None,
            model_history: Vec::new(),
            model_strategy: request.model_strategy,
            run_profile: normalize_run_profile(request.run_profile.as_deref()),
            task_model_overrides: request.task_model_overrides.unwrap_or_else(|| json!({})),
            session_policy: normalize_session_policy(request.session_policy.as_deref()),
            git_policy: normalize_git_policy(request.git_policy.as_deref()),
            tools_settings: request.tools_settings,
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
            auto_run_enabled: !should_schedule,
            control_revision: 0,
            pause_requested: false,
            paused_at: if should_schedule { None } else { Some(now) },
            pause_reason: if should_schedule {
                None
            } else {
                Some("Board created; execution will start automatically.".to_string())
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
            final_matrix_qa_complete: false,
            auto_retry: json!({
                "enabled": false,
                "delayMinutes": 10,
                "maxAttempts": 3,
                "attempts": 0,
                "nextRetryAt": null,
                "lastRetryAt": null,
                "lastError": "",
                "updatedAt": now,
            }),
            logs: vec!["Created agentic board".to_string()],
            next_task_sequence: 0,
            tasks: Vec::new(),
            requirement_matrix: Vec::new(),
            requirement_baseline: Vec::new(),
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
            provider_usage: default_provider_usage(),
            provider_usage_by_session: json!({}),
            compaction_ledger: Vec::new(),
            change_ledger: Vec::new(),
            git_ledger: Vec::new(),
            validation_runs: Vec::new(),
            rag_enabled: RagClient::is_configured(),
            rag_service_url: RagClient::configured_descriptor(),
            rag_queries: Vec::new(),
            rag_ingestions: Vec::new(),
            rag_trace_refs: Vec::new(),
            tdd_enabled: default_tdd_enabled(),
            tdd_policy: default_tdd_policy(),
            qa_artifacts: Vec::new(),
            promotion_candidates: Vec::new(),
            planning_round: 0,
            review_round: 0,
            bootstrap_complete: false,
            matrix_gap_review_complete: false,
            agents_knowledge_updated: false,
            v2_coverage_fallback_added: false,
            final_review: None,
        };
        let task = BoardTask::manual(
            &mut run,
            TaskRequest {
                prompt: Some(prompt),
                command: None,
                title: request.title,
                details: request.details,
                description: request.description,
                acceptance_criteria: None,
                acceptance: None,
                criteria: None,
                references: None,
                files: None,
                paths: None,
                requirement_ids: None,
                requirements: None,
                priority: None,
                depends_on: None,
                dependencies: None,
                status: Some("pending".to_string()),
            },
        )?;
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
        json!({
            "id": self.id,
            "orchestrationVersion": self.orchestration_version,
            "provider": self.provider,
            "model": self.model,
            "primaryModel": self.primary_model,
            "nextModel": self.next_model,
            "nextProvider": self.next_provider,
            "lastEffectiveModel": self.last_effective_model,
            "modelStrategy": self.model_strategy,
            "runProfile": self.run_profile,
            "sessionPolicy": self.session_policy,
            "gitPolicy": self.git_policy,
            "modelHistory": self.model_history,
            "taskModelOverrides": self.task_model_overrides,
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
            "requirementCounts": count_statuses(self.requirement_matrix.iter().filter_map(|value| value.get("status").and_then(Value::as_str))),
            "sourceFileCount": self.source_manifest.len(),
            "sourceChunkCount": self.source_chunks.len(),
            "codebaseFileCount": self.codebase_manifest.len(),
            "codebaseChunkCount": self.codebase_chunks.len(),
            "codebaseUnderstandingCount": self.codebase_understanding.len(),
            "finalMatrixQaComplete": self.final_matrix_qa_complete,
            "matrixGapReviewComplete": self.matrix_gap_review_complete,
            "v2CoverageFallbackAdded": self.v2_coverage_fallback_added,
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
            "resumable": is_resumable_run(self),
            "toolsSettings": self.tools_settings,
            "autoRetry": self.auto_retry,
            "filePath": file_path,
        })
    }

    fn detail_json(&self, file_path: Option<String>) -> Value {
        let mut value = self.summary_json(file_path);
        if let Some(object) = value.as_object_mut() {
            object.insert("logs".to_string(), json!(self.logs));
            object.insert("tasks".to_string(), json!(self.tasks));
            object.insert(
                "requirementMatrix".to_string(),
                json!(self.requirement_matrix),
            );
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
        value
    }
}

impl BoardTask {
    fn manual(run: &mut BoardRun, request: TaskRequest) -> Result<Self> {
        let prompt = trim_string(request.prompt.or(request.command)).unwrap_or_default();
        let title = trim_string(request.title)
            .or_else(|| title_from_prompt(&prompt))
            .ok_or_else(|| bad_request("Manual task title or prompt is required."))?;
        let details =
            trim_string(request.details.or(request.description)).unwrap_or_else(|| prompt.clone());
        let status = normalize_task_status(request.status.as_deref(), "backlog")?;
        let references = [request.references, request.files, request.paths]
            .into_iter()
            .flat_map(value_to_strings)
            .collect();
        Ok(Self {
            id: allocate_task_id(run),
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
            requirement_ids: value_to_strings(request.requirement_ids.or(request.requirements)),
            priority: trim_string(request.priority).unwrap_or_else(|| "medium".to_string()),
            depends_on: value_to_strings(request.depends_on.or(request.dependencies)),
            manual_task: true,
            prompt_task: false,
            task_origin: "user_manual".to_string(),
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
            group_id: None,
        })
    }

    fn draft(run: &mut BoardRun, title: String, details: String) -> Self {
        Self {
            id: allocate_task_id(run),
            title,
            status: "backlog".to_string(),
            summary: String::new(),
            description: details.clone(),
            details,
            prompt: String::new(),
            error: None,
            acceptance_criteria: vec!["Complete the task described by this card.".to_string()],
            references: Vec::new(),
            requirement_ids: Vec::new(),
            priority: "medium".to_string(),
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
            group_id: None,
        }
    }
}

async fn create_run(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(mut request): Json<CreateRunRequest>,
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

    let scheduled_start_at = parse_optional_scheduled_start(request.scheduled_start_at.as_deref())?;
    let should_schedule = scheduled_start_at.is_some_and(|time| time > Utc::now());
    let (run_id, reused) = {
        let _guard = board_run_mutation_lock();
        let mut reused_run_id = None;
        if request.force_new_run != Some(true)
            && let Some(project_path) = trim_string(request.project_path.clone())
            && let Some(mut latest) = latest_run_for_project(&state, &user.0.id, &project_path)?
        {
            if should_schedule
                && (latest.run.loop_started || latest.run.active || latest.run.status == "running")
            {
                return Err(ServerError::new(
                    StatusCode::CONFLICT,
                    "Pause the active agentic board before scheduling a future start.",
                ));
            }
            apply_run_options(&mut latest.run, &request)?;
            let mut task = BoardTask::manual(
                &mut latest.run,
                TaskRequest {
                    prompt: request.command.clone().or(request.prompt.clone()),
                    command: None,
                    title: request.title.clone(),
                    details: request.details.clone(),
                    description: request.description.clone(),
                    acceptance_criteria: None,
                    acceptance: None,
                    criteria: None,
                    references: None,
                    files: None,
                    paths: None,
                    requirement_ids: None,
                    requirements: None,
                    priority: None,
                    depends_on: None,
                    dependencies: None,
                    status: Some("pending".to_string()),
                },
            )?;
            ensure_manual_task_requirements(&mut latest.run.requirement_matrix, &mut task);
            mark_requirements_from_tasks(&mut latest.run, std::slice::from_ref(&task));
            latest.run.tasks.push(task);
            reset_requirement_review_state(&mut latest.run);
            latest.run.status = if should_schedule {
                "scheduled".to_string()
            } else {
                "paused".to_string()
            };
            latest.run.active = false;
            latest.run.scheduled_start_at = scheduled_start_at;
            latest.run.paused_at = if should_schedule {
                None
            } else {
                Some(Utc::now())
            };
            latest.run.pause_reason = if should_schedule {
                None
            } else {
                Some("New task added to existing board.".to_string())
            };
            latest
                .run
                .append_log("Reused existing board and added a task from start request");
            latest.run.touch();
            let run_id = latest.run.id.clone();
            save_run(&state, &latest.run)?;
            reused_run_id = Some(run_id);
        }

        if let Some(run_id) = reused_run_id {
            (run_id, true)
        } else {
            let run = BoardRun::new(Some(user.0.id.clone()), request)?;
            let run_id = run.id.clone();
            save_run(&state, &run)?;
            (run_id, false)
        }
    };
    let stored = if should_schedule {
        load_user_run(&state, &user.0.id, &run_id)?
    } else {
        begin_run(&state, &user.0.id, &run_id)?
    };
    Ok((
        if should_schedule {
            StatusCode::ACCEPTED
        } else if reused {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(
            json!({ "success": true, "reused": reused, "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
        ),
    ))
}

async fn list_runs(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<RunsQuery>,
) -> Result<Json<Value>> {
    let project_path = trim_string(query.project_path);
    let include_history =
        is_true(query.history.as_deref()) || is_true(query.include_history.as_deref());
    let mut runs = load_runs(&state)?
        .into_iter()
        .filter(|stored| stored.run.user_id.as_deref() == Some(&user.0.id))
        .filter(|stored| {
            project_path
                .as_deref()
                .is_none_or(|path| stored.run.project_path == path)
        })
        .collect::<Vec<_>>();
    for stored in &mut runs {
        backfill_board_session_links(&state, &mut stored.run).await?;
    }
    runs.sort_by(|left, right| {
        right
            .run
            .updated_at
            .cmp(&left.run.updated_at)
            .then_with(|| right.run.id.cmp(&left.run.id))
    });
    if !include_history {
        let mut seen = BTreeMap::<String, ()>::new();
        runs.retain(|stored| seen.insert(stored.run.project_path.clone(), ()).is_none());
    }
    Ok(Json(json!({
        "success": true,
        "runs": runs.into_iter().map(|stored| stored.run.summary_json(Some(stored.path.display().to_string()))).collect::<Vec<_>>(),
    })))
}

async fn delete_runs_for_project(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<RunsQuery>,
) -> Result<Json<Value>> {
    let project_path =
        trim_string(query.project_path).ok_or_else(|| bad_request("projectPath is required"))?;
    let mut deleted = 0usize;
    for stored in load_runs(&state)? {
        if stored.run.user_id.as_deref() == Some(&user.0.id)
            && stored.run.project_path == project_path
        {
            fs::remove_file(&stored.path).map_err(io_error)?;
            deleted += 1;
        }
    }
    Ok(Json(json!({ "success": true, "deleted": deleted })))
}

async fn get_run(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>> {
    let mut stored = load_user_run(&state, &user.0.id, &id)?;
    backfill_board_session_links(&state, &mut stored.run).await?;
    Ok(Json(
        json!({ "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

/// Lazily classify sessions created before board-session metadata existed.
/// This intentionally runs only while serving a board list/detail request;
/// ordinary session reads remain read-only and never scan board snapshots.
async fn backfill_board_session_links(state: &AppState, run: &mut BoardRun) -> Result<()> {
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
        if session
            .board_run_id
            .as_deref()
            .is_some_and(|id| id != run.id)
        {
            continue;
        }
        state
            .sessions
            .mark_board_session(&session_id, run.id.clone(), task_id.clone())
            .await?;
        if let Some(task_id) = task_id
            && let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id)
            && task.provider_session_id.as_deref() != Some(session_id.as_str())
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
        if session
            .board_run_id
            .as_deref()
            .is_some_and(|id| id != run.id)
        {
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
        let explicit_run = prompt.contains(&format!("Board run id: {}", run.id));
        let signature = legacy_board_prompt_signature(prompt);
        if !explicit_run && !signature {
            continue;
        }
        // Signature-only prompts (bootstrap/planning) do not carry a run id.
        // Require proximity to one of this run's recorded provider calls so a
        // normal chat quoting board terminology cannot be classified.
        if !explicit_run && !legacy_prompt_matches_run_telemetry(run, first_user_message.timestamp)
        {
            continue;
        }
        let task_id = legacy_board_task_id(run, prompt);
        if !session.board_session
            || session.board_run_id.as_deref() != Some(run.id.as_str())
            || (task_id.is_some() && session.board_task_id.as_deref() != task_id.as_deref())
        {
            state
                .sessions
                .mark_board_session(&session.id, run.id.clone(), task_id.clone())
                .await?;
        }
        if let Some(task_id) = task_id
            && let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id)
            && task.provider_session_id.as_deref() != Some(session.id.as_str())
        {
            task.provider_session_id = Some(session.id.clone());
            changed_run = true;
        }
    }
    if changed_run {
        // The lazy migration is a deliberate compatibility write. Avoid
        // touching updated_at/control state so loading an old board does not
        // appear as user activity.
        save_run(state, run)?;
    }
    Ok(())
}

fn known_board_session_refs(run: &BoardRun) -> Vec<(String, Option<String>)> {
    let mut refs = BTreeMap::<String, Option<String>>::new();
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
        refs.entry(session_id.to_string()).or_default();
    }
    for task in &run.tasks {
        if let Some(session_id) = task
            .provider_session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            refs.insert(session_id.to_string(), Some(task.id.clone()));
        }
    }
    for entry in &run.prompt_telemetry {
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
        refs.entry(session_id.to_string())
            .and_modify(|existing| {
                if existing.is_none() {
                    *existing = task_id.clone();
                }
            })
            .or_insert(task_id);
    }
    for artifact in &run.qa_artifacts {
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
        refs.insert(session_id.to_string(), task_id);
    }
    refs.into_iter().collect()
}

fn legacy_board_prompt_signature(prompt: &str) -> bool {
    [
        "autonomous Kanban agent",
        "before Kanban planning",
        "io-workbench Kanban board",
        "io-workbench Kanban runner",
        "agentic Kanban task result",
        "RAG promotion candidates for io-workbench",
        "autonomous Kanban run",
    ]
    .iter()
    .any(|signature| prompt.contains(signature))
}

fn legacy_prompt_matches_run_telemetry(run: &BoardRun, timestamp: DateTime<Utc>) -> bool {
    run.prompt_telemetry.iter().any(|entry| {
        entry
            .get("startedAt")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_utc)
            .is_some_and(|started_at| (timestamp - started_at).num_seconds().unsigned_abs() <= 30)
    })
}

fn legacy_board_task_id(run: &BoardRun, prompt: &str) -> Option<String> {
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

async fn pause_run(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    request: Option<Json<PauseRequest>>,
) -> Result<Json<Value>> {
    let request = request
        .map(|Json(request)| request)
        .unwrap_or(PauseRequest { reason: None });
    mutate_run(&state, &user.0.id, &id, |run| {
        request_board_pause(run, trim_string(request.reason));
        Ok(())
    })
}

async fn resume_run(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>> {
    let body = body.map(|Json(body)| body).unwrap_or_else(|| json!({}));
    let _ = mutate_run(&state, &user.0.id, &id, |run| {
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
    let stored = begin_run(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn schedule_run(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ScheduleRequest>,
) -> Result<Json<Value>> {
    let scheduled_start_at = trim_string(request.scheduled_start_at);
    let Some(scheduled_start_at) = scheduled_start_at else {
        return mutate_run(&state, &user.0.id, &id, |run| {
            clear_run_schedule(run);
            Ok(())
        });
    };
    let scheduled_start_at = parse_rfc3339_utc(&scheduled_start_at)
        .ok_or_else(|| bad_request("Scheduled start time is invalid"))?;
    if scheduled_start_at <= Utc::now() {
        let stored = begin_run(&state, &user.0.id, &id)?;
        return Ok(Json(
            json!({ "success": true, "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
        ));
    }
    mutate_run(&state, &user.0.id, &id, |run| {
        if run.loop_started || run.active || run.status == "running" {
            return Err(ServerError::new(
                StatusCode::CONFLICT,
                "Pause the active run before scheduling a future start.",
            ));
        }
        run.status = "scheduled".to_string();
        run.scheduled_start_at = Some(scheduled_start_at);
        run.auto_run_enabled = true;
        run.pause_requested = false;
        run.paused_at = None;
        run.pause_reason = None;
        run.cancellation_reason = None;
        run.abort_source = None;
        run.abort_requested_at = None;
        run.current_provider_session_id = None;
        run.provider_call_started_at = None;
        run.provider_call_label = None;
        bump_control_revision(run);
        run.append_log(format!("Run scheduled to start at {scheduled_start_at}"));
        Ok(())
    })
}

fn clear_run_schedule(run: &mut BoardRun) {
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

async fn abort_run(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    request: Option<Json<PauseRequest>>,
) -> Result<Json<Value>> {
    let request = request
        .map(|Json(request)| request)
        .unwrap_or(PauseRequest { reason: None });
    let reason = trim_string(request.reason).unwrap_or_else(|| "user request".to_string());
    mutate_run(&state, &user.0.id, &id, |run| {
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
    mutate_run(&state, &user.0.id, &id, |run| {
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
    mutate_run(&state, &user.0.id, &id, |run| {
        run.model_strategy = body
            .get("modelStrategy")
            .cloned()
            .or_else(|| Some(body.clone()));
        if let Some(profile) = body.get("runProfile").and_then(Value::as_str) {
            run.run_profile = normalize_run_profile(Some(profile));
        }
        if let Some(overrides) = body.get("taskModelOverrides").cloned() {
            run.task_model_overrides = overrides;
        }
        if let Some(policy) = body.get("sessionPolicy").and_then(Value::as_str) {
            run.session_policy = normalize_session_policy(Some(policy));
        }
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
    mutate_run(&state, &user.0.id, &id, |run| {
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
    mutate_run(&state, &user.0.id, &id, |run| {
        run.tools_settings = body.get("toolsSettings").cloned().or_else(|| Some(body));
        run.append_log("Updated board tool settings");
        Ok(())
    })
}

async fn update_task_models(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    mutate_run(&state, &user.0.id, &id, |run| {
        run.task_model_overrides = body
            .get("taskModelOverrides")
            .or_else(|| body.get("models"))
            .cloned()
            .unwrap_or(body);
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
    mutate_run(&state, &user.0.id, &id, |run| {
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
    let _guard = board_run_mutation_lock();
    let mut stored = load_user_run(&state, &user.0.id, &id)?;
    let mut task = BoardTask::manual(&mut stored.run, request)?;
    if !task.status.starts_with("backlog") {
        ensure_manual_task_requirements(&mut stored.run.requirement_matrix, &mut task);
        mark_requirements_from_tasks(&mut stored.run, std::slice::from_ref(&task));
        reset_requirement_review_state(&mut stored.run);
    }
    stored.run.tasks.push(task);
    stored.run.append_log("Added manual board task");
    stored.run.touch();
    save_run(&state, &stored.run)?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({ "success": true, "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
        ),
    ))
}

async fn draft_tasks(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<PromptRequest>,
) -> Result<Json<Value>> {
    let stored = load_user_run(&state, &user.0.id, &id)?;
    let prompt =
        trim_string(request.prompt.clone()).ok_or_else(|| bad_request("Prompt is required"))?;
    let attempt = generate_prompt_task_drafts(
        &state,
        &stored.run,
        &prompt,
        request.model.as_deref(),
        request.run_profile.as_deref(),
    )
    .await;
    {
        let _guard = board_run_mutation_lock();
        let mut stored = load_user_run(&state, &user.0.id, &id)?;
        record_prompt_task_generation_attempt(
            &mut stored.run,
            "Kanban task draft preview",
            &attempt,
        );
        stored.run.touch();
        save_run(&state, &stored.run)?;
    }
    let (tasks, warning) = attempt.result?;
    Ok(Json(
        json!({ "success": true, "tasks": tasks, "warning": warning }),
    ))
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
    let run_profile = request
        .run_profile
        .as_deref()
        .map(|value| normalize_run_profile(Some(value)))
        .unwrap_or_default();
    let (task_id, response) = {
        let _guard = board_run_mutation_lock();
        let mut stored = load_user_run(&state, &user.0.id, &id)?;
        let task = backlog_generation_placeholder(&mut stored.run, &prompt, &model, &run_profile);
        let task_id = task.id.clone();
        stored.run.tasks.push(task);
        stored.run.append_log(format!(
            "Started backlog task generation from prompt: {task_id}"
        ));
        stored.run.touch();
        save_run(&state, &stored.run)?;
        (
            task_id.clone(),
            json!({
                "success": true,
                "taskId": task_id,
                "run": stored.run.detail_json(Some(stored.path.display().to_string())),
            }),
        )
    };
    spawn_backlog_prompt_generation(
        state.clone(),
        user.0.id.clone(),
        id,
        task_id.clone(),
        prompt,
        model,
        run_profile,
    );
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn promote_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    let _ = update_task_status(&state, &user.0.id, &id, &[task_id], "pending")?;
    let stored = begin_run(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn demote_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    update_task_status(&state, &user.0.id, &id, &[task_id], "backlog")
}

async fn retry_attention_tasks(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<TaskIdsRequest>,
) -> Result<Json<Value>> {
    let ids = request.task_ids.unwrap_or_default();
    let _ = update_task_status(&state, &user.0.id, &id, &ids, "pending")?;
    let stored = begin_run(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn retry_backlog_failed_tasks(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<TaskIdsRequest>,
) -> Result<Json<Value>> {
    let ids = request.task_ids.unwrap_or_default();
    let targets = {
        let _guard = board_run_mutation_lock();
        let mut stored = load_user_run(&state, &user.0.id, &id)?;
        let requested = ids.into_iter().collect::<BTreeSet<_>>();
        let mut targets = Vec::new();
        for task in &mut stored.run.tasks {
            if task.status != "backlog_failed"
                || (!requested.is_empty() && !requested.contains(&task.id))
            {
                continue;
            }
            let prompt = if task.prompt.trim().is_empty() {
                task.details.trim().to_string()
            } else {
                task.prompt.trim().to_string()
            };
            if prompt.is_empty() {
                continue;
            }
            let model = task_reference_value(task, "Breakdown model:");
            let run_profile = task_reference_value(task, "Breakdown profile:");
            task.status = "backlog_generating".to_string();
            task.summary.clear();
            task.error = None;
            task.started_at = None;
            task.completed_at = None;
            task.backlog_generation_task = true;
            targets.push((task.id.clone(), prompt, model, run_profile));
        }
        if targets.is_empty() {
            return Err(not_found("Danger run or failed backlog task not found"));
        }
        stored.run.append_log(format!(
            "Retrying {} failed backlog task generation request(s)",
            targets.len()
        ));
        stored.run.touch();
        save_run(&state, &stored.run)?;
        targets
    };
    for (task_id, prompt, model, run_profile) in targets {
        spawn_backlog_prompt_generation(
            state.clone(),
            user.0.id.clone(),
            id.clone(),
            task_id,
            prompt,
            model,
            run_profile,
        );
    }
    let stored = load_user_run(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

async fn delete_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>> {
    let _guard = board_run_mutation_lock();
    let mut stored = load_user_run(&state, &user.0.id, &id)?;
    delete_board_task(&mut stored.run, &task_id)?;
    stored.run.append_log("Deleted board task");
    stored.run.touch();
    save_run(&state, &stored.run)?;
    Ok(Json(
        json!({ "success": true, "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

fn delete_board_task(run: &mut BoardRun, task_id: &str) -> Result<()> {
    if run.current_task_id.as_deref() == Some(task_id) {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The currently executing task cannot be deleted.",
        ));
    }
    let before = run.tasks.len();
    run.tasks.retain(|task| task.id != task_id);
    if run.tasks.len() == before {
        return Err(not_found("Danger run or backlog task not found"));
    }
    for task in &mut run.tasks {
        task.depends_on.retain(|dependency| dependency != task_id);
        if task.source_task_id.as_deref() == Some(task_id) {
            task.source_task_id = None;
        }
        if task.source_qa_task_id.as_deref() == Some(task_id) {
            task.source_qa_task_id = None;
        }
    }
    Ok(())
}

async fn update_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath((id, task_id)): AxumPath<(String, String)>,
    Json(request): Json<UpdateTaskRequest>,
) -> Result<Json<Value>> {
    let status = normalize_task_status(request.status.as_deref(), "")?;
    let _ = update_task_status(&state, &user.0.id, &id, &[task_id], &status)?;
    if status == "pending" {
        let stored = begin_run(&state, &user.0.id, &id)?;
        return Ok(Json(
            json!({ "success": true, "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
        ));
    }
    let stored = load_user_run(&state, &user.0.id, &id)?;
    Ok(Json(
        json!({ "success": true, "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

fn update_task_status(
    state: &AppState,
    user_id: &str,
    run_id: &str,
    task_ids: &[String],
    status: &str,
) -> Result<Json<Value>> {
    let status = normalize_task_status(Some(status), "")?;
    let _guard = board_run_mutation_lock();
    let mut stored = load_user_run(state, user_id, run_id)?;
    let mut updated = 0usize;
    let update_all_attention = task_ids.is_empty() && status == "pending";
    let mut updated_tasks = Vec::new();
    for task in &mut stored.run.tasks {
        let matches = task_ids.iter().any(|id| id == &task.id)
            || (update_all_attention
                && matches!(
                    task.status.as_str(),
                    "blocked" | "failed" | "backlog_failed"
                ));
        if matches {
            task.status = status.clone();
            task.error = None;
            if matches!(status.as_str(), "pending" | "backlog") {
                task.started_at = None;
                task.completed_at = None;
            }
            if status == "pending" && is_user_authored_task(task) {
                ensure_manual_task_requirements(&mut stored.run.requirement_matrix, task);
            }
            updated_tasks.push(task.clone());
            updated += 1;
        }
    }
    if updated == 0 {
        return Err(not_found("Danger run or task not found"));
    }
    if status == "pending" {
        mark_requirements_from_tasks(&mut stored.run, &updated_tasks);
        reset_requirement_review_state(&mut stored.run);
    }
    stored
        .run
        .append_log(format!("Moved {updated} board task(s) to {status}"));
    stored.run.touch();
    save_run(state, &stored.run)?;
    Ok(Json(
        json!({ "success": true, "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

fn mutate_run(
    state: &AppState,
    user_id: &str,
    id: &str,
    mutate: impl FnOnce(&mut BoardRun) -> Result<()>,
) -> Result<Json<Value>> {
    let stored = mutate_stored_run(state, user_id, id, mutate)?;
    Ok(Json(
        json!({ "success": true, "run": stored.run.detail_json(Some(stored.path.display().to_string())) }),
    ))
}

fn mutate_stored_run(
    state: &AppState,
    user_id: &str,
    id: &str,
    mutate: impl FnOnce(&mut BoardRun) -> Result<()>,
) -> Result<StoredRun> {
    let _guard = board_run_mutation_lock();
    let mut stored = load_user_run(state, user_id, id)?;
    mutate(&mut stored.run)?;
    stored.run.touch();
    save_run(state, &stored.run)?;
    Ok(stored)
}

fn begin_run(state: &AppState, user_id: &str, id: &str) -> Result<StoredRun> {
    let (should_spawn, stored) = {
        let _guard = board_run_mutation_lock();
        let mut stored = load_user_run(state, user_id, id)?;
        let should_spawn = !stored.run.loop_started;
        stored.run.status = "running".to_string();
        stored.run.scheduled_start_at = None;
        stored.run.active = true;
        stored.run.loop_started = true;
        stored.run.auto_run_enabled = true;
        stored.run.pause_requested = false;
        stored.run.paused_at = None;
        stored.run.pause_reason = None;
        stored.run.cancellation_reason = None;
        bump_control_revision(&mut stored.run);
        stored.run.current_phase = Some("task_execution".to_string());
        stored.run.phase_started_at = Some(Utc::now());
        stored.run.phase_details = Some(json!({ "source": "kanban_board" }));
        stored.run.append_log("Agentic board execution started");
        stored.run.touch();
        save_run(state, &stored.run)?;
        (should_spawn, stored)
    };

    if should_spawn {
        let state = state.clone();
        let user_id = user_id.to_string();
        let run_id = id.to_string();
        tokio::spawn(async move {
            if let Err(error) = run_board_loop(state, user_id, run_id).await {
                tracing::warn!(error = %server_error_message(&error), "agentic board runner failed");
            }
        });
    }

    Ok(stored)
}

async fn run_board_loop(state: AppState, user_id: String, run_id: String) -> Result<()> {
    loop {
        let mut stored = load_user_run(&state, &user_id, &run_id)?;
        if matches!(
            stored.run.status.as_str(),
            "cancelled" | "failed" | "blocked" | "completed"
        ) {
            if matches!(stored.run.status.as_str(), "failed" | "blocked") {
                let status_for_retry = stored.run.status.clone();
                schedule_auto_retry_if_eligible(&mut stored.run, &status_for_retry);
            }
            stored.run.active = false;
            stored.run.loop_started = false;
            save_run(&state, &stored.run)?;
            return Ok(());
        }
        if stored.run.status == "paused" || stored.run.pause_requested {
            settle_board_pause(&mut stored.run);
            stored.run.touch();
            save_run(&state, &stored.run)?;
            return Ok(());
        }

        if !stored.run.bootstrap_complete {
            bootstrap_agentic_run(&state, &user_id, &run_id).await?;
            continue;
        }

        if should_plan_tasks(&stored.run) {
            stored.run.current_phase = Some("task_planning".to_string());
            stored.run.phase_started_at = Some(Utc::now());
            stored.run.phase_details =
                Some(json!({ "planningRound": stored.run.planning_round + 1 }));
            stored
                .run
                .append_log("Planning Kanban task queue from requirements and codebase context");
            stored.run.touch();
            save_run(&state, &stored.run)?;
            plan_agentic_tasks(&state, &user_id, &run_id).await?;
            continue;
        }

        let Some(task_index) = pick_next_task_index(&stored.run) else {
            let waiting_tasks = dependency_waiting_tasks(&stored.run);
            if !waiting_tasks.is_empty() {
                stored.run.status = "blocked".to_string();
                stored.run.active = false;
                stored.run.loop_started = false;
                stored.run.current_phase = Some("blocked".to_string());
                stored.run.append_log(format!(
                    "No runnable tasks: {} task(s) waiting on blocked dependencies ({})",
                    waiting_tasks.len(),
                    waiting_tasks.join(", ")
                ));
                stored.run.touch();
                save_run(&state, &stored.run)?;
                return Ok(());
            }
            let implementation_needed = requirements_needing_implementation(&stored.run);
            if uses_simplified_orchestration(&stored.run) && !implementation_needed.is_empty() {
                if !stored.run.v2_coverage_fallback_added {
                    let fallback_tasks =
                        build_v2_fallback_feature_queue(&stored.run, &implementation_needed);
                    mark_requirements_from_tasks(&mut stored.run, &fallback_tasks);
                    let count = fallback_tasks.len();
                    stored.run.tasks.extend(fallback_tasks);
                    stored.run.v2_coverage_fallback_added = true;
                    stored.run.matrix_gap_review_complete = true;
                    stored.run.final_matrix_qa_complete = false;
                    stored.run.append_log(format!(
                        "Added {count} bounded coverage feature(s) without another planning call"
                    ));
                    stored.run.touch();
                    save_run(&state, &stored.run)?;
                    continue;
                }
                stored.run.final_review = Some(json!({
                    "complete": false,
                    "summary": format!(
                        "Feature execution ended with uncovered requirements: {}",
                        implementation_needed
                            .iter()
                            .filter_map(|item| item.get("id").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    "remainingTasks": [],
                }));
                stored.run.append_log(
                    stored
                        .run
                        .final_review
                        .as_ref()
                        .and_then(|value| value.get("summary"))
                        .and_then(Value::as_str)
                        .unwrap_or("Feature execution ended with uncovered requirements")
                        .to_string(),
                );
                stored.run.status = "blocked".to_string();
                stored.run.active = false;
                stored.run.loop_started = false;
                stored.run.matrix_gap_review_complete = true;
                stored.run.touch();
                save_run(&state, &stored.run)?;
                return Ok(());
            }
            if !stored.run.matrix_gap_review_complete {
                stored.run.current_phase = Some("matrix_gap_review".to_string());
                stored.run.phase_started_at = Some(Utc::now());
                stored.run.phase_details =
                    Some(json!({ "reviewRound": stored.run.review_round + 1 }));
                stored.run.append_log("Running requirement gap review");
                stored.run.touch();
                save_run(&state, &stored.run)?;
                review_requirement_gaps(&state, &user_id, &run_id).await?;
                continue;
            }
            if !uses_simplified_orchestration(&stored.run)
                && !stored.run.agents_knowledge_updated
                && append_agents_knowledge_task(
                    &mut stored.run,
                    "Implementation work completed before final QA",
                    None,
                )
            {
                stored.run.current_phase = Some("agents_update".to_string());
                stored.run.phase_started_at = Some(Utc::now());
                stored
                    .run
                    .append_log("Appended AGENTS.md knowledge update task");
                stored.run.touch();
                save_run(&state, &stored.run)?;
                continue;
            }
            if !stored.run.final_matrix_qa_complete {
                if append_final_qa_task(&mut stored.run, "Final validation before completion") {
                    stored.run.current_phase = Some("final_qa".to_string());
                    stored.run.phase_started_at = Some(Utc::now());
                    stored.run.append_log("Appended system final QA task");
                    stored.run.touch();
                    save_run(&state, &stored.run)?;
                    continue;
                }
                stored.run.current_phase = Some("final_review".to_string());
                stored.run.phase_started_at = Some(Utc::now());
                stored.run.phase_details =
                    Some(json!({ "reviewRound": stored.run.review_round + 1 }));
                stored.run.append_log("Running final agentic review");
                stored.run.touch();
                save_run(&state, &stored.run)?;
                final_agentic_review(&state, &user_id, &run_id).await?;
                continue;
            }
            let integrity_issues = requirement_integrity_issues(&stored.run);
            if !integrity_issues.is_empty() {
                stored.run.status = "blocked".to_string();
                stored.run.active = false;
                stored.run.loop_started = false;
                stored.run.current_phase = Some("blocked".to_string());
                stored.run.final_review = Some(json!({
                    "complete": false,
                    "summary": format!(
                        "Original requirement integrity failed: {}",
                        integrity_issues.join(" ")
                    ),
                    "remainingTasks": [],
                }));
                stored.run.append_log(format!(
                    "Immutable requirement integrity failed: {}",
                    integrity_issues.join(" ")
                ));
                stored.run.touch();
                save_run(&state, &stored.run)?;
                return Ok(());
            }
            if !stored.run.requirement_matrix.is_empty() && !all_requirements_satisfied(&stored.run)
            {
                stored.run.status = "blocked".to_string();
                stored.run.active = false;
                stored.run.loop_started = false;
                stored.run.current_phase = Some("blocked".to_string());
                stored.run.final_review = Some(json!({
                    "complete": false,
                    "summary": format!(
                        "Requirement matrix not satisfied: {}",
                        unsatisfied_requirement_summary(&stored.run)
                    ),
                    "remainingTasks": [],
                }));
                stored.run.append_log(format!(
                    "Agentic board stopped with unsatisfied requirements: {}",
                    unsatisfied_requirement_summary(&stored.run)
                ));
                stored.run.touch();
                save_run(&state, &stored.run)?;
                return Ok(());
            }
            if !has_passing_final_qa_task(&stored.run) {
                stored.run.status = "blocked".to_string();
                stored.run.active = false;
                stored.run.loop_started = false;
                stored.run.current_phase = Some("blocked".to_string());
                stored.run.final_review = Some(json!({
                    "complete": false,
                    "summary": "Requirement matrix reached terminal statuses, but final QA did not cover every requirement.",
                    "remainingTasks": [],
                }));
                stored
                    .run
                    .append_log("Agentic board stopped before full final QA coverage completed");
                stored.run.touch();
                save_run(&state, &stored.run)?;
                return Ok(());
            }
            if append_promotion_review_task(&mut stored.run, "Final promotion review") {
                stored.run.current_phase = Some("promotion_review".to_string());
                stored.run.phase_started_at = Some(Utc::now());
                stored.run.append_log("Appended RAG promotion review task");
                stored.run.touch();
                save_run(&state, &stored.run)?;
                continue;
            }
            stored.run.status = "completed".to_string();
            stored.run.active = false;
            stored.run.loop_started = false;
            stored.run.current_task_id = None;
            stored.run.current_task_title.clear();
            stored.run.current_task_status.clear();
            stored.run.current_phase = Some("completed".to_string());
            stored.run.phase_details = Some(json!({ "taskCount": stored.run.tasks.len() }));
            stored.run.final_matrix_qa_complete = true;
            stored.run.final_review = Some(json!({
                "complete": true,
                "summary": "All runnable board tasks completed.",
            }));
            stored.run.append_log("Agentic board execution completed");
            stored.run.touch();
            save_run(&state, &stored.run)?;
            return Ok(());
        };

        let task_id = stored.run.tasks[task_index].id.clone();
        let task_title = stored.run.tasks[task_index].title.clone();
        let started_at = Utc::now();
        stored.run.status = "running".to_string();
        stored.run.current_task_id = Some(task_id.clone());
        stored.run.current_task_title = task_title.clone();
        stored.run.current_task_status = "in_progress".to_string();
        let task_phase = stored
            .run
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
        stored.run.current_phase = Some(task_phase.to_string());
        stored.run.phase_started_at = Some(started_at);
        stored.run.phase_details = Some(json!({ "taskId": task_id, "taskTitle": task_title }));
        apply_task_model_routing(&mut stored.run, task_index);
        if let Some(task) = stored.run.tasks.get_mut(task_index) {
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
        stored.run.append_log(format!("Executing task {task_id}"));
        stored.run.touch();
        save_run(&state, &stored.run)?;

        if stored
            .run
            .tasks
            .get(task_index)
            .is_some_and(is_promotion_review_task)
        {
            execute_promotion_review_task(&state, &user_id, &run_id, &mut stored.run, task_index)
                .await?;
            stored.run.current_task_id = None;
            stored.run.current_task_title.clear();
            stored.run.current_task_status.clear();
            stored.run.touch();
            save_run(&state, &stored.run)?;
            continue;
        }

        let managed_git_ready =
            ensure_managed_git_branch_for_task_group(&mut stored.run, &task_id).await;
        if let Err(error) = managed_git_ready {
            let message = server_error_message(&error);
            if let Some(task) = stored.run.tasks.iter_mut().find(|task| task.id == task_id) {
                task.status = "blocked".to_string();
                task.error = Some(message.clone());
                task.summary = message.clone();
                task.completed_at = Some(Utc::now());
                task.qa_passed = Some(false);
            }
            stored.run.status = "blocked".to_string();
            stored.run.active = false;
            stored.run.loop_started = false;
            stored.run.append_log(format!(
                "Task blocked before execution by managed git policy: {message}"
            ));
            stored.run.touch();
            save_run(&state, &stored.run)?;
            return Ok(());
        }
        save_run(&state, &stored.run)?;

        if !ensure_tdd_baseline_for_task(&state, &user_id, &run_id, &mut stored.run, task_index)
            .await?
        {
            stored.run.current_task_id = None;
            stored.run.current_task_title.clear();
            stored.run.current_task_status.clear();
            stored.run.touch();
            save_run(&state, &stored.run)?;
            continue;
        }
        stored = load_user_run(&state, &user_id, &run_id)?;
        if stored.run.status == "paused" || stored.run.pause_requested {
            settle_board_pause(&mut stored.run);
            stored.run.touch();
            save_run(&state, &stored.run)?;
            return Ok(());
        }
        let Some(task_index) = stored.run.tasks.iter().position(|task| task.id == task_id) else {
            continue;
        };
        if let Some(task) = stored.run.tasks.get_mut(task_index) {
            if task.tdd_phase == "qa_failed_expected" {
                task.tdd_phase = "dev_pending".to_string();
            }
        }

        attach_rag_context_for_task(&mut stored.run, task_index).await;
        stored.run.touch();
        save_run(&state, &stored.run)?;
        stored = load_user_run(&state, &user_id, &run_id)?;
        if stored.run.status == "paused" || stored.run.pause_requested {
            settle_board_pause(&mut stored.run);
            stored.run.touch();
            save_run(&state, &stored.run)?;
            return Ok(());
        }
        let Some(task_index) = stored.run.tasks.iter().position(|task| task.id == task_id) else {
            continue;
        };

        let before_workspace = capture_workspace_snapshot(&stored.run.project_path);
        stored.run.provider_call_started_at = Some(Utc::now());
        stored.run.provider_call_label = Some(format!("task execution for {task_id}"));
        stored.run.touch();
        save_run(&state, &stored.run)?;
        let provider_attempt =
            execute_provider_task_with_fallback(&state, &stored.run, task_index).await;
        let mut stored = load_user_run(&state, &user_id, &run_id)?;
        let task_position = stored.run.tasks.iter().position(|task| task.id == task_id);
        let now = Utc::now();

        if let Some(fallback) = provider_attempt.fallback {
            let previous_provider = stored.run.provider.clone();
            let previous_model = stored.run.model.clone();
            stored.run.provider = fallback.provider.clone();
            stored.run.model = fallback.model.clone();
            stored.run.last_effective_model = Some(fallback.model.clone());
            reset_provider_session(&mut stored.run, "provider fallback");
            stored.run.model_history.push(json!({
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
                .run
                .append_log("Primary provider call failed; activated configured fallback");
        }

        match provider_attempt.result {
            Ok(mut result) => {
                let mut parsed = parse_execution_result(&result.assistant_text)
                    .unwrap_or_else(|| missing_json_task_result(&result.assistant_text));
                let mut fatal_provider_errors =
                    filter_fatal_provider_errors(&result.errors, result.exit_code);
                if result.errors.len() > fatal_provider_errors.len() {
                    stored.run.append_log(format!(
                        "Ignored {} non-fatal provider advisory message(s) for {task_id}",
                        result.errors.len() - fatal_provider_errors.len()
                    ));
                }
                let mut failed_with_provider_error =
                    result.exit_code != 0 || !fatal_provider_errors.is_empty();

                if !failed_with_provider_error
                    && is_recoverable_self_reported_blocker(&parsed)
                    && stored
                        .run
                        .tasks
                        .get(task_position.unwrap_or(task_index))
                        .map(|task| task.attempt_count < MAX_TASK_ATTEMPTS)
                        .unwrap_or(false)
                {
                    let stale_tool_blocker = is_tool_environment_self_reported_blocker(&parsed)
                        && !provider_events_have_tool_evidence(&result.stream_events);
                    if stale_tool_blocker {
                        reset_provider_session(
                            &mut stored.run,
                            &format!("stale tool-environment blocker reported by {task_id}"),
                        );
                    }
                    if let Some(index) = task_position {
                        if let Some(task) = stored.run.tasks.get_mut(index) {
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
                    stored.run.append_log(if stale_tool_blocker {
                        format!("Retrying stale tool-environment blocker for {task_id} in a fresh session")
                    } else {
                        format!("Retrying recoverable blocker for {task_id}")
                    });
                    stored.run.touch();
                    save_run(&state, &stored.run)?;

                    match execute_provider_task(&state, &stored.run, task_index).await {
                        Ok(retry_result) => {
                            result = retry_result;
                            parsed = parse_execution_result(&result.assistant_text).unwrap_or_else(
                                || missing_json_task_result(&result.assistant_text),
                            );
                            fatal_provider_errors =
                                filter_fatal_provider_errors(&result.errors, result.exit_code);
                            if result.errors.len() > fatal_provider_errors.len() {
                                stored.run.append_log(format!(
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
                    record_task_workspace_changes(&mut stored.run, &task_id, before_workspace);
                if failed_with_provider_error
                    && should_treat_provider_errors_as_followup(&result, &parsed, &change_summary)
                {
                    parsed = convert_missing_json_provider_error_to_followup(&parsed, &result);
                    failed_with_provider_error = false;
                    stored.run.append_log(format!(
                        "Converted provider error to follow-up for {task_id} because the task changed files but missed final JSON"
                    ));
                }
                if !failed_with_provider_error
                    && should_repair_task_result(&stored.run, &task_id, &parsed, &change_summary)
                {
                    parsed = repair_task_result_if_needed(
                        &state,
                        &user_id,
                        &run_id,
                        &task_id,
                        task_index,
                        &result.assistant_text,
                        parsed,
                        &change_summary,
                    )
                    .await;
                }
                let is_agents_knowledge_task = stored
                    .run
                    .tasks
                    .get(task_position.unwrap_or(task_index))
                    .map(|task| task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID)
                    .unwrap_or(false);
                if !failed_with_provider_error && !is_agents_knowledge_task {
                    let task_for_validation = stored
                        .run
                        .tasks
                        .get(task_position.unwrap_or(task_index))
                        .cloned();
                    let validation = if let Some(task) = task_for_validation.as_ref() {
                        run_tdd_validation(&stored.run, task, "feature").await
                    } else {
                        run_deterministic_validation(&stored.run, &task_id, "feature").await
                    };
                    parsed = apply_deterministic_validation_result(parsed, &validation);
                    stored.run.validation_runs.push(validation.clone());
                    if let Some(index) = task_position {
                        if let Some(task) = stored.run.tasks.get_mut(index) {
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
                        &stored.run,
                        &task_id,
                        parsed,
                        &change_summary,
                    );
                }
                refresh_codebase_context_after_task(&mut stored.run, &change_summary);
                let completion_summary = resolved_execution_summary(&parsed, &result.summary);
                if let Some(index) = task_position {
                    let task = &mut stored.run.tasks[index];
                    if result.session_id.is_some() {
                        task.provider_session_id = result.session_id.clone();
                    }
                    task.transcript.extend(result.stream_events.clone());
                    task.transcript.push(json!({
                        "timestamp": now,
                        "kind": "assistant",
                        "provider": stored.run.provider,
                        "content": result.assistant_text,
                    }));
                    if !result.stderr.trim().is_empty() {
                        task.transcript.push(json!({
                            "timestamp": now,
                            "kind": "stderr",
                            "provider": stored.run.provider,
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
                    task.changed_files = value_to_strings(parsed.get("changedFiles").cloned());
                    if task.changed_files.is_empty() {
                        task.changed_files = change_summary_paths(&change_summary);
                    }
                    task.evidence = value_to_strings(parsed.get("evidence").cloned());
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
                    } else if parsed_status_done(Some(&parsed)) {
                        task.status = "completed".to_string();
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
                            stored.run.final_matrix_qa_complete = true;
                        }
                        if task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID {
                            stored.run.agents_knowledge_updated = true;
                            stored.run.agents_context =
                                Some(read_agents_context(&stored.run.project_path));
                        }
                    } else {
                        let needs_followup = parsed
                            .get("status")
                            .and_then(Value::as_str)
                            .is_some_and(|status| status == "needs_followup");
                        task.status = if needs_followup {
                            "completed".to_string()
                        } else {
                            "failed".to_string()
                        };
                        if task.tdd_phase != "disabled" && !is_qa_task(task) {
                            task.tdd_phase = "fix_pending".to_string();
                            task.fix_attempts = task.fix_attempts.saturating_add(1);
                        }
                        task.qa_passed = Some(false);
                        task.error = Some(
                            parsed
                                .get("summary")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| {
                                    format!("Provider exited with code {}", result.exit_code)
                                }),
                        );
                    }
                }
                if let Some(session_id) = result.session_id.clone() {
                    stored.run.current_provider_session_id = Some(session_id.clone());
                    stored.run.actual_session_id = Some(session_id.clone());
                    if stored.run.session_id.is_none()
                        || should_resume_provider_session(&stored.run)
                    {
                        stored.run.session_id = Some(session_id);
                    }
                }
                if let Some(task_for_usage) = stored.run.tasks.get(task_index) {
                    let prompt_for_usage =
                        build_task_execution_prompt(&stored.run, task_for_usage, task_index);
                    increment_provider_usage(
                        &mut stored.run,
                        &prompt_for_usage,
                        &result.assistant_text,
                        result.session_id.as_deref(),
                        result.token_usage.as_ref(),
                    );
                }
                apply_task_result_to_run(&mut stored.run, &task_id, &parsed);
                if !failed_with_provider_error {
                    ingest_rag_task_outcome(&mut stored.run, &task_id, &parsed).await;
                }
                if !failed_with_provider_error && !uses_simplified_orchestration(&stored.run) {
                    append_suggested_backlog_tasks_from_result(&mut stored.run, &task_id, &parsed);
                }
                let qa_followup_added = if failed_with_provider_error {
                    false
                } else if should_queue_qa_verdict_retry(
                    &stored.run,
                    &task_id,
                    &parsed,
                    &change_summary,
                ) {
                    queue_qa_verdict_retry(&mut stored.run, &task_id, &parsed)
                } else if is_qa_verdict_retry_task_id(&stored.run, &task_id)
                    && is_missing_final_json_result(&parsed)
                {
                    mark_qa_verdict_retry_blocked(&mut stored.run, &task_id, &parsed);
                    true
                } else if is_qa_task_id(&stored.run, &task_id) && qa_needs_followup(&parsed) {
                    append_followup_task_if_needed(&mut stored.run, &task_id, &parsed)
                } else {
                    false
                };
                let followup_added = if failed_with_provider_error || qa_followup_added {
                    false
                } else {
                    append_followup_task_if_needed(&mut stored.run, &task_id, &parsed)
                };
                let post_qa_added = if failed_with_provider_error
                    || qa_followup_added
                    || followup_added
                    || uses_simplified_orchestration(&stored.run)
                {
                    false
                } else {
                    let source_task = stored
                        .run
                        .tasks
                        .iter()
                        .find(|task| task.id == task_id)
                        .cloned();
                    source_task
                        .as_ref()
                        .filter(|task| {
                            task.status == "completed"
                                && task_needs_immediate_ai_qa(task, &parsed)
                                && !has_task_qa_for_source(&stored.run, &task.id)
                        })
                        .map(|task| {
                            append_task_qa_task(
                                &mut stored.run,
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
                    || uses_simplified_orchestration(&stored.run)
                {
                    false
                } else {
                    let source_task = stored
                        .run
                        .tasks
                        .iter()
                        .find(|task| task.id == task_id)
                        .cloned();
                    source_task
                        .as_ref()
                        .filter(|task| {
                            task.status == "completed"
                                && task_needs_agents_knowledge_update(task, &parsed)
                                && !has_agents_knowledge_task_for_source(&stored.run, &task.id)
                        })
                        .map(|task| {
                            append_agents_knowledge_task(
                                &mut stored.run,
                                "Preserve durable code structure, command, database, migration, or verification knowledge for later tasks",
                                Some(task),
                            )
                        })
                        .unwrap_or(false)
                };
                if !qa_followup_added && !followup_added {
                    if let Some(entry) =
                        compact_provider_session_after_task_group(&state, &stored.run, &task_id)
                            .await
                    {
                        stored.run.compaction_ledger.push(entry);
                    }
                }
                if post_agents_added {
                    stored
                        .run
                        .append_log(format!("Inserted post-task AGENTS work after {task_id}"));
                }
                if post_qa_added {
                    stored
                        .run
                        .append_log(format!("Inserted post-task QA work after {task_id}"));
                }
                let completed_for_git = stored
                    .run
                    .tasks
                    .iter()
                    .find(|task| task.id == task_id)
                    .is_some_and(|task| task.status == "completed");
                if completed_for_git {
                    if let Err(error) =
                        finalize_managed_git_task_group(&mut stored.run, &task_id).await
                    {
                        let message = server_error_message(&error);
                        if let Some(task) =
                            stored.run.tasks.iter_mut().find(|task| task.id == task_id)
                        {
                            task.status = "blocked".to_string();
                            task.error = Some(message.clone());
                            task.summary = if task.summary.trim().is_empty() {
                                message.clone()
                            } else {
                                format!("{} {}", task.summary, message)
                            };
                        }
                        stored.run.status = "blocked".to_string();
                        stored.run.active = false;
                        stored.run.loop_started = false;
                        stored.run.append_log(format!(
                            "Blocked after completion by managed git policy: {message}"
                        ));
                        stored.run.touch();
                        save_run(&state, &stored.run)?;
                        return Ok(());
                    }
                }
                stored.run.append_log(format!(
                    "Task {task_id} finished with exit code {}",
                    result.exit_code
                ));
            }
            Err(error) => {
                let message = server_error_message(&error);
                if let Some(index) = task_position {
                    let task = &mut stored.run.tasks[index];
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
                stored.run.append_log(format!(
                    "Task {task_id} failed: {}",
                    server_error_message(&error)
                ));
            }
        }

        stored.run.current_task_id = None;
        stored.run.current_task_title.clear();
        stored.run.current_task_status.clear();
        stored.run.provider_call_started_at = None;
        stored.run.provider_call_label = None;
        stored.run.current_provider_session_id = None;
        stored.run.touch();
        save_run(&state, &stored.run)?;
    }
}

async fn bootstrap_agentic_run(state: &AppState, user_id: &str, run_id: &str) -> Result<()> {
    let snapshot = load_user_run(state, user_id, run_id)?.run;
    if bootstrap_should_yield(&snapshot) {
        return Ok(());
    }
    let workspace_baseline = snapshot
        .workspace_baseline
        .is_none()
        .then(|| capture_workspace_snapshot(&snapshot.project_path));
    let agents_context = read_agents_context(&snapshot.project_path);
    mutate_stored_run(state, user_id, run_id, |run| {
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
    if bootstrap_checkpoint_requested(state, user_id, run_id)? {
        return Ok(());
    }

    let snapshot = load_user_run(state, user_id, run_id)?.run;
    let source_bundle = build_source_bundle(&snapshot.project_path, &snapshot.source_prompt);
    mutate_stored_run(state, user_id, run_id, |run| {
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
    if bootstrap_checkpoint_requested(state, user_id, run_id)? {
        return Ok(());
    }

    let mut rag_snapshot = load_user_run(state, user_id, run_id)?.run;
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
    mutate_stored_run(state, user_id, run_id, |run| {
        run.rag_ingestions.extend(rag_ingestions);
        run.rag_trace_refs.extend(rag_trace_refs);
        trim_rag_history(run);
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, run_id)? {
        return Ok(());
    }

    let source_chunk_count = load_user_run(state, user_id, run_id)?
        .run
        .source_chunks
        .len();
    mutate_stored_run(state, user_id, run_id, |run| {
        set_phase(
            run,
            "requirement_extraction",
            json!({ "sourceChunks": source_chunk_count }),
        );
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, run_id)? {
        return Ok(());
    }
    let requirements = extract_requirements_for_run(state, user_id, run_id).await?;
    mutate_stored_run(state, user_id, run_id, |run| {
        run.requirement_matrix = requirements.clone();
        run.requirement_baseline = requirements
            .iter()
            .map(|requirement| {
                json!({
                    "id": requirement.get("id").cloned().unwrap_or(Value::Null),
                    "sourceChunkId": requirement.get("sourceChunkId").cloned().unwrap_or(Value::Null),
                    "sourcePath": requirement.get("sourcePath").cloned().unwrap_or(Value::Null),
                    "heading": requirement.get("heading").cloned().unwrap_or(Value::Null),
                    "requirement": requirement.get("requirement").cloned().unwrap_or(Value::Null),
                    "acceptanceCriteria": requirement.get("acceptanceCriteria").cloned().unwrap_or(json!([])),
                    "priority": requirement.get("priority").cloned().unwrap_or(json!("medium")),
                    "dependencies": requirement.get("dependencies").cloned().unwrap_or(json!([])),
                })
            })
            .collect();
        run.append_log(format!(
            "Requirement matrix contains {} item(s)",
            run.requirement_matrix.len()
        ));
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, run_id)? {
        return Ok(());
    }

    let project_path = load_user_run(state, user_id, run_id)?.run.project_path;
    let codebase_bundle = build_codebase_bundle(&project_path);
    mutate_stored_run(state, user_id, run_id, |run| {
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
    if bootstrap_checkpoint_requested(state, user_id, run_id)? {
        return Ok(());
    }

    mutate_stored_run(state, user_id, run_id, |run| {
        set_phase(run, "codebase_recon", json!({}));
        Ok(())
    })?;
    if bootstrap_checkpoint_requested(state, user_id, run_id)? {
        return Ok(());
    }
    let codebase_map = perform_codebase_recon(state, user_id, run_id).await?;
    mutate_stored_run(state, user_id, run_id, |run| {
        run.codebase_map = Some(codebase_map.clone());
        run.environment_state = Some(environment_from_codebase_map(&codebase_map));
        run.bootstrap_complete = true;
        set_phase(run, "task_execution", json!({ "bootstrapComplete": true }));
        run.append_log("Agentic bootstrap complete");
        Ok(())
    })?;
    Ok(())
}

fn bootstrap_checkpoint_requested(state: &AppState, user_id: &str, run_id: &str) -> Result<bool> {
    Ok(bootstrap_should_yield(
        &load_user_run(state, user_id, run_id)?.run,
    ))
}

fn bootstrap_should_yield(run: &BoardRun) -> bool {
    run.pause_requested
        || matches!(
            run.status.as_str(),
            "pausing" | "paused" | "cancelled" | "failed" | "blocked" | "completed"
        )
}

async fn extract_requirements_for_run(
    state: &AppState,
    user_id: &str,
    run_id: &str,
) -> Result<Vec<Value>> {
    let stored = load_user_run(state, user_id, run_id)?;
    let prompt = build_requirement_extraction_prompt(&stored.run);
    let output =
        execute_internal_prompt(state, user_id, run_id, "requirement extraction", &prompt).await;
    let parsed = output
        .as_ref()
        .ok()
        .and_then(|text| parse_json_object(text))
        .unwrap_or_else(|| json!({}));
    let mut requirements = sanitize_requirements(&parsed);
    if requirements.is_empty() {
        requirements = fallback_requirements(&stored.run);
    }
    Ok(requirements)
}

async fn perform_codebase_recon(state: &AppState, user_id: &str, run_id: &str) -> Result<Value> {
    let stored = load_user_run(state, user_id, run_id)?;
    let local_snapshot = local_codebase_snapshot(&stored.run);
    let prompt = build_codebase_recon_prompt(&stored.run, &local_snapshot);
    let parsed = execute_internal_prompt(state, user_id, run_id, "codebase recon", &prompt)
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

async fn plan_agentic_tasks(state: &AppState, user_id: &str, run_id: &str) -> Result<()> {
    let stored = load_user_run(state, user_id, run_id)?;
    let prompt = build_planning_prompt(&stored.run);
    let parsed = execute_internal_prompt(state, user_id, run_id, "task planning", &prompt)
        .await
        .ok()
        .and_then(|text| parse_json_object(&text))
        .unwrap_or_else(|| json!({}));
    let mut stored = load_user_run(state, user_id, run_id)?;
    let planned = sanitize_planned_tasks(&stored.run, &parsed);
    if planned.is_empty() {
        stored
            .run
            .append_log("Task planning returned no usable tasks; keeping existing pending card(s)");
        mark_existing_prompt_task_planned(&mut stored.run);
    } else if has_only_seed_prompt_task(&stored.run) {
        stored.run.tasks = planned;
    } else {
        let mut existing_titles = stored
            .run
            .tasks
            .iter()
            .map(|task| task.title.to_lowercase())
            .collect::<BTreeSet<_>>();
        for task in planned {
            if existing_titles.insert(task.title.to_lowercase()) {
                stored.run.tasks.push(task);
            }
        }
    }
    for task in &mut stored.run.tasks {
        if task.status == "planned" {
            task.status = "pending".to_string();
        }
    }
    stored.run.planning_round = stored.run.planning_round.saturating_add(1);
    stored.run.matrix_gap_review_complete = false;
    stored.run.final_matrix_qa_complete = false;
    let task_count = stored.run.tasks.len();
    set_phase(
        &mut stored.run,
        "task_execution",
        json!({ "taskCount": task_count }),
    );
    stored.run.append_log(format!(
        "Kanban queue contains {} task(s)",
        stored.run.tasks.len()
    ));
    stored.run.touch();
    save_run(state, &stored.run)
}

async fn review_requirement_gaps(state: &AppState, user_id: &str, run_id: &str) -> Result<()> {
    let stored = load_user_run(state, user_id, run_id)?;
    let prompt = build_gap_review_prompt(&stored.run);
    let parsed = execute_internal_prompt(state, user_id, run_id, "requirement gap review", &prompt)
        .await
        .ok()
        .and_then(|text| parse_json_object(&text))
        .unwrap_or_else(|| json!({}));
    let mut stored = load_user_run(state, user_id, run_id)?;
    apply_requirement_updates(
        &mut stored.run,
        parsed.get("requirementUpdates"),
        "gap-review",
    );
    let followups = sanitize_followup_tasks(&stored.run, &parsed);
    if !followups.is_empty() {
        stored.run.tasks.extend(followups);
        stored.run.matrix_gap_review_complete = false;
        stored
            .run
            .append_log("Gap review added follow-up Kanban task(s)");
    } else {
        stored.run.matrix_gap_review_complete = true;
        stored
            .run
            .append_log("Requirement gap review completed with no follow-up tasks");
    }
    stored.run.review_round = stored.run.review_round.saturating_add(1);
    let gap_review_complete = stored.run.matrix_gap_review_complete;
    set_phase(
        &mut stored.run,
        "task_execution",
        json!({ "gapReviewComplete": gap_review_complete }),
    );
    stored.run.touch();
    save_run(state, &stored.run)
}

async fn final_agentic_review(state: &AppState, user_id: &str, run_id: &str) -> Result<()> {
    let mut stored = load_user_run(state, user_id, run_id)?;
    let final_validation = run_deterministic_validation(&stored.run, "final-review", "final").await;
    stored.run.validation_runs.push(final_validation);
    stored
        .run
        .append_log("Final deterministic validation completed");
    stored.run.touch();
    save_run(state, &stored.run)?;
    let stored = load_user_run(state, user_id, run_id)?;
    let prompt = build_final_review_prompt(&stored.run);
    let parsed = match execute_internal_prompt(state, user_id, run_id, "final review", &prompt)
        .await
    {
        Ok(text) => parse_json_object(&text).unwrap_or_else(|| {
            json!({
                "status": "needs_followup",
                "summary": "Final review did not return usable JSON.",
                "qaResult": "blocked",
                "evidence": [],
                "remainingGaps": ["Final review result was not machine-readable."],
                "requirementUpdates": [],
                "suggestedBacklogTasks": [],
            })
        }),
        Err(error) => json!({
            "status": "blocked",
            "summary": format!("Final review provider call failed: {}", server_error_message(&error)),
            "qaResult": "blocked",
            "evidence": [],
            "remainingGaps": [server_error_message(&error)],
            "requirementUpdates": [],
            "suggestedBacklogTasks": [],
        }),
    };
    let mut stored = load_user_run(state, user_id, run_id)?;
    apply_requirement_updates(
        &mut stored.run,
        parsed.get("requirementUpdates"),
        "final-review",
    );
    let status = parsed
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("done")
        .to_string();
    let followups = sanitize_followup_tasks(&stored.run, &parsed);
    if !followups.is_empty() || matches!(status.as_str(), "blocked" | "needs_followup") {
        stored.run.tasks.extend(followups);
        stored.run.final_matrix_qa_complete = false;
        stored.run.matrix_gap_review_complete = false;
        stored.run.status = if has_runnable_tasks(&stored.run) {
            "running".to_string()
        } else {
            "blocked".to_string()
        };
        stored.run.final_review = Some(parsed);
        stored.run.append_log("Final review found remaining work");
        let status_for_retry = stored.run.status.clone();
        schedule_auto_retry_if_eligible(&mut stored.run, &status_for_retry);
    } else {
        stored.run.final_matrix_qa_complete = true;
        stored.run.final_review = Some(parsed);
        stored.run.append_log("Final review passed");
    }
    stored.run.review_round = stored.run.review_round.saturating_add(1);
    let final_review_complete = stored.run.final_matrix_qa_complete;
    set_phase(
        &mut stored.run,
        "task_execution",
        json!({ "finalReviewComplete": final_review_complete }),
    );
    stored.run.touch();
    save_run(state, &stored.run)
}

fn should_plan_tasks(run: &BoardRun) -> bool {
    if run.planning_round == 0 && has_only_seed_prompt_task(run) {
        return true;
    }
    !has_runnable_tasks(run)
        && run.tasks.is_empty()
        && run.requirement_matrix.iter().any(|requirement| {
            matches!(
                requirement.get("status").and_then(Value::as_str),
                Some("extracted" | "planned" | "in_progress")
            )
        })
}

fn requirements_needing_implementation(run: &BoardRun) -> Vec<Value> {
    run.requirement_matrix
        .iter()
        .filter(|requirement| requirement_needs_implementation(requirement))
        .cloned()
        .collect()
}

fn requirement_needs_implementation(requirement: &Value) -> bool {
    let status = requirement
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("");
    !matches!(
        status,
        "verified" | "blocked" | "deferred" | "non_actionable"
    ) && !matches!(status, "implemented" | "already_implemented")
}

fn all_requirements_satisfied(run: &BoardRun) -> bool {
    !run.requirement_matrix.is_empty()
        && run.requirement_matrix.iter().all(|requirement| {
            matches!(
                requirement.get("status").and_then(Value::as_str),
                Some("verified" | "non_actionable")
            )
        })
}

fn unsatisfied_requirement_summary(run: &BoardRun) -> String {
    let summary = run
        .requirement_matrix
        .iter()
        .filter(|requirement| {
            !matches!(
                requirement.get("status").and_then(Value::as_str),
                Some("verified" | "non_actionable")
            )
        })
        .filter_map(|requirement| {
            let id = requirement.get("id").and_then(Value::as_str)?;
            let status = requirement
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            Some(format!("{id}:{status}"))
        })
        .collect::<Vec<_>>()
        .join(", ");
    if summary.is_empty() {
        "no requirements verified".to_string()
    } else {
        summary
    }
}

fn requirement_integrity_issues(run: &BoardRun) -> Vec<String> {
    if !uses_simplified_orchestration(run) || run.requirement_baseline.is_empty() {
        return Vec::new();
    }
    let mut issues = Vec::new();
    for original in &run.requirement_baseline {
        let id = original.get("id").and_then(Value::as_str).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        let Some(current) = run
            .requirement_matrix
            .iter()
            .find(|requirement| requirement.get("id").and_then(Value::as_str) == Some(id))
        else {
            issues.push(format!("{id} is missing from the requirement matrix."));
            continue;
        };
        if current
            .get("requirement")
            .and_then(Value::as_str)
            .unwrap_or("")
            != original
                .get("requirement")
                .and_then(Value::as_str)
                .unwrap_or("")
        {
            issues.push(format!("{id} text changed after planning."));
        }
        if normalize_string_list(current.get("acceptanceCriteria"))
            != normalize_string_list(original.get("acceptanceCriteria"))
        {
            issues.push(format!("{id} acceptance criteria changed after planning."));
        }
    }
    issues
}

fn build_v2_fallback_feature_queue(run: &BoardRun, requirements: &[Value]) -> Vec<BoardTask> {
    let actionable = requirements
        .iter()
        .filter(|requirement| {
            requirement.get("status").and_then(Value::as_str) != Some("non_actionable")
        })
        .cloned()
        .collect::<Vec<_>>();
    let product_requirements = actionable
        .iter()
        .filter(|requirement| !is_v2_support_requirement(requirement))
        .take(8)
        .cloned()
        .collect::<Vec<_>>();
    let support_ids = actionable
        .iter()
        .filter(|requirement| is_v2_support_requirement(requirement))
        .filter_map(|requirement| {
            requirement
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    let source = if !product_requirements.is_empty() {
        product_requirements
    } else if !actionable.is_empty() {
        actionable.into_iter().take(8).collect()
    } else {
        vec![json!({
            "id": "REQ-0001",
            "heading": "Requested feature",
            "requirement": limit_text(&run.source_prompt, 180),
            "acceptanceCriteria": ["The requested behavior works and is validated locally."],
            "priority": "medium",
            "sourcePath": "User prompt",
        })]
    };

    source
        .into_iter()
        .enumerate()
        .map(|(index, requirement)| {
            let requirement_id = requirement
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("REQ-0001")
                .to_string();
            let heading = requirement
                .get("heading")
                .and_then(Value::as_str)
                .unwrap_or("Feature");
            let requirement_text = requirement
                .get("requirement")
                .and_then(Value::as_str)
                .unwrap_or("Implement the attached requirement.");
            let title = specific_task_title(heading, requirement_text)
                .unwrap_or_else(|| format!("Implement feature {}", index + 1));
            let details = format!(
                "Implement uncovered requirement {requirement_id}.\n\nRequirement: {requirement_text}"
            );
            let mut requirement_ids = vec![requirement_id.clone()];
            for support_id in &support_ids {
                if !requirement_ids.contains(support_id) {
                    requirement_ids.push(support_id.clone());
                }
            }
            BoardTask {
                id: unique_task_id(run, &format!("feature-repair-{}", index + 1)),
                title,
                status: "pending".to_string(),
                summary: String::new(),
                details: details.clone(),
                description: details.clone(),
                prompt: details,
                error: None,
                acceptance_criteria: normalize_string_list(requirement.get("acceptanceCriteria"))
                    .into_iter()
                    .chain(std::iter::once(
                        "Implement the attached requirement and verify its observable behavior locally."
                            .to_string(),
                    ))
                    .collect::<Vec<_>>(),
                references: vec![format!(
                    "{}: {}",
                    requirement_id,
                    requirement
                        .get("sourcePath")
                        .and_then(Value::as_str)
                        .unwrap_or("User prompt")
                )],
                requirement_ids,
                priority: normalize_priority(requirement.get("priority").and_then(Value::as_str))
                    .to_string(),
                depends_on: Vec::new(),
                manual_task: false,
                prompt_task: false,
                task_origin: "system_v2_coverage_fallback".to_string(),
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
                group_id: Some(format!("feature-repair-{}", index + 1)),
            }
        })
        .collect()
}

fn mark_requirements_from_tasks(run: &mut BoardRun, tasks: &[BoardTask]) {
    let updates = tasks
        .iter()
        .flat_map(|task| {
            task.requirement_ids.iter().map(|id| {
                json!({
                    "id": id,
                    "status": "planned",
                    "evidence": [format!("Attached to Kanban task {}.", task.id)],
                })
            })
        })
        .collect::<Vec<_>>();
    apply_requirement_updates(run, Some(&Value::Array(updates)), "task-planning");
}

fn normalize_suggested_task_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn append_suggested_backlog_tasks_from_result(
    run: &mut BoardRun,
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
    let mut existing_keys = run
        .tasks
        .iter()
        .map(|task| normalize_suggested_task_key(&task.title))
        .filter(|key| !key.is_empty())
        .collect::<BTreeSet<_>>();
    let mut created = Vec::new();
    for suggestion in suggestions {
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
        if key.is_empty() || !existing_keys.insert(key) {
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
                acceptance_criteria: suggestion.get("acceptanceCriteria").cloned(),
                acceptance: suggestion.get("acceptance").cloned(),
                criteria: suggestion.get("criteria").cloned(),
                references: suggestion.get("references").cloned(),
                files: suggestion.get("files").cloned(),
                paths: suggestion.get("paths").cloned(),
                requirement_ids: suggestion.get("requirementIds").cloned(),
                requirements: suggestion.get("requirements").cloned(),
                priority: suggestion
                    .get("priority")
                    .and_then(Value::as_str)
                    .map(str::to_string),
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

fn ensure_manual_task_requirements(requirement_matrix: &mut Vec<Value>, task: &mut BoardTask) {
    if task.requirement_ids.is_empty() {
        task.requirement_ids = vec![next_manual_requirement_id(requirement_matrix)];
    }
    let now = Utc::now();
    for requirement_id in task.requirement_ids.clone() {
        if requirement_matrix.iter().any(|requirement| {
            requirement.get("id").and_then(Value::as_str) == Some(&requirement_id)
        }) {
            continue;
        }
        let acceptance_criteria = if task.acceptance_criteria.is_empty() {
            vec!["Complete the added task and verify it locally.".to_string()]
        } else {
            task.acceptance_criteria.clone()
        };
        let heading = match canonical_task_origin(&task.task_origin) {
            "user_prompt_generated" => "Prompt-added task",
            "ai_suggested_backlog" => "Suggested backlog task",
            _ => "Manual task",
        };
        requirement_matrix.push(json!({
            "id": requirement_id,
            "sourceChunkId": "",
            "sourcePath": "Agentic workspace",
            "heading": heading,
            "requirement": if task.prompt.trim().is_empty() { &task.title } else { &task.prompt },
            "acceptanceCriteria": acceptance_criteria,
            "priority": "medium",
            "dependencies": [],
            "status": "extracted",
            "evidence": [format!("Created from {}: {}", task.id, task.title)],
            "plannedBy": [],
            "implementedBy": [],
            "verifiedBy": [],
            "blockedReason": "",
            "notes": "",
            "createdAt": now,
            "updatedAt": now,
        }));
    }
}

fn next_manual_requirement_id(requirement_matrix: &[Value]) -> String {
    let existing = requirement_matrix
        .iter()
        .filter_map(|requirement| requirement.get("id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    (1..)
        .map(|index| format!("REQ-MANUAL-{index}"))
        .find(|candidate| !existing.contains(candidate.as_str()))
        .unwrap_or_else(|| format!("REQ-MANUAL-{}", Uuid::new_v4()))
}

fn reset_requirement_review_state(run: &mut BoardRun) {
    run.matrix_gap_review_complete = false;
    run.final_matrix_qa_complete = false;
    run.final_review = None;
    run.agents_knowledge_updated = false;
}

fn unique_task_id(run: &BoardRun, base: &str) -> String {
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

fn specific_task_title(heading: &str, requirement: &str) -> Option<String> {
    let candidate = [heading, requirement]
        .into_iter()
        .map(str::trim)
        .find(|value| !value.is_empty() && !value.eq_ignore_ascii_case("requirement"))?;
    Some(limit_text(candidate, 96).replace('\n', " "))
}

fn has_runnable_tasks(run: &BoardRun) -> bool {
    run.tasks
        .iter()
        .any(|task| matches!(task.status.as_str(), "pending" | "planned"))
}

fn has_only_seed_prompt_task(run: &BoardRun) -> bool {
    run.tasks.len() == 1
        && run.tasks[0].id == "task-1"
        && run.tasks[0].prompt.trim() == run.source_prompt.trim()
        && matches!(
            run.tasks[0].status.as_str(),
            "pending" | "planned" | "backlog"
        )
}

fn mark_existing_prompt_task_planned(run: &mut BoardRun) {
    let req_ids = run
        .requirement_matrix
        .iter()
        .filter_map(|value| value.get("id").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    for task in &mut run.tasks {
        if task.requirement_ids.is_empty() {
            task.requirement_ids = req_ids.clone();
        }
        if task.acceptance_criteria.is_empty() {
            task.acceptance_criteria =
                vec!["Complete the user request and verify the changed behavior.".to_string()];
        }
        if task.status == "backlog" || task.status == "planned" {
            task.status = "pending".to_string();
        }
    }
    apply_requirement_updates(
        run,
        Some(&json!(
            req_ids
                .into_iter()
                .map(|id| json!({
                    "id": id,
                    "status": "planned",
                    "evidence": ["Attached to the seed Kanban task after planning fallback."]
                }))
                .collect::<Vec<_>>()
        )),
        "planning-fallback",
    );
}

fn append_final_qa_task(run: &mut BoardRun, reason: &str) -> bool {
    if run
        .tasks
        .iter()
        .any(|task| task.final_qa_task && task.status == "pending")
    {
        return false;
    }
    if run.tasks.iter().any(|task| {
        task.final_qa_task && task.status == "completed" && task.qa_passed == Some(true)
    }) {
        return false;
    }
    let requirement_ids = requirements_for_final_qa(run);
    if !run.requirement_matrix.is_empty() && requirement_ids.is_empty() {
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
        status: "pending".to_string(),
        summary: String::new(),
        details: format!(
            "Validate every immutable requirement against current files, deterministic command evidence, and completed task results. Reason: {reason}"
        ),
        description: "Independent final validation".to_string(),
        prompt: "Run final QA validation and return the required JSON verdict.".to_string(),
        error: None,
        acceptance_criteria: vec![
            "Independently validate every immutable prompt/source requirement against current files and deterministic command evidence.".to_string(),
            "Inspect implementation directly; do not trust feature summaries as proof.".to_string(),
            "Return done only when every attached requirement has concrete evidence and deterministic checks pass.".to_string(),
            "Do not edit files during this validation task and do not modify git history.".to_string(),
        ],
        references: vec![
            "Original user prompt".to_string(),
            "Changed files and local verification output".to_string(),
        ],
        requirement_ids,
        priority: "low".to_string(),
        depends_on: Vec::new(),
        manual_task: false,
        prompt_task: false,
        task_origin: "system_final_qa".to_string(),
        task_type: "final_qa".to_string(),
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
    });
    true
}

fn append_promotion_review_task(run: &mut BoardRun, reason: &str) -> bool {
    if run.promotion_candidates.is_empty() {
        return false;
    }
    if run.tasks.iter().any(|task| is_promotion_review_task(task)) {
        return false;
    }
    run.tasks.push(BoardTask {
        id: PROMOTION_REVIEW_TASK_ID.to_string(),
        title: "Review RAG promotion candidates".to_string(),
        status: "pending".to_string(),
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
        requirement_ids: Vec::new(),
        priority: "low".to_string(),
        depends_on: Vec::new(),
        manual_task: false,
        prompt_task: false,
        task_origin: "system_promotion".to_string(),
        task_type: "promotion".to_string(),
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
    });
    true
}

fn is_promotion_review_task(task: &BoardTask) -> bool {
    task.task_type == "promotion" || task.id == PROMOTION_REVIEW_TASK_ID
}

fn has_passing_final_qa_task(run: &BoardRun) -> bool {
    if !run.requirement_matrix.is_empty() {
        return all_requirements_satisfied(run) && final_qa_covers_all_requirements(run);
    }
    run.tasks.iter().any(|task| {
        task.final_qa_task && task.status == "completed" && task.qa_passed != Some(false)
    })
}

fn final_qa_covers_all_requirements(run: &BoardRun) -> bool {
    let required = run
        .requirement_matrix
        .iter()
        .filter_map(|requirement| {
            requirement
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<BTreeSet<_>>();
    if required.is_empty() {
        return false;
    }
    let covered = run
        .tasks
        .iter()
        .filter(|task| {
            task.final_qa_task && task.status == "completed" && task.qa_passed != Some(false)
        })
        .flat_map(|task| task.requirement_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    required.is_subset(&covered)
}

fn append_agents_knowledge_task(
    run: &mut BoardRun,
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
        let has_completed_implementation = run.tasks.iter().any(|task| {
            task.status == "completed" && !is_qa_task(task) && !task.agents_knowledge_task
        });
        if !has_completed_implementation && run.requirement_matrix.is_empty() {
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

fn has_task_qa_for_source(run: &BoardRun, source_task_id: &str) -> bool {
    run.tasks
        .iter()
        .any(|task| task.task_level_qa && task.source_task_id.as_deref() == Some(source_task_id))
}

fn append_task_qa_task(run: &mut BoardRun, source_task: &BoardTask, reason: &str) -> bool {
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

fn create_task_qa_task(run: &BoardRun, source_task: &BoardTask, reason: &str) -> BoardTask {
    let title_seed = if source_task.title.trim().is_empty() {
        limit_text(&run.source_prompt, 180)
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
    BoardTask {
        id: unique_task_id(run, "task-qa"),
        title: format!("QA validate {}", limit_text(&title_seed, 120).replace('\n', " ")),
        status: "pending".to_string(),
        summary: String::new(),
        details: details.clone(),
        description: details.clone(),
        prompt: details,
        error: None,
        acceptance_criteria: vec![
            "Validate the source task against its acceptance criteria, source references, attached requirements, and changed files.".to_string(),
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
        requirement_ids: source_task.requirement_ids.clone(),
        priority: source_task.priority.clone(),
        depends_on: vec![source_task.id.clone()],
        manual_task: false,
        prompt_task: false,
        task_origin: "system_qa".to_string(),
        task_type: "qa".to_string(),
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
        group_id: source_task.group_id.clone(),
    }
}

fn task_needs_immediate_ai_qa(task: &BoardTask, parsed: &Value) -> bool {
    if is_qa_task(task) || task.agents_knowledge_task || task.id == AGENTS_KNOWLEDGE_TASK_ID {
        return false;
    }
    if task.priority == "high" || task.qa_fix_task || task.followup_task {
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

fn has_agents_knowledge_task_for_source(run: &BoardRun, source_task_id: &str) -> bool {
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
    run: &BoardRun,
    reason: &str,
    source_task: Option<&BoardTask>,
) -> BoardTask {
    let source_task_id = source_task.map(|task| task.id.clone());
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
        "Update AGENTS.md with stable project knowledge from this agentic run".to_string()
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
    BoardTask {
        id,
        title,
        status: "pending".to_string(),
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
        requirement_ids: Vec::new(),
        priority: "low".to_string(),
        depends_on: source_task_id.iter().cloned().collect(),
        manual_task: false,
        prompt_task: false,
        task_origin: "system_agents".to_string(),
        task_type: "agents_knowledge".to_string(),
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
        group_id: Some(AGENTS_KNOWLEDGE_TASK_ID.to_string()),
    }
}

fn requirements_for_final_qa(run: &BoardRun) -> Vec<String> {
    run.requirement_matrix
        .iter()
        .filter(|requirement| {
            !matches!(
                requirement.get("status").and_then(Value::as_str),
                Some("verified" | "blocked" | "deferred" | "non_actionable")
            )
        })
        .filter_map(|requirement| {
            requirement
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

fn is_qa_task(task: &BoardTask) -> bool {
    task.qa_task || task.final_qa_task || task.id == FINAL_QA_TASK_ID
}

fn is_qa_task_id(run: &BoardRun, task_id: &str) -> bool {
    run.tasks
        .iter()
        .find(|task| task.id == task_id)
        .map(is_qa_task)
        .unwrap_or(false)
}

fn is_qa_verdict_retry_task_id(run: &BoardRun, task_id: &str) -> bool {
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

fn resolve_derived_requirement_ids(source_task: &BoardTask, parsed: &Value) -> Vec<String> {
    if source_task.requirement_ids.is_empty() {
        return Vec::new();
    }
    let source_ids = source_task
        .requirement_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let explicit_ids = parsed
        .get("requirementUpdates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|update| update.get("id").and_then(Value::as_str))
        .filter(|id| source_ids.contains(id))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let explicit_ids = dedupe_strings(explicit_ids);
    if !explicit_ids.is_empty() {
        return explicit_ids;
    }
    let covered_ids = normalize_string_list(parsed.get("coveredRequirements"))
        .into_iter()
        .filter(|id| source_ids.contains(id.as_str()))
        .collect::<Vec<_>>();
    let covered_ids = dedupe_strings(covered_ids);
    if !covered_ids.is_empty() {
        return covered_ids;
    }
    source_task.requirement_ids.clone()
}

fn should_queue_qa_verdict_retry(
    run: &BoardRun,
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

fn queue_qa_verdict_retry(run: &mut BoardRun, task_id: &str, parsed: &Value) -> bool {
    let Some(source_index) = run.tasks.iter().position(|task| task.id == task_id) else {
        return false;
    };
    let source_task = run.tasks[source_index].clone();
    if let Some(task) = run.tasks.get_mut(source_index) {
        task.status = "completed".to_string();
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
    let retry = BoardTask {
        id: id.clone(),
        title: format!("QA verdict retry: {}", source_task.title),
        status: "pending".to_string(),
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
        requirement_ids: resolve_derived_requirement_ids(&source_task, parsed),
        priority: source_task.priority.clone(),
        depends_on: vec![task_id.to_string()],
        manual_task: false,
        prompt_task: false,
        task_origin: "system_qa_verdict_retry".to_string(),
        task_type: "qa".to_string(),
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
        group_id: source_task.group_id.clone(),
    };
    run.tasks.insert(source_index + 1, retry);
    run.append_log(format!(
        "QA JSON contract missing for {task_id}; queued compact verdict retry {id}"
    ));
    true
}

fn mark_qa_verdict_retry_blocked(run: &mut BoardRun, task_id: &str, parsed: &Value) {
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
    run: &mut BoardRun,
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
    let group_id = source_task
        .group_id
        .clone()
        .unwrap_or_else(|| source_task.id.clone());
    let existing_followups = run
        .tasks
        .iter()
        .filter(|task| task.followup_task && task.group_id.as_deref() == Some(&group_id))
        .count();
    if existing_followups >= MAX_FOLLOWUP_TASKS_PER_GROUP {
        if let Some(task) = run.tasks.iter_mut().find(|task| task.id == source_task_id) {
            task.status = "blocked".to_string();
            task.error = Some(format!(
                "Follow-up limit reached for {group_id} ({MAX_FOLLOWUP_TASKS_PER_GROUP})."
            ));
        }
        run.append_log(format!(
            "Task follow-up limit reached for {source_task_id}; marked blocked"
        ));
        return false;
    }
    let max_fix_attempts = max_tdd_fix_attempts(run);
    if !source_task.qa_test_commands.is_empty() && source_task.fix_attempts >= max_fix_attempts {
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
    let qa_fix = is_qa_task(&source_task);
    let title = if qa_fix {
        format!(
            "Fix QA findings for: {}",
            limit_text(&run.source_prompt, 180).replace('\n', " ")
        )
    } else {
        format!("Continue follow-up: {}", source_task.title)
    };
    let issues = normalize_string_list(parsed.get("remainingIssues"))
        .into_iter()
        .chain(normalize_string_list(parsed.get("remainingGaps")))
        .collect::<Vec<_>>();
    let details = [
        format!("Source task: {} - {}", source_task.id, source_task.title),
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
        status: "pending".to_string(),
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
        requirement_ids: resolve_derived_requirement_ids(&source_task, parsed),
        priority: if qa_fix {
            "high".to_string()
        } else {
            source_task.priority.clone()
        },
        depends_on: vec![source_task_id.to_string()],
        manual_task: false,
        prompt_task: false,
        task_origin: if qa_fix {
            "system_qa_fix".to_string()
        } else {
            "system_followup".to_string()
        },
        task_type: if qa_fix {
            "qa_fix".to_string()
        } else {
            "implementation".to_string()
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
        group_id: Some(group_id),
    };
    if qa_fix {
        reset_requirement_review_state(run);
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
    run: &BoardRun,
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

fn pick_next_task_index(run: &BoardRun) -> Option<usize> {
    let mut ready = Vec::<(usize, u8)>::new();
    let mut pending_only_waiting = Vec::<usize>::new();
    for (index, task) in run.tasks.iter().enumerate() {
        if !matches!(task.status.as_str(), "pending" | "planned") {
            continue;
        }
        let unmet = unmet_task_dependencies(run, task);
        if unmet.is_empty() {
            ready.push((index, task_priority_rank(&task.priority)));
        } else if unmet.iter().all(|id| {
            run.tasks
                .iter()
                .find(|candidate| candidate.id == *id)
                .is_some_and(|candidate| matches!(candidate.status.as_str(), "pending" | "planned"))
        }) {
            pending_only_waiting.push(index);
        }
    }
    ready
        .into_iter()
        .min_by_key(|(index, rank)| (*rank, *index))
        .map(|(index, _)| index)
        .or_else(|| pending_only_waiting.first().copied())
}

fn unmet_task_dependencies(run: &BoardRun, task: &BoardTask) -> Vec<String> {
    let mut dependencies = task.depends_on.clone();
    if let Some(id) = task
        .source_qa_task_id
        .as_ref()
        .or(task.source_task_id.as_ref())
    {
        if !dependencies.contains(id) {
            dependencies.push(id.clone());
        }
    }
    dependencies
        .into_iter()
        .filter(|id| id != &task.id)
        .filter(|id| {
            run.tasks
                .iter()
                .find(|candidate| candidate.id == *id)
                .is_some_and(|candidate| !matches!(candidate.status.as_str(), "completed" | "done"))
        })
        .collect()
}

fn dependency_waiting_tasks(run: &BoardRun) -> Vec<String> {
    run.tasks
        .iter()
        .filter(|task| matches!(task.status.as_str(), "pending" | "planned"))
        .filter(|task| !unmet_task_dependencies(run, task).is_empty())
        .map(|task| task.id.clone())
        .collect()
}

fn task_priority_rank(priority: &str) -> u8 {
    match normalize_priority(Some(priority)) {
        "high" => 0,
        "medium" => 1,
        "low" => 2,
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
    run_id: &str,
    label: &str,
    prompt: &str,
) -> Result<String> {
    let mut stored = load_user_run(state, user_id, run_id)?;
    stored.run.provider_call_started_at = Some(Utc::now());
    stored.run.provider_call_label = Some(label.to_string());
    stored.run.current_provider_session_id = stored.run.session_id.clone();
    stored.run.prompt_telemetry.push(json!({
        "phase": stored.run.current_phase,
        "label": label,
        "chars": prompt.chars().count(),
        "estimatedTokens": estimate_tokens(prompt),
        "startedAt": Utc::now(),
    }));
    let telemetry_index = stored.run.prompt_telemetry.len().saturating_sub(1);
    stored.run.touch();
    save_run(state, &stored.run)?;

    let result = execute_provider_prompt(state, &stored.run, label, prompt).await;
    let mut stored = load_user_run(state, user_id, run_id)?;
    stored.run.provider_call_started_at = None;
    stored.run.provider_call_label = None;
    stored.run.current_provider_session_id = None;
    match &result {
        Ok(output) => {
            finalize_prompt_telemetry(
                &mut stored.run,
                telemetry_index,
                output.session_id.as_deref(),
                output.effective_model.as_deref(),
                output.token_usage.as_ref(),
            );
            increment_provider_usage(
                &mut stored.run,
                prompt,
                &output.output,
                output.session_id.as_deref(),
                output.token_usage.as_ref(),
            );
            stored
                .run
                .append_log(format!("Internal provider call completed: {label}"));
        }
        Err(error) => {
            stored.run.append_log(format!(
                "Internal provider call failed for {label}: {}",
                server_error_message(error)
            ));
        }
    }
    stored.run.touch();
    save_run(state, &stored.run)?;
    result.map(|output| output.output)
}

async fn execute_provider_prompt(
    state: &AppState,
    run: &BoardRun,
    label: &str,
    prompt: &str,
) -> Result<ProviderPromptResult> {
    let provider = normalize_provider(Some(&run.provider))?;
    let model = effective_model_for_phase(run, label);
    let result = execute_shared_provider_turn(
        state,
        run,
        &provider,
        &model,
        prompt,
        reusable_session_id(run).as_deref(),
        board_task_id_for_label(run, label).as_deref(),
    )
    .await?;
    if result.exit_code == 0 {
        return Ok(ProviderPromptResult {
            output: result.assistant_text,
            session_id: Some(result.session_id),
            token_usage: result.token_usage,
            effective_model: if model.trim().is_empty() {
                None
            } else {
                Some(model)
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
    run: &mut BoardRun,
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
    let mut references = Vec::new();
    let mut seen = BTreeSet::<PathBuf>::new();
    for token in prompt.split_whitespace() {
        let cleaned = token
            .trim_matches(|ch: char| {
                matches!(
                    ch,
                    '"' | '\'' | '`' | ',' | ':' | ';' | ')' | '(' | '[' | ']'
                )
            })
            .trim();
        if cleaned.len() < 2 || cleaned.starts_with("http://") || cleaned.starts_with("https://") {
            continue;
        }
        if !(cleaned.contains('/') || cleaned.contains('.') || cleaned == "AGENTS.md") {
            continue;
        }
        let candidate = if Path::new(cleaned).is_absolute() {
            PathBuf::from(cleaned)
        } else {
            root.join(cleaned)
        };
        if candidate.exists() && candidate.starts_with(root) && seen.insert(candidate.clone()) {
            references.push(json!({
                "matchedFrom": cleaned,
                "path": relative_display(root, &candidate),
                "absolutePath": candidate,
                "reason": "prompt-reference",
            }));
        }
    }
    references
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
    run_id: &str,
    run: &mut BoardRun,
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
    if !run.requirement_matrix.is_empty() && task.requirement_ids.is_empty() {
        if let Some(task) = run.tasks.get_mut(task_index) {
            task.status = "blocked".to_string();
            task.tdd_phase = "blocked".to_string();
            task.error = Some(
                "TDD policy requires implementation tasks to map to at least one requirement."
                    .to_string(),
            );
            task.completed_at = Some(Utc::now());
        }
        run.append_log(format!(
            "Blocked {} because it has no requirement mapping for TDD",
            task.id
        ));
        return Ok(false);
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
    save_run(state, run)?;
    if let Ok(stored) = load_user_run(state, user_id, run_id)
        && (stored.run.status == "paused"
            || stored.run.pause_requested
            || stored.run.status == "cancelled")
    {
        *run = stored.run;
        return Ok(false);
    }

    let prompt = build_qa_generation_prompt(run, &task, task_index);
    let before_workspace = capture_workspace_snapshot(&run.project_path);
    let output = execute_internal_prompt(
        state,
        user_id,
        run_id,
        &format!("tdd qa generation for {}", task.id),
        &prompt,
    )
    .await;

    if let Ok(stored) = load_user_run(state, user_id, run_id) {
        *run = stored.run;
    }
    let task_index = run
        .tasks
        .iter()
        .position(|candidate| candidate.id == task.id)
        .unwrap_or(task_index);
    let now = Utc::now();
    let parsed = match output {
        Ok(text) => parse_json_object(&text).unwrap_or_else(|| {
            json!({
                "status": "blocked",
                "summary": "QA generation did not return the required JSON contract.",
                "testFiles": [],
                "commands": [],
                "notes": [limit_text(&text, 1200)],
            })
        }),
        Err(error) => json!({
            "status": "blocked",
            "summary": server_error_message(&error),
            "testFiles": [],
            "commands": [],
            "notes": [],
        }),
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
    let baseline = if commands.is_empty() {
        json!({
            "stage": "qa_baseline",
            "taskId": task.id,
            "startedAt": now,
            "completedAt": Utc::now(),
            "passed": false,
            "commands": [],
            "blocked": true,
            "summary": "QA generation returned no test commands.",
        })
    } else {
        run_generated_test_commands(&run.project_path, &task.id, &commands, "qa_baseline").await
    };
    let qa_generation_done = parsed_status_done(Some(&parsed));
    let baseline_failed = qa_generation_done && validation_has_failure(&baseline);
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
        if baseline_failed {
            task.status = "in_progress".to_string();
            task.tdd_phase = "qa_failed_expected".to_string();
            task.error = None;
            true
        } else {
            task.status = "blocked".to_string();
            task.tdd_phase = "qa_needs_review".to_string();
            task.qa_passed = Some(false);
            task.error = Some(
                "Generated QA tests did not fail before implementation; tests may be weak or feature already exists."
                    .to_string(),
            );
            task.completed_at = Some(Utc::now());
            false
        }
    } else {
        false
    };
    if outcome {
        run.append_log(format!(
            "TDD baseline failed as expected for {task_id_for_log}; implementation may start"
        ));
    } else {
        run.append_log(format!(
            "Blocked {task_id_for_log} because generated QA tests passed before implementation"
        ));
    }
    Ok(outcome)
}

fn task_requires_tdd(run: &BoardRun, task: &BoardTask) -> bool {
    run.tdd_enabled
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

fn max_tdd_fix_attempts(run: &BoardRun) -> u32 {
    run.tdd_policy
        .get("maxFixAttempts")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(3)
}

async fn index_project_for_rag(run: &mut BoardRun) {
    let Some(client) = rag_client_for_run(run) else {
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

async fn attach_rag_context_for_task(run: &mut BoardRun, task_index: usize) {
    let Some(task) = run.tasks.get(task_index).cloned() else {
        return;
    };
    let Some(client) = rag_client_for_run(run) else {
        return;
    };
    let phase = rag_phase_for_task(&task);
    let project_id = rag_project_id(run);
    record_rag_trace_ref(run, Some(&task.id), "query", &project_id);
    let request = RagQueryRequest {
        project_id,
        run_id: run.id.clone(),
        task_id: task.id.clone(),
        phase,
        query: rag_task_query(&task),
        requirements: task_requirement_values(run, &task),
        known_files: rag_known_files(&task),
        validation_error: task.deterministic_validation.clone(),
        scopes: vec![
            "global_standard".to_string(),
            "project_specific".to_string(),
            "validation_error".to_string(),
        ],
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
                task.rag_prompt_context = limit_text(&response.prompt_context, 12_000);
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

async fn ingest_rag_task_outcome(run: &mut BoardRun, task_id: &str, parsed: &Value) {
    let Some(client) = rag_client_for_run(run) else {
        return;
    };
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id).cloned() else {
        return;
    };
    let project_id = rag_project_id(run);
    if parsed_status_done(Some(parsed)) {
        record_rag_trace_ref(run, Some(&task.id), "task_result", &project_id);
        let request = TaskResultIngestRequest {
            project_id: project_id.clone(),
            run_id: run.id.clone(),
            task_id: task.id.clone(),
            requirements: task_requirement_values(run, &task),
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
            "requirements": task_requirement_values(run, &task),
            "changedFiles": task.changed_files.clone(),
            "testFiles": task.qa_test_paths.clone(),
            "commands": task.commands_run.clone(),
            "validation": task.deterministic_validation.clone(),
            "summary": task.summary.clone(),
            "recordedAt": Utc::now(),
        }));
    }

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
    trim_rag_history(run);
}

async fn execute_promotion_review_task(
    state: &AppState,
    user_id: &str,
    run_id: &str,
    run: &mut BoardRun,
    task_index: usize,
) -> Result<()> {
    let Some(task) = run.tasks.get(task_index).cloned() else {
        return Ok(());
    };
    let Some(client) = rag_client_for_run(run) else {
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
    save_run(state, run)?;

    let prompt = build_promotion_review_prompt(run, &candidates);
    let output =
        execute_internal_prompt(state, user_id, run_id, "rag promotion review", &prompt).await;
    if let Ok(stored) = load_user_run(state, user_id, run_id) {
        *run = stored.run;
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
    } else if let Some(client) = rag_client_for_run(run) {
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

fn mark_promotion_review_task(run: &mut BoardRun, task_id: &str, result: Value) {
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

fn rag_client_for_run(run: &mut BoardRun) -> Option<RagClient> {
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

fn rag_project_id(run: &BoardRun) -> String {
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

fn task_requirement_values(run: &BoardRun, task: &BoardTask) -> Vec<Value> {
    let ids = task
        .requirement_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    run.requirement_matrix
        .iter()
        .filter(|requirement| {
            requirement
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| ids.is_empty() || ids.contains(id))
        })
        .cloned()
        .collect()
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
    run: &mut BoardRun,
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

fn trim_rag_history(run: &mut BoardRun) {
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
    run: &BoardRun,
    task_index: usize,
) -> Result<ProviderTaskResult> {
    let task = run
        .tasks
        .get(task_index)
        .ok_or_else(|| not_found("Danger task not found"))?;
    let prompt = build_task_execution_prompt(run, task, task_index);
    let provider = normalize_provider(Some(&run.provider))?;
    let model = effective_model_for_task(run, task);
    let reusable_session = if uses_simplified_orchestration(run) {
        None
    } else {
        reusable_session_id(run)
    };
    let session_id = task
        .provider_session_id
        .as_deref()
        .or(reusable_session.as_deref());
    let result = execute_shared_provider_turn(
        state,
        run,
        &provider,
        &model,
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
    run: &BoardRun,
    task_index: usize,
) -> ProviderExecutionAttempt {
    let primary_result = execute_provider_task(state, run, task_index).await;
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

fn configured_provider_fallback(run: &BoardRun) -> Option<(String, String)> {
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

fn board_provider_controls(run: &BoardRun) -> BoardProviderControls {
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
    run: &BoardRun,
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
        user_id
            .as_deref()
            .and_then(|user_id| agentic_direct_ai_runtime_config(state, user_id, provider))
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
    run: &BoardRun,
    task_id: &str,
    session_id: &str,
) -> Result<()> {
    let Some(user_id) = run.user_id.as_deref() else {
        return Ok(());
    };
    let _guard = board_run_mutation_lock();
    let mut stored = load_user_run(state, user_id, &run.id)?;
    if let Some(task) = stored.run.tasks.iter_mut().find(|task| task.id == task_id) {
        task.provider_session_id = Some(session_id.to_string());
        task.transcript_updated_at = Some(Utc::now());
    }
    stored.run.current_provider_session_id = Some(session_id.to_string());
    stored.run.touch();
    save_run(state, &stored.run)
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
    run: &BoardRun,
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
    run: &BoardRun,
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
                if board_run_interrupted(state, run) {
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
    run: &BoardRun,
    provider: Provider,
    session: SessionSummary,
    model: Option<String>,
) -> Result<SharedProviderTurnResult> {
    loop {
        if board_run_interrupted(state, run) {
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

fn board_run_interrupted(state: &AppState, run: &BoardRun) -> bool {
    load_user_run(state, run.user_id.as_deref().unwrap_or_default(), &run.id)
        .map(|stored| board_run_should_abort_provider(&stored.run))
        .unwrap_or(false)
}

fn board_run_should_abort_provider(run: &BoardRun) -> bool {
    run.status == "cancelled" || run.cancellation_reason.is_some() || run.canceled_at.is_some()
}

fn board_run_has_in_flight_work(run: &BoardRun) -> bool {
    run.current_provider_session_id.is_some() || run.provider_call_started_at.is_some()
}

fn request_board_pause(run: &mut BoardRun, reason: Option<String>) {
    bump_control_revision(run);
    run.auto_run_enabled = false;
    run.pause_reason = reason.or_else(|| Some("user request".to_string()));
    if board_run_has_in_flight_work(run) {
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

fn prepare_board_resume(run: &mut BoardRun) {
    bump_control_revision(run);
    run.status = "running".to_string();
    run.scheduled_start_at = None;
    run.active = true;
    run.auto_run_enabled = true;
    run.pause_requested = false;
    run.paused_at = None;
    run.pause_reason = None;
    run.append_log("Board resume requested");
}

fn settle_board_pause(run: &mut BoardRun) {
    if let Some(current_task_id) = run.current_task_id.as_deref()
        && let Some(task) = run.tasks.iter_mut().find(|task| task.id == current_task_id)
        && matches!(task.status.as_str(), "running" | "in_progress")
    {
        task.status = "pending".to_string();
        task.started_at = None;
        task.completed_at = None;
        task.transcript.push(json!({
            "timestamp": Utc::now(),
            "kind": "status",
            "status": "pending",
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

fn reset_in_flight_board_tasks(run: &mut BoardRun, message: &str) {
    for task in &mut run.tasks {
        if !matches!(task.status.as_str(), "running" | "in_progress") {
            continue;
        }
        task.status = "pending".to_string();
        task.started_at = None;
        task.completed_at = None;
        task.transcript.push(json!({
            "timestamp": Utc::now(),
            "kind": "status",
            "status": "pending",
            "content": message,
        }));
        task.transcript_updated_at = Some(Utc::now());
    }
}

fn bump_control_revision(run: &mut BoardRun) {
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
) -> Option<DirectAiRuntimeConfig> {
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

fn build_task_execution_prompt(run: &BoardRun, task: &BoardTask, index: usize) -> String {
    if task.final_qa_task {
        return build_final_qa_prompt(run, task, index);
    }
    if rag_phase_for_task(task) == "fix" {
        return build_fix_prompt(run, task, index);
    }
    build_dev_prompt(run, task, index)
}

fn build_dev_prompt(run: &BoardRun, task: &BoardTask, index: usize) -> String {
    build_execution_prompt_with_mode(
        run,
        task,
        index,
        "Dev",
        "Implement the smallest production change that satisfies the generated tests and acceptance criteria.",
    )
}

fn build_fix_prompt(run: &BoardRun, task: &BoardTask, index: usize) -> String {
    build_execution_prompt_with_mode(
        run,
        task,
        index,
        "Fix",
        "Fix the latest validation failure first; preserve generated tests and use the failure logs as the repair target.",
    )
}

fn build_final_qa_prompt(run: &BoardRun, task: &BoardTask, index: usize) -> String {
    build_execution_prompt_with_mode(
        run,
        task,
        index,
        "Final QA",
        "Validate every requirement against current files, generated-test evidence, and deterministic validation history before returning done.",
    )
}

fn build_execution_prompt_with_mode(
    run: &BoardRun,
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
    format!(
        r#"You are running an autonomous implementation queue in danger mode for an io-workbench Kanban board.

Prompt template: {prompt_mode}
Project: {project_name}
Project path: {project_path}
Board run id: {run_id}
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

Attached requirements:
{requirements}

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
- Implement only this Kanban task and its attached requirements. Do not implement workspace user experience.
- Do not weaken, skip, delete, or rewrite generated TDD tests except to fix broken test syntax or align with existing test harness conventions.
- Run focused local checks when practical.
- Never run git commit, git push, create tags, or otherwise change git history.
- Do not ask for user confirmation.
- Return JSON only, with this schema:
{{
  "status": "done" | "blocked" | "needs_followup",
  "summary": "short summary",
  "changedFiles": ["files changed or inspected as already correct"],
  "coveredRequirements": ["REQ-0001"],
  "commandsRun": ["commands/checks actually run"],
  "qaResult": "pass" | "fail" | "blocked" | "not_run",
  "evidence": ["specific verification or code evidence"],
  "remainingIssues": [],
  "remainingGaps": [],
  "requirementUpdates": [
    {{
      "id": "REQ-0001",
      "status": "implemented" | "already_implemented" | "blocked" | "deferred" | "non_actionable",
      "evidence": ["specific file, command, or inspection evidence"],
      "notes": "",
      "blockedReason": ""
    }}
  ],
  "suggestedBacklogTasks": []
}}
"#,
        project_name = run.project_name,
        project_path = run.project_path,
        run_id = run.id,
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
        requirements = task_requirement_summary(run, task),
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

fn build_qa_generation_prompt(run: &BoardRun, task: &BoardTask, index: usize) -> String {
    let acceptance = if task.acceptance_criteria.is_empty() {
        "- Complete the card exactly as described.".to_string()
    } else {
        task.acceptance_criteria
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        r#"You are the QA phase of a TDD-first io-workbench Kanban runner.

Goal: create failing tests before implementation. Do not implement the feature.

Project: {project_name}
Project path: {project_path}
Board run id: {run_id}
Task {task_number}: {task_id}
Title: {title}

Details:
{details}

Acceptance criteria:
{acceptance}

Attached requirements:
{requirements}

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
{{
  "status": "done" | "blocked",
  "summary": "short QA summary",
  "testFiles": ["test file paths created or updated"],
  "commands": ["focused commands to run these generated tests"],
  "coveredRequirements": ["REQ-0001"],
  "notes": []
}}"#,
        project_name = run.project_name,
        project_path = run.project_path,
        run_id = run.id,
        task_number = index + 1,
        task_id = task.id,
        title = task.title,
        details = if task.details.trim().is_empty() {
            &task.description
        } else {
            &task.details
        },
        acceptance = acceptance,
        requirements = task_requirement_summary(run, task),
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

fn build_promotion_review_prompt(run: &BoardRun, candidates: &[Value]) -> String {
    format!(
        r#"You are reviewing RAG promotion candidates for io-workbench.

Project: {project_name}
Board run id: {run_id}

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
        run_id = run.id,
        candidates = serde_json::to_string_pretty(candidates).unwrap_or_default(),
    )
}

fn build_requirement_extraction_prompt(run: &BoardRun) -> String {
    let source_chunks = run
        .source_chunks
        .iter()
        .take(30)
        .map(|chunk| {
            format!(
                "{} {}\n{}",
                chunk.get("id").and_then(Value::as_str).unwrap_or("SRC"),
                chunk.get("path").and_then(Value::as_str).unwrap_or(""),
                limit_text(
                    chunk.get("content").and_then(Value::as_str).unwrap_or(""),
                    4_000
                ),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n");
    format!(
        r#"You are extracting an immutable requirement matrix for an autonomous Kanban agent.

User request:
{prompt}

Explicit source chunks:
{source_chunks}

Return JSON only. No markdown fence.
Schema:
{{
  "requirements": [
    {{
      "id": "REQ-0001",
      "sourceChunkId": "SRC-0001 or empty",
      "sourcePath": "User prompt or source path",
      "heading": "short heading",
      "requirement": "specific requirement",
      "acceptanceCriteria": ["verifiable criterion"],
      "priority": "high|medium|low",
      "dependencies": [],
      "status": "extracted",
      "evidence": [],
      "plannedBy": [],
      "implementedBy": [],
      "verifiedBy": [],
      "blockedReason": "",
      "notes": ""
    }}
  ]
}}

Rules:
- Extract concrete product/code behavior only.
- Include requirements from the prompt even when no source chunks are provided.
- If a referenced source is missing, create a blocked requirement explaining that source resolution must be fixed.
- Keep each requirement independently verifiable."#,
        prompt = run.source_prompt,
        source_chunks = if source_chunks.is_empty() {
            "None"
        } else {
            &source_chunks
        },
    )
}

fn build_codebase_recon_prompt(run: &BoardRun, local_snapshot: &Value) -> String {
    format!(
        r#"You are performing read-only codebase reconnaissance before Kanban planning.

User request:
{prompt}

Extracted requirements:
{requirements}

Local static snapshot:
{snapshot}

Return JSON only. No markdown fence.
Schema:
{{
  "summary": "short architecture summary",
  "architecture": ["important modules, runtime boundaries, data flow, framework facts"],
  "implementedCapabilities": ["requirements or capabilities that appear already implemented"],
  "missingCapabilities": ["requirements or capabilities that appear missing or partial"],
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
        prompt = run.source_prompt,
        requirements = requirement_summary(run),
        snapshot = serde_json::to_string_pretty(local_snapshot).unwrap_or_default(),
    )
}

fn build_planning_prompt(run: &BoardRun) -> String {
    format!(
        r#"You are preparing an autonomous implementation queue for the io-workbench Kanban board.

User request:
{prompt}

Run profile: {profile}
Git policy: {git_policy}

Requirement coverage:
{requirements}

Codebase reconnaissance:
{codebase}

Return JSON only. No markdown fence.
Schema:
{{
  "summary": "one sentence",
  "tasks": [
    {{
      "id": "task-1",
      "title": "concrete bounded task title",
      "details": "implementation details",
      "acceptanceCriteria": ["specific verifiable outcome"],
      "references": ["relevant source file, requirement ID, or code file"],
      "requirementIds": ["REQ-0001"],
      "priority": "high|medium|low",
      "dependsOn": []
    }}
  ]
}}

Rules:
- Create implementation tasks only; do not create separate workspace UX, planning, review, git, or documentation-maintenance cards.
- Keep dependencies sparse and acyclic.
- Every actionable requirement must appear in at least one task.
- Each task must be small enough to complete in one autonomous provider execution.
- Include checks and QA in acceptance criteria for the feature task itself."#,
        prompt = run.source_prompt,
        profile = run.run_profile,
        git_policy = run.git_policy,
        requirements = requirement_summary(run),
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
    )
}

fn build_gap_review_prompt(run: &BoardRun) -> String {
    format!(
        r#"Review requirement coverage for an autonomous Kanban run.

User request:
{prompt}

Requirements:
{requirements}

Completed task summaries:
{tasks}

Return JSON only. No markdown fence.
Schema:
{{
  "status": "done|needs_followup|blocked",
  "summary": "short review",
  "requirementUpdates": [
    {{
      "id": "REQ-0001",
      "status": "implemented|verified|already_implemented|blocked|deferred|non_actionable",
      "evidence": ["specific evidence"],
      "notes": "",
      "blockedReason": ""
    }}
  ],
  "suggestedBacklogTasks": [
    {{
      "title": "follow-up implementation task",
      "details": "why it is needed",
      "acceptanceCriteria": ["verifiable outcome"],
      "references": ["REQ-0001"],
      "priority": "high|medium|low"
    }}
  ]
}}

Rules:
- Add follow-up tasks only for concrete missing implementation or verification gaps.
- Do not create workspace UX tasks.
- Do not claim coverage without evidence from task results or existing requirement evidence."#,
        prompt = run.source_prompt,
        requirements = requirement_summary(run),
        tasks = completed_task_summary(run),
    )
}

fn build_final_review_prompt(run: &BoardRun) -> String {
    format!(
        r#"Perform the final audit for this agentic Kanban run.

User request:
{prompt}

Requirements:
{requirements}

Task results:
{tasks}

Server-run validation evidence:
{validation}

Codebase map:
{codebase}

Return JSON only. No markdown fence.
Schema:
{{
  "status": "done|needs_followup|blocked",
  "summary": "final result",
  "qaResult": "pass|fail|blocked|not_run",
  "evidence": ["specific checks or code evidence"],
  "remainingGaps": [],
  "requirementUpdates": [],
  "suggestedBacklogTasks": []
}}

Rules:
- Pass only when all runnable requirements are implemented or legitimately non-actionable/deferred/blocked with evidence.
- Add concrete follow-up Kanban tasks if any required implementation remains.
- Do not mention or implement workspace user experience."#,
        prompt = run.source_prompt,
        requirements = requirement_summary(run),
        tasks = task_result_summary(run),
        validation = serde_json::to_string_pretty(&run.validation_runs).unwrap_or_default(),
        codebase = serde_json::to_string_pretty(&run.codebase_map).unwrap_or_default(),
    )
}

fn requirement_summary(run: &BoardRun) -> String {
    if run.requirement_matrix.is_empty() {
        return "No requirement matrix exists yet.".to_string();
    }
    run.requirement_matrix
        .iter()
        .take(120)
        .map(|req| {
            format!(
                "{} [{}] {} ({})",
                req.get("id").and_then(Value::as_str).unwrap_or("REQ"),
                req.get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("extracted"),
                req.get("requirement").and_then(Value::as_str).unwrap_or(""),
                req.get("sourcePath")
                    .and_then(Value::as_str)
                    .unwrap_or("User prompt"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn completed_task_summary(run: &BoardRun) -> String {
    run.tasks
        .iter()
        .filter(|task| task.status == "completed")
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

fn task_result_summary(run: &BoardRun) -> String {
    run.tasks
        .iter()
        .map(|task| {
            format!(
                "{} [{}] {}\nRequirements: {}\nEvidence: {}\nRemaining: {}",
                task.id,
                task.status,
                task.summary,
                task.requirement_ids.join(", "),
                task.evidence.join("; "),
                task.remaining_issues.join("; ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn task_requirement_summary(run: &BoardRun, task: &BoardTask) -> String {
    let ids = task
        .requirement_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let selected = run
        .requirement_matrix
        .iter()
        .filter(|requirement| {
            requirement
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| ids.is_empty() || ids.contains(id))
        })
        .cloned()
        .collect::<Vec<_>>();
    if selected.is_empty() {
        "No attached requirements; use the task details and original user request as scope."
            .to_string()
    } else {
        serde_json::to_string_pretty(&selected).unwrap_or_else(|_| requirement_summary(run))
    }
}

fn sanitize_requirements(parsed: &Value) -> Vec<Value> {
    let items = parsed
        .get("requirements")
        .or_else(|| parsed.get("requirementMatrix"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let requirement = item.get("requirement").and_then(Value::as_str)?.trim();
            if requirement.is_empty() {
                return None;
            }
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("REQ-{:04}", index + 1));
            Some(json!({
                "id": id,
                "sourceChunkId": item.get("sourceChunkId").and_then(Value::as_str).unwrap_or(""),
                "sourcePath": item.get("sourcePath").and_then(Value::as_str).unwrap_or("User prompt"),
                "heading": item.get("heading").and_then(Value::as_str).unwrap_or("Requirement"),
                "requirement": requirement,
                "acceptanceCriteria": normalize_string_list(item.get("acceptanceCriteria")),
                "priority": normalize_priority(item.get("priority").and_then(Value::as_str)),
                "dependencies": normalize_string_list(item.get("dependencies")),
                "status": item.get("status").and_then(Value::as_str).unwrap_or("extracted"),
                "evidence": normalize_string_list(item.get("evidence")),
                "plannedBy": normalize_string_list(item.get("plannedBy")),
                "implementedBy": normalize_string_list(item.get("implementedBy")),
                "verifiedBy": normalize_string_list(item.get("verifiedBy")),
                "blockedReason": item.get("blockedReason").and_then(Value::as_str).unwrap_or(""),
                "notes": item.get("notes").and_then(Value::as_str).unwrap_or(""),
                "extractedAt": Utc::now(),
                "updatedAt": Utc::now(),
            }))
        })
        .collect()
}

fn fallback_requirements(run: &BoardRun) -> Vec<Value> {
    vec![json!({
        "id": "REQ-0001",
        "sourceChunkId": "",
        "sourcePath": "User prompt",
        "heading": "User request",
        "requirement": run.source_prompt,
        "acceptanceCriteria": ["The user request is implemented completely within the Kanban board scope."],
        "priority": "high",
        "dependencies": [],
        "status": "extracted",
        "evidence": [],
        "plannedBy": [],
        "implementedBy": [],
        "verifiedBy": [],
        "blockedReason": "",
        "notes": "Fallback requirement generated because provider extraction returned no usable JSON.",
        "extractedAt": Utc::now(),
        "updatedAt": Utc::now(),
    })]
}

fn sanitize_planned_tasks(run: &BoardRun, parsed: &Value) -> Vec<BoardTask> {
    let tasks = parsed
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| task_from_json(run, item, index, "planned"))
        .collect::<Vec<_>>();
    if uses_simplified_orchestration(run) {
        constrain_v2_feature_tasks(run, tasks, 8)
    } else {
        tasks
    }
}

fn sanitize_followup_tasks(run: &BoardRun, parsed: &Value) -> Vec<BoardTask> {
    parsed
        .get("suggestedBacklogTasks")
        .or_else(|| parsed.get("tasks"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| task_from_json(run, item, run.tasks.len() + index, "pending"))
        .collect()
}

fn task_from_json(run: &BoardRun, item: Value, index: usize, status: &str) -> Option<BoardTask> {
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
    Some(BoardTask {
        id: item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("task-{}", index + 1)),
        title: title.to_string(),
        status: status.to_string(),
        summary: String::new(),
        details: details.clone(),
        description: details.clone(),
        prompt: details,
        error: None,
        acceptance_criteria: normalize_string_list(item.get("acceptanceCriteria")),
        references: normalize_string_list(item.get("references")),
        requirement_ids: normalize_string_list(item.get("requirementIds")),
        priority: normalize_priority(item.get("priority").and_then(Value::as_str)).to_string(),
        depends_on: normalize_string_list(item.get("dependsOn")),
        manual_task: false,
        prompt_task: false,
        task_origin: "planned".to_string(),
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
        group_id: Some(next_task_id(run)),
    })
}

fn constrain_v2_feature_tasks(
    run: &BoardRun,
    tasks: Vec<BoardTask>,
    max_features: usize,
) -> Vec<BoardTask> {
    let allowed_ids = run
        .requirement_matrix
        .iter()
        .filter_map(|requirement| {
            requirement
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<BTreeSet<_>>();
    let mut seen_titles = BTreeSet::new();
    let mut candidates = tasks
        .into_iter()
        .filter(|task| !is_support_only_task(task))
        .filter_map(|mut task| {
            let key = normalize_task_key(&task.title);
            if key.is_empty() || !seen_titles.insert(key) {
                return None;
            }
            task.qa_task = false;
            task.final_qa_task = false;
            task.agents_knowledge_task = false;
            task.task_type = "feature".to_string();
            if !allowed_ids.is_empty() {
                task.requirement_ids.retain(|id| allowed_ids.contains(id));
            }
            Some(task)
        })
        .take(max_features.max(1))
        .collect::<Vec<_>>();

    if candidates.is_empty() {
        candidates =
            build_v2_fallback_feature_queue(run, &requirements_needing_implementation(run));
    }

    let support_ids = run
        .requirement_matrix
        .iter()
        .filter(|requirement| {
            requirement.get("status").and_then(Value::as_str) != Some("non_actionable")
                && is_v2_support_requirement(requirement)
        })
        .filter_map(|requirement| {
            requirement
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    if !support_ids.is_empty() && !candidates.is_empty() {
        for task in &mut candidates {
            for id in &support_ids {
                if !task.requirement_ids.contains(id) {
                    task.requirement_ids.push(id.clone());
                }
            }
        }
    }
    candidates
}

fn is_support_only_task(task: &BoardTask) -> bool {
    is_qa_task(task)
        || task.agents_knowledge_task
        || support_text_match(&format!(
            "{} {} {}",
            task.title,
            task.acceptance_criteria.join(" "),
            task.references.join(" ")
        ))
}

fn is_v2_support_requirement(requirement: &Value) -> bool {
    let heading = requirement
        .get("heading")
        .and_then(Value::as_str)
        .unwrap_or("");
    let text = format!(
        "{} {}",
        heading,
        requirement
            .get("requirement")
            .and_then(Value::as_str)
            .unwrap_or("")
    );
    if product_workflow_match(&text) {
        return false;
    }
    support_text_match(&text) || support_heading_match(heading)
}

fn support_text_match(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "automated test",
        "test suite",
        "test script",
        "build script",
        "lint script",
        "package script",
        "typecheck",
        "validation",
        "horizontal overflow",
        "viewport width",
        "responsive",
        "agents.md",
        "git constraint",
        "git policy",
        "localstorage persistence",
        "storage reload",
        "project setup",
        "scaffold",
        "tooling",
        "configuration",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn support_heading_match(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "test",
        "qa",
        "validation",
        "responsive",
        "layout",
        "persistence",
        "storage",
        "script",
        "setup",
        "scaffold",
        "constraint",
        "git policy",
        "agents.md",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn product_workflow_match(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "add", "edit", "delete", "remove", "toggle", "complete", "search", "filter", "sort",
        "login", "logout", "register", "checkout", "pay", "upload", "download", "export", "import",
        "schedule", "book", "assign", "approve", "reject", "message", "notify", "sync", "track",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn normalize_task_key(value: &str) -> String {
    value
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
        "coveredRequirements": [],
        "commandsRun": [],
        "qaResult": "blocked",
        "evidence": [limit_text(output, 1200)],
        "remainingIssues": ["The task result was not machine-readable. The next attempt must return the required JSON contract."],
        "remainingGaps": ["Missing strict task result JSON."],
        "requirementUpdates": [],
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
    if let Some(updates) = parsed.get("requirementUpdates").and_then(Value::as_array) {
        for update in updates {
            let id = update.get("id").and_then(Value::as_str).unwrap_or("");
            let status = update.get("status").and_then(Value::as_str).unwrap_or("");
            if !id.is_empty() || !status.is_empty() {
                parts.push(format!("{id}:{status}"));
            }
        }
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

fn reset_provider_session(run: &mut BoardRun, reason: &str) {
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
    run_id: &str,
    task_id: &str,
    task_index: usize,
    assistant_output: &str,
    parsed: Value,
    change_summary: &Value,
) -> Value {
    let Ok(stored) = load_user_run(state, user_id, run_id) else {
        return parsed;
    };
    let Some(task) = stored.run.tasks.iter().find(|task| task.id == task_id) else {
        return parsed;
    };
    let issues = strict_result_schema_issues(&stored.run, task, &parsed, change_summary);
    if issues.is_empty() {
        return parsed;
    }
    let prompt = build_task_result_repair_prompt(
        &stored.run,
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
        run_id,
        &format!("result schema repair for {task_id}"),
        &prompt,
    )
    .await
    .ok()
    .and_then(|text| parse_json_object(&text));

    let mut stored = match load_user_run(state, user_id, run_id) {
        Ok(stored) => stored,
        Err(_) => return repaired.unwrap_or(parsed),
    };
    if let Some(index) = stored.run.tasks.iter().position(|task| task.id == task_id) {
        stored.run.tasks[index].result_validation = Some(json!({
            "schemaIssues": issues,
            "repairAttemptedAt": Utc::now(),
            "repaired": repaired.is_some(),
        }));
        stored.run.append_log(format!(
            "Result schema repair {} for {task_id}",
            if repaired.is_some() {
                "succeeded"
            } else {
                "failed"
            }
        ));
        stored.run.touch();
        let _ = save_run(state, &stored.run);
    }
    repaired.unwrap_or(parsed)
}

fn should_repair_task_result(
    run: &BoardRun,
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
    _run: &BoardRun,
    task: &BoardTask,
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
    if !parsed
        .get("requirementUpdates")
        .is_some_and(|value| value.is_array())
    {
        issues.push("Missing requirementUpdates array.".to_string());
    }
    if parsed_status_done(Some(parsed)) {
        let changed_files = normalize_string_list(parsed.get("changedFiles"));
        let commands = normalize_string_list(parsed.get("commandsRun"));
        let evidence = normalize_string_list(parsed.get("evidence"));
        let touched_count = change_summary
            .get("touchedFileCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if touched_count == 0
            && changed_files.is_empty()
            && commands.is_empty()
            && evidence.is_empty()
        {
            issues.push("Done result lacks changed files, commands, and evidence.".to_string());
        }
        if !task.requirement_ids.is_empty() {
            let updated = parsed
                .get("requirementUpdates")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|update| update.get("id").and_then(Value::as_str))
                .collect::<BTreeSet<_>>();
            for id in &task.requirement_ids {
                if !updated.contains(id.as_str()) {
                    issues.push(format!("Missing requirement update for {id}."));
                }
            }
        }
    }
    issues
}

fn build_task_result_repair_prompt(
    run: &BoardRun,
    task: &BoardTask,
    index: usize,
    parsed: &Value,
    assistant_output: &str,
    change_summary: &Value,
    issues: &[String],
) -> String {
    format!(
        r#"Repair the previous agentic Kanban task result into the required JSON contract.

This is a reporting repair only.
- Do not edit files.
- Do not rerun implementation.
- Do not claim verification that is not present in the previous output, task transcript, requirement evidence, or workspace delta.
- If the previous output does not contain enough evidence to honestly mark the task done, return status "needs_followup".
- Return JSON only. No markdown fence.

User request:
{request}

Task {number} of {total}: {title}
Details:
{details}

Attached requirements:
{requirements}

Schema/evidence issues:
{issues}

Workspace delta:
{delta}

Previous parsed result:
{parsed}

Previous assistant output:
{output}

Required schema:
{{
  "status": "done" | "blocked" | "needs_followup",
  "summary": "short summary",
  "changedFiles": ["files changed or inspected as already correct"],
  "coveredRequirements": ["attached requirement IDs"],
  "commandsRun": ["commands/checks actually shown in previous output"],
  "qaResult": "pass" | "fail" | "blocked" | "not_run",
  "evidence": ["specific evidence from previous output or workspace delta"],
  "remainingIssues": [],
  "remainingGaps": [],
  "requirementUpdates": [
    {{
      "id": "REQ-0001",
      "status": "implemented" | "already_implemented" | "blocked" | "deferred" | "non_actionable",
      "evidence": ["specific file, command, or inspection evidence"],
      "notes": "",
      "blockedReason": ""
    }}
  ],
  "suggestedBacklogTasks": []
}}"#,
        request = run.source_prompt,
        number = index + 1,
        total = run.tasks.len(),
        title = task.title,
        details = task.details,
        requirements = task_requirement_summary(run, task),
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

fn reusable_session_id(run: &BoardRun) -> Option<String> {
    run.actual_session_id
        .clone()
        .or_else(|| run.session_id.clone())
        .filter(|value| !value.trim().is_empty())
}

fn board_task_id_for_label(run: &BoardRun, label: &str) -> Option<String> {
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

fn should_resume_provider_session(run: &BoardRun) -> bool {
    matches!(
        normalize_session_policy(Some(&run.session_policy)).as_str(),
        "continuous"
    ) && run.provider == "claude"
}

fn uses_simplified_orchestration(run: &BoardRun) -> bool {
    run.orchestration_version >= 2
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

fn apply_deterministic_validation_result(mut parsed: Value, validation: &Value) -> Value {
    let mut evidence = normalize_string_list(parsed.get("evidence"));
    evidence.push(format_validation_run(validation));
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

fn apply_completion_evidence_gate(
    run: &BoardRun,
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
    let requirement_updates = parsed
        .get("requirementUpdates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let all_no_edit_status = !requirement_updates.is_empty()
        && requirement_updates.iter().all(|update| {
            matches!(
                update.get("status").and_then(Value::as_str),
                Some("already_implemented" | "non_actionable" | "deferred")
            )
        });
    if touched_count > 0 || !changed_files.is_empty() || !commands.is_empty() || all_no_edit_status
    {
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

fn apply_task_result_to_run(run: &mut BoardRun, task_id: &str, parsed: &Value) {
    apply_requirement_updates(run, parsed.get("requirementUpdates"), task_id);
    for file in normalize_string_list(parsed.get("changedFiles")) {
        run.change_ledger.push(json!({
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
        run.validation_runs.push(json!({
            "taskId": task_id,
            "command": command,
            "passed": passed,
            "completedAt": Utc::now(),
        }));
    }
}

fn record_task_workspace_changes(run: &mut BoardRun, task_id: &str, before: Value) -> Value {
    let after = capture_workspace_snapshot(&run.project_path);
    let summary = summarize_workspace_delta(task_id, &before, &after);
    if let Some(task) = run.tasks.iter_mut().find(|task| task.id == task_id) {
        task.changed_file_summary = Some(summary.clone());
        let paths = change_summary_paths(&summary);
        if !paths.is_empty() {
            task.changed_files = paths;
        }
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

async fn ensure_managed_git_branch_for_task_group(run: &mut BoardRun, task_id: &str) -> Result<()> {
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

async fn finalize_managed_git_task_group(run: &mut BoardRun, task_id: &str) -> Result<()> {
    if run.git_policy != "managed" {
        return Ok(());
    }
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id).cloned() else {
        return Ok(());
    };
    if task.status != "completed" {
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

fn task_group_has_unfinished_work(run: &BoardRun, group_id: &str) -> bool {
    run.tasks.iter().any(|task| {
        task.group_id.as_deref() == Some(group_id)
            && matches!(
                task.status.as_str(),
                "pending" | "planned" | "in_progress" | "backlog" | "backlog_generating"
            )
    })
}

fn managed_git_entry<'a>(run: &'a BoardRun, group_id: &str) -> Option<&'a Value> {
    run.git_ledger
        .iter()
        .find(|entry| entry.get("groupId").and_then(Value::as_str) == Some(group_id))
}

fn managed_git_entry_mut<'a>(run: &'a mut BoardRun, group_id: &str) -> Option<&'a mut Value> {
    run.git_ledger
        .iter_mut()
        .find(|entry| entry.get("groupId").and_then(Value::as_str) == Some(group_id))
}

fn set_managed_git_entry_field(run: &mut BoardRun, group_id: &str, key: &str, value: Value) {
    if let Some(entry) = managed_git_entry_mut(run, group_id).and_then(Value::as_object_mut) {
        entry.insert(key.to_string(), value);
    }
}

fn mark_managed_git_failed(run: &mut BoardRun, group_id: &str, message: &str) {
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
    run: &mut BoardRun,
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
    run: &mut BoardRun,
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
    let touched_files = paths
        .into_iter()
        .filter_map(|path| {
            let before_file = before_map.get(&path);
            let after_file = after_map.get(&path);
            if before_file == after_file {
                return None;
            }
            Some(json!({
                "path": path,
                "beforeStatus": before_file.and_then(|value| value.get("status")).and_then(Value::as_str).unwrap_or(""),
                "afterStatus": after_file.and_then(|value| value.get("status")).and_then(Value::as_str).unwrap_or(""),
                "beforeHash": before_file.and_then(|value| value.get("hash")).and_then(Value::as_str),
                "afterHash": after_file.and_then(|value| value.get("hash")).and_then(Value::as_str),
            }))
        })
        .collect::<Vec<_>>();
    let touched_file_count = touched_files.len();
    json!({
        "taskId": task_id,
        "capturedAt": Utc::now(),
        "isGit": after.get("isGit").and_then(Value::as_bool).unwrap_or(false),
        "touchedFiles": touched_files,
        "touchedFileCount": touched_file_count,
        "currentWorkspaceFiles": after.get("files").cloned().unwrap_or_else(|| json!([])),
        "currentWorkspaceFileCount": after.get("files").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "shortStat": after.get("shortStat").and_then(Value::as_str).unwrap_or(""),
        "unavailableReason": after.get("error").and_then(Value::as_str).unwrap_or(""),
    })
}

fn change_summary_paths(summary: &Value) -> Vec<String> {
    summary
        .get("touchedFiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|file| file.get("path").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn refresh_codebase_context_after_task(run: &mut BoardRun, change_summary: &Value) {
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

async fn run_deterministic_validation(run: &BoardRun, task_id: &str, stage: &str) -> Value {
    let started_at = Utc::now();
    let scripts = package_validation_scripts(&run.project_path, stage);
    let mut commands = Vec::new();
    for (script_name, command_text) in scripts {
        let command_started = Utc::now();
        let result = run_shell_validation_command(&run.project_path, &command_text).await;
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

async fn run_tdd_validation(run: &BoardRun, task: &BoardTask, stage: &str) -> Value {
    if is_qa_task(task) || task.qa_test_commands.is_empty() {
        return run_deterministic_validation(run, &task.id, stage).await;
    }
    let generated =
        run_generated_test_commands(&run.project_path, &task.id, &task.qa_test_commands, stage)
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
) -> Value {
    let started_at = Utc::now();
    let mut commands = Vec::new();
    for command_text in commands_to_run {
        let command_started = Utc::now();
        let result = run_shell_validation_command(project_path, command_text).await;
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
    match timeout(DETERMINISTIC_VALIDATION_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok((
            output.status.code().unwrap_or(1),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )),
        Ok(Err(error)) => Err(format!("Validation command failed: {error}")),
        Err(_) => Err(format!(
            "Validation command timed out after {} seconds",
            DETERMINISTIC_VALIDATION_TIMEOUT.as_secs()
        )),
    }
}

fn package_validation_scripts(project_path: &str, stage: &str) -> Vec<(String, String)> {
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
        .take(if stage == "final" { 4 } else { 2 })
        .map(|name| (name.to_string(), format!("{runner} run {name}")))
        .collect()
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

fn format_validation_run(validation: &Value) -> String {
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

fn apply_requirement_updates(run: &mut BoardRun, updates: Option<&Value>, source: &str) {
    let Some(updates) = updates.and_then(Value::as_array) else {
        return;
    };
    for update in updates {
        let Some(id) = update.get("id").and_then(Value::as_str) else {
            continue;
        };
        if let Some(requirement) = run
            .requirement_matrix
            .iter_mut()
            .find(|requirement| requirement.get("id").and_then(Value::as_str) == Some(id))
        {
            if let Some(object) = requirement.as_object_mut() {
                if let Some(status) = update.get("status").and_then(Value::as_str) {
                    object.insert("status".to_string(), json!(status));
                }
                if let Some(notes) = update.get("notes").and_then(Value::as_str) {
                    object.insert("notes".to_string(), json!(notes));
                }
                if let Some(reason) = update.get("blockedReason").and_then(Value::as_str) {
                    object.insert("blockedReason".to_string(), json!(reason));
                }
                let mut evidence = object
                    .get("evidence")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for item in normalize_string_list(update.get("evidence")) {
                    evidence.push(json!(format!("{source}: {item}")));
                }
                object.insert("evidence".to_string(), Value::Array(evidence));
                let field = match update.get("status").and_then(Value::as_str) {
                    Some("planned") => "plannedBy",
                    Some("verified") => "verifiedBy",
                    Some("implemented") | Some("already_implemented") => "implementedBy",
                    _ => "",
                };
                if !field.is_empty() {
                    let mut values = object
                        .get(field)
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    values.push(json!(source));
                    object.insert(field.to_string(), Value::Array(values));
                }
                object.insert("updatedAt".to_string(), json!(Utc::now()));
            }
        }
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
    match value.map(str::trim) {
        Some("high") => "high",
        Some("low") => "low",
        _ => "medium",
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

fn is_resumable_run(run: &BoardRun) -> bool {
    match run.status.as_str() {
        "scheduled" | "paused" | "pausing" => true,
        "blocked" | "failed" | "cancelled" => run.tasks.iter().any(|task| {
            matches!(
                task.status.as_str(),
                "pending" | "running" | "in_progress" | "blocked"
            )
        }),
        _ => false,
    }
}

fn schedule_auto_retry_if_eligible(run: &mut BoardRun, reason: &str) -> bool {
    let state = normalize_auto_retry(&run.auto_retry);
    if !state
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || !is_resumable_run(run)
    {
        run.auto_retry = state;
        return false;
    }
    let attempts = state.get("attempts").and_then(Value::as_u64).unwrap_or(0);
    let max_attempts = state
        .get("maxAttempts")
        .and_then(Value::as_u64)
        .unwrap_or(3);
    if attempts >= max_attempts {
        run.auto_retry = merge_auto_retry(
            state,
            json!({
                "nextRetryAt": null,
                "lastError": format!("Max auto retries reached ({attempts}/{max_attempts})"),
                "updatedAt": Utc::now(),
            }),
        );
        run.append_log("Auto retry stopped: max attempts reached");
        return true;
    }
    let delay_minutes = state
        .get("delayMinutes")
        .and_then(Value::as_i64)
        .unwrap_or(10)
        .max(1);
    let next_retry_at = Utc::now() + chrono::Duration::minutes(delay_minutes);
    run.auto_retry = merge_auto_retry(
        state,
        json!({
            "nextRetryAt": next_retry_at,
            "lastError": "",
            "updatedAt": Utc::now(),
        }),
    );
    run.append_log(format!(
        "Auto retry scheduled in {delay_minutes} minute(s) after {reason}"
    ));
    true
}

fn reset_attention_tasks_for_retry(run: &mut BoardRun) {
    for task in &mut run.tasks {
        if matches!(
            task.status.as_str(),
            "failed" | "blocked" | "backlog_failed" | "in_progress"
        ) {
            task.status = "pending".to_string();
            task.error = None;
        }
    }
    run.status = "running".to_string();
    run.active = true;
    run.loop_started = false;
    run.matrix_gap_review_complete = false;
    run.final_matrix_qa_complete = false;
    run.paused_at = None;
    run.pause_reason = None;
    run.cancellation_reason = None;
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
            let files = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|line| {
                    let path = line.get(3..).unwrap_or(line).trim().to_string();
                    json!({
                        "status": line.get(..2).unwrap_or("").trim(),
                        "path": path,
                        "hash": hash_workspace_file(project_path, line.get(3..).unwrap_or(line).trim()),
                    })
                })
                .collect::<Vec<_>>();
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

fn hash_workspace_file(project_path: &str, path: &str) -> Option<String> {
    let clean_path = path
        .split(" -> ")
        .last()
        .unwrap_or(path)
        .trim()
        .trim_matches('"');
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

fn local_codebase_snapshot(run: &BoardRun) -> Value {
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

fn set_phase(run: &mut BoardRun, phase: &str, details: Value) {
    run.current_phase = Some(phase.to_string());
    run.phase_started_at = Some(Utc::now());
    run.phase_details = Some(details);
}

fn effective_model_for_phase(run: &BoardRun, label: &str) -> String {
    let phase_key = label.replace(' ', "_");
    run.task_model_overrides
        .get(&phase_key)
        .or_else(|| run.task_model_overrides.get(label))
        .or_else(|| run.task_model_overrides.get(model_type_for_phase(label)))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|model| !model.trim().is_empty())
        .or_else(|| {
            run.model_strategy
                .as_ref()
                .and_then(|strategy| strategy.get("model"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| run.model.clone())
}

fn effective_model_for_task(run: &BoardRun, task: &BoardTask) -> String {
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
        })
        .unwrap_or_else(|| run.model.clone())
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
    } else {
        "implementation"
    }
}

fn apply_task_model_routing(run: &mut BoardRun, task_index: usize) {
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
    match policy.map(str::trim).filter(|value| !value.is_empty()) {
        Some("continuous") => "continuous".to_string(),
        Some("task-model") | Some("task_model") | Some("per-task") => "task-model".to_string(),
        _ => "task-model".to_string(),
    }
}

fn increment_provider_usage(
    run: &mut BoardRun,
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

fn apply_run_options(run: &mut BoardRun, request: &CreateRunRequest) -> Result<()> {
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
        run.model_strategy = request.model_strategy.clone();
    }
    if let Some(profile) = trim_string(request.run_profile.clone()) {
        run.run_profile = normalize_run_profile(Some(&profile));
    }
    if let Some(overrides) = request.task_model_overrides.clone() {
        run.task_model_overrides = overrides;
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
    Ok(())
}

#[derive(Debug)]
struct StoredRun {
    path: PathBuf,
    run: BoardRun,
}

fn load_user_run(state: &AppState, user_id: &str, id: &str) -> Result<StoredRun> {
    load_runs(state)?
        .into_iter()
        .find(|stored| stored.run.id == id && stored.run.user_id.as_deref() == Some(user_id))
        .ok_or_else(|| not_found("Danger run not found"))
}

fn latest_run_for_project(
    state: &AppState,
    user_id: &str,
    project_path: &str,
) -> Result<Option<StoredRun>> {
    let mut runs = load_runs(state)?
        .into_iter()
        .filter(|stored| {
            stored.run.user_id.as_deref() == Some(user_id)
                && stored.run.project_path == project_path
        })
        .collect::<Vec<_>>();
    runs.sort_by(|left, right| right.run.updated_at.cmp(&left.run.updated_at));
    Ok(runs.into_iter().next())
}

fn load_runs(state: &AppState) -> Result<Vec<StoredRun>> {
    let dir = runs_dir(state);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut runs = Vec::new();
    for entry in fs::read_dir(&dir).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let content = fs::read_to_string(&path).map_err(io_error)?;
        match serde_json::from_str::<BoardRun>(&content) {
            Ok(mut run) => {
                normalize_board_run_provenance(&mut run);
                runs.push(StoredRun { path, run });
            }
            Err(error) => {
                tracing::warn!(file = %path.display(), %error, "failed to read agentic board snapshot");
            }
        }
    }
    Ok(runs)
}

fn save_run(state: &AppState, run: &BoardRun) -> Result<()> {
    let _guard = BOARD_RUN_SAVE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = runs_dir(state);
    fs::create_dir_all(&dir).map_err(io_error)?;
    let path = run_file_path(state, &run.id);
    let temp_path = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    let mut run_to_save = run.clone();
    if let Ok(content) = fs::read_to_string(&path)
        && let Ok(current) = serde_json::from_str::<BoardRun>(&content)
    {
        preserve_newer_control_state(&mut run_to_save, &current);
    }
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

fn preserve_newer_control_state(run: &mut BoardRun, current: &BoardRun) {
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
            if matches!(task.status.as_str(), "running" | "in_progress")
                && !matches!(current_task.status.as_str(), "running" | "in_progress")
            {
                task.status = current_task.status.clone();
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

fn board_run_mutation_lock() -> MutexGuard<'static, ()> {
    BOARD_RUN_MUTATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn runs_dir(state: &AppState) -> PathBuf {
    state.config.config_dir.join(BOARD_RUNS_DIR)
}

fn run_file_path(state: &AppState, id: &str) -> PathBuf {
    runs_dir(state).join(format!("{id}.json"))
}

#[cfg(test)]
fn prompt_to_task_drafts(run: &mut BoardRun, prompt: &str) -> Vec<BoardTask> {
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
    effective_model: String,
    started_at: DateTime<Utc>,
}

async fn generate_prompt_task_drafts(
    state: &AppState,
    run: &BoardRun,
    prompt: &str,
    model: Option<&str>,
    run_profile: Option<&str>,
) -> PromptTaskDraftAttempt {
    let profile = run_profile
        .map(|value| normalize_run_profile(Some(value)))
        .unwrap_or_else(|| normalize_run_profile(Some(&run.run_profile)));
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
        .or_else(|| trim_string(Some(run.primary_model.clone())))
        .unwrap_or_else(|| run.model.clone());
    let mut generation_run = run.clone();
    generation_run.run_profile = profile.clone();
    generation_run.actual_session_id = None;
    generation_run.current_provider_session_id = None;
    generation_run.session_id = None;
    generation_run.current_task_id = run.current_task_id.clone().or_else(|| {
        run.tasks
            .iter()
            .find(|task| task.backlog_generation_task && task.status == "backlog_generating")
            .map(|task| task.id.clone())
    });
    if !selected_model.trim().is_empty() {
        generation_run.model = selected_model.clone();
        generation_run.primary_model = selected_model.clone();
    }
    let provider_prompt = build_prompt_task_draft_prompt(&generation_run, prompt, &profile);
    let provider = generation_run.provider.clone();
    let started_at = Utc::now();
    let provider_result = execute_shared_provider_turn(
        state,
        &generation_run,
        &provider,
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
            effective_model: selected_model,
            started_at,
        },
    }
}

fn record_prompt_task_generation_attempt(
    run: &mut BoardRun,
    label: &str,
    attempt: &PromptTaskDraftAttempt,
) {
    let controls = board_provider_controls(run);
    let mut telemetry = json!({
        "phase": "backlog_generation",
        "label": label,
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

fn build_prompt_task_draft_prompt(run: &BoardRun, prompt: &str, profile: &str) -> String {
    format!(
        r#"Create implementation-ready Kanban backlog cards for this focused follow-up prompt.

Prompt:
{prompt}

Run profile: {profile}
Profile requirements: {profile_instructions}

Known requirements:
{requirements}

Existing board tasks:
{tasks}

Return JSON only. No markdown fence.
Schema:
{{
  "tasks": [
    {{
      "title": "specific implementation task",
      "details": "bounded implementation details",
      "acceptanceCriteria": ["verifiable outcome"],
      "references": ["relevant file, source, or requirement"],
      "files": ["likely file or path"],
      "requirementIds": ["REQ-0001"],
      "priority": "high|medium|low",
      "dependsOn": []
    }}
  ]
}}

Rules:
- Generate only cards directly needed for the prompt-matched feature area.
- Preserve explicit user scope; do not add unrelated cleanup or product ideas.
- Keep every card independently actionable and verifiable.
- Prefer a small number of complete cards over many vague cards."#,
        prompt = prompt,
        profile = profile,
        profile_instructions = run_profile_instructions(profile),
        requirements = requirement_summary(run),
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

fn run_profile_instructions(profile: &str) -> &'static str {
    match normalize_run_profile(Some(profile)).as_str() {
        "minimal" => {
            "Implement explicit requirements with minimal expansion and the lowest useful context footprint."
        }
        "product_ready" => {
            "Include complete workflow detail, useful summaries, structural responsive checks, and strict edge-case QA when relevant."
        }
        _ => {
            "Include needed validation, persistence, empty and error states, and local verification for a complete feature."
        }
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
            Some(json!({
                "title": title,
                "details": details,
                "prompt": prompt,
                "acceptanceCriteria": normalize_string_list(
                    task.get("acceptanceCriteria")
                        .or_else(|| task.get("acceptance"))
                        .or_else(|| task.get("criteria")),
                ),
                "references": references,
                "requirementIds": normalize_string_list(
                    task.get("requirementIds").or_else(|| task.get("requirements")),
                ),
                "priority": normalize_priority(task.get("priority").and_then(Value::as_str)),
                "dependsOn": normalize_string_list(
                    task.get("dependsOn").or_else(|| task.get("dependencies")),
                ),
                "status": "backlog",
            }))
        })
        .collect()
}

fn backlog_generation_placeholder(
    run: &mut BoardRun,
    prompt: &str,
    model: &str,
    run_profile: &str,
) -> BoardTask {
    let profile = if run_profile.trim().is_empty() {
        normalize_run_profile(Some(&run.run_profile))
    } else {
        normalize_run_profile(Some(run_profile))
    };
    let mut task = BoardTask::draft(
        run,
        "Adding tasks to backlog from prompt".to_string(),
        prompt.to_string(),
    );
    task.status = "backlog_generating".to_string();
    task.prompt = prompt.to_string();
    task.acceptance_criteria =
        vec!["Generate one or more backlog task cards from the prompt.".to_string()];
    task.references = vec![
        "Prompt backlog generation is running in the background.".to_string(),
        format!(
            "Breakdown model: {}",
            model
                .trim()
                .is_empty()
                .then_some("provider default")
                .unwrap_or(model.trim())
        ),
        format!("Breakdown profile: {profile}"),
        format!(
            "Breakdown scope: {}",
            if prompt_requests_broad_coverage(prompt) {
                "Broad coverage"
            } else {
                "Focused prompt scope"
            }
        ),
    ];
    task.backlog_generation_task = true;
    task.manual_task = false;
    task.prompt_task = true;
    task.task_origin = "user_prompt_generated".to_string();
    task
}

fn spawn_backlog_prompt_generation(
    state: AppState,
    user_id: String,
    run_id: String,
    placeholder_task_id: String,
    prompt: String,
    model: String,
    run_profile: String,
) {
    tokio::spawn(async move {
        if let Err(error) = complete_backlog_prompt_generation(
            &state,
            &user_id,
            &run_id,
            &placeholder_task_id,
            &prompt,
            &model,
            &run_profile,
        )
        .await
        {
            tracing::warn!(
                run_id = %run_id,
                task_id = %placeholder_task_id,
                error = %server_error_message(&error),
                "backlog prompt generation failed"
            );
        }
    });
}

async fn complete_backlog_prompt_generation(
    state: &AppState,
    user_id: &str,
    run_id: &str,
    placeholder_task_id: &str,
    prompt: &str,
    model: &str,
    run_profile: &str,
) -> Result<()> {
    let snapshot = load_user_run(state, user_id, run_id)?;
    let attempt = generate_prompt_task_drafts(
        state,
        &snapshot.run,
        prompt,
        (!model.trim().is_empty() && model.trim() != "provider default").then_some(model),
        (!run_profile.trim().is_empty()).then_some(run_profile),
    )
    .await;
    let _guard = board_run_mutation_lock();
    let mut stored = load_user_run(state, user_id, run_id)?;
    let Some(index) = stored
        .run
        .tasks
        .iter()
        .position(|task| task.id == placeholder_task_id)
    else {
        return Ok(());
    };
    if stored.run.tasks[index].status != "backlog_generating" {
        return Ok(());
    }
    record_prompt_task_generation_attempt(
        &mut stored.run,
        "Kanban backlog prompt generation",
        &attempt,
    );
    match attempt.result {
        Ok((drafts, warning)) => {
            let mut generated = drafts
                .into_iter()
                .map(|draft| prompt_task_from_draft(&mut stored.run, draft, prompt))
                .collect::<Vec<_>>();
            generated[0].id = placeholder_task_id.to_string();
            sanitize_generated_task_dependencies(&stored.run, &mut generated, placeholder_task_id);
            if let Some(warning) = warning.as_deref().filter(|value| !value.trim().is_empty()) {
                generated[0]
                    .references
                    .push(format!("Task generation note: {warning}"));
            }
            let count = generated.len();
            stored.run.tasks.splice(index..=index, generated);
            stored.run.append_log(format!(
                "Backlog prompt generated {count} task(s) from {placeholder_task_id}"
            ));
        }
        Err(error) => {
            let message = server_error_message(&error);
            let task = &mut stored.run.tasks[index];
            task.status = "backlog_failed".to_string();
            task.error = Some(message.clone());
            task.summary = "Task generation failed. Retry this card when ready.".to_string();
            task.completed_at = Some(Utc::now());
            stored.run.append_log(format!(
                "Backlog prompt generation failed for {placeholder_task_id}: {message}"
            ));
        }
    }
    stored.run.touch();
    save_run(state, &stored.run)
}

fn sanitize_generated_task_dependencies(
    run: &BoardRun,
    generated: &mut [BoardTask],
    placeholder_task_id: &str,
) {
    let valid_ids = run
        .tasks
        .iter()
        .filter(|task| task.id != placeholder_task_id)
        .map(|task| task.id.clone())
        .chain(generated.iter().map(|task| task.id.clone()))
        .collect::<BTreeSet<_>>();
    for task in generated {
        task.depends_on
            .retain(|dependency| dependency != &task.id && valid_ids.contains(dependency));
    }
}

fn prompt_task_from_draft(run: &mut BoardRun, draft: Value, prompt: &str) -> BoardTask {
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
    let mut task = BoardTask::draft(run, title, details.clone());
    task.prompt = prompt.to_string();
    task.acceptance_criteria = normalize_string_list(draft.get("acceptanceCriteria"));
    if task.acceptance_criteria.is_empty() {
        task.acceptance_criteria = vec!["Complete the task described by this card.".to_string()];
    }
    task.references = normalize_string_list(draft.get("references"));
    task.requirement_ids = normalize_string_list(draft.get("requirementIds"));
    task.priority = normalize_priority(draft.get("priority").and_then(Value::as_str)).to_string();
    task.depends_on = normalize_string_list(draft.get("dependsOn"));
    task.status = "backlog".to_string();
    task.manual_task = false;
    task.prompt_task = true;
    task.task_origin = "user_prompt_generated".to_string();
    task.backlog_generation_task = false;
    task
}

fn task_reference_value(task: &BoardTask, prefix: &str) -> String {
    task.references
        .iter()
        .find_map(|reference| {
            reference
                .to_ascii_lowercase()
                .starts_with(&prefix.to_ascii_lowercase())
                .then(|| reference[prefix.len()..].trim().to_string())
        })
        .unwrap_or_default()
}

fn prompt_requests_broad_coverage(prompt: &str) -> bool {
    let text = prompt.to_ascii_lowercase();
    let broad = ["all", "every", "entire", "whole", "complete", "full"]
        .iter()
        .any(|word| text.split_whitespace().any(|part| part == *word));
    broad
        && [
            "app",
            "application",
            "project",
            "codebase",
            "requirements",
            "features",
            "workflows",
            "modules",
            "board",
            "regression",
            "coverage",
            "audit",
        ]
        .iter()
        .any(|word| text.contains(word))
}

fn allocate_task_id(run: &mut BoardRun) -> String {
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

fn next_task_id(run: &BoardRun) -> String {
    let mut run = run.clone();
    allocate_task_id(&mut run)
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
            .filter(|task| orchestration_version < 2 || !task.internal_validation)
            .map(|task| task.status.as_str()),
    )
}

fn count_statuses<'a>(statuses: impl Iterator<Item = &'a str>) -> Value {
    let mut counts = serde_json::Map::new();
    let mut total = 0usize;
    for status in statuses {
        total += 1;
        let key = if status == "done" {
            "completed"
        } else {
            status
        };
        let next = counts.get(key).and_then(Value::as_u64).unwrap_or(0) + 1;
        counts.insert(key.to_string(), json!(next));
    }
    counts.insert("total".to_string(), json!(total));
    Value::Object(counts)
}

fn normalize_provider(provider: Option<&str>) -> Result<String> {
    match provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_PROVIDER)
    {
        "claude" | "cursor" | "codex" | "gemini" => {
            Ok(provider.unwrap_or(DEFAULT_PROVIDER).trim().to_string())
        }
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
        "backlog" | "backlog_generating" | "backlog_failed" | "pending" | "planned" | "running"
        | "in_progress" | "pausing" | "cancelling" | "qa" | "review" | "blocked" | "failed"
        | "cancelled" | "completed" | "done" => Ok(status.to_string()),
        _ => Err(bad_request(
            "Task status must be one of: backlog, pending, planned, in_progress, qa, review, blocked, failed, cancelled, completed",
        )),
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

fn normalize_board_run_provenance(run: &mut BoardRun) {
    for task in &mut run.tasks {
        let canonical = canonical_task_origin(&task.task_origin);
        task.task_origin = if canonical.is_empty() {
            infer_legacy_task_origin(task)
                .unwrap_or_default()
                .to_string()
        } else {
            canonical.to_string()
        };
    }
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

fn normalize_run_profile(profile: Option<&str>) -> String {
    match profile
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('-', "_")
        .as_str()
    {
        "minimal"
        | "follow_requirements"
        | "requirements"
        | "requirement_only"
        | "strict"
        | "cheap" => "minimal".to_string(),
        "product_ready" | "productready" | "product" | "polished" | "expensive" | "quality" => {
            "product_ready".to_string()
        }
        _ => "complete_app".to_string(),
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

fn is_true(value: Option<&str>) -> bool {
    matches!(value, Some("true" | "1" | "yes"))
}

fn default_orchestration_version() -> u32 {
    2
}

fn default_provider_string() -> String {
    DEFAULT_PROVIDER.to_string()
}

fn default_run_profile() -> String {
    normalize_run_profile(None)
}

fn default_git_policy() -> String {
    "read_only".to_string()
}

fn default_paused_status() -> String {
    "paused".to_string()
}

fn default_priority() -> String {
    "medium".to_string()
}

fn default_task_type() -> String {
    "implementation".to_string()
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

    fn board_run(value: Value) -> BoardRun {
        let request = serde_json::from_value::<CreateRunRequest>(value).unwrap();
        BoardRun::new(None, request).unwrap()
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

        let mut run = board_run(json!({
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

        let mut run = board_run(json!({
            "command": "Implement feature",
            "projectPath": project.display().to_string(),
            "provider": "codex"
        }));
        run.id = "legacy-run".to_string();
        run.user_id = Some("user-1".to_string());
        run.tasks[0].provider_session_id = Some("legacy-board-chat".to_string());
        save_run(&state, &run).expect("persist legacy board run");

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
                    content: format!("Board run id: {}\nTask 1: {}", run.id, run.tasks[0].id),
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
        assert_eq!(classified.board_run_id.as_deref(), Some("legacy-run"));
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
    fn new_run_preserves_every_mobile_configuration_field() {
        let run = board_run(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "projectName": "project",
            "provider": "gemini",
            "model": "gemini-2.5-pro",
            "nextProvider": "codex",
            "nextModel": "gpt-5",
            "modelStrategy": {"mode": "fallback"},
            "runProfile": "complete_app",
            "taskModelOverrides": {"qa": "sonnet"},
            "sessionPolicy": "task-model",
            "gitPolicy": "managed",
            "toolsSettings": {"shell": true}
        }));

        assert_eq!(run.provider, "gemini");
        assert_eq!(run.model, "gemini-2.5-pro");
        assert_eq!(run.next_provider, "codex");
        assert_eq!(run.next_model, "gpt-5");
        assert_eq!(run.model_strategy, Some(json!({"mode": "fallback"})));
        assert_eq!(run.run_profile, "complete_app");
        assert_eq!(run.task_model_overrides, json!({"qa": "sonnet"}));
        assert_eq!(run.session_policy, "task-model");
        assert_eq!(run.git_policy, "managed");
        assert_eq!(run.tools_settings, Some(json!({"shell": true})));
    }

    #[test]
    fn orchestration_v2_summary_hides_internal_validation_tasks() {
        let mut run = board_run(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].status = "completed".to_string();
        let mut validation = run.tasks[0].clone();
        validation.id = "task-final-qa".to_string();
        validation.internal_validation = true;
        validation.status = "completed".to_string();
        run.tasks.push(validation);

        let summary = run.summary_json(None);

        assert_eq!(summary["taskCounts"]["total"], 1);
        assert_eq!(summary["taskCounts"]["completed"], 1);
    }

    #[test]
    fn summary_reports_actual_telemetry_validation_and_resumability() {
        let mut run = board_run(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.status = "running".to_string();
        run.auto_run_enabled = true;
        run.tasks[0].status = "pending".to_string();
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
        let mut run = board_run(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let request = serde_json::from_value::<TaskRequest>(json!({
            "title": "Rich task",
            "details": "Implement the feature",
            "priority": "high",
            "status": "backlog",
            "acceptanceCriteria": ["Tests pass", "UI renders"],
            "references": ["README.md"],
            "files": ["src/main.rs"],
            "paths": ["tests/main.rs"],
            "requirementIds": ["REQ-1"],
            "dependsOn": ["task-0"]
        }))
        .unwrap();

        let task = BoardTask::manual(&mut run, request).unwrap();

        assert_eq!(task.title, "Rich task");
        assert_eq!(task.details, "Implement the feature");
        assert_eq!(task.priority, "high");
        assert_eq!(task.status, "backlog");
        assert_eq!(task.acceptance_criteria, vec!["Tests pass", "UI renders"]);
        assert_eq!(
            task.references,
            vec!["README.md", "src/main.rs", "tests/main.rs"]
        );
        assert_eq!(task.requirement_ids, vec!["REQ-1"]);
        assert_eq!(task.depends_on, vec!["task-0"]);
        assert_eq!(task.task_origin, "user_manual");
    }

    #[test]
    fn legacy_task_origins_normalize_without_changing_system_origins() {
        let mut run = board_run(json!({
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

        normalize_board_run_provenance(&mut run);

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
    fn promoted_user_task_receives_a_requirement_matrix_entry() {
        let mut run = board_run(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.requirement_matrix.clear();
        let request = serde_json::from_value::<TaskRequest>(json!({
            "title": "Export audit log",
            "details": "Add CSV export with a focused test.",
            "status": "backlog"
        }))
        .unwrap();
        let mut task = BoardTask::manual(&mut run, request).unwrap();

        ensure_manual_task_requirements(&mut run.requirement_matrix, &mut task);
        mark_requirements_from_tasks(&mut run, std::slice::from_ref(&task));

        assert_eq!(task.requirement_ids, vec!["REQ-MANUAL-1"]);
        assert_eq!(run.requirement_matrix.len(), 1);
        assert_eq!(run.requirement_matrix[0]["id"], "REQ-MANUAL-1");
        assert_eq!(run.requirement_matrix[0]["heading"], "Manual task");
        assert_eq!(run.requirement_matrix[0]["status"], "planned");
    }

    #[test]
    fn successful_task_adds_deduplicated_optional_backlog_suggestions() {
        let mut run = board_run(json!({
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
    fn derived_tasks_use_only_requirements_named_by_the_result() {
        let mut run = board_run(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        run.tasks[0].requirement_ids = vec![
            "REQ-1".to_string(),
            "REQ-2".to_string(),
            "REQ-3".to_string(),
        ];
        let source = run.tasks[0].clone();

        assert_eq!(
            resolve_derived_requirement_ids(
                &source,
                &json!({
                    "requirementUpdates": [{"id": "REQ-2"}, {"id": "OTHER"}],
                    "coveredRequirements": ["REQ-1"]
                }),
            ),
            vec!["REQ-2"]
        );
        assert_eq!(
            resolve_derived_requirement_ids(
                &source,
                &json!({"coveredRequirements": ["REQ-3", "OTHER"]}),
            ),
            vec!["REQ-3"]
        );
        assert_eq!(
            resolve_derived_requirement_ids(&source, &json!({})),
            source.requirement_ids
        );
    }

    #[test]
    fn manual_task_rejects_unknown_status() {
        let mut run = board_run(json!({
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
    fn run_profiles_normalize_to_the_three_supported_journeys() {
        assert_eq!(normalize_run_profile(Some("minimal")), "minimal");
        assert_eq!(normalize_run_profile(Some("strict")), "minimal");
        assert_eq!(normalize_run_profile(Some("complete")), "complete_app");
        assert_eq!(
            normalize_run_profile(Some("product-ready")),
            "product_ready"
        );
        assert_eq!(normalize_run_profile(Some("quality")), "product_ready");
    }

    #[test]
    fn prompt_generation_placeholder_preserves_retry_settings() {
        let mut run = board_run(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "runProfile": "minimal"
        }));

        let task = backlog_generation_placeholder(
            &mut run,
            "Add export filters",
            "gpt-5.6-sol",
            "product_ready",
        );

        assert_eq!(task.status, "backlog_generating");
        assert!(task.backlog_generation_task);
        assert!(
            task.references
                .contains(&"Breakdown model: gpt-5.6-sol".to_string())
        );
        assert!(
            task.references
                .contains(&"Breakdown profile: product_ready".to_string())
        );
    }

    #[test]
    fn six_stage_model_routes_select_the_expected_override() {
        let mut run = board_run(json!({
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
        implementation.status = "pending".to_string();
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
    fn pause_during_provider_turn_keeps_single_runner_owner() {
        let mut run = board_run(json!({
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
        assert!(!board_run_should_abort_provider(&run));
    }

    #[test]
    fn pause_before_provider_turn_returns_current_card_to_todo() {
        let mut run = board_run(json!({
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

        request_board_pause(&mut run, Some("user request".to_string()));

        assert_eq!(run.status, "paused");
        assert!(!run.active);
        assert!(!run.loop_started);
        assert!(!run.pause_requested);
        assert_eq!(run.tasks[0].status, "pending");
        assert_eq!(run.tasks[0].started_at, None);
        assert_eq!(run.current_task_id, None);
        assert_eq!(run.control_revision, 1);
    }

    #[test]
    fn stale_runner_save_preserves_newer_pause_control_state() {
        let mut stale = board_run(json!({
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
        assert_eq!(stale.tasks[0].status, "pending");
        assert_eq!(stale.control_revision, paused.control_revision);
        assert_eq!(stale.pause_reason.as_deref(), Some("user request"));
    }

    #[test]
    fn abort_resets_in_flight_cards_and_control_pointers() {
        let mut run = board_run(json!({
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

        assert_eq!(run.tasks[0].status, "pending");
        assert_eq!(run.current_task_id, None);
        assert_eq!(run.current_provider_session_id, None);
        assert_eq!(run.provider_call_started_at, None);
    }

    #[test]
    fn immediate_resume_reuses_existing_runner_owner() {
        let mut run = board_run(json!({
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
        let mut run = board_run(json!({
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
        let mut run = board_run(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        run.status = "pausing".to_string();
        run.pause_requested = true;
        assert!(!board_run_should_abort_provider(&run));

        run.status = "cancelled".to_string();
        assert!(board_run_should_abort_provider(&run));
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
        let mut run = board_run(json!({
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
    fn legacy_task_sequence_migrates_from_existing_ids() {
        let mut run = board_run(json!({
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
        let mut run = board_run(json!({
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
    fn task_dependencies_ignore_self_and_missing_cards() {
        let mut run = board_run(json!({
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

        assert_eq!(unmet_task_dependencies(&run, &dependent), vec!["task-1"]);

        run.tasks[0].status = "completed".to_string();
        assert!(unmet_task_dependencies(&run, &dependent).is_empty());
    }

    #[test]
    fn generated_placeholder_card_drops_self_dependency() {
        let mut run = board_run(json!({
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
        let mut run = board_run(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project"
        }));
        let placeholder =
            backlog_generation_placeholder(&mut run, "Generate a task", "gpt-5.6-sol", "minimal");
        run.tasks.push(placeholder.clone());

        let prompt = build_prompt_task_draft_prompt(&run, "Generate a task", "minimal");

        assert!(!prompt.contains(&format!("{} [", placeholder.id)));
        assert!(prompt.contains("task-1 [pending] Initial task"));
    }

    #[test]
    fn deleting_task_cleans_dependency_and_source_references() {
        let mut run = board_run(json!({
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

        assert_eq!(run.tasks[0].depends_on, vec!["external"]);
        assert_eq!(run.tasks[0].source_task_id, None);
        assert_eq!(run.tasks[0].source_qa_task_id, None);
    }

    #[test]
    fn deleting_current_task_is_rejected() {
        let mut run = board_run(json!({
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
        let request = serde_json::from_value::<CreateRunRequest>(json!({
            "command": "Initial task",
            "projectPath": "/tmp/project",
            "scheduledStartAt": "not-a-timestamp"
        }))
        .unwrap();

        let error = BoardRun::new(None, request).unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.body.error.contains("valid RFC3339"));
    }

    #[test]
    fn board_strategy_enables_gpt_5_6_sol_fast_mode() {
        let run = board_run(json!({
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
        let mut run = board_run(json!({
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
            effective_model: "gpt-5.6-sol".to_string(),
            started_at: Utc::now(),
        };

        record_prompt_task_generation_attempt(
            &mut run,
            "Kanban backlog prompt generation",
            &attempt,
        );

        let telemetry = run.prompt_telemetry.last().unwrap();
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
    fn clearing_schedule_returns_scheduled_run_to_paused_state() {
        let mut run = board_run(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "scheduledStartAt": "2099-08-09T01:00:00Z"
        }));
        run.status = "scheduled".to_string();
        run.active = false;
        run.loop_started = false;
        run.auto_run_enabled = true;

        clear_run_schedule(&mut run);

        assert_eq!(run.status, "paused");
        assert_eq!(run.scheduled_start_at, None);
        assert!(!run.auto_run_enabled);
        assert_eq!(run.pause_reason.as_deref(), Some("schedule cleared"));
    }

    #[test]
    fn run_detail_exposes_every_mobile_evidence_collection() {
        let run = board_run(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project"
        }));
        let detail = run.detail_json(None);
        for key in [
            "logs",
            "tasks",
            "requirementMatrix",
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
        let run = board_run(json!({
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
        let strategy_run = board_run(json!({
            "command": "Implement feature",
            "projectPath": "/tmp/project",
            "provider": "claude",
            "model": "sonnet",
            "modelStrategy": {
                "fallbackProvider": "gemini",
                "fallbackModel": "gemini-2.5-pro"
            }
        }));
        let same_run = board_run(json!({
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
    fn legacy_tcd_qa_prompt_maps_to_exact_board_task() {
        let mut run = board_run(json!({
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
        let prompt = r#"You are the QA phase of a TDD-first io-workbench Kanban runner.

Project: TCD-Meida-new
Board run id: 28c3b53f-e616-43d9-b4dc-8353fdac7249
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
        let mut run = board_run(json!({
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
        assert!(legacy_prompt_matches_run_telemetry(
            &run,
            started_at + chrono::Duration::seconds(20)
        ));
        assert!(!legacy_prompt_matches_run_telemetry(
            &run,
            started_at + chrono::Duration::minutes(2)
        ));
        assert_eq!(legacy_board_task_id(&run, prompt), None);
    }
}

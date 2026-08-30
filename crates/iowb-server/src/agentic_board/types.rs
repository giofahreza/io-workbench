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

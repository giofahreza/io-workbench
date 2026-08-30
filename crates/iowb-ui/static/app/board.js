const BOARD_COLUMNS = [
  { id: "backlog", title: "Backlog", description: "Needs human approval" },
  { id: "todo", title: "Todo", description: "Ready to start" },
  { id: "active", title: "In Progress", description: "Currently executing" },
  { id: "blocked", title: "Blocked", description: "Waiting on other tasks" },
  { id: "done", title: "Done", description: "Completed groups" },
];

async function loadBoard() {
  const projectPath = activeProjectPath();
  const label = qs("#board-project-label");
  if (label) label.textContent = projectPath ? selectedProjectLabel("#active-project") : "No project selected";
  if (!projectPath) {
    state.board = null;
    renderBoard();
    return;
  }
  state.boardLoading = true;
  renderBoard();
  try {
    const query = new URLSearchParams({ projectPath });
    const body = await api(`/api/danger/boards?${query.toString()}`);
    const boards = Array.isArray(body.boards) ? body.boards : [];
    const boardId = boards[0]?.id || "";
    state.board = boardId ? await loadBoardDetail(boardId) : null;
    rememberBoardChatSessionIds(state.board);
    hideBoardChatSessionsFromLists();
  } finally {
    state.boardLoading = false;
    renderBoard();
  }
}

async function loadBoardDetail(boardId) {
  const body = await api(`/api/danger/boards/${encodeURIComponent(boardId)}`);
  return body.board || null;
}

function rememberBoardChatSessionIds(run) {
  for (const task of run?.tasks || []) {
    const sessionId = boardTaskSessionId(task);
    if (sessionId) state.boardChatSessionIds.add(sessionId);
  }
}

function renderBoard() {
  renderBoardControls();
  const status = qs("#board-status");
  const columns = qs("#board-columns");
  const details = qs("#board-details");
  if (!columns) return;
  const projectPath = activeProjectPath();
  if (!projectPath) {
    if (status) status.textContent = "Select a project to use the agentic board.";
    if (details) details.innerHTML = "";
    columns.innerHTML = "";
    return;
  }
  if (state.boardLoading) {
    if (status) status.textContent = "Loading board...";
    if (details) details.innerHTML = "";
    columns.innerHTML = "";
    return;
  }
  if (!state.board) {
    if (status) status.textContent = "No board for this project yet.";
    if (details) details.innerHTML = "";
    columns.innerHTML = BOARD_COLUMNS.map((column) => renderBoardColumn(column, [])).join("");
    return;
  }
  const run = state.board;
  const tasks = (Array.isArray(run.tasks) ? run.tasks : []).filter((task) => {
    return boardColumnForTask(task) !== "backlog" || boardTaskIsPlanningLevel(task);
  });
  if (status) {
    status.innerHTML = `
      <strong>${escapeHtml(run.projectName || "Project board")}</strong>
      <span>${escapeHtml(run.status || "paused")}</span>
      <span>${escapeHtml(tasks.length)} task${tasks.length === 1 ? "" : "s"}</span>
      <span>${escapeHtml(run.provider || "claude")}${run.model ? ` · ${escapeHtml(run.model)}` : ""}</span>
      <span>TDD ${run.tddEnabled ? "on" : "off"}</span>
      <span>RAG ${run.ragEnabled ? "on" : "off"} · ${escapeHtml(run.ragQueryCount || 0)} queries</span>
    `;
  }
  if (details) details.innerHTML = renderBoardDetails(run);
  const byColumn = new Map(BOARD_COLUMNS.map((column) => [column.id, []]));
  tasks.filter((task) => boardColumnForTask(task) === "backlog").forEach((task) => {
    byColumn.get(boardColumnForTask(task))?.push(task);
  });
  const groupedColumns = new Map(BOARD_COLUMNS.map((column) => [column.id, []]));
  boardTaskGroupsForDisplay(run, tasks).forEach((group) => {
    const column = boardColumnForGroup(group);
    if (column !== "backlog") groupedColumns.get(column)?.push(group);
  });
  columns.innerHTML = BOARD_COLUMNS.map((column) => {
    const items = column.id === "backlog"
      ? byColumn.get(column.id) || []
      : groupedColumns.get(column.id) || [];
    return renderBoardColumn(column, items, column.id === "backlog" ? renderBoardCard : renderBoardGroupCard);
  }).join("");
}

function renderBoardDetails(run) {
  const validations = Array.isArray(run.validationRuns) ? run.validationRuns : [];
  const promotions = Array.isArray(run.promotionCandidates) ? run.promotionCandidates : [];
  const finalReview = run.finalReview || {};
  const tddPolicy = run.tddPolicy || {};
  const modelStrategy = run.modelStrategy || {};
  const validationConfig = run.validationConfig || {};
  const ragSettings = run.ragSettings || {};
  const qaPolicy = run.qaPolicy || {};
  const autoRetry = run.autoRetry || {};
  const latestValidations = validations.slice(-4).reverse();
  const latestPromotions = promotions.slice(-4).reverse();
  const breakdownTranscript = Array.isArray(run.backlogBreakdown?.transcript)
    ? run.backlogBreakdown.transcript
    : [];
  return `
    <section class="board-details-grid">
      <article>
        <h3>Strategy</h3>
        <p>${escapeHtml(statusLabel(run.boardProfile || "complete_app"))} · ${escapeHtml(statusLabel(modelStrategy.mode || "manual"))}</p>
        <p>${escapeHtml(statusLabel(run.sessionPolicy || "continuous"))} · ${escapeHtml(statusLabel(run.gitPolicy || "read_only"))}</p>
        <p>Cheap: ${escapeHtml(modelStrategy.cheapModel || "provider default")}</p>
        <p>Expensive: ${escapeHtml(modelStrategy.expensiveModel || "provider default")}</p>
      </article>
      <article>
        <h3>TDD Policy</h3>
        <p>${run.tddEnabled === false ? "Disabled" : "Enabled"}</p>
        <p>Failing baseline: ${escapeHtml(String(tddPolicy.requireFailingTestBeforeDev !== false))}</p>
        <p>Allow no tests: ${escapeHtml(String(tddPolicy.allowImplementationWithoutTests === true))}</p>
        <p>Max fixes: ${escapeHtml(tddPolicy.maxFixAttempts ?? 3)}</p>
      </article>
      <article>
        <h3>Validation</h3>
        <p>${validationConfig.enabled === false ? "Disabled" : "Enabled"} · timeout ${escapeHtml(validationConfig.timeoutSeconds || 120)}s</p>
        <p>Limits: feature ${escapeHtml(validationConfig.maxFeatureCommands ?? 2)} · final ${escapeHtml(validationConfig.maxFinalCommands ?? 4)} · QA ${escapeHtml(validationConfig.maxQaCommands ?? 2)}</p>
        ${latestValidations.length ? latestValidations.map((item) => `<p>${escapeHtml(item.stage || item.command || "validation")} · ${item.passed === false ? "fail" : "pass"}</p>`).join("") : "<p>No validation yet</p>"}
      </article>
      <article>
        <h3>RAG</h3>
        <p>${ragSettings.enabled === false ? "Disabled" : "Enabled"} · query ${ragSettings.queryEnabled === false ? "off" : "on"}</p>
        <p>${escapeHtml(run.ragQueryCount || 0)} queries · ${escapeHtml(ragSettings.contextMaxChars || 12000)} chars</p>
      </article>
      <article>
        <h3>QA Policy</h3>
        <p>${escapeHtml(statusLabel(qaPolicy.taskQaMode || "high_risk"))}</p>
        <p>Follow-ups: ${escapeHtml(qaPolicy.maxFollowupsPerGroup ?? 3)} · attempts: ${escapeHtml(qaPolicy.maxTaskAttempts ?? 2)}</p>
        <p>Tool repair: ${qaPolicy.repairMalformedToolCalls === false ? "off" : "on"} · retries ${escapeHtml(qaPolicy.malformedToolCallRepairRetries ?? 1)}</p>
      </article>
      <article>
        <h3>Auto Retry</h3>
        <p>${autoRetry.enabled === true ? "Enabled" : "Disabled"} · ${escapeHtml(autoRetry.delayMinutes ?? 10)} min</p>
        <p>Attempts: ${escapeHtml(autoRetry.attempts ?? 0)}/${escapeHtml(autoRetry.maxAttempts ?? 3)}</p>
      </article>
      <article>
        <h3>Promotion</h3>
        <p>${escapeHtml(promotions.length)} candidate${promotions.length === 1 ? "" : "s"}</p>
        ${latestPromotions.length ? latestPromotions.map((item) => {
          const name = item.title || item.taskId || "candidate";
          const summary = item.summary && item.summary !== name ? ` — ${item.summary}` : "";
          return `<p>${escapeHtml(`${name}${summary}`)}</p>`;
        }).join("") : "<p>No candidates yet</p>"}
      </article>
      <article>
        <h3>Final QA</h3>
        <p>${finalReview.complete === true ? "Complete" : finalReview.complete === false ? "Incomplete" : "Pending"}</p>
        ${finalReview.summary ? `<p>${escapeHtml(finalReview.summary)}</p>` : ""}
      </article>
      ${breakdownTranscript.length ? `<article><h3>Board discussion</h3><button type="button" data-board-view-transcript="breakdown">View transcript</button></article>` : ""}
    </section>
  `;
}

function renderBoardControls() {
  const run = state.board;
  const status = String(run?.status || "").toLowerCase();
  const hasRun = Boolean(run?.id);
  const running = hasRun && ["running", "planning", "in_progress"].includes(status);
  const terminal = hasRun && ["completed", "cancelled", "failed"].includes(status);
  const hasTodo = Array.isArray(run?.tasks) && run.tasks.some((task) => ["todo", "pending", "planned"].includes(String(task.status || "").toLowerCase()));
  const resume = qs("#board-resume");
  const pause = qs("#board-pause");
  const abort = qs("#board-abort");
  if (resume) resume.disabled = !hasRun || running || (terminal && !hasTodo);
  if (pause) pause.disabled = !hasRun || !running;
  if (abort) abort.disabled = !hasRun || terminal;
}

function renderBoardColumn(column, items, renderer = renderBoardCard) {
  return `
    <section class="board-column" data-board-column="${escapeHtml(column.id)}">
      <header>
        <div>
          <h3>${escapeHtml(column.title)}</h3>
          <span>${escapeHtml(column.description)}</span>
        </div>
        <strong>${items.length}</strong>
      </header>
      <div class="board-card-list">
        ${items.length ? items.map(renderer).join("") : `<div class="board-empty-column">No cards</div>`}
      </div>
    </section>
  `;
}

function boardTaskGroupId(task) {
  return String(task?.groupId || task?.group_id || task?.id || "").trim();
}

function boardTaskGroupsForDisplay(run, tasks) {
  const sourceTasks = (Array.isArray(run?.tasks) ? run.tasks : tasks).filter((task) => {
    return !task?.backlogGenerationTask
      && !(Number(run?.orchestrationVersion || 0) >= 2 && task?.internalValidation);
  });
  const byId = new Map(sourceTasks.map((task) => [String(task?.id || ""), task]));
  const apiGroups = Array.isArray(run?.taskGroups) ? run.taskGroups : [];
  const groups = apiGroups.map((group) => {
    const groupTasks = Array.isArray(group?.subtasks)
      ? group.subtasks.map((task) => byId.get(String(task?.id || "")) || task).filter(Boolean)
      : Array.isArray(group?.taskIds)
        ? group.taskIds.map((id) => byId.get(String(id || ""))).filter(Boolean)
        : [];
    if (!groupTasks.length) return null;
    return { ...group, subtasks: groupTasks };
  }).filter(Boolean);
  if (groups.length) return groups;

  const grouped = new Map();
  sourceTasks.forEach((task) => {
    const id = boardTaskGroupId(task);
    if (!id) return;
    if (!grouped.has(id)) grouped.set(id, []);
    grouped.get(id).push(task);
  });
  return [...grouped.entries()].map(([id, subtasks]) => ({ id, subtasks }));
}

function boardGroupPrimaryTask(group) {
  const subtasks = Array.isArray(group?.subtasks) ? group.subtasks : [];
  const primaryId = String(group?.primaryTaskId || "").trim();
  return subtasks.find((task) => String(task?.id || "") === primaryId)
    || subtasks.find((task) => !boardTaskParentId(task))
    || subtasks.find((task) => boardTaskIsPlanningLevel(task))
    || subtasks[0]
    || null;
}

function boardColumnForGroup(group) {
  const status = String(group?.status || "").trim().toLowerCase();
  if (["running", "in_progress", "pausing", "cancelling"].includes(status)) return "active";
  if (["blocked", "failed", "cancelled"].includes(status)) return "blocked";
  if (["done", "completed"].includes(status)) return "done";
  if (status === "backlog") return "backlog";
  if (status === "todo" || status === "pending" || status === "planned") return "todo";
  const subtasks = Array.isArray(group?.subtasks) ? group.subtasks : [];
  if (subtasks.some((task) => boardColumnForTask(task) === "active")) return "active";
  if (subtasks.some((task) => boardColumnForTask(task) === "blocked")) return "blocked";
  if (subtasks.some((task) => boardColumnForTask(task) === "todo")) return "todo";
  if (subtasks.length && subtasks.every((task) => boardColumnForTask(task) === "done")) return "done";
  return "backlog";
}

function renderBoardGroupCard(group) {
  const subtasks = Array.isArray(group?.subtasks) ? group.subtasks : [];
  const primary = boardGroupPrimaryTask(group);
  if (!primary) return "";
  const children = subtasks.filter((task) => String(task?.id || "") !== String(primary?.id || ""));
  const completed = children.filter((task) => boardColumnForTask(task) === "done").length;
  const total = children.length;
  return `
    <article class="board-feature-card" data-board-group-id="${escapeHtml(group.id || boardTaskGroupId(primary))}">
      <header class="board-feature-header">
        <div>
          <span class="badge board-ticket-type">${escapeHtml(statusLabel(boardTaskLevel(primary)))}</span>
          <span class="badge">Feature group</span>
          <strong>${escapeHtml(group.title || primary.title || primary.id || "Work item")}</strong>
        </div>
        <span class="board-feature-progress">${escapeHtml(total ? `${completed}/${total} nested work complete` : "No nested work")}</span>
      </header>
      <div class="board-feature-primary">${renderBoardCard(primary, { nested: true })}</div>
      ${children.length ? `<div class="board-feature-children"><h5>Nested work</h5>${children.map((task) => renderBoardCard(task, { nested: true })).join("")}</div>` : ""}
    </article>
  `;
}

function boardTaskSessionId(task) {
  return String(firstDefined(
    task?.providerSessionId,
    task?.provider_session_id,
    task?.sessionId,
    task?.session_id,
    "",
  ) || "").trim();
}

function boardTaskChatAvailable(task) {
  return Boolean(boardTaskSessionId(task));
}

async function loadBoardSessionTranscript(sessionId) {
  const id = String(sessionId || "").trim();
  if (!id) return [];
  const body = await api(`/api/sessions/${encodeURIComponent(id)}/snapshot?limit=${CHAT_HISTORY_PAGE_SIZE}`);
  const firstPage = Array.isArray(body) ? body : (body?.messages || []);
  const totalCount = Number(body?.total_count ?? body?.totalCount ?? firstPage.length) || firstPage.length;
  let messages = firstPage;
  let offset = Math.max(0, totalCount - firstPage.length);
  while (offset > 0) {
    const nextOffset = Math.max(0, offset - CHAT_HISTORY_PAGE_SIZE);
    const page = await api(
      `/api/sessions/${encodeURIComponent(id)}/messages?limit=${offset - nextOffset}&offset=${nextOffset}`,
    );
    const older = Array.isArray(page) ? page : (page?.messages || []);
    if (!older.length) break;
    messages = older.concat(messages);
    if (older.length >= offset) break;
    offset = nextOffset;
  }
  return messages;
}

async function openBoardTaskTranscript(taskId) {
  const task = (state.board?.tasks || []).find((item) => String(item?.id || "") === String(taskId || ""));
  if (!task) throw new Error("This board task is no longer available. Refresh the board and try again.");
  const embeddedTranscript = Array.isArray(task.transcript) ? task.transcript : [];
  if (embeddedTranscript.length) {
    openBoardTranscriptModal(`${task.title || task.id} transcript`, embeddedTranscript);
    return;
  }
  const sessionId = boardTaskSessionId(task);
  if (!boardTaskChatAvailable(task)) {
    throw new Error("This task does not have a chat session yet.");
  }
  state.boardChatSessionIds.add(sessionId);
  hideBoardChatSessionsFromLists();
  const transcript = await loadBoardSessionTranscript(sessionId);
  openBoardTranscriptModal(`${task.title || task.id} transcript`, transcript);
  hideBoardChatSessionsFromLists();
}

async function openBoardTaskChatSession(taskId) {
  const task = (state.board?.tasks || []).find((item) => String(item?.id || "") === String(taskId || ""));
  if (!task) throw new Error("This board task is no longer available. Refresh the board and try again.");
  const sessionId = boardTaskSessionId(task);
  if (!boardTaskChatAvailable(task)) {
    throw new Error("This task does not have a chat session yet.");
  }
  state.boardChatSessionIds.add(sessionId);
  hideBoardChatSessionsFromLists();
  await pickChatSession(
    sessionId,
    state.board?.projectPath || activeProjectPath(),
    { boardSession: true, forceSnapshot: true },
  );
}

function renderBoardCard(task, options = {}) {
  const nested = options.nested === true;
  const status = String(task.status || "backlog");
  const details = task.details || task.description || task.prompt || "";
  const references = Array.isArray(task.references) ? task.references : [];
  const acceptance = Array.isArray(task.acceptanceCriteria) ? task.acceptanceCriteria : [];
  const sideEffects = boardTaskSideEffects(task);
  const sideEffectsPending = sideEffects.length > 0 && task.sideEffectsApproved !== true;
  const researchPending = boardTaskKind(task) === "research"
    && boardTaskColumnStatus(task) === "done"
    && task.researchAccepted !== true;
  const taskSessionId = boardTaskSessionId(task);
  const transcriptAvailable = Array.isArray(task.transcript) && task.transcript.length > 0;
  const chatSessionAvailable = Boolean(taskSessionId && boardTaskChatAvailable(task));
  const level = boardTaskLevel(task);
  const kind = boardTaskKind(task);
  const breadcrumb = boardTaskBreadcrumb(task, state.board?.tasks || []);
  const blockers = boardTaskBlockers(task);
  const affectedDescendants = boardTaskScopeEffectDescendants(task);
  return `
    <article class="board-card${nested ? " board-card-nested" : ""}" data-board-task-id="${escapeHtml(task.id)}">
      <div class="board-card-topline">
        <span class="badge">${escapeHtml(statusLabel(level))}</span>
        <span class="badge">${escapeHtml(statusLabel(kind))}</span>
        <span>${escapeHtml(task.priority || "medium")}</span>
      </div>
      <h4>${escapeHtml(task.title || task.id || "Task")}</h4>
      ${breadcrumb ? `<p class="board-card-breadcrumb">${escapeHtml(breadcrumb)}</p>` : ""}
      ${details ? `<p>${escapeHtml(details)}</p>` : ""}
      ${blockers.length ? `<p class="board-card-blockers">Blocked by: ${escapeHtml(blockers.join(", "))}</p>` : ""}
      ${acceptance.length ? `<ul>${acceptance.slice(0, 3).map((item) => `<li>${escapeHtml(item)}</li>`).join("")}</ul>` : ""}
      ${references.length ? `<div class="board-card-meta">${references.slice(0, 3).map((item) => `<code>${escapeHtml(item)}</code>`).join("")}</div>` : ""}
      ${sideEffectsPending ? `<p class="board-card-blockers">External side-effect approval required</p>` : ""}
      ${researchPending ? `<p class="board-card-blockers">Research output awaits acceptance</p>` : ""}
      ${affectedDescendants.length && boardTaskColumnStatus(task) !== "done" ? `<p class="board-card-blockers">${affectedDescendants.length} generated descendant${affectedDescendants.length === 1 ? "" : "s"} recorded effects; resolve before changing scope</p>` : ""}
      <div class="board-card-actions">
        <button type="button" data-board-open-details="${escapeHtml(task.id)}">View details</button>
        <button type="button" data-board-discuss-task="${escapeHtml(task.id)}">Discuss</button>
        ${boardTaskCanBreakdown(task) ? `<button type="button" data-board-breakdown-task="${escapeHtml(task.id)}">Breakdown</button>` : ""}
        ${chatSessionAvailable ? `<button type="button" data-board-open-chat-session="${escapeHtml(task.id)}">View chat session</button>` : ""}
        ${transcriptAvailable ? `<button type="button" data-board-view-transcript="${escapeHtml(task.id)}">View transcript</button>` : ""}
        ${boardMoveButton(task, "backlog", "Backlog")}
        ${boardMoveButton(task, "pending", "Todo")}
      </div>
    </article>
  `;
}

function boardMoveButton(task, status, label) {
  const current = String(task?.status || "backlog").trim().toLowerCase();
  const normalizedCurrent = ["pending", "planned"].includes(current) ? "todo" : current;
  const normalizedTarget = status === "pending" ? "todo" : status;
  if (normalizedCurrent === normalizedTarget) return "";
  if (normalizedTarget === "todo" && ["blocked", "failed"].includes(normalizedCurrent)) {
    return `<button type="button" data-board-retry-task="${escapeHtml(task.id)}">Retry transient failure</button>`;
  }
  if (normalizedTarget === "todo" && normalizedCurrent !== "backlog") return "";
  if (normalizedTarget === "backlog" && normalizedCurrent !== "todo") return "";
  return `<button type="button" data-board-task-status="${escapeHtml(status)}" data-board-task-id="${escapeHtml(task.id)}">${escapeHtml(label)}</button>`;
}

function boardColumnForTask(task) {
  const status = String(task.status || "").toLowerCase();
  if (status.startsWith("backlog")) return "backlog";
  if (status === "completed" || status === "done") return "done";
  if (status === "blocked" || status === "failed" || status === "cancelled") return "blocked";
  if (status === "running" || status === "in_progress" || status === "pausing" || status === "cancelling") return "active";
  return "todo";
}

function boardTaskLevel(task) {
  const explicit = String(task?.level || task?.hierarchyLevel || task?.hierarchy_level || "")
    .trim()
    .toLowerCase()
    .replaceAll("-", "_")
    .replaceAll(" ", "_");
  if (["initiative", "epic", "story", "task", "subtask"].includes(explicit)) return explicit;
  return task?.executable || task?.parentId || task?.parent_id ? "subtask" : "story";
}

function boardTaskIsPlanningLevel(task) {
  return ["initiative", "epic", "story"].includes(boardTaskLevel(task));
}

function boardTaskCanBreakdown(task) {
  const level = boardTaskLevel(task);
  const status = String(task?.status || "backlog").trim().toLowerCase();
  const failure = String(firstDefined(task?.error, task?.summary, "") || "").trim()
    .replace(/^Planning error:\s*/i, "");
  const retryablePlanningFailure = ["blocked", "failed"].includes(status)
    && failure.toLowerCase().startsWith("hierarchy breakdown ");
  return ["initiative", "epic", "story", "task"].includes(level)
    && (["backlog", "todo", "pending", "planned"].includes(status) || retryablePlanningFailure);
}

function boardTaskKind(task) {
  const explicit = String(task?.kind || task?.taskType || task?.task_type || "implementation").trim().toLowerCase();
  if (task?.qaFixTask || task?.sourceQaTaskId || explicit === "qa_fix") return "fix";
  if (task?.followupTask || explicit === "followup") return "followup";
  if (task?.qaTask || task?.finalQaTask || task?.taskLevelQa || ["qa", "final_qa"].includes(explicit)) return "qa";
  return explicit.replaceAll("-", "_").replaceAll(" ", "_") || "implementation";
}

function boardTaskColumnStatus(task) {
  return boardColumnForTask(task) === "done" ? "done" : String(task?.status || "backlog").toLowerCase();
}

function boardTaskSideEffects(task) {
  const value = Array.isArray(task?.sideEffects) ? task.sideEffects : [];
  return [...new Set(value.map((item) => {
    if (typeof item === "string") return item.trim();
    return String(item?.name || item?.title || item?.path || item?.description || "").trim();
  }).filter(Boolean))];
}

function boardTaskSideEffectEvidence(task) {
  const value = Array.isArray(task?.sideEffectEvidence) ? task.sideEffectEvidence : [];
  return value.map((item) => typeof item === "string"
    ? item.trim()
    : String(item?.description || item?.summary || item?.name || "").trim()).filter(Boolean);
}

function boardTaskHasRecordedEffects(task) {
  return [
    task?.changedFiles,
    task?.sideEffectEvidence,
  ].some((value) => Array.isArray(value) && value.length > 0);
}

function boardTaskScopeEffectDescendants(task) {
  const rootId = String(task?.id || "").trim();
  if (!rootId) return [];
  const tasks = Array.isArray(state.board?.tasks) ? state.board.tasks : [];
  const descendants = [];
  let frontier = new Set([rootId]);
  const seen = new Set([rootId]);
  while (frontier.size) {
    const children = tasks.filter((candidate) => {
      const candidateId = String(candidate?.id || "").trim();
      return candidateId && !seen.has(candidateId) && frontier.has(boardTaskParentId(candidate));
    });
    if (!children.length) break;
    descendants.push(...children);
    children.forEach((candidate) => seen.add(String(candidate.id)));
    frontier = new Set(children.map((candidate) => String(candidate.id)));
  }
  return descendants.filter(boardTaskHasRecordedEffects);
}

function boardApprovedFixTaskId(task) {
  const failedId = String(task?.id || "").trim();
  if (!failedId) return "";
  const tasks = Array.isArray(state.board?.tasks) ? state.board.tasks : [];
  const fix = tasks.find((candidate) => {
    const sourceId = String(candidate?.sourceTaskId || candidate?.source_task_id || candidate?.sourceQaTaskId || candidate?.source_qa_task_id || "").trim();
    const status = boardTaskColumnStatus(candidate);
    return String(candidate?.id || "").trim()
      && String(candidate.id).trim() !== failedId
      && boardTaskKind(candidate) === "fix"
      && sourceId === failedId
      && ["todo", "active", "done"].includes(status);
  });
  return String(fix?.id || "").trim();
}

function boardTaskIsUserCreatedChild(task) {
  return (task?.manualTask === true || String(task?.taskOrigin || "") === "user_manual")
    && Boolean(boardTaskParentId(task))
    && boardTaskColumnStatus(task) === "backlog";
}

function boardTaskParentId(task) {
  return String(task?.parentId || task?.parent_id || "").trim();
}

function boardTaskBreadcrumbParentId(task) {
  const explicitParentId = boardTaskParentId(task);
  if (explicitParentId) return explicitParentId;
  const sourceId = String(task?.sourceTaskId || task?.source_task_id || "").trim();
  if (!sourceId) return "";
  const kind = boardTaskKind(task);
  return ["qa", "fix", "followup", "review"].includes(kind) ? sourceId : "";
}

function boardTaskBreadcrumb(task, tasks) {
  const byId = new Map((tasks || []).map((item) => [String(item.id || ""), item]));
  const labels = [];
  const seen = new Set();
  let current = task;
  while (current) {
    const id = String(current.id || "");
    if (id && seen.has(id)) break;
    if (id) seen.add(id);
    const title = String(current.title || current.details || current.description || "").trim();
    if (title) labels.unshift(title);
    const parentId = boardTaskBreadcrumbParentId(current);
    current = parentId ? byId.get(parentId) : null;
  }
  return labels.join(" > ");
}

function boardTaskDescendants(task, tasks) {
  const rootId = String(task?.id || "").trim();
  if (!rootId) return [];
  const childrenByParent = new Map();
  (tasks || []).forEach((candidate) => {
    const candidateId = String(candidate?.id || "").trim();
    const parentId = boardTaskParentId(candidate);
    if (!candidateId || !parentId) return;
    if (!childrenByParent.has(parentId)) childrenByParent.set(parentId, []);
    childrenByParent.get(parentId).push(candidate);
  });
  const descendants = [];
  const seen = new Set([rootId]);
  const visit = (parentId, depth) => {
    (childrenByParent.get(parentId) || []).forEach((child) => {
      const childId = String(child?.id || "").trim();
      if (!childId || seen.has(childId)) return;
      seen.add(childId);
      descendants.push({ task: child, depth });
      visit(childId, depth + 1);
    });
  };
  visit(rootId, 0);
  return descendants;
}

function boardTaskBlockers(task) {
  const values = Array.isArray(task?.blockedBy)
    ? task.blockedBy
    : Array.isArray(task?.dependsOn)
      ? task.dependsOn
      : [];
  return [...new Set(values.map((value) => String(value || "").trim()).filter(Boolean))];
}

function renderBoardNestedWork(task) {
  const descendants = boardTaskDescendants(task, state.board?.tasks || []);
  if (!descendants.length) return "";
  return `
    <section class="board-nested-work">
      <h3>Nested work</h3>
      <p class="board-nested-work-note">Tasks and subtasks stay inside this parent detail and do not appear as top-level Backlog cards.</p>
      <div class="board-nested-work-list">
        ${descendants.map(({ task: child, depth }) => {
          const childDetails = child.details || child.description || child.prompt || "";
          const childBlockers = boardTaskBlockers(child);
          const childStatus = boardTaskColumnStatus(child);
          return `
            <article class="board-nested-work-item" style="--board-nested-depth: ${depth};">
              <div class="board-nested-work-heading">
                <div class="board-detail-badges"><span class="badge">${escapeHtml(statusLabel(boardTaskLevel(child)))}</span><span class="badge">${escapeHtml(statusLabel(boardTaskKind(child)))}</span><span class="badge">${escapeHtml(statusLabel(childStatus))}</span></div>
                <strong>${escapeHtml(child.title || child.id || "Nested work")}</strong>
              </div>
              ${childDetails ? `<p>${escapeHtml(childDetails)}</p>` : ""}
              ${childBlockers.length ? `<p class="board-card-blockers">Blocked by: ${escapeHtml(childBlockers.join(", "))}</p>` : ""}
              <button type="button" data-board-open-details="${escapeHtml(child.id)}">View details</button>
              ${boardTaskChatAvailable(child) ? `<button type="button" data-board-open-chat-session="${escapeHtml(child.id)}">View chat session</button>` : ""}
            </article>
          `;
        }).join("")}
      </div>
    </section>
  `;
}

function boardTaskDiscussion(task) {
  return Array.isArray(task?.discussion)
    ? task.discussion
    : Array.isArray(task?.hierarchy?.discussion)
      ? task.hierarchy.discussion
      : [];
}

function boardProposalDiffText(proposal) {
  const changes = Array.isArray(proposal?.diff?.changes) ? proposal.diff.changes : [];
  if (!changes.length) return proposal?.diff?.changed === false ? "No scope fields change." : "No structured field changes reported.";
  return changes.map((change) => {
    const before = JSON.stringify(change?.before ?? null);
    const after = JSON.stringify(change?.after ?? null);
    return `${change?.path || "$"}: ${before} -> ${after}`;
  }).join("\n");
}

function boardProposalMarkup(task) {
  const proposals = boardTaskDiscussion(task)
    .filter((proposal) => proposal?.kind === "proposal" || proposal?.proposalId)
    .slice()
    .reverse();
  if (!proposals.length) return "";
  return `
    <section class="board-proposals">
      <h3>Discussion proposals</h3>
      ${proposals.map((proposal) => {
        const status = String(proposal.status || "pending");
        const warnings = Array.isArray(proposal.warnings) ? proposal.warnings : [];
        const canDecide = status === "pending" && proposal.proposalId;
        return `<article class="board-proposal board-proposal-${escapeHtml(status)}">
          <div class="board-proposal-header"><strong>${escapeHtml(statusLabel(proposal.action || "message"))}</strong><span class="badge">${escapeHtml(statusLabel(status))}</span></div>
          ${proposal.summary ? `<p>${escapeHtml(proposal.summary)}</p>` : ""}
          <pre class="board-proposal-diff">${escapeHtml(boardProposalDiffText(proposal))}</pre>
          ${warnings.length ? `<ul class="board-proposal-warnings">${warnings.map((warning) => `<li>${escapeHtml(warning)}</li>`).join("")}</ul>` : ""}
          ${proposal.error ? `<p class="danger-text">${escapeHtml(proposal.error)}</p>` : ""}
          ${canDecide ? `<div class="board-modal-actions"><button type="button" class="primary-action" data-board-proposal-apply="${escapeHtml(proposal.proposalId)}" data-board-proposal-task="${escapeHtml(task.id)}">Apply</button><button type="button" data-board-proposal-reject="${escapeHtml(proposal.proposalId)}" data-board-proposal-task="${escapeHtml(task.id)}">Reject</button></div>` : ""}
        </article>`;
      }).join("")}
    </section>
  `;
}

function closeBoardModal() {
  qs("#board-modal")?.remove();
}

function boardTranscriptEntryContent(entry) {
  return String(entry?.content || entry?.summary || entry?.text || entry?.toolResult || entry?.toolInput || "").trim()
    || [entry?.kind, entry?.status, entry?.toolName].filter(Boolean).join(" · ");
}

function boardTranscriptEntryRole(entry) {
  const role = String(entry?.role || entry?.kind || "system").toLowerCase();
  if (role === "user" || role === "prompt") return "user";
  if (role === "assistant" || role === "message") return "assistant";
  if (role === "tool" || role.includes("tool")) return "tool";
  return "system";
}

function openBoardTranscriptModal(title, entries) {
  closeBoardModal();
  const safeEntries = Array.isArray(entries) ? entries : [];
  document.body.insertAdjacentHTML("beforeend", `
    <div id="board-modal" class="board-modal" role="dialog" aria-modal="true" aria-label="${escapeHtml(title)}">
      <section class="board-modal-dialog board-transcript-dialog">
        <header class="board-modal-header"><h2>${escapeHtml(title)}</h2></header>
        <div class="board-transcript-messages">
          ${safeEntries.length ? safeEntries.map((entry) => `
            <article class="board-transcript-message board-transcript-${escapeHtml(boardTranscriptEntryRole(entry))}">
              <header><strong>${escapeHtml(statusLabel(boardTranscriptEntryRole(entry)))}</strong><time>${escapeHtml(entry?.timestamp || "")}</time></header>
              <pre>${escapeHtml(boardTranscriptEntryContent(entry))}</pre>
            </article>
          `).join("") : `<p class="empty">No transcript captured yet.</p>`}
        </div>
        <footer class="board-modal-footer"><button type="button" data-board-modal-close>Back</button></footer>
      </section>
    </div>
  `);
  qs("#board-modal")?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget || event.target.closest("[data-board-modal-close]")) closeBoardModal();
  });
}

function openBoardAttemptModal(title, attempt) {
  closeBoardModal();
  const snapshot = attempt && typeof attempt === "object" ? attempt : {};
  document.body.insertAdjacentHTML("beforeend", `
    <div id="board-modal" class="board-modal" role="dialog" aria-modal="true" aria-label="${escapeHtml(title)}">
      <section class="board-modal-dialog board-transcript-dialog">
        <header class="board-modal-header"><h2>${escapeHtml(title)}</h2><button type="button" data-board-modal-close aria-label="Close">Close</button></header>
        <div class="board-modal-body"><pre>${escapeHtml(JSON.stringify(snapshot, null, 2))}</pre></div>
      </section>
    </div>
  `);
  qs("#board-modal")?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget || event.target.closest("[data-board-modal-close]")) closeBoardModal();
  });
}

function openBoardTaskDetails(taskId) {
  const task = (state.board?.tasks || []).find((item) => String(item.id) === String(taskId));
  if (!task) return;
  closeBoardModal();
  const blockers = boardTaskBlockers(task);
  const breadcrumb = boardTaskBreadcrumb(task, state.board?.tasks || []);
  const criteria = Array.isArray(task.acceptanceCriteria) ? task.acceptanceCriteria : [];
  const changedFiles = Array.isArray(task.changedFiles) ? task.changedFiles : [];
  const evidence = Array.isArray(task.evidence) ? task.evidence : [];
  const transcript = Array.isArray(task.transcript) ? task.transcript : [];
  const chatSessionAvailable = Boolean(boardTaskSessionId(task) && boardTaskChatAvailable(task));
  const transcriptAvailable = transcript.length > 0;
  const attempts = Array.isArray(task.attempts) ? task.attempts : [];
  const sideEffects = boardTaskSideEffects(task);
  const sideEffectEvidence = boardTaskSideEffectEvidence(task);
  const sideEffectsApproved = task.sideEffectsApproved === true;
  const requiresSideEffectDeclaration = task.requiresSideEffectDeclaration === true;
  const sideEffectApproval = task.sideEffectApproval || null;
  const supersededBy = String(task.supersededBy || "").trim();
  const researchPending = boardTaskKind(task) === "research"
    && boardTaskColumnStatus(task) === "done"
    && task.researchAccepted !== true;
  const researchItems = Array.isArray(task.result?.proposedPlanningItems) ? task.result.proposedPlanningItems : [];
  const manualEnvironment = task.manualTestEnvironment && typeof task.manualTestEnvironment === "object"
    ? task.manualTestEnvironment
    : null;
  const manualResultObject = task.result && typeof task.result === "object" ? task.result : {};
  const manualSteps = Array.isArray(manualResultObject.manualTestSteps)
    ? manualResultObject.manualTestSteps.map((item) => String(item || "").trim()).filter(Boolean)
    : [];
  const manualResult = String(manualResultObject.manualTestResult || "").trim();
  const affectedDescendants = boardTaskScopeEffectDescendants(task);
  const attentionTask = ["blocked", "failed"].includes(boardTaskColumnStatus(task));
  const approvedFixTaskId = attentionTask ? boardApprovedFixTaskId(task) : "";
  const canApproveSideEffects = sideEffects.length > 0
    && !sideEffectsApproved
    && ["backlog", "todo", "blocked", "failed"].includes(boardTaskColumnStatus(task));
  const canRevokeSideEffectApproval = sideEffects.length > 0
    && sideEffectsApproved
    && ["backlog", "todo", "blocked", "failed"].includes(boardTaskColumnStatus(task));
  const canDeclareSideEffects = requiresSideEffectDeclaration
    && !sideEffects.length
    && task.executable === true
    && boardTaskColumnStatus(task) === "backlog";
  const canDetach = boardTaskIsUserCreatedChild(task);
  document.body.insertAdjacentHTML("beforeend", `
    <div id="board-modal" class="board-modal" role="dialog" aria-modal="true" aria-label="Task details">
      <section class="board-modal-dialog">
        <header class="board-modal-header"><h2>${escapeHtml(task.title || task.id || "Task")}</h2><button type="button" data-board-modal-close aria-label="Close">Close</button></header>
        <div class="board-modal-body">
          <div class="board-detail-badges"><span class="badge">${escapeHtml(statusLabel(boardTaskLevel(task)))}</span><span class="badge">${escapeHtml(statusLabel(boardTaskKind(task)))}</span><span class="badge">${escapeHtml(statusLabel(task.status || "backlog"))}</span><span class="badge">${escapeHtml(task.priority || "medium")}</span></div>
          ${breadcrumb ? `<p class="board-card-breadcrumb">${escapeHtml(breadcrumb)}</p>` : ""}
          <h3>Description</h3><p>${escapeHtml(task.details || task.description || task.prompt || "No description captured.")}</p>
          ${supersededBy ? `<h3>Superseded by</h3><p><code>${escapeHtml(supersededBy)}</code></p>` : ""}
          ${criteria.length ? `<h3>Acceptance criteria</h3><ul>${criteria.map((item) => `<li>${escapeHtml(item)}</li>`).join("")}</ul>` : ""}
          ${blockers.length ? `<h3>Blocked by</h3><p><code>${escapeHtml(blockers.join(", "))}</code></p>` : ""}
          ${task.error ? `<h3>Error</h3><p class="danger-text">${escapeHtml(task.error)}</p>` : ""}
          ${changedFiles.length ? `<h3>Changed files</h3><p><code>${escapeHtml(changedFiles.join("\n"))}</code></p>` : ""}
          ${evidence.length ? `<h3>Evidence</h3><ul>${evidence.map((item) => `<li>${escapeHtml(item)}</li>`).join("")}</ul>` : ""}
          ${sideEffects.length ? `<h3>External side effects</h3><ul>${sideEffects.map((item) => `<li>${escapeHtml(item)}</li>`).join("")}</ul><p>Approval: ${sideEffectsApproved ? "approved" : "pending"}</p>` : requiresSideEffectDeclaration ? `<h3>External side effects</h3><p class="danger-text">Declaration required before this subtask can run.</p>` : ""}
          ${sideEffectEvidence.length ? `<h3>External side-effect evidence</h3><ul>${sideEffectEvidence.map((item) => `<li>${escapeHtml(item)}</li>`).join("")}</ul>` : ""}
          ${manualEnvironment ? `<h3>Manual-test environment</h3><pre>${escapeHtml(JSON.stringify(manualEnvironment, null, 2))}</pre>` : ""}
          ${manualSteps.length ? `<h3>Manual-test steps</h3><ol>${manualSteps.map((item) => `<li>${escapeHtml(item)}</li>`).join("")}</ol>` : ""}
          ${manualResult ? `<h3>Manual-test result</h3><p>${escapeHtml(manualResult)}</p>` : ""}
          ${affectedDescendants.length ? `<h3>Affected generated descendants</h3><ul>${affectedDescendants.map((item) => `<li>${escapeHtml(item.title || item.id)} <code>${escapeHtml(item.id || "")}</code></li>`).join("")}</ul>` : ""}
          ${sideEffectApproval ? `<h3>Side-effect approval audit</h3><pre>${escapeHtml(JSON.stringify(sideEffectApproval, null, 2))}</pre>` : ""}
          ${task.researchAccepted === true ? `<h3>Research acceptance</h3><pre>${escapeHtml(JSON.stringify(task.researchAcceptance || {}, null, 2))}</pre>` : ""}
          ${researchItems.length ? `<h3>Proposed planning items</h3><ul>${researchItems.map((item) => `<li>${escapeHtml(item?.title || item?.details || "Planning item")}</li>`).join("")}</ul>` : ""}
          ${renderBoardNestedWork(task)}
          ${boardProposalMarkup(task)}
          ${attempts.length ? `<h3>Attempts</h3><ol>${attempts.map((attempt, index) => `<li>${escapeHtml(`Attempt ${index + 1} · ${statusLabel(attempt?.status || "unknown")}`)} <button type="button" data-board-view-attempt="${escapeHtml(task.id)}" data-board-attempt-index="${index}">View attempt</button></li>`).join("")}</ol>` : ""}
          <div class="board-modal-actions">
            <button type="button" data-board-discuss-task="${escapeHtml(task.id)}">Discuss</button>
            ${boardTaskCanBreakdown(task) ? `<button type="button" data-board-breakdown-task="${escapeHtml(task.id)}">Breakdown</button>` : ""}
            ${chatSessionAvailable ? `<button type="button" data-board-open-chat-session="${escapeHtml(task.id)}">View chat session</button>` : ""}
            ${canDeclareSideEffects ? `<button type="button" class="primary-action" data-board-side-effects-declare="${escapeHtml(task.id)}">Declare external side effects</button>` : ""}
            ${canApproveSideEffects ? `<button type="button" class="primary-action" data-board-side-effects-approve="${escapeHtml(task.id)}">Approve external side effects</button>` : ""}
            ${canRevokeSideEffectApproval ? `<button type="button" data-board-side-effects-revoke="${escapeHtml(task.id)}">Revoke side-effect approval</button>` : ""}
            ${researchPending ? `<button type="button" class="primary-action" data-board-research-accept="${escapeHtml(task.id)}">Accept research output</button>` : ""}
            ${canDetach ? `<button type="button" data-board-detach-task="${escapeHtml(task.id)}">Detach as Backlog story</button>` : ""}
            ${attentionTask ? `<button type="button" data-board-retry-task="${escapeHtml(task.id)}">Retry transient failure</button>` : ""}
            ${approvedFixTaskId ? `<button type="button" class="primary-action" data-board-retry-fix-task="${escapeHtml(task.id)}" data-board-retry-fix-id="${escapeHtml(approvedFixTaskId)}">Retry with approved fix</button>` : ""}
            ${affectedDescendants.length && boardTaskColumnStatus(task) !== "done" ? `<button type="button" data-board-scope-effects="keep" data-board-scope-effects-task="${escapeHtml(task.id)}">Keep recorded changes</button><button type="button" data-board-scope-effects="revert" data-board-scope-effects-task="${escapeHtml(task.id)}">Create revert work</button><button type="button" data-board-scope-effects="cleanup" data-board-scope-effects-task="${escapeHtml(task.id)}">Create cleanup work</button>` : ""}
            ${transcriptAvailable ? `<button type="button" data-board-view-transcript="${escapeHtml(task.id)}">View transcript</button>` : ""}
          </div>
        </div>
      </section>
    </div>
  `);
  qs("#board-modal")?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget || event.target.closest("[data-board-modal-close]")) closeBoardModal();
    const nestedDetailsButton = event.target.closest("[data-board-open-details]");
    if (nestedDetailsButton) {
      openBoardTaskDetails(nestedDetailsButton.dataset.boardOpenDetails);
      return;
    }
    const applyButton = event.target.closest("[data-board-proposal-apply]");
    if (applyButton) {
      withButtonLoading(applyButton, () => resolveBoardDiscussionProposal(
        applyButton.dataset.boardProposalTask,
        applyButton.dataset.boardProposalApply,
        "apply",
      )).catch(showError);
      return;
    }
    const rejectButton = event.target.closest("[data-board-proposal-reject]");
    if (rejectButton) {
      withButtonLoading(rejectButton, () => resolveBoardDiscussionProposal(
        rejectButton.dataset.boardProposalTask,
        rejectButton.dataset.boardProposalReject,
        "reject",
      )).catch(showError);
      return;
    }
    const transcriptButton = event.target.closest("[data-board-view-transcript]");
    if (transcriptButton) {
      openBoardTaskTranscript(transcriptButton.dataset.boardViewTranscript).catch(showError);
      return;
    }
    const chatSessionButton = event.target.closest("[data-board-open-chat-session]");
    if (chatSessionButton) {
      openBoardTaskChatSession(chatSessionButton.dataset.boardOpenChatSession).catch(showError);
      return;
    }
    const attemptButton = event.target.closest("[data-board-view-attempt]");
    if (attemptButton) {
      const index = Number.parseInt(attemptButton.dataset.boardAttemptIndex || "-1", 10);
      const attempt = Number.isInteger(index) && index >= 0 ? attempts[index] : null;
      if (attempt) openBoardAttemptModal(`${task.title || task.id} · attempt ${index + 1}`, attempt);
      return;
    }
    const discussButton = event.target.closest("[data-board-discuss-task]");
    if (discussButton) openBoardDiscussionModal(task.id);
    const breakdownButton = event.target.closest("[data-board-breakdown-task]");
    if (breakdownButton) {
      withButtonLoading(breakdownButton, () => breakdownBoardTask(task.id)).catch(showError);
      return;
    }
    const approveButton = event.target.closest("[data-board-side-effects-approve]");
    if (approveButton) {
      if (!window.confirm("Approve the declared external side effects for this subtask?")) return;
      withButtonLoading(approveButton, () => approveBoardTaskSideEffects(task.id, true)).catch(showError);
      return;
    }
    const declareButton = event.target.closest("[data-board-side-effects-declare]");
    if (declareButton) {
      const raw = window.prompt("List one possible external side effect per line", "");
      if (raw === null) return;
      const values = raw.split(/\r?\n/).map((value) => value.trim()).filter(Boolean);
      if (!values.length) {
        showToast("Declare at least one possible external side effect", "error");
        return;
      }
      withButtonLoading(
        declareButton,
        () => declareBoardTaskSideEffects(task.id, values),
      ).catch(showError);
      return;
    }
    const revokeButton = event.target.closest("[data-board-side-effects-revoke]");
    if (revokeButton) {
      if (!window.confirm("Revoke external side-effect approval?")) return;
      withButtonLoading(revokeButton, () => approveBoardTaskSideEffects(task.id, false)).catch(showError);
      return;
    }
    const researchButton = event.target.closest("[data-board-research-accept]");
    if (researchButton) {
      const note = window.prompt("Optional research acceptance note", "") ?? "";
      withButtonLoading(researchButton, () => acceptBoardTaskResearch(task.id, note)).catch(showError);
      return;
    }
    const detachButton = event.target.closest("[data-board-detach-task]");
    if (detachButton) {
      if (!window.confirm("Detach this user-created child and preserve it as a Backlog story?")) return;
      withButtonLoading(detachButton, () => detachBoardTask(task.id)).catch(showError);
      return;
    }
    const retryButton = event.target.closest("[data-board-retry-task]");
    if (retryButton) {
      if (!window.confirm("Retry this task as a transient/environment failure?")) return;
      withButtonLoading(
        retryButton,
        () => retryBoardTask(retryButton.dataset.boardRetryTask || task.id),
      ).catch(showError);
      return;
    }
    const retryFixButton = event.target.closest("[data-board-retry-fix-task]");
    if (retryFixButton) {
      if (!window.confirm("Retry this task after applying the approved linked fix?")) return;
      withButtonLoading(
        retryFixButton,
        () => retryBoardTask(
          retryFixButton.dataset.boardRetryFixTask || task.id,
          "fix",
          retryFixButton.dataset.boardRetryFixId || "",
        ),
      ).catch(showError);
      return;
    }
    const scopeEffectsButton = event.target.closest("[data-board-scope-effects]");
    if (scopeEffectsButton) {
      const decision = scopeEffectsButton.dataset.boardScopeEffects;
      const label = decision === "keep"
        ? "Keep the recorded changes"
        : decision === "revert"
          ? "Create explicit revert work"
          : "Create explicit cleanup work";
      if (!window.confirm(`${label} for the recorded effects of this scope?`)) return;
      withButtonLoading(
        scopeEffectsButton,
        () => resolveBoardTaskScopeEffects(
          scopeEffectsButton.dataset.boardScopeEffectsTask || task.id,
          decision,
        ),
      ).catch(showError);
    }
  });
}

function openBoardDiscussionModal(taskId) {
  const task = (state.board?.tasks || []).find((item) => String(item.id) === String(taskId));
  if (!task) return;
  closeBoardModal();
  document.body.insertAdjacentHTML("beforeend", `
    <div id="board-modal" class="board-modal" role="dialog" aria-modal="true" aria-label="Discuss task">
      <section class="board-modal-dialog">
        <header class="board-modal-header"><h2>Discuss · ${escapeHtml(task.title || task.id)}</h2><button type="button" data-board-modal-close aria-label="Close">Close</button></header>
        <form id="board-discussion-form" class="board-modal-body">
          <label><span>Action</span><select name="action">
            <option value="">Discuss</option><option value="edit">Edit scope</option><option value="replace">Replace scope</option><option value="reprioritize">Reprioritize</option><option value="delete">Delete</option><option value="split">Split</option><option value="merge">Merge</option><option value="regenerate_children">Regenerate children</option><option value="re_research">Re-research</option><option value="revision">Create revision</option><option value="fix">Create fix</option><option value="replacement">Create replacement</option>
          </select></label>
          <label><span>Message or action data</span><textarea name="message" rows="7" placeholder="Explain the change or provide action data"></textarea></label>
          <p class="board-discussion-note">The AI will prepare a proposal for review. Nothing changes until you apply it.</p>
          <div class="board-modal-actions"><button type="submit" class="primary-action">Prepare proposal</button><button type="button" data-board-modal-close>Cancel</button></div>
        </form>
      </section>
    </div>
  `);
  const modal = qs("#board-modal");
  modal?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget || event.target.closest("[data-board-modal-close]")) closeBoardModal();
  });
  qs("#board-discussion-form")?.addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.currentTarget;
    const action = form.elements.action.value;
    const message = form.elements.message.value.trim();
    if (!message && !action) throw new Error("Write a discussion message.");
    let payload = {};
    if (["edit", "replace"].includes(action)) payload = { details: message };
    if (action === "reprioritize") payload = { priority: message };
    if (action === "merge") payload = { targetId: message };
    if (action === "split") payload = { items: message.split(/\r?\n/).map((title) => title.trim()).filter(Boolean).map((title) => ({ title, details: title })) };
    if (action === "re_research") payload = { title: "Research revised direction", details: message };
    if (["revision", "fix", "replacement"].includes(action)) payload = { title: `${statusLabel(action)} for completed item`, details: message, kind: action, ...(action === "replacement" ? { supersedeSource: true } : {}) };
    const response = await api(`/api/danger/boards/${encodeURIComponent(state.board.id)}/tasks/${encodeURIComponent(taskId)}/discussion`, {
      method: "POST",
      body: JSON.stringify({ message, action, payload }),
    });
    closeBoardModal();
    await loadBoard();
    openBoardTaskDetails(taskId);
    if (response?.success === false) showToast("Discussion proposal could not be prepared", "error");
  });
}

async function resolveBoardDiscussionProposal(taskId, proposalId, decision) {
  const run = state.board;
  if (!run?.id || !taskId || !proposalId) return;
  await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/${encodeURIComponent(taskId)}/discussion/${encodeURIComponent(proposalId)}/${decision}`, {
    method: "POST",
  });
  closeBoardModal();
  await loadBoard();
  openBoardTaskDetails(taskId);
}

function statusLabel(status) {
  return String(status || "backlog")
    .replaceAll("_", " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function multilineInputLines(selector) {
  return (qs(selector)?.value || "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

async function createBoard(event) {
  event.preventDefault();
  const projectPath = activeProjectPath();
  if (!projectPath) throw new Error("Select a project before creating a board.");
  const prompt = qs("#board-start-prompt")?.value.trim() || "";
  if (!prompt) throw new Error("Enter a board prompt.");
  const provider = qs("#board-provider")?.value || "claude";
  const model = qs("#board-model")?.value.trim() || "";
  const boardProfile = qs("#board-profile")?.value || "complete_app";
  const strategyMode = qs("#board-model-strategy")?.value || "manual";
  const sessionPolicy = qs("#board-session-policy")?.value || "continuous";
  const gitPolicy = qs("#board-git-policy")?.value || "read_only";
  const cheapModel = qs("#board-cheap-model")?.value.trim() || "";
  const expensiveModel = qs("#board-expensive-model")?.value.trim() || "";
  const tddEnabled = qs("#board-tdd-enabled")?.value !== "false";
  const tddBaseline = qs("#board-tdd-baseline")?.value !== "false";
  const tddAllowNoTests = qs("#board-tdd-allow-no-tests")?.value === "true";
  const tddMaxFixes = numericInputValue("#board-tdd-max-fixes", 3, 0, 20);
  const validationEnabled = qs("#board-validation-enabled")?.value !== "false";
  const validationTimeout = numericInputValue("#board-validation-timeout", 120, 5, 3600);
  const validationFeatureCommands = multilineInputLines("#board-validation-feature-commands");
  const validationFinalCommands = multilineInputLines("#board-validation-final-commands");
  const validationQaCommands = multilineInputLines("#board-validation-qa-commands");
  const validationMaxFeature = numericInputValue("#board-validation-max-feature", 2, 0, 20);
  const validationMaxFinal = numericInputValue("#board-validation-max-final", 4, 0, 20);
  const validationMaxQa = numericInputValue("#board-validation-max-qa", 2, 0, 20);
  const ragEnabled = qs("#board-rag-enabled")?.value !== "false";
  const ragContextChars = numericInputValue("#board-rag-context-chars", 12000, 1000, 80000);
  const qaMode = qs("#board-qa-mode")?.value || "high_risk";
  const qaFollowups = numericInputValue("#board-qa-followups", 3, 0, 20);
  const qaAttempts = numericInputValue("#board-qa-attempts", 2, 1, 10);
  const repairMalformedToolCalls = qs("#board-tool-repair-enabled")?.value !== "false";
  const toolRepairRetries = numericInputValue("#board-tool-repair-retries", 1, 0, 3);
  const autoRetryEnabled = qs("#board-auto-retry-enabled")?.value === "true";
  const autoRetryDelay = numericInputValue("#board-auto-retry-delay", 10, 1, 1440);
  const autoRetryAttempts = numericInputValue("#board-auto-retry-attempts", 3, 1, 100);
  await api("/api/danger/boards", {
    method: "POST",
    body: JSON.stringify({
      command: prompt,
      projectPath,
      projectName: activeProjectName() || selectedProjectLabel("#active-project"),
      provider,
      model,
      boardProfile,
      sessionPolicy,
      gitPolicy,
      modelStrategy: {
        mode: strategyMode,
        cheapModel,
        expensiveModel,
      },
      tddEnabled,
      tddPolicy: {
        requireFailingTestBeforeDev: tddBaseline,
        allowImplementationWithoutTests: tddAllowNoTests,
        maxFixAttempts: tddMaxFixes,
      },
      validationConfig: {
        enabled: validationEnabled,
        featureCommands: validationFeatureCommands,
        finalCommands: validationFinalCommands,
        qaCommands: validationQaCommands,
        maxFeatureCommands: validationMaxFeature,
        maxFinalCommands: validationMaxFinal,
        maxQaCommands: validationMaxQa,
        timeoutSeconds: validationTimeout,
      },
      ragSettings: {
        enabled: ragEnabled,
        queryEnabled: ragEnabled,
        indexOnBootstrap: ragEnabled,
        contextMaxChars: ragContextChars,
      },
      qaPolicy: {
        taskQaMode: qaMode,
        maxFollowupsPerGroup: qaFollowups,
        maxTaskAttempts: qaAttempts,
        repairMalformedToolCalls,
        malformedToolCallRepairRetries: toolRepairRetries,
      },
      autoRetry: {
        enabled: autoRetryEnabled,
        delayMinutes: autoRetryDelay,
        maxAttempts: autoRetryAttempts,
      },
    }),
  });
  qs("#board-start-prompt").value = "";
  await loadBoard();
  showToast("Board updated", "ok");
}

async function addBoardTask(event) {
  event.preventDefault();
  const run = state.board;
  if (!run?.id) throw new Error("Create a board first.");
  const prompt = qs("#board-task-prompt")?.value.trim() || "";
  if (!prompt) throw new Error("Enter a task prompt.");
  const lines = prompt.split(/\n+/).map((line) => line.trim()).filter(Boolean);
  if (lines.length > 1) {
    await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/backlog-from-prompt`, {
      method: "POST",
      body: JSON.stringify({ prompt }),
    });
  } else {
    await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks`, {
      method: "POST",
      body: JSON.stringify({ prompt, status: "backlog" }),
    });
  }
  qs("#board-task-prompt").value = "";
  await loadBoard();
  showToast("Task added", "ok");
}

async function moveBoardTask(taskId, status) {
  const run = state.board;
  if (!run?.id || !taskId) return;
  if (status === "pending") {
    await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/${encodeURIComponent(taskId)}/promote`, { method: "POST" });
  } else if (status === "backlog") {
    await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/${encodeURIComponent(taskId)}/demote`, { method: "POST" });
  } else {
    await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/${encodeURIComponent(taskId)}`, {
      method: "PATCH",
      body: JSON.stringify({ status }),
    });
  }
  await loadBoard();
}

async function retryBoardTask(taskId, mode = "transient", fixTaskId = "") {
  const run = state.board;
  if (!run?.id || !taskId) return;
  const body = {
    taskIds: [taskId],
    mode,
    ...(fixTaskId ? { fixTaskId } : {}),
    reason: mode === "fix"
      ? "User approved the linked fix subtask"
      : "User requested a transient failure retry",
  };
  await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/retry-attention`, {
    method: "POST",
    body: JSON.stringify(body),
  });
  closeBoardModal();
  await loadBoard();
  openBoardTaskDetails(taskId);
}

async function breakdownBoardTask(taskId) {
  const run = state.board;
  if (!run?.id || !taskId) return;
  await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/${encodeURIComponent(taskId)}/breakdown`, {
    method: "POST",
    body: JSON.stringify({}),
  });
  closeBoardModal();
  await loadBoard();
  showToast("Breakdown added to the board", "ok");
}

async function approveBoardTaskSideEffects(taskId, approved = true, note = "") {
  const run = state.board;
  if (!run?.id || !taskId) return;
  await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/${encodeURIComponent(taskId)}/side-effects/approve`, {
    method: "POST",
    body: JSON.stringify({ approved, ...(note.trim() ? { note: note.trim() } : {}) }),
  });
  closeBoardModal();
  await loadBoard();
  openBoardTaskDetails(taskId);
  showToast(approved ? "External side effects approved" : "External side-effect approval revoked", "ok");
}

async function declareBoardTaskSideEffects(taskId, sideEffects) {
  const run = state.board;
  if (!run?.id || !taskId || !Array.isArray(sideEffects) || !sideEffects.length) return;
  await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/${encodeURIComponent(taskId)}`, {
    method: "PATCH",
    body: JSON.stringify({ sideEffects }),
  });
  closeBoardModal();
  await loadBoard();
  openBoardTaskDetails(taskId);
  showToast("External side-effect declaration saved", "ok");
}

async function acceptBoardTaskResearch(taskId, note = "") {
  const run = state.board;
  if (!run?.id || !taskId) return;
  await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/${encodeURIComponent(taskId)}/research/accept`, {
    method: "POST",
    body: JSON.stringify(note.trim() ? { note: note.trim() } : {}),
  });
  closeBoardModal();
  await loadBoard();
  openBoardTaskDetails(taskId);
  showToast("Research output accepted; planning remains in Backlog", "ok");
}

async function detachBoardTask(taskId) {
  const run = state.board;
  if (!run?.id || !taskId) return;
  await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/${encodeURIComponent(taskId)}/detach`, {
    method: "POST",
    body: JSON.stringify({}),
  });
  closeBoardModal();
  await loadBoard();
  openBoardTaskDetails(taskId);
  showToast("User-created child detached as a Backlog story", "ok");
}

async function resolveBoardTaskScopeEffects(taskId, decision, note = "") {
  const run = state.board;
  if (!run?.id || !taskId) return;
  await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/${encodeURIComponent(taskId)}/scope-effects/resolve`, {
    method: "POST",
    body: JSON.stringify({ decision, ...(note.trim() ? { note: note.trim() } : {}) }),
  });
  closeBoardModal();
  await loadBoard();
  openBoardTaskDetails(taskId);
  showToast("Recorded child effects resolved", "ok");
}

async function deleteBoardTask(taskId) {
  const run = state.board;
  if (!run?.id || !taskId) return;
  await api(`/api/danger/boards/${encodeURIComponent(run.id)}/tasks/${encodeURIComponent(taskId)}`, { method: "DELETE" });
  await loadBoard();
  showToast("Task deleted", "ok");
}

async function boardAction(action) {
  const run = state.board;
  if (!run?.id) throw new Error("Create a board first.");
  const body = action === "pause"
    ? { reason: "user request" }
    : action === "abort"
      ? { reason: "user request" }
      : {};
  await api(`/api/danger/boards/${encodeURIComponent(run.id)}/${action}`, {
    method: "POST",
    body: JSON.stringify(body),
  });
  await loadBoard();
  showToast(action === "resume" ? "Board started" : action === "pause" ? "Board paused" : "Board aborted", "ok");
}

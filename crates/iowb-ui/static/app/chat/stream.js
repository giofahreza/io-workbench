let chatMetadataModalReturnFocus = null;

function selectedRunningChatSession() {
  const ids = chatEventSessionIds();
  for (const sessionId of ids) {
    const live = state.sessionStatusById?.[sessionId];
    if (normalizeSidebarSessionStatus(live?.status) !== "running") continue;
    const session = findChatSession(sessionId);
    return {
      provider: live?.provider || sessionProvider(session),
      sessionId,
    };
  }
  if (state.currentSession?.sessionId && ids.has(state.currentSession.sessionId)) {
    return state.currentSession;
  }
  return null;
}

function requestAbortSelectedChatSession(selectedRun = selectedRunningChatSession()) {
  if (!selectedRun) return false;
  if (state.ws?.readyState !== WebSocket.OPEN) {
    showError(new Error("Chat connection is not ready."));
    return false;
  }
  state.chatStoppingSessionId = selectedRun.sessionId;
  updateProcessingLabel("Stopping");
  rememberCurrentChatSession({ sessionId: selectedRun.sessionId, live: true, status: "running" });
  state.ws.send(JSON.stringify({
    type: "abort_session",
    provider: selectedRun.provider,
    sessionId: selectedRun.sessionId,
  }));
  updateChatComposerState();
  scheduleChatReconciliation(selectedRun.sessionId, { delayMs: CHAT_ACTIVE_POLL_INTERVAL_MS });
  return true;
}

function sessionMetaForStatus(payload = {}) {
  const sid = payload.sessionId || state.chatSessionId || state.pendingChatSessionId;
  const persisted = getSessionOverridesFor(sid) || {};
  const normalized = normalizeMessageMeta(persisted);
  return {
    ...normalized,
    cli: payload.provider || normalized.cli || state.preferences.chatCli || state.preferences.chatProvider || "codex",
    model: normalized.model || state.preferences.chatModel || "",
    mode: normalized.mode || state.preferences.chatMode || "",
    effort: normalized.effort || state.preferences.chatEffort || "",
    thinking: normalized.thinking ?? state.preferences.chatThinking,
    fast: normalized.fast ?? state.preferences.chatFast ?? false,
  };
}

function setProcessingText(textNode, label = "Sending") {
  if (!textNode) return;
  textNode.innerHTML = `<span class="chat-processing-label">${escapeHtml(label)}</span><span class="chat-processing-dots" aria-hidden="true"></span>`;
}

function updateProcessingLabel(label) {
  const processing = state.chatProcessing;
  if (!processing?.node?.isConnected || !processing.text) return;
  const existing = processing.text.querySelector(".chat-processing-label");
  if (existing && existing.textContent === label) return;
  setProcessingText(processing.text, label);
}

function ensureChatProcessing(payload = {}) {
  if (!isActiveChatSessionEvent(payload)) return null;
  const output = chatOutputRoot();
  if (!output) return null;
  if (chatStream.node?.isConnected && chatStream.role === "assistant" && state.chatBuffer) {
    return null;
  }
  if (state.chatProcessing?.node?.isConnected) {
    renderChatLineFooter(state.chatProcessing.footer, null);
    scrollChatToBottom();
    updateChatEmptyState();
    return state.chatProcessing;
  }
  const line = buildChatLineNode("assistant");
  line.node.classList.add("chat-line-processing");
  line.node.dataset.processingSessionId = payload.sessionId || "";
  setProcessingText(line.text);
  renderChatLineFooter(line.footer, null);
  output.appendChild(line.node);
  state.chatProcessing = {
    provider: payload.provider || "",
    sessionId: payload.sessionId || "",
    ...line,
  };
  scrollChatToBottom();
  updateChatEmptyState();
  return state.chatProcessing;
}

function adoptChatProcessingForStream() {
  const processing = state.chatProcessing;
  if (!processing?.node?.isConnected) return false;
  processing.node.classList.remove("chat-line-processing");
  delete processing.node.dataset.processingSessionId;
  if (processing.text) processing.text.textContent = "";
  chatStream = {
    role: "assistant",
    node: processing.node,
    text: processing.text,
    footer: processing.footer,
    buffer: "",
  };
  state.chatProcessing = null;
  return true;
}

function clearChatProcessing() {
  const processing = state.chatProcessing;
  if (processing?.node?.isConnected) processing.node.remove();
  state.chatProcessing = null;
}

function finishChatProcessing(payload = {}, label = "") {
  if (!state.chatProcessing?.node?.isConnected) return false;
  if (payload.sessionId && state.chatProcessing.sessionId && payload.sessionId !== state.chatProcessing.sessionId) {
    return false;
  }
  if (!label) {
    clearChatProcessing();
    updateChatEmptyState();
    return true;
  }
  state.chatProcessing.node.classList.remove("chat-line-processing");
  state.chatProcessing.node.classList.add("chat-line-system");
  state.chatProcessing.text.textContent = label;
  renderChatLineFooter(state.chatProcessing.footer, null);
  state.chatProcessing = null;
  scrollChatToBottom();
  updateChatEmptyState();
  return true;
}

function renderChatLineFooter(footer, meta) {
  if (!footer) return;
  const normalized = normalizedChatMessageMeta(meta || {});
  const summary = chatLineFooterSummary(normalized);
  let button = footer.querySelector("[data-chat-line-footer-details]");
  if (!button) {
    footer.innerHTML = `<button type="button" class="chat-line-footer-details" data-chat-line-footer-details aria-haspopup="dialog" aria-label="Show message details" title="Show message details">` +
      `<span class="chat-line-footer-details-label">details</span>` +
      `<span class="chat-line-footer-details-preview"></span>` +
      `</button>`;
    button = footer.querySelector("[data-chat-line-footer-details]");
    button.addEventListener("click", (event) => {
      openChatMetadataModal(button._iowbMetadata || {}, event.currentTarget);
    });
  }
  button._iowbMetadata = normalized;
  const preview = button.querySelector(".chat-line-footer-details-preview");
  if (preview.textContent !== summary) preview.textContent = summary;
}

function chatLineFooterSummary(meta = {}) {
  const summary = [];
  const identity = meta.model || meta.cli || meta.provider;
  if (identity) summary.push(identity);
  if (meta.mode) summary.push(meta.mode);
  if (meta.fast === true) summary.push("fast");
  if (meta.tokenUsage) summary.push(meta.tokenUsage);
  if (meta.status) summary.push(meta.status);
  if (meta.elapsed) summary.push(meta.elapsed);
  if (!summary.length && meta.receivedAt) summary.push(meta.receivedAt);
  return summary.join(" · ") || "Details";
}

function chatMetadataText(value) {
  if (value === undefined || value === null || value === "") return "";
  if (typeof value === "object") {
    try {
      return JSON.stringify(value, null, 2);
    } catch {
      return String(value);
    }
  }
  return String(value);
}

function chatMetadataLabel(key) {
  return String(key || "")
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function chatMetadataHasValue(value) {
  return value !== undefined && value !== null && value !== "";
}

function chatMetadataFirst(source, ...keys) {
  if (!source || typeof source !== "object") return undefined;
  for (const key of keys) {
    if (chatMetadataHasValue(source[key])) return source[key];
  }
  return undefined;
}

function chatMetadataBoolean(value) {
  if (value === true) return "Yes";
  if (value === false) return "No";
  return "—";
}

function chatMetadataTime(value) {
  if (!chatMetadataHasValue(value)) return "—";
  if (typeof formatReceivedDateTime === "function") {
    const parsed = new Date(value);
    if (!Number.isNaN(parsed.getTime())) return formatReceivedDateTime(value);
  }
  return chatMetadataText(value);
}

function chatMetadataRuntimeLabel(value) {
  const runtime = String(value || "").toLowerCase();
  if (runtime === "native_cli" || runtime === "native-cli") return "Native CLI";
  if (runtime === "io_gateway" || runtime === "io-gateway") return "IO Gateway";
  return chatMetadataHasValue(value) ? String(value) : "—";
}

function chatMetadataSessionContext(meta = {}) {
  const appState = typeof state !== "undefined" && state ? state : {};
  const sessionId = String(
    meta.sessionId
      || appState.chatSessionId
      || appState.pendingChatSessionId
      || appState.currentSession?.sessionId
      || "",
  ).trim();
  let liveSession = null;
  if (sessionId && typeof findChatSession === "function") liveSession = findChatSession(sessionId);
  let cachedSession = null;
  if (sessionId && typeof cachedChatSession === "function") cachedSession = cachedChatSession(sessionId)?.session || null;
  const session = liveSession || cachedSession || null;
  const projectPath = session?.projectPath || session?.project_path || "";
  const project = (appState.projects || []).find((candidate) => (
    candidate?.path === projectPath
  )) || null;
  const live = sessionId ? appState.sessionStatusById?.[sessionId] || {} : {};
  const response = sessionId ? appState.chatResponseStateBySession?.[sessionId] || {} : {};
  const chatWindow = typeof chatHistoryWindow !== "undefined" && chatHistoryWindow?.sessionId === sessionId
    ? chatHistoryWindow
    : null;
  const chatOpen = Boolean(sessionId && (
    appState.chatSessionId === sessionId || appState.pendingChatSessionId === sessionId
  ));
  const recovery = sessionId ? appState.chatRecoveryBySession?.[sessionId] || {} : {};
  const compaction = sessionId ? appState.chatManualCompactionBySession?.[sessionId] || {} : {};
  let pinned = false;
  if (session && typeof isChatSessionPinned === "function") {
    try {
      pinned = isChatSessionPinned(session, projectPath);
    } catch {
      pinned = false;
    }
  }
  const rawStatus = chatMetadataFirst(live, "status")
    || chatMetadataFirst(session, "status")
    || (appState.currentSession?.sessionId === sessionId ? "running" : "");
  const normalizedStatus = typeof normalizeSidebarSessionStatus === "function"
    ? normalizeSidebarSessionStatus(rawStatus)
    : String(rawStatus || "").toLowerCase();
  const currentResponse = appState.currentSession?.sessionId === sessionId;
  const processing = appState.chatProcessing?.sessionId === sessionId;
  const running = ["starting", "running", "waiting-for-input"].includes(String(rawStatus).toLowerCase())
    || normalizedStatus === "running"
    || currentResponse
    || processing;
  const responseState = {
    ...response,
    activeResponseId: response.activeResponseId || live.responseId || recovery.responseId || compaction.responseId || "",
    sequence: response.sequence ?? live.sequence ?? 0,
  };
  return {
    appState,
    sessionId,
    session,
    project,
    live,
    response: responseState,
    chatWindow,
    chatOpen,
    recovery,
    compaction,
    pinned,
    rawStatus,
    normalizedStatus,
    running,
    processing,
  };
}

function chatMetadataRowEntries(definitions = [], options = {}) {
  return definitions.map((definition) => {
    const [label, value, rowOptions = {}] = definition;
    const present = chatMetadataHasValue(value);
    if (!present && options.includeEmpty !== true && rowOptions.includeEmpty !== true) return null;
    const text = present ? chatMetadataText(value) : (rowOptions.empty || options.empty || "—");
    return {
      label,
      text,
      code: rowOptions.code === true || typeof value === "object" || text.includes("\n"),
    };
  }).filter(Boolean);
}

function chatMetadataTokenValue(usage, key) {
  if (!usage || typeof usage !== "object") return undefined;
  const aliases = {
    used: ["used", "totalUsed"],
    total: ["total", "used"],
    input: ["input", "inputTokens"],
    output: ["output", "outputTokens"],
    cacheCreation: ["cacheCreation", "cache_creation", "cacheCreate", "cacheCreationTokens"],
    cacheRead: ["cacheRead", "cache_read", "cacheReadTokens"],
    reasoning: ["reasoning", "reasoningTokens"],
    costUsd: ["costUsd", "cost_usd", "cost"],
    completeness: ["completeness"],
    partialAttempts: ["partialAttempts", "partial_attempts"],
    missingAttempts: ["missingAttempts", "missing_attempts"],
    afterCompact: ["afterCompact", "after_compact"],
    compactedAt: ["compactedAt", "compacted_at"],
  };
  const direct = chatMetadataFirst(usage, ...(aliases[key] || [key]));
  if (chatMetadataHasValue(direct)) return direct;
  const breakdown = usage.breakdown;
  if (breakdown && typeof breakdown === "object") {
    return chatMetadataFirst(breakdown, ...(aliases[key] || [key]));
  }
  return undefined;
}

function chatMetadataTokenCount(value) {
  if (!chatMetadataHasValue(value)) return "—";
  const numeric = Number(value);
  return Number.isFinite(numeric) ? numeric.toLocaleString() : chatMetadataText(value);
}

function chatMetadataTokenCost(value) {
  if (!chatMetadataHasValue(value)) return "—";
  const numeric = Number(value);
  return Number.isFinite(numeric) ? `USD ${numeric.toFixed(6)}` : chatMetadataText(value);
}

function chatMetadataTokenRows(usage, kind = "current") {
  if (!usage || typeof usage !== "object") return [{ label: "Usage", text: "Unavailable", code: false }];
  const rows = [];
  const addCount = (label, key) => rows.push({ label, text: chatMetadataTokenCount(chatMetadataTokenValue(usage, key)) });
  const addCost = (label = "Cost") => rows.push({ label, text: chatMetadataTokenCost(chatMetadataTokenValue(usage, "costUsd")) });
  if (kind === "current") {
    addCount("Used", "used");
    if (chatMetadataHasValue(chatMetadataTokenValue(usage, "total"))) addCount("Total", "total");
  } else {
    addCount("Total", "total");
  }
  addCount("Input", "input");
  addCount("Output", "output");
  addCount("Cache creation", "cacheCreation");
  addCount("Cache read", "cacheRead");
  addCount("Reasoning", "reasoning");
  addCost();
  if (kind !== "current") {
    const completeness = chatMetadataTokenValue(usage, "completeness");
    rows.push({ label: "Completeness", text: chatMetadataHasValue(completeness) ? String(completeness) : "—" });
    rows.push({ label: "Partial attempts", text: chatMetadataTokenCount(chatMetadataTokenValue(usage, "partialAttempts")) });
    rows.push({ label: "Missing attempts", text: chatMetadataTokenCount(chatMetadataTokenValue(usage, "missingAttempts")) });
  }
  return rows;
}

function chatMetadataDetailModel(meta = {}, trigger = null) {
  const normalized = normalizedChatMessageMeta(meta || {});
  const context = chatMetadataSessionContext(normalized);
  const session = context.session || {};
  const project = context.project || {};
  const sessionProviderValue = chatMetadataFirst(session, "provider", "cli") || normalized.cli || normalized.provider;
  const sessionProjectPath = chatMetadataFirst(session, "projectPath", "project_path") || "";
  const projectName = chatMetadataFirst(project, "name", "repoName", "repo_name")
    || chatMetadataFirst(session, "projectName", "project_name")
    || sessionProjectPath.split(/[\\/]/).filter(Boolean).at(-1);
  const sessionCurrentUsage = chatMetadataFirst(session, "tokenUsage", "token_usage") || normalized.tokenUsageDetails;
  const lifetimeUsage = chatMetadataFirst(session, "lifetimeTokenUsage", "lifetime_token_usage");
  const contextUsage = chatMetadataFirst(session, "contextTokenUsage", "context_token_usage");
  const spentUsage = chatMetadataFirst(session, "spentTokenUsage", "spent_token_usage");
  const messageRows = chatMetadataEntries(normalized, trigger);
  const sessionRows = chatMetadataRowEntries([
    ["Title", chatMetadataFirst(session, "title", "summary")],
    ["Session ID", context.sessionId || normalized.sessionId],
    ["Provider", sessionProviderValue],
    ["Native session ID", chatMetadataFirst(session, "nativeSessionId", "native_session_id")],
    ["Runtime", chatMetadataRuntimeLabel(chatMetadataFirst(session, "runtime"))],
    ["Project", projectName],
    ["Project path", sessionProjectPath],
    ["Model", chatMetadataFirst(session, "model") || normalized.model],
    ["Effort", chatMetadataFirst(session, "effort") || normalized.effort],
    ["Mode", chatMetadataFirst(session, "mode") || normalized.mode],
    ["Thinking", chatMetadataBoolean(chatMetadataFirst(session, "thinking"))],
    ["Fast priority", chatMetadataBoolean(chatMetadataFirst(session, "fast"))],
    ["Messages", chatMetadataFirst(session, "messageCount", "message_count")],
    ["First user", chatMetadataTime(chatMetadataFirst(session, "firstUserAt", "first_user_at"))],
    ["Received", chatMetadataTime(chatMetadataFirst(session, "receivedAt", "received_at"))],
    ["Last activity", chatMetadataTime(chatMetadataFirst(session, "lastActivity", "last_activity"))],
    ["Last message", chatMetadataTime(chatMetadataFirst(session, "lastMessageAt", "last_message_at"))],
  ], { includeEmpty: true });
  const stateRows = chatMetadataRowEntries([
    ["Active", chatMetadataBoolean(chatMetadataFirst(session, "active"))],
    ["Ongoing", context.running ? "Yes" : "No"],
    ["Pinned", chatMetadataBoolean(context.pinned)],
    ["External", chatMetadataBoolean(chatMetadataFirst(session, "external"))],
    ["Open in chat", context.chatOpen ? "Yes" : "No"],
    ["Running", context.running ? "Yes" : "No"],
    ["Starting", context.processing && !context.response.activeResponseId ? "Yes" : "No"],
    ["Stopping", context.appState.chatStoppingSessionId === context.sessionId ? "Yes" : "No"],
    ["Runtime status", context.rawStatus || context.normalizedStatus],
    ["Active response ID", context.response.activeResponseId],
    ["Completed response ID", context.response.completedResponseId],
    ["Response sequence", context.response.sequence],
    ["Loaded messages", context.chatWindow ? `${context.chatWindow.messages.length}/${context.chatWindow.totalCount}` : undefined],
    ["Older messages", context.chatWindow ? (context.chatWindow.offset > 0 ? "Available" : "None") : undefined],
    ["Recovery state", chatMetadataFirst(context.recovery, "state")],
    ["Recovery response ID", chatMetadataFirst(context.recovery, "responseId")],
    ["Compaction state", chatMetadataFirst(context.compaction, "state")],
    ["Compaction response ID", chatMetadataFirst(context.compaction, "responseId")],
  ], { includeEmpty: true });
  const sections = [
    { title: "Message", rows: messageRows },
    { title: "Session", rows: sessionRows },
    { title: "State", rows: stateRows },
    { title: "Token usage", rows: chatMetadataTokenRows(sessionCurrentUsage, "current") },
    { title: "Lifetime tokens", rows: chatMetadataTokenRows(lifetimeUsage, "lifetime") },
  ];
  if (spentUsage && typeof spentUsage === "object") {
    const whole = chatMetadataFirst(spentUsage, "wholeSession", "whole_session");
    const sinceCompact = chatMetadataFirst(spentUsage, "sinceCompact", "since_compact");
    const spentRows = [];
    if (whole) spentRows.push(...chatMetadataTokenRows(whole, "spent").map((row) => ({ ...row, label: `Whole ${row.label}` })));
    if (sinceCompact) {
      const compactedAt = chatMetadataFirst(spentUsage, "compactedAt", "compacted_at");
      if (compactedAt) spentRows.push({ label: "Compacted at", text: chatMetadataTime(compactedAt) });
      spentRows.push(...chatMetadataTokenRows(sinceCompact, "spent").map((row) => ({ ...row, label: `Since compact ${row.label}` })));
    } else {
      spentRows.push({ label: "Since compact", text: "Not compacted" });
    }
    sections.push({ title: "Tokens spent", rows: spentRows.length ? spentRows : [{ label: "Usage", text: "Unavailable" }] });
  } else {
    sections.push({ title: "Tokens spent", rows: [{ label: "Usage", text: "Unavailable" }] });
  }
  if (contextUsage && typeof contextUsage === "object") {
    const contextRows = [
      { label: "Scope", text: chatMetadataTokenValue(contextUsage, "afterCompact") === true ? "After compact" : "Whole session" },
    ];
    const compactedAt = chatMetadataTokenValue(contextUsage, "compactedAt");
    if (compactedAt) contextRows.push({ label: "Compacted at", text: chatMetadataTime(compactedAt) });
    contextRows.push(...chatMetadataTokenRows(contextUsage, "context"));
    sections.push({ title: "Context tokens", rows: contextRows });
  } else {
    sections.push({ title: "Context tokens", rows: [{ label: "Usage", text: "Unavailable" }] });
  }
  const rawRows = [
    { label: "Message metadata", text: chatMetadataText(normalized), code: true },
  ];
  if (context.session) rawRows.push({ label: "Session snapshot", text: chatMetadataText(context.session), code: true });
  if (Object.keys(context.live || {}).length || Object.keys(context.response || {}).length) {
    rawRows.push({
      label: "Runtime snapshot",
      text: chatMetadataText({ live: context.live, response: context.response }),
      code: true,
    });
  }
  sections.push({ title: "Raw payload", rows: rawRows });
  return {
    role: normalized.role || (trigger?.closest(".chat-line-user") ? "user" : "assistant"),
    sessionId: context.sessionId,
    sections,
  };
}

function chatMetadataEntries(meta = {}, trigger = null) {
  const entries = [];
  const added = new Set();
  const add = (label, value, options = {}) => {
    const text = chatMetadataText(value);
    if (!text || added.has(label)) return;
    added.add(label);
    entries.push({
      label,
      text,
      code: options.code === true || typeof value === "object" || text.includes("\n"),
    });
  };
  const role = meta.role || (trigger?.closest(".chat-line-user") ? "user" : "assistant");
  add("Role", role);
  add("Message ID", meta.id || meta.messageId);
  add("Session ID", meta.sessionId);
  add("CLI", meta.cli || meta.provider);
  add("Model", meta.model);
  add("Mode", meta.mode);
  add("Effort", meta.effort);
  if (meta.thinking !== undefined && meta.thinking !== null) add("Thinking", meta.thinking ? "on" : "off");
  if (meta.fast !== undefined && meta.fast !== null) add("Fast priority", meta.fast ? "requested" : "not requested");
  add("Status", meta.status);
  add("Tokens", meta.tokenUsage);
  add("Token breakdown", meta.tokenUsageDetails, { code: true });
  add("Sent", meta.sentAt);
  add("Received", meta.receivedAt);
  add("Elapsed", meta.elapsed);
  add("Timestamp", meta.timestamp);
  add("Metadata", meta.metadata, { code: true });

  const reserved = new Set([
    "role", "id", "messageId", "sessionId", "cli", "provider", "model", "mode", "effort",
    "thinking", "fast", "status", "tokenUsage", "tokenUsageDetails", "sentAt", "receivedAt",
    "elapsed", "elapsedMs", "timestamp", "metadata", "chatCli", "chatProvider", "chatModel",
    "chatMode", "chatEffort", "chatThinking", "chatFast", "sent_at", "received_at", "token_usage",
  ]);
  Object.entries(meta).forEach(([key, value]) => {
    if (reserved.has(key)) return;
    add(chatMetadataLabel(key), value);
  });
  return entries;
}

function chatMetadataModalFocusableElements(modal) {
  if (!modal) return [];
  return [...modal.querySelectorAll(
    "button:not([disabled]), a[href], input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex=\"-1\"])",
  )].filter((element) => !element.hidden && !element.closest(".hidden"));
}

function trapChatMetadataModalFocus(event, dialog) {
  if (event.key !== "Tab") return;
  const focusable = chatMetadataModalFocusableElements(dialog);
  if (!focusable.length) return;
  const first = focusable[0];
  const last = focusable.at(-1);
  if (!dialog.contains(document.activeElement)) {
    event.preventDefault();
    (event.shiftKey ? last : first).focus();
  } else if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}

function closeChatMetadataModal({ restoreFocus = true } = {}) {
  const modal = qs("#chat-metadata-modal");
  if (!modal) return;
  modal.remove();
  document.body.classList.remove("chat-metadata-modal-open");
  const returnFocus = chatMetadataModalReturnFocus;
  chatMetadataModalReturnFocus = null;
  if (restoreFocus && returnFocus?.isConnected && !returnFocus.disabled) {
    window.requestAnimationFrame(() => returnFocus.focus({ preventScroll: true }));
  }
}

function chatMetadataModalHtml(meta, trigger) {
  const detail = chatMetadataDetailModel(meta, trigger);
  const title = detail.role === "user"
    ? "User message details"
    : "Assistant message details";
  const sections = detail.sections.map((section) => {
    const rows = (section.rows || []).map((entry) => `<div class="chat-metadata-modal-row">` +
      `<dt>${escapeHtml(entry.label)}</dt>` +
      `<dd class="${entry.code ? "is-code" : ""}">${escapeHtml(entry.text)}</dd>` +
      `</div>`).join("");
    return `<section class="chat-metadata-section">` +
      `<h3 class="chat-metadata-section-title">${escapeHtml(section.title)}</h3>` +
      `<dl class="chat-metadata-list">${rows || `<div class="chat-metadata-empty">No data available.</div>`}</dl>` +
      `</section>`;
  }).join("");
  return `<div id="chat-metadata-modal" class="chat-metadata-modal" role="presentation">` +
    `<section class="chat-metadata-dialog" role="dialog" aria-modal="true" aria-labelledby="chat-metadata-title" aria-describedby="chat-metadata-description">` +
    `<header class="chat-metadata-header">` +
    `<div><span class="chat-metadata-kicker">MESSAGE DETAILS</span><h2 id="chat-metadata-title">${escapeHtml(title)}</h2></div>` +
    `<button type="button" class="icon-button" data-chat-metadata-close aria-label="Close message details" title="Close" data-symbol="close"></button>` +
    `</header>` +
    `<div class="chat-metadata-body">` +
    `<p id="chat-metadata-description" class="chat-metadata-description">Complete message, session, runtime, and token details captured for this turn.</p>` +
    sections +
    `</div>` +
    `<footer class="chat-metadata-footer"><button type="button" class="primary-action" data-chat-metadata-close>Close</button></footer>` +
    `</section>` +
    `</div>`;
}

function openChatMetadataModal(meta = {}, trigger = null) {
  closeChatMetadataModal({ restoreFocus: false });
  chatMetadataModalReturnFocus = trigger instanceof HTMLElement
    ? trigger
    : (document.activeElement instanceof HTMLElement ? document.activeElement : null);
  document.body.insertAdjacentHTML("beforeend", chatMetadataModalHtml(meta, trigger));
  const modal = qs("#chat-metadata-modal");
  const dialog = modal?.querySelector(".chat-metadata-dialog");
  if (!modal || !dialog) return;
  modal.addEventListener("click", (event) => {
    if (event.target === modal || event.target.closest("[data-chat-metadata-close]")) {
      closeChatMetadataModal();
    }
  });
  modal.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      closeChatMetadataModal();
      return;
    }
    trapChatMetadataModalFocus(event, dialog);
  });
  document.body.classList.add("chat-metadata-modal-open");
  window.requestAnimationFrame(() => modal.querySelector("[data-chat-metadata-close]")?.focus({ preventScroll: true }));
}

function persistedChatMessageId(message) {
  const id = String(message?.id || "").trim();
  if (!id || id.startsWith("local-") || id.startsWith("local:")) return "";
  return id;
}

function replayUserPromptLine(message, meta = null, output = chatOutputRoot(), options = {}) {
  if (!output) return;
  const existing = options.existingNode;
  const { node, text, footer } = existing ? {
    node: existing,
    text: existing.querySelector(":scope > .chat-line-text"),
    footer: existing.querySelector(":scope > .chat-line-footer"),
  } : buildChatLineNode("user");
  const content = String(message?.content || "");
  if (text.textContent !== content) {
    if (text.childNodes.length === 1 && text.firstChild.nodeType === 3) patchChatRenderedText(text.firstChild, content);
    else text.textContent = content;
  }
  attachUserChatActions(node, message);
  const edit = node.querySelector("[data-chat-edit-from-here]");
  if (edit) {
    edit.disabled = Boolean(findChatSession(state.chatSessionId)?.active || selectedRunningChatSession());
    edit.title = edit.disabled ? "Stop the current response before editing from here" : "Edit from here";
  }
  renderChatLineFooter(footer, meta);
  placeChatRenderedLine(output, node, options);
  return node;
}

function replayAssistantLine(content, meta = null, options = {}) {
  const output = options.output || chatOutputRoot();
  if (!output) return;
  const existing = options.existingNode;
  const { node, text, footer } = existing ? {
    node: existing,
    text: existing.querySelector(":scope > .chat-line-text"),
    footer: existing.querySelector(":scope > .chat-line-footer"),
  } : buildChatLineNode("assistant");
  // Agent output is untrusted. HTML/CSS/script text must never become
  // application DOM, even when a tool has returned an entire web page.
  // renderChatBubbleHtml escapes every line and only emits a whitelist
  // of block tags, so this stays safe while exec / Parameters /
  // exec / Details become collapsible blocks.
  renderChatMessageContent(text, content);
  if (options.showResponseHeader) {
    renderChatResponseHeader(node, options.headerMeta || meta || {});
  } else {
    node.querySelector(":scope > .chat-response-header")?.remove();
  }
  renderChatLineFooter(footer, meta);
  if (options.copyText) attachAssistantChatActions(node, options.copyText);
  else node.querySelector("[data-chat-copy-kind='assistant']")?.remove();
  placeChatRenderedLine(output, node, options);
  return node;
}

function replayToolLine(content, output = chatOutputRoot(), options = {}) {
  if (!output) return;
  if (!String(content || "").trim()) return;
  const existing = options.existingNode;
  const { node, text, footer } = existing ? {
    node: existing,
    text: existing.querySelector(":scope > .chat-line-text"),
    footer: existing.querySelector(":scope > .chat-line-footer"),
  } : buildChatLineNode("assistant");
  node.classList.add("chat-line-tool-message");
  // Tool rows already contain structured `exec/tool / Parameters|Details`
  // sections. Render those directly so the UI does not create a second
  // outer "Tool output" collapsible around the real collapsibles.
  renderChatMessageContent(text, content);
  renderChatLineFooter(footer, null);
  placeChatRenderedLine(output, node, options);
  return node;
}

function stripAnsi(value) {
  return String(value || "").replace(/\u001B(?:[@-_]|\[[0-?]*[ -/]*[@-~])/g, "");
}

function normalizeChatToolHeading(value) {
  return String(value || "")
    .trim()
    .replace(/^[#>*\-` ]+/, "")
    .replace(/[#*` ]+$/, "")
    .trim();
}

function isChatAssistantBoundary(value) {
  return /^(codex|claude|assistant|response)$/i.test(normalizeChatToolHeading(value));
}

function isChatToolTelemetryHeading(value) {
  return /^(?:exec(?:\s*\/\s*(?:parameters|details))?|bash(?:\s*\/\s*(?:parameters|details))?|shell(?:\s+command)?(?:\s*\/\s*(?:parameters|details))?|command_execution|function_call(?:_output)?|custom_tool_call(?:_output)?|tool(?:\s*\/\s*(?:parameters|details))?|(?:edit|create|delete|move)\s*\/\s*.+|file_change(?:\s*\/\s*.+)?|apply[_\s]+patch(?:\s*\/\s*(?:parameters|details))?|patch\s*:.*|diff\s+--git\b.*)$/i
    .test(normalizeChatToolHeading(value));
}

function withoutChatToolTelemetrySections(value) {
  const visible = [];
  let suppressingToolSection = false;
  let fenced = false;
  for (const line of String(value || "").split("\n")) {
    const fenceBoundary = line.trimStart().startsWith("```");
    if (!suppressingToolSection && !fenced && isChatToolTelemetryHeading(line)) {
      suppressingToolSection = true;
    } else if (suppressingToolSection && !fenced && isChatAssistantBoundary(line)) {
      suppressingToolSection = false;
      visible.push(line);
    } else if (!suppressingToolSection) {
      visible.push(line);
    }
    if (fenceBoundary) fenced = !fenced;
  }
  return visible.join("\n").trim();
}

function withoutChatTokenUsageSections(value) {
  const visible = [];
  let skippingUsageValues = false;
  let fenced = false;
  const usageValue = /^(?:(?:used|total|input|output|cached?|cache creation|cache read)\s*[:=]\s*)?[\d,.]+(?:\.\d+)?\s*(?:tokens?|tok|[kmb])?(?:\s*\([^)]*\))?$/i;
  for (const line of String(value || "").split("\n")) {
    const fenceBoundary = line.trimStart().startsWith("```");
    const normalized = normalizeChatToolHeading(line);
    if (!fenced && /^(tokens used|token usage)$/i.test(normalized)) {
      skippingUsageValues = true;
    } else if (skippingUsageValues && !fenced && (
      isChatAssistantBoundary(line) ||
      /^response$/i.test(normalized) ||
      isChatToolTelemetryHeading(line)
    )) {
      skippingUsageValues = false;
      visible.push(line);
    } else if (skippingUsageValues && !fenced && (!normalized || usageValue.test(normalized))) {
      // Drop token accounting rows.
    } else if (skippingUsageValues) {
      skippingUsageValues = false;
      visible.push(line);
    } else {
      visible.push(line);
    }
    if (fenceBoundary) fenced = !fenced;
  }
  return visible.join("\n").trim();
}

function chatTimelineDisplay(value) {
  return withoutChatTokenUsageSections(value).trim();
}

function assistantResponseContent(value, provider, previousPrompt) {
  const normalized = stripAnsi(value)
    .replace(/\r\n?/g, "\n")
    .trim();
  if (!normalized) return "";
  const isCodex = String(provider || "").toLowerCase() === "codex" || normalized.split("\n").some((line) => line.startsWith("OpenAI Codex v"));
  if (!isCodex) return chatTimelineDisplay(normalized);
  const lines = normalized.split("\n");
  const separators = lines
    .map((line, index) => line.trim() === "--------" ? index : -1)
    .filter((index) => index >= 0);
  let remainder = separators.length >= 2
    ? lines.slice(separators[1] + 1).join("\n").trim()
    : normalized;
  if (/^user\n/i.test(remainder)) {
    remainder = remainder.slice(remainder.indexOf("\n") + 1).trimStart();
    const prompt = String(previousPrompt || "").trim();
    if (prompt && remainder.startsWith(prompt)) {
      remainder = remainder.slice(prompt.length).trimStart();
    }
  }
  const remainderLines = remainder.split("\n");
  if (remainderLines.length > 1 && isChatAssistantBoundary(remainderLines[0])) {
    remainder = remainderLines.slice(1).join("\n").trimStart();
  }
  return chatTimelineDisplay(
    remainder
      .split("\n")
      .filter((line) => {
        const trimmed = line.trim();
        return !(
          trimmed.startsWith("Reading additional input from stdin") ||
          trimmed.startsWith("OpenAI Codex v") ||
          trimmed === "--------" ||
          /^warning: Model metadata/i.test(trimmed)
        );
      })
      .join("\n")
      .trim(),
  );
}

function persistedChatMessageMeta(raw) {
  if (raw?.metadata && typeof raw.metadata === "object") return raw.metadata;
  if (raw?.meta && typeof raw.meta === "object") return raw.meta;
  return {};
}

function chatTurnFast(sessionId) {
  if (chatHistoryWindow.sessionId !== sessionId) return undefined;
  const prompt = [...chatHistoryWindow.messages].reverse().find((message) => message.role === "user");
  return normalizedChatMessageMeta(persistedChatMessageMeta(prompt)).fast;
}

function chatMessageProvider(raw, persistedMeta = null, session = null) {
  const meta = persistedMeta && typeof persistedMeta === "object"
    ? persistedMeta
    : persistedChatMessageMeta(raw);
  return raw?.provider
    || meta.provider
    || meta.cli
    || session?.provider
    || session?.cli
    || "";
}

function isTerminalStatusMessageMeta(meta) {
  return String(meta?.kind || "").toLowerCase() === "terminal_status";
}

function replayChatMessages(messages, options = {}) {
  const output = options.output || chatOutputRoot();
  if (!output) return;
  const session = options.session || findChatSession(options.sessionId || state.chatSessionId) || null;
  const sessionId = options.sessionId || session?.id || state.chatSessionId || "";
  syncChatPromptHistoryFromMessages(messages, sessionId);
  const { controls, presentation } = chatResponsePresentation(messages, {
    session,
    sessionId,
    overrides: options.overrides,
  });
  let previousPrompt = null;
  messages.forEach((raw, index) => {
    if (!raw) return;
    const role = String(raw.role || "").toLowerCase();
    const content = raw.content == null ? "" : String(raw.content);
    const persistedMeta = persistedChatMessageMeta(raw);
    const key = chatRenderMessageKey(raw);
    const candidate = options.reuseNodes?.get(key);
    const expectedClass = role === "user" ? "chat-line-user" : role === "system" ? "chat-line-system" : "chat-line-assistant";
    const existingNode = candidate?.classList.contains(expectedClass) ? candidate : null;
    const placement = { inPlace: options.inPlace, renderedNodes: options.renderedNodes, existingNode };
    let renderedNode = null;
    if (role === "user") {
      renderedNode = replayUserPromptLine(raw, chatMessageFooterMeta(raw, role, controls, null, session), output, placement);
      previousPrompt = raw;
    } else if (role === "assistant") {
      if (isTerminalStatusMessageMeta(persistedMeta)) return;
      const displayContent = assistantResponseContent(
        content,
        chatMessageProvider(raw, persistedMeta, session),
        previousPrompt?.content || "",
      );
      if (!displayContent) return;
      const turn = presentation.get(index) || {};
      renderedNode = replayAssistantLine(
        displayContent,
        turn.showResponseFooter
          ? (turn.footerMeta || chatMessageFooterMeta(raw, role, controls, previousPrompt, session))
          : null,
        {
          output,
          ...placement,
          showResponseHeader: turn.showResponseHeader === true,
          headerMeta: turn.headerMeta,
          copyText: turn.showResponseFooter ? turn.copyText : "",
        },
      );
    } else if (role === "tool") {
      renderedNode = replayToolLine(content, output, placement);
    } else if (role === "system") {
      const node = existingNode || document.createElement("div");
      node.className = "chat-line-system";
      if (persistedMeta.kind === "chat_error") {
        node.classList.add("chat-error-history");
        node.replaceChildren();
        const title = document.createElement("strong");
        title.textContent = persistedMeta.phase === "rejected" ? "Message not sent" : "Chat error";
        const text = document.createElement("p"); text.textContent = content;
        node.append(title, text);
        if (persistedMeta.action) {
          const action = document.createElement("button"); action.type = "button";
          action.textContent = "Review chat setup";
          action.onclick = async () => {
            togglePromptConfigPanel(true);
            await loadChatModelsIntoSelect(chatCliValue(), true);
          };
          node.append(action);
        }
        const url = safeProviderDocsUrl(persistedMeta.docsUrl);
        if (url) {
          const link = document.createElement("a"); link.href = url; link.target = "_blank";
          link.rel = "noopener noreferrer"; link.textContent = "Official setup guide";
          node.append(link);
        }
      } else if (node.textContent !== content) node.textContent = content;
      placeChatRenderedLine(output, node, placement);
      renderedNode = node;
    }
    if (renderedNode && key) renderedNode.dataset.chatHistoryKey = key;
  });
  if (!options.output) scrollChatToBottom();
}

function prependChatHistoryPage(page, previousMessages, options = {}) {
  // Recompute response boundaries without moving their existing DOM out of the
  // document; moving through a fragment would discard focus and selections.
  return reconcileChatMessages(page.concat(previousMessages), options);
}

// Normalize raw metadata stored in storage. ISO timestamps become the
// format used by `renderChatLineFooter` and `elapsedMs` is converted into
// the human-friendly `elapsed` string the footer expects.
function formatTokenUsage(value) {
  if (!value) return "";
  if (typeof value === "string") return value;
  if (typeof value !== "object") return "";
  const used = Number(value.used) || 0;
  const input = Number(value.input) || 0;
  const output = Number(value.output) || 0;
  if (!used && !input && !output) return "";
  return `${used} (in ${input} / out ${output})`;
}

function compactTokenCount(value) {
  const count = Number(value) || 0;
  if (count >= 1_000_000_000) return `${(count / 1_000_000_000).toFixed(count >= 10_000_000_000 ? 0 : 1)}B`;
  if (count >= 1_000_000) return `${(count / 1_000_000).toFixed(count >= 10_000_000 ? 0 : 1)}M`;
  if (count >= 1_000) return `${(count / 1_000).toFixed(count >= 10_000 ? 0 : 1)}K`;
  return String(count);
}

function formatLifetimeTokenUsage(value) {
  if (!value || typeof value !== "object") return "";
  const total = Number(value.total) || 0;
  const missing = Number(value.missingAttempts) || 0;
  const partial = Number(value.partialAttempts) || 0;
  const completeness = String(value.completeness || "").toLowerCase();
  if (!total && (missing || completeness === "missing")) return "Unknown tokens";
  if (!total) return "";
  const prefix = missing || partial || completeness === "partial" ? ">=" : "";
  return `${prefix}${compactTokenCount(total)} tokens`;
}

function formatSpentTokenUsage(value, includeZero = false) {
  if (!value || typeof value !== "object") return "";
  const total = Number(value.total) || 0;
  const missing = Number(value.missingAttempts) || 0;
  const partial = Number(value.partialAttempts) || 0;
  const completeness = String(value.completeness || "").toLowerCase();
  if (!total && (missing || completeness === "missing")) return "Unknown tokens";
  if (!total) return includeZero ? "0 tokens" : "";
  const prefix = missing || partial || completeness === "partial" ? ">=" : "";
  return `${prefix}${compactTokenCount(total)} tokens`;
}

function formatSessionDisplayTokenUsage(session) {
  const spent = session?.spentTokenUsage;
  if (spent) {
    const scoped = spent.sinceCompact || spent.wholeSession;
    const includeZero = Boolean(spent.sinceCompact);
    const formatted = formatSpentTokenUsage(scoped, includeZero);
    if (formatted) return formatted;
  }
  const scoped = session?.contextTokenUsage;
  if (scoped?.afterCompact) {
    return formatLifetimeTokenUsage(scoped) || `${compactTokenCount(scoped.total)} tokens`;
  }
  return formatLifetimeTokenUsage(scoped) || formatLifetimeTokenUsage(session?.lifetimeTokenUsage);
}

function sessionDisplayTokenUsageTitle(session) {
  if (session?.spentTokenUsage) {
    return session.spentTokenUsage.sinceCompact ? "Tokens spent since compact" : "Tokens spent whole session";
  }
  return session?.contextTokenUsage?.afterCompact ? "Token usage after compact" : "Lifetime token usage";
}

function normalizeMessageMeta(raw) {
  if (!raw || typeof raw !== "object") return {};
  const meta = { ...raw };
  if (typeof meta.receivedAt === "string" && !Number.isNaN(new Date(meta.receivedAt).getTime())) {
    meta.receivedAt = formatReceivedDateTime(meta.receivedAt);
  }
  if (typeof meta.sentAt === "string" && !Number.isNaN(new Date(meta.sentAt).getTime())) {
    meta.sentAt = formatReceivedDateTime(meta.sentAt);
  }
  if (typeof meta.elapsedMs === "number" && meta.elapsedMs >= 0 && !meta.elapsed) {
    const totalMs = meta.elapsedMs;
    if (totalMs < 1000) meta.elapsed = "<1s";
    else {
      const s = Math.round(totalMs / 1000);
      if (s < 60) meta.elapsed = `${s}s`;
      else {
        const m = Math.floor(s / 60);
        const r = s % 60;
        meta.elapsed = r ? `${m}m ${r}s` : `${m}m`;
      }
    }
  }
  // tokenUsage may be persisted as an object {used,input,output,...} on the
  // message row; flatten it to the display string the footer expects.
  if (meta.tokenUsage && typeof meta.tokenUsage === "object") {
    meta.tokenUsageDetails = { ...meta.tokenUsage };
    meta.tokenUsage = formatTokenUsage(meta.tokenUsage);
  }
  delete meta.elapsedMs;
  return meta;
}

let chatHistoryTailRequest = null;
let chatHistoryRequestSessionId = "";
const chatSessionEventVersions = new Map();

function noteChatSessionEvent(sessionId) {
  if (sessionId) chatSessionEventVersions.set(sessionId, chatSessionEventVersion(sessionId) + 1);
}

function chatSessionEventVersion(sessionId) {
  return chatSessionEventVersions.get(sessionId) || 0;
}

async function loadChatHistoryForSession(sessionId, opts = {}) {
  if (!sessionId) return false;
  if ((state.chatSessionId || state.pendingChatSessionId) !== sessionId) return false;
  if (chatHistoryRequestSessionId !== sessionId) {
    chatHistoryRequestSessionId = sessionId;
    chatHistoryGeneration += 1;
    chatHistoryTailRequest = null;
    chatOlderHistoryRequest = null;
    chatHistoryDemandSessionId = "";
    state.chatOlderMessagesLoadingSessionId = "";
  }
  const generation = chatHistoryGeneration;
  const older = opts.older === true;
  if (older && (chatHistoryWindow.sessionId !== sessionId || state.chatSessionId !== sessionId)) return false;
  const existing = older ? chatOlderHistoryRequest : chatHistoryTailRequest;
  if (existing?.sessionId === sessionId && existing.generation === generation) {
    if (opts.forceBottom) existing.options.forceBottom = true;
    return existing.promise;
  }
  // Serialize paging and tail commits without cancelling either operation. An
  // ordinary poll is not a session switch and must not invalidate user demand.
  const preceding = older ? chatHistoryTailRequest : chatOlderHistoryRequest;
  const request = { sessionId, generation, options: { ...opts }, promise: null };
  if (older) {
    chatOlderHistoryRequest = request;
    state.chatOlderMessagesLoadingSessionId = sessionId;
    showOlderMessagesLoading();
  } else {
    chatHistoryTailRequest = request;
  }
  request.promise = (async () => {
    if (preceding?.generation === generation) await preceding.promise;
    if (generation !== chatHistoryGeneration
      || (state.chatSessionId || state.pendingChatSessionId) !== sessionId) return false;
    return performChatHistoryLoad(sessionId, request.options);
  })();
  try {
    return await request.promise;
  } finally {
    if (older && chatOlderHistoryRequest === request) {
      chatOlderHistoryRequest = null;
      state.chatOlderMessagesLoadingSessionId = "";
      chatHistoryDemandSessionId = "";
      renderOlderChatMessagesControl();
    } else if (!older && chatHistoryTailRequest === request) {
      chatHistoryTailRequest = null;
    }
  }
}

function retainRicherChatMessage(previous, incoming) {
  if (!previous || !incoming?.metadata?.contentTruncated || previous.metadata?.contentTruncated) return incoming;
  const text = String(previous.content || "");
  const bytes = typeof TextEncoder === "function" ? new TextEncoder().encode(text).length : text.length;
  // A response-size budget is not a history edit. Only retain a cached body
  // when its size and available prefix agree with the bounded representation.
  const prefix = String(incoming.content || "").split("\n[", 1)[0].slice(0, 64);
  if (bytes !== incoming.metadata.originalContentBytes || (prefix && !text.startsWith(prefix))) return incoming;
  return { ...incoming, content: text, metadata: { ...incoming.metadata, contentTruncated: false } };
}

async function chatHistoryTailWindow(sessionId, page, totalCount, generation) {
  const tailOffset = Math.max(0, totalCount - page.length);
  const current = chatHistoryWindow;
  if (current.sessionId !== sessionId) return { messages: page, offset: tailOffset };
  const previous = current.messages.filter((message) => persistedChatMessageId(message));
  if (!previous.length) return { messages: page, offset: tailOffset };
  const byId = new Map(previous.map((message) => [persistedChatMessageId(message), message]));
  const retain = (message) => retainRicherChatMessage(byId.get(persistedChatMessageId(message)), message);
  const overlap = previous.findIndex((message) => persistedChatMessageId(message) === persistedChatMessageId(page[0]));
  if (overlap >= 0 && current.offset + overlap === tailOffset
    && previous.slice(overlap).every((message, index) => index >= page.length
      || persistedChatMessageId(message) === persistedChatMessageId(page[index]))) {
    return { messages: previous.slice(0, overlap).concat(page.map(retain)), offset: current.offset };
  }
  if (!totalCount) return { messages: [], offset: 0 };
  // There is a gap or a changed numeric boundary. Refetch the covered range in
  // bounded pages; never concatenate a disjoint tail as though it were adjacent.
  const previousTotal = current.persistedTotalCount ?? (current.offset + previous.length);
  const offset = Math.max(0, Math.min(current.offset, tailOffset) - Math.max(0, previousTotal - totalCount));
  const messages = [];
  for (let cursor = offset; cursor < tailOffset;) {
    const limit = Math.min(CHAT_HISTORY_PAGE_SIZE, tailOffset - cursor);
    const response = await api(`/api/sessions/${encodeURIComponent(sessionId)}/messages?limit=${limit}&offset=${cursor}`);
    if (generation !== chatHistoryGeneration || state.chatSessionId !== sessionId || stagedChatEdit()) return null;
    const rows = Array.isArray(response) ? response : (response?.messages || []);
    const count = Number(response?.total_count ?? response?.totalCount ?? totalCount);
    if (count !== totalCount || rows.length !== limit) return null;
    messages.push(...rows.map(retain));
    cursor += limit;
  }
  messages.push(...page.map(retain));
  const keys = messages.map(persistedChatMessageId);
  if (keys.some((key) => !key) || new Set(keys).size !== keys.length) return null;
  return { messages, offset };
}

async function performChatHistoryLoad(sessionId, opts = {}) {
  if (!sessionId) return false;
  if (stagedChatEdit()?.sourceSessionId === sessionId) return true;
  const generation = chatHistoryGeneration;
  const previousWindow = chatHistoryWindow;
  const eventVersion = chatSessionEventVersion(sessionId);
  const recoveryBeforeRequest = state.chatRecoveryBySession[sessionId];
  try {
    const loadingOlder = opts.older === true && chatHistoryWindow.sessionId === sessionId;
    const requestedOffset = loadingOlder
      ? Math.max(0, chatHistoryWindow.offset - CHAT_HISTORY_PAGE_SIZE)
      : 0;
    const requestedLimit = loadingOlder
      ? chatHistoryWindow.offset - requestedOffset
      : CHAT_HISTORY_PAGE_SIZE;
    if (loadingOlder && requestedLimit <= 0) return true;
    const query = loadingOlder
      ? `limit=${requestedLimit}&offset=${requestedOffset}`
      : `limit=${CHAT_HISTORY_PAGE_SIZE}`;
    const url = loadingOlder
      ? `/api/sessions/${encodeURIComponent(sessionId)}/messages?${query}`
      : `/api/sessions/${encodeURIComponent(sessionId)}/snapshot?${query}`;
    let body;
    try {
      body = await api(url);
    } catch (error) {
      if (loadingOlder) throw error;
      await loadProjects().catch(() => {});
      const fallbackSession = findChatSession(sessionId);
      if (!fallbackSession) throw error;
      const fallbackOffset = Math.max(
        0,
        (Number(fallbackSession.messageCount) || 0) - CHAT_HISTORY_PAGE_SIZE,
      );
      const fallbackQuery = [
        `limit=${CHAT_HISTORY_PAGE_SIZE}`,
        `offset=${fallbackOffset}`,
        fallbackSession.external ? "tail=true" : "",
      ].filter(Boolean).join("&");
      const response = await api(`/api/sessions/${encodeURIComponent(sessionId)}/messages?${fallbackQuery}`);
      body = {
        session: fallbackSession,
        messages: response?.messages || [],
        hasMore: response?.has_more ?? response?.hasMore ?? fallbackOffset > 0,
        totalCount: response?.total_count ?? response?.totalCount ?? fallbackSession.messageCount ?? 0,
      };
    }
    if (generation !== chatHistoryGeneration
      || (state.chatSessionId || state.pendingChatSessionId) !== sessionId
      || stagedChatEdit()?.sourceSessionId === sessionId
      || (loadingOlder && (chatHistoryWindow.sessionId !== sessionId || chatHistoryWindow.offset !== previousWindow.offset))) return false;
    const receivedPage = Array.isArray(body) ? body : (body.messages || []);
    const existingIds = new Set(loadingOlder ? previousWindow.messages.map(persistedChatMessageId).filter(Boolean) : []);
    const page = receivedPage.filter((message) => {
      const id = persistedChatMessageId(message);
      if (!id) return true;
      if (existingIds.has(id)) return false;
      existingIds.add(id);
      return true;
    });
    const totalCount = Math.max(0, Number(body?.total_count ?? body?.totalCount ?? page.length) || 0);
    const loadedWindow = loadingOlder ? null : await chatHistoryTailWindow(sessionId, page, totalCount, generation);
    if (!loadingOlder && !loadedWindow) return false;
    if (generation !== chatHistoryGeneration || (state.chatSessionId || state.pendingChatSessionId) !== sessionId
      || stagedChatEdit()?.sourceSessionId === sessionId) return false;
    // Capture after all I/O, but before recovery/status rendering can change
    // the transcript. Respect any scrolling the reader did during the request.
    const viewport = typeof captureChatViewport === "function" ? captureChatViewport() : null;
    const anchor = !viewport ? captureChatHistoryAnchor() : null;
    const previousOutput = chatOutputRoot();
    const previousHeight = previousOutput?.scrollHeight || 0;
    const previousTop = previousOutput?.scrollTop || 0;
    const newerEvents = chatSessionEventVersion(sessionId) !== eventVersion;
    const rawSnapshotSession = body?.session || findChatSession(sessionId);
    const snapshotSession = newerEvents && rawSnapshotSession
      ? { ...rawSnapshotSession, active: chatSessionIsLive(sessionId) } : rawSnapshotSession;
    const snapshotLive = Boolean(snapshotSession?.active);
    if (!loadingOlder && !newerEvents && state.chatRecoveryBySession[sessionId] === recoveryBeforeRequest) {
      rememberChatRecovery(sessionId, body?.recovery || null);
    }
    const recoveryState = state.chatRecoveryBySession[sessionId]?.state || "";
    const snapshotStatus = snapshotSession?.active || recoveryState === "starting"
      ? "running"
      : (["required", "failed"].includes(recoveryState) ? "failed" : "completed");
    if (snapshotSession?.id) {
      if (snapshotSession.projectPath) {
        setActiveProject(snapshotSession.projectPath);
      }
      state.sessions = (state.sessions || []).filter((session) => session?.id !== snapshotSession.id);
      state.sessions.push(snapshotSession);
      mergeSessionIntoProjects(snapshotSession);
      if (!newerEvents) {
        rememberSidebarSessionStatus({
          sessionId: snapshotSession.id,
          provider: snapshotSession.provider,
          status: snapshotStatus,
        });
      }
    }
    const currentMessages = chatHistoryWindow.sessionId === sessionId ? chatHistoryWindow.messages : [];
    const optimistic = currentMessages.filter((message) => !persistedChatMessageId(message));
    // Until this turn has an authoritative inactive snapshot, keep its local
    // presentation together. There is no safe content-based identity match
    // between a provisional prompt and a newly persisted server message.
    const keepOptimistic = optimistic.length > 0 && (snapshotLive || newerEvents);
    const messages = loadingOlder ? page.concat(currentMessages)
      : (keepOptimistic ? currentMessages : loadedWindow.messages);
    const split = splitCachedStreamingMessage(messages, sessionId);
    const replayMessages = split.messages;
    const offset = loadingOlder
      ? requestedOffset
      : (keepOptimistic ? chatHistoryWindow.offset : loadedWindow.offset);
    const sameSession = chatHistoryWindow.sessionId === sessionId;
    chatHistoryWindow = { sessionId, offset, totalCount,
      persistedTotalCount: keepOptimistic ? (previousWindow.persistedTotalCount ?? previousWindow.totalCount) : totalCount,
      messages: replayMessages };
    const persisted = getSessionOverridesFor(sessionId) || {};
    if (loadingOlder) {
      prependChatHistoryPage(page, currentMessages, {
        session: snapshotSession, sessionId, overrides: persisted,
      });
    } else {
      if (!sameSession) resetChatOutputDom();
      // REST history does not own a current response. In particular it must
      // never write back a buffer captured before awaiting its request.
      if (!snapshotLive && !newerEvents) {
        if (chatStream.node?.isConnected && !chatStream.node.dataset.chatHistoryKey) chatStream.node.remove();
        chatStream = { role: null, node: null, text: null, buffer: "" };
        state.chatBuffer = "";
        delete state.chatOutputBuffersBySession[sessionId];
        clearChatProcessing();
      }
      reconcileChatMessages(replayMessages, {
        session: snapshotSession,
        sessionId,
        overrides: persisted,
      });
      renderChatRecoveryCard(sessionId);
      renderChatManualCompactionCard(sessionId);
    }
    renderOlderChatMessagesControl();
    if (!loadingOlder && snapshotLive) {
      if (!chatStream.node?.isConnected && state.chatOutputBuffersBySession[sessionId]) {
        restoreChatStreamSnapshot(sessionId, {
          buffer: state.chatOutputBuffersBySession[sessionId], provider: sessionProvider(snapshotSession),
        });
      }
      if (!chatStream.node?.isConnected) {
        ensureChatProcessing({
          provider: sessionProvider(snapshotSession),
          sessionId,
        });
      }
    }
    const output = chatOutputRoot();
    if (!sameSession && !loadingOlder) {
      // Opening a different transcript is an intentional reset, so do not
      // restore the prior session's viewport (or its invalidated revision).
      scrollChatToBottom(true);
    } else if (viewport) {
      restoreChatViewport(viewport, { forceBottom: opts.forceBottom === true || state.chatJumpToLatestPending });
    } else if (loadingOlder && output) {
      if (!restoreChatHistoryAnchor(anchor)) {
        output.scrollTop = Math.max(0, output.scrollHeight - previousHeight + previousTop);
      }
    } else if (opts.forceBottom || state.chatJumpToLatestPending) {
      scrollChatToBottom(true);
    } else {
      scrollChatToBottom();
    }
    maybeLoadOlderChatMessages();
    // Restore the current session controls after replaying the persisted
    // per-turn metadata in the transcript.
    if (!newerEvents) loadSessionOverridesIntoState(sessionId, snapshotSession);
    renderChatFooter(null);
    rememberCurrentChatSession({
      sessionId,
      session: snapshotSession,
      messages: replayMessages,
      offset,
      totalCount,
      live: snapshotLive,
      status: snapshotStatus,
    });
    persistActiveChatSelection(sessionId, snapshotSession?.projectPath || activeProjectPath());
    if (typeof updateMainHeader === "function") updateMainHeader();
    updateChatEmptyState();
    return true;
  } catch (error) {
    if (generation !== chatHistoryGeneration
      || (state.chatSessionId || state.pendingChatSessionId) !== sessionId) return false;
    showError(new Error(`Could not load chat history: ${error.message}`));
    return false;
  }
}

function scheduleChatReconciliation(sessionId = state.chatSessionId, opts = {}) {
  const id = (sessionId || "").trim();
  if (!id || !canLoadProtectedData()) return;
  if (stagedChatEdit()?.sourceSessionId === id) return;
  if (state.chatReconcileTimers[id]) {
    window.clearTimeout(state.chatReconcileTimers[id]);
  }
  state.chatReconcileTimers[id] = window.setTimeout(async () => {
    delete state.chatReconcileTimers[id];
    if (state.chatSessionId !== id && state.pendingChatSessionId !== id) return;
    const ok = await loadChatHistoryForSession(id);
    if (ok) {
      await loadChatPromptDraft(id, { preserveUserInput: true }).catch((error) => {
        console.warn("Unable to load prompt draft after chat reconciliation", error);
      });
    } else if (opts.reportFailure) {
      showToast("Could not reconcile chat session", "danger");
    }
  }, opts.delayMs ?? 0);
}

async function reconcileSelectedChatRecoverySnapshot(generation = state.wsGeneration) {
  const sessionId = String(state.chatSessionId || state.pendingChatSessionId || "").trim();
  if (!sessionId) return false;
  const session = findChatSession(sessionId) || cachedChatSession(sessionId)?.session;
  if (!session || session.pending || sessionId.startsWith("local-")) return false;
  const recoveryBeforeRequest = state.chatRecoveryBySession[sessionId] || null;
  const body = await api(`/api/sessions/${encodeURIComponent(sessionId)}/snapshot?limit=1`);
  if (generation !== state.wsGeneration) return false;
  if (state.chatSessionId !== sessionId && state.pendingChatSessionId !== sessionId) return false;
  // A recovery event may have arrived while the snapshot was loading. Keep
  // that newer state instead of replacing it with an older snapshot.
  if ((state.chatRecoveryBySession[sessionId] || null) !== recoveryBeforeRequest) return false;
  rememberChatRecovery(sessionId, body?.recovery || null);
  return true;
}

function chatRecoveryHasOptimisticTimelineState(sessionId) {
  const id = String(sessionId || "").trim();
  if (!id || chatHistoryWindow.sessionId !== id) return false;
  return chatHistoryWindow.messages.some((message) => (
    /^local-(?:user|assistant|stream)-/.test(String(message?.id || ""))
  ));
}

function handleChatRecoveryRequired(payload = {}) {
  const sessionId = String(payload.sessionId || "").trim();
  if (!sessionId) return;
  noteChatSessionEvent(sessionId);
  const selected = isActiveChatSessionEvent({ sessionId });
  const reconcileTimeline = selected && chatRecoveryHasOptimisticTimelineState(sessionId);
  const rejectedPrompt = reconcileTimeline
    ? [...chatHistoryWindow.messages].reverse().find((message) => (
      /^local-user-/.test(String(message?.id || ""))
    ))?.content || ""
    : "";
  rememberChatRecovery(sessionId, payload.recovery || payload);
  if (!reconcileTimeline) return;
  if (rejectedPrompt && !qs("#chat-prompt")?.value.trim()) {
    setChatPromptValue(rejectedPrompt);
    noteChatPromptUserEdit(rejectedPrompt);
    writeLocalChatPromptDraft(rejectedPrompt, sessionId);
    scheduleChatPromptDraftSave();
  }
  // The server snapshot is authoritative. Replaying it replaces optimistic
  // local rows from the blocked send while leaving the visible Workbench
  // session identity alone. The rejected text is restored as an unsent draft.
  loadChatHistoryForSession(sessionId, { forceBottom: true }).catch((error) => {
    console.warn("Unable to reconcile chat after recovery became required", error);
  });
}

function scheduleCompletedChatReconciliation(sessionId) {
  const id = (sessionId || "").trim();
  if (!id) return;
  for (const delayMs of CHAT_COMPLETION_RECONCILE_DELAYS_MS) {
    window.setTimeout(() => {
      if (state.chatSessionId === id || state.pendingChatSessionId === id) {
        scheduleChatReconciliation(id);
      }
    }, delayMs);
  }
}

function startChatActivityPoll() {
  if (state.chatActivityPollTimer) {
    window.clearInterval(state.chatActivityPollTimer);
  }
  state.chatActivityPollTimer = window.setInterval(() => {
    const sessionId = state.chatSessionId || state.pendingChatSessionId || "";
    if (!sessionId || !canLoadProtectedData()) return;
    if (!chatSessionIsLive(sessionId)) return;
    scheduleChatReconciliation(sessionId);
  }, CHAT_ACTIVE_POLL_INTERVAL_MS);
}

async function pickChatSession(sessionId, projectPath, options = {}) {
  const id = (sessionId || "").trim();
  if (!id) return;
  if (state.chatEditFromHere.submitting) {
    showToast("Wait for the replacement chat to finish creating", "ok");
    return;
  }
  closeChatEditFromHerePicker();
  clearChatImages();
  await saveChatPromptDraftNow().catch((error) => {
    console.warn("Unable to sync prompt draft before switching session", error);
  });
  preserveActiveChatStreamSnapshot();
  state.chatEditFromHere.staged = null;
  const session = findChatSession(id);
  state.pendingChatSessionId = id;
  state.chatSessionId = id;
  ensureChatPromptHistoryScope(id);
  state.preferences.lastChatSessionId = id;
  savePreferences();
  if (projectPath) setActiveProject(projectPath);
  await switchView("chat");
  if (session?.pending) {
    resetChatOutputDom();
    setChatPromptValue("", { draftApplied: true });
    updateChatEmptyState();
    return;
  }
  state.pendingChatSessionId = "";
  const renderedCached = options.forceSnapshot === true ? false : renderCachedChatSession(id);
  persistActiveChatSelection(id, projectPath || sessionProjectPath(session, activeProjectPath()));
  if (renderedCached) {
    scheduleChatReconciliation(id, { reportFailure: true });
    await loadChatPromptDraft(id).catch((error) => {
      console.warn("Unable to load prompt draft for cached session", error);
    });
    await loadChatPromptHistory(id);
    return;
  }
  await loadChatHistoryForSession(id, { forceBottom: true });
  await loadChatPromptDraft(id);
  await loadChatPromptHistory(id);
}

// Start a fresh chat for a project. Like the Android controller, this does
// not create a session id yet; the temporary id is allocated only when the
// first prompt is sent.
async function startNewChatForProject(projectPath) {
  if (!projectPath) return;
  if (state.chatEditFromHere.submitting) {
    showToast("Wait for the replacement chat to finish creating", "ok");
    return;
  }
  closeChatEditFromHerePicker();
  clearChatImages();
  await saveChatPromptDraftNow().catch((error) => {
    console.warn("Unable to sync prompt draft before starting new chat", error);
  });
  preserveActiveChatStreamSnapshot();
  state.chatEditFromHere.staged = null;
  setActiveProject(projectPath);
  state.chatSessionId = "";
  state.pendingChatSessionId = "";
  state.currentSession = null;
  state.chatSetupAttempt = null;
  state.newChatRuntime = "";
  clearActiveChatSelection();
  ensureChatPromptHistoryScope("");
  state.chatBuffer = "";
  chatHistoryWindow = { sessionId: "", offset: 0, totalCount: 0, messages: [] };
  if (!state.expandedProjectPaths.has(projectPath)) {
    state.expandedProjectPaths.add(projectPath);
    saveExpandedProjectPaths();
  }
  savePreferences();
  renderProjects();
  renderSidebarProjects();
  state.chatSuppressAutoOpenOnce = true;
  await switchView("chat");
  resetChatOutputDom();
  updateChatEmptyState();
  const prompt = qs("#chat-prompt");
  if (prompt) {
    setChatPromptValue(readLocalChatPromptDraft(""), { draftApplied: true });
    state.chatPromptDraftSessionId = "";
    prompt.focus();
  }
  loadChatModelsIntoSelect(chatCliValue()).catch(() => {});
}

function pickDefaultChatSession() {
  // Choose the most recent session we can find.  If the active project has
  // sessions, prefer those; otherwise fall back to the single most-recent
  // session across all projects.
  const allSessions = []
    .concat(
      ...(state.projects || []).map((p) => (p.sessions || [])
        .map((s) => ({ ...s, projectPath: p.path }))),
    )
    .concat((state.sessions || []).map((s) => ({ ...s })));
  if (!allSessions.length) return null;
  const activePath = activeProjectPath();
  const activeProject = activePath
    ? (state.projects || []).find((p) => p.path === activePath)
    : null;
  const activeSessions = activeProject ? (activeProject.sessions || []) : [];
  if (activeSessions.length) {
    return activeSessions
      .slice()
      .sort((a, b) => new Date(b.lastActivity || 0) - new Date(a.lastActivity || 0))[0];
  }
  // No active project (or no sessions in it): pick topmost project with the
  // most recent chat history.
  const projectsWithSessions = (state.projects || [])
    .filter((p) => (p.sessions || []).length > 0);
  if (!projectsWithSessions.length) return null;
  // Collect all sessions, attach their project path, and pick the newest.
  return projectsWithSessions
    .flatMap((p) => (p.sessions || [])
      .map((s) => ({ ...s, projectPath: p.path })))
    .sort((a, b) => new Date(b.lastActivity || 0) - new Date(a.lastActivity || 0))[0];
}

async function autoOpenLatestChatSession() {
  const target = pickDefaultChatSession();
  if (!target) return false;
  if (target.projectPath) {
    setActiveProject(target.projectPath);
  }
  await pickChatSession(target.id, target.projectPath);
  return true;
}

function activeProjectChatSessions() {
  const project = (state.projects || []).find((item) => item.path === activeProjectPath());
  return project ? sidebarProjectSessions(project) : [];
}

function currentChatNavigationEntries() {
  const current = findChatSession(state.chatSessionId);
  if (current && isChatSessionPinned(current, sessionProjectPath(current, activeProjectPath()))) {
    return pinnedChatEntries();
  }
  return activeProjectChatSessions();
}

async function navigateAdjacentChatSession(direction) {
  if (!state.chatSessionId) return false;
  const entries = currentChatNavigationEntries();
  if (entries.length < 2) return false;
  const currentIndex = entries.findIndex((session) => session.id === state.chatSessionId);
  if (currentIndex < 0) return false;
  const nextIndex = currentIndex + direction;
  if (nextIndex < 0 || nextIndex >= entries.length) return false;
  const next = entries[nextIndex];
  if (!next?.id) return false;
  hapticFeedback(8);
  await pickChatSession(next.id, sessionProjectPath(next, activeProjectPath()));
  renderSidebarProjects();
  return true;
}

async function navigatePinnedChatSession(direction) {
  const entries = pinnedChatEntries();
  if (!entries.length) return false;
  const currentIndex = entries.findIndex((session) => session.id === state.chatSessionId);
  let nextIndex;
  if (currentIndex < 0) {
    nextIndex = direction > 0 ? 0 : entries.length - 1;
  } else {
    nextIndex = (currentIndex + direction + entries.length) % entries.length;
  }
  const next = entries[nextIndex];
  if (!next?.id || (entries.length === 1 && next.id === state.chatSessionId)) return false;
  hapticFeedback(8);
  await pickChatSession(next.id, sessionProjectPath(next, activeProjectPath()));
  renderSidebarProjects();
  return true;
}

function isPinnedChatShortcut(event) {
  if (!event?.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return 0;
  if (event.code === "Comma" || event.key === ",") return -1;
  if (event.code === "Period" || event.key === ".") return 1;
  return 0;
}

function bindPinnedChatShortcuts() {
  document.addEventListener("keydown", (event) => {
    const direction = isPinnedChatShortcut(event);
    if (!direction) return;
    event.preventDefault();
    event.stopPropagation();
    navigatePinnedChatSession(direction).catch(showError);
  }, true);
}

function chatSwipeIgnoredTarget(target) {
  if (!(target instanceof HTMLElement)) return false;
  return Boolean(target.closest("button, a, input, textarea, select, .chat-composer, .chat-provider-picker"));
}

function handleChatTouchStart(event) {
  if (window.innerWidth >= 640 || chatSwipeIgnoredTarget(event.target)) return;
  const touch = event.touches?.[0];
  if (!touch) return;
  state.chatSwipe = {
    startX: touch.clientX,
    startY: touch.clientY,
    deltaX: 0,
    deltaY: 0,
  };
}

function handleChatTouchMove(event) {
  if (!state.chatSwipe) return;
  const touch = event.touches?.[0];
  if (!touch) return;
  state.chatSwipe.deltaX = touch.clientX - state.chatSwipe.startX;
  state.chatSwipe.deltaY = touch.clientY - state.chatSwipe.startY;
}

function resetChatSwipe() {
  state.chatSwipe = null;
}

function handleChatTouchEnd() {
  const swipe = state.chatSwipe;
  resetChatSwipe();
  if (!swipe || window.innerWidth >= 640) return;
  const horizontalDistance = Math.abs(swipe.deltaX);
  const verticalDistance = Math.abs(swipe.deltaY);
  const horizontal = horizontalDistance >= CHAT_SWIPE_MIN_DISTANCE
    && verticalDistance <= CHAT_SWIPE_MAX_VERTICAL_DRIFT
    && horizontalDistance > verticalDistance * CHAT_SWIPE_DIRECTION_RATIO;
  if (!horizontal) return;
  navigateAdjacentChatSession(swipe.deltaX > 0 ? -1 : 1).catch(showError);
}

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
  if (!meta || typeof meta !== "object") {
    footer.innerHTML = "";
    return;
  }
  const items = [];
  if (meta.cli) items.push(`<span>Cli: <strong>${escapeHtml(meta.cli)}</strong></span>`);
  if (meta.model) items.push(`<span>Model: <strong>${escapeHtml(meta.model)}</strong></span>`);
  // Mode / effort are persisted as strings (including "default"/"medium").
  // Show them whenever the field is present and non-empty so the footer
  // reflects what was actually sent.
  if (meta.mode !== undefined && meta.mode !== null && meta.mode !== "") {
    items.push(`<span>Mode: <strong>${escapeHtml(meta.mode)}</strong></span>`);
  }
  if (meta.effort !== undefined && meta.effort !== null && meta.effort !== "") {
    items.push(`<span>Effort: <strong>${escapeHtml(meta.effort)}</strong></span>`);
  }
  if (meta.status) items.push(`<span>Status: <strong>${escapeHtml(meta.status)}</strong></span>`);
  if (meta.thinking) items.push(`<span>Thinking on</span>`);
  if (meta.fast) items.push(`<span>Fast priority requested</span>`);
  if (meta.tokenUsage) items.push(`<span>Tokens: <strong>${escapeHtml(meta.tokenUsage)}</strong></span>`);
  if (meta.receivedAt) items.push(`<span>Received: <strong>${escapeHtml(meta.receivedAt)}</strong></span>`);
  if (meta.elapsed) items.push(`<span>Elapsed: <strong>${escapeHtml(meta.elapsed)}</strong></span>`);
  if (meta.sentAt) items.push(`<span>Sent: <strong>${escapeHtml(meta.sentAt)}</strong></span>`);
  // Wrap the metadata in a collapsible so the chat transcript stays compact
  // when re-opening a session. Users can click the toggle to inspect the
  // exact CLI, model, mode, effort, token, and timestamp fields for any
  // individual turn without having those fields crowd every reply.
  footer.innerHTML = items.length
    ? `<details class="chat-line-footer-details">` +
      `<summary class="chat-line-footer-summary" aria-label="Show message metadata">` +
      `<span class="chat-line-footer-summary-label">details</span>` +
      `</summary>` +
      `<div class="chat-line-footer-items">${items.join("")}</div>` +
      `</details>`
    : "";
}

function persistedChatMessageId(message) {
  const id = String(message?.id || "").trim();
  if (!id || id.startsWith("local-") || id.startsWith("local:")) return "";
  return id;
}

function replayUserPromptLine(message, meta = null) {
  const output = chatOutputRoot();
  if (!output) return;
  const { node, text, footer } = buildChatLineNode("user");
  text.textContent = String(message?.content || "");
  attachUserChatActions(node, message);
  renderChatLineFooter(footer, meta);
  output.appendChild(node);
}

function replayAssistantLine(content, meta = null, options = {}) {
  const output = chatOutputRoot();
  if (!output) return;
  const { node, text, footer } = buildChatLineNode("assistant");
  // Agent output is untrusted. HTML/CSS/script text must never become
  // application DOM, even when a tool has returned an entire web page.
  // renderChatBubbleHtml escapes every line and only emits a whitelist
  // of block tags, so this stays safe while exec / Parameters /
  // exec / Details become collapsible blocks.
  text.innerHTML = renderChatBubbleHtml(String(content));
  if (options.showResponseHeader) {
    renderChatResponseHeader(node, options.headerMeta || meta || {});
  }
  renderChatLineFooter(footer, meta);
  if (options.copyText) attachAssistantChatActions(node, options.copyText);
  output.appendChild(node);
}

function replayToolLine(content) {
  const output = chatOutputRoot();
  if (!output) return;
  if (!String(content || "").trim()) return;
  const { node, text, footer } = buildChatLineNode("assistant");
  node.classList.add("chat-line-tool-message");
  // Tool rows already contain structured `exec/tool / Parameters|Details`
  // sections. Render those directly so the UI does not create a second
  // outer "Tool output" collapsible around the real collapsibles.
  text.innerHTML = renderChatBubbleHtml(String(content || ""));
  renderChatLineFooter(footer, null);
  output.appendChild(node);
}

function stripAnsi(value) {
  return String(value || "").replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");
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
    if (role === "user") {
      replayUserPromptLine(raw, chatMessageFooterMeta(raw, role, controls));
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
      replayAssistantLine(
        displayContent,
        turn.showResponseFooter
          ? (turn.footerMeta || chatMessageFooterMeta(raw, role, controls, previousPrompt))
          : null,
        {
          showResponseHeader: turn.showResponseHeader === true,
          headerMeta: turn.headerMeta,
          copyText: turn.showResponseFooter ? turn.copyText : "",
        },
      );
    } else if (role === "tool") {
      replayToolLine(content);
    } else if (role === "system") {
      const output = chatOutputRoot();
      if (!output) return;
      const node = document.createElement("div");
      node.className = "chat-line-system";
      node.textContent = content;
      output.appendChild(node);
    }
  });
  scrollChatToBottom();
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
    meta.tokenUsage = formatTokenUsage(meta.tokenUsage);
  }
  delete meta.elapsedMs;
  return meta;
}

async function loadChatHistoryForSession(sessionId, opts = {}) {
  if (!sessionId) return false;
  if (stagedChatEdit()?.sourceSessionId === sessionId) return true;
  try {
    const preservedStream = preserveActiveChatStreamSnapshot(sessionId);
    const loadingOlder = opts.older === true && chatHistoryWindow.sessionId === sessionId;
    const previousOutput = chatOutputRoot();
    const previousHeight = previousOutput?.scrollHeight || 0;
    const previousTop = previousOutput?.scrollTop || 0;
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
    const page = Array.isArray(body) ? body : (body.messages || []);
    const totalCount = Number(body?.total_count ?? body?.totalCount ?? page.length) || page.length;
    const snapshotSession = body?.session || findChatSession(sessionId);
    const boardSession = isBoardChatSession(snapshotSession, sessionId);
    const snapshotLive = Boolean(snapshotSession?.active);
    if (boardSession) {
      state.boardChatSessionIds.add(sessionId);
      if (snapshotSession?.id) state.boardChatSessionIds.add(snapshotSession.id);
    }
    if (!loadingOlder) rememberChatRecovery(sessionId, body?.recovery || null);
    const recoveryState = state.chatRecoveryBySession[sessionId]?.state || "";
    const snapshotStatus = snapshotSession?.active || recoveryState === "starting"
      ? "running"
      : (["required", "failed"].includes(recoveryState) ? "failed" : "completed");
    if (snapshotSession?.id) {
      if (snapshotSession.projectPath) {
        setActiveProject(snapshotSession.projectPath);
      }
      if (boardSession) {
        hideBoardChatSessionsFromLists();
      } else {
        state.sessions = (state.sessions || []).filter((session) => session?.id !== snapshotSession.id);
        state.sessions.push(snapshotSession);
        mergeSessionIntoProjects(snapshotSession);
      }
      rememberSidebarSessionStatus({
        sessionId: snapshotSession.id,
        provider: snapshotSession.provider,
        status: snapshotStatus,
      });
    }
    const messages = loadingOlder
      ? page.concat(chatHistoryWindow.messages)
      : page;
    const split = splitCachedStreamingMessage(messages, sessionId);
    const replayMessages = split.messages;
    const streamSnapshot = preservedStream || {
      sessionId,
      buffer: split.streamingBuffer || state.chatOutputBuffersBySession[sessionId] || "",
      provider: sessionProvider(snapshotSession),
    };
    const offset = loadingOlder
      ? requestedOffset
      : Math.max(0, totalCount - page.length);
    chatHistoryWindow = { sessionId, offset, totalCount, messages: replayMessages };
    resetChatOutputDom();
    const persisted = getSessionOverridesFor(sessionId) || {};
    replayChatMessages(replayMessages, {
      session: snapshotSession,
      sessionId,
      overrides: persisted,
    });
    renderChatRecoveryCard(sessionId);
    renderChatManualCompactionCard(sessionId);
    if (snapshotLive || chatSessionIsLive(sessionId)) {
      const restored = restoreChatStreamSnapshot(sessionId, streamSnapshot);
      if (!restored) {
        ensureChatProcessing({
          provider: sessionProvider(snapshotSession),
          sessionId,
        });
      }
    }
    const output = chatOutputRoot();
    if (loadingOlder && output) {
      output.scrollTop = Math.max(0, output.scrollHeight - previousHeight + previousTop);
    } else if (opts.forceBottom || state.chatJumpToLatestPending) {
      scrollChatToBottom(true);
    } else {
      scrollChatToBottom();
    }
    maybeLoadOlderChatMessages();
    // Restore the current session controls after replaying the persisted
    // per-turn metadata in the transcript.
    loadSessionOverridesIntoState(sessionId, snapshotSession);
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
    if (!boardSession) {
      persistActiveChatSelection(sessionId, snapshotSession?.projectPath || activeProjectPath());
    }
    updateChatEmptyState();
    return true;
  } catch (error) {
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
  if (options.boardSession === true) {
    state.boardChatSessionIds.add(id);
    hideBoardChatSessionsFromLists();
  }
  setBoardChatWsSubscription(options.boardSession === true ? id : "");
  const session = findChatSession(id);
  state.pendingChatSessionId = id;
  state.chatSessionId = id;
  ensureChatPromptHistoryScope(id);
  if (options.boardSession !== true) {
    state.preferences.lastChatSessionId = id;
    savePreferences();
  }
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
  if (options.boardSession !== true) {
    persistActiveChatSelection(id, projectPath || sessionProjectPath(session, activeProjectPath()));
  }
  if (renderedCached) {
    scheduleChatReconciliation(id, { reportFailure: true });
    await loadChatPromptDraft(id).catch((error) => {
      console.warn("Unable to load prompt draft for cached session", error);
    });
    await loadChatPromptHistory(id);
    return;
  }
  await loadChatHistoryForSession(id, { forceBottom: true });
  if (options.boardSession === true) hideBoardChatSessionsFromLists();
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
  setBoardChatWsSubscription("");
  setActiveProject(projectPath);
  state.chatSessionId = "";
  state.pendingChatSessionId = "";
  state.currentSession = null;
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
}

function pickDefaultChatSession() {
  // Choose the most recent session we can find.  If the active project has
  // sessions, prefer those; otherwise fall back to the single most-recent
  // session across all projects.
  const allSessions = []
    .concat(
      ...(state.projects || []).map((p) => (p.sessions || [])
        .filter((s) => !isBoardChatSession(s))
        .map((s) => ({ ...s, projectPath: p.path }))),
    )
    .concat((state.sessions || []).filter((s) => !isBoardChatSession(s)).map((s) => ({ ...s })));
  if (!allSessions.length) return null;
  const activePath = activeProjectPath();
  const activeProject = activePath
    ? (state.projects || []).find((p) => p.path === activePath)
    : null;
  const activeSessions = activeProject
    ? (activeProject.sessions || []).filter((session) => !isBoardChatSession(session))
    : [];
  if (activeSessions.length) {
    return activeSessions
      .slice()
      .sort((a, b) => new Date(b.lastActivity || 0) - new Date(a.lastActivity || 0))[0];
  }
  // No active project (or no sessions in it): pick topmost project with the
  // most recent chat history.
  const projectsWithSessions = (state.projects || [])
    .filter((p) => (p.sessions || []).some((session) => !isBoardChatSession(session)));
  if (!projectsWithSessions.length) return null;
  // Collect all sessions, attach their project path, and pick the newest.
  return projectsWithSessions
    .flatMap((p) => (p.sessions || [])
      .filter((s) => !isBoardChatSession(s))
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
  if (isSelectedBoardChatSession()) return [];
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

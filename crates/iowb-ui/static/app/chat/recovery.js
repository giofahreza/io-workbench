function chatRecoveryRequestId() {
  if (window.crypto?.randomUUID) return `compact-${window.crypto.randomUUID()}`;
  return `compact-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;
}

function chatManualCompactionIsStarting(compaction) {
  return Boolean(compaction && (compaction.pending || compaction.state === "starting"));
}

function normalizeChatRecovery(recovery, sessionId = "") {
  if (!recovery || typeof recovery !== "object") return null;
  const failedMessageId = String(recovery.failedMessageId || recovery.failed_message_id || "").trim();
  if (!failedMessageId) return null;
  return {
    ...recovery,
    sessionId: String(sessionId || recovery.sessionId || "").trim(),
    failedMessageId,
    requestId: String(recovery.requestId || recovery.request_id || "").trim(),
    state: String(recovery.state || "required").toLowerCase(),
    pending: false,
  };
}

function chatRecoveryBlocksNormalSend(recovery) {
  return Boolean(recovery && ["required", "failed", "starting"].includes(recovery.state));
}

function chatRecoveryIsStarting(recovery) {
  return Boolean(recovery && (recovery.pending || recovery.state === "starting"));
}

function rememberChatRecovery(sessionId, recovery) {
  const sid = String(sessionId || "").trim();
  if (!sid) return;
  const normalized = normalizeChatRecovery(recovery, sid);
  if (normalized) state.chatRecoveryBySession[sid] = normalized;
  else delete state.chatRecoveryBySession[sid];
  const selected = isActiveChatSessionEvent({ sessionId: sid });
  if (normalized && ["required", "failed"].includes(normalized.state) && selected) {
    if (!state.chatProcessing?.sessionId || state.chatProcessing.sessionId === sid) {
      clearChatProcessing();
    }
    if (state.currentSession?.sessionId === sid) state.currentSession = null;
    const session = findChatSession(sid);
    rememberSidebarSessionStatus({
      sessionId: sid,
      provider: sessionProvider(session),
      status: "failed",
    });
    rememberCurrentChatSession({ sessionId: sid, live: false, status: "failed" });
  }
  if (selected || chatHistoryWindow.sessionId === sid) {
    renderChatRecoveryCard(sid);
    renderChatManualCompactionCard(sid);
    updateChatComposerState();
  }
}

function renderChatRecoveryCard(sessionId = state.chatSessionId) {
  const output = chatOutputRoot();
  if (!output) return;
  output.querySelectorAll(".chat-context-recovery").forEach((node) => node.remove());
  const recovery = state.chatRecoveryBySession[String(sessionId || "").trim()];
  if (!recovery || !isActiveChatSessionEvent({ sessionId })) return;
  const card = document.createElement("section");
  card.className = "chat-context-recovery chat-line-system";
  const busy = chatRecoveryIsStarting(recovery);
  const title = busy
    ? "Compacting context"
    : recovery.state === "failed"
    ? "Clean-context compaction failed"
    : "This chat needs a clean context";
  const continuity = recovery.state === "failed"
    ? "Your original context mapping is still intact. Your chat, URL, title, settings, and visible messages remain unchanged."
    : "Your chat, URL, title, settings, and all visible messages stay unchanged.";
  const detail = recovery.observedBytes
    ? ` Current native context: ${formatBytes(recovery.observedBytes)}; gateway limit: ${formatBytes(recovery.limitBytes || 0)}.`
    : "";
  card.innerHTML = `<div class="chat-context-recovery-copy">
      <strong>${title}</strong>
      <span>${escapeHtml(recovery.message || "The native Codex context is too large to continue safely.")}${escapeHtml(detail)}</span>
      <span>${escapeHtml(continuity)}</span>
    </div>
    <button type="button" class="primary-action" data-chat-compact-retry${busy ? " disabled" : ""}>${busy ? "Compacting…" : "Compact & retry"}</button>`;
  output.appendChild(card);
  card.querySelector("[data-chat-compact-retry]")?.addEventListener("click", (event) => {
    compactAndRetryChatContext(sessionId, event.currentTarget).catch(showError);
  });
  scrollChatToBottom();
}

async function compactAndRetryChatContext(sessionId, button = null) {
  const sid = String(sessionId || "").trim();
  const recovery = state.chatRecoveryBySession[sid];
  if (!sid || !recovery || recovery.pending || recovery.state === "starting") return;
  recovery.pending = true;
  recovery.state = "starting";
  const requestId = chatRecoveryRequestId();
  recovery.requestId = requestId;
  recovery.responseId = "";
  const attemptIsCurrent = () => state.chatRecoveryBySession[sid] === recovery
    && recovery.requestId === requestId
    && recovery.pending
    && recovery.state === "starting";
  renderChatRecoveryCard(sid);
  updateChatComposerState();
  try {
    const response = await api(`/api/sessions/${encodeURIComponent(sid)}/compact-and-retry`, {
      method: "POST",
      body: JSON.stringify({
        requestId: recovery.requestId,
        failedMessageId: recovery.failedMessageId,
      }),
    });
    if (!attemptIsCurrent()) return;
    recovery.pending = false;
    const responseState = String(response?.state || "starting").toLowerCase();
    recovery.state = responseState;
    recovery.responseId = String(response?.responseId || response?.response_id || "").trim();
    if (responseState === "failed") {
      recovery.message = response?.message
        || "The clean-context compaction failed. Your original context mapping was kept; you can try again.";
      clearChatProcessing();
      renderChatRecoveryCard(sid);
      updateChatComposerState();
      return;
    }
    if (responseState !== "starting") {
      delete state.chatRecoveryBySession[sid];
      clearChatProcessing();
      renderChatRecoveryCard(sid);
      updateChatComposerState();
      scheduleCompletedChatReconciliation(sid);
      return;
    }
    renderChatRecoveryCard(sid);
    ensureChatProcessing({ provider: "codex", sessionId: sid });
    updateProcessingLabel("Compacting context");
  } catch (error) {
    if (!attemptIsCurrent()) return;
    recovery.pending = false;
    recovery.state = "failed";
    recovery.message = error?.message || String(error);
    renderChatRecoveryCard(sid);
    updateChatComposerState();
    throw error;
  }
}

function renderChatManualCompactionCard(sessionId = state.chatSessionId) {
  const output = chatOutputRoot();
  if (!output) return;
  output.querySelectorAll(".chat-manual-compaction").forEach((node) => node.remove());
  const sid = String(sessionId || "").trim();
  const compaction = state.chatManualCompactionBySession[sid];
  if (!compaction || !isActiveChatSessionEvent({ sessionId: sid })) return;
  const busy = chatManualCompactionIsStarting(compaction);
  const card = document.createElement("section");
  card.className = "chat-manual-compaction chat-line-system";
  const title = busy ? "Compacting context" : "Manual compaction failed";
  const message = busy
    ? "Preparing a clean Codex context for future turns."
    : (compaction.message || "The clean-context compaction failed. Your original context mapping was kept; you can try again.");
  card.innerHTML = `<div class="chat-manual-compaction-copy">
      <strong>${escapeHtml(title)}</strong>
      <span>${escapeHtml(message)}</span>
      <span>Your chat, URL, title, settings, draft, project, and visible messages stay unchanged.</span>
    </div>
    <div class="chat-manual-compaction-actions">
      ${busy ? "" : `<button type="button" class="secondary-action" data-chat-manual-compact-dismiss>Dismiss</button>`}
      <button type="button" class="primary-action" data-chat-manual-compact-retry${busy ? " disabled" : ""}>${busy ? "Compacting…" : "Try again"}</button>
    </div>`;
  output.appendChild(card);
  card.querySelector("[data-chat-manual-compact-retry]")?.addEventListener("click", (event) => {
    compactChatSessionContext(sid, event.currentTarget, { skipConfirm: true }).catch(showError);
  });
  card.querySelector("[data-chat-manual-compact-dismiss]")?.addEventListener("click", () => {
    delete state.chatManualCompactionBySession[sid];
    renderChatManualCompactionCard(sid);
    updateChatComposerState();
  });
  scrollChatToBottom();
}

function selectedChatCanManualCompact() {
  const sid = String(state.chatSessionId || "").trim();
  if (!sid || sid.startsWith("local-") || !canLoadProtectedData()) return false;
  const session = findChatSession(sid) || cachedChatSession(sid)?.session || {};
  const provider = String(sessionProvider(session) || chatCliValue() || "").toLowerCase();
  if (provider !== "codex") return false;
  const recovery = state.chatRecoveryBySession[sid];
  if (chatRecoveryBlocksNormalSend(recovery)) return false;
  if (selectedRunningChatSession() || selectedChatIsStopping() || state.chatProcessing) return false;
  const loadedCount = chatHistoryWindow.sessionId === sid ? chatHistoryWindow.messages.length : 0;
  const storedCount = Number(session.messageCount ?? session.message_count ?? 0) || 0;
  return loadedCount > 0 || storedCount > 0;
}

async function compactChatSessionContext(sessionId, button = null, opts = {}) {
  const sid = String(sessionId || "").trim();
  if (!sid) return;
  if (!selectedChatCanManualCompact()) {
    showToast("Open an idle Codex chat with messages before compacting context.", "danger");
    return;
  }
  if (!opts.skipConfirm) {
    const confirmed = window.confirm(
      "Compact this chat into a clean Codex context? The visible chat stays unchanged.",
    );
    if (!confirmed) return;
  }
  const requestId = chatRecoveryRequestId();
  const compaction = {
    sessionId: sid,
    requestId,
    responseId: "",
    state: "starting",
    pending: true,
    message: "Preparing a clean Codex context for future turns.",
  };
  state.chatManualCompactionBySession[sid] = compaction;
  renderChatManualCompactionCard(sid);
  updateChatComposerState();
  ensureChatProcessing({ provider: "codex", sessionId: sid });
  updateProcessingLabel("Compacting context");
  const attemptIsCurrent = () => state.chatManualCompactionBySession[sid] === compaction
    && compaction.requestId === requestId
    && compaction.state === "starting";
  if (button) button.disabled = true;
  try {
    const response = await api(`/api/sessions/${encodeURIComponent(sid)}/compact`, {
      method: "POST",
      body: JSON.stringify({ requestId }),
    });
    if (!attemptIsCurrent()) return;
    compaction.pending = false;
    const responseState = String(response?.state || "starting").toLowerCase();
    compaction.state = responseState;
    compaction.responseId = String(response?.responseId || response?.response_id || "").trim();
    if (responseState === "failed") {
      compaction.message = response?.message
        || "The clean-context compaction failed. Your original context mapping was kept; you can try again.";
      clearChatProcessing();
      renderChatManualCompactionCard(sid);
      updateChatComposerState();
      return;
    }
    if (responseState !== "starting") {
      delete state.chatManualCompactionBySession[sid];
      clearChatProcessing();
      renderChatManualCompactionCard(sid);
      updateChatComposerState();
      scheduleCompletedChatReconciliation(sid);
      return;
    }
    renderChatManualCompactionCard(sid);
    ensureChatProcessing({ provider: "codex", sessionId: sid });
    updateProcessingLabel("Compacting context");
  } catch (error) {
    if (!attemptIsCurrent()) return;
    compaction.pending = false;
    compaction.state = "failed";
    compaction.message = error?.message || String(error);
    clearChatProcessing();
    renderChatManualCompactionCard(sid);
    updateChatComposerState();
    throw error;
  } finally {
    if (button) button.disabled = false;
    updateChatComposerState();
  }
}

function chatEventSessionIds() {
  return new Set([
    state.chatSessionId,
    state.pendingChatSessionId,
  ].filter(Boolean));
}

function selectedChatIsStopping() {
  const ids = chatEventSessionIds();
  return Boolean(state.chatStoppingSessionId && ids.has(state.chatStoppingSessionId));
}

function isActiveChatSessionEvent(payload = {}) {
  if (!payload.sessionId) return false;
  const ids = chatEventSessionIds();
  if (!ids.size) return false;
  return ids.has(payload.sessionId);
}

function chatEventResponseId(payload = {}) {
  return String(payload.responseId || payload.response_id || "").trim();
}

function chatEventSequence(payload = {}) {
  const value = payload.sequence;
  if (value == null || value === "") return null;
  const number = Number(value);
  return Number.isFinite(number) ? number : null;
}

function chatRecoveryMatchesResponse(recovery, payload = {}) {
  const expected = String(recovery?.responseId || "").trim();
  const observed = chatEventResponseId(payload);
  return Boolean(expected && observed && expected === observed);
}

function chatManualCompactionMatchesResponse(compaction, payload = {}) {
  if (!compaction || compaction.state !== "starting") return false;
  const expected = String(compaction.responseId || "").trim();
  const observed = chatEventResponseId(payload);
  return Boolean(
    (expected && observed && expected === observed)
    || (!expected && observed)
  );
}

function rememberManualCompactionSuppression(sessionId, responseId) {
  const sid = String(sessionId || "").trim();
  const rid = String(responseId || "").trim();
  if (!sid || !rid) return;
  state.chatManualCompactionSuppressedResponsesBySession[sid] = rid;
}

function isSuppressedManualCompactionResponse(payload = {}) {
  const sid = String(payload.sessionId || "").trim();
  const rid = chatEventResponseId(payload);
  return Boolean(sid && rid && state.chatManualCompactionSuppressedResponsesBySession[sid] === rid);
}

function chatResponseState(sessionId) {
  const id = (sessionId || "").trim();
  if (!id) return {};
  const current = state.chatResponseStateBySession[id];
  if (current && typeof current === "object") return current;
  const next = { activeResponseId: "", completedResponseId: "", sequence: 0 };
  state.chatResponseStateBySession[id] = next;
  return next;
}

function acceptsOrderedChatResponseEvent(payload = {}, options = {}) {
  const sessionId = String(payload.sessionId || "").trim();
  if (!sessionId) return true;
  const responseId = chatEventResponseId(payload);
  const sequence = chatEventSequence(payload);
  if (!responseId || sequence == null) return true;
  const tracked = chatResponseState(sessionId);
  const runningEvent = options.runningEvent === true;
  const allowNewResponse = options.allowNewResponse === true;
  if (responseId === tracked.completedResponseId) {
    return !runningEvent && sequence > (Number(tracked.sequence) || 0);
  }
  if (tracked.activeResponseId && tracked.activeResponseId !== responseId) {
    return false;
  }
  if (!tracked.activeResponseId && responseId !== tracked.completedResponseId && !allowNewResponse) {
    return false;
  }
  if (!tracked.activeResponseId && responseId !== tracked.completedResponseId && allowNewResponse) {
    return true;
  }
  return sequence > (Number(tracked.sequence) || 0);
}

function rememberOrderedChatResponseEvent(payload = {}, options = {}) {
  const sessionId = String(payload.sessionId || "").trim();
  const responseId = chatEventResponseId(payload);
  const sequence = chatEventSequence(payload);
  if (!sessionId || !responseId || sequence == null) return;
  const tracked = chatResponseState(sessionId);
  const responseChanged = tracked.activeResponseId !== responseId
    && tracked.completedResponseId !== responseId;
  tracked.sequence = responseChanged ? sequence : Math.max(Number(tracked.sequence) || 0, sequence);
  if (options.terminal === true) {
    tracked.activeResponseId = "";
    tracked.completedResponseId = responseId;
  } else {
    tracked.activeResponseId = responseId;
  }
}

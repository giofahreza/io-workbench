function resetChatOutputDom() {
  const output = qs("#chat-output");
  if (output) output.innerHTML = "";
  chatStream = { role: null, node: null, text: null, buffer: "" };
  state.chatProcessing = null;
  state.chatBuffer = "";
  renderChatFooter(null);
  updateChatEmptyState();
}

// The current assistant message that's being streamed into.  We keep
// references so each new chunk renders into the right text node without
// tearing down the existing struct.
let chatStream = { role: null, node: null, text: null, buffer: "" };
const CHAT_HISTORY_PAGE_SIZE = 30;
const CHAT_LIVE_RENDER_MAX_CHARS = 128 * 1024;
let chatHistoryWindow = { sessionId: "", offset: 0, totalCount: 0, messages: [] };

function chatOutputRoot() {
  return qs("#chat-output");
}

function normalizedWebServerUrl() {
  return window.location.origin.replace(/\/+$/, "");
}

function chatCacheKey(sessionId) {
  return `${normalizedWebServerUrl()}::session::${sessionId || ""}`;
}

function activeChatSelectionMatchesServer() {
  const savedServer = safeLocalStorageGet(ACTIVE_CHAT_SERVER_KEY, "") || "";
  return savedServer && savedServer === normalizedWebServerUrl();
}

function savedActiveChatSessionId() {
  if (!activeChatSelectionMatchesServer()) return "";
  return (safeLocalStorageGet(ACTIVE_CHAT_SESSION_KEY, "") || "").trim();
}

function persistActiveChatSelection(sessionId = state.chatSessionId, projectPath = activeProjectPath()) {
  const id = (sessionId || "").trim();
  if (!id) return;
  safeLocalStorageSet(ACTIVE_CHAT_SESSION_KEY, id);
  safeLocalStorageSet(ACTIVE_CHAT_SERVER_KEY, normalizedWebServerUrl());
  safeLocalStorageSet(ACTIVE_CHAT_PROJECT_KEY, projectPath || "");
  state.preferences.lastChatSessionId = id;
  savePreferences();
}

function clearActiveChatSelection(sessionId = state.chatSessionId) {
  const saved = savedActiveChatSessionId();
  if (sessionId && saved && saved !== sessionId) return;
  safeLocalStorageRemove(ACTIVE_CHAT_SESSION_KEY);
  safeLocalStorageRemove(ACTIVE_CHAT_SERVER_KEY);
  safeLocalStorageRemove(ACTIVE_CHAT_PROJECT_KEY);
}

function serializedChatCacheStorageBytes(value) {
  try {
    const serialized = typeof value === "string" ? value : JSON.stringify(value);
    return typeof serialized === "string"
      ? serialized.length * 2
      : Number.MAX_SAFE_INTEGER;
  } catch {
    return Number.MAX_SAFE_INTEGER;
  }
}

function truncateChatCacheText(value, maxChars) {
  const text = String(value ?? "");
  const limit = Math.max(0, Number(maxChars) || 0);
  if (text.length <= limit) return text;
  if (!limit) return "";
  const marker = "\n[content truncated in local cache]\n";
  if (limit <= marker.length) return text.slice(-limit);
  const remaining = limit - marker.length;
  const headChars = Math.floor(remaining / 3);
  return `${text.slice(0, headChars)}${marker}${text.slice(-remaining + headChars)}`;
}

function compactChatCacheMetadata(raw) {
  const source = raw?.metadata && typeof raw.metadata === "object"
    ? raw.metadata
    : (raw?.meta && typeof raw.meta === "object" ? raw.meta : null);
  if (!source) return null;
  const metadata = {};
  [
    "kind",
    "cli",
    "provider",
    "model",
    "effort",
    "mode",
    "thinking",
    "fast",
    "sentAt",
    "receivedAt",
    "elapsed",
    "elapsedMs",
    "tokenUsage",
  ].forEach((field) => {
    const value = source[field];
    if (value === undefined || value === null) return;
    if (["string", "number", "boolean"].includes(typeof value)) {
      metadata[field] = typeof value === "string"
        ? truncateChatCacheText(value, 4096)
        : value;
    } else if (field === "tokenUsage" && typeof value === "object") {
      const usage = {};
      ["used", "input", "output", "total", "missingAttempts", "partialAttempts", "completeness"]
        .forEach((usageField) => {
          const usageValue = value[usageField];
          if (usageValue !== undefined && usageValue !== null) usage[usageField] = usageValue;
        });
      if (Object.keys(usage).length) metadata[field] = usage;
    }
  });
  return Object.keys(metadata).length ? metadata : null;
}

function compactChatCacheSession(session) {
  if (!session || typeof session !== "object") return null;
  const compact = {};
  [
    "id",
    "title",
    "summary",
    "provider",
    "__provider",
    "cli",
    "projectPath",
    "projectName",
    "nativeSessionId",
    "native_session_id",
    "messageCount",
    "message_count",
    "status",
    "active",
    "pending",
    "external",
    "boardSession",
    "board_session",
    "boardId",
    "board_id",
    "boardRunId",
    "board_run_id",
    "boardTaskId",
    "board_task_id",
    "model",
    "effort",
    "mode",
    "thinking",
    "fast",
    "lastActivity",
    "updatedAt",
    "createdAt",
  ].forEach((field) => {
    const value = session[field];
    if (value === undefined || value === null) return;
    if (typeof value === "string") compact[field] = truncateChatCacheText(value, 4096);
    else if (["number", "boolean"].includes(typeof value)) compact[field] = value;
  });
  return Object.keys(compact).length ? compact : null;
}

function compactChatCacheMessage(raw, contentLimit, includeMetadata = true) {
  if (!raw || typeof raw !== "object") return null;
  const message = {};
  ["id", "role", "timestamp", "receivedAt", "provider"].forEach((field) => {
    const value = raw[field];
    if (value !== undefined && value !== null) {
      message[field] = typeof value === "string" ? truncateChatCacheText(value, 4096) : value;
    }
  });
  message.content = truncateChatCacheText(raw.content, contentLimit);
  if (includeMetadata) {
    const metadata = compactChatCacheMetadata(raw);
    if (metadata) message.metadata = metadata;
  }
  return message;
}

function buildBoundedChatCacheEntry(raw, messages, droppedMessages, contentLimit, streamLimit, options = {}) {
  const offset = Math.max(0, Number(raw.offset) || 0) + droppedMessages;
  const totalCount = Math.max(Number(raw.totalCount) || 0, offset + messages.length);
  const sessionId = truncateChatCacheText(raw.sessionId, 1024);
  const entry = {
    key: truncateChatCacheText(raw.key, 2048),
    sessionId,
    projectPath: truncateChatCacheText(raw.projectPath, 4096),
    session: options.includeSession === false ? null : compactChatCacheSession(raw.session),
    status: truncateChatCacheText(raw.status, 64),
    messages: messages
      .map((message) => compactChatCacheMessage(message, contentLimit, options.includeMetadata !== false))
      .filter(Boolean),
    offset,
    totalCount,
    streamingBuffer: raw.live
      ? truncateChatCacheText(raw.streamingBuffer, streamLimit)
      : "",
    live: raw.live === true,
    updatedAt: truncateChatCacheText(raw.updatedAt, 64),
  };
  if (!entry.session) delete entry.session;
  return entry;
}

function boundedChatCacheEntry(raw) {
  if (!raw || typeof raw !== "object") return null;
  const originalMessages = Array.isArray(raw.messages) ? raw.messages : [];
  let messages = originalMessages.slice(-MAX_CHAT_TRANSCRIPT_CACHE_MESSAGES);
  let droppedMessages = originalMessages.length - messages.length;
  let contentLimit = MAX_CHAT_TRANSCRIPT_MESSAGE_CONTENT_CHARS;
  let streamLimit = MAX_CHAT_TRANSCRIPT_STREAMING_CONTENT_CHARS;
  let entry = buildBoundedChatCacheEntry(
    raw,
    messages,
    droppedMessages,
    contentLimit,
    streamLimit,
  );

  while (
    serializedChatCacheStorageBytes(entry) > MAX_CHAT_TRANSCRIPT_CACHE_ENTRY_STORAGE_BYTES
    && messages.length > 1
  ) {
    messages = messages.slice(1);
    droppedMessages += 1;
    entry = buildBoundedChatCacheEntry(
      raw,
      messages,
      droppedMessages,
      contentLimit,
      streamLimit,
    );
  }

  for (const limit of [64 * 1024, 32 * 1024, 16 * 1024, 8 * 1024, 4 * 1024, 1024, 0]) {
    if (serializedChatCacheStorageBytes(entry) <= MAX_CHAT_TRANSCRIPT_CACHE_ENTRY_STORAGE_BYTES) break;
    contentLimit = limit;
    streamLimit = Math.min(MAX_CHAT_TRANSCRIPT_STREAMING_CONTENT_CHARS, limit * 2 / 3);
    entry = buildBoundedChatCacheEntry(
      raw,
      messages,
      droppedMessages,
      contentLimit,
      streamLimit,
    );
  }

  if (serializedChatCacheStorageBytes(entry) > MAX_CHAT_TRANSCRIPT_CACHE_ENTRY_STORAGE_BYTES) {
    entry = buildBoundedChatCacheEntry(
      raw,
      messages.slice(-1),
      droppedMessages + Math.max(0, messages.length - 1),
      0,
      0,
      { includeSession: false, includeMetadata: false },
    );
  }
  return entry;
}

function boundedChatTranscriptCacheEntries(entries) {
  const normalized = (Array.isArray(entries) ? entries : [])
    .map(boundedChatCacheEntry)
    .filter((entry) => entry?.key && entry.sessionId)
    .slice(-MAX_CHAT_TRANSCRIPT_CACHE);
  let retained = [];
  for (let index = normalized.length - 1; index >= 0 && retained.length < MAX_CHAT_TRANSCRIPT_CACHE; index -= 1) {
    const next = [normalized[index], ...retained];
    if (
      retained.length > 0
      && serializedChatCacheStorageBytes({ version: CHAT_TRANSCRIPT_CACHE_VERSION, entries: next })
        > MAX_CHAT_TRANSCRIPT_CACHE_STORAGE_BYTES
    ) {
      break;
    }
    retained = next;
  }
  return retained;
}

function chatCacheEntries() {
  if (!state.chatTranscriptCache || state.chatTranscriptCache.version !== CHAT_TRANSCRIPT_CACHE_VERSION) {
    state.chatTranscriptCache = { version: CHAT_TRANSCRIPT_CACHE_VERSION, entries: [] };
    state.chatTranscriptCacheNormalized = true;
  }
  if (!Array.isArray(state.chatTranscriptCache.entries)) {
    state.chatTranscriptCache.entries = [];
  }
  if (!state.chatTranscriptCacheNormalized) {
    state.chatTranscriptCache.entries = boundedChatTranscriptCacheEntries(state.chatTranscriptCache.entries);
    state.chatTranscriptCacheNormalized = true;
  }
  return state.chatTranscriptCache.entries;
}

function persistChatTranscriptCache() {
  const entries = boundedChatTranscriptCacheEntries(chatCacheEntries());
  state.chatTranscriptCache = { version: CHAT_TRANSCRIPT_CACHE_VERSION, entries };
  state.chatTranscriptCacheNormalized = true;
  let persisted = false;
  try {
    persisted = safeLocalStorageSet(CHAT_TRANSCRIPT_CACHE_KEY, JSON.stringify(state.chatTranscriptCache));
  } catch {
    persisted = false;
  }
  if (persisted) return true;
  // The cache is disposable. Removing an older oversized value can free
  // enough quota for future preferences/drafts even when this write cannot
  // be committed right now; history loading must continue either way.
  safeLocalStorageRemove(CHAT_TRANSCRIPT_CACHE_KEY);
  localStorageWriteFailures.add(CHAT_TRANSCRIPT_CACHE_KEY);
  try {
    return safeLocalStorageSet(CHAT_TRANSCRIPT_CACHE_KEY, JSON.stringify(state.chatTranscriptCache));
  } catch {
    return false;
  }
}

function cachedChatSession(sessionId) {
  const key = chatCacheKey((sessionId || "").trim());
  return chatCacheEntries().find((entry) => entry.key === key) || null;
}

function chatStreamingMessageId(sessionId) {
  return `local-stream-${sessionId || ""}`;
}

function chatSessionIsLive(sessionId = state.chatSessionId) {
  const live = state.sessionStatusById?.[sessionId];
  return normalizeSidebarSessionStatus(live?.status) === "running"
    || Boolean(state.chatProcessing?.sessionId === sessionId)
    || Boolean(state.currentSession?.sessionId === sessionId);
}

function currentChatStreamSnapshot(sessionId = state.chatSessionId || state.pendingChatSessionId) {
  const sid = String(sessionId || "").trim();
  if (!sid) return null;
  let buffer = String(state.chatOutputBuffersBySession[sid] || "");
  if ((state.chatSessionId === sid || state.pendingChatSessionId === sid) && state.chatBuffer) {
    buffer = String(state.chatBuffer || "");
  }
  if (!buffer.trim()) return null;
  const session = findChatSession(sid) || cachedChatSession(sid)?.session || null;
  const persisted = getSessionOverridesFor(sid) || {};
  const provider = state.currentSession?.sessionId === sid
    ? state.currentSession.provider
    : (persisted.cli || persisted.provider || sessionProvider(session) || chatCliValue() || "");
  return {
    sessionId: sid,
    buffer: buffer.slice(-CHAT_LIVE_RENDER_MAX_CHARS),
    provider,
  };
}

function splitCachedStreamingMessage(messages, sessionId) {
  const streamingId = chatStreamingMessageId(sessionId);
  let streamingBuffer = "";
  const visibleMessages = [];
  for (const message of Array.isArray(messages) ? messages : []) {
    if (message?.id === streamingId) {
      streamingBuffer = String(message.content || "");
      continue;
    }
    visibleMessages.push(message);
  }
  return { messages: visibleMessages, streamingBuffer };
}

function rememberCurrentChatSession(patch = {}) {
  const sessionId = (patch.sessionId || state.chatSessionId || state.pendingChatSessionId || "").trim();
  if (!sessionId) return;
  const key = chatCacheKey(sessionId);
  const cached = cachedChatSession(sessionId);
  const entries = chatCacheEntries().filter((entry) => entry.key !== key);
  const session = patch.session || findChatSession(sessionId) || cached?.session || null;
  const boardSession = isBoardChatSession(session, sessionId);
  const projectPath = patch.projectPath || sessionProjectPath(session, activeProjectPath());
  const messages = Array.isArray(patch.messages)
    ? patch.messages
    : (chatHistoryWindow.sessionId === sessionId ? chatHistoryWindow.messages : (cached?.messages || []));
  const split = splitCachedStreamingMessage(messages, sessionId);
  const streamingBuffer = typeof patch.streamingBuffer === "string"
    ? patch.streamingBuffer
    : (split.streamingBuffer || (patch.live ? currentChatStreamSnapshot(sessionId)?.buffer || "" : ""));
  const offset = Number(patch.offset ?? (chatHistoryWindow.sessionId === sessionId ? chatHistoryWindow.offset : cached?.offset || 0)) || 0;
  const totalCount = Number(patch.totalCount ?? (chatHistoryWindow.sessionId === sessionId ? chatHistoryWindow.totalCount : cached?.totalCount || split.messages.length)) || split.messages.length;
  entries.push({
    key,
    sessionId,
    projectPath,
    session: boardSession && session ? { ...session, boardSession: true } : session,
    status: patch.status || (session ? sidebarSessionStatus(session) : "") || (chatSessionIsLive(sessionId) ? "running" : "completed"),
    messages: split.messages.slice(-MAX_CHAT_TRANSCRIPT_CACHE_MESSAGES),
    offset: Math.max(0, offset + Math.max(0, split.messages.length - MAX_CHAT_TRANSCRIPT_CACHE_MESSAGES)),
    totalCount: Math.max(totalCount, split.messages.length),
    streamingBuffer: (patch.live ?? chatSessionIsLive(sessionId))
      ? streamingBuffer.slice(-CHAT_LIVE_RENDER_MAX_CHARS)
      : "",
    live: patch.live ?? chatSessionIsLive(sessionId),
    updatedAt: new Date().toISOString(),
  });
  state.chatTranscriptCache.entries = entries.slice(-MAX_CHAT_TRANSCRIPT_CACHE);
  persistChatTranscriptCache();
  if (boardSession) hideBoardChatSessionsFromLists();
}

function preserveActiveChatStreamSnapshot(sessionId = state.chatSessionId || state.pendingChatSessionId) {
  const snapshot = currentChatStreamSnapshot(sessionId);
  if (!snapshot) return null;
  state.chatOutputBuffersBySession[snapshot.sessionId] = snapshot.buffer;
  rememberCurrentChatSession({
    sessionId: snapshot.sessionId,
    live: true,
    status: "running",
    streamingBuffer: snapshot.buffer,
  });
  return snapshot;
}

function restoreChatStreamSnapshot(sessionId, snapshot = null) {
  const sid = String(sessionId || "").trim();
  if (!sid) return false;
  const buffer = String(snapshot?.buffer || state.chatOutputBuffersBySession[sid] || "");
  if (!buffer.trim()) return false;
  state.chatOutputBuffersBySession[sid] = buffer.slice(-CHAT_LIVE_RENDER_MAX_CHARS);
  state.chatBuffer = "";
  appendChat(state.chatOutputBuffersBySession[sid], {
    provider: snapshot?.provider || "",
    sessionId: sid,
  });
  return true;
}

function renderCachedChatSession(sessionId) {
  const cached = cachedChatSession(sessionId);
  if (!cached) return false;
  if (isBoardChatSession(cached.session, sessionId)) {
    state.boardChatSessionIds.add(sessionId);
    hideBoardChatSessionsFromLists();
  }
  const split = splitCachedStreamingMessage(cached.messages, sessionId);
  const messages = split.messages;
  const streamingBuffer = String(cached.streamingBuffer || split.streamingBuffer || state.chatOutputBuffersBySession[sessionId] || "");
  state.chatSessionId = sessionId;
  state.pendingChatSessionId = "";
  chatHistoryWindow = {
    sessionId,
    offset: Math.max(0, Number(cached.offset) || 0),
    totalCount: Math.max(Number(cached.totalCount) || messages.length, messages.length),
    messages,
  };
  resetChatOutputDom();
  replayChatMessages(messages);
  scrollChatToBottom(true);
  if (cached.live || chatSessionIsLive(sessionId)) {
    const restored = restoreChatStreamSnapshot(sessionId, {
      buffer: streamingBuffer,
      provider: sessionProvider(cached.session),
    });
    if (!restored) {
      ensureChatProcessing({
        sessionId,
        provider: sessionProvider(cached.session),
      });
    }
  }
  loadSessionOverridesIntoState(sessionId, cached.session);
  renderChatFooter(null);
  updateChatEmptyState();
  return true;
}

function rememberBackgroundChatOutput(payload = {}) {
  const sessionId = (payload.sessionId || "").trim();
  if (!sessionId || isActiveChatSessionEvent(payload)) return false;
  if (state.boardChatSessionIds.has(sessionId)) return false;
  const cached = cachedChatSession(sessionId);
  const session = findChatSession(sessionId) || cached?.session || null;
  if (!cached && !session) return false;
  const previous = state.chatOutputBuffersBySession[sessionId] || "";
  const nextBuffer = `${previous}${payload.content || ""}`.slice(-CHAT_LIVE_RENDER_MAX_CHARS);
  const sessionControls = getSessionOverridesFor(sessionId) || {};
  let messages = Array.isArray(cached?.messages) ? cached.messages.slice() : [];
  if (nextBuffer.trim()) {
    const streamingId = `local-stream-${sessionId}`;
    const existingIndex = messages.findIndex((message) => message.id === streamingId);
    const message = {
      id: payload.done ? `local-assistant-${Date.now()}` : streamingId,
      role: "assistant",
      content: nextBuffer,
      timestamp: new Date().toISOString(),
      metadata: {
        cli: payload.provider || sessionProvider(session),
        model: state.preferences.chatModel || "",
        effort: state.preferences.chatEffort || "",
        mode: state.preferences.chatMode || "",
        fast: sessionControls.fast ?? state.preferences.chatFast ?? false,
      },
    };
    if (existingIndex >= 0) messages[existingIndex] = message;
    else messages = messages.concat(message);
  }
  if (payload.done) delete state.chatOutputBuffersBySession[sessionId];
  else state.chatOutputBuffersBySession[sessionId] = nextBuffer;
  rememberCurrentChatSession({
    sessionId,
    session,
    messages,
    live: !payload.done,
    status: payload.done ? "completed" : "running",
  });
  if (payload.done) scheduleCompletedChatReconciliation(sessionId);
  return true;
}

function chatDisplaySettingsScope(sessionId = state.chatSessionId) {
  const id = (sessionId || "").trim();
  const server = normalizedWebServerUrl();
  if (id) return `${server}::session::${id}`;
  return `${server}::new`;
}

function chatDisplaySettingsKey(scope = chatDisplaySettingsScope()) {
  return `iowb.chatDisplaySettings:${scope}`;
}

function chatDisplaySettingsScopeLabel() {
  const session = state.chatSessionId?.trim();
  const title = session ? findChatSession(session)?.title : "";
  return title || (session ? `Session ${session}` : "New chat");
}

function chatDisplaySettings() {
  const scoped = readJsonStorage(chatDisplaySettingsKey(), null);
  if (scoped && typeof scoped === "object") {
    return {
      expandThinking: scoped.expandThinking !== false,
      expandParameters: scoped.expandParameters !== false,
      autoScrollToBottom: scoped.autoScrollToBottom === true,
    };
  }
  return {
    expandThinking: state.preferences.chatExpandThinking !== false,
    expandParameters: state.preferences.chatExpandParameters !== false,
    autoScrollToBottom: state.preferences.chatAutoScrollToBottom === true,
  };
}

function saveChatDisplaySettings(settings) {
  safeLocalStorageSetJson(chatDisplaySettingsKey(), {
    expandThinking: settings.expandThinking !== false,
    expandParameters: settings.expandParameters !== false,
    autoScrollToBottom: settings.autoScrollToBottom === true,
  });
}

function isChatNearBottom() {
  const output = chatOutputRoot();
  if (!output || output.scrollHeight <= output.clientHeight) return true;
  return output.scrollHeight - output.clientHeight - output.scrollTop <= CHAT_AUTOSCROLL_THRESHOLD_PX;
}

function scrollChatToBottom(force = false) {
  const shouldScroll = force || state.chatJumpToLatestPending || chatDisplaySettings().autoScrollToBottom;
  if (!shouldScroll) {
    updateChatJumpToLatestButton();
    return;
  }
  const output = chatOutputRoot();
  if (!output) return;
  output.scrollTop = output.scrollHeight;
  state.chatJumpToLatestPending = false;
  updateChatJumpToLatestButton();
}

function updateChatJumpToLatestButton() {
  const button = qs("#chat-jump-latest");
  if (!button) return;
  const output = chatOutputRoot();
  const hasRows = Boolean(output && output.children.length > 0 && output.textContent.trim());
  button.classList.toggle("hidden", activeView() !== "chat" || !hasRows);
  button.disabled = !hasRows || state.chatJumpToLatestPending || isChatNearBottom();
}

function jumpToLatestChatMessage() {
  state.chatJumpToLatestPending = true;
  scrollChatToBottom(true);
}

function showOlderMessagesLoading() {
  const output = chatOutputRoot();
  if (!output || output.querySelector(".chat-history-loading")) return;
  const row = document.createElement("div");
  row.className = "chat-history-loading";
  row.textContent = "Loading older messages";
  output.prepend(row);
}

function maybeLoadOlderChatMessages() {
  const sessionId = state.chatSessionId || "";
  const output = chatOutputRoot();
  if (
    activeView() !== "chat" ||
    !sessionId ||
    state.chatOlderMessagesLoadingSessionId ||
    chatHistoryWindow.sessionId !== sessionId ||
    chatHistoryWindow.offset <= 0 ||
    !output ||
    output.scrollTop > CHAT_HISTORY_LOAD_THRESHOLD_PX
  ) {
    return;
  }
  state.chatOlderMessagesLoadingSessionId = sessionId;
  showOlderMessagesLoading();
  loadChatHistoryForSession(sessionId, { older: true }).finally(() => {
    if (state.chatOlderMessagesLoadingSessionId === sessionId) {
      state.chatOlderMessagesLoadingSessionId = "";
    }
  });
}

function buildChatLineNode(role) {
  const node = document.createElement("div");
  node.className = role === "user" ? "chat-line-user" : "chat-line-assistant";
  const text = document.createElement("div");
  text.className = "chat-line-text";
  const footer = document.createElement("div");
  footer.className = "chat-line-footer";
  node.appendChild(text);
  node.appendChild(footer);
  return { node, text, footer };
}

function chatFooterTimestamp(value) {
  if (!value) return "";
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? String(value) : formatReceivedDateTime(value);
}

function normalizedChatMessageMeta(raw = {}) {
  const source = raw && typeof raw === "object" ? raw : {};
  const meta = {
    ...source,
    cli: firstDefined(source.cli, source.provider, source.chatCli),
    model: firstDefined(source.model, source.chatModel),
    effort: firstDefined(source.effort, source.chatEffort),
    mode: firstDefined(source.mode, source.chatMode),
    thinking: source.thinking ?? source.chatThinking,
    fast: source.fast ?? source.chatFast,
    sentAt: firstDefined(source.sentAt, source.sent_at),
    receivedAt: firstDefined(source.receivedAt, source.received_at),
    tokenUsage: firstDefined(source.tokenUsage, source.token_usage),
    elapsedMs: firstDefined(source.elapsedMs, source.elapsed_ms),
  };
  if (meta.sentAt) meta.sentAt = chatFooterTimestamp(meta.sentAt);
  if (meta.receivedAt) meta.receivedAt = chatFooterTimestamp(meta.receivedAt);
  return normalizeMessageMeta(meta);
}

function applyChatControlFallback(meta, fallback = {}) {
  const next = { ...meta };
  ["cli", "model", "effort", "mode", "thinking", "fast"].forEach((field) => {
    if (next[field] === undefined || next[field] === null || next[field] === "") {
      const value = fallback[field];
      if (value !== undefined && value !== null && value !== "") next[field] = value;
    }
  });
  return next;
}

function chatSessionControls(sessionId = state.chatSessionId, session = null, overrides = null) {
  const persisted = overrides || getSessionOverridesFor(sessionId) || {};
  return normalizedChatMessageMeta({
    cli: firstDefined(session?.provider, session?.cli, persisted.cli, persisted.provider, state.preferences.chatCli, state.preferences.chatProvider, "codex"),
    model: firstDefined(session?.model, persisted.model, state.preferences.chatModel, ""),
    effort: firstDefined(session?.effort, persisted.effort, state.preferences.chatEffort, "medium"),
    mode: firstDefined(session?.mode, persisted.mode, state.preferences.chatMode, "default"),
    thinking: session?.thinking ?? persisted.thinking ?? state.preferences.chatThinking ?? false,
    fast: session?.fast ?? persisted.fast ?? state.preferences.chatFast ?? false,
  });
}

function chatMessageFooterMeta(message, role, controls = {}, previousPrompt = null) {
  let meta = normalizedChatMessageMeta(persistedChatMessageMeta(message));
  meta = applyChatControlFallback(meta, controls);
  const timestamp = chatFooterTimestamp(message?.timestamp || message?.receivedAt || "");
  if (role === "user") {
    if (!meta.sentAt && timestamp) meta.sentAt = timestamp;
  } else {
    if (!meta.receivedAt && timestamp) meta.receivedAt = timestamp;
    if (previousPrompt?.timestamp && message?.timestamp) {
      meta.elapsed = formatElapsed(previousPrompt.timestamp, message.timestamp);
    }
  }
  return meta;
}

function chatResponseCopyText(messages, indices, previousPrompt, session) {
  return indices
    .map((index) => {
      const message = messages[index];
      const content = assistantResponseContent(
        String(message?.content || ""),
        chatMessageProvider(message, persistedChatMessageMeta(message), session),
        previousPrompt?.content || "",
      );
      return withoutChatToolTelemetrySections(content);
    })
    .filter(Boolean)
    .join("\n\n")
    .trim();
}

function chatResponsePresentation(messages, options = {}) {
  const presentation = new Map();
  const session = options.session || findChatSession(options.sessionId || state.chatSessionId) || null;
  const sessionId = options.sessionId || session?.id || state.chatSessionId || "";
  const controls = chatSessionControls(sessionId, session, options.overrides);
  let previousPrompt = null;
  let index = 0;

  while (index < messages.length) {
    const message = messages[index];
    const role = String(message?.role || "").toLowerCase();
    if (role === "user") {
      previousPrompt = message;
      index += 1;
      continue;
    }

    const groupStart = index;
    while (
      index + 1 < messages.length &&
      String(messages[index + 1]?.role || "").toLowerCase() !== "user"
    ) {
      index += 1;
    }
    const assistantIndices = [];
    for (let cursor = groupStart; cursor <= index; cursor += 1) {
      const candidate = messages[cursor];
      if (
        String(candidate?.role || "").toLowerCase() === "assistant" &&
        !isTerminalStatusMessageMeta(persistedChatMessageMeta(candidate)) &&
        assistantResponseContent(
          String(candidate?.content || ""),
          chatMessageProvider(candidate, persistedChatMessageMeta(candidate), session),
          previousPrompt?.content || "",
        )
      ) {
        assistantIndices.push(cursor);
      }
    }
    if (assistantIndices.length) {
      const firstIndex = assistantIndices[0];
      const lastIndex = assistantIndices[assistantIndices.length - 1];
      const first = presentation.get(firstIndex) || {};
      presentation.set(firstIndex, {
        ...first,
        showResponseHeader: true,
        headerMeta: chatMessageFooterMeta(messages[firstIndex], "assistant", controls, previousPrompt),
      });
      const last = presentation.get(lastIndex) || {};
      presentation.set(lastIndex, {
        ...last,
        showResponseFooter: true,
        footerMeta: chatMessageFooterMeta(messages[lastIndex], "assistant", controls, previousPrompt),
        copyText: chatResponseCopyText(messages, assistantIndices, previousPrompt, session),
      });
    }
    index += 1;
  }
  return { controls, presentation };
}

function renderChatResponseHeader(node, meta = {}) {
  if (!node) return;
  const normalized = normalizedChatMessageMeta(meta);
  const provider = String(firstDefined(normalized.cli, normalized.provider, "codex")).toLowerCase();
  const model = String(normalized.model || "").trim();
  let header = node.querySelector(":scope > .chat-response-header");
  if (!header) {
    header = document.createElement("div");
    header.className = "chat-response-header";
    const text = node.querySelector(":scope > .chat-line-text");
    node.insertBefore(header, text || null);
  }
  const signature = `${provider}\u0000${model}`;
  if (header.dataset.signature === signature) return;
  header.dataset.signature = signature;
  const icon = document.createElement("img");
  icon.src = sidebarProviderIcon(provider);
  icon.alt = "";
  icon.setAttribute("aria-hidden", "true");
  const label = document.createElement("span");
  label.textContent = [sidebarProviderLabel(provider), model].filter(Boolean).join(" · ");
  header.replaceChildren(icon, label);
}

function chatLineActions(node, kind) {
  let actions = node?.querySelector(":scope > .chat-line-actions");
  if (actions) return actions;
  actions = document.createElement("div");
  actions.className = `chat-line-actions chat-line-actions-${kind}`;
  const footer = node?.querySelector(":scope > .chat-line-footer");
  if (footer) node.insertBefore(actions, footer);
  else node?.appendChild(actions);
  return actions;
}

function attachChatCopyAction(node, content, kind, label) {
  if (!node || !String(content || "").trim()) return;
  const actions = chatLineActions(node, kind);
  const selector = `[data-chat-copy-kind="${kind}"]`;
  let button = actions.querySelector(selector);
  if (!button) {
    button = document.createElement("button");
    button.type = "button";
    button.className = "chat-message-copy icon-button";
    button.dataset.symbol = "copy";
    button.dataset.chatCopyKind = kind;
    button.setAttribute("aria-label", label);
    button.title = label;
    button.addEventListener("click", async (event) => {
      event.stopPropagation();
      await copyText(button._iowbCopyText || "");
      showToast("Copied", "ok");
    });
    actions.appendChild(button);
  }
  button._iowbCopyText = content;
}

function attachUserChatActions(node, message, options = {}) {
  const actions = chatLineActions(node, "user");
  const messageId = persistedChatMessageId(message);
  if (messageId && options.allowEdit !== false && !actions.querySelector("[data-chat-edit-from-here]")) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "chat-message-edit icon-button";
    button.dataset.symbol = "pencil";
    button.dataset.chatEditFromHere = messageId;
    button.setAttribute("aria-label", "Edit from here");
    button.title = "Edit from here";
    const session = findChatSession(state.chatSessionId);
    if (session?.active || selectedRunningChatSession()) {
      button.disabled = true;
      button.title = "Stop the current response before editing from here";
    }
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      editChatFromHere(messageId, button).catch(showError);
    });
    actions.appendChild(button);
  }
  attachChatCopyAction(node, String(message?.content || ""), "user", "Copy prompt");
}

function attachAssistantChatActions(node, content) {
  attachChatCopyAction(node, content, "assistant", "Copy response");
}

function chatPromptHistoryScope(sessionId = state.chatSessionId) {
  const server = normalizedWebServerUrl();
  const id = (sessionId || "").trim();
  if (id) return `${server}::session::${id}`;
  return `${server}::${activeProjectPath() || "default"}::new`;
}

function chatPromptHistoryStorageKey(scope = state.chatPromptHistoryScope) {
  return `iowb.chatPromptHistory:${scope || chatPromptHistoryScope()}`;
}

function promptHistoryHash(value) {
  let hash = 0;
  const input = String(value || "");
  for (let index = 0; index < input.length; index += 1) {
    hash = ((hash << 5) - hash + input.charCodeAt(index)) | 0;
  }
  return Math.abs(hash).toString(36);
}

function normalizePromptHistoryItem(entry, index = 0) {
  if (typeof entry === "string") {
    const content = entry.trim();
    if (!content) return null;
    return {
      id: `legacy:${index}:${promptHistoryHash(content)}`,
      content,
      timestamp: new Date(0 + index).toISOString(),
      local: true,
    };
  }
  if (!entry || typeof entry !== "object") return null;
  const content = String(entry.content || "").trim();
  if (!content) return null;
  const id = String(entry.id || entry.localId || entry.local_id || "").trim()
    || `local:${index}:${promptHistoryHash(content)}`;
  const rawTimestamp = String(entry.timestamp || entry.createdAt || entry.created_at || "").trim();
  const timestamp = rawTimestamp && !Number.isNaN(new Date(rawTimestamp).getTime())
    ? new Date(rawTimestamp).toISOString()
    : new Date(0 + index).toISOString();
  return {
    id,
    content,
    timestamp,
    local: Boolean(entry.local),
  };
}

function readChatPromptHistoryCache(scope) {
  const raw = readJsonStorage(chatPromptHistoryStorageKey(scope), null);
  let prompts = [];
  let hasOlder = false;
  if (Array.isArray(raw)) {
    prompts = raw;
  } else if (raw && typeof raw === "object") {
    prompts = Array.isArray(raw.prompts) ? raw.prompts : [];
    hasOlder = raw.hasOlder === true || raw.has_more === true;
  } else {
    prompts = readJsonStorage("iowb.chatPromptHistory", []);
  }
  return {
    prompts: prompts
      .map(normalizePromptHistoryItem)
      .filter(Boolean)
      .slice(-MAX_PROMPT_HISTORY),
    hasOlder,
  };
}

function ensureChatPromptHistoryScope(sessionId = state.chatSessionId) {
  const scope = chatPromptHistoryScope(sessionId);
  if (state.chatPromptHistoryScope === scope) return;
  state.chatPromptHistoryLoadScope = "";
  state.chatPromptHistoryPendingPreviousAfterLoad = false;
  state.chatPromptHistoryScope = scope;
  const cached = readChatPromptHistoryCache(scope);
  state.chatPromptHistory = cached.prompts;
  state.chatPromptHistoryHasOlder = cached.hasOlder;
  state.chatPromptHistoryIndex = state.chatPromptHistory.length;
  state.chatPromptHistoryScratch = "";
  loadChatPromptHistoryPage(sessionId, { older: false }).catch((error) => {
    console.warn("Unable to load prompt history", error);
  });
}

function persistChatPromptHistory() {
  ensureChatPromptHistoryScope();
  state.chatPromptHistory = (state.chatPromptHistory || [])
    .map(normalizePromptHistoryItem)
    .filter(Boolean)
    .sort((left, right) => String(left.timestamp).localeCompare(String(right.timestamp)) || String(left.id).localeCompare(String(right.id)))
    .slice(-MAX_PROMPT_HISTORY);
  window.localStorage.setItem(chatPromptHistoryStorageKey(), JSON.stringify({
    prompts: state.chatPromptHistory,
    hasOlder: state.chatPromptHistoryHasOlder === true,
  }));
}

function mergeChatPromptHistory(items, options = {}) {
  ensureChatPromptHistoryScope();
  const normalizedItems = (items || []).map(normalizePromptHistoryItem).filter(Boolean);
  if (!normalizedItems.length) {
    if (typeof options.hasOlder === "boolean") state.chatPromptHistoryHasOlder = options.hasOlder;
    persistChatPromptHistory();
    return;
  }
  const previousSize = state.chatPromptHistory.length;
  const wasAtEnd = state.chatPromptHistoryIndex >= previousSize;
  const selectedId = state.chatPromptHistory[state.chatPromptHistoryIndex]?.id || "";
  const merged = new Map();
  for (const item of state.chatPromptHistory) {
    merged.set(item.id, item);
  }
  for (const item of normalizedItems) {
    if (!item.local) {
      for (const [key, existing] of merged.entries()) {
        if (existing.local && existing.content === item.content) {
          merged.delete(key);
          break;
        }
      }
    }
    merged.set(item.id, item);
  }
  state.chatPromptHistory = Array.from(merged.values())
    .sort((left, right) => String(left.timestamp).localeCompare(String(right.timestamp)) || String(left.id).localeCompare(String(right.id)))
    .slice(-MAX_PROMPT_HISTORY);
  if (typeof options.hasOlder === "boolean") state.chatPromptHistoryHasOlder = options.hasOlder;
  if (selectedId) {
    const selectedIndex = state.chatPromptHistory.findIndex((item) => item.id === selectedId);
    state.chatPromptHistoryIndex = selectedIndex >= 0 ? selectedIndex : state.chatPromptHistory.length;
  } else if (wasAtEnd) {
    state.chatPromptHistoryIndex = state.chatPromptHistory.length;
  } else {
    state.chatPromptHistoryIndex = Math.max(0, Math.min(state.chatPromptHistoryIndex, state.chatPromptHistory.length));
  }
  persistChatPromptHistory();
}

function rememberChatPrompt(prompt) {
  ensureChatPromptHistoryScope();
  const value = String(prompt || "").trim();
  if (!value) return;
  state.chatPromptHistory = (state.chatPromptHistory || []).filter((item) => !(item?.local && item?.content === value));
  state.chatPromptHistory.push({
    id: `local:${Date.now()}:${promptHistoryHash(value)}`,
    content: value,
    timestamp: new Date().toISOString(),
    local: true,
  });
  state.chatPromptHistory = state.chatPromptHistory.slice(-MAX_PROMPT_HISTORY);
  persistChatPromptHistory();
  state.chatPromptHistoryIndex = state.chatPromptHistory.length;
  state.chatPromptHistoryScratch = "";
}

function chatPromptHistoryCursor() {
  return (state.chatPromptHistory || []).find((item) => !item.local && item.timestamp && item.id) || null;
}

async function loadChatPromptHistoryPage(sessionId = state.chatSessionId, options = {}) {
  const id = (sessionId || "").trim();
  ensureChatPromptHistoryScope(id);
  const session = id ? (findChatSession(id) || cachedChatSession(id)?.session) : null;
  if (!id || !session || session.pending || state.pendingChatSessionId === id || !canLoadProtectedData()) return false;
  const scope = chatPromptHistoryScope(id);
  const older = options.older === true;
  let cursor = null;
  if (older) {
    if (!state.chatPromptHistoryHasOlder) return false;
    cursor = chatPromptHistoryCursor();
    if (!cursor) return false;
  }
  if (state.chatPromptHistoryLoadScope === scope) return false;
  state.chatPromptHistoryLoadScope = scope;
  try {
    const params = new URLSearchParams({ limit: String(PROMPT_HISTORY_PAGE_SIZE) });
    if (cursor) {
      params.set("before_timestamp", cursor.timestamp);
      params.set("before_id", cursor.id);
    }
    const body = await api(`/api/sessions/${encodeURIComponent(id)}/prompts?${params.toString()}`);
    if ((body?.session_id || body?.sessionId || id) !== id || state.chatPromptHistoryScope !== scope) return false;
    mergeChatPromptHistory(body?.prompts || [], {
      hasOlder: body?.has_more === true || body?.hasMore === true,
    });
    if (state.chatPromptHistoryPendingPreviousAfterLoad && state.chatPromptHistory.length) {
      state.chatPromptHistoryPendingPreviousAfterLoad = false;
      navigateChatPromptHistory(-1);
    } else if (!state.chatPromptHistory.length) {
      state.chatPromptHistoryPendingPreviousAfterLoad = false;
    }
    return true;
  } catch (error) {
    state.chatPromptHistoryPendingPreviousAfterLoad = false;
    console.warn("Unable to load prompt history", error);
    return false;
  } finally {
    if (state.chatPromptHistoryLoadScope === scope) {
      state.chatPromptHistoryLoadScope = "";
    }
  }
}

async function loadChatPromptHistory(sessionId) {
  return loadChatPromptHistoryPage(sessionId, { older: false });
}

function maybePrefetchOlderChatPromptHistory() {
  if (state.chatPromptHistoryIndex <= PROMPT_HISTORY_PREFETCH_REMAINING) {
    loadChatPromptHistoryPage(state.chatSessionId, { older: true }).catch((error) => {
      console.warn("Unable to prefetch prompt history", error);
    });
  }
}

function syncChatPromptHistoryFromMessages(messages, sessionId = state.chatSessionId) {
  ensureChatPromptHistoryScope(sessionId);
  const items = (messages || [])
    .map((message, index) => {
      if (String(message?.role || "").toLowerCase() !== "user") return null;
      const content = String(message?.content || "").trim();
      if (!content) return null;
      return {
        id: String(message.id || message.localId || message.local_id || `transcript:${index}:${promptHistoryHash(content)}`),
        content,
        timestamp: message.timestamp || new Date(0 + index).toISOString(),
        local: !message.id,
      };
    })
    .filter(Boolean);
  mergeChatPromptHistory(items);
}

function navigateChatPromptHistory(direction) {
  ensureChatPromptHistoryScope();
  const history = state.chatPromptHistory || [];
  const prompt = qs("#chat-prompt");
  if (!prompt) return;
  if (!history.length) {
    if (direction < 0) {
      state.chatPromptHistoryPendingPreviousAfterLoad = true;
      loadChatPromptHistoryPage(state.chatSessionId, { older: false }).catch((error) => {
        console.warn("Unable to load prompt history", error);
      });
    }
    return;
  }
  if (state.chatPromptHistoryIndex < 0 || state.chatPromptHistoryIndex > history.length) {
    state.chatPromptHistoryIndex = history.length;
  }
  if (direction < 0) {
    if (state.chatPromptHistoryIndex >= history.length) {
      state.chatPromptHistoryScratch = prompt.value || "";
    }
    if (state.chatPromptHistoryIndex <= 0) {
      if (state.chatPromptHistoryHasOlder) {
        state.chatPromptHistoryPendingPreviousAfterLoad = true;
        loadChatPromptHistoryPage(state.chatSessionId, { older: true }).catch((error) => {
          console.warn("Unable to load older prompt history", error);
        });
      }
      return;
    }
    state.chatPromptHistoryIndex = Math.max(0, state.chatPromptHistoryIndex - 1);
    setChatPromptValue(history[state.chatPromptHistoryIndex]?.content || "");
    noteChatPromptUserEdit();
    maybePrefetchOlderChatPromptHistory();
  } else {
    if (state.chatPromptHistoryIndex < history.length - 1) {
      state.chatPromptHistoryIndex += 1;
      setChatPromptValue(history[state.chatPromptHistoryIndex]?.content || "");
      noteChatPromptUserEdit();
    } else {
      state.chatPromptHistoryIndex = history.length;
      setChatPromptValue(state.chatPromptHistoryScratch);
      noteChatPromptUserEdit();
    }
  }
  scheduleChatPromptDraftSave();
}

function chatEditFromHereItems() {
  const items = [];
  const seen = new Set();
  const add = (entry) => {
    const id = persistedChatMessageId(entry);
    const content = String(entry?.content || "").trim();
    if (!id || !content || seen.has(id)) return;
    seen.add(id);
    items.push({ id, content, timestamp: entry?.timestamp || "" });
  };
  (state.chatPromptHistory || []).forEach((entry) => {
    if (!entry?.local) add(entry);
  });
  if (chatHistoryWindow.sessionId === state.chatSessionId) {
    (chatHistoryWindow.messages || []).forEach((message) => {
      if (String(message?.role || "").toLowerCase() === "user") add(message);
    });
  }
  return items.sort((left, right) => {
    const leftTime = new Date(left.timestamp || 0).getTime() || 0;
    const rightTime = new Date(right.timestamp || 0).getTime() || 0;
    return leftTime - rightTime;
  });
}

function closeChatEditFromHerePicker() {
  state.chatEditFromHere.pickerOpen = false;
  state.chatEditFromHere.items = [];
  state.chatEditFromHere.index = -1;
  state.chatEditFromHere.armedUntil = 0;
  qs("#chat-edit-from-here-picker")?.remove();
}

function renderChatEditFromHerePicker() {
  qs("#chat-edit-from-here-picker")?.remove();
  if (!state.chatEditFromHere.pickerOpen) return;
  const selected = state.chatEditFromHere.items[state.chatEditFromHere.index];
  if (!selected) return;
  const root = document.createElement("section");
  root.id = "chat-edit-from-here-picker";
  root.className = "chat-edit-from-here-picker";
  root.setAttribute("role", "dialog");
  root.setAttribute("aria-modal", "false");
  root.setAttribute("aria-label", "Choose a prompt to edit from here");
  root.innerHTML = `
    <div class="chat-edit-from-here-picker-header">
      <div class="chat-edit-from-here-picker-title">Edit from here</div>
      <div class="chat-edit-from-here-picker-count">${state.chatEditFromHere.index + 1} of ${state.chatEditFromHere.items.length}</div>
    </div>
    <div class="chat-edit-from-here-picker-preview">${escapeHtml(selected.content)}</div>
    <div class="chat-edit-from-here-picker-hint">↑/↓ choose · Enter confirm · Esc cancel</div>
  `;
  document.body.appendChild(root);
}

function openChatEditFromHerePicker() {
  const items = chatEditFromHereItems();
  if (!items.length) {
    showToast("No persisted prompts are available", "danger");
    state.chatEditFromHere.armedUntil = 0;
    return false;
  }
  state.chatEditFromHere.items = items;
  state.chatEditFromHere.index = items.length - 1;
  state.chatEditFromHere.pickerOpen = true;
  state.chatEditFromHere.armedUntil = 0;
  renderChatEditFromHerePicker();
  return true;
}

function moveChatEditFromHerePicker(direction) {
  const { items } = state.chatEditFromHere;
  if (!items.length) return;
  state.chatEditFromHere.index = Math.max(
    0,
    Math.min(items.length - 1, state.chatEditFromHere.index + direction),
  );
  renderChatEditFromHerePicker();
}

function chatEditFromHereRequestId() {
  if (window.crypto?.randomUUID) return window.crypto.randomUUID();
  return `web-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;
}

function stagedChatEdit() {
  const staged = state.chatEditFromHere.staged;
  if (!staged || staged.sourceSessionId !== state.chatSessionId) return null;
  return staged;
}

function renderStagedChatEdit(staged) {
  chatHistoryWindow = {
    sessionId: staged.sourceSessionId,
    offset: 0,
    totalCount: staged.prefixMessages.length,
    messages: staged.prefixMessages.slice(),
  };
  resetChatOutputDom();
  replayChatMessages(staged.prefixMessages);
  state.chatImages = [];
  renderChatImages();
  setChatPromptValue(staged.draftContent);
  noteChatPromptUserEdit(staged.draftContent);
  qs("#chat-prompt")?.focus();
}

function cancelStagedChatEdit() {
  if (state.chatEditFromHere.submitting) {
    showToast("Wait for the replacement chat to finish creating", "ok");
    return false;
  }
  const staged = stagedChatEdit();
  if (!staged) {
    state.chatEditFromHere.staged = null;
    state.chatEditFromHere.submitting = false;
    return false;
  }
  state.chatEditFromHere.staged = null;
  state.chatEditFromHere.submitting = false;
  chatHistoryWindow = {
    ...staged.sourceHistory,
    messages: staged.sourceHistory.messages.slice(),
  };
  resetChatOutputDom();
  replayChatMessages(chatHistoryWindow.messages);
  state.chatImages = staged.sourceImages.slice();
  renderChatImages();
  setChatPromptValue(staged.originalDraft);
  noteChatPromptUserEdit(staged.originalDraft);
  ensureChatPromptHistoryScope(staged.sourceSessionId);
  showToast("Edit from here cancelled", "ok");
  return true;
}

async function loadCompleteChatHistoryForEdit(sourceSessionId) {
  if (chatHistoryWindow.sessionId !== sourceSessionId) {
    const loaded = await loadChatHistoryForSession(sourceSessionId);
    if (!loaded) throw new Error("Could not load this chat before editing it.");
  }
  while (chatHistoryWindow.offset > 0) {
    const previousOffset = chatHistoryWindow.offset;
    const loaded = await loadChatHistoryForSession(sourceSessionId, { older: true });
    if (!loaded || chatHistoryWindow.offset >= previousOffset) {
      throw new Error("Could not load the earlier chat history needed for this edit.");
    }
  }
  return {
    ...chatHistoryWindow,
    messages: chatHistoryWindow.messages.slice(),
  };
}

async function editChatFromHere(messageId, button = null) {
  if (state.chatEditFromHere.submitting) return;
  const sourceSessionId = String(state.chatSessionId || "").trim();
  if (!sourceSessionId) throw new Error("Open a persisted chat session first.");
  if (selectedRunningChatSession() || findChatSession(sourceSessionId)?.active) {
    throw new Error("Stop the current response before editing from here.");
  }
  const persistedMessageId = String(messageId || "").trim();
  if (!persistedMessageId) throw new Error("Select a persisted user prompt first.");
  await withButtonLoading(button, async () => {
    const existing = stagedChatEdit();
    let sourceHistory;
    let originalDraft;
    let requestId;
    if (existing) {
      sourceHistory = existing.sourceHistory;
      originalDraft = existing.originalDraft;
      requestId = existing.beforeMessageId === persistedMessageId
        ? existing.requestId
        : chatEditFromHereRequestId();
    } else {
      await saveChatPromptDraftNow().catch((error) => {
        console.warn("Unable to sync the current draft before editing from here", error);
      });
      sourceHistory = await loadCompleteChatHistoryForEdit(sourceSessionId);
      originalDraft = qs("#chat-prompt")?.value || "";
      requestId = chatEditFromHereRequestId();
    }
    const targetIndex = sourceHistory.messages.findIndex(
      (message) => persistedChatMessageId(message) === persistedMessageId,
    );
    const target = sourceHistory.messages[targetIndex];
    if (targetIndex < 0 || String(target?.role || "").toLowerCase() !== "user") {
      throw new Error("The selected persisted prompt was not found in this chat.");
    }
    const staged = {
      sourceSessionId,
      beforeMessageId: persistedMessageId,
      requestId,
      projectPath: findChatSession(sourceSessionId)?.projectPath || activeProjectPath(),
      originalDraft,
      sourceImages: existing?.sourceImages || state.chatImages.slice(),
      draftContent: String(target.content || ""),
      prefixMessages: sourceHistory.messages.slice(0, targetIndex),
      sourceHistory,
    };
    state.chatEditFromHere.staged = staged;
    state.chatEditFromHere.submitting = false;
    closeChatEditFromHerePicker();
    renderStagedChatEdit(staged);
    showToast("Edit the prompt, then send to replace this branch", "ok");
  });
}

function handleChatEditFromHereKeydown(event) {
  if (activeView() !== "chat") return false;
  if (state.folderBrowser.open || state.commandPalette.open || qs("#chat-session-config-modal")) {
    return false;
  }
  if (state.chatEditFromHere.pickerOpen) {
    if (event.key === "ArrowUp") {
      event.preventDefault();
      event.stopPropagation();
      moveChatEditFromHerePicker(-1);
      return true;
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      event.stopPropagation();
      moveChatEditFromHerePicker(1);
      return true;
    }
    if (event.key === "Enter" && !event.isComposing) {
      event.preventDefault();
      event.stopPropagation();
      const selected = state.chatEditFromHere.items[state.chatEditFromHere.index];
      if (selected) editChatFromHere(selected.id).catch(showError);
      return true;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeChatEditFromHerePicker();
      return true;
    }
    return false;
  }
  if (event.key === "Escape" && stagedChatEdit()) {
    event.preventDefault();
    event.stopPropagation();
    cancelStagedChatEdit();
    return true;
  }
  if (event.key !== "Escape") {
    state.chatEditFromHere.armedUntil = 0;
    return false;
  }
  const selectedRun = selectedRunningChatSession();
  if (selectedRun) {
    event.preventDefault();
    event.stopPropagation();
    requestAbortSelectedChatSession();
    return true;
  }
  const prompt = qs("#chat-prompt");
  if (!prompt || prompt.value.trim() || event.repeat || !currentChatDraftSessionId()) {
    state.chatEditFromHere.armedUntil = 0;
    return false;
  }
  event.preventDefault();
  event.stopPropagation();
  const now = Date.now();
  if (now <= state.chatEditFromHere.armedUntil) {
    openChatEditFromHerePicker();
  } else {
    state.chatEditFromHere.armedUntil = now + 1600;
    showToast("Press Esc again to edit an earlier prompt", "ok");
  }
  return true;
}

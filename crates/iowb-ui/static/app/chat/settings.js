function renderSessions() {
  const list = qs("#sessions-list");
  if (!list) {
    renderSidebarProjects();
    return;
  }
  const filter = qs("#sessions-filter")?.value.trim().toLowerCase() || "";
  const sessions = state.sessions.filter((session) => {
    if (isBoardChatSession(session)) return false;
    const haystack = [
      session.id,
      session.title,
      session.provider,
      session.projectPath,
      session.status,
    ].join(" ").toLowerCase();
    return !filter || haystack.includes(filter);
  });
  if (!sessions.length) {
    list.innerHTML = '<p class="empty">No active sessions.</p>';
    renderSidebarSessions();
    return;
  }

  renderVirtualList(list, "sessions", sessions, {
    rowHeight: 104,
    render: (session) => {
      const displayTokens = formatSessionDisplayTokenUsage(session);
      return `<article class="row">
      <strong>${escapeHtml(session.title || session.id)}</strong>
      <span>${escapeHtml(session.provider)} · ${escapeHtml(session.projectPath)}</span>
      <span class="meta">${escapeHtml([
        session.external ? "External CLI history" : `${session.messageCount} messages`,
        displayTokens,
      ].filter(Boolean).join(" · "))}</span>
      <div class="row-actions">
        <button type="button" data-session-use="${escapeHtml(session.id)}">Use</button>
        <button type="button" data-session-open="${escapeHtml(session.id)}">Messages</button>
      </div>
    </article>`;
    },
    bind: (root) => {
      root.querySelectorAll("[data-session-use]").forEach((button) => {
        button.addEventListener("click", () => {
          qs("#session-id-input").value = button.dataset.sessionUse;
        });
      });
      root.querySelectorAll("[data-session-open]").forEach((button) => {
        button.addEventListener("click", () => {
          qs("#session-id-input").value = button.dataset.sessionOpen;
          loadSessionMessages().catch(showError);
        });
      });
    },
  });
  renderSidebarSessions();
}

function resetVirtualList(key) {
  if (state.virtualLists[key]) {
    state.virtualLists[key].scrollTop = 0;
  }
}

function renderSettings() {
  setSettingsTab(state.activeSettingsTab);
  renderSettingsServerStatus(state.settings);
  renderSettingsResponse(state.settings);
}

function setSettingsTab(tab) {
  const next = tab || "agents";
  state.activeSettingsTab = next;
  window.localStorage.setItem("iowb.settingsTab", next);
  document.querySelectorAll("[data-settings-tab]").forEach((button) => {
    const active = button.dataset.settingsTab === next;
    button.classList.toggle("active", active);
    button.setAttribute("aria-selected", active ? "true" : "false");
  });
  document.querySelectorAll("[data-settings-panel]").forEach((panel) => {
    panel.classList.toggle("active", panel.dataset.settingsPanel === next);
  });
  if (next === "direct-ai" && canLoadProtectedData() && !state.ioGatewayStatus) {
    loadIoGatewayConfig().catch(showError);
  }
}

function renderSettingsServerStatus(body) {
  const target = qs("#settings-server-status");
  if (!target) return;
  if (!body || typeof body !== "object") {
    target.innerHTML = '<p class="empty">Server status unavailable.</p>';
    return;
  }
  const uptime = firstDefined(body.uptime, body.uptimeSeconds, body.uptime_seconds, body.runtime?.uptimeSeconds, "");
  const configDir = firstDefined(body.configDir, body.config_dir, body.paths?.configDir, "");
  const serverState = firstDefined(body.status, body.state, body.service, "Online");
  const version = firstDefined(body.version, body.build?.version, "n/a");
  target.innerHTML = [
    metricCard(serverState, "Server"),
    metricCard(version, "Version"),
    metricCard(state.ws?.readyState === WebSocket.OPEN ? "Connected" : "Disconnected", "WebSocket"),
    metricCard(uptime || configDir || "Ready", uptime ? "Uptime" : configDir ? "Config Dir" : "Runtime"),
  ].join("");
}

function renderMetrics() {
  const grid = qs("#metrics-grid");
  if (!grid) return;
  const metrics = state.metrics?.metrics || {};
  grid.innerHTML = [
    metricCard(metrics.projects?.count ?? 0, "Projects"),
    metricCard(metrics.sessions?.active ?? 0, "Active Sessions"),
    metricCard(metrics.processes?.active ?? 0, "Processes"),
    metricCard(metrics.memory?.rssKb ? `${metrics.memory.rssKb} KB` : "n/a", "RSS Memory"),
  ].join("");
  renderJson("#metrics-json", state.metrics);
}

function metricCard(value, label) {
  return `<article class="metric"><strong>${escapeHtml(value)}</strong><span>${escapeHtml(label)}</span></article>`;
}

function setOutput(selector, value, className = "") {
  const target = qs(selector);
  if (!target) return;
  target.className = className ? `output-panel ${className}` : "output-panel";
  target.textContent = value;
}

function renderJson(selector, value) {
  setOutput(selector, JSON.stringify(value, null, 2), "json-output");
}

function matchesText(value, query) {
  if (!query) return true;
  return String(value || "").toLowerCase().includes(query.toLowerCase());
}

function firstDefined(...values) {
  return values.find((value) => value !== undefined && value !== null && value !== "");
}

function formatDate(value) {
  if (!value) return "";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
}

function formatShortDate(value) {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return String(value);
  return date.toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function resultBadge(success) {
  return `<span class="badge ${success ? "ok" : "danger"}">${success ? "ok" : "error"}</span>`;
}

function operationMessage(body) {
  return firstDefined(body.message, body.output, body.error, body.details, JSON.stringify(body));
}

function bindCopyButtons(root = document) {
  root.querySelectorAll("[data-copy-text]").forEach((button) => {
    button.addEventListener("click", async () => {
      const value = button.dataset.copyText || "";
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(value);
      }
      button.textContent = "Copied";
      window.setTimeout(() => {
        button.textContent = button.dataset.copyLabel || "Copy";
      }, 900);
    });
    button.dataset.copyLabel = button.textContent;
  });
}

async function copyText(value) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value || "");
    return;
  }
  const area = document.createElement("textarea");
  area.value = value || "";
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.select();
  document.execCommand("copy");
  area.remove();
}

function savePreferences() {
  window.localStorage.setItem("iowb.webPreferences", JSON.stringify(state.preferences));
}

function terminalSizeFromSettings() {
  return {
    cols: numericInputValue("#shell-cols", state.preferences.shellCols || 100, 20, 300),
    rows: numericInputValue("#shell-rows", state.preferences.shellRows || 30, 8, 120),
  };
}

function saveTerminalSizePreference() {
  const size = terminalSizeFromSettings();
  state.preferences.shellCols = size.cols;
  state.preferences.shellRows = size.rows;
  savePreferences();
}

function applyTerminalSizeToInputs() {
  const cols = Math.min(300, Math.max(20, Number(state.preferences.shellCols) || 100));
  const rows = Math.min(120, Math.max(8, Number(state.preferences.shellRows) || 30));
  if (qs("#shell-cols")) qs("#shell-cols").value = String(cols);
  if (qs("#shell-rows")) qs("#shell-rows").value = String(rows);
}

function applyChatProviderPreference() {
  const provider = CHAT_PROVIDERS.has(state.preferences.chatProvider)
    ? state.preferences.chatProvider
    : "codex";
  state.preferences.chatProvider = provider;
  state.preferences.chatCli = provider;
  // The legacy settings-panel chat-provider dropdown may still exist; keep
  // it in sync if present.
  const settingSelect = qs("#chat-provider-setting");
  if (settingSelect) settingSelect.value = provider;
  renderChatProviderPicker();
}

async function applyTerminalSizePreference(syncServer = false) {
  const size = terminalSizeFromSettings();
  state.shellTerm?.resize(size.cols, size.rows);
  if (syncServer && state.currentShellProcess) {
    await resizeCurrentShell();
  }
}

// Chat override controls (Model / Mode / Effort) live directly above the
// prompt input. The CLI and Thinking toggles were removed from the
// composer — they're driven by the stored preferences (and the sidebar
// provider buttons). Per-session overrides are persisted to
// iowb.webPreferences.chatSessionOverrides so the chat "remembers" what
// the user picked across refreshes.
const CHAT_PROVIDERS_LOCAL = ["codex", "claude", "gemini"];

function isPlainRecord(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function normalizeChatMode(value) {
  const normalized = String(value ?? "").trim().toLowerCase();
  const aliases = {
    accept: "accept-edits",
    acceptedits: "accept-edits",
    "plan-only": "plan",
    "bypass-permissions": "bypass",
    bypasspermissions: "bypass",
  };
  const resolved = aliases[normalized] || normalized;
  return CHAT_MODES.has(resolved) ? resolved : "default";
}

function normalizeChatEffort(value) {
  const normalized = String(value ?? "")
    .trim()
    .toLowerCase()
    .replace(/^x[-_ ]high$/, "xhigh");
  return CHAT_EFFORTS.has(normalized) ? normalized : "medium";
}

function normalizeChatThinking(value, fallback = false) {
  if (typeof value === "boolean") return value;
  if (value === 1) return true;
  if (value === 0) return false;
  const normalized = String(value ?? "").trim().toLowerCase();
  if (["true", "yes", "on", "1"].includes(normalized)) return true;
  if (["false", "no", "off", "0", ""].includes(normalized)) return false;
  return fallback;
}

function normalizeChatFast(value, fallback = false) {
  if (typeof value === "boolean") return value;
  if (value === 1) return true;
  if (value === 0) return false;
  const normalized = String(value ?? "").trim().toLowerCase();
  if (["true", "yes", "on", "1", "fast", "priority"].includes(normalized)) return true;
  if (["false", "no", "off", "0", "", "default", "standard"].includes(normalized)) return false;
  return fallback;
}

function ownSessionControl(raw, names) {
  if (!isPlainRecord(raw)) return { found: false, value: undefined };
  for (const name of names) {
    if (Object.prototype.hasOwnProperty.call(raw, name) && raw[name] !== null) {
      return { found: true, value: raw[name] };
    }
  }
  return { found: false, value: undefined };
}

function normalizeSessionControls(raw, local = false) {
  const controls = {};
  const provider = ownSessionControl(raw, local
    ? ["cli", "provider", "chatProvider"]
    : ["provider", "cli"]);
  const model = ownSessionControl(raw, ["model", "chatModel"]);
  const effort = ownSessionControl(raw, ["effort", "reasoningEffort", "reasoning_effort"]);
  const mode = ownSessionControl(raw, ["mode", "permissionMode", "permission_mode"]);
  const thinking = ownSessionControl(raw, ["thinking", "chatThinking"]);
  const fast = ownSessionControl(raw, ["fast", "fastMode", "fast_mode"]);
  if (provider.found) {
    const value = String(provider.value ?? "").trim().toLowerCase();
    controls.cli = CHAT_PROVIDERS_LOCAL.includes(value) ? value : "codex";
  }
  if (model.found) controls.model = String(model.value ?? "").trim();
  if (effort.found) controls.effort = normalizeChatEffort(effort.value);
  if (mode.found) controls.mode = normalizeChatMode(mode.value);
  if (thinking.found) controls.thinking = normalizeChatThinking(thinking.value);
  if (fast.found) controls.fast = normalizeChatFast(fast.value);
  return controls;
}

function normalizeSessionOverrideEntry(entry) {
  if (!isPlainRecord(entry)) return null;
  return { ...entry, ...normalizeSessionControls(entry, true) };
}

function chatCliValue() {
  const v = state.preferences.chatCli || state.preferences.chatProvider;
  return CHAT_PROVIDERS_LOCAL.includes(v) ? v : "codex";
}

function isGatewayModelValue(value) {
  return /^[a-z][a-z0-9_-]{1,12}:/i.test(String(value || "").trim());
}

function runtimeProviderForModel(provider, model) {
  return provider === "codex" && isGatewayModelValue(model) ? "codex" : provider;
}

function shouldFetchTokenUsage(provider, model) {
  return provider === "codex" || !isGatewayModelValue(model);
}

function chatModelValue() {
  const select = qs("#chat-model");
  if (select) return String(select.value ?? "").trim();
  return String(state.preferences.chatModel ?? "").trim();
}

function chatModeValue() {
  return normalizeChatMode(qs("#chat-mode")?.value ?? state.preferences.chatMode);
}

function chatEffortValue() {
  return normalizeChatEffort(qs("#chat-effort")?.value ?? state.preferences.chatEffort);
}

function chatThinkingValue() {
  return normalizeChatThinking(state.preferences.chatThinking);
}

function chatFastValue() {
  return chatCliValue() === "codex" && normalizeChatFast(state.preferences.chatFast);
}

function setChatFastRequested(requested) {
  const next = chatCliValue() === "codex" && normalizeChatFast(requested);
  state.preferences.chatFast = next;
  savePreferences();
  const sid = state.chatSessionId || state.pendingChatSessionId || state.preferences.lastChatSessionId;
  if (sid) saveSessionOverrides(sid, { fast: next });
  updatePendingChatProvider(chatCliValue());
  updateChatComposerState();
}

function updateChatFastControl() {
  const isCodex = chatCliValue() === "codex";
  const requested = isCodex && normalizeChatFast(state.preferences.chatFast);
  const label = !isCodex
    ? "Fast priority requests are available only for Codex"
    : requested
      ? "Disable Fast priority request"
      : "Enable Fast priority request";
  document.querySelectorAll("#chat-fast-toggle, [data-chat-fast-setting]").forEach((toggle) => {
    toggle.checked = requested;
    toggle.disabled = !isCodex;
    toggle.setAttribute("aria-label", label);
    const control = toggle.closest(".chat-fast-control, .chat-session-config-fast");
    control?.classList.toggle("active", requested);
    control?.setAttribute("title", `${label}; the service decides the tier actually used`);
  });
  const statusText = isCodex ? (requested ? "Requested" : "Off") : "Codex only";
  const inlineStatus = qs("#chat-fast-status");
  if (inlineStatus) inlineStatus.textContent = statusText;
  document.querySelectorAll("[data-chat-fast-setting-status]").forEach((status) => {
    status.textContent = statusText;
  });
}

async function loadChatModelsIntoSelect(provider) {
  const select = qs("#chat-model");
  if (!select) return;
  if (!canLoadProtectedData()) {
    select.disabled = true;
    select.innerHTML = `<option value="">Sign in to load models</option>`;
    return;
  }
  const targetProvider = CHAT_PROVIDERS_LOCAL.includes(provider) ? provider : "codex";
  select.dataset.modelProvider = targetProvider;
  select.disabled = true;
  try {
    const body = await api(`/api/chat/models?provider=${encodeURIComponent(targetProvider)}`);
    if (select.dataset.modelProvider !== targetProvider) return;
    const list = Array.isArray(body.models) ? body.models : [];
    // Normalize each entry to {value,label}. The server may return either a
    // plain string or an object {value,label} depending on which catalog
    // contributed the row (CLI, curated fallback, or AI proxy).
    const entries = list
      .map((entry) => {
        if (entry === null || entry === undefined) return null;
        if (typeof entry === "string") {
          return { value: entry, label: entry || "CLI default" };
        }
        if (typeof entry === "object") {
          const value = entry.value ?? entry.id ?? entry.name;
          if (value === null || value === undefined) return null;
          const normalizedValue = String(value);
          return {
            value: normalizedValue,
            label: String(entry.label ?? (normalizedValue || "CLI default")),
          };
        }
        return null;
      })
      .filter(Boolean);
    const current = select.value;
    select.innerHTML = entries.length
      ? entries.map((m) => `<option value="${escapeHtml(m.value)}">${escapeHtml(m.label)}</option>`).join("")
      : `<option value="">No models available</option>`;
    const values = entries.map((m) => m.value);
    const preferred = String(state.preferences.chatModel ?? "").trim();
    if (values.includes(preferred)) select.value = preferred;
    else if (values.includes(current)) select.value = current;
    else select.value = "";
    state.preferences.chatModel = select.value;
    savePreferences();
  } catch (error) {
    console.warn("[io-workbench] could not load chat models", error);
    if (select.dataset.modelProvider === targetProvider) {
      select.innerHTML = `<option value="">CLI default</option>`;
      select.value = "";
    }
  } finally {
    if (select.dataset.modelProvider === targetProvider) select.disabled = false;
  }
}

function readSessionOverrides() {
  const raw = state.preferences?.chatSessionOverrides;
  if (!isPlainRecord(raw)) {
    state.preferences.chatSessionOverrides = {};
    savePreferences();
    return {};
  }
  const normalized = {};
  let changed = false;
  for (const [sessionId, entry] of Object.entries(raw)) {
    const next = normalizeSessionOverrideEntry(entry);
    if (!next) {
      changed = true;
      continue;
    }
    normalized[sessionId] = next;
    if (JSON.stringify(next) !== JSON.stringify(entry)) changed = true;
  }
  if (changed) {
    state.preferences.chatSessionOverrides = normalized;
    savePreferences();
  }
  return normalized;
}

function writeSessionOverrides(next) {
  if (!state.preferences) state.preferences = {};
  state.preferences.chatSessionOverrides = next || {};
  savePreferences();
}

function getSessionOverridesFor(sessionId) {
  return sessionId ? readSessionOverrides()[sessionId] || null : null;
}

function saveSessionOverrides(sessionId, patch) {
  if (!sessionId) return;
  const all = readSessionOverrides();
  all[sessionId] = normalizeSessionOverrideEntry(Object.assign({}, all[sessionId] || {}, patch)) || {};
  writeSessionOverrides(all);
}

function loadSessionOverridesIntoState(sessionId, session = null) {
  const globalControls = normalizeSessionControls({
    cli: state.preferences.chatCli || state.preferences.chatProvider,
    model: state.preferences.chatModel,
    effort: state.preferences.chatEffort,
    mode: state.preferences.chatMode,
    thinking: state.preferences.chatThinking,
    fast: state.preferences.chatFast,
  }, true);
  const localControls = normalizeSessionControls(getSessionOverridesFor(sessionId), true);
  const sessionControls = normalizeSessionControls(session, false);
  const next = {
    cli: "codex",
    model: "",
    effort: "medium",
    mode: "default",
    thinking: false,
    fast: false,
    ...globalControls,
    ...localControls,
    ...sessionControls,
  };
  state.preferences.chatCli = next.cli;
  state.preferences.chatProvider = next.cli;
  state.preferences.chatModel = next.model;
  state.preferences.chatEffort = next.effort;
  state.preferences.chatMode = next.mode;
  state.preferences.chatThinking = next.thinking;
  state.preferences.chatFast = next.fast;
  const effortSelect = qs("#chat-effort");
  if (effortSelect) effortSelect.value = next.effort;
  const modeSelect = qs("#chat-mode");
  if (modeSelect) modeSelect.value = next.mode;
  const modelSelect = qs("#chat-model");
  if (modelSelect) {
    const hasModel = [...modelSelect.options].some((option) => option.value === next.model);
    if (!hasModel) {
      const option = document.createElement("option");
      option.value = next.model;
      option.textContent = next.model || "CLI default";
      modelSelect.replaceChildren(option);
    }
    modelSelect.value = next.model;
  }
  if (sessionId && Object.keys(sessionControls).length) {
    saveSessionOverrides(sessionId, sessionControls);
  } else {
    savePreferences();
  }
  renderChatProviderPicker();
  if (!qs("#prompt-config-panel")?.classList.contains("hidden")) {
    loadChatModelsIntoSelect(next.cli).catch(() => {});
  }
  updateChatComposerState();
}

function savePreferencesToLocal() {
  if (!state.preferences) state.preferences = {};
  savePreferences();
}

function renderChatFooter(meta) {
  const root = qs("#chat-footer");
  if (!root) return;
  if (!meta || typeof meta !== "object") {
    root.classList.add("hidden");
    root.innerHTML = "";
    return;
  }
  const items = [];
  if (meta.cli) items.push(`<span class="meta">Cli: <strong>${escapeHtml(meta.cli)}</strong></span>`);
  if (meta.model) items.push(`<span class="meta">Model: <strong>${escapeHtml(meta.model)}</strong></span>`);
  if (meta.mode) items.push(`<span class="meta">Mode: <strong>${escapeHtml(meta.mode)}</strong></span>`);
  if (meta.effort) items.push(`<span class="meta">Effort: <strong>${escapeHtml(meta.effort)}</strong></span>`);
  if (meta.receivedAt) items.push(`<span class="meta">Received: <strong>${escapeHtml(meta.receivedAt)}</strong></span>`);
  if (meta.elapsed) items.push(`<span class="meta">Elapsed: <strong>${escapeHtml(meta.elapsed)}</strong></span>`);
  if (items.length) {
    root.classList.remove("hidden");
    root.innerHTML = items.join("");
  } else {
    root.classList.add("hidden");
    root.innerHTML = "";
  }
}

function applyPreferences() {
  state.preferences.chatModel = String(state.preferences.chatModel ?? "").trim();
  state.preferences.chatEffort = normalizeChatEffort(state.preferences.chatEffort);
  state.preferences.chatMode = normalizeChatMode(state.preferences.chatMode);
  state.preferences.chatThinking = normalizeChatThinking(state.preferences.chatThinking);
  state.preferences.chatFast = normalizeChatFast(state.preferences.chatFast);
  savePreferences();
  document.body.classList.toggle("compact", !!state.preferences.compact);
  document.body.classList.toggle("wrap-output", !!state.preferences.wrapOutput);
  qs("#pref-compact").checked = !!state.preferences.compact;
  qs("#pref-wrap").checked = !!state.preferences.wrapOutput;
  applyTerminalSizeToInputs();
  applyChatProviderPreference();
  // Populate the chat-controls select widgets from current preferences.
  if (qs("#chat-effort")) qs("#chat-effort").value = state.preferences.chatEffort;
  if (qs("#chat-mode")) qs("#chat-mode").value = state.preferences.chatMode;
  updateChatFastControl();
  const modelSelect = qs("#chat-model");
  if (modelSelect && !modelSelect.options.length) {
    modelSelect.disabled = true;
    modelSelect.innerHTML = canLoadProtectedData()
      ? `<option value="">Open prompt config to load models</option>`
      : `<option value="">Sign in to load models</option>`;
  }
  if (state.codeEditor) {
    state.codeEditor.setOption("lineWrapping", !!state.preferences.wrapOutput);
  }
}

function filteredItems(items, query, fields) {
  const needle = query.trim().toLowerCase();
  if (!needle) return items;
  return items.filter((item) => fields.map((field) => {
    const value = typeof field === "function" ? field(item) : item[field];
    return String(value || "");
  }).join(" ").toLowerCase().includes(needle));
}

function showMoreButton(key, total, renderer) {
  if (state.limits[key] >= total) return "";
  return `<button class="show-more" type="button" data-show-more="${key}" data-renderer="${renderer}">
    Show ${Math.min(100, total - state.limits[key])} more of ${total}
  </button>`;
}

function bindShowMore(root = document) {
  root.querySelectorAll("[data-show-more]").forEach((button) => {
    button.addEventListener("click", () => {
      state.limits[button.dataset.showMore] += 100;
      ({
        files: () => renderFileEntries(),
        sessions: () => renderSessions(),
        sessionMessages: () => renderSessionMessages(),
        gitFiles: () => renderGitFiles(),
        dbConnections: () => renderDbConnections(),
        toolRuns: () => state.lastToolRuns && renderToolRuns("#tools-output", state.lastToolRuns),
        settingsRows: () => renderSettingsRows(),
      })[button.dataset.renderer]?.();
    });
  });
}

function renderVirtualList(target, key, items, options) {
  const rowHeight = options.rowHeight || 72;
  const overscan = options.overscan || 8;
  const minRows = options.minRows || 8;
  const visibleRows = options.fillViewport
    ? Math.max(minRows, Math.min(items.length || minRows, options.maxRows || items.length || minRows))
    : Math.min(items.length, minRows);
  const viewportHeight = Math.min(options.maxHeight || 520, Math.max(rowHeight * 4, rowHeight * visibleRows));
  const current = state.virtualLists[key] || { scrollTop: 0 };
  const scrollTop = Math.min(current.scrollTop || 0, Math.max(0, items.length * rowHeight - viewportHeight));
  const start = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const end = Math.min(items.length, Math.ceil((scrollTop + viewportHeight) / rowHeight) + overscan);
  const visible = items.slice(start, end);
  state.virtualLists[key] = { scrollTop, total: items.length };
  target._virtualRender = () => renderVirtualList(target, key, items, options);

  target.classList.add("virtual-list");
  target.style.maxHeight = `${viewportHeight}px`;
  target.innerHTML = `<div class="virtual-spacer" style="height:${items.length * rowHeight}px">
    <div class="virtual-window" style="transform:translateY(${start * rowHeight}px)">
      ${visible.map((item, index) => options.render(item, start + index)).join("")}
    </div>
  </div>`;
  target.scrollTop = scrollTop;

  if (target.dataset.virtualBound !== key) {
    target.dataset.virtualBound = key;
    target.addEventListener("scroll", () => {
      const entry = state.virtualLists[key] || {};
      entry.scrollTop = target.scrollTop;
      state.virtualLists[key] = entry;
      window.requestAnimationFrame(() => {
        if (state.virtualLists[key]?.scrollTop === target.scrollTop) {
          target._virtualRender?.();
        }
      });
    });
  }
  options.bind?.(target);
}

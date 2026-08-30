async function bootstrapProtected() {
  const canLoadProtected = await loadAuthStatus();
  if (!canLoadProtected) {
    setWsStatus("error");
    return;
  }
  await loadSidebarState().catch(() => {});
  await loadSharedPinnedChatSessions().catch(() => {});
  await loadProjects().catch(showError);
  applyPreferences();
  // Re-apply the most recently persisted chat session overrides so the
  // chat-controls row reflects what was used for the last conversation.
  const persistedSessions = readSessionOverrides();
  const lastSessionId = state.preferences.lastChatSessionId;
  if (lastSessionId && persistedSessions[lastSessionId]) {
    loadSessionOverridesIntoState(lastSessionId);
    renderChatFooter(persistedSessions[lastSessionId]);
  }
  connectWs();
  const savedView = window.localStorage.getItem("iowb.lastView") || activeView() || "chat";
  const targetView = qs(`#${savedView}-view`) ? savedView : "chat";
  const savedProjectPath = activeChatSelectionMatchesServer()
    ? (window.localStorage.getItem(ACTIVE_CHAT_PROJECT_KEY) || "")
    : "";
  if (savedProjectPath && state.projects.some((project) => project.path === savedProjectPath)) {
    setActiveProject(savedProjectPath);
  }
  const savedSessionId = savedActiveChatSessionId();
  if (targetView === "chat" && savedSessionId) {
    state.chatSessionId = savedSessionId;
    renderCachedChatSession(savedSessionId);
  }
  await switchView(targetView);
  if (targetView === "chat" && savedSessionId) {
    const session = findChatSession(savedSessionId);
    await pickChatSession(savedSessionId, sessionProjectPath(session, savedProjectPath || activeProjectPath())).catch(showError);
  } else if (targetView === "chat" && !state.chatSessionId) {
    // If we landed on the chat view and nothing is selected, auto-open the
    // most recent session for the current project (or the most recent session
    // across all projects if no project is currently active).
    await autoOpenLatestChatSession().catch(showError);
  }
}

function showError(error) {
  const message = error?.message || String(error);
  showToast(message, "danger");
  if (qs("#chat-view")?.classList.contains("active")) {
    appendChatLine(`[error] ${message}`);
  } else if (qs("#shell-view")?.classList.contains("active")) {
    appendShell(`[error] ${message}\n`);
  } else if (qs("#database-view")?.classList.contains("active")) {
    setOutput("#db-output", message, "error-output");
  } else if (qs("#git-view")?.classList.contains("active")) {
    setOutput("#git-output", message, "error-output");
  } else if (qs("#files-view")?.classList.contains("active")) {
    const status = qs("#file-editor-status");
    if (status) status.textContent = message;
  } else if (qs("#board-view")?.classList.contains("active")) {
    const status = qs("#board-status");
    if (status) status.textContent = message;
  } else if (qs("#settings-view")?.classList.contains("active")) {
    setOutput("#settings-json", message, "error-output");
  } else {
    console.error(error);
  }
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}


// Append the user prompt bubble floated right, with override metadata.
// Also persists per-session override metadata so refresh reopens the same
// chat with the same setup.
function appendUserPromptToChat(prompt, meta) {
  const output = chatOutputRoot();
  if (!output) {
    renderChatFooter(null);
    return;
  }
  clearChatProcessing();
  // Finalize any pending assistant stream so the user prompt doesn't get
  // stacked behind an in-progress stream node.
  if (chatStream.node) {
    chatStream.node = null;
    chatStream.text = null;
    chatStream.role = null;
    chatStream.buffer = "";
  }
  const { node, text, footer } = buildChatLineNode("user");
  text.textContent = prompt;
  attachUserChatActions(node, { content: prompt });
  renderChatLineFooter(footer, meta);
  output.appendChild(node);
  scrollChatToBottom(true);
  updateChatEmptyState();
  renderChatFooter(null);
}

function pad2(n) { return n < 10 ? `0${n}` : `${n}`; }

function formatReceivedDateTime(iso) {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
}

function formatElapsed(fromIso, toIso) {
  const from = new Date(fromIso).getTime();
  const to = new Date(toIso).getTime();
  if (!Number.isFinite(from) || !Number.isFinite(to) || to < from) return "";
  const ms = to - from;
  if (ms < 1000) return "<1s";
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  if (m < 60) return r ? `${m}m ${r}s` : `${m}m`;
  const h = Math.floor(m / 60);
  const mr = m % 60;
  return mr ? `${h}h ${mr}m` : `${h}h`;
}

function rememberSessionMeta(sessionId, patch) {
  if (!sessionId) return;
  saveSessionOverrides(sessionId, patch);
}

function lastChatUserPromptContent() {
  const sessionId = state.chatSessionId || state.pendingChatSessionId || "";
  const messages = chatHistoryWindow.sessionId === sessionId ? chatHistoryWindow.messages : [];
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    if (String(messages[index]?.role || "").toLowerCase() === "user") {
      return String(messages[index]?.content || "");
    }
  }
  return "";
}

function appendChat(value, opts = {}) {
  const output = chatOutputRoot();
  if (!output) return;
  if (typeof value !== "string") value = String(value);
  state.chatBuffer = `${state.chatBuffer}${value}`.slice(-CHAT_LIVE_RENDER_MAX_CHARS);
  const sessionId = opts.sessionId || state.chatSessionId || state.pendingChatSessionId || state.currentSession?.sessionId || "";
  if (sessionId) state.chatOutputBuffersBySession[sessionId] = state.chatBuffer;
  // Lazily spin up the assistant stream node the first time we receive
  // content after a reset / user prompt.
  if (!chatStream.node || chatStream.role !== "assistant") {
    if (!adoptChatProcessingForStream()) {
      chatStream = { role: "assistant", ...buildChatLineNode("assistant") };
      output.appendChild(chatStream.node);
    }
  }
  const liveControls = chatSessionControls(
    sessionId,
    findChatSession(sessionId) || state.currentSession,
  );
  liveControls.cli = opts.provider || liveControls.cli;
  renderChatResponseHeader(chatStream.node, liveControls);
  // Render as Markdown so exec / Parameters and exec / Details sections
  // become collapsible blocks while the rest stays in plain Markdown form.
  // renderChatBubbleHtml keeps `textContent` safe by escaping every line.
  const displayContent = assistantResponseContent(
    state.chatBuffer,
    opts.provider || state.currentSession?.provider || chatCliValue(),
    lastChatUserPromptContent(),
  );
  chatStream.text.innerHTML = renderChatBubbleHtml(displayContent);
  scrollChatToBottom();
  updateChatEmptyState();
}

function renderChatBubbleHtml(value) {
  return renderMarkdownLiteWithSections(value).body;
}

function appendChatLine(value) {
  appendChat(`${value}\n`);
}

function appendShell(value) {
  state.shellBuffer = `${state.shellBuffer}${value}`.slice(-200_000);
  if (state.shellTerm) {
    state.shellTerm.write(value);
  } else {
    renderShell();
  }
}

function renderShell() {
  const output = qs("#shell-output");
  if (state.shellTerm) {
    state.shellTerm.clear();
    if (state.shellBuffer) {
      state.shellTerm.write(state.shellBuffer);
    }
    return;
  }
  output.innerHTML = renderAnsi(state.shellBuffer);
  output.scrollTop = output.scrollHeight;
}

function renderAnsi(value) {
  const classes = [];
  let html = "";
  let index = 0;
  const pattern = /\x1b\[([0-9;]*)m/g;
  for (const match of value.matchAll(pattern)) {
    html += escapeHtml(value.slice(index, match.index));
    const codes = (match[1] || "0").split(";").map((code) => Number(code || 0));
    for (const code of codes) {
      if (code === 0) classes.length = 0;
      if (code === 1 && !classes.includes("ansi-bold")) classes.push("ansi-bold");
      if (code >= 30 && code <= 37) {
        classes.splice(0, classes.length, ...classes.filter((item) => !item.startsWith("ansi-fg-")));
        classes.push(`ansi-fg-${code - 30}`);
      }
      if (code >= 90 && code <= 97) {
        classes.splice(0, classes.length, ...classes.filter((item) => !item.startsWith("ansi-fg-")));
        classes.push(`ansi-fg-${code - 90 + 8}`);
      }
    }
    index = match.index + match[0].length;
    if (classes.length) html += `<span class="${classes.join(" ")}">`;
    if (classes.length) {
      const next = value.slice(index).search(pattern);
      const end = next === -1 ? value.length : index + next;
      html += escapeHtml(value.slice(index, end));
      html += "</span>";
      index = end;
    }
  }
  html += escapeHtml(value.slice(index));
  return html.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");
}

function defaultShellCommand() {
  if (navigator.userAgent.includes("Windows")) return "powershell.exe";
  return "/bin/bash";
}

function initCodeEditor() {
  if (!window.CodeMirror || state.codeEditor) return;
  const textarea = qs("#file-editor-content");
  state.codeEditor = window.CodeMirror.fromTextArea(textarea, {
    autoCloseBrackets: true,
    indentUnit: 2,
    lineNumbers: true,
    lineWrapping: !!state.preferences.wrapOutput,
    matchBrackets: true,
    mode: null,
    tabSize: 2,
    viewportMargin: 80,
    extraKeys: {
      "Ctrl-S": () => saveFile(new Event("submit")).catch(showError),
      "Cmd-S": () => saveFile(new Event("submit")).catch(showError),
      "Ctrl-F": () => {
        qs("#editor-search")?.focus();
        qs("#editor-search")?.select();
      },
      "Cmd-F": () => {
        qs("#editor-search")?.focus();
        qs("#editor-search")?.select();
      },
      Tab: (editor) => editor.replaceSelection("  ", "end"),
    },
  });
  document.body.classList.add("code-editor-active");
  state.codeEditor.on("change", () => {
    if (state.suppressEditorChange) return;
    state.currentFileDirty = true;
    resetEditorSearch();
    updateEditorChrome();
  });
  state.codeEditor.on("cursorActivity", updateEditorChrome);
  refreshEditorWidget();
}

async function ensureXterm() {
  if (state.shellTerm) return true;
  const output = qs("#shell-output");
  output?.setAttribute("aria-busy", "true");
  try {
    await Promise.all([
      loadStylesheetOnce("/vendor/xterm/xterm.css"),
      loadScriptOnce("/vendor/xterm/xterm.js"),
    ]);
    initXterm();
    renderShell();
    return Boolean(state.shellTerm);
  } catch (error) {
    console.warn("[io-workbench] enhanced terminal unavailable; using text fallback", error);
    renderShell();
    return false;
  } finally {
    output?.setAttribute("aria-busy", "false");
  }
}

function initXterm() {
  const TerminalCtor = window.Terminal?.Terminal || window.Terminal;
  if (!TerminalCtor || state.shellTerm) return;
  const output = qs("#shell-output");
  const terminalSize = terminalSizeFromSettings();
  state.shellTerm = new TerminalCtor({
    cols: terminalSize.cols,
    rows: terminalSize.rows,
    convertEol: true,
    cursorBlink: true,
    scrollback: 5000,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace',
    fontSize: 13,
    theme: {
      background: "#111827",
      foreground: "#f9fafb",
    },
  });
  output.textContent = "";
  state.shellTerm.open(output);
  state.shellTerm.onData((data) => {
    sendShellInput(transformShellShortcutInput(data)).catch(showError);
  });
  document.body.classList.add("xterm-active");
  applyTerminalSizePreference(false).catch(showError);
}

bindNavigation();
bindFloatingNavigationPosition();
bindPinnedChatShortcuts();
bindCommandPalette();
bindForms();
applyPreferences();
setSettingsTab(state.activeSettingsTab);
registerServiceWorker();
syncNavigationState();
updateEditorChrome();
updateNotificationStatus();
autosizeChatPrompt();
renderShell();
updateShellStatus();
setGitActiveView("changes", { load: false });
updateMainHeader();
loadHealth().catch((error) => {
  const summary = qs("#server-summary");
  if (summary) summary.textContent = error.message;
});
bootstrapProtected();

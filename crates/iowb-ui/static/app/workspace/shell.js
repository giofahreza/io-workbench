function renderToolResponse(body) {
  if (body.run) {
    renderToolRuns("#tools-output", { namespace: body.run.namespace, runs: [body.run] });
    return;
  }
  if (body.runs) {
    renderToolRuns("#tools-output", body);
    return;
  }
  if (body.server || body.servers) {
    renderMcpServers("#tools-output", body);
    return;
  }
  renderJson("#tools-output", body);
}

function renderToolRuns(selector, body) {
  state.lastToolRuns = body;
  const target = qs(selector);
  if (!target) return;
  const filter = qs("#tool-filter")?.value || "";
  const runs = filteredItems(body.runs || [], filter, [
    "namespace",
    "action",
    "command",
    "stdout",
    "stderr",
    (run) => run.success ? "success ok" : "failed error",
  ]);
  target.className = "output-panel result-list";
  if (!runs.length) {
    target.innerHTML = `<p class="empty">No ${escapeHtml(body.namespace || "tool")} runs yet.</p>`;
    return;
  }
  const visible = runs.slice(0, state.limits.toolRuns);
  target.innerHTML = visible.map((run) => `<article class="result-row tool-run">
    <header class="row-title">
      ${resultBadge(run.success)}
      <strong>${escapeHtml(run.namespace)} · ${escapeHtml(run.action)}</strong>
      <span class="meta">${escapeHtml(formatDate(run.createdAt))} · ${run.durationMs} ms</span>
    </header>
    <span class="meta">${escapeHtml([run.command, ...(run.args || [])].join(" "))}</span>
    ${run.stdout ? `<details open><summary>stdout</summary><pre>${escapeHtml(run.stdout)}</pre></details>` : ""}
    ${run.stderr ? `<details><summary>stderr</summary><pre>${escapeHtml(run.stderr)}</pre></details>` : ""}
  </article>`).join("") + showMoreButton("toolRuns", runs.length, "toolRuns");
  bindShowMore(target);
}

function renderMcpServers(selector, body) {
  const servers = body.servers || (body.server ? [body.server] : []);
  const target = qs(selector);
  if (!target) return;
  target.className = "output-panel result-list";
  if (!servers.length) {
    target.innerHTML = '<p class="empty">No MCP servers recorded.</p>';
    return;
  }
  target.innerHTML = servers.map((server) => `<article class="result-row">
    <header class="row-title">
      <strong>${escapeHtml(server.name || server.id)}</strong>
      <span class="badge ${server.status === "running" ? "ok" : "warn"}">${escapeHtml(server.status || "unknown")}</span>
    </header>
    <span class="meta">${escapeHtml(server.id)} · process ${escapeHtml(server.processId || "")}</span>
    <span>${escapeHtml([server.command, ...(server.args || [])].join(" "))}</span>
    <div class="row-actions">
      <button type="button" data-mcp-use="${escapeHtml(server.id)}">Use ID</button>
      ${server.status === "running" ? `<button type="button" data-mcp-stop="${escapeHtml(server.id)}">Stop</button>` : ""}
    </div>
  </article>`).join("");
  target.querySelectorAll("[data-mcp-use]").forEach((button) => {
    button.addEventListener("click", () => {
      qs("#mcp-server-id").value = button.dataset.mcpUse;
    });
  });
  target.querySelectorAll("[data-mcp-stop]").forEach((button) => {
    button.addEventListener("click", () => {
      qs("#mcp-server-id").value = button.dataset.mcpStop;
      stopMcpServer().catch(showError);
    });
  });
}

async function runTool(event) {
  event.preventDefault();
  const kind = qs("#tool-kind").value;
  const endpoint = {
    mcp: "/api/mcp/tools/call",
    "mcp-utils": "/api/mcp-utils/run",
    commands: "/api/commands/run",
    plugins: "/api/plugins/run",
    taskmaster: "/api/taskmaster/run",
    danger: "/api/danger/boards",
    notifications: "/api/notifications/push",
  }[kind];
  const payload = parseJsonField("#tool-payload", {});
  const command = qs("#tool-command").value.trim();
  const args = parseJsonField("#tool-args", []);
  const body = await api(endpoint, {
    method: "POST",
    body: JSON.stringify({ command: command || undefined, args, payload }),
  });
  renderToolResponse(body);
}

async function loadToolRuns() {
  const kind = qs("#tool-kind").value;
  const body = await api(`/api/tool-runs/${encodeURIComponent(kind)}`);
  renderToolRuns("#tools-output", body);
}

async function startMcpServer(event) {
  event.preventDefault();
  const command = qs("#mcp-server-command").value.trim();
  if (!command) return;
  const body = await api("/api/mcp/servers", {
    method: "POST",
    body: JSON.stringify({
      name: qs("#mcp-server-name").value.trim() || command,
      command,
      args: parseJsonField("#mcp-server-args", []),
    }),
  });
  renderMcpServers("#tools-output", body);
}

async function loadMcpServers() {
  const body = await api("/api/mcp/servers");
  renderMcpServers("#tools-output", body);
}

async function stopMcpServer() {
  const serverId = qs("#mcp-server-id").value.trim();
  if (!serverId) return;
  const body = await api(`/api/mcp/servers/${encodeURIComponent(serverId)}`, {
    method: "DELETE",
  });
  renderMcpServers("#tools-output", body);
}

async function transcribeAudio(event) {
  event.preventDefault();
  const file = qs("#audio-file").files[0];
  if (!file) return;
  const formData = new FormData();
  formData.append("audio", file);
  const body = await apiUpload("/api/audio/transcribe", formData);
  renderJson("#tools-output", body);
  qs("#audio-file").value = "";
}

async function loadProcesses() {
  const body = await api("/api/process");
  renderProcesses(body);
}

function updateShellStatus(label = "") {
  const dot = qs("#shell-status-dot");
  const status = qs("#shell-status-label");
  if (!dot || !status) return;
  dot.classList.toggle("connected", !!state.currentShellProcess);
  dot.classList.toggle("connecting", !!state.shellStarting);
  status.textContent = label
    || (state.shellStarting ? "Starting terminal" : state.currentShellProcess ? "Terminal connected" : "Terminal");
  qs("#stop-shell").disabled = !state.currentShellProcess;
}

function focusShellTerm() {
  if (activeView() !== "shell") return;
  window.requestAnimationFrame(() => {
    if (state.shellTerm?.focus) {
      state.shellTerm.focus();
    } else {
      qs("#shell-output")?.focus();
    }
  });
}

async function startShell(options = {}) {
  if (state.shellStarting) return;
  const command = defaultShellCommand();
  const projectPath = activeProjectPath("#active-project");
  if (!projectPath) return;
  if (!options.force && state.currentShellProcess && state.currentShellProjectPath === projectPath) {
    focusShellTerm();
    return;
  }
  state.shellStarting = true;
  updateShellStatus("Starting terminal");
  if (options.force && state.currentShellProcess) {
    await api(`/api/process/${encodeURIComponent(state.currentShellProcess)}`, { method: "DELETE" }).catch(() => {});
    state.currentShellProcess = null;
    state.currentShellProjectPath = "";
    resetShellResizeTracking();
  }
  state.shellBuffer = "";
  renderShell();
  const terminalSize = terminalSizeFromSettings();
  state.shellTerm?.resize(terminalSize.cols, terminalSize.rows);
  try {
    const body = await api("/api/process", {
      method: "POST",
      body: JSON.stringify({
        command,
        args: [],
        cwd: projectPath,
        pty: true,
        cols: terminalSize.cols,
        rows: terminalSize.rows,
      }),
    });
    state.currentShellProcess = body.id;
    state.currentShellProjectPath = projectPath;
    state.shellLastResizeSignature = `${body.id}:${terminalSize.cols}x${terminalSize.rows}`;
    if (options.auto) state.shellAutoStartedProjectPath = projectPath;
    appendShell(`[started ${body.id}]\n`);
    updateShellStatus();
    focusShellTerm();
    loadProcesses().catch(() => {});
  } finally {
    state.shellStarting = false;
    updateShellStatus();
  }
}

async function ensureShellRunningForActiveProject() {
  const projectPath = activeProjectPath("#active-project");
  if (!projectPath || state.shellStarting) {
    updateShellStatus();
    focusShellTerm();
    return;
  }
  if (state.currentShellProcess && state.currentShellProjectPath !== projectPath) {
    await startShell({ auto: true, force: true });
    return;
  }
  if (state.currentShellProcess) {
    updateShellStatus();
    focusShellTerm();
    return;
  }
  await startShell({ auto: true });
}

async function sendShellInput(data) {
  if (!state.currentShellProcess) return;
  if (state.ws && state.ws.readyState === WebSocket.OPEN) {
    state.ws.send(JSON.stringify({
      type: "process_input",
      processId: state.currentShellProcess,
      data,
    }));
    return;
  }
  await api(`/api/process/${encodeURIComponent(state.currentShellProcess)}/input`, {
    method: "POST",
    body: JSON.stringify({ data }),
  });
}

async function resizeCurrentShell() {
  if (!state.currentShellProcess) return;
  const payload = terminalSizeFromSettings();
  const signature = `${state.currentShellProcess}:${payload.cols}x${payload.rows}`;
  if (state.shellLastResizeSignature === signature) return;
  state.shellTerm?.resize(payload.cols, payload.rows);
  if (state.ws && state.ws.readyState === WebSocket.OPEN) {
    state.ws.send(JSON.stringify({
      type: "resize_terminal",
      processId: state.currentShellProcess,
      ...payload,
    }));
  } else {
    await api(`/api/process/${encodeURIComponent(state.currentShellProcess)}/resize`, {
      method: "POST",
      body: JSON.stringify(payload),
    });
  }
  state.shellLastResizeSignature = signature;
}

function resetShellResizeTracking() {
  state.shellLastResizeSignature = "";
}

function shellFitSize() {
  const output = qs("#shell-output");
  if (!output || !output.clientWidth) return null;
  const styles = getComputedStyle(output);
  const probe = document.createElement("span");
  probe.textContent = "W";
  probe.style.position = "absolute";
  probe.style.visibility = "hidden";
  probe.style.whiteSpace = "pre";
  probe.style.fontFamily = styles.fontFamily;
  probe.style.fontSize = styles.fontSize;
  probe.style.lineHeight = styles.lineHeight;
  output.appendChild(probe);
  const rect = probe.getBoundingClientRect();
  probe.remove();
  const fontSize = Number.parseFloat(styles.fontSize) || 13;
  const charWidth = rect.width || fontSize * 0.62;
  const charHeight = rect.height || fontSize * 1.45;
  const horizontalPadding = Number.parseFloat(styles.paddingLeft || 0) + Number.parseFloat(styles.paddingRight || 0) + 2;
  const verticalPadding = Number.parseFloat(styles.paddingTop || 0) + Number.parseFloat(styles.paddingBottom || 0) + 2;
  const width = Math.max(0, output.clientWidth - horizontalPadding);
  const height = Math.max(0, output.clientHeight - verticalPadding);
  return {
    cols: Math.min(300, Math.max(20, Math.floor(width / charWidth))),
    rows: Math.min(120, Math.max(8, Math.floor(height / charHeight))),
  };
}

async function fitShellTermToContainer(syncServer = false) {
  const size = shellFitSize();
  if (!size) return;
  qs("#shell-cols").value = String(size.cols);
  qs("#shell-rows").value = String(size.rows);
  state.preferences.shellCols = size.cols;
  state.preferences.shellRows = size.rows;
  savePreferences();
  state.shellTerm?.resize(size.cols, size.rows);
  if (syncServer && state.currentShellProcess) {
    await resizeCurrentShell();
  }
}

function handleShellOutputKey(event) {
  if (!state.currentShellProcess) return;
  const data = terminalKeyData(event);
  if (!data) return;
  event.preventDefault();
  sendShellInput(transformShellShortcutInput(data)).catch(showError);
}

function terminalKeyData(event) {
  if (event.ctrlKey && event.key.length === 1) {
    const code = event.key.toUpperCase().charCodeAt(0);
    if (code >= 64 && code <= 95) return String.fromCharCode(code - 64);
  }
  if (event.altKey || event.metaKey) return "";
  if (event.key.length === 1) return event.key;
  return {
    Enter: "\r",
    Backspace: "\x7f",
    Tab: "\t",
    Escape: "\x1b",
    ArrowUp: "\x1b[A",
    ArrowDown: "\x1b[B",
    ArrowRight: "\x1b[C",
    ArrowLeft: "\x1b[D",
    Delete: "\x1b[3~",
    Home: "\x1b[H",
    End: "\x1b[F",
    PageUp: "\x1b[5~",
    PageDown: "\x1b[6~",
  }[event.key] || "";
}

function updateShellModifierButtons() {
  qs("#shell-mod-ctrl")?.classList.toggle("active", !!state.shellCtrlActive);
  qs("#shell-mod-alt")?.classList.toggle("active", !!state.shellAltActive);
}

function transformShellShortcutInput(data) {
  let output = data;
  if (state.shellCtrlActive && data.length === 1) {
    const code = data.toLowerCase().charCodeAt(0);
    if (code >= 97 && code <= 122) {
      output = String.fromCharCode(code - 96);
    }
    state.shellCtrlActive = false;
  }
  if (state.shellAltActive && data.length === 1) {
    output = `\x1b${output}`;
    state.shellAltActive = false;
  }
  updateShellModifierButtons();
  return output;
}

function sendShellShortcut(data) {
  sendShellInput(transformShellShortcutInput(data)).catch(showError);
  focusShellTerm();
}

function decodeShellSequence(value = "") {
  return value
    .replaceAll("\\u001b", "\x1b")
    .replaceAll("\\t", "\t");
}

function latestShellUrl() {
  const matches = state.shellBuffer.match(/https?:\/\/[^\s"'<>]+/g) || [];
  return matches.at(-1) || "";
}

async function copyShellText(text, fallbackMessage) {
  if (!text) {
    showToast(fallbackMessage, "warn");
    focusShellTerm();
    return;
  }
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
  } else {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    document.execCommand("copy");
    area.remove();
  }
  showToast("Copied", "ok");
  focusShellTerm();
}

async function pasteIntoShell() {
  let text = "";
  if (navigator.clipboard?.readText) {
    try {
      text = await navigator.clipboard.readText();
    } catch {
      text = window.prompt("Paste text to send to terminal:") || "";
    }
  } else {
    text = window.prompt("Paste text to send to terminal:") || "";
  }
  if (text) sendShellShortcut(text);
}

function bindShellShortcuts() {
  document.querySelectorAll(".terminal-shortcuts button").forEach((button) => {
    button.addEventListener("pointerdown", (event) => event.preventDefault());
  });
  qs("#shell-copy-selection").addEventListener("click", () => {
    copyShellText(state.shellTerm?.getSelection?.() || "", "No terminal selection").catch(showError);
  });
  qs("#shell-copy-latest-link").addEventListener("click", () => {
    copyShellText(latestShellUrl(), "No URL found").catch(showError);
  });
  qs("#shell-paste").addEventListener("click", () => pasteIntoShell().catch(showError));
  qs("#shell-scroll-bottom").addEventListener("click", () => {
    state.shellTerm?.scrollToBottom?.();
    focusShellTerm();
  });
  document.querySelectorAll("[data-shell-sequence]").forEach((button) => {
    button.addEventListener("click", () => {
      sendShellShortcut(decodeShellSequence(button.dataset.shellSequence || ""));
    });
  });
  document.querySelectorAll("[data-shell-modifier]").forEach((button) => {
    button.addEventListener("click", () => {
      const modifier = button.dataset.shellModifier;
      if (modifier === "ctrl") state.shellCtrlActive = !state.shellCtrlActive;
      if (modifier === "alt") state.shellAltActive = !state.shellAltActive;
      updateShellModifierButtons();
      focusShellTerm();
    });
  });
}

function renderProcesses(processes) {
  const target = qs("#process-list");
  if (!Array.isArray(processes) || !processes.length) {
    target.innerHTML = "";
    target.classList.remove("active");
    return;
  }
  target.classList.toggle("active", !!state.shellProcessListOpen);
  target.innerHTML = processes.map((process) => `<article class="row process-row">
    <strong>${escapeHtml(process.command)}</strong>
    <span class="meta">${escapeHtml(process.id)} · ${process.pty ? "PTY" : "process"} · ${escapeHtml(formatDate(process.started_at || process.startedAt))}</span>
    ${process.cwd ? `<span>${escapeHtml(process.cwd)}</span>` : ""}
    <div class="row-actions">
      <button type="button" data-process-use="${escapeHtml(process.id)}">Use</button>
      <button type="button" data-process-stop="${escapeHtml(process.id)}">Stop</button>
    </div>
  </article>`).join("");
  target.querySelectorAll("[data-process-use]").forEach((button) => {
    button.addEventListener("click", () => {
      state.currentShellProcess = button.dataset.processUse;
      state.currentShellProjectPath = "";
      resetShellResizeTracking();
      appendShell(`[selected ${state.currentShellProcess}]\n`);
      updateShellStatus();
      focusShellTerm();
    });
  });
  target.querySelectorAll("[data-process-stop]").forEach((button) => {
    button.addEventListener("click", async () => {
      await api(`/api/process/${encodeURIComponent(button.dataset.processStop)}`, { method: "DELETE" });
      if (state.currentShellProcess === button.dataset.processStop) {
        state.currentShellProcess = null;
        state.currentShellProjectPath = "";
        resetShellResizeTracking();
      }
      updateShellStatus();
      await loadProcesses();
    });
  });
}

function renderSettingsResponse(body) {
  if (body?.apiKeys) {
    renderApiKeys(body.apiKeys);
    return;
  }
  if (body?.preferences) {
    renderNotificationPreferences(body.preferences);
    return;
  }
  if (body?.credentials) {
    renderCredentials(body.credentials);
    return;
  }
  if (body?.apiKey) {
    renderCreatedApiKey(body.apiKey);
    return;
  }
  if (body?.config && body?.effective && Object.prototype.hasOwnProperty.call(body, "runtimeReady")) {
    renderIoGatewayConfig(body);
    return;
  }
  state.lastSettingsRows = null;
  renderJson("#settings-json", body);
}

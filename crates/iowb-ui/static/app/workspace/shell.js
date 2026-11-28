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
  const attachment = state.shellAttachment;
  const connected = !!state.currentShellProcess && attachment?.ready && attachment.phase === "running";
  const connecting = state.shellStarting || attachment?.syncing;
  dot.classList.toggle("connected", !!connected);
  dot.classList.toggle("connecting", !!connecting);
  status.textContent = label
    || (connecting ? "Connecting terminal" : connected ? `Terminal connected${attachment.safe ? " · safe mode" : ""}`
      : attachment?.phase === "exited" ? "Terminal exited" : attachment ? "Terminal disconnected" : "Terminal");
  qs("#stop-shell").disabled = !state.currentShellProcess;
}

function resetShellScreen() {
  state.shellBuffer = "";
  if (state.shellTerm) {
    state.shellTerm.dispose();
    state.shellTerm = null;
    initXterm();
  }
  renderShell();
}

function clearShellOutput() {
  const attachment = state.shellAttachment;
  if (attachment) {
    attachment.sequence = Math.max(attachment.sequence, ...attachment.pending.keys());
    attachment.pending.clear();
    attachment.pendingBytes = 0;
    attachment.legacy = [];
    attachment.legacyBytes = 0;
  }
  resetShellScreen();
}

function detachShellAttachment() {
  const previous = state.shellAttachment;
  if (previous) {
    window.clearTimeout(previous.timer);
    previous.controller.abort();
  }
  state.shellAttachment = null;
  state.currentShellProcess = null;
  state.currentShellProjectPath = "";
  resetShellResizeTracking();
  return previous;
}

function createShellAttachment(id, projectPath, shell = {}) {
  const attachment = {
    id, projectPath, safe: !!shell.safe, phase: "attaching", ready: false,
    sequence: 0, pending: new Map(), pendingBytes: 0, legacy: [], legacyBytes: 0,
    syncing: false, snapshotPromise: null, refreshAgain: false,
    pendingExit: null, timer: null, controller: new AbortController(),
    inputPromise: Promise.resolve(),
  };
  state.shellAttachment = attachment;
  state.currentShellProcess = id;
  state.currentShellProjectPath = projectPath;
  return attachment;
}

function queueShellChunk(attachment, chunk) {
  const sequence = Number(chunk.sequence);
  if (!Number.isSafeInteger(sequence) || sequence <= 0) {
    const data = String(chunk.data || "");
    attachment.legacy.push(data);
    attachment.legacyBytes += data.length;
    while (attachment.legacy.length > 128 || attachment.legacyBytes > 128_000) {
      attachment.legacyBytes -= attachment.legacy.shift().length;
    }
    return;
  }
  if (sequence <= attachment.sequence || attachment.pending.has(sequence)) return;
  const data = String(chunk.data || "");
  attachment.pending.set(sequence, data);
  attachment.pendingBytes += data.length;
  if (attachment.pendingBytes > 128_000 || attachment.pending.size > 512) {
    attachment.pending.clear();
    attachment.pendingBytes = 0;
    scheduleShellSnapshot(attachment);
  }
}

function flushShellChunks(attachment) {
  if (state.shellAttachment !== attachment || attachment.syncing) return;
  while (attachment.pending.has(attachment.sequence + 1)) {
    const sequence = attachment.sequence + 1;
    const data = attachment.pending.get(sequence);
    attachment.pending.delete(sequence);
    attachment.pendingBytes -= data.length;
    attachment.sequence = sequence;
    appendShell(data);
  }
  for (const data of attachment.legacy.splice(0)) appendShell(data);
  attachment.legacyBytes = 0;
  if (attachment.pending.size) scheduleShellSnapshot(attachment);
}

function finishShellAttachment(attachment, code, error = "") {
  if (state.shellAttachment !== attachment || attachment.phase === "exited") return;
  attachment.phase = "exited";
  attachment.ready = false;
  window.clearTimeout(attachment.timer);
  state.currentShellProcess = null;
  resetShellResizeTracking();
  appendShell(`\n[process exited: ${code ?? "terminated"}] ${attachment.id}\n`);
  if (error) appendShell(`[terminal error] ${error}\n`);
  updateShellStatus();
}

function scheduleShellSnapshot(attachment, delay = 30) {
  if (state.shellAttachment !== attachment || attachment.phase === "exited" || attachment.timer) return;
  attachment.timer = window.setTimeout(() => {
    attachment.timer = null;
    refreshShellSnapshot(attachment).catch(showError);
  }, delay);
}

function refreshShellSnapshot(attachment = state.shellAttachment) {
  if (!attachment || state.shellAttachment !== attachment || attachment.phase === "exited") return Promise.resolve();
  if (attachment.snapshotPromise) {
    attachment.refreshAgain = true;
    return attachment.snapshotPromise;
  }
  window.clearTimeout(attachment.timer);
  attachment.timer = null;
  attachment.syncing = true;
  updateShellStatus();
  attachment.snapshotPromise = api(`/api/terminal/${encodeURIComponent(attachment.id)}?afterSequence=${attachment.sequence}`, {
    signal: attachment.controller.signal,
  }).then((snapshot) => {
    if (state.shellAttachment !== attachment) return;
    if (snapshot.process?.id !== attachment.id || !Number.isSafeInteger(snapshot.lastSequence)) {
      throw new Error("Invalid terminal snapshot from the Workbench host.");
    }
    if (snapshot.firstSequence > attachment.sequence + 1) {
      appendShell("\n[earlier terminal output is no longer available]\n");
      attachment.sequence = snapshot.firstSequence - 1;
      for (const [sequence, data] of attachment.pending) {
        if (sequence <= attachment.sequence) {
          attachment.pending.delete(sequence);
          attachment.pendingBytes -= data.length;
        }
      }
    }
    for (const chunk of snapshot.chunks || []) queueShellChunk(attachment, chunk);
    attachment.syncing = false;
    flushShellChunks(attachment);
    if (!snapshot.running) {
      finishShellAttachment(attachment, snapshot.exitCode, snapshot.error);
    } else {
      attachment.ready = !attachment.pendingExit;
      attachment.phase = attachment.pendingExit ? "closing" : "running";
    }
    updateShellStatus();
  }).catch((error) => {
    if (state.shellAttachment !== attachment || error.name === "AbortError") return;
    attachment.ready = false;
    if (error.status === 404) {
      finishShellAttachment(attachment, null, "The process ended or the host restarted; its retained output is unavailable.");
      return;
    }
    attachment.phase = "disconnected";
    throw error;
  }).finally(() => {
    attachment.syncing = false;
    attachment.snapshotPromise = null;
    if (state.shellAttachment === attachment) {
      updateShellStatus();
      if (attachment.refreshAgain && attachment.phase !== "exited") {
        attachment.refreshAgain = false;
        scheduleShellSnapshot(attachment);
      } else if (attachment.phase === "closing" || (attachment.phase === "running" && state.ws?.readyState !== WebSocket.OPEN)) {
        scheduleShellSnapshot(attachment, 1000);
      }
    }
  });
  return attachment.snapshotPromise;
}

function handleShellProcessEvent(payload) {
  const attachment = state.shellAttachment;
  if (!attachment || payload.processId !== attachment.id || attachment.phase === "exited") return;
  if (payload.type === "process_output") {
    queueShellChunk(attachment, payload);
    if (attachment.ready) flushShellChunks(attachment);
    else if (!attachment.syncing) scheduleShellSnapshot(attachment, 1000);
  } else if (payload.type === "process_exited") {
    attachment.pendingExit = payload;
    attachment.ready = false;
    attachment.phase = "closing";
    refreshShellSnapshot(attachment).catch(showError);
  }
}

async function useShellProcess(id, projectPath = activeProjectPath("#active-project")) {
  ++state.shellGeneration;
  state.shellStarting = false;
  detachShellAttachment();
  resetShellScreen();
  const attachment = createShellAttachment(id, projectPath);
  appendShell(`[selected ${id}]\n`);
  sendWorkspaceSubscription();
  await refreshShellSnapshot(attachment);
  focusShellTerm();
}

async function stopShell() {
  const attachment = state.shellAttachment;
  if (!attachment || !state.currentShellProcess) return;
  await api(`/api/process/${encodeURIComponent(attachment.id)}`, { method: "DELETE" }).catch((error) => {
    if (error.status !== 404) throw error;
  });
  if (state.shellAttachment === attachment && attachment.phase !== "exited") {
    attachment.ready = false;
    attachment.phase = "closing";
    updateShellStatus("Stopping terminal");
    await refreshShellSnapshot(attachment);
  }
  loadProcesses().catch(() => {});
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
  const projectPath = activeProjectPath("#active-project");
  if (!projectPath) return;
  if (!options.force && state.currentShellProcess && state.currentShellProjectPath === projectPath) {
    focusShellTerm();
    return;
  }
  state.shellStarting = true;
  const generation = ++state.shellGeneration;
  updateShellStatus("Starting terminal");
  const previous = detachShellAttachment();
  resetShellScreen();
  const terminalSize = terminalSizeFromSettings();
  state.shellTerm?.resize(terminalSize.cols, terminalSize.rows);
  try {
    if (previous && previous.phase !== "exited") {
      await api(`/api/process/${encodeURIComponent(previous.id)}`, { method: "DELETE" }).catch((error) => {
        if (error.status !== 404) throw error;
      });
    }
    const body = await api("/api/terminal", {
      method: "POST",
      body: JSON.stringify({
        cwd: projectPath,
        safe: !!options.safe,
        cols: terminalSize.cols,
        rows: terminalSize.rows,
      }),
    });
    if (generation !== state.shellGeneration || projectPath !== activeProjectPath("#active-project")) {
      await api(`/api/process/${encodeURIComponent(body.id)}`, { method: "DELETE" }).catch(() => {});
      return;
    }
    const attachment = createShellAttachment(body.id, projectPath, body.shell);
    state.shellLastResizeSignature = `${body.id}:${terminalSize.cols}x${terminalSize.rows}`;
    if (options.auto) state.shellAutoStartedProjectPath = projectPath;
    appendShell(`[started ${body.id}] ${body.shell?.command || ""}${body.shell?.safe ? " · safe mode" : ""}\n`);
    sendWorkspaceSubscription();
    await refreshShellSnapshot(attachment);
    focusShellTerm();
    loadProcesses().catch(() => {});
  } finally {
    if (generation === state.shellGeneration) {
      state.shellStarting = false;
      updateShellStatus();
      if (projectPath !== activeProjectPath("#active-project") && activeView() === "shell") {
        ensureShellRunningForActiveProject().catch(showError);
      }
    }
  }
}

async function ensureShellRunningForActiveProject() {
  const projectPath = activeProjectPath("#active-project");
  if (!projectPath || state.shellStarting) {
    updateShellStatus();
    focusShellTerm();
    return;
  }
  if (state.shellAttachment && state.currentShellProjectPath !== projectPath) {
    await startShell({ auto: true, force: true });
    return;
  }
  if (state.shellAttachment) {
    updateShellStatus();
    focusShellTerm();
    return;
  }
  await startShell({ auto: true });
}

async function sendShellInput(data) {
  const attachment = state.shellAttachment;
  if (!state.currentShellProcess || !attachment?.ready) return;
  const pending = attachment.inputPromise.catch(() => {}).then(async () => {
    if (state.shellAttachment !== attachment || !attachment.ready) return;
    if (state.ws?.readyState === WebSocket.OPEN) {
      state.ws.send(JSON.stringify({ type: "process_input", processId: attachment.id, data }));
      return;
    }
    await api(`/api/process/${encodeURIComponent(attachment.id)}/input`, {
      method: "POST", body: JSON.stringify({ data }), signal: attachment.controller.signal,
    });
  });
  attachment.inputPromise = pending;
  await pending;
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
      useShellProcess(button.dataset.processUse).catch(showError);
    });
  });
  target.querySelectorAll("[data-process-stop]").forEach((button) => {
    button.addEventListener("click", () => withButtonLoading(button, async () => {
      if (state.currentShellProcess === button.dataset.processStop) {
        await stopShell();
        return;
      }
      await api(`/api/process/${encodeURIComponent(button.dataset.processStop)}`, { method: "DELETE" });
      await loadProcesses();
    }).catch(showError));
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

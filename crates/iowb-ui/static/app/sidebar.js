function orderedProjects() {
  const order = new Map(state.projectOrder.map((path, index) => [path, index]));
  return [...state.projects].sort((a, b) => {
    const aIndex = order.has(a.path) ? order.get(a.path) : Number.MAX_SAFE_INTEGER;
    const bIndex = order.has(b.path) ? order.get(b.path) : Number.MAX_SAFE_INTEGER;
    if (aIndex !== bIndex) return aIndex - bIndex;
    return a.name.localeCompare(b.name);
  });
}

function syncProjectOrder() {
  const knownPaths = new Set(state.projects.map((project) => project.path));
  const ordered = state.projectOrder.filter((path) => knownPaths.has(path));
  for (const project of state.projects) {
    if (!ordered.includes(project.path)) ordered.push(project.path);
  }
  state.projectOrder = ordered;
  saveSidebarStateLocal();
}

function saveProjectMeta() {
  persistSidebarState();
}

function saveExpandedProjectPaths() {
  persistSidebarState();
}

function hapticFeedback(pattern = 12) {
  try {
    if (navigator.vibrate) navigator.vibrate(pattern);
  } catch {
    // Browser haptics are best effort and not supported on every platform.
  }
}

function sidebarStatePayload() {
  return {
    projectOrder: state.projectOrder,
    projectMeta: state.projectMeta,
    expandedProjectPaths: [...state.expandedProjectPaths],
    updatedAt: new Date().toISOString(),
  };
}

function saveSidebarStateLocal(payload = sidebarStatePayload()) {
  window.localStorage.setItem("iowb.projectOrder", JSON.stringify(payload.projectOrder || []));
  window.localStorage.setItem("iowb.projectMeta", JSON.stringify(payload.projectMeta || {}));
  window.localStorage.setItem("iowb.expandedProjects", JSON.stringify(payload.expandedProjectPaths || []));
  if (Object.prototype.hasOwnProperty.call(payload, "pinnedChatSessions")) {
    window.localStorage.setItem(PINNED_CHAT_SESSIONS_KEY, JSON.stringify(payload.pinnedChatSessions || []));
  }
  if (payload.updatedAt) window.localStorage.setItem(SIDEBAR_STATE_UPDATED_KEY, payload.updatedAt);
}

function applySidebarStatePayload(payload) {
  if (!payload || typeof payload !== "object") return false;
  if (Array.isArray(payload.projectOrder)) {
    state.projectOrder = payload.projectOrder.filter((path) => typeof path === "string");
  }
  if (payload.projectMeta && typeof payload.projectMeta === "object" && !Array.isArray(payload.projectMeta)) {
    state.projectMeta = payload.projectMeta;
  }
  if (Array.isArray(payload.expandedProjectPaths)) {
    state.expandedProjectPaths = new Set(payload.expandedProjectPaths.filter((path) => typeof path === "string"));
  }
  if (Array.isArray(payload.pinnedChatSessions)) {
    state.legacySidebarPinnedChatSessions = normalizePinnedChatSessions(payload.pinnedChatSessions);
  }
  saveSidebarStateLocal(payload);
  return true;
}

async function loadSidebarState() {
  try {
    const body = await api("/api/settings");
    const remote = (body?.settings || []).find((entry) => entry.key === SIDEBAR_STATE_SETTING_KEY)?.value;
    if (!remote || typeof remote !== "object") return false;
    const localUpdatedAt = Date.parse(window.localStorage.getItem(SIDEBAR_STATE_UPDATED_KEY) || "") || 0;
    const remoteUpdatedAt = Date.parse(remote.updatedAt || "") || 0;
    if (!localUpdatedAt || (remoteUpdatedAt && remoteUpdatedAt >= localUpdatedAt)) {
      return applySidebarStatePayload(remote);
    }
  } catch (error) {
    console.debug("sidebar state load skipped", error);
  }
  return false;
}

function persistSidebarState() {
  const payload = sidebarStatePayload();
  saveSidebarStateLocal(payload);
  window.clearTimeout(state.sidebarStatePersistTimer);
  state.sidebarStatePersistTimer = window.setTimeout(() => {
    api(`/api/settings/value/${encodeURIComponent(SIDEBAR_STATE_SETTING_KEY)}`, {
      method: "PUT",
      body: JSON.stringify({ value: payload }),
    }).catch(() => {});
  }, 450);
}

function sidebarProjectMeta(path) {
  return state.projectMeta[path] || {};
}

function projectDisplayName(project) {
  return sidebarProjectMeta(project.path).label || project.name;
}

function selectedProjectLabel(selector = "#active-project") {
  const path = activeProjectPath(selector);
  const project = state.projects.find((item) => item.path === path);
  return project ? projectDisplayName(project) : "No project";
}

function updateMainHeader(view = activeView()) {
  const title = qs("#view-title");
  const headerSubtitle = qs("#view-subtitle");
  if (title) title.textContent = VIEW_NAMES[view] || view;
  const subtitle = selectedProjectLabel("#active-project");
  if (headerSubtitle) headerSubtitle.textContent = subtitle;
  const bottomTitle = qs("#bottom-view-title");
  const bottomMeta = qs("#bottom-view-meta");
  if (bottomTitle) bottomTitle.textContent = VIEW_NAMES[view] || view;
  if (bottomMeta) bottomMeta.textContent = subtitle;
}

function renameSidebarProject(path) {
  const project = state.projects.find((item) => item.path === path);
  if (!project) return;
  const current = projectDisplayName(project);
  const next = window.prompt("Project display name", current);
  if (next === null) return;
  const label = next.trim();
  state.projectMeta[path] = {
    ...sidebarProjectMeta(path),
    label: label && label !== project.name ? label : undefined,
  };
  if (!state.projectMeta[path].label && !state.projectMeta[path].hidden) {
    delete state.projectMeta[path];
  }
  state.openProjectMenuPath = "";
  saveProjectMeta();
  renderProjectOptions();
  renderSidebarProjects();
}

function hideSidebarProject(path) {
  if (!path) return;
  state.projectMeta[path] = {
    ...sidebarProjectMeta(path),
    hidden: true,
  };
  state.openProjectMenuPath = "";
  state.expandedProjectPaths.delete(path);
  saveProjectMeta();
  saveExpandedProjectPaths();
  renderSidebarProjects();
  showToast("Project removed from sidebar", "ok");
}

function projectDropPlacement(sourcePath, targetPath, event, target) {
  const rect = target.getBoundingClientRect();
  return event.clientY >= rect.top + rect.height / 2 ? "after" : "before";
}

function moveProjectOrder(sourcePath, targetPath, placement = "before") {
  if (!sourcePath || !targetPath || sourcePath === targetPath) return;
  syncProjectOrder();
  const order = state.projectOrder.filter((path) => path !== sourcePath);
  const targetIndex = order.indexOf(targetPath);
  const insertIndex = targetIndex < 0 ? order.length : targetIndex + (placement === "after" ? 1 : 0);
  order.splice(insertIndex, 0, sourcePath);
  state.projectOrder = order;
  persistSidebarState();
  hapticFeedback(8);
  renderSidebarProjects();
}

function setWsStatus(status, detail = "") {
  const dot = qs("#ws-dot");
  const label = qs("#ws-label");
  if (!dot || !label) return;
  dot.className = "dot";
  if (status === "connected") {
    dot.classList.add("ok");
    label.textContent = "Connected";
    label.title = detail || "WebSocket connected";
  } else if (status === "reconnecting") {
    label.textContent = "Reconnecting";
    label.title = detail || "WebSocket reconnecting";
  } else if (status === "error") {
    dot.classList.add("error");
    label.textContent = "Disconnected";
    label.title = detail || "WebSocket disconnected";
  } else {
    label.textContent = "Connecting";
    label.title = detail || "WebSocket connecting";
  }
  state.wsLastDetail = label.title;
}

function showAuthPanel(mode) {
  const otpMode = mode === "otp";
  const passwordInput = qs("#auth-password");
  if (state.auth) state.auth = { ...state.auth, isAuthenticated: false };
  detachAuthProtectedShell();
  qs("#auth-panel").classList.remove("hidden");
  qs("#auth-title").textContent = mode === "setup" ? "Create Account" : otpMode ? "Enter OTP" : "Welcome Back";
  qs("#auth-description").textContent = mode === "setup"
    ? "Create your first local io-workbench account."
    : otpMode
      ? "Enter the OTP configured for this io-workbench server."
      : "Sign in to your io-workbench workspace.";
  qs("#auth-submit").textContent = mode === "setup" ? "Create Account" : otpMode ? "Unlock" : "Sign In";
  qs("#auth-form").dataset.mode = mode;
  qs("#auth-username-label").classList.toggle("hidden", otpMode);
  qs("#auth-username").required = !otpMode;
  qs("#auth-username").disabled = otpMode;
  qs("#auth-password-text").textContent = otpMode ? "OTP" : "Password";
  passwordInput.type = otpMode ? "text" : "password";
  passwordInput.autocomplete = otpMode ? "one-time-code" : "current-password";
  if (otpMode) {
    passwordInput.inputMode = "numeric";
    passwordInput.pattern = "[0-9]*";
    passwordInput.maxLength = 6;
  } else {
    passwordInput.removeAttribute("inputmode");
    passwordInput.removeAttribute("pattern");
    passwordInput.removeAttribute("maxlength");
  }
  passwordInput.placeholder = otpMode ? "Enter 6-digit OTP" : "Enter your password";
  qs("#auth-password-toggle").classList.toggle("hidden", otpMode);
  qs("#auth-logout")?.classList.add("hidden");
}

function authPanelMode() {
  if (state.auth?.needsSetup) return "setup";
  return state.auth?.authMode === "otp" ? "otp" : "login";
}

function hideAuthPanel() {
  attachAuthProtectedShell();
  qs("#auth-panel").classList.add("hidden");
  qs("#auth-message").textContent = "";
  if (state.token) {
    qs("#auth-logout")?.classList.remove("hidden");
  }
}

function renderProjects() {
  renderProjectOptions();
  renderSidebarProjects();
  renderSidebarSessions();
  updateChatEmptyState();
  updateChatComposerState();
}

function renderProjectOptions() {
  const options = state.projects.length
    ? orderedProjects()
      .map((project) => `<option value="${escapeHtml(project.path)}">${escapeHtml(projectDisplayName(project))}</option>`)
      .join("")
    : `<option value="">No projects</option>`;
  ["#active-project"].forEach((selector) => {
    const select = qs(selector);
    if (!select) return;
    const previous = select.value;
    select.innerHTML = options;
    const desired = state.projects.some((project) => project.path === previous)
      ? previous
      : state.activeProjectPath;
    if (desired) select.value = desired;
  });
  updateMainHeader();
}

function setActiveProject(projectPath) {
  const nextProjectPath = projectPath || "";
  const projectChanged = nextProjectPath !== state.activeProjectPath;
  if (projectChanged) {
    state.fileLoadRequestId += 1;
    state.fileLoading = false;
    state.fileProjectPath = "";
    state.fileEntries = [];
    state.fileExpandedPaths.clear();
    state.fileLoadedDirectoryPaths.clear();
    state.fileLoadingDirectoryPaths.clear();
    state.fileSelectedPaths.clear();
    const openFilePath = qs("#file-editor-path")?.value.trim() || "";
    if (openFilePath && !state.currentFileDirty) {
      closeFileEditor({ skipDirtyCheck: true, focusFiles: false });
    } else if (
      openFilePath
      && state.currentFileProjectPath !== nextProjectPath
      && activeView() === "files"
    ) {
      showToast("Unsaved file remains open from the previous project", "ok");
      updateEditorChrome();
    }
  }
  state.activeProjectPath = nextProjectPath;
  window.localStorage.setItem("iowb.activeProjectPath", state.activeProjectPath);
  ["#active-project"].forEach((selector) => {
    const select = qs(selector);
    if (select) select.value = projectPath;
  });
  updateMainHeader();
  renderSidebarProjects();
  if (qs("#file-editor-path")?.value.trim()) updateEditorChrome();
  updateChatEmptyState();
  updateChatComposerState();
}

function sidebarFilterText() {
  return state.sidebarSearch.trim().toLowerCase();
}

function projectMatchesSidebarSearch(project) {
  const query = sidebarFilterText();
  if (!query) return true;
  return [projectDisplayName(project), project.name, project.path].join(" ").toLowerCase().includes(query);
}

function sessionMatchesSidebarSearch(session) {
  const query = sidebarFilterText();
  if (!query) return true;
  return [
    session.id,
    session.title,
    session.provider,
    session.projectPath,
    session.projectName,
    session.status,
  ].join(" ").toLowerCase().includes(query);
}

function sidebarSessions() {
  if (state.sessions.length) {
    return state.sessions.filter((session) => !isBoardChatSession(session));
  }
  return state.projects.flatMap((project) => (project.sessions || [])
    .filter((session) => !isBoardChatSession(session))
    .map((session) => ({
      ...session,
      projectName: project.name,
      projectPath: session.projectPath || project.path,
    })));
}

function sidebarProjectSessions(project) {
  return (project.sessions || [])
    .filter((session) => session?.id && !isBoardChatSession(session))
    .map((session) => ({
      ...session,
      projectName: project.name,
      projectPath: session.projectPath || project.path,
    }))
    .sort((a, b) => {
      const aDate = new Date(a.lastActivity || a.lastMessageAt || a.updatedAt || a.updated_at || 0).getTime() || 0;
      const bDate = new Date(b.lastActivity || b.lastMessageAt || b.updatedAt || b.updated_at || 0).getTime() || 0;
      return bDate - aDate;
    });
}

function pinnedChatKey(projectPath, sessionId, provider = "") {
  return [projectPath || "", sessionId || "", provider || ""].join("::");
}

function normalizePinnedChatSessions(entries) {
  const seen = new Set();
  return (Array.isArray(entries) ? entries : [])
    .map((entry) => {
      if (typeof entry === "string") {
        if (!entry.includes("::")) {
          return { sessionId: entry.trim(), projectPath: "", provider: "" };
        }
        const [projectPath = "", sessionId = "", provider = ""] = entry.split("::");
        return { projectPath, sessionId, provider };
      }
      if (!entry || typeof entry !== "object") return null;
      return {
        key: entry.key || pinnedChatKey(entry.projectPath, entry.sessionId, entry.provider),
        projectPath: entry.projectPath || "",
        projectName: entry.projectName || "",
        projectDisplayName: entry.projectDisplayName || entry.projectName || "",
        sessionId: entry.sessionId || "",
        title: entry.title || entry.sessionName || "",
        sessionName: entry.sessionName || entry.title || "",
        provider: entry.provider || "",
        pinnedAt: entry.pinnedAt || new Date().toISOString(),
      };
    })
    .filter((entry) => {
      if (!entry?.sessionId) return false;
      entry.key = entry.key || pinnedChatKey(entry.projectPath, entry.sessionId, entry.provider);
      if (seen.has(entry.sessionId)) return false;
      seen.add(entry.sessionId);
      return true;
    });
}

function savePinnedChatSessionsLocal(entries = state.pinnedChatSessions) {
  window.localStorage.setItem(PINNED_CHAT_SESSIONS_KEY, JSON.stringify(normalizePinnedChatSessions(entries)));
}

function sharedPinnedChatSessionsPayload(entries = state.pinnedChatSessions) {
  return normalizePinnedChatSessions(entries).map((entry) => ({
    key: entry.key || pinnedChatKey(entry.projectPath, entry.sessionId, entry.provider),
    sessionId: entry.sessionId,
    projectName: entry.projectName || "",
    projectDisplayName: entry.projectDisplayName || entry.projectName || "",
    projectPath: entry.projectPath || "",
    provider: entry.provider || "",
    sessionName: entry.sessionName || entry.title || "",
    pinnedAt: entry.pinnedAt || new Date().toISOString(),
  }));
}

async function saveSharedPinnedChatSessions(entries = state.pinnedChatSessions) {
  const pinnedSessions = sharedPinnedChatSessionsPayload(entries);
  return api("/api/settings/sidebar-active-sessions", {
    method: "PUT",
    body: JSON.stringify({ pinnedSessions }),
  });
}

async function loadSharedPinnedChatSessions() {
  if (state.pinnedChatSessionsDirty) {
    persistPinnedChatSessions();
    return false;
  }
  const loadGeneration = ++state.pinnedChatSessionsLoadGeneration;
  const localRevision = state.pinnedChatSessionsRevision;
  const localDirty = state.pinnedChatSessionsDirty;
  const localPinned = normalizePinnedChatSessions([
    ...readJsonStorage(PINNED_CHAT_SESSIONS_KEY, []),
    ...state.legacySidebarPinnedChatSessions,
  ]);
  try {
    const response = await api("/api/settings/sidebar-active-sessions");
    const remotePinned = normalizePinnedChatSessions(response?.pinnedSessions || []);
    const remoteInitialized = response?.initialized === true
      || (response?.initialized == null && remotePinned.length > 0);
    if (
      loadGeneration !== state.pinnedChatSessionsLoadGeneration
      || localRevision !== state.pinnedChatSessionsRevision
      || localDirty
      || state.pinnedChatSessionsDirty
    ) {
      return false;
    }
    if (remoteInitialized) {
      state.pinnedChatSessions = remotePinned;
      savePinnedChatSessionsLocal(remotePinned);
      renderSidebarProjects();
      return true;
    }
    state.pinnedChatSessions = localPinned;
    savePinnedChatSessionsLocal(localPinned);
    renderSidebarProjects();
    if (localPinned.length) {
      persistPinnedChatSessions();
    }
    return true;
  } catch (error) {
    console.debug("shared pinned chat load skipped", error);
    if (
      loadGeneration === state.pinnedChatSessionsLoadGeneration
      && localRevision === state.pinnedChatSessionsRevision
      && !state.pinnedChatSessionsDirty
      && localPinned.length
    ) {
      state.pinnedChatSessions = localPinned;
      savePinnedChatSessionsLocal(localPinned);
    }
    return false;
  }
}

function persistPinnedChatSessions() {
  state.pinnedChatSessions = normalizePinnedChatSessions(state.pinnedChatSessions);
  savePinnedChatSessionsLocal(state.pinnedChatSessions);
  const revision = ++state.pinnedChatSessionsRevision;
  state.pinnedChatSessionsDirty = true;
  state.pinnedChatSessionsLoadGeneration += 1;
  window.clearTimeout(state.pinnedChatSessionsPersistTimer);
  state.pinnedChatSessionsPersistTimer = window.setTimeout(() => {
    state.pinnedChatSessionsPersistTimer = null;
    const pinnedSessions = sharedPinnedChatSessionsPayload(state.pinnedChatSessions);
    const save = state.pinnedChatSessionsSaveChain
      .catch(() => {})
      .then(() => saveSharedPinnedChatSessions(pinnedSessions));
    state.pinnedChatSessionsSaveChain = save;
    save.then(() => {
      if (revision === state.pinnedChatSessionsRevision) {
        state.pinnedChatSessionsDirty = false;
      }
    }).catch((error) => {
      console.warn("Unable to sync pinned chat sessions", error);
    });
  }, 300);
}

function sessionProjectPath(session, fallback = "") {
  if (!session) return fallback;
  if (session.projectPath) return session.projectPath;
  if (fallback) return fallback;
  const project = (state.projects || []).find((item) => item.name === session.projectName);
  return project?.path || "";
}

function sessionProvider(session) {
  return String(session?.provider || session?.__provider || "codex").toLowerCase();
}

function nativeSessionId(session) {
  return String(session?.nativeSessionId || session?.native_session_id || "").trim();
}

function shellSingleQuote(value) {
  return `'${String(value || "").replaceAll("'", "'\\''")}'`;
}

function codexResumeCommand(session, projectPath = "") {
  const nativeId = nativeSessionId(session);
  if (!nativeId || sessionProvider(session) !== "codex") return "";
  const path = sessionProjectPath(session, projectPath);
  const resume = `codex resume ${shellSingleQuote(nativeId)}`;
  return path ? `cd ${shellSingleQuote(path)} && ${resume}` : resume;
}

function isChatSessionPinned(session, projectPath = "") {
  const path = sessionProjectPath(session, projectPath);
  const provider = sessionProvider(session);
  const exactKey = pinnedChatKey(path, session?.id, provider);
  return state.pinnedChatSessions.some((entry) => {
    if (entry.key === exactKey) return true;
    return entry.sessionId === session?.id
      && (!entry.projectPath || !path || entry.projectPath === path)
      && (!entry.provider || !provider || entry.provider === provider);
  });
}

function pinnedChatEntries() {
  const entries = [];
  const pinned = normalizePinnedChatSessions(state.pinnedChatSessions);
  for (const pin of pinned) {
    if (state.boardChatSessionIds.has(pin.sessionId)) continue;
    const project = (state.projects || []).find((item) => item.path === pin.projectPath || item.name === pin.projectName);
    const session = findChatSession(pin.sessionId);
    if (isBoardChatSession(session, pin.sessionId)) continue;
    const projectPath = pin.projectPath || sessionProjectPath(session, project?.path || "");
    if (!session) {
      entries.push({
        id: pin.sessionId,
        title: pin.title || pin.sessionName || pin.sessionId,
        provider: pin.provider || "",
        projectPath,
        projectName: pin.projectDisplayName || pin.projectName || project?.name || "",
        pinKey: pin.key || pinnedChatKey(projectPath, pin.sessionId, pin.provider || ""),
        messageCount: 0,
      });
      continue;
    }
    entries.push({
      ...session,
      provider: pin.provider || sessionProvider(session),
      projectPath,
      projectName: pin.projectName || session.projectName || project?.name || "",
      pinKey: pin.key || pinnedChatKey(projectPath, pin.sessionId, pin.provider || sessionProvider(session)),
    });
  }
  return entries;
}

function togglePinnedChatSession(sessionId, projectPath = "", provider = "") {
  const session = findChatSession(sessionId);
  if (!session || isBoardChatSession(session, sessionId)) return;
  const path = sessionProjectPath(session, projectPath);
  const project = (state.projects || []).find((item) => item.path === path || item.name === session.projectName);
  const normalizedProvider = provider || sessionProvider(session);
  const key = pinnedChatKey(path, sessionId, normalizedProvider);
  const existing = state.pinnedChatSessions.findIndex((entry) => entry.key === key || (
    entry.sessionId === sessionId
    && (!entry.projectPath || !path || entry.projectPath === path)
    && (!entry.provider || entry.provider === normalizedProvider)
  ));
  if (existing >= 0) {
    state.pinnedChatSessions.splice(existing, 1);
    showToast("Chat unpinned", "ok");
  } else {
    state.pinnedChatSessions.unshift({
      key,
      sessionId,
      provider: normalizedProvider,
      projectPath: path,
      projectName: session.projectName || project?.name || "",
      pinnedAt: new Date().toISOString(),
    });
    showToast("Chat pinned", "ok");
  }
  hapticFeedback(10);
  persistPinnedChatSessions();
  renderSidebarProjects();
}

function transferPinnedChatSession(sourceSessionId, destination) {
  if (!sourceSessionId || !destination?.id) return;
  let changed = false;
  state.pinnedChatSessions = normalizePinnedChatSessions(state.pinnedChatSessions).map((entry) => {
    if (entry.sessionId !== sourceSessionId) return entry;
    changed = true;
    const projectPath = destination.projectPath || entry.projectPath || "";
    const provider = sessionProvider(destination) || entry.provider || "";
    return {
      ...entry,
      key: pinnedChatKey(projectPath, destination.id, provider),
      sessionId: destination.id,
      projectPath,
      provider,
      sessionName: destination.title || entry.sessionName || "",
    };
  });
  if (changed) persistPinnedChatSessions();
}

function movePinnedChatOrder(sourceKey, targetKey, placement = "before") {
  if (!sourceKey || !targetKey || sourceKey === targetKey) return;
  const pinned = normalizePinnedChatSessions(state.pinnedChatSessions);
  const sourceIndex = pinned.findIndex((entry) => entry.key === sourceKey);
  const targetIndex = pinned.findIndex((entry) => entry.key === targetKey);
  if (sourceIndex < 0 || targetIndex < 0) return;
  const [source] = pinned.splice(sourceIndex, 1);
  const nextTargetIndex = pinned.findIndex((entry) => entry.key === targetKey);
  const insertIndex = nextTargetIndex < 0
    ? pinned.length
    : nextTargetIndex + (placement === "after" ? 1 : 0);
  pinned.splice(insertIndex, 0, source);
  state.pinnedChatSessions = pinned;
  persistPinnedChatSessions();
  hapticFeedback(8);
  renderPinnedSidebarSessions();
}

function clearPinnedChatDragClasses() {
  document.querySelectorAll(".sidebar-history-item.pinned-reorderable.dragging, .sidebar-history-item.pinned-reorderable.drag-over, .sidebar-history-item.pinned-reorderable.drag-over-before, .sidebar-history-item.pinned-reorderable.drag-over-after").forEach((row) => {
    row.classList.remove("dragging", "drag-over", "drag-over-before", "drag-over-after");
  });
}

function finishPinnedChatPointerDrag() {
  const drag = state.pointerPinnedChatDrag;
  if (drag?.dragging && drag.overKey) {
    movePinnedChatOrder(drag.key, drag.overKey, drag.placement || "before");
  }
  if (drag?.dragging) hapticFeedback([8, 20, 8]);
  state.pointerPinnedChatDrag = null;
  document.body.classList.remove("sidebar-pinned-dragging");
  clearPinnedChatDragClasses();
  document.removeEventListener("pointermove", handlePinnedChatPointerMove);
  document.removeEventListener("pointerup", finishPinnedChatPointerDrag);
  document.removeEventListener("pointercancel", finishPinnedChatPointerDrag);
}

function handlePinnedChatPointerMove(event) {
  const drag = state.pointerPinnedChatDrag;
  if (!drag) return;
  const distance = Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY);
  if (!drag.dragging) {
    const heldMs = Date.now() - drag.startedAt;
    if (heldMs < SIDEBAR_DRAG_HOLD_MS || distance < SIDEBAR_DRAG_MOVE_PX) return;
    drag.dragging = true;
    hapticFeedback(12);
  }
  event.preventDefault();
  document.body.classList.add("sidebar-pinned-dragging");
  autoScrollSidebarDuringProjectDrag(event);
  clearPinnedChatDragClasses();
  const sourceRow = document.querySelector(`[data-sidebar-pinned-key="${CSS.escape(drag.key)}"]`);
  sourceRow?.classList.add("dragging");
  const overRow = document.elementFromPoint(event.clientX, event.clientY)?.closest("[data-sidebar-pinned-key]");
  if (!overRow || overRow.dataset.sidebarPinnedKey === drag.key) {
    drag.overKey = "";
    drag.placement = "";
    return;
  }
  const nextOverKey = overRow.dataset.sidebarPinnedKey;
  const nextPlacement = projectDropPlacement(drag.key, nextOverKey, event, overRow);
  overRow.classList.add("drag-over", `drag-over-${nextPlacement}`);
  if (drag.overKey !== nextOverKey || drag.placement !== nextPlacement) hapticFeedback(8);
  drag.overKey = nextOverKey;
  drag.placement = nextPlacement;
}

function bindPinnedSessionReorder(target) {
  target.querySelectorAll("[data-sidebar-pinned-drag-handle]").forEach((handle) => {
    handle.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      const key = handle.dataset.sidebarPinnedDragHandle;
      if (!key || !handle.closest("[data-sidebar-pinned-key]")) return;
      state.pointerPinnedChatDrag = {
        key,
        startX: event.clientX,
        startY: event.clientY,
        startedAt: Date.now(),
        dragging: false,
      };
      hapticFeedback(6);
      event.stopPropagation();
      event.preventDefault();
      document.addEventListener("pointermove", handlePinnedChatPointerMove, { passive: false });
      document.addEventListener("pointerup", finishPinnedChatPointerDrag, { once: true });
      document.addEventListener("pointercancel", finishPinnedChatPointerDrag, { once: true });
    });
    handle.addEventListener("click", (event) => {
      event.stopPropagation();
      event.preventDefault();
    });
    handle.addEventListener("keydown", (event) => {
      if (!["ArrowUp", "ArrowDown"].includes(event.key)) return;
      event.preventDefault();
      event.stopPropagation();
      const key = handle.dataset.sidebarPinnedDragHandle;
      const visibleKeys = [...target.querySelectorAll("[data-sidebar-pinned-drag-handle]")]
        .map((item) => item.dataset.sidebarPinnedDragHandle)
        .filter(Boolean);
      const fromIndex = visibleKeys.indexOf(key);
      if (fromIndex < 0) return;
      const toIndex = event.key === "ArrowUp"
        ? Math.max(0, fromIndex - 1)
        : Math.min(visibleKeys.length - 1, fromIndex + 1);
      if (toIndex === fromIndex) return;
      movePinnedChatOrder(key, visibleKeys[toIndex], toIndex < fromIndex ? "before" : "after");
      hapticFeedback([8, 20, 8]);
      requestAnimationFrame(() => {
        document.querySelector(`[data-sidebar-pinned-drag-handle="${CSS.escape(key)}"]`)?.focus();
      });
    });
  });
}

function clearSidebarProjectDragClasses() {
  document.querySelectorAll(".project-sidebar-row.dragging, .project-sidebar-row.drag-over, .project-sidebar-row.drag-over-before, .project-sidebar-row.drag-over-after").forEach((row) => {
    row.classList.remove("dragging", "drag-over", "drag-over-before", "drag-over-after");
  });
}

function sidebarProjectDragScrollContainer() {
  return qs(".sidebar-context") || qs(".sidebar");
}

function autoScrollSidebarDuringProjectDrag(event) {
  const container = sidebarProjectDragScrollContainer();
  if (!container) return;
  const rect = container.getBoundingClientRect();
  const edge = 54;
  const maxStep = 18;
  if (event.clientY < rect.top + edge) {
    const intensity = 1 - Math.max(0, event.clientY - rect.top) / edge;
    container.scrollTop -= Math.ceil(maxStep * intensity);
  } else if (event.clientY > rect.bottom - edge) {
    const intensity = 1 - Math.max(0, rect.bottom - event.clientY) / edge;
    container.scrollTop += Math.ceil(maxStep * intensity);
  }
}

function finishSidebarProjectPointerDrag() {
  const drag = state.pointerProjectDrag;
  if (drag?.dragging && drag.overPath) {
    moveProjectOrder(drag.path, drag.overPath, drag.placement || "before");
  }
  if (state.pointerProjectDrag?.dragging) {
    state.suppressSidebarProjectClickUntil = Date.now() + 350;
    hapticFeedback([8, 20, 8]);
  }
  state.pointerProjectDrag = null;
  document.body.classList.remove("sidebar-project-dragging");
  clearSidebarProjectDragClasses();
  document.removeEventListener("pointermove", handleSidebarProjectPointerMove);
  document.removeEventListener("pointerup", finishSidebarProjectPointerDrag);
  document.removeEventListener("pointercancel", finishSidebarProjectPointerDrag);
}

function handleSidebarProjectPointerMove(event) {
  const drag = state.pointerProjectDrag;
  if (!drag) return;
  const dx = event.clientX - drag.startX;
  const dy = event.clientY - drag.startY;
  const absDx = Math.abs(dx);
  const absDy = Math.abs(dy);
  if (!drag.dragging) {
    const heldMs = Date.now() - (drag.startedAt || 0);
    const distance = Math.hypot(absDx, absDy);
    if (heldMs < SIDEBAR_DRAG_HOLD_MS || distance < SIDEBAR_DRAG_MOVE_PX) return;
    drag.dragging = true;
    hapticFeedback(12);
  }
  event.preventDefault();
  document.body.classList.add("sidebar-project-dragging");
  autoScrollSidebarDuringProjectDrag(event);
  clearSidebarProjectDragClasses();
  const sourceRow = document.querySelector(`[data-sidebar-project-row="${CSS.escape(drag.path)}"]`);
  sourceRow?.classList.add("dragging");
  const overRow = document.elementFromPoint(event.clientX, event.clientY)?.closest("[data-sidebar-project-row]");
  if (!overRow || overRow.dataset.sidebarProjectRow === drag.path) {
    drag.overPath = "";
    drag.placement = "";
    return;
  }
  const nextOverPath = overRow.dataset.sidebarProjectRow;
  const nextPlacement = projectDropPlacement(drag.path, nextOverPath, event, overRow);
  overRow.classList.add("drag-over", `drag-over-${nextPlacement}`);
  if (drag.overPath !== nextOverPath || drag.placement !== nextPlacement) {
    hapticFeedback(8);
  }
  drag.overPath = nextOverPath;
  drag.placement = nextPlacement;
}

function sidebarProviderLabel(provider) {
  if (provider === "claude") return "Claude";
  if (provider === "gemini") return "Gemini";
  return "Codex";
}

function sidebarProviderIcon(provider) {
  if (provider === "claude") return "/icons/claude-white.svg";
  if (provider === "gemini") return "/icons/gemini-ai-icon.svg";
  return "/icons/codex-white.svg";
}

function normalizeSidebarSessionStatus(status) {
  const value = String(status || "").toLowerCase();
  if (["starting", "running", "waiting-for-input", "processing", "pending"].includes(value)) return "running";
  if (["completed", "complete", "done", "success"].includes(value)) return "completed";
  if (["failed", "aborted", "cancelled", "canceled", "error"].includes(value)) return "failed";
  return "";
}

function sidebarSessionStatus(session) {
  const live = state.sessionStatusById?.[session.id];
  return normalizeSidebarSessionStatus(live?.status || session.status || (session.pending ? "pending" : ""));
}

function rememberSidebarSessionStatus(payload = {}) {
  const sessionId = payload.sessionId || payload.id || "";
  if (!sessionId) return;
  const status = normalizeSidebarSessionStatus(payload.status);
  if (!status) return;
  if (state.boardChatSessionIds.has(sessionId) && !isActiveChatSessionEvent({ sessionId })) {
    return;
  }
  state.sessionStatusById[sessionId] = {
    status,
    provider: payload.provider || "",
    updatedAt: new Date().toISOString(),
  };
  if (state.boardChatSessionIds.has(sessionId)) return;
  renderSidebarProjects();
  renderPinnedSidebarSessions();
}

function sidebarSessionCardHtml(session, options = {}) {
  const title = session.title || session.summary || session.id;
  const projectPath = options.projectPath || session.projectPath || "";
  const cli = (session.provider || "codex").toLowerCase();
  const cliLabel = sidebarProviderLabel(cli);
  const cliIcon = sidebarProviderIcon(cli);
  const isActive = options.active ?? session.id === state.chatSessionId;
  const pinned = options.pinned ?? isChatSessionPinned(session, projectPath);
  const reorderable = Boolean(options.reorderable);
  const pinKey = options.pinKey || session.pinKey || pinnedChatKey(projectPath, session.id, cli);
  const showProject = Boolean(options.showProject);
  const lastActivity = session.lastActivity || session.updatedAt || session.createdAt;
  const relative = formatRelativeTime(lastActivity);
  const messageCount = Number(session.messageCount || 0);
  const messageCountLabel = session.external ? "history" : String(messageCount);
  const messageCountTitle = session.external ? "External CLI history" : `${messageCount} messages`;
  const displayTokens = formatSessionDisplayTokenUsage(session);
  const displayTokensTitle = sessionDisplayTokenUsageTitle(session);
  const pending = session.pending ? "true" : "false";
  const status = sidebarSessionStatus(session);
  const statusLabel = status === "running" ? "Running" : status === "completed" ? "Completed" : status === "failed" ? "Failed" : "";
  const projectLabel = showProject
    ? (options.projectName || session.projectName || projectPath || "")
    : "";
  return `<article class="sidebar-history-item${isActive ? " active" : ""}${pinned ? " pinned" : ""}${reorderable ? " pinned-reorderable" : ""}${status ? ` is-${escapeHtml(status)}` : ""}" data-sidebar-session-card="${escapeHtml(session.id)}" data-sidebar-card-provider="${escapeHtml(cli)}" data-sidebar-card-project-path="${escapeHtml(projectPath)}"${reorderable ? ` data-sidebar-pinned-key="${escapeHtml(pinKey)}"` : ""} data-pending="${pending}" data-status="${escapeHtml(status)}">
    <button type="button" class="sidebar-history-main" data-sidebar-session="${escapeHtml(session.id)}" data-sidebar-provider="${escapeHtml(cli)}" data-sidebar-project-path="${escapeHtml(projectPath)}" data-pending="${pending}">
      <div class="session-title-row">
        ${status ? `<span class="session-status-icon" aria-label="${escapeHtml(statusLabel)}" title="${escapeHtml(statusLabel)}"></span>` : ""}
        <div class="session-title">${escapeHtml(title)}</div>
      </div>
      ${projectLabel ? `<div class="session-project">${escapeHtml(projectLabel)}</div>` : ""}
      <div class="session-bottom">
        <span class="meta-time">${escapeHtml(relative || "never")}</span>
        <span class="meta-right">
          <span class="meta-count" title="${escapeHtml(messageCountTitle)}">${escapeHtml(messageCountLabel)}</span>
          ${displayTokens ? `<span class="meta-count" title="${escapeHtml(displayTokensTitle)}">${escapeHtml(displayTokens)}</span>` : ""}
          <span class="cli-badge ${escapeHtml(cli)}" aria-label="${escapeHtml(cliLabel)}" title="${escapeHtml(cliLabel)}">
            <img src="${escapeHtml(cliIcon)}" alt="" aria-hidden="true" loading="lazy" decoding="async" />
          </span>
        </span>
      </div>
    </button>
    ${reorderable ? `<button type="button" class="pinned-session-drag-handle" data-sidebar-pinned-drag-handle="${escapeHtml(pinKey)}" aria-label="Drag to reorder ${escapeHtml(title)}" title="Drag to reorder" aria-keyshortcuts="ArrowUp ArrowDown">
      <i></i><i></i><i></i><i></i><i></i><i></i>
    </button>` : ""}
    <button type="button" class="session-pin icon-button${pinned ? " active" : ""}" data-sidebar-session-pin="${escapeHtml(session.id)}" data-sidebar-project-path="${escapeHtml(projectPath)}" data-sidebar-provider="${escapeHtml(cli)}" aria-label="${pinned ? "Unpin chat session" : "Pin chat session"}" title="${pinned ? "Unpin chat session" : "Pin chat session"}" data-symbol="pin"></button>
    ${reorderable ? "" : `<button type="button" class="session-delete icon-button" data-sidebar-session-delete="${escapeHtml(session.id)}" data-sidebar-project-path="${escapeHtml(projectPath)}" aria-label="Delete chat session" title="Delete chat session" data-symbol="trash"></button>`}
  </article>`;
}

function findChatSession(sessionId) {
  if (!sessionId) return null;
  for (const project of state.projects || []) {
    const session = (project.sessions || []).find((item) => item.id === sessionId);
    if (session) return session;
  }
  return (state.sessions || []).find((item) => item.id === sessionId) || null;
}

function mergeSessionIntoProjects(session) {
  if (!session?.id || isBoardChatSession(session)) return;
  state.projects = (state.projects || []).map((project) => {
    const matchesProject = session.projectPath && project.path === session.projectPath;
    const hasSession = (project.sessions || []).some((item) => item.id === session.id);
    if (!matchesProject && !hasSession) return project;
    const sessions = (project.sessions || [])
      .filter((item) => item.id !== session.id)
      .concat({ ...session, projectPath: session.projectPath || project.path });
    return { ...project, sessions };
  });
}

function removeChatSessionFromState(sessionId) {
  state.sessions = (state.sessions || []).filter((session) => session.id !== sessionId);
  for (const project of state.projects || []) {
    if (Array.isArray(project.sessions)) {
      project.sessions = project.sessions.filter((session) => session.id !== sessionId);
    }
  }
  state.chatTranscriptCache.entries = chatCacheEntries().filter((entry) => entry.sessionId !== sessionId);
  persistChatTranscriptCache();
}

function hideChatSessionFromLists(sessionId) {
  state.sessions = (state.sessions || []).filter((session) => session.id !== sessionId);
  for (const project of state.projects || []) {
    if (Array.isArray(project.sessions)) {
      project.sessions = project.sessions.filter((session) => session.id !== sessionId);
    }
  }
}

function isBoardChatSession(session, sessionId = session?.id) {
  const id = String(sessionId || "").trim();
  return Boolean(
    session?.boardSession
    || session?.board_session
    || session?.boardId
    || session?.board_id
    || session?.boardRunId
    || session?.board_run_id
    || session?.boardTaskId
    || session?.board_task_id,
  ) || Boolean(id && state.boardChatSessionIds.has(id));
}

function isSelectedBoardChatSession(sessionId = state.chatSessionId) {
  return isBoardChatSession(
    findChatSession(sessionId) || cachedChatSession(sessionId)?.session,
    sessionId,
  );
}

function hideBoardChatSessionsFromLists() {
  for (const session of state.sessions || []) {
    if (isBoardChatSession(session) && session?.id) state.boardChatSessionIds.add(session.id);
  }
  for (const project of state.projects || []) {
    for (const session of project.sessions || []) {
      if (isBoardChatSession(session) && session?.id) state.boardChatSessionIds.add(session.id);
    }
  }
  state.sessions = (state.sessions || []).filter((session) => !isBoardChatSession(session));
  for (const project of state.projects || []) {
    if (Array.isArray(project.sessions)) {
      project.sessions = project.sessions.filter((session) => !isBoardChatSession(session));
    }
  }
}

function deleteSessionOverride(sessionId) {
  const all = readSessionOverrides();
  if (!Object.prototype.hasOwnProperty.call(all, sessionId)) return;
  delete all[sessionId];
  writeSessionOverrides(all);
}

function clearSelectedChatSession(sessionId) {
  if (state.chatSessionId !== sessionId && state.pendingChatSessionId !== sessionId) return;
  closeChatEditFromHerePicker();
  state.chatSessionId = "";
  state.pendingChatSessionId = "";
  state.chatPromptDraftSessionId = "";
  state.chatEditFromHere.staged = null;
  state.currentSession = null;
  if (state.preferences.lastChatSessionId === sessionId) {
    state.preferences.lastChatSessionId = "";
    savePreferences();
  }
  clearActiveChatSelection(sessionId);
  resetChatOutputDom();
  const prompt = qs("#chat-prompt");
  if (prompt) {
    setChatPromptValue("");
    noteChatPromptUserEdit("");
    autosizeChatPrompt();
  }
}

async function deleteChatSession(sessionId, projectPath = "", button = null) {
  if (state.chatEditFromHere.submitting) {
    showToast("Wait for the replacement chat to finish creating", "ok");
    return;
  }
  const id = (sessionId || "").trim();
  if (!id) return;
  const session = findChatSession(id);
  const pending = Boolean(session?.pending);
  const title = session?.title || session?.summary || id;
  if (!pending && !window.confirm(`Delete chat session "${title}"?`)) return;

  const removeLocal = () => {
    removeChatSessionFromState(id);
    state.pinnedChatSessions = state.pinnedChatSessions.filter((entry) => entry.sessionId !== id);
    persistPinnedChatSessions();
    deleteSessionOverride(id);
    clearSelectedChatSession(id);
    renderProjects();
    renderSidebarProjects();
    renderSidebarSessions();
    updateChatEmptyState();
  };

  if (pending) {
    removeLocal();
    showToast("Chat draft deleted", "ok");
    return;
  }

  await withButtonLoading(button, () => api(`/api/sessions/${encodeURIComponent(id)}`, {
    method: "DELETE",
  }));
  removeLocal();
  await loadProjects().catch(() => {});
  showToast("Chat session deleted", "ok");
}

function sessionContextMenuHtml(session, projectPath = "") {
  const id = session?.id || "";
  const path = sessionProjectPath(session, projectPath);
  const provider = sessionProvider(session);
  const pinned = isChatSessionPinned(session, path);
  const resumeCommand = codexResumeCommand(session, path);
  return `<div id="session-context-menu" class="file-context-menu session-context-menu" role="menu" aria-label="Session actions">
    <button type="button" data-symbol="open" data-session-context-action="open" data-session-context-id="${escapeHtml(id)}" data-session-context-project-path="${escapeHtml(path)}">Open Chat</button>
    ${resumeCommand ? `<button type="button" data-symbol="copy" data-session-context-action="copy-codex-resume" data-session-context-id="${escapeHtml(id)}" data-session-context-project-path="${escapeHtml(path)}">Copy Codex Resume Command</button>` : ""}
    <button type="button" data-symbol="copy" data-session-context-action="copy-session-id" data-session-context-id="${escapeHtml(id)}">Copy Workbench Session ID</button>
    <hr />
    <button type="button" data-symbol="pin" data-session-context-action="toggle-pin" data-session-context-id="${escapeHtml(id)}" data-session-context-provider="${escapeHtml(provider)}" data-session-context-project-path="${escapeHtml(path)}">${pinned ? "Unpin Chat" : "Pin Chat"}</button>
    ${session?.pending ? "" : `<button type="button" class="danger" data-symbol="trash" data-session-context-action="delete" data-session-context-id="${escapeHtml(id)}" data-session-context-project-path="${escapeHtml(path)}">Delete Chat</button>`}
  </div>`;
}

function closeSessionContextMenu() {
  state.sessionContextMenu = null;
  qs("#session-context-menu")?.remove();
}

function openSessionContextMenu(session, projectPath, x, y) {
  if (!session?.id || isBoardChatSession(session)) return;
  closeFileContextMenu();
  closeSessionContextMenu();
  state.sessionContextMenu = { sessionId: session.id };
  document.body.insertAdjacentHTML("beforeend", sessionContextMenuHtml(session, projectPath));
  const menu = qs("#session-context-menu");
  const left = Math.min(Math.max(8, x), window.innerWidth - menu.offsetWidth - 8);
  const top = Math.min(Math.max(8, y), window.innerHeight - menu.offsetHeight - 8);
  menu.style.left = `${Math.round(left)}px`;
  menu.style.top = `${Math.round(top)}px`;
  menu.querySelectorAll("[data-session-context-action]").forEach((button) => {
    button.addEventListener("click", () => {
      handleSessionContextAction(
        button.dataset.sessionContextAction,
        button.dataset.sessionContextId,
        button.dataset.sessionContextProjectPath || projectPath || "",
        button.dataset.sessionContextProvider || sessionProvider(session),
      ).catch(showError);
    });
  });
}

function openSessionContextMenuFromRow(row, x, y) {
  const sessionId = row?.dataset.sidebarSessionCard || "";
  const session = findChatSession(sessionId);
  if (!session) return;
  openSessionContextMenu(session, row.dataset.sidebarCardProjectPath || session.projectPath || "", x, y);
}

async function handleSessionContextAction(action, sessionId, projectPath = "", provider = "") {
  const session = findChatSession(sessionId);
  closeSessionContextMenu();
  if (!session && !["copy-session-id"].includes(action)) return;
  if (action === "open") {
    if (provider) setChatProvider(provider);
    await pickChatSession(sessionId, projectPath);
  } else if (action === "copy-codex-resume") {
    const command = codexResumeCommand(session, projectPath);
    if (!command) return;
    await copyText(command);
    showToast("Codex resume command copied", "ok");
  } else if (action === "copy-session-id") {
    await copyText(sessionId);
    showToast("Workbench session ID copied", "ok");
  } else if (action === "toggle-pin") {
    togglePinnedChatSession(sessionId, projectPath, provider);
  } else if (action === "delete") {
    await deleteChatSession(sessionId, projectPath).catch(showError);
  }
}

function updatePendingChatProvider(provider) {
  const value = CHAT_PROVIDERS_LOCAL.includes(provider) ? provider : "codex";
  if (!state.pendingChatSessionId) return;
  const pending = findChatSession(state.pendingChatSessionId);
  if (!pending?.pending) return;
  pending.provider = value;
  saveSessionOverrides(state.pendingChatSessionId, {
    cli: value,
    model: chatModelValue(),
    effort: chatEffortValue(),
    mode: chatModeValue(),
    thinking: chatThinkingValue(),
    fast: chatFastValue(),
  });
  renderSidebarProjects();
}

function renderChatProviderPicker() {
  const selected = chatCliValue();
  document.querySelectorAll("[data-chat-provider-option]").forEach((button) => {
    const value = button.dataset.chatProviderOption;
    const active = value === selected;
    button.classList.toggle("active", active);
    button.setAttribute("aria-pressed", active ? "true" : "false");
  });
  document.querySelectorAll(".chat-config-provider-picker").forEach((picker) => {
    picker.classList.toggle("hidden", !selectedChatIsFreshDraft());
  });
  updateChatFastControl();
}

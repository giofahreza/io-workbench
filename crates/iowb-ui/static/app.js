const TOKEN_STORAGE_KEY = "iowb.token";
window.localStorage.removeItem(TOKEN_STORAGE_KEY);

const APP_VERSION = "20260729-03";
const SIDEBAR_STATE_SETTING_KEY = "iowb.web.sidebar";
const SIDEBAR_STATE_UPDATED_KEY = "iowb.sidebarStateUpdatedAt";
const PINNED_CHAT_SESSIONS_KEY = "iowb.pinnedChatSessions";
const APP_VERSION_STORAGE_KEY = "iowb.web.version";
const APP_RELOAD_STORAGE_KEY = `iowb.web.reloaded.${APP_VERSION}`;
const WS_CONNECT_TIMEOUT_MS = 8000;
const WS_RETRY_BASE_MS = 1200;
const WS_RETRY_MAX_MS = 10000;

// Minimum time the user must hold the project grip before movement becomes a
// drag-to-reorder gesture. Only the grip starts dragging; the rest of the row
// stays available for normal click and vertical sidebar scroll.
const SIDEBAR_DRAG_HOLD_MS = 160;
const SIDEBAR_DRAG_MOVE_PX = 5;
const CHAT_SWIPE_MIN_DISTANCE = 72;
const CHAT_SWIPE_MAX_VERTICAL_DRIFT = 64;
const CHAT_SWIPE_DIRECTION_RATIO = 1.5;

const CHAT_PROVIDERS = new Set(["codex", "claude", "cursor", "gemini"]);

function readJsonStorage(key, fallback) {
  try {
    const raw = window.localStorage.getItem(key);
    return raw ? JSON.parse(raw) : fallback;
  } catch {
    window.localStorage.removeItem(key);
    return fallback;
  }
}

const state = {
  auth: null,
  projects: [],
  sessions: [],
  settings: null,
  metrics: null,
  dbConnections: [],
  selectedDbConnection: null,
  selectedDbTargetConnection: null,
  selectedDbObject: null,
  editingDbConnection: null,
  dbExplorerNodes: [],
  dbDiagram: {
    zoom: 1,
    query: "",
  },
  lastDbObjectDetails: null,
  lastToolRuns: null,
  lastSettingsRows: null,
  notificationPreferences: null,
  gitStatus: null,
  gitBranches: null,
  gitCommits: null,
  currentGitDiffFile: null,
  currentConflictFile: null,
  gitSelectedFiles: new Set(),
  gitCollapsedFolders: new Set(),
  gitCommitMessage: "",
  fileEntries: [],
  fileExpandedPaths: new Set(),
  fileSelectedPaths: new Set(),
  fileCreating: null,
  fileRenamingPath: "",
  fileContextMenu: null,
  fileUploadTargetPath: "",
  fileViewMode: window.localStorage.getItem("iowb.fileViewMode") || "detailed",
  editorSearch: {
    query: "",
    matches: [],
    current: -1,
  },
  currentFileDirty: false,
  chatBuffer: "",
  chatProcessing: null,
  lastSessionMessages: [],
  shellBuffer: "",
  virtualLists: {},
  shellFitTimer: null,
  floatingNavSyncRaf: null,
  floatingNavSettleTimer: null,
  shellStarting: false,
  currentShellProjectPath: "",
  shellAutoStartedProjectPath: "",
  shellLastResizeSignature: "",
  shellProcessListOpen: false,
  shellCtrlActive: false,
  shellAltActive: false,
  shellTouchScrollY: null,
  shellTouchScrollRemainder: 0,
  preferences: (() => {
    const parsed = readJsonStorage("iowb.webPreferences", {});
    if (!parsed.chatSessionOverrides) parsed.chatSessionOverrides = {};
    return parsed;
  })(),
  projectOrder: readJsonStorage("iowb.projectOrder", []),
  projectMeta: readJsonStorage("iowb.projectMeta", {}),
  pinnedChatSessions: readJsonStorage(PINNED_CHAT_SESSIONS_KEY, []),
  activeProjectPath: window.localStorage.getItem("iowb.activeProjectPath") || "",
  expandedProjectPaths: new Set(readJsonStorage("iowb.expandedProjects", [])),
  limits: {
    files: 250,
    sessions: 100,
    sessionMessages: 80,
    gitFiles: 150,
    dbConnections: 80,
    toolRuns: 50,
    settingsRows: 80,
  },
  token: window.sessionStorage.getItem(TOKEN_STORAGE_KEY) || "",
  ws: null,
  wsRetry: null,
  wsRetryAttempt: 0,
  wsConnectTimer: null,
  wsGeneration: 0,
  wsLastDetail: "",
  currentSession: null,
  pendingChatSessionId: "",
  currentShellProcess: null,
  gitActiveView: "changes",
  chatImages: [],
  codeEditor: null,
  suppressEditorChange: false,
  shellTerm: null,
  sidebarSearch: "",
  openProjectMenuPath: "",
  pointerProjectDrag: null,
  chatSwipe: null,
  activeSettingsTab: window.localStorage.getItem("iowb.settingsTab") || "agents",
  suppressSidebarProjectClickUntil: 0,
  sidebarStatePersistTimer: null,
  commandPalette: {
    open: false,
    query: "",
    selectedIndex: 0,
  },
  folderBrowser: {
    open: false,
    action: "select",
    targetInput: "",
    path: "~",
    homePath: "",
    entries: [],
    filter: "",
    showHidden: false,
    loading: false,
  },
};

const qs = (selector) => document.querySelector(selector);

const AUTH_PROTECTED_SELECTORS = [
  ".sidebar",
  ".topbar",
  ".main-content-header",
  ".workspace > .view",
  "#command-palette",
  "#folder-browser",
  ".bottom-nav",
];

const authProtectedSlots = [];

function collectAuthProtectedSlots() {
  if (authProtectedSlots.length) return;
  const seen = new Set();
  for (const selector of AUTH_PROTECTED_SELECTORS) {
    document.querySelectorAll(selector).forEach((node) => {
      if (!(node instanceof HTMLElement) || seen.has(node)) return;
      seen.add(node);
      authProtectedSlots.push({
        node,
        marker: document.createComment(`auth-protected:${node.id || selector}`),
      });
    });
  }
}

function detachAuthProtectedShell() {
  collectAuthProtectedSlots();
  document.body.classList.add("auth-active");
  document.body.classList.remove("auth-pending", "sidebar-open");
  document.body.classList.remove(
    "code-editor-active",
    "xterm-active",
    "files-editor-open",
    "sidebar-project-dragging",
  );
  closeCommandPalette();
  closeFolderBrowser();
  authProtectedSlots.forEach(({ node, marker }) => {
    node.setAttribute("aria-hidden", "true");
    node.setAttribute("inert", "");
    if (!node.isConnected) return;
    node.parentNode.insertBefore(marker, node);
    node.remove();
  });
}

function attachAuthProtectedShell() {
  collectAuthProtectedSlots();
  authProtectedSlots.forEach(({ node, marker }) => {
    node.removeAttribute("aria-hidden");
    node.removeAttribute("inert");
    if (!node.isConnected && marker.parentNode) {
      marker.replaceWith(node);
    }
  });
  if (state.codeEditor) document.body.classList.add("code-editor-active");
  if (state.shellTerm) document.body.classList.add("xterm-active");
  document.body.classList.remove("auth-active", "auth-pending");
}

const VIEW_NAMES = {
  files: "Project Files",
  chat: "Chat",
  shell: "Shell",
  git: "Git",
  database: "Database",
  settings: "Settings",
};

const VIEW_SUBTITLES = {
  files: "Browse, edit, upload, and organize project files.",
  chat: "Start or resume agent sessions in the selected project.",
  shell: "Run a PTY-backed terminal in the selected project.",
  git: "Review changes, resolve conflicts, and commit selected work.",
  database: "Manage connections, explore schemas, and run SQL.",
  settings: "Configure credentials, notifications, Direct AI, and server state.",
};

async function api(path, options = {}) {
  const headers = {
    "Content-Type": "application/json",
    ...options.headers,
  };
  if (state.token) {
    headers.Authorization = `Bearer ${state.token}`;
  }

  const response = await fetch(path, { ...options, headers });
  const text = await response.text();
  const body = text ? JSON.parse(text) : null;
  if (!response.ok) {
    if (response.status === 401) {
      showAuthPanel(authPanelMode());
    }
    throw new Error(body?.details || body?.error || response.statusText);
  }
  return body;
}

async function apiUpload(path, formData) {
  const headers = {};
  if (state.token) {
    headers.Authorization = `Bearer ${state.token}`;
  }
  const response = await fetch(path, {
    method: "POST",
    headers,
    body: formData,
  });
  const text = await response.text();
  const body = text ? JSON.parse(text) : null;
  if (!response.ok) {
    throw new Error(body?.details || body?.error || response.statusText);
  }
  return body;
}

async function withButtonLoading(button, task) {
  const target = typeof button === "string" ? qs(button) : button;
  if (!target) return task();
  target.classList.add("is-loading");
  target.disabled = true;
  try {
    return await task();
  } finally {
    target.classList.remove("is-loading");
    target.disabled = false;
  }
}

function activeProjectPath(selector = "#active-project") {
  const selected = qs(selector)?.value || state.activeProjectPath || state.projects[0]?.path || "";
  if (!selected) return "";
  return state.projects.some((project) => project.path === selected) ? selected : state.projects[0]?.path || "";
}

function activeProjectName(selectId = "#active-project") {
  const path = qs(selectId)?.value || activeProjectPath(selectId);
  return state.projects.find((project) => project.path === path)?.name || "";
}

function chatProvider() {
  // The composer no longer exposes a CLI/Thinking picker — those values are
  // driven by the stored preference and the sidebar provider buttons.
  const setting = qs("#chat-provider-setting")?.value;
  if (setting && CHAT_PROVIDERS.has(setting)) return setting;
  const value = state.preferences.chatProvider || "codex";
  return CHAT_PROVIDERS.has(value) ? value : "codex";
}

function canLoadProtectedData() {
  return Boolean(state.auth && (!state.auth.enabled || state.auth.isAuthenticated || state.token));
}

function setChatProvider(provider) {
  const value = CHAT_PROVIDERS.has(provider) ? provider : "codex";
  state.preferences.chatProvider = value;
  state.preferences.chatCli = value;
  state.preferences.chatModel = "";
  const settingSelect = qs("#chat-provider-setting");
  if (settingSelect) settingSelect.value = value;
  const modelSelect = qs("#chat-model");
  if (modelSelect) {
    modelSelect.disabled = true;
    modelSelect.innerHTML = canLoadProtectedData()
      ? `<option value="">Loading models...</option>`
      : `<option value="">Sign in to load models</option>`;
  }
  savePreferences();
  renderChatProviderPicker();
  if (canLoadProtectedData()) loadChatModelsIntoSelect(value).catch(() => {});
}

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
    pinnedChatSessions: state.pinnedChatSessions,
    updatedAt: new Date().toISOString(),
  };
}

function saveSidebarStateLocal(payload = sidebarStatePayload()) {
  window.localStorage.setItem("iowb.projectOrder", JSON.stringify(payload.projectOrder || []));
  window.localStorage.setItem("iowb.projectMeta", JSON.stringify(payload.projectMeta || {}));
  window.localStorage.setItem("iowb.expandedProjects", JSON.stringify(payload.expandedProjectPaths || []));
  window.localStorage.setItem(PINNED_CHAT_SESSIONS_KEY, JSON.stringify(payload.pinnedChatSessions || []));
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
    state.pinnedChatSessions = normalizePinnedChatSessions(payload.pinnedChatSessions);
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
    if (previous) select.value = previous;
  });
  updateMainHeader();
}

function setActiveProject(projectPath) {
  state.activeProjectPath = projectPath || "";
  window.localStorage.setItem("iowb.activeProjectPath", state.activeProjectPath);
  ["#active-project"].forEach((selector) => {
    const select = qs(selector);
    if (select) select.value = projectPath;
  });
  updateMainHeader();
  renderSidebarProjects();
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
  if (state.sessions.length) return state.sessions;
  return state.projects.flatMap((project) => (project.sessions || []).map((session) => ({
    ...session,
    projectName: project.name,
    projectPath: session.projectPath || project.path,
  })));
}

function sidebarProjectSessions(project) {
  const byId = new Map();
  for (const session of project.sessions || []) {
    if (!session?.id) continue;
    byId.set(session.id, {
      ...session,
      projectName: project.name,
      projectPath: session.projectPath || project.path,
    });
  }
  for (const session of state.sessions || []) {
    const path = session.projectPath || "";
    const matchesPath = path && path === project.path;
    const matchesName = session.projectName && session.projectName === project.name;
    if (!session?.id || (!matchesPath && !matchesName)) continue;
    byId.set(session.id, {
      ...session,
      projectName: session.projectName || project.name,
      projectPath: session.projectPath || project.path,
    });
  }
  return [...byId.values()].sort((a, b) => {
    const aDate = new Date(a.updatedAt || a.updated_at || a.timestamp || 0).getTime() || 0;
    const bDate = new Date(b.updatedAt || b.updated_at || b.timestamp || 0).getTime() || 0;
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
        const [projectPath = "", sessionId = "", provider = ""] = entry.split("::");
        return { projectPath, sessionId, provider };
      }
      if (!entry || typeof entry !== "object") return null;
      return {
        key: entry.key || pinnedChatKey(entry.projectPath, entry.sessionId, entry.provider),
        projectPath: entry.projectPath || "",
        projectName: entry.projectName || "",
        sessionId: entry.sessionId || "",
        provider: entry.provider || "",
        pinnedAt: entry.pinnedAt || new Date().toISOString(),
      };
    })
    .filter((entry) => {
      if (!entry?.sessionId) return false;
      entry.key = entry.key || pinnedChatKey(entry.projectPath, entry.sessionId, entry.provider);
      if (seen.has(entry.key)) return false;
      seen.add(entry.key);
      return true;
    });
}

function persistPinnedChatSessions() {
  state.pinnedChatSessions = normalizePinnedChatSessions(state.pinnedChatSessions);
  persistSidebarState();
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
    const project = (state.projects || []).find((item) => item.path === pin.projectPath || item.name === pin.projectName);
    const session = findChatSession(pin.sessionId);
    if (!session) continue;
    const projectPath = pin.projectPath || sessionProjectPath(session, project?.path || "");
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
  if (!session) return;
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
  if (provider === "cursor") return "Cursor";
  return "Codex";
}

function sidebarProviderIcon(provider) {
  if (provider === "claude") return "/icons/claude-white.svg";
  if (provider === "gemini") return "/icons/gemini-ai-icon.svg";
  if (provider === "cursor") return "/icons/cursor-white.svg";
  return "/icons/codex-white.svg";
}

function sidebarSessionCardHtml(session, options = {}) {
  const title = session.title || session.summary || session.id;
  const projectPath = options.projectPath || session.projectPath || "";
  const cli = (session.provider || "codex").toLowerCase();
  const cliLabel = sidebarProviderLabel(cli);
  const cliIcon = sidebarProviderIcon(cli);
  const isActive = options.active ?? session.id === state.chatSessionId;
  const pinned = options.pinned ?? isChatSessionPinned(session, projectPath);
  const showProject = Boolean(options.showProject);
  const lastActivity = session.lastActivity || session.updatedAt || session.createdAt;
  const relative = formatRelativeTime(lastActivity);
  const messageCount = Number(session.messageCount || 0);
  const pending = session.pending ? "true" : "false";
  const projectLabel = showProject
    ? (options.projectName || session.projectName || projectPath || "")
    : "";
  return `<article class="sidebar-history-item${isActive ? " active" : ""}${pinned ? " pinned" : ""}" data-sidebar-session-card="${escapeHtml(session.id)}" data-pending="${pending}">
    <button type="button" class="sidebar-history-main" data-sidebar-session="${escapeHtml(session.id)}" data-sidebar-provider="${escapeHtml(cli)}" data-sidebar-project-path="${escapeHtml(projectPath)}" data-pending="${pending}">
      <div class="session-title">${escapeHtml(title)}</div>
      ${projectLabel ? `<div class="session-project">${escapeHtml(projectLabel)}</div>` : ""}
      <div class="session-bottom">
        <span class="meta-time">${escapeHtml(relative || "never")}</span>
        <span class="meta-right">
          <span class="meta-count" title="${messageCount} messages">${messageCount}</span>
          <span class="cli-badge ${escapeHtml(cli)}" aria-label="${escapeHtml(cliLabel)}" title="${escapeHtml(cliLabel)}">
            <img src="${escapeHtml(cliIcon)}" alt="" aria-hidden="true" loading="lazy" decoding="async" />
          </span>
        </span>
      </div>
    </button>
    <button type="button" class="session-pin icon-button${pinned ? " active" : ""}" data-sidebar-session-pin="${escapeHtml(session.id)}" data-sidebar-project-path="${escapeHtml(projectPath)}" data-sidebar-provider="${escapeHtml(cli)}" aria-label="${pinned ? "Unpin chat session" : "Pin chat session"}" title="${pinned ? "Unpin chat session" : "Pin chat session"}" data-symbol="pin"></button>
    <button type="button" class="session-delete icon-button" data-sidebar-session-delete="${escapeHtml(session.id)}" data-sidebar-project-path="${escapeHtml(projectPath)}" aria-label="Delete chat session" title="Delete chat session" data-symbol="trash"></button>
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

function removeChatSessionFromState(sessionId) {
  state.sessions = (state.sessions || []).filter((session) => session.id !== sessionId);
  for (const project of state.projects || []) {
    if (Array.isArray(project.sessions)) {
      project.sessions = project.sessions.filter((session) => session.id !== sessionId);
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
  state.chatSessionId = "";
  state.pendingChatSessionId = "";
  state.currentSession = null;
  if (state.preferences.lastChatSessionId === sessionId) {
    state.preferences.lastChatSessionId = "";
    savePreferences();
  }
  resetChatOutputDom();
  const prompt = qs("#chat-prompt");
  if (prompt) {
    prompt.value = "";
    autosizeChatPrompt();
  }
}

async function deleteChatSession(sessionId, projectPath = "", button = null) {
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
  });
  renderSidebarProjects();
}

function renderChatProviderPicker() {
  const selected = chatCliValue();
  document.querySelectorAll("[data-chat-provider-option]").forEach((button) => {
    const active = button.dataset.chatProviderOption === selected;
    button.classList.toggle("active", active);
    button.setAttribute("aria-pressed", active ? "true" : "false");
  });
}

function chatOutputIsEmpty() {
  const output = chatOutputRoot();
  if (!output) return true;
  return output.children.length === 0 && !output.textContent.trim();
}

function selectedChatIsFreshDraft() {
  if (!state.chatSessionId) return true;
  const session = findChatSession(state.chatSessionId);
  return Boolean(session?.pending || state.pendingChatSessionId === state.chatSessionId);
}

function chatSessionIdForSubmit() {
  const pendingId = state.pendingChatSessionId;
  const pending = pendingId ? findChatSession(pendingId) : null;
  if (pending?.pending) return pendingId;
  return state.chatSessionId || "";
}

function updateChatEmptyState() {
  const emptyState = qs("#chat-empty-state");
  if (!emptyState) return;
  const shouldShow = activeView() === "chat" && chatOutputIsEmpty() && selectedChatIsFreshDraft();
  emptyState.classList.toggle("hidden", !shouldShow);
  renderChatProviderPicker();
}

function chooseNewChatProvider(provider) {
  if (!CHAT_PROVIDERS_LOCAL.includes(provider)) return;
  setChatProvider(provider);
  updatePendingChatProvider(provider);
  renderChatProviderPicker();
  qs("#chat-prompt")?.focus();
}

function bindSidebarSessionActions(target) {
  target.querySelectorAll("[data-sidebar-session]").forEach((button) => {
    button.addEventListener("click", async (event) => {
      event.stopPropagation();
      event.preventDefault();
      hapticFeedback(6);
      if (button.dataset.sidebarProvider) setChatProvider(button.dataset.sidebarProvider);
      await pickChatSession(
        button.dataset.sidebarSession || "",
        button.dataset.sidebarProjectPath || "",
      );
      renderSidebarSessions();
    });
  });
  target.querySelectorAll("[data-sidebar-session-delete]").forEach((button) => {
    button.addEventListener("click", async (event) => {
      event.stopPropagation();
      event.preventDefault();
      hapticFeedback(10);
      await deleteChatSession(
        button.dataset.sidebarSessionDelete || "",
        button.dataset.sidebarProjectPath || "",
        button,
      ).catch(showError);
    });
  });
  target.querySelectorAll("[data-sidebar-session-pin]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      event.preventDefault();
      togglePinnedChatSession(
        button.dataset.sidebarSessionPin || "",
        button.dataset.sidebarProjectPath || "",
        button.dataset.sidebarProvider || "",
      );
    });
  });
}

function renderPinnedSidebarSessions() {
  const section = qs("#sidebar-pinned-section");
  const target = qs("#sidebar-pinned-sessions");
  const count = qs("#sidebar-pinned-count");
  if (!section || !target) return;
  const search = sidebarFilterText();
  const sessions = pinnedChatEntries().filter((session) => !search || sessionMatchesSidebarSearch(session));
  section.classList.toggle("hidden", sessions.length === 0);
  if (count) count.textContent = String(sessions.length);
  if (!sessions.length) {
    target.innerHTML = "";
    return;
  }
  target.innerHTML = sessions.map((session) => sidebarSessionCardHtml(session, {
    projectName: session.projectName,
    projectPath: session.projectPath,
    pinned: true,
    showProject: true,
  })).join("");
  bindSidebarSessionActions(target);
}

function renderSidebarProjects() {
  const target = qs("#sidebar-projects");
  if (!target) return;
  renderPinnedSidebarSessions();
  const activePath = activeProjectPath();
  const ordered = orderedProjects();
  let visibleProjects = ordered.filter((project) => !sidebarProjectMeta(project.path).hidden);
  if (state.projects.length && !visibleProjects.length) {
    state.projectMeta = Object.fromEntries(
      Object.entries(state.projectMeta)
        .map(([path, meta]) => [path, { ...meta, hidden: undefined }])
        .filter(([, meta]) => meta.label),
    );
    persistSidebarState();
    visibleProjects = ordered;
    showToast("Restored hidden projects", "ok");
  }
  const projects = visibleProjects.filter(projectMatchesSidebarSearch);
  if (!projects.length) {
    target.innerHTML = `<p class="sidebar-empty">${state.projects.length ? "No matching visible projects." : "No projects yet."}</p>`;
    return;
  }
  target.innerHTML = projects.map((project) => {
    const active = project.path === activePath ? " active" : "";
    const expanded = state.expandedProjectPaths.has(project.path);
    const sessions = sidebarProjectSessions(project).filter(sessionMatchesSidebarSearch);
    const sessionCount = sessions.length;
    const displayName = projectDisplayName(project);
    const menuOpen = state.openProjectMenuPath === project.path;
    const sessionHtml = expanded
      ? `<div class="project-session-list" data-sidebar-project-sessions="${escapeHtml(project.path)}">
        <button type="button" class="project-session-new-chat" data-sidebar-new-chat="${escapeHtml(project.path)}" aria-label="Start new chat in ${escapeHtml(displayName)}" title="Start new chat" data-symbol="plus">
          <span aria-hidden="true"></span>
          <strong>New chat</strong>
        </button>
        ${sessions.length
    ? sessions.slice(0, 12).map((session) => sidebarSessionCardHtml(session, {
      projectName: displayName,
      projectPath: project.path,
    })).join("")
    : '<p class="sidebar-empty">No chat sessions.</p>'}
      </div>`
      : "";
    return `<div class="project-sidebar-wrapper${expanded ? " expanded" : ""}" data-sidebar-project-wrap="${escapeHtml(project.path)}">
      <div class="project-sidebar-row project-sidebar-card${active}" data-sidebar-project-row="${escapeHtml(project.path)}">
        <button type="button" class="project-drag-handle" data-sidebar-drag-handle="${escapeHtml(project.path)}" aria-label="Drag to reorder ${escapeHtml(displayName)}" title="Drag to reorder">
          <i></i><i></i><i></i><i></i><i></i><i></i>
        </button>
        <button type="button" class="sidebar-item project-sidebar-item" data-sidebar-project="${escapeHtml(project.path)}" aria-label="Select ${escapeHtml(displayName)} and show sessions" aria-expanded="${expanded ? "true" : "false"}"${active ? ' aria-current="true"' : ""}>
          <span class="project-sidebar-text">
            <strong>${escapeHtml(displayName)}</strong>
            <span>${escapeHtml(project.path)}</span>
            <em>${sessionCount} sessions</em>
          </span>
        </button>
        <div class="project-menu-wrap">
          <button type="button" class="icon-button${menuOpen ? " active" : ""}" data-project-menu-button="${escapeHtml(project.path)}" aria-label="Project options" title="Project options" data-symbol="dots-vertical"></button>
          <div class="project-menu${menuOpen ? "" : " hidden"}" role="menu">
            <button type="button" data-project-rename="${escapeHtml(project.path)}">Rename</button>
            <button type="button" class="danger" data-project-hide="${escapeHtml(project.path)}">Remove from sidebar</button>
          </div>
        </div>
      </div>
      ${sessionHtml}
    </div>`;
  }).join("");
  target.querySelectorAll("[data-sidebar-project]").forEach((button) => {
    button.addEventListener("click", (event) => {
      if (Date.now() < state.suppressSidebarProjectClickUntil) return;
      event.preventDefault();
      event.stopPropagation();
      const isMobile = window.matchMedia("(max-width: 760px)").matches;
      const path = button.dataset.sidebarProject;
      const wasActive = path === activeProjectPath();
      const wasExpanded = state.expandedProjectPaths.has(path);
      setActiveProject(path);
      // Switching projects should reveal its sessions. Clicking the already
      // active expanded project is the intentional collapse gesture.
      if (wasActive && wasExpanded) {
        state.expandedProjectPaths.delete(path);
      } else {
        state.expandedProjectPaths.add(path);
      }
      saveExpandedProjectPaths();
      // On mobile, do NOT trigger loadView — it would auto-close the
      // sidebar via switchView, hiding the session list the user just
      // expanded. The full chat view will fire when the user taps a
      // session below.
      if (!isMobile) {
        loadView(activeView()).catch(showError);
      }
      renderSidebarProjects();
      renderSidebarSessions();
    });
  });
  target.querySelectorAll("[data-project-menu-button]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      state.openProjectMenuPath = state.openProjectMenuPath === button.dataset.projectMenuButton
        ? ""
        : button.dataset.projectMenuButton;
      renderSidebarProjects();
    });
  });
  target.querySelectorAll("[data-project-rename]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      renameSidebarProject(button.dataset.projectRename);
    });
  });
  target.querySelectorAll("[data-project-hide]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      hideSidebarProject(button.dataset.projectHide);
    });
  });
	  target.querySelectorAll("[data-sidebar-new-chat]").forEach((button) => {
	    button.addEventListener("click", (event) => {
	      event.stopPropagation();
	      event.preventDefault();
	      hapticFeedback(8);
	      startNewChatForProject(button.dataset.sidebarNewChat).catch(showError);
	    });
    button.addEventListener("pointerdown", (event) => {
      // Keep this control isolated from surrounding project/session clicks.
      event.stopPropagation();
    });
  });
  bindSidebarSessionActions(target);
  target.querySelectorAll("[data-sidebar-drag-handle]").forEach((handle) => {
    handle.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      // Initiate drag only from the dedicated grip handle, never from the
      // rest of the row. This frees the rest of the row for vertical scroll
      // gestures on touch devices and for ordinary click-to-open on desktop.
      const projectPath = handle.dataset.sidebarDragHandle;
      const row = handle.closest("[data-sidebar-project-row]");
      if (!projectPath || !row) return;
      state.pointerProjectDrag = {
        path: projectPath,
        startX: event.clientX,
        startY: event.clientY,
        startedAt: Date.now(),
        dragging: false,
      };
      hapticFeedback(6);
      // The handle lives inside the project button. Suppress the next
      // button click so a tap on the grip doesn't toggle the project.
      state.suppressSidebarProjectClickUntil = Date.now() + 600;
      // Stop the project-item button click that would otherwise fire on the
      // same pointerup once the drag ends without committing to a reorder.
      event.stopPropagation();
      event.preventDefault();
      document.addEventListener("pointermove", handleSidebarProjectPointerMove, { passive: false });
      document.addEventListener("pointerup", finishSidebarProjectPointerDrag, { once: true });
      document.addEventListener("pointercancel", finishSidebarProjectPointerDrag, { once: true });
    });
    // Keyboard fallback: focus the handle and press Space/Enter to "pick up"
    // the project, then arrow keys to move it within the sidebar list.
    handle.addEventListener("keydown", (event) => {
      if (![" ", "Enter", "ArrowUp", "ArrowDown"].includes(event.key)) return;
      event.preventDefault();
      const projectPath = handle.dataset.sidebarDragHandle;
      const projects = orderedProjects()
        .filter((project) => !sidebarProjectMeta(project.path).hidden)
        .filter(projectMatchesSidebarSearch)
        .map((p) => p.path);
      const fromIndex = projects.indexOf(projectPath);
      if (fromIndex < 0) return;
      let toIndex = fromIndex;
      if (event.key === "ArrowUp") toIndex = Math.max(0, fromIndex - 1);
      if (event.key === "ArrowDown") toIndex = Math.min(projects.length - 1, fromIndex + 1);
      if (toIndex === fromIndex) return;
      const target = projects[toIndex];
      const placement = toIndex < fromIndex ? "before" : "after";
      moveProjectOrder(projectPath, target, placement);
      hapticFeedback([8, 20, 8]);
      // Restore focus to the handle after the re-render so the user can
      // keep arrow-keying the row around the list.
      requestAnimationFrame(() => {
        const next = document.querySelector(`[data-sidebar-drag-handle="${CSS.escape(projectPath)}"]`);
        next?.focus();
      });
    });
  });
}

function formatRelativeTime(value) {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  const diffMs = Date.now() - date.getTime();
  const sec = Math.round(diffMs / 1000);
  if (sec < 0) return "just now";
  if (sec < 30) return "just now";
  if (sec < 60) return `${sec} seconds ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min} minute${min === 1 ? "" : "s"} ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr} hour${hr === 1 ? "" : "s"} ago`;
  const day = Math.floor(hr / 24);
  if (day < 30) return `${day} day${day === 1 ? "" : "s"} ago`;
  const mon = Math.floor(day / 30);
  if (mon < 12) return `${mon} month${mon === 1 ? "" : "s"} ago`;
  const yr = Math.floor(day / 365);
  return `${yr} year${yr === 1 ? "" : "s"} ago`;
}

function renderSidebarSessions() {
  const target = qs("#sidebar-sessions");
  if (!target) return;
  const search = (qs("#sidebar-search")?.value || "").trim().toLowerCase();
  const sessions = sidebarSessions()
    .filter((session) => !search || sessionMatchesSidebarSearch(session))
    .sort((a, b) => new Date(b.lastActivity || b.updatedAt || 0) - new Date(a.lastActivity || a.updatedAt || 0))
    .slice(0, 30);
  if (!sessions.length) {
    target.innerHTML = '<p class="sidebar-empty">No recent sessions.</p>';
    return;
  }
  target.innerHTML = sessions.map((session) => {
    return sidebarSessionCardHtml(session);
  }).join("");
  bindSidebarSessionActions(target);
}


// ---------------------------------------------------------------------------
// Chat history loaders.  When the user opens a chat session from the sidebar
// (or when bootstrap auto-picks the most recent session), we hydrate the
// transcript by replaying the persisted messages into `#chat-output` with
// the user prompts floated right and assistant replies left-aligned. We also
// restore the per-session overrides + footer metadata so the chat looks
// identical to the live session it was when the user last used it.
// ---------------------------------------------------------------------------

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

function chatOutputRoot() {
  return qs("#chat-output");
}

function scrollChatToBottom() {
  const output = chatOutputRoot();
  if (!output) return;
  output.scrollTop = output.scrollHeight;
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

function chatEventSessionIds() {
  return new Set([
    state.chatSessionId,
    state.pendingChatSessionId,
    state.currentSession?.sessionId,
    state.preferences?.lastChatSessionId,
  ].filter(Boolean));
}

function isActiveChatSessionEvent(payload = {}) {
  if (!payload.sessionId) return false;
  const ids = chatEventSessionIds();
  if (!ids.size) return true;
  return ids.has(payload.sessionId);
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
  };
}

function setProcessingText(textNode, label = "Processing") {
  if (!textNode) return;
  textNode.innerHTML = `<span class="chat-processing-label">${escapeHtml(label)}</span><span class="chat-processing-dots" aria-hidden="true"></span>`;
}

function ensureChatProcessing(payload = {}) {
  if (!isActiveChatSessionEvent(payload)) return null;
  const output = chatOutputRoot();
  if (!output) return null;
  if (chatStream.node?.isConnected && chatStream.role === "assistant" && state.chatBuffer) {
    return null;
  }
  if (state.chatProcessing?.node?.isConnected) {
    renderChatLineFooter(state.chatProcessing.footer, sessionMetaForStatus(payload));
    scrollChatToBottom();
    updateChatEmptyState();
    return state.chatProcessing;
  }
  const line = buildChatLineNode("assistant");
  line.node.classList.add("chat-line-processing");
  line.node.dataset.processingSessionId = payload.sessionId || "";
  setProcessingText(line.text);
  renderChatLineFooter(line.footer, sessionMetaForStatus(payload));
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
  renderChatLineFooter(state.chatProcessing.footer, sessionMetaForStatus(payload));
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
  if (meta.thinking) items.push(`<span>Thinking on</span>`);
  if (meta.tokenUsage) items.push(`<span>Tokens: <strong>${escapeHtml(meta.tokenUsage)}</strong></span>`);
  if (meta.receivedAt) items.push(`<span>Received: <strong>${escapeHtml(meta.receivedAt)}</strong></span>`);
  if (meta.elapsed) items.push(`<span>Elapsed: <strong>${escapeHtml(meta.elapsed)}</strong></span>`);
  if (meta.sentAt) items.push(`<span>Sent: <strong>${escapeHtml(meta.sentAt)}</strong></span>`);
  footer.innerHTML = items.join("");
}

function replayUserPromptLine(prompt, meta) {
  const output = chatOutputRoot();
  if (!output) return;
  const isHtml = typeof prompt === "string" && /<[a-z][^>]*>/i.test(prompt);
  const { node, text, footer } = buildChatLineNode("user");
  if (isHtml) text.innerHTML = prompt;
  else text.textContent = String(prompt);
  renderChatLineFooter(footer, meta);
  output.appendChild(node);
}

function replayAssistantLine(content, meta) {
  const output = chatOutputRoot();
  if (!output) return;
  const isHtml = typeof content === "string" && /<[a-z][^>]*>/i.test(content);
  const { node, text, footer } = buildChatLineNode("assistant");
  if (isHtml) text.innerHTML = content;
  else text.textContent = String(content);
  renderChatLineFooter(footer, meta);
  output.appendChild(node);
}

function replayChatMessages(messages) {
  for (const raw of messages) {
    if (!raw) continue;
    const role = String(raw.role || "").toLowerCase();
    const content = raw.content == null ? "" : String(raw.content);
    // Per-turn metadata (cli / model / sentAt / receivedAt / tokenUsage /
    // elapsed) is stored on each message row in storage. Fall back to the
    // legacy `meta` alias in case older sessions used it.
    const persistedMeta = raw.metadata && typeof raw.metadata === "object"
      ? raw.metadata
      : (raw.meta && typeof raw.meta === "object" ? raw.meta : {});
    const meta = normalizeMessageMeta(persistedMeta);
    if (role === "user") replayUserPromptLine(content, meta);
    else if (role === "assistant") replayAssistantLine(content, meta);
    else if (role === "system") {
      const output = chatOutputRoot();
      if (!output) return;
      const node = document.createElement("div");
      node.className = "chat-line-system";
      node.textContent = content;
      output.appendChild(node);
    }
  }
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
  try {
    const url = `/api/sessions/${encodeURIComponent(sessionId)}/messages?limit=200&offset=0`;
    const body = await api(url);
    const messages = Array.isArray(body) ? body : (body.messages || []);
    resetChatOutputDom();
    const persisted = getSessionOverridesFor(sessionId) || {};
    const all = readSessionOverrides();
    const overrides = all[sessionId] || {};
    const replayMeta = (msg, role) => {
      // Prefer the per-message metadata persisted on the message row by the
      // server. Fall back to the legacy per-session override so older turns
      // still render something useful in the footer.
      const stored = msg.metadata && typeof msg.metadata === "object"
        ? msg.metadata
        : (msg.meta && typeof msg.meta === "object" ? msg.meta : null);
      if (stored) {
        const normalized = normalizeMessageMeta(stored);
        if (Object.keys(normalized).length) return normalized;
      }
      const isLatest = messages.indexOf(msg) === messages.length - 1;
      if (role === "user") {
        return {
          cli: persisted.cli || persisted.provider,
          model: persisted.model,
          mode: persisted.mode,
          effort: persisted.effort,
          sentAt: persisted.sentAt,
        };
      }
      return {
        cli: persisted.cli || persisted.provider,
        model: persisted.model,
        mode: persisted.mode,
        effort: persisted.effort,
        tokenUsage: isLatest ? persisted.tokenUsage : "",
        receivedAt: isLatest ? persisted.receivedAt : "",
        elapsed: isLatest ? persisted.elapsed : "",
      };
    };
    for (const raw of messages) {
      if (!raw) continue;
      const role = String(raw.role || "").toLowerCase();
      const content = raw.content == null ? "" : String(raw.content);
      const meta = replayMeta(raw, role);
      if (role === "user") replayUserPromptLine(content, meta);
      else if (role === "assistant") replayAssistantLine(content, meta);
      else if (role === "system") {
        const output = chatOutputRoot();
        if (!output) continue;
        const node = document.createElement("div");
        node.className = "chat-line-system";
        node.textContent = content;
        output.appendChild(node);
      }
    }
    scrollChatToBottom();
    // Restore the per-session overrides + footer (legacy slot).
    loadSessionOverridesIntoState(sessionId);
    renderChatFooter(getSessionOverridesFor(sessionId));
    updateChatEmptyState();
    return true;
  } catch (error) {
    showError(new Error(`Could not load chat history: ${error.message}`));
    return false;
  }
}

async function pickChatSession(sessionId, projectPath) {
  const id = (sessionId || "").trim();
  if (!id) return;
  const session = findChatSession(id);
  state.pendingChatSessionId = id;
  state.chatSessionId = id;
  state.preferences.lastChatSessionId = id;
  savePreferences();
  if (projectPath) setActiveProject(projectPath);
  await switchView("chat");
  if (session?.pending) {
    resetChatOutputDom();
    updateChatEmptyState();
    return;
  }
  state.pendingChatSessionId = "";
  await loadChatHistoryForSession(id);
}

// Start a fresh chat session for a given project. Wipes any active session
// pointer, makes the project the active one, focuses the prompt, and clears
// the chat output so the user can type and send right away. Also inserts a
// placeholder session entry under the project in the sidebar so the user
// can see where the new chat will live before they type anything; the
// server adopts this id when the first message is submitted, so the entry
// transitions seamlessly from placeholder to real session.
async function startNewChatForProject(projectPath) {
  if (!projectPath) return;
  setActiveProject(projectPath);
  const project = (state.projects || []).find((p) => p.path === projectPath);
  const cli = chatCliValue();
  const placeholderId = `local-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
  if (!Array.isArray(state.sessions)) state.sessions = [];
  // Remove any other placeholder session we created earlier in this project
  // so the sidebar only ever shows one "new chat" pending entry.
  state.sessions = state.sessions.filter((session) => {
    if (!session || !session.pending) return true;
    const sessionPath = session.projectPath || session.projectName;
    return sessionPath !== projectPath && sessionPath !== (project && project.name);
  });
  state.sessions.push({
    id: placeholderId,
    provider: cli,
    projectPath,
    projectName: project && project.name,
    title: "New chat",
    messageCount: 0,
    pending: true,
    lastActivity: new Date().toISOString(),
  });
  state.chatSessionId = placeholderId;
  state.pendingChatSessionId = placeholderId;
  state.preferences.lastChatSessionId = placeholderId;
  state.chatBuffer = "";
  if (!state.expandedProjectPaths.has(projectPath)) {
    state.expandedProjectPaths.add(projectPath);
    saveExpandedProjectPaths();
  }
  savePreferences();
  renderProjects();
  renderSidebarProjects();
  await switchView("chat");
  resetChatOutputDom();
  updateChatEmptyState();
  const prompt = qs("#chat-prompt");
  if (prompt) {
    prompt.value = "";
    prompt.focus();
    autosizeChatPrompt();
  }
}

function pickDefaultChatSession() {
  // Choose the most recent session we can find.  If the active project has
  // sessions, prefer those; otherwise fall back to the single most-recent
  // session across all projects.
  const allSessions = []
    .concat(
      ...(state.projects || []).map((p) => (p.sessions || []).map((s) => ({ ...s, projectPath: p.path }))),
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
    .flatMap((p) => (p.sessions || []).map((s) => ({ ...s, projectPath: p.path })))
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

function renderSessions() {
  const list = qs("#sessions-list");
  if (!list) {
    renderSidebarProjects();
    return;
  }
  const filter = qs("#sessions-filter")?.value.trim().toLowerCase() || "";
  const sessions = state.sessions.filter((session) => {
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
    render: (session) => `<article class="row">
      <strong>${escapeHtml(session.title || session.id)}</strong>
      <span>${escapeHtml(session.provider)} · ${escapeHtml(session.projectPath)}</span>
      <span class="meta">${session.messageCount} messages</span>
      <div class="row-actions">
        <button type="button" data-session-use="${escapeHtml(session.id)}">Use</button>
        <button type="button" data-session-open="${escapeHtml(session.id)}">Messages</button>
      </div>
    </article>`,
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
  return qs("#chat-model")?.value || state.preferences.chatModel || "";
}

function chatModeValue() {
  return qs("#chat-mode")?.value || state.preferences.chatMode || "default";
}

function chatEffortValue() {
  return qs("#chat-effort")?.value || state.preferences.chatEffort || "medium";
}

function chatThinkingValue() {
  return Boolean(state.preferences.chatThinking);
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
        if (typeof entry === "string") return { value: entry, label: entry };
        if (typeof entry === "object") {
          const value = entry.value ?? entry.id ?? entry.name ?? "";
          if (!value) return null;
          return { value: String(value), label: String(entry.label ?? value) };
        }
        return null;
      })
      .filter(Boolean);
    const current = select.value;
    select.innerHTML = entries.length
      ? entries.map((m) => `<option value="${escapeHtml(m.value)}">${escapeHtml(m.label)}</option>`).join("")
      : `<option value="">No models available</option>`;
    const values = entries.map((m) => m.value);
    if (current && values.includes(current)) select.value = current;
    else if (state.preferences.chatModel && values.includes(state.preferences.chatModel)) {
      select.value = state.preferences.chatModel;
    } else if (values.length) {
      select.value = values[0];
    }
    state.preferences.chatModel = select.value || "";
    savePreferences();
  } catch (error) {
    console.warn("[io-workbench] could not load chat models", error);
  } finally {
    if (select.dataset.modelProvider === targetProvider) select.disabled = false;
  }
}

function readSessionOverrides() {
  return (state.preferences && state.preferences.chatSessionOverrides) || {};
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
  all[sessionId] = Object.assign({}, all[sessionId] || {}, patch);
  writeSessionOverrides(all);
}

function loadSessionOverridesIntoState(sessionId) {
  const entry = getSessionOverridesFor(sessionId);
  if (!entry) return;
  if (entry.cli) {
    state.preferences.chatCli = entry.cli;
    state.preferences.chatProvider = entry.cli;
  }
  if (entry.model !== undefined) {
    state.preferences.chatModel = entry.model;
    const s = qs("#chat-model");
    if (s) s.value = entry.model;
  }
  if (entry.effort !== undefined) {
    state.preferences.chatEffort = entry.effort;
    const s = qs("#chat-effort");
    if (s) s.value = entry.effort;
  }
  if (entry.mode !== undefined) {
    state.preferences.chatMode = entry.mode;
    const s = qs("#chat-mode");
    if (s) s.value = entry.mode;
  }
  if (entry.thinking !== undefined) {
    state.preferences.chatThinking = entry.thinking;
  }
  renderChatProviderPicker();
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
  if (meta.tokenUsage) items.push(`<span class="meta">Tokens: <strong>${escapeHtml(meta.tokenUsage)}</strong></span>`);
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
  document.body.classList.toggle("compact", !!state.preferences.compact);
  document.body.classList.toggle("wrap-output", !!state.preferences.wrapOutput);
  qs("#pref-compact").checked = !!state.preferences.compact;
  qs("#pref-wrap").checked = !!state.preferences.wrapOutput;
  applyTerminalSizeToInputs();
  applyChatProviderPreference();
  // Populate the chat-controls select widgets from current preferences.
  if (qs("#chat-effort")) qs("#chat-effort").value = state.preferences.chatEffort || "medium";
  if (qs("#chat-mode")) qs("#chat-mode").value = state.preferences.chatMode || "default";
  if (qs("#chat-model")) {
    loadChatModelsIntoSelect(state.preferences.chatCli || state.preferences.chatProvider || "codex").catch(() => {});
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

async function loadAuthStatus() {
  state.auth = await api("/api/auth/status");
  if (state.auth.isAuthenticated) {
    hideAuthPanel();
    return true;
  }
  if (!state.auth.enabled) {
    hideAuthPanel();
    return true;
  }
  if (state.auth.needsSetup) {
    showAuthPanel("setup");
    return false;
  }
  showAuthPanel(authPanelMode());
  return false;
}

async function loadHealth() {
  const health = await api("/health");
  const summary = qs("#server-summary");
  if (summary) {
    summary.textContent = `${health.service} ${health.version} · ${health.config_dir}`;
  }
}

async function loadProjects() {
  const body = await api("/api/projects");
  state.projects = body.projects || [];
  syncProjectOrder();
  const activeExists = state.projects.some((project) => project.path === state.activeProjectPath);
  if (!activeExists) {
    state.activeProjectPath = state.projects[0]?.path || "";
    window.localStorage.setItem("iowb.activeProjectPath", state.activeProjectPath);
  }
  renderProjects();
}

async function loadSettings() {
  state.settings = await api("/api/settings/server-status");
  renderSettings();
}

async function loadMetrics() {
  state.metrics = await api("/api/metrics/runtime");
  renderMetrics();
}

async function loadFiles() {
  const project = activeProjectName();
  if (!project) return;
  const path = qs("#files-path").value.trim() || ".";
  const body = await api(`/api/projects/${encodeURIComponent(project)}/files?path=${encodeURIComponent(path)}`);
  state.fileEntries = Array.isArray(body) ? body : body.entries || [];
  state.fileSelectedPaths = new Set([...state.fileSelectedPaths].filter((selectedPath) => findFileEntryByPath(state.fileEntries, selectedPath)));
  renderFileEntries();
}

function renderFileEntries(entries = state.fileEntries) {
  const target = qs("#files-tree");
  const filter = qs("#files-filter")?.value.trim().toLowerCase() || "";
  const visibleEntries = filterFileEntries(entries, filter);
  renderFileToolbar(visibleEntries, filter);
  if (!visibleEntries.length) {
    target.innerHTML = `<div class="file-tree-empty">
      <span class="file-tree-empty-icon" aria-hidden="true"></span>
      <strong>${filter ? "No matches found" : "No files found"}</strong>
      <span>${filter ? "Try a different search." : "Check the selected project path."}</span>
    </div>`;
    return;
  }

  const flattened = flattenVisibleFileEntries(visibleEntries, 0, !!filter);
  target.classList.toggle("files-view-simple", state.fileViewMode === "simple");
  target.classList.toggle("files-view-compact", state.fileViewMode === "compact");
  target.classList.toggle("files-view-detailed", state.fileViewMode === "detailed");
  const rows = [];
  if (state.fileCreating && normalizeProjectPath(state.fileCreating.parentPath) === ".") {
    rows.push(fileCreateRowHtml(state.fileCreating, 0));
  }
  flattened.forEach(({ entry, depth }) => {
    rows.push(fileEntryHtml(entry, depth));
    if (
      state.fileCreating
      && entry.type === "directory"
      && normalizeProjectPath(state.fileCreating.parentPath) === normalizeProjectPath(entry.path)
    ) {
      rows.push(fileCreateRowHtml(state.fileCreating, depth + 1));
    }
  });
  target.innerHTML = rows.join("");

  target.querySelectorAll("[data-file-select]").forEach((checkbox) => {
    checkbox.addEventListener("click", (event) => event.stopPropagation());
    checkbox.addEventListener("change", (event) => {
      const path = event.currentTarget.dataset.fileSelect;
      if (!path) return;
      if (event.currentTarget.checked) {
        state.fileSelectedPaths.add(path);
      } else {
        state.fileSelectedPaths.delete(path);
      }
      renderFileEntries();
    });
  });

  target.querySelectorAll("[data-file-row-path]").forEach((row) => {
    const activate = () => {
      const path = row.dataset.fileRowPath;
      if (!path) return;
      if (row.dataset.kind === "directory") {
        toggleFileDirectory(path);
        return;
      }
      openFile(path).catch(showError);
    };
    row.addEventListener("click", (event) => {
      if (event.target.closest("input, button")) return;
      activate();
    });
    row.addEventListener("keydown", (event) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      activate();
    });
    row.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      const entry = findFileEntryByPath(state.fileEntries, row.dataset.fileRowPath);
      if (entry) openFileContextMenu(entry, event.clientX, event.clientY);
    });
  });
  target.querySelectorAll("[data-file-menu]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      const entry = findFileEntryByPath(state.fileEntries, button.dataset.fileMenu);
      const rect = button.getBoundingClientRect();
      if (entry) openFileContextMenu(entry, rect.left, rect.bottom + 4);
    });
  });
  bindFileInlineInputs(target);
}

function renderFileBreadcrumbs(path) {
  const target = qs("#files-breadcrumbs");
  if (!target) return;
  const normalized = normalizeProjectPath(path);
  const parts = normalized === "." ? [] : normalized.split("/").filter(Boolean);
  const buttons = [
    `<button type="button" data-file-nav=".">.</button>`,
    ...parts.map((part, index) => {
      const segmentPath = parts.slice(0, index + 1).join("/");
      return `<button type="button" data-file-nav="${escapeHtml(segmentPath)}">${escapeHtml(part)}</button>`;
    }),
  ];
  target.innerHTML = buttons.join('<span class="breadcrumb-separator">/</span>');
  target.querySelectorAll("[data-file-nav]").forEach((button) => {
    button.addEventListener("click", () => {
      qs("#files-path").value = button.dataset.fileNav;
      loadFiles().catch(showError);
    });
  });
}

function normalizeProjectPath(path) {
  const normalized = String(path || ".")
    .replaceAll("\\", "/")
    .replace(/^\/+/, "")
    .replace(/\/+/g, "/")
    .replace(/\/$/, "");
  return normalized && normalized !== "." ? normalized : ".";
}

function parentProjectPath(path) {
  const normalized = normalizeProjectPath(path);
  if (normalized === ".") return ".";
  const parts = normalized.split("/").filter(Boolean);
  parts.pop();
  return parts.length ? parts.join("/") : ".";
}

function parentFilesystemPath(path) {
  const value = String(path || "~").trim() || "~";
  if (value === "~" || value === "/" || /^[A-Za-z]:[\\/]?$/.test(value)) return "";
  const normalized = value.replaceAll("\\", "/").replace(/\/+$/, "");
  const index = normalized.lastIndexOf("/");
  if (normalized.startsWith("~/")) {
    if (index <= 1) return "~";
    return normalized.slice(0, index);
  }
  if (index <= 0) return normalized.startsWith("/") ? "/" : "";
  if (index === 2 && /^[A-Za-z]:/.test(normalized)) return `${normalized.slice(0, 2)}/`;
  return normalized.slice(0, index);
}

function filesystemDirname(path) {
  const value = String(path || "").replaceAll("\\", "/").replace(/\/+$/, "");
  const index = value.lastIndexOf("/");
  if (index <= 0) return value.startsWith("/") ? "/" : "";
  if (index === 2 && /^[A-Za-z]:/.test(value)) return `${value.slice(0, 2)}/`;
  return value.slice(0, index);
}

function sameFilesystemPath(left, right) {
  const normalize = (value) => String(value || "").replaceAll("\\", "/").replace(/\/+$/, "");
  return normalize(left) === normalize(right);
}

function resolvedBrowsePath(requestedPath, entries = []) {
  const requested = String(requestedPath || "~").trim() || "~";
  if (requested !== "~") return requested;
  const firstAbsolute = entries.find((entry) => String(entry.path || "").startsWith("/"));
  return firstAbsolute ? filesystemDirname(firstAbsolute.path) : requested;
}

function folderBrowserEntries() {
  const filter = state.folderBrowser.filter.trim().toLowerCase();
  return (state.folderBrowser.entries || [])
    .filter((entry) => entry.type === "directory")
    .filter((entry) => state.folderBrowser.showHidden || !entry.name.startsWith("."))
    .filter((entry) => !filter || [entry.name, entry.path].join(" ").toLowerCase().includes(filter));
}

function folderBrowserActionLabel() {
  return state.folderBrowser.action === "add-project" ? "Add Project" : "Use Folder";
}

function renderFolderBrowser() {
  const browser = state.folderBrowser;
  qs("#folder-browser-title").textContent = browser.action === "add-project" ? "Add Project" : "Select Folder";
  qs("#folder-browser-path").textContent = browser.path || "~";
  qs("#folder-browser-filter").value = browser.filter;
  const parentPath = parentFilesystemPath(browser.path);
  qs("#folder-browser-up").disabled = !parentPath || sameFilesystemPath(browser.path, browser.homePath);
  qs("#folder-browser-use").disabled = browser.loading;
  qs("#folder-browser-use").setAttribute("aria-label", folderBrowserActionLabel());
  qs("#folder-browser-use").title = folderBrowserActionLabel();
  qs("#folder-browser-use").dataset.symbol = browser.action === "add-project" ? "plus" : "check";
  qs("#folder-browser-hidden").classList.toggle("active", browser.showHidden);
  qs("#folder-browser-hidden").setAttribute(
    "aria-label",
    browser.showHidden ? "Hide hidden folders" : "Show hidden folders",
  );
  qs("#folder-browser-hidden").title = browser.showHidden ? "Hide hidden folders" : "Show hidden folders";
  const status = qs("#folder-browser-status");
  const list = qs("#folder-browser-list");
  if (browser.loading) {
    status.textContent = "Loading folders...";
    list.innerHTML = "";
    return;
  }
  const entries = folderBrowserEntries();
  const hiddenCount = (browser.entries || []).filter((entry) => entry.type === "directory" && entry.name.startsWith(".")).length;
  const selectLabel = browser.action === "add-project" ? "Add Project" : "Use Folder";
  const selectIcon = browser.action === "add-project" ? "plus" : "check";
  status.textContent = entries.length
    ? `${entries.length} folder${entries.length === 1 ? "" : "s"}${!browser.showHidden && hiddenCount ? ` · ${hiddenCount} hidden` : ""}`
    : "No folders found.";
  list.innerHTML = entries.map((entry) => `
    <article class="folder-row" data-folder-open-card="${escapeHtml(entry.path)}" tabindex="0" role="button" aria-label="Open ${escapeHtml(entry.name || entry.path)}">
      <div class="folder-info">
        <strong>${escapeHtml(entry.name || entry.path)}</strong>
        <span>${escapeHtml(entry.path)}</span>
      </div>
      <div class="folder-row-actions">
        <button type="button" class="icon-button" data-folder-open="${escapeHtml(entry.path)}" aria-label="Open folder" title="Open folder" data-symbol="open"></button>
        <button type="button" class="icon-button secondary-action" data-folder-select="${escapeHtml(entry.path)}" aria-label="${selectLabel}" title="${selectLabel}" data-symbol="${selectIcon}"></button>
      </div>
    </article>
  `).join("");
  list.querySelectorAll("[data-folder-open-card]").forEach((card) => {
    const open = () => loadFolderBrowser(card.dataset.folderOpenCard).catch(showError);
    card.addEventListener("click", (event) => {
      if (event.target.closest("button")) return;
      open();
    });
    card.addEventListener("keydown", (event) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      open();
    });
  });
  list.querySelectorAll("[data-folder-open]").forEach((button) => {
    button.addEventListener("click", () => loadFolderBrowser(button.dataset.folderOpen).catch(showError));
  });
  list.querySelectorAll("[data-folder-select]").forEach((button) => {
    button.addEventListener("click", () => selectFolderBrowserPath(button.dataset.folderSelect).catch(showError));
  });
}

async function loadFolderBrowser(path = state.folderBrowser.path || "~") {
  state.folderBrowser.loading = true;
  renderFolderBrowser();
  const body = await api(`/api/browse-filesystem?path=${encodeURIComponent(path || "~")}`);
  state.folderBrowser.entries = body.entries || [];
  state.folderBrowser.path = resolvedBrowsePath(body.path || path || "~", state.folderBrowser.entries);
  if ((path || "~") === "~") {
    state.folderBrowser.homePath = state.folderBrowser.path;
  }
  state.folderBrowser.loading = false;
  renderFolderBrowser();
}

function openFolderBrowser(targetInput = "", options = {}) {
  const currentValue = targetInput ? qs(targetInput)?.value.trim() : "";
  state.folderBrowser.open = true;
  state.folderBrowser.action = options.action || "select";
  state.folderBrowser.targetInput = targetInput;
  state.folderBrowser.path = options.path || currentValue || "~";
  state.folderBrowser.filter = "";
  state.folderBrowser.entries = [];
  state.folderBrowser.showHidden = false;
  qs("#folder-browser").classList.remove("hidden");
  loadFolderBrowser(state.folderBrowser.path).catch((error) => {
    state.folderBrowser.loading = false;
    renderFolderBrowser();
    showError(error);
  });
}

function closeFolderBrowser() {
  state.folderBrowser.open = false;
  qs("#folder-browser")?.classList.add("hidden");
}

async function addProjectPath(path) {
  const trimmed = String(path || "").trim();
  if (!trimmed) return null;
  const project = await api("/api/projects/create", {
    method: "POST",
    body: JSON.stringify({ path: trimmed }),
  });
  await loadProjects();
  setActiveProject(project?.path || trimmed);
  showToast("Project added", "ok");
  return project;
}

async function selectFolderBrowserPath(path = state.folderBrowser.path) {
  if (state.folderBrowser.action === "add-project") {
    await addProjectPath(path);
    closeFolderBrowser();
    return;
  }
  const target = qs(state.folderBrowser.targetInput || "");
  if (target) {
    target.value = path || "";
    target.dispatchEvent(new Event("input", { bubbles: true }));
  }
  closeFolderBrowser();
  showToast("Folder selected", "ok");
}

function flattenFileEntries(entries, depth = 0) {
  return entries.flatMap((entry) => [
    { entry, depth },
    ...flattenFileEntries(entry.children || [], depth + 1),
  ]);
}

function flattenVisibleFileEntries(entries, depth = 0, forceExpanded = false) {
  return entries.flatMap((entry) => {
    const isDirectory = entry.type === "directory";
    const isExpanded = forceExpanded || state.fileExpandedPaths.has(entry.path);
    const children = isDirectory && isExpanded
      ? flattenVisibleFileEntries(entry.children || [], depth + 1, forceExpanded)
      : [];
    return [{ entry, depth }, ...children];
  });
}

function filterFileEntries(entries, filter) {
  if (!filter) return entries;
  return entries
    .map((entry) => {
      const children = filterFileEntries(entry.children || [], filter);
      const matches = [entry.name, entry.path, entry.type].some((value) => matchesText(value, filter));
      if (matches || children.length) {
        return { ...entry, children };
      }
      return null;
    })
    .filter(Boolean);
}

function findFileEntryByPath(entries, path) {
  for (const entry of entries || []) {
    if (entry.path === path) return entry;
    const child = findFileEntryByPath(entry.children || [], path);
    if (child) return child;
  }
  return null;
}

function fileEntriesSelectable(entries) {
  return flattenFileEntries(entries).map(({ entry }) => entry);
}

function setFileTreeViewMode(mode) {
  state.fileViewMode = ["simple", "compact", "detailed"].includes(mode) ? mode : "detailed";
  window.localStorage.setItem("iowb.fileViewMode", state.fileViewMode);
  renderFileEntries();
}

function renderFileToolbar(visibleEntries, filter) {
  const selectable = fileEntriesSelectable(visibleEntries);
  const selectablePaths = new Set(selectable.map((entry) => entry.path));
  state.fileSelectedPaths = new Set([...state.fileSelectedPaths].filter((path) => selectablePaths.has(path)));
  const selectedCount = state.fileSelectedPaths.size;
  const allSelected = selectable.length > 0 && selectable.every((entry) => state.fileSelectedPaths.has(entry.path));
  const partialSelected = selectedCount > 0 && !allSelected;
  const selectAll = qs("#files-select-all");
  if (selectAll) {
    selectAll.dataset.symbol = allSelected ? "check-square" : "square";
    selectAll.classList.toggle("active", allSelected || partialSelected);
    selectAll.disabled = selectable.length === 0;
    selectAll.title = allSelected ? "Deselect all" : "Select all";
    selectAll.setAttribute("aria-label", selectAll.title);
  }
  qs("#files-selection-chip")?.classList.toggle("hidden", selectedCount === 0);
  const count = qs("#files-selection-count");
  if (count) count.textContent = `${selectedCount} selected`;
  qs("#files-clear-filter")?.classList.toggle("hidden", !filter);
  qs("#files-columns")?.classList.toggle("hidden", state.fileViewMode !== "detailed");
  document.querySelectorAll("[data-file-view-mode]").forEach((button) => {
    const active = button.dataset.fileViewMode === state.fileViewMode;
    button.classList.toggle("active", active);
    button.setAttribute("aria-pressed", String(active));
  });
}

function toggleFileDirectory(path) {
  if (state.fileExpandedPaths.has(path)) {
    state.fileExpandedPaths.delete(path);
  } else {
    state.fileExpandedPaths.add(path);
  }
  renderFileEntries();
}

function toggleAllVisibleFilesSelection() {
  const filter = qs("#files-filter")?.value.trim().toLowerCase() || "";
  const visibleEntries = filterFileEntries(state.fileEntries, filter);
  const selectable = fileEntriesSelectable(visibleEntries);
  const allSelected = selectable.length > 0 && selectable.every((entry) => state.fileSelectedPaths.has(entry.path));
  if (allSelected) {
    selectable.forEach((entry) => state.fileSelectedPaths.delete(entry.path));
  } else {
    selectable.forEach((entry) => state.fileSelectedPaths.add(entry.path));
  }
  renderFileEntries();
}

function fileCreateRowHtml(createState, depth) {
  const name = escapeHtml(createState.name || (createState.directory ? "untitled-folder" : "untitled.txt"));
  const iconKind = createState.directory ? "folder" : "file";
  return `<article class="file-tree-row creating file-tree-row-${escapeHtml(state.fileViewMode)}" data-file-inline-row="create" data-kind="${createState.directory ? "directory" : "file"}">
    <div class="file-tree-name" style="--file-depth:${depth}">
      <span></span>
      <span class="file-disclosure file" aria-hidden="true"></span>
      <span class="file-icon file-icon-${iconKind}" aria-hidden="true"></span>
      <input class="file-inline-input" data-file-create-input value="${name}" aria-label="${createState.directory ? "New folder name" : "New file name"}" />
    </div>
    <span class="file-tree-size"></span>
    <span class="file-tree-modified"></span>
    <span class="file-tree-permissions"></span>
  </article>`;
}

function startCreateFileTreePath(directory, parentPath = ".") {
  const parent = normalizeProjectPath(parentPath || ".");
  state.fileCreating = {
    directory: !!directory,
    parentPath: parent,
    name: directory ? "untitled-folder" : "untitled.txt",
  };
  state.fileRenamingPath = "";
  closeFileContextMenu();
  if (parent !== ".") state.fileExpandedPaths.add(parent);
  renderFileEntries();
  requestAnimationFrame(() => {
    const input = qs("[data-file-create-input]");
    input?.focus();
    input?.select();
  });
}

function cancelFileCreate() {
  state.fileCreating = null;
  renderFileEntries();
}

async function commitFileCreate() {
  const createState = state.fileCreating;
  const input = qs("[data-file-create-input]");
  const name = normalizeProjectPath(input?.value || "");
  if (!createState || !name || name === ".") {
    cancelFileCreate();
    return;
  }
  await createFileTreePath(createState.directory, createState.parentPath, name);
}

async function createFileTreePath(directory, parentPath = ".", name = "") {
  const project = activeProjectName();
  if (!project) return;
  const base = normalizeProjectPath(parentPath || qs("#files-path")?.value || ".");
  const trimmed = normalizeProjectPath(name || "");
  if (!trimmed || trimmed === ".") return;
  const filePath = normalizeProjectPath(base === "." ? trimmed : `${base}/${trimmed}`);
  await api(`/api/projects/${encodeURIComponent(project)}/files/create`, {
    method: "POST",
    body: JSON.stringify({
      filePath,
      content: "",
      directory,
    }),
  });
  if (base !== ".") state.fileExpandedPaths.add(base);
  state.fileCreating = null;
  await loadFiles();
  showToast(`${directory ? "Folder" : "File"} created`, "ok");
}

function startRenameFilePath(filePath) {
  if (!filePath) return;
  state.fileRenamingPath = filePath;
  state.fileCreating = null;
  closeFileContextMenu();
  renderFileEntries();
  requestAnimationFrame(() => {
    const input = qs("[data-file-rename-input]");
    input?.focus();
    input?.select();
  });
}

function cancelFileRename() {
  state.fileRenamingPath = "";
  renderFileEntries();
}

async function commitFileRename(oldPath) {
  const input = qs("[data-file-rename-input]");
  const name = normalizeProjectPath(input?.value || "");
  if (!oldPath || !name || name === ".") {
    cancelFileRename();
    return;
  }
  const parent = parentProjectPath(oldPath);
  const newPath = normalizeProjectPath(parent === "." ? name : `${parent}/${name}`);
  if (newPath === oldPath) {
    cancelFileRename();
    return;
  }
  await renameFilePath(oldPath, newPath);
}

function bindFileInlineInputs(root) {
  root.querySelector("[data-file-create-input]")?.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      cancelFileCreate();
    } else if (event.key === "Enter") {
      event.preventDefault();
      commitFileCreate().catch(showError);
    }
  });
  root.querySelector("[data-file-create-input]")?.addEventListener("blur", () => {
    commitFileCreate().catch(showError);
  });
  root.querySelector("[data-file-rename-input]")?.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      cancelFileRename();
    } else if (event.key === "Enter") {
      event.preventDefault();
      commitFileRename(event.currentTarget.dataset.fileRenameInput).catch(showError);
    }
  });
  root.querySelector("[data-file-rename-input]")?.addEventListener("blur", (event) => {
    commitFileRename(event.currentTarget.dataset.fileRenameInput).catch(showError);
  });
}

function filePermissions(entry) {
  return entry.type === "directory" ? "rwxr-xr-x" : "rw-r--r--";
}

function fileIconKind(name) {
  const lower = String(name || "").toLowerCase();
  if (/\.(png|jpe?g|gif|webp|svg|ico|avif)$/.test(lower)) return "image";
  if (/\.(json|toml|ya?ml|lock)$/.test(lower)) return "code";
  if (/\.(md|markdown|txt)$/.test(lower)) return "text";
  if (/\.(rs|js|jsx|ts|tsx|css|html|py|sh|sql)$/.test(lower)) return "code";
  return "file";
}

function fileEntryHtml(entry, depth) {
  const path = escapeHtml(entry.path);
  const name = escapeHtml(entry.name);
  const isDirectory = entry.type === "directory";
  const expanded = isDirectory && state.fileExpandedPaths.has(entry.path);
  const renaming = state.fileRenamingPath === entry.path;
  const checked = state.fileSelectedPaths.has(entry.path) ? " checked" : "";
  const iconKind = isDirectory ? (expanded ? "folder-open" : "folder") : fileIconKind(entry.name);
  const size = isDirectory ? "" : escapeHtml(formatBytes(entry.size || 0));
  const modified = escapeHtml(formatRelativeTime(entry.modified));
  const permissions = escapeHtml(filePermissions(entry));
  const rowMode = escapeHtml(state.fileViewMode);
  const displayName = renaming
    ? `<input class="file-inline-input" data-file-rename-input="${path}" value="${name}" aria-label="Rename ${name}" />`
    : `<span class="file-name">${name}</span>`;
  return `<article class="file-tree-row${renaming ? " renaming" : ""} file-tree-row-${rowMode}" data-kind="${isDirectory ? "directory" : "file"}" data-file-row-path="${path}" role="button" tabindex="0" aria-label="${isDirectory ? "Open" : "Open file"} ${name}"${isDirectory ? ` aria-expanded="${expanded ? "true" : "false"}"` : ""}>
    <div class="file-tree-name" style="--file-depth:${depth}">
      <input class="file-tree-checkbox" type="checkbox" data-file-select="${path}" aria-label="Select ${name}"${checked} />
      <span class="file-disclosure${expanded ? " open" : ""}${isDirectory ? "" : " file"}" aria-hidden="true"></span>
      <span class="file-icon file-icon-${iconKind}" aria-hidden="true"></span>
      ${displayName}
      <span class="file-tree-actions">
        <button type="button" class="icon-button" data-file-menu="${path}" aria-label="File actions" title="File actions" data-symbol="dots-vertical"></button>
      </span>
    </div>
    <span class="file-tree-size">${size}</span>
    <span class="file-tree-modified">${modified}</span>
    <span class="file-tree-permissions">${permissions}</span>
  </article>`;
}

function fileContextMenuHtml(entry) {
  const isDirectory = entry.type === "directory";
  const path = escapeHtml(entry.path);
  return `<div id="file-context-menu" class="file-context-menu" role="menu">
    <button type="button" data-symbol="open" data-file-context-action="open" data-file-context-path="${path}">${isDirectory ? "Expand" : "Open"}</button>
    ${isDirectory ? `<button type="button" data-symbol="file-plus" data-file-context-action="new-file" data-file-context-path="${path}">New File</button>
    <button type="button" data-symbol="folder-plus" data-file-context-action="new-folder" data-file-context-path="${path}">New Folder</button>
    <button type="button" data-symbol="upload" data-file-context-action="upload" data-file-context-path="${path}">Upload Files</button>` : ""}
    <hr />
    <button type="button" data-symbol="rename" data-file-context-action="rename" data-file-context-path="${path}">Rename</button>
    <button type="button" data-symbol="copy" data-file-context-action="copy-path" data-file-context-path="${path}">Copy Path</button>
    ${!isDirectory ? `<button type="button" data-symbol="download" data-file-context-action="download" data-file-context-path="${path}">Download</button>` : ""}
    <button type="button" class="danger" data-symbol="trash" data-file-context-action="delete" data-file-context-path="${path}">Delete</button>
  </div>`;
}

function closeFileContextMenu() {
  state.fileContextMenu = null;
  qs("#file-context-menu")?.remove();
}

function openFileContextMenu(entry, x, y) {
  closeFileContextMenu();
  state.fileContextMenu = { path: entry.path };
  document.body.insertAdjacentHTML("beforeend", fileContextMenuHtml(entry));
  const menu = qs("#file-context-menu");
  const left = Math.min(Math.max(8, x), window.innerWidth - menu.offsetWidth - 8);
  const top = Math.min(Math.max(8, y), window.innerHeight - menu.offsetHeight - 8);
  menu.style.left = `${Math.round(left)}px`;
  menu.style.top = `${Math.round(top)}px`;
  menu.querySelectorAll("[data-file-context-action]").forEach((button) => {
    button.addEventListener("click", () => {
      handleFileContextAction(button.dataset.fileContextAction, button.dataset.fileContextPath).catch(showError);
    });
  });
}

async function handleFileContextAction(action, path) {
  const entry = findFileEntryByPath(state.fileEntries, path);
  closeFileContextMenu();
  if (!entry && !["copy-path"].includes(action)) return;
  if (action === "open") {
    if (entry.type === "directory") toggleFileDirectory(path);
    else await openFile(path);
  } else if (action === "new-file") {
    startCreateFileTreePath(false, path);
  } else if (action === "new-folder") {
    startCreateFileTreePath(true, path);
  } else if (action === "upload") {
    state.fileUploadTargetPath = normalizeProjectPath(path || ".");
    qs("#file-upload-input")?.click();
  } else if (action === "rename") {
    startRenameFilePath(path);
  } else if (action === "copy-path") {
    await copyText(path);
    showToast("File path copied", "ok");
  } else if (action === "download") {
    await downloadFilePath(path);
  } else if (action === "delete") {
    if (window.confirm(`Delete ${path}?`)) await deleteFilePath(path);
  }
}

function editorText() {
  if (state.codeEditor) {
    return state.codeEditor.getValue();
  }
  return qs("#file-editor-content").value;
}

function setEditorText(value) {
  if (state.codeEditor) {
    state.suppressEditorChange = true;
    state.codeEditor.setValue(value || "");
    state.suppressEditorChange = false;
    state.codeEditor.refresh();
    state.codeEditor.save();
  } else {
    qs("#file-editor-content").value = value || "";
  }
}

function editorCursorIndex() {
  if (state.codeEditor) {
    return state.codeEditor.indexFromPos(state.codeEditor.getCursor());
  }
  return qs("#file-editor-content").selectionStart;
}

function setEditorSelection(start, end = start) {
  if (state.codeEditor) {
    state.codeEditor.focus();
    state.codeEditor.setSelection(state.codeEditor.posFromIndex(start), state.codeEditor.posFromIndex(end));
    return;
  }
  const editor = qs("#file-editor-content");
  editor.focus();
  editor.setSelectionRange(start, end);
}

function replaceEditorRange(start, end, replacement) {
  if (state.codeEditor) {
    state.codeEditor.replaceRange(replacement, state.codeEditor.posFromIndex(start), state.codeEditor.posFromIndex(end));
    return;
  }
  qs("#file-editor-content").setRangeText(replacement, start, end, "end");
}

function editorModeForPath(filePath) {
  const extension = filePath.split(".").pop()?.toLowerCase() || "";
  if (["js", "jsx", "mjs", "cjs", "json", "ts", "tsx"].includes(extension)) return "javascript";
  if (["html", "htm"].includes(extension)) return "htmlmixed";
  if (["css", "scss", "sass", "less"].includes(extension)) return "css";
  if (["md", "markdown"].includes(extension)) return "gfm";
  if (extension === "rs") return "rust";
  if (["sh", "bash", "zsh", "fish"].includes(extension)) return "shell";
  if (["py", "pyw"].includes(extension)) return "python";
  if (["sql"].includes(extension)) return "sql";
  if (["yaml", "yml"].includes(extension)) return "yaml";
  if (extension === "toml") return "toml";
  if (["xml", "svg"].includes(extension)) return "xml";
  return null;
}

function refreshEditorWidget(filePath = qs("#file-editor-path").value.trim()) {
  if (!state.codeEditor) return;
  state.codeEditor.setOption("mode", editorModeForPath(filePath));
  state.codeEditor.setOption("lineWrapping", !!state.preferences.wrapOutput);
  window.setTimeout(() => state.codeEditor?.refresh(), 0);
}

async function openFile(filePath) {
  await loadFileContent(filePath);
}

async function loadFileContent(filePath, options = {}) {
  const project = activeProjectName();
  if (!project) return;
  if (!options.skipDirtyCheck && !confirmDiscardDirtyFile()) return;
  const body = await api(`/api/projects/${encodeURIComponent(project)}/files/content?path=${encodeURIComponent(filePath)}`);
  qs("#file-editor-path").value = body.path;
  setEditorText(body.content || "");
  refreshEditorWidget(body.path);
  resetEditorSearch();
  state.currentFileDirty = false;
  updateEditorChrome();
}

async function saveFile(event) {
  event.preventDefault();
  const project = activeProjectName();
  const filePath = qs("#file-editor-path").value.trim();
  if (!project || !filePath) return;
  await api(`/api/projects/${encodeURIComponent(project)}/file`, {
    method: "PUT",
    body: JSON.stringify({
      filePath,
      content: editorText(),
    }),
  });
  state.currentFileDirty = false;
  updateEditorChrome();
  await loadFiles();
  showToast(`Saved ${filePath}`, "ok");
}

async function createPath(directory) {
  const project = activeProjectName();
  const filePath = qs("#file-editor-path").value.trim();
  if (!project || !filePath) return;
  await api(`/api/projects/${encodeURIComponent(project)}/files/create`, {
    method: "POST",
    body: JSON.stringify({
      filePath,
      content: directory ? "" : editorText(),
      directory,
    }),
  });
  await loadFiles();
  showToast(`${directory ? "Directory" : "File"} created`, "ok");
}

async function deletePath() {
  const filePath = qs("#file-editor-path").value.trim();
  if (!filePath) return;
  if (!window.confirm(`Delete ${filePath}?`)) return;
  await deleteFilePath(filePath);
}

async function deleteFilePath(filePath) {
  const project = activeProjectName();
  if (!project || !filePath) return;
  await api(`/api/projects/${encodeURIComponent(project)}/files`, {
    method: "DELETE",
    body: JSON.stringify({ filePath }),
  });
  if (qs("#file-editor-path").value.trim() === filePath) {
    qs("#file-editor-path").value = "";
    setEditorText("");
    state.currentFileDirty = false;
    updateEditorChrome();
  }
  await loadFiles();
  showToast(`Deleted ${filePath}`, "ok");
}

async function renamePath() {
  const oldPath = qs("#file-editor-path").value.trim();
  const newPath = qs("#file-rename-path").value.trim();
  await renameFilePath(oldPath, newPath);
  qs("#file-rename-path").value = "";
}

async function renameFilePath(oldPath, newPath) {
  const project = activeProjectName();
  if (!project || !oldPath || !newPath) return;
  await api(`/api/projects/${encodeURIComponent(project)}/files/rename`, {
    method: "PUT",
    body: JSON.stringify({ oldPath, newPath }),
  });
  if (qs("#file-editor-path").value.trim() === oldPath) {
    qs("#file-editor-path").value = newPath;
    refreshEditorWidget(newPath);
    updateEditorChrome();
  }
  state.fileRenamingPath = "";
  await loadFiles();
  showToast(`Renamed to ${newPath}`, "ok");
}

async function uploadProjectFiles() {
  const project = activeProjectName();
  const files = [...qs("#file-upload-input").files];
  if (!project || !files.length) return;
  const formData = new FormData();
  formData.append("targetPath", state.fileUploadTargetPath || qs("#files-path").value.trim() || ".");
  files.forEach((file) => formData.append("files", file));
  const body = await apiUpload(`/api/projects/${encodeURIComponent(project)}/files/upload`, formData);
  setEditorText(JSON.stringify(body, null, 2));
  state.currentFileDirty = false;
  updateEditorChrome();
  qs("#file-upload-input").value = "";
  state.fileUploadTargetPath = "";
  await loadFiles();
  showToast(`Uploaded ${files.length} file${files.length === 1 ? "" : "s"}`, "ok");
}

async function uploadProjectFolder() {
  const project = activeProjectName();
  const files = [...qs("#folder-upload-input").files];
  if (!project || !files.length) return;
  const relativePaths = files.map((file) => normalizeProjectPath(file.webkitRelativePath || file.name));
  const formData = new FormData();
  formData.append("targetPath", qs("#files-path").value.trim() || ".");
  formData.append("relativePaths", JSON.stringify(relativePaths));
  files.forEach((file) => formData.append("files", file));
  const body = await apiUpload(`/api/projects/${encodeURIComponent(project)}/files/upload`, formData);
  setEditorText(JSON.stringify(body, null, 2));
  qs("#folder-upload-input").value = "";
  state.currentFileDirty = false;
  updateEditorChrome();
  await loadFiles();
  showToast(`Uploaded ${files.length} folder file${files.length === 1 ? "" : "s"}`, "ok");
}

function downloadCurrentFile() {
  const filePath = qs("#file-editor-path").value.trim();
  if (!filePath) return;
  const blob = new Blob([editorText()], { type: "text/plain;charset=utf-8" });
  const link = document.createElement("a");
  link.href = URL.createObjectURL(blob);
  link.download = filePath.split("/").filter(Boolean).pop() || "download.txt";
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(link.href);
}

async function downloadFilePath(filePath) {
  const project = activeProjectName();
  if (!project || !filePath) return;
  const body = await api(`/api/projects/${encodeURIComponent(project)}/files/content?path=${encodeURIComponent(filePath)}`);
  const blob = new Blob([body.content || ""], { type: "text/plain;charset=utf-8" });
  const link = document.createElement("a");
  link.href = URL.createObjectURL(blob);
  link.download = filePath.split("/").filter(Boolean).pop() || "download.txt";
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(link.href);
}

async function reloadCurrentFile() {
  const filePath = qs("#file-editor-path").value.trim();
  if (!filePath || !confirmDiscardDirtyFile()) return;
  await loadFileContent(filePath, { skipDirtyCheck: true });
}

async function copyCurrentFilePath(event) {
  const filePath = qs("#file-editor-path").value.trim();
  if (!filePath || !navigator.clipboard?.writeText) return;
  await navigator.clipboard.writeText(filePath);
  showToast("Copied file path", "ok");
  const button = event?.currentTarget;
  if (!button) return;
  const label = button.textContent;
  button.textContent = "Copied";
  window.setTimeout(() => {
    button.textContent = label;
  }, 900);
}

function confirmDiscardDirtyFile() {
  return !state.currentFileDirty || window.confirm("Discard unsaved file changes?");
}

function updateEditorChrome() {
  const value = editorText();
  const lineCount = Math.max(1, value.split("\n").length);
  qs("#file-editor-lines").textContent = Array.from({ length: lineCount }, (_, index) => index + 1).join("\n");
  const beforeCursor = value.slice(0, editorCursorIndex());
  const line = beforeCursor.split("\n").length;
  const col = beforeCursor.length - beforeCursor.lastIndexOf("\n");
  const filePath = qs("#file-editor-path").value.trim();
  document.body.classList.toggle("files-editor-open", !!filePath);
  qs("#file-editor-position").textContent = `Ln ${line}, Col ${col}`;
  qs("#file-editor-status").textContent = filePath
    ? `${state.currentFileDirty ? "Unsaved" : "Saved"} · ${filePath}`
    : "No file loaded";
  updateEditorSearchStatus();
}

function handleEditorKeydown(event) {
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
    event.preventDefault();
    saveFile(event).catch(showError);
    return;
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
    event.preventDefault();
    qs("#editor-search")?.focus();
    qs("#editor-search")?.select();
    return;
  }
  if (event.key !== "Tab") return;
  event.preventDefault();
  const editor = event.currentTarget;
  const start = editor.selectionStart;
  const end = editor.selectionEnd;
  const indent = "  ";
  editor.value = `${editor.value.slice(0, start)}${indent}${editor.value.slice(end)}`;
  editor.selectionStart = editor.selectionEnd = start + indent.length;
  state.currentFileDirty = true;
  resetEditorSearch();
  updateEditorChrome();
}

function resetEditorSearch() {
  state.editorSearch = {
    query: qs("#editor-search")?.value || "",
    matches: [],
    current: -1,
  };
  updateEditorSearchStatus();
}

function refreshEditorSearchMatches() {
  const query = qs("#editor-search")?.value || "";
  const value = editorText();
  if (state.editorSearch.query === query && state.editorSearch.matches.length) {
    return;
  }
  const matches = [];
  if (query) {
    let index = value.indexOf(query);
    while (index !== -1 && matches.length < 10_000) {
      matches.push(index);
      index = value.indexOf(query, index + Math.max(1, query.length));
    }
  }
  state.editorSearch = { query, matches, current: -1 };
  updateEditorSearchStatus();
}

function updateEditorSearchStatus() {
  const target = qs("#editor-search-status");
  if (!target) return;
  const { query, matches, current } = state.editorSearch;
  if (!query) {
    target.textContent = "No search";
  } else if (!matches.length) {
    target.textContent = "0 matches";
  } else {
    target.textContent = `${current >= 0 ? current + 1 : 0} of ${matches.length}`;
  }
}

function findEditorMatch(direction = 1) {
  refreshEditorSearchMatches();
  const { query, matches } = state.editorSearch;
  if (!query || !matches.length) return;
  let next = state.editorSearch.current;
  if (next < 0) {
    next = direction >= 0
      ? matches.findIndex((index) => index >= editorCursorIndex())
      : findLastIndex(matches, (index) => index < editorCursorIndex());
    if (next < 0) next = direction >= 0 ? 0 : matches.length - 1;
  } else {
    next = (next + direction + matches.length) % matches.length;
  }
  selectEditorMatch(next);
}

function selectEditorMatch(matchIndex) {
  const start = state.editorSearch.matches[matchIndex];
  if (start === undefined) return;
  state.editorSearch.current = matchIndex;
  setEditorSelection(start, start + state.editorSearch.query.length);
  updateEditorChrome();
}

function replaceEditorMatch() {
  refreshEditorSearchMatches();
  if (!state.editorSearch.query || !state.editorSearch.matches.length) return;
  if (state.editorSearch.current < 0) {
    findEditorMatch(1);
  }
  const start = state.editorSearch.matches[state.editorSearch.current];
  const end = start + state.editorSearch.query.length;
  if (start === undefined || editorText().slice(start, end) !== state.editorSearch.query) return;
  const replacement = qs("#editor-replace").value;
  replaceEditorRange(start, end, replacement);
  state.currentFileDirty = true;
  resetEditorSearch();
  findEditorMatch(1);
}

function replaceAllEditorMatches() {
  const query = qs("#editor-search")?.value || "";
  if (!query) return;
  const parts = editorText().split(query);
  const count = parts.length - 1;
  if (!count) return;
  setEditorText(parts.join(qs("#editor-replace").value));
  state.currentFileDirty = true;
  resetEditorSearch();
  updateEditorChrome();
}

function goToEditorLine() {
  const lineNumber = Math.max(1, Number(qs("#editor-goto-line")?.value) || 1);
  const lines = editorText().split("\n");
  const targetLine = Math.min(lineNumber, lines.length);
  const index = lines.slice(0, targetLine - 1).reduce((total, line) => total + line.length + 1, 0);
  setEditorSelection(index);
  updateEditorChrome();
}

function findLastIndex(items, predicate) {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    if (predicate(items[index], index)) return index;
  }
  return -1;
}

async function loadGitStatus() {
  const project = activeProjectName();
  if (!project) return;
  qs("#git-files").innerHTML = '<p class="empty">Loading source control.</p>';
  qs("#git-output").innerHTML = "";
  let body;
  try {
    body = await api(`/api/git/status?project=${encodeURIComponent(project)}`);
  } catch (error) {
    state.gitStatus = null;
    state.gitSelectedFiles = new Set();
    renderGitSummary(null);
    qs("#git-files").innerHTML = "";
    setOutput("#git-output", error.message || String(error), "error-output");
    return;
  }
  state.gitStatus = body;
  const nextFiles = gitFilesFromStatus(body);
  const available = new Set(nextFiles.map((file) => file.path));
  state.gitSelectedFiles = new Set([...state.gitSelectedFiles].filter((path) => available.has(path)));
  if (body.branch && !qs("#git-branch").value.trim()) {
    qs("#git-branch").value = body.branch;
  }
  renderGitSummary(body);
  renderGitFiles();
  if (state.gitActiveView === "changes") {
    const previewFile = state.currentGitDiffFile && available.has(state.currentGitDiffFile)
      ? state.currentGitDiffFile
      : nextFiles[0]?.path;
    if (previewFile) {
      await gitDiffForFile(previewFile);
    } else {
      renderGitStatus(body);
    }
  } else {
    setGitActiveView(state.gitActiveView, { load: true });
  }
}

function renderGitSummary(status = state.gitStatus) {
  const target = qs("#git-summary");
  const count = gitFilesFromStatus(status).length;
  const countTarget = qs("#git-change-count");
  if (countTarget) countTarget.textContent = String(count);
  if (target) target.innerHTML = "";
}

function setGitActiveView(view, options = {}) {
  const nextView = ["changes", "history", "branches"].includes(view) ? view : "changes";
  state.gitActiveView = nextView;
  const panel = qs("#git-view");
  if (panel) panel.dataset.gitView = nextView;
  document.querySelectorAll("[data-git-view-tab]").forEach((button) => {
    const active = button.dataset.gitViewTab === nextView;
    button.classList.toggle("active", active);
    button.setAttribute("aria-selected", active ? "true" : "false");
  });
  if (nextView === "changes") {
    renderGitFiles();
    if (state.gitStatus) renderGitStatus(state.gitStatus);
    return;
  }
  if (options.load === false) return;
  if (nextView === "history") {
    gitRead("/api/git/commits?limit=25", renderGitCommits).catch(showError);
  } else if (nextView === "branches") {
    gitRead("/api/git/branches", renderGitBranches).catch(showError);
  }
}

function gitFilesFromStatus(status = state.gitStatus) {
  if (!status) return [];
  if (Array.isArray(status.files) && status.files.length) return status.files;
  const groups = [
    ["modified", "M"],
    ["conflicted", "UU"],
    ["added", "A"],
    ["deleted", "D"],
    ["untracked", "U"],
  ];
  return groups.flatMap(([key, code]) => (status[key] || []).map((path) => ({ path, status: code })));
}

function gitStatusLabel(status) {
  if (isGitConflictStatus(status)) return "Conflicted";
  if (status === "M") return "Modified";
  if (status === "A") return "Added";
  if (status === "D") return "Deleted";
  if (status === "U" || status === "??") return "Untracked";
  return status || "Changed";
}

function gitStatusClass(status) {
  if (isGitConflictStatus(status)) return "status-conflict";
  if (status === "A") return "status-a";
  if (status === "D") return "status-d";
  if (status === "U" || status === "??") return "status-u";
  return "status-m";
}

function createGitFolderNode(name = "", path = "") {
  return { name, path, folders: [], files: [] };
}

function buildGitFileTree(files) {
  const root = createGitFolderNode();
  const folderMap = new Map([["", root]]);
  files.forEach((file) => {
    const parts = String(file.path || "").split("/").filter(Boolean);
    if (!parts.length) return;
    let parent = root;
    let currentPath = "";
    parts.slice(0, -1).forEach((part) => {
      currentPath = currentPath ? `${currentPath}/${part}` : part;
      let folder = folderMap.get(currentPath);
      if (!folder) {
        folder = createGitFolderNode(part, currentPath);
        folderMap.set(currentPath, folder);
        parent.folders.push(folder);
      }
      parent = folder;
    });
    parent.files.push({ ...file, name: parts.at(-1) || file.path });
  });
  const sort = (node) => {
    node.folders.sort((a, b) => a.name.localeCompare(b.name));
    node.files.sort((a, b) => a.name.localeCompare(b.name));
    node.folders.forEach(sort);
  };
  sort(root);
  return root;
}

function countGitFolderFiles(node) {
  return node.files.length + node.folders.reduce((total, folder) => total + countGitFolderFiles(folder), 0);
}

function gitFolderFiles(node) {
  return [...node.files.map((file) => file.path), ...node.folders.flatMap(gitFolderFiles)];
}

function gitChangeSectionHtml(group, label, files, emptyText, actionLabel) {
  const action = actionLabel
    ? `<button type="button" data-git-section-action="${group}">${escapeHtml(actionLabel)}</button>`
    : "";
  const body = files.length
    ? gitTreeHtml(buildGitFileTree(files), group, 0)
    : `<div class="git-change-empty">${escapeHtml(emptyText)}</div>`;
  return `<section class="git-change-section" data-git-change-section="${escapeHtml(group)}">
    <header class="git-change-header">
      <span>${escapeHtml(label)} (${files.length})</span>
      ${action}
    </header>
    ${body}
  </section>`;
}

function gitTreeHtml(node, group, depth) {
  const folders = node.folders.map((folder) => gitFolderHtml(folder, group, depth)).join("");
  const files = node.files.map((file) => gitFileRowHtml(file, depth)).join("");
  return `${folders}${files}`;
}

function gitFolderHtml(folder, group, depth) {
  const key = `${group}:${folder.path}`;
  const collapsed = state.gitCollapsedFolders.has(key);
  const files = gitFolderFiles(folder).join("\n");
  return `<div class="git-folder-block">
    <button type="button" class="git-folder-row" data-git-folder-toggle="${escapeHtml(key)}" aria-expanded="${collapsed ? "false" : "true"}" style="padding-left:${12 + depth * 16}px">
      <span class="git-folder-main">
        <span class="git-folder-chevron" aria-hidden="true"></span>
        <strong>${escapeHtml(folder.name)}</strong>
      </span>
      <span class="git-change-count">${countGitFolderFiles(folder)}</span>
    </button>
    ${collapsed ? "" : gitTreeHtml(folder, group, depth + 1)}
    <template data-git-folder-files="${escapeHtml(key)}">${escapeHtml(files)}</template>
  </div>`;
}

function gitFileRowHtml(file, depth) {
  const active = state.currentGitDiffFile === file.path ? " active" : "";
  const statusLabel = gitStatusLabel(file.status);
  const statusClass = gitStatusClass(file.status);
  const checked = state.gitSelectedFiles.has(file.path) ? " checked" : "";
  const isUntracked = file.status === "U" || file.status === "??";
  return `<article class="git-file-row${active}${isGitConflictStatus(file.status) ? " conflicted" : ""}" data-git-file-row="${escapeHtml(file.path)}" style="padding-left:${12 + depth * 16}px">
    <input type="checkbox" data-git-file="${escapeHtml(file.path)}" aria-label="Stage ${escapeHtml(file.path)}"${checked} />
    <button type="button" class="git-file-main" data-git-file-preview="${escapeHtml(file.path)}" title="${escapeHtml(file.path)}">
      <span class="git-file-icon" aria-hidden="true"></span>
      <strong>${escapeHtml(file.name || file.path)}</strong>
    </button>
    <span class="git-row-actions">
      <button type="button" class="icon-button" data-git-open-file="${escapeHtml(file.path)}" aria-label="Open file" title="Open file" data-symbol="open"></button>
      <button type="button" class="icon-button" data-git-file-diff="${escapeHtml(file.path)}" aria-label="Show diff" title="Show diff" data-symbol="diff"></button>
      ${isGitConflictStatus(file.status) ? `<button type="button" class="icon-button" data-git-conflict-file="${escapeHtml(file.path)}" aria-label="Resolve conflict" title="Resolve conflict" data-symbol="alert"></button>` : ""}
      ${["M", "D", "U", "??"].includes(file.status) || isGitConflictStatus(file.status)
    ? `<button type="button" class="icon-button" data-git-file-action="${escapeHtml(file.path)}" data-git-file-status="${escapeHtml(file.status)}" aria-label="${isUntracked ? "Delete untracked file" : "Discard changes"}" title="${isUntracked ? "Delete untracked file" : "Discard changes"}" data-symbol="trash"></button>`
    : ""}
      <span class="git-status-badge ${statusClass}" title="${escapeHtml(statusLabel)}">${escapeHtml(file.status)}</span>
    </span>
  </article>`;
}

function bindGitFileTree(root) {
  root.querySelector("[data-git-open-commit]")?.addEventListener("click", openGitCommitModal);
  root.querySelectorAll("[data-git-section-action]").forEach((button) => {
    button.addEventListener("click", () => {
      const files = gitFilesFromStatus(state.gitStatus).map((file) => file.path);
      if (button.dataset.gitSectionAction === "changes") {
        state.gitSelectedFiles = new Set(files);
      } else {
        state.gitSelectedFiles = new Set();
      }
      renderGitFiles();
    });
  });
  root.querySelectorAll("[data-git-folder-toggle]").forEach((button) => {
    button.addEventListener("click", () => {
      const key = button.dataset.gitFolderToggle;
      if (state.gitCollapsedFolders.has(key)) state.gitCollapsedFolders.delete(key);
      else state.gitCollapsedFolders.add(key);
      renderGitFiles();
    });
  });
  root.querySelectorAll("[data-git-file]").forEach((input) => {
    input.addEventListener("change", () => {
      if (input.checked) state.gitSelectedFiles.add(input.dataset.gitFile);
      else state.gitSelectedFiles.delete(input.dataset.gitFile);
      renderGitFiles();
    });
  });
  root.querySelectorAll("[data-git-file-preview], [data-git-file-diff]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.preventDefault();
      gitDiffForFile(button.dataset.gitFilePreview || button.dataset.gitFileDiff).catch(showError);
    });
  });
  root.querySelectorAll("[data-git-open-file]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.preventDefault();
      openGitChangedFile(button.dataset.gitOpenFile).catch(showError);
    });
  });
  root.querySelectorAll("[data-git-file-action]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.preventDefault();
      requestGitFileAction(button.dataset.gitFileAction, button.dataset.gitFileStatus).catch(showError);
    });
  });
  root.querySelectorAll("[data-git-conflict-file]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.preventDefault();
      loadGitConflictFile(button.dataset.gitConflictFile).catch(showError);
    });
  });
}

function renderGitFiles() {
  const files = gitFilesFromStatus(state.gitStatus).filter((file) => {
    const filter = qs("#git-filter")?.value.trim().toLowerCase() || "";
    return !filter || `${file.status} ${file.path}`.toLowerCase().includes(filter);
  });
  const target = qs("#git-files");
  if (!files.length) {
    target.innerHTML = `<div class="git-commit-inline">
      <span>0 files selected</span>
      <button type="button" disabled data-symbol="check">Commit</button>
    </div>
    <div class="git-change-empty">Working tree is clean.</div>`;
    return;
  }
  const staged = files.filter((file) => state.gitSelectedFiles.has(file.path));
  const changes = files.filter((file) => !state.gitSelectedFiles.has(file.path));
  target.innerHTML = `<div class="git-commit-inline">
      <span>${staged.length} file${staged.length === 1 ? "" : "s"} selected</span>
      <button type="button" data-git-open-commit${staged.length ? "" : " disabled"} data-symbol="check">Commit</button>
    </div>
    ${gitChangeSectionHtml("staged", "Staged", staged, "No staged files", staged.length ? "Unstage All" : "")}
    ${gitChangeSectionHtml("changes", "Changes", changes, changes.length ? "" : "All changes staged", changes.length ? "Stage All" : "")}`;
  bindGitFileTree(target);
}

function renderGitStatus(status) {
  const target = qs("#git-output");
  target.className = "output-panel git-output";
  const files = gitFilesFromStatus(status);
  target.innerHTML = files.length
    ? '<div class="git-change-empty">Select a changed file to preview its diff.</div>'
    : '<div class="git-change-empty">No changes detected.</div>';
}

function isGitConflictStatus(status = "") {
  return ["DD", "AU", "UD", "UA", "DU", "AA", "UU"].includes(status)
    || String(status).slice(0, 2).includes("U");
}

function selectedGitFiles() {
  return [...state.gitSelectedFiles];
}

function setGitFileSelection(checked) {
  const files = gitFilesFromStatus(state.gitStatus);
  state.gitSelectedFiles = checked ? new Set(files.map((file) => file.path)) : new Set();
  document.querySelectorAll("[data-git-file]").forEach((input) => {
    input.checked = checked;
  });
  renderGitFiles();
}

async function openGitChangedFile(file) {
  if (!file) return;
  if (await switchView("files")) {
    await openFile(file);
  }
}

async function requestGitFileAction(file, status) {
  if (!file) return;
  const untracked = status === "U" || status === "??";
  const message = untracked
    ? `Delete untracked file "${file}"?`
    : `Discard changes to "${file}"?`;
  if (!window.confirm(message)) return;
  await gitFileOperation(untracked ? "/api/git/delete-untracked" : "/api/git/discard", file);
}

async function gitFileOperation(path, file) {
  const project = activeProjectName();
  if (!project || !file) return;
  const body = await api(path, {
    method: "POST",
    body: JSON.stringify({ project, file }),
  });
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
}

function closeGitCommitModal() {
  state.gitCommitMessage = qs("#git-commit-message-input")?.value || state.gitCommitMessage || "";
  qs("#git-commit-modal")?.remove();
}

function openGitCommitModal() {
  const files = selectedGitFiles();
  if (!files.length) return;
  qs("#git-commit-modal")?.remove();
  const message = escapeHtml(state.gitCommitMessage || qs("#git-message")?.value || "");
  document.body.insertAdjacentHTML("beforeend", `<div id="git-commit-modal" class="git-commit-modal">
    <section class="git-commit-dialog" role="dialog" aria-modal="true" aria-labelledby="git-commit-title">
      <header>
        <div>
          <h3 id="git-commit-title">Commit Changes</h3>
          <span class="meta">${files.length} file${files.length === 1 ? "" : "s"} selected</span>
        </div>
        <button type="button" class="icon-button" data-git-commit-close aria-label="Close" title="Close" data-symbol="close"></button>
      </header>
      <textarea id="git-commit-message-input" placeholder="Message (Ctrl+Enter to commit)">${message}</textarea>
      <div class="button-row">
        <button type="button" class="icon-button" data-git-commit-generate aria-label="Generate commit message" title="Generate commit message" data-symbol="sparkles"></button>
        <span class="grow"></span>
        <button type="button" data-git-commit-close>Cancel</button>
        <button type="button" class="primary-action" data-git-commit-submit>Commit</button>
      </div>
    </section>
  </div>`);
  const modal = qs("#git-commit-modal");
  const input = qs("#git-commit-message-input");
  input?.focus();
  input?.setSelectionRange(input.value.length, input.value.length);
  modal.addEventListener("click", (event) => {
    if (event.target === modal) closeGitCommitModal();
  });
  modal.querySelectorAll("[data-git-commit-close]").forEach((button) => {
    button.addEventListener("click", closeGitCommitModal);
  });
  modal.querySelector("[data-git-commit-generate]")?.addEventListener("click", (event) => {
    withButtonLoading(event.currentTarget, async () => {
      await generateGitMessage();
      input.value = qs("#git-message").value;
      state.gitCommitMessage = input.value;
    }).catch(showError);
  });
  modal.querySelector("[data-git-commit-submit]")?.addEventListener("click", (event) => {
    withButtonLoading(event.currentTarget, async () => {
      state.gitCommitMessage = input.value.trim();
      qs("#git-message").value = state.gitCommitMessage;
      await commitGitSelection();
      closeGitCommitModal();
    }).catch(showError);
  });
  input?.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closeGitCommitModal();
    } else if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      state.gitCommitMessage = input.value.trim();
      qs("#git-message").value = state.gitCommitMessage;
      commitGitSelection().then(closeGitCommitModal).catch(showError);
    }
  });
}

function selectedSessionId() {
  return qs("#session-id-input")?.value.trim()
    || state.pendingChatSessionId
    || state.sessions[0]?.id
    || "";
}

function renderSearchResults(body) {
  const results = firstDefined(body.results, body.conversations, body.sessions, body.items, []);
  if (!Array.isArray(results) || !results.length) {
    setOutput("#sessions-output", "No matching conversations.", "empty-output");
    return;
  }
  const target = qs("#sessions-output");
  target.className = "output-panel result-list";
  target.innerHTML = results.map((result) => {
    const id = firstDefined(result.sessionId, result.session_id, result.id, "");
    const title = firstDefined(result.title, result.summary, id, "Conversation");
    const meta = [result.provider, result.projectPath, result.timestamp, result.updatedAt]
      .filter(Boolean)
      .join(" · ");
    const excerpt = firstDefined(result.excerpt, result.content, result.preview, "");
    return `<article class="result-row">
      <strong>${escapeHtml(title)}</strong>
      <span class="meta">${escapeHtml(meta)}</span>
      ${excerpt ? `<div class="message-body">${renderMarkdownLite(excerpt)}</div>` : ""}
      ${id ? `<button type="button" data-session-result="${escapeHtml(id)}">Use Session</button>` : ""}
    </article>`;
  }).join("");
  target.querySelectorAll("[data-session-result]").forEach((button) => {
    button.addEventListener("click", () => {
      qs("#session-id-input").value = button.dataset.sessionResult;
      loadSessionMessages().catch(showError);
    });
  });
}

function renderSessionMessages(body = null) {
  if (body) {
    state.lastSessionMessages = Array.isArray(body) ? body : body.messages || [];
  }
  const messages = state.lastSessionMessages;
  const target = qs("#sessions-output");
  target.className = "output-panel message-list";
  if (!messages.length) {
    target.textContent = "No messages for this session.";
    return;
  }
  renderVirtualList(target, "sessionMessages", messages, {
    rowHeight: 180,
    maxHeight: 640,
    minRows: 4,
    render: (message) => {
      const role = firstDefined(message.role, "message");
      const timestamp = message.timestamp ? new Date(message.timestamp).toLocaleString() : "";
      return `<article class="message ${escapeHtml(role)}">
        <header>
          <strong>${escapeHtml(role)}</strong>
          <span>${escapeHtml(timestamp)}</span>
        </header>
        <div class="message-body">${renderMarkdownLite(message.content || "")}</div>
      </article>`;
    },
  });
}

async function uploadChatImages() {
  const project = activeProjectName();
  const files = [...qs("#chat-image-input").files];
  if (!project || !files.length) return;
  const formData = new FormData();
  files.forEach((file) => formData.append("images", file));
  const body = await apiUpload(`/api/projects/${encodeURIComponent(project)}/upload-images`, formData);
  state.chatImages = [...state.chatImages, ...(body.images || [])].slice(-5);
  qs("#chat-image-input").value = "";
  renderChatImages();
  showToast(`Attached ${files.length} image${files.length === 1 ? "" : "s"}`, "ok");
}

function clearChatImages() {
  state.chatImages = [];
  qs("#chat-image-input").value = "";
  renderChatImages();
}

function renderChatImages() {
  const target = qs("#chat-image-preview");
  if (!target) return;
  target.innerHTML = state.chatImages.length
    ? state.chatImages.map((image) => `<article class="image-preview">
      <img src="${escapeHtml(image.data)}" alt="${escapeHtml(image.name || "attached image")}" />
      <span>${escapeHtml(image.name || "image")} · ${escapeHtml(formatBytes(image.size || 0))}</span>
    </article>`).join("")
    : "";
  updateChatComposerState();
}

function chatPromptWithImages(prompt) {
  if (!state.chatImages.length) return prompt;
  const imageMarkdown = state.chatImages
    .map((image) => `![${image.name || "attached image"}](${image.data})`)
    .join("\n");
  return `${prompt}\n\n${imageMarkdown}`;
}

function autosizeChatPrompt() {
  const input = qs("#chat-prompt");
  if (!input) return;
  input.style.height = "0px";
  const styles = getComputedStyle(input);
  const maxHeight = Number.parseFloat(styles.maxHeight) || 180;
  const minHeight = Number.parseFloat(styles.minHeight) || 50;
  input.style.height = `${Math.min(maxHeight, Math.max(minHeight, input.scrollHeight))}px`;
  updateChatComposerState();
}

function updateChatComposerState() {
  const input = qs("#chat-prompt");
  const clear = qs("#clear-chat");
  const submit = qs("#chat-submit");
  const hasPrompt = Boolean(input?.value.trim());
  const canSubmit = hasPrompt || state.chatImages.length > 0;

  if (clear) {
    clear.classList.toggle("is-empty", !hasPrompt);
    clear.setAttribute("aria-hidden", hasPrompt ? "false" : "true");
    clear.tabIndex = hasPrompt ? 0 : -1;
  }
  if (submit) submit.disabled = !canSubmit;
}

function formatBytes(value) {
  const bytes = Number(value) || 0;
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}

function renderMarkdownLite(value) {
  const lines = String(value || "").replace(/\r\n?/g, "\n").split("\n");
  const html = [];
  let inCode = false;
  let listMode = "";
  const closeList = () => {
    if (!listMode) return;
    html.push(`</${listMode}>`);
    listMode = "";
  };
  const openList = (mode) => {
    if (listMode === mode) return;
    closeList();
    listMode = mode;
    html.push(`<${mode}>`);
  };

  for (const line of lines) {
    if (line.trim().startsWith("```")) {
      closeList();
      html.push(inCode ? "</code></pre>" : `<pre class="markdown-code"><code>`);
      inCode = !inCode;
      continue;
    }
    if (inCode) {
      html.push(`${escapeHtml(line)}\n`);
      continue;
    }
    if (!line.trim()) {
      closeList();
      continue;
    }
    const heading = line.match(/^(#{1,6})\s+(.+)$/);
    if (heading) {
      closeList();
      const level = Math.min(4, heading[1].length + 2);
      html.push(`<h${level}>${renderMarkdownInline(heading[2])}</h${level}>`);
      continue;
    }
    const quote = line.match(/^>\s?(.*)$/);
    if (quote) {
      closeList();
      html.push(`<blockquote>${renderMarkdownInline(quote[1])}</blockquote>`);
      continue;
    }
    const unordered = line.match(/^\s*[-*]\s+(.+)$/);
    if (unordered) {
      openList("ul");
      html.push(`<li>${renderMarkdownInline(unordered[1])}</li>`);
      continue;
    }
    const ordered = line.match(/^\s*\d+\.\s+(.+)$/);
    if (ordered) {
      openList("ol");
      html.push(`<li>${renderMarkdownInline(ordered[1])}</li>`);
      continue;
    }
    closeList();
    html.push(`<p>${renderMarkdownInline(line)}</p>`);
  }
  closeList();
  if (inCode) html.push("</code></pre>");
  return html.join("");
}

function renderMarkdownInline(value) {
  const pattern = /(`([^`]+)`|\*\*([^*]+)\*\*|__([^_]+)__|\[([^\]\n]+)\]\(([^)\s]+)\))/g;
  let html = "";
  let index = 0;
  for (const match of value.matchAll(pattern)) {
    html += escapeHtml(value.slice(index, match.index));
    if (match[2] !== undefined) {
      html += `<code>${escapeHtml(match[2])}</code>`;
    } else if (match[3] !== undefined || match[4] !== undefined) {
      html += `<strong>${escapeHtml(match[3] ?? match[4])}</strong>`;
    } else if (match[5] !== undefined && match[6] !== undefined) {
      const href = safeMarkdownUrl(match[6]);
      html += href
        ? `<a href="${escapeHtml(href)}" target="_blank" rel="noreferrer noopener">${escapeHtml(match[5])}</a>`
        : escapeHtml(match[0]);
    }
    index = match.index + match[0].length;
  }
  html += escapeHtml(value.slice(index));
  return html;
}

function safeMarkdownUrl(raw) {
  try {
    const url = new URL(raw, window.location.href);
    if (["http:", "https:", "mailto:"].includes(url.protocol)) {
      return url.href;
    }
  } catch {
    return "";
  }
  return "";
}

async function searchSessions(event) {
  event.preventDefault();
  const q = qs("#session-search").value.trim();
  if (q.length < 2) return;
  const body = await api(`/api/search/conversations?q=${encodeURIComponent(q)}&limit=25`);
  renderSearchResults(body);
}

async function loadSessionMessages() {
  const sessionId = selectedSessionId();
  if (!sessionId) return;
  const body = await api(`/api/sessions/${encodeURIComponent(sessionId)}/messages`);
  resetVirtualList("sessionMessages");
  renderSessionMessages(body);
}

async function loadSessionModel() {
  const sessionId = selectedSessionId();
  if (!sessionId) return;
  const provider = qs("#session-provider").value;
  const body = await api(`/api/sessions/${encodeURIComponent(sessionId)}/model?provider=${encodeURIComponent(provider)}`);
  qs("#session-model-input").value = body.model || "";
  renderJson("#sessions-output", body);
}

async function updateSessionModel() {
  const sessionId = selectedSessionId();
  const model = qs("#session-model-input").value.trim();
  if (!sessionId || !model) return;
  const body = await api(`/api/sessions/${encodeURIComponent(sessionId)}/model`, {
    method: "PUT",
    body: JSON.stringify({
      provider: qs("#session-provider").value,
      model,
    }),
  });
  renderJson("#sessions-output", body);
  await loadProjects().catch(() => {});
}

async function loadProjectSessions() {
  const project = activeProjectName();
  if (!project) return;
  const body = await api(`/api/projects/${encodeURIComponent(project)}/sessions`);
  state.sessions = Array.isArray(body) ? body : body.sessions || [];
  renderSessions();
  renderJson("#sessions-output", { project, sessions: state.sessions });
}

async function loadSessionTokenUsage() {
  const project = activeProjectName();
  const sessionId = selectedSessionId();
  if (!project || !sessionId) return;
  const provider = qs("#session-provider").value;
  const body = await api(`/api/projects/${encodeURIComponent(project)}/sessions/${encodeURIComponent(sessionId)}/token-usage?provider=${encodeURIComponent(provider)}`);
  renderJson("#sessions-output", body);
}

async function renameSelectedSession() {
  const sessionId = selectedSessionId();
  const summary = qs("#session-search").value.trim();
  if (!sessionId || !summary) return;
  const body = await api(`/api/sessions/${encodeURIComponent(sessionId)}/rename`, {
    method: "PUT",
    body: JSON.stringify({
      provider: qs("#session-provider").value,
      summary,
    }),
  });
  renderJson("#sessions-output", body);
}

async function generateGitMessage() {
  const project = activeProjectName();
  const files = selectedGitFiles();
  if (!project || !files.length) return;
  const body = await api("/api/git/generate-commit-message", {
    method: "POST",
    body: JSON.stringify({ project, files }),
  });
  qs("#git-message").value = body.message || "";
  renderGeneratedGitMessage(body.message || "");
}

async function commitGitSelection() {
  const project = activeProjectName();
  const files = selectedGitFiles();
  const message = qs("#git-message").value.trim();
  if (!project || !files.length || !message) return;
  const body = await api("/api/git/commit", {
    method: "POST",
    body: JSON.stringify({ project, files, message }),
  });
  renderGitOperation(body);
  await loadGitStatus();
}

async function gitOperation(path) {
  const project = activeProjectName();
  if (!project) return;
  const body = await api(path, {
    method: "POST",
    body: JSON.stringify({ project }),
  });
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
}

async function gitRead(path, renderer = renderJson) {
  const project = activeProjectName();
  if (!project) return;
  const body = await api(`${path}${path.includes("?") ? "&" : "?"}project=${encodeURIComponent(project)}`);
  renderer("#git-output", body);
}

async function gitDiffSelected() {
  const project = activeProjectName();
  const file = selectedGitFiles()[0];
  if (!project || !file) return;
  await gitDiffForFile(file);
}

async function gitDiffForFile(file) {
  const project = activeProjectName();
  if (!project || !file) return;
  state.currentGitDiffFile = file;
  renderGitFiles();
  const body = await api(`/api/git/diff?project=${encodeURIComponent(project)}&file=${encodeURIComponent(file)}`);
  renderGitDiff(file, body);
}

async function gitFileDiffSelected() {
  const project = activeProjectName();
  const file = selectedGitFiles()[0];
  if (!project || !file) return;
  await gitFileReviewForFile(file);
}

async function gitFileReviewForFile(file) {
  const project = activeProjectName();
  if (!project || !file) return;
  const body = await api(`/api/git/file-with-diff?project=${encodeURIComponent(project)}&file=${encodeURIComponent(file)}`);
  renderGitFileReview(file, body);
}

async function loadGitConflicts() {
  const project = activeProjectName();
  if (!project) return;
  const body = await api(`/api/git/conflicts?project=${encodeURIComponent(project)}`);
  renderGitConflicts(body);
}

async function loadGitConflictFile(file) {
  const project = activeProjectName();
  if (!project || !file) return;
  const body = await api(`/api/git/conflict-file?project=${encodeURIComponent(project)}&file=${encodeURIComponent(file)}`);
  renderGitConflictFile(body);
}

function renderGitConflicts(body) {
  const files = body.files || [];
  const target = qs("#git-output");
  target.className = "output-panel result-list";
  if (!files.length) {
    target.innerHTML = '<p class="empty">No unresolved Git conflicts.</p>';
    return;
  }
  target.innerHTML = files.map((file) => `<article class="result-row conflict-row">
    <header class="row-title">
      <strong>${escapeHtml(file.path)}</strong>
      <span class="badge danger">${escapeHtml(file.status)}</span>
    </header>
    <span class="meta">${escapeHtml(file.conflictCount)} conflict region(s)</span>
    <div class="row-actions">
      <button type="button" data-git-conflict-open="${escapeHtml(file.path)}">Open</button>
      <button type="button" data-git-conflict-quick="${escapeHtml(file.path)}" data-resolution="ours">Use Ours</button>
      <button type="button" data-git-conflict-quick="${escapeHtml(file.path)}" data-resolution="theirs">Use Theirs</button>
    </div>
  </article>`).join("");
  target.querySelectorAll("[data-git-conflict-open]").forEach((button) => {
    button.addEventListener("click", () => loadGitConflictFile(button.dataset.gitConflictOpen).catch(showError));
  });
  target.querySelectorAll("[data-git-conflict-quick]").forEach((button) => {
    button.addEventListener("click", () => {
      resolveGitConflict(button.dataset.gitConflictQuick, button.dataset.resolution).catch(showError);
    });
  });
}

function renderGitConflictFile(body) {
  state.currentConflictFile = body.path;
  const target = qs("#git-output");
  target.className = "output-panel conflict-editor";
  const regions = body.conflicts || [];
  const regionHtml = regions.length
    ? `<div class="conflict-region-list">${regions.map((region, index) => `<article class="conflict-region">
        <header class="row-title">
          <strong>Conflict ${index + 1}</strong>
          <span class="meta">lines ${escapeHtml(region.startLine)}-${escapeHtml(region.endLine)}</span>
        </header>
        <div class="side-by-side three-way">
          <section><h3>Ours</h3><pre>${escapeHtml(region.ours || "")}</pre></section>
          ${region.base ? `<section><h3>Base</h3><pre>${escapeHtml(region.base)}</pre></section>` : ""}
          <section><h3>Theirs</h3><pre>${escapeHtml(region.theirs || "")}</pre></section>
        </div>
      </article>`).join("")}</div>`
    : '<p class="empty">No conflict markers found in this file. You can still mark it resolved.</p>';
  target.innerHTML = `<div class="output-title">
      <span>${escapeHtml(body.path)} <span class="badge danger">${escapeHtml(body.status)}</span></span>
      <span>${regions.length} conflict region(s)</span>
    </div>
    <div class="conflict-toolbar">
      <button type="button" data-conflict-resolution="ours">Use Ours</button>
      <button type="button" data-conflict-resolution="theirs">Use Theirs</button>
      <button type="button" data-conflict-resolution="manual">Save Manual Resolution</button>
      <button type="button" data-conflict-refresh>Refresh</button>
    </div>
    ${regionHtml}
    <textarea id="git-conflict-content" spellcheck="false">${escapeHtml(body.content || "")}</textarea>`;
  target.querySelectorAll("[data-conflict-resolution]").forEach((button) => {
    button.addEventListener("click", () => {
      const resolution = button.dataset.conflictResolution;
      const content = resolution === "manual" ? qs("#git-conflict-content").value : undefined;
      resolveGitConflict(body.path, resolution, content).catch(showError);
    });
  });
  target.querySelector("[data-conflict-refresh]")?.addEventListener("click", () => {
    loadGitConflictFile(body.path).catch(showError);
  });
}

async function resolveGitConflict(file, resolution, content) {
  const project = activeProjectName();
  if (!project || !file || !resolution) return;
  const payload = { project, file, resolution, stage: true };
  if (content !== undefined) payload.content = content;
  const body = await api("/api/git/resolve-conflict", {
    method: "POST",
    body: JSON.stringify(payload),
  });
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
  await loadGitConflicts().catch(() => {});
}

function renderGitDiff(file, body) {
  const diff = body.diff || "";
  const target = qs("#git-output");
  state.currentGitDiffFile = file;
  target.className = "output-panel diff-view";
  const status = gitFilesFromStatus(state.gitStatus).find((item) => item.path === file)?.status || "";
  const statusBadge = status
    ? `<span class="git-status-badge ${gitStatusClass(status)}" title="${escapeHtml(gitStatusLabel(status))}">${escapeHtml(status)}</span>`
    : "";
  const header = `<div class="git-diff-header">
    <div class="git-diff-title">
      <strong>${escapeHtml(file)}</strong>
      <span>${escapeHtml(status ? gitStatusLabel(status) : "Diff preview")}</span>
    </div>
    <div class="git-diff-actions">
      ${statusBadge}
      <button type="button" class="icon-button" data-git-diff-open="${escapeHtml(file)}" aria-label="Open file" title="Open file" data-symbol="open"></button>
    </div>
  </div>`;
  if (!diff.trim()) {
    target.innerHTML = `${header}<div class="git-diff-scroll"><div class="git-diff-card"><p class="empty">No diff for this file.</p></div></div>`;
    target.querySelector("[data-git-diff-open]")?.addEventListener("click", () => openGitChangedFile(file).catch(showError));
    return;
  }
  const parsed = parseDiffHunks(diff);
  const truncated = body.isTruncated ? '<span class="badge warn">truncated</span>' : "";
  const controls = parsed.hunks.length
    ? `<div class="diff-toolbar">
        <span>${parsed.hunks.length} hunk(s) ${truncated}</span>
        <button type="button" data-git-hunks-select="all">Select All</button>
        <button type="button" data-git-hunks-select="none">Select None</button>
        <button type="button" data-git-hunks-apply="stage">Stage Hunks</button>
        <button type="button" data-git-hunks-apply="unstage">Unstage Hunks</button>
      </div>`
    : "";
  const prelude = parsed.prelude.length
    ? `<pre class="diff-prelude">${parsed.prelude.map(diffLineHtml).join("")}</pre>`
    : "";
  const hunks = parsed.hunks.map((hunk, index) => `<section class="diff-hunk">
    <label class="diff-hunk-header">
      <input type="checkbox" data-git-hunk="${index}" checked />
      <span>${escapeHtml(hunk.header || `Hunk ${index + 1}`)}</span>
    </label>
    <pre>${hunk.lines.map(diffLineHtml).join("")}</pre>
  </section>`).join("");
  target.innerHTML = `${header}<div class="git-diff-scroll"><div class="git-diff-card">${controls}${prelude}${hunks}</div></div>`;
  target.querySelector("[data-git-diff-open]")?.addEventListener("click", () => openGitChangedFile(file).catch(showError));
  target.querySelectorAll("[data-git-hunks-select]").forEach((button) => {
    button.addEventListener("click", () => setGitHunkSelection(button.dataset.gitHunksSelect === "all"));
  });
  target.querySelectorAll("[data-git-hunks-apply]").forEach((button) => {
    button.addEventListener("click", () => applySelectedGitHunks(button.dataset.gitHunksApply).catch(showError));
  });
}

function parseDiffHunks(diff) {
  const prelude = [];
  const hunks = [];
  let current = null;
  for (const line of diff.split("\n")) {
    if (line.startsWith("@@")) {
      current = { header: line, lines: [line] };
      hunks.push(current);
      continue;
    }
    if (current) {
      current.lines.push(line);
    } else {
      prelude.push(line);
    }
  }
  return { prelude: prelude.filter((line) => line.trim()), hunks };
}

function diffLineHtml(line) {
  let kind = "context";
  if (line.startsWith("@@")) kind = "hunk";
  else if (line.startsWith("+++") || line.startsWith("---") || line.startsWith("diff ")) kind = "meta";
  else if (line.startsWith("+")) kind = "add";
  else if (line.startsWith("-")) kind = "remove";
  return `<span class="diff-line ${kind}">${escapeHtml(line || " ")}</span>`;
}

function selectedGitHunks() {
  return [...document.querySelectorAll("[data-git-hunk]:checked")]
    .map((input) => Number(input.dataset.gitHunk))
    .filter(Number.isInteger);
}

function setGitHunkSelection(checked) {
  document.querySelectorAll("[data-git-hunk]").forEach((input) => {
    input.checked = checked;
  });
}

async function applySelectedGitHunks(operation) {
  const project = activeProjectName();
  const file = state.currentGitDiffFile;
  const hunkIndexes = selectedGitHunks();
  if (!project || !file || !hunkIndexes.length) return;
  const body = await api("/api/git/apply-hunks", {
    method: "POST",
    body: JSON.stringify({ project, file, operation, hunkIndexes }),
  });
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
  await gitDiffForFile(file).catch(() => {});
}

function renderGitFileReview(file, body) {
  const target = qs("#git-output");
  target.className = "output-panel file-review";
  const oldLines = (body.oldContent || "").split("\n");
  const currentLines = (body.currentContent || "").split("\n");
  const maxLines = Math.max(oldLines.length, currentLines.length, 1);
  const oldHtml = [];
  const currentHtml = [];
  for (let index = 0; index < maxLines; index += 1) {
    const oldLine = oldLines[index] ?? "";
    const currentLine = currentLines[index] ?? "";
    const changed = oldLine !== currentLine;
    oldHtml.push(`<span class="${changed ? "remove" : ""}"><b>${index + 1}</b>${escapeHtml(oldLine || " ")}</span>`);
    currentHtml.push(`<span class="${changed ? "add" : ""}"><b>${index + 1}</b>${escapeHtml(currentLine || " ")}</span>`);
  }
  const badges = [
    body.isDeleted ? '<span class="badge danger">deleted</span>' : "",
    body.isUntracked ? '<span class="badge ok">untracked</span>' : "",
  ].join("");
  target.innerHTML = `<div class="output-title">${escapeHtml(file)} ${badges}</div>
    <div class="side-by-side">
      <section><h3>Before</h3><pre>${oldHtml.join("")}</pre></section>
      <section><h3>Current</h3><pre>${currentHtml.join("")}</pre></section>
    </div>`;
}

function renderGeneratedGitMessage(message) {
  const target = qs("#git-output");
  target.className = "output-panel result-list";
  target.innerHTML = `<article class="result-row">
    <strong>Generated Commit Message</strong>
    <div class="message-body">${renderMarkdownLite(message || "No message generated.")}</div>
    <button type="button" data-copy-text="${escapeHtml(message)}">Copy</button>
  </article>`;
  bindCopyButtons(target);
}

function renderGitOperation(body) {
  const target = qs("#git-output");
  target.className = "output-panel result-list";
  target.innerHTML = `<article class="result-row">
    <header class="row-title">${resultBadge(body.success !== false)}<strong>${escapeHtml(operationMessage(body))}</strong></header>
    ${body.remoteName || body.remoteUrl || body.remoteBranch || body.branch ? `<span class="meta">${escapeHtml([
      body.branch ? `branch ${body.branch}` : "",
      body.remoteName ? `remote ${body.remoteName}` : "",
      body.remoteBranch ? `upstream ${body.remoteBranch}` : "",
      body.remoteUrl || "",
    ].filter(Boolean).join(" · "))}</span>` : ""}
  </article>`;
}

function renderGitOperationList(results) {
  const target = qs("#git-output");
  target.className = "output-panel result-list";
  target.innerHTML = results.map((result, index) => `<article class="result-row">
    <header class="row-title">${resultBadge(result.success !== false)}<strong>Operation ${index + 1}</strong></header>
    <span>${escapeHtml(operationMessage(result))}</span>
  </article>`).join("");
}

function renderGitBranches(selector, body) {
  state.gitBranches = body;
  const target = qs(selector);
  const local = body.localBranches || [];
  const remote = body.remoteBranches || [];
  target.className = "output-panel result-list";
  target.innerHTML = [
    branchGroupHtml("Local Branches", local, true),
    branchGroupHtml("Remote Branches", remote, false),
  ].join("") || '<p class="empty">No branches found.</p>';
  target.querySelectorAll("[data-branch-name]").forEach((button) => {
    button.addEventListener("click", () => {
      qs("#git-branch").value = button.dataset.branchName;
    });
  });
}

function branchGroupHtml(title, branches, usable) {
  if (!branches.length) return "";
  return `<article class="result-row">
    <strong>${escapeHtml(title)}</strong>
    <div class="pill-list">${branches.map((branch) => `<button type="button" ${usable ? `data-branch-name="${escapeHtml(branch.replace(/^\*\s*/, ""))}"` : ""}>${escapeHtml(branch)}</button>`).join("")}</div>
  </article>`;
}

function renderGitCommits(selector, body) {
  state.gitCommits = body;
  const commits = body.commits || [];
  const target = qs(selector);
  target.className = "output-panel result-list";
  if (!commits.length) {
    target.innerHTML = '<p class="empty">No commit history found.</p>';
    return;
  }
  target.innerHTML = commits.map((commit) => `<article class="result-row commit-row">
    <header class="row-title">
      <strong>${escapeHtml(commit.message)}</strong>
      <span class="row-actions">
        <button type="button" data-commit-diff="${escapeHtml(commit.hash)}">Diff</button>
        <button type="button" data-commit-use="${escapeHtml(commit.hash)}">Use Hash</button>
        <button type="button" data-copy-text="${escapeHtml(commit.hash)}">Copy Hash</button>
      </span>
    </header>
    <span class="meta">${escapeHtml(commit.hash)} · ${escapeHtml(commit.author)} &lt;${escapeHtml(commit.email)}&gt; · ${escapeHtml(commit.date)}</span>
    ${commit.stats ? `<span>${escapeHtml(commit.stats)}</span>` : ""}
  </article>`).join("");
  bindCopyButtons(target);
  target.querySelectorAll("[data-commit-diff]").forEach((button) => {
    button.addEventListener("click", () => gitCommitDiff(button.dataset.commitDiff).catch(showError));
  });
  target.querySelectorAll("[data-commit-use]").forEach((button) => {
    button.addEventListener("click", () => {
      qs("#git-branch").value = button.dataset.commitUse;
    });
  });
}

async function gitCommitDiff(commit) {
  const project = activeProjectName();
  if (!project || !commit) return;
  const body = await api(`/api/git/commit-diff?project=${encodeURIComponent(project)}&commit=${encodeURIComponent(commit)}`);
  renderGitDiff(commit, body);
}

function renderGitRemoteStatus(selector, body) {
  const target = qs(selector);
  target.className = "output-panel result-list";
  target.innerHTML = `<article class="result-row">
    <header class="row-title">
      <strong>${escapeHtml(body.branch || "Remote Status")}</strong>
      <span class="badge ${body.isUpToDate ? "ok" : "warn"}">${body.isUpToDate ? "up to date" : "attention"}</span>
    </header>
    <span class="meta">${escapeHtml([
      body.hasRemote ? "remote configured" : "no remote",
      body.hasUpstream ? "upstream configured" : "no upstream",
      body.remoteName ? `remote ${body.remoteName}` : "",
      body.remoteBranch ? `tracking ${body.remoteBranch}` : "",
    ].filter(Boolean).join(" · "))}</span>
    <span>${escapeHtml(`ahead ${body.ahead ?? 0} · behind ${body.behind ?? 0}`)}</span>
    ${body.message ? `<span>${escapeHtml(body.message)}</span>` : ""}
  </article>`;
}

async function publishCurrentBranch() {
  if (!qs("#git-branch").value.trim() && state.gitStatus?.branch) {
    qs("#git-branch").value = state.gitStatus.branch;
  }
  await gitBranchOperation("/api/git/publish");
}

async function gitBranchOperation(path) {
  const project = activeProjectName();
  const branch = qs("#git-branch").value.trim();
  if (!project || !branch) return;
  const body = await api(path, {
    method: "POST",
    body: JSON.stringify({ project, branch }),
  });
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
}

async function setGitRemote() {
  const project = activeProjectName();
  const url = qs("#git-remote-url").value.trim();
  if (!project || !url) return;
  const body = await api("/api/git/remote", {
    method: "POST",
    body: JSON.stringify({ project, name: "origin", url }),
  });
  renderGitOperation(body);
}

async function gitSelectedFileOperation(path) {
  const project = activeProjectName();
  const files = selectedGitFiles();
  if (!project || !files.length) return;
  const results = [];
  for (const file of files) {
    results.push(await api(path, {
      method: "POST",
      body: JSON.stringify({ project, file }),
    }));
  }
  renderGitOperationList(results);
  await loadGitStatus().catch(() => {});
  showToast("Git selection updated", "ok");
}

async function loadDbConnections() {
  const body = await api("/api/database/connections");
  state.dbConnections = body.connections || [];
  if (state.selectedDbConnection && !state.dbConnections.some((connection) => connection.id === state.selectedDbConnection)) {
    state.selectedDbConnection = null;
  }
  if (state.selectedDbTargetConnection && !state.dbConnections.some((connection) => connection.id === state.selectedDbTargetConnection)) {
    state.selectedDbTargetConnection = null;
  }
  if (!state.selectedDbConnection && state.dbConnections[0]) {
    state.selectedDbConnection = state.dbConnections[0].id;
  }
  if (!state.selectedDbTargetConnection && state.selectedDbConnection) {
    state.selectedDbTargetConnection = state.selectedDbConnection;
  }
  renderDbConnections();
  renderDbTargetOptions();
}

function renderDbConnections() {
  const target = qs("#db-connections");
  const filter = qs("#db-filter")?.value.trim().toLowerCase() || "";
  const connections = state.dbConnections.filter((connection) => {
    const haystack = [
      connection.name,
      connection.type,
      connection.databaseName,
      connection.filePath,
      connection.host,
    ].join(" ").toLowerCase();
    return !filter || haystack.includes(filter);
  });
  if (!connections.length) {
    target.innerHTML = '<p class="empty">No database connections.</p>';
    qs("#db-explorer-tree").innerHTML = "";
    renderDbTargetOptions();
    return;
  }
  const visible = connections.slice(0, state.limits.dbConnections);
  target.innerHTML = visible
    .map((connection) => `<article class="row ${connection.id === state.selectedDbConnection ? "selected" : ""}">
      <strong>${escapeHtml(connection.name)}</strong>
      <span>${escapeHtml(connection.type)} · ${escapeHtml(connection.databaseName || connection.filePath || connection.host || "")}</span>
      <span class="meta">${escapeHtml([
        connection.host && connection.port ? `:${connection.port}` : "",
        connection.lastTestStatus ? `last test ${connection.lastTestStatus}` : "",
        connection.updatedAt ? `updated ${formatDate(connection.updatedAt)}` : "",
      ].filter(Boolean).join(" · "))}</span>
      <div class="row-actions">
        <button type="button" data-db-id="${connection.id}">Select</button>
        <button type="button" data-db-edit="${connection.id}">Edit</button>
        <button type="button" data-db-test="${connection.id}">Test</button>
        <button type="button" data-db-delete="${connection.id}">Delete</button>
      </div>
    </article>`)
    .join("") + showMoreButton("dbConnections", connections.length, "dbConnections");
  target.querySelectorAll("[data-db-id]").forEach((button) => {
    button.addEventListener("click", () => {
      selectDbConnection(Number(button.dataset.dbId));
    });
  });
  target.querySelectorAll("[data-db-edit]").forEach((button) => {
    button.addEventListener("click", () => editDbConnection(Number(button.dataset.dbEdit)));
  });
  target.querySelectorAll("[data-db-test]").forEach((button) => {
    button.addEventListener("click", () => testDbConnection(Number(button.dataset.dbTest)));
  });
  target.querySelectorAll("[data-db-delete]").forEach((button) => {
    button.addEventListener("click", () => deleteDbConnection(Number(button.dataset.dbDelete)).catch(showError));
  });
  bindShowMore(target);
}

function renderDbTargetOptions() {
  const select = qs("#db-target-connection");
  if (!select) return;
  const previous = select.value || String(state.selectedDbTargetConnection || "");
  select.innerHTML = state.dbConnections
    .map((connection) => `<option value="${connection.id}">${escapeHtml(connection.name)} (${escapeHtml(connection.type)})</option>`)
    .join("");
  if (previous && state.dbConnections.some((connection) => String(connection.id) === previous)) {
    select.value = previous;
  } else if (state.selectedDbConnection) {
    select.value = String(state.selectedDbConnection);
  }
  state.selectedDbTargetConnection = Number(select.value) || null;
}

function selectDbConnection(connectionId) {
  state.selectedDbConnection = connectionId;
  if (!state.selectedDbTargetConnection) {
    state.selectedDbTargetConnection = connectionId;
  }
  const targetSelect = qs("#db-target-connection");
  if (targetSelect && state.selectedDbTargetConnection) {
    targetSelect.value = String(state.selectedDbTargetConnection);
  }
  renderDbConnections();
  setOutput("#db-output", `selected connection ${connectionId}`);
  loadDbExplorer().catch(showError);
}

function selectedDbConnectionProfile() {
  return state.dbConnections.find((connection) => connection.id === selectedDbConnectionId()) || null;
}

async function createDbConnection(event) {
  event.preventDefault();
  const payload = dbConnectionFormPayload();
  if (!payload) return;
  const endpoint = state.editingDbConnection
    ? `/api/database/connections/${encodeURIComponent(state.editingDbConnection)}`
    : "/api/database/connections";
  const body = await api(endpoint, {
    method: state.editingDbConnection ? "PUT" : "POST",
    body: JSON.stringify(payload),
  });
  state.selectedDbConnection = body.connection?.id || state.editingDbConnection || null;
  state.editingDbConnection = null;
  qs("#db-password").value = "";
  qs("#db-save-button").textContent = "Save Connection";
  await loadDbConnections();
  if (state.selectedDbConnection) {
    await loadDbExplorer().catch(showError);
  }
}

function dbConnectionFormPayload() {
  const type = qs("#db-type").value;
  const location = qs("#db-location").value.trim();
  const payload = {
    name: qs("#db-name").value.trim(),
    type,
    databaseName: qs("#db-database").value.trim() || undefined,
    port: qs("#db-port").value ? Number(qs("#db-port").value) : undefined,
    username: qs("#db-username").value.trim() || undefined,
    password: qs("#db-password").value || undefined,
    showAllDatabases: qs("#db-show-all").checked,
  };
  if (!payload.name || !location) return null;
  if (type === "sqlite") {
    payload.filePath = location;
    payload.port = undefined;
    payload.showAllDatabases = false;
  } else {
    payload.host = location;
  }
  return payload;
}

function resetDbConnectionForm() {
  state.editingDbConnection = null;
  qs("#db-name").value = "";
  qs("#db-type").value = "sqlite";
  qs("#db-location").value = "";
  qs("#db-database").value = "";
  qs("#db-username").value = "";
  qs("#db-password").value = "";
  qs("#db-port").value = "";
  qs("#db-show-all").checked = false;
  qs("#db-save-button").textContent = "Save Connection";
}

function editDbConnection(connectionId) {
  const connection = state.dbConnections.find((item) => item.id === connectionId);
  if (!connection) return;
  state.editingDbConnection = connectionId;
  state.selectedDbConnection = connectionId;
  qs("#db-name").value = connection.name || "";
  qs("#db-type").value = connection.type || "sqlite";
  qs("#db-location").value = connection.type === "sqlite"
    ? connection.filePath || ""
    : connection.host || "";
  qs("#db-database").value = connection.databaseName || "";
  qs("#db-username").value = connection.username || "";
  qs("#db-password").value = "";
  qs("#db-port").value = connection.port || "";
  qs("#db-show-all").checked = !!connection.showAllDatabases;
  qs("#db-save-button").textContent = "Update Connection";
  renderDbConnections();
}

async function testDbConnectionForm() {
  const payload = dbConnectionFormPayload();
  if (!payload) return;
  const body = await api("/api/database/connections/test", {
    method: "POST",
    body: JSON.stringify({
      existingConnectionId: state.editingDbConnection || undefined,
      connection: payload,
    }),
  });
  renderJson("#db-output", body);
}

async function testDbConnection(connectionId) {
  const body = await api(`/api/database/connections/${connectionId}/test`, { method: "POST" });
  if (body.connection) {
    const index = state.dbConnections.findIndex((connection) => connection.id === body.connection.id);
    if (index >= 0) state.dbConnections[index] = body.connection;
  }
  renderDbConnections();
  renderJson("#db-output", body);
}

async function deleteDbConnection(connectionId = selectedDbConnectionId()) {
  if (!connectionId) return;
  if (!window.confirm("Delete this database connection?")) return;
  const body = await api(`/api/database/connections/${encodeURIComponent(connectionId)}`, { method: "DELETE" });
  if (state.selectedDbConnection === connectionId) {
    state.selectedDbConnection = null;
    state.selectedDbObject = null;
    qs("#db-explorer-tree").innerHTML = "";
  }
  if (state.selectedDbTargetConnection === connectionId) {
    state.selectedDbTargetConnection = null;
  }
  if (state.editingDbConnection === connectionId) {
    resetDbConnectionForm();
  }
  renderJson("#db-output", body);
  await loadDbConnections();
}

async function runDbQuery(event) {
  event.preventDefault();
  if (!state.selectedDbConnection) return;
  const body = await api(`/api/database/connections/${state.selectedDbConnection}/query`, {
    method: "POST",
    body: JSON.stringify({
      sql: qs("#db-sql").value,
      databaseName: qs("#db-context-database").value.trim() || undefined,
      schemaName: qs("#db-context-schema").value.trim() || undefined,
      maxRows: 200,
    }),
  });
  renderDbResult(body);
}

function selectedDbConnectionId() {
  return state.selectedDbConnection || state.dbConnections[0]?.id || null;
}

async function dbRead(path) {
  const connectionId = selectedDbConnectionId();
  if (!connectionId) return;
  const body = await api(path.replace("{id}", encodeURIComponent(connectionId)));
  renderJson("#db-output", body);
}

async function loadDbExplorer() {
  const connectionId = selectedDbConnectionId();
  if (!connectionId) return;
  const body = await api(`/api/database/connections/${encodeURIComponent(connectionId)}/explorer`);
  state.dbExplorerNodes = body.nodes || [];
  renderDbExplorer(body, "Database Explorer");
}

async function loadDbExplorerNode(node) {
  const connectionId = selectedDbConnectionId();
  if (!connectionId) return;
  setDbObjectContext(node);
  const params = new URLSearchParams({ nodeType: node.type });
  if (node.databaseName) params.set("databaseName", node.databaseName);
  if (node.schemaName) params.set("schemaName", node.schemaName);
  const body = await api(`/api/database/connections/${encodeURIComponent(connectionId)}/explorer?${params.toString()}`);
  state.dbExplorerNodes = body.nodes || [];
  renderDbExplorer(body, `${node.type}: ${node.name}`);
}

function renderDbExplorer(body, title) {
  const nodes = body.nodes || [];
  const tree = qs("#db-explorer-tree");
  tree.innerHTML = nodes.length
    ? `<div class="section-label">${escapeHtml(title)}</div>${nodes.map(dbExplorerNodeHtml).join("")}`
    : '<p class="empty">No database objects found.</p>';
  tree.querySelectorAll("[data-db-node]").forEach((button) => {
    button.addEventListener("click", () => {
      const node = JSON.parse(button.dataset.dbNode);
      setDbObjectContext(node);
      if (node.hasChildren) {
        loadDbExplorerNode(node).catch(showError);
        return;
      }
      if (["table", "view"].includes(node.type)) {
        loadDbTableData().catch(showError);
      }
    });
  });
  tree.querySelectorAll("[data-db-details]").forEach((button) => {
    button.addEventListener("click", () => {
      const node = JSON.parse(button.dataset.dbDetails);
      setDbObjectContext(node);
      loadDbObjectDetails().catch(showError);
    });
  });
  tree.querySelectorAll("[data-db-select-sql]").forEach((button) => {
    button.addEventListener("click", () => {
      const node = JSON.parse(button.dataset.dbSelectSql);
      setDbObjectContext(node);
      setDbSql("select");
    });
  });
  renderJson("#db-output", body);
}

function dbExplorerNodeHtml(node) {
  const encoded = escapeHtml(JSON.stringify(node));
  const meta = [
    node.type,
    node.databaseName,
    node.schemaName,
    node.description,
  ].filter(Boolean).join(" · ");
  return `<article class="row db-node">
    <strong>${escapeHtml(node.name)}</strong>
    <span class="meta">${escapeHtml(meta)}</span>
    <div class="row-actions">
      <button type="button" data-db-node="${encoded}">${node.hasChildren ? "Open" : "Use"}</button>
      <button type="button" data-db-details="${encoded}">Details</button>
      ${["table", "view"].includes(node.type) ? `<button type="button" data-db-select-sql="${encoded}">SQL</button>` : ""}
    </div>
  </article>`;
}

function setDbObjectContext(node) {
  state.selectedDbObject = node;
  if (["table", "view"].includes(node.type)) {
    qs("#db-table").value = node.name || "";
  } else {
    qs("#db-table").value = "";
  }
  qs("#db-context-database").value = node.databaseName || "";
  qs("#db-context-schema").value = node.schemaName || "";
  qs("#db-offset").value = "0";
}

function dbContextParams(extra = {}) {
  const params = new URLSearchParams();
  const databaseName = qs("#db-context-database").value.trim();
  const schemaName = qs("#db-context-schema").value.trim();
  if (databaseName) params.set("databaseName", databaseName);
  if (schemaName) params.set("schemaName", schemaName);
  Object.entries(extra).forEach(([key, value]) => {
    if (value !== undefined && value !== null && String(value).trim() !== "") {
      params.set(key, String(value));
    }
  });
  return params;
}

async function loadDbObjectDetails() {
  const connectionId = selectedDbConnectionId();
  const tableName = qs("#db-table").value.trim();
  if (!connectionId) return;
  const objectType = state.selectedDbObject?.type || (tableName ? "table" : "database");
  const objectName = state.selectedDbObject?.name || tableName || qs("#db-context-database").value.trim() || "main";
  const params = dbContextParams({
    objectType,
    name: objectName,
    includeRelational: true,
  });
  const body = await api(`/api/database/connections/${encodeURIComponent(connectionId)}/object-details?${params.toString()}`);
  renderDbObjectDetails(body);
}

async function loadDbRelationshipDiagram() {
  const connectionId = selectedDbConnectionId();
  if (!connectionId) return;
  const schemaName = qs("#db-context-schema").value.trim();
  const objectName = schemaName
    || qs("#db-context-database").value.trim()
    || selectedDbConnectionProfile()?.databaseName
    || "main";
  const params = dbContextParams({
    objectType: schemaName ? "schema" : "database",
    name: objectName,
    includeRelational: true,
  });
  const body = await api(`/api/database/connections/${encodeURIComponent(connectionId)}/object-details?${params.toString()}`);
  renderDbObjectDetails(body);
}

function renderDbObjectDetails(body) {
  state.lastDbObjectDetails = body;
  const details = body.details || body;
  const columns = details.columns || [];
  const objects = details.objects || [];
  const foreignKeys = details.foreignKeys || [];
  const relationalSchema = details.relationalSchema;
  const relationships = relationalSchema?.relationships || [];
  const diagram = relationalSchema ? renderDbRelationshipDiagram(relationalSchema) : "";
  const target = qs("#db-output");
  target.className = "output-panel result-list";
  target.innerHTML = `<article class="result-row">
    <header class="row-title">
      <strong>${escapeHtml(details.name || "Object Details")}</strong>
      <span class="badge">${escapeHtml(details.type || details.objectType || "table")}</span>
    </header>
    <span class="meta">${escapeHtml([details.databaseName, details.schemaName].filter(Boolean).join(" · "))}</span>
    ${columns.length ? `<div class="table-scroll"><table><thead><tr><th>Column</th><th>Type</th><th>Nullable</th><th>Key</th></tr></thead><tbody>${columns.map((column) => `<tr>
      <td>${escapeHtml(column.name)}</td>
      <td>${escapeHtml(column.dataType || column.nativeType || "")}</td>
      <td>${escapeHtml(column.nullable === false ? "no" : "yes")}</td>
      <td>${column.isPrimaryKey ? "primary" : ""}</td>
    </tr>`).join("")}</tbody></table></div>` : ""}
    ${foreignKeys.length ? `<details open><summary>Foreign keys</summary><div class="table-scroll"><table><thead><tr><th>Column</th><th>References</th><th>Update</th><th>Delete</th></tr></thead><tbody>${foreignKeys.map((key) => `<tr>
      <td>${escapeHtml(key.columnName)}</td>
      <td>${escapeHtml([key.referencedSchemaName, key.referencedTableName, key.referencedColumnName].filter(Boolean).join("."))}</td>
      <td>${escapeHtml(key.onUpdate || "")}</td>
      <td>${escapeHtml(key.onDelete || "")}</td>
    </tr>`).join("")}</tbody></table></div></details>` : ""}
    ${objects.length ? `<details open><summary>Objects</summary><div class="table-scroll"><table><thead><tr><th>Name</th><th>Type</th><th>Database</th><th>Schema</th></tr></thead><tbody>${objects.map((object) => `<tr>
      <td>${escapeHtml(object.name)}</td>
      <td>${escapeHtml(object.type)}</td>
      <td>${escapeHtml(object.databaseName || "")}</td>
      <td>${escapeHtml(object.schemaName || "")}</td>
    </tr>`).join("")}</tbody></table></div></details>` : ""}
    ${diagram}
    ${relationships.length ? `<details><summary>Relationships</summary><div class="table-scroll"><table><thead><tr><th>Source</th><th>Target</th></tr></thead><tbody>${relationships.map((relationship) => `<tr>
      <td>${escapeHtml([relationship.sourceSchemaName, relationship.sourceTableName, relationship.sourceColumnName].filter(Boolean).join("."))}</td>
      <td>${escapeHtml([relationship.targetSchemaName, relationship.targetTableName, relationship.targetColumnName].filter(Boolean).join("."))}</td>
    </tr>`).join("")}</tbody></table></div></details>` : ""}
    ${!columns.length && !objects.length && !foreignKeys.length && !relationships.length ? '<p class="empty">No object details.</p>' : ""}
  </article>`;
  bindDbDiagramControls();
}

function renderDbRelationshipDiagram(schema) {
  const maxTables = 40;
  const tableMap = new Map();
  (schema.tables || []).forEach((table) => {
    tableMap.set(dbTableKey(table.schemaName, table.name), table);
  });
  (schema.relationships || []).forEach((relationship) => {
    const sourceKey = dbTableKey(relationship.sourceSchemaName, relationship.sourceTableName);
    const targetKey = dbTableKey(relationship.targetSchemaName, relationship.targetTableName);
    if (!tableMap.has(sourceKey)) {
      tableMap.set(sourceKey, {
        name: relationship.sourceTableName,
        schemaName: relationship.sourceSchemaName,
        columns: [],
        isExternal: true,
      });
    }
    if (!tableMap.has(targetKey)) {
      tableMap.set(targetKey, {
        name: relationship.targetTableName,
        schemaName: relationship.targetSchemaName,
        columns: [],
        isExternal: true,
      });
    }
  });
  const tables = [...tableMap.values()].slice(0, maxTables);
  if (!tables.length) return "";

  const columns = Math.max(1, Math.ceil(Math.sqrt(tables.length)));
  const nodeWidth = 220;
  const nodeHeight = 148;
  const gapX = 56;
  const gapY = 48;
  const width = columns * nodeWidth + (columns - 1) * gapX + 32;
  const rows = Math.ceil(tables.length / columns);
  const height = rows * nodeHeight + (rows - 1) * gapY + 32;
  const zoom = state.dbDiagram.zoom || 1;
  const query = (state.dbDiagram.query || "").trim().toLowerCase();
  const positions = new Map();
  tables.forEach((table, index) => {
    const col = index % columns;
    const row = Math.floor(index / columns);
    positions.set(dbTableKey(table.schemaName, table.name), {
      x: 16 + col * (nodeWidth + gapX),
      y: 16 + row * (nodeHeight + gapY),
    });
  });

  const paths = (schema.relationships || []).map((relationship, index) => {
    const source = positions.get(dbTableKey(relationship.sourceSchemaName, relationship.sourceTableName));
    const target = positions.get(dbTableKey(relationship.targetSchemaName, relationship.targetTableName));
    if (!source || !target) return "";
    const startX = source.x + nodeWidth;
    const startY = source.y + 44 + (index % 5) * 10;
    const endX = target.x;
    const endY = target.y + 44 + (index % 5) * 10;
    const midX = startX + Math.max(24, (endX - startX) / 2);
    const label = `${relationship.sourceColumnName} -> ${relationship.targetColumnName}`;
    const sourceMatches = tableMatchesDiagramQuery({ schemaName: relationship.sourceSchemaName, name: relationship.sourceTableName }, query);
    const targetMatches = tableMatchesDiagramQuery({ schemaName: relationship.targetSchemaName, name: relationship.targetTableName }, query);
    const dimmed = query && !sourceMatches && !targetMatches ? "dimmed" : "";
    return `<path class="${dimmed}" d="M${startX} ${startY} C${midX} ${startY}, ${midX} ${endY}, ${endX} ${endY}" marker-end="url(#db-arrow)" />
      <title>${escapeHtml(label)}</title>`;
  }).join("");

  const nodes = tables.map((table) => {
    const position = positions.get(dbTableKey(table.schemaName, table.name));
    const matches = tableMatchesDiagramQuery(table, query);
    const dimmed = query && !matches ? "dimmed" : "";
    const matched = query && matches ? "matched" : "";
    const columns = (table.columns || []).slice(0, 5);
    const hidden = Math.max(0, (table.columns || []).length - columns.length);
    const columnText = columns.map((column, index) => `<text x="${position.x + 12}" y="${position.y + 58 + index * 18}" class="db-schema-column">
      ${escapeHtml(column.name)}${column.isPrimaryKey ? " *" : ""}${column.dataType ? `: ${escapeHtml(column.dataType)}` : ""}
    </text>`).join("");
    const schemaLabel = [table.schemaName, table.isExternal ? "external" : ""].filter(Boolean).join(" · ");
    return `<g class="db-schema-node ${table.isExternal ? "external" : ""} ${matched} ${dimmed}" data-db-diagram-table="${escapeHtml(table.name)}" data-db-diagram-schema="${escapeHtml(table.schemaName || "")}">
      <rect x="${position.x}" y="${position.y}" width="${nodeWidth}" height="${nodeHeight}" rx="6" />
      <text x="${position.x + 12}" y="${position.y + 24}" class="db-schema-title">${escapeHtml(table.name)}</text>
      ${schemaLabel ? `<text x="${position.x + 12}" y="${position.y + 42}" class="db-schema-meta">${escapeHtml(schemaLabel)}</text>` : ""}
      ${columnText}
      ${hidden ? `<text x="${position.x + 12}" y="${position.y + 58 + columns.length * 18}" class="db-schema-meta">+${hidden} more</text>` : ""}
    </g>`;
  }).join("");

  const clipped = tableMap.size > maxTables
    ? `<p class="empty">Showing ${maxTables} of ${tableMap.size} tables.</p>`
    : "";
  return `<details open class="db-schema-section">
    <summary>Relationship Diagram</summary>
    <div class="db-schema-toolbar">
      <input id="db-diagram-filter" value="${escapeHtml(state.dbDiagram.query)}" placeholder="Filter tables" />
      <button type="button" data-db-diagram-zoom="-0.15">Zoom Out</button>
      <button type="button" data-db-diagram-zoom="0.15">Zoom In</button>
      <button type="button" data-db-diagram-reset>Reset</button>
    </div>
    <div class="db-schema-diagram">
      <svg viewBox="0 0 ${width} ${height}" style="width:${Math.round(width * zoom)}px" role="img" aria-label="Database relationship diagram">
        <defs>
          <marker id="db-arrow" markerWidth="10" markerHeight="10" refX="8" refY="3" orient="auto" markerUnits="strokeWidth">
            <path d="M0,0 L0,6 L9,3 z" />
          </marker>
        </defs>
        <g class="db-schema-links">${paths}</g>
        <g>${nodes}</g>
      </svg>
    </div>
    ${clipped}
  </details>`;
}

function dbTableKey(schemaName, tableName) {
  return `${schemaName || ""}.${tableName || ""}`;
}

function tableMatchesDiagramQuery(table, query) {
  if (!query) return true;
  return [table.name, table.schemaName, table.databaseName]
    .filter(Boolean)
    .join(" ")
    .toLowerCase()
    .includes(query);
}

function bindDbDiagramControls() {
  const root = qs("#db-output");
  root.querySelector("#db-diagram-filter")?.addEventListener("input", (event) => {
    state.dbDiagram.query = event.currentTarget.value;
    renderDbObjectDetails(state.lastDbObjectDetails);
    qs("#db-diagram-filter")?.focus();
  });
  root.querySelectorAll("[data-db-diagram-zoom]").forEach((button) => {
    button.addEventListener("click", () => {
      state.dbDiagram.zoom = Math.min(2.5, Math.max(0.5, state.dbDiagram.zoom + Number(button.dataset.dbDiagramZoom || 0)));
      renderDbObjectDetails(state.lastDbObjectDetails);
    });
  });
  root.querySelector("[data-db-diagram-reset]")?.addEventListener("click", () => {
    state.dbDiagram = { zoom: 1, query: "" };
    renderDbObjectDetails(state.lastDbObjectDetails);
  });
  root.querySelectorAll("[data-db-diagram-table]").forEach((node) => {
    node.addEventListener("click", () => {
      qs("#db-table").value = node.dataset.dbDiagramTable || "";
      qs("#db-context-schema").value = node.dataset.dbDiagramSchema || "";
      state.selectedDbObject = {
        type: "table",
        name: node.dataset.dbDiagramTable || "",
        schemaName: node.dataset.dbDiagramSchema || "",
        databaseName: qs("#db-context-database").value.trim() || undefined,
      };
      loadDbObjectDetails().catch(showError);
    });
  });
}

function setDbSql(kind) {
  const tableName = qs("#db-table").value.trim();
  if (!tableName) return;
  const qualified = dbQualifiedTableName(tableName);
  qs("#db-sql").value = kind === "count"
    ? `SELECT COUNT(*) AS count FROM ${qualified};`
    : `SELECT * FROM ${qualified} LIMIT 100;`;
}

async function loadDbTableData() {
  const connectionId = selectedDbConnectionId();
  const tableName = qs("#db-table").value.trim();
  if (!connectionId || !tableName) return;
  const params = dbContextParams({
    tableName,
    includeTotalCount: true,
    limit: numericInputValue("#db-limit", 50, 1, 500),
    offset: numericInputValue("#db-offset", 0, 0, 1_000_000_000),
  });
  const body = await api(`/api/database/connections/${encodeURIComponent(connectionId)}/table-data?${params.toString()}`);
  renderDbResult(body);
}

async function dbFileJob(path) {
  const connectionId = selectedDbConnectionId();
  const tableName = qs("#db-table").value.trim();
  const filePath = qs("#db-file-path").value.trim();
  if (!connectionId || !tableName || !filePath) return;
  const body = await api(path, {
    method: "POST",
    body: JSON.stringify({
      connectionId,
      tableName,
      databaseName: qs("#db-context-database").value.trim() || undefined,
      schemaName: qs("#db-context-schema").value.trim() || undefined,
      filePath,
    }),
  });
  renderJson("#db-output", body);
}

async function transferDbTable() {
  const sourceId = selectedDbConnectionId();
  const targetId = Number(qs("#db-target-connection")?.value) || sourceId;
  const tableName = qs("#db-table").value.trim();
  if (!sourceId || !tableName) return;
  const targetTable = qs("#db-target-table").value.trim();
  const body = await api("/api/database/transfers", {
    method: "POST",
    body: JSON.stringify({
      mode: qs("#db-transfer-mode").value,
      source: {
        connectionId: sourceId,
        databaseName: qs("#db-context-database").value.trim() || undefined,
        schemaName: qs("#db-context-schema").value.trim() || undefined,
        tableName,
      },
      target: {
        connectionId: targetId,
        databaseName: qs("#db-context-database").value.trim() || undefined,
        schemaName: qs("#db-context-schema").value.trim() || undefined,
        tableName: targetTable || (sourceId === targetId ? `${tableName}_copy` : tableName),
      },
    }),
  });
  renderDbJobs({ jobs: body.job ? [body.job] : [] });
}

async function loadDbJobs() {
  const body = await api("/api/database/jobs");
  renderDbJobs(body);
}

async function loadDbJob(jobId) {
  if (!jobId) return;
  const body = await api(`/api/database/jobs/${encodeURIComponent(jobId)}`);
  renderDbJobs({ jobs: body.job ? [body.job] : [] });
}

function previousDbPage() {
  const limit = numericInputValue("#db-limit", 50, 1, 500);
  const offset = numericInputValue("#db-offset", 0, 0, 1_000_000_000);
  qs("#db-offset").value = String(Math.max(0, offset - limit));
  loadDbTableData().catch(showError);
}

function renderDbResult(body) {
  const result = body.result || body.data || body;
  const rows = result.rows || [];
  const columns = result.columns?.length
    ? result.columns.map((column) => column.name)
    : Object.keys(rows[0] || {});
  const target = qs("#db-output");
  target.className = "output-panel table-output";
  if (!rows.length || !columns.length) {
    renderJson("#db-output", body);
    return;
  }
  const summary = [
    result.tableName,
    result.statementType,
    `${result.returnedRowCount ?? result.rowCount ?? rows.length} rows`,
    result.totalRowCount !== undefined ? `${result.totalRowCount} total` : "",
    result.durationMs !== undefined ? `${result.durationMs} ms` : "",
    result.resultTruncated || result.hasMore ? "truncated" : "",
  ].filter(Boolean).join(" · ");
  const head = columns.map((column) => `<th>${escapeHtml(column)}</th>`).join("");
  const tableRows = rows.map((row) => `<tr>${columns.map((column) => {
    const value = row[column];
    return `<td>${escapeHtml(value === null || value === undefined ? "null" : typeof value === "object" ? JSON.stringify(value) : value)}</td>`;
  }).join("")}</tr>`).join("");
  const pager = result.hasMore ? `<button type="button" data-db-next-page>Next Page</button>` : "";
  const previous = result.offset > 0 ? '<button type="button" data-db-prev-page>Previous Page</button>' : "";
  target.innerHTML = `<div class="output-title"><span>${escapeHtml(summary)}</span><span class="row-actions">${previous}${pager}</span></div><div class="table-scroll"><table><thead><tr>${head}</tr></thead><tbody>${tableRows}</tbody></table></div>`;
  target.querySelector("[data-db-prev-page]")?.addEventListener("click", previousDbPage);
  target.querySelector("[data-db-next-page]")?.addEventListener("click", () => {
    const nextOffset = (result.offset || 0) + (result.limit || rows.length);
    qs("#db-offset").value = String(nextOffset);
    loadDbTableData().catch(showError);
  });
}

function renderDbJobs(body) {
  const jobs = body.jobs || [];
  const target = qs("#db-output");
  target.className = "output-panel result-list";
  if (!jobs.length) {
    target.innerHTML = '<p class="empty">No database jobs.</p>';
    return;
  }
  target.innerHTML = jobs.map((job) => `<article class="result-row">
    <header class="row-title">
      <strong>${escapeHtml(job.type || job.id)}</strong>
      <span class="badge ${job.status === "succeeded" ? "ok" : job.status === "failed" ? "danger" : "warn"}">${escapeHtml(job.status)}</span>
    </header>
    <span class="meta">${escapeHtml(job.id)} · ${escapeHtml(formatDate(job.updatedAt))}</span>
    <span>${escapeHtml([
      job.source?.connectionName || `source ${job.source?.connectionId || ""}`,
      job.source?.tableName,
      "to",
      job.target?.connectionName || `target ${job.target?.connectionId || ""}`,
      job.target?.tableName,
    ].filter(Boolean).join(" "))}</span>
    <button type="button" data-db-job-detail="${escapeHtml(job.id)}">Details</button>
    ${job.error?.message ? `<span>${escapeHtml(job.error.message)}</span>` : ""}
    ${job.logs?.length ? `<details><summary>Logs</summary><pre>${escapeHtml(job.logs.map((log) => `[${formatDate(log.timestamp)}] ${log.level}: ${log.message}`).join("\n"))}</pre></details>` : ""}
  </article>`).join("");
  target.querySelectorAll("[data-db-job-detail]").forEach((button) => {
    button.addEventListener("click", () => loadDbJob(button.dataset.dbJobDetail).catch(showError));
  });
}

function dbQualifiedTableName(tableName) {
  const connection = selectedDbConnectionProfile();
  const type = connection?.type || "sqlite";
  const databaseName = qs("#db-context-database").value.trim();
  const schemaName = qs("#db-context-schema").value.trim();
  const quote = (part) => quoteSqlIdentifier(part, type);
  if (type === "postgresql" && schemaName) {
    return `${quote(schemaName)}.${quote(tableName)}`;
  }
  if ((type === "mysql" || type === "mariadb") && databaseName) {
    return `${quote(databaseName)}.${quote(tableName)}`;
  }
  return quote(tableName);
}

function quoteSqlIdentifier(value, type) {
  const quote = type === "mysql" || type === "mariadb" ? "`" : '"';
  const escaped = String(value).replaceAll(quote, `${quote}${quote}`);
  return `${quote}${escaped}${quote}`;
}

function numericInputValue(selector, fallback, min, max) {
  const value = Number(qs(selector).value);
  if (!Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, Math.trunc(value)));
}

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
    danger: "/api/danger/run",
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
  state.lastSettingsRows = null;
  renderJson("#settings-json", body);
}

function defaultNotificationPreferences() {
  return {
    channels: {
      browser: true,
      webPush: false,
    },
    events: {
      sessionComplete: true,
      permissionRequired: true,
      processFailed: true,
    },
  };
}

function normalizeNotificationPreferences(preferences = {}) {
  const defaults = defaultNotificationPreferences();
  return {
    channels: {
      ...defaults.channels,
      ...(preferences.channels || {}),
    },
    events: {
      ...defaults.events,
      ...(preferences.events || {}),
    },
  };
}

function renderNotificationPreferences(preferences) {
  const normalized = normalizeNotificationPreferences(preferences);
  state.notificationPreferences = normalized;
  state.lastSettingsRows = null;
  qs("#notify-browser").checked = !!normalized.channels.browser;
  qs("#notify-web-push").checked = !!normalized.channels.webPush;
  qs("#notify-session-complete").checked = !!normalized.events.sessionComplete;
  qs("#notify-permission-required").checked = !!normalized.events.permissionRequired;
  qs("#notify-process-failed").checked = !!normalized.events.processFailed;
  qs("#settings-json-input").value = JSON.stringify(normalized, null, 2);
  updateNotificationStatus();
  const enabledEvents = Object.entries(normalized.events)
    .filter(([, enabled]) => enabled)
    .map(([event]) => event);
  const enabledChannels = Object.entries(normalized.channels)
    .filter(([, enabled]) => enabled)
    .map(([channel]) => channel);
  const target = qs("#settings-json");
  target.className = "output-panel result-list";
  target.innerHTML = `<article class="result-row">
    <header class="row-title">
      <strong>Notification Preferences</strong>
      <span class="badge ${enabledChannels.length ? "ok" : "warn"}">${enabledChannels.length ? "enabled" : "muted"}</span>
    </header>
    <span class="meta">Channels: ${escapeHtml(enabledChannels.join(", ") || "none")}</span>
    <span class="meta">Events: ${escapeHtml(enabledEvents.join(", ") || "none")}</span>
  </article>`;
}

function notificationPreferencesFromControls() {
  return {
    channels: {
      browser: qs("#notify-browser").checked,
      webPush: qs("#notify-web-push").checked,
    },
    events: {
      sessionComplete: qs("#notify-session-complete").checked,
      permissionRequired: qs("#notify-permission-required").checked,
      processFailed: qs("#notify-process-failed").checked,
    },
  };
}

function updateNotificationStatus() {
  const status = qs("#notification-status");
  if (!status) return;
  status.className = "badge warn";
  if (!("Notification" in window)) {
    status.textContent = "Unsupported";
    return;
  }
  const permission = Notification.permission;
  status.textContent = permission;
  status.className = `badge ${permission === "granted" ? "ok" : permission === "denied" ? "danger" : "warn"}`;
}

async function saveNotificationPreferences() {
  const preferences = notificationPreferencesFromControls();
  const body = await api("/api/settings/notification-preferences", {
    method: "PUT",
    body: JSON.stringify(preferences),
  });
  renderSettingsResponse(body);
}

async function requestNotificationPermission() {
  if (!("Notification" in window)) {
    updateNotificationStatus();
    return;
  }
  await Notification.requestPermission();
  updateNotificationStatus();
}

async function previewBrowserNotification() {
  if (!("Notification" in window)) {
    updateNotificationStatus();
    return;
  }
  if (Notification.permission !== "granted") {
    await requestNotificationPermission();
  }
  if (Notification.permission !== "granted") return;
  const title = "io-workbench";
  const options = {
    body: "Browser notifications are enabled.",
    tag: "iowb-preview",
  };
  let registration = null;
  if (navigator.serviceWorker?.ready) {
    registration = await navigator.serviceWorker.ready.catch(() => null);
  }
  if (registration?.showNotification) {
    await registration.showNotification(title, options);
  } else {
    new Notification(title, options);
  }
}

async function testPushNotificationCommand() {
  const body = await api("/api/notifications/test", {
    method: "POST",
    body: JSON.stringify({
      title: "io-workbench",
      body: "Test notification",
      preferences: notificationPreferencesFromControls(),
    }),
  });
  renderSettingsResponse(body);
}

function renderApiKeys(apiKeys) {
  state.lastSettingsRows = { type: "apiKeys", rows: apiKeys };
  const rows = filteredItems(apiKeys, qs("#settings-filter")?.value || "", [
    "keyName",
    "keyPrefix",
    "api_key",
    (key) => key.isActive ? "active" : "inactive",
  ]);
  const target = qs("#settings-json");
  target.className = "output-panel result-list";
  if (!rows.length) {
    target.innerHTML = '<p class="empty">No API keys.</p>';
    return;
  }
  const visible = rows.slice(0, state.limits.settingsRows);
  target.innerHTML = visible.map((key) => `<article class="result-row">
    <header class="row-title">
      <strong>${escapeHtml(key.keyName)}</strong>
      <span class="badge ${key.isActive ? "ok" : "warn"}">${key.isActive ? "active" : "inactive"}</span>
    </header>
    <span class="meta">ID ${key.id} · ${escapeHtml(key.keyPrefix || key.api_key || "")} · ${escapeHtml(formatDate(key.createdAt))}</span>
    <div class="row-actions">
      <button type="button" data-settings-action="api-key-toggle" data-settings-id="${key.id}" data-settings-value="${key.isActive ? "false" : "true"}">Toggle</button>
      <button type="button" data-settings-action="api-key-delete" data-settings-id="${key.id}">Delete</button>
    </div>
  </article>`).join("") + showMoreButton("settingsRows", rows.length, "settingsRows");
  bindSettingsActionButtons(target);
  bindShowMore(target);
}

function renderCreatedApiKey(apiKey) {
  state.lastSettingsRows = null;
  const target = qs("#settings-json");
  target.className = "output-panel result-list";
  target.innerHTML = `<article class="result-row">
    <header class="row-title">
      <strong>${escapeHtml(apiKey.keyName || "API Key")}</strong>
      <span class="badge ok">created</span>
    </header>
    <span class="meta">ID ${apiKey.id} · ${escapeHtml(apiKey.keyPrefix || "")}</span>
    <pre>${escapeHtml(apiKey.api_key || apiKey.apiKey || "")}</pre>
    <button type="button" data-copy-text="${escapeHtml(apiKey.api_key || apiKey.apiKey || "")}">Copy Key</button>
  </article>`;
  bindCopyButtons(target);
}

function renderCredentials(credentials) {
  state.lastSettingsRows = { type: "credentials", rows: credentials };
  const rows = filteredItems(credentials, qs("#settings-filter")?.value || "", [
    "credentialName",
    "credentialType",
    "description",
    (credential) => credential.isActive ? "active" : "inactive",
  ]);
  const target = qs("#settings-json");
  target.className = "output-panel result-list";
  if (!rows.length) {
    target.innerHTML = '<p class="empty">No credentials.</p>';
    return;
  }
  const visible = rows.slice(0, state.limits.settingsRows);
  target.innerHTML = visible.map((credential) => `<article class="result-row">
    <header class="row-title">
      <strong>${escapeHtml(credential.credentialName)}</strong>
      <span class="badge ${credential.isActive ? "ok" : "warn"}">${credential.isActive ? "active" : "inactive"}</span>
    </header>
    <span class="meta">ID ${credential.id} · ${escapeHtml(credential.credentialType)} · ${escapeHtml(formatDate(credential.updatedAt))}</span>
    ${credential.description ? `<span>${escapeHtml(credential.description)}</span>` : ""}
    <div class="row-actions">
      <button type="button" data-settings-action="credential-toggle" data-settings-id="${credential.id}" data-settings-value="${credential.isActive ? "false" : "true"}">Toggle</button>
      <button type="button" data-settings-action="credential-delete" data-settings-id="${credential.id}">Delete</button>
    </div>
  </article>`).join("") + showMoreButton("settingsRows", rows.length, "settingsRows");
  bindSettingsActionButtons(target);
  bindShowMore(target);
}

function renderSettingsRows() {
  if (!state.lastSettingsRows) return;
  if (state.lastSettingsRows.type === "apiKeys") {
    renderApiKeys(state.lastSettingsRows.rows);
  } else if (state.lastSettingsRows.type === "credentials") {
    renderCredentials(state.lastSettingsRows.rows);
  }
}

function bindSettingsActionButtons(root) {
  root.querySelectorAll("[data-settings-action]").forEach((button) => {
    button.addEventListener("click", () => {
      qs("#settings-action").value = button.dataset.settingsAction;
      qs("#settings-name").value = button.dataset.settingsId || "";
      qs("#settings-value").value = button.dataset.settingsValue || "";
    });
  });
}

async function loadSettingsView(path) {
  const body = await api(path);
  renderSettingsResponse(body);
}

async function applySettingsAction(event) {
  event.preventDefault();
  const action = qs("#settings-action").value;
  const name = qs("#settings-name").value.trim();
  const type = qs("#settings-type").value.trim();
  const value = qs("#settings-value").value.trim();
  const json = parseJsonField("#settings-json-input", {});
  let body;
  if (action === "api-key") {
    if (!name) return;
    body = await api("/api/settings/api-keys", {
      method: "POST",
      body: JSON.stringify({ keyName: name }),
    });
  } else if (action === "api-key-delete") {
    if (!name) return;
    body = await api(`/api/settings/api-keys/${encodeURIComponent(name)}`, {
      method: "DELETE",
    });
  } else if (action === "api-key-toggle") {
    if (!name) return;
    body = await api(`/api/settings/api-keys/${encodeURIComponent(name)}/toggle`, {
      method: "PATCH",
      body: JSON.stringify({ isActive: value !== "false" }),
    });
  } else if (action === "credential") {
    if (!name || !type || !value) return;
    body = await api("/api/settings/credentials", {
      method: "POST",
      body: JSON.stringify({
        credentialName: name,
        credentialType: type,
        credentialValue: value,
      }),
    });
  } else if (action === "credential-delete") {
    if (!name) return;
    body = await api(`/api/settings/credentials/${encodeURIComponent(name)}`, {
      method: "DELETE",
    });
  } else if (action === "credential-toggle") {
    if (!name) return;
    body = await api(`/api/settings/credentials/${encodeURIComponent(name)}/toggle`, {
      method: "PATCH",
      body: JSON.stringify({ isActive: value !== "false" }),
    });
  } else if (action === "git-config") {
    if (!name || !type) return;
    body = await api("/api/user/git-config", {
      method: "POST",
      body: JSON.stringify({ gitName: name, gitEmail: type }),
    });
  } else if (action === "notification") {
    body = await api("/api/settings/notification-preferences", {
      method: "PUT",
      body: JSON.stringify(json),
    });
  } else if (action === "direct-ai") {
    body = await api("/api/settings/direct-ai", {
      method: "PUT",
      body: JSON.stringify(json),
    });
  } else if (action === "onboarding") {
    body = await api("/api/user/complete-onboarding", {
      method: "POST",
    });
  }
  renderSettingsResponse(body);
}

function parseJsonField(selector, fallback) {
  const value = qs(selector).value.trim();
  if (!value) return fallback;
  return JSON.parse(value);
}

function connectWs() {
  if (state.ws) {
    const previous = state.ws;
    state.ws = null;
    previous.close();
  }
  if (state.wsRetry) {
    window.clearTimeout(state.wsRetry);
    state.wsRetry = null;
  }
  if (state.wsConnectTimer) {
    window.clearTimeout(state.wsConnectTimer);
    state.wsConnectTimer = null;
  }

  const generation = ++state.wsGeneration;
  const reconnecting = state.wsRetryAttempt > 0;
  setWsStatus(reconnecting ? "reconnecting" : "connecting", reconnecting ? `Reconnect attempt ${state.wsRetryAttempt + 1}` : "Opening WebSocket");
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  const token = state.token ? `?token=${encodeURIComponent(state.token)}` : "";
  const ws = new WebSocket(`${protocol}//${window.location.host}/ws${token}`);
  ws._iowbGeneration = generation;
  state.ws = ws;
  state.wsConnectTimer = window.setTimeout(() => {
    if (state.ws !== ws || ws.readyState === WebSocket.OPEN) return;
    setWsStatus("error", "WebSocket connection timed out");
    ws.close();
  }, WS_CONNECT_TIMEOUT_MS);

  ws.addEventListener("open", () => {
    if (state.ws !== ws || ws._iowbGeneration !== state.wsGeneration) return;
    window.clearTimeout(state.wsConnectTimer);
    state.wsConnectTimer = null;
    state.wsRetryAttempt = 0;
    setWsStatus("connected");
    ws.send(JSON.stringify({ type: "ping", nonce: String(Date.now()) }));
    ws.send(JSON.stringify({ type: "subscribe", topics: ["sessions", "processes", "projects"] }));
  });

  ws.addEventListener("message", (event) => {
    let payload;
    try {
      payload = JSON.parse(event.data);
    } catch (error) {
      setWsStatus("error", `Invalid WebSocket payload: ${error.message}`);
      return;
    }
    if (payload.type === "projects_updated") {
      state.projects = payload.projects || [];
      syncProjectOrder();
      renderProjects();
    }
    if (payload.type === "active_sessions") {
      state.sessions = payload.sessions || [];
      renderSessions();
    }
    if (payload.type === "session_status") {
      const status = String(payload.status || "").toLowerCase();
      if (isActiveChatSessionEvent(payload)) {
        state.currentSession = {
          provider: payload.provider,
          sessionId: payload.sessionId,
        };
      }
      if (status === "starting" || status === "running" || status === "waiting-for-input") {
        ensureChatProcessing(payload);
      } else if (status === "completed") {
        if (!chatStream.node || chatStream.role !== "assistant" || !state.chatBuffer) {
          finishChatProcessing(payload);
        }
      } else if (status === "failed" || status === "aborted") {
        const label = status === "aborted" ? "Aborted" : "Failed";
        if (!chatStream.node || chatStream.role !== "assistant" || !state.chatBuffer.trim()) {
          finishChatProcessing(payload, label);
        }
      }
    }
    if (payload.type === "output") {
      state.currentSession = {
        provider: payload.provider,
        sessionId: payload.sessionId,
      };
      if (payload.content) appendChat(payload.content);
      if (payload.done) {
        // Finalize the assistant stream node: store the received-at time,
        // capture token usage, and write the footer into the bubble so the
        // data persists with the message itself.
        const hasAssistantContent = Boolean(state.chatBuffer.trim());
        if (!hasAssistantContent && state.chatProcessing?.sessionId === payload.sessionId) {
          finishChatProcessing(payload);
        }
        if (!hasAssistantContent) return;
        const receivedAt = new Date().toISOString();
        const sid = payload.sessionId;
        const proj = state.preferences.chatCli || state.preferences.chatProvider || "codex";
        const finalizeBubble = (entry) => {
          if (chatStream.node && chatStream.role === "assistant") {
            renderChatLineFooter(chatStream.node.querySelector(".chat-line-footer"), entry);
          }
        };
        if (sid) {
          // Optimistically write a provisional entry so the user sees the
          // footer immediately, then refine it with token usage.
          const all = readSessionOverrides();
          const prev = all[sid] || {};
          const provisional = {
            ...prev,
            cli: proj,
            model: prev.model || state.preferences.chatModel || "",
            effort: prev.effort || state.preferences.chatEffort || "",
            mode: prev.mode || state.preferences.chatMode || "",
            receivedAt: formatReceivedDateTime(receivedAt),
          };
          if (prev.sentAt) provisional.elapsed = formatElapsed(prev.sentAt, receivedAt);
          finalizeBubble(provisional);
          const usageProvider = runtimeProviderForModel(
            proj,
            provisional.model || state.preferences.chatModel || "",
          );
          const usageModel = provisional.model || state.preferences.chatModel || "";
          if (shouldFetchTokenUsage(proj, usageModel) && findChatSession(sid)) {
            api(`/api/projects/${encodeURIComponent(state.activeProjectPath || "")}/sessions/${encodeURIComponent(sid)}/token-usage?provider=${encodeURIComponent(usageProvider)}`).then((usage) => {
              const tokenUsage = usage?.used ? `${usage.used} (in ${usage.breakdown?.input || 0} / out ${usage.breakdown?.output || 0})` : "";
              const all2 = readSessionOverrides();
              const prev2 = all2[sid] || {};
              const persistedEntry = {
                ...prev2,
                cli: proj,
                model: prev2.model || state.preferences.chatModel || "",
                effort: prev2.effort || state.preferences.chatEffort || "",
                mode: prev2.mode || state.preferences.chatMode || "",
                receivedAt: formatReceivedDateTime(receivedAt),
                tokenUsage,
              };
              if (prev2.sentAt) persistedEntry.elapsed = formatElapsed(prev2.sentAt, receivedAt);
              all2[sid] = persistedEntry;
              writeSessionOverrides(all2);
              if (state.chatSessionId === sid || state.pendingChatSessionId === sid) {
                finalizeBubble(persistedEntry);
              }
            }).catch(() => {});
          }
        }
      }
    }
    if (payload.type === "session_metadata") {
      // Server broadcasts final metadata when the agent finishes. Update the
      // footer stored on the active stream node and persist it.
      const sid = payload.sessionId;
      if (sid) {
        const all = readSessionOverrides();
        const prev = all[sid] || {};
        const receivedAt = payload.receivedAt || prev.receivedAt || new Date().toISOString();
        const tokenUsage = payload.tokenUsage
          ? (typeof payload.tokenUsage.used === "number"
            ? `${payload.tokenUsage.used} (in ${payload.tokenUsage.input || 0} / out ${payload.tokenUsage.output || 0})`
            : "")
          : prev.tokenUsage || "";
        const entry = {
          ...prev,
          cli: payload.provider || prev.cli,
          model: payload.model || prev.model || state.preferences.chatModel || "",
          effort: payload.effort || prev.effort || state.preferences.chatEffort || "",
          mode: payload.mode || prev.mode || state.preferences.chatMode || "",
          receivedAt: formatReceivedDateTime(receivedAt),
          tokenUsage,
        };
        if (prev.sentAt) entry.elapsed = formatElapsed(prev.sentAt, receivedAt);
        all[sid] = entry;
        writeSessionOverrides(all);
        if (state.chatSessionId === sid || state.pendingChatSessionId === sid) {
          if (chatStream.node && chatStream.role === "assistant") {
            renderChatLineFooter(chatStream.node.querySelector(".chat-line-footer"), entry);
          }
        }
      }
    }
    if (payload.type === "process_output") {
      appendShell(payload.data);
    }
    if (payload.type === "process_exited") {
      appendShell(`\n[process exited: ${payload.code ?? "terminated"}]\n`);
      if (state.currentShellProcess === payload.processId) {
        state.currentShellProcess = null;
        state.currentShellProjectPath = "";
        resetShellResizeTracking();
        updateShellStatus();
      }
    }
    if (payload.type === "error") {
      appendChatLine(`[error] ${payload.message}${payload.details ? `: ${payload.details}` : ""}`);
    }
  });

  ws.addEventListener("close", () => {
    if (state.ws !== ws || ws._iowbGeneration !== state.wsGeneration) return;
    window.clearTimeout(state.wsConnectTimer);
    state.wsConnectTimer = null;
    state.ws = null;
    if (!state.auth?.enabled || state.token) {
      const delay = Math.min(WS_RETRY_MAX_MS, WS_RETRY_BASE_MS * (2 ** state.wsRetryAttempt));
      state.wsRetryAttempt += 1;
      setWsStatus("reconnecting", `WebSocket closed. Retrying in ${Math.round(delay / 1000)}s`);
      state.wsRetry = window.setTimeout(connectWs, delay);
      return;
    }
    setWsStatus("error", "WebSocket closed before authentication");
  });

  ws.addEventListener("error", () => {
    if (state.ws !== ws || ws._iowbGeneration !== state.wsGeneration) return;
    setWsStatus("error", "WebSocket error");
  });
}

function bindNavigation() {
  document.querySelectorAll("button[data-view]").forEach((button) => {
    button.addEventListener("click", () => {
      switchView(button.dataset.view).catch(showError);
    });
  });
}

function scheduleShellFit(syncServer = false, delay = 120) {
  if (!state.shellTerm || activeView() !== "shell") return;
  window.clearTimeout(state.shellFitTimer);
  state.shellFitTimer = window.setTimeout(() => {
    fitShellTermToContainer(syncServer).catch(showError);
  }, delay);
}

function containShellScroll(event) {
  const output = qs("#shell-output");
  if (!output || !output.contains(event.target)) return;
  const viewport = output.querySelector(".xterm-scrollable-element") || output.querySelector(".xterm-viewport");
  const scrollTarget = viewport || output;
  if (scrollTarget.scrollHeight <= scrollTarget.clientHeight + 1) return;
  event.stopPropagation();
}

function shellTermLineHeight() {
  const row = qs("#shell-output .xterm-rows > div");
  const rowHeight = row?.getBoundingClientRect().height || 0;
  if (rowHeight > 0) return rowHeight;
  const output = qs("#shell-output");
  const lineHeight = Number.parseFloat(output ? getComputedStyle(output).lineHeight : "");
  return Number.isFinite(lineHeight) && lineHeight > 0 ? lineHeight : 18;
}

function scrollShellTermByLines(lines) {
  if (!state.shellTerm || !Number.isFinite(lines) || lines === 0) return false;
  state.shellTerm.scrollLines(lines);
  return true;
}

function handleShellWheel(event) {
  if (!state.shellTerm || event.ctrlKey) {
    containShellScroll(event);
    return;
  }
  const lineHeight = shellTermLineHeight();
  const pixels = event.deltaMode === WheelEvent.DOM_DELTA_LINE
    ? event.deltaY * lineHeight
    : event.deltaMode === WheelEvent.DOM_DELTA_PAGE
      ? event.deltaY * qs("#shell-output").clientHeight
      : event.deltaY;
  const lines = Math.sign(pixels) * Math.max(1, Math.round(Math.abs(pixels) / lineHeight));
  if (scrollShellTermByLines(lines)) {
    event.preventDefault();
    event.stopPropagation();
  }
}

function beginShellTouchScroll(event) {
  if (!state.shellTerm || event.touches.length !== 1) return;
  state.shellTouchScrollY = event.touches[0].clientY;
  state.shellTouchScrollRemainder = 0;
}

function handleShellTouchScroll(event) {
  if (!state.shellTerm || state.shellTouchScrollY === null || event.touches.length !== 1) {
    containShellScroll(event);
    return;
  }
  const nextY = event.touches[0].clientY;
  const rows = (state.shellTouchScrollY - nextY) / shellTermLineHeight();
  state.shellTouchScrollY = nextY;
  const pendingRows = state.shellTouchScrollRemainder + rows;
  const lines = pendingRows < 0 ? Math.ceil(pendingRows) : Math.floor(pendingRows);
  state.shellTouchScrollRemainder = pendingRows - lines;
  if (scrollShellTermByLines(lines)) {
    event.preventDefault();
    event.stopPropagation();
  }
}

function endShellTouchScroll() {
  state.shellTouchScrollY = null;
  state.shellTouchScrollRemainder = 0;
}

function syncFloatingNavigationPosition() {
  const viewport = window.visualViewport;
  const visualHeight = viewport?.height || window.innerHeight || document.documentElement.clientHeight || 0;
  if (visualHeight > 0) {
    document.documentElement.style.setProperty("--app-viewport-height", `${Math.round(visualHeight)}px`);
  }
  const layoutHeight = Math.max(document.documentElement.clientHeight || 0, window.innerHeight || 0);
  const rawOffset = viewport ? layoutHeight - viewport.height - viewport.offsetTop : 0;
  const offset = Number.isFinite(rawOffset) ? Math.max(0, rawOffset) : 0;
  document.documentElement.style.setProperty("--bottom-nav-keyboard-offset", `${Math.round(offset)}px`);
  scheduleShellFit(true, 320);
}

function scheduleFloatingNavigationPositionSync() {
  if (state.floatingNavSyncRaf) {
    window.cancelAnimationFrame(state.floatingNavSyncRaf);
  }
  state.floatingNavSyncRaf = window.requestAnimationFrame(() => {
    state.floatingNavSyncRaf = null;
    syncFloatingNavigationPosition();
  });
  window.clearTimeout(state.floatingNavSettleTimer);
  state.floatingNavSettleTimer = window.setTimeout(syncFloatingNavigationPosition, 360);
}

function bindFloatingNavigationPosition() {
  syncFloatingNavigationPosition();
  window.visualViewport?.addEventListener("resize", scheduleFloatingNavigationPositionSync);
  window.visualViewport?.addEventListener("scroll", scheduleFloatingNavigationPositionSync);
  window.addEventListener("resize", scheduleFloatingNavigationPositionSync);
  window.addEventListener("scroll", scheduleFloatingNavigationPositionSync, { passive: true });
}

function activeView() {
  return document.querySelector(".view.active")?.id?.replace(/-view$/, "")
    || document.querySelector("button[data-view].active")?.dataset.view
    || "chat";
}

function syncNavigationState(view = activeView()) {
  const primaryMobileViews = new Set(["chat", "files", "shell", "git", "database"]);
  qs("#bottom-more")?.classList.toggle("active", !primaryMobileViews.has(view));
  qs(".more-nav")?.classList.toggle("active", !["chat", "files", "shell", "git", "database"].includes(view));
}

async function switchView(view) {
  const panel = qs(`#${view}-view`);
  if (!panel) return false;
  if (activeView() === "files" && view !== "files" && !confirmDiscardDirtyFile()) {
    return false;
  }
  document.querySelectorAll("button[data-view]").forEach((item) => item.classList.remove("active"));
  document.querySelectorAll(".view").forEach((item) => {
    item.classList.remove("active");
    item.setAttribute("aria-hidden", "true");
  });
  document.querySelectorAll(`button[data-view="${view}"]`).forEach((item) => item.classList.add("active"));
  syncNavigationState(view);
  document.body.dataset.activeView = view;
  panel.classList.add("active");
  panel.setAttribute("aria-hidden", "false");
  const title = qs("#view-title");
  const subtitle = qs("#view-subtitle");
  if (title) title.textContent = VIEW_NAMES[view] || view;
  if (subtitle) subtitle.textContent = VIEW_SUBTITLES[view] || "";
  closeMoreSheet();
  closeSidebar();
  window.localStorage.setItem("iowb.lastView", view);
  await loadView(view);
  if (!panel.isConnected) return false;
  updateMainHeader(view);
  if (view === "shell") {
    await ensureShellRunningForActiveProject();
    scheduleShellFit(true);
  }
  if (view === "chat") {
    updateChatEmptyState();
  }
  return true;
}

async function loadView(view) {
  if (view === "files") await loadFiles();
  if (view === "git") await loadGitStatus();
  if (view === "shell") await loadProcesses();
  if (view === "database") await loadDbConnections();
  if (view === "settings") {
    await loadSettings();
    await Promise.all([
      loadMetrics().catch(showError),
      loadToolRuns().catch(showError),
    ]);
  }
  if (view === "chat" && !state.chatSessionId) {
    // No session is open yet — auto-open the most recent session so the chat
    // tab is never blank when the user lands on it.
    await autoOpenLatestChatSession().catch(showError);
  }
}

async function refreshCurrentView() {
  const view = activeView();
  if (view === "files") await loadFiles();
  else if (view === "git") await loadGitStatus();
  else if (view === "database") await loadDbConnections();
  else if (view === "settings") {
    await loadSettings();
    await loadMetrics();
    await loadToolRuns();
  }
  else if (view === "shell") await loadProcesses();
}

function commandPaletteCommands() {
  const viewCommands = Object.entries(VIEW_NAMES).map(([view, label]) => ({
    id: `view-${view}`,
    title: label,
    section: "View",
    keywords: `open switch panel ${view}`,
    run: () => switchView(view),
  }));
  const projectCommands = state.projects.map((project) => ({
    id: `project-${project.id || project.path}`,
    title: `Use ${projectDisplayName(project)}`,
    section: "Projects",
    keywords: `${projectDisplayName(project)} ${project.name} ${project.path} workspace select`,
    run: async () => {
      setActiveProject(project.path);
      renderSidebarProjects();
      await loadView(activeView());
    },
  }));
  const sessionCommands = sidebarSessions().slice(0, 30).map((session) => ({
    id: `session-${session.provider || "agent"}-${session.id}`,
    title: session.title || session.summary || session.id,
    section: "Sessions",
    keywords: `${session.id} ${session.provider || ""} ${session.projectPath || ""} chat history conversation`,
    run: async () => {
      if (session.provider) setChatProvider(session.provider);
      await pickChatSession(session.id, session.projectPath || "");
    },
  }));
  const fileCommands = flattenFileEntries(state.fileEntries)
    .filter(({ entry }) => entry.type !== "directory")
    .slice(0, 40)
    .map(({ entry }) => ({
      id: `file-${entry.path}`,
      title: entry.path,
      section: "Files",
      keywords: `${entry.name || ""} ${entry.path} edit open file`,
      run: async () => {
        if (await switchView("files")) await openFile(entry.path);
      },
    }));
  return [
    ...viewCommands,
    ...projectCommands,
    ...sessionCommands,
    ...fileCommands,
    {
      id: "refresh-current",
      title: `Refresh ${VIEW_NAMES[activeView()] || "Current View"}`,
      section: "Current",
      keywords: "reload sync update current",
      run: refreshCurrentView,
    },
    {
      id: "save-current-file",
      title: "Save Current File",
      section: "Files",
      keywords: "editor write persist ctrl s",
      disabled: () => !qs("#file-editor-path")?.value.trim(),
      run: () => saveFile(new Event("submit")),
    },
    {
      id: "focus-editor-search",
      title: "Find In File",
      section: "Files",
      keywords: "search replace editor ctrl f",
      run: async () => {
        if (await switchView("files")) {
          qs("#editor-search")?.focus();
          qs("#editor-search")?.select();
        }
      },
    },
    {
      id: "reload-current-file",
      title: "Reload Current File",
      section: "Files",
      keywords: "refresh editor discard",
      disabled: () => !qs("#file-editor-path")?.value.trim(),
      run: reloadCurrentFile,
    },
    {
      id: "copy-current-file-path",
      title: "Copy Current File Path",
      section: "Files",
      keywords: "clipboard path editor",
      disabled: () => !qs("#file-editor-path")?.value.trim(),
      run: copyCurrentFilePath,
    },
    {
      id: "git-status",
      title: "Refresh Git Status",
      section: "Git",
      keywords: "changes branch working tree",
      run: async () => {
        if (await switchView("git")) await loadGitStatus();
      },
    },
    {
      id: "git-conflicts",
      title: "Show Git Conflicts",
      section: "Git",
      keywords: "merge conflict resolve ours theirs",
      run: async () => {
        if (await switchView("git")) await loadGitConflicts();
      },
    },
    {
      id: "git-diff-selected",
      title: "Diff Selected Git File",
      section: "Git",
      keywords: "review patch changes",
      disabled: () => !selectedGitFiles().length,
      run: async () => {
        if (await switchView("git")) await gitDiffSelected();
      },
    },
    {
      id: "git-stage-selected",
      title: "Stage Selected Git Files",
      section: "Git",
      keywords: "add index selected",
      disabled: () => !selectedGitFiles().length,
      run: async () => {
        if (await switchView("git")) await gitSelectedFileOperation("/api/git/stage");
      },
    },
    {
      id: "git-unstage-selected",
      title: "Unstage Selected Git Files",
      section: "Git",
      keywords: "reset index selected",
      disabled: () => !selectedGitFiles().length,
      run: async () => {
        if (await switchView("git")) await gitSelectedFileOperation("/api/git/unstage");
      },
    },
    {
      id: "database-explorer",
      title: "Open Database Explorer",
      section: "Database",
      keywords: "schema objects tables",
      run: async () => {
        if (await switchView("database")) await loadDbExplorer();
      },
    },
    {
      id: "database-diagram",
      title: "Open Relationship Diagram",
      section: "Database",
      keywords: "erd foreign keys schema graph",
      run: async () => {
        if (await switchView("database")) await loadDbRelationshipDiagram();
      },
    },
    {
      id: "database-jobs",
      title: "Show Database Jobs",
      section: "Database",
      keywords: "import export transfer history",
      run: async () => {
        if (await switchView("database")) await loadDbJobs();
      },
    },
    {
      id: "clear-chat",
      title: "Clear Chat Output",
      section: "Chat",
      keywords: "reset transcript",
      run: async () => {
        if (await switchView("chat")) {
          state.chatBuffer = "";
          resetChatOutputDom();
        }
      },
    },
    {
      id: "abort-session",
      title: "Abort Current Chat Session",
      section: "Chat",
      keywords: "stop cancel agent",
      disabled: () => !state.currentSession,
      run: async () => {
        if (await switchView("chat") && state.currentSession && state.ws?.readyState === WebSocket.OPEN) {
          state.ws.send(JSON.stringify({
            type: "abort_session",
            provider: state.currentSession.provider,
            sessionId: state.currentSession.sessionId,
          }));
        }
      },
    },
    {
      id: "clear-shell",
      title: "Clear Shell Output",
      section: "Shell",
      keywords: "terminal reset",
      run: async () => {
        if (await switchView("shell")) {
          state.shellBuffer = "";
          renderShell();
        }
      },
    },
    {
      id: "refresh-processes",
      title: "Refresh Processes",
      section: "Shell",
      keywords: "terminal process pty",
      run: async () => {
        if (await switchView("shell")) await loadProcesses();
      },
    },
    {
      id: "interrupt-shell",
      title: "Send Ctrl-C To Shell",
      section: "Shell",
      keywords: "interrupt terminal stop",
      disabled: () => !state.currentShellProcess,
      run: async () => {
        if (await switchView("shell")) await sendShellInput("\x03");
      },
    },
    {
      id: "tools-runs",
      title: "Refresh Tool Runs",
      section: "Settings",
      keywords: "mcp commands plugins history",
      run: async () => {
        if (await switchView("settings")) {
          setSettingsTab("tools");
          await loadToolRuns();
        }
      },
    },
    {
      id: "settings-api-keys",
      title: "Open API Key Settings",
      section: "Settings",
      keywords: "credentials tokens settings",
      run: async () => {
        if (await switchView("settings")) {
          setSettingsTab("api");
          await loadSettingsView("/api/settings/api-keys");
        }
      },
    },
    {
      id: "settings-notifications",
      title: "Open Notification Settings",
      section: "Settings",
      keywords: "browser push permission preferences",
      run: async () => {
        if (await switchView("settings")) {
          setSettingsTab("notifications");
          await loadSettingsView("/api/settings/notification-preferences");
        }
      },
    },
    {
      id: "preview-browser-notification",
      title: "Preview Browser Notification",
      section: "Settings",
      keywords: "permission notify alert pwa",
      disabled: () => !("Notification" in window),
      run: async () => {
        if (await switchView("settings")) await previewBrowserNotification();
      },
    },
    {
      id: "api-docs",
      title: "Open API Docs",
      section: "Web",
      keywords: "routes reference endpoints",
      run: () => window.open("/api-docs.html", "_blank", "noopener"),
    },
    {
      id: "cache-tools",
      title: "Open Cache Tools",
      section: "Web",
      keywords: "pwa service worker offline clear",
      run: () => window.open("/clear-cache.html", "_blank", "noopener"),
    },
  ];
}

function commandPaletteFilteredCommands() {
  const query = state.commandPalette.query.trim().toLowerCase();
  const commands = commandPaletteCommands();
  if (!query) return commands;
  const tokens = query.split(/\s+/).filter(Boolean);
  return commands.filter((command) => {
    const haystack = [command.title, command.section, command.keywords]
      .filter(Boolean)
      .join(" ")
      .toLowerCase();
    return tokens.every((token) => haystack.includes(token));
  });
}

function isCommandDisabled(command) {
  return !!command.disabled?.();
}

function openCommandPalette() {
  state.commandPalette.open = true;
  state.commandPalette.query = "";
  state.commandPalette.selectedIndex = 0;
  qs("#command-palette")?.classList.remove("hidden");
  const search = qs("#command-search");
  if (search) search.value = "";
  renderCommandPalette();
  window.setTimeout(() => qs("#command-search")?.focus(), 0);
}

function closeCommandPalette() {
  state.commandPalette.open = false;
  qs("#command-palette")?.classList.add("hidden");
}

function openMoreSheet() {
  qs("#more-sheet")?.classList.remove("hidden");
}

function closeMoreSheet() {
  qs("#more-sheet")?.classList.add("hidden");
}

function openAddProjectFolderBrowser() {
  closeSidebar();
  openFolderBrowser("", { action: "add-project" });
}

function toggleSidebar() {
  if (window.matchMedia("(max-width: 760px)").matches) {
    document.body.classList.toggle("sidebar-open");
    qs("#bottom-sidebar")?.classList.toggle("active", document.body.classList.contains("sidebar-open"));
    return;
  }
  document.body.classList.toggle("sidebar-collapsed");
  qs("#bottom-sidebar")?.classList.toggle("active", !document.body.classList.contains("sidebar-collapsed"));
}

function closeSidebar() {
  document.body.classList.remove("sidebar-open");
  if (window.matchMedia("(max-width: 760px)").matches) {
    qs("#bottom-sidebar")?.classList.remove("active");
  }
}

function showToast(message, tone = "info") {
  const stack = qs("#toast-stack");
  if (!stack) return;
  const toast = document.createElement("div");
  toast.className = `toast ${tone}`;
  toast.textContent = message;
  stack.appendChild(toast);
  window.setTimeout(() => {
    toast.classList.add("leaving");
    window.setTimeout(() => toast.remove(), 180);
  }, 3200);
}

function renderCommandPalette() {
  const target = qs("#command-results");
  if (!target) return;
  const commands = commandPaletteFilteredCommands();
  state.commandPalette.selectedIndex = Math.min(
    Math.max(0, state.commandPalette.selectedIndex),
    Math.max(0, commands.length - 1),
  );
  if (!commands.length) {
    target.innerHTML = '<p class="empty">No commands found.</p>';
    return;
  }
  target.innerHTML = commands.map((command, index) => {
    const active = index === state.commandPalette.selectedIndex ? "active" : "";
    const disabled = isCommandDisabled(command);
    return `<button type="button" class="command-result ${active}" data-command-id="${escapeHtml(command.id)}" ${disabled ? "disabled" : ""}>
      <span>
        <strong>${escapeHtml(command.title)}</strong>
        <span class="command-meta">${escapeHtml(command.section || "")}</span>
      </span>
      ${disabled ? '<span class="badge warn">Unavailable</span>' : ""}
    </button>`;
  }).join("");
  target.querySelectorAll("[data-command-id]").forEach((button) => {
    button.addEventListener("mouseenter", () => {
      const index = commands.findIndex((command) => command.id === button.dataset.commandId);
      if (index >= 0) state.commandPalette.selectedIndex = index;
    });
    button.addEventListener("click", () => {
      executeCommand(button.dataset.commandId).catch(showError);
    });
  });
  target.querySelector(".command-result.active")?.scrollIntoView({ block: "nearest" });
}

function moveCommandPaletteSelection(delta) {
  const commands = commandPaletteFilteredCommands();
  if (!commands.length) return;
  let next = state.commandPalette.selectedIndex;
  for (let step = 0; step < commands.length; step += 1) {
    next = (next + delta + commands.length) % commands.length;
    if (!isCommandDisabled(commands[next])) break;
  }
  state.commandPalette.selectedIndex = next;
  renderCommandPalette();
}

async function executeCommand(commandId) {
  const command = commandPaletteCommands().find((item) => item.id === commandId)
    || commandPaletteFilteredCommands()[state.commandPalette.selectedIndex];
  if (!command || isCommandDisabled(command)) return;
  closeCommandPalette();
  await command.run();
}

function bindCommandPalette() {
  qs("#open-command-palette")?.addEventListener("click", openCommandPalette);
  qs("#sidebar-command-palette")?.addEventListener("click", openCommandPalette);
  qs("#command-palette")?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget) closeCommandPalette();
  });
  qs("#command-search")?.addEventListener("input", (event) => {
    state.commandPalette.query = event.currentTarget.value;
    state.commandPalette.selectedIndex = 0;
    renderCommandPalette();
  });
  qs("#command-search")?.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      moveCommandPaletteSelection(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      moveCommandPaletteSelection(-1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const command = commandPaletteFilteredCommands()[state.commandPalette.selectedIndex];
      executeCommand(command?.id).catch(showError);
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeCommandPalette();
    }
  });
  document.addEventListener("keydown", (event) => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      openCommandPalette();
    } else if (event.key === "Escape" && state.folderBrowser.open) {
      event.preventDefault();
      closeFolderBrowser();
    } else if (event.key === "Escape" && document.body.classList.contains("sidebar-open")) {
      event.preventDefault();
      closeSidebar();
    } else if (event.key === "Escape" && state.commandPalette.open) {
      event.preventDefault();
      closeCommandPalette();
    }
  });
}

function registerServiceWorker() {
  if (!("serviceWorker" in navigator)) return;
  window.localStorage.setItem(APP_VERSION_STORAGE_KEY, APP_VERSION);
  const hadController = Boolean(navigator.serviceWorker.controller);

  const reloadForUpdatedShell = (reason = "service-worker") => {
    if (window.sessionStorage.getItem(APP_RELOAD_STORAGE_KEY)) return;
    window.sessionStorage.setItem(APP_RELOAD_STORAGE_KEY, reason);
    const url = new URL(window.location.href);
    url.searchParams.set("v", APP_VERSION);
    window.location.replace(url.href);
  };

  navigator.serviceWorker.addEventListener("controllerchange", () => {
    if (!hadController) return;
    reloadForUpdatedShell("controllerchange");
  });
  navigator.serviceWorker.addEventListener("message", (event) => {
    const message = event.data || {};
    if (message.type === "iowb_app_updated" && message.version !== APP_VERSION) {
      reloadForUpdatedShell("app-updated");
    }
  });

  const register = async () => {
    try {
      const registration = await navigator.serviceWorker.register(`/sw.js?v=${APP_VERSION}`);
      if (registration.waiting) {
        registration.waiting.postMessage({ type: "iowb_skip_waiting" });
      }
      registration.addEventListener("updatefound", () => {
        const worker = registration.installing;
        worker?.addEventListener("statechange", () => {
          if (worker.state === "installed" && navigator.serviceWorker.controller) {
            worker.postMessage({ type: "iowb_skip_waiting" });
          }
        });
      });
    } catch {
      // The app still works without the service worker.
    }
  };
  if (document.readyState === "complete") {
    register();
  } else {
    window.addEventListener("load", register, { once: true });
  }
}

function bindForms() {
  window.addEventListener("beforeunload", (event) => {
    if (!state.currentFileDirty) return;
    event.preventDefault();
    event.returnValue = "";
  });
  window.addEventListener("resize", () => {
    scheduleShellFit(true, 320);
  });
  const chatBody = qs(".chat-body");
  chatBody?.addEventListener("touchstart", handleChatTouchStart, { passive: true });
  chatBody?.addEventListener("touchmove", handleChatTouchMove, { passive: true });
  chatBody?.addEventListener("touchend", handleChatTouchEnd, { passive: true });
  chatBody?.addEventListener("touchcancel", resetChatSwipe, { passive: true });
  qs("#pref-compact").addEventListener("change", (event) => {
    state.preferences.compact = event.currentTarget.checked;
    savePreferences();
    applyPreferences();
  });
  qs("#pref-wrap").addEventListener("change", (event) => {
    state.preferences.wrapOutput = event.currentTarget.checked;
    savePreferences();
    applyPreferences();
  });
  document.querySelectorAll("[data-settings-tab]").forEach((button) => {
    button.addEventListener("click", () => setSettingsTab(button.dataset.settingsTab));
  });
  qs("#chat-provider-setting")?.addEventListener("change", (event) => {
    setChatProvider(event.currentTarget.value);
  });
  qs("#active-project")?.addEventListener("change", (event) => {
    setActiveProject(event.currentTarget.value);
    loadView(activeView()).catch(showError);
  });
  qs("#sidebar-search")?.addEventListener("input", (event) => {
    state.sidebarSearch = event.currentTarget.value;
    renderSidebarProjects();
    renderSidebarSessions();
  });
  qs("#sidebar-new-project")?.addEventListener("click", openAddProjectFolderBrowser);
  qs("#sidebar-manage-projects")?.addEventListener("click", openAddProjectFolderBrowser);
  qs("#sidebar-refresh")?.addEventListener("click", (event) => {
    withButtonLoading(event.currentTarget, async () => {
      await Promise.all([
        loadProjects().catch(showError),
        loadView(activeView()).catch(showError),
      ]);
      showToast("Workspace refreshed", "ok");
    }).catch(showError);
  });
  qs("#bottom-sidebar")?.addEventListener("click", toggleSidebar);
  qs("#main-sidebar-toggle")?.addEventListener("click", toggleSidebar);
  document.addEventListener("pointerdown", (event) => {
    if (!document.body.classList.contains("sidebar-open")) return;
    if (!window.matchMedia("(max-width: 760px)").matches) return;
    if (event.target.closest(".sidebar") || event.target.closest("#bottom-sidebar")) return;
    closeSidebar();
  }, true);
  document.addEventListener("click", (event) => {
    if (state.fileContextMenu && !event.target.closest("#file-context-menu") && !event.target.closest("[data-file-menu]")) {
      closeFileContextMenu();
    }
    if (state.openProjectMenuPath && !event.target.closest(".project-menu-wrap")) {
      state.openProjectMenuPath = "";
      renderSidebarProjects();
    }
    if (!document.body.classList.contains("sidebar-open")) return;
    if (!window.matchMedia("(max-width: 760px)").matches) return;
    if (event.target.closest(".sidebar") || event.target.closest("#bottom-sidebar")) return;
    closeSidebar();
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeFileContextMenu();
  });
  qs("#bottom-more")?.addEventListener("click", openMoreSheet);
  qs("#more-close")?.addEventListener("click", closeMoreSheet);
  qs("#more-sheet")?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget) closeMoreSheet();
  });
  qs("#auth-password-toggle")?.addEventListener("click", () => {
    const input = qs("#auth-password");
    const button = qs("#auth-password-toggle");
    const showing = input.type === "text";
    input.type = showing ? "password" : "text";
    button.textContent = showing ? "Show" : "Hide";
    button.title = showing ? "Show password" : "Hide password";
  });
  qs("#refresh-projects")?.addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadProjects).catch(showError));
  qs("#refresh-files").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadFiles).catch(showError));
  qs("#refresh-sessions")?.addEventListener("click", renderSessions);
  qs("#refresh-git").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadGitStatus).catch(showError));
  qs("#refresh-db").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadDbConnections).catch(showError));
  qs("#refresh-tool-runs").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadToolRuns).catch(showError));
  qs("#refresh-metrics").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadMetrics).catch(showError));
  qs("#refresh-settings").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadSettings).catch(showError));
  document.querySelectorAll("[data-chat-provider-option]").forEach((button) => {
    button.addEventListener("click", () => chooseNewChatProvider(button.dataset.chatProviderOption));
  });
  qs("#folder-browser-close").addEventListener("click", closeFolderBrowser);
  qs("#folder-browser").addEventListener("click", (event) => {
    if (event.target === event.currentTarget) closeFolderBrowser();
  });
  qs("#folder-browser-home").addEventListener("click", () => loadFolderBrowser("~").catch(showError));
  qs("#folder-browser-up").addEventListener("click", () => {
    const parentPath = parentFilesystemPath(state.folderBrowser.path);
    if (parentPath) loadFolderBrowser(parentPath).catch(showError);
  });
  qs("#folder-browser-hidden").addEventListener("click", () => {
    state.folderBrowser.showHidden = !state.folderBrowser.showHidden;
    renderFolderBrowser();
  });
  qs("#folder-browser-filter").addEventListener("input", (event) => {
    state.folderBrowser.filter = event.currentTarget.value;
    renderFolderBrowser();
  });
  qs("#folder-browser-use").addEventListener("click", () => selectFolderBrowserPath().catch(showError));
  qs("#files-path").addEventListener("change", () => {
    if (confirmDiscardDirtyFile()) {
      resetVirtualList("files");
      loadFiles().catch(showError);
    }
  });
  qs("#files-filter").addEventListener("input", () => {
    resetVirtualList("files");
    renderFileEntries();
  });
  qs("#files-clear-filter")?.addEventListener("click", () => {
    qs("#files-filter").value = "";
    resetVirtualList("files");
    renderFileEntries();
    qs("#files-filter").focus();
  });
  qs("#files-select-all")?.addEventListener("click", toggleAllVisibleFilesSelection);
  qs("#files-clear-selection")?.addEventListener("click", () => {
    state.fileSelectedPaths.clear();
    renderFileEntries();
  });
  qs("#files-collapse-all")?.addEventListener("click", () => {
    state.fileExpandedPaths.clear();
    renderFileEntries();
  });
  document.querySelectorAll("[data-file-view-mode]").forEach((button) => {
    button.addEventListener("click", () => setFileTreeViewMode(button.dataset.fileViewMode));
  });
  qs("#git-filter").addEventListener("input", () => {
    resetVirtualList("gitFiles");
    renderGitFiles();
  });
  document.querySelectorAll("[data-git-view-tab]").forEach((button) => {
    button.addEventListener("click", () => setGitActiveView(button.dataset.gitViewTab));
  });
  qs("#git-select-all").addEventListener("click", () => setGitFileSelection(true));
  qs("#git-select-none").addEventListener("click", () => setGitFileSelection(false));
  qs("#sessions-filter")?.addEventListener("input", () => {
    resetVirtualList("sessions");
    renderSessions();
  });
  qs("#db-filter").addEventListener("input", renderDbConnections);
  qs("#tool-filter").addEventListener("input", () => state.lastToolRuns && renderToolRuns("#tools-output", state.lastToolRuns));
  qs("#settings-filter").addEventListener("input", renderSettingsRows);
  qs("#file-editor-content").addEventListener("input", () => {
    state.currentFileDirty = true;
    resetEditorSearch();
    updateEditorChrome();
  });
  qs("#file-editor-content").addEventListener("click", updateEditorChrome);
  qs("#file-editor-content").addEventListener("keyup", updateEditorChrome);
  qs("#file-editor-content").addEventListener("keydown", handleEditorKeydown);
  qs("#file-editor-content").addEventListener("scroll", () => {
    qs("#file-editor-lines").scrollTop = qs("#file-editor-content").scrollTop;
  });
  qs("#editor-search").addEventListener("input", () => {
    resetEditorSearch();
    refreshEditorSearchMatches();
  });
  qs("#editor-search").addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    findEditorMatch(event.shiftKey ? -1 : 1);
  });
  qs("#editor-find-prev").addEventListener("click", () => findEditorMatch(-1));
  qs("#editor-find-next").addEventListener("click", () => findEditorMatch(1));
  qs("#editor-replace-one").addEventListener("click", replaceEditorMatch);
  qs("#editor-replace-all").addEventListener("click", replaceAllEditorMatches);
  qs("#editor-go-line").addEventListener("click", goToEditorLine);
  qs("#editor-goto-line").addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    goToEditorLine();
  });
  qs("#file-editor-form").addEventListener("submit", (event) => saveFile(event).catch(showError));
  qs("#create-file").addEventListener("click", () => startCreateFileTreePath(false, qs("#files-path")?.value || "."));
  qs("#create-directory").addEventListener("click", () => startCreateFileTreePath(true, qs("#files-path")?.value || "."));
  qs("#delete-file").addEventListener("click", () => deletePath().catch(showError));
  qs("#download-file").addEventListener("click", downloadCurrentFile);
  qs("#reload-file").addEventListener("click", (event) => withButtonLoading(event.currentTarget, reloadCurrentFile).catch(showError));
  qs("#copy-file-path").addEventListener("click", (event) => copyCurrentFilePath(event).catch(showError));
  qs("#rename-file").addEventListener("click", () => renamePath().catch(showError));
  qs("#upload-files").addEventListener("click", () => {
    state.fileUploadTargetPath = qs("#files-path")?.value.trim() || ".";
    qs("#file-upload-input")?.click();
  });
  qs("#file-upload-input").addEventListener("change", () => uploadProjectFiles().catch(showError));
  qs("#folder-upload-input")?.addEventListener("change", () => uploadProjectFolder().catch(showError));
  qs("#session-search-form")?.addEventListener("submit", (event) => searchSessions(event).catch(showError));
  qs("#load-project-sessions")?.addEventListener("click", () => loadProjectSessions().catch(showError));
  qs("#load-session-messages")?.addEventListener("click", () => loadSessionMessages().catch(showError));
  qs("#load-session-model")?.addEventListener("click", () => loadSessionModel().catch(showError));
  qs("#update-session-model")?.addEventListener("click", () => updateSessionModel().catch(showError));
  qs("#load-session-token-usage")?.addEventListener("click", () => loadSessionTokenUsage().catch(showError));
  qs("#rename-session-action")?.addEventListener("click", () => renameSelectedSession().catch(showError));
  qs("#git-init").addEventListener("click", () => gitOperation("/api/git/init").catch(showError));
  qs("#git-initial-commit").addEventListener("click", () => gitOperation("/api/git/initial-commit").catch(showError));
  qs("#git-generate-message").addEventListener("click", () => generateGitMessage().catch(showError));
  qs("#git-commit").addEventListener("click", () => commitGitSelection().catch(showError));
  qs("#git-diff").addEventListener("click", () => gitDiffSelected().catch(showError));
  qs("#git-file-diff").addEventListener("click", () => gitFileDiffSelected().catch(showError));
  qs("#git-conflicts").addEventListener("click", () => loadGitConflicts().catch(showError));
  qs("#git-branches").addEventListener("click", () => setGitActiveView("branches"));
  qs("#git-commits").addEventListener("click", () => setGitActiveView("history"));
  qs("#git-remote-status").addEventListener("click", () => gitRead("/api/git/remote-status", renderGitRemoteStatus).catch(showError));
  qs("#git-fetch").addEventListener("click", (event) => withButtonLoading(event.currentTarget, () => gitOperation("/api/git/fetch")).catch(showError));
  qs("#git-pull").addEventListener("click", () => gitOperation("/api/git/pull").catch(showError));
  qs("#git-push").addEventListener("click", () => gitOperation("/api/git/push").catch(showError));
  qs("#git-stage").addEventListener("click", () => gitSelectedFileOperation("/api/git/stage").catch(showError));
  qs("#git-unstage").addEventListener("click", () => gitSelectedFileOperation("/api/git/unstage").catch(showError));
  qs("#git-checkout").addEventListener("click", () => gitBranchOperation("/api/git/checkout").catch(showError));
  qs("#git-create-branch").addEventListener("click", () => gitBranchOperation("/api/git/create-branch").catch(showError));
  qs("#git-delete-branch").addEventListener("click", () => gitBranchOperation("/api/git/delete-branch").catch(showError));
  qs("#git-set-remote").addEventListener("click", () => setGitRemote().catch(showError));
  qs("#git-publish").addEventListener("click", () => publishCurrentBranch().catch(showError));
  qs("#git-revert-local").addEventListener("click", () => gitOperation("/api/git/revert-local-commit").catch(showError));
  qs("#git-discard").addEventListener("click", () => gitSelectedFileOperation("/api/git/discard").catch(showError));
  qs("#git-delete-untracked").addEventListener("click", () => gitSelectedFileOperation("/api/git/delete-untracked").catch(showError));
  qs("#db-create-form").addEventListener("submit", (event) => createDbConnection(event).catch(showError));
  qs("#db-new-connection").addEventListener("click", resetDbConnectionForm);
  qs("#db-test-unsaved").addEventListener("click", () => testDbConnectionForm().catch(showError));
  qs("#db-delete-selected").addEventListener("click", () => deleteDbConnection().catch(showError));
  qs("#db-target-connection").addEventListener("change", (event) => {
    state.selectedDbTargetConnection = Number(event.currentTarget.value) || null;
  });
  qs("#db-query-form").addEventListener("submit", (event) => runDbQuery(event).catch(showError));
  qs("#db-explorer").addEventListener("click", () => loadDbExplorer().catch(showError));
  qs("#db-describe").addEventListener("click", () => loadDbObjectDetails().catch(showError));
  qs("#db-diagram").addEventListener("click", () => loadDbRelationshipDiagram().catch(showError));
  qs("#db-select-sql").addEventListener("click", () => setDbSql("select"));
  qs("#db-count-sql").addEventListener("click", () => setDbSql("count"));
  qs("#db-prev-page").addEventListener("click", previousDbPage);
  qs("#db-table-data").addEventListener("click", () => loadDbTableData().catch(showError));
  qs("#db-export").addEventListener("click", () => dbFileJob("/api/database/export").catch(showError));
  qs("#db-import").addEventListener("click", () => dbFileJob("/api/database/import").catch(showError));
  qs("#db-transfer").addEventListener("click", () => transferDbTable().catch(showError));
  qs("#db-jobs").addEventListener("click", () => loadDbJobs().catch(showError));
  qs("#tool-run-form").addEventListener("submit", (event) => runTool(event).catch(showError));
  qs("#mcp-server-form").addEventListener("submit", (event) => startMcpServer(event).catch(showError));
  qs("#refresh-mcp-servers").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadMcpServers).catch(showError));
  qs("#stop-mcp-server").addEventListener("click", () => stopMcpServer().catch(showError));
  qs("#audio-transcribe-form").addEventListener("submit", (event) => transcribeAudio(event).catch(showError));
  qs("#settings-action-form").addEventListener("submit", (event) => applySettingsAction(event).catch(showError));
  qs("#load-cli-status").addEventListener("click", () => loadSettingsView("/api/cli").catch(showError));
  qs("#load-user-settings").addEventListener("click", () => loadSettingsView("/api/user").catch(showError));
  qs("#load-api-keys").addEventListener("click", () => loadSettingsView("/api/settings/api-keys").catch(showError));
  qs("#load-credentials").addEventListener("click", () => loadSettingsView("/api/settings/credentials").catch(showError));
  qs("#load-notifications").addEventListener("click", () => loadSettingsView("/api/settings/notification-preferences").catch(showError));
  qs("#notification-save").addEventListener("click", () => saveNotificationPreferences().catch(showError));
  qs("#notification-permission").addEventListener("click", () => requestNotificationPermission().catch(showError));
  qs("#notification-preview").addEventListener("click", () => previewBrowserNotification().catch(showError));
  qs("#notification-test-push").addEventListener("click", () => testPushNotificationCommand().catch(showError));
  qs("#load-direct-ai").addEventListener("click", () => loadSettingsView("/api/settings/direct-ai").catch(showError));
  qs("#load-direct-ai-models").addEventListener("click", () => loadSettingsView("/api/settings/direct-ai/models").catch(showError));
  qs("#load-git-config").addEventListener("click", () => loadSettingsView("/api/user/git-config").catch(showError));
  qs("#chat-upload-images").addEventListener("click", () => qs("#chat-image-input").click());
  qs("#chat-image-input").addEventListener("change", () => uploadChatImages().catch(showError));
  qs("#chat-prompt").addEventListener("input", autosizeChatPrompt);
  qs("#chat-prompt").addEventListener("focus", autosizeChatPrompt);
  qs("#clear-chat").addEventListener("click", () => {
    const prompt = qs("#chat-prompt");
    prompt.value = "";
    autosizeChatPrompt();
    prompt.focus();
  });
  qs("#clear-shell").addEventListener("click", () => {
    state.shellBuffer = "";
    renderShell();
  });
  qs("#restart-shell").addEventListener("click", (event) => withButtonLoading(event.currentTarget, () => startShell({ force: true })).catch(showError));
  const shellOutput = qs("#shell-output");
  shellOutput.addEventListener("keydown", handleShellOutputKey);
  shellOutput.addEventListener("mousedown", focusShellTerm);
  shellOutput.addEventListener("click", focusShellTerm);
  shellOutput.addEventListener("wheel", handleShellWheel, { passive: false });
  shellOutput.addEventListener("touchstart", beginShellTouchScroll, { passive: true });
  shellOutput.addEventListener("touchmove", handleShellTouchScroll, { passive: false });
  shellOutput.addEventListener("touchend", endShellTouchScroll, { passive: true });
  shellOutput.addEventListener("touchcancel", endShellTouchScroll, { passive: true });
  qs("#shell-cols").addEventListener("change", () => {
    saveTerminalSizePreference();
    applyTerminalSizePreference(true).catch(showError);
  });
  qs("#shell-rows").addEventListener("change", () => {
    saveTerminalSizePreference();
    applyTerminalSizePreference(true).catch(showError);
  });
  bindShellShortcuts();

  qs("#auth-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const mode = qs("#auth-form").dataset.mode || "login";
    const username = mode === "otp" ? "otp" : qs("#auth-username").value.trim();
    const password = qs("#auth-password").value;
    const endpoint = mode === "setup" ? "/api/auth/register" : "/api/auth/login";

    try {
      const body = await api(endpoint, {
        method: "POST",
        body: JSON.stringify({ username, password }),
      });
      state.token = body.token || "";
      window.sessionStorage.setItem(TOKEN_STORAGE_KEY, state.token);
      window.localStorage.removeItem(TOKEN_STORAGE_KEY);
      qs("#auth-password").value = "";
      await bootstrapProtected();
      showToast(mode === "setup" ? "Account created" : "Signed in", "ok");
    } catch (error) {
      qs("#auth-message").textContent = error.message;
    }
  });

  qs("#auth-logout")?.addEventListener("click", async () => {
    try {
      await api("/api/auth/logout", { method: "POST" });
    } catch {
      // Token removal is enough for local logout.
    }
    state.token = "";
    window.sessionStorage.removeItem(TOKEN_STORAGE_KEY);
    window.localStorage.removeItem(TOKEN_STORAGE_KEY);
    showAuthPanel(authPanelMode());
    if (state.ws) state.ws.close();
    showToast("Signed out", "ok");
  });

  qs("#chat-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const projectPath = activeProjectPath();
    if (!projectPath) {
      showError(new Error("Select a project before sending chat."));
      return;
    }
    const cli = chatCliValue();
    const prompt = chatPromptWithImages(qs("#chat-prompt").value.trim());
    if (!prompt) return;
    if (!state.ws || state.ws.readyState !== WebSocket.OPEN) {
      connectWs();
      showError(new Error("Chat connection is not ready. Reconnecting now."));
      return;
    }
    clearChatProcessing();
    chatStream = { role: null, node: null, text: null, buffer: "" };
    state.chatBuffer = "";
    // Capture per-session overrides so refresh reopens the same setup.
    const model = chatModelValue();
    const effort = chatEffortValue();
    const mode = chatModeValue();
    const thinking = chatThinkingValue();
    state.preferences.chatCli = cli;
    state.preferences.chatModel = model;
    state.preferences.chatEffort = effort;
    state.preferences.chatMode = mode;
    state.preferences.chatThinking = thinking;
    savePreferences();

    const startedAt = new Date().toISOString();
    const sessionId = chatSessionIdForSubmit();
    if (sessionId) {
      saveSessionOverrides(sessionId, { cli, model, effort, mode, thinking, sentAt: startedAt });
    }
    const message = {
      type: "start_session",
      provider: cli,
      projectPath,
      prompt,
      model,
      effort,
      mode,
      thinking: thinking || undefined,
    };
    if (sessionId) message.sessionId = sessionId;
    state.ws.send(JSON.stringify(message));
    // Drop the placeholder entry for this session; the server will adopt the
    // id and the next `projects_updated` broadcast will surface the real
    // session row in its place.
    if (sessionId && Array.isArray(state.sessions)) {
      state.sessions = state.sessions.filter((session) => {
        if (!session) return false;
        if (session.id !== sessionId) return true;
        return !session.pending;
      });
      renderSidebarProjects();
    }
    if (sessionId) {
      state.chatSessionId = sessionId;
      state.preferences.lastChatSessionId = sessionId;
      savePreferences();
    }
    state.pendingChatSessionId = "";
    qs("#chat-prompt").value = "";
    autosizeChatPrompt();
    updateChatComposerState();

    // Show the user prompt in the chat with right-aligned styling, plus the
    // current overrides footer so the data below the prompt is visible.
    appendUserPromptToChat(prompt, {
      cli, model, effort, mode, thinking, sentAt: startedAt,
    });
    ensureChatProcessing({
      provider: cli,
      sessionId: sessionId || state.chatSessionId,
    });
  });
  // Chat-controls row: persist model/mode/effort changes immediately.
  qs("#chat-model")?.addEventListener("change", () => {
    state.preferences.chatModel = chatModelValue();
    savePreferences();
    // If a session is currently active, persist the override against it.
    const sid = state.chatSessionId || state.pendingChatSessionId || state.preferences.lastChatSessionId;
    if (sid) saveSessionOverrides(sid, { model: state.preferences.chatModel, cli: chatCliValue() });
  });
  qs("#chat-mode")?.addEventListener("change", () => {
    state.preferences.chatMode = chatModeValue();
    savePreferences();
    const sid = state.chatSessionId || state.pendingChatSessionId || state.preferences.lastChatSessionId;
    if (sid) saveSessionOverrides(sid, { mode: state.preferences.chatMode });
  });
  qs("#chat-effort")?.addEventListener("change", () => {
    state.preferences.chatEffort = chatEffortValue();
    savePreferences();
    const sid = state.chatSessionId || state.pendingChatSessionId || state.preferences.lastChatSessionId;
    if (sid) saveSessionOverrides(sid, { effort: state.preferences.chatEffort });
  });



  qs("#abort-session").addEventListener("click", () => {
    if (!state.currentSession || !state.ws || state.ws.readyState !== WebSocket.OPEN) return;
    state.ws.send(JSON.stringify({
      type: "abort_session",
      provider: state.currentSession.provider,
      sessionId: state.currentSession.sessionId,
    }));
  });

  qs("#stop-shell").addEventListener("click", async () => {
    if (!state.currentShellProcess) return;
    await api(`/api/process/${state.currentShellProcess}`, { method: "DELETE" });
    state.currentShellProcess = null;
    state.currentShellProjectPath = "";
    resetShellResizeTracking();
    updateShellStatus();
    loadProcesses().catch(() => {});
  });
  qs("#refresh-processes").addEventListener("click", (event) => {
    state.shellProcessListOpen = !state.shellProcessListOpen;
    withButtonLoading(event.currentTarget, loadProcesses).catch(showError);
  });
}

async function bootstrapProtected() {
  const canLoadProtected = await loadAuthStatus();
  if (!canLoadProtected) {
    setWsStatus("error");
    return;
  }
  await loadSidebarState().catch(() => {});
  await loadProjects().catch(showError);
  await Promise.allSettled([
    loadSettings().catch(showError),
    loadMetrics().catch(showError),
    loadDbConnections().catch(showError),
  ]);
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
  await switchView(targetView);
  // If we landed on the chat view and nothing is selected, auto-open the
  // most recent session for the current project (or the most recent session
  // across all projects if no project is currently active).
  if (targetView === "chat" && !state.chatSessionId) {
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
    if (meta) renderChatFooter({ ...meta, role: "user" });
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
  const sentAt = meta?.sentAt || new Date().toISOString();
  const inlineMeta = {
    cli: meta?.cli,
    model: meta?.model,
    mode: meta?.mode,
    effort: meta?.effort,
    thinking: meta?.thinking,
    sentAt: formatReceivedDateTime(sentAt),
  };
  renderChatLineFooter(footer, inlineMeta);
  output.appendChild(node);
  scrollChatToBottom();
  updateChatEmptyState();
  if (meta) renderChatFooter({ ...meta, role: "user" });
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

function appendChat(value) {
  const output = chatOutputRoot();
  if (!output) return;
  if (typeof value !== "string") value = String(value);
  state.chatBuffer = `${state.chatBuffer}${value}`.slice(-200_000);
  // Lazily spin up the assistant stream node the first time we receive
  // content after a reset / user prompt.
  if (!chatStream.node || chatStream.role !== "assistant") {
    if (!adoptChatProcessingForStream()) {
      chatStream = { role: "assistant", ...buildChatLineNode("assistant") };
      output.appendChild(chatStream.node);
    }
  }
  chatStream.text.textContent = state.chatBuffer;
  scrollChatToBottom();
  updateChatEmptyState();
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
  state.codeEditor.on("scroll", updateEditorChrome);
  refreshEditorWidget();
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
bindCommandPalette();
bindForms();
applyPreferences();
setSettingsTab(state.activeSettingsTab);
initCodeEditor();
initXterm();
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

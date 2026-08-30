const SIDEBAR_STATE_SETTING_KEY = "iowb.web.sidebar";
const SIDEBAR_STATE_UPDATED_KEY = "iowb.sidebarStateUpdatedAt";
const PINNED_CHAT_SESSIONS_KEY = "iowb.pinnedChatSessions";
const ACTIVE_CHAT_SESSION_KEY = "iowb.web.activeChatSession";
const ACTIVE_CHAT_SERVER_KEY = "iowb.web.activeChatServer";
const ACTIVE_CHAT_PROJECT_KEY = "iowb.web.activeChatProject";
const CHAT_TRANSCRIPT_CACHE_KEY = "iowb.web.chatTranscriptCache";
const APP_VERSION_STORAGE_KEY = "iowb.web.version";
const APP_RELOAD_STORAGE_KEY = `iowb.web.reloaded.${APP_VERSION}`;
const WS_CONNECT_TIMEOUT_MS = 8000;
const WS_RETRY_BASE_MS = 1200;
const WS_RETRY_MAX_MS = 10000;
const CHAT_ACTIVE_POLL_INTERVAL_MS = 2000;
const CHAT_COMPLETION_RECONCILE_DELAYS_MS = [280, 750, 1500, 3000];
const CHAT_TRANSCRIPT_CACHE_VERSION = 1;
const MAX_CHAT_TRANSCRIPT_CACHE = 16;
const CHAT_AUTOSCROLL_THRESHOLD_PX = 160;
const CHAT_HISTORY_LOAD_THRESHOLD_PX = 96;
const MAX_PROMPT_HISTORY = 80;
const PROMPT_HISTORY_PAGE_SIZE = 10;
const PROMPT_HISTORY_PREFETCH_REMAINING = 5;

// Minimum time the user must hold the project grip before movement becomes a
// drag-to-reorder gesture. Only the grip starts dragging; the rest of the row
// stays available for normal click and vertical sidebar scroll.
const SIDEBAR_DRAG_HOLD_MS = 160;
const SIDEBAR_DRAG_MOVE_PX = 5;
const CHAT_SWIPE_MIN_DISTANCE = 72;
const CHAT_SWIPE_MAX_VERTICAL_DRIFT = 64;
const CHAT_SWIPE_DIRECTION_RATIO = 1.5;

const CHAT_PROVIDERS = new Set(["codex", "claude", "gemini"]);
const CHAT_MODES = new Set(["default", "plan", "accept-edits", "bypass"]);
const CHAT_EFFORTS = new Set(["low", "medium", "high", "xhigh", "max", "ultra"]);

document.documentElement.classList.toggle(
  "android-web",
  /\bAndroid\b/i.test(navigator.userAgent || ""),
);

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
  ioGatewayStatus: null,
  ioGatewayLoadPromise: null,
  ioGatewayLoadGeneration: 0,
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
  fileLoadedDirectoryPaths: new Set(),
  fileLoadingDirectoryPaths: new Set(),
  fileLoading: false,
  fileLoadRequestId: 0,
  fileRefreshTimer: null,
  filePendingRefreshProjectPath: "",
  filePendingRefreshPaths: new Set(),
  fileContentRequestId: 0,
  fileProjectPath: "",
  fileRootPath: ".",
  fileSelectedPaths: new Set(),
  fileCreating: null,
  fileRenamingPath: "",
  fileContextMenu: null,
  sessionContextMenu: null,
  fileUploadTargetPath: "",
  fileViewMode: window.localStorage.getItem("iowb.fileViewMode") || "detailed",
  editorSearch: {
    query: "",
    matches: [],
    current: -1,
  },
  currentFileDirty: false,
  chatBuffer: "",
  chatPromptDraftSessionId: "",
  chatPromptDraftSaveTimer: null,
  chatPromptDraftLoadingSessionId: "",
  chatPromptDraftRevision: 0,
  chatPromptDraftApplied: false,
  chatPromptLastAppliedDraft: "",
  chatPromptHistory: [],
  chatPromptHistoryScope: "",
  chatPromptHistoryIndex: -1,
  chatPromptHistoryScratch: "",
  chatPromptHistoryHasOlder: false,
  chatPromptHistoryLoadScope: "",
  chatPromptHistoryPendingPreviousAfterLoad: false,
  chatEditFromHere: {
    armedUntil: 0,
    pickerOpen: false,
    items: [],
    index: -1,
    staged: null,
    submitting: false,
  },
  chatProcessing: null,
  chatStoppingSessionId: "",
  chatResponseStateBySession: {},
  chatRecoveryBySession: {},
  chatManualCompactionBySession: {},
  chatManualCompactionSuppressedResponsesBySession: {},
  chatOutputBuffersBySession: {},
  chatTranscriptCache: readJsonStorage(CHAT_TRANSCRIPT_CACHE_KEY, { version: CHAT_TRANSCRIPT_CACHE_VERSION, entries: [] }),
  chatReconcileTimers: {},
  chatActivityPollTimer: null,
  chatSuppressAutoOpenOnce: false,
  chatOlderMessagesLoadingSessionId: "",
  chatJumpToLatestPending: false,
  sessionStatusById: {},
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
  legacySidebarPinnedChatSessions: [],
  pinnedChatSessionsPersistTimer: null,
  pinnedChatSessionsRevision: 0,
  pinnedChatSessionsDirty: false,
  pinnedChatSessionsLoadGeneration: 0,
  pinnedChatSessionsSaveChain: Promise.resolve(),
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
  currentFileProjectPath: "",
  fileEditorLineCount: 0,
  suppressEditorChange: false,
  shellTerm: null,
  sidebarSearch: "",
  openProjectMenuPath: "",
  pointerProjectDrag: null,
  pointerPinnedChatDrag: null,
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
  board: null,
  boardChatSessionIds: new Set(),
  boardWsSessionId: "",
  boardLoading: false,
};

const qs = (selector) => document.querySelector(selector);
const assetLoadPromises = new Map();

function versionedAssetUrl(path) {
  const url = new URL(path, window.location.origin);
  url.searchParams.set("v", APP_VERSION);
  return `${url.pathname}${url.search}`;
}

function loadScriptOnce(path) {
  const src = versionedAssetUrl(path);
  if (assetLoadPromises.has(src)) return assetLoadPromises.get(src);
  const promise = new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = src;
    script.async = true;
    script.dataset.iowbAsset = path;
    script.addEventListener("load", () => resolve(script), { once: true });
    script.addEventListener("error", () => reject(new Error(`Unable to load ${path}`)), { once: true });
    document.head.appendChild(script);
  }).catch((error) => {
    assetLoadPromises.delete(src);
    throw error;
  });
  assetLoadPromises.set(src, promise);
  return promise;
}

function loadStylesheetOnce(path) {
  const href = versionedAssetUrl(path);
  if (assetLoadPromises.has(href)) return assetLoadPromises.get(href);
  const promise = new Promise((resolve, reject) => {
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.href = href;
    link.dataset.iowbAsset = path;
    link.addEventListener("load", () => resolve(link), { once: true });
    link.addEventListener("error", () => reject(new Error(`Unable to load ${path}`)), { once: true });
    document.head.appendChild(link);
  }).catch((error) => {
    assetLoadPromises.delete(href);
    throw error;
  });
  assetLoadPromises.set(href, promise);
  return promise;
}

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
  board: "Board",
  files: "Project Files",
  chat: "Chat",
  shell: "Shell",
  git: "Git",
  database: "Database",
  settings: "Settings",
};

const VIEW_SUBTITLES = {
  board: "Track agentic boards and task progress.",
  files: "Browse, edit, upload, and organize project files.",
  chat: "Start or resume agent sessions in the selected project.",
  shell: "Run a PTY-backed terminal in the selected project.",
  git: "Review changes, resolve conflicts, and commit selected work.",
  database: "Manage connections, explore schemas, and run SQL.",
  settings: "Configure credentials, notifications, IO Gateway, and server state.",
};

async function api(path, options = {}) {
  const headers = {
    "Content-Type": "application/json",
    ...options.headers,
  };
  if (state.token) {
    headers.Authorization = `Bearer ${state.token}`;
  }

  const response = await fetch(path, { cache: "no-store", ...options, headers });
  const text = await response.text();
  const body = text ? JSON.parse(text) : null;
  if (!response.ok) {
    if (response.status === 401) {
      showAuthPanel(authPanelMode());
    }
    const error = new Error(body?.details || body?.error || response.statusText);
    error.status = response.status;
    error.body = body;
    throw error;
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

function activeProjectRecord(selectId = "#active-project") {
  const path = activeProjectPath(selectId);
  return state.projects.find((project) => project.path === path) || null;
}

function activeProjectKey(selectId = "#active-project") {
  const project = activeProjectRecord(selectId);
  return project?.id || project?.name || "";
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
      ? `<option value="">Open prompt config to load models</option>`
      : `<option value="">Sign in to load models</option>`;
  }
  savePreferences();
  renderChatProviderPicker();
  if (!qs("#prompt-config-panel")?.classList.contains("hidden")) {
    loadChatModelsIntoSelect(value).catch(() => {});
  }
}

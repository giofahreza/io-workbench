const state = {
  serverUrl: window.localStorage.getItem("iowb.mobile.serverUrl") || "",
  token: window.localStorage.getItem("iowb.mobile.token") || "",
  projects: [],
  currentPath: ".",
  ws: null,
  retry: null,
};

const qs = (selector) => document.querySelector(selector);

function normalizeServerUrl(value) {
  return value.trim().replace(/\/+$/, "");
}

function setStatus(label, ok = false, error = false) {
  qs("#status").textContent = label;
  qs("#dot").className = `dot${ok ? " ok" : ""}${error ? " error" : ""}`;
}

function log(message) {
  const target = qs("#log");
  target.textContent += `${new Date().toLocaleTimeString()} ${message}\n`;
  target.scrollTop = target.scrollHeight;
}

async function api(path, options = {}) {
  const headers = {};
  if (state.token) {
    headers.Authorization = `Bearer ${state.token}`;
  }
  if (options.body && !(options.body instanceof FormData)) {
    headers["Content-Type"] = "application/json";
  }
  const response = await fetch(`${state.serverUrl}${path}`, { ...options, headers });
  const text = await response.text();
  const body = text ? JSON.parse(text) : null;
  if (!response.ok) {
    throw new Error(body?.error || response.statusText);
  }
  return body;
}

async function apiUpload(path, formData) {
  const headers = {};
  if (state.token) {
    headers.Authorization = `Bearer ${state.token}`;
  }
  const response = await fetch(`${state.serverUrl}${path}`, {
    method: "POST",
    headers,
    body: formData,
  });
  const text = await response.text();
  const body = text ? JSON.parse(text) : null;
  if (!response.ok) {
    throw new Error(body?.error || response.statusText);
  }
  return body;
}

async function connect() {
  if (!state.serverUrl) {
    setStatus("Server required", false, true);
    return;
  }
  window.localStorage.setItem("iowb.mobile.serverUrl", state.serverUrl);
  window.localStorage.setItem("iowb.mobile.token", state.token);
  setStatus("Checking");
  const health = await api("/health");
  log(`${health.service} ${health.version}`);
  await loadProjects();
  connectWs();
}

async function loadProjects() {
  const body = await api("/api/projects");
  const projects = body.projects || [];
  state.projects = projects;
  const container = qs("#projects");
  qs("#project-count").textContent = `${projects.length}`;
  if (!projects.length) {
    container.innerHTML = '<article class="project"><strong>No projects</strong><span>Add projects from the main web UI or CLI.</span></article>';
    renderProjectOptions();
    renderFiles([]);
    return;
  }
  container.innerHTML = projects
    .map((project) => `<article class="project"><strong>${escapeHtml(project.name)}</strong><span>${escapeHtml(project.path)}</span></article>`)
    .join("");
  renderProjectOptions();
  await loadFiles().catch((error) => log(`files: ${error.message}`));
}

function renderProjectOptions() {
  const options = state.projects
    .map((project) => `<option value="${escapeHtml(project.name)}">${escapeHtml(project.name)}</option>`)
    .join("");
  ["#upload-project", "#file-project"].forEach((selector) => {
    const select = qs(selector);
    const previous = select.value;
    select.innerHTML = options;
    if (previous) select.value = previous;
  });
}

function normalizePath(path) {
  const normalized = String(path || ".")
    .replaceAll("\\", "/")
    .replace(/^\/+/, "")
    .replace(/\/+/g, "/")
    .replace(/\/$/, "");
  return normalized && normalized !== "." ? normalized : ".";
}

function parentPath(path) {
  const normalized = normalizePath(path);
  if (normalized === ".") return ".";
  const parts = normalized.split("/").filter(Boolean);
  parts.pop();
  return parts.length ? parts.join("/") : ".";
}

async function loadFiles() {
  const project = qs("#file-project").value || state.projects[0]?.name || "";
  if (!project) {
    renderFiles([]);
    return;
  }
  state.currentPath = normalizePath(qs("#file-path").value || state.currentPath);
  qs("#file-path").value = state.currentPath;
  const body = await api(`/api/projects/${encodeURIComponent(project)}/files?path=${encodeURIComponent(state.currentPath)}`);
  renderFiles(Array.isArray(body) ? body : body.entries || []);
}

function renderFiles(entries) {
  const target = qs("#files");
  qs("#file-preview").classList.add("hidden");
  if (!entries.length) {
    target.innerHTML = '<article class="project"><strong>No files</strong><span>This path is empty or no project is selected.</span></article>';
    return;
  }
  target.innerHTML = entries.map((entry) => `<article class="project file-row">
    <span>
      <strong>${escapeHtml(entry.name)}</strong>
      <span>${escapeHtml(entry.type)} · ${escapeHtml(entry.path)}</span>
    </span>
    <button type="button" data-${entry.type === "directory" ? "dir" : "file"}="${escapeHtml(entry.path)}">
      ${entry.type === "directory" ? "Open" : "Preview"}
    </button>
  </article>`).join("");
  target.querySelectorAll("[data-dir]").forEach((button) => {
    button.addEventListener("click", () => {
      state.currentPath = normalizePath(button.dataset.dir);
      qs("#file-path").value = state.currentPath;
      loadFiles().catch((error) => log(error.message));
    });
  });
  target.querySelectorAll("[data-file]").forEach((button) => {
    button.addEventListener("click", () => previewFile(button.dataset.file).catch((error) => log(error.message)));
  });
}

async function previewFile(path) {
  const project = qs("#file-project").value || state.projects[0]?.name || "";
  if (!project || !path) return;
  const body = await api(`/api/projects/${encodeURIComponent(project)}/files/content?path=${encodeURIComponent(path)}`);
  const target = qs("#file-preview");
  target.textContent = body.content || "";
  target.classList.remove("hidden");
}

async function uploadFiles(event) {
  event.preventDefault();
  const project = qs("#upload-project").value;
  const files = [...qs("#upload-files").files];
  if (!project || !files.length) return;
  const formData = new FormData();
  formData.append("targetPath", qs("#upload-path").value.trim() || ".");
  files.forEach((file) => formData.append("files", file));
  const body = await apiUpload(`/api/projects/${encodeURIComponent(project)}/files/upload`, formData);
  log(`uploaded ${body.files?.length || files.length} file(s)`);
  qs("#upload-files").value = "";
  if (project === qs("#file-project").value) {
    await loadFiles().catch((error) => log(`files: ${error.message}`));
  }
}

function connectWs() {
  if (state.ws) {
    state.ws.close();
  }
  if (state.retry) {
    window.clearTimeout(state.retry);
    state.retry = null;
  }
  const url = new URL(state.serverUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.pathname = "/ws";
  url.search = state.token ? `?token=${encodeURIComponent(state.token)}` : "";
  const ws = new WebSocket(url.toString());
  state.ws = ws;
  setStatus("Connecting");

  ws.addEventListener("open", () => {
    setStatus("Connected", true);
    ws.send(JSON.stringify({ type: "subscribe", topics: ["sessions", "projects", "processes"] }));
    ws.send(JSON.stringify({ type: "ping", nonce: String(Date.now()) }));
  });
  ws.addEventListener("message", (event) => {
    const payload = JSON.parse(event.data);
    if (payload.type === "projects_updated") {
      loadProjects().catch((error) => log(`projects: ${error.message}`));
    } else if (payload.type === "active_sessions") {
      log(`active sessions: ${(payload.sessions || []).length}`);
    } else if (payload.type === "output" && payload.content) {
      log(payload.content.trim());
    } else if (payload.type === "error") {
      log(`error: ${payload.message}`);
    }
  });
  ws.addEventListener("close", () => {
    setStatus("Disconnected", false, true);
    state.retry = window.setTimeout(connectWs, 2500);
  });
  ws.addEventListener("error", () => setStatus("Connection error", false, true));
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (char) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[char]);
}

qs("#server-url").value = state.serverUrl;
qs("#token").value = state.token;
qs("#connect-form").addEventListener("submit", (event) => {
  event.preventDefault();
  state.serverUrl = normalizeServerUrl(qs("#server-url").value);
  state.token = qs("#token").value.trim();
  connect().catch((error) => {
    setStatus("Failed", false, true);
    log(error.message);
  });
});

qs("#refresh").addEventListener("click", () => {
  loadProjects().catch((error) => log(error.message));
});

qs("#browse-form").addEventListener("submit", (event) => {
  event.preventDefault();
  loadFiles().catch((error) => log(error.message));
});

qs("#file-parent").addEventListener("click", () => {
  state.currentPath = parentPath(qs("#file-path").value);
  qs("#file-path").value = state.currentPath;
  loadFiles().catch((error) => log(error.message));
});

qs("#file-project").addEventListener("change", () => {
  qs("#upload-project").value = qs("#file-project").value;
  loadFiles().catch((error) => log(error.message));
});

qs("#upload-project").addEventListener("change", () => {
  qs("#file-project").value = qs("#upload-project").value;
});

qs("#upload-form").addEventListener("submit", (event) => {
  uploadFiles(event).catch((error) => log(error.message));
});

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("./sw.js").catch(() => {});
}

if (state.serverUrl) {
  connect().catch((error) => {
    setStatus("Failed", false, true);
    log(error.message);
  });
}

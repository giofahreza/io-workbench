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
  const body = await api("/api/projects?includeSessions=true");
  state.projects = body.projects || [];
  hideBoardChatSessionsFromLists();
  syncProjectOrder();
  const activeExists = state.projects.some((project) => project.path === state.activeProjectPath);
  if (!activeExists) {
    setActiveProject(state.projects[0]?.path || "");
  }
  renderProjects();
}

async function loadSettings() {
  state.settings = await api("/api/settings/server-status");
  if (state.activeSettingsTab === "direct-ai") state.ioGatewayStatus = null;
  renderSettings();
}

async function loadMetrics() {
  state.metrics = await api("/api/metrics/runtime");
  renderMetrics();
}

async function loadFiles() {
  const project = activeProjectKey();
  if (!project) {
    state.fileLoadRequestId += 1;
    state.fileLoading = false;
    state.fileProjectPath = "";
    state.fileEntries = [];
    state.fileExpandedPaths.clear();
    state.fileLoadedDirectoryPaths.clear();
    state.fileLoadingDirectoryPaths.clear();
    state.fileSelectedPaths.clear();
    renderFileEntries();
    return;
  }
  const projectPath = activeProjectPath();
  const path = normalizeProjectPath(qs("#files-path").value.trim() || ".");
  const requestId = ++state.fileLoadRequestId;
  const contextChanged = state.fileProjectPath !== projectPath || state.fileRootPath !== path;
  const expandedPaths = contextChanged
    ? []
    : [...state.fileExpandedPaths].sort((left, right) => left.split("/").length - right.split("/").length);
  state.fileLoading = true;
  if (contextChanged) {
    state.fileEntries = [];
    state.fileExpandedPaths.clear();
    state.fileLoadedDirectoryPaths.clear();
    state.fileLoadingDirectoryPaths.clear();
    state.fileSelectedPaths.clear();
    state.fileCreating = null;
    state.fileRenamingPath = "";
  }
  renderFileEntries();
  try {
    const body = await api(`/api/projects/${encodeURIComponent(project)}/files?path=${encodeURIComponent(path)}&maxDepth=0`);
    if (
      requestId !== state.fileLoadRequestId
      || projectPath !== activeProjectPath()
      || path !== normalizeProjectPath(qs("#files-path").value.trim() || ".")
    ) return;
    state.fileProjectPath = projectPath;
    state.fileRootPath = path;
    state.fileEntries = Array.isArray(body) ? body : body.entries || [];
    state.fileLoadedDirectoryPaths = new Set([path]);
    state.fileLoadingDirectoryPaths.clear();
    state.fileExpandedPaths.clear();
    for (const expandedPath of expandedPaths) {
      if (!findFileEntryByPath(state.fileEntries, expandedPath)) continue;
      state.fileExpandedPaths.add(expandedPath);
      await loadFileDirectory(expandedPath, { force: true });
      if (requestId !== state.fileLoadRequestId || projectPath !== activeProjectPath()) return;
    }
    state.fileSelectedPaths = new Set(
      [...state.fileSelectedPaths].filter((selectedPath) => findFileEntryByPath(state.fileEntries, selectedPath)),
    );
    renderFileBreadcrumbs(path);
  } finally {
    if (requestId === state.fileLoadRequestId) {
      state.fileLoading = false;
      renderFileEntries();
    }
  }
}

function replaceFileEntryChildren(entries, path, children) {
  let replaced = false;
  const nextEntries = (entries || []).map((entry) => {
    if (entry.path === path) {
      replaced = true;
      return { ...entry, children };
    }
    const previousChildren = entry.children || [];
    const nextChildren = replaceFileEntryChildren(previousChildren, path, children);
    if (nextChildren !== previousChildren) {
      replaced = true;
      return { ...entry, children: nextChildren };
    }
    return entry;
  });
  return replaced ? nextEntries : entries;
}

async function loadFileDirectory(path, options = {}) {
  const normalizedPath = normalizeProjectPath(path);
  const force = !!options.force;
  if (
    !force
    && (
      state.fileLoadedDirectoryPaths.has(normalizedPath)
      || state.fileLoadingDirectoryPaths.has(normalizedPath)
    )
  ) return;
  const project = activeProjectKey();
  const projectPath = activeProjectPath();
  const requestId = state.fileLoadRequestId;
  if (!project || !findFileEntryByPath(state.fileEntries, normalizedPath)) return;
  state.fileLoadedDirectoryPaths.delete(normalizedPath);
  state.fileLoadingDirectoryPaths.add(normalizedPath);
  renderFileEntries();
  try {
    const body = await api(`/api/projects/${encodeURIComponent(project)}/files?path=${encodeURIComponent(normalizedPath)}&maxDepth=0`);
    if (requestId !== state.fileLoadRequestId || projectPath !== activeProjectPath()) return;
    const children = Array.isArray(body) ? body : body.entries || [];
    state.fileEntries = replaceFileEntryChildren(state.fileEntries, normalizedPath, children);
    state.fileLoadedDirectoryPaths.add(normalizedPath);
  } catch (error) {
    state.fileExpandedPaths.delete(normalizedPath);
    throw error;
  } finally {
    state.fileLoadingDirectoryPaths.delete(normalizedPath);
    if (requestId === state.fileLoadRequestId) renderFileEntries();
  }
}

function renderFileEntries(entries = state.fileEntries) {
  const target = qs("#files-tree");
  if (!target) return;
  const filter = qs("#files-filter")?.value.trim().toLowerCase() || "";
  const visibleEntries = filterFileEntries(entries, filter);
  target.setAttribute("aria-busy", state.fileLoading ? "true" : "false");
  renderFileToolbar(visibleEntries, filter);
  if (!visibleEntries.length) {
    target.innerHTML = `<div class="file-tree-empty">
      <span class="file-tree-empty-icon" aria-hidden="true"></span>
      <strong>${state.fileLoading ? "Loading files" : (filter ? "No matches found" : "No files found")}</strong>
      <span>${state.fileLoading ? "Reading the selected project…" : (filter ? "Try a different search or expand more folders." : "Check the selected project path.")}</span>
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

  target.querySelectorAll("[data-file-open]").forEach((button) => {
    button.addEventListener("click", () => {
      const path = button.dataset.fileOpen;
      if (!path) return;
      if (button.dataset.kind === "directory") {
        toggleFileDirectory(path).catch(showError);
      } else {
        openFile(path).catch(showError);
      }
    });
  });
  target.querySelectorAll("[data-file-row-path]").forEach((row) => {
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

function sameProjectRootPath(left, right) {
  const normalize = (value) => String(value || "").replaceAll("\\", "/").replace(/\/+$/, "");
  return normalize(left) === normalize(right);
}

function scheduleProjectFilesRefresh(payload) {
  const projectPath = String(payload?.projectPath || "");
  if (!projectPath || !sameProjectRootPath(projectPath, activeProjectPath())) return;
  (payload.paths || []).forEach((path) => state.filePendingRefreshPaths.add(normalizeProjectPath(path)));
  state.filePendingRefreshProjectPath = projectPath;
  window.clearTimeout(state.fileRefreshTimer);
  state.fileRefreshTimer = window.setTimeout(() => {
    const refreshProjectPath = state.filePendingRefreshProjectPath;
    state.fileRefreshTimer = null;
    state.filePendingRefreshProjectPath = "";
    state.filePendingRefreshPaths.clear();
    if (activeView() !== "files") return;
    if (!sameProjectRootPath(refreshProjectPath, activeProjectPath())) return;
    loadFiles().catch(showError);
  }, 250);
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
    <article class="folder-row">
      <button type="button" class="folder-info" data-folder-open-card="${escapeHtml(entry.path)}" aria-label="Open ${escapeHtml(entry.name || entry.path)}">
        <strong>${escapeHtml(entry.name || entry.path)}</strong>
        <span>${escapeHtml(entry.path)}</span>
      </button>
      <div class="folder-row-actions">
        <button type="button" class="icon-button" data-folder-open="${escapeHtml(entry.path)}" aria-label="Open folder" title="Open folder" data-symbol="open"></button>
        <button type="button" class="icon-button secondary-action" data-folder-select="${escapeHtml(entry.path)}" aria-label="${selectLabel}" title="${selectLabel}" data-symbol="${selectIcon}"></button>
      </div>
    </article>
  `).join("");
  list.querySelectorAll("[data-folder-open-card]").forEach((button) => {
    button.addEventListener("click", () => loadFolderBrowser(button.dataset.folderOpenCard).catch(showError));
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

async function toggleFileDirectory(path) {
  if (state.fileExpandedPaths.has(path)) {
    state.fileExpandedPaths.delete(path);
    renderFileEntries();
    return;
  }
  state.fileExpandedPaths.add(path);
  renderFileEntries();
  await loadFileDirectory(path);
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
      <span class="file-tree-checkbox-spacer" aria-hidden="true"></span>
      <span class="file-entry-edit">
        <span class="file-disclosure file" aria-hidden="true"></span>
        <span class="file-icon file-icon-${iconKind}" aria-hidden="true"></span>
        <input class="file-inline-input" data-file-create-input value="${name}" aria-label="${createState.directory ? "New folder name" : "New file name"}" />
      </span>
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
  const project = activeProjectKey();
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
  const loading = isDirectory && state.fileLoadingDirectoryPaths.has(entry.path);
  const renaming = state.fileRenamingPath === entry.path;
  const checked = state.fileSelectedPaths.has(entry.path) ? " checked" : "";
  const iconKind = isDirectory ? (expanded ? "folder-open" : "folder") : fileIconKind(entry.name);
  const size = isDirectory ? "" : escapeHtml(formatBytes(entry.size || 0));
  const modified = escapeHtml(formatRelativeTime(entry.modified));
  const permissions = escapeHtml(filePermissions(entry));
  const rowMode = escapeHtml(state.fileViewMode);
  const disclosure = `<span class="file-disclosure${expanded ? " open" : ""}${loading ? " loading" : ""}${isDirectory ? "" : " file"}" aria-hidden="true"></span>`;
  const displayName = renaming
    ? `<span class="file-entry-edit">${disclosure}<span class="file-icon file-icon-${iconKind}" aria-hidden="true"></span><input class="file-inline-input" data-file-rename-input="${path}" value="${name}" aria-label="Rename ${name}" /></span>`
    : `<button type="button" class="file-entry-open" data-file-open="${path}" data-kind="${isDirectory ? "directory" : "file"}" aria-label="${isDirectory ? (expanded ? "Collapse" : "Expand") : "Open file"} ${name}"${isDirectory ? ` aria-expanded="${expanded ? "true" : "false"}"` : ""}>${disclosure}<span class="file-icon file-icon-${iconKind}" aria-hidden="true"></span><span class="file-name">${name}</span></button>`;
  return `<article class="file-tree-row${renaming ? " renaming" : ""} file-tree-row-${rowMode}" data-kind="${isDirectory ? "directory" : "file"}" data-file-row-path="${path}">
    <div class="file-tree-name" style="--file-depth:${depth}">
      <input class="file-tree-checkbox" type="checkbox" data-file-select="${path}" aria-label="Select ${name}"${checked} />
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
  closeSessionContextMenu();
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
    if (entry.type === "directory") await toggleFileDirectory(path);
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

function editorModeAssetPaths(filePath) {
  const mode = editorModeForPath(filePath);
  const assets = {
    css: ["/vendor/codemirror/mode/css.js"],
    gfm: [
      "/vendor/codemirror/mode/xml.js",
      "/vendor/codemirror/mode/markdown.js",
      "/vendor/codemirror/addon/mode/overlay.js",
      "/vendor/codemirror/mode/gfm.js",
    ],
    htmlmixed: [
      "/vendor/codemirror/mode/xml.js",
      "/vendor/codemirror/mode/javascript.js",
      "/vendor/codemirror/mode/css.js",
      "/vendor/codemirror/mode/htmlmixed.js",
    ],
    javascript: ["/vendor/codemirror/mode/javascript.js"],
    python: ["/vendor/codemirror/mode/python.js"],
    rust: [
      "/vendor/codemirror/addon/simple.js",
      "/vendor/codemirror/mode/rust.js",
    ],
    shell: ["/vendor/codemirror/mode/shell.js"],
    sql: ["/vendor/codemirror/mode/sql.js"],
    toml: ["/vendor/codemirror/mode/toml.js"],
    xml: ["/vendor/codemirror/mode/xml.js"],
    yaml: ["/vendor/codemirror/mode/yaml.js"],
  };
  return assets[mode] || [];
}

async function ensureCodeEditor(filePath) {
  try {
    await Promise.all([
      loadStylesheetOnce("/vendor/codemirror/codemirror.css"),
      loadScriptOnce("/vendor/codemirror/codemirror.js"),
    ]);
    await Promise.all([
      loadScriptOnce("/vendor/codemirror/addon/matchbrackets.js"),
      loadScriptOnce("/vendor/codemirror/addon/closebrackets.js"),
    ]);
    initCodeEditor();
  } catch (error) {
    console.warn("[io-workbench] advanced editor unavailable; using textarea", error);
    return false;
  }

  try {
    for (const path of editorModeAssetPaths(filePath)) {
      await loadScriptOnce(path);
    }
  } catch (error) {
    console.warn("[io-workbench] syntax highlighting unavailable for file", filePath, error);
  }
  return Boolean(state.codeEditor);
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
  const project = activeProjectKey();
  const projectPath = activeProjectPath();
  if (!project) return;
  if (!options.skipDirtyCheck && !confirmDiscardDirtyFile()) return;
  const requestId = ++state.fileContentRequestId;
  const form = qs("#file-editor-form");
  form?.setAttribute("aria-busy", "true");
  qs("#file-editor-status").textContent = `Loading ${filePath}…`;
  try {
    const body = await api(`/api/projects/${encodeURIComponent(project)}/files/content?path=${encodeURIComponent(filePath)}`);
    if (requestId !== state.fileContentRequestId || projectPath !== activeProjectPath()) return;
    qs("#file-editor-path").value = body.path;
    state.currentFileProjectPath = projectPath;
    state.currentFileDirty = false;
    setEditorText(body.content || "");
    resetEditorSearch();
    updateEditorChrome();
    await ensureCodeEditor(body.path);
    if (requestId === state.fileContentRequestId) refreshEditorWidget(body.path);
  } finally {
    if (requestId === state.fileContentRequestId) form?.setAttribute("aria-busy", "false");
  }
}

function currentFileProjectMatches() {
  return !state.currentFileProjectPath || state.currentFileProjectPath === activeProjectPath();
}

function requireCurrentFileProject() {
  if (currentFileProjectMatches()) return;
  const project = state.projects.find((item) => item.path === state.currentFileProjectPath);
  throw new Error(`This file belongs to ${projectDisplayName(project || { path: state.currentFileProjectPath, name: state.currentFileProjectPath })}. Switch back to that project before changing it.`);
}

function closeFileEditor(options = {}) {
  if (!options.skipDirtyCheck && !confirmDiscardDirtyFile()) return false;
  state.fileContentRequestId += 1;
  state.currentFileDirty = false;
  state.currentFileProjectPath = "";
  qs("#file-editor-path").value = "";
  qs("#file-editor-form")?.setAttribute("aria-busy", "false");
  setEditorText("");
  resetEditorSearch();
  updateEditorChrome();
  if (options.focusFiles !== false) {
    window.requestAnimationFrame(() => qs("#files-filter")?.focus());
  }
  return true;
}

async function saveFile(event) {
  event.preventDefault();
  requireCurrentFileProject();
  const project = activeProjectKey();
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
  requireCurrentFileProject();
  const project = activeProjectKey();
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
  requireCurrentFileProject();
  const filePath = qs("#file-editor-path").value.trim();
  if (!filePath) return;
  if (!window.confirm(`Delete ${filePath}?`)) return;
  await deleteFilePath(filePath);
}

async function deleteFilePath(filePath) {
  const project = activeProjectKey();
  if (!project || !filePath) return;
  await api(`/api/projects/${encodeURIComponent(project)}/files`, {
    method: "DELETE",
    body: JSON.stringify({ filePath }),
  });
  if (
    qs("#file-editor-path").value.trim() === filePath
    && state.currentFileProjectPath === activeProjectPath()
  ) {
    closeFileEditor({ skipDirtyCheck: true, focusFiles: false });
  }
  await loadFiles();
  showToast(`Deleted ${filePath}`, "ok");
}

async function renamePath() {
  requireCurrentFileProject();
  const oldPath = qs("#file-editor-path").value.trim();
  const newPath = qs("#file-rename-path").value.trim();
  await renameFilePath(oldPath, newPath);
  qs("#file-rename-path").value = "";
}

async function renameFilePath(oldPath, newPath) {
  const project = activeProjectKey();
  if (!project || !oldPath || !newPath) return;
  await api(`/api/projects/${encodeURIComponent(project)}/files/rename`, {
    method: "PUT",
    body: JSON.stringify({ oldPath, newPath }),
  });
  if (
    qs("#file-editor-path").value.trim() === oldPath
    && state.currentFileProjectPath === activeProjectPath()
  ) {
    qs("#file-editor-path").value = newPath;
    refreshEditorWidget(newPath);
    updateEditorChrome();
  }
  state.fileRenamingPath = "";
  await loadFiles();
  showToast(`Renamed to ${newPath}`, "ok");
}

async function uploadProjectFiles() {
  const project = activeProjectKey();
  const files = [...qs("#file-upload-input").files];
  if (!project || !files.length) return;
  const formData = new FormData();
  formData.append("targetPath", state.fileUploadTargetPath || qs("#files-path").value.trim() || ".");
  files.forEach((file) => formData.append("files", file));
  await apiUpload(`/api/projects/${encodeURIComponent(project)}/files/upload`, formData);
  qs("#file-upload-input").value = "";
  state.fileUploadTargetPath = "";
  await loadFiles();
  showToast(`Uploaded ${files.length} file${files.length === 1 ? "" : "s"}`, "ok");
}

async function uploadProjectFolder() {
  const project = activeProjectKey();
  const files = [...qs("#folder-upload-input").files];
  if (!project || !files.length) return;
  const relativePaths = files.map((file) => normalizeProjectPath(file.webkitRelativePath || file.name));
  const formData = new FormData();
  formData.append("targetPath", qs("#files-path").value.trim() || ".");
  formData.append("relativePaths", JSON.stringify(relativePaths));
  files.forEach((file) => formData.append("files", file));
  await apiUpload(`/api/projects/${encodeURIComponent(project)}/files/upload`, formData);
  qs("#folder-upload-input").value = "";
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
  const project = activeProjectKey();
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
  requireCurrentFileProject();
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
  let line;
  let col;
  if (state.codeEditor) {
    const cursor = state.codeEditor.getCursor();
    line = cursor.line + 1;
    col = cursor.ch + 1;
  } else {
    const value = editorText();
    const lineCount = Math.max(1, value.split("\n").length);
    if (lineCount !== state.fileEditorLineCount) {
      qs("#file-editor-lines").textContent = Array.from({ length: lineCount }, (_, index) => index + 1).join("\n");
      state.fileEditorLineCount = lineCount;
    }
    const beforeCursor = value.slice(0, editorCursorIndex());
    line = beforeCursor.split("\n").length;
    col = beforeCursor.length - beforeCursor.lastIndexOf("\n");
  }
  const filePath = qs("#file-editor-path").value.trim();
  const projectMismatch = Boolean(filePath && !currentFileProjectMatches());
  document.body.classList.toggle("files-editor-open", !!filePath);
  qs("#file-editor-position").textContent = `Ln ${line}, Col ${col}`;
  qs("#file-editor-status").textContent = filePath
    ? `${projectMismatch ? "Previous project" : (state.currentFileDirty ? "Unsaved" : "Saved")} · ${filePath}`
    : "No file loaded";
  qs("#file-editor-status")?.classList.toggle("warn", projectMismatch);
  ["#file-editor-form button[type='submit']", "#delete-file", "#reload-file", "#rename-file"].forEach((selector) => {
    const control = qs(selector);
    if (control) control.disabled = !filePath || projectMismatch;
  });
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

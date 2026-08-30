async function loadGitStatus() {
  const project = activeProjectKey();
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
  const project = activeProjectKey();
  if (!project || !file) return;
  const body = await api(path, {
    method: "POST",
    body: JSON.stringify({ project, file }),
  });
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
}

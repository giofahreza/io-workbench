async function loadGitWorkspace(options = {}) {
  const project = activeProjectKey();
  if (!project) return null;
  if (!options.force && state.gitWorkspace && state.gitWorkspaceProject === project) {
    renderGitRepositorySelector(state.gitWorkspace);
    return state.gitWorkspace;
  }
  try {
    const body = await api(`/api/git/workspace?project=${encodeURIComponent(project)}`);
    state.gitWorkspace = body;
    state.gitWorkspaceProject = project;
  } catch (error) {
    // Older servers predate the workspace catalog. Keep their single-root
    // behavior working while still using repository IDs with new servers.
    if (error.status !== 404) throw error;
    state.gitWorkspace = {
      projectPath: project,
      hasRootRepository: true,
      defaultRepositoryId: "root",
      repositories: [{
        id: "root",
        name: project.split(/[\\/]/).filter(Boolean).at(-1) || "Repository",
        path: project,
        relativePath: ".",
        kind: "root",
        initialized: true,
        isDefault: true,
        branch: null,
      }],
    };
    state.gitWorkspaceProject = project;
  }
  const workspace = state.gitWorkspace;
  const current = state.gitSelectedRepositoryId;
  const available = workspace.repositories || [];
  const preferred = available.some((repository) => repository.id === current)
    ? current
    : workspace.defaultRepositoryId || (available.length === 1 ? available[0].id : "");
  state.gitSelectedRepositoryId = preferred;
  renderGitRepositorySelector(workspace);
  return workspace;
}

function renderGitRepositorySelector(workspace = state.gitWorkspace) {
  const selector = qs("#git-repository");
  const message = qs("#git-workspace-message");
  if (!selector) return;
  const repositories = workspace?.repositories || [];
  selector.innerHTML = repositories.length
    ? repositories.map((repository) => {
      const label = [repository.relativePath || repository.name, repository.kind ? `[${repository.kind}]` : "", repository.branch || ""]
        .filter(Boolean).join(" · ");
      return `<option value="${escapeHtml(repository.id)}" ${repository.id === selectedGitRepositoryId() ? "selected" : ""}>${escapeHtml(label)}${repository.initialized ? "" : " (uninitialized · initialize)"}</option>`;
    }).join("")
    : '<option value="">No Git repositories found</option>';
  selector.disabled = !repositories.length;
  if (message) {
    if (!repositories.length) {
      message.textContent = "This project is a workspace with no Git repositories. Use Init only after confirming a new root repository.";
    } else if (!workspace.hasRootRepository && repositories.filter((repository) => repository.initialized).length > 1) {
      message.textContent = "Git workspace · no main repository · choose a repository before running Git operations.";
    } else if (!selectedGitRepositoryId()) {
      message.textContent = "Choose an initialized Git repository to continue.";
    } else {
      const selected = repositories.find((repository) => repository.id === selectedGitRepositoryId());
      message.textContent = selected
        ? `${selected.name} · ${selected.kind || "repository"} · all Git operations stay inside this worktree`
        : "Git operations are scoped to the selected worktree.";
    }
  }
}

async function loadGitStatus(options = {}) {
  const project = activeProjectKey();
  if (!project) return;
  try {
    await loadGitWorkspace(options);
  } catch (error) {
    state.gitStatus = null;
    state.gitSelectedFiles = new Set();
    renderGitRepositorySelector(null);
    renderGitSummary(null);
    qs("#git-files").innerHTML = "";
    setOutput("#git-output", error.message || String(error), "error-output");
    return;
  }
  if (!selectedGitRepositoryId() || selectedGitRepository()?.initialized === false) {
    state.gitStatus = null;
    state.gitSelectedFiles = new Set();
    renderGitSummary(null);
    qs("#git-files").innerHTML = selectedGitRepository()?.initialized === false
      ? '<p class="empty">This repository is an uninitialized submodule. Use Init to check it out before viewing status.</p>'
      : '<p class="empty">Select an initialized repository to view its status.</p>';
    qs("#git-output").innerHTML = "";
    return;
  }
  qs("#git-files").innerHTML = '<p class="empty">Loading source control.</p>';
  qs("#git-output").innerHTML = "";
  let body;
  try {
    body = await api(gitQuery("/api/git/status"));
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
  if (String(status).includes("M")) return "Modified";
  if (String(status).includes("A")) return "Added";
  if (String(status).includes("D")) return "Deleted";
  if (status === "U" || status === "??") return "Untracked";
  return status || "Changed";
}

function gitStatusClass(status) {
  if (isGitConflictStatus(status)) return "status-conflict";
  if (String(status).includes("A")) return "status-a";
  if (String(status).includes("D")) return "status-d";
  if (status === "U" || status === "??") return "status-u";
  return "status-m";
}

function normalizedGitRelativePath(path) {
  return String(path || "").replaceAll("\\", "/").replace(/^\.\//, "").replace(/^\//, "");
}

function gitSubmoduleForFile(file) {
  const path = normalizedGitRelativePath(file?.path);
  return (state.gitWorkspace?.repositories || []).find((repository) => {
    const kind = String(repository.kind || "").toLowerCase();
    return (kind === "submodule" || kind === "uninitialized")
      && normalizedGitRelativePath(repository.relativePath) === path;
  }) || null;
}

function isGitSubmoduleFile(file) {
  return Boolean(file?.submoduleState || gitSubmoduleForFile(file));
}

function canStageGitFile(file) {
  return gitSubmoduleForFile(file)?.initialized !== false;
}

function canDiscardGitFile(file) {
  return !isGitSubmoduleFile(file);
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
  const isSubmodule = isGitSubmoduleFile(file);
  const canStage = canStageGitFile(file);
  return `<article class="git-file-row${active}${isGitConflictStatus(file.status) ? " conflicted" : ""}" data-git-file-row="${escapeHtml(file.path)}" style="padding-left:${12 + depth * 16}px">
    <input type="checkbox" data-git-file="${escapeHtml(file.path)}" aria-label="Stage ${escapeHtml(file.path)}"${checked}${canStage ? "" : " disabled"} />
    <button type="button" class="git-file-main" data-git-file-preview="${escapeHtml(file.path)}" title="${escapeHtml(file.path)}">
      <span class="git-file-icon" aria-hidden="true"></span>
      <strong>${escapeHtml(file.name || file.path)}</strong>
    </button>
    <span class="git-row-actions">
      <button type="button" class="icon-button" data-git-open-file="${escapeHtml(file.path)}" aria-label="Open file" title="Open file" data-symbol="open"></button>
      <button type="button" class="icon-button" data-git-file-diff="${escapeHtml(file.path)}" aria-label="Show diff" title="Show diff" data-symbol="diff"></button>
      ${isGitConflictStatus(file.status) && !isSubmodule ? `<button type="button" class="icon-button" data-git-conflict-file="${escapeHtml(file.path)}" aria-label="Resolve conflict" title="Resolve conflict" data-symbol="alert"></button>` : ""}
      ${canDiscard && (/[MDU]/.test(file.status) || isGitConflictStatus(file.status))
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
      const files = gitFilesFromStatus(state.gitStatus)
        .filter(canStageGitFile)
        .map((file) => file.path);
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
      const file = gitFilesFromStatus(state.gitStatus).find((item) => item.path === input.dataset.gitFile);
      if (input.checked && file && canStageGitFile(file)) state.gitSelectedFiles.add(input.dataset.gitFile);
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
  state.gitSelectedFiles = checked
    ? new Set(files.filter(canStageGitFile).map((file) => file.path))
    : new Set();
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
  const fileStatus = gitFilesFromStatus(state.gitStatus).find((item) => item.path === file);
  if (fileStatus && !canDiscardGitFile(fileStatus)) {
    throw new Error("Discard is unavailable for submodule pointers; select the child repository instead.");
  }
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
    body: JSON.stringify(gitBody({ file })),
  });
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
}

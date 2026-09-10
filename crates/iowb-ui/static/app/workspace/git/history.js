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
      setGitBranchSelection(button.dataset.branchName);
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
      setGitBranchSelection(button.dataset.commitUse);
    });
  });
}

function renderGitStashes(selector, body) {
  state.gitStashes = body;
  const target = qs(selector);
  const stashes = body.stashes || [];
  target.className = "output-panel result-list";
  if (!stashes.length) {
    target.innerHTML = '<p class="empty">No stashes found.</p>';
    return;
  }
  target.innerHTML = stashes.map((stash) => `<article class="result-row git-record-row">
    <header class="row-title">
      <div>
        <strong>${escapeHtml(stash.message || stash.reference)}</strong>
        <span class="meta">${escapeHtml(stash.reference)} · ${escapeHtml(stash.hash?.slice(0, 8) || "")} · ${escapeHtml(stash.author || "")}</span>
      </div>
      <span class="row-actions">
        <button type="button" data-git-stash-action="apply" data-git-stash-reference="${escapeHtml(stash.reference)}">Apply</button>
        <button type="button" data-git-stash-action="pop" data-git-stash-reference="${escapeHtml(stash.reference)}">Pop</button>
        <button type="button" data-git-stash-action="drop" data-git-stash-reference="${escapeHtml(stash.reference)}">Drop</button>
      </span>
    </header>
    <span class="meta">${escapeHtml(stash.date || "")}</span>
  </article>`).join("");
  target.querySelectorAll("[data-git-stash-action]").forEach((button) => {
    button.addEventListener("click", () => {
      const action = button.dataset.gitStashAction;
      const reference = button.dataset.gitStashReference;
      if (action === "drop" && !window.confirm(`Drop ${reference}? This cannot be undone.`)) return;
      gitStashOperation(`/api/git/stash/${action}`, reference).catch(showError);
    });
  });
}

function renderGitTags(selector, body) {
  state.gitTags = body;
  const target = qs(selector);
  const tags = body.tags || [];
  target.className = "output-panel result-list";
  if (!tags.length) {
    target.innerHTML = '<p class="empty">No tags found.</p>';
    return;
  }
  target.innerHTML = tags.map((tag) => `<article class="result-row git-record-row">
    <header class="row-title">
      <div>
        <strong>${escapeHtml(tag.name)}</strong>
        <span class="meta">${escapeHtml(tag.hash?.slice(0, 8) || "")} · ${escapeHtml(tag.objectType || "")}</span>
      </div>
      <span class="row-actions">
        <button type="button" data-git-tag-action="push" data-git-tag-name="${escapeHtml(tag.name)}">Push</button>
        <button type="button" data-git-tag-action="delete" data-git-tag-name="${escapeHtml(tag.name)}">Delete</button>
      </span>
    </header>
    <span class="meta">${escapeHtml(tag.date || "")}${tag.message ? ` · ${escapeHtml(tag.message)}` : ""}</span>
  </article>`).join("");
  target.querySelectorAll("[data-git-tag-action]").forEach((button) => {
    button.addEventListener("click", () => {
      const action = button.dataset.gitTagAction;
      const tag = button.dataset.gitTagName;
      if (action === "delete" && !window.confirm(`Delete tag ${tag}?`)) return;
      gitTagOperation(action === "push" ? "/api/git/tag/push" : "/api/git/tag/delete", tag).catch(showError);
    });
  });
}

async function gitStashOperation(path, reference) {
  const project = activeProjectKey();
  if (!project || !reference) return;
  const body = await api(path, {
    method: "POST",
    body: JSON.stringify(gitBody({ reference })),
  });
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
}

async function gitTagOperation(path, tag) {
  const project = activeProjectKey();
  if (!project || !tag) return;
  const body = await api(path, {
    method: "POST",
    body: JSON.stringify(gitBody({ tag })),
  });
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
}

async function createGitStash() {
  const project = activeProjectKey();
  if (!project) return;
  const message = qs("#git-stash-message").value.trim();
  const body = await api("/api/git/stash", {
    method: "POST",
    body: JSON.stringify(gitBody({ message: message || undefined })),
  });
  qs("#git-stash-message").value = "";
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
}

async function createGitTag() {
  const project = activeProjectKey();
  const tag = qs("#git-tag-name").value.trim();
  if (!project || !tag) return;
  const message = qs("#git-tag-message").value.trim();
  const body = await api("/api/git/tag", {
    method: "POST",
    body: JSON.stringify(gitBody({ tag, message: message || undefined })),
  });
  qs("#git-tag-name").value = "";
  qs("#git-tag-message").value = "";
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
}

async function gitCommitDiff(commit) {
  const project = activeProjectKey();
  if (!project || !commit) return;
  const body = await api(gitQuery("/api/git/commit-diff", { commit }));
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
  if (!qs("#git-branch")?.value.trim() && state.gitStatus?.branch) {
    setGitBranchSelection(state.gitStatus.branch);
  }
  await gitBranchOperation("/api/git/publish");
}

async function syncGitRemote() {
  await gitOperation("/api/git/pull");
  await gitOperation("/api/git/push");
}

async function createGitBranch() {
  const branch = window.prompt("New branch name", "")?.trim();
  if (!branch) return;
  await gitBranchOperation("/api/git/create-branch", branch);
}

async function gitBranchOperation(path, branchOverride = null) {
  const project = activeProjectKey();
  const branch = String(branchOverride ?? qs("#git-branch")?.value ?? "").trim();
  if (!project || !branch) return;
  const body = await api(path, {
    method: "POST",
    body: JSON.stringify(gitBody({ branch })),
  });
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
}

async function setGitRemote() {
  const project = activeProjectKey();
  const url = qs("#git-remote-url").value.trim();
  if (!project || !url) return;
  const body = await api("/api/git/remote", {
    method: "POST",
    body: JSON.stringify(gitBody({ name: "origin", url })),
  });
  renderGitOperation(body);
}

async function gitSelectedFileOperation(path) {
  const project = activeProjectKey();
  const selected = selectedGitFiles();
  const statusFiles = gitFilesFromStatus(state.gitStatus);
  const files = selected.filter((file) => {
    const status = statusFiles.find((item) => item.path === file);
    if (!status) return false;
    if (path === "/api/git/stage") return isUnstagedGitFile(status) && canStageGitFile(status);
    if (path === "/api/git/unstage") return isStagedGitFile(status) && canStageGitFile(status);
    if (path === "/api/git/delete-untracked") return status.status === "??";
    if (path === "/api/git/discard") return isUnstagedGitFile(status) && canDiscardGitFile(status);
    return true;
  });
  if (!project || !files.length) return;
  const results = [];
  for (const file of files) {
    results.push(await api(path, {
      method: "POST",
      body: JSON.stringify(gitBody({ file })),
    }));
  }
  renderGitOperationList(results);
  await loadGitStatus().catch(() => {});
  showToast("Git selection updated", "ok");
}

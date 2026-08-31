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
  if (!qs("#git-branch").value.trim() && state.gitStatus?.branch) {
    qs("#git-branch").value = state.gitStatus.branch;
  }
  await gitBranchOperation("/api/git/publish");
}

async function gitBranchOperation(path) {
  const project = activeProjectKey();
  const branch = qs("#git-branch").value.trim();
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

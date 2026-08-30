async function loadGitConflicts() {
  const project = activeProjectKey();
  if (!project) return;
  const body = await api(`/api/git/conflicts?project=${encodeURIComponent(project)}`);
  renderGitConflicts(body);
}

async function loadGitConflictFile(file) {
  const project = activeProjectKey();
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
  const project = activeProjectKey();
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

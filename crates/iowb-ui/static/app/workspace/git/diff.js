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
  const fileStatus = gitFilesFromStatus(state.gitStatus).find((item) => item.path === file);
  const controls = parsed.hunks.length && (!fileStatus || !isGitSubmoduleFile(fileStatus))
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
  const project = activeProjectKey();
  const file = state.currentGitDiffFile;
  const hunkIndexes = selectedGitHunks();
  if (!project || !file || !hunkIndexes.length) return;
  const body = await api("/api/git/apply-hunks", {
    method: "POST",
    body: JSON.stringify(gitBody({ file, operation, hunkIndexes })),
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

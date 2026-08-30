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
  const project = activeProjectKey();
  if (!project) return;
  const body = await api(`/api/projects/${encodeURIComponent(project)}/sessions`);
  state.sessions = (Array.isArray(body) ? body : body.sessions || [])
    .filter((session) => !isBoardChatSession(session));
  renderSessions();
  renderJson("#sessions-output", { project, sessions: state.sessions });
}

async function loadSessionTokenUsage() {
  const project = activeProjectKey();
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
  const project = activeProjectKey();
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
  const project = activeProjectKey();
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
  const project = activeProjectKey();
  if (!project) return;
  const body = await api(path, {
    method: "POST",
    body: JSON.stringify({ project }),
  });
  renderGitOperation(body);
  await loadGitStatus().catch(() => {});
}

async function gitRead(path, renderer = renderJson) {
  const project = activeProjectKey();
  if (!project) return;
  const body = await api(`${path}${path.includes("?") ? "&" : "?"}project=${encodeURIComponent(project)}`);
  renderer("#git-output", body);
}

async function gitDiffSelected() {
  const project = activeProjectKey();
  const file = selectedGitFiles()[0];
  if (!project || !file) return;
  await gitDiffForFile(file);
}

async function gitDiffForFile(file) {
  const project = activeProjectKey();
  if (!project || !file) return;
  state.currentGitDiffFile = file;
  renderGitFiles();
  const body = await api(`/api/git/diff?project=${encodeURIComponent(project)}&file=${encodeURIComponent(file)}`);
  renderGitDiff(file, body);
}

async function gitFileDiffSelected() {
  const project = activeProjectKey();
  const file = selectedGitFiles()[0];
  if (!project || !file) return;
  await gitFileReviewForFile(file);
}

async function gitFileReviewForFile(file) {
  const project = activeProjectKey();
  if (!project || !file) return;
  const body = await api(`/api/git/file-with-diff?project=${encodeURIComponent(project)}&file=${encodeURIComponent(file)}`);
  renderGitFileReview(file, body);
}

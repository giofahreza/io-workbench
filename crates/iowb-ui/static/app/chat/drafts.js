function togglePromptConfigPanel(force) {
  const panel = qs("#prompt-config-panel");
  const toggle = qs("#prompt-config-toggle");
  if (!panel || !toggle) return;
  const open = force === undefined ? panel.classList.contains("hidden") : Boolean(force);
  panel.classList.toggle("hidden", !open);
  toggle.classList.toggle("active", open);
  toggle.setAttribute("aria-expanded", open ? "true" : "false");
  if (open) {
    loadChatModelsIntoSelect(chatCliValue()).catch(() => {});
  }
}

function closeChatSessionConfigModal() {
  qs("#chat-session-config-modal")?.remove();
}

function showChatSessionConfigModal() {
  closeChatEditFromHerePicker();
  closeChatSessionConfigModal();
  const settings = chatDisplaySettings();
  const fastRequested = chatFastValue();
  const fastAvailable = chatCliValue() === "codex";
  document.body.insertAdjacentHTML("beforeend", `<div id="chat-session-config-modal" class="chat-session-config-modal">
    <section class="chat-session-config-dialog" role="dialog" aria-modal="true" aria-labelledby="chat-session-config-title">
      <header>
        <h3 id="chat-session-config-title">Chat config</h3>
        <button type="button" class="icon-button" data-chat-session-config-close aria-label="Close" title="Close" data-symbol="close"></button>
      </header>
      <div class="chat-session-config-body">
        <p class="chat-session-config-scope">${escapeHtml(chatDisplaySettingsScopeLabel())}</p>
        <label class="chat-session-config-fast${fastRequested ? " active" : ""}" title="${fastAvailable ? "Request Fast priority processing for Codex; the service decides the tier actually used" : "Fast priority requests are available only for Codex"}">
          <input type="checkbox" data-chat-fast-setting${fastRequested ? " checked" : ""}${fastAvailable ? "" : " disabled"} />
          <span class="chat-session-config-fast-copy">
            <strong>Fast priority</strong>
            <small>Request lower latency; actual availability may vary.</small>
          </span>
          <span class="chat-session-config-fast-status" data-chat-fast-setting-status>${fastAvailable ? (fastRequested ? "Requested" : "Off") : "Codex only"}</span>
        </label>
        <label><input type="checkbox" data-chat-display-setting="expandThinking"${settings.expandThinking ? " checked" : ""} /> Auto expand thinking</label>
        <label><input type="checkbox" data-chat-display-setting="expandParameters"${settings.expandParameters ? " checked" : ""} /> Auto expand parameters</label>
        <label><input type="checkbox" data-chat-display-setting="autoScrollToBottom"${settings.autoScrollToBottom ? " checked" : ""} /> Auto scroll to bottom</label>
      </div>
    </section>
  </div>`);
  const modal = qs("#chat-session-config-modal");
  modal?.addEventListener("click", (event) => {
    if (event.target === modal) closeChatSessionConfigModal();
  });
  modal?.querySelectorAll("[data-chat-session-config-close]").forEach((button) => {
    button.addEventListener("click", closeChatSessionConfigModal);
  });
  modal?.querySelectorAll("[data-chat-display-setting]").forEach((input) => {
    input.addEventListener("change", () => {
      saveChatDisplaySettings({
        expandThinking: modal.querySelector('[data-chat-display-setting="expandThinking"]')?.checked !== false,
        expandParameters: modal.querySelector('[data-chat-display-setting="expandParameters"]')?.checked !== false,
        autoScrollToBottom: modal.querySelector('[data-chat-display-setting="autoScrollToBottom"]')?.checked === true,
      });
      if (state.chatSessionId) loadChatHistoryForSession(state.chatSessionId).catch(showError);
      updateChatJumpToLatestButton();
    });
  });
  modal?.querySelector("[data-chat-fast-setting]")?.addEventListener("change", (event) => {
    setChatFastRequested(event.currentTarget.checked);
  });
  updateChatFastControl();
}

function chatOutputIsEmpty() {
  const output = chatOutputRoot();
  if (!output) return true;
  return output.children.length === 0 && !output.textContent.trim();
}

function selectedChatIsFreshDraft() {
  if (!state.chatSessionId) return true;
  const session = findChatSession(state.chatSessionId);
  return Boolean(session?.pending || state.pendingChatSessionId === state.chatSessionId);
}

function chatSessionIdForSubmit() {
  const pendingId = state.pendingChatSessionId;
  const pending = pendingId ? findChatSession(pendingId) : null;
  if (pending?.pending) return pendingId;
  return state.chatSessionId || "";
}

function currentChatDraftSessionId() {
  const id = state.chatSessionId || "";
  if (stagedChatEdit()) return "";
  if (!id || state.pendingChatSessionId === id) return "";
  const session = findChatSession(id) || cachedChatSession(id)?.session;
  if (!session || session.pending) return "";
  return id;
}

function chatDraftScope(sessionId = state.chatSessionId) {
  const server = normalizedWebServerUrl();
  const id = (sessionId || "").trim();
  const session = id ? (findChatSession(id) || cachedChatSession(id)?.session) : null;
  if (id && session && !session.pending && state.pendingChatSessionId !== id) {
    return `${server}::session::${id}`;
  }
  return `${server}::${activeProjectPath() || "default"}::new`;
}

function chatDraftStorageKey(scope = chatDraftScope()) {
  return `iowb.chatDraft:${scope}`;
}

function readLocalChatPromptDraft(sessionId = state.chatSessionId) {
  return window.localStorage.getItem(chatDraftStorageKey(chatDraftScope(sessionId))) || "";
}

function writeLocalChatPromptDraft(content, sessionId = state.chatSessionId) {
  const key = chatDraftStorageKey(chatDraftScope(sessionId));
  if (String(content || "").trim()) {
    window.localStorage.setItem(key, content || "");
  } else {
    window.localStorage.removeItem(key);
  }
}

function noteChatPromptUserEdit(value = qs("#chat-prompt")?.value || "") {
  state.chatPromptDraftRevision += 1;
  state.chatPromptDraftApplied = false;
  state.chatPromptLastAppliedDraft = "";
  state.chatPromptHistoryScratch = value || "";
}

function setChatPromptValue(value, options = {}) {
  const prompt = qs("#chat-prompt");
  if (!prompt) return;
  prompt.value = value || "";
  if (options.draftApplied) {
    state.chatPromptDraftApplied = true;
    state.chatPromptLastAppliedDraft = prompt.value;
  }
  autosizeChatPrompt();
  const length = prompt.value.length;
  try {
    prompt.setSelectionRange(length, length);
  } catch {
    // Some browsers do not expose selection APIs on detached inputs.
  }
  updateChatComposerState();
}

function applyLoadedChatPromptDraft(value, options = {}) {
  const prompt = qs("#chat-prompt");
  if (!prompt) return false;
  const draft = value || "";
  const force = options.force === true;
  const startRevision = Number(options.startRevision);
  const revisionChanged = Number.isFinite(startRevision)
    && state.chatPromptDraftRevision !== startRevision;
  const current = prompt.value || "";
  const currentIsEmpty = !current.trim();
  const currentIsAppliedDraft = state.chatPromptDraftApplied
    && current === state.chatPromptLastAppliedDraft;
  if (!force && (revisionChanged || (!currentIsEmpty && !currentIsAppliedDraft))) {
    return false;
  }
  setChatPromptValue(draft, { draftApplied: true });
  return true;
}

async function loadChatPromptDraft(sessionId, options = {}) {
  const id = (sessionId || "").trim();
  if (stagedChatEdit()?.sourceSessionId === id) return;
  const force = options.force === true || options.preserveUserInput !== true;
  const startRevision = state.chatPromptDraftRevision;
  if (!id) {
    state.chatPromptDraftSessionId = "";
    applyLoadedChatPromptDraft(readLocalChatPromptDraft(""), { force, startRevision });
    return;
  }
  const session = findChatSession(id) || cachedChatSession(id)?.session;
  if (!session || session.pending || state.pendingChatSessionId === id) {
    state.chatPromptDraftSessionId = "";
    applyLoadedChatPromptDraft(readLocalChatPromptDraft(id), { force, startRevision });
    return;
  }
  state.chatPromptDraftLoadingSessionId = id;
  try {
    const body = await api(`/api/sessions/${encodeURIComponent(id)}/draft`);
    if (state.chatPromptDraftLoadingSessionId !== id || currentChatDraftSessionId() !== id) return;
    state.chatPromptDraftSessionId = id;
    applyLoadedChatPromptDraft(body?.content || readLocalChatPromptDraft(id), { force, startRevision });
  } catch (error) {
    if (state.chatPromptDraftLoadingSessionId === id && currentChatDraftSessionId() === id) {
      applyLoadedChatPromptDraft(readLocalChatPromptDraft(id), { force, startRevision });
      console.warn("Could not load remote prompt draft", error);
    }
  } finally {
    if (state.chatPromptDraftLoadingSessionId === id) {
      state.chatPromptDraftLoadingSessionId = "";
    }
  }
}

async function saveChatPromptDraftNow() {
  if (state.chatPromptDraftSaveTimer) {
    window.clearTimeout(state.chatPromptDraftSaveTimer);
    state.chatPromptDraftSaveTimer = null;
  }
  if (stagedChatEdit()) return;
  const sessionId = currentChatDraftSessionId();
  const prompt = qs("#chat-prompt");
  if (!prompt) return;
  const content = prompt.value || "";
  writeLocalChatPromptDraft(content, sessionId);
  if (!sessionId) return;
  state.chatPromptDraftSessionId = sessionId;
  if (!content.trim()) {
    await api(`/api/sessions/${encodeURIComponent(sessionId)}/draft`, { method: "DELETE" });
    return;
  }
  await api(`/api/sessions/${encodeURIComponent(sessionId)}/draft`, {
    method: "PUT",
    body: JSON.stringify({ content }),
  });
}

function scheduleChatPromptDraftSave() {
  if (state.chatPromptDraftSaveTimer) {
    window.clearTimeout(state.chatPromptDraftSaveTimer);
  }
  state.chatPromptDraftSaveTimer = window.setTimeout(() => {
    saveChatPromptDraftNow().catch((error) => {
      console.warn("Unable to sync prompt draft", error);
    });
  }, 1000);
}

function clearRemoteChatPromptDraft(sessionId) {
  const id = (sessionId || "").trim();
  if (!id) return;
  if (state.chatPromptDraftSaveTimer) {
    window.clearTimeout(state.chatPromptDraftSaveTimer);
    state.chatPromptDraftSaveTimer = null;
  }
  api(`/api/sessions/${encodeURIComponent(id)}/draft`, { method: "DELETE" }).catch((error) => {
    console.warn("Unable to clear prompt draft", error);
  });
}

function updateChatEmptyState() {
  const emptyState = qs("#chat-empty-state");
  if (!emptyState) return;
  const shouldShow = activeView() === "chat" && chatOutputIsEmpty() && selectedChatIsFreshDraft();
  const hasProject = Boolean(activeProjectPath());
  emptyState.classList.toggle("hidden", !shouldShow);
  qs("#chat-empty-add-project")?.classList.toggle("hidden", hasProject);
  emptyState.querySelector(".chat-provider-picker")?.classList.toggle("hidden", !hasProject);
  const title = qs("#chat-empty-title");
  const description = qs("#chat-empty-description");
  if (title) title.textContent = hasProject ? "Start a new conversation" : "Add your first project";
  if (description) {
    description.textContent = hasProject
      ? "Choose an agent, then describe what you want to build or change."
      : "Choose a folder to give agents a safe workspace for files, commands, and chat sessions.";
  }
  renderChatProviderPicker();
  updateChatComposerState();
  updateChatJumpToLatestButton();
}

function chooseNewChatProvider(provider) {
  if (!CHAT_PROVIDERS_LOCAL.includes(provider)) return;
  setChatProvider(provider);
  updatePendingChatProvider(provider);
  renderChatProviderPicker();
  qs("#chat-prompt")?.focus();
}

function bindSidebarSessionActions(target) {
  target.querySelectorAll("[data-sidebar-session-card]").forEach((row) => {
    row.addEventListener("contextmenu", (event) => {
      if (event.target.closest("[data-sidebar-session-pin], [data-sidebar-session-delete], [data-sidebar-pinned-drag-handle]")) return;
      event.preventDefault();
      openSessionContextMenuFromRow(row, event.clientX, event.clientY);
    });
    let longPressTimer = null;
    const cancelLongPress = () => {
      window.clearTimeout(longPressTimer);
      longPressTimer = null;
    };
    row.addEventListener("pointerdown", (event) => {
      if (event.pointerType === "mouse") return;
      if (event.target.closest("[data-sidebar-session-pin], [data-sidebar-session-delete], [data-sidebar-pinned-drag-handle]")) return;
      cancelLongPress();
      longPressTimer = window.setTimeout(() => {
        hapticFeedback(8);
        row.dataset.sidebarSuppressClick = "true";
        openSessionContextMenuFromRow(row, event.clientX, event.clientY);
      }, 540);
    });
    row.addEventListener("pointerup", cancelLongPress);
    row.addEventListener("pointercancel", cancelLongPress);
    row.addEventListener("pointerleave", cancelLongPress);
  });
  target.querySelectorAll("[data-sidebar-session]").forEach((button) => {
    button.addEventListener("click", async (event) => {
      event.stopPropagation();
      event.preventDefault();
      const row = button.closest("[data-sidebar-session-card]");
      if (row?.dataset.sidebarSuppressClick === "true") {
        row.dataset.sidebarSuppressClick = "false";
        return;
      }
      hapticFeedback(6);
      if (button.dataset.sidebarProvider) setChatProvider(button.dataset.sidebarProvider);
      await pickChatSession(
        button.dataset.sidebarSession || "",
        button.dataset.sidebarProjectPath || "",
      );
      renderSidebarSessions();
    });
  });
  target.querySelectorAll("[data-sidebar-session-delete]").forEach((button) => {
    button.addEventListener("click", async (event) => {
      event.stopPropagation();
      event.preventDefault();
      hapticFeedback(10);
      await deleteChatSession(
        button.dataset.sidebarSessionDelete || "",
        button.dataset.sidebarProjectPath || "",
        button,
      ).catch(showError);
    });
  });
  target.querySelectorAll("[data-sidebar-session-pin]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      event.preventDefault();
      togglePinnedChatSession(
        button.dataset.sidebarSessionPin || "",
        button.dataset.sidebarProjectPath || "",
        button.dataset.sidebarProvider || "",
      );
    });
  });
}

function renderPinnedSidebarSessions() {
  const section = qs("#sidebar-pinned-section");
  const target = qs("#sidebar-pinned-sessions");
  const count = qs("#sidebar-pinned-count");
  if (!section || !target) return;
  const search = sidebarFilterText();
  const sessions = pinnedChatEntries().filter((session) => !search || sessionMatchesSidebarSearch(session));
  section.classList.toggle("hidden", sessions.length === 0);
  if (count) count.textContent = String(sessions.length);
  if (!sessions.length) {
    target.innerHTML = "";
    return;
  }
  target.innerHTML = sessions.map((session) => sidebarSessionCardHtml(session, {
    projectName: session.projectName,
    projectPath: session.projectPath,
    pinKey: session.pinKey,
    pinned: true,
    reorderable: true,
    showProject: true,
  })).join("");
  bindSidebarSessionActions(target);
  bindPinnedSessionReorder(target);
}

function renderSidebarProjects() {
  const target = qs("#sidebar-projects");
  if (!target) return;
  renderPinnedSidebarSessions();
  const activePath = activeProjectPath();
  const ordered = orderedProjects();
  let visibleProjects = ordered.filter((project) => !sidebarProjectMeta(project.path).hidden);
  if (state.projects.length && !visibleProjects.length) {
    state.projectMeta = Object.fromEntries(
      Object.entries(state.projectMeta)
        .map(([path, meta]) => [path, { ...meta, hidden: undefined }])
        .filter(([, meta]) => meta.label),
    );
    persistSidebarState();
    visibleProjects = ordered;
    showToast("Restored hidden projects", "ok");
  }
  const projects = visibleProjects.filter(projectMatchesSidebarSearch);
  if (!projects.length) {
    target.innerHTML = `<p class="sidebar-empty">${state.projects.length ? "No matching visible projects." : "No projects yet."}</p>`;
    return;
  }
  target.innerHTML = projects.map((project) => {
    const active = project.path === activePath ? " active" : "";
    const expanded = state.expandedProjectPaths.has(project.path);
    const sessions = sidebarProjectSessions(project).filter(sessionMatchesSidebarSearch);
    const sessionCount = sessions.length;
    const displayName = projectDisplayName(project);
    const menuOpen = state.openProjectMenuPath === project.path;
    const sessionHtml = expanded
      ? `<div class="project-session-list" data-sidebar-project-sessions="${escapeHtml(project.path)}">
        <button type="button" class="project-session-new-chat" data-sidebar-new-chat="${escapeHtml(project.path)}" aria-label="Start new chat in ${escapeHtml(displayName)}" title="Start new chat" data-symbol="plus">
          <span aria-hidden="true"></span>
          <strong>New chat</strong>
        </button>
        ${sessions.length
    ? sessions.map((session) => sidebarSessionCardHtml(session, {
      projectName: displayName,
      projectPath: project.path,
    })).join("")
    : '<p class="sidebar-empty">No chat sessions.</p>'}
      </div>`
      : "";
    return `<div class="project-sidebar-wrapper${expanded ? " expanded" : ""}" data-sidebar-project-wrap="${escapeHtml(project.path)}">
      <div class="project-sidebar-row project-sidebar-card${active}" data-sidebar-project-row="${escapeHtml(project.path)}">
        <button type="button" class="project-drag-handle" data-sidebar-drag-handle="${escapeHtml(project.path)}" aria-label="Drag to reorder ${escapeHtml(displayName)}" title="Drag to reorder">
          <i></i><i></i><i></i><i></i><i></i><i></i>
        </button>
        <button type="button" class="sidebar-item project-sidebar-item" data-sidebar-project="${escapeHtml(project.path)}" aria-label="Select ${escapeHtml(displayName)} and show sessions" aria-expanded="${expanded ? "true" : "false"}"${active ? ' aria-current="true"' : ""}>
          <span class="project-sidebar-text">
            <strong>${escapeHtml(displayName)}</strong>
            <span>${escapeHtml(project.path)}</span>
            <em>${sessionCount} sessions</em>
          </span>
        </button>
        <div class="project-menu-wrap">
          <button type="button" class="icon-button${menuOpen ? " active" : ""}" data-project-menu-button="${escapeHtml(project.path)}" aria-label="Project options" title="Project options" data-symbol="dots-vertical"></button>
          <div class="project-menu${menuOpen ? "" : " hidden"}" role="menu">
            <button type="button" data-project-rename="${escapeHtml(project.path)}">Rename</button>
            <button type="button" class="danger" data-project-hide="${escapeHtml(project.path)}">Remove from sidebar</button>
          </div>
        </div>
      </div>
      ${sessionHtml}
    </div>`;
  }).join("");
  target.querySelectorAll("[data-sidebar-project]").forEach((button) => {
    button.addEventListener("click", (event) => {
      if (Date.now() < state.suppressSidebarProjectClickUntil) return;
      event.preventDefault();
      event.stopPropagation();
      const isMobile = window.matchMedia("(max-width: 760px)").matches;
      const path = button.dataset.sidebarProject;
      const wasActive = path === activeProjectPath();
      const wasExpanded = state.expandedProjectPaths.has(path);
      setActiveProject(path);
      // Switching projects should reveal its sessions. Clicking the already
      // active expanded project is the intentional collapse gesture.
      if (wasActive && wasExpanded) {
        state.expandedProjectPaths.delete(path);
      } else {
        state.expandedProjectPaths.add(path);
      }
      saveExpandedProjectPaths();
      // On mobile, do NOT trigger loadView — it would auto-close the
      // sidebar via switchView, hiding the session list the user just
      // expanded. The full chat view will fire when the user taps a
      // session below.
      if (!isMobile) {
        loadView(activeView()).catch(showError);
      }
      renderSidebarProjects();
      renderSidebarSessions();
    });
  });
  target.querySelectorAll("[data-project-menu-button]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      state.openProjectMenuPath = state.openProjectMenuPath === button.dataset.projectMenuButton
        ? ""
        : button.dataset.projectMenuButton;
      renderSidebarProjects();
    });
  });
  target.querySelectorAll("[data-project-rename]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      renameSidebarProject(button.dataset.projectRename);
    });
  });
  target.querySelectorAll("[data-project-hide]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      hideSidebarProject(button.dataset.projectHide);
    });
  });
	  target.querySelectorAll("[data-sidebar-new-chat]").forEach((button) => {
	    button.addEventListener("click", (event) => {
	      event.stopPropagation();
	      event.preventDefault();
	      hapticFeedback(8);
	      startNewChatForProject(button.dataset.sidebarNewChat).catch(showError);
	    });
    button.addEventListener("pointerdown", (event) => {
      // Keep this control isolated from surrounding project/session clicks.
      event.stopPropagation();
    });
  });
  bindSidebarSessionActions(target);
  target.querySelectorAll("[data-sidebar-drag-handle]").forEach((handle) => {
    handle.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      // Initiate drag only from the dedicated grip handle, never from the
      // rest of the row. This frees the rest of the row for vertical scroll
      // gestures on touch devices and for ordinary click-to-open on desktop.
      const projectPath = handle.dataset.sidebarDragHandle;
      const row = handle.closest("[data-sidebar-project-row]");
      if (!projectPath || !row) return;
      state.pointerProjectDrag = {
        path: projectPath,
        startX: event.clientX,
        startY: event.clientY,
        startedAt: Date.now(),
        dragging: false,
      };
      hapticFeedback(6);
      // The handle lives inside the project button. Suppress the next
      // button click so a tap on the grip doesn't toggle the project.
      state.suppressSidebarProjectClickUntil = Date.now() + 600;
      // Stop the project-item button click that would otherwise fire on the
      // same pointerup once the drag ends without committing to a reorder.
      event.stopPropagation();
      event.preventDefault();
      document.addEventListener("pointermove", handleSidebarProjectPointerMove, { passive: false });
      document.addEventListener("pointerup", finishSidebarProjectPointerDrag, { once: true });
      document.addEventListener("pointercancel", finishSidebarProjectPointerDrag, { once: true });
    });
    // Keyboard fallback: focus the handle and press Space/Enter to "pick up"
    // the project, then arrow keys to move it within the sidebar list.
    handle.addEventListener("keydown", (event) => {
      if (![" ", "Enter", "ArrowUp", "ArrowDown"].includes(event.key)) return;
      event.preventDefault();
      const projectPath = handle.dataset.sidebarDragHandle;
      const projects = orderedProjects()
        .filter((project) => !sidebarProjectMeta(project.path).hidden)
        .filter(projectMatchesSidebarSearch)
        .map((p) => p.path);
      const fromIndex = projects.indexOf(projectPath);
      if (fromIndex < 0) return;
      let toIndex = fromIndex;
      if (event.key === "ArrowUp") toIndex = Math.max(0, fromIndex - 1);
      if (event.key === "ArrowDown") toIndex = Math.min(projects.length - 1, fromIndex + 1);
      if (toIndex === fromIndex) return;
      const target = projects[toIndex];
      const placement = toIndex < fromIndex ? "before" : "after";
      moveProjectOrder(projectPath, target, placement);
      hapticFeedback([8, 20, 8]);
      // Restore focus to the handle after the re-render so the user can
      // keep arrow-keying the row around the list.
      requestAnimationFrame(() => {
        const next = document.querySelector(`[data-sidebar-drag-handle="${CSS.escape(projectPath)}"]`);
        next?.focus();
      });
    });
  });
}

function formatRelativeTime(value) {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  const diffMs = Date.now() - date.getTime();
  const sec = Math.round(diffMs / 1000);
  if (sec < 0) return "just now";
  if (sec < 30) return "just now";
  if (sec < 60) return `${sec} seconds ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min} minute${min === 1 ? "" : "s"} ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr} hour${hr === 1 ? "" : "s"} ago`;
  const day = Math.floor(hr / 24);
  if (day < 30) return `${day} day${day === 1 ? "" : "s"} ago`;
  const mon = Math.floor(day / 30);
  if (mon < 12) return `${mon} month${mon === 1 ? "" : "s"} ago`;
  const yr = Math.floor(day / 365);
  return `${yr} year${yr === 1 ? "" : "s"} ago`;
}

function renderSidebarSessions() {
  const target = qs("#sidebar-sessions");
  if (!target) return;
  const search = (qs("#sidebar-search")?.value || "").trim().toLowerCase();
  const sessions = sidebarSessions()
    .filter((session) => !search || sessionMatchesSidebarSearch(session))
    .sort((a, b) => new Date(b.lastActivity || b.updatedAt || 0) - new Date(a.lastActivity || a.updatedAt || 0))
    .slice(0, 30);
  if (!sessions.length) {
    target.innerHTML = '<p class="sidebar-empty">No recent sessions.</p>';
    return;
  }
  target.innerHTML = sessions.map((session) => {
    return sidebarSessionCardHtml(session);
  }).join("");
  bindSidebarSessionActions(target);
}


// ---------------------------------------------------------------------------
// Chat history loaders.  When the user opens a chat session from the sidebar
// (or when bootstrap auto-picks the most recent session), we hydrate the
// transcript by replaying the persisted messages into `#chat-output` with
// the user prompts floated right and assistant replies left-aligned. We also
// restore the per-session overrides + footer metadata so the chat looks
// identical to the live session it was when the user last used it.
// ---------------------------------------------------------------------------

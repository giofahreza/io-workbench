function bindNavigation() {
  document.querySelectorAll("button[data-view]").forEach((button) => {
    button.addEventListener("click", () => {
      switchView(button.dataset.view).catch(showError);
    });
  });
}

function scheduleShellFit(syncServer = false, delay = 120) {
  if (!state.shellTerm || activeView() !== "shell") return;
  window.clearTimeout(state.shellFitTimer);
  state.shellFitTimer = window.setTimeout(() => {
    fitShellTermToContainer(syncServer).catch(showError);
  }, delay);
}

function containShellScroll(event) {
  const output = qs("#shell-output");
  if (!output || !output.contains(event.target)) return;
  const viewport = output.querySelector(".xterm-scrollable-element") || output.querySelector(".xterm-viewport");
  const scrollTarget = viewport || output;
  if (scrollTarget.scrollHeight <= scrollTarget.clientHeight + 1) return;
  event.stopPropagation();
}

function shellTermLineHeight() {
  const row = qs("#shell-output .xterm-rows > div");
  const rowHeight = row?.getBoundingClientRect().height || 0;
  if (rowHeight > 0) return rowHeight;
  const output = qs("#shell-output");
  const lineHeight = Number.parseFloat(output ? getComputedStyle(output).lineHeight : "");
  return Number.isFinite(lineHeight) && lineHeight > 0 ? lineHeight : 18;
}

function scrollShellTermByLines(lines) {
  if (!state.shellTerm || !Number.isFinite(lines) || lines === 0) return false;
  state.shellTerm.scrollLines(lines);
  return true;
}

function handleShellWheel(event) {
  if (!state.shellTerm || event.ctrlKey) {
    containShellScroll(event);
    return;
  }
  const lineHeight = shellTermLineHeight();
  const pixels = event.deltaMode === WheelEvent.DOM_DELTA_LINE
    ? event.deltaY * lineHeight
    : event.deltaMode === WheelEvent.DOM_DELTA_PAGE
      ? event.deltaY * qs("#shell-output").clientHeight
      : event.deltaY;
  const lines = Math.sign(pixels) * Math.max(1, Math.round(Math.abs(pixels) / lineHeight));
  if (scrollShellTermByLines(lines)) {
    event.preventDefault();
    event.stopPropagation();
  }
}

function beginShellTouchScroll(event) {
  if (!state.shellTerm || event.touches.length !== 1) return;
  state.shellTouchScrollY = event.touches[0].clientY;
  state.shellTouchScrollRemainder = 0;
}

function handleShellTouchScroll(event) {
  if (!state.shellTerm || state.shellTouchScrollY === null || event.touches.length !== 1) {
    containShellScroll(event);
    return;
  }
  const nextY = event.touches[0].clientY;
  const rows = (state.shellTouchScrollY - nextY) / shellTermLineHeight();
  state.shellTouchScrollY = nextY;
  const pendingRows = state.shellTouchScrollRemainder + rows;
  const lines = pendingRows < 0 ? Math.ceil(pendingRows) : Math.floor(pendingRows);
  state.shellTouchScrollRemainder = pendingRows - lines;
  if (scrollShellTermByLines(lines)) {
    event.preventDefault();
    event.stopPropagation();
  }
}

function endShellTouchScroll() {
  state.shellTouchScrollY = null;
  state.shellTouchScrollRemainder = 0;
}

function syncFloatingNavigationPosition() {
  const viewport = window.visualViewport;
  const visualHeight = viewport?.height || window.innerHeight || document.documentElement.clientHeight || 0;
  if (visualHeight > 0) {
    document.documentElement.style.setProperty("--app-viewport-height", `${Math.round(visualHeight)}px`);
  }
  const layoutHeight = Math.max(document.documentElement.clientHeight || 0, window.innerHeight || 0);
  const rawOffset = viewport ? layoutHeight - viewport.height - viewport.offsetTop : 0;
  const offset = Number.isFinite(rawOffset) ? Math.max(0, rawOffset) : 0;
  document.documentElement.style.setProperty("--bottom-nav-keyboard-offset", `${Math.round(offset)}px`);
  scheduleShellFit(true, 320);
}

function scheduleFloatingNavigationPositionSync() {
  if (state.floatingNavSyncRaf) {
    window.cancelAnimationFrame(state.floatingNavSyncRaf);
  }
  state.floatingNavSyncRaf = window.requestAnimationFrame(() => {
    state.floatingNavSyncRaf = null;
    syncFloatingNavigationPosition();
  });
  window.clearTimeout(state.floatingNavSettleTimer);
  state.floatingNavSettleTimer = window.setTimeout(syncFloatingNavigationPosition, 360);
}

function bindFloatingNavigationPosition() {
  syncFloatingNavigationPosition();
  window.visualViewport?.addEventListener("resize", scheduleFloatingNavigationPositionSync);
  window.visualViewport?.addEventListener("scroll", scheduleFloatingNavigationPositionSync);
  window.addEventListener("resize", scheduleFloatingNavigationPositionSync);
  window.addEventListener("scroll", scheduleFloatingNavigationPositionSync, { passive: true });
}

function activeView() {
  return document.querySelector(".view.active")?.id?.replace(/-view$/, "")
    || document.querySelector("button[data-view].active")?.dataset.view
    || "chat";
}

function syncNavigationState(view = activeView()) {
  const primaryMobileViews = new Set(["chat", "files", "shell", "git", "board", "database"]);
  qs("#bottom-more")?.classList.toggle("active", !primaryMobileViews.has(view));
  qs(".more-nav")?.classList.toggle("active", !["chat", "files", "shell", "git", "board", "database"].includes(view));
}

async function switchView(view) {
  const panel = qs(`#${view}-view`);
  if (!panel) return false;
  if (activeView() === "files" && view !== "files" && !confirmDiscardDirtyFile()) {
    return false;
  }
  if (view !== "chat") closeChatEditFromHerePicker();
  if (view !== "chat" && state.boardWsSessionId) setBoardChatWsSubscription("");
  document.querySelectorAll("button[data-view]").forEach((item) => item.classList.remove("active"));
  document.querySelectorAll(".view").forEach((item) => {
    item.classList.remove("active");
    item.setAttribute("aria-hidden", "true");
  });
  document.querySelectorAll(`button[data-view="${view}"]`).forEach((item) => item.classList.add("active"));
  syncNavigationState(view);
  document.body.dataset.activeView = view;
  panel.classList.add("active");
  panel.setAttribute("aria-hidden", "false");
  updateMainHeader(view);
  const title = qs("#view-title");
  const subtitle = qs("#view-subtitle");
  if (title) title.textContent = VIEW_NAMES[view] || view;
  if (subtitle) subtitle.textContent = VIEW_SUBTITLES[view] || "";
  closeMoreSheet();
  closeSidebar();
  window.localStorage.setItem("iowb.lastView", view);
  if (view === "shell") {
    await Promise.all([ensureXterm(), loadView(view)]);
  } else {
    await loadView(view);
  }
  if (!panel.isConnected) return false;
  updateMainHeader(view);
  if (view === "shell") {
    await ensureShellRunningForActiveProject();
    scheduleShellFit(true);
  }
  if (view === "chat") {
    if (isSelectedBoardChatSession()) {
      setBoardChatWsSubscription(state.chatSessionId);
    }
    updateChatEmptyState();
  }
  return true;
}

async function loadView(view) {
  if (view === "board") await loadBoard();
  if (view === "files") await loadFiles();
  if (view === "git") await loadGitStatus();
  if (view === "shell") await loadProcesses();
  if (view === "database") await loadDbConnections();
  if (view === "settings") {
    await loadSettings();
    await Promise.all([
      loadMetrics().catch(showError),
      loadToolRuns().catch(showError),
    ]);
  }
  if (view === "chat" && !state.chatSessionId && !state.chatSuppressAutoOpenOnce) {
    // No session is open yet — auto-open the most recent session so the chat
    // tab is never blank when the user lands on it.
    await autoOpenLatestChatSession().catch(showError);
  }
  if (view === "chat") {
    state.chatSuppressAutoOpenOnce = false;
  }
}

async function refreshCurrentView() {
  const view = activeView();
  if (view === "board") await loadBoard();
  else if (view === "files") await loadFiles();
  else if (view === "git") await loadGitStatus();
  else if (view === "database") await loadDbConnections();
  else if (view === "settings") {
    await loadSettings();
    await loadMetrics();
    await loadToolRuns();
  }
  else if (view === "shell") await loadProcesses();
}

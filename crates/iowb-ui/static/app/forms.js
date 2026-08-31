function bindForms() {
  window.addEventListener("beforeunload", (event) => {
    if (!state.currentFileDirty) return;
    event.preventDefault();
    event.returnValue = "";
  });
  window.addEventListener("resize", () => {
    scheduleShellFit(true, 320);
  });
  const chatBody = qs(".chat-body");
  chatBody?.addEventListener("touchstart", handleChatTouchStart, { passive: true });
  chatBody?.addEventListener("touchmove", handleChatTouchMove, { passive: true });
  chatBody?.addEventListener("touchend", handleChatTouchEnd, { passive: true });
  chatBody?.addEventListener("touchcancel", resetChatSwipe, { passive: true });
  const chatOutput = qs("#chat-output");
  chatOutput?.addEventListener("scroll", () => {
    maybeLoadOlderChatMessages();
    updateChatJumpToLatestButton();
  }, { passive: true });
  qs("#chat-jump-latest")?.addEventListener("click", jumpToLatestChatMessage);
  qs("#pref-compact").addEventListener("change", (event) => {
    state.preferences.compact = event.currentTarget.checked;
    savePreferences();
    applyPreferences();
  });
  qs("#pref-wrap").addEventListener("change", (event) => {
    state.preferences.wrapOutput = event.currentTarget.checked;
    savePreferences();
    applyPreferences();
  });
  document.querySelectorAll("[data-settings-tab]").forEach((button) => {
    button.addEventListener("click", () => setSettingsTab(button.dataset.settingsTab));
  });
  qs("#chat-provider-setting")?.addEventListener("change", (event) => {
    setChatProvider(event.currentTarget.value);
  });
  qs("#active-project")?.addEventListener("change", (event) => {
    setActiveProject(event.currentTarget.value);
    loadView(activeView()).catch(showError);
  });
  qs("#sidebar-search")?.addEventListener("input", (event) => {
    state.sidebarSearch = event.currentTarget.value;
    renderSidebarProjects();
    renderSidebarSessions();
  });
  qs("#sidebar-new-project")?.addEventListener("click", openAddProjectFolderBrowser);
  qs("#sidebar-manage-projects")?.addEventListener("click", openAddProjectFolderBrowser);
  qs("#chat-empty-add-project")?.addEventListener("click", openAddProjectFolderBrowser);
  qs("#sidebar-refresh")?.addEventListener("click", (event) => {
    withButtonLoading(event.currentTarget, async () => {
      await Promise.all([
        loadSharedPinnedChatSessions().catch(showError),
        loadProjects().catch(showError),
        loadView(activeView()).catch(showError),
      ]);
      showToast("Workspace refreshed", "ok");
    }).catch(showError);
  });
  qs("#bottom-sidebar")?.addEventListener("click", toggleSidebar);
  qs("#main-sidebar-toggle")?.addEventListener("click", toggleSidebar);
  qs("#mobile-sidebar-fab")?.addEventListener("click", toggleSidebar);
  document.addEventListener("pointerdown", (event) => {
    if (!document.body.classList.contains("sidebar-open")) return;
    if (!window.matchMedia("(max-width: 760px)").matches) return;
    if (event.target.closest(".sidebar") || event.target.closest("#bottom-sidebar") || event.target.closest("#mobile-sidebar-fab")) return;
    closeSidebar();
  }, true);
  document.addEventListener("visibilitychange", () => {
    if (
      document.visibilityState === "visible"
      && !document.body.classList.contains("auth-active")
    ) {
      loadSharedPinnedChatSessions().catch((error) => {
        console.debug("shared pinned chat refresh skipped", error);
      });
    }
  });
  document.addEventListener("click", (event) => {
    if (state.fileContextMenu && !event.target.closest("#file-context-menu") && !event.target.closest("[data-file-menu]")) {
      closeFileContextMenu();
    }
    if (state.sessionContextMenu && !event.target.closest("#session-context-menu") && !event.target.closest("[data-sidebar-session-card]")) {
      closeSessionContextMenu();
    }
    if (state.openProjectMenuPath && !event.target.closest(".project-menu-wrap")) {
      state.openProjectMenuPath = "";
      renderSidebarProjects();
    }
    if (!document.body.classList.contains("sidebar-open")) return;
    if (!window.matchMedia("(max-width: 760px)").matches) return;
    if (event.target.closest(".sidebar") || event.target.closest("#bottom-sidebar")) return;
    closeSidebar();
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeFileContextMenu();
  });
  qs("#bottom-more")?.addEventListener("click", openMoreSheet);
  qs("#more-close")?.addEventListener("click", closeMoreSheet);
  qs("#more-sheet")?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget) closeMoreSheet();
  });
  qs("#auth-password-toggle")?.addEventListener("click", () => {
    const input = qs("#auth-password");
    const button = qs("#auth-password-toggle");
    const showing = input.type === "text";
    input.type = showing ? "password" : "text";
    button.textContent = showing ? "Show" : "Hide";
    button.title = showing ? "Show password" : "Hide password";
  });
  qs("#refresh-projects")?.addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadProjects).catch(showError));
  qs("#refresh-files").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadFiles).catch(showError));
  qs("#refresh-sessions")?.addEventListener("click", renderSessions);
  qs("#refresh-git").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadGitStatus).catch(showError));
  qs("#git-repository")?.addEventListener("change", (event) => {
    state.gitSelectedRepositoryId = event.currentTarget.value;
    state.gitStatus = null;
    loadGitStatus().catch(showError);
  });
  qs("#refresh-db").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadDbConnections).catch(showError));
  qs("#refresh-tool-runs").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadToolRuns).catch(showError));
  qs("#refresh-metrics").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadMetrics).catch(showError));
  qs("#refresh-settings").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadSettings).catch(showError));
  qs("#board-refresh")?.addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadBoard).catch(showError));
  qs("#board-resume")?.addEventListener("click", (event) => withButtonLoading(event.currentTarget, () => boardAction("resume")).catch(showError));
  qs("#board-pause")?.addEventListener("click", (event) => withButtonLoading(event.currentTarget, () => boardAction("pause")).catch(showError));
  qs("#board-abort")?.addEventListener("click", (event) => withButtonLoading(event.currentTarget, () => boardAction("abort")).catch(showError));
  qs("#board-start-form")?.addEventListener("submit", (event) => withButtonLoading(event.submitter, () => createBoard(event)).catch(showError));
  qs("#board-task-form")?.addEventListener("submit", (event) => withButtonLoading(event.submitter, () => addBoardTask(event)).catch(showError));
  qs("#board-details")?.addEventListener("click", (event) => {
    const transcriptButton = event.target.closest("[data-board-view-transcript]");
    if (transcriptButton && transcriptButton.dataset.boardViewTranscript === "breakdown") {
      openBoardTranscriptModal(
        "Backlog breakdown transcript",
        state.board?.backlogBreakdown?.transcript || [],
      );
    }
  });
  qs("#board-columns")?.addEventListener("click", (event) => {
    const detailsButton = event.target.closest("[data-board-open-details]");
    if (detailsButton) {
      openBoardTaskDetails(detailsButton.dataset.boardOpenDetails);
      return;
    }
    const discussionButton = event.target.closest("[data-board-discuss-task]");
    if (discussionButton) {
      openBoardDiscussionModal(discussionButton.dataset.boardDiscussTask);
      return;
    }
    const breakdownButton = event.target.closest("[data-board-breakdown-task]");
    if (breakdownButton) {
      withButtonLoading(breakdownButton, () => breakdownBoardTask(breakdownButton.dataset.boardBreakdownTask)).catch(showError);
      return;
    }
    const transcriptButton = event.target.closest("[data-board-view-transcript]");
    if (transcriptButton) {
      openBoardTaskTranscript(transcriptButton.dataset.boardViewTranscript).catch(showError);
      return;
    }
    const chatSessionButton = event.target.closest("[data-board-open-chat-session]");
    if (chatSessionButton) {
      openBoardTaskChatSession(chatSessionButton.dataset.boardOpenChatSession).catch(showError);
      return;
    }
    const retryButton = event.target.closest("[data-board-retry-task]");
    if (retryButton) {
      if (!window.confirm("Retry this task as a transient/environment failure?")) return;
      withButtonLoading(
        retryButton,
        () => retryBoardTask(retryButton.dataset.boardRetryTask),
      ).catch(showError);
      return;
    }
    const moveButton = event.target.closest("[data-board-task-status]");
    if (moveButton) {
      withButtonLoading(moveButton, () => moveBoardTask(moveButton.dataset.boardTaskId, moveButton.dataset.boardTaskStatus)).catch(showError);
      return;
    }
  });
  document.querySelectorAll("[data-chat-provider-option]").forEach((button) => {
    button.addEventListener("click", () => chooseNewChatProvider(button.dataset.chatProviderOption));
  });
  qs("#folder-browser-close").addEventListener("click", closeFolderBrowser);
  qs("#folder-browser").addEventListener("click", (event) => {
    if (event.target === event.currentTarget) closeFolderBrowser();
  });
  qs("#folder-browser-home").addEventListener("click", () => loadFolderBrowser("~").catch(showError));
  qs("#folder-browser-up").addEventListener("click", () => {
    const parentPath = parentFilesystemPath(state.folderBrowser.path);
    if (parentPath) loadFolderBrowser(parentPath).catch(showError);
  });
  qs("#folder-browser-hidden").addEventListener("click", () => {
    state.folderBrowser.showHidden = !state.folderBrowser.showHidden;
    renderFolderBrowser();
  });
  qs("#folder-browser-filter").addEventListener("input", (event) => {
    state.folderBrowser.filter = event.currentTarget.value;
    renderFolderBrowser();
  });
  qs("#folder-browser-use").addEventListener("click", () => selectFolderBrowserPath().catch(showError));
  qs("#files-path").addEventListener("change", () => {
    if (confirmDiscardDirtyFile()) {
      resetVirtualList("files");
      loadFiles().catch(showError);
    }
  });
  qs("#files-filter").addEventListener("input", () => {
    resetVirtualList("files");
    renderFileEntries();
  });
  qs("#files-clear-filter")?.addEventListener("click", () => {
    qs("#files-filter").value = "";
    resetVirtualList("files");
    renderFileEntries();
    qs("#files-filter").focus();
  });
  qs("#files-select-all")?.addEventListener("click", toggleAllVisibleFilesSelection);
  qs("#files-clear-selection")?.addEventListener("click", () => {
    state.fileSelectedPaths.clear();
    renderFileEntries();
  });
  qs("#files-collapse-all")?.addEventListener("click", () => {
    state.fileExpandedPaths.clear();
    renderFileEntries();
  });
  document.querySelectorAll("[data-file-view-mode]").forEach((button) => {
    button.addEventListener("click", () => setFileTreeViewMode(button.dataset.fileViewMode));
  });
  qs("#git-filter").addEventListener("input", () => {
    resetVirtualList("gitFiles");
    renderGitFiles();
  });
  document.querySelectorAll("[data-git-view-tab]").forEach((button) => {
    button.addEventListener("click", () => setGitActiveView(button.dataset.gitViewTab));
  });
  qs("#git-select-all").addEventListener("click", () => setGitFileSelection(true));
  qs("#git-select-none").addEventListener("click", () => setGitFileSelection(false));
  qs("#sessions-filter")?.addEventListener("input", () => {
    resetVirtualList("sessions");
    renderSessions();
  });
  qs("#db-filter").addEventListener("input", renderDbConnections);
  qs("#tool-filter").addEventListener("input", () => state.lastToolRuns && renderToolRuns("#tools-output", state.lastToolRuns));
  qs("#settings-filter").addEventListener("input", renderSettingsRows);
  qs("#file-editor-content").addEventListener("input", () => {
    state.currentFileDirty = true;
    resetEditorSearch();
    updateEditorChrome();
  });
  qs("#file-editor-content").addEventListener("click", updateEditorChrome);
  qs("#file-editor-content").addEventListener("keyup", updateEditorChrome);
  qs("#file-editor-content").addEventListener("keydown", handleEditorKeydown);
  qs("#file-editor-content").addEventListener("scroll", () => {
    qs("#file-editor-lines").scrollTop = qs("#file-editor-content").scrollTop;
  });
  qs("#editor-search").addEventListener("input", () => {
    resetEditorSearch();
    refreshEditorSearchMatches();
  });
  qs("#editor-search").addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    findEditorMatch(event.shiftKey ? -1 : 1);
  });
  qs("#editor-find-prev").addEventListener("click", () => findEditorMatch(-1));
  qs("#editor-find-next").addEventListener("click", () => findEditorMatch(1));
  qs("#editor-replace-one").addEventListener("click", replaceEditorMatch);
  qs("#editor-replace-all").addEventListener("click", replaceAllEditorMatches);
  qs("#editor-go-line").addEventListener("click", goToEditorLine);
  qs("#editor-goto-line").addEventListener("keydown", (event) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    goToEditorLine();
  });
  qs("#file-editor-form").addEventListener("submit", (event) => saveFile(event).catch(showError));
  qs("#editor-close")?.addEventListener("click", () => closeFileEditor());
  qs("#create-file").addEventListener("click", () => startCreateFileTreePath(false, qs("#files-path")?.value || "."));
  qs("#create-directory").addEventListener("click", () => startCreateFileTreePath(true, qs("#files-path")?.value || "."));
  qs("#editor-create-file")?.addEventListener("click", () => startCreateFileTreePath(false, qs("#files-path")?.value || "."));
  qs("#editor-create-directory")?.addEventListener("click", () => startCreateFileTreePath(true, qs("#files-path")?.value || "."));
  qs("#delete-file").addEventListener("click", () => deletePath().catch(showError));
  qs("#download-file").addEventListener("click", downloadCurrentFile);
  qs("#reload-file").addEventListener("click", (event) => withButtonLoading(event.currentTarget, reloadCurrentFile).catch(showError));
  qs("#copy-file-path").addEventListener("click", (event) => copyCurrentFilePath(event).catch(showError));
  qs("#rename-file").addEventListener("click", () => renamePath().catch(showError));
  qs("#upload-files").addEventListener("click", () => {
    state.fileUploadTargetPath = qs("#files-path")?.value.trim() || ".";
    qs("#file-upload-input")?.click();
  });
  qs("#file-upload-input").addEventListener("change", () => uploadProjectFiles().catch(showError));
  qs("#folder-upload-input")?.addEventListener("change", () => uploadProjectFolder().catch(showError));
  qs("#session-search-form")?.addEventListener("submit", (event) => searchSessions(event).catch(showError));
  qs("#load-project-sessions")?.addEventListener("click", () => loadProjectSessions().catch(showError));
  qs("#load-session-messages")?.addEventListener("click", () => loadSessionMessages().catch(showError));
  qs("#load-session-model")?.addEventListener("click", () => loadSessionModel().catch(showError));
  qs("#update-session-model")?.addEventListener("click", () => updateSessionModel().catch(showError));
  qs("#load-session-token-usage")?.addEventListener("click", () => loadSessionTokenUsage().catch(showError));
  qs("#rename-session-action")?.addEventListener("click", () => renameSelectedSession().catch(showError));
  qs("#git-init").addEventListener("click", () => initializeGitRepository().catch(showError));
  qs("#git-initial-commit").addEventListener("click", () => createGitInitialCommit().catch(showError));
  qs("#git-generate-message").addEventListener("click", () => generateGitMessage().catch(showError));
  qs("#git-commit").addEventListener("click", () => commitGitSelection().catch(showError));
  qs("#git-diff").addEventListener("click", () => gitDiffSelected().catch(showError));
  qs("#git-file-diff").addEventListener("click", () => gitFileDiffSelected().catch(showError));
  qs("#git-conflicts").addEventListener("click", () => loadGitConflicts().catch(showError));
  qs("#git-branches").addEventListener("click", () => setGitActiveView("branches"));
  qs("#git-commits").addEventListener("click", () => setGitActiveView("history"));
  qs("#git-remote-status").addEventListener("click", () => gitRead("/api/git/remote-status", renderGitRemoteStatus).catch(showError));
  qs("#git-fetch").addEventListener("click", (event) => withButtonLoading(event.currentTarget, () => gitOperation("/api/git/fetch")).catch(showError));
  qs("#git-pull").addEventListener("click", () => gitOperation("/api/git/pull").catch(showError));
  qs("#git-push").addEventListener("click", () => gitOperation("/api/git/push").catch(showError));
  qs("#git-stage").addEventListener("click", () => gitSelectedFileOperation("/api/git/stage").catch(showError));
  qs("#git-unstage").addEventListener("click", () => gitSelectedFileOperation("/api/git/unstage").catch(showError));
  qs("#git-checkout").addEventListener("click", () => gitBranchOperation("/api/git/checkout").catch(showError));
  qs("#git-create-branch").addEventListener("click", () => gitBranchOperation("/api/git/create-branch").catch(showError));
  qs("#git-delete-branch").addEventListener("click", () => gitBranchOperation("/api/git/delete-branch").catch(showError));
  qs("#git-set-remote").addEventListener("click", () => setGitRemote().catch(showError));
  qs("#git-publish").addEventListener("click", () => publishCurrentBranch().catch(showError));
  qs("#git-revert-local").addEventListener("click", () => gitOperation("/api/git/revert-local-commit").catch(showError));
  qs("#git-discard").addEventListener("click", () => gitSelectedFileOperation("/api/git/discard").catch(showError));
  qs("#git-delete-untracked").addEventListener("click", () => gitSelectedFileOperation("/api/git/delete-untracked").catch(showError));
  qs("#db-create-form").addEventListener("submit", (event) => createDbConnection(event).catch(showError));
  qs("#db-new-connection").addEventListener("click", resetDbConnectionForm);
  qs("#db-test-unsaved").addEventListener("click", () => testDbConnectionForm().catch(showError));
  qs("#db-delete-selected").addEventListener("click", () => deleteDbConnection().catch(showError));
  qs("#db-target-connection").addEventListener("change", (event) => {
    state.selectedDbTargetConnection = Number(event.currentTarget.value) || null;
  });
  qs("#db-query-form").addEventListener("submit", (event) => runDbQuery(event).catch(showError));
  qs("#db-explorer").addEventListener("click", () => loadDbExplorer().catch(showError));
  qs("#db-describe").addEventListener("click", () => loadDbObjectDetails().catch(showError));
  qs("#db-diagram").addEventListener("click", () => loadDbRelationshipDiagram().catch(showError));
  qs("#db-select-sql").addEventListener("click", () => setDbSql("select"));
  qs("#db-count-sql").addEventListener("click", () => setDbSql("count"));
  qs("#db-prev-page").addEventListener("click", previousDbPage);
  qs("#db-table-data").addEventListener("click", () => loadDbTableData().catch(showError));
  qs("#db-export").addEventListener("click", () => dbFileJob("/api/database/export").catch(showError));
  qs("#db-import").addEventListener("click", () => dbFileJob("/api/database/import").catch(showError));
  qs("#db-transfer").addEventListener("click", () => transferDbTable().catch(showError));
  qs("#db-jobs").addEventListener("click", () => loadDbJobs().catch(showError));
  qs("#tool-run-form").addEventListener("submit", (event) => runTool(event).catch(showError));
  qs("#mcp-server-form").addEventListener("submit", (event) => startMcpServer(event).catch(showError));
  qs("#refresh-mcp-servers").addEventListener("click", (event) => withButtonLoading(event.currentTarget, loadMcpServers).catch(showError));
  qs("#stop-mcp-server").addEventListener("click", () => stopMcpServer().catch(showError));
  qs("#audio-transcribe-form").addEventListener("submit", (event) => transcribeAudio(event).catch(showError));
  qs("#settings-action-form").addEventListener("submit", (event) => applySettingsAction(event).catch(showError));
  qs("#load-cli-status").addEventListener("click", () => loadSettingsView("/api/cli").catch(showError));
  qs("#load-user-settings").addEventListener("click", () => loadSettingsView("/api/user").catch(showError));
  qs("#load-api-keys").addEventListener("click", () => loadSettingsView("/api/settings/api-keys").catch(showError));
  qs("#load-credentials").addEventListener("click", () => loadSettingsView("/api/settings/credentials").catch(showError));
  qs("#load-notifications").addEventListener("click", () => loadSettingsView("/api/settings/notification-preferences").catch(showError));
  qs("#notification-save").addEventListener("click", () => saveNotificationPreferences().catch(showError));
  qs("#notification-permission").addEventListener("click", () => requestNotificationPermission().catch(showError));
  qs("#notification-preview").addEventListener("click", () => previewBrowserNotification().catch(showError));
  qs("#notification-test-push").addEventListener("click", () => testPushNotificationCommand().catch(showError));
  qs("#io-gateway-form").addEventListener("submit", (event) => withButtonLoading("#save-direct-ai", () => saveIoGatewayConfig(event)).catch(showError));
  document.querySelectorAll("[data-io-gateway-secret]").forEach((button) => {
    button.addEventListener("click", () => toggleIoGatewaySecret(button.dataset.ioGatewaySecret).catch(showError));
  });
  qs("#load-direct-ai").addEventListener("click", (event) => withButtonLoading(event.currentTarget, () => loadIoGatewayConfig({ force: true })).catch(showError));
  qs("#load-direct-ai-models").addEventListener("click", () => loadSettingsView("/api/settings/direct-ai/models").catch(showError));
  qs("#load-git-config").addEventListener("click", () => loadSettingsView("/api/user/git-config").catch(showError));
  qs("#prompt-config-toggle")?.addEventListener("click", () => togglePromptConfigPanel());
  qs("#chat-upload-images").addEventListener("click", () => qs("#chat-image-input").click());
  qs("#chat-image-input").addEventListener("change", () => uploadChatImages().catch(showError));
  qs("#clear-chat-images")?.addEventListener("click", clearChatImages);
  qs("#prompt-history-prev")?.addEventListener("click", () => navigateChatPromptHistory(-1));
  qs("#prompt-history-next")?.addEventListener("click", () => navigateChatPromptHistory(1));
  qs("#chat-thinking-toggle")?.addEventListener("click", () => {
    state.preferences.chatThinking = !chatThinkingValue();
    savePreferences();
    const sid = state.chatSessionId || state.pendingChatSessionId || state.preferences.lastChatSessionId;
    if (sid) saveSessionOverrides(sid, { thinking: state.preferences.chatThinking });
    updatePendingChatProvider(chatCliValue());
    updateChatComposerState();
  });
  qs("#chat-fast-toggle")?.addEventListener("change", (event) => {
    setChatFastRequested(event.currentTarget.checked);
  });
	  qs("#reload-chat-session")?.addEventListener("click", (event) => {
	    const sessionId = state.chatSessionId || state.preferences.lastChatSessionId || "";
	    if (!sessionId) return;
	    withButtonLoading(event.currentTarget, () => loadChatHistoryForSession(sessionId)).catch(showError);
	  });
	  qs("#compact-chat-session")?.addEventListener("click", (event) => {
	    compactChatSessionContext(state.chatSessionId, event.currentTarget).catch(showError);
	  });
	  qs("#chat-session-config")?.addEventListener("click", showChatSessionConfigModal);
  qs("#chat-prompt").addEventListener("input", (event) => {
    const prompt = event.currentTarget;
    noteChatPromptUserEdit(prompt.value || "");
    if (!stagedChatEdit()) {
      const sessionId = currentChatDraftSessionId();
      writeLocalChatPromptDraft(prompt.value || "", sessionId);
      if (sessionId) state.chatPromptDraftSessionId = sessionId;
    }
    autosizeChatPrompt();
    scheduleChatPromptDraftSave();
    ensureChatPromptHistoryScope();
    state.chatPromptHistoryIndex = (state.chatPromptHistory || []).length;
  });
  qs("#chat-prompt").addEventListener("focus", autosizeChatPrompt);
  qs("#chat-prompt").addEventListener("keydown", (event) => {
    if (handleChatEditFromHereKeydown(event)) return;
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      qs("#chat-form")?.requestSubmit();
      return;
    }
    if ((event.key === "ArrowUp" || event.key === "ArrowDown") && !event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey) {
      const input = event.currentTarget;
      const atStart = (input.selectionStart ?? 0) === 0 && (input.selectionEnd ?? 0) === 0;
      const atEnd = (input.selectionStart ?? 0) === input.value.length && (input.selectionEnd ?? 0) === input.value.length;
      if ((event.key === "ArrowUp" && atStart) || (event.key === "ArrowDown" && atEnd)) {
        event.preventDefault();
        navigateChatPromptHistory(event.key === "ArrowUp" ? -1 : 1);
      }
    }
  });
  qs("#clear-chat").addEventListener("click", () => {
    const prompt = qs("#chat-prompt");
    clearChatPromptInput();
    const sessionId = currentChatDraftSessionId();
    writeLocalChatPromptDraft("", sessionId);
    if (sessionId) clearRemoteChatPromptDraft(sessionId);
    updateChatComposerState();
    prompt.focus();
  });
  qs("#clear-shell").addEventListener("click", () => {
    state.shellBuffer = "";
    renderShell();
  });
  qs("#restart-shell").addEventListener("click", (event) => withButtonLoading(event.currentTarget, () => startShell({ force: true })).catch(showError));
  const shellOutput = qs("#shell-output");
  shellOutput.addEventListener("keydown", handleShellOutputKey);
  shellOutput.addEventListener("mousedown", focusShellTerm);
  shellOutput.addEventListener("click", focusShellTerm);
  shellOutput.addEventListener("wheel", handleShellWheel, { passive: false });
  shellOutput.addEventListener("touchstart", beginShellTouchScroll, { passive: true });
  shellOutput.addEventListener("touchmove", handleShellTouchScroll, { passive: false });
  shellOutput.addEventListener("touchend", endShellTouchScroll, { passive: true });
  shellOutput.addEventListener("touchcancel", endShellTouchScroll, { passive: true });
  qs("#shell-cols").addEventListener("change", () => {
    saveTerminalSizePreference();
    applyTerminalSizePreference(true).catch(showError);
  });
  qs("#shell-rows").addEventListener("change", () => {
    saveTerminalSizePreference();
    applyTerminalSizePreference(true).catch(showError);
  });
  bindShellShortcuts();

  qs("#auth-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const mode = qs("#auth-form").dataset.mode || "login";
    const username = mode === "otp" ? "otp" : qs("#auth-username").value.trim();
    const password = qs("#auth-password").value;
    const endpoint = mode === "setup" ? "/api/auth/register" : "/api/auth/login";

    try {
      const body = await api(endpoint, {
        method: "POST",
        body: JSON.stringify({ username, password }),
      });
      state.token = body.token || "";
      window.sessionStorage.setItem(TOKEN_STORAGE_KEY, state.token);
      window.localStorage.removeItem(TOKEN_STORAGE_KEY);
      qs("#auth-password").value = "";
      await bootstrapProtected();
      showToast(mode === "setup" ? "Account created" : "Signed in", "ok");
    } catch (error) {
      qs("#auth-message").textContent = error.message;
    }
  });

  qs("#auth-logout")?.addEventListener("click", async () => {
    try {
      await api("/api/auth/logout", { method: "POST" });
    } catch {
      // Token removal is enough for local logout.
    }
    state.token = "";
    window.sessionStorage.removeItem(TOKEN_STORAGE_KEY);
    window.localStorage.removeItem(TOKEN_STORAGE_KEY);
    showAuthPanel(authPanelMode());
    if (state.ws) state.ws.close();
    showToast("Signed out", "ok");
  });

  qs("#chat-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const recovery = state.chatRecoveryBySession[state.chatSessionId || ""];
    if (chatRecoveryBlocksNormalSend(recovery)) {
      renderChatRecoveryCard(state.chatSessionId);
      showToast("Compact & retry this chat before sending another message.", "danger");
      return;
    }
    if (selectedChatIsStopping()) return;
    const selectedRun = selectedRunningChatSession();
    if (selectedRun) {
      requestAbortSelectedChatSession(selectedRun);
      return;
    }
    const projectPath = activeProjectPath();
    if (!projectPath) {
      showError(new Error("Select a project before sending chat."));
      return;
    }
    const cli = chatCliValue();
    if (!cli) {
      showError(new Error("Pick a CLI (Codex, Claude, or Gemini) before sending a prompt."));
      return;
    }
    const draftContent = qs("#chat-prompt").value.trim();
    const prompt = chatPromptWithImages(draftContent);
    if (!prompt) return;
    if (!state.ws || state.ws.readyState !== WebSocket.OPEN) {
      connectWs();
      showError(new Error("Chat connection is not ready. Reconnecting now."));
      return;
    }
    const stagedEdit = stagedChatEdit();
    let replacement = null;
    if (stagedEdit) {
      if (state.chatEditFromHere.submitting) return;
      state.chatEditFromHere.submitting = true;
      updateChatComposerState();
      try {
        replacement = await api(
          `/api/sessions/${encodeURIComponent(stagedEdit.sourceSessionId)}/fork`,
          {
            method: "POST",
            body: JSON.stringify({
              beforeMessageId: stagedEdit.beforeMessageId,
              requestId: stagedEdit.requestId,
              replace: true,
              draftContent,
            }),
          },
        );
      } catch (error) {
        state.chatEditFromHere.submitting = false;
        updateChatComposerState();
        showError(error);
        qs("#chat-prompt")?.focus();
        return;
      }
      const destination = replacement?.session;
      if (!destination?.id) {
        state.chatEditFromHere.submitting = false;
        updateChatComposerState();
        showError(new Error("The server did not return the replacement chat session."));
        return;
      }
      const boardReplacement = isBoardChatSession(destination)
        || state.boardChatSessionIds.has(stagedEdit.sourceSessionId);
      if (boardReplacement) {
        state.boardChatSessionIds.add(destination.id);
        hideBoardChatSessionsFromLists();
        setBoardChatWsSubscription(destination.id);
      } else {
        setBoardChatWsSubscription("");
        state.sessions = (state.sessions || [])
          .filter((session) => session?.id !== destination.id)
          .concat(destination);
        mergeSessionIntoProjects(destination);
        transferPinnedChatSession(stagedEdit.sourceSessionId, destination);
      }
      if (replacement.sourceHidden === true) {
        hideChatSessionFromLists(stagedEdit.sourceSessionId);
      }
      state.chatSessionId = destination.id;
      state.pendingChatSessionId = "";
      state.chatEditFromHere.staged = null;
      state.chatEditFromHere.submitting = false;
      chatHistoryWindow = {
        sessionId: destination.id,
        offset: 0,
        totalCount: stagedEdit.prefixMessages.length,
        messages: stagedEdit.prefixMessages.map((message, index) => ({
          ...message,
          id: `local-fork-prefix-${index}`,
        })),
      };
      resetChatOutputDom();
      replayChatMessages(chatHistoryWindow.messages);
      renderProjects();
      renderSidebarProjects();
      renderSidebarSessions();
    }
    clearChatProcessing();
    chatStream = { role: null, node: null, text: null, buffer: "" };
    state.chatBuffer = "";
    // Capture per-session overrides so refresh reopens the same setup.
    const model = chatModelValue();
    const effort = chatEffortValue();
    const mode = chatModeValue();
    const thinking = chatThinkingValue();
    const fast = chatFastValue();
    state.preferences.chatCli = cli;
    state.preferences.chatModel = model;
    state.preferences.chatEffort = effort;
    state.preferences.chatMode = mode;
    state.preferences.chatThinking = thinking;
    state.preferences.chatFast = fast;
    savePreferences();

    const startedAt = new Date().toISOString();
    let sessionId = replacement?.session?.id || chatSessionIdForSubmit();
    if (!sessionId) {
      sessionId = `local-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
      state.chatSessionId = sessionId;
      state.pendingChatSessionId = sessionId;
    }
    ensureChatPromptHistoryScope(sessionId);
    rememberChatPrompt(qs("#chat-prompt").value.trim());
    if (sessionId) {
      saveSessionOverrides(sessionId, { cli, model, effort, mode, thinking, fast, sentAt: startedAt });
      rememberSidebarSessionStatus({ sessionId, provider: cli, status: "starting" });
    }
    const message = {
      type: "start_session",
      provider: cli,
      projectPath,
      prompt,
      model,
      effort,
      mode,
      thinking: thinking || undefined,
      fast,
    };
    if (sessionId) message.sessionId = sessionId;
    try {
      state.ws.send(JSON.stringify(message));
    } catch (error) {
      if (replacement?.session?.id) {
        setChatPromptValue(draftContent);
        noteChatPromptUserEdit(draftContent);
        writeLocalChatPromptDraft(draftContent, replacement.session.id);
        rememberSidebarSessionStatus({ sessionId: replacement.session.id, provider: cli, status: "completed" });
      }
      showError(error);
      return;
    }
    renderChatProviderPicker();
    if (sessionId) {
      state.chatSessionId = sessionId;
      if (!isSelectedBoardChatSession(sessionId)) {
        state.preferences.lastChatSessionId = sessionId;
        savePreferences();
      }
      writeLocalChatPromptDraft("", sessionId);
      if (currentChatDraftSessionId() === sessionId) clearRemoteChatPromptDraft(sessionId);
    }
    state.pendingChatSessionId = "";
    clearChatPromptInput();
    clearChatImages();
    updateChatComposerState();

    // Show the user prompt in the chat with right-aligned styling, plus the
    // current overrides footer so the data below the prompt is visible.
    appendUserPromptToChat(prompt, {
      cli, model, effort, mode, thinking, fast, sentAt: startedAt,
    });
    const currentMessages = chatHistoryWindow.sessionId === sessionId ? chatHistoryWindow.messages : [];
    const nextMessages = currentMessages.concat({
      id: `local-user-${Date.now()}`,
      role: "user",
      content: prompt,
      timestamp: startedAt,
      metadata: { cli, model, effort, mode, thinking, fast, sentAt: startedAt },
    });
    chatHistoryWindow = {
      sessionId,
      offset: chatHistoryWindow.sessionId === sessionId ? chatHistoryWindow.offset : 0,
      totalCount: Math.max(
        chatHistoryWindow.sessionId === sessionId ? chatHistoryWindow.totalCount + 1 : nextMessages.length,
        nextMessages.length,
      ),
      messages: nextMessages,
    };
    if (!isSelectedBoardChatSession(sessionId)) {
      persistActiveChatSelection(sessionId, projectPath);
    }
    rememberCurrentChatSession({
      sessionId,
      projectPath,
      messages: nextMessages,
      live: true,
      status: "running",
    });
    ensureChatProcessing({
      provider: cli,
      sessionId: sessionId || state.chatSessionId,
    });
    updateChatComposerState();
  });
  // Chat-controls row: persist model/mode/effort changes immediately.
  qs("#chat-model")?.addEventListener("change", () => {
    state.preferences.chatModel = chatModelValue();
    savePreferences();
    // If a session is currently active, persist the override against it.
    const sid = state.chatSessionId || state.pendingChatSessionId || state.preferences.lastChatSessionId;
    if (sid) saveSessionOverrides(sid, { model: state.preferences.chatModel, cli: chatCliValue() });
  });
  qs("#chat-mode")?.addEventListener("change", () => {
    state.preferences.chatMode = chatModeValue();
    savePreferences();
    const sid = state.chatSessionId || state.pendingChatSessionId || state.preferences.lastChatSessionId;
    if (sid) saveSessionOverrides(sid, { mode: state.preferences.chatMode });
  });
  qs("#chat-effort")?.addEventListener("change", () => {
    state.preferences.chatEffort = chatEffortValue();
    savePreferences();
    const sid = state.chatSessionId || state.pendingChatSessionId || state.preferences.lastChatSessionId;
    if (sid) saveSessionOverrides(sid, { effort: state.preferences.chatEffort });
  });
  qs("#stop-shell").addEventListener("click", async () => {
    if (!state.currentShellProcess) return;
    await api(`/api/process/${state.currentShellProcess}`, { method: "DELETE" });
    state.currentShellProcess = null;
    state.currentShellProjectPath = "";
    resetShellResizeTracking();
    updateShellStatus();
    loadProcesses().catch(() => {});
  });
  qs("#refresh-processes").addEventListener("click", (event) => {
    state.shellProcessListOpen = !state.shellProcessListOpen;
    withButtonLoading(event.currentTarget, loadProcesses).catch(showError);
  });
}

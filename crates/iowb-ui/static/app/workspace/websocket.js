function connectWs() {
  if (state.ws) {
    const previous = state.ws;
    state.ws = null;
    previous.close();
  }
  if (state.wsRetry) {
    window.clearTimeout(state.wsRetry);
    state.wsRetry = null;
  }
  if (state.wsConnectTimer) {
    window.clearTimeout(state.wsConnectTimer);
    state.wsConnectTimer = null;
  }

  const generation = ++state.wsGeneration;
  const reconnecting = state.wsRetryAttempt > 0;
  setWsStatus(reconnecting ? "reconnecting" : "connecting", reconnecting ? `Reconnect attempt ${state.wsRetryAttempt + 1}` : "Opening WebSocket");
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  const token = state.token ? `?token=${encodeURIComponent(state.token)}` : "";
  const ws = new WebSocket(`${protocol}//${window.location.host}/ws${token}`);
  ws._iowbGeneration = generation;
  state.ws = ws;
  state.wsConnectTimer = window.setTimeout(() => {
    if (state.ws !== ws || ws.readyState === WebSocket.OPEN) return;
    setWsStatus("error", "WebSocket connection timed out");
    ws.close();
  }, WS_CONNECT_TIMEOUT_MS);

  ws.addEventListener("open", () => {
    if (state.ws !== ws || ws._iowbGeneration !== state.wsGeneration) return;
    window.clearTimeout(state.wsConnectTimer);
    state.wsConnectTimer = null;
    state.wsRetryAttempt = 0;
    setWsStatus("connected");
    ws.send(JSON.stringify({ type: "ping", nonce: String(Date.now()) }));
    sendWorkspaceSubscription();
    startChatActivityPoll();
    reconcileSelectedChatRecoverySnapshot(generation).catch((error) => {
      console.warn("Unable to refresh selected chat recovery after reconnect", error);
    });
  });

  ws.addEventListener("message", (event) => {
    let payload;
    try {
      payload = JSON.parse(event.data);
    } catch (error) {
      setWsStatus("error", `Invalid WebSocket payload: ${error.message}`);
      return;
    }
    if (payload.type === "projects_updated") {
      state.projects = payload.projects || [];
      hideBoardChatSessionsFromLists();
      syncProjectOrder();
      renderProjects();
    }
    if (payload.type === "project_files_changed") {
      scheduleProjectFilesRefresh(payload);
    }
    if (payload.type === "active_sessions") {
      state.sessions = (payload.sessions || []).filter((session) => !isBoardChatSession(session));
      state.sessions.forEach(mergeSessionIntoProjects);
      hideBoardChatSessionsFromLists();
      renderSidebarProjects();
      renderSessions();
    }
    if (payload.type === "session_status") {
      const status = String(payload.status || "").toLowerCase();
      const statusRunning = status === "starting" || status === "running" || status === "waiting-for-input";
      if (!acceptsOrderedChatResponseEvent(payload, {
        runningEvent: statusRunning,
        allowNewResponse: statusRunning
          || state.chatProcessing?.sessionId === payload.sessionId
          || state.currentSession?.sessionId === payload.sessionId,
      })) {
        return;
      }
      const sessionRecovery = state.chatRecoveryBySession[payload.sessionId || ""];
      if (statusRunning && sessionRecovery?.state === "starting") {
        const responseId = chatEventResponseId(payload);
        if (responseId) sessionRecovery.responseId = responseId;
      }
      const manualCompaction = state.chatManualCompactionBySession[payload.sessionId || ""];
      if (statusRunning && manualCompaction?.state === "starting") {
        const responseId = chatEventResponseId(payload);
        if (responseId) manualCompaction.responseId = responseId;
      }
      rememberOrderedChatResponseEvent(payload, { terminal: !statusRunning });
      rememberSidebarSessionStatus(payload);
      if (!isActiveChatSessionEvent(payload) && cachedChatSession(payload.sessionId || "")) {
        const normalized = normalizeSidebarSessionStatus(status);
        rememberCurrentChatSession({
          sessionId: payload.sessionId,
          live: normalized === "running",
          status: normalized || "",
        });
      }
      if (isActiveChatSessionEvent(payload)) {
        state.currentSession = {
          provider: payload.provider,
          sessionId: payload.sessionId,
        };
      } else if (payload.sessionId && state.currentSession?.sessionId === payload.sessionId) {
        state.currentSession = null;
      }
      if (statusRunning) {
        if (isActiveChatSessionEvent(payload)) {
          rememberCurrentChatSession({ sessionId: payload.sessionId, live: true, status: "running" });
          ensureChatProcessing(payload);
          if (!selectedChatIsStopping() && (status === "running" || status === "waiting-for-input")) {
            updateProcessingLabel("Processing");
          }
        }
      } else if (status === "completed") {
        if (sessionRecovery?.state === "starting" && chatRecoveryMatchesResponse(sessionRecovery, payload)) {
          delete state.chatRecoveryBySession[payload.sessionId || ""];
          if (isActiveChatSessionEvent(payload)) renderChatRecoveryCard(payload.sessionId);
        }
        if (manualCompaction?.state === "starting" && chatRecoveryMatchesResponse(manualCompaction, payload)) {
          rememberManualCompactionSuppression(payload.sessionId, chatEventResponseId(payload));
          delete state.chatManualCompactionBySession[payload.sessionId || ""];
          if (isActiveChatSessionEvent(payload)) renderChatManualCompactionCard(payload.sessionId);
        }
        if (isActiveChatSessionEvent(payload) && (!chatStream.node || chatStream.role !== "assistant" || !state.chatBuffer)) {
          finishChatProcessing(payload);
        }
        if (isActiveChatSessionEvent(payload)) {
          if (state.chatStoppingSessionId === payload.sessionId) state.chatStoppingSessionId = "";
          rememberCurrentChatSession({ sessionId: payload.sessionId, live: false, status: "completed" });
          scheduleCompletedChatReconciliation(payload.sessionId);
        }
        if (!payload.sessionId || state.currentSession?.sessionId === payload.sessionId) {
          state.currentSession = null;
        }
      } else if (status === "failed" || status === "aborted") {
        const label = status === "aborted" ? "Aborted" : "Failed";
        if (isActiveChatSessionEvent(payload) && (!chatStream.node || chatStream.role !== "assistant" || !state.chatBuffer.trim())) {
          finishChatProcessing(payload, label);
        }
        if (isActiveChatSessionEvent(payload)) {
          const recovery = state.chatRecoveryBySession[payload.sessionId || ""];
          if (recovery?.state === "starting" && chatRecoveryMatchesResponse(recovery, payload)) {
            recovery.pending = false;
            recovery.state = "failed";
            recovery.message = "The clean-context compaction failed. Your original context mapping was kept; you can try again.";
            renderChatRecoveryCard(payload.sessionId);
          }
          const compaction = state.chatManualCompactionBySession[payload.sessionId || ""];
          if (compaction?.state === "starting" && chatRecoveryMatchesResponse(compaction, payload)) {
            rememberManualCompactionSuppression(payload.sessionId, chatEventResponseId(payload));
            compaction.pending = false;
            compaction.state = "failed";
            compaction.message = status === "aborted"
              ? "The clean-context compaction was stopped. Your original context mapping was kept; you can try again."
              : "The clean-context compaction failed. Your original context mapping was kept; you can try again.";
            renderChatManualCompactionCard(payload.sessionId);
          }
          if (state.chatStoppingSessionId === payload.sessionId) state.chatStoppingSessionId = "";
          rememberCurrentChatSession({ sessionId: payload.sessionId, live: false, status: "failed" });
          scheduleCompletedChatReconciliation(payload.sessionId);
        }
        if (!payload.sessionId || state.currentSession?.sessionId === payload.sessionId) {
          state.currentSession = null;
        }
      }
      updateChatComposerState();
    }
    if (payload.type === "output") {
      if (!acceptsOrderedChatResponseEvent(payload, {
        runningEvent: payload.done !== true,
        allowNewResponse: chatSessionIsLive(payload.sessionId)
          || state.chatProcessing?.sessionId === payload.sessionId
          || state.currentSession?.sessionId === payload.sessionId,
      })) {
        return;
      }
      rememberOrderedChatResponseEvent(payload, { terminal: payload.done === true });
      if (isSuppressedManualCompactionResponse(payload)) {
        if (payload.done && (!payload.sessionId || state.currentSession?.sessionId === payload.sessionId)) {
          state.currentSession = null;
          updateChatComposerState();
        }
        return;
      }
      const manualCompaction = state.chatManualCompactionBySession[payload.sessionId || ""];
      if (chatManualCompactionMatchesResponse(manualCompaction, payload)) {
        const responseId = chatEventResponseId(payload);
        if (responseId) manualCompaction.responseId = responseId;
        if (payload.done) {
          rememberManualCompactionSuppression(payload.sessionId, responseId);
          delete state.chatManualCompactionBySession[payload.sessionId || ""];
          if (isActiveChatSessionEvent(payload)) {
            renderChatManualCompactionCard(payload.sessionId);
            finishChatProcessing(payload);
            scheduleCompletedChatReconciliation(payload.sessionId);
          }
          if (!payload.sessionId || state.currentSession?.sessionId === payload.sessionId) {
            state.currentSession = null;
          }
          updateChatComposerState();
        } else if (isActiveChatSessionEvent(payload)) {
          ensureChatProcessing(payload);
          updateProcessingLabel("Compacting context");
          updateChatComposerState();
        }
        return;
      }
      if (!isActiveChatSessionEvent(payload)) {
        rememberBackgroundChatOutput(payload);
        if (payload.done && state.currentSession?.sessionId === payload.sessionId) {
          state.currentSession = null;
          updateChatComposerState();
        }
        return;
      }
      state.currentSession = {
        provider: payload.provider,
        sessionId: payload.sessionId,
      };
      if (payload.content) appendChat(payload.content, {
        provider: payload.provider || "",
        sessionId: payload.sessionId || "",
      });
      if (payload.done) {
        // Finalize the assistant stream node: store the received-at time,
        // capture token usage, and write the footer into the bubble so the
        // data persists with the message itself.
        const hasAssistantContent = Boolean(state.chatBuffer.trim());
        if (!hasAssistantContent && state.chatProcessing?.sessionId === payload.sessionId) {
          finishChatProcessing(payload);
        }
        if (!payload.sessionId || state.currentSession?.sessionId === payload.sessionId) {
          state.currentSession = null;
          updateChatComposerState();
        }
        if (state.chatStoppingSessionId === payload.sessionId) state.chatStoppingSessionId = "";
        if (!hasAssistantContent) return;
        const receivedAt = new Date().toISOString();
        const sid = payload.sessionId;
        const proj = state.preferences.chatCli || state.preferences.chatProvider || "codex";
        let assistantMeta = {
          cli: proj,
          model: state.preferences.chatModel || "",
          effort: state.preferences.chatEffort || "",
          mode: state.preferences.chatMode || "",
          fast: state.preferences.chatFast ?? false,
          receivedAt: formatReceivedDateTime(receivedAt),
        };
        const finalizeBubble = (entry) => {
          if (chatStream.node && chatStream.role === "assistant") {
            renderChatResponseHeader(chatStream.node, entry);
            renderChatLineFooter(chatStream.node.querySelector(".chat-line-footer"), entry);
            const copyContent = withoutChatToolTelemetrySections(
              assistantResponseContent(
                state.chatBuffer,
                entry.cli || payload.provider || chatCliValue(),
                lastChatUserPromptContent(),
              ),
            );
            attachAssistantChatActions(chatStream.node, copyContent);
          }
        };
        if (sid) {
          // Optimistically write a provisional entry so the user sees the
          // footer immediately, then refine it with token usage.
          const all = readSessionOverrides();
          const prev = all[sid] || {};
          const provisional = {
            ...prev,
            cli: proj,
            model: prev.model || state.preferences.chatModel || "",
            effort: prev.effort || state.preferences.chatEffort || "",
            mode: prev.mode || state.preferences.chatMode || "",
            fast: prev.fast ?? state.preferences.chatFast ?? false,
            receivedAt: formatReceivedDateTime(receivedAt),
          };
          if (prev.sentAt) provisional.elapsed = formatElapsed(prev.sentAt, receivedAt);
          assistantMeta = provisional;
          finalizeBubble(provisional);
          const usageProvider = runtimeProviderForModel(
            proj,
            provisional.model || state.preferences.chatModel || "",
          );
          const usageModel = provisional.model || state.preferences.chatModel || "";
          if (shouldFetchTokenUsage(proj, usageModel) && findChatSession(sid)) {
            api(`/api/projects/${encodeURIComponent(state.activeProjectPath || "")}/sessions/${encodeURIComponent(sid)}/token-usage?provider=${encodeURIComponent(usageProvider)}`).then((usage) => {
              const tokenUsage = usage?.used ? `${usage.used} (in ${usage.breakdown?.input || 0} / out ${usage.breakdown?.output || 0})` : "";
              const all2 = readSessionOverrides();
              const prev2 = all2[sid] || {};
              const persistedEntry = {
                ...prev2,
                cli: proj,
                model: prev2.model || state.preferences.chatModel || "",
                effort: prev2.effort || state.preferences.chatEffort || "",
                mode: prev2.mode || state.preferences.chatMode || "",
                fast: prev2.fast ?? state.preferences.chatFast ?? false,
                receivedAt: formatReceivedDateTime(receivedAt),
                tokenUsage,
              };
              if (prev2.sentAt) persistedEntry.elapsed = formatElapsed(prev2.sentAt, receivedAt);
              all2[sid] = persistedEntry;
              writeSessionOverrides(all2);
              if (state.chatSessionId === sid || state.pendingChatSessionId === sid) {
                finalizeBubble(persistedEntry);
              }
              if (chatHistoryWindow.sessionId === sid) {
                const messages = chatHistoryWindow.messages.map((message) => {
                  if (message.id !== assistantMeta.id) return message;
                  return { ...message, metadata: persistedEntry };
                });
                chatHistoryWindow = { ...chatHistoryWindow, messages };
                rememberCurrentChatSession({ sessionId: sid, messages, live: false, status: "completed" });
              }
            }).catch(() => {});
          }
        }
        if (sid) {
          const assistantMessage = {
            id: `local-assistant-${Date.now()}`,
            role: "assistant",
            content: state.chatBuffer,
            timestamp: receivedAt,
            metadata: assistantMeta,
          };
          assistantMeta.id = assistantMessage.id;
          const currentMessages = chatHistoryWindow.sessionId === sid ? chatHistoryWindow.messages : [];
          const messages = currentMessages.concat(assistantMessage);
          chatHistoryWindow = {
            sessionId: sid,
            offset: chatHistoryWindow.sessionId === sid ? chatHistoryWindow.offset : 0,
            totalCount: Math.max(
              chatHistoryWindow.sessionId === sid ? chatHistoryWindow.totalCount + 1 : messages.length,
              messages.length,
            ),
            messages,
          };
          rememberSidebarSessionStatus({ sessionId: sid, provider: payload.provider || proj, status: "completed" });
          rememberCurrentChatSession({ sessionId: sid, messages, live: false, status: "completed" });
          delete state.chatOutputBuffersBySession[sid];
          scheduleCompletedChatReconciliation(sid);
        }
      }
    }
    if (payload.type === "session_metadata") {
      // Server broadcasts final metadata when the agent finishes. Update the
      // footer stored on the active stream node and persist it.
      const sid = payload.sessionId;
      if (!acceptsOrderedChatResponseEvent(payload, {
        runningEvent: false,
        allowNewResponse: state.chatProcessing?.sessionId === sid
          || state.currentSession?.sessionId === sid,
      })) {
        return;
      }
      rememberOrderedChatResponseEvent(payload, { terminal: true });
      if (sid) {
        const all = readSessionOverrides();
        const prev = all[sid] || {};
        const receivedAt = payload.receivedAt || prev.receivedAt || new Date().toISOString();
        const tokenUsage = payload.tokenUsage
          ? (typeof payload.tokenUsage.used === "number"
            ? `${payload.tokenUsage.used} (in ${payload.tokenUsage.input || 0} / out ${payload.tokenUsage.output || 0})`
            : "")
          : prev.tokenUsage || "";
        const entry = {
          ...prev,
          cli: payload.provider || prev.cli,
          model: payload.model ?? prev.model ?? state.preferences.chatModel ?? "",
          effort: payload.effort ?? prev.effort ?? state.preferences.chatEffort ?? "",
          mode: payload.mode ?? prev.mode ?? state.preferences.chatMode ?? "",
          thinking: payload.thinking ?? prev.thinking ?? state.preferences.chatThinking ?? false,
          fast: payload.fast ?? prev.fast ?? state.preferences.chatFast ?? false,
          receivedAt: formatReceivedDateTime(receivedAt),
          tokenUsage,
        };
        if (prev.sentAt) entry.elapsed = formatElapsed(prev.sentAt, receivedAt);
        all[sid] = entry;
        writeSessionOverrides(all);
        if (
          payload.nativeSessionId ||
          payload.native_session_id ||
          payload.lifetimeTokenUsage ||
          payload.contextTokenUsage ||
          payload.spentTokenUsage
        ) {
          const nativeId = nativeSessionId(payload);
          const patchSession = (session) => session?.id === sid
            ? {
              ...session,
              nativeSessionId: nativeId || session.nativeSessionId,
              lifetimeTokenUsage: payload.lifetimeTokenUsage || session.lifetimeTokenUsage,
              contextTokenUsage: payload.contextTokenUsage || session.contextTokenUsage,
              spentTokenUsage: payload.spentTokenUsage || session.spentTokenUsage,
            }
            : session;
          state.sessions = (state.sessions || []).map(patchSession);
          state.projects = (state.projects || []).map((project) => ({
            ...project,
            sessions: (project.sessions || []).map(patchSession),
          }));
          renderSidebarProjects();
          renderSidebarSessions();
          renderPinnedSidebarSessions();
          if (activeView() === "sessions") renderSessions();
        }
        if (state.chatSessionId === sid || state.pendingChatSessionId === sid) {
          if (chatStream.node && chatStream.role === "assistant") {
            renderChatResponseHeader(chatStream.node, entry);
            renderChatLineFooter(chatStream.node.querySelector(".chat-line-footer"), entry);
            const copyContent = withoutChatToolTelemetrySections(
              assistantResponseContent(
                state.chatBuffer,
                entry.cli || payload.provider || chatCliValue(),
                lastChatUserPromptContent(),
              ),
            );
            attachAssistantChatActions(chatStream.node, copyContent);
          }
        }
      }
    }
    if (payload.type === "chat_recovery_required") {
      handleChatRecoveryRequired(payload);
    }
    if (payload.type === "process_output") {
      appendShell(payload.data);
    }
    if (payload.type === "process_exited") {
      appendShell(`\n[process exited: ${payload.code ?? "terminated"}]\n`);
      if (state.currentShellProcess === payload.processId) {
        state.currentShellProcess = null;
        state.currentShellProjectPath = "";
        resetShellResizeTracking();
        updateShellStatus();
      }
    }
    if (payload.type === "error") {
      if (payload.sessionId && !isActiveChatSessionEvent(payload)) return;
      appendChatLine(`[error] ${payload.message}${payload.details ? `: ${payload.details}` : ""}`);
    }
  });

  ws.addEventListener("close", () => {
    if (state.ws !== ws || ws._iowbGeneration !== state.wsGeneration) return;
    window.clearTimeout(state.wsConnectTimer);
    state.wsConnectTimer = null;
    state.ws = null;
    if (!state.auth?.enabled || state.token) {
      const delay = Math.min(WS_RETRY_MAX_MS, WS_RETRY_BASE_MS * (2 ** state.wsRetryAttempt));
      state.wsRetryAttempt += 1;
      setWsStatus("reconnecting", `WebSocket closed. Retrying in ${Math.round(delay / 1000)}s`);
      state.wsRetry = window.setTimeout(connectWs, delay);
      return;
    }
    setWsStatus("error", "WebSocket closed before authentication");
  });

  ws.addEventListener("error", () => {
    if (state.ws !== ws || ws._iowbGeneration !== state.wsGeneration) return;
    setWsStatus("error", "WebSocket error");
  });
}

function sendWorkspaceSubscription() {
  if (state.ws?.readyState !== WebSocket.OPEN) return false;
  const boardSessionId = String(state.boardWsSessionId || "").trim();
  state.ws.send(JSON.stringify({
    type: "subscribe",
    topics: ["sessions", "processes", "projects"],
    sessionIds: boardSessionId ? [boardSessionId] : [],
  }));
  return true;
}

function setBoardChatWsSubscription(sessionId = "") {
  state.boardWsSessionId = String(sessionId || "").trim();
  sendWorkspaceSubscription();
}

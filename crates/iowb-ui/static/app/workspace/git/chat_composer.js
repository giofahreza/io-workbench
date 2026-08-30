function selectedSessionId() {
  return qs("#session-id-input")?.value.trim()
    || state.pendingChatSessionId
    || state.sessions[0]?.id
    || "";
}

function renderSearchResults(body) {
  const results = firstDefined(body.results, body.conversations, body.sessions, body.items, []);
  if (!Array.isArray(results) || !results.length) {
    setOutput("#sessions-output", "No matching conversations.", "empty-output");
    return;
  }
  const target = qs("#sessions-output");
  target.className = "output-panel result-list";
  target.innerHTML = results.map((result) => {
    const id = firstDefined(result.sessionId, result.session_id, result.id, "");
    const title = firstDefined(result.title, result.summary, id, "Conversation");
    const meta = [result.provider, result.projectPath, result.timestamp, result.updatedAt]
      .filter(Boolean)
      .join(" · ");
    const excerpt = firstDefined(result.excerpt, result.content, result.preview, "");
    return `<article class="result-row">
      <strong>${escapeHtml(title)}</strong>
      <span class="meta">${escapeHtml(meta)}</span>
      ${excerpt ? `<div class="message-body">${renderMarkdownLite(excerpt)}</div>` : ""}
      ${id ? `<button type="button" data-session-result="${escapeHtml(id)}">Use Session</button>` : ""}
    </article>`;
  }).join("");
  target.querySelectorAll("[data-session-result]").forEach((button) => {
    button.addEventListener("click", () => {
      qs("#session-id-input").value = button.dataset.sessionResult;
      loadSessionMessages().catch(showError);
    });
  });
}

function renderSessionMessages(body = null) {
  if (body) {
    state.lastSessionMessages = Array.isArray(body) ? body : body.messages || [];
  }
  const messages = state.lastSessionMessages;
  const target = qs("#sessions-output");
  target.className = "output-panel message-list";
  if (!messages.length) {
    target.textContent = "No messages for this session.";
    return;
  }
  renderVirtualList(target, "sessionMessages", messages, {
    rowHeight: 180,
    maxHeight: 640,
    minRows: 4,
    render: (message) => {
      const role = firstDefined(message.role, "message");
      const timestamp = message.timestamp ? new Date(message.timestamp).toLocaleString() : "";
      return `<article class="message ${escapeHtml(role)}">
        <header>
          <strong>${escapeHtml(role)}</strong>
          <span>${escapeHtml(timestamp)}</span>
        </header>
        <div class="message-body">${renderMarkdownLite(message.content || "")}</div>
      </article>`;
    },
  });
}

async function uploadChatImages() {
  const project = activeProjectKey();
  const files = [...qs("#chat-image-input").files];
  if (!project || !files.length) return;
  const formData = new FormData();
  files.forEach((file) => formData.append("images", file));
  const body = await apiUpload(`/api/projects/${encodeURIComponent(project)}/upload-images`, formData);
  const uploaded = (body.images || []).map((image, index) => ({
    ...image,
    previewUrl: files[index] ? URL.createObjectURL(files[index]) : "",
  }));
  const combined = [...state.chatImages, ...uploaded];
  combined.slice(0, Math.max(0, combined.length - 5)).forEach((image) => {
    if (image.previewUrl?.startsWith("blob:")) URL.revokeObjectURL(image.previewUrl);
  });
  state.chatImages = combined.slice(-5);
  qs("#chat-image-input").value = "";
  renderChatImages();
  showToast(`Attached ${files.length} image${files.length === 1 ? "" : "s"}`, "ok");
}

function clearChatImages() {
  state.chatImages.forEach((image) => {
    if (image.previewUrl?.startsWith("blob:")) URL.revokeObjectURL(image.previewUrl);
  });
  state.chatImages = [];
  qs("#chat-image-input").value = "";
  renderChatImages();
}

function renderChatImages() {
  const target = qs("#chat-image-preview");
  const count = qs("#chat-image-count");
  if (count) {
    count.textContent = String(state.chatImages.length);
    count.setAttribute("aria-label", `${state.chatImages.length} image${state.chatImages.length === 1 ? "" : "s"}`);
  }
  if (!target) return;
  target.innerHTML = state.chatImages.length
    ? state.chatImages.map((image) => `<article class="image-preview">
      ${image.previewUrl ? `<img src="${escapeHtml(image.previewUrl)}" alt="${escapeHtml(image.name || "attached image")}" />` : ""}
      <span>${escapeHtml(image.name || "image")} · ${escapeHtml(formatBytes(image.size || 0))}</span>
    </article>`).join("")
    : "";
  updateChatComposerState();
}

function chatPromptWithImages(prompt) {
  if (!state.chatImages.length) return prompt;
  const imageMarkdown = state.chatImages
    .map((image) => `Attached image file: \`${image.path}\` (${image.name || "image"}, ${image.mimeType || "image"})`)
    .join("\n");
  return `${prompt}\n\n${imageMarkdown}`;
}

function autosizeChatPrompt() {
  const input = qs("#chat-prompt");
  if (!input) return;
  input.style.height = "auto";
  const styles = getComputedStyle(input);
  const maxHeight = Number.parseFloat(styles.maxHeight) || 180;
  const minHeight = Number.parseFloat(styles.minHeight) || 50;
  input.style.height = `${Math.min(maxHeight, Math.max(minHeight, input.scrollHeight))}px`;
  if (!input.value) input.scrollTop = 0;
  updateChatComposerState();
}

function clearChatPromptInput() {
  const input = qs("#chat-prompt");
  if (!input) return;
  input.style.transition = "none";
  input.value = "";
  noteChatPromptUserEdit("");
  input.style.removeProperty("height");
  input.scrollTop = 0;
  void input.offsetHeight;
  autosizeChatPrompt();
  void input.offsetHeight;
  input.style.removeProperty("transition");
}

function updateChatComposerState() {
  const input = qs("#chat-prompt");
  const clear = qs("#clear-chat");
  const submit = qs("#chat-submit");
  const thinking = qs("#chat-thinking-toggle");
  const manualCompact = qs("#compact-chat-session");
  const hasPrompt = Boolean(input?.value.trim());
  const hasProject = Boolean(activeProjectPath());
  const canSubmit = hasPrompt || state.chatImages.length > 0;
  const stopping = selectedChatIsStopping();
  const busy = Boolean(selectedRunningChatSession() || state.chatProcessing || stopping);
  const replacing = state.chatEditFromHere.submitting;
  const recovery = state.chatRecoveryBySession[state.chatSessionId || ""];
  const recoveryBlocksSend = chatRecoveryBlocksNormalSend(recovery);
  const recoveringContext = chatRecoveryIsStarting(recovery);

  if (input) {
    input.disabled = !hasProject || replacing || recoveringContext;
    input.placeholder = recoveringContext
      ? "Compacting this chat's context"
      : replacing
      ? "Creating replacement chat"
      : (hasProject ? "Ask the agent" : "Add a project to start chatting");
  }
  qs("#chat-form")?.classList.toggle("no-project", !hasProject);
  const promptConfigToggle = qs("#prompt-config-toggle");
  if (promptConfigToggle) promptConfigToggle.disabled = !hasProject;

  if (clear) {
    clear.classList.toggle("is-empty", !hasPrompt);
    clear.setAttribute("aria-hidden", hasPrompt ? "false" : "true");
    clear.tabIndex = hasPrompt ? 0 : -1;
  }
  if (thinking) {
    const enabled = chatThinkingValue();
    thinking.classList.toggle("active", enabled);
    thinking.dataset.symbol = enabled ? "thinking-on" : "thinking-off";
    thinking.setAttribute("aria-label", enabled ? "Disable thinking" : "Enable thinking");
    thinking.title = enabled ? "Disable thinking" : "Enable thinking";
  }
  if (manualCompact) {
    const manualCompaction = state.chatManualCompactionBySession[state.chatSessionId || ""];
    const manualStarting = chatManualCompactionIsStarting(manualCompaction);
    manualCompact.disabled = !selectedChatCanManualCompact() || manualStarting;
    manualCompact.classList.toggle("active", manualStarting);
    const label = manualStarting ? "Compacting context" : "Compact context";
    manualCompact.setAttribute("aria-label", label);
    manualCompact.title = label;
  }
  updateChatFastControl();
  if (submit) {
    submit.disabled = recoveryBlocksSend || replacing || stopping || (!busy && (!hasProject || !canSubmit));
    submit.dataset.symbol = busy ? "stop" : "send";
    const submitLabel = recoveringContext
      ? "Compacting context"
      : recoveryBlocksSend
      ? "Compact & retry required"
      : (stopping ? "Stopping" : (busy ? "Abort chat" : "Send"));
    submit.setAttribute("aria-label", submitLabel);
    submit.title = submitLabel;
    submit.classList.toggle("is-stop", busy);
  }
}

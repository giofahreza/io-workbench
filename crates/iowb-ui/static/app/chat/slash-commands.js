const chatSlashState = { scope: "", catalog: null, loading: null, selected: 0, hiddenFor: null, fetchedAt: 0, preparing: false };

function chatSlashContext() {
  const session = findChatSession(state.chatSessionId);
  return { provider: chatCliValue(), projectPath: activeProjectPath(),
    sessionId: session?.id || (state.chatSessionId && !state.chatSessionId.startsWith("local-") ? state.chatSessionId : null),
    model: chatModelValue() || null };
}

function chatSlashName(value) {
  return /^\s*\/([a-zA-Z0-9_:-]{1,160})(?:\s|$)/.exec(value)?.[1] || null;
}

async function loadChatSlashCatalog(refresh = false) {
  const context = chatSlashContext();
  const scope = JSON.stringify(context);
  if (!context.projectPath || !context.provider) return null;
  if (chatSlashState.scope !== scope) {
    Object.assign(chatSlashState, { scope, catalog: null, loading: null, selected: 0 });
  }
  if (!refresh && chatSlashState.catalog && Date.now() - chatSlashState.fetchedAt < 15000) return chatSlashState.catalog;
  if (chatSlashState.loading) return chatSlashState.loading;
  const query = new URLSearchParams(Object.entries(context).filter(([, value]) => value != null));
  if (refresh) query.set("refresh", "true");
  const pending = api(`/api/chat/commands?${query}`).then((catalog) => {
    if (chatSlashState.scope === scope) Object.assign(chatSlashState, { catalog, fetchedAt: Date.now() });
    return catalog;
  }).finally(() => { if (chatSlashState.loading === pending) chatSlashState.loading = null; });
  chatSlashState.loading = pending;
  return pending;
}

function chatSlashMatches() {
  if (chatSlashState.scope !== JSON.stringify(chatSlashContext())) return [];
  const text = qs("#chat-prompt")?.value || "";
  if (!/^\/[a-zA-Z0-9_:-]*$/.test(text) || chatSlashState.hiddenFor === text) return [];
  return (chatSlashState.catalog?.commands || []).filter((command) => command.name.toLowerCase().startsWith(text.slice(1).toLowerCase()));
}

function renderChatSlashSuggestions() {
  const menu = qs("#chat-slash-commands");
  if (!menu) return;
  const matches = chatSlashMatches().slice(0, 40);
  menu.replaceChildren();
  menu.hidden = matches.length === 0;
  qs("#chat-prompt")?.setAttribute("aria-expanded", String(!menu.hidden));
  chatSlashState.selected = Math.min(chatSlashState.selected, Math.max(0, matches.length - 1));
  matches.forEach((command, index) => {
    const button = document.createElement("button");
    button.type = "button";
    button.setAttribute("role", "option");
    button.setAttribute("aria-selected", String(index === chatSlashState.selected));
    button.className = index === chatSlashState.selected ? "selected" : "";
    button.textContent = `/${command.name} ${command.argumentHint || ""} — ${command.available ? command.description : command.reason} · ${command.source}${command.verified ? "" : " (provisional)"}`;
    button.disabled = !command.available;
    button.addEventListener("mousedown", (event) => event.preventDefault());
    button.addEventListener("click", () => selectChatSlashCommand(command));
    menu.append(button);
  });
}

function selectChatSlashCommand(command) {
  if (!command.available) return;
  setChatPromptValue(`/${command.name} `);
  qs("#chat-prompt")?.dispatchEvent(new Event("input", { bubbles: true }));
  qs("#chat-prompt")?.focus();
  renderChatSlashSuggestions();
}

function handleChatSlashKeydown(event) {
  if (event.isComposing || qs("#chat-slash-commands")?.hidden !== false) return false;
  const matches = chatSlashMatches().slice(0, 40);
  if (!matches.length) return false;
  if (event.key === "Escape") chatSlashState.hiddenFor = qs("#chat-prompt").value;
  else if (["ArrowUp", "ArrowDown"].includes(event.key)) {
    chatSlashState.selected = (chatSlashState.selected + (event.key === "ArrowUp" ? -1 : 1) + matches.length) % matches.length;
  } else if (event.key === "Tab" || (event.key === "Enter" && !event.shiftKey)) selectChatSlashCommand(matches[chatSlashState.selected]);
  else return false;
  event.preventDefault();
  renderChatSlashSuggestions();
  return true;
}

function bindChatSlashCommands() {
  const input = qs("#chat-prompt");
  input?.addEventListener("input", () => {
    chatSlashState.hiddenFor = null;
    chatSlashState.selected = 0;
    renderChatSlashSuggestions();
    if (/^\/[a-zA-Z0-9_:-]*$/.test(input.value)) loadChatSlashCatalog().then(renderChatSlashSuggestions).catch((error) => {
      const menu = qs("#chat-slash-commands");
      if (menu && input.value.startsWith("/")) {
        menu.hidden = false;
        menu.textContent = `Commands unavailable: ${error.message}`;
      }
    });
  });
}

async function interceptChatSlashCommand(prompt) {
  if (!chatSlashName(prompt)) return false;
  if (chatSlashState.preparing) return true;
  chatSlashState.preparing = true;
  const scope = JSON.stringify(chatSlashContext());
  const unchanged = () => scope === JSON.stringify(chatSlashContext()) && qs("#chat-prompt").value.trim() === prompt;
  try {
    const prepared = await api("/api/chat/commands/prepare", { method: "POST", body: JSON.stringify({ ...chatSlashContext(), prompt }) });
    if (!unchanged()) return true;
    if (prepared.command && (state.chatImages?.length || stagedChatEdit())) throw new Error("Send commands separately from attachments or edit-from-here.");
    if (prepared.action === "prompt") return false;
    if (prepared.action === "model") {
      togglePromptConfigPanel(true);
      const picker = qs("#chat-model");
      picker?.focus();
      picker?.click();
    } else if (prepared.action === "new") await startNewChatForProject(activeProjectPath());
    else if (prepared.action === "compact") await compactChatSessionContext(state.chatSessionId);
    else if (prepared.action === "help") {
      const catalog = await loadChatSlashCatalog(true);
      if (!unchanged()) return true;
      window.alert(catalog.commands.map((command) => `/${command.name} — ${command.description}${command.available ? "" : ` (${command.reason})`}`).join("\n"));
    } else if (prepared.action === "mcp") window.alert(prepared.message);
    else if (prepared.action === "status") showToast(`${chatCliValue()} · ${chatModelValue() || "CLI default"} · ${selectedRunningChatSession() ? "running" : "idle"}`);
    clearChatPromptInput();
    renderChatSlashSuggestions();
    return true;
  } catch (error) {
    if (unchanged()) showError(error);
    return true;
  } finally { chatSlashState.preparing = false; }
}

function commandPaletteCommands() {
  const viewCommands = Object.entries(VIEW_NAMES).map(([view, label]) => ({
    id: `view-${view}`,
    title: label,
    section: "View",
    keywords: `open switch panel ${view}`,
    run: () => switchView(view),
  }));
  const projectCommands = state.projects.map((project) => ({
    id: `project-${project.id || project.path}`,
    title: `Use ${projectDisplayName(project)}`,
    section: "Projects",
    keywords: `${projectDisplayName(project)} ${project.name} ${project.path} workspace select`,
    run: async () => {
      setActiveProject(project.path);
      renderSidebarProjects();
      await loadView(activeView());
    },
  }));
  const sessionCommands = sidebarSessions().slice(0, 30).map((session) => ({
    id: `session-${session.provider || "agent"}-${session.id}`,
    title: session.title || session.summary || session.id,
    section: "Sessions",
    keywords: `${session.id} ${session.provider || ""} ${session.projectPath || ""} chat history conversation`,
    run: async () => {
      if (session.provider) setChatProvider(session.provider);
      await pickChatSession(session.id, session.projectPath || "");
    },
  }));
  const fileCommands = flattenFileEntries(state.fileEntries)
    .filter(({ entry }) => entry.type !== "directory")
    .slice(0, 40)
    .map(({ entry }) => ({
      id: `file-${entry.path}`,
      title: entry.path,
      section: "Files",
      keywords: `${entry.name || ""} ${entry.path} edit open file`,
      run: async () => {
        if (await switchView("files")) await openFile(entry.path);
      },
    }));
  return [
    ...viewCommands,
    ...projectCommands,
    ...sessionCommands,
    ...fileCommands,
    {
      id: "refresh-current",
      title: `Refresh ${VIEW_NAMES[activeView()] || "Current View"}`,
      section: "Current",
      keywords: "reload sync update current",
      run: refreshCurrentView,
    },
    {
      id: "save-current-file",
      title: "Save Current File",
      section: "Files",
      keywords: "editor write persist ctrl s",
      disabled: () => !qs("#file-editor-path")?.value.trim(),
      run: () => saveFile(new Event("submit")),
    },
    {
      id: "focus-editor-search",
      title: "Find In File",
      section: "Files",
      keywords: "search replace editor ctrl f",
      run: async () => {
        if (await switchView("files")) {
          qs("#editor-search")?.focus();
          qs("#editor-search")?.select();
        }
      },
    },
    {
      id: "reload-current-file",
      title: "Reload Current File",
      section: "Files",
      keywords: "refresh editor discard",
      disabled: () => !qs("#file-editor-path")?.value.trim(),
      run: reloadCurrentFile,
    },
    {
      id: "copy-current-file-path",
      title: "Copy Current File Path",
      section: "Files",
      keywords: "clipboard path editor",
      disabled: () => !qs("#file-editor-path")?.value.trim(),
      run: copyCurrentFilePath,
    },
    {
      id: "git-status",
      title: "Refresh Git Status",
      section: "Git",
      keywords: "changes branch working tree",
      run: async () => {
        if (await switchView("git")) await loadGitStatus();
      },
    },
    {
      id: "git-conflicts",
      title: "Show Git Conflicts",
      section: "Git",
      keywords: "merge conflict resolve ours theirs",
      run: async () => {
        if (await switchView("git")) await loadGitConflicts();
      },
    },
    {
      id: "git-diff-selected",
      title: "Diff Selected Git File",
      section: "Git",
      keywords: "review patch changes",
      disabled: () => !selectedGitFiles().length,
      run: async () => {
        if (await switchView("git")) await gitDiffSelected();
      },
    },
    {
      id: "git-stage-selected",
      title: "Stage Selected Git Files",
      section: "Git",
      keywords: "add index selected",
      disabled: () => !selectedGitFiles().length,
      run: async () => {
        if (await switchView("git")) await gitSelectedFileOperation("/api/git/stage");
      },
    },
    {
      id: "git-unstage-selected",
      title: "Unstage Selected Git Files",
      section: "Git",
      keywords: "reset index selected",
      disabled: () => !selectedGitFiles().length,
      run: async () => {
        if (await switchView("git")) await gitSelectedFileOperation("/api/git/unstage");
      },
    },
    {
      id: "database-explorer",
      title: "Open Database Explorer",
      section: "Database",
      keywords: "schema objects tables",
      run: async () => {
        if (await switchView("database")) await loadDbExplorer();
      },
    },
    {
      id: "database-diagram",
      title: "Open Relationship Diagram",
      section: "Database",
      keywords: "erd foreign keys schema graph",
      run: async () => {
        if (await switchView("database")) await loadDbRelationshipDiagram();
      },
    },
    {
      id: "database-jobs",
      title: "Show Database Jobs",
      section: "Database",
      keywords: "import export transfer history",
      run: async () => {
        if (await switchView("database")) await loadDbJobs();
      },
    },
    {
      id: "clear-chat",
      title: "Clear Chat Output",
      section: "Chat",
      keywords: "reset transcript",
      run: async () => {
        if (await switchView("chat")) {
          state.chatBuffer = "";
          resetChatOutputDom();
        }
      },
    },
    {
      id: "abort-session",
      title: "Abort Current Chat Session",
      section: "Chat",
      keywords: "stop cancel agent",
      disabled: () => !selectedRunningChatSession(),
      run: async () => {
        const selectedRun = selectedRunningChatSession();
        if (await switchView("chat")) requestAbortSelectedChatSession(selectedRun);
      },
    },
    {
      id: "clear-shell",
      title: "Clear Shell Output",
      section: "Shell",
      keywords: "terminal reset",
      run: async () => {
        if (await switchView("shell")) {
          state.shellBuffer = "";
          renderShell();
        }
      },
    },
    {
      id: "refresh-processes",
      title: "Refresh Processes",
      section: "Shell",
      keywords: "terminal process pty",
      run: async () => {
        if (await switchView("shell")) await loadProcesses();
      },
    },
    {
      id: "interrupt-shell",
      title: "Send Ctrl-C To Shell",
      section: "Shell",
      keywords: "interrupt terminal stop",
      disabled: () => !state.currentShellProcess,
      run: async () => {
        if (await switchView("shell")) await sendShellInput("\x03");
      },
    },
    {
      id: "tools-runs",
      title: "Refresh Tool Runs",
      section: "Settings",
      keywords: "mcp commands plugins history",
      run: async () => {
        if (await switchView("settings")) {
          setSettingsTab("tools");
          await loadToolRuns();
        }
      },
    },
    {
      id: "settings-api-keys",
      title: "Open API Key Settings",
      section: "Settings",
      keywords: "credentials tokens settings",
      run: async () => {
        if (await switchView("settings")) {
          setSettingsTab("api");
          await loadSettingsView("/api/settings/api-keys");
        }
      },
    },
    {
      id: "settings-notifications",
      title: "Open Notification Settings",
      section: "Settings",
      keywords: "browser push permission preferences",
      run: async () => {
        if (await switchView("settings")) {
          setSettingsTab("notifications");
          await loadSettingsView("/api/settings/notification-preferences");
        }
      },
    },
    {
      id: "preview-browser-notification",
      title: "Preview Browser Notification",
      section: "Settings",
      keywords: "permission notify alert pwa",
      disabled: () => !("Notification" in window),
      run: async () => {
        if (await switchView("settings")) await previewBrowserNotification();
      },
    },
    {
      id: "api-docs",
      title: "Open API Docs",
      section: "Web",
      keywords: "routes reference endpoints",
      run: () => window.open("/api-docs.html", "_blank", "noopener"),
    },
    {
      id: "cache-tools",
      title: "Open Cache Tools",
      section: "Web",
      keywords: "pwa service worker offline clear",
      run: () => window.open("/clear-cache.html", "_blank", "noopener"),
    },
  ];
}

function commandPaletteFilteredCommands() {
  const query = state.commandPalette.query.trim().toLowerCase();
  const commands = commandPaletteCommands();
  if (!query) return commands;
  const tokens = query.split(/\s+/).filter(Boolean);
  return commands.filter((command) => {
    const haystack = [command.title, command.section, command.keywords]
      .filter(Boolean)
      .join(" ")
      .toLowerCase();
    return tokens.every((token) => haystack.includes(token));
  });
}

function isCommandDisabled(command) {
  return !!command.disabled?.();
}

function openCommandPalette() {
  closeChatEditFromHerePicker();
  state.commandPalette.open = true;
  state.commandPalette.query = "";
  state.commandPalette.selectedIndex = 0;
  qs("#command-palette")?.classList.remove("hidden");
  const search = qs("#command-search");
  if (search) search.value = "";
  renderCommandPalette();
  window.setTimeout(() => qs("#command-search")?.focus(), 0);
}

function closeCommandPalette() {
  state.commandPalette.open = false;
  qs("#command-palette")?.classList.add("hidden");
}

function openMoreSheet() {
  qs("#more-sheet")?.classList.remove("hidden");
}

function closeMoreSheet() {
  qs("#more-sheet")?.classList.add("hidden");
}

function openAddProjectFolderBrowser() {
  closeSidebar();
  openFolderBrowser("", { action: "add-project" });
}

function toggleSidebar() {
  const mobile = window.matchMedia("(max-width: 760px)").matches;
  const opening = mobile
    ? !document.body.classList.contains("sidebar-open")
    : document.body.classList.contains("sidebar-collapsed");
  if (mobile) {
    document.body.classList.toggle("sidebar-open");
    qs("#bottom-sidebar")?.classList.toggle("active", document.body.classList.contains("sidebar-open"));
  } else {
    document.body.classList.toggle("sidebar-collapsed");
    qs("#bottom-sidebar")?.classList.toggle("active", !document.body.classList.contains("sidebar-collapsed"));
  }
  if (opening) {
    loadSharedPinnedChatSessions().catch((error) => {
      console.debug("shared pinned chat refresh skipped", error);
    });
  }
}

function closeSidebar() {
  document.body.classList.remove("sidebar-open");
  if (window.matchMedia("(max-width: 760px)").matches) {
    qs("#bottom-sidebar")?.classList.remove("active");
  }
}

function showToast(message, tone = "info") {
  const stack = qs("#toast-stack");
  if (!stack) return;
  const toast = document.createElement("div");
  toast.className = `toast ${tone}`;
  toast.textContent = message;
  stack.appendChild(toast);
  window.setTimeout(() => {
    toast.classList.add("leaving");
    window.setTimeout(() => toast.remove(), 180);
  }, 3200);
}

function renderCommandPalette() {
  const target = qs("#command-results");
  if (!target) return;
  const commands = commandPaletteFilteredCommands();
  state.commandPalette.selectedIndex = Math.min(
    Math.max(0, state.commandPalette.selectedIndex),
    Math.max(0, commands.length - 1),
  );
  if (!commands.length) {
    target.innerHTML = '<p class="empty">No commands found.</p>';
    return;
  }
  target.innerHTML = commands.map((command, index) => {
    const active = index === state.commandPalette.selectedIndex ? "active" : "";
    const disabled = isCommandDisabled(command);
    return `<button type="button" class="command-result ${active}" data-command-id="${escapeHtml(command.id)}" ${disabled ? "disabled" : ""}>
      <span>
        <strong>${escapeHtml(command.title)}</strong>
        <span class="command-meta">${escapeHtml(command.section || "")}</span>
      </span>
      ${disabled ? '<span class="badge warn">Unavailable</span>' : ""}
    </button>`;
  }).join("");
  target.querySelectorAll("[data-command-id]").forEach((button) => {
    button.addEventListener("mouseenter", () => {
      const index = commands.findIndex((command) => command.id === button.dataset.commandId);
      if (index >= 0) state.commandPalette.selectedIndex = index;
    });
    button.addEventListener("click", () => {
      executeCommand(button.dataset.commandId).catch(showError);
    });
  });
  target.querySelector(".command-result.active")?.scrollIntoView({ block: "nearest" });
}

function moveCommandPaletteSelection(delta) {
  const commands = commandPaletteFilteredCommands();
  if (!commands.length) return;
  let next = state.commandPalette.selectedIndex;
  for (let step = 0; step < commands.length; step += 1) {
    next = (next + delta + commands.length) % commands.length;
    if (!isCommandDisabled(commands[next])) break;
  }
  state.commandPalette.selectedIndex = next;
  renderCommandPalette();
}

async function executeCommand(commandId) {
  const command = commandPaletteCommands().find((item) => item.id === commandId)
    || commandPaletteFilteredCommands()[state.commandPalette.selectedIndex];
  if (!command || isCommandDisabled(command)) return;
  closeCommandPalette();
  await command.run();
}

function bindCommandPalette() {
  qs("#open-command-palette")?.addEventListener("click", openCommandPalette);
  qs("#sidebar-command-palette")?.addEventListener("click", openCommandPalette);
  qs("#command-palette")?.addEventListener("click", (event) => {
    if (event.target === event.currentTarget) closeCommandPalette();
  });
  qs("#command-search")?.addEventListener("input", (event) => {
    state.commandPalette.query = event.currentTarget.value;
    state.commandPalette.selectedIndex = 0;
    renderCommandPalette();
  });
  qs("#command-search")?.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      moveCommandPaletteSelection(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      moveCommandPaletteSelection(-1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const command = commandPaletteFilteredCommands()[state.commandPalette.selectedIndex];
      executeCommand(command?.id).catch(showError);
    } else if (event.key === "Escape") {
      event.preventDefault();
      closeCommandPalette();
    }
  });
  document.addEventListener("keydown", (event) => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      openCommandPalette();
    } else if (event.key === "Escape" && state.folderBrowser.open) {
      event.preventDefault();
      closeFolderBrowser();
    } else if (event.key === "Escape" && document.body.classList.contains("sidebar-open")) {
      event.preventDefault();
      closeSidebar();
    } else if (event.key === "Escape" && state.commandPalette.open) {
      event.preventDefault();
      closeCommandPalette();
    }
  });
}

function registerServiceWorker() {
  if (!("serviceWorker" in navigator)) return;
  window.localStorage.setItem(APP_VERSION_STORAGE_KEY, APP_VERSION);
  const hadController = Boolean(navigator.serviceWorker.controller);

  const reloadForUpdatedShell = (reason = "service-worker") => {
    if (window.sessionStorage.getItem(APP_RELOAD_STORAGE_KEY)) return;
    window.sessionStorage.setItem(APP_RELOAD_STORAGE_KEY, reason);
    const url = new URL(window.location.href);
    url.searchParams.set("v", APP_VERSION);
    window.location.replace(url.href);
  };

  navigator.serviceWorker.addEventListener("controllerchange", () => {
    if (!hadController) return;
    reloadForUpdatedShell("controllerchange");
  });
  navigator.serviceWorker.addEventListener("message", (event) => {
    const message = event.data || {};
    if (message.type === "iowb_app_updated" && message.version !== APP_VERSION) {
      reloadForUpdatedShell("app-updated");
    }
  });

  const register = async () => {
    try {
      const registration = await navigator.serviceWorker.register(`/sw.js?v=${APP_VERSION}`);
      if (registration.waiting) {
        registration.waiting.postMessage({ type: "iowb_skip_waiting" });
      }
      registration.addEventListener("updatefound", () => {
        const worker = registration.installing;
        worker?.addEventListener("statechange", () => {
          if (worker.state === "installed" && navigator.serviceWorker.controller) {
            worker.postMessage({ type: "iowb_skip_waiting" });
          }
        });
      });
    } catch {
      // The app still works without the service worker.
    }
  };
  if (document.readyState === "complete") {
    register();
  } else {
    window.addEventListener("load", register, { once: true });
  }
}

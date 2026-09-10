let gitCommitModalReturnFocus = null;
let gitCommitModalSubmitting = false;

function closeGitCommitModal({ restoreFocus = true, force = false } = {}) {
  const modal = qs("#git-commit-modal");
  if (!modal || (gitCommitModalSubmitting && !force)) return;
  const input = modal.querySelector("#git-message");
  if (input) state.gitCommitMessage = input.value;
  modal.remove();
  const returnFocus = gitCommitModalReturnFocus;
  gitCommitModalReturnFocus = null;
  if (restoreFocus && returnFocus?.isConnected && !returnFocus.disabled) {
    window.requestAnimationFrame(() => returnFocus.focus({ preventScroll: true }));
  }
}

function updateGitCommitSubmitState(input, submitButton) {
  if (!input || !submitButton) return;
  submitButton.disabled = !input.value.trim() || gitCommitModalSubmitting;
}

function setGitCommitModalBusy(modal, busy) {
  if (!modal) return;
  modal.classList.toggle("is-busy", busy);
  modal.querySelectorAll("[data-git-commit-close], [data-git-commit-generate]").forEach((button) => {
    button.disabled = busy;
  });
  const input = modal.querySelector("#git-message");
  if (input) input.readOnly = busy;
}

function trapGitCommitModalFocus(event, dialog) {
  if (event.key !== "Tab") return;
  const focusable = [...dialog.querySelectorAll(
    'button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
  )].filter((element) => !element.hidden);
  if (!focusable.length) return;
  const first = focusable[0];
  const last = focusable.at(-1);
  if (!dialog.contains(document.activeElement)) {
    event.preventDefault();
    (event.shiftKey ? last : first).focus();
    return;
  }
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}

async function submitGitCommitModal() {
  if (gitCommitModalSubmitting) return false;
  const modal = qs("#git-commit-modal");
  const input = modal?.querySelector("#git-message");
  const message = input?.value.trim() || "";
  if (!message) {
    input?.focus();
    input?.setCustomValidity("Enter a commit message.");
    input?.reportValidity();
    return false;
  }
  if (!selectedGitFiles().length) {
    showToast("Select at least one changed file before committing.", "warn");
    return false;
  }
  input?.setCustomValidity("");
  state.gitCommitMessage = message;
  gitCommitModalSubmitting = true;
  setGitCommitModalBusy(modal, true);
  try {
    const committed = await commitGitSelection();
    if (!committed) return false;
    closeGitCommitModal({ force: true });
    return true;
  } finally {
    gitCommitModalSubmitting = false;
    setGitCommitModalBusy(modal, false);
  }
}

function openGitCommitModal({ generateOnOpen = false } = {}) {
  const files = selectedGitFiles();
  if (!files.length) {
    showToast("Select at least one changed file before committing.", "warn");
    return;
  }
  closeGitCommitModal({ restoreFocus: false, force: true });
  gitCommitModalReturnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const message = state.gitCommitMessage || "";
  document.body.insertAdjacentHTML("beforeend", `<div id="git-commit-modal" class="git-commit-modal">
    <section class="git-commit-dialog" role="dialog" aria-modal="true" aria-labelledby="git-commit-title" aria-describedby="git-commit-message-help">
      <header>
        <div>
          <h3 id="git-commit-title">Commit Changes</h3>
          <span class="meta">${files.length} file${files.length === 1 ? "" : "s"} selected</span>
        </div>
        <button type="button" class="icon-button" data-git-commit-close aria-label="Close" title="Close" data-symbol="close"></button>
      </header>
      <form class="git-commit-form">
        <label class="git-commit-message-field" for="git-message">
          <span>Commit message</span>
          <textarea id="git-message" name="message" rows="4" required spellcheck="true" autocapitalize="sentences" autocomplete="off" placeholder="Describe the change you are committing"></textarea>
        </label>
        <p id="git-commit-message-help" class="git-commit-message-help" data-git-commit-ai-status aria-live="polite">AI drafts a message from the selected changes. Review it before committing. Ctrl/⌘ + Enter commits.</p>
        <div class="button-row git-commit-actions">
          <button type="button" class="icon-button git-commit-generate" data-git-commit-generate aria-label="Generate commit message with AI" title="Generate commit message with AI" data-symbol="sparkles">Generate with AI</button>
          <span class="grow"></span>
          <button type="button" data-git-commit-close>Cancel</button>
          <button type="submit" class="primary-action" data-git-commit-submit>Commit</button>
        </div>
      </form>
    </section>
  </div>`);
  const modal = qs("#git-commit-modal");
  const dialog = modal?.querySelector(".git-commit-dialog");
  const form = modal?.querySelector(".git-commit-form");
  const input = modal?.querySelector("#git-message");
  const generateButton = modal?.querySelector("[data-git-commit-generate]");
  const submitButton = modal?.querySelector("[data-git-commit-submit]");
  const aiStatus = modal?.querySelector("[data-git-commit-ai-status]");
  input.value = message;
  updateGitCommitSubmitState(input, submitButton);
  input?.focus();
  input?.setSelectionRange(input.value.length, input.value.length);
  input?.addEventListener("input", () => {
    input.setCustomValidity("");
    updateGitCommitSubmitState(input, submitButton);
  });
  modal?.addEventListener("click", (event) => {
    if (event.target === modal) closeGitCommitModal();
  });
  modal?.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      closeGitCommitModal();
      return;
    }
    trapGitCommitModalFocus(event, dialog);
  });
  modal?.querySelectorAll("[data-git-commit-close]").forEach((button) => {
    button.addEventListener("click", closeGitCommitModal);
  });
  const generate = () => withButtonLoading(generateButton, async () => {
    if (aiStatus) aiStatus.textContent = "Generating an AI draft from the selected changes…";
    await generateGitMessage();
    if (!input?.isConnected) return;
    input.value = state.gitCommitMessage || "";
    input.focus();
    input.select();
    updateGitCommitSubmitState(input, submitButton);
    if (aiStatus) aiStatus.textContent = "AI draft ready. Review it before committing.";
  }).catch((error) => {
    if (aiStatus) aiStatus.textContent = "AI could not generate a draft. You can still write a message.";
    showError(error);
  });
  generateButton?.addEventListener("click", generate);
  form?.addEventListener("submit", (event) => {
    event.preventDefault();
    withButtonLoading(submitButton, submitGitCommitModal)
      .finally(() => updateGitCommitSubmitState(input, submitButton))
      .catch(showError);
  });
  input?.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      form?.requestSubmit();
    }
  });
  if (generateOnOpen) generate();
}

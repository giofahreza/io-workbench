function closeGitCommitModal() {
  state.gitCommitMessage = qs("#git-commit-message-input")?.value || state.gitCommitMessage || "";
  qs("#git-commit-modal")?.remove();
}

function openGitCommitModal() {
  const files = selectedGitFiles();
  if (!files.length) return;
  qs("#git-commit-modal")?.remove();
  const message = escapeHtml(state.gitCommitMessage || qs("#git-message")?.value || "");
  document.body.insertAdjacentHTML("beforeend", `<div id="git-commit-modal" class="git-commit-modal">
    <section class="git-commit-dialog" role="dialog" aria-modal="true" aria-labelledby="git-commit-title">
      <header>
        <div>
          <h3 id="git-commit-title">Commit Changes</h3>
          <span class="meta">${files.length} file${files.length === 1 ? "" : "s"} selected</span>
        </div>
        <button type="button" class="icon-button" data-git-commit-close aria-label="Close" title="Close" data-symbol="close"></button>
      </header>
      <textarea id="git-commit-message-input" placeholder="Message (Ctrl+Enter to commit)">${message}</textarea>
      <div class="button-row">
        <button type="button" class="icon-button" data-git-commit-generate aria-label="Generate commit message" title="Generate commit message" data-symbol="sparkles"></button>
        <span class="grow"></span>
        <button type="button" data-git-commit-close>Cancel</button>
        <button type="button" class="primary-action" data-git-commit-submit>Commit</button>
      </div>
    </section>
  </div>`);
  const modal = qs("#git-commit-modal");
  const input = qs("#git-commit-message-input");
  input?.focus();
  input?.setSelectionRange(input.value.length, input.value.length);
  modal.addEventListener("click", (event) => {
    if (event.target === modal) closeGitCommitModal();
  });
  modal.querySelectorAll("[data-git-commit-close]").forEach((button) => {
    button.addEventListener("click", closeGitCommitModal);
  });
  modal.querySelector("[data-git-commit-generate]")?.addEventListener("click", (event) => {
    withButtonLoading(event.currentTarget, async () => {
      await generateGitMessage();
      input.value = qs("#git-message").value;
      state.gitCommitMessage = input.value;
    }).catch(showError);
  });
  modal.querySelector("[data-git-commit-submit]")?.addEventListener("click", (event) => {
    withButtonLoading(event.currentTarget, async () => {
      state.gitCommitMessage = input.value.trim();
      qs("#git-message").value = state.gitCommitMessage;
      await commitGitSelection();
      closeGitCommitModal();
    }).catch(showError);
  });
  input?.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closeGitCommitModal();
    } else if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      state.gitCommitMessage = input.value.trim();
      qs("#git-message").value = state.gitCommitMessage;
      commitGitSelection().then(closeGitCommitModal).catch(showError);
    }
  });
}

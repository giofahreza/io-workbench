const menu = document.querySelector(".menu");

if (menu) {
  const summary = menu.querySelector("summary");

  for (const link of menu.querySelectorAll("a")) {
    link.addEventListener("click", () => {
      menu.open = false;
    });
  }

  document.addEventListener("click", (event) => {
    if (menu.open && !menu.contains(event.target)) menu.open = false;
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && menu.open) {
      menu.open = false;
      summary?.focus();
    }
  });
}

const copyStatus = document.getElementById("copy-status");

function fallbackCopy(text) {
  const input = document.createElement("textarea");
  input.value = text;
  input.setAttribute("readonly", "");
  input.style.position = "fixed";
  input.style.opacity = "0";
  document.body.append(input);
  input.select();
  const copied = document.execCommand("copy");
  input.remove();
  if (!copied) throw new Error("Copy command failed");
}

async function copyText(text) {
  if (navigator.clipboard && window.isSecureContext) {
    await navigator.clipboard.writeText(text);
    return;
  }

  fallbackCopy(text);
}

for (const button of document.querySelectorAll("[data-copy]")) {
  button.addEventListener("click", async () => {
    const command = button.dataset.copy;
    if (!command) return;

    const originalLabel = button.textContent;
    const label = button.dataset.copyLabel || "Command";

    try {
      await copyText(command);
      button.dataset.copyState = "copied";
      button.textContent = "Copied";
      if (copyStatus) copyStatus.textContent = `${label} copied to the clipboard.`;

      window.setTimeout(() => {
        button.dataset.copyState = "";
        button.textContent = originalLabel;
      }, 1800);
    } catch {
      if (copyStatus) {
        copyStatus.textContent = `Unable to copy the ${label.toLowerCase()}. Select it from the command block instead.`;
      }
    }
  });
}

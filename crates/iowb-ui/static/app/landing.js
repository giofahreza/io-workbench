const themeStorageKey = "iowb.landing.theme";
const darkThemeColor = "#101715";
const lightThemeColor = "#f5f7f5";
const root = document.documentElement;
const themeToggle = document.querySelector("[data-theme-toggle]");
const systemTheme = window.matchMedia("(prefers-color-scheme: dark)");

function savedTheme() {
  try {
    const theme = window.localStorage.getItem(themeStorageKey);
    return theme === "light" || theme === "dark" ? theme : null;
  } catch {
    return null;
  }
}

function systemPreferredTheme() {
  return systemTheme.matches ? "dark" : "light";
}

function applyTheme(theme) {
  const nextTheme = theme === "dark" ? "dark" : "light";
  root.dataset.theme = nextTheme;
  document.querySelector('meta[name="theme-color"]')?.setAttribute(
    "content",
    nextTheme === "dark" ? darkThemeColor : lightThemeColor,
  );

  if (!themeToggle) return;
  const nextThemeLabel = nextTheme === "dark" ? "light" : "dark";
  themeToggle.setAttribute("aria-label", "Switch to " + nextThemeLabel + " theme");
  themeToggle.setAttribute("aria-pressed", String(nextTheme === "dark"));
  themeToggle.title = "Switch to " + nextThemeLabel + " theme";
}

applyTheme(savedTheme() ?? systemPreferredTheme());

themeToggle?.addEventListener("click", () => {
  const nextTheme = root.dataset.theme === "dark" ? "light" : "dark";
  try {
    window.localStorage.setItem(themeStorageKey, nextTheme);
  } catch {
    // The current visit still uses the selected theme when storage is unavailable.
  }
  applyTheme(nextTheme);
});

function followSystemTheme(event) {
  if (!savedTheme()) applyTheme(event.matches ? "dark" : "light");
}

if (systemTheme.addEventListener) {
  systemTheme.addEventListener("change", followSystemTheme);
} else {
  systemTheme.addListener(followSystemTheme);
}

for (const button of document.querySelectorAll("[data-copy]")) {
  button.addEventListener("click", async () => {
    const command = button.dataset.copy;
    const status = button.parentElement?.querySelector(".copy-status");
    if (!command || !status) return;

    try {
      await navigator.clipboard.writeText(command);
      status.textContent = "Copied";
      button.textContent = "Copied";
    } catch {
      status.textContent = "Select and copy the command";
    }

    window.setTimeout(() => {
      status.textContent = "";
      button.textContent = "Copy commands";
    }, 1800);
  });
}

for (const link of document.querySelectorAll(".mobile-menu a")) {
  link.addEventListener("click", () => {
    link.closest("details")?.removeAttribute("open");
  });
}

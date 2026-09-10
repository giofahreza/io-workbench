(() => {
  const storageKey = "iowb.landing.theme";
  const root = document.documentElement;
  const prefersDark = window.matchMedia?.("(prefers-color-scheme: dark)");

  function readStoredTheme() {
    try {
      const value = window.localStorage.getItem(storageKey);
      return value === "light" || value === "dark" ? value : null;
    } catch {
      return null;
    }
  }

  function writeStoredTheme(theme) {
    try {
      window.localStorage.setItem(storageKey, theme);
    } catch {
      // Persistence is optional when browser storage is unavailable.
    }
  }

  function resolvedTheme() {
    if (root.dataset.theme === "light" || root.dataset.theme === "dark") {
      return root.dataset.theme;
    }

    return prefersDark?.matches ? "dark" : "light";
  }

  function updateThemeColor() {
    const meta = document.querySelector('meta[name="theme-color"]');
    if (!meta) return;

    const light = meta.dataset.themeColorLight || "#15235d";
    const dark = meta.dataset.themeColorDark || "#070b1f";
    meta.setAttribute("content", resolvedTheme() === "dark" ? dark : light);
  }

  function updateControls() {
    const activeTheme = resolvedTheme();
    const nextTheme = activeTheme === "dark" ? "light" : "dark";

    for (const button of document.querySelectorAll("[data-theme-toggle]")) {
      button.setAttribute("aria-pressed", String(activeTheme === "dark"));
      button.setAttribute("aria-label", `Switch to ${nextTheme} theme`);
      button.title = `Switch to ${nextTheme} theme`;

      const label = button.querySelector("[data-theme-label]");
      if (label) label.textContent = activeTheme === "dark" ? "Dark" : "Light";
    }
  }

  function refresh() {
    updateThemeColor();
    updateControls();
  }

  function installControls() {
    for (const button of document.querySelectorAll("[data-theme-toggle]")) {
      if (button.dataset.themeBound === "true") continue;

      button.dataset.themeBound = "true";
      button.addEventListener("click", () => {
        const nextTheme = resolvedTheme() === "dark" ? "light" : "dark";
        root.dataset.theme = nextTheme;
        writeStoredTheme(nextTheme);
        refresh();
      });
    }

    refresh();
  }

  let loaderReleased = false;

  function releaseBrandLoader() {
    if (loaderReleased) return;
    loaderReleased = true;
    root.classList.add("brand-ready");

    const loader = document.querySelector("[data-brand-loader]");
    if (!loader) return;
    loader.setAttribute("aria-hidden", "true");
    window.setTimeout(() => loader.remove(), 380);
  }

  function installBrandLoader() {
    const releaseAfterLoad = () => window.setTimeout(releaseBrandLoader, 120);
    if (document.readyState === "complete") {
      releaseAfterLoad();
    } else {
      window.addEventListener("load", releaseAfterLoad, { once: true });
    }

    // A stalled third-party request should never leave the public site inert.
    window.setTimeout(releaseBrandLoader, 4800);
  }

  const savedTheme = readStoredTheme();
  if (savedTheme) root.dataset.theme = savedTheme;
  updateThemeColor();

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", installControls, { once: true });
  } else {
    installControls();
  }

  installBrandLoader();

  const handleSystemThemeChange = () => {
    if (!root.dataset.theme) refresh();
  };

  if (prefersDark?.addEventListener) {
    prefersDark.addEventListener("change", handleSystemThemeChange);
  } else if (prefersDark?.addListener) {
    prefersDark.addListener(handleSystemThemeChange);
  }
})();

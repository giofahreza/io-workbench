(() => {
  const storageKey = "iowb.landing.theme";

  try {
    const savedTheme = window.localStorage.getItem(storageKey);
    const theme = savedTheme === "light" || savedTheme === "dark"
      ? savedTheme
      : window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light";

    document.documentElement.dataset.theme = theme;
  } catch {
    if (window.matchMedia("(prefers-color-scheme: dark)").matches) {
      document.documentElement.dataset.theme = "dark";
    }
  }
})();

"use strict";

const TOKEN_STORAGE_KEY = "iowb.token";
const APP_VERSION = "20260910-07";
const APP_MODULES = Object.freeze([
  "/app/core.js",
  "/app/sidebar.js",
  "/app/chat/prompt-history.js",
  "/app/chat/drafts.js",
  "/app/chat/history.js",
  "/app/chat/recovery.js",
  "/app/chat/stream.js",
  "/app/chat/settings.js",
  "/app/workspace/files.js",
  "/app/workspace/git/status.js",
  "/app/workspace/git/commit.js",
  "/app/workspace/git/chat_composer.js",
  "/vendor/markdown-it/markdown-it.min.js",
  "/app/workspace/git/markdown.js",
  "/app/workspace/git/session_actions.js",
  "/app/workspace/git/conflicts.js",
  "/app/workspace/git/diff.js",
  "/app/workspace/git/history.js",
  "/app/workspace/database.js",
  "/app/workspace/shell.js",
  "/app/workspace/settings.js",
  "/app/workspace/websocket.js",
  "/app/navigation.js",
  "/app/board.js",
  "/app/commands.js",
  "/app/forms.js",
  "/app/startup.js",
]);

try {
  window.localStorage.removeItem(TOKEN_STORAGE_KEY);
} catch {
  // Local storage is optional; token handling uses sessionStorage.
}

let brandLoaderReleased = false;

function releaseBrandLoader() {
  if (brandLoaderReleased) return;
  brandLoaderReleased = true;
  document.documentElement.classList.add("brand-ready");

  const loader = document.querySelector("[data-brand-loader]");
  if (!loader) return;
  loader.setAttribute("aria-hidden", "true");
  window.setTimeout(() => loader.remove(), 380);
}

window.addEventListener("iowb:app-ready", releaseBrandLoader, { once: true });
window.setTimeout(releaseBrandLoader, 7000);

function loadAppModule(path) {
  return new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = `${path}?v=${encodeURIComponent(APP_VERSION)}`;
    script.async = false;
    script.addEventListener("load", resolve, { once: true });
    script.addEventListener(
      "error",
      () => reject(new Error(`Unable to load application module: ${path}`)),
      { once: true },
    );
    document.head.append(script);
  });
}

async function loadApplication() {
  try {
    for (const path of APP_MODULES) {
      await loadAppModule(path);
    }
  } catch (error) {
    console.error("[io-workbench] application startup failed", error);
    document.body.classList.remove("auth-pending");
    const summary = document.querySelector("#server-summary");
    if (summary) summary.textContent = error.message;
    releaseBrandLoader();
  }
}

loadApplication();

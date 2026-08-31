"use strict";

// Legacy entrypoint: keep direct links working while the main loader uses
// the explicit feature files below.
const GIT_MODULES = Object.freeze([
  "/app/workspace/git/status.js",
  "/app/workspace/git/commit.js",
  "/app/workspace/git/chat_composer.js",
  "/app/workspace/git/markdown.js",
  "/app/workspace/git/session_actions.js",
  "/app/workspace/git/conflicts.js",
  "/app/workspace/git/diff.js",
  "/app/workspace/git/history.js",
]);

function loadGitModule(path) {
  return new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = `${path}?v=${encodeURIComponent("20260830-01")}`;
    script.async = false;
    script.addEventListener("load", resolve, { once: true });
    script.addEventListener(
      "error",
      () => reject(new Error(`Unable to load Git module: ${path}`)),
      { once: true },
    );
    document.head.append(script);
  });
}

(async () => {
  for (const path of GIT_MODULES) await loadGitModule(path);
})();

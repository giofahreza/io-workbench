const APP_VERSION = "20260830-01";
const CACHE_NAME = `io-workbench-web-${APP_VERSION}`;
const CORE_ASSETS = [
  "/",
  "/index.html",
  `/styles.css?v=${APP_VERSION}`,
  `/app.js?v=${APP_VERSION}`,
  `/styles/base.css?v=${APP_VERSION}`,
  `/styles/sidebar.css?v=${APP_VERSION}`,
  `/styles/layout.css?v=${APP_VERSION}`,
  `/styles/workspace.css?v=${APP_VERSION}`,
  `/styles/chat.css?v=${APP_VERSION}`,
  `/styles/shell-and-git.css?v=${APP_VERSION}`,
  `/styles/output.css?v=${APP_VERSION}`,
  `/styles/board.css?v=${APP_VERSION}`,
  `/styles/responsive/large.css?v=${APP_VERSION}`,
  `/styles/responsive/mobile.css?v=${APP_VERSION}`,
  `/styles/responsive/touch-and-small.css?v=${APP_VERSION}`,
  `/app/core.js?v=${APP_VERSION}`,
  `/app/sidebar.js?v=${APP_VERSION}`,
  `/app/chat/prompt-history.js?v=${APP_VERSION}`,
  `/app/chat/drafts.js?v=${APP_VERSION}`,
  `/app/chat/history.js?v=${APP_VERSION}`,
  `/app/chat/recovery.js?v=${APP_VERSION}`,
  `/app/chat/stream.js?v=${APP_VERSION}`,
  `/app/chat/settings.js?v=${APP_VERSION}`,
  `/app/workspace/files.js?v=${APP_VERSION}`,
  `/app/workspace/git.js?v=${APP_VERSION}`,
  `/app/workspace/git/status.js?v=${APP_VERSION}`,
  `/app/workspace/git/commit.js?v=${APP_VERSION}`,
  `/app/workspace/git/chat_composer.js?v=${APP_VERSION}`,
  `/app/workspace/git/markdown.js?v=${APP_VERSION}`,
  `/app/workspace/git/session_actions.js?v=${APP_VERSION}`,
  `/app/workspace/git/conflicts.js?v=${APP_VERSION}`,
  `/app/workspace/git/diff.js?v=${APP_VERSION}`,
  `/app/workspace/git/history.js?v=${APP_VERSION}`,
  `/app/workspace/database.js?v=${APP_VERSION}`,
  `/app/workspace/shell.js?v=${APP_VERSION}`,
  `/app/workspace/settings.js?v=${APP_VERSION}`,
  `/app/workspace/websocket.js?v=${APP_VERSION}`,
  `/app/navigation.js?v=${APP_VERSION}`,
  `/app/board.js?v=${APP_VERSION}`,
  `/app/commands.js?v=${APP_VERSION}`,
  `/app/forms.js?v=${APP_VERSION}`,
  `/app/startup.js?v=${APP_VERSION}`,
  "/manifest.webmanifest",
  "/icon.svg",
  "/icons/codex.svg",
  "/icons/codex-white.svg",
  "/icons/claude-ai-icon.svg",
  "/icons/claude-white.svg",
  "/icons/gemini-ai-icon.svg"
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches.open(CACHE_NAME)
      .then((cache) => cache.addAll(CORE_ASSETS))
      .then(() => self.skipWaiting())
  );
});

self.addEventListener("message", (event) => {
  if (event.data?.type === "iowb_skip_waiting") {
    self.skipWaiting();
  }
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((key) => key !== CACHE_NAME).map((key) => caches.delete(key))))
      .then(() => self.clients.claim())
      .then(() => self.clients.matchAll({ type: "window", includeUncontrolled: true }))
      .then((clients) => clients.forEach((client) => {
        const url = new URL(client.url);
        if (url.origin !== self.location.origin || url.pathname === "/clear-cache.html") return;
        client.postMessage({ type: "iowb_app_updated", version: APP_VERSION });
      }))
  );
});

self.addEventListener("fetch", (event) => {
  const request = event.request;
  if (request.method !== "GET") return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;
  if (url.pathname.startsWith("/api/") || url.pathname === "/ws") return;

  event.respondWith(
    fetch(request)
      .then((response) => {
        const copy = response.clone();
        caches.open(CACHE_NAME).then((cache) => cache.put(request, copy));
        return response;
      })
      .catch(() => caches.match(request).then((cached) => {
        if (cached) return cached;
        if (request.mode === "navigate") return caches.match("/");
        return Response.error();
      }))
  );
});

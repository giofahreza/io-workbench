const APP_VERSION = "20260819-01";
const CACHE_NAME = `io-workbench-web-${APP_VERSION}`;
const CORE_ASSETS = [
  "/",
  "/index.html",
  `/styles.css?v=${APP_VERSION}`,
  `/app.js?v=${APP_VERSION}`,
  "/manifest.webmanifest",
  "/icon.svg",
  "/icons/codex.svg",
  "/icons/codex-white.svg",
  "/icons/claude-ai-icon.svg",
  "/icons/claude-white.svg",
  "/icons/gemini-ai-icon.svg",
  "/icons/cursor.svg",
  "/icons/cursor-white.svg"
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

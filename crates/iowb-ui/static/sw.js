const CACHE_NAME = "io-workbench-web-v48";
const CORE_ASSETS = [
  "/",
  "/index.html",
  "/styles.css?v=20260727-06",
  "/app.js?v=20260727-06",
  "/manifest.webmanifest",
  "/openapi.json",
  "/icon.svg",
  "/api-docs.html",
  "/clear-cache.html",
  "/vendor/codemirror/codemirror.css",
  "/vendor/codemirror/codemirror.js",
  "/vendor/codemirror/addon/matchbrackets.js",
  "/vendor/codemirror/addon/closebrackets.js",
  "/vendor/codemirror/addon/simple.js",
  "/vendor/codemirror/mode/css.js",
  "/vendor/codemirror/mode/gfm.js",
  "/vendor/codemirror/mode/htmlmixed.js",
  "/vendor/codemirror/mode/javascript.js",
  "/vendor/codemirror/mode/markdown.js",
  "/vendor/codemirror/mode/python.js",
  "/vendor/codemirror/mode/rust.js",
  "/vendor/codemirror/mode/shell.js",
  "/vendor/codemirror/mode/sql.js",
  "/vendor/codemirror/mode/toml.js",
  "/vendor/codemirror/mode/xml.js",
  "/vendor/codemirror/mode/yaml.js",
  "/vendor/xterm/xterm.css",
  "/vendor/xterm/xterm.js"
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches.open(CACHE_NAME)
      .then((cache) => cache.addAll(CORE_ASSETS))
      .then(() => self.skipWaiting())
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((key) => key !== CACHE_NAME).map((key) => caches.delete(key))))
      .then(() => self.clients.claim())
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

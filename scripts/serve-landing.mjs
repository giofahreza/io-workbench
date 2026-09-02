import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const staticDirectory = join(scriptDirectory, "..", "crates", "iowb-ui", "static");
const host = process.env.LANDING_HOST ?? "127.0.0.1";
const port = Number.parseInt(process.env.LANDING_PORT ?? "8102", 10);

if (!Number.isSafeInteger(port) || port < 1 || port > 65535) {
  throw new Error("LANDING_PORT must be an integer between 1 and 65535");
}

const assets = new Map([
  ["/", ["landing.html", "text/html; charset=utf-8", "no-store"]],
  ["/landing", ["landing.html", "text/html; charset=utf-8", "no-store"]],
  ["/landing/", ["landing.html", "text/html; charset=utf-8", "no-store"]],
  ["/landing.html", ["landing.html", "text/html; charset=utf-8", "no-store"]],
  ["/docs/", ["docs/index.html", "text/html; charset=utf-8", "no-store"]],
  ["/docs.html", ["docs/index.html", "text/html; charset=utf-8", "no-store"]],
  ["/docs/search-index.json", ["docs/search-index.json", "application/json; charset=utf-8", "public, max-age=300"]],
  ["/styles/landing.css", ["styles/landing.css", "text/css; charset=utf-8", "public, max-age=300"]],
  ["/styles/docs.css", ["styles/docs.css", "text/css; charset=utf-8", "public, max-age=300"]],
  ["/app/landing-theme.js", ["app/landing-theme.js", "application/javascript; charset=utf-8", "public, max-age=300"]],
  ["/app/landing.js", ["app/landing.js", "application/javascript; charset=utf-8", "public, max-age=300"]],
  ["/app/docs.js", ["app/docs.js", "application/javascript; charset=utf-8", "public, max-age=300"]],
  ["/openapi.json", ["openapi.json", "application/json; charset=utf-8", "public, max-age=300"]],
  ["/icon.svg", ["icon.svg", "image/svg+xml", "public, max-age=300"]],
  ["/icons/codex-white.svg", ["icons/codex-white.svg", "image/svg+xml", "public, max-age=300"]],
  ["/icons/claude-white.svg", ["icons/claude-white.svg", "image/svg+xml", "public, max-age=300"]],
  ["/icons/gemini-ai-icon.svg", ["icons/gemini-ai-icon.svg", "image/svg+xml", "public, max-age=300"]],
]);

for (const topic of [
  "quick-start",
  "install-and-update",
  "web-workspace",
  "mobile",
  "desktop-client",
  "remote-access",
  "agents-and-sessions",
  "projects-files-git",
  "database-workspace",
  "terminal-and-mobile",
  "boards-and-tools",
  "security-and-boundaries",
  "configuration",
  "deployment-and-recovery",
  "settings-and-integrations",
  "troubleshooting",
  "api",
]) {
  const page = "docs/" + topic + "/index.html";
  assets.set("/docs/" + topic, [page, "text/html; charset=utf-8", "no-store"]);
  assets.set("/docs/" + topic + "/", [page, "text/html; charset=utf-8", "no-store"]);
}

for (const category of ["get-started", "workbench", "operations", "reference"]) {
  const page = "docs/category/" + category + "/index.html";
  assets.set("/docs/category/" + category, [page, "text/html; charset=utf-8", "no-store"]);
  assets.set("/docs/category/" + category + "/", [page, "text/html; charset=utf-8", "no-store"]);
}

for (const topic of [
  "web-workspace",
  "mobile-clients",
  "native-desktop-client",
  "agents-and-providers",
  "projects-and-git",
  "database-and-terminal",
  "remote-access-and-security",
  "operations-and-api",
]) {
  const page = "docs/topic/" + topic + "/index.html";
  assets.set("/docs/topic/" + topic, [page, "text/html; charset=utf-8", "no-store"]);
  assets.set("/docs/topic/" + topic + "/", [page, "text/html; charset=utf-8", "no-store"]);
}

const securityHeaders = {
  "Content-Security-Policy":
    "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; img-src 'self' data:; script-src 'self'; style-src 'self'",
  "Referrer-Policy": "strict-origin-when-cross-origin",
  "X-Content-Type-Options": "nosniff",
  "X-Frame-Options": "DENY",
};

function sendText(response, status, body, headers = {}) {
  const bytes = Buffer.from(body);
  response.writeHead(status, {
    ...securityHeaders,
    "Cache-Control": "no-store",
    "Content-Length": bytes.byteLength,
    "Content-Type": "text/plain; charset=utf-8",
    ...headers,
  });
  response.end(bytes);
}

async function serve(request, response) {
  if (request.method !== "GET" && request.method !== "HEAD") {
    sendText(response, 405, "Method not allowed", { Allow: "GET, HEAD" });
    return;
  }

  const requestUrl = new URL(request.url ?? "/", "http://landing.local");
  if (requestUrl.pathname === "/docs") {
    response.writeHead(308, {
      ...securityHeaders,
      "Cache-Control": "no-store",
      Location: "/docs/" + requestUrl.search,
    });
    response.end();
    return;
  }

  if (requestUrl.pathname === "/health") {
    const body = JSON.stringify({ status: "ok", service: "io-workbench-landing" });
    if (request.method === "HEAD") {
      response.writeHead(200, {
        ...securityHeaders,
        "Cache-Control": "no-store",
        "Content-Length": Buffer.byteLength(body),
        "Content-Type": "application/json; charset=utf-8",
      });
      response.end();
      return;
    }
    response.writeHead(200, {
      ...securityHeaders,
      "Cache-Control": "no-store",
      "Content-Length": Buffer.byteLength(body),
      "Content-Type": "application/json; charset=utf-8",
    });
    response.end(body);
    return;
  }

  const asset = assets.get(requestUrl.pathname);
  if (!asset) {
    sendText(response, 404, "Not found");
    return;
  }

  const [relativePath, contentType, cacheControl] = asset;
  try {
    const body = await readFile(join(staticDirectory, relativePath));
    response.writeHead(200, {
      ...securityHeaders,
      "Cache-Control": cacheControl,
      "Content-Length": body.byteLength,
      "Content-Type": contentType,
    });
    response.end(request.method === "HEAD" ? undefined : body);
  } catch (error) {
    console.error("Failed to read landing asset", relativePath, error);
    sendText(response, 500, "Landing page asset unavailable");
  }
}

const server = createServer((request, response) => {
  void serve(request, response).catch((error) => {
    console.error("Landing request failed", error);
    if (!response.headersSent) {
      sendText(response, 500, "Internal server error");
      return;
    }
    response.end();
  });
});

server.listen({ host, port }, () => {
  console.log("io-workbench landing listening on http://" + host + ":" + port);
});

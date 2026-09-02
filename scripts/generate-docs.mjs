import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outputDirectory = join(repositoryRoot, "crates", "iowb-ui", "static", "docs");
const assetVersion = "20260901-05";
const docsVersion = "2026.09";
const updated = "September 1, 2026";
const generatorArguments = process.argv.slice(2);
const checkMode = generatorArguments.includes("--check");
const unsupportedArguments = generatorArguments.filter((argument) => argument !== "--check");

if (unsupportedArguments.length) {
  throw new Error("Unsupported argument(s): " + unsupportedArguments.join(", ") + ". Use --check to verify generated docs are current.");
}

const groups = [
  {
    title: "Get started",
    slug: "get-started",
    description: "Install a released host, open the web workspace, and connect a phone safely.",
  },
  {
    title: "Workbench workflows",
    slug: "workbench",
    description: "Use agents, code, Git, data, and terminals against one host project.",
  },
  {
    title: "Operate safely",
    slug: "operations",
    description: "Set access boundaries, configure the service, and run controlled automation.",
  },
  {
    title: "Reference",
    slug: "reference",
    description: "Diagnose common problems and navigate the bundled API reference.",
  },
];

const topicCategories = [
  {
    title: "Web workspace",
    slug: "web-workspace",
    description: "Browser setup, project selection, Chat, Files, Git, data, Shell, and review flows.",
  },
  {
    title: "Mobile clients",
    slug: "mobile-clients",
    description: "Android, browser/PWA, iPhone/iPad, connection profiles, and remote handheld controls.",
  },
  {
    title: "Native desktop client",
    slug: "native-desktop-client",
    description: "The source-built wxWidgets client for local or remote io-workbench hosts.",
  },
  {
    title: "Agents and providers",
    slug: "agents-and-providers",
    description: "Claude, Codex, Gemini, CLI readiness, prompts, sessions, and delivery automation.",
  },
  {
    title: "Projects and Git",
    slug: "projects-and-git",
    description: "Host workspaces, file review, source control, workspace creation, and Git synchronization.",
  },
  {
    title: "Database and terminal",
    slug: "database-and-terminal",
    description: "Saved database connections, SQL, row transfer, server PTYs, and validation evidence.",
  },
  {
    title: "Remote access and security",
    slug: "remote-access-and-security",
    description: "Authentication, HTTPS/WSS, VPNs, tunnels, proxy boundaries, and host authority.",
  },
  {
    title: "Operations and API",
    slug: "operations-and-api",
    description: "Configuration, deployment, recovery, integrations, diagnostics, and API-oriented workflows.",
  },
];

function escapeHtml(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function plainText(value) {
  return String(value)
    .replace(/<script[\s\S]*?<\/script>/gi, " ")
    .replace(/<style[\s\S]*?<\/style>/gi, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function slugify(value) {
  return String(value)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function pageUrl(page) {
  return "/docs/" + page.slug + "/";
}

function categoryUrl(group) {
  return "/docs/category/" + group.slug + "/";
}

function topicUrl(topic) {
  return "/docs/topic/" + topic.slug + "/";
}

function docLink(slug, label) {
  return '<a class="docs-inline-link" href="/docs/' + escapeHtml(slug) + '/">' + escapeHtml(label) + "</a>";
}

function paragraph(value) {
  return "<p>" + value + "</p>";
}

function lead(value) {
  return '<p class="docs-lead">' + value + "</p>";
}

function section(title, content, id) {
  const headingId = id || slugify(title);
  return [
    '<section id="' + escapeHtml(headingId) + '" aria-labelledby="' + escapeHtml(headingId) + '-title">',
    '<h2 id="' + escapeHtml(headingId) + '-title">' + escapeHtml(title) + "</h2>",
    content,
    "</section>",
  ].join("\n");
}

function heading(title) {
  return "<h3>" + escapeHtml(title) + "</h3>";
}

function list(items) {
  return '<ul class="docs-list">' + items.map((item) => "<li>" + item + "</li>").join("") + "</ul>";
}

function steps(items) {
  return '<ol class="docs-steps">' + items.map((item, index) => [
    "<li>",
    "<span>" + (index + 1) + "</span>",
    "<div><strong>" + escapeHtml(item[0]) + "</strong><p>" + item[1] + "</p></div>",
    "</li>",
  ].join("")).join("") + "</ol>";
}

function compactSteps(items) {
  return '<ol class="docs-steps compact">' + items.map((item, index) => [
    "<li>",
    "<span>" + (index + 1) + "</span>",
    "<div><strong>" + escapeHtml(item[0]) + "</strong><p>" + item[1] + "</p></div>",
    "</li>",
  ].join("")).join("") + "</ol>";
}

function cards(items, threeColumns) {
  const className = threeColumns ? "docs-grid docs-grid-three" : "docs-grid";
  return '<div class="' + className + '">' + items.map((item) => [
    "<article>",
    item[0] ? '<span class="docs-card-kicker">' + escapeHtml(item[0]) + "</span>" : "",
    "<h3>" + escapeHtml(item[1]) + "</h3>",
    "<p>" + item[2] + "</p>",
    "</article>",
  ].join("")).join("") + "</div>";
}

function table(headers, rows) {
  return [
    '<div class="docs-table-wrap"><table class="docs-table"><thead><tr>',
    headers.map((header) => "<th>" + escapeHtml(header) + "</th>").join(""),
    "</tr></thead><tbody>",
    rows.map((row) => "<tr>" + row.map((cell) => "<td>" + cell + "</td>").join("") + "</tr>").join(""),
    "</tbody></table></div>",
  ].join("");
}

function codeCard(value, label) {
  const source = String(value).trim();
  return [
    '<div class="docs-code-card">',
    '<div class="docs-code-head"><span>' + escapeHtml(label || "Shell") + '</span><button class="docs-copy-button" type="button" data-copy="' + escapeHtml(source) + '">Copy</button></div>',
    "<pre><code>" + escapeHtml(source) + "</code></pre>",
    '<p class="docs-copy-status" aria-live="polite"></p>',
    "</div>",
  ].join("");
}

function note(title, value, warning) {
  return '<div class="docs-note' + (warning ? " warning" : "") + '"><strong>' + escapeHtml(title) + "</strong><p>" + value + "</p></div>";
}

function pageList(items) {
  return '<ul class="docs-page-list">' + items.map((item) => [
    "<li><a href=\"" + item.href + "\">",
    "<span>" + escapeHtml(item.title) + "</span>",
    "<small>" + escapeHtml(item.summary) + "</small>",
    "</a></li>",
  ].join("")).join("") + "</ul>";
}

const pageDefinitions = [
  {
    slug: "quick-start",
    title: "Quick start",
    group: "Get started",
    type: "Tutorial",
    summary: "Install a released host or source build, create the first account, and prove a local workspace is ready.",
    keywords: "install release binary linux macos windows source rust cargo git docker first user setup account workspace health provider cli claude codex gemini",
    body: [
      lead("Install <strong>io-workbench</strong> on the machine that owns your repository, provider CLI credentials, and development tools. The browser is the control surface; the host stays the place where code, processes, and agent CLIs run."),
      section("What runs where", cards([
        ["Host", "The source of truth", "The Rust server, project directories, configured Claude/Codex/Gemini CLIs, Git credentials, databases, and PTY processes live on this machine."],
        ["Web UI", "Your first cockpit", "Open the local URL in a browser to sign in, choose a host project, start sessions, review files and Git, query data, and run a terminal."],
        ["Mobile", "A remote client", "Android and PWA clients call the same host over HTTP(S) and WebSocket. They do not run the agent CLI or terminal locally."],
      ], true)),
      section("Before you start", list([
        "<strong>Choose a release or source build</strong>: use a tagged prebuilt host when you want the supported Linux, macOS, or Windows executable; use source when you are developing io-workbench or need an unreleased change.",
        "<strong>Current stable Rust and Cargo</strong> with edition-2024 support (Rust 1.85 or newer) are required only for a source install.",
        "<strong>Git</strong> is required to clone the repository and to use workspace Git tools.",
        "<strong>Submodule access</strong>: the root clone uses HTTPS but its current submodule URLs use GitHub SSH. Make sure an SSH key/agent can access GitHub, or rewrite the relevant submodule URLs to HTTPS and run <code>git submodule sync --recursive &amp;&amp; git submodule update --init --recursive</code> if the recursive clone cannot fetch them.",
        "<strong>A host project directory</strong> should be available under the workspace policy you intend to use.",
        "<strong>Provider CLIs</strong> must be installed and authenticated on this host for each provider you plan to use. io-workbench supervises those CLIs; it does not install or authenticate them for you.",
      ])),
      section("Install a released host binary", [
        paragraph("Release tags publish checked native host packages for Linux, macOS, and Windows. The one-line installer detects the operating system and CPU, verifies the release checksum, installs the command for the current user, and never starts a service on its own."),
        codeCard("curl -fsSL https://github.com/giofahreza/io-workbench/releases/latest/download/install.sh | sh", "Linux or macOS"),
        paragraph("For the Windows command, architecture choices, manual archive installs, Android APKs, checksum verification, and update procedure, use " + docLink("install-and-update", "Install and update") + ". After installation, start the local host deliberately with <code>io-workbench start</code>. Its default address is <code>http://127.0.0.1:8787</code> and authentication remains enabled."),
      ].join("\n")),
      section("Install from source", [
        paragraph("For development or an unreleased build, clone recursively so the checkout includes its submodules, then start the server from the repository root."),
        codeCard("git clone --recurse-submodules https://github.com/giofahreza/io-workbench.git\ncd io-workbench\ncargo run -p iowb-cli --bin io-workbench -- start"),
        paragraph("The default listener is <code>127.0.0.1:8787</code>. It is deliberately local-only until you choose a remote-access boundary."),
      ].join("\n")),
      section("Open the web UI and create the first account", [
        steps([
          ["Open the local address", "Visit <code>http://127.0.0.1:8787</code> in a browser running on the host."],
          ["Complete setup mode", "When no local user exists, the server presents first-user setup. Create the account that will own this local workbench."],
          ["Keep authentication on", "Authentication is enabled by default. After setup, protected REST endpoints and WebSocket upgrades require your authenticated browser session or another supported credential."],
          ["Add a host project", "In the sidebar, choose <strong>+ Add project</strong>, browse to an existing host directory, and add it. That browser picker adds existing directories only. Create or Git-clone the directory through a trusted host workflow first, or use the separate " + docLink("desktop-client", "native desktop") + " or protected workspace-API flow when it fits your setup."],
        ]),
        note("Default storage", "Unless overridden, configuration is stored under <code>~/.io-workbench</code>, the server database is <code>~/.io-workbench/io-workbench.db</code>, and the workspace root starts at the host user home directory."),
      ].join("\n")),
      section("Verify the host before real work", [
        paragraph("Use the health endpoint to prove the running service is reachable. Use <code>status</code> separately to print the CLI's configured runtime information; it is not a substitute for a live health check."),
        codeCard("curl http://127.0.0.1:8787/health", "Runtime health"),
        codeCard("cargo run -p iowb-cli --bin io-workbench -- status", "Configuration summary"),
        list([
          "<strong>Health</strong> should report that the server is reachable.",
          "<strong>Status</strong> prints CLI configuration details; use it to inspect setup, not to prove a remote service is responding.",
          "<strong>Settings → Agents → CLI Status</strong> is a readiness hint: it checks CLI version plus configured credential material. After it looks ready, prove the provider with one small, non-destructive agent task.",
          "<strong>First project</strong> should open with its files and Git status visible before you rely on an agent session.",
        ]),
      ].join("\n")),
      section("Optional: run in a container", [
        paragraph("A container is useful when you deliberately want the host runtime isolated. The repository, credentials, tools, and PTY still live in the host or container environment—not on a phone."),
        codeCard("docker build -t io-workbench .\ndocker run --rm -p 127.0.0.1:8787:8787 -v iowb-data:/data -v \"$PWD:/workspace\" io-workbench"),
        note("Provision provider CLIs in the image", "The shipped image installs the base runtime tools, not Claude, Codex, or Gemini CLIs. Build a controlled custom image or otherwise provision the selected CLI and its credentials inside the container before expecting agent sessions to work."),
        paragraph("For a persistent deployment, keep the configuration/data volume backed up, use a service manager or orchestrator rather than a foreground shell, and re-run the health check after an image or configuration update."),
      ].join("\n")),
      section("Continue with a real workflow", [
        paragraph("Use " + docLink("install-and-update", "Install and update") + " for release packages and device-specific installers, the " + docLink("web-workspace", "web workspace guide") + " for the first browser session, " + docLink("mobile", "mobile apps") + " for a phone client, " + docLink("desktop-client", "native desktop client") + " for the wxWidgets control surface, and " + docLink("deployment-and-recovery", "Deployment and recovery") + " before making the host persistent. If the server will be reached from another device, configure " + docLink("remote-access", "remote access") + " before putting a public hostname into a client."),
      ].join("\n")),
    ].join("\n"),
  },
  {
    slug: "install-and-update",
    title: "Install and update",
    group: "Get started",
    type: "Installation guide",
    summary: "Install checked release builds on Linux, macOS, Windows, or Android and update them without starting an unintended service.",
    keywords: "install update release binary curl powershell windows macos linux android apk sha256 checksum github releases arm64 x86_64 security",
    body: [
      lead("Each version tag, such as <code>v0.1.0</code>, publishes the io-workbench host binary for Linux, macOS, and Windows plus signed Android client APKs. The host executable is the self-hosted server and Web UI; Android is an authenticated remote client. Install the package that matches the device you are preparing, then deliberately start or connect it."),
      section("Choose the right release asset", table(
        ["Device", "What the release installs", "What it does"],
        [
          ["<strong>Linux</strong>", "A <code>linux-x86_64</code> or <code>linux-aarch64</code> host archive and the <code>io-workbench</code>/<code>iowb</code> commands.", "Runs the server, embedded Web UI, provider CLIs you have separately installed, projects, databases, and PTY on this machine. Both packages are built on an Ubuntu 22.04 glibc baseline for broad Linux compatibility."],
          ["<strong>macOS</strong>", "A <code>macos-x86_64</code> (Intel) or <code>macos-aarch64</code> (Apple Silicon) host archive.", "Runs the same local or remote host. It is a command-line host package, not a notarized Finder <code>.app</code>."],
          ["<strong>Windows</strong>", "A <code>windows-x86_64</code> ZIP with <code>io-workbench.exe</code> and <code>iowb.exe</code>.", "Runs the same host from PowerShell, Windows Terminal, a service manager, or another deliberate deployment boundary."],
          ["<strong>Android</strong>", "A signed <code>android-arm64-v8a</code> APK for physical ARM phones or <code>android-x86_64</code> for a matching emulator.", "Connects to a running host. It does not run the host, provider CLIs, project files, databases, or a local Linux shell on the phone."],
        ],
      )),
      section("Linux and macOS: install the current host", [
        paragraph("Use this convenience command in a terminal. It resolves the latest GitHub Release, detects <code>x86_64</code> or <code>aarch64</code>, downloads the matching archive, checks it against that release's <code>SHA256SUMS</code>, and installs the commands into <code>~/.local/bin</code>. It does not use <code>sudo</code> and does not create or start a service."),
        codeCard("curl -fsSL https://github.com/giofahreza/io-workbench/releases/latest/download/install.sh | sh", "Linux or macOS"),
        steps([
          ["Make the command visible", "Open a new terminal. If <code>io-workbench</code> is not found, add <code>export PATH=\"$HOME/.local/bin:$PATH\"</code> to the shell startup file you use, then open a new terminal."],
          ["Start the host deliberately", "Run <code>io-workbench start</code>. It listens on <code>127.0.0.1:8787</code> by default; open that local URL and complete first-user setup. Authentication stays enabled."],
          ["Choose a remote boundary before sharing", "For another device, use the " + docLink("remote-access", "Remote access") + " guide to put the host behind a trusted LAN, VPN, authenticated tunnel, or HTTPS/WSS proxy. Do not expose a development listener directly."],
        ]),
        note("macOS security prompt", "The release is a command-line archive, not a notarized desktop application bundle. Verify the release checksum and GitHub repository before running it; follow your organization's macOS policy if Gatekeeper asks for confirmation instead of broadly disabling Gatekeeper."),
      ].join("\n")),
      section("Windows: PowerShell installer or a downloaded script", [
        paragraph("Run the following in PowerShell to install the current x86_64 host package. The installer verifies the matching release checksum, installs into <code>%LOCALAPPDATA%\\Programs\\io-workbench</code>, adds that location to the current user's PATH when possible, and prints the next command. It does not start a host or register a service."),
        codeCard("irm https://github.com/giofahreza/io-workbench/releases/latest/download/install.ps1 | iex", "PowerShell"),
        paragraph("If you prefer to download and inspect the script instead of piping it into PowerShell, use the literal <code>curl.exe</code> flow below. Run it from a writable directory, inspect the file, then execute it with your normal PowerShell policy."),
        codeCard("curl.exe -fL -o .\\install-iowb.ps1 https://github.com/giofahreza/io-workbench/releases/latest/download/install.ps1\nGet-Content .\\install-iowb.ps1\n.\\install-iowb.ps1", "PowerShell with curl.exe"),
        steps([
          ["Open a fresh terminal", "PATH changes made for the user account are picked up by a new PowerShell or Windows Terminal window. If your policy blocks local script execution, inspect the downloaded file and use the approved execution-policy process rather than weakening system-wide policy."],
          ["Start only when ready", "Run <code>io-workbench start</code>, then open <code>http://127.0.0.1:8787</code>. The default listener is local-only and the app's normal authentication flow remains enabled."],
          ["Use a service manager for persistence", "A foreground terminal is suitable for a first local check. For a host that must survive sign-out or restart, follow " + docLink("deployment-and-recovery", "Deployment and recovery") + " and configure a deliberate service identity, data path, and remote boundary."],
        ]),
      ].join("\n")),
      section("Android: install the released remote client", [
        paragraph("On a typical physical Android phone, open the ARM64 APK URL in the browser and install it with Android's package installer. The x86_64 APK is for a matching Android emulator; it is not the normal physical-phone choice."),
        codeCard("https://github.com/giofahreza/io-workbench/releases/latest/download/io-workbench-android-arm64-v8a.apk", "Physical Android phone (arm64-v8a)"),
        codeCard("https://github.com/giofahreza/io-workbench/releases/latest/download/io-workbench-android-x86_64.apk", "Android emulator (x86_64)"),
        steps([
          ["Download the matching APK", "Use the GitHub Release link in a device browser, or download it on a trusted computer and transfer it with USB/ADB. Use only the release asset for this repository and architecture."],
          ["Allow that installer source", "Android may ask you to permit installs from the browser or Files app you used. Grant the permission only for that source, complete the installation prompt, and revoke it afterward if you do not normally sideload apps."],
          ["Connect to a host", "Open the app, add the HTTPS/VPN/LAN base URL of an already-running io-workbench host, then authenticate and choose the remote project. The native terminal renders the host PTY; it is not a local Termux replacement."],
        ]),
        note("Android is a client, not the server", "Installing the APK does not install Claude, Codex, Gemini, Git credentials, a database server, project checkout, or host shell on the phone. Set those up on the Linux, macOS, or Windows machine that runs <code>io-workbench start</code>, then use the phone as its control surface.", true),
        paragraph("Android does not include a universal <code>curl</code> command, but a computer with Android Platform Tools can download and install a repeatable APK over USB debugging. Replace the example tag and choose the matching ABI. The exact versioned names are <code>io-workbench-vX.Y.Z-android-arm64-v8a.apk</code> and <code>io-workbench-vX.Y.Z-android-x86_64.apk</code>."),
        codeCard("TAG=vX.Y.Z\nASSET=io-workbench-${TAG}-android-arm64-v8a.apk\ncurl -fL -o \"$ASSET\" \"https://github.com/giofahreza/io-workbench/releases/download/${TAG}/${ASSET}\"\nadb install -r \"$ASSET\"", "Android phone from a computer with ADB"),
      ].join("\n")),
      section("Verify a manual release download", [
        paragraph("The convenience installers verify their selected archive before installing it. When downloading an archive or APK yourself, fetch <code>SHA256SUMS</code> from the same release tag and compare the named asset before executing or installing it. Do not compare a checksum copied from a chat, screenshot, or a different release."),
        codeCard("TAG=vX.Y.Z\nASSET=io-workbench-${TAG}-linux-x86_64.tar.gz\nBASE=https://github.com/giofahreza/io-workbench/releases/download/${TAG}\ncurl -fLO \"${BASE}/${ASSET}\"\ncurl -fLO \"${BASE}/SHA256SUMS\"\ngrep \" ${ASSET}$\" SHA256SUMS | sha256sum --check -", "Linux checksum example"),
        codeCard("$tag = \"vX.Y.Z\"\n$asset = \"io-workbench-$tag-windows-x86_64.zip\"\n$base = \"https://github.com/giofahreza/io-workbench/releases/download/$tag\"\ncurl.exe -fL -O \"$base/$asset\"\ncurl.exe -fL -O \"$base/SHA256SUMS\"\nGet-FileHash \".\\$asset\" -Algorithm SHA256", "Windows checksum value"),
        note("What a release checksum proves", "<code>SHA256SUMS</code> from the same GitHub Release catches corruption and selecting the wrong asset, but it shares the GitHub Release trust boundary and is not an independently signed authenticity proof. For release-build provenance, verify the downloaded file with GitHub CLI: <code>gh attestation verify &lt;downloaded-file&gt; --repo giofahreza/io-workbench</code>."),
        note("Windows comparison", "Compare the hexadecimal value from <code>Get-FileHash</code> with the line for the same asset in <code>SHA256SUMS</code>. The release installer performs that exact asset-to-checksum check automatically; manual downloads require you to make the comparison."),
      ].join("\n")),
      section("Update and roll back deliberately", table(
        ["Device", "Normal update", "If something is wrong"],
        [
          ["<strong>Linux / macOS</strong>", "Run the same installer again, then stop/restart only the host process or service you intentionally manage.", "Keep the previous archive/binary and back up <code>~/.io-workbench</code> plus projects before a production update. Use the " + docLink("deployment-and-recovery", "deployment guide") + " for a service rollback."],
          ["<strong>Windows</strong>", "Close any running host process, rerun the installer, then start the new binary when ready.", "Keep data and project directories separate from the program directory. Restore the earlier verified archive only after stopping the managed process."],
          ["<strong>Android</strong>", "Download the next APK for the same ABI and install it over the existing app. A release signed with the same key is treated as an update and preserves app data.", "If Android reports a signature mismatch, stop and verify that the APK is the official release asset; do not uninstall by default and lose local profiles before checking provenance."],
        ],
      )),
      section("Release security checklist", list([
        "<strong>Prefer a version tag</strong>: use <code>releases/latest/download</code> only when you deliberately want the newest stable release; use a versioned asset such as <code>v0.1.0</code> when you need a repeatable deployment.",
        "<strong>Understand the convenience trade-off</strong>: <code>curl | sh</code> and <code>irm | iex</code> are fast paths. Download the installer first, inspect it, and run it locally when your security policy requires review.",
        "<strong>Verify before execution</strong>: release installers verify native archives; compare <code>SHA256SUMS</code> yourself for any manual archive or APK install, and use <code>gh attestation verify</code> when you need GitHub build provenance.",
        "<strong>Do not put credentials in installer commands</strong>: provider CLIs and their authentication remain on the host after the binary install. Keep credentials in that controlled host/service environment, never in a copied installer URL or mobile screenshot.",
      ])),
      section("Continue with", paragraph("After installing a host, use " + docLink("quick-start", "Quick start") + " for first-user setup and a local health check, " + docLink("web-workspace", "Web workspace") + " for the browser workflow, " + docLink("mobile", "Mobile apps") + " for connection profiles and device behavior, and " + docLink("deployment-and-recovery", "Deployment and recovery") + " before operating a persistent public host.")),
    ].join("\n"),
  },
  {
    slug: "web-workspace",
    title: "Web workspace",
    group: "Get started",
    type: "Tutorial",
    summary: "Use the browser UI from first sign-in through a reviewable agent, code, Git, data, and terminal loop.",
    keywords: "web workspace browser ui sign in add existing project command palette database shell agent prompt config claude codex gemini files git board reconnect",
    body: [
      lead("The Web workspace is the primary remote desk for an io-workbench host. It gives one authenticated browser view to projects, configured agent CLIs, files, Git evidence, database work, and a live host terminal."),
      section("First browser session", [
        steps([
          ["Open the server URL", "Use the local address from " + docLink("quick-start", "Quick start") + ", or the HTTPS address configured by your remote-access boundary."],
          ["Sign in or finish setup", "First-user setup creates the local account. The standard password form requires a username of at least 3 characters and a password of at least 6 characters."],
          ["Add an existing host project", "In the sidebar, choose <strong>+ Add project</strong>, browse to an existing directory on the host, then add it. The browser picker does not create or clone folders. Prepare the directory through a trusted host workflow first, or use the separate native-desktop/protected-API workspace flow when appropriate."],
          ["Check the host tools", "Open <strong>Settings → Agents → CLI Status</strong>. Treat its installed/authenticated result as a readiness hint, then prove the selected provider with one small, non-destructive task before opening a paid or long-running session."],
        ]),
      ].join("\n")),
      section("Start an agent session", [
        steps([
          ["Choose the project", "Keep the code, Git state, Shell working directory, and session history tied to one project context."],
          ["Select the provider, then configure the prompt", "On an empty Chat, choose Claude, Codex, or Gemini to select the host CLI and focus the composer. Use the <strong>Prompt config</strong> (flask) button beside the composer to load compatible models and choose model, mode, or effort before sending."],
          ["Give the agent a bounded task", "State the goal, relevant paths, constraints, and validation command. A focused first task makes the code/Git review loop easier to verify."],
          ["Review or stop streamed work", "Follow the response as it arrives. While a turn is running, <strong>Send</strong> becomes <strong>Abort chat</strong>; stop it before changing a task that is no longer safe or useful."],
          ["Resume instead of restarting context", "Sessions and assistant output are persisted. Reopen a previous session to continue the project conversation rather than rebuilding context in a new tab."],
        ]),
        note("Reconnect behavior", "A dropped browser connection does not move the work to the client. The server keeps the session/process context and can replay persisted output after the browser reconnects."),
      ].join("\n")),
      section("Use the project surfaces as one delivery loop", table(
        ["Surface", "Use it in the flow", "What remains on the host"],
        [
          ["<strong>Chat</strong>", "Plan, ask, resume, inspect streamed output, and choose the provider per session.", "Configured Claude, Codex, and Gemini CLIs plus session history."],
          ["<strong>Files</strong>", "Browse, edit, upload, search, preview, and inspect the source the agent is changing.", "The selected project directory within the configured workspace boundary."],
          ["<strong>Git</strong>", "Check status and diffs, stage/commit, inspect branches/remotes, and fetch/pull/push when appropriate.", "Git repository, remotes, and credentials."],
          ["<strong>Database</strong>", "Save/test connections, inspect schema, run SQL, and review rows next to the change that affects them.", "Database connections and database access from the server."],
          ["<strong>Shell</strong>", "Run a real PTY in the selected project for test, build, migration, or operational commands.", "Shell process, working directory, output, and process controls."],
          ["<strong>Board</strong>", "Break broad delivery work into reviewable tasks, validations, and approvals.", "Board state and command-backed integrations configured on the host."],
        ],
      )),
      section("A practical first task", [
        compactSteps([
          ["Add the host project", "The browser's <strong>Sidebar → + Add project</strong> picker accepts an existing directory. Prepare one through a trusted host workflow first, then browse to it, add it, and check the displayed path before touching files or commands."],
          ["Verify the agent CLI", "Go to <strong>Settings → Agents → CLI Status</strong>, then run one small, non-destructive task with the selected Claude, Codex, or Gemini CLI before relying on it for a larger job."],
          ["Create the Chat session", "On an empty Chat, select the provider. Open <strong>Prompt config</strong> (flask) beside the composer to choose model/mode/effort, then send a bounded plan that names the expected validation command."],
          ["Inspect Files and Git", "Open <strong>Files</strong>, select a file from the tree, use Edit or Preview, then Save intentional changes. Open <strong>Git</strong>, select changed rows, review Diff/File Review, stage the intended files, enter a commit message, then use <strong>Commit Selected</strong>."],
          ["Add data evidence when needed", "Open the Command palette with <strong>Command</strong> or <strong>Ctrl/Cmd+K</strong>, select <strong>Open Database Explorer</strong>, then choose <strong>New</strong>. Fill the connection form, use <strong>Test Form</strong>, then <strong>Save Connection</strong>. Select the saved connection and use Explorer or SQL → Run Query for a narrow check."],
          ["Run Shell validation", "Select <strong>Shell</strong> to start or reuse a PTY in the chosen project, then run the relevant test, build, migration, or lint command on the host. Restart replaces that PTY; Stop kills it."],
          ["Track and close the work", "For reviewable substeps, open <strong>Board</strong>, enter a Board Prompt, choose provider/model and Git Policy (the default is Read Only), then use <strong>Add To Board</strong>. Review the backlog and explicitly start it with <strong>Start board tasks</strong>; creating a backlog does not execute it."],
        ]),
      ].join("\n")),
      section("Web UI boundaries", [
        cards([
          ["Files", "No device-local checkout", "The browser views and edits the server project through authenticated APIs. It is not an editor backed by browser storage."],
          ["Agents", "No hosted model proxy", "The selected CLI runs in your host environment using the credentials and provider setup you control."],
          ["Shell", "No browser shell", "Shell input, resize, output, abort, and process visibility target a server-hosted PTY."],
          ["Remote use", "Treat it as a control plane", "Use HTTPS/WSS and a narrow network boundary when this URL leaves the trusted local machine."],
        ]),
      ].join("\n")),
      section("Find these guides inside the app", paragraph("Open <strong>Settings → About → Product Docs</strong>, or open <strong>Sidebar → Command</strong> (or <strong>Ctrl/Cmd+K</strong>) and choose <strong>Open Product Docs</strong>.")),
      section("Next steps", paragraph("Pair the web desk with " + docLink("mobile", "mobile access") + " for a phone client, read " + docLink("agents-and-sessions", "Agents and sessions") + " for provider and recovery details, or use " + docLink("settings-and-integrations", "Settings and integrations") + " to configure the richer operational surfaces.")),
    ].join("\n"),
  },
  {
    slug: "mobile",
    title: "Mobile apps",
    group: "Get started",
    type: "Tutorial",
    summary: "Install the released Android client or build it from source, use the mobile web/PWA route, and connect it safely to a running host.",
    keywords: "mobile android native app apk release install update adb pwa ios web harness server url emulator 10.0.2.2 lan tunnel https wss remote termux",
    body: [
      lead("Mobile clients are remote control surfaces for a running io-workbench server. They do not run your agent CLIs, workspace processes, or PTY locally. Start the host first, then connect the phone to a reachable authenticated server URL."),
      section("Choose the mobile surface", table(
        ["Surface", "Best for", "Current delivery"],
        [
          ["<strong>Android native</strong>", "A dedicated mobile experience with native editor and terminal controls.", "Signed APKs are published with every <code>v*</code> GitHub Release for <code>arm64-v8a</code> and <code>x86_64</code>. You can also build from source for development."],
          ["<strong>Web harness / PWA</strong>", "Fast browser-based mobile use and the supported iOS path.", "Kotlin/JS responsive UI that can be served and installed as a PWA."],
          ["<strong>Legacy PWA fallback</strong>", "Simple compatibility testing when the newer harness is not used.", "Package <code>apps/mobile/www</code> and host it over HTTPS."],
          ["<strong>iPhone / iPad</strong>", "Safari-installed web harness/PWA.", "The supported iOS route is the HTTPS browser/PWA surface. The SwiftUI shell is legacy/experimental; do not expect Android-native terminal, Database, or profile parity."],
        ],
      )),
      section("Prepare the server before installing a client", [
        list([
          "<strong>Finish host setup</strong> with " + docLink("quick-start", "Quick start") + " and verify the selected project and provider CLIs on the server.",
          "<strong>Choose a reachable address</strong>: a LAN IP on a trusted network, a VPN address, an authenticated tunnel, or an HTTPS reverse-proxy hostname.",
          "<strong>Keep authentication enabled</strong>. Password, token, and OTP flows protect the remote client; a phone should not point at an unauthenticated public listener.",
          "<strong>Forward WebSocket traffic</strong> as well as HTTP. Live events use <code>/ws</code>, so a proxy must support HTTPS and WSS end-to-end.",
        ]),
        note("Never confuse a phone with the host", "The phone may render a terminal or editor, but commands, files, Git operations, databases, provider CLIs, and the PTY still run on the connected server.", true),
      ].join("\n")),
      section("Install the native Android app", [
        paragraph("A release tag publishes signed APKs for the two supported Android architectures. On a physical phone, use the <code>arm64-v8a</code> release; use <code>x86_64</code> only for a matching emulator. Read " + docLink("install-and-update", "Install and update") + " for the direct links, unknown-app-source permission, checksum guidance, and safe upgrade behavior."),
        steps([
          ["Pick the matching release APK", "Use <code>io-workbench-android-arm64-v8a.apk</code> from the latest release for a typical physical ARM phone, or <code>io-workbench-android-x86_64.apk</code> for an x86_64 emulator."],
          ["Install it", "Download from the repository's GitHub Release page and use Android's installer, or with USB debugging enabled run <code>adb install -r /path/to/release.apk</code>. Android may ask to permit installs from the browser or Files app used for this one install."],
          ["Launch the app", "The Android app stores server profiles, so one device can reconnect to more than one workbench host."],
        ]),
        heading("Build from source when developing the Android client"),
        paragraph("Install JDK 17, an Android SDK, and Gradle on <code>PATH</code> (or add a Gradle wrapper before using these commands). This repository does not include <code>gradlew</code>. Gradle detects the SDK through <code>ANDROID_HOME</code>, <code>ANDROID_SDK_ROOT</code>, or <code>local.properties</code>. The native Android project has a minimum SDK of 26 and produces ABI-specific debug APKs for <code>arm64-v8a</code> and <code>x86_64</code>."),
        codeCard("cd apps/mobile\ngradle mobileCheck\ngradle :androidApp:assembleDebug -Piowb.android.enabled=true\nfind androidApp/build/outputs/apk/debug -name \"*.apk\" -print"),
      ].join("\n")),
      section("Connect Android to the host", [
        steps([
          ["Add server", "Start with <strong>Add server</strong> and optionally give the profile a name."],
          ["Enter the API URL", "Use the exact base URL of the io-workbench host; omit a trailing slash when copying a public URL."],
          ["Authenticate", "Enter username and password, a server token, or an OTP code according to the host authentication mode. The client probes <code>/api/auth/status</code> and follows setup, password, token, or open flow."],
          ["Add headers only when required", "Use custom headers only when an authenticated proxy or access layer explicitly requires them."],
          ["Connect and choose a project", "After the profile connects, select the remote project, open a session, and let the client reconnect its event stream when network conditions change."],
        ]),
      ].join("\n")),
      section("Use the right URL for the device", table(
        ["Where the client runs", "Use this server URL", "Why"],
        [
          ["<strong>Android emulator</strong>", "<code>http://10.0.2.2:8787</code>", "The emulator's special host-machine address."],
          ["<strong>iPhone / iPad PWA</strong>", "<code>https://workbench.example</code>", "Install the harness over HTTPS and use an HTTPS/WSS workbench endpoint. An HTTPS PWA cannot call an HTTP host without mixed-content blocking."],
          ["<strong>Physical device on trusted LAN</strong>", "<code>http://HOST_LAN_IP:8787</code>", "Only when the host is intentionally listening on that LAN and the network is trusted."],
          ["<strong>Phone away from the host</strong>", "<code>https://workbench.example</code>", "Use VPN, authenticated tunnel, or a reverse proxy that terminates TLS and passes WSS."],
        ],
      )),
      section("Use the PWA or browser harness", [
        paragraph("For the newer Kotlin/JS harness, install Gradle on <code>PATH</code> (or add a wrapper; this repository does not include <code>gradlew</code>), then run the browser development target or build a static distribution and serve it from a URL the phone can reach. The harness is the supported iPhone/iPad route; the older SwiftUI shell is experimental."),
        codeCard("cd apps/mobile\ngradle :webHarness:jsBrowserDevelopmentRun\n# Or build a distribution:\ngradle :webHarness:jsBrowserDevelopmentExecutableDistribution\ncd webHarness/build/dist/js/developmentExecutable\npython3 -m http.server 18880 --bind 0.0.0.0"),
        note("Development server versus installed PWA", "The Python server is appropriate for local development or a trusted LAN test. For a phone-installable PWA and service-worker behavior outside that path, serve the harness through HTTPS. An HTTPS-installed harness must connect to an HTTPS/WSS workbench endpoint, not <code>http://HOST_LAN_IP</code>, to avoid mixed-content blocking."),
        steps([
          ["Open the HTTPS harness", "On iPhone or iPad, open the harness URL in Safari. On Android, use the browser when you want the harness instead of the native app."],
          ["Install it", "In Safari, use Share → <strong>Add to Home Screen</strong>. Other mobile browsers expose their own Install/Add to Home Screen action."],
          ["Connect and authenticate", "Enter the HTTPS workbench base URL, then complete its supported login/register flow and select a project/session."],
          ["Understand saved access", "The harness remembers one most-recent URL and credential/token in browser storage; it is not Android's selectable multi-profile store. Use a device lock and clear site data before handing a shared device to someone else."],
        ]),
        note("PWA is an online control surface", "The harness service worker caches the application shell, not <code>/api/</code> data or <code>/ws</code> events. An installed PWA is not an offline copy of projects, chat, terminal, or database access."),
        note("Header-protected hosts", "The native Android client can send custom access headers when a trusted proxy explicitly requires them. The web harness has no custom-header setup screen; use browser-level access or Android native for a hostname protected by service-token/header-only access layers."),
        paragraph("For the legacy fallback, run <code>cd apps/mobile &amp;&amp; ./package-pwa.sh</code>, host the unpacked <code>www</code> folder over HTTPS, then use the browser's <em>Install</em> or <em>Add to Home Screen</em> action. The legacy PWA connects with the API URL and bearer token; the newer harness can handle login/register flows."),
      ].join("\n")),
      section("Mobile capability matrix", table(
        ["Capability", "Android native", "Web harness / PWA / iOS"],
        [
          ["<strong>Chat, Files, Git, Board</strong>", "Available through the native mobile client against the remote host.", "Available through the shared Kotlin/JS browser surface."],
          ["<strong>Terminal</strong>", "Native terminal renderer built with Termux components by default; it can use a web-terminal mode when needed.", "Browser-rendered remote terminal controls; do not expect Android-native terminal parity on iOS."],
          ["<strong>Database workspace</strong>", "Has a native Database surface for connections, explorer, SQL, paged rows, and Android-specific structured row clipboard actions.", "The mobile web harness currently does not expose the Database tab. Use Android native or the full web workspace for database work."],
          ["<strong>Structured row copy/paste</strong>", "Select rows and copy/paste JSON or CSV with the native Android data grid.", "No mobile-browser row clipboard controls; the full browser database grid also does not currently expose them."],
          ["<strong>Saved servers</strong>", "Choose among named server profiles stored by the Android app.", "The harness keeps the most recently used connection in browser storage; it is not a multi-profile picker."],
        ],
      )),
      section("Use mobile in a real work loop", [
        cards([
          ["Chat", "Continue the remote session", "Open or pin a session, send a bounded request, and inspect the same persisted response history you see on the web."],
          ["Files and Git", "Review before approving", "Read and save host files, then inspect Git status/diff on the remote project rather than assuming the agent summary is complete."],
          ["Database", "Copy structured rows on Android", "Native Android can select rows and copy/paste structured JSON or CSV. The browser database grid does not yet expose row clipboard controls."],
        ["Terminal", "Control the host PTY", "Android uses a native renderer built with Termux components and handy Esc, Tab, Ctrl, Alt, and arrow keys. Long-press the Terminal navigation button → <strong>Terminal renderer</strong> to switch between Native (Termux) and xterm rendering. It is not a local Termux/Linux environment."],
        ]),
      ].join("\n")),
      section("First Android work cycle", compactSteps([
        ["Connect the saved server profile", "Choose the profile you added, confirm its URL/auth state, and wait for the project list to load from the host."],
        ["Open the project and Chat", "Select the remote project, open or create a session, choose the configured provider/model, and send a bounded request with a validation expectation."],
        ["Review Files and Git", "Read the host files changed by the task, then inspect Git status/diff before you approve or continue the request."],
        ["Validate in Terminal", "Open the remote PTY in the selected project and run the smallest relevant test, build, migration, or inspection command."],
        ["Use Database on Android when needed", "Open <strong>Database → Explorer → Add connection</strong>, enter the connection, use <strong>Test</strong>, then <strong>Save</strong>. Use Explorer or SQL for a narrow check; select/copy/paste structured rows only after confirming the target."],
        ["Record the result", "Return to Chat or Board with the diff, command result, and any data evidence so the next decision remains reviewable."],
      ])),
      section("If connection fails", list([
        "<strong>Wrong URL</strong>: test the address from the phone browser and confirm it points to io-workbench rather than a proxy login HTML page.",
        "<strong>Network boundary</strong>: verify VPN/tunnel/proxy DNS, TLS, and WebSocket forwarding for <code>/ws</code>.",
        "<strong>Authentication</strong>: confirm the selected password, token, or OTP mode and re-enter credentials after a server-side change.",
        "<strong>Host service</strong>: check <code>/health</code> and the server process before rebuilding the app.",
      ])),
    ].join("\n"),
  },
  {
    slug: "desktop-client",
    title: "Native desktop client",
    group: "Get started",
    type: "Tutorial",
    summary: "Build and use the source-built wxWidgets desktop client to control a local or remote io-workbench host.",
    keywords: "desktop native wxwidgets cmake local remote connect login setup websocket libvterm browser launcher start local github clone workspace",
    body: [
      lead("Use the native desktop client when you want a wxWidgets/Scintilla control surface for an io-workbench host. It does not move projects, provider CLIs, databases, or PTYs onto the desktop machine; those remain on the connected host."),
      section("Choose the right desktop route", table(
        ["Route", "What it is", "Use it when"],
        [
          ["<strong>Browser launcher</strong>", "<code>apps/desktop/io-workbench-desktop.sh</code> starts a local release server, waits for <code>/health</code>, opens the default browser, and shuts down that child server when the launcher exits.", "You want the normal " + docLink("web-workspace", "Web workspace") + " opened quickly on the local machine."],
          ["<strong>Native desktop client</strong>", "A separate C++/wxWidgets program that talks to a local or cloud io-workbench server over REST and WebSocket.", "You want native desktop controls for the connected host and are comfortable building the client from source."],
        ],
      )),
      section("Build the native client from source", [
        paragraph("The native project requires CMake 3.20+, a C++17 compiler, wxWidgets, libcurl, and nlohmann JSON. On Linux, install the documented packages, then build from the repository root."),
        codeCard("sudo apt install cmake g++ libwxgtk3.2-dev libcurl4-openssl-dev nlohmann-json3-dev libvterm-dev\n\ncmake -S apps/desktop/native -B apps/desktop/native/build\ncmake --build apps/desktop/native/build\n./apps/desktop/native/build/iowb-native-desktop", "Debian/Ubuntu source build"),
        list([
          "<strong>macOS</strong>: install <code>cmake wxwidgets curl nlohmann-json libvterm</code> with Homebrew, then use the same CMake commands.",
          "<strong>Windows</strong>: the CMake project has a Windows packaging target, but this repository does not document a tested package-manager prerequisite recipe. Build only after supplying compatible CMake, wxWidgets, curl, and JSON dependencies.",
          "<strong>Optional features</strong>: configure with <code>-DIOWB_DESKTOP_ENABLE_WEBSOCKET=OFF</code> or <code>-DIOWB_DESKTOP_ENABLE_LIBVTERM=OFF</code> if a dependency is unavailable.",
        ]),
      ].join("\n")),
      section("Start a local host from the desktop client", [
        codeCard("cargo build --release -p iowb-cli --bin io-workbench\nIO_WORKBENCH_DESKTOP_BIN=\"$PWD/target/release/io-workbench\" \\\n  ./apps/desktop/native/build/iowb-native-desktop", "Build host, then launch native client"),
        steps([
          ["Open the connection bar", "Start the native application, then choose <strong>Start Local</strong>."],
          ["Wait for the local host", "The client starts <code>io-workbench start --host 127.0.0.1 --port 8787</code>, polls its health endpoint, then connects to it."],
          ["Finish setup or login", "For first-user setup or a password-based host, use <strong>Login / Setup</strong>. For a token host, paste the existing bearer token into the connection-bar <strong>Token</strong> field, then choose Connect."],
          ["Select the host project", "Add, create, or select a project, then work in Chat, Files, Git, Database, Terminal, Settings, or Tools against that host."],
        ]),
        note("Use an external service for a custom or persistent host", "<strong>Start Local</strong> always targets <code>127.0.0.1:8787</code> and can stop only the child process it started. For another host/port or a service that should survive the desktop app, start io-workbench outside the client and use Connect."),
      ].join("\n")),
      section("Connect to a remote host", steps([
        ["Enter the server root URL", "Use an address such as <code>https://workbench.example</code>, not a documentation path such as <code>/docs/</code>."],
        ["Choose Connect", "The client checks host health and authentication state, then loads projects, settings, and process information after a successful connection."],
        ["Authenticate with the matching native control", "<strong>Login / Setup</strong> is a username/password dialog for setup and password-based hosts. For an existing bearer token, paste it into the connection-bar <strong>Token</strong> field, then choose Connect. The native client has no dedicated OTP field, so use Web/mobile or an already-issued bearer token for a TOTP-only deployment."],
        ["Keep transport secure", "For a remote hostname, use HTTPS; the client maps that to WSS for its live stream. Follow the " + docLink("remote-access", "Remote access") + " boundary guidance and keep server auth enabled."],
      ])),
      section("Use the desktop work loop", [
        cards([
          ["Chat", "Run project-scoped work", "Choose the host project and provider, set the prompt configuration, send a bounded task, and inspect persisted output."],
          ["Files and Git", "Make source evidence visible", "Review/edit host files and Git state before accepting or synchronizing a change."],
          ["Database and Terminal", "Validate on the host", "Inspect data, run a narrow SQL check, or operate the selected project’s server-hosted PTY."],
        ], true),
        paragraph("The native tabs are a control surface: Chat, Files, Git, Database, Settings, Terminal, and Tools all act on the connected io-workbench host rather than making a second local workspace."),
      ].join("\n")),
      section("Know the current limits", list([
        "<strong>Release host package versus native desktop client</strong>: tagged Linux/macOS/Windows packages install the <code>io-workbench</code> host server, not this separate wxWidgets executable. Build this native client from source; use " + docLink("install-and-update", "Install and update") + " when you need the released host binary.",
        "<strong>WebSocket is optional at build time</strong>: without libcurl WebSocket support, REST-backed project, file, and Git workflows remain, but live process-output streaming is unavailable.",
        "<strong>libvterm is optional at build time</strong>: without it, the terminal falls back to basic styled ANSI text instead of the richer native terminal buffer.",
        "<strong>Login persistence depends on the platform secret store</strong>: if it is unavailable, expect to log in or paste a token again after restarting the client.",
      ])),
      section("Continue with", paragraph("Use " + docLink("projects-files-git", "Projects, files, and Git") + " for review flow, " + docLink("terminal-and-mobile", "Terminal and mobile controls") + " for PTY behavior, and " + docLink("security-and-boundaries", "Security and boundaries") + " before connecting a desktop client to a remote host.")),
    ].join("\n"),
  },
  {
    slug: "remote-access",
    title: "Remote access",
    group: "Get started",
    type: "How-to guide",
    summary: "Reach an io-workbench host from another browser or phone without exposing a development listener carelessly.",
    keywords: "remote access lan host port vpn reverse proxy cloudflare tunnel https wss websocket mobile security auth",
    body: [
      lead("The default server binds to <code>127.0.0.1:8787</code>, which is ideal for first-run setup. Remote use is a network-design decision: place a VPN, authenticated tunnel, or HTTPS/WSS reverse proxy in front of the control plane before a phone or outside browser connects."),
      section("Choose a boundary", table(
        ["Boundary", "Use it when", "What to verify"],
        [
          ["<strong>VPN</strong>", "Your devices are already members of a private network.", "The VPN address reaches the host, the firewall is narrow, and the client receives an HTTPS/WSS path when needed."],
          ["<strong>HTTPS reverse proxy</strong>", "You operate a hostname and want normal browser/mobile access.", "TLS termination, proxy authentication, <code>/ws</code> upgrade support, and a trusted upstream listener."],
          ["<strong>Authenticated tunnel</strong>", "You need selective remote access without opening inbound firewall ports.", "The tunnel requires identity checks and forwards HTTP plus WebSocket traffic to the local service."],
          ["<strong>Trusted LAN</strong>", "Temporary access is limited to a private network you control.", "The listener/firewall is scoped narrowly and no public route can reach it."],
        ],
      )),
      section("Temporarily listen on a trusted LAN", [
        paragraph("Only do this when the LAN is intentionally trusted and the host firewall scope is understood. Keep authentication enabled."),
        codeCard("IO_WORKBENCH_HOST=0.0.0.0 IO_WORKBENCH_PORT=8787 cargo run -p iowb-cli --bin io-workbench -- start"),
        note("Do not publish a development listener", "Never disable authentication or bind broadly merely to make a phone connect. Use a VPN, authenticated tunnel, or reverse proxy for any route that leaves a trusted network.", true),
      ].join("\n")),
      section("Remote client checklist", compactSteps([
        ["Start locally first", "Confirm <code>curl http://127.0.0.1:8787/health</code> succeeds on the host."],
        ["Add the network boundary", "Expose only the route your devices need and terminate TLS at a controlled boundary."],
        ["Pass HTTP and WebSocket", "The web/mobile client needs regular API calls plus WSS upgrades at <code>/ws</code>."],
        ["Use the public HTTPS URL", "Enter that same URL in the browser or " + docLink("mobile", "mobile profile") + "."],
        ["Test authenticated flow", "Sign in, list a project, open a session, and verify live output before relying on remote access."],
      ])),
      section("Credentials and transport", [
        paragraph("Browser sessions, bearer tokens, and WebSocket authentication are meaningful only when the transport is protected. A proxy must preserve authorization headers and WebSocket query authentication where used. Prefer HTTPS/WSS everywhere outside a private local development path."),
      ].join("\n")),
    ].join("\n"),
  },
  {
    slug: "agents-and-sessions",
    title: "Agents and sessions",
    group: "Workbench workflows",
    type: "How-to guide",
    summary: "Run configured Claude, Codex, and Gemini CLIs with durable project sessions and reconnect-aware history.",
    keywords: "agents sessions claude codex gemini cli prompt model resume reconnect stream history token usage",
    body: [
      lead("io-workbench is a remote control plane around the agent CLIs you install and authenticate on the host. Choose the provider per session while keeping the project, files, Git state, terminal, and response history together."),
      section("Provider execution model", table(
        ["Provider", "Default host command", "Important boundary"],
        [
          ["<strong>Claude</strong>", "<code>claude --print {prompt}</code>", "The Claude CLI and its authentication live on the server host."],
          ["<strong>Codex</strong>", "<code>codex exec {prompt}</code>", "The Codex CLI and its authentication live on the server host."],
          ["<strong>Gemini</strong>", "<code>gemini --prompt {prompt}</code>", "The Gemini CLI and its authentication live on the server host."],
        ],
      )),
      section("Start a session deliberately", steps([
        ["Confirm CLI Status", "In Settings, use CLI Status as a readiness hint for the provider command and configured credential material. Do not infer readiness from a model name alone; prove one small, non-destructive task before relying on it."],
        ["Select the project first", "The session inherits the selected host project context; make sure it is the project whose files, Git state, and Shell working directory you expect."],
        ["Choose provider, then prompt configuration", "On an empty Chat, select the ready provider. It selects the CLI and focuses the composer; open <strong>Prompt config</strong> (flask) beside the composer to load compatible models and choose model, mode, or effort."],
        ["Send a bounded prompt", "Include the desired outcome, relevant paths, constraints, and how the result should be validated."],
        ["Inspect evidence", "Use Files, Git, Database, and Terminal to check the agent's work before accepting or committing it."],
      ])),
      section("Resume and recover", [
        list([
          "<strong>Persistent history</strong>: messages and session metadata are retained so a project does not become a pile of lost browser tabs.",
          "<strong>Reconnect replay</strong>: persisted assistant output can be replayed after a browser or mobile connection drops.",
          "<strong>Active work</strong>: stop or abort a running operation from the control surface when the task must be changed.",
          "<strong>Provider selection</strong>: sessions can use different configured providers without splitting the project into unrelated remote products.",
        ]),
        note("Do not invent provider setup commands", "Install and authenticate each selected provider CLI using that provider's supported workflow on the host. Then use CLI Status as a visibility hint and run a small task to prove the service identity can actually use it."),
      ].join("\n")),
      section("Useful companion guides", paragraph("Use " + docLink("web-workspace", "Web workspace") + " for the browser delivery loop and " + docLink("configuration", "Configuration") + " when command templates or provider overrides need to change.")),
    ].join("\n"),
  },
  {
    slug: "projects-files-git",
    title: "Projects, files, and Git",
    group: "Workbench workflows",
    type: "How-to guide",
    summary: "Keep host directories, file edits, diffs, branches, and remote Git operations in one reviewable project context.",
    keywords: "projects files editor upload search replace markdown git diff commit branch remote fetch pull push clone workspace",
    body: [
      lead("A project is the unit that ties an agent session to a real host directory. Files and Git tools make the change visible before you ask a terminal or agent to prove it."),
      section("Add the right project to the Web workspace", [
        steps([
          ["Prepare the directory on the host", "The browser <strong>+ Add project</strong> picker adds an existing directory; it does not create a workspace or GitHub clone. Prepare the directory through a trusted host workflow before using that picker."],
          ["Open the folder browser", "In the sidebar, choose <strong>+ Add project</strong>, then browse the server filesystem to the existing directory."],
          ["Add the project", "Confirm that folder. The workbench records it as a project and loads its host-owned files and Git state."],
          ["Check the root", "Confirm the displayed project path is the expected repository before starting sessions or commands."],
        ]),
        note("Host-owned files", "Web and mobile clients do not copy the project to themselves. Every browser edit, upload, Git action, and agent process is scoped to the server-side project."),
        note("Optional host preflight", "Before adding a directory, run <code>cargo run -p iowb-cli --bin io-workbench -- sandbox /srv/projects/example</code> on the host to verify that the configured workspace root accepts that path."),
      ].join("\n")),
      section("Create or clone outside the browser picker", [
        paragraph("The existing-folder restriction belongs to the current Web picker, not to every io-workbench client. The " + docLink("desktop-client", "native desktop client") + " has create-workspace and GitHub-clone flows. For controlled automation, the protected <code>POST /api/projects/create-workspace</code> route also accepts a <code>workspaceType</code> of <code>new</code> and can clone a <code>githubUrl</code> into that new workspace; <code>GET /api/projects/clone-progress</code> streams clone progress."),
        note("Keep clone scope narrow", "A GitHub clone is accepted only for a new workspace, and every requested path is still checked against the configured workspace root. Read the running " + docLink("api", "API reference") + " before automating the request shape or credential handling."),
      ].join("\n")),
      section("Review changes before shipping", compactSteps([
        ["Ask for a plan", "Use the agent session to identify intended files and validation."],
        ["Open changed files", "In Files, select a file from the tree, choose Edit or Preview, then Save intentional edits. Use find/replace or go-to-line only after you understand the change."],
        ["Inspect Git status and diff", "In Git, select the changed rows and use Diff/File Review. Make sure the diff contains only expected changes; resolve conflicts deliberately."],
        ["Run validation", "Use the server <strong>Shell</strong> or a configured validation step in the same project directory."],
        ["Stage and commit", "Stage selected files, enter a commit message, then choose <strong>Commit Selected</strong>. Both selected files and a message are required. Use the Branches and top Fetch/Pull/Push controls only when the host credentials and target remote are understood."],
      ])),
      section("What the Git surface supports", cards([
        ["Inspect", "Status and history", "Review status, repository history, file diffs, branches, remotes, and conflict state without leaving the selected project."],
        ["Change", "Stage and commit", "Stage selected work, create commits, and use generated messages only as a starting point for review."],
        ["Synchronize", "Remote operations", "Fetch, pull, push, publish, and manage branches/remotes where host credentials allow it. The browser picker adds existing folders; use a trusted host workflow, the native desktop flow, or the protected workspace API when a new clone is needed."],
        ["Recover", "Dangerous actions need intent", "Discarding changes or deleting untracked files affects the host filesystem; inspect the target and use the narrowest action."],
      ])),
      section("Project boundary controls", paragraph("The server's workspace root and file limits define what can be read or changed. Review " + docLink("security-and-boundaries", "Security and boundaries") + " before opening a broader filesystem scope or command integration.")),
    ].join("\n"),
  },
  {
    slug: "database-workspace",
    title: "Database workspace",
    group: "Workbench workflows",
    type: "How-to guide",
    summary: "Save connections, inspect schemas, run SQL, work with paginated data, and understand browser versus Android clipboard capabilities.",
    keywords: "database sqlite postgresql mysql mariadb schema relationships sql query pagination json import export transfer copy paste android navicat",
    body: [
      lead("The database workspace keeps data investigation beside the code and agent work that changes it. Use saved connections, schema exploration, SQL, paginated rows, and JSON jobs without turning a chat transcript into your only evidence."),
      section("Supported connection types", table(
        ["Connection", "Use it for", "Available workspace flow"],
        [
          ["<strong>SQLite</strong>", "Local database files visible to the host.", "Save/test the path, inspect schema, run SQL, browse rows, and use the supported table-transfer flow."],
          ["<strong>PostgreSQL</strong>", "Server-hosted relational databases.", "Save/test connections, inspect objects and relationships, run SQL, and browse paginated data."],
          ["<strong>MySQL</strong>", "MySQL-backed application data.", "Save/test connections, inspect objects, run SQL, and browse paginated data."],
          ["<strong>MariaDB</strong>", "MariaDB-backed application data.", "Save/test connections, inspect objects, run SQL, and browse paginated data."],
        ],
      )),
      section("Inspect and query data safely", steps([
        ["Open Database Explorer", "Open the Command palette with <strong>Command</strong> or <strong>Ctrl/Cmd+K</strong>, then choose <strong>Open Database Explorer</strong>. Database is not a permanent desktop navigation tab."],
        ["Save and test a connection", "Choose <strong>New</strong> to clear the form, enter its name/type/location details, use <strong>Test Form</strong>, then <strong>Save Connection</strong>. Saved connections belong to the signed-in account on the server, not to one selected project."],
        ["Select and explore", "Select the saved connection, then use Explorer to walk database/schema/object nodes, columns, foreign keys, details, and relationships before composing changes."],
        ["Start with a narrow SQL query", "Use SQL → Run Query with a limited read query first, then browse paginated table data to understand the shape of the result."],
        ["Use jobs for JSON movement", "Use JSON import/export jobs when data must move with explicit progress, warnings, and result feedback."],
        ["Review database changes with code", "Keep the migration, related source diff, validation output, and database evidence together in the same project review loop."],
      ])),
      section("Transfer and clipboard boundaries", [
        table(
          ["Capability", "Current behavior"],
          [
            ["<strong>Table transfer</strong>", "Use the current table-transfer flow with SQLite connections and review job feedback, warnings, and row limits before treating it as a migration tool."],
            ["<strong>Browser data grid</strong>", "The browser view can inspect and query data but does not yet expose row clipboard controls."],
            ["<strong>Native Android data grid</strong>", "Android can select rows and copy or paste structured JSON or CSV, which is useful for controlled data review on a phone."],
          ],
        ),
        note("Treat writes as production actions", "Saved connection details and an easy SQL surface do not make a database disposable. Confirm the target, query scope, and backup/rollback expectation before running mutations."),
      ].join("\n")),
      section("Use it with the rest of the workbench", paragraph("A good loop is: inspect schema and a narrow query, ask the agent for the smallest compatible change, review the source and Git diff, then validate through the " + docLink("terminal-and-mobile", "host terminal") + " or application test suite.")),
    ].join("\n"),
  },
  {
    slug: "terminal-and-mobile",
    title: "Terminal and mobile controls",
    group: "Workbench workflows",
    type: "How-to guide",
    summary: "Operate a live server-hosted PTY from the browser or Android without mistaking it for a local device shell.",
    keywords: "terminal pty remote shell process resize input output abort copy paste android termux esc tab ctrl alt arrows",
    body: [
      lead("The browser calls this surface <strong>Shell</strong>; it is a real PTY backed by the selected project on the server. It is for running host commands, watching output, resizing the terminal, and stopping processes—not an output-only command log."),
      section("Start a terminal in the correct project", steps([
        ["Select the project", "The Shell starts against the selected server-side project path, so check the project before running a migration, build, or destructive command."],
        ["Open Shell", "Select <strong>Shell</strong> in the workspace. It automatically starts or reuses a PTY for that project; wait for the connected state."],
        ["Run the smallest useful command", "Use a test, build, status, or inspection command first; the host process has the same project filesystem and credentials as the workbench."],
        ["Resize and interact", "Input, output streaming, resize events, copy/paste, and process visibility are part of the terminal control surface."],
        ["Stop deliberately", "Use process controls or the terminal interrupt when the command must end. Restart replaces the current PTY; Stop kills it. Inspect the resulting Git/data state before starting again."],
      ])),
      section("Browser and Android controls", cards([
        ["Web", "Full remote Shell", "Use the browser Shell to type, resize, copy a selection or recent output, paste commands, and manage active processes."],
        ["Android", "Native terminal renderer", "Android uses a native renderer built with Termux components and includes handheld Esc, Tab, Ctrl, Alt, and arrow controls."],
        ["Both", "Server-hosted execution", "Every command, package action, shell process, and PTY runs on the io-workbench server—not in the browser or on Android."],
      ], true)),
      section("Important Android distinction", note("Not a local Termux replacement", "The Android terminal feels native on the device, but it is a remote renderer for the server PTY. Installing a mobile client does not install Linux packages, agent CLIs, or a local shell on the phone.", true)),
      section("Terminal safety", list([
        "<strong>Check the working directory</strong> before every command that can mutate the project or a database.",
        "<strong>Use explicit validation</strong> rather than assuming an agent's text means a command passed.",
        "<strong>Keep remote exposure narrow</strong> because an authenticated terminal is a high-impact host control.",
        "<strong>Do not paste unreviewed commands</strong> into a production-connected PTY without understanding their target and rollback path.",
      ])),
    ].join("\n"),
  },
  {
    slug: "boards-and-tools",
    title: "Boards and tools",
    group: "Operate safely",
    type: "How-to guide",
    summary: "Turn broad work into reviewable tasks with the Agentic Board and deliberately configured command-backed tools.",
    keywords: "agentic board tasks validation qa approval mcp commands plugins taskmaster watchers automation notifications",
    body: [
      lead("The Agentic Board and tool integrations make work reviewable rather than opaque. Use them to break a broad goal into scoped tasks, provider choices, validation evidence, and explicit approval boundaries."),
      section("Run a controlled delivery loop", steps([
        ["Select the project and open Board", "Keep the task scope tied to the same host project whose files, Git state, database, and Shell you will review."],
        ["Author the board request", "Enter a Board Prompt, choose the provider/model and Git Policy, then use <strong>Add To Board</strong>. The default Git Policy is Read Only; change it deliberately only when the task truly needs write authority."],
        ["Review the backlog before execution", "Inspect the generated tasks, constraints, expected validation, and approval points. Adding a backlog creates reviewable work; it does not start execution."],
        ["Start intentionally", "Use <strong>Start board tasks</strong> only after the backlog and policy are understood. Confirm the configured provider and command-backed integration before allowing host work."],
        ["Review progress and evidence", "Use task status, output, Git changes, Shell validation, and database results to decide whether a task should continue, retry, or wait for approval."],
        ["Close only verified work", "Mark the task done after the code, tests, data effects, and Git evidence match the intended result."],
      ])),
      section("Command-backed integrations", table(
        ["Integration", "What it can do", "Operator responsibility"],
        [
          ["<strong>MCP</strong>", "Start/stop/list configured MCP processes and run command-backed tools with persisted run history.", "Configure the host command and arguments deliberately; treat tool execution as host execution."],
          ["<strong>Commands and plugins</strong>", "Run slash-command, plugin, and utility integrations.", "Review the command path, environment, arguments, and workspace access before enabling it."],
          ["<strong>Taskmaster and board actions</strong>", "Coordinate planning, task runs, validation, retries, and delivery state.", "Keep human approvals and validation expectations explicit."],
          ["<strong>Watchers and notifications</strong>", "Monitor project roots and send configured notifications.", "Limit watched paths and destinations to what the host operator intends."],
        ],
      )),
      section("Configure first, automate second", paragraph("These integrations are intentionally command-backed. Review " + docLink("configuration", "Configuration") + " and " + docLink("security-and-boundaries", "Security and boundaries") + " before giving a remote client access to additional host commands.")),
    ].join("\n"),
  },
  {
    slug: "security-and-boundaries",
    title: "Security and boundaries",
    group: "Operate safely",
    type: "Explanation",
    summary: "Understand what authentication, host ownership, workspace limits, and transport security protect in a remote control plane.",
    keywords: "security auth token otp bearer workspace root remote vpn proxy tunnel tls https wss terminal command host boundary",
    body: [
      lead("io-workbench gives an authenticated client control over host files, configured CLIs, Git, data connections, and terminal processes. The security model is therefore about preserving clear boundaries: who can connect, what host resources they can reach, and how traffic is protected."),
      section("Authentication defaults", list([
        "<strong>Authentication is enabled by default</strong>. With no local user, the server starts in setup mode; after registration, protected REST and WebSocket routes require authentication.",
        "<strong>Installation token</strong>: <code>IO_WORKBENCH_TOKEN</code> can supply a server token for local automation or token-based clients.",
        "<strong>OTP mode</strong>: <code>IO_WORKBENCH_OTP_SECRET</code> enables server-login TOTP from a valid Base32 secret. OTP takes precedence if both OTP and token are set.",
        "<strong>Browser/mobile credentials</strong> should never cross an unencrypted public connection. Use HTTPS/WSS outside a trusted local path.",
      ])),
      section("Authentication mode consequences", paragraph("Either <code>IO_WORKBENCH_TOKEN</code> or <code>IO_WORKBENCH_OTP_SECRET</code> enforces authentication even if <code>IO_WORKBENCH_AUTH_REQUIRED=false</code>, and either disables first-user registration. Use the detailed " + docLink("configuration", "Configuration") + " guide to choose one mode and keep the secret in the service environment.")),
      section("Keep authority on the host", cards([
        ["Projects", "Workspace boundary", "The configured workspace root limits where project/file operations begin. Select the narrowest host directory that supports the work."],
        ["Agents", "Provider credentials", "Claude, Codex, and Gemini credentials remain where their CLIs run: the server environment."],
        ["Terminal", "Process authority", "A PTY can execute high-impact host commands. Limit remote access to users and networks you trust."],
        ["Integrations", "Command authority", "MCP, plugins, commands, Taskmaster, and notification tools should be configured with explicit commands and least privilege."],
      ])),
      section("Remote access rules", [
        list([
          "<strong>Prefer VPN, authenticated tunnel, or reverse proxy</strong> over direct public exposure of the development listener.",
          "<strong>Terminate TLS and pass WSS</strong> for browsers and mobile clients outside the local network.",
          "<strong>Keep auth on</strong>; do not set <code>IO_WORKBENCH_AUTH_REQUIRED=false</code> for an exposed service.",
          "<strong>Scope firewall rules</strong> to the intended interface and network, especially when binding to <code>0.0.0.0</code> for a LAN test.",
        ]),
        paragraph("For a concrete setup path, use " + docLink("remote-access", "Remote access") + "."),
      ].join("\n")),
      section("Before running a high-impact action", compactSteps([
        ["Confirm the host and project", "Make sure you are connected to the expected server profile and selected project."],
        ["Inspect the target", "Read the diff, SQL, path, process, or command before executing it."],
        ["Keep evidence", "Use session history, Git, terminal output, and board validation so the action can be reviewed later."],
        ["Use a rollback path", "For data, deployment, or destructive file work, know the backup/revert strategy before the action starts."],
      ])),
    ].join("\n"),
  },
  {
    slug: "configuration",
    title: "Configuration",
    group: "Operate safely",
    type: "Reference",
    summary: "Configure listener, storage, workspace, authentication, provider command, and runtime limits through environment variables.",
    keywords: "configuration environment IO_WORKBENCH_HOST PORT CONFIG_DIR DATABASE_PATH WORKSPACE_ROOT AUTH_REQUIRED TOKEN OTP_SECRET totp providers cli args stdin limits",
    body: [
      lead("io-workbench configuration uses environment variables with the <code>IO_WORKBENCH_</code> prefix. Start from the safe local defaults, then make a focused change for storage, workspace scope, authentication, provider command behavior, or remote access."),
      section("Important defaults", table(
        ["Setting", "Default", "Use it to"],
        [
          ["<code>IO_WORKBENCH_HOST</code>", "<code>127.0.0.1</code>", "Keep first-run access local; change deliberately for a trusted LAN/proxy upstream."],
          ["<code>IO_WORKBENCH_PORT</code>", "<code>8787</code>", "Choose the listener port."],
          ["<code>IO_WORKBENCH_CONFIG_DIR</code>", "<code>~/.io-workbench</code>", "Move configuration storage."],
          ["<code>IO_WORKBENCH_DATABASE_PATH</code>", "<code>~/.io-workbench/io-workbench.db</code>", "Move the server SQLite database."],
          ["<code>IO_WORKBENCH_WORKSPACE_ROOT</code>", "Host user home", "Narrow the project/file scope."],
          ["<code>IO_WORKBENCH_AUTH_REQUIRED</code>", "<code>true</code>", "Keep authentication enabled; only disable for a deliberately trusted local dev setup."],
          ["<code>IO_WORKBENCH_TOKEN</code>", "Unset", "Provide token authentication for local automation or supported clients."],
          ["<code>IO_WORKBENCH_OTP_SECRET</code>", "Unset", "Enable server-login TOTP with a valid Base32 secret."],
        ],
      )),
      section("Example: controlled local override", [
        codeCard("IO_WORKBENCH_WORKSPACE_ROOT=/srv/projects \\\nIO_WORKBENCH_CONFIG_DIR=/srv/io-workbench-config \\\ncargo run -p iowb-cli --bin io-workbench -- start"),
        note("Restart after configuration changes", "Environment variables are read by the server process. Apply the change in the service environment and restart the specific service instance that runs io-workbench."),
      ].join("\n")),
      section("Choose one server-login mode deliberately", [
        paragraph("With the default password mode, the first visitor completes single-user setup. Setting <code>IO_WORKBENCH_TOKEN</code> switches the server to token mode. Setting <code>IO_WORKBENCH_OTP_SECRET</code> switches it to TOTP mode; the secret must be valid Base32 and decode to at least 10 bytes."),
        list([
          "<strong>OTP wins over token</strong>: when both variables are set, the server uses OTP mode rather than token mode.",
          "<strong>Token or OTP always enforces authentication</strong>: either variable requires auth even when <code>IO_WORKBENCH_AUTH_REQUIRED=false</code>.",
          "<strong>Token or OTP disables first-user registration</strong>: configure the chosen secret before exposing the server, then use the matching documented client flow. Native desktop has password/setup dialogs plus a bearer-token field, but no dedicated OTP field.",
          "<strong>Do not confuse this with IO Gateway settings</strong>: Settings → IO Gateway has a separate optional Secret OTP for that integration. It does not configure io-workbench server-login TOTP.",
        ]),
        note("Protect the secret", "Store a token or TOTP secret in the service/container secret environment, not in browser/mobile code, chat history, screenshots, or a public shell profile.", true),
      ].join("\n")),
      section("Provider command configuration", [
        paragraph("The default commands are <code>claude --print {prompt}</code>, <code>codex exec {prompt}</code>, and <code>gemini --prompt {prompt}</code>. Use provider-specific variables such as <code>IO_WORKBENCH_CODEX_COMMAND</code> and <code>IO_WORKBENCH_CODEX_ARGS_JSON</code>, or global command/argument overrides, only when the host CLI truly requires them."),
        list([
          "<strong>Argument templates</strong> support <code>{prompt}</code>, <code>{session_id}</code>, and <code>{model}</code>.",
          "<strong>Stdin mode</strong> can be enabled with <code>IO_WORKBENCH_AGENT_STDIN=true</code> or a provider-specific variant when a CLI expects its prompt on stdin.",
          "<strong>Validate first</strong> with CLI Status and one small agent task after changing command templates.",
        ]),
      ].join("\n")),
      section("Limits and integrations", list([
        "<strong>Runtime limits</strong>: configure maximum sessions, scan depth, file read size, tool timeout, and database transfer row limits using the documented <code>IO_WORKBENCH_*</code> variables.",
        "<strong>Command-backed integrations</strong>: MCP, command, plugin, Taskmaster, push, and transcription variables point at host commands. Keep paths and arguments explicit.",
        "<strong>Direct AI keys</strong>: provider-specific credentials such as <code>ANTHROPIC_API_KEY</code> or <code>CODEX_GATEWAY_KEY</code> belong in the server environment, not mobile/browser source.",
      ])),
      section("Related setup", paragraph("Use " + docLink("remote-access", "Remote access") + " for listener/proxy decisions and " + docLink("troubleshooting", "Troubleshooting") + " when a changed setting does not behave as expected.")),
    ].join("\n"),
  },
  {
    slug: "deployment-and-recovery",
    title: "Deployment and recovery",
    group: "Operate safely",
    type: "Operations guide",
    summary: "Run io-workbench persistently, preserve its host data, verify upgrades, and recover or archive legacy state safely.",
    keywords: "deployment docker systemd service github release tag workbench.giofahreza.com backup upgrade rollback recovery health data workspace config import legacy persistent",
    body: [
      lead("A remote workbench should be operated as a persistent host service, not a browser tab or an expendable container. Preserve the server configuration/data and project workspace separately, put the listener behind the right network boundary, and prove each update before you rely on it."),
      section("Choose the persistent boundaries", table(
        ["Boundary", "What to preserve", "Why it matters"],
        [
          ["<strong>Configuration and server data</strong>", "The configured data directory and its server SQLite database.", "It contains workbench configuration, users, settings, session history, saved connections, and related state."],
          ["<strong>Project workspace</strong>", "The host directories you add as projects.", "Source files, Git repositories, database files, and Shell work remain here; they are not copied into a browser or phone."],
          ["<strong>Provider credentials and tools</strong>", "The host or custom image environment that runs Claude, Codex, and Gemini CLIs.", "The shipped container image does not install those CLIs or log them in for you."],
        ],
      )),
      section("Run a persistent container", [
        paragraph("The supplied image uses <code>/data</code> for configuration/data and <code>/workspace</code> for the workspace. Bind the host port locally when a reverse proxy, tunnel, or VPN will be the remote boundary."),
        codeCard("docker build -t io-workbench .\ndocker volume create iowb-data\ndocker run -d --name io-workbench --restart unless-stopped \\\n  -p 127.0.0.1:8787:8787 \\\n  -v iowb-data:/data \\\n  -v /srv/projects:/workspace \\\n  io-workbench", "Persistent Docker example"),
        note("Treat the workspace mount as authority", "Mount only the project root the server should control. A broad host bind mount grants the workbench, its authenticated users, agent CLIs, and Shell access to that data."),
        note("Persist provider login state deliberately", "The <code>/data</code> and <code>/workspace</code> mounts do not by themselves preserve provider CLI authentication homes. Provision and persist the selected CLIs’ credentials for the same controlled container or service identity before relying on agent sessions."),
      ].join("\n")),
      section("Run a source build under a service manager", [
        paragraph("The repository includes <code>deploy/io-workbench.service.example</code> as a production baseline. Copy it to the host only after replacing the service user, data path, workspace path, and provider credential policy. Keep the managed binary outside a developer checkout so a deployment cannot overwrite local build output."),
        codeCard("[Unit]\nDescription=io-workbench production host\nAfter=network-online.target\n\n[Service]\nUser=io-workbench\nWorkingDirectory=/srv/projects\nEnvironment=IO_WORKBENCH_HOST=127.0.0.1\nEnvironment=IO_WORKBENCH_PORT=8100\nEnvironment=IO_WORKBENCH_CONFIG_DIR=/var/lib/io-workbench\nEnvironment=IO_WORKBENCH_WORKSPACE_ROOT=/srv/projects\nExecStart=/opt/io-workbench/io-workbench start\nRestart=on-failure\n\n[Install]\nWantedBy=multi-user.target", "Release-binary systemd baseline"),
        paragraph("Build the release binary with <code>cargo build --release -p iowb-cli --bin io-workbench</code>, or let the tag workflow replace <code>/opt/io-workbench/io-workbench</code>. Run the service as a dedicated non-login account where appropriate, and use " + docLink("remote-access", "Remote access") + " rather than publishing a development listener directly."),
      ].join("\n")),
      section("Release-tag production deployment", [
        paragraph("Pushing a <code>v*</code> tag runs the release automation: it verifies the repository, publishes the checked native archives and Android APKs to GitHub Releases, then deploys the published Linux host package to <code>workbench.giofahreza.com</code>. The deployment job stages and verifies the release artifact before replacing the managed binary, restarts the configured service, and checks local plus public health."),
        table(
          ["Stage", "What the automation proves", "Operator responsibility"],
          [
            ["<strong>Release</strong>", "The tag has published the platform packages, installers, APKs, and <code>SHA256SUMS</code>.", "Review the GitHub Release asset list and checksums before treating it as an approved production version."],
            ["<strong>Host update</strong>", "The configured Linux deployment host received the exact staged artifact and the service restarted.", "Keep service ownership, data directory, project workspace, provider CLIs, and credential material provisioned outside the replacement binary."],
            ["<strong>Public boundary</strong>", "The workflow polls <code>https://workbench.giofahreza.com/health</code> after the local health check.", "Keep the DNS/Cloudflare or reverse-proxy route, TLS, authentication, WebSocket forwarding, and firewall policy in place."],
          ],
        ),
        codeCard("curl -fsS https://workbench.giofahreza.com/health", "Public production health"),
        note("Deployment prerequisites", "The release workflow deliberately takes its SSH target, service name, installation paths, and health URLs from protected GitHub Actions secrets. Configure those once for the production host and limit the SSH key to the deployment account; a tag cannot safely invent a DNS route, service account, or provider credential."),
        heading("Configure GitHub Actions and the production host once"),
        paragraph("The repository files <code>deploy/README.md</code>, <code>deploy/io-workbench.service.example</code>, and <code>deploy/cloudflared-workbench-ingress.example.yml</code> are the operator starting point. Use a dedicated release directory such as <code>/opt/io-workbench</code>; do not deploy over <code>target/release</code> in a source checkout."),
        table(
          ["GitHub secret", "Typical value", "Why it is needed"],
          [
            ["<code>DEPLOY_HOST</code>, <code>DEPLOY_USER</code>, <code>DEPLOY_SSH_KEY</code>, <code>DEPLOY_SSH_KNOWN_HOSTS</code>", "The production host, restricted deployment account/key, and its expected OpenSSH known-hosts entry.", "Transfers only the verified Linux release artifact and pins the host key instead of trusting a fresh network lookup."],
            ["<code>DEPLOY_REMOTE_STAGE_DIR</code>", "A user-writable staging directory such as <code>/home/io-workbench/deploy</code>.", "Allows checksum verification before privileged installation."],
            ["<code>DEPLOY_LIVE_BINARY</code>, <code>DEPLOY_SERVICE_NAME</code>", "<code>/opt/io-workbench/io-workbench</code> and <code>io-workbench.service</code>.", "Defines the managed executable and the systemd service to restart."],
            ["<code>DEPLOY_HEALTH_URL</code>", "<code>http://127.0.0.1:8100/health</code>.", "Proves the restarted host locally before the public check."],
            ["<code>DEPLOY_SSH_PORT</code>, <code>DEPLOY_PUBLIC_HEALTH_URL</code>", "Optional; port <code>22</code> and <code>https://workbench.giofahreza.com/health</code> are the defaults.", "Overrides the network defaults only when the production layout requires it."],
            ["<code>IOWB_ANDROID_KEYSTORE_BASE64</code>, <code>IOWB_ANDROID_KEYSTORE_PASSWORD</code>, <code>IOWB_ANDROID_KEY_ALIAS</code>, <code>IOWB_ANDROID_KEY_PASSWORD</code>", "One stable Android signing key and its credentials.", "Keeps future APKs installable as updates rather than signature-mismatched new apps."],
            ["<code>IOWB_SUBMODULES_TOKEN</code>", "Optional read-only Contents token for the private <code>apps</code>/<code>rag</code> submodules.", "Lets the tag workflow check out the same mobile and Rust source that the tag references."],
          ],
        ),
        codeCard("# Add this before cloudflared's final catch-all ingress rule.\n- hostname: workbench.giofahreza.com\n  service: http://127.0.0.1:8100", "Cloudflare Tunnel ingress"),
        note("Do not skip the one-time boundary work", "Create the Cloudflare DNS/tunnel hostname, add the ingress route, reload cloudflared, give the deployment account narrowly scoped non-interactive permission to install the release binary and restart only this service, and store its controlled <code>DEPLOY_SSH_KNOWN_HOSTS</code> entry. The workflow never trusts a newly scanned SSH host key, and it intentionally fails its public health check until that boundary is real."),
        paragraph("The release deployment replaces only executable code. It preserves the data/config directory and workspace, retains a timestamped prior binary, and automatically restores that prior binary if the new service fails local restart or health verification. If the public check still fails after a healthy local restart, use the workflow output to repair the tunnel/proxy boundary rather than deleting application data as an update workaround."),
      ].join("\n")),
      section("Back up, update, and verify", compactSteps([
        ["Record the known-good state", "Capture the running version, configured paths, service/image version, and a health result before changing anything. Back up the configuration/data location and the relevant project repositories or database snapshots."],
        ["Keep data while replacing code", "Stop only the service/container, replace the binary or image, and keep its configuration/data volume and workspace mount intact. Do not delete persistent data as part of an ordinary update."],
        ["Restart and verify locally", "Start the updated instance and run <code>curl http://127.0.0.1:8787/health</code>. Then sign in, open one expected project, verify CLI Status, and perform a small Shell or session check."],
        ["Verify the remote boundary", "If clients use a VPN, proxy, or tunnel, test HTTPS login and live WSS output after the local health check passes."],
        ["Roll back deliberately", "If the new build fails, restore the previous binary/image and service configuration while preserving the data backup. Investigate schema/configuration compatibility before attempting a data rollback."],
      ])),
      section("Inspect old workbench data without overwriting it", [
        paragraph("The legacy import is archival, not an automatic live-settings migration. It copies legacy files into a timestamped <code>legacy-imports</code> directory under the current config directory and leaves the original source unchanged."),
        codeCard("cargo run -p iowb-cli --bin io-workbench -- import-legacy --dry-run\ncargo run -p iowb-cli --bin io-workbench -- import-legacy", "Legacy archive import"),
        note("Review before relying on it", "Run <code>--dry-run</code> first. Inspect the copied archive and decide what to migrate or recreate through the current UI; do not assume it restores active users, sessions, or settings automatically."),
      ].join("\n")),
      section("Continue with", paragraph("Use " + docLink("configuration", "Configuration") + " for paths and runtime variables, " + docLink("security-and-boundaries", "Security and boundaries") + " for host authority, and " + docLink("troubleshooting", "Troubleshooting") + " when a restart or remote check fails.")),
    ].join("\n"),
  },
  {
    slug: "settings-and-integrations",
    title: "Settings and integrations",
    group: "Operate safely",
    type: "How-to guide",
    summary: "Configure agents, Git identity, API material, notifications, gateway chat, runtime status, and command-backed tools without widening host authority accidentally.",
    keywords: "settings agents cli status git identity api tokens credentials notifications io gateway metrics tools mcp plugins taskmaster transcription",
    body: [
      lead("Settings is the operator desk for the workbench's rich control surfaces: provider readiness, Git identity, access material, browser notifications, optional IO Gateway chat, runtime health, and command-backed tools. Use it to make a deliberate host configuration visible—not to grant a browser or phone new authority by accident."),
      section("Start with the Settings map", table(
        ["Settings tab", "Use it for", "Safe first action"],
        [
          ["<strong>Agents</strong>", "CLI Status and user/onboarding state.", "Use CLI Status as a readiness hint, then prove the provider with one small non-destructive task before a larger session."],
          ["<strong>Appearance and Git</strong>", "Workspace density/wrapping/Shell geometry and host Git identity.", "Set the display you need, then load Git Config and verify the identity before committing."],
          ["<strong>API Tokens</strong>", "Stored API keys and credentials.", "Inspect existing material and create or change only the smallest-scoped credential needed."],
          ["<strong>Notifications</strong>", "Browser/web-push preferences and completion/failure events.", "Load preferences, request browser permission if appropriate, save, then use Preview before relying on a notification path."],
          ["<strong>IO Gateway</strong>", "Optional gateway-backed chat configuration and models.", "Review the effective connection/model state before enabling <em>Use IO Gateway for chat</em> or saving a gateway secret."],
          ["<strong>Server Status and Tools</strong>", "Runtime metrics, MCP lifecycle, command-backed utilities, plugins, Taskmaster, notifications, and transcription.", "Inspect status first; configure explicit host command paths and arguments before running a tool."],
        ],
      )),
      section("A safe configuration sequence", steps([
        ["Confirm the host boundary", "Select the intended project and confirm the server URL, authenticated account, workspace root, and remote-access boundary before editing credentials or running tools."],
        ["Prove the small control", "Use CLI Status, Git Config, Load Notifications, or Server Status to inspect current state before saving a change."],
        ["Save one scoped change", "For tokens, notifications, or an IO Gateway connection, change one field set at a time and keep secrets out of chat transcripts, screenshots, and mobile/browser source."],
        ["Test without broad execution", "Use Preview for notifications, a small agent task for a CLI/gateway change, and a harmless command/path for a command-backed integration."],
        ["Review the result", "Check Server Status, tool-run history, session output, Git evidence, and the remote client boundary before treating the configuration as ready."],
      ])),
      section("Command-backed tools are host execution", [
        paragraph("The Tools tab can start/list/stop configured MCP servers, run MCP/utility/command/plugin/Taskmaster/notification actions, and send audio to a configured transcription command. These are deliberately command-backed integrations, not managed SaaS switches."),
        list([
          "<strong>Use explicit paths and arguments</strong>; do not paste unreviewed commands into a host-control form.",
          "<strong>Keep environment and workspace scope narrow</strong>; an integration inherits authority from the server host.",
          "<strong>Test the least-impacting action first</strong> and inspect persisted tool output before enabling a broader workflow.",
          "<strong>Disable or remove stale integrations</strong> when their command, credential, or access boundary is no longer intended.",
        ]),
      ].join("\n")),
      section("Continue with", paragraph("Read " + docLink("agents-and-sessions", "Agents and sessions") + " before provider work, " + docLink("boards-and-tools", "Boards and tools") + " before task automation, " + docLink("configuration", "Configuration") + " for environment-backed settings, and " + docLink("security-and-boundaries", "Security and boundaries") + " before extending host control.")),
    ].join("\n"),
  },
  {
    slug: "troubleshooting",
    title: "Troubleshooting",
    group: "Reference",
    type: "Troubleshooting",
    summary: "Diagnose host startup, authentication, provider CLI, project, WebSocket, mobile, and remote-access failures.",
    keywords: "troubleshooting health login auth token otp cli status project websocket mobile android pwa remote proxy wss",
    body: [
      lead("Start diagnosis at the host, then move outward through authentication, project scope, provider CLIs, and finally remote client/network details. A phone or browser error often begins with a server or proxy condition."),
      section("First checks", [
        codeCard("curl http://127.0.0.1:8787/health\ncargo run -p iowb-cli --bin io-workbench -- status"),
        list([
          "<strong>Server unavailable</strong>: confirm the command/service is running and the host/port match the URL you are testing.",
          "<strong>Wrong listener</strong>: a local-only listener will not be reachable from a physical phone even if it works in the host browser.",
          "<strong>Wrong project</strong>: verify the selected project path and workspace root before assuming files are missing.",
        ]),
      ].join("\n")),
      section("Common symptoms", table(
        ["Symptom", "Likely check"],
        [
          ["<strong>First login does not work</strong>", "Confirm whether the server is in first-user setup, password, token, or OTP mode; re-enter the appropriate credential."],
          ["<strong>Provider is unavailable</strong>", "Open Settings → Agents → CLI Status. Install/authenticate the CLI on the host and verify any command override."],
          ["<strong>Browser receives no live output</strong>", "Check the authenticated WebSocket connection and proxy support for <code>/ws</code>."],
          ["<strong>Mobile receives a web page instead of API data</strong>", "The URL likely points to a proxy/login page or wrong hostname. Check the exact server base URL and access-layer headers."],
          ["<strong>Android cannot reach host</strong>", "Use <code>10.0.2.2</code> only in the Android emulator; use LAN/VPN/tunnel/HTTPS for a physical device."],
          ["<strong>Terminal is not local</strong>", "Confirm the selected remote host/project and remember the PTY runs there, not on the phone."],
          ["<strong>Database action is unexpected</strong>", "Recheck saved connection, schema/table, query scope, and transfer job feedback before retrying."],
        ],
      )),
      section("Remote access diagnosis", compactSteps([
        ["Prove host health", "Test <code>/health</code> locally on the server."],
        ["Prove the proxy route", "Open the HTTPS hostname from a browser and confirm it reaches the workbench rather than an unrelated page."],
        ["Prove authentication", "Sign in, list projects, and open one normal REST request before testing a live session."],
        ["Prove WSS", "Start a small session or terminal and verify streamed events arrive through <code>/ws</code>."],
        ["Recheck the mobile profile", "Update the saved server URL, credentials, and optional headers only after the server/proxy path is known good."],
      ])),
      section("When to inspect configuration", paragraph("If the host starts but behaves differently from expectation, review " + docLink("configuration", "Configuration") + " for listener, workspace root, auth, provider command, and integration environment variables.")),
    ].join("\n"),
  },
  {
    slug: "api",
    title: "API reference",
    group: "Reference",
    type: "Reference",
    summary: "Find health, WebSocket, and bundled OpenAPI material while treating the running server as the source of truth.",
    keywords: "api openapi health websocket ws rest authentication bearer token endpoint reference",
    body: [
      lead("The browser, Android client, and PWA use the io-workbench HTTP API and WebSocket event stream. Use this page to orient yourself, then check the bundled OpenAPI document and the running server behavior before building a separate integration."),
      section("Key entry points", table(
        ["Endpoint", "Purpose", "Notes"],
        [
          ["<code>GET /health</code>", "Simple host reachability check.", "Use it first for local and remote diagnostics."],
          ["<code>GET /ws</code>", "Live events and controls.", "Requires the authenticated WebSocket flow; remote proxies must pass WSS upgrades."],
          ["<code>GET /openapi.json</code>", "Bundled OpenAPI index.", "Useful for orientation, but treat it as an index that can lag newly added router endpoints."],
          ["<code>/api/*</code>", "Protected workbench REST families.", "Authentication, project scope, and endpoint-specific controls apply."],
        ],
      )),
      section("Authentication and transport", [
        list([
          "<strong>Browser sessions</strong> are created through the supported setup/login flow.",
          "<strong>Bearer tokens</strong> can be used for supported automation/mobile paths when token authentication is configured.",
          "<strong>WebSocket auth</strong> follows the client's supported authenticated connection flow.",
          "<strong>HTTPS/WSS</strong> is required when credentials or tokens leave a trusted local development path.",
        ]),
      ].join("\n")),
      section("Integration workflow", compactSteps([
        ["Start with health", "Check the target host and its network boundary before debugging an API request."],
        ["Authenticate deliberately", "Use an account/token that matches the server's configured authentication mode."],
        ["Constrain project scope", "Operate only against the expected project and path; a remote API is still host control."],
        ["Use the OpenAPI index as a guide", "Confirm endpoint availability and response behavior against the running build when the contract matters."],
        ["Test WebSocket separately", "A REST request can succeed while a reverse proxy still blocks or rewrites WSS upgrades."],
      ])),
      section("Related documents", paragraph("Read " + docLink("security-and-boundaries", "Security and boundaries") + " before automating host-control endpoints, and " + docLink("remote-access", "Remote access") + " before giving an API client a public hostname.")),
    ].join("\n"),
  },
];

const pageRelationships = {
  "quick-start": {
    topics: ["Web workspace", "Mobile clients", "Native desktop client", "Remote access and security"],
    seeAlso: ["install-and-update", "web-workspace", "mobile", "deployment-and-recovery"],
  },
  "install-and-update": {
    topics: ["Mobile clients", "Native desktop client", "Remote access and security", "Operations and API"],
    seeAlso: ["quick-start", "mobile", "deployment-and-recovery", "remote-access"],
  },
  "web-workspace": {
    topics: ["Web workspace", "Agents and providers", "Projects and Git", "Database and terminal"],
    seeAlso: ["agents-and-sessions", "projects-files-git", "database-workspace", "mobile"],
  },
  mobile: {
    topics: ["Mobile clients", "Database and terminal", "Remote access and security"],
    seeAlso: ["install-and-update", "remote-access", "terminal-and-mobile", "database-workspace"],
  },
  "desktop-client": {
    topics: ["Native desktop client", "Projects and Git", "Remote access and security"],
    seeAlso: ["install-and-update", "projects-files-git", "terminal-and-mobile", "remote-access"],
  },
  "remote-access": {
    topics: ["Mobile clients", "Native desktop client", "Remote access and security"],
    seeAlso: ["security-and-boundaries", "mobile", "desktop-client", "deployment-and-recovery"],
  },
  "agents-and-sessions": {
    topics: ["Web workspace", "Agents and providers", "Projects and Git"],
    seeAlso: ["web-workspace", "projects-files-git", "configuration", "boards-and-tools"],
  },
  "projects-files-git": {
    topics: ["Web workspace", "Native desktop client", "Projects and Git"],
    seeAlso: ["web-workspace", "desktop-client", "terminal-and-mobile", "security-and-boundaries"],
  },
  "database-workspace": {
    topics: ["Web workspace", "Mobile clients", "Native desktop client", "Database and terminal"],
    seeAlso: ["terminal-and-mobile", "mobile", "web-workspace", "projects-files-git"],
  },
  "terminal-and-mobile": {
    topics: ["Web workspace", "Mobile clients", "Native desktop client", "Database and terminal"],
    seeAlso: ["web-workspace", "mobile", "desktop-client", "security-and-boundaries"],
  },
  "boards-and-tools": {
    topics: ["Agents and providers", "Operations and API"],
    seeAlso: ["agents-and-sessions", "settings-and-integrations", "security-and-boundaries"],
  },
  "security-and-boundaries": {
    topics: ["Remote access and security", "Operations and API"],
    seeAlso: ["remote-access", "configuration", "deployment-and-recovery", "api"],
  },
  configuration: {
    topics: ["Agents and providers", "Remote access and security", "Operations and API"],
    seeAlso: ["security-and-boundaries", "agents-and-sessions", "settings-and-integrations", "remote-access"],
  },
  "deployment-and-recovery": {
    topics: ["Remote access and security", "Operations and API"],
    seeAlso: ["install-and-update", "configuration", "remote-access", "security-and-boundaries"],
  },
  "settings-and-integrations": {
    topics: ["Agents and providers", "Operations and API"],
    seeAlso: ["agents-and-sessions", "boards-and-tools", "configuration", "security-and-boundaries"],
  },
  troubleshooting: {
    topics: ["Mobile clients", "Native desktop client", "Remote access and security", "Operations and API"],
    seeAlso: ["configuration", "remote-access", "mobile", "desktop-client"],
  },
  api: {
    topics: ["Projects and Git", "Remote access and security", "Operations and API"],
    seeAlso: ["security-and-boundaries", "remote-access", "projects-files-git", "configuration"],
  },
};

const knownTopicNames = new Set(topicCategories.map((topic) => topic.title));
const knownPageSlugs = new Set(pageDefinitions.map((page) => page.slug));
const pages = pageDefinitions.map((page) => {
  const relationship = pageRelationships[page.slug];
  if (!relationship?.topics?.length) {
    throw new Error("Documentation page is missing topics: " + page.slug);
  }
  if (relationship.topics.some((topic) => !knownTopicNames.has(topic))) {
    throw new Error("Documentation page uses an unknown topic: " + page.slug);
  }
  if ((relationship.seeAlso ?? []).some((slug) => slug === page.slug || !knownPageSlugs.has(slug))) {
    throw new Error("Documentation page has an invalid related-guide link: " + page.slug);
  }
  return {
    ...page,
    topics: relationship.topics,
    seeAlso: relationship.seeAlso ?? [],
  };
});

if (Object.keys(pageRelationships).length !== pages.length) {
  throw new Error("Every documentation page must have one relationship record.");
}

function renderHeader() {
  return [
    '<a class="skip-link" href="#docs-content">Skip to docs content</a>',
    '<header class="docs-header"><div class="docs-shell docs-header-inner">',
    '<a class="docs-brand" href="/landing" aria-label="io-workbench home"><span class="docs-brand-mark" aria-hidden="true">io</span><span>io-workbench</span></a>',
    '<nav class="docs-site-nav" aria-label="Site navigation"><a href="/landing">Home</a><a href="/docs/" aria-current="page">Docs</a><a href="https://github.com/giofahreza/io-workbench" target="_blank" rel="noreferrer">GitHub</a></nav>',
    '<button class="docs-theme-toggle" type="button" data-theme-toggle aria-label="Toggle color theme" aria-pressed="false" title="Toggle color theme"><span class="docs-theme-icon" aria-hidden="true"></span><span class="docs-theme-label" aria-hidden="true"></span></button>',
    "</div></header>",
  ].join("\n");
}

function sidebarLink(page, activeSlug) {
  const active = page.slug === activeSlug;
  return [
    '<a href="' + pageUrl(page) + '" data-doc-link data-title="' + escapeHtml(page.title) + '" data-category="' + escapeHtml(page.group) + '" data-summary="' + escapeHtml(page.summary) + '" data-keywords="' + escapeHtml(page.keywords) + '"' + (active ? ' class="is-active" aria-current="page"' : "") + ">",
    "<span>" + escapeHtml(page.title) + "</span><small>" + escapeHtml(page.summary) + "</small>",
    "</a>",
  ].join("");
}

function topicSidebarLink(topic, activeSlug) {
  const active = activeSlug === "topic-" + topic.slug;
  return [
    '<a href="' + topicUrl(topic) + '" data-doc-link data-title="' + escapeHtml(topic.title) + '" data-category="Topic" data-summary="' + escapeHtml(topic.description) + '" data-keywords="' + escapeHtml(topic.title.toLowerCase()) + ' documentation topic"' + (active ? ' class="is-active" aria-current="page"' : "") + ">",
    "<span>" + escapeHtml(topic.title) + "</span><small>" + escapeHtml(topic.description) + "</small>",
    "</a>",
  ].join("");
}

function renderSidebar(activeSlug) {
  const groupsHtml = groups.map((group) => {
    const groupPages = pages.filter((page) => page.group === group.title);
    return [
      '<div class="docs-nav-group">',
      '<p class="docs-nav-label">' + escapeHtml(group.title) + "</p>",
      groupPages.map((page) => sidebarLink(page, activeSlug)).join(""),
      "</div>",
    ].join("");
  }).join("\n");

  return [
    '<aside class="docs-sidebar" aria-label="Documentation topics">',
    '<label class="docs-search" for="docs-search"><span>Search docs</span><input id="docs-search" type="search" autocomplete="off" placeholder="Search guides" /></label>',
    '<div id="docs-search-results" class="docs-search-results" aria-live="polite"></div>',
    '<nav class="docs-nav" aria-label="Documentation navigation">',
    '<a class="docs-home-link' + (activeSlug ? "" : " is-active") + '" href="/docs/" data-doc-link data-title="Documentation" data-category="Product documentation" data-summary="Install and use io-workbench from web, native desktop, and mobile clients." data-keywords="documentation overview install web desktop mobile remote agents database terminal"' + (activeSlug ? "" : ' aria-current="page"') + ">Docs home</a>",
    groupsHtml,
    '<div class="docs-nav-group"><p class="docs-nav-label">Browse topics</p>',
    topicCategories.map((topic) => topicSidebarLink(topic, activeSlug)).join(""),
    "</div></nav>",
    '<p class="docs-sidebar-note">The server owns the project, agent CLIs, processes, and PTY. Web, native desktop, and mobile are authenticated control surfaces.</p>',
    "</aside>",
  ].join("\n");
}

function renderMeta(page) {
  const values = [
    ["Type", page.type],
    ["Applies to", page.group],
    ["Docs version", docsVersion],
    ["Updated", updated],
  ];
  return '<dl class="docs-meta">' + values.map((value) => "<div><dt>" + escapeHtml(value[0]) + "</dt><dd>" + escapeHtml(value[1]) + "</dd></div>").join("") + "</dl>";
}

function renderTopicLinks(page) {
  const topics = page.topics
    .map((title) => topicCategories.find((topic) => topic.title === title))
    .filter(Boolean);
  if (!topics.length) return "";
  return '<p class="docs-topic-links"><span>Topics</span>' + topics.map((topic) => '<a href="' + topicUrl(topic) + '">' + escapeHtml(topic.title) + "</a>").join("") + "</p>";
}

function renderRelated(page) {
  const relatedSlugs = page.seeAlso?.length
    ? page.seeAlso
    : pages
      .filter((item) => item.slug !== page.slug && item.group === page.group)
      .slice(0, 3)
      .map((item) => item.slug);
  const related = relatedSlugs
    .map((slug) => pages.find((item) => item.slug === slug))
    .filter(Boolean);
  if (!related.length) return "";
  return section("Related guides", '<ul class="docs-page-list compact">' + related.map((item) => '<li><a href="' + pageUrl(item) + '"><span>' + escapeHtml(item.title) + "</span><small>" + escapeHtml(item.summary) + "</small></a></li>").join("") + "</ul>", "related-guides");
}

function renderArticle(page) {
  return [
    '<article id="docs-content" class="docs-content docs-article" data-page-slug="' + escapeHtml(page.slug) + '" data-page-group="' + escapeHtml(page.group) + '">',
    '<div class="docs-heading"><p class="docs-eyebrow">' + escapeHtml(page.group) + "</p><h1>" + escapeHtml(page.title) + "</h1>" + renderMeta(page) + "</div>",
    renderTopicLinks(page),
    page.body,
    renderRelated(page),
    "</article>",
  ].join("\n");
}

function renderShell(title, description, activeSlug, article) {
  return [
    "<!doctype html>",
    '<html lang="en">',
    "<head>",
    '<meta charset="utf-8" />',
    '<meta name="viewport" content="width=device-width, initial-scale=1" />',
    '<meta name="description" content="' + escapeHtml(description) + '" />',
    '<meta name="theme-color" content="#f4f7f5" />',
    "<title>" + escapeHtml(title) + " — io-workbench Docs</title>",
    '<link rel="icon" href="/icon.svg" type="image/svg+xml" />',
    '<script src="/app/landing-theme.js?v=' + assetVersion + '"></script>',
    '<link rel="stylesheet" href="/styles/docs.css?v=' + assetVersion + '" />',
    '<script src="/app/docs.js?v=' + assetVersion + '" defer></script>',
    "</head>",
    "<body>",
    renderHeader(),
    '<main class="docs-page-shell"><div class="docs-layout docs-shell">',
    renderSidebar(activeSlug),
    article,
    '<aside class="docs-on-page" aria-label="On this page"><p>On this page</p><nav id="on-this-page"></nav></aside>',
    "</div></main>",
    '<footer class="docs-footer docs-shell"><span>io-workbench Docs</span><div><a href="/landing">Home</a><a href="https://github.com/giofahreza/io-workbench" target="_blank" rel="noreferrer">GitHub</a></div></footer>',
    "</body>",
    "</html>",
    "",
  ].join("\n");
}

function renderHome() {
  const startPages = ["quick-start", "install-and-update", "web-workspace", "mobile", "desktop-client", "remote-access", "deployment-and-recovery"]
    .map((slug) => pages.find((page) => page.slug === slug))
    .filter(Boolean);
  const article = [
    '<article id="docs-content" class="docs-content docs-article docs-index-page" data-page-slug="" data-page-group="">',
    '<div class="docs-heading"><p class="docs-eyebrow">Product documentation</p><h1>Documentation</h1><dl class="docs-meta"><div><dt>Product</dt><dd>io-workbench</dd></div><div><dt>Docs version</dt><dd>' + docsVersion + "</dd></div><div><dt>Updated</dt><dd>" + updated + "</dd></div></dl></div>",
    lead("Install and operate a self-hosted remote workbench for configured Claude, Codex, and Gemini CLIs. These guides cover release binaries, Android APKs, Web, native desktop, and mobile flows—not just the feature list."),
    section("Start with the path you need", pageList(startPages.map((page) => ({
      title: page.title,
      summary: page.summary,
      href: pageUrl(page),
    })))),
    section("How the product fits together", cards([
      ["1. Host", "Run io-workbench where the work lives", "The server owns repositories, CLIs, provider credentials, database connectivity, shell processes, and storage."],
      ["2. Control surface", "Use Web, native desktop, or mobile", "Sign in from a browser, source-built native desktop client, Android client, or PWA to operate the same selected project remotely."],
      ["3. Evidence loop", "Review before shipping", "Use agent output with files, Git, database results, terminal validation, and board state instead of trusting a chat-only summary."],
    ], true)),
    section("Guide map", groups.map((group) => [
      '<section class="docs-category" aria-labelledby="category-' + escapeHtml(group.slug) + '">',
      '<h3 id="category-' + escapeHtml(group.slug) + '"><a href="' + categoryUrl(group) + '">' + escapeHtml(group.title) + "</a></h3>",
      "<p>" + escapeHtml(group.description) + "</p>",
      '<ul class="docs-page-list compact">',
      pages.filter((page) => page.group === group.title).map((page) => '<li><a href="' + pageUrl(page) + '"><span>' + escapeHtml(page.title) + "</span><small>" + escapeHtml(page.summary) + "</small></a></li>").join(""),
      "</ul></section>",
    ].join("")).join("\n"), "guide-map"),
    section("Browse by task", pageList(topicCategories.map((topic) => ({
      title: topic.title,
      summary: topic.description,
      href: topicUrl(topic),
    })))),
    section("Product boundaries", list([
      "<strong>Not a hosted replacement for your host environment</strong>: your configured provider CLIs and project work remain on the server you run.",
      "<strong>Not a phone-local terminal</strong>: Android renders a remote PTY; it does not run host commands locally.",
      "<strong>Not a direct-public-development service</strong>: remote use should sit behind a VPN, authenticated tunnel, or HTTPS/WSS proxy with auth enabled.",
    ])),
    "</article>",
  ].join("\n");
  return renderShell("Documentation", "io-workbench installation, web workspace, native desktop, mobile client, remote access, workflow, configuration, and troubleshooting documentation.", "", article);
}

function renderCategory(group) {
  const groupPages = pages.filter((page) => page.group === group.title);
  const article = [
    '<article id="docs-content" class="docs-content docs-article docs-category-page" data-page-slug="category-' + escapeHtml(group.slug) + '" data-page-group="Documentation area">',
    '<div class="docs-heading"><p class="docs-eyebrow">Documentation area</p><h1>' + escapeHtml(group.title) + "</h1><dl class=\"docs-meta\"><div><dt>Guides</dt><dd>" + groupPages.length + "</dd></div><div><dt>Docs version</dt><dd>" + docsVersion + "</dd></div></dl></div>",
    lead(escapeHtml(group.description)),
    section("Guides in this area", pageList(groupPages.map((page) => ({
      title: page.title,
      summary: page.summary,
      href: pageUrl(page),
    })))),
    "</article>",
  ].join("\n");
  return renderShell(group.title, group.description, "category-" + group.slug, article);
}

function renderTopic(topic) {
  const topicPages = pages.filter((page) => page.topics.includes(topic.title));
  const relatedTopics = topicCategories
    .filter((item) => item.slug !== topic.slug)
    .filter((item) => topicPages.some((page) => page.topics.includes(item.title)));
  const article = [
    '<article id="docs-content" class="docs-content docs-article docs-category-page" data-page-slug="topic-' + escapeHtml(topic.slug) + '" data-page-group="Topic">',
    '<div class="docs-heading"><p class="docs-eyebrow">Topic</p><h1>' + escapeHtml(topic.title) + "</h1><dl class=\"docs-meta\"><div><dt>Guides</dt><dd>" + topicPages.length + "</dd></div><div><dt>Docs version</dt><dd>" + docsVersion + "</dd></div></dl></div>",
    lead(escapeHtml(topic.description)),
    section("Guides for this topic", pageList(topicPages.map((page) => ({
      title: page.title,
      summary: page.summary,
      href: pageUrl(page),
    })))),
    relatedTopics.length
      ? section("Related topics", pageList(relatedTopics.map((item) => ({
        title: item.title,
        summary: item.description,
        href: topicUrl(item),
      }))))
      : "",
    "</article>",
  ].join("\n");
  return renderShell(topic.title, topic.description, "topic-" + topic.slug, article);
}

function buildSearchIndex() {
  const records = pages.map((page) => ({
    title: page.title,
    category: page.group,
    type: page.type,
    summary: page.summary,
    href: pageUrl(page),
    keywords: page.keywords + " " + page.topics.join(" "),
    headings: [...page.body.matchAll(/<h[23][^>]*>(.*?)<\/h[23]>/g)].map((match) => plainText(match[1])),
    excerpt: plainText(page.body).slice(0, 420),
  }));

  return {
    generatedAt: "2026-09-01",
    docsVersion,
    pages: [
      {
        title: "Documentation",
        category: "Product documentation",
        type: "Main page",
        summary: "Install and operate io-workbench from Web, native desktop, and mobile clients.",
        href: "/docs/",
        keywords: "documentation overview install web native desktop mobile remote workbench",
        headings: ["Start with the path you need", "How the product fits together", "Guide map", "Browse by task", "Product boundaries"],
        excerpt: "io-workbench is a self-hosted remote workbench for configured agent CLIs, files, Git, databases, terminals, and reviewable delivery work.",
      },
      ...records,
      ...groups.map((group) => ({
        title: group.title,
        category: "Documentation area",
        type: "Category",
        summary: group.description,
        href: categoryUrl(group),
        keywords: group.title.toLowerCase() + " documentation category",
        headings: ["Guides in this area"],
        excerpt: group.description,
      })),
      ...topicCategories.map((topic) => ({
        title: topic.title,
        category: "Topic",
        type: "Topic",
        summary: topic.description,
        href: topicUrl(topic),
        keywords: topic.title.toLowerCase() + " documentation topic",
        headings: ["Guides for this topic", "Related topics"],
        excerpt: topic.description,
      })),
    ],
  };
}

function writeOutput(relativePath, value) {
  const target = join(outputDirectory, relativePath);
  const output = value.replace(/[ \t]+$/gm, "").replace(/\n*$/, "\n");

  if (checkMode) {
    let existing;
    try {
      existing = readFileSync(target, "utf8");
    } catch {
      console.error("Generated docs are missing: " + relativePath);
      process.exitCode = 1;
      return;
    }

    if (existing !== output) {
      console.error("Generated docs are out of date: " + relativePath + ". Run node scripts/generate-docs.mjs.");
      process.exitCode = 1;
    }
    return;
  }

  mkdirSync(dirname(target), { recursive: true });
  writeFileSync(target, output);
}

writeOutput("index.html", renderHome());
pages.forEach((page) => {
  writeOutput(page.slug + "/index.html", renderShell(page.title, page.summary, page.slug, renderArticle(page)));
});
groups.forEach((group) => {
  writeOutput("category/" + group.slug + "/index.html", renderCategory(group));
});
topicCategories.forEach((topic) => {
  writeOutput("topic/" + topic.slug + "/index.html", renderTopic(topic));
});
writeOutput("search-index.json", JSON.stringify(buildSearchIndex(), null, 2) + "\n");

if (checkMode && !process.exitCode) {
  console.log("Generated docs are up to date.");
}

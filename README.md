# io-workbench

`io-workbench` is the Rust rewrite scaffold for the workspace server and client suite described in `plan.md`.

Landing page and product documentation: <https://workbench.giofahreza.com>.

The current implementation establishes the Rust foundation and several core workflows:

- Rust workspace with `iowb-*` crates.
- `io-workbench` and `iowb` binaries.
- Axum/Tokio server.
- `/health`, `/ws`, and initial `/api/*` route families.
- Shared Serde protocol types in `iowb-protocol`.
- SQLite initialization under `~/.io-workbench/io-workbench.db`.
- Single-user setup/login with bcrypt password hashes and hashed bearer tokens.
- User-scoped settings persistence for notification preferences, sidebar active sessions, direct-AI config, API keys, and credentials.
- Project indexing, file tree/read/write/create/rename/delete/upload APIs, image upload data URIs, and safe workspace path validation.
- Debounced filesystem watchers for indexed project roots, with WebSocket project-list broadcasts after changes.
- Supervised process start/list/abort with bounded stdout/stderr streaming and bounded stdin writes over REST or WebSocket.
- PTY-backed shell sessions with input, resize, output streaming, and abort support.
- Session message persistence, active-session tracking, session rename/model metadata, and persisted conversation search.
- Agent provider orchestration for Claude, Codex, and Gemini through supervised CLI processes, WebSocket start/abort commands, reconnect replay, and persisted assistant output.
- Git workspace tools for init/status/diff/file diff/commit history/branches/remotes/fetch/pull/push/publish/discard/delete-untracked, GitHub clone workspace creation, and optional Direct AI commit-message generation.
- Database workspace APIs for saved connections, SQLite/PostgreSQL/MySQL/MariaDB connection tests, explorer nodes, object details, SQL query execution, paginated table data, cross-database table transfer, and JSON import/export jobs.
- User Git identity settings, onboarding state, provider CLI status checks, and Claude/Codex token usage parsing.
- MCP server process start/stop/list endpoints plus command-backed MCP tool execution.
- Command-backed slash command, plugin install/remove/run, Taskmaster run, agentic board, push notification, and MCP utility endpoints with persisted command history.
- Settings-backed compatibility endpoints for provider config, external agent REST config, and legacy state paths.
- Optional command-backed audio transcription through `IO_WORKBENCH_TRANSCRIBE_COMMAND`.
- Runtime metrics endpoint for memory, active process, project, session, and configured limit snapshots.
- Legacy data import command for copying old `~/.web-ai-cli` data into the new config directory without modifying the source.
- Static remote mobile PWA package and desktop launcher package metadata under `apps/`.
- Embedded static UI served by the Rust binary with project/workspace creation, file browse/edit/create/rename/delete/upload, chat, shell/process, session search/messages/model/rename, git, database, MCP/tool-runner/audio transcription, metrics, and settings views.
- Tag-driven GitHub Release packaging for signed Android APKs and native Linux,
  macOS, and Windows host binaries.

Larger workflows from the reference app that still need deeper orchestration:

- Rich editor/terminal widgets, diff viewers, markdown rendering, and list virtualization in the browser UI.
- App-store distribution and signed packaging for the separate C++/wxWidgets
  desktop control surface.
- Live PostgreSQL/MySQL/MariaDB integration coverage requires external database services in CI.

AI commit message generation uses Direct AI when configured and falls back to a local deterministic message when Direct AI is off or unavailable.

## Run

```sh
cargo run -p iowb-cli --bin io-workbench -- start
```

Then open:

```text
http://127.0.0.1:8787
```

Useful commands:

```sh
cargo run -p iowb-cli --bin io-workbench -- status
cargo run -p iowb-cli --bin io-workbench -- sandbox /path/to/project
cargo run -p iowb-cli --bin io-workbench -- import-legacy --dry-run
cargo run -p iowb-cli --bin iowb -- version
```

## Install a released host

Each version Git tag, such as `v0.1.0`, publishes checksum-verified host
packages for Linux, macOS, and Windows. The released `io-workbench` binary hosts the Web UI, project
workspace, provider CLIs, database tools, and PTY on that computer; it is not
the separate source-built wxWidgets desktop client.

Linux or macOS:

```sh
curl -fsSL https://github.com/giofahreza/io-workbench/releases/latest/download/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://github.com/giofahreza/io-workbench/releases/latest/download/install.ps1 | iex
```

Both installers verify the selected release archive, install only for the
current user, and do not start a background service. Start the local,
authenticated host explicitly when ready:

```sh
io-workbench start
```

Then open `http://127.0.0.1:8787` and finish first-user setup. Android
releases provide signed `arm64-v8a` (physical phones) and `x86_64`
(emulators) APKs; Android is a remote client and connects to a running host.
Use the GitHub Release assets and `SHA256SUMS` for a manual download. The
complete device-by-device guide, including Android browser/ADB installation,
updates, and GitHub build-attestation verification, is available at
`/docs/install-and-update/` on the landing site.

## Configuration

Environment variables use the `IO_WORKBENCH_` prefix:

- `IO_WORKBENCH_HOST`
- `IO_WORKBENCH_PORT`
- `IO_WORKBENCH_CONFIG_DIR`
- `IO_WORKBENCH_DATABASE_PATH`
- `IO_WORKBENCH_WORKSPACE_ROOT`
- `IO_WORKBENCH_AUTH_REQUIRED` (defaults to `true`; set to `false` only for a trusted local dev server)
- `IO_WORKBENCH_TOKEN`
- `IO_WORKBENCH_MAX_SESSIONS`
- `IO_WORKBENCH_MAX_SCAN_DEPTH`
- `IO_WORKBENCH_MAX_FILE_READ_BYTES`
- `IO_WORKBENCH_AGENT_COMMAND`
- `IO_WORKBENCH_AGENT_ARGS_JSON`
- `IO_WORKBENCH_AGENT_STDIN`
- `IO_WORKBENCH_TOOL_TIMEOUT_SECS`
- `IO_WORKBENCH_DATABASE_TRANSFER_MAX_ROWS`
- Direct AI provider keys such as `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `MINIMAX_API_KEY`, or `CODEX_GATEWAY_KEY`
- `IO_WORKBENCH_TRANSCRIBE_COMMAND`
- `IO_WORKBENCH_TRANSCRIBE_ARGS_JSON`
- `IO_WORKBENCH_MCP_SERVER_COMMAND`
- `IO_WORKBENCH_MCP_SERVER_ARGS_JSON`
- `IO_WORKBENCH_MCP_COMMAND`
- `IO_WORKBENCH_MCP_ARGS_JSON`
- `IO_WORKBENCH_MCP_UTILS_COMMAND`
- `IO_WORKBENCH_MCP_UTILS_ARGS_JSON`
- `IO_WORKBENCH_COMMANDS_COMMAND`
- `IO_WORKBENCH_COMMANDS_ARGS_JSON`
- `IO_WORKBENCH_PLUGIN_COMMAND`
- `IO_WORKBENCH_PLUGIN_ARGS_JSON`
- `IO_WORKBENCH_TASKMASTER_COMMAND`
- `IO_WORKBENCH_TASKMASTER_ARGS_JSON`
- `IO_WORKBENCH_DANGER_COMMAND`
- `IO_WORKBENCH_DANGER_ARGS_JSON`
- `IO_WORKBENCH_PUSH_COMMAND`
- `IO_WORKBENCH_PUSH_ARGS_JSON`

Authentication starts in setup mode when no local user exists. After registration, protected REST routes and WebSocket upgrades require a bearer token. `IO_WORKBENCH_TOKEN` can be used as an installation token for local automation.

Defaults:

- Config directory: `~/.io-workbench`
- Database: `~/.io-workbench/io-workbench.db`
- Server: `127.0.0.1:8787`
- Workspace root: user home directory

Agent commands default to the installed provider CLIs:

- Claude: `claude --print {prompt}`
- Codex: `codex exec {prompt}`
- Gemini: `gemini --prompt {prompt}`

Use `IO_WORKBENCH_AGENT_COMMAND` and `IO_WORKBENCH_AGENT_ARGS_JSON` to override all providers, or provider-specific variants such as `IO_WORKBENCH_CODEX_COMMAND` and `IO_WORKBENCH_CODEX_ARGS_JSON`. Argument templates support `{prompt}`, `{session_id}`, and `{model}`. Set `IO_WORKBENCH_AGENT_STDIN=true` or a provider-specific `IO_WORKBENCH_CODEX_STDIN=true` when the prompt should be written to stdin instead of passed as an argument.

Command-backed tool endpoints accept a request-level `command`/`args` override or use the matching environment variables above. Argument templates support `{action}`, `{namespace}`, `{payload_path}`, and `{payload_json}`. Default args are `["{action}", "{payload_path}"]`.

## Package

Build the single server binary:

```sh
cargo build --release -p iowb-cli --bin io-workbench
```

Build and run the Docker image:

```sh
docker build -t io-workbench .
docker run --rm -p 8787:8787 -v iowb-data:/data -v "$PWD:/workspace" io-workbench
```

Run the desktop launcher after a release build:

```sh
apps/desktop/io-workbench-desktop.sh
```

Build desktop release archives:

```sh
apps/desktop/package-release.sh
```

Build the native C++/wxWidgets desktop client:

```sh
cmake -S apps/desktop/native -B apps/desktop/native/build
cmake --build apps/desktop/native/build
```

Build the mobile PWA archive:

```sh
apps/mobile/package-pwa.sh
```

## Workspace Layout

```text
crates/
  iowb-cli        CLI binaries
  iowb-server     Axum HTTP/WebSocket server
  iowb-core       App state and long-lived managers
  iowb-protocol   Shared API and WebSocket types
  iowb-storage    SQLite persistence
  iowb-fs         File tree, file IO, path validation
  iowb-process    Tokio process supervision
  iowb-ui         Embedded static UI assets
apps/
  mobile          Static remote mobile PWA package
  desktop         Desktop launcher, release archives, and package metadata
```

## Validate

```sh
cargo fmt --check
cargo check --workspace
cargo test --workspace
node --check crates/iowb-ui/static/app.js
node --check apps/mobile/www/app.js
node --check apps/mobile/www/sw.js
```

# io-workbench Rust Rewrite Plan

## Goal

Rewrite the current `web-ai-cli` concept as `io-workbench`: a faster, lighter Rust-first workspace server and client suite that keeps the same user workflow.

The rewrite is mainly motivated by runtime and packaging improvements:

- Lower idle memory than Node/Express.
- Better long-running WebSocket and task handling.
- Cheaper file watching, project scanning, and indexing.
- Better process supervision for agent CLI sessions.
- Better streaming and backpressure control.
- Single compiled binary instead of requiring a Node runtime and npm tree.
- Cleaner async runtime with `tokio`.
- Easier packaging for self-hosted and cloud servers.
- Full removal of old `cloudcli`, `web-ai-cli`, and `claude-code-ui` branding.

## Product Principle

Keep the current app flow and concept:

- Start a local or remote workspace server.
- Open a browser, desktop client, or mobile client.
- Browse projects and sessions.
- Chat with agent sessions.
- Inspect and edit files.
- Use Git tools.
- Use terminal/process output.
- Use settings, credentials, and workspace configuration.
- Use mobile as a remote client to the running server.

Change the implementation and branding, not the core workflow.

## New Product Identity

Use `io-workbench` everywhere as the public product name.

Recommended naming:

- Binary: `io-workbench`
- Short alias: `iowb`
- Config directory: `~/.io-workbench`
- Database path: `~/.io-workbench/io-workbench.db`
- Environment prefix: `IO_WORKBENCH_`
- Server name: `io-workbench server`
- Mobile app name: `io-workbench`

Remove old public names before release:

- `cloudcli`
- `CloudCLI`
- `Web AI CLI`
- `claude-code-ui`
- old package/repository references unless kept only for migration notes

## Architecture

Build one Rust workspace with shared protocol and runtime crates.

```text
io-workbench/
  Cargo.toml
  crates/
    iowb-cli/        # binary: io-workbench / iowb
    iowb-server/     # axum + tokio HTTP/WebSocket/static serving
    iowb-core/       # app state, projects, sessions, config, permissions
    iowb-protocol/   # shared serde API and websocket event types
    iowb-storage/    # SQLite persistence
    iowb-fs/         # project scanning, indexing, file watching
    iowb-process/    # process and PTY supervision
    iowb-ui/         # Rust frontend app
  apps/
    mobile/          # mobile packaging and remote server setup
    desktop/         # optional desktop packaging later
```

## Runtime Model

The Rust server is the source of truth.

```text
io-workbench start
  -> starts the Rust server
  -> serves the UI
  -> exposes /api/*
  -> exposes /ws
  -> manages project state, sessions, files, Git, database, and processes
```

Clients connect to the server:

```text
browser UI     -> HTTP + WebSocket -> Rust server
mobile app     -> HTTPS/WSS        -> Rust server
desktop client -> HTTP + WebSocket -> Rust server
```

Mobile should remain a remote client. It should not run agent CLIs locally.

## Recommended Stack

Backend and runtime:

- `tokio` for async runtime.
- `axum` for HTTP routes and WebSocket handling.
- `tower-http` for middleware, tracing, CORS, compression, and static serving.
- `serde` for typed request, response, and event payloads.
- `clap` for CLI parsing.
- `tracing` for structured logging.
- `notify` for filesystem watching.
- `sqlx` or `rusqlite` for SQLite.
- `tokio::process` for child process control.
- `portable-pty` for terminal/PTY support.
- `include_dir` or `rust-embed` for embedding built UI assets into the binary.

Frontend:

- Prefer a Rust-owned frontend using Dioxus for web/mobile/desktop direction.
- Keep hard widgets isolated behind interfaces.
- Allow selective use of mature browser widgets where needed, especially editor and terminal widgets.

Hard frontend areas to treat carefully:

- Code editor.
- Terminal.
- Diff viewer.
- Markdown rendering.
- Long chat/session virtualization.
- File tree virtualization.

## Core Runtime Components

Use explicit long-lived managers instead of route-local mutable behavior.

```text
AppState
  ConfigManager
  AuthManager
  SessionManager
  TaskManager
  ProcessManager
  ProjectIndex
  WatchManager
  WsHub
  Storage
```

Each active session or task should be supervised by a Tokio task.

Use bounded channels for all streaming paths:

```text
websocket reader
  -> bounded command channel
  -> session/process actor
  -> bounded event channel
  -> websocket writer
```

This is important for predictable memory usage, cancellation, cleanup, and backpressure.

## API Strategy

Keep the existing REST/WebSocket flow where it still matches the product.

Initial route families:

- `/health`
- `/api/auth/*`
- `/api/projects/*`
- `/api/sessions/*`
- `/api/git/*`
- `/api/settings/*`
- `/api/database/*`
- `/api/process/*`
- `/ws`

Define all request, response, and WebSocket event payloads in `iowb-protocol`.

Generate or share client types from Rust structs so the frontend and backend cannot drift.

## CLI Commands

Initial commands:

```text
io-workbench
io-workbench start
io-workbench status
io-workbench sandbox <project-path>
io-workbench version
io-workbench help
```

Alias:

```text
iowb
```

Compatibility aliases can exist during migration only, but new documentation should use `io-workbench` and `iowb`.

## Migration Strategy

Do not port old files line-by-line. Port workflows.

### Phase 1: Foundation

- Create Rust workspace.
- Add CLI binary.
- Add server skeleton.
- Add app config loading.
- Add structured logging.
- Add health endpoint.
- Add embedded static UI serving.

### Phase 2: Branding

- Replace public product name with `io-workbench`.
- Define new config directory and env var names.
- Add migration/import path from old `~/.web-ai-cli` data if needed.
- Remove old package and README naming from the new codebase.

### Phase 3: Storage and Auth

- Implement SQLite schema.
- Implement local user/auth flow.
- Implement token handling.
- Implement settings persistence.
- Add database migrations.

### Phase 4: Projects and Files

- Port project discovery.
- Port file tree APIs.
- Port file read/write/create/rename/delete/upload flows.
- Add project scanning cache.
- Add filesystem watcher with debounced updates.

### Phase 5: WebSocket and Sessions

- Define typed WebSocket protocol.
- Implement connection auth.
- Implement reconnect behavior.
- Implement session lifecycle tracking.
- Implement bounded streaming channels.
- Implement cancellation and cleanup.

### Phase 6: Process and PTY Runtime

- Implement process supervisor.
- Implement PTY sessions.
- Implement stdout/stderr/event streaming.
- Implement process cancellation.
- Implement orphan cleanup.
- Add limits for buffers, task count, and idle sessions.

### Phase 7: Git and Workspace Tools

- Port Git status/diff/branch/commit/pull/push flows.
- Prefer Rust Git libraries where practical.
- Shell out to `git` only where it is simpler or more compatible.

### Phase 8: Database and Advanced Tools

- Port database connection manager.
- Port query and table browsing flows.
- Port import/export jobs.
- Port long-running job tracking.

### Phase 9: Frontend Replacement

- Build the new `io-workbench` UI shell.
- Keep the same major workflows and navigation.
- Rebuild views around the new typed protocol.
- Optimize long lists and streaming output from the start.
- Use mature editor/terminal widgets if pure Rust replacements are not good enough.

### Phase 10: Mobile

- Build mobile app as a remote client.
- First launch asks for server URL.
- Store server URL and auth token securely.
- Use HTTPS/WSS for remote connections.
- Support reconnect and offline error states.
- Add file picker/share integration.
- Add push notifications later if needed.

### Phase 11: Packaging

- Produce a single server binary.
- Package Docker image with minimal runtime.
- Package Linux/macOS/Windows binaries.
- Package mobile apps separately.
- Include built UI assets in the server binary or ship them next to the binary.

## Performance Rules

- Prefer bounded queues over unbounded channels.
- Put explicit limits on log buffers, process output buffers, upload size, project scan depth, and concurrent tasks.
- Avoid full project rescans when watcher deltas are enough.
- Use cancellation tokens for every long-running task.
- Make websocket writers tolerant of slow clients.
- Drop or compact old streaming events where the UI can reload durable state.
- Keep database writes batched where practical.
- Measure idle memory, active session memory, websocket fanout, scan time, and process cleanup behavior.

## Compatibility Rules

- Preserve the current user workflow.
- Preserve old data through import/migration where valuable.
- Do not preserve old naming in the new product UI.
- Do not require Node at runtime.
- Do not require mobile devices to run agent CLIs locally.
- Do not rewrite external agent CLIs; only supervise and integrate them.

## Definition of Done

The rewrite is successful when:

- Users can run `io-workbench start` and use the app the same way as before.
- The server runs as a single compiled binary.
- The app no longer requires Node/npm at runtime.
- Idle memory is materially lower than the current Node/Express server.
- Long-running sessions survive reconnects and clean up correctly.
- Project scanning and watchers are cheaper and debounced.
- Process output streaming has bounded memory behavior.
- Mobile can connect to a remote server and use the same workflows.
- Public branding is fully `io-workbench`.

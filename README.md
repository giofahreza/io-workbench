# io-workbench

> A self-hosted remote development command center for Claude, Codex, and Gemini—alongside your code, Git, databases, and a live terminal.

[Website](https://workbench.giofahreza.com) · [Documentation](https://workbench.giofahreza.com/docs/) · [Releases](https://github.com/giofahreza/io-workbench/releases)

io-workbench turns the machine where your projects live into a remote, authenticated workspace. Run your configured agent CLIs against the same project context, inspect files and Git changes, work with databases, and use a real server-hosted PTY from the browser, desktop, or mobile client.

## Why io-workbench

A remote agent chat is useful, but delivery work also touches source code, branches, rows, shell output, and validation. io-workbench keeps those surfaces connected instead of splitting them between unrelated tools.

- Choose Claude, Codex, or Gemini for a session while keeping project context and history together.
- Review files, diffs, branches, remotes, commits, and workspace changes beside the agent conversation.
- Use a Navicat-style database workspace for SQLite, PostgreSQL, MySQL, and MariaDB.
- Run and control a live PTY terminal; Android includes touch-friendly terminal controls and structured database row copy/paste.
- Plan and review work with the Agentic Board, validation steps, and durable session history.

## Install

Released Linux, macOS, and Windows archives contain the `io-workbench` host binary. Android releases are signed remote-client APKs that connect to a running host.

### Linux and macOS

```sh
curl -fsSL https://github.com/giofahreza/io-workbench/releases/latest/download/install.sh | sh
```

When run from a terminal, the installer asks you to review the bind address and port, configuration/data path, workspace boundary, opt-in Linux systemd or macOS LaunchAgent startup, optional IO Gateway, and optional Codex/Claude/Gemini CLI setup. It can run offline version/auth readiness checks by default after you select a provider, while a real read-only model request is always opt-in because it can use quota. It defaults to an authenticated loopback host with manual startup; it never asks for provider credentials. In noninteractive use, it installs only the verified binary unless you explicitly select more. A managed macOS startup item can be removed safely with `curl -fsSL https://github.com/giofahreza/io-workbench/releases/latest/download/install.sh | sh -s -- --disable-autostart`.

### Windows PowerShell

```powershell
irm https://github.com/giofahreza/io-workbench/releases/latest/download/install.ps1 | iex
```

The Windows installer offers the same safe host and optional CLI/Gateway choices, using a per-user Scheduled Task for opt-in startup instead of systemd.

### Android

Download the appropriate signed APK (`arm64-v8a` for most phones, `x86_64` for emulators) from the [latest release](https://github.com/giofahreza/io-workbench/releases/latest). The [install and update guide](https://workbench.giofahreza.com/docs/install-and-update/) covers checksums, updates, ADB, and browser/PWA options.

## Start a host

```sh
io-workbench start
```

Open <http://127.0.0.1:8787>, complete first-user setup, add a project, and confirm the provider CLIs you want to use are installed and authenticated on that host.

After a first install or upgrade, run `io-workbench doctor --require-running` for a repair-oriented local check. Use `io-workbench doctor --json --require-running` in automation, or `io-workbench setup` to print the non-mutating first-run checklist.

For remote access, put the server behind your own VPN, authenticated reverse proxy, or tunnel with HTTPS/WSS support. `workbench.giofahreza.com` is the public GitHub Pages landing and docs site—not an io-workbench API or host endpoint.

## Build from source

```sh
git clone --recurse-submodules https://github.com/giofahreza/io-workbench.git
cd io-workbench
cargo run -p iowb-cli --bin io-workbench -- start
```

Build the release binary with:

```sh
cargo build --release -p iowb-cli --bin io-workbench
```

## Documentation

The hosted documentation has complete installation and workflow guides for web, desktop, Android, and PWA clients:

- [Get started](https://workbench.giofahreza.com/docs/quick-start/)
- [Install and update](https://workbench.giofahreza.com/docs/install-and-update/)
- [Web workspace](https://workbench.giofahreza.com/docs/web-workspace/)
- [Mobile clients](https://workbench.giofahreza.com/docs/mobile/)
- [Deployment and recovery](https://workbench.giofahreza.com/docs/deployment-and-recovery/)

Repository design and operator material lives in [`docs/`](docs/):

- [Production deployment](docs/deployment.md)
- [Rust rewrite architecture](docs/architecture/rust-rewrite-plan.md)
- [Agentic Board / Kanban design](docs/design/kanban.md)
- [Codex app-server migration plan](docs/migrations/codex-app-server.md)

## Project layout

```text
crates/
  iowb-cli        CLI binaries
  iowb-server     Axum HTTP/WebSocket server
  iowb-core       App state and long-lived managers
  iowb-protocol   Shared API and WebSocket types
  iowb-storage    SQLite persistence
  iowb-fs         File tree, file IO, and path validation
  iowb-process    Tokio process supervision
  iowb-ui         Embedded static UI assets
apps/             Mobile and desktop clients (submodule)
docs/             Repository architecture, design, migration, and deployment docs
```

## Verify a source checkout

```sh
cargo fmt --check
cargo check --workspace
cargo test --workspace
node scripts/generate-docs.mjs --check
```

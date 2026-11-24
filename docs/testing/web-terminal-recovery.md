# Web terminal recovery verification

Validated on the affected Linux host on 2026-11-24. Tests use isolated server
ports, projects, and configuration directories, not the live Workbench database.

## Root cause and repair

The browser originally launched Bash, whose user startup file executed
`~/.local/bin/zsh`. That launcher selected a private Zsh 5.9 binary whose
compiled module directory no longer existed. The intact system Zsh package is
5.9.2. After backing up the original symlink, the host launcher now points to
`/usr/bin/zsh`. No shell startup file, completion configuration, or theme was
disabled or rewritten.

The application fixes separately address browser-selected host shells, unrelated
process output entering the terminal, fast exits before the start response,
reconnect replay duplication, stale attachment requests, and final-tail loss.
See [the lifecycle/API contract](../architecture/terminal-lifecycle.md).

## Interactive browser verification

Chromium exercised the actual application DOM, xterm, HTTP routes, WebSocket,
and native PTYs. Both the normal configured shell and explicit safe terminal
were tested. A separate host-specific run loaded the real user's Bash and Zsh
startup files against an isolated release server.

| Check | Observed result |
|---|---|
| Real Bash startup hands off through the repaired user-local launcher | Zsh reports `5.9.2`, with the normal prompt/theme |
| `zmodload zsh/parameter zsh/zle zsh/datetime zsh/terminfo` | All modules load; no missing-module, `zle`, `bindkey`, or math-expression errors |
| Type commands through the browser terminal | Expected output and selected host working directory |
| Run `sleep 20`, then press Ctrl+C | Sleep is interrupted; the next typed command succeeds |
| Start another process that prints and exits | Its output and exit never alter the selected terminal |
| Replace the WebSocket connection | Retained/live output remains ordered and appears once |
| Block WebSockets while keeping HTTP available | Keyboard input and polled output continue working |
| Clear, then refresh/reconnect | Consumed output is not replayed onto the cleared screen |
| Explicit safe restart after an invalid configured shell | A profile-free shell accepts commands; the old process is stopped |
| Exit with a nonzero code or press Stop | Final output precedes the exit marker; controls report exited, without an automatic restart loop |

Live testing initially exposed a second input-order defect: each fallback
keystroke created an independent HTTP request. Concurrent requests could reorder
typed commands. Input now uses a per-attachment serialized queue, with a
regression that deliberately holds the first request. The full browser-to-PTY
scenario passes after that correction. Inspection also caught a missing safe
terminal icon definition; the browser regression now checks its rendered mask.

## Automated coverage

- `iowb-process`: 16 passing tests, including shell selection/arguments, literal
  executable paths, safe recovery, fast pipe/PTY exits, retention bounds, cursor
  semantics, and evicted-ring watermarks.
- `terminal-api.test.cjs`: four actual-server tests for host shell startup,
  input/resize/exit, retained fast exits, safe environment handling, authentication,
  working-directory boundaries, and unchanged arbitrary process starts.
- `terminal-replay.test.cjs`: actual PTY/WebSocket output identities survive late
  subscription and reconnect.
- Web `terminal-browser.test.cjs`: nine controlled-I/O browser regressions for
  interleaving, stale selections/starts, clear, safe/Stop controls, reconnect,
  HTTP input serialization, ring gaps, and expired process IDs.
- Server `terminal-browser.test.cjs`: two actual browser/server/PTY acceptance
  scenarios, including HTTP-only fallback and recovery from an invalid profile.
- The clean-source Web regression suites pass 85 tests, including existing chat
  history, scrolling, focus, provider-readiness, and viewport coverage.
- The clean-source Rust core, process, protocol, server, storage, TUI, and UI
  library suites pass 1,104 tests; one pre-existing test remains ignored.

The broader Rust run uses clean committed source, temporary HOME/TMPDIR, no
inherited provider credentials, non-networking provider stubs, and the system
Git. This matters on this host: its user-local Git wrapper depends on the real
HOME and cannot run under a temporary HOME. Neither failed test-harness setup
was treated as an application regression or used to justify unrelated edits.

Reproduce the terminal acceptance tests with a built binary and an installed
Playwright/Chromium pair:

```sh
cargo test -p iowb-process -p iowb-protocol -p iowb-server -p iowb-ui
cargo build --release -p iowb-cli --bin io-workbench
IOWB_TEST_BINARY="$PWD/target/release/io-workbench" \
PLAYWRIGHT_MODULE=/path/to/playwright \
node --test crates/iowb-server/tests/terminal-api.test.cjs \
  crates/iowb-server/tests/terminal-replay.test.cjs \
  crates/iowb-server/tests/terminal-browser.test.cjs \
  crates/iowb-ui/tests/terminal-browser.test.cjs
```

The new terminal browser suites also accept `CHROME_EXECUTABLE` for an explicitly
installed Chromium executable. No paid provider request is needed.

## Deployment boundary

Build the installed artifact from a clean worktree of the committed terminal
changes, not the developer's unrelated pending slash-command or desktop-cleanup
edits. The machine's managed executable is `/opt/io-workbench/io-workbench`; the
service is `io-workbench-8100.service`. Preserve its existing configuration,
database, and a rollback binary. After the managed restart, verify service
health, the executable checksum, and Web asset version
`20261124-terminal-01`. Record the result in deployment metadata rather than
assuming that copying a binary also replaced the running process.

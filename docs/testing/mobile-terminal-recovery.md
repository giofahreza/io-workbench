# Mobile terminal recovery validation

Validated on 2026-11-23 (Asia/Jakarta).

## Changes and regression coverage

| Problem | Behavior after the fix | Verification |
|---|---|---|
| Firebase configuration is absent from the APK | Skip optional push registration and continue connecting | Android emulator log confirms registration was skipped; terminal and WebSocket work |
| WebSocket is unavailable when opening Terminal | Create the PTY through HTTP and subscribe when the stream recovers | Native Android UI created exactly one process with WebSockets blocked; restored connection recovered its prompt |
| Socket connects before the delayed process-start response supplies its ID | Refresh the subscription after receiving the ID, including an already healthy socket | Native Android UI with a five-second delayed POST response; initial prompt appeared without another reconnect |
| Replay overlaps live output or a replacement connection | Merge by immutable per-process sequence, retaining order and real repeated output | Server PTY/WebSocket integration test, shared controller tests, native command input and reconnect |
| Replay includes chunks already cleared or trimmed | Keep a sequence floor and reject discarded chunks | Shared controller tests exercise clear, the 1,000-chunk limit, and subsequent new output |
| Service account advertises `nologin` | Fall back to a usable `sh`; retain valid configured shells | Actual Oracle Linux PTYs as the service user, with valid Bash and `nologin`, in a directory containing spaces |
| Shell path has spaces, is missing, is nonexecutable, names a directory, or contains shell syntax | Quote executable paths, fall back for unusable paths, and avoid evaluating the value as commands | JVM subprocess test; Termux retains its `bash -l` arguments |
| Another chat subscription is active while a terminal starts | Preserve that subscription when adding the terminal ID | Shared controller delayed-start test |
| Older servers omit output sequences | Decode legacy frames and retain repeated unsequenced text | Protocol and shared controller tests |

The delayed-start Android test observed the socket connecting at 18:45:54.660
UTC and the held successful POST response at 18:45:59.462 UTC. The prompt
appeared after that response without forcing a reconnect. Commands entered
through the native terminal (`echo FIXED_REPLAY_ONCE` and `pwd`) succeeded in
the selected project. Screenshots before and after reconnect showed the same
transcript without duplicate replay.

## Automated results

| Suite | Passed | Failed |
|---|---:|---:|
| Rust core, protocol, server, storage, TUI, UI | 1,085 | 0 |
| Rust process | 9 | 0 |
| Web and browser | 76 | 0 |
| Server integration, including terminal replay | 33 | 0 |
| Mobile shared JVM | 807 | 0 |
| Total | 2,010 | 0 |

The full acceptance runner completed successfully. The process tests were run
separately because the active runner started before its package list was
extended; future runs include them. One pre-existing Rust test remains ignored
and the pre-existing Git `FETCH_HEAD` compatibility test remains explicitly
excluded. Neither is included in the passed total. Android compilation and
`assembleDebug` also succeeded.

Reproduce with JDK 21, Android SDK 36, Gradle, Rust 1.93.1, and Playwright with
Chromium installed:

```sh
PLAYWRIGHT_MODULE=/path/to/playwright node scripts/test-provider-acceptance.mjs
cd apps/mobile
gradle --offline :shared:jvmTest :androidApp:assembleDebug
```

Manual Android checks used an Android 16/API 36 ARM64 emulator, the native UI,
an isolated actual Workbench server, and a proxy controlling WebSocket
availability and HTTP response timing. Linux checks used temporary PTYs on
Oracle; the deployed Workbench service was not changed or restarted. These
checks cover terminal recovery, not the remaining provider onboarding/device
cases recorded in `plan.md`.

## Delivered build and compatibility

- ARM64 APK: `0.1.89-terminal-recovery`, version code `100016`.
- Download: https://gofile.io/d/XW040TQe
- Size: 11,495,373 bytes; upload size and MD5 matched the local tested APK.
- SHA-256: `cc260955baf686255b2f75042faae53eb8ecd7edfb4d5512dc73867f20d8dbab`.
- Signing certificate SHA-256: `e11d4f88ea8d840b84f3e4621df12cbffec99ba7a1d13df084d9e82bb5231961`.

The original Linux signing key was unavailable. The user explicitly authorized
uninstalling the previous app and reinstalling a build signed with the existing
Mac key. This APK cannot update an installation signed with the original key;
that installation requires uninstall/reinstall. An update over the emulator's
same-key build was also verified and retained its connection preferences.

Deploy both the updated server and mobile app for sequence-based replay
deduplication. Wire fields remain backward compatible, but older servers do not
provide chunk identities and cannot support this deduplication guarantee.

Local evidence is retained in `/tmp/iowb-terminal-fixed/` (screenshots, proxy
timings, Linux PTY results, and tested APK). Automated logs are
`/tmp/iowb-terminal-fix-regression.log`,
`/tmp/iowb-terminal-fix-process-tests.log`, and
`/tmp/iowb-terminal-fix-mobile.log`.

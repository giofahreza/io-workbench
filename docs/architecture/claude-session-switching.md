# Claude CLI and Workbench session switching

Workbench can continue a closed native Claude conversation and send turns to a
Claude terminal that stays open. Both paths retain the native conversation UUID.
The live path uses Claude's mods API and one native runtime. It does not open a
second `claude --print --resume` process against the live transcript.

## Start a terminal that supports live switching

Run from the project directory, as the same OS user that runs Workbench:

```sh
io-workbench claude
# Or reopen an existing conversation:
io-workbench claude --resume <native-session-uuid>
```

Open that project's Claude conversation in Workbench, choose **Native CLI** and
**Keep native Claude session settings**, and send after the current turn ends.
The terminal displays the same native turns and can accept the following prompt.
Web and mobile use the existing native-session settings and streaming protocol.
Both processes must use the same `CLAUDE_CONFIG_DIR` when it is customized.
Keep one native CLI process per conversation UUID. Opening a second independent
`claude --resume` process does not attach another client to this runtime.

The launcher supplies a local plugin with `--plugin-dir`. It does not install a
global Claude plugin or edit Claude's personal settings. Ordinary Claude launch
arguments are forwarded. `IO_WORKBENCH_CLAUDE_COMMAND` selects the executable.
The embedded plugin is extracted into Workbench's configuration directory.

An already-running plain `claude` process cannot acquire this mod retroactively.
Finish its current turn, exit it, and reopen the same UUID with this launcher.
Workbench refuses an observed separate owner instead of launching another writer.

## Ordinary continuation

After the terminal exits, Workbench can resume the conversation with Claude's
existing print-mode continuation. Claude owns and writes the native transcript.
Original terminal-only flags, ephemeral permissions, loaded modules, or session
state are not universally restored by `--resume`; configure those in the native
terminal and keep it open when exact runtime continuity matters.

The live bridge currently requires Unix. Unbridged continuation checks process
ownership through `/proc` on Linux and Android. On other platforms, continuation
of an imported conversation requires the live bridge because Workbench cannot
reliably rule out another native owner. Linux was tested; other Unix platforms
have not been exercised by the integration harness.

## Delivery and recovery

- The mod spawns a local Workbench helper that listens on an ephemeral IPv4
  loopback port. A per-instance bearer is kept in private registry files. It is
  separate from the Workbench server bearer. Browser origins are refused.
- A stable durable run ID identifies each submission. Retries return its receipt.
  The helper persists intent before dispatch and native turn acceptance before
  acknowledgement. Commands and event batches are deduplicated independently.
  The helper bounds its in-memory receipt cache and reads older receipts from
  disk on demand, preserving deduplication across eviction and restarts.
- Claude skips a mod's own `prompt.submit` hooks. The bridge observes the native
  `session.append` event instead, uses its engine-stamped plugin origin and user
  UUID, and associates the ensuing native `turnId`. It never identifies ownership
  by matching prompt text. This also distinguishes repeated identical prompts
  when merging Workbench history with later terminal turns.
- Workbench restarts reconnect to the existing receipt and observe the accepted
  turn. Missing receipts, helper restarts during delivery, and uncertain transport
  outcomes require inspecting the native conversation; they do not trigger a
  fallback process or automatic resubmission.
- Busy or stopped terminals reject new sends. A definite rejection restores the
  unsent prompt. Cancellation addresses the associated native turn rather than
  sending Escape or aborting another terminal turn.
- `/clear` and `/resume` retire the previous helper and register the new native
  UUID. Requests to a stale conversation never enter its replacement.
- Native files are read through the existing history parser. The bridge does not
  synthesize user or assistant rows inside Claude's transcript.

## Boundaries

This is an integration with Claude's mods API, not an implementation of Claude
Remote Control and not complete CLI feature parity. It was tested with Claude
Code **2.1.332**; older builds or organization policies that disable mods may
prevent the bridge from loading.

Tool permission prompts and interactive questions remain in the terminal. The
bridge does not grant tool permissions. Change permission, thinking, and fast
settings there; Workbench preserves the active native defaults. Explicit model
and effort changes apply to the submitted turn. Raw terminal commands beginning
with `/` or `!` must run in the terminal; supported Workbench skill expansion can
still produce an ordinary prompt. `@file` references and pasted media are not
expanded by the mod's text submission API.

Workbench's live receipt and replay output is bounded (65,536 UTF-16 code units
in the mod, at most 256 KiB in the helper). Claude's original transcript remains
the source for complete history. Receipts contain prompt and response text; keep
the Claude configuration directory private and do not remove receipts for an
active turn.

## Validation

```sh
cargo build -p iowb-cli --bin io-workbench
cargo test -p iowb-core -p iowb-storage -p iowb-server --lib
python3 scripts/test-claude-session-switching.py
```

The integration script runs a real Claude TUI and Workbench against a loopback
Anthropic API fixture with disposable configuration and a dummy key. It verifies
both switching directions, request deduplication, server restart during a turn,
busy rejection, targeted abort, terminal approval routing, helper-loss
uncertainty, `/clear`, `/resume`, and closed-terminal continuation. It does not
send inference to an external provider or use personal provider credentials.

Sources: [Claude mods API](https://code.claude.com/docs/en/plugins/mods/api),
[mods reference](https://code.claude.com/docs/en/plugins/mods/reference), and
[sessions](https://code.claude.com/docs/en/sessions). The implementation was checked
against the API declarations generated by the tested Claude CLI build.

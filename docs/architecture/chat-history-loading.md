# Chat history loading and scroll stability

## Behavior

Older history advances one bounded page per accepted user demand. Scroll/layout
callbacks do not create demand: upward wheel, touch, or keyboard input does, and
an explicit **Load older messages** control also works when a page has no visible
rows. Input received while a page is loading does not queue another page.

The main web client deduplicates in-flight older requests and message IDs, rejects
stale session/generation responses, and prepends rather than rebuilding the whole
transcript. It reconciles the response group crossing the page boundary while
retaining existing message nodes and live-stream nodes.

## Web chat reconciliation

Ordinary two-second activity polls are not session changes. They do not reset the
transcript or discard older pages the reader already loaded. Tail and older-page
requests are single-flight and serialized per selected session. Session changes
invalidate stale responses; another poll does not invalidate an older-page load.

Tail snapshots are merged by persisted message ID and checked against the loaded
range. A disjoint tail or shifted history boundary triggers bounded 30-message
page reads for the covered range. A changed count, invalid page, or session switch
during those reads aborts that commit. This is not an atomic cross-page snapshot:
without a server revision token, arbitrary same-count external edits between
requests cannot be fully detected. Ordinary overlapping appends need no backfill.

Message rows and rendered Markdown blocks are updated in place. Unchanged rows,
the older-history control, disclosure state, focus, and text selection remain
attached. Recovery cards also reuse their nodes. Local prompt/completion IDs are
render identities, not persisted IDs; identical text is never a deduplication key.
Explicit Thinking/Parameters preference changes update existing sections locally,
including live output, without fetching history or resetting the other section
kind. Later stream updates preserve those choices.

WebSocket events own live output. A REST request never restores a stream buffer
captured before its network wait. Event versions protect newer output and terminal
status from stale snapshots. During an active optimistic turn, local presentation
is retained until an authoritative inactive snapshot can replace it safely.
Delayed token-usage replies are bound to their completed message, not the current
stream; they cannot overwrite the following turn's footer or cached live status.

## Web scroll policy

- Native browser scroll anchoring remains enabled.
- Following the latest output is explicit intent, subject to the auto-scroll
  preference. Upward scrolling pauses it, even close to the bottom; returning to
  the bottom or choosing Jump to latest resumes it.
- History commits capture the viewport after network I/O and before DOM changes,
  so scrolling while a request is pending is respected.
- A visible text offset (or message node fallback) anchors reading position. Only
  measured residual movement is corrected, immediately and on the next animation
  frame. Resize observation covers late image sizing and content reflow.
- User input, session changes, and deliberate transcript resets invalidate pending
  corrections. Scrolling inside a nested code block does not count as transcript
  navigation. Layout/resize callbacks never request additional history pages.
- The older-message spinner has a fixed border-box size so entering its loading
  state cannot change the height above the first visible row.

This is incremental rendering, not transcript virtualization. Large loaded
histories still consume DOM and Markdown-rendering work; no universal latency or
cross-browser guarantee is implied.

## Other clients

Android and the Kotlin web harness use `ChatHistoryLoadGate`. Android render-model
equality excludes list position, so prepending messages does not mark unchanged
rows as changed. Assistant expansion identity follows message identity rather
than list position. The harness preserves existing rows for a pure prepend and
falls back to a complete render when the previous suffix changed.

## Backend

- Direct persisted external-history pages use an indexed sequence seek and read
  only the selected message bodies after checking the source fingerprint.
- Compacted histories cache stored, external, and merged message descriptors.
  Warm reads reuse descriptors and fetch only selected message bodies.
- SQLite triggers track per-session history revisions across connections.
  Chronological appends can extend stored descriptors; edits, deletions, and
  backfills invalidate them. External changes invalidate by file fingerprint.
- Mapped external histories cache the complete canonical merge to preserve
  message identities, metadata, and total counts across subsequent pages.
- History cache entries share the existing estimated 64 MiB eviction budget with
  external message caching and have a bounded entry count.

Cold external parsing, the first mapped merge, and rebuilding an invalidated
history can still require a full source scan. These changes eliminate repeated
warm-page materialization and input-free pagination cascades; they do not provide
full web transcript virtualization or a latency guarantee on every device.

## Validation

From the repository root:

```sh
cargo test -p iowb-storage -p iowb-core -p iowb-ui --lib --offline
node --test crates/iowb-ui/tests/chat-history.test.cjs
gradle -p apps/mobile :shared:jvmTest :androidApp:compileDebugKotlin :webHarness:compileKotlinJs --offline
```

The browser regression runs the actual web prepend/render code in desktop and
mobile-sized Chromium viewports. It checks node identity, boundary response
headers/copy text, expanded-state preservation, live-stream preservation, and
viewport drift. Supply an installed Playwright module; `CHROME_EXECUTABLE` is
optional when Playwright's own browser is installed:

```sh
PLAYWRIGHT_MODULE=/absolute/path/to/node_modules/playwright \
CHROME_EXECUTABLE=/absolute/path/to/chrome \
node --test crates/iowb-ui/tests/chat-history-browser.test.cjs
```

The browser test is explicitly skipped when `PLAYWRIGHT_MODULE` is absent.
The dependency-free Node tests cover filtered-page cascades, concurrent requests,
overlapping IDs, stale session responses, failed-request retry behavior, gap
backfills, deletion boundaries, and richer cached message projections.

For the web scroll repair, run the complete web regression suite:

```sh
PLAYWRIGHT_MODULE=/absolute/path/to/node_modules/playwright \
CHROME_EXECUTABLE=/absolute/path/to/chrome \
node --test crates/iowb-ui/tests/*.test.cjs
cargo test -p iowb-ui --lib
git diff --check
```

The full-page fixture loads the real HTML, CSS, application modules, startup,
polling timer, and WebSocket handlers at 390px and 1280px. Only APIs and WebSocket
transport are mocked, with deterministic delayed replies and synthetic messages;
no production sessions or credentials are read. It covers natural polling,
prepend at scrollTop zero, retained loaded history, append gaps, deletions, delayed
images, user movement during requests, streaming races, disclosure/selection
preservation, follow/pause behavior, concurrent paging, and session switches.
Focused renderer/viewport tests supplement these with DOM identity, Markdown
safety, text reflow, and fallback behavior without native anchoring.

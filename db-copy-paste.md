⚠ marks cases where the current implementation has a limitation or a potential data-loss risk that needs particular attention.

## Execution rules and evidence

The original 163 cases and the first 60 adversarial cases retain their IDs.
Section 13 adds **30 more adversarial cases on October 5, 2026**, bringing this
document to **253 case IDs**. All additions **ADV-061–ADV-090 are NOT RUN**.
Consult `db-copy-paste-results.md` for qualified evidence on earlier cases;
API/native checks do not imply that their Android/manual variants passed.
Existing results do not cover additions automatically; add a separate ledger
entry when each new case is attempted.

- Use disposable databases, accounts, connections, tables, and synthetic
  secrets. Never inject outages, revoke permissions, or create destructive
  races against user data or production port8100.
- Pin the app/server build, engine/version, device, endpoint, and fixture for
  each run. Record whether the test uses actual UI actions, API checks, or
  injected faults. Supporting automation does not become a manual UI pass.
- Give input rows unique run/row markers. Capture native database baselines,
  then compare exact persisted values and side effects after the operation.
  A toast or an HTTP success code alone is not an integrity oracle.
- Use barriers or an observable in-flight state for races; a timed sleep alone
  does not prove the race occurred. If the necessary state cannot be reached,
  mark BLOCKED or N/A with evidence instead of assuming the case passed.
- Before retrying an uncertain write, inspect persisted rows/job state. Never
  resubmit an entire batch merely because the UI response was lost.
- Set finite payload, memory, duration, and concurrency budgets for stress
  tests. Increase one dimension at a time; record the largest tested size.
- Restore permissions, settings, clipboard fixtures, and auxiliary services.
  Preserve failure evidence, but do not record real credentials or user data.

The expectations below are acceptance criteria to investigate, **not claims
that every protection already exists**. Missing protection must be recorded
as a gap; unsupported features may pass only by safely rejecting the operation
with an accurate explanation. N/A needs evidence that the feature is absent.

Copy this cycle into the results ledger for **each** new case:

```text
Case ID / variant:
Priority: P0 (wrong writes/privacy), P1 (integrity/recovery), P2 (usability)
Build/device/engine/fixture:
[ ] Preconditions and independent native baseline recorded
[ ] First manual attempt: PASS / FAIL / BLOCKED / N/A + evidence
[ ] If failed: expected vs actual, reproducible steps, root cause recorded
[ ] Fix applied (or explicitly N/A); changed files/build recorded
[ ] Exact failing attempt retested; native values/counts/side effects checked
[ ] Neighboring success, cancel, and recovery paths retested
[ ] If retest fails: open another numbered fix/retest cycle; do not overwrite it
[ ] Fixture/settings restored and final status recorded
```

1. Row selection and copying


    ROW-01 — Single row: Copy one selected row; copied values match the source.

    ROW-02 — Multiple rows: Copy contiguous and noncontiguous selections; only selected rows are included.

    ROW-03 — Selection order: Select rows in reverse/random order; copied rows follow their displayed row order.

    ROW-04 — First/last rows: Copy the first row, last row, and both together.

    ROW-05 — Select all: Copy all visible rows; verify whether selection is page-scoped and that the count is accurate.

    ROW-06 — No selection: Show “Select at least one row to copy”; preserve the previous clipboard.

    ROW-07 — Selection after data changes: Sort, filter, paginate, refresh, or delete rows before copying; stale selection must not copy unintended records.

    ROW-08 — Different sources: Copy from a table, view, SQL result, and Data-panel query result; use the correct result and source metadata.

    ROW-09 — Tab isolation: Select different rows in separate tabs/panels; copying uses the active source’s selection.

    ROW-10 — Empty results: Copy from an empty result or missing column metadata; handle gracefully.

    ROW-11 — Hidden columns: Verify exactly which fields are copied and whether this matches the UI’s promise.

    ROW-12 — Snapshot behavior: Change or delete source records after copying; saved clipboard rows retain their copied values.

    ROW-13 — Replace existing copy: Copy dataset B after dataset A; subsequent paste uses B.


2. Internal clipboard and Android clipboard


    CLIP-01 — Empty state: Neither clipboard contains data; Paste is unavailable.

    CLIP-02 — Internal only: Saved database rows exist while Android clipboard is empty; row paste works.

    CLIP-03 — Android only: No saved database rows; valid Android text is used.

    CLIP-04 — ⚠ Both populated: Saved database rows currently take precedence over newer Android clipboard text; verify the user can identify which source will be pasted.

    CLIP-05 — ⚠ Copy another kind of value: After copying rows, copy a cell, definition, or external text; verify row Paste still using the internal rows does not cause an unexpected import.

    CLIP-06 — Action updates: Copy, clear, or externally replace clipboard data while the screen is open; Paste visibility updates correctly.

    CLIP-07 — Background changes: Change clipboard content in another app, then return; action state reflects the available content.

    CLIP-08 — Blank content: Test empty string, spaces, tabs, and newlines; row Paste treats blank content as empty.

    CLIP-09 — Non-text content: Images, URI-only items, and HTML-only items do not crash or trigger unintended provider reads.

    CLIP-10 — Multiple items: Verify only the first clipboard item is considered; later items are not silently merged.

    CLIP-11 — Clipboard failure: Clipboard service unavailable, access denied, or write failure; report the actual outcome without losing usable internal rows.

    CLIP-12 — Clear: Clear removes internal rows, persisted rows, and Android clipboard content; inspector and actions update.

    CLIP-13 — Partial clear failure: Android clipboard clearing fails; internal rows are cleared and remaining system content is represented accurately.

    CLIP-14 — Persistence: Restart the app or kill its process after copying; saved rows remain usable.

    CLIP-15 — Invalid persistence: Corrupt, empty, or incompatible saved payloads do not crash startup.

    CLIP-16 — Deleted source connection: Clipboard inspection and paste still work from the saved row snapshot.


3. External JSON, CSV, and TSV


    FORMAT-01 — JSON array: Paste one or multiple row objects in an array.

    FORMAT-02 — JSON wrapper: Paste { "rows": [...], "columns": ["id", "name"] }.

    FORMAT-03 — Column objects: Paste a wrapper whose columns use { "name": "id" }.

    FORMAT-04 — Inferred columns: Omit column metadata; infer the union of keys across all rows.

    FORMAT-05 — Different row shapes: Rows have different keys or key order; values map correctly.

    FORMAT-06 — Empty JSON: Test [], empty rows, and empty row objects; no misleading successful import.

    FORMAT-07 — ⚠ Mixed JSON elements: Mix objects, numbers, strings, nulls, and arrays. The parser currently filters non-object entries; verify skipped content and row counts are apparent.

    FORMAT-08 — ⚠ Single JSON object: Paste output from “Copy row as JSON.” A bare row object is not currently recognized as a one-row import; verify useful handling without accidental CSV interpretation.

    FORMAT-09 — Malformed JSON: Truncated input, invalid escaping, primitive roots, or malformed column metadata; no crash or unintended writes.

    FORMAT-10 — Basic CSV: Header plus one/multiple rows imports correctly.

    FORMAT-11 — CSV line endings: Test LF, CRLF, and CR, with and without a final newline.

    FORMAT-12 — CSV quoting: Quoted commas, multiline fields, escaped quotes, and empty quoted fields retain their content.

    FORMAT-13 — CSV headers: Test whitespace, blank headers, duplicate headers, case differences, Unicode, and a UTF-8 BOM.

    FORMAT-14 — ⚠ Uneven CSV rows: Missing fields currently become empty strings; extra fields are ignored. Verify this cannot silently produce misleading data.

    FORMAT-15 — ⚠ Empty CSV records: Test blank lines and rows containing only empty fields; verify whether dropping them is acceptable and visible.

    FORMAT-16 — ⚠ Broken CSV quoting: Unclosed quotes and quotes inside unquoted fields; the permissive parser must not silently misrepresent the input.

    FORMAT-17 — ⚠ TSV/spreadsheet paste: Paste tab-separated spreadsheet data. Row import currently uses a comma-based parser; verify it does not silently import the wrong structure.

    FORMAT-18 — ⚠ NULL round trip: Compare SQL NULL, JSON null, CSV empty field, empty string, and literal "null"; do not assume CSV preserves these distinctions.

    FORMAT-19 — Wrong clipboard payload: Missing/wrong type, columns, or rows; reject invalid structured requests with useful errors.


4. Column mapping and defaults


    MAP-01 — Exact names: Source and target names match; all values enter the correct columns.

    MAP-02 — Reordered columns: Source and target column order differs; mapping remains name-based.

    MAP-03 — Normalized names: Test User ID, user_id, and USER-ID; verify the implemented normalized matching.

    MAP-04 — Exact-match priority: An exact match and a normalized match both exist; exact match wins.

    MAP-05 — ⚠ Ambiguous names: Multiple source columns normalize to the same target; detect unintended selection or overwritten values.

    MAP-06 — ⚠ Unicode normalization: Different non-ASCII names normalize to an empty/similar key; unrelated columns must not silently map together.

    MAP-07 — Multiple target matches: One source name matches several normalized target names; verify the outcome is safe and understandable.

    MAP-08 — Unknown source columns: Insert mapped values and report ignored source columns.

    MAP-09 — No matching columns: Reject before inserting anything.

    MAP-10 — Required target missing: Missing required non-generated column without a default causes preflight rejection.

    MAP-11 — Optional target missing: Unmapped nullable/defaulted columns use the database’s applicable behavior.

    MAP-12 — ⚠ Missing value within a row: A mapped column is absent from one row; current behavior supplies NULL rather than its default.

    MAP-13 — Generated columns: Identity, auto-increment, and computed columns are omitted appropriately even when supplied by the source.

    MAP-14 — Non-generated primary key: Missing required primary-key values produce an accurate failure.

    MAP-15 — Schema changes: Rename, add, or drop target columns between copy, confirmation, and execution; no wrong-column insertion.


5. Data types and integrity

Run applicable cases through internal row paste, external JSON, CSV, and cell replacement.


    DATA-01 — Text boundaries: Empty, whitespace-only, leading/trailing whitespace, and maximum-length values.

    DATA-02 — Escaping: Quotes, backslashes, commas, tabs, CR/LF, and embedded control characters.

    DATA-03 — Unicode: Emoji, combining characters, non-Latin scripts, and right-to-left text.

    DATA-04 — SQL-like text: Values containing SQL syntax remain literal data.

    DATA-05 — Integers: Zero, negative values, min/max boundaries, overflow, and integers beyond JavaScript’s safe range.

    DATA-06 — Decimal/floating point: High precision, scale limits, scientific notation, rounding, and unsupported non-finite values.

    DATA-07 — Boolean: Actual booleans versus "true", "false", 0, and 1; verify engine-specific conversion.

    DATA-08 — Dates/times: Valid/invalid dates, leap days, timezones, daylight-saving boundaries, and fractional seconds.

    DATA-09 — UUID/enums: Valid, invalid, empty, and unknown values.

    DATA-10 — Structured values: JSON objects, arrays, JSON strings, JSON null, and database array types.

    DATA-11 — Binary: NULL, empty bytes, large BLOBs, valid encoded markers, and invalid encoding.

    DATA-12 — Engine-specific types: Spatial and other supported native types; unsupported conversions fail clearly.

    DATA-13 — Primary-key conflicts: Conflict with existing rows and duplicates inside the pasted batch, including composite keys.

    DATA-14 — Other constraints: UNIQUE, NOT NULL, CHECK, and foreign-key violations identify the failing rows.

    DATA-15 — Related rows: Parent/child ordering, self-references, and missing parents behave consistently with constraints.

    DATA-16 — Database side effects: Triggers and defaults execute correctly; generated IDs/sequences remain valid for the next normal insert.


6. Paste confirmation, execution, and results


    PASTE-01 — No destination: No selected table; no write request.

    PASTE-02 — Read-only destination: Views/read-only targets cannot be modified through Paste.

    PASTE-03 — Query destination: A query with a known mutation target pastes correctly; ambiguous joins/aggregations cannot choose an arbitrary table.

    PASTE-04 — Confirmation content: Display the correct connection scope, destination table, row count, and columns.

    PASTE-05 — Cancel/dismiss: Cancel, Back, or outside dismissal before confirmation causes zero writes.

    PASTE-06 — Destination changes: Switch table/tab/connection while confirmation is open; import targets the originally confirmed destination.

    PASTE-07 — Insert semantics: Pasting rows inserts new records; it does not replace selected records or perform an implicit upsert.

    PASTE-08 — All succeed: Insert count matches actual persisted rows.

    PASTE-09 — Partial success: Fail the first, middle, or last row; successful rows remain, and failures are reported accurately.

    PASTE-10 — All fail: Report zero inserted and the correct failure count without a success-only message.

    PASTE-11 — Result accounting: Inserted plus failed counts match attempted rows; error indices identify the correct input rows.

    PASTE-12 — Error response variants: Handle result plus warning/error, error without result, and malformed responses.

    PASTE-13 — Duplicate taps: Rapidly tap Paste/Import; detect unintended duplicate submissions.

    PASTE-14 — Intentional repeat: Paste twice; second insertion follows constraints and does not claim automatic deduplication.

    PASTE-15 — ⚠ Retry after partial success: Successful rows from the first attempt remain; retry does not hide duplicate-insert risk.

    PASTE-16 — Refresh destination: Refresh the correct table/query/panel and its counts after completion.

    PASTE-17 — Navigation during execution: Switch or close tabs while paste runs; completion must not populate the wrong screen.

    PASTE-18 — Filtered results: Successfully inserted rows may not match the current filter; counts/messages still reflect the server result.


7. Single-cell copy and replacement


    CELL-01 — Copy column name: Copy the exact name, including spaces and special characters.

    CELL-02 — Copy full value: Copy the complete raw value, even when the displayed preview is truncated.

    CELL-03 — Copy row JSON: Output contains the intended row fields and valid JSON.

    CELL-04 — Editable target: “Paste as replacement” appears only where editing is supported.

    CELL-05 — Prefilled editor: Clipboard content appears in the editor before saving.

    CELL-06 — Cancel replacement: Cancelling leaves the stored value unchanged.

    CELL-07 — Save replacement: Only the intended cell/record changes, using the correct primary-key identity.

    CELL-08 — Empty versus absent: Empty-string clipboard can replace with empty text; absent clipboard content shows an error.

    CELL-09 — NULL handling: Empty text, literal "null", JSON null, and “Set SQL NULL” remain distinct.

    CELL-10 — Invalid replacement: Type/constraint failures leave a clear error and accurate displayed value.

    CELL-11 — Concurrent modification: Another client updates/deletes the row before Save; no false success or unintended record update.

    CELL-12 — Size boundary: Test 499,999, 500,000, and 500,001 UTF-8 bytes. Above 500,000, the copy helper should offer file saving.

    CELL-13 — Multibyte boundary: Repeat the size test with emoji/non-ASCII text; measure bytes rather than character count.

    CELL-14 — Save fallback: Cancel or fail file saving; do not report a completed clipboard copy.


8. Rectangular range copying


    RANGE-01 — Range shapes: Copy one cell, one row, one column, and a multirow/multicolumn rectangle.

    RANGE-02 — Reverse selection: Select opposite corners in every direction; output remains in table order.

    RANGE-03 — CSV versus TSV: Verify headers, delimiters, and the exact selected subset.

    RANGE-04 — Escaping: Tabs, commas, quotes, and embedded newlines are quoted correctly.

    RANGE-05 — Value representation: Verify NULL, booleans, numbers, and nested JSON serialization.

    RANGE-06 — Exact field lookup: Exact column-name matches win over case-insensitive alternatives.

    RANGE-07 — Invalidated selection: Refresh/filter/page changes cannot copy an out-of-bounds or unintended rectangle.

    RANGE-08 — Done action: Clears selection and hides the range toolbar.


9. Clipboard inspector, keyboard, and accessibility


    UI-01 — Metadata: Show accurate source, timestamp, row count, and column count.

    UI-02 — Preview boundary: Test 0, 1, 50, and 51+ rows; preview limits do not truncate the stored payload.

    UI-03 — Inspector Copy JSON: Copies all saved rows, including those beyond the preview.

    UI-04 — Inspector Paste: Enabled only for an appropriate selected table and opens the correct confirmation.

    UI-05 — Inspector updates: Copy/clear while the inspector is open; no stale counts or stale preview.

    UI-06 — Keyboard shortcuts: Ctrl/Meta+C and V dispatch the appropriate database action outside editable text fields.

    UI-07 — Text-field shortcuts: Within an editor, C/V shortcuts retain normal text editing behavior.

    UI-08 — Repeated key events: Key-up and held-key repeats do not trigger duplicate imports.

    UI-09 — Inspector navigation: F6 and Escape open/close or restore the previous section correctly.

    UI-10 — Accessibility/layout: Screen readers identify copy/paste actions; large fonts, narrow screens, long names, and landscape mode keep confirmation and errors readable.


10. Definition copying and table transfer

These cover the related table-copy workflows exposed by the app.


    TRANSFER-01 — Copy definition: Database/schema/table definition copying preserves the complete DDL text.

    TRANSFER-02 — Missing definition: Empty/unavailable/very large definitions are handled without a false success or crash.

    TRANSFER-03 — Existing-table copy: Requires an existing destination; copies rows using the destination mapping.

    TRANSFER-04 — Schema only: Creates a new empty table.

    TRANSFER-05 — Schema and data: Creates a new table and copies the expected rows.

    TRANSFER-06 — Empty source: Schema creation succeeds with zero copied rows where applicable.

    TRANSFER-07 — Destination conflict: New-table modes refuse an existing destination without overwriting it.

    TRANSFER-08 — Target selection: Same/different connections, databases, and schemas resolve correctly.

    TRANSFER-09 — Names: Suggested names, manually edited names, quoted identifiers, reserved words, spaces, and Unicode work correctly.

    TRANSFER-10 — Self-copy: Reject an explicitly identical source and destination.

    TRANSFER-11 — ⚠ Aliased self-copy: Different connection records or schema aliases reference the same physical table; copying must not repeatedly consume newly inserted rows.

    TRANSFER-12 — Batch boundaries: Test 0, 1, 199, 200, 201, 400, and 401 rows, plus a much larger source.

    TRANSFER-13 — Source changes during transfer: Insert/update/delete source rows during paging; detect skipped or duplicated records.

    TRANSFER-14 — Failure-detail boundary: Test 49, 50, and 51+ failures; total failure count remains correct when displayed details are capped.

    TRANSFER-15 — Job lifecycle: Queued/running/completed/failed states, progress, logs, warnings, and result counts agree.

    TRANSFER-16 — Client lifecycle: Navigate away, background, restart, and reconnect; recover the correct server job status.

    TRANSFER-17 — Partial creation: Table creation succeeds but copying fails; report the surviving empty/partial table.

    TRANSFER-18 — Portable payload validation: Unsupported version/type, missing table/columns, malformed entries, and conflicting primary-key metadata are handled clearly.

    TRANSFER-19 — ⚠ Schema fidelity: Check defaults, keys, indexes, foreign keys, triggers, comments, and generated/identity properties individually. The portable format does not preserve all of them.

    TRANSFER-20 — Cross-engine matrix: Repeat applicable cases for every supported source→target engine pair, including both directions.


11. Failure recovery, permissions, and performance


    ROBUST-01 — Connection failures: Missing/deleted connection, unreachable host, expired credentials, and expired app session.

    ROBUST-02 — Permissions: Source SELECT denied, target INSERT denied, table/schema creation denied, and read-only database.

    ROBUST-03 — Authorization: Requests referencing another user’s inaccessible connection are rejected.

    ROBUST-04 — Destination disappears: Drop/rename the target after confirmation or during execution; report actual partial results.

    ROBUST-05 — Database failures: Lock contention, deadlock, timeout, storage full, and connection exhaustion.

    ROBUST-06 — Network interruption: Disconnect before submission, during processing, and after commit but before the response; do not falsely claim zero writes.

    ROBUST-07 — Server interruption: Restart/crash the server mid-operation; restored status must not misrepresent partial writes.

    ROBUST-08 — Concurrent clients: Simultaneous imports into the same table respect constraints and report independent outcomes.

    ROBUST-09 — Large row copy: Large JSON row batches handle Android clipboard limits; this path lacks the cell helper’s 500,000-byte safeguard.

    ROBUST-10 — Request-size guard: Test below/at/above the configured request-body limit with the limit enabled and disabled.

    ROBUST-11 — Large parsing workload: Many rows, many columns, and one huge cell do not cause an unresponsive UI or silent truncation.

    ROBUST-12 — Large failure response: Thousands of failed direct-paste rows remain usable without excessive memory consumption or a frozen error dialog.

    ROBUST-13 — Repeated operation lifecycle: Repeatedly open/close panels and copy/paste; no accumulating listeners, duplicate callbacks, or stale action state.

    ROBUST-14 — Sensitive values in errors: Failure reports provide enough context to identify a problem without unintentionally exposing credentials or unrelated data.


## 12. First adversarial expansion — consult the results ledger for evidence

### A. Clipboard and asynchronous races

- [ ] ADV-001 — **P0: Clipboard replaced after confirmation.** With internal
  rows empty, open confirmation for external payload A (`approved-A`), replace
  Android clipboard with B (`unapproved-B`), then confirm. Import the reviewed
  A snapshot or invalidate/reconfirm; never silently insert B under A's preview.
  Verify persisted markers, not just the displayed count.

- [ ] ADV-002 — **P0: Clear changes clipboard precedence mid-dialog.** Open
  confirmation from internal rows A while external rows B exist; clear A through
  another reachable surface. Confirm or cancel the original dialog. Never fall
  back silently to B; cancellation must cause zero writes. If the UI prevents
  the state change, record that protection rather than injecting controller state.

- [ ] ADV-003 — **P1: Older copy finishes after newer copy.** Start a large
  copy A, then a small copy B while A is observably pending. Once both finish,
  inspect internal rows, system text, and metadata. B must remain the latest
  accepted copy; stale A must not overwrite either clipboard or its source label.

- [ ] ADV-004 — **P1: Clear races with copy serialization.** Clear both
  clipboards while a large copy is pending; await its completion and restart.
  Cleared rows must not reappear from a stale completion or persistence write.
  Repeat with clear followed by a new copy B; B must survive.

- [ ] ADV-005 — **P1: External copy overlaps publication.** While a large app
  copy is pending, publish external text B. Observe the actual completion order,
  then open Paste. The chosen snapshot/source must be accurately disclosed;
  internal rows, publication success, and external text must not be conflated.

- [ ] ADV-006 — **P0: Old query response overtakes a new query.** Delay query
  A, run query B with the same columns but different row markers, then release
  A. Select and copy from the displayed result. Data, selection, source metadata,
  and clipboard must all describe the same active query; A must not replace B.

- [ ] ADV-007 — **P0: Completion crosses server profiles.** Submit a marked
  paste on QA server A and hold its response; switch to QA server B with a
  same-named table, then release A. Only A may receive the write. A's result
  must not replace B's grid, selection, counts, or operation status.

- [ ] ADV-008 — **P1: Clipboard preview read overtakes clear.** Delay a
  clipboard inspection/read using a labelled test hook, clear the clipboard,
  then release the read. The inspector and Paste visibility must not resurrect
  stale content. Label fault-injected coverage separately from normal UI coverage.

- [ ] ADV-009 — **P0: Two confirmations overlap in time.** Open a paste
  confirmation, then try another Paste using touch or a fresh keyboard shortcut.
  Only one active submission may result from one confirmation. If a second
  dialog is allowed, each must retain its own exact payload and destination.

- [ ] ADV-010 — **P1: Listener event storm after reattachment.** Repeatedly
  alternate nonempty/empty external text while navigating away/back through
  20 bounded cycles; end with an empty clipboard. With internal rows cleared,
  Paste must settle hidden, with no duplicate dialogs, crash, or stale snapshot.
  Record callback/listener counts only if actually observable.

### B. Parser ambiguity and hostile payload structure

- [ ] ADV-011 — **P0: Conflicting duplicate JSON keys.** Paste raw
  `[{"id":101,"id":102,"name":"duplicate-key"}]`; repeat with a duplicate
  key inside a JSON-valued cell. Reject ambiguity or visibly disclose a
  deterministic policy; preview and stored values must never disagree.

- [ ] ADV-012 — **P1: Duplicate column metadata.** Use
  `{"columns":["id","name","name"],"rows":[{"id":201,"name":"one"}]}`
  and the equivalent column-object wrapper. Reject or explicitly deduplicate;
  no repeated SQL assignment, shifted value, or misleading column count.

- [ ] ADV-013 — **P1: Wrapper-looking ordinary columns.** Import rows whose
  real column names are `rows`, `columns`, `type`, and `version`, individually
  and together. Distinguish row fields from transport metadata at the correct
  nesting level; no dropped fields or reinterpretation of cell contents as rows.

- [ ] ADV-014 — **P1: Encoded-marker collision.** Put an ordinary JSON object
  that resembles the app's binary/encoded-text marker into a JSON column; also
  try an unsupported marker version. Preserve ordinary data or reject clearly;
  never silently decode the wrong value or treat malformed encoding as success.

- [ ] ADV-015 — **P1: Deep rather than wide JSON.** Increase nesting depth
  through bounded steps (for example 16, 64, 128, 256) while keeping bytes small.
  Require bounded rejection or responsive completion, no stack crash, and a
  successful tiny valid paste afterwards. Record the tested depth ceiling.

- [ ] ADV-016 — **P1: Escaped NUL inside text.** Paste `before\u0000after`
  between two valid marked rows. Require exact preservation where supported or
  an explicit row failure; never store only `before`. Verify neighboring rows
  against the actual transaction/partial-success policy.

- [ ] ADV-017 — **P1: Unpaired Unicode surrogates.** Supply raw JSON escape
  strings `\uD800`, `\uDC00`, and a valid paired emoji in separate rows.
  Invalid sequences must not silently change an identity or crash parsing;
  record rejection/replacement policy and compare valid-pair bytes exactly.

- [ ] ADV-018 — **P1: Content that resembles parser syntax.** Use JSON text
  cells containing CSV headers/newlines and CSV quoted fields containing complete
  JSON objects/arrays. The outer format alone determines parsing; nested text
  must not be recursively imported or split into extra rows.

- [ ] ADV-019 — **P1: Invisible structural characters.** Place BOM, zero-width
  space, and nonbreaking space separately at the document start, inside a quoted
  cell, and in a column name. Only intentional structural handling is allowed;
  preserved data must remain byte-exact and ambiguous names must not mis-map.

- [ ] ADV-020 — **P1: Trailing second document.** Paste two adjacent JSON
  arrays, a valid array followed by garbage, and JSON followed by CSV. Reject
  trailing unconsumed content without writes; do not silently import the first
  document while claiming to have processed the complete clipboard.

### C. Target identity, row identity, and exact mapping

- [ ] ADV-021 — **P0: Same connection ID, different database.** Prepare DB-A
  and DB-B with same-named targets and different sentinels. After confirming
  DB-A's destination, edit the saved connection to point at DB-B from another
  client. Execution must reject/reconfirm the changed identity, not silently
  write to DB-B. Record missing identity protection as a gap if reproduced.

- [ ] ADV-022 — **P0: Search-path shadowing.** Create `qa_one.items` and
  `qa_two.items`; review the qualified first target, then alter the session or
  role search path using a controlled fixture. Only the reviewed qualified
  target may change, regardless of which unqualified name now resolves first.

- [ ] ADV-023 — **P0: Drop/recreate under the same table name.** After
  confirmation, replace the disposable target with a new object having the same
  name and compatible columns. Record both object identities. Require detection
  or renewed confirmation; explicitly report absent identity protection rather
  than claiming that a name match proves the original object was used.

- [ ] ADV-024 — **P0: Duplicate result labels from a join.** Run a query with
  two differently valued `id` columns. Copy rows and inspect metadata before
  any paste. Reject ambiguous labels or preserve a disclosed disambiguation;
  never silently discard one value. Repeat using explicit unique SQL aliases.

- [ ] ADV-025 — **P0: Composite-key delimiter collision.** Create keys
  `("a|b","c")` and `("a","b|c")`, plus empty/Unicode key components.
  Replace one cell. Exactly one intended record must change; identity handling
  must not rely on ambiguous concatenated display strings.

- [ ] ADV-026 — **P0: Display-equivalent key values.** Use text keys `001`,
  `1`, `1 `, and case variants where the engine permits them. Sort then edit or
  paste a replacement into one row. Verify exact key typing/collation and the
  affected-row count; display normalization must not select another record.

- [ ] ADV-027 — **P0: Primary-key column editing.** Change an editable primary
  key, then immediately edit a different cell in that row. The second action
  must use the refreshed key; it must not update a different row subsequently
  created with the old key. If key editing is unsupported, reject it safely.

- [ ] ADV-028 — **P0: Row deleted and key reused.** Open a cell editor; another
  client deletes the row and inserts a different marked row under the same key.
  Save the stale editor. Require conflict detection or renewed review; record
  any blind overwrite of the replacement record as an identity-safety gap.

- [ ] ADV-029 — **P1: Visually identical Unicode identifiers.** Create columns
  with composed/decomposed accents and Latin/Cyrillic lookalikes where supported;
  give each distinct markers. Mapping must prefer exact identities or reject
  ambiguity. Inspect native values so visual similarity cannot hide a swap.

- [ ] ADV-030 — **P0: Alias spoofs an editable primary key.** Query an expression
  such as `id + 100 AS id` alongside a real row value, then attempt cell editing.
  The derived alias must not authorize mutation of an unrelated physical row
  whose key matches the computed value; reject or use proven source identity.

### D. Permissions, privacy, and account boundaries

- [ ] ADV-031 — **P0: Account switch with retained internal rows.** Account A
  copies synthetic private rows, then logs out and B logs in on the same install.
  Inspect internal clipboard and source metadata. A's app-private snapshot must
  not leak into B's account; record any intentional device-global policy and
  its disclosure separately from Android's system clipboard behavior.

- [ ] ADV-032 — **P0: Logout during a pending copy.** Delay A's row copy,
  log out, and release completion after B signs in. No stale A snapshot, source
  label, success toast, or persisted clipboard may be published into B's state.

- [ ] ADV-033 — **P0: INSERT revoked after confirmation.** A restricted role
  opens confirmation while authorized; revoke INSERT in a second admin session
  before submission. Require denied writes and truthful counts. Regrant and
  make a fresh reviewed attempt; it must recover without an app restart.

- [ ] ADV-034 — **P0: Row-level security mixed batch.** Use a nonowner,
  nonbypass PostgreSQL role allowed to insert tenant A but not B. Paste alternating
  A/B records. Verify no unauthorized B rows, correct committed/failed counts,
  and no unrelated tenant values in errors. Do not run this fixture as superuser.

- [ ] ADV-035 — **P0: UPDATE-only/INSERT-only privilege split.** Test a role
  allowed to insert but not update, then the reverse. Row Paste must remain an
  INSERT operation and cell replacement an UPDATE operation; neither may fall
  back to another mutation method to bypass a denied permission.

- [ ] ADV-036 — **P0: Job ownership guessed from another account.** Create a
  transfer as A; as B, request its known job ID through the app/API observation
  lane. Deny access to status, errors, source/destination metadata and cancellation
  if present. A's job and its rows must remain unaffected.

- [ ] ADV-037 — **P0: Synthetic secrets in connection metadata.** Put unique
  fake password/token markers in QA connection configuration, not row data.
  Inspect copied JSON, clip descriptions, saved exports and errors. Source
  identification must not serialize secret-bearing connection configuration.

- [ ] ADV-038 — **P1: Spreadsheet-formula text export.** Use harmless text
  `=1+1`, `+1+1`, `-1+1`, `@SUM(1,1)` and tab-prefixed formula-like values.
  Export/copy CSV/TSV and reimport. Preserve database literals; document any
  explicitly selected spreadsheet-safe transformation and its reversibility.
  Do not use executable commands or external network formulas.

- [ ] ADV-039 — **P1: SQL/export header injection via identifiers.** Use quoted
  identifiers containing line breaks, quotes, commas and SQL-comment-looking
  text. Copy definitions, rows and headers. Escaping must keep them identifiers
  or data; validate in a disposable target and verify no additional objects.

- [ ] ADV-040 — **P0: Two installed app instances and clipboard scope.** Where
  an owned separate profile/install is available, copy private internal rows in
  instance A and inspect B. App-private persistence must not cross the boundary.
  Record system-clipboard sharing separately; absence of such a setup is BLOCKED,
  not evidence of private-store isolation.

### E. UI lifecycle and recoverability under pressure

- [ ] ADV-041 — **P1: Rotation with confirmation open.** Open a reviewed paste
  confirmation, rotate twice, then cancel. Payload/target remain consistent and
  native data unchanged. Repeat with one Import: exactly one request, no replay
  from recreation, and no lost operation tracking.

- [ ] ADV-042 — **P1: Recreation with unsaved cell replacement.** Paste text
  into a cell editor, change font scale or recreate the Activity, then inspect
  the draft. Preserve exact draft/target or explicitly discard; never save
  automatically or attach the draft to a different selected row.

- [ ] ADV-043 — **P1: Screen lock during confirmation.** Lock/unlock the owned
  device with a confirmation open while clipboard availability changes. Require
  a still-valid reviewed snapshot or explicit invalidation, with no automatic
  import, stale hidden modal, or permanent busy state after unlocking.

- [ ] ADV-044 — **P1: Background timeout before submission.** Open confirmation,
  background until the QA session expires, then return and confirm. Reauthentication
  must not silently replay the pending mutation. Inspect native rows before
  retrying and require a fresh intentional submission after authentication.

- [ ] ADV-045 — **P1: Keyboard shown versus modal buttons.** With the SQL or
  cell editor keyboard visible, open the reachable paste/cancel flow in portrait
  and landscape. Import and Cancel remain reachable without accidental activation;
  Back dismisses the correct layer and never submits a mutation.

- [ ] ADV-046 — **P1: Scrolling during row selection.** Select a row, rapidly
  scroll it offscreen and back, then select a distant row and Copy. UI recycling
  must preserve identities, not checkbox positions. Compare exact copied IDs
  and ensure adjacent rows never inherit another row's selection.

- [ ] ADV-047 — **P1: Long press becomes a scroll gesture.** Start a cell long
  press, drag beyond the touch threshold, and release over an action's former
  location. No hidden Copy/Paste/Delete action should fire. A subsequent deliberate
  cell tap/long press must still open the intended row's actions.

- [ ] ADV-048 — **P1: Focus returns after clipboard permission denial.** Deny
  clipboard access in a supported owned-device fault setup, return to Database,
  then restore access and copy fresh text. Require no crash/focus loop and correct
  recovery; old error state must not permanently suppress newly available data.

- [ ] ADV-049 — **P1: Recreate during save-to-file fallback.** Launch the large
  value export picker, recreate the app Activity, then finish or cancel the picker.
  Match any saved file to the original reviewed value; no double write, wrong-cell
  export, or clipboard-success message for an unfinished file operation.

- [ ] ADV-050 — **P2: Bidirectional text in confirmation.** Use synthetic RTL
  table/column names mixed with digits and Latin markers, including directional
  controls. Review source, target and counts. The user must be able to distinguish
  the actual destination; UI directionality must never alter the executed identity.

### F. Commit-time failure, unknown outcomes, and batch boundaries

- [ ] ADV-051 — **P0: Deferred constraint fails at commit.** Use a supported
  deferred FK/unique constraint and data that passes statement execution but fails
  at commit. Compare final persisted rows. Never report rolled-back statements
  as successful committed inserts; disclose the actual transaction boundary.

- [ ] ADV-052 — **P1: Trigger suppresses insertion.** In a disposable engine
  fixture, use a trigger that declines one marked row without raising an error.
  Verify reported inserted/skipped counts against actual rows; attempted statement
  count must not masquerade as committed row count.

- [ ] ADV-053 — **P1: Trigger rewrites visible values.** Use a trigger that
  normalizes a supplied value and populates an audit row. After paste, refresh
  from the database; show persisted values rather than optimistic input echoes.
  Verify one intended audit effect per committed record.

- [ ] ADV-054 — **P0: Deadlock retry duplicates side effects.** Induce a bounded
  two-session deadlock during one marked paste with a transactional audit table.
  If automatic retries occur, each committed row/audit effect must appear once.
  If no retry exists, report the failure accurately and recover on a fresh attempt.

- [ ] ADV-055 — **P1: Deferred failure crosses a batch boundary.** Arrange
  a commit-time failure around observed batch boundaries (and 199/200/201 if
  applicable). Determine which batches actually committed; counts and row indices
  must match that outcome, without claiming global rollback or global success.

- [ ] ADV-056 — **P0: Process killed after submit but before acknowledgement.**
  Barrier the response after a uniquely marked insert is known committed, then
  kill only the QA app. Relaunch and inspect data before interacting. No automatic
  duplicate submission; missing acknowledgement must not be reported as zero writes.

- [ ] ADV-057 — **P1: Refresh fails after successful import.** Allow the write
  to commit, then fail only its follow-up SELECT. Preserve the successful mutation
  outcome and clearly distinguish stale-view/refresh failure; do not invite a
  blind batch retry as though INSERT had failed. Restore reads and refresh once.

- [ ] ADV-058 — **P1: Lost status response followed by late terminal response.**
  For a real transfer job, hold an earlier running response until after completion
  is observed, then release it. Completed status/counts must not regress to running
  or trigger a second transfer. Keep request/job identities in the evidence.

- [ ] ADV-059 — **P1: Failure indices after preflight omission.** Supply a
  marked mixed-shape batch containing parser-rejected/skipped entries followed
  by a database constraint failure. Errors must identify the correct original
  input row, with separate skipped/rejected/attempted counts and no index drift.

- [ ] ADV-060 — **P0: Cancel semantics after execution starts.** If a running
  paste/transfer exposes Cancel, invoke it before the first write, between batches,
  and after commit but before final UI delivery. Inspect native rows each time;
  cancellation must not imply rollback of committed work. If no running Cancel
  exists, record N/A for that control and verify navigation is not labelled rollback.

## 13. Second adversarial expansion — ADV-061–ADV-090 NOT RUN

All checkboxes below start unchecked. Use the per-case first-attempt → fix →
retest cycle above, with independent records for each platform/engine variant.
Fault hooks, historical builds, document providers, and retained SQL sessions
are explicit prerequisites, not assumed capabilities. Record unavailable
test infrastructure as BLOCKED; use N/A only for an inapplicable feature.

### G. Persisted clipboard compatibility and atomicity

- [ ] ADV-061 — **P1: Upgrade a real saved clipboard.** With an available
  previous app build, copy rows containing NULL, exact decimals, and binary data;
  upgrade in place without clearing storage. Inspect and paste into a disposable
  destination. Values and source identity must survive exactly, or migration must
  explicitly reject the old snapshot. A synthetic payload is not upgrade evidence.

- [ ] ADV-062 — **P1: Future persisted format version.** In an owned private-storage
  fixture, supply a valid payload with an unsupported future version and launch
  twice. Compare stored bytes and inspector state. Do not silently normalize it
  into a valid empty/current snapshot; preserve it or explicitly invalidate it
  under a documented policy. A deliberate fresh copy must recover normal behavior.

- [ ] ADV-063 — **P0: Torn snapshot and metadata publication.** Use a labelled
  fault hook to interrupt replacement of saved clipboard A with B between payload
  and metadata persistence. Relaunch after each observable write boundary. Recover
  one complete snapshot or reject it; never combine A's values with B's source,
  types, columns, or count. Compare both raw storage and an actual paste result.

- [ ] ADV-064 — **P1: Persistence fails after clipboard publication.** Exhaust
  only a bounded QA storage allocation or inject a save failure after publishing
  a new system clipboard value. Inspect both clipboard sources immediately and
  after relaunch. Any volatile-versus-saved distinction must be accurate; no claim
  that B was saved while silently pasting previously persisted A as B.

### H. File import/export boundaries and document providers

- [ ] ADV-065 — **P1: Export fails on flush or close.** Use an owned provider
  accepting writes but failing flush/close, tested separately. Export a marked
  full value. Require a failed/incomplete outcome, not success based only on
  write acceptance. Read the provider's actual bytes and verify clipboard state.

- [ ] ADV-066 — **P1: Short export replaces a longer document.** Where replacement
  is supported, export a short value over a longer fixture with a tail sentinel.
  Reopen through the provider: bytes must equal the short value, without appended
  content or residual tail. Record N/A if this UI/provider cannot replace files.

- [ ] ADV-067 — **P1: Unknown-length import with short reads.** Use an owned
  provider reporting unknown/zero length and returning valid CSV/JSON in tiny
  chunks. Import must read through EOF, not treat a short read or size metadata
  as completion. Match every marked row and exact field value to native storage.

- [ ] ADV-068 — **P1: Read failure after a valid prefix.** Return several complete
  records followed by an injected stream error, not clean EOF. Check whether
  prevalidation or streaming execution is promised. Never report complete import;
  disclose any committed prefix and its original indices. Restore the provider
  and verify a fresh preview without blindly resubmitting already committed rows.

- [ ] ADV-069 — **P0: Same filename, different document identity.** Provide two
  owned document URIs with the same display name and different row markers.
  Select A, cancel, then select B; also recreate the picker when supported.
  Preview and execution must use B's URI/content, never a filename-keyed cache of A.

- [ ] ADV-070 — **P1: Document access revoked after selection.** Revoke only the
  fixture's URI grant after picker selection but before stream opening. Import or
  export must report access failure without falling back to another cached file.
  Regrant/reselect and verify exact successful content with no duplicate execution.

- [ ] ADV-071 — **P1: Cancelled exports release provider resources.** Perform
  20 bounded cancel/failure cycles after stream opening, using provider counters.
  Open streams/descriptors must return to baseline after a recorded timeout;
  no later writes may target abandoned documents. A subsequent valid export
  must complete with exact bytes. Record measured counts, not just responsiveness.

- [ ] ADV-072 — **P1: File encoding declaration and actual bytes disagree.**
  Import owned UTF-16/BOM, malformed UTF-8, and valid UTF-8 files under misleading
  MIME types/extensions. Follow a documented decoding policy or explicitly reject;
  never silently insert replacement characters or a corrupted first header.
  Verify accepted text by code points and native values, not rendered appearance.

### I. Engine/session semantics beyond basic type conversion

- [ ] ADV-073 — **P0: Pooled role/session state survives a request.** Using a
  supported session surface or explicit QA hook, set a restricted role/RLS context,
  finish or abort its transaction, then paste with the intended fresh identity.
  Prove connection reuse and record effective identity. The later request must
  have its intended permissions, with no inherited elevated or stale tenant state.

- [ ] ADV-074 — **P1: Date interpretation changes after preview.** In controlled
  sessions, change PostgreSQL DateStyle (or supported equivalent) between preview
  and execution of ambiguous dates. Record actual settings per operation. Typed
  ISO values must remain exact; ambiguous text must be rejected or its conversion
  clearly disclosed rather than previewing one date and storing another.

- [ ] ADV-075 — **P0: Temporary table shadows reviewed permanent target.** Where
  retained sessions exist, create a temporary table named like the permanent
  destination, with separate marker baselines. Paste into the reviewed permanent
  table and inspect both physical objects. Only that explicitly qualified target
  may change; record backend session identity to prove shadowing was exercised.

- [ ] ADV-076 — **P1: Partition routing and rejected partition key.** Paste marked
  rows into a partitioned parent, routing to two partitions plus one unmatched key.
  Compare parent and child row sets against the documented transaction policy.
  Counts must reflect committed rows, not attempted routing; no arbitrary child
  becomes the destination. Repeat with an existing default partition where supported.

- [ ] ADV-077 — **P1: Statement error poisons a reused transaction.** Trigger a
  middle-row constraint failure on an engine with transaction-abort semantics.
  Finish the import, then perform a valid independent paste and SELECT on an
  observably reused connection. Rollback/reset must restore usability; no inherited
  aborted transaction or unexplained loss of the subsequent valid row.

- [ ] ADV-078 — **P1: Successful statement carries a conversion warning.** In a
  supported permissive engine/session mode, submit a value that is truncated or
  coerced with a warning instead of an error. Compare stored values and warnings.
  The result must disclose lossy conversion or prevent it; a green success count
  alone must not imply exact-value preservation. Pin the session mode in evidence.

### J. Related-record details and their copy/edit destinations

- [ ] ADV-079 — **P0: Composite foreign key uses every component.** Create two
  parents sharing one key component and children referencing each composite key.
  Open related record from each FK cell, then copy its row. The modal and copied
  data must show exactly the referenced parent and all promised columns, never
  the first match on one component. Verify native parent keys and values.

- [ ] ADV-080 — **P1: Nullable composite relationship is not a wildcard.** For a
  supported MATCH SIMPLE composite FK, use children with one NULL component and
  parents matching the non-NULL component. Open related record: do not infer a
  parent by dropping NULL predicates. Show no resolvable relationship or disable
  the action; source and unrelated parent data must remain unchanged.

- [ ] ADV-081 — **P0: Foreign key references a non-primary unique key.** Make a
  child reference a parent's unique external code while its primary key differs.
  Related details and Copy row must resolve through the declared FK columns.
  If editing is offered, it must identify the resolved parent correctly and alter
  only that parent; never interpret the external code as its primary key.

- [ ] ADV-082 — **P0: Multiple foreign keys need explicit relationship identity.**
  Use created_by and updated_by pointing to different users, plus a cell participating
  in two declared relationships where supported. Each action must select the
  intended constraint or offer a labelled choice. Modal title, complete row data,
  and copied row must agree; no arbitrary first-constraint selection.

- [ ] ADV-083 — **P0: Related tables share a name across schemas.** Create two
  schemas containing users with identical keys and different markers; point the
  child FK at only one. Open related data and Copy row after changing the active
  explorer schema. Use the declared referenced schema, not the currently selected
  schema. Any subsequent Paste confirmation must name its actual destination.

- [ ] ADV-084 — **P0: Referenced row is hidden by authorization.** Let a QA role
  read a child while RLS hides its parent. Open related record and attempt exposed
  copy actions. Do not leak the parent's fields via a privileged fallback or stale
  modal cache. Restore visibility and reopen; verify the authorized full row.

### K. Complete-value provenance, mutation boundaries, and recovery UX

- [ ] ADV-085 — **P1: Paged viewer must not become the copied value.** Open a
  multi-page cell with unique beginning/middle/end markers, navigate to page two,
  then Copy full value, Save full value, and Copy row JSON in separate attempts.
  Each action promising full data must contain every marker exactly once; only
  an explicitly selected-text action may copy a subset. Verify lengths and hashes.

- [ ] ADV-086 — **P0: Related-modal nesting retains the active row identity.**
  Open child → parent → another related record where nesting is available.
  Copy at each level, then return and reopen the child. Inspector metadata and
  raw JSON must identify that level's row. If editing is exposed, cancel first,
  then edit a fixture marker and prove only the intended level's record changed.

- [ ] ADV-087 — **P1: Count query finishes while rows are selected.** Select
  noncontiguous marked rows, hold Get exact count in flight, and copy before and
  after releasing its response. Count completion must not rebuild selection into
  different rows or clear the clipboard. Compare selected keys and both payloads;
  this case changes only count state, not the row-result dataset.

- [ ] ADV-088 — **P0: Failed replacement must not leave an optimistic copy source.**
  Paste an invalid replacement into a constrained cell and let Save fail. Without
  refreshing, inspect details and Copy row/full value. They must use persisted
  values or explicitly labelled unsaved draft data, not present a rejected edit as
  stored data. Cancel/reopen and verify the original value natively and in the UI.

- [ ] ADV-089 — **P1: Failure list cap does not truncate retry provenance.** Make
  more failures than the observed detail-display cap, with unique row markers.
  If retry/export-failed-rows is offered, compare its set with the complete native
  outcome: every failed input once and no successful input. If absent, record N/A
  for that action and verify the UI labels the visible errors as incomplete.

- [ ] ADV-090 — **P1: Round-trip stability across repeated generations.** On a
  deterministic, trigger-free typed fixture, perform copy → fresh-table paste →
  copy for three generations using the lossless structured format. Compare typed
  values, NULLs, byte content, exact decimals, and column identity at every step,
  excluding explicitly generated keys. Detect accumulated quoting, base64 wrapping,
  or stringification; raw JSON property order alone is not a failure.

# Database copy/paste — execution ledger

Started 2026-10-01. Scope: every case in `db-copy-paste.md` (253 cases plus
listed variants after the two October5 adversarial expansions). The original
163-case ledger below is historical coverage; new ADV cases need new evidence.

Only isolated disposable databases are used. Existing application services and user databases are not restart/failure-injection targets.

Evidence labels: **UI** = actual Android emulator interaction; **API/DB** = live HTTP operation with direct persisted-data verification; **REGRESSION** = automated helper/server tests, supporting evidence only.

Status: PENDING / PASS / FAIL / BLOCKED. A supporting regression test does not by itself pass a manual UI case. Fix is N/A only after the first test passes; every reproduced defect requires a new retest entry.

The per-case ledger is the current coverage index. Dated cycles below preserve
the findings and unfinished work at their original checkpoints; a historical
unchecked item is not proof that its later, linked retest is still pending.

## Remaining manual coverage gates

The table below describes retained variants from the original 163 cases only.
It does **not** imply the 90 added ADV cases have been executed.

Rechecked October 5, 2026 after the uploaded-APK lifecycle smoke. These are
blocked variants, not untested ordinary database workflows or new product bugs.
Do not mark a whole case fully passed merely because its other variants passed.

| Case | Remaining manual variant | Prerequisite before another app test |
| --- | --- | --- |
| CLIP-11 | Clipboard service genuinely unavailable | A safe platform fault setup; injected null-service tests remain supplementary. |
| UI-06 | Meta+C/V outside editable fields | Separate calibration Activity must receive the actual modifier/key callbacks. |
| UI-07 | Meta+C/V inside editor | Same successful calibration, then exact text-editing/no-import observations. |
| UI-10 | Audible TalkBack announcements | Working audio and an observer able to verify the spoken output. |

- [x] Latest preflight: ADB5056 has no connected QA device; default ADB lists
  only emulator5594, not a newly available physical test device.
- [x] Latest audio preflight: both PulseAudio sink/source queries refuse the
  connection. Do not infer audible speech from accessibility labels/events.
- [x] Port8100 remains listening and unchanged. Restarting it does not resolve
  these Android input/audio/service prerequisites.
- [ ] Execute each remaining variant when its prerequisite is satisfied,
  retaining first-test → any justified fix → retest evidence.

This preflight adds no new manual pass, product fix, or release artifact.

## Process checklist

### Adversarial continuation — 2026-10-05, commit results and row identity

- [x] Restore only owned QA8928 and emulator5584; PostgreSQL55447 already
  available. Pin installed APK `7be9d8a4d5dadf6c2edee0e5410d1b0f276756c4413e8f6d7de6dc80d44d37e2`
  and server `9ed789bdcf89efbea1da98f7f1ef399e038123c881460f06a5ad075ab3d30344`.
  Workspace APK file has changed since prior turn; do not install/claim it is
  this pinned build. Port8100 and other emulators untouched.
- [x] Create fresh synthetic `qa_ui.adv051_ui_oct05`,052/053 counterparts,
  audit table and trigger functions; never reset prior evidence.
- [x] First ADV-051 harness attempt reached target but could not save evidence
  because its new output directory was absent. No Paste/Import submitted,
  native target empty. Fix runner directory setup, reuse verified target view.
- [x] ADV-051 actual Android: confirm three rows and Import once. Deferred FK
  fails for input row2 at commit; UI2inserted/1failed names `Input row 2` and FK.
  Native IDs exactly1,3. This demonstrates per-row commits, not atomic batch
  rollback. Fix N/A. Evidence: `artifacts/dbqa-adv-ui-20261005-next/ADV-051-*`
  and `manual-ui-evidence.jsonl`.
- [x] ADV-052 Android: first navigation timed out before Paste/Import; native
  target empty. Recover loaded Explorer scope without replaying mutations.
  Single import then reports2inserted/1failed, `Input row 2` explicitly states
  trigger/rule suppression. Native IDs exactly1,3. Fix N/A.
- [x] ADV-053 Android: UI3inserted/0failed, three uppercase persisted titles,
  exactly three corresponding audit effects. Explicit Run refresh shows
  ROW-1/ROW-2/ROW-3, not optimistic lowercase inputs. Fix N/A. Same evidence
  directory includes per-case confirmation/result and persisted-grid screenshots.
- [x] ADV-025 actual Android/native edits: delimiter-collision pair first tested
  on the old build (`pipe-first` → `pipe-first-edited`, other pair unchanged).
  On rebuilt APK37e29e06, edit empty/Unicode key `(empty,雪)` and `(é,empty)`
  once each. Full native before/after comparison proves only the intended title
  changes, with every key and neighboring row preserved. Screenshots/XML and
  SUBMIT_ONCE/PASS_UI_NATIVE records retained in the same evidence directory.
- [x] ADV-030 first actual Android test **FAIL wrong physical row**. Query
  `SELECT id + 100 AS id, title FROM "qa_ui"."adv030_ui_oct05" WHERE id = 1`
  exposes Edit/Paste replacement for computed id101. Synthetic rows1/101 share
  returned title but carry distinct omitted proof markers. Saving
  `must-not-target-101` changes physical101 (unrelated), leaves physical1 alone,
  and optimistically shows the wrong value in the query grid. Preserve native
  rows and `ADV-030-*` screenshots/XML plus SUBMIT_ONCE/failure JSONL.
- [x] After interruption, re-read source: projection-safety fix not present;
  restart only QA PostgreSQL55447/server8928, verify original wrong-row change
  persists. ADV-025 fixture remains untouched. No uncertain submission replay.
- [x] ADV-030 fix: existing-row mutations require unchanged direct projections
  and the executed result SQL, not just editable SQL draft text. Calculated,
  renamed, duplicate/ambiguous projections fail closed. Save/delete callbacks
  recheck eligibility. Automatic-refresh source checks remain independent.
- [x] Focused shared regressions17 passed; `mobileCheck :androidApp:assembleDebug`
  passed (6m18s). Installed ARM64 APK SHA-256
  `37e29e066eac84a04cc7865098673708391da80f0c545e17106dcb3c9a3a801d`.
- [x] ADV-030 rebuilt Android retest: explicitly execute original calculated-ID
  query; result id101 exposes no Edit/Paste replacement/SetNULL. Change SQL draft
  to a safe SELECT without Run: stale result remains read-only, selected row has
  no Delete action. Native failure-fixture rows unchanged. Positive direct-table
  composite-key edits above succeed. Initial harness assertion encountered a
  restored safe table query, not the computed query; no write submitted. Another
  navigation attempt hit autocomplete; dismiss with Escape before continuing.
  Neither setup interruption is counted as a product failure/pass.
- [x] Root/apps `git diff --check` passed after source fix.

### Adversarial continuation — 2026-10-05, second expansion

- [x] Reconcile 253-case plan against historical ledger: no inherited manual
  passes for ADV-061–090; preserve all earlier first-failure/retest records.
- [x] Restore only isolated PostgreSQL55447 and QA8928. Running server matches
  SHA-256 `fca7552337e88bcd30c05d042c1f29a1fd8d92c6d586a30eaceb96c3cc791456`.
  Production8100 and the other pre-existing emulators remain unchanged.
- [x] Owned emulator5584 first launch hits System UI ANR before case execution;
  select Wait once, then verify responsive navigation and QA8928 endpoint.
  Evidence: `artifacts/dbqa-adv-ui-20261005-resume/system-ui-anr.{png,xml}`.
  This is a setup interruption, not a database-feature failure or pass.
- [x] Pin installed Android APK SHA-256
  `0713d0d74ca3270429f8e9d2255dcab674ab56d85e29ee978007afdd0e817289`.
- [x] ADV-076 API/native first attempt and independent fresh-fixture repeat:
  without default partition, native IDs1,3 in exact low/high children, unmatched
  input row2 fails23514; response attempted3/inserted2/failed1. With default
  partition, all3 commit and ID2 routes exclusively into the default child.
- [x] ADV-077 API/native first attempt: valid follow-up inserts succeed but
  failed backend PID is not reused; mark reuse variant BLOCKED, not passed.
- [x] ADV-077 fresh-fixture follow-up: bounded four-slot pool exercise proves
  failed backend1430462 reused by successful ID103. Native trigger SELECT reads2
  on that same session; final IDs exactly1,3,100,101,102,103. This is backend
  evidence with native trigger instrumentation, **not Android query coverage**.
- [x] No product failure found in ADV-076/077 backend probes; fix N/A for those
  attempts. Evidence: `artifacts/dbqa-adv-partition-reset-20261005-{a,b}-results.jsonl`;
  fresh schemas and synthetic fixtures retained, never reset/replayed.
- [x] ADV-001 actual Android confirmation race: review A(id1), externally publish
  B(id2) with the foreground clipboard helper while confirmation remains open,
  then tap Import once. UI says1inserted/0failed and native target contains only
  reviewed A; B never inserted. Fix N/A. Target `qa_ui.adv001_resume_oct05` retained.
- [x] ADV-011 Android raw duplicate keys: top-level, nested, and escaped-equivalent
  names explicitly rejected; no confirmation/write. Visually reviewed rejection
  screenshots and verified unchanged native target after each recorded variant.
- [x] ADV-012 Android duplicate column metadata: string and object forms each
  show duplicate-column rejection; native rows unchanged. Fix N/A.
- [x] ADV-020 Android trailing second JSON, trailing garbage, and appended CSV:
  each shows malformed-JSON rejection, no prefix import, native rows unchanged.
- [x] Evidence under `artifacts/dbqa-adv-ui-20261005-resume/`: per-variant raw
  payload/baseline/native JSON, confirmation/result XML, screenshots, and
  `manual-ui-evidence.jsonl`. Android's clipboard-read toast initially obscured
  error capture; repeat only the rejected preview to photograph the actual app
  rejection. This is an evidence-capture correction, not a product fix.
- [x] ADV-016 PostgreSQL Android lane: escaped NUL in input row2 is rejected
  with explicit U+0000 reason and one-based row2 label; rows601/603 persist
  exactly,602 absent; UI2inserted/1failed matches native. Other-engine lanes pending.
- [x] ADV-006 two read-only Android attempts recorded actual A `PgSleep` PIDs,
  but neither observed a B grid before A completed. **INCONCLUSIVE**, not pass
  or reproduced stale-result defect. Evidence:
  `artifacts/dbqa-adv006-ui-20261005{,-short}/manual-ui-evidence.jsonl`.
  Harness review identified unconditional Back after paste as a possible client
  navigation/cancellation disturbance; next attempt must omit Back/Escape and
  explicitly verify working-state/editor stability before F5 submission.
- [x] ADV-017 high-surrogate first attempt: **FAIL validation UX**. Raw escaped
  `\uD800` reaches confirmation; Import shows `Import outcome not confirmed`
  with `Reason: Input length = 1`, instead of rejecting invalid text before submit.
  XML labels were empty but the saved screenshot captures the actual error.
  Native701 absent before interruption and after restarting QA PostgreSQL.
- [x] ADV-017 low-surrogate independent reproduction: escaped `\uDC00` for702
  gives the same error, native702 absent. Valid emoji pair control703 succeeds
  once with exact UTF-8 `f09f9880` and UI1inserted/0failed.
- [x] ADV-017 fix: shared clipboard parser validates raw UTF-16 and decoded
  JSON strings (including nested values, keys, and metadata) before confirmation.
  Invalid pairs receive a specific unpaired-surrogate error; valid pairs and
  literal escaped text are preserved. No encoding replacement/coercion added.
- [x] ADV-017 focused shared JVM tests:28 passed,0failures/errors.
- [x] ADV-017 rebuilt Android exact rejection/control retest: both original
  high701 and low702 inputs explicitly reject before confirmation; native rows
  remain absent. Valid emoji713 and literal `\uD800`714 preserve exact bytes.
  Fixed ARM64 APK SHA-256:
  `fba731910ff87cc44ae68e4f27c54f742db00158c064e88ccfdbf701177c67f5`.
  Evidence: `017-{high,low}-fixed-attempt1-rejection.png` and the exact-fix-retest
  record in `artifacts/dbqa-adv-ui-20261005-resume/manual-ui-evidence.jsonl`.
- [x] Fixed build: `mobileCheck :androidApp:assembleDebug` completed with one
  worker; root/apps whitespace checks passed. Gofile upload completed once:
  https://gofile.io/d/aLDoU5vi — 10,255,182 bytes; service size and MD5 match
  local APK (`e80aa5d125ac016ecaa5a333706d5e01`). Downloaded SHA-256 was not
  independently verified; do not describe service verification as a download check.
- [x] ADV-018 JSON-text direction:715 contains exact CSV-looking header/newline
  text as one cell. CSV-object direction:716 contains exact `{"x":[1,2]}` as
  text; UI1inserted/0failed and native readback agree. The interrupted attempt
  failed to find its table tab before Paste; native716 was checked absent before
  the corrected navigation and single Import. CSV-array direction remains pending
  at this checkpoint.
- [x] ADV-006 third read-only attempt omitted Back/Escape; B editor contents
  reverted before F5 could be sent. No B submission/response proven, so the race
  remains INCONCLUSIVE. Evidence: `artifacts/dbqa-adv006-ui-20261005-focused/`.
- [x] ADV-013 actual Android/native: ordinary `rows`, `columns`, `type`,
  `version` fields preserved individually in a row array and together in a bare
  row object; IDs801–805 exact in fresh `qa_ui.adv_parser_resume_oct05`.
- [x] ADV-014 actual Android/native: marker-looking JSONB object remains an
  object, including an unsupported version999 and invalid base64-looking value.
  IDs811/812 exact; no implicit binary decode or value loss. Fix N/A.
- [x] ADV-018 remaining CSV-array direction:820 contains complete JSON array
  text verbatim; UI1inserted/0failed. Full native-table equality proves no nested
  ID999 row appeared. Both outer-format directions now tested.
- [x] ADV-019 partial Android/native: BOM, zero-width space, NBSP inside quoted
  JSON strings preserve exact values (1400–1402). Initial BOM and NBSP are
  accepted by the parser's explicit prefix/trim policy (1410/1411). Initial
  zero-width space produces a CSV parsing-error accessibility toast with no
  confirmation or write (1412 absent); screenshot missed toast, so retain that
  evidence limitation. Invisible column-name variants remain pending.
- [x] ADV-015 first Android/native: nested payload depths16/64 succeed exactly.
  Depth128 **FAIL validation UX**: confirmation accepted, then server HTTP400
  `recursion limit exceeded`, displayed as `Import outcome not confirmed`.
  Native1128 absent; no uncertain import replayed. Tiny follow-up1300 succeeds,
  demonstrating responsiveness. Saved `parser-depth128-first-failure.{png,xml}`.
- [x] ADV-015 implement iterative pre-parse depth check for JSON-shaped input:
  maximum96 containers with server-envelope headroom, ignoring quoted/escaped
  brackets and leaving CSV/TSV text unchanged. Focused parser suite32passed.
- [x] ADV-015 rebuilt Android: exact failed depth128 input and depth256 both
  reject before confirmation with the new depth-limit message; native unchanged.
  First rapid256 probe missed the fresh toast event; isolated rejected-preview
  retry captured it. No Import was replayed. New depth64 row1564 and tiny1500
  import exactly; emoji1505 and literal escape1506 remain exact (UI4/0).
- [x] Recheck ADV-017 high/low surrogate rejection and ADV-011 duplicate keys,
  ADV-020 trailing JSON on final APK; fresh accessibility rejection events,
  no confirmation, no native writes. `final-*-toast.png` / `final-*-rejected.xml`
  and per-variant JSONL retain evidence. App restart restored connection/table.
- [x] Final mobile build `mobileCheck :androidApp:assembleDebug` passed.
  Tested ARM64 SHA-256:
  `7be9d8a4d5dadf6c2edee0e5410d1b0f276756c4413e8f6d7de6dc80d44d37e2`.
  Replacement Gofile https://gofile.io/d/xSj5WB1u verified service size10,255,182
  and MD5 `0c10f83dc031d3fcb575287e482078ba`; no independent downloaded SHA claim.
- [x] ADV-019 column-name first test **FAIL silent field loss**. Fresh columns
  BOM+marker, ZWSP+marker, NBSP+marker alongside ordinary marker: Android UI3/0;
 1420/1421 preserve the special-column values, but1422 NBSP+marker is NULL,
  losing `nbsp-column` while ordinary marker retains `plain-sentinel`.
  Server paste endpoint trims source metadata names, collapsing NBSP+marker
  into marker. Preserve these original rows as failure evidence; never replay.
- [x] ADV-019 exact source-name server fix implemented for structured clipboard,
  explicit sourceColumns, and inferred row keys. Shared extraction preserves
  identifiers rather than trimming/filtering them. Regression checks actual
  SQLite persistence for exact leading/trailing-space, NBSP, BOM and ZWSP names
  alongside ordinary marker. Original PostgreSQL1422 remains untouched.
- [x] ADV-019 focused regression passes; ambiguous-name regression also passes.
  Compiled test binary database suite104passed with live PostgreSQL enabled;
  MySQL/MariaDB live gates not enabled. Rebuilt CLI successfully; restart only
  isolated QA8928, server SHA-256
  `9ed789bdcf89efbea1da98f7f1ef399e038123c881460f06a5ad075ab3d30344`.
  Port8100 unchanged. Temporarily close owned5584 to reduce build memory
  pressure, then relaunch same AVD without wiping app/fixture data.
- [x] ADV-019 fresh Android/native retest on rebuilt QA server:1430–1432
  preserve BOM/ZWSP/NBSP column values exactly, with ordinary marker's separate
  `plain-sentinel` unchanged. UI3inserted/0failed matches native. Original failed
  row1422 retained, not repaired/replayed. Evidence:
  `adv019-columns-fixed-{confirmation,result}.{png,xml}` and
  `invisible-column-fixed-retest` JSONL record.
- [x] Final root/apps whitespace checks pass. APK unchanged since verified
  final upload; server-side column-identity fix requires the rebuilt server,
  not merely installing the APK. QA8928 runs that build;8100 was not restarted.
- [ ] Remaining unexecuted ADV cases/variants still pending; no inferred passes.


### Adversarial execution — 2026-10-05, first live commit checks

- [x] Read current checklist/source and isolate QA8928/PostgreSQL55447; leave
  production8100 unchanged. Initial emulator startup deferred while available
  host memory was about1.1GiB; begin low-memory API/native checks instead.
- [x] Create fresh `adv_commit_20261005a` schema and QA connection33 with empty
  targets; no existing fixture reset. Pin running server SHA-256
  `245ff24080f3d5c9dbbd6540cc0d6af270ccd4c7fac94b9cbe4186f3df8897ce`.
- [x] ADV-051 first API/native attempt: deferred FK rejects input row2 at its
  transaction completion; response2inserted/1failed matches native IDs1,3.
  This is supporting backend evidence; Android UI variant remains pending.
- [x] ADV-052 first API/native attempt: **FAIL**. BEFORE INSERT trigger returns
  NULL for row2; response says3inserted/0failed/success, native has only IDs1,3.
- [x] ADV-052 implement affected-row accounting fix: direct row insertion
  rejects zero affected rows; shared transfer insertion returns actual driver
  counts; transfer jobs/file imports/portable imports account for suppression.
- [x] ADV-052 fresh fixture `adv_commit_20261005b` on rebuilt server:
  attempted3/inserted2/failed1/success=false, failure rowIndex1, native IDs1,3.
  Both neighboring ADV-051 and ADV-053 API/native checks pass again.
- [x] Final rebuild including portable-import caller passes. Fresh fixture
  `adv_commit_20261005c` repeats all3checks successfully; server SHA-256
  `fca7552337e88bcd30c05d042c1f29a1fd8d92c6d586a30eaceb96c3cc791456`.
  Real table-transfer job additionally reports copied2/failed1, indexed failure1,
  ROW_COPY_FAILURES warning and exact native IDs1,3. Its existing terminal
  `succeeded` means the job finished with warnings, not that all rows copied.
  Evidence: `artifacts/dbqa-adv-commit-20261005-c-results.jsonl`.
- [x] SQLite and live PostgreSQL focused regressions pass, including empty,
  all-suppressed and mixed inputs. Broader `cargo test -p iowb-server database::
  -- --test-threads=1` passes103 test functions with live PostgreSQL enabled.
  Environment-gated MySQL/MariaDB live variants were not enabled; do not infer
  those engines were exercised from the harness's aggregate count.
- [x] ADV-053 first API/native attempt: trigger uppercases all3titles and creates
  exactly3matching audit rows; response3inserted matches native values. Actual
  Android refresh/display variant remains pending.

Evidence: first failure `artifacts/dbqa-adv-commit-20261005-results.jsonl`;
retest `artifacts/dbqa-adv-commit-20261005-b-results.jsonl`; opt-in runner
`artifacts/dbqa-adv-commit-20261005.py`. No uncertain write was replayed.

- [x] ADV-034 API/native: verified restricted role is nonowner, NOSUPERUSER,
  NOBYPASSRLS. Alternating A/B/A/B inserts only two A rows; B failures use42501
  at indices1,3. Hidden B sentinel remains unchanged and is absent from errors.
- [x] ADV-035 API/native: SELECT+INSERT permits paste and denies cell UPDATE;
  SELECT+UPDATE denies paste and permits cell replacement. Native readback
  confirms no permission-bypassing fallback. Revoke fixture grants and disable
  role login afterward; retain only synthetic rows/connection as evidence.
- [x] Actual emulator5584 attempt: launch initially times out, eventually opens
  Database, then Android displays `System UI isn't responding` while host
  available RAM falls below1GiB. Stop only the owned emulator. No database UI
  mutation submitted; this is inconclusive setup failure, not an app defect.
- [ ] ADV-001 and remaining Android cases need a responsive owned device.
- [ ] ADV-006 source audit suggests a possible stale explicit-query completion;
  NOT REPRODUCED. Prepared barrier-based read-only manual plan at
  `artifacts/dbqa-adv006-plan.md`; no speculative product fix.

Permission evidence: `artifacts/dbqa-adv-permissions-20261005-results.jsonl`.
Five ADV cases now have API/native observations (034,035,051,052,053); their
Android UI variants are **pending**. The other55 ADV cases have no completed
execution in this cycle. No new manual Android pass is claimed.

Product edits are server-side only (`row_operations.rs`, `transfer_runtime.rs`,
`transfer_jobs.rs`, `table_operations.rs`, and focused `tests/data_integrity.rs`).
No mobile source/APK changes or Gofile upload belong to this cycle. Root/apps
whitespace checks pass. Production8100 has not been restarted or upgraded.

#### Interrupted-run recovery — October 5, 2026

- [x] Read back the saved first-failure/fix/fresh-retest evidence; do not replay
  completed inserts. The final server binary still matches the recorded SHA.
- [x] Confirm QA8928/PostgreSQL55447 are stopped after interruption and8100
  remains listening. No production restart performed by this test lane.
- [x] Try owned lightweight ATD5582 with requested1024MiB RAM. Emulator raises
  allocation to2560MiB while host available memory is about1802MiB; stop it
  before app interaction. Confirm no owned5582/5584 emulator remains running.
- [x] Root diff whitespace check passes; preserve unrelated work.
- [ ] Resume actual Android cases only with adequate resources or a responsive
  explicitly available test device. This startup attempt adds no case pass.

### Continuation — 2026-10-05, owned5584 uploaded-APK lifecycle smoke

- [x] Reconcile all 163 source IDs with 163 ledger entries; preserve historical
  evidence. Boot only owned `dbqa_5584_standard` on ADB5056/emulator5584.
  This lane did not use emulator5594 or restart production8100/8787.
- [x] Fresh separate-activity calibration: Ctrl+C/V each deliver all four
  callbacks; Meta+C/V each accept four events but deliver zero callbacks.
  Stop before claiming Workbench Meta coverage.
- [x] CLIP-11 preflight reports `Service clipboard: found`; no actual
  absent-service test is possible as configured. Host audio sinks/sources
  report connection refused; this AVD has audio disabled. No speech pass.
- [x] Install the exact October4 uploaded ARM64 APK from its pinned artifact,
  not the subsequently replaced build-output APK. Installed SHA-256 matches
  `0713d0d74ca3270429f8e9d2255dcab674ab56d85e29ee978007afdd0e817289`.
  Normal replacement refused an older version code; explicit debug downgrade
  succeeded without clearing the owned test app's data.
- [x] Start isolated PostgreSQL55447 and QA8928. Correct setup-only failures:
  PostgreSQL initially attempted occupied5432; explicit55447 succeeded.
  QA8928 initially required auth; restore its open-auth test configuration
  and relaunch the app. No production service/configuration change.
- [x] Actual UI: Inspector → Clear clipboard, then reopen the existing QA
  table. Paste is absent with both clipboard stores empty.
- [x] External clipboard helper writes one JSON row. On return, Paste appears.
  Tap Paste: confirmation names Android clipboard, exact QA table and columns
  `id, instant`. Cancel; no Import submitted.
- [x] Navigate Database → Projects → Database: Paste remains visible.
- [x] Clear system clipboard externally: Paste disappears. Repeat navigation:
  Paste remains absent. This exercises focus return and view reattachment.
- [x] Native before/after readback of `qa_ui.qa_ui_named_zone_oct03zone1`
  matches all five IDs/timestamps exactly. Final app responsive, PID4293;
  AndroidRuntime error log empty.
- [x] Remove the temporary key-calibration package; shut down this lane's
  emulator5584, QA8928 and PostgreSQL55447. Root/apps diff whitespace checks pass.

This adds actual **UI regression coverage for the October4 clipboard lifecycle
fix**, not a manual absent-service, Meta, or audible-TalkBack pass. No new
product defect was reproduced; no application source change was made. Remaining
manual variants still require suitable platform/input/audio capabilities.

### Continuation — 2026-10-05, independent inventory and fresh keyboard calibration

- [x] Reconcile all 163 source IDs against 163 unique ledger rows; no missing or
  duplicate IDs. Independent read-only audit agrees. Historical failures retain
  their linked fixes/retests; no additional unresolved reproduced defect found.
- [x] Inspect the available emulator5594 before input. Workbench is on the prior
  Kanban fixture's load-error screen, with no open edit/import dialog. For this
  continuation root owns the device; no database or Kanban mutation was issued.
- [x] Install the existing separate key-delivery calibration APK. Execute and
  inspect each result individually: Ctrl+C delivers all four expected events;
  Meta+C accepts four injections but delivers zero events and opens Contacts;
  Meta+V accepts four injections but delivers zero events; Ctrl+V delivers all
  four expected events. Both Ctrl pairs verify; both Meta pairs fail calibration.
- [x] Capture and visually review Meta screenshots. Stop before Workbench Meta
  tests because the prerequisite input delivery fails. This is fresh platform
  calibration evidence, not a new product pass or a Workbench shortcut failure.
- [x] Remove the temporary calibration package and restore Workbench foreground;
  inspect the settled screenshot. Prior fixture still has its proxy connection
  error. No endpoint/settings/data changes and no service restarts were made.
- [x] Recheck audio availability: PulseAudio connection refused; the running ATD
  was launched with `-no-audio` and has no installed TalkBack/TTS package found.
  No actual audible-output test or pass is claimed.
- [ ] CLIP-11 unavailable-service manual variant remains blocked: absent-service
  handling has automated coverage, but selective real platform service removal
  is not available in this setup. Do not substitute a mocked pass.
- [ ] UI-06/UI-07 Meta variants remain blocked by the independently reproduced
  input-delivery failure. They need a device/input path passing calibration.
- [ ] UI-10 actual spoken output remains unverified; prior actual TalkBack
  focus/actions and layout results stand. It needs an audio-capable setup and
  an observer of the speech.

Evidence: `artifacts/db-copy-paste-continuation-20261005/` (four calibration
traces, visually reviewed screenshots, machine-readable coverage inventory).
This continuation adds zero manual product passes and reproduces zero new
product defects. Only this ledger/evidence changed; no mobile source changes,
APK rebuild/upload, or port8100 restart was necessary. These are qualified
results across existing builds, not a claim of 163 exhaustive current-build
manual passes.


### Fix continuation — 2026-10-04, clipboard service lifecycle

- [x] Inspect current Android code before editing. Found service lookup outside
  the read/clear guards and unguarded listener registration/removal in the
  database view lifecycle. This is a source-confirmed exception path, not a
  newly reproduced manual platform crash.
- [x] Guard optional service lookup, reads, registration and removal; retry
  failed registration on attachment/focus and unregister the original service.
  Do not cache an unavailable service forever or duplicate a successful listener.
- [x] Move clear-operation service lookup inside its existing failure handler.
  Leave internal-row copy/paste storage and truthful copy-publication errors intact.
- [x] Seven focused shared JVM fault-injection tests pass: absent service,
  throwing lookup, registration failure, later availability, idempotence/original
  service removal, removal failure and failed-read recovery.
- [x] Independently inspect Ctrl/Meta routing: both reach the same database
  commands when delivered, and EditText copy/paste bypasses database row actions.
  No keyboard code change justified by the platform-intercepted Meta evidence.
- [x] Full `mobileCheck` passes: 670 shared JVM tests, zero failures/errors/skips,
  plus web-harness Kotlin compilation. Android `assembleDebug` passes.
- [x] Root/apps `git diff --check` pass.
- [x] Rebuilt ARM64 APK uploaded to Gofile: https://gofile.io/d/LCq7DgI7 .
  Local SHA-256 pinned:
  `0713d0d74ca3270429f8e9d2255dcab674ab56d85e29ee978007afdd0e817289`.
  Provider size10,222,414bytes and MD5 match the pinned upload. This is service
  integrity verification, not a downloaded SHA-256 roundtrip or device install.

This closes a code-hardening gap with **automated fault-injection coverage**.
It does not turn CLIP-11 absent-service manual coverage, UI-06/07 Meta delivery,
or UI-10 audible speech into manual passes. No QA device is currently connected;
host audio remains unavailable. Production port8100 is unchanged.

### Continuation — 2026-10-04, remaining-case feasibility recheck

- [x] Read current source checklist and latest execution ledger; preserve
  historical first-failure/fix/retest records without replaying completed work.
- [x] Revalidate the three retained full-body boundary records: complete sent
  byte counts and expected400/400/413 responses. This is evidence readback,
  not a new execution of ROBUST-10.
- [x] Check owned test-device availability: ADB5056 lists no devices. The
  unrelated host emulator5594 is not claimed or used for database QA.
- [x] Check host audio: both `pactl list short sinks` and `sources` fail with
  connection refused. No audible screen-reader result can be claimed.
- [x] Reinspect Meta calibration: accepted injections produced zero callbacks
  and Android Contacts routing; completed Ctrl cases are not Meta substitutes.
- [ ] CLIP-11 absent-service manual variant: needs a safe real platform setup;
  no selective service-removal route exists in the prepared harness. A mocked
  null ClipboardManager test would be supporting instrumentation, not manual.
- [ ] UI-06/07 Meta manual variants: need a keyboard/device input path that
  delivers Meta events to a separate calibration app before testing Workbench.
- [ ] UI-10 spoken output: needs an audio-enabled test setup and an observer
  able to verify the actual speech, beyond the completed focus/action tests.

No new manual case was executed or passed in this feasibility recheck. No
new product defect was reproduced, no source fix or build was justified, and
port8100 was left running unchanged. These blockers require test capability
changes; another run of already passed cases would not close them.

### Continuation — 2026-10-03, full request-size boundaries

- [x] Reconcile all163 IDs and latest temporal results; do not replay them.
- [x] Recheck environment: prior QA8928/emulator5584 were stopped, not assumed
  live. Leave production8100/8787 and other agents' devices untouched.
- [x] ROBUST-10 first full below-limit request: send1,048,575,999bytes;
  HTTP400 `INVALID_CLIPBOARD` proves body extraction and JSON parsing completed.
- [x] Exact-limit preflight first stopped before sending due competing host
  memory use. Retain as a resource guard, not a failed application request.
- [x] Exact-limit attempt under reduced1.5GiB auxiliary cap: process OOM-killed,
  no response. Preserve as test-resource failure, not product-limit verdict.
- [x] Fix runner failure capture; restore original4GiB budget in a fresh
  auxiliary unit after checking >6GiB available. No product change.
- [x] Exact-limit retest: send1,048,576,000bytes; HTTP400 `INVALID_CLIPBOARD`.
- [x] Above-limit: send1,048,576,001bytes; HTTP413 length-limit exceeded.
- [x] Verify complete sent-byte counters, body hashes, zero OOM events on
  successful runs, final health and zero configured database connections.
- [x] Reconcile remaining platform blockers and root/apps whitespace checks.

Evidence: `artifacts/db-copy-paste-body-limit-20261003/`; runner:
`artifacts/dbqa-body-limit-20261003.py`. Pinned server SHA:
`245ff24080f3d5c9dbbd6540cc0d6af270ccd4c7fac94b9cbe4186f3df8897ce`.
Fresh loopback8936 config/workspace:
`/home/giofahreza/.local/state/io-workbench-dbqa/body-limit-20261003.S0ZOny/`.
Payload is an empty sourceRows object padded with JSON whitespace, streamed
in<=64KiB chunks. The handler refuses empty rows before connection lookup;
no user database is configured or touched. Below/at-limit HTTP400 is expected
semantic validation, not transport rejection. Above-limit413 is the guard.
Successful retest unit `dbqa-body-limit-8936-oct03-retest.service` had4GiB cap,
zero swap allowance,1CPU quota, finite10minute lifetime; observed cumulative
peak1,102,790,656bytes and zero OOM/max events. The first reduced-cap failure
and its unknown sent-byte count remain explicitly recorded; not overwritten.

This closes the enabled-limit **API transport** variant, not a1GB Android
paste/rendering pass. Disabled-limit variant stays N/A because no setting
exists. No new product bug reproduced, no product build/upload required.
CLIP-11 absent clipboard service, UI-06/07 calibrated Meta dispatch and UI-10
audible TalkBack remain unverified: no safe service-removal path, prior Meta
events intercepted by Android, and no operational audio sink/source. Existing
Ctrl, denied-write and TalkBack action passes do not substitute for these.
Auxiliary8936 was stopped after readback; no listener remains. QA fixture and
failure evidence were retained. Production was not restarted.

### Continuation — 2026-10-03, delete-only permissions and time-only values

- [x] Reconcile completed cases and retained six unsigned-copy retests; no
  replay. Verify QA8928 binary, PostgreSQL55447 identity and owned emulator5584.
- [x] ROBUST-02 / CELL-11: actual Android SELECT+DELETE-only role deletion;
  permitted Delete, unchanged neighbor, then revoke DELETE after confirmation.
- [x] DATA-08: actual TIME(6)/TIMETZ(6) import with fractional seconds,
  offsets, end-of-day boundary and invalid time; verify native exact values
  and truthful per-input failures.
- [x] DATA-08: named-zone DST gap/fold inputs against native PostgreSQL
  canonical instants, explicit-offset comparators and invalid-zone rejection.
- [x] Inspect time-only UI/copied values against stored values; fix a
  demonstrated defect with a failing regression, then repeat the actual route.
- [x] Final evidence reconciliation and targeted regression checks.

Host snapshot for ROBUST-10:14GiB RAM,5.6GiB available, existing7.2GiB swap
use and concurrent QA workloads. Do not silently substitute a header-only
probe for the unexecuted full1GB below/at/above body tests.

Delete-only evidence: `artifacts/db-copy-paste-delete-only-20261003/oct03del1-roybxd8h/`.
Actual first Delete succeeds with no UPDATE privilege and leaves the complete
neighbor unchanged. Revoke DELETE after the second confirmation opens: actual
Delete reports0deleted/1failed, Input row1, explicit permission denial; neighbor
unchanged. The runner stopped between operations when Android spell suggestions
obscured Explorer focus. Screenshot reviewed, popup dismissed, resumed only the
unsubmitted second action via the existing table tab. Runner now reuses that
tab; the successful Delete was not replayed. No product fix needed for this case.

TIME/TIMETZ evidence: `artifacts/db-copy-paste-time-only-20261003/`.
The original `oct03time1-import-asvxjkbz` stopped before submission because
the QA Explorer's many cached schemas exceeded the driver's bounded search.
Verified the prepared table was empty with its original three-column schema;
created isolated connection alias32 (`00 QA temporal oct03`) and resumed once
in `oct03time1-resume-u003ksdj`. Actual Import reports3inserted/1failed and
identifies invalid25:00 as Input row4. Native TIME(6)/TIMETZ(6) preserve all
six fractional digits, +05:45/-03:30 offsets and24:00 boundary.
`oct03time1-copy-existing-whkuls36` then opens the native table (no text casts),
selects all three rows and uses actual Copy. Both saved internal rows and real
Android clipboard equal native canonical strings, including24:00; source
connection/table attribution and unchanged schema verified. No product fix
needed. Navigation/test-driver changes are not counted as product fixes.

Named-zone evidence: `artifacts/db-copy-paste-named-zone-20261003/`.
`oct03zone1-import-m5nenvzc`: actual Android Import inserts5valid rows and
rejects the invalid zone as Input row6. Spring-gap and autumn-fold named-zone
inputs, explicit-04:00/-05:00 comparators and fractional-fold value all equal
native PostgreSQL casts to integer microseconds. In
`oct03zone1-copy-existing-z7neowk6`, actual native SELECT and Copy preserve all
five instants in both internal and Android clipboards. Recasting the copied
UTC strings yields identical integer microseconds, including123456fraction;
schema/data unchanged. Preview screenshot reviewed. No product fix needed.

Closure:16offline temporal/delete-only driver tests pass; root/apps whitespace
checks pass. QA8928 remains healthy on the unchanged pinned binary and v64 APK.
No product source changes, rebuild, upload, or production8100 restart were
needed for this continuation. All163 original IDs remain accounted for, not
universally passed. Remaining named limitations: ROBUST-10 complete1GB-body
boundaries (unsafe under current memory constraints), CLIP-11 absent clipboard
service (no safe injection), UI-06/07 Meta dispatch (calibration fails), UI-10
audible TalkBack (no-audio emulator). Physical ARM, optional PostGIS/SRID,
two simultaneous Android clients and exhaustive engine/type cross-products
remain outside the demonstrated coverage. Earlier unchecked historical
entries are not silently promoted to passes or rerun as new cases.

### Late continuation — 2026-10-02, remaining permission/engine variants

- [x] Reconcile previous v64 results; do not replay completed imports.
- [x] Restore preserved QA8928, PostgreSQL55447, MySQL53308/MariaDB53307 and
  emulator5584 after the host restart. Installed v64 remains available.
  Kanban emulator5592 and production8100 are not test targets.
- [x] DATA-05 / TRANSFER-04/05/19: actual unsigned64 new-schema copy; inspect
  destination type and exact values, source unchanged, truthful job outcome.
- [x] CELL-04/07/10 / ROBUST-02: column-only UPDATE grant; permitted edit,
  forbidden-column control, revoke granted column after editor opens.
- [x] CELL-11 / ROBUST-05: MyISAM guarded Save and Delete reject safely;
  InnoDB adjacent control remains usable.
- [x] For each confirmed failure: preserve first evidence, add a focused
  regression, apply a narrow fix, rebuild and run the same actual UI route.

No newly queued variant is passed merely by an earlier supporting server test.

#### Resumed evidence — October 2, 2026, 23:40 WIB onward

- [x] Reconcile interrupted MyISAM edit `late02a`: process absent, only
  fixture-ready evidence present; native rows unchanged. Retain as incomplete,
  not a passed Save and not a replay target.
- [x] Fresh MyISAM Save `late02b`: actual Android Save refused with the full
  InnoDB safety explanation; both native rows and schema unchanged.
- [x] Fresh MyISAM Delete `late02b`: actual selected-row Delete reports
  `Deleted 0 row(s); 1 failed`, `Input row 1`, and the InnoDB safety reason;
  both native rows and schema unchanged.
- [x] InnoDB adjacent successful Save/Delete control.
- [x] PostgreSQL column-grant allowed/denied/revoked UI variants.
- [x] Unsigned64 new-schema first test, fix if needed, and fresh UI retests.

MyISAM screenshots, hierarchies and native oracles:
`artifacts/db-copy-paste-myisam-guard-20261002/edit-late02b-mv5pn4f3/`
and `artifacts/db-copy-paste-myisam-guard-20261002/delete-late02b-gkzhocxa/`.
No product fix was needed for these two refusal tests.

Adjacent InnoDB control completed October 3,00:03 WIB:
`artifacts/db-copy-paste-innodb-control-20261002/late02ic1-gkqk9zw5/`.
Actual Save changed only row1 title; actual Delete reports1deleted and leaves
exactly row2 with its original bytes. Schema unchanged throughout.

Column-grant evidence: `artifacts/db-copy-paste-column-grants-20261002/oct02cg1-fud2jb95/`.
Allowed title Save changed only title. Secret Save was rejected with a readable
permission error and an unchanged full row. Revoking title UPDATE with its
editor already open produced HTTP403 and preserved the row. The initial runner
stopped before the secret action because the column was off-screen; repaired
grid-scrolling navigation and resumed only the unsubmitted controls. The
successful Save was not repeated. Strengthened the permissive live regression
`postgres_column_update_grant_is_not_reported_as_denied` to require success and
the returned full row; isolated PostgreSQL55447 run: **1 passed**.

**Confirmed first failure — unsigned new-schema copy:** actual Android
MySQL→MySQL Schema + data job `dbjob_465ea6fbebd84bcc841f668d367c2687`
created signed `bigint` from source `bigint unsigned`. Both values
9223372036854775808 and18446744073709551615 failed with native1264 range errors.
UI and raw job truthfully report Failed,0copied/2failed; source rows/schema
unchanged, created target empty. Evidence retained in
`artifacts/db-copy-paste-unsigned-schema-transfer-20261002/late02us1-fzhgxf_o/`.

- [x] Preserve first unsigned-schema UI failure and independent native oracle.
- [x] Failing regression for exact unsigned destination mapping: both initial
  type-mapping and SQLite exact-value tests failed before product edits.
- [x] Narrow mapping/literal fix and focused regression pass:4tests with live
  MySQL53308/PostgreSQL55447 plus SQLite; signed behavior remains unchanged.
- [x] Fresh actual Android same-engine and PostgreSQL/SQLite retests.

**Confirmed second first failure — SQLite silent rounding:** actual Android
MySQL→SQLite Schema + data in
`artifacts/db-copy-paste-unsigned-schema-transfer-20261002/late02us1-6r51dlpw/`
reports2copied/0failed with portability warnings, but creates amount INTEGER
and stores both unsigned64 boundaries as REAL scientific notation rather than
the exact decimal integers. Source unchanged. This is a data-integrity failure,
not a passed transfer merely because the job completed.

PostgreSQL first UI copy also reproduced signed-BIGINT range failure,0/2,
unchanged source: `late02us1-k3zmomdn/` under the same evidence parent.
Fix preserves MySQL/MariaDB unsigned widths, widens PostgreSQL unsigned64 to
NUMERIC(20,0), and uses exact quoted decimal TEXT in SQLite. Smaller unsigned
widths widen only as needed. Only QA8928 restarted after successful
`cargo build -p iowb-cli --bin iowb`; production8100 remains untouched.
Fixed live backend PID167761, SHA256
`245ff24080f3d5c9dbbd6540cc0d6af270ccd4c7fac94b9cbe4186f3df8897ce`.

Fixed actual Android retests (fresh fixtures; no first-failure replay):

- [x] MySQL→MySQL Schema + data: `fixed03us1-vhyg36_8/`, unsigned schema
  retained,2/0, both exact boundary values, source unchanged.
- [x] MySQL→SQLite Schema + data: `fixed03us1-3vkuy0zi/`, amount TEXT,
  native typeof text and exact decimal values,2/0, source unchanged. Existing
  SQLite managed-id omission is explicitly reported by the job.
- [x] MySQL→PostgreSQL Schema + data: `fixed03us1-kh5zb4sc/`, exact values,
  2/0, source unchanged; native precision20/scale0/NOTNULL confirmed separately.
- [x] MySQL/PostgreSQL/SQLite schema-only adjacent controls: respectively
  `fixed03us1-n8z422u4/`, `fixed03us1-fergogzq/`, `fixed03us1-_74m46cm/`.
  All create the intended unsigned/numeric/text types, leave targets empty,
  preserve source rows/schema, and report0copied/0failed. PostgreSQL native
  precision20/scale0/NOTNULL independently verified.

Supporting final suite: `cargo test -p iowb-server database:: -- --test-threads=1`
with live PG55447 and MySQL53308 flags enabled: **101 passed,0failed**.
Runner offline tests: **15passed**. Root/apps diff whitespace checks pass.

Closure reconciliation after interruption: all six fixed transfer evidence
records independently rechecked against terminal UI, matching raw job, exact
native rows and schema. All six jobs succeeded (UI retains disclosed portability
warnings); no submissions replayed. Fixed QA8928 is healthy and still runs the
pinned binary above. This is a server-only change; the tested v64 APK is unchanged.

Coverage remains qualified: all163 original IDs are accounted for, not every
possible engine/type/fault combination. Exact1GB request-body boundaries,
absent clipboard-service fault injection, Meta dispatch, audible TalkBack and
physical ARM hardware remain explicitly limited/unverified. Optional deeper
variants such as PostGIS/SRID and two simultaneous Android
clients are not implied by the completed tests. Failed test destinations were
retained as evidence, not silently repaired or reused.

### Fresh continuation — 2026-10-02, transaction-failure variants

This is a new run after the completed v63 delivery, not a replay of historical
unchecked checkpoints. Root owns emulator5584; only QA8928 and disposable
PostgreSQL55447 fixtures are in scope. Production services remain untouched.

- [x] Reconcile all163 original IDs with the coverage ledger; inspect current
  runtime and installed v63 version. QA8928 remained healthy; no restart needed.
- [x] Independently audit remaining type/permission/constraint variants.
- [x] DATA-16 / PASTE-09/11: first actual Android three-row import with a
  BEFORE INSERT trigger rejecting the middle row; independently inspect both
  committed destination rows and transactional trigger audit records.
- [x] DATA-14/15 / PASTE-09/11: first actual Android import with a deferred
  foreign key to a separate parent; verify per-row commit failures and counts.
- [x] DATA-05 / PASTE-09/11: actual MySQL unsigned64 boundary import stores
  9223372036854775808 and18446744073709551615 exactly; overflow18446744073709551616
  fails. UI2/1, native exact decimal strings, unchanged schema.
- [x] Preserve each result, fix demonstrated product defects if any, then
  retest the same route and adjacent controls. No automatic resubmission.

First evidence: trigger `oct02next-dx5bcz9f/trigger` and deferred
`oct02next-48fbz2i5/deferred` under
`artifacts/db-copy-paste-adversarial-pg-v63-20261002/`; unsigned
`artifacts/db-copy-paste-unsigned-bigint-20261002/oct02u1-t2lh4ys1/`.
Both PostgreSQL imports report2/1; destination and trigger audit contain only
IDs1/3, with rejected ID2 absent and schema unchanged.

**New display defect:** failed second input displays `Row: #1`, while the grid
uses one-based numbering. Backend attribution is correct; presentation is
misleading. Shared display-only conversion now says `Input row 2`, retaining
zero-based API indices. Nine new formatter regressions pass.

- [x] v64 exact-APK trigger retest: `Input row 2`,2/1, exact IDs1/3 in target
  and audit, no ID2, unchanged schema.
- [x] v64 exact-APK deferred-FK retest: `Input row 2`,2/1, failed transaction's
  audit side effects absent, parent unchanged, complete neighbor rows retained.
- [x] v64 adjacent unsigned64 overflow retest: verify `Input row 3` plus exact
  upper-bound values, accurate2/1 and unchanged schema.

Fixed PostgreSQL evidence:
`artifacts/db-copy-paste-adversarial-pg-v63-20261002/oct02fix-o70ji13b/`.
The parent directory retains its original preparation name; embedded APK hashes
prove these retests run v64, not v63. Root visually reviewed the corrected dialog.
Unsigned retest evidence:
`artifacts/db-copy-paste-unsigned-bigint-20261002/oct02u2-vsl4x2sq/`.
Root visually reviewed the full third-input error; the native MySQL error's own
`at row 1` describes its single INSERT statement, while the app now unambiguously
labels the submitted batch position `Input row 3`.

- [x] Full current shared tests:618 across60suites,0failures/errors/skips,
  including9new display regressions. `mobileCheck`, assembly and both diff
  checks pass. No new full Rust run claimed for this display-only correction.
- [x] Exact v64 ARM64 APK installed and tested under emulator translation.
  SHA256 `8cb001e46339849c3e97f0d7dab3685a8d4826500b256d35243923fd90e7d3c4`.
- [x] Uploaded tested artifact once: https://gofile.io/d/z38OBqaA.
  Service size10,189,646 and MD5`0b621bf75306cc20eb00f611e5e793ce`
  match. Public receipt retained in thev64artifact directory; no downloaded
  SHA256 roundtrip or expiry guarantee claimed.

This newly built artifact includes the current preserved workspace, including
pre-existing post-v63 changes. Database cases above are the manual scope;
unrelated Board workflows are not claimed as newly manually tested.

The first two trigger navigation attempts stopped before any import, with
both native tables empty. The helper's Back dismissal left Database; checking
`mInputShown` did not prevent it. Manual Escape calibration preserved the exact
searched connection card; the helper now uses Escape. Root resumed only after
verifying the same fixture empty, then imported exactly once. This is a harness
correction, not evidence of a fixed application navigation defect.

Column-level UPDATE, MyISAM guarded edits and unsigned schema-transfer fidelity
remain additional queued variants, not newly claimed manual passes here.

- [x] Read every case and inspect current dirty worktree.
- [x] Build isolated fixtures and baseline artifacts (PostgreSQL, SQLite, MariaDB, genuine MySQL; baseline 8927, candidate 8928).
- [x] Account for every case and named variant; execute the safe, reachable
  matrix and explicitly retain resource/platform blockers and N/A variants.
- [x] Reproduce and fix each demonstrated product defect; distinguish observer
  interruptions from application failures and retain first-failure evidence.
- [x] Retest every fixed case and adjacent regression paths; no demonstrated
  product failure remains open in the executed matrix. This is not zero-bug proof.
- [x] Run final shared/server/mobile checks:515shared,71server database,
  separate7MariaDB, mobileCheck/assembly and both diff checks; see v62 logs/XML.
- [x] Rebuild/pin both ABIs; verify installed x86 checksum on both emulators,
  testv62, upload ARM64 and match Gofile size/MD5. No ARM64 execution claimed.

## Continuation — 2026-10-02, completed with explicit limits

The preceding process checklist records the completed first pass, not a claim
that newly identified interleavings have passed. This continuation preserves
all previous evidence and adds adversarial variants within the same163cases.

- [x] Reinspect source, original checklist, evidence, running services and limits.
- [x] Recover retained v62 standard emulator5584, pin installed checksum and
  directQA8928. Initial5582 boot hit its bounded memory cap; standard5584 then
  showed Android SystemUI ANR. These are preserved environment/observer events,
  not Workbench defects. No databases/tests had been submitted. Dismissed Wait,
  raised only the owned emulator cap to4GiB, verified normal navigation, and
  use one emulator at a time. Boot evidence: dbqa-continue-boot-20261002-ci4wpenc.
- [x] CELL11: actual Save while a native peer holds an uncommitted UPDATE;
  observe server lock wait, commit peer, verify peer survives and UI conflicts.
- [x] Adjacent guarded Delete/server regressions for the same race.
- [x] TRANSFER20/DATA:16typed source→target pairs beyond basicINTEGER/TEXT,
  with exact native values and explicit unsupported conversion limits.
  A separate misleading-default-warning defect found during this matrix is
  still tracked through its own fix/retest cycle below.
- [x] ROW07/RANGE07: selected Data-query result reordered by cell-save refresh.
- [x] FORMAT09/19: plain/escaped duplicate JSON member names and nested controls.
- [x] CELL04/10: revoke UPDATE only on a fresh owned role after editor opens.
- [x] CLIP06/PASTE04 source snapshot: external clipboardA→confirmation→clipboardB→Import.
  Actualv62 confirmation retained A, and one Import stored only A; UI1/0 and
  native ID/title matched. Evidence `clipboard-swap-otcpttup` under
  `artifacts/db-copy-paste-snapshot-permission-20261002/` (exact path recorded
  by the probe). No fix needed; both clipboards cleared afterward.
  This is not new destination-switch or Cancel evidence; the raw runner's
  PASTE05 identifier and earlier PASTE06 cross-reference were misclassifications.
- [x] ROBUST05: actual filesystem ENOSPC in a private16MiB tmpfs namespace,
  then recovery after releasing its own filler. Not host filesystem exhaustion
  or physical-device failure; actual UI/native cycle F below records the pass.
- [x] Record each first failure, focused fix, exact retest and adjacent checks;
  rebuild/re-upload if client code changes. No production8100/8787 restart.

The request-body cap, absent clipboard service, TalkBack, Meta dispatch and
ARM64 execution limits were rechecked, not silently marked complete. Actual
TalkBack actions and ARM64-under-translation now pass; remaining limits are
listed in the current v63 verification section below.
Plans: artifacts/dbqa-new-risk-plan-20261002.md and
artifacts/dbqa-platform-tail-feasibility-20261002.md.

### Continuation cycle A — pending-write concurrency

- [x] **First API/DB test: FAIL.** Fresh PostgreSQL Save and Delete both waited
  on the native peer's uncommitted update (actual transaction lock observed).
  After peer COMMIT, Save returned 200 and overwrote the newer value; Delete
  returned 200 and removed the changed row. Preserved evidence:
  `artifacts/db-copy-paste-concurrency-20261002/api-first1140-rlg8a_l4/evidence.jsonl`.
- [x] Implement a single-transaction locked snapshot check plus mutation and
  readback for PostgreSQL/InnoDB; SQLite uses an immediate transaction.
  Nontransactional MySQL tables reject guarded mutations before writing.
- [x] Compile and run exact concurrency regressions on PostgreSQL, genuine
  MySQL and MariaDB; verify SQLite normal/stale controls.
- [x] **First UI/DB test: FAIL.** Actual cell editor was prefilled; native peer
  held UPDATE; Android Save produced a proven server transaction lock wait.
  After peer COMMIT, Android toasted `title updated` and displayed
  `app-must-not-win`; native SQL confirmed the peer value was lost. Evidence:
  `artifacts/db-copy-paste-concurrency-20261002/first1235-22cmr5k8/`.
- [x] Matching v63 ARM64 Save retest on fixed server: actual PostgreSQL
  transactionid lock wait observed, peer COMMIT, Android full error409/stale-row
  reason and native `peer-committed` preserved. Evidence:
  `artifacts/db-copy-paste-concurrency-20261002/v63save-anbkqx85/`.
- [x] Matching v63 ARM64 bulk Delete retest: actual transactionid lock wait,
  peer COMMIT, Android `Deleted 0 row(s); 1 failed` with the stale-row reason,
  and native peer row preserved. Full result screenshot reviewed. Evidence:
  `artifacts/db-copy-paste-concurrency-20261002/v63delete-ub_qoluh/`.
- [x] **Adjacent actual bulk Delete UI/DB: FAIL.** `del1300-wk2696p0` captured
  the real Delete-one-row confirmation, the server lock wait, and peer COMMIT.
  Android reported one deleted row; native SQL confirmed the changed row was
  gone. Source review found Android bulk deletion omitted original snapshots,
  so fixing the atomic single-row path alone would not close this UI defect.
- [x] Send aligned original snapshots for native multi-row Delete, validate
  their shape server-side, and preserve accurate per-row partial results.
- [x] Preserve two preparation-only observer stops, `first1122` and `first1127`.
  Both tables still held `(1, before)`; neither submitted Save. The latter was
  a scope-dialog dismissal/navigation timing issue; the runner now waits for
  the actual footer. These are not product mutation failures.

### Continuation cycle B — typed transfer preparation

- [x] Prepare four owned sources, eight rows and ten fields per engine; exact
  native source values verified. No transfer job was submitted by preparation.
- [x] **Read-only API/DB mismatch found:** MySQL and MariaDB binary-collated
  text is returned as binary markers; actual BLOB bytes that look like JSON
  are parsed as JSON. Preserved source responses in
  `artifacts/db-copy-paste-typed-transfer-20261002/source-{6,3}-api-first.json`.
- [x] **First UI/DB test: FAIL.** Android MySQL→MySQL transfer submitted once;
  job `dbjob_cacbaa7ad0aa47f9bd0bc04224c3664a` reported six copied/two failed.
  Native row3 BLOB silently changed from bytes `74727565` (`true`) to `31`
  (`1`); object-looking bytes caused the two reported failures. Source remains
  unchanged; partial target preserved. Evidence: `typed-matrix.jsonl` and
  `1016-6-6-table-copy-terminal.png` in the typed-transfer directory above.
- [x] Fix confirmed decoding path; retest exact native values and engine pairs.
- [x] Root cause traced to pinned SQLx0.8.0 dropping MySQL wire collation from
  its text/binary classification. Vendored only `sqlx-mysql`0.8.0 with a narrow
  TypeInfo flag correction, retained full licenses/provenance, and left its
  version unchanged. No major driver upgrade. Actual binary bytes are never
  parsed as JSON by content; generic queries, table reads and atomic snapshots
  now share the corrected driver classification.
- [x] **Adjacent API/DB Boolean conversion: FAIL.** SQLite integer0/1/NULL/2
  into PostgreSQL BOOLEAN copied only SQL NULL; valid0/1 were rejected with
  SQL42804 (invalid2 was correctly rejected). Evidence `boolean-api-first.json`
  in the same directory. This is API/native evidence, not an Android pass.
- [x] Normalize exact integer0/1 for Boolean transfer destinations only;
  retain invalid2 rejection. Automated live regression passed; Android pairs
  still require the rebuilt candidate.
- [x] **Adjacent API/DB JSON→TEXT: FAIL.** A transfer reported eight copied,
  zero failed, while JSON Boolean `true` became SQLite TEXT `1` (a JSON number).
  Preserved `json-text-api-first.json`. Added source-aware serialization for
  JSON→text transfers, preserving SQL NULL and already encoded JSON strings.
- [x] **Screenshot review: FAIL, misleading job badge.** The same partial
  transfer was labelled `Succeeded` despite two failed rows. Added an
  outcome-label helper with regression cases and applied it to the job card/details: `Partial` for
  surviving partial work, `Failed` for no successful work, and `Warnings` for
  warning-only completion. Raw server job lifecycle remains unchanged.
- [x] Six label regressions pass; actual v63 retained partial job card and
  full details both show **Partial / copied6 / failed2**, verified by immutable
  job ID and unchanged native partial target. Details screenshot reviewed.
- [x] Fresh MySQL→MySQL v63 actual transfer passes8/0 and all80native field
  comparisons, including BLOB `true` bytes74727565 (previously corrupted to31).
  New card/full details show **Succeeded** with exact immutable job ID;
  source rows and both schemas unchanged. Original failure target preserved.
- [x] Complete all16typed pairs and invalid-Boolean boundary on exact v63.
  All128rows/1280native field comparisons pass, with source values and both
  schemas unchanged. SQLite0/1/NULL→PGBOOLEAN copies3; invalid2 rejects once
  with one failure and truthful **Partial** card/full details. Exact job
  `dbjob_12c1662fb5da42dcbd497acd383b4c47`; root reviewed the full result.
  Evidence: `artifacts/db-copy-paste-typed-transfer-v63-20261002/`.
  Two initial read-only observer stops (overscrolling and stale coordinates
  after modal focus restoration) occurred before any transfer. Both preserved;
  smaller scrolls and fresh exact-card/immutable-ID checks corrected the harness.
- [x] **Additional UI/DB warning defect found:** existing-table MariaDB→PG
  job `dbjob_0ed0492342964b5c8e8af6894b8d824f` copied8/failed0 and preserved
  all80fields plus both schemas, but showed **Warnings** for nine source
  defaults allegedly skipped. Existing-table mode does not copy schema or
  change target defaults. The data-integrity pass stands; the warning UX fails.
  A same-job read-only observation first corrected the runner's expected-label
  mismatch; subsequent source audit established this separate product defect.
- [x] Gate schema-loss default warnings to schema-creation modes; preserve
  actual data/mapping warnings and all historical job results. Add regression
  controls for existing target defaults and both schema-creation modes, then
  run a fresh actual Android transfer after a safe QA-only backend restart.
- [x] Fresh actual Android existing-table warning retest passes on warning-only
  backendfa586535: job`dbjob_927ee14f4b6545169a488b88d107186b`, copied8/failed0,
  no warnings, all80native fields exact, source/target schemas unchanged and
  original warning job unchanged. Root reviewed terminal screenshot. Positive
  schema-warning/constraint controls remain in progress on separate targets.
  Evidence: `artifacts/db-copy-paste-warning-controls-v63-20261002/first-oct02c/`.
  Two initial source-navigation stops and one target-card stop preceded any
  submission; native empty target/zero matching jobs were verified each time.
  One subsequent manual Start transfer was followed by observe-only validation.
  The first two stops reused one screenshot filename, losing the separate first
  visual capture; both original logs/JSONL records remain. Later captures use
  unique directories. These stops are not database-operation failures.
- [x] Test-first regression confirms the warning defect: existing-table copy
  preserves exact target DDL, explicit defaults, existing row and copied value,
  but fails the no-schema-loss-warning assertion. Both schema-creation controls
  pass and independently verify actual default loss is still warned. Red log:
  `artifacts/dbqa-default-warning-red-20261002.log` (1failed/2passed).
- [x] Narrow mode gate fixes the three focused regressions (3/3), followed
  by94/94 full database tests with actual PostgreSQL/genuine MySQL enabled.
  Logs: `artifacts/dbqa-default-warning-green-20261002.log` and
  `artifacts/dbqa-default-warning-full-20261002.log`. At15:12WIB, after explicit
  device release and proving no active jobs, only QA8928 restarted once to the
  warning-fixed CLI (PID2097035, SHAfa586535…). The nullability fix is NOT in
  this binary. Fresh Android warning/first-constraint controls now follow.

### Remaining continuation execution order

Keep one emulator owner at a time: duplicate-member inputs → automatic
selection-refresh probes → clipboard confirmation snapshot → revoked UPDATE
→ bounded private filesystem-full/recovery. Server/client rebuilds are
coordinated after first failures; no mutation is replayed merely because its
observer was interrupted. Exact fixed-case retests use fresh uniquely named
fixtures and preserve all original failing targets.

### Continuation cycle C — ambiguous structured input

- [x] **First UI tests: FAIL, plain and escaped duplicate member names.**
  Both offered normal Import-one-row confirmation after collapsing duplicate
  `title` / `\u0074itle` members, without an ambiguity warning. Each confirmation
  was cancelled; native targets remain empty. Evidence:
  `artifacts/db-copy-paste-duplicate-json-20261002/dup1341-dcdv1yjn/`.
- [x] Nested duplicate-member control also offered normal import (FAIL);
  cancelled, native target empty. All three first-test records are preserved.
- [x] Add raw-name validation alongside structural JSON parsing, retaining
  separate-object keys as valid; seven focused regression tests added.
- [x] Exact v63 ARM64 Android rejection retests: plain/escaped/nested duplicate
  keys each show the duplicate-name reason, no confirmation and zero native rows.
- [x] Valid controls: repeated names in separate rows, separate nested objects,
  and distinct case/Unicode-normalization forms all import with exact native
  values and truthful2/0,1/0,1/0 counts. One Import per control; clipboards cleared.
  Evidence: `artifacts/db-copy-paste-duplicate-json-20261002/v63dup2-la9m6208/`.

### Build/check record — continuation

- [x] First bounded one-job Rust compile found four errors in the new atomic
  helper: SQLx0.8 RawSql's `fetch_optional` returns a row, not an Option.
  Switched bounded raw reads to fetch-all/first, retaining raw typed decoding
  and actual missing-row handling. First log: `artifacts/dbqa-rust-database-20261002.log`.
- [x] Rerun compile and database regressions after the typed-transfer fixes;
  final91/14 backend checks and555shared/mobile checks are recorded below.
  Exact UI matrix completion remains separately tracked in cycle B.
- [x] Consolidated full server database suite: **87/87 passed**, with live
  PostgreSQL55447 and genuine MySQL53308 explicitly enabled. This includes
  concurrency409/native-preservation, bulk-delete indexing/validation,
  missing-row404, binary/text fidelity and both conversion regressions.
  Log: `artifacts/dbqa-rust-database-consolidated-20261002.log`.
- [x] MariaDB subset, patched-driver unit tests and fresh CLI/client builds.
  QA8928 safely switched to the fixed binary at13:04; production8100/8787
  remain untouched. Candidate Android retests have their own checklists.
- [x] First MariaDB subset:12passed/2race tests stopped at missing observed
  lock-wait, before their result oracle. An independent native held-peer/locked
  read calibrated the same observer successfully. Preserve first log
  `dbqa-rust-mariadb-20261002.log`; investigate the bounded test before claiming
  either a pass or a product defect. No observer requirement is weakened.
- [x] MariaDB diagnostic showed correct409/stale-row message and preserved
  native peer value. Observer cause: MariaDB's InnoDB monitor requires100ms
  idle between reads; polling every25ms kept its initial empty cache alive.
  Changed only observer cadence to200ms, keeping the exact lock-wait predicate.
  Rerun pending. Upstream reference: MariaDB10.11 `storage/innobase/trx/trx0i_s.cc`,
  `CACHE_MIN_IDLE_TIME_NS` and per-read `last_read` updates.
- [x] Corrected MariaDB observer retest: **14/14 passed** with the actual
  engine lock-wait requirement retained. The earlier two stops remain recorded
  as harness failures, not additional product defects.
- [x] Patched MySQL driver unit checks: **2/2 passed**;
  `artifacts/dbqa-rust-driver-20261002.log`.
- [x] First v63 mobile compile: **FAIL**, two test-only type-inference errors
  in `DatabasePanelSelectionTest`. Replaced nullable concrete-set assertions
  with explicit required-value set comparisons; expectations unchanged.
  Preserved `artifacts/db-copy-paste-v63-20261002/mobile-build.log`.
- [x] Rerun shared tests/mobileCheck/assembly after that correction: **PASS**,
  56 shared suites / **555 tests**, zero failures/errors/skips. Final build
  succeeded in14m52s under the existing resource cap. Log
  `artifacts/db-copy-paste-v63-20261002/mobile-build-retest.log`; XML beside it.
- [x] Native permission audit found a new atomic-Delete regression:
  PostgreSQL SELECT+DELETE-only roles cannot use SELECT FOR UPDATE. A focused
  test now checks valid Delete and rejects stale snapshots without firing a
  nontransactional sequence-incrementing trigger. Fix/test cycle pending;
  previous87-test pass is not final sign-off for this newly discovered case.
- [x] DELETE-only first regression test failed with PostgreSQL42501. The fix
  uses a transaction-scoped EXCLUSIVE relation lock only for DELETE roles
  without any UPDATE grant; ordinary writers retain row locks. Retest passes
  valid Delete, observed held-peer wait, stale409, preserved peer data and
  no stale-trigger sequence side effect. The fallback serializes table writers.
- [x] Final live backend suite: **91/91 passed** (PostgreSQL55447 and genuine
  MySQL53308 enabled), then **14/14 passed** against MariaDB53307. Logs:
  `artifacts/dbqa-rust-database-final-resume-20261002.log` and
  `artifacts/dbqa-rust-mariadb-final-resume-20261002.log`.
- [x] CLI build passes; binary SHA256
  `7261ac379415f42e2ba77385f400a71c93d516201b7c7c2de002d23e65beef6a`.
  QA8928 safely restarted at13:04 before fixed-backend Android tests (below).

### Continuation cycle D — automatic-refresh selection

- [x] **First UI/DB test: FAIL.** Selected displayed ID1, edited ID2's title
  so the query reordered to2,1,3, and observed a new automatic-execution time.
  Row-position1 stayed checked; actual Copy returned ID2, not the originally
  selected ID1. Native order and copied payload agree. Evidence:
  `artifacts/db-copy-paste-panel-selection-20261002/first1155-rows-5whkf9nj/`.
- [x] Clear the refreshed panel's remembered row indexes on accepted result
  replacement or failure; clear the visible selection only when that same
  panel is active. Explicit Run uses the same rule. Added four source-isolation
  regression tests; other panels retain their selection.
- [x] **Adjacent range first UI/DB test: PASS onv62.** A three-cell range was
  cleared when Save automatically refreshed/reordered the query; no stale
  range toolbar remained. Native order2,1,3 and new execution time verified.
  Evidence: `first1201-range-80_fbkdi` in the same directory. No range fix needed.
- [x] Four shared selection tests and exact v63 ARM64 Android retest pass.
  After editing ID2, native/display order is2,1,3; all row checkboxes clear,
  Copy hides and Ctrl+C preserves both clipboard snapshots. Evidence:
  `artifacts/db-copy-paste-panel-selection-20261002/v63rows-rows-gkp_n8e8/`.
  A gateway interruption left the original editor open before Save (native
  rows unchanged, no submission record). Resumed that same editor once. A
  pre-Save harness assertion initially compared text labels to integers; fixed
  only the oracle and preserved its log. No repeated Save or product failure
  was inferred from either interruption.

### Continuation cycle E — permission changes after opening an editor

- [x] **First UI/DB test: FAIL, incorrect reason.** A fresh role opened a
  permitted editor, then lost UPDATE before one actual Save. Native data stayed
  unchanged, but HTTP400/UI falsely said the table had no primary key. Native
  catalog verifies it does. PostgreSQL information-schema visibility hides its
  key from the now-SELECT-only role. Evidence:
  `artifacts/db-copy-paste-snapshot-permission-20261002/permission-revoke-1lg_7w5n/`.
- [x] Supersede the probe's premature PASS (it checked only unchanged data
  before checking the reason): overall first case is FAIL. Preserve that
  original record and add explicit failure/observer correction, not erase it.
- [x] Distinguish permission loss from genuinely missing primary keys;
  preserve SELECT-only UI safety and column-level UPDATE grants.
- [x] Three live permission regressions pass. Actual v63 ARM64 editor→revoke
  UPDATE→one Save now reports HTTP403 with the precise UPDATE-privilege reason;
  native original row remains unchanged. Full dialog reviewed and closed.
  Evidence: `artifacts/db-copy-paste-snapshot-permission-20261002/permission-revoke-na3ryz6i/`.

### Continuation cycle F — real bounded filesystem-full and recovery

- [x] Actual v62 Android Import on private16MiB tmpfs reached ENOSPC:
  **5 inserted / 35 failed**, readable disk-full reason, native IDs1–5 and
  all4096-byte value hashes exact; integrity check OK. No host/user disk filled.
- [x] Released only the owned filler, then one recovery Import reported1/0.
  Native generated ID6/value hash match and the prior five rows are unchanged.
  Initial recovery observer incorrectly expected supplied ID1001 despite the
  displayed ignored-identity warning; preserved that harness error and verified
  the same completed operation without a second Import.
- [x] Clear both clipboards, return idle DB/8928, preserve six-row SQLite
  backup and stop only auxiliary8929. No listener remains there.
- [x] Detailed evidence and setup-only stops:
  `artifacts/dbqa-private-enospc-results-20261002.md` and
  `artifacts/db-copy-paste-private-enospc-20261002/ui-ksp07ctn/`.

### Continuation candidate — v63

- [x] Pin built ARM64 SHA256
  `15bc96c3b7b9af81bb6c371a6a3a39fa4245f7b30c1f57ea3c90060ae318de02`
  and x86 SHA256
  `d2cedfcfdff408fa55c81e6f1590c2ee477ce55c43530fd966c78585c31aaae1`.
- [x] Static ABI-independent code/resource parity audit passes. Actual ARM64
  APK installation succeeds; installed checksum/version63/primaryCpuAbi verified.
  Fresh cold-start observer initially timed out; process stayed alive, no fatal
  crash observed, then actual navigation reached connected Database Explorer.
  Startup screenshot/XML are in the v63 directory. This is not a startup-speed pass.
- [x] Correct earlier device limitation: current5584 is x86 Android15 with
  `libndk_translation.so` and ARM64 compatibility. Current tests use the actual
  ARM64 package under translation, **not physical ARM64 hardware**. System
  TalkBack15 is installed. The later actual accessibility cycle below now
  supersedes the old unavailable-package assumption; audible speech remains
  unverified on the no-audio emulator.
- [x] Exact candidate UI regressions and final backend build/retest; delivery
  is recorded separately in the current v63 section below.
- [x] At13:04, after the JSON batch finished and cleared both clipboards,
  restart only existing QA8928 unit. NewPID1831192 and running executable SHA
  match the pinned candidate; health reports the exact existing QA config.
  Production8100/8787 remain untouched. Backend-dependent UI retests start here.
- [x] Interruption recovery: owned PostgreSQL55447 had shut down and was
  restored from its verified existing data directory. MySQL/MariaDB fixtures
  remained healthy; QA8928 stayed alive. Interrupted duplicate probe created
  only `qa_duplicate_json_v63dup_plain`, independently verified empty. Its
  evidence directory is retained; fresh `v63dup2` fixtures are used for retest.

### Adjacent schema constraint cycle — first supporting failure

- [x] Review of the positive schema-warning controls found that the builder
  omits explicit NOTNULL for a single primary key. SQLite TEXT PRIMARY KEY does
  not imply NOTNULL, unlike ordinary assumptions for other engines.
- [x] Two actual SQLite worker regressions fail: source rejects NULL text PK,
  but schema-only and schema+data targets accept it. PRAGMA and native insert
  probes agree; ordinary title NOTNULL remains enforced. Probe writes roll back.
  Log: `artifacts/dbqa-not-null-red-20261002.log` (0passed/2failed).
- [x] First actual Android schema-only copy FAILS on warning-onlyfa586535.
  One confirmed Create table submitted job`dbjob_857d9d1039154deaa5360d05676ade5e`.
  Source`qa_tr_warn_oct02c_source` rejects NULL primary keys; actual target
  `qa_tr_warn_oct02c_schema` becomes TEXT PRIMARY KEY without NOTNULL and
  accepts NULL. Both native probes rolled back; target remains empty and
  source remains unchanged. Exactly two real schema-portability/default-loss
  warnings still appear. Root reviewed terminal screenshot and native evidence:
  `artifacts/db-copy-paste-warning-controls-v63-20261002/first-oct02c/`.
- [x] Deploy narrow nullability fix, then fresh Android schema-only/schema+data
  retests including real NULL-enforcement probes. Preserve the failed target.
  Prior94-test pass predates these new tests and is not final sign-off.
- [x] Additional portable-parser/shared-builder regression fails before the
  change, then all3focused tests pass after preserving explicitfalse nullability
  on single PKs. True/unknown nullability is not strengthened. Logs:
  `artifacts/dbqa-not-null-portable-red-20261002.log` and
  `artifacts/dbqa-not-null-green-20261002.log`.
  Source fix is prepared in parallel, but CLI/runtime deliberately retain the
  old implementation for the first actual Android observation. No manual pass
  is inferred from these supporting tests.
- [x] Full source regressions after the nullability fix:97/97database tests
  with actual PostgreSQL/genuine MySQL enabled, followed by14/14MariaDB.
  Logs: `artifacts/dbqa-not-null-full-20261002.log` and
  `artifacts/dbqa-not-null-mariadb-20261002.log`.
- [x] Final CLI build passes (`artifacts/dbqa-not-null-final-cli-20261002.log`)
  with unchanged source/test hashes since97/14green. After explicit device
  release and zero pending jobs, root restarted only QA8928 once at15:39WIB.
  LivePID2148555/executableSHA
  `fbe2b6d441483d26076866c8ff8628b5a297018e30b03d6fa8084f27cb760400`
  and exact health config match. Fresh oct02d Android retests follow.
- [x] Final fixed-server actual Android controls all PASS on fresh oct02d
  targets, with one submission per case and no navigation stop in this run.
  Existing-table copy job`dbjob_3cf7cdc066f0421bb14f36071efc856f` copies8/0,
  preserves80exact fields/both schemas and has no false warnings. Schema-only
  job`dbjob_997a2a0ecaa34de5a27c3af93bf70b7d` creates0rows; schema+data
  job`dbjob_35ab6947c780477693778a04c73a5118` creates1exactrow. Both preserve
  explicit PK/ordinary-column NOTNULL, reject a real NULL-key insert, and retain
  exactly the two genuine portability/default-loss warnings. Constraint probes
  roll back and leave original rows unchanged. Old failed oct02c target stays
  empty with its original broken DDL; original warning job remains unchanged.
  Evidence: `artifacts/db-copy-paste-warning-controls-v63-20261002/retest-oct02d/`.
  Root reviewed terminal images/native results; device released idle on Jobs.

### TalkBack continuation — actual interaction completed

- [x] Pin exact ARM64v63 and capture original settings/notification permission
  flags. First Explorer-navigation observer stops and a transient loading
  observation are preserved; safe app restart recovered normal read-only access.
- [x] Real TalkBack service binds with touch exploration enabled. Ordinary ADB
  tap/swipe failed input calibration; emulator-console hardware touch produces
  genuine green focus and inputMode0. UIAutomator capture reset reader focus,
  so active gesture evidence uses screenshots/service state instead.
- [x] Actual Workbench row selection and Copy: reviewed focus, hardware double
  tap, exact clipboard/native row equality. Root independently reviewed focused
  Copy screenshot and selected-row state. No import yet at this checkpoint.
- [x] Actual Paste/Cancel: confirmation source/target/columns and insertion/
  partial-failure warning are reachable; Cancel writes nothing. Valid Import
  uses one reviewed double tap and reports1/0; native original row plus exact
  imported UTF8value match. Duplicate ID with changed text uses one Import,
  reports0/1 with readable constraint reason, and leaves both rows unchanged.
  Root reviewed actual focused Import and error-body screenshots.
- [x] Exact nine-setting/permission restoration and settled idle navigation
  smoke. Original notification grant/flags and ToastRecorder-only service state
  match; both clipboards are empty. Actual Explorer→Jobs→Clipboard→Explorer
  passes with stable PID and no crash/ANR candidate signals. The first early
  Database tap after relaunch stayed on Projects; one settled tap succeeded.
  Preserve both observations: this is not a first-attempt startup pass or a
  diagnosed/fixed navigation defect. SingleClick is the default and FocusFirst
  does not explain the observation. Device released idle at15:10WIB.
  Observer records' `appMutationSubmitted:false` means the readback
  helper itself did not submit a mutation, not that the preceding UI Import
  did nothing; actual hardware activation/result/native evidence is linked.
  Evidence: `artifacts/db-copy-paste-talkback-20261002/actual-v63-arm64/`.
  Audible speech remains unverified under emulator `-no-audio`.

### Final bounded navigation check — 2026-10-02

- [x] Preserve first helper attempt `ready-ppi3pm9x` as INCONCLUSIVE: Android
  did not create the first readiness hierarchy; the helper stopped before any
  tap. Launch reported timeout, but the app remained resumed/alive with no
  crash-buffer entries. This is not a failed database operation or a pass.
- [x] Fix only the observer: retain dump output and reobserve missing/null
  hierarchy within the original45s/30s phase deadlines. No automatic restart
  or input retry;11offline tests verify the boundaries and exactly one tap.
- [x] Separate bounded retest `ready-q69qf_e_` passes on the same pinned APK:
  cold launch reportsOK; two consecutive visible-ready Projects snapshots have
  identical enabled Database bounds. One timestamped tap at(216,1552) opens
  Database in the first passive observation (~4.95s, including capture overhead).
  No second tap, no database write, and original accessibility settings unchanged.
  No crash/ANR candidate signals in the captured log. This proves ready-state
  navigation, not the cause of the earlier untimestamped missed tap.
- [x] Root visual review distinguishes navigation from data readiness: the
  first Database capture still says Working. Later passive capture (no input)
  shows the correct saved final schema jobs and QA PostgreSQL context loaded.
  No exact data-load speed or first-attempt cold-start guarantee is claimed.

Evidence: `artifacts/db-copy-paste-ready-navigation-20261002/`; each run has
append-only events, launch/log output and unique screenshots/XML. Final device
is idle on Database→Jobs. Only the isolated QA server was restarted during this
continuation; no user/production server or database was restarted or mutated.

## Current v63 verification — 2026-10-02

- [x] Exact ARM64 package is installed and exercised on Android15 emulator5584
  with `libndk_translation.so`; this is not physical ARM64 hardware testing.
  SHA256: `15bc96c3b7b9af81bb6c371a6a3a39fa4245f7b30c1f57ea3c90060ae318de02`.
- [x] Shared56suites/555tests: zero failures, errors or skips. `mobileCheck`
  and Android debug assembly pass. An initial test-assertion compilation error
  is retained separately, corrected, and followed by the successful full run.
  Exact logs/XML and266shared-entry ABI parity are in
  `artifacts/db-copy-paste-v63-20261002/`; the successful build log is
  `mobile-build-retest.log`, not the earlier failing `mobile-build.log`.
  Delivery is this pinned12:40DB-QA build. Later unrelated Board/controller
  source edits exist in the shared dirty worktree and remain untouched; these
  APK/test claims do not certify or include those later edits.
- [x] Final source database regressions:97/97 with actual PostgreSQL/genuine
  MySQL enabled; separate MariaDB14/14. These support but do not substitute
  for the actual Android/native-data results in the continuation cycles.
- [x] Exact v63 retests cover Save/Delete pending-write races, automatic
  query-refresh selection, duplicate JSON rejection/valid controls, revoked
  UPDATE,16typed transfer pairs, Boolean boundary, accurate job status and
  real TalkBack actions. Earlier cases retain their actual tested build;
  this is not a claim that all163case variants were rerun onv63.
- [x] Final backend CLI deployment and all three fresh warning/NOTNULL Android
  controls pass; final ready-state cold navigation check passes with the
  qualifications above. Both diff checks pass;163originalIDs each have exactly
  one current ledger row, with no omissions or duplicates.
- [x] Verified Gofile delivery of this exact tested APK; one upload only.
  Interruption recovery revalidated the saved private response locally without
  another upload. Gofile's size10,189,654bytes and MD5
  `f5445a670f02a0bf145ebf79d45cb9b8` match the pinned APK.

### Current delivery — ARM64 v63

[Download the tested ARM64 APK](https://gofile.io/d/DXlTaPop).
Public verification record:
`artifacts/db-copy-paste-v63-20261002/gofile-verification.json`.
Local SHA256 is pinned above. Verification is Gofile-reported size/MD5, not a
downloaded SHA256 roundtrip; no expiry guarantee is asserted. Only the APK was
uploaded. Private service responses/account tokens remain outside the repository.
Server fixes are verified on isolated QA8928, not deployed to production8100/8787.

Current limits: full~1GB request-body boundary is resource-blocked; disabled
limit setting is N/A; absent clipboard-service simulation remains unavailable;
Meta input did not pass platform calibration (Ctrl did). TalkBack focus/actions
pass, but audible speech is unverified. The bounded private16MiB filesystem-full
cycle passed onv62, not rerun onv63. An early post-launch navigation tap remains
an undiagnosed observation; settled navigation passes. No zero-bug guarantee
or physical-device/performance guarantee is implied. Production8100/8787 and
user databases remain untouched; backend fixes are deployed only to isolatedQA.

## Final verification — v62, 2026-10-01–02 (historical)

- All163 source case IDs have exactly one ledger row; no missing or duplicate
  IDs. Counts below describe bounded evidence, not163unqualified passes.
- Typed rows:84actual Android batches/323variants, plus86cell-replacement keys
  and adjacent regression controls. Independent read-only native recheck84/84.
- Transfers:48basic engine-pair/mode cases plus72additional scenarios.
  Definition scopes/precedence/empty faults:14/14. No duplicate replay of an
  already submitted operation to recover an interrupted observer.
- Final remaining queue closed:3generated-column engine imports,1SELECT-only
  cell safety control,4selection/copy controls and4job-header layout observations.
  Earlier evidence retains its actual build version; this is not a claim that
  the whole historical matrix was rerun onv62.
- v62 build/mobile checks pass; shared50suites/515tests have zero failures,
  errors or skips. Final server71database tests with live PostgreSQL/genuine
  MySQL enabled pass; separate7MariaDB tests pass. Logs and shared XML are in
  `artifacts/db-copy-paste-v62-20261001/`.
- Latest package includes one8dp spacing correction for long job headers.
  Exact normal/narrow/enlarged-font emulator retest and three additional
  generated-column engines pass onv62. SELECT-only-role guard and all four
  final selection controls pass. Upload verification is recorded below.
- ARM64 SHA256:
  `f3c58595578529acd7b02ec5fbdc65a4bc9bd55f0ab328ea77580e5f8c8c20b1`.
  x86 SHA256:
  `335c3de306b73ddf4599cf54327eec8a59d5540de3ab9eccd4c686df7bc76f14`.
  Static266-entry parity and signatures pass; this is not ARM64 execution.
- Production8100/8787 remain untouched. Backend fixes run only on isolated8928;
  the owned crash-test8929 is stopped with config and partial data preserved.

### Delivery

[Download ARM64 debug APK v62](https://gofile.io/d/oun0IJ89).
Local size10,173,254bytes and MD5`c2725358e2f99dda619e9473c2e94ec9`
match Gofile's successful upload response. Local SHA256 is pinned above.
No downloaded SHA verification or expiry guarantee is claimed. Sanitized public
record: `artifacts/db-copy-paste-v62-20261001/gofile-verification.json`;
account tokens/private responses remain outside the repository.

### Explicit test limits

This subsection preserves the v62 checkpoint; the continuation supersedes its
TalkBack-installation, ARM64-compatibility and filesystem-full assumptions.
Actual ARM64-under-translation and bounded filesystem ENOSPC now have evidence
above; final accessibility coverage is tracked separately, not inferred here.

The full below/at/above1,048,576,000-byte bodies are resource-blocked;
disabled-limit mode is N/A because no setting exists. TalkBack and a safely
absent clipboard service were unavailable. Meta input failed calibration before
reaching Workbench; Ctrl was tested. Physical filesystem ENOSPC was not injected.
Both emulators are x86-only. Typed Android cases are primarily PostgreSQL;
the four-engine basic transfer matrix is not every native-type/fault cross-product.
See the per-case qualifications and
`artifacts/dbqa-independent-final-coverage-audit-20261001.md` for precise limits.
These exceptions are not passes and this report does not promise zero bugs.

## Per-case ledger

### Historical resumed UI checkpoint, 21:38

- [x] Recheck both live runners, pinned APK and isolated8928 health before acting; no restart or duplicate import.
- [x] Complete external JSON:30/30 groups,111/111 variants, each with actual confirmation/result XML+PNG and independent typed PostgreSQL comparison.
- [x] Preserve the original raw-decimal corruption and validate its fix once in a fresh target; do not overwrite first-failure evidence.
- [x] Complete86/86 cell-replacement keys and11 additional numeric/Boolean regression controls, with actual version identities retained.
- [x] Review actual floating-point and self-reference result screenshots: full destination/count/error text and dismiss controls remain readable.
- [x] Finish internal rows:27/27 groups,107/107 variants at21:49. Final eight
  groups retain source-copy, exact confirmation/report and independent native
  comparisons in `20261001-213852-rows-yr6gpvwf/iowb-data-row-ui-76z6jyca/`.
  Default/trigger/next-ID checks also pass. The earlier19/27 checkpoint is
  superseded by this linked completed run, not credited twice.
- [x] Complete CSV:27/27 groups,105/105 variants on v61. Original zero-progress
  checkpoint at21:49 is superseded by the complete evidence index below.
- [ ] Finish keyboard/modal/large-copy, clipboard lifecycle/range and transfer tails.

JSON evidence: initial five groups in
`artifacts/db-copy-paste-data-20261001/resumed-ui/20261001-203932-rows-uxz6j9fa/`,
fresh raw-decimal retest in `20261001-210455-rows-2ha_bcf0/`, and remaining24
groups in `20261001-210600-rows-8u8b0hm5/` under the same parent. The last
runner exited0; effects/defaults/triggers and next-ID checks passed. Its third
default insert is explicitly a native verification probe, not an Android insert.
The read-only `row_evidence_crosswalk.py` validates complete result reports,
failure indices, exact scopes and all variants before counting a pass.
Cell evidence is indexed in `artifacts/db-copy-paste-data-20261001/cell-evidence-crosswalk.md`.
This is representative PostgreSQL typed Android coverage; the four-engine API
matrix and48 basic engine/mode Android transfers remain distinct evidence.

Historical build checkpoint: v61 is pinned in
`artifacts/db-copy-paste-v61-20261001/`. All515 shared tests pass with zero
failures/errors/skips, plus mobileCheck and debug assembly. Both emulator
installed APK hashes were independently verified:
`6f2122f7a89a6973edc1d7dba069e2cfd28027750a774b21aad699965c65431c`.
ARM64 companion:
`3863baf81f31fb14c5d81195ade5a8f253704080eaa4787f886a5b12321b63d2`.
This build fixes empty single-column CSV/TSV export records. Actual rebuilt
range CSV/TSV and CSV-file export retests pass; Boolean/numeric/nested-JSON
range controls and three query-invalidation routes also pass. Remaining typed
CSV, transfer and definition queues are still pending.
No Gofile upload has occurred. Prior versions/evidence retain their identities.
Static v61 parity/signature audit confirms all266 shared ZIP entries are
byte-identical between x86 and ARM64, with only the expected native ABI
library differing. Report: `artifacts/dbqa-apk-parity-v61-20261001.md`.
This is not ARM64 execution evidence.

Historical build checkpoint: v60 is pinned in
`artifacts/db-copy-paste-v60-20261001/`. All513 shared tests pass with zero
failures/errors/skips, plus mobileCheck and debug assembly. Installed and
independently verified on both x86 emulators:
`a5e165e7db289e574ad4dd362c07bb92e1c08d9af6c92edcb513cc596486826b`.
ARM64 companion: `f52fc6a96042344efa806390d5e1d3ff5725708972a7c6428e55a74fbffae9be`.
The raw-decimal precision defect has passed its exact rebuilt Android retest;
remaining row/transfer/lifecycle variants and Gofile delivery are still pending.
The513-test follow-up adds a real filter-request wire assertion; production
code/APK is unchanged from the initial512-test v60 build. XML is retained in
both `jvmTest` and `jvmTest-followup` inside the pinned artifact directory.
ARM64 execution is not claimed on these x86-only emulators.

Historical build checkpoint, 20:39: v59 is pinned in
`artifacts/db-copy-paste-v59-20261001/`. All506 shared tests pass with zero
failures/errors/skips; `mobileCheck` and debug assembly pass. Installed5582
x86 SHA-256 `98df2534ede5137db42fd10937953fa96380b4daed63ebf5557de4c1f0c75c34`;
ARM64 companion `1c9c4498098e639267ff8c22e5199ccdf5710fb85c5ce5171eb538f1da45a294`.
ARM64 has not been executed or uploaded. POINT UI retests and remaining
row/transfer/lifecycle variants are still underway. Earlier checkpoints below
retain their original identities; they are not statements about the latest APK.

Historical automated checkpoint, 2026-10-01 17:37: candidate v50 builds successfully; all 486 shared JVM tests and 62 Rust database tests pass, with `mobileCheck` and both diff checks passing. This is supporting regression evidence, not completion of the manual matrix. Matching x86_64 emulator build SHA-256: `fef38548add236c07095d27e8b818b11c3d594c9d674a7bb4e770ac80851f901`; ARM64 companion: `5234923a30c70bda7afe7ed51c576e09868f49e073efed09070d94d2d18e7776`.

Latest installed UI artifact at the 18:54 checkpoint: v54 x86_64, SHA-256 `dc1684c605fe4f113b0a776224d4353a87e0ee31a0e427e286e88d9229353c96`, verified on both recovered emulators. Built ARM64 companion: `fa3be329acfe6933f95c564d5c4cb120b6d3f77448e17b24540c6a03798cc746`; not executed on these x86-only devices. v54 passes499 shared tests (zero failures/errors/skips), `mobileCheck` and assembly; XML archived in `artifacts/db-copy-paste-shared-tests-v54-20261001/`. Neither Gofile delivery nor all manual variants is complete.

New candidate v55 is assembled and pinned in `artifacts/db-copy-paste-v55-20261001/`:
x86_64 SHA `0f1a19aaa36fa6fa4832a623906605d58367ba7e0bf680238793feb9a9d45d3f`,
ARM64 SHA `506457980073ad1a1ad33f8ae028ec74ba2e5f19ddfc8b2dca88541501a5308a`.
Initial shared run:501tests, one Git-test recorder race failure (all database
suites passed); preserved XML in `artifacts/db-copy-paste-shared-tests-v55-first-20261001/`.
The race occurred in the test-only unsynchronized request recorder, before Git
sync began. Mutex-protected recording/snapshotting preserves every assertion;
focused retest and full501tests now pass (zero failures/errors/skips), followed
by `mobileCheck` and assembly. Passing XML is in
`artifacts/db-copy-paste-shared-tests-v55-pass-20261001/`; pinned APK checksums
remain unchanged. Both emulators independently verify the v55 x86 checksum.
New server64-test
database suite passes with live PostgreSQL explicitly enabled; other-engine
environment-gated live tests were not enabled by that command. No upload yet.

v56 includes the qualified-source explanation correction, with `mobileCheck`
and assembly green (unchanged shared tests reused the passing501-test result).
Pinned ARM64SHA `9ef4c89a983df2d0d5ab53a9dac432a34d383298bea42801e5bcfc774191eaee`;
x86SHA `aae7188a865ba4acf6bd74114fde471b2af24ebe6f8af8c1aea6fa88d87431dd` in
`artifacts/db-copy-paste-v56-20261001/`. Installed/verified on5582;5584 is
finishing its v55 cell batch before its coordinated upgrade. Actual v56
unqualified/qualified source-gate controls pass: the blocked query names the
correct quoted table and has no Paste action; the qualified query enables
Paste. No import was submitted in this explanation-only check. Evidence:
`artifacts/db-copy-paste-db-faults-recovery-20261001/v56-qualified-control-xhq6v868/`.
The remaining manual matrix and Gofile upload are not complete.

v58 adds metadata-aware value-preview labels only: binary transport wrappers,
POINT and SQL arrays are no longer mislabelled JSON. The full504 shared tests
pass with zero failures/errors/skips, plus `mobileCheck` and debug assembly.
XML: `artifacts/db-copy-paste-shared-tests-v58-20261001/`.
Pinned x86SHA `2865974aa00fe40f7e8df9c7e16d724e9e5270bdd8c05fad6eaa7f410ac11ff7`;
ARM64SHA `4f7fb971dc558f568c753392691ba21295b7fdc1732162c0be7d08a7c7fbbe48`
in `artifacts/db-copy-paste-v58-20261001/`. The ARM64 build has not run on these
x86-only emulators. v57 was an intermediate tested build, never uploaded.

| Case | First test | Fix | Retest | Evidence / variants |
| --- | --- | --- | --- | --- |
| ROW-01 | PASS (UI): single selected row | N/A | PASS exact v62 UI/native nine-field retest | OnlyID1/code-001, complete longtext and END-FULL-VALUE copied with exact typed fields/source/timestamp. Native60-row source unchanged. Evidence: artifacts/db-copy-paste-selection-final-20261001/selection-lrgo1e2c/evidence.jsonl controlsingle. |
| ROW-02 | PASS (UI): contiguous and noncontiguous | N/A | PASS exact v62 UI/native nine-field retest | Actual selection3,2,1copies IDs1,2,3; selection3,5,1copies onlyIDs1,3,5. Everyfield matches native, source unchanged. Evidence: selection-final/selection-lrgo1e2c reverse-contiguous and random-noncontiguous. |
| ROW-03 | PASS (UI): reverse and random order | N/A | PASS exact v62 selection-order retest | Reverse3,2,1and random3,5,1produce displayed-order1,2,3and1,3,5, not click order. Fresh copied timestamp/scope and complete native equality verified for each. Evidence: selection-final/selection-lrgo1e2c. |
| ROW-04 | PASS actual v60 first/last/both | N/A | PASS complete nine-field native equality | Query exposes firstID1 and lastID60; three actual copies contain only1, only60, then both in displayed order. Evidence: DATA20261001-215505-lifecycle-kha86cwv. Later no-selection observer interruption does not invalidate these completed copies. |
| ROW-05 | PASS (UI): page-scoped select all | N/A | PASS exact v62 50-of60 UI/native retest | Native source remains60rows; SELECTLIMIT50then actualSelectAll/Copy yields exactlyIDs1–50and allninefields, never51–60. Root visually reviewed copied-screen evidence. Source unchanged and bothclipboards cleared afterward. Evidence: selection-final/selection-lrgo1e2c select-all-loaded. |
| ROW-06 | PASS empty-table; first populated warning observation missed | No product defect established; preserve first miss | PASS two v60 populated-table controls | Actual Ctrl+C with100ms then default0ms injection shows Select at least one row to copy and preserves system sentinel plus saved snapshot. Evidence215856/215919 no-selection-probe directories, DATA20261001. First toast miss remains unexplained rather than relabeled a fixed app bug; see dedicated cycle. |
| ROW-07 | FAIL (UI): stale table index; later separate SQL rerun also copies wrong row | Table reload invalidation fixed; separate SQL rerun/result/automatic-refresh invalidation added | PASS tested table/SQL/header/pagination variants (UI/native DB v47) | Exact reorder and id1→id2-only SQL retests clear checks/hide Copy/show no-selection toast and preserve clipboard/native rows. Accepted automatic refresh inserts id2 above selected id1 but clears selection. Actual header Descending/Equals60, Next/Previous and Ctrl+R all clear row/range state; full copied control results independently match native order/subsets. Earlier delete/reload regression also passes. Durable evidence: row-inspector archive. Continuation: v62 automatic cell-Save refresh copied ID2 from stale ID1 selection (FAIL). Panel-scoped invalidation added; v63 exact UI/native retest passes with all checks cleared, Copy hidden, and Ctrl+C preserving the clipboard (cycle D). |
| ROW-08 | PASS copied values; FAIL changed-query source identity (UI) | Label unproven executed source as SQL query result | PASS tested table/Data/distinct SQL/view UI variants | Distinct SQL and read-only view copies match native values. Changed-query provenance now says QA PostgreSQL/qa/qa_ui/SQL query result (14:22:21); known-table control retains ui_paste_free_batch2 (14:23:09). Earlier encoded-BIGINT and stale-selection harness mistakes are explicitly excluded. |
| ROW-09 | PASS distinct SQL tabs (UI/native DB v47) | N/A | PASS active source/selection retained independently | Tab A selects id1; tab B selects id32. Actual Ctrl+Shift+Tab/Ctrl+Tab and Copy export each tab's exact native id/code/title values. Durable tab-isolation-v47 evidence. |
| ROW-10 | FAIL missing table metadata Copy no-op; later v52 query rows hidden when metadata absent (UI) | Infer ordered display/copy/export names from actual rows when metadata absent; retain authoritative types/edit guards | PASS prior table fallback plus v53 Data/SQL table/cards Copy and actual CSV export | First query failure `.o3os7sqr`:3 columns removed,2 rows preserved, but no grid. Fixed `.k67f3hqr` proves selectable Data+SQL grids, exact Copy and both51-byte CSV files; `.qg65ga50` proves Data+SQL cards and exact Copy. Native source unchanged, screenshots reviewed. Empty/affected-row-only inference additionally covered by regressions, not invented manual passes. |
| ROW-11 | PASS actual53-field copy (UI/native v52) | N/A | PASS hidden suffix included | Grid explicitly shows first48 fields; actual row Copy exports all53 fields with exact native equality. Evidence: DATA resumed-ui `20261001-182737-additional-wfw8vhmf`,18:28:55. |
| ROW-12 | PASS actual v60 source-update snapshot | N/A | PASS update and deletion snapshot variants | After native source update, actual Inspector JSON/internal-only Paste retains original completeID1. Separate actual copy→source-row deletion→Paste also preserves every original field; source stays0,targetexact1. Evidence: DATA220103-lifecycle-4k4xuif4 and221121-delete-xc6pstkd (20261001 prefixes). ADB interruption after deletion was resumed from verified saved/native state; no copy/delete/import replay. |
| ROW-13 | PASS (UI/native DB v47) | N/A | PASS prior tab isolation and actual v60 B replacement | Earlier B/id32 tab-isolation remains valid. v60 lifecycle copies A/ID1 then B/ID2; fresh replacement target receives only complete B/ID2 with independent native equality. Evidence: DATA20261001-220103-lifecycle-4k4xuif4. |
| CLIP-01 | PASS (UI): both clipboards cleared | N/A | PASS (UI v43 external-clear control) | Inspector Clear emptied both sources and Paste disappeared on writable Data table. Later format/clipboard suite independently confirmed hidden Paste with both empty. |
| CLIP-02 | PASS actual v60 internal-only paste | N/A | PASS before and after explicit force-stop | Android clipboard independently empty; actual savedID1 import preserves original values after source update. After force-stop/relaunch, retainedB/ID2 also pastes exactly with no system fallback. Evidence: DATA220103-lifecycle-4k4xuif4 and220613-lifecycle-restart-tail-1r42m89b (20261001 prefixes). |
| CLIP-03 | PASS (UI v43) | N/A | PASS (UI/native DB) | Internal rows empty; external JSON used and confirmation names Android clipboard. Multi-item test imports the first object's exact values. |
| CLIP-04 | PASS cell-text vs saved rows (UI/native v49) | N/A | PASS cell-text and newer external JSON precedence controls | Prior actual saved-row import ignores newly copied cell text (.l2py60wt). v60 savedB plus newer externalJSON9999 opens confirmation explicitly naming saved source/priority; Cancel leaves target exactlyB. Evidence: DATA20261001-220103-lifecycle-4k4xuif4. |
| CLIP-05 | PASS cell-copy variant (UI/native v49) | N/A | PASS cell, external-text and actual definition-copy precedence | Cell Copy changes Android text to root-code while row Paste explicitly identifies saved rows; externalJSON/cancel retains savedB. v61 envprec2338 copies fullDDL after2saved rows: confirmation names savedsource/timestamp/priority, actual Import inserts exactly those2native rows, not DDL. Source unchanged. Evidence: artifacts/db-copy-paste-definitions-recovery-20261001/envprec2338/result.json. |
| CLIP-06 | PASS external changes (UI v43) | N/A | PASS external and in-app Copy/Clear update paths | Foreground helper replace/clear updates Paste on return. Continuously open inspector reflects external changes and Clear (.8_n7os0d). Actual v60 CopyA→CopyB updates saved data and Paste; Clear→restart leaves no saved/system data (DATA220103-lifecycle-4k4xuif4 and220613-lifecycle-restart-tail-1r42m89b). Continuation: replacement of Android text while confirmation is open does not change the confirmed source snapshot; see PASTE-04. Existing idle visibility checks remain separate. |
| CLIP-07 | PASS (UI v43) | N/A | PASS (UI) | Foreground helper replaces/clears actual Android clipboard; Back resumes Workbench and action visibility matches content each time. |
| CLIP-08 | PASS (UI v43) | N/A | PASS (UI/native DB) | Empty string, spaces, tabs, CR/LF all hide Paste; no writes. Separate cleared-clipboard control also passes. |
| CLIP-09 | PASS row-paste URI guard; FAIL cell replacement provider read (UI) | Read direct clipboard text; never coerce a URI-only item | PASS URI-only image/png and HTML/empty-plain-text UI; null-plain-text HTML unrepresentable | URI/image retest records zero provider calls and no replacement editor. Actual HTML with empty plain text hides Paste; Ctrl+V reports Clipboard is empty, with zero native writes/provider reads (v50 root-recovery .fus6uteu). Android rejects constructing HTML with null plain text; that is a platform limitation, not an app pass. |
| CLIP-10 | PASS (UI v43) | N/A | PASS (UI/native DB) | Two real ClipData items; confirmation offers 1 row and native DB contains only first item's id 9002/name, not second-item sentinel. |
| CLIP-11 | FAIL (UI): denied write falsely says copied | Verify direct text readback before copied toast | PASS denied-write and retained-snapshot paste (UI/native v49); unavailable-service variant untested | Standard Android SystemUI recorded actual failure toast with WRITE_CLIPBOARD ignored; exact sentinel retained. App-op restored. Inspector exported the complete retained row, then actual Import inserted exactly one matching row into a fresh target. Clipboard-preview interception was a harness interruption, not an app failure; observation resumed without resubmitting. Evidence: root-recovery directories .4dkylrz3 and .g65jlq9a. |
| CLIP-12 | PASS visible clear (UI) | N/A | PASS actual v60 Clear→force-stop→relaunch | Actual Inspector Clear removes saved rows, persisted snapshot and Android content. After relaunch inspector still says empty, saved key absent and actual system clipboard empty. Evidence: DATA20261001-220613-lifecycle-restart-tail-1r42m89b. |
| CLIP-13 | PASS (UI): actual Android clear denial | Verify remaining clipboard after clear | PASS (UI) | WRITE_CLIPBOARD ignore: internal 60 rows cleared; system sentinel retained; explicit could-not-clear toast and inspector accurately reports system text. App-op restored to allow. |
| CLIP-14 | PASS APK-update/process-restart variant (UI) | N/A | PASS explicit v60 force-stop/relaunch and actual saved-row import | SavedB exact source/timestamp/fields survives with empty Android clipboard; Inspector export matches, actual Paste adds onlyB/ID2 to target already holdingA/ID1. Native target exactly[1,2], replacement target[2]. Early startup observer stopped on Loading; corrected freshPID/navigation observation resumed only pending actions, no imports repeated. Evidence: DATA20261001-220613-lifecycle-restart-tail-1r42m89b. Earlier60-row update/restart evidence remains separate. |
| CLIP-15 | PASS (UI + persisted-state readback) | N/A | PASS four variants | QA-only instrumentation seeded truncated JSON, empty string, wrong root and wrong field types. Actual app startup/inspector stays usable, shows empty clipboard and removes invalid saved key. Original value restored in finally. |
| CLIP-16 | PASS actual v60 deleted-source-profile lifecycle | N/A | PASS copy→alias deletion→restart→inspect→internal Paste | Actual source18/ID60 snapshot survives deleting only disposableQAconnection18; native database row remains. After restart and system-clipboard clear, actual confirmation targets profile1's fresh table and imports exact nine-field row. Alias18 absent,targetexact1. Evidence: DATA20261001-221409-clip16-p0qhd5kt. Earlier offscreen-picker stop preceded any copy/deletion and is retained. Only profile configuration removed; database untouched and profile recreatable. |
| FORMAT-01 | PASS baseline UI array control | Exact-priority server mapping follow-up fixed | PASS (UI/native DB v43) | Two-object array imports exact values; CLIP-10 additionally imports a single-object array. Early candidate target name/Name collision rejected; final candidate passes. |
| FORMAT-02 | PASS (REGRESSION; baseline UI not run) | N/A | PASS (UI/native DB v43) | String-column rows wrapper imports one exact row, index 1. |
| FORMAT-03 | PASS (REGRESSION; baseline UI not run) | N/A | PASS (UI/native DB v43) | Object-column metadata imports one exact row, index 2. |
| FORMAT-04 | PASS (REGRESSION; baseline UI not run) | N/A | PASS (UI/native DB v43) | Inferred union includes later-row keys; missing values become SQL NULL, index 3. |
| FORMAT-05 | PASS (REGRESSION; baseline UI not run) | N/A | PASS (UI/native DB v43) | Reordered and missing keys map correctly across 3 rows, index 4. |
| FORMAT-06 | PASS rejection (REGRESSION; baseline UI not run) | Clear shared-parser rejection | PASS (UI/native DB v43) | Empty array, empty rows wrapper and empty row object all show rejection toast and zero writes, indices 5–7. |
| FORMAT-07 | FAIL (UI): silently discarded nonobjects | Reject entire mixed array | PASS (UI/native DB v43) | Object/number/string/null/array mixture rejects before confirmation; index 8. |
| FORMAT-08 | FAIL (UI): bare object rejected as CSV | Recognize single row; preserve ordinary reserved fields; lossless copy wrapper | PASS bare-object imports and actual Copy row as JSON→external Paste (v52) | Earlier bare/reserved-field imports indices9/35/36 remain valid. Actual cell Copy row as JSON produces encoded-text wrapper; real Paste preserves JSON string and binary000102ff exactly (CELL03 evidence). |
| FORMAT-09 | FAIL (UI): malformed JSON became CSV; numeric metadata accepted | Strict JSON syntax/root/metadata validation; no CSV fallback | PASS (UI/native DB v43) | Truncated JSON, numeric metadata, primitive root, invalid escape reject with useful toasts and zero writes, indices 10–12/39. Continuation: duplicate plain/escaped/nested members initially offered Import (FAIL); decoded-name validation now rejects all three before confirmation. Three valid separate/nested/case-Unicode controls import exact values on v63 (cycle C). |
| FORMAT-10 | PASS (REGRESSION; baseline UI not run) | N/A | PASS (UI/native DB v43) | Basic two-row CSV imports both exact rows, index 13; line-ending cases exercise single-row CSV. |
| FORMAT-11 | PASS (REGRESSION; baseline UI not run) | N/A | PASS (UI/native DB v43) | All 6 LF/CRLF/CR × with/without-final-newline variants import exactly, indices 14–19. |
| FORMAT-12 | PASS (REGRESSION; baseline UI not run) | N/A | PASS (UI/native DB v43) | Comma, embedded LF, escaped quotes and quoted empty text retain exact content, index 20. |
| FORMAT-13 | FAIL (UI): duplicate header accepted; BOM defect in regression | Reject duplicates; strip BOM; retain exact case/Unicode | PASS (UI/native DB v43) | Duplicate rejects; BOM imports; whitespace/blank header/case-distinct name and Name/Unicode values are exact, indices 21–23. |
| FORMAT-14 | FAIL (UI): short/extra records silently accepted | Reject uneven records with record detail | PASS (UI/native DB v43) | Both missing and extra field variants reject before confirmation; zero writes, indices 24–25. |
| FORMAT-15 | FAIL (UI): explicit all-empty row dropped | Preserve explicit empty records; ignore physically blank lines | PASS (UI/native DB v43) | Empty-field record persists empty strings; blank-only input rejects no rows; blank between populated records imports exactly 2, indices 26/27/40. |
| FORMAT-16 | FAIL (UI): unclosed/bare quotes accepted | Strict quoting validation | PASS (UI/native DB v43) | Unclosed quoted and quote-in-unquoted fields reject with record number; indices 28–29. Trailing text after closing quote additionally covered by regression. |
| FORMAT-17 | FAIL (UI): TSV collapsed; ambiguous header misparsed | Detect TSV; reject unquoted mixed-delimiter header | PASS (UI/native DB v43) | Spreadsheet TSV imports exact two-column row; mixed comma/tab header clearly rejects, indices 30–31. |
| FORMAT-18 | FAIL actual shared serializer→HTTP/native round trip: 5/10 JSON values changed | Lossless encoded-text wrapper and native-json marker | PASS (UI/native DB v43); full serializer/API round trip 10/10 | JSON null/empty/literal null and CSV empty/quoted-empty/literal null checked, indices 32–33. JSONB addenda 37–38 additionally prove SQL NULL versus JSON null/string/boolean/number using IS NULL, jsonb_typeof and text casts. CSV does not preserve SQL NULL distinction. |
| FORMAT-19 | FAIL (UI): wrong wrapper type accepted | Validate wrapper type and metadata | PASS ten malformed-wrapper Android variants (v50) plus prior API controls | Numeric/null type, null/empty/numeric/duplicate columns, rows-object/missing rows, invalid valueFormat, and unlisted fields each produce their specific validation toast, no import confirmation, and zero native writes. Evidence: root-recovery .lkp86gcx; earlier wrong-type first failure retained. Continuation: duplicate-member first failures and all six actual v63 reject/valid controls are recorded in cycle C; these do not relabel older wrapper cases as rerun. |
| MAP-01 | PASS (API/DB) | N/A | PASS (UI/native DB) | Exact names insert every value into the correct PostgreSQL column; batch2 13:42:13. |
| MAP-02 | PASS (API/DB) | N/A | PASS (UI/native DB) | Reordered source fields remain name-mapped; exact native comparison. |
| MAP-03 | PASS (API/DB) | N/A | PASS (UI/native DB) | Separately executed User ID and USER-ID both map to user_id; exact user_id control also passes. |
| MAP-04 | PASS (API/DB) | N/A | PASS (UI/native DB) | Exact user_id wins over User ID; ignored normalized source is explicitly reported. |
| MAP-05 | FAIL (API/DB) | Fixed: reject ambiguous/unrelated mappings | PASS (UI/native DB) | Ambiguous User ID/USER-ID sources produce useful rejection and zero writes. |
| MAP-06 | FAIL (API/DB) | Fixed: reject ambiguous/unrelated mappings | PASS (UI/native DB) | Unrelated Unicode 名前 does not map to 城市; no compatible columns, zero writes. |
| MAP-07 | FAIL (API/DB) | Fixed: reject ambiguous/unrelated mappings | PASS (UI/native DB) | a_b matching multiple target columns is rejected, with zero writes and useful explanation. |
| MAP-08 | PASS (API/DB) | N/A | PASS (UI/native DB) | Known values insert exactly; unknown source is visibly reported as ignored. |
| MAP-09 | PASS (API/DB) | N/A | PASS (UI/native DB) | No matching columns rejected before insertion; native table unchanged. |
| MAP-10 | PASS (API/DB) | N/A | PASS (UI/native DB) | Missing required title rejects preflight; column failure reported, zero writes. |
| MAP-11 | PASS (API/DB) | N/A | PASS (UI/native DB) | Unmapped nullable fields become SQL NULL and omitted defaulted field uses db-default. |
| MAP-12 | Observed (API/DB): missing mapped field becomes NULL | Conditional confirmation warning added | PASS (UI/native DB) | Actual confirmation warns mapped missing fields become SQL NULL, not defaults; two native rows prove supplied value versus SQL NULL. |
| MAP-13 | PASS (API/DB) | N/A | PASS actual PostgreSQL and v62 SQLite/MariaDB/genuineMySQL generated-column controls | Each additional engine's actual Import inserts2rows, reports ignored id/computed, and stores generatedIDs1/2, uppercasecomputed and db-default exactly. Separate native DEFAULTVALUES probe getsID3; not counted as another Android insertion. Evidence: artifacts/db-copy-paste-generated-recovery-20261001/sqlite-8fwhmjj8,mariadb-yay_b7ky,mysql-7rnnygp0. Earlier SQLite observer expected SQL-empty wording on a Data panel; stopped beforePaste, nativeempty, no product defect or repeated import. |
| MAP-14 | PASS (API/DB) | N/A | PASS (UI/native DB) | Missing non-generated TEXT primary key produces Inserted0/failed1 with id NOT NULL detail; table unchanged. |
| MAP-15 | PASS (API/DB): rename/add/drop | N/A | PASS (UI/native DB): all3 confirmation-timed DDL variants | Native rename/add/drop applied while actual Android confirmation was visible. Rename rejects old name; added default applies; dropped field is ignored; no wrong-column insertion. |
| DATA-01 | PASS specified API/DB variants | N/A for tested variants | PASS applicable cell/internal/JSON/CSV UI/native text controls | Actual empty/whitespace/trim-sensitive/65,536-byte text and VARCHAR(8) length8/9 controls pass. JSON text/varchar groups are complete; cell crosswalk includes empty, spaces, whitespace-only, width and large-text controls. This is not engine TEXT-maximum coverage. |
| DATA-02 | FAIL v54 UI NUL error clarity; other escaping API checks passed | PostgreSQL raw-NUL generated-statement guard | PASS exact NUL fix and applicable cell/internal/JSON/CSV escaping | First vagueHTTP500 preserved in192005-cells-v_fzxlgv. Exact NUL retest gives readableHTTP400 binary guidance and unchanged native UTF8hex. Quotes/backslashes/comma/tab/CRLF/control strings survive actual JSON/cell operations. Root reviewed rejection screenshot. |
| DATA-03 | PASS API/native UTF-8 oracle after harness charset correction | No product fix for CLI charset issue | PASS applicable cell/internal/JSON/CSV Unicode variants | Emoji/ZWJ, combining accent, non-Latin scripts and RTL controls have actual cell/JSON storage evidence. CSV Chinese FORMAT23 remains separate. Four-engine API preservation is not four-engine typed Android coverage. |
| DATA-04 | PASS specified API/DB variants | N/A | PASS applicable cell/internal/JSON/CSV literal-data variants | Actual quote/comment/backslash/SELECT and dollar-delimiter text remain literal stored data. Native independent equality, not only success messages, verifies these cases. |
| DATA-05 | PASS specified API/DB variants | Lossless v60 database JSON transport adds protection | PASS applicable cell/internal/JSON/CSV integer boundaries and native_bulk | Actual0/-1/signed64 min/max/9007199254740993 and both overflow rejections pass. JSON native_bulk additionally preserves exact typed fields. SQLite REAL-affinity overflow and unsigned-width limits remain qualified in the earlier API evidence. |
| DATA-06 | FAIL API numeric preservation; v54 NaN editor rejection; v59 raw JSON silently rounded | Exact-text editor validation; v60 database-scoped lossless JSON serializer | PASS exact raw-number fix and applicable cell/internal/JSON/CSV numeric variants | Original raw12345678901234567.12345678901234567890 stored12345678901234568.00000000000000000000 despite1/0. Fresh v60 target preserves every digit; original corrupted target remains. String-decimal/scientific/scale/native nonfinite/overflow and float-invalid controls pass. Actual wire regressions and513-test suite support, not replace, UI evidence. |
| DATA-07 | FAIL v54 actual Boolean-zero replacement; API aliases passed | Android validation reuses shared Boolean alias parser | PASS zero/one fix and applicable cell/internal/JSON/CSV Boolean controls | Native true→false→true proves actual0/1 saves. Invalid aliases reject unchanged; true/false and numeric/string alias routes retain their explicitly recorded native outcomes. Continuation cycle B found transfer-only integer0/1 rejection into PGBOOLEAN; exact transfer normalization added. All16typed v63 engine pairs preserve Boolean/SQLNULL fields; additional actual SQLite0/1/NULL/2 boundary passes3/1 with invalid2 absent, source unchanged, and accurate Partial card/details. |
| DATA-08 | FAIL API DATE/TIMESTAMP shift and microsecond loss | Preserve date-only/naive semantics and timestamp fractions | PASS applicable cell/internal/JSON/CSV date/zoned/native_bulk plus October3 TIME/TIMETZ and named-zone controls | Actual valid/invalid leap days/month/empty, UTC/+07, explicit DST-before/after offsets and fractional seconds pass. October3 Android imports and native-query Copy preserve TIME(6)/TIMETZ(6) fractions, offsets,24:00, reject25:00; named-zone gap/fold and explicit-offset comparators preserve exact microseconds, reject invalid zone. Both clipboards and unchanged schema checked. SQLite date affinity and every engine's temporal cross-product are not implied. |
| DATA-09 | FAIL PostgreSQL custom-enum API read panic | Safe unknown-OID text fallback | PASS applicable cell/internal/JSON/CSV UUID/enum variants | Actual valid/invalid/empty native PG UUID and enum routes pass. Text/CHECK-based fixtures on other engines are not native UUID coverage. |
| DATA-10 | FAIL API/helper typed data loss; UI NaN acceptance and full-view JSON/null copy | Strict JSON validation; resolved metadata; lossless formatter and serializer | PASS applicable cell/internal/JSON/CSV structured values and separate full-view retests | Actual JSON object/array/string/null/Boolean/number and encoded-text routes preserve types and SQLNULL distinctions. Native multidimensional arrays, empty arrays and invalid replacement controls pass. Four Data+four SQL full-view retests remain separately linked underCELL02; no full portable-schema guarantee. Continuation cycle B exposed JSONtrue→SQLiteTEXT1 despite8/0. Source-aware serialization now preserves all tested document kinds and SQLNULL in16actual v63 typed engine pairs, including MySQL→SQLite. |
| DATA-11 | FAIL API/helper invalid markers, marker bytes and bytea[] loss | Marker validation, editor buffers and recursive bytea[] decoding | PASS applicable cell/internal/JSON/CSV binary variants | Actual NULL/empty/000102ff/262,144-byte data, multidimensional/empty arrays and bad base64/encoding/native-hex rejection pass with exact native bytes. Internal256KiB copied source and imported target both match independently. CSV marker-looking text is not a typed buffer. Continuation actual MySQL→MySQL corrupted BLOB74727565to31 and failed object-looking bytes. Narrow charset-aware SQLx patch and removal of content guessing now pass exact binary/NULL/empty/JSON-looking bytes across16actual v63 typed pairs; original failed target preserved (cycle B). |
| DATA-12 | FAIL API POINT snapshot paste; unchanged POINT editor rejected its own prefill | Typed POINT conversion and shared numeric x/y parser | PASS exact POINT fix and applicable cell/internal/JSON/CSV POINT/INET controls | Unchanged object, changed object, native coordinate text and malformed-object rejection verified in actual UI; other fields unchanged. IPv6 and invalidINET pass. First failure/retests: artifacts/db-copy-paste-data-20261001/point-edit-regression-20261001.md. PostGIS/SRID/extensions remain outside coverage. |
| DATA-13 | PASS specified API/DB variants | N/A | PASS applicable internal/JSON/CSV PK/composite variants; PK editing guarded | Existing and in-batch duplicate keys produce exact failed indices; valid later rows persist and seeded rows remain unchanged. PK cell replacement is deliberately unavailable rather than an omitted successful mutation. |
| DATA-14 | PASS specified API/DB variants | N/A | PASS applicable cell/internal/JSON/CSV constraint variants | UNIQUE/NOTNULL/CHECK/FK failures identify rows, preserve originals and retain valid rows before/after. The actual result dialog and independent native count/values agree. CSV empty text is not SQLNULL. |
| DATA-15 | PASS specified API/DB variants | N/A | PASS applicable cell/internal/JSON/CSV relationship variants | Parent-before-child, child-before-parent, missing parent and self-reference outcomes agree with native constraints. Root visually reviewed complete self-FK report. Deferred/cyclic/cascade/concurrent variants are not implied. |
| DATA-16 | PASS specified API/DB variants | N/A | PASS internal/JSON/CSV generated/default/trigger UI imports plus labeled native next-ID probes | Two actual Android rows omit supplied identity/computed values, execute defaults and audit triggers, and store exact generated fields. A separately labeled native DEFAULTVALUES insert getsID3 and correct defaults/audit; it is not counted as a third UI insertion. |
| PASTE-01 | PASS SQL no-destination variant (UI) | N/A | PASS v61 actual no-selected-table state and passive zero-request audit | Actual Explorer connection selection removes selected-table key while retaining saved rows/history. Inspector Paste is disabled; tap and Ctrl+V open no confirmation. Same8940runtime/control/state observes0→0row-paste POST arrivals over10.2seconds; both clipboards unchanged. Evidence: DATA20261001-223638-no-target-f5z53jw1. Earlier SQLSELECT1 guard remains separate; no universal absence-of-network claim. |
| PASTE-02 | PASS view guard (UI/native DB) | N/A | PASS view UI and actual database-denied destination controls | Read-only view hides Paste/Delete; outside-editor Ctrl+V does nothing and base table is unchanged. Row Edit controls are disabled; view Copy retains exact values. Separate ROBUST02 actual INSERT-denied/read-only imports report0/1 with readable errors and unchanged native rows; permissions are enforced by the database, not predicted from column metadata. |
| PASTE-03 | FAIL (UI): ambiguous Data JOIN allowed Paste into selected table | Require safe single-table source in Data/SQL paste action and entrypoints | PASS tested Data/SQL UI variants | Data JOIN, distinct SQL JOIN and aggregate hide Paste and reject outside-editor Ctrl+V with zero writes. Simple Data and correctly scoped distinct SQL query insert exact new rows. |
| PASTE-04 | PARTIAL (UI): target/count/columns correct; source absent | Added source identity and snapshot timestamp | PASS tested confirmation fields (UI) | Actual Android source, QA PostgreSQL/qa/qa_ui/ui_target, row count and columns match inputs. Confirmation also states insert-only and partial-retry semantics; saved-snapshot identity/timestamp is separately observed in CLIP-14. Continuation: actual clipboard A→confirmation→clipboard B→one Import retains A and stores exactly A with UI1/0. This verifies source snapshot, not destination switching or cancellation. |
| PASTE-05 | PASS (UI/native DB): Cancel, Back and outside | N/A | PASS final-candidate regression | All three dismissals close confirmation, release the import gate and leave fresh ui_paste_final_142648 empty (14:27:21–14:27:47). |
| PASTE-06 | PASS attempted tab switch while confirmation open (UI) | N/A | PASS reachable tab/connection attempts, including v60 | Prior Ctrl+Tab retains captured target and exact1-row import. v60 actual tap on underlying Connection spinner safely dismisses confirmation without switching or writing; Ctrl+Tab leaves identical source/target/content, then Cancel preserves native rows. No successful connection-switch-and-import race was reachable or fabricated. Evidence: artifacts/dbqa-env-keyboard-final-smoke-20261001.md, .eav845mi. |
| PASTE-07 | PASS insert/no-upsert variants (UI/native DB) | N/A for tested variants | PASS explicit selected-record UI/native | Row1 is verified selected, then Paste appends a new generated-ID row; selected row and all existing values remain unchanged. Duplicate-constrained variants also reject rather than upsert. |
| PASTE-08 | PASS (UI/native DB): 2 inserted/0 failed | N/A | PASS final-candidate regression | Both code/title values and generated IDs independently read from fresh PostgreSQL fixture (14:28:01); earlier nullable/default-note values also verified. |
| PASTE-09 | PASS (UI/native DB): first/middle/last failure | N/A | PASS final-candidate regression | Each3-row batch inserts exactly2, reports1 duplicate-code failure at#0/#1/#2, and retains exact successful values (14:28:15–14:28:45). |
| PASTE-10 | PASS (UI/native DB): all 2 rows fail | N/A | PASS final-candidate regression | Dialog says Inserted0;2failed with both row indices; destination unchanged (14:29:01). |
| PASTE-11 | PASS tested accounting (UI/native DB) | N/A | PASS final-candidate regression | All-success,3 partial positions, all-fail and partial-retry counts match native deltas; zero-based error indices identify correct inputs (14:28–14:29). |
| PASTE-12 | FAIL UI: missing/inconsistent counts falsely reported; warning dropped | Strict response-count validation and warning display implemented | PASS all8 rebuilt Android/native fault variants | Final candidate correctly handles error-only, malformedJSON, wrongshape, incomplete/inconsistent counts, result+error, result+warning and disconnect-after-commit. Exact consumed proxy events and native deltas retained; unknown outcomes are never reported as confirmed0/0. |
| PASTE-13 | FAIL UI/native: rapid taps insert duplicate rows | Confirmation/in-flight single-operation guard implemented | PASS exact rapid-tap UI/native retest | Baseline triple Paste/Import persisted2identical rows and stacked2dialogs. Final candidate inserts exactly1 row, shows1 result, leaves no stacked confirmation (14:20:12). Cancel/repeat regressions tracked below. |
| PASTE-14 | PASS constrained repeat (UI/native DB) | N/A | PASS constrained and unconstrained final-candidate UI/native | Repeated unique codes fail and preserve old values; final candidate accepts2 deliberate fresh confirmations into unconstrained fixture as2 separate exact rows (14:31:06/14:31:20). |
| PASTE-15 | PASS (UI/native DB): partial then retry | Existing confirmation warns successful rows remain/retry may duplicate | PASS final-candidate regression | First attempt inserts1/fails1; retry inserts0/fails2 because the prior successful code now conflicts (14:29:17/14:29:32). Native rows preserved; warning visible before both attempts. |
| PASTE-16 | FAIL (UI/native DB): completion executes edited SQL without Run | Captured-query/scope guard plus conservative automatic SELECT gate | PASS changed-draft UI/native retest; unchanged active-query counts pass | Root's first completed 10-row import executes unclicked UPDATE; retest at 13:31:14 inserts 10 while the visible draft remains unexecuted and native probe stays safe-refresh-before. |
| PASTE-17 | FAIL unclicked UPDATE; later FAIL missing completion target context | Safe refresh fixed; captured-target summary implemented | PASS tested changed-draft, switched-panel and closed-source UI variants | Final Ctrl+Tab switch keeps other panel active/empty and names original target in result (14:20:53). Ctrl+W closes source during delayed request; only original target receives row, result names it, closed panel stays closed (14:33:02). Whole-workspace cancellation is distinct; offscreen-tab harness abort excluded. |
| PASTE-18 | PASS (UI/native DB) | N/A | PASS filtered-out insertion | Actual query remains No rows returned while one nonmatching row inserts; UI reports1/0 and native row exists outside filter. |
| CELL-01 | FAIL (UI): Name copied as name | Exact metadata lookup; unique-only case-insensitive fallback | PASS prior Name fix plus six actual v52 exact-name variants | Space, mixed quote/comma, reserved select, Japanese+emoji, Name and name each copy exactly. Evidence: DATA resumed-ui `20261001-183133-additional-dnq176w3`; actual clipboard readback, not displayed text alone. |
| CELL-02 | FAIL (UI v48): typed full viewer re-encodes JSON string/null and mislabels SQL NULL | Shared lossless formatter for cell Copy and continuous viewer | PASS eight Data/SQL variants (UI/native v49) | Native JSON string, object, JSON null and SQL NULL copied through actual full-value modal; four typed Data and four separate SQL results agree with native values/types and SQL NULL label. Screenshot visually inspected. First failures/retests: artifacts/db-copy-paste-data-20261001/full-value-ui-report-20261001.md. Earlier full-size/UTF-8 byte-boundary controls remain in CELL12/13; other typed/range variants remain separately tracked. |
| CELL-03 | PASS actual Copy row as JSON→Paste (UI/native v52) | N/A | PASS exact complete fields and typed values | Clear saved rows through Inspector, actual cell action copies valid encoded-text row wrapper; external Paste inserts exactid1, JSON string true (not boolean) and bytes000102ff. Evidence: DATA resumed-ui `20261001-182513-additional-keayjhjn`,18:27:23. |
| CELL-04 | PASS read-only view variant (UI) | N/A | PASS view/no-PK, PK/identity/generated and v62 SELECT-only-role cell controls | Actual unsupported targets omit edit/paste/SQLNULL while view/copy remain; native data unchanged. Ordinary NOTNULL field permits editing but noSQLNULL. Fresh SELECT-onlyrole/profile27 has SELECT=true/UPDATE=false; actual title details omitEdit/Paste-as-replacement and preserve exactrow. No Save attempted, no claimed denied-Save/CELL10 pass or universal privilege detection. Evidence: DATA182904/215155 plus generated-recovery/update-denied-bc4jttqc. Continuation: a fresh editor opened with UPDATE allowed, then the grant was revoked. v63 safely rejected one Save and preserved the row; this is separate from the earlier pre-open read-only control (cycle E). |
| CELL-05 | PASS (UI) | N/A | PASS tested text/JSON variants | Clipboard text appears exactly before Save; JSON query column uses resolved native column type. |
| CELL-06 | PASS (UI/native DB) | N/A | PASS Cancel→Discard | Original stored text including whitespace remains unchanged. |
| CELL-07 | FAIL (UI/native DB): wrong column changed | Exact metadata lookup and edit allowlists; strict query PK names | PASS reproduced case (UI/native DB) | First edit changed name instead of Name. Rebuilt retest changes only Name to Upper-fixed on id 1; name remains Upper-edited and all other fields unchanged. More cell-type variants remain under CELL-09/10. |
| CELL-08 | FAIL URI-only variant (UI): unintended provider read | Read direct clipboard text only | PASS empty/absent/URI variants (UI/native DB/provider audit) | Empty string stores non-NULL empty text; absent clipboard reports no text. URI-only retest opens no editor and records zero provider calls. |
| CELL-09 | PASS (UI/native DB) | N/A | PASS distinctions plus five explicit v60 SQLNULL actions | Empty text/literalnull/JSONnull/JSONstring remain distinct. Actual Set SQLNULL from nonNULL JSON, text[], JSONB[], BYTEA and BYTEA[] baselines passes native ISNULL checks,5/5. NOTNULL field omits only Set SQLNULL, retaining ordinary edit. Evidence: DATA214921-additional-ggrmcptb and215155-additional-jwjgyixp (20261001 prefixes). |
| CELL-10 | FAIL query JSONB NaN acceptance; later v53 error reason inaccessible after toast | Strict typed validation; explicit44dp persistent full-error control | PASS exact feedback retests and completed86-key typed cell matrix | First truncated-reason screenshot `184125-cells-aip_h75y/005-PASS_UI.png` retained. v54 normal/1.5font retests expose complete reason/Close and preserve12345678; font restored. All86cell keys plus11numeric/Boolean controls now have actual UI/native evidence, including invalid type/constraint cases with unchanged originals. Crosswalk: artifacts/db-copy-paste-data-20261001/cell-evidence-crosswalk.md; versions/failures remain individually attributed. Continuation: revoked UPDATE initially caused a false no-primary-key error (FAIL). The narrow privilege diagnostic now passes three live regressions and exact v63 Android retest: HTTP403 UPDATE-privilege reason, unchanged native row (cycle E). |
| CELL-11 | PASS concurrent-update variant (UI/native DB) | N/A | PASS concurrent update and actual v60 deletion | Other-client update remains withHTTP409. Separate deleted-row Save shows actualHTTP404 Row not found event plus persistent error-details control; native count stays0 and row is not recreated. Evidence: DATA20261001-215409-additional-o7z96_74. Root reviewed screenshot/event; no success message inferred from stale pre-edit grid. Continuation: held-uncommitted-peer Save and actual bulk Delete FAILED on v62. Transactional lock/check/write plus aligned bulk snapshots now pass exact v63 UI/native retests: actual lock wait, peer COMMIT, truthful conflict, peer value preserved (cycle A). Legacy callers omitting snapshots remain unguarded. |
| CELL-12 | PASS (UI): all three ASCII byte boundaries | N/A | PASS native exact comparison | 499,999 and 500,000 UTF-8 bytes copied exactly (SHA-256 recorded); 500,001 opens file picker. Cancel retains clipboard sentinel. |
| CELL-13 | PASS (UI): all three emoji byte boundaries | N/A | PASS native exact comparison | 499,999 bytes/125,002 characters and 500,000 bytes/125,000 characters copied exactly; 500,001 bytes opens picker. Cancel retains sentinel. |
| CELL-14 | PASS Cancel variant (UI); later actual write/missing-picker failures pass | Existing picker-launch exception handling and serialized save slots verified | PASS Cancel, provider-write failure, missing picker and exact normal retries (UI/native v51) | New provider audit and actual error toast prove failed write; disabled exact picker resolves No activity found and reports failure, then is restored. Both preserve sentinel/process/details; normal retries each save all500001native bytes, SHA95ed9dfa…. Evidence root-recovery `.xc9z97f5` + `.lvy7_887`; excluded calibration attempts detailed in `file-failure-calibration.md`. |
| RANGE-01 | PASS (UI/native DB) | N/A | PASS both CSV and TSV | One cell, row, column, and rectangle each preserve the exact selected subset. |
| RANGE-02 | PASS (UI/native DB) | N/A | PASS both CSV and TSV | All four corner directions retain displayed table order. |
| RANGE-03 | PASS (UI/native DB) | N/A | PASS 14 shape/direction-format combinations | Parsed headers, delimiters and exact selected values compared independently. |
| RANGE-04 | PASS (UI/native DB) | N/A | PASS both CSV and TSV | Special column name plus comma/tab/newline/quotes/empty text retain exact values after parsing copied text. |
| RANGE-05 | FAIL actual v60 single-column empty-row loss | Quote empty CSV/TSV fields in range and whole-result export | PASS exact v61 CSV+TSV fix, adjacent CSV-file and all typed range controls | Before: note+two bare newlines loses selected rows. After: both range formats and actual saved CSV parse as header plus two one-field empty records; nativeSQLNULL/empty source unchanged. Boolean/numeric/nestedJSON×CSV/TSV also match native values. First failures221638/221927 retained; exact retests DATA20261001-222708-range-_nw_wpp9,223009-empty-csv-export-vtyeepd1,223102-range-zwsjig99. CSV still has noSQLNULL tag. Root reviewed nested-JSON selection screenshot. |
| RANGE-06 | PASS (UI/native DB) | N/A | PASS both CSV and TSV | Case-distinct Name/name remain separate across all tested shapes/directions. |
| RANGE-07 | PASS actual header/page/refresh controls (UI/native DB v47) | N/A for these controls | PASS header/pager/refresh plus v61 query reload/WHERE/OFFSET | Earlier actual sort/filter/Next/Previous/refresh clears row/range state and preserves clipboard. Three additional v61 tests verify an active range immediately before actual Run; same-query reload, changedWHERE and changedOFFSET each clear range controls. Evidence: DATA20261001-223102-range-zwsjig99. Query edits are not relabelled as header/pager actions. Continuation: an additional v62 three-cell range clears after cell-Save query reorder; native order and fresh execution timestamp verified. No range fix was needed (cycle D). |
| RANGE-08 | PASS (UI) | N/A | PASS after each of 16 copies | Done hides toolbar and resets the next selection. |
| UI-01 | PASS inspector metadata (UI) | N/A | PASS current v47 1/50/51-row variants | Actual source/timestamp/count/3 columns match each new snapshot. Earlier 50-row9-column and60-row restart observations remain separate; six-row large snapshot has4columns, not5. Durable inspector-boundaries-retest-v47 evidence. |
| UI-02 | PASS stored-count/export variants; initial exact-end automation incomplete | Correct only the harness's nested/outer scroll end oracle | PASS actual 0/1/50/51 boundaries (UI/native DB v47) | Empty state has no CopyJSON. One-row preview ends at1. Actual last-visible48/49/50 followed by next section establishes50-row end. With51stored, preview ends50/notice and CopyJSON still exports all51 exact native rows. Initial insufficient-scroll assertions are preserved as harness-incomplete, not product failures. |
| UI-03 | PASS six-row file fallback and 50/60-row export (UI) | Safe clipboard/file publication path | PASS current v47 beyond-preview full export | Actual CopyJSON at1/50/51 boundaries preserves every native field, including row51 beyond the50-row preview. Earlier six-row large-file fallback retains all6 exact ASCII/emoji byte lengths; final large-copy smoke remains separately tracked. |
| UI-04 | FAIL (UI v46): unsafe SQL inspector Paste enabled; active SQL B targets last Data A | Capture inspector entry section; use active query key, safe-source guard and actual query dispatch | PASS known/view/unsafe-query/context retests and v61 no-selected-table control | Known Data target imports exactly; view/unsafeJOIN disable Paste; SQL-B writes onlyB whileDataA stays empty. Actual v61 connection-only Explorer→Inspector retains saved1row but disables Paste; disabledtap/Ctrl+V produce no confirmation and zero observed row-pastePOSTs. Evidence: artifacts/db-copy-paste-row-inspector-20261001/ and DATA20261001-223638-no-target-f5z53jw1. |
| UI-05 | PASS clear variant (UI) | N/A | PASS continuously open inspector (UI v49) | Actual Copy JSON exports all saved fields; external replacement retains the internal 1-row/3-column preview. Clear removes actions/preview, external text appears live, external clear returns empty. Root-recovery .8_n7os0d. Selecting new grid rows is unavailable while the inspector is open and is not claimed. |
| UI-06 | FAIL (UI): Ctrl+C in Data does nothing | Route database shortcuts at Activity; add Data C/V | PASS current v60 Ctrl controls; Meta unavailable on test input path | Actual Ctrl copy has exact native values and Ctrl Paste opens correct confirmation. Separate focused calibration receives all4Ctrl events; Meta has4accepted injections but0callbacks and launches Android Contacts. No Workbench Meta pass or universal platform claim. Evidence: artifacts/dbqa-env-keyboard-final-smoke-20261001.md, .m469gyjd/.gy0e8ecr. |
| UI-07 | Initial foreground/selection observer interruptions | Verify focus and actual caret insertion; no product change | PASS native Ctrl editor Copy/Paste on v49 and v60; Meta unavailable | Selected SQL copies exactly; actual v60 Paste inserts exact clipboard text at caret without Run/import, original SQL otherwise intact and restored, native rows unchanged. Unproved select-all/HOME expectations are harness interruptions, not app failures. Earlier .cklrgvi8 and current .pv8_7luv evidence retained; Meta blocked by calibrated input-path limitation. |
| UI-08 | FAIL (UI): Paste stopped after Cancel | Only suppress repeated DOWN, not a fresh press | PASS held Ctrl+C/V repeats and fresh later press (UI/native v49) | Actual 20 repeated DOWN events plus UP produce exactly one copy publication and one import with one persisted row; no stacked confirmation. Fresh Ctrl+V still opens a new confirmation, cancelled unchanged. Evidence: root-recovery .vk5p4fhk. |
| UI-09 | FAIL (UI): F6 not routed without child focus | Activity-level database shortcut handler | PASS (UI): F6 opens, Escape restores original Data panel | Retested on version 43 after native restart. |
| UI-10 | FAIL font-scale change (UI v50): returns Database to Projects; later v51 inactive SQL draft loses scope | Restore Database destination; persist SQL only from complete mounted editor snapshot | PASS v51 layout/v52 SQL recreation plus actual v63 TalkBack focus/actions; audible speech unverified | Layout `.6oq1ogzh/visual-review.md`; scope first failure `scope-9rr0n6hz`, fixed inactive `scope-fgm4uwo1`/active `scope-639ebhf7` retain all fields/exact SQL without execution. v63 actual hardware TalkBack selection, Copy, Paste/Cancel, Import1/0, duplicate Import0/1, full error focus and dismissal pass with exact native rows. Original nine settings/notification flags restored; both clipboards cleared. `artifacts/dbqa-talkback-results-20261002.md`. Settled navigation passes; initial early post-launch tap did not navigate and remains a qualified observation. Emulator has no audio; no spoken-output claim. |
| TRANSFER-01 | PASS server definitions and small SQLite UI copy | N/A | PASS actual table/database/schema/trigger definition scopes | v61 envdef2315 plus envdef2315-resume complete11scopes: exact PG/MariaDB/MySQL tableDDL, both MariaDB/MySQL triggerDDL, absent PG/MariaDB/MySQL database and PGschema definitions with no falseCopy, exact21-byte descriptive SQLite database text without OpenSQL, and missing-table404. Earlier smallSQLite table87bytes remains valid. Allavailable text compared in full to native definition; unavailable definition is not invented as DDL. Evidence under artifacts/db-copy-paste-definitions-recovery-20261001/. |
| TRANSFER-02 | FAIL (UI): giant definition details ANR before Copy; later .sql.txt filename defect | Bound text previews; virtualize full view; guard Copy/file fallback; correct SQL MIME | PASS giant open/cancel/save/full-view plus unavailable/missing/empty/whitespace UI controls |600,077-byte Details is bounded; Cancel preserves clipboard; direct/full-view Save produce exact.sql files; Find reaches finalcharacters600076–600077. v61 missingSQLite table404 and unavailable definitions expose no falseCopy. envfault2343 consumes one scopedGET each for empty/whitespace, hidesCopy/OpenSQL, preserves sentinel/native source, then restores direct8928/proxypassthrough. Historical emulator exit remains unconfirmed, not an invented app crash. Evidence: transfer-recovery and definitions-recovery directories. |
| TRANSFER-03 | PASS (API/DB) | N/A | PASS basic16-pair UI matrix plus v60 reordered/missing-target controls | Actual forms copy exact rows into existing targets. Separate SQLite reordered target maps byname; missing-existing-target job fails, source unchanged. Evidence: transfer-recovery `{v49,v50}-matrix/matrix.jsonl` and v60-context/context.jsonl cases306/308. |
| TRANSFER-04 | FAIL (API/DB): MySQL source metadata absent | Decode catalog UTF-8 byte strings | PASS basic16-pair schema-only UI/native matrix (v49/v50) | Actual forms create empty destinations with expected basic schema. Advanced-property fidelity remains separate; original metadata failures are preserved. |
| TRANSFER-05 | FAIL (API/DB): MySQL source became TEXT PK | Preserve catalog type metadata | PASS basic16-pair schema+data UI/native matrix (v49/v50) | Each target has two exact INTEGER/TEXT records and expected basic schema; source is unchanged. This is not the full type/boundary cross-product. |
| TRANSFER-06 | FAIL MySQL metadata variant; SQLite passed | Catalog type metadata fix | PASS actual v60 empty-source UI/jobs/native on all4engines | PostgreSQL, SQLite, MariaDB and genuineMySQL8.4 each create the expected empty destination, with0copied and correct terminal job/native state. Evidence: artifacts/db-copy-paste-transfer-recovery-20261001/v60-boundaries-tail/. Earlier API first-failure/corrected replay remains preserved. |
| TRANSFER-07 | PASS (API/DB) | N/A | PASS API all4engines and actual v60 conflict UI/native | Actual schema+data conflict reports Failed; seeded destination remains1row and source unchanged. Evidence: transfer-recovery/v60-context/context.jsonl case307. No overwrite. |
| TRANSFER-08 | PASS (API/DB) | N/A | PASS same/different-profile, cross-engine and separate-schema/database UI/native | Basic48 matrix exercises actual target selectors. Additional v60 PostgreSQL source-schema and target-database cases301/302 each reach the exact scoped destination with independently verified expected values. Evidence: transfer-recovery/v60-context/context.jsonl. |
| TRANSFER-09 | PASS server identifier variants | No product fix; correct empty-field hint in form observer | PASS all16actual identifier UI/native variants and suggested/manual-name controls | Spaces, Unicode, quote/backtick names and exact reserved select on all4engines have exact native names/values with unchanged sources. Evidence: transfer-recovery v60-names, v60-names-reobserve-403, v61-names. Case403 was observed from its already completed job after buildpause, never resubmitted. v61-suggestions-retest verifies same-connection _copy default, different-connection source-name default, and preservation of manually edited name without submission. Initial observer stop is retained separately. |
| TRANSFER-10 | PASS (API/DB) | N/A | PASS actual v60 exact-self UI rejection | Explicit identical source/target rejects; full error is accessible and2native source rows remain unchanged. Evidence: transfer-recovery/v60-context/context.jsonl case305. |
| TRANSFER-11 | FAIL (API/DB): consumed newly inserted rows | Stream one source snapshot to temporary JSONL; bounded read batches | PASS actual v60 SQLite and PostgreSQL aliased-self UI/jobs/native | Each actual transfer copies exactly original401once, reports401copied and leaves802native rows. SQLite profile alias and PostgreSQL implicit/explicit schema alias both pass, no runaway consumption. Evidence: transfer-recovery/v60-context/context.jsonl cases303/309. Original201→402attempts failure and API retests remain preserved. |
| TRANSFER-12 | PASS (API/DB) | Snapshot fix preserves boundaries | PASS actual v60 UI/jobs/native0/1/199/200/201/400/401/2000 | Each actual submitted transfer reaches terminal counts matching independent native rows and its uniquely scoped job. Evidence: artifacts/db-copy-paste-transfer-recovery-20261001/v60-boundaries/. Larger25,000-row/50MB terminal25,000/0 after339.3sec is separate API/native evidence, not an Android pass. |
| TRANSFER-13 | FAIL (API/DB): changed source produced302wrong rows | Stable single-SELECT spool before writes | PASS actual v61 source-mutation UI/job/native lifecycle | Actual401-row transfer starts, then a peer deletes100rows, updates301 and inserts999. Same job finishes with original401, row301=before and no999; source becomes302. Evidence: transfer-recovery/v61-lifecycle. A separate progress-display defect found in this run is tracked underTRANSFER15, not hidden by correct stored data. |
| TRANSFER-14 | PASS (API/DB) | Correct harness scrolling only | PASS actual v60 UI49/50/51/1000 totals and49/50/50/50detail rows | Complete/capped details inspected through actual overlapping scrolls; native existing rows unchanged. First49-case fling observer missed5rows, then SAME job reobserved all49 without resubmission. Evidence: transfer-recovery v60-boundaries-reobserve-49 and v60-boundaries-tail. Root reviewed1000-failure last-details screenshot and exact1000warning; no success misreported. |
| TRANSFER-15 | FAIL: all-row failure said succeeded; Jobs reused scroll offset; slow copying showed metadata75% | Truthful status/checkpoints; reset new Jobs to top; indeterminate preparation plus acknowledged-row progress | PASS actual UI/native job states/progress; continuation warning UX tracked separately | Earlier v50 visibility retest remains. Fresh v61-progress-full-retest records101monotonic server checkpoints0–100, actual early/middle/terminal UI and exact401native rows. Root reviewed53%/212copied screenshot; earlier XML49%/197was separate. Seven error routes and same-job crash recovery pass. v63 retained original6/2job and new3/1Boolean job show Partial in card/full details; clean8/0job shows Succeeded, each immutable ID verified. False DEFAULT_SKIPPED for existing-table copy is a separate newly confirmed warning defect undergoing focused retest in cycle B. Raw server lifecycle is unchanged; queued remains a log message, not an enum. |
| TRANSFER-16 | FAIL server reconnect: stale running job | Detect interrupted worker and warn unknown additional writes | PASS actual Android navigate/background/restart/reconnect plus server crash recovery | v61-lifecycle retains same401-row job across navigation/background/app restart and finishes exact original snapshot despite source mutations. v61-crash reconnects to same auxiliary job after server restart: Failed/TRANSFER_INTERRUPTED, saved19versus native20, explicit uncertainty and no resubmission. Evidence under artifacts/db-copy-paste-transfer-recovery-20261001/. |
| TRANSFER-17 | FAIL: surviving table absent from result | Persist createdTable and partial result before later failure | PASS actual v61 partial-creation UI/jobs/native | Schema creation succeeds but generated-only mapping fails; UI visibly reports Created table:yes/copied0 and full NO_COMPATIBLE_COLUMNS reason. Native destination exists empty. Evidence: transfer-recovery/v61-errors/error-cases.jsonl case103; screenshot reviewed. |
| TRANSFER-18 | FAIL malformed portable data silently accepted/lost | Strict structure/PK/binary validation; type-aware transfer literals | PASS17actual v61 SAF variants plus exact PostgreSQL numeric import/export | Actual DocumentsUI imports validate data/schema/binary/ordinaryJSON-marker controls; malformed structures/PK/binary reject readably with no table creation. Additional PG import and actual ExportJSON→SAF save preserve long decimal/nested large number/Boolean/string/null types. Native NUMERIC exports exact decimal text with numeric metadata, not forced floating point. Evidence: transfer-recovery/v61-portable, v61-portable-numeric and v61-portable-numeric-export-retest. Export-only resume after navigation-observer stop never reimported data. Saved3079bytes SHA b3680f8e74cb1f5fea2b2ccda7737fca250560b2e0676f3742de8f45899d6e3a. |
| TRANSFER-19 | FAIL visibility; actual v63 SQLite TEXT-PK NOTNULL loss | General/upfront portability warnings; explicit NOTNULL retained in builder | PASS historical detailed schema controls plus fresh v63 schema-only/schema+data UI/native constraint retests | Historical fixtures: transfer-recovery/v60-context case304 and transfer-fidelity-recovery pg-j_jc8mt6,mariadb-oh6cimrp,mysql-03dgkwm4 (14checks each). New actual first-oct02c schema-only lost explicit NOTNULL; source rejectedNULL and copied target acceptedNULL, both probes rolled back. After fix, fresh retest-oct02d schema-only0rows/schema+data1row preserve constraint flags and rejectNULL with rows unchanged. Both retain genuine portability/default warnings; failed target preserved. See adjacent schema constraint cycle. Defaults/indexes/FK/triggers/comments/generated behavior remain disclosed limitations, not full-fidelity cloning. |
| TRANSFER-20 | FAIL4/48 initial genuine-engine/mode variants; continuation typed corruption and false warnings | MySQL catalog decoding, narrow charset-aware driver fix, Boolean/JSON conversions and schema-only default-warning gate | PASS48 historical basic mode combinations plus72scenarios;16additional v63 typed data comparisons and fresh warning retest | Historical16pairs×3modes use genuinePG/SQLite/MariaDB/MySQL8.4; basic schema is INTEGER/TEXT/PK. Continuation adds16typed pairs×8rows×10fields (128rows/1280native comparisons), unchanged sources/both schemas. Four original MariaDB-source jobs retain their historical false warnings; fresh MariaDB→PG first-oct02c and final retest-oct02d jobs now report8/0/no warnings with80exact fields each. Real schema-loss warnings remain in both final schema-copy modes. Not all historical variants rerun onv63; no exhaustive native-type/fault cross-product or full-fidelity schema claim. |
| ROBUST-01 | PASS (API/DB) | N/A | PASS actual deleted/missing destination, unreachable host, invalid role/password, expired DB password and app-session variants | Earlier deletedalias25 and expired/revoked-session controls preserve native rows. New v61 actual transfer failures identify ETIMEDOUT, invalidrole, expiredMySQLpassword and wrongpassword; no target created. Evidence: transfer-recovery/v61-errors/error-cases.jsonl cases104–107 plus ENV auth/deleted-destination report. Disposablealias18 was separately removed by CLIP16; original database remains intact. |
| ROBUST-02 | PASS (API/DB) | N/A | PASS Android source SELECT, target INSERT, table/schema CREATE denial and read-only controls | INSERT-denied/read-only imports report0/1 unchanged; schemaCREATE denial leaves namespace absent. v61 sourceSELECT/tableCREATE produce failed jobs/readable reasons with no target. v62 SELECT-onlyrole cell details offer no mutationcontrols and preserve exactrow; no deniedSave was attempted or credited. Evidence: transfer-recovery/v61-errors cases101/102, ENVv54 controls and generated-recovery/update-denied-bc4jttqc. Continuation: post-open UPDATE revocation passes exact v63 UI/native retest with truthful403. Supporting native regression also verifies authorized DELETE-only roles and stale-delete rejection before trigger side effects; it is not another Android role test. |
| ROBUST-03 | PASS (API/DB) | N/A | PASS actual authenticated Android request isolation (v56) | Own query first succeeds; exact later UI request receives scoped proxy connection-ID substitution to another principal's inaccessible connection and is rejected404. Readable non-retryable reason, no private row leak, both native fixtures unchanged. Evidence: artifacts/db-copy-paste-auth-recovery-20261001/isolation-ugab5lzz/. This tests request authorization, not all cross-server draft persistence. |
| ROBUST-04 | PASS (API/DB) | N/A for tested accounting | PASS actual confirmation-timed rename and during-import drop (UI/native v54) | Rename after confirmation: full original-target404 reason readable, old table absent and renamed table empty (`v54env1909-rename-at3mdrkk`). Drop during40-row import: UI reports2inserted/38failed, matching2persisted audit entries; remaining row indices/reason visible (`v54env1912-drop-q29hst7j`). Only fresh owned fixtures mutated; the explicit DROP removes the destination, not its separate audit. Earlier API50-row variant remains supporting evidence. |
| ROBUST-05 | PASS tested database faults (API/DB) | N/A for tested variants | PASS Android lock-timeout/deadlock imports, bounded-role query exhaustion and both bounded MEMORY-full imports | v54 timeout/deadlock:0inserted/1failed, useful reason/native0. Exhaustion limits only a fresh role to1connection; actual query fails clearly, not a claimed Paste submission. v55 MariaDB `memory-mariadb-mwzto86d`:7inserted/93failed; MySQL `memory-mysql-wc3xbjcc`:10/90; exact native rows match. MySQL actual scroll reaches78additional-details notice and fixedOK. All screenshots reviewed. At the earlier baseline checkpoint filesystem ENOSPC was not yet covered; the later bounded continuation is recorded below. Continuation: real private16MiB tmpfs ENOSPC/recovery passes on v62: one import5/35, exact native hashes/integrity, free owned filler, one recovery1/0 with prior rows unchanged. Initial generated-ID observer error was corrected by reobserving the same operation, not resubmitting. No physical storage-device failure or host disk filling (cycle F). |
| ROBUST-06 | FAIL timeout/unknown accounting; later v53 retry guidance visually inaccessible | Immediate durable import-warning dialog plus reopenable full error; unknown counts remain unknown | PASS v54 actual before/during/after-commit UI/native/visual variants | Before:0native/notforwarded; during:40native commits despite dropped response; after:1native commit/no response at1.5font. Each shows full original target/retry guidance/reason, no invented counts and reopenable details. Safe explicit retry only after verified-empty before-case inserts1row. Evidence in `artifacts/dbqa-import-error-readability-20261001.md`; offscreen-footer/result pre-submit harness stops preserved separately. |
| ROBUST-07 | FAIL (API/DB): restart left job running forever | Fail stale job with explicit unknown-additional-writes warning | PASS actual v61 UI/server-crash/native recovery | Only owned auxiliary8929PID682876 was killed; same-config restartPID686410 returns same job dbjob_1cebefef9c9a4a45aec2f8b290068511 as Failed/TRANSFER_INTERRUPTED/retryablefalse. Saved19rows≤stable20native rows; explicit additional-writes warning readable, no duplicates/resubmission. Evidence: transfer-recovery/v61-crash/crash.json. Production8100/8787 untouched. |
| ROBUST-08 | PASS (API/DB) | N/A | PASS Android UI plus explicitly identified concurrent API peer (v54) | Actual UI confirms100rows and reports100inserted/0failed; peer starts after the first commit while that import is running and reports0inserted/100UNIQUE failures. Native database contains exactly100matching rows. This is one Android client plus one API client, not two emulators. Evidence: `artifacts/db-copy-paste-robust-recovery-20261001/v54env1906-concurrent-62_gb9by/`. |
| ROBUST-09 | FAIL (UI): app crash on six large rows | Off-thread encoding; safe clipboard helper/file fallback; retain internal snapshot | PASS rebuilt v60 exact Copy→Cancel and Inspector→Save retest | Actual6-row/4-column snapshot survives Cancel with Android sentinel unchanged. SAF Save produces3,001,055bytes; every native field and all ASCII/emoji499999/500000/500001-byte values match. PID28751 stays stable and inspector usable. Saved-file SHA d2637948f5b9abb8434651a825ed9cb5e1a95ee2f20e1a3044851a03c18bfdb4; .2a4j4o9h in artifacts/dbqa-env-keyboard-final-smoke-20261001.md. Root independently reviewed retained inspector screenshot. |
| ROBUST-10 | Earlier resource-blocked; October3 complete-body boundary tests PASS API transport | No product fix; correct insufficient auxiliary memory cap and failure evidence capture | PASS L-1/L/L+1 complete sent bodies; disabled variant N/A | FixedL=1,048,576,000bytes. L-1/L parse fully and reach expected400 INVALID_CLIPBOARD; L+1 returns413. Fresh isolated8936, bounded streaming client, zero connections. First1.5GiB-cap exact-limit OOM is retained as resource failure; fresh4GiB retest has zero OOM events. No disabled-limit setting and no1GB Android rendering claim. |
| ROBUST-11 | PASS bounded Android stress (UI/native DB) | N/A for executed workloads | PASS3000rows/400columns/350KBcell | Exact complete native values and hashes, usable confirmation/results, successful dismiss, no process restart. Observed parse snapshots2.18–2.37s include ADB overhead, not uninterrupted frame-time proof; arbitrary larger limits are not implied. |
| ROBUST-12 | PASS server accounting; Android dialog now executed | N/A for accurate counts | PASS3000failure UI/native workload | UI reports0inserted/3000failed,12visible details plus2988additional; actual swipe and dismiss work, native0writes, PID stable. PSS160288→139088KiB for this run; not an exhaustive leak/performance benchmark. |
| ROBUST-13 | PASS actual12-cycle Android/native test (v51) | N/A for tested bounded lifecycle | PASS12 intended submissions, exactly12 persisted rows, stable process | Each cycle opens/closes panels, copies, confirms and pastes with fresh state assertions; periodic clear also passes. No duplicate callback/import observed. Evidence: root-recovery `.gar6vzcj/manual-paste-evidence.jsonl`, completed18:12:17. Bounded interaction test, not an unbounded heap-leak proof. |
| ROBUST-14 | FAIL MySQL/Maria duplicate message echoed private literal | Redact value, preserve constraint and ER_DUP_ENTRY; safe encoded literals | PASS API and Android UI/native four-engine redaction (v56) | Actual PostgreSQL, SQLite, MariaDB and genuine MySQL8.4 duplicate imports each report0/1, keep the seeded row unchanged and preserve useful constraint/target detail without echoing private test values. All result screenshots reviewed; root independently reviewed MySQL. Exact evidence directories are in artifacts/dbqa-env-robust-tail-20261001.md. Authorized query data behind the modal is not part of the error-disclosure assertion. |

## Test cycles and evidence

### Final v62 manual closure

- [x] SQLite/MariaDB/genuineMySQL generated imports:2actual UI rows each,
  exact generatedIDs/computed/default fields, followed by separately labelled
  native next-ID3probes. Every confirmation/result retained and visually reviewed.
- [x] Fresh PostgreSQL SELECT-onlyrole has no cell mutation controls in actual
  details; source unchanged. No deniedSave was attempted, so no falseCELL10
  denied-Save credit. `generated-recovery/update-denied-bc4jttqc/`.
- [x] Actual four-copy smoke on pinnedv62: single1; reverse3,2,1→1,2,3;
  random3,5,1→1,3,5; selectall50of60. Exact complete nine-field native equality,
  fresh timestamps/source metadata and unchanged source after every copy.
  `artifacts/db-copy-paste-selection-final-20261001/selection-lrgo1e2c/`.
- [x] Source/artifact audit verifies only compiled jobCard spacing and manifest
  version changed sincev61; no post-compilation source edits. Both ABI pins
  match isolated outputs. `artifacts/dbqa-v62-source-artifact-release-check-20261001.md`.
- [x] Emulator5582 logs contain no recordedv62crash/ANR; both QA clipboards
  cleared through UI and independently verified empty. Original size/font
  restored, idle on direct8928; no jobs/database/evidence deleted.
  `transfer-recovery/v62-final-hygiene/hygiene.json`.
- [x] Emulator5584 final handback at00:05Jakarta,2026-10-02: v62/direct8928,
  QA PostgreSQL Explorer idle with no modal,0observed runningjobs, bothclipboards
  independently empty, font1.0/720×1600/density320/nooverrides. Current app PID
  crashbuffer empty; historical devicewide UIAutomator errors are retained
  separately, not misreported as app crashes. No active ENV runner/holder.
  `artifacts/dbqa-env-final-tails-20261001.md` and
  `selection-final/env-final-idle-sy3967pm/`. Fixtures/evidence preserved.
- [x] Final ARM64 upload succeeds; service size/MD5 match the pinned file and
  public Gofile page returnsHTTP200. See Delivery above. Production services
  remain untouched; isolated backend changes are not a production deployment.

### Slow transfer progress — actual failure, regression, server fix

- [x] Actual v61 slow401-row lifecycle test shows75% Loading target table
  metadata while native rows are already committed. Same job survives Android
  restart and finishes with401exact snapshot rows; no resubmission. Evidence:
  `artifacts/db-copy-paste-transfer-recovery-20261001/v61-lifecycle/`;
  job`dbjob_434d7347281a4bc8a73c504616cda333`.
- [x] Added regression fails before the fix. All persisted450-row callbacks
  show percentage sequence `[0,0,0,0,25,25,50,50,50,75,75,44,89,100]`.
  Manual75→50for401rows was code-derived, not two captured UI frames.
  First evidence: `artifacts/dbqa-transfer-progress-first-20261001.md`.
- [x] Server fix separates indeterminate preparation from actual row progress,
  publishes copy0before the first write, and checkpoints only acknowledged
  outcomes at firstrow/500ms/batch boundaries. Result+progress persist together.
  Copy percentage stays at most99until every row is processed; empty total stays0.
  Generic file-job rounding and interrupted-write uncertainty remain unchanged.
- [x] Focused first regression retest passes. Six new regressions cover real401
  row native counts, mixed failure indices across batches, empty/schema-only
  jobs,500ms boundaries and premature100% rounding.
- [x] Full71Rust database tests pass with PostgreSQL55447 and genuineMySQL53308
  explicitly enabled; separate seven MySQL-family tests pass on MariaDB53307.
  Both diff checks pass. No Android source/APK change in this server-only fix.
- [x] Rebuild and restart only isolated8928 at both owners' acknowledged idle
  boundary. Actual running executable SHA256:
  `838f92df68705d4c4ed16a2535ce8ce87419b0016e9d649b6262f8eeebe737d2`.
- [x] Fresh uninterrupted actual UI retest: job
  `dbjob_16d3cf11cd364892976afdffc2d03000`,101sampled checkpoints0–100,
  early/middle/terminal screenshots and exact401native rows/source unchanged.
  Evidence: `artifacts/db-copy-paste-transfer-recovery-20261001/v61-progress-full-retest/`.
  Root visually reviewed the53%/212copied screenshot; its earlier XML snapshot
  was49%/197and is not claimed to be simultaneous. Two earlier observer-only
  stops (native-before-job timing and TEXT-ID oracle) retain their evidence;
  their already submitted job was completed/reobserved, never resubmitted.
- [x] Isolated8929crash test retains same job after restart and accurately warns
  saved19is only a checkpoint; independent stable native count is20. No retry
  or duplicate submission. Actual failed-state screenshot reviewed by root.
- [x] Minor adjacent UI10 defect found in that screenshot: long job title
  touches the Failed status. Add8dp logical-start spacing, rebuildv62 and
  independently verify installedSHA. Existing Succeeded/Failed jobs have16px
  gaps at720×1600/font1.0 and640×1280/font1.6. Root reviewed narrowFailed
  screenshot; titles wrap and statuses remain separate. Exact original size
  and font restored; no transfer/import submitted. Evidence:
  `artifacts/db-copy-paste-transfer-recovery-20261001/v62-status-gap/status-gap.json`.

### Completed typed row routes — v61 CSV closure

- [x] Internal rows:27groups/107variants; externalJSON:30groups/111variants;
  CSV:27groups/105variants. Total84batches/323route variants with actual Android
  confirmation/results and independent typed PostgreSQL equality. Exact source
  versions remain in each record; this does not claim all were run on v61.
- [x] CSV first12groups: DATA20261001-223935-rows-f3x4xbnu; next14groups:
  DATA20261001-225227-rows-drrposao; final effects group:
  DATA20261001-230630-rows-ivew0phi. Two read-only ADB255 scope-observation stops
  happened in PREPARING before Paste/Import; native target/audit states were
  verified before resuming only unfinished groups. No reset or repeated import.
- [x] Final CSV effects UI reports2inserted/0failed, ignores supplied id/computed
  fields, and preserves native generated/default/trigger values. The follow-up
  native DEFAULTVALUES insert yieldingID3 is separately labelled, not UI credit.
- [x] Root reviewed the oracle implementation: exact row count plus each field
  compared to independently typed SQL expressions; JSON comparisons occur in
  PostgreSQL, not Python's permissive Boolean/integer equality.
- [x] Complete index: `artifacts/db-copy-paste-data-20261001/row-evidence-crosswalk.md`.
  CSV intentionally has no SQLNULL tag; null-specific vectors are excluded or
  explicitly interpreted as empty text. CSV binary stress is128KiB; internal
  andJSON stress is256KiB. This is not four-engine typed Android coverage.
- [x] Additional final-state audit passes84/84 typed native assertions inside
  database-enforced READONLY transactions, including documented effectsID3
  probes/audits and the fresh v60 raw-decimal target. Evidence:
  `artifacts/db-copy-paste-data-20261001/final-row-native-20261001-230812-_p_umvxg/result.json`.
  This independently verifies retained data; it is not new Android coverage.
- [ ] Finish the remaining definition/generated/permission/transfer queues,
  then final artifact delivery. Typed-route completion alone is not all163cases.

### Empty single-column CSV/TSV record loss — first failure and fix

- [x] Actual v60 two-cell range contains SQLNULL and empty text; CopyCSV
  publishes `note\n\n`. Independent csv.reader yields header plus a zero-field
  record, not both selected rows. No database mutation. First failure:
  DATA20261001-221927-range-empty-observation-_aidepmr (and interrupted
  221638-range-mj2ugi1f). Prior saved-row clipboard remains intact.
- [x] Add exact range CSV+TSV and whole-result CSV leading/interior/trailing
  empty-row regressions. First run:9tests,2failures, matching both serializers.
  Failing XML retained in artifacts/db-copy-paste-empty-csv-first-20261001/.
- [x] Quote empty fields in both exporters. This preserves row count; it does
  not invent a SQLNULL tag or claim CSV distinguishes NULL from empty text.
- [x] Complete v61 full515-test shared suite, mobileCheck and debug assembly;
  both installed x86 APKs independently match the pinned new hash. No Rust
  change/restart required.
- [x] Exact actual v61 CopyCSV and CopyTSV retests each preserve both empty
  selected records, native source unchanged. Evidence:
  DATA20261001-222708-range-_nw_wpp9/manual-ui-evidence.jsonl.
- [x] Adjacent actual ExportCSV→DocumentsUI Save retains both empty records:
  exact file `note\n""\n""`, native source unchanged. Evidence:
  DATA20261001-223009-empty-csv-export-vtyeepd1. An earlier read-only ADB255
  interruption occurred before Save; the same pending export was resumed,
  not submitted twice.
- [x] Remaining six Boolean/numeric/nestedJSON×CSV/TSV controls and three
  query-invalidation controls pass on v61, with root visual review of the
  nested-JSON selection. Evidence: DATA20261001-223102-range-zwsjig99.
- [ ] Finish typed CSV row imports and the other remaining manual queues.

### Populated no-selection shortcut — stopped observation and same-build controls

- [x] Lifecycle run `20261001-215505-lifecycle-kha86cwv` passed first/last/both
  row copies, then stopped because the expected no-selection toast was not
  captured. Preserve screenshot/XML; source and both targets were still
  untouched. This is a failed observation, not a proven data or dispatch bug.
- [x] With the same v60 build, actual Ctrl+C with100ms and then default0ms
  injection each shows Select at least one row to copy and preserves both
  system and saved clipboards. Evidence: DATA resumed-ui
  `215856-no-selection-probe-myvoyp_j` and `215919-no-selection-probe-8uhm1cbu`
  (20261001 prefixes).
- [x] Review Activity/Data dispatch and recorder: empty selection requests the
  warning synchronously. Focused Android log says Toast already killed at the
  first attempt, but no quota/rate-limit evidence establishes a cause. Do not
  relabel the original miss as a confirmed platform or application defect.
- [x] Resume only after the completed copy controls with pristine-source and
  empty-target guards. No successful copy/import was silently repeated and no
  speculative product patch was made.
- [x] Finish the one-time snapshot/persistence lifecycle tail through its
  validated resume boundary; source-update retention, internal-only Paste,
  B replacement, explicit precedence, force-stop retention and Clear→restart
  all pass. A transient Loading observer stop was preserved and corrected
  only in the harness; successful prior imports were not repeated.

### Raw JSON number rounding — first failure, isolated fix and rebuilt retest

- [x] v59 actual Android JSON import claimed1inserted/0failed, but native
  NUMERIC(38,20) stored `12345678901234568.00000000000000000000` instead of
  input `12345678901234567.12345678901234567890`. Original target retained;
  no reset or retry. Root reviewed the misleading-success screenshot.
  Evidence: DATA resumed-ui `203932-rows-uxz6j9fa/iowb-data-row-ui-irk8z9sk/`
  (`20261001-` prefix), `raw_decimal-json` artifacts.
- [x] Isolate server with identical raw HTTP digits into a fresh owned target:
  exact native value retained. No Rust fix indicated. Detailed trace:
  `artifacts/dbqa-raw-decimal-wire-investigation-20261001.md`.
- [x] Reproduce client rounding in a failing actual Ktor wire regression;
  failure XML preserved in `artifacts/db-copy-paste-numeric-wire-first-20261001/`.
- [x] Use a database-scoped exact-number serializer rather than Double coercion.
  Recursive numeric literals are grammar-checked; quoted strings, Boolean,
  JSON/SQL null and binary markers keep their types. Row/key/original-value,
  snapshot/persistence, copy export and portable payload paths are covered.
- [x] v60 passes512 shared tests, mobileCheck and assembly. Exact x86/ARM64
  APKs and XML pinned in `artifacts/db-copy-paste-v60-20261001/`.
- [x] Exact v60 Android retest passes into fresh
  `qa_ui.ui_dr_pg_v60_raw_decimal_json`: native value retains every original
  digit, actual UI reports1/0 for the correct target. Evidence:
  DATA `20261001-210455-rows-2ha_bcf0/iowb-data-row-ui-m8sypmq0/evidence.jsonl`.
  Two prior navigation-only stops (`205830-rows-5o153fxu`,
  `210221-rows-v0nm3fwu`) made no submission; schema refresh and leaving the
  large schema-details panel restored exact table navigation. Destination
  remained empty before this first actual retest import.
- [x] Complete the remaining JSON groups:30/30 groups and111/111 variants
  pass across their archived actual APK identities, including the fresh v60
  precision retest. Each rejection also proves unchanged native data.
- [ ] Finish remaining internal/CSV variants and lifecycle coverage.

### POINT editor unchanged-save failure — first test and fix

- [x] Actual v58 Edit value opens a POINT snapshot as numeric JSON x/y.
  Pressing Save without changing it produces HTTP500 invalid point input;
  independent native value and full-row MD5 remain unchanged. Evidence:
  DATA `202934-point-edit-qfwe5kbl/001-PREFILL_OBSERVED` and `002-FAIL_UI`.
- [x] Preserve the numeric x/y object in the shared type-aware editor parser,
  while retaining native `(x,y)` text input. Reject missing/extra/nonnumeric
  object coordinates locally. TEXT and JSON lookalikes keep their original
  semantics. Cell and full-row editors share this parser.
- [x] v59 shared506-test suite, mobileCheck and debug assembly pass; exact
  x86/ARM64 artifacts and suite XML pinned in db-copy-paste-v59-20261001.
- [x] Actual v59 unchanged-save retest: editor closes safely; POINT and full-row
  MD5 unchanged. Changed numeric-object and native `(x,y)` controls each save
  the exact expected point and preserve every other field. Malformed object
  stays in the editor with a readable numeric-coordinate reason and unchanged
  full-row MD5. Root independently reviewed its rejection screenshot.
  Evidence under DATA resumed-ui: `204051-point-edit-vx0b_ed7`,
  `204139-point-edit-k1d8i6cj`, `204234-point-edit-8a044zzv`,
  `204345-point-edit-7ovwyjms` (all prefixed `20261001-`).

### Explicit live MySQL/MariaDB definition regression — new failures

- [x] Enable the MySQL live branches against genuine disposable MySQL8.4:
  seven tests run, four pass, two server metadata defects plus one fixture
  reserved-word syntax error reproduced. This is not Android UI credit.
- [x] Run the same enabled branches against MariaDB: six pass; trigger
  metadata decoding also fails there (Datetime instead of Timestamp).
- [x] Fix both application failures and repair the fixture quoting. Full65
  database tests pass with PostgreSQL55447 and MySQL53308 explicitly enabled;
  seven MySQL-family tests also pass against MariaDB53307. CLI rebuilt.
- [ ] Complete Android definition tails against the corrected server.
- [x] At both device owners' confirmed completed-case boundaries, restart only
  the owned QA8928 unit. Health verifies the same fixed-config/workspace;
  PID376284, binarySHA
  `7ee1f4eff483b59cd17142e4daefee3eb583f6105c9a7f3fa8a1d0a5a2a5fb40`.
  Neither8100 nor8787 was restarted. Android preview/definition retests use this
  new server; earlier records retain their original server/build identity.

Evidence: `artifacts/dbqa-live-mysql-definition-first-20261001.md`. Earlier
PostgreSQL-enabled64-test runs skipped these engine-gated branches. The table
and trigger definition claims remain partial until these retests complete.

### Cell matrix completion and binary-preview correction — 20:00 checkpoint

- [x] Complete all 86 unique cell-replacement vectors through Android UI and
  independent PostgreSQL value/unchanged-value checks. The read-only evidence
  crosswalk confirms 86/86, not 86 checklist cases; earlier failures and their
  exact retests remain preserved. See
  `artifacts/db-copy-paste-data-20261001/cell-evidence-crosswalk.md`.
- [x] Finish valid zero-byte/binary controls, including the 262,144-byte scalar
  and nested/empty/invalid binary arrays; no truncation or freeze observed.
- [x] Independently inspect the large-binary result screenshot. The saved bytes
  are correct, but its grid preview incorrectly says `{ 3 properties } · JSON`.
  First observation: DATA `194515-cells-kjj0trzy/019-PASS_UI.png`; the original
  integrity pass is not changed into a data-loss failure.
- [x] Add a display-only, column-type-aware binary summary to table/card cells.
  Copying and editing still receive the complete original value. A JSON object
  resembling a binary transport marker must not be relabelled as binary.
- [x] Complete new preview regressions and rebuild v58:504 shared tests pass.
  The adjacent native POINT/SQL-array labels are also type-aware; real JSON
  remains JSON and missing metadata uses neutral Object/Array labels.
- [x] Strict v59 visible-screen retest:16controls, Data/SQL × table/cards ×
  binary/POINT/native-array/JSON-lookalike, preserve full native row MD5.
  Actual visible summary bounds are checked, not offscreen hierarchy text or
  matching query-editor words. DATA reviewed actual SQL JSON-card screenshot;
  evidence `20261001-204439-preview-rbyd315u` under resumed-ui.
- [x] First v58 UI retest exposes an additional server metadata issue: BYTEA
  preview is `{ 3 properties } · 17` because PostgreSQL query columns used raw
  OIDs as nativeType. No data changed. Preserved evidence: DATA
  `201144-preview-q9a9_k4n/first-failure.md` and interrupted PNG/XML.
- [x] Return PostgreSQL type names, resolving custom types from described
  metadata only when column ordinal/name/OID match. New live regression checks
  BYTEA, INT8[], POINT, JSONB and custom enum in populated and empty results.
  Full explicitly enabled65-test run passes; coordinated QA8928 restart and
  all16actual Android controls above now pass.
- [x] First internal-row group passes13 text/Unicode/escaping variants with
  exact Copy snapshot and native target checks. Evidence:
  `artifacts/db-copy-paste-data-20261001/resumed-ui/20261001-200538-rows-wslgkp3m/iowb-data-row-ui-kd3fv_3x/evidence.jsonl`.
  These13 rows must not be reset or re-imported for a cleaner transcript.
- [ ] Complete the separate internal-row, JSON-row, CSV-row and lifecycle
  matrices. Cell coverage does not substitute for any of these routes.

### v54 numeric/Boolean validation divergence — fix awaiting rebuilt retest

- [x] Actual NaN first failure preserved: supported PostgreSQL NUMERIC NaN
  remains rejected locally with "Enter a valid number"; native row unchanged.
- [x] Actual Boolean0 first failure preserved: checkbox correctly appears
  false, but local Save rejects the retained clipboard text. No mutation.
- [x] Replace duplicate Android number/Boolean validation with the shared
  parser. Exact numeric strings retain arbitrary precision, integer syntax
  still rejects fractions, and native nonfinite values reach database-specific
  capability/range checks. Strict JSON and date/time feedback stay unchanged.
- [x] Add lexical-regression methods and correct the prior test fixture that
  incorrectly used fractional input to represent a BIGINT precision check.
- [x] Finish v55 shared/mobile checks and assembly after the separately recorded
  test-recorder race repair;501tests green. Install and independently verify
  the pinned emulator APK on both devices at acknowledged idle boundaries.
- [x] Repeat exact numeric NaN and Boolean0 failures through actual UI/native
  readback; both pass. Boolean1 control also passes.
- [x] Finish11 affected numeric/invalid-Boolean regression controls on v56,
  with exact native values or rejected/unchanged storage. Evidence:
  DATA `195853-cells-h0ph_58b`, `200351-cells-g5fwdgq4`, `200444-cells-llmbol9x`.

Additional v55 UX observation: the safe-source guard correctly disables writes
for an unqualified MySQL/MariaDB query, but its explanation names only
"edits and deletes" and does not show the required qualified source. Source
wording now names Paste/edit/delete and the actual engine-quoted qualified
table. The safety predicate is unchanged. Rebuilt v56 passes the actual
blocked/qualified controls: correct quoted `qa`.`table` guidance, no Paste for
the unsafe query and enabled Paste for the qualified control. Both screenshots
were reviewed; no import was submitted. Evidence:
`artifacts/db-copy-paste-db-faults-recovery-20261001/v56-qualified-control-xhq6v868/`.

### v54 NUL-character rejection — first failure and fix in progress

- [x] Actual cell replacement containing U+0000 reaches Save once. PostgreSQL
  leaves the previous value unchanged, but the full visible error is only
  HTTP500 / "invalid message format". This is a useful-error failure, not data
  loss. Evidence: DATA resumed-ui `192005-cells-v_fzxlgv/nul-observation.md`
  and `nul-rejection-details.png/xml`.
- [x] Separate harness limitation: Android's XML dumper rejects U+0000 while
  the editor is open. Preserve its failure; inspect the actual screenshot for
  the one Save tap instead. The app itself does not crash.
- [x] Add PostgreSQL-only raw-NUL validation before generated row mutation
  and transfer statements are sent; return HTTP400 with explicit U+0000/NUL
  and binary-column guidance. Do not echo the submitted private value, reject
  encoded binary zero bytes, or change other database engines.
- [x] Initial two regression tests pass with live PostgreSQL explicitly
  enabled: scalar/text-array rejection keeps the row unchanged; encoded
  binary zeros still update correctly, and SQLite retains embedded NUL text.
- [x] Expanded transfer regression also passes with real PostgreSQL: NUL row
  rejected without insertion. CLI rebuilt. At a confirmed idle boundary,
  replace the already-stopped QA8928 only; health verifies the same config and
  workspace. New user service `dbqa-candidate-8928`, PID295999, binarySHA
  `a0d727c03627a4c91d29f975a94b0b192cb976253034efd15ee207c110025797`.
- [x] Exact NUL editor retest passes on v55/new server: full usefulHTTP400
  reason and unchanged native bytes. Actual rejection screenshot independently
  reviewed by DATA and root; one Save only. Evidence:
  `artifacts/db-copy-paste-data-20261001/resumed-ui/20261001-v55-nul-retest/`.
- [x] Finish adjacent valid binary-zero UI controls under the new server;
  cell crosswalk also records scalar256KiB and nested/empty/invalid arrays.

### Resumed v54 permission/concurrency checkpoint — 19:03 onward

- [x] Preserve current server8928 and emulator ownership after the interrupted
  turn; health identifies the existing disposable recovery config. No production
  service restart or database mutation.
- [x] Concurrent100-row Android import and independently labelled API peer:
  accurate100/0 and0/100 outcomes, exactly100 native rows.
- [x] Rename after confirmation and drop during import: original target and
  useful failure details remain visible; actual partial writes match the
  independent native audit.
- [x] INSERT denied and read-only transaction UI variants: useful complete
  reasons, accurate0/1 counts, native destinations unchanged.
- [x] Read-only delivery preflight of pinned v54 ARM64 artifact:
  10,173,254 bytes; MD5 `2319f08b88d9a69f2b334f8104176726`; exactSHA and actual
  AArch64 ELF library verified. No upload occurred; this is not final release
  selection, service verification or ARM64 execution.
- [ ] Finish the remaining fault/auth, typed cell/row, snapshot/range and
  transfer variants; retain all first failures and explicit harness exclusions.

### v53 → v54 persistent error-readability cycle — 18:43 onward

- [x] First cell constraint test rejects VARCHAR overflow correctly and leaves
  `12345678` unchanged, but its persistent header truncates the useful reason.
  Preserve the raw integrity PASS and separately record the visual defect.
- [x] Independent actual disconnect-before test shows unknown-outcome text only
  in an ellipsized header; critical check-before-retry guidance is inaccessible.
- [x] Add a visible44dp `Error · View details` control; it opens the captured
  complete message in a selectable, scrollable native dialog. Normal/loading
  status remains noninteractive and returns to its original compact height.
- [x] Show the uncertain-import warning immediately in a durable result dialog,
  retaining original target, reason and retry guidance without claiming counts.
  Dismissed warnings can be reopened from the header. No automatic retries.
- [x] Preserve cancellation and paste-gate `finally` cleanup; add two shared
  warning-summary regressions. v54 shared499 tests, mobileCheck and build pass.
- [x] Actual VARCHAR overflow retests at1.0 and1.5font show the complete reason
  and Close, with unchanged native value and44dp control; screenshots reviewed.
- [x] Actual disconnect-before warning is completely readable onv54; exact
  proxy proves no forwarding and independent storage confirms zero writes.
- [x] Reopen the full warning, then deliberately retry only after verified
  empty target and restored proxy. Exactly one row inserts and gate releases.
  Evidence: robust-recovery `v54env1851-disconnect-before-ii771umw`.
- [x] During-response loss commits40native rows; full warning and reopened
  details stay readable, no retry (`v54env1855-disconnect-during-2quvrtku`).
- [x] After-commit response loss commits1native row; full automatic warning and
  reopened details fit at1.5font (`v54env1903-disconnect-after-commit-5e_m4ev8`).
  Font restored1.0. Earlier offscreen-footer/result locator stops occurred before
  submission and are excluded as harness interruptions, not product failures.

Reports: `artifacts/dbqa-import-error-readability-20261001.md` and DATA
resumed-ui `185121-cells-hf1tnyy0` / `185235-cells-flxvfwu0`. The original
truncated-error screenshots remain intact; this does not pass all remaining
constraint, permission or error-redaction variants.

### v52 → v53 missing query-column metadata cycle — 18:28–18:40

- [x] First actual Data query failure: exact one-shot response removes three
  metadata columns but retains two real rows. UI incorrectly reports statement
  success and offers neither grid nor selection. Source remains unchanged.
- [x] Preserve first screenshot, transformed response, native rows and installed
  v52 checksum in root-recovery `.o3os7sqr/query-metadata-first-failure.json`.
- [x] Share the existing ordered-key fallback across Data/SQL grids, cards,
  Copy and CSV export. Actual metadata stays authoritative; no inferred types,
  primary keys, mutation targets or extra edit permissions.
- [x] Six new regressions cover missing/known metadata, case-distinct keys,
  empty results, affected-row-only statements and RETURNING row objects.
- [x] v53 full shared497 tests, `mobileCheck` and assembly pass; exact installed
  x86 checksums verified on both devices.
- [x] Actual Data and SQL table-mode Copy match all native returned values.
- [x] Actual DocumentsUI exports from both routes contain exactly the two
  expected CSV records,51bytes each (`.k67f3hqr`).
- [x] Actual Data and SQL card-mode Copy also match (`.qg65ga50`).
- [x] Fixed screenshots visually reviewed; source data unchanged throughout.

This closes the specific missing-query-metadata gap, not the unrelated pending
permission, lifecycle, type and transfer variants.

### v51 → v52 SQL draft recreation cycle — 18:14–18:25

- [x] Establish that the original Query3 was offscreen, not missing.
- [x] First discriminating UI test: set a non-default draft scope without Run,
  switch to Data, recreate by font scale, then return to Query3. SQL survives
  but qa_scope_draft/public/137 is incorrectly replaced by qa/qa_ui/250.
- [x] Fix persistence: a missing/partially detached SQL editor is not a cleared
  input. Capture only complete mounted controls; intentionally empty mounted
  fields retain their existing clear-to-null behavior.
- [x] Add five shared regressions; v52 full shared491 tests, `mobileCheck` and
  assembly pass. Pinned artifacts and XML totals are recorded above.
- [x] Install and independently checksum-verify v52 on both x86 emulators.
- [x] Actual inactive-editor retest retains exact scope and SQL after Data
  recreation (`scope-fgm4uwo1`).
- [x] Adjacent active-editor recreation also retains exact scope and SQL
  (`scope-639ebhf7`). Original draft and font restored; no SQL was executed.
- [x] Visually inspect the retained field values in the actual retest screenshot.

Evidence: `artifacts/db-copy-paste-sql-recreation-20261001/`. These are scoped
passes, not completion of remaining data/transfer/fault variants.

Execution details will be appended here as each case is exercised. No case has been passed from code inspection alone.

### Host-restart recovery after 16:19

The interruption cleared the host's entire `/tmp`, not just the Rust process.
The old temporary PostgreSQL cluster, SQLite file, server configuration,
emulator clone, unarchived helpers and pinnedv47 APK were lost. Repository source
and archived evidence survived; the stopped owned MariaDB/MySQL containers and
their persistent volumes survived too. Completed archived case results remain
valid for their recorded fixtures/artifacts; new fixtures will not be presented
as continuity of the lost database state.

- [x] Confirm candidate8928 is stopped and old temporary paths are absent; inspect surviving source/evidence before recovery.
- [x] Create persistent recovery root `/home/giofahreza/.local/state/io-workbench-dbqa/recovery-20261001.H78h6T`; map the absent old `/tmp/iowb-dbqa-qP7L4f` path to it for existing runners.
- [x] Clone only the original emulator's configuration/data/cache into a new owned persistent AVD; original AVD remains untouched. Repoint only the owned `iowb_db_copy_1001_5582` registration.
- [x] Boot recovered emulator5582, confirm no Workbench/helper package is installed, set720×1600/density320/font1.0. No user app data needed clearing this time.
- [x] Reconstruct new PostgreSQL/SQLite fixture sources and isolated configuration; restart only owned MariaDB/MySQL containers and candidate8928. Four genuine-engine source fixtures match native expectations; this is setup, not manual-case credit.
- [x] Restore archived/reconstruct missing QA helper dependencies into durable storage. Real foreground clipboard calibration preserves Unicode, CRLF and NUL exactly on the recovered device.
- [x] Build/pin v48 ARM64 and x86-64. Recovered emulator advertises only x86_64; ARM64 installation correctly rejects ABI mismatch. Install matching x86-64 build and independently verify device SHA-256 `1d87afbb964cd2bd635d0eb41c316e1303eecfac6dea0af68d1f8979ab89e753`. Pinned ARM64 deliverable SHA-256 `af3f0026ab6231c44cf87a85e29b2c17b69874ce1695398ad97564a3f39956d4`. Both are under `artifacts/db-copy-paste-v48-20261001/`; do not claim ARM64 ran on this emulator or that the lost v47 APK was reinstalled.
- [ ] Repeat the interrupted large-definition candidate retest, then resume remaining transfer/DATA/root tails.

Recovered ATD caveat: its default `debug.hwui.drawing_enabled=0` produced entirely
black screenshots despite functional UI hierarchies/gestures. Root diagnosed
this from device properties, set drawing to `true` on only emulator5582, and
restarted only Workbench. [Visible renderer control](artifacts/db-copy-paste-v48-20261001/renderer-enabled.png)
was visually inspected. Earlier recovered-v48 definition Cancel/Save native
checks can establish clipboard/file behavior, but cannot establish drawn-layout
performance or an ANR fix. Repeat those flows with rendering enabled before
crediting visual stability. The `.sql.txt` save-name mismatch was genuinely
observed; MIME correction and exact-name retest are pending.

Immediately before interruption the transfer agent observed large Definition
Copy→Cancel opening the actual file picker, preserving its sentinel and stable
app PID. That run's raw `/tmp` evidence was not archived before loss. It is an
observed interim result, not final sign-off; Save/full-view remain unconfirmed
and must be run again. No transfer-matrix UI case was completed in that window.

### Resumed SQL/inspector and verification cycle

- [x] Preserve two separate SQL wrong-row first failures and the inspector unsafe-query/wrong-target failures in the [row/inspector evidence archive](artifacts/db-copy-paste-row-inspector-20261001/report.md).
- [x] Build and pin isolated ARM64 v47: `8e18df888ceb0e49cc155a1bcd8b1ab4d6bd80fbb4bf029ce9175669e164de6d`.
- [x] Complete the archived v47 SQL/inspector retests and adjacent tab/header/0–1–50–51 preview cases recorded in the row-inspector report. Later uncovered query-metadata/no-destination variants remain separate pending cases.
- [x] Current-source shared JVM tests: 476 tests, zero failures/errors/skips; isolated `mobileCheck` passed. XML totals were read after this run, not copied from an earlier count.
- [x] Current-source Rust `database::` suite: 62 passed, zero failed.
- [x] Root and apps diff whitespace checks passed at this checkpoint.
- [ ] Re-run necessary final checks after subsequent fixes; this checkpoint is not final manual sign-off.
- [x] Prepare opt-in root UI tail runner for denied row clipboard publication, cell/internal precedence, continuously open inspector, real held Ctrl+C/V, editor shortcuts and 12 bounded lifecycle cycles. Preparation alone earns no manual passes.

The inspector first failures were observed on the checksum-verified v46 candidate. Their confirmations were cancelled, so no unintended database changes were permitted. Fixes preserve normal query destination guards rather than using a remembered Data table. Candidate8928 health is verified; real8100/8787 remain untouched.

### Root-owned UI tails (preparation and subsequent execution)

These scripts refuse device access without an explicit execution flag and pinned APK hash. They create only new disposable fixtures and preserve evidence on assertion failure. Preparation and compilation do not pass a case.

- [x] `artifacts/dbqa-root-final-ui.py denied`: denied publication and retained complete snapshot actually imported; root-recovery `.4dkylrz3` + `.g65jlq9a`. Intervening overlay harness interruption retained.
- [x] `cell-precedence`: exact saved-row import after cell Copy; `.l2py60wt`. Definition/external-JSON precedence remains separately assigned.
- [x] `inspector`: continuously open Copy JSON/external update/Clear; `.8_n7os0d`. No invented selection gesture while inspector is open.
- [x] `held`: actual20DOWN+UP events; one import/publication, then fresh shortcut works; `.vk5p4fhk`.
- [x] `text-fields`: exact native Ctrl+C/V draft with zero execution; `.cklrgvi8`. Earlier observer-focus harness interruption retained.
- [x] `html`: empty plain-text variant passes; `.fus6uteu`. Android rejects null plain text at construction; unsupported variant is not an app pass.
- [x] `invalid-wrappers`: all ten actual rejection variants, zero writes; `.lkp86gcx`.
- [x] `layout`: actual v51 narrow/large-font and landscape tests with subsequent screenshot review; `.6oq1ogzh/visual-review.md`. Spoken TalkBack remains untested.
- [x] `cycles`: all twelve completed18:12:17 with exact native accounting, stable process and periodic clearing; `.gar6vzcj`.
- [x] `artifacts/dbqa-file-failure-ui.py write-failure`: new provider audit pair, truthful failure, unchanged clipboard and exact normal-save retry; `.xc9z97f5`.
- [x] `missing-picker`: exact owned picker disabled/restored, truthful failure and exact normal-save retry; `.lvy7_887`. Read-only resolution independently confirmed picker restored.
- [x] `artifacts/dbqa-query-metadata-ui.py`: first v52 failure preserved; v53 actual Data and SQL table/cards Copy plus both DocumentsUI CSV exports pass with exact native fields and inferred columns. Evidence: `artifacts/dbqa-query-metadata-retest-20261001.md`.

Recovery checkpoint: root reran `cargo test -p iowb-server database::
--no-fail-fast -- --test-threads=1` after fixture recovery:62passed,0failed.
Root/apps whitespace checks also pass. These are supporting regression checks,
not completion of the pending Android variants or final delivery.

### Recovered v48 → v49 defect cycle

- [x] With drawing enabled, reproduce typed Data-cell full-view copying errors:
  JSON string gains another encoding layer; JSON null becomes a string; SQL NULL
  becomes lowercase null with an inaccurate ambiguity label. Separate SQL-result
  controls pass, demonstrating that the old behavior depends on column metadata.
  Raw evidence: `artifacts/db-copy-paste-data-20261001/resumed-ui/20261001-165045-additional-ch720lct/`.
- [x] Replace duplicate rendering/copy conversion with one lossless shared
  transport-value formatter; SQL NULL remains distinct. Add six regression tests.
- [x] Fix definition export MIME to `application/sql` for both Copy paths;
  ordinary cell text exports keep `text/plain`.
- [x] Isolated v49 `:shared:jvmTest mobileCheck :androidApp:assembleDebug` passed;
  actual XML totals482tests,0failures,0errors,0skips.
- [x] Pin v49 ARM64 SHA256 `40fbb038d0f02d0cbf5aa127ef8e292981c9df8c0d57d544629d25b04ccfebe4`
  and x86-64 SHA256 `3f08de00a1851c9bd0d0a8a0a9d79871d3726e6a92d46ef4801e727f374968df`
  under `artifacts/db-copy-paste-v49-20261001/`.
- [x] Retest all four typed Data full-value variants on checksum-verified v49:
  exact JSON string/object/JSON null/SQL NULL results, including the SQL NULL label.
  Evidence: `artifacts/db-copy-paste-data-20261001/resumed-ui/20261001-165729-additional-1aza1qn1/`.
- [x] Complete all four separate SQL regression controls on v49; full-value cycle
  is8/8 actual UI/native comparisons. [Full report](artifacts/db-copy-paste-data-20261001/full-value-ui-report-20261001.md).
- [x] Complete definition export filename/full-view retests with rendering enabled:
  direct and continuous-view Copy both save exact600,077-byte `.sql` files;
  tail search reaches the final characters. Availability/provider failure
  variants and the rest of TRANSFER remain separately tracked.

The first recovered denied-row-copy attempt on5584 is **inconclusive**, not a
new product failure: app-op rejection occurred and the system sentinel remained,
but the required toast was absent from the observer. Investigation found that
the recovered ATD image has no SystemUI (`SystemUI uid: -1`) and uses
`com.android.fakesystemapp`. No toast-outcome pass is inferred from that image.
The app-op was restored. Evidence directory: `db-copy-paste-root-recovery-20261001/db-root-row-clipboard-ui.c8mql19b/`.
An independent standard Google APIs QA emulator is being prepared for these
toast-dependent checks; other devices and real services remain untouched.

Standard5584 is now ready on v49 with real SystemUI and calibrated Workbench
toast64 events; setup evidence is under
`artifacts/db-copy-paste-v49-20261001/emulator5584-standard-setup/`.
Its first root denied-copy runner stopped **before Copy** because the rebuilt
navigation helper omitted its prior schema refresh after creating a fresh
fixture. Manual Ctrl+R made the new objects discoverable; this is a harness
navigation exclusion, not a clipboard/product failure. The untouched fixtures
and attempt record are retained in `db-root-row-clipboard-ui.t4po4wy7/`.
- [ ] Missing-query-metadata actual UI variant: narrowly scoped one-shot proxy; copied rows must use inferred keys and match native data.
- [ ] Final pinned-APK smoke, affected regressions, final build/test checks, and Gofile upload metadata verification.

### Large definition and interrupted-device recovery cycle

- [x] First actual large-definition test: opening Table details ANRs before Copy is reached. Captured main-thread native text-layout stack and Android exit information; host memory pressure is recorded as a contributing condition, not silently excluded.
- [x] Independent small-definition control: actual Copy matches native SQLite DDL byte-for-byte (87 bytes).
- [x] Fix: bound only displayed previews/default/extra summaries to2,048 characters without splitting surrogate pairs/CRLF; preserve complete source values. Large View full uses the existing continuous virtualized viewer.
- [x] Render-only retest: large details and its Copy action become reachable. A subsequent Copy attempt is interrupted by loss of the whole emulator; no clipboard result or app-specific crash is inferred.
- [x] Additional copy hardening: Definition Copy uses the same off-thread500,000-byte guard, file-save fallback and verified clipboard-publication helper as cells/rows.
- [x] Isolated v46 assembly and11 focused tests pass (4 preview +7 clipboard-publication tests). APK pinned before another build can replace it: `5af58b34a43d0c93e9a3d8ed1a83d524af34a19c6729571756dcde49c0c59098`,10,156,870 bytes. This is an intermediate candidate, not final sign-off.
- [x] Preserve first-failure/control/interrupted evidence in [definition UI archive](artifacts/db-copy-paste-definition-ui-20261001/) and the [explicit staged-result manifest](artifacts/dbqa-definition-staged-20261001.json).
- [ ] Final candidate actual large Copy → Cancel / Save → exact saved SQL; continuous full-view and small-copy regression.
- [ ] Document-provider write failure and missing-picker recovery.

The old read-only emulator process was lost. A separate writable clone, `iowb_db_copy_1001_5582`, now uses the same emulator5582/ADB5056 address. Only its copied app state was cleared to prevent auto-connecting the old template to a normal server; original emulator/user app data were not cleared. Test databases and archived evidence survived. A later task interruption also stopped the isolated QA services; candidate8928 and PostgreSQL55447 were restarted from their original disposable data directories. These recovery actions are infrastructure setup, not additional passing product tests. Standard APK output was concurrently replaced by an unrelated build, so subsequent artifacts use `artifacts/dbqa-isolate-android-build.gradle` and pinned checksums. Real8100/8787 remain outside failure-injection/restart scope.

### DATA evidence reconciliation (API/helper evidence is not Android completion)

The [DATA report](artifacts/db-copy-paste-data-20261001/DATA-01-16-report.md) records 953 non-CSV variant/groups and 331 actual shared-CSV-parser→HTTP records across PostgreSQL, SQLite, MariaDB and genuine MySQL. Its final saved-evidence audit checks 1,250 invariants with zero mismatches after explicitly documented CLI-charset and real-FK fixture corrections. Focused follow-ups add 240 JSON protocol comparisons, 48 scalar editor native comparisons, four actual metadata/read→editor-helper→HTTP binary checks and 34/34 JSON/JSONB/bytea-array grouped route comparisons. These counts overlap and are not added into a manual-case total.

Actual Android overlap is credited narrowly: FORMAT indices 20/23/32/33 cover specified CSV escaping/Chinese/empty-text variants; FORMAT37/38 plus native JSON predicates cover external encoded/native JSON scalar/null variants. Root's [selected unchanged UI/native records](artifacts/db-copy-paste-data-20261001/ui-data-evidence-extract.json) cover empty/literal-null text, JSON string/null/SQL NULL cell actions, typed-invalid-boolean rejection and query-cell NaN failure→13:32:11 retest. The NaN retest preserved the existing erroneous JSON string while rejecting a new invalid edit; it did not silently repair prior data.

The [remaining route/variant matrix](artifacts/db-copy-paste-data-20261001/UI-data-matrix.txt), new-table-only SQL fixtures and explicit-opt-in Python runners are preparation only. They have not been executed on the device by the data reviewer. Internal structured HTTP requests are not actual Android Copy→Paste evidence; 500 KB cell-copy/file-fallback checks are not large-value paste or absolute database maximum tests. No DATA row or all-163-case UI completion is inferred from helper tests or prepared runners.

### Mapping cycle (API/DB, then Android UI)

- [x] First test: each MAP-01–15 request sent to baseline server and stored rows read independently.
- [x] Reproduce wrong writes for MAP-05/06/07 in disposable tables.
- [x] Fix exact-priority mapping and reject ambiguous normalized matches.
- [x] Retest all 15 against candidate; unsafe variants now reject before writes.
- [x] Retest target rename/add/drop variants against fresh metadata.
- [x] Prepare second-batch PostgreSQL mirror: 18 API/native mapping variants pass, including separate User ID/USER-ID successes and identity/computed omission. These are fixture validations, not Android passes.
- [x] Exercise all18 PostgreSQL mapping variants through actual Android confirmations, including separately normalized names and confirmation-timed rename/add/drop.

Durable evidence: [mapping first test](artifacts/db-copy-paste-format-20261001/map-red.jsonl), [latest API mapping retest](artifacts/db-copy-paste-format-20261001/map-green2.jsonl), [latest API schema-change retest](artifacts/db-copy-paste-format-20261001/map-schema-green2.jsonl). MAP-12 intentionally supplies SQL NULL for an absent mapped field, not its default; the actual Android confirmation warning and exact native values now pass.

Historical second-batch preparation: [API/native mirror checks](artifacts/db-copy-paste-batch2-prep-20261001/api-fixture-validation.jsonl) and [runner prerequisites/pending variants](artifacts/db-copy-paste-batch2-prep-20261001/preparation-index-v2.txt) were prepared-only evidence. Subsequent Android execution is recorded separately; no UI result is inferred merely from scripts compiling.

Second-batch real Android execution began13:41: [earlier UI/native snapshot](artifacts/db-copy-paste-batch2-ui-20261001T1408.jsonl). All18 mapping variants pass, including actual confirmation-timed schema changes. PASTE12/13 defects and missing PASTE17 completion-target context are preserved as first failures; final-candidate Android retests now pass. Query harness mistakes have explicit exclusion/correction records and are not counted as product defects.

### Schema-copy fidelity cycle

- [x] API/DB test confirmed portable schema is not a full-fidelity clone.
- [x] Server warnings added to transfers and portable import/export.
- [x] Added an upfront Android schema-copy/import warning; captures transfer endpoints before confirmation.
- [ ] Build and manually verify warning and Cancel produce no table/job.

### Android format and external clipboard cycles

- [x] Baseline version 42: 14 real clipboard/Paste variants, including 13 parser failures and one valid-array control. Unsafe confirmations cancelled; destination remained empty.
- [x] Shared parsing fixes: strict JSON/CSV/TSV, lossless explicit empty records, reserved-field handling and copy value-format preservation; 45 targeted tests passed.
- [x] Version 43: 41 FORMAT variants (indices 0–40) passed through actual Android clipboard → Paste → confirmation/import or error toast. Each accepted map matched independently specified native PostgreSQL values; rejects left the destination unchanged.
- [x] SQL NULL and JSONB null/string/boolean/number verified independently using native SQL type predicates, not ambiguous JSON output alone.
- [x] CLIP-03/07/08/10 tested with real foreground clipboard helper. CLIP-06 external changes and CLIP-09 URI-only image/png variant pass with limitations retained in their rows above.
- [x] Final owned destination contained 46 rows: 45 from format variants and 1 first-item-only clipboard import. Both clipboards empty, Paste hidden, no modal, emulator released to root on qa_format grid.
- [ ] Actual cell “Copy row as JSON” → paste end-to-end gesture remains with CELL-03; direct bare-object/wrapper inputs are not substituted for this gesture.
- [x] URI-only image clipboard/provider audit follow-up: actual cell replacement retest records zero provider calls after reset; see root13:32:33 record in [refresh/retest evidence](artifacts/db-copy-paste-root-ui-refresh-retest-20261001.jsonl).
- [ ] HTML-only clipboard content remains unexecuted; unsupported fixture construction must not be recorded as an app pass.

Durable evidence: [format UI ledger](artifacts/db-copy-paste-format-20261001/manual-ui-evidence.jsonl), [clipboard UI ledger](artifacts/db-copy-paste-format-20261001/manual-clipboard-evidence.jsonl), [native SQL/JSON null-type assertions](artifacts/db-copy-paste-format-20261001/manual-jsonb-type-evidence.json), and [archive index](artifacts/db-copy-paste-format-20261001/archive-index.txt). Screenshots/XML and supporting fixtures/logs are archived beside these files. Use the latest fixed record per index: index 0 is corroborated by `fixed43-ui-first.log` and `ui-fixed-00-FORMAT-01`; indices 1–40 carry `buildTag: v43` and `ui-fixed-v43-*`. Earlier failed/inconclusive probes and explicit corrections are retained. Tested APK SHA-256: `38c4f631a99ea6bfadf2bec667d3eae937bc199dd57b0e5f679a772ca5d98b61`. [Final cleared state](artifacts/db-copy-paste-format-20261001/ui-v43-format-final-cleared.png).

### Intermediate build (not final sign-off)

- [x] `gradle -p apps/mobile mobileCheck :androidApp:assembleDebug -Piowb.android.enabled=true` passed, including shared JVM tests, JS checks and Android compilation/assembly.
- [x] Preserved the actual installed baseline APK before upgrade: `/tmp/iowb-copy-paste-manual.VgPqV0/baseline-arm64.apk`, SHA-256 `3aebe521a5eff613884c8c12c22d1ab52f1c5410a5dffee32736171ee41fc3f8` (do not confuse with the older output-directory artifact).
- [ ] Final build/tests after all later fixes.

### Android selection, clipboard-crash and keyboard cycles

Evidence: [Android UI event ledger snapshot through 12:40:48](artifacts/db-copy-paste-android-ui-20261001T124048.jsonl), 33 records, SHA-256 `10486b46423fffded5da4488b3bacbebbe9fa169892c06b7fdd8c5382cd7cb7f`. The [12:38:18 snapshot](artifacts/db-copy-paste-android-ui-20261001T123818.jsonl) and [earlier snapshot](artifacts/db-copy-paste-android-ui.jsonl) are retained rather than overwritten. Entries retain incomplete probes and explicit corrections; successful row-copy comparisons use real Android clipboard data and native PostgreSQL values. Later retests must be recorded in new snapshots, not inferred from a source fix.

- [x] ROW-01–03 first tests: single, contiguous, noncontiguous, reverse and random selections; nine-field comparisons for multirow cases passed.
- [x] ROW-05/06/08/10 first-batch variants: page-scoped select-all copies50 of60 source rows; empty-table Ctrl+C preserves clipboard with useful toast; Data-panel query copies all60 exact IDs. Later batch2 adds missing-table-metadata, separate SQL and view passes. Populated no-selection and missing-query-metadata UI remain pending.
- [x] ROW-07 first failure: selected id 1 deleted externally; exact-count reload kept index 0 and Copy silently copied id 2.
- [x] ROW-07 fix: clear positional selection before replacing the accepted table result.
- [x] ROW-07 retest: exact-count-enabled refresh now clears checkboxes and hides Copy; other stated variants still pending.
- [x] ROBUST-09 first failure: six large rows caused a fatal clipboard transaction of 3,000,996 bytes.
- [x] ROBUST-09 fix: serialize off the UI thread; preserve internal snapshot; use byte-size guard and JSON-file fallback; protect against stale copy completion.
- [x] ROBUST-09 retest: same six rows no longer crash; cancelled save retains 6 rows and 4 columns (corrected evidence); Inspector Copy JSON → Save produces a valid six-row file with all ASCII/emoji byte lengths correct.
- [x] UI-06/UI-09 fix and tested retest: Ctrl+C, Ctrl+V, F6 and Escape work without a focused database child. Meta+V did not open confirmation; no Meta-key pass is claimed.
- [x] UI-08 second failure: cancelling a keyboard-opened Paste dialog left a stale key token and blocked a fresh Ctrl+V.
- [x] UI-08 fix/retest: fresh Ctrl+V opens again after Cancel; held-repeat variant still pending.
- [x] Historical pre-recovery observation: version43 installed over baseline42 without a downgrade flag (then reported as ARM64). This is not the current recovered emulator build.
- [x] UI-01/02/03 tested variants: accurate 50-row source/timestamp/count metadata; 60-row snapshot exports all 60 IDs. The exact end of its first-50 preview and explicit zero/one preview-boundary cases remain pending.
- [x] CLIP-01/12/14 observed: Clear removes current preview/actions; APK-update/process restart preserves one-row and later complete 60-row snapshots. Persistence-clear after restart and explicit force-kill variants remain pending.
- [x] CELL-01/CELL-07 failure reproduced: visible Name resolves to lowercase name; Copy emits wrong column name and Save changes the wrong native column.
- [x] CELL-01/CELL-07 exact-name metadata fix and Android/native retest: copying Name preserves case; only Name changes on save while name remains unchanged. Special-name variant remains under CELL-01.
- [x] CLIP-11 failure reproduced: actual clipboard write denial leaves sentinel unchanged while UI says Column name copied.
- [ ] CLIP-11 truthful clipboard-publication fix and denied-write retest, including retained internal rows.
- [ ] Held-key repeats, copy-while-inspector-open, zero/one preview boundaries, and final-APK regression are still pending.

### Android paste batch 1 (12:43–12:58)

Durable evidence: [append-only paste ledger](artifacts/db-copy-paste-paste-20261001-batch1/manual-paste-evidence.jsonl), [archive index](artifacts/db-copy-paste-paste-20261001-batch1/archive-index.txt), with confirmation/result screenshots and native before/after maps beside them.

- [x] PASTE-03 first failure: a Data-panel JOIN enabled Paste and confirmed the selected table as destination; cancelled before writing.
- [x] PASTE-03 fix: conservative single-table source check for Data action availability and Data/SQL/inspector entrypoints; SQL action bar uses its safe-source predicate too.
- [x] PASTE-03 Data retest: ambiguous JOIN hides Paste; Ctrl+V outside the editor produces no confirmation and no writes. Simple single-table Data query successfully imports rows.
- [x] Supporting regression: DatabaseSqlSafetyTest and DatabaseRowActionAvailabilityTest pass after the paste safety changes; [test log](artifacts/db-copy-paste-paste-20261001-batch1/paste-safety-tests.log). These tests supplement, not replace, the UI retests.
- [x] PASTE-04/05: correct confirmation scope/count/columns and all Cancel/Back/outside zero-write dismissals.
- [x] PASTE-08–11: native all-success, first/middle/last partial failures and all-fail accounting; failed indices are accurate.
- [x] PASTE-14/15 constrained repeat and partial retry preserve committed rows; explicit retry warning is visible.
- [x] PASTE-16 active Data-query refresh shows correct row totals after successful imports.
- [x] Follow-up PASTE-16/17 FAILURE: root completed a 10-row import while an UPDATE draft was visible but never run; completion executed that draft and changed the native probe. [Root raw evidence snapshot](artifacts/db-copy-paste-root-ui-reconcile-20261001.jsonl), FAIL_UI record at 13:06:54.
- [x] Follow-up PASTE-16/17 changed-draft retest PASS:10 inserted, visible UPDATE never executed, native probe remains safe-refresh-before. [Root raw retest snapshot](artifacts/db-copy-paste-root-ui-refresh-retest-20261001.jsonl), PASS_UI record at13:31:14. Later batch2 also verifies actual switched-panel completion and original destination identity.
- [x] Captured-query/scope refresh and switched-panel original-target retests pass; close-source variant is tracked in batch2 below.
- [x] ROBUST-06 first failure and retest: timeout after 17 native commits initially showed only a socket error; now explicitly warns the outcome is unconfirmed and to check the destination before retrying.
- [x] Later batch2 executes all18 MAP variants plus PASTE02/03/07/12/13/14/18 and successful-completion PASTE17 switch. PASTE01 fresh no-selected-object and true connection-change-during-confirmation variants remain unexecuted; no all-variant completion is implied.
- [x] ROBUST11/12 later Android workloads pass:3000 rows,400 columns,350KB cell and3000 failures. These are bounded actual workloads, not an arbitrary-limit or uninterrupted-frame-time guarantee.

Historical batch1 guard/timeout retests used ARM64 version43 SHA-256 `16bcc08f385e90c59a241ed3271c95f19083eb6838a1f413c6d4b9f96de3d33e`. Basic paste outcomes have since passed on final candidate2c6a7bcd in batch2. At that earlier release, ui_target had9 rows and ui_slow_target had34; both clipboards were empty. No production server was restarted; candidate8928 restarts were coordinated only at request-free boundaries.

### Android mapping/paste batch2 (13:41–14:36)

Durable evidence: [complete append-only UI/native ledger](artifacts/db-copy-paste-paste-20261001-batch2/manual-paste-evidence.jsonl), [archive index and precise limits](artifacts/db-copy-paste-paste-20261001-batch2/archive-index.txt), and [identical final snapshot](artifacts/db-copy-paste-batch2-ui-20261001T1437.jsonl). The93 records include first failures and harness corrections, not93 unique passing cases. JSONL SHA-256 `341247e65e068fe8f6c6af746e71ca9a6ecdff4d9ca95b7c4e78eaebfb57487c`; screenshots/XML, bounded-stress payloads and native comparisons are beside it.

- [x] All18 PostgreSQL MAP variants exercised through actual confirmation/import, including confirmation-timed rename/add/drop.
- [x] Actual final-candidate PASTE12 eight-way fault replay, rapid-tap gate, changed-query provenance, known-source control and missing-table-metadata Copy retests pass.
- [x] Final-candidate Cancel/Back/outside, all-success, first/middle/last partial failures, all-fail, constrained retry, deliberate unconstrained repeat and legitimate preflight0/0 regressions pass.
- [x] PASTE06 actual Ctrl+Tab attempt is blocked by the native confirmation and retains the captured target. PASTE17 actual switched-tab and closed-source-panel completion both retain original target identity without changing/reopening the current panel.
- [x] Bounded actual stress:3000 rows,400 columns,350KB cell and3000 constraint failures; exact native values/counts, usable dialogs and unchanged process PID. No claim about arbitrary size limits or uninterrupted frame time.
- [ ] PASTE01 fresh no-selected-object workspace, PASTE06 actual connection-change, ROW10 missing-query-metadata and unrelated remaining cases retain their precise pending status.

Historical batch2 handoff artifact (14:36:54, before host recovery), reported ARM64 version43 SHA-256 `2c6a7bcd9aed553e3ee853ebdd646604ecb04433a9747d3cc55ddcb940e18c46`; not the final delivery build. Explicit ADB release at14:36:54: chooser verified Current DB8928, Database→Explorer, no modal/IME, both clipboards empty; proxy8950 passthrough. Added QA8950 saved profile remains; original profiles unchanged. A premature handoff assertion before accepting server-switch confirmation is explicitly excluded and superseded by the final verified record. Root/TRANSFER own the device after this historical handoff.

### Server audit links and open limits

See [transfer audit](artifacts/dbqa-transfer-report.md) for per-variant results, failure/retest cycles, actual engine identities and remaining UI work. Raw evidence is append-only under `artifacts/dbqa-transfer-*.jsonl`.

See [DATA-01–16 audit](artifacts/db-copy-paste-data-20261001/DATA-01-16-report.md) for live HTTP/native-value checks across all four engines and precise remaining limits. The baseline includes 953 non-CSV variants/groups plus 331 real CSV-parser→HTTP variants; these counts are not Android UI passes or counts of distinct checklist cases.

- [x] Transfer snapshot retests: aliased copy stops after the original 401 rows; concurrent source mutations do not change the captured set.
- [x] Restart recovery retest: an interrupted job is failed with an unknown-additional-writes warning instead of remaining running forever.
- [x] Cross-engine basic INTEGER/TEXT/PK matrix: four first-run MySQL-source schema failures fixed; final replay 48/48 (16 genuine engine pairs × 3 modes). This is not every data type/boundary cross-product.
- [ ] Exact 1,048,576,000-byte body-limit boundary: blocked on safe resources; no disabled-limit configuration exists. Header-only probes do not count as body-limit tests.
- [x] Android timeout-after-partial-write unknown-outcome messaging is reproduced, fixed and retested; batch2 also verifies forwarded commit then response disconnect. Exact before/during disconnect timing variants remain API-only.

### Resumed root UI cycle — 2026-10-01 13:31–13:33

- [x] PASTE-16/17 first failure reproduced with an actually changed visible SQL draft: import completion ran an UPDATE without pressing Run.
- [x] Fix captures SQL/scope/target/panel/result identity, checks before and after refresh, and skips changed contexts. Automatic refresh has a separate conservative SELECT gate; explicit Run remains unchanged.
- [x] Android retest: 10 rows inserted; visible unclicked UPDATE remains unexecuted; independent probe remains `safe-refresh-before`.
- [x] CELL-10 first failure: query-result JSONB editor saved invalid `NaN` as JSON string.
- [x] Fix resolved column metadata plus strict validation; Android retest leaves the error in Edit doc and preserves native stored value.
- [x] CLIP-09/CELL-08 first failure: URI-only replacement attempted a Workbench provider read and opened an empty editor.
- [x] Direct-text fix retest: no editor opened, zero provider calls after audit reset. Android clipboard still URI-only with null text.
- [ ] Remaining range/persistence/root variants and subsequent agents' UI batches are still pending. These results do not complete all 163 cases.

### Range and corrupt-persistence cycle — 2026-10-01 13:34–13:40

- [x] Sixteen actual Android range copies: seven shapes/directions × CSV/TSV, plus escaping in both formats. Copied contents parsed and compared to native PostgreSQL values.
- [x] Exact `Name` / `name` behavior confirmed; Done hides the selection toolbar after every copy.
- [ ] Remaining range type representations and stale refresh/filter/page cases are prepared, not yet run.
- [x] Four corrupt persistence variants injected into the disposable app only; restart and inspector showed empty state without crash; readback proves invalid saved key removed. Original clipboard restored afterward.

Evidence: [root UI events through 13:40](artifacts/db-copy-paste-root-ui-20261001T134000.jsonl). Historical pre-recovery installed checksum at13:40 (reported ARM64): `a19e8b6831547b4598af5b729999081db5afe8a1800d938c262bed0e3ad81d5c`; not the current emulator artifact.

### Intermediate verification — 2026-10-01 13:45

- [x] `gradle -p apps/mobile :shared:jvmTest mobileCheck -Piowb.android.enabled=true --console=plain` passed after the refresh/typed-cell/URI fixes.
- [x] JVM XML totals: 439 tests, zero failures/errors.
- [x] `cargo test -p iowb-server database:: --no-fail-fast -- --test-threads=1`: 62 passed, zero failures (13:46).
- [x] Both root and apps `git diff --check` passed.
- [ ] These checks must be rerun after any further fixes; final manual validation and Gofile delivery remain incomplete.

### Rapid-tap import cycle — retested

- [x] First Android failure: three rapid Paste taps followed by three Import taps inserted two identical unconstrained rows and opened two completion dialogs (batch2 evidence 13:52–13:53).
- [x] Fix: workspace gate reserves one confirmation, consumes its ticket once, rejects concurrent submissions, and releases on cancel/completion/failure; stale dismiss/completion cannot unlock a newer operation. Detached workspaces dismiss stale confirmation and invalidate its ticket.
- [x] Four focused gate regressions and Android Kotlin compilation passed.
- [x] Final-candidate exact triple Paste/Import retest inserts1 row, shows1 result and leaves no stacked confirmation (14:20:12).
- [x] Cancel/Back/outside regressions all close confirmation and write nothing (14:27). Two fresh intentional-repeat confirmations each insert exactly1 row (14:31), proving the gate releases for deliberate later actions.
- [x] Legitimate no-compatible and missing-required preflight0/0 responses retain useful errors and zero writes after strict response decoder change (14:30:04/14:30:17).

### Current-candidate automated verification — 2026-10-01 14:30

- [x] Root ran `gradle -p apps/mobile :shared:jvmTest mobileCheck -Piowb.android.enabled=true --console=plain`: BUILD SUCCESSFUL in24seconds,460 tests,zero failures/errors.
- [x] Durable per-suite XML: [shared test results](artifacts/db-copy-paste-shared-tests-20261001T143000/). No raw Gradle logfile was saved; terminal result reported by root. No source change or APK install occurred during this check.
- [ ] This is current-candidate verification, not completion of remaining manual variants or final Gofile delivery.

### Query-copy provenance cycle — retested

- [x] First Android failure: SQL changed from the tab's original `ui_target_repeat` context to `ui_paste_free_batch2`; values copied correctly, but inspector falsely named the original table (batch2 14:06:17).
- [x] Fix checks the executed result SQL and scope before reusing a context table name. Unproven/changed query sources are explicitly labeled `SQL query result`; both Data-query and separate SQL-query copy use this rule.
- [x] Three source-identity regressions and Android Kotlin compilation passed.
- [x] Rebuilt/install exact ARM64 candidate2c6a7bcd; changed-source retest passes at14:22:21 and known-source control at14:23:09. Clipboard values remain exact; source identities are respectively SQL query result and ui_paste_free_batch2.

### Missing-metadata Copy cycle — tested table variant retested

- [x] First Android failure at14:14:08: table-data response contained rows but no column metadata; visible Copy silently did nothing.
- [x] Fix infers the union of selected row keys instead of returning early; result-row detection also permits metadata-less query rows.
- [x] Final-candidate table response retest at14:23:43: proxy removed3 metadata columns, preserved12 rows, actual Copy exported exact row1 and inferred code/id/title. Source database unchanged.
- [ ] Query-response metadata stripping remains unexecuted in Android; unit fallback coverage is supporting evidence only.

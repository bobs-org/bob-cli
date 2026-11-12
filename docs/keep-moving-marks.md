# Keep-moving marks

This page is the authoritative display contract for keep-moving marks. The
JavaScript mirror lives in bob-ledger-tools: `src/141d-keep-moving-marks-model.js`
(pure core), `src/141e-keep-moving-marks-render.js` (render model and widget),
`src/271g-plugin-keep-moving-marks.js` (Live Preview extension, refresh, and
toggle), and `src/271h-plugin-keep-moving-marks-reading.js` (Reading view sweep).
The tests are `scripts/test-ledger-tools-keep-moving-marks.cjs` and
`scripts/test-keep-moving-marks-parity.cjs`. There is no api namespace.

## What you see

In today's daily note, the last planned Task Link to a leaf project can show how
many of that project's ready tasks the Keep-moving picker will offer:

```text
- [ ] () — WRITE
    - job_email_recruiters > ^luzern ⏭ 4
```

Those four rows are the project's Next residents. If a project has Pending,
Next, and Ready rows, the count and tooltip follow that lane order. With every
ledger mark present, a row reads:

```text
- ◐ bob_git#^wire-hooks 🔓 🔑 1 ⏭ 3 ⦸ off
```

The mark forecasts the `Keep ‹N› moving` group that a managed Done gesture
offers when it closes the task. It is a display promise; it does not select,
link, or change any task.

## Principles

- **Display-only.** The mark never writes to the vault.
- **Truthful or neutral.** A shown mark promises that its count and rows match
  the picker under the same inputs. No mark does not guarantee that the picker
  will be absent: a multi-link close can still produce a group. Uncertain,
  stale, or disagreeing inputs produce no mark.
- **One rule shared with the picker.** The mark uses the same keep-moving rule
  as the cycler's full successor planner. The identity set of rows and fold
  counts must match; zero rows means no mark.
- **What is deliberately not counted:** hand-offs (🔑), relation successors,
  project-finished pull-ins, and residents of sub-projects. Hand-offs are
  dependents and use their own mark. Relations are sequenced, not blocked.
  Project-finished pull-ins have a separate group. A project's mark counts only
  its own residents, never work in descendants.

## Which links

All these gates must hold:

1. The rendered note is today's canonical daily note and the session toggle is
   on. `plan.keep_moving` and `plan.link_unblocked` are enabled, the Tasks memo
   is warm, and the target project's source text is cached.
2. The link passes the 🔑 link gates in [Frees marks](frees-marks.md): it is a
   dedicated Task Link directly below an open Pomodoro entry in today's note.
   Struck, embedded, prose, nested, fenced, Events, closed-entry, and other-day
   links do not receive a mark.
3. X's note N is a recognized leaf project, and X is not the project's `^prj`
   task. A leaf is a sub-project whose R2 root parent differs from itself;
   top-level, `root: true`, orphan, and unresolved-parent projects are roots.
   See [task-dependencies.md §12.15](task-dependencies.md#1215-keep-moving-offers-leaf-projects).
4. No live link anywhere in today's Pomodoros targets another open task in N.
   The `^prj` task counts as work; closed targets and sub-project tasks do not.
   Live links include plain, embedded, and `#`-deferred Task Links under open
   entries. Thus the mark appears only on the last planned link; when another
   linked task closes, the surviving link can gain the mark (the baton pass).
5. N's `^prj` task is not Blocked: it is not `[?]`, has no open prerequisite,
   no future project schedule, and no `#prj_is_blocked`.
6. X's task subtree does not embed another task. Such a close could widen the
   completed set, so the result is neutral.
7. Cached project text and the Tasks memo agree that X and every counted
   resident are open. A mismatch means the sources are mid-update; wait for a
   refresh and show nothing.
8. At least one row remains offerable after applying the count rule. If none
   remain, project-finished or another picker group owns the result.

The target X does not need an `[id::]` field. A project task linked twice has
the same mark on both rows.

## The count rule

The mark computes the rows the picker would offer after a managed Done of X:

```text
RESIDENTS = open task lines in N's text (#task plus #ref without #task),
            excluding N's ^prj task R and X; never include descendants
OPEN_IDS  = ids of open tasks in the Tasks memo, minus X's id (X is closing)

For each resident t, in Pending → Next → Ready order, then line order:
  blocked   ⇔ status is [?] ∨ #prj_is_blocked ∨ an open dependsOn id resolves
  scheduled ⇔ not blocked ∧ scheduled date is after today
  hidden    ⇔ not blocked ∧ not scheduled ∧ whole-token #hide
  otherwise offer t unless it is a hand-off of X or a configured relation
  successor of X, or t already has a live Task Link today

N_mark = number of offered rows
mark ⇔ all link gates pass ∧ N_mark ≥ 1
```

Unresolved dependency ids do not block. A resident scheduled today, a `#ref`
line without `#task`, nested subtasks, and recurring tasks due today are counted
when otherwise offerable. The tooltip reports non-offered rows as blocked,
scheduled later, or hidden. The full picker remains the reference for parity.

**Parity invariant:** for each fixture, row identities (path plus block id, or
path plus line when there is no block id) equal the cycler's selectable
`keep_moving` rows after verdict dedup for a managed Done of X. Fold counts are
equal, and zero rows correspond exactly to no mark.

## Anatomy and look

- Host: `span.bob-keep-moving-mark[role=img][aria-label][data-tooltip-position=top][data-count][data-block-id][data-project]`.
  It contains an empty `aria-hidden` CSS-mask glyph and the exact count. It has
  no `title` attribute and no listeners, so Reading view can re-render it.
- The filled skip-forward glyph matches the picker symbol and uses the Next
  status token. The number stays muted like 🔑. There is no border or filled
  capsule at rest, and no `9+` cap.
- CSS contract (the glyph variable is defined beside `--bob-off-clock-icon`):

```css
--bob-keep-moving-glyph: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'%3E%3Cpath d='M3.5 2.75 11 8l-7.5 5.25z'/%3E%3Crect x='11.75' y='2.75' width='1.9' height='10.5' rx='.6'/%3E%3C/svg%3E");

.bob-keep-moving-mark {
  --bob-keep-moving-mark-ink: var(--task-status-next, var(--color-green, #3f8f5f));
  display: inline-flex;
  align-items: baseline;
  gap: 0.22em;
  margin-inline-start: 0.4em;
  padding: 0 0.42em;
  border-radius: 999px;
  font-family: var(--font-interface);
  font-size: 0.8em;
  font-variant-numeric: tabular-nums;
  line-height: 1;
  white-space: nowrap;
  vertical-align: baseline;
  color: var(--bob-keep-moving-mark-color, var(--text-muted));
  user-select: none;
  cursor: default;
  print-color-adjust: exact;
  transition: background-color 120ms ease, color 120ms ease;
}
.bob-keep-moving-mark-glyph {
  display: inline-block;
  inline-size: 0.95em;
  block-size: 0.95em;
  align-self: center;
  background-color: var(--bob-keep-moving-mark-ink);
  -webkit-mask: var(--bob-keep-moving-glyph) center / contain no-repeat;
  mask: var(--bob-keep-moving-glyph) center / contain no-repeat;
}
.bob-keep-moving-mark:hover {
  color: var(--text-normal);
  background-color: color-mix(in srgb, var(--bob-keep-moving-mark-ink) 12%, transparent);
}
.bob-keep-moving-mark.bob-keep-moving-mark-tick {
  animation: bob-keep-moving-mark-tick 180ms cubic-bezier(0.2, 0.9, 0.3, 1.25);
}
@keyframes bob-keep-moving-mark-tick {
  from { opacity: 0.3; scale: 0.6; }
  to { opacity: 1; scale: 1; }
}
@media (prefers-reduced-motion: reduce) {
  .bob-keep-moving-mark,
  .bob-keep-moving-mark.bob-keep-moving-mark-tick {
    animation: none;
    transition: none;
  }
}
@media (forced-colors: active) {
  .bob-keep-moving-mark { color: CanvasText; }
  .bob-keep-moving-mark-glyph { background-color: CanvasText; }
}
body:not(.bob-keep-moving-marks) .bob-keep-moving-mark { display: none; }
```

## Placement

- **Live Preview:** zero-width widget after `]]` and before a trailing ` #`,
  with `side: 3`. It covers no text and stays at the link end while the caret
  reveals raw Markdown.
- **Reading view:** place after trailing 🔓, mail reply, and 🔑 hosts, but before
  the off-clock chip. Without those marks, place after `a.internal-link` or its
  sole `del`/`s`/`strike` wrapper. The off-clock insertion also steps past this
  host, preserving a single order regardless of processor order.

## Tooltip

The one-line-per-row `aria-label` and Obsidian tooltip never contain `::`, 🔓,
or “unblocked”. Example:

```text
Closing this offers 4 tasks to keep job_email_recruiters moving
It's the last job_email_recruiters task planned today
• ‹title› · Next
• ‹title› · Next
• ‹title› · Next
+1 more
ready in job_email_recruiters · not dependents
not offered  2 blocked · 1 scheduled later · 1 hidden
```

Use “offers 1 task” for one row. Show up to three row titles, cleaned with
`dependencyCleanTaskText` and truncated to 47 characters plus `…`; append the
lane name (Pending, Next, Ready). The note stem names N. Omit zero fold counts
and omit the entire fold line when all counts are zero. Replace stray `::` with
`:`.

## Interaction

A plain mousedown places the caret at the widget position and focuses the
editor; it never navigates or writes. Mod+click in Live Preview opens project N
in a new tab. The Reading host stays listener-free. There is no tab stop or
live-region announcement.

## Motion

Each editor view keeps a bounded LRU (about 2,000 keys) of X path plus block id
to last eligible count; eligible zero results are stored as zero. When a known
key changes to a positive count, add the tick class for about 400 ms. Do not
tick on first paint, scroll, viewport-only rebuild, or Reading view. Reduced
motion disables the tick. A change to zero removes the mark.

## Liveness

One coalesced `scheduleKeepMovingMarksRefresh()` (about 120 ms) runs on the
`scheduleLiveWidgetRefresh()` fan-out, relevant metadata changes and resolves,
vault rename/delete, source-text cache fills, local-midnight rollover,
`layout-change`, and the toggle. Live Preview also rebuilds on document,
viewport, mode, and file changes, never selection alone. Discard out-of-order
results; invalidation must not leave a stale positive visible. Reading refresh
sweeps today's rendered notes in every window.

## Toggle

`toggle-keep-moving-marks` is the session toggle named “Toggle Task Link
keep-moving marks”, default on. It syncs the gate class to popout windows and
shows “Keep-moving marks on/off”. Version one has no api namespace.

## Performance

Parse live links once from the whole day (including below-fold links), group by
project, and evaluate the rule once per project per build. Create widgets only
inside visible ranges. Never read from disk during render. Request project
text only after cheap frontmatter gates and only for projects behind an open X.
The shared source-text LRU cap is 64. The node budget is one build under about
25 ms for 200 links / 2,000 tasks and for 50 links / 3,000 tasks across 10
project notes.

## Architecture

The core is pure, synchronous, and never throws. Rendering, Reading view,
tests, and parity checks use the same functions with injected project text,
frontmatter, link resolution, Markdown paths, config, and relation edges. The
leaf predicate ports the cycler's root-parent walk. Residents come from held
project text; the Tasks memo supplies link resolution and open ids. Relation successors come from ledger-tools' own
relation index. If configured successor labels exist but the index is not
ready, the answer is unknown. Rendering does not call the cycler or a Rust JSON
count.

## Deferred

- Mac Capture idle agenda, Task Card, and nav pickers; revisit when Bryan asks.
- A dimmed “queued” mark for non-last sibling links; revisit when Bryan asks.
- A per-note `keep_moving: false`; revisit if a leaf project proves noisy.
- Links in other daily notes, closed entries, or prose.

## Conformance vectors

### LP — leaf vectors

The leaf rule and LP1–LP17 are defined in [task-dependencies.md §12.17](task-dependencies.md#1217-lp-leaf-vectors); that table is authoritative.

### KB — which links and the count (mark)

Unless a row says otherwise: today's daily note has an open Pomodoro and a live
link to X (`Reply to Luzern ^luzern`) in N, `job_email_recruiters.md`; N has an
open, unblocked `^prj` task and parent `job_2026`, a project under the `job`
area whose Sub-projects marker lists N.

| # | Situation | Mark |
| --- | --- | --- |
| KB1 | residents Pending P, Next B, Ready A; X is N's only live link | `⏭ 3`; tooltip rows P, B, A |
| KB2 | another open resident U also has a live link (plain, `#`-deferred, or `![[…]]` embed; any open entry) | none on X; none on U |
| KB3 | KB2, then U's link is struck or U closes | X gains `⏭ N` with one tick (Live Preview) |
| KB4 | U is linked only struck, under a closed entry, in another day's note, or in prose | X marked; U counted when offerable |
| KB5 | R itself is live-linked | none |
| KB6 | N is a root project (LP2, LP3, LP5, LP14, LP15) | none |
| KB7 | N has 4 other Next residents and nothing else open (the screenshot) | `⏭ 4` |
| KB8 | N is a sub-project (LP1) | marked as KB1 |
| KB9 | every other resident is Blocked, scheduled after today, or `#hide` | none |
| KB10 | X is N's last open task | none (`project_finished` owns that close) |
| KB11 | resident D depends only on X | D not counted (🔑 counts it); folds follow the parity invariant |
| KB12 | resident S has `starts after [[X]]` in LINKS, and `starts after` is a configured successor label | S not counted |
| KB13 | R is `[?]`, has an open prerequisite, has a future project schedule, or carries `#prj_is_blocked` | none |
| KB14 | `plan.keep_moving: false`; `plan.link_unblocked: false`; toggle off; cold Tasks memo; N's text not yet cached; successor labels configured but relation index not ready | none (each) |
| KB15 | X has no `[id::]` | marked |
| KB16 | X is linked under two open entries | both rows show the same `⏭ N` |
| KB17 | N's parent stem collides with `_generated/tag_pages/topic/job_2026.md` | same answer as without the tag page |
| KB18 | a resident scheduled today; a `#ref` line without `#task`; a nested subtask; a recurring task due today | each counted |
| KB19 | a resident's `[dependsOn::]` names an id that resolves to nothing | counted (unresolved never blocks) |
| KB20 | X's subtree embeds a task (`![[…#^…]]`) | none (neutral) |
| KB21 | X Done or Cancelled; or the link is struck, embedded, prose, nested, under a closed entry, in Events, fenced, or in a daily note other than today's | none |
| KB22 | X lives in a sub-project K of N | evaluated for K only; N's residents never count |
| KB23 | N's cached text and the memo disagree on whether X or a counted resident is open | none until they agree |
| KB24 | every row above | ROWS identity set == the cycler planner's `keep_moving` rows for a managed Done of X; folds equal; 0 ⇔ no mark |

### KT — tooltip

| # | Case | aria-label |
| --- | --- | --- |
| KT1 | KB1 with P `Ship it`, B `Wire it`, A `Write it` | `Closing this offers 3 tasks to keep job_email_recruiters moving\nIt's the last job_email_recruiters task planned today\n• Ship it · Pending\n• Wire it · Next\n• Write it · Ready\nready in job_email_recruiters · not dependents` |
| KT2 | one row | first line `Closing this offers 1 task to keep job_email_recruiters moving` |
| KT3 | five rows | three rows, then `+2 more` |
| KT4 | folds: 2 blocked, 0 scheduled, 1 hidden | last line `not offered  2 blocked · 1 hidden` |
| KT5 | a 60-character title | first 47 characters plus `…` |
| KT6 | a title with links, tags, fields, and `^id` | cleaned by `dependencyCleanTaskText` |
| KT7 | a stray `::` | becomes `:` |
| KT8 | any model | contains neither `🔓` nor `unblocked` |

### KP — placement and interaction

| # | Case | Result |
| --- | --- | --- |
| KP1 | Live Preview | widget at the link end with `side: 3`; no line-height change |
| KP2 | link with 🔓, 🔑, ⏭, and the off chip | order: link, 🔓, 🔑, ⏭, off chip (`side: 4`) |
| KP3 | Reading view, no other marks | host directly after `a.internal-link` (or its sole `del`/`s` wrapper) |
| KP4 | Reading view with 🔓 and/or 🔑, plus the off chip | host after the last of 🔓/🔑; off chip still last |
| KP5 | caret on the row (raw `[[…]]` revealed) | mark stays at the link end |
| KP6 | toggle off, then on | marks vanish and return; row height unchanged |
| KP7 | plain click / Mod+click | caret placed, nothing written / N opens in a new tab |

## Live verification

Scratch vaults only (never `~/bob`). Two git clones of the vault repo:
research snapshot `d5d358c4` and current `HEAD` `8d0e2532`. Today's daily
note is already `2026/20261111.md`, so no copy was required. Release `bob`
`0.5.0` from this tree, `BOB_NOW=2026-11-11 11:30:00`. Obsidian 1.15.0
(`/snap/obsidian/72/app/obsidian`) under `xvfb-run -a`, Tasks
`taskFormat: dataview`, bob-plugins from the linked repo. Isolated profile
and vault under `/tmp`; Chrome's singleton socket needs a short `TMPDIR`.

**Snapshot `d5d358c4` — `bob capture --dry-run -f json`.** Ten live dedicated
Task Links under open Pomodoros. Exactly one non-empty `keep_moving` array:
`!job_email_recruiters:bryan-stephen-luzern-risk` offers four rows
(`^ecogpt`, `^ray-lyles`, `^product-engineer-dan-dowd`,
`^senior-rust-java-engineer`), `pull_in.offer_count` 4, title
`Keep ‹job_email_recruiters› moving`, every row `selected: false`. The other
nine links (`bob`, `sase`, `sase_blog`, `cash`, `body`) return empty
`keep_moving`. Zero capture mismatches.

**Snapshot — rendered marks (Live Preview).** After scrolling the WRITE
entry into view, exactly one `.bob-keep-moving-mark`: `data-count="4"`,
`data-block-id="bryan-stephen-luzern-risk"`,
`data-project="job_email_recruiters.md"`, compact text `4`, glyph child
present. Tooltip (aria-label) lists EcoGPT, Ray Lyles, Dan Dowd, then
`+1 more`, matching KT3 truncation and the capture identity set. No
keep-moving host on `bob`, `sase`, `sase_blog`, `cash`, or `body` links
(`sase_blog#^s0-spike` shows a 🔑 frees pill only). Screenshot reviewed
against Anatomy and look.

**Current `HEAD`.** The screenshot recruiter link is already struck under a
closed WRITE. Ten live links remain; capture marks exactly one:
`!job_email_recruiters:product-engineer-dan-dowd` offers the three leftover
Next rows (`^ecogpt`, `^ray-lyles`, `^senior-rust-java-engineer`). Quiet
stems stay empty. Zero capture mismatches.

**Picker.** Capture JSON is the picker identity set: `Keep ‹job_email_recruiters›
moving` with those four rows and nothing preselected. In the isolated
scratch Obsidian, the cycler's Task-Link close (`handleActiveTaskBlockLinkOpenDone`,
nav `api.successors` v4 present) completed the linked task and struck the
ledger row; the successor modal did not open within 30s and `successors.pick`
was not invoked. Esc therefore had no extra pull-in (the four offered tasks
were not linked). That hang is outside this audit's small-fix budget; the
mark and capture already agree on the four-row group. The cycler's KM33
wiring test (`scripts/test-task-status-cycler-keep-moving.cjs`) drives the
same daily-note Task-Link close through `handleActiveTaskBlockLinkOpenDone`:
it opens exactly one `Keep ‹job_email_recruiters› moving` group with those
four rows and nothing preselected, and Esc writes only the close and strike.


**2026-11-11 — sub-project rule restored.** The earlier record's standalone
recruiter premise no longer describes the vault: `job_email_recruiters` now
has project parent `job_2026`, under the `job` area. Root projects never qualify,
with or without a Sub-projects marker; the mark and picker still agree for
sub-projects. LP1–LP17 and KB24 cover the reverted rule, including generated
parent-stem collisions and equal offered identities and fold counts.

A fresh scratch clone of vault HEAD `5c2bec06`, built `bob` 0.5.0, and the
updated plugin bundles were checked at `BOB_NOW=2026-11-11 16:00:00`.
`bob capture --dry-run -f json '!job_email_recruiters:product-engineer-dan-dowd'` offers
three Next rows: `ecogpt`, `ray-lyles`, and `senior-rust-java-engineer`.
Scratch Obsidian 1.15.0 renders `⏭ 3` on that link with the same tooltip rows.
`job_update_cv` is already closed, so the negative case uses
`dev_plex_hack_patch`, whose parent is the `dev` area. Only in scratch, a live
link to its open `no-clip-secrets` task and a Next resident were staged.
That dry-run returns no keep-moving offers, and its visible ledger link has
no mark. Both screenshots were inspected. Live vault notes were unchanged;
the existing picker-stall investigation remains separate.

The packaged startup smoke check passed, and Cycler 1.49.1 synced successfully.
After Bryan approved replacing the abandoned Ledger 1.110.2 build, upstream was
rechecked and advanced to `934bbbb`, preserving the landed navigation update.
The approved deployment stamp still matched `99ccca1` at `2026-11-11T19:43:12Z`.
Ledger 1.111.1 synced at `2026-11-11T22:21:07Z`, backing up the three replaced files
under `~/.local/state/bob-cli/plugin-backups/20261111-172107/bob-ledger-tools/`.
The post-sync Obsidian gate passed: all ten plugins matched, all four editors
mounted, the relation index was ready, and no exceptions or crashes occurred.

# Snooze marks

A Snoozed Task wears an alarm clock where a Today task wears a tomato. This
page is the display-only contract for all four snooze surfaces: the clock on
the task definition, the woken pill on the ledger Task Link, the paint on
`## Snoozed` rows and their heading, and the snooze chip in `bob-plan`.
Behavior, grammar, classification, and JSON live in [Snoozed
tasks](snooze.md); this page owns pixels, strings, tooltips, navigation,
liveness, and the executable `SM-*` vectors in
`tests/fixtures/snooze/marks.json`. The Rust side paints nothing; the
JavaScript mirror lives in bob-ledger-tools and runs those vectors verbatim.

## What you see

```text
definition:  - [*] ⌗ ⏰ 15:30 Fix the parser crash ^fix
row asleep:  - **1530** [[job#^email-dan]] · in 2h 30m
row due:     - **1530** [[job#^email-dan]] · due · waking
row blocked: - **1530** [[job#^email-dan]] · waits on prerequisites
history:     - ~~**1530** [[job#^email-dan]]~~ · woke 15:32
ledger link: - [[job#^email-dan]] ⏰ woke 15:32
plan chip:   TODAY 9/10 · ⏰ 2 · 15:30
```

`⌗` approximates the task identity glyph. `⏰` is the single alarm glyph for
every snooze surface: Lucide `alarm-clock` in Obsidian, never 💤 (the Reading
Board's Snoozed lane) and never ◷ (scheduled project cards). Shape plus words
carry every state; color is never the only cue.

| State | Definition clock | `## Snoozed` row | Ledger Task Link | Plan chip |
| --- | --- | --- | --- | --- |
| **Asleep** | indigo `⏰ 15:30` | indigo time pill, faint `in 2h 30m` | (not in the ledger) | indigo `⏰ 2 · 15:30` |
| **Due** | amber `⏰ 15:30 · due` | amber `due · waking` | — | amber when any row is due |
| **Blocked** | muted `⏰ 15:30` | muted `waits on prerequisites` | — | time omitted when only Blocked rows remain |
| **Woken** | 🍅, with the existing 600 ms entrance pop | struck row, dim `woke 15:32` | warm `⏰ woke 15:32` for 60 minutes, then a quiet outline | counted normally in TODAY |

- A definition never wears both marks. A live ledger link wins and the clock
  disappears; 🍅 enters with the existing 600 ms pop. Ignored rows never
  create a clock.
- Mod+click on the clock jumps to the Snoozed row, mirroring Mod+click on 🍅.
- Marks are display-only, cannot be copied, do not shift line height, and
  respect reduced motion and forced colors.

## Principles

- **Display-only:** nothing writes the task or the daily note; source bytes,
  copy/yank output, and document positions stay unchanged.
- **Truthful or neutral:** missing facts, stale caches, ambiguous identity, or
  an unproven rendered source mapping yield neutral paint, never a guessed
  state.
- **One clock:** the clock means the snoozed reader's live-or-blocked set for
  that task. It never means daily-note residence or earlier work.
- **Rare and quiet:** only a small number of lines carry snooze paint, so the
  alarm reads as one concept across all four surfaces.

## Which definitions get a clock

A line gets a snooze clock only when all of these hold:

1. The snooze cache is ready for the current date and daily path.
2. The live task status is open and committed: `*` (Next), `/` (In Progress),
   or `?` (Blocked). Ready, done, and cancelled tasks never wear a clock.
3. It is a real `#task` line: an exact `#task` tag token, outside inline code
   and fences, with a standalone trailing `^id`.
4. The task is the resolved identity of exactly one live-or-blocked Snoozed
   row in today's note. Unresolved, ambiguous, ignored, and history-only
   identities are neutral. Duplicate ids are neutral because the link target
   is ambiguous.
5. The task has no live dedicated Task Link under an open Pomodoro. A live
   ledger link wins: the clock leaves and 🍅 stays.
6. The host is not a canonical daily note, and the line is not rendered inside
   one.
7. Both the Today-marks toggle and the `toggle-snooze-marks` toggle are on.

Link spelling never matters: basename, local (`[[#^id]]`), embedded, and
aliased links resolve through the vault link resolver to the same identity,
and two spellings of the same note and block id are the same task.

Precedence is fixed: Today tomato first, then the planned Snoozed clock, then
absent. Blocked clocks are muted. Ignored rows never create a clock.

## Glyph anatomy

The host is an empty `span.bob-snooze-mark` with `role="img"`, an accessible
label, and a top-positioned Obsidian tooltip. Copying a line never copies the
clock, and a theme snippet can replace it with a mask.

The host draws the Lucide `alarm-clock` glyph from
`content: var(--bob-snooze-mark-glyph)` through a CSS mask, tinted
`currentColor`. The time rides in `::after` from
`attr(data-bob-snooze-time)` with tabular numbers
(`font-variant-numeric: tabular-nums`), so `15:30` never reflows as the
countdown ticks.

- Asleep is indigo, a `color-mix` of the theme blue and purple, matching the
  ledger's asleep tone.
- Due is amber with one gentle transition pulse. The pulse never replays on
  scrolling, rerender, or startup.
- Blocked is muted.
- The box matches the Today mark metrics (`1.2em × 1.2em`, `font-size:
  0.85em`, `line-height: 1`, `vertical-align: -0.14em`,
  `margin-inline: 0.06em 0.3em`, `border-radius: 999px`,
  `user-select: none`, `cursor: default`), so the clock never changes line
  height. Hover uses a capsule tinted with the task line's status, with
  `background-color 120ms ease`.
- The decoration builder and the rendered-view post-processor both check the
  toggles. The toggle gates are session-only:
  `body.bob-snooze-marks-off .bob-snooze-mark { display: none; }` hides every
  clock in main and popout windows. The builder also hides clocks when the
  Today-marks toggle is off, since the two marks share one slot.

## Tooltip and accessibility

The tooltip is the element's `aria-label`, with
`data-tooltip-position="top"`. There is no `title`, avoiding duplicate
tooltips. It has two lines joined with `\n` and never contains `::`.

- Line one is `Snoozed until 15:30 · in 2h 30m` for an asleep clock. A due
  clock ends in `due — waking`; a blocked clock ends in
  `waits on prerequisites`.
- Line two is `Ctrl-click to open the Snoozed row` (`⌘-click to open the
  Snoozed row` on macOS). It is omitted on mobile.

The clock adds no tab stop and no live-region announcement. Reduced-motion
settings disable the due pulse and every transition. Forced-colors mode keeps
a visible glyph, a `1px solid CanvasText` outline, and readable state text.
The glyph uses `print-color-adjust: exact`.

## Interaction

- **Plain click** places the cursor at the title start (`view.posAtDOM(dom)`)
  and focuses the editor. It never writes or navigates.
- **Mod+click** (Cmd on macOS, Ctrl elsewhere, primary button) jumps to the
  current Snoozed row in today's daily note. An already-open daily-note leaf
  is reused; the cursor moves to the row token, centered in view. The target
  is revalidated against the open note and rescanned after source line moves;
  when the row is gone, a Notice says
  `This task's Snoozed row is no longer in today's note`. A second press
  while a jump is in flight is ignored. Plain clicks keep ordinary editor
  placement.
- **Go to the Snoozed row** is the keyboard twin of Mod+click. It is an editor
  command with no default hotkey and jumps for the task on the cursor line.
  When that line has no live Snoozed row, it shows `Not snoozed for today`
  (or `No task on this line`).

## Which ledger links get a woken pill

A dedicated, unstruck Task Link under an **open** Pomodoro entry wears
`⏰ woke 15:32` only when all of these hold:

1. The rendered file is today's canonical daily note.
2. The row is a ledger Task Link: a direct child bullet of an open entry
   whose marker-stripped body is exactly one plain or aliased block link, not
   struck, fenced, embedded, or mixed with prose. The established
   ledger-link parser decides; prose mentions, nested notes, fenced links,
   and struck links never qualify.
3. The link is not an Event Task link and its target is not an Event Task.
4. The target resolves to exactly one open, committed, unblocked `#task`
   through the vault link resolver. Missing and ambiguous targets are
   neutral.
5. A struck history row in the same note's `## Snoozed` section matches that
   resolved path and block identity, with a valid `woke HH:MM`. The latest
   valid wake time wins; later source order breaks equal-time ties, matching
   the Rust agenda's `attach_snooze_history` selection. A re-snoozed task
   whose struck row was replaced by a new active row has no pill.
6. The `toggle-snooze-marks` toggle is on.
7. The pill is for today. A negative age (woke time later than now) is
   neutral.

Closed entries carry 🍅 history already and get no pill. Earlier daily notes
keep a quiet history pill under eligible open entries (see Historical notes),
never a fresh accent or a live countdown.

## Woken pill anatomy and placement

The host is a `span.bob-snooze-woke` with `role="img"`, an accessible label,
and a top-positioned Obsidian tooltip. It paints through the same shared
alarm glyph as the definition clock: warm amber-gold while fresh, a quiet
outline once settled.

- **Fresh** means today and `0 <= age < 60 minutes`, where age is now minus
  the actual wake time. Exactly 60 minutes settles to the outline.
- The text is `⏰ woke 15:32`, using tabular numbers.
- The tooltip names the target and the exact early/on-time/late meaning:
  `Woke 15:32 (snoozed until 15:30, 2m late)`,
  `Woke 15:28 (snoozed until 15:30, 2m early)`, or
  `Woke 15:30 (snoozed until 15:30, on time)`. Line two is the navigation
  hint, as on the definition clock. Any `::` in the label becomes `:`.

Live Preview inserts a zero-width widget at the beginning of the link, before
`~~` when struck or `[[` otherwise, at `side: -3`, ahead of the type mark
(`-2`) and the progress mark (`-1`), so the row reads
`pill link #TYPE ◐`. Reading view inserts the pill before the link container
(including a wrapper that contains only the link), moved before ledger type
or progress marks already there, giving the same order regardless of
postprocessor order. The pill's class joins the ledger mark rebuild
allowlist so ledger rebuilds preserve it. The pill adds no line height or
document position, takes no tab stop, makes no live announcement, and is not
copied with the row.

A plain mousedown in Live Preview places the caret and focuses the editor.
Mod+click with the primary button jumps to the matching struck row; plain
click never navigates or writes.

## Snoozed row paint

Only the structural H2's own rows are painted, with exact row source mapping.
Notes beneath rows are retained and the underlying Markdown is never
rewritten. A similarly named heading inside a fence, an indented child
example, or another section never paints, and painting never mutates a note
to decorate it.

| Row | Paint |
| --- | --- |
| **Asleep** | an indigo time pill (`15:30`) plus a faint live countdown (`in 2h 30m`) |
| **Due** | amber `due · waking` |
| **Blocked** | muted `waits on prerequisites` |
| **History** (struck) | dimmed, with `woke 15:32` |
| **Ignored** | the state's muted reason chip (see below) |

Ignored reasons are short and accurate:

| Reason | Chip text |
| --- | --- |
| Ready | `Ready — won't wake` |
| Missing, ambiguous, done, cancelled, already linked, event, non-task | `Missing — won't wake`, `Ambiguous — won't wake`, `Done — won't wake`, `Cancelled — won't wake`, `Already linked — won't wake`, `Event — won't wake`, `Not a task — won't wake` |
| Duplicate loser | `Duplicate — earliest wins` |
| Malformed row | `Unrecognized row` |

Malformed, extra-link, fenced, and indented row-like text is reported and
never executed: it gets no actionable time or membership. Child notes that
merely look row-like stay notes. A due row whose countdown passes while the
note is open flips to `due · waking` on the minute tick without reopening
the note.

## Heading summary

The `## Snoozed` heading carries a trailing summary chip built from unique
planned identities. A duplicate loser, history row, ignored row,
already-linked row, or off-clock nuance never joins the count; only
Asleep, Due, and Blocked identities do.

- Subcounts distinguish asleep, due, and blocked. Zero subcounts are
  omitted: `2 asleep · next 19:00`, `1 asleep · 1 due · 1 blocked · next
  15:30`, `1 blocked`.
- `next HH:MM` is the earliest live target time and is omitted when no live
  row remains, including blocked-only sections.
- A section containing only ignored or history rows gets no live heading
  summary.
- With the snooze toggle off there is no paint and no summary; source text
  stays readable.

## Historical notes

An earlier daily note keeps neutral historical paint: quiet rows with no
live countdown and no today-state heading summary. Its eligible open ledger
entries keep a quiet history pill under them (`⏰ woke 15:32`, outline tone
only), never a fresh accent and never a live countdown. No old note
contributes to today's marks, today's counters, or the plan chip.

## Toggles

**Toggle snooze marks** (`toggle-snooze-marks`) is a session toggle, on by
default. It gates the definition clocks, the decorative daily row paint, and
the woken-link pills: off means no paint anywhere, while source text stays
readable. It also gates the decorative daily row and heading summary. The
definition clock additionally honors the Today-marks toggle, since the two
marks share one slot: either toggle off hides the clock, while an existing
🍅 keeps its own Today toggle behavior.

The plan chip is a meter, not paint: it stays available with all paint off
because it reports counts. Both toggles update main and popout windows. Bob
exposes no API namespace for this feature.

## Surfaces

| Surface | Contract |
| --- | --- |
| Live Preview of area, project, reference, and inbox notes | v1; exact task-line and row matching |
| Reading view, block embeds, hover popovers | Paint only with exact source mapping; otherwise neutral |
| Canonical daily notes and content rendered inside them | Never carry a definition clock; their `## Snoozed` rows and history pills paint normally |
| Tasks query results (dash, freshness, rotten, blocked, crowded) | Out of scope |
| Source mode | Never painted; raw text only |

## Staying current

One coalesced refresh path runs from the landed snooze cache and the minute
lifecycle. It covers countdown changes, Asleep-to-Due flips, retiming with
unchanged membership, Blocked transitions, wake-history changes, the
60-minute fresh-to-settled expiry, midnight, source-cache fills, metadata
updates, renames and deletes, toggle changes, and popouts. The existing
cache's membership-only plan rerender is not enough on its own: row-detail
changes rerender paint too. Display code spawns no second waker and installs
no redundant polling timers, and every listener and timer is disposed on
unload.

The current editor document is authoritative in Live Preview, including
unsaved edits. Reading view uses proven source mapping with the established
source caches; no synchronous file read or subprocess call runs during
render. Late async reads are dropped by a generation guard, and a read
failure degrades to no paint.

## Performance

Snoozed rows are scanned once per document version and indexed by resolved
identity. Each unique link path resolves once per build. Live Preview
creates widgets only in visible ranges and never rebuilds on selection
alone. Rendering never reads from disk. A synthetic ledger with 200 links
and 50 Snoozed rows should build in about 25 ms or less, matching the Event
link times budget.

## Plan chip

A separate snoozed meter renders in the `bob-plan` block immediately beside
TODAY: `⏰ 2 · 19:00`. The time is omitted when only Blocked rows remain
(`⏰ 1`). The chip is indigo, or amber when at least one row is Due. It never
inherits `workToneClass`, over-cap styling, or TODAY's status color.

- The accessible tooltip reads, for example,
  `2 snoozed · next wakes 19:00 · not counted in TODAY · 11/13 when all
  wake`. Due and blocked words join when needed
  (`2 snoozed · 1 due · next wakes 15:30 · not counted in TODAY · 9/11 when
  all wake`); a blocked-only queue never claims a next wake. `a/b` is
  current Today over Today plus the on-clock addition.
- Clicking the chip jumps to today's `## Snoozed` heading through the common
  section jump.
- The forecast adds the current on-clock ledger link count to the unique
  eligible Asleep/Due on-clock targets, reusing the same effective-clock
  resolver as the link budget (explicit tags and inherited type-filtered
  defaults). Blocked, ignored, history, duplicates, already-linked, and
  off-clock rows are excluded from the addition. The forecast never changes
  `budget.status`, `links`, TODAY, lane counts, or wake permission; an
  over-cap forecast raises the advisory `snooze_wake_would_exceed_cap` lint
  and wake still runs.
- Queue-only notes without a Pomodoros section still show a chip. Missing
  task or cache data stays neutral. An empty queue removes the chip
  immediately. Retiming, minute transitions, and inherited clock or type
  changes rerender the chip even when queue membership is unchanged.

## Conformance vectors

`tests/fixtures/snooze/marks.json` owns four groups with documented,
language-neutral schemas (`schemas` in the fixture file). The JSON is the
executable data; the tables below are its readable index. Every vector pins
an explicit clock, source, task, and resolver facts plus the expected
display model.

Unless a vector says otherwise, today is 2026-11-05, now is 13:00, zone is
`America/New_York`, the note is `2026/20261105.md`, the task is
`- [*] #task Email Dan ^email-dan` in `job.md`, and both toggles are on.

### SM-D — definition clocks

| ID | Setup | Result |
| --- | --- | --- |
| SM-D01 | asleep row `**1530**`, now 13:00 | indigo clock `⏰ 15:30`; tooltip `Snoozed until 15:30 · in 2h 30m`; jumps to the Snoozed row |
| SM-D02 | due row `**1530**`, now 16:00 | amber clock `⏰ 15:30 · due`; tooltip ends `due — waking` |
| SM-D03 | now exactly 15:30 (`HHMM <= now` is due), In Progress task | amber due clock, same strings as SM-D02 |
| SM-D04 | Blocked task with a live blocked row | muted clock `⏰ 15:30`; tooltip ends `waits on prerequisites` |
| SM-D05 | task also has a live ledger link under an open Pomodoro | no clock; 🍅 wins and jumps to the Task Link |
| SM-D06 | Ready target (`ready` reason) | no clock; a release stays final |
| SM-D07 | unresolvable target (`missing`) | no clock |
| SM-D08 | Event Task target (`event_task`) | no clock |
| SM-D09 | `^email-dan` ends two lines of `job.md` | no clock; ambiguous identity is neutral |
| SM-D10 | closed task (`done`) | no clock; flips on the same frame |
| SM-D11 | host is the canonical daily note | no clock; daily notes never carry definition clocks |
| SM-D12 | task line rendered inside a daily note | no clock |
| SM-D13 | cache not ready (startup, date rollover) | no clock |
| SM-D14 | `toggle-snooze-marks` off | no clock; source text stays readable |
| SM-D15 | Today-marks toggle off, snooze toggle on | no clock; the shared slot honors both toggles |
| SM-D16 | both toggles off | no clock |
| SM-D17 | row link spelled `[[job#^email-dan\|Dan]]` | same clock as SM-D01; spelling never matters |
| SM-D18 | row link spelled `[[#^email-dan]]` | same clock as SM-D01 |
| SM-D19 | `#task` only inside inline code | no clock |
| SM-D20 | task line inside a fence | no clock |

### SM-W — woken pills

Default: open `WRITE` entry, unstruck plain link `[[job#^email-dan]]`, one
struck history row (`line 8`, `woke 15:32` on `**1530**`), resolved open
task, now 16:00.

| ID | Setup | Result |
| --- | --- | --- |
| SM-W01 | age 28 minutes | fresh warm pill `⏰ woke 15:32`; tooltip `Woke 15:32 (snoozed until 15:30, 2m late)`; jumps to line 8 |
| SM-W02 | age 59 minutes | fresh, same shape as SM-W01 |
| SM-W03 | age exactly 60 minutes | settled quiet outline; same text |
| SM-W04 | age 61 minutes | settled quiet outline |
| SM-W05 | woke time later than now (negative age) | no pill; neutral |
| SM-W06 | woke 15:28, age 32 | fresh; tooltip `2m early` |
| SM-W07 | woke 15:30 | fresh; tooltip `on time` |
| SM-W08 | note is 2026-11-04, now next morning | quiet history pill, never fresh, no countdown; jumps to the old note's row |
| SM-W09 | entry is closed | no pill; closed entries carry 🍅 history |
| SM-W10 | link target is done | no pill |
| SM-W11 | history target is ambiguous | no pill |
| SM-W12 | history target is missing | no pill |
| SM-W13 | two history rows, `woke 15:32` then `woke 15:35` | pill shows the latest wake, `⏰ woke 15:35`, jumping to line 10 |
| SM-W14 | two history rows with equal `woke 15:32` | pill shows `⏰ woke 15:32`, jumping to the later source line 10 |
| SM-W15 | re-snoozed: struck row replaced by a new active row | no pill |
| SM-W16 | `toggle-snooze-marks` off | no pill |
| SM-W17 | embedded ledger link | no pill; embeds never qualify |
| SM-W18 | link targets an Event Task | no pill; Event Tasks never snooze |

### SM-R — Snoozed rows and heading summaries

Paint default: active row `- **1530** [[job#^email-dan]]`, Next task,
resolved, now 13:00.

| ID | Setup | Result |
| --- | --- | --- |
| SM-R01 | asleep, now 13:00 | indigo time pill `15:30` plus faint live `in 2h 30m` countdown |
| SM-R02 | asleep row, now 16:00 | amber `due · waking`, no countdown |
| SM-R03 | Blocked task | muted `waits on prerequisites` |
| SM-R04 | struck history row | dimmed `woke 15:32` |
| SM-R05 | Ready row | muted `Ready — won't wake` |
| SM-R06 | missing target | muted `Missing — won't wake` |
| SM-R07 | duplicate loser | muted `Duplicate — earliest wins`, never counted |
| SM-R08 | row-like text inside a fence | no paint; never executed |
| SM-R09 | indented row-like text | no paint; a child note, not a row |
| SM-R10 | aliased link `[[job#^email-dan\|Dan]]` | same asleep paint as SM-R01 |
| SM-R11 | local link `[[#^email-dan]]` | same asleep paint as SM-R01 |
| SM-R12 | `toggle-snooze-marks` off | no paint; source stays readable |
| SM-R13 | note is 2026-11-04 | neutral historical paint, time pill only, no countdown, no today summary |
| SM-R14 | two asleep rows `1530`, `1900` | heading `2 asleep · next 15:30` |
| SM-R15 | asleep `1530`, due `1600`, blocked `1700`, now 16:30 | heading `1 asleep · 1 due · 1 blocked · next 15:30` |
| SM-R16 | one blocked row only | heading `1 blocked`, no `next` |
| SM-R17 | history and ignored rows only | no live heading summary |
| SM-R18 | toggle off with live rows | no paint and no summary |

### SM-P — plan chip meter

Default: today count 9, ledger links 8, cap 10, queue of two on-clock asleep
rows (`1530`, `1900`), now 13:00.

| ID | Setup | Result |
| --- | --- | --- |
| SM-P01 | two asleep on-clock rows | chip `⏰ 2 · 15:30`, indigo; tooltip `2 snoozed · next wakes 15:30 · not counted in TODAY · 9/11 when all wake`; forecast 10/10, no lint |
| SM-P02 | one asleep plus one due row, now 16:30 | amber chip; tooltip names `1 due`; forecast 10/10 |
| SM-P03 | one blocked row only | chip `⏰ 1` (no time); tooltip names `1 blocked`, no next wake; forecast 8/10 |
| SM-P04 | two asleep rows, both off-clock | chip still shows with time; forecast 8/10 — off-clock rows never join the addition |
| SM-P05 | empty queue | no chip |
| SM-P06 | ignored and history rows only | no chip; an empty planned set has no chip |
| SM-P07 | links plus on-clock addition exactly at cap (10/10) | chip shows; exactly at cap is fine, no lint |
| SM-P08 | links 9 plus 2 on-clock (11/10) | chip shows, never cap-colored; lint `snooze_wake_would_exceed_cap`; wake still runs |
| SM-P09 | three rows, two sharing one identity | count 2; a duplicate loser is never counted |
| SM-P10 | queue plus an already-linked row | count unchanged; live links win and leave the plan |
| SM-P11 | one due plus one blocked row, now 16:30 | amber `⏰ 2 · 16:00`; tooltip names both; forecast 9/10 — Blocked never feeds the timer or the addition |
| SM-P12 | single asleep row, today 4, links 3 | chip `⏰ 1 · 15:30`; tooltip `1 snoozed · next wakes 15:30 · not counted in TODAY · 4/5 when all wake` |
| SM-P13 | `toggle-snooze-marks` off with a live queue | chip still shows; the meter reports counts with all paint off |
| SM-P14 | missing daily note | no chip |

## JavaScript mirror

bob-ledger-tools implements this contract in fresh sibling fragments with
thin hooks into the existing near-limit files: shared snooze display and
model helpers plus a runtime fragment; definition adapters beside the Today
`143`/`275`/`276`/`277` machinery; woken-link fragments beside the
Event-link-time `291d`/`291e`/`291f` machinery; separate row
model/editor/reading fragments; and a small plan helper beside
`290-plugin-today-and-location.js`. Every hand-edited fragment stays at or
below 1,000 lines. The bundle regenerates with `npm run build`; never edit
the generated bundle. New tests join `package.json`'s `test:unit` list; new
helpers may stay private or join the test harness, with no new public
namespace.

Parity is byte-for-byte: each plugin phase copies
`tests/fixtures/snooze/marks.json` from the bob-cli checkout into its
fixture directory unchanged and executes its group's inputs and expected
values verbatim, resolving `BOB_CLI_DIR` to that checkout. No test
substitutes simplified local data for the canonical fixture.

## Deferred

Nothing is deferred: the visual-proof phase verifies this contract in real
Obsidian (light and dark, Live Preview and Reading view) and attaches the
evidence to its own bead.

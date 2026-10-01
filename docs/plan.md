# Plan budget, Today, and lanes

By default, the plan budget allows 3 themes and 10 distinct Task Links
under today's open Pomodoros. GTD is exempt. The first open, non-exempt
Pomodoro is the highlight. Separately, Today lists the open tasks with
a dedicated Task Link under today's open Pomodoros, and the NEXT and
PENDING lanes count every `[*]` and `[/]` task visible today (caps 15
and 10). Removing a Task Link never changes a task's lane: only an
explicit release (Alt+N) returns Next or Pending work to Ready.

For a typical day, capture or link work into the daily Pomodoro ledger,
inspect it with read-only `bob plan`, then run `bob task-status-hooks`
after capture or session close to reconcile task statuses and clean up
links. Run `bob plan` again for counts after that cleanup: the budget
included in a hooks run describes the ledger as it was before the run.

This page is the authoritative definition. The Rust engine
(`src/native/plan_budget/`) implements it; `bob-ledger-tools` mirrors
it in JavaScript, and every other surface calls one of those two
instead of re-implementing it.

## The ledger

The ledger is the lines of today's daily note (`YYYY/YYYYMMDD.md`)
from a `## Pomodoros…` heading up to the next `## ` heading. Fenced
code blocks are skipped.

## Entries

Column-0 checkbox list items (`- [c] …`). An entry is **open** when
`c` is not `x`, `X` or `-`. Only open entries count; completed and
cancelled entries are history.

## Names

An entry's name is the text after `—` (em dash) that follows the
leading `()` placeholder or time range. A merged name (`BOB + DECKS`)
splits on `+` into **components**. Components compare
case-insensitively after collapsing whitespace.

## Exempt entries

Components listed in `plan.exempt` (default `[GTD]`) are never
themes. An entry whose components are all exempt is ignored for
links too.

## Themes

The number of distinct non-exempt components across open entries. An
**unnamed** open entry counts as one theme, shown as `(unnamed)`,
but only when it holds at least one counted link. An empty `()`
placeholder never counts.

## Links

The number of distinct `(target, block_id)` pairs among block links
on indented lines below an open, non-exempt entry, up to the next
column-0 entry, heading, or prose line. Indented prose lines count too;
the budget checks link syntax but does not look up the target task.
Accepted forms:

- `[[target#^id]]`, `![[target#^id]]`, and `[[target#^id|alias]]`;
- with or without 🍅 markers;
- with or without a trailing `#` move-only marker.

Excluded:

- links inside `~~…~~` (struck);
- links in fenced code.

The target is compared with any `.md` suffix removed. An empty
target means the daily note itself. A link planned under two entries
counts once.

## Highlight and running

**Highlight** is the first open non-exempt entry in ledger order.
**Running** is the open entry whose body starts with a time range.

## Status

`over` when themes > `max_themes` or links > `max_links`; otherwise
`ok`. Being exactly at the cap is fine.

## Lints

Each lint has a stable code, a message, and a 1-based `line` when
one applies:

| Code | When |
| --- | --- |
| `plan_theme_cap_exceeded` | themes are over the cap |
| `plan_link_cap_exceeded` | links are over the cap |
| `duplicate_open_pomodoro_name` | the same non-exempt component is on two open entries |
| `inventory_label_open` | an open component is in `plan.inventory_labels`; it still counts as a theme |
| `subheading_in_pomodoros` | a `###`–`######` heading sits inside the section; it splits the heading's time total |
| `next_cap_exceeded` | NEXT is over the cap; never changes `status`, nothing is refused |
| `pending_cap_exceeded` | PENDING is over the cap; never changes `status`, nothing is refused |
| `ready_cap_exceeded` | READY is over the cap; Obsidian `bob-plan` block only (`bob plan` has no READY count); never changes `status`, nothing is refused |
| `note_ready_cap_invalid` | a note's `ready_cap` is not `1–999` or `off`; emitted once per note, then the default cap applies |
| `note_ready_in_terminal_project` | a done/canceled project still holds counted Ready tasks; not capped, listed under "not capped" |
| `today_link_unresolved` | a ledger link resolves to no countable task (missing or ambiguous note, unreadable file, or no open task behind the block ID) |

## Today

Today is the open tasks with a dedicated Task Link under today's
**open** Pomodoros, computed at read time. It is never a tag, a
task-line field, or a file-path filter.

1. **Ledger and open entries** are exactly the plan budget's:
   today's daily note (`YYYY/YYYYMMDD.md`), its `## Pomodoros…`
   section, column-0 checkbox entries; an entry is open unless its
   status is `x`, `X`, or `-`.
2. **Every open entry counts**, including exempt ones (GTD) and
   unnamed placeholders.
3. **Only dedicated Task Links count:** a direct child bullet, at
   the entry's first child indentation, whose body after stripping
   🍅 markers is exactly one plain `[[target#^id]]` or embedded
   `![[target#^id]]` block link, not struck through and not fenced.
   Deeper descendants and mixed-text bullets do not count. This is
   exactly `list_queued_links` in
   `src/native/capture_pomodoro_start.rs` (the `=x` / start lineup
   rule).
4. **Resolution:** an empty target is the daily note itself;
   otherwise the hooks' rules apply (exact vault-relative path with
   or without `.md`, then a unique case-insensitive basename).
   Unresolved or ambiguous links are skipped; Rust reports them as
   the lint `today_link_unresolved`. JavaScript uses
   `metadataCache.getFirstLinkpathDest(target, dailyPath)`;
   ambiguous basenames are a documented divergence, and no
   conformance vector uses one.
5. **Today's tasks** are the resolved `#task` lines with that block
   ID whose status is open (Ready, Next, In Progress, or Blocked);
   done and cancelled tasks drop out. Deduplicate by (path, block
   ID), keeping the ledger order of first occurrence. The key is
   `"<vault path with .md>#<block id>"`.
6. Transcluded dependencies do **not** inherit Today; the hooks
   still promote them to Next.

## Lanes (NEXT and PENDING)

The daily lane review in [`docs/freshness.md`](freshness.md) §4 walks
Pending and Next tasks once a day on their lane interval.

**NEXT** is every `[*]` task and **PENDING** every `[/]` task that
the dash's defaults show: not done, not dependency-blocked, not
`#hide` (case-insensitive substring, so `#hide/x` and `#Hide` are out),
not under `_templates`, not under `_conflicts` (both case-insensitive),
not in `dash.md` itself for dashboard sections, and no scheduled date
after today. Lane visibility is shared by the dashboard sections and
the whole-lane budgets through one tested base predicate; status and
TODAY are layered on top (PENDING uses `IN_PROGRESS`, NEXT uses symbol
`*`).

A NEXT task is a `#task` line that bob-cli's native-internal lane query
matches with this query (`status.symbol is` is native-internal syntax, not
valid Obsidian Tasks syntax):

```text
not done
status.symbol is *
is not blocked
tags do not include #hide
folder does not include _templates
path does not include _conflicts
(no scheduled date) OR (scheduled on or before today)
```

The native-internal PENDING query is identical with `status.symbol is /`;
the dashboard PENDING block uses `status.type is IN_PROGRESS` instead.
The dashboard NEXT block uses
`filter by function task.status.symbol === "*"` (Tasks 8.4.0 syntax for the
same `[*]` selection). This matches the dash's own defaults.

The dashboard and the whole lane differ on TODAY by design. The
dashboard PENDING/NEXT sections exclude TODAY (plus `dash.md` itself);
`bob plan`, navigation notices, native CLI parity, and other callers
keep the **whole lane, Today included**, so those counts don't swing
during the day. The dashboard badges show the **section count over the
whole-lane cap** as the primary number (for example `PENDING 49/10`),
red when the whole lane exceeds the cap, with the whole-lane pressure
in the tooltip and accessible label (for example `49 in this section;
whole lane 50/10; 1 in TODAY`); cap warnings still use the full lane.
In the rare case where section <= cap < lane, the badge can read, for
example, `NEXT 15/15` in red.
`dashboardLaneBudget("pending" | "next")` in bob-ledger-tools is the
versioned dashboard contract. When Tasks data or the current-day Today
cache is not ready, the dashboard section is unavailable (`–`), never a
silent zero; a plugin too old for that API keeps the inline fallback,
which applies the same base visibility and whatever TODAY predicate is
available.

## READY backlog (dashboard and daily badge)

**READY** is the freshness-gated global backlog the `dash.md` READY
chip and the daily `bob-plan` READY badge share: the visible TODO pool
(see `docs/freshness.md` §4) minus the NEW and ROTTEN review buckets,
regardless of which daily file hosts the badge. It holds recently
human-confirmed tasks plus the preexisting freshness-exempt Ready tasks.
It excludes completed, cancelled, and non-task entries; template and
`_conflicts` paths; `dash.md` itself (ordinary ready tasks living in a
daily note remain eligible); `#hide`; dependency-blocked tasks
(`isBlocked` against the full Tasks list); future-scheduled tasks
(unscheduled and scheduled on or before the current local day count);
Today tasks (`isToday`); and any task whose read-time bucket is `new`
or `rotten`. A null bucket alone never proves a task is Ready. The
count is the Obsidian dashboard backlog and its feedback is the shared
badge; this feature adds no native READY count, `bob plan` lint, capture
enforcement, or tmux meter. The daily `bob-plan` block shows a
`ready_cap_exceeded` lint line beneath its chips when READY is strictly
over the cap. A cap exactly met raises no lint.

The badge shows `READY n/cap` (for example `READY 87/100`); exactly at
the cap is fine and only a strict excess turns red (`READY 101/100`).
There is no intermediate warning color. This is a **soft limit**: it
communicates backlog pressure without refusing capture, changing task
statuses, or removing tasks. The cap now bounds only the
confirmed/exempt pullable backlog: skipping review can lower READY
without lowering total lane pressure (`NEW + ROTTEN + READY`). The
tooltip carries the whole lane, for example
`READY 120/100 · lane 210 = 3 new + 87 rotten + 120 ready`. ROTTEN is
the counterweight: `ROTTEN 31 · ✓ 12` (or `✓ 12/15` with a budget).
Clicking the badge opens `dash#READY Tasks`. The badge on an older
daily file is a **live current backlog**, not a historical snapshot:
Today exclusion and scheduled-date eligibility use the current day even
when the PLAN portion describes that older file's ledger. `READY –`
means Tasks data, required Today state, or the count is unavailable;
zero is reserved for a successfully evaluated empty queue.

## Ready cap per note

Every area/project note has a soft cap on its Ready lane
(`plan.max_ready_per_note`, default 5, per-note `ready_cap`
override). Crowded notes are named, counted, and easy to act on:
in `bob ready`, a dash CROWDED chip, and a live chip on each note's
`## Tasks` heading. All surfaces share one read-time contract,
implemented in Rust (`src/native/note_ready/`) and in
bob-ledger-tools and pinned by shared vectors R1–R14.

The cap counts the whole Ready lane by residence, whatever its
freshness: the lane (not the gated READY backlog) is counted
because a gated per-note count would hide work behind the rot
cliff and invert review (skipping review would lower the count
without lowering pressure).

```text
counted(t)  = lane(t) ∧ ¬recurring(t) ∧ block_id(t) ≠ "prj"
lane(t)     = the existing READY_QUERY / readyTaskVisible ∧ ¬planTaskIsBlocked predicate:
              TODO-type status, not done, not dependency-blocked, no #hide tag, not under
              _templates/ or _conflicts/, no scheduled date or scheduled ≤ today.
              Freshness bucket and Today are NOT filters (whole lane).
note(t)     = the vault file that holds t (residence; never parent, heading, embed, backlink)
eligible(n) = type(n) ∈ {[[area]], [[project]]} (quoted, single-quoted, bare, flow-list,
              block-list forms) ∧ (area ∨ status ∉ {done, canceled, cancelled})
              ∧ path not under .git/ .obsidian/ _templates/ _conflicts/ _generated/ done/
cap(n)      = ready_cap integer 1–999 → (cap, source "note")
              | ready_cap off / false → exempt
              | ready_cap present but invalid → lint note_ready_cap_invalid, then default
              | absent → plan.max_ready_per_note (source "config") else 5 (source "default")
count(n)    = |{ t : note(t) = n ∧ counted(t) }|   (each row counts; nested child tasks count)
make_up(n)  = { new, rotten, ready = count − new − rotten } from freshness buckets
              (rotten includes resurfaced; null when freshness is unavailable)
recurring(n)= lane rows in n excluded only because they recur (shown as ↻ k)
state(n)    = exempt | crowded (count > cap) | full (count = cap) | room (0 < count < cap)
              | empty (count = 0); surfaces add "unavailable" when no snapshot exists
totals      = notes (eligible, not exempt) with areas/projects split, crowded, full, room,
              empty, exempt, counted (Σ count over capped notes),
              excess (Σ max(0, count − cap)), recurring
lint note_ready_in_terminal_project: a done/canceled project still holding counted rows
              (not capped; listed under "not capped")
```

The note-entry field names are shared by CLI JSON and
`api.noteReady`: `path`, `name` (stem), `kind` (`area`|`project`),
`status`, `parent`, `count`, `cap`, `cap_source`
(`note`|`config`|`default`|`preview`), `state`, `over_by`,
`make_up` (`{ready,new,rotten}` or null), and `recurring`. Lints
are `{code, path, message}`, emitted once per note, never once per
row.

**Words:** a note is **crowded**, **full**, or has **room**. An
exempt note has **no cap**. The verbs are always
**split · sequence · defer · drop**. The UI never suggests
"promote to Next".

**States:** room is dim/muted (`ready 3/5`); full is muted with
`· full` text and no amber; crowded is red with `+k`
(`bob-plan-over` in Obsidian); exempt is muted (`ready 65 · no cap`
/ `no cap`); unavailable is `–`, never `0`; fixed is green
(`CROWDED 0 ✓`). Color always comes with text. Numbers use tabular
numerals with a stable width. Obsidian styling uses theme variables
only (no hex), and motion respects `prefers-reduced-motion`. Chips
and rows carry aria labels and tooltips that spell out the count,
for example `11 ready-lane tasks = 11 ready + 0 new + 0 rotten ·
cap 5 (Bob config) · 6 over · split, sequence, defer, or drop`.

**`ready_cap`:** a per-note frontmatter integer `1–999`, or `off`
(case-insensitive) or YAML `false` for exempt. YAML 1.1 parsers read
a bare `off` as `false`. An invalid value emits
`note_ready_cap_invalid` once per note and falls back to the
default.

**Failure behavior:** an invalid global `plan.max_ready_per_note`
exits 2 for `bob plan`/`bob ready` (with a JSON error envelope under
`-f json`); every other caller falls back to defaults. An invalid
per-note `ready_cap` produces a lint and falls back to the default.
A vault I/O failure exits 1. A non-Dataview task format exits 2.
Crowded notes never fail the report. Only `--check` maps them to
exit 3 (in `bob ready`). A missing Tasks plugin or a non-`Warm`
Tasks cache gives `available: false` in the plugin, and every
surface shows `–`. Unavailable freshness gives `make_up: null`;
counts are still shown.

**Mobile:** `config.yml` is unreadable there, so the default cap
applies and the tooltip says so. Per-note `ready_cap` works
everywhere.

**Conformance vectors (copied verbatim into Rust and JS tests):**

| #   | Fixture                                                                                                                                                                                | Expected                                                                                                     |
| --- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| R1  | 6 `[ ]` tasks, default cap                                                                                                                                                             | `count 6`, crowded, `over_by 1`, `excess 1`                                                                  |
| R2  | 5 `[ ]` tasks                                                                                                                                                                          | full, no lint                                                                                                |
| R3  | `#hide`, `#Hide`, `#hide/x`, future `scheduled`, dependency-blocked, `[?]`, `[*]`, `[/]`, `[x]`, `[-]`                                                                                 | none count                                                                                                   |
| R4  | 3 tasks: one unstamped (NEW), one stamped 8 days ago (ROTTEN), one fresh                                                                                                               | `count 3`, make-up `1 ready + 1 new + 1 rotten`                                                              |
| R5  | `[repeat:: every day]` and `🔁` tasks                                                                                                                                                  | not counted; `recurring 2`                                                                                   |
| R6  | a visible `^prj` task                                                                                                                                                                  | not counted                                                                                                  |
| R7  | tasks in a daily note, an untyped note, `_templates/`, `done/`, `dash.md`                                                                                                              | absent from the per-note list                                                                                |
| R8  | `ready_cap: 8` / `off` / `OFF` / `false` / `0` / `lots` / `1000`                                                                                                                       | cap 8 (source `note`) / exempt / exempt / exempt / lint then default / lint then default / lint then default |
| R9  | a parent `[ ]` with 2 child `[ ]` tasks                                                                                                                                                | `count 3`                                                                                                    |
| R10 | `status: done` (and `cancelled`) project with 2 `[ ]` tasks                                                                                                                            | not capped; lint `note_ready_in_terminal_project`                                                            |
| R11 | `type: "[[project]]"`, `type: '[[area]]'`, bare `type: [[project]]` (Obsidian parses it as nested array `[["project"]]`), flow list `["[[area]]"]`, block list, nested folder `x/y.md` | all eligible; Rust and JS agree                                                                              |
| R12 | stamping `[fresh::]` on a NEW task (Alt+Shift+F) in a note at 5                                                                                                                        | count stays 5 (full); make-up moves 1 from new to ready                                                      |
| R13 | a Today-linked `[ ]` task                                                                                                                                                              | counted (whole lane)                                                                                         |
| R14 | `plan.max_ready_per_note: 3`; one note with `ready_cap: 8`                                                                                                                             | other notes cap 3 (source `config`); that note cap 8 (source `note`)                                         |

## Config

```yaml
# Today's plan budget (bob plan, capture, tmux, Obsidian) and the NEXT/PENDING/READY lane caps.
plan:
  max_themes: 3 # distinct open Pomodoro names besides the exempt ones
  max_links: 10 # distinct open Task Links outside exempt entries
  max_next: 15 # open Next tasks visible today (see above)
  max_pending: 10 # open In Progress tasks visible today (see above)
  max_ready: 100 # soft limit for dashboard READY tasks, excluding Today (see above)
  max_ready_per_note: 5 # soft cap per area/project note; frontmatter ready_cap overrides
  strict: false # refuse #NAME captures that would create a theme past max_themes
  exempt: [GTD] # open entries that never count as themes
  inventory_labels: [LATER, MISC, NEW FEATURES, SASE] # open names that are storage, not themes
```

The block is optional. A missing file or a missing block means the
defaults above, shared by Rust and JavaScript. Unknown keys stay
ignored.

**Validation.** Caps are integers ≥ 1 (`max_ready_per_note` is
`1–999`), and the lists hold non-empty strings.

**Invalid values:**

- `bob plan` exits 2 with a clear message.
- Every other surface falls back to the defaults and says so once:
  as a warning string, a stderr line, or a Notice. An invalid plan
  config must never break capture, hooks, tmux, or a keymap.

## `bob plan`

`bob plan` is read-only. Options: `-b/--bob-dir DIR`,
`-f/--format human|json`, `-h/--help`. Environment: `BOB_DIR`,
`BOB_DAY_FILE`, `BOB_NOW`, `BOB_CONFIG_FILE`, `NO_COLOR`.

JSON adds `ok: true` and `schema_version: 2` to the report. This example
shows all report fields for one open theme and one exempt entry:

```json
{
  "date": "2026-10-01",
  "daily_file": "2026/20261001.md",
  "caps": { "max_themes": 3, "max_links": 10, "max_next": 15, "max_pending": 10, "max_ready": 100, "strict": false },
  "status": "ok",
  "themes": { "count": 1, "cap": 3, "over": false },
  "links": { "count": 3, "cap": 10, "over": false },
  "today": { "count": 2 },
  "next": { "count": 12, "cap": 15, "over": false },
  "pending": { "count": 8, "cap": 10, "over": false },
  "today_tasks": [
    {
      "path": "sase.md",
      "block_id": "fix-it",
      "line": 120,
      "status_symbol": "*",
      "status_name": "Next",
      "text": "Fix it",
      "entry_line": 55,
      "entry_name": "BOB",
      "ledger_line": 56
    }
  ],
  "theme_names": ["GOALS"],
  "entries": [
    {
      "line": 24,
      "name": "GOALS",
      "components": ["GOALS"],
      "exempt": false,
      "running": true,
      "highlight": true,
      "time_range": "0945-1015",
      "links": 3
    },
    {
      "line": 30,
      "name": "GTD",
      "components": ["GTD"],
      "exempt": true,
      "running": false,
      "highlight": false,
      "links": 0
    }
  ],
  "warnings": [],
  "ok": true,
  "schema_version": 2
}
```

`theme_names` lists the ordered theme names alongside `entries`.
Lint objects omit `line` when none applies; entries omit
`time_range` when untimed. `today_tasks` is in ledger order of
first occurrence; `today.count` is its length.

Human output:

```text
bob plan · Thu 2026-10-01 · 2026/20261001.md

  PLAN  3/3 themes · 7/10 links      TODAY 7 · PENDING 8/10 · NEXT 12/15

  ★ GOALS    ▶ 0945-1015   3 links
    …

  TODAY
    [*] sase#^fix-it        Fix it
    [/] bob#^capture-stop   Better capture stop
```

Meters are green within the cap and red when over. ★ is yellow and
▶ is cyan; exempt rows are dimmed. When lints occur, they appear below
the entries in yellow, with the code dimmed. An open inventory label such
as MISC appears as a row and counts as a theme even though it also raises
`inventory_label_open`.

With no daily note the header says `no daily note yet`; with no
Pomodoros section it says `no Pomodoros section`. `TODAY 0` and the
lanes are still shown, and the command exits 0. Exit codes: 0 for a
report, 1 for an I/O failure, 2 for usage or an invalid plan config.

## `bob ready`

`bob ready` is read-only. Options: `[NOTE]`, `-a/--all`,
`-b/--bob-dir DIR`, `-n/--cap N`, `-c/--check`, `-f/--format
human|json`, `-h/--help`. Environment: `BOB_DIR`, `BOB_NOW`,
`BOB_CONFIG_FILE`, `NO_COLOR`.

The overview lists every area/project note against its cap: crowded
notes with red overflow bars, full notes up to the cap marker, room
notes as a compact `name count` list, and a footer with the empty
count, the not-capped list, and the recurring total. `-a` expands
room notes into bar rows, lists empty names, adds terminal projects
to the not-capped list, and prints a `LINTS` block in the
`freshness` style. With nothing crowded the summary reads
`CROWDED 0 ✓ · every note has room` in green, and the quickest win
(`Make room → bob ready <note>`, the smallest excess) is omitted.
`--cap N` previews a different default cap for the run only: note
overrides and exemptions still apply, and affected entries report
`preview` as their cap source. `--check` exits 3 when any note is
crowded, still printing the report (JSON included).

`bob ready NOTE` lists one note's lane tasks in file order with
`path:line` references, `new` / `rotten Nd` / `fresh Nd` labels, and
`^block-id` markers, plus an `also here` line (next, pending,
blocked, recurring) and a make-room footer. NOTE resolves as a vault
path (with or without `.md`), then an exact stem, then a
case-insensitive stem. An ambiguous stem exits 2 listing every
candidate; an unknown or non-area/project note exits 2 with up to
three closest-name suggestions.

JSON adds `ok: true`, `schema_version: 1`, `definition:
"ready_lane"`, and the shared note-entry fields (`path`, `name`,
`kind`, `status`, `parent`, `count`, `cap`, `cap_source`, `state`,
`over_by`, `make_up`, `recurring`) to the report. The overview
carries `notes` (every eligible and exempt note, regardless of
`-a`); the worklist carries `note`, `tasks` (`path`, `line`,
`text`, `block_id`, `bucket`, `fresh_on`), and `also` (`next`,
`pending`, `blocked`, `recurring`). Errors use
`{ok: false, schema_version: 1, error: {code, message}}`.

Human output (no ANSI without a TTY or with `NO_COLOR`, same glyphs
and text):

```text
bob ready · Thu 2026-10-01 · cap 5 per note

  CROWDED 1 · 2 over · 1 full · 3 notes (1 area · 2 projects)

  CROWDED
    sase_remote  7/5  ■■■■■│■■  +2   project · parent sase · 6 new
  FULL
    bob  5/5  ■■■■■│  full   project · 5 new
  ROOM
    cash 2

  1 empty · not capped: gkeep_inbox 1 (ready_cap: off) · ↻ 1 recurring   (-a for all)

  Make room → bob ready sase_remote
  split Ctrl+Shift+N · sequence / defer / drop Ctrl+Shift+P
```

Exit codes: 0 for a report (even when notes are crowded), 1 for a
vault I/O failure, 2 for an invalid `plan.max_ready_per_note`, a
non-Dataview task format, or an unresolvable note, and 3 with
`--check` when at least one note is crowded.

## Surfaces

| Surface | What it shows |
| --- | --- |
| `bob plan` | The full plan report: meters, today's themes (★ highlight, ▶ running), the TODAY list, and lint messages with codes |
| `bob ready` | The per-note Ready cap report: crowded/full/room bar overview, the single-note worklist, schema-1 JSON, `--check` (exit 3), and `--cap` preview |
| Daily note with a `bob-plan` code block | The Bob Ledger Tools plugin (api v3 with freshness namespace v3: `isToday`, `todayRank`, `nextBudget`, `pendingBudget`, `dashboardLaneBudget`, `renderDashboardLaneBadge`, `readyBudget`, `renderReadyBadge`, `renderReviewChip`, `freshness.reviewModel`) renders TODAY, PENDING, NEXT, READY chips, a theme line, and any lints. TODAY is the theme/link budget (`TODAY 3/3 · 7/10`, or `TODAY –` with no Pomodoros section). PENDING, NEXT, and READY show `–` when their data is unavailable; the shared READY badge is the freshness-gated live current backlog (`READY n/100` with a whole-lane tooltip) that opens `dash#READY Tasks` and never changes the PLAN status. Daily PENDING/NEXT keep whole-lane `pendingBudget`/`nextBudget`; only the dashboard uses the section budgets. |
| `dash.md` | Its NEW, PENDING, NEXT, READY, BLOCKED, ROTTEN, and TODAY chips in that order and mutually exclusive TODAY / NEW / PENDING / NEXT / READY sections (section order TODAY → NEW → PENDING → NEXT → READY) use the Bob Ledger Tools api v3 with freshness namespace v3 (`dashboardLaneBudget`/`renderDashboardLaneBadge` for PENDING/NEXT sections, `readyBudget`/`renderReadyBadge` for gated READY plus `renderReviewChip`/`reviewModel` for NEW/ROTTEN, with a guarded inline fallback when the plugin is older or unloaded). PENDING/NEXT badges show the section count with the whole-lane pressure in the tooltip; non-dashboard `pendingBudget`/`nextBudget` keep whole-lane semantics. TODAY is that same theme/link budget and opens today's daily note. |
| `bob tmux-pomodoro` | Appends `plan T/Tc · L/Lc` to an available Pomodoro status (or shows the meter alone). It requires a daily note with a Pomodoros section; an over-cap meter uses tmux reverse video. |
| `bob task-status-hooks` | A `plan_budget` object in JSON and a human meter line such as `plan 3/3 themes · 7/10 links · TODAY 7 · PENDING 8/10 · NEXT 12/15`, when the daily note has a Pomodoros section and the plan config is valid. The meter describes the ledger before sync cleanup. |
| `bob capture` | When a capture changes today's Pomodoros section, a before/after theme and link budget, cap warnings if the count grows over a cap, and the Task Link destination (for example `→ under GOALS (next up)`). Strict mode can refuse a new over-cap theme. |
| Bob Mac Capture | The same budget in Themes and Links capsules, warning captions, and shorter destination rows such as `→ GOALS · next up` or `→ running GOALS 0945–1015`. |
| Obsidian Notices | A lane-aware suffix on Task Link changes. Ctrl+Shift+Enter link and unlink Notices append the plan meter, for example `Linked · Next · plan 1/3 · 2/10` (🔴 when over a plan cap); Alt+N lane Notices report the lanes, for example `→ Ready · 2 tasks · unlinked 1 from today · NEXT 11/15 · PENDING 7/10`, with 🔴 plus a prune hint when over a lane cap. |

## Conformance examples

The vectors below are shared with the `bob-ledger-tools`
JavaScript mirror as its test vectors. Caps are the defaults
unless noted.

1. **Merged name with duplicate.** `BOB + DECKS` next to a separate
   `DECKS`: DECKS counts once and raises
   `duplicate_open_pomodoro_name`.

   ```markdown
   ## Pomodoros

   - [ ] () — BOB + DECKS
       - [[a#^one]]
   - [ ] () — DECKS
       - [[b#^two]]
   ```

   Themes are `BOB`, `DECKS` (2/3); links are 2/10; one
   `duplicate_open_pomodoro_name` at the second entry's line.

2. **Link forms and exclusions.** A struck link, GTD's `[[#^gtd]]`,
   a link planned twice, an embedded link, and a `#`-marked link:

   ```markdown
   ## Pomodoros

   - [ ] () — GOALS
       - [[task#^aaa]] and ~~[[task#^struck]]~~
       - ![[emb#^eee]]
       - [[mark#^mmm]]#
   - [ ] () — DECKS
       - [[task#^aaa]]
   - [ ] () — GTD
       - [[#^gtd]]
   ```

   Links are `(task, aaa)`, `(emb, eee)`, `(mark, mmm)`: 3/10. The
   struck link never counts, the twice-planned link counts once,
   and GTD's link is ignored with the entry.

3. **Unnamed placeholders.** An unnamed entry with links counts as
   one `(unnamed)` theme; one without links never counts:

   ```markdown
   ## Pomodoros

   - [ ] ()
       - [[solo#^one]]
   - [ ] ()
   - [ ] () — GOALS
   ```

   Themes are `(unnamed)`, `GOALS` (2/3); links are 1/10.

4. **Subheading in the section.** A `### Notes` heading raises
   `subheading_in_pomodoros` and closes the entry span above it:

   ```markdown
   ## Pomodoros

   - [ ] () — GOALS
       - [[task#^aaa]]

   ### Notes

   - [[task#^bbb]] is just prose, not a link under GOALS
   ```

   Links are 1/10: the prose link sits outside any entry span.

5. **Open inventory label.** An open `LATER` still counts as a
   theme and raises `inventory_label_open`:

   ```markdown
   ## Pomodoros

   - [ ] () — LATER
       - [[task#^aaa]]
   ```

   Themes are 1/3 with one `inventory_label_open`.

6. **Over the cap.** Four themes with the default cap raises
   `plan_theme_cap_exceeded` and sets status `over`:

   ```markdown
   ## Pomodoros

   - [ ] () — ONE
   - [ ] () — TWO
   - [ ] () — THREE
   - [ ] () — FOUR
   ```

   Themes are 4/3; being exactly at the cap is fine, going past
   it is not.

7. **Cancelled and completed entries.** A `[-]` entry and a
   completed entry never count:

   ```markdown
   ## Pomodoros

   - [-] () — GONE
       - [[task#^aaa]]
   - [x] () — DONE
       - [[task#^bbb]]
   - [ ] () — GOALS
       - [[task#^ccc]]
   ```

   Themes are `GOALS` (1/3); links are 1/10.

8. **Daily-note target aliases.** When evaluating `2026/20260930.md`,
   these three links count as one because they have the same block ID:

   ```markdown
   ## Pomodoros

   - [ ] () — GOALS
       - [[#^aaa]]
       - [[2026/20260930#^aaa]]
       - [[20260930#^aaa]]
   ```

   Links are 1/10.

## Today conformance examples

Each vector gives the ledger, the notes it resolves against, and
the expected ordered Today keys (`"<vault path with .md>#<block
id>"`). Caps are the defaults unless noted. The vectors are shared
with the `bob-ledger-tools` JavaScript mirror as its test vectors.

- **T1 GTD.** An exempt entry's empty-target link resolves to the
  daily note itself:

  ```markdown
  ## Pomodoros

  - [ ] () — GTD
      - [[#^gtd]]
  ```

  with `- [ ] #task Gtd chore ^gtd` in `2026/20261001.md` →
  `2026/20261001.md#gtd`.

- **T2 markers.** `🍅 [[a#^x]]` counts, `~~[[a#^y]]~~` doesn't,
  `![[a#^z]]` counts:

  ```markdown
  ## Pomodoros

  - [ ] () — GOALS
      - 🍅 [[a#^x]]
      - ~~[[a#^y]]~~
      - ![[a#^z]]
  ```

  → `a.md#x`, `a.md#z`.

- **T3 closed entries.** Links under `[x]` and `[-]` entries don't
  count:

  ```markdown
  ## Pomodoros

  - [x] () — DONE
      - [[a#^x]]
  - [-] () — GONE
      - [[a#^y]]
  - [ ] () — OPEN
  ```

  → no keys.

- **T4 shapes.** A mixed bullet `Review [[a#^m]]` and a link nested
  under a note bullet don't count:

  ```markdown
  ## Pomodoros

  - [ ] () — GOALS
      - Review [[a#^m]]
      - Note text
          - [[a#^deep]]
  ```

  → no keys.

- **T5 dedupe.** One task under two open entries, and `[[a#^x]]`
  plus `[[dir/a#^x]]` resolving to the same note → one key at its
  first position:

  ```markdown
  ## Pomodoros

  - [ ] () — ONE
      - [[a#^x]]
  - [ ] () — TWO
      - [[a#^x]]
      - [[dir/a#^x]]
  ```

  with only `dir/a.md` holding `^x` → `dir/a.md#x` at the first
  entry's position.

- **T6 fenced.** A link inside a fenced block doesn't count:

  ````markdown
  ## Pomodoros

  - [ ] () — GOALS
      - [[a#^x]]

  ```
  - [[a#^q]]
  ```
  ````

  → `a.md#x`.

- **T7 status.** Linked `[x]` and `[-]` tasks drop out; `[?]`
  stays:

  ```markdown
  ## Pomodoros

  - [ ] () — GOALS
      - [[a#^x]]
      - [[a#^y]]
      - [[a#^z]]
      - [[a#^w]]
  ```

  with `- [x] #task Done ^x`, `- [-] #task Gone ^y`,
  `- [?] #task Waiting ^z`, `- [ ] #task Open ^w` in `a.md` →
  `a.md#z`, `a.md#w`, and no lint.

- **T8 unresolved.** `[[missing#^q]]` gives no key, plus
  `today_link_unresolved` at the link's ledger line:

  ```markdown
  ## Pomodoros

  - [ ] () — GOALS
      - [[missing#^q]]
  ```

  → no keys, one `today_link_unresolved`.

- **T9 alias.** `[[a#^x|alias]]` counts, pinned to whatever
  `list_queued_links` does with aliases:

  ```markdown
  ## Pomodoros

  - [ ] () — GOALS
      - [[a#^x|alias]]
  ```

  → `a.md#x`.

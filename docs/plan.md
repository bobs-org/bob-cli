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

## Config

```yaml
# Today's plan budget (bob plan, capture, tmux, Obsidian) and the NEXT/PENDING/READY lane caps.
plan:
  max_themes: 3 # distinct open Pomodoro names besides the exempt ones
  max_links: 10 # distinct open Task Links outside exempt entries
  max_next: 15 # open Next tasks visible today (see above)
  max_pending: 10 # open In Progress tasks visible today (see above)
  max_ready: 100 # soft limit for dashboard READY tasks, excluding Today (see above)
  strict: false # refuse #NAME captures that would create a theme past max_themes
  exempt: [GTD] # open entries that never count as themes
  inventory_labels: [LATER, MISC, NEW FEATURES, SASE] # open names that are storage, not themes
```

The block is optional. A missing file or a missing block means the
defaults above, shared by Rust and JavaScript. Unknown keys stay
ignored.

**Validation.** Caps are integers ≥ 1, and the lists hold non-empty
strings.

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

## Surfaces

| Surface | What it shows |
| --- | --- |
| `bob plan` | The full plan report: meters, today's themes (★ highlight, ▶ running), the TODAY list, and lint messages with codes |
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

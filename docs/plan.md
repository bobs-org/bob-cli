# Plan budget and NOW

Today is closed: GTD plus at most 3 themes. The first theme is the
highlight. Nothing is copied forward by default. This week is `#now`:
at most 15 tasks. Dropping a link from today never loses it, because
`#now` keeps it in view. Everything else is READY (next) or deferred
with a P-level (later).

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
in any descendant bullet of an open, non-exempt entry. Accepted
forms:

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
| `now_cap_exceeded` | NOW is over the cap; only on surfaces that compute NOW |

## NOW (this week's bets)

A NOW task is a `#task` line that the native Tasks engine (the one
behind `bob query --tasks`) matches with this query:

```text
not done
tags include #now
is not blocked
tags do not include #hide
folder does not include _templates
path does not include _conflicts
(no scheduled date) OR (scheduled on or before today)
```

This matches the dash's own defaults, so the chip, the `### NOW
Tasks` section and `bob plan` always agree. `has_now_tag(text)`
means the case-sensitive whole token `#now`: preceded by the line
start or whitespace, followed by the end or whitespace. `#now` is
**never** a Next source and never changes task status.

## Config

```yaml
# Today's plan budget (bob plan, capture, tmux, Obsidian) and the weekly #now cap.
plan:
  max_themes: 3 # distinct open Pomodoro names besides the exempt ones
  max_links: 10 # distinct open Task Links outside exempt entries
  max_now: 15 # open #now tasks visible today (see above)
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

JSON is `{"ok": true, "schema_version": 1, …report…}` where the
report looks like this:

```json
{
  "date": "2026-09-30",
  "daily_file": "2026/20260930.md",
  "caps": { "max_themes": 3, "max_links": 10, "max_now": 15, "strict": false },
  "status": "ok",
  "themes": { "count": 3, "cap": 3, "over": false },
  "links": { "count": 7, "cap": 10, "over": false },
  "now": { "count": 12, "cap": 15, "over": false },
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
    }
  ],
  "warnings": [
    {
      "code": "inventory_label_open",
      "message": "MISC is an inventory label, not a theme",
      "line": 60
    }
  ]
}
```

`theme_names` lists the ordered theme names alongside `entries`.
Lint objects omit `line` when none applies; entries omit
`time_range` when untimed.

Human output:

```text
bob plan · Wed 2026-09-30 · 2026/20260930.md

  PLAN  3/3 themes · 7/10 links        NOW  12/15

  ★ GOALS    ▶ 0945-1015   3 links
    DECKS                  2 links
    BOB                    2 links
    GTD      exempt        0 links

  ⚠ MISC is an inventory label, not a theme (line 60)  inventory_label_open
```

Meters are green within the cap and red when over. ★ is yellow and
▶ is cyan; exempt rows are dimmed. Lints are yellow, with the code
dimmed.

With no daily note the header says `no daily note yet`; with no
Pomodoros section it says `no Pomodoros section`. NOW is still
shown, and the command exits 0. Exit codes: 0 for a report, 1 for
an I/O failure, 2 for usage or an invalid plan config.

## Surfaces

| Surface | What Bryan sees |
| --- | --- |
| `bob plan` | The full plan report: meters, today's themes (★ highlight, ▶ running), and lints with hints |
| Daily note, above `## Pomodoros` | A live ` ```bob-plan ` block: `PLAN 3/3 · 7/10`, `NOW 12/15`, then `★ GOALS · DECKS · BOB` |
| `dash.md` chip bar | `NOW 12/15` and `PLAN 3/3 · 7/10` chips, red when over the cap; a `### NOW Tasks` section |
| tmux status line | `<status> · plan T/Tc · L/Lc \| ` (for example `0945-1015 — GOALS · plan 3/3 · 7/10 \| `), reverse video when over |
| `bob task-status-hooks` | A `plan_budget` JSON block and one human line (`plan 3/3 themes · 7/10 links · NOW 12/15`) |
| `bob capture` / Bob Mac Capture | Where the Task Link lands (`→ under GOALS (next up)`, `→ into running GOALS (0945-1015)`, `→ new Pomodoro BOB`), a before→after meter, cap warnings, and optional strict refusal |
| Obsidian Notices | `· plan 3/3 · 11/10 🔴` on Ctrl+Shift+Enter; `#now added · NOW 13/15` on the toggle |

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

8. **Empty target means the daily note.** In `2026/20260930.md`,
   these three targets are the same link when the block ID matches:

   ```markdown
   ## Pomodoros

   - [ ] () — GOALS
       - [[#^aaa]]
       - [[2026/20260930#^aaa]]
       - [[20260930#^aaa]]
   ```

   Links are 1/10.

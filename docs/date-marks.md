# Task date marks

This guide is the authoritative display contract for task date marks,
in the style of `docs/freshness.md` §11 and the `docs/projects.md`
"Priority marks" section. The JavaScript mirror is `api.dateMarks`
(namespace v1, top-level api v3) in bob-ledger-tools; its tests run
the conformance vectors below verbatim.

## What you see

Every canonical `created`, `scheduled`, `completion`, and `cancelled`
task date renders as a small monochrome icon plus a calendar label
instead of a `KEY | YYYY-MM-DD` Dataview pill:

```text
before: - [ ] Add ability to install agent CLIs  CREATED│2026-08-31  SCHEDULED│2026-09-04
after:  - [ ] Add ability to install agent CLIs  + Aug 31  ⧗ Sep 4
```

More examples (glyphs approximated in plain text):

```text
- [?] Ship the importer  + yesterday  ⧗ Fri
- [ ] Rename the queue input  + Sep 29  ⧗ today          ← scheduled today reads a touch stronger
- [x] Review the /sase_memory_write skill  + Aug 29  ✓ Sep 3   ← closed tasks rest (faint)
- [-] Old idea  + Mar 2  ⊘ today
```

Hovering any mark shows the exact date and its distance, for example
`Scheduled Fri, Oct 9 · in 2 days` followed by
`Ctrl+Shift+P to reschedule`. Moving the cursor into a mark, or
clicking it, reveals `[scheduled:: 2026-10-09]` for editing.

## Principles

Display-only: `[key:: YYYY-MM-DD]` stays the only stored form;
nothing writes a date mark, and the Rust, Tasks, and Dataview
semantics are unchanged. Store absolute, show relative: near dates
read as words, and the tooltip always has the exact date. Shape
carries meaning; color whispers: each field has its own silhouette,
ink is monochrome, and emphasis comes from weight and opacity, never
hue. Quiet by default, louder only when it matters: `created` is the
quietest mark, and a scheduled date arriving today is the only
emphasized state. Reversible: the cursor or a click reveals the raw
field, source mode shows raw text, a session toggle restores the
pills, and without bob-ledger-tools the vault looks exactly as it
does today. One glyph definition (CSS masks) shared by every
surface.

## Fields and glyphs

The four glyphs are refined, monochrome versions of the Tasks emoji
already seen in query results (`➕ ⏳ ✅ ❌`). The one deliberate
change is cancelled: `⊘` replaces `×`, because a bare `×` beside the
created `+` reads as a pair of math operators (and as a close
button).

| Stored key   | Tooltip verb | Glyph            | Tasks emoji it replaces |
| ------------ | ------------ | ---------------- | ----------------------- |
| `created`    | `Created`    | plus             | ➕                      |
| `scheduled`  | `Scheduled`  | hourglass        | ⏳                      |
| `completion` | `Done`       | check            | ✅                      |
| `cancelled`  | `Cancelled`  | circle-slash (⊘) | ❌                      |

`due` and `start` are out of scope: the vault uses `start` for clock
times and `due` only 3 times. Adding one later is a new row here, a
new glyph, and a new key in the repair-flag selector.

Final geometry (viewBox `0 0 16 16`, black stroke,
`stroke-width 1.9`, `stroke-linecap`/`stroke-linejoin` `round`,
`fill none`; used as CSS masks): plus `M8 3.25v9.5 M3.25 8h9.5`;
hourglass `M4 1.9h8 M4 14.1h8` with bulbs
`M5.1 1.9v1.6c0 1.9 1.2 3.1 2.9 4.5c1.7-1.4 2.9-2.6 2.9-4.5V1.9` and
`M5.1 14.1v-1.6c0-1.9 1.2-3.1 2.9-4.5c1.7 1.4 2.9 2.6 2.9 4.5v1.6`;
check `M3.1 8.6l3.1 3.1l6.7-6.9`; circle-slash
`<circle cx="8" cy="8" r="5.9"/>` plus `M3.83 3.83l8.34 8.34`.

## Label grammar

Let `Δ` be whole days from today (local date) to the field's date.

| Δ                           | Label                                       |
| --------------------------- | ------------------------------------------- |
| 0                           | `today`                                     |
| +1                          | `tomorrow`                                  |
| −1                          | `yesterday`                                 |
| +2 … +6                     | short weekday of the date: `Mon` … `Sun`    |
| any other Δ, same year      | `Oct 22` (short month, day without padding) |
| any other Δ, different year | `Oct 22, 2027`                              |

Weekday names are used only for the coming six days: a past weekday
would make `⧗ Mon` ambiguous, so past dates beyond yesterday use
month and day. The weekday rule wins over the year rule (on Dec 30, a
Jan 1 date reads `Fri`). Words are lowercase; month and weekday
names are capitalized fixed English, matching the freshness mark.
One grammar for all four fields makes a closed task read as a small
timeline (`+ Aug 29  ✓ Sep 3`); the task's age stays one hover away.

## Tooltip

The mark carries `role="img"`, `aria-label`, and
`data-tooltip-position="top"`, lines joined with `\n`, never
containing `::`. Line 1 is `{Verb} {short date} · {relative}`,
where `{short date}` is the freshness short date (`Fri, Oct 9`, year
appended across years) and `{relative}` is `today`, `tomorrow`,
`yesterday`, `in N days`, or `N days ago` in exact days. Line 2,
scheduled only, is `Ctrl+Shift+P to reschedule`. Examples:
`Created Tue, Sep 29 · 8 days ago`;
`Scheduled Fri, Oct 9 · in 2 days` + reschedule hint;
`Done Mon, Oct 5 · 2 days ago`;
`Cancelled Tue, Dec 30, 2025 · 281 days ago`.

## Anatomy and tones

A mark is `span.bob-date-mark` with `span.bob-date-mark-glyph` (the
mask) and `span.bob-date-mark-label` (the text), carrying
`data-field`, `data-date`, `data-when` (`past` | `today` |
`future`), and `data-fold-space` (`data-rendered="true"` on
rendered-view marks). Metrics match `.bob-fresh-mark` exactly
(interface font at 0.8em, `tabular-nums`, weight 550, inline-flex
with a 0.22em gap, pill padding/margin, `vertical-align: 0.05em`);
the glyph box is 1.08em square; hover is opacity 1 plus a 10%
`currentColor` capsule; transitions off under
`prefers-reduced-motion`.

| Situation                                      | Ink                                          | Opacity / weight |
| ---------------------------------------------- | -------------------------------------------- | ---------------- |
| default                                        | `--bob-date-color-{field}`, else `--text-muted` | 0.85          |
| quiet: any `created`; past `scheduled`         | same ink                                     | 0.7              |
| emphasis: `scheduled` with `data-when="today"` | `--bob-date-color-today`, else `--text-normal` | 1, weight 650 |
| resting: inside a closed task (`x`, `X`, `-`)  | `--text-faint`                               | 0.75; wins over every other row |

Resting is derived from the nearest `li.task-list-item[data-task]`
or `.HyperMD-task-line[data-task]` ancestor, exactly like the
priority mark. Theme hooks
`--bob-date-color-{created,scheduled,completion,cancelled,today}`
are unset by default. There is no red and no "overdue" state: a past
scheduled date means the task already resurfaced, not that it is
late.

## Eligibility and folding

For each key: the occurrence gate counts
`/(^|[^A-Za-z0-9_-])key\s*::/gi` and requires exactly one (so
`[rescheduled:: …]` never counts, and duplicates get no mark). The
canonical form is `[key:: YYYY-MM-DD]` or `(key:: YYYY-MM-DD)` with
a matching bracket pair, a lowercase key touching `::`, padding
spaces only, and a strict calendar date. Each key is judged on its
own. Any canonical field outside code gets a mark: task lines,
plain bullets, quotes, continuation lines, paragraphs. Excluded:
source mode and code positions in Live Preview; text under `code`,
`pre`, `.dataview.inline-field`, `.bob-date-mark`,
`.bob-priority-mark`, or `.bob-fresh-mark` in rendered views.

Whitespace-run folding (Live Preview): the decoration starts at the
first space of the U+0020 run directly before the field, and the
widget restores one uniform gap — Tasks-written double-spaced
`completion`/`cancelled` fields sit with the same gap as
single-spaced ones. A field beginning the line's content never
folds.

## Repair flag

While marks are on, any leftover
`created`/`scheduled`/`completion`/`cancelled` Dataview pill in Live
Preview is a field Tasks cannot read: a bad date, a word, a
datetime, a duplicate, mismatched brackets, an uppercase key, or a
space before `::`. CSS flags it with full opacity and a dashed
orange border, matching the freshness and priority flags. Template
placeholders in `_templates` are also flagged: the flag truthfully
says Tasks cannot read the value as a date.

## Surfaces and interaction

Live Preview uses a `Prec.highest` ViewPlugin emitting one
`Decoration.replace` per eligible field, in line order. The mark
hides while any selection overlaps its own field span, so editing
`created` never reveals `scheduled`. Mousedown places the cursor at
the field start and focuses the editor; it never writes. Widget
equality includes the label, `when`, and tooltip, so a new day
always re-renders. Rebuilds happen on doc, viewport, or selection
changes, Live Preview or file switches, and the `dateMarksRefresh`
effect. The rendered-view post-processor (sort order 50, before
Dataview's pass at 100) splits each matching text node into
text / mark / text / mark / … for every eligible field, covering
reading view, embeds, hover previews, Dataview `TASK` views, and
Tasks descriptions with a non-trailing field. A duplicate split
across separate text nodes is not detected (the accepted priority
limit).

## Rollover and toggle

The existing minute interval calls
`refreshDateMarksForRollover(now)`: when the local date changed, it
dispatches `dateMarksRefresh` to every markdown editor and relabels
every `.bob-date-mark[data-rendered="true"]` in place (label,
`aria-label`, `data-when`). Live Preview widget DOM is never edited
in place. The command "Toggle task date marks" (id
`toggle-date-marks`) is session-only and on by default: it flips
`body.bob-date-marks` (which also gates the repair flag), dispatches
the refresh effect, triggers the Tasks re-render event, and shows
`Date marks on` / `Date marks off`. When off, JS creates no marks.

## `api.dateMarks` v1

Frozen and additive (top-level api stays v3):
`{ version: 1, fields: ["created", "scheduled", "completion",
"cancelled"], model(field, dateText), render(host, field, dateText,
options) }`. Both are synchronous and never throw. `render` appends
to `host` and returns the element, or `null` for an unknown field or
a non-canonical date. `options.decorative` renders `aria-hidden`
with no tooltip; `options.inheritColor` makes the ink
`currentColor`. Nothing consumes the namespace yet; it exists so a
later Task Card or notice can show `⧗ Fri` with the same glyph.

## Rejected alternatives

Changing storage to Tasks emoji or relative text breaks the Tasks
Dataview format and Dataview's field index, rewrites thousands of
vault fields plus the bob-cli parsers and writers, and relative text
rots overnight. A CSS-only restyle of the Dataview pill cannot read
the value, so the ISO date would stay with no `today`/`Fri` and no
repair semantics. An age voice for created (`5w`) adds a second
grammar and breaks the closed-task timeline. Weekday names for past
dates are ambiguous for scheduled. `×` for cancelled reads as math
next to `+` and as a close button. Per-field hues clash with the
status colors, and a past scheduled date is not overdue. Hiding
created on closed tasks drops data; the resting tone quiets it
instead. One lifecycle chip per task loses per-field reveal. Click
never opens a card or picker: display surfaces never act. `due` and
`start` are out of scope (see above).

Non-goals: storage, parser, Rust CLI/TUI, `bob query`, Bob Mac
Capture output; Task Card, notice, and picker rows (the api enables
them later); italic dates in Schedule/Work/Cancel Log entries; Tasks
group headings; project-note frontmatter properties; memory notes.

## Conformance vectors

Today is `2026-10-07` (Wednesday) unless noted. ⏎ separates tooltip
lines. `fold N` is `foldLength` in Live Preview.

| #    | Input                                                                                          | Expected                                                                                                         |
| ---- | ---------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| DM1  | `- [ ] #task Buy milk [created:: 2026-10-07]`                                                  | created · `today` · when `today` · `Created Wed, Oct 7 · today` · fold 1 · quiet tone                            |
| DM2  | `- [ ] #task Call mom [created::2026-10-06] ^call`                                             | created · `yesterday` · `Created Tue, Oct 6 · yesterday`                                                         |
| DM3  | `- [?] #task Ship [scheduled:: 2026-10-08]`                                                    | scheduled · `tomorrow` · `Scheduled Thu, Oct 8 · tomorrow⏎Ctrl+Shift+P to reschedule`                            |
| DM4  | `[scheduled:: 2026-10-09]`                                                                     | `Fri` · `Scheduled Fri, Oct 9 · in 2 days⏎Ctrl+Shift+P to reschedule`                                            |
| DM5  | `[scheduled:: 2026-10-13]`                                                                     | `Tue` (Δ 6) · `Scheduled Tue, Oct 13 · in 6 days⏎…`                                                              |
| DM6  | `[scheduled:: 2026-10-14]`                                                                     | `Oct 14` (Δ 7) · `Scheduled Wed, Oct 14 · in 7 days⏎…`                                                           |
| DM7  | `- [ ] #task Rename [scheduled:: 2026-10-07]`                                                  | `today` · when `today` (emphasis tone) · `Scheduled Wed, Oct 7 · today⏎…`                                        |
| DM8  | `- [ ] #task Install [scheduled:: 2026-09-04]`                                                 | `Sep 4` · when `past` (quiet tone) · `Scheduled Fri, Sep 4 · 33 days ago⏎…`                                      |
| DM9  | `- [x] #task Report [completion:: 2026-10-05]`                                                 | completion · `Oct 5` (never a past weekday) · `Done Mon, Oct 5 · 2 days ago` · resting (CSS)                     |
| DM10 | `- [-] #task Old [cancelled:: 2025-12-30]`                                                     | cancelled · `Dec 30, 2025` · `Cancelled Tue, Dec 30, 2025 · 281 days ago`                                        |
| DM11 | `[scheduled:: 2027-01-02]`                                                                     | `Jan 2, 2027` · `Scheduled Sat, Jan 2, 2027 · in 87 days⏎…`                                                      |
| DM12 | today `2026-12-30`, `[scheduled:: 2027-01-01]`                                                 | `Fri` (Δ 2 across the year) · `Scheduled Fri, Jan 1, 2027 · in 2 days⏎…`                                         |
| DM13 | `- [x] #task Review skill [created:: 2026-08-29]  [completion:: 2026-09-03] ^review`           | two marks in order: created `Aug 29` fold 1; completion `Sep 3` fold 2                                           |
| DM14 | `- [ ] #task A [fresh:: 2026-10-05] [created::2026-09-29] [scheduled:: 2026-10-09]`            | created `Sep 29`, scheduled `Fri`; both ranges disjoint from the fresh mark's range                              |
| DM15 | `- [ ] #task B (scheduled:: 2026-10-09)`                                                       | `Fri`                                                                                                            |
| DM16 | `- [x] #task C [ completion:: 2026-10-07 ]`                                                    | `today`                                                                                                          |
| DM17 | `- Idea for later [created::2026-07-03]`                                                       | created `Jul 3` (plain bullet)                                                                                   |
| DM18 | `> - [ ] #task Quoted [created:: 2026-10-01]`                                                  | `Oct 1` · fold 1                                                                                                 |
| DM19 | `- [ ] #task D x[created:: 2026-10-01]`                                                        | `Oct 1` · fold 0                                                                                                 |
| DM20 | `- [created:: 2026-10-01] starts the bullet`                                                   | `Oct 1` · fold 0 (the field begins the content)                                                                  |
| DM21 | `Paragraph text [scheduled:: 2026-10-09]`                                                      | `Fri` (non-list line)                                                                                            |
| DM22 | ``- [ ] #task E `[created:: 2026-10-01]` ``                                                    | untouched (code)                                                                                                 |
| DM23 | `- [ ] #task G [rescheduled:: 2026-10-09] [scheduled:: 2026-10-10]`                            | scheduled `Sat`; `rescheduled` untouched and not counted as a duplicate                                          |
| DN1  | `[scheduled:: 2026-13-01]`                                                                     | no mark (repair pill)                                                                                            |
| DN2  | `[scheduled:: tomorrow]`                                                                       | no mark (repair pill)                                                                                            |
| DN3  | `- [ ] #task F [scheduled:: 2026-09-10] (scheduled:: 2026-09-11)`                              | no mark on either (repair pills)                                                                                 |
| DN4  | `[Created:: 2026-10-01]`                                                                       | no mark (key case)                                                                                               |
| DN5  | `[created:: 2026-10-01)`                                                                       | no mark (mismatched brackets)                                                                                    |
| DN6  | `[created :: 2026-10-01]`                                                                      | no mark (key must touch `::`)                                                                                    |
| DN7  | `[created:: 2026-10-01T09:30]`                                                                 | no mark                                                                                                          |
| DN8  | `[due:: 2026-10-09]` / `[start::1700]`                                                         | untouched, and never repair-flagged                                                                              |
| DN9  | `- [ ] #task H [scheduled:: 2026-13-01] [created:: 2026-10-01]`                                | created `Oct 1` still marks; scheduled is a repair pill                                                          |
| RO1  | a rendered `Fri` mark (date `2026-10-09`) when the day becomes `2026-10-08`, then `2026-10-09` | relabels in place to `tomorrow`, then `today` (`data-when="today"`); Live Preview editors get the refresh effect |

## Live verification (Bryan, in Obsidian)

- [ ] All four glyphs and every label form look right in light and dark themes
- [ ] The cursor or a click reveals only that field's raw text
- [ ] Tasks-written double-spaced `[completion:: …]` fields sit with the same gap as single-spaced ones
- [ ] Created reads quiet, scheduled-today reads stronger, and closed tasks rest
- [ ] A hand-broken `[scheduled:: 2026-13-01]` shows the dashed repair pill
- [ ] The toggle restores the pills and then turns the marks back on
- [ ] Reading view, embeds, and hover previews show the marks
- [ ] Labels roll over after midnight without a reload
- [ ] Fresh, priority, and date marks sit together cleanly on one line
- [ ] Metadata Menu does not double-decorate
- [ ] Mobile (iOS) renders the glyphs

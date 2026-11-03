# Event link times

In a daily note's Pomodoro ledger, a Task Link to one of that note's timed Event Tasks
wears the same time pill as the row in `## Events`. The pill helps a MEET lineup read as
a schedule. It is display-only: Bob does not write to the vault, and without the plugin
the note looks as it does today.

## What you see

The time pill leads the Task Link, matching the Events row's time, title, and off-clock
chip order:

```text
- [x] (**0815-0830** [off:: 15m]) — MEET
    - ⟦0815-0830⟧ ~~^swings~~
- [ ] () — MEET
    - ⟦30m|1830-1900⟧ ^virtual-cfg-appt
    - ⟦1900-2030⟧ ^disney-movie-night
```

Here `⟦…⟧` represents the existing calendar Event time pill and any buffer wings. A
struck, completed meeting still gets its pill. The pill belongs to the event's meeting
span, not the Pomodoro session span.

## Principles

- **Display-only:** no vault writes; the feature adds no markup to source text.
- **Same pill:** the host contains the existing `bob-calendar-event-time` element and
  reuses its attributes and CSS. Calendar Event styling remains the single source of
  truth.
- **Event time:** the display uses the Event Task's span, state, clock, and buffers. It
  does not read or infer from the Pomodoro entry's span or `[t::]` / `[off::]` fields.
- **Equal to Mac Capture:** a link qualifies only when it targets a unique, timed Event
  Task in the same daily note, matching the Mac agenda's event-chip rule.
- **Neutral on uncertainty:** unresolved or cross-note targets, duplicate block IDs,
  all-day context rows, missing source text, and errors produce no pill or warning.

## Which links get a pill

All conditions below must hold:

1. The rendered file is a canonical daily note, regardless of its date.
2. The row is a dedicated ledger Task Link under a Pomodoro entry, open or closed. It
   may be plain, prefixed by `🍅 `, struck through, aliased, or followed by the trailing
   ` #` move-only marker.
3. It is not an embed, mixed prose, a deeper bullet, or inside a fence, frontmatter, or
   `%%` comment. Rows in `## Events`, `## Notes`, and other sections do not qualify.
4. The link resolves to the rendered note itself. An empty link path qualifies; a
   non-empty path qualifies only when Obsidian resolves it to that same file.
5. Its block ID belongs to exactly one checkbox Event Task row in `## Events` with a
   leading `**HHMM-HHMM**` time span. A `#task` tag is optional, as it is for the Events
   painter.
6. Both the **Task Link event times** toggle and calendar event painting are on.

This includes hand-written timed Event Tasks and completed events. It excludes an
all-day context row, duplicate IDs, embeds, other notes' events, and ordinary task links
that do not target an Event Task.

## Resolution

Resolve the source line and target in this order:

1. **Live Preview:** use the editor's current document text, including unsaved edits.
   Scan its `## Events` rows once per document version and map block IDs to rows.
2. **Reading view section:** use the rendered element's section source text when
   available.
3. **Calendar source cache:** use the existing calendar Event source lines.
4. **Task theme source cache:** request a fill for the rendered file if the source is
   missing; leave the pill absent until the fill completes, then refresh. Never read the
   file from disk during render.

For the link path, an empty path means the rendered note. Otherwise resolve it with
Obsidian's metadata cache relative to that note and require the resolved file to be the
same note. The block ID must identify one and only one Event Task row. For each qualifying
row, the time text is copied exactly as written (for example, `1830-1900`).

State comes from the shared calendar Event time helpers. Today's note uses the current
clock; another daily note has no past, running, or future state, just like its Events
pill. A running event uses the MEET hue. `#on` takes precedence over `#off`; otherwise
the root's inherited `offclock:` setting determines the dashed off-clock treatment.
Buffer wings are computed by the same helpers as the Events pill. The shared clock fix
also corrects the Events pill's handling of afternoon and evening events before they
start.

## Anatomy and look

The host is a `span.bob-event-link-time.bob-calendar-event-time-wrap` with `role="img"`,
an accessible label, a top-positioned Obsidian tooltip, and the Event Task block ID. It
contains an optional before-buffer wing, the existing `bob-calendar-event-time` pill,
and an optional after-buffer wing. The inner pill's `data-*` attributes match those
produced for the Event row; its `title` is omitted because the host supplies the
accessible label and tooltip.

The pill adds no line height or document position. It is not selectable, uses a default
cursor, and is not copied with the row. Existing Event pill CSS draws it; no separate
visual style is introduced.

## Placement

Live Preview inserts a zero-width widget at the beginning of the link, before the opening
`~~` when struck or before `[[` otherwise. A preceding `🍅 ` stays before the pill. The
widget uses `side: -3`, before the ledger type mark (`-2`) and progress mark (`-1`), so
the row reads `pill #TYPE ◐ link`. It stays at that position when the cursor reveals the
raw link text.

Reading view inserts the pill before the link container, including a `del` or `s` wrapper
when that wrapper contains only the link. If ledger type or progress marks already sit
directly before the link container, the pill is moved before them. This gives the same
order regardless of processor order. The host has `margin-inline-end: 0.35em`.

## Tooltip

The `aria-label` and Obsidian tooltip show the cleaned Event Task title, limited to 80
characters (79 plus `…`), or `Calendar event` for an empty title. If the Event pill has a
tooltip, it appears on a second line, including buffer and off-clock details. Any `::`
in the label is changed to `:`. No inner element has a `title` attribute.

## Interaction

A plain mousedown in Live Preview places the caret at the widget position and focuses
the editor. Mod-click with the primary button opens the Task Link in a tab, like
Mod-clicking the link itself. A plain click does not navigate or write. The pill has no
tab stop, live-region announcement, or animation.

## Liveness

Live Preview rebuilds after document or viewport changes, a Live Preview/source mode
switch, a file switch, its own refresh effect, or the calendar Event refresh effect. It
does not rebuild on selection alone. Calendar painting refreshes link pills on the same
clock cadence: Live Preview through that effect, and Reading view by sweeping the
rendered pills for today's note.

A coalesced refresh runs after metadata cache `changed` / `resolved` events, vault rename
or delete, source-text cache fills, and either toggle. Those updates cover changes to
inherited `offclock:`, type, and parent data. Reading view sweeps rendered daily notes in
all windows, including popouts, and adds, updates, or removes hosts in place.

## Toggles

**Toggle Task Link event times** (`toggle-event-link-times`) is session-only and on by
default. Calendar Event painting must also be on, since the link pill uses that pill's
look. Both toggles update main and popout windows. Bob exposes no API namespace for this
feature.

## Performance

Event rows are scanned once per document version and indexed by block ID. Each unique
link path is resolved once per build. Live Preview creates widgets only in visible
ranges. Rendering never reads from disk. A synthetic ledger with 200 links and 50 Event
rows should build in about 25 ms or less.

## Deferred

- Links to an Event Task in another daily note: add if Bryan asks; Mac Capture is also
  same-note only.
- Prose links in Events, Notes, or other sections: add if Bryan asks.
- Embedded ledger links: not supported.
- Replacing the block-ID link text with an Event title: deferred with readable task-link
  titles in the Task Link marks contract.

## Conformance vectors

Unless a vector says otherwise, today is 2026-11-03, now is 10:00, the note is
`2026/20261103.md`, and its `## Events` section contains:

```text
- [x] #task **0815-0830** Swings! #off [root:: abby] [type:: MEET] ^swings
- [ ] #task **0945-1015** Standup [root:: bob] [type:: MEET] ^standup
- [*] #task **1830-1900** (virtual) CFG Appt #off [buffer:: -30m] [root:: body] [type:: MEET] ^cfg
- [ ] **1200-1230** Park [root:: abby] ^park
- **all day** Holiday ^holiday
```

`love.md` is an area with `offclock: true`; `abby.md` is an area whose parent is
`love.md`; `bob.md` is a project with the task `Fix the parser` at `^parser`. The daily
note also has `^gtd` above `## Events`.

### EL — which links

| # | Ledger row under `## Pomodoros` | Result |
| --- | --- | --- |
| EL1 | Open `() — MEET`, child `[[#^cfg]]` | Pill: `1830-1900` with a `30m` before wing |
| EL2 | Closed timed MEET, child `~~[[#^swings]]~~` | Pill, though the entry is closed and link struck |
| EL3 | Open entry, child `🍅 [[#^standup]]` | Pill |
| EL4 | Open entry, child `[[#^standup]] #` | Pill |
| EL5 | Open entry, child `[[#^standup\|Standup]]` | Pill |
| EL6 | Open entry, child `[[2026/20261103#^standup]]` | Pill; explicit path resolves to same note |
| EL7 | Open entry, child `[[#^park]]` | Pill; a hand-written Event Task need not have `#task` |
| EL8 | Child `![[#^standup]]` | No pill; embed |
| EL9 | Child `join [[#^standup]] early` | No pill; mixed prose |
| EL10 | Deeper child `[[#^standup]]` | No pill; deeper bullet |
| EL11 | Child `[[bob#^parser]]` | No pill; not an Event Task |
| EL12 | Child `[[#^gtd]]` | No pill; task outside `## Events` |
| EL13 | Child `[[#^holiday]]` | No pill; all-day context row |
| EL14 | Child `[[#^dup]]` when two Event rows carry `^dup` | No pill; ambiguous ID |
| EL15 | Child `[[2026/20261102#^swings]]` | No pill; another note's event is deferred |
| EL16 | Previous day's ledger links its own `[[#^swings]]` | Pill, with no `data-state` |
| EL17 | Prose link under Events or Notes | No pill; prose is deferred |
| EL18 | EL1 inside a fenced block | No pill |
| EL19 | A non-daily note has Events and Pomodoros sections | No pill |
| EL20 | Either toggle is off | No pill; JavaScript and CSS gates |

### ES — state

The shared time-state and buffer-phase helpers determine both the Events pill and link
pill state.

| # | Span, buffer, and time | Result |
| --- | --- | --- |
| ES1 | `1830-1900` at 10:00 | `future` |
| ES2 | `1200-1230` at 10:00 | `future` |
| ES3 | `0945-1015` at 10:00 | `running`, with the MEET hue |
| ES4 | `0815-0830` at 10:00 | `past` |
| ES5 | `2300-0030` at 23:45 | `running` |
| ES6 | `2300-0030` at 00:10 | `running`; midnight-crossing span shifts the clock forward |
| ES7 | `2300-0030` at 22:00 | `future` |
| ES8 | `1830-1900`, `-30m`, at 18:10 | `future`, phase `lead-in` |
| ES9 | `1200-1300`, `-30m +15m`, at 13:05 / 13:20 | No state, phase `lead-out` / `past` |
| ES10 | Any span in a note that is not today's | No state |

The shared clock fix also makes an afternoon event remain future until it starts in
`## Events`.

### ER — resolution and parity

| # | Case | Result |
| --- | --- | --- |
| ER1 | EL1 | `data-buffer-before="30"`, `30m` before wing, and off-clock data |
| ER2 | EL7, whose `[root:: abby]` inherits `love`'s off-clock setting | `data-off-clock="true"` |
| ER3 | EL3, whose `[root:: bob]` has no off-clock setting | No `data-off-clock` |
| ER4 | Add `#on` to `^park` | No `data-off-clock` |
| ER5 | Unsaved Live Preview edit changes `^standup` span to `1000-1030` | Pill shows `1000-1030` immediately |
| ER6 | Unsaved Live Preview edit adds `#off` to `^standup` | Pill becomes dashed immediately |
| ER7 | Any pill row | Inner attributes equal `gcalEventsTimeAttributes` minus `title` |
| ER8 | Event row is `[x]` or `[-]` | Pill still shows |

### ET — tooltip

| # | Case | `aria-label` |
| --- | --- | --- |
| ET1 | EL1 | `(virtual) CFG Appt` then `Holds 18:00–19:00 · 30m before` on line 2 |
| ET2 | EL2 | `Swings!` |
| ET3 | EL7 | `Park` then `Off-clock · from love` on line 2 |
| ET4 | 95-character title | First 79 characters plus `…` |
| ET5 | Title contains `::` | `::` becomes `:` |

### EP — placement

| # | Case | Result |
| --- | --- | --- |
| EP1 | Live Preview | Widget is at the link start, `side: -3`, without changing line height |
| EP2 | Running `[/]` event under an open entry | `pill ◐ link` |
| EP3 | Off-clock link chip also appears | `pill link off` |
| EP4 | Cursor reveals raw `[[…]]` link text | Pill stays at the same end |
| EP5 | Pill row in Reading view | ◐ / 🔓 / 🔑 still paint; no status ring appears |
| EP6 | Event pill and link pill computed styles | Same background, ink, radius, padding, and outline style per state |
| EP7 | Struck link | Strike does not run through the pill |

## JavaScript mirror

The implementation mirrors this contract in `src/291d-event-link-times.js` (core),
`src/291e-plugin-event-link-times.js` and
`src/291f-plugin-event-link-times-reading.js` (Live Preview and Reading view), with
coverage in `scripts/test-ledger-tools-event-link-times.cjs`. The feature has no API
namespace.

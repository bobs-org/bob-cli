# Today marks

This guide is the authoritative display contract for Today marks. The JavaScript
mirror lives in bob-ledger-tools: `src/143-today-marks.js` (pure core),
`src/275-plugin-today-marks.js` (Live Preview, toggle, and jump), and
`src/276-plugin-today-marks-reading.js` (rendered views). Its tests run the
conformance vectors below verbatim. There is no api namespace.

## What you see

A warm 🍅 follows the identity glyph on an open task line linked under one of
today's open Pomodoros. A committed task without a Task Link stays unmarked:

```text
before: - [*] #task Fix the parser crash ^fix
after:  - [*] ⌗ 🍅 Fix the parser crash ^fix
        - [/] #task Update the project stats #prj ^stats
        - [*] #task Merge queue refactor #ref [[Queue design]] ^queue
        - [*] #task Finish the E2E suite ^e2e
```

Here the first row is linked under today's open Pomodoro. The other rows are
not. On a reference task the mark follows the `#task #ref` identity pair:

```text
before: - [*] #task #ref Read the queue design ^queue
after:  - [*] ⌗ 🍅 Read the queue design ^queue
```

`⌗` approximates the existing task identity glyph. The mark is display-only;
the source line never contains 🍅.

## Which tasks get the mark

Today is the set of open tasks with a dedicated Task Link under today's **open**
Pomodoros, computed at read time. See [Today in the plan contract](plan.md#today).
The mark reads the existing Today cache (`todayCache.rank`) and does not store
Today on a task. Residence in the daily note is not enough, and links only under
closed Pomodoros are history, not Today.

A line gets a mark only when all of these hold:

1. The cache is ready for the current date and daily path.
2. The live task status is open: ` `, `*`, `/`, or `?`. Completing or cancelling
   a task removes its mark on the same frame.
3. It is a real `#task` line: an exact `#task` tag token, outside inline code
   and fences, with a standalone trailing `^id`.
4. The key `"<host path>.md#<id>"` is in Today.
5. That id occurs on only one line in the host note. Duplicate ids are neutral
   because the Task Link target is ambiguous.
6. The host is not a canonical daily note, and the line is not rendered inside
   one.
7. The session toggle is on.

## Principles

- **Display-only:** nothing writes the mark; task and ledger source stay unchanged.
- **Truthful or neutral:** uncertainty, stale cache, duplicate ids, and unproven
  rendered source mappings produce no mark.
- **One Today:** the mark means exactly the plan's Today set. It does not mean
  daily-note residence or earlier work.
- **Rare and warm:** only a small number of task lines carry 🍅, so it is a quiet
  accent rather than another status pill.

## Placement

The mark goes at the first non-space character after the identity slot, before
the title. The slot is the leading exact `#task` token, the `#task #ref`
pair (📖), or the canonical `#task #gtd #pre` / `#task #gtd #post`
checklist run (☀ / ⚑). On `#task #prj` lines it sits between the hash and the
`#prj` pill.
When `#task` is not the body's first token, the mark goes at the body start,
right after the checkbox and its space.

Live Preview uses a zero-width `Decoration.widget({ side: -1 })` at that offset.
The caret at the title start draws after the mark. The widget takes no document
positions, so vim motions (`0`, `^`, `I`, `w`) are unchanged. It covers no text,
so it remains visible when the cursor reveals raw `#task`; it does not rebuild
on `selectionSet`. Unmarked rows reserve no space.

## Glyph anatomy

The host is an empty `span.bob-today-mark`; CSS `::before` draws the glyph from
`content: var(--bob-today-mark-glyph, "🍅")`. Copying a line never copies 🍅,
and a theme snippet can replace it with a mask. The pseudo-element uses the
color emoji font stack `"Apple Color Emoji", "Segoe UI Emoji", "Noto Color Emoji",
sans-serif` and `font-variant-emoji: emoji`, so the tomato remains a color emoji.

The box is `inline-flex`, centered, `1.2em × 1.2em`, `font-size: 0.85em`,
`line-height: 1`, `vertical-align: -0.14em`, `margin-inline: 0.06em 0.3em`,
`border-radius: 999px`, `user-select: none`, and `cursor: default`. It does not
change line height. Hover uses a capsule tinted with the task line's status:
`color-mix(in srgb, var(--task-status-accent, var(--text-muted)) 14%, transparent)`
with `background-color 120ms ease`.

Only a key that entered Today within the last 600 ms gets the entrance animation
`bob-today-mark-in 180ms cubic-bezier(0.2, 0.9, 0.3, 1.25)`, from
`opacity: 0; scale: 0.5` to `opacity: 1; scale: 1`. It does not play at startup,
on scroll, rerender, or midnight. Reduced-motion settings disable animation
and transition. Forced-colors mode uses a `Canvas` hover capsule with a `1px
solid CanvasText` outline. The glyph uses `print-color-adjust: exact`.

The toggle is an opt-out gate:
`body.bob-today-marks-off .bob-today-mark { display: none; }`. This lets marks
remain visible in popout windows where plugin body classes may be absent. The
decoration builder and rendered-view post-processor also check the toggle.

## Tooltip and accessibility

The tooltip is the element's `aria-label`, with
`data-tooltip-position="top"`. There is no `title`, avoiding duplicate
tooltips. It has two lines joined with `\n` and never contains `::` because
Dataview re-scans `innerHTML`.

- Line one is `Today · running in <session>` for the unique open timed entry;
  otherwise it is `Today · queued in <session>`. If more than one open entry
  links the task, the primary entry is the running one, otherwise the first in
  ledger order, and the line ends in ` +N more`.
- A named entry gives its name and, when timed, its range: `PLAN 0725–0815` or
  `BLOG`. An unnamed entry gives its derived Pomodoro home label (for example
  `bob · dev +1`) and its range when timed. If it has no derived label, use its
  range alone (`0940–1010`), or `unnamed Pomodoro` when untimed. Ranges use an
  en dash.
- Line two is `⌘-click to open in today's note` on macOS, or
  `Ctrl-click to open in today's note` elsewhere. It is omitted on mobile.

The task line never gets the word “today.” The mark adds no tab stop and no
live-region announcement.

## Interaction

- **Plain click** places the cursor at the title start (`view.posAtDOM(dom)`)
  and focuses the editor. It never writes or navigates.
- **Mod+click** (Cmd on macOS, Ctrl elsewhere, primary button) opens today's
  daily note at the Task Link. An already-open daily-note leaf is reused;
  otherwise the note opens as with “Open today's daily note.” The cursor moves
  to the link token on the primary entry's line, centered in view. If the
  cached `ledgerLine` no longer holds the link, the open note is rescanned with
  `computeTodayLinks` and `resolveTodayLinkKeys`. If the link is gone, a Notice
  says `This task's Task Link is no longer in today's note`. A second press
  while a jump is in flight is ignored.
- **Go to today's Task Link** is the keyboard twin of Mod+click. It is an
  editor command with no default hotkey and jumps for the task on the cursor
  line. If that line is not on today's plan, it shows `Not on today's plan` (or
  `No task on this line`).

## Toggle

`Toggle Today marks` (`toggle-today-marks`) is a session toggle, on by default.
It flips `body.bob-today-marks-off`, refreshes every editor and rendered row,
and shows `Today marks on` or `Today marks off`. It never changes source text.

## Surfaces

| Surface | Contract |
| --- | --- |
| Live Preview of area, project, reference, and inbox notes | v1; exact matching task lines |
| Reading view, block embeds, hover popovers | Mark only with exact source mapping; otherwise neutral |
| Canonical daily notes and content rendered inside them | Never marked |
| Tasks query results (dash, freshness, rotten, blocked, crowded) | Out of scope; Today-exclusive queries gain no information from a mark |
| Source mode | Never marked; raw text only |

## Staying current

The cache rebuilds on the daily note's metadata `changed` event, vault create,
delete, or rename, `resolved`, the minute rollover tick, and focus or visibility
revalidation. The readiness gate applies. Detail-only changes (start, rename,
or moving a task between entries) refresh tooltips without reloading Tasks. Late
async reads are dropped by a generation guard. A read failure degrades to no
marks.

## Conformance vectors

Notation: `T` is an area note holding `- [s] #task Fix ^fix`, `D` is today's
daily note, and every link sits at an open entry's first child indent unless
stated.

| ID | Setup | Mark on T? |
| --- | --- | --- |
| TM1 | `\t- [[T#^fix]]` under the unique open timed entry `- [ ] (**0725-0815** [t:: 50m]) — PLAN`, `s=*` | yes; tooltip `Today · running in PLAN 0725–0815` |
| TM2 | same link under placeholder `- [ ] () — BLOG`, `s=/` | yes; `Today · queued in BLOG` |
| TM3 | `s=*` or `/`, no link in `D` (sticky lane) | no |
| TM4 | only `\t- 🍅 [[T#^fix]]` under a closed `- [x]` entry | no |
| TM5 | TM4 plus a placeholder link | yes |
| TM6 | `🍅 [[T#^fix]]` under an open entry | yes |
| TM7 | embedded `\t- ![[T#^fix]]` under an open entry | yes, on T's line |
| TM8 | struck, nested, mixed-text, trailing `#` move-only, fenced, or outside `## Pomodoros` | no |
| TM9 | `s` is `x`, `X`, or `-`, link still present | no; it flips on the same frame |
| TM10 | `s=?` or `s=" "`, link still present (reconcile lag) | yes |
| TM11 | no exact `#task` tag (`#tasks`, `#Task`), or no standalone trailing `^id` | no |
| TM12 | linked under two open entries; then one link removed | one mark throughout; tooltip `… +1 more`, then without it |
| TM13 | the task lives in a canonical daily note (`^gtd`), or T is rendered inside a daily note | no |
| TM14 | cache not ready (startup), or the date rolled over | no |
| TM15 | `^fix` ends two lines of T | no mark on either |
| TM16 | `#task #ref` / `#task #prj` / `#task #gtd #pre` lines | after the 📖 pair / between the hash and `#prj` / after the sunrise run |
| TM17 | cursor at the title start; cursor inside `#task` | mark shown, caret after it; hash revealed, mark stays |
| TM18 | Ctrl+Shift+Enter link, then unlink, from T | appears with the entrance pop, then disappears |
| TM19 | rendered view where the source line cannot be proven | no (neutral) |
| TM20 | toggle off, then on | hidden everywhere at once, then restored; source unchanged |
| TM21 | `#task` only inside inline code, or the line is inside a fence | no |
| TM22 | the linking placeholder starts (`() — BLOG` → `(**0900-0930** [t:: 30m]) — BLOG`), with no key change | tooltip flips `queued` → `running`; no Tasks reload |
| TM23 | Mod+click the mark; plain click; the keyboard command on T's line | opens D at the Task Link (running entry first); cursor at title start; same jump as Mod+click |

## Deferred

- A running-now tone, if Bryan wants to distinguish now from later by hovering.
- A worked-earlier-today ghost tone, if he misses that signal.
- 🍅 on Depends-On/BLOCKS chips, the Task Card, and nav pickers, once a consumer
  needs `api.today`.
- Mac Capture candidates with Rust `today: true`, coordinated with
  `decisions:mac-capture-is-a-thin-client`.
- Narrowing Ctrl+Shift+Enter's presence test; the recommendation is to leave it.

## Live verification

Bryan verifies in Obsidian after `live_preview` and again after `reading`:

- [ ] In `sase.md`, exactly the Next rows linked under today's open Pomodoros show 🍅. The merge-queue ref row, the project-stats row, and the E2E row do not.
- [ ] Ctrl+Shift+Enter link and unlink make the mark pop in and vanish within about a quarter second, with the daily note open in another pane and with it closed.
- [ ] Completing a marked task removes its mark on the same frame.
- [ ] Starting the placeholder Pomodoro flips the tooltip from `queued in` to `running in`.
- [ ] Light and dark themes look right on green (Next) and amber (Pending) tints, beside a violet `#prj` pill, after the 📖 pair, and with Apple emoji on the Mac.
- [ ] vim `j`/`k` does not jitter, and the caret sits after the mark at the title start.
- [ ] Mod+click and “Go to today's Task Link” land on the Task Link, reusing the open daily pane.
- [ ] Midnight (or waking the laptop the next day) clears the marks without a reload.
- [ ] The toggle hides and restores marks everywhere, including in a popout window.
- [ ] Daily notes and `dash.md` never show a mark.
- [ ] Reading view shows the same marks as Live Preview. If full-note Reading view shows none, `ctx.getSectionInfo` is unavailable there: file a follow-up rather than adding title matching.

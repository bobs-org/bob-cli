# Task tag marks

This guide is the authoritative display contract for task tag marks,
in the style of `docs/date-marks.md`. The JavaScript mirror lives in
bob-ledger-tools (`src/138-task-tag-marks.js` pure core plus
`src/264-plugin-task-tag-marks.js` Live Preview, rendered views, and
toggle); its tests run the conformance vectors below verbatim. There
is **no api namespace** (the top-level api stays v3; add one only
when a consumer exists).

## What you see

`#task` is the Tasks global filter (`globalFilter: "#task"`,
`removeGlobalFilter: false`), so about 3,400 task lines start with an
accent-colored `#task` tag pill. That pill repeats on almost every
line and adds the most visual noise. Every exact `#task` tag on a
task line renders as one small, faint, monochrome hash glyph instead:

```text
before: - [ ] #task Rename the queue input  ▮▮▮  + Sep 29  ⧗ today
after:  - [ ] ⌗ Rename the queue input  ▮▮▮  + Sep 29  ⧗ today      (⌗ ≈ the faint hash glyph)
        - [ ] Pack charger                                         (plain checklist: no glyph, unchanged)
dash.md Tasks results:  - [ ] Rename the queue input  ▮▮▮  + Sep 29   (tasks_results = hide: tag dropped)
```

Hovering the glyph shows `#task · tracked task` plus the demote hint.
Moving the cursor into the glyph, or clicking it, reveals `#task`
for editing.

## Principles

Display-only: the text `#task` stays the only stored form, and
nothing writes the mark. Tasks, Dataview, bob-cli, and capture
semantics are unchanged. Quietest mark in the family: it repeats on
every task line, so it uses faint ink and no resting background. It
is truthful, never guessing: a mark appears only on exact, whole
`#task` tags on task lines; anything else stays Obsidian's normal tag
pill. It is reversible: the cursor or a click reveals the raw tag,
source mode shows raw text, a session toggle restores the pills
instantly, and without the plugin the vault looks exactly as it does
today. There is one glyph definition, a CSS mask shared by every
host.

## The glyph: a soft slanted hash

A Lucide-style hash keeps the tag affordance: you can see the tag is
still there and still editable. It is drawn thin, slanted, and
round-capped, so it reads as an icon rather than text. Geometry
(viewBox `0 0 16 16`, stroked, used as a CSS mask, tunable ±0.3
units): `stroke-width 1.6`, `stroke-linecap round`, path
`M6.5 3 5.5 13M10.5 3 9.5 13M3.5 6.25h9M3.5 9.75h9`. The glyph is
centered at (8, 8), with roughly square cells, and is about the
height of a text `#`. CSS custom property, defined once on `body`:

```css
--bob-task-tag-glyph: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16' fill='none' stroke='%23000' stroke-width='1.6' stroke-linecap='round'%3E%3Cpath d='M6.5 3 5.5 13M10.5 3 9.5 13M3.5 6.25h9M3.5 9.75h9'/%3E%3C/svg%3E");
```

## Anatomy and tones

There are two hosts with **one shared rule set**, both gated by
`body.bob-task-tag-marks`:

1. **Live Preview widget:** an empty `span.bob-task-tag-mark` with
   `role="img"`, `aria-label`, and `data-tooltip-position="top"`.
2. **Rendered views:** Obsidian's own `a.tag` element with the class
   `bob-task-tag-mark` added, plus the same `aria-label` and
   `data-tooltip-position`. The element is never replaced: its text
   `#task` stays in the DOM for copy and screen readers, and its
   native click (tag search) keeps working.

The shared rules use family metrics: `font-family:
var(--font-interface)`, `font-size: 0.8em`, `display: inline-block`,
`position: relative`, a content-box glyph area of `1.08em` square,
padding `0.06em 0.14em`, margin `0 0.08em`, `border: 0`,
`border-radius: 999px`, `background: none`, `text-decoration: none`,
`overflow: hidden`, `white-space: nowrap`, `color: transparent` (this
hides the `a.tag` text), and `vertical-align: -0.1em` (tunable so the
hash sits on the text baseline like a `#`). Every theme tag-pill
property (`--tag-*` padding, background, border, color, weight, size)
is overridden with enough specificity
(`body.bob-task-tag-marks a.tag.bob-task-tag-mark`). The glyph is
drawn by `::before { content: ""; position: absolute; inset: 0.06em
0.14em; background-color: var(--bob-task-tag-ink); }` with prefixed
and unprefixed `mask-image: var(--bob-task-tag-glyph)`,
`mask-size: contain`, `mask-repeat: no-repeat`,
`mask-position: center`, plus `print-color-adjust: exact` (and the
`-webkit-` form) so PDF exports keep the glyph.

| Situation                            | Ink (`--bob-task-tag-ink`)                     | Other                                                                                                                          |
| ------------------------------------ | ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| rest                                 | `var(--bob-task-tag-color, var(--text-faint))` | opacity 0.9                                                                                                                    |
| hover                                | `var(--bob-task-tag-color, var(--text-muted))` | opacity 1; capsule `color-mix(in srgb, var(--text-muted) 10%, transparent)` (not `currentColor`: it is transparent here)       |
| resting: closed task (`x`, `X`, `-`) | rest ink                                       | opacity 0.55, derived from the nearest `li.task-list-item[data-task]` / `.HyperMD-task-line[data-task]` like the priority mark |

Cursor: `default` on the Live Preview widget (as in the family) and
`pointer` on the `a.tag` host (it is a link). Transitions (`opacity`,
`background-color`, 120ms) are off under `prefers-reduced-motion`.
The theme hook `--bob-task-tag-color` is unset by default. The mark
ignores status accent colors (shape carries meaning; color whispers).

## Tooltip

The text is exactly `#task · tracked task` followed by
`Ctrl+Shift+] to demote to a bullet` (lines joined with `\n`). It
never contains `::`, because Dataview re-scans `innerHTML`. Exported
as the `TASK_TAG_MARK_TOOLTIP` constant. `Ctrl+Shift+]` is
task-status-cycler's `toggle-obsidian-task` hotkey, the existing
gesture that removes the checkbox and `#task`.

## Eligibility

- **Tag token**: the exact, case-sensitive text `#task`. The
  character before it is the start of the text or whitespace. The
  character after it is the end of the text or not a tag character
  (`/[\p{L}\p{N}_\/-]/u`). So `#tasks`, `#task/sub`, `#Task`,
  `foo#task`, and `(#task)` never match, while `#task!` does.
- **Live Preview**: only on task lines, using
  `freshnessTaskStatus(line) !== null` (quote-aware, so callouts and
  numbered lists count). Every eligible token on the line gets its
  own mark. A token is skipped when
  `freshnessMarkPosInCode(tree, absFrom + 1)` is true (inline code or
  a code block). Plain bullets (`- #task …`), paragraphs, and
  headings are left alone: on a non-task line, the raw pill is the
  honest signal that Tasks ignores the line.
- **Rendered views**: an `a.tag` whose `textContent` is exactly
  `#task` (and whose `href`, if present, is `#task`). Its nearest
  `li` ancestor within the post-processor root must carry
  `task-list-item`, so a nested plain child bullet under a task gets
  no mark, while a task nested under a plain bullet does. It must not
  sit under `code`/`pre` or inside a Tasks result row
  (`.plugin-tasks-list-item`, `.tasks-list-text`, or
  `.task-description`). With no `li` ancestor (for example, a
  detached element), there is no mark.

## Surfaces and interaction

- **Live Preview**: a `Prec.highest` ViewPlugin emits one
  `Decoration.replace` per eligible token, covering exactly the 5
  characters of `#task`. There is no whitespace folding, so the
  spaces around the tag stay text. The mark is revealed (no
  decoration) while any selection range touches the tag
  **inclusively** (`sel.from <= absTo && sel.to >= absFrom`). As a
  result, the cursor never rests against a mark: typing `#task` stays
  raw until you type the following space. The ledger snippet `#task
  [created::…]` (cursor between the two spaces) shows the glyph
  immediately. Mousedown places the cursor at the tag start and
  focuses the editor; it never writes. Widget equality uses a
  constant key, so marks never flicker. Rebuild on doc, viewport, or
  selection changes, Live Preview or file switches, and the new
  `taskTagMarksRefresh` effect. A cheap
  `line.text.indexOf("#task")` prefilter runs before the task-line
  check. There are no marks in source mode.
- **Rendered views**: a markdown post-processor (sort order 50)
  annotates eligible `a.tag` elements in place: it adds the class
  plus `aria-label` and `data-tooltip-position`, and never creates or
  removes nodes. It is idempotent, and attributes survive Dataview's
  `innerHTML` round trip. It covers reading view, embeds, hover
  previews, canvas cards, and Dataview task lists.
- **Coexistence**: ranges are disjoint from the freshness, priority,
  and date marks. The date mark's whitespace fold may start exactly
  at the tag's end (adjacent is fine, never overlapping). Vector TT16
  asserts this.

## Tasks query results

Tasks query results use CSS only, with no JS, and are keyed on the
selector
`body.bob-task-tag-marks .plugin-tasks-list-item .task-description a.tag:is([data-tag-name="#task"], [href="#task"])`.
Tasks 8.4.0 sets `data-tag-name` on description tags in
`addInternalClasses`. `hide tags` and re-renders keep working because
no JS touches Tasks' DOM. The JS post-processor never annotates these
rows (TR6). The body class makes the toggle instant.

Every row is a `#task` task by the global filter, so the tag carries
no information there and is hidden with `display: none`. The rule sits
after, and with higher specificity than, the shared host rules. The
description's leading space remains, giving a uniform gap on every
row (accepted).

## Toggle and lifecycle

The command "Toggle task tag marks" (id `toggle-task-tag-marks`) is
session-only and on by default. It flips
`this.taskTagMarksEnabled` and `body.bob-task-tag-marks`, dispatches
the refresh effect to every markdown editor, and shows the Notice
`Task tag marks on` / `Task tag marks off`. Toggling **off** also
strips the class, `aria-label`, and `data-tooltip-position` from
every annotated `a.tag.bob-task-tag-mark` in the document, so pills
look and behave natively at once. Toggling **on** re-annotates every
eligible `a.tag` in `document.body` using the same eligibility
function, so rendered views update without a re-render. Because Tasks
results are CSS-only, the body class handles them instantly.
`onunload` removes the body class and strips the annotations. When
off, JS creates no marks.

## Rejected alternatives

- **Hide `#task` entirely everywhere**: this erases the
  tracked-versus-plain distinction for about 17,800 plain checkboxes
  and leaves invisible text under the cursor.
- **Change the global filter or storage**: this rewrites about 3,400
  lines and breaks the bob-cli parsers, capture, Tasks, and the
  plugins that match `#task`.
- **Tasks `removeGlobalFilter: true`**: this is a vault-wide Tasks
  setting with edit-modal side effects, and it is not toggleable or
  owned here.
- **A CSS-only `.cm-active` line reveal**: every `j`/`k` step would
  reveal the tag on the new line, shifting every title by about
  1.5em.
- **A dot or bullet**: it reads as a nested bullet. **A check icon**
  is redundant with the checkbox and clashes with done. **A filled
  tag silhouette** turns into a blob at 0.8em. **Lucide `list-todo`**
  is too busy. **Accent or status hues** compete with the status line
  tints.
- **Replacing `a.tag` with a new span in rendered views**: this loses
  native tag search, copy text, and robustness to re-renders.
- **A glyph in Tasks results**: it would be a column of identical
  glyphs that carries zero information.
- **An api namespace with no consumer**: rejected. Pickers, the Task
  Card, and notices already strip `#task`.

Non-goals: storage, Rust CLI and TUI, `bob query`, Bob Mac Capture,
the Obsidian search and backlinks panes, the tag pane, source mode,
Mod+click tag search from Live Preview, and repair flags (tags have no
malformed canonical form).

## Conformance vectors

Live Preview core: `taskTagMarkRanges(lineText)` returns
`[{ from, to }]` in UTF-16 line offsets.

| #    | Input                                                      | Expected                                                                             |
| ---- | ---------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| TT1  | `- [ ] #task Buy milk`                                     | `[6,11)`                                                                             |
| TT2  | `- [x] #task Report [completion:: 2026-10-05]`             | `[6,11)` (resting tone is CSS)                                                       |
| TT3  | `- [?] #task #gtd #context/home Check weather`             | `[6,11)` only; other tags untouched                                                  |
| TT4  | `> - [ ] #task Quoted`                                     | `[8,13)` (quote-aware)                                                               |
| TT5  | `1. [ ] #task Numbered`                                    | `[7,12)`                                                                             |
| TT6  | `- [x] (1645-1710) #task #sase Research`                   | `[18,23)` (mid-line tag)                                                             |
| TT7  | `- [ ] Pack charger`                                       | none (plain checklist)                                                               |
| TT8  | `- #task Launch epic!`                                     | none (not a task line)                                                               |
| TT9  | `- [ ] #tasks a` / `- [ ] #task/sub a` / `- [ ] #Task a`   | none                                                                                 |
| TT10 | ``- [ ] Note `the #task tag` here``                        | none in Live Preview: the core returns `[16,21)`, and the inline-code check drops it |
| TT11 | `- [ ] foo#task`                                           | none                                                                                 |
| TT12 | `- [ ] #task! Ship it`                                     | `[6,11)`                                                                             |
| TT13 | `- [ ] #task  [created::2026-10-07]`, cursor 12            | mark `[6,11)` shown (cursor is not touching)                                         |
| TT14 | `- [ ] #task Buy milk`, cursor 5 / 6 / 11 / selection 0–20 | shown / revealed / revealed / revealed                                               |
| TT15 | `- [ ] #task A #task B`                                    | `[6,11)` and `[14,19)`                                                               |
| TT16 | `- [ ] #task A [priority:: high] [created:: 2026-10-07]`   | `[6,11)`, disjoint from the priority and date mark ranges                            |
| TT17 | `Paragraph #task text`                                     | none                                                                                 |
| TT18 | `- [ ] #task`                                              | `[6,11)` (the tag ends the line)                                                     |
| TT19 | `- [ ] (#task) parens`                                     | none (conservative boundary)                                                         |

Rendered views (fake DOM in tests):

| #   | DOM                                                         | Expected                                                                            |
| --- | ----------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| TR1 | `li.task-list-item > a.tag[href="#task"]` with text `#task` | annotated (class, tooltip `aria-label`, `data-tooltip-position="top"`)              |
| TR2 | `li.task-list-item > p > a.tag` (loose list)                | annotated                                                                           |
| TR3 | task li > `ul` > plain li > `a.tag` `#task`                 | not annotated                                                                       |
| TR4 | plain li > `ul` > `li.task-list-item` > `a.tag` `#task`     | annotated                                                                           |
| TR5 | `a.tag` text `#Task` / `#tasks`                             | not annotated                                                                       |
| TR6 | `a.tag` inside `.plugin-tasks-list-item .task-description`  | not annotated (Tasks rows are CSS-only)                                             |
| TR7 | detached root, no `li` ancestor                             | not annotated                                                                       |
| TR8 | a second pass over TR1                                      | no change (idempotent)                                                              |
| TR9 | marks off / toggle off / toggle on                          | nothing annotated / annotations stripped document-wide / eligible tags re-annotated |

## Live verification (Bryan, in Obsidian)

- The glyph looks right beside the checkbox in light and dark
  themes.
- Plain checkboxes show no glyph.
- The cursor or a click reveals `#task`, and typing `#task` stays raw
  until the space.
- The ledger `ta` task snippet shows the glyph at once.
- Vim `j`/`k` through a task list does not jitter unless the column
  is inside the tag.
- Reading view, embeds, and hover previews show the glyph, and
  clicking it opens the `#task` tag search.
- `dash.md` Tasks results show no tag.
- The toggle restores pills everywhere and back again.
- Closed tasks rest.
- The glyph sits cleanly with the priority, date, and fresh marks on
  one line.
- Mobile (iOS) renders it.
- PDF export shows the glyph.

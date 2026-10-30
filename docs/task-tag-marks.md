# Task tag marks

This guide is the authoritative display contract for task tag marks,
in the style of `docs/date-marks.md`. The JavaScript mirror lives in
bob-ledger-tools (`src/138-task-tag-marks.js` and
`src/138-checklist-task-marks.js` pure core plus
`src/264-plugin-task-tag-marks.js` Live Preview, rendered views, and
toggle); its tests run the conformance vectors below verbatim. There
is **no api namespace** (the top-level api stays v3; add one only
when a consumer exists).

## What you see

`#task` is the Tasks global filter (`globalFilter: "#task"`,
`removeGlobalFilter: false`), so about 3,400 task lines start with an
accent-colored `#task` tag pill. That pill repeats on almost every
line and adds the most visual noise. Every exact `#task` tag on a
task line renders as one small teal identity hash glyph instead.
When the line has an explicit `[type:: X]` (or legacy `[theme:: X]`),
the slot is the hash plus the type name in that type's ink, and the
trailing field is folded away:

```text
before: - [ ] #task Rename the queue input  ▮▮▮  + Sep 29  ⧗ today
after:  - [ ] ⌗ Rename the queue input  ▮▮▮  + Sep 29  ⧗ today      (⌗ ≈ the teal identity hash glyph)
        - [ ] #FIX Significantly speed up sase commits               (explicit FIX in FIX ink)
        - [ ] ⌗ Restart athena                                       (inherited BUILD: hash only)
        - [ ] Pack charger                                           (plain checklist: no glyph, unchanged)
dash.md Tasks results:  - [ ] Rename the queue input  ▮▮▮  + Sep 29   (tasks_results = hide: tag dropped)
```

Hovering the generic glyph shows `#task · tracked task` plus the demote hint.
An explicit type shows `FIX · set on this task · … · Ctrl+Shift+P t to change`.
An inherited type stays a plain hash whose tooltip reads `BUILD · from dev`.
Moving the cursor into the glyph, or clicking it, reveals `#task`
for editing; moving into the folded field reveals `[type:: X]`.

## Principles

Display-only: the text `#task` stays the only stored form, and
nothing writes the mark. Tasks, Dataview, bob-cli, and capture
semantics are unchanged. Small but distinct: it repeats on every
task line, so it uses no pill or resting background. The hash is one
constant teal identity ink until a task has an explicit type; then
the name and ink encode that type. Ink never encodes lane, priority,
freshness, or checkbox state. It
is truthful, never guessing: a mark appears only on exact, whole
`#task` tags on task lines; anything else stays Obsidian's normal tag
pill. With stale catalog data the generic glyph is shown rather than a
confidently wrong type. It is reversible: the cursor or a click reveals the raw tag,
source mode shows raw text, a session toggle restores the pills
instantly, and without the plugin the vault looks exactly as it does
today. There is one glyph definition, a CSS mask shared by every
host.

## The glyph: a soft slanted hash

A Lucide-style hash keeps the tag affordance: you can see the tag is
still there and still editable. It is drawn thin, slanted, and
round-capped, so it reads as an icon rather than text. Geometry
(viewBox `0 0 16 16`, stroked, used as a CSS mask, tunable ±0.3
units): `stroke-width 1.8`, `stroke-linecap round`, path
`M6.5 3 5.5 13M10.5 3 9.5 13M3.5 6.25h9M3.5 9.75h9`. The glyph is
centered at (8, 8), with roughly square cells, and is about the
height of a text `#`. CSS custom properties, defined once on `body`:

```css
--bob-task-tag-glyph: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16' fill='none' stroke='%23000' stroke-width='1.8' stroke-linecap='round'%3E%3Cpath d='M6.5 3 5.5 13M10.5 3 9.5 13M3.5 6.25h9M3.5 9.75h9'/%3E%3C/svg%3E");
--bob-task-tag-hue: color-mix(in srgb, var(--color-cyan, #00bfbc) 72%, var(--text-normal));
```

## Ink

The ink is one constant identity hue: a deep teal that means
"tracked task" and nothing else. It never follows status, lane,
priority, or freshness; that job stays with the checkbox and line
tint. It never uses red, orange, yellow, green, or blue, and it
stays legible on the red Blocked line tint.

| Hue           | Already means                                                                              |
| ------------- | ------------------------------------------------------------------------------------------ |
| red           | Blocked (line tint, blocked chips)                                                         |
| orange        | attention: due freshness, ROTTEN, repair flags, the Pending chip                           |
| yellow        | In Progress (line tint, the half-ring progress mark)                                       |
| green         | Next (line tint), fresh `✓ today`                                                          |
| blue          | the Ready lane                                                                             |
| violet accent | tags (`#prj`, `#gtd`, …) and links                                                         |
| cyan          | inline code chips (`--inline-code-hue: var(--color-cyan)`)                                 |
| grey          | priority and date marks, cancelled                                                         |

Vault counts (2026-10-08): 3,406 `#task` lines, 2,934 closed, about
470 open — mostly Blocked (`[?]`, 383 lines) on the red tint, plus
Next (`[*]`, 19), In Progress (`[/]`, 7), and plain `[ ]` (63).
656 `#task` lines carry another tag pill, usually `#prj` in the
violet accent, beside the glyph.

The chosen teal blends theme cyan toward the text color so it adapts
to both themes: about `#0a9391` in light (about 3.8:1 on white, above
the 3:1 non-text contrast floor) and a soft aqua of about `#79dedc`
in dark. Teal complements the red Blocked tint, so it is crispest
where most open `#task` glyphs sit, and it stands apart from the
violet `#prj` pill. It shares a hue family only with inline code, a
bordered monospace chip the glyph never reads as. Defined on `body`:

```css
--bob-task-tag-hue: color-mix(in srgb, var(--color-cyan, #00bfbc) 72%, var(--text-normal));
```

The theme hook `--bob-task-tag-color` is unset by default and still
wins when set.

When the task has an explicit type, that teal is replaced by the type
ink for the type's hue slot. Defined on `.theme-light` and
`.theme-dark` for each of the eight palette slots:

```css
--bob-type-ink-<slot>: color-mix(in srgb, var(--bob-hue-<slot>) 55%, var(--text-normal));
```

`--bob-task-tag-color` still wins when set. Exempt and `auto`-muted hues use
the fixed `--bob-type-ink-muted` token, one hex per theme, defined on
`.theme-light` / `.theme-dark` beside the slot inks; `--bob-task-tag-color`
still wins. A node test holds it to ≥ 4.5:1 on the same lane matrix.
Unconfigured names use `--text-muted` with a dotted underline; repair uses
`#?` in warning-orange.

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

| Situation                            | Ink (`--bob-task-tag-ink`)                                                                        | Other                                                                                                                                                                  |
| ------------------------------------ | ----------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| rest                                 | `var(--bob-task-tag-color, var(--bob-task-tag-hue))`                                            | opacity 0.9                                                                                                                                                            |
| hover                                | `var(--bob-task-tag-color, var(--bob-task-tag-hue))`                                            | opacity 1; capsule `color-mix(in srgb, var(--bob-task-tag-ink) 14%, transparent)` on both hosts (not `currentColor`: it is transparent here)                            |
| resting: closed task (`x`, `X`, `-`) | `color-mix(in srgb, var(--bob-task-tag-color, var(--bob-task-tag-hue)) 40%, var(--text-faint))` | opacity 0.75, derived from the nearest `li.task-list-item[data-task]` / `.HyperMD-task-line[data-task]` like the priority mark; wins on both hosts |

Cursor: `default` on the Live Preview widget (as in the family) and
`pointer` on the `a.tag` host (it is a link). Transitions (`opacity`,
`background-color`, 120ms) are off under `prefers-reduced-motion`.
The `--bob-task-tag-color` hook is unset by default. The generic hash ink
names a tracked task, never its lane. When `data-bob-type` is set on a
hash mark, CSS hides the `::before` mask and draws `#TYPE` as text in
`::after`, using the same ink for the hash and name. The host keeps only
the capsule padding. The book and checklist variants retain their glyph
and draw the type name beside it.

## Tooltip

The generic text is exactly `#task · tracked task` followed by
`Ctrl+Shift+] to demote to a bullet` (lines joined with `\n`). An
explicit type uses
`FIX · set on this task · dev suggests BUILD · Ctrl+Shift+P t to change`.
An inherited type uses `BUILD · from dev` on the plain glyph. A
disagreeing ledger link uses
`BUILD task under FIX · Ctrl+Shift+M to move · Ctrl+Shift+P t to retype`.
None of these strings contain `::`, because Dataview re-scans
`innerHTML`. Exported as `TASK_TAG_MARK_TOOLTIP`,
`taskTagIdentityTooltip`, and `ledgerTypeDisagreeTooltip`.
`Ctrl+Shift+]` is
task-status-cycler's `toggle-obsidian-task` hotkey, the existing
gesture that removes the checkbox and `#task`. The aria label for an
explicit type is `#task · type FIX`.

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
  focuses the editor; it never writes. Widget equality keys include type, hue, and state, so a retype
  refreshes the mark and an unchanged type never flickers. Rebuild on doc, viewport, or
  selection changes, Live Preview or file switches, and the new
  `taskTagMarksRefresh` effect. A cheap
  `line.text.indexOf("#task")` prefilter runs before the task-line
  check. There are no marks in source mode.
- **Rendered views**: a markdown post-processor (sort order 50)
  annotates eligible `a.tag` elements in place: it adds the class
  plus `aria-label` and `data-tooltip-position`, and never creates or
  removes nodes. When the type is shown it also sets `data-bob-type`,
  `data-bob-type-hue`, and `data-bob-type-state`; CSS `::after` draws
  `#TYPE` for hash marks and the type name after the book/checklist
  glyphs, so the post-processor creates no extra nodes. It is
  idempotent, and attributes survive Dataview's
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

## Reference reading tasks

A task line whose exact `#task` tag is immediately followed by one
whitespace run and `#ref` (any case, so `#REF` and `#Ref` pair) is a
reference reading task: the pair renders as **one** open-book glyph in
the slot, in the same teal identity ink the hash uses. Shape says the
kind; ink still says "tracked task".

```text
before: - [ ] #task #ref Harness Engineering
after:  - [ ] 📖 Harness Engineering               (📖 ≈ the teal open-book glyph)
```

One identity slot: `#ref` anywhere else (prose, headings, code,
non-task lines, `#ref #task` order, `#task #references`) never renders
the book. `#task #references` keeps the hash on `#task`; `#ref #task`
keeps the hash on `#task`; a lone `#ref` gets no mark.

The glyph is a 16×16 stroked open-book mask in the hash's stroke
language (`stroke-width` 1.8, round caps and joins), defined once as
`--bob-ref-task-glyph` beside `--bob-task-tag-glyph`. Closed reading
tasks rest at the same 40 % mix toward `--text-faint` as closed hashes.
`🔖` already means "jump to this highlight" on annotation follow-ups,
which share the same `## Tasks` lists, so the book keeps one symbol for
reading tasks. One symbol, one meaning.

Behavior matches the hash: the cursor or a click reveals both raw tags
(one touch anywhere on the pair exposes both, since the decoration
covers the whole span); the session toggle, keyboard access, and PDF
export behave as for `#task`. Hovering the book shows
`#task #ref · reference reading task`; its accessible label is
"Reference reading task".

Reading view turns the `#task` anchor into the book and hides the
adjacent `#ref` anchor in place (it stays in the DOM for copy and
Dataview's `innerHTML` round trip). In Tasks query results, where
`#task` already hides, the `#ref` pill renders as the book (CSS-only,
like the `#task` rule).

Pickers never show the pair: display text drops the `#ref` that follows
`#task` (as it drops `#task`) and reads `📖 Harness Engineering` — in
the task pickers, the Task Card header, and inbox-route subtitles. The
stored line keeps both tags, so Tasks, Dataview, `bob query`, and grep
keep working.

## GTD checklist tasks

The canonical run `#task #gtd #pre` is one teal sunrise in the task
identity slot. `#task #gtd #post` is one teal flag. The partners may
appear in either order after `#task`; the single glyph replaces the
whole run, so its shape names the checklist lane while the shared teal
ink still means "tracked task."

```text
before: - [?] #task #gtd #pre Brush teeth
after:  - [?] ☀ Brush teeth                 (☀ sunrise: PRE opens the review)
        - [?] ⚑ Morning review: chores done  (⚑ flag: POST closes the review)
```

The two 16×16 stroked masks use the hash/book geometry: no fill,
`stroke-width: 1.8`, round caps and joins. They are defined beside
`--bob-ref-task-glyph` as `--bob-pre-task-glyph` and
`--bob-post-task-glyph`. The sunrise path is
`M1.4 12.2h13.2M3.8 12.2a4.2 4.2 0 0 1 8.4 0M8 3.9v2.4M2.13 6.33l1.7 1.7M13.87 6.33l-1.7 1.7`;
the flag path is
`M3.6 14.2V2.3M3.6 2.9h8.6l-2.1 3.2 2.1 3.2H3.6`.

Both variants use the same theme-aware teal identity ink, hover capsule,
and sizing as the hash and book. The glyph does not encode status;
closed tasks (`x`, `X`, `-`) rest at the shared 40% mix toward
`--text-faint` with 0.75 opacity.

| Kind | Variant class | Accessible label | Tooltip |
| --- | --- | --- | --- |
| PRE | `bob-pre-task-mark` | `PRE opening checklist task` | `#task #gtd #pre · PRE opening checklist` / `Opens the ]s walk · Ctrl+Enter when done` |
| POST | `bob-post-task-mark` | `POST closing checklist task` | `#task #gtd #post · POST closing checklist` / `Closes the ]s walk (]S) · Ctrl+Enter when done` |

Eligibility is deliberately narrow. On a real task line, the exact
`#task` token must be followed in the identity slot by one whitespace
run, the exact whole-token tags `#gtd` and `#pre` or `#post` in either
order, and one whitespace run between them. Matching is
case-insensitive for partner tags. A line containing both `#pre` and
`#post` anywhere keeps its pills as a conflict signal. The existing
`#task #ref` pair takes precedence. Tags elsewhere in the title, a
missing partner, `#prep`, `#gtd/pre`, a non-task line, or code keep the
native pills.

Live Preview replaces the full run with one stable widget; touching any
part of it reveals all three raw tags for editing. Reading view annotates
the `#task` anchor and hides both partner anchors in place, preserving
copy, re-scans, and the toggle. Tasks query rows use CSS only: the
sunrise or flag appears when the description has the matching `#gtd`
pair, and the redundant `#gtd` pill is hidden. Rows with both `#pre`
and `#post` remain native.

CSS cannot enforce that the partner tags occupy the identity slot,
because sibling combinators ignore text nodes. The Tasks-row rule may
therefore draw on a misplaced but otherwise matching pair; the vault's
rows are canonical. No JavaScript touches Tasks' DOM.

The Today 🍅 follows the complete checklist identity run, so it stays
between the glyph and the title. The existing task-tag-marks toggle
restores the three source pills everywhere. Pickers, notices, the review
footer, CLI/TUI, `bob query`, and Bob Mac Capture are unchanged.

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

The command "Toggle inherited task types" (id
`toggle-inherited-task-types`) is session-only and **off** by default.
When on, inherited types render as `#BUILD` in a quieter ink (weight
400, about 60% of explicit). When off, the hash stays and the tooltip
still reads `BUILD · from dev`. Tasks query rows keep hiding the hash.

The command "Toggle task type marks" (id `toggle-task-theme-marks`)
keeps its id and now folds `[type::]` / `[theme::]` fields without a
trailing pill. Session-name pills (ledger, pickers, Task Card, Day
strip, Stats) stay. Notice copy is `Task type marks on` /
`Task type marks off`.

## Ledger agreement marks

A live Task Link that disagrees with its task's type (D5) shows
`#BUILD` before the link with a dashed warning-orange underline.
In an entry with two or more type components, agreeing links show a
quiet, underline-free `#BUILD` prefix. Inherited types are never
prefixed. With stale cache the prefix is omitted. Clicking never
writes.

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
  is too busy.
- **Ink**: faint grey (the 1.35.0 ink) is lost beside the grey
  priority and date marks and near-invisible on closed lines;
  status hues and blue would claim a state or the Ready lane;
  orange means attention and repair; pink and orchid turn muddy on
  the Blocked tint and read as a near-miss beside violet pills;
  the task-type tag accent makes the mark read as one more tag pill
  beside `#prj`.
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
`[{ from, to, kind }]` in UTF-16 line offsets, where `kind` is `task | ref
| pre | post`. A `ref` range spans `#task #ref`; `pre` and `post` ranges
span the whole three-tag checklist identity run.

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
| TT20 | `- [ ] #task #ref Harness Engineering`                    | `[{[6,16), ref}]` (one range covering both tokens)                                   |
| TT21 | `- [x] #task #ref Done book [completion:: 2026-10-05]`     | `[{[6,16), ref}]` (resting tone is CSS)                                              |
| TT22 | `- [ ] #task #REF Loud`                                    | `[{[6,16), ref}]` (case-insensitive partner)                                         |
| TT23 | `- [ ] #task   #ref Extra spaces`                          | `[{[6,18), ref}]` (extra whitespace still pairs)                                     |
| TT24 | `- [ ] #task #references Not a ref`                        | `[{[6,11), task}]` (hash only)                                                       |
| TT25 | `- [ ] #ref #task Reversed`                                | `[{[11,16), task}]` (hash only on `#task`)                                           |
| TT26 | `- [ ] #ref Alone`                                         | none (`#ref` alone)                                                                  |
| TT27 | `- #task #ref Not a task`                                  | none (not a task line)                                                               |
| TT28 | ``- [ ] Note `the #task #ref pair` here``                  | none in Live Preview: the core returns `[{[16,26), ref}]`, and the inline-code check drops it |

PRE/POST checklist marks:

| # | Input | Expected |
| --- | --- | --- |
| CK1 | `- [ ] #task #gtd #pre Brush teeth` | `[{[6,21), pre}]` |
| CK2 | `- [?] #task #gtd #post Morning review` | `[{[6,22), post}]` |
| CK3 | `- [x] #task #gtd #pre Check weather [completion:: 2026-10-21]` | `[{[6,21), pre}]` (resting tone is CSS) |
| CK4 | `- [ ] #task #pre #gtd Swapped` | `[{[6,21), pre}]` |
| CK5 | `- [ ] #task #GTD #Pre Loud` | `[{[6,21), pre}]` |
| CK6 | `- [ ] #task  #gtd   #post Extra spaces` | `[{[6,25), post}]` |
| CK7 | `- [ ] #task #gtd #pre #post Conflict` / `- [ ] #task #gtd #pre Brush #post` | `[{[6,11), task}]`; pills stay |
| CK8 | `- [ ] #task #gtd Topic only` / `- [ ] #task #pre Missing gtd` | `[{[6,11), task}]` |
| CK9 | `- [ ] #task #gtd #prep Typo` / `- [ ] #task #gtd/pre Nested` | `[{[6,11), task}]` |
| CK10 | `- [ ] #task Brush teeth #gtd #pre` | `[{[6,11), task}]` (not in the slot) |
| CK11 | `- [ ] #task #ref #gtd #pre Odd` | `[{[6,16), ref}]` (the ref pair wins) |
| CK12 | `- #task #gtd #pre Not a task` / ``- [ ] Note `#task #gtd #pre` `` | none / none in Live Preview (code) |
| CK13 | `- [ ] #task #gtd #pre Brush teeth ^teeth`, Today-linked | 🍅 widget at offset 22 |
| CK14 | `checklistKindForText` on `#gtd #pre` / `#GTD #Post` / `#gtd #pre #post` / `#gtd/pre` / `#pre, #gtd` | `pre` / `post` / `pre` / `null` / `pre` |

Type identity (D11). `taskTagTypeFieldOf` / `taskTagIdentity*` / `ledgerTypeMarkKind`.
Ink now encodes type when the field is explicit; lane, priority, freshness, and
checkbox state still never tint the hash.

| # | Input | Expected |
| --- | --- | --- |
| TI1 | `- [ ] #task Do [type:: FIX]` | field `FIX`; identity shown as `#FIX` in type ink; tooltip `FIX · set on this task · … · Ctrl+Shift+P t to change`; aria `#task · type FIX`; `[type:: FIX]` folded |
| TI2 | `- [ ] #task Do [theme:: WRITE]` | same as TI1 for legacy `theme` |
| TI3 | `- [ ] #task Restart athena` with inherited BUILD | hash only; tooltip `BUILD · from dev`; `data-bob-type` absent until inherited toggle is on |
| TI4 | `- [ ] #task #ref Harness [type:: BUILD]` | book plus `BUILD` name |
| TI5 | `- [ ] #task #gtd #pre Brush [type:: GTD]` | sunrise plus `GTD` name |
| TI6 | `- [ ] #task Buy [type:: SNACK]` unconfigured | `#SNACK` in muted ink, dotted underline |
| TI7 | `- [ ] #task Fix [type:: ???]` | `#?` repair mark; raw field visible |
| TI8 | inherited toggle off / on | hash only / `#BUILD` in quieter ink |
| TI9 | `- [*] #task Spike [type:: PLAN]` (exempt, no hue) | `#PLAN` in the muted ink, readable on the Next tint |
| LM1 | Task Link to a BUILD task under `— FIX` | `#BUILD` prefix, dashed orange underline |
| LM2 | Task Link to a BUILD task under `— BUILD + FIX` | quiet `#BUILD` prefix, no underline |
| LM3 | Task Link to an inherited-only task, or stale cache | no prefix |

Contrast: 8 hue slots plus the muted ink × {plain, Blocked, In Progress, Next} × {light, dark} all ≥ 4.5:1.

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
| TR10 | task li with `#task` + adjacent `#ref` anchors              | `#task` annotated with the book (reference label and tooltip); `#ref` hidden in place |
| TR11 | `#task` + `#references` anchors, or `#ref` before `#task`   | hash only on `#task`                                                                 |
| TR12 | `#ref` anchor inside `.plugin-tasks-list-item .task-description` | not annotated (Tasks rows are CSS-only: the `#ref` pill renders the book)         |
| TR13 | pair present / toggle off / toggle on                        | book plus hidden `#ref` / book stripped and `#ref` revealed document-wide / pair re-annotated |
| CR1 | task li with adjacent `#task #gtd #pre` anchors | `#task` has sunrise class, label, and tooltip; `#gtd` and `#pre` stay hidden in place |
| CR2 | `#task #gtd #pre #post`, or `#task` + `#gtd` + a word + `#pre` | hash only; every pill stays visible |
| CR3 | checklist pair present / toggle off / toggle on | variants, labels, and hidden classes stripped document-wide / then re-annotated |
| CR4 | Today reading view on a CR1 row | 🍅 inserted after the hidden `#pre` anchor and before the title |
| CR5 | Tasks row with `#gtd` + `#post` / with `#pre` + `#post` | flag drawn and `#gtd` hidden / every pill stays native |

## Live verification (Bryan, in Obsidian)

- The teal ink reads clearly yet stays secondary to the title in
  light and dark themes.
- An explicit `#FIX` reads as type ink, not as another tag pill, and
  the trailing `[type:: FIX]` is gone.
- The ink stays legible on Blocked (red tint), Next (green tint),
  and In Progress (yellow tint) lines, and sits well beside a
  `#prj` pill.
- A BUILD task under a FIX Pomodoro shows a dashed `#BUILD` before
  the Task Link.
- The hover capsule appears in both Live Preview and reading view.
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
- Closed tasks show a faint ghost of the hue (reading view and Live Preview).
- The glyph sits cleanly with the priority, date, and fresh marks on
  one line.
- Mobile (iOS) renders it.
- PDF export shows the glyph.
- On red Blocked `[?]` rows, sunrise and flag are distinct in light and
  dark themes and on iOS.
- `]s` keeps the glyph on the landing row, and 🍅 follows it on
  Today-linked PRE rows.
- Touching the checklist glyph reveals all three tags; typing stays raw.
- Tasks query rows show the matching CSS glyph, while a hand-typed
  `#pre #post` conflict keeps all its pills.

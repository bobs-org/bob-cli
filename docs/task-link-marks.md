# Task link marks

This guide is the authoritative display contract for task link marks. The JavaScript
mirror lives in bob-ledger-tools: `src/148-task-link-marks.js` (pure core),
`src/149-task-link-mark-live-model.js` (live-document model, ring host, and widget),
`src/284-plugin-task-link-marks.js` (Live Preview extension, refresh, and toggle),
`src/284a-plugin-task-link-marks-resolve.js` (the target resolver), and
`src/285-plugin-task-link-marks-reading.js` (rendered views). Its tests run the
conformance vectors below verbatim. There is no api namespace.

Decided for v1 (`bob-cli-f5`): ledger rows stay unmarked (`ledger_rows = skip`), and
links to Done or Cancelled tasks keep full link styling (`dim_closed_links = no`).
Only the ring carries the closed state.

## What you see

Every rendered link whose target is a `#task` line wears a small status ring in
front of the link. The ring's silhouette means "this is a task"; its fill and ink
give the task's status. This is the same idea as GitHub's issue-state icons and
Linear's status circles.

```text
before: This should be implemented with bob > ^move-task-to-bullet!
after:  This should be implemented with ◯ bob > ^move-task-to-bullet!      (Ready task)

before: Blocked on bob > ^gcal-projects, see also dev_tools > ^z-241023-0r
after:  Blocked on ⊖ bob > ^gcal-projects, see also dev_tools > ^z-241023-0r
                                                     (non-task block: unchanged)
```

The ring is the only change. Link text, link color, underline, hover preview, and
click behavior stay Obsidian's own. Nothing is written to the vault. Without the
plugin, the vault looks exactly as it does today.

## Principles

- **Identity by shape, state by fill.** Every state is drawn on the same circle, so
  "task link" reads at a glance even in grayscale. The fill tells the state, and the
  ink repeats it. Color is never the only signal.
- **Truthful or neutral.** A ring appears only when the target provably resolves to
  exactly one `#task` line. Unresolved notes, missing ids, duplicate ids, non-task
  blocks, plain checklists, and unknown data render **no ring**. No warning glyph
  appears in prose, because a link to a non-task block is legitimate.
- **Rings, not checkboxes.** A square glyph next to a link would invite clicking to
  complete the task. A ring reads as a status indicator, and it matches the existing
  ◐ mark.
- **Quiet.** The ring is small and has no pill or background at rest. It never changes
  line height, never takes document positions, and never rebuilds on selection-only
  updates.

## Which links get a ring

A link gets a ring only when **all** of these hold:

1. **It is a block link.** It is a wikilink `[[path#^id]]` or `[[path#^id|alias]]`, a
   same-note `[[#^id]]`, or an internal Markdown link `[text](path.md#^id)`. The
   Markdown destination may use the `<…>` form or `%20`-style percent encoding, and
   `^` may be written as `%5E`. Block ids match `[A-Za-z0-9-]+`, the same rule as
   `RELATION_BLOCK_ID_RE`.
2. **It is not an embed.** `![[…]]` and `![…](…)` get no ring, because the transclusion
   already shows the task's own checkbox.
3. **It is not inside** inline code, a fenced code block, a `%% … %%` comment, or
   frontmatter or properties.
4. **It is not on a managed edge row.** Depends-On, BLOCKS, LINKS, LINKED FROM, and
   Sub-projects rows (any `mentionIsEdgeLine` verdict except not-a-line) get no ring.
   Chips own those rows. When chips are toggled off, or the cursor reveals the raw
   text, the row is meant to read raw.
5. **It is not a dedicated Pomodoro Task Link row** in a canonical daily note's
   `## Pomodoros` section. Such a row is a direct child bullet of a column-0 Pomodoro
   entry (open or closed). Its body is exactly one `[[target#^id]]` (alias allowed),
   optionally after a `🍅 ` prefix, optionally struck `~~…~~`, and optionally followed
   by a trailing ` #` move-only marker. Deeper bullets, mixed prose, the Events
   section, and every other section **do** get rings. Ledger rows keep their
   `🍅` / `◐` slot; only prose links in daily notes get rings.
6. **The target is a task.** The link resolves to a file
   (`getFirstLinkpathDest(linkpath, sourcePath)`; an empty linkpath means the source
   note). The block id names exactly one line in that file. That line is a list item
   with a checkbox **and** an exact `#task` tag token outside inline code. A plain
   checklist row without `#task` is not a Bob task.
7. **The session toggle is on.**

## Target data: one resolver, three sources, in this order

1. **Live Preview, same-note target** (the resolved path is the editor's own file): read
   the **live editor document**. Build a block-id → line map once per document version.
   Parse the target line with the core's task-line parser: checkbox symbol, exact
   `#task` outside inline code, trailing standalone `^id`. A duplicate id means no ring.
   A same-note status change shows on the same frame, before any save.
2. **The Tasks memo**, through the existing `dependencyTasksIndex()` keyed
   `path\0blockId` (the one cache the dependency and relation chips share). A memo hit
   is a `#task` line, because the Tasks global filter is `#task`. That index pass also
   records keys seen more than once, so a duplicate id is neutral.
3. **Metadata cache fallback**, used when Tasks is absent, not yet indexed, or misses
   the key: `blocks[id]` gives the line, `listItems` gives the checkbox char at that
   line, and `tags` must hold `#task` on that line. Match block ids the same way
   `relationIndexHasCachedBlock` does. This path has no title text, so the tooltip drops
   its title line. Never read files from disk during render.

**State from symbol:** ` ` gives ready, `*` next, `/` in-progress, `?` blocked, `x`/`X`
done, and `-` cancelled. Any other symbol falls back to the Tasks status type: DONE
gives done, CANCELLED gives cancelled, NON_TASK gives no ring, and anything else gives
ready.

## The ring family

Each state has one CSS mask on a 16×16 viewBox, defined once on `body` and shared by
every host.

| State | `data-state` | Glyph | Ink (theme hook first) |
| --- | --- | --- | --- |
| Ready `[ ]` | `ready` | empty ring | `--bob-task-link-ready-color`, then the task-identity teal `var(--bob-task-tag-color, var(--bob-task-tag-hue))` |
| Next `[*]` | `next` | ring + center dot | `--bob-task-link-next-color`, then `--task-status-next` (green) |
| In Progress `[/]` | `in-progress` | ring + filled right half (the existing `--bob-progress-glyph`, reused verbatim) | `--bob-task-link-in-progress-color`, then `--task-status-in-progress` (amber) |
| Blocked `[?]` | `blocked` | ring + horizontal bar (⊖) | `--bob-task-link-blocked-color`, then `--task-status-blocked` (red) |
| Done `[x]` | `done` | filled disc with the check knocked out | `--bob-task-link-done-color`, then `var(--task-status-done, var(--text-muted))` |
| Cancelled `[-]` | `cancelled` | ring + diagonal slash (⊘) | `--bob-task-link-cancelled-color`, then `--task-status-cancelled` (gray) |

Each status ink chains its theme hook to the vault status variable (for
example `var(--bob-task-link-next-color, var(--task-status-next,
var(--color-green)))`); the palette hex lives in the theme, never in this
section, per the stylesheet contracts.

- **Geometry:**
  - Ring: `circle cx 8 cy 8 r 6.05`, `stroke-width 1.9`, matching the progress glyph.
  - Next dot: `circle r 2.35`.
  - Blocked bar: `M5.1 8h5.8`.
  - Cancelled slash: `M5.3 10.7 10.7 5.3`.
  - Bar and slash: round caps, `stroke-width 1.9`.
  - Done: a `circle r 7` disc with the check `M5 8.3 7.1 10.4 11 6.1` (stroke 1.9, round
    caps and joins) knocked out through an SVG `<mask>` inside the data URI.
- **Why teal for Ready:** Ready has no lane tint in the vault, and teal already means
  "tracked task" (the `#task` hash glyph). So the most common open state reads as
  "task, nothing special". Teal stays distinct from the violet link text it sits beside.
  Done and Cancelled recede (muted, `opacity: 0.8`). The other states use the vault's
  established status hues.
- **Host:** an empty `span.bob-task-link-mark[data-state]`. `::before` draws the mask in
  `background-color: var(--bob-task-link-ink)`. Starting metrics:
  - `display: inline-flex`, centered, a `1em × 1em` box with a `0.86em` glyph.
  - `vertical-align: -0.13em`, `margin-inline: 0.04em 0.24em`, `line-height: 1`.
  - `border-radius: 999px`, `user-select: none`, `cursor: default`,
    `text-decoration: none`.
  - Copying text never copies a glyph, and the box never changes line height.
- **Hover:** full opacity plus a capsule,
  `color-mix(in srgb, var(--bob-task-link-ink) 12%, transparent)`, with `120ms ease`
  transitions.
- **Swap animation:** the ring plays
  `bob-task-link-mark-swap 180ms cubic-bezier(0.2, 0.9, 0.3, 1.25)` (`scale 0.6 → 1`,
  `opacity 0.3 → 1`) only when a key that was already painted this session changes
  state. It never plays on first paint, startup, scroll, or rerender. A bounded per-key
  last-state map (about 2,000 keys, LRU) drives it, kept per editor so each open
  editor animates its own change. Only a refresh pass or a document change can start
  it. The host carries `bob-task-link-mark-swap` for a short window (about 400 ms) after
  the change, so a rebuild during the animation keeps the same element.
- **Accessibility and print:**
  - `prefers-reduced-motion: reduce` turns off the animation and transitions.
  - In `forced-colors: active` the ink is `CanvasText`, so the shapes still carry state.
  - `print-color-adjust: exact`.
- **Gate:** use the mark-gate mirror (`applyMarkGateClass("bob-task-link-marks", on)`
  plus `registerMarkGatePopoutSync`) with
  `body:not(.bob-task-link-marks) .bob-task-link-mark { display: none; }`. The JS checks
  in the builder and post-processor stay the source of truth.

## Placement

- **Live Preview:** a zero-width `Decoration.widget` at the link token start. That is
  `[[` or `[`, or the opening `~~` of a struck link, so the strike never runs through
  the ring.
  - The widget takes no document positions, so vim motions are unchanged.
  - It covers no text, so it stays put when the cursor reveals raw `[[…]]`.
  - When another widget shares the offset (a Today mark on a task title that starts
    with a link), the ring is always **last**, closest to the link.
- **Reading view:** insert the host immediately before the `a.internal-link`, or before
  its `del`/`s` wrapper when the anchor is that wrapper's only child.

## Tooltip

The tooltip is the element's `aria-label` plus `data-tooltip-position="top"` (Obsidian's
tooltip), with `role="img"`. There is no `title` attribute. The tooltip never contains
`::`, because Dataview re-scans `innerHTML`, so any stray `::` becomes `:`.

- **Line 1:** the target's title, cleaned with `dependencyCleanTaskText` and cut to 80
  characters (79 plus `…`).
- **Line 2:** `<Status> · <note>`, where Status is one of Ready, Next, In Progress,
  Blocked, Done, or Cancelled. `<note>` is the target file's basename without `.md`, or
  `this note` for a same-note target.
- When no title is known (metadata fallback), only line 2 shows.

```text
Move a task to a bullet
Ready · bob
```

## Interaction

- **Plain click** never navigates and never writes. In Live Preview it places the caret
  just before the link (`view.posAtDOM`) and focuses the editor. In Reading view it
  does nothing.
- **Mod+click** (Cmd on macOS, Ctrl elsewhere, primary button) behaves exactly like
  Mod+clicking the link text: `workspace.openLinkText(linktext, sourcePath, "tab")`.
- No tab stop and no live-region announcement.

## Liveness

One coalesced `scheduleTaskLinkMarksRefresh()` (about 120 ms) runs on these triggers:

- the existing `scheduleLiveWidgetRefresh()` fan-out (the Tasks `cache-update` event);
- `metadataCache` `changed` and `resolved`;
- vault `rename` and `delete`;
- the toggle.

On refresh it dispatches a `StateEffect` that rebuilds every Live Preview editor's
rings, and it sweeps stamped Reading-view anchors in every known document (main window
and popouts), adding, updating, or removing rings in place without re-rendering the
note. A Live Preview rebuild also happens on `docChanged`, `viewportChanged`, a Live
Preview ↔ source switch, and a file switch, the same as
`progressMarkShouldRebuild`. It never rebuilds on selection alone.

## Surfaces

- **Live Preview** of every Markdown note, including canvas cards that use the Markdown
  editor.
- **Reading view, block and note embeds, and hover popovers.** The transcluded content's
  own links resolve against the embedded file, because its post-processor context
  carries that file's path.
- **Tasks and Dataview query rows** (`dash`, `rotten`, `blocked`, and others): rings
  appear when Obsidian runs the post-processor on the row. Tasks renders descriptions
  with the task's own path, so links resolve correctly. If a row's path cannot be
  proven, it gets no ring.
- **Live Preview tables and callouts:** these render through Obsidian's embedded
  renderer. They get rings when that renderer runs post-processors with a source path,
  and no ring otherwise.
- **Source mode:** never.

## Performance

- Process only visible ranges. A per-line prefilter (`#^` or a case-insensitive `#%5E`)
  runs before any scan.
- Resolve each unique `(linkpath, sourcePath)` once per build.
- No disk reads in render, and no line-height change.
- The Reading sweep queries only stamped anchors.
- A node performance test builds a synthetic 1,000-line note with 2,000 task links. The
  viewport-limited build must stay under a generous bound (about 25 ms).

## Deferred

Deferred with triggers:

- Readable titles: show a bare task link's text as the task title instead of
  `bob > ^id`.
- Kind glyphs inside the ring for `^prj`, `#ref`, and Event Tasks.
- A broken-block flag for links whose id has vanished.
- Rings in nav pickers, the Task Card, and Mac Capture candidates.
- Mod+click opening the Task Card.
- Ledger rows (decided `skip` for v1; a later epic reopens it).

## Conformance vectors

Tests run these vectors verbatim.

**TS — scan.** `taskLinkMarkSpans(lineText)` returns
`[{at, linkpath, blockId, kind, alias}]`, where `at` is the UTF-16 offset where the
ring goes.

| # | line | result |
| --- | --- | --- |
| TS1 | `See [[bob#^fix]] now` | one span: at 4, `bob`, `fix`, wiki |
| TS2 | `See [[bob#^fix\|the fix]]` | one span: at 4, alias `the fix` |
| TS3 | `[[#^fix]]` | one span: at 0, linkpath empty (same note) |
| TS4 | `![[bob#^fix]]` | none (embed) |
| TS5 | `~~[[bob#^fix]]~~` | one span: at 0 (before the strike) |
| TS6 | `` `[[bob#^fix]]` `` | none (inline code) |
| TS7 | `[the fix](bob.md#^fix)` | one span: at 0, `bob.md`, `fix`, md |
| TS8 | `[x](<my note.md#^fix>)` | one span: linkpath `my note.md` |
| TS9 | `[x](my%20note.md#^fix)` | one span: linkpath `my note.md` |
| TS10 | `[x](bob.md#%5Efix)` | one span: blockId `fix` |
| TS11 | `[[bob]] [[bob#Plan]] [x](https://e.com/a#^fix)` | none |
| TS12 | `![x](bob.md#^fix)` | none (embed) |
| TS13 | `[[bob#^fix]] and [[#^ship]]` | two spans, in order |
| TS14 | `[[bob#^not an id]]` | none |
| TS15 | `%%[[bob#^fix]]%%` | none (inline comment) |

**TC — context.** These are document-level line eligibility rules.

| # | where | ring? |
| --- | --- | --- |
| TC1 | prose line in an area note | yes |
| TC2 | inside a fenced code block | no |
| TC3 | inside a multi-line `%% … %%` comment | no |
| TC4 | frontmatter | no |
| TC5 | `⛓️ **DEPENDS ON:** [[#^a]]`, and the same for BLOCKS, LINKS, LINKED FROM, and Sub-projects rows (malformed included) | no |
| TC6 | a `#task` line whose title mentions `[[bob#^fix]]` | yes |
| TC7 | a Cancel, Schedule, Work, or Rotten Log entry | yes |
| TC8 | a daily-note ledger row whose body is `[[bob#^fix]]`, `🍅 [[bob#^fix]]`, `~~[[bob#^fix]]~~`, `🍅 ~~[[bob#^fix]]~~`, or `[[bob#^fix]] #` | no (`ledger_rows = skip`) |
| TC9 | a bullet nested under a ledger Task Link: `see [[bob#^fix]]` | yes |
| TC10 | a ledger entry's direct child with mixed prose: `pair with [[bob#^fix]]` | yes |
| TC11 | `- [[bob#^fix]]` in an area note | yes |
| TC12 | a daily note outside `## Pomodoros` (Notes, Events rows) | yes |
| TC13 | a heading, blockquote, or callout line | yes |

**TR — resolve and state.** Each row gives a target line, or a situation, and its
result.

| # | target | result |
| --- | --- | --- |
| TR1 | `- [ ] #task Fix ^fix` | ready |
| TR2 | `- [*] #task Fix ^fix` | next |
| TR3 | `- [/] #task Fix ^fix` | in-progress |
| TR4 | `- [?] #task Fix ^fix` | blocked |
| TR5 | `- [x] #task Fix ^fix` and `- [X] …` | done |
| TR6 | `- [-] #task Fix ^fix` | cancelled |
| TR7 | `- [>] #task Fix ^fix` | by Tasks status type (DONE gives done, CANCELLED gives cancelled, NON_TASK gives none, anything else gives ready) |
| TR8 | `- [ ] Pack charger ^fix` (no `#task`) | none |
| TR9 | `- Plain bullet ^fix` | none |
| TR10 | paragraph `Some text ^fix` | none |
| TR11 | the id is not in the target note | none |
| TR12 | the note is unresolved | none |
| TR13 | the id ends two lines in the target note | none |
| TR14 | ``- [ ] `#task` in code ^fix`` | none |
| TR15 | Live Preview, same-note target edited from `[ ]` to `[x]`, unsaved | done on the same frame |
| TR16 | Tasks absent or not ready; metadata shows `[/]` and `#task` | in-progress, tooltip without a title |
| TR17 | a `^prj` task, a `#task #ref` reading task, or an Event Task | ring by status (no kind glyph in v1) |

**TT — tooltip.**

| # | case | aria-label |
| --- | --- | --- |
| TT1 | `- [/] #task Fix the parser crash ^fix` in `bob.md`, linked from another note | `Fix the parser crash\nIn Progress · bob` |
| TT2 | same-note Ready target | `<title>\nReady · this note` |
| TT3 | target in `prj/rap/now_rap_target.md` | `<title>\n<Status> · now_rap_target` |
| TT4 | title with links, tags, fields, and `^id` | cleaned per `dependencyCleanTaskText` |
| TT5 | 95-character title | first 79 characters plus `…` |
| TT6 | title unknown (TR16) | `In Progress · bob` |
| TT7 | a stray `::` left in the cleaned title | becomes `:` |

**TG — glyph.**

| # | state | `data-state` | glyph variable | ink variable |
| --- | --- | --- | --- | --- |
| TG1 | ready | `ready` | `--bob-task-link-glyph-ready` | teal identity |
| TG2 | next | `next` | `--bob-task-link-glyph-next` | `--task-status-next` |
| TG3 | in-progress | `in-progress` | `--bob-progress-glyph` | `--task-status-in-progress` |
| TG4 | blocked | `blocked` | `--bob-task-link-glyph-blocked` | `--task-status-blocked` |
| TG5 | done | `done` | `--bob-task-link-glyph-done` | `--task-status-done`, else `--text-muted` |
| TG6 | cancelled | `cancelled` | `--bob-task-link-glyph-cancelled` | `--task-status-cancelled` |

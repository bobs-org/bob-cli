# Blocked-reason marks

This page is the authoritative display contract for blocked-reason
marks: the reason a Blocked (`[?]`) checkbox shows on its face, in
the style of `docs/date-marks.md` and the _Priority marks_ section of
`docs/projects.md`. The JavaScript mirror is `api.blockMarks`
(namespace v1, top-level api version unchanged) in
bob-ledger-tools; its tests run the conformance vectors below
verbatim.

## What you see

Today every Blocked row looks the same: a solid red box with a white
`?` on a red-tinted row. Behind that one look are different
situations that need different responses — finish a prerequisite,
fix the project, or wait or reschedule:

```text
before: - [?] Ship the importer [dependsOn:: x]
        - [?] Review the queue #prj_is_blocked
        - [?] Brush teeth [priority:: medium] [scheduled:: 2026-11-20]
        - [?] Water the plants [repeat:: every day when done] [scheduled:: 2026-11-02]

after:  - [&] Ship the importer …                  ← solid, held by its own prerequisite
        - [^] Review the queue                      ← solid, held by its project
        - [2] Brush teeth …                         ← hollow digit, P2 deferral
        - [⟲] Water the plants …                    ← hollow loop, next run is later
```

Legend (glyphs approximated in plain text):

| Key       | Box                       | Family    | Meaning                                         |
| --------- | ------------------------- | --------- | ----------------------------------------------- |
| `dep`     | solid, white `&`          | `held`    | waiting on its own open prerequisite            |
| `prj`     | solid, white chevron mask | `held`    | its project (or an ancestor project) is blocked |
| `p1`–`p4` | hollow digit `1`–`4`      | `waiting` | deferred by a P1–P4 priority roll               |
| `repeat`  | hollow loop mask          | `waiting` | recurring; the next run is later                |
| `date`    | hollow `?`                | `waiting` | a date picked by hand                           |
| `stale`   | dashed `?`, muted         | `stale`   | Blocked, but no live reason is left             |
| (none)    | today's plain `?`         | —         | the reasons can't be proven yet                 |

## Principles

Display-only: stored Markdown keeps `[?]`, reconcile keeps deriving
it (`docs/task-status-hooks.md`), and the Tasks registry keeps its
single `?` = Blocked (`ON_HOLD`) status. Only the rendered checkbox
changes. With bob-ledger-tools disabled or the toggle off, the vault
looks exactly as it does today. Shape carries meaning; color
whispers: solid boxes mean held by other work, hollow boxes mean
waiting for a date. Truthful or neutral: when a reason can't be
proven, the row shows today's plain `?` rather than a guess.
Reversible: a session toggle restores today's look, and without the
plugin nothing changes. One glyph definition shared by every
surface.

## The marks

Only rows whose stored checkbox symbol is exactly `?` and whose line
is a `#task` line can get a mark. The `#task` tag is the vault's
Tasks global filter. The line must sit outside code and fences, and
the check is quote-aware. Ready ` `, Next `*`, Pending `/`, closed,
and cancelled boxes are never touched, even when the row has a
priority or a future date. A known lane box never gets a block mark.

## Classification and precedence

Inputs, all line-local except the prerequisite lookup:

1. **Open prerequisite.** Parse the `[dependsOn:: a,b]` /
   `(dependsOn:: …)` ids from the line text. In Live Preview this is
   the editor's current text, so unsaved edits count. Look each id up
   in a memo-local `id → tasks` index over the Tasks cache. A target
   is open when its `status.type` is `TODO`, `IN_PROGRESS`, or
   `ON_HOLD`. This mirrors reconcile (`docs/task-status-hooks.md`:
   open targets block; a duplicated id blocks if any instance is
   open).
   - The dependency input is **open** when any id resolves to an open
     task.
   - It is **closed** when every id resolves and none is open, or
     when the line has no ids.
   - It is **unknown** when the Tasks cache isn't ready, or when some
     id resolves to no cached task and no other id is open. The cache
     is ready when the Tasks plugin returns a non-empty task array.
     An unresolved id is neutral, not ignored: the Tasks cache
     indexes only `#task` lines, so an id it can't see may still be
     an open target to reconcile.
2. **Project blocked.** An exact `#prj_is_blocked` tag token (not
   `#prj_is_blocked_old`). This is the sanctioned proxy
   (`docs/task-status-hooks.md`, project blocking).
3. **Own future date.** A canonical `[scheduled:: YYYY-MM-DD]`
   strictly after the local date today. A date that is today does not
   block.
4. **Date source** (only when 3 holds): a readable `[repeat:: …]`
   rule, else a canonical ladder priority, else hand. A canonical
   ladder priority means a lowercase `priority` field with value
   `high`, `medium`, `low`, or `lowest`.

Winner order: **own open prerequisite > project blocked > own future
date**. Within date holds the order is recurring > ladder priority >
hand. If the dependency input is _unknown_, the row shows plain `?`,
because the top-precedence input can't be ruled out. With no live
reason and every input known, the row shows `stale`.

**Digit.** Use the configured ladder label when it matches `^P([1-4])$`
(`P2` → `2`). Otherwise, or when the ladder is unknown (for example
on mobile), use the default map `high` → 1, `medium` → 2, `low` → 3,
`lowest` → 4. Off-ladder `highest` and non-canonical values (`High`,
`P2`, duplicates) are date-held rows with no digit. They show hollow
`?` and keep their urgent `!` mark or repair pill. Ladder membership,
not Schedule Log provenance, picks the digit.

**More reasons.** Every live reason that loses becomes a corner pip
and an `Also` tooltip line. The pip takes the ink of the highest
losing reason. A prerequisite plus a date gives an _earliest_ return,
not a promise: a losing date reads `Also scheduled …`, never `until`.

## Families, tones, and tokens

- **`held` (solid):** today's filled Blocked box (`--task-status-blocked`,
  falling back to `--color-red`) with a white glyph. The line keeps
  today's 13% red tint and 2 px rail.
- **`waiting` (hollow):** a 1.5 px border in the level's ink, a 12%
  wash of that ink, and the glyph in the same ink. Waiting lines use
  `--bob-block-defer-tint: 5.5%` red tint with the rail at 45%
  opacity, so held-by-work rows stand out.
- **`stale`:** a dashed 1.5 px border in `--text-muted`, a muted `?`,
  no wash, no line tint, and no rail.

The ink ramp is one ordinal red-to-grey ramp, not four hues. The hue
budget is spent (`docs/task-tag-marks.md` → _Ink_), and the
priority-marks contract already rejected a per-level hue ramp:

```css
body {
  --bob-block-ink-anchor: color-mix(in srgb, var(--color-red) 72%, var(--text-normal));
  --bob-block-ink-1: var(--bob-block-ink-anchor); /* P1, hand ? */
  --bob-block-ink-2: color-mix(
    in srgb,
    var(--bob-block-ink-anchor) 68%,
    var(--text-muted)
  );
  --bob-block-ink-3: color-mix(
    in srgb,
    var(--bob-block-ink-anchor) 38%,
    var(--text-muted)
  );
  --bob-block-ink-4: color-mix(
    in srgb,
    var(--bob-block-ink-anchor) 12%,
    var(--text-muted)
  ); /* P4, ⟲ */
}
```

A node test (`scripts/test-ledger-tools-block-marks-contrast.cjs`)
computes each glyph-on-wash-on-tint contrast with
Obsidian default-theme tokens, light and dark, and enforces at least
4.5:1. If a value fails, adjust the mix percentages and record the
final values here.

Measured paint calibration (child-epic phase paint_api, 2026-11-02;
base tokens read from the shipped Obsidian bundle: background
`#ffffff` / `#1c1c1c`, normal text `#222222` / `#dadada`, muted text
`#5c5c5c` / `#b3b3b3`, red `#e93147` / `#fb464c`):

- The contract ramp ships unchanged (72% anchor; 68/38/12% level
  mixes): every waiting glyph already meets 4.5:1 on its washed row,
  so no waiting correction was needed. `date` uses the P1 ink and
  `repeat` the P4 ink.
- White on the uncorrected solid red fails (4.20 light, 3.45 dark),
  so held boxes use a local darkening mix of the blocked fill —
  `black 4%` in light (`#e02f44`, white at 4.52) and `black 14%`
  in dark (`#d83c41`, white at 4.52) — without redefining the global
  blocked color.
- Waiting glyph ratios (ink on 12% wash over the 5.5% row tint):
  light P1 4.87, P2 5.36, P3 5.51, P4 5.37;
  dark P1 4.70, P2 4.94, P3 5.35, P4 5.83.

## Glyphs

`&`, the digits, and `?` are text through `--task-status-symbol`, in
the interface font with `font-variant-numeric: tabular-nums`. The
`prj` box draws a chevron-up mask (viewBox `0 0 16 16`, a single
stroked polyline `M4 10.25 8 6.25l4 4`, round caps and joins, stroke
width 2.1, shipped as `--bob-block-glyph-chevron`). It is a mask, not
the `^` character, because `^` sits too high in the interface font.
The `repeat` box reuses the existing `--bob-date-glyph-repeat` mask,
so the checkbox loop matches the `⟲ daily` recurrence mark on the
same line. The pip is a 6 px dot at the box's top-right corner
(`top: -3px; right: -3px`), ringed 1.5 px in `--background-primary`
(`box-shadow: 0 0 0 1.5px`), filled with the highest losing reason's
ink, with visible overflow and no pointer handling.

## Pip

A 6 px dot at the box's top-right corner, ringed 1.5 px in
`--background-primary` and filled with the losing reason's ink (solid
red for `dep`/`prj`). It never changes line height or the box's hit
area.

## Tooltip and accessible name

Tooltip lines are joined with `\n` and never contain `::`. Dates use
the date-marks tooltip grammar `{short date} · {relative}` (for
example `Fri, Nov 20 · in 19 days`).

| Mark      | Line 1                                                                | Line 2                                                                |
| --------- | --------------------------------------------------------------------- | --------------------------------------------------------------------- |
| `dep`     | `Blocked · after <first open prerequisite's clean text>` (+ ` +N`)    | `Ctrl+Shift+B to edit prerequisites`                                  |
| `prj`     | `Blocked · via project <task's note stem>`                            | best effort, one hop: the note's `^prj` own reason, else `via a parent project`; omitted when unknown |
| `p1`–`p4` | `Blocked · P2 deferral · until Fri, Nov 20 · in 19 days`              | `Ctrl+Shift+P to reschedule or change priority`                       |
| `date`    | `Blocked · until Wed, Nov 4 · in 3 days`                              | `Ctrl+Shift+P to reschedule`                                          |
| `repeat`  | `Recurring · next run Mon, Nov 2 · tomorrow`                          | —                                                                     |
| `stale`   | `Blocked, but no reason is left`                                      | `Clears on the next reconcile`                                        |
| + losing  | one line each: `Also after <text>`, `Also via project <stem>`, `Also scheduled <short date> · <relative>` |                              |

The checkbox keeps its checkbox role: `aria-label` and
`data-tooltip-position="top"` go on the input itself. Only the
miniature `render` element uses `role="img"`.

## Surfaces and interaction

| Surface                                                     | Mechanism                                                                              |
| ----------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| Live Preview                                                | `Decoration.line` attributes on `.HyperMD-task-line[data-task="?"]`                    |
| Reading view, embeds, hover previews, Dataview `TASK` views | markdown post-processor sets the same attributes on `li.task-list-item[data-task="?"]` |
| Priority bars / `#prj_is_blocked` pill                      | folded where the box already says it                                                   |
| Tasks query results                                         | CSS on Tasks' own row attributes                                                       |
| `DEPENDS ON` / `BLOCKS` chips                               | miniature box through the shared element builder                                       |
| Task Card                                                   | `api.blockMarks` v1 in bob-navigation-hotkeys                                          |

Row attributes (identical on every JS surface):

- `data-bob-block="<key>"`;
- `data-bob-block-family="held|waiting|stale"`;
- `data-bob-block-more="<losing key>"`, only when a pip shows.

CSS is gated by `body.bob-block-marks` and wins by specificity over
the vault snippet, never by `!important`. It styles only the row's
**own** checkbox: use a child combinator in reading view, and in Live
Preview the line holds exactly one box. Nested child items must never
inherit a mark. Marked rows draw their whole box (border, background,
glyph), so they look right even without the vault snippet. Plain `?`
stays the snippet's job.

Color, border, and background changes may ease over 120 ms or less.
Nothing animates under `prefers-reduced-motion`. In Live Preview,
tooltips attach lazily: a capture-phase `mouseover` on the editor sets
`aria-label` on the hovered checkbox, computed from the line's model,
so Obsidian's tooltip handler sees it on the same hover. Obsidian's
checkbox widget is never replaced or wrapped, and no click is
intercepted: clicking the box does exactly what clicking `[?]` does
today.

## Tasks query results and their known limits

Tasks query result rows get CSS-only marks from Tasks' own row
attributes: no pip, no tooltip (accepted; the row attributes carry no
reason text). A project-tagged row shows `prj` first, because Tasks
rows can't see open prerequisites; document that as a known limit.
Otherwise a future `data-task-scheduled` plus a ladder
`data-task-priority` shows the digit, else a future date shows hollow
`?` (or `⟲` only if Tasks' recurring marker proves usable in the
prototype), else `&` by elimination. The toggle restores Tasks
results.

## Reliability and invalidation

- **Cost.** Classification is O(1) per visible line: field regexes
  plus one index lookup per `dependsOn` id. Rendering never runs a
  CLI subprocess, reads disk, or walks the vault. The `id → tasks`
  index lives on the freshness memo, so it shares the memo's
  invalidation.
- **Invalidation.** Rebuild Live Preview decorations on doc and
  viewport changes, Live Preview or file switches, and a block-marks
  refresh effect. Dispatch that effect on the Tasks `cache-update`
  fan-out, the toggle, ladder config changes, the date-marks midnight
  rollover, and app resume (`visibilitychange`). Never rebuild on
  selection alone. Reading view re-classifies in place on the same
  triggers.
- **Stability.** Line decorations carry stable attribute values, so
  unchanged marks never flicker. Marks never change line height. The
  overlay never intercepts pointer events.
- **Truthful or neutral.** The cache is cold, an id is unresolved, or
  the line is non-canonical: show plain `?`. `stale` appears only
  when every input is known.
- **Fence guard.** Live Preview excludes fenced and inline code through
  the shared syntax-tree helper, which matches the live tree's
  `hmd-codeblock` node name (measured in the real-Obsidian e2e) alongside
  the legacy `HyperMD-codeblock` name and the standard Lezer code names.
  A fenced `?` task stays neutral even though the pure model classifies
  the same text.

## Session toggle

The command **Toggle blocked-reason marks** (id
`toggle-block-marks`) is session-only and on by default. It flips
`body.bob-block-marks`, refreshes editors and rendered views,
triggers the Tasks re-render event, and shows the Notice
`Blocked-reason marks on` / `Blocked-reason marks off`. When off, JS
creates no attributes and the vault looks exactly as today.

## `api.blockMarks` v1

The new namespace leaves the top-level api version unchanged. Every
function is synchronous and never throws.

- `model(lineText, { path })` returns a frozen
  `{ key, family, digit, more, reasons[], tooltip, modelKey }`, or
  `null` when the row gets no mark (neutral, ineligible, or toggled
  off). Each `reasons[]` entry is
  `{ kind: "dep"|"prj"|"date", key, text }`, winner first.
- `render(host, model, { decorative })` appends a listener-free
  miniature box
  `span.bob-block-mark[data-bob-block][data-bob-block-family]` (plus
  `data-bob-block-more` when a losing reason shows as a pip) to
  `host` and returns it, or returns `null` for invalid input, a
  disabled gate, or an unavailable DOM. It gets `role="img"` and
  an `aria-label` unless `decorative`. It resolves the host's own
  document, accepts synthetic digit models, and never requires a
  source task.

`model` ships with the pure classifier; `render` ships with checkbox
rendering (it shares the CSS).

## Rejected alternatives

Stored statuses / a `[>]` Deferred status: a new stored status would
be a second source of truth, touching reconcile, the Tasks registry,
and every writer, and would reopen derived-status decisions. Icons
for every mark: a separate Lucide-style icon per reason doubles the
mask inventory for silhouettes text already draws. Always-visible
reason text: a reason string on every Blocked row clutters 478 rows
to serve what a hover already says. A per-level hue ramp: the hue
budget is spent, and priority marks already rejected it. Priority out
of the box entirely: the digit is the teaching moment that ties the
box to the P1–P4 strip. `role="img"` on the checkbox: it would strip
an interactive control's semantics. Field-presence `&`: showing `&`
for any Depends-On line, open or not, would mark rows whose
prerequisites are all done.

## Conformance vectors

Today is 2026-11-01 (local). The default ladder is P1 `high` 2–7, P2
`medium` 8–30, P3 `low` 31–90, P4 `lowest` 91–365. The Tasks cache is
ready and holds open `[id:: x]`, open `[id:: y]`, and done
`[id:: d]`. No cached task carries `nope`. ⏎ separates tooltip lines.

| #    | Input                                                                                       | Expected                                                                                                          |
| ---- | ------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| BM1  | `- [?] #task A [priority:: medium] [scheduled:: 2026-11-20]`                                | `p2` waiting · `Blocked · P2 deferral · until Fri, Nov 20 · in 19 days⏎Ctrl+Shift+P to reschedule or change priority` · bars folded |
| BM2  | `- [?] #task B [scheduled:: 2026-11-04]`                                                    | `date` waiting · `Blocked · until Wed, Nov 4 · in 3 days⏎Ctrl+Shift+P to reschedule`                              |
| BM3  | `- [?] #task C [repeat:: every day when done] [scheduled:: 2026-11-02]`                     | `repeat` waiting · `Recurring · next run Mon, Nov 2 · tomorrow`                                                   |
| BM4  | `- [?] #task D [dependsOn:: x]`                                                             | `dep` held · `Blocked · after <x text>⏎Ctrl+Shift+B to edit prerequisites`                                        |
| BM5  | `- [?] #task D [dependsOn:: d]`                                                             | `stale` · `Blocked, but no reason is left⏎Clears on the next reconcile`                                           |
| BM6  | BM4 with the Tasks cache not ready                                                          | no mark (plain `?`)                                                                                               |
| BM7  | `- [?] #task E #prj_is_blocked`                                                             | `prj` held · `Blocked · via project <stem>` · pill folded                                                         |
| BM8  | `- [?] #task F [dependsOn:: x] [priority:: low] [scheduled:: 2026-12-20]`                   | `dep` held, more `p3` · adds `Also scheduled Sun, Dec 20 · in 49 days` · bars shown                               |
| BM9  | `- [?] #task G #prj_is_blocked [priority:: medium] [scheduled:: 2026-11-20]`                | `prj` held, more `p2` · bars shown                                                                                |
| BM10 | `- [?] #task H [priority:: highest] [scheduled:: 2026-11-20]`                               | `date` waiting · urgent `!` kept                                                                                  |
| BM11 | `- [?] #task I [priority:: medium] [scheduled:: 2026-11-01]`                                | `stale` (today doesn't block) · bars shown                                                                        |
| BM12 | `- [*] #task J [priority:: high] [scheduled:: 2026-11-20]`                                  | no mark; `*` unchanged; 4 bars                                                                                    |
| BM13 | `- [?] #task K [priority:: high] [scheduled:: 2026-11-05]` whose newest Schedule Log entry is a typed reason | `p1` waiting (ladder membership wins)                                                                |
| BM14 | `- [?] #task L #prj_is_blocked_old`                                                         | not a project token → `stale`                                                                                     |
| BM15 | `- [?] #task M [priority:: High] [scheduled:: 2026-12-01]`                                  | `date` waiting · repair pill still shows                                                                          |
| BM16 | `- [?] plain bullet [scheduled:: 2026-11-20]`                                               | no mark (not a `#task` line)                                                                                      |
| BM17 | `- [?] #task N [dependsOn:: nope]`                                                          | no mark (unresolved id → unknown)                                                                                 |
| BM18 | `- [?] #task O [dependsOn:: nope, x]`                                                       | `dep` held (an open id wins over an unresolved one)                                                               |
| BM19 | `- [?] #task P #prj_is_blocked [dependsOn:: y]`                                             | `dep` held, more `prj` · pill kept                                                                                |
| BM20 | `> - [?] #task Q [priority:: lowest] [scheduled:: 2027-03-03]`                              | `p4` waiting (quote-aware)                                                                                        |
| BM21 | `` - [?] #task R `[scheduled:: 2026-11-20]` ``                                               | `stale` (field in code is not a field)                                                                            |
| BM22 | BM1 with ladder `{label: "Soon", value: medium, 8–30}`                                      | `p2` (non-`P<n>` label falls back to the default map)                                                             |
| BM23 | BM3 evaluated after midnight on 2026-11-02                                                  | `stale`                                                                                                           |
| BM24 | Tasks row `data-task="?"`, `data-task-priority="medium"`, future `data-task-scheduled`, no project tag | CSS `2` (Tasks results)                                                                     |
| BM25 | `- [?] #task S [dependsOn:: x, d]`                                                          | `dep` held (an open id wins over a closed one)                                                                    |
| BM26 | `- [ ] #task T [priority:: high] [scheduled:: 2026-11-20]`                                  | no mark (not a Blocked box)                                                                                       |
| BM27 | `- [?] #task U [scheduled:: 2026-10-22]`                                                    | `stale` (past date blocks nothing; no priority, no tag, no ids)                                                   |

## Live verification (real-Obsidian e2e, bob-plugins `scripts/obsidian-block-marks-e2e.cjs`)

Verified in an isolated scratch vault (light and dark, Live Preview and
reading view) with screenshots reviewed against the accepted mockup:

- every applicable BM row renders its mark in both views and both themes
- `dep` rows read `&`, `prj` rows read the chevron, digit rows read hollow `1`–`4`
- waiting rows are quieter than held rows (5.5% tint vs 13%)
- hovering a marked checkbox shows its tooltip text; clicking the box
  stores exactly the same status with the gate on and off
- the toggle removes all marks and restores the prior appearance, then
  turns the marks back on, without changing row height
- nested child items never inherit a mark
- embeds, hover previews, and Dataview TASK rows carry marks where their
  origin is provable; unprovable origins stay neutral; Tasks query rows
  stay unmarked by JS (their CSS marks belong to a later phase)
- fenced code stays neutral; quoted tasks mark in both views

Still pending for the parent visual-review phase: a real reconcile
turning a row `stale` and then clear, iOS glyph rendering, third-party
checkbox packs, human scrolling checks, a 100-task project note scroll,
and the Tasks-results CSS marks (BM24).

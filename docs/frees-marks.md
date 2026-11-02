# Frees marks

This guide is the authoritative display contract for frees marks. The JavaScript
mirror lives in bob-ledger-tools: `src/141b-frees-marks.js` (pure core),
`src/271b-plugin-frees-marks.js` (Live Preview extension, Reading view, refresh,
and toggle). Its tests run the conformance vectors below verbatim in
`scripts/test-ledger-tools-frees-marks.cjs`. There is no api namespace.

Decided for v1 (`bob-cli-g1`): the pill is compact (`pill_text = compact`). It reads
`🔑 N` — about half as wide as spelling the word out — and its meaning comes from
the tooltip. The rejected `words` form would have read `🔑 frees N`.

## What you see

In today's daily note, each Task Link under an open Pomodoro whose task is the last
blocker of other open tasks wears a quiet trailing mark that says how many tasks
closing it would free:

```text
- [ ] () — READ
	- sase > ^ref-the-ai-native-sdlc-playbook 🔑 1
```

Hovering the mark shows:

```text
Closing this frees 1 task
• Launch another epic chain for sase task beads! · sase
```

This is the forward-looking partner of the existing ledger marks. ◐ before a link means
"in progress now". 🔓 after a link means "something done today freed this task". The new
🔑 after a link means "finishing this frees N tasks". It helps Bryan put unlocking work
first when he plans the day.

## Principles

- **Display-only.** Nothing writes the mark; task and ledger source stay unchanged.
- **Truthful or neutral.** Uncertainty, cold memo, duplicate or unresolvable targets,
  closed or id-less tasks, and any thrown error produce no mark. No warning glyph
  appears anywhere, because a link to a non-qualifying task is legitimate.
- **One rule shared with BLOCKS.** The pill's N always equals the `lastCount` in the
  task's own BLOCKS row summary. Both read the same memo through the same two helpers,
  so the ledger and the task's own note can never disagree.
- **What the number promises.** In the same Tasks snapshot, every counted task leaves
  Blocked the moment X closes. The Ctrl+Enter successor picker offers each one as a
  hand-off, with three exceptions, and the tooltip marks the first two:
  - a `#hide` task is recovered but never offered;
  - a `^prj` task counts as one, as it does on BLOCKS, and its open residents are
    offered as the `Project ‹note›` group;
  - a task already linked today is not offered again.
- **What is deliberately not counted, and why.** These never use 🔓/`"unblocked"`
  wording in the successor copy rules (§12.6), so they don't belong in `"frees"`
  either:
  - keep-moving offers (⏭, §12.15). These are already-Ready tasks of a leaf project,
    never dependents.
  - relation successors (🪢 `starts after`, §12.18). These are never blocked, only
    sequenced. They are deferred as a possible separate 🪢 segment.
  - project-finished pull-ins (§12.10). Closing X finishes the project; it frees
    nothing.
  - dependents still waiting on other work. They show in the tooltip as
    `Also blocks …`, never in the pill.

## Which links get the mark

A link gets a mark only when **all** of these hold. They are the same gates the 🔓
glyph uses.

1. The note is **today's** canonical daily note (`currentTodayDailyPath`). Other daily
   notes never get it.
2. The link is a dedicated Task Link row that `progressMarkLinks` accepts. The row is a
   direct child, at the first child indent, of an **open** Pomodoro entry (running, or a
   planned `()` placeholder, named or unnamed). Its body is exactly one
   `[[target#^id]]`, with an optional alias, an optional leading `🍅`, and an optional
   trailing ` #` move-only marker. Excluded: struck `~~[[…]]~~`, embedded `![[…]]` (the
   transclusion already shows the task's own BLOCKS row), mixed prose, deeper bullets,
   fenced lines, closed entries, Events and every other section.
3. The target resolves through the Tasks memo to exactly one `#task`, using the same
   path/block-id resolution as `unblockedGlyphLinkedTask`. Same-note `[[#^id]]` targets
   resolve against the daily path. The target is OPEN and carries an `[id::]`.
4. N ≥ 1.
5. The Tasks memo is warm. A cold memo draws nothing.
6. The session toggle is on.

**Truthful or neutral.** Unresolved, ambiguous, closed, or id-less targets, and any
thrown error, show no mark. Nothing ever writes to the vault. Without the plugin, the
vault looks exactly as it does today.

## The count rule

```text
X       = the task a qualifying ledger link resolves to (Tasks memo; see "Which links")
x       = X's [id::] value (Tasks `id`), trimmed. No [id::] → no mark: closing X can
          never unblock anyone (task-dependencies.md §12.2 GATE)
OPEN(t) = t.status.type ∈ {TODO, IN_PROGRESS, ON_HOLD}
LIVE(x) = blocksDependentsIndex(memo).get(x) minus X itself
          (open tasks whose dependsOn names x; exactly the BLOCKS row's live set)
FREED   = [D ∈ LIVE(x) : blocksLastBlocker(D, X, memo, memo.dateText).lastBlocker]
          (no other dependsOn id resolves to an open task, no scheduled date after
           today, no #prj_is_blocked: exactly the BLOCKS 🔑 set)
N       = |FREED|;  WAITING = |LIVE(x)| − N
mark   ⇔ OPEN(X) ∧ N ≥ 1
```

**Parity invariant.** For every task X, the pill's N equals the `lastCount` in X's own
BLOCKS row summary (`frees N`, or the N in `frees N of M`). Both read the same memo,
`this.freshnessEnsureMemo()`, through the same two helpers. A test asserts this on every
count fixture. The ledger and the task's own note can never disagree.

## Anatomy and look

- Host element:
  `span.bob-frees-mark[role=img][aria-label][data-tooltip-position=top][data-count][data-block-id]`.
  It holds `span.bob-frees-mark-key` (the text `🔑`) followed by the count text `N`
  (decided `pill_text = compact`; the rejected `words` form would have read
  `frees N`). It has no `title` attribute and is listener-free, so it survives
  Dataview's innerHTML round-trip. A DOM builder takes a fake `doc`, as
  `buildProgressMarkElement` does.
- Quiet, muted, borderless at rest. The emoji key carries the only color, so no hue from
  the spent budget (`docs/task-tag-marks.md` → Ink) is used. Starting CSS, in
  `plugins/bob-ledger-tools/styles.css` after the unblocked-glyph block:

```css
.bob-frees-mark {
  display: inline-flex;
  align-items: baseline;
  gap: 0.22em;
  margin-inline-start: 0.4em;
  padding: 0 0.42em;
  border-radius: 999px;
  font-family: var(--font-interface);
  font-size: 0.8em;
  font-variant-numeric: tabular-nums;
  line-height: 1;
  white-space: nowrap;
  vertical-align: baseline;
  color: var(--bob-frees-mark-color, var(--text-muted));
  user-select: none;
  cursor: default;
  print-color-adjust: exact;
  transition:
    background-color 120ms ease,
    color 120ms ease;
}
.bob-frees-mark-key {
  font-size: 0.95em;
}
.bob-frees-mark:hover {
  color: var(--text-normal);
  background-color: color-mix(in srgb, currentColor 12%, transparent);
}
.bob-frees-mark.bob-frees-mark-tick {
  animation: bob-frees-mark-tick 180ms cubic-bezier(0.2, 0.9, 0.3, 1.25);
}
@keyframes bob-frees-mark-tick {
  from {
    opacity: 0.3;
    scale: 0.6;
  }
  to {
    opacity: 1;
    scale: 1;
  }
}
@media (prefers-reduced-motion: reduce) {
  .bob-frees-mark,
  .bob-frees-mark.bob-frees-mark-tick {
    animation: none;
    transition: none;
  }
}
@media (forced-colors: active) {
  .bob-frees-mark {
    color: CanvasText;
  }
}
body:not(.bob-frees-marks) .bob-frees-mark {
  display: none;
}
```

The mark never changes line height, never wraps inside itself, and never takes document
positions. `--bob-frees-mark-color` is a theme hook that is unset by default.

## Placement

- **Live Preview:** a zero-width `Decoration.widget` at the link token's **end** (just
  after `]]`, before any trailing ` #`), with `side: 2`. 🔓 sits at the same offset with
  `side: 1`, so a link that has both reads `link 🔓 🔑 N`: the past (freed by),
  then the future (frees). The widget covers no text, so it stays put while the cursor
  reveals raw `[[…]]`, and it never rebuilds on selection alone.
- **Reading view:** insert the host right after the `span.bob-unblocked-glyph` that
  follows the link if one is present. Otherwise insert it right after the
  `a.internal-link`, or after its `del`/`s` wrapper when the anchor is that wrapper's
  only child.

## Tooltip (`aria-label`, Obsidian tooltip, never contains `::`)

```text
Closing this frees N task|tasks
• ‹title› · ‹note›                    (up to 3 rows, BLOCKS canonical order)
• 🧩 ‹project note› project            (a ^prj dependent)
• ‹title› · ‹note› · hidden            (a #hide dependent: freed, not offered)
+K more                                 (when N > 3)
Also blocks W waiting on other work     (when WAITING = W ≥ 1)
```

- Titles are cleaned with `dependencyCleanTaskText`, then cut to 48 characters (47 plus
  `…`), following the §12.6 copy rule.
- `‹note›` is the dependent's file basename without `.md`, or `this note` when it lives
  in today's daily note.
- Rows use the same order as `blocksSortLinks`, so the tooltip lists tasks in the order
  X's BLOCKS row shows them.
- Any stray `::` becomes `:`.

## Interaction

- **Plain click (mousedown):** place the caret at the widget position (the link end) and
  focus the editor. Never navigate, never write. This matches ◐/🔓.
- **Mod+click (primary button):** `workspace.openLinkText(linktext, sourcePath, "tab")`,
  the same as Mod-clicking the link. It opens X's block, where the BLOCKS row lists the
  🔑 dependents.
- No tab stop and no live-region announcement.

## Motion

- No animation on first paint, startup, scroll, or rerender.
- Each editor view keeps a bounded LRU map (about 2,000 keys) of
  `path\0blockId → last N`, for every eligible link it has evaluated, including N = 0.
  When a key already in the map changes to a new N ≥ 1, the host gets
  `bob-frees-mark-tick` for about 400 ms. So a pill that appears or changes count
  because of an edit elsewhere pops once. A pill whose count drops to 0 simply goes
  away.
- Reading view never animates.
- Reduced motion turns this off. Copy the `task-link-marks` swap machinery
  (`149-task-link-mark-live-model.js`) where it fits.

## Liveness

One coalesced `scheduleFreesMarksRefresh()` (about 120 ms) runs on:

- the `scheduleLiveWidgetRefresh()` fan-out (the Tasks `cache-update`), where dependents
  in other notes change;
- metadataCache `changed` for today's daily path;
- the local-midnight rollover;
- the toggle.

Live Preview also rebuilds on `docChanged`, `viewportChanged`, and the Live Preview ↔
source switch. The Reading-view refresh sweeps stamped hosts in today's daily note,
adding, updating, or removing them in place. Copy the 🔓 mixin's structure method for
method.

## Toggle

The command `toggle-frees-marks` is named "Toggle Task Link frees marks". It is a
session toggle, default on, using `applyMarkGateClass("bob-frees-marks", on)` plus
`registerMarkGatePopoutSync`. v1 has no api namespace.

## Performance

- Build the dependents index once per memo generation (it is already cached on the memo
  by `blocksDependentsIndex`).
- Resolve each unique `(linkpath, blockId)` once per build.
- Build only for visible ranges.
- Never read from disk during render.
- A node perf test with a synthetic 200-link ledger and 2,000-task memo keeps one build
  under about 25 ms.

## Deferred

Deferred with triggers:

- A separate `🪢 N` "starts after this" segment for relation successors. Trigger:
  `relations.successor_labels` is configured and Bryan asks.
- Echoes in Bob Mac Capture's idle agenda, the Task Card, and nav pickers. Trigger:
  Bryan asks. It needs a Rust/JSON count, because the Mac is a thin client.
- Joint unlocks: "frees 1 with ^y" when two of today's planned tasks together free a
  dependent.
- Links under closed entries, other daily notes, and prose links.
- Counting a freed project's residents instead of the project as one.

## Conformance vectors

Tests run these vectors verbatim. Pill text shows the decided compact form (`🔑 N`).

**FL — which links.** Each row is today's daily note `## Pomodoros`, and its target X is
open, has an `[id::]`, and frees 1 unless stated otherwise.

| #    | Ledger                                                        | Mark?                                    |
| ---- | ------------------------------------------------------------- | ---------------------------------------- |
| FL1  | `- [ ] () — READ` / `\t- [[sase#^a]]`                         | yes                                      |
| FL2  | `- [ ] (**1210-1235** [t:: 25m]) — BUILD` / `\t- [[sase#^a]]` | yes                                      |
| FL3  | `- [x] (**0800-0815** [t:: 15m])` / `\t- [[sase#^a]]`         | no (closed entry)                        |
| FL4  | open entry / `\t- 🍅 [[sase#^a]]`                             | yes                                      |
| FL5  | open entry / `\t- [[sase#^a]] #`                              | yes; widget at the link end, before ` #` |
| FL6  | open entry / `\t- ~~[[sase#^a]]~~`                            | no                                       |
| FL7  | open entry / `\t- ![[sase#^a]]`                               | no                                       |
| FL8  | open entry / `\t- read [[sase#^a]] first`                     | no                                       |
| FL9  | open entry / `\t\t- [[sase#^a]]` under another child          | no                                       |
| FL10 | same ledger in a daily note that is not today's               | no                                       |
| FL11 | `## Events` row linking `[[sase#^a]]`                         | no                                       |
| FL12 | open entry / `\t- [[sase#^a\|Read it]]`                       | yes                                      |
| FL13 | open entry / `\t- [[#^a]]`, X in the daily note itself        | yes                                      |
| FL14 | open entry inside a fenced block                              | no                                       |
| FL15 | Tasks memo cold (no tasks)                                    | no                                       |
| FL16 | toggle off                                                    | no (CSS gate and JS)                     |

**FC — count.** X is `- [/] #task Read it [id:: x] ^a` in `sase.md`. Today is
2026-11-02.

| #    | Dependents (all `#task`)                              | N / WAITING                                      | Mark                                              |
| ---- | ----------------------------------------------------- | ------------------------------------------------ | ------------------------------------------------- |
| FC1  | `[?] D [dependsOn:: x]`                               | 1 / 0                                            | `🔑 1`                                            |
| FC2  | `[?] D [dependsOn:: x,y]`, y open                     | 0 / 1                                            | none                                              |
| FC3  | `[?] D [dependsOn:: x,y]`, y done                     | 1 / 0                                            | `🔑 1`                                            |
| FC4  | `[?] D [dependsOn:: x] [scheduled:: 2026-11-03]`      | 0 / 1                                            | none                                              |
| FC5  | `[?] D [dependsOn:: x] [scheduled:: 2026-11-02]`      | 1 / 0                                            | `🔑 1`                                            |
| FC6  | `[?] D [dependsOn:: x] #prj_is_blocked`               | 0 / 1                                            | none                                              |
| FC7  | `^prj` task of `sase_blog.md` with `[dependsOn:: x]`  | 1 / 0                                            | `🔑 1`; tooltip row `🧩 sase_blog project`         |
| FC8  | `[?] D #hide [dependsOn:: x]`                         | 1 / 0                                            | `🔑 1`; tooltip row ends `· hidden`                |
| FC9  | X has no `[id::]`                                     | —                                                | none                                              |
| FC10 | X is `[x]`                                            | —                                                | none                                              |
| FC11 | `[x] D [dependsOn:: x]` and `[-] E [dependsOn:: x]`   | 0 / 0                                            | none                                              |
| FC12 | `[?] D [dependsOn:: x,zzz]`, zzz resolves to nothing  | 1 / 0                                            | `🔑 1` (unresolved never blocks, R4)              |
| FC13 | 3 freeing, 2 waiting                                  | 3 / 2                                            | `🔑 3`; tooltip lists 3 + `Also blocks 2 waiting on other work` |
| FC14 | 5 freeing                                             | 5 / 0                                            | `🔑 5`; tooltip lists 3 + `+2 more`               |
| FC15 | stale `[ ] D [dependsOn:: x]` (reconcile not yet run) | 1 / 0                                            | `🔑 1` (matches SL15: still offered)              |
| FC16 | X names itself: `[dependsOn:: x]` on X                | 0 / 0                                            | none                                              |
| FC17 | every FC row                                          | pill N == `blocksChipModel(...)` lastCount for X | parity                                            |

**FT — tooltip.**

| #   | Case                                                                   | aria-label                                                                           |
| --- | ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| FT1 | FC1, D = `Launch another epic chain for sase task beads!` in `sase.md` | `Closing this frees 1 task\n• Launch another epic chain for sase task beads! · sase` |
| FT2 | N = 2, one in the daily note                                           | second row ends `· this note`; header says `2 tasks`                                 |
| FT3 | a 60-character title                                                   | first 47 characters plus `…`                                                         |
| FT4 | a title with links, tags, fields, `^id`                                | cleaned per `dependencyCleanTaskText`                                                |
| FT5 | a stray `::`                                                           | becomes `:`                                                                          |
| FT6 | WAITING = 1                                                            | last line `Also blocks 1 waiting on other work`                                      |

**FP — placement.**

| #   | Case                                     | Result                                                   |
| --- | ---------------------------------------- | -------------------------------------------------------- |
| FP1 | Live Preview                             | widget at the link end, `side: 2`; no line-height change |
| FP2 | the same link also has 🔓                | order: link, 🔓, 🔑 mark                                 |
| FP3 | Reading view, no 🔓                      | host directly after `a.internal-link`                    |
| FP4 | Reading view with 🔓                     | host directly after `span.bob-unblocked-glyph`           |
| FP5 | cursor on the row (raw `[[…]]` revealed) | mark still at the link end                               |

## Live verification (real-Obsidian e2e, bob-plugins `scripts/obsidian-frees-marks-e2e.cjs`)

The offline `--self-test` plus `scripts/test-ledger-tools-frees-marks.cjs` cover the
FL/FC/FT/FP vectors, compact pill text (`🔑N`, never `frees`), BLOCKS last-blocker
parity, and Reading insertion beside a `bob-task-link-mark` sibling and through one
wrapping `<p>`.

Verified 49/49 in an isolated scratch vault (light and dark, Live Preview and
Reading) with screenshots reviewed against Anatomy and look. Compact pill, quiet
muted rest color, baseline-aligned key, 🔓 then 🔑 on the both-marks row, and a CDP
hover whose visible `.tooltip` matches the aria-label. CSS metrics from the live
pass: mark 36.1×13.2px, `gap: 0.22em` (2.82px), `margin-inline-start: 0.4em`
(5.12px), `font-size: 0.8em` (12.8px), `vertical-align: baseline`; row height 26.375px
with the gate on and off. No CSS values changed.

Environment: Obsidian 1.15.0 (`/snap/bin/obsidian`), `xvfb-run -a`, never the live
vault. Tasks plugin `taskFormat: dataview` so `[id::]` / `[dependsOn::]` parse.

- FC1, FC7, FC8, FC13, FC14, same-note `[[#^same-1]]`, and the 🔓+🔑 row render the
  compact pill in both views and both themes; FC2 stays bare
- closed-entry, struck, embed, mixed-prose, deeper, and Events links stay bare
- Reading FP3 places the host after `a.internal-link`; FP4 places it after
  `span.bob-unblocked-glyph`
- a CDP hover plus `Input.dispatchMouseEvent` shows the visible `.tooltip` text for
  FC1: `Closing this frees 1 task` / `• Launch another epic chain for sase task beads! · sase`
- plain click places the caret at the link end and writes nothing; Mod+click opens
  the owner note in a tab
- an edit that adds a second FC1 dependent changes the pill from 1 to 2 within about
  a second and fires `bob-frees-mark-tick` once on Live Preview
- the session toggle removes every mark, leaves row height unchanged, then restores
  the marks

Still pending: iOS glyph rendering, human scrolling checks, and a 100-link ledger
scroll. Joint unlocks and a 🪢 segment stay deferred per this contract.

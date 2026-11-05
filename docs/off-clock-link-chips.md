# Off-clock link chips

This guide is the authoritative display contract for off-clock link chips. The
JavaScript mirror lives in bob-ledger-tools: `src/141c-off-clock-link-chips.js`
(pure core), `src/271c-plugin-off-clock-link-chips.js` (Live Preview extension,
refresh, and toggle), and `src/271d-plugin-off-clock-link-chips-reading.js`
(rendered views). Its tests run the conformance vectors below verbatim in
`scripts/test-ledger-tools-off-clock-link-chips.cjs`. There is no api namespace.

## What you see

In a daily note's Pomodoro ledger, every Task Link whose task resolves off the
clock wears the same dashed `timer-off` `off` chip that off-clock Event Tasks
already wear, whether or not the task's own line carries `#off`:

```text
before:
- [ ] () — LUNCH
	- love > ^lunch
- [ ] (**1210-1235** [t:: 25m]) — MEET
	- this note > ^standup-1210
	- this note > ^school-pickup

after:
- [ ] () — LUNCH
	- love > ^lunch ⊘off                 (inherited: love.md has offclock: true)
- [ ] (**1210-1235** [t:: 25m]) — MEET
	- this note > ^standup-1210
	- this note > ^school-pickup ⊘off    (Event Task tagged #off)
```

(`⊘off` stands for the existing dashed `timer-off` chip.) In the MEET example the
sibling link has no chip, which is also the visual explanation of why that mixed
session stays on the work clock.

## Principles

- **Display-only.** Nothing is written. No `#off` is ever materialized onto a task
  line or a ledger row. Without the plugin the vault looks exactly as it does
  today.
- **One rule, one resolver.** A chip appears iff the session-clock writer would
  resolve that link off-clock: the chip runs `pomodoroClockLinkFacts` with the same
  `resolveTask` / `resolveClock` sources `se<X>` uses, parameterized by the rendered
  note's path instead of the active file. A fact with `offClock === true` gets a
  chip; `false` or `null` (unresolved) gets none.
- **About the task, not the session.** The chip shows the linked task's current
  effective clock. It never reads, judges, or changes the session's stamped `[t::]`
  / `[off::]` key (a closed row's key is history). A mixed lineup shows chips only
  on its off-clock links.
- **Truthful or neutral.** Unresolved notes, missing or ambiguous same-note ids,
  lines that are not checkbox rows, a note whose text is not cached yet, and any
  thrown error show no chip and no warning.
- **Same look as the Event Task chip.** One shared CSS rule draws both chips;
  changing it changes both.

## Which links get the chip

A link gets a chip only when **all** of these hold:

1. The rendered note is a canonical daily note (`PLAN_DAILY_PATH_RE`). Any canonical
   daily note qualifies, under any Pomodoro entry, open or closed: closed entries
   and past daily notes wear chips too, because the clock belongs to the task, like
   the `#off` tag it stands in for.
2. The line is a dedicated ledger Task Link row per `taskLinkMarkIsLedgerTaskLinkLine`:
   plain, `🍅`-prefixed, struck `~~[[…]]~~`, aliased, or followed by the ` #`
   move-only marker. The chip describes the task, so struck (completed) and ` #`
   (deferred) rows are included even though ` #` rows do not count toward the
   session clock.
3. Not embedded (`![[…]]` renders the task's own line), not mixed prose, not a deeper
   bullet, not fenced, not in frontmatter or a `%%` comment, not in `## Events` or
   any other section. Prose task links keep only their status ring.
4. The link's fact resolves `offClock === true`.
5. The session toggle is on.

## Resolution (what "off-clock" means here)

Exactly the counted-links rule's per-link step
(`docs/reference/vault-format.md` → "Task clock tags and note defaults"): a
standalone `#on` on the task line wins (no chip, even beside an off spelling); else
`#off`, `#offclock`, `#off-clock`, or `#untracked` (case-insensitive, standalone,
outside code spans and Markdown links) is off; else the nearest area or project
note on the root walk that declares a clock policy decides (a valid
`[root:: NOTE]` starts the walk; untyped notes are walked through): a non-empty
`offclock_types:` list decides whatever the note's `offclock:` says (off-clock
iff the task's effective type key is listed), otherwise the nearest
`offclock: true|false`; an invalid nearer value is on-clock); else on-clock. Same-note links (`[[#^id]]`, including Event Tasks in the daily note's
`## Events`) read the note's own current lines — in Live Preview the live editor
document, so an unsaved `#off` edit shows on the same frame. Other-note links read
the target line through the writer's source (metadata-cache block line +
source-text cache).

## Anatomy and look

- Host:
  `span.bob-off-clock-link-chip[role=img][aria-label][data-tooltip-position=top][data-clock-source][data-block-id]`,
  text content exactly `off` (always the canonical spelling, even for an alias or an
  inherited clock; the tooltip names the source). `data-clock-source` is `off_tag` or
  `note`. No `title` attribute; listener-free DOM builder that takes a `doc`, like
  `buildFreesMarkElement`.
- Look: identical to the Event Task chip. One shared rule serves both: the
  `timer-off` mask variable is defined on `body` as `--bob-off-clock-icon`, and
  `.bob-off-clock-link-chip` joins the Event Task chip's look rule, its `::before`
  icon rule, and its `forced-colors` rule — without scoping the new class under
  `.bob-calendar-events`, so turning calendar painting off does not hide link chips.
  Chip-only additions: `margin-inline-start: 0.4em`, `vertical-align: baseline`,
  `user-select: none` (copying never copies `off`), `cursor: default`,
  `print-color-adjust: exact`. The chip never changes line height and takes no
  document positions.
- Gate:
  `body:not(.bob-off-clock-link-chips) .bob-off-clock-link-chip { display: none; }`;
  the JS checks in the builder and the sweep remain the source of truth.

## Placement

- **Live Preview:** a zero-width `Decoration.widget` at the link end — just after
  `]]`, or after the closing `~~` of a struck link, and before any trailing ` #` —
  with `side: 3`, so a link that also has 🔓 (`side: 1`) and 🔑 (`side: 2`) reads
  `link 🔓 🔑 N off`. The chip stays put while the cursor reveals raw `[[…]]`.
- **Reading view:** insert the host after the run of `span.bob-unblocked-glyph` /
  `span.bob-frees-mark` siblings that follows the link's `a.internal-link` (or its
  `del`/`s` wrapper when the anchor is that wrapper's only child); with none,
  directly after the anchor or wrapper. A later 🔓 or 🔑 insertion lands before the
  chip on its own, so the order converges in any processor order.
- **Allow-list:** `bob-off-clock-link-chip` is in `ledgerMarkClass` in
  `progressMarkReadingContext`, so ◐, 🔓, 🔑, and the ring's ledger-row exclusion
  still recognize a row that already wears a chip. Without this, a chipped ledger
  row would stop being a ledger row and could grow a status ring.

## Tooltip (`aria-label`, Obsidian tooltip, one line, never contains `::`)

- `off_tag`: `Off-clock · tagged <spelling>`, where `<spelling>` is the tag exactly
  as the task line writes it (from `eventTaskOffClockTagRange`), e.g.
  `Off-clock · tagged #off`, `Off-clock · tagged #Untracked`.
- `note`: `Off-clock · from <stem>` via `gcalEventsFromNoteTitle` — the same copy
  the inherited Event Task time pill uses, e.g. `Off-clock · from love`.
- Any stray `::` becomes `:`.

## Interaction

A plain mousedown in Live Preview places the caret at the widget position and
focuses the editor; Mod+click (primary button) opens the linked task like
Mod-clicking the link (`workspace.openLinkText(linktext, sourcePath, "tab")`). Never
navigates on a plain click, never writes, no tab stop, no live-region announcement.
No animation.

## Liveness

One coalesced `scheduleOffClockLinkChipsRefresh()` (about 120 ms) runs on:

- the `scheduleLiveWidgetRefresh()` fan-out;
- `metadataCache` `changed` (any note: an `offclock:` / `offclock_types:`, `type:`, or `parent:` edit
  changes inheritance, and a target line can gain or lose a tag) and `resolved`;
- vault `rename` and `delete`;
- a source-text cache fill (`taskThemeRequestSourceText` already refreshes theme
  marks and home chips there; this refresh runs beside them);
- the toggle.

Live Preview also rebuilds on `docChanged`, `viewportChanged`, the Live Preview ↔
source switch, and a file switch — never on selection alone. The Reading refresh
sweeps rendered daily notes in every window (main and popouts), adding, updating,
or removing hosts in place.

**Source-text freshness (small shared fix).** On `metadataCache` `changed(file,
data)`, when `file.path` is already in the source-text cache, the cached text is
replaced with `data` (no new entries are added). Today a note edited outside an open
editor (sync, `bob` writes) keeps stale cached text, so both home chips and `se<X>`
can read an old line; this makes the chip, home chips, and the writer agree with
the file.

## Toggle

The command `toggle-off-clock-link-chips` is named "Toggle Task Link off-clock
chips". It is a session toggle, default on, using
`applyMarkGateClass("bob-off-clock-link-chips", on)` plus
`registerMarkGatePopoutSync`. It is independent of "Toggle calendar event
painting". There is no api namespace.

## Performance

Resolve each unique `(pathPart, blockId)` once per build or sweep; add Live Preview
widgets only inside visible ranges; never read from disk during render (the
source-text cache fills asynchronously outside render, as today). A node perf test
with a synthetic 200-link ledger over 50 target notes keeps one Live Preview build
under about 25 ms.

## Deferred

Deferred with triggers:

- Bob Mac Capture's idle agenda rows, the Task Card, and nav pickers. Trigger:
  Bryan asks. The Mac is a thin client, so this needs a per-link clock in bob's
  `capture-pomodoros --tasks` JSON first.
- Embedded `![[…]]` ledger rows.
- An `#on` override marker on links whose task opts back onto the clock under an
  off-clock note.
- A tooltip line naming the session effect (for example why a mixed session stays
  worked).
- Prose links. Trigger: Bryan asks.

## Conformance vectors

Tests run these vectors verbatim.

Shared fixture unless a row says otherwise: daily note `2026/20261103.md` (today,
2026-11-03); `love.md` has `type: "[[area]]"` and `offclock: true` and holds
`- [ ] #task Lunch ^lunch`; `bob.md` has `type: "[[project]]"`, no `offclock`, and
holds `- [ ] #task Fix the parser ^parser`.

**OC — which links.**

| #    | Ledger (under `## Pomodoros`)                                       | Chip?                          |
| ---- | ------------------------------------------------------------------- | ------------------------------ |
| OC1  | `- [ ] () — LUNCH` / `\t- [[love#^lunch]]`                          | yes                            |
| OC2  | `- [x] (**1200-1300** [off:: 60m]) — LUNCH` / `\t- [[love#^lunch]]` | yes                            |
| OC3  | open entry / `\t- 🍅 [[love#^lunch]]`                               | yes                            |
| OC4  | open entry / `\t- [[love#^lunch]] #`                                | yes; at the link end, before ` #` |
| OC5  | open entry / `\t- ~~[[love#^lunch]]~~`                              | yes; after the closing `~~`    |
| OC6  | open entry / `\t- [[love#^lunch\|Lunch]]`                           | yes                            |
| OC7  | open entry / `\t- ![[love#^lunch]]`                                 | no (embed)                     |
| OC8  | open entry / `\t- lunch with [[love#^lunch]]`                       | no (mixed prose)               |
| OC9  | open entry / `\t\t- [[love#^lunch]]` under another child            | no (deeper bullet)             |
| OC10 | OC1 in the past daily note `2026/20261102.md`                       | yes                            |
| OC11 | `## Events` row or a `## Notes` bullet linking `[[love#^lunch]]`    | no (prose links)               |
| OC12 | OC1 inside a fenced block                                           | no                             |
| OC13 | OC1 in a non-daily note that has a `## Pomodoros` heading           | no                             |
| OC14 | open entry / `\t- [[bob#^parser]]`                                  | no (on-clock)                  |
| OC15 | toggle off                                                          | no (CSS gate and JS)           |

**OR — resolution.** Each row is an OC1-shaped ledger link to the given task.

| #    | Linked task                                                                                | Chip / `data-clock-source`        |
| ---- | ------------------------------------------------------------------------------------------ | --------------------------------- |
| OR1  | `- [ ] #task Walk #off ^a` in `bob.md`                                                     | yes / `off_tag`                   |
| OR2  | `- [ ] #task Walk #OFFCLOCK ^a` in `bob.md`                                                | yes / `off_tag`; text still `off` |
| OR3  | untagged task in `abby.md` (area, `parent: "[[love]]"`, no `offclock`)                     | yes / `note` (love)               |
| OR4  | untagged task in `fun.md` (area under love, `offclock: false`)                             | no                                |
| OR5  | `#off` task in `fun.md`                                                                    | yes / `off_tag`                   |
| OR6  | `- [ ] #task Lunch #on ^a` in `love.md`                                                    | no                                |
| OR7  | `- [ ] #task Lunch #on #off ^a` in `love.md`                                               | no (`#on` wins)                   |
| OR8  | untagged task in `odd.md` (area under love, `offclock: maybe`)                             | no                                |
| OR8b | `[type:: MEET]` task in `meet.md` (area under love, `offclock_types: [MEET]`)               | yes / `note` (meet)               |
| OR8c | untagged task in `meet.md` (area under love, `offclock_types: [MEET]`)                     | no                                |
| OR9  | untagged task in `dog.md` (no `type`, `parent: "[[love]]"`)                                | yes / `note` (love)               |
| OR10 | untagged `- [ ] #task Swim [root:: abby] ^a` in `inbox.md`                                 | yes / `note` (love)               |
| OR11 | same-note `[[#^evt]]` to Event Task `- [ ] **1200-1230** School pickup #off ^evt`          | yes / `off_tag`                   |
| OR12 | same-note `[[#^evt]]` to untagged Event Task `- [ ] **1200-1230** Park [root:: abby] ^evt` | yes / `note` (love)               |
| OR13 | `[[nowhere#^a]]`, a missing id, or a same-note id carried by two lines                     | no                                |
| OR14 | target line `- Walk #off ^a` (no checkbox)                                                 | no                                |
| OR15 | other-note target whose text is not cached yet                                             | no; appears after the cache fills |
| OR16 | `- [x] #task Lunch ^lunch` in `love.md` (done; the link is struck)                         | yes / `note` (love)               |
| OR17 | Live Preview, same-note task edited to add `#off`, unsaved                                 | yes on the same frame             |
| OR18 | `#off` inside a code span or a Markdown link text on the target line                       | no                                |

**OT — tooltip.**

| #   | Case           | aria-label                     |
| --- | -------------- | ------------------------------ |
| OT1 | OR1            | `Off-clock · tagged #off`      |
| OT2 | OR2            | `Off-clock · tagged #OFFCLOCK` |
| OT3 | OR3            | `Off-clock · from love`        |
| OT4 | OR11           | `Off-clock · tagged #off`      |
| OT5 | any stray `::` | becomes `:`                    |

**OP — placement.**

| #   | Case                                           | Result                                                   |
| --- | ---------------------------------------------- | -------------------------------------------------------- |
| OP1 | Live Preview                                   | widget at the link end, `side: 3`; no line-height change |
| OP2 | the link also has 🔓 and 🔑                    | order: link, 🔓, 🔑 N, chip (both views)                 |
| OP3 | Reading view, no other mark                    | host directly after `a.internal-link` (or its `del`)     |
| OP4 | cursor on the row (raw `[[…]]` revealed)       | chip still at the link end                               |
| OP5 | a chipped row in Reading view                  | ◐ / 🔓 / 🔑 still paint; no status ring appears          |
| OP6 | Event Task chip and link chip, computed styles | same outline style, outline color, ink, font size, icon  |

**OV — parity with the writer.** For every case in the shared
`scripts/fixtures/pomodoro_clock/vectors.json` whose session clock comes from its
existing lineup (exclude MEET event-run cases, close cases, and cases whose token
adds links; the test lists each excluded id and why), `clockVerdict` over the chip
facts of that session's links equals `expected_off_clock`, and a link wears a chip
iff its fact is off-clock.

## Live verification (real-Obsidian e2e, bob-plugins `scripts/obsidian-off-clock-link-chips-e2e.cjs`)

The offline `--self-test` plus `scripts/test-ledger-tools-off-clock-link-chips.cjs`
cover the OC/OR/OT/OP/OV vectors, shared Event Task look, writer parity, and the
source-text freshness update.

Verified 88/88 in an isolated scratch vault (light and dark, Live Preview and
Reading) with screenshots reviewed against Anatomy and look. Dashed `timer-off`
`off` chip after off-clock ledger links, matching Event Task outline style,
outline color, ink, font size, and mask; 🔓 then 🔑 then the chip on the both-marks
row; Events and Notes prose links stay unchipped. CSS metrics from the landing
pass: link chip `font-size` 14.08px (`0.88em`), `outline-style: dashed`,
`outline-color: color(srgb 0.360784 0.360784 0.360784 / 0.55)`,
`color: rgb(92, 92, 92)`, `vertical-align: baseline`,
`margin-inline-start: 5.632px` (`0.4em`); row height 26.375px with the gate on
and off. No CSS values changed.

Environment: Obsidian 1.15.0 (`/snap/bin/obsidian`) on Athena, `xvfb-run -a`,
never the live vault. Tasks plugin `taskFormat: dataview` so `[id::]` /
`[dependsOn::]` parse.

- OC1–OC6, OC10, OR1–OR3, OR5, OR11, OR12, OP2, and OP5 render the chip in Live
  Preview; OC14, OR4, OR6, embeds, mixed prose, Events, and Notes stay bare
- Reading view paints the same chips and OP2 order (`link`, 🔓, 🔑, chip); OP3
  places the host after `a.internal-link`
- OP6 computed styles of the ledger chip match the Event Task chip on the same
  page (dashed outline, muted ink, timer-off mask)
- a CDP hover plus `Input.dispatchMouseEvent` shows the visible `.tooltip` text
  for OT1: `Off-clock · tagged #off`
- plain click writes nothing and places the caret at the exact link-end column
  (`ch=18` on `\t- [[love#^lunch]]`); Mod+click opens `love.md` in a tab
- setting `fun.md` to `offclock: true` adds its chip in 262 ms, and adding `#off`
  to a closed `bob.md` task adds its chip in 263 ms (1 s target, 500 ms
  documented tolerance); an unsaved same-note `#off` edit shows on the same
  frame in 167 ms
- the session toggle removes every chip, leaves row height unchanged, then
  restores the chips in Live Preview, Reading, and a popout; turning calendar
  event painting off leaves link chips visible

Reading-view status rings still appear on ledger rows, including the unchipped
parser link; Live Preview keeps those rows ring-free. Reading view also did not
expose a `bob-progress-mark` on the in-progress lunch row (Live Preview did).
Those gaps predate the chip and stay out of this contract's pass.

Still pending: iOS glyph rendering and a 100-link ledger scroll. Prose-link
chips stay deferred per this contract.

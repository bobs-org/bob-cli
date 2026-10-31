# Pomodoro homes

Pomodoro homes label work from the notes of the tasks linked under each ledger
entry. The label is derived when Bob reads the ledger; Bob never writes a
session name, home, or label back to a daily note. Open entries show a forecast.
Closed entries show the attribution Stats counts. The `parts` keep each task's
nearest home, while the label, chip, and dot name **root parents**
([R2](#r2-root-parent), [R7](#r7-labels-chips-and-dots)): a session that links
`bob` and its sub-project reads `bob`.

This is the shared contract for Rust, the Obsidian ledger mirror, and clients
that render Bob's JSON. Rust owns the meaning. Consumers may ignore new JSON
fields; this contract does not require a schema-version change.

## R1 — Task homes

Resolve a dedicated Task Link relative to its daily note using Bob's link rules:
an exact vault path with or without `.md`, then a unique case-insensitive
basename. A missing or ambiguous link is `@unfiled` and increments
`unresolved_links` or `ambiguous_links`.

The link target's note determines home; its block id only distinguishes tasks.
Walk scalar `parent:` wikilinks from the linked note toward its ancestors.
Aliases, `.md`, and headings are removed. List-form, missing, self, and
unresolvable parents end the walk. Stop after 16 notes or when a path repeats.
Archive (`done/*_done.md`, `type: [[done]]`) and reference (`type: [[ref]]`)
notes have no home of their own and are walked through.

The first `type: [[area]]` or `type: [[project]]` on the chain, starting at the
linked note, is the home. Closed and canceled projects remain valid homes. A
note without either type is `@unfiled`. A set `[root:: X]` field starts this
walk at X instead of the linked note ([R11](#r11-task-root-fields)).

An empty target resolves to the current daily note. A daily note walks
like any other note: without an area, project, or root field, a task in one
is `@unfiled`. The same residence chain feeds task type
suggestions: the first valid area/project `task_type:` default (or legacy
`theme:` default) on the chain is
the note source ([PT13](task-types.md#pt13-task-type-suggestions)). The top-level area is the outermost area on the chain.
A subarea is the nearest area below that top-level area. A home without an area uses
`@unfiled` for its area dimension.

## R2 — Root parent

A task's working time groups under its **root parent**: the area note the
task lives in, else the nearest project marked `root: true` on the walk up
to that area, else the top-level project above its project note. A
top-level project is a project whose `parent` walk reaches an area before
any other project (normally its `parent` is an area; a sub-area counts). A
project under another project is a sub-project at any depth. "Top" is never
folder depth, size, or status.

Take `chain` as built in R1 and `h` as the nearest-home index:

- If `chain[h]` is an area, `root = chain[h]`.
- If `chain[h]` is a project, let `a` be the first area index above `h`,
  or the chain's end. Then `root` is the first project in `chain[h..a]`
  with `root: true`, else the last project in `chain[h..a]`.
  Non-area, non-project notes in between are walked through, exactly as R1
  walks archive and ref notes.
- If the walk never reaches an area, the outermost project reached stands
  in as the root. It is not a top-level project, and its area stays
  `@unfiled`.
- Buckets are their own root. Only `@unlinked` and `@unfiled` remain as
  reserved buckets; every other root is a real note.

Root projects (`root: true`): a project note may set `root: true` in its
frontmatter to become its own root parent, grouping its own and its
sub-projects' working time under itself instead of the top-level project
above it. The flag is read with `frontmatter_value(fm, "root")` and counts
only when `trim_yaml_scalar` equals `true` ASCII-case-insensitively, so
`"true"` and `True` count while an absent, `false`, `yes`, empty, or list
value is silently off. The key is ignored on every non-project note.
Reporting only: it changes no blocking, Sub-projects lines, Ready caps,
review tiers, capture routing, plan budget, or `bob projects` output.

| Task lives in | Walk | Root parent |
| --- | --- | --- |
| `sase_goals.md` (project) | `sase_goals → sase → dev` | `sase` |
| `sase_blog.md` (project, `root: true`) | `sase_blog → sase → dev` | `sase_blog` |
| `sase_pre_blog.md` (project) | `sase_pre_blog → sase_blog → sase → dev` | `sase_blog` |
| `sase.md` / `bob.md` (project) | `→ dev` / `→ gtd` | itself |
| `dev.md` (area) | — | `dev` |
| `gtd_daily.md` (sub-area) | — | `gtd_daily` |
| `p.md` (project, parent `gtd_daily`) | `p → gtd_daily` | `p` |
| `done/sase_blog_done.md` (archive) | `→ sase_blog → sase → dev` | `sase_blog` |
| `ref/x.md` (parent `bob`) | `→ bob → dev` | `bob` |
| `ref/y.md` (parent `dev`) | `→ dev` | `dev` |
| orphan project `o.md` (no area above) | `o → …` | `o`, area `@unfiled` |
| unrooted task defined in a daily note | — | `@unfiled` |
| `^gtd` with `[root:: gtd_daily]` | `gtd_daily → gtd` | `gtd_daily` |
| inbox task with `[root:: sase]` | `sase → dev` | `sase` |
| `[root:: sase_goals]` (no `root: true`) | `sase_goals → sase → dev` | `sase`, plus a `root_field_not_root` lint |
| closed session with no Task Link | — | `@unlinked` |
| missing or ambiguous link | — | `@unfiled` |

Roots are derived from today's note tree at read time, never written. A
leftover `rollup:` frontmatter key is silently ignored: project rollup is
retired and never regroups homes. Labels use the bare root stem (`sase`,
`gtd`); buckets keep `unlinked` and `unfiled`. Pomodoro labels, chips, and
dots group by root parent as well ([R7](#r7-labels-chips-and-dots)), while
`homes.parts` stay per nearest home.
Hues follow R8:
bob reports each root's area hue (R8); bars give every root they show a distinct
colour (see stats.md). Stats splits each session per task first, then sums shares by root,
so every dimension still sums to the metric total; `apportioned` root
minutes come from sessions with two or more distinct roots. Count mode
assigns the session to the root with the most distinct tasks (ties to the
first task in ledger order), which is not always the lead project's root.
`in:` and `exclude_in:` keep matching the full residence chain, so
`in: [sase_blog]` still measures blog time on its own.

## R3 — Links that count

Count dedicated Task Links: direct child bullets at the Pomodoro's first child
indentation whose body is exactly one block link after removing 🍅 markers and
one enclosing `~~…~~` strike. Plain links, embeds, and struck links count.
Mixed prose, fenced code, deferred `[[note]]#` bodies, deeper descendants,
Depends-On lines, Work Log lines, and Schedule Log lines do not count. The
capture start picker keeps its existing rule and continues to omit struck
links.

De-duplicate resolved links by `(note path, block id)`. If resolution fails,
de-duplicate by the raw `(target, block id)` pair. Open and closed entries use
the same link rule. Open homes are forecasts and never count as worked time.

## R4 — Session names never choose a home

A session's name never chooses its home. Every linked task attributes through
its residence or its `[root:: X]` field, exactly as R1 resolves it. A `PLAN`
session linking `[[#^gtd]]` attributes to `gtd_daily` when that row carries
`[root:: gtd_daily]`. A `PLAN` session linking `[[sase_blog#^s0-spike]]`
attributes to `sase_blog`. An unlinked closed `PLAN` session is `@unlinked`.
"How much PLAN" remains the Stats type dimension, which already reads
`PLAN` time, while the homes read the linked tasks' notes.

## R5 — Splitting and lead home

For a closed session, divide its counted minutes evenly among distinct linked
tasks. Each task receives the floor of the split; the entire remainder goes to
the first task in ledger order (8m over 3 tasks is 4/2/2; 5m is 3/1/1). Sum
those shares by home. In an open session, parts instead use the number of
distinct task links. A closed entry without a recorded or range duration is
still attributed: its parts carry 0 minutes and task-based shares.

Count-mode Stats assigns the whole session to its lead home: the home with the
most distinct tasks, with ties decided by the first task in ledger order.
Shares are whole percentages of the session and sum to 100 when there is a
non-empty attribution. The percentage rounding gives remaining points to the
largest fractional remainders, with ties in ledger order. No composite home
names are generated. Each dimension's parts sum to its metric total.

An `apportioned` amount is the minutes in a part that came from sessions with
two or more distinct homes in that dimension. A session can be apportioned in
`project` but not `area` when multiple projects share one area.

## R6 — Unattributed buckets

`@unlinked` is a closed session with no dedicated Task Links.
`@unfiled` is a linked task whose note has no area or project, or whose link is
ambiguous or unresolved. Both buckets remain visible outside any `other` group.
They are never guessed from the session name. Their repair hints are “link the
task to attribute it” and “file the task in an area or project, or give it a
root field”. An open entry
with no links has no parts and an empty label.

## R7 — Labels, chips, and dots

The label, chip, and dot name **root parents** ([R2](#r2-root-parent)), not
nearest homes. `homes.parts`, their shares and minutes, the lead home, and every
Stats dimension keep the nearest-home granularity Stats needs. No JSON field is
added, removed, or retyped. Derive the display fields in this order:

1. **Group parts by root parent.** A part's root is its home's root parent.
   Bucket parts are their own root (`@unfiled`, `@unlinked`). A root's label is
   the lowercase note stem of its path, or `unfiled` / `unlinked` for a bucket.
2. **Root share** is the sum of its parts' whole-percent shares. **Root tasks**
   is the sum of their task counts. **Root order** is the ledger order of its
   first part.
3. **Label.** Sort roots by share, descending, with ties kept in root order.
   Show up to two root labels joined by ` · `, then append ` +N` for the rest. A
   session with no homes has an empty label.
4. **Chip.** The chip is the label unless it is suppressed for an unlinked
   session, or when the session name's case-insensitive `+` component set equals
   the set of **root** labels. An unfiled-only chip says `unfiled`. A `— SASE`
   session that links only `sase_blog_notes` tasks therefore has no chip.
5. **Dot.** Use a filled `●` for an attributed session and a hollow `○` for a
   forecast. The dot takes the **lead root's** area hue. The lead root has the
   most distinct tasks, with ties going to the first task in ledger order (the
   rule Stats count mode uses for roots). A root's hue equals each of its parts'
   hue, because the root lies on the same chain below the same top-level area,
   so take it from the root's first part. `lead_home` is unchanged.
6. **Tooltip.** The Obsidian ledger chip adds one tooltip line per root, in
   label order: `<root> · N task(s)`, plus ` · Mm` when attributed.

A session that links `bob#^…` tasks and `bob_command_mode_improvements#^prj`
(a sub-project of `bob`) reads `○ bob`; its `parts` still list both homes. A
chip never routes a capture, changes status, reorders a session, or feeds the
plan budget. Homes are not capture `#name` targets; separate sessions remain
separate.

## R8 — Area hues

The palette uses these fixed categorical slots. An area's own `color:` selects
one slot only when the area is top-level. Invalid top-level values produce one
`area_color_invalid` lint and use the local FNV-1a fallback. A subarea and
project inherit the top-level area's hue. `color:` elsewhere is
ignored. `bob stats` reports `area_hue_collision` when top-level areas share a
slot and suggests setting `color:` on one of them.

| Slot | Hue | Light | Dark | xterm-256 |
| ---: | --- | --- | --- | ---: |
| 1 | `blue` | `#2a78d6` | `#3987e5` | 33 |
| 2 | `orange` | `#eb6834` | `#d95926` | 208 |
| 3 | `aqua` | `#1baf7a` | `#199e70` | 36 |
| 4 | `yellow` | `#eda100` | `#c98500` | 214 |
| 5 | `magenta` | `#e87ba4` | `#d55181` | 211 |
| 6 | `green` | `#008300` | `#008300` | 28 |
| 7 | `violet` | `#6250d6` | `#9085e9` | 62 |
| 8 | `red` | `#e34948` | `#e66767` | 167 |
| — | `gray` (buckets) | `text-muted` | `text-muted` | dim |

Without an explicit slot, assign the hue by
`palette[fnv1a32(lowercase_area_stem) % 8]`. The algorithm is local and
deterministic; consumers do not need a global area catalog to resolve a hue.
Stats bars order area segments by slot so adjacent colors keep the validated
palette ordering.

Bryan's current top-level area assignments are `dev: blue`, `gtd: orange`,
`job: aqua`, `love: yellow`, `cash: magenta`, `body: green`, and `meet: violet`.
These are vault configuration, not application constants.

## R9 — JSON object

Every emitting surface uses this additive shape:

```json
{
  "homes": {
    "state": "attributed",
    "kind": "work",
    "label": "bob · dev +1",
    "chip": "bob · dev +1",
    "hue": "orange",
    "parts": [
      {
        "key": "bob.md",
        "label": "bob",
        "kind": "project",
        "area": "gtd.md",
        "area_label": "gtd",
        "hue": "orange",
        "tasks": 1,
        "share": 34,
        "minutes": 17
      }
    ]
  }
}
```

`state` is `attributed` for every closed entry and `forecast` for every open
entry; `kind` is `work` or
`unlinked`. `label`, `chip`, and `hue` name root parents ([R7](#r7-labels-chips-and-dots))
while `parts` stay per nearest home. `chip` and `hue` are nullable. Each part's kind is `project`,
`area`, or `bucket`. `share` is a whole percentage: minute-based for attributed
sessions with a duration, and task-based when a closed entry has no duration or
for forecasts. `minutes` is present on every attributed part; it is `0` when a
closed entry has no duration. An unlinked closed entry has one `@unlinked` part;
an unlinked forecast has an empty `parts` array.

## R10 — Current note tree

Resolve homes and root parents from the current vault tree each time.
Moving a task or changing a project parent can regroup past minutes by root,
but cannot change the total time. Stats surfaces say “Grouped by today's
note tree · from Task Links”. V1 does not store snapshots.

## R11 — Task root fields

A task may name its root parent with `[root:: NOTE]` or `(root:: NOTE)`.
Recognition is the shared `task_fields::inline_fields(line, "root")` scanner:
the key is case-sensitive and any spacing around `::` is allowed. The
canonical written form is `[root:: gtd_daily]`: one space after `::` and a
bare note name.

**Value.** A note name or a vault-relative path, with or without `.md`. It
resolves relative to the task's own note using Bob's link rules
(`HomeSource::resolve`: an exact vault path, then a unique case-insensitive
basename).

**States**, mirroring the PT13 theme field:

- _none_: no field.
- _set_: exactly one field whose value resolves to one note X.
- _repair_: the field is ignored and the task attributes from its own note
  exactly as if it had no field. Bob reports the repair. Reasons:
  - `duplicate`: two or more root fields.
  - `invalid`: blank, or contains any of `[ ] | # ^`. A wikilink value is
    invalid, and the hint says to write the bare name.
  - `missing`: no note matches.
  - `ambiguous`: two or more basename matches.
  - `no_home`: X's walk reaches no area or project.
- Closed tasks keep their field, and it keeps applying, so history stays
  attributed.
- Root fields never chain. Only the linked task's own field counts; fields
  on tasks inside X are irrelevant. A field naming the task's own note is a
  silent no-op.

**Meaning: as-if residence.** With a set field naming X, Bob resolves the
task's homes exactly as if the checkbox lived in X:

- The R1 chain starts at X: X, then X's scalar `parent:` walk, with the same
  16-note and cycle limits and the same archive/ref walk-through.
- R2 picks the root from that chain.
- The task's own note contributes nothing.

So an area at any depth, a `root: true` project, or a top-level project names
itself as the root parent. When the chain from X yields a root R ≠ X (for
example X is a sub-project without `root: true`, or a ref note), the time
still follows X's chain and groups under R. Bob reports `root_field_not_root`
naming R, with the fix: "sase_goals groups under sase; write [root:: sase] or
set root: true on sase_goals". This keeps the set of root parents a function
of note frontmatter: `root: true` stays the one way to create a root, and a
per-task field can never invent a stray root.

A set root field applies before residence is read: R1 starts at X, so the
old `^gtd` and daily-note special cases are gone. Session names never choose
a home (R4).

**Scope: reporting only, like `root: true`.** The field applies to:

- Pomodoro homes on every homes surface: `bob plan`, capture outputs,
  `capture-pomodoros --tasks`, the Obsidian ledger chips, and `bob stats`
  sessions, parts, and catalog.
- Stats task metrics (done/created/dropped project, area, subarea, and
  `root_parent` parts).
- Stats `in:` / `exclude_in:` matching (against X's chain, not the task's
  own note).
- The PT13 note-default theme walk, which starts at X.

The Stats `note` part stays the file that holds the checkbox. The field never
changes residence, blocking (`#prj_is_blocked`), Ready caps, `task_refresh`
freshness intervals, review tiers and the `]s` walk, capture routing, task
move, archive, `## Tasks` grouping, `bob projects`, or the plan budget.

**Placement and preservation.** The canonical position is the left edge of
the trailing metadata run, the same position `set_task_theme` uses. A field
authored anywhere on the line is still recognized. Root is a
description-region field: it is never part of the Obsidian Tasks suffix,
freshness stamps land after it, and it is not a run-extending freshness key.
Task move and archive, freshness stamping, keeps, and unblock, type
set/clear, reconcile, complete, and cancel, and ref rename's link rewrite
keep the authored bytes identical.

**Lints.** `bob stats` reports two additive health lists, once per
`(path, line)`, for root fields it reads while attributing in-window sessions
and task rows. The human output has matching `warning <message>  <code>`
lines.

- `health.homes.root_field_invalid: [{path, line, raw, reason}]`, reason from
  the repair list above.
- `health.homes.root_field_not_root: [{path, line, raw, root}]`.

## Shared conformance vectors

[`tests/fixtures/pomodoro_homes/vectors.json`](https://github.com/bobs-org/bob-cli/blob/master/tests/fixtures/pomodoro_homes/vectors.json)
contains a raw mini-vault and hand-calculated ledger expectations. The Rust
tests run every case against both indexed and disk sources. The shared cases
are also the JavaScript mirror's contract; only ambiguous basename resolution
is marked `rust_only` because Obsidian's resolver owns that behavior.

| Case | What it proves |
| --- | --- |
| `subproject-archive-homes` | archive/ref chain, struck links, de-duplication; sub-projects label as their root parent |
| `resolution-and-lints` | archived/ref/project/area homes, cycles, explicit and fallback hues |
| `ambiguous-basename` | duplicate basename resolves to `@unfiled`; the sole `rust_only` case |
| `chips, forecast shares, empty forecasts, and session identity` | chip suppression by the root-label set, forecast and unlinked shapes, ties, named sessions |
| `split-rounding-and-duration` | 50m across 3 tasks is 18/16/16; `[t:: 40m]` takes precedence over a 50m range |
| `share-rounding` | equal task weights round to 34/33/33, giving the tied remainder to ledger order rather than the last part |
| `gtd-root-across-days` | another day's rooted `^gtd` attributes to `gtd_daily`; an unrooted one is `@unfiled` |
| `depth-and-parent-shapes` | direct child and grandchild homes, list/self parents stop, the depth cap is 16, and unquoted scalar `parent: [[x]]` resolves |
| `parent-scalar-shapes` | quoted scalar wikilinks resolve; single-item YAML lists and bare strings do not |
| `unresolved-and-closed-zero` | unresolved links are `@unfiled`; closed `- [x] ()` stays attributed with zero-minute parts |
| `session-names-never-home` | named `PLAN` linking a rooted `^gtd`, linking a project, an unlinked closed `PLAN`, and `PLAN + BOB` |
| `named-sessions-use-task-homes` | named sessions attribute through their task links; unrooted `^gtd` links are `@unfiled` |
| `daily-note-task-home` | an unrooted daily-note task is `@unfiled` |
| `root-fields` | set `[root::]` re-homes `^gtd` and inbox tasks, paren form, `sase_goals` rolls up with a lint, and missing/duplicate/invalid/no_home fall back; a closed task keeps attributing |
| `root-parent-chips` | a project and its sub-project share one root label; a `root: true` project (and a quoted `"TRUE"`) is its own root; roots share-rank and tie-break by ledger order; the dot takes the lead root's hue while `lead_home` stays per home |

## Limits and rejected alternatives

- Parent walks stop after 16 notes and at missing, invalid, self, or repeated
  parent links.
- Homes are based on note residence and dedicated Pomodoro Task Links. Work
  notes, prose, and schedule/dependency logs do not attribute time.
- Session names are never rewritten from derived homes; history is not frozen
  at close; there is no composite `Mixed` bucket; names are never used to guess
  a home.

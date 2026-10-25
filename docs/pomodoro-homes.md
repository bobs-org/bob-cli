# Pomodoro homes

Pomodoro homes label work from the notes of the tasks linked under each ledger
entry. The label is derived when Bob reads the ledger; Bob never writes a
session name, home, or label back to a daily note. Open entries show a forecast.
Closed entries show the attribution Stats counts.

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
note without either type is `@unfiled`.

A link to a canonical daily note's `^gtd` block (case-insensitive) is the
`@review:<first exempt name>` home. An empty target resolves to the current
daily note. The top-level area is the outermost area on the chain. A subarea is
the nearest area below that top-level area. A home without an area uses
`@unfiled` for its area dimension.

## R2 — Root parent

A task's working time groups under its **root parent**: the area note the
task lives in, else the top-level project above its project note. A
top-level project is a project whose `parent` walk reaches an area before
any other project (normally its `parent` is an area; a sub-area counts). A
project under another project is a sub-project at any depth. "Top" is never
folder depth, size, or status.

Take `chain` as built in R1 and `h` as the nearest-home index:

- If `chain[h]` is an area, `root = chain[h]`.
- If `chain[h]` is a project, let `a` be the first area index above `h`,
  or the chain's end. Then `root` is the last project in `chain[h..a]`.
  Non-area, non-project notes in between are walked through, exactly as R1
  walks archive and ref notes.
- If the walk never reaches an area, the outermost project reached stands
  in as the root. It is not a top-level project, and its area stays
  `@unfiled`.
- A bucket is its own root: `@review:<NAME>`, `@unlinked`, and `@unfiled`.

| Task lives in | Walk | Root parent |
| --- | --- | --- |
| `sase_blog.md` (project) | `sase_blog → sase → dev` | `sase` |
| `sase.md` / `bob.md` (project) | `→ dev` / `→ gtd` | itself |
| `dev.md` (area) | — | `dev` |
| `gtd_daily.md` (sub-area) | — | `gtd_daily` |
| `p.md` (project, parent `gtd_daily`) | `p → gtd_daily` | `p` |
| `done/sase_blog_done.md` (archive) | `→ sase_blog → sase → dev` | `sase` |
| `ref/x.md` (parent `bob`) | `→ bob → dev` | `bob` |
| `ref/y.md` (parent `dev`) | `→ dev` | `dev` |
| orphan project `o.md` (no area above) | `o → …` | `o`, area `@unfiled` |
| GTD review session / `[[#^gtd]]` bridge | — | `@review:GTD` |
| closed session with no Task Link | — | `@unlinked` |
| missing or ambiguous link | — | `@unfiled` |

Roots are derived from today's note tree at read time, never written. A
leftover `rollup:` frontmatter key is silently ignored: project rollup is
retired and never regroups homes. Labels use the bare root stem (`sase`,
`gtd`); buckets keep `GTD review`, `unlinked`, `unfiled`. Hues follow R8:
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

## R4 — Review sessions

A named session is a review session when every `+`-separated name component is
in `plan.exempt` (default `[GTD]`). Its minutes go to one `@review:<NAME>` part
per exempt component; composite names split their minutes evenly, with any
remainder going to the first component. The bucket label is `<NAME> review`.
Its task links do not attribute.

An unnamed session is also a review session when one of its dedicated links is
that day's `^gtd` row. This historical bridge uses the first configured exempt
name and is disabled when `plan.exempt` is empty. The link still maps to that
review home in a work session. A named composite such as `GTD + BOB` is a work
session unless every component is exempt.

The review bucket's area is the top-level area of the note resolved from the
lowercase review name (`GTD` → `gtd.md`). If that note has no area or project,
the bucket itself is the area key. Review parts inherit that area's hue.

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

`@unlinked` is a closed, non-review session with no dedicated Task Links.
`@unfiled` is a linked task whose note has no area or project, or whose link is
ambiguous or unresolved. Both buckets remain visible outside any `other` group.
They are never guessed from the session name. Their repair hints are “link the
task to attribute it” and “file the task in an area or project”. An open entry
with no links has no parts and an empty label.

## R7 — Labels, chips, and dots

Use a lowercase note stem for project and area labels. Use `GTD review`,
`unlinked`, and `unfiled` for reserved buckets. Sort labels by share, descending;
show up to two joined by ` · `, then append ` +N` for additional homes. A
session with no homes has an empty label.

The chip is the label unless suppressed for an unlinked session, a named review
session, or a name whose case-insensitive component set equals the set of home
labels. An unnamed review bridge chip says `GTD review`. An unfiled-only chip
says `unfiled`.

Use a filled `●` for an attributed session and a hollow `○` for a forecast.
The dot takes the lead home's area hue. A chip never routes a capture, changes
status, reorders a session, or feeds the plan budget. Homes are not capture
`#name` targets; separate sessions remain separate.

## R8 — Area hues

The palette uses these fixed categorical slots. An area's own `color:` selects
one slot only when the area is top-level. Invalid top-level values produce one
`area_color_invalid` lint and use the local FNV-1a fallback. A subarea, project,
and review bucket inherit the top-level area's hue. `color:` elsewhere is
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
`job: aqua`, `love: yellow`, `cash: magenta`, and `body: green`. These are vault
configuration, not application constants.

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
entry; `kind` is `work`, `review`, or
`unlinked`. `chip` and `hue` are nullable. Each part's kind is `project`,
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

## Shared conformance vectors

[`tests/fixtures/pomodoro_homes/vectors.json`](https://github.com/bobs-org/bob-cli/blob/master/tests/fixtures/pomodoro_homes/vectors.json)
contains a raw mini-vault and hand-calculated ledger expectations. The Rust
tests run every case against both indexed and disk sources. The shared cases
are also the JavaScript mirror's contract; only ambiguous basename resolution
is marked `rust_only` because Obsidian's resolver owns that behavior.

| Case | What it proves |
| --- | --- |
| `subproject-archive-review` | archive/ref chain, struck links, de-duplication, review sessions and same-day bridge |
| `resolution-and-lints` | archived/ref/project/area homes, cycles, explicit and fallback hues |
| `ambiguous-basename` | duplicate basename resolves to `@unfiled`; the sole `rust_only` case |
| `chips, forecast shares, empty forecasts, and session identity` | chip suppression, forecast and unlinked shapes, ties, named sessions |
| `split-rounding-and-duration` | 50m across 3 tasks is 18/16/16; `[t:: 40m]` takes precedence over a 50m range |
| `share-rounding` | equal task weights round to 34/33/33, giving the tied remainder to ledger order rather than the last part |
| `daily-review-link` | another day's canonical `^gtd` is a review-home task share; only a same-day link triggers the unnamed review bridge |
| `depth-and-parent-shapes` | direct child and grandchild homes, list/self parents stop, the depth cap is 16, and unquoted scalar `parent: [[x]]` resolves |
| `parent-scalar-shapes` | quoted scalar wikilinks resolve; single-item YAML lists and bare strings do not |
| `unresolved-and-closed-zero` | unresolved links are `@unfiled`; closed `- [x] ()` stays attributed with zero-minute parts |
| `review-decisions` | named review sessions, same-day bridge, and mixed named work |

## Limits and rejected alternatives

- Parent walks stop after 16 notes and at missing, invalid, self, or repeated
  parent links.
- Homes are based on note residence and dedicated Pomodoro Task Links. Work
  notes, prose, and schedule/dependency logs do not attribute time.
- Session names are never rewritten from derived homes; history is not frozen
  at close; there is no composite `Mixed` bucket; names are never used to guess
  a home.

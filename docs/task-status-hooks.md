# Task Status Hooks

`bob task-status-hooks` makes the current Pomodoro ledger the source of truth
for active Obsidian Tasks promotions and structural cleanup. It also uses the
latest existing earlier daily note as a read-only recent-activity source for
deciding whether In Progress tasks in area and project notes remain active. It
keeps references to completed tasks retired as struck, non-embedded links
beneath their Pomodoros. Live non-transcluded links beneath completed Pomodoros carry
the machine-owned Pomodoro marker (`🍅`); embedded and provenance-unknown
retired links do not. Links beneath open Pomodoros are unmarked, and a link to
an unambiguously canceled Tasks task removes its complete Markdown list-item
subtree from an open Pomodoro without changing the canceled task itself. It
also follows transcluded
dependency bullets recursively, promoting each target to the strongest
applicable Next (`[*]`) or In Progress (`[/]`) status. It independently
reconciles the derived Blocked (`[?]`) marker from Tasks `[id:: ...]` and
`[dependsOn:: ...]` metadata.

## Usage

```bash
bob task-status-hooks [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json]
```

The vault root comes from `--bob-dir`, then `BOB_DIR`, then `~/bob`. The current
daily ledger comes from `BOB_DAY_FILE` when set; otherwise it is
`<vault>/YYYY/YYYYMMDD.md` for the local date or `BOB_NOW` override. When the
selected filename has a valid daily date, that date anchors the historical
lookup even for a fixture or manual override. Otherwise the effective current
date is the anchor.

Starting strictly before the anchor, the command selects the newest existing
canonical `<vault>/YYYY/YYYYMMDD.md` file. Missing intervening days or weeks
and year boundaries need no special handling. Malformed names, mismatched year
directories, the anchor date, and future dates are ignored. No earlier daily
note is a valid result.

Use `--dry-run` to compute and print the complete sync without writing files.
Repeated successful runs are idempotent.

`task-status-hooks` is the canonical and documented command name. The hidden
`task-status-setter` and `mark-next-tasks` spellings remain compatibility-only
dispatch aliases and show canonical usage when asked for help.

## Sync Rules

Within the current daily note's `## Pomodoros` section, the command reads block
links from indented bullets beneath open top-level Pomodoro entries:

```markdown
- [ ] Write and review (0900-0930)
  - [[dev#^write-design]]
  - Review [[Projects/Alpha#^parser-review]]
- [x] Earlier session (0830-0900)
  - 🍅 [[dev#^closed-session-link-is-not-a-next-source]]
```

Only links with a block fragment (`[[note#^block-id]]`) count. Ordinary note
links and heading links do not. Targets resolve by an exact vault-relative
path first and then by a unique, case-insensitive note basename. Ambiguous or
missing targets produce warnings and are not guessed.

Before task statuses, completed-reference retirement, or relocation are
planned, the command removes cross-Pomodoro duplicates. For each link that
resolves to a scanned Tasks task, identity is the resolved vault-relative
Markdown path plus block ID. Explicit paths, unique basenames, same-note
targets, aliases, and embeds therefore compare equal when they name the same
task; the same block ID in different notes remains distinct.

Ownership follows file order. The first surviving line beneath an open
Pomodoro owns each resolved task on that line. A line beneath a later open
Pomodoro is removed when any task on it is already owned by a different open
Pomodoro. Repeats within the same owning Pomodoro do not cause removal. Every
later conflicting line is removed in one run, and a line selected for removal
does not become the owner of its other links.

Removal is deliberately physical-line based: authored text and unrelated links
on a conflicting line are removed with it, while following nested lines remain
unless independently selected. If several duplicate tasks identify one line,
the line is removed and reported once with every canonical task identity. Links
beneath completed or cancelled Pomodoros, top-level entry lines, fenced
examples, and unresolved or non-task targets are not destructive cleanup
candidates.

### Canceled Task References

After duplicate-line ownership is decided, every surviving block-link
occurrence beneath an open Pomodoro is checked against the Tasks status
registry. When an occurrence resolves only to tasks with a recognized
`CANCELLED` status, its complete Markdown list item is removed: deletion starts
on the containing bullet's line and continues through the end of its nested
list-item subtree. This includes conventional `[-]` tasks and any configured
single-character symbol whose `statusSettings.coreStatuses` or
`statusSettings.customStatuses` entry has type `CANCELLED`. The task itself
remains canceled.

Plain, embedded, aliased, Pomodoro-marked, and exactly struck forms all
qualify. The list item is the mutation unit, so authored prose, live or
completed sibling links, unresolved links, embeds, aliases, markers,
strikethrough, and nested content on that item are removed with it. A canceled
link in a nested bullet removes that nested item without removing its parent;
a canceled link on the parent removes the parent and all descendants. Multiple
qualifying occurrences on one bullet still produce separate compatibility
records in occurrence order even though the subtree is deleted once. Covered
descendant bullets do not produce redundant cancellation, token-edit, move, or
marker reports.

Cleanup is limited to indented bullets owned by open Pomodoros in the selected
daily note. Links beneath completed or canceled Pomodoros, on top-level
Pomodoro lines, outside `## Pomodoros`, or inside fenced examples remain
untouched by this rule. Unresolved links and links that do not resolve to a
scanned Tasks task also remain and keep their warning behavior.

If one block ID matches several Tasks task lines, every match must have a
recognized `CANCELLED` type before the containing item qualifies. All-canceled
duplicates qualify; canceled/open, canceled/done, and canceled/unknown mixes
remain in place and produce an ambiguity warning. A physical line already
selected by cross-Pomodoro duplicate cleanup is deleted once and does not also
produce a canceled-reference report. Duplicate cleanup remains physical-line
only; canceled-reference cleanup uses the full list-item subtree. When a
completed parent bullet is relocated, any independently canceled descendant
subtree is omitted from the moved content.

The rewritten Pomodoro section is scanned again after duplicate removal and
canceled/completed-reference structural rewrites. Only references that will
actually be written contribute to the direct desired-status map and dependency
graph. Thus an otherwise-live task mentioned only as sibling or nested content
in a removed item, a removed canceled root, and any otherwise-unreachable
dependency chain stop contributing desired Next or In-Progress state in that
same run.

After resolving the surviving direct Pomodoro links, the command reads
dependency edges from the linked tasks' child blocks. An edge must be a child
bullet whose entire content is one transcluded block link:

```markdown
- [ ] #task Ship the feature ^ship
  - ![[#^write-tests]]
  - ![[Quality/Review#^review]]
- [ ] #task Write tests ^write-tests
```

Same-note and cross-note targets use the same resolver as Pomodoro links. The
target block must belong to a scanned Tasks task. Plain `[[#^id]]` links,
aliases, mixed-content bullets, fenced examples, non-task `#^ref` blocks, and
unresolvable targets are not dependency edges. Unresolvable candidates emit a
warning naming the referencing task's file and line.

Block fragments are file-scoped: `Alpha.md#^review` and `Beta.md#^review` are
distinct graph nodes, and an explicit transclusion path selects only its named
note. The active-rank graph deliberately traverses these resolved
path-plus-fragment links. Tasks metadata separately determines whether the
displayed status is Blocked, using vault-wide IDs such as `Alpha__review`.

Each surviving direct Pomodoro task has a minimum desired status of Next. A
direct task already In Progress instead seeds In Progress. Every dependency
inherits its source task's effective ranked status, using the order
`[ ] < [*] < [/]`. The effective status is the stronger of the incoming request
and the target's current supported status, so an already-In-Progress
intermediate task promotes lower-status descendants to In Progress. Multiple
parents merge by taking the strongest request.

Traversal uses a monotonic work queue and is cycle-safe: a task is revisited
only when its effective rank increases. Dependencies of dependencies are
included, while a task reached both directly and through a dependency is
counted as direct. Removing a Pomodoro link removes that task's otherwise
unreachable dependency chain from the desired map on the next run.

### Rolling Recent Activity

Status promotion still uses only the structurally normalized current ledger.
Separately, the command builds a recent-activity identity set from every
non-retired block link beneath a recognized Pomodoro in the normalized current
ledger and the selected previous daily note. A struck link is retired and does
not count; aliases, embeds, markers, alternate note spellings, and same-note
links normalize through the usual resolved vault-relative path plus block ID.
Each daily retains its own resolution context, so `[[#^local]]` resolves
against the daily where it appears even when both notes contain identical raw
link text.

The previous daily note is read-only. It never receives duplicate cleanup,
link retirement, marker repair, canceled-reference removal, Pomodoro
relocation, or task-status writes. If it has no `## Pomodoros` section, it is
the selected source with zero references; the command does not fail or fall
through to an older note. Non-retired historical links may preserve an
already-In-Progress task, but they never promote a Ready task to Next or In
Progress.

Recent roots traverse the same eligible transcluded-task dependency graph as
current promotion roots. This protects an area/project In-Progress dependency
reachable from either daily and keeps cycles stable across repeated runs.

After this traversal, a recognized `[/]` Tasks task is reset to `[ ]` only
when all of the following are true:

- its note has frontmatter `type: [[area]]` or `type: [[project]]`, including
  the shared parser's single- and double-quoted scalar forms;
- the note is not a canonical daily note or the selected current ledger;
- the task's canonical path-plus-block identity is absent from recent
  activity, or the task has no usable trailing block ID; and
- it has no open Dataview dependency that requires Blocked status.

Directory names and tags do not establish area/project scope. Ordinary notes,
daily notes, generated references, terminal statuses, unknown/custom
statuses, and other checkbox states are not subject to this rollback.

## Dependency Blocked Status

After the full vault scan and the final post-rewrite Pomodoro graph are known,
the command indexes Dataview task identities from both square-bracket and
parenthesized fields:

```markdown
- [ ] #task Parent [dependsOn:: Tasks__child]
- [ ] #task Child [id:: Tasks__child]
- [ ] #task Equivalent parenthesized metadata (dependsOn:: Tasks__child)
```

A recognized non-terminal parent is blocked when any `dependsOn` value matches
at least one open task with the same vault-wide `id`. `TODO`, `IN_PROGRESS`,
and `ON_HOLD` targets are open. `DONE`, `CANCELLED`, and `NON_TASK` targets do
not block, and neither do unrecognized target statuses. Missing IDs are
ignored; if an ID is duplicated, any recognized open instance is sufficient.
Self-dependencies, chains, and cycles therefore remain blocked under the same
direct Tasks 8 semantics. Only direct metadata decides a parent's marker;
transitive blocking follows because Blocked is itself an open `ON_HOLD`
status.

Blocked is derived state. It overrides Ready (`[ ]`), Next (`[*]`), and In
Progress (`[/]`) while an open dependency exists. Once all matching targets
are terminal or missing, a Blocked task returns to the final active status
computed by the Pomodoro graph (`[*]` or `[/]`), or Ready when unreachable. No
hidden previous-status field is stored. A Blocked task with no `dependsOn`
metadata is likewise recovered to its final Pomodoro rank or Ready. Terminal
parents and unknown/custom parent statuses remain untouched even if they retain
dependency metadata.

Ctrl+Enter recovery in the Task Status Cycler plugin is intentionally narrower
and immediate. After that keypress actually changes one or more tasks to Done,
the plugin reopens only Blocked dependents that directly name one of those
tasks and have no other recognized open dependency in the post-close vault
snapshot. The immediate target is always Ready (`[ ]`): the plugin does not
guess the final Pomodoro rank. It reads unsaved open Markdown buffers, preserves
the active cursor, skips stale or failed notes without rolling back completed
tasks, and serializes recovery with closed-reference retirement. A later
`bob task-status-hooks` run remains authoritative across the whole vault and
may promote the recovered task to Next or In Progress. Closing a dependency
never reopens a Done, canceled, non-task, unknown, unrelated, or already-active
dependent, and Ctrl+Enter does not clean unrelated Blocked tasks with no
dependencies.

The installed Tasks registry must contain exactly one compatible status:

```json
{
  "symbol": "?",
  "name": "Blocked",
  "nextStatusSymbol": " ",
  "availableAsCommand": true,
  "type": "ON_HOLD"
}
```

If a planned Blocked or unblocked write needs this contract and the definition
is missing, duplicated by symbol/name, or incompatible, the command exits with
an actionable error before any note write.

The command scans Markdown task lines allowed by the Obsidian Tasks
`globalFilter` setting. If that setting cannot be read, the filter defaults to
`#task`. The combined transition precedence is:

| Existing status | Desired/reachability state | Result |
| --- | --- | --- |
| done, canceled, or non-task | open task dependency | unchanged |
| `[ ]`, `[*]`, or `[/]` | open task dependency | `[?]` |
| `[?]` | open task dependency | unchanged |
| `[?]` | no open dependency; desired In Progress | `[/]` |
| `[?]` | no open dependency; desired Next | `[*]` |
| `[?]` | no open dependency; unreachable | `[ ]` |
| `[ ]` | desired Next | `[*]` |
| `[ ]` or `[*]` | desired In Progress | `[/]` |
| `[*]` | desired Next | unchanged |
| `[/]` | desired Next or In Progress | unchanged |
| `[*]` | unreachable | `[ ]` |
| `[/]` in an area/project note | no rolling recent activity | `[ ]` |
| `[/]` in an area/project note | rolling recent activity from either daily or an eligible dependency path | unchanged |
| `[ ]` or out-of-scope `[/]` | unreachable | unchanged |
| done, canceled, non-task, or unknown/custom | any | unchanged |

Ranked propagation itself is monotonic and never lowers a dependency target.
Removing a transclusion therefore does not perform a matching rollback. The
separate vault-wide cleanup rule still resets any Next task that is no longer
reachable from the final open-Pomodoro graph. The distinct scoped rollback
resets stale In Progress only under the rolling-activity rules above and never
resets terminal/custom statuses. Open Dataview dependency derivation takes
precedence over both cleanup rules, producing one stable final state.

The machine-managed `#task #ref ... ^ref` reading task in a generated reference
note is an ordinary scanned task. Promoting it to `[*]` or `[/]` therefore
flows through the next highlights sync as the corresponding reference status;
clearing an unreachable `[*]` back to `[ ]` flows through as `status: ready`.
Existing `[/]`, `[x]`/`[X]`, and `[-]` statuses in generated reference notes
are never lowered because those notes are outside the area/project rollback
scope. Because the highlights lifecycle is also
stored in the PDF marker, preview with `bob highlights scan --dry-run` and use a
reviewed `bob highlights scan --write-pdfs` when marker write-back is needed.

Completion is classified separately from Next synchronization. Conventional
`[x]` and `[X]` tasks are complete. A custom checkbox symbol is also complete
when its entry in `statusSettings.coreStatuses` or
`statusSettings.customStatuses` has type `DONE`. `CANCELLED`, `IN_PROGRESS`,
`ON_HOLD`, `NON_TASK`, unknown, and unchecked statuses are not complete.

For each bullet beneath an open or completed Pomodoro that contains a block
link resolving unambiguously to a completed Tasks task, the command retires
that link as `~~[[...]]~~`. Aliases and neighboring text are preserved, so
`[[dev#^done|result]]`, `![[dev#^done|result]]`, and
`~~![[dev#^done|result]]~~` all become
`~~[[dev#^done|result]]~~`. On mixed-content bullets, only links proven
complete are changed. Canonical struck links are unchanged.

## Pomodoro Marker

The marker records that a non-transcluded link participated in a completed
Pomodoro. It belongs to the individual link, not the bullet, and is normalized
from each occurrence's syntax before retirement:

| Link state | Canonical grammar |
| --- | --- |
| completed, live non-transcluded | `🍅 [[dev#^write-tests]]` |
| completed, embedded | `![[dev#^reference]]` |
| retired with recorded participation | `🍅 ~~[[dev#^write-tests|alias]]~~` |
| retired with unknown/embed provenance | `~~[[dev#^reference]]~~` |
| mixed content | `Work on 🍅 [[a#^x]] and ~~[[b#^y]]~~` |

The sync adds or canonicalizes markers on live non-transcluded links beneath
completed Pomodoros and removes markers from embedded links. For an already
struck non-embedded link, it preserves whether a marker exists: an existing
marker is canonicalized, but a missing marker is not backfilled because the
strike may be the only surviving evidence that the link was retired from a
transclusion. Open Pomodoros are unmarked. Cancelled (`[-]`) Pomodoros, fenced
code, the top-level Pomodoro line, and links outside `## Pomodoros` are
untouched. Marker repair is decoration-only and never changes Next/dependency
selection.

The containing bullet is relocated according to this order:

1. The single open top-level Pomodoro with a valid time range is the current
   Pomodoro.
2. If there is no current Pomodoro, the last completed (`[x]` or `[X]`)
   top-level Pomodoro in document order is used.
3. If neither exists, the bullet stays where it is and only the link is
   struck.

Relocation happens at bullet granularity. Nested descendants move with their
parent, multiple moved bullets retain their document order, and the root
indentation is normalized to the destination's child indentation. When the
current Pomodoro is already the owner, only retirement is needed. A repair
found beneath a completed Pomodoro is always normalized in place and is never
moved into a newer session. On a bullet moved to the completed fallback, a
completed embedded link becomes unmarked `~~[[...]]~~`, while a completed live
non-transcluded link becomes `🍅 ~~[[...]]~~`. A bullet moved to the current
open Pomodoro remains unmarked.

For example, a completed task under a future Pomodoro is moved to the current
timed entry:

```markdown
- [ ] Current work (0900-0930)
  - ~~[[dev#^finished]]~~
- [ ] Future work
```

With no timed open entry, the last completed entry is the fallback:

```markdown
- [x] Earlier work
  - 🍅 ~~[[dev#^finished]]~~
- [ ] Future work
```

With only untimed open entries, the link is struck in place because no
relocation target exists.

A task must have a trailing `^block-id` to be linked. The edit changes only
the status character, preserving indentation, list markers, descriptions,
block IDs, and line endings.

The vault scan skips dot-prefixed directories, `done/`, `_generated/`, and
`_templates/`, so archived tasks and templates are never synchronized.
Consequently, a dependency link into `done/` is reported as unresolved; the
archived task itself remains untouched.

## Guard Rails

The command exits with status 1 and writes nothing when the current daily note
is missing, has no `## Pomodoros` section, or contains multiple open timed
Pomodoros. A valid but empty current section is a valid source of truth: it
clears every scanned `[*]` task and applies scoped stale-In-Progress rollback
using the optional previous source. This distinction prevents a missing or
malformed current ledger from causing a mass clear.

The previous daily is optional and never weakens those current-ledger guards.
No previous note is valid, while a selected previous note with no Pomodoros
section is a successful empty historical source. The selected file remains
byte-for-byte read-only even when the current ledger and task notes change.

A planned Blocked/unblocked status edit also fails atomically when the Tasks
registry is unreadable or its Blocked definition is missing, duplicated, or
incompatible. Dry-run uses the same guard. This prevents both an unknown `[?]`
marker and partial composition with daily-note structural edits.

Unresolved direct or dependency links are warnings, not failures. If duplicate
task block IDs occur in one resolved note, every matching task is synchronized
and the ambiguity is reported. Completed-link normalization proceeds only when
all duplicate matches are complete; conflicting completion states are warned
and left structurally unchanged. Canceled-reference list-item removal likewise
proceeds only when every match has a recognized `CANCELLED` status; mixed
cancellation states are warned and retained.

## Output

Human output lists every Next promotion, In-Progress promotion, Next clear,
scoped In-Progress clear, Blocked transition, unblock, duplicate line removal,
retired reference, move, and marker repair, plus every canceled-reference
list-item trigger, followed by a summary. The selected previous daily path and
its reference count appear in changed and no-op reports.
Canceled-reference rows show the target, block ID, original one-based line
number, and owning Pomodoro that triggered complete list-item deletion. Blocked
rows include the open dependency IDs;
unblocked rows include unresolved IDs when present. Duplicate removals show the
original daily-note line number, text, owning Pomodoro, and canonical task
identities. Marker additions and removals have their own
`marked`/`unmarked` sections and summary counts. Dependency-derived promotions
carry a `(dependency)` suffix. Next and In-Progress promotions have separate
sections and summary counts. Dry-run
uses the same planning path and reports what would happen
without changing any file. Warnings go to stderr. A no-op prints a single
`already in sync` line only when neither task statuses nor daily-note links
need changes.

JSON mode prints one object on stdout with these stable fields:

```json
{
  "ok": true,
  "dry_run": true,
  "daily_file": "2026/20260710.md",
  "previous_daily_file": "2026/20260703.md",
  "open_pomodoros": 1,
  "references": 2,
  "previous_daily_references": 3,
  "recent_activity_references": 4,
  "dependency_references": 1,
  "scanned_files": 128,
  "marked_next": [
    {
      "path": "dev.md",
      "line_number": 12,
      "block_id": "write-design",
      "description": "Write the design",
      "dependency": false
    },
    {
      "path": "dev.md",
      "line_number": 18,
      "block_id": "write-tests",
      "description": "Write tests",
      "dependency": true
    }
  ],
  "marked_in_progress": [
    {
      "path": "dev.md",
      "line_number": 24,
      "block_id": "review-tests",
      "description": "Review tests",
      "dependency": true
    }
  ],
  "cleared": [],
  "cleared_in_progress": [
    {
      "path": "Projects/Alpha.md",
      "line_number": 29,
      "block_id": "stale-work",
      "description": "Stale project work",
      "dependency": false
    }
  ],
  "marked_blocked": [
    {
      "path": "dev.md",
      "line_number": 30,
      "block_id": "ship",
      "description": "Ship the feature",
      "from": "/",
      "to": "?",
      "open_dependency_ids": ["dev__review"],
      "unresolved_dependency_ids": []
    }
  ],
  "unblocked": [
    {
      "path": "dev.md",
      "line_number": 38,
      "block_id": "released",
      "description": "Release the feature",
      "from": "?",
      "to": "*",
      "open_dependency_ids": [],
      "unresolved_dependency_ids": ["deleted_task"]
    }
  ],
  "struck_completed_references": [
    {
      "target": "dev",
      "block_id": "finished",
      "pomodoro": "- [ ] Future work",
      "removed_embed": false
    }
  ],
  "embedded_completed_references": [],
  "moved_completed_references": [
    {
      "target": "dev",
      "block_id": "finished",
      "source_pomodoro": "- [ ] Future work",
      "destination_pomodoro": "- [ ] Current work (0900-0930)"
    }
  ],
  "marker_added_references": [
    {
      "target": "dev",
      "block_id": "finished",
      "pomodoro": "- [x] Earlier work"
    }
  ],
  "marker_removed_references": [],
  "removed_canceled_references": [
    {
      "target": "dev",
      "block_id": "canceled-work",
      "line_number": 8,
      "pomodoro": "- [ ] Current work (0900-0930)"
    }
  ],
  "removed_duplicate_lines": [
    {
      "line_number": 9,
      "pomodoro": "- [ ] Later work",
      "line": "  - [[Alpha#^ship|duplicate]] and [[Beta#^review]]",
      "duplicate_tasks": [
        {
          "path": "Projects/Alpha.md",
          "block_id": "ship"
        }
      ]
    }
  ],
  "kept_next": 0,
  "kept_in_progress": 1,
  "unresolved_references": []
}
```

`references` retains its input-count contract and counts unique raw direct
Pomodoro block links before structural cleanup; consumers do not need to
reinterpret that older field. A canceled reference removed during this run
therefore remains part of `references`, while the post-rewrite graph excludes
it. `dependency_references` counts additional unique task blocks reached
through dependency edges in the final rewritten ledger.
`previous_daily_file` is the optional vault-relative selected historical path;
it is `null` when no earlier canonical daily exists.
`previous_daily_references` counts unique eligible non-retired raw links in
that note, or zero for no note or a note without a Pomodoros section.
`recent_activity_references` counts unique resolved path-plus-block roots from
the normalized current and previous sources before dependency traversal.
`marked_next` contains only `[ ] -> [*]` changes, while
`marked_in_progress` contains both `[ ] -> [/]` and `[*] -> [/]` changes. Each
change item's `dependency` boolean distinguishes direct references from
dependency-only graph reachability. `marked_blocked` and `unblocked` are
additive fields and do not duplicate changes into those older arrays.
`cleared` remains the compatibility list for `[*] -> [ ]`, while
`cleared_in_progress` separately reports scoped `[/] -> [ ]` changes with the
same path, line, block ID, description, and dependency shape. Their
`from`/`to` values are the actual checkbox symbols, and their dependency-ID
arrays explain the derived decision. Each
`removed_canceled_references` item represents one removed occurrence and
contains its link `target`, `block_id`, one-based original `line_number`, and
owning `pomodoro`. This stable compatibility field reports the qualifying
references that triggered list-item deletion; multiple qualifying occurrences
on one deleted item remain separate entries. The array follows deterministic
file/occurrence order. Each
`removed_duplicate_lines` item represents one physical line and contains its
one-based original `line_number`, original `line`, owning `pomodoro`, and one or
more canonical path-plus-block `duplicate_tasks`. Each unresolved reference
contains `target`, `block_id`, and `reason`; marker-reference entries contain
`target`, `block_id`, and the owning `pomodoro` line. JSON
failures also remain machine-readable as `{ "ok": false, "error": "..." }`.
`embedded_completed_references` is a deprecated, always-empty compatibility
field for one contract cycle.

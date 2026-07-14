# Next Task Sync

`bob mark-next-tasks` makes today's Pomodoro ledger the source of truth for
which vault tasks have the Obsidian Tasks **Next** status (`[*]`) and keeps
references to completed tasks retired as struck, non-embedded links beneath
their Pomodoros. Live non-transcluded links beneath completed Pomodoros carry
the machine-owned Pomodoro marker (`🍅`); embedded and provenance-unknown
retired links do not. Links beneath open Pomodoros are unmarked. It
also follows transcluded dependency bullets recursively, so the complete
active dependency chain becomes Next.

## Usage

```bash
bob mark-next-tasks [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json]
```

The vault root comes from `--bob-dir`, then `BOB_DIR`, then `~/bob`. The daily
note comes from `BOB_DAY_FILE` when set; otherwise it is
`<vault>/YYYY/YYYYMMDD.md` for the local date or `BOB_NOW` override.

Use `--dry-run` to compute and print the complete sync without writing files.
Repeated successful runs are idempotent.

## Sync Rules

Within the daily note's `## Pomodoros` section, the command reads block links
from indented bullets beneath open top-level Pomodoro entries:

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

The rewritten Pomodoro section is scanned again after duplicate removal and
completed-reference structural rewrites. Only references that will actually be
written contribute to the direct desired set and dependency closure. Thus an
otherwise-live task mentioned only as unrelated content on a removed line, and
its otherwise-unreachable dependency chain, are cleared from Next in that same
run.

After resolving the surviving direct Pomodoro links, the command reads
dependency edges from the linked tasks' child blocks. An edge must be a child
bullet whose entire content is one transcluded block link:

```markdown
- [ ] #task Ship the feature ^ship
  - ![[#^write-tests]]
  - ![[Quality/Review#^review|Review checklist]]
- [ ] #task Write tests ^write-tests
```

Same-note and cross-note targets use the same resolver as Pomodoro links. The
target block must belong to a scanned Tasks task, and a transclusion may use a
display alias such as `![[note#^id|display text]]`. Plain `[[#^id]]` links,
mixed-content bullets, fenced examples, non-task `#^ref` blocks, and
unresolvable targets are not dependency edges. Unresolvable candidates emit a
warning naming the referencing task's file and line.

Block fragments are file-scoped: `Alpha.md#^review` and `Beta.md#^review` are
distinct graph nodes, and an explicit transclusion path selects only its named
note. The command deliberately traverses these resolved path-plus-fragment
links rather than `[id::]`/`[dependsOn::]` metadata. Tasks metadata represents
the same target vault-wide with a path-qualified value such as
`Alpha__review`.

Traversal is breadth-first and cycle-safe. Dependencies of dependencies are
included, while a task reached both directly and through a dependency is
counted as direct. Removing a Pomodoro link removes that task's otherwise
unreachable dependency chain from the desired set on the next run.

The graph does not consult `#hide`, `[id::]`, or `[dependsOn::]`. In
particular, `#hide` can keep a generated reference task out of dashboards and
Tasks dependency metadata without preventing a sole transcluded child link
from propagating Next to that task.

The command scans Markdown task lines allowed by the Obsidian Tasks
`globalFilter` setting. If that setting cannot be read, the filter defaults to
`#task`. It then applies these transitions:

| Existing status | Reachable from an open Pomodoro | Result |
| --- | --- | --- |
| `[ ]` | yes | `[*]` |
| `[*]` | no | `[ ]` |
| `[*]` | yes | unchanged |
| `[/]` | either | unchanged |
| done, canceled, or unknown | either | unchanged |

The machine-managed `#task #ref ... ^ref` reading task in a generated reference
note is an ordinary scanned task. Promoting it from `[ ]` to `[*]` therefore
flows through the next highlights sync as reference `status: next`; clearing it
back to `[ ]` flows through as `status: ready`. `[/]`, `[x]`/`[X]`, and `[-]`
remain untouched by `mark-next-tasks`. Because the highlights lifecycle is also
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

The command exits with status 1 and writes nothing when today's daily note is
missing, has no `## Pomodoros` section, or contains multiple open timed
Pomodoros. A valid but empty section is a valid source of truth: it clears
every scanned `[*]` task. This distinction prevents a missing or malformed
daily note from causing a mass clear.

Unresolved direct or dependency links are warnings, not failures. If duplicate
task block IDs occur in one resolved note, every matching task is synchronized
and the ambiguity is reported. Completed-link normalization proceeds only when
all duplicate matches are complete; conflicting completion states are warned
and left structurally unchanged.

## Output

Human output lists every promotion, clear, duplicate line removal, retired
reference, move, and marker repair, followed by a summary. Duplicate removals
show the original daily-note line number, text, owning Pomodoro, and canonical
task identities. Marker additions and removals have their own
`marked`/`unmarked` sections and summary counts. Dependency-derived promotions
carry a `(dependency)` suffix. Dry-run
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
  "open_pomodoros": 1,
  "references": 2,
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
  "cleared": [],
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
reinterpret that older field. `dependency_references` counts additional unique
task blocks reached through dependency edges in the final rewritten ledger.
Each change item's `dependency` boolean distinguishes the two sources. Each
`removed_duplicate_lines` item represents one physical line and contains its
one-based original `line_number`, original `line`, owning `pomodoro`, and one or
more canonical path-plus-block `duplicate_tasks`. Each unresolved reference
contains `target`, `block_id`, and `reason`; marker-reference entries contain
`target`, `block_id`, and the owning `pomodoro` line. JSON
failures also remain machine-readable as `{ "ok": false, "error": "..." }`.
`embedded_completed_references` is a deprecated, always-empty compatibility
field for one contract cycle.

# Next Task Sync

`bob mark-next-tasks` makes today's Pomodoro ledger the source of truth for
which vault tasks have the Obsidian Tasks **Next** status (`[*]`) and keeps
references to completed tasks embedded beneath the most relevant Pomodoro.

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
  - [[dev#^closed-session-link-is-ignored]]
```

Only links with a block fragment (`[[note#^block-id]]`) count. Ordinary note
links and heading links do not. Targets resolve by an exact vault-relative
path first and then by a unique, case-insensitive note basename. Ambiguous or
missing targets produce warnings and are not guessed.

The command scans Markdown task lines allowed by the Obsidian Tasks
`globalFilter` setting. If that setting cannot be read, the filter defaults to
`#task`. It then applies these transitions:

| Existing status | Linked from an open Pomodoro | Result |
| --- | --- | --- |
| `[ ]` | yes | `[*]` |
| `[*]` | no | `[ ]` |
| `[*]` | yes | unchanged |
| `[/]` | either | unchanged |
| done, canceled, or unknown | either | unchanged |

Completion is classified separately from Next synchronization. Conventional
`[x]` and `[X]` tasks are complete. A custom checkbox symbol is also complete
when its entry in `statusSettings.coreStatuses` or
`statusSettings.customStatuses` has type `DONE`. `CANCELLED`, `IN_PROGRESS`,
`ON_HOLD`, `NON_TASK`, unknown, and unchecked statuses are not complete.

For each bullet beneath an open Pomodoro that contains a block link resolving
unambiguously to a completed Tasks task, the command inserts `!` immediately
before that link. Aliases and neighboring text are preserved, so
`[[dev#^done|result]]` becomes `![[dev#^done|result]]`. Already embedded links
are unchanged. On mixed-content bullets, only links proven complete are
embedded.

The containing bullet is relocated according to this order:

1. The single open top-level Pomodoro with a valid time range is the current
   Pomodoro.
2. If there is no current Pomodoro, the last completed (`[x]` or `[X]`)
   top-level Pomodoro in document order is used.
3. If neither exists, the bullet stays where it is and only the link is
   embedded.

Relocation happens at bullet granularity. Nested descendants move with their
parent, multiple moved bullets retain their document order, and the root
indentation is normalized to the destination's child indentation. When the
current Pomodoro is already the owner, only embedding is needed.

For example, a completed task under a future Pomodoro is moved to the current
timed entry:

```markdown
- [ ] Current work (0900-0930)
  - ![[dev#^finished]]
- [ ] Future work
```

With no timed open entry, the last completed entry is the fallback:

```markdown
- [x] Earlier work
  - ![[dev#^finished]]
- [ ] Future work
```

With only untimed open entries, the link is embedded in place because no
relocation target exists.

A task must have a trailing `^block-id` to be linked. The edit changes only
the status character, preserving indentation, list markers, descriptions,
block IDs, and line endings.

The vault scan skips dot-prefixed directories, `done/`, `_generated/`, and
`_templates/`, so archived tasks and templates are never synchronized.

## Guard Rails

The command exits with status 1 and writes nothing when today's daily note is
missing, has no `## Pomodoros` section, or contains multiple open timed
Pomodoros. A valid but empty section is a valid source of truth: it clears
every scanned `[*]` task. This distinction prevents a missing or malformed
daily note from causing a mass clear.

Unresolved links are warnings, not failures. If duplicate task block IDs occur
in one resolved note, every matching task is synchronized and the ambiguity is
reported. Completed-link normalization proceeds only when all duplicate
matches are complete; conflicting completion states are warned and left
structurally unchanged.

## Output

Human output lists every promotion, clear, embed, and move, followed by a
summary. Dry-run uses the same planning path and reports what would happen
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
  "scanned_files": 128,
  "marked_next": [
    {
      "path": "dev.md",
      "line_number": 12,
      "block_id": "write-design",
      "description": "Write the design"
    }
  ],
  "cleared": [],
  "embedded_completed_references": [
    {
      "target": "dev",
      "block_id": "finished",
      "pomodoro": "- [ ] Future work"
    }
  ],
  "moved_completed_references": [
    {
      "target": "dev",
      "block_id": "finished",
      "source_pomodoro": "- [ ] Future work",
      "destination_pomodoro": "- [ ] Current work (0900-0930)"
    }
  ],
  "kept_next": 0,
  "kept_in_progress": 1,
  "unresolved_references": []
}
```

Each unresolved reference contains `target`, `block_id`, and `reason`. JSON
failures also remain machine-readable as `{ "ok": false, "error": "..." }`.

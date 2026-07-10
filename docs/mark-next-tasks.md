# Next Task Sync

`bob mark-next-tasks` makes today's Pomodoro ledger the source of truth for
which vault tasks have the Obsidian Tasks **Next** status (`[*]`).

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

A task must have a trailing `^block-id` to be linked. The edit changes only
the status character, preserving indentation, list markers, descriptions,
block IDs, and line endings.

The vault scan skips dot-prefixed directories, `done/`, `_generated/`, and
`_templates/`, so archived tasks and templates are never synchronized.

## Guard Rails

The command exits with status 1 and writes nothing when today's daily note is
missing or has no `## Pomodoros` section. A valid but empty section is a valid
source of truth: it clears every scanned `[*]` task. This distinction prevents
a missing or malformed daily note from causing a mass clear.

Unresolved links are warnings, not failures. If duplicate task block IDs occur
in one resolved note, every matching task is synchronized and the ambiguity is
reported.

## Output

Human output lists every promotion and clear, followed by a summary. Warnings
go to stderr. A no-op prints a single `already in sync` line.

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
  "kept_next": 0,
  "kept_in_progress": 1,
  "unresolved_references": []
}
```

Each unresolved reference contains `target`, `block_id`, and `reason`. JSON
failures also remain machine-readable as `{ "ok": false, "error": "..." }`.

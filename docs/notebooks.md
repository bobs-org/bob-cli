# Notebooks

A notebook is a task-free collection of note bullets that always hangs under
an area, a project, or another notebook. It is a vault-root Markdown file
with frontmatter `type: "[[notebook]]"`.

## The file

```markdown
---
parent: "[[dev]]"
template: "[[new_notebook]]"
type: "[[notebook]]"
summary: Linux machine setup, shell shortcuts, and troubleshooting notes.
created: 2026-11-09T09:14:00-05:00
---

# Linux

#dev notes on Linux

- Use <C-x><C-e> to edit the current command in $EDITOR [created::2026-11-09]
  - nested detail is free-form: code, links, images, sub-bullets

## Troubleshooting

- Check which process owns a port before restarting services [created::2026-11-09]
```

Key order on files bob creates is `parent`, `template`, `type`, `summary`,
`created`. The type accepts every form `frontmatter_type_names` accepts:
scalar, flow-list, block-list, and bare `[[notebook]]`.

Identity is the file stem. Reparenting changes placement, never the name.

## Identity, title, parent, owner, root

- **Title:** the first H1 before the first bullet or `##`, with wikilinks
  reduced to display text and emphasis stripped. Otherwise the first
  `aliases:` entry. Otherwise the stem through `humanize_stem`.
- **Parent:** exactly one `parent` that resolves path-aware (`[[x]]`,
  `"[[x]]"`, `[[x.md|label]]`, path-qualified) to an area, a project of any
  status, or another notebook. The chain must reach an area or project and
  must not cycle.
- **Owner:** the nearest area or project on the parent chain (through the
  existing Root Parent walk). Transparent to root, clock, and type walks.
- **Root:** the owner's Root Parent (area at any depth, `root: true`
  project, or outermost project).

Notebooks never become a Pomodoro home, Stats root, Ready lane, or review
item. Unknown note types stay `NoteKind::Other`.

## Counts and recency

- **Notes:** column-zero `-` bullets outside fenced code. Nested detail
  never increments counts. Checkbox rows are errors and are not counted.
- **Sections:** each `##` heading with its column-zero note count below it,
  ending at the next heading of level 2 or less.
- **Last note (`last_note`):** the latest valid `[created::YYYY-MM-DD]`
  date anywhere in the file, or null. File mtimes are never used.

Ordering is deterministic: roots and siblings by display title
case-insensitively, with a path tie-breaker.

## Rules

Hard errors:

| Code | Trigger |
| --- | --- |
| `notebook_parent_missing` | Missing, empty, or unresolvable single parent |
| `notebook_parent_ambiguous` | Multiple parent declarations or targets, or ambiguous link resolution |
| `notebook_parent_kind` | Parent is not an area, project, or notebook, or the chain reaches no owner |
| `notebook_cycle` | Self-parent or a cyclic parent chain |
| `notebook_checkbox` | Any checkbox list row outside fenced code, at any depth |
| `notebook_child_kind` | An area, project, ref, or record instance names a notebook as its parent |
| `notebook_duplicate_block_id` | A block ID repeated within the notebook outside fenced examples |
| `notebook_not_at_root` | Notebook below a subdirectory rather than the vault root |

Soft warnings:

| Code | Trigger |
| --- | --- |
| `notebook_summary_missing` | Summary absent or blank |
| `notebook_summary_long` | Summary exceeds 140 characters (measured in characters) |
| `notebook_summary_multiline` | Summary spans multiple lines |
| `notebook_shape` | Numbered top-level lists, `====` dividers, H3+, extra or misplaced H1s, stray top-level paragraphs |
| `notebook_parent_unquoted` | Parent uses the unquoted `[[x]]` spelling |

Checkbox matching covers `-`, `*`, `+`, and ordered rows with arbitrary
status characters and nesting. Fenced examples with backtick or tilde fences
are inert and keep physical line numbers. Findings never rewrite a file.

`summary:` is one plain line describing scope, not contents. Creation
requires it; `check` warns when it is missing.

## Commands

`bob notebooks` lives in the Vault section of `bob --help`. Bare
`bob notebooks` defaults to the read-only `list`.

```text
bob notebooks [list] [-b DIR] [-f human|json] [-r ROOT]
bob notebooks check [-b DIR] [-f human|json]
```

- `--bob-dir/-b`, `--format/-f`, `--root/-r` all have short aliases.
  Options stay alphabetical.
- Root filtering uses the computed root identity, never ownership or
  directory location. A filter with no matches yields a valid empty report.
- Findings do not make `list` nonzero. `check` exits 1 on any error, 0 for
  warning-only or clean reports, and 2 on usage errors.
- Real I/O failures are reported, never converted into a misleading clean
  scan.
- Both commands are read-only: vault bytes are unchanged. `check` is
  explicitly read-only.

Human `list` groups notebooks by root with root hues, tree glyphs for nested
notebooks, dimmed summaries, issue indicators, and aligned
`N notes · M § · age` counts. Invalid or unowned notes get a visible group.
Human `check` groups actionable findings by notebook and ends with a
notebook, root, error, and warning summary. Output respects `NO_COLOR` and
piped output and never depends on terminal width or mtimes.

`check` is read-only.

## JSON

Every envelope carries `schema_version: 1` and emits no ANSI escapes or
human preamble. List and check share one report shape; `ok` reflects whether
error findings exist.

```json
{
  "ok": true,
  "schema_version": 1,
  "notebooks": [
    {
      "stem": "git",
      "path": "git.md",
      "title": "Git",
      "summary": "Git commands, …",
      "parent": "dev_tools",
      "parent_kind": "notebook",
      "owner": "dev",
      "root": "dev",
      "breadcrumb": ["dev", "Developer tools", "Git"],
      "children": [],
      "notes": 6,
      "sections": [{ "title": "Config", "notes": 2 }],
      "last_note": "2026-11-09",
      "issues": []
    }
  ],
  "catalogs": [],
  "counts": { "notebooks": 26, "roots": 7, "errors": 0, "warnings": 4 }
}
```

Missing ownership or dates are null. Invalid notebooks are retained.
`counts` matches the reported set after list filtering. This phase emits
`catalogs: []`; the later catalog phase fills it.

## Teaching refusals

Notebooks hold no tasks. Wherever a notebook is offered as a task or
reference home, bob teaches the owner route:

```text
x.md is a notebook and holds no tasks — use its owner @dev
```

If no owner resolves, bob teaches that notebooks hold no tasks and directs
the user to choose an area or project and repair the notebook with
`bob notebooks check`. Notebook candidates stay absent from task-move
destinations, inbox-routing destinations, parent candidates, and
`scan_capture_targets`.

See `docs/reference/vault-format.md` for note types and
`docs/reference/json-outputs.md` for the report schema.

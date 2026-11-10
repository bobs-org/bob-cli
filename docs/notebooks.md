# Notebooks

A notebook is a task-free collection of note bullets that always hangs under
an area, a project, or another notebook. It is a vault-root Markdown file
with frontmatter `type: "[[notebook]]"`. Every area or project shows the
notebooks beneath it in a generated [catalog](#the-catalog) that
`bob notebooks sync` maintains.

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

## Capture

Notebook routes parse as `notebook_note` or `notebook_sub_note`. Use
`@notes` for a note, `@notes#Ideas` for a section, `@notes^id` for an
authored block ID, or `@notes+id` for a sub-note. The body stays opaque to
task-marker parsing. `bob capture` writes these modes through the guarded
capture transaction: top-level and section notes append after the last note
in the target region (exact title or whole-slug match; unquoted near misses
refuse with `notebook_section_near_miss` and quoted selectors create
verbatim; otherwise a missing section creates a Title Case `##` at EOF),
named notes carry `^id`, sub-notes append unstamped after their parent at any
depth, and `--route`, `--section`, and `--task` follow the destination.
Dry-run and commit share the same plan and bytes; each item carries
`kind: notebook_note` or `notebook_sub_note` plus a `notebook` object with
`route`, `title`, `breadcrumb`, `section`, `section_action`, `block_id`, and
`parent_block_id`. Human output is `📓 Note → Title › Section`,
`📓 Note → Title › new section Name`, or `📓 Note → Title › ^id`.
Routes to other note types keep their existing behavior. See
[JSON outputs](reference/json-outputs.md#capture-and-protocol-helpers) for the
parse spans.

## Commands

`bob notebooks` lives in the Vault section of `bob --help`. Bare
`bob notebooks` defaults to the read-only `list`.

```text
bob notebooks [list] [-b DIR] [-f human|json] [-r ROOT]
bob notebooks check [-b DIR] [-f human|json]
bob notebooks sync [-b DIR] [-d] [-f human|json]
bob notebooks create PARENT ID -s SUMMARY [-t TITLE] [-b DIR] [-d] [-f human|json] [-o]
```

- Every long option has a short alias; options stay alphabetical.
- Root filtering uses the computed root identity, never ownership or
  directory location. It filters notebooks and host catalogs by that root
  and recomputes `counts`. A filter with no matches is a valid empty report.
- Findings do not make `list` nonzero. `check` exits 1 on any error, 0 for
  warning-only or clean reports, and 2 on usage errors.
- Real I/O failures are reported, never converted into a misleading clean
  scan.
- `list` and `check` are read-only: vault bytes, index, and HEAD are
  unchanged.

| Command | Exit |
| --- | --- |
| `list` | 0 |
| `check` | 1 on any error (including a damaged catalog), else 0 |
| `sync` | 1 when any catalog is damaged or the run fails, else 0 |
| `create` | 2 on validation or refusal, 1 on a runtime failure, else 0 |

Human `list` groups notebooks by root with root hues, tree glyphs for nested
notebooks, dimmed summaries, issue indicators, and aligned
`N notes · M § · age` counts; invalid or unowned notes get a visible group,
and a catalog that is not current is named with the `sync` hint. Human
`check` groups findings by notebook, prints host catalog findings once
under the host path, and ends with a notebook, root, error, and warning
summary. Output respects `NO_COLOR` and piped output and never depends on
terminal width or mtimes.

## The catalog

Every area or project that has notebooks beneath it carries one generated
block. Bob owns the bytes between the markers; the heading, the block's
position, and everything else belong to the note's author.

```markdown
## Notebooks

<!-- bob:notebooks:begin v1 -->

> [!notebooks]
>
> - **[[dev_tools|Developer tools]]** — Command-line tools and dotfile setup.
>   - **[[git|Git]]** — Git commands, configuration, and workflow tips.

<!-- bob:notebooks:end -->
```

- **Scope.** A host lists notebooks whose owner lies in its own subtree,
  stopping at another root: a descendant area, a `root: true` project, or an
  outermost project. Notebooks owned by a non-root sub-project appear in
  that project's own catalog and in its root's under a `🧩 [[project]]`
  group line. Notebooks nest under their parent notebook. A host's own
  notebooks come first, then the groups by project stem. Within each level
  rows sort by display title, case-insensitively, with the path as the
  tie-breaker. A notebook with no resolvable owner chain (a missing,
  ambiguous, or cyclic parent, or not at the vault root) appears in no
  catalog; `check` reports it.
- **Rows.** `**[[stem|Title]]**`, then ` — summary` only when a summary
  exists. Titles and summaries are inert text: link, wikilink, emphasis,
  code, HTML, tag, block-ID, math, comment, highlight, and inline-field
  openers are backslash-escaped, and whitespace collapses to one line.
  No colors, counts, recency, widths, dates, or mtimes are written; hue,
  counts, and age are drawn by clients.
- **First write.** The whole block goes immediately before the first real
  `## Tasks` heading, or at the end of the note after one blank line.
  Frontmatter and fenced examples never count as headings or markers. An
  existing unmarked `## Notebooks` heading is the author's: bob never
  adopts its body.
- **Later writes.** Only the bytes between the markers change, wherever the
  block was moved; moving it never makes it stale. A UTF-8 BOM, LF or CRLF
  line endings, and every other byte survive. A current block produces no
  edit.
- **Empty subtree.** The region is deleted with its markers. The
  `## Notebooks` heading goes too, only when its section is then blank
  lines; authored prose keeps the heading.
- **Damage.** A lone, duplicated, or reversed marker, or an unsupported or
  malformed `bob:notebooks` marker, is `notebook_catalog_damaged`, with the
  path and line. Bob never infers a repair range and never edits that host.
- **States.** `missing` (required content, no region), `stale` (differing
  or removable content), `current`, or `damaged`. Reparenting updates both
  the old and new host.
- **Findings.** A missing or stale host is the warning
  `notebook_catalog_stale` (hint `bob notebooks sync`); a damaged host is the
  error `notebook_catalog_damaged`. Both belong to the host, appear under its
  path in `check`, and count once in the totals.
- **Section protection.** `## Notebooks` is never a section-bullet target or
  completion row: `@dev#notebooks` refuses with
  `notebooks_section_generated` instead of writing before the first
  heading or into the callout.
- `bob projects sync` never lists a notebook on a Sub-projects line.

## Sync

`bob notebooks sync [-d]` repairs every catalog.

- **Dry run** (`-d`) plans read-only, prints a unified diff per host, and
  writes nothing: no lock, no vault-sync, no commit, no Obsidian.
- **A real run** verifies the vault identity and worktree, takes the shared
  bounded maintenance lock, pre-syncs, plans from the synced snapshot,
  revalidates every file preimage and read dependency, installs through the
  shared guarded writer (no unguarded writes), and makes one scoped commit
  `notebooks: sync catalogs (N notes)`, where N counts changed host files.
  It then post-syncs.
- **No-op.** A vault whose catalogs are current writes nothing, commits
  nothing, and skips the post-sync; ordinary vault-sync commits are never
  labelled as catalog commits.
- **Damaged hosts** are skipped and reported while healthy hosts apply
  together; the command exits 1. An incomplete scan (`io`) writes and
  deletes nothing.
- **Failure.** A stale input or an install, verification, or commit failure
  rolls back only what bob wrote, never over a later edit, and says so. A
  commit that stands while the post-sync failed is reported as committed
  locally with `sync.post: "failed"`, apart from an unapplied run.
- **JSON** is one `schema_version: 1` envelope on stdout, failures included;
  waiting and progress chatter goes to stderr.

## Create

`bob notebooks create PARENT ID -s SUMMARY` writes `<parent>_<id>.md` and
every affected catalog row in one guarded write and one commit
(`notebooks: create <file>`).

- **Parent:** an area, a project of any status, or a notebook with a valid
  chain, by stem, `stem.md`, or path. Missing, ambiguous, unsupported, or
  cyclic parents refuse.
- **ID:** the block-ID alphabet (letters, digits, `-`). The file name follows
  the project-note rule: hyphens become underscores. A case-insensitive
  collision refuses, and an existing path is never replaced, even one that
  appears after planning.
- **Summary:** required, one line; over 140 characters warns. **Title:** `-t`,
  else the humanized ID with its first letter capitalized.
- **File:** keys `parent`, `template`, `type`, `summary`, `created`, a blank
  line, and the H1. Summaries use typed YAML, quoted when needed (`yes`,
  `42`, `a: b`). `created` uses Bob's clock, so `BOB_NOW` is deterministic.
- **One view.** Every host edit comes from the post-create view. The parent
  and owner/root ancestry are guarded even when not rewritten. A damaged
  affected catalog refuses the whole create (`notebook_catalog_damaged`);
  an unrelated damaged catalog is tolerated and never repaired.
- **Dry run** (`-d`) shows the new file and host diffs. **`-o`** opens the
  note in Obsidian after a successful real create; failing to open is a
  warning and never rolls anything back.
- Refusal codes: `notebook_id_invalid`, `notebook_summary_required`,
  `notebook_summary_multiline`, `notebook_title_invalid`,
  `notebook_parent_missing`, `notebook_parent_ambiguous`,
  `notebook_parent_unsupported`, `notebook_parent_invalid`,
  `notebook_exists`, and `notebook_catalog_damaged`.

## Nightly

`bob nightly` runs `notebooks` between `period notes` and the final
`vault-sync`. It calls the lock-free core of `sync` once under the lock
nightly already holds, with no extra sync cycles, and a failure never stops
the later steps.

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
  "catalogs": [
    {
      "host": "dev",
      "path": "dev.md",
      "root": "dev",
      "state": "current",
      "notebooks": 17,
      "issues": []
    }
  ],
  "counts": { "notebooks": 26, "roots": 7, "errors": 0, "warnings": 4 }
}
```

Missing ownership or dates are null, and `summary` is the decoded YAML
string. Invalid notebooks are retained.
`counts` matches the reported set after list filtering and includes host
catalog findings once. `sync` and `create` print their own envelopes (see
[JSON outputs](reference/json-outputs.md#notebook-reports)) with `ok`,
`dry_run`, `changed`, `changes[]`, `catalogs`, `findings`, `commit`, `sync`,
and `warnings`; `create` adds `notebook`, and failures add `error`.

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

---
create_time: 2026-06-12 14:21:48
status: done
prompt: sdd/prompts/202606/projects_subprojects_line.md
---
# Plan: `bob projects sync` — one beautiful machine-owned Sub-projects line under `^prj`

## Problem

The just-landed sub-project link bullets work, but they don't look great and their ownership model is hostile to
hand-edited notes. Today sync emits one bare `- [[child]]` bullet per open sub-project, interleaved at the same level as
anything the user wrote, and it claims the entire "pure wikilink bullet" namespace under `^prj`: a user who adds their
own `- [[some_note]]` sub-bullet gets it deleted on the next run, and the only escape hatch is remembering to append
prose. The user wants the generated links grouped into something that looks intentional, with the rest of the `^prj`
sub-bullet space unambiguously theirs. Backward compatibility is explicitly not required — `bob projects sync` has never
been run against the real vault.

## Design

### One machine-owned line, everything else is the user's

All generated sub-project links collapse into a **single grouped line** nested one tab under `^prj`:

```markdown
- [ ] #task Design and configure “projects” in Obsidian! [p::2] ^prj
  - 🧩 **Sub-projects:** [[bob_projects_clean_bad_links]] • [[sase_blog]]
  - Remember to check the Obsidian forum thread!
  - [[scratch_note]] my own link bullet, never touched
```

Why this shape, having looked at the real vault:

- **`^prj` sits at the very top of every project note** (line 8 of `sase_blog.md`, right above the H1). A multi-line
  generated block there pushes the note's own content down; one tight line keeps the head of every parent note calm.
  Open-children-only listing self-prunes, so the line stays short in practice.
- **It reads beautifully in both modes.** In reading/live-preview the line renders as 🧩 **Sub-projects:** followed by
  styled wikilinks separated by `•` — a breadcrumb-like row that matches the vault's minimal, tab-indented aesthetic.
  The 🧩 is the one deliberate splash of flair: the vault is otherwise emoji-free, which makes the marker instantly
  recognizable as machine-generated ("sync owns this line — don't hand-edit it"). It is a single `const` so the sigil or
  the whole prefix is a one-line change.
- **Ownership becomes one sentence.** Sync owns exactly one line — the one starting with `🧩 **Sub-projects:**`. Every
  other sub-bullet under `^prj` is the user's, in any format they like, including bare wikilink bullets. No more
  pure-link-bullet namespace grab, no more extra-text escape hatch, no more duplicate-suppression scanning of user
  prose.
- Plain list item, no checkbox, no `#task`: invisible to the dash Tasks query and to the scanner's task counters,
  exactly like the current bullets.

### Canonical form

The line is fully canonical and regenerated wholesale whenever it drifts:

- Text: `<prj_indent>\t- 🧩 **Sub-projects:** [[a]] • [[b]] • [[c]]` — links sorted case-insensitively by stem,
  original-case stems, no path, no alias, `•` separator.
- Indentation is always the `^prj` line's leading whitespace plus one tab (the vault convention), normalized even if the
  user dragged the line elsewhere.
- Position: an existing marker line is rewritten **in place** (minimal churn if the user reordered their own bullets
  above it); when absent it is inserted directly after the `^prj` line; when the open-children set is empty it is
  deleted. Duplicate marker lines (anything else in the sub-block starting with the marker prefix) are deleted — the
  prefix namespace is machine-owned.

### Reconciliation

Within the existing gate (effective status non-terminal, `^prj` state `Open`):

- desired = the open-children stem list already computed for `[p::2]` (same single source of truth, same single-run
  convergence: the run that checks a child's `^prj` rewrites or deletes the parent's line in the same pass).
- existing = the wikilinks on the first marker line (empty when no marker).
- Set difference drives per-link reporting exactly as today: `added [[x]] to ^prj  open sub-project` /
  `removed [[x]] from ^prj  no longer an open sub-project`, with `would add`/`would remove` in dry-run.
- When the set is unchanged but the line is non-canonical (wrong order, separator, indentation, hand-edits inside the
  line, duplicate marker lines), one normalization event is emitted via a new `PrjEditAction::Update` arm:
  `updated sub-projects on ^prj  canonical format` (`would update` in dry-run). All of these count toward the existing
  `^prj edited` summary bucket.

No backward compatibility: previously generated bare `- [[child]]` bullets are now ordinary user content and are left
alone (the real vault has none — sync never ran there).

### Rejected alternatives

- **Marker bullet with nested link bullets** (`- **Sub-projects:**` + one indented bullet per child): scales to huge
  child lists, but costs N+1 lines at the very top of every parent note, needs nested-run ownership rules (user edits
  inside the block, partial indents, where the block ends), and looks heavier in the vault's actual notes. The single
  line wraps gracefully in Obsidian if a parent ever has many open children.
- **Comment-delimited region** (`%% bob:subprojects %%` fences): robust ownership, but ugly in source mode and
  invisible-markers-with-side-effects is exactly the kind of magic the vault avoids.
- **Status quo per-bullet ownership with a smarter heuristic**: any rule keyed on bullet _shape_ (pure link vs prose)
  keeps colliding with user content; keying on an explicit marker prefix makes ownership visible and total.

## Implementation

All changes in `src/native/projects.rs` plus docs and tests. No new CLI flags or subcommands.

### 1. Parser

- Marker prefix constant (`🧩 **Sub-projects:**`) and separator constant (`•`).
- `PrjSubBlockLine` drops `pure_link`/`wikilink_targets` in favor of: the trimmed line text (for canonical comparison),
  `is_marker` (list item whose content after the bullet starts with the marker prefix), and `links: Vec<WikilinkRef>`
  (original-case stem + normalized name, for set diffing and reporting). `parse_pure_link_bullet` and
  `PrjSubBlock::linked_targets()` go away.

### 2. Planning

`plan_project_sync()` keeps its signature (project + open-children stems). Inside the existing open/non-terminal block,
after the priority logic:

- `ProjectChange::AddSubprojectLink { stem }` per desired stem missing from the first marker line's links
  (case-insensitive).
- `ProjectChange::RemoveSubprojectLink { stem }` per marker-line link not in the desired set (`target`/`line_number`
  fields are dropped — applying no longer needs them).
- `ProjectChange::NormalizeSubprojects` when the set diff is empty but the sub-block is non-canonical: first marker line
  text differs from the canonical render, or extra marker lines exist. Renders through the new `PrjEditAction::Update`
  (`updated`/`would update`, preposition `on`, field `sub-projects`, reason `canonical format`).

### 3. Applying edits

All three change variants funnel into one `sync_subprojects_line_edits(contents, added, removed)` helper that re-derives
the sub-block layout (existing `prj_sub_block_layout`):

- final set = (first marker line's links ∖ removed) ∪ added, sorted case-insensitively.
- empty → delete every marker line (full line spans, CRLF-safe).
- non-empty → one replacement edit for the first marker line (or one insertion directly after the `^prj` line when no
  marker exists, reusing the existing `line_ending()`/EOF-without-newline handling) plus deletion edits for any extra
  marker lines.
- Replacements/insertions land on lines distinct from the `^prj` line itself, so spans never overlap the
  priority/scheduled edits.

### 4. Docs and help text

- `docs/projects.md`: replace the three sub-project-bullet Sync Rules bullets and the duplicate-suppression paragraph
  with the new model — the canonical line format, total ownership of the marker-prefixed line, "every other sub-bullet
  is yours", deletion-when-empty — and update the example block and typical output (including an `updated sub-projects`
  sample line).
- `sync` subcommand `long_about`: reword the sub-project sentence to describe the single Sub-projects line.

### 5. Tests

Unit tests (rewriting the existing sub-project link tests; no back-compat cases):

- Parsing: marker recognition (prefix match, original-case links extracted), non-marker bullets (prose, bare wikilinks)
  carry links but are not markers, marker at deeper nesting still recognized, sub-block lines still never affect task
  counts.
- Planning: adds/removes from set diff; normalize on wrong order/separator/indent and on duplicate marker lines;
  canonical line → no changes; user bullets (bare links included) never produce changes; case-insensitive matching;
  gating unchanged (terminal projects and missing/checked/malformed `^prj` get no line edits).
- Edits: fresh insert directly after `^prj` (tab indent); in-place rewrite preserving line position below user bullets;
  deletion when last child closes; duplicate marker cleanup; CRLF preservation; `^prj` as final line without trailing
  newline.

Integration tests (`tests/cli.rs`, rewriting the four existing sub-project link cases):

- Parent with two open children: one Sub-projects line with both links sorted and `•`-separated; second run is a no-op.
- Checking the only child's `^prj`: same run flips the child to done, deletes the parent's Sub-projects line, and
  removes `[p::2]`; second run is a no-op.
- User sub-bullets (prose and a bare `- [[non_project_note]]`) survive untouched, with the generated line inserted
  directly after `^prj` above them.
- Hand-mangled marker line (reordered links, extra prose) is rewritten canonically with the `updated sub-projects`
  event.
- Dry-run prints `would add`/`would remove`/`would update` without writing.

## Out of scope

- `bob projects list` output is unchanged.
- No status decoration of child links and no alias text in generated links.
- Links elsewhere in the note body are untouched.
- No new CLI flags or subcommands; no config for the marker text (it is a code constant).
- No validation of `parent` values (cycles, dangling links) — unchanged.

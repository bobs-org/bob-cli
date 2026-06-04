---
create_time: 2026-06-04
status: research
topic: PDF, Highlights, Dataview, and Obsidian task workflow roadmap
---
# Research: PDF Highlights Tasks Roadmap

## Short Answer

Use `~/bob/lib/<ref_type>/` as the active PDF library for Highlights-driven
reference notes, and let Obsidian Sync be the normal transport to Athena. Avoid
ad hoc writes into a live synced vault. For any Athena-side bulk import, stage
files outside the vault, pause or account for the polling `ob` sync service,
copy only content files into `lib/`, then run a one-shot sync and a read-only
`bob highlights scan --dry-run`.

For tasks captured while reading in Highlights, use ordinary Obsidian task
lines in the generated reference note, with Dataview inline fields on the task
line. Do not put captured tasks into note frontmatter. Reference-note
frontmatter should keep describing the source (`type`, `parent`, `status`,
`source_pdf`, `highlights_*`); task-line metadata should describe the action
item and its annotation origin.

The next implementation should build on the existing `bob highlights` pipeline:
sidecar parser, stable generated block IDs, managed Highlights region,
Dataview-readable ref-note frontmatter, and the generated PDF reading task. The
first code feature should be explicit task extraction from Highlights sidecars,
not a broader PDF/Dataview rewrite.

## Verified Local Context

Checked in this workspace on 2026-06-04:

- Project memory says `~/bob/` is Bryan's Obsidian vault, and Athena uses
  `obsidian-headless` through `ob` for Obsidian Sync.
- `ob-sync-bob.service` is active on Athena and runs the polling wrapper
  `/home/bryan/.local/bin/ob-sync-bob-poll`, not a fragile long-running
  `ob sync --continuous` watcher.
- `bob nightly` owns the shared `ob sync --path <vault>` gate before
  maintenance writes. `bob highlights` and `bob dataview` both report
  `ob_sync: not-run` / no sync; they read the local vault state as-is.
- `docs/highlights-ref-sync.md` defines active defaults:
  `BOB_HIGHLIGHTS_LIB_DIR=lib` and `BOB_HIGHLIGHTS_REF_DIR=ref`.
- Current active `~/bob/lib` contains 5 PDFs and 4 adjacent Markdown sidecars
  totaling about 3.4M. `~/bob/old_lib` is a much larger tracked legacy PDF
  archive at about 814M.
- `bob highlights doctor` found 5 active library PDFs, 4 sidecars, 1 missing
  sidecar, 5 readable PDF markers, `ob` available, and a dirty vault.
- A read-only Dataview query found 12 reference notes with `source_pdf`; 5 of
  those source PDFs exist under active `~/bob/lib`, and 7 currently point at
  missing active `lib/...` paths on Athena.
- Bryan's Tasks plugin uses `globalFilter: "#task"` and `taskFormat:
  "dataview"`. Dataview is installed at `0.5.68`.

## Current Highlights Model

`bob highlights sync <pdf>` reads the first standalone page-1 PDF text note as
the marker, parses a flat marker list, and creates or updates a reference note
under `ref/<ref_type>/...` for PDFs under `lib/<ref_type>/...`.

Reference-note frontmatter currently includes:

- user/reference fields such as `status`, `parent`, `title`, `topics`, `url`;
- command-managed `type: "[[ref]]"` and path-derived `ref_type`;
- pipeline fields such as `source_pdf`, `source_pdf_sha256`,
  `highlights_sidecar`, `highlights_count`, `highlights_synced_at`,
  `highlights_marker_hash`, `highlights_marker_base`, and
  `pipeline_version`.

The body contract is intentionally narrow:

- manual content outside the managed region is preserved;
- generated highlights live inside `<!-- highlights:begin -->` /
  `<!-- highlights:end -->`;
- generated highlight/note blocks get stable `^h-...` block IDs;
- removed generated blocks are preserved as tombstones;
- the generated PDF task line is a reading-status affordance:

```md
- [ ] #task [[lib/books/example.pdf]] [p::2] ^task
```

Checking that generated PDF task maps the reference's `status` to `read`. It is
not a general action-item capture mechanism. A captured task from a highlight
should not set the reference note to `read`, and a completed captured task
should not write back to the PDF marker during the MVP.

## PDF Movement and Athena Sync

The safest transport model is:

1. Put PDFs and their Highlights sidecars into the Obsidian vault on the
   machine where reading/annotation happens, normally the MacBook:
   `~/bob/lib/<ref_type>/<name>.pdf` and `~/bob/lib/<ref_type>/<name>.md`.
2. Let Obsidian Sync propagate those content files to Athena.
3. Let Athena's polling `ob-sync-bob` service pull them into local `~/bob`.
4. Run `bob highlights scan --dry-run` on Athena to validate markers,
   sidecars, output paths, and dirty-target safety.

For direct Athena-side imports, use a controlled import instead of rsyncing into
a live vault tree:

1. Run or wait for a clean `ob sync --path ~/bob` cycle first.
2. Stage incoming PDFs and sidecars outside the vault.
3. Copy only content files into `~/bob/lib/<ref_type>/`, preferably via
   temp-file plus rename or an rsync mode that does not expose partial
   destination files.
4. Do not copy another machine's `.obsidian`, `.git`, sync cache, or plugin
   state into Athena's vault.
5. Keep Highlights, desktop Obsidian, and bulk PDF marker writes idle during
   the import window.
6. Run `bob highlights doctor` and `bob highlights scan --dry-run`.
7. Run `ob sync --path ~/bob`, then leave the polling service to continue.

For search/indexing jobs that do not need Obsidian links, mirror PDFs outside
the vault instead of writing back into `~/bob`. The vault should remain the
source of truth; an external mirror can be regenerated from it.

`old_lib` should stay a legacy/archive concern until there is a deliberate
migration. Do not point the default Highlights scan at `old_lib`; most old PDFs
will not have the current marker/sidecar contract, and scanning them would
create noise before the active `lib` pipeline is stable.

## Dataview Task Model

Captured annotation tasks should be rendered as normal task lines because that
is what Dataview and the Tasks plugin index well:

```md
- [ ] #task Support SASE tool-call replay? [p::2] [task_source:: highlights] [source_ref:: [[ref/papers/log_is_the_agent#^h-383c9e969ec8]]] [source_page:: Page 2] ^ht-383c9e969ec8-1
```

Recommended semantics:

- `#task` is required so the Tasks plugin sees it under Bryan's global filter.
- `[p::2]` follows current Bob convention and makes priority visible to
  Dataview as `p`. Use `[priority:: ...]` only if Tasks-plugin priority UI
  support is specifically needed.
- `[task_source:: highlights]` distinguishes annotation-captured tasks from
  the generated PDF reading task and from ordinary Bob tasks.
- `[source_ref:: [[ref/...#^h-...]]]` links back to the generated highlight or
  note block that produced the task.
- `[source_page:: Page 2]` preserves the readable page label from the sidecar.
- The task inherits page-level Dataview fields from the reference note:
  `source_pdf`, `source_pdf_sha256`, `parent`, `status`, `ref_type`,
  `highlights_sidecar`, and `highlights_count`.
- A stable task block ID such as `^ht-...` gives Dataview, Obsidian links, and
  future reconciliation a durable identity.

Good Dataview selectors after implementation:

```dataview
TASK
FROM "ref"
WHERE contains(tags, "#task")
  AND task_source = "highlights"
  AND !completed
```

```dataview
TABLE status, parent, source_pdf, highlights_count
FROM "ref"
WHERE source_pdf
SORT file.path ASC
```

The key design choice is that frontmatter remains note-level metadata. Task
state, priority, scheduling, dependencies, and provenance belong on the task
line. That matches Bryan's existing Dataview-format task usage and avoids
inventing a separate task database.

## Capturing Tasks in Highlights

Use an explicit syntax first:

```md
- [ ] #task Support SASE tool-call replay? [p::2]
```

This can appear in a standalone Highlights note or as a comment after a
highlight. The parser should detect explicit Markdown task lines before the
current comment-list normalization strips unordered-list markers.

Avoid implicit conversion in the first pass. A plain sidecar bullet like:

```md
- Support SASE tool call replay?
```

is currently just a comment. Converting every bullet into a task would create
false positives in research notes. Shorthands such as `Task:` or `TODO:` can be
added later if the explicit task syntax feels too heavy.

Rendering should preserve task status across syncs. If Bob regenerates the
managed Highlights region from a sidecar, it must not reset a task the user has
checked in Obsidian. The practical model is:

- generate task IDs deterministically from source PDF, annotation block ID,
  task ordinal, and normalized task text;
- when re-rendering, read existing generated task lines by block ID;
- preserve checkbox status and Tasks fields such as `[completion::]`,
  `[cancelled::]`, `[scheduled::]`, `[due::]`, `[id::]`, and `[dependsOn::]`
  when the generated task identity still matches;
- treat text changes as a new task unless a future reconciliation rule is
  added.

## Relationship to Existing `bob highlights`

This should be an extension of `src/native/highlights_ref/mod.rs`, not a new
parallel sync command.

Reuse existing pieces:

- config and path rules for `bob_dir`, `lib_dir`, and `ref_dir`;
- sidecar discovery for `foo.md` and `foo.textbundle/text.md`;
- marker/frontmatter projection and conflict detection;
- generated-note body rendering and managed-region replacement;
- stable block-ID style;
- dirty-target safety and dry-run reporting;
- Dataview-readable frontmatter contract.

Keep ownership boundaries:

- PDF marker/frontmatter sync remains two-way and conflict-aware.
- Highlights, comments, notes, and captured tasks are one-way from sidecar/PDF
  into the reference note.
- Captured-task completion state is note-side state and should be preserved
  locally; do not write it back into the PDF marker in the first pass.
- The generated PDF `^task` line remains the read-status affordance. Captured
  annotation tasks should use separate `^ht-...` IDs and
  `[task_source:: highlights]`.

The dry-run report should eventually include captured-task counts and actions,
for example `highlight_tasks: 3`, `highlight_tasks_preserved: 1`, and
`highlight_tasks_created: 2`. This makes scheduled scans auditable without
having to inspect every generated note.

## Implement First

1. Settle current active library state.
   - Resolve the current checked-task/status conflicts reported by
     `bob highlights scan --dry-run`.
   - Decide whether the 7 Dataview `source_pdf` paths that are missing under
     active `~/bob/lib` should be copied into `lib`, moved to archival notes,
     or left as known missing assets.
   - Get `bob highlights doctor` and `bob highlights scan --dry-run` to a
     clean, explainable baseline.

2. Write down the Athena PDF import procedure.
   - Use `lib/<ref_type>/` as the active destination.
   - Keep `old_lib` out of the default scan path.
   - Use staging, one-shot sync checks, and dry-run validation.

3. Add task extraction for explicit Markdown task lines in sidecars.
   - Parse explicit `- [ ] #task ...` / `- [x] #task ...` lines before comment
     normalization.
   - Render captured tasks with stable `^ht-...` block IDs and task-line
     inline metadata.
   - Preserve existing task status and Tasks fields by block ID on re-render.
   - Add focused tests for standalone-note tasks, highlight-comment tasks,
     checked-task preservation, and non-task bullets staying comments.

4. Add Dataview/Bases views after the data exists.
   - A reading queue can use the existing generated PDF `^task` lines.
   - An action queue can query `task_source = "highlights"`.
   - Searchable reference tables can keep using `source_pdf`,
     `highlights_count`, `status`, and `parent`.

## Defer

- Two-way write-back of captured task completion into PDF annotations or
  sidecars.
- Implicit task shorthands such as `TODO:` or ordinary bullet conversion.
- Bulk migration of `old_lib` into the active Highlights marker/sidecar
  contract.
- Full PDF text extraction, OCR, semantic summaries, or AI-generated tasks.
- DataviewJS support or live Obsidian rendering in `bob dataview`.
- Cross-device conflict resolution beyond the existing marker/frontmatter and
  dirty-target safety model.

## Sources

Local files and commands used:

- `memory/long/obsidian.md`, read through `sase memory read`
- `docs/highlights-ref-sync.md`
- `docs/dataview.md`
- `README.md`
- `src/native/highlights_ref/mod.rs`
- `src/native/ob.rs`
- `src/native/nightly.rs`
- `sdd/research/202606/dataview_parity_consolidated.md`
- `sdd/research/202606/bulk_obsidian_task_properties.md`
- `sdd/tales/202606/highlights_sidecar_style_1.md`
- `sdd/tales/202606/highlights_ref_comment_bullets.md`
- `sdd/tales/202606/highlights_ref_pdf_task_line.md`
- `sdd/tales/202606/migrate_ref_pdf_lines_to_tasks.md`
- `sdd/tales/202606/highlights_ref_task_done_status.md`
- `bob dataview` read-only queries against `~/bob`
- `bob highlights doctor`
- `bob highlights scan --dry-run`
- read-only inspection of `~/bob/.obsidian/plugins/obsidian-tasks-plugin/data.json`
  and `~/bob/.obsidian/plugins/dataview/manifest.json`

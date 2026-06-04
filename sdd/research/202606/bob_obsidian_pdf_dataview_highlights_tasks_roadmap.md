---
create_time: 2026-06-04
status: research
topic: Bob/Obsidian PDF, Dataview, and Highlights task workflow roadmap
---
# Research: Bob/Obsidian PDF, Dataview, and Highlights Task Workflow

## Scope

This consolidates the two independent research drafts:

- `sdd/research/202606/pdf_highlights_tasks_roadmap.md`
- `sdd/research/202606/pdf_athena_dataview_highlights_tasks_roadmap.md`

The request was to connect PDF/reference-note storage, Dataview task metadata,
Highlights sidecars/annotations, and Athena/headless sync constraints into a
practical roadmap for turning PDF reading and annotation into actionable
Obsidian tasks and searchable reference notes.

## Bottom Line

Use `~/bob/lib/<ref_type>/` as the active Highlights PDF library, but do not use
Obsidian Sync as the primary heavy-PDF transport to Athena. The safer operating
model is:

- the Mac/Highlights environment is the only PDF and PDF-marker writer;
- Athena treats PDFs as read-only inputs;
- PDFs move to Athena through git or another explicit one-way mirror;
- Obsidian Sync carries Markdown: ordinary notes, generated `ref/` notes, and
  Highlights `.md` sidecars;
- Obsidian Sync selective sync should exclude the PDF file type and/or
  `lib`/`old_lib` once the PDF channel is intentionally split.

Tasks captured while reading should be rendered as ordinary Markdown task lines
with Dataview bracket inline fields, not frontmatter. They should be generated
inside the managed Highlights region of the reference note, carry `#task`, use
stable task block IDs distinct from the existing `^task` reading-status line,
and inherit page-level metadata such as `source_pdf`, `ref_type`, `parent`, and
`status`.

This is an incremental `bob highlights` renderer extension, not a new pipeline.
The first implementation should support explicit checkbox task lines in
Highlights sidecars, preserve existing task status on re-render, and add
headless `bob dataview` verification. Defer implicit task detection, write-back
of annotation-task completion to PDFs, old-library migration, and scheduled
automation.

## Verified Context

Checked on Athena on 2026-06-04:

- `~/bob/` is Bryan's Obsidian vault, and this machine uses
  `obsidian-headless` through `ob` for local sync workflows.
- This host is Athena (`robots.md` names `%athena` as the desktop machine;
  `linux.md` maps `A = [[athena]]`).
- `ob-sync-bob.service` is active and runs
  `/home/bryan/.local/bin/ob-sync-bob-poll` every 30 seconds.
- `bob highlights` and `bob dataview` do not run `ob sync`; `bob nightly` owns
  the shared `ob sync --path <vault>` gate.
- The active Highlights defaults are `BOB_HIGHLIGHTS_LIB_DIR=lib` and
  `BOB_HIGHLIGHTS_REF_DIR=ref`.
- Obsidian Sync is enabled in `~/bob/.obsidian/core-plugins.json`, and
  `~/bob` also has git remote `git@github.com:bbugyi200/bob.git`.
- `~/bob/.gitignore` explicitly allows PDFs and other attachments. Git tracks
  664 PDFs under `lib`/`old_lib` today: 5 active `lib` PDFs and 659 archived
  `old_lib` PDFs. The filesystem has one additional untracked archive PDF under
  `old_lib`.
- Local PDF footprint under `lib`/`old_lib` is 665 PDFs / 854,606,711 bytes.
  Several `old_lib` PDFs are around 5.0-5.2M bytes.
- Obsidian's current Sync docs list Standard at 5 MB/file and Plus at
  200 MB/file. This makes Sync a brittle channel for near-cap PDFs, especially
  when git already carries the same binaries.
- Current `bob highlights doctor` sees 5 active PDFs, 4 sidecars, one missing
  sidecar, 5 readable PDF markers, `ob` available, `ob_sync: not-run`, and a
  dirty vault worktree.
- Current `bob highlights scan --dry-run` plans 1 update and reports 4
  checked-task/status conflicts where the generated PDF `^task` is checked but
  the marker status is `wip` while the stored base was `read`.
- A read-only `bob dataview` table finds 12 reference notes with `source_pdf`;
  5 of those PDFs are present in active `~/bob/lib`, and 7 are currently
  missing from active `lib` on Athena.
- Tasks plugin settings use `globalFilter: "#task"` and
  `taskFormat: "dataview"`. Dataview is installed at `0.5.68`. Current
  `bob dataview` sees 383 explicit `#task` rows, including 12 generated
  reference-note PDF reading-status tasks.

## Prior-Work Reconciliation

Both prior drafts correctly identify the `bob highlights` generated-note
contract as the right extension point and Dataview task-line fields as the right
task metadata format.

The main conflict was PDF transport:

- one draft recommended letting Obsidian Sync be the normal PDF transport to
  Athena;
- the other recommended excluding PDFs from Obsidian Sync and using git as the
  binary channel.

The consolidated recommendation chooses the second path. The vault is already
dual-synced, git already tracks PDFs, active and archive PDFs are near the
documented Obsidian Sync Standard file limit, and Athena has a polling headless
Sync service that should not race with foreign PDF writes. Obsidian Sync remains
valuable for Markdown freshness, but PDFs should be a deliberately narrowed,
one-writer channel.

The strongest addition from the first draft is the explicit active-library
cleanup step: several `source_pdf` reference notes point to PDFs missing from
Athena's active `lib`, and the scan baseline currently has checked-task/status
conflicts. Those should be made clean before automation.

## PDF Movement to Athena

### Recommended Operating Model

1. **Mac/Highlights is the sole PDF writer.** Run Highlights and any
   `bob highlights ... --write-pdf` operation only where the PDFs are actively
   read and annotated. Keep Athena read-only for PDFs.
2. **Use git, or a future dedicated mirror, for PDFs.** Git already tracks the
   active PDFs and the archive. The lowest-risk first step is for Athena to pull
   PDFs from `origin` instead of receiving ad hoc copies into a live synced
   vault.
3. **Use Obsidian Sync for Markdown.** Generated `ref/` notes and Highlights
   sidecars are small Markdown files and are the useful real-time Sync payload.
4. **Use selective sync to remove PDFs from the Obsidian Sync channel.** Disable
   the PDF type and/or exclude `lib` and `old_lib`. Obsidian's docs note that
   excluding a folder or file type does not automatically delete copies already
   present in the remote vault, so storage reclamation needs a deliberate
   one-time cleanup if it matters.
5. **Keep `old_lib` archived.** Do not add `old_lib` to the default Highlights
   scan. It contains hundreds of legacy PDFs that are not known to follow the
   current marker/sidecar contract.

### Controlled Import Rules

If PDFs must be copied directly to Athena, treat that as a controlled import,
not a background rsync into a live vault:

1. Wait for a clean `ob sync --path ~/bob` cycle, or temporarily pause the
   polling headless sync service for the import window.
2. Stage incoming PDFs and sidecars outside `~/bob`.
3. Copy only content files into `~/bob/lib/<ref_type>/`; never copy another
   machine's `.obsidian`, `.git`, Sync state, plugin state, cache, or workspace
   files.
4. Use temp-file plus rename, or an rsync mode that avoids exposing partial
   destination files.
5. Keep Highlights, desktop Obsidian, and PDF marker writes idle during the
   import.
6. Run `bob highlights doctor` and `bob highlights scan --dry-run`.
7. Run or wait for `ob sync --path ~/bob`, then let the polling service resume.

For search/indexing work that does not need Obsidian links, mirror PDFs outside
the vault and regenerate that mirror from `~/bob`; do not let search tooling
write back into the vault.

## Dataview Model for Highlights-Captured Tasks

### Storage Shape

Captured annotation tasks should be Markdown task lines in generated reference
notes:

```markdown
- [ ] #task Support SASE tool-call replay? [p::2] [task_source:: highlights] [source_page:: Page 2] [source_block:: [[ref/papers/log_is_the_agent#^h-383c9e969ec8]]] ^ht-383c9e969ec8-1
```

Semantics:

- `#task` is required because Bryan's Tasks plugin uses it as the global
  filter.
- Dataview task/list metadata should use bracket fields such as `[p::2]`,
  `[due:: 2026-06-10]`, and `[scheduled:: 2026-06-10]`.
- `[task_source:: highlights]` distinguishes annotation action items from the
  existing generated PDF reading-status line.
- `[source_page:: ...]` preserves the sidecar page label.
- `[source_block:: [[ref/...#^h-...]]]` links back to the generated highlight or
  standalone note block when one exists.
- The task inherits page frontmatter from the reference note, including
  `source_pdf`, `source_pdf_sha256`, `ref_type`, `parent`, `status`,
  `highlights_sidecar`, and `highlights_count`.
- Use stable task block IDs such as `^ht-...`; reserve `^task` for the existing
  one-line PDF reading-status affordance.

Keep task state and action metadata on the task line. Do not place captured
task fields in note frontmatter; frontmatter describes the reference source, not
individual action items.

### Capture Grammar

Start with explicit checkbox tasks in Highlights sidecars:

```markdown
- [ ] #task Follow up on the benchmark claim [p::1]
- [ ] Check whether this applies to Athena [due:: 2026-06-10]
```

The parser should recognize checkbox lines before the current linked-sidecar
comment normalization strips list markers. If the source line is an explicit
checkbox task but omits `#task`, the renderer can add `#task` so the task is
visible to Bryan's Tasks queries. Plain bullets should remain comments in the
MVP; converting every bullet or `TODO:` line would create too many false
positives in research notes.

Support both relevant sidecar surfaces:

- per-highlight comments, where the generated task can link back to the
  highlight block and page label;
- standalone non-marker notes, where the generated task can link to its own
  generated note block.

Document-level marker tasks can be considered later. Starting with sidecars
keeps the marker/frontmatter grammar narrow and avoids mixing source metadata
with reading action items.

### Re-Render Preservation

`bob highlights` regenerates the managed Highlights body. A re-render must not
silently reset task progress.

Recommended policy:

- derive a stable task identity from source PDF path, page label, annotation
  identity, task ordinal, and normalized task text;
- on re-render, match existing generated task lines by `^ht-...` block ID;
- preserve checkbox state and Tasks fields such as `[created::]`,
  `[completion::]`, `[cancelled::]`, `[scheduled::]`, `[start::]`, `[due::]`,
  `[priority::]`, `[id::]`, `[dependsOn::]`, and `[repeat::]`;
- treat text changes as a new generated task unless a future reconciliation
  rule is added.

Useful verification queries after implementation:

```dataview
TASK
FROM "ref"
WHERE contains(tags, "#task")
  AND task_source = "highlights"
  AND !completed
```

```dataview
TABLE source_pdf, highlights_sidecar, highlights_count, status, parent
FROM "ref"
WHERE source_pdf
SORT file.path ASC
```

On Athena these should be runnable with `bob dataview` in native mode without a
desktop Obsidian session.

## Relationship to Existing `bob highlights`

This should extend `src/native/highlights_ref/mod.rs` rather than introduce a
parallel command.

Reuse the existing contracts:

- `bob_dir`, `lib_dir`, and `ref_dir` path rules;
- first-page PDF marker and marker/frontmatter projection;
- sidecar discovery for adjacent Markdown and TextBundle notes;
- simple and linked-page sidecar parsing;
- managed `<!-- highlights:begin -->` / `<!-- highlights:end -->` body region;
- deterministic `^h-...` generated block IDs and removed-highlight tombstones;
- generated PDF `^task` reading-status line;
- dry-run output, dirty-target refusal, byte-identical skip, and atomic note
  writes;
- `--write-pdf` / `--write-pdfs` opt-in for any PDF marker mutation.

Keep ownership boundaries clear:

- marker/frontmatter fields are the existing two-way, conflict-aware sync
  surface;
- highlights, comments, standalone notes, and captured tasks remain one-way
  from sidecar/PDF into the generated reference note;
- the existing `^task` line maps reference reading status to `read`;
- captured annotation tasks are separate action items and should not update the
  reference `status`;
- completed captured tasks should not write back to PDF markers in the MVP.

The generic task-property mutator researched in
`sdd/research/202606/bulk_obsidian_task_properties.md` is related but separate.
It would edit arbitrary user-authored tasks across the vault. Highlights task
support should only write generated reference-note content owned by
`bob highlights`.

## Roadmap

### Implement First

1. **Settle the active library baseline.**
   - Decide whether the 7 missing active `source_pdf` assets should be pulled
     into `~/bob/lib`, archived, or left as known missing references.
   - Resolve the current checked `^task` versus marker/frontmatter `status`
     conflicts reported by `bob highlights scan --dry-run`.
   - Get `bob highlights doctor` and `bob highlights scan --dry-run` to a
     clean, explainable baseline before scheduling anything.

2. **Document and configure the PDF channel split.**
   - Mac/Highlights is the sole PDF writer.
   - Athena pulls or mirrors PDFs read-only.
   - Obsidian Sync carries Markdown and excludes PDFs or PDF-heavy folders.
   - `old_lib` remains out of the default Highlights scan path.

3. **Add explicit sidecar task extraction.**
   - Parse `- [ ] ...` / `- [x] ...` task lines before list-comment
     normalization.
   - Render tasks with `#task`, `[task_source:: highlights]`, page/provenance
     fields, and stable `^ht-...` block IDs.
   - Preserve checkbox status and existing Tasks fields by block ID on
     re-render.
   - Add tests for highlight-comment tasks, standalone-note tasks, non-task
     bullets staying comments, stable IDs, and status preservation.

4. **Add Dataview verification docs.**
   - A reading queue can keep using the existing generated PDF `^task` lines.
   - An action queue should query `task_source = "highlights"`.
   - Reference tables should keep using inherited page fields such as
     `source_pdf`, `highlights_count`, `status`, `parent`, and `ref_type`.

### Defer

- Two-way write-back of captured task completion into PDF annotations or
  sidecars.
- Implicit conversion of plain bullets, `TODO:`, or prose into tasks.
- Marker-level document tasks unless sidecar tasks prove insufficient.
- Bulk migration of `old_lib` into the active marker/sidecar contract.
- Full PDF text extraction, OCR, semantic summaries, or AI-generated tasks.
- DataviewJS, live Obsidian rendering, or interactive task checking in
  `bob dataview`.
- Athena cron/daemon automation beyond the existing headless sync service.
- Moving PDFs out of `bob.git` into LFS, git-annex, a separate repo, or rsync
  mirror. That may become worthwhile, but it is a storage-channel migration,
  not part of the first task-capture MVP.

## Open Questions

- Which Obsidian Sync plan is active? Standard's documented 5 MB/file limit
  makes the channel split urgent for near-cap PDFs; Plus changes urgency but
  not the race/conflict argument.
- Should Bryan author captured tasks as explicit checkbox lines in Highlights,
  or should a later shorthand such as `TODO:` be supported after the explicit
  MVP?
- Is git-tracked PDF storage acceptable long term, or should PDFs eventually
  move to a dedicated binary channel?

## Sources

Local transcripts:

- `~/.sase/chats/202606/bob_cli-ace_run-260604_192529.md`
- `~/.sase/chats/202606/bob_cli-ace_run-260604_192530.md`

Local files and commands:

- `memory/long/obsidian.md`, read through `sase memory read`
- `docs/highlights-ref-sync.md`
- `docs/dataview.md`
- `README.md`
- `src/native/highlights_ref/mod.rs`
- `src/native/ob.rs`
- `src/native/nightly.rs`
- `sdd/research/202606/bulk_obsidian_task_properties.md`
- `sdd/research/202606/dataview_parity_consolidated.md`
- `~/bob/.obsidian/core-plugins.json`
- `~/bob/.obsidian/plugins/obsidian-tasks-plugin/data.json`
- `~/bob/.obsidian/plugins/dataview/manifest.json`
- `bob highlights doctor`
- `bob highlights scan --dry-run`
- read-only `bob dataview` queries against `~/bob`
- read-only `git -C ~/bob` and filesystem checks for PDF counts and sync state

External docs:

- Obsidian Help, Sync settings and selective sync:
  https://help.obsidian.md/sync/settings
- Obsidian Help, Plans and storage limits:
  https://help.obsidian.md/Obsidian+Sync/Plans+and+storage+limits

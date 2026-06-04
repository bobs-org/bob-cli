---
create_time: 2026-06-04 10:45:58
status: pending
prompt: sdd/prompts/202606/highlights_ref_read_status.md
---

# Plan: Change `bob highlights-ref` Completion Status from `done` to `read`

## Context

`bob highlights-ref` currently treats `status` as a synced marker/frontmatter field for generated Obsidian reference
notes. The active status vocabulary in `src/native/highlights_ref/mod.rs` is:

- `unread`
- `wip`
- `done`
- `abandoned`
- `legacy`

The command also has special completion behavior for generated PDF task lines:

```md
- [ ] #task [[lib/books/example.pdf]] ^task
```

When that generated task is checked, the current implementation contributes `status: done`, writes `status: done` into
the reference note, and writes `- status: done` back to the PDF marker when `--write-pdf` is supplied.

There is prior project context showing `read` existed historically in real ref-note metadata before the recent status
standardization. The active command contract, docs, and tests now use `done`, so this change needs to deliberately move
the managed ref-note completion state to `read` rather than doing a shallow documentation update.

This is not a CLI surface change. No new `bob highlights-ref` subcommands, flags, or option semantics are being added,
so the CLI-rules long-term memory is not required. I did use the required audited Obsidian memory read because this
touches Bob vault note and frontmatter workflow semantics.

## Scope

1. Make `read` the canonical completed/read status for `bob highlights-ref` managed reference notes.
2. Remove `done` from the canonical supported ref-note status vocabulary shown to users.
3. Preserve migration compatibility for existing managed notes and PDF markers that already contain `status: done`.
4. Change checked generated PDF tasks so they contribute `status: read`.
5. Update tests and documentation for the new canonical vocabulary and completion semantics.

Out of scope:

- Changing `bob move-done-tasks`, archive note paths, task text, or unrelated `done` terminology outside
  `highlights-ref`.
- Bulk-editing the real `~/bob` vault directly as part of this code change. Existing managed notes/PDF markers should
  migrate through the existing `highlights-ref sync --write-pdf` write path, with dry-run visibility first.
- Rewriting historical SDD tales except where a new plan record is needed.

## Design

### Canonical Status Model

Introduce explicit status constants near the existing `FIELD_STATUS` definition:

- `STATUS_UNREAD = "unread"`
- `STATUS_WIP = "wip"`
- `STATUS_READ = "read"`
- `STATUS_ABANDONED = "abandoned"`
- `STATUS_LEGACY = "legacy"`
- `DEPRECATED_STATUS_DONE = "done"`

Update the canonical allowed list to:

- `unread`
- `wip`
- `read`
- `abandoned`
- `legacy`

Replace helpers named around `done` with read-oriented or completion-oriented names, for example:

- `projection_status_is_read()`
- `status_value_is_read()`
- or a neutral `projection_status_is_completed()` if that reads better locally.

Keep `done` as a deprecated input alias only for migration. It should not appear as the canonical rendered output for
new reference notes, reference frontmatter, or PDF markers.

### Migration-Compatible Normalization

Existing synced notes can have `done` in several places:

- PDF marker note contents.
- Reference note frontmatter.
- `highlights_marker_base` stored snapshot JSON.
- The stored `highlights_marker_hash`, which was computed over the old `done` projection.

If `done` is simply removed from validation, those notes and PDFs become unsyncable before the command can migrate them.
Instead, add shared status normalization for projection inputs:

1. Parse marker/frontmatter/base projections as today.
2. Before final validation and sync resolution, canonicalize `status: done` to `status: read` when the value is a scalar
   string.
3. Track whether marker and/or frontmatter input used deprecated `done`, so reporting and write planning can explain the
   normalization.
4. Continue rejecting non-scalar status values and unsupported statuses such as `queued`, `complete`, or
   `collect_fleeting_notes`.

Normalize the stored base projection too. That keeps three-way merge conflict behavior based on the canonical semantic
state. The old stored hash may still differ, which is acceptable: it causes one planned migration sync where both sides
are treated as having moved to the same canonical projection.

### Marker Write Planning

Today, PDF marker writes are mainly triggered when frontmatter contributes to the selected projection. Deprecated marker
normalization is different: even when the marker is the selected source, the rendered canonical marker differs from the
PDF because `done` must become `read`.

Adjust marker write planning so a PDF marker update is needed when either condition is true:

- frontmatter or checked-task completion contributes to the selected projection and the rendered marker differs from the
  PDF marker text;
- the marker input itself was normalized from deprecated `done` to canonical `read`.

Keep the existing write safety contract:

- `sync --dry-run` previews the marker update without writing.
- plain `sync` refuses before writes if a PDF marker update is needed and `--write-pdf` was not supplied.
- `sync --write-pdf` updates the PDF marker first, refreshes the PDF SHA-256, then writes the reference note.
- writing `scan` still refuses per PDF when marker normalization would require PDF writes; `scan --dry-run` reports the
  needed work.

### Checked PDF Task Semantics

Change the generated PDF task completion signal from `done` to `read`:

- A checked generated `^task` line contributes exactly `status: read`.
- An unchecked generated task still does not infer a replacement status.
- New notes render the generated task checked when the selected projection has `status: read`.
- Existing notes with the generated `^task` line have only that checkbox marker rewritten to match `status: read`.
- Existing notes without the generated task line are not bulk-migrated.

Old `status: done` should behave like `status: read` after normalization, so old completed notes keep their generated
task checked while the synced metadata migrates to the canonical word.

### Conflict Behavior

Keep the existing marker/frontmatter conflict model, but interpret completion canonically:

- Checked task plus unchanged marker/frontmatter selects `status: read`.
- Checked task plus compatible non-status edits auto-merges those edits with `status: read`.
- Checked task plus marker/frontmatter status changed to `done` normalizes to `read` and does not conflict.
- Checked task plus marker/frontmatter status changed to another non-read value, such as `abandoned`, fails without
  writes and tells the user to uncheck the task or set marker/frontmatter status to `read`.
- `--prefer marker` and `--prefer frontmatter` continue resolving marker/frontmatter conflicts, but they should not
  silently discard a checked-task `read` signal.

### Reporting

Update command output so dry runs explain the new status:

- `pdf_task_contribution: status=read`
- sync reason text such as `checked PDF task set status read`
- conflict/help text that says `read`, not `done`

Add a compact report signal when deprecated normalization is involved, for example:

```text
status_normalization: done->read
```

This is especially useful for `scan --dry-run`, because it lets the user find old managed PDFs/notes that need targeted
`sync --write-pdf` migration.

## Implementation Steps

1. Update status constants and validation in `src/native/highlights_ref/mod.rs`.
   - Make `read` canonical.
   - Remove `done` from the canonical supported list.
   - Add deprecated `done` normalization before validation rejects unsupported values.

2. Thread normalization metadata through planning.
   - Track whether marker, frontmatter, or base used deprecated `done`.
   - Normalize base snapshots before three-way merge.
   - Ensure old stored `done` hashes produce a one-time planned canonicalization rather than a hard conflict.

3. Update marker write planning.
   - Treat marker-side `done -> read` normalization as marker write-back work.
   - Preserve the existing `--write-pdf` guard and dry-run behavior.

4. Change task-completion semantics.
   - Replace `status: done` insertion with `status: read`.
   - Update checkbox rendering helpers to check generated tasks when status is `read`.
   - Update dirty-note allowance tests only as needed; the allowed dirty body change is still the exact generated
     checkbox toggle.

5. Update user-facing text.
   - Replace active `highlights-ref` docs and README status vocabulary with `read`.
   - Update checked-task docs and troubleshooting text from `status: done` to `status: read`.
   - Mention that existing `done` inputs are treated as deprecated and normalized to `read` during sync.

6. Update tests.
   - Unit coverage for canonical `read` status validation.
   - Unit coverage that `done` normalizes to `read` for marker/frontmatter/base projections.
   - Integration coverage that checked generated tasks write `status: read` to note frontmatter and PDF markers.
   - Integration coverage that marker/frontmatter `status: read` checks the generated task line.
   - Integration coverage that old `status: done` marker/frontmatter/base state migrates to `read` with
     `sync --write-pdf` and settles on a repeat sync.
   - Integration coverage that plain `sync` refuses before writes when deprecated marker normalization requires a PDF
     marker update.
   - Conflict coverage updated so competing non-read statuses still fail.

7. Verify.
   - `cargo fmt --check`
   - `cargo test highlights_ref`
   - `cargo test --test cli highlights_ref`
   - If those pass quickly, run the broader `cargo test`.
   - Run a focused `rg` over active `highlights-ref` code/docs/tests to ensure remaining canonical `done` references are
     either deprecated-normalization text or unrelated non-ref-note terminology.

## Risks and Mitigations

- Risk: Existing `status: done` PDFs fail before they can be migrated. Mitigation: parse and normalize `done` as a
  deprecated alias, then render canonical `read`.
- Risk: Old `highlights_marker_hash` values cause false conflicts. Mitigation: normalize marker/frontmatter/base
  projections before merge and allow the old hash mismatch to produce one planned canonicalization sync.
- Risk: The command writes PDFs unexpectedly during library scans. Mitigation: keep PDF writes opt-in; `scan --dry-run`
  reports migration work and writing `scan` refuses per PDF when marker writes are required.
- Risk: Renaming `done` broadly breaks unrelated task archive behavior. Mitigation: scope edits and searches to
  `highlights-ref`; leave `move-done-tasks` and archive semantics alone.
- Risk: Users cannot tell why a no-content-looking sync wants a PDF write. Mitigation: add report text for
  `done -> read` normalization and update troubleshooting docs.

## Acceptance Criteria

- `bob highlights-ref` documents canonical statuses as `unread`, `wip`, `read`, `abandoned`, and `legacy`.
- New generated reference notes never use `status: done`; read/completed references use `status: read`.
- Checking the generated `^task` line produces `status: read` in dry-run output, note frontmatter, and PDF markers when
  `--write-pdf` is supplied.
- Existing managed `status: done` notes/PDF markers can migrate to `status: read` through `sync --write-pdf` instead of
  failing validation.
- Plain `sync` and writing `scan` still refuse before writes when a PDF marker update is required.
- Focused highlights-ref tests pass.

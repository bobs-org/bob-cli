---
create_time: 2026-06-03 22:39:02
status: done
prompt: sdd/prompts/202606/ref_status_standardization.md
---
# Plan: Reference Note Status Standardization

## Context

The `status` field is currently treated as a required synced field by the `bob highlights-ref` marker/frontmatter
workflow, but it is only checked for presence. The requested supported status vocabulary is now:

- `unread`
- `wip`
- `done`
- `abandoned`
- `legacy`

The real Bob vault currently has 282 Markdown files under `~/bob/ref/ai/`. Every one has a frontmatter `status` field.
The current distribution is:

- `unread`: 144
- `collect_fleeting_notes`: 92
- `read`: 25
- `review_fleeting_notes`: 13
- `review_lit_notes`: 4
- `abandoned`: 4

The vault has unrelated pre-existing dirty files outside `ref/ai`. Those must be left untouched. The vault instructions
also require committing any `~/bob` file changes before finishing, using the SASE commit workflow.

## Scope

1. Enforce the allowed status vocabulary in Bob CLI ref-note sync code.
2. Update tests and docs so examples and fixtures use only supported statuses.
3. Migrate `~/bob/ref/ai/**/*.md` so every note has `status: "legacy"`.
4. Preserve each note's previous status in a separate frontmatter field named `legacy_status`.
5. Verify the code and the vault metadata sweep, then commit only the touched vault files.

Out of scope: bulk-mutating non-`ref/ai` vault notes. If other ref notes still use older statuses, report them rather
than silently rewriting them.

## Design

Add a single canonical status list in `src/native/highlights_ref/mod.rs` near the existing `FIELD_STATUS` constant.
Extend the required marker validation so `status` must be a scalar string and must exactly match one of the five
supported lowercase values. This preserves the existing missing/empty required field errors while adding a clear
unsupported-status error for values such as `queued`, `complete`, `read`, or `collect_fleeting_notes`.

Validation should run anywhere a synced projection can become authoritative:

- marker parsing, because PDF marker notes are user input;
- final selected/merged projection validation, because frontmatter edits can become the source of truth during
  write-back or conflict resolution.

Do not make `legacy_status` a synced marker field. It should remain ordinary frontmatter that is preserved by rendering
but not written back into PDF markers unless a marker explicitly opts into it as an unknown synced field.

For the vault migration, use a deterministic frontmatter-only rewrite over `~/bob/ref/ai/**/*.md`:

- read the current frontmatter status value;
- if `legacy_status` is absent, insert `legacy_status: <old status value>` immediately after the `status` line;
- replace the `status` line with `status: "legacy"`;
- leave all body content, ordering outside those lines, and unrelated files unchanged.

This is idempotent for the expected current state. After the first pass, a repeat pass should not overwrite the
preserved `legacy_status` value with `legacy`.

## Implementation Steps

1. Add the allowed status constant and validation helper in `src/native/highlights_ref/mod.rs`.
2. Wire status validation into existing projection validation without changing missing `status` or missing `parent`
   error behavior.
3. Update CLI/unit tests that currently use unsupported statuses (`queued`, `complete`, `marker-side`,
   `frontmatter-side`) to use supported statuses where success is expected.
4. Add explicit failure coverage for unsupported marker status and unsupported frontmatter status write-back.
5. Update README and `docs/highlights-ref-sync.md` to document the supported status vocabulary and the `legacy_status`
   preservation field.
6. Re-check `git -C ~/bob status --short`, then run the scoped vault metadata migration on `~/bob/ref/ai/**/*.md`.
7. Verify:
   - all `~/bob/ref/ai` statuses are `legacy`;
   - `legacy_status` distribution matches the pre-migration status distribution;
   - no files outside `~/bob/ref/ai` were changed by the migration.
8. Run focused tests around `highlights_ref`, then run the broader Rust test suite if time/runtime allows.
9. Commit only the vault migration files using the SASE git commit workflow, as required by the vault instructions.
   Leave unrelated pre-existing vault changes unstaged and untouched.

## Risks and Mitigations

- Existing non-`ref/ai` notes or PDFs may still contain older statuses. The new validator will reject them when they are
  used as synced ref-note input. I will report any known leftovers instead of broadening the migration without approval.
- A bulk rewrite can accidentally disturb body text. I will use a frontmatter pattern limited to the top `status` line
  and verify the resulting diff before committing.
- Obsidian Sync can introduce unrelated vault changes during the run. I will inspect vault Git status before and after
  the migration and stage only the touched `ref/ai` files for the required vault commit.

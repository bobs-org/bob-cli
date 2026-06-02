---
create_time: 2026-06-02 08:52:25
status: done
prompt: sdd/prompts/202606/done_tasks_frontmatter.md
---
# Plan: `bob collect-done` `done_tasks` frontmatter

## Goal

Extend `bob collect-done` so every source note that has, or is about to have, a corresponding archive note under `done/`
gets a `done_tasks` frontmatter property linking to that archive note.

Use the same archive mapping that already exists:

- `obsidian.md` -> `done/obsidian_done.md`
- `foo/bar.md` -> `done/foo/bar_done.md`

The source-note property should use an Obsidian wiki link with the vault-relative archive path and no `.md` extension,
matching the existing `parent: "[[done]]"` convention:

```yaml
done_tasks: "[[done/foo/bar_done]]"
```

## Current Findings

- `src/native/collect_done.rs` already has deterministic source-to-archive path mapping, archive frontmatter helpers,
  atomic writes, sync-before-write behavior, and path-scoped Git dirty checks/commits.
- The current implementation only adds `parent: "[[done]]"` to archive notes. It never adds metadata to the original
  source notes.
- A read-only check of `/home/bryan/bob` found no current `/home/bryan/bob/done` directory and no existing `done_tasks:`
  properties. Even so, the implementation must support backfilling source notes when `done/` archives already exist in a
  vault or fixture.

## Design

1. Keep archive path derivation as the single source of truth.
   - Reuse `archive_relative_path(source_relative_path)` for both archive writes and source-note links.
   - Add a helper that turns an archive path into an Obsidian wiki link by stripping `.md` and normalizing separators to
     `/`.

2. Add source frontmatter transformation.
   - Introduce a source-note helper, separate from archive `parent` handling, that ensures a top-level
     `done_tasks: "<link>"` line exists in the YAML frontmatter.
   - Preserve existing file contents, field order, line endings, and body text as much as the existing frontmatter
     helpers do.
   - If a note has no frontmatter, prepend frontmatter containing only `done_tasks`.
   - If malformed frontmatter has an opening marker without a closing marker, treat it like the archive helper does:
     prepend a valid frontmatter block rather than trying to rewrite ambiguous content.
   - If `done_tasks` already has the expected link, leave it unchanged.
   - If `done_tasks` exists with a stale scalar value, replace it with the expected link. This keeps the property as a
     single canonical link because each source note maps to one archive note.

3. Expand planning beyond task movement.
   - During `build_collection_plan`, scan every source markdown note that is already considered eligible for scanning
     and still exclude `done/`, `.git/`, and `.obsidian/`.
   - For each source note, compute its archive path and determine whether the archive exists or will be created by the
     current task collection pass.
   - Ensure `done_tasks` in the planned source contents whenever the corresponding archive exists or will exist.
   - Include metadata-only source updates in the plan even when the note has fewer completed/canceled task blocks than
     the threshold.
   - Do not create archive notes only for metadata. Archives should still be created only when task blocks are moved.

4. Preserve safety behavior.
   - Include metadata-only source paths in the touched-path set used by Git dirty checks.
   - Keep refusing pre-existing changes in any candidate source/archive file before mutation.
   - Stage, commit, and push only the source/archive paths touched by collection or `done_tasks` metadata updates.
   - Keep `ob sync --path <vault>` before scanning/writing.

5. Improve command output enough to explain metadata-only work.
   - Keep existing scan/move/git sections.
   - Add a count for source notes whose `done_tasks` property will be added or repaired.
   - Make the summary distinguish moved task blocks from source metadata updates, so a metadata-only run is not reported
     as "no vault changes."

6. Update documentation.
   - Document that `bob collect-done` links each original note back to its archive using `done_tasks`.
   - Document that existing `done/` archive notes are backfilled into source frontmatter on future runs.
   - Update the real-vault smoke-test wording to check both archive `parent` and source `done_tasks`.

## Test Plan

- Unit tests for archive link generation:
  - root source path maps to `[[done/obsidian_done]]`
  - nested source path maps to `[[done/foo/bar_done]]`

- Unit tests for source frontmatter:
  - add `done_tasks` to existing frontmatter
  - create frontmatter when source has none
  - replace stale `done_tasks`
  - leave an already-correct `done_tasks` unchanged
  - preserve CRLF line endings

- Planning tests:
  - collecting tasks creates/updates the archive and adds `done_tasks` to the source
  - an existing archive with no threshold-met tasks creates a metadata-only source update
  - an already-linked source with an existing archive is not planned as a write
  - a missing archive and no threshold-met tasks does not create a metadata-only change

- CLI/Git tests:
  - sync still runs before metadata-only source writes
  - a Git vault commits metadata-only `done_tasks` source updates
  - dirty candidate refusal covers metadata-only source candidates

- Validation commands:
  - `cargo fmt --check`
  - `cargo test collect_done`
  - `cargo clippy --all-targets --all-features`
  - `cargo test`
  - `just check-scripts`
  - `cargo run -- collect-done --help`

## Real Vault Check

Before any real `/home/bryan/bob` mutation, inspect `git -C /home/bryan/bob status --short`. The current read-only check
showed many dirty files and no `/home/bryan/bob/done` directory, so fixture coverage should prove the backfill behavior.
If a real-vault run is attempted later, the existing dirty-candidate safety should either commit only clean touched
paths or report the dirty source/archive candidates that prevent mutation.

---
create_time: 2026-06-02 09:14:01
status: wip
prompt: sdd/prompts/202606/done_archive_parent_type.md
---
# Plan: Move Done Archive Notes To Source Parent Links

## Goal

Change `bob collect-done` so every note under `~/bob/done/` that represents archived tasks points its `parent`
frontmatter property at the original source note, while a new `type` frontmatter property links the archive note to
`[[done]]`.

The desired shape is:

```yaml
---
parent: "[[obsidian]]"
type: "[[done]]"
---
```

For nested source notes, the parent link should preserve the vault-relative path without the `.md` extension, for
example `done/2026/20260528_day_done.md` should use:

```yaml
parent: "[[2026/20260528_day]]"
type: "[[done]]"
```

The source-note `done_tasks` behavior added previously should remain unchanged.

## Current State

- `src/native/collect_done.rs` currently hard-codes archive frontmatter as `parent: "[[done]]"`.
- `collect-done` already derives archive paths from source paths:
  - `obsidian.md` -> `done/obsidian_done.md`
  - `2026/20260528_day.md` -> `done/2026/20260528_day_done.md`
- The planner currently represents source metadata-only updates, but archive writes are tied to task movement
  (`task_count > 0`). This means existing archive notes are not repaired unless new tasks are appended.
- The existing real vault archive notes found under `~/bob/done/` are:
  - `done/gtd_daily_done.md`
  - `done/mac_inbox_done.md`
  - `done/needs_attn_tasks_done.md`
  - `done/obsidian_done.md`
  - `done/2026/20260528_day_done.md`
- Each of those has a matching source note in `~/bob/` and currently has `parent: "[[done]]"` with no `type` property.

## Implementation Approach

1. Add source-note wiki link generation.
   - Generalize the existing archive wiki-link helper into a vault-relative wiki-link helper, or add a parallel helper
     for source paths.
   - Keep validation strict: only vault-relative normal path components, valid UTF-8, no extension in the emitted link.
   - Expected examples:
     - `obsidian.md` -> `[[obsidian]]`
     - `foo/bar.md` -> `[[foo/bar]]`

2. Replace the static archive frontmatter line with source-aware metadata.
   - Generate archive `parent` from the source note link.
   - Generate archive `type` as `type: "[[done]]"`.
   - Use quoted wiki links for consistency with existing `parent` and `done_tasks` frontmatter output.
   - Prefer stable ordering for generated or inserted archive metadata: `parent` first, then `type`.

3. Update archive frontmatter repair logic.
   - For a new archive note, create frontmatter containing both `parent` and `type`.
   - For an existing archive note with frontmatter:
     - Replace any existing `parent:` line with the source-note link.
     - Replace any existing `type:` line with `[[done]]`.
     - Insert missing `parent` and/or `type` before the closing frontmatter marker.
     - Preserve unrelated frontmatter keys and existing line endings.
   - For an existing archive note with missing or malformed frontmatter, prepend valid archive frontmatter as the
     current implementation does.

4. Extend collection planning to include archive metadata-only updates.
   - `FilePlan` should distinguish:
     - source contents changed because task blocks moved,
     - source frontmatter changed because `done_tasks` was added or repaired,
     - archive contents changed because tasks were appended,
     - archive frontmatter changed because `parent` or `type` was repaired.
   - Do not rely on `task_count > 0` as the only signal that the archive file will be written.
   - When a matching archive exists but no tasks move, read it during planning, compute the repaired metadata, and
     include an archive metadata-only plan if the contents differ.
   - Continue to skip source files whose matching archive does not exist and whose task count is below the threshold.

5. Preserve Git safety and CLI reporting.
   - Include archive metadata-only paths in dirty-file checks before writing.
   - Stage archive metadata-only files when committing.
   - Keep source-only metadata commits possible.
   - Update output counts so users can see archive metadata repairs separately from archive task appends.
   - Keep existing behavior for non-Git vaults and Git vaults with unrelated dirty files.

6. Update docs and help text.
   - README `collect-done` section should describe archive notes as:
     - `parent` links to the original note,
     - `type` links to `[[done]]`,
     - existing archive notes are repaired/backfilled.
   - Update smoke-test wording to check both `parent: "[[source]]"` and `type: "[[done]]"`.
   - Adjust CLI/help summaries if they still imply only source links or archive-only writes.

7. Update tests.
   - Unit coverage:
     - new archive note frontmatter uses source parent plus done type,
     - stale `parent: "[[done]]"` is replaced with the source link,
     - missing `type` is inserted,
     - stale `type` is replaced,
     - already-correct archive metadata is unchanged,
     - CRLF preservation still works,
     - nested source paths produce nested parent links.
   - Planner coverage:
     - task-moving plans write the archive with source parent plus done type,
     - existing archive with stale metadata creates an archive metadata-only plan,
     - existing archive with correct metadata and correct source `done_tasks` is not planned,
     - missing archive below threshold is still ignored.
   - CLI/Git coverage:
     - created archives contain the new metadata,
     - metadata-only archive repairs are committed and staged,
     - dirty archive metadata candidates are refused before writes,
     - existing source `done_tasks` behavior still passes.

## Real Vault Migration

After implementation and automated validation pass, update the real vault as part of this task:

1. Confirm the vault state before writing:
   - `git -C ~/bob status --short`
   - inspect the current done-note frontmatter inventory with `rg -n "^(parent|type):" ~/bob/done -g '*.md'`
2. Run the updated command against `~/bob` through the repo binary path, allowing its normal `ob sync`, dirty-file
   checks, Git commit, and push behavior to run.
3. Verify all existing `~/bob/done/*.md` files now have the expected metadata:
   - `done/gtd_daily_done.md`: `parent: "[[gtd_daily]]"`, `type: "[[done]]"`
   - `done/mac_inbox_done.md`: `parent: "[[mac_inbox]]"`, `type: "[[done]]"`
   - `done/needs_attn_tasks_done.md`: `parent: "[[needs_attn_tasks]]"`, `type: "[[done]]"`
   - `done/obsidian_done.md`: `parent: "[[obsidian]]"`, `type: "[[done]]"`
   - `done/2026/20260528_day_done.md`: `parent: "[[2026/20260528_day]]"`, `type: "[[done]]"`
4. If any orphaned archive note appears during migration, stop and report it rather than inventing a parent link.

## Validation

Run these checks before reporting completion:

- `cargo fmt --check`
- `cargo test collect_done`
- `cargo clippy --all-targets --all-features`
- `cargo test`
- `just check-scripts`
- `cargo run -- collect-done --help`
- `git diff --check`
- real-vault verification commands from the migration section

## Risks And Mitigations

- Risk: Archive metadata-only repairs could bypass Git dirty checks.
  - Mitigation: make archive metadata changes first-class planned writes and include them in touched paths.
- Risk: Reusing the existing `task_count > 0` archive-write signal could skip migration.
  - Mitigation: introduce an explicit archive write/update signal based on computed archive contents.
- Risk: An archive note may not have a corresponding source note.
  - Mitigation: the source-driven planner naturally handles matched archives; explicitly inventory `~/bob/done` during
    real vault migration and stop on orphans.
- Risk: YAML formatting could become inconsistent.
  - Mitigation: continue using quoted wiki links and preserve existing line endings/frontmatter keys.

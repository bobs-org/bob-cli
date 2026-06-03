---
create_time: 2026-06-02 22:07:09
status: wip
prompt: sdd/prompts/202606/collect_done_block_id_dedup.md
---
# Plan: `bob collect-done` archive block ID de-duplication

## Goal

Make `bob collect-done` ensure that moved Obsidian block IDs are unique within the destination archive note under
`~/bob/done/`. When a moved block ID such as `^foobar` would collide with another block ID in that archive file, rewrite
that moved ID to `^foobar-<N>`, where `<N>` is the smallest positive integer that leaves the final destination file with
a unique block ID.

The uniqueness scope is the single archive file being written, for example `done/foo/bar_done.md`, not the entire vault
or the entire `done/` tree.

## Current Behavior

- `src/native/collect_done.rs::transform_markdown()` extracts completed and canceled `#task` blocks and appends them to
  the archive text without changing block IDs.
- It records moved block IDs in a set and records duplicates as `ambiguous_moved_block_ids`.
- `moved_block_targets()` skips ambiguous IDs, so links to duplicate moved IDs are not repaired.
- Existing archive contents are only considered for frontmatter and append behavior; they are not used to prevent block
  ID collisions.

This means `collect-done` can append a moved `^foobar` into `done/..._done.md` even when that file already contains
`^foobar`, or when multiple moved blocks contain the same `^foobar`.

## Design

1. Track block ID occurrences, not just sets.
   - Add a small internal occurrence type that records the original ID plus byte offsets in `archive_append`.
   - Keep using the current block ID parser semantics: only explicit standalone Obsidian block IDs like `^abc123` count;
     links such as `[[note#^abc123]]` inside task text should not reserve or rewrite an ID.
   - Preserve line endings and all task text other than the rewritten block ID token.

2. De-duplicate after the archive destination is known.
   - In `build_collection_plan()`, after reading `existing_archive` for the source's `relative_archive_path`, run a
     helper over the moved `archive_append`.
   - Seed the used-ID set from actual block IDs already present in the existing archive note.
   - Also account for IDs in the moved append text so a generated suffix does not steal a distinct ID that can otherwise
     remain unchanged in the same final archive.
   - Process moved ID occurrences in source order:
     - Keep an occurrence's original ID when it can remain unique in the final archive.
     - Otherwise choose the smallest `N >= 1` where `<original>-<N>` is not used or otherwise reserved for another kept
       moved ID in that archive.
     - Record the assignment from original ID to final ID.

3. Update archive text before frontmatter/link repair planning.
   - Replace IDs in the archive append text before calling `archive_contents()`.
   - Existing archive notes are never rewritten solely to rename old IDs; the requested behavior applies to blocks being
     moved into the archive.
   - Metadata-only archive repairs keep their current behavior.

4. Preserve correct link repair behavior.
   - Change moved block link targets from "source path + original ID -> archive path" to "source path + original ID ->
     archive path + final ID" for original IDs that have exactly one moved occurrence.
   - If a unique moved `^foobar` is renamed to `^foobar-1` because the archive already had `^foobar`, repair links to
     `done/...#^foobar-1`.
   - If multiple moved blocks originally share `^foobar`, keep treating incoming links to `source#^foobar` as ambiguous:
     the archive IDs will be unique, but existing references cannot identify which original duplicate they meant.

5. Update reporting.
   - Keep the existing "ambiguous moved block ids" metric for original IDs with multiple moved occurrences, because it
     still affects link repair safety.
   - Add or update a moved-block-ID rename count so runs that perform de-duplication are visible in command output.

6. Update documentation.
   - Extend the `bob collect-done` README section to state that moved block IDs are de-duplicated against their archive
     note by appending the smallest available `-<N>` suffix.
   - Document that link repair follows renamed unique IDs, while links to originally duplicated IDs remain conservative.

## Test Plan

Add focused unit coverage in `src/native/collect_done.rs`:

- Duplicate moved IDs in one source become unique archive IDs, for example `^dup` and `^dup-1`.
- Existing archive IDs are reserved, so moving `^dup` into an archive that already has `^dup` produces `^dup-1`.
- Suffix selection skips occupied candidates, for example existing `^dup` and `^dup-1` causes the moved ID to become
  `^dup-2`.
- Suffix selection preserves distinct moved IDs where possible, for example duplicate `^dup` plus an existing/moved
  `^dup-1` does not create two `^dup-1` IDs.
- CRLF task blocks keep CRLF line endings after ID rewriting.
- Link repair for a unique original ID uses the final renamed ID when an archive collision forced a suffix.
- Link repair for originally duplicated IDs remains skipped even though the archived block IDs are made unique.

Add at least one CLI/integration-style test in `tests/cli.rs`:

- Run `bob collect-done --threshold=1` against a fixture vault with a pre-existing `done/..._done.md` containing
  `^abc123` and a source task containing `^abc123`.
- Verify the archive contains the moved task with `^abc123-1`, the source task is removed, any unambiguous links are
  repaired to `done/...#^abc123-1`, and the commit includes the expected touched files.

Run:

- `cargo test collect_done`
- `cargo test --test cli collect_done`
- `cargo run -- collect-done --help`

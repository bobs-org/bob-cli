---
create_time: 2026-06-02 20:53:29
status: wip
prompt: sdd/prompts/202606/collect_done_obsidian_link_repair.md
---
# Plan: Repair Obsidian Links When `bob collect-done` Moves Tasks

## Goal

Extend `bob collect-done` so moving completed or canceled `#task` blocks into their `done/..._done.md` archive also
repairs Obsidian links that target the moved task blocks.

The core behavior should be:

- A task moved from `obsidian.md` to `done/obsidian_done.md` with block id `^a93ea3` should cause links like
  `[[obsidian#^a93ea3]]` and `![[obsidian#^a93ea3]]` to become `[[done/obsidian_done#^a93ea3]]` and
  `![[done/obsidian_done#^a93ea3]]`.
- Aliases should be preserved, e.g. `[[obsidian#^a93ea3|collect-done]]` should become
  `[[done/obsidian_done#^a93ea3|collect-done]]`.
- Links to tasks that are not moved in the current run should be left unchanged.
- Existing source/archive metadata behavior (`done_tasks`, archive `parent`, archive `type`) should continue to work.

## Current Findings

- `src/native/collect_done.rs` already has deterministic source-to-archive mapping:
  - `obsidian.md` -> `done/obsidian_done.md`
  - `foo/bar.md` -> `done/foo/bar_done.md`
- `build_collection_plan` currently produces per-source `FilePlan` entries with planned source and archive contents. Git
  dirty checks and staging are derived from those planned source/archive paths only.
- `transform_markdown` already identifies exactly which task blocks will move, but it only returns task count, remaining
  source contents, and archive append text. It does not retain moved block ids or source line/block context.
- The live vault uses Obsidian block links heavily. Current completed/canceled tasks include block ids like `^a93ea3`,
  `^inline-t-query`, `^a7b168`, `^gh-register`, `^prj-sibling-state`, and `^sase-var`.
- The live vault also has current links to those ids in the daily note, including normal wikilinks and embeds:
  - `[[obsidian#^a93ea3]]`
  - `![[obsidian#^a7b168]]`
  - `![[sase#^sase-var]]`
- The live vault currently has unrelated dirty notes (`2026/20260602_day.md`, `sase.md`). Since those are likely link
  repair/source candidates, the implementation must include link-repair files in pre-mutation dirty checks, not patch
  around the safety model.

## Design

1. Track moved block ids during task extraction.
   - Extend `Transform` to include a deterministic set of block ids found in the moved task blocks.
   - Extract ids from every line included in a moved block, not only the first task line, because Obsidian block ids can
     live on nested child lines or continuation lines.
   - Treat only explicit block ids as repairable anchors. Heading-only links or links to tasks with no `^block-id`
     cannot be repaired deterministically.

2. Build a moved-link target map.
   - For every source file whose task count meets the threshold, map `(source_relative_path, block_id)` to the archive
     wikilink target without the `.md` extension, such as `done/obsidian_done`.
   - Keep the existing archive-path helper as the source of truth so link repair cannot drift from collection behavior.
   - If duplicate block ids are found within the same moving source, keep the command conservative: report the ambiguity
     and avoid rewriting links for that ambiguous id rather than guessing.

3. Resolve Obsidian link targets safely.
   - Build a note index from vault markdown files so links can be resolved to vault-relative `.md` paths.
   - Resolve full-path targets exactly, e.g. `2026/20260602_day`.
   - Resolve basename-only targets, e.g. `obsidian`, only when the basename maps to exactly one note in the vault.
   - Resolve empty same-note targets such as `[[#^id]]` relative to the file being repaired.
   - Leave ambiguous or unresolved targets unchanged.

4. Repair wikilinks and embeds.
   - Parse `[[...]]` spans directly rather than doing broad regex replacement.
   - Preserve the leading embed marker when present (`![[...]]`).
   - Preserve aliases and display text after `|`.
   - Rewrite only links whose resolved note is a moving source and whose fragment is a moved block id.
   - Leave links that already point at the archive unchanged.

5. Optionally support Markdown inline links where the destination is unambiguous.
   - Support simple Obsidian-compatible Markdown block links such as `[text](obsidian.md#^a93ea3)` and
     `[text](#^a93ea3)`.
   - Preserve link text and any destination form that can be safely rewritten.
   - Leave complex Markdown destinations with titles, nested parentheses, or unresolved paths unchanged unless focused
     tests prove the parser handles them without collateral edits.

6. Merge link repair into the existing collection plan.
   - After the existing source/archive planning pass, build a final planned-content map keyed by vault-relative path.
   - Seed it with planned source/archive contents from `FilePlan`.
   - Scan vault markdown files for link repairs when there is at least one moved block id. Use the already-planned
     contents for paths that collection will also write; otherwise read the current file contents.
   - Include markdown files under `done/` in the link-repair scan because archive notes can contain links too. Continue
     excluding `.git/` and `.obsidian/`.
   - If a non-source/archive note changes only because links were repaired, add it as a first-class planned write.
   - Write each changed path once from the final planned-content map, so source/archive metadata updates and link
     repairs cannot overwrite each other.

7. Preserve Git safety and reporting.
   - Include link-only repair files in `touched_git_paths`.
   - Refuse to mutate if any source, archive, or link-repair candidate has pre-existing Git changes.
   - Stage and commit only the planned touched paths.
   - Add output counts for moved block ids discovered, Obsidian links repaired, and link-repair files updated.
   - Keep existing behavior for non-Git vaults, missing Git, unrelated dirty files, archive metadata-only repairs, and
     source `done_tasks` backfills.

8. Update documentation.
   - Update the `README.md` `collect-done` section to say the command repairs Obsidian links to moved task block ids.
   - Document that only explicit block-id links are repairable; tasks without block ids have no stable link target to
     rewrite.
   - Update smoke-test wording to check link repair in addition to archive metadata and source `done_tasks`.

## Test Plan

- Unit tests for block-id extraction:
  - task-line block id is collected,
  - nested/continuation block ids are collected,
  - block ids from below-threshold tasks do not trigger link repair,
  - duplicate moved ids in one source are treated conservatively.

- Unit tests for link parsing/rewriting:
  - `[[obsidian#^abc123]]` rewrites to `[[done/obsidian_done#^abc123]]`,
  - `![[obsidian#^abc123]]` preserves the embed marker,
  - `[[obsidian#^abc123|alias]]` preserves the alias,
  - `[[#^abc123]]` in the source note rewrites to the archive,
  - nested paths rewrite from `[[foo/bar#^abc123]]` to `[[done/foo/bar_done#^abc123]]`,
  - basename-only links rewrite only when the basename is unique,
  - heading links, unresolved links, non-moved block ids, and already-archive links are unchanged,
  - simple Markdown inline block links rewrite if implemented.

- Planner tests:
  - a moved task with a linked block id updates a separate note that links to it,
  - source/archive planned contents also go through link repair,
  - no link scan/write occurs when no task blocks move,
  - link-only repair files are included in planned touched paths,
  - existing `done_tasks` and archive metadata-only behavior still passes.

- CLI/Git tests:
  - a Git vault commits source, archive, and link-repair note together when all are clean,
  - a dirty link-repair note is refused before any source/archive mutation,
  - unrelated dirty files remain ignored,
  - cronjob still runs collect-done before sync and stages only the expected collect-done paths.

## Validation

Run the focused checks first:

- `cargo fmt --check`
- `cargo test collect_done`
- `cargo test collect_done_commits_and_pushes_collection_changes_only`
- `cargo test collect_done_refuses_dirty_candidate_files_before_mutation`

Then run the broader package checks:

- `cargo clippy --all-targets --all-features`
- `cargo test`
- `just check-scripts`
- `cargo run -- collect-done --help`
- `git diff --check`

For the live vault, do not force a mutation while relevant notes are dirty. First inspect:

- `git -C ~/bob status --short`
- a read-only inventory of current candidate task ids and links

If the dirty notes are still present, the expected live-vault behavior is a clean refusal before mutation. Once the
vault is clean, run the updated command against `~/bob` and verify that links in daily notes now point at
`done/..._done#^id` for task blocks that were actually moved.

## Risks And Mitigations

- Risk: broad text replacement corrupts unrelated wiki syntax.
  - Mitigation: parse link spans and rewrite only resolved block-id targets from the moved-target map.
- Risk: link-only files bypass dirty checks.
  - Mitigation: make link repairs first-class planned writes and feed them into touched-path calculation before
    mutation.
- Risk: basename-only links resolve to the wrong note.
  - Mitigation: rewrite basename links only when the note index proves they are unique.
- Risk: archive/source planned writes overwrite link repairs.
  - Mitigation: merge all planned contents by path and write each final path once.
- Risk: generated notes may contain links but be regenerated elsewhere.
  - Mitigation: treat markdown files as Obsidian notes for now; the Git dirty/staging model keeps the impact visible. If
    generated-file rewrites prove noisy, add an explicit documented exclusion later rather than silently skipping links.

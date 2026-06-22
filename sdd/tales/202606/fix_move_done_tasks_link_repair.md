---
create_time: 2026-06-22 08:11:19
status: done
prompt: sdd/prompts/202606/fix_move_done_tasks_link_repair.md
---
# Fix `bob move-done-tasks` block-link repair

## Problem / product context

`bob move-done-tasks` moves completed/canceled `#task` blocks out of the live notes in `~/bob/` into archive notes under
`~/bob/done/` (e.g. `sase.md` → `done/sase_done.md`). As part of the same run it is supposed to repair **every** block
link in `~/bob/` that pointed at a moved task, so that `[[sase#^auto-pair]]` style references keep resolving after the
block has been relocated.

In practice the repair is failing. `~/bob/2026/20260621.md` contains many links such as `[[sase#^auto-pair]]`,
`[[sase#^pretty-agent-restore]]`, `[[sase#^no-git-home]]`, `[[sase#^piw-xprompt-expand]]`, etc. that are now broken —
the target blocks live in `done/sase_done.md` but the links still point at `sase`. Roughly 17 files across the vault are
affected, and the breakage is silent (no error, the links are simply left untouched).

## Root cause analysis

There are **two distinct defects**, both in `src/native/collect_done.rs`.

### Defect 1 — generated tag pages create a false basename collision (the active bug)

The link-repair pass resolves a link's note target with `NoteIndex::resolve`. The `NoteIndex` is built from the set of
markdown files returned by `link_repair_markdown_files` → `collect_markdown_files` → filtered by `should_skip_directory`
(≈ lines 1627–1675).

`should_skip_directory` only skips `.git`, `.obsidian`, and (when collecting, not repairing) `done`. It does **not**
skip `_generated` or `_templates`. This diverges from the project-wide convention in
`src/native/projects.rs::is_excluded_directory` (≈ lines 2440–2449), which excludes all five: `.git`, `.obsidian`,
`_generated`, `_templates`, `done`.

Because `_generated` is scanned, the auto-generated tag page `_generated/tag_pages/topic/sase.md` is added to the index
alongside the real `sase.md`. `NoteIndex::from_paths` keys notes by basename and stores `None` whenever two paths share
a basename (≈ lines 1162–1189), and `NoteIndex::resolve` returns `None` for an ambiguous basename (≈ lines 1209–1215).
So `resolve(.., "sase")` returns `None`, `repair_wiki_link_inner` bails out, and the `[[sase#^…]]` links are silently
skipped.

This is **not** specific to `sase`: `_generated/tag_pages/topic/` mirrors essentially every topic note (`ads.md`,
`ai.md`, `api.md`, `code.md`, `day.md`, …), so link repair is broken for any note whose basename also has a generated
tag page. Confirmed on disk: both `./sase.md` and `./_generated/tag_pages/topic/sase.md` exist.

### Defect 2 — repair is "move-coupled", so pre-existing breakage is never healed

Even with Defect 1 fixed, the repair only targets blocks moved **in the current run**. `moved_block_targets` (≈ lines
1334–1358) builds its target map purely from `FilePlan`s with `task_count > 0` (this run's moves). The broken links in
`~/bob/2026/20260621.md` point at `sase` blocks that were archived in a **prior** run, so on any future run there is
nothing in the current-run target map for them and they stay broken. Fixing Defect 1 alone prevents _new_ breakage but
does not repair the ~17 already-broken files.

## Goals

1. Restore correct link repair for current-run moves (eliminate the false `_generated` ambiguity).
2. Heal **already-broken** block links left by prior runs, with no manual file editing.
3. Make repair **idempotent** and **self-correcting**: each run reconciles links against the actual archive/source
   state, so the same class of breakage cannot silently recur.
4. Preserve existing behavior: block-id de-duplication/rename handling, markdown + wiki link forms, embeds (`![[…]]`),
   aliases (`[[…|alias]]`), and git commit/push.

## Non-goals

- No change to which tasks are collected, the threshold logic, archive paths, or frontmatter (`parent:` / `type:` /
  `done_tasks:`).
- No change to the `done/` archive layout or naming (`<stem>_done.md`).
- Not adding shortest-path/Obsidian-default disambiguation for genuine collisions outside `_generated`/`_templates`
  (explicitly deferred — see decision Q2).

## Design

The two defects map to two changes. Both live in `src/native/collect_done.rs`.

### Part A — exclude generated/template directories from the scans (decision Q2)

Mirror `projects.rs::is_excluded_directory` in `collect_done.rs` so the directory walk also skips `_generated` and
`_templates`:

- Extend `should_skip_directory` so that, **regardless of `include_done`**, it skips `_generated` and `_templates` in
  addition to the existing `.git` / `.obsidian` (and the conditional `done`).
- To prevent future drift between the two modules, prefer factoring the always-excluded directory names (`.git`,
  `.obsidian`, `_generated`, `_templates`) into one shared constant/helper referenced by both `should_skip_directory`
  and `projects.rs::is_excluded_directory`. (A literal reuse of `is_excluded_directory` is not possible directly because
  it unconditionally excludes `done`, whereas the link-repair scan must _include_ `done`. The shared piece is only the
  always-excluded set.)

Effect: `_generated`/`_templates` notes no longer enter the `NoteIndex`, so the false `sase` ambiguity disappears and
`resolve(.., "sase")` returns the real `./sase.md`. It also stops these directories from being collected for tasks or
written to as repair targets (repairs there would be pointless — generated files are regenerated, templates are not real
notes).

This single change fixes Defect 1 and is sufficient for _current-run_ moves to repair correctly.

### Part B — self-healing target map derived from archive/source state (decision Q1)

Generalize the repair so the target map is computed from the **actual current state** of archives vs. sources each run,
instead of only from this run's moves.

**Core rule:** a link `[[source#^id]]` (or the markdown/embed equivalent) should be repaired to point at the archive
when `^id` is **absent from the source note** but **present in the archive note whose parent is that source** — i.e. the
block has been relocated, whenever that happened.

Concretely, replace the current `moved_block_targets` with a builder that produces the same `MovedBlockTargets` map
(`(source_relative_path, block_id) → MovedBlockTarget{archive_target, block_id}`) from these inputs:

1. **Enumerate archive notes.** Iterate the notes under `done/` already discovered by `link_repair_markdown_files` (it
   scans with `include_done = true`).
2. **Map each archive back to its source.** Reverse the `archive_relative_path` mapping (strip the `done/` prefix and
   the `_done` suffix from the stem: `done/foo/bar_done.md` → `foo/bar.md`). The archive's `parent:` frontmatter is the
   authoritative cross-check and can be used to validate the derived source.
3. **Diff block ids.** Using `block_ids_in_markdown` (already present, ≈ line 1805): for each block id present in the
   **archive body** but **not** in the **source body**, register `(source_path, id) → { archive_target, block_id: id }`.
   - Use **planned** contents for any file modified this run (post-move source and deduplicated archive from the
     `FilePlan`s) and **on-disk** contents otherwise. This unifies two cases under one rule: current-run moves (block
     now gone from the post-move source, present in the new archive) and pre-existing breakage (block absent from the
     on-disk source, present in the on-disk archive).
4. **Overlay current-run rename/ambiguity metadata.** Keep using the existing per-file `moved_block_id_final_ids` /
   `ambiguous_moved_block_ids` for **this run's** moves, layered on top of the diff result (taking precedence):
   - **Renames:** when a moved block's id collides in the archive it is renamed (`^dup` → `^dup-1`). The link still says
     `^dup`, but neither source nor archive now contains `^dup` — only `^dup-1`. The exact-id diff cannot recover this,
     so the current-run mapping `dup → dup-1` must be applied as `(source, dup) → archive#^dup-1`. This preserves the
     existing de-dup behavior.
   - **Ambiguous ids:** continue to skip ids the current run flagged ambiguous, matching today's conservative behavior
     (don't guess which duplicate a link meant).

The rest of the pipeline is unchanged: `repair_links_in_note` / `repair_wiki_links` / `repair_markdown_links` resolve
the link's note target via `NoteIndex::resolve` and look up `(resolved_path, id)` in this map. Because the map is the
same type and shape, no downstream changes are required.

**Why this is correct and safe:**

- **Heals existing breakage:** on the next run, `done/sase_done.md` contains `^auto-pair` etc. while `sase.md` no longer
  does, so the diff registers `(sase.md, auto-pair) → done/sase_done#^auto-pair`, and the `[[sase#^auto-pair]]` links in
  `2026/20260621.md` (and the other ~16 files) get repaired — even though nothing is moved that run.
  (`CollectionPlan::is_empty` already treats a non-empty `link_repairs` as work, lines 40–41, so a heal-only run
  proceeds and commits.)
- **Idempotent:** once repaired, the link points at `done/sase_done`, whose `resolve` no longer maps to the `sase.md`
  source key, so it is never touched again. A correctly-pointing link is likewise a no-op.
- **Conservative:** if `^id` still exists in the source, no target is registered, so live links are never rewritten.
  Only block ids that genuinely moved out of their source are healed.

**Known limitation (acceptable):** matching is by exact block id, which assumes ids are unique within a source note
(Obsidian's own assumption). A source note that legitimately reused the _same_ id on two blocks, both since archived,
can only be healed to the block that kept the original id. The current-run path already guards this via the
ambiguous-skip; pre-existing duplicates are a rare, out-of-scope edge case and will be left as-is rather than
mis-repaired.

### Output/reporting note

With self-healing, links repaired in a run can exceed the count of blocks moved that run. The plan summary printing (the
"moved block ids" / "renames" / link-repair-count lines, ≈ lines 290–419) should keep "moved block ids" tied to
current-run moves while the link-repair counts reflect total links healed. Verify the summary still reads sensibly for a
heal-only run (no moves, only repairs).

## Affected code (high level)

- `src/native/collect_done.rs`
  - `should_skip_directory` (Part A) — add `_generated` / `_templates`.
  - `moved_block_targets` → self-healing target builder (Part B); add archive→source derivation and a block-id diff over
    planned/on-disk contents; overlay current-run rename/ambiguity metadata.
  - Touch points only if needed: `apply_link_repairs_to_plan` (provide on-disk archive/source reads for files not in the
    current `FilePlan`s), summary printing.
- `src/native/projects.rs` — only if factoring the shared always-excluded directory set out of `is_excluded_directory`
  (optional, to avoid drift).

## Testing strategy

Add unit tests alongside the existing `#[cfg(test)]` suite in `collect_done.rs` (which already uses `TempDir` vault
fixtures and asserts on `build_collection_plan` output and repaired contents):

1. **`_generated` exclusion / no false ambiguity (Defect 1 regression):** vault with both `sase.md` and
   `_generated/tag_pages/topic/sase.md`, a moved `#task` block in `sase.md`, and a note linking `[[sase#^id]]`. Assert
   the link is repaired to `done/sase_done#^id` (would be skipped before the fix).
2. **`_generated`/`_templates` not collected or repaired:** a task block and a `[[note#^id]]` link placed inside
   `_generated`/`_templates` are left untouched and produce no archive.
3. **Self-healing of pre-existing breakage (Defect 2):** vault where `done/sase_done.md` already contains `^id` and
   `sase.md` does **not**, with a note linking `[[sase#^id]]`, and **no** new tasks to move this run. Assert the link is
   repaired and the run is not treated as empty.
4. **Idempotency:** run the plan twice; assert the second run produces no link repairs / no changes.
5. **Conservative no-op:** `^id` present in both source and archive → link is left unchanged.
6. **Rename overlay still works:** current-run move whose id collides in the archive (`^dup` → `^dup-1`) →
   `[[source#^dup]]` repaired to `…_done#^dup-1` (guard against regressing the existing dedup tests around lines
   2274–2337).
7. **Link forms:** wiki, markdown `[txt](note#^id)`, embeds `![[note#^id]]`, and aliases `[[note#^id|alias]]` all
   repaired; URLs untouched.

Run `cargo test` (and `cargo clippy`/`cargo fmt` per repo conventions) in the workspace's isolated environment.

## Verification on the real vault

After the change, run `bob move-done-tasks` against `~/bob/` and confirm:

- `~/bob/2026/20260621.md` links (`[[sase#^auto-pair]]`, `[[sase#^pretty-agent-restore]]`, `[[sase#^no-git-home]]`,
  `[[sase#^piw-xprompt-expand]]`, …) are rewritten to point at `done/sase_done` (or the correct archive) and resolve in
  Obsidian.
- The other affected files (~17 total) are healed in the same run.
- A second immediate run reports no further link repairs (idempotent).
- Review the git commit the command produces to confirm only intended link rewrites (and any genuine new moves) are
  included.

## Risks & mitigations

- **Over-eager rewrites:** mitigated by the "absent from source, present in archive" rule plus the ambiguous-id skip;
  covered by the conservative-no-op and idempotency tests.
- **Archive→source derivation mismatch:** path-reversal is deterministic and cross-checked against the archive `parent:`
  frontmatter; nested-source cases are already exercised by existing tests (`*_nested_source_parent`).
- **Cross-module drift on the excluded-directory set:** mitigated by sharing the constant/helper between
  `collect_done.rs` and `projects.rs`.

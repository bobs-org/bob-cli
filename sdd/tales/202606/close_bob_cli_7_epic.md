---
create_time: 2026-06-11 19:02:28
status: done
prompt: sdd/prompts/202606/close_bob_cli_7_epic.md
---
# Plan: Close out epic bead bob-cli-7 (`bob projects` command)

## Context / Verification Findings

Epic `bob-cli-7` ("bob projects command", plan `sdd/epics/202606/bob_projects_command.md`) has three closed child beads.
I verified each phase against the source code, git history, and the live vault:

- **Phase 1 (bob-cli-7.1, commit 6240085)** — `src/native/projects.rs` scanner + `bob projects list`, registration in
  `src/runner.rs`/`src/native.rs`, README/justfile smoke entries, and `tests/cli.rs` coverage all landed. ✅
- **Phase 2 (bob-cli-7.2, commit d6610c7)** — `bob projects sync` mutation engine, `docs/projects.md`,
  dry-run/warning/error handling, and CLI tests all landed. ✅
- **Phase 3 (bob-cli-7.3, commit 2ab7f3b)** — all vault edits are present in `~/bob`'s working tree (template `^prj`
  line, 9 legacy projects retired, `^prj` tasks on the 5 live projects + `bob_projects.md` verified, `project.md`
  contract updated, real sync run — `bob.md` carries `[scheduled::2026-06-11]`), **but the vault changes were never
  committed**. The Phase 3 agent's transcript explicitly states it skipped the `/sase_git_commit` step because the turn
  "did not explicitly request one". The epic plan's Phase 3 step 6 requires that commit.
- Child bead notes are `COMMIT:` markers only; all referenced commits exist and their work landed. ✅
- End-to-end validation passes today: `just all` green, `bob projects list` shows the 6 live projects,
  `bob projects sync --dry-run` reports 0 actions / 0 warnings (idempotent). ✅
- `just pyvision` does not exist in this repo's justfile (Rust project) — that step is N/A.
- The epic plan file `sdd/epics/202606/bob_projects_command.md` still has frontmatter `status: wip`.

## Remaining Work

### Step 1 — Commit the Phase 3 vault changes in `~/bob`

Use the `/sase_git_commit` skill (the only sanctioned commit path) against the `~/bob` repo, staging exactly the 17
Phase 3 files and nothing else from the dirty live vault:

- `_templates/new_project.md`
- Retired legacy projects (9): `balance_coupling.md`, `cat_theory_for_devs.md`, `clean_arch.md`,
  `how_to_read_a_book.md`, `soft_arch_hard_parts.md`, `think_fast_and_slow.md`, `outlive.md`, `prj_yserve.md`,
  `prj_zorg.md`
- Live projects with new `^prj` tasks (5): `bob.md`, `gkeep_gdocs_inbox_dump.md`, `sase_blog.md`, `sase_install.md`,
  `sase_version.md`
- `bob_projects.md` (prototype `^prj` normalized to canonical form)
- `project.md` (type-contract documentation)

Note: some of these files also contain Bryan's own intermingled live edits (new tasks added in Obsidian). File-level
staging is the granularity the epic plan prescribed ("commit only own files"); those co-resident edits are Bryan's own
vault content and are safe to include. Re-check `git -C ~/bob status` immediately before staging in case Obsidian Sync
moved anything.

Commit message should reference `bob-cli-7.3` (vault migration for the `bob projects` command).

### Step 2 — Update the epic plan file frontmatter

In `sdd/epics/202606/bob_projects_command.md`, change frontmatter `status: wip` → `status: done`.

### Step 3 — Close the epic bead (final step)

Run `sase bead close bob-cli-7` and read it back with `sase bead show bob-cli-7` to confirm the epic and all children
show closed. (`just pyvision` would run after this, but the target does not exist in this repo, so it is skipped.)

## Out of Scope

- Committing the bob-cli workspace changes (bead store + plan-file status) — left to the standard post-completion
  finalizer, per repo convention.
- Any further vault cleanup beyond the 17 Phase 3 files.

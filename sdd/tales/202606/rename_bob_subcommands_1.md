---
create_time: 2026-06-03 03:04:35
status: done
prompt: sdd/prompts/202606/rename_bob_subcommands_1.md
---
# Plan: Rename Bob task-moving and bulk commit subcommands

## Goal

Rename the two user-facing top-level `bob` subcommands:

- `bob collect-done` -> `bob move-done-tasks`
- `bob sync` -> `bob bulk-git-commit`

The rename should be complete for the current command surface: top-level help, per-command help, cronjob step names,
default commit messages, tests, justfile smokes, and README examples should use the new names. The nested Highlights
command `bob highlights-ref sync ...` is a different domain command and should not be renamed.

## Current shape

- Top-level subcommands are declared once in `src/runner.rs::SUBCOMMANDS` and dispatched through `NativeCommand` in
  `src/native.rs`.
- `collect-done` is native-only and implemented in `src/native/collect_done.rs`. Its public strings include
  `COMMAND_NAME`, help usage, stdout section title, error prefixes, and default Git commit subject
  `bob collect-done YYYY-MM-DD`.
- `sync` is implemented in `src/native/sync.rs`; despite the old name it now stages, commits, and pushes vault Git
  changes. The shared Obsidian sync gate lives in `cronjob`, so `bulk-git-commit` is a more accurate top-level name.
- `cronjob` calls both implementations directly and prints wrapped step names `collect-done` and `sync`.
- Legacy installed binaries and embedded script assets still include `bob_sync`. Those are compatibility shims, not
  top-level `bob` subcommands.
- Current docs/tests contain many references to the old names. Historical SDD plans and tales also reference them; those
  are archival records and should not be rewritten as part of this rename.

## Compatibility decisions

1. Remove the old top-level subcommand names. After this change, `bob collect-done` and `bob sync` should be unknown
   commands unless the user asks for transitional aliases.
2. Keep legacy installed binaries such as `bob_sync` intact. Existing external callers of `bob_sync` should continue to
   run the same native implementation through `run_legacy("bob_sync")`.
3. Rename native user-facing environment variables for the bulk commit command: add `BOB_BULK_GIT_COMMIT_LOCK_FILE` and
   `BOB_BULK_GIT_COMMIT_MESSAGE`.
4. Keep `BOB_SYNC_LOCK_FILE` and `BOB_SYNC_COMMIT_MESSAGE` as fallback aliases for compatibility, and document them as
   deprecated compatibility names.
5. Do not rename `OB_COMMAND`; it controls the real Obsidian `ob sync` command used by `cronjob` and by task-moving
   setup, not the old top-level `sync` subcommand specifically.

## Implementation

1. Update command registration and dispatch.
   - In `src/runner.rs`, replace `collect-done` with `move-done-tasks` and `sync` with `bulk-git-commit`, keeping
     `SUBCOMMANDS` alphabetically sorted.
   - Update command descriptions and top-level examples.
   - In `src/native.rs`, rename the relevant `NativeCommand` variants if that keeps the dispatcher clearer, while
     keeping the implementation modules focused on the existing behavior.

2. Update task-moving user-facing strings.
   - Change `COMMAND_NAME` to `bob move-done-tasks`.
   - Change the printed title from "Collect done tasks" to "Move done tasks".
   - Change default commit subject to `bob move-done-tasks YYYY-MM-DD`.
   - Keep internal helper names and fixture paths where they describe the existing algorithm and are not user-facing.

3. Update bulk Git commit user-facing strings and env handling.
   - Change default commit subject to `bob bulk-git-commit YYYY-MM-DD`.
   - Read `BOB_BULK_GIT_COMMIT_MESSAGE` first, then `BOB_SYNC_COMMIT_MESSAGE`.
   - In the shared lock helper, read `BOB_BULK_GIT_COMMIT_LOCK_FILE` first, then `BOB_SYNC_LOCK_FILE`.
   - Keep the legacy `bob_sync` binary, Cargo target, embedded script asset, and script syntax checks in place.

4. Update cronjob labels and comments.
   - Step names become `move-done-tasks` and `bulk-git-commit`.
   - Keep the shared gate label `obsidian-sync` / "Obsidian sync" because that refers to the actual `ob sync` operation,
     not the renamed bulk commit command.
   - Update cronjob tests to expect the new step names and commit subjects.

5. Update docs and developer conveniences.
   - README command list, examples, runtime dependency notes, environment section, migration notes, release checklist,
     and smoke-test snippets should use `move-done-tasks` and `bulk-git-commit`.
   - `justfile` install smoke should call `bob move-done-tasks --help`.
   - Current source docs/comments should be updated when they describe the top-level command surface; historical SDD
     records stay untouched.

6. Update tests.
   - Integration tests should invoke `move-done-tasks` and `bulk-git-commit`.
   - Assertions for commit subjects, help usage, top-level help ordering, and cronjob output should use the new names.
   - Add or update a focused compatibility assertion that `bob_sync` still works as a legacy binary path if existing
     coverage does not already prove that.
   - Add or update env-var tests for the new `BOB_BULK_GIT_COMMIT_*` names and old `BOB_SYNC_*` fallback aliases where
     practical.

## Verification

- `cargo fmt --check`
- Targeted tests for the renamed surfaces, for example:
  - `cargo test move_done_tasks`
  - `cargo test bulk_git_commit`
  - `cargo test cronjob`
  - `cargo test top_level_help`
- `cargo test`
- `cargo run -- move-done-tasks --help`
- `cargo run -- -h`
- Optionally verify old top-level names are rejected with exit code 2: `cargo run -- collect-done --help` and
  `cargo run -- sync --help`.
- `just check-scripts` if script/package files are touched or release-check confidence is needed.

## Risks

- External automation using `bob collect-done` or `bob sync` will need to update to the new names. Keeping the old
  top-level aliases would reduce breakage but would make the rename incomplete, so this plan removes them and documents
  the migration.
- `sync` appears in other meanings, especially `ob sync` and `highlights-ref sync`. Those should not be
  blanket-replaced.
- Environment-variable compatibility needs careful precedence so new names win without silently breaking existing
  cron/test setup that still exports `BOB_SYNC_*`.

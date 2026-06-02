---
create_time: 2026-06-02 07:44:23
status: wip
prompt: sdd/prompts/202606/collect_done.md
---
# Plan: `bob collect-done`

## Context

`bob-cli` is a native-first Rust CLI with top-level subcommands delegated from `src/runner.rs` into `src/native/*`.
Existing commands already establish useful patterns for `BOB_DIR`, opportunistic `ob sync`, test fixtures, and
integration tests.

Read-only inspection of `~/bob` shows that completed tasks use `- [x] ... #task` and canceled tasks use
`- [-] ... #task`; `- [/]` is in-progress and must not be collected. `~/bob` is already a git worktree and currently has
pre-existing dirty files, so this command must not use blanket `git add -A` for its own commit.

## Design Decisions

- Add `bob collect-done` as a native Rust command.
- Support `--threshold <N>` with default `10`.
- Use `BOB_DIR` for the vault path, defaulting to `~/bob`, matching existing commands.
- Exclude `done/` from the scan so archived notes are never reprocessed.
- Treat `[x]` and `[-]` checkbox task lines containing `#task` as collectible. Leave `[ ]`, `[/]`, and any other
  active/non-final statuses in place.
- Move a task block as the task line plus markdown list continuations: deeper indented sub-bullets, deeper indented
  continuation lines, and blank lines that belong to that list block. If a completed parent contains nested active
  items, move the whole subtree with the parent.
- Map source notes to archive notes as:
  - `foo/bar.md` -> `done/foo/bar_done.md`
  - `obsidian.md` -> `done/obsidian_done.md`
- Ensure every archive note has frontmatter with `parent: "[[done]]"`.
- Append collected blocks to the archive note in original order, preserving line endings and block text where practical.
- If `ob` is available, run `ob sync --path <vault>` before writing files. Missing `ob` is reported as a skipped sync,
  not an error. Sync failures other than the known "already running" case should stop before file mutation.
- If the vault is a git worktree, stage only the source and archive files touched by this command, commit, and push. If
  the vault is not a git worktree, warn and do not initialize git.
- When git is available and the vault is dirty before collection, protect commit quality by refusing to modify candidate
  source/archive files that are already dirty, unless a later implementation phase adds an explicit override. This
  avoids committing unrelated user edits from the same file.
- Output should be rich but script-friendly: clear sections for sync, scan, moves, git, and summary; stable text for
  tests; optional terminal styling only when stdout is a TTY.

## Phase 1: Command Surface and Registry

Goal: make `bob collect-done` a first-class native command without touching the vault.

Work:

- Add a `CollectDone` native command and `src/native/collect_done.rs`.
- Add `collect-done` to the top-level command table in alphabetical order.
- Because there is no legacy script implementation, adjust command dispatch so a subcommand can be native-only while
  existing script-backed commands keep their `BOB_CLI_USE_SCRIPT=1` fallback behavior.
- Implement argument parsing and help for `--threshold <N>`.
- Update top-level help examples and command ordering tests.

Validation:

- `cargo fmt --check`
- `cargo test top_level_help_lists_commands_alphabetically_with_examples`
- `bob collect-done --help`

## Phase 2: Markdown Scan and Move Engine

Goal: implement deterministic task discovery and in-memory transformations.

Work:

- Recursively scan `*.md` files under the vault, excluding `done/`, `.git/`, and `.obsidian/`.
- Implement task-line recognition for completed `[x]` and canceled `[-]` `#task` checkbox bullets.
- Implement block extraction that preserves associated sub-bullets and continuations.
- Select only files whose collectible task count is at least `--threshold`.
- Build transformed source contents and archive append contents in memory before any writes.
- Add focused unit tests and fixtures for nested blocks, canceled tasks, active tasks, in-progress tasks, root-level
  notes, nested-path notes, and threshold behavior.

Validation:

- `cargo test collect_done`
- `cargo test`

## Phase 3: Vault Writes, Frontmatter, and Sync

Goal: safely mutate the vault and produce useful progress output.

Work:

- Run `ob sync --path <vault>` before file writes when `ob` or `OB_COMMAND` is available; skip with an explicit output
  line when unavailable.
- Handle the known obsidian-headless "sync already running" message as a continue condition, matching existing command
  behavior.
- Create archive parent directories as needed.
- Create or update archive frontmatter with `parent: "[[done]]"`.
- Write changed source and archive files atomically enough for local use (write temp file in the same directory, then
  rename).
- Add output sections for sync, scan, per-file moves, and summary.
- Add integration tests with stubbed `ob` proving sync runs before writes, a missing `ob` skips cleanly, and a failing
  sync stops before mutation.

Validation:

- `cargo test collect_done`
- `cargo test`

## Phase 4: Git Commit and Push

Goal: commit and push exactly the collection changes when the vault is a git repo.

Work:

- Detect whether the vault is inside a git worktree with `git -C <vault> rev-parse --is-inside-work-tree`.
- If not a git repo, emit a warning and skip all git initialization, commit, and push work.
- If it is a git repo, record pre-run dirty paths before mutation.
- Refuse to mutate candidate source/archive files that are dirty before the command runs; this protects the command
  commit from absorbing unrelated edits.
- Stage only touched source/archive paths.
- Commit with a clear message such as `bob collect-done 2026-06-02`.
- Push after a successful commit.
- If no files changed after collection, do not commit or push.
- Add integration tests using a temporary git repo and bare remote to verify commit creation, push, non-repo warning,
  path-scoped staging, and dirty candidate refusal.

Validation:

- `cargo test collect_done`
- `cargo test`

## Phase 5: Documentation and End-to-End Verification

Goal: make the feature understandable and prove it works on the real vault.

Work:

- Update `README.md` with `bob collect-done`, `--threshold`, `BOB_DIR`, sync, and git behavior.
- Run the full release-quality check set:
  - `cargo fmt --check`
  - `cargo clippy --all-targets --all-features`
  - `cargo test`
  - `just check-scripts`
- Install or run the local binary in a way that exercises the new command.
- Run `bob collect-done` against `~/bob`.
- Verify the run:
  - archive notes were created under `~/bob/done/..._done.md`
  - source notes no longer contain the collected done/canceled task blocks
  - archive notes have `parent: "[[done]]"`
  - an appropriate git commit was created for touched paths
  - the commit was pushed
- Report any pre-existing dirty vault paths that prevented a file from being collected.

Acceptance Criteria:

- `bob collect-done` scans the vault, moves done/canceled task blocks from every note meeting the threshold, and leaves
  active tasks untouched.
- Archive paths and frontmatter match the requested Obsidian layout.
- `ob sync` runs before writes when available.
- Git behavior is safe for a dirty vault and does not initialize a non-repo.
- The final real-vault run creates and pushes a git commit when there are collection changes.

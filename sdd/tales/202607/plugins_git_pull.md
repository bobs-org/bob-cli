---
create_time: 2026-07-08 12:33:51
status: wip
prompt: sdd/prompts/202607/plugins_git_pull.md
---
# Plan: `git pull` the bob-plugins repo before `bob plugins list` / `sync`

## Problem

`bob plugins list` and `bob plugins sync` analyze which plugin files need updating by reading the **currently
checked-out** copy of the `bob-plugins` repo (`<repo>/plugins/<id>/`). When the local checkout is stale — for example a
plugin change was committed and pushed from another machine — the analysis and the sync are based on out-of-date source
files.

**Goal:** before either command determines what needs syncing, refresh the `bob-plugins` repo with `git pull` so the
analysis reflects the latest committed plugin files. The pull must be safe (never destroy local edits, never hang in
non-interactive/agent contexts) and must be opt-out-able.

## Where the code lives

- `src/native/plugins.rs` — the whole `bob plugins` command.
  - `run(args)` dispatches to `run_list` / `run_sync`.
  - `run_list` (`ArgMatches`) resolves `repo` + `bob_dir`, then calls `scan_plugins(&repo, &bob_dir)`.
  - `run_sync` (`ArgMatches`) builds `SyncOptions` (resolves `repo`, etc.), then calls `sync_plugins(&options)`.
  - `build_cli`, `list_command`, `sync_command`, and the per-arg builder functions (`repo_arg`, `bob_dir_arg`, …) define
    the CLI surface.
  - `repo_from_matches` resolves the repo path from `-r/--repo`, `BOB_PLUGINS_DIR`, or the default
    `~/projects/github/bobs-org/bob-plugins`.
  - `vault_file_is_dirty` is the existing precedent for shelling out to `git` and **degrading gracefully** when git/the
    repo is unavailable.
- `src/native/ob.rs` — reusable git helpers:
  - `git_command(path, child_env)` → `Command` preloaded with `git -C <path>` and the shared child environment (both
    `pub(crate)`).
  - `child_env()` → the non-interactive environment (ssh-agent vars + `GIT_SSH_COMMAND="ssh -o BatchMode=yes"` when
    unset), so remote git never blocks on an interactive SSH prompt. Also `pub(crate)`.
- `src/native/collect_done.rs` — precedent for detecting a git worktree (`git rev-parse --is-inside-work-tree`) and
  skipping git work with a warning when the target is not a worktree or git is missing.
- Docs: `README.md` (the `bob plugins` section, ~lines 210–242) and `docs/plugins.md` (the full command contract).
- Tests: `tests/cli.rs` (integration) and the `#[cfg(test)]` module in `src/native/plugins.rs` (unit).

## Design decisions

These are the product/behavior choices baked into this plan. They are called out here so they can be vetoed at review
time.

1. **Pull is on by default; `-n, --no-pull` opts out.** The user asked for pull to be the default behavior of both
   commands. A short-aliased opt-out (`bob plugins list --no-pull`, `bob plugins sync -n`) covers offline use,
   deliberately testing an un-pulled local checkout, and reproducing the old behavior. `-n` does not collide with any
   existing plugins short flag (`-B, -b, -d, -F, -f, -p, -r`).

2. **Pull runs once, up front**, right after the repo path is resolved and before `scan_plugins` / `sync_plugins`. Same
   placement in both `run_list` and `run_sync`.

3. **Non-git repo or missing git → silently skip the pull and continue.** This mirrors the existing
   `vault_file_is_dirty` philosophy ("no git state to act on → proceed") and, importantly, keeps every existing test
   green: the test suites point `-r` at plain temp directories that are not git repos, so the pull is a no-op there.
   Detection uses `git rev-parse --is-inside-work-tree` (as in `collect_done.rs`).

4. **Pull failure warns and continues — it never aborts the command and never, by itself, changes the exit code.** If
   `git pull` fails (diverged history, network down, auth failure, a conflicting local edit that git refuses to
   overwrite), we print a clear warning to stderr and fall back to analyzing the existing checkout. Rationale: matches
   the codebase's graceful-degradation style, and a stale analysis with a loud warning is a better failure mode than a
   hard error — especially for the normal editing workflow where the repo may hold uncommitted local plugin edits. (git
   itself refuses a pull that would clobber uncommitted changes, so local edits are never lost by this feature.)

5. **Pull diagnostics go to stderr; git's own stdout is captured, not inherited.** `list` supports `-f json`, whose
   stdout must stay pure JSON, and its table output is asserted by tests; routing pull status/warnings to stderr
   (prefixed `bob plugins: …`, matching the existing `issues` convention) keeps stdout clean in every mode. Capturing
   git's output (via `.output()`) prevents git from writing progress onto our stdout and prevents any interactive prompt
   on the inherited terminal.

6. **Non-interactive by construction.** Reuse `ob::git_command(&repo, &ob::child_env())` so an SSH pull runs with
   `BatchMode=yes`, and additionally set `GIT_TERMINAL_PROMPT=0` so git fails fast instead of hanging on a credential
   prompt. This matters because `bob` is frequently run by background agents. A pull that would need interaction fails
   cleanly → decision #4 warns and continues.

7. **Plain `git pull`.** Matches the user's explicit request ("run the `git pull` command"). See Alternatives for
   `--ff-only`.

## Implementation outline

All code changes are in `src/native/plugins.rs` unless noted.

### 1. Add the `--no-pull` flag

- New builder `no_pull_arg()`:
  - long `--no-pull`, short `-n`, `ArgAction::SetTrue`.
  - help: something like `Skip 'git pull' on the plugins repo before analyzing`.
- Register it in **alphabetical option order** (per the CLI rules memory) on all three command surfaces that resolve a
  repo:
  - `build_cli` (top-level `bob plugins`, which defaults to `list`): between `format_arg()` and `repo_arg()`.
  - `list_command`: between `format_arg()` and `repo_arg()`.
  - `sync_command`: between `force_arg()` and `plugin_arg()`.
- Helper `no_pull_from_matches(matches) -> bool` (or inline `matches.get_flag`).

### 2. Add the pull helper

A small, testable function, e.g.:

```
fn pull_repo(repo: &Path) -> PullOutcome
```

Behavior:

- If `repo` is not a git worktree, or `git` is not found → return `PullOutcome::Skipped` (no output).
- Otherwise run `ob::git_command(repo, &ob::child_env())` with args `["pull"]`, `.env("GIT_TERMINAL_PROMPT", "0")`,
  captured via `.output()`.
  - success → `PullOutcome::Pulled { summary }` where `summary` is a concise one-line status distilled from git's stdout
    (e.g. "Already up to date." vs. "Fast-forward" / files-changed line).
  - failure → `PullOutcome::Failed { message }` carrying git's stderr (trimmed).

A `PullOutcome` enum (mirroring the `FileAction` / `GitDetection` style already in the codebase) keeps this
unit-testable and lets the caller decide what to print. Reason worktree-detection lives here (not only in the caller):
it must be silent and must not perturb exit codes.

### 3. Call the helper from `run_list` and `run_sync`

- In `run_list`: after `let repo = repo_from_matches(matches);`, and gated on `!matches.get_flag("no-pull")`, call
  `pull_repo(&repo)` and print its outcome to **stderr** (`Pulled` → an info line only when something actually changed
  or always as a terse status; `Failed` → a `warning:`-style line). Then proceed to `scan_plugins` exactly as today.
- In `run_sync`: same, after `repo_from_matches`, before building the rest of `SyncOptions` / calling `sync_plugins`.
  Keep it consistent with `list`.
- The pull's own success/failure must **not** flip the command exit code (list/sync still exit non-zero only for their
  existing reasons). A `Failed` pull is a warning, matching how a skipped dirty file is a warning.

### 4. Help text & examples

- Ensure `-h/--help` stays excellent and options remain alphabetical (CLI rules memory). Optionally add a
  `bob plugins sync --no-pull` example to the relevant `after_help`.

### 5. Documentation

- `README.md` `bob plugins` section: update the usage synopses to include `[-n|--no-pull]` and add a sentence that both
  commands `git pull` the repo first (default on), skip gracefully when it is not a git repo, and warn + continue on
  failure.
- `docs/plugins.md`: add a short "Repo refresh" subsection documenting the default pull, the `-n, --no-pull` opt-out,
  the silent-skip on non-git repos, the warn-and-continue failure mode, and the non-interactive behavior. Add
  `-n, --no-pull` to the sync options list (alphabetical) and to the usage block(s).

## Testing strategy

### Preserve existing tests

- All current `tests/cli.rs` plugins tests and `plugins.rs` unit tests point at non-git temp dirs, so decision #3
  (silent skip) keeps them green without edits — **except** the help-order assertions, which must learn about the new
  flag.
- Update the three option-order assertions to include `-n, --no-pull` in the correct alphabetical slot:
  - `plugins_help_lists_subcommand_and_options` (list options order).
  - `plugins_sync_help_lists_options_alphabetically` (sync options order: …`-F, --force`, `-n, --no-pull`,
    `-p, --plugin`…).
  - Confirm the top-level `bob plugins --help` still passes.

### New tests

- **Unit test for `pull_repo`** (in `plugins.rs`, reusing the module's existing `run_git`/temp-dir helpers): create a
  bare "remote", clone it into `repo`, commit a new plugin file to the remote from a second clone, then assert
  `pull_repo(&repo)` fast-forwards `repo` (the new file is now present) and returns `Pulled`. Also assert a non-git temp
  dir returns `Skipped` with no side effects.
- **Integration test — pull refreshes before analysis**: stand up a remote+clone `repo`, advance the remote so a
  plugin's `main.js` changes, run `bob plugins list -r <repo> …` (default pull), and assert the reported SYNC state
  reflects the _pulled_ content, not the pre-pull checkout. A parallel `sync` variant asserts the vault receives the
  pulled bytes.
- **Integration test — `--no-pull` skips**: with the same fixture, run with `-n/--no-pull` and assert the pre-pull
  content is what gets analyzed (no fast-forward performed).
- **Integration test — non-git repo is silent**: reuse an existing-style non-git temp repo and assert default-pull runs
  produce no new stderr noise (i.e. the `stderr.is_empty()` guarantee for `plugins_list_renders_table_and_summary` still
  holds).
- **Integration test — JSON stays pure**: `bob plugins list -f json` against a git-repo fixture still parses as JSON
  (pull status went to stderr).

### Verification

- `cargo test` (unit + `tests/cli.rs`).
- `cargo clippy` / formatting per repo norms (see `justfile`).
- Manual smoke: run `bob plugins list` and `bob plugins sync --dry-run` against the real repo to confirm the pull line
  renders sensibly and a forced offline state warns-and-continues.

## Risks & mitigations

- **Breaking stdout consumers of `list -f json`.** Mitigated by routing all pull output to stderr and capturing git's
  stdout (decision #5); covered by the JSON-purity test.
- **Hanging in agent/cron contexts on auth.** Mitigated by `BatchMode=yes` + `GIT_TERMINAL_PROMPT=0` and output capture
  (decision #6).
- **Destroying in-progress local plugin edits.** Not possible: `git pull` refuses to overwrite conflicting uncommitted
  changes; on that refusal we warn and continue (decision #4), and the user's local edits still drive the sync.
- **Test fixtures pointing at non-git dirs suddenly erroring.** Mitigated by the silent-skip path (decision #3) and an
  explicit regression test.

## Alternatives considered

- **`git pull --ff-only`** instead of a plain pull — safer for a source-of-truth repo (no surprise merge commits; clean
  failure on divergence). Rejected as the default to match the user's explicit wording, but it is a drop-in if
  preferred; with warn-and-continue, an `--ff-only` failure on divergence would simply warn.
- **`git fetch` only** (refresh remote-tracking refs without moving the worktree) — does not update the files that are
  actually analyzed, so it fails the goal.
- **Abort the command on pull failure** — rejected as too aggressive for the editing workflow and inconsistent with the
  codebase's graceful degradation.
- **Env-var global opt-out (e.g. `BOB_PLUGINS_NO_PULL`)** — out of scope; the `--no-pull` flag is sufficient. Easy to
  add later if wanted.

## Open questions for review

1. Confirm plain `git pull` vs. `git pull --ff-only` (see Alternatives).
2. Confirm pull status belongs on **stderr** (keeps `list` stdout pure for JSON and table). Alternative: print it on
   stdout for `sync` only, since `sync` has no machine-readable mode.
3. Confirm the short alias `-n` for `--no-pull` is acceptable.

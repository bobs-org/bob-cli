---
create_time: 2026-06-02 18:46:07
status: done
prompt: sdd/prompts/202606/bob_cronjob.md
---
# Plan: `bob cronjob` — the single nightly entry point

## Goal

Introduce one new subcommand, `bob cronjob`, that becomes the **only** thing the user's crontab runs. It performs the
once-nightly Obsidian sync up front, then delegates to a sequence of "wrapped commands" (today: `collect-done` and
`sync`; more to come). The wrapped commands keep ownership of their own git commits / pushes, but no longer run
`ob sync` themselves — that responsibility moves up to `cronjob`. The run produces beautiful, clearly-sectioned output
so it is always obvious which wrapped step is executing.

This plan reflects the four answered design questions:

- **Q1 — Standalone sync:** Remove `ob sync` from the wrapped commands entirely. `ob sync` runs **only** inside
  `bob cronjob`. Standalone `bob sync` becomes commit+push of the vault; standalone `bob collect-done` just
  archives+commits.
- **Q2 — Failure mode:** A failed **shared `ob sync` aborts the whole run** (we should not commit on top of a vault we
  could not sync). After that gate, each wrapped step runs even if a prior one failed; `cronjob` exits non-zero if any
  step failed, with a clear pass/fail summary.
- **Q3 — Run order:** `collect-done` **then** `sync` (archive first so the sync push includes that night's archive edits
  — mirrors today's 3:25 / 3:30 split).
- **Lead on design:** A reusable "step runner" presentation so every wrapped command is announced with a labeled section
  and a ✓/✗ result, and adding the next wrapped command in the future is a one-line registration.

## Background — how the code looks today

- Subcommands are declared in a `SUBCOMMANDS` table in `src/runner.rs` and dispatched through `NativeCommand` in
  `src/native.rs` to a module under `src/native/`. A unit test enforces that the table stays alphabetically sorted.
- `src/native/sync.rs::run()` does: acquire an exclusive lock → discover the `ob` binary (PATH, else NVM) → verify the
  vault is a git worktree → **`ob sync`** → **`ob sync-status`** → `git add -A .` → commit (`bob sync <date>`) →
  `git push`. It carries a `Context { ob_command, child_env }` where `child_env` injects the ssh-agent environment and
  `GIT_SSH_COMMAND="ssh -o BatchMode=yes"` so pushes are non-interactive.
- `src/native/collect_done.rs::run_collect_done()` does: **`ob sync`** (its own second copy, discovered via the
  `OB_COMMAND` env var, gracefully skipped if missing) → scan/plan → write archive+source files → stage **specific
  paths** → commit (`bob collect-done <date>`) → `git push`. Its `git_command()` does **not** inject the ssh-agent /
  BatchMode env.
- So `ob sync` is implemented **twice** with divergent `ob` discovery and divergent git environments. Consolidating it
  is a natural part of this work.

## Design

### 1. New shared Obsidian module: `src/native/ob.rs`

Extract the duplicated `ob sync` logic into one place that `cronjob` calls once.

- `pub(crate) fn sync_vault(vault: &Path) -> Result<SyncOutcome, i32>` — runs `ob sync --path <vault>` and then
  `ob sync-status --path <vault>`, returning `Ran` / `SkippedMissingCommand` / `AlreadyRunning` (treating "already
  running" and "ob not found" as non-fatal, matching today's behavior).
- Move `ob` discovery here and make it **honor `OB_COMMAND` first** (used by the test suite), then PATH, then the NVM
  fallback — unifying the two discovery paths that exist today.
- Move the shared **git environment** here too: `pub(crate) fn child_env()` returning the ssh-agent vars +
  `GIT_SSH_COMMAND=ssh -o BatchMode=yes`, plus a `git_command(vault, &child_env)` helper. This gives _both_ wrapped
  commands the non-interactive push environment under cron (collect-done lacks it today).

### 2. Make the wrapped commands sync-free and callable as library functions

The user wants `cronjob` to **reuse the Rust logic** of the wrapped commands, not shell out to `bob sync` /
`bob collect-done`. So each wrapped command exposes a core entry point that does its real work and returns a result,
while its public `run()` stays a thin standalone wrapper.

- **`src/native/sync.rs`**
  - Remove `run_ob_sync` and the `ob sync-status` call from this module.
  - Standalone `run()` = acquire lock → verify worktree → `git add -A .` → commit-if-changed → push. (Commit message
    override `BOB_SYNC_COMMIT_MESSAGE` preserved.)
  - Factor the worktree+git body into `pub(crate) fn commit_and_push_vault(...)` that takes the shared `child_env` and
    does **not** acquire the lock, so `cronjob` (which already holds the lock) can call it directly.
- **`src/native/collect_done.rs`**
  - Remove its `run_ob_sync` copy and `SyncOutcome` plumbing from the collection flow; drop the `sync:` section from its
    standalone output.
  - Standalone `run()` = parse args → run the collection.
  - Factor the collection body into `pub(crate) fn run_collection(threshold, &child_env) -> i32` (using the shared
    `git_command` so its push inherits the BatchMode env) that `cronjob` calls.

### 3. New orchestrator: `src/native/cronjob.rs`

`pub(crate) fn run(args) -> i32` that:

1. **Acquires the exclusive lock once** for the whole nightly run (move the lock helper into `ob.rs` or a small shared
   spot; standalone `sync` keeps using it, `cronjob` holds it across all steps).
2. Builds the shared `child_env` once.
3. **Gate — shared Obsidian sync:** prints the sync section, calls `ob::sync_vault`. On `Err`, prints the abort summary
   and **exits non-zero without running any wrapped step** (Q2).
4. **Runs the wrapped steps in order** (Q3: collect-done, then sync) via a small **step registry** — a slice of
   `{ name, blurb, run: fn(&ChildEnv) -> i32 }`. Each step is announced with its own section, runs to completion, and
   its exit code is recorded. A failing step does **not** stop later steps.
5. **Prints the summary** and returns non-zero if the gate or any step failed (specifically: exit code = first non-zero
   among the steps, else 0).

Adding a future wrapped command = appending one entry to the step registry + writing its `run_*` core function. The
orchestrator code does not change.

### 4. Output design (the "make it beautiful" part)

A reusable presentation layer in `cronjob.rs` (kept dependency-free — just `println!` + a tiny ANSI helper). Because
this runs under cron with output redirected to a log file, **color is auto-disabled when stdout is not a TTY** (and
honors `NO_COLOR`); structure comes from box-drawing/rule lines and ✓/✗ markers so the log stays readable and the live
terminal looks great. Each step's own stdout is shown inside its section so you always know who is talking.

Illustrative shape:

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  bob cronjob · 2026-06-02T03:30:01
  Nightly maintenance for the Bob Obsidian vault — /home/bryan/bob
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

▸ Obsidian sync (shared, runs once)
  …ob sync output…
  ✓ vault synced

╭─ step 1/2 · collect-done ─ Archive done & canceled tasks ──────────
│  …collect-done output…
╰─ ✓ collect-done ok

╭─ step 2/2 · sync ─ Commit and push the vault ─────────────────────
│  …sync output…
╰─ ✓ sync ok

━━ Summary ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  ✓ obsidian-sync   ok
  ✓ collect-done    ok
  ✓ sync            ok
  All steps passed · 2026-06-02T03:30:05
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

On failure the relevant marker becomes `✗`, the summary lists which step(s) failed (with exit codes), and `cronjob`
returns non-zero.

### 5. Register the subcommand

- Add `NativeCommand::Cronjob` to `src/native.rs` and wire it in `run()`.
- Add a `cronjob` entry to the `SUBCOMMANDS` table in `src/runner.rs` with `script_command: None` (no legacy shell
  script, like `collect-done`). Alphabetical position: **between `collect-done` and `notify`**, satisfying the
  sorted-subcommands invariant.
- Add an example line to the top-level help `AFTER_HELP`.

### 6. Update the crontab

Replace the two `bob` lines with a single nightly entry at 3:30 AM. This edits the **live user crontab** (not a repo
file); I will show the exact before/after diff and apply it with `crontab` only after plan approval, since you
explicitly asked for it.

Remove:

```
30 3 * * * … bob sync     >> /var/tmp/bob_sync.log 2>&1
25 3 * * * … bob collect-done >> /var/tmp/bob_collect_done.log 2>&1
```

Add (keeps the profile + NVM sourcing so `ob`/node resolve under cron):

```
30 3 * * * bash -c ". $HOME/.profile; . $HOME/.config/nvm/nvm.sh; nvm use --silent default; bob cronjob" >> /var/tmp/bob_cronjob.log 2>&1
```

## Testing

Follow the existing `tests/cli.rs` patterns (stub `ob` via `OB_COMMAND`, real temp git repo with a bare remote via
`init_git_vault_with_remote`, `BOB_SYNC_LOCK_FILE` to isolate the lock):

- **New `cronjob` tests:** happy path runs sync once then both wrapped steps in order, commits land from both, summary
  shows all ✓; failing `ob sync` aborts before any wrapped step (no commits); a failing wrapped step still lets the
  later step run and yields a non-zero exit with a ✗ in the summary; output is plain (no ANSI) when not a TTY.
- **Update existing tests:** the standalone-sync test (`bob_sync_uses_stubbed_ob_and_git_commands`) no longer expects
  `ob sync` / `ob sync-status`; the `collect_done_runs_sync_before_*` and
  `collect_done_failing_sync_stops_before_mutation` / `collect_done_skips_missing_ob_command_*` tests move their
  `ob sync` assertions to the `cronjob` suite (standalone collect-done no longer syncs).
- Keep the `subcommands_are_sorted_alphabetically` and `build_cli` tests green.

## Out of scope / notes

- No new crate dependencies (ANSI + TTY detection done with a tiny local helper / `std`).
- The legacy embedded shell scripts (`bob_sync`, etc.) and the `BOB_CLI_USE_SCRIPT` fallback are left untouched;
  `cronjob` is native-only.
- Behavior of `ob sync` "already running" / "ob missing" stays non-fatal, just centralized.

## Step-by-step implementation order

1. Add `src/native/ob.rs` (shared `sync_vault`, `ob` discovery incl. `OB_COMMAND`, `child_env`, `git_command`, lock
   helper).
2. Refactor `sync.rs` to drop `ob sync` and expose `commit_and_push_vault`.
3. Refactor `collect_done.rs` to drop `ob sync` and expose `run_collection`.
4. Add `src/native/cronjob.rs` (orchestrator + output framework + step registry).
5. Register `cronjob` in `native.rs` and `runner.rs` (+ help example).
6. Update / add tests; run `cargo test` and `cargo clippy`.
7. Update the crontab (show diff, apply after approval).

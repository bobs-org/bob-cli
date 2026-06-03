---
create_time: 2026-06-03 05:18:31
status: done
prompt: sdd/prompts/202606/cli_rules_conformance.md
---
# Plan: Bring Bob CLI Help And Ordering Into CLI Rules Conformance

## Context

The new long-term CLI rules require that when CLI subcommands or options are added or changed:

- `-h|--help` output must be clear, complete, consistent, and easy to scan.
- Listed subcommands and options must stay alphabetically sorted.
- Color should be used where it improves readability, while keeping non-TTY output clean.

This repository is a Rust CLI crate with:

- Preferred entry point: `bob <subcommand>`.
- Legacy installed binaries: `bob_pomodoro`, `bob_notify`, `bob_sync`, and `tmux_bob_pomodoro`.
- Embedded shell fallback activated by `BOB_CLI_USE_SCRIPT=1`.

The current top-level `bob -h` path is already improved and guarded by tests, but several command-level paths do not yet
meet the new standard.

## Findings From Inspection

Confirmed good baseline:

- `src/runner.rs` keeps top-level `SUBCOMMANDS` alphabetically sorted.
- Existing unit tests guard top-level command ordering and Clap construction.
- Existing integration tests cover top-level help, several native-only help paths, and ANSI-free captured help output.
- `bob cronjob` already uses TTY-aware color for operational output, disabling ANSI in non-TTY output and with
  `NO_COLOR`.

Conformance gaps to fix:

- `bob pomodoro --help` and legacy `bob_pomodoro --help` currently exit with an unexpected-argument error.
- `bob tmux-pomodoro --help` and legacy `tmux_bob_pomodoro --help` currently exit successfully with no help text.
- `bob cronjob --help` is not intercepted by the native command, so the delegated command can run maintenance work
  instead of showing help.
- `BOB_CLI_USE_SCRIPT=1` fallback scripts do not consistently make `--help` safe for `bob_pomodoro`,
  `tmux_bob_pomodoro`, and `bob_sync`.
- `bob highlights-ref --help` lists nested commands in declaration order: `scan`, `sync`, `doctor`, `marker`, not
  alphabetical order.
- `bob highlights-ref sync --help` lists user-defined options in mixed order because command-specific options and shared
  config options are appended in separate groups.
- README and `just install-smoke` only smoke-test a subset of safe help paths, so they would not catch the above gaps.

## Implementation Strategy

Keep this as a CLI-surface conformance pass. Do not change vault business logic, command defaults, file transforms, Git
behavior, or cron scheduling semantics except where necessary to make `--help` exit before side effects.

### 1. Define And Enforce The Help Contract

Adopt the following contract in tests and implementation:

- Every installed command surface supports `-h` and `--help` and exits `0`.
- Help paths perform no vault, Git, Obsidian, notification, or sleep work.
- Captured help output contains no ANSI escape bytes.
- TTY output may use color through existing TTY-aware mechanisms.
- Top-level and nested subcommand lists are alphabetical; Clap's synthetic `help` command may remain in its conventional
  generated position.
- User-defined options are listed alphabetically by long option name where the command owns their ordering. Default
  Clap-generated `-h, --help` can remain in Clap's generated position unless a command switches to explicit help flags.

### 2. Normalize Native Help Paths

Add explicit help handling before operational work for native commands that currently lack it:

- `cronjob`: parse `-h|--help` and print a concise help page describing the nightly shared `ob sync` gate, wrapped
  steps, key environment variables, and absence of options. Reject other unexpected args with a normal usage hint.
- `pomodoro`: support `-h|--help`, document `-d|--debug`, `-v|--verbose`, `BOB_DAY_FILE`, `BOB_DIR`, and `BOB_NOW`, and
  keep existing debug/verbose behavior.
- `tmux-pomodoro`: support `-h|--help`, document tmux status-line output and relevant Pomodoro environment variables,
  and reject unexpected args instead of silently ignoring them for the preferred `bob tmux-pomodoro` surface.

Review existing native help pages for:

- `bulk-git-commit`
- `move-done-tasks`
- `notify`
- `highlights-ref` and its nested commands

Keep their behavioral parsing stable, but adjust wording/order where needed for consistency and completeness.

### 3. Fix Alphabetical Ordering In `highlights-ref`

Reorder nested `highlights-ref` subcommands alphabetically in the Clap builder:

- `doctor`
- `marker`
- `scan`
- `sync`

Rework shared config option registration so each nested help screen lists user-defined options alphabetically by long
flag. The intended visible order is:

- Config-only commands: `--bob-dir`, `--default-parent`, `--lib-dir`, `--ref-dir`
- `scan`: `--bob-dir`, `--default-parent`, `--dry-run`, `--lib-dir`, `--ref-dir`
- `sync`: `--bob-dir`, `--default-parent`, `--dry-run`, `--lib-dir`, `--prefer`, `--ref-dir`, `--write-pdf`

Keep the `PDF` positional argument in the `Arguments` section, not mixed into option sorting.

### 4. Align Legacy Binaries And Script Fallback

Because installed legacy binaries are still public command surfaces:

- Ensure native legacy binaries inherit the same safe help behavior through `run_legacy`.
- Update embedded fallback scripts so `BOB_CLI_USE_SCRIPT=1` is also safe:
  - `scripts/bob_pomodoro`: add `-h|--help` help output matching the native command closely.
  - `scripts/tmux_bob_pomodoro`: add `-h|--help` help output and reject unexpected args.
  - `scripts/bob_sync`: add `-h|--help` help output before lock, `ob`, Git, or SSH work.
- Preserve `scripts/bob_notify` behavior unless a small wording/order cleanup is needed to match native help.

### 5. Add Regression Tests

Add focused integration tests in `tests/cli.rs` for help safety and ordering:

- `bob <subcommand> --help` exits `0`, prints a command-specific usage/help marker, and emits no ANSI escapes for all
  top-level subcommands.
- Legacy binaries `bob_pomodoro --help`, `bob_notify --help`, `bob_sync --help`, and `tmux_bob_pomodoro --help` exit `0`
  and print help.
- `BOB_CLI_USE_SCRIPT=1` fallback help is safe for the commands that can delegate to scripts, especially `pomodoro`,
  `notify`, and `tmux-pomodoro`.
- `bob highlights-ref --help` lists nested commands alphabetically.
- Representative nested help, especially `bob highlights-ref sync --help`, lists options alphabetically.
- `bob cronjob --help` exits before operational work. Use a minimal environment/stub setup if needed so the test would
  fail if cronjob attempted `ob`, Git, or vault mutation.

Keep assertions structural and substring/order based, not full snapshots, to avoid brittle formatting tests.

### 6. Update Smoke Checks And Documentation

Update `just install-smoke` and the README release smoke snippets to include safe help checks for the full public
surface:

- `bob --help`
- `bob bulk-git-commit --help`
- `bob cronjob --help`
- `bob highlights-ref --help`
- `bob move-done-tasks --help`
- `bob notify --help`
- `bob pomodoro --help`
- `bob tmux-pomodoro --help`
- Legacy binary help checks where useful

Do not add smoke commands that perform vault mutation.

### 7. Validation

After implementation, run:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features`
- `cargo test`
- `just check-scripts`
- `just install-smoke`

Also manually inspect representative help output:

- `bob --help`
- `bob cronjob --help`
- `bob pomodoro --help`
- `bob tmux-pomodoro --help`
- `bob highlights-ref --help`
- `bob highlights-ref sync --help`

## Acceptance Criteria

- Every public command and legacy binary has safe, useful `-h|--help` output.
- `--help` never runs cronjob, Git, Obsidian sync, notification, sleep, or vault mutation logic.
- Top-level and `highlights-ref` subcommand listings are alphabetical.
- User-defined options in help output are alphabetically ordered where this code controls their display.
- Captured help output remains ANSI-free; TTY-aware color remains available where already useful.
- Tests and install smoke checks guard the standards going forward.

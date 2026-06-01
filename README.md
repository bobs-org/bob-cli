# Bob CLI

`bob-cli` installs the `bob` command and compatibility shims for the Bob Obsidian
vault workflow. The command implementations are native Rust by default. The
earlier Bash and Python implementations remain embedded as a rollback path:
set `BOB_CLI_USE_SCRIPT=1` to extract those scripts into `XDG_CACHE_HOME` and
delegate to them.

The preferred interface is `bob <subcommand>`. Legacy command names still exist
as installed binaries for existing tmux, shell, and automation callers.

## Installation

For local development from this checkout:

```bash
cargo install --path . --locked --force
```

For installation from the Git remote:

```bash
cargo install --git git@github.com:bbugyi200/bob-cli.git --locked --force bob-cli
```

To smoke-test an install without replacing an existing user install:

```bash
root="$(mktemp -d)"
cargo install --path . --locked --root "$root"
"$root/bin/bob" pomodoro-runtimes --help
BOB_DAY_FILE=/tmp/bob-cli-missing-day.md "$root/bin/bob" pomodoro
BOB_DAY_FILE=/tmp/bob-cli-missing-day.md "$root/bin/bob" tmux-pomodoro
"$root/bin/bob_notify" --help
```

## Commands

```bash
bob pomodoro
```

Prints the current Pomodoro ledger entry from today's Bob daily note, including
time remaining or recent overdue status. It defaults to
`$BOB_DIR/YYYY/YYYYMMDD_day.md`, or `~/bob/YYYY/YYYYMMDD_day.md` when `BOB_DIR`
is unset, unless `BOB_DAY_FILE` is set.

```bash
bob pomodoro-runtimes [--check] [NOTE ...]
```

Runs `ob sync`, then annotates completed Pomodoro ledger entries with runtime
suffixes. With no `NOTE` arguments it uses today's Bob daily note. `--check`
reports pending changes without writing them.

```bash
bob notify PRE_CHECK_SLEEP POST_NOTIFY_SLEEP
```

Polls `bob_pomodoro` until the current Pomodoro is overdue, then sends a desktop
notification when `notify-send` is available and rings the terminal bell.

```bash
bob sync
```

Synchronizes the Bob Obsidian vault with `ob`, stages and commits vault changes,
and pushes via Git. This command mutates the vault repository and should only be
run when Git remotes and SSH credentials are ready.

```bash
bob tmux-pomodoro
```

Prints Pomodoro status in the tmux status-line format.

The installed compatibility shims are:

```text
bob_pomodoro
bob_pomodoro_runtimes
bob_notify
bob_sync
tmux_bob_pomodoro
```

They call the same native Rust command implementations as `bob <subcommand>`.

## Runtime Dependencies

Normal command execution no longer requires Bash, Python, or Perl. These tools
are still useful for validating or forcing the embedded script fallback with
`BOB_CLI_USE_SCRIPT=1`.

The remaining runtime dependencies are:

- `ob` from obsidian-headless for vault sync and runtime annotation
- `git` and `ssh` for `bob sync`
- `notify-send` for desktop notifications from `bob notify`
- `bash` only when `bob sync` needs to load `ob` through NVM or source
  `~/.ssh-agent-thing`

No old chezmoi script files are required after installation. Cargo installs the
Rust binaries, and the binaries carry the script assets they need.

## Environment

`BOB_DIR` sets the Bob vault directory. It defaults to `~/bob`.

`BOB_DAY_FILE` sets the exact daily note path used by `bob pomodoro`.

`BOB_NOW` sets the current timestamp for Pomodoro status and default runtime note
selection. Supported formats include `YYYY-MM-DD`, `YYYY-MM-DD HH:MM`, and
`YYYY-MM-DD HH:MM:SS`.

`DATE` preserves the legacy date override behavior. It can be a date command
prefix such as `date --utc`, or a timestamp in the same formats accepted by
`BOB_NOW`.

`OB_COMMAND` overrides the `ob` executable used by `bob pomodoro-runtimes`.

`BOB_SYNC_LOCK_FILE` overrides the lock path used by `bob sync`.

`BOB_SYNC_COMMIT_MESSAGE` overrides the commit message used by `bob sync`.

`BOB_CLI_USE_SCRIPT=1` forces the embedded Bash/Python fallback implementation.

## Migration Notes

Use `bob pomodoro`, `bob pomodoro-runtimes`, `bob notify`, `bob sync`, and
`bob tmux-pomodoro` for new integrations. The legacy command names are installed
only as compatibility shims for existing callers.

The original script implementations remain embedded only as a rollback path.
New integrations should rely on the native Rust command behavior.

## Release Checklist

Run the package checks from a clean worktree:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features
cargo test
just check-scripts
cargo package --list
```

Run a local install smoke test:

```bash
root="$(mktemp -d)"
cargo install --path . --locked --root "$root"
"$root/bin/bob" pomodoro-runtimes --help
BOB_DAY_FILE=/tmp/bob-cli-missing-day.md "$root/bin/bob" pomodoro
BOB_DAY_FILE=/tmp/bob-cli-missing-day.md "$root/bin/bob" tmux-pomodoro
```

Run a tmux status smoke test after installing locally:

```bash
tmux display-message -p '#(bob tmux-pomodoro)'
```

Before running `bob sync` in a release smoke test, verify that `BOB_DIR` points
at the intended vault and that its Git remote can be pushed without prompts.

# Bob CLI

`bob-cli` installs the `bob` command and compatibility shims for the Bob Obsidian
vault and Pomodoro workflow. The command implementations are native Rust by
default. The earlier shell implementations remain embedded as a rollback path:
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
"$root/bin/bob" collect-done --help
BOB_DAY_FILE=/tmp/bob-cli-missing-day.md "$root/bin/bob" pomodoro
BOB_DAY_FILE=/tmp/bob-cli-missing-day.md "$root/bin/bob" tmux-pomodoro
"$root/bin/bob_notify" --help
```

## Commands

```bash
bob collect-done [--threshold N]
```

Scans the Bob vault for completed (`[x]`) and canceled (`[-]`) Markdown task
blocks containing `#task`, then moves blocks from notes that meet the threshold
into matching archive notes under `done/`. The default threshold is `10`; use a
smaller value for a targeted collection pass, such as `--threshold 1` in a
fixture vault.

Archive paths mirror the source note path and add `_done` to the file stem. For
example, `projects/foo.md` archives to `done/projects/foo_done.md`. Archive
notes are created with `parent` pointing at the original source note plus
`type: "[[done]]"`, such as `parent: "[[projects/foo]]"` and
`type: "[[done]]"`.
Existing archive notes have `parent` and `type` frontmatter inserted or repaired
before new blocks are appended. Source notes that have a matching archive note
are linked back to it with `done_tasks`, such as
`done_tasks: "[[done/projects/foo_done]]"`. Existing archive notes under `done/`
are backfilled into source note frontmatter and archive metadata on future runs
even when no task blocks meet the threshold.

When task blocks with explicit Obsidian block ids are moved, links to those
blocks are repaired across vault Markdown notes. For example,
`[[projects/foo#^abc123]]`, `![[projects/foo#^abc123]]`, and aliases such as
`[[projects/foo#^abc123|follow-up]]` are rewritten to
`[[done/projects/foo_done#^abc123]]`. Moved block ids are de-duplicated within
their destination archive note before link repair. If `^abc123` already exists
in `done/projects/foo_done.md`, the moved id becomes the smallest available
suffix such as `^abc123-1`, and repaired links point at that final id. If
multiple moved blocks originally share the same id, their archived ids are still
made unique, but existing links to the original duplicate id are left unchanged
because the intended block is ambiguous. Only explicit `^block-id` targets can
be rewritten; heading links and tasks without block ids do not have a stable
target to repair.

Before writing files, `bob collect-done` runs `ob sync --path <vault>` when the
configured `ob` command is available. Missing `ob` is reported as a skipped sync.
Other sync failures stop the command before vault files are changed. In a Git
worktree, the command refuses to modify source, archive, or link-repair
candidates that already have uncommitted changes, stages only the files it
touches, commits with a `bob collect-done YYYY-MM-DD` message, and pushes.
Non-Git vaults are left uncommitted.

```bash
bob highlights-ref scan [--dry-run]
bob highlights-ref sync <pdf> [--dry-run] [--write-pdf] [--prefer marker|frontmatter]
bob highlights-ref doctor
bob highlights-ref marker <pdf>
```

Prepares the Highlights app PDF annotation to Bob reference note sync workflow.
The Phase 1 command surface is intentionally no-write: each command resolves
configuration and reports that no PDF or vault files were modified. Later phases
will scan PDFs under the configured library directory, treat the first
standalone PDF note as the marker note, sync selected marker/frontmatter fields
both ways, and render Highlights sidecar annotations into
`ref/<pdf-basename>.md`.

The full contract and MacBook setup guide live in
[`docs/highlights-ref-sync.md`](docs/highlights-ref-sync.md).

```bash
bob pomodoro
```

Prints the current Pomodoro ledger entry from today's Bob daily note, including
time remaining or recent overdue status. It defaults to
`$BOB_DIR/YYYY/YYYYMMDD_day.md`, or `~/bob/YYYY/YYYYMMDD_day.md` when `BOB_DIR`
is unset, unless `BOB_DAY_FILE` is set.
Ledger entries may use bold Markdown ranges such as
`(**0945-1015** [t:: 30m])`; command output remains plain, for example
`0945-1015 Review crate skeleton`.

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
bob_notify
bob_sync
tmux_bob_pomodoro
```

They call the same native Rust command implementations as `bob <subcommand>`.

## Runtime Dependencies

Normal command execution no longer requires Bash or Perl. These tools are still
useful for validating or forcing the embedded script fallback with
`BOB_CLI_USE_SCRIPT=1`.

The remaining runtime dependencies are:

- `ob` from obsidian-headless for `bob sync` and pre-write
  `bob collect-done` sync
- `git` and `ssh` for `bob sync` and for `bob collect-done` commit/push
  behavior when the vault is a Git worktree
- `notify-send` for desktop notifications from `bob notify`
- `bash` only when `bob sync` needs to load `ob` through NVM or source
  `~/.ssh-agent-thing`

No old chezmoi script files are required after installation. Cargo installs the
Rust binaries, and the binaries carry the script assets they need.

## Environment

`BOB_DIR` sets the Bob vault directory. It defaults to `~/bob`.

`BOB_DAY_FILE` sets the exact daily note path used by `bob pomodoro`.

`BOB_NOW` sets the current timestamp for Pomodoro status and default runtime note
selection. It also controls the default `bob collect-done YYYY-MM-DD` commit
message date. Supported formats include `YYYY-MM-DD`, `YYYY-MM-DD HH:MM`, and
`YYYY-MM-DD HH:MM:SS`.

`BOB_HIGHLIGHTS_LIB_DIR` sets the Highlights PDF library directory used by
`bob highlights-ref`. It defaults to `lib` under `BOB_DIR`. Relative values are
resolved under the Bob vault; absolute paths and `~/...` paths are used as
configured.

`BOB_HIGHLIGHTS_REF_DIR` sets the generated reference note directory used by
`bob highlights-ref`. It defaults to `ref` under `BOB_DIR`.

`BOB_HIGHLIGHTS_DEFAULT_PARENT` sets the fallback `parent` frontmatter value for
new reference notes when the PDF marker note omits `parent`. It defaults to
`[[obsidian]]`.

`DATE` preserves the legacy date override behavior. It can be a date command
prefix such as `date --utc`, or a timestamp in the same formats accepted by
`BOB_NOW`.

`OB_COMMAND` overrides the `ob` executable used by `bob collect-done`. If that
executable is unavailable, collection reports the skipped sync before scanning
the vault.

`BOB_SYNC_LOCK_FILE` overrides the lock path used by `bob sync`.

`BOB_SYNC_COMMIT_MESSAGE` overrides the commit message used by `bob sync`.

`BOB_CLI_USE_SCRIPT=1` forces the embedded shell fallback implementation.

## Migration Notes

Use `bob pomodoro`, `bob notify`, `bob sync`, and `bob tmux-pomodoro` for new
integrations, and run `bob collect-done` when done and canceled task blocks
should be archived from the vault. The legacy command names are installed only
as compatibility shims for existing callers.

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
"$root/bin/bob" collect-done --help
BOB_DAY_FILE=/tmp/bob-cli-missing-day.md "$root/bin/bob" pomodoro
BOB_DAY_FILE=/tmp/bob-cli-missing-day.md "$root/bin/bob" tmux-pomodoro
"$root/bin/bob_notify" --help
```

Run a tmux status smoke test after installing locally:

```bash
tmux display-message -p '#(bob tmux-pomodoro)'
```

Before running `bob sync` in a release smoke test, verify that `BOB_DIR` points
at the intended vault and that its Git remote can be pushed without prompts.
Before running `bob collect-done` against the real vault, verify that `~/bob` is
the intended vault, inspect `git -C ~/bob status --short`, and expect the command
to skip candidate files that are already dirty.

For an end-to-end collection smoke test, install the local binary, run
`bob collect-done` against `~/bob`, then verify that archive notes under
`~/bob/done` include `parent: "[[source]]"` for the original note and
`type: "[[done]]"`, source notes include matching `done_tasks` links and no
longer contain the collected blocks, Obsidian links to moved `^block-id` task
blocks point at `done/..._done#^block-id`, and the vault Git commit was pushed.

# Bob CLI

`bob-cli` installs the `bob` command for the Bob Obsidian vault and Pomodoro
workflow. The preferred interface is `bob <subcommand>`; `bob --help` is the
authoritative command index. Command implementations are native Rust by
default.

Legacy command names still exist as installed binaries for existing tmux,
shell, and automation callers. The Pomodoro and notification shell
implementations remain embedded as a targeted rollback path; see
[Compatibility shims](#compatibility-shims) for the exact mappings and fallback
behavior.

## Contents

- [Installation](#installation)
- [Terms](#terms)
- [Daily workflow](#daily-workflow)
- [Vault layout](#vault-layout)
- [Commands](#commands)
- [Capture](#capture)
- [Query](#query)
- [Task status hooks](#task-status-hooks)
- [Projects](#projects)
- [Plugins](#plugins)
- [Highlights](#highlights)
- [Nightly maintenance](#nightly-maintenance)
- [Vault sync](#vault-sync)
- [Move done tasks](#move-done-tasks)
- [Pomodoro status](#pomodoro-status)
- [Compatibility shims](#compatibility-shims)
- [Runtime dependencies](#runtime-dependencies)
- [Environment](#environment)
- [Migration notes](#migration-notes)
- [Release checklist](#release-checklist)
- [Detailed command contracts](#detailed-command-contracts)

## Installation

Installation requires a current stable Rust toolchain with `cargo`. The default
vault location is `~/bob`; set `BOB_DIR` when the vault lives elsewhere.

For local development from this checkout:

```bash
cargo install --path . --locked --force
```

For installation from the Git remote:

```bash
cargo install --git git@github.com:bobs-org/bob-cli.git --locked --force bob-cli
```

With `just` installed, smoke-test an install without replacing an existing
user install:

```bash
just install-smoke
```

After installation, verify the vault selection with read-only commands before
running a command that writes or pushes changes:

```bash
export BOB_DIR=/path/to/bob-vault
bob --help
bob capture-targets
bob projects list
```

Priority rolls (`p:<N>`) and Highlights pre-scan hooks read
`~/.config/bob/config.yml`. Override that path with `BOB_CONFIG_FILE` or
`XDG_CONFIG_HOME`; see [Environment](#environment).

## Terms

These words show up across every command. They are vault conventions, not
separate `bob` subcommands.

| Term | Meaning |
| --- | --- |
| Vault | The Obsidian folder Bob operates on (`~/bob` by default; `BOB_DIR` to override) |
| Route | The lowercase name of a top-level note; `@groceries` writes `groceries.md` |
| Pomodoro | A checkbox in the daily note's `Pomodoros` section for one work session |
| Block ID | The trailing `^id` on a task line, used to link or nest under that task |
| Task link | A `[[note#^id]]` (or embed) pointing at a task. When that is the only content of a Pomodoro child bullet, it is that session's planned work |
| Schedule Log | A managed `🗓️ **SCHEDULE LOG**` child that records each schedule change |
| Work Log | A managed `🛠️ **WORK LOG**` child that records work summaries |

## Daily workflow

Bob tracks a daily Pomodoro ledger inside an Obsidian vault and keeps that vault
synced through Git. Capture, linking, status, and nightly maintenance are
separate steps:

1. **Capture** with `bob capture` or Bob Mac Capture (the macOS panel that
   calls the same commands). Tasks land in `mac_inbox.md` unless an `@route`
   token sends them to another note; scheduled checkbox-bearing captures start
   Blocked (`[?]`).
2. **Link today's work** onto a Pomodoro in the daily note. That happens when
   you capture with `@route:id` (which also marks the task Next), or when you
   add a task link under a Pomodoro in Obsidian. `bob pomodoro`,
   `bob tmux-pomodoro`, and `bob notify` only *read* that ledger; they do not
   create links.
3. **Reconcile statuses** with `bob task-status-hooks` so Next, In Progress, and
   Blocked markers follow the ledger and any schedules changed outside capture.
4. **Nightly**, run `bob nightly` to reconcile the vault through Git, archive
   done and canceled tasks, and reconcile the vault again.

Read-only inspection (`bob query`, `bob projects list`, `bob plugins list`,
`bob highlights doctor`) can run at any time.

## Vault layout

Paths below are relative to `BOB_DIR` (`~/bob` by default):

| Path | Role |
| --- | --- |
| `mac_inbox.md` | Default capture target |
| `<route>.md` | Area or project note selected by an `@route` token |
| `YYYY/YYYYMMDD.md` | Daily note; the `Pomodoros` section is the session ledger |
| `done/` | Archive notes written by `bob move-done-tasks` |
| `.obsidian/plugins/` | Installed community plugins, including Bob's custom plugins |
| `xlib/` | Highlights intake PDFs from `bob highlights create` |
| `lib/` | Highlights library PDFs after `bob highlights scan` |
| `old_lib/` | Archival predecessor of `lib/`; tracked in the vault Git repo, which is now the vault's only sync channel |
| `ref/` | Generated Highlights reference notes |

Daily-note selection uses `BOB_DAY_FILE` when set, otherwise
`<bob-dir>/YYYY/YYYYMMDD.md` for the local date (or `BOB_NOW`).

## Commands

Bob's workflow commands are:

| Command | Purpose |
| --- | --- |
| [`capture`](#capture) | Capture a task or section bullet, optionally with clipboard content |
| `capture-complete` | Complete capture marker or wikilink syntax at the cursor |
| `capture-parse` | Preview what in-progress capture text and wikilinks mean |
| `capture-pomodoro-name` | Assign a canonical name to an unnamed Pomodoro |
| `capture-pomodoros` | List today's Pomodoro ledger entries |
| `capture-rewrite` | Apply the capture grammar's automatic draft rewrites (bare `@@` absorption) |
| `capture-sections` | List the non-`Tasks` headings in a routed note |
| `capture-targets` | List inbox, area, and non-terminal project capture routes |
| `capture-task-id` | Assign a user-authored block ID to an open capture task |
| `capture-task-sections` | List the ALL-CAPS child sections of a capture task |
| `capture-tasks` | List the open tasks in a routed note |
| [`highlights`](#highlights) | Synchronize Highlights PDF annotations with reference notes |
| [`move-done-tasks`](#move-done-tasks) | Archive done and canceled task blocks and repair their links |
| [`nightly`](#nightly-maintenance) | Run the Git sync and maintenance workflow |
| [`notify`](#pomodoro-status) | Notify when the current Pomodoro finishes |
| [`plugins`](#plugins) | List and deploy Bob's custom Obsidian plugins |
| [`pomodoro`](#pomodoro-status) | Print the current Pomodoro status |
| [`projects`](#projects) | Inspect and synchronize project lifecycle tasks |
| [`query`](#query) | Run headless Dataview or Tasks queries, or live Dataview queries |
| [`task-status-hooks`](#task-status-hooks) | Reconcile Pomodoro links, task ranks, and derived Blocked state |
| [`tmux-pomodoro`](#pomodoro-status) | Print Pomodoro status for a tmux status line |
| [`vault-sync`](#vault-sync) | Reconcile the Bob vault through Git |

Use `bob <command> --help` for concise usage. The sections below summarize each
workflow and link to the detailed command contract where one exists.

The hidden `task-status-setter` and `mark-next-tasks` spellings remain
compatibility-only aliases for `task-status-hooks` and are not listed in
top-level help.

## Capture

```bash
bob capture [OPTIONS] [--] [TEXT]...
```

Captures one task, ordinary Markdown bullet, or task sub-bullet into the vault
without opening desktop Obsidian. The default destination is `mac_inbox.md`.
`TEXT` may be several physical lines: each item’s first nonblank line is the
parent, later authored bullets become children, and blank lines split a batch
of items. A `@@route` or `@@route+id` token anywhere in the draft is a
draft-wide destination declaration, not body text; item-local `@...` markers
still win, and `bob capture` warns when a local marker shadows the declaration
typed on that same item. The whole batch is planned before anything is written.

| Marker | Meaning |
| --- | --- |
| `@@route` | Shared task destination, anywhere in the draft, for otherwise-unrouted items |
| `@@route+id` | Shared parent-task destination, anywhere in the draft, for otherwise-unrouted items |
| `@route` | Task in `<route>.md` |
| `@route#Section` | Ordinary bullet under a matching non-`Tasks` heading |
| `@route#` | Ordinary bullet under any non-`Tasks` heading |
| `@route^id` | Ordinary task with a user-authored block ID |
| `@route:id` | Next-status task plus a Pomodoro task link; scheduled tasks start Blocked |
| `@route:id#pomodoro` | Same, linked under the named open Pomodoro |
| `@route+id` | Child bullet under an existing task |
| `@route+id#section` | Child bullet under an ALL-CAPS section of that task |
| trailing `#` | Plain-text note on a Pomodoro (no `@route`) |
| `s:<N>` | Schedule N days from today; checkbox-bearing captures start Blocked, including `s:0` |
| `p:<N>` | Write priority level N and roll a scheduled date, so checkbox-bearing captures start Blocked |
| `%`, `%N`, `%header` | Attach clipboard content |

`#` is not one marker. A trailing bare `#` is a Pomodoro note; `@route#…` selects
a heading in that note; `@route+id#…` selects an ALL-CAPS child section of that
task; `@route:id#…` selects a named open Pomodoro. A `#` in the middle of the
body stays ordinary text. The retired
`@route::id` spelling is not accepted; use `@route^id` for an ordinary task
with a block ID.

```bash
bob capture buy milk @groceries
bob capture '@dev^foobar' 'Some ordinary task.'
bob capture '@dev:foobar' 'Some foobar task.'
bob capture '@dev:foobar#bugs' 'Some foobar task.'
bob capture '@cash+goog-exit' 'Called Morgan Stanley today.'
bob capture remembered to bump the timeout #
printf '@@foo\nFirst task\n\nSecond task @bar\n' | bob capture
```

Editor clients such as Bob Mac Capture call `bob capture --format json`,
`bob capture-parse`, `bob capture-rewrite`, and `bob capture-complete`.
`bob capture-rewrite` turns a bare `@@` typed inside an item that already has
a `@route` (or `@route+id`) marker into `@@route` (or `@@route+id`),
deleting the marker it absorbed. Discovery helpers
(`capture-targets`, `capture-sections`, `capture-tasks`,
`capture-task-sections`, `capture-pomodoros`) feed those pickers.
`capture-task-id` assigns a user-authored block ID to an open task that still
lacks one. `capture-pomodoro-name` assigns a canonical ALL-CAPS name to an
unnamed Pomodoro in today's daily note.

The full grammar, JSON contracts, and picker protocol live in
[`docs/capture.md`](docs/capture.md).

## Query

```bash
bob query --source '#project'
bob query --query 'LIST FROM #waiting'
bob query --format json --query-file queries/projects.dql
bob query --tasks 'status.type is TODO' --origin dash.md
bob query --format json --tasks-file queries/all.tasks
bob query --format markdown --tasks-note dash.md
```

Runs Dataview source expressions, DQL queries, and Obsidian Tasks queries from
the shell. The default native engine evaluates queries against the local
Markdown vault, so scripts do not need a running desktop Obsidian app. `paths`
output prints vault-relative Markdown paths, `json` output is stable for
scripts, and `markdown` output prints Dataview-rendered Markdown for supported
DQL results. Native Tasks support includes filters, Boolean expressions,
JavaScript `by function` instructions with Moment, sorting, grouping, limits,
layout instructions, Query File Defaults, placeholders, and rendered Markdown.
`--tasks-note` runs every fenced Tasks block with its note context and identifies
each result by heading.

This command does not reconcile the vault; freshness is handled by
`bob vault-sync` and the configured background or cron sync path. Use
`--engine obsidian` when you want exact
behavior from the live Dataview plugin in an open Obsidian vault. Tasks inputs
remain native-only, with an env-gated live renderer harness for parity checks.

The full command contract and live smoke-test steps live in
[`docs/dataview.md`](docs/dataview.md).

## Task status hooks

```bash
bob task-status-hooks [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json]
```

Run this after capturing or closing Pomodoro-linked work, and after
`bob projects sync` writes schedules. It makes today's Pomodoro ledger the
source of truth for Next / In Progress promotions and structural cleanup, and
uses the latest existing earlier daily note as a read-only recent-activity
source.

Direct block links under open Pomodoros promote Ready tasks to Next (`[*]`) and
leave In Progress (`[/]`) alone. Sole transcluded dependencies inherit the
strongest parent rank. Unreachable Next tasks clear back to Ready unless recent
activity still references them; stale In Progress in `[[area]]` / `[[project]]`
notes rolls back the same way. Independently, open Dataview dependencies and
future `[scheduled:: YYYY-MM-DD]` dates mark a task Blocked (`[?]`). The command
also retires completed references, moves stray bullets onto the current
Pomodoro, repairs Pomodoro markers, de-duplicates the same task under later
open Pomodoros, and removes list items that only point at canceled tasks.

```bash
bob task-status-hooks --dry-run
```

The command refuses to change files if the current daily note is missing, lacks
a `Pomodoros` section, or has multiple open timed Pomodoros. The full sync,
link-resolution, exclusion, output, and JSON contract lives in
[`docs/task-status-hooks.md`](docs/task-status-hooks.md).

## Projects

```bash
bob projects list [-b|--bob-dir DIR]
bob projects sync [-b|--bob-dir DIR] [-d|--dry-run]
```

Scans notes whose frontmatter declares `type: "[[project]]"`. `list` prints
frontmatter status, open `#task` counts, and the `^prj` lifecycle task. `sync`
reconciles `status` from that task (`done`, `canceled`, or reopen to `wip`),
manages `#hide` so `^prj` surfaces on `dash.md` only when nothing else is open,
maintains the machine-owned Sub-projects ledger, and propagates optional
`scheduled: YYYY-MM-DD` frontmatter onto ordinary open tasks.

`sync` writes frontmatter, `#hide`, Sub-projects lines, and inline schedules;
it does not change checkboxes. Run `bob task-status-hooks` afterward to derive
or recover `[?]` Blocked markers. The property picker in Bob Navigation
Hotkeys can propagate schedules and reconcile Blocked in the same editor
transaction. The full project task contract lives in
[`docs/projects.md`](docs/projects.md).

## Plugins

```bash
bob plugins [-b|--bob-dir DIR] [-f|--format table|json] [-n|--no-pull] [-r|--repo DIR]
bob plugins list [-b|--bob-dir DIR] [-f|--format table|json] [-n|--no-pull] [-r|--repo DIR]
bob plugins sync [-B|--backup-dir DIR] [-b|--bob-dir DIR] [-d|--dry-run] [-F|--force] [-n|--no-pull] [-p|--plugin ID] [-r|--repo DIR]
```

Lists Bryan's custom Bob Obsidian plugins from the
[`bobs-org/bob-plugins`](https://github.com/bobs-org/bob-plugins) repo and
annotates each with live vault state. Running `bob plugins` with no subcommand
runs `list`. Managed files are `manifest.json`, `main.js`, and `styles.css`
when present; runtime files such as `data.json` are never touched.

The repo root resolves from `-r, --repo`, then `BOB_PLUGINS_DIR`, then
`~/projects/github/bobs-org/bob-plugins`. The vault root resolves from
`-b, --bob-dir`, then `BOB_DIR`, then `~/bob`. By default, `list` and `sync`
run a non-interactive `git pull` first; pass `-n, --no-pull` to skip it.
`sync` copies managed files into
`<bob-dir>/.obsidian/plugins/<id>/`, skips vault files with uncommitted Git
changes unless `-F, --force` is set, and writes timestamped backups first.

The full command contract lives in [`docs/plugins.md`](docs/plugins.md).

## Highlights

```bash
bob highlights create <md-file> [-d|--dry-run] [-f|--force] [-i|--include-id] [-o|--output PDF] [-P|--parent NOTE] [-s|--status STATUS] [-t|--ref-type DIR] [-x|--xlib-dir PATH]
bob highlights doctor [-x|--xlib-dir PATH]
bob highlights marker <pdf> [-x|--xlib-dir PATH]
bob highlights scan [-d|--dry-run] [-j|--jobs N] [-v|--verbose] [-w|--write-pdfs] [-x|--xlib-dir PATH]
bob highlights sync <pdf> [-d|--dry-run] [-w|--write-pdf] [-p|--prefer marker|frontmatter] [-x|--xlib-dir PATH]
```

Turns Markdown into Highlights-ready PDFs and turns Highlights annotations into
Obsidian reference notes.

- `create <md-file>` renders through pandoc and xelatex into
  `xlib/chat/<basename>.pdf` (override the subdirectory with `--ref-type`) and
  embeds the page-1 marker `scan` needs. `-o, --output` writes the complete PDF
  path instead, including the filename, and cannot be combined with
  `--ref-type`. Relative output paths are resolved from the current directory
  and a leading `~` is expanded. `--include-id` adds marker `id` from the
  Markdown filename stem. Intake targets still go through `scan`; a PDF written
  directly into the library is also found by `scan`; a PDF outside both
  directories needs `bob highlights sync <PDF>`.
- `scan` runs the configured pre-scan hook on writing runs, then moves pending
  PDFs from `xlib/<rel>` to `lib/<rel>` and recursively syncs the library. By
  default it does not write PDF markers; use `scan --dry-run --write-pdfs`,
  review, then `scan --write-pdfs`. `-v, --verbose` prints the detailed per-PDF
  plan instead of the concise report.
- `sync <pdf>` updates one reference note from the page-1 marker and sidecar.
- `marker <pdf>` inspects that marker without writing.
- `doctor` checks vault paths, intake, sidecars, markers, Git, pandoc, and
  optional `ob` without writing.

Generated notes live under `ref/`. Nested library PDFs such as
`lib/books/foo.pdf` write `ref/books/foo.md` with `type: "[[ref]]"` and
`ref_type: books`. The generated `^ref` task is the visible lifecycle control.
Marker `status` values are `ready`, `next`, `wip`, `read`, `abandoned`, and
`legacy`.

The full contract and MacBook setup guide live in
[`docs/highlights-ref-sync.md`](docs/highlights-ref-sync.md).

## Nightly maintenance

```bash
bob nightly
```

Runs the nightly Bob maintenance path. It acquires the shared vault-sync lock
(default `$XDG_RUNTIME_DIR/bob_sync.lock` when that variable names an existing
directory, otherwise `/tmp/bob_sync.lock`), then:

1. Runs `bob vault-sync` against the vault.
2. Runs `bob move-done-tasks` against the vault.
3. Runs `bob vault-sync` against the vault again.

A failed step is reported but does not prevent later steps from running. If
another Bob maintenance run already holds the lock, the command exits 0 after
printing that it is already active. `bob nightly` accepts no options other than
`-h, --help`.

## Vault sync

```bash
bob vault-sync [run] [-n|--dry-run] [-m|--message MESSAGE] [-q|--quiet]
bob vault-sync status [-j|--json]
```

Runs one Git reconcile cycle for the Bob vault. The default subcommand is
`run`, so `bob vault-sync --dry-run` is accepted. A cycle acquires the shared
maintenance lock, recovers interrupted merge/rebase/cherry-pick state, commits
local vault changes when present, fetches and merges `origin/master`, resolves
supported conflicts by writing local conflict copies under `_conflicts/`, and
pushes with bounded non-fast-forward retries.

The command refuses to stage any file at or above 95 MiB, warns for files at or
above 50 MiB, and writes a status record after each non-dry-run cycle. Use
`bob vault-sync status --json` for the machine-readable record containing the
last attempt/success timestamps, local and remote SHAs, committed-file count,
push retries, duration, conflict-copy paths, interrupted-merge recovery flag,
and last error.

If another maintenance command already holds the lock, `bob vault-sync run`
exits 0 silently.

The operational runbook for the two-machine Bob vault sync channel lives in
[`docs/vault-git-sync.md`](docs/vault-git-sync.md).

## Move done tasks

```bash
bob move-done-tasks [-t|--threshold N]
```

Scans the Bob vault for completed (`[x]`) and canceled (`[-]`) Markdown task
blocks containing `#task`, then moves blocks from notes that meet the threshold
into matching archive notes under `done/`. The default threshold is `10`; use a
smaller value for a targeted collection pass, such as `-t 1` in a
fixture vault.

Archive paths mirror the source note path and add `_done` to the file stem. For
example, `projects/foo.md` archives to `done/projects/foo_done.md`. Archive
notes are created with `parent` pointing at the original source note plus
`type: "[[done]]"`, such as `parent: "[[projects/foo]]"` and
`type: "[[done]]"`. Existing archive notes have `parent` and `type` frontmatter
inserted or repaired before new blocks are appended. Source notes that have a
matching archive note are linked back to it with `done_tasks`, such as
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

Task dependency metadata has a separate vault-wide identity from its Obsidian
block link. A task at `projects/foo.md#^abc123` uses
`[id:: projects__foo__abc123]`, and dependents use the same value in
`[dependsOn:: projects__foo__abc123]`; the trailing block token remains
`^abc123`. When a task moves, the command rewrites its `[id::]` to the archive
path/final block ID and repairs exact dependency tokens across all planned
files. Metadata and link repair share the same atomic preview/write plan.

The command itself does not reconcile the full vault; `bob nightly` runs
`vault-sync` before and after invoking it. In a Git worktree, the command stages
only the files it touches, commits with a `bob move-done-tasks YYYY-MM-DD`
message, and pushes. Existing uncommitted changes in touched source, archive,
or link-repair files are included in that scoped commit after the command
rewrites those files. Non-Git vaults are left uncommitted.

## Pomodoro status

```bash
bob pomodoro [-d|--debug] [-s|--show-stale] [-v|--verbose]
```

Prints the current Pomodoro ledger entry from today's Bob daily note, including
time remaining or recent overdue status. It defaults to
`$BOB_DIR/YYYY/YYYYMMDD.md`, or `~/bob/YYYY/YYYYMMDD.md` when `BOB_DIR` is
unset, unless `BOB_DAY_FILE` is set.
Ledger entries may use bold Markdown ranges such as
`(**0945-1015** [t:: 30m])`; command output remains plain, for example
`0945-1015 Review crate skeleton`.

The command exits successfully with no output when the daily note is missing,
has no open Pomodoro, or the open Pomodoro is more than nine minutes overdue.
Pass `-s` / `--show-stale` when a consumer needs to distinguish an old open
entry from no open entry; stale open Pomodoros keep the same normalized
`[OVERDUE by <minutes>m] HHMM-HHMM <task>` output shape as recent overdue
Pomodoros. `-d, --debug` and `-v, --verbose` enable debug tracing on stderr.

```bash
bob notify [-v] PRE_CHECK_SLEEP POST_NOTIFY_SLEEP
```

Polls Pomodoro status until the current entry is overdue, then sends a desktop
notification when `notify-send` is available and rings the terminal bell three
times. `PRE_CHECK_SLEEP` is the seconds to wait between status checks;
`POST_NOTIFY_SLEEP` is the seconds to wait after a notification before polling
again. Polling uses the same default status as `bob pomodoro` without
`--show-stale`: an entry more than nine minutes overdue looks like no open
Pomodoro, so start `bob notify` while the session is still running or only
recently overdue. Loop status messages always go to stderr. `-v` / `--verbose`
may be repeated; extra debug tracing is emitted at `-vv`. Help text still uses
the legacy binary name `bob_notify`.

```bash
bob tmux-pomodoro
```

Prints Pomodoro status in tmux status-line format: the regular status followed
by ` | `. Missing or stale Pomodoros produce no output.

## Compatibility shims

The installed legacy binaries map to the preferred interface as follows:

| Compatibility binary | Preferred command |
| --- | --- |
| `bob_notify` | `bob notify` |
| `bob_pomodoro` | `bob pomodoro` |
| `tmux_bob_pomodoro` | `bob tmux-pomodoro` |

By default they call the same native Rust implementations as the preferred
commands. With `BOB_CLI_USE_SCRIPT=1`, the notification and Pomodoro commands
and their shims delegate to their embedded shell assets. Native-only commands
ignore the fallback setting. Extracted assets are cached in a version-and-content-specific subdirectory of
`$XDG_CACHE_HOME/bob-cli/scripts/`. If `XDG_CACHE_HOME` is unset or empty, the
base is `$HOME/.cache`; if neither variable is available, Bob uses the system
temporary directory.

## Runtime dependencies

Native command execution does not require Bash or Perl. Forced shell fallback
with `BOB_CLI_USE_SCRIPT=1` requires Bash, and the Pomodoro-based fallback
scripts also require Perl.

The documented workflows use these external-tool integrations:

- `obsidian` CLI plus a running desktop Obsidian vault with the Dataview plugin
  only when using `bob query --engine obsidian`
- `git` for `bob vault-sync`, Git-backed `bob move-done-tasks`, plugin dirty-file
  checks, and the default `bob plugins` repository refresh; remote operations
  also need the credentials required by the configured remote
- `notify-send` for desktop notifications from `bob notify`; Bob also rings the
  terminal bell whether or not `notify-send` is available
- platform clipboard tools for `bob capture` clipboard input: `pbpaste` on
  macOS; `wl-paste`, `xclip`, or `xsel` on Linux; or `tmux show-buffer` in a
  display-less tmux session (see `BOB_CLIPBOARD_CMD` below for the exact
  fallback order)
- `pandoc` and `xelatex` for `bob highlights create`; override pandoc with
  `BOB_PANDOC_COMMAND`
- `bash` for the embedded shell fallback and for sourcing
  `~/.ssh-agent-thing`; the Pomodoro shell fallback additionally uses `perl`

No old chezmoi script files are required after installation. Cargo installs the
Rust binaries, and the binaries carry the script assets they need.

## Environment

`BOB_VAULT_SYNC_LOCK_FILE` overrides the lock path used by `bob vault-sync` and
`bob nightly`.
The default path is the same shared `bob_sync.lock` path used by nightly
maintenance.

`BOB_VAULT_SYNC_STATE_FILE` overrides the JSON status record written and read
by `bob vault-sync`. The default is
`$XDG_STATE_HOME/bob-cli/vault-sync.json`, or
`$HOME/.local/state/bob-cli/vault-sync.json` when `XDG_STATE_HOME` is unset.

`BOB_CLI_USE_SCRIPT=1` selects an embedded shell implementation where one is
available. See [Compatibility shims](#compatibility-shims) for the exact command
coverage and cache location.

`BOB_CLIPBOARD_CMD` is whitespace-split into a command and arguments and takes
priority over platform clipboard tools for `bob capture`. Without it, capture
uses `pbpaste` on macOS; on Linux it uses `wl-paste --no-newline --type text`
under Wayland or `xclip -selection clipboard -o` under X11, falling back to
`xsel --clipboard --output` when `xclip` is unavailable. A tmux session without
a display uses `tmux show-buffer`. Setting `BOB_CLIPBOARD_CMD` is also the
recommended deterministic automation and test hook.

`BOB_CLIPBOARD_HISTORY_CMD` is the portable clipboard-history provider for
counted captures above one. It is whitespace-split like `BOB_CLIPBOARD_CMD`,
receives the requested total count as its final argument, and must print a UTF-8
JSON array of complete clipboard strings ordered newest first. JSON framing
allows an entry to contain newlines. Bob reads the live clipboard separately,
removes at most the first equal history candidate, and then requires enough
older candidates to fulfill the exact count. A failed command, malformed JSON,
invalid entry, or insufficient result aborts the capture without vault writes.

Without that override, macOS reads Clipy's production `sqlite.db` history
read-only, validates the required schema, and reconstructs stored UTF-8 text
and file/URL assets rather than using Clipy's truncated display title. Other
platforms have no automatic history provider and report how to configure
`BOB_CLIPBOARD_HISTORY_CMD`; `%` and `%1` continue to use the portable live
clipboard source alone.

`BOB_CONFIG_FILE` sets the exact bullet-property config file used by `p:<N>`
priority rolls. When unset, Bob uses `$XDG_CONFIG_HOME/bob/config.yml`, then
`~/.config/bob/config.yml`.

`BOB_DATAVIEW_OBSIDIAN_COMMAND` overrides the executable used by
`bob query --engine obsidian`.

`BOB_DATAVIEW_VAULT` sets the default Obsidian vault name or ID forwarded to
`obsidian eval` by `bob query --engine obsidian`.

`BOB_DAY_FILE` sets the exact daily note path used by `bob pomodoro`,
`bob tmux-pomodoro`, `bob notify` (via the same status reader), Pomodoro-linked
and Pomodoro-note `bob capture` requests, and `bob task-status-hooks`.

`BOB_DIR` sets the Bob vault directory. It defaults to `~/bob`.

`BOB_HIGHLIGHTS_LIB_DIR` sets the Highlights PDF library directory used by
`bob highlights`. It defaults to `lib` under `BOB_DIR`. Relative values are
resolved under the Bob vault; absolute paths and `~/...` paths are used as
configured.

`BOB_HIGHLIGHTS_PRE_SCAN_COMMAND` overrides
`highlights.pre_scan_command` from `~/.config/bob/config.yml` for
`bob highlights scan`. Non-empty values run with `sh -c` from `BOB_DIR` before
intake; an empty value disables a configured hook. `scan --dry-run` reports the
hook it would run without executing it.

`BOB_HIGHLIGHTS_REF_DIR` sets the generated reference note directory used by
`bob highlights`. It defaults to `ref` under `BOB_DIR`.

`BOB_HIGHLIGHTS_XLIB_DIR` sets the Highlights PDF intake directory used by
`bob highlights`. It defaults to `xlib` under `BOB_DIR`. `lib` and `xlib` must
be distinct, non-nested directories so intake cannot move PDFs inside the tree
being scanned.

`BOB_NOW` overrides the local date and time used for Pomodoro status and default
daily-note selection by `bob pomodoro`, Pomodoro-linked capture, and
`bob task-status-hooks`. It also controls capture created/scheduled dates and
clipboard-snippet names, native Tasks-query date calculations, the default
`bob move-done-tasks YYYY-MM-DD` commit-message date, scheduled-project
visibility, and the timestamped directory name for plugin backups. Supported
formats are `YYYY-MM-DD`, `YYYY-MM-DD HH:MM`, and `YYYY-MM-DD HH:MM:SS`; `T`
may replace the space. Timezone names and UTC-offset suffixes are not accepted.
An unsupported value is ignored, after which Bob tries `DATE` and then the
system clock.

`BOB_PANDOC_COMMAND` overrides the pandoc executable used by
`bob highlights create`.

`BOB_PLUGINS_DIR` sets the source repository used by `bob plugins`. It defaults
to `~/projects/github/bobs-org/bob-plugins`.

`BOB_PLUGIN_BACKUPS_DIR` sets the base directory for backups created before
`bob plugins sync` overwrites a vault plugin file. It defaults to
`~/.local/state/bob-cli/plugin-backups`.

`BOB_PRIORITY_ROLL_SEED` pins the `p:<N>` scheduled-date roll to a decimal
integer seed so a `--dry-run` preview matches a real capture. Unset means each
capture rolls independently.

`DATE` preserves the legacy date override behavior, including the date used by
`bob capture` when `BOB_NOW` is unset. It can be a date command prefix such as
`date --utc`, or a timestamp in the same formats accepted by `BOB_NOW`.

`NO_COLOR` disables ANSI color in native human-readable output that would
otherwise be styled when stdout is a terminal.

`XDG_CACHE_HOME` is the cache root for extracted shell-fallback assets. See
[Compatibility shims](#compatibility-shims).

`XDG_CONFIG_HOME` is the base directory for the default
`$XDG_CONFIG_HOME/bob/config.yml` path used when `BOB_CONFIG_FILE` is unset.

`XDG_RUNTIME_DIR` is the preferred directory for the default
`bob_sync.lock` maintenance lock when that path exists as a directory.

## Migration notes

Use `bob pomodoro`, `bob notify`, `bob vault-sync`, and
`bob tmux-pomodoro` for new integrations, and run
`bob move-done-tasks` when done and canceled task blocks should be archived
from the vault.

The old top-level commands were renamed: `bob collect-done` is now
`bob move-done-tasks`, `bob dataview` is now `bob query`, `bob highlights-ref`
is now `bob highlights`. `bob sync`, `bob bulk-git-commit`, and the `bob_sync`
binary have been retired in favor of `bob vault-sync`. The old top-level names
are no longer registered.

The original script implementations remain embedded only as a rollback path.
New integrations should rely on the native Rust command behavior.

The retired `@<route>::<block-id>` capture spelling is no longer accepted; use
`@<route>^<block-id>` for an ordinary task with a requested block ID, and
`@<route>:<block-id>` for a Pomodoro-linked next task, optionally with
`#<pomodoro>` to name the open Pomodoro. Sub-bullet capture uses
`@<route>+<block-id>`.

## Release checklist

Run the package checks from a clean worktree:

```bash
just all
just check-scripts
just package-list
```

Run a local install smoke test:

```bash
just install-smoke
```

Run a tmux status smoke test after installing locally:

```bash
tmux display-message -p '#(bob tmux-pomodoro)'
```

Before running `bob vault-sync` in a release smoke test, verify that `BOB_DIR`
points at the intended vault and that its Git remote can be pushed without
prompts. Before running `bob move-done-tasks` against the real vault,
verify that `~/bob` is the intended vault, inspect `git -C ~/bob status --short`,
and review any local edits that may be included when touched candidate files are
rewritten.

The default `bob query` smoke tests are local and headless. Before running
live Obsidian smoke tests, start desktop Obsidian, open the target vault, enable
Dataview, and use the explicit `--engine obsidian` examples in
[`docs/dataview.md`](docs/dataview.md).

For an end-to-end collection smoke test, install the local binary, run
`bob move-done-tasks` against `~/bob`, then verify that archive notes under
`~/bob/done` include `parent: "[[source]]"` for the original note and
`type: "[[done]]"`, source notes include matching `done_tasks` links and no
longer contain the collected blocks, Obsidian links to moved `^block-id` task
blocks point at `done/..._done#^block-id`, and the vault Git commit was pushed.

## Detailed command contracts

| Topic | Document |
| --- | --- |
| Capture grammar, JSON, and picker protocol | [`docs/capture.md`](docs/capture.md) |
| `bob query` Dataview and Tasks | [`docs/dataview.md`](docs/dataview.md) |
| Highlights PDF intake and reference notes | [`docs/highlights-ref-sync.md`](docs/highlights-ref-sync.md) |
| Obsidian Sync folder exclusion runbook (historical) | [`docs/obsidian-sync-exclusions.md`](docs/obsidian-sync-exclusions.md) |
| Bob vault Git sync runbook | [`docs/vault-git-sync.md`](docs/vault-git-sync.md) |
| Custom plugin list and vault deploy | [`docs/plugins.md`](docs/plugins.md) |
| Project `^prj` lifecycle and schedules | [`docs/projects.md`](docs/projects.md) |
| Pomodoro-driven task status sync | [`docs/task-status-hooks.md`](docs/task-status-hooks.md) |

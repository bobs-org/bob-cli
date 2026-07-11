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
cargo install --git git@github.com:bobs-org/bob-cli.git --locked --force bob-cli
```

To smoke-test an install without replacing an existing user install:

```bash
root="$(mktemp -d)"
cargo install --path . --locked --root "$root"
"$root/bin/bob" --help
"$root/bin/bob" bulk-git-commit --help
"$root/bin/bob" capture --help
"$root/bin/bob" query --help
"$root/bin/bob" highlights --help
"$root/bin/bob" mark-next-tasks --help
"$root/bin/bob" move-done-tasks --help
"$root/bin/bob" nightly --help
"$root/bin/bob" notify --help
"$root/bin/bob" pomodoro --help
"$root/bin/bob" projects --help
"$root/bin/bob" projects sync --help
"$root/bin/bob" tmux-pomodoro --help
"$root/bin/bob_notify" --help
"$root/bin/bob_pomodoro" --help
"$root/bin/bob_sync" --help
"$root/bin/tmux_bob_pomodoro" --help
```

## Commands

```bash
bob bulk-git-commit
```

Stages all Bob vault changes, commits them when anything changed, and pushes via
Git. This command does not run `ob sync`; use `bob nightly` for the nightly path
that syncs Obsidian before maintenance steps. `bob bulk-git-commit` mutates the
vault repository and should only be run when Git remotes and SSH credentials are
ready.

```bash
bob capture [OPTIONS] [--] [TEXT]...
```

Captures one task into the Bob vault without requiring desktop Obsidian to be
open. Text is normalized to one line, written as
`- [ ] #task <text> [created::YYYY-MM-DD]`, and routed to `mac_inbox.md` by
default. The created date uses the local date from `BOB_NOW`, `DATE`, or the
system clock.

Automatic routing matches the Hammerspoon capture keymap: a leading
`@route text` prefix wins, otherwise a trailing `text @route` suffix is used.
Route names use `A-Z`, `a-z`, `0-9`, `_`, and `-`, are lower-cased, and write
to `<route>.md` at the vault root. Existing target files, including
`mac_inbox.md`, prefer a Markdown `Tasks` section: new captures insert after
the last top-level `#task` block in that section, or after one blank line below
the `Tasks` heading when the section has no tasks yet. Files without a `Tasks`
section keep the older fallback of inserting after the last top-level `#task`
block and its indented continuation lines, or appending at EOF.

Use a leading or trailing `@<route>:<block-id>` marker to create a
Pomodoro-linked next task. For example,
`bob capture '@dev:foobar' 'Some foobar task.'` writes:

```markdown
- [*] #task Some foobar task. [created::2026-07-10] ^foobar
```

It also adds `[[dev#^foobar]]` as a child bullet of an eligible open Pomodoro
in today's daily note. The route is lower-cased; route and block-ID characters
are limited to letters, digits, `_`, and `-`. Scheduled offsets work in either
terminal order, and the block ID remains the final task token after any
`[scheduled::YYYY-MM-DD]` property.

The daily note is selected from `BOB_DAY_FILE` when set, otherwise from
`<bob-dir>/YYYY/YYYYMMDD.md` using `BOB_NOW` or the local date. Within its
`Pomodoros` section, capture prefers the single open top-level entry with a
recognized bold or legacy time range; when there is no timed entry, it uses the
first open top-level entry. Completed and nested entries are ignored. Multiple
open timed entries are treated as an invariant error. The link is inserted
after the selected entry's existing children and reuses their indentation when
possible.

The routed note and daily note are both parsed and validated before either is
replaced. A missing daily note or Pomodoros section, no eligible entry, timed
ambiguity, malformed marker, or duplicate block ID leaves both notes unchanged.
`--dry-run` performs the same validation and reports both planned edits without
writing either file.

Append `#<section-prefix>` or a bare `#` to an `@route` token, as in
`@notes#Ideas` or `@notes#`, to capture an ordinary Markdown bullet instead of
a task. It renders as `- <text> [created::YYYY-MM-DD]` and is placed in a
non-`Tasks` section whose heading title starts with the prefix (compared case
insensitively), or any non-`Tasks` section when the marker is a bare `#`. A
matching non-H1 section is preferred; a matching H1 heading is used only when no
non-H1 heading matches. If no heading matches, the bullet goes into the
pre-heading (zeroth) section. Within the chosen section the bullet is inserted
after the last existing top-level bullet, otherwise just below the heading (or
after any YAML frontmatter for the zeroth section). The suffixed route token may
lead or trail the body, so `@notes#Ideas jot idea` and `jot idea @notes#Ideas`
both capture into `notes.md`. Standalone terminal `#...` markers, such as
`note #Ideas @foo` or `note @foo #`, are no longer accepted and fail with a
usage error.

A `--route` target keeps `@tokens` literal. Add `--section TITLE` with
`--route` to force bullet mode and place the bullet in a non-`Tasks` heading
whose title matches `TITLE` exactly, compared case insensitively. This exact
section path is intended for picker integrations; typed `@route#prefix` tokens
keep the prefix-matching behavior described above. Without `--section`,
`--route` captures a task.

Useful options:

- `-b, --bob-dir DIR`: Bob vault root; defaults to `BOB_DIR` or `~/bob`
- `-d, --dry-run`: parse, format, and report without writing
- `-f, --format human|json`: human confirmation or stable JSON for callers
- `-r, --route NAME`: force `NAME.md` and keep any `@tokens` in the text literal
- `-s, --section TITLE`: with `--route`, force a bullet into the exact section

If `TEXT` is omitted and stdin is piped, `bob capture` reads one line from
stdin. Put options before text, or use `--` when the task itself starts with a
hyphen. Hammerspoon integrations should call
`bob capture --format json -- <text>` and parse the JSON object, whose stable
fields include `ok`, `dry_run`, `routed`, `route`, `route_label`,
`relative_target`, `target`, `text`, `task_line`, `kind`, `created`, and
`placement`. The `kind` field is `"task"` or `"bullet"`, and `task_line` holds
the rendered line for either kind. On JSON-mode failures, stdout is still a
single object with `ok: false` and an `error` string.

Pomodoro-linked results use kind `"pomodoro_task"` and additionally include
`block_id`, `day_file`, `block_link`, and `pomodoro_link_placement`. Ordinary
capture JSON remains unchanged.

The Hammerspoon panel opened by `cmd+shift+ctrl+i` also supports incomplete
trailing markers. Use `<task> @:` to choose an area or project and then enter a
block ID, `<task> @route:` to prompt only for the block ID, or
`<task> @:block-id` to prompt only for the destination. A complete
`<task> @route:block-id` request captures immediately. The panel validates
each supplied or prompted component, emits only the canonical colon marker,
and retains staged values when validation or capture fails. Existing `@`,
`@#`, and `@route#` picker flows are unchanged.

```bash
bob nightly
```

Runs the nightly maintenance sequence. It performs one shared
`ob sync --path <vault>` gate first, then runs `bob move-done-tasks` and
`bob bulk-git-commit` in order. A failed Obsidian sync aborts before the wrapped
steps touch the vault; a failed wrapped step is reported but does not prevent
later wrapped steps from running.

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
each result by heading. JSON task records include parsed metadata, status and
priority, source location and hierarchy, dependencies, blocked/blocking state,
and urgency. This command does not run `ob sync`; vault freshness is handled by
the external background or cron sync path. Use `--engine obsidian` when you want
exact behavior from the live Dataview plugin in an open Obsidian vault; Tasks
inputs remain native-only, with an env-gated live renderer harness for parity
checks.

The full command contract and live smoke-test steps live in
[`docs/dataview.md`](docs/dataview.md).

```bash
bob mark-next-tasks [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json]
```

Synchronizes the vault's `[*]` Next tasks from block links beneath open
Pomodoros in today's daily note. A linked `[ ]` task becomes `[*]`; an
unlinked `[*]` task becomes `[ ]`. It also embeds links to completed Tasks
tasks and moves their containing bullet beneath the current timed Pomodoro, or
the last completed Pomodoro when there is no current one. In-progress `[/]`,
completed, canceled, unknown, and non-Tasks checkbox statuses are not changed.

For example, this open ledger entry makes the linked task Next:

```markdown
- [ ] Work session (0900-0930)
  - [[Projects/Alpha#^ship-design]]
```

Run `bob mark-next-tasks --dry-run` to preview every promotion, clear, embed,
and move. The command refuses to change files if the daily note is missing,
lacks a `Pomodoros` section, or has multiple open timed Pomodoros. The full
sync, link-resolution, exclusion, output, and JSON contract lives in
[`docs/mark-next-tasks.md`](docs/mark-next-tasks.md).

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

Task dependency metadata has a separate vault-wide identity from its Obsidian
block link. A task at `projects/foo.md#^abc123` uses
`[id:: projects__foo__abc123]`, and dependents use the same value in
`[dependsOn:: projects__foo__abc123]`; the trailing block token remains
`^abc123`. When a task moves, the command rewrites its `[id::]` to the archive
path/final block ID and repairs exact dependency tokens across all planned
files. Metadata and link repair share the same atomic preview/write plan.

The command itself does not run `ob sync`; `bob nightly` runs the shared
Obsidian sync gate before invoking it. In a Git worktree, the command stages
only the files it touches, commits with a `bob move-done-tasks YYYY-MM-DD`
message, and pushes. Existing uncommitted changes in touched source, archive,
or link-repair files are included in that scoped commit after the command
rewrites those files. Non-Git vaults are left uncommitted.

```bash
bob projects list [-b|--bob-dir DIR]
bob projects sync [-b|--bob-dir DIR] [-d|--dry-run]
```

Scans the Bob vault for notes whose frontmatter declares
`type: "[[project]]"` and prints a read-only overview of each project note. The
table includes frontmatter status, open `#task` count, open P0 task count, and
the state of the project completion task anchored with `^prj`. `sync` makes
that `^prj` task the lifecycle control: checking it sets `status: done`,
canceling it sets `status: canceled`, reopening it on a `done` or `canceled`
project sets `status: wip`, and active projects with no open P0 tasks
get `[scheduled::YYYY-mm-dd]` appended to the open `^prj` task. Use
`--dry-run` to preview the exact actions without writing.

The full project task contract lives in [`docs/projects.md`](docs/projects.md).

```bash
bob plugins [-b|--bob-dir DIR] [-f|--format table|json] [-n|--no-pull] [-r|--repo DIR]
bob plugins list [-b|--bob-dir DIR] [-f|--format table|json] [-n|--no-pull] [-r|--repo DIR]
bob plugins sync [-b|--bob-dir DIR] [-d|--dry-run] [-F|--force] [-n|--no-pull] [-p|--plugin ID] [-r|--repo DIR]
```

Lists Bryan's custom Bob Obsidian plugins from the
[`bobs-org/bob-plugins`](https://github.com/bobs-org/bob-plugins) repo and
annotates each with live vault state. Running `bob plugins` with no subcommand
runs `list`. The read-only table reads every `<repo>/plugins/<id>/manifest.json`
for the plugin id, version, and description; byte-compares the managed files
(`manifest.json`, `main.js`, and `styles.css` when present) against the vault
copy to report a `SYNC` state of `synced`, `drift`, or `missing`; and reads the
vault's `community-plugins.json` to report a `VAULT` state of `enabled`,
`disabled`, or `not installed`. A header names the repo and plugin count, and a
footer summarizes `N synced · M drift · K not installed`.

The repo root resolves from `-r, --repo`, then `BOB_PLUGINS_DIR`, then the
default `~/projects/github/bobs-org/bob-plugins`. The vault root resolves from
`-b, --bob-dir`, then `BOB_DIR`, then `~/bob`. By default, `list` and `sync`
run a non-interactive `git pull` in the plugins repo before analysis; pass
`-n, --no-pull` to use the current checkout. Non-Git repos skip the pull
silently, and pull failures warn on stderr but continue with the existing
checkout. `list` exits non-zero only on a real error such as an unreadable repo;
drift and not-installed plugins are reported, not failures. Pass
`-f, --format json` for a stable object with `ok`, `repo`, `bob_dir`, `count`,
`synced`, `drift`, `not_installed`, and a `plugins` array whose entries carry
`id`, `version`, `description`, `sync`, and `vault`.

`sync` deploys the repo into the vault, copying the managed files
(`manifest.json`, `main.js`, and `styles.css` when present) into each
`<bob-dir>/.obsidian/plugins/<id>/` while never touching runtime files such as
`data.json`. It reports each file as copied, unchanged, or skipped. A vault file
with uncommitted changes in the vault Git repo is skipped with a warning so
local edits are never clobbered silently; pass `-F, --force` to overwrite it.
Use `-d, --dry-run` to preview and `-p, --plugin <ID>` to sync a single plugin.

The full command contract lives in [`docs/plugins.md`](docs/plugins.md).

```bash
bob highlights doctor
bob highlights marker <pdf>
bob highlights scan [-d|--dry-run] [-j|--jobs N] [-w|--write-pdfs]
bob highlights sync <pdf> [-d|--dry-run] [-w|--write-pdf] [-p|--prefer marker|frontmatter]
```

Prepares the Highlights app PDF annotation to Bob reference note sync workflow.
`sync <pdf>` reads the first standalone `/Text` PDF note annotation on page 1
as the marker note, parses its `key: value` list, and creates or updates
`ref/<ref_type>/<pdf-basename>.md` frontmatter and the managed Highlights body
region for PDFs under `lib/<ref_type>/`. Top-level library PDFs and explicit
out-of-library syncs keep the legacy `ref/<pdf-basename>.md` target. It stores a
canonical marker projection hash so later runs can sync marker-only edits into
frontmatter and, when `--write-pdf` is supplied, frontmatter-only edits back
into the PDF marker. Simultaneous marker/frontmatter edits fail with a conflict
report unless `--prefer marker` or `--prefer frontmatter` is supplied.
`--dry-run` reports the planned note/PDF actions without writing either side.
`marker <pdf>` inspects and renders the marker contract without writing. `scan`
recursively processes PDFs under the configured library with collision and
dirty-target preflights. By default scan does not write PDF markers; use
`scan --dry-run --write-pdfs`, review the planned marker updates, back up PDFs,
then run `scan --write-pdfs` to opt in to bulk marker write-back. `doctor`
checks vault paths, sidecars, marker readability, Git state, and optional `ob`
availability without writing files.
Marker notes must include `status` and `parent`; marker `parent` must be a bare
note target such as `obsidian`, while generated reference-note frontmatter
renders it as an Obsidian wikilink. `status` must be one of `unread`, `wip`,
`read`, `abandoned`, or `legacy`. Existing `status: done` marker/frontmatter
inputs are treated as a deprecated alias and normalized to `read` during sync.
Existing migrated notes may also carry `legacy_status` frontmatter to preserve
the previous value; it is not a standard marker-synced field. Generated
reference notes always include `type: "[[ref]]"` and include command-managed
`ref_type` when it can be derived from the first library path component.
The generated PDF task line is a status affordance: `[x]`/`[X]` contributes
`status: read`, `[-]` contributes `status: abandoned`, and unchecking it on an
already `read` or `abandoned` ref reopens it to `status: wip` (a freshly
generated `[ ]` task contributes no replacement status). PDF marker write-back
for task-derived status still requires targeted `--write-pdf` or reviewed bulk
`scan --write-pdfs`.

For `lib/books/foo.pdf`, `sync` discovers `lib/books/foo.md` first and can
parse simple `foo.textbundle/text.md` or `text.markdown` sidecars. Image and
area annotations require a TextBundle sidecar (`foo.textbundle/text.md` plus
`foo.textbundle/assets/`) beside the PDF with the matching basename. Highlights
sometimes fails to create that bundle the first time, so manually export it once
and verify with `bob highlights scan --dry-run`; see
[`docs/highlights-ref-sync.md`](docs/highlights-ref-sync.md) for the workaround.
Highlights,
highlight comments, and standalone non-marker notes render into the managed
`<!-- highlights:begin -->` region using stable `^h-...` block IDs. Existing
manual body content outside that region is preserved, and disappeared generated
blocks are kept as tombstones under `### Removed highlights`. Markdown bullet
lines tagged with `#task` inside highlight comments or standalone non-marker
notes also create Obsidian tasks when the marker/frontmatter-selected PDF status
is `wip`, before the generated PDF `^ref` checkbox contributes a closing
`read` or `abandoned` status. That final closing run still imports newly added
annotation tasks, and a run that reopens a `read` or `abandoned` ref to `wip`
imports them too; later runs whose selected status is already non-`wip` skip
task intake. By default, tasks are inserted immediately under the generated PDF
`^ref` line. If the final whitespace-delimited task token is a strict `@name`
route suffix (`A-Z`, `a-z`, `0-9`, `_`, and `-`, starting with an
alphanumeric), the suffix is stripped and the task is appended to existing
`~/bob/name.md` instead; routed target notes are never auto-created. Created
tasks carry `[created::YYYY-MM-DD]`, `[h:: ...]`, and a `🔖` backlink to the
annotation-level generated source block such as `[[#^h-...|🔖]]` or
`[[ref/books/foo#^h-...|🔖]]`. The `[h:: ...]` property is the durable processed
marker that moves with completed, cancelled, edited, or `bob move-done-tasks`
archived tasks, so re-syncing does not recreate them without PDF or sidecar
edits. Older `[highlight_task:: ...]` fields and old `#^ht-...` source links
are still recognized for compatibility, but new tasks no longer write them.

The full contract and MacBook setup guide live in
[`docs/highlights-ref-sync.md`](docs/highlights-ref-sync.md).

```bash
bob pomodoro
```

Prints the current Pomodoro ledger entry from today's Bob daily note, including
time remaining or recent overdue status. It defaults to
`$BOB_DIR/YYYY/YYYYMMDD.md`, or `~/bob/YYYY/YYYYMMDD.md` when `BOB_DIR` is
unset, unless `BOB_DAY_FILE` is set.
Ledger entries may use bold Markdown ranges such as
`(**0945-1015** [t:: 30m])`; command output remains plain, for example
`0945-1015 Review crate skeleton`.

```bash
bob notify PRE_CHECK_SLEEP POST_NOTIFY_SLEEP
```

Polls `bob_pomodoro` until the current Pomodoro is overdue, then sends a desktop
notification when `notify-send` is available and rings the terminal bell.

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

- `ob` from obsidian-headless for the shared `bob nightly` Obsidian sync gate
- `obsidian` CLI plus a running desktop Obsidian vault with the Dataview plugin
  only when using `bob query --engine obsidian`
- `git` and `ssh` for `bob bulk-git-commit` and for `bob move-done-tasks`
  commit/push behavior when the vault is a Git worktree
- `notify-send` for desktop notifications from `bob notify`
- `bash` only when loading `ob` through the NVM fallback or sourcing
  `~/.ssh-agent-thing`

No old chezmoi script files are required after installation. Cargo installs the
Rust binaries, and the binaries carry the script assets they need.

## Environment

`BOB_DIR` sets the Bob vault directory. It defaults to `~/bob`.

`BOB_DATAVIEW_OBSIDIAN_COMMAND` overrides the executable used by
`bob query --engine obsidian`.

`BOB_DATAVIEW_VAULT` sets the default Obsidian vault name or ID forwarded to
`obsidian eval` by `bob query --engine obsidian`.

`BOB_DAY_FILE` sets the exact daily note path used by `bob pomodoro`,
Pomodoro-linked `bob capture` requests, and `bob mark-next-tasks`.

`BOB_NOW` sets the current timestamp for Pomodoro status, the `bob capture`
`[created::YYYY-MM-DD]` stamp, and default runtime note selection, including
the daily note used by `bob mark-next-tasks`. It also controls the default
`bob move-done-tasks YYYY-MM-DD` commit message date.
Supported formats include `YYYY-MM-DD`, `YYYY-MM-DD HH:MM`, and
`YYYY-MM-DD HH:MM:SS`.

`BOB_HIGHLIGHTS_LIB_DIR` sets the Highlights PDF library directory used by
`bob highlights`. It defaults to `lib` under `BOB_DIR`. Relative values are
resolved under the Bob vault; absolute paths and `~/...` paths are used as
configured.

`BOB_HIGHLIGHTS_REF_DIR` sets the generated reference note directory used by
`bob highlights`. It defaults to `ref` under `BOB_DIR`.

`DATE` preserves the legacy date override behavior, including the date used by
`bob capture` when `BOB_NOW` is unset. It can be a date command prefix such as
`date --utc`, or a timestamp in the same formats accepted by `BOB_NOW`.

`OB_COMMAND` overrides the `ob` executable used by the shared `bob nightly`
Obsidian sync gate.

`BOB_BULK_GIT_COMMIT_LOCK_FILE` overrides the lock path used by
`bob bulk-git-commit` and `bob nightly`.

`BOB_BULK_GIT_COMMIT_MESSAGE` overrides the commit message used by
`bob bulk-git-commit`.

`BOB_SYNC_LOCK_FILE` is a deprecated compatibility alias for
`BOB_BULK_GIT_COMMIT_LOCK_FILE`.

`BOB_SYNC_COMMIT_MESSAGE` is a deprecated compatibility alias for
`BOB_BULK_GIT_COMMIT_MESSAGE`.

`BOB_CLI_USE_SCRIPT=1` forces the embedded shell fallback implementation.

## Migration Notes

Use `bob pomodoro`, `bob notify`, `bob bulk-git-commit`, and
`bob tmux-pomodoro` for new integrations, and run `bob move-done-tasks` when
done and canceled task blocks should be archived from the vault.

The old top-level commands were renamed: `bob collect-done` is now
`bob move-done-tasks`, `bob dataview` is now `bob query`, `bob highlights-ref`
is now `bob highlights`, and `bob sync` is now `bob bulk-git-commit`. The old
top-level names are no longer registered. Legacy installed binaries such as
`bob_sync` remain compatibility shims for existing callers.

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
"$root/bin/bob" --help
"$root/bin/bob" bulk-git-commit --help
"$root/bin/bob" capture --help
"$root/bin/bob" query --help
"$root/bin/bob" highlights --help
"$root/bin/bob" move-done-tasks --help
"$root/bin/bob" nightly --help
"$root/bin/bob" notify --help
"$root/bin/bob" pomodoro --help
"$root/bin/bob" projects --help
"$root/bin/bob" projects sync --help
"$root/bin/bob" tmux-pomodoro --help
"$root/bin/bob_notify" --help
"$root/bin/bob_pomodoro" --help
"$root/bin/bob_sync" --help
"$root/bin/tmux_bob_pomodoro" --help
```

Run a tmux status smoke test after installing locally:

```bash
tmux display-message -p '#(bob tmux-pomodoro)'
```

Before running `bob bulk-git-commit` in a release smoke test, verify that
`BOB_DIR` points at the intended vault and that its Git remote can be pushed
without prompts. Before running `bob move-done-tasks` against the real vault,
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

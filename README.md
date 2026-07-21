# Bob CLI

`bob-cli` installs the `bob` command and compatibility shims for the Bob Obsidian
vault and Pomodoro workflow. Command implementations are native Rust by default.
The Pomodoro, notification, and legacy `bob_sync` shell implementations remain
embedded as a targeted rollback path; see [Compatibility shims](#compatibility-shims)
for the exact mappings and fallback behavior.

The preferred interface is `bob <subcommand>`. Legacy command names still exist
as installed binaries for existing tmux, shell, and automation callers.

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

## Commands

`bob --help` is the authoritative command index. Bob's workflow commands are:

| Command | Purpose |
| --- | --- |
| `bulk-git-commit` | Stage, commit, and push all Bob vault changes |
| `capture` | Capture a task or section bullet, optionally with clipboard content |
| `capture-sections` | List the non-`Tasks` headings in a routed note |
| `capture-targets` | List inbox, area, and non-terminal project capture routes |
| `highlights` | Synchronize Highlights PDF annotations with reference notes |
| `move-done-tasks` | Archive done and canceled task blocks and repair their links |
| `nightly` | Run the Obsidian sync and maintenance workflow |
| `notify` | Notify when the current Pomodoro finishes |
| `plugins` | List and deploy Bob's custom Obsidian plugins |
| `pomodoro` | Print the current Pomodoro status |
| `projects` | Inspect and synchronize project lifecycle tasks |
| `query` | Run headless Dataview or Tasks queries, or live Dataview queries |
| `task-status-hooks` | Reconcile Pomodoro links, task ranks, and dependency state |
| `tmux-pomodoro` | Print Pomodoro status for a tmux status line |

Use `bob <command> --help` for concise usage. The sections below explain the
workflow and link to the detailed command contracts where one exists.

```bash
bob bulk-git-commit
```

Stages all Bob vault changes, commits them when anything changed, and pushes via
Git. This command does not run `ob sync`; use `bob nightly` for the nightly path
that syncs Obsidian before maintenance steps. `bob bulk-git-commit` mutates the
vault repository and should only be run when its Git remote and required
credentials are ready.

```bash
bob capture [OPTIONS] [--] [TEXT]...
```

Captures one task or ordinary Markdown bullet into the Bob vault without
requiring desktop Obsidian to be open. Input whitespace is normalized to one
line. Task mode writes `- [ ] #task <text> [created::YYYY-MM-DD]` and routes to
`mac_inbox.md` by default; bullet mode writes into a selected non-`Tasks`
section as described below. The created date uses the local date from
`BOB_NOW`, `DATE`, or the system clock.

Automatic routing matches the Hammerspoon capture keymap: a leading
`@route text` prefix wins, otherwise a trailing `text @route` suffix is used.
Route names use `A-Z`, `a-z`, `0-9`, `_`, and `-`, are lower-cased, and write
to `<route>.md` at the vault root. Existing target files, including
`mac_inbox.md`, prefer a Markdown `Tasks` section: new captures insert after
the last top-level `#task` block in that section, or after one blank line below
the `Tasks` heading when the section has no tasks yet. Files without a `Tasks`
section keep the older fallback of inserting after the last top-level `#task`
block and its indented continuation lines, or appending at EOF.

Append a lowercase `s:<N>` token to schedule the capture `N` days from today.
It is recognized only in the terminal token region and may appear on either
side of a trailing route marker. The token is removed from the body and adds
`[scheduled::YYYY-MM-DD]` after the created stamp.

Append one of these whitespace-delimited terminal markers to capture clipboard
content beneath the new task or bullet:

- `%` captures the live clipboard once without a header.
- `%<positive integer>` captures exactly that many values without headers: the
  live clipboard first, followed by recent history newest first. For example,
  `bob capture research links %3` captures three values. `%1` is equivalent to
  `%`, leading zeroes are accepted, and `%0` stays literal.
- `%<nonnumeric header>` captures the live clipboard once under an explicit
  header. Headers accept letters, digits, `_`, and `-`, render in uppercase,
  and replace underscores with spaces; for example, `%build_log` renders
  `**BUILD LOG:**`.

The marker composes with `s:<N>`, ordinary routes, bullet routes, and Pomodoro
routes in either terminal order. Invalid `%...` tokens and `%` tokens in the
middle of the body stay literal. A counted capture requires every requested
entry to read, normalize, classify, and plan successfully; insufficient or
invalid history aborts the capture instead of writing a partial result.

Clipboard content is rendered according to its shape:

- One text line up to 1,000 characters becomes an inline child bullet.
- Two to ten flat text lines become child bullets, nested beneath an explicit
  header when one is present.
- One to ten top-level unordered Markdown list items using `-`, `*`, or `+`
  become child bullets. Their source list markers and separating whitespace are
  removed while inline Markdown, including checkbox text, is preserved.
- Absolute file paths (including quoted paths, `file://` URIs, and `~/...`)
  become attachments. Images are copied to `img/` and embedded at 400px;
  other files are copied to `file/` and linked.
- Long, indented, blank-line-separated, or other Markdown-structured text is
  saved verbatim as `file/clip-YYYYMMDD-HHMMSS[-slug].md` and linked without
  the `.md` suffix. Ordered, nested, wrapped, mixed, or empty-item lists use
  this snippet fallback instead of being partially normalized.

Each value in a counted history capture is classified independently, so limits
such as the ten-attachment maximum apply per entry. All resulting lines are
flattened in source order as direct, headerless children; entries receive no
index labels, container bullets, or separators.

Without a header, one item is written as a direct child and multiple items are
written as direct sibling children:

```markdown
- [ ] #task Parent
  - clipboard text
- [ ] #task Another parent
  - first line
  - second line
```

For example, a clipboard containing this flat Markdown list:

```markdown
- first copied item
* second item with **inline Markdown**
+ [ ] third checkbox item
```

is normalized beneath the captured parent without doubling the source markers:

```markdown
- [ ] #task Parent
  - first copied item
  - second item with **inline Markdown**
  - [ ] third checkbox item
```

An explicit header stays inline for one item and owns a nested list for
multiple items:

```markdown
- [ ] #task Parent
  - **BUILD LOG:** clipboard text
- [ ] #task Another parent
  - **BUILD LOG:**
    - first line
    - second line
```

Attachment names are sanitized for Obsidian links. An existing identical file
is reused; differing content receives an eight-character SHA-256 suffix. Up to
ten attachment paths may be pasted at once. Clipboard text must be non-empty
UTF-8 without NUL bytes; binary clipboard contents should be represented by a
copied file path. Clipboard and note edits are planned before anything is
written, and newly created clipboard files are removed if the note write fails.
`--dry-run` performs the same planning but creates no directories or files.

Use `-c, --clip[=HEADER]` to force clipboard capture without a marker. Bare
`--clip` captures without a header, while `--clip=build_log` supplies an
explicit header. Both forms force a single live value and keep `%` tokens in
the captured text literal. A numeric header can be requested unambiguously with
`--clip=20`; use `-n, --no-clip` when a genuine trailing `%N` or other `%...`
token should remain literal. `--clip` and `--no-clip` conflict.

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
- `-c, --clip[=HEADER]`: force clipboard capture, optionally with a header
- `-d, --dry-run`: plan and report without writing notes or clipboard files
- `-f, --format human|json`: human confirmation or stable JSON for callers
- `-n, --no-clip`: keep trailing `%...` clipboard markers literal
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

Clipboard captures additionally include a `clip` object. Single captures keep
the existing shape: `header`, `mode` (`"inline"`, `"lines"`, `"attachments"`,
or `"snippet"`), `lines` (the exact rendered child lines), and `attachments`.
Each attachment has `source`, vault-relative `saved`, `kind` (`"image"` or
`"file"`), and `reused` fields. Snippet results also include the vault-relative
`snippet` path. The `header` value is `null` when the capture omitted a header
and is the rendered string (for example, `"BUILD LOG"`) when one was explicit.

Counted histories above one use `mode: "history"`, `header: null`, flattened
`lines`, and attachment records aggregated in entry order. Their `entries`
array contains one ordinary headerless clip object per requested value, keeping
entry boundaries and any owning `snippet` path explicit. The aggregate omits a
singular `snippet` field. `%1` uses the unchanged single-capture shape.
`task_line` remains the parent line only, and non-clipboard JSON omits `clip`.

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
bob capture-sections --route NAME [-b|--bob-dir DIR] [-f|--format human|json]
bob capture-targets [-b|--bob-dir DIR] [-f|--format human|json] [-v|--verbose]
```

These read-only discovery commands support interactive capture pickers. A
*route* is the canonical lowercase name for a `<route>.md` note at the vault
root; for example, route `cash` selects `cash.md`. The command-line route
options accept ASCII uppercase too and normalize it to lowercase. A picker
normally uses the commands in this order:

1. Run `capture-targets` and let the user choose a route.
2. For a bullet capture, run `capture-sections` for that route and let the user
   choose a heading. A task capture skips this step.
3. Run `bob capture --route NAME --section TITLE -- <text>` for a bullet, or
   omit `--section` for a task.

On a successful scan, `capture-targets` returns `mac_inbox` first even when
`mac_inbox.md` does not exist, followed by top-level area notes and
non-terminal project notes, with each group sorted by route. Eligible note
filenames must already be lowercase and may contain only ASCII letters,
digits, `_`, and `-`. Area and project classification comes from YAML
frontmatter `type: "[[area]]"` or `type: "[[project]]"`; the equivalent bare
values are also accepted. Nested notes, projects whose status is `done`,
`canceled`, or `cancelled` (case-insensitively), and other note types are
omitted. Human output groups routes by kind. JSON output has `ok`, `bob_dir`,
`count`, and an ordered `targets` array; each target has `route`, `name`,
`label`, `kind`, `is_default`, `status`, and `relative_path`. `--verbose`
reports top-level Markdown files omitted because their filename is not a valid
route; other omissions remain silent.

`capture-sections` lists each parsed ATX heading (H1-H6) except a heading
titled exactly `Tasks`, in document order. It ignores headings in YAML
frontmatter and fenced code blocks. Route input is normalized to lowercase,
and a missing note successfully returns an empty list. JSON output has `ok`,
the normalized `route`, `count`, and an ordered `sections` array whose entries
each have `title` and `level`.

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
bob task-status-hooks [-b|--bob-dir DIR] [-d|--dry-run] [-f|--format human|json]
```

Synchronizes active task statuses from block links beneath open Pomodoros in
the current daily ledger. It also reads the latest existing earlier canonical
daily note, searching across missing days, weeks, and year boundaries, as a
read-only recent-activity source. A directly linked `[ ]` task becomes Next
(`[*]`), while a direct task already In Progress (`[/]`) keeps that stronger
status. Sole
transcluded task dependencies inherit their parent's effective status
recursively: Next promotes Ready to Next, and In Progress promotes Ready or
Next to In Progress. Multiple paths use the strongest request, stronger
intermediate tasks pass their status to descendants, and propagation never
lowers a task. Separately, an unreachable `[*]` task becomes `[ ]`, preserving
the command's vault-wide Next clearing policy. An In-Progress `[/]` task in a
note whose frontmatter type is exactly `[[area]]` or `[[project]]` becomes
Ready when neither daily source nor their eligible dependency closure reaches
it; a missing block ID is stale by definition. Historical links protect
existing In-Progress state but never promote Ready tasks, and the historical
daily is never written. Ordinary notes, daily notes, terminal/custom statuses,
and other checkbox states remain outside this rollback. Vault-wide Tasks
`[id:: ...]`/`[dependsOn:: ...]` metadata is reconciled independently: any
recognized open parent with an open dependency becomes Blocked (`[?]`), and a
no-longer-blocked task returns to its final Pomodoro-derived rank or Ready.
Blocked tasks with no dependency metadata recover on this whole-vault pass;
Done, canceled, non-task, and unknown parents remain untouched. Blocked writes
require a single compatible `Blocked`/`?`/`ON_HOLD` Tasks registry entry and
fail before any note write when that contract is absent or incompatible. It
also retires links to completed Tasks tasks as `~~[[...]]~~` and moves bullets
found beneath open Pomodoros to the current timed Pomodoro, or the last
completed Pomodoro when there is no current one. It also marks live
non-transcluded links beneath completed Pomodoros, keeps embedded links
unmarked, preserves the marker provenance of already-struck history, and
removes stray markers beneath open Pomodoros. A block link beneath an open
Pomodoro whose every matching Tasks task has a recognized `CANCELLED` status
removes its complete Markdown list-item subtree, including authored prose,
sibling links, and nested content. Plain, embedded, aliased, Pomodoro-marked,
and exactly struck occurrences qualify, including custom single-character
cancellation symbols, without changing the canceled task. Links on top-level
Pomodoro lines, beneath completed or canceled Pomodoros, in fenced examples,
unresolved links, and mixed-status duplicate block IDs are retained. Duplicate
physical-line cleanup takes precedence, and the rewritten current ledger
drives status and dependency propagation in the same run so collateral
references removed with the item no longer contribute.
Completed, canceled, unknown, and non-Tasks checkbox statuses are not changed.
When multiple open Pomodoros link the same resolved task, the earliest open
Pomodoro keeps it and matching physical lines beneath later open Pomodoros are
removed in full. Identity is the resolved note path plus block ID, so aliases,
embeds, and alternate note spellings are de-duplicated together; repeats within
one Pomodoro are preserved.

For example, this open ledger entry gives the linked task and its eligible
dependency chain a minimum desired status of Next:

```markdown
- [ ] Work session (0900-0930)
  - [[Projects/Alpha#^ship-design]]
```

Run `bob task-status-hooks --dry-run` to preview every Next or In-Progress
promotion, Next clear, scoped In-Progress clear, Blocked transition, unblock,
duplicate-line removal, canceled-reference list-item removal, retirement,
move, and Pomodoro-marker repair. The command refuses to change files if the
current daily note is missing, lacks a `Pomodoros` section, or has multiple
open timed Pomodoros. The full
sync, link-resolution, exclusion, output, and JSON contract lives in
[`docs/task-status-hooks.md`](docs/task-status-hooks.md).

Task Status Cycler's Ctrl+Enter path performs a narrower immediate recovery:
when the keypress closes a dependency's final recognized open instance, an
affected Blocked dependent becomes Ready. It preserves terminal and unrelated
tasks and leaves Ready/Next/In-Progress ranking to the next authoritative
`bob task-status-hooks` pass. The hidden `task-status-setter` and
`mark-next-tasks` spellings remain compatibility-only aliases and are not
listed in top-level help.

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
table includes frontmatter status, open `#task` count, open non-hidden ordinary
`#task` count, and the state of the project completion task anchored with
`^prj`. The `^prj` task is the lifecycle control: checking it sets `status:
done`, canceling it sets `status: canceled`, and reopening it changes either
terminal status back to `status: wip`. An open `^prj` on a project that is
already non-terminal leaves its existing status unchanged.

For a non-terminal project, `sync` uses `#hide` to show `^prj` in `dash.md`'s
Tasks query only when no other open, non-hidden `#task` and no open sub-project
exists. It also maintains the machine-owned Sub-projects ledger beneath
`^prj`. Run `projects list` to inspect the current state, `projects sync
--dry-run` to preview reconciliation, and `projects sync` to apply it.

An optional frontmatter `scheduled: YYYY-MM-DD` overrides normal task
visibility for a non-terminal project. A future date adds `#hide` to every
Markdown task. On the scheduled date and afterward, `sync` removes `#hide`
from every task except `^prj`; `^prj` keeps its existing visibility when the
note contains another task, and is unhidden when it is the only task. A stale
inline `[scheduled:: ...]` field is removed from an open `^prj` task. The full
project task contract lives in [`docs/projects.md`](docs/projects.md).

```bash
bob plugins [-b|--bob-dir DIR] [-f|--format table|json] [-n|--no-pull] [-r|--repo DIR]
bob plugins list [-b|--bob-dir DIR] [-f|--format table|json] [-n|--no-pull] [-r|--repo DIR]
bob plugins sync [-B|--backup-dir DIR] [-b|--bob-dir DIR] [-d|--dry-run] [-F|--force] [-n|--no-pull] [-p|--plugin ID] [-r|--repo DIR]
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
Before overwriting an existing file, `sync` copies it into a timestamped backup
directory; `-B, --backup-dir` selects the backup base directory. Use `-d,
--dry-run` to preview diffs, skip decisions, and backup paths before writing,
and `-p, --plugin <ID>` to sync a single plugin.

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
renders it as an Obsidian wikilink. `status` must be one of `ready`, `next`,
`wip`, `read`, `abandoned`, or `legacy`. Existing `status: unread` and
`status: done` marker/frontmatter inputs are deprecated aliases and normalize to
`ready` and `read`, respectively, during sync. Existing migrated notes may also
carry `legacy_status` frontmatter to preserve the previous value; it is not a
standard marker-synced field. Generated reference notes always include
`type: "[[ref]]"` and include command-managed `ref_type` when it can be derived
from the first library path component.

The generated PDF `^ref` task is the visible lifecycle control: `[ ]` maps to
`ready`, `[*]` to `next`, `[/]` to `wip`, `[x]`/`[X]` to `read`, and `[-]` to
`abandoned`. Moving a terminal task back to `[ ]` therefore reopens it to
`ready`; a missing `^ref` task contributes no lifecycle status. PDF marker
write-back for task-derived status still requires targeted `--write-pdf` or
reviewed bulk `scan --write-pdfs`.

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

### Compatibility shims

The installed legacy binaries map to the preferred interface as follows:

| Compatibility binary | Preferred command |
| --- | --- |
| `bob_notify` | `bob notify` |
| `bob_pomodoro` | `bob pomodoro` |
| `bob_sync` | `bob bulk-git-commit` |
| `tmux_bob_pomodoro` | `bob tmux-pomodoro` |

By default they call the same native Rust implementations as the preferred
commands. With `BOB_CLI_USE_SCRIPT=1`, the notification and Pomodoro commands
and their shims delegate to their embedded shell assets. The `bob_sync` shim
also delegates to its embedded script, but `bob bulk-git-commit` remains native.
Native-only commands ignore the fallback setting. Extracted assets are cached
in a version-and-content-specific subdirectory of
`$XDG_CACHE_HOME/bob-cli/scripts/`. If `XDG_CACHE_HOME` is unset or empty, the
base is `$HOME/.cache`; if neither variable is available, Bob uses the system
temporary directory.

## Runtime Dependencies

Native command execution does not require Bash or Perl. Forced shell fallback
with `BOB_CLI_USE_SCRIPT=1` requires Bash, and the Pomodoro-based fallback
scripts also require Perl.

The documented workflows use these external-tool integrations:

- `ob` from obsidian-headless for the shared `bob nightly` Obsidian sync gate;
  the gate is skipped when `ob` is unavailable
- `obsidian` CLI plus a running desktop Obsidian vault with the Dataview plugin
  only when using `bob query --engine obsidian`
- `git` for `bob bulk-git-commit`, Git-backed `bob move-done-tasks`, plugin
  dirty-file checks, and the default `bob plugins` repository refresh; remote
  operations also need the credentials required by the configured remote
- `notify-send` for desktop notifications from `bob notify`; Bob also rings the
  terminal bell whether or not `notify-send` is available
- platform clipboard tools for `bob capture` clipboard input: `pbpaste` on
  macOS; `wl-paste`, `xclip`, or `xsel` on Linux; or `tmux show-buffer` in a
  display-less tmux session (the next section gives the exact fallback order)
- `bash` for the embedded shell fallback, for loading `ob` through the NVM
  fallback, or for sourcing `~/.ssh-agent-thing`; the Pomodoro shell fallback
  additionally uses `perl`

No old chezmoi script files are required after installation. Cargo installs the
Rust binaries, and the binaries carry the script assets they need.

## Environment

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

`BOB_DIR` sets the Bob vault directory. It defaults to `~/bob`.

`BOB_DATAVIEW_OBSIDIAN_COMMAND` overrides the executable used by
`bob query --engine obsidian`.

`BOB_DATAVIEW_VAULT` sets the default Obsidian vault name or ID forwarded to
`obsidian eval` by `bob query --engine obsidian`.

`BOB_DAY_FILE` sets the exact daily note path used by `bob pomodoro`,
Pomodoro-linked `bob capture` requests, and `bob task-status-hooks`.

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

`BOB_HIGHLIGHTS_LIB_DIR` sets the Highlights PDF library directory used by
`bob highlights`. It defaults to `lib` under `BOB_DIR`. Relative values are
resolved under the Bob vault; absolute paths and `~/...` paths are used as
configured.

`BOB_HIGHLIGHTS_REF_DIR` sets the generated reference note directory used by
`bob highlights`. It defaults to `ref` under `BOB_DIR`.

`BOB_PLUGINS_DIR` sets the source repository used by `bob plugins`. It defaults
to `~/projects/github/bobs-org/bob-plugins`.

`BOB_PLUGIN_BACKUPS_DIR` sets the base directory for backups created before
`bob plugins sync` overwrites a vault plugin file. It defaults to
`~/.local/state/bob-cli/plugin-backups`.

`DATE` preserves the legacy date override behavior, including the date used by
`bob capture` when `BOB_NOW` is unset. It can be a date command prefix such as
`date --utc`, or a timestamp in the same formats accepted by `BOB_NOW`.

`OB_COMMAND` overrides the `ob` executable used by the shared `bob nightly`
Obsidian sync gate.

`NO_COLOR` disables ANSI color in native human-readable output that would
otherwise be styled when stdout is a terminal.

`BOB_BULK_GIT_COMMIT_LOCK_FILE` overrides the lock path used by
`bob bulk-git-commit` and `bob nightly`.

`BOB_BULK_GIT_COMMIT_MESSAGE` overrides the commit message used by
`bob bulk-git-commit`.

`BOB_SYNC_LOCK_FILE` is a deprecated compatibility alias for
`BOB_BULK_GIT_COMMIT_LOCK_FILE`.

`BOB_SYNC_COMMIT_MESSAGE` is a deprecated compatibility alias for
`BOB_BULK_GIT_COMMIT_MESSAGE`.

`BOB_CLI_USE_SCRIPT=1` selects an embedded shell implementation where one is
available. See [Compatibility shims](#compatibility-shims) for the exact command
coverage and cache location.

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

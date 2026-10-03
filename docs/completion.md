# Shell completion

Every <TAB> after `bob` is answered live by the `bob` on your `PATH`,
so completion always matches the installed binary: commands, options,
and vault values such as capture routes, tasks, and open Pomodoros.
Say "shell completion" (never just "completion") so it is never
confused with `capture-complete`.

## Overview

The `bob` binary is the grammar. Each <TAB> runs a hidden
`bob __complete` request against one composed clap tree plus bob's own
read-only, vault-aware value providers. The file installed into the
shell is a small, stable, protocol-versioned adapter that only renders
what bob returns. "Updating completion" means ensuring that adapter is
present and current, not regenerating anything.

```text
 zsh/bash ──TAB──▶ adapter (_bob / bob, written by `bob completion install`)
                     │  command bob __complete zsh --protocol 1 --suffix "$SUFFIX" -- <words…> "$PREFIX"
                     ▼
 bob __complete ─▶ early intercept in run_bob (nothing else runs)
                ─▶ completion::tree()            one exhaustive clap tree
                ─▶ engine.rs → clap_complete     subcommands, options, static choices
                ─▶ kinds + providers              routes, tasks, Pomodoros, capture markers …
                ─▶ present.rs                     presentation rules, human groups, directives
                ─▶ protocol.rs                    value\tdesc\tgroup\tsuffix | !prefix !dirs !files !message
```

Completion never writes: no vault writes, `git`, network, clipboard,
locks, script materialization, or state/log files. The only exception
is the explicitly requested `BOB_COMPLETE_DEBUG=<file>`.

## Protocol 1

Request:

```text
bob __complete <SHELL> --protocol <N> [--suffix <TEXT>] -- <WORD>...
```

- **`SHELL`** is `zsh` or `bash`. It changes only presentation.
- **`WORD`s** are the command line's words up to the cursor,
  shell-unquoted. The first word is the command as typed and is
  ignored. The last word is the cursor word's text _before_ the cursor,
  and may be empty.
- **`--suffix`** is the unquoted text of the cursor word _after_ the
  cursor. It is absent or empty when the cursor sits at the end of the
  word.
- **A malformed request** exits 2 with a message on stderr; adapters
  discard stderr.

Response. UTF-8 lines on stdout. Exit 0, including on internal
failure, which yields empty output.

- **Directives** are whole lines, emitted before any candidate:
  - `!prefix <N>`: keep the first N Unicode scalar values of the
    cursor-word prefix, and let candidates replace only the rest.
  - `!dirs`: complete directories natively.
  - `!files` / `!files <glob>`: complete files natively, optionally
    filtered.
  - `!files-in <root>\t<glob>`: complete paths relative to `root`
    (vault notes). The separator is a TAB, so roots may contain spaces.
  - `!message <text>`: show a hint and offer no candidates (free-text
    slots, version skew).
- **Candidate lines:** `value<TAB>description<TAB>group<TAB>space|nospace`.
  - `value` is never empty and never contains TAB, LF, or CR; bob
    drops such candidates.
  - `description` may be empty, and LF/CR/TAB become spaces.
  - An empty `group` means `values`.
  - `nospace` means the user is expected to keep typing.
- **Order is display order.** Groups display in order of first
  appearance, and adapters never re-sort.
- **Version skew.** The binary supports protocol 1 only.
  - An older adapter gets
    `!message bob shell completion is out of date — run: bob completion install`.
  - A newer adapter gets
    `!message this bob is older than its shell completion — reinstall bob (just install)`.
- **Debugging.** `BOB_COMPLETE_DEBUG=<file>` appends the request, the
  response, the elapsed milliseconds, and any error.
- **The shell filters, not bob.** Bob never prefix-filters within a
  slot: it resolves the slot from the cursor word and returns that
  slot's full candidate set. Attached `--opt=` values are served with
  `!prefix`.

## What completes

- **Commands.** Every `bob` subcommand under the `commands` group,
  then the ten `capture-*` frontend endpoints under
  `capture protocol`. Hidden aliases and the auto `help` command never
  appear.
- **Options.** A lone `-` offers short and long forms adjacent with
  identical descriptions; `--` offers long forms only. Options already
  present (unless repeatable), options conflicting with present ones,
  and `-h/--help` once other arguments exist are dropped. Once capture
  text has started, or after `--`, no options are offered.
- **Static choices.** Every option with clap possible values
  (`--format`, `--engine`, highlights `--status`/`--prefer`, gkeep
  `--source`, …) completes its values under a group named after the
  slot, such as `format`.
- **Paths.** Directory options (`--bob-dir`, `--repo`, `--backup-dir`,
  `--lib-dir`, `--ref-dir`, `--xlib-dir`, `query --vault`) answer
  `!dirs`. File options (`--query-file`, `--tasks-file`, highlights
  markdown inputs, PDF arguments, clip `--html`) answer `!files` with
  a glob where one applies. A slot the builder annotates with a clap
  `ValueHint` (for example `completion install -t`, `completion zsh -o`)
  answers the same way with no table entry; a path-specific table entry
  (the highlights PDF `--output`) beats the hint.
- **Free text.** Everything else answers a `!message` hint naming the
  slot, for example `MESSAGE — Override the generated Git commit
  message`. Stale-safe refs (`--task-ref`) and the bare note name of
  `highlights --parent` stay free text: they name one thing, not a
  set.
- **Capture markers.** `TEXT` on `capture`, `capture-parse`, and
  `capture-rewrite` completes the marker at the end of the active word
  through the in-process `capture_complete` extraction (see below).
  Wikilinks (`[[…`) are deferred and offer nothing.
- **Vault values.** These slots read the live vault through the same
  in-process scans as the commands they complete, so short clusters
  (`-rcash`), `--route=cash`, and `BOB_DIR` defaults behave exactly as
  at runtime. A slot whose prerequisite is missing answers
  `!message pass --route first` (or `pass --task first`) instead of
  guessing. Vault scans run behind a 150 ms deadline so a slow vault
  can never delay the prompt; on timeout bob answers nothing.
  - `--route` (on `capture`, `capture-sections`, `capture-tasks`,
    `capture-task-id`, `capture-task-sections`): routes in
    capture-targets scan order, grouped `inbox` / `areas` / `projects`.
  - `capture --section`: non-Tasks sections of the routed note, grouped
    `sections in <route>`.
  - `capture --task` and `capture-task-sections --block-id`: open tasks
    of the routed note by block ID, grouped `tasks in <route>`.
  - `capture --task-section`: ALL-CAPS child sections of the `--task`
    parent by exact title, grouped `task sections`.
  - `capture-pomodoro-name --pomodoro-ref`: open Pomodoros by
    stale-safe ref, grouped `open Pomodoros`.
  - `plugins sync --plugin`: directory names under
    `<--repo or BOB_PLUGINS_DIR>/plugins/*/`, grouped `plugins`. Never
    git.
  - `randomize --level`: configured priority labels in config order;
    the description is the roll window (for example `2–7 days`).
  - `query --tasks-note`, `query --origin`, `ready NOTE`,
    `capture-task-id --note-path`: vault notes
    as `!files-in <bob-dir>\t*.md`.

## Live transcripts (fixture vault)

The menus below are real `bob __complete` transcripts captured in a
sandboxed session against the completion test fixture (one area `cash`,
one project `dev`, one open task `^ship-it` in `dev`), so no private
vault content lands in docs. Each candidate line is
`value<TAB>description<TAB>group<TAB>space|nospace`.

```text
$ bob __complete zsh --protocol 1 -- bob ""
capture<TAB>Capture a task or bullet into the Bob vault<TAB>commands<TAB>space
completion<TAB>Install and inspect shell completion for bob<TAB>commands<TAB>space
freshness<TAB>Walk the tiered freshness review queue and seed the cutover<TAB>commands<TAB>space
… (every subcommand, in help order)
capture-complete<TAB>Complete the capture marker at the cursor<TAB>capture protocol<TAB>space
… (all ten capture-* endpoints, grouped last)
```

```text
$ bob __complete zsh --protocol 1 -- bob capture-sections --route ""
mac_inbox<TAB>inbox · default capture target<TAB>inbox<TAB>space
cash<TAB>area<TAB>areas<TAB>space
dev<TAB>project · active<TAB>projects<TAB>space
```

```text
$ bob __complete zsh --protocol 1 -- bob capture "@dev:"
@dev:ship-it<TAB>Ship the release<TAB>tasks in dev<TAB>space
```

```text
$ bob __complete zsh --protocol 1 -- bob capture fix it "@dev:"
!prefix 7
@dev:fix<TAB>new block ID<TAB>new task ID<TAB>space
```

```text
$ bob __complete zsh --protocol 1 -- bob capture --format ""
human<TAB>Colored text for people<TAB>format<TAB>space
json<TAB>Machine-readable JSON<TAB>format<TAB>space
```

```text
$ bob __complete zsh --protocol 1 -- bob completion ""
bash<TAB>Print the bash completion adapter<TAB>commands<TAB>space
install<TAB>Install or refresh the completion adapter for your shells<TAB>commands<TAB>space
status<TAB>Show installed adapters and the bob they call<TAB>commands<TAB>space
uninstall<TAB>Remove completion adapters that bob installed<TAB>commands<TAB>space
zsh<TAB>Print the zsh completion adapter<TAB>commands<TAB>space
```

## Performance

Measured 2026-10-02 on `apollo` (debug build) against the real vault
(`BOB_DIR=~/bob`), 20 samples after one warmup. `__complete` never
writes, so measuring on the real vault is safe.

| Slot | p50 | p95 |
| ---- | --- | --- |
| `bob <TAB>` (structural) | 10.9 ms | 13.8 ms |
| `capture-sections --route <TAB>` (vault) | 27.6 ms | 33.0 ms |
| `capture --route dev --task <TAB>` (vault) | 13.5 ms | 16.3 ms |
| `capture fix it @dev:<TAB>` (capture text) | 14.5 ms | 17.4 ms |
| `capture-pomodoro-name --pomodoro-ref <TAB>` (vault) | 11.5 ms | 15.9 ms |

Structural p95 is under 20 ms and vault p95 is under 75 ms, so no
fix was needed. If a slot regresses past those budgets, the latency
checks in `tests/cli/completion/` warn first; a real regression gets a
task bead.

## Installing

`just install` installs `bob` from this checkout and refreshes shell
completion in one step. From the Git remote instead:

```bash
cargo install --git git@github.com:bobs-org/bob-cli.git --locked bob-cli && bob completion install
```

`bob completion install` targets your shells directly:

```bash
bob completion install           # $SHELL, plus every bob-owned adapter
bob completion install zsh -d    # show the plan without writing anything
```

With no `SHELL` arguments bob installs for `$SHELL` plus every bob-owned
adapter; with `-t/--target` and no `SHELL` arguments it installs only
`$SHELL`'s shell, so a second owned adapter never turns a single-target
install into a usage error.

For zsh the target directory is the first match: `--target DIR`, the
previous install location from the manifest, the first writable fpath
entry under `$HOME`, oh-my-zsh completions, then `~/.zfunc` (created as
needed). Installs that land on the `~/.zfunc` home default always print
the exact `fpath=(~/.zfunc $fpath)` line to add before compinit —
including `-n` and dry runs, which never probe. For bash it is
`${BASH_COMPLETION_USER_DIR:-${XDG_DATA_HOME:-~/.local/share}/bash-completion}/completions/bob`.
Files bob did not write are refused without `--force`, and every write is
atomic. Moving an owned install with `-t` removes the previous adapter
when it still matches the manifest (plus any `.zwc` next to it) and says
so; an edited previous file is left behind with the exact `rm` command.
Verification probes a real shell unless `--no-verify` is given;
`--quiet` prints only warnings and errors. Combining `--target` with more
than one shell is a usage error.

## Commands

- `bob completion` (same as `status`, plus the hidden `list` alias):
  show installed adapters and the `bob` they call.
- `bob completion install [SHELL]...`: install or refresh adapters.
  `-d/--dry-run`, `-f/--force`, `-n/--no-verify`, `-q/--quiet`,
  `-t/--target DIR`.
- `bob completion status`: `-j/--json` for machine output, `-v/--verify`
  to probe a real shell now.
- `bob completion uninstall [SHELL]...`: remove only files whose stamp
  and manifest digest prove bob wrote them, plus any stale `_bob.zwc`.
  An edited file is refused with the exact `rm` command instead.
- `bob completion bash [-o FILE]`: print the bash adapter, or write it
  to FILE yourself (unrecorded: status reports it as externally managed).
- `bob completion zsh [-o FILE]`: print the zsh adapter, or write it to
  FILE yourself (unrecorded: status reports it as externally managed).

Exit codes are 0 for success or an explicit no-op, 1 when an install or
uninstall failed, when a live install verification is `not registered`,
`shadowed by …`, or `bob is bound to …`, or when `status -v` found a
broken install (an `unverified` probe is a warning and stays 0), and 2
for usage errors. A dry run never prints a closer; `Completion is live`
prints only when every row is unchanged and registered.

## Status states

- `not installed`: nothing there and nothing recorded. Renders
  `· bash not installed → bob completion install bash`, never probes in
  any mode, and never fails the exit code.
- `current`: bob-owned, and the bytes equal what this binary writes.
- `outdated`: bob-owned, but the bytes differ. Hint:
  `bob completion install`.
- `edited`: the stamp is present, but the digest differs from the
  manifest. Hint: reinstall with `-f`.
- `foreign`: no bob stamp. Hint: reinstall with `-f`.
- `current (externally managed)`: an unrecorded file whose bytes equal
  this adapter. Bob reports it and never adopts it.
- `outdated (externally managed)`: an unrecorded stamped file whose bytes
  differ. Refused without `-f`, like a foreign file, and never adopted.
- `missing`: the manifest records an install, but the file is gone. Stays
  a failure.

Without `-v`, status never spawns a shell: the target is the manifest
record or the probe-free default. A recorded unhealthy verification
renders `⚠`, never `✓`, and the exit code stays 0.

## Troubleshooting

- `bob completion status -v` probes a real shell now and reports
  `registered as _bob`, `not registered` (zsh: an `fpath` line or a
  stale-compdump `rm` plus `exec zsh`; bash: the exact `source <path>`
  line for `~/.bashrc`), `shadowed by <file>`, `bob is bound to <fn>`,
  or `unverified (<reason>)`. The probe initializes compinit only when
  the rc did not (`(( ${+_comps} )) || compinit -D`), so a stale compdump
  from the rc stays visible and the `rm …​/.zcompdump* && exec zsh`
  remedy (`rm -f "${ZDOTDIR:-$HOME}"/.zcompdump* && exec zsh`) can fire. Without `-v`, status shows the verification
  recorded at install time, but only when its digest and path still match
  the file. With `-n`, install reports
  `registration not checked → bob completion status -v` instead of
  probing.
- `BOB_COMPLETE_DEBUG=<file>` appends each request, response, elapsed
  milliseconds, and any error or timeout.
- A stale compinit dump looks installed but never loads: remove it with
  `rm -f "${ZDOTDIR:-$HOME}"/.zcompdump*` and restart the shell.
- PATH shadowing: `<TAB>` asks the `bob` on `PATH`, so when status warns
  that it differs from the running binary, fix `PATH` or reinstall.
- Version skew answers: an old adapter hears `bob shell completion is
  out of date — run: bob completion install`; a newer adapter than the
  binary hears `this bob is older than its shell completion — reinstall
  bob (just install)`.

## Why no rc edits and no `eval`

`bob completion install` writes one file and records it in the manifest
under `$XDG_STATE_HOME/bob-cli/completion/`. It never edits `~/.zshrc`,
`~/.bashrc`, or any rc file: it prints the exact line to add instead, so
your shell startup stays yours. And adapters never `eval` generated code — every
`<TAB>` runs `bob __complete` against the binary on `PATH` and renders
what it returns, so completion can never drift from the CLI.

## Styling

The zsh adapter ships bob-scoped presentation defaults, applied only
where you have set no style of your own:

- Group headers render as bold green `── <group> ──`, matching
  `bob --help`.
- Candidates stay grouped (`group-name` is set to the empty style),
  so each slot keeps its own header instead of merging into one list.

Both are scoped to `bob`, so other commands are unaffected. To use
your own look, set the same styles yourself — yours win:

```zsh
zstyle ':completion:*:*:bob:*:descriptions' format '── %d ──'
```

Directives go through the native `_files` and `_message` widgets,
so your `list-colors`, `menu select`, quoting, and native colors
keep working. Setting `NO_COLOR` switches bob's default header to
the plain `── %d ──` form.

## Bash

The bash adapter is values-only: descriptions and groups are ignored, so
every slot completes plain values through `COMPREPLY`. It registers with
`complete -F _bob bob` (no `-o default`, so free-text slots never fall
back to filenames).

- **Word reassembly.** Prior words and the cursor word are rebuilt from
  `COMP_LINE`/`COMP_POINT`, rejoining tokens that `COMP_WORDBREAKS` split
  at `:` and `=`. Simple quoting is unquoted, and the text after the
  cursor is sent as `--suffix`.
- **`!prefix N`.** Each reply is the kept text plus the value, minus the
  cursor-word text up to and including its last wordbreak character,
  whatever it is (`=`, `:`, …) — the part readline already broke off
  and will replace. A word whose quote is still open at the cursor
  skips the stripping and the escaping: readline replaces the whole
  quoted text and adds the closing quote itself. Other unquoted values
  with spaces or shell metacharacters are `%q`-escaped so each stays
  one argument.
- **Directives.** `!dirs` completes directories via `compgen -d`;
  `!files [glob]` completes files via `compgen -f` filtered by the glob;
  `!files-in` completes paths relative to its root, filtered by the text
  after the kept prefix; `!message` answers an empty reply. Unknown `!`
  directives are ignored.
- **Spacing.** `compopt -o nospace` is set only when every candidate is
  `nospace`; file directives set `compopt -o filenames`.
- **Installing.** The target is
  `${BASH_COMPLETION_USER_DIR:-${XDG_DATA_HOME:-~/.local/share}/bash-completion}/completions/bob`.
  Verification triggers bash-completion's lazy loader (`_comp_load bob`
  in bash-completion ≥ 2.12, else `__load_completion bob`), then checks
  that `complete -p bob` names `_bob`. Without bash-completion the report
  is `not registered` with the exact `source <path>` line for
  `~/.bashrc`, since bob never edits rc files.

## Capture markers

`TEXT` on `capture`, `capture-parse`, and `capture-rewrite` completes the
capture marker at the end of the active word through an in-process
`capture_complete` extraction, so shell completion can never disagree with
the marker highlighting `bob capture-parse` derives. Shell completion is
another thin client of that service (see `docs/capture.md`
`bob capture-complete`).

- **Slots.** The cursor word is `TEXT` when the trailing `TEXT` has
  already received a word, when `--` precedes it, or when it sits at the
  `TEXT` position and does not start with `-`. `raw_text` joins the
  `TEXT` words before the cursor with single spaces plus the cursor
  prefix, with `cursor = raw_text.len()`.
- **Release gates.** Safe rows only (no `requires_block_id` or
  `requires_name`; create-Pomodoro rows stay as `new Pomodoro`); end of
  the active word only (a non-empty `--suffix` offers nothing, the
  replacement must end at the cursor and start inside the cursor word, a
  newline in the word offers nothing); nothing beyond completion (no
  `capture-task-id`, `capture-pomodoro-name`, writes, or dry runs);
  wikilinks (`[[…`) are deferred before the note index read and offer
  nothing.
- **Block IDs follow `capture-complete`.** A solo `@route:` links
  existing tasks, grouped `tasks in <route>`. Body-bearing text mints a
  new task with that ID, so shell completion offers the field's
  suggestions instead (for example `@dev:fix`, grouped `new task ID`)
  and never an ID `capture-complete` lists as used.
- **Action items offer nothing**, exactly as in `capture-complete`:
  closes (`=x…`, `=*`, `=!`), starts (`=`, `=<X>`), numeric `+N` / `-N`
  adjustments and `++N` / `--N` shifts, and Work Log text on or below
  the `=x` line. The exact lone `+` remains an action; capture completion
  separately opens the parent-task picker, while shell completion offers
  only identified `@route+block-id` values in the `parent tasks` group.
- **Presentation.** Values are full marker texts such as
  `@dev:remote-power`. Descriptions come from the row: task text, route
  kind, `new block ID`, or Pomodoro time and name. Groups are human
  words: `inbox` / `areas` / `projects`, `sections in dev`,
  `tasks in dev`, `parent tasks`, `new task ID`, `task sections`,
  `open Pomodoros`, `active tasks`. A value ending in `:` `+` `#` `=` `^` is `nospace`;
  a complete marker gets a space. A quoted word carrying its own prefix
  (for example `fix it @dev:`) is served with `!prefix 7`, counted in
  Unicode scalar values.

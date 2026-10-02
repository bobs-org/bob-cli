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
  a glob where one applies.
- **Free text.** Everything else answers a `!message` hint naming the
  slot, for example `MESSAGE — Override the generated Git commit
  message`. Stale-safe refs (`--task-ref`) and the bare note name of
  `highlights --parent` stay free text: they name one thing, not a
  set. Capture `TEXT` carries an interim hint until marker completion
  lands in a later phase.
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
  - `query --tasks-note`, `query --origin`, `ready NOTE`: vault notes
    as `!files-in <bob-dir>\t*.md`.

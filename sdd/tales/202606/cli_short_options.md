---
create_time: 2026-06-04 13:51:08
status: wip
prompt: sdd/prompts/202606/cli_short_options.md
---
# Plan: Require Short Options for All Long CLI Options

## Context

The existing `memory/long/cli_rules.md` says CLI changes must keep help output excellent, keep subcommands/options
sorted, and prefer useful color. The new rule is:

- All public command-line options with a long form such as `--foobar` must also have a short form such as `-f`.

This applies to Bob's public command surfaces and installed legacy-compatible binaries. It does not apply to internal
subprocess arguments passed to tools like `git`, `ob`, `cargo`, or `obsidian`, and it does not require rewriting
historical SDD prompt/tale/epic notes that record prior command designs.

## Current Gaps Found

- `bob dataview` uses Clap and currently has long-only options:
  - `--bob-dir`
  - `--engine`
  - `--format`
  - `--origin`
  - `--query`
  - `--query-file`
  - `--source`
  - `--strict-paths`
  - `--vault`
- `bob highlights` uses Clap and currently has long-only options:
  - shared config: `--bob-dir`, `--lib-dir`, `--ref-dir`
  - `scan`: `--dry-run`
  - `sync`: `--dry-run`, `--prefer`, `--write-pdf`
  - `--jobs` and `--write-pdfs` already have `-j` and `-w`
- `bob move-done-tasks` uses manual parsing and currently has long-only `--threshold`.
- Existing legacy-compatible commands already expose short forms for their current long options:
  - `-h|--help`
  - `-d|--debug`
  - `-v|--verbose`

## Short Option Assignments

Use mnemonic, command-local short aliases. Reusing the same short letter in different subcommands is acceptable because
the parsers are scoped per command.

`bob dataview`:

- `-b, --bob-dir <PATH>`
- `-e, --engine <ENGINE>`
- `-f, --format <FORMAT>`
- `-o, --origin <VAULT_RELATIVE_PATH>`
- `-q, --query <DQL>`
- `-Q, --query-file <PATH>`
- `-s, --source <SOURCE>`
- `-S, --strict-paths`
- `-v, --vault <NAME_OR_ID>`

`bob highlights`:

- `-b, --bob-dir <PATH>`
- `-d, --dry-run`
- `-j, --jobs <N>` already exists
- `-l, --lib-dir <PATH>`
- `-p, --prefer <SIDE>`
- `-r, --ref-dir <PATH>`
- `-w, --write-pdf` for `sync`
- `-w, --write-pdfs` for `scan` already exists

`bob move-done-tasks`:

- `-t, --threshold <N>`

## Implementation Steps

1. Add the new rule to `memory/long/cli_rules.md` after the existing sorting rule, keeping the memory file concise and
   consistent.

2. Update parser definitions:
   - Add `.short(...)` to each long-only `Arg` builder in `src/native/dataview.rs`.
   - Add `.short(...)` to each long-only `Arg` builder in `src/native/highlights_ref/mod.rs`.
   - Update `src/native/collect_done.rs` manual parsing to accept `-t N`, and preferably attached forms `-t=N` and `-tN`
     for parity with the existing `--threshold=N` convenience form.
   - Keep `-h|--help` behavior unchanged everywhere.

3. Update user-facing help text and examples where they define command usage:
   - `src/native/collect_done.rs` help should show `[-t|--threshold N]` or equivalent and list `-t, --threshold`.
   - Clap-generated Dataview and Highlights help should pick up the aliases from parser definitions.
   - Update top-level examples in `src/runner.rs` only if needed for consistency; long-form examples can remain because
     the rule requires short options to exist, not that examples must prefer them.

4. Update docs that describe current command surfaces:
   - `README.md` command synopsis lines for `move-done-tasks` and `highlights`.
   - `docs/dataview.md` option list so each documented long option includes its short alias.
   - `docs/highlights-ref-sync.md` command synopsis and any compact option reference lines that currently define the
     current interface.
   - Avoid churn in historical `sdd/` files unless a current, user-facing command reference there is actively consumed
     by tests.

5. Add or adjust tests:
   - Add focused coverage that short aliases are accepted for `bob dataview`, `bob highlights`, and
     `bob move-done-tasks`.
   - Add a help-output guard that fails if an option-definition line starts with a long option only, such as
     `--threshold` or Clap's `      --bob-dir`, across the current Bob command help surfaces.
   - Update existing tests that assert help ordering or exact option names so they expect the new short aliases.

6. Verification:
   - Run `cargo fmt --check`.
   - Run focused CLI tests first, likely `cargo test --test cli`.
   - Run the full suite with `cargo test`.
   - Run `cargo clippy --all-targets --all-features`.
   - Run `just install-smoke` if the earlier test pass leaves enough time, because it exercises installed binary help
     for each command.

## Risks and Mitigations

- Short-letter collisions can cause parse errors or confusing help. Mitigation: choose aliases per command scope and
  preserve existing meanings (`-j`, `-w`, `-h`, `-v` where already established).
- `-v` means `--verbose` for legacy commands but `--vault` for `bob dataview`. Mitigation: this is command-local and
  Dataview currently has no verbosity option.
- Uppercase `-Q` and `-S` are less ergonomic. Mitigation: they avoid ambiguity with `-q|--query` and `-s|--source`.
- Over-updating historical planning files would create noisy, low-value churn. Mitigation: restrict edits to current
  memory, source, tests, README, and docs.

---
create_time: 2026-06-16 10:30:37
status: wip
prompt: sdd/prompts/202606/capture_targets_verbose_warnings.md
---
# Plan: Gate capture-targets skip warnings behind verbose output

## Context

`bob capture-targets` currently scans the vault for routable top-level markdown notes and records non-routable markdown
files as warnings. On successful scans, `run()` always prints those warnings to stderr after the human or JSON result.
In normal use this is noisy because expected root-level notes such as `AGENTS.md`, uppercase agent docs, and
period-meeting files are intentionally skipped.

Project CLI rules require excellent help text, alphabetically listed options, and a short alias for every public long
option. The requested option is `-v|--verbose`, scoped to `bob capture-targets`.

## Goal

Add a `-v|--verbose` option to `bob capture-targets` so routine
`bob capture-targets: ... skipping non-routable note ...` warnings are printed only when verbose mode is enabled.
Default command output should remain focused on targets and should keep stderr empty when the scan succeeds with only
skip warnings.

## Design

1. Add a command-local `verbose` flag to `src/native/capture_targets.rs`.
   - Define a `verbose_arg()` using clap `ArgAction::SetTrue`, `.short('v')`, `.long("verbose")`, and concise help text.
   - Add the argument to `build_cli()` after `help_arg()` so the rendered options stay alphabetically ordered as `-b`,
     `-f`, `-h`, `-v`.
   - Read the flag in `run()` with `matches.get_flag("verbose")`.

2. Keep scan behavior unchanged.
   - `scan_capture_targets()` should continue collecting `warnings` and `issues`.
   - Routability validation, target ordering, JSON schema, and human output formatting should not change.

3. Gate warning emission at the output boundary.
   - On successful scans, print warnings to stderr only when `verbose` is true.
   - On human scan failures, always print real `issues`; print accumulated skip warnings only when `verbose` is true.
   - Preserve JSON failure behavior: print the JSON error object to stdout and keep stderr clean.

4. Update tests around the public contract.
   - Update the capture-targets help ordering assertion to include `-v, --verbose`.
   - Change the existing JSON success test so the default invocation with a non-routable note expects empty stderr while
     preserving target JSON and ordering assertions.
   - Add or extend coverage for `--verbose` and the `-v` alias to prove skipped-note warnings appear only in verbose
     mode.
   - Keep existing empty-vault, human output, native-help, and long-only-option checks passing.

## Verification

Run focused tests first:

```bash
cargo test --test cli capture_targets
```

Then run the broader CLI suite if focused tests pass:

```bash
cargo test --test cli
```

If time allows, do a manual smoke against a scratch vault with one routable area note and one non-routable uppercase
note:

```bash
BOB_DIR=<scratch-vault> cargo run -- capture-targets
BOB_DIR=<scratch-vault> cargo run -- capture-targets --verbose
BOB_DIR=<scratch-vault> cargo run -- capture-targets -v -f json
```

Expected result: default invocations produce no skip-warning stderr on success; verbose invocations print the existing
`bob capture-targets:` skip warning format.

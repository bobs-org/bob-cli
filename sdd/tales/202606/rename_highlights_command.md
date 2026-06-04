---
create_time: 2026-06-04 11:01:11
status: done
prompt: sdd/prompts/202606/rename_highlights_command.md
---
# Plan: Rename `bob highlights-ref` to `bob highlights`

## Goal

Rename the user-facing top-level command group from `bob highlights-ref` to `bob highlights` while preserving the
existing nested command behavior:

- `bob highlights doctor`
- `bob highlights marker <pdf>`
- `bob highlights scan [--dry-run] [--jobs N]`
- `bob highlights sync <pdf> [--dry-run] [--write-pdf] [--prefer marker|frontmatter]`

The rename should be complete across the active command surface: top-level help, nested help, error prefixes, README
examples, setup docs, install smoke checks, and integration tests.

## Current Shape

- Top-level `bob` subcommands are registered in `src/runner.rs::SUBCOMMANDS`, rendered in declaration order, and guarded
  by help-order tests.
- The highlights command is native-only. It is dispatched through `NativeCommand::HighlightsRef` in `src/native.rs` into
  `src/native/highlights_ref/mod.rs`.
- The nested command parser in `src/native/highlights_ref/mod.rs` uses `COMMAND_NAME = "bob highlights-ref"` as the Clap
  binary name and as the error prefix.
- The nested subcommands are already alphabetically ordered as `doctor`, `marker`, `scan`, `sync`, and the sync options
  are already sorted.
- Current docs and tests contain many literal `bob highlights-ref` command invocations. Some `highlights-ref` strings
  are not command names, such as `docs/highlights-ref-sync.md`, fixture paths, temporary test names, Highlights URL
  examples, and the pipeline version `highlights-ref-mvp-3`.

## Compatibility Decisions

1. Remove the old top-level `bob highlights-ref` spelling from the active CLI, matching the repo's recent command-rename
   policy for `collect-done` and `sync`.
2. Do not add a transitional alias unless requested separately. Listing both `highlights` and `highlights-ref` would
   make the rename incomplete and clutter top-level help.
3. Keep implementation/module/storage names that describe the existing reference-sync domain or persisted metadata:
   - `src/native/highlights_ref/`
   - `highlights_ref_*` Rust test names and fixture directories
   - `docs/highlights-ref-sync.md` unless a docs-file rename is requested separately
   - frontmatter fields such as `highlights_marker_hash`
   - `PIPELINE_VERSION = "highlights-ref-mvp-3"` to avoid implying a data-format migration
4. Keep `BOB_HIGHLIGHTS_LIB_DIR` and `BOB_HIGHLIGHTS_REF_DIR`; these already describe the Highlights feature, not the
   old command spelling.
5. Do not rewrite archival SDD prompts/tales just because they mention the old command. Only current product docs and
   executable examples should change.

## Implementation

1. Update command registration and dispatch.
   - In `src/runner.rs`, change the registered subcommand name from `highlights-ref` to `highlights`.
   - Update the top-level examples from `bob highlights-ref scan --dry-run` to `bob highlights scan --dry-run`.
   - Rename the internal enum variant from `HighlightsRef` to `Highlights` if it improves dispatcher clarity, while
     keeping the implementation module name `highlights_ref`.

2. Update nested command identity.
   - In `src/native/highlights_ref/mod.rs`, change `COMMAND_NAME` to `bob highlights`.
   - Confirm all nested help and Clap error output now show `bob highlights`.
   - Leave output headings such as `Highlights reference sync` unchanged because they describe the workflow, not the CLI
     spelling.

3. Update active docs and developer conveniences.
   - In `README.md`, replace current command invocations and smoke-test snippets with `bob highlights ...`.
   - Add a migration note that `bob highlights-ref` is now `bob highlights` and the old top-level name is no longer
     registered.
   - In `docs/highlights-ref-sync.md`, update command examples, troubleshooting commands, launchd/cron snippets, and
     setup text to use `bob highlights ...`.
   - For launch-agent labels/log paths or backup directory examples that encode the old command name, update them only
     where they are active copy/paste setup snippets; avoid broad historical rewrites.
   - In `justfile`, update `install-smoke` to run `bob highlights --help`.

4. Update tests.
   - Change integration-test command invocations from `.arg("highlights-ref")` to `.arg("highlights")`.
   - Update help assertions to expect `Usage: bob highlights`.
   - Update `top_level_help_lists_commands_alphabetically_with_examples` so the command order contains `highlights` and
     no longer contains `highlights-ref`.
   - Add or adjust a focused negative assertion that `bob highlights-ref --help` is rejected with exit code 2, proving
     the old top-level spelling is gone.
   - Update any test helper scripts or spawned process snippets that invoke `$BOB_BIN highlights-ref ...`.
   - Leave test function names, tempdir names, and fixture paths alone unless they assert user-facing text.

## Verification

- `cargo fmt --check`
- Focused help and rename tests:
  - `cargo test highlights_ref_help`
  - `cargo test top_level_help_lists_commands_alphabetically_with_examples`
- Focused behavior tests for the renamed command path:
  - `cargo test highlights_ref`
- Manual help checks:
  - `cargo run -- highlights --help`
  - `cargo run -- highlights scan --help`
  - `cargo run -- highlights sync --help`
  - `cargo run -- -h`
  - `cargo run -- highlights-ref --help` should fail with exit code 2
- Full confidence check:
  - `cargo test`
  - `just install-smoke` if package/install-surface confidence is needed

## Risks

- External automation currently calling `bob highlights-ref scan` will break. The docs should make the migration
  explicit, and any local launchd/cron examples should show the new spelling.
- A blanket text replacement would damage non-command identifiers such as persisted pipeline metadata, fixture names,
  docs filenames, and Highlights URL examples. The implementation should use targeted replacements and review every
  remaining `highlights-ref` occurrence.
- Removing the old spelling means users get Clap's unknown-subcommand error. If a friendlier migration error is desired
  later, it should be designed as an explicit compatibility feature rather than an accidental alias.

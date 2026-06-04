---
create_time: 2026-06-04 14:44:24
status: wip
prompt: sdd/prompts/202606/rename_cronjob_to_nightly.md
---
# Plan: Rename `bob cronjob` to `bob nightly`

## Goal

Rename the public nightly maintenance command from `bob cronjob` to `bob nightly`, preserving the existing behavior: one
shared Obsidian sync gate, followed by the wrapped maintenance steps, with the same failure semantics and TTY-aware
output. Update the user's live crontab so the scheduled job invokes the new command.

## Decisions

- Treat this as a true public command rename, not an alias addition: `bob nightly` is the supported command and
  `bob cronjob` should no longer appear in top-level help or current documentation.
- Keep the maintenance behavior unchanged. Only naming, help text, docs, tests, and scheduling should change.
- Rename internal command identifiers where practical (`Cronjob` -> `Nightly`, `cronjob.rs` -> `nightly.rs`) so the
  codebase does not keep carrying the old public name in current implementation paths.
- Do not rewrite historical SDD prompt/tale files just to edit past wording. Update current source, tests, README,
  justfile smoke checks, and non-historical docs.
- Update the crontab command from `bob cronjob` to `bob nightly`; also rename the cron log target from
  `/var/tmp/bob_cronjob.log` to `/var/tmp/bob_nightly.log` so future logs match the command name.

## Implementation Scope

1. CLI registration
   - Update `src/native.rs`: rename the module and enum variant to `nightly` / `NativeCommand::Nightly`.
   - Update `src/runner.rs`: replace the `cronjob` subcommand with `nightly`, keep the subcommand table alphabetically
     sorted, and update top-level examples.

2. Native implementation
   - Rename `src/native/cronjob.rs` to `src/native/nightly.rs`.
   - Update user-facing strings: module docs, error prefixes, help usage, help prose, "Try ... --help" text, and the run
     header from `bob cronjob` to `bob nightly`.
   - Leave the wrapped steps and shared sync/lock behavior untouched.

3. Related comments and docs
   - Update current references in `src/native/sync.rs`, `src/native/ob.rs`, and `src/native/collect_done.rs`.
   - Update README install smoke tests, command docs, runtime dependency notes, environment docs, migration notes, and
     release checklist.
   - Update `docs/highlights-ref-sync.md` if it still points readers at `bob cronjob`.
   - Update `justfile` install smoke checks.

4. Tests
   - Update CLI help safety tests and no-long-only-option coverage to use `nightly`.
   - Rename cronjob-focused test names to nightly-focused names for clarity.
   - Update runtime invocations and assertions that inspect help/output labels.
   - Add or update coverage that the old `cronjob` spelling is absent from top-level help and not treated as the current
     public command surface.

5. User crontab
   - Save a timestamped backup of the current crontab before modification.
   - Replace the existing Bob line: `bob cronjob` -> `bob nightly` `/var/tmp/bob_cronjob.log` ->
     `/var/tmp/bob_nightly.log`
   - Reinstall the edited crontab with `crontab`.
   - Verify `crontab -l` contains the new command and no scheduled `bob cronjob` entry.

## Verification

- Run `cargo fmt --check`.
- Run targeted CLI tests around help and nightly behavior, then run the broader test suite if time and runtime allow:
  `cargo test nightly`, `cargo test all_top_level_subcommand_help_is_safe_and_plain`,
  `cargo test public_help_surfaces_do_not_list_long_only_options`, and `cargo test`.
- Run `just check-scripts` or at least the install smoke target if the full package checks are practical.
- Verify the live crontab after editing.

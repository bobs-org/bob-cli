---
create_time: 2026-06-03 15:04:53
status: wip
prompt: sdd/prompts/202606/dataview_no_sync.md
---
# Plan: Make `bob dataview` Read-Only and Sync-Free

## Context

`bob dataview` is a read-only query command. It currently exposes `--sync`, and both the Obsidian and dynomark execution
paths call `ob sync` before querying when that flag is set. That conflicts with the intended ownership model: `ob sync`
belongs to the background service / cron-style maintenance path, while Dataview should only read the already-open or
already-synced vault state.

The relevant code is concentrated in `src/native/dataview.rs`, with CLI coverage and help assertions in `tests/cli.rs`,
and user-facing references in `README.md` and `docs/dataview.md`.

## Intended Behavior

- `bob dataview` never invokes `ob sync` or `ob sync-status`.
- The `--sync` option is removed rather than made a no-op, so callers do not get a false freshness guarantee.
- `--bob-dir` validation remains in place when the user supplies `--bob-dir`.
- `--engine dynomark` still validates and uses the Bob vault path because it runs from that directory.
- The Obsidian engine still uses the running desktop Obsidian session and reserves stdout for query results.
- The shared Obsidian sync gate remains available through the existing maintenance/background paths; this change is
  scoped to Dataview.

## Implementation Steps

1. Update `src/native/dataview.rs` to remove Dataview sync plumbing:
   - Remove `Request.sync`.
   - Remove the `sync_before_query` helper and Dataview `SyncFailed` error variant.
   - Remove the `--sync` argument from the clap command.
   - Stop passing sync-derived validation state into `VaultConfig::from_matches`; retain validation for explicit
     `--bob-dir` and for dynomark.
   - Remove the direct `ob` dependency from this module if it becomes unused.

2. Update CLI tests in `tests/cli.rs`:
   - Adjust the help-order test so `--sync` is absent.
   - Replace the current sync-specific tests with coverage proving that a normal Dataview query does not run `ob` even
     when `OB_COMMAND` points at a failing/logging stub.
   - Optionally assert that `bob dataview --sync ...` is rejected by clap as an unknown argument, making stale callers
     fail visibly.
   - Keep existing stdout-cleanliness and Obsidian/dynomark query behavior tests intact.

3. Update documentation:
   - Remove `--sync` examples and option text from `docs/dataview.md`.
   - Update README command summary, runtime dependencies, and smoke-test guidance so `ob` is associated with the shared
     cron/background sync path, not `bob dataview`.
   - Add a short note that Dataview does not run `ob sync`; vault freshness is handled externally.

4. Verify:
   - Run `cargo fmt --check`.
   - Run focused CLI tests for Dataview first, such as `cargo test dataview`.
   - Run the broader project checks that are practical in this workspace, at minimum `cargo test` and
     `cargo clippy --all-targets --all-features`.
   - If a full check is blocked by environment assumptions, report the exact blocker and the focused tests that passed.

## Risks and Tradeoffs

- Removing `--sync` is a breaking CLI change for callers that explicitly used it. That is intentional: a no-op flag
  would hide stale assumptions, while the command contract says Dataview should not own sync.
- The Obsidian engine can still observe whatever state desktop Obsidian currently has loaded. This is consistent with a
  read-only query command and the background sync service model.
- Dynomark remains a headless fallback over the local vault directory; it should not trigger freshness work itself.

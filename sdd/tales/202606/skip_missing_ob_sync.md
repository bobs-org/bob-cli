---
create_time: 2026-06-01 11:17:28
status: done
prompt: sdd/prompts/202606/skip_missing_ob_sync.md
---
# Plan: Skip Missing `ob sync` for Pomodoro Runtimes

## Context

`bob pomodoro-runtimes` annotates completed Pomodoro ledger entries in Bob daily notes. It currently attempts `ob sync`
before processing notes. On machines without the `ob` executable, both the native Rust implementation and the embedded
Python fallback treat the missing command as a hard error:

```text
bob_pomodoro_runtimes: ob command not found: ob
```

That prevents note annotation from running on machines that do not have `obsidian-headless` installed.

## Goal

Make the `ob sync` step optional for `bob pomodoro-runtimes` when the configured `ob` command is unavailable, while
preserving existing failure behavior when `ob` is available but the sync command itself fails.

## Scope

- Update the native Rust `bob pomodoro-runtimes` path in `src/native/runtimes.rs`.
- Update the embedded Python fallback in `scripts/bob_pomodoro_runtimes` so `BOB_CLI_USE_SCRIPT=1` behaves the same way.
- Add focused integration coverage for a machine/PATH with no `ob` command.
- Update user-facing README text so the optional-sync behavior is documented.

## Design

- Treat `io::ErrorKind::NotFound` from launching the configured `OB_COMMAND` as a successful no-op sync in Rust.
- Treat `shutil.which(ob_command) is None` as a successful no-op sync in Python.
- Keep all other `ob sync` errors unchanged:
  - process launch errors other than not-found still fail;
  - nonzero `ob sync` exits still fail unless they contain the existing "Another sync instance is already running"
    message.
- Prefer silent skipping for the missing executable case. The requested behavior is to skip sync when the command does
  not exist; printing an error-like message would preserve the confusing output the change is intended to remove.

## Tests

- Add a native integration test that runs `bob pomodoro-runtimes` with a PATH containing no `ob`, verifies the command
  succeeds, verifies it updates the note, and verifies stderr does not report "ob command not found".
- Add a fallback integration test with `BOB_CLI_USE_SCRIPT=1` and a missing `OB_COMMAND`, verifying the same skip
  behavior through the embedded Python script.
- Run targeted tests for `pomodoro-runtimes`, then run the broader Rust test suite if the targeted pass is clean.

## Risks

- A typo in `OB_COMMAND` will now be treated as "sync unavailable" rather than a hard failure for `pomodoro-runtimes`.
  This matches the requested machine-portability behavior, and only applies to runtime annotation, not the dedicated
  mutating `bob sync` command.
- Machines without `ob` may annotate stale local notes if Obsidian Sync has not already completed elsewhere. That is
  acceptable because the explicit request is to allow annotation to proceed when `ob` is absent.

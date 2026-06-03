---
create_time: 2026-06-03 17:11:26
status: wip
prompt: sdd/prompts/202606/maybe_bob_highlights_sync_quiet_cron.md
---
# Quiet `maybe_bob_highlights_sync` for Cron

## Context

`maybe_bob_highlights_sync` lives in the chezmoi source tree at:

```text
/home/bryan/.local/share/chezmoi/home/bin/executable_maybe_bob_highlights_sync
```

The installed command is intended to run from the user's crontab every 15 minutes. Cron emails whenever a job writes to
stdout or stderr, so the wrapper must be silent for all successful paths:

- missing or unchanged Highlights library
- another sync already holding the lock
- `bob highlights-ref scan` completing successfully, even if it created or updated notes

The native `bob highlights-ref scan` command is intentionally report-oriented today: on success it prints config,
per-PDF entries, a summary, and `writes: ...`. That output is useful interactively, so this change should avoid changing
the `bob` CLI surface unless needed. The cron-specific silence belongs in the wrapper.

## Plan

1. Keep the existing lazy gate and lock semantics.
   - Continue resolving `BOB_DIR`, `BOB_HIGHLIGHTS_LIB_DIR`, and the lock directory as the script already does.
   - Keep no-op exits quiet when the library is missing, has no recent files, or another run owns the lock.

2. Make the successful scan path silent.
   - Run `bob highlights-ref scan` with stdout and stderr redirected to a temporary log file.
   - If the command exits `0`, remove the log and exit `0` without printing anything.
   - If the command exits nonzero, print a short failure header plus the captured command output to stderr, then exit
     with the same status so cron sends a useful problem email.

3. Improve command resolution failure handling.
   - Preserve the existing `command -v bob` then `$HOME/.cargo/bin/bob` fallback.
   - If the fallback is not executable, report that as a problem on stderr and exit nonzero instead of relying on a
     vague shell execution error.

4. Ensure cleanup is robust.
   - Create a temporary output file with `mktemp`.
   - Extend the existing trap so both the lock directory and temp output file are removed on exit or interruption.

5. Verify behavior without touching the real vault.
   - Run `sh -n` on the chezmoi source script.
   - Use a temporary `BOB_DIR` whose `lib` directory has no recent files and confirm stdout/stderr are empty and exit is
     `0`.
   - Use a temporary `BOB_DIR` with a recent library file and a stub `bob` earlier in `PATH` that exits `0`; confirm the
     wrapper exits `0`, invokes `highlights-ref scan`, and emits no stdout/stderr.
   - Repeat with a stub `bob` that exits nonzero and writes to stdout/stderr; confirm the wrapper exits nonzero and
     emits the captured diagnostic output only on failure.

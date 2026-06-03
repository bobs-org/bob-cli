---
create_time: 2026-06-03 11:52:31
status: wip
prompt: sdd/prompts/202606/maybe_bob_highlights_sync.md
---
# Plan: Add `maybe_bob_highlights_sync`

## Context

The requested script should live in the chezmoi source tree so it installs as a normal user command. This chezmoi repo's
script convention is `home/bin/executable_<name>`, so the source file should be:

```text
/home/bryan/.local/share/chezmoi/home/bin/executable_maybe_bob_highlights_sync
```

and the applied command name will be:

```text
~/bin/maybe_bob_highlights_sync
```

The current `bob highlights-ref` command surface is:

```text
bob highlights-ref scan [--dry-run]
bob highlights-ref sync <pdf> [--dry-run] [--write-pdf] [--prefer marker|frontmatter]
```

So `sync` is a one-PDF command, while `scan` is the library-wide operation that recursively processes `~/bob/lib` and
updates `~/bob/ref`. For an hourly cron job that wakes only when the library has recent changes, the script should call
`bob highlights-ref scan` after the gate. This also avoids missing Highlights Markdown sidecar edits, which can change
the generated note body even when the PDF path itself was not the only changed file.

## Implementation

1. Add a small POSIX shell script at `home/bin/executable_maybe_bob_highlights_sync`.
   - Use `#!/bin/sh` to match the existing simple Bob wrappers.
   - Keep the no-op path quiet so hourly cron does not send mail when nothing changed.
   - Resolve `BOB_DIR`, defaulting to `$HOME/bob`.
   - Resolve `BOB_HIGHLIGHTS_LIB_DIR`, defaulting to `lib` under `BOB_DIR`, with support for absolute paths and simple
     `~/...` values.

2. Add the lazy recent-change gate.
   - Check regular files under the resolved library directory with `find "$lib_dir" -type f -mtime 0`.
   - Use the library-wide file check rather than only `*.pdf` so recent Highlights sidecar files also trigger a scan.
   - Exit `0` immediately if the library directory is missing or if the recent file count is zero.

3. Run the sync operation only after the gate passes.
   - Resolve `bob` through `command -v bob`, falling back to `$HOME/.cargo/bin/bob`, matching the existing
     `bob_sync`/`bob_pomodoro` wrappers.
   - Execute:

     ```sh
     "$bob_bin" highlights-ref scan
     ```

   - Do not pass `--write-pdf`; scheduled automation should update notes from Highlights state, not write PDF marker
     changes back.

4. Add a cheap overlap guard.
   - Use an atomic lock directory under `${TMPDIR:-/tmp}` so a long scan does not overlap the next hourly cron run.
   - If the lock already exists, exit `0` quietly.
   - Remove the lock via `trap` on exit.

5. Verify the script locally without touching the real vault.
   - Run `sh -n` against the chezmoi source script.
   - Use a temporary `BOB_DIR` with an empty or old `lib` directory to confirm the no-op path exits `0`.
   - Use a temporary `BOB_DIR`, a recently modified file under `lib`, and a stub `bob` earlier in `PATH` to confirm the
     script invokes `highlights-ref scan`.

## Deferred

Do not add or edit a crontab entry in this change unless explicitly requested. The script will be ready for an hourly
entry such as:

```cron
0 * * * * ~/bin/maybe_bob_highlights_sync
```

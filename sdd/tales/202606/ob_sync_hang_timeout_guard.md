---
title: Stop ob-sync-bob from stalling when a single `ob sync` hangs
status: proposed
date: 2026-06-14
create_time: 2026-06-14 07:24:41
prompt: sdd/prompts/202606/ob_sync_hang_timeout_guard.md
---

# Plan: Bound each `ob sync` attempt so a hung sync can't stall the poll loop

## Problem

`ob-sync-bob.service` does not reliably sync the `~/bob` Obsidian vault. This morning (2026-06-14) Bryan had to run
`ob sync --path ~/bob` by hand to get changes to sync.

## Diagnosis (root cause confirmed by logs)

The service runs `/home/bryan/.local/bin/ob-sync-bob-poll`, a bash loop that runs a one-shot
`ob sync --path /home/bryan/bob` every 30s (this polling design was the result of the prior
`ob_sync_continuous_polling_fix` effort, which deliberately replaced `ob sync --continuous` to avoid missed `fs.watch`
events — see `sdd/tales/202606/ob_sync_continuous_polling_fix.md`).

Journal evidence shows a single `ob sync` invocation **hung for ~12.5 hours**:

- `Jun 13 18:29:00` PID 2682090 logged `Starting sync:` then `Jun 13 18:29:01` `Connecting...`
- That same PID did **not** log `Disconnected from server` until `Jun 14 07:10:19`.
- Between those timestamps there were **zero** sync cycles.

The machine was **awake and active** the entire time — not suspended:

- `uptime` = 22:48 (booted ~Jun 13 08:27, no reboot since).
- ~14,223 non-`ob-sync` journal lines were emitted during the gap (sshd, gpg-agent, etc.).
- No `systemd-sleep` / suspend / resume events in that window.

So a single `ob sync` wedged at the **"Connecting..."** stage (most likely a TCP connection to the Obsidian Sync server
that stalled with no client-side socket timeout) and never returned.

**Root cause:** the poll loop runs `"$OB_BIN" sync ... & wait "$child_pid"` with **no timeout**. `ob sync` has no
built-in timeout (only `--path` / `--continuous`). When one invocation hangs, the loop's `wait` blocks forever, so **the
entire polling loop stalls indefinitely** — no further sync cycles run until the hung process is killed externally (a
manual `systemctl --user restart` or, as today, Bryan running `ob sync` by hand). This exactly matches "unreliable
sync": it works most of the time, then silently freezes.

**Secondary symptom (same cause):** the recurring `ob-sync-bob.service: Failed with result 'timeout'` entries (Jun 04,
11, 14) happen because the wedged node process ignores `SIGTERM`, so a stop/restart waits the full `TimeoutStopSec=30s`
before systemd `SIGKILL`s it. This adds a 30s delay and a failed-unit state to every recovery restart.

### Ruled out

- **Suspend/resume** causing the gap — machine was demonstrably awake.
- **Auth/credential expiry** — `ob sync-status --path /home/bryan/bob` returns healthy config.
- **Service not running / crash-looping** — unit is `active`, `NRestarts=0`; it's the inner `ob sync` process that
  hangs, not the unit.
- **Missed `fs.watch` events** (the _prior_ bug) — already addressed by the polling design; this is a different failure
  mode (a hang), so reverting to `--continuous` is **not** the answer and would regress the earlier fix.

## Recommended fix

Keep the polling design; **bound each `ob sync` attempt with GNU `timeout`** (confirmed available: `/usr/bin/timeout`,
coreutils 9.7) so a hung sync is killed and the loop retries on the next cycle instead of stalling forever.

In `/home/bryan/.local/bin/ob-sync-bob-poll`, change the invocation from:

```bash
"$OB_BIN" sync --path "$VAULT_PATH" &
```

to a timeout-wrapped form, roughly:

```bash
timeout --signal=TERM --kill-after="${OB_SYNC_KILL_AFTER_SECONDS:-15}" \
        "${OB_SYNC_TIMEOUT_SECONDS:-120}" \
        "$OB_BIN" sync --path "$VAULT_PATH" &
```

Why this works:

- Normal syncs finish in ~2–10s, so a 120s bound never affects healthy cycles.
- A wedged sync is force-killed within `120s + 15s`, then the existing retry branch logs the non-zero exit (`timeout`
  returns `124`) and sleeps before the next attempt — the loop **self-heals** instead of freezing for hours.
- `--kill-after` guarantees a hung, `SIGTERM`-ignoring process is `SIGKILL`ed, which also removes the lingering process
  that currently causes the `'timeout'` stop failures, so manual restarts become fast and clean.
- Make the bound and kill-grace configurable via `OB_SYNC_TIMEOUT_SECONDS` / `OB_SYNC_KILL_AFTER_SECONDS`, mirroring the
  existing `OB_SYNC_INTERVAL_SECONDS` pattern.

The systemd unit needs no functional change (it just runs the wrapper). Optionally add a short comment in the unit
noting the wrapper now bounds each attempt.

## Implementation steps

1. Edit `/home/bryan/.local/bin/ob-sync-bob-poll`:
   - Add `OB_SYNC_TIMEOUT_SECONDS` (default 120) and `OB_SYNC_KILL_AFTER_SECONDS` (default 15) parsing alongside the
     existing interval validation.
   - Wrap the `ob sync` invocation in `timeout --signal=TERM --kill-after=… …`.
   - Keep the existing trap/`wait`/retry/logging structure intact (`child_pid` becomes the `timeout` PID; the existing
     non-zero-exit branch already logs and retries).
   - Add a log line on timeout (exit 124) for clarity, e.g. distinguish "timed out" from other non-zero exits.
2. Reload and restart:
   ```bash
   systemctl --user daemon-reload   # only if the unit file changed
   systemctl --user restart ob-sync-bob.service
   systemctl --user status ob-sync-bob.service --no-pager
   ```
3. Record this effort under SDD docs (`sdd/prompts/202606/`, `sdd/tales/202606/`) following the convention used by the
   prior `ob_sync_continuous_polling_fix` effort, so the hang/timeout history is captured next to it.

## Validation

- `journalctl --user -u ob-sync-bob.service -f` shows continuing one-shot `Starting sync:` → `Fully synced` cycles every
  ~30s after the change.
- Inject a hang to prove self-healing: temporarily set a tiny bound (e.g. `OB_SYNC_TIMEOUT_SECONDS=1`), confirm the log
  shows the timeout (exit 124) **and** that the loop continues to the next cycle instead of stalling; then restore the
  default.
- A controlled vault edit is picked up by a later cycle without any manual restart.
- No unrelated files in `/home/bryan/bob` or the bob-cli workspace are modified.

## Rejected alternatives

- **Revert to `ob sync --continuous`** — regresses the prior fix (missed `fs.watch` events) and `--continuous` can hang
  the same way without a watchdog.
- **systemd `WatchdogSec` / `sd_notify`** — would catch a hung wrapper, but requires the bash loop to emit heartbeats
  and is heavier than needed; the `timeout` wrapper fixes the actual hang at its source. Can revisit if hangs persist.
- **Lower `TimeoutStopSec`** — only speeds up the forced kill on restart; does nothing for the indefinite stall during
  normal operation.
- **Patch `obsidian-headless` to add a socket timeout** — brittle, overwritten by package updates; out of scope.

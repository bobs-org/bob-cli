---
title: bob tick
description: Run Bob's frequent background work in a fixed, configurable order
---

# `bob tick`

`bob tick` runs Bob's frequent background work as one ordered pass: Calendar,
Gmail, Keep, reference scans, project sync, then task reconciliation. A step
runs only when its own cadence is due, so a scheduler can call `bob tick`
every few minutes without repeating hourly work. The command does not hold the
shared vault lock across steps; each child command manages its own lock.

See the [CLI reference](reference/cli/tick.md) for flags and examples.

## Steps and defaults

| Order | Step | Child command | Default cadence | Maximum run time |
| ---: | --- | --- | --- | ---: |
| 1 | `gcal` | `bob gcal sync` | `1h` | 10m |
| 2 | `gmail` | `bob gmail sync` | `1h` | 20m |
| 3 | `gkeep` | `bob gkeep pull` | `manual` | 20m |
| 4 | `refs` | `bob ref scan -w` | `15m` | 30m |
| 5 | `projects` | `bob projects sync` | `15m` | 10m |
| 6 | `tasks` | `bob task reconcile --retry-timeout 120` | `15m` | 10m |

The order is fixed: external data arrives before Bob derives project and task
state. The `gkeep` step is manual by default because it can clip web pages.
Run `bob tick gkeep` or `bob tick -a` when you want it to run.

## Configuration

Configure the scheduler in `~/.config/bob/config.yml`:

```yaml
tick:
  # writer: <host label>   # defaults to gcal.writer; unset means any host
  steps:
    gcal: 1h
    gmail: 1h
    gkeep: manual
    refs: 15m
    projects: 15m
    tasks: 15m
```

Each step accepts an interval from `5m` through `7d`, `manual`, or `off`;
YAML `false` also means `off`. Missing settings use the defaults above. The
configuration is strict inside `tick:`: unknown keys, steps, or values stop
`bob tick` with a usage error before it starts work. Other Bob commands ignore
an invalid `tick:` block.

The writer pin comes from `tick.writer`, falling back to `gcal.writer`. A
wrong-host run exits with code 2; `--dry-run` and `--status` remain available
on any host and show the pin.

## Scheduling and gates

With no step names, `bob tick` runs due steps. Due time is measured from the
last start, rounded down to the minute; a failed, timed-out, or
`inputs_changed` child still starts the next interval. Skipped steps, closed
precondition gates, and `vault_busy` results do not consume it. A backwards
clock change is ignored with a warning.

Naming steps runs only those steps now, in catalog order, while keeping safety
gates. `--all` runs every step that is not `off`, including manual steps. A
scheduled intake step that starts also brings interval-based `projects` and
`tasks` forward, even before their next interval. If either cannot start,
a saved follow-up retries it on the next scheduled pass.

Calendar and task reconciliation wait until today's daily note exists, is
rendered, and contains `## Pomodoros`. Calendar runs include yesterday on the
first successful run of each local day. Automated Calendar runs also observe
the one-hour cooldown shared with direct `bob gcal sync` runs. A child that
times out on the vault or scan lock reports `vault_busy` and waits without
consuming its interval, as does Gmail when the vault is busy.

Scheduled reference scans run when files in `lib/` or `xlib/` changed, a
pre-scan hook is configured, or six hours have passed since a full scan.
Named `refs` and `--all` always scan. An unchanged scan gate is checked again
on the next tick.

## Results and recovery

Each step reports `ok`, `failed`, `waiting`, or `skipped`, with a reason and a
short summary. Waiting covers unmet preconditions such as a missing daily
note, a busy vault, notes edited mid-scan, or an unreachable network. Exit
status is 0 when steps succeeded, waited, or were not due; 1 when a step
failed; and 2 for usage/configuration errors or a writer-host mismatch. A
step timeout fails that step and the later steps continue after its process
group is stopped. A handled signal exits with `128 + signal number` (for
example, 143 for `SIGTERM`).

A step that fails only because the network or a Google/Git remote is
unreachable waits as `offline` with `offline · retrying next tick` and exits
0: `error.kind` or top-level `kind` in
`network | timeout | rate_limited | transient`, a Gmail `failure_kinds` made
only of those kinds, or a vault-sync remote failure surfaced as
`kind: network` (projects `sync_failed`, refs `pre_sync_failed`). The step
reruns on the next tick without consuming its interval.

Contention waits escalate: when a step keeps reporting `vault_busy`,
`inputs_changed`, or `offline` for 2 hours without a success, the next wait
reports `failed` with reason `stalled` and a summary like
`still waiting after 2h · vault busy · retrying next tick` (or
`still waiting after 2h · offline · retrying next tick`). The stalled
failure consumes the interval, so a stuck lock mails once per interval
instead of every 5 minutes. Gate waits (`note_missing`,
`note_unrendered`, `pomodoros_missing`) never escalate.

Failures name their cause: the row `detail` carries the child's message on
the first line and its hint on the second line when they differ. A Gmail
failure carries the first `failure_kinds` entry plus the first warnings
line.

Reasons are stable codes: `due`, `never_ran`, `follows_intake`, `follow_up`,
`named`, and `all` explain a run; `not_due`, `manual`, `off`, and `unchanged`
explain a skip; `note_missing`, `note_unrendered`, `pomodoros_missing`,
`vault_busy`, `inputs_changed`, and `offline` explain a wait; `exit_nonzero`,
`timeout`, `spawn_failed`, `interrupted`, and `stalled` explain a failure.

Only one run proceeds at a time. A second non-interactive run prints
`already running` with the step in flight and exits 0 without writing state
or a receipt; an interactive run waits for the first to finish.

Scheduler state and the latest receipt are stored per host under
`$XDG_STATE_HOME/bob-cli/tick/`. The state records per-step starts, outcomes,
successes, and pending follow-ups. `bob tick --status` displays the step
board; `bob tick --format json` emits the receipt. On macOS, each locked run
also posts `org.bobs.bob.tick-finished`.

The receipt (`last.json`) carries `schema_version`, `command`, `host`,
`vault`, `trigger`, `started_at`, `finished_at`, `exit_code`, and `steps[]`.
Each step row carries `name`, `cadence`, `outcome`, `reason`, `detail`,
`started_at`, `duration_ms`, `exit_code`, `summary`, `next_due_at`,
`last_success_at`, and, for gated steps, a `gate` object with `kind`, `open`,
`reason`, and `detail`. It also carries `last_finished_at`, `last_outcome`,
and `last_summary`. Reference gates can add `fingerprint_changed`, `hook`,
and `full_scan_due_at`. Summaries contain counts or scheduler status, never
mail or calendar text.

Read-only views run no steps and leave scheduler state and `last.json` unchanged:

| View | JSON fields |
| --- | --- |
| `bob tick -d -f json` | `mode: "plan"`; per-step `would_run` reflects cadence and gates. No row claims work ran. |
| `bob tick -s -f json` | `mode: "status"`; saved step results with current gate information for steps selected by the scheduler. Optional step names filter the board. |

Receipts have `schema_version: 1` and `command: "tick"`, with no `ok` field;
`exit_code` describes this invocation. In status mode, check the saved step
outcomes for previous failures. A wrong-host read-only view adds `banner`.
An already-running response has `reason: "already_running"` and
`steps: []`, without receipt timestamps. Configuration and writer errors
print to stderr without a JSON receipt, even with `-f json`.

`SIGINT`, `SIGTERM`, and `SIGHUP` stop the active child process group and
record an interrupted failure. If `bob tick` itself receives `SIGKILL`, the
child may outlive it.

## Troubleshooting

- `bob tick -d` shows what would run now and which gate holds each step.
- `bob tick -s` shows each step's last result, last success, and next run.
- `cronctl logs bob-tick --machine mac` shows the Mac's recent tick history.

## Related command contracts

- [Calendar sync](gcal.md#sync)
- [Gmail sync](gmail.md#sync)
- [Keep pull](gkeep.md#pull)
- [Reference scans](ref.md)
- [Project sync](projects.md#commands)
- [Task reconciliation](task-status-hooks.md#usage)

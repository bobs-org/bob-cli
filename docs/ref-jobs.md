# Ref jobs (`bob ref jobs`)

A bare public link captured with `bob capture` is never clipped
inline: capture queues one durable **ref job** and returns at once,
and a detached background worker clips it through the same typed
ingest as `bob ref create`. The capture confirmation says
`queued … → reading queue`. That names the job, not a row in
`bob ref list`. Capture planning and submission do not fetch
the URL; the worker needs network access and the target's normal
[import dependencies](highlights-create.md#prerequisites).
A failed clip falls back to exactly the inbox task capture would
have written, plus a ⚠️ bullet with the reason and a retry command.

## Lifecycle

```
pending → clipping → clipped | already in library | already queued
                  ↘ fell back (clip failed, inbox task written)
                  ↘ stuck (the fallback write itself failed)
```

One attempt in v1: a retryable failure still falls back instead of
retrying. `bob ref scan` turns clipped intake PDFs into ref notes
later. Nothing in bob-cli starts that scan. The published
[Mac scan job](highlights-ref-sync.md#scheduled-scan) runs hourly
(`StartInterval` 3600, or cron `0 * * * *`) and the example command is
`bob highlights scan --dry-run` until you remove `--dry-run` after clean
cycles. `bob highlights` is a permanent alias of `bob ref`. On another
setup, run `bob ref scan` yourself. The `bob ref jobs` footer currently
prints `the Mac runs it every 15 minutes`; follow the Scheduled Scan
runbook for the interval and the dry-run command. Neither capture nor the
worker runs `scan`, and neither returns a ref-note path before it exists.
Use `bob ref jobs` to inspect work before the note appears in
`bob ref list`.

These jobs are created by `bob capture`. `bob ref create` imports inline,
and [Google Keep URL imports](gkeep.md#url-only-notes-go-to-the-reading-queue)
also clip inline during `pull`; they do not use this spool.

## Layout

Everything lives under `${XDG_STATE_HOME:-~/.local/state}/bob-cli/ref/jobs/`.
Directories are mode 0700 and files 0600.

The spool is shared by all vaults using the same user state directory.
Each job file stores its destination `bob_dir`. `jobs list` and `jobs run`
inspect or process those stored jobs rather than selecting the current
`BOB_DIR`. Human rows and JSON list entries do not print `bob_dir`; it is
only in the job file. A JSON `path` is that spool file, not the vault.

```text
ref/ingest.lock              machine-wide ingest lock (capture worker and bob gkeep pull)
ref/jobs/pending/<id>.json   queued jobs
ref/jobs/running/<id>.json   the job the worker is clipping
ref/jobs/stuck/<id>.json     jobs whose fallback write failed (never lost)
ref/jobs/done.jsonl          terminal outcomes (last 1000 once past 2000)
ref/jobs/worker.lock         single-flight worker lock
ref/jobs/worker.log          the detached worker's stdout and stderr
```

**Job file** (`schema_version: 1`; temp file, fsync, rename, fsync
directory):

```json
{
  "schema_version": 1,
  "id": "20261007T143012-3f9a1c",
  "created_at": "2026-10-07T14:30:12-04:00",
  "source": "capture",
  "bob_dir": "/home/bryan/bob",
  "url": "https://example.com/post?utm_source=x",
  "cleaned_url": "https://example.com/post",
  "dedupe_key": "https://example.com/post",
  "display": "example.com/post",
  "route_hint": "article",
  "parent": "mac_inbox",
  "attempts": 0,
  "fallback": {
    "relative_target": "mac_inbox.md",
    "task_line": "- [ ] #task https://example.com/post?utm_source=x [created::2026-10-07]"
  }
}
```

`parent` is the resolved parent route capture staged for the job
(`schema_version` stays 1; jobs written by an older `bob` omit it and
the worker uses the source's inbox instead: `capture` →
`mac_inbox`). `bob ref jobs` shows it as `→ <parent>` on pending and
clipping rows and as `parent` in `-f json`.

`fallback.task_line` is the exact line capture would have written for
the item with routing off, so its `created` date is the capture day.

**`done.jsonl` record:**

```text
{schema_version, id, url, cleaned_url, display,
 outcome: created|already_in_library|already_queued|fell_back,
 parent?, pdf?, note?, error?{kind,message,retryable},
 fallback?{relative_target}, created_at, started_at, finished_at}
```

## Worker

`bob ref jobs run` holds `worker.lock` (non-blocking) for the whole
pass; a second worker prints `another clip worker is running` and
exits 0. It then:

1. **Recovers.** A `running/` file is stale, because the lock is
   held. At 2+ attempts it fails as `internal` ("the clip worker
   stopped twice while clipping this link") and falls back without
   clipping again; otherwise it moves back to `pending/`.
2. **Retries `stuck/` fallbacks** without clipping again.
3. **Clips `pending/`**, oldest first, claiming each job into
   `running/` with a `started_at` stamp.

A clip success appends `created`, `already_in_library`, or
`already_queued` to `done.jsonl`. Human output and JSON `state` say
`clipped` for a `created` outcome. A clip failure writes the
fallback task and appends `fell_back`; when the fallback write
itself fails, the job parks in `stuck/` with both errors and the
run exits 1. After releasing the lock the worker re-checks
`pending/`, so a job queued mid-pass is never stranded without
another kick.

## Kick

After a successful capture commit with at least one new job, `bob
capture` kicks `bob ref jobs run -q` fully detached: new session and
process group, stdin from `/dev/null`, stdout and stderr appended to
`worker.log`. The kick never waits and never inherits the caller's
pipes, so a submit returns in well under a second while the clip
runs. A kick failure prints a warning without changing the exit
code. `BOB_REF_JOBS_KICK=off|0|false` disables it.

## Fallback

A failed clip writes `fallback.task_line` into the job's parent note
plus one child bullet:

```text
⚠️ Clip failed (<kind>): <message> · retry: bob ref create <url> -P <parent>
```

The write goes through capture's staged-file commit — same section
placement as capture, same disk-preimage guard with re-read
retries — and never takes the vault lock, matching capture. A
missing target is created exactly as capture would create it.

## Commands

```bash
bob ref jobs [-a|--all] [-f|--format human|json]  # list (bare default; read-only)
bob ref jobs list [-a|--all] [-f|--format human|json]
bob ref jobs run [-q|--quiet]                     # clip pending jobs
```

A bare `bob ref jobs` lists; it never clips or writes. Pending,
clipping, and stuck jobs always show. Finished jobs older than 7 days
need `-a/--all`. When the visible set is empty, the human header reads
`bob ref · jobs · nothing pending · nothing in the last 7 days`, including
after `--all`. Human `run` prints one line per job (`⟳ clipping …`, then
`✓ clipped … → xlib/blogs/post.pdf` or `↩ fell back … → mac_inbox.md
(blocked: …)`), then a summary (`ok 2 clipped · 1 fell back`, or
`nothing to do`).

## Doctor

`bob ref doctor` prints a `ref jobs:` row with pending, stuck, and
the oldest pending age. It warns when a pending job is older than 1
hour or any job is stuck (hint: `bob ref jobs run`).

## Troubleshooting

- `bob ref jobs` shows every state; `bob ref jobs -f json` is the
  machine-readable form.
- `worker.log` holds the detached worker's stdout and stderr (trimmed
  to the last 256 KiB past 1 MiB).
- `stuck/` jobs need `bob ref jobs run`: it retries the fallback
  write without clipping again.
- A pending job older than an hour usually means the kick never ran;
  run `bob ref jobs run` by hand and check `worker.log`.

### Multiple vaults

Current in-flight deduplication compares URL keys across the shared spool
without checking the destination vault. If the same link is pending or
running for vault A, capture in vault B reports `already clipping` and queues
no job for B. The list does not show which vault owns the in-flight job.
Wait for A's job to finish and capture the link again in B, or use
`bob ref create --bob-dir /path/to/vault-b '<URL>'` to import it inline.
That inline import does not use this spool. Stuck and finished jobs do not
block a new capture.

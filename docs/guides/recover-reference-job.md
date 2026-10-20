---
title: "Recover a stuck reference job"
description: "Inspect a stuck ref job, run the worker to retry its fallback write, and confirm the job left stuck."
tags:
  - CLI
---

# Recover a stuck reference job

This guide recovers a reference job whose clip failed and whose fallback
inbox write also failed. `bob ref jobs run` retries that fallback write
without clipping the URL again. There is no separate retry or delete
subcommand.

## Before you start

Install the CLI. Ref jobs live in the user state spool
(`${XDG_STATE_HOME:-~/.local/state}/bob-cli/ref/jobs/`), not in the vault.
`bob ref jobs` lists that spool; it does not select `BOB_DIR`.

## 1. Inspect job state

```bash
bob ref jobs
bob ref doctor
```

Pending, clipping, and stuck jobs always show. `bob ref doctor` prints a
`ref jobs:` row with pending and stuck counts and warns when any job is
stuck, with the hint `bob ref jobs run`. Confirm a `stuck` row and note its
display URL. `bob ref jobs -f json` is the machine-readable form.
`worker.log` holds the detached worker's stdout and stderr.

A `clipped` (queued) job is already finished in the spool; drain it with
`bob ref scan -i`, not another worker pass.

## 2. Run the worker

```bash
bob ref jobs run
```

The worker holds `worker.lock`. A second run prints
`another clip worker is running` and exits 0. For a stuck job it retries the
fallback write only. Confirm a line for that URL and a summary that is not
`nothing to do`. Exit 1 means the fallback write failed again; the job stays
in `stuck/` with both errors.

A pending job older than an hour usually means the capture kick never ran;
the same `bob ref jobs run` clips those pending jobs (one attempt, then
fallback on failure).

## 3. Confirm recovery

```bash
bob ref jobs
```

Confirm the stuck row is gone. A successful fallback writes the original URL
task into the job's parent note plus a ⚠️ child with
`retry: bob ref create <url> -P <parent>`. Open that note and confirm the
task line. To clip the URL now, run the printed `bob ref create` command;
that inline import does not use this spool.

## If it goes wrong

If the job is still `stuck`, read `worker.log` and the ⚠️ reason, fix the
parent note path or vault lock, and run `bob ref jobs run` again. Do not
delete files under `ref/jobs/stuck/` by hand. If the same URL is pending for
another vault, wait for that job to finish or import with
`bob ref create --bob-dir /path/to/vault-b '<URL>'`.

## Related

- [Ref jobs](../ref-jobs.md)
- [Save a link to your reading queue](save-link-to-reading-queue.md)
- [bob ref](../reference/cli/ref.md)
- [Reference library](../ref.md)

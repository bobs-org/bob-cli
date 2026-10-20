---
title: "Save a link to your reading queue"
description: "Queue a public URL as a background reference job, then inspect job completion and the reading list."
tags:
  - CLI
  - macOS
---

# Save a link to your reading queue

This guide saves one supported public URL as a background reference job.
Capture returns at once and does not fetch the URL. The job files the PDF,
reference note, and reading task later. Success of the capture is the queued
job; the reading list updates when the clip finishes.

## Before you start

Install the CLI and point it at your vault. The host that runs the clip needs
the usual [import dependencies](../highlights-create.md#prerequisites). Mac
Capture submits the same draft through `bob capture`.

## 1. Preview and queue a supported URL

```bash
bob capture --dry-run 'https://example.com/article'
```

```text { .no-copy title="Output" }
[dry-run] ok would queue  example.com/article → mac_inbox
  new to your library · reading task lands in mac_inbox.md · file it later
```

Confirm the headline says `would queue` and names a parent. Then submit:

```bash
bob capture 'https://example.com/article'
```

```text { .no-copy title="Output" }
✓ queued  example.com/article → mac_inbox
  new to your library · reading task lands in mac_inbox.md · file it later
```

`queued … → reading queue` names the job. It is not yet a row in
`bob ref list`. Quote the URL when the shell would treat `#` as a comment.
Add `@workshop` (or `-r workshop`) to file the reading task under that
route. `-R` / `--no-ref` keeps the URL as an ordinary task.

On a Mac, paste the same URL into the capture panel, confirm the preview
says it would queue, and submit.

## 2. Inspect the job, then the reading queue

```bash
bob ref jobs
```

Pending, clipping, and stuck jobs always show. A finished clip from the last
7 days shows without `--all`. When the worker files the note you should see
`filed` (or `clipped` if the PDF is waiting in `xlib/`).

```bash
bob ref list
```

`bob ref list` is the reading queue: queued and started reference notes
under `ref/`. A job that only clipped to `xlib/` appears here after
`bob ref scan -i`. `already in library` at capture time means the link was
already known and no new job was queued.

Hosts without a dot, IP literals, and configured exclude hosts stay tasks.
See [URL routing](../ref.md#url-routing). To import immediately instead of
queueing, run `bob ref create 'https://example.com/article' -P mac_inbox`.

## If it goes wrong

If `bob ref jobs` still shows `pending` after an hour, the detached kick did
not run. Run `bob ref jobs run` and read `worker.log`. A failed clip writes
the URL as a task plus a ⚠️ retry bullet; a failed fallback parks the job as
`stuck`. Recover that with
[Recover a stuck reference job](recover-reference-job.md). Extra words,
tags, or child lines on the draft keep the URL as a task.

## Related

- [Capture syntax](../reference/capture-syntax.md#bare-public-url)
- [Ref jobs](../ref-jobs.md)
- [Reference library](../ref.md)
- [Recover a stuck reference job](recover-reference-job.md)
- [Capture a task from anywhere on your Mac](capture-from-your-mac.md)

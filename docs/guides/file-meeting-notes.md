---
title: "File meeting notes"
description: "Preview or file Event Task notes as a meetings reference from the CLI."
tags:
  - CLI
---

# File meeting notes

This guide files one Event Task's notes as a `meetings` reference with
`bob ref meet`. All names are fictional. Obsidian and Mac Capture gestures
below apply only where those clients expose them; this guide invents no Mac
shortcut.

## Before you start

Point `bob` at your vault. The host that files needs the usual PDF toolchain
(pandoc, XeLaTeX) and a clean vault-sync state. Meeting selection reads the
vault only.

## 1. List meetings that need notes

```bash
bob ref meet -b ~/bob
```

```text { .no-copy title="Output" }
Recent meetings · 1 need notes
2026-11-24 14:00–14:30 · Cedar Labs review
  EVENT 20261124#^cedar-review
  root job · missing
```

Filter with an EVENT token, `--day`, or `--window`. JSON clients use
`bob ref meet -l -f json`.

## 2. Preview the filing

Collect notes in a file (or the clipboard), then preview:

```bash
bob ref meet -i notes.md 20261124#^cedar-review -P job -d
```

The preview names the meeting, match evidence, destination, title, note and
PDF paths, task and relation, outline, and warnings. Nothing is written.
Editor review (`-e`) never launches under `-d`; an empty editor-only dry run
reports `no_notes`.

## 3. File the notes

```bash
bob ref meet -i notes.md 20261124#^cedar-review -P job
```

Success files the reference note, reading task, `notes from` relation, full
recap, and meeting PDF in one scoped commit. To revise the draft first, pass
`-e` to review it in `VISUAL`, `EDITOR`, or `vi`. To file from the
clipboard, pass `-c` instead of `-i`. To include the transcript appendix,
pass `-t`.

When intake cannot file now, the result is `queued`: run
`bob ref scan -i` later. Queued output never says `filed`.

## 4. Replace filed notes

```bash
bob ref meet --overwrite -i revised.md 20261124#^cedar-review
```

Without `--overwrite`, a filed occurrence refuses with `already_filed`
naming the note and task. Overwrite keeps the note, task identity, LINKS,
and status unless `--status` is passed explicitly. An edited recap is
preserved with a warning.

## If it goes wrong

Ambiguous meetings and missing parents prompt with numbered pickers on a
TTY (`Which? [1] ›`, `File under which note? ›`); off-TTY and `-f json`
runs return `ambiguous_meeting` or `needs_parent` with candidates and exit
2. A stale event refuses or queues without editing the daily note. A missing
or inbox root may be re-rooted only to a valid chosen parent; other roots
stay untouched.

## Related

- [Meeting notes](../meeting-notes.md) — the full contract.
- [Reference library](../ref.md) — reading state and identity.
- [Recover Bob Refs](recover-bob-refs.md) — repair a filed reference.

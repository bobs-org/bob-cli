---
title: "Keep Ready lanes under their cap"
description: "Inspect per-note Ready counts, make room on crowded notes, then confirm the cap report."
tags:
  - CLI
  - Obsidian
---

# Keep Ready lanes under their cap

This guide brings each area and project note's Ready lane to or below its
soft cap. The cap counts the whole Ready lane by residence, including NEW
and ROTTEN rows. The dashboard READY badge is a separate freshness-gated
backlog.

## Before you start

Install the CLI and open the vault in Obsidian with **Bob Ledger Tools**
enabled. The default cap is `plan.max_ready_per_note` (5). A note may set
`ready_cap: N` or `ready_cap: off` in frontmatter.

## 1. Inspect per-note counts

```bash
bob ready -b /path/to/vault
```

```text { .no-copy title="Output" }
bob ready · Tue 2026-10-20 · cap 5 per note

  CROWDED 1 · 2 over · 1 full · 3 notes (2 area · 1 project)

  CROWDED
    workshop  7/5  ■■■■■│■■  +2   area · 6 new
  FULL
    garden  5/5  ■■■■■│  full   area · 5 new
  ROOM
    cash 2

  1 empty · not capped: gkeep_inbox 1 (ready_cap: off)

  Make room → bob ready workshop
  split Ctrl+Shift+N · defer Ctrl+Shift+P 1–4 · drop Ctrl+Shift+P x · sequence Ctrl+Shift+P b
```

Confirm which notes are CROWDED (over cap), FULL (at cap), or ROOM (under
cap). Inbox files that set `ready_cap: off` are exempt. `bob ready` exits 0
even when notes are crowded; pass `--check` if a script should exit 3.

## 2. Open one crowded note's worklist

```bash
bob ready workshop -b /path/to/vault
```

The worklist names each counted Ready task in that note. In Obsidian, the
same pressure shows as a CROWDED chip on `dash.md` and on the note's
`## Tasks` heading. Open `workshop.md` and confirm the extra Ready rows
match the CLI list.

## 3. Make deliberate room, then recheck

On a counted task, use one Task Card action (`Ctrl+Shift+P`):

- **Split** a large task (`Ctrl+Shift+N` from the hint line).
- **Defer** with `1`–`4` (rolls a scheduled date).
- **Drop** with `x` (cancels).
- **Sequence** with `b` (adds a prerequisite so the task becomes Blocked).

Recurring rows and `^prj` are not counted. Then:

```bash
bob ready -b /path/to/vault
```

Confirm the note left CROWDED. A remaining FULL note is at its cap, which is
allowed. To preview a different default for one run, pass `--cap 7`. To
raise or exempt one note, set `ready_cap:` in that note's frontmatter and
rerun `bob ready`.

## If it goes wrong

An invalid `ready_cap` prints `note_ready_cap_invalid` once and falls back
to the default cap. Fix the frontmatter to an integer 1–999 or `off`. Exit 2
means a missing vault, a bad `plan.max_ready_per_note`, or an unknown note
name. Crowded is a report, not a refused write.

## Related

- [Plan budget, Today, and lanes](../plan.md)
- [bob ready](../reference/cli/ready.md)
- [Vault format](../reference/vault-format.md)
- [Run the morning review walk](run-morning-review.md)
- [Add prerequisites while capturing](add-prerequisites.md)

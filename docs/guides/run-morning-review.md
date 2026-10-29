---
title: "Run the morning review walk"
description: "Inspect today's review queue, walk each due row, and keep or resolve with explicit gestures."
tags:
  - CLI
  - Obsidian
---

# Run the morning review walk

This guide walks today's freshness review queue. The CLI lists the due rows;
Obsidian is where you keep, complete, or skip them. A stamp records that the
task still needs doing as written. It does not change the task's lane.

## Before you start

Install the CLI, open the vault in Obsidian, and enable **Bob Ledger Tools**
and **Bob Navigation Hotkeys**. The walk keys below are the plugin defaults
and the Vim `]s` / `[s` bindings documented with them.

## 1. Inspect the queue

```bash
bob freshness -b /path/to/vault
```

```text { .no-copy title="Output" }
bob freshness - Tue 2026-10-20 - every 7d - pending 1d - next 1d - keeps 3

  REVIEW 6 due - 1 pre - 2 new - 1 pending - 1 tickler - 1 post

  PRE 1
    gtd_daily.md:1      Import inbox tasks from Google Keep  Checklist · complete to resolve
  NEW 2
    mac_inbox.md:4      Sharpen the shears                   created 2026-10-20
    garden.md:8         Water the seedlings                  created 2026-10-20
  PENDING 1
    workshop.md:12      Oil the hinges                       due today · fresh 2026-10-19 · every 1d (pending)
  TICKLER 1
    garden.md:20        Order compost                        tickler · scheduled 2026-10-20 · fresh 2026-10-01
  ── review closeout ──
  POST 1
    gtd_daily.md:20     Morning review                       Closeout · complete last
```

Confirm the `REVIEW N due` total and the tier groups. The walk order is PRE
→ NEW → PROJECTS → PENDING → NEXT → RECURRING → TICKLER → ROTTEN → POST.
Empty tiers are omitted. `bob freshness list --limit 10` shortens the printed
rows; counts still cover the whole vault.

## 2. Enter the walk in Obsidian

Press `]s` or `Ctrl+Alt+J` to land on the next due row (`[s` /
`Ctrl+Alt+K` go backward). `]s` from outside starts at PRE when PRE is due.
`[S` / `]S` cycle occupied review groups and always land on the destination
group's first due task. From ROTTEN, `]S` jumps to POST's first row. The
status-bar review footer shows the remaining due count and the current tier
while the cursor is on a queue row.

Confirm the caret is on the same note and line `bob freshness` listed, and
that a landing toast names the tier.

## 3. Keep or resolve the landed row

On the row `]s` just landed on, pick one explicit answer:

- **Keep and stay:** `Alt+F` stamps the task and stays on the row.
- **Keep and advance:** `Ctrl+Alt+F` stamps and moves to the next remaining
  due row. On a Pending task it may ask for an optional Work Log summary;
  blank Enter still keeps.
- **Skip without answering:** `]s` leaves the row due.
- **Complete a PRE/POST checklist row:** `Ctrl+Enter` completes it. On PRE it
  never leaves that group; `Ctrl+Alt+F` can cross into the next tier.
  Checklist rows are never stamped.

A counted keep on a due Ready ROTTEN or scheduled TICKLER row asks "Why
keep it as written?" before writing. The reason is optional — a blank
Enter logs `🤷 no reason given` — and Esc cancels the keep entirely.
Each answer lands as a dated entry under the task's `🥀 **ROTTEN
LOG**`, newest first, so later reviews show how many times, when, and
why the task was kept.

An inbox task asks where it goes before a non-closing answer writes. After
each keep, `bob freshness` drops that row from today's queue.

Walk NEW through TICKLER until the notice says commitments are done. ROTTEN
is optional upkeep. Finish POST last.

## If it goes wrong

If `]s` returns to a row you already left, that row is still the current
review task; press `]s` again to step from it, or `<C-o>` to jump back. A
fast second press during the short settle window can skip a row; `[s`
recovers it. `Ctrl+Shift+M` moves the task and does not advance the walk.
Do not run `bob freshness seed`; the one-time cutover already ran.

## Related

- [Task freshness](../freshness.md)
- [bob freshness](../reference/cli/freshness.md)
- [Bob Navigation Hotkeys](../reference/plugins/bob-navigation-hotkeys.md)
- [Keep Ready lanes under their cap](keep-ready-under-cap.md)
- [Continue in Obsidian](../start/obsidian.md)

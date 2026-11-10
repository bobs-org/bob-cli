---
keyword: Morning GTD Review Footer
aliases:
  - "review footer"
---

The bob-ledger-tools item in Obsidian's desktop status bar that keeps the morning GTD
review — the tiered `]s` walk over the shared [[task-freshness]] queue — in view
while you work through it. It answers three questions at a glance: how much is still
due, where the cursor sits in the walk, and whether the commitment tiers are done
(after them come ROTTEN upkeep, which is fine to stop partway, and the POST
closeout). Off a queue row it reads `⟳ Review N due · K commitments` (or
`Commitments done` / `Upkeep budget met`); the hint becomes the `Alt+P` preview
when the cursor is on an open task with a priority, and stays `]s next` otherwise.
On a queue row it reads `⟳ Review r/N · TIER i/M` plus that row's detail and, when the row has a
recommended roll, the `Alt+P` preview in the hint slot (for example
`Alt+P roll P2 1/1`); on TICK rows that preview renders as a tinted answer chip. The other nonempty tier
groups follow in walk order, then the `✓ upkeep/budget today` meter. To fit the
status bar it uses short tier labels — `WIP` for PENDING (WIP is an official alias of
the Pending status), `RECUR` for RECURRING, `TICKS` for TICKLER — explained in its
tooltip. Each group, and the current tier's context, wears that tier's own color; a
hairline divider separates the commitment groups from ROTTEN and POST; and as space
runs out it drops groups from the end of the walk (marking the cut with `…`), then
detail, hint, and meter, except on TICK rows, where the meter goes before the `Alt+P` chip.
Clicking jumps to the next due task like `]s`. It is a read-only projection: it never
stamps or changes tasks, its numbers are queue positions rather than progress, and it
hides when Tasks is unavailable. When nothing is due, it hides unless the cursor is
on an open task with a priority; then it shows only `⟳ Nothing due` plus that preview.
`]s` notices and
`bob freshness list` keep full tier names, and dashboard ROTTEN chips fold TICKLER
into ROTTEN where the footer splits them; see [[decisions/review-walk-is-tiered]].

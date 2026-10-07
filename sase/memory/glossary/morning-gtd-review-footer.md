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
`Commitments done` / `Upkeep budget met`) with a `]s next` hint; on a queue row it
reads `⟳ Review r/N · TIER i/M` plus that row's detail. The other nonempty tier
groups follow in walk order, then the `✓ upkeep/budget today` meter. To fit the
status bar it uses short tier labels — `WIP` for PENDING (WIP is an official alias of
the Pending status), `TICKS` for TICKLER, `REFS` for REFERENCES — explained in its
tooltip, and it drops groups, then detail, hint, and meter as space runs out.
Clicking jumps to the next due task like `]s`. It is a read-only projection: it never
stamps or changes tasks, its numbers are queue positions rather than progress, and it
hides when nothing is due or Tasks is unavailable. `]s` notices and
`bob freshness list` keep full tier names, and dashboard ROTTEN chips fold TICKLER
into ROTTEN where the footer splits them; see [[decisions/review-walk-is-tiered]].

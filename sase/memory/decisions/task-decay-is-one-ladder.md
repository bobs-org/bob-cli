---
keyword: Task Decay Is One Ladder
aliases:
  - task decay
  - one ladder
  - decay ladder
summary:
  "Repeated freshness keeps on WIP, Next, Ready, and P-level tasks follow one
  consent-based decay ladder (WIP → Next → Ready → P1 → … → P4): each rung
  allows a configured number of due keeps, then the next due keep opens one
  rung-aware card whose Enter steps exactly one rung down and whose Alt+F
  stays and restarts the count."
metadata:
  status: accepted
  decided: 2026-11-02
---

**Applies to.** bob-cli, bob-plugins, and the vault.

**Claim.** Repeated freshness keeps on WIP, Next, Ready, and P-level tasks
follow one consent-based decay ladder (WIP → Next → Ready → P1 → … → P4).
Each rung allows a configured number of due Alt+F keeps (Bryan: 2 on every
rung: `freshness.decay.pending_keeps` / `next_keeps` / `ready_keeps`, then
`keeps`, then 3; the priority property's per-level `decay.rolls` stays the
separate Ctrl+Enter roll allowance). A rung's N+1th due keep writes nothing
and opens one rung-aware decay card: Enter steps exactly one rung down (WIP →
Next → Ready → P1 → … → P4; P4 rolls again; never a drop), Alt+F stays and
restarts the count, E rewords, D drops. Early and Today-linked WIP/Next keeps
stamp quietly with an "early · not counted" notice: no prompt, no log entry.
Every keep, stay, step, and drop writes one rung-qualified Rotten Log entry
(blank reasons log 🤷); lane streaks start at 0 with no backfill, and legacy
entries parse as they are.

**Why.** The journal evidence: over 22 review days Next rows were kept 88
times (five tasks 11–12 times each), there are zero Next Rotten Log entries
because every Next reason was blank, and WIP holds 14 tasks against a soft
cap of 8. Ready-only decay never asked about any of them.

Evidence:
`research:202611/task_decay_ladder/task_decay_ladder.md`;
epic `bob-cli-g9` (`plan:202611/task_decay_ladder.md`).

Rejected alternatives:

- **Silent auto-demotion.** Nothing moves without an explicit answer.
- **Lengthening intervals only.** Cadence changes do not ask the urgency
  question the card asks.
- **Per-status lifetime fields.** One `[keeps::]` streak field; rung
  changes clear it.
- **An int-or-map `keeps`.** Flat per-rung keys keep older hosts working;
  an int-or-map value would make an older bob exit 2.
- **Folding rolls into `[keeps::]`.** The Ctrl+Enter roll streak
  (`decay.rolls`, Schedule Log derived) and the keep streak stay separate;
  the card reuses the ladder and writers but skips the roll recommendation.
- **A `held` flag for dependency-promoted lane rows.** No steady-state
  re-raise path exists: a linked task with an open prerequisite is Blocked,
  and its Task Link is consumed in the same run that raises its
  prerequisites, so the sticky lane is the only thing keeping it.

**Cost.** Card volume (about 1.7 a day at steady state), muscle memory
(mitigated by destination copy and undo), and three implementations kept in
sync (Rust, navigation, ledger-tools under shared vectors).

**Reopens when.** Stay dominates card answers for a month, or WIP grace
proves too short for agent epics.

Links [[glossary/task-decay]], [[glossary/keep-streak]],
[[glossary/rotten-log]], and [[glossary/task-freshness]].

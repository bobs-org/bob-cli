---
keyword: Task Decay
aliases:
  - decay
  - decay ladder
  - decay card
  - step down
---

The consent-based ladder that moves a repeatedly kept task one rung down, one
explicit answer at a time. Rungs: WIP (`[/]`; machine lane `pending`, the
user-facing alias of In Progress) → Next (`[*]`) → Ready (`[ ]` with no
priority) → P1 → P2 → P3 → P4 (each a `[priority::]` level). Blocked is an
overlay, never a rung.

Each rung allows a configured number of due Alt+F keeps (Bryan: 2 on every
rung). The allowance is the rung's `freshness.decay.pending_keeps` /
`next_keeps` / `ready_keeps` key (Ready and every P-level share `ready_keeps`),
then `freshness.decay.keeps`, then 3; `false` on one rung (or `decay: false`
overall) counts and logs but never asks. The priority property's per-level
`decay.rolls` (Bryan: 2) is the separate Ctrl+Enter same-level roll allowance,
not a keep allowance. A rung's N+1th due keep writes nothing and opens the
decay card: Enter steps exactly one rung down (P4 rolls again at P4), Alt+F
stays and restarts the [[keep-streak]] count, E rewords, D drops. Enter never
drops, nothing moves without a keypress, and nothing decays on a timer. Every
counted keep, stay, step, and drop writes one rung-qualified [[rotten-log]]
entry (a blank reason always logs 🤷); steps also write their [[schedule-log]]
decay entry. See [[decisions/task-decay-is-one-ladder]].

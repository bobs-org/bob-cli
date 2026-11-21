---
keyword: A Jump Successor Lands Last And Parks The Walk
aliases:
  - jump successor parks the walk
  - pinned close parks the walk
  - ctrl+j parks the walk
summary:
  "Ctrl+J pins at most one successor offer per card. After every write lands,
  Bob focuses that task; a pinned close from a ]s landing parks the walk at
  the closed row's neighbour instead of advancing."
metadata:
  status: accepted
  decided: 2026-11-21
---

**Applies to.** bob-plugins (bob-navigation-hotkeys, task-status-cycler),
Bob Mac Capture, bob-cli docs, and the Bob vault review ritual.

**Claim.** Ctrl+J pins at most one successor offer per card. The pin is
navigation only. After every write lands, Bob focuses that task. A pinned
close that started on a `]s` landing parks the walk at the closed row's
neighbour instead of advancing. The pin wins over Mac Capture's open-notes
landing.

**Why.** The research bottom line, plus the park precedent in
[[decisions/task-move-never-advances-the-walk]]: Bryan wants to land on the
chosen successor and keep working there, so the walk must wait.

Rejected alternatives:

- **Keying the pin off the ⌘↩ submit modifier.** That couples navigation to
  one capture key and leaves Obsidian closes without a pin.
- **Landing after the walk advance.** Two landings: the next review row,
  then the pin. The walk would already have left the closed neighbour.
- **Persisting the pin in the vault.** The pin is not a write; it dies with
  the card.

Evidence:
`research:202611/successor_jump_pin/successor_jump_pin.md`;
`plan:202611/successor_jump_pin.md`;
bob-navigation-hotkeys 2.95.0 (successors v5, reviewWalk v2, notice v5);
task-status-cycler 1.52.0 (api v6);
`docs/freshness.md` §§6/13 and `docs/task-dependencies.md` §12.24 in bob-cli.

**Cost.** Reaching the next review item after a pinned close takes one `]s`.

**Reopens when.** Bryan presses `]s` after nearly every pinned close.

Links [[decisions/task-move-never-advances-the-walk]] (the park this close
reuses), [[decisions/answering-advances-the-walk]] (the advance this close
stays out of), and [[decisions/ctrl-enter-leaves-pre]] (the PRE-exit a
pinned last-chore close replaces).

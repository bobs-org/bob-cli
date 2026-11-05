---
keyword: Open Pomodoro Names Follow Task Links
summary: Every live Task Link under one of today's open, typed Pomodoros agrees
  with its entry's name; writers, the close settle, and bob task reconcile
  enforce it, and reconcile never writes task fields.
metadata:
  status: accepted
title: Open Pomodoro Names Follow Task Links
aliases:
- open-pomodoro-names-follow-task-links
---

# Open Pomodoro Names Follow Task Links

*Requested · project*

**Applies to.** bob-cli, bob-plugins, and the vault.

**Claim.** The shared contract's rule and enforcement layers: `agree_entry` keeps
an open entry's name in agreement with its direct Task Link children (live links
add, unsupported types drop only when every child is live or worked, prose or
unknown children protect, MEET entries and containers never rename);
`settle_closed` names a closed session from its worked types; writers agree the
entries they touch at their shared write paths; and `bob task reconcile` repairs
today's open names through the shared applier. Reconcile never writes `[type::]`
fields and never moves links.

**Why.** The task is the deliberate, durable fact; names change at today's daily
only; Enter already merges.

Rejected alternatives:

- **Warning-only.** Flag drift and never repair.
- **Reconcile retypes tasks.** Repair names by changing task fields.
- **Reconcile moves links.** Repair names by relocating links.
- **Add-only.** Grow missing types but never drop unsupported ones.
- **Removal only with same-run proof.** Drop a type only with proof from the
  same run.
- **Rebuilding names with no guards.** Rewrite names without the
  live-or-worked, prose/unknown-protects, and last-type guards.
- **Provenance or pinned metadata.** Track name authority per entry.
- **Read-time derived names.** Compute display names without writing them.
- **An in-editor normalizer.** Fix names in the editor instead of writers and
  reconcile.

**Cost.** A hand-typed entry rename reverts when the links disagree; a running
session can be renamed while watched; reconcile makes unattended name writes
behind the quiet period; and a run whose type inputs change between planning
and writing defers instead of writing stale names.

**Reopens when.** Deliberately planned types get dropped more than rarely.

Evidence:
`research:202611/pomodoro_names_follow_task_types/pomodoro_names_follow_task_types.md`;
`plan:202611/pomodoro_names_follow_task_types.md`.

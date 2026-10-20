---
title: "Why the vault syncs through git"
description: "What git sync guarantees for the Bob vault, what it costs, and where conflicts go."
search:
  boost: 2
tags:
  - CLI
  - macOS
---

# Why the vault syncs through git

This concept explains why each machine keeps a full Markdown vault and reconciles it through git.

Every machine holds the same vault: plain Markdown files, with commits recording each change. A sync cycle commits local work, fetches the shared history, reconciles copies through fast-forward or merge, and pushes the result. The useful properties of this design are history, coordination with maintenance writers, and bounded recovery. History shows who changed what and lets any host re-read or repair an earlier state. Coordination comes from sharing one maintenance lock on each host, so background sync, nightly maintenance, task-status writes, and reference scans never mutate the vault concurrently. Recovery is bounded: supported conflicts resolve into quarantined copies instead of silently overwriting work.

Git does not provide real-time collaborative editing or a lock across all hosts. Two editors on two machines can diverge, and reconciliation happens on the next cycle, not keystroke by keystroke. The shared maintenance lock serializes cooperating operations on one host; it does not imply that editors or remote machines share it. Sync keeps honest copies honest; it does not make simultaneous edits merge themselves. That delay is the price of plain files: every host can keep working offline, and the next cycle shows exactly where histories diverged.

## Supported conflicts

When both sides change the same content, remote content stays at the working path and the local version is preserved under `_conflicts/`, with an event record in `_conflicts/sync_conflicts.md`. Quarantined copies are committed like any other change so they reach every host, but they are excluded from active task views: vault walkers, the Tasks query, and Dataview all skip the conflict directory, so a conflict copy never appears as live work.

Unhandled or repeatedly interrupted merges can halt sync and need attention. A halted cycle records its state and stops before changing the vault again; later cycles keep halting on the same merge until it is resolved by hand. Status, credentials, triggers, recovery, and rollback live in the runbook. This page states the tradeoff; the runbook owns the procedure. In particular, never rewrite synced history to clear the vault: no reset-hard, no force-push, no take-ours-or-theirs shortcut.

## References ride the same vault

Reference creation fits the synchronized vault, and annotation ownership stays separate. The host that renders a reference files it under the same locks and commits, so the note and its reading task travel through sync like any other vault change. Annotation sync remains the Mac's full scan: one writer owns highlights, while every host can produce references. Keeping those two responsibilities distinct is why intake and annotation have separate runbooks even though both end up as files in the same repository.

## Related

- [Your first Bob day](../start/first-bob-day.md) — sync the vault after a day of work.
- [Tasks and lanes](tasks-and-lanes.md) — why quarantined copies never become tasks.
- [Understand Bob](index.md) — the eight nouns and where sync fits.
- [How Bob fits together](how-bob-fits-together.md) — which surfaces write the vault.
- [Bob vault Git sync runbook](../vault-git-sync.md) — the behavioral authority for triggers, conflicts, and recovery.

---
metadata:
  status: accepted
summary: >-
  The creating host files each reference under the maintenance and scan locks,
  queues locally when unsafe, and leaves annotation sync to the Mac.
---
# References Are Ingested At Birth

**Applies to.** bob-cli and the vault hosts.

**Claim.** The host that renders a reference files it under
`bob_sync.lock → scan.lock`, with a pre-sync and one scoped commit. If it cannot
ingest safely, the PDF stays in that host's `xlib/` for its local intake drain.
Annotation sync remains the Mac's full scan.

**Why.** The research measured a median 4.6-minute delay, an 11.6-hour maximum,
and a 24% rate of commits that contained a new library PDF without its note.
Option G closes that split-commit window and makes a reference available from
the creating host without waiting for the Mac. It rejects A's tighter Mac cron
and B's SSH nudge because neither works while the Mac sleeps; C's filesystem
watchers are unreliable or Linux-only; D's full scan on each host breaks the
single annotation writer and widens the partial-commit window; E's tracked
`xlib/` duplicates binary Git churn and makes athena a single point of failure;
F's athena outbox is sound but more complex than needed unless real
cross-host collisions appear.

**Cost.** Agents' creates commit to the vault, and parent-note merges rely on
Git.

**Reopens when.** `_conflicts/sync_conflicts.md` shows same-stem collisions;
revisit the single-publisher design then.

Evidence: `research:202610/ref_create_auto_ingest_across_tailnet/ref_create_auto_ingest_across_tailnet.md`;
`plan:202610/ref_ingest_at_birth.md`; bob-cli `85eac25` (writing-scan lock),
`c4ac06f` (create ingest), `fa7745f` (intake-only drain), `5848c39` (producer
ingest), and `4703619` (intake health).

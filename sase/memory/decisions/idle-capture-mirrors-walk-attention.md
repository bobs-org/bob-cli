---
keyword: Idle Capture Mirrors Walk Attention
summary: Today's Bob walk attention appears as s/w/b marks in the idle agenda title row, from an independent `bob walks` refresh lane.
metadata:
  status: superseded-in-part
  superseded_by:
    - decisions:review-attention-follows-the-walk
    - decisions:w-count-reads-lane-excess
---
*Accepted · project*

This decision partly supersedes [[decisions:idle-capture-shows-ledger-agenda]]: that
record still governs the ledger agenda facts, fit, and folding, while this one governs
the additional walk-attention marks.

**Applies to.** Bob Mac Capture's idle agenda and `WalkAttentionStore`; bob-cli's
`bob walks --format json` report; and the shared walk-attention contract implemented
by bob-ledger-tools.

**Claim.** The idle agenda mirrors today's Bob walk attention in its title row:

1. **Bob owns the attention facts.** The same-day `bob walks` report supplies each
   family's availability, attention, tier, label, and counts. Capture does not read the
   vault or recompute the selector's predicates in Swift. The existing ledger agenda
   still gets its own facts from `bob capture-pomodoros --tasks`.
2. **A mark appears exactly when its same-day walk is available and needs attention.**
   In the default idle agenda view, the trailing title row shows the lowercase
   monospaced `s`, `w`, and `b` marks in that order, each with the report's tier hue dot.
   Calm, unavailable, or wrong-day families show no mark or placeholder. The panel does
   not mirror Obsidian's selected walk family; selection changes no attention fact.
3. **Counts and labels keep the marks useful and legible.** `s` carries Bob's due
   commitment count and `b` its waiting-row count; `w` stays bare because its report
   label names the over-limit lanes. The full Bob labels are available as help and
   accessibility text.
4. **Walk attention has an independent in-memory refresh lane.** It cannot delay the
   agenda request. The store date-guards snapshots, reuses fresh same-day data on show,
   and refreshes when dirty, missing, or at least 60 seconds old. Relevant filtered
   vault events debounce for 2 seconds while the panel is visible and 10 seconds while
   hidden. Launch, submit, wake, unlock, day or clock change, and Recheck Bob refresh
   immediately. A failed refresh keeps the last good same-day marks and reports the
   error in Settings; a Bob version without `walks` shows no marks and an upgrade hint.

**Why.** The title row gives attention a stable place beside the agenda while Bob's
report keeps the facts aligned with the Obsidian walker. A separate process lane keeps
the whole-vault walks scan from delaying the agenda's first frame or refresh. Rejected
alternatives:

- **Recompute attention in Swift.** That would create a second engine and risk drift
  from the walk selector.
- **Publish attention from the plugin to a file.** The file could be stale when
  Obsidian is closed and would tie freshness to plugin writes.
- **Add walks to `capture-pomodoros --tasks`.** A whole-vault scan would make every
  agenda refresh wait on the slower request.
- **Mirror the selected walk.** Obsidian's local selection only changes `]]` repeats;
  the same attention fact uses the same dot treatment either way.

**Cost.** The app runs a separate whole-vault request on the refresh triggers above and
keeps its last successful same-day report in memory. When refresh fails, that report may
be stale until the next successful scan; an older Bob shows an actionable upgrade hint.

Evidence: epic `bob-cli-mx` and approved plan
`plan:202611/mac_walk_attention_marks.md`; bob-cli `c50f8895` (`bob walks` and
`docs/walks.md`); bob-plugins `5964d35` (walker footer pinned to the walks vectors);
bob-mac-capture `fee9ffab` and `a19bdd0a` (walks client, presentation, store, and
marks); docs `docs/reference/mac-capture/idle-agenda.md` and
`docs/reference/mac-capture/settings.md`; exact-SHA macOS CI run `44240009172` passed on
master `a19bdd0a`.

**Reopens when.** The `bob walks` attention predicate or JSON contract changes, the
idle agenda starts writing the vault or deriving attention locally, or walk marks gain
an action beyond displaying Bob's report.

Superseded in part: [[decisions:review-attention-follows-the-walk]] retires the
commitment-count `s` claim — `s` is now lit whenever `]s` has a row to land on
and its count reads the walk total; the refresh lane and the `w`/`b` rules here
remain in force.

Superseded in part: [[decisions:w-count-reads-lane-excess]] retires the
bare-`w` half of claim 3; `w` now shows Bob's `counts.excess`.

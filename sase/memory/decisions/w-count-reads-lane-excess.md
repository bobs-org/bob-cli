---
keyword: The Mac `w` Mark Counts Lane Excess
summary: The Mac `w` mark shows Bob's summed WIP/NEXT/READY lane excess.
metadata:
  status: accepted
  decided: 2026-11-18
---

**Applies to.** bob-cli `bob walks` and `docs/walks.md`, and Bob Mac Capture's
idle-agenda `w` mark.

**Claim.** An available `w` reports `counts.excess = Σ max(0, count − cap)` over
WIP, NEXT, and READY, which is the fewest task changes that silence `w`. The Mac
`w` shows it the way `s` shows `counts.walk`. An older bob without the field keeps
a bare `w`. The label and walker footer are unchanged.

**Why.** Bryan's 2026-11-18 request, including the WIP 11/10 and NEXT 22/20 → `3`
example. Rejected alternatives:

- Keeping `w` bare.
- Having Swift sum `lanes`, which breaks the thin-client rule.
- Counting only WIP and NEXT, since READY over its cap also lights `w`.
- Putting the number in the shared label, which breaks byte-equality with the
  walker footer.

**Cost.** One more JSON field.

Evidence: approved plan `plan:202611/w_walk_excess_count.md` and W9 in
`docs/walks.md`.

**Reopens when.** The `w` predicate changes, a lane joins or leaves `w`, or the
footer gains its own `w` count.

[[decisions:idle-capture-mirrors-walk-attention]]

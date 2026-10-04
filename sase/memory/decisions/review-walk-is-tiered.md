---
keyword: Review Walk Is Tiered With Daily Lane Review
aliases:
  - tiered walk
  - tiered morning review
  - daily lane review
  - walk tiers
summary:
  "The ]s walk visits one shared queue in explicit tiers NEW → PENDING →
  NEXT → RETURNED → ROTTEN; Pending and Next tasks come due for daily review
  under pending_interval / next_interval (default 1, false walks that lane
  off); tiers never feed buckets or chips; upkeep outside the lanes counts
  the budget; stamps stay and the seed never re-runs."
metadata:
  status: accepted
  decided: 2026-10-01
---

**Applies to.** bob-cli, bob-plugins, vault, dash.

**Claim.** The morning walk is a structural tier order in the shared
evaluator (Rust plus JavaScript, kept in sync under the walk vectors):
NEW → PENDING → NEXT → RETURNED → ROTTEN, with per-tier comparators in
`docs/freshness.md` §4. Pending (`[/]`) and Next (`[*]`) tasks come due
for a daily review set by `pending_interval` / `next_interval` (integer
1–365 or `false`; absent or null means the default 1). A walked lane's
interval overrides the whole Ready chain for that task (`[refresh::]`,
note `task_refresh`, `freshness.interval`); `false` walks that lane off
and its tasks fall back to the Ready chain. Tiers are due-only: a task
stamped today drops out of the walk, and recurring, daily-note, hidden,
dependency-blocked, future-scheduled, and Today-linked tasks are in no
tier. The walk stays out of buckets and chips: `state()`, `bucket()`, NEW
and READY gating, the `B = NEW ∪ RETURNED ∪ ROTTEN ∪ READY` partition,
and the dash chips are unchanged, and lane rows carry null
`state`/`bucket`. The upkeep budget counts today's stamps on tasks outside
the lanes (`upkeep_today`), and every `✓` meter shows that number. No
stamps are stripped or flattened and `bob freshness seed` never re-runs:
the seeded `[fresh::]` dates come due by 2026-10-08 and are truthful
after that.

Rejected alternatives:

- **One interval-sorted queue.** A single `[refresh:: 1]` Ready task
  would outrank a Next task; the tier order is structural, not emergent.
- **A `scheduled` interval key.** A 1-day leash on every past-`scheduled`
  task would bring 59+ tasks back each morning. "Reviewed on the day it
  is due" is the RETURNED tier instead. Reopen as an opt-in Ready-only
  key if Bryan wants the daily nag.
- **Lanes always in the walk.** The `false` off-switch returns to
  Ready-only tiers (NEW → RETURNED → ROTTEN) with no code change.
- **A nav-only walk.** Both evaluators implement the contract; nav
  renders what the shared queue says.
- **Lanes in `bucket = rotten`.** Buckets and chips keep their partition;
  tiers are a separate walk dimension.
- **Stripping or flattening stamps.** That would turn about 150–200 tasks
  NEW at once; the census stays read-only.
- **An escalation sub-tier.** Deferred returned tasks roll their P-level;
  no extra tier tracks them.
- **A Ready default of 1.** The Ready backlog keeps its 7-day cadence;
  only the lanes default to daily.

Evidence:
`research:202610/tiered_morning_review_walk/tiered_morning_review_walk.md`;
`plan:202610/tiered_morning_review_walk.md`;
`docs/freshness.md` §§2/4/6/7/10–13 and schema 3 in bob-cli;
`api.freshness` namespace v4 in bob-ledger-tools;
tier notices, walk anchor, and lane-aware refresh row in
bob-navigation-hotkeys 1.50.0.

**Cost.** About 25 lane decisions a day at the caps (≤ 10 pending,
≤ 15 next); the rubber-stamp risk of a daily lane queue; two evaluators
kept in sync under the Q/L/R/S/B vectors.

**Reopens when.** The lane keep rate stays above 90% after lengthening
`next_interval`, or long-interval tasks starve past the weekly prune.

Amended in place 2026-10-04 at Bryan's request: the freshness trial was removed; nothing waits on it.

Links [[decisions/task-lanes-are-sticky]] (its "daily review with release"
cost) and [[decisions/ready-is-freshness-gated]] (its partition, chips, and
gating claims stand; only the review ritual order is superseded in part).

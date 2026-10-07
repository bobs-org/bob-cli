---
keyword: READY Is Freshness-Gated With NEW and ROTTEN Review
aliases:
  - gated ready
  - freshness-gated ready
  - new rotten ready
  - ready partition
summary:
  "READY is the freshness-gated confirmed/exempt backlog (visible TODO pool
  minus NEW and ROTTEN buckets) with TODAY → NEW → PENDING → NEXT → READY
  sections and NEW/PENDING/NEXT/READY/BLOCKED/ROTTEN/TODAY chips; review
  clears NEW then ROTTEN; no tags, fields, or status changes store review state."
metadata:
  status: superseded-in-part
  decided: 2026-10-01
  superseded_by:
    - decisions/note-ready-cap-counts-the-lane
    - decisions/review-walk-is-tiered
    - decisions/rotten-keeps-use-priority-decay
---

**Applies to.** bob-cli, bob-plugins, vault, dash.

**Claim.** READY means recently human-confirmed work plus the preexisting
freshness-exempt Ready tasks, computed at read time from the stable bucket
contract in `docs/freshness.md` §4 (`bucket(new)`, `bucket(rotten)` for
resurfaced and age-expired, null otherwise). The visible pool partitions as
`B = NEW ∪ TICKLER ∪ ROTTEN ∪ READY`, pairwise disjoint. Dash section order
is TODAY → NEW → PENDING → NEXT → READY; chip order is NEW, PENDING, NEXT,
READY, BLOCKED, ROTTEN, TODAY. NEW (unconfirmed, always visible, never
limited) clears to 0 first, then `rotten.md` (TICKLER plus expired ROTTEN)
until 0 or budget, then PENDING → NEXT. Tasks stay in their source notes;
`rotten.md` rows are click-through views and `]s` / Alt+Shift+F reviews
source lines. Rejected alternatives:

- **Stored `#rotten` / `#new` tags, persisted classification, new statuses,
  task relocation, or automatic confirmation.** Classification stays a
  read-time evaluation; stamps remain the only write.
- **Duplicate inline evaluators in dash queries.** Dash and rotten share one
  snapshot-backed bucket predicate and `reviewModel()`.
- **Dim-not-hide as the initial policy.** Review buckets stay out of READY;
  dimming is reconsidered only if gating hides needed work after interval/budget tuning.
- **Filtering READY with `state === "fresh"`.** That would lose exempt tasks;
  READY uses `bucket !== "new" && bucket !== "rotten"` on the visible pool.

Evidence:
`plan:202610/freshness_gated_ready.md`;
`research:202610/freshness_gated_ready_dash/freshness_gated_ready_dash.md`;
`docs/freshness.md` §§4/6/8/13 and `docs/plan.md` READY backlog in bob-cli;
`api.freshness.bucket` plus `reviewModel()` and gated `readyBudget` in
bob-ledger-tools 1.11.0; `dash.md` NEW/gated-READY plus `rotten.md`
TICKLER/ROTTEN groups in the vault.

**Cost.** READY visibility depends on review: skipping review lowers READY
without lowering total lane pressure. Headless `bob query` without the plugin
stays ungated; `bob freshness list` is the headless review interface. Two
implementations (Rust bucket plus JavaScript bucket/model) stay in sync under
schema 1 until the vocab-rotten migration.

**Reopens when.** A required plugin-free surface needs gating, or gating hides
needed work (a lost-needed-task case) after interval/budget tuning.

Supersedes in part [[decisions/today-is-read-from-the-ledger]] for the dash
section list only.

Superseded in part: chip list only — see [[decisions/note-ready-cap-counts-the-lane]].

Superseded in part: review ritual order only — see [[decisions/review-walk-is-tiered]].

Amended in place 2026-10-04 at Bryan's request: the freshness trial was removed; nothing waits on it.

Superseded in part: 'stamps remain the only write' only — the approved
decision card invokes separate existing writers on explicit approval, see
[[decisions/rotten-keeps-use-priority-decay]].

Amended in place 2026-10-07 at Bryan's request: the RETURNED walk tier is renamed TICKLER (machine tier `tickler`, footer label TICKS); the Ready state stays `resurfaced`.

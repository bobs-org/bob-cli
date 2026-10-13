---
keyword: Unblocked Tasks Resurface As Ticklers
aliases:
- unblocked ticklers
- unblocked field
- returned_by unblocked
summary: 'Every automatic Blocked-to-Ready recovery records [unblocked:: today] unless
  a WRITE skip applies, so the task is TICKLER in the next morning review; a keep
  on that row leaves the streak unchanged.'
metadata:
  status: superseded-in-part
  superseded_by:
  - decisions:unblocked-tasks-are-offered
---

**Applies to.** bob-cli, bob-plugins, vault.

**Claim.** Every automatic transition of an open task from Blocked
`[?]` to Ready `[ ]` records `[unblocked:: today]` on the recovered
line (replacing any older value), unless a WRITE skip applies: the row
is outside freshness scope, has no `[fresh::]`, is already a scheduled
tickler (`fresh(t) < scheduled(t) ≤ today`), or lands in Next or
Pending. Writers are reconcile Unblock; capture/cycler close recovery
(including `project_member`); nav/cycler project recovery of
residents; and nav recoveries of lines other than the gesture's own
line. Every stamp clears the field; reconcile prunes `[unblocked:: U]`
when `fresh > U`. At read time,
`returned_on` is the latest of an arrived `scheduled` and a valid
`unblocked` (`fresh ≤ unblocked ≤ today`); `returned_by` is
`scheduled` or `unblocked`, with `scheduled` on a tie; RESURFACED plus
tier TICKLER follow from `returned_on`. A keep on a
`returned_by: unblocked` row neither increments nor resets
`[keeps::]`, and `decide_for` never opens the decision card for it.
`docs/freshness.md` §4 and §10b own the contract; see also
[[decisions/review-walk-is-tiered]] and
[[decisions/ready-is-freshness-gated]].

**Why.** Recover-only, never-linked project members kept their old
`[fresh::]` and could sit FRESH, outside the walk, for up to six
mornings. Read-time `returned_on` from prerequisite completion dates
(option A) misses the project unblocks Bryan performs himself —
removing a `^prj` prerequisite or Ctrl+D of the project date — plus
deleted prerequisites and closed projects. Rejected alternatives:

- **Read-time `returned_on` (option A).** Misses those unblocks.
- **Clearing `[fresh::]`.** A stamp is a human confirmation; automation
  never stamps.
- **Recovering to Next.** That would invent a lane.
- **Successor-linking residents.** Residents recover recover-only.

Evidence:
`research:202610/unblocked_task_review_guarantee.md`;
`research:202610/prj_task_inline_schedule_and_deps.md` §4;
epic `bob-cli-6u` (`plan:202610/prj_line_project_blocking.md`);
bob-cli `f51ba28` (Rust freshness), `34d4ab6` (reconcile Unblock),
`13d183f` (close recovery);
bob-plugins `f47be84` (ledger-tools evaluator, `api.freshness` v12).

**Cost.** Automation writes a non-stamp freshness input. Recovery
writers live in four engines. The guarantee relies on reconcile having
run since the unblock. Lane walks must stay on. Old plugin versions do
not write the field during the mixed-version window; reconcile prunes
`fresh > U`.

**Reopens when.** A required surface cannot wait for reconcile, or
stamps must remain the only freshness write.

Superseded in part: project residents are now offered as successors and linked only
when picked; the former rejected "successor-linking residents" behavior is an explicit
offer. See [[decisions:unblocked-tasks-are-offered]].

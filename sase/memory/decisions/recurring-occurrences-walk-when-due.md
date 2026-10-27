---
keyword: Recurring Occurrences Walk When Due
aliases:
  - recurring review tier
  - nine-tier walk
  - hide-gated projects
summary:
  "The ]s walk is PRE → NEW → PROJECTS → PENDING → NEXT → RECURRING →
  TICKLER → ROTTEN → POST; due recurring occurrences walk in RECURRING
  (never stamped); schema 13 retired REFERENCES so refs use ordinary
  groups; a ^prj walks in PROJECTS only when it lacks #hide."
metadata:
  status: accepted
  decided: 2026-10-27
---

**Applies to.** bob-cli, bob-plugins, vault.

**Claim.** The shared freshness walk is nine tiers: PRE → NEW → PROJECTS →
PENDING → NEXT → RECURRING → TICKLER → ROTTEN → POST (`docs/freshness.md`
schema 14). A non-checklist recurring task whose occurrence date (the
earliest valid `scheduled`, `due`, or `start`) has arrived walks in
RECURRING, between NEXT and TICKLER (`tier_position=before_tickler`). It
is never stamped. It leaves the walk when completed, rescheduled past
today, linked to Today, or cancelled through Tasks. Alt+F and Ctrl+Alt+F
on a landed RECURRING row write nothing and stay (`ctrl_alt_f=refuse`);
the notice names the real answers (Ctrl+Enter, link, reschedule). Schema
13 retired the REFERENCES tier: `#ref` and legacy `^ref` tasks use
ordinary NEW, lane, TICKLER, and ROTTEN groups. `^prj` remains the only
tracker exception. A `^prj` walks in PROJECTS only when it lacks `#hide`,
which `bob projects sync` owns; there is no open-task occupancy count and
no frontmatter `scheduled` gate.

**Why.** Recurring rows were excluded from every stamp-resolved tier
because Tasks copies `[fresh::]` into the next occurrence and because
the old full-READY walk was assumed to surface them. After the due-only
walk replaced that READY walk, arrived occurrences vanished from `]s`,
the footer, and `bob freshness list` (four overdue non-checklist rows on
2026-10-08). RECURRING restores the "recurrence resurfaces it" half
without stamping. Grouping it with TICKLER keeps the date-driven arrival
tiers together and leaves NEW first after PRE. A reflexive Ctrl+Alt+F
must never mark a due obligation done. Bryan then asked that `^prj`
review follow the `#hide` tag `bob projects sync` already writes, and
that reference tasks follow the same freshness and review rules as other
tasks in their lane.

Rejected alternatives:

- **Stamping recurring rows.** Tasks copies `[fresh::]`/`[keeps::]`
  forward, and an overdue fixed-cadence completion would create an
  already-arrived occurrence that looks fresh and hides.
- **Folding the rows into TICKLER.** TICKLER resolves by stamp.
- **Walking undated recurring rows daily.** They have no occurrence; they
  get a `recurring_undated` lint instead.
- **Admitting `[?]` like checklist rows.** The 15-minute hooks restore
  `[ ]` after the date arrives; the row stays due the next morning.
- **`tier_position=after_pre`.** That would put due recurring obligations
  immediately after PRE chores and push NEW off the first post-chore
  slot.
- **`ctrl_alt_f=skip`.** Write nothing and advance like `]s`; a due
  recurring row would get no explicit answer.
- **Ctrl+Alt+F completes, as on PRE/POST.** A reflexive key must never
  mark Pay salary done.
- **Lifting the recurring exclusion for every task.** Recurring rows
  still never enter NEW or ROTTEN.
- **Keeping a REFERENCES tier and `reference_interval`.** Bryan asked
  that refs use ordinary lane freshness.
- **PROJECTS occupancy counts or a frontmatter `scheduled` gate.** Sync
  already encodes occupancy and future-schedule hiding in `#hide`.

Evidence:
`plan:202610/recurring_review_tier.md` (`tier_position=before_tickler`,
`ctrl_alt_f=refuse`);
`plan:202610/recurring_review_landing.md`;
`plan:202610/tracker_hide_gate_and_references_tier.md`;
`plan:202610/unify_reference_task_freshness.md`;
`docs/freshness.md` schema 14;
bob-cli `56a5e688` (schema 11 RECURRING), `54712fea` (`#hide` PROJECTS
gate), `87501347` (schema 13 ordinary refs);
bob-plugins `9233b0c` (ledger freshness namespace v9 `recurringTier`);
the 2026-10-08 census of four overdue recurring rows.

**Cost.** A due recurring row asks every morning until answered. An
overdue fixed-cadence task needs one answer per missed occurrence, or
one reschedule. An arrival-morning `[?]` row appears only after the
first hooks run. Hidden empty projects wait on `bob projects sync`
(every 15 minutes) before their `^prj` walks.

**Reopens when.** An arrival-morning miss is traced to a stale `[?]`, or
RECURRING rows are routinely skipped with `]s` (then they need a
snooze).

Links [[decisions/review-walk-is-tiered]] (the older nine-tier order,
blanket recurring exclusion, and REFERENCES claim this record
supersedes in part) and [[decisions/answering-advances-the-walk]]
(RECURRING answers stay; Ctrl+Alt+F refuses).

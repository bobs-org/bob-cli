---
keyword: Decision Rules Are Read At Planning Time
aliases:
  - planning-time decision reads
  - read decisions when planning
summary:
  "Decision records bind at planning time, not on every turn: planners read the
  decisions roster with `sase memory read roster:decisions` and cite governing
  records, researchers consult it before recommending, and unplanned changes in
  governed domains consult it first. The roster loads on demand so the 77-record
  catalog no longer rides along in every agent bundle."
metadata:
  status: accepted
  decided: 2026-11-25
---

# Decision Rules Are Read At Planning Time

**Applies to.** bob-cli, bob-plugins, Bob Mac Capture, and the Bob vault.

**Claim.** Decision records bind at planning time, not on every turn. Before writing
or revising a SASE plan, the planner reads the decisions roster with
`sase memory read roster:decisions -r "<why>"` and cites each governing decision as
`decisions:<keyword>`. Researchers consult the roster before recommending anything it
may govern. An unplanned change in a governed domain — capture, walk/review,
tasks/lanes/decay, gcal/meetings, refs, themes/relations, releases, or Mac Capture —
consults the roster first when no plan cites the decision. The web's roster loads on
demand: the descriptor preamble stays always loaded, instruction bundles and the
committed AGENTS.md carry a generated pointer instead of the roster, and the on-disk
descriptor keeps the full roster for human readers.

**Why.** Every agent turn paid for the whole 77-record roster (about 25.6 KB, 69% of
the root bundle), while the bare `sase memory read decisions` read prints every strand
body and was already run by accident. Researchers are the heaviest readers of decision
records, and frozen per-invocation bundles make planner-scoped compilation impossible,
so the roster must be a standing planning-time read rather than ambient context.

Evidence: `research:202611/on_demand_web_rosters/on_demand_web_rosters.md`;
epic `sase-1v7` (`plan:202611/on_demand_web_rosters.md`).

Rejected alternatives:

- **Rely on compaction.** Compaction still runs every turn and only trims superseded
  summaries; the catalog grows by about 30 records a month.
- **A titles-only roster.** Titles alone do not carry the claim, cost, and reopen
  conditions a planner must cite.
- **A flat reference note.** A reference note is read on demand with no catalog,
  trigger list, or audit trail; the roster read is audited and batches with strand
  reads.
- **Semantic search over records.** Search answers a question but cannot certify that
  a planner saw every governing record.

**Cost.** Ambient discovery is lost for unplanned work: an agent that never plans and
never touches a governed domain never sees a decision summary. Compliance is social
in v1 — nothing enforces the roster read before `sase plan propose`. The `roster`
slug is reserved and cannot name a strand.

**Reopens when.** Unplanned changes measurably contradict records, or measured planner
compliance stays poor; then add a `sase plan validate` roster check or a standing
titles-only rung.

Links [[decisions/bob-cli-releases-ride-a-daily-train]].

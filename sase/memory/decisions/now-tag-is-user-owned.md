---
keyword: "#now Is A User-Owned Weekly Bet, Never A Status"
aliases:
  - "#now"
  - now tag
  - this week's bet
  - now vs in progress
  - roadmap field
summary:
  "Only Bryan's explicit gestures add or remove #now; no automation infers, adds, or
  strips it. It never changes task status or feeds Next, and it stays a tag, never an
  inline field."
metadata:
  status: superseded
  decided: 2026-09-29
  superseded_by:
    - decisions/task-lanes-are-sticky
    - decisions/today-is-read-from-the-ledger
---

**Applies to.** bob-cli, bob-plugins, Bob Mac Capture, vault.

**Claim.** `#now` is a plain Obsidian tag on a `#task` line that marks one of this
week's bets. It is the Now tier of the roadmap: Now is `#now`, Next is Ready work, and
Later is a P1–P4 deferral (`p:<N>` in capture, Ctrl+Shift+P in Obsidian). Only an
explicit gesture writes it: typing it, a trailing `#now` on new capture text, or Bob
Navigation Hotkeys' Alt+N toggle and Ctrl+Shift+P `#now` row. Nothing adds it implicitly
— removing a Task Link with Ctrl+Shift+Enter or dropping one with `=x~<K>` leaves the
tag as it was — and no hook, cron job, or sync strips it. It is independent of the
checkbox: `bob task-status-hooks` only counts it, it never makes a task Next, and a task
can be both `#now` and `[/]`. The NOW cap (`plan.max_now`, default 15) is visibility,
not enforcement: over the cap, chips and `bob plan` turn red and `now_cap_exceeded` is
reported, but nothing refuses. `docs/plan.md` owns the NOW query and its Rust and
JavaScript implementations; `docs/capture.md` owns the grammar.

**Why.** The tag answers the status-lock trap measured on 2026-09-29: removing a task's
Task Link from today's [[glossary:pomodoro]] ledger demoted it, and nothing else
remembered that it mattered this week, so everything stayed linked — about 50 `[/]` and
25 `[*]` tasks, and 80 queued links, 58% of them a week or more without a 🍅. A tag only
Bryan owns separates commitment from activity, so the derived statuses in
[[decisions/task-status-is-derived]] can decay honestly: `[/]` is a footprint, `#now` a
promise. Rejected alternatives:

- **An inline `[roadmap:: now]` or `[horizon:: …]` field.** Bob's property writer
  appends fields at the far right, and Tasks parses trailing Dataview fields right to
  left; a scratch-vault test showed such a field silently erasing `priority` and
  `created`. A tag is safe anywhere on the line, as `#hide` already is.
- **`#now` as a Next source, or tools that backfill it.** Keeping `#now` tasks Next
  re-inflates NEXT; hooks or cron that sweep leftovers into a horizon edit history
  behind open editors and multi-machine sync; and silent diversion hides where a task
  went.
- **A hand-kept `roadmap.md`, explicit Next/Later values, or a `roadmap.base`.**
  Hand-kept horizon lists had already died twice in the vault (the zorg-era
  `now_*`/`soon_*`/`maybe_*` notes and `sase_blog_blockers.md`); P-levels already are
  the Later horizon and resurface on their own; and Bases rows are files, not tasks.
- **New capture tokens** (`h:`, `r:`, `~id`, `!id`). `#now` in body text already parsed.

Evidence:
`research:202609/pomodoro_closed_day_now_tag_automation/pomodoro_closed_day_now_tag_automation.md`
§§1, 2, 4, 6.4; `research:202609/now_tag_vs_in_progress_status.md`; epic `bob-cli-2o`
(`plan:202609/pomodoro_plan_budget_now_tag.md`); bob-cli `d28f8cd`; bob-plugins
`b68618f`.

**Cost.** A second marker with a manual lifecycle: only the Monday review prunes NOW,
and if that lapses NOW becomes the next pile with nothing but a red chip to say so.
Dropping a link never tags the task, so Bryan must tag before dropping or lose it from
view. The tag survives deferral — a P-level makes the task Blocked and hides it from
NOW, and it reappears when the date arrives unless untagged. Capture can tag only new
task text; tagging an existing task needs Obsidian. The NOW predicate is implemented in
Rust and mirrored in JavaScript, and both must keep matching the dash's query defaults.

**Reopens when.** The two-week trial (2026-09-30 → 2026-10-13) or a later weekly review
shows NOW ignored — the research's rule is then to delete the tag and keep only the
capped ledger; promotion from Ready proves too slow and a second horizon tag such as
`#next` is needed; or Obsidian Tasks or Bases gains task-level properties that are as
parser-safe as a tag.

Superseded: `#now` is retired — see [[decisions/task-lanes-are-sticky]] and
[[decisions/today-is-read-from-the-ledger]]. The Next lane now holds this week's
commitments.

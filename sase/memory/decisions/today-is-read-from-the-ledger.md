---
keyword: Today Is Read From The Ledger, Never Written To Tasks
aliases:
  - ledger today
  - today definition
  - read-time today
  - today vs now
summary:
  "Today is the open tasks with a dedicated Task Link under today's open
  Pomodoros, computed at read time by bob plan and bob-ledger-tools; never a
  tag, task-line field, or file-path filter; #now is retired."
metadata:
  status: accepted
  decided: 2026-09-30
---

**Applies to.** bob-cli, bob-plugins, vault, dash.

**Claim.** Today means dedicated Task Links under today's **open** Pomodoros
only; closed 🍅 lines are history. The definition lives in `docs/plan.md`,
with Rust `today_tasks` and ledger-tools `isToday` sharing conformance
vectors. The dash sections are exclusive (TODAY / PENDING / NEXT / READY).
`#now` is removed everywhere, and "committed but not today" is the Next lane.

**Why.** A file-path filter selects tasks that live in the daily note (1
result against 7 live links), and a hooks-managed `#today` tag is a second
copy of the ledger rewritten on task lines every 15 minutes, with churn,
races, and staleness. `filter by function` can call the plugin api, and the
Tasks reload event exists. Rejected alternatives:

- **A path filter.** It measures residence, not work.
- **A `#today` tag.** A stopgap only, with write churn on every pass.
- **Keeping `#now` through its trial.** The trial already failed; the tag is
  retired now.
- **One `bob-dashboard` block.** The fallback if the Tasks refresh proves
  fragile.

Evidence:
`research:202609/retire_now_sticky_lanes_ledger_today/retire_now_sticky_lanes_ledger_today.md`;
epic `bob-cli-2y` (`plan:202609/retire_now_sticky_lanes.md`); phase
`bob-cli-2y.2` (superseding record).

**Cost.** It relies on an internal Tasks event. Headless `bob query` sees an
empty Today. There are two implementations (Rust and JavaScript) to keep in
sync. Resolver divergence on ambiguous basenames is documented. Capture can
no longer mark "this week" on new text.

**Reopens when.** Tasks removes the event or `app` access, or the refresh
proves unreliable.

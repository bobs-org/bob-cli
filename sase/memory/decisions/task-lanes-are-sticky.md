---
keyword: Next And Pending Are Sticky Lanes; Only Blocked Is Derived
aliases:
  - sticky lanes
  - sticky next
  - sticky pending
  - lane release
summary:
  "Linking raises Ready to Next and an =x close sets In Progress (PENDING); no
  unlink, hooks run, or capture drop lowers them; only Alt+N release returns a
  task to Ready; Blocked stays derived."
metadata:
  status: accepted
  decided: 2026-09-30
---

**Applies to.** bob-cli, bob-plugins, Bob Mac Capture, vault.

**Claim.** Lanes are sticky:

1. Adding a Task Link under today's open Pomodoros raises Ready `[ ]` or
   Blocked `[?]` to Next `[*]`; Next and Pending are unchanged.
2. An `=x` in-progress outcome sets `[/]` (unchanged).
3. Removing a link **never** changes the lane: the hooks, Ctrl+Shift+Enter,
   `@route+id!`, `=x~K`, `=~K`, and hand deletion.
4. Only **release** (Alt+N, or the Ctrl+Shift+P lane row) lowers Next or
   Pending to Ready. Releasing also removes the task's live links from today's
   open Pomodoros; releasing a Pending task offers the optional Work Log
   prompt.
5. Alt+N on a Ready task **commits** it to Next without linking it.
6. Blocked stays derived, overrides every lane, and recovers to the existing
   derived rank (Ready unless linked today or reached by recent activity).
7. **Carve-out (R9):** tasks that live in canonical daily notes (such as
   `^gtd`) keep today's derived clearing of `[*]`, including the
   recent-activity `KeptNext` grace.
8. **R12:** the `[/]` symbol, its "In Progress" name and IN_PROGRESS type
   stay. Only dash and chip labels say PENDING. Picker group names
   (`in_progress`) stay.

**Why.** Sticky lanes protect dropped work by default, where `#now` did only
if tagged before dropping. The one bulk `#now` tagging covered exactly the 73
`[/]` + `[*]` tasks and was stripped the same day. Bryan cancelled a separate
Submitted status ("WIP status should fill this role"). Swarm work needs a
quiet place to wait. Pending → Next → Ready is Kanban's pull order and matches
capture's `queued > in_progress > next`. Rejected alternatives:

- **Keep `#now` with a derived `[*]`.** It repeats Today.
- **A Submitted/Waiting status.** Bryan cancelled it; WIP fills the role.
- **No promotion on link.** That is the status-lock trap.
- **Age-based Next decay.** Not adopted, and never for Pending.
- **A hidden pre-block status.** No hidden previous-status field is stored.

Evidence:
`research:202609/retire_now_sticky_lanes_ledger_today/retire_now_sticky_lanes_ledger_today.md`;
epic `bob-cli-2y` (`plan:202609/retire_now_sticky_lanes.md`); phase
`bob-cli-2y.2` (hooks-sticky implementation and verification).

**Cost.** No automatic forgetting: about 2–3 tasks a day of lane growth at
September's rates, so caps, chips, lints, a daily review with release and a
weekly prune are required. `[/]` no longer means "worked recently". Lanes
don't survive Blocked. Dependency-promoted Next is sticky too. About 80 legacy
statuses need one-time triage.

**Reopens when.** Lanes must survive Blocked (then Blocked becomes an overlay).

Amended in place 2026-10-04 at Bryan's request: the freshness trial was removed; nothing waits on it.

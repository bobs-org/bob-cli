---
keyword: Capture Links A Dependency-Blocked Task By Dropping Its Prerequisites
aliases:
  - capture dependency drop
  - drop blocking deps
  - capture link drop
summary:
  "Typing an existing-task capture link on a task Blocked only by its own
  open prerequisites drops those links and then performs the gesture;
  plugins keep today's consumption."
metadata:
  status: accepted
  decided: 2026-10-13
---

**Applies to.** bob-cli, Bob Mac Capture.

**Claim.** When a capture existing-task link gesture (`@route:id` /
`^route:id`, `@route+id!`, Ensure Next `@route+id[#name]`, or
`@route:id=x…`) targets a task that is Blocked only by its own open
prerequisites (optionally plus its own single future `scheduled`),
capture removes those open prerequisite links from the managed
Depends-On line and `[dependsOn::]` field, then performs the gesture.
The drop runs only when every gate holds: a `local_dependency` reason;
no project or inherited project reason; a target other than a
recognised project's `^prj`; at most the single-field future schedule
that pull-forward already retires; exactly one accepted managed
Depends-On child; an unchanged task line between assessment and write;
and the drop staged in the same batch as the rest of the gesture. Bob
Mac Capture only renders bob's `dropped_dependencies` and `link_guard`
fields. Plugin gestures (Ctrl+Shift+Enter, `^^`) keep PDG1
consumption. `docs/task-dependencies.md` §13.5.1 owns the contract.
See also [[decisions:projects-block-from-prj-line]] and
[[decisions:task-deps-are-depends-on-links]].

**Why.** Bryan asked on 2026-10-13 that when an `@file:id` link
gesture in `bob capture` or Bob Mac Capture targets a Blocked task
with open dependencies, capture "remove the dependency and complete
the operation when it is safe to do so. I think we always fail to do
anything currently." Consumption left the target unlinked in both
clients and read as a failure. Rejected alternatives:

- **Keep consumption.** The gesture still looks like it did nothing.
- **Drop for every reason, including project and `^prj` reasons.**
  Project prerequisites belong to the Task Card's subtree pass; dropping
  a resident's own links would not unblock it.
- **An opt-in sigil.** The live preview already shows the drop before
  submit.
- **Strike dropped links as history.** The writer form forbids strikes;
  vault git keeps the history.
- **Plugin parity in the same change.** Ctrl+Shift+Enter and `^^` stay
  on PDG1.

Evidence: `plan:202610/capture_link_drops_blocking_deps.md`; bob-cli
`92515f1` (cli-core), `bdfc8c3` (cli-gestures); bob-mac-capture
`8f89423` (mac).

**Cost.** Dropped edges leave no trace on the task beyond capture
output and vault git. Capture and plugins diverge. Project-blocked and
`^prj` targets still refuse.

**Reopens when.** Bryan wants plugin gestures to drop too; he wants a
`^prj` drop through the subtree pass; or dropped edges need a record
on the task.

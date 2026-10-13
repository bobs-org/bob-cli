---
keyword: Projects Block Their Tasks From The ^prj Line
aliases:
  - project dependencies
  - prj_is_blocked
  - blocked link consumption
summary:
  "A project's prerequisites and date live on its ^prj task; while that
  line is Blocked, every open resident is Blocked and tagged
  #prj_is_blocked, and Blocked tasks keep no Task Links under today's
  open Pomodoros."
metadata:
  status: superseded-in-part
  superseded_by: decisions:subprojects-inherit-project-blocking
  decided: 2026-10-12
---

**Applies to.** bob-cli, bob-plugins, vault.

**Claim.** A project's prerequisites live on its `^prj` Depends-On line
and its date as inline `[scheduled::]`, with no project frontmatter
`depends_on` or `scheduled`. While `^prj` has its own reason to be
Blocked — an open projected prerequisite, or a future inline date —
every open task in its note is Blocked `[?]` and carries the
machine-owned `#prj_is_blocked` tag. `^prj` itself is never tagged; a
hand-set `[?]` on `^prj` blocks nobody. There is no sub-project
cascade: blocking is by residence only. Blocked tasks keep no Task
Links under today's open Pomodoros: a gesture that would link one
raises its open prerequisites instead (or reports a project date), and
`bob task reconcile` removes stale live links and emptied Pomodoros.
`docs/task-dependencies.md` §13 owns the contract; see also
[[decisions/task-deps-are-depends-on-links]],
[[decisions/task-status-is-derived]], and
[[decisions/task-lanes-are-sticky]].

**Why.** Frontmatter is not needed: no vault query reads project
`scheduled` or `depends_on`, and the `^prj` line already drives
reconcile, R1–R10, successor recovery, the Depends on stage, and
ledger chips. The project design already treats the task line as the
interaction point. Inheritance without consumption would leave Blocked
work planned today. A tag on every open resident states a fact about
the project, so writers do not recompute it when a resident's own
reasons change. Rejected alternatives:

- **Frontmatter `depends_on` / `scheduled` (canceled bob-cli-6s).** YAML
  storage was the odd one out and raced `bob-project-tasks`.
- **Inheritance without consumption.** Blocked tasks would keep live
  Task Links.
- **Copying project dependencies onto residents.** Duplicate edges and
  a second writer surface.
- **Residents depending on their own `^prj`.** A guaranteed deadlock.
- **`status: waiting` as a reason.** Status stays derived from the
  line.
- **A Blocked overlay.** Lanes do not survive Blocked.
- **A sub-project cascade.** Blocking is by residence only.
- **A render-only reason chip instead of a tag.** `blocked.md` and JS
  paths need a stored token.
- **Tag only sole-reason residents.** Every immediate writer would
  recompute the tag; `every_resident` states a project fact.

Evidence:
`research:202610/prj_task_inline_schedule_and_deps.md`;
`research:202610/project_dependency_blocking/project_dependency_blocking.md`;
epic `bob-cli-6u` (`plan:202610/prj_line_project_blocking.md`);
bob-cli `37c100f` (contract), `f6fbd7d` (cli-model), `23f2fd9` (hoist),
`34d4ab6` (reconcile block/tag), `2340689` (consume links),
`13d183f` (close), `66c4ddb` / `36f997b` (capture);
bob-plugins `c9938d8` (nav-model), `a42f45d` (schedule),
`046028c` (Depends on), `1abde2f` (convert/move), `744247c`
(cycler/block-id-prompt).

**Cost.** Lane loss is project-wide. The `^prj` line is longer. Tasks
and Dataview `tags include #prj` is a substring match for
`#prj_is_blocked`. A missed immediate tag write is repaired only on
the next reconcile. Project dates still propagate onto residents
(raise-only; a hand-edited earlier `^prj` date keeps old copies).
Consumption may delete a running timed Pomodoro. Daily-note tasks keep
the R9 carve-out.

**Reopens when.** Lanes must survive Blocked; Bryan wants a sub-project
cascade or re-linking on unblock; a projects-by-date Properties/Bases
view is needed.

Superseded in part: residence-only blocking remains for each project’s own residents; sub-project descendants now inherit blocked ancestor reasons — see [[decisions:subprojects-inherit-project-blocking]].

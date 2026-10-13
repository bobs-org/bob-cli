---
keyword: Task Dependency Link
aliases:
  - "task dep link"
  - "dep link"
---

A plain (never transcluded) [[task-link]] to a prerequisite of another Obsidian
task. A task's dep links all sit on its Depends-On line, its first direct child:
`⛓️ **DEPENDS ON:** [[#^a]] • [[note#^b]]`. That line is the source of truth: Bob
derives the task's `[dependsOn::]` field and each target's `[id::]` from it, and
the task stays Blocked `[?]` while any target is open.
Each open prerequisite carries a generated `🧱 **BLOCKS:**` line listing its open
dependents, which Bob maintains from Depends-On lines and never reads back, so
dependencies are edited only on the dependent. A [[project-task]]'s dep
links sit on its own Depends-On line like any task's; while they are open, or
its inline `scheduled` is future, every open task in its note is Blocked. A
Blocked task keeps no Task Links under today's open Pomodoros: linking one
raises its open prerequisites instead, and `bob task reconcile` removes such
links. Add or remove dep links with Ctrl+Shift+P → Depends on, which
fuzzy-searches open tasks across the vault, or by editing the line. A dep link
is a prerequisite, not a sub-task: closing the dependent never closes its
targets, and dep links never count as Pomodoro Task Links. See
[[decisions/task-deps-are-depends-on-links]] and
[[decisions/projects-block-from-prj-line]]; syntax details live in
`docs/task-dependencies.md`.

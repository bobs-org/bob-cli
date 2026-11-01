---
keyword: Leaf Project
title: Leaf Project
summary: A project note that is not its own root parent — a sub-project whose
  time reports under a root project above it.
---

# Leaf Project

*Requested · project*

A [[project-note]] that is not its own [[root-parent]], meaning a sub-project
at any depth whose `parent` walk meets another project before any
[[area-note]] and that does not set `root: true` itself. A
[[top-level-project]], a `root: true` project, and an orphan project (nothing
above it) are roots, never leaves. "Leaf" names the reporting role, not tree
position: a leaf project may have sub-projects. Leafness is derived from the
note tree at gesture time, never stored, and a task's `[root::]` field never
changes it. Closing the task behind a leaf project's last live
[[task-link]] in today's Pomodoros offers the project's ready tasks as
keep-moving [[successor-offer]]s, which are never dependents (see
`docs/task-dependencies.md` §12.15).

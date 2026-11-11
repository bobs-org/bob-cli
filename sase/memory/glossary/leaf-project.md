---
keyword: Leaf Project
title: Leaf Project
summary: A recognised project note that is not an umbrella — a root whose
  ^prj lists sub-projects.
---

# Leaf Project

*Requested · project*

A recognised [[project-note]] that is not an umbrella. An umbrella is a root
project (its own [[root-parent]]) whose open [[project-task]] (`^prj`) carries
a `🧩 **Sub-projects:**` marker line listing at least one wikilink child, in
any state. A standalone [[top-level-project]], a `root: true` project with no
sub-projects, and an orphan project are leaves. A root that has split out
children stays an umbrella even when every child is closed. Leafness is
derived at gesture time from the note's held text, never stored, and a task's
`[root::]` field never changes it. Closing the task behind a leaf project's
last live [[task-link]] in today's Pomodoros offers the project's ready tasks
as keep-moving [[successor-offer]]s, which are never dependents (see
`docs/task-dependencies.md` §12.15).

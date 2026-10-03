---
keyword: Project Task
aliases:
  - "prj task"
---

The single project-level `#task` checkbox in a [[project-note]], identified by the exact
trailing block ID `^prj`. Its description states what must be true for the project to be
complete. `bob projects sync` maps checked to `status: done`, canceled to
`status: canceled`, and reopening a terminal project to `status: wip`; an open
nonterminal project's status is preserved. New lines include `#prj`; legacy lines
without that tag remain recognized. See `docs/projects.md`.

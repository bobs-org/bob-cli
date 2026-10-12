---
keyword: Project Task
aliases:
  - "prj task"
---

The single project-level `#task` checkbox in a [[project-note]], identified by the exact
trailing block ID `^prj`. Its description states what must be true for the project to be
complete. The `^prj` line carries the project's date (inline `scheduled`) and dep links
(its Depends-On line); while either blocks it, every open task in the note is Blocked
and tagged `#prj_is_blocked`. `bob projects sync` maps checked to `status: done`,
canceled to `status: canceled`, and reopening a terminal project to `status: wip`; an
open nonterminal project's status is preserved. New lines include `#prj`; legacy lines
without that tag remain recognized. See `docs/projects.md` and
[[decisions/projects-block-from-prj-line]].

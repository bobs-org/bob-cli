---
keyword: Project Note
aliases:
  - "prj note"
---

A Bob vault Markdown file with frontmatter `type: "[[project]]"` that holds a project's
context and work. Its [[project-task]] (`^prj`) states the completion criteria and
represents the project's overall status; ordinary tasks describe work toward that
outcome. A closed project remains a project note if its `^prj` task is archived. An
`offclock: true|false` key sets the clock default for tasks on the root walk
beneath it (see [[decisions:off-clock-follows-tag-then-note-default]]). See
`docs/projects.md`.

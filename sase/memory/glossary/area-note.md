---
keyword: Area Note
---

A Bob vault Markdown file with frontmatter `type: "[[area]]"` for an ongoing sphere of
responsibility with no end state, such as `cash`, `job`, or `dev`. Unlike a
[[project-note]], it has no `^prj` task and no done/canceled lifecycle, so Bob always
treats it as open. Its `parent` names the area it sits under (`"[[area]]"` for a
top-level area); sub-areas and projects point `parent` at the area (or, for a
sub-project, the project) they belong to. Every `#task` belongs in exactly one area or
project note by residence, the file holding its checkbox line; a [[task-link]] or embed
elsewhere never moves it. Area notes hold the ongoing tasks, usually under `## Tasks`,
that belong to no finite project; the inboxes (`inbox`, `mac_inbox`, `gkeep_inbox`) are
area notes whose tasks await triage. The only exceptions are a reference note's own
[[reference-task]], each daily note's templated `^gtd` task, and closed tasks that
`bob task archive` moved into `done/`; move any other stray task with Ctrl+Shift+M.
Root-level area notes are `@route` capture targets, and every area note gets the
per-note Ready cap and `## Tasks` status grouping.

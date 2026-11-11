---
keyword: Task Drawer
aliases:
  - drawer
---

A direct non-checkbox ALL-CAPS child bullet of a `#task` line, with its notes nested
beneath it. A drawer holds part of one task's work (`REQUIREMENTS`, `FUTURE WORK`);
its entries are plain bullets with no checkbox, `#task`, or `created` field. A drawer
is nonempty by construction: capture never creates an empty drawer, and promotion of a
task to a project note turns each drawer into an H2 section while reversal restores
them.

In capture, a `#` glued after `@route+id` always names a drawer (`@foo+bar#requirements`),
matched exactly against the target task at commit time and created on demand with the
catalog's most-used spelling. A quoted literal (`#"FUTURE WORK"`) or `--drawer TITLE`
forces creation and skips the near-duplicate guard; an unquoted slug that is a strict
prefix of, or edit distance 1 from, an existing drawer is refused. On a project's
`^prj` task, the note's eligible `##` sections are its drawers. `capture-complete`
lists the target's drawers in document order, then catalog recents, then a creation row,
with reserved, ambiguous, and near-duplicate choices refused and quorum copy authored by
bob. `bob capture-drawers` lists one task's drawers plus catalog recents.

In a capture draft, a `#` right after a first-level bullet's `- ` completes to the
drawer's title (`- #future-work` becomes `- FUTURE WORK`), and an ALL-CAPS first-level
bullet with nested notes is a drawer bullet, merging into an existing drawer under a
note-less `@route+id`. On a direct notebook-body route, `- #Section` instead selects
a notebook section for its nested notes; it is not a drawer. See
[[decisions:capture-bullets-mirror-the-task-block]].

Managed lines and logs are never drawers: Depends-On, BLOCKS, [[task-relation]] LINKS
lines, and the Schedule, Work, Rotten, and Cancel logs, plus `TASKS`. A drawer is not a
Markdown `##` heading, a [[project-note]] H2 section, an [[annotation-section]], a
[[pomodoro]] name, or a [[task-type]]: `@note#Heading` routes to a note heading,
`=name` names a Pomodoro, and a free-standing `#fix` at an item edge, other than a
first-level bullet start, types the task.
See `docs/capture.md`.

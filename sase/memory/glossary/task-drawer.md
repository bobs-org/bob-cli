---
keyword: Task Drawer
aliases:
  - drawer
---

A direct non-checkbox child bullet of a `#task` line whose whole title is bold, such as
`- **Future Work**`, with its notes nested beneath it. The title stays exactly as
authored; identity is case-folded. A drawer holds part of one task's work;
its entries are plain bullets with no checkbox, `#task`, or `created` field. A drawer
is nonempty by construction: capture never creates an empty drawer, and promotion of a
task to a project note turns each drawer into an H2 section while reversal restores
them.

A task that owns [[task-requirement]]s keeps its notes in a `- **Notes**` drawer (key
`NOTES`). That drawer is born only when a note lands in it
(`notes_drawer_birth = with_first_note`): the first-req scoop moved at least one note,
or a note is being written. A req-owning task with no notes has no Notes header. Drawers sit after requirements and before
logs. A checkbox inside a drawer is never a requirement. The `REQUIREMENTS` drawer
was renamed to `NOTES` vault-wide.

In capture, a `#` glued after `@route+id` names a drawer (`@foo+bar#notes`),
matched by the target task's case-folded key at commit time and created on demand with
the catalog's preferred spelling. A quoted literal (`#"Future Work"`) or `--drawer TITLE`
forces creation and skips the near-duplicate guard; an unquoted slug that is a strict
prefix of, or edit distance 1 from, an existing drawer is refused. On a project's
`^prj` task, the note's eligible `##` sections are its drawers. `capture-complete`
lists the target's drawers in document order, then catalog recents, then a creation row,
with reserved, ambiguous, and near-duplicate choices refused and quorum copy authored by
bob. `bob capture-drawers` lists one task's drawers plus catalog recents.

In a capture draft, a `#` right after a first-level bullet's `- ` completes to the
drawer title (`- #future-work` becomes `- **Future Work**`), and a first-level bold-title
bullet with nested notes is a drawer bullet, merging into an existing drawer under a
note-less `@route+id`. On a direct notebook-body route, `- #Section` instead selects
a notebook section for its nested notes; it is not a drawer. See
[[decisions:capture-bullets-mirror-the-task-block]] and
[[decisions:drawers-keep-their-names]]. ALL-CAPS alone no longer marks a drawer.
Obsidian and Bob Mac Capture draw a drawer header as an outline tray plus its verbatim
semibold title; plugins off, it reads as a bold header.

Managed lines and logs are never drawers: Depends-On, BLOCKS, [[task-relation]] LINKS
lines, and the Schedule, Work, Rotten, and Cancel logs, plus `TASKS`. A drawer is not a
Markdown `##` heading, a [[project-note]] H2 section, an [[annotation-section]], a
[[pomodoro]] name, or a [[task-type]]: `@note#Heading` routes to a note heading,
`=name` names a Pomodoro, and a free-standing `#fix` at an item edge, other than a
first-level bullet start, types the task.
See `docs/capture.md`.

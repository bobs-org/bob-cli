---
keyword: Project Note Capture
aliases:
  - "project capture"
  - "project-note capture"
---

A [[capture]] item whose route marker is sealed with a trailing `@`, `@<route>^<id>@` or
`@<route>:<id>@` (optionally followed by `#<pomodoro>`), so it creates a brand-new
[[project-note]] instead of a task. With `%n`, the `^` seal creates a [[notebook]]
instead (parent area, project, or notebook; the item text becomes its `summary`). The marker may lead the item or trail any of its
lines. bob writes `<route>_<id>.md` at the vault root (each `-` in the ID becomes `_`)
with `parent: "[[<route>]]"`, `type: "[[project]]"`, `status: wip`, and the
`template: "[[new_project]]"` line, in the same shape as the Obsidian **Create project
note from task** command, which instead promotes an existing task. The item's parent
text becomes the [[project-task]] line `- [ ] #task #prj <text> #hide ^prj` (`[?]`
once the item resolves a schedule), which is never linked. `<route>.md` must already
be an area or non-terminal project note, a target note that already exists or is staged
refuses the capture, and the same batch lists it in the parent's `## Projects`
catalog.

The sigil before the ID picks the form. The `^` (create) form links only the item's
explicit `:` bullets. The `:` (offer) form creates the same note, then offers its open,
unhidden, not-yet-linked `## Tasks` rows as [[successor-offer]]s of the new `^prj`,
linking none until picked with `-p` (Bob Mac Capture's Pull-In card). A `#<pomodoro>`
after the seal names the Pomodoro those links land under, created on demand, and is
refused when the item has nothing to link or offer.

First-level bullets mirror the new note
([[decisions:capture-bullets-mirror-the-task-block]]). Each becomes a `## Tasks` row: a
trailing ` :<id>` names it, makes it Next, and adds its [[task-link]] under the
Pomodoro its type places (or the `#<pomodoro>` one), while ` ^<id>` only names it. An
ALL-CAPS bullet with nested notes becomes a `## Title Case` section instead. A bullet
that is only `^<id>` or `:<id>` moves an existing open top-level task out of the parent
note (`:` also links it), and `- ^^` chooses several from a Move card before writing.
The new note, the moved blocks, and today's ledger write in one all-or-nothing batch.

`%c`, `--clip`, `=<X>` and `=x` session suffixes, `!`, and `@@` declarations are
refused, and the retired `+` seals (`@route^id+`, `@route:id+`) teach the `@` spelling.
Not `@route^id` (an ordinary task with a block ID), `@route:id` (a task linked into a
Pomodoro), `@route+id` (a note under an existing task), or record-note capture
(`@dir/^stem`). `capture-parse` reports mode `project_note`, or `pomodoro_project_note`
once the item links a task. See `docs/capture.md` ("Project notes") and
`docs/reference/capture-syntax.md` ("Project captures").

---
keyword: Capture Bullets Mirror The Task Block
aliases:
  - capture bullets mirror the task block
  - bullet drawers
  - first-level bullet drawer
  - multi-note capture
summary:
  "A first-level capture bullet is its task's child: ALL-CAPS with notes is a drawer, a
  leading # names one, and note-less @file+id files each bullet as a note."
metadata:
  status: accepted
  decided: 2026-11-02
---

**Applies to.** bob-cli and Bob Mac Capture.

**Claim.** A first-level capture bullet that lands directly under a task is that task's
child, so the draft reads exactly like the block it writes. That position is a new task,
a new project note, or a note-less `@route+id` (also `@route+id#drawer`, where every
bullet files inside that drawer). There an ALL-CAPS title with nested notes is a
[[glossary:task-drawer]]; a `#` that begins the bullet (`- #future-work`, `- #"TITLE"`)
is only a typing shortcut that completes to that title and never types the task (a type
marker keeps working at a line's trailing edge, a nested bullet's start, and the parent
line); and every other first-level bullet, with its nested bullets, is one note. A
note-less `@route+id` followed by bullets files each as its own note, in draft order,
with drawer bullets merging into the target's drawers. The bullet drawer menu inserts
the bare ALL-CAPS title (plus the nested `  - ` bullet when none follows), and a row is
valid only when committing that literal title reaches the row's intent. One resolver
serves the menu, `capture-parse`, the dry-run preview, and the commit.

**Why.** The draft reads like the note it writes, so one resolver serves every surface
and they cannot disagree. The shape also fills the refused gap where a note-less
`@file+id` with bullets had no meaning. Rejected alternatives were keeping bullet-start
types, a `- #drawer text` shorthand (it cannot survive the bare-title rewrite),
repeating `@file+id` per note, treating ALL-CAPS bullets as plain notes under existing
tasks (duplicate headers), and spreading `s:`/`$`/`%c` over every note.

**Cost.** A grammar 4 break: a first-level `- #fix …` no longer types the item. Multi-note
items refuse the single-note markers (`multi_note_marker`), and drawer notes stay one line
each under the two-level authored grammar.

**Reopens when.** Authored bullets gain a third level, or Bryan wants bullet-start types
back.

**Evidence.** `plan:202611/capture_drawer_bullets.md`; bob-cli commits `bbd26d97`
(first-level `#` is a drawer shortcut, grammar 4), `5ac2fbc2` (several notes on one
existing task), and the bullet-menu phase `bob-cli-fc.3`. See also
[[decisions:capture-sigils-say-where-what-how]] and [[glossary:task-drawer]].

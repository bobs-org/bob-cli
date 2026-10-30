---
title: Task Type
aliases:
  - type
  - theme
  - pomodoro theme
  - pomodoro-theme
---

A task's type is its kind of work: one ALL-CAPS name. Bob computes an open task's
effective type in this order: its explicit `[type:: NAME]` (or legacy `[theme:: NAME]`)
field; `plan.reading_type` for a reading task; the nearest area/project `task_type:`
default (or legacy `theme:`); then `plan.default_type`. A task field stores only an
exception to inheritance: choosing the inherited type clears it. Closed tasks keep
their fields as history.

A Pomodoro name can contain several `+`-separated components. With a configured
vocabulary, matching components are types and the others are labels; without one, each
component is a type unless it is an inventory label. A task's effective type and its
live Pomodoro Task Link agree: retyping updates the task field and moves or renames its
link as needed, while naming a linked entry retypes its open linked tasks. Disagreement
is shown as drift; it is not silently repaired. PLAN is an ordinary type, while
`plan.exempt` only controls budget counting and mute color.

`bob types` manages the catalog and preferences; `bob themes` remains a CLI alias.
Configured `types:` colors take precedence, followed by journal pins, exempt and
unconfigured mute colors, the top-level area's color, and a stable name hash. Hiding a
type removes it from Recent suggestions only. See `docs/task-types.md`.

This is not a SASE bead `task_type`, a note's frontmatter `type:`, a Pomodoro home, or
an Obsidian appearance theme.

---
type: short
parent: AGENTS.md
description: Project-local glossary generated from sase.yml.
sase_generated: glossary
---

# Glossary of Terms

## Pomodoro

A checkbox item in the "Pomodoros" section of my Obsidian daily file that represents a
particular session (or planned session) of work. Past pomodoros have a timespan
associated with them and are closed (i.e. checked). Current pomodoros have a timespan
but are open (i.e. unchecked). Future pomodoros have an empty `()` instead of a timespan
and are also open (i.e. unchecked).

## Schedule Log

When the `<ctrl+shift+p>` Obsidian keymap is used to add the `scheduled` or `priority`
dataview property to a task (or when the `bob capture` command's input argument contains
the special `p:<N>` syntax), we add a `SCHEDULE LOG` bullet that contains one sub-bullet
that corresponds with each time the task was scheduled / re-scheduled.

## Task Link

ALIASES: task block link

A block link to an Obsidian task. These can be located anywhere but, when they are the
only contents of a sub-bullet on a pomodoro, they are treated as logged/planned tasks
for that pomodoro. As another special case, transcluded task links that are the only
contents of a sub-bullet on another Obsidian task are treated as sub-tasks (i.e.
dependencies) of that task.

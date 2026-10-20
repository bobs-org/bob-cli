---
title: "Link related tasks"
description: "Add a directed, optionally labelled relation between tasks without changing their work state."
tags:
  - CLI
  - Obsidian
  - macOS
---

# Link related tasks

Use a task relation to record why two tasks are connected. A relation is
authored from one task to another and stays separate from prerequisites and
Pomodoro work links.

## Choose a direction

- `~note:block-id` makes the captured task link to the named task.
- `~~note:block-id` makes the named task link to the captured task.

For example, capture a follow-up to a task in another note:

```bash
bob capture 'Investigate retry handling ~service:retry-loop|"follow-up of"'
```

Or create a task that an existing task should link to:

```bash
bob capture 'Write the migration note ~~service:retry-loop|"migration note for"'
```

Use `@route+block-id` to add a relation to an existing owner task without
creating a new task:

```bash
bob capture '@bob+retry-investigation ~service:retry-loop|"related to"'
```

Quote the draft in a shell so it passes `~` and `~~` to Bob literally. The
related task needs a block id. For capture syntax and locator rules, see
[Linking tasks with `~` and `~~`](../capture.md#linking-tasks).

## Add a relation in Obsidian or Mac Capture

In the Obsidian Task Card, use **Links to** (`l`) to choose a task the current
task relates to, or **Linked from** (`⇧L`) to choose a task that should relate
to the current one. The linked-from action writes the chosen source task's
`LINKS` line; Bob maintains the target's `LINKED FROM` mirror.

Mac Capture accepts the same `~` and `~~` draft syntax. Choose the task, press
`⇧↩` to open the relation card, then review the label and relation in the
preview before submitting. See [Capture a task from anywhere on your
Mac](capture-from-your-mac.md) for the panel workflow.

## Add a label

Labels are optional and always describe the relation from the source task's
side. Use `|label` for a one-word label and `|"follow-up of"` for a label with
spaces. For a known label, the incoming view uses Bob's built-in inverse
wording; the stored label stays as authored. See the [relation label
rules](../task-relations.md#32-labels).

## What a relation changes

A relation adds an entry to the source's `🪢 **LINKS:**` line. Bob generates
the matching `🪢 **LINKED FROM:**` entry on a writable target, and the live
relation chips show both sides. Reconcile repairs the stored mirror if a
write was skipped or the line was edited.

Relations never block a task, count as Pomodoro Task Links, change status or
schedule, or change priority or freshness. Use [Add prerequisites while
capturing](add-prerequisites.md) when one task must finish before another.

## Related

- [Task relations](../task-relations.md)
- [Capture syntax](../reference/capture-syntax.md#tilde-link)
- [Projects and dependencies](../concepts/projects-and-dependencies.md)
- [Capture a task from anywhere on your Mac](capture-from-your-mac.md)

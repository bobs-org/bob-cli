---
title: "Projects and dependencies"
description: "How project notes, parent projects, and prerequisite links combine to block work."
search:
  boost: 2
tags:
  - CLI
  - Obsidian
---

# Projects and dependencies

This concept explains the difference between living in a project, descending from one, and waiting on one.

Residence is where a task lives: the project note whose file holds the line. A parent project is the note's declared ancestor through a scalar frontmatter `parent:` wikilink, as in the demo vault where `Projects/seedlings.md` descends from `[[garden]]`. A prerequisite is a task that must finish first, recorded as a plain task link on the dependent's managed Depends-On line. The three answers compose: a task can live in a child project, inherit blocking from a grandparent, and still wait on its own prerequisites.

A project's `^prj` task states the outcome, for example "Raise healthy seedlings for the garden." That line represents the project itself: its prerequisites and its inline schedule decide whether the whole project is blocked, and while that line is Blocked every open resident is Blocked too.

## One managed line

Each task holds at most one managed first-child Depends-On line:

```markdown
- [ ] #task Harden the cold frame before the frost ^harden-frame
  - ⛓️ **DEPENDS ON:** [[garden#^prep-beds]]
```

That line is the source of truth. Bob derives the task's `[dependsOn::]` field and each target's `[id::]` from it, and shows a generated `🧱 **BLOCKS:**` mirror on each open prerequisite as a reverse view rather than another source of dependency intent. Editing happens on the Depends-On line; the fields and the mirror follow.

## How blocking is derived

Blocking is recomputed from causes, never asserted by hand. Future scheduling and open prerequisites on `^prj` block the project, and inherited blocking flows down through child projects: a blocked ancestor blocks each descendant project's `^prj`, which in turn blocks that note's open residents. Because reasons live on lines and links, blocked work cannot remain planned under today's open Pomodoros; reconcile consumes those stale links and empties Pomodoros left childless.

A manually set Blocked checkbox alone is not a project blocking reason. Writers recompute the marker from the `^prj` causes, and a hand-toggled marker without a cause is repaired on the next pass. The same rule applies to derived fields and tags: they are projections to read, not inputs to edit. When a project recovers, residents recover to the lane their current causes justify rather than to a remembered earlier lane.

## Finishing a prerequisite

Managed completion can recover dependents automatically, but offers require explicit selection before work is linked into Today. When a prerequisite closes, Bob recovers each dependent — clearing derived Blocked markers, retiring project tags, and recording an unblock stamp where applicable — and then offers each would-be successor as an explicit choice. Only picked rows are linked under today, raised to Next, and stamped fresh; unpicked work follows its ordinary recovery and review rules without a link.

Descendant recovery can require a reconcile pass, because non-Task-Card writers respect the inherited tag and leave the tree repair to the next run. Do not expect every close to repair the entire project tree in one gesture. Direct capture on a task Blocked only by its own prerequisites is the documented exception to the blanket consumption rule: instead of refusing or merely raising prerequisites, that gesture drops the target's own open prerequisite links and then performs the requested link. Exact gesture semantics, project-date handling, and offer grouping belong in the dependency contract.

## Related

- [Your first Bob day](../start/first-bob-day.md) — finish work and choose what comes next.
- [Tasks and lanes](tasks-and-lanes.md) — what Blocked overrides and how recovery recomputes.
- [Today and the Pomodoro ledger](today-and-the-pomodoro-ledger.md) — where successor offers land.
- [Understand Bob](index.md) — the eight nouns and where projects fit.
- [Project Task Sync](../projects.md) — the authoritative `^prj` lifecycle and rollup.
- [Task dependency links](../task-dependencies.md) — Depends-On grammar, blocking, and successor offers.
- [Task Status Hooks](../task-status-hooks.md) — how reconcile derives and consumes Blocked.

---
keyword: Task Successor
title: Task Successor
aliases:
  - successor
summary: A task positioned to take another task's place in today's Pomodoro plan
  when that task is completed or promoted to a project.
---

# Task Successor

*Requested · project*

aka successor

A task positioned to take its predecessor's place in today's Pomodoro plan when the
predecessor leaves it. The successors of a managed Done gesture are the tasks it
unblocks: direct dependents, open residents of a project whose `^prj` it recovered,
and project-finished pull-ins. Promoting a task to a [[project-note]] makes the new
note's open tasks its successors, because the [[project-task]] never inherits the
predecessor's [[task-link]]. Successors are computed when the gesture runs and are
never stored. None is linked automatically: each becomes a [[successor-offer]], and
only a picked one is linked under its Pomodoro destination, raised to Next, and
stamped fresh.

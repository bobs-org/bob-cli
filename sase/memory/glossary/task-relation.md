---
aliases:
  - relation
  - LINKS line
  - LINKED FROM line
---
# Task Relation

*Requested · project*

A directed, optionally labelled link from one task to another, inert unless its label
carries an effect. It is authored on
the source task's 🪢 **LINKS:** line and mirrored on the target task's generated 🪢
**LINKED FROM:** line. The generated LINKED FROM line also lists enabled-by-default [[task-mention]]s
(`(💬 mentions)` entries), which are not relations. Add one with Capture `~` / `~~` or the Task Card `l` / `⇧L`
gestures. A relation is distinct from a Pomodoro [[task-link]]: it never blocks a task,
counts as a Pomodoro Task Link, or changes its schedule, priority, or freshness.
Committing a configured cancel, complete, or merge label through capture or the Links
stage cancels the source, completes it, or merges it into the target (moving its notes
and cancelling it), respectively, at commit time; see [[task-merge]].

A relation whose label is a configured successor label (such as `starts after`) also
offers its source as a [[successor-offer]] when its target is parked (left Pending with
no live Task Link today) or done; it still never blocks, and its mirror reads
`starts before`.

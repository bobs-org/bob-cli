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
A plain link cannot coexist with a [[task-dependency-link]] for the same task pair
in either direction; reconcile and merge absorb the redundant plain link.
Committing a configured cancel, complete, or merge label through capture or the Links
stage cancels the source, completes it, or merges it into the target by copying its notes
and cancelling it, respectively, at commit time; see [[task-merge]].

An unlabelled relation is a plain link, and a plain link and a [[task-dependency-link]] never join the same task pair
in either direction: adding a dependency absorbs the plain link, and only
an explicit Task Card stage gesture releases a dependency into a plain link.

A relation whose label is a configured successor label (such as `starts after`) also
offers its source as a [[successor-offer]] when its target is parked (left Pending with
no live Task Link today) or done; it still never blocks, and its mirror reads
`starts before`.

A plain (unlabelled) link and a dependency between the same two tasks never coexist in either direction; adding a dependency absorbs either plain link, and reconcile and merge remove overlaps.

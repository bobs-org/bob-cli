---
keyword: Notebook
aliases:
  - "notebook note"
---

A Bob vault Markdown file with frontmatter `type: "[[notebook]]"` that keeps a
named collection of related notes (observations, explanations, ideas, logs)
without the obligations of a new [[area-note]] or [[project-note]]. Its body is
note bullets, optionally grouped under `##` sections below an H1 title; each
bullet may carry nested detail, a `[created::]` stamp, and a stable `^id`. It
never holds a `#task` or any checkbox, so it is never a task's residence, a
Pomodoro home, a Ready lane, a review item, or a [[root-parent]]; root, clock,
and type walks pass through it to the nearest area or project above it (its
owner).

Its single `parent` names an area note, a project note, or another notebook, and
the chain always reaches an area or project. A one-line `summary:` states what
belongs there. Every area or project lists the notebooks beneath it, down to the
next root parent, in a machine-owned `## Notebooks` catalog that
`bob notebooks sync` maintains: its content is bob's, its position is Bryan's.
`bob notebooks list` and `bob notebooks check` report the tree, counts, and
contract findings.

In [[capture]], the destination decides. On a notebook route, `@nb` adds a
top-level note, `@nb#section` adds one to that `##` section (exact match,
created on demand), `@nb^id` or `@nb#section^id` adds a named note, `@nb+id`
adds a child of the note bullet `^id`, and a draft-wide `@@nb` sends every
unrouted item there as a note. In a route-only `@nb` list, a first-level
`- #Section` bullet selects a section and its nested one-line bullets become
notes in that section; plain first-level siblings remain top-region notes.
`@parent^id@ %n <summary>` creates a notebook with the same section-bullet
behavior, while a bare `%n` stays a Pomodoro note. Task-only markers are
refused with a rewrite to the owner's route.

Not a [[reference-note]], a [[record-note]] or [[album-note]], a [[task-drawer]],
a [[period-note]], the Pomodoro note `%n` writes, or an Obsidian folder. An album
and a notebook are siblings under an area or project; neither can parent the
other. See
`docs/notebooks.md` and [[decisions:notebook-routes-write-notes]].

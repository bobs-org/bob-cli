---
keyword: Notebook
aliases:
  - "notebook note"
---

A Bob vault Markdown file with frontmatter `type: "[[notebook]]"` that keeps a
named collection of related notes (observations, explanations, ideas, logs)
without the obligations of a new [[area-note]] or [[project-note]]. Its body
is note bullets, optionally grouped under `##` sections below an H1 title;
each bullet may carry nested detail, a `[created::]` stamp, and a stable
`^id`. It never holds a `#task` or any checkbox, so it is never a task's
residence, a Pomodoro home, a Ready lane, a review item, or a [[root-parent]];
root, clock, and type walks pass through it to the nearest area or project
above it (its owner).

Its single `parent` names an area note, a project note, or another notebook,
and the chain always reaches an area or project. A one-line `summary:`
states what belongs there. `bob notebooks list` and `bob notebooks check`
report the tree, counts, and contract findings. See `docs/notebooks.md`.

Not a [[reference-note]], a record note, a task drawer, or an Obsidian
folder.

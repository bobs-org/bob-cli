---
keyword: Notebook Card
aliases:
  - notebook title card
---

The note-top [[property-card]] for a [[notebook]]. Bob Ledger Tools paints it
in desktop Live Preview and Reading view, and it replaces the linked notebook
header wherever it mounts.

- **Identity.** A spine tile and a `NOTEBOOK` kicker, with the owner's crumbs
  in the root hue, lead the notebook title (opening H1, which hides, else the
  first alias, else the humanized stem) and its `summary:` scope line (a quiet
  `＋ Add summary` when missing).
- **Data.** From the shared `bob notebooks list` report it adds the direct note
  count, the newest dated note (an authored `[created::]` date, never file
  time), section chips with counts that jump to their `##` headings, child
  notebooks, and findings: hard errors as a visible pill, warnings in the
  foot's lint chip. Missing report data is omitted, never zeroed.
- **Foot.** It carries `created`, the `@stem` capture route (click copies), and
  the Properties drawer, where importer keys fold under Managed.
- **Keys.** `==` focuses `summary`, `+` opens the Property Editor, `-` the
  drawer, and `_` folds the card's details.
- **Passive.** Painting writes nothing, creates no tasks, and spawns no bob.

Not the `[!notebooks]` catalog on an owner note, a task home, or a review
queue. See `docs/reference/plugins/notebook-card.md`.

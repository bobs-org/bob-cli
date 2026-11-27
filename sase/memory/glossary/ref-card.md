---
keyword: Ref Card
aliases:
  - reference card
  - ref title card
---

The note-top [[property-card]] for a library [[reference-note]] (exact
`type: "[[ref]]"` under `ref/`). Bob Ledger Tools paints it in desktop Live
Preview and Reading view. It answers what the reference is, where it came
from, where Bryan is with it, and what it holds, and it omits whatever a
note lacks; nothing is zeroed.

- **Identity (local, paints first).** A ribbon tile bearing the kind's icon
  and a `REF · <Kind>` kicker with the parent's crumbs in the root hue lead
  the title (`title:`, else the matching first H1, which hides, else the
  stem). An optional one-line `summary:` dek follows, then a byline of
  author, source host ↗, and publication date (never a file date), and PDF
  and Listen chips.
- **Reading (one shared `bob ref list` snapshot).** The reading lane appears
  in the Reading Board's words (Reading, Next, Ready, Snoozed with its wake
  date or prerequisite, Read with its date, Dropped, Legacy). A Today pill
  comes from the ledger, and annotation counts come from the last Highlights
  scan (never reading progress). Reread lineage, arXiv/DOI chips, and bob's
  diagnostics sit in the foot. Report data patches existing rows and never
  adds a line.
- **Keys.** `=` opens the Reference editor (Title, Summary, Authors,
  Published, Source, Aliases). It routes reading state, residence, and file
  name to the [[reference-task]] and `bob ref rename`. `+` opens the Property
  Editor; `-` opens the drawer, where bob-owned sync bookkeeping folds under
  Managed; `_` compacts the card to kicker and title.
- **Passive.** Painting never writes, spawns bob, or touches a PDF. Native
  Properties return on uncertain evidence, and the reading-task embed stays
  visible below the card.

Not the [[ref-panel]] (Bob Refs), the Reading Board, a reading task, a PDF
preview, or a schema editor. Root `*_ref` hub notes get no card. See
`docs/reference/plugins/ref-card.md`.

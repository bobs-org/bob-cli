---
keyword: Prose Card
aliases:
  - prose title card
---

The note-top [[property-card]] for a [[prose-note]], including the legacy
`[[article]]` spelling. Bob Ledger Tools paints it as a quiet title page in
desktop Live Preview and Reading view.

- **Identity.** A folio tile bearing the title's initial (or a pilcrow) and a
  `PROSE` kicker, with the parent's crumbs in the root hue, lead the title
  (`title:`, else the first alias, else the file stem verbatim) and the
  optional one-line `summary:` dek. A missing dek shows nothing.
- **Data.** From Bob Prose's read-only `api.prose` v1 outline snapshot it adds
  approximate words and reading time, the section count, `N to write` (stub
  sections with no deeper section; a written umbrella is not counted), and a
  findings lint chip. Parts without a snapshot are omitted, never zeroed;
  without Bob Prose the card is identity-only. A `§` chip shows local
  `section_numbers` and toggles it through Bob Prose's command.
- **Fold and title.** It is folded by default; `_` or a `to write` chip opens a
  bounded outline whose rows jump to their sections, then findings. It owns
  only the inline title, because every body heading is a section and stays
  visible.
- **Takeover and keys.** It replaces native Properties only under the family
  gate. `=` focuses `title`, `+` opens the Property Editor, and `-` the
  drawer.
- **Passive.** Painting never computes counts, numbers, or stubs, and never
  writes, formats, or spawns bob.

Not Bob Prose's formatter or hanging numbers, a publishing status, or a
completion score. See `docs/reference/plugins/prose-card.md`.

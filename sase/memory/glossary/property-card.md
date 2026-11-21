---
keyword: Property Card
aliases:
  - card family
---

The shared family of note-top panels — album, record, day, period, and bank
cards — built on the bob-ledger-tools kit. The kit owns the `.bob-property-card`
chrome, the `==` dispatcher registry (bank → album → record → day and period,
then extensions), owned-key property hiding, the host mount lifecycle, and the
title block. On album, record, day, and period notes the card is the note's one
visible title: a species glyph tile and kicker word (ALBUM, RECORD, DAY, WEEK,
QUARTER, YEAR) name the species, and hue never does. The inline title and the
recognized H1 or nav lines hide at render time only; a Live Preview cursor or
selection on them reveals the raw Markdown. Title ownership is separate from
Properties row ownership, and the bank card shows its species badge without
owning the title. "Card" means the panel; "card editor" means what `==` opens.
Painting reads the metadata cache and confirmed source bytes, never spawns bob,
and never writes the vault; a failed mount restores the native title, H1, and
Properties. See `docs/develop/plugins/architecture.md`.

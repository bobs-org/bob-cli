---
keyword: Property Card
aliases:
  - card family
---

The shared family of note-top panels — album, record, day, period, and bank
cards — built on the bob-ledger-tools kit. The kit owns the `.bob-property-card`
chrome, the `==` dispatcher registry (bank → album → record → day and period,
then extensions), individual-row and whole-frontmatter ownership, the host
mount lifecycle, and the title block. A mounted album, record, day, week,
quarter, or year card and foot replace in-document native Properties only in
desktop Live Preview or Reading, with valid source matching the cache,
confirmed `props_v1` for the current bob executable, and working editor routing.
Bank keeps its native block. The foot carries the created stamp and a complete
Properties drawer; `==` opens the species editor, `-` toggles the drawer, and
`+` opens the Property Editor in Live Preview Vim normal mode. On album,
record, day, and period notes the card is the note's one visible title: a
species glyph tile and kicker word (ALBUM, RECORD, DAY, WEEK,
QUARTER, YEAR) name the species, and hue never does. The inline title and the
recognized H1 or nav lines hide at render time only; a Live Preview cursor or
selection on them reveals the raw Markdown. Title ownership is separate from
individual-row and whole-frontmatter ownership, and the bank card shows its
species badge without owning the title. "Card" means the panel; "card editor"
means what `==` opens.
Painting reads the metadata cache and confirmed source bytes, never spawns bob,
and never writes the vault. Uncertain capability or source/cache evidence
restores native Properties with the existing per-row fallback; failed mounts,
Source, denied contexts, and unload restore native rendering. Add file property
opens Add mode on owned documents, while the File Properties sidebar stays
native. See `docs/develop/plugins/architecture.md` and
`docs/reference/plugins/property-editor.md`.

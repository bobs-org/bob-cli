---
keyword: Property Card
aliases:
  - card family
---

The shared family of note-top album, record, day, period, and bank cards, built
on the bob-ledger-tools kit. The kit owns `.bob-property-card` chrome, the `==`
dispatcher registry (bank → album → record → day/period, then extensions), row
and whole-frontmatter ownership, mount lifecycle, and title block. Desktop Live
Preview or Reading card-and-foot mounts replace native Properties only when
valid source matches the cache, `props_v1` is confirmed for the current bob, and
editor routing works; `bank.md` qualifies under the same gate. The foot shows
`created` and the full Properties drawer; `==` opens the species editor, `-`
toggles the drawer, `_` folds its section, and `+` opens Property Editor in Live
Preview Vim normal mode. Album, record, day, period, and bank cards own one
visible title: a species tile and kicker name it; hue does not. The inline title
and recognized H1/nav lines hide at render time; a Live Preview cursor or
selection reveals raw Markdown. Title ownership is distinct from row and
whole-frontmatter ownership. "Card" means the panel; "card editor" means what
`==` opens. Painting reads cached metadata and confirmed source, never spawns bob
or writes the vault. Uncertain evidence restores native Properties and the
existing per-row fallback; failed mounts, Source, denied contexts, and unload do
the same. Add file property opens Add mode on owned notes; the File Properties
sidebar stays native. See `docs/develop/plugins/architecture.md` and
`docs/reference/plugins/property-editor.md`.

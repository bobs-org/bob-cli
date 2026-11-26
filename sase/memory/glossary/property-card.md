---
keyword: Property Card
aliases:
  - card family
---

The shared family of title-slot album, record, day, period, [[area-card]],
[[project-card]], and [[bank-card]] views, built on the bob-ledger-tools kit.
The kit owns `.bob-property-card` chrome, the `==` species-editor dispatcher,
row and whole-frontmatter ownership, mount lifecycle, and title block. Album,
record, and temporal cards use `props_v1`; Area and Project settings require
`props_contract_v1` on top of `props_v1`. The path-canonical `bank.md` hub is
untyped and taskless; its one Bank Card uses `props_v1` and never has Area
semantics. The foot shows `created` and the full Properties drawer; `-` toggles
the drawer, `_` folds the card's own section, `==` opens the species editor,
and `+` opens the generic Property Editor in Live Preview Vim normal mode.
Every card owns one visible title: its species tile and kicker name it; hue
does not. The inline title and recognized H1/nav lines hide at render time; a
Live Preview cursor or selection reveals raw Markdown. Title ownership is
distinct from row and whole-frontmatter ownership. "Card" means the panel;
"card editor" means what `==` opens. Painting reads cached metadata and
confirmed source, never spawns bob or writes the vault. Uncertain evidence
restores native Properties and the existing per-row fallback; failed mounts,
Source, denied contexts, and unload do the same. Add file property opens Add
mode on owned notes; the File Properties sidebar stays native. See
`docs/reference/plugins/area-and-project-cards.md`,
`docs/develop/plugins/architecture.md`, and
`docs/reference/plugins/property-editor.md`.

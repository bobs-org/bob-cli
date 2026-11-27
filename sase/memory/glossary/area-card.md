---
origin: requested
scope: project
keyword: Area Card
---

A [[property-card]] home card for a note typed `[[area]]`. Its AREA tile and
kicker name the species; the title comes from the eligible H1, first alias, or
file name, and parent links form clickable crumbs. An authored `summary` is
the subtitle. The card shows only authored policy values and live readouts:
the Ready count and cap, direct Today count, inbox pill when Bob reports it,
and local lint. Missing report data stays absent, never zero. It does not show
inherited policies or subtree counts.

Area Settings require `props_contract_v1` on top of `props_v1`. One shared
Properties foot shows `created` and the full drawer. `-` toggles the drawer,
`_` folds the Area's own details, `=` opens Area Settings, and `+` opens the
generic Property Editor. Paint reads local metadata, confirmed source, and
cached reports; it never writes the vault or spawns bob.

Native Properties hide only while the current source matches the cache and the
required capabilities and editor routing are confirmed. Missing or damaged
evidence, a failed mount, Source, embeds, hover, canvas, print, and mobile
restore native rendering. `bank.md` is excluded by path because it is the
taskless Bank hub, never an Area note, even if it carries an accidental Area
type. See `docs/reference/plugins/area-and-project-cards.md`.

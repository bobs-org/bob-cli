---
origin: requested
scope: project
keyword: Project Card
---

A [[property-card]] home card for a note typed `[[project]]`. Its PROJECT tile
and kicker name the species; the title comes from the eligible H1, first
alias, or file name, and parent links form clickable crumbs. A ROOT pill
appears for `root: true`. The top-level recognized `^prj` task supplies the
WIP, SCHEDULED, BLOCKED, DONE, or CANCELED lifecycle. The task stays directly
below the card as its editable control; the card never rewrites or duplicates
it. Missing or invalid `^prj` rows show repair guidance.

The card shows authored policy values, project open and Today readouts, local
lint, and report-backed blocking reasons or close dates when available.
Unavailable reports hide those details instead of showing zero. Project
Settings require `props_contract_v1` on top of `props_v1`; the card does not
infer parent policies. One shared Properties foot shows `created` and the
full drawer. `-` toggles the drawer, `_` folds the card's own details, `==`
opens Project Settings, and `+` opens the generic Property Editor.

Paint reads local metadata, confirmed source, and cached reports; it never
writes the vault or spawns bob. Native Properties hide only while the source,
cache, capability, and editor-routing gates are confirmed. Missing or damaged
evidence, a failed mount, Source, embeds, hover, canvas, print, and mobile
restore native rendering. See `docs/reference/plugins/area-and-project-cards.md`.

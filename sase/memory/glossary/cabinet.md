---
origin: requested
scope: project
keyword: Cabinet
aliases:
  - property cabinet
  - cabinet stack
---

A named, independently collapsible compartment at the bottom of a
[[property-card]]. It has one header row (chevron, noun, a one-line summary
that still says what a closed cabinet holds, any attention chip or stamp,
and a keycap) over a body that opens in place. Cabinets stack in one fixed
order, with the **property cabinet** (Properties, toggled with `-`) always
last. Above it come content cabinets (Areas, Projects, Notebooks, and Albums
on an [[area-card]] or [[project-card]], Records on an [[album-card]],
Outline on a prose card), then configuration cabinets (Schema on an album
card, Setup on the [[bank-card]]). Empty catalogs get none. Records starts
open, the other content cabinets start closed, and Outline, Schema, and
Setup open themselves for a new problem.

A content cabinet shows the same rows, grouping, and actions as the
bob-owned catalog callout it stands in for. The callout stays in the note,
and only its rendered section (plus an H2 that labels nothing else) hides
while a working cabinet is mounted. Source mode, mobile, embeds, hover,
print, and anything uncertain show the stored section instead.

In Live Preview Vim normal mode, `_` toggles the only cabinet above
Properties. With two or more, it arms a numbered pick: digits count up from
`1` just above Properties and are lit on the headers and in the footer. A
digit toggles that cabinet, `_` again picks `1`, `-` picks Properties, and
Esc, `q`, or Ctrl+`[` cancels. A card with no cabinet above Properties keeps
`_` for its own details fold.

Open or closed is session-only view state: never frontmatter, never a vault
write, and never a bob spawn. Not a [[task-drawer]], an Obsidian heading or
callout fold, the compact details chevron, or the stored H2 section itself.
See `docs/reference/plugins/cabinets.md`.

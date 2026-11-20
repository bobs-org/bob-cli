---
keyword: Intention Card
aliases:
  - intentions card
---

A [[property-card]] Bob Ledger Tools paints at the top of a canonical daily or
period note in Reading view and Live Preview when any of `intentions`,
`summary`, or `reflection` is nonempty. It shows the numbered [[intention]]s,
SUMMARY with provenance (`✦ agent` for a stamped draft, `✎ edited` otherwise),
REFLECTION, and the parent line; a recent closed subject missing a band shows
one quiet pending line instead. `==` opens the card editor; every write goes
through `bob periods update` with the server's field fingerprint, and the card
fails open. Desktop-only: Source mode, embeds, popovers, and mobile keep native
Properties. See `docs/reference/plugins/intention-card.md`.

---
keyword: Property Card
aliases:
  - card family
---

The shared family of note-top panels — album, bank, record, and intention
cards — built on the bob-ledger-tools kit. The kit owns the `.bob-property-card`
chrome, the `==` dispatcher registry (bank → album → record → intention, then
extensions), owned-key property hiding, and the host mount lifecycle. "Card"
means the panel; "card editor" means what `==` opens. Painting reads the
metadata cache and confirmed source bytes, never spawns bob, and never writes
the vault; a failed mount leaves native Properties visible. See
`docs/develop/plugins/architecture.md`.

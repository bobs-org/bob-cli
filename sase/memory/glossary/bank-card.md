---
keyword: Bank Card
aliases:
  - bob-bank
  - setup panel
---

A [[property-card]] view of bank state. `bob-bank` fences show the queue
(`bank.md`), month (`bank/YYYY-MM.md`), and subscriptions (`subscriptions.md`).
`bank.md` is the untyped, path-canonical hub under cash; its parent is display
affinity, not an Area edge, and the note never holds tasks. Exactly one
title-slot Bank Card owns it, gated by source/cache evidence, confirmed
`props_v1`, and working editor routing. Otherwise native Properties and the
existing per-row `bank` fallback return. The card's Setup section shows
`bob bank config` categories, accounts, rules, and findings; its title block
keeps the bank identity and cash breadcrumb. Setup folds when healthy; `_`
toggles it, `=` opens Bank Setup, and `+` opens the generic Property Editor.
The `bank` row hands off to `=`; the card has no Area Settings, including when
older Bob reports the former hybrid contract. The shared foot is the only
Properties drawer, toggled with `-`. Painting never writes; mobile remains
read-only. Not the [[bank-badge]], Bank Period Charts, or a Stats fact. See
`docs/bank.md` and [[decisions:bank-hub-is-path-canonical-never-area]].

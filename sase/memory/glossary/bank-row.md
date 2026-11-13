---
keyword: Bank Row
aliases:
  - "bank row"
  - "transaction row"
---

One posted transaction or deposit as a top-level checkbox line under a day
heading in a [[bank-month-note]] (`- [x] $13.71 Smashburger [cat:: dining]`).
`[ ]` means waiting and `[x]` means acknowledged. Typed rows start ticked;
`bob bank import` writes the rest unchecked, once. Bob later only ticks a
waiting row (`ack`) or appends `^bank-<hex>` to a typed row it matched.
Ctrl+Enter on a waiting row opens the Category card, which ticks it and files
`[cat:: key]` in one gesture. The card stays open on the next waiting row. It
never carries `#task`, so it is never a task, and it is never a record
note. `bob bank check` lints every row. See `docs/bank.md`.

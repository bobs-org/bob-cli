---
keyword: Bank Month Note
aliases:
  - "bank month"
  - "month note"
---

A Bob vault note at `bank/YYYY-MM.md` under the bank hub (`bank.md`), with
`type: "[[bank_month]]"`, `parent: "[[bank]]"`, and `month: YYYY-MM`. The
filename is authoritative. It reads like a statement: one `##` heading per
day, oldest first, and [[bank-row]]s beneath. Bob writes the note once from
the canonical template (`bob bank ensure` or `bob bank import`). Totals on
the month card and in `bob bank report` are drawn at read time and never
written into the note. Not a [[notebook]], an album, or a
[[period-note]]. See `docs/bank.md`.

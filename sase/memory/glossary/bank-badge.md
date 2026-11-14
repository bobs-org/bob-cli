---
keyword: Bank Badge
aliases:
  - bank chip
  - daily bank chip
---

A read-only money summary that follows one [[day-strip]] or [[stats-strip]] and covers exactly that strip's period: a day, ISO week, quarter, ISO week-year, or all time over the bank's own history. bob supplies the counted totals, waiting [[bank-row]]s, and coverage from `bob bank report`. In counts `income` and `refund` inflows; Out counts outflows except transfers and cash-account rows. Each side appears only above $0.00, and the badge hides when both sides and period waiting are zero. In uses a `+` and green ink, Out uses body ink, and waiting uses warning ink. The chrome tints green only when In exceeds Out; otherwise it is neutral for counted money or amber for waiting without money. Color never carries meaning alone, and the badge names partial coverage. Its tooltip includes exact cents, excluded money, largest outflows, waiting, and coverage. Click, Enter, or Space opens the oldest waiting row, a day heading, the single [[bank-month-note]], or `bank.md`; it never creates a note or writes the vault. `stats.bank_badge` (alias `stats.bank_chip`) controls every host. The badge is not a Stats metric, not a `bob-bank` card, and does not appear in `bob-period-parts`.

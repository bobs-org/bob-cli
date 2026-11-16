---
title: "Audit subscriptions"
description: "Keep the subscriptions album honest in ten minutes a month: read the report, act on cancel and decide rows, and apply price hints."
tags:
  - CLI
  - Obsidian
---

# Audit subscriptions

Once a month, open the subscriptions album and run one report. The
report shows every discretionary recurring charge with its ledger, so
the audit is reading, not detective work: cancel what stopped earning
its keep, decide what is undecided, and accept the price changes the
bank already proves.

Exact flags, ledger rules, and the JSON contract live in
[Bank](../bank.md).

## Before you start

`bank.md` holds the `bank:` block and a subscriptions album exists under
it (the report names `bob albums create` when one does not). Ten quiet
minutes helps; the only commands are below.

## 1. Read the report

```bash
bob bank subscriptions
```

```text { .no-copy title="Output" }
🔁 Subscriptions   ≈ $136.39 a month · ≈ $1,636.68 a year
   1 to cancel · 1 to decide · 2 findings — bob bank check

✂️  To cancel
    Daily Ledger     $6.00  every 4 weeks  last Nov 10   next ≈ Dec 8    still charging
🤔  To decide
    Cloudbox        $19.99  monthly        last Nov 3    next ≈ Dec 3
💚  Active
    Codecraft      $100.00  yearly         last Feb 14   next ≈ Feb 14
    Tunely           $9.99  monthly        last Nov 21   next ≈ Dec 21   ↑ $10.99 since Oct
```

Groups follow action order: ✂️ act now, 🤔 decide, 💚 fine, 🚫 history.
The header estimates commitments, not bank spend: variable pricing
averages the trailing three months, and cancelled records count zero.
Cancelled records stay hidden unless `-a` is given or a finding names
them.

## 2. Act on ✂️ and 🤔

Open each To cancel row's Manage link and cancel for real, then mark the
record `cancelled` with its `ended` date so a later charge raises the
after-cancel sentry. Decide each To decide row one way or the other: keep
it (`active`), schedule its end (`to_cancel`), or retire it
(`cancelled`). Anything unconfirmed stays `to_decide`; the report never
decides for you.

## 3. Apply the price hints

A `↑` flag means the bank disagrees with the record's amount, and the
finding carries the paste-ready fix. Run it to accept the new posted
charge (tax included) as the record's amount, which clears the flag:

```bash
bob bank check
```

`check` prints the same findings with the exact
`bob records update subscription …` line per price change. Candidates
for possible new subscriptions arrive the same way, with a
`bob records new subscription …` line or a `bank.rules` dismissal.

## 4. Glance at the quiet rows

Skim the 💚 group for surprises: a `last` date older than one cadence
step, or a `next ≈` date that already passed. Rerun with `-a` once in a
while and confirm the cancelled group is still quiet; a charge after a
cancel always shows, even without `-a`.

## If it goes wrong

`subscription records are unavailable; run bob albums check` means the
album schema drifted; fix what `albums check` names and rerun. A
`bank_subscription_match_invalid` finding names a Shows-up-as needle the
matcher ignores; replace it with a specific, digit-free statement
phrase. A `bank_subscription_overlap` finding names two live records
sharing one needle; make the needles distinct or leave the shared phrase
matchless and link by hand.

## Related

- [Bank](../bank.md)
- [bob bank](../reference/cli/bank.md)
- [Bob Ledger Tools](../reference/plugins/bob-ledger-tools.md)

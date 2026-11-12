---
title: "Acknowledge bank transactions"
description: "Type today's posted rows in two minutes, import a weekly QFX, catch up after a lapse, and read account status."
tags:
  - CLI
  - Obsidian
---

# Acknowledge bank transactions

This guide is the daily bank ritual. Each posted transaction or deposit
becomes one checkbox row in this month's bank note. Typing it acknowledges
it. A Monday QFX import writes only what you missed, and `bob bank check`
proves the bank's own numbers.

Exact grammar, totals, and findings live in [Bank](../bank.md).

## Before you start

Install the CLI, point it at the vault, and enable **Bob Ledger Tools**.
`bank.md` already holds the `bank:` block and a `view: queue` card. The
default account is Wells Fargo checking.

## 1. Acknowledge what posted today

Open today's heading and type each posted row ticked. In a bank month note,
Enter after a row continues with `- [x] $`. Completing the chore with
nothing new is the record; do not write a fake `$0.00` row.

```bash
bob bank ensure -D today
```

```text { .no-copy title="Output" }
Created heading Thu Nov 12 in bank/2026-11.md
bank/2026-11.md:12
```

In Obsidian, **Bank: open today** runs the same ensure and parks the cursor
on that line. Type:

```markdown
- [x] $13.71 Smashburger [cat:: dining]
- [x] $4.25 7-Eleven — coffee on the walk
```

`$` is required. `+` is money in. `⚑` or `!!` flags a row to look into.
`—` or `--` starts a note. `[cat:: cash]` is an ATM; a `[acct:: cash]`
row traces where that cash went and never counts in Out.

Tick leftover waiting rows, or acknowledge the day in one gesture:

```bash
bob bank ack -D today
```

**Bank: acknowledge this day** acks the IDs the queue card drew.

```bash
bob bank
```

```text { .no-copy title="Output" }
🏦 Bank · nothing to acknowledge
  WF Checking · unverified · Nov 12

Download a QFX from Wells Fargo, then: bob bank import ~/Downloads/*.qfx
```

Today's daily note also shows a quiet chip, `🏦 out $17.96 · in +$0.00`.
A click ensures that day and jumps there.

## 2. Import last week's QFX on Monday

Download the last 14 days as Quicken (QFX) from Wells Fargo, then:

```bash
bob bank import ~/Downloads/*.qfx
```

```text { .no-copy title="Output" }
🏦 Import · WF Checking · Nov 1 – Nov 14 · Checking1.qfx
  9 new (waiting) · 23 matched your rows · 41 already seen · 0 ambiguous · 4 before start
  out $1,204.33 · in +$2,500.00
  ledger balance $12,345.67 as of Nov 14 · ✓ balanced
  → bank/2026-11.md  +9 rows, 23 IDs
Committed bank: import 9 rows (checking). Next: bob bank to acknowledge the 9.
```

Import writes new rows unchecked, matches typed rows by account, exact
cents, and a short date window, and commits once. Tick the waiting rows
with `bob bank` or the queue card. In last week's note, write one line:
what surprised you?

A second import of the same file is a no-op. `-d` prints the exact diffs
without writing.

## 3. Catch up after a lapse

Import fills the days you skipped. Acknowledge a whole day with
`bob bank ack -D YYYY-MM-DD` or **Bank: acknowledge this day**. Then:

```bash
bob bank check
bob bank report
```

A deleted imported row stays deleted until you restore it on purpose:

```bash
bob bank import -F
```

## 4. Add an account or a rule

The first `bank.accounts` alias is the default for rows without
`[acct::]`. Add a card later with one more entry (`kind: credit`, the same
`start:`). A `cash` account is never imported.

```yaml
rules:
  - match: VENMO
    cat: transfer
```

The first matching rule names rows that import writes. Categories are
optional; only `transfer`, `income`, `refund`, and `cash` change totals.

## 5. Read the status chips

| Status | Meaning |
| --- | --- |
| `balanced` | Coverage, presence, and the bank's ledger balance all hold |
| `delta` | The reconstructed ledger disagrees; `check` lists missing or edited rows |
| `gap` | QFX windows have a hole from `start` through the latest balance date |
| `stale` | The last QFX balance is older than `stale_after` (10 days) |
| `unverified` | No QFX ledger balance yet, including CSV-only |
| `untracked` | A `cash` account |

`bob bank` never says you are caught up while an account is stale, has a
gap, or shows a Δ.

## If it goes wrong

`bob bank check` names every finding. A malformed QFX refuses the whole
batch (exit 2) and writes nothing. Possible twins are listed; delete one,
bob never merges. An ignored `.bob/bank/<host>.jsonl` is refused until
`!/.bob/bank/*.jsonl` is on the vault `.gitignore` allowlist.

## Related

- [Bank](../bank.md)
- [bob bank](../reference/cli/bank.md)
- [Bob Ledger Tools](../reference/plugins/bob-ledger-tools.md)
- [Vault Git sync](../vault-git-sync.md)

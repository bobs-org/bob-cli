---
title: "Acknowledge bank transactions"
description: "File a week of bank rows in two minutes: Monday's QFX import, the Category card walk, the mid-week glance, and account status."
tags:
  - CLI
  - Obsidian
---

# Acknowledge bank transactions

Bank tracking is one Monday chore. Download the QFX, run one import, then
file each waiting row with one key: Ctrl+Enter opens the Category card, a
single key files the row's category, and the cursor lands on the next
waiting row. A week of transactions takes about two minutes.

Exact grammar, totals, and findings live in [Bank](../bank.md).

## Before you start

Install the CLI, point it at the vault, and enable **Bob Ledger Tools**.
`bank.md` already holds the `bank:` block and a `view: queue` card. The
default account is Wells Fargo checking.

## 1. Download and import on Monday

Open the weekly **Bank** chore and follow its Wells Fargo link (Download
Account Activity sign-on). Pick Checking and Quicken (QFX), keep the
default dates (at least 14 days back, so the overlap heals a skipped
week), and run one command on the Mac:

```bash
bob bank import ~/Downloads/*.qfx && rm ~/Downloads/*.qfx
```

The `rm` removes the download because the QFX holds the full account
number, and `&&` never deletes after a refused import.

```text { .no-copy title="Output" }
🏦 Import · WF Checking · Nov 1 – Nov 14 · Checking1.qfx
  9 new (waiting) · 23 matched your rows · 41 already seen · 0 ambiguous · 4 before start
  out $1,204.33 · in +$2,500.00
  ledger balance $12,345.67 as of Nov 14 · ✓ balanced
  → bank/2026-11.md  +9 rows, 23 IDs
Committed bank: import 9 rows (checking).
Next: file the 9 waiting rows — Ctrl+Enter on each in bank/2026-11.md (or bob bank to list them).
```

Import writes new rows unchecked, matches typed rows by account, exact
cents, and a short date window, and commits once. It ends by naming the
walk. A second import of the same file is a no-op. `-d` prints the exact
diffs without writing.

In last week's note, write one line: what surprised you?

## 2. File each waiting row with one key

In the month note, Ctrl+Enter on a waiting row opens the Category card.
Bob's suggestion is highlighted: ↵ (or a second Ctrl+Enter) accepts it, a
letter picks another category, Space files the row with no category, and
Esc cancels. Each filing ticks the row and writes `[cat:: key]` in one
undo step. The card stays open on the next waiting row, so the next key
files it, and Esc leaves the cursor there. After the last
row, a notice reports the session (rows, time, and accepted / picked /
no-category counts) and names any other months that still hold waiting
rows.

Ctrl+Enter on a ticked row only unticks it: no card, no jump.
**Bank: set category** opens the same card on any row later without
touching its checkbox, so a correction never disturbs the tick.

Row basics: `$` is required, `+` is money in, `⚑` or `!!` flags a row to
look into, and `—` or `--` starts a note. `[cat:: cash]` is an ATM; an
`[acct:: cash]` row traces where that cash went and never counts in Out.

## 3. Glance mid-week (optional)

Typing a row still acknowledges it. Open today's heading and type each
posted row ticked; in a bank month note, Enter after a row continues with
`- [x] $`. Typed rows match the next import by account, cents, and date.
Completing the chore with nothing new is the record; do not write a fake
`$0.00` row.

```bash
bob bank ensure -D today
```

```text { .no-copy title="Output" }
Created heading Thu Nov 12 in bank/2026-11.md
bank/2026-11.md:12
```

In Obsidian, **Bank: open today** runs the same ensure and parks the
cursor on that line. Type:

```markdown
- [x] $6.45 Corner Cafe [cat:: coffee]
```

Today's daily note keeps its quiet chip,
`🏦 out $17.96 · in +$0.00`. A click ensures that day and jumps there.

## 4. Catch up after a lapse

Import fills the days you skipped. Bulk paths stay tick-only and never set
a category: acknowledge a whole day with `bob bank ack -D YYYY-MM-DD` or
**Bank: acknowledge this day**, or click waiting checkboxes. Then walk the
month with the card and fix stragglers with **Bank: set category**. Then:

```bash
bob bank check
bob bank report
```

A deleted imported row stays deleted until you restore it on purpose:

```bash
bob bank import -F
```

## 5. Edit the categories

`bank.categories` in `bank.md` is an ordered map from key to emoji label.
A value is either a scalar or a mapping with a hotkey, a group, and a
hidden flag:

```yaml
categories:
  coffee: { label: ☕ Coffee, hotkey: c, group: Everyday }
  kids: { label: 🧸 Kids, hidden: true }
  transit: 🚆 Transit
```

Hotkeys are one character in `a-z0-9` (`q` always cancels the card).
Groups name the card's sections. A hidden category stays valid on rows, in
`check`, and in totals, but is never offered. Built-ins always exist
(`income` i, `refund` r, `transfer` t, `cash` a, under Money moves) and may
be overridden. Renames are free while no row carries a category, and new
rules stay rare: after one filing, learning suggests the repeat.

## 6. Read the status chips

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
`!/.bob/bank/*.jsonl` is on the vault `.gitignore` allowlist. If the card
opens restricted (Space and Esc only), update the plugins and retry:
Ctrl+Enter keeps its plain tick in the meantime.

## Related

- [Bank](../bank.md)
- [bob bank](../reference/cli/bank.md)
- [Bob Ledger Tools](../reference/plugins/bob-ledger-tools.md)
- [Vault Git sync](../vault-git-sync.md)

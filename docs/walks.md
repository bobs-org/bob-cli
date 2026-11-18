# Walk attention

One attention fact per bracket-walk family, shared by the Obsidian walker
footer and Bob Mac Capture's idle-agenda marks. `bob walks` is the
headless report. It is read-only: it never stamps a task, never walks a
queue, and never writes the vault.

The families are `s` (review), `w` (lanes), and `b` (bank), always in that
order. `n` never carries attention and is never in the report. A family's
mark is on exactly when the family is `available` and has `attention`.
bob-ledger-tools' selector pushes and Bob Mac Capture's idle-agenda marks
both follow this page.

## Families

Each family is computed on its own. A failure in one makes only that family
`available: false` with a `reason`; the others still report. A freshness
scan failure makes both `s` and `w` unavailable, because `w` uses the same
snapshot. An invalid plan config makes only `w` unavailable. An invalid
bank config makes only `b` unavailable. A teaching-empty bank (no
`bank.md` and no `bank/` folder) is available with waiting 0.

### s — review

One `freshness::scan::scan`. Counts come from the same helper
`bob freshness list` uses, so `walk` and `by_tier` cannot disagree.

The `s` mark is on exactly when `]s` has a row to land on: attention
iff `walk > 0`, with the panel reading `Review clear` when there is
nothing to land on.

- **commitments** = pre + new + projects + pending + next + recurring +
  tickler.
- **attention** iff `walk > 0`.
- **tier** = the first non-zero of all nine groups in walk order: PRE →
  NEW → PROJECTS → PENDING → NEXT → RECURRING → TICKLER → ROTTEN →
  POST. That is where a fresh `]s` enters the walk.
- **label** (attention only), in three forms:

  | commitments | label |
  | --- | --- |
  | ≥ 2 | `Review: {walk} due · {commitments} commitments — {TIER} first` |
  | 1 | `Review: {walk} due · 1 commitment — {TIER} first` |
  | 0 | `Review: {walk} due · commitments done — {TIER} first` (echoes the walk's own "Commitments done" notice) |

  Footer labels: PRE, NEW, PROJECTS, WIP (pending), NEXT, RECUR
  (recurring), TICKS (tickler), ROTTEN, POST.

PRE and POST share cyan; ROTTEN is orange. `counts.walk` is the Mac `s`
count.

### w — lanes

Uses the same snapshot. Lane counts are off-Today membership of the `]w`
walk, not due-for-review counts:

- **pending**: `snapshot.pending` rows
- **next**: `snapshot.next` rows
- **ready**: `snapshot.ready` rows whose review bucket is neither `new`
  nor `rotten`

All three exclude Today-linked rows (`is_today`) and the vault-root
`dash.md` (case-insensitive). Caps come from `plan.max_pending` (10),
`plan.max_next` (15), and `plan.max_ready` (100). **over** iff
`count > cap`; exactly at cap is not attention. **attention** iff any
lane is over. **excess** = `Σ max(0, count − cap)` over all three lanes;
it is the fewest task changes that silence `w`. **attention** iff
`excess > 0`. **tier** is `over`. **label** lists over lanes only, in
lane order: `Lanes over limit: WIP 12/10 · NEXT 16/15`. `counts.excess`
is the Mac `w` count.

### b — bank

`bank::scan::scan_bank` plus `bank::list::build` with the waiting filter,
across every `bank/YYYY-MM.md` month. `[ ]` is waiting; `[x]` or `[X]` is
acknowledged (the rule [Bank](bank.md) already defines). **attention** iff
`waiting > 0`. **tier** is `waiting`. **label**:
`Bank: 3 waiting · oldest Mon Nov 3`, using the bank walk's long day
label (`weekday month day`, unpadded day). Drop ` · oldest …` when no
date is known.

## Labels

bob owns the labels, byte-for-byte equal to the walker footer's push
labels minus the plugin-only ` — click to show` / `; click to show`
hints. Separators are the middle dot ` · ` and the em dash ` — `.

## JSON (schema_version 1)

New optional fields keep version 1; only a breaking change bumps it.
`walks` is always `s`, `w`, `b`. `tier` and `label` are non-null exactly
when `attention` is true. An available `s` carries
`"counts": {"walk": N, "commitments": N}`. An available `w` carries
`"counts": {"excess": N}` and `lanes` (all three, each with `lane`,
`label`, `count`, `cap`, `over`).
An available `b` carries `"counts": {"waiting": N}` and
`"oldest_waiting": "YYYY-MM-DD" | null`. `date` is the local today the
evaluation used. Unavailable families add `reason` and omit family extras.

```json
{
  "ok": true,
  "schema_version": 1,
  "date": "2026-11-17",
  "walks": [
    {
      "family": "s",
      "name": "review",
      "available": true,
      "attention": true,
      "tier": "next",
      "label": "Review: 14 due · 4 commitments — NEXT first",
      "counts": { "walk": 14, "commitments": 4 }
    },
    {
      "family": "w",
      "name": "lanes",
      "available": true,
      "attention": true,
      "tier": "over",
      "label": "Lanes over limit: WIP 12/10",
      "counts": { "excess": 2 },
      "lanes": [
        { "lane": "pending", "label": "WIP", "count": 12, "cap": 10, "over": true },
        { "lane": "next", "label": "NEXT", "count": 9, "cap": 15, "over": false },
        { "lane": "ready", "label": "READY", "count": 61, "cap": 100, "over": false }
      ]
    },
    {
      "family": "b",
      "name": "bank",
      "available": false,
      "attention": false,
      "tier": null,
      "label": null,
      "reason": "invalid bank config: …"
    }
  ]
}
```

Exit 0 whenever the report prints, including unavailable families.
Nonzero only for clap usage errors or an unusable vault root, with the
existing unversioned `{"ok": false, "error": …}` JSON envelope.

## Human output

```text
s ● Review: 14 due · 4 commitments — NEXT first
w ● Lanes over limit: WIP 12/10
b · Bank clear
```

The `●` takes the family's tier hue through the existing styler, honoring
`NO_COLOR` and non-TTY output. Calm lines are dim: `Review clear`,
`Lanes within limits`, `Bank clear`. An unavailable family prints
`? unavailable — <reason>`, dim.

## Conformance vectors

Rust tests in `tests/cli/walks.rs` run these verbatim; bob-ledger-tools'
JS tests do the same in a later phase. Goldens live in
`tests/fixtures/walks/` (`walks-clear.json`, `walks-all.json`,
`walks-bank-unavailable.json`).

| Id  | Setup                                                               | Expected                                                                                 |
| --- | ------------------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| S1  | ROTTEN 1                                                            | on, tier `rotten`, walk 1, commitments 0, `Review: 1 due · commitments done — ROTTEN first` |
| S2  | POST 1                                                              | on, tier `post`, walk 1, commitments 0, `Review: 1 due · commitments done — POST first`     |
| S3  | NEW 1, ROTTEN 1                                                     | on, tier `new`, walk 2, commitments 1, `Review: 2 due · 1 commitment — NEW first`           |
| S4  | PRE 1, NEXT 2                                                       | on, tier `pre`, `Review: 3 due · 3 commitments — PRE first`                                |
| S5  | TICKLER 2                                                           | on, tier `tickler`, label ends `TICKS first`                                               |
| S6  | ROTTEN 2, POST 1                                                    | on, tier `rotten`, walk 3, commitments 0, `Review: 3 due · commitments done — ROTTEN first` |
| S7  | nothing due                                                         | off, `tier`/`label` null, walk 0; human line `· Review clear`                              |
| W1  | PENDING 11, cap 10                                                  | on, `WIP 11/10`, excess 1                   |
| W2  | PENDING 10, cap 10                                                  | off, excess 0                               |
| W3  | a Today-linked `[/]` task                                           | not counted                                 |
| W4  | an unstamped (NEW) Ready task                                       | not in READY                                |
| W5  | a ROTTEN Ready task                                                 | not in READY                                |
| W6  | rows in `dash.md`                                                   | not counted                                 |
| W7  | future-scheduled, `#hide`, dependency-blocked, or `_templates` rows | not counted                                 |
| W8  | READY 101, cap 100                                                  | on, `READY 101/100`, excess 1               |
| W9  | PENDING 11, NEXT 22, plan.max_next 20                               | on, WIP 11/10 · NEXT 22/20, excess 3        |
| B1  | one `[ ]` row under a day heading                                   | waiting 1                                   |
| B2  | `[x]` and `[X]` rows                                                | waiting 0                                   |
| B3  | waiting rows in two months                                          | oldest comes from the earlier month         |
| B4  | invalid bank config                                                 | `b` unavailable; `s` and `w` still reported |

`excess` expectations are bob-only, because the walker footer renders no
`w` count.

## Related

- [Task freshness](freshness.md)
- [Plan budget, Today, and lanes](plan.md)
- [Bank](bank.md)
- [`bob walks`](reference/cli/walks.md)

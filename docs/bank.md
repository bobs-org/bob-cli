# Bank

A bank hub and monthly notes turn every posted transaction and deposit into
one checkbox row. Typing a row acknowledges it. Totals are drawn when they
are shown and never written into notes.

`bob bank` lives in the Integrations section of `bob --help`. Membership:
commands over bank rows, bank month notes, and their import journal. Bare
`bob bank` runs the read-only `list`. This page is the contract. `check`,
`list`, `report`, `ensure`, and `ack` ship now; `import` ships in a later
phase.

```text
bob bank [list] [-a] [-b DIR] [-f human|json] [-m MONTH]
bob bank ack [ID...] [-b DIR] [-D DAY] [-d] [-f human|json]
bob bank check [-b DIR] [-f human|json]
bob bank ensure [-b DIR] [-D DAY] [-f human|json] [-o]
bob bank report [-b DIR] [-D DAY] [-f human|json] [-m MONTH] [-w WEEK] [-y YEAR]
```

- Every long option has a short alias; options stay alphabetical.
  Subcommands are alphabetical.
- `list`, `report`, and `ensure` exit 0, or 1 on a runtime failure.
- `check` exits 1 on any error, 0 for warning-only, info-only, or clean
  reports, and 0 for the teaching empty state.
- `ack` exits 2 when an ID is unknown or isn't a row, and changes nothing
  on a refusal.
- `ensure` exits 2 on an invalid day or configuration.
- Real I/O failures are reported on stderr and exit 1.
- `check`, `list`, and `report` are read-only: vault bytes, index, and
  HEAD are unchanged. `ensure` and `ack` are guarded writes with no
  commit, which vault-sync carries.
- `-D` takes `today`, `yesterday`, `tomorrow`, or `YYYY-MM-DD`.
  `-m` takes `YYYY-MM`, `-w` takes `YYYY-Www` (ISO, Monday first), and
  `-y` takes `YYYY`. `report` takes at most one period and defaults to
  the current month. `report -D` yields one day.
- `BOB_NOW` overrides "today" everywhere.

## Files

| Path | Kind | Membership |
| --- | --- | --- |
| `bank.md` | Bank hub | Fixed vault-root stem. Frontmatter holds a `bank:` mapping. One per vault. |
| `bank/YYYY-MM.md` | Bank month note | The filename is authoritative. Other files in `bank/` are ignored. |
| `bank_month.md` | Type note | An ordinary `[[type]]` child. |
| `.bob/bank/*.jsonl` | Journal | A later-phase per-host journal. bob is the only writer. |

The scan reads `bank.md` and `bank/*.md` only. It never scans the whole
vault.

With no `bank.md` and no `bank/` folder, every `bob bank` command prints a
teaching empty state that names this page and a paste-ready `bank:` block,
then exits 0. `check` exits 1 with `bank_hub_missing` when `bank/` exists
without `bank.md`, or when `bank.md` has no `bank:` mapping.

Bank rows never carry `#task`, so they stay invisible to `bob plan`,
`bob ready`, `bob task archive`, `bob task reconcile`, freshness, status
hooks, and the Tasks plugin's `#task` global filter.

## `bank.md`

The hub is a sub-area (`type: "[[area]]"`, `parent: "[[cash]]"`). It holds
the `bank:` configuration, later a queue card, and bank tasks such as
disputes.

```markdown
---
parent: "[[cash]]"
type: "[[area]]"
bank:
  accounts:
    checking:
      label: WF Checking
      kind: checking
      start: 2026-11-12
  categories:
    groceries: 🛒 Groceries
    dining: 🍽️ Dining
  rules:
    - match: VENMO
      cat: transfer
---

# 🏦 Bank
```

| Key | Rule |
| --- | --- |
| `bank.accounts` | Required, non-empty, ordered map. Alias `[a-z][a-z0-9_]*`. The first account is the default for rows without `[acct::]`. |
| `…<alias>.label` | Optional display name. Defaults to the humanized alias. |
| `…<alias>.kind` | Required. `checking`, `savings`, `credit`, or `cash`. A `cash` account is never imported. |
| `…<alias>.start` | Required for every kind except `cash`. `YYYY-MM-DD`. |
| `bank.categories` | Optional ordered map from key to `"<emoji> Label"`. Labels split by the album choice-label rule. |
| built-in categories | `income` (💵 Income), `refund` (↩️ Refund), `transfer` (↔️ Transfer), and `cash` (🏧 Cash out) always exist. Declaring one overrides only its label. |
| `bank.rules` | Optional list of `{match, payee?, cat?, link?}`. First match wins; used when import writes rows. |
| `bank.stale_after` | Optional days (default `10`). |

Every configuration error names the file, the 1-based line, and the key
path, for example
`bank.md:5 bank.accounts.checking.kind: expected checking, savings, credit, or cash`.
Unknown keys inside `bank:` are errors with suggestions. Unknown top-level
YAML keys are tolerated. No account number, routing number, credential, or
balance is stored.

## Bank month notes

`bank/YYYY-MM.md` has `type: "[[bank_month]]"`, `parent: "[[bank]]"`, and
`month: YYYY-MM`. One `##` heading per day, oldest first, with bank rows
under each.

````markdown
---
parent: "[[bank]]"
type: "[[bank_month]]"
month: 2026-11
created: 2026-11-12T07:30:00-05:00
---

# 🏦 November 2026

```bob-bank
view: month
```

## Thu Nov 12

- [x] $13.71 Smashburger [cat:: dining]
- [ ] $61.20 ShopRite [cat:: groceries]
- [ ] +$1,250.00 Payroll [cat:: income]
````

**Day headings.** Canonical form: `## Thu Nov 12`. Also read: `## Nov 12`,
`## 2026-11-12`, and `## 2026-11-12 Thu`. Any other `##` heading is not a
day. An empty day heading is legal and means nothing posted.

Writers insert a missing day heading in date order, or after the fence when
there are none; separate sections with one blank line; put rows right under
the heading after one blank line. Planners preserve BOM, CRLF, and every
other byte.

## Row grammar

A **row** is a top-level list item (no indent) with a checkbox, under a day
heading, whose body starts with an amount. Indented children are free notes.
A checkbox line whose body does not start with an amount is a plain
checklist line.

```text
- [x] $13.71 Smashburger [cat:: dining] [acct:: card] [[eat/smashburger|🍽️]] ⚑ — with Kelly ^bank-8f3a2c1d
```

- **Box.** `[ ]` is waiting; `[x]` or `[X]` is acknowledged. Any other
  status is `bank_row_status`.
- **Amount.** Optional `**…**` bold, then an optional sign (`+`, `-`, or
  `−`), then a required `$`, then digits with optional thousands commas and
  an optional `.d` or `.dd`. `+` is money in. No sign, `-`, or `−` is money
  out. Parsed to signed integer cents. JSON uses negative for out.
- **Payee.** Text after the amount up to the first ` [`, `[[`, `—`, `--`,
  `⚑`, `!!`, or trailing ` ^id`. Trimmed; must not be empty.
- **Tail**, in any order: `[cat:: key]`, `[acct:: alias]`, wikilinks, `⚑`
  or `!!` (flag), and a note after the first `—`, `--`, or flag.
- **ID.** Trailing `^bank-<hex>` carries bank identity. Any other trailing
  block ID is ignored.
- **Empty stub.** `- [x] $` or `- [ ] $` with nothing after `$` is ignored
  and yields no finding.
- **Canonical amount display.** Thousands commas, two decimals, and `+` on
  money in.

Historical `@BANK` `total:` lines are never rows.

## Ownership

bob writes a row once. After that, bob makes only two edits to a row:
`ack` ticks `[ ]` to `[x]`, and import appends ` ^bank-<hex>` to a typed
row it matched. Bryan's edits always win.

## Commands

`list` (the bare default) shows the waiting queue across every month,
grouped by day, oldest first. `-a` shows all rows, and `-m` limits the
output to one month.

`report` explains one period with exact cents. Week boundaries are ISO,
Monday first. `report -D` yields one day: that is the daily chip's data.
Human output shows bars and a heat strip, and never depends on terminal
width. Amounts are tabular and right-aligned with explicit signs;
inflows are green and outflows are neutral, because spending is not an
error. State is always carried by text and shape, never by color alone.

`ensure` creates the folder, note, and heading on demand, and is
idempotent. It prints the path and the line where the next row goes
(after the day's last row, or the blank line under the heading). `-o`
opens the note in Obsidian.

`ack` ticks the named waiting rows, every waiting row of `-D DAY`, or
both. A row that is already ticked is a no-op.

## Totals

`report` computes every total from rows at read time. Rows on
`cash`-kind accounts and rows with `[cat:: transfer]` are kept apart.

| Total | Rule |
| --- | --- |
| Out | Outflows excluding `transfer` and cash-account rows. Includes `cash` (ATM) and uncategorized outflows. |
| In | Inflows categorized `income` or `refund`. A credit alone is never income. |
| Net | In − Out. |
| Refunds | `refund` inflows, also shown on their own. |
| Transfers | `transfer` in and out, shown, never in Out or In. A card purchase plus its payment from checking counts once. |
| Cash out | Outflows with `[cat:: cash]`, labelled "destination unknown". Cash-account rows sum into "cash traced". |
| Unclassified | Uncategorized outflows (inside Out) and uncategorized inflows (outside In). Always shown, never hidden. |
| Breakdown | Out by category, in declared order, then `cash`, then unclassified. Each entry has a share of Out. |
| Waiting / flagged | Counts of `[ ]` rows and `⚑` rows. |

An inflow with a spending category (for example, a restaurant refund
tagged `dining`) is counted as unclassified in, and `check` gives the
info hint `bank_inflow_category`: "use refund or income".

## Account status

Until import ships, every importable account is `unverified` (with the
hint `No import yet — bob bank import FILE`) and cash is `untracked`.
Later statuses: `balanced`, `delta`, `gap`, `stale`.

## Findings (`bob bank check`)

Human output groups by note, then day, and ends with a summary. `-f json`
follows the envelope below. `issues` counts errors and warnings (not info).

| Code | Severity | Trigger |
| --- | --- | --- |
| `bank_hub_missing` | error | `bank/` exists without `bank.md`, or `bank.md` has no `bank:` mapping |
| `bank_config_invalid` | error | Any `bank:` problem (file, line, key path) |
| `bank_day_invalid` | error | A date-shaped day heading outside the note's month, or an impossible date |
| `bank_row_unparsed` | error | An amount-led row that doesn't parse |
| `bank_row_payee_missing` | error | A row with no payee |
| `bank_row_status` | error | A row whose box is not `[ ]`, `[x]`, or `[X]` |
| `bank_row_task` | error | `#task` on a row |
| `bank_row_outside_day` | error | An amount-led row not under a day heading |
| `bank_category_unknown` | error | Unknown `[cat::]` value, with a suggestion |
| `bank_account_unknown` | error | Unknown `[acct::]` value, with a suggestion |
| `bank_id_duplicate` | error | One `^bank-` ID on two rows |
| `bank_month_mismatch` | warning | `month:` or `type:` disagrees with the filename |
| `bank_day_duplicate` | warning | Two headings for one day |
| `bank_day_order` | warning | Headings out of oldest-first order |
| `bank_day_weekday` | warning | A weekday that disagrees with the date |
| `bank_row_no_checkbox` | warning | An amount-led list item with no checkbox |
| `bank_row_zero` | warning | A `$0.00` row |
| `bank_field_unknown` | warning | An inline field other than `cat` or `acct` |
| `bank_flagged` | info | Each `⚑` row |
| `bank_inflow_category` | info | An inflow with a spending category |

Import and balance findings (`bank_journal_malformed`,
`bank_balance_delta`, `bank_coverage_gap`, `bank_row_missing`,
`bank_amount_edited`, `bank_account_stale`, `bank_row_unmatched`,
`bank_twins`) ship with those phases.

## JSON

`schema_version: 1`. Amounts are always signed integer `*_cents`, never
strings or floats. Every report shares the objects below. A row is
`{path, line, date, account, cents, payee, category, links, note,
flagged, acknowledged, id, origin}`; `origin` is `typed` (no ID),
`matched`, or `imported` (before the journal exists, a row with an ID
reports `imported`).

- `list` adds `filter` (`waiting` or `all`), `month`,
  `days[] {date, heading, path, line, rows[]}`, and
  `counts {rows, waiting, flagged}`.
- `report` adds `period {kind, key, start, end, label, so_far, note}`,
  `totals {out_cents, in_cents, net_cents, refund_cents,
  transfer_out_cents, transfer_in_cents, cash_out_cents,
  cash_traced_cents, unclassified_out_cents, unclassified_in_cents}`,
  `breakdown[] {key, emoji, label, out_cents, share, rows}`,
  `days[] {date, heading, path, line, out_cents, in_cents, rows,
  waiting, flagged}` (every day of the period, including empty ones),
  and `waiting {period, all, oldest}` plus `flagged`.
- `check` adds `findings[] {code, severity, path, line, message, hint}`.
- `ensure` adds `{path, line, day, created_note, created_heading}`.
- `ack` prints the envelope `ok`, `dry_run`, `changed`,
  `changes[] {path, diff?}`, `findings`, `commit`, `sync`, and
  `warnings`, plus `acknowledged[]`, `already[]`, and `unknown[]`.

Shared objects:

```json
{
  "ok": true,
  "schema_version": 1,
  "today": "2026-11-14",
  "categories": [
    { "key": "groceries", "emoji": "🛒", "label": "Groceries", "builtin": false }
  ],
  "accounts": [
    {
      "alias": "checking",
      "label": "WF Checking",
      "kind": "checking",
      "default": true,
      "start": "2026-11-12",
      "status": "unverified",
      "last_import": null,
      "balance_cents": null,
      "balance_asof": null,
      "delta_cents": null,
      "hint": "No import yet — bob bank import FILE"
    }
  ],
  "issues": { "errors": 0, "warnings": 0 },
  "findings": []
}
```

See [JSON outputs](reference/json-outputs.md#bank-reports).

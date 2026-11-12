# Bank

A bank hub and monthly notes turn every posted transaction and deposit into
one checkbox row. Typing a row acknowledges it. Totals are drawn when they
are shown and never written into notes.

`bob bank` lives in the Integrations section of `bob --help`. Membership:
commands over bank rows, bank month notes, and their import journal. Bare
`bob bank` runs the read-only `list`. This page is the contract. `check`,
`list`, `report`, `ensure`, `ack`, and `import` ship now; balance
verdicts ship in a later phase.

```text
bob bank [list] [-a] [-b DIR] [-f human|json] [-m MONTH]
bob bank ack [ID...] [-b DIR] [-D DAY] [-d] [-f human|json]
bob bank check [-b DIR] [-f human|json]
bob bank ensure [-b DIR] [-D DAY] [-f human|json] [-o]
bob bank import FILE... [-a ALIAS] [-b DIR] [-d] [-f human|json]
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
- `import` exits 2 on refused input or an unsafe/ambiguous batch, and 1
  on runtime failures.
- Real I/O failures are reported on stderr and exit 1.
- `check`, `list`, and `report` are read-only: vault bytes, index, and
  HEAD are unchanged. `ensure` and `ack` are guarded writes with no
  commit, which vault-sync carries. `import` is a guarded write with one
  scoped `bank: import` commit plus pre/post sync (see
  [Import](#import)). `-d` previews diffs without writing: no lock, no
  Git, no sync, no journal append.
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
| `.bob/bank/<host>.jsonl` | Per-host import journal | Each host appends to its own file; readers union every host file and never rewrite them. bob is the only writer. See [Journal](#journal). |

The scan reads `bank.md` and `bank/*.md` only. It never scans the whole
vault.

With no `bank.md` and no `bank/` folder, commands other than `import` print a
teaching empty state that names this page and a paste-ready `bank:` block,
then exit 0. `import` requires a valid hub and refuses missing configuration
with exit 2. `check` exits 1 with `bank_hub_missing` when `bank/` exists
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
`ack` ticks `[ ]` to `[x]`, and `import` appends ` ^bank-<hex>` to a typed
row it matched. A match changes only the trailing bank ID: checkbox,
wording, fields, links, notes, existing non-bank IDs, and child bullets
stay byte-identical. Bryan's edits always win.

## Import

`bob bank import FILE... [-a ALIAS] [-d]` plans the whole batch before
writing, then records posted exports in one guarded, committed
transaction. Files are parsed before the maintenance path is entered, so
a malformed member refuses the full batch with no vault, journal, Git,
or sync change. Files process in `(posted, input-file order, record
order)`; the same pure planner feeds preview and apply, so `-d`
after-images equal a real run on the same snapshot and clock.

```bash
bob bank import ~/Downloads/*.qfx
bob bank import -d ~/Downloads/checking.qfx
bob bank import -a checking -f json ~/Downloads/export.csv
```

### Sources

Two export shapes, told apart by content: OFX/QFX (an `OFXHEADER` or
`<OFX` probe) or the single headerless Wells Fargo CSV profile.

OFX accepts 1.x SGML leaf tags without closing tags and 2.x XML,
including preambles, CRLF/BOM, ordinary entities, and Wells Fargo
header/`CHARSET` noise. Transaction, transaction-list, statement, and
ledger-balance aggregate containers must close and nest correctly; a
truncated container refuses before the maintenance/sync lifecycle. Parsed
fields are `TRNTYPE`, `DTPOSTED`,
`TRNAMT`, `FITID`, `NAME`, and `MEMO`; coverage comes from
`BANKTRANLIST`'s `DTSTART`/`DTEND`; the ledger balance (never available
balance) comes from `LEDGERBAL`'s `BALAMT`/`DTASOF`; the account kind
comes from `BANKACCTFROM/ACCTTYPE` or `CCACCTFROM`. A valid empty
statement still carries coverage/balance evidence. OFX dates are
validated `NaiveDate`s; money is checked signed `i64` cents (fractional
cents and overflow refuse the batch). The statement keeps its posted
calendar day.

CSV is exactly five columns —
`Date, Amount, *, (blank), Description` — with quoted commas, escaped
quotes, CRLF, and signed decimal money. The profile is validated, not
guessed: any other column count or a non-blank fourth column refuses the
batch. Bank IDs are `csv:<date>:<cents>:<occurrence>`, with occurrence
counted per date/amount in file order, independently for each file, so
identical same-day purchases stay distinct and overlapping whole-date
exports reuse the same identities. CSV carries no balance or account
kind.

A batch fails entirely — exit 2, nothing written — on malformed
required fields, invalid dates, fractional cents, overflow,
unsupported currency, conflicting repeats of a bank ID, an
unresolvable account structure, or truncated input. A malformed
`STMTTRN` is never silently skipped. If one file holds several
statement blocks that cannot be resolved separately without conflating
accounts, import refuses and teaches separate per-account exports.

### Account resolution

`-a ALIAS` selects a configured importable account. Otherwise the file's
OFX kind chooses the unique configured account of that kind. Ambiguity
refuses with the candidate aliases and a `-a` hint; unknown and `cash`
aliases refuse. CSV always requires `-a ALIAS`, even with one configured
account. Account and routing numbers select nothing and
are discarded: never stored, never echoed.

### Matching

A transaction is **seen** when any host journaled its key, or its row ID
is already in the bank notes: it is never rewritten, ticked, or
resurrected. A row installed before a crash but not yet journaled is
recognized on retry and the missing journal evidence is filled without
writing a second row. A transaction dated before its account `start` is
recorded as `before_start` — journaled for audit, with no row written
or matched. Repeated transaction identities must keep their posted date
and cents across batch files and the folded journal; conflicts refuse the
entire batch. If an unjournaled matching bank ID appears on multiple rows,
import refuses with the source file and record so it does not choose a row.

Anything else matches at most one typed row: same non-`cash` account,
exactly equal signed cents, no bank ID yet, posted within
`[posted − 1 day, posted + 3 days]`, uniquely nearest date, consumed
once per batch. An equal-distance tie writes a new waiting row and
counts `ambiguous`. Matches never choose by payee or file order to break
a tie. `$20` and `$24` stay separate because cents must be exactly
equal.

Otherwise import renders an unchecked canonical row under its posted
day with the shared month/day helpers. A new day heading follows the
whole previous day section, preserving trailing prose, checklist lines,
and fenced content; rows append after existing row blocks without
separating a row from its children. BOM and CRLF are preserved. Dates can
cross months.

### Naming

New rows resolve `payee` / `category` / `link` through four tiers,
highest first:

1. The first `bank.rules` entry whose `match` text is a
   case-insensitive substring of the raw description. Unspecified rule
   fields fall through to lower tiers.
2. The most recent acknowledged ID-bearing row with the same journal
   merchant (learned from current row text, never a stale journal copy).
   Waiting rows never teach a name; ties break deterministically by note
   and line.
3. Wells Fargo built-ins: ATM withdrawals (`cash`), ATM/overdraft fees
   (`fees` when configured), card payments and online transfers
   (`transfer`). The raw type and description classify before cleaning
   can remove a useful discriminator.
4. The cleaned, title-cased description.

Cleaning is conservative and fixture-driven: authorization/date
prefixes, `SQ *`, `TST*`, `PAYPAL *`, `#1234` store numbers, trailing
`S<digits> CARD <dddd>`, and recognizable trailing city/state spans are
stripped, but never so much that an ordinary multiword merchant is
erased. Only bank-derived payees (tiers 3–4) pass through the grammar
sanitizer; rule and learned payees are already vault text. Untrusted
source descriptions stay one grammar-safe payee line and cannot inject
headings, block IDs, fields, or links. A positive amount alone never
implies income or refund; undeclared categories are dropped.

With no selected link, an exact case-insensitive restaurant title match
through the records registry supplies its note link and kind emoji
(normally `[[eat/<stem>|<emoji>]]`). No fuzzy guessing, no record
mutation, no full-vault scan. Ambiguous titles omit the link. An
explicitly selected higher-priority link is preserved. Existing
written/matched rows are never renamed or recategorized.

### Hashed IDs

The bank ID (`FITID` or the CSV occurrence key) is transient: it lives
only in parse/identity state and never in output, errors, or journal
payloads. The journal key is SHA-256 of `alias + "\n" + bank_id`. The row
ID is the key's first eight hex characters, extended to twelve when a
different known key occupies the eight-character ID; an unresolvable
twelve-character collision refuses rather than conflates rows. Every run
of at least four digits in `desc` is masked as `••••`, and the merchant
key derives from cleaned, masked text so identifiers cannot leak
through that second field.

### Journal

`.bob/bank/<host>.jsonl` journal events use the existing `v: 1`
envelope with timestamp, host, and kind:

- `bank.import`: `{account, source, file_sha256, start, end,
  balance_cents?, balance_asof?, counts}`. `source` is the export
  format (`ofx` or `csv`); raw paths, files, and bank identifiers are
  never journal payloads.
- `bank.txn`: `{account, key, id, posted, cents, desc, merchant,
  outcome, note}`. Outcomes are `written`, `matched`, or
  `before_start`; `note` is the vault-relative target note, null for
  before-start transactions.

Reads union every host in deterministic writer order, expose
contradictions instead of silently overwriting immutable money, and
validate dates, digest lengths, aliases, integer cents, outcomes, and
relative note paths. Malformed lines produce bounded
`bank_journal_malformed` warnings without printing the raw line, and
`check` surfaces them. Reads keep enough original event metadata for the
balance phase to select anchors, union windows, and prove presence;
repeated observations never multiply amounts.

A new import event is created for new evidence (a new file
digest/window/balance), even when all its transactions were seen. A
fully recorded identical replay has no new events and no commit. A new
empty statement may change only the journal. Counts distinguish new,
matched, seen, ambiguous, and before-start; ambiguous is a subset of
new. Journal `txn` events emit once per new identity, including
recoveries, never once per overlapping file.

### Commit, preview, and replay

Apply runs inside the shared transaction: validate row/read/journal
preimages plus the snapshot membership used for dedupe, install guarded
month files first, append prepared events to this host's journal, verify
rows and journal, then include both paths in one scoped commit —
`bank: import N rows (<alias>)`, or a stable comma-separated alias list
for multi-account batches. `N` counts new waiting rows, never matches
or journal events. On pre-commit failure, owned row writes and appends
roll back only while current bytes still match what this transaction
installed; later external edits are preserved and incomplete rollback
is reported. A commit that succeeded stays committed. Post-sync failure
keeps the local commit and prints the recovery hint instead of rolling
back.

Before installing month rows, real apply checks the destination
`.bob/bank/<host>.jsonl` against Git ignore rules. If Git ignores it,
import refuses without writing rows or a journal and shows the vault
`.gitignore` allowlist line `!/.bob/bank/*.jsonl`; it never force-adds
ignored data. Dry run does not run Git.

Dry run invokes only the shared pure planner and shows exact month and
journal after-images/diffs. A dry run creates no directories, appends
nothing, runs no Git or sync, and acquires no write lock. Identical
replay produces no scoped import commit and no new journal events.

Human output prints one block per file (account, date range; new,
matched, seen, ambiguous, before-start counts; out/in cents; ledger
balance and date when present, always marked unverified), then affected
paths, commit/sync results, and a next action. It respects `NO_COLOR`
and pipes, and never claims balanced or all caught up. Usage errors
exit 2.

### Current limits

Balance verdicts stay unverified: account `status` remains
`unverified`, and there are deliberately no balanced/delta/gap/stale
verdicts, no coverage or presence findings, no twins, and no `-F`
restoration until the balance phase lands. Also out of scope: plugin UI,
subscriptions, historical `@BANK` migration, feeds, budgets, pending
transactions, and multi-currency accounting. CSV is a fallback profile
with no cross-format fuzzy identity inference.

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

`import` plans the whole batch, then writes new rows and the journal in
one scoped commit (see [Import](#import)). `-d` previews month and
journal diffs without writing anything.

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

Until the balance phase ships, every importable account is
`unverified` (with the hint `No import yet — bob bank import FILE`) and
cash is `untracked`. Later statuses: `balanced`, `delta`, `gap`,
`stale`.

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

`bank_journal_malformed` ships with import (warning: a skipped journal
line, with bounded output). Balance findings (`bank_balance_delta`,
`bank_coverage_gap`, `bank_row_missing`, `bank_amount_edited`,
`bank_account_stale`, `bank_row_unmatched`, `bank_twins`) ship with the
balance phase.

## JSON

`schema_version: 1`. Amounts are always signed integer `*_cents`, never
strings or floats. Every report shares the objects below. A row is
`{path, line, date, account, cents, payee, category, links, note,
flagged, acknowledged, id, origin}`; `origin` is `typed` (no ID),
`matched` (ID with journal `matched` evidence), or `imported` (an ID
without journal evidence, including every journaled `written` row).

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
- `import` prints the envelope `ok`, `today`, `categories`, `accounts`,
  `issues`, `dry_run`, `changed`, `changes[] {path, diff?}`,
  `findings[] {code, path, line, message}`, `commit {sha, subject}`,
  `sync {pre, post}`, and `warnings`, plus the parent-contract
  `files[] {file, account, source, start, end, new, matched, seen,
  ambiguous, before_start, out_cents, in_cents, balance_cents,
  balance_asof, status}` and `rows[]` using existing row JSON.
  `balance_cents`/`balance_asof` report the ledger balance and date with
  no balanced verdict. Diagnostics identify a file and field/record
  position without echoing private identifiers or raw data.

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

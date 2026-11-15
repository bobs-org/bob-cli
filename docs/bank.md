# Bank

A bank hub and monthly notes turn every posted transaction and deposit into
one checkbox row. Typing a row acknowledges it. Totals are drawn when they
are shown and never written into notes. Weekly ritual:
[Acknowledge bank transactions](guides/acknowledge-bank-transactions.md).

`bob bank` lives in the Integrations section of `bob --help`. Membership:
commands over bank rows, bank month notes, their import journal, and the hub's
`bank:` setup. Bare
`bob bank` runs the read-only `list`. This page is the contract. `check`,
`config`, `list`, `report`, `ensure`, `ack`, and `import` ship now, including account
balance verdicts and deliberate restoration of missing imported rows.

```text
bob bank [list] [-a] [-b DIR] [-f human|json] [-m MONTH]
bob bank ack [ID...] [-b DIR] [-D DAY] [-d] [-f human|json]
bob bank check [-b DIR] [-f human|json]
bob bank config [show] [-b DIR] [-f human|json]
bob bank config update [-b DIR] [-d] [-f human|json] [-i FILE]
bob bank ensure [-b DIR] [-D DAY] [-f human|json] [-o]
bob bank import [FILE...] [-a ALIAS] [-b DIR] [-d] [-F] [-f human|json]
bob bank report [-b DIR] [-D DAY] [-f human|json] [-m MONTH] [-p PERIOD] [-s] [-w WEEK] [-y YEAR]
```

- Every long option has a short alias; options stay alphabetical.
  Subcommands are alphabetical.
- `list`, `report`, and `ensure` exit 0, or 1 on a runtime failure.
- `config show` exits 0 for a valid or teaching-empty setup, and 1 when
  the configuration is missing or invalid.
- `config update` exits 2 on envelope or semantic refusal, and 1 on
  runtime, lock, or transaction failure.
- `check` exits 1 on any error, 0 for warning-only, info-only, or clean
  reports, and 0 for the teaching empty state.
- `ack` exits 2 when an ID is unknown or isn't a row, and changes nothing
  on a refusal.
- `ensure` exits 2 on an invalid day or configuration.
- `import` exits 2 on refused input or an unsafe/ambiguous batch, and 1
  on runtime failures.
- Real I/O failures are reported on stderr and exit 1.
- `check`, `config show`, `list`, and `report` are read-only: vault
  bytes, index, and HEAD are unchanged. `ensure` and `ack` are guarded
  writes with no commit, which vault-sync carries. `config update` is a
  guarded write with one scoped `bank: config` commit plus pre/post
  sync. `import` is a guarded write with one scoped `bank: import`
  commit plus pre/post sync (see [Import](#import)). `-d` previews diffs
  without writing: no lock, no Git, no sync, no journal append.
- `-D` takes `today`, `yesterday`, `tomorrow`, or `YYYY-MM-DD`.
  `-m` takes `YYYY-MM`, `-w` takes `YYYY-Www` (ISO, Monday first), and
  `-y` takes `YYYY` (calendar year). `-p` takes one stats-strip period:
  `YYYY-MM-DD`, `GGGG-Www`, `GGGG-Qn`, `GGGG` (ISO week-year, as on
  period notes), `all`, `last-week`, `last-quarter`, `last-year`, or a
  period-note path. `-p 2026` is the ISO week-year; `-y 2026` is
  January–December. `report` takes at most one of
  `-D`/`-m`/`-p`/`-w`/`-y` and defaults to the current month.
  `report -D` yields one day; `report -p all` spans the bank's own
  history, from the earliest account `start` or row date through the
  later of today and the latest row date. `-s` omits per-day rows;
  totals, breakdown, waiting, coverage, and navigation remain.
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

With no `bank.md` and no `bank/` folder, commands other than `import` exit 0.
Human output names this page and includes a paste-ready `bank:` block;
JSON reports an empty result. `import` requires a valid hub and refuses
missing configuration with exit 2. `check` exits 1 with `bank_hub_missing` when `bank/` exists
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
    coffee: { label: ☕ Coffee, hotkey: c, group: Everyday }
    kids: { label: 🧸 Kids, hidden: true }
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
| `bank.accounts` | Required, non-empty, ordered map. Alias `[a-z][a-z0-9_]*`. The first account is the default for rows without `[acct::]`; imports omit that default alias from new rows and write `[acct:: alias]` only for another account. |
| `…<alias>.label` | Optional display name. Defaults to the humanized alias. |
| `…<alias>.kind` | Required. `checking`, `savings`, `credit`, or `cash`. A `cash` account is never imported. |
| `…<alias>.start` | Required for every kind except `cash`. `YYYY-MM-DD`. |
| `bank.categories` | Optional ordered map. A value is the scalar `"<emoji> Label"` or a mapping. Labels split by the album choice-label rule. |
| `…<key>.label` | Required in the mapping form. Same emoji/label split as the scalar form. |
| `…<key>.hotkey` | Optional. One character in `[a-z0-9]`. A YAML integer 0–9 reads as that digit; quote it in docs (`hotkey: "1"`). `q` is reserved. |
| `…<key>.group` | Optional, non-empty text. Names the Category card's section. |
| `…<key>.hidden` | Optional boolean, default false. A hidden category stays valid on rows, in `check`, and in totals. It is never offered as a suggestion. |
| built-in categories | `income` (💵 Income, `i`), `refund` (↩️ Refund, `r`), `transfer` (↔️ Transfer, `t`), and `cash` (🏧 Cash out, `a`) always exist, in group `Money moves`. Declaring one may override its label, hotkey, group, or hidden flag. Undeclared built-ins follow the declared keys. |
| `bank.rules` | Optional list of `{match, payee?, cat?, link?}`. First match wins; used when import writes rows and when read-time suggestions run. A match text with a run of four or more digits cannot match at read time (the journal description is masked). |
| `bank.stale_after` | Optional days (default `10`). |

Hotkeys are assigned once, in this order: every explicit hotkey is claimed
(including hidden keys, so unhiding never reshuffles); built-in defaults
are claimed unless taken; remaining visible categories derive a key in
declared order (first free character of the key, then the rest of the key,
then the label's ASCII letters lowercased, then `1`–`9` and `0`). A visible
category with no free character reports `hotkey: null`. Hidden categories
without an explicit hotkey report `null`.

Every configuration error names the file, the 1-based line, and the key
path, for example
`bank.md:5 bank.accounts.checking.kind: expected checking, savings, credit, or cash`.
Unknown keys inside `bank:` are errors with suggestions. Unknown top-level
YAML keys are tolerated. No account number, routing number, credential, or
balance is stored.

## Setup

`bob bank config` and `bob bank config show` print the effective `bank:` setup
used by bob. The human view groups effective categories by group, shows the
assigned key and its source, lists free keys and account status, and points
findings at their `bank.md` line. `-f json` returns the same versioned report
used by setup surfaces. The command is read-only.

Category entries distinguish declared categories from bob's built-ins, and
show whether labels are authored or supplied by default. Hotkey provenance is
`explicit`, `builtin_default`, `derived`, or `none`; a hidden explicit key still
reserves that key. `free_hotkeys` excludes assigned keys and reserved `q`.
Account usage separates rows with `[acct:: alias]` from rows using the first
configured account by default. Category usage counts row tags and rule
references. Account journal usage counts import and transaction events.

The `sha256:` `config_fingerprint` hashes a canonical JSON encoding of authored
configuration meaning. It includes accounts in declaration order (alias,
authored label or null, kind, and start), categories in declaration order (key,
label, explicit hotkey or null, group or null, and explicit hidden value or
null), rules in order (match, payee, category, and link), and explicit
`stale_after` or null. Flow/block YAML style, quoting, comments, and whitespace
do not change it; authored semantic changes do. `list` and `report` also return
this fingerprint so clients can guard later writes without another read.

When `bank:` is invalid, the report keeps any safely parsed configuration and
returns every located finding. A missing mapping returns a starter block; a
teaching-empty vault returns the same starter guidance without inventing a
configuration. A rule match with a run of four or more digits gets a warning
because digit masking prevents it from matching read-time hints; imports still
use the rule.

### `bob bank config update`

The guarded writer. Envelope
`{"schema_version": 1, "expect": "<fingerprint>", "ops": [...]}`.
`-i FILE` reads that JSON; `-i -` and omitted `-i` read stdin. Labels
never reach argv. An empty `ops` list is a valid no-op. Duplicate JSON
keys, unknown versions, unknown ops, unknown keys, and wrong types
refuse with structured `failure`.

Ops, applied atomically in order then emitted through the entry editor:

| Op | Payload | Effect |
| --- | --- | --- |
| `category.add` | `{label, group?, hotkey?, hidden?, after?\|before?}` | Derives the stored key from the label (emoji stripped, lowercased, non-alphanumerics → `_`, must match `[a-z][a-z0-9_]*`). Same-label collision refuses `category_exists` (suggesting unhide when hidden); any other key collision suffixes `_2`, `_3`. Pins bob's suggested free key unless `hotkey` is given. Default position is after the last member of the group. |
| `category.set` | `{key, set{label, group, hotkey, hidden}, unset[hotkey, group]}` | The stored key never changes. Setting an undeclared built-in creates its override entry. Unsetting `hotkey` returns it to derivation. |
| `category.reset` | `{key}` | Built-ins only: removes the override entry. |
| `category.move` | `{key, before\|after}` | Reorders declared categories. |
| `category.remove` | `{key}` | Only when the complete scan shows zero rows and zero rule references; otherwise `category_in_use`. Built-ins refuse `builtin_category`. |
| `group.rename` | `{group, to}` | Renames the group on every declared member. |
| `group.move` | `{group, before\|after}` | Moves the group's members as one block, preserving relative order. |

The built-in `Money moves` group of undeclared built-ins refuses
`group_builtin`. Hotkeys refuse `hotkey_taken` (naming the owner),
`hotkey_reserved` (`q`), and `hotkey_invalid`.

Every result carries `before_fingerprint`, `after_fingerprint`, one
human `consequence` line plus `per_op[]`, `hotkeys {shifted[] {key,
from, to}, free[]}`, the post-image `categories[]` (report shape),
`inverse_ops`, `changes[] {path, diff}`, `commit {sha, subject}`,
`sync {pre, post}`, `warnings`, and on refusal a typed `failure {code,
message, hint, op_index, key_path, line, affected}`. Human output
shows the consequence, the colored diff, hotkey table changes, then
commit and sync lines.

Writer guarantees:

1. Dry run, cancel, refusal, and no-op are byte-identical: no lock, no
   commit, no sync, no cache writes.
2. The post-image re-parses through `parse_bank_config` to the planned
   fingerprint; anything else refuses `post_image_mismatch` and writes
   nothing.
3. Only touched entries are re-emitted, in their own or their
   neighbours' style. Sibling keys (`accounts`, `rules`,
   `stale_after`, other top-level frontmatter) survive every op.
4. Unpatchable YAML (anchors, aliases, tags, duplicate keys, comments
   inside a flow value, multi-line flow values, tab indentation, a
   flow-style `bank:`/`categories:` container) refuses
   `unsupported_yaml`.
5. `expect` guards concurrency: a fingerprint mismatch refuses
   `stale_config`.
6. Apply goes through the shared vault lock: pre-sync, one scoped
   commit `bank: config (<short summary>)` (for example
   `bank: config (+fast_food)` or `bank: config (3 edits)`),
   post-sync, verify-installed rollback. A post-commit sync failure
   reports "saved locally; sync failed"; retries re-sync only.
7. `inverse_ops` revert the change as a normal edit with exact-byte
   restoration where the span is unchanged, and refuse `undo_blocked`
   when later rows now depend on what would be undone.

When `bank:` is invalid, every op refuses (`invalid_config`: "fix
bank: in YAML first"). Bob never edits a config it cannot fully parse.

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

`ensure` and `import` write this template once when the note is missing.
`created` is the write time in the configured `timezone` (the host zone when
unset) with an `±HH:MM` offset, and honors `BOB_NOW`.

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

## Walking bank rows

`]b` / `[b` step waiting rows oldest-first across every bank month (all rows
once nothing waits); `]B` / `[B` jump to the first row of the next /
previous day that has rows in the active scope. See the Bank walk section in
the [Bob Navigation Hotkeys reference](reference/plugins/bob-navigation-hotkeys.md#bank-walk).

## Ownership

bob writes a row once. After that, bob makes only two edits to a row:
`ack` ticks `[ ]` to `[x]`, and `import` appends ` ^bank-<hex>` to a typed
row it matched. A match changes only the trailing bank ID: checkbox,
wording, fields, links, notes, existing non-bank IDs, and child bullets
stay byte-identical. The Category card never writes rows itself: the
category it files is Bryan's edit in his editor, applied through the same
single editor transaction as the tick. Bryan's edits always win.

## Import

`bob bank import [FILE...] [-a ALIAS] [-d] [-F]` plans the whole batch
before writing, then records posted exports in one guarded, committed
transaction. Omitting FILE is refused input (exit 2) before the
maintenance lock or pre-sync unless `-F` (or `--force`) explicitly
requests restoration from the journal. Files are parsed before the maintenance
path is entered, so a malformed member refuses the full batch with no
vault, journal, Git, or sync change. Files process in `(posted, input-file order, record
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
are discarded: never stored, never echoed. The first configured account is
the default. New rows omit `[acct::]` for it and include the alias only for
non-default accounts; JSON continues to report the selected account.

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

When import writes waiting rows, the human `Next:` line points at the
walk and names the oldest month that still has waiting rows:
`Next: file the N waiting rows in bank/YYYY-MM.md — ]b walks them oldest first, Ctrl+Enter files each (or bob bank to list them).`
Several waiting months add `and K more months`; one waiting row reads
`Next: file the 1 waiting row in bank/YYYY-MM.md — ]b jumps to it, Ctrl+Enter files it (or bob bank to list it).`
JSON is unchanged.

### Naming

New rows resolve `payee` / `category` / `link` through four tiers,
highest first. Category uses one shared suggestion function with
`list`/`report` (rule, then learned, then bank). Import calls it with the
raw description and transaction type; reads use the masked journal
description and no type.

1. The first `bank.rules` entry whose `match` text is a
   case-insensitive substring of the raw description. Unspecified rule
   fields fall through to lower tiers. At read time the same rule runs on
   the journal description, or the payee when there is none; a match text
   with a run of four or more digits cannot match then.
2. The most recent acknowledged ID-bearing row with the same journal
   merchant (learned from current row text, never a stale journal copy).
   Waiting rows never teach a name; ties break deterministically by note
   and line. Typed rows join through `merchant_key(payee)`.
3. Wells Fargo built-ins: ATM withdrawals (`cash`), ATM/overdraft fees
   (`fees` when configured), card payments and online transfers
   (`transfer`). The raw type and description classify before cleaning
   can remove a useful discriminator. Reads classify from the journal
   description only.
4. The cleaned, title-cased description.

Cleaning is conservative and fixture-driven. It strips Wells Fargo QFX
`PURCHASE [RETURN] [AUTHORIZED ON] MM/DD … [CARD dddd]` and
`RECURRING PAYMENT [AUTHORIZED ON] MM/DD …` segments at the start or after
the merchant, including the place, state, and plain `CARD dddd` tail. The
CSV `AUTHORIZED ON` shape remains supported. It also removes `SQ *`, `TST*`,
and `PAYPAL *` prefixes,
short digit-bearing `*` order codes, `#1234` store numbers, trailing
`S<digits> CARD <dddd>` and `CARD <dddd>` tails, and recognizable city/state
spans. A word-only `*` sub-merchant stays distinct. Standalone long digit
runs are redacted, phone-number tokens are dropped, and formatted ACH/P2P
account-holder name tails are removed only after reference digits and a
recognized payment-rail marker. The stages never erase an ordinary
multiword merchant. A learned payee is re-cleaned only when it still carries
one of these forms of noise; an already-clean edited name stays byte-for-byte
unchanged. Bank-derived payees (tiers 3–4) pass through the grammar
sanitizer; rules remain vault text. Untrusted source descriptions stay one
grammar-safe payee line and cannot inject headings, block IDs, fields, or
links. A positive amount alone never implies income or refund; undeclared
categories are dropped.

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
key derives from the same cleaned, masked text. Purchase dates, places,
card tails, order codes, and other transaction-specific noise therefore do
not split repeat-merchant keys. Old journal events are recomputed from their
masked descriptions at read time; no journal migration is needed.

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
balance and date when present), followed by account statuses,
restoration details, affected paths, commit/sync results, and a next
action. It respects `NO_COLOR` and pipes. Usage errors exit 2.

### Current limits

Out of scope: historical `@BANK` migration, feeds, budgets, pending
transactions, and multi-currency accounting. CSV is a fallback profile
with no cross-format fuzzy identity inference. Subscription records are
attributed at import and read time as described in
[Subscriptions](#subscriptions).

## Subscriptions

A subscriptions album holds one record per discretionary recurring charge
(for example Tunely `$9.99` monthly, Cloudbox `$19.99` monthly). Records
never hold charges; `bob bank` attributes charges to records at read time
and import time from the same shared resolver and ledger. Import never
creates records, and no backfill rows are written.

The album binds by noun through the records registry
(`RecordContext::lookup_noun("subscription")`), never by folder stem. A
renamed album with the same noun works; a hard-coded `subscriptions/`
path does not. The link text is the album emoji.

Each record carries `status` (`to_cancel`, `to_decide`, `active`,
`cancelled`), `amount` (posted charge, tax included, exact integer cents),
`cadence` (`monthly`, `four_weekly`, `quarterly`, `yearly`), `pricing`
(`fixed`, defaulting to `fixed`, or `variable`), an optional `match`
list ("Shows up as"), `category` (a `bank.categories` key), `url`,
`renews`, and `ended`. An explicitly empty field is not absent. Malformed
required identity, status, amount, cadence, or pricing makes that record
unusable for matching; invalid optional dates become diagnosed absent
values and an unknown category cannot supply a suggestion. Invalid needles
are excluded while the rest of a valid record stays usable. Matchless
records stay explicitly linkable.

A needle matches as a case-insensitive substring of the raw description at
import time, the masked journal description at read time, or the payee for
a typed row without journal evidence. Trim needles, then drop any shorter
than three characters, any with an ASCII run of four or more digits, or
any equal after case folding to `PURCHASE`, `RECURRING`, `PAYMENT`,
`RECURRING PAYMENT`, `AUTHORIZED`, `AUTHORIZED ON`, `CARD`, `POS`, `ACH`,
`DEBIT`, `GOOGLE`, `APPLE`, `PAYPAL`, `AMAZON`, `SQ`, or `TST`. Deduplicate
remaining needles case-insensitively within each record. Collect matches
across all statuses; live beats cancelled, but cancelled-only attribution
still counts. One record wins only on a strictly longest matched needle;
an exact tie means no attribution with tied identities preserved. Never
use amount, file order, or history to break a tie.

Attribution covers every folded journal transaction once, including
`before_start`, joined to its unique current bank row when present (the
row supplies links and category, the journal supplies the descriptor).
ID-less typed rows join only outside that account's merged import windows.
Historical ledger membership is those journal transactions plus those
uncovered typed rows. Orphan journal IDs still receive a row projection;
they do not enter gross, net, latest, price, or timeline facts. A covered
typed row with an explicit subscription link keeps that row's projection.
Duplicate IDs never copy an arbitrary current row into history. Journal-only
classification uses the effective bank config: matching rules, then built-in
categories. Unique explicit row links stay authoritative. Sign alone never
infers income or refund. A unique explicit subscription link wins over every
pattern; a malformed or unresolved link never creates a phantom. Pattern
inference skips zero amounts, non-refund inflows, `transfer`/`income`/`cash`
rows, and Wells Fargo built-in transfers, cash withdrawals, and fees. A
refund (an inflow with `[cat:: refund]`) attributes and nets against spending
but never counts as a charge for latest, price, or interval facts.

Ledger facts are pure functions of the registry, attribution, coverage,
and today: charges oldest first with signed cents, refunds kept separate
with gross, refund, and net totals, latest charge, next expected (`renews`
or latest plus one cadence step with month-end clamping), monthly
equivalent (fixed by cadence with checked i128 half-up integer math,
variable as the trailing three-month charge average, cancelled as zero),
price change (fixed monthly or four-weekly needs the last two charges equal
and different from `amount`; quarterly or yearly needs the latest different;
variable never), still-charging (`to_cancel` with latest within 1.5 cadence
steps of today), after-cancel (cancelled with charges after `ended`), and
a twelve-bucket timeline (`charged`, `missed` only when merged coverage
spans the interval, `uncovered`, `upcoming`). Unrepresentable derived
amounts are omitted rather than zeroed. The current cadence interval is a
signed offset from the original anchor, so a future `renews` and an old
anchor both place today's bucket correctly. `to_cancel` marks only the
latest qualifying charge; `after_cancel` marks every charge that satisfies
that record's ended rule, and the record-level finding points at the newest.
Refunds are never charge sentries or variable usage charges.

Import preview and apply share one after-state: the existing journal folded
with this batch's new transaction and import events, bank rows overlaid with
planned after-images, then enrichment of the full snapshot before selecting
affected or restored IDs. `bob bank import -F` loads the same subscription
registry even without input files. Import plans bind the consulted album
declaration, parent and ancestor notes, every root Markdown note (including
notes that are not albums yet), and the record folder's membership,
including an absent folder. Changing any of those between plan and apply
refuses before writing rows or journal bytes.

A missing album means every subscription field is `null` or `false`. An
invalid album disables subscription facts for that read and adds one bounded
`bank_subscription_album_invalid` warning (with a `bob albums check` hint);
read output carries it once in JSON and once for humans, `bank check`
carries it as a warning finding, and import proceeds without facts with one
`bank_import_subscriptions_unavailable` finding. Unreadable or racing
membership refuses the import. Warning-only checks still exit 0.

## Commands

`list` (the bare default) shows the waiting queue across every month,
grouped by day, oldest first. `-a` shows all rows, and `-m` limits the
output to one month.

`report` explains one period with exact cents. Week boundaries are ISO,
Monday first. `report -D` yields one day, including the data used by a
daily Bank Badge.
`-p` reports any stats-strip period instead: a day, an ISO week, a
quarter, an ISO week-year, or all time over the bank's own history.
When the bank's tracked history or import coverage falls short of the
period, human output adds one line such as
`Tracked since Nov 11 · imported through Nov 13`.
`-c` adds the child-period chart aggregate described in
[Report charts](#report-charts) for a week (`-w`, or `-p` with a week), a
quarter, an ISO week-year, or all time. Human output then ends with one
aligned line per child period (label, In, Out, waiting count, and state
words such as `not imported yet`, `not tracked`, `partly covered`, and
`so far`), followed by the ISO year's quarters or all time's years. Any
other period exits 2 with
`-c/--charts needs a week, quarter, ISO year, or all-time period (-w or -p)`.
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
journal diffs without writing anything. `-F` (or `--force`) deliberately
recreates rows for journaled transactions that are missing from the month
notes. It works without source files, restores rows under their posted days,
and never duplicates a row whose ID is still present. Restored imported rows
wait for acknowledgment; rows originally matched to typed entries stay ticked.

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

## Bank badge

A Bank Badge follows a Day or Stats Strip and summarizes exactly that
strip's period: one day, ISO week, ISO quarter, ISO week-year, or all time
over the bank's tracked history. The plugin reads `bob bank report`; the
badge never writes the vault and is not a Stats fact or one of the
`bob-bank` cards. It does not appear in `bob-period-parts`.

| Report state | Badge |
| --- | --- |
| No counted money and no waiting rows | Hidden, with no added space |
| In and/or Out above $0.00 | Show each present side only; In has a `+` sign |
| No money but period waiting rows | Show the waiting count |
| Partial coverage | Name the tracked start and, for a closed period, any short imported-through date |

In counts `income` and `refund` inflows. Out counts outflows except
transfers and cash-account rows. Waiting counts only rows in the badge's
period. Day and week amounts show exact cents; quarter, year, and all-time
amounts show whole dollars rounded half-up, except nonzero amounts under
$1 retain cents. The tooltip keeps exact cents, excluded money, largest
outflows, waiting, and coverage.

In amounts use green ink, Out uses normal text, and waiting uses warning
ink. The chrome is green only when In exceeds Out; otherwise it is neutral
when money is shown, or warning-toned for waiting-only badges. It is never
red or magenta, and words and signs carry the meaning without color.

Activation opens the oldest waiting row when one exists. Otherwise, day
and week badges open the first day with counted money; quarter, year, and
all-time badges open the only month note when there is exactly one, and
otherwise open `bank.md`. This is read-only; it never creates a note.

`stats.bank_badge` in `~/.config/bob/config.yml` controls all badge hosts
and defaults to `true`. The permanent `stats.bank_chip` alias is used only
when `stats.bank_badge` is not a boolean.

## Bank period charts

A Bank Period Charts panel follows the Bank Badge on week, quarter, ISO
week-year, and all-time period notes, after that period's Stats Strip. It
never appears on daily notes, `bob-period-parts`, embeds, hover popovers, or
mobile. The plugin reads `bob bank report -p <id> -s -c`; the panel never
writes the vault and is not a Stats fact or one of the `bob-bank` cards.

The panel shows two charts and an optional quiet line:

- **In / Out**: when money moved, on the strip's own child-period axis —
  days of a week, weeks of a quarter or ISO year, quarters (or years past
  64 quarters) of all time. ISO years group weeks into quarters, and all
  time groups quarters into years.
- **Out by category**: where Out went, as ranked bars with the top 5 and
  an `N more` row. The plugin never adds money: bob folds the categories
  and supplies group totals.
- **Not counted**: a muted line naming only nonzero parts — transfers,
  unclassified inflows, traced cash, and (only when `stats.bank_badge` is
  off) the waiting count and coverage text.

One zero line runs through every cashflow chart. In hangs above it in
green; Out hangs below it in a neutral fill, so money never looks like the
upward Worked columns above it. Week notes draw In as markers above the
line; longer periods draw mirrored bars on one shared $/px scale. Only the
largest Out bucket (and, on mirrored charts, the largest In bucket) gets a
direct amount label. Every coverage state is also spelled out in words in
the tooltip, the readout, and the table twin: `not imported yet`
(`awaiting_import`), `not tracked` (`untracked`), `partly covered`
(`partial`), `so far`, and future stubs.

| Report state | Panel |
| --- | --- |
| Host gated off (config off, future period, not yet visible) | Nothing, and no bob spawn |
| Teaching empty | Hidden |
| Older bob refusing `-c`, or a bad `charts` payload | Hidden, plus one `console.warn` per session; the badge is unaffected |
| `charts` missing, undecodable, or for another period | One muted `Bank charts unavailable` line |
| No counted money and no waiting rows | Hidden, with no added space |
| No money but waiting rows, with `stats.bank_badge` on | Hidden |
| No money but waiting rows, with `stats.bank_badge` off | One quiet waiting line |
| Counted money | Header, the available charts, and the Not-counted line when it has parts |

A cashflow chart with fewer than 2 buckets is omitted, and an empty
category series is omitted; when only one chart is available it shows alone
and the view switch hides. The header holds a collapse disclosure
(`BANK CHARTS`), a view glyph cycling Both → In / Out → Categories, and a
readout naming the hovered or focused bucket. The per-device view and
collapse live under the `bob-ledger-tools-bank-period-view` localStorage
key. The cashflow axis is one roving tab stop (Arrow Left/Right, Home/End;
Enter, Space, or click activates; Escape clears the readout), and each
chart has a visually hidden table twin so no value hides behind hover.

Activation is read-only and never creates a note. Day buckets open the
month note at the day heading; period buckets open the child note, or show
`No note for <id> yet` when it does not exist. Category rows and the
Not-counted parts are inert, except the waiting link.

`stats.bank_period` in `~/.config/bob/config.yml` gates every period host
and defaults to `true`, independently of `stats.bank_badge`. Nothing is
written to notes, `bob periods brief`, `bob stats`, or Mac surfaces.

See [Report charts](#report-charts) for the `charts` contract and
[JSON outputs](reference/json-outputs.md#bank-reports).

## Account status

The first QFX ledger balance at or after an account's `start` anchors its
opening balance: bob subtracts the journaled transactions posted from
`start` through that balance's as-of date. Later QFX balances are checked
against the same derived anchor and current rows carrying journaled IDs.
Repeated observations do not count a transaction twice. Missing or edited
rows change the ledger total and are reported separately. QFX import windows
must form continuous coverage from `start` through the reported balance date.

Importable account statuses are `balanced`, `delta`, `gap`, `stale`, and
`unverified`; cash accounts are always `untracked`. `balanced` means the
latest QFX ledger balance agrees and coverage is continuous. `delta` means
the bank balance differs from the ledger sum. `gap` names a break in QFX
coverage. `stale` means the latest ledger balance is older than
`bank.stale_after` days (10 by default). `unverified` means there is no QFX
ledger balance yet, including CSV-only accounts. The JSON `last_import` is
the local date of the latest import event; `balance_asof` is the bank's date.

`bob bank check` also reports missing journaled rows, edited amounts,
possible twins, and ID-less typed rows in covered windows that have no bank
match. Edits remain in the note and never change the journal's bank amount.
Possible twins are warnings only; bob never merges them.

## Findings (`bob bank check`)

Human output groups by note, then day, and ends with a summary. `-f json`
follows the envelope below. `issues` counts errors and warnings (not info).

| Code | Severity | Trigger |
| --- | --- | --- |
| `bank_hub_missing` | error | `bank/` exists without `bank.md`, or `bank.md` has no `bank:` mapping |
| `bank_config_invalid` | error | Any `bank:` problem (file, line, key path) |
| `bank_rule_read_time_dead` | warning | A rule match has a run of four or more digits; it still applies during import, but digit-masked journal descriptions prevent read-time matches |
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

`bank_journal_malformed` is a warning for a skipped journal line, with
bounded output. Balance findings are:

| Code | Severity | Trigger |
| --- | --- | --- |
| `bank_balance_delta` | error | The latest QFX ledger balance differs from the derived opening anchor plus current rows carrying journaled IDs |
| `bank_coverage_gap` | warning | QFX import windows have a gap from the configured account start through the latest balance date |
| `bank_row_missing` | error | A journaled transaction's `^bank-` row is absent; restore deliberately with `bob bank import -F` |
| `bank_amount_edited` | warning | The row is present but its amount differs from the immutable journal amount |
| `bank_account_stale` | warning | The latest QFX ledger balance is older than `stale_after` |
| `bank_row_unmatched` | info | A typed row in a covered window has no exact bank match yet |
| `bank_twins` | warning | A typed row and imported transaction share a merchant/date window but differ in amount |

Subscription findings follow the note groups under one `🔁 Subscriptions`
heading. Warnings and info findings do not change `check`'s exit status.

| Code | Severity | Trigger |
| --- | --- | --- |
| `bank_subscription_cancelled_charge` | warning | A cancelled record has a charge after `ended`, or any charge when `ended` is empty; the newest charge is named |
| `bank_subscription_to_cancel_charged` | info | A `to_cancel` record was charged within 1.5 cadence steps |
| `bank_subscription_price` | info | The ledger detects a fixed-price change and offers a `bob records update` command |
| `bank_subscription_candidate` | info | A recurring marker appears, or stable charges form a monthly/four-weekly run of 3, a quarterly run of 3, or a yearly run of 2; the top 10 by annualized cost include a proposal and paste-ready create or dismiss fix |
| `bank_subscription_match_invalid` | warning | A Shows-up-as needle is too short, contains a masked four-digit run, or is statement noise |
| `bank_subscription_overlap` | info | Live records share a folded needle, or an observed transaction ties between records |
| `bank_subscription_category_unknown` | warning | A record names a category missing from `bank.categories`; check suggests the nearest key |
| `bank_subscription_amount_invalid` | warning | A record amount is missing, negative, or has fractional cents |
| `bank_subscription_album_invalid` | warning | The album cannot be read; check points at `bob albums check` |

Candidate detection groups eligible outflows by the cleaned, digit-masked
merchant. Bank rules, attributed transactions, refunds, built-in transfers,
cash withdrawals, fees, and non-outflows are excluded. The recurring marker
can identify a first charge; interval candidates need an amount spread no
greater than 25% of the median. Check proposes records or a `bank.rules`
dismissal and never writes either one.

## JSON

`schema_version: 1`. Amounts are always signed integer `*_cents`, never
strings or floats. Every report shares the objects below. A row is
`{path, line, date, account, cents, payee, category, links, note,
flagged, acknowledged, id, origin, suggestion, subscription,
bank_recurring, subscription_candidate}`; `origin` is `typed` (no ID),
`matched` (ID with journal `matched` evidence), or `imported` (an ID
without journal evidence, including every journaled `written` row).
`suggestion` is `{category, source}` or `null`; `source` is `rule`,
`subscription`, `learned`, or `bank`. Hidden and undeclared categories are
never suggested; a subscription category fills an uncategorized attributed
row only after `rule` and before `learned`. `subscription` is `null` or
`{stem, path, title, status, status_label, cadence, pricing, amount_cents,
source, needle, drift_cents, price_change, state}` with vault-relative
`.md` paths; `source` is `link` or `match`, `drift_cents` is the fixed
outflow minus `amount` (`null` for variable or refunds), `price_change` is
`{from_cents, to_cents, since}` or `null`, and `state` is `after_cancel`,
`to_cancel`, `usage`, `drift`, or `ok`. `bank_recurring` reads the
unmasked `RECURRING PAYMENT` marker; `subscription_candidate` is true only
for an eligible outflow with that marker, no attribution, and no matching
bank rule. A missing or invalid album forces the whole projection to
`null`/`false`. Each `categories[]` entry is
`{key, emoji, label, builtin, hotkey, group, hidden}`; `hotkey` and
`group` are a string or `null`.

- `list` adds `filter` (`waiting` or `all`), `month`,
  `config_fingerprint` (`sha256:` or `null` when YAML cannot be parsed),
  `days[] {date, heading, path, line, rows[]}`,
  `counts {rows, waiting, flagged}`, and `warnings[]`/`findings[]` carrying
  at most one bounded `bank_subscription_album_invalid` entry.
- `config show` adds `state` (`valid`, `invalid`, `missing`, or
  `teaching_empty`), `path`, `line`, `fingerprint`, `partial`,
  `config_fingerprint`, effective `categories[]` with `line`, `hotkey_source`,
  `rows`, `rule_refs`, `declared`, `provenance {entry, label, hotkey}`, and
  `usage {rows, rule_references}`; `accounts[]` include `line`,
  `label_explicit`, `rows`, `journal_events`, label provenance, and
  explicit/default row plus journal usage; `rules[]` (present even when empty),
  `stale_after {days, explicit, line, source_line}`,
  `free_hotkeys[]`, aggregate `usage`, `issues`, and
  `findings[] {code, severity, path, line, key_path, message, hint}`. It
  includes partial config fields on parse errors and `starter` guidance when
  the mapping is missing.
- `report` adds `period {kind, key, start, end, label, so_far, note}`,
  where `kind` is `day`, `week`, `month`, `year` (calendar, `-y` only),
  `quarter`, `iso_year`, or `all_time`. The `key` is the canonical id
  (`2026-Q4`, `2026`, `all_time`); `so_far` means the period holds
  today. It also adds `totals {out_cents, in_cents, net_cents,
  refund_cents, transfer_out_cents, transfer_in_cents, cash_out_cents,
  cash_traced_cents, unclassified_out_cents, unclassified_in_cents}`,
  `breakdown[] {key, emoji, label, out_cents, share, rows}`,
  `days[] {date, heading, path, line, out_cents, in_cents, rows,
  waiting, flagged}` (every day of the period, including empty ones;
  `-s` reports `"days": []` with totals intact),
  `waiting {period, all, oldest}` plus `flagged`,
  `config_fingerprint`,
  `coverage {tracked_since, imported_through, start, through, complete}`
  (dates are `YYYY-MM-DD` or `null`; `through` is always a date), and
  `navigation {waiting {path, line, date}, first {path, line, date},
  months[]}` (`waiting` is the oldest waiting row in the period,
  `first` the day heading of the earliest counted-money day, `months`
  the vault-relative month notes holding the period's rows; each target
  is `null` when absent), and `warnings[]`/`findings[]` carrying at most
  one bounded `bank_subscription_album_invalid` entry. With `-c` it also
  adds `charts`; see [Report charts](#report-charts).
- `check` adds `findings[] {code, severity, path, line, message, hint}`;
  configuration findings also include `key_path`. Subscription candidate
  findings also include `proposal {title, amount_cents, cadence, match,
  category}`, where `match` is a one-item string array or `null`, and category
  is a bank category key or `null`. An invalid subscription album adds one
  `bank_subscription_album_invalid` warning finding.
- `ensure` adds `{path, line, day, created_note, created_heading}`.
- `ack` prints the envelope `ok`, `dry_run`, `changed`,
  `changes[] {path, diff?}`, `findings`, `commit`, `sync`, and
  `warnings`, plus `acknowledged[]`, `already[]`, and `unknown[]`.
- `import` prints the envelope `ok`, `today`, `categories`, `accounts`,
  `issues`, `dry_run`, `changed`, `changes[] {path, diff?}`,
  `findings[] {code, path, line, message}`, `commit {sha, subject}`,
  `sync {pre, post}`, `restored`, `restored_rows[]`, and `warnings`, plus the parent-contract
  `files[] {file, account, source, start, end, new, matched, seen,
  ambiguous, before_start, out_cents, in_cents, balance_cents,
  balance_asof, status}` and `rows[]` using existing row JSON.
  `balance_cents`/`balance_asof` report the ledger balance and date.
  Diagnostics identify a file and field/record
  position without echoing private identifiers or raw data.

Shared objects:

```json
{
  "ok": true,
  "schema_version": 1,
  "today": "2026-11-14",
  "categories": [
    { "key": "groceries", "emoji": "🛒", "label": "Groceries", "builtin": false, "hotkey": "g", "group": null, "hidden": false }
  ],
  "accounts": [
    {
      "alias": "checking",
      "label": "WF Checking",
      "kind": "checking",
      "default": true,
      "start": "2026-11-01",
      "status": "balanced",
      "last_import": "2026-11-14",
      "balance_cents": 100000,
      "balance_asof": "2026-11-12",
      "delta_cents": 0,
      "hint": "Ledger balance matches through 2026-11-12"
    }
  ],
  "issues": { "errors": 0, "warnings": 0 },
  "findings": []
}
```

### Report charts

`report -c` adds `charts`: the period's money split into its Stats Strip
child periods, with coverage states and a folded category series, so a
client draws cashflow and category charts without adding cents itself.

- `-c` needs a week (`-w`, or `-p` with a week), a quarter, an ISO
  week-year, or all time. With `-D`, `-m`, `-y`, the default month, or a
  `-p` day, `report` exits 2 before reading rows (`ok: false` under
  `-f json`) with
  `-c/--charts needs a week, quarter, ISO year, or all-time period (-w or -p)`.
- Without `-c`, the output has no `charts` key and is otherwise identical.
  With `-c`, `-s` still reports `"days": []`, and the teaching empty state
  is unchanged.

```json
{
  "charts": {
    "version": 1,
    "granularity": "day",
    "buckets": [
      {
        "id": "2026-11-11", "kind": "day", "label": "Wed 11",
        "start": "2026-11-11", "end": "2026-11-11", "group": null,
        "so_far": false, "state": "awaiting_import",
        "in_cents": 0, "out_cents": 24977, "net_cents": -24977,
        "waiting": 7, "rows": 7,
        "top_out": [
          { "key": "groceries", "emoji": "🛒", "label": "Groceries", "out_cents": 14210 }
        ],
        "target": { "path": "bank/2026-11.md", "line": 12 }
      }
    ],
    "groups": [],
    "categories": {
      "limit": 5,
      "series": [
        { "key": "groceries", "emoji": "🛒", "label": "Groceries", "kind": "category",
          "out_cents": 14210, "share": 0.569, "rows": 3 }
      ],
      "rest": { "count": 2, "out_cents": 2320, "share": 0.093, "rows": 2 }
    }
  }
}
```

**Children.** The buckets are the period's Stats Strip columns, from the
same periods-calendar partition as `bob periods show`.

| Period | `granularity` | Buckets | `group` | `groups[]` |
| --- | --- | --- | --- | --- |
| ISO week | `day` | 7 days | `null` | `[]` |
| Quarter | `week` | 13 or 14 ISO weeks | `null` | `[]` |
| ISO week-year | `week` | 52 or 53 ISO weeks | `Q1`–`Q4` | The 4 quarters |
| All time | `quarter` | Quarters of the bank's history | ISO year | The ISO years |
| All time over 64 quarters | `year` | ISO years | `null` | `[]` |

- A bucket's `id` and `label` equal the matching `bob periods show`
  `columns[].id` and `columns[].caption` (`2026-11-11` and `Wed 11`,
  `2026-W46` and `W46`, `2026-Q4` and `Q4`, `2026` and `2026`). `kind` is
  `day`, `week`, `quarter`, or `year`. `start` and `end` are clipped to
  `period`.
- All time uses the report's own bank-history span (the `period` above),
  not the daily-note span. Past 64 ISO years, `charts` is `null` and the
  command still exits 0. An empty or reversed span, such as a history whose
  only account starts in the future, has `buckets: []` and `groups: []`.
- `groups[]` entries use the bucket shape with `group: null`; a group's
  `label` is the caption its buckets name in `group`.

**Money.** Each bucket and group applies the [Totals](#totals) rules to the
rows dated inside its span. `in_cents`, `out_cents`, and `net_cents`
(`in_cents − out_cents`) match how `totals` counts, so transfers,
cash-account rows, and uncategorized inflows stay outside In and Out.
`rows` counts every bank row in the span, and `waiting` counts its `[ ]`
rows. `top_out[]` holds up to three `{key, emoji, label, out_cents}`
breakdown entries for the span, largest first, ties broken by `key`.

**Targets.** A day bucket targets its day heading in the bank month note,
or is `null` when that heading does not exist. Week, quarter, and year
buckets and groups target line 1 of the period note (for example
`{"path": "2026/2026-W46.md", "line": 1}`), whether or not the note exists.

**States.** For a span `[s, e]`, with today `T`,
`coverage.tracked_since` `TS`, and `coverage.imported_through` `IT`, the
first matching rule wins. `so_far` is `s ≤ T ≤ e`.

| `state` | Rule |
| --- | --- |
| `future` | `s > T` |
| `untracked` | `TS` exists and `e < TS` |
| `awaiting_import` | `IT` exists and `s > IT` |
| `partial` | `TS` exists and `s < TS`, or `IT` exists and `IT < min(e, T)` |
| `tracked` | Otherwise |

A state never zeroes money: a bucket's money is whatever its rows say.

**Categories.** `categories` folds the period's `breakdown[]`.

1. Entries rank by `out_cents` descending, then `key`.
2. Nonzero `cash` and `unclassified` entries are pinned.
3. `series[]` holds the pinned entries plus the top `limit` (5) minus
   pinned other entries, in rank order. When exactly one other entry is
   left over, it joins `series` instead of becoming `1 more`.
4. Everything else folds into `rest {count, out_cents, share, rows}`,
   where `share` is its fraction of Out; `rest` is `null` when nothing
   folds.

Each series entry is `{key, emoji, label, kind, out_cents, share, rows}`,
with bob's breakdown `share`; `kind` is `category`, `cash`, or
`unclassified`.

**Invariants.**

- Bucket ids are unique, and there are at most 64 buckets.
- Buckets tile `[period.start, period.end]` exactly, with no gap or
  overlap; so do non-empty `groups[]`.
- Every bucket and group has `net_cents == in_cents − out_cents`.
- Buckets, and non-empty groups, sum to `totals.in_cents`,
  `totals.out_cents`, `waiting.period`, and the period's row count.
- `series[]` plus `rest` sum to `totals.out_cents`.

See [JSON outputs](reference/json-outputs.md#bank-reports).

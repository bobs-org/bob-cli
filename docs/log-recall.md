---
title: Log Recall
description: Vault-derived phrase history and the log recall behavior shared by Bob clients.
---

# Log Recall

Log Recall gives Schedule, Work, Rotten, and Cancel Log prompts a shared history
of human-written phrases. Each history is derived from the vault; Bob does not
keep a separate phrase database. `bob logs list` is the read-only catalog
interface used by Bob clients.

## Catalog sources

The scanner walks Markdown notes in the vault, including `done/`, and skips
dot-directories and Bob's standard excluded note directories. It reads only
direct child entries under these managed markers:

| Kind | Marker | `last_used` precision |
| --- | --- | --- |
| Schedule | `🗓️ **SCHEDULE LOG**` | `day-approx` |
| Work | `🛠️ **WORK LOG**` | minute |
| Rotten | `🥀 **ROTTEN LOG**` | day |
| Cancel | `❌ **CANCEL LOG**` | day |

An entry is `*STAMP* — BODY`, with one space on each side of the em dash. Fenced
code and detail sub-bullets are ignored. Work accepts `YYYY-MM-DD HH:MM` and
older day-only stamps. Rotten and Cancel use `YYYY-MM-DD`. Schedule accepts
`FROM → TO` or `TO`; its `last_used` is the earlier date on the stamp, capped at
today. Schedule recency is approximate because the stamp records schedule dates,
not the time the reason was entered.

Bob collapses body whitespace and trims both ends. Rotten entries remove only
the known trailing keep-count text (`· kept N×`, `· kept N× in|at <Rung>`, the
legacy `· kept in Next`, and `· stayed in|at <Rung> after N keep(s)`). Blank
phrases and phrases over 200 characters are excluded and counted. Duplicate
`(kind, stamp, key)` entries count once, including copies in different notes.

Machine-written text is excluded. A body starting with `🎲`, `🤷`, `🍂`, `🪢`,
`📬`, or `🔓` is machine text. Bob also filters the Google Calendar cancellation
reasons `Deleted in Google Calendar`, `Declined in Google Calendar`, and
`Moved to … in Google Calendar`, plus task-merge section-provenance reasons.
The Rust writer registry test exercises formatter samples and checks that known
managed-log writer call sites are registered.

Every `bob logs list` call walks and stats the vault. A per-note `(mtime_ns,
size)` cache keeps unchanged-note parsing cheap; there is no CLI snapshot TTL, so
a just-written entry appears on the next call. Keystroke-driven callers may reuse
a scan up to 30 seconds.

## `bob logs list`

`bob logs` runs `list`. The command accepts:

| Option | Meaning |
| --- | --- |
| `-b`, `--bob-dir DIR` | Vault root; defaults to `BOB_DIR` or `~/bob` |
| `-f`, `--format human\|json` | Output format; default `human` |
| `-l`, `--log KIND` | Select `schedule`, `work`, `rotten`, or `cancel`; repeatable; default all four |
| `-n`, `--limit N` | Maximum phrases per log; default 10 for human output and 200 for JSON |
| `-q`, `--query TEXT` | Filter and rank phrases by literal match |

JSON uses `schema_version: 1`. The response contains `ok`, the original query,
`half_life_days`, one object per selected log, and bounded warnings. Each log
reports its kind, label, emoji, `recency_precision`, total phrase count,
truncation flag, exclusion counts, and phrases. A phrase has `text`, normalized
`key`, unique `uses`, `last_used`, computed `score`, a zero-based `recency_rank`,
and `match` (`exact`, `prefix`, `word`, or `substring`) when a non-empty query is
provided. Empty catalogs succeed with no phrases. Vault and argument errors
return a nonzero exit; JSON mode returns `ok: false` and an `error` object.

## Matching and ranking

The phrase key is Unicode lowercase of NFC text with collapsed whitespace.
Clients normalize only the query; Bob returns the canonical key and the most
recent original spelling. Matching is literal, in this order:

1. Exact key.
2. Key prefix.
3. Word prefix: the query starts at a word boundary, where the preceding
   character is not alphanumeric and the matching character is alphanumeric.
4. Substring.

Match tiers sort before each other. Within a tier, the newest matching phrase is
pinned first. Remaining phrases sort by `score` descending, `recency_rank`
ascending, then key ascending. With an empty query, every phrase shares one tier
and the newest phrase is pinned first.

Bob computes frecency as the sum across deduplicated uses of
`0.5^(age_days / 7)`. `HALF_LIFE_DAYS` is 7.0. Day-precision log kinds use whole
day ages; Work uses its recorded minute precision. `recency_rank` is computed
across the whole kind catalog before applying the result limit. Shared ranking
vectors live at `tests/fixtures/log_recall/vectors.json` so the Rust, JavaScript,
and Swift implementations can stay aligned.

## Client behavior

Clients show the existing write preview first, followed by a `USED BEFORE` band.
The band filters from the field's preserved draft, not from a recalled value.
The initial field is unchanged and nothing is selected. Ctrl+N, Ctrl+P, and free
↑/↓ enter the band at its first row and fill the field. Later key presses move
through rows without wrapping. Moving above the first row or pressing Esc / Ctrl+[
restores the draft; a second Esc follows the host prompt's normal back-out path.
Typing, deleting, or pasting after a recall adopts the visible value as the new
draft. Enter always follows the host prompt's existing write path and writes
exactly the text visible in the field.

The band never claims Enter, Tab, IME composition, chords with Shift / Alt / Meta,
or Ctrl+N/P when no catalog is available. Hover does not select. Clicking a row
fills the field without committing or moving focus. A fill dispatches one input
event so the host preview follows the visible text. With no Bob, an unsupported
older Bob, malformed data, a timeout, or a mobile host, the band takes no space
and the prompt keeps its existing behavior.

Roomy surfaces reserve a stable height for up to five rows. Dense surfaces show
up to three rows after navigation begins or after two draft characters. The
header names the kind, shows the current filter or recall position, and provides
Ctrl+N/P hints when rows are available. Row metadata shows use count and age; task
titles never appear in the band.

After a successful Obsidian write, a per-session overlay can make the phrase
available immediately while the vault scan refreshes. An existing catalog phrase
keeps its score and count; a new overlay phrase starts at one use and score 1.0.
Merging a later scan is idempotent.

## Surfaces

| Surface | History |
| --- | --- |
| Obsidian Task Link Lane and Lane Release prompts | Work |
| Obsidian Work Summary prompt | Work |
| Task Card schedule-review reason | Schedule |
| Task Card schedule-review summary and schedule-work-log | Work |
| Task Card lane-release-reason | Work |
| Task Card cancel-reason | Cancel |
| Rotten keep and batch keep prompts | Rotten |
| Successor picker inline cancel reason | Cancel |
| Mac Capture pull-in cancel reason | Cancel |
| Mac inline Work Log completion | Work |

For inline Work Log completion, `capture-parse` marks only the entry summary
with `pomodoro_close_log_summary`. `capture-complete` returns context
`work_log` with a replacement range over that summary. It filters the full
summary draft and returns the top three Work phrases once it contains at least
two characters; shorter drafts return no candidates. Each candidate exposes
the phrase as both `replacement` and `text`, plus its normalized `key`, `uses`,
`last_used`, `recency_rank`, and optional `match` tier. The response sets
`initial_selection: "none"`, so accepting a phrase is always an explicit row
choice and replaces only the summary.

The typed-date `scheduled` stage and relation stages are excluded. Capture has
no Schedule or Rotten prompt. See [JSON outputs](reference/json-outputs.md#task-log-reports)
for the catalog response shape.

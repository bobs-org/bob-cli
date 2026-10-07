# Reference library (`bob ref find`, `bob ref list`)

`bob ref find` looks up URLs, arXiv IDs, DOIs, vault paths, note stems,
titles, and frontmatter ids in the reference library under `ref/`. It
answers "is this already in my library?" with a verdict per query, in
human, Markdown, or versioned JSON output. `bob ref list` renders
filtered library views, defaulting to the reading queue (queued and
started notes). Both verbs never write to the vault (the one exception
is `find -i`, which reads intake PDF markers).

## Coverage

Only notes under `<ref-dir>/**/*.md` are indexed (default `ref/`).
Skipped: hidden directories, `*.assets/` directories, and conflict
copies. A note that cannot be read or parsed still yields a row with a
diagnostic; nothing is silently dropped.

Absence from this index is never proof of not having read something:
zorg-era reading records elsewhere in the vault are not indexed
(`bob ref doctor` counts them), and annotations are the snapshot written
by the last Highlights scan, not live PDF state.

## Reading state

Reading state is derived, never stored. The effective `status` is decided
per note:

1. Exactly one `^ref` tracker with a known mark maps through the
   checkbox marks (`[ ]` ready, `[*]` next, `[/]` wip, `[x]` read,
   `[-]` abandoned). Against the frontmatter `status` (with
   `unread`/`done` normalized) and the stored `highlights_marker_base`:
   agreement is `ok`; a tracker move with unchanged frontmatter is
   `pending` (the tracker wins); both sides moving is `conflict`.
2. No usable tracker (none, several, or an unknown mark): the normalized
   frontmatter `status`, plus a diagnostic for several trackers or an
   unknown mark.
3. Neither: `status: null` and `unknown` reading state.

| `reading_state` | From modern status | From `legacy_status`        |
| --------------- | ------------------ | --------------------------- |
| `queued`        | `ready`, `next`    | `unread`                    |
| `started`       | `wip`              | `collect_fleeting_notes`    |
| `finished`      | `read`             | `read`, `review_fleeting_notes`, `review_lit_notes` |
| `dropped`       | `abandoned`        | `abandoned`                 |
| `unknown`       | none, or `conflict`| `book`, missing, unrecognized |

`reading_state_source` names the evidence (`ref_task:[x]`,
`frontmatter:read`, `legacy_status:review_lit_notes`,
`conflict:…`, `none`).

## Identity

Stored values come from `source_url` then `url` (scalars or lists).
HTTP(S) values key on the cleaned dedupe key (already arXiv-aware),
plus `arxiv:<id>` for arXiv URLs (including `10.48550/arXiv.<id>` DOIs)
and `doi:<doi>` for `doi.org` URLs. Values that fail validation use
opaque `raw:` keys and earn an `opaque_url` diagnostic.

Queries classify in order: URL, arXiv ID (bare or `arxiv:`-prefixed),
DOI (bare or `doi:`-prefixed), path (contains `/` or ends in
`.md`/`.pdf`), single token (note stem or frontmatter `id`), anything
else (title). Title scoring is Dice over normalized token sets (0-100,
normalized equality is 100 with kind `title_exact`); URL misses fall
back to scoring their slug words as `slug_title`. At most 5 candidates
are kept.

When a PDF-backed note shares an identity key with a note without a
PDF, the PDF-less note is `superseded_by` the PDF-backed one. `find`
still reaches superseded notes and prints them as `also` companions.

## Usage

```bash
bob ref find https://arxiv.org/abs/1706.03762
printf '%s\n' URL1 URL2 | bob ref find - -f json
bob ref find 1706.03762 -i
```

Options: `-b/--bob-dir`, `-f/--format human|json|markdown` (default
human), `-i/--include-intake`, `-m/--min-score 1-100` (default 60),
`-r/--ref-dir`, `-x/--xlib-dir`. Queries run in order with duplicates
kept; `-` reads one query per line from stdin.

Verdicts per query: `in_library` (an exact match), `in_intake` (only a
queued intake PDF matched, with `-i`), `possible` (only title
candidates at or above `--min-score`), `not_found` (nothing matched).
A title match is only ever a candidate, never proof; `not found` means
only "not under ref/".

## `bob ref list`

```bash
bob ref list
bob ref list -R finished -S 30d -g
bob ref list -o external -R finished -f json
bob ref list -s legacy -R queued
```

Options: `-A/--all`, `-b/--bob-dir`, `-f/--format human|json|markdown`
(default human), `-g/--git-dates`, `-n/--limit N` (default 50),
`-o/--origin external|agent-report`, `-P/--parent NOTE`,
`-R/--reading-state` (comma-separated, or `all`), `-r/--ref-dir`,
`-S/--since DATE`, `-s/--status` (comma-separated),
`-t/--ref-type` (comma-separated). `-A` conflicts with `-n`.

With no filter option at all, `list` shows the reading queue (`queued`
and `started` notes) in every format. Any filter option searches every
reading state unless `-R` narrows it. Values within one option are ORed;
different options are ANDed. Superseded notes are always excluded and
counted in `hidden.superseded` (`find` and `show` still reach them).

The row date is `finished` for the finished and dropped states, `added`
otherwise. `--since` accepts `YYYY-MM-DD` or `<N>d|w|m|y` relative to
today (calendar months); rows without a date are excluded and counted in
`filters.undated_excluded`. Rows order by reading state (started,
queued, finished, dropped, unknown), modern before legacy, `next` before
`ready` within queued, row date newest first with undated last, then
title. `--limit` applies after filtering and ordering.

`-g/--git-dates` fills a missing `added` or `finished` date from one
`git log` pass over `^ref` tracker lines only on modern notes, with
source `git`. When Git is missing, the vault is not a repo, or the pass
fails, `coverage.git_dates` is `"unavailable"` with one stderr warning;
this is never fatal.

Human output groups rows under one heading per reading state with its
count (pre-cap totals), with the status chip, row date (or `—`), title,
`♫` when audio is bound, type, and a dim path; the path drops first on
narrow terminals, then titles truncate. A pending-sync `*` gets a
footnote. Without `-s`, legacy-era rows collapse to one dim summary line
instead of listing. The cap applies to the rows that are listed;
truncation prints `… N more · -n N or -A to show more`. An empty default
view prints `Nothing queued ✓`.

Markdown output is a table with the columns State | Status | Date |
Title | Type | Note, followed by the matched/returned and coverage lines.
The JSON envelope carries `filters` (the effective `reading_state` plus
`reading_state_defaulted`, every active filter, the `limit`, and
`undated_excluded`), `matched`, `returned`, `truncated`, and
`hidden.superseded` alongside the shared `coverage` and `library`
counts.

## JSON envelope

`REF_SCHEMA_VERSION` is 1. Compact one-line JSON on stdout, no ANSI:

```json
{"ok":true,"schema_version":1,"command":"ref find","generated_at":"2026-10-06T12:00:00",
 "coverage":{"ref_dir":"ref","notes":32,"skipped":3,"intake":"not_checked",
   "scope":"Only notes under ref/ are indexed. …",
   "annotations":"Annotations are the snapshot written by the last Highlights scan, not live PDF state."},
 "library":{"notes":31,"finished":10,"started":2,"queued":12,"dropped":3,"unknown":4},
 "summary":{"queries":1,"in_library":1,"finished":1,"in_intake":0,"possible":0,"not_found":0},
 "results":[{"query":"https://example.com/blog/capture-flows","query_kind":"url",
   "keys":["https://example.com/blog/capture-flows"],
   "verdict":"in_library","reading_state":"finished",
   "matches":[{"match_kind":"identity","matched_key":"https://example.com/blog/capture-flows","ref":{}}],
   "candidates":[],
   "intake":[]}]}
```

`coverage.intake` is `not_checked`, `checked`, or `unavailable` (no
intake directory). `library` counts exclude superseded notes.
`generated_at` is local `YYYY-MM-DDTHH:MM:SS` (pin with `BOB_NOW`).
Each `ref` object is the full index row: path, link, title, origin,
ref_type, era, status, status_sync, legacy_status, reading_state and
its source, parent, urls, identity keys, author/published/captured,
added/finished dates and sources, source_pdf, audio, annotation and
comment counts, snapshot, research_ref, superseded_by, and diagnostics.

## Errors and exit codes

Exit 0 whenever the lookup ran, including `not_found`; exit 1 for
vault failures (a missing ref dir) with
`bob ref: error: reference directory not found: <path>` plus the hint
`pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR`; exit 2 for usage
errors. In JSON mode, failures print
`{"ok":false,"schema_version":1,"command":"ref find","error":{"code":"…","message":"…","hint":"…"}}`
to stdout and exit 1. A missing intake directory under `-i` is not a
failure: coverage reports `unavailable` and the lookup continues.

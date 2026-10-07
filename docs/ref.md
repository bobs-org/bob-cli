# Reference library (`bob ref find`, `bob ref list`, `bob ref show`)

`bob ref find` looks up URLs, arXiv IDs, DOIs, vault paths, note stems,
titles, and frontmatter ids in the reference library under `ref/`. It
answers "is this already in my library?" with a verdict per query, in
human, Markdown, or versioned JSON output. `bob ref list` renders
filtered library views, defaulting to the reading queue (queued and
started notes). `bob ref show` resolves one or more exact references
into their metadata, annotations, notes, and tasks. All three verbs
never write to the vault (the one exception is `find -i`, which reads
intake PDF markers).

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
else (title). URL queries derive the same stored-side keys (the dedupe
key plus `doi:`/`arxiv:` through the same DOI helpers, so a
percent-encoded DOI URL matches too); bare and `doi:`-prefixed DOI
queries derive the `doi:` key plus the `arxiv:` key for arXiv DOIs, so
one paper's twin notes both match in primary-match order. Title scoring
is Dice over normalized token sets (0-100, normalized equality is 100
with kind `title_exact`); URL misses fall back to scoring their slug
words as `slug_title`. At most 5 candidates are kept.

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
narrow terminals, then titles truncate. A pending-sync `*` (dimmed) gets
a footnote. Without `-s`, legacy-era rows collapse to one dim summary line
instead of listing. The cap applies to the rows that are listed;
truncation prints `… N more · -n N or -A to show more`. An empty default
view prints `Nothing queued ✓`.

Markdown output is a table with the columns State | Status | Date |
Title | Type | Note (six header cells with a six-cell separator row),
followed by a blank line and the matched/returned and coverage lines.
The JSON envelope carries `filters` (the effective `reading_state` plus
`reading_state_defaulted`, every active filter, the `limit`, and
`undated_excluded`), `matched`, `returned`, `truncated`, and
`hidden.superseded` alongside the shared `coverage` and `library`
counts.

## `bob ref show`

```bash
bob ref show ea_graph
bob ref show ea_graph -c
bob ref show ref/papers/ea_graph.md -f json
```

Options: `-b/--bob-dir`, `-c/--comments-only`, `-f/--format
human|json|markdown` (default human), `-N/--no-annotations`, `-r/--ref-dir`.
`-c` conflicts with `-N`.

Each `REF` is a vault path, note stem or id, URL, arXiv ID, DOI, or
exact title. Resolution is exact (the exact match kinds plus a unique
`title_exact`): several matches collapse to the one note that is not
superseded, with the superseded companions listed under `also` (a dim
`also: <path> (superseded legacy note)` line in human output); any other
multi-match fails as `ambiguous reference` and names its candidates (it
never picks the first stem); a miss fails as `no reference note matches
<REF>` with up to three title candidates as `hint:` lines. Every `REF`
resolves before anything prints, so one failure exits 1 and prints
nothing else.

Each shown row extends the base index row with the note's content:

- `annotations_status`: `parsed`, `absent` (no managed region, as on
  legacy notes), or `unparsed` (with `raw_region`).
- `annotations`: one entry per live block — `page_label`, `kind`
  (`highlight`, `note`, or `image`), `quote`, `comment`, `asset`,
  `block_id`, and `link` (`[[ref/x#^h-…]]`). Quote and comment stay in
  separate fields; standalone notes carry their text as `comment`.
- `excluded`: counts of `marker_mirrors`, `preamble`, and `removed`
  (tombstoned) blocks. Tombstones carry no text, so they are counted
  instead of shown.
- `own_notes`: the user's own text (a legacy note's whole migrated
  body).
- `tasks`: the parsed `## Tasks` lines (`checked`, `mark`, `text`,
  `block_id`).
- `also`: superseded companion paths.

`-c/--comments-only` keeps only annotations with a comment and
standalone notes, with their quotes. `-N/--no-annotations` keeps
metadata and notes only. The `annotations` array honors the flags; the
status, counts, and `excluded` facts always describe the whole region.

Human output prints the title, a reading-state header (chip, date,
type, origin, path), the metadata rows that have a value, then the
annotations grouped by page with wrapped quotes (`“…”`) and cyan `↳`
comments, the exclusion parenthetical, and the `NOTES` and `TASKS`
sections. Each `TASKS` row carries its linked annotation's page label
as a dim suffix. Empty sections stay omitted; several `REF`s are
separated by a dim rule.

Markdown output is a quotable digest per note: `## <title>`, bullets for
the note link, reading state with evidence and date, source URL and
arXiv/DOI, origin and type, annotation counts and snapshot date (plus
`Research report: research:…` when set, and `Also: …` companions), then
`### Annotations` (one `**Page N**` label per page, quotes as `>`
blockquotes, comments as `Comment: …` lines, standalone notes as
`Note: …`), `### Notes`, and `### Tasks` (keeping each task's real
mark, so a `[-]` task stays `[-]`).

Agent usage: resolve candidates with `bob ref find` first, then read
exact notes with `bob ref show <path>`. Prefer `-c -f markdown` for
Bryan's own thoughts on a reference, and never claim a `not_found`
result means he has not read something — it only means it is not under
`ref/`.

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
`show` resolution failures (`ambiguous_reference`,
`unknown_reference`) exit 1 as well; in JSON mode they add
`"candidates":[{"path":…,"title":…}]` to the envelope.

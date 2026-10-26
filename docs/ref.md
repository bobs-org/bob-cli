# Reference library (`bob ref`)

`bob ref find` looks up URLs, arXiv IDs, DOIs, vault paths, note stems,
titles, and frontmatter ids in the reference library under `ref/`. It
answers "is this already in my library?" with a verdict per query, in
human, Markdown, or versioned JSON output. `bob ref list` renders
filtered library views, defaulting to the reading queue (queued and
started notes). `bob ref show` resolves one or more exact references
into their metadata, annotations, notes, and tasks. `bob ref code` reads
the saved Markdown source and exact render-time code blocks without
rerendering. `bob ref today`
links a reference's v2 reading task under today's Pomodoro. `find`,
`list`, `show`, and `code` are read-only; `find -i` also reads intake PDF
markers. `bob ref today --dry-run` plans without writing. `bob ref rename` renames
an unannotated modern reference's files and updates links (preview with `-d`).
`bob ref migrate-zorg`
plans copies of zorg-era `status::` reading records into legacy notes under
`ref/zorg/`. It is a dry run by default; `-w/--write` applies the plan.

## Choose a command

| Goal | Command | Effect |
| --- | --- | --- |
| Check whether a reference is known | `bob ref find <URL-or-identifier>` | Read library notes; `--include-intake` also checks queued PDFs |
| See what remains to read | `bob ref list` | Read queued and started reference notes |
| Read annotations and your notes | `bob ref show <note-path>` | Read one exact reference; `-c` selects commented annotations |
| Copy or inspect a saved code block | `bob ref code <REF> [N]` | Read exact block bytes, a block catalog, or the saved Markdown source |
| Read a reference today | `bob ref today <REF> [-p NAME]` | Set its reading task Next and link it under today's Pomodoro |
| Fix a reference's file name | `bob ref rename <REF> <NAME> -d`, then without `-d` | Move the note, PDF, and companions, update links, and make the reference id follow its stem while keeping the reading task |
| Import a URL, local PDF, or Markdown file now | `bob ref create <TARGET> -P <parent>` | File the PDF, note, and reading task inline; use `-k` to queue it in `xlib/` |
| Save a bare public link quickly | `bob capture '<URL>'` | Queue a background job when the routing policy admits the URL; the job files the note when it runs, and a queued clip appears in `ref list` after the host's intake drain (`bob ref scan -i`) |
| Inspect or process background imports | `bob ref jobs`, `bob ref jobs run` | List jobs, or fetch pending links; see [ref jobs](ref-jobs.md) |
| Drain queued PDFs | `bob ref scan -i` (`--intake-only`) | File only the PDFs and late audio queued in `xlib/` (pull, note, reading task, one commit each, push); exits 1 while anything stays queued |
| Repair references or sync annotations | `bob ref scan --dry-run`, then `bob ref scan` | Preview, then process queued PDFs and sync notes/annotations |
| Diagnose prerequisites and library health | `bob ref doctor` | Check paths, tools, routing policy, jobs, index diagnostics, and the write-once `ref sources` store |
| Bring old reading records into the index | `bob ref migrate-zorg`, then `--write` | Preview, then create and commit legacy notes; see [migration](#migrating-zorg-era-records-bob-ref-migrate-zorg) |

When `bob ref create` records a reference `id`, the id matches the final output
basename, which is also the reference note stem. Default targets add `_2`,
`_3`, and so on when that candidate is occupied by a file or sidecar under
`xlib/`, `lib/`, or `ref/`, or by an id already recorded anywhere under
`ref/`. Id collisions are compared case-insensitively. With `--output`, Bob
keeps the exact path and derives the id from its basename; if that id is
already used under `ref/`, create refuses and asks for a different output
filename. Existing references are left unchanged; use `bob ref rename` to
repair an existing id/stem mismatch.

`bob ref create --dry-run` skips installing the final PDF and audio. A local
PDF or Markdown file stays local: Markdown returns before pandoc runs. A PDF
URL is downloaded, and an arXiv target downloads both the metadata and the
PDF, before the dry-run report. A web article, including `--html`, launches
the browser and loads the page, then skips image localization and the PDF
write. For an offline link preview, use `bob capture --dry-run '<URL>'`.

The reading queue is derived from notes under `ref/`. A pending capture job or
an intake PDF under `xlib/` does not appear in `ref list` until a scan creates
its note; successful `bob ref create` calls file the note inline by default.
`bob ref scan -f json` reports the same scan as a versioned JSON
envelope (see [JSON output](highlights-ref-sync.md#json-output)); its client
is the Refs panel in Bob Mac Capture, which runs it when Bryan presses ⌘S
there or when Bob Mac Capture observes ⌘S in Highlights after a library PDF
finishes saving.
In Obsidian, `Cmd+Enter` on a reading task (or a Task Link or managed embed pointing at one) opens its `source_pdf` in Highlights through bob-navigation-hotkeys, without writing the vault.
Lookup commands require the configured reference directory to exist;
a missing directory is an error rather than an empty library. Directory
overrides are described in the [pipeline guide](highlights-ref-sync.md#default-paths).

## Coverage

Only notes under `<ref-dir>/**/*.md` are indexed (default `ref/`).
Skipped: hidden directories, `*.assets/` directories, and conflict
copies. A note that cannot be read or parsed still yields a row with a
diagnostic; nothing is silently dropped.

Absence from this index is never proof of not having read something:
zorg-era `status::` reading records outside `ref/` are not indexed
until `bob ref migrate-zorg --write` copies them into `ref/zorg/` (`bob ref
doctor` counts any that remain), and annotations are the snapshot
written by the last Highlights scan, not live PDF state.

`bob ref doctor` also prints a `ref sources` row for the write-once
store under `.bob/ref-sources/`. `ok (N saved · M code blocks)` means
every `source_markdown` pointer verified; `warn` lists dangling
pointers, digest mismatches, missing catalogs, orphans, and ignored
`code.json` files, with `bob ref attach-source` and
[Reference sources](vault-git-sync.md#reference-sources) as hints.
Frozen notes under `ref_history/` keep their pointer, so a store the
archive still names is not an orphan. Doctor never writes.

## Reading state

Reading state is derived, never stored. Current-format (v2) reading tasks
are `#task #ref` lines linking to the reference note; older (v1) trackers
live inside that note and end in `^ref`. One open v2 task outside `done/`
wins; several make Bob fall back to frontmatter. With none, Bob tries the
newest closed v2 task, then in-note v1 trackers, then frontmatter. A selected
v2 task supplies the row parent from its location: the area/project/inbox
stem, or the `done/` archive's `parent:`. The effective `status` is decided
per note:

1. One selected reading task with a known mark maps through the
   checkbox marks (`[ ]` ready, `[*]` next, `[/]` wip, `[x]` read,
   `[-]` abandoned). A single `[?]` tracker defers to the frontmatter
   `status`: with an open frontmatter status the tracker agrees (source
   `ref_task:[?]`, no diagnostic); with a terminal frontmatter status it
   targets `ready`. Against the frontmatter `status` (with
   `unread`/`done` normalized) and the stored `highlights_marker_base`:
   agreement is `ok`; a tracker move with unchanged frontmatter is
   `pending` (the tracker wins); both sides moving is `conflict`.
2. No usable tracker (none, several, an unknown mark, or `[?]` without a
   usable frontmatter status): the normalized frontmatter `status`, plus
   a diagnostic for several trackers or an unknown mark (`unknown_ref_mark`
   now covers other custom marks only).
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

`blocked` is a boolean overlay on the reading lane, always present on
every `list`, `show`, and `find` row: true when the reading task selected
by the rules above is Blocked `[?]`, false otherwise.
The boolean is separate from `status` and `reading_state` — a blocked
`next` row still reads `queued` in its lane — it only says the note is
waiting on something else. The Obsidian Reading Board reads Blocked from
this field and never parses `reading_state_source`. Bob Mac Capture's Refs
panel reads the row state, but its current inspector does not yet show the
blocked reasons or task priority/schedule fields.

`blocked_by` follows `blocked` on every row. It is always present, and
it is `[]` when `blocked` is false or no reason remains on the selected
reading-task line (for example a `[scheduled::]` date that is today).
Reasons are read from that line only and never run reconcile. They
appear in `effective_block_reasons` order:

- `local_dependency` lists, in `[dependsOn::]` order, every id outside
  inline code that names an open task. Open means some task outside
  `done/` carries `[id:: <id>]` and has an open Tasks status type.
  Closed, archived-only, and unresolved ids are omitted, and the entry
  itself is omitted when none qualify. Each prerequisite carries `id`,
  `title`, `path`, `block_id`, `link`, `mark`, and `ref_path` (the
  indexed reference path when the prerequisite is itself a reading
  task, else `null`).
- `local_schedule` appears when `scheduled` is strictly after today
  (`env::current_datetime()`, so `BOB_NOW` pins it).
- `project` appears when the line carries the exact `#prj_is_blocked`
  tag outside inline code. `project` is the reading task's residence
  note path; `link` is that note's wikilink.

```json
"blocked": true,
"blocked_by": [
  {
    "kind": "local_dependency",
    "prerequisites": [
      {
        "id": "sase__ref-prereq-open",
        "title": "Open Prerequisite Paper",
        "path": "sase.md",
        "block_id": "ref-prereq-open",
        "link": "[[sase#^ref-prereq-open]]",
        "mark": "*",
        "ref_path": "ref/papers/prereq_open.md"
      }
    ]
  },
  { "kind": "local_schedule", "date": "2026-10-23" },
  { "kind": "project", "project": "sase_post_blog.md", "link": "[[sase_post_blog]]" }
]
```

A legacy note whose `legacy_status` is `book` and that carries a
non-empty `legacy_chapter_statuses` list derives its reading state
from its chapters instead: each chapter status maps through the
`legacy_status` column above (unknown and dropped chapters set
aside), then finished wins when every remaining chapter is finished,
started when any remaining chapter is started or finished, queued
when chapters still remain, dropped when any chapter was dropped,
and unknown otherwise. Its `reading_state_source` is
`legacy_chapters:<finished>/<total>`; a book without chapters stays
`unknown`.

## Identity

Stored values come from `source_url` then `url` (scalars or lists).
HTTP(S) values key on the shared
[URL dedupe key](highlights-clip.md#marker-fields-and-dedupe) (already arXiv-aware),
plus `arxiv:<id>` for arXiv URLs (including `10.48550/arXiv.<id>` DOIs)
and `doi:<doi>` for `doi.org` URLs. Values that fail validation use
opaque `raw:` keys and earn an `opaque_url` diagnostic.

Path-escape normalization applies to both stored URLs and lookup queries:
`%7e` matches `~`, while reserved escapes stay distinct from their literal
characters. The sole `@` alias is the leading `/%40` on `medium.com`
(including `www.medium.com`), which matches `/@`. Other hosts, Medium
subdomains, and later `@` characters do not receive that alias. Matching
does not rewrite the note's stored URLs.

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

A reference with no rereads is a family of one. Every explicit reread
— single-source (vault note, Markdown, local PDF, PDF URL, arXiv, web
article) or exact-URL-set collection — is named `<root>__vN` from the
family root: the second reading is `<root>__v2`, the third `<root>__v3`.
Rereads link their lineage through a note-local `earlier:` list naming
every live family member, newest first:

```yaml
earlier:
  - "[[ref/papers/attention_is_all_you_need__v2|v2 · 2026-09-01]]"
  - "[[ref/papers/attention|v1 · 2026-03-02]]"
```

The reading number is 1 plus the number of `earlier:` entries whose
target is under `ref/` — computed from frontmatter, never parsed from a
filename, so a stem that merely looks versioned (such as
`protocol__v2`) is reading 1. Several readings sharing one exact
collection URL set are one family, named `<root>__vN` from the family
root; unrelated notes sharing keys across families stay ambiguous. The `earlier:` list is written once at
birth (block style, Obsidian's canonical Properties form; inline reads
too); the index only reads it, never re-derives it. Every PDF-backed
row sharing an identity key that sits in one family earns no
`duplicate_identity` diagnostic; a key shared outside one family still
warns, naming only the cross-family pairs, and a PDF-less note's
`superseded_by` winner is the newest family reading. An `earlier:`
entry whose target resolves nowhere warns as `earlier_dangling` (a
deleted middle reading keeps the surviving numbers).

Import deduplication and library lookup both index every URL in scalar or
list-valued `source_url` and `url` metadata. This applies to ref notes and
queued intake markers, so any URL in a collection can prevent a duplicate
import and can be found with `ref find -i`.

For a multi-URL `bob ref create`, the marker and ref note store
`source_url` as an ordered list in the order the URLs were supplied. The
collection yields one PDF, one note, and one reading task inline by default.
Every listed URL resolves to that same reference.

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
only "not under ref/". Within one reading family the newest reading is
primary, ahead of reading state, so an identity query's first match is
the newest reading; `find` still lists every copy, with family
companions on dim `also` lines (`also <path> (<era>, earlier reading v1
· <status>)`).

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

`-P/--parent NOTE` resolves through area/project routes and
`project_name_aliases` (`bob-cli` matches `bob`); any other name matches
the frontmatter parent literally (ASCII case-insensitive, never an
error). `filters.parent` echoes the canonical route when the input
resolved, else the normalized input.

The row date is `finished` for the finished and dropped states, `added`
otherwise. `--since` accepts `YYYY-MM-DD` or `<N>d|w|m|y` relative to
today (calendar months); rows without a date are excluded and counted in
`filters.undated_excluded`. Rows order by reading state (started,
queued, finished, dropped, unknown), modern before legacy, `next` before
`ready` within queued, row date newest first with undated last, then
title. `--limit` applies after filtering and ordering.

The `added` date uses this precedence: frontmatter `captured`, the date
part of frontmatter `created`, a zorg `source_block` date, then (with
`-g`) the Git birth date. `added_source` is `captured`, `created`,
`zorg_block`, or `git`. `added_at` is the most precise instant known on
that `added` day: a full offset-bearing `created` value supplies it when
its own-offset date matches `added`, including when `captured` wins the
date precedence. Date-only `created` values, mismatched dates, and zorg
dates have no `added_at`.

`-g/--git-dates` uses a rename-following file-birth pass to fill missing
`added` and `added_at` for every modern row, including v2 rows. A separate
`^ref` tracker pass fills missing `finished` dates for v1 rows only; it
never backfills a v2 row's `finished`. If Git is missing, the vault is not
a repo, or either pass fails, `coverage.git_dates` is `"unavailable"`
with one stderr warning; this is never fatal. Git birth follows file
renames and reports the oldest commit that added the note.

Human output groups rows under one heading per reading state with its
count (pre-cap totals), with the status chip, row date (or `—`), title
(plus a `vN` reading chip for readings past the first, and a `v1` chip
on a root that has later readings), `♫` when audio is bound, type, an
optional dim blocked reason, and a dim
path. The path drops first on a narrow terminal, then the reason, then
titles truncate. A blocked row's reason is one of `💤 until YYYY-MM-DD`,
`⛓ after <title>` (or `+N` more), `🧱 <project stem> blocked`, or
`⏸ blocked` when no reason remains; several reasons join with ` · `.
A pending-sync `*` (dimmed) gets a footnote. Without `-s`, legacy-era
rows collapse to one dim summary line,
`+ N zorg-era legacy notes hidden (…) · show them with -s legacy`, instead
of listing. Any `-s` value turns that collapse off and also filters to the
named statuses, so `-s legacy` is the view that lists those notes. JSON and
Markdown include the matching rows either way. The cap applies to the rows
that are listed;
truncation prints `… N more · -n N or -A to show more`. An empty default
view prints `Nothing queued ✓`. After `just install` on apollo,
`bob ref list -s ready,next,wip -A -f json` measured 0.32 s warm on the
live vault (five runs 0.30–0.35 s with the vault cache warm; 0.78 s
cold).

Markdown output is a table with the columns State | Status | Date |
Title | Type | Note (six header cells with a six-cell separator row),
followed by a blank line and the matched/returned and coverage lines.
The JSON envelope carries `filters` (the effective `reading_state` plus
`reading_state_defaulted`, every active filter, the `limit`, and
`undated_excluded`), `matched`, `returned`, `truncated`, and
`hidden.superseded` alongside the shared `coverage` and `library`
counts. Every row includes `marker_id`, the frontmatter `id`, or `null`
when that id is absent. Each `task` object also carries `priority`
(`highest | high | medium | low | lowest`, else `null`) and `scheduled`
(a strict calendar date, past or future, else `null`), both read from
the selected reading-task line. The Obsidian [Reading Board](#reading-board-in-obsidian)
consumes `blocked_by`, `task.priority`, and `task.scheduled` to draw these
rows as lanes. Bob Mac Capture's Refs panel currently ignores those keys;
inspector support is tracked separately in `bob-cli-9f`.

## Reading Board in Obsidian

`dash_references` opens on a read-only kanban of the reading queue. It is a
` ```bob-reading-board ` fence that Bob Ledger Tools draws from
`bob ref list -s ready,next,wip -A -f json` (plus `-P` and `-t` when set), so
the board, the terminal, and Bob Mac Capture agree on what can be read now.
The board never writes: acting on a reference stays with `bob ref today` and
the `^ref` task keymaps.

The fence body is optional YAML:

- `parent: <note>` passes `-P`, so the board shows one area or project's
  queue (routes and `project_name_aliases` apply). The block works on any
  note, for example a project note with `parent: sase`.
- `type: <type or list>` passes `-t`.
- Unknown keys and bad values become one muted lint line each, never an
  error.

**Open Reading Board** (`open-reading-board`) opens `dash_references`.

### Lanes

Lanes come from each row's `status` and `blocked` fields; only modern rows
with status `wip`, `next`, or `ready` appear.

| Lane       | Rows                 |
| ---------- | -------------------- |
| 📖 Reading | `wip`, not blocked   |
| ⭐ Next    | `next`, not blocked  |
| 📥 Ready   | `ready`, not blocked |
| 💤 Snoozed | `blocked`, any status  |

The header reads `N awake · M snoozed` (awake is Reading + Next + Ready), the
data time, and a ↻ refresh button. An empty board says `Nothing queued ✓`.
The four lanes sit in a row on wide panes, two by two at readable line width,
and in one column below that.

Awake cards show the title, a kind and topic line (kind from the `refs.base`
category vocabulary, topic from `parent`), the added date, `♫` for audio, `✎ N`
for annotations, and the task priority mark. A `TODAY` pill appears only when
the reading task is linked under today's Pomodoros and the Today cache is
ready. A dim `*` marks a pending frontmatter sync. Cards linked today sort
first; otherwise the order is `bob ref list` order.

### Snoozed reasons

Snoozed rows are compact and grouped, in this order:

1. **Waking**: `blocked_by` is empty; the next reconcile returns the
   reference to its lane (`⏰ now`).
2. **This week**, **This month**, **Later**: the only reason is a schedule,
   sorted by wake date (the same buckets as the note snapshot's Scheduled
   lane).
3. **Waiting on**: everything else, with a second line `⛓ after <first
   prerequisite>` (`+N` for more) or `🧱 in blocked project <stem>`. Hovering
   a row outlines the prerequisite card when it is on the board.

Rows whose underlying status is not `ready` show where they return
(`→ ⭐ Next`, `→ 📖 Reading`). The lane header toggles collapse for the
session. These are the same reasons the human `bob ref list` output prints.

### Keys and states

| Gesture                        | Effect                                                    |
| ------------------------------ | --------------------------------------------------------- |
| Click / Enter                  | Open the reference note                                   |
| ⌘/Ctrl-click, middle-click     | Open it in a new tab                                      |
| ⌘/Ctrl+Enter, **PDF** button   | Open the PDF in Highlights (new tab for the note without a PDF or off macOS) |
| ⌥/Alt+Enter, **Task** button   | Go to the reading task                                    |
| ←/→, ↑/↓ (h/l, k/j), Home/End  | Move focus across and within lanes                        |
| ↻ or `r`                       | Refresh now                                               |

The first paint shows skeleton lanes. Background revalidation is silent and
repaints only when the rows change. If `bob ref list` fails, the last frame
stays with a muted "Showing the board from HH:MM" line and Retry; with no
frame the board shows an actionable error. When `bob` is not found, set
`stats.bob_path` in `~/.config/bob/config.yml`, as for the Stats page.
Under an older bob without `blocked_by`, blocked rows sit in Waiting on as
`⏸ blocked` with one hint to update bob-cli. The board needs the desktop app.

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
superseded, or to the newest reading when every live match is in one
reading family, with the companions listed under `also` (a dim
`also: <path> (…)` line in human output — `earlier reading v1 ·
<status>` for family companions, `superseded legacy note` for
superseded ones); any other
multi-match fails as `ambiguous reference` and names its candidates (it
never picks the first stem); a miss fails as `no reference note matches
<REF>` with up to three title candidates as `hint:` lines. Every `REF`
resolves before anything prints, so one failure exits 1 and prints
nothing else.

Each shown row extends the base index row with the note's content:

The base row's `marker_id` is the note's frontmatter `id`, or `null` when
it is absent.

- `annotations_status`: `parsed`, `absent` (no managed region, as on
  legacy notes), or `unparsed` (with `raw_region`).
- `annotations`: one entry per live block — `page_label`, `kind`
  (`highlight`, `note`, or `image`), `quote`, `comment`, `asset`,
  `images`, `unresolved_images`, `pieces`, `block_id`, `link`
  (`[[ref/x#^h-…]]`), and `section` (the section `id`, omitted for
  ungrouped blocks). Quote and comment stay in separate fields;
  standalone notes carry their text as `comment`. `images` is the list of
  vault-relative image-note paths (without any `|width` hint);
  `unresolved_images` is the list of source texts from warning callouts.
  Both arrays are always present. `pieces` is present only on `<` groups:
  one entry per tucked piece, head first, each with its own `page_label`,
  `quote`, `comment`, `images`, and `unresolved_images`. The top-level
  `quote` / `comment` / `images` / `unresolved_images` keep the flattened
  group view (piece quotes joined with a blank line), so current consumers
  still see every word. A note with unresolved image notes
  carries the row diagnostic `unresolved_image_note`. Annotations follow
  region order, so sectioned blocks come first.
- `sections`: live annotation sections when the managed region has at
  least one `[!summary|section]` card. Each entry has `id`, `title`,
  `tag`, `block_id`, `link` (`[[ref/x#^s-…]]`), `count`, and optional
  `description`, `pages`, and `images` (omitted when empty). `count` and
  `pages` are computed from the parsed member blocks, never by parsing
  the card caption. The array is omitted when the region has no
  sections.
- `excluded`: counts of `marker_mirrors`, `preamble`, and `removed`
  (tombstoned) blocks. Tombstones carry no text, so they are counted
  instead of shown.
- `own_notes`: the user's own text (a legacy note's whole migrated
  body).
- `tasks`: the parsed `## Tasks` lines (`checked`, `mark`, `text`,
  `block_id`, plus `path` only on follow-ups found elsewhere).
- `also`: companion paths (family companions and superseded notes).
- `later`: later readings of the shown row (family members with a higher
  reading number, oldest first).

`-c/--comments-only` keeps only annotations with a comment, standalone
notes, or image notes (embedded or unresolved), with their quotes. A `<`
group is kept when any piece has a comment or image note, and all of its
pieces are shown.
`-N/--no-annotations` keeps
metadata and notes only. The `annotations` array honors the flags; `-N`
also omits `sections`. `-c` keeps the full `sections` list, because like
the status and `excluded` counts it describes the whole region.

Human output prints the title, a reading-state header (chip, a `vN`
reading chip for readings past the first — and a `v1` chip on a root
that has later readings — date, type, origin, path), the reading-task
line, the metadata rows that have a value (including an `earlier:` row
of entry labels and a derived `later:` row), then the annotations grouped by page with wrapped quotes
(`“…”`), `[image: <vault path>]` lines for image notes,
yellow `[image not found: <source>]` (or `unreadable` / `unsupported`)
lines, and cyan `↳` comments, the exclusion parenthetical, and the
`NOTES` and `TASKS` sections. When the region has sections, annotations
group under magenta `▍` headers (`▍Memory  @memory · 2 highlights ·
Pages 4–5`): the bar is magenta, the title bold, the caption dim; the
description is dim, wrapped, and every line carries the bar. A blank
line separates groups, and the page-run rule resets per group. Empty
scaffolds print `@tag · nothing tagged yet`. Ungrouped blocks follow
under `Other highlights · N`. With `-c`, a section header prints only
when one of its members is kept. Unsectioned rows print exactly as
today. A `<` group prints each piece's wrapped
quote, image lines, and cyan `↳` comment in order, with each follower
preceded by a dim seam line at the quote indent (`┄ Page 12`, or a bare
`┄` without a label). Single annotations print exactly as before. The reading-task line is `📖 Reading task
<residence> · <lane> → <link>` for open v2, `📖 Read/Dropped · <date> ·
<residence> [(archived)]` for closed v2, `📖 Reading task in this note
(v1) · <lane>` for v1, and `📖 Reading task ⚠ N open tasks claim this
ref · run bob ref doctor` for ambiguity. Each `TASKS` row carries its
linked annotation's page label as a dim suffix, or `   § <Title>` when
the task's `block_id` is a section's, plus a dim `· <path>`
for follow-ups found elsewhere. Empty sections stay omitted; several
`REF`s are separated by a dim rule.

Paired `**…**` runs in highlight quotes render as bold underline (ANSI SGR
`1;4`) in color-enabled human output. Plain human output, Markdown, and JSON
retain the literal `**` markers. The emphasis stays paired across wrapped
lines; unmatched markers remain literal. See
[Underline emphasis](highlights-ref-sync.md#underline-emphasis) for how PDF
underlines become these quote runs.

Markdown output is a quotable digest per note: `## <title>`, bullets for
the note link, reading state with evidence and date, a `- **Reading
task:** …` bullet with the same text as the human line, source URL and
arXiv/DOI, origin and type, annotation counts and snapshot date (plus
`Research report: research:…` when set, `Earlier:`/`Later:` lineage
bullets when the row has lineage, and an `Also: …` line naming each
companion with its parenthetical), then
`### Annotations` (one `**Page N**` label per page, quotes as `>`
blockquotes, image notes as `Image: <vault path>` or
`Image not found: <source>` lines, comments as `Comment: …` lines,
standalone notes as `Note: …`), `### Notes`, and `### Tasks` (keeping each task's real
mark, so a `[-]` task stays `[-]`). When the region has sections, each
section is a `#### <Title>` with an italic caption
(`*@memory · 2 highlights · Pages 4–5*`), the description as `>` lines,
members with today's `**Page N**` captions (reset per group), and
`_Nothing tagged yet._` for empty scaffolds; ungrouped blocks follow
under `#### Other highlights`. Unsectioned digests stay byte-identical.
A `<` group prints each piece as its
own `>` quote with its own image and `Comment:` lines, with each follower
preceded by an italic `*┄ Page 12*` seam line.

Agent usage: resolve candidates with `bob ref find` first, then read
exact notes with `bob ref show <path>`. Prefer `-c -f markdown` for
Bryan's own thoughts on a reference, and never claim a `not_found`
result means he has not read something — it only means it is not under
`ref/`.

## `bob ref code`

```bash
bob ref code ea_graph
bob ref code ea_graph 2 | pbcopy
bob ref code ea_graph 2 -f markdown
bob ref code ea_graph -p 7 -f json
bob ref code ea_graph --source > source.md
```

Options: `-b/--bob-dir`, `-f/--format human|json|markdown|raw`,
`-p/--page PAGE`, `-r/--ref-dir`, and `-s/--source`. Without a block
number, the default format is a human list; with `N`, the default is
`raw` and writes only that block's exact text, with no added newline.
`-f markdown` writes a fenced block when `N` is present, or a titled,
sectioned digest when it is absent. `-p` keeps blocks whose rendered
page range includes that page. `--source` writes the exact saved
`source.md` bytes and conflicts with `N`, `-p`, and `-f`.

The command resolves `REF` with the same exact resolver as `bob ref
show`, verifies the saved source digest, and never runs pandoc. JSON
uses `schema_version: 1` and reports the reference, source, blocks,
and one of these statuses: `available`, `no_code`, `no_catalog`,
`no_source`, `source_missing`, or `source_invalid`. Each block includes
its exact `text`, a safe fenced form, language, title, summary,
caption, section, and rendered page range. JSON status responses exit
successfully so callers can branch on `status`; unknown or ambiguous
references and a requested block number that does not exist exit 1.

The `id` on each block is generation-scoped as `<source-hash12>/<N>`.
The summary skips blank lines, simple rules, box-drawing lines, and
lone brackets or braces. For Mermaid blocks, a leading frontmatter
title becomes the caption and the diagram type becomes the summary.

## `bob ref today`

```bash
bob ref today ea_graph
bob ref today ea_graph -p reading
bob ref today ea_graph -d -f json
```

Options: `-b/--bob-dir`, `-d/--dry-run`, `-e/--exact` (requires `-p`),
`-f/--format human|json` (default human), `-p/--pomodoro NAME`,
`-P/--pomodoro-ref LINE:DIGEST`, `-r/--ref-dir`. `-p` conflicts with
`-P`.

`REF` uses the exact resolver from `bob ref show`: note path, stem,
frontmatter id, URL, arXiv ID, DOI, or a unique exact title. The
command then locates the reference's v2 reading task and runs the same
planner as `bob capture '^route:block-id[#name]'` (see [Linking and
starting existing tasks](capture.md#linking-and-starting-existing-tasks)).
It never starts a Pomodoro and never commits or syncs; vault-sync picks
the change up, as with capture. The tracker change reads
`status_sync: pending` until the next `ref scan -w` (the Refs panel's
⌘S), exactly as an Obsidian Ctrl+Shift+Enter on a ref task does today.

| Flags | Destination |
| --- | --- |
| none | Capture's no-name rule. If the task already has a dedicated live link under an open Pomodoro (Q), it stays there (`already_current`). Otherwise the implicit current/next entry: the single open timed entry, else the first open entry |
| `-p NAME` | Capture's `#name` rule: whole-slug match among open entries, then prefix match, else create the canonical named future Pomodoro. Q's subtree moves |
| `-p NAME -e` | Whole-slug match among open entries only, else create. Never a prefix match |
| `-P LINE:DIGEST` | Exactly that entry, resolved stale-safe through the `ref` values the dry run lists. A completed entry gives `pomodoro_closed`; stale or ambiguous gives `stale_pomodoro` |

Human output prints `✓ linked 📖 <title>` (or `would link` on a dry
run), then capture's pomodoro-link details. `--dry-run` also lists
open Pomodoros (`●` running, `◆` default, `✓` linked) with `-P
LINE:DIGEST` so unnamed entries can be targeted. An unnamed row prints
its derived homes label (from `homes`) instead of `(untitled)` when its
chip is non-null.

JSON is schema 1 with `command: "ref today"`. A success envelope
carries `mode` (`dry_run` or `write`), `changed`, `ref`, `task`,
`pomodoros` (every open top-level entry, with `running` / `default` /
`linked` flags), and `capture` (the same item object `bob capture -f
json` reports for `^route:block-id`). Each unnamed `pomodoros` row also
carries the derived R9 `homes` object (forecast: open rows never carry
minutes); named rows do no resolution and omit it. A guarded gesture is
`ok: true` with `capture.kind == "link_guard"`: it is a real write that
promotes prerequisites, as in capture.

```json
{"ok":true,"schema_version":1,"command":"ref today","mode":"dry_run","changed":true,"ref":{"path":"ref/papers/attention.md","link":"[[ref/papers/attention]]","title":"Attention Is All You Need","marker_id":"attention"},"task":{"path":"sase.md","block_id":"ref-attention","link":"[[sase#^ref-attention]]","status_symbol":" ","status_name":"Ready"},"pomodoros":[{"ref":"41:0b1c2d3e","line":41,"name":"BUGS","running":true,"default":true,"linked":false}],"capture":{"kind":"pomodoro_link"}}
```

Failures print
`{"ok":false,"schema_version":1,"command":"ref today","error":{"code","message","hint"}}`
and add `candidates` for resolution failures, plus `ref`, `task`, and
`pomodoros` whenever resolution got that far. Exit 1 for failures,
exit 2 for usage (including `invalid_pomodoro_name`).

| Code | When |
| --- | --- |
| `unknown_reference` | REF matches nothing |
| `ambiguous_reference` | REF matches several notes |
| `no_reading_task` | The reference has no reading task |
| `ambiguous_reading_task` | Several open reading tasks claim the reference |
| `v1_reading_task` | The task is an in-note v1 `^ref` tracker |
| `reading_task_closed` | The task is archived, read, or abandoned |
| `reading_task_unroutable` | The task's residence is not a root note |
| `no_daily_note` | Today's daily note does not exist |
| `no_pomodoros_section` | The daily note has no Pomodoros section |
| `no_open_pomodoro` | There is no open entry and no selector |
| `multiple_running` | Implicit selection or named creation with several open timed entries |
| `invalid_pomodoro_name` | bob's Pomodoro name usage text |
| `stale_pomodoro` | The `-P` ref no longer matches an entry |
| `pomodoro_closed` | The `-P` ref names a completed entry |
| `ledger_invariant` | For example, several movable links |
| `note_changed` | A pre-image mismatch at write time; retry |
| `io` | Any other I/O failure |

## `bob ref rename`

```text
bob ref rename <REF> <NAME> -d
```

`REF` uses the exact resolver from `bob ref show`: note path, stem,
frontmatter id, URL, arXiv ID, DOI, or a unique exact title. `NAME` is a
same-directory file stem matching `[A-Za-z0-9][A-Za-z0-9_.-]*`; one trailing
`.pdf` is ignored. For example, the motivating gist can be previewed with:

```bash
bob ref rename 442a6bf555914893e9891c11519de94f karpathy_llm_wiki -d
```

The dry-run inventories the note, PDF, sidecar, audio, and inbound links, then
reports the frontmatter paths that would change. It does not write files, take
locks, sync, or commit. Live mode applies the same plan as one scoped commit.
The JSON form (`-d -f json`) uses schema version 1 and includes `from`, `to`,
`id`, `moves`, `links`, `frontmatter`, `kept.task`, and the current `ref` row;
`id` is `{from, to, changed}` with nullable ids. `commit` is `null` and both
sync values are `skipped` in a dry run.

V1 full renames accept only modern, PDF-backed, unannotated references in their
current `ref/<type>/` directory. They refuse legacy and intake references,
annotated references, and notes whose PDF sync plan is not clean. A rename
changes the file stem for the note, PDF, and any same-stem sidecar or audio
companions; it updates inbound vault links and the note's `source_pdf`,
`highlights_sidecar`, and `audio` paths. The frontmatter id and PDF marker id
follow the new stem. If another reference already records that id, Bob picks
the first free `<stem>_2`, `<stem>_3`, and so on; the bare stem is checked
against other ids, and suffixed ids are checked against other ids and stems.
Id-less references stay id-less. An id change rewrites the PDF marker, so close
the PDF in Highlights first. The title, `source_url`, dates, and reading task
block ID stay. Same-stem `bob ref rename <REF> <current-stem>` repairs a
mismatched id, including on annotated references, and reports
`outcome: "id_updated"`; an already-correct id returns `unchanged`. After an
id change, `bob ref find <old id>` no longer resolves. Links in browser
bookmarks and copied `obsidian://` or `highlights://` URIs are outside the
vault and cannot be updated.

Machine-readable refusal codes are `unknown_reference`,
`ambiguous_reference`, `legacy_reference`, `intake_reference`, `invalid_name`,
`name_taken`, `has_highlights`, `listen_card_link`, `not_in_sync`,
`dirty_paths`, `not_a_worktree`, `lock_busy`, `scan_busy`, `sync_failed`, and
`verify_failed`. JSON errors use the shared schema-version-1 envelope with an
`error` object containing `code`, `message`, and optional `hint`.

### Live write flow

The live command takes the shared `bob_sync.lock` first (60 second wait), then
the reference scan writer lock (30 second wait). It requires a Git worktree so
the rename can be reverted. Unless `--offline` is set, it runs vault-sync while
holding both locks and replans from the synced files. Offline runs refuse when
any source, destination, or rewritten path is dirty.

The writer snapshots every Markdown file it will rewrite, moves the bundle with
no-clobber semantics, and atomically writes the planned link and frontmatter
changes. When the id changes, it then runs the sync engine's `bob ref sync -w`
write path inside the transaction to update the PDF marker and refreshed sync
metadata. It verifies that a dry-run `bob ref sync` on the new PDF plans no
work, and that the rebuilt index and PDF marker both have the planned id while
the reading-task `(path, block_id)` stays the same, with no duplicate identity
or orphaned task introduced. A write or verification failure moves files back,
restores the saved Markdown before-images, and atomically restores the original
PDF bytes unless the PDF changed concurrently.

On success, Bob commits only the old and new bundle paths and rewritten notes
with subject `bob ref rename: <old> → <new>`. The JSON `commit` object contains
the commit `sha` and `subject`. Unless `--offline` is set, Bob then runs
vault-sync again. If that post-sync fails, the local rename commit stays valid:
the command exits successfully, reports `sync.post: "failed"`, and tells you to
run `bob vault-sync`. `--offline` skips both sync cycles and commits locally.

A rerun using the old stem or note path succeeds with
`outcome: "already_renamed"` when the latest commit touching the new note has
the matching rename subject. Passing the current stem as both `REF` and `NAME`
returns `outcome: "unchanged"` when the id is allocated correctly. If the id
needs repair, the same-stem run commits the id-only update with subject
`bob ref rename: <stem> (id <old> → <new>)` and reports no file moves or link
rewrites.

### Renaming while the PDF is open in Highlights

Acceptance (Mac, 2026-10-15): a scratch-vault script renamed a PDF while it was
open in Highlights. Result: PASS — Highlights did not recreate the old PDF or
note path, so the `confirm` fallback never triggers. The motivating live rename
(`442a6bf555914893e9891c11519de94f` → `karpathy_llm_wiki`) then committed
cleanly in the live vault with pre/post sync OK; verification confirmed the old
paths absent, the new paths present, and a follow-up dry run unchanged with the
frontmatter `id` and reading-task `^ref` preserved. Recorded from
`bob-cli-7x.6`.

### Rollback runbook

Each rename is one Git commit. To undo it, revert that commit and publish the
revert through vault-sync:

```bash
git -C ~/bob log --format=%h --grep='^bob ref rename' -1
bob vault-sync status --json
git -C ~/bob revert --no-edit <sha>
bob vault-sync
```

## Migrating zorg-era records (`bob ref migrate-zorg`)

```bash
bob ref migrate-zorg
bob ref migrate-zorg -f json
bob ref migrate-zorg --write --offline
```

Options: `-b/--bob-dir`, `-f/--format human|json` (default
human), `-o/--offline`, `-r/--ref-dir`, `-w/--write`. The bare
command is a read-only dry run: no lock, no sync, no writes. It
plans one legacy note per unmirrored zorg-era `status::` record and
prints the plan as a human or JSON report. With `-w/--write` the
same plan is applied as one scoped commit between two vault-sync
cycles. A rerun afterwards reports `nothing to migrate`.

The write flow, mirroring `bob task reroll`:

1. Take `bob_sync.lock` (60 s budget) and require a git worktree:
   reversibility depends on git.
2. Pre-sync, unless `--offline`, so the migration commit holds only
   the new files. A failed pre-sync aborts with an `--offline` hint.
3. Re-plan from disk under the lock. An empty plan prints
   `nothing to migrate` and exits 0 without committing or
   post-syncing.
4. Refuse before writing anything when a planned target already
   exists, or when `ref/zorg/` holds uncommitted changes.
5. Write each note with create-new semantics (temp file, fsync,
   no-clobber rename, byte-compare re-read).
6. Verify through the rebuilt index and coverage: every planned path
   yields a row with no `invalid_yaml`, `opaque_url`, or
   `missing_type` diagnostic, each row carries its planned reading
   state, and coverage drops by exactly `notes + chapters`. Any
   failure deletes every file the run created (plus its emptied
   directories) and exits 1 without committing.
7. Commit exactly the written paths as
   `bob ref migrate-zorg: <records> records into <notes> notes under
   ref/zorg`, then post-sync unless `--offline`.

The JSON report carries `mode: "write"` with the created `commit`
(`sha`, `subject`, `paths`); the human report ends with the
`committed <sha> <subject>` line.

### Rollback runbook

The migration is one commit, so one `git revert` undoes it:

```bash
git -C ~/bob log --format=%h --grep='^bob ref migrate-zorg' -1
bob vault-sync status --json   # check that no sync is running
git -C ~/bob revert --no-edit <sha>
bob vault-sync
```

If the revert hits conflicts because migrated notes were edited
since, resolve them by hand: keep the pre-migration side for every
`ref/zorg/` path to restore the unmigrated tree.

Every parsed record lands in exactly one bucket: `already_migrated`
(an indexed ref note carries the same `source_path` plus the same
`source_block` with a matching `source_id`, or the block in its
`source_blocks`), `skipped` (`no_owner`, `no_id`, or
`unassigned_chapter`), `chapter` (folded into a book note), or `note`.

- **Layout.** Each note goes to `<ref-dir>/zorg/<hub>/<stem>.md`,
  where the hub is the source file's stem. The stem is the record's
  `ID::` value unless that stem already names a Markdown file anywhere
  in the vault (including `_generated/` and `ref/`) or another note
  planned in the same run; collisions become `<ID>_ref`,
  `<ID>_ref_2`, and so on, planned in source-path then owner-line
  order. The report lists every rename and the file it avoided.
- **Note shape.** Frontmatter in fixed key order (`parent`, `type`,
  `tags`, `ref_type`, `status`, `legacy_status`, `title`, `url`,
  `source_note`, `source_block`, `source_id`, `source_path`,
  `source_line_start`, `source_line_end`, plus `source_blocks` and
  `legacy_chapter_statuses` on books). Tags are `zorg/reference` plus
  the `#tags` on the owner line. `ref_type` is the `lib/<kind>/` of
  the first `file::` wikilink, else `zorg` (`chat` notes read as
  `agent-report` origin). The title is the record's `title::` field
  when present, else the humanized ID (words split on `_`/`-` and
  capitalized, with ai, api, cli, gtd, html, http, json, llm, lsp, mcp,
  pdf, prd, sase, sql, ui, url, ux, and yaml uppercased; `bob` renders as
  `Bob`). The body
  carries `## Files`, `## Related`, `## Chapters` (books), and an
  escaped `## Original Record` fence. The original source records are never
  edited or deleted. No `^ref` tracker is written, so these notes do not
  become actionable tracker tasks. Their `legacy_status` (or chapter states)
  still determines their reading state. `unread` becomes `queued` and
  `collect_fleeting_notes` becomes `started`, so those notes match the
  default reading-queue filter. Human `list` still hides them behind
  `+ N zorg-era legacy notes hidden (…) · show them with -s legacy` until
  you pass `-s`. Any `-s` value turns the collapse off and filters to that
  status. JSON and Markdown include the matching rows either way.
- **Books.** A source file holding exactly one BOOK record is a book;
  its `LID::` records (plus an `ID::` record tied by a `| BOOK:`
  line) fold into the book's note as `## Chapters` lines with their
  blocks appended to the fence. The book's reading state derives
  from its chapters. An `LID::` record with no owning book is
  skipped as `unassigned_chapter`.
- **URLs.** The first token of the inline `url::` value plus the
  first token of each nested item under it; only values that pass
  URL validation are stored (one scalar, a list for several, omitted
  for none), so no migrated note earns an `opaque_url` diagnostic.
  URL, arXiv, and DOI matches against existing notes are reported
  as identity hits, and each record still migrates as its own note.

The human report prints a headline (`N records in F files → M notes
under ref/zorg/`), counts by status and by file, books with chapter
counts and derived states, renamed stems, notes without a URL,
identity hits, the already-migrated count, skipped records with
reasons, and `coverage after --write: K unindexed`. The JSON
envelope carries the same data with `mode: "dry_run"` and
`commit: null`; each note entry shows its path, title, `ref_type`,
provenance, `reading_state` (computed by running the rendered note
through the index row builder), chapter count, and rename.

## Migrating open ref tasks (`bob ref migrate-tasks`)

```bash
bob ref migrate-tasks
bob ref migrate-tasks -f tsv
bob ref migrate-tasks -m map.tsv --write --offline
```

Options: `-b/--bob-dir`, `-f/--format human|json|tsv` (default
human), `-m/--map FILE`, `-o/--offline`, `-r/--ref-dir`,
`-w/--write`. The bare command is a read-only dry run: no lock, no
sync, no writes. It plans every open v1 ref task's move into its
parent note and prints the plan as a human, JSON, or TSV report.
With `-w/--write` the same plan is applied as one revertible commit
between two vault-sync cycles. A rerun afterwards reports
`nothing to migrate`.

Parents resolve in order: the `-m` map row, else the note's
frontmatter `parent` wikilink, else the PDF marker `parent`. An
unmapped ref is listed and `--write` refuses the whole run before
any write, as does any note with two or more open trackers.

What is preserved and dropped: the mark stays, `[fresh::]`,
`[refresh::]`, `[keeps::]`, priority, schedule, close fields,
`[id::]`, `[dependsOn::]`, and every other tag stay; whole-token
`#hide` and the PDF wikilink are dropped. The new line carries
`#task #ref`, a path-qualified `[[ref/<type>/<stem>|<title>]]` link,
`[created::YYYY-MM-DD]` when missing, and `^ref-<slug>` at the end.

The write flow, mirroring `bob task reroll`:

1. Take `bob_sync.lock` (60 s budget) and require a git worktree.
2. Pre-sync, unless `--offline`.
3. Re-plan from disk under the lock. An empty plan prints
   `nothing to migrate` and exits 0 without committing or
   post-syncing.
4. Refuse before any write when any ref is unmapped, any note has
   multiple open trackers, or any changing path differs from `HEAD`.
5. Snapshot every changing path, write sequentially (reading task,
   follow-ups, ref-note edit, graph rewrite), and verify through the
   rebuilt index.
6. Commit exactly the written paths as
   `bob ref migrate-tasks: <N> ref tasks into <M> notes`, then
   post-sync unless `--offline`.

The JSON report carries `mode: "write"` with the created `commit`
(`sha`, `subject`, `paths`); `-f tsv` prints
`ref_note<TAB>parent<TAB>status<TAB>title` for map round trips.

### Rollback runbook

The migration is one commit, so one `git revert` undoes it:

```bash
git -C ~/bob log --format=%h --grep='^bob ref migrate-tasks' -1
bob vault-sync status --json   # check that no sync is running
git -C ~/bob revert --no-edit <sha>
bob vault-sync
```

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
`marker_id` (the frontmatter `id`, or `null` when absent), ref_type, era,
status, status_sync, legacy_status,
reading_state and its source, `blocked`, `blocked_by`, parent, `task`, urls, the
`earlier:` lineage entries (`target`, `label`, `kind`: `reading` or
`replaced`) and the `reading` number (`null` for rows outside a family,
`1` for a root), identity
keys, author/published/captured, added date/source/instant and finished dates/sources,
source_pdf, audio, annotation and comment counts, snapshot,
research_ref, superseded_by, and diagnostics. `show` rows add `later`
(the family paths newer than the shown row). `task` is always present
(`null` when there is no task): v2 carries `path` (the residence file),
`block_id`, `link` (`[[path#^id]]` or `[[path]]`), `mark`,
`archived`, `priority`, and `scheduled`; v1 carries the ref note path,
`block_id: "ref"`, `link: [[ref/…#^ref]]`, `archived: false`, and the
same `priority`/`scheduled` fields from the tracker line. One
`(path, block_id)` join works for both eras. `blocked`, `blocked_by`,
`task`, `marker_id`, `added_at`, `reading`, `earlier`, and `later` are
additive under `schema_version` 1.

Diagnostics: `multiple_open_ref_tasks` (several open v2 claim the ref;
detail lists `path:line`), `open_ref_task_in_done` (open task under
`done/`, report-only), `open_ref_without_task` (v2 note with a
ready/next/wip status but no task), `orphan_ref_task` (a `#ref` line
resolving to no note), `ref_task_outside_area` (live task outside an
area/project/inbox), `parent_mismatch` (frontmatter parent differs from
residence), `earlier_dangling` (an `earlier:` entry whose target
resolves nowhere), and `open_v1_tracker` (open in-note `^ref`; run `bob ref
migrate-tasks`).

## Errors and exit codes

Exit 0 whenever the lookup ran, including `not_found`; exit 1 for
vault failures (a missing ref dir) with
`bob ref: error: reference directory not found: <path>` plus the hint
`pass -b/--bob-dir or -r/--ref-dir, or set BOB_DIR`; exit 2 for usage
errors. In JSON mode, failures print
`{"ok":false,"schema_version":1,"command":"ref find","error":{"code":"…","message":"…","hint":"…"}}`
to stdout and exit 1. A missing intake directory under `-i` is not a
failure: coverage reports `unavailable` and the lookup continues.
`show` and `today` resolution failures (`ambiguous_reference`,
`unknown_reference`) exit 1 as well; in JSON mode they add
`"candidates":[{"path":…,"title":…}]` to the envelope. `today` uses
the same envelope with `command: "ref today"` and the error codes in
[bob ref today](#bob-ref-today).

## URL routing

Bare public links captured with `bob capture`, shared to Google Keep
and pulled with `bob gkeep pull`, or previewed with `bob capture-parse`
are classified as references instead of inbox tasks. Capture saves a
background job and prints `→ reading queue` before any note exists.
Keep clips an intake PDF during `pull`. `bob ref scan` writes the note
that `bob ref list` shows. `capture-parse` only classifies. The
classifier, policy, and offline verdict below are shared by every entry
point; nothing in this section fetches the URL.

A token is a reference URL only when it has no whitespace, carries an
optional single `<…>` wrapper, uses `http`/`https`, passes
`validate_and_clean`, and has a host that is not an IP literal and
contains at least one `.` with non-empty labels. Corporate short links
(`http://go/x`), `localhost`, and IP literals stay tasks.

Policy lives under `highlights.url_routing` in `~/.config/bob/config.yml`
(`BOB_CONFIG_FILE` overrides the file; `XDG_CONFIG_HOME` overrides its base):

```yaml
highlights:
  url_routing:
    capture: true # bare-URL capture items queue a background reference job
    gkeep: true # URL-only Keep notes are clipped during bob gkeep pull
    exclude_hosts: # replaces the defaults; each entry matches the host and its subdomains
      [google.com, googleplex.com, youtube.com, youtu.be, github.com, x.com, twitter.com]
```

Missing keys take the defaults shown. A configured `exclude_hosts`
list replaces the defaults; an empty list excludes nothing. Entries
are normalized (lowercased, with any scheme, leading `www.`, trailing
`.`, trailing `/`, path, or port stripped). A missing config file uses
the defaults. An invalid `url_routing` block turns routing off: `bob
capture`, `bob gkeep list`, and `bob gkeep pull` warn with
`URL routing is off: …`, and `bob capture-parse` stays silent. In every
case the bare URL stays a task. `capture: false` and `gkeep: false` turn
off that entry point with no warning. `bob ref doctor` prints the
effective policy as `url routing: capture on · gkeep on · excludes
google.com, …`, or `url routing: warn (off: …)` when the file is invalid.

`bob capture --no-ref '<URL>'` and `bob gkeep pull --no-ref` bypass routing
for one invocation. Capture also keeps an item as a task when it has extra
prose, markers, child lines, a global destination, or forced destination flags.
`bob capture-parse` reads this policy from the config file. It does not open
the vault or fetch the URL. `-R`, `capture: false`, and an invalid config
all keep its links classified as tasks.

Capture queues links for background clipping through durable ref jobs; a
failed clip falls back to exactly the inbox task capture would have written,
plus a ⚠️ bullet. Google Keep imports clip inline during `pull`: retryable
failures stay in Keep, while permanent failures become tasks with a warning.
See [ref jobs](ref-jobs.md) and [Keep URL imports](gkeep.md#url-only-notes-go-to-the-reading-queue).

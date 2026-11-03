# Period Notes

Every daily note's parent is its ISO week note, and the tree climbs
week → quarter → year → all_time → org with no dangling links. bob
creates each period note ahead of time from a deterministic scaffold
and never edits a note it did not just create.

`bob periods` is the terminal surface. The
[`bob periods` CLI reference](reference/cli/periods.md) indexes the
commands. This page specifies the core contract: the calendar, ids,
paths, labels, scaffolds, note anatomy, review states, agent callouts,
`ensure`/`list`, vault adoption (`migrate`), the problems report, and
the report-only nightly step.

## Calendar and identity

- **Week**: an ISO week, Monday–Sunday. Its year is the ISO week-year.
  Years have 52 or 53 weeks (53 when Dec 28 falls in W53).
- **Quarter**: 13 whole ISO weeks: Q1 = W01–W13, Q2 = W14–W26,
  Q3 = W27–W39, Q4 = W40–W52, plus W53 when it exists.
- **Year**: an ISO week-year, from the Monday of W01 to the Sunday of
  its last week.
- **All time**: a singleton. Its report span runs from the Monday of
  the earliest ISO week that contains a canonical daily note through
  today.

Period ids:

- canonical forms `2026-W45`, `2026-Q4`, `2026`, `all`;
- parsing is case-insensitive and also accepts `all_time` and
  `all-time`, plus a vault-relative note path with or without `.md`
  (`2026/2026-W45.md`, `2026.md`, `all_time.md`);
- weeks must be `01..=weeks_in_year`, quarters `1..=4`, and years four
  digits in `1970..=2999`;
- anything else is a usage error that names the accepted forms.

Paths (link targets drop `.md`):

- week: `YYYY/YYYY-Www.md`, filed under its ISO week-year;
- quarter: `YYYY/YYYY-Qn.md`;
- year: `YYYY.md` at the vault root;
- all time: `all_time.md`.

Daily notes keep their calendar-year folder.

Parents:

- daily → `[[GGGG/GGGG-Www]]`;
- week → its quarter;
- quarter → `[[YYYY]]`;
- year → `[[all_time]]`;
- all_time → `[[org]]`.

Range labels use an en dash with spaces:

- same month: `Nov 2 – 8`;
- same year, different months: `Oct 26 – Nov 1`;
- different calendar years: `Dec 28, 2026 – Jan 3, 2027`.

Titles:

- week: `2026-W45 · Nov 2 – 8`;
- quarter: `2026-Q4 · W40–W53 · Sep 28, 2026 – Jan 3, 2027`;
- year: `2026 · Dec 29, 2025 – Jan 3, 2027 · 53 weeks`;
- all time: `All time`.

Short labels are `W45` (no leading zero: `W1`), `Q4`, `2026`,
`All time`. Nav labels use the long id when a neighbour is in another
ISO year (`‹ 2025-W52`, `2027-W01 ›`). Up labels read `↑ 2026-Q4`,
`↑ 2026`, `↑ All time`, `↑ org`. Day labels read `Mon 2`, with the
month on the week's first day and on the 1st of any month
(`Mon Nov 2`, `Sun Nov 1`).

Golden facts: 2025-12-29 → `2026/2026-W01` → `2026-Q1` → `2026` →
`all_time`; 2026-W53 = Dec 28, 2026 – Jan 3, 2027 in 2026-Q4; 2027-W01
starts 2027-01-04; 2026-Q4 = W40–W53 = 14 weeks; 2000–2100 holds
exactly 18 53-week years.

## Note anatomy and ownership

bob writes each scaffold once. Its bytes are a pure function of the
period id: no `created:` field and no wall clock. Each scaffold is
LF-terminated, has one blank line between blocks, and ends with a
single trailing newline.

Week (`2026/2026-W45.md`): frontmatter (`parent`, `type: [[day]]`,
`id`, `date`, `end`, one alias, tag `weekly`), `#` title, the
prev/up/next nav line, the seven day links, the `bob-stats-strip`
fence, `## Intentions`, `## Days` with the `bob-period-parts` fence,
`## Review`, `## Reflection`, and the weekly callout.

Quarter: frontmatter with tag `quarterly`, the title, nav, the week
child links, the strip fence, `## Outcomes`, `## Weeks` with the parts
fence, `## Review`, `## Reflection`, and the quarterly callout.

Year: frontmatter with tag `yearly` (the id is quoted: `id: "2026"`),
the title with its week count, nav, the quarter child links, the strip
fence, `## Theme`, `## Outcomes`, `## Quarters` with the parts fence,
`## Review`, `## Reflection`, and the yearly callout.

All time: frontmatter (`parent: [[org]]`, `type: [[day]]`,
`id: all_time`, alias `All time`, tag `all-time`), `# All time`,
`[[org|↑ org]]`, the strip fence, `## Years` with the parts fence,
`## Notes`, and the all-time callout.

Ownership:

- **Frontmatter, title, nav and child links, fences, and the callout**
  belong to bob. Written once; only the callout can be refreshed.
- **Strip and parts table** belong to bob-ledger-tools, rendered live.
  No number is ever written into Markdown.
- **`## Review`** belongs to an agent, through `bob periods draft`.
- **`## Intentions` / `## Outcomes` / `## Theme`, `## Reflection`,
  `## Notes`** belong to Bryan. No tool or agent writes them.

Period notes hold no task rows.

## Review state and Headline

Sections are the bodies under exact `## <Name>` headings up to the
next `## ` heading, the `> [!agent]` callout, `## Archive`, or EOF,
with blank lines trimmed.

Review state (first match wins):

- `reflected`: `## Reflection` has text;
- `drafted`: Review has text, and frontmatter `review_digest` equals
  its digest;
- `edited`: Review has text, and the digest is absent or different;
- `empty`: none of the above.

Digest: `sha256:` plus the first 16 hex characters of SHA-256 over the
Review body, after normalizing line endings to `\n`, stripping trailing
whitespace from each line, and trimming. `review_drafted: YYYY-MM-DD`
records the draft day.

Headline: the first Review line matching
`^\*\*Headline:\*\*\s+(.+?)\s*$`, else none. Reflection lead: the first
sentence of `## Reflection`, capped at 40 words with `…`. Instructions
version: parsed from the callout title
`Agent instructions · <kind> v<N>` (currently v1).

## Agent instruction callouts (v1)

Each scaffold ends with its callout. Weekly callouts draft at most 250
words from `bob periods brief`; quarterly at most 400; yearly at most
600. Every callout checks the brief first, treats the brief as the only
source of numbers, links every bullet, copies numbers exactly, and
saves only through `bob periods draft`. The all-time callout writes
nothing: it runs `ensure`, reports `list -p` problems, and edits
nothing.

## CLI so far

`bob periods` is a group in the **Vault** section for commands about
period notes. Bare `bob periods` runs the read-only `list`.

- `bob periods ensure [-d/--date DATE] [-f human|json] [-n/--dry-run] [-q/--quiet]`:
  create-only, one week of look-ahead. It wants the week, quarter,
  year, and all-time chains for DATE (default: today from `BOB_NOW`)
  and for DATE + 7 days. Missing notes are created; empty or
  whitespace-only notes are filled; notes with content are never
  touched. Human output prints one `created <path>` line per write
  (`would create` with `-n`); `-q` stays quiet when there was nothing
  to do. JSON returns
  `{ok, schema_version, created[], filled[], existing[], dry_run}`.
- `bob periods list [-f human|json] [-k/--kind week|quarter|year] [-p/--problems] [-y/--year YEAR]`:
  read-only. Prints the period notes that exist, newest first, grouped
  year → quarter → weeks, each with its review-state mark and Headline.
  JSON returns `{ok, schema_version, rows[]}` with
  `{id, kind, path, review_state, headline}` rows. `-p` adds the
  problems report below.
- `bob periods migrate [-a/--apply] [-f human|json] [-o/--offline] [-s/--since DATE]`:
  dry run by default. `-a` writes one `vault_rewrite` transaction and
  one commit. `-o` skips vault-sync before and after, as with
  `bob types rename`.
- `bob periods show [PERIOD] [-f human|json] [-k/--clock worked|off-clock|total]`:
  read-only. With no PERIOD it shows the current ISO week. It prints a compact
  terminal report or the schema-1 report below. The selected clock is applied to
  Pomodoro minutes and lanes; Worked is the default.

Exit codes: 0 success; 1 vault I/O or transaction failure (including
`lock_busy` after waiting at most 3 s on `bob_sync.lock`); 2 usage;
3 a `migrate` refusal (not a worktree, dirty paths).
JSON errors use
`{"ok":false,"schema_version":1,"error":{"code","message","hint"}}`.

`ensure` creates files without clobbering (temp file plus hard link)
and re-reads just before each write. It never commits; vault-sync
commits, and agents commit as the vault's `AGENTS.md` says.
Environment: `BOB_DIR`, `BOB_NOW`, `BOB_CONFIG_FILE`, `NO_COLOR`.

## Migration

`bob periods migrate` is dry run by default. The adoption range starts
at the Monday of the ISO week that contains `--since` (default: the
first day of the current ISO year) and runs through the current week +
1. One `vault_rewrite` transaction:

1. Creates `all_time.md` if it is missing (fills it when empty).
2. Adopts each in-range root year note (`YYYY.md`): sets `parent`,
   `type`, `id`, `date`, `end`, `aliases`, and `tags` by line edits,
   keeps every other key (including `zorg_*`), replaces unquoted
   `parent: [[journal]]` textually, inserts the year scaffold above the
   old body, and moves the old body under a final `## Archive` heading
   after the callout. Missing year notes are created. Out-of-range year
   notes (`2024.md`, `2025.md`) get only their `parent` rewritten to
   `"[[all_time]]"`.
3. Creates every missing quarter and week note in the range. Empty
   notes are filled. Notes with content are never touched (`existing`).
4. Rewrites every canonical daily note dated on or after the range
   start (future dailies included): `parent:` becomes
   `"[[GGGG/GGGG-Www]]"` with the original quote style; a nav line that
   matches
   `^\[\[(\d{4}/\d{8})\|prev\]\]\s+\|\s+\[\[(\d{4}/\d{8})\|next\]\]\s*$`
   becomes `[[prev|prev]] · [[GGGG/GGGG-Www|Www]] · [[next|next]]`.
   Anything else is `nav_skipped`.
5. Never creates, edits, or deletes monthly notes.

Apply (`-a`) snapshots daily `worked`, `done`, and `created` (with
`root_parent` and `theme` parts) over the adopted range, writes, then
recomputes. Any difference restores every file and fails with
`verify_failed`, naming the first differing day and metric. It also
requires zero `dangling_daily_parent` problems in the range. The
commit subject is `periods: adopt the period-note tree for ISO
<years>`. Dry run prints counts per action, one sample daily diff,
`nav_skipped` / `existing` / adopted paths, and the subject `-a` will
commit.

## Problems

`bob periods list -p/--problems` is report-only (exit 0). Adopted ISO
years are those with at least one weekly note. Each problem has
`path`, `detail`, and a `fix` hint:

- `dangling_daily_parent`: a daily in an adopted year whose parent is
  not its week;
- `missing_period_note`: an expected note for a week that contains a
  daily dated today or earlier, or the parent of such a week;
- `unscaffolded`: a period note that is empty or missing its required
  headings or fences;
- `wrong_parent`: a period note whose parent is not the expected one;
- `task_rows`: a period note with a `#task` checkbox line;
- `stale_instructions` (info): an unreflected note whose callout
  version is older than current.

## Nightly

`bob nightly` runs a report-only **Period notes** step after
`articles` and before the final vault-sync. It prints `bob periods
list -p` and writes nothing, so athena never creates period notes.

## Period reports

`bob periods show` performs one day-grouped `bob stats` computation over the
requested period and its typical-history window. It does not create notes. The
report is the source for both the stats strip and the parts table; clients use
the supplied display strings, labels, review states, and catalogs without
recomputing them.

The schema-1 JSON object has these top-level members:

| Member | Contents |
| --- | --- |
| `ok`, `schema_version` | `true`, `1` |
| `today`, `generated_at` | The pinned local date and report timestamp |
| `period` | Canonical id, kind, short label, title, date range, week/day counts, state, observed-through date, and coverage |
| `note` | Period-note path, existence, instruction version, review state, and Headline |
| `links` | Parent, previous, and next note ids, paths, and labels |
| `facts` | Visible Day-strip facts in `worked`, `reviewed`, `done`, `created`, `review_closed` order |
| `lanes` | Worked root-parent and type parts, with labels, minutes, displays, shares, and a type-fold flag |
| `columns` | Render-ready strip columns: seven days for a week, weeks for a quarter/year, and quarters for all time |
| `groups` | Quarter Worked totals for a year, or year totals for all time |
| `children` | Parts-table rows: days, weeks, quarters, or years |
| `homes`, `theme_hues` | The Stats home and type catalogs used by the Day strip |
| `warnings` | Report warnings; empty when none apply |

Every fact has `metric`, `label`, `unit`, `value`, `display`, `so_far`,
`quality`, and `typical`. `review_closed` also has `of` and displays `N of M`.
Worked adds `offclock: {value, display}` only when the selected lens contains
positive off-clock minutes. A hidden metric (`stats.hide`) is omitted. Current
period totals add the complete daily values to today's provisional value;
`so_far` is true and `quality` keeps missing or pre-tracking days distinct from
zero. For the 2026-11-03 live example, week Worked is the Nov 2 complete day
plus the Nov 3 provisional value.

Typical values use closed prior periods only. A usable prior period has no
`not_tracked` day and at least one day with data; a missing day is no data, not
zero. Weeks use the median total of four prior weeks and need two usable weeks.
Quarters use a median per-week rate over four prior quarters, and years use
three; those rates are scaled to the current quarter/year's week count when
lengths differ. Both `words` and `detail` say when scaling occurs. The
`relation` is `above`, `below`, or `equal` only for a closed period; it is null
for current periods. All time has no typical value. `review_closed` has no
typical line.

`lanes.root_parent` and `lanes.theme` each contain `{key, label, minutes,
display, share}` rows. The weekly type lane is unmodified. Quarter, year, and
all-time type lanes fold `(unnamed)` and types below 2% of Worked into `other`.
`columns` include the date/period id, kind, labels, path and existence, start
and end, state, Worked value/display/quality, a root-parent `stack`, mark,
month `tick`, and parent `group` where relevant. Day marks are `met`, `missed`,
`open`, or null; period marks use the child note's review state. Current columns
include today's provisional value. Future columns remain identifiable and
carry no values.

`children` rows carry their id, kind, label, range label, note path and
existence, and state. Non-future rows add Worked and Done, review state,
Headline and Reflection lead, closeout for days, and the top three
root-parent `mostly` shares plus their automatic `mostly_display`. A future
row contains only its link identity and dates. The weekly `columns` and
`children` repeat the same day data; quarter columns and children repeat the
same week data. Year columns are weeks grouped by quarter and all-time columns
are quarters grouped by year. `groups` carry `key`, `label`, and
`worked_display`.

Future periods show a `Starts …` coverage line and do not run Stats. A closed
period with gaps reports `<N> of <M> days logged`; complete coverage has an
empty display. Period reads never write a note.

On the real vault, one `cargo run --quiet -- periods show 2026 -f json` run
measured 1.38 s wall time (including the Cargo launcher); it returned 53 week
columns, four quarter children, and four quarter groups. This met the 1.5 s
year-report target without a persistent cache.

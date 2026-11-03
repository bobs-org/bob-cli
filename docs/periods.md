# Period Notes

Every daily note's parent is its ISO week note, and the tree climbs
week → quarter → year → all_time → org with no dangling links. bob
creates each period note ahead of time from a deterministic scaffold
and never edits a note it did not just create.

`bob periods` is the terminal surface. The
[`bob periods` CLI reference](reference/cli/periods.md) indexes the
commands. This page specifies the core contract: the calendar, ids,
paths, labels, scaffolds, note anatomy, review states, agent callouts,
and `ensure`/`list`.

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
- `bob periods list [-f human|json] [-k/--kind week|quarter|year] [-y/--year YEAR]`:
  read-only. Prints the period notes that exist, newest first, grouped
  year → quarter → weeks, each with its review-state mark and Headline.
  JSON returns `{ok, schema_version, rows[]}` with
  `{id, kind, path, review_state, headline}` rows.

Exit codes: 0 success; 1 vault I/O or transaction failure (including
`lock_busy` after waiting at most 3 s on `bob_sync.lock`); 2 usage.
JSON errors use
`{"ok":false,"schema_version":1,"error":{"code","message","hint"}}`.

`ensure` creates files without clobbering (temp file plus hard link)
and re-reads just before each write. It never commits; vault-sync
commits, and agents commit as the vault's `AGENTS.md` says.
Environment: `BOB_DIR`, `BOB_NOW`, `BOB_CONFIG_FILE`, `NO_COLOR`.

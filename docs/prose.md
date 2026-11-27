# Prose notes

A prose note is a note whose body is free-form authored Markdown that the author owns:
paragraphs, headings, lists, tables, quotes, and code. bob may format its layout and
(unless turned off) section numbers, but never changes a word or adds structure. A prose
note holds no `#task` lines, `^prj` or `^ref` anchors, or second type. The test is the
note's primary role: an area or project with prose sections stays an area or project.

## Names and compatibility

`type: "[[prose]]"` is canonical. Notes typed `[[article]]` remain prose notes, and
`bob articles` remains a hidden alias for `bob prose`. The ArticlePlan contract keeps
`schema_version: 1`, the `summary.articles` key, and the `not-article` refusal code;
clients must continue rejecting unknown schema versions.

bob-cli owns prose-note section numbering, link repair, and layout. The Obsidian
`bob-articles` plugin sends the current buffer to bob and applies bob's returned line
edits as one undoable transaction. Formatting changes layout and heading numbers; it
never changes prose.

## Marker, options, and refusals

```yaml
---
parent: "[[sase_blog]]"
type: "[[prose]]"
created: 2026-10-15T15:10:41-04:00
title: "Why I built SASE" # optional display title; shown before aliases or file name
summary: "A short description of the piece." # optional one-line dek for Prose Card
section_numbers: true     # optional; true by default
---
```

Only `type: "[[prose]]"` or the legacy `type: "[[article]]"` enables formatting.
`title` feeds the repeated-title finding and the `list`/`outline` labels, and
the Prose Card uses it as the display title before aliases or the file stem.
`summary` is an optional one-line dek shown by the Prose Card; the formatter
does not write either property. See the [Prose Card](https://bobs-org.github.io/reference/plugins/prose-card/).
`section_numbers` accepts lowercase `true` or `false`; any other value produces a
`bad-option` finding on that line and leaves numbering on. The plugin's `.bob-article`
class is runtime-only and is never written to frontmatter.

Formatting skips the whole note in these cases; it never formats only part of a note.

| Code | Condition and response |
| --- | --- |
| `not-article` | The `type` includes neither `[[prose]]` nor legacy `[[article]]`; run `bob prose adopt`. |
| `frontmatter-unclosed` | The opening `---` has no closing delimiter. |
| `mixed-type` | The type also names another kind, such as `[[project]]`. |
| `holds-anchor` | The note contains a `^prj` or `^ref` block anchor; prose notes cannot be projects or refs. |
| `holds-tasks` | A Markdown checkbox task line has a `#task` tag outside inline code; move it to the parent note. The refusal lists the 1-based lines. |
| `conflict-markers` | The note contains Git `<<<<<<<` or `>>>>>>>` lines. |
| `excluded-directory` | The path is below `_conflicts`, `_templates`, `_generated`, `.obsidian`, or `.git`. |

Task lines, anchors, and conflict markers inside frontmatter, fenced code, math,
comments, and HTML comments are ignored. A prose mention of `#task` is not a task.

## Section numbering: N1–N9

| Rule | Behavior |
| --- | --- |
| N1 — Sections | Count ATX headings from `#` through `######` with zero to three leading spaces, and single-line Setext headings, in the top-level body. Empty headings count. Headings in frontmatter, fenced code, math, comments, HTML blocks, blockquotes/callouts, or list and footnote continuation lines do not count. |
| N2 — Preamble | Text before the first section is an unnumbered lede. |
| N3 — Levels | Keep the shallowest original level. A later heading becomes one level deeper than the nearest earlier heading whose original level was shallower. This closes skipped levels by promoting headings; it never pushes one down. |
| N4 — Counting | Depth is `new level - base + 1`. Each depth has a counter starting at 1; a shallower heading resets deeper counters. Sections deeper than depth 4 get `deep-section`. |
| N5 — Written form | Depth 1 uses `1.`; deeper levels use `1.1`, `1.1.1`, and so on. A heading is written `#…# NUMBER TITLE`, with one space between parts. The title bytes and inline markup are preserved. |
| N6 — Number ownership | Before numbering, bob removes one leading token matching `^\d{1,3}\.` or `^\d{1,3}(\.\d{1,3})+\.?`, followed by whitespace or end of title. A bare number without a dot is not owned, so `2024 taxes` and `30 Burger` keep their numbers. |
| N7 — Literal-number escape | Write `1\.5 million users` when `1.5` is prose. The Markdown escape renders the decimal and prevents N6 from treating it as bob's token. |
| N8 — Links | Numbered heading links are refreshed under the rules below. |
| N9 — Numbering off | With `section_numbers: false`, bob removes owned tokens and writes no numbers. Turning numbering back on restores them. Both states are idempotent. |

The token grammar follows the house style: top-level `1.`, then `1.1`, then `1.1.1`.
One known risk remains: an unescaped heading such as `## 1.5 million users` looks like an
owned number and will be rewritten. Adoption previews every replaced token, mismatched
number depth produces `number-replaced`, a single `u` undoes a plugin format, and git
retains the history.

## Formatting: F1–F11

| Rule | Behavior |
| --- | --- |
| F1 | Keep frontmatter bytes unchanged and put exactly one blank line between frontmatter and body. |
| F2 | Rewrite section heading lines as `#…# NUMBER TITLE`; remove heading indentation, extra whitespace after hashes, closing hashes, and trailing whitespace. A `#` inside the title remains. |
| F3 | Close skipped heading levels by applying N3. |
| F4 | Apply N4–N6 and N9. |
| F5 | Put one blank line before and after each section heading, except before the first body heading or after a heading at end of file. Headings next to math, comments, and HTML blocks get the same spacing. |
| F6 | Collapse a run of two or more blank lines to one when it separates top-level blocks. Preserve blank runs inside protected regions, between items in the same list, and between two chunks of indented code. |
| F7 | Remove trailing spaces and tabs except inside fenced code, math, comments, and HTML blocks. |
| F8 | Remove leading blank lines and finish with exactly one newline. Preserve each existing LF or CRLF ending, use the file's first ending for inserted lines, and keep a UTF-8 BOM. A frontmatter-only note ends immediately after its closing delimiter's newline. |
| F9 | Refresh in-note heading links. |
| F10 | Convert a single-line Setext heading to ATX and count it as `setext`. Leave multi-line Setext headings unchanged and report `setext-multiline`. `---` after a list item, quote, table, or lazy list continuation is a thematic break, not a heading underline. |
| F11 | Put one blank line before and after each top-level fenced code block. Fences may interrupt paragraphs; this spacing does not apply inside lists or quotes, or after an unclosed fence. |

When a fence never closes, bob leaves the opening line and everything after it untouched
and reports `unclosed-fence`.

## Findings

Findings are reports, never automatic fixes. Each has `code`, `severity`, a 1-based
`line`, `message`, and `hint`; bob owns all display strings.

| Code | Severity | What it reports |
| --- | --- | --- |
| `pseudo-footnote` | warning | A column-zero `[label]: …` definition that is not a `[^label]:` footnote and whose first destination token is not URL/path-like. A destination is excluded if it contains `://`, begins with `mailto:` (case-insensitive), `#`, `/`, `./`, or `../`. |
| `broken-heading-link` | warning | An in-note heading link matches no section. |
| `ambiguous-heading-link` | warning | An in-note heading link matches more than one section. |
| `inbound-heading-link` | warning | Another note links to a heading this prose note no longer has; the finding names the source `path:line`. |
| `deep-section` | info | A section is deeper than depth 4. |
| `unclosed-fence` | warning | A code fence never closes. |
| `title-heading` | info | The first section repeats the file name or `title:`; Obsidian already displays the inline title. |
| `number-replaced` | warning | A replaced token has a different number of parts than the section depth. Escape literal text as `1\.5`. |
| `setext-multiline` | warning | A Setext heading spans multiple lines and is left unchanged. |
| `bad-option` | warning | `section_numbers` is not lowercase `true` or `false`. |

bob never reflows paragraphs, title-cases headings, applies smart typography, aligns
table pipes, normalizes emphasis/list/thematic-break markers, moves footnotes, adds block
IDs, builds a table of contents, reorders sections, or edits frontmatter except for F1
and the `type` line added by `adopt`.

## Heading identity and links

A heading identity removes one bob-owned number, collapses whitespace, and compares
case-insensitively. bob compares both the raw title and a plain form with emphasis,
inline-code, and wikilink markup stripped. In-note links, inbound links, and prose-note
capture selectors share this identity rule.

For each fragment segment, bob resolves in this order:

1. Match the fragment against a heading's pre-format text. This preserves the target
   Obsidian resolved before a change, including when two headings share a title.
2. If there is no exact match, match the single heading with the same identity. For a
   nested path such as `#A#B`, resolve each later segment inside the previous section's
   subtree.
3. If none match, report `broken-heading-link`; if several identity matches remain,
   report `ambiguous-heading-link`. bob does not guess.

The scanner covers `[[#…]]`, `[[note#…]]`, `[[path/note#…]]` when the target names the
same note, embeds such as `![[note#…]]`, and Markdown links such as `[x](#…)` and
`[x](note.md#…)`. It changes only the leading number token in a fragment, preserves
aliases and percent encoding, and removes owned tokens when numbering is off. Block
links (`#^id`) are left alone. Links inside frontmatter, code, math, comments, and HTML
blocks are not scanned.

## Word invariant and idempotency

Before any write, bob compares the ordered non-whitespace tokens in the source and
formatted output. It ignores heading markup, closing hashes, and one owned section
number on section-heading lines; it drops Setext underline lines and removes owned
number tokens from heading-link fragments. If the sequences differ, bob refuses the
write with `words-changed` and identifies the first differing token in context. bob
formats the result a second time; if that pass changes it, bob reports
`not-idempotent` and writes nothing.

## Prose sections: extract and inline

One gesture turns an authored section of an area, project, or prose note back into a
prose note and folds it back. `bob prose extract` moves a heading section into a new
prose note, leaving the heading and a one-link stub bullet behind; `bob prose inline`
splices a prose note's body back into the unique parent section whose stub links to
it, then trashes the note. Both preview by default and write only with `-w`, through
one verified `vault_rewrite` commit. A prose section is derived extraction
eligibility, never a stored section type: the host note keeps its role, and bob never
formats an area or project note.

### Terms

- **Host note:** an area note, a project note, or a prose note (either spelling), and
  nothing else. Notebooks, albums, records, refs (by type or ref folder), daily and
  period notes, mail notes, untyped notes, mixed-type notes, `done/`, and the excluded
  directories (`_conflicts`, `_templates`, `_generated`, `.obsidian`, `.git`, `.trash`,
  `.bob`) refuse with `not-host-note`.
- **Heading:** an ATX or single-line Setext heading that the real classifier counts in
  the top-level body (N1). A fake heading in a fence, callout, comment, or list body is
  never a boundary.
- **Prose section:** a heading H at level L (1–6) plus its subtree, running to the line
  before the next heading with original level ≤ L, or to end of file. Its **body** is
  everything after H's line (after the underline, for Setext). Blank lines at the
  body's start and end are **seam blank lines**.
- **Stub:** a section whose body, ignoring seam blank lines, is exactly one top-level
  bullet holding one link (`[[stem]]`, `[[stem|alias]]`) that resolves to a prose
  note. The stub keeps the heading byte for byte and reads as a one-note drawer.

### Selection

- `-s, --section IDENTITY` uses the heading identity (an exact raw-title match comes
  first, then a single identity match), including `#A#B` nested paths.
  - No match refuses with `no-section` and lists close matches.
  - Several matches refuse with `ambiguous-section` and list their lines.
- `-l, --line N` selects the innermost heading whose section contains line N; the
  heading line itself counts. A line in the frontmatter or above the first heading is
  `no-section`: type a heading above the text, then extract it.
- Exactly one selector is required.

### Eligibility refusals

Checks run in this order, and the first failure is reported. Each refusal carries a
1-based line (when it has one), a message, and a hint. An ineligible context never
falls back to converting the surrounding note.

| Code | Condition |
| --- | --- |
| `not-host-note` | The note is not a host note (see Terms). |
| `no-section` / `ambiguous-section` | See Selection. |
| `structural-section` | In an area or project note, H, an ancestor of H, or a heading inside the subtree is structural: `is_structural_h2_key` (Tasks and everything under it, status groups, Pomodoros, Events, Highlights, Snoozed, the generated catalogs), the records index, or a project note's `## Notes` home. Also refused: any subtree intersecting a bob-owned region (`owned_regions` of every catalog spec, a status-group badge row, or a `<!-- bob:` marker block), in any host. |
| `holds-tasks` / `holds-anchor` / `conflict-markers` | `marker::body_refusals` over the subtree's classified lines, with the formatter's masking, worded for a section rather than a note. |
| `stub-section` | H's section is already a stub for a prose note whose `parent` is this note. The refusal's `next` names the inline of that note. |
| `unclosed-fence` | A fence opens in the subtree and never closes. |
| `setext-multiline` | A multi-line Setext heading lies inside the range, so bob cannot place the boundary. |
| `owned-number-heading` | The host is an area or project note, and a moved sub-heading's title starts with an N6 token, such as `### 1.5 million users`. The new note's numbering-off pass would strip it. Hint: write `1\.5`. |
| `definition-boundary` | A footnote label (`[^x]`) or reference-link label is used on one side of the boundary and defined on the other. Labels case-fold as in CommonMark, and definitions are document-global. |
| `depth-overflow` | A promoted heading level would fall outside 2–6. This is a defensive check, because promotion never deepens a heading. |

Lists, bullet-only sections, plain checkboxes, tables, quotes, code, and empty sections
are all eligible. Bullet-only sections can start as bullets and graduate to prose.

### Extract (`bob prose extract`)

| Rule | Behavior |
| --- | --- |
| X1 Name | The stem is `<owner>_<slug>`, where the slug is the de-numbered title, lowercased, with each run of characters outside `[a-z0-9]` replaced by `_`, trimmed, and capped at 60 characters. `-n, --name STEM` overrides it. The note goes in the owner's folder, so relative attachment links stay valid. A case-insensitive `.md` collision refuses `name-taken`. An empty or invalid stem refuses `bad-name`. |
| X2 Frontmatter | Keys in this order: `parent: "[[owner]]"` (the `link_destination` form), `type: "[[prose]]"`, `created:` (`format_created_timestamp`), `title:` (H's title bytes exactly as written, de-numbered only when the host is a prose note), `extracted_from: "[[owner]]"`, `extracted_heading:` (the same bytes as `title`), and `section_numbers: false`. Strings use `yaml_string_scalar`. |
| X3 Body | The body lines are kept verbatim. Every heading in them is promoted by L − 1, so H's children become `##`. Moved owned number tokens drop: in a prose host they are bob's (N6), and in other hosts they already refused. The new note then gets its prose format pass, with numbering off, in the same plan. The preview counts its layout edits (`counts.layout`); those and link rewrites are the only other byte changes. Line endings and BOM follow F8. |
| X4 Stub | H's line stays byte for byte. Its body becomes one blank line, the stub bullet `- [[stem\|Title]]` (with no alias when the title contains `\|`, `[[`, or `]]`), then one blank seam line before whatever follows. When the host is a prose note, the host's format pass joins the plan so its numbers and in-note links stay current. |
| X5 Links | Apply the link relocation below to the host, the new note, and every linking note in the vault. |
| X6 Invariants | The word-token invariant holds across the plan. The host before equals the host after with the stub replaced by the new note's body. The comparison ignores heading markers, owned number tokens, and link spans, which are compared through the relocation map. Each linker's tokens are unchanged outside rewritten link spans. Formatting the new note again is a no-op. The written stub must classify as `stub-section`. |
| X7 Focus | `focus` names the new note's first body line. |

### Inline (`bob prose inline`)

| Rule | Behavior |
| --- | --- |
| I1 Note | The note must be a prose note (`not-prose`) with no conflict markers (`conflict-markers`) and not in an excluded directory. |
| I2 Parent | `parent` must resolve to exactly one host note (`no-parent` / `parent-not-host`). |
| I3 Children | No note may name this note as its `parent` (`has-children`, listing up to five). |
| I4 Frontmatter | These keys are absorbed: `parent`, `type`, `created`, `title`, `section_numbers`, `template`, `extracted_from`, `extracted_heading`. Any other key refuses `unknown-frontmatter` and lists the keys. If `title` no longer matches the stub heading's identity, the plan carries a `title-dropped` warning finding. |
| I5 Stub | Exactly one section of the parent, at any level, must be a stub resolving to this note (`stub-missing` / `stub-ambiguous`). Provenance is not required: a note without provenance (root prose) inlines only into a stub someone wrote by hand. |
| I6 Depth | The shift is L − 1, where L is the stub heading's level. Every heading level h in the note must satisfy 2 ≤ h and h + L − 1 ≤ 6. Otherwise inline refuses `depth-overflow` and names the line. |
| I7 Body | When the note's numbering is on, strip owned number tokens (N6/N9) first. Then demote by L − 1 and splice the body into the stub. The heading line is unchanged, and each seam gets one blank line. When the parent is a prose note, its format pass joins the plan. |
| I8 Collisions | A block id in the note that the parent already uses outside the stub refuses `block-id-collision`. A footnote or reference label defined in both notes refuses `definition-collision`. |
| I9 Links and trash | Apply the inverse relocation, and trash the note through `vault_rewrite`'s trash operation. `focus` names the parent's stub heading line. |

### Link relocation

Extract maps the moved headings and block ids to the new note; inline applies the
inverse. T is the kept heading, S is a moved heading, and `^id` is a moved block id.

| Link | Found in | Extract writes | Inline writes |
| --- | --- | --- | --- |
| `[[#S]]`, `[[#^id]]` | the host, outside the move | `[[stem#S]]`, `[[stem#^id]]` | pathless again |
| `[[#Other]]`, `[[#^kept]]`, `[[#T]]` | the moved text | `[[owner#Other]]` and so on | pathless again |
| `[[#S2]]` between two moved headings | the moved text | unchanged | unchanged |
| `[[owner#S]]`, `[[owner#^id]]`, `![[owner#S]]`, `[x](owner.md#S)` | any other note | the path becomes the stem, and the fragment becomes S without an owned number | `[[owner#S]]`, using the shortest fragment that resolves uniquely (`T#S` only when needed) |
| `[[owner#T]]` | anywhere | unchanged (it lands on the stub) | unchanged |
| `[[stem]]`, `[[stem\|alias]]`, `![[stem]]` | anywhere except the stub itself | — | `[[owner#T]]`, keeping the alias and embed |
| `[[owner]]` | anywhere | unchanged | unchanged |

Preserved: the alias, the embed `!`, display text, percent-encoding, the `.md` suffix
style, and stem versus path form. A link whose result lands in the note that contains
it is written pathless. Links inside frontmatter, code, math, comments, and HTML
blocks are never scanned; neither are the `.bob/`, `.trash/`, `_conflicts`,
`.obsidian`, and `.git` directories, or non-UTF-8 files. A fragment matching both a
moved heading and one that stays refuses `ambiguous-link`, naming `path:line`.

### Losslessness

- **L1:** take an eligible section S in a layout-clean host, meaning the prose format
  pass would not change S's promoted body, with no owned-number ambiguity.
  `inline(extract(S))` restores the host byte for byte, apart from bob-owned number
  tokens (a prose host renumbers) and seam blank lines.
- **L2:** after a round trip, every link in every other note resolves to the same
  heading or block as before. Fragment spelling may canonicalize, for example
  `[[owner#T#S]]` → `[[owner#S]]`.
- **L3:** anything that would be lost either refuses or appears in the preview: layout
  normalization (`counts.layout`), unknown frontmatter keys, a dropped title, and
  collisions.

### Provenance and write safety

A derived prose note records `extracted_from: "[[owner]]"` and
`extracted_heading:` (H's title bytes, the same bytes as `title:`). These keys are
what the later reversible-projects work will read; inline absorbs them (I4) and needs
no provenance of its own (I5).

Preview and write share one planner (W1). A write plans again from disk after
pre-sync. The fingerprint is the sha256 of the inputs only: each touched path with
its before-image sha256 (or `absent`), plus the resolved selector (heading and end
line), the stem, and the stub form; it never includes the `created:` timestamp (W2).
Pass `-e FINGERPRINT` to refuse with `plan-changed` when the replanned fingerprint
differs. Writes go through `vault_rewrite::run`, with trash, no-replace creates, and
the intent journal; the verify hook re-reads every written file and re-checks X6 or
I5–I9 on the written bytes. Undo is the inverse command, or `git revert` of the one
commit (W3). The whole conversion is all-or-nothing: it aborts on any drifted file
(`stale_note`) and is never half applied (W4). Commit subjects are
`bob prose extract: <owner> § <title> → <stem>` and
`bob prose inline: <stem> → <owner> § <title>` (W5).

### Toggle dispatch

The Obsidian gesture is a thin client that decides only from bob's JSON:

1. It runs `bob prose extract PATH -l <cursor line> -f json`.
2. On a preview, it opens the extract modal.
3. On a refusal whose `next` is set, it runs that command's preview and opens the
   inline modal. bob sets `next` in two cases: a `stub-section` refusal names the
   stub's note, and `no-section` in a prose note that has a parent names the note
   itself, so the cursor above a prose note's first heading inlines the note.
4. On any other refusal, it shows bob's message and hint.

## Write safety and vault scanning

`format` and `adopt -w` check each changed prose note's SHA-256 preimage before writing. A
note changed after planning gets status `error`, reason code `preimage-changed`, and a
message saying it was not written. Other changed prose notes in the batch still write;
the command exits 2 if any note was skipped or errored. Human output and JSON use the
post-write status, so a skipped write never appears as successful.

The prose-note writes use an atomic temporary-file replacement. For inbound repair, bob
writes changed prose notes first (folding cross-note repairs into those writes),
then scans and atomically writes other linking notes after checking each linker's
preimage. A drifted inbound note is skipped with a reason; the prose-note write stands.
The drifted prose note's inbound rewrites are skipped. Non-UTF-8 Markdown files are
treated as untyped notes and non-linkers; other I/O errors fail the run. Excluded directories
are not scanned.

Inbound repair runs on write when a heading changes. `format -a` also scans for stale
links from earlier runs. The scan prefilters notes by case-insensitive prose-note-stem
substring, then resolves links against the vault. `--dry-run` shows inbound diffs;
`--stdin` returns rewrites in the plan without writing.

## CLI

A bare `bob prose` runs the read-only `list` command.

| Command | Effect |
| --- | --- |
| `bob prose` or `bob prose list` | List prose-note sections, words, TODO stubs, findings, and state. |
| `bob prose adopt NOTE` | Preview adding the marker and formatting. |
| `bob prose adopt -w NOTE` | Write adoption and formatting. |
| `bob prose format NOTE...` | Format named prose notes; writes by default. |
| `bob prose format -a` | Format all discovered prose notes. Valid changed notes write even if another note is refused. |
| `bob prose outline NOTE` | Show the section outline and word counts. |
| `bob prose extract NOTE -s SECTION` | Preview moving a heading section into a new prose note, leaving a link stub. |
| `bob prose extract -w NOTE -s SECTION` | Write the extraction through one verified commit. |
| `bob prose inline NOTE` | Preview folding a prose note back into the parent section that links to it. |
| `bob prose inline -w NOTE` | Write the inline, trashing the prose note through one verified commit. |

`NOTE` accepts an exact vault-relative Markdown path or a unique note stem. Ambiguous
names list candidates; missing names list close stem matches where available. `-b,
--bob-dir PATH` selects the vault, otherwise bob uses `BOB_DIR` or `~/bob`.

`format` options: `-a, --all`; `-b, --bob-dir PATH`; `-c, --check` (write nothing,
exit 1 if formatting is needed); `-d, --dry-run` (show a unified diff, write nothing);
`-f, --format human|json`; `-n, --section-numbers on|off` (set the frontmatter value
and format in one plan; conflicts with `--all` and `--check`); `-s, --stdin` (read one
named note from stdin, print its plan, and write nothing). `--stdin` cannot be combined
with `--all` or `--dry-run`, and can be combined with `--section-numbers`.
`adopt` options are `-b`, `-f`, `-s`, and `-w, --write`; preview is the default. An
already-adopted prose note is a successful no-op. Adoption preview includes a diff,
`Numbers replaced`, findings, and a write hint.

`extract` options: `-b, --bob-dir PATH`; `-e, --expect FINGERPRINT` (refuse with
`plan-changed` when the replanned fingerprint differs; write nothing);
`-f, --format human|json`; `-l, --line N` (extract the section containing line N);
`-n, --name STEM` (override the derived prose-note stem); `-o, --offline` (commit
locally; skip vault sync before and after); `-s, --section IDENTITY` (heading identity
or nested `Parent#Child` path); `-w, --write` (write through one verified commit;
preview without it). Exactly one of `-s` and `-l` is required. `inline` options are
`-b`, `-e`, `-f`, `-o`, and `-w` with the same meanings. Online writes need
`-b/--bob-dir` and `BOB_DIR` to resolve to the same vault; a mismatch refuses before
lock or sync. Both commands exit 0 on a preview or write, and 2 on a refusal, usage
error, or write failure.

Exit codes:

| Command | 0 | 1 | 2 |
| --- | --- | --- | --- |
| `format` | Success, including no changes. | `--check` found at least one changed prose note. | Usage error, refusal, or formatting/write error. |
| `adopt` | Adoption succeeded or the note was already a prose note. | — | Usage error, refusal, or formatting/write error. |
| `list`, `outline` | Success. | — | Usage/resolution error. |

Human format output is silent when no work is needed, except a section-number toggle
reports `unchanged` when its requested value is already set. Changed notes get a summary line;
warnings and info findings use distinct prefixes. `--check` prints which notes need
formatting. The list header uses `~` for the home directory, and singular/plural wording
follows the counts. Nightly runs the report-only `format -a -c` check after task archive;
it marks the step passed unless bob exits 2 or higher.

## JSON: ArticlePlan v1

`format -f json` and `adopt -f json` print the complete response, including when a note
is skipped or errors:

```json
{
  "schema_version": 1,
  "notes": [{
    "path": "sase_blog_contents.md",
    "status": "changed",
    "reason": null,
    "sha256": "<input text SHA-256>",
    "edits": [{ "line": 18, "delete": 1, "insert": ["# 1. The Abstractions", ""] }],
    "changes": {
      "numbered": 10, "renumbered": 0, "unnumbered": 0,
      "promoted": 2, "setext": 0, "links": 0, "layout": 9
    },
    "replaced": [{ "line": 30, "token": "3.", "number": "1.2" }],
    "notice": "Prose note formatted · 10 sections numbered",
    "findings": [],
    "outline": { "title": "sase_blog_contents", "numbering": true, "words": 253,
      "preamble": {}, "sections": [] },
    "inbound": []
  }],
  "summary": { "articles": 1, "changed": 1, "unchanged": 0, "skipped": 0, "errors": 0 }
}
```

Statuses are `changed`, `unchanged`, `skipped`, and `error`. `reason` is null or has a
`code` and `message`. `changes.numbered` counts headings with no written token that get
a number; `renumbered` counts written tokens that change. `formatted` is intentionally
omitted: clients apply `edits`. The contract only grows; clients must reject an unknown
`schema_version` and must not compute numbers or display strings themselves.

`list -f json` uses the same response and removes each note's `edits`. `outline -f json`
prints the `outline` object. In an outline, `line`/`end_line` refer to input lines,
`output_line` refers to formatted text, `title` is de-numbered, `written` is the source
token or null, `number` is bob's result or null when numbering is off, `stale` means the
source differs, and `stub` means the section body is empty or contains only TODO/TBD
markers. `replaced` lists every owned number removed or replaced. In `format --stdin`,
`inbound` entries contain `{path, line, sha256, before, after}` for the linking note.

## JSON: ProseConversion v1

`extract -f json` and `inline -f json` print the complete ProseConversion response,
including when a note is refused:

```json
{
  "schema_version": 1,
  "contract": "prose-conversion",
  "command": "extract",
  "status": "preview",
  "fingerprint": "sha256:1db9…",
  "note": "probe",
  "section": { "line": 12, "end_line": 14, "level": 2, "title": "Intro" },
  "target": { "path": "probe_intro.md", "stem": "probe_intro" },
  "changes": [{ "path": "probe.md", "action": "update",
    "sha256": "c223a…", "diff": "--- a/probe.md\n…" }],
  "links": [],
  "counts": { "notes": 2, "links": 0, "layout": 1, "replaced": 0 },
  "findings": [],
  "refusal": null,
  "focus": { "path": "probe_intro.md", "line": 11 },
  "notice": "Extracted “Intro” into probe_intro · 0 links repaired",
  "commit": null,
  "sync": { "pre": "skipped", "post": "skipped" },
  "warnings": []
}
```

`status` is `preview`, `written`, or `refused`. A refusal carries `code`, `message`,
`hint`, and 1-based `path`/`line` where they apply; toggle dispatch reads `next`
(`{command, note}`) when it is set. `section` names the resolved extract section, and
`target` names the note the command writes (extract) or folds into (inline).
`changes[]` carries per-file unified diffs with `action` `update`, `create`, or
`trash`; inline's trash entry holds the note's before-image. `counts.links` counts
repaired link spans, `counts.layout` counts the new note's format-pass edits, and
`counts.replaced` counts replaced owned numbers. `focus` names where the client
should land: the new note's first body line after extract, the parent's stub heading
line after inline. `commit` carries the one commit's sha and subject on a write.
`sync` reports the vault-sync pre/post state (`skipped` with `-o`). `findings` holds
`title-dropped` and siblings; `warnings` is reserved for additive strings.

ArticlePlan is unchanged: it keeps `schema_version: 1`, the `summary.articles` key,
and the `not-article` code. The two contracts version separately, and clients must
reject an unknown `schema_version` on either.

## Plugin behavior

The plugin never parses prose-note grammar or computes numbers. On save, when leaving a
note, or through **Bob: Format prose note**, it sends the buffer to bob and applies the
returned edits in one CodeMirror transaction. It checks the buffer and inbound line
preimages before writes and suppresses reformatting after an undo until fresh input. The
**Bob: Convert note to prose…** command shows bob's adoption preview and refusal.
Reading and Live Preview decorate written numbers from bob's outline; stale-number
tooltips avoid promising an on-save change while the undo guard blocks saves.

The plugin applies the `.bob-article` reading look, line numbers, and responsive hanging
section numbers from bob's outline. Refreshing an outline rerenders open Reading views
once. It shows at most one quiet missing/old-bob or timeout notice per session, and
save-path refusals at most once per note per session.

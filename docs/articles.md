# Article notes

An article is a long-form Bob vault note marked with `type: "[[article]]"`. bob-cli
owns article numbering, link repair, and layout. The Obsidian `bob-articles` plugin sends
the current buffer to bob and applies bob's returned line edits as one undoable
transaction. Formatting changes layout and heading numbers; it never changes prose.

## Marker, options, and refusals

```yaml
---
parent: "[[sase_blog]]"
type: "[[article]]"
created: 2026-10-15T15:10:41-04:00
title: "Why I built SASE" # optional publication title; defaults to file name
section_numbers: true     # optional; true by default
---
```

Only `type: "[[article]]"` enables formatting. `title` feeds the repeated-title
finding and the `list`/`outline` labels. `section_numbers` accepts lowercase `true` or
`false`; any other value produces a `bad-option` finding on that line and leaves
numbering on. The plugin's `.bob-article` class is runtime-only and is never written to
frontmatter.

Formatting skips the whole note in these cases; it never formats only part of a note.

| Code | Condition and response |
| --- | --- |
| `not-article` | The `type` does not include `[[article]]`; run `bob articles adopt`. |
| `frontmatter-unclosed` | The opening `---` has no closing delimiter. |
| `mixed-type` | The type also names another kind, such as `[[project]]`. |
| `holds-anchor` | The note contains a `^prj` or `^ref` block anchor; articles cannot be projects or references. |
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
| `inbound-heading-link` | warning | Another note links to a heading this article no longer has; the finding names the source `path:line`. |
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
inline-code, and wikilink markup stripped. In-note links, inbound links, and article
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

## Write safety and vault scanning

`format` and `adopt -w` check each changed article's SHA-256 preimage before writing. A
note changed after planning gets status `error`, reason code `preimage-changed`, and a
message saying it was not written. Other changed articles in the batch still write;
the command exits 2 if any note was skipped or errored. Human output and JSON use the
post-write status, so a skipped write never appears as successful.

The article writes use an atomic temporary-file replacement. For inbound repair, bob
writes changed article notes first (folding cross-article repairs into those writes),
then scans and atomically writes other linking notes after checking each linker's
preimage. A drifted inbound note is skipped with a reason; the article write stands.
The drifted article's inbound rewrites are skipped. Non-UTF-8 Markdown files are treated
as non-articles and non-linkers; other I/O errors fail the run. Excluded directories
are not scanned.

Inbound repair runs on write when a heading changes. `format -a` also scans for stale
links from earlier runs. The scan prefilters notes by case-insensitive article-stem
substring, then resolves links against the vault. `--dry-run` shows inbound diffs;
`--stdin` returns rewrites in the plan without writing.

## CLI

A bare `bob articles` runs the read-only `list` command.

| Command | Effect |
| --- | --- |
| `bob articles` or `bob articles list` | List article sections, words, TODO stubs, findings, and state. |
| `bob articles adopt NOTE` | Preview adding the marker and formatting. |
| `bob articles adopt -w NOTE` | Write adoption and formatting. |
| `bob articles format NOTE...` | Format named articles; writes by default. |
| `bob articles format -a` | Format all discovered articles. Valid changed notes write even if another note is refused. |
| `bob articles outline NOTE` | Show the section outline and word counts. |

`NOTE` accepts an exact vault-relative Markdown path or a unique note stem. Ambiguous
names list candidates; missing names list close stem matches where available. `-b,
--bob-dir PATH` selects the vault, otherwise bob uses `BOB_DIR` or `~/bob`.

`format` options: `-a, --all`; `-b, --bob-dir PATH`; `-c, --check` (write nothing,
exit 1 if formatting is needed); `-d, --dry-run` (show a unified diff, write nothing);
`-f, --format human|json`; `-s, --stdin` (read one named note from stdin, print its
plan, and write nothing). `--stdin` cannot be combined with `--all` or `--dry-run`.
`adopt` options are `-b`, `-f`, `-s`, and `-w, --write`; preview is the default. An
already-adopted article is a successful no-op. Adoption preview includes a diff,
`Numbers replaced`, findings, and a write hint.

Exit codes:

| Command | 0 | 1 | 2 |
| --- | --- | --- | --- |
| `format` | Success, including no changes. | `--check` found at least one changed article. | Usage error, refusal, or formatting/write error. |
| `adopt` | Adoption succeeded or the note was already an article. | — | Usage error, refusal, or formatting/write error. |
| `list`, `outline` | Success. | — | Usage/resolution error. |

Human format output is silent when no work is needed. Changed notes get a summary line;
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
    "notice": "Article formatted · 10 sections numbered",
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

## Plugin behavior

The plugin never parses article grammar or computes numbers. On save, when leaving a
note, or through **Bob: Format article**, it sends the buffer to bob and applies the
returned edits in one CodeMirror transaction. It checks the buffer and inbound line
preimages before writes and suppresses reformatting after an undo until fresh input. The
**Bob: Convert note to article…** command shows bob's adoption preview and refusal.
Reading and Live Preview decorate written numbers from bob's outline; stale-number
tooltips avoid promising an on-save change while the undo guard blocks saves.

The plugin applies the `.bob-article` reading look, line numbers, and responsive hanging
section numbers from bob's outline. Refreshing an outline rerenders open Reading views
once. It shows at most one quiet missing/old-bob or timeout notice per session, and
save-path refusals at most once per note per session.

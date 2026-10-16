# Article notes

`bob articles` inspects and formats Markdown notes with an explicit
`type: "[[article]]"` frontmatter value. It is the only CLI implementation of
article formatting; the engine numbers every section as `1.` / `1.1` / `1.1.1`,
refreshes in-note heading links, and changes layout without changing the words.

A bare `bob articles` runs the read-only list view.

## Commands

| Command | Effect |
| --- | --- |
| `bob articles` or `bob articles list` | List article paths, section and word counts, TODO stubs, findings, and formatting state. |
| `bob articles outline NOTE` | Show the article's numbered outline and word counts. |
| `bob articles format NOTE...` | Format one or more articles and write by default. |
| `bob articles format --all` | Format every discovered article in the vault. |

`NOTE` accepts a vault-relative `.md` path or a unique note stem. A path is preferred
when names are ambiguous. `--bob-dir PATH` (`-b`) selects the vault; otherwise bob
uses `BOB_DIR` or `~/bob`.

## Format options

| Option | Effect |
| --- | --- |
| `--all`, `-a` | Select every discovered article instead of NOTE arguments. |
| `--bob-dir PATH`, `-b` | Select the vault root. |
| `--check`, `-c` | Write nothing; exit 1 when any article would change. |
| `--dry-run`, `-d` | Print a colored unified diff and write nothing. |
| `--format human\|json`, `-f` | Select human output or the JSON plan. |
| `--stdin`, `-s` | Read one named article from stdin, print its plan, and write nothing. |

Formatting refuses notes that are not articles, have unclosed frontmatter, combine
article with another type, contain a project/reference anchor or task, contain Git
conflict markers, or live under `.git`, `.obsidian`, `_conflicts`, `_generated`, or
`_templates`. A refusal never writes part of a note. `--all` and `--check` can report
refusals in the plan.

Before writing, bob rereads every changed note and checks its SHA-256 preimage against
the plan. If a source changed, it refuses the write. Each accepted result is installed
with a temporary file and atomic rename. Human format output is silent when nothing
changes; warnings appear under changed-note summaries.

## JSON contract

Every `format -f json` mode prints the complete ArticlePlan v1 response, including on
formatting refusals. It contains `schema_version`, `notes`, and `summary`. A note
includes its path, status, reason, source SHA-256, line edits, change counts, replaced
number tokens, bob-owned notice, findings, outline, and inbound rewrites. `formatted`
is intentionally omitted; clients apply the `edits` or use the in-process result.

`list -f json` uses the same response and removes only each note's `edits` field.
`outline -f json` prints the single note's `outline` object. ArticlePlan v1 is
additive: existing fields keep their meaning, and clients must reject unknown schema
versions.

`format` exits 0 on success, 1 when `--check` finds changes, and 2 for usage errors,
refusals, or formatting errors. Vault discovery errors exit 1.

# Highlights Reference Note Sync

`bob highlights-ref` turns Highlights app PDF annotations into Obsidian
reference notes in the Bob vault.

Code lives in this `bob-cli` repository. On the MacBook, use a checkout at
`~/projects/bob-cli`; do not install from ad hoc scripts outside that checkout.

## MVP Status

The MVP implements marker/frontmatter synchronization, Markdown sidecar parsing,
generated note rendering, recursive library scan, prerequisite checks, output
collision detection, dirty-target refusal, and atomic note writes.

`sync <pdf>` loads one PDF with native Rust code, treats the first standalone
`/Text` annotation as the marker note, parses the marker list, creates or
updates `ref/<pdf-basename>.md`, and rewrites only the managed Highlights body
region from the sidecar when one is present. `marker <pdf>` inspects the same
marker without writing.

`scan` recursively finds PDFs under the configured library directory, preflights
all target note paths before writing anything, and then syncs each PDF in stable
path order. It refuses duplicate output paths such as two PDFs with the same
basename that would both write `ref/<basename>.md`.

`doctor` checks vault paths, library/ref directories, sidecar presence, marker
readability, default parent shape, Git worktree status, and optional `ob`
availability. It never writes files.

Available commands:

```bash
bob highlights-ref scan [--dry-run]
bob highlights-ref sync <pdf> [--dry-run] [--write-pdf] [--prefer marker|frontmatter]
bob highlights-ref doctor
bob highlights-ref marker <pdf>
```

`sync <pdf> --dry-run` prints the resolved configuration and planned note/PDF
actions without modifying either side. Without `--dry-run`, the command writes
the reference note when frontmatter changes. It only writes the PDF marker when
frontmatter is the selected source and `--write-pdf` is supplied.

## Default Paths

The Bob vault root is `BOB_DIR`, defaulting to `~/bob`.

PDFs are discovered under the Highlights library directory:

```text
BOB_HIGHLIGHTS_LIB_DIR=lib
```

Reference notes are written under:

```text
BOB_HIGHLIGHTS_REF_DIR=ref
```

Relative `BOB_HIGHLIGHTS_LIB_DIR` and `BOB_HIGHLIGHTS_REF_DIR` values are
resolved under `BOB_DIR`. Absolute paths and `~/...` paths are used directly
after tilde expansion.

For `~/bob/lib/systems-performance.pdf`, the default output note is:

```text
~/bob/ref/systems-performance.md
```

## Marker Note Grammar

The marker note is the first standalone PDF note annotation in PDF order. It is
not identified by a sentinel token.

The marker note is an unordered list of `key: value` pairs:

```text
- status: reading
- parent: [[obsidian]]
- title: Systems Performance
- aliases: ["Systems Performance", "Brendan Gregg systems performance"]
- topics: [linux, performance]
- source_url: https://example.com/book
```

Allowed list markers:

```text
- key: value
* key: value
```

`status` is the only required marker key.

Values should be parsed as a YAML-compatible subset when possible:

- strings
- numbers
- booleans
- nulls
- inline lists

Invalid values fall back to strings when that is unambiguous. Duplicate marker
keys, invalid marker list items, pipeline-owned marker keys, and a missing or
empty `status` produce clear errors.

## Parent Handling

New Markdown notes under `~/bob` require `parent` frontmatter. The marker note
does not require `parent`.

If the marker omits `parent`, the command uses:

```text
BOB_HIGHLIGHTS_DEFAULT_PARENT=[[obsidian]]
```

Override precedence is:

1. Command flag: `--default-parent '[[some note]]'`
2. Environment: `BOB_HIGHLIGHTS_DEFAULT_PARENT`
3. Built-in default: `[[obsidian]]`

## Synced Properties

The PDF marker note and the reference note frontmatter share a synced
user-property projection.

Pipeline/provenance fields are excluded from marker sync:

```text
source_pdf
source_pdf_sha256
highlights_sidecar
highlights_count
highlights_synced_at
highlights_marker_hash
highlights_marker_fields
pipeline_version
```

Unknown marker keys should round-trip into frontmatter. New frontmatter keys
should sync back to the marker only when they are standard supported fields or
explicitly listed in `highlights_marker_fields`.

Implemented conflict policy:

- If only the marker changed, update frontmatter.
- If only frontmatter changed, update the PDF marker note when PDF writes are
  enabled.
- If both changed differently, fail without modifying either side unless
  `--prefer marker` or `--prefer frontmatter` is supplied.

The last synced user-property projection is stored as `highlights_marker_hash`.
`--prefer frontmatter` requires `--write-pdf` whenever the PDF marker must be
updated.

PDF marker writes are performed by saving a temporary PDF next to the original
and renaming it over the target. Before first PDF writes, commit or otherwise
back up the PDF library. Keep Highlights and Obsidian idle while testing PDF
writes so the apps do not race the CLI.

## Scan, Safety, and Git/ob Behavior

`scan --dry-run` reports every PDF it would process, each target reference note,
the sidecar path if present, the selected sync source, and the note/PDF marker
action. It does not create directories, write notes, or write PDFs.

Before a writing scan, the command builds the complete plan for every PDF,
rejects duplicate output paths, and checks Git status for existing vault files
it would modify. If a target ref note or PDF marker target is dirty, it fails
before any write. There is no force mode in the MVP; commit, stash, or clean the
dirty file before rerunning.

Reference note writes are atomic temporary-file renames and are skipped when the
rendered note is byte-identical to the existing file.

`bob highlights-ref` does not run `ob sync` before or after writes. The existing
`bob cronjob` sync gate owns `ob sync` orchestration, while this command only
reports whether `ob` is available through `doctor`.

## Generated Body Contract

Highlights, comments on highlights, and standalone non-marker notes sync one way
from the PDF/sidecar into the reference note.

For `foo.pdf`, sidecar discovery looks for `foo.md` first. If there is no
Markdown sidecar, it recognizes `foo.textbundle` only when the bundle contains
`text.md` or `text.markdown`; other TextBundle contents fail with an explicit
unsupported-sidecar error. If no sidecar exists, `sync` still performs
marker/frontmatter sync. For a new note it creates an empty managed region; for
an existing note it preserves the existing managed region.

The configured Markdown sidecar format is intentionally simple:

- Page labels are Markdown headings such as `## Page 12` or `## p. 12`.
- Annotation spacers are horizontal rules such as `---`.
- A highlight starts with one or more blockquote lines.
- Non-heading text after a highlight is rendered as a highlight comment. A
  leading `Comment:` or `Note:` label is stripped.
- A standalone note is non-heading text outside a blockquote. A leading `Note:`
  label is stripped.
- The first standalone note in sidecar order is treated as the PDF marker mirror
  and is excluded from generated content.

Example sidecar fragment:

```md
## Page 12

Note: marker note mirrored from the PDF

---

> Latency is not throughput.

Comment: Compare this with SLO notes.

---

Note: Keep this standalone observation.
```

The generated region is the only body region the tool owns:

```md
<!-- highlights:begin -->

<!-- highlights:end -->
```

Manual content outside those markers must be preserved. User edits inside the
generated region may be overwritten.

New generated notes include a title, a PDF wikilink, `## Summary`, `## My
Notes`, and `## Highlights`. Existing notes must already contain the managed
begin/end markers; otherwise `sync` fails instead of guessing where generated
content belongs.

Generated blocks use Obsidian block IDs beginning with `^h-`. The MVP ID is a
deterministic content hash over source PDF path, page label, annotation kind,
sidecar order on the page, and quote/note text. Highlight comments are not part
of the hash, so editing only a comment updates the block without changing its
ID. If a previously generated block disappears from the sidecar, the command
keeps the old block ID under `### Removed highlights` with a tombstone message.

## MacBook Setup Outline

Run these steps on the MacBook to test the marker/frontmatter sync against real
Highlights files:

```bash
cd ~/projects
git clone git@github.com:bbugyi200/bob-cli.git bob-cli
cd ~/projects/bob-cli
cargo install --path ~/projects/bob-cli --locked --force
```

Confirm the vault layout:

```bash
mkdir -p ~/bob/lib ~/bob/ref
git -C ~/bob status --short
```

In Highlights Pro on the MacBook:

- Keep PDFs under `~/bob/lib`.
- Enable sidecar autosave next to PDFs.
- Use the Markdown sidecar format documented above: page headings, annotation
  spacers, blockquote highlights, and comment/note paragraphs.
- Add one standalone PDF note as the first note annotation.
- Put at least `- status: reading` in that first standalone note.

Initial command checks:

```bash
bob highlights-ref doctor
bob highlights-ref scan --dry-run
bob highlights-ref sync ~/bob/lib/example.pdf --dry-run
bob highlights-ref marker ~/bob/lib/example.pdf
```

Before first writes:

```bash
git -C ~/bob status --short
```

Commit or stash unrelated vault edits before enabling sync writes. Keep
Highlights and Obsidian idle while PDF marker writes are being tested so the
apps do not race the CLI.

## Automation Outline

The MVP automation target is a scheduled `scan`, not a live recursive watcher.
Start with a dry-run schedule:

```bash
bob highlights-ref scan --dry-run
```

Start with dry runs and enable writes only after reviewing the generated output.

# Highlights Reference Note Sync

`bob highlights-ref` is the planned Bob CLI command for turning Highlights app
PDF annotations into Obsidian reference notes in the Bob vault.

Code lives in this `bob-cli` repository. On the MacBook, use a checkout at
`~/projects/bob-cli`; do not install from ad hoc scripts outside that checkout.

## Phase 1 Status

Phase 1 establishes the command surface, configuration contract, fixtures, and
documentation. The current implementation does not parse PDFs, does not scan the
vault, and does not write PDF or Markdown files.

Available commands:

```bash
bob highlights-ref scan [--dry-run]
bob highlights-ref sync <pdf> [--dry-run] [--write-pdf] [--prefer marker|frontmatter]
bob highlights-ref doctor
bob highlights-ref marker <pdf>
```

Every Phase 1 command prints the resolved configuration and `writes: none`.

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

Invalid values should fall back to strings when that is unambiguous. Duplicate
keys, invalid list items, and a missing `status` should produce clear errors in
the implementation phases.

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

Conflict policy for later phases:

- If only the marker changed, update frontmatter.
- If only frontmatter changed, update the PDF marker note when PDF writes are
  enabled.
- If both changed differently, fail without modifying either side unless
  `--prefer marker` or `--prefer frontmatter` is supplied.

The last synced user-property projection should be stored as
`highlights_marker_hash`.

## Generated Body Contract

Highlights, comments on highlights, and standalone non-marker notes sync one way
from the PDF/sidecar into the reference note.

The generated region is the only body region the tool owns:

```md
<!-- highlights:begin -->

<!-- highlights:end -->
```

Manual content outside those markers must be preserved. User edits inside the
generated region may be overwritten.

## MacBook Setup Outline

Run these steps on the MacBook once implementation phases are complete enough to
test with real Highlights files:

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
- Use the Markdown sidecar format that later parser phases document.
- Add one standalone PDF note as the first note annotation.
- Put at least `- status: reading` in that first standalone note.

Initial command checks:

```bash
bob highlights-ref doctor
bob highlights-ref scan --dry-run
bob highlights-ref sync ~/bob/lib/example.pdf --dry-run
bob highlights-ref marker ~/bob/lib/example.pdf
```

Before first writes in later phases:

```bash
git -C ~/bob status --short
```

Commit or stash unrelated vault edits before enabling sync writes. Keep
Highlights and Obsidian idle while PDF marker writes are being tested so the
apps do not race the CLI.

## Automation Outline

The MVP automation target is a scheduled `scan`, not a live recursive watcher.
A later phase should provide a LaunchAgent or cron snippet equivalent to:

```bash
bob highlights-ref scan --dry-run
```

Start with dry runs and enable writes only after reviewing the generated output.

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
`/Text` annotation on page 1 as the marker note, parses the marker list,
creates or updates `ref/<ref_type>/<pdf-basename>.md` for PDFs under
`lib/<ref_type>/`, and rewrites only the managed Highlights body region from the
sidecar when one is present. Top-level library PDFs and explicit out-of-library
syncs keep the legacy `ref/<pdf-basename>.md` target. `marker <pdf>` inspects
the same marker without writing.

`scan` recursively finds PDFs under the configured library directory, preflights
all target note paths before writing anything, and then syncs each PDF in stable
path order. It refuses duplicate output paths such as two PDFs that would both
write the same `ref/<ref_type>/<basename>.md` target.

`doctor` checks vault paths, library/ref directories, sidecar presence, marker
readability, Git worktree status, and optional `ob` availability. It never
writes files.

Available commands:

```bash
bob highlights-ref doctor
bob highlights-ref marker <pdf>
bob highlights-ref scan [--dry-run]
bob highlights-ref sync <pdf> [--dry-run] [--write-pdf] [--prefer marker|frontmatter]
```

`sync <pdf> --dry-run` prints the resolved configuration and planned note/PDF
actions without modifying either side. Without `--dry-run`, the command writes
the reference note when frontmatter changes. It only writes the PDF marker when
frontmatter is the selected source and `--write-pdf` is supplied.

## Release Handoff Summary

The MVP is ready for Linux-side release checks and MacBook dry-run validation.
It includes the native `bob highlights-ref` command, synthetic PDF marker
fixtures, frontmatter/marker conflict detection, generated highlight rendering,
recursive scan preflights, dirty target refusal, MacBook setup guidance, and
scheduled dry-run automation examples.

Known risks to validate on the MacBook:

- Real Highlights sidecar Markdown may vary from the fixture-backed parser
  contract. Keep the documented Highlights Note Format settings fixed while
  testing.
- PDF marker write-back is implemented with native PDF annotation writes, but
  real Highlights-authored files may expose annotation shapes not covered by
  Linux fixtures. Use `--write-pdf` only after backing up the target PDF.
- Scheduled `scan` should stay note-only. Keep PDF marker write-back as a
  targeted manual action until real-file behavior is trusted.
- `~/bob/lib` and `~/bob/ref` are the MVP defaults even though this Linux host
  has an observed `~/bob/lit` path.

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

For `~/bob/lib/books/systems-performance.pdf`, the default output note is:

```text
~/bob/ref/books/systems-performance.md
```

The first path component below the library directory is the reference type. For
the example above, generated frontmatter includes:

```yaml
ref_type: books
```

Deeper paths preserve the full library-relative path in `ref/` while keeping the
first component as `ref_type`:

```text
~/bob/lib/books/performance/systems-performance.pdf
~/bob/ref/books/performance/systems-performance.md
```

Top-level library PDFs remain supported for existing files and do not derive a
`ref_type`:

```text
~/bob/lib/systems-performance.pdf
~/bob/ref/systems-performance.md
```

## Marker Note Grammar

The marker note is the first standalone `/Text` PDF note annotation on page 1.
It is not identified by a sentinel token, and standalone notes on later pages
are not marker candidates.

The marker note is an unordered list of `key: value` pairs:

```text
- status: wip
- parent: obsidian
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

`status` and `parent` are required marker keys. `parent` may be a bare note
target such as `obsidian` or `Systems Performance`; existing wikilinks such as
`[[obsidian]]` are also accepted. Generated reference notes render `parent` as
an Obsidian wikilink.

`status` must be a scalar string and must exactly match one of:

- `unread`
- `wip`
- `done`
- `abandoned`
- `legacy`

Values should be parsed as a YAML-compatible subset when possible:

- strings
- numbers
- booleans
- nulls
- inline lists

Invalid values fall back to strings when that is unambiguous. Duplicate marker
keys, invalid marker list items, pipeline-owned marker keys, command-managed
marker keys such as `type` or `ref_type`, and missing or empty `status` or
`parent` produce clear errors. Unsupported `status` values are also rejected.

## Required Parent, Type, and Ref Type

New Markdown notes under `~/bob` require `parent` frontmatter. The PDF marker
is authoritative for `parent`, so the marker must include it.

Generated reference notes also always include command-managed frontmatter:

```yaml
type: "[[ref]]"
```

Nested library PDFs also include path-derived frontmatter:

```yaml
ref_type: books
```

`type` and `ref_type` are replaced on each successful sync, excluded from
marker/frontmatter round-trip hashes, and rejected if either appears in the PDF
marker note. `ref_type` is omitted for top-level library PDFs and explicit
out-of-library syncs because there is no library category to derive.

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
highlights_marker_base
highlights_marker_hash
highlights_marker_fields
pipeline_version
```

The command-managed `type` and `ref_type` fields are also excluded from marker
sync. `type` is always rendered as `[[ref]]`; `ref_type` is rendered only when
the PDF path is under `lib/<ref_type>/`.

Unknown marker keys should round-trip into frontmatter. New frontmatter keys
should sync back to the marker only when they are standard supported fields or
explicitly listed in `highlights_marker_fields`.

`legacy_status` is ordinary frontmatter used to preserve a migrated note's old
status value. It is preserved when rendering notes, but it is not a standard
marker-synced field and will not be written back to the PDF marker unless a
marker explicitly opts into it as an unknown synced field.

Implemented conflict policy:

- If only the marker changed, update frontmatter.
- If only frontmatter changed, update the PDF marker note when PDF writes are
  enabled.
- If marker and frontmatter changed different fields from the stored base,
  auto-merge them. PDF marker writes are still opt-in with `--write-pdf`.
- If both changed the same field differently, fail without modifying either side unless
  `--prefer marker` or `--prefer frontmatter` is supplied.

The last synced user-property projection is stored as `highlights_marker_hash`
and as compact JSON in `highlights_marker_base`. The hash keeps old-note
compatibility; the base snapshot lets the command prove safe field-level
merges. `--prefer frontmatter` and any auto-merge that includes frontmatter
changes require `--write-pdf` whenever the PDF marker must be updated.

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

Two Markdown sidecar shapes are supported. The simple shape is:

- Page labels are Markdown headings such as `## Page 12` or `## p. 12`.
- Annotation spacers are horizontal rules such as `---`.
- A highlight starts with one or more blockquote lines.
- Non-heading text after a highlight is rendered as a highlight comment. A
  leading `Comment:` or `Note:` label is stripped.
- A standalone note is non-heading text outside a blockquote. A leading `Note:`
  label is stripped.
- The first standalone note in sidecar order is treated as the PDF marker mirror
  and is excluded from generated content.

Simple sidecar fragment:

```md
## Page 12

Note: marker note mirrored from the PDF

---

> Latency is not throughput.

Comment: Compare this with SLO notes.

---

Note: Keep this standalone observation.
```

The linked-page Highlights export shape is also supported:

- Page labels may be Markdown-link headings such as
  `#### [Page 1](highlights://book#page=1)`. The link label, `Page 1`, is used
  as the rendered page heading.
- Non-page headings such as the document title and dated metadata headings are
  ignored by the annotation parser.
- Annotation spacers may use any Markdown horizontal rule style, including
  `***`.
- Highlights may be hard-wrapped so that only the first physical line starts
  with `>`. Immediate nonblank continuation lines are kept as highlight text,
  but marker-list fields and explicit `Comment:`/`Note:` labels start the
  comment side instead.
- User comments exported as Markdown list items after a highlight are rendered
  without the list marker.
- A blockquoted marker mirror title followed by marker-list fields such as
  `status` and `parent` is excluded from generated content.

Linked sidecar fragment:

```md
# Highlights Reference Note Sync

#### [Page 1](highlights://highlights-ref-sync#page=1)

##### 2026-06-03:

> Highlights Reference Note Sync

- status: wip
- parent: obsidian

***

#### [Page 2](highlights://highlights-ref-sync#page=2)

##### 2026-06-03:

> It only writes the PDF marker when frontmatter is the selected
source and --write-pdf is supplied.

- Support sase tool call replay?
```

The generated region is the only body region the tool owns:

```md
<!-- highlights:begin -->

<!-- highlights:end -->
```

Manual content outside those markers must be preserved. User edits inside the
generated region may be overwritten.

New generated notes include a title, a PDF wikilink Obsidian task line with
the stable `^task` block ID, and `## Highlights`.
Existing notes must already contain the managed begin/end markers; otherwise
`sync` fails instead of guessing where generated content belongs.

Generated blocks use Obsidian block IDs beginning with `^h-`. The MVP ID is a
deterministic content hash over source PDF path, page label, annotation kind,
sidecar order on the page, and quote/note text. Highlight comments are not part
of the hash, so editing only a comment updates the block without changing its
ID. If a previously generated block disappears from the sidecar, the command
keeps the old block ID under `### Removed highlights` with a tombstone message.

## MacBook Setup Guide

Run these steps on the MacBook. The intended checkout is
`~/projects/bob-cli`, and the intended vault paths are `~/bob/lib` for PDFs and
`~/bob/ref` for generated reference notes.

This Linux host currently has `~/bob/lit`, but the requested MVP defaults are
still `~/bob/lib` and `~/bob/ref`. Do not infer `lit` as the production default.
If a one-off test must use `lit`, pass `--lib-dir lit` explicitly.

Install prerequisites if needed:

```bash
xcode-select --install
command -v cargo >/dev/null || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

Clone or update `bob-cli`, then install the local checkout:

```bash
mkdir -p ~/projects
if [ -d ~/projects/bob-cli/.git ]; then
  git -C ~/projects/bob-cli pull --ff-only
else
  git clone git@github.com:bbugyi200/bob-cli.git ~/projects/bob-cli
fi
cargo install --path ~/projects/bob-cli --locked --force
bob highlights-ref --help
```

Create or confirm the vault layout:

```bash
mkdir -p ~/bob/lib/books ~/bob/ref
git -C ~/bob status --short
```

In Highlights Pro on the MacBook:

- Keep PDFs that should sync under `~/bob/lib/<ref_type>/`, such as
  `~/bob/lib/books`.
- Enable autosaved Markdown sidecars next to each PDF, so
  `~/bob/lib/books/example.pdf` gets `~/bob/lib/books/example.md`.
- Prefer Markdown sidecars over TextBundle for the MVP.
- Lock the Highlights Note Format to the sidecar contract above: page headings,
  `---` annotation separators, highlights as blockquote lines, highlight
  comments as plain paragraphs, and standalone notes as plain paragraphs.
- Add exactly one marker note as the first standalone `/Text` PDF note
  annotation on page 1.
- Put at least `status` and `parent` in that page-1 standalone note.

Use this marker as a starting point:

```text
- status: wip
- parent: obsidian
- title: Example Title
- topics: [example]
```

Run the initial checks:

```bash
bob highlights-ref doctor
bob highlights-ref scan --dry-run
bob highlights-ref sync ~/bob/lib/books/example.pdf --dry-run
bob highlights-ref marker ~/bob/lib/books/example.pdf
```

MacBook validation checklist:

- `cargo install --path ~/projects/bob-cli --locked --force` installs the local
  checkout.
- `bob highlights-ref doctor` reports valid vault/library/ref paths, marker
  readability, Git status, and optional `ob` availability.
- `bob highlights-ref scan --dry-run` lists the expected PDFs under
  `~/bob/lib`, reports the intended `~/bob/ref/<ref_type>/*.md` targets, and
  prints `writes: none`.
- `bob highlights-ref sync ~/bob/lib/books/example.pdf --dry-run` shows the
  expected marker page/note, sync source, sidecar path, note action, and no
  writes.
- The first real note write creates or updates `~/bob/ref/books/example.md`
  with `parent`, `type: "[[ref]]"`, `ref_type: books`, pipeline metadata,
  manual sections, and the managed Highlights region.
- A second run with unchanged inputs reports `writes: none`.
- If frontmatter-only edits require PDF marker write-back, a targeted dry run
  reports `pdf_marker_action: would-update` before any `--write-pdf` run.
- `git -C ~/bob status --short` is reviewed before and after each write pass.

The sync model is deliberately asymmetric:

- Marker/frontmatter fields are 2-way and can conflict.
- Highlights, highlight comments, and standalone non-marker notes are PDF or
  sidecar to reference note only.
- Edits inside the managed `<!-- highlights:begin -->` region in Obsidian may be
  overwritten.

Enable note writes only after reviewing the dry-run output:

```bash
git -C ~/bob status --short
bob highlights-ref sync ~/bob/lib/books/example.pdf
bob highlights-ref scan
```

`scan` does not enable PDF marker write-back. If a dry run reports
`pdf_marker_action: would-update`, handle that PDF with a targeted command after
backing up the PDF:

```bash
bob highlights-ref sync ~/bob/lib/books/example.pdf --dry-run
bob highlights-ref sync ~/bob/lib/books/example.pdf --write-pdf
```

The intended frontmatter edit workflow is:

```bash
$EDITOR ~/bob/ref/books/example.md
bob highlights-ref sync ~/bob/lib/books/example.pdf --dry-run
bob highlights-ref sync ~/bob/lib/books/example.pdf --write-pdf
```

If the ref note is tracked in Git, the write-back command may update that dirty
note only when the dirty changes are unstaged frontmatter-only edits and the
file still matches what the command planned from. Body edits, managed-region
edits, staged changes, untracked notes, and dirty PDFs are still refused.

Keep Highlights and Obsidian idle while testing PDF marker writes so the apps do
not race the CLI.

## Scheduled Scan

The MVP automation target is a scheduled `scan`, not a live recursive watcher.
Start with a dry-run schedule. On the MacBook account, create this LaunchAgent:

```bash
mkdir -p ~/Library/LaunchAgents ~/Library/Logs/bob
cat > ~/Library/LaunchAgents/com.bryan.bob-highlights-ref-scan.plist <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.bryan.bob-highlights-ref-scan</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/zsh</string>
    <string>-lc</string>
    <string>/Users/bryan/.cargo/bin/bob highlights-ref scan --dry-run</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>BOB_DIR</key>
    <string>/Users/bryan/bob</string>
    <key>BOB_HIGHLIGHTS_LIB_DIR</key>
    <string>lib</string>
    <key>BOB_HIGHLIGHTS_REF_DIR</key>
    <string>ref</string>
  </dict>
  <key>StartInterval</key>
  <integer>3600</integer>
  <key>StandardOutPath</key>
  <string>/Users/bryan/Library/Logs/bob/highlights-ref-scan.out</string>
  <key>StandardErrorPath</key>
  <string>/Users/bryan/Library/Logs/bob/highlights-ref-scan.err</string>
</dict>
</plist>
PLIST
plutil -lint ~/Library/LaunchAgents/com.bryan.bob-highlights-ref-scan.plist
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.bryan.bob-highlights-ref-scan.plist 2>/dev/null || true
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.bryan.bob-highlights-ref-scan.plist
launchctl kickstart -k gui/$(id -u)/com.bryan.bob-highlights-ref-scan
tail -n 80 ~/Library/Logs/bob/highlights-ref-scan.out
tail -n 80 ~/Library/Logs/bob/highlights-ref-scan.err
```

After several clean dry-run cycles, remove `--dry-run` from the
`ProgramArguments` command and reload the LaunchAgent with the same
`launchctl bootout`, `bootstrap`, and `kickstart` commands. Keep PDF marker
write-back manual; do not schedule `sync --write-pdf`.

A cron fallback is also acceptable:

```cron
SHELL=/bin/zsh
PATH=/Users/bryan/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/bin:/usr/bin
BOB_DIR=/Users/bryan/bob
BOB_HIGHLIGHTS_LIB_DIR=lib
BOB_HIGHLIGHTS_REF_DIR=ref
0 * * * * /Users/bryan/.cargo/bin/bob highlights-ref scan --dry-run >> /Users/bryan/Library/Logs/bob/highlights-ref-scan.log 2>&1
```

Disable the LaunchAgent with:

```bash
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.bryan.bob-highlights-ref-scan.plist
```

## Disable One PDF

The MVP has no marker key such as `sync: false`. `scan` processes every `.pdf`
under the configured library directory.

To disable one PDF, move that PDF and its sidecar out of `~/bob/lib`:

```bash
mkdir -p ~/bob/lib-disabled
mv ~/bob/lib/books/example.pdf ~/bob/lib-disabled/
mv ~/bob/lib/books/example.md ~/bob/lib-disabled/ 2>/dev/null || true
mv ~/bob/lib/books/example.textbundle ~/bob/lib-disabled/ 2>/dev/null || true
git -C ~/bob status --short
```

The existing `~/bob/ref/books/example.md` is no longer touched once the PDF is
outside the scanned library. Leave it in place for archival use, or move it
explicitly:

```bash
mkdir -p ~/bob/ref-disabled
mv ~/bob/ref/books/example.md ~/bob/ref-disabled/
```

Do not remove the managed Highlights markers as a disable mechanism. If a PDF is
still scanned and its existing reference note lacks the managed markers, the
command fails before guessing where generated content belongs.

## Backup and Rollback

Before first writes, make the vault clean or intentionally checkpointed:

```bash
git -C ~/bob status --short
git -C ~/bob stash push -u -m "pre-highlights-ref"
git -C ~/bob status --short
```

If the current vault changes should be kept as the baseline, commit them instead
of stashing:

```bash
git -C ~/bob add ref lib
git -C ~/bob commit -m "Checkpoint before highlights-ref sync"
```

Before enabling `--write-pdf`, keep a PDF backup outside the write path:

```bash
backup_dir=~/bob/backups/highlights-ref/$(date +%Y%m%d-%H%M%S)
mkdir -p "$backup_dir/lib/books"
cp -p ~/bob/lib/books/example.pdf "$backup_dir/lib/books/"
cp -p ~/bob/lib/books/example.md "$backup_dir/lib/books/" 2>/dev/null || true
```

For a full library backup before broader testing:

```bash
backup_dir=~/bob/backups/highlights-ref/$(date +%Y%m%d-%H%M%S)
mkdir -p "$backup_dir"
rsync -a --include='*/' --include='*.pdf' --include='*.md' --include='*.textbundle/***' --exclude='*' ~/bob/lib/ "$backup_dir/lib/"
```

Inspect note writes:

```bash
git -C ~/bob diff -- ref/books/example.md
git -C ~/bob status --short
```

Rollback a generated reference note:

```bash
git -C ~/bob restore -- ref/books/example.md
```

Rollback a PDF marker write from the backup:

```bash
cp -p "$backup_dir/lib/books/example.pdf" ~/bob/lib/books/example.pdf
```

## Conflict Resolution

When the marker and frontmatter both changed since the last synced projection,
the command compares both sides to `highlights_marker_base`. Non-overlapping
field edits auto-merge and dry runs report `sync_source: auto-merge`. Same-field
conflicts still fail and write nothing. Inspect both sides:

```bash
bob highlights-ref marker ~/bob/lib/books/example.pdf
sed -n '1,120p' ~/bob/ref/books/example.md
bob highlights-ref sync ~/bob/lib/books/example.pdf --dry-run
```

Choose the PDF marker as the source of truth:

```bash
bob highlights-ref sync ~/bob/lib/books/example.pdf --prefer marker
```

Choose the Obsidian frontmatter as the source of truth and write it back to the
PDF marker:

```bash
bob highlights-ref sync ~/bob/lib/books/example.pdf --prefer frontmatter --write-pdf
```

If the only change is frontmatter and the command says `--write-pdf` is missing,
or a dry-run auto-merge reports `pdf_marker_action: would-update`, review the
marker first, back up the PDF, then run:

```bash
bob highlights-ref sync ~/bob/lib/books/example.pdf --write-pdf
```

## Expected Failures

Common failure snippets and fixes:

| Message snippet | Meaning | Fix |
| --- | --- | --- |
| `library directory does not exist or is not a directory` | `~/bob/lib` is missing or `--lib-dir` points at the wrong path. | Create `~/bob/lib` or pass the intended `--lib-dir`. |
| `no standalone /Text note annotations found on page 1` | The PDF has no page-1 standalone marker note. | Add the first standalone PDF note on page 1 in Highlights. |
| `missing required marker key: status` | The marker list lacks `status`. | Add `- status: wip` to the marker. |
| `unsupported status` | `status` is not one of `unread`, `wip`, `done`, `abandoned`, or `legacy`. | Change the marker or frontmatter status to a supported value. |
| `missing required marker key: parent` | The marker/frontmatter projection lacks `parent`. | Add `- parent: obsidian` to the marker or frontmatter source; `[[obsidian]]` is also accepted. |
| `'type' is command-managed` | The marker tries to set the generated note `type`. | Remove `type` from the marker; generated notes get `type: "[[ref]]"` automatically. |
| `'ref_type' is command-managed` | The marker tries to set the path-derived reference type. | Remove `ref_type` from the marker; nested library paths derive it automatically. |
| `invalid marker item on line` | A marker line is not `- key: value` or `* key: value`. | Rewrite the marker as a flat list. |
| `duplicate marker key on line` | The marker repeats a normalized key. | Keep only one value for that key. |
| `output path collision(s) detected before writes` | Multiple PDFs would write the same reference note path, such as `ref/books/example.md`. | Rename or move one PDF before scanning. |
| `refusing to modify dirty vault files` | Git reports dirty touched paths outside the allowed frontmatter-only note write-back case. | Commit, stash, or clean those paths. |
| `frontmatter changed but --write-pdf was not supplied` | Frontmatter contributes to the selected projection, so the PDF marker needs an opt-in write. | Back up the PDF, then run targeted `sync --write-pdf`. |
| `marker/frontmatter conflict` | Marker and frontmatter changed the same field differently, or the note has no stored base snapshot for a safe merge. | Inspect both sides, then rerun with `--prefer marker` or `--prefer frontmatter --write-pdf`. |
| `changed during sync; rerun` | The note or PDF changed after planning and before writing. | Rerun after closing or pausing apps that may touch the file. |
| `existing reference note is missing the managed Highlights region` | An existing ref note lacks `<!-- highlights:begin -->` and `<!-- highlights:end -->`. | Add both markers around the generated section or move the note aside and regenerate. |
| `unsupported textbundle sidecar` | The `.textbundle` has no `text.md` or `text.markdown`. | Switch Highlights to Markdown sidecars or add one of those files. |

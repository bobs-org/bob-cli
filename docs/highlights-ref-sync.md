# Highlights Reference Note Sync

`bob highlights` turns Highlights app PDF annotations into Obsidian
reference notes in the Bob vault.

Code lives in this `bob-cli` repository. On the MacBook, use a checkout at
`~/projects/bob-cli`; do not install from ad hoc scripts outside that checkout.

## MVP Status

The MVP implements marker/frontmatter synchronization, Markdown/TextBundle
sidecar parsing, generated note rendering, TextBundle image selection asset
copying, recursive library scan, prerequisite checks, output collision
detection, dirty-target refusal, and atomic note writes.

`sync <pdf>` loads one PDF with native Rust code, treats the first standalone
`/Text` annotation on page 1 as the marker note, parses the marker list,
creates or updates `ref/<ref_type>/<pdf-basename>.md` for PDFs under
`lib/<ref_type>/`, and rewrites only the managed Highlights body region from the
sidecar when one is present. Top-level library PDFs and explicit out-of-library
syncs keep the legacy `ref/<pdf-basename>.md` target. `marker <pdf>` inspects
the same marker without writing.

`scan` recursively finds PDFs under the configured library directory and
processes them in stable path order. Per-PDF validation or write failures are
reported without stopping unrelated PDFs; the final command status is still
non-zero when any PDF fails. It refuses duplicate output paths such as two PDFs
that would both write the same `ref/<ref_type>/<basename>.md` target.

`doctor` checks vault paths, library/ref directories, sidecar presence, marker
readability, Git worktree status, and optional `ob` availability. It never
writes files.

Available commands:

```bash
bob highlights doctor [-b|--bob-dir PATH] [-l|--lib-dir PATH] [-r|--ref-dir PATH]
bob highlights marker <pdf> [-b|--bob-dir PATH] [-l|--lib-dir PATH] [-r|--ref-dir PATH]
bob highlights scan [-b|--bob-dir PATH] [-d|--dry-run] [-j|--jobs N] [-l|--lib-dir PATH] [-r|--ref-dir PATH] [-w|--write-pdfs]
bob highlights sync <pdf> [-b|--bob-dir PATH] [-d|--dry-run] [-l|--lib-dir PATH] [-p|--prefer marker|frontmatter] [-r|--ref-dir PATH] [-w|--write-pdf]
```

Path configuration options are `-b, --bob-dir <PATH>`, `-l, --lib-dir <PATH>`,
and `-r, --ref-dir <PATH>`. `scan` also accepts `-j, --jobs <N>`.

`sync <pdf> --dry-run` prints the resolved configuration and planned note/PDF
actions without modifying either side. Without `--dry-run`, the command writes
the reference note when frontmatter changes. It only writes the PDF marker when
the selected projection needs marker write-back and `--write-pdf` is supplied.
For recursive scans, `--write-pdfs` is the bulk opt-in for marker write-back;
`scan --dry-run --write-pdfs` remains read-only and previews the same marker
updates.

## Release Handoff Summary

The MVP is ready for Linux-side release checks and MacBook dry-run validation.
It includes the native `bob highlights` command, synthetic PDF marker
fixtures, frontmatter/marker conflict detection, generated highlight rendering,
recursive scan preflights, dirty target refusal, MacBook setup guidance, and
scheduled dry-run automation examples.

Known risks to validate on the MacBook:

- Real Highlights sidecar Markdown may vary from the fixture-backed parser
  contract. Keep the documented Highlights Note Format settings fixed while
  testing.
- PDF marker write-back is implemented with native PDF annotation writes, but
  real Highlights-authored files may expose annotation shapes not covered by
  Linux fixtures. Use `--write-pdf` or `--write-pdfs` only after backing up the
  target PDFs.
- Scheduled `scan` should stay dry-run or note-only unless bulk PDF marker
  writes are intentionally wanted and backed up.
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

`status` and `parent` are required marker keys. Marker `parent` must be a bare
note target such as `obsidian` or `Systems Performance`; Obsidian wikilinks such
as `[[obsidian]]`, aliases, embeds, and block links are rejected in the PDF
marker. Generated reference notes still render `parent` frontmatter as an
Obsidian wikilink, for example `parent: "[[obsidian]]"`.

`status` must be a scalar string and must exactly match one of:

- `unread`
- `wip`
- `read`
- `abandoned`
- `legacy`

Existing `status: done` marker/frontmatter/base inputs are accepted only as a
deprecated alias and are normalized to `read` during sync.

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
- If the generated PDF task line uses `[x]` or `[X]`, treat that as a
  note-side `status: read` signal.
- If marker and frontmatter changed different fields from the stored base,
  auto-merge them. PDF marker writes are still opt-in with targeted
  `sync --write-pdf` or bulk `scan --write-pdfs`.
- If both changed the same field differently, fail without modifying either side unless
  `--prefer marker` or `--prefer frontmatter` is supplied.

The last synced user-property projection is stored as `highlights_marker_hash`
and as compact JSON in `highlights_marker_base`. The hash keeps old-note
compatibility; the base snapshot lets the command prove safe field-level
merges. `--prefer frontmatter`, checked-task completion, and any auto-merge
that includes frontmatter changes require `--write-pdf` for targeted `sync`, or
`--write-pdfs` for recursive `scan`, whenever the PDF marker must be updated.

PDF marker writes are performed by saving a temporary PDF next to the original
and renaming it over the target. Before first PDF writes, commit or otherwise
back up the PDF library. Keep Highlights and Obsidian idle while testing PDF
writes so the apps do not race the CLI.

## Scan, Safety, and Git/ob Behavior

`scan --dry-run` reports every discovered PDF. Valid PDFs show their target
reference note, sidecar path if present, selected sync source, and note/PDF
marker action. Invalid PDFs show a `plan_error`. Scan output also reports
`write_pdfs: true|false` so bulk marker-write runs are auditable. Dry runs do
not create directories, write notes, or write PDFs, even when combined with
`--write-pdfs`.

Before a writing scan, the command rejects duplicate output paths, builds
per-PDF plans, and checks Git status for existing vault files that successfully
planned PDFs would modify. If a target ref note or PDF marker target is dirty,
it fails before any write, except for a tracked target ref note whose only body
change is the exact generated `^ref` checkbox toggle, optionally combined with
frontmatter edits. There is no force mode in the MVP; commit, stash, or clean
unrelated dirty files before rerunning.

Image assets copied from TextBundle sidecars are treated as note-side writes.
They are planned during dry runs, checked by the same dirty-target preflight
when the destination already exists, and written only during non-dry-run syncs.
Scan output includes image totals when a planned PDF contains image selections.

Planning failures for one PDF do not stop valid plans from writing. Write-time
failures such as a changed note/PDF or temporary save failure are reported as
`write_failure` entries, and later valid PDFs continue. The scan summary reports
`plan_failures`, `write_failures`, and `scan_failures`; any non-zero failure
count makes the command exit non-zero even when other PDFs were processed.

Reference note writes and image asset copies are atomic temporary-file renames
and are skipped when the rendered note or content-addressed asset is
byte-identical to the existing file.

`bob highlights` does not run `ob sync` before or after writes. The existing
`bob nightly` sync gate owns `ob sync` orchestration, while this command only
reports whether `ob` is available through `doctor`.

## Generated Body Contract

Highlights, image selections, comments on annotations, and standalone
non-marker notes sync one way from the PDF/sidecar into the reference note.

For `foo.pdf`, sidecar discovery looks for `foo.md` first. If there is no
Markdown sidecar, it recognizes `foo.textbundle` only when the bundle contains
`text.md` or `text.markdown`; other TextBundle contents fail with an explicit
unsupported-sidecar error. Image selections require the referenced asset file to
exist relative to the sidecar text file's directory. This makes
`foo.textbundle/text.md` resolve `![](assets/figure.png)` to
`foo.textbundle/assets/figure.png`. Plain Markdown sidecars can still reference
local image files, but Highlights' plain Markdown export does not include image
assets; re-export as TextBundle when area selections should sync. If no sidecar
exists, `sync` still performs marker/frontmatter sync. For a new note it creates
an empty managed region; for an existing note it preserves the existing managed
region.

Two Markdown sidecar shapes are supported. The simple shape is:

- Page labels are Markdown headings such as `## Page 12` or `## p. 12`.
- Annotation spacers are horizontal rules such as `---`.
- A highlight starts with one or more blockquote lines.
- Non-heading text after a highlight is rendered as a highlight comment. A
  leading `Comment:` or `Note:` label is stripped.
- A standalone note is non-heading text outside a blockquote. A leading `Note:`
  label is stripped.
- A Markdown image line such as `![alt](assets/figure.png)` outside a blockquote
  is an image selection when the target has a supported image extension. Text
  after the image is treated as its comment. Multiple images in one chunk render
  as separate image annotations. Non-image targets such as PDFs remain ordinary
  standalone note text.
- A note attached to an image annotation may be stored either as regular text
  below the image or inside the Markdown image title, such as
  `![alt](assets/figure.png "note text")`. Both shapes render as the image's
  nested `[!note] Comment` callout. When both are present, the explicit text
  below the image is used first and the title text is appended unless it merely
  repeats that text. The image alt text is treated as metadata, not a note, so
  generic captions are not rendered as comments, and the asset path is never
  rendered as note text.
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

Image selection sidecar fragment:

```md
## Page 12

![Latency figure](assets/figure.png)

Comment: Compare this figure with the latency table on p.14.
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

> It only writes the PDF marker when marker write-back is needed
and the matching write opt-in is supplied.

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
`[p::2]` priority and the stable `^ref` block ID, and `## Highlights`.
Existing notes must already contain the managed begin/end markers; otherwise
`sync` fails instead of guessing where generated content belongs.

Generated annotations render as Obsidian callouts. Highlight text uses a
`[!quote]` callout; a highlight or image comment is nested inside that quote as
a `[!note] Comment` callout so the annotation's trailing `^h-...` block ID still
covers the quote/image and the comment together. Image selections use
`[!quote] Image` with a vault-relative Obsidian embed. Standalone non-marker
notes render as `[!note]` callouts. Removed annotation tombstones render under
`### Removed highlights` as `[!warning] Removed highlight` callouts.

Example generated body:

```md
### Page 12

> [!quote] Latency is not throughput.
>
> > [!note] Comment Compare this with SLO notes.

^h-2b91f0a4c7de

> [!note] Keep this standalone observation.

^h-8f42a61a90cc
```

Image selections copy their asset into a per-note assets directory beside the
reference note. For `ref/books/example.md`, the destination is
`ref/books/example.assets/h-<id>.<ext>`, and the generated block embeds it with a
vault-relative wikilink:

```md
### Page 12

> [!quote] Image ![[ref/books/example.assets/h-2b91f0a4c7de.png]]
>
> > [!note] Comment Compare this figure with the latency table on p.14.

^h-2b91f0a4c7de
```

Asset filenames and image block IDs are content-addressed from the source PDF
path and image bytes, so re-exported or renamed TextBundle assets keep the same
`^h-...` block and stored filename when the image bytes are unchanged. Existing
matching assets are skipped; an existing destination with different bytes is a
write failure instead of an overwrite. When an image selection is removed from
the sidecar, the generated block is tombstoned like other removed annotations,
but the copied asset file is left in place. Asset garbage collection is future
work.

Annotation text is beautified only while rendering the generated region:

- Consecutive prose lines are reflowed into one line. Blank lines remain
  paragraph breaks.
- Markdown unordered list lines (`- `, `* `, `+ `, including task checkboxes)
  start their own logical lines, and wrapped continuation text joins onto that
  list item.
- Hyphenated PDF line endings are healed at join points: lowercase next words
  drop the hyphen (`through-` + `put` -> `throughput`), while uppercase or digit
  next fragments keep it (`Marie-` + `Curie` -> `Marie-Curie`).
- Ligature glyphs, non-breaking spaces, soft hyphens, zero-width characters,
  doubled spaces, and tab runs are normalized away.

The cleanup is render-only. Block IDs, annotation-task source links, and
processed `[h:: ...]` markers continue to use the raw normalized sidecar text,
so unchanged sidecar annotations keep their existing `^h-...` IDs when a note is
regenerated with the callout format.

The generated task line is a completion affordance:

```md
- [ ] #task [[lib/books/example.pdf]] [p::2] ^ref
```

Checking it with `[x]` or `[X]` means `status: read`. Cancelling it with `[-]`
means `status: abandoned` and may keep metadata such as
`[cancelled:: 2026-06-04]`. Unchecking it does not infer a replacement status.
When the final synced status is `read`, `sync` checks the generated task line;
when it is `abandoned`, `sync` cancels the generated task line. Existing notes
without that exact generated line are not bulk-migrated. If the checked or
cancelled task would update the PDF marker, `sync --dry-run` previews
`pdf_marker_action: would-update`, plain `sync` refuses before writes, and
targeted `sync --write-pdf` writes the marker. `scan --dry-run` previews this
work. A writing scan keeps the default note-only refusal unless `--write-pdfs`
is supplied.

Highlight comments and standalone non-marker notes can also create actionable
Obsidian tasks when the marker/frontmatter-selected PDF status is `wip`, before
the generated PDF `^ref` checkbox contributes a closing `read` or `abandoned`
status. The final run that closes a `wip` PDF still imports newly added
annotation tasks in that same run; subsequent runs whose selected status is
already non-`wip` skip task intake. Any unordered Markdown bullet line whose
item text contains `#task` as a whitespace-delimited token is copied to an
unchecked task:

```md
- #task Compare this claim with the appendix.
- [x] #task Email the citation to Alex.
- #task Send the quote to Alice @alice
```

By default, created tasks are top-level siblings immediately under the
generated PDF `^ref` line:

```md
- [ ] #task Compare this claim with the appendix. [[#^h-2b91f0a4c7de|🔖]] [h:: 4c0a13d2...] [created::2026-06-07]
- [ ] #task Email the citation to Alex. [[#^h-2b91f0a4c7de|🔖]] [h:: 910f6ce7...] [created::2026-06-07]
```

If the final whitespace-delimited token is a strict `@name` route suffix, the
suffix is removed from the created task text and the task is appended to
existing root-level note `~/bob/name.md` instead. Route names must match
`@([A-Za-z0-9][A-Za-z0-9_-]*)`; tokens with punctuation, dots, slashes, empty
names, or path traversal content are ordinary task text. Routed target notes
must already exist and must not be directories, because new root notes need
explicit frontmatter that this command cannot infer. For example, `@alice`
creates this line in `~/bob/alice.md`:

```md
- [ ] #task Send the quote to Alice [[ref/books/example#^h-2b91f0a4c7de|🔖]] [h:: 9b31c4a0...] [created::2026-06-07]
```

Each created task carries a block backlink to the annotation-level generated
source block, rendered as an aliased `🔖` link between the task prose and
processed/created properties. Same-note tasks use compact same-file targets
(`[[#^h-...]]`). Routed tasks use full vault-relative note targets
(`[[ref/books/example#^h-...]]`) so the source note remains recoverable after
the task moves to another note or `done/`. The managed Highlights region keeps
the annotation-level `^h-...` block and does not attach task-specific `^ht-...`
block IDs to source task lines:

```md
> [!quote] Highlighted claim.
>
> > [!note] Comment #task Compare this claim with the appendix.
> > #task Email the citation to Alex.

^h-2b91f0a4c7de
```

The `[h:: ...]` processed ID is computed from the source reference note path,
source annotation block, and normalized task identity. Duplicate identical
`#task` bullets in the same annotation still create only one task. The property
is the durable processed marker that moves with the task when it is completed,
cancelled, edited, or archived by `bob move-done-tasks`, so later syncs do not
recreate it and do not write any processed state back into the PDF or sidecar.
Older tasks that already have `[highlight_task:: ...]` are still recognized for
compatibility, and tasks created by the old `^ht` implementation are recognized
by their `#^ht-...` backlinks. New tasks do not write `[highlight_task:: ...]`
or `^ht` source anchors. If both processed properties and old source backlinks
are removed from a moved or heavily edited task, the legacy normalized text
fallback only covers unchanged text.

Annotation-created tasks are independent tasks, not subtasks of the PDF
reading-status task. Later syncs preserve existing checkbox state and task
properties such as `[completion::]`, `[cancelled::]`, `[due::]`, or edited
priority fields. Completing or cancelling these annotation-created tasks does
not update the PDF marker or reference-note reading status.

Generated blocks use Obsidian block IDs beginning with `^h-`. Text highlight
and standalone-note IDs are deterministic content hashes over source PDF path,
page label, annotation kind, sidecar order on the page, and quote/note text.
Highlight comments are not part of the hash, so editing only a comment updates
the block without changing its ID. Image IDs use the source PDF path plus image
bytes instead, so asset renames and nearby annotation order changes do not churn
the image block. If a previously generated block disappears from the sidecar,
the command keeps the old block ID under `### Removed highlights` with a
tombstone message. Editing text highlight content itself mints a new block ID; a
task created earlier keeps its original link, which then targets the tombstoned
block under `### Removed highlights` — still a valid, resolvable jump.

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
bob highlights --help
```

Create or confirm the vault layout:

```bash
mkdir -p ~/bob/lib/books ~/bob/ref
git -C ~/bob status --short
```

In Highlights Pro on the MacBook:

- Keep PDFs that should sync under `~/bob/lib/<ref_type>/`, such as
  `~/bob/lib/books`.
- Enable autosaved sidecars next to each PDF. Plain Markdown creates
  `~/bob/lib/books/example.md`; TextBundle creates
  `~/bob/lib/books/example.textbundle/text.md`.
- Use TextBundle export for PDFs where Highlights area/rectangle selections
  should sync, because plain Markdown export does not include image assets.
- Lock the Highlights Note Format to the sidecar contract above: page headings,
  `---` annotation separators, highlights as blockquote lines, highlight
  comments as plain paragraphs, standalone notes as plain paragraphs, and image
  selections as Markdown image links.
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
bob highlights doctor
bob highlights scan --dry-run
bob highlights sync ~/bob/lib/books/example.pdf --dry-run
bob highlights marker ~/bob/lib/books/example.pdf
```

MacBook validation checklist:

- `cargo install --path ~/projects/bob-cli --locked --force` installs the local
  checkout.
- `bob highlights doctor` reports valid vault/library/ref paths, marker
  readability, Git status, and optional `ob` availability.
- `bob highlights scan --dry-run` lists the expected PDFs under
  `~/bob/lib`, reports the intended `~/bob/ref/<ref_type>/*.md` targets, and
  prints `writes: none`. If `scan_failures` is non-zero, inspect the per-PDF
  `plan_error` lines while noting that valid PDFs were still reported.
- `bob highlights sync ~/bob/lib/books/example.pdf --dry-run` shows the
  expected marker page/note, sync source, sidecar path, note action, and no
  writes.
- The first real note write creates or updates `~/bob/ref/books/example.md`
  with `parent`, `type: "[[ref]]"`, `ref_type: books`, pipeline metadata,
  manual sections, and the managed Highlights region.
- For a TextBundle image selection, the first real write also creates
  `~/bob/ref/books/example.assets/h-<id>.<ext>` and embeds it from the generated
  callout with a vault-relative wikilink.
- A second run with unchanged inputs reports `writes: none`.
- If frontmatter edits or a checked generated task require PDF marker
  write-back, a dry run reports `pdf_marker_action: would-update` before any
  targeted `--write-pdf` or bulk `--write-pdfs` run.
- `git -C ~/bob status --short` is reviewed before and after each write pass.

The sync model is deliberately asymmetric:

- Marker/frontmatter fields are 2-way and can conflict.
- Highlights, highlight comments, and standalone non-marker notes are PDF or
  sidecar to reference note only.
- Annotation-created `#task` bullets are created in the reference note or an
  explicitly routed existing root note only, and do not sync completion state
  back to the PDF marker.
- Edits inside the managed `<!-- highlights:begin -->` region in Obsidian may be
  overwritten.

Enable note writes only after reviewing the dry-run output:

```bash
git -C ~/bob status --short
bob highlights sync ~/bob/lib/books/example.pdf
bob highlights scan
```

`scan` does not enable PDF marker write-back by default. If a dry run reports
`pdf_markers_would_update`, review those PDFs and back up the library before
bulk write-back:

```bash
bob highlights scan --dry-run --write-pdfs
bob highlights scan --write-pdfs
```

For a single PDF, keep using the targeted singular flag:

```bash
bob highlights sync ~/bob/lib/books/example.pdf --dry-run
bob highlights sync ~/bob/lib/books/example.pdf --write-pdf
```

The intended frontmatter edit workflow is:

```bash
$EDITOR ~/bob/ref/books/example.md
bob highlights sync ~/bob/lib/books/example.pdf --dry-run
bob highlights sync ~/bob/lib/books/example.pdf --write-pdf
```

If the ref note is tracked in Git, the write-back command may update that dirty
note only when the dirty changes are unstaged frontmatter edits and/or the exact
generated `^ref` checkbox toggle, and the file still matches what the command
planned from. Other body edits, managed-region edits, staged changes, untracked
notes, and dirty PDFs are still refused.

Keep Highlights and Obsidian idle while testing PDF marker writes so the apps do
not race the CLI.

## Scheduled Scan

The MVP automation target is a scheduled `scan`, not a live recursive watcher.
Start with a dry-run schedule. On the MacBook account, create this LaunchAgent:

```bash
mkdir -p ~/Library/LaunchAgents ~/Library/Logs/bob
cat > ~/Library/LaunchAgents/com.bryan.bob-highlights-scan.plist <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.bryan.bob-highlights-scan</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/zsh</string>
    <string>-lc</string>
    <string>/Users/bryan/.cargo/bin/bob highlights scan --dry-run</string>
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
  <string>/Users/bryan/Library/Logs/bob/highlights-scan.out</string>
  <key>StandardErrorPath</key>
  <string>/Users/bryan/Library/Logs/bob/highlights-scan.err</string>
</dict>
</plist>
PLIST
plutil -lint ~/Library/LaunchAgents/com.bryan.bob-highlights-scan.plist
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.bryan.bob-highlights-scan.plist 2>/dev/null || true
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.bryan.bob-highlights-scan.plist
launchctl kickstart -k gui/$(id -u)/com.bryan.bob-highlights-scan
tail -n 80 ~/Library/Logs/bob/highlights-scan.out
tail -n 80 ~/Library/Logs/bob/highlights-scan.err
```

After several clean dry-run cycles, remove `--dry-run` from the
`ProgramArguments` command and reload the LaunchAgent with the same
`launchctl bootout`, `bootstrap`, and `kickstart` commands. Keep PDF marker
write-back manual; do not schedule `sync --write-pdf` or `scan --write-pdfs`
unless bulk PDF writes are deliberately desired and backed up.

A cron fallback is also acceptable:

```cron
SHELL=/bin/zsh
PATH=/Users/bryan/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/bin:/usr/bin
BOB_DIR=/Users/bryan/bob
BOB_HIGHLIGHTS_LIB_DIR=lib
BOB_HIGHLIGHTS_REF_DIR=ref
0 * * * * /Users/bryan/.cargo/bin/bob highlights scan --dry-run >> /Users/bryan/Library/Logs/bob/highlights-scan.log 2>&1
```

Disable the LaunchAgent with:

```bash
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.bryan.bob-highlights-scan.plist
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
git -C ~/bob stash push -u -m "pre-highlights"
git -C ~/bob status --short
```

If the current vault changes should be kept as the baseline, commit them instead
of stashing:

```bash
git -C ~/bob add ref lib
git -C ~/bob commit -m "Checkpoint before highlights sync"
```

Before enabling `--write-pdf` or `--write-pdfs`, keep a PDF backup outside the
write path:

```bash
backup_dir=~/bob/backups/highlights/$(date +%Y%m%d-%H%M%S)
mkdir -p "$backup_dir/lib/books"
cp -p ~/bob/lib/books/example.pdf "$backup_dir/lib/books/"
cp -p ~/bob/lib/books/example.md "$backup_dir/lib/books/" 2>/dev/null || true
```

For a full library backup before broader testing:

```bash
backup_dir=~/bob/backups/highlights/$(date +%Y%m%d-%H%M%S)
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
bob highlights marker ~/bob/lib/books/example.pdf
sed -n '1,120p' ~/bob/ref/books/example.md
bob highlights sync ~/bob/lib/books/example.pdf --dry-run
```

Choose the PDF marker as the source of truth:

```bash
bob highlights sync ~/bob/lib/books/example.pdf --prefer marker
```

Choose the Obsidian frontmatter as the source of truth and write it back to the
PDF marker:

```bash
bob highlights sync ~/bob/lib/books/example.pdf --prefer frontmatter --write-pdf
```

If the only change is frontmatter, the generated task line uses `[x]`, `[X]`,
or `[-]`, or a dry-run auto-merge reports `pdf_marker_action: would-update`,
review the marker first, back up the PDF, then run the targeted write:

```bash
bob highlights sync ~/bob/lib/books/example.pdf --write-pdf
```

For reviewed bulk scan write-back, preview the library and then opt in:

```bash
bob highlights scan --dry-run --write-pdfs
bob highlights scan --write-pdfs
```

## Expected Failures

Common failure snippets and fixes:

During `scan`, marker validation, sidecar validation, managed-region
validation, `--write-pdf` refusals, and per-PDF write races are reported for the
affected PDF while unrelated valid PDFs may still be planned or written. The
summary shows `result: partial-failure` and the command exits non-zero when any
PDF fails. Library discovery errors, output path collisions, and dirty target
preflight failures remain hard global failures before writes.

| Message snippet | Meaning | Fix |
| --- | --- | --- |
| `library directory does not exist or is not a directory` | `~/bob/lib` is missing or `--lib-dir` points at the wrong path. | Create `~/bob/lib` or pass the intended `--lib-dir`. |
| `no standalone /Text note annotations found on page 1` | The PDF has no page-1 standalone marker note. | Add the first standalone PDF note on page 1 in Highlights. |
| `missing required marker key: status` | The marker list lacks `status`. | Add `- status: wip` to the marker. |
| `unsupported status` | `status` is not one of `unread`, `wip`, `read`, `abandoned`, or `legacy`. | Change the marker or frontmatter status to a supported value. |
| `missing required marker key: parent` | The marker/frontmatter projection lacks `parent`. | Add a bare marker parent such as `- parent: obsidian`, or add frontmatter parent such as `parent: "[[obsidian]]"`. |
| `wikilinks are not supported` | The PDF marker `parent` uses Obsidian link syntax. | Remove the brackets in the PDF marker, e.g. use `- parent: obsidian`. |
| `'type' is command-managed` | The marker tries to set the generated note `type`. | Remove `type` from the marker; generated notes get `type: "[[ref]]"` automatically. |
| `'ref_type' is command-managed` | The marker tries to set the path-derived reference type. | Remove `ref_type` from the marker; nested library paths derive it automatically. |
| `invalid marker item on line` | A marker line is not `- key: value` or `* key: value`. | Rewrite the marker as a flat list. |
| `duplicate marker key on line` | The marker repeats a normalized key. | Keep only one value for that key. |
| `output path collision(s) detected before writes` | Multiple PDFs would write the same reference note path, such as `ref/books/example.md`, or the same planned image asset destination. | Rename or move one PDF before scanning. |
| `image asset not found` | The sidecar references an image file that is not present relative to the sidecar text file. This usually means the PDF was exported as plain Markdown instead of TextBundle. | Re-export the sidecar as TextBundle so `assets/...` files are included. |
| `image asset destination exists with different bytes` | A content-addressed `ref/.../*.assets/h-<id>.<ext>` destination already exists but its bytes do not match the source asset. | Inspect the existing asset, then remove or restore it before rerunning. |
| `refusing to modify dirty vault files` | Git reports dirty touched paths outside the allowed frontmatter and generated-task checkbox write-back case. | Commit, stash, or clean those paths. |
| `reference note changed but --write-pdf was not supplied` | Frontmatter or the generated task contributes `status: read` or `status: abandoned` to the selected projection, so the PDF marker needs an opt-in write. | Back up the PDF, then run targeted `sync --write-pdf` or reviewed bulk `scan --write-pdfs`. |
| `checked PDF task conflicts` | The generated task says `status: read`, but marker or frontmatter changed `status` to another value from the stored base. | Uncheck the task or set the marker/frontmatter status to `read`. |
| `cancelled PDF task conflicts` | The generated task says `status: abandoned`, but marker or frontmatter changed `status` to another value from the stored base. | Uncancel the task or set the marker/frontmatter status to `abandoned`. |
| `marker/frontmatter conflict` | Marker and frontmatter changed the same field differently, or the note has no stored base snapshot for a safe merge. | Inspect both sides, then rerun with `--prefer marker` or `--prefer frontmatter --write-pdf`. |
| `changed during sync; rerun` | The note or PDF changed after planning and before writing. | Rerun after closing or pausing apps that may touch the file. |
| `scan completed with ... per-PDF failure(s)` | A recursive scan finished reporting or writing valid PDFs, but at least one PDF had a `plan_error` or `write_failure`. | Fix the named PDFs and rerun; review successful writes before assuming the scan wrote nothing. |
| `existing reference note is missing the managed Highlights region` | An existing ref note lacks `<!-- highlights:begin -->` and `<!-- highlights:end -->`. | Add both markers around the generated section or move the note aside and regenerate. |
| `unsupported textbundle sidecar` | The `.textbundle` has no `text.md` or `text.markdown`. | Re-export a valid TextBundle or add one of those files. |

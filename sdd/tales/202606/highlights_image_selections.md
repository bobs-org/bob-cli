---
create_time: 2026-06-15 09:04:32
status: done
prompt: sdd/prompts/202606/highlights_image_selections.md
---
# Highlights Image Selection Support Plan

## Context

`bob highlights scan` (and `sync`) turns Highlights-app PDF annotations into Obsidian reference notes. Today the sidecar
parser in `src/native/highlights_ref/mod.rs` understands exactly two annotation shapes:

- **Highlights** — blockquote lines (`> ...`), optionally followed by a comment.
- **Standalone notes** — non-blockquote paragraphs (first one is the marker mirror and is skipped).

Each annotation renders as a beautiful Obsidian callout (`[!quote]` for highlights with nested `[!note] Comment`,
`[!note]` for standalone notes), carries a stable `^h-<id>` block ID, can spawn `#task` items, and is tombstoned under
`### Removed highlights` when it disappears from the sidecar.

**The gap:** the Highlights app also supports _area / rectangle selections_ — visual snapshots of a PDF region (figures,
tables, equations, diagrams). These are first-class annotations in Highlights, but `bob highlights` ignores them. Worse
than ignoring: a Markdown image line like `![](assets/figure.png)` falls through to the standalone-note branch today, so
it is silently rendered as a `[!note]` callout containing broken literal text with an unresolvable relative path, and
the image file is never brought into the vault.

### Authoritative constraint discovered during research

Highlights' **plain Markdown export deliberately strips images** — "Markdown is a plain text format and will therefore
exclude images." Images are only preserved by the **TextBundle** export, which stores them in an `assets/` subfolder and
references them with relative `![](assets/<name>.<ext>)` links (per the TextBundle spec). The parser already discovers
`.textbundle` sidecars (`text.md` / `text.markdown`) but reads only the text file and never looks at `assets/`.

**Design consequence:** reliable image support is fundamentally a TextBundle feature. The plan resolves image references
relative to the sidecar's own directory (which is the bundle root), so TextBundle "just works," while a plain `.md`
sidecar that references a missing image fails with guidance to re-export as TextBundle.

## Goal

Make image / area selections from the Highlights app a fully supported, first-class annotation in `bob highlights scan`
and `bob highlights sync` — **intuitive** (zero new flags; works the moment a TextBundle is present), **reliable**
(content-addressed identity, atomic idempotent copies, dry-run safe, integrated with every existing safety preflight),
and **beautiful** (image embeds rendered inside the same callout family as text highlights, with the same
comment/task/tombstone affordances).

Everything that already works for text highlights — block-ID stability, removed-annotation tombstones, comment
rendering, `#task` intake and `@route` suffixes, marker/frontmatter sync, dirty-target refusal, per-PDF error isolation
— must keep working unchanged.

## Design Decisions (the "lead" calls)

These are the choices I'm proposing. Rationale and rejected alternatives are included so they can be challenged at
review.

### 1. Source of truth: TextBundle `assets/`, resolved relative to the sidecar

Image references are resolved as `sidecar_dir.join(<relative-target>)`. For a TextBundle that is
`foo.textbundle/assets/x.png`; for a plain `.md` sidecar it is `foo's dir/<target>`. If the resolved file exists, it is
synced; if not, the PDF fails with a clear per-PDF error pointing at TextBundle export. This keeps the rule general and
format-agnostic instead of hard-coding bundle internals.

### 2. Content-addressed identity and storage (the reliability cornerstone)

Highlights re-exports often rewrite asset filenames (random/opaque names) even when the captured pixels are unchanged.
So image identity must **not** depend on the asset filename, the page ordinal, or surrounding text — only on the image
bytes.

- **Block ID:** `^h-<id>` where `<id>` is the first 12 hex of `sha256(source_pdf_value · "image" · image_sha256)`. This
  intentionally diverges from the text-annotation scheme (which folds in page label + ordinal): images are addressed
  purely by content, so adding annotations above an image, re-cropping noise, or a renamed asset does **not** churn the
  block ID, the note body, or the stored file. Two identical captures dedupe to one block (a feature).
- **Stored filename:** the asset is copied into the vault as `h-<id>.<ext>`, so the file on disk is self-describing and
  ties 1:1 to its block. Idempotency becomes trivial: if `h-<id>.<ext>` already exists, the content is by definition
  identical and the copy is skipped.

### 3. Where images live: a per-note `*.assets/` folder beside the reference note

For `ref/books/example.md`, images go to `ref/books/example.assets/h-<id>.<ext>`.

- Co-located → deleting/moving the note's assets is obvious; no cross-PDF collisions; locality keeps `git diff`
  readable.
- The folder name (`example.assets`) does **not** start with a dot, so Obsidian indexes it (dot-folders are hidden by
  Obsidian and would break embeds — an easy trap this avoids).
- Rejected: a single central `attachments/` dir (harder cleanup, name collisions, noisier diffs) and Obsidian's
  configured attachment folder (none is configured in this vault; default is vault root, which would scatter files).

### 4. Rendering: same callout family as highlights, image embedded inline

Image selections are visual quotes from the document, so they stay in the `[!quote]` family for visual unity,
distinguished by an explicit `Image` title and the embed:

```md
### Page 12

> [!quote] Image ![[ref/books/example.assets/h-2b91f0a4c7de.png]]
>
> > [!note] Comment Compare this figure with the latency table on p.14.

^h-2b91f0a4c7de
```

- Embeds use **vault-relative Obsidian wikilinks** (`![[ref/.../h-<id>.png]]`), matching the codebase's existing
  wikilink conventions and resolving unambiguously even when the same image appears in multiple notes.
- Comments on an image render with the exact same nested `[!note] Comment` callout used for text highlights — no
  special-casing downstream.
- Rejected: a bare top-level `![[...]]` embed (loses the visual grouping and the block-ID anchor placement) and a
  non-standard `[!image]` type (Obsidian has no such built-in; it would fall back to default styling with no benefit).

### 5. No new CLI flags (intuitive by default)

Producing a correct reference note _requires_ the image file to be present in the vault, so asset copies are part of
note generation, gated by the existing `--dry-run`. They are low-risk writes — new, content-addressed files that never
overwrite user content and are trivially `git restore`-able — unlike PDF marker writes, which mutate source PDFs and
stay behind `--write-pdf(s)`. Adding a flag would make the common path require ceremony for no safety gain. (CLI rules
reviewed: no new options means no new short-alias/sorting obligations; the rule to keep output beautiful and colored is
honored in §"Scan output".)

## Implementation Plan

All work is in `src/native/highlights_ref/mod.rs` plus docs/fixtures/tests.

### A. Parse image annotations

1. Add `SidecarAnnotationKind::Image`, and extend `SidecarAnnotation` with an optional image descriptor (the relative
   image target string + optional alt text). `as_str()` returns `"image"`.
2. In `parse_sidecar_chunk`, after the existing blockquote branch and before the plain standalone-note branch, detect a
   Markdown **image** (`![alt](target)`, distinguished from a normal link by the leading `!`) in the non-heading chunk
   lines:
   - The first image reference in the chunk becomes an `Image` annotation; any remaining non-image, non-heading text
     becomes its `comment` / `task_source` (reusing the exact comment pipeline highlights already use, so `#task`
     extraction and `@route` suffixes work for free).
   - Recognize common raster/vector image extensions (`png`, `jpg`, `jpeg`, `gif`, `webp`, `bmp`, `svg`, `avif`,
     `heic`). A `![..](..)` whose target is not image-like is left to fall through as ordinary note text (defensive; not
     expected from Highlights).
   - Edge cases documented and handled deterministically: a chunk with multiple images renders each as its own
     content-addressed `Image` block (lossless); a blockquote chunk that also contains an image keeps today's
     highlight-with-comment behavior (images only trigger in non-blockquote chunks). The first-standalone-note
     marker-mirror skip is unaffected — an image is never the marker mirror.

### B. Resolve, hash, and rewrite-time identity

3. Add an image-resolution pre-pass used by `render_sidecar_highlights` (already `Result`-returning): for each `Image`
   annotation, resolve `sidecar_dir.join(target)`, read the bytes, compute `image_sha256`, and derive the
   content-addressed block ID and the vault destination `…/<note-stem>.assets/h-<id>.<ext>`. Missing/unreadable files
   produce a clear `CommandError` → per-PDF `plan_error` (e.g.
   `image asset not found: assets/figure.png — export the sidecar as a TextBundle so images are included`).
4. Branch `annotation_block_id` on kind: text annotations keep the existing
   `(pdf · kind · page · ordinal · normalized text)` identity untouched (no block-ID churn for existing notes); `Image`
   uses `(pdf · "image" · image_sha256)`.

### C. Render image blocks

5. Extend `render_annotation_block` with an `Image` arm: emit a `> [!quote] Image` callout whose body is the
   vault-relative `![[…]]` embed, nest the comment as `[!note] Comment` exactly as highlights do, then the `^h-<id>`
   anchor. `RenderedHighlights` gains an `image_count` so the umbrella annotation `count` is unchanged (images are
   annotations) while we can surface an image breakdown.
6. Tombstones: removed images flow through the existing `### Removed highlights` path automatically (the ID is simply
   absent from `current_ids`). The orphaned asset file is intentionally **left in place** (safe, no data loss; the
   tombstone is a text warning, not the image). Asset garbage-collection is called out as explicit future work, not MVP.

### D. Plan and execute asset copies (reliable writes)

7. Add an `image_assets: Vec<ImageAssetWrite>` field to `PdfSyncPlan`, each entry `{ source_path, dest_path, action }`
   where `action` is `copy` or `none` (skip when `dest` already exists — content-addressing guarantees identical bytes).
8. Execution (`execute_pdf_sync`): create the `*.assets/` dir as needed and copy pending assets via an **atomic
   temp-file + rename** (mirroring `atomic_write`), only on a real (non-dry-run) write. A destination that exists with
   _different_ bytes (effectively impossible without external tampering/hash collision) is reported as a `write_failure`
   for that PDF, not a silent overwrite.
9. Safety preflights:
   - `validate_output_collisions`: extend so two PDFs that would write the same asset destination are reported alongside
     note-path collisions (the per-note `*.assets/` scheme already prevents this in practice, but the guard stays
     honest).
   - `ensure_safe_to_write`: include planned asset destinations in the touched paths so a dirty/modified existing asset
     under our control is refused before any write, consistent with the note/PDF dirty-refusal model. New (untracked,
     non-existent) destinations are created normally.
   - `--dry-run` performs resolution + hashing (so it can preview and surface plan errors) but writes nothing — no dirs,
     no files.

### E. Scan / sync output (beautiful + useful)

10. Per-PDF `scan_details`: when images are present, append a dim `N image(s)` breakdown after the existing
    `N highlights` umbrella (e.g. `created note · 8 highlights · 2 images`). `sync --dry-run` / `sync` likewise report
    an `images:`/`image_assets:` line. The scan summary footer gains a total `images` (and `image assets written`) tally
    so bulk runs are auditable. Colors reuse the existing `Styler` (green/cyan/dim), honoring the CLI rule to prefer
    beautiful colored output.

### F. Documentation and fixtures

11. `docs/highlights-ref-sync.md`: add an **Image Selections** subsection under "Generated Body Contract" (with the
    rendered example above), document the TextBundle-only constraint in the MacBook setup guide ("enable TextBundle
    export to sync area selections"), note the content-addressed `*.assets/` layout and the left-in-place orphan
    behavior, and add Expected-Failures rows (`image asset not found`, multi-PDF asset collision).
12. Fixtures (`tests/fixtures/highlights_ref/`): add a small synthetic image asset and an `image_sidecar.md` documenting
    the shape; integration tests in `tests/cli.rs` build a real `.textbundle` (text + `assets/` PNG) at runtime,
    matching the existing "synthetic at runtime" fixture philosophy.

## Testing Strategy

- **Parser unit tests:** an `![](assets/x.png)` chunk → `Image` annotation; image-with-comment attaches the comment and
  extracts `#task`/`@route`; a `![](x.pdf)`-style non-image target falls through to note text.
- **Identity stability:** same image bytes under a _renamed_ asset file → same `^h-<id>`; inserting annotations above an
  image does not change its ID; two identical images dedupe to one block.
- **Rendering:** image callout + embed path + nested comment; removed image → tombstone while its prior block ID is
  preserved.
- **Write/plan reliability:** asset copy entry is planned; idempotent skip when destination exists; `--dry-run` writes
  nothing yet previews counts and surfaces a missing-asset `plan_error`; collision and dirty-asset preflight refusals;
  per-PDF error isolation (one bad image doesn't stop other PDFs).
- **Integration (`tests/cli.rs`):** runtime-built `.textbundle` with an image → `scan` creates the note, writes
  `…/<stem>.assets/h-<id>.png`, embeds it, and a second unchanged run reports `writes: none`.

## Out of Scope / Future Work

- Garbage-collecting orphaned asset files when an image annotation is removed (MVP leaves them; tombstone remains a text
  warning).
- Re-compressing / re-encoding images (assets are copied byte-for-byte).
- Writing image annotations back into the PDF/marker (image sync stays one-way: PDF/sidecar → reference note, like
  highlights and standalone notes).
- A `--no-images` / custom assets-dir flag (can be added later if a real need appears; omitted now to keep the default
  path intuitive).

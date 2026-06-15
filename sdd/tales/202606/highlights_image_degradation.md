---
create_time: 2026-06-15 10:00:59
status: wip
prompt: sdd/prompts/202606/highlights_image_degradation.md
---
# Fix: an unresolvable image selection must not block the whole reference note

## Context

`bob highlights scan`/`sync` turn Highlights-app PDF annotations into Obsidian reference notes. The recently shipped
image-selection feature (commit `2400274`, plan `sdd/tales/202606/highlights_image_selections.md`) parses Markdown image
links (`![](assets/x.png)`) out of a sidecar, copies the asset into a per-note `*.assets/` folder, and renders an
`[!quote] Image` callout.

The user reports two symptoms on `~/bob/lib/docs/gastown_readme.pdf`:

1. Its image / area selection "doesn't get processed" by `bob highlights scan`.
2. Worse, that image selection causes **new or edited highlights/notes to stop being reflected** in the reference note
   `~/bob/ref/docs/gastown_readme.md`.

## Root Cause (verified by reproduction)

Image-asset resolution failure is treated as a **fatal, whole-PDF error**, so one unresolvable image discards every
other annotation for that PDF.

Concretely, in `src/native/highlights_ref/mod.rs`:

- `plan_pdf_sync` calls `render_sidecar_highlights(...)` and propagates any error with `?` (around line 884).
- `render_sidecar_highlights` calls `resolve_sidecar_image_assets(...)?` (around line 3272–3273).
- `resolve_sidecar_image_assets` returns a hard `CommandError` the moment an image asset cannot be found or read
  (`image asset not found: ... - export the sidecar as a TextBundle so images are included`, around line 3141–3153). (A
  second fatal point exists at line 3298–3300, "image annotation was not resolved".)

The error bubbles all the way up and turns the **entire** PDF into a `plan_error`. The reference note is never written,
so all of that PDF's text highlights, comments, standalone notes — and any edits to them — silently fail to sync. The
image both "doesn't get processed" (it errored) and "blocks everything else" (the note write is aborted).

This was an intentional but too-coarse design choice in the original plan (§B step 3: "Missing/unreadable files produce
a clear `CommandError` → per-PDF `plan_error`"). Per-PDF isolation is the right granularity _between_ PDFs; it is the
wrong granularity _within_ a PDF, where it sacrifices unrelated annotations for one bad image.

### Evidence

Investigated against the real PDF and reproduced in an isolated temp vault:

- `~/bob/ref/docs/gastown_readme.md` records `highlights_sidecar: lib/docs/gastown_readme.md` (a plain `.md` sidecar)
  and `highlights_count: 10` from a prior good sync. The PDF carries a `/Square` annotation (Highlights' area/image
  selection) plus 8 `/Highlight` annotations.
- Plain `.md` sidecar containing `![](assets/area-selection.png)` with the asset **absent** (the realistic
  plain-Markdown-export case): `sync --dry-run` and `scan --dry-run` both abort that PDF with
  `plan_error: image asset not found ...`; `notes_update: 0`, `result: partial-failure`. The valid `Town` highlight in
  the same sidecar is dropped.
- Same sidecar with the image line **removed**: highlights sync normally (`note_action: update`). This isolates the
  image as the sole culprit.
- A valid **TextBundle** with the asset present: works correctly — `images: 1`, asset copied to
  `ref/docs/gastown_readme.assets/h-<id>.png`, rendered as `> [!quote] Image ![[...]]`. So the feature's happy path is
  sound; the defect is purely the fatal error handling.

The most likely real-world trigger: Highlights' **plain-Markdown export** keeps the `![](...)` link but does not write
the asset bytes (its "Markdown strips images" behavior), so resolution fails and nukes the note. The fix must be robust
regardless of the exact export quirk — any unresolvable image link (plain `.md`, renamed/missing TextBundle asset)
currently destroys the note.

## Goal

An unresolvable image selection must **degrade gracefully within the note**, never block it:

- All text highlights, comments, and standalone notes (and edits to them) for that PDF keep syncing.
- The unresolved image stays **visible and discoverable** in the note, with actionable guidance (re-export as
  TextBundle), and preserves its comment / `#task` affordances.
- `scan`/`sync` output surfaces the unresolved image clearly and auditable-y, reusing the existing colored `Styler`.
- The existing happy path (resolvable TextBundle image) is unchanged: same block IDs, same asset copies, same render.

## Design Decisions

### 1. Image-resolution failure is per-image and non-fatal

`resolve_sidecar_image_assets` stops returning a hard `CommandError` for not-found / unreadable / non-relative targets.
Instead it classifies each `Image` annotation as **resolved** (asset write planned, content-addressed block ID as today)
or **unresolved** (carry a short reason string). Multiple images in one PDF are handled independently — a bad one never
suppresses a good one. Truly exceptional, non-image errors keep propagating; only the "this specific image could not be
resolved" cases degrade.

### 2. Unresolved images render an in-note placeholder, not a hard stop

An unresolved image renders inside the same callout family as a clear placeholder, e.g.:

```md
> [!warning] Image not synced — re-export this PDF as a TextBundle so the area selection is included.
>
> > [!note] Comment <original comment, if any>

^h-<stable-id>
```

This keeps the annotation visible in Obsidian, keeps the comment/`#task` pipeline intact, and tells the user exactly how
to fix it. No asset copy is planned for an unresolved image.

### 3. Stable fallback block ID for unresolved images

A content-addressed ID needs bytes we do not have, so unresolved images use a deterministic fallback derived from
`(source_pdf · "image-unresolved" · normalized target · page label · ordinal)`. This stays stable across repeated runs
while the image remains unresolved. When the asset later appears (TextBundle re-export), the block naturally becomes the
real content-addressed `^h-<contenthash>` and the placeholder is tombstoned under `### Removed highlights` — consistent
with how editing a highlight mints a new ID and tombstones the old one.

### 4. Surface unresolved images in output (no new flags)

Reuse the existing reporting/`Styler` plumbing:

- Per-PDF: an `images_unresolved: N` line (and the existing image breakdown) in verbose scan and `sync` dry-run/real
  output, plus a dim warning line per unresolved image with its reason.
- Scan summary: an `images unresolved` tally so bulk runs stay auditable.
- The note still writes, so an unresolved-image-only PDF is a **non-fatal warning**, not a `plan_failure`. This is the
  crux of the fix: scheduled scans keep updating notes and reporting the warning instead of perpetually failing and
  freezing the note. (This relaxation of today's hard-failure contract is the main reviewable decision; called out
  explicitly for challenge at review.)
- No new CLI subcommands or options — consistent with the feature's flag-free design and the CLI rule to keep output
  beautiful and colored.

## Implementation Plan

All code in `src/native/highlights_ref/mod.rs`, plus docs/tests.

### A. Make resolution per-image and non-fatal

- Rework `resolve_sidecar_image_assets` to return, per `Image` annotation, either a planned `ImageAssetWrite` (resolved)
  or an unresolved marker with a reason. Drop the `?`-on-first-failure behavior for not-found / unreadable /
  non-relative-target cases; keep genuinely exceptional errors propagating.
- Add a fallback `image_unresolved_block_id` (decision §3) alongside the existing `image_annotation_block_id`.

### B. Render gracefully

- In `render_sidecar_highlights`, remove the fatal `ok_or_else(... "image annotation was not resolved")` path; route
  unresolved images to the placeholder renderer using the fallback ID, still emitting the nested comment callout and
  feeding `annotation_task_candidates` so `#task`/`@route` on an image comment is not lost.
- Add an `Image`-unresolved arm (or a small placeholder branch) in/near `render_annotation_block`.
- Extend `RenderedHighlights` with an `images_unresolved` count (and reasons as needed); keep `image_count` meaning
  "resolved images rendered as embeds".

### C. Plumb reporting (decision §4)

- Thread the unresolved count/reasons through `PdfSyncPlan` → `SyncWriteReport` → `scan_details` / scan summary / `sync`
  dry-run + real output, reusing `Styler`. Ensure an unresolved-image-only PDF is reported as a warning and does **not**
  increment `plan_failures` / make the run exit non-zero on its own.

### D. Docs

- Update `docs/highlights-ref-sync.md`: replace the "image asset not found → per-PDF failure" contract with the
  graceful-degradation behavior (placeholder block, preserved comments/tasks, warning + tally, note still writes).
  Update the Expected-Failures table row for `image asset not found` accordingly, and the Generated Body Contract /
  MacBook validation notes.

### E. Tests

- Update the existing unit test that asserts `render_sidecar_highlights` errors on a missing image to instead assert
  graceful degradation (placeholder block + other annotations preserved).
- New coverage: plain `.md` image link with missing asset → note still writes with all text highlights + an
  `images_unresolved` placeholder; mixed resolved+unresolved images in one PDF; `scan` reports the PDF as a warning (not
  a `plan_failure`) and still writes other PDFs; a later TextBundle re-export resolves the image and tombstones the
  placeholder; valid-TextBundle happy path unchanged (regression guard).
- `tests/cli.rs` integration mirroring the reproduced scenario end-to-end.

## Out of Scope / Future Work

- **Extracting the area-selection image directly from the PDF `/Square` annotation** so images sync without a
  TextBundle. This is an alternative reading of "the image selection doesn't work," but it is a large new capability
  (rasterizing a PDF region; `lopdf` does not render) and contradicts the feature's deliberate "TextBundle is the source
  of truth for images" design. Noted as a possible larger direction, not part of this bugfix.
- Garbage-collecting orphaned asset files (already future work in the original plan).
- The pre-existing quirk where a marker mirror can render as a `[!note]` (visible in the current gastown ref note) is
  unrelated to this bug and left untouched.

## Risks / Validation

- Relaxing the hard-failure contract is the key behavioral change; mitigated by keeping the unresolved image loud
  (in-note placeholder + colored warning + summary tally) so it is not silently ignored.
- Validate on the real `gastown_readme.pdf` once its sidecar is present: confirm text highlights/edits sync and the
  image shows a placeholder; then confirm a TextBundle re-export upgrades the placeholder to a real embed.
